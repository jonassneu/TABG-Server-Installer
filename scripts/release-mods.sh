#!/usr/bin/env bash
# Build the client mods and write the catalog tabg uses for share codes (§7):
# target/mods-dist/catalog.json + Tabg.<Mod>-<version>.dll, all sha256-pinned.
#
#   scripts/release-mods.sh            build only (test with TABG_MOD_CATALOG=target/mods-dist/catalog.json)
#   scripts/release-mods.sh --upload   also upload new files and the catalog to the "mods" GitHub release (gh CLI);
#                                      released files are never replaced, so bump a mod's version to ship it
#
# Needs the .NET SDK and TABG_CLIENT_DIR (see mods/Local.props.example).
set -euo pipefail
cd "$(dirname "$0")/.."
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1
[ -d "$HOME/.dotnet" ] && export PATH="$HOME/.dotnet:$PATH"

# Mods friends can install. Dev tools (DevAutoJoin, WorldDumper) are not part of the catalog.
MODS=(AdminRadar.Client CoordsDisplay CustomGameSkins.Client CustomGrenades Destruction DevTestMap.Client
      FlyingControls ModSettings PerformanceClient PopupBlocker PrincessMode ProximityChat.Client RangeMap.Client
      Visuals)

# Visuals embeds the shader bundle that Unity compiles (needs the editor + a licence, see there).
[[ -f target/bundles/visuals ]] || scripts/build-shaders.sh

OUT=target/mods-dist
rm -rf "$OUT"; mkdir -p "$OUT"
entries=()
for m in "${MODS[@]}"; do
  dotnet build -v q -c Release "mods/src/$m" >/dev/null || { dotnet build -c Release "mods/src/$m" | grep -E " error " | sort -u; exit 1; }
  dll="mods/src/$m/bin/Release/netstandard2.0/Tabg.$m.dll"
  # The version is the BepInPlugin attribute's third argument, or a PluginVersion constant.
  version=$( { grep -rhoE 'BepInPlugin\("[^"]+", "[^"]+", "[0-9.]+"' "mods/src/$m" || grep -rhoE 'PluginVersion = "[0-9.]+"' "mods/src/$m" || true; } \
    | head -1 | grep -oE '[0-9.]+"$' | tr -d '"' || true)
  version=${version:-0.0.0}
  guid=$( { grep -rhoE 'BepInPlugin\("[^"]+"' "mods/src/$m" || grep -rhoE 'PluginGuid = "[^"]+"' "mods/src/$m" || true; } | head -1 | grep -oE '"[^"]+"$' | tr -d '"' || true)
  # Hard dependencies on other plugins (soft ones are optional and left out).
  deps=$(python3 -I - "mods/src/$m" <<'PY'
import json, pathlib, re, sys
found = set()
for f in pathlib.Path(sys.argv[1]).glob("*.cs"):
    for guid, rest in re.findall(r'BepInDependency\(\s*"([^"]+)"\s*(\)|,[^)]*\))', f.read_text(encoding="utf-8")):
        if "SoftDependency" not in rest:
            found.add(guid)
print(json.dumps(sorted(found)))
PY
)
  id=$(echo "$m" | sed -E 's/\.Client$//; s/([a-z0-9])([A-Z])/\1-\2/g' | tr 'A-Z.' 'a-z-')
  file="Tabg.$m-$version.dll"
  cp "$dll" "$OUT/$file"
  sha=$(sha256sum "$OUT/$file" | cut -d' ' -f1)
  size=$(stat -c %s "$OUT/$file")
  entries+=("{\"id\":\"$id\",\"name\":\"$m\",\"guid\":\"$guid\",\"depends\":$deps,\"version\":\"$version\",\"file\":\"$file\",\"sha256\":\"$sha\",\"size\":$size}")
done
(IFS=,; echo "[${entries[*]}]") | python3 -m json.tool > "$OUT/catalog.json"
python3 -c 'import json,sys; [print(e["id"].ljust(20), e["version"].ljust(8), e["file"]) for e in json.load(open(sys.argv[1]))]' "$OUT/catalog.json"

if [[ "${1:-}" == "--upload" ]]; then
  gh release view mods >/dev/null 2>&1 || gh release create mods --title "Client mods" --notes "Client mods for tabg share codes. tabg pins every file by sha256."
  # Share codes pin each file's sha256, and builds aren't byte-for-byte reproducible: a file that is
  # already released is never replaced (bump the mod's version to ship a change). The catalog keeps
  # the released entries for those and adds the new ones.
  gh release download mods -p catalog.json -O "$OUT/released.json" --clobber 2>/dev/null || echo '[]' > "$OUT/released.json"
  new=$(python3 -I - "$OUT/released.json" "$OUT/catalog.json" <<'PY'
import json, sys
released = json.load(open(sys.argv[1]))
have = {e["file"]: e for e in released}
built = json.load(open(sys.argv[2]))
merged = [have.get(e["file"], e) for e in built]
json.dump(merged, open(sys.argv[2], "w"), indent=4)
print(" ".join(e["file"] for e in built if e["file"] not in have))
PY
)
  rm "$OUT/released.json"
  if [[ -n "$new" ]]; then
    echo "uploading: $new"
    (cd "$OUT" && gh release upload mods $new)
  else
    echo "no new mod files"
  fi
  gh release upload mods "$OUT/catalog.json" --clobber
fi
