# tabg — Design

Status: agreed 2026-11-04. This replaces everything in the old repo (three GUIs, TabgInstaller.Core, BepInEx server mods).

## Decisions

| Topic | Decision |
|---|---|
| Audience | Friends hosting a server on their own PC (Windows or Linux) |
| Game server | Our own server written in Rust, replacing Landfall's Unity dedicated server. Unmodified TABG clients connect to it |
| Tool | One Rust binary, `tabg`: native window (egui), CLI, local web dashboard |
| Runtime | Console window + browser. The CLI talks to the running host over localhost |
| Dashboard | Server-rendered HTML + htmx, live updates over SSE, served by the binary |
| Joining | Real join codes via Unity Relay, the same way the official server gets them. Direct IP (UPnP) as fallback |
| Client mods | Optional extras only; the base game needs no mods. Auto-sync via share code: catalog mods from GitHub Releases, custom DLLs from the host |
| v1 scope | Setup, start/stop, live console, mods, share/join, settings + presets, players & admins, loadout & ring editor, crash recovery + backups |
| Mods | All groups kept. Server-side mods become native Rust features; client mods stay C#/BepInEx |
| Builds | Everything is built and tested locally (`scripts/check.sh`); no CI. Mod releases come from a local release script |
| Repo | Same repo, old code deleted, proprietary DLLs purged from history |

## 1. What tabg is

**Goal:** one person double-clicks `tabg`, gets a working TABG server in a few minutes, and sends friends a join code. Friends join with their normal TABG. If the host uses client-side extras (proximity chat, vehicles, maps), friends run `tabg join <code>` once and get exactly the right mods.

**Non-goals for v1:** public 24/7 hosting, server clusters, a mod marketplace for third-party mods, Steam ticket verification.

## 2. User flows

**Host**
1. Download `tabg` (one file, about 10–15 MB) and run it. The dashboard opens at `http://localhost:<port>`.
2. Setup: tabg finds the TABG install and extracts the map data from it once (§4.5). Then you pick a preset (Battle Royale, Gun Game, Deathmatch…).
3. Start. The dashboard shows the join code, players, live console, settings, loadouts and ring, presets, and backups.
4. Share: if client mods are in use, the dashboard shows a share code for friends.

**Friend**
- Base game: enter the join code in TABG. Nothing to install.
- With client extras: `tabg join TABG-…` finds the friend's TABG, makes a modded copy, installs the exact client mods (hash-checked), and offers a Launch button.

## 3. System overview

```
 host PC
 ┌───────────────────────────────────────────────────────────────┐
 │ tabg (host process)                                           │
 │  ├─ dashboard  (axum, localhost only)  ◄── browser            │
 │  ├─ local API  ◄── `tabg …` CLI                               │
 │  ├─ sync endpoint (read-only, content-addressed) ◄── friends  │
 │  └─ supervisor ──spawns──► tabg server-process (child)        │
 │                            ├─ game simulation                 │
 │                            ├─ ENet transport (direct IP)      │
 │                            └─ Unity Transport over Unity Relay│
 └───────────────────────────────────────────────────────────────┘
        ▲ JSON-lines IPC on stdio (commands in, events out)
 friends: unmodified TABG client ──relay/IP──► game server
          optional modded copy (C# client mods) for extras
```

The game server runs as a **child process** of the host, so a crash in game logic never takes down the dashboard. The supervisor restarts it with backoff and records why it crashed.

## 4. The game server

### 4.1 Compatibility target
- Target: the current Steam TABG client. The server must match the client's version hash (`APP_VERSION` in Landfall's `ServerClient`). tabg reads the hash from the host's install at setup, so a TABG update shows a clear "client version X not supported yet" error instead of broken joins.
- We implement Landfall's behaviour from protocol knowledge. **No Landfall code is copied and no Landfall assets are redistributed** (§10).

### 4.2 Transports
Both carry the same message layer (§4.3).

