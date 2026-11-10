//! Presets: partial `tabg.toml`s. Built-ins ship inside the binary (from `presets/`); saved ones
//! live in the data dir. Applying one merges its `config` table over the current config, so name,
//! network, admins and mods stay as they are unless the preset sets them.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::ServerToml;

#[derive(Clone, Debug, Serialize)]
pub struct Preset {
    pub id: String,
    pub title: String,
    pub description: String,
    pub builtin: bool,
    #[serde(skip)]
    pub config: toml::Table,
}

#[derive(Deserialize)]
struct PresetFile {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    config: toml::Table,
}

const BUILTIN: &[(&str, &str)] = &[
    (
        "battle-royale",
        include_str!("../../../presets/battle-royale.toml"),
    ),
    (
        "solo-royale",
        include_str!("../../../presets/solo-royale.toml"),
    ),
    (
        "quick-match",
        include_str!("../../../presets/quick-match.toml"),
    ),
    (
        "loot-frenzy",
        include_str!("../../../presets/loot-frenzy.toml"),
    ),
    (
        "armed-drop",
        include_str!("../../../presets/armed-drop.toml"),
    ),
    (
        "deathmatch",
        include_str!("../../../presets/deathmatch.toml"),
    ),
    (
        "industry-deathmatch",
        include_str!("../../../presets/industry-deathmatch.toml"),
    ),
    ("gun-game", include_str!("../../../presets/gun-game.toml")),
    (
        "shooting-range",
        include_str!("../../../presets/shooting-range.toml"),
    ),
    (
        "island-gun-game",
        include_str!("../../../presets/island-gun-game.toml"),
    ),
    ("brawl", include_str!("../../../presets/brawl.toml")),
    ("bomb", include_str!("../../../presets/bomb.toml")),
    (
        "flying-vehicles",
        include_str!("../../../presets/flying-vehicles.toml"),
    ),
    (
        "capture-the-princess",
        include_str!("../../../presets/capture-the-princess.toml"),
    ),
];

fn parse(id: &str, text: &str, builtin: bool) -> Result<Preset, String> {
    let f: PresetFile = toml::from_str(text).map_err(|e| format!("preset {id}: {e}"))?;
    Ok(Preset {
        id: id.to_owned(),
        title: f.title,
        description: f.description,
        builtin,
        config: f.config,
    })
}

/// Built-ins first, then saved presets from `dir` (a saved preset may not reuse a built-in id).
pub fn all(dir: &Path) -> Vec<Preset> {
    let mut out: Vec<Preset> = BUILTIN
        .iter()
        .map(|(id, text)| parse(id, text, true).expect("built-in presets are valid"))
        .collect();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut saved: Vec<Preset> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let id = path.file_stem()?.to_str()?.to_owned();
            if path.extension()? != "toml" || out.iter().any(|p| p.id == id) {
                return None;
            }
            match parse(&id, &std::fs::read_to_string(&path).ok()?, false) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!("{e}");
                    None
                }
            }
        })
        .collect();
    saved.sort_by(|a, b| a.id.cmp(&b.id));
    out.extend(saved);
    out
}

pub fn find(dir: &Path, id: &str) -> Option<Preset> {
    all(dir).into_iter().find(|p| p.id == id)
}

fn merge(into: &mut toml::Table, from: &toml::Table) {
    for (k, v) in from {
        match (into.get_mut(k), v) {
            (Some(toml::Value::Table(a)), toml::Value::Table(b)) => merge(a, b),
            _ => {
                into.insert(k.clone(), v.clone());
            }
        }
    }
}

/// `config` with the preset applied.
pub fn apply(config: &ServerToml, preset: &Preset) -> Result<ServerToml, String> {
    let mut table = toml::Table::try_from(config).map_err(|e| e.to_string())?;
    merge(&mut table, &preset.config);
    table.insert("preset".into(), toml::Value::String(preset.id.clone()));
    toml::Value::Table(table)
        .try_into()
        .map_err(|e: toml::de::Error| format!("preset {}: {}", preset.id, e.message()))
}

/// Save the game rules of `config` (game, ring, loadout) as a preset in `dir`.
pub fn save(
    dir: &Path,
    id: &str,
    title: &str,
    description: &str,
    config: &ServerToml,
) -> Result<(), String> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("preset ids use lowercase letters, digits and dashes".into());
    }
    if BUILTIN.iter().any(|(b, _)| *b == id) {
        return Err(format!("{id} is a built-in preset; pick another id"));
    }
    let mut cfg = toml::Table::new();
    let full = toml::Table::try_from(config).map_err(|e| e.to_string())?;
    for key in ["game", "ring", "rules", "loadout"] {
        if let Some(v) = full.get(key) {
            cfg.insert(key.into(), v.clone());
        }
    }
    let mut file = toml::Table::new();
    file.insert("title".into(), title.into());
    file.insert("description".into(), description.into());
    file.insert("config".into(), toml::Value::Table(cfg));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let text = toml::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{id}.toml")), text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Teams;

    #[test]
    fn every_preset_file_is_built_in() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../presets");
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.path().file_stem()?.to_str().map(str::to_owned))
            .collect();
        files.sort();
        let mut builtin: Vec<String> = BUILTIN.iter().map(|(id, _)| (*id).to_owned()).collect();
        builtin.sort();
        assert_eq!(builtin, files);
    }

    #[test]
    fn builtins_parse_and_produce_valid_configs() {
        let dir = tempfile::tempdir().unwrap();
        for p in all(dir.path()) {
            let c = apply(&ServerToml::default(), &p).unwrap();
            assert!(c.validate().is_empty(), "{}: {:?}", p.id, c.validate());
            assert_eq!(c.preset, p.id);
        }
    }

    #[test]
    fn apply_keeps_what_the_preset_doesnt_set() {
        let dir = tempfile::tempdir().unwrap();
        let c = ServerToml {
            name: "friday".into(),
            players: crate::config::Players {
                admins: vec!["me".into()],
                banned: Vec::new(),
            },
            ..Default::default()
        };
        let quick = find(dir.path(), "quick-match").unwrap();
        let after = apply(&c, &quick).unwrap();
        assert_eq!(after.name, "friday");
        assert_eq!(after.players.admins, vec!["me".to_string()]);
        assert_eq!(after.game.teams, Teams::Solo);
        assert_eq!(after.ring.times.len(), 6);
        let armed = apply(&after, &find(dir.path(), "armed-drop").unwrap()).unwrap();
        assert_eq!(armed.loadout.len(), 4);
        assert_eq!(
            armed.ring.times.len(),
            6,
            "armed-drop leaves the ring alone"
        );
    }

    #[test]
    fn save_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ServerToml::default();
        c.game.countdown = 5.0;
        save(dir.path(), "mine", "Mine", "test", &c).unwrap();
        assert!(save(dir.path(), "battle-royale", "x", "", &c).is_err());
        assert!(save(dir.path(), "Bad Id", "x", "", &c).is_err());
        let p = find(dir.path(), "mine").unwrap();
        assert!(!p.builtin);
        let applied = apply(&ServerToml::default(), &p).unwrap();
        assert_eq!(applied.game.countdown, 5.0);
    }
}
