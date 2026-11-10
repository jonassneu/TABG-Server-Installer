//! Share codes and `tabg join` (§6): the host lists the client mods friends need; a share code
//! carries them (catalog mods by id, version and sha256; the host's own DLLs by sha256 plus the
//! host's sync address) and the join code. `tabg join <share code>` makes a modded copy of the
//! friend's TABG with exactly those mods, every file hash-checked.

use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use serde::{Deserialize, Serialize};

use crate::config::ServerToml;
use crate::fetch::{self, sha256_file, sha256_hex};
use crate::paths::Paths;
use crate::setup::Install;

/// Where catalog mods are published (one rolling GitHub release).
pub const CATALOG_BASE: &str =
    "https://github.com/jonassneu/TABG-Server-Installer/releases/download/mods";
const PREFIX: &str = "TABG1-";

/// BepInEx 5.4.23.3 for Windows (the client is a Windows build, also under Proton).
const BEPINEX_WIN: (&str, &str) = (
    "https://github.com/BepInEx/BepInEx/releases/download/v5.4.23.3/BepInEx_win_x64_5.4.23.3.zip",
    "41a089e5b1b1f0713b331346baf6677b1184c69eabebf51101097954e854c749",
);
/// Lets the game start without Easy Anti-Cheat (C0mputery/AntiCheatBootErrorRemover 1.0, Apache-2.0).
const EAC_REMOVER: (&str, &str, &str) = (
    "https://github.com/C0mputery/AntiCheatBootErrorRemover/releases/download/1.0/AntiCheatBootErrorRemover.dll",
    "4dd862aea061519cad2aeab56472797c8f4fdad0a9f595f6585debe7ce714e8e",
    "AntiCheatBootErrorRemover.dll",
);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    /// The BepInEx plugin GUID.
    #[serde(default)]
    pub guid: String,
    /// GUIDs of plugins this one needs (installed with it).
    #[serde(default)]
    pub depends: Vec<String>,
    pub version: String,
    /// File name in the release (also the installed name).
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedMod {
    pub id: String,
    pub version: String,
    pub file: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedFile {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ShareCode {
    /// Join code or `ip:port`, when the server was running.
    #[serde(rename = "j", default, skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    #[serde(rename = "m", default, skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<SharedMod>,
    #[serde(rename = "f", default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SharedFile>,
    /// The host's sync endpoint for `files`, e.g. `http://1.2.3.4:7781`.
    #[serde(rename = "h", default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

impl ShareCode {
    pub fn encode(&self) -> String {
        format!(
            "{PREFIX}{}",
            B64.encode(serde_json::to_vec(self).expect("serializes"))
        )
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let body = text
            .trim()
            .strip_prefix(PREFIX)
            .ok_or("not a tabg share code (they start with TABG1-)")?;
        let json = B64
            .decode(body)
            .map_err(|_| "the share code is damaged (copy all of it)")?;
        serde_json::from_slice(&json).map_err(|e| format!("bad share code: {e}"))
    }
}

/// The mod catalog: `TABG_MOD_CATALOG` (a local catalog.json, files next to it) or the release.
pub fn load_catalog(paths: &Paths) -> Result<(Vec<CatalogEntry>, String), String> {
    if let Some(local) = std::env::var_os("TABG_MOD_CATALOG") {
        let path = PathBuf::from(local);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let base = path
            .parent()
            .map(|p| format!("file://{}", p.display()))
            .unwrap_or_default();
        return Ok((
            serde_json::from_str(&text).map_err(|e| e.to_string())?,
            base,
        ));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build()
        .into();
    let text = agent
        .get(format!("{CATALOG_BASE}/catalog.json"))
        .call()
        .and_then(|mut r| r.body_mut().read_to_string())
        .map_err(|e| format!("couldn't get the mod catalog: {e}"))?;
    let _ = paths;
    Ok((
        serde_json::from_str(&text).map_err(|e| e.to_string())?,
        CATALOG_BASE.to_owned(),
    ))
}

/// The host side of a share: the code, plus which local file serves which hash.
#[derive(Clone, Debug, Default)]
pub struct Pack {
    pub code: ShareCode,
    pub files: Vec<(String, PathBuf)>,
}

impl Pack {
    pub fn file(&self, sha256: &str) -> Option<&Path> {
        self.files
            .iter()
            .find(|(s, _)| s == sha256)
            .map(|(_, p)| p.as_path())
    }
}

fn add_with_dependencies(
    mods: &mut Vec<SharedMod>,
    catalog: &[CatalogEntry],
    entry: &CatalogEntry,
) -> Result<(), String> {
    if mods.iter().any(|m| m.id == entry.id) {
        return Ok(());
    }
    mods.push(SharedMod {
        id: entry.id.clone(),
        version: entry.version.clone(),
        file: entry.file.clone(),
        sha256: entry.sha256.clone(),
    });
    for guid in &entry.depends {
        let dep = catalog
            .iter()
            .filter(|c| &c.guid == guid)
            .max_by(|a, b| a.version.cmp(&b.version))
            .ok_or_else(|| format!("{} needs {guid}, which is not in the catalog", entry.id))?;
        add_with_dependencies(mods, catalog, dep)?;
    }
    Ok(())
}

fn is_file_entry(entry: &str) -> bool {
    entry.to_ascii_lowercase().ends_with(".dll")
}

/// Build the share for `config.client_mods()`: `id@version` / `id` from the catalog, or a path
/// to a DLL of your own (relative paths are relative to the server folder).
pub fn build(
    paths: &Paths,
    config: &ServerToml,
    join: Option<String>,
    sync: Option<String>,
) -> Result<Pack, String> {
    let mut pack = Pack::default();
    pack.code.join = join;
    let client = config.client_mods();
    let catalog_needed = client.iter().any(|e| !is_file_entry(e));
    let catalog = if catalog_needed {
        load_catalog(paths)?.0
    } else {
        Vec::new()
    };
    for entry in &client {
        if is_file_entry(entry) {
            let path = Path::new(entry);
            let path = if path.is_absolute() {
                path.to_owned()
            } else {
                paths.server().join(path)
            };
            let sha256 = sha256_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("bad file name")?
                .to_owned();
            pack.code.files.push(SharedFile {
                name,
                sha256: sha256.clone(),
                size,
            });
            pack.files.push((sha256, path));
            continue;
        }
        let (id, version) = entry.split_once('@').unwrap_or((entry, ""));
        let found = catalog
            .iter()
            .filter(|c| c.id == id && (version.is_empty() || c.version == version))
            .max_by(|a, b| a.version.cmp(&b.version))
            .ok_or_else(|| format!("{entry} is not in the mod catalog"))?;
        add_with_dependencies(&mut pack.code.mods, &catalog, found)?;
    }
    if !pack.code.files.is_empty() {
        pack.code.host = sync;
    }
    Ok(pack)
}

// ---------------------------------------------------------------------------------------------
// Friend side.

/// What tabg installed into the client copy, so a later join removes exactly those files.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Manifest {
    plugins: Vec<String>,
    share: Option<ShareCode>,
}

const MANIFEST: &str = "tabg-client.json";
/// Never copied into the modded copy.
const CLIENT_SKIP: &[&str] = &[
    "EasyAntiCheat",
    "BepInEx",
    "winhttp.dll",
    "doorstop_config.ini",
    ".doorstop_version",
];

fn link_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            link_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if dst.exists() {
        // Same file already (hard link from an earlier join)?
        if std::fs::metadata(dst)?.len() == std::fs::metadata(src)?.len() {
            return Ok(());
        }
        std::fs::remove_file(dst)?;
    }
    std::fs::hard_link(src, dst).or_else(|_| std::fs::copy(src, dst).map(|_| ()))
}

fn fetch_to(url: &str, sha256: &str, cache: &Path) -> Result<Vec<u8>, String> {
    if let Some(path) = url.strip_prefix("file://") {
        let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        if sha256_hex(&data) != sha256 {
            return Err(format!("{path}: wrong sha256"));
        }
        return Ok(data);
    }
    let path = fetch::download(url, sha256, cache)?;
    std::fs::read(path).map_err(|e| e.to_string())
}

/// The result of a join: where the modded copy is and what to type in the game.
#[derive(Clone, Debug, Serialize)]
pub struct Joined {
    pub copy: PathBuf,
    pub exe: PathBuf,
    pub join: Option<String>,
    pub installed: Vec<String>,
}

/// The name a catalog file is installed under: its assembly's (`Tabg.ModSettings-1.0.0.dll` →
/// `Tabg.ModSettings.dll`). .NET finds a referenced assembly by that name, and mods that use
/// another mod's types (FlyingControls uses ModSettings) fail to load if it's still versioned.
fn assembly_file(file: &str) -> String {
    let Some(stem) = file.strip_suffix(".dll") else {
        return file.to_owned();
    };
    match stem.rsplit_once('-') {
        Some((name, version))
            if !name.is_empty()
                && !version.is_empty()
                && version
                    .split('.')
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) =>
        {
            format!("{name}.dll")
        }
        _ => file.to_owned(),
    }
}

/// Make (or update) the modded copy of TABG for `code`.
pub fn join(paths: &Paths, code: &str, progress: &mut dyn FnMut(&str)) -> Result<Joined, String> {
    let share = ShareCode::decode(code)?;
    let mut install = Install::load(paths);
    install.detect();
    let client = install
        .client_dir
        .clone()
        .ok_or("TABG not found; install it with Steam first")?;
    let copy = paths.client_copy();
    progress(&format!(
        "preparing a modded copy of TABG in {}",
        copy.display()
    ));
    std::fs::create_dir_all(&copy).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(&client)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let name = entry.file_name();
        if CLIENT_SKIP.iter().any(|s| name == *s) {
            continue;
        }
        link_tree(&entry.path(), &copy.join(&name)).map_err(|e| format!("copy {name:?}: {e}"))?;
    }
    std::fs::write(copy.join("steam_appid.txt"), "823130").map_err(|e| e.to_string())?;

    if !copy.join("BepInEx/core/BepInEx.Preloader.dll").exists() {
        progress("installing BepInEx 5.4.23.3");
        let zip = fetch::download(BEPINEX_WIN.0, BEPINEX_WIN.1, &paths.downloads())?;
        fetch::unzip(&zip, &copy)?;
    }
    let plugins = copy.join("BepInEx/plugins");
    std::fs::create_dir_all(&plugins).map_err(|e| e.to_string())?;

    // Remove what an earlier join installed, then install exactly this pack.
    let manifest_path = copy.join(MANIFEST);
    let old: Manifest = std::fs::read(&manifest_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    for f in &old.plugins {
        let _ = std::fs::remove_file(plugins.join(f));
    }
    let cache = paths.downloads();
    let mut installed = Vec::new();
    let mut put = |name: &str, data: &[u8]| -> Result<(), String> {
        if name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(format!("refusing file name {name:?}"));
        }
        std::fs::write(plugins.join(name), data).map_err(|e| e.to_string())?;
        installed.push(name.to_owned());
        Ok(())
    };
    let remover = fetch_to(EAC_REMOVER.0, EAC_REMOVER.1, &cache)?;
    put(EAC_REMOVER.2, &remover)?;
    let base = if share.mods.is_empty() {
        String::new()
    } else {
        load_catalog(paths)
            .map(|(_, base)| base)
            .unwrap_or_else(|_| CATALOG_BASE.to_owned())
    };
    for m in &share.mods {
        progress(&format!("getting {} {}", m.id, m.version));
        let data = fetch_to(&format!("{base}/{}", m.file), &m.sha256, &cache)?;
        put(&assembly_file(&m.file), &data)?;
    }
    if !share.files.is_empty() {
        let host = share
            .host
            .as_deref()
            .ok_or("the share code lists host files but no host address")?;
        for f in &share.files {
            progress(&format!("getting {} from the host", f.name));
            let data = fetch_to(&format!("{host}/f/{}", f.sha256), &f.sha256, &cache)?;
            put(&f.name, &data)?;
        }
    }
    let manifest = Manifest {
        plugins: installed.clone(),
        share: Some(share.clone()),
    };
    crate::paths::write_atomic(
        &manifest_path,
        &serde_json::to_vec_pretty(&manifest).expect("serializes"),
    )
    .map_err(|e| e.to_string())?;
    install.save(paths).map_err(|e| e.to_string())?;
    Ok(Joined {
        exe: copy.join("TotallyAccurateBattlegrounds.exe"),
        copy,
        join: share.join,
        installed,
    })
}