1. **ENet (direct IP).** Landfall uses ENet-CSharp, a fork of ENet whose wire format may differ from upstream ENet. We vendor the fork's MIT-licensed C source and call it through FFI (`tabg-enet-sys`); porting it to pure Rust can come later. Clients connect with `ConnectToServerIP(ip, port)`.
2. **Unity Transport over Unity Relay (join codes).** The official server signs in anonymously to Unity Gaming Services (Landfall's project), creates a relay allocation and gets a join code. Clients join with `ConnectToRelayServer(code)` over Unity Transport. We implement:
   - the UGS anonymous sign-in, allocation and join-code REST calls;
   - the relay bind/ping/relay protocol;
   - the Unity Transport framing and reliability that the client uses.

   We use it exactly as the official server does: one allocation per running server.

Relay is the default. Direct IP is the fallback: tabg tries UPnP to open the UDP port, and the dashboard explains manual port forwarding if UPnP fails.

### 4.3 Message layer
- Frame: `[event code: u8][payload]` on channel 0, reliable or unreliable per message.
- 126 event codes; the server handles 48 of them as inbound (Join, RoomInit, RequestWorldState, PlayerUpdate, PlayerFire, DamageEvent, RequestWeaponPickUp, RequestItemDrop, CarUpdate, RequestSeat, RequestAirplaneDrop, ReviveState, ChatMessage, AdminCommand…).
- Payloads are little-endian binary in .NET `BinaryWriter` style. Each message gets a Rust struct with `encode`/`decode` in `tabg-protocol`, plus round-trip tests against **recorded real traffic** (§9).
- Our own client-mod protocols use event codes ≥ 240 (240 voice, 241 radar, 242 range, 243 devtest, 244 skins, 245 vehicles, 246 destruction, 247 weather), and 239 princess now that 240–247 are taken (231–239 aren't TABG events either). New features go through one code with a channel byte, plus a capability handshake (§7).

### 4.4 Game simulation
- Landfall's server is mostly a referee. Clients send their own movement, shots and damage. The server validates them, owns match state (players, teams, inventory, loot, ring, plane, cars, game phase) and broadcasts. We copy this model first and tighten validation later.
- `tabg-game` is pure logic with no IO and no clock: `state + input → state + outgoing messages`. That makes it unit-testable and deterministic.
- Game modes: Battle Royale first, then Brawl/Deathmatch, Gun Game (loadout progression), Bomb, Deception, and Test.

### 4.5 World data
Loot spawners, player and car spawns, networked interactables (doors etc., with the IDs the client expects), map bounds, loot tables, plane paths, and later the navmesh and collision geometry all live in Landfall's Unity scene files.

- **Extraction:** a small C# BepInEx dumper mod runs once in batch mode against the host's own TABG files. It loads each map and writes `world.json` (and `navmesh.bin` later). It runs at setup, is cached per game-version hash, and its output is never redistributed.
- The dumper is the only place left where we run Landfall's Unity code. Whether to use the client install or the dedicated-server install is decided in M1.

### 4.6 Server list
The official server sends `GameServerHeartbeat` to Landfall's community backend (name, join code, player count, mode, password flag…). We send the same heartbeat so the server shows up in TABG's community server list. This is on by default and can be turned off for private servers.

### 4.7 Identity, admins and commands
- v1 does not verify Steam tickets. Players are identified by the IDs the client sends on join (Steam/Epic/PlayFab, as ServerLogger records them).
- Admins, bans and permissions are native, stored by player ID.
- Chat commands are implemented natively, with the command set people know from Citruslib: `/give`, `/bring`, `/goto`, `/send_pos`, `/start`, kick/ban… The dashboard console runs the same commands as "console".

### 4.8 Old server mods → native features

| Old BepInEx server mod | In the Rust server |
|---|---|
| AntiCheatBypass, Citruslib, EOS DLL copy, SteamCMD | not needed |
| MatchCore (loadouts, rings, votes, drops, spell drops, win rules) | core game rules + settings |
| ServerLogger | player history store |
| SoloTesting | "allow solo start" setting |
| ProximityChat.Server | voice relay for event 240 (needs client mod) |
| RangeMap / DevTestMap servers | "Range" and "DevTest" modes (need the Maps client mod) |
| CustomGameSkins.Server | skins authorization, **off unless the host enables it** |
| AdminRadar.Server | native bot radar (`game.bot_radar`): bots only, never real players |
| FakePlayers (bots) | native bots on the extracted navmesh (M6) |
| UnusedVehicles + Vehicles.Shared rules | native vehicle/FPV rules (M6, done) |
| PerformanceServer | the server is efficient by design; delta replication only if a client mod needs it later |

## 5. The tabg tool

### 5.1 Process model and CLI
- `tabg` with no arguments opens the window (`tabg-gui`, eframe/egui, no webview), which is the host process; without a display it falls back to the browser dashboard, which `tabg host` always uses. Other commands talk to a running host over its local API, starting one if needed.
- Commands: `setup`, `start`, `stop`, `restart`, `status`, `console`, `say`, `kick`, `ban`, `admins`, `config get|set`, `preset list|apply|save`, `mods`, `share`, `join <code>`, `backup list|restore`, `logs`.

### 5.2 State and the declarative model
- Each server is a folder with `tabg.toml`: preset, mode, game settings, loadouts, ring, admins, the client mod pack, and the network mode. The dashboard and CLI only edit this file, then **apply** it. The running server hot-reloads what it can and says when a restart is needed.
- A preset is a partial `tabg.toml`. The built-in presets ship as data.
- **Import:** an existing `game_settings.txt` / `TheStarterPack.txt` / `PlayerPerms.json` can be imported once. The format rules from the old code are kept for this: 0/1 keys, the starter-pack loadout syntax, and cross-field rules such as `KillsToWin` only for Brawl.
- Data lives in the OS app-data dir (`~/.local/share/tabg`, `%APPDATA%\tabg`): servers, extracted world cache, download cache (by sha256), backups, logs, and the player store (SQLite).

### 5.3 Dashboard
Overview (status, join code, players, quick actions) · Console · Players (kick/ban/admin, history) · Settings (typed forms, validation) · Loadouts & ring editor · Presets · Client mods & share · Backups · Logs.

### 5.4 Reliability
- Crash recovery: the supervisor restarts the server with exponential backoff, keeps the crash log, and shows it in the dashboard.
- Backups: `tabg.toml` and the player store, snapshotted before every apply and restorable with one click. They are small; game files are never copied.

### 5.5 Security
- The dashboard and local API listen only on `127.0.0.1`. They check the `Host` header against DNS rebinding and `Origin` on every state-changing request, and require a per-session token, so websites open in your browser can't control the server.
- The sync endpoint is a separate port. It is read-only and serves only files in the current share code, by sha256, and never lists directories.

## 6. Client side

- **Base game:** unmodified client, nothing to install.
- **Modded copy** (only when the server uses client extras):
  - Copy the TABG install without `EasyAntiCheat/` and write `steam_appid.txt` = `823130`.
  - Install BepInEx 5.4.23.3 (win x64, Doorstop 4 config) and PopupBlocker; the EAC boot-error remover is pinned by version and hash.
  - Launch the exe directly while Steam is running. On Linux, run under Proton with `WINEDLLOVERRIDES="winhttp=n,b"`.
  - The copy has its own `tabg.toml` with the pack, and `apply` keeps it exactly in sync.
- **Share code:** a self-contained encoding of the client pack (mod id@version for catalog mods; sha256 plus the host's sync address for custom DLLs). No central server is involved.
- **`tabg join`:**
  1. Decode the share code.
  2. Fetch catalog mods from GitHub Releases and custom files from the host.
  3. Verify every hash.
  4. Apply.
  5. Launch.

## 7. Client mods (C#)

- **Kept:** ProximityChat.Client, ModSettings, PopupBlocker, FlyingControls, CustomGrenades (client), Destruction, CoordsDisplay, PerformanceClient (client-side optimizations only), Maps.Client (RangeMap + DevTestMap merged), CustomGameSkins.Client, AdminRadar.Client (local-player ESP fallback removed).
- **Visuals** (new): one image effect on the main camera after the game's own post-processing, aiming for a warm cinematic storybook look: split toning (warm highlights, cool shadows; not on the sky), vibrance/saturation, soft S-curve, a sky gradient (deep blue overhead, cream horizon, clouds and sun kept), depth-based aerial haze that warms toward the sun, sun rays (quarter-res radial blur of the bright sky around `DayAndNightCycle.light`) and glow, vignette, CAS sharpening, and optional subtle ink outlines (second derivative of 1/depth; near side only; creases only up close; only where there is visible contrast). Also: volumetric sunlight (Volumetric.shader: half-res dithered raymarch through the cascaded shadow map with HG scattering, in a sun command buffer at AfterShadowMap, added in OnPreCull because TABG renders deferred and light buffers are collected during culling); procedural volumetric cumulus clouds (Clouds.shader, half res, sky pixels only), which replace the painted cloud masks of the skybox (face textures swapped for black, leaving only the gradient) and the world's SM_Generic_Cloud meshes; volumetric smoke for smoke grenades (Smoke.shader, half-res raymarch, depth-tested, 3 sun samples, a generated 3D Worley noise): the game's m_Smoke particle systems keep simulating with their renderer off, and an ellipsoid is refitted to their live particles so size, colour and lifetime stay the game's; biome weather (Weather.cs: the biome comes from the terrain layers around the player, L_Snow/L_Sand_Deser/L_Grass, beaches count as grass, blended near borders; each biome has weighted events picked per 180 s slot of the synced time of day, so modded players agree without networking; an admin's `/weather <name|auto>` overrides it for every modded player through event 247 `WTHR` (the mod says hello on join, the server answers with the forced weather and sends every change; `Auto` hands it back to the biome, and the server's pick beats the local Force setting); drives snow and rain drawn in the image effect (marks on world-anchored vertical planes along each view ray, depth-tested, widening and dimming below pixel size), a lens pass over the finished frame (refracting rain beads and running drops, flakes that land and melt; wets in about 6 s, dries in about 14 s), dust particles and, for sand, a spray of tiny grains on the same planes, rolling dust clouds from the smoke noise, soft banks drifting over the ground, a warmer and dimmer scene with a pale sun, grit and a dust film on the lens and an image-only shake in gusts, distance fog, overcast that removes most of the sun and its shadows and thickens the clouds, wet ground with puddles in rain (normals from depth; soaks in 30 s, dries in 2 minutes), and lightning; particles hide under roofs and everything fades out at altitude); a light layer on grenades (GrenadeFx.cs: extra low-density volumes in the smoke raymarch, on Explosion.Awake/Explode and SpawnObject.Spawn; every original effect stays visible, field haze takes the real radius and ends with the real object); a flashbang composite (StunEffect.Update patched: its white image is hidden and its alpha drives the effect, with the frame of the flash burned in and a blurred, over-exposed recovery, so blindness lasts exactly as long as in the unmodded game); sharper soft shadows (4096 map, 4 near-weighted cascades) and Amplify AO at High with blur/fade, only when the game's shadow option is High; the sea is Crest (the AnimateWater plane and THOcean are disabled), so it is improved by tuning Crest's material (foam on, colours, sun through waves) rather than replacing it. Explosions: a Harmony postfix on Explosion.Explode(NetworkOwner) (damage >= 10, radius >= 1.5, spells/melee/auras skipped by name; energy weapons blue-white) spawns a fading point light and feeds up to 4 visible flashes (line-of-sight raycast) to the image effect, which adds a quarter-res bloom of bright pixels near them plus a halo; costs nothing without explosions. Tuned for the default time of day (0.25); TABG's own afternoon/evening skies are pink, which the grade then pushes to purple. F8 toggles, sliders in ModSettings, debug views (outline mask, depth, passthrough, rays, distance). Leaves shadows/quality to PerformanceClient.
- **Destruction** (new, event 246 `DSTR`): every explosion breaks the map where it hits, and whatever loses its support comes down. Since 1.1 everything static in the map is breakable (not roads, terrain, ground, decals, water, clouds), not only buildings: props, containers, pipes, rocks, and trees (and bushes), which never get cut but fall over whole. Explosions count by damage >= 20 or |force| >= 100 (knockback, implosion, black hole), so the Gaussbow, air and orbital strikes, Torrent and Promise count too; only melee hits, heals and info effects are skipped by name. Statically batched objects (Unity `isPartOfStaticBatch`: the filter holds the shared world-space batch mesh) are found by `renderer.bounds`; if the batch is readable, the object's submeshes (`MeshRenderer.subMeshStartIndex` + material count) are cut out as its own mesh, otherwise it breaks whole and its debris draws those submeshes with `Graphics.DrawMesh`. LOD groups: only LOD0 is a part, the other levels are hidden with it. Debris is solid (layer Default, convex mesh collider up to 2500 vertices, else a box), and stays while a player is within 90 m, going 25 s after the last one left (at most 450 pieces, the farthest go first). A grid over the structures (32 m) keeps blast and collider lookups local; only structures that were hit are pumped each frame. The server does not simulate buildings: clients report each explosion (`Explosion.Explode`, spells and melee skipped by name), the server merges reports of the same blast (3 m, 0.5 s), keeps one ordered log, broadcasts it and replays it to late joiners; a new match sends a reset. Every client derives the same result from the log, so all fracturing is deterministic (no system RNG, piece order sorted by position, node order fixed). Buildings are found at map load: renderers named like building parts (`SM_Bld_*`, `SM_House*`, castle walls...), clustered by touching bounds, with furniture and doors inside attached as unbreakable parts. On first damage a building is captured: its meshes are read, and because most of the map is baked by MeshCombineStudio into merged, unreadable per-area meshes, the affected cells are rebuilt from their source objects without that building (reads the source meshes, which stay readable). From then on the building is drawn by its own section meshes (16 m grid). Pieces in the blast are cut into Voronoi cells (one jittered seed per occupied 1.4 m cell, triangles clipped along the bisector planes; `Core/Fracture.cs`), so cracks are irregular; collision triangles (boxes and mesh colliders) are cut with the same seeds, so walls open up for players and bullets exactly where they look open, and untouched colliders stay as the game built them. A cut edge gets a strip inward from the surface so walls show thickness. `Core/SupportGraph.cs` decides what stands: parts link to what they touch, and stay up if connected to an anchor in the ground and if the weight flowing down the shortest paths does not exceed what each part bears; the weakest part breaks first and it is recomputed, so buildings give way progressively. Whatever stands when the map loads counts as stable (a group with no anchor is based at its lowest point, capacities start above the current load), so only damage brings things down. Groups wider than 56 m (touching buildings chain up into whole towns) are split into 40 m tiles so the work per building stays small; a collapse does not cross a tile border. Fracture and support run on a worker thread, one blast at a time per building; the main thread only swaps meshes and spawns debris. Debris and dust are local (not synchronised). Limits: bots still path through the original building, loot does not fall, and players without the mod see the original walls. `game.destruction` (default off) switches it on and adds the mod to the share code (`ServerToml::client_mods`); unit tests in `mods/tests/ClientMods.Tests` (no game needed), server tests in `tabg-game`.
- **Shaders:** `mods/unity/` is a minimal Unity 2021.3.45f2 project (the game's exact version); every folder under `Assets/Bundles/` becomes a Windows asset bundle via `scripts/build-shaders.sh` (headless, ~3 s), and mods embed their bundle as a resource so they stay one DLL. Gotchas the script handles: the editor needs `libxml2.so.2` (Arch: `libxml2-legacy`), Unity's `bee_backend` hangs on exit with `--stdin-canary` (wrapped), Unity's script compiler is bypassed by building the editor script with dotnet (`mods/unity/EditorTools`), and `Packages/manifest.json` must list `com.unity.modules.assetbundle` or bundles come out without their AssetBundle object and the game rejects them as "not compatible with this newer version". The editor needs a free Personal licence (Unity Hub sign-in, once).
- **New:** the world dumper (§4.5) and **Tabg.Net**, a shared library for client mods:
  - one receive dispatcher;
  - framed sub-channels;
  - a capability handshake on join (the client announces mods and versions, and the server enables matching features per player).
- **Dropped:** EnhancedClient, StarterPack, TestMod/GiveMod, WeaponSpawnConfig, and all server-side BepInEx mods (replaced by §4.8).
- **Build:**
  - One solution, `mods/Mods.sln`. `mods/src/Directory.Build.props` sets everything; the only input is `TabgClientDir`, a modded TABG client copy with BepInEx installed (env `TABG_CLIENT_DIR`, gitignored `mods/Local.props`, or `-p:`). Game, Unity, Sirenix and BepInEx assemblies all resolve from there, and projects list extra modules as `<GameRef Include="UnityEngine.AudioModule" />`.
  - Output DLLs are named `Tabg.<Mod>.dll`.
  - No DLLs in git (`.gitignore` blocks them).
  - Deterministic builds with `PathMap`, and no post-build copies into tracked folders.
  - `mods/tests/Vehicles.Tests` is now xUnit, one fact per rule area (20). It needs no game files.
- **Reading the game:** `scripts/decompile.sh [--all] [client|server|both]` decompiles every game assembly of your own client and server installs (Assembly-CSharp, firstpass, Sirenix, EasyRoads, PlayFab, …; Unity engine and framework DLLs only with `--all`) into git-ignored `target/decompiled/{client,server}/`, as ilspy C# projects, in about 40 s. Use it to find what to Harmony-patch. `scripts/extract-assets.sh [client|server] [unity|content|sounds|shaders|all]` extracts the assets with AssetRipper 2.0.0 and vgmstream r2117 (sha256-pinned, fetched to `~/.local/share/tabg/tools`): `target/assets/<side>-unity/` is a Unity 2021.3.45f2 project (prefabs incl. UI, scenes, materials, shaders) for inspecting UI and building AssetBundles; `target/assets/<side>-content/` has models as `.glb` for Blender, `.png` and `.ogg`; `target/assets/<side>-sounds/<Bank>/` has the FMOD banks (gunshots, hits, vehicles, ambience, UI, catchphrases: 4,095 sounds) as `.wav`. `target/assets/<side>-shaders/` has all 171 shaders decompiled from D3D11 bytecode to ShaderLab + HLSL by USCSandbox (GPL-3.0, pinned commit, built from source with our fixes in `scripts/patches/uscsandbox.patch`: multi-segment blobs, cbuffer array stride, Switch/Ryujinx code dropped). That HLSL is machine-like (one function per variant, `tmp0`… registers): a reference for writing our own shaders, not drop-in source. The Unity project keeps AssetRipper's placeholder shaders, since AssetRipper's own decompiler is paid. The client takes about 6 minutes and 12 GB. Both are Landfall's code and content: they stay local and are never committed, quoted at length or shipped. Mods ship only our own patches and our own assets.
- **Release:** `scripts/release-mods` builds Release, packs each mod, writes `catalog.json` (id, version, sha256, size, dependencies) and uploads to a GitHub Release. tabg pins catalog versions.

## 8. Repo layout

```
crates/
  tabg-protocol    event codes, message structs, codec, version hash
  tabg-enet-sys    vendored ENet-CSharp C source + FFI
  tabg-utp         Unity Transport framing + Unity Relay + UGS auth client
  tabg-net         transport trait over ENet and UTP
  tabg-game        pure simulation: rooms, players, loot, ring, modes, commands
  tabg-world       world.json model and loader (navmesh later)
  tabg-server      tick loop: net + game + world, IPC to the host
  tabg-host        supervisor, state/apply, player store, backups, sync, UPnP, heartbeat
  tabg-gui         the window (eframe/egui)
  tabg-web         dashboard + local API (axum, htmx, SSE)
  tabg             the binary: CLI entry point
  tabg-probe       headless test client that speaks the protocol (tests + debugging)
mods/              C# client mods, dumper, Tabg.Net, recorder (dev only)
presets/           built-in presets (TOML)
scripts/           release-mods, record-session
docs/DESIGN.md
```

Rust stack: tokio, axum, maud or askama + htmx, clap, serde + toml, reqwest (rustls), igd-next (UPnP), rusqlite, sha2, zip, tracing.

## 9. Testing

- **Recorded ground truth:** a dev-only C# recorder mod on Landfall's own server (on the dev machine) logs every inbound and outbound event with timestamps while real clients play. The recordings become fixtures for:
  - codec round-trip tests: decode → encode gives the same bytes;
  - behaviour tests: same inputs, then compare our outgoing messages with Landfall's.
- **Game logic:** unit tests in `tabg-game`, plus property tests for codecs.
- **Integration:** `tabg-probe` clients join our server in tests (no game files needed) and play scripted scenarios.
- **Manual:** a real TABG client joins at the end of each milestone.
- Everything runs locally via `scripts/check.sh` (`--mods` adds the C# client mods). There is no CI.

## 10. Risks

| Risk | Mitigation |
|---|---|
| ENet-CSharp wire format differs from upstream ENet | Vendor the fork's C source; verify in M1 |
| Unity Relay / Unity Transport reimplementation is large and could be blocked by Unity/Landfall; ToS risk | Use it exactly like the official server; direct IP + UPnP fallback always works |
| Behaviour the client expects that isn't visible in code (timing, ordering) | Recorded sessions + `tabg-probe` comparisons |
| TABG update changes the protocol | Version hash check at setup; record a new session and diff |
| Unmodified client with EAC joining a server without EAC | Verify in M1 (the official server runs fine with `AntiCheat=false`) |
| Bots and vehicles need navmesh/collision data | Dumper exports them (M6); bots come last |
| Legal | Implement from protocol knowledge, copy no Landfall code, redistribute no Landfall files or extracted data, keep Landfall/Unity/Sirenix DLLs out of git. Landfall's terms may still object; that's an accepted risk |

## 11. Milestones

- **M0 – Reset.** Mirror backup → delete old code → purge proprietary binaries from history → force-push → new workspace skeleton. *(Done 2026-11-04: fresh single-commit history; old repo, tags, PR refs and all release downloads are backed up outside the repo.)*
- **M1 – Proof.**
  - ~~Recorder mod and one recorded session.~~ Replaced by `scripts/reference-server.sh`: Landfall's own server runs offline in a network namespace and `tabg-probe` joins it, so we capture its real replies without a mod or a human player.
  - ~~`tabg-protocol` for the join/spawn/movement messages.~~ Done; the join broadcast and accept reply match Landfall's server byte for byte, and its full world state (118 cars) decodes.
  - ~~ENet transport.~~ Done (`tabg-enet-sys` + `tabg-net`); Landfall's server accepts our ENet connections.
  - Our server (`tabg-server`) accepts players, sends the world, and replicates movement; two probes see each other move (end-to-end test).
  - ~~A real client joins by IP.~~ Done 2026-11-04, fully automated: `scripts/real-client-test.sh` launches a test copy of the real client under Proton with the `DevAutoJoin` dev plugin, which joins `127.0.0.1:7777` from the main menu. The unmodified game logic loaded `MainWorld_Base`, spawned at our lobby spawn, showed "WAITING FOR PLAYERS / 1 ALIVE / 100 HP", and kept pinging and sending movement.
  - ~~The real client sees a second player move.~~ Done 2026-11-04: with a `tabg-probe` bot walking circles in front of it, the real client showed "2 ALIVE" and rendered "tabg-bot" moving (2160 movement batches received). **M1 complete.**
  - ~~Dumper v0.~~ `mods/src/WorldDumper` + `scripts/dump-world.sh` export the lobby spawn points (6 on `ServerTerrain`, identical to what Landfall's server hands out) from the host's own dedicated-server install, offline; `tabg-server --world` uses them. Interactables, loot spawners and cars come with M3.

  Findings from the reference server (2026-11-04):
  - The Epic token and product-user-id fields in `RoomInit` must be NUL-terminated or the server throws.
  - Solo clients must send `RoomInit` as the host of a 1-player squad; otherwise Landfall's server refuses for lack of a booking, and since the refusal goes to a not-yet-accepted peer it is never delivered (the client just times out). Our server doesn't require bookings.
  - The world state is sent at an estimated size with a zero-filled tail (~3.7 KB extra).
  - A player with no weapon has `weapon_type = i32::MAX`.
  - After the world state the official server also sends `RecievedSteamID` (1 byte). It carries validated players' Steam/PlayFab/Epic ids so clients can load their cosmetics, and offline it's empty, so we don't send it.
  - The client has a direct-connect UI (`IP[:port]`, default port 7777) besides join codes.
  - The real client zero-pads its `RoomInit` too, and its last flag is `FillUpSquad`.
  - Joining before the client's Epic login completes fails with "Failed generating token" (the client fetches an EOS id token right after the ENet connect).
- ~~**M2 – Join codes.**~~ Done 2026-11-04. UGS sign-in, relay allocation, Unity Transport, server-list heartbeat, UPnP fallback.
  - ~~Relay hosting.~~ Done 2026-11-04 (`crates/tabg-utp`): anonymous UGS sign-in with the project id read from the host's `globalgamemanagers`, relay allocation and join code, the relay UDP protocol (bind with HMAC, keep-alive pings, rebind, reallocation when the relay drops us), Unity Transport's connection protocol (session tokens, ping/pong, timeouts) and TABG's pipelines (Fragmentation + ReliableSequenced window 64, UnreliableSequenced) with TABG's `[u16 length]` framing. `tabg-server --relay` prints the join code; `tabg-probe --relay CODE` joins like a player. A live test (`cargo test -p tabg-utp -- --ignored`) hosts and joins through Unity's relay; the real client joined by code (`TABG_TEST_RELAY=1 scripts/real-client-test.sh`) and played through countdown and plane with a relay bot.
  - ~~Server list.~~ Done 2026-11-04: opt-in `--list` posts `GameServerHeartbeat` to Landfall's community backend every 5 s (on its own thread), with the game version taken from the world dump (`app_version`, so it follows game updates). Join code is the relay code, or `public-ip:port` for direct servers. Verified: our server appeared in `GetServerList`. The backend has a word filter on names and descriptions (it refused "(ignore)" and "automated"); its error is logged.
  - ~~UPnP.~~ Done 2026-11-04: `--upnp` maps the UDP port (1 h lease, renewed, removed on Ctrl-C). Not verifiable here: this network has no UPnP router, so we only saw the fallback warning.
  - Direct IP needs only ENet: the client resets to its ENet networker on every return to the main menu, so it never uses Unity Transport for a direct join.
- **M3 – Battle Royale playable.** Loot, inventory, weapons, damage, death/revive, ring, plane drop, cars, match flow, win.
  - ~~Match flow.~~ Done 2026-11-04: countdown (players-to-start, timed force start), plane (45 s path through the first ring, 5 s progress sync, jump requests, auto-drop), landing relay, ring cycle with the map's `ring_times` (dumped), last-team-standing win, refusal of late joiners, restart after the end. In the real client: "GAME STARTS IN 3", plane ride, jump, landing, "RING PROGRESS 44 %".
  - ~~Damage and death.~~ Done 2026-11-04: client-reported damage with the official rules (only in the match, only after jumping, 1 s grace after going down, downed damage capped at 50 × 0.12), down-before-death, whole-squad-down kills the squad, revive (to 15 HP), self-reported fall/ring deaths, `PlayerDead`/`TeamDead` with placement, kill counting, and relays for fire (with projectile sync), weapon/gear changes, effects, markers and player state. In the real client: the bot's hit took the player to 40/100, the lethal one triggered "Local player network die", "Handling Team Dead" and the switch to `Ended`.
  - ~~Loot.~~ Done 2026-11-04: the dumper exports 44,247 loot spots, 14 match modifiers with their weighted presets and 247 item names; at match start a modifier is drawn by rarity, ~10 % of spots are kept (`StripLootByPercentage`), and each gets one weighted drop. The chunk system (128 m chunks, 3×3 view) sends `ChunkEntry` on jumping and entry/exit diffs on crossing chunks, `PlayerEnteredChunk`/`PlayerLeftChunk` to watchers, and movement only to watchers. Pickups and drops work; equipment is tracked from weapon changes. In the real client: landed in town, picked up an M16 and two pistols and fired them.
  - ~~Cars.~~ Done 2026-11-04: the dumper exports the 118 cars the official server spawns (type, pose, seats with driver flag, damageable parts); the world state lists them, chunk entries/exits include them (not in the lobby, like the official server), seats are exclusive, the driver's `CarUpdate` moves car and occupants (with chunk changes), the last driver may push the empty car for 5 s, car parts take damage, and downed/dead/leaving players are ejected. Drivers are replicated with the driving body, passengers as inside-car. In the real client (auto-jump + auto-seat from `DevAutoJoin`): cars spawned around the landing, the player got on a Bike and drove.
  - ~~Healing, throws, chat and the rest.~~ Done 2026-11-04: healing items with amounts from the dump (bandage 15, med kit 50), health-state increases, thrown items (grenades don't become loot), proximity chat, the parrot (rate-limited), catchphrases, spectating, projectile sync, the generic player package relay, the built-in "Start Game" admin command, and native admin chat commands (`/start`, `/give <item> [qty]`, `/heal`, `/kick <name>`; admins via `--admin`). In the real client: `/give 132 2` dropped two med kits and replied.
  - ~~Inventory, death drops, souls, blessings, curses.~~ Done 2026-11-05 (`room/inventory.rs`): the server tracks what each player carries, like the official server: pickups and handouts add; drops, throws, healing and fired ammo remove. The dead (and anyone leaving mid-match) drop it all (`PlayerDeathLootDrop`), plus a soul (`SoulDrop`, soul item 10 from the dump). Not in Bomb or gun game. A soul brings a dead teammate back above the reviver with a random curse (`RequestRespawnTeamMate`, curses from the dump); three cleanse a curse (`CurseCleansed`); blessings are broadcast (`BlessingRecieved`). In the real client: a bot's death left its ammo on the ground.
  - ~~Spell drops and the respawn minigame.~~ Done 2026-11-05 (`room/extras.rs`). Spell drops: the first falls 60–100 s after the ring starts, then one every 136–272 s (`Spelldrop_Server` timing from the dump), at a loot spot above y = 110 inside the white ring. Rarity is 15 % legendary, 35 % epic, 50 % rare (`DropSpawned`); opening one (`RequestClickInteract`) spills 3 rare, 2 epic or 1 legendary blessings (`DropOpened`, blessing pools from the dump). Respawn minigame (`game.respawn_minigame`, off by default like the official server): an early Battle Royale death (not in the last two rings, once per match) sends `PlayerDead` with spectate 254, and the team isn't out while someone fights the boss. A won `BossFightResult` brings the player back (`PlayerRespawnFromBoss`, optionally with a curse or a blessing); a lost one may put the team out. In the real client: after "YOU DEAD" the player was in the boss arena with its countdown, and the spell drop arrived during the match.
  - ~~Validating actions against the inventory.~~ Done 2026-11-05 (`game.inventory_check`, `--inventory-check`): drops, throws and healing are checked against the tracked inventory. `log` (the default) allows them and writes one console line per player and item (`inventory check: … wanted to drop 3 × item 132 but carries 0`); `enforce` refuses items the player doesn't carry and clamps quantities to what they do; `off` trusts clients like the official server. Ammo is never checked (item names ending in " Ammo", plus anything a gun fired): guns can come loaded, so the server's ammo count drifts. Bots and Bomb (bought guns aren't tracked) are exempt. Firing and weapon switches aren't checked yet. Unit-tested, and in the real client (`TABG_TEST_HEAL=131 TABG_TEST_DROP=132:1@75` with the loadout): it healed with a bandage from the loadout and dropped a med kit with no report. The default stays `log` until real matches with friends show no false reports. Weapon switches (`WeaponChange`) are checked the same way (`equip`): enforce doesn't relay a weapon the player never got. `scripts/real-client-test.sh` now runs the client without a window or sound by default (Unity `-batchmode -nographics`, Wine audio off; `TABG_TEST_WINDOW=1` shows it); the heal/drop test passes the same way headless.
- ~~**M4 – tabg v1.**~~ Done 2026-11-05. Dashboard, settings and presets, players/admins/console, loadout & ring editor, crash recovery, backups, share/join with client sync, Windows + Linux builds.
  - ~~Host, dashboard and CLI.~~ Done 2026-11-05 (`crates/tabg-host`, `crates/tabg-web`, `crates/tabg`). `tabg` starts the host: the dashboard (Overview with join code, Console, Players, Settings, Loadout & Ring, Presets, Backups; htmx + SSE) and the JSON API on 127.0.0.1 (Host/Origin checks, session token via the printed link or `Authorization: Bearer` from `host.json`). The game server runs as `tabg serve`, a child process with JSON-lines IPC; the supervisor restarts it with exponential backoff and writes a crash log (tested with `kill -9`: back with a new join code after 1 s). CLI: `setup`, `start` (starts a background host if needed), `stop`, `restart`, `status`, `console`, `say`, `kick`, `ban`/`unban`, `admins`, `config show|get|set|path`, `preset list|apply|save`, `backup list|restore`, `logs [-f]`. Without a running host, file-only commands use a temporary in-process API, so there's one code path.
  - ~~Setup.~~ Done: Steam library detection (native, Flatpak, Windows registry), then the map export from the TABG Dedicated Server (app 2311970): a hard-linked copy, BepInEx 5.4.23.3 pinned by sha256, the dumper plugin embedded in the binary, and a private `game_settings.txt` (port 47999, no relay). The dumper switches the export server's heartbeat off. It takes about 8 s and is cached per game version.
  - ~~Config, presets, players.~~ Done: `tabg.toml` (unknown keys are errors, validation by field, Landfall's MIT word list for public names), 5 built-in presets that use only working features (Battle Royale, Solo Royale, Quick Match, Loot Frenzy, Armed Drop) plus saved ones, a backup before every change (50 kept, one-click restore), hot reload for lobby rules and a restart for network/name/player-limit changes. Admins and bans work by name or player id (`epic:<product user id>`); in-game `/ban` is written back to `tabg.toml`. The landing loadout is sent as `PlayerLootRecieved`, straight into the inventory. Player history (joins, kills, deaths, wins, names) lives in `players.json`, not SQLite: that is enough for a friends' server and has no C dependency.
  - ~~Landing loadout in the real client.~~ Verified 2026-11-05 (`TABG_TEST_SERVER_ARGS="--loadout 160:1,6:240,131:5,132:2"`): after landing the player held the M16 with "30 | 210" ammo, 2 med kits and 5 bandages.
  - ~~Windows build.~~ Done 2026-11-05: `scripts/build-release.sh` builds the dumper plugin, embeds it, and builds `tabg` for Linux and (with zig + cargo-zigbuild, `x86_64-pc-windows-gnu`) Windows into `target/dist/` with SHA256SUMS. About 8 MB each. The Windows build passed a smoke test under Proton's Wine (`--version`, presets, `config set/get` through the in-process API).
  - ~~Share/join with client sync.~~ Done 2026-11-05: `[mods] client` lists catalog mods (`id@version`) and the host's own DLLs. `tabg share` (or the Share page) makes a `TABG1-…` code with the join code, catalog mods pinned by sha256 (plus their hard BepInEx dependencies), and own DLLs by sha256 with the host's sync address. The sync endpoint is read-only `GET /f/<sha256>` on TCP 7781, started only when needed; `TABG_SHARE_HOST` overrides the address for LANs. `tabg join <code>` makes the modded copy (hard links, no `EasyAntiCheat/`, `steam_appid.txt`, BepInEx 5.4.23.3 and AntiCheatBootErrorRemover 1.0, both pinned) with exactly the shared plugins, and removes what an earlier join installed. `tabg launch` starts it (Proton Experimental on Linux). `scripts/release-mods.sh` builds the 11 client mods into `target/mods-dist/` with `catalog.json`; `--upload` publishes them to the `mods` GitHub release (first published 2026-11-05: 11 mods plus `catalog.json`; the downloaded files match the catalog's sha256). Verified end to end: a friend data dir joined a share code (local catalog plus a host-served DLL) in 0.7 s, launched, BepInEx loaded every plugin, and the real client joined our server by relay code.
- **M5 – Modes & extras.** Brawl, Gun Game, Bomb, Deception, Test; MatchCore rules; proximity chat relay; Range/DevTest; skins (opt-in); radar.
  - ~~Respawns, Deathmatch and Gun Game (MatchCore rules).~~ Done 2026-11-05: `[rules]` in `tabg.toml` (`respawn`, `respawn_delay`, `kills_to_win`, `time_limit`, `gun_game` levels). Dead players get `PlayerRespawn` (`[count]` plus 23 bytes each: index, new index, health, position, rotation, curse 255) at a random loot spot inside the white ring (loot spots sit on floors and terrain), a chunk teleport, and the loadout plus their gun game level. No team eliminations with respawns. The match ends at the kill limit, past the last gun game level, at the time limit (most kills wins), or when everyone left. Presets "Deathmatch" and "Gun Game" (10 weapon levels, ammo in the loadout); the dashboard has a Rules card and a level editor. In the real client: killed by the bot, back 5 s later in a town with 100 HP and a new M16 ("30 | 210"), "2 ALIVE".
  - ~~Proximity voice relay.~~ Done 2026-11-05 (`room/voice.rs`): event 240 frames from the ProximityChat client mod are checked (version, size), rate-limited to 60/s per speaker, stamped with the real sender and sent unreliably to living players within `game.voice_range` (50 m by default, 0 = off). Unit-tested. A real-client check needs two players with microphones and is still to do.
  - ~~Test mode, Range and DevTest maps.~~ Done 2026-11-05: `game.mode = "test"` starts without plane or ring (everyone where they stand, chunk entry and loadout at the start, like the official `TestGameMode`). `game.map = "range" | "dev_test"` plays the RangeMap / DevTestMap client mods' maps: their hello (events 242 `RNGE` v2 / 243 `DVTM` v1) is accepted, F6 item requests go straight into the inventory (Range also gets `ItemGranted`), the island's loot and cars are left out, and the map's own spawns are used. Validation requires the map's client mod in the share. Presets "Shooting Range" and "Island Gun Game". In the real client: the handshake was accepted, WilhelmTest loaded, Test mode started ("#1 : 0 Kills"), and the M16 loadout arrived.
  - ~~Skins (opt-in).~~ Done 2026-11-05 (`room/skins.rs`): with `game.skins = true`, a CustomGameSkins client's hello (event 244, `SKIN` v1) is accepted; outfits (6 slots of item and colour) are rate-limited (0.5 s), shape-checked, stored as the player's gear and broadcast as a vanilla `GearChange`. Off by default: the hello is answered "disabled". Unit-tested. Unlike the old plugin we can't check items against the game's gear database (it isn't dumped).
  - ~~Brawl.~~ Done 2026-11-05: setup also exports the map in Brawl mode (`<version>-brawl.json`): 6 team and 12 solo spawn houses (5 spawns and 34/22 tiered loot spots each, read from the unspawned `BrawlLootSelector` prefab, because the live copy uses up its spots) and the 14 loot presets. `game.mode = "brawl"` plays it like the official `BrawlGameMode`: teams spawn in their house, at the end of the countdown one preset is drawn and the same loot goes into every house, the doors open (`GameStateChanged` 5), a wiped team respawns at its house, and the first team to `kills_to_win` (or the last team left) wins. Brawl preset; Mode/Map selectors and the Rules card on the Settings page. In the real client: the CastleFight map loaded, the player stood in the spawn house with the round's loot on the walls (e.g. Liberating Barrett), and the scoreboard showed both teams.
  - Fixed 2026-11-05: rustfmt had reformatted the built-in preset table, so later text edits silently missed it, and only the first 5 presets were built in. All 10 are now built in, and a test checks that every file in `presets/` is.
  - ~~Bomb.~~ Done 2026-11-05 (`room/bomb.rs`): setup also exports TABG_CS in Bomb mode (`<version>-bomb.json`: two teams' spawns, the `DABOMBTAG` bomb spawn, bomb item 168, start weapon m1911). Played like the official `BombGameMode`: teams balanced (team 0 = T, team 1 = CT), each round respawns everyone at their side, puts the bomb back and hands out guns (`GunPurchasedAccepted`); the countdown is the buy phase (`RequestPurchaseGun`, money via `MoneyChanged`: 500 to start, +200 per kill, +300 plant or defuse, +3000/+2000 per round), then `Started` with the round time as an int. T picks up and plants the bomb (`BombPlanted`), CT defuses (`BombStartDefuse` / `BombStopDefuse`). A round ends by wipe, explosion, defuse or time (`RoundOver` with team and reason, `ScoresChanged`); sides switch at half time; 10 s intermission; first to `rules.rounds_to_win` gets `TeamDead` placements and `Ended`. Bomb preset; round settings on the Rules card. In the real client: TABG_CS loaded, both teams on the scoreboard, the m1911 start gun, and the round timer counting down from 1:30.
  - Deception isn't played: Landfall's server has no Deception game mode (its `SetGameMode` has no case for it).
  - Capture the Princess (2026-11-10, `princess.rs`, `room/princess.rs`, `room/princess_bots.rs`, `room/princess_loadouts.rs`; `game.mode = "princess"`): Actual Castle against New Castle on the island (first City, which was too far: ~750 m of escort against ~450 m now; New Castle's round tower and courtyard gate are links too), played as Battle Royale for the client (only that loads the island) but with no plane or ring: `Started`, then one `PlayerRespawn` puts everyone at their cage and the princesses in their hideouts. Teams are groups 0 and 1; the princesses are server-side players in groups 2 and 3 (so both teams can hurt them), dressed with gear from the client's database (princess hair 315, sweet dress 483 in its own turquoise or colour 25 pink, crown 105 in the helmet slot). Whoever holds a gun on a princess (weapon out, on foot, within 16 m, aim within 12° plus her width; bots only when they mean to) walks her: the other team's to their cage along a flow field (`flow_field`: Dijkstra from the goal over the region around both bases), her own team's home, or, for a player in her territory, where they point. Own-team kills respawn her halfway home with +15 % speed (three times at most); an enemy kill resets that and sends her home. Kills earn helicopter smoke grenades (item 208, a token per player; the throw spawns a FlyingControls `Heli` 3 s later on clear ground there). Two turrets per base fire server-flown homing missiles (no owner, pass through people) at enemy-crewed aircraft more than 4 m up; each hit takes 55 % of the airborne health. Event 239 (`PRNC` v1; 247 went to weather) sends the PrincessMode client mod its team's view (4 Hz) and banners; players without it get the banners as chat. The walkability grid can't see under the castle's gatehouses, so the five gates are links (`NavGrid::add_link`; walkers keep their level under arches, `walk_height`) and a gateway is walked to its far end before the field is asked again. Bots attack (rally ~100–150 m out, come in from the side), defend, escort, guard the escort, rescue a taken princess and walk her home; skills easy/normal/hard change reaction, aim, damage, sight and speed, and apply to bot-only teams (bots next to players play normal). They bandage when out of a fight and bleed out when downed. Remote players are drawn by the client as a ragdoll pulled towards the position it gets, which is the hip (its own updates send `m_Hip.position`; a respawn puts the feet 0.9387 m below), with the legs animated from the walking direction (world space) and the movement byte (bit 0 forward, 1 backward, 2 strafing, 3 sprinting, 4 crouching, 5 prone, 6 g-mode, 7 jump): bots stand on the grid with their feet, so their updates add 0.94 m, move every tick (not every think), turn at a skill-dependent rate and send those bits; in this mode they only see within their field of view (100°/120°/140°, everyone within 5 m), hear gunfire (45/75/100 m) and running within 12 m, shoot only once facing the target, and their aim settles over 1.5 s. Loadouts: three of twelve offered after each (re)spawn and on coming home after 30 s away (`OFFER`/`PICK` on 239), the first one after 30 s or straight away without the mod; granted items go through the client's normal pickup, so full slots swap. ModSettings must ship with FlyingControls (it uses its types; the catalog only lists hard dependencies). Tests: `room/princess_tests.rs` on a flat island, plus `island_bots` (ignored) on the real grid: there bots cross the island, take the pink princess and cage her in about 16 minutes of a bot-only match. Verified in the real client: the island loads, everyone respawns at their cage, the loadout and the banners arrive.
  - Fixed: the dumper exports Brawl data only when a live `BrawlLootSelector` is in the scene. The prefab is loaded in every mode, so a Battle Royale dump would otherwise have turned the server into Brawl.
  - ~~Radar.~~ Done with the bots (see M6).
- ~~**M6 – Bots & vehicles.**~~ Done 2026-11-05. Navmesh bots (FakePlayers successor) and vehicle/FPV rules.
  - ~~Bots.~~ Done 2026-11-05. The dumper writes a walkability grid next to the world (`world.nav`: gzip `TBNG` v1, 1 m cells over the island's land, top-surface height per cell, 0 = water; 1808×1848 cells in about 3 s from downward raycasts). Bots (`game.bots`, `--bots N`) are server-side players with indices counting down from 253, above every peer id, so they never get packets. They join the lobby once a human is there, jump from the plane over land (random moment, or `--bot-jump`), fall, land with a random gun, walk A* paths (0.7 m step limit) into the ring or around nearby, swim to shore if they end up in water, and fight the nearest visible enemy (line of sight ray-marched on the grid): `PlayerFire` to players nearby, hits through the normal damage path. They hold fire for 5 s after landing and react after 0.5–1.2 s, hit at most half their shots and do 70 % of the gun's damage. Outside the storm's edge they lose 1–10 HP per second, depending on the ring, and die a ring death. Players' clients do that for themselves, but bots have no client. Battle Royale on the island only (the other maps have no grid yet). Verified in the real client: bots counted in "ALIVE", their shots and kills show up, a bot killed another bot.
  - ~~Bot radar.~~ Done 2026-11-05 (`room/radar.rs`, AdminRadar.Client 2.0 rewritten, ~330 lines instead of 1,450): `game.bot_radar = "off" | "admins" | "everyone"` (`--bot-radar`), off by default. The client says hello (event 241, `ARDR` v2) on join and gets accepted or denied (off / admins only). Four times a second, allowed players get a snapshot of the **bots only**: index, team, state (plane, falling, walking, fighting, swimming), health, position, walk goal, target and gun. Real players are never sent, so the radar can't be used as a wallhack. Permission is checked on every snapshot, so a hot reload takes effect without rejoining. The old client's dummy hacks (moving bodies, faking drops, local-player fallback) are gone: our bots are ordinary players to the client. F4 toggles it. Verified in the real client: the radar showed the surviving bot, and its label said "Walking, 24 HP" after a bot fight.
  - ~~Vehicles (UnusedVehicles successor).~~ Done 2026-11-05 (`vehicles.rs` rules ported from `Vehicles.Shared`, `roads.rs`, `room/vehicles.rs`, `room/vehicle_combat.rs`, `room/taxi.rs`; event 245 `THGV` v2, unchanged FlyingControls client). Setup now also exports all 16 vehicle types from the prefabs (seats, parts, collision boxes; nothing is spawned, so broken prefabs can't throw) and the road graph, built by the client's own `RoadNetwork` code (5,054 points, 6,188 links).
    - `game.unused_vehicles` (chance per map car, `--unused-vehicles`) adds the unused types (helicopter, UFO, hover bike and car, buggies…) next to map cars, up to `game.unused_vehicles_max` (15). They are in the world state, so vanilla clients see them; flying needs FlyingControls. CannonCar and DeceptionBossCar stay out. Preset "Flying Vehicles".
    - Admin `/vehicle <name>` spawns one on clear, flat ground in front of the admin (3 s cooldown, 16 extra at most) for FlyingControls clients only, sent as `Spawn` and confirmed with `Ack`. Vanilla clients never hear of it (it's filtered from their chunk entries). `/vehicles` lists what's on the map.
    - Aircraft have their own health (30 in the air, 300 grounded, 2 s protection when spawned and when the first pilot gets in). Damage reports for aircraft count as damage, a pilot's `Crash` at 18 m/s or more destroys it, and `Detonate` lists the occupants so their clients spare them. The aircraft's health goes out as `AircraftHealth` every 2 s. A helicopter whose pilot leaves hands over to a passenger (`PilotHandover`). An empty one falls on the height field (`AircraftFall` at 20 Hz) and explodes if it hits hard. `EjectRequest` gets a shield and a launch.
    - Helicopter missiles: lock (0.65 s), 4 s cooldown, guided (25 m/s, turning 1.8 rad/s, losing lock behind cover) or straight (55 m/s). The server flies them and sweeps them against the height field, vehicle hulls and players. Three bullet hits (`MissileHit`, checked against the missile's recent positions and cover) shoot one down.
    - FPV drones: `/fpv` hands out item 30001. A throw uses exactly one, and the launch must follow it within 4 s and 25 m. Moves are checked for speed, sequence and collisions, with a 60 s flight, 1 km range and 2 s link timeout. Crashes must hit something, and three bullet hits shoot a drone down.
    - Explosions damage bots on the server (hand-grenade or dynamite falloff, walls block). Players' clients report their own blast damage with a receipt trailer (stripped before decoding), which earns the attacker a `BlastConfirmed` hit marker, once per target.
    - Hover taxis: the driver picks a destination and the server flies the taxi along the roads (Dijkstra plus the ported `RoadFollower`, 20 m/s, a 2× boost for 2 s with an 8 s cooldown). It hops over obstacles found on the height field and reports `TaxiStatus`. Client car updates can't move a taxi.
    - Not ported: bots in vehicles (the old FakePlayers integration).
    - Verified in the real client with FlyingControls: 133 cars (118 + 15 unused) in the world state, and `/vehicle heli` put a Police helicopter on the ground 13 m from the player ("New vehicle #133 spawned"). Flying, missiles, drones and taxis are covered by unit tests but haven't been flown by a person yet.

## 12. Carried over from the old repo
Knowledge worth keeping, rewritten rather than copied:
- the client modded-copy recipe (§6);
- Steam library detection (`libraryfolders.vdf`, registry on Windows, native and Flatpak Steam paths on Linux);
- server names must use Landfall's word list (`landfallgames/tabg-word-list`), or the server-list heartbeat rejects them;
- the item ID database (now dumped from the game: e.g. 6 = Normal Ammo);
- the preset contents;
- the game_settings/TheStarterPack format rules (for import);
- the pure rule code in Vehicles.Shared;
- the tests for ProximityChat frame processing and MatchCore config parsing.