/// Start the modded copy. Steam must be running (TABG needs it for the login).
pub fn launch(paths: &Paths) -> Result<(), String> {
    let copy = paths.client_copy();
    let exe = copy.join("TotallyAccurateBattlegrounds.exe");
    if !exe.exists() {
        return Err("no modded copy yet; run `tabg join <share code>` first".into());
    }
    #[cfg(windows)]
    {
        std::process::Command::new(&exe)
            .current_dir(&copy)
            .spawn()
            .map_err(|e| format!("start TABG: {e}"))?;
    }
    #[cfg(not(windows))]
    {
        let proton = crate::steam::libraries()
            .into_iter()
            .map(|l| l.join("steamapps/common/Proton - Experimental/proton"))
            .find(|p| p.exists())
            .ok_or("Proton Experimental not found (install it in Steam: Library → Tools)")?;
        let steam_root = crate::steam::libraries()
            .into_iter()
            .find(|l| l.join("steamapps/compatdata").is_dir())
            .ok_or("no Steam folder found")?;
        let compat = steam_root.join("steamapps/compatdata/823130");
        std::process::Command::new(proton)
            .arg("run")
            .arg(&exe)
            .current_dir(&copy)
            .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam_root)
            .env("STEAM_COMPAT_DATA_PATH", compat)
            .env("STEAM_COMPAT_APP_ID", "823130")
            .env("SteamAppId", "823130")
            .env("SteamGameId", "823130")
            .env("WINEDLLOVERRIDES", "winhttp=n,b")
            .spawn()
            .map_err(|e| format!("start TABG with Proton: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_files_install_under_their_assembly_name() {
        assert_eq!(
            assembly_file("Tabg.ModSettings-1.0.0.dll"),
            "Tabg.ModSettings.dll"
        );
        assert_eq!(
            assembly_file("Tabg.AdminRadar.Client-2.0.0.dll"),
            "Tabg.AdminRadar.Client.dll"
        );
        assert_eq!(assembly_file("Tabg.Visuals-1.7.0.dll"), "Tabg.Visuals.dll");
        assert_eq!(
            assembly_file("Tabg.Foo-bar.dll"),
            "Tabg.Foo-bar.dll",
            "not a version"
        );
        assert_eq!(assembly_file("My.Mod.dll"), "My.Mod.dll");
    }

    #[test]
    fn share_codes_round_trip_and_reject_garbage() {
        let code = ShareCode {
            join: Some("AB12CD".into()),
            mods: vec![SharedMod {
                id: "coords-display".into(),
                version: "1.0.0".into(),
                file: "Tabg.CoordsDisplay-1.0.0.dll".into(),
                sha256: "00".repeat(32),
            }],
            files: Vec::new(),
            host: None,
        };
        let text = code.encode();
        assert!(text.starts_with("TABG1-"));
        assert_eq!(ShareCode::decode(&text).unwrap(), code);
        assert!(ShareCode::decode("TABG1-!!!").is_err());
        assert!(ShareCode::decode("hello").is_err());
    }

    #[test]
    fn catalog_mods_bring_their_dependencies() {
        let entry = |id: &str, guid: &str, depends: &[&str]| CatalogEntry {
            id: id.into(),
            name: id.into(),
            guid: guid.into(),
            depends: depends.iter().map(|d| (*d).into()).collect(),
            version: "1.0.0".into(),
            file: format!("{id}.dll"),
            sha256: "00".repeat(32),
            size: 1,
        };
        let catalog = vec![
            entry("coords-display", "x.coords", &["x.settings"]),
            entry("mod-settings", "x.settings", &[]),
        ];
        let mut mods = Vec::new();
        add_with_dependencies(&mut mods, &catalog, &catalog[0]).unwrap();
        add_with_dependencies(&mut mods, &catalog, &catalog[1]).unwrap();
        let ids: Vec<_> = mods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["coords-display", "mod-settings"]);
        let broken = vec![entry("lonely", "x.lonely", &["x.missing"])];
        assert!(add_with_dependencies(&mut Vec::new(), &broken, &broken[0]).is_err());
    }

    #[test]
    fn build_hashes_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().to_owned());
        std::fs::create_dir_all(paths.server()).unwrap();
        std::fs::write(paths.server().join("Mine.dll"), b"dll bytes").unwrap();
        let mut config = ServerToml::default();
        config.mods.client = vec!["Mine.dll".into()];
        let pack = build(
            &paths,
            &config,
            Some("CODE12".into()),
            Some("http://h:7781".into()),
        )
        .unwrap();
        assert_eq!(pack.code.files.len(), 1);
        assert_eq!(pack.code.files[0].sha256, sha256_hex(b"dll bytes"));
        assert_eq!(pack.code.host.as_deref(), Some("http://h:7781"));
        assert!(pack.file(&sha256_hex(b"dll bytes")).is_some());
        assert!(pack.file("nope").is_none());
    }
}
