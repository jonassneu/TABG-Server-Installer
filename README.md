<div align="center">

<pre>
 _____                                                               _____
( ___ )-------------------------------------------------------------( ___ )
 |   |                                                               |   |
 |   |  _____  _    ____   ____      ____                            |   |
 |   | |_   _|/ \  | __ ) / ___|    / ___|  ___ _ ____   _____ _ __  |   |
 |   |   | | / _ \ |  _ \| |  _     \___ \ / _ \ '__\ \ / / _ \ '__| |   |
 |   |   | |/ ___ \| |_) | |_| |     ___) |  __/ |   \ V /  __/ |    |   |
 |   |  _|_/_/   \_\____/ \____| _  |____/ \___|_|    \_/ \___|_|    |   |
 |   |                                                               |   |
 |   |           T A B G   S E R V E R   I N S T A L L E R           |   |
 |___|                                                               |___|
(_____)-------------------------------------------------------------(_____)
</pre>

**Host a [Totally Accurate Battlegrounds](https://store.steampowered.com/app/823130/Totally_Accurate_Battlegrounds/) server for your friends, with one program.**

[![GitHub release](https://img.shields.io/github/v/release/jonassneu/TABG-Server-Installer)](https://github.com/jonassneu/TABG-Server-Installer/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust-orange.svg)](https://www.rust-lang.org/)

</div>

---

## About

`tabg` is a single program for Windows and Linux. Run it and its window opens. A few minutes later you
have a TABG server and a join code you can send to your friends.

It brings its **own TABG game server, written in Rust**. You don't need Landfall's Unity server,
SteamCMD, BepInEx or Citruslib on the host. Friends join with their **normal, unmodified TABG**
through the game's join-code field. If you turn on extras such as proximity voice, flying vehicles or
the shooting range, friends run `tabg join <code>` once and get exactly the mods your server uses.

> This is a full rewrite. The old C# installer (v5.2.0 and earlier, WPF launcher plus BepInEx
> server plugins) is retired. Its features are now built into the server (see
> [What happened to the old plugins](#what-happened-to-the-old-plugins)).

## Requirements

- Windows or Linux (x86-64).
- **Host:** TABG and the free **TABG Dedicated Server** installed through Steam. tabg reads the map
  data from your own install once during setup; nothing from the game is shipped with tabg.
- **Friends:** TABG on Steam. That's all for the base game. For client extras, Steam must be running,
  and Linux needs Proton.

## Download

1. Go to the [latest release](https://github.com/jonassneu/TABG-Server-Installer/releases/latest).
2. Download `tabg-<version>-windows-x86_64.exe` or `tabg-<version>-linux-x86_64`.
3. Put it anywhere and run it. It's one file with nothing to install.

Or [build it yourself](#building).

## Quick Start

### Server Setup

1. Run `tabg`. Choose **Host a server** in the window that opens.
2. tabg finds your Steam libraries and exports the map data from the TABG Dedicated Server. This only
   happens once per game version.
3. Pick a preset on the **Game mode** tab, for example *Battle Royale*, *Gun Game* or *Deathmatch*.
4. Click **Start server**. The join code appears at the top of the window.
5. Send the join code to your friends. They enter it in TABG's join field.

On a machine without a display, `tabg` opens the browser dashboard instead (`tabg host` does that on
purpose). Everything also works from the command line, also while the window is open:

```bash
tabg setup
tabg preset apply gun-game
tabg start
tabg status          # join code, players
```

### Client Setup (for players)

Only needed if the host uses client extras. The **Mods** tab then makes a share code (`TABG1-…`) and
a message to send to friends.

1. Run `tabg`, choose **Join a friend** and paste the code. Or run `tabg join TABG1-…`.
2. tabg makes a separate modded copy of your TABG (your Steam install is not touched), installs
   BepInEx and exactly the mods the server uses, and checks every file's hash.
3. Press **Start TABG** (or run `tabg launch`) to start the modded copy. Don't launch it from Steam.

When the host changes the mod list, run `tabg join` with the new code. Mods from an earlier join are
removed.

## Features

### Core

- **Own game server.** A Rust reimplementation of the TABG server. Vanilla clients connect to it
  either through Unity Relay join codes (the same way the official server does it) or by direct IP
  (`IP:7777`, with UPnP).
- **Server list.** Optionally show up in TABG's community server list.
- **Window, dashboard and CLI.** Status, join code, players, live console and quick actions, in
  tabg's own window (native, written in Rust), in the browser, or with `tabg …`.
- **Crash recovery.** The game server runs as a child process. If it crashes, it is restarted with a
  new join code and the crash log is kept.
- **Backups.** Taken automatically before every config change (the last 50 are kept); one click
  restores one.
- **Share codes.** One code carries the join code plus the exact client mods, pinned by sha256.
- **Inventory check.** Drops, throws and healing with items a player never picked up are reported in
  the console, or refused with `game.inventory_check = "enforce"`.

### The Window

| Tab | What it does |
|---|---|
| **Overview** | Who's online by squad, kick, ban or remove bots, the match phase, a message to everyone, latest activity |
| **Players** | Everyone online or ever seen, heal, give items, admins, bans |
| **Game mode** | The built-in presets and your own, with a preview of exactly what changes before you use one |
| **Rules** | Mode, map, teams, starting rules, loot, time of day, bots, vehicles, respawns and more, with validation. Nothing applies until you press Save |
| **Loadout and ring** | What everyone lands with, ring times, sizes and speeds with a live diagram, gun game levels |
| **Mods** | The client mods friends need, your own DLLs, the share code and a message for friends |
| **Console** | Live log with filters and search, plus the same commands admins use in chat |
| **Server** | Name, password, player limit, network, backups with one-click restore, game data |

Anything that would disconnect people asks first and says how many are online. The window talks to the
same host as the CLI, so closing it stops the server.

The browser dashboard (`tabg host`) has the same features in a simpler form. It only listens on
`127.0.0.1`, checks `Host` and `Origin`, and needs a session token, so websites in your browser can't
control your server.

### Game Modes

| Mode | What it is |
|---|---|
| Battle Royale | Squads, plane drop, loot, ring, spell drops, souls, blessings and curses, revives |
| Deathmatch | Respawn 5 s after dying, armed, inside the ring. First to 20 kills wins |
| Gun Game | Every kill gives you the next weapon; the first kill with the last weapon wins |
| Brawl | Teams start in spawn houses on the castle map with the same loot. First team to 20 kills wins |
| Bomb | TABG_CS: buy guns, T plants, CT defuses, sides switch at half time |
| Capture the Princess | Actual Castle against New Castle: walk the other team's princess to your cage at gunpoint (see [Capture the Princess](#capture-the-princess)) |
| Test | No plane or ring; used for the Shooting Range and the DevTest island |

### Presets

| Preset | Description | Client mods |
|---|---|---|
| Battle Royale | TABG as Landfall ships it: squads, 2 players to start, map loot and ring | none |
| Solo Royale | Battle Royale where everyone fights alone | none |
| Quick Match | Short countdown, more loot, and a ring that moves twice as fast | none |
| Loot Frenzy | Four times the loot, and everyone lands with bandages | none |
| Armed Drop | Everyone lands with an M16, ammo and heals | none |
| Deathmatch | Respawns, first to 20 kills or most kills after 15 minutes | none |
| Industry Deathmatch | The ring closes around Industry in 10 s and stays; respawns with guns and grenades, first to 20 kills | `destruction` |
| Gun Game | 10 weapon levels, ending with the katana | none |
| Brawl | Spawn houses on the castle map, first team to 20 kills | none |
| Bomb | First to 7 rounds on TABG_CS | none |
| Flying Vehicles | Battle Royale with helicopters, UFOs and hover taxis; `/vehicle` and `/fpv` for admins | `flying-controls` |
| Shooting Range | WilhelmTest range with an all-items menu, respawn after 3 s | `range-map` |
| Island Gun Game | Gun Game on the hidden DevTest island | `dev-test-map` |
| Capture the Princess | Actual Castle against New Castle, bots fill both teams, loadouts to choose, helicopters for kills, base turrets | `flying-controls`, `mod-settings`, `destruction` |

The client mods a preset needs are added to the share code automatically.

### Bots

Set `game.bots` and the server adds AI players. They are normal players to every client. Bots jump
from the plane, land with a random gun, walk to the ring on the island's walkability grid, swim to
shore, take storm damage, and fight anyone they can see. Their aim is fair: they hold fire for 5 s
after landing, react after 0.5–1.2 s, hit at most half their shots, and do 70 % of the gun's damage.

### Admin Commands

Admins are set on the Players page or with `tabg admins add <name>`. These commands work in chat (with
`/`) and in the dashboard console:

| Command | What it does |
|---|---|
| `/start` | Start the match now |
| `/say <text>` | Message everyone |
| `/players` | List players |
| `/kick <name>` · `/ban <name>` | Kick or ban (bans are saved to `tabg.toml`) |
| `/give <item> [qty]` | Drop an item next to you |
| `/heal` | Full health |
| `/weather [name\|auto]` | Show the weather, or set it for everyone with Visuals: Clear, Overcast, Mist, Rain, Storm, Flurries, Snowfall, Blizzard, Haze, Sandstorm; `auto` follows the biome again |
| `/vehicles` · `/vehicle <name>` | List vehicles, or spawn one in front of you (FlyingControls players) |
| `/fpv` | Get an FPV drone |

### Command Line

Every command talks to the running host, so they also work while the window is open.

| Command | What it does |
|---|---|
| `tabg` / `tabg gui` | Open the window |
| `tabg host [--no-browser]` | Run the host without a window and open the browser dashboard |
| `tabg setup [--force]` | Find TABG and export the map data |
| `tabg start` · `stop` · `restart` | Control the game server |
| `tabg status` | Join code, phase and players |
| `tabg console` | Type admin commands |
| `tabg say` · `kick` · `ban` · `unban` | Talk to or moderate players |
| `tabg admins [add\|remove <name>]` | Manage admins |
| `tabg config show\|get\|set\|path` | Read or change `tabg.toml` |
| `tabg preset list\|apply\|save` | Built-in and your own presets |
| `tabg backup list\|restore <id>` | Config backups |
| `tabg logs [-n 50] [-f]` | Server log |
| `tabg share` | Make the share code for friends |
| `tabg join <code> [--launch]` · `tabg launch` | Get ready for a friend's server, start the modded copy |

## Client Mods

All of them are optional. Friends get them through the share code, never by hand.

| Mod | Catalog id | Description | Key |
|---|---|---|---|
| ProximityChat | `proximity-chat` | Voice chat with nearby players, over the game's own connection | |
| FlyingControls | `flying-controls` | Fly helicopters and UFOs, fire missiles, FPV drones, call hover taxis | |
| RangeMap | `range-map` | Loads the WilhelmTest shooting range, with a searchable all-items menu | F6 |
| DevTestMap | `dev-test-map` | Loads the hidden DevTest island, with a searchable all-items menu | F6 |
| CustomGameSkins | `custom-game-skins` | Wardrobe with every built-in skin, only on servers that allow it | F7 |
| AdminRadar | `admin-radar` | Bot radar: shows **bots only**, never real players | F4 |
| CustomGrenades | `custom-grenades` | Big Smoke grenade effects and MGL flashbangs | |
| CoordsDisplay | `coords-display` | Coordinate overlay | F5 |
| ModSettings | `mod-settings` | In-game settings for the other mods | F9 |
| PerformanceClient | `performance-client` | FPS, culling, streaming and network optimizations, with a frame-time overlay | F10 |
| PopupBlocker | `popup-blocker` | Hides the anti-cheat popups in the modded copy | |
| Destruction | `destruction` | Explosions break buildings, props and trees (see [Destruction](#destruction)). Added automatically with `game.destruction` | |
| Visuals | `visuals` | Better lighting, clouds, smoke, weather and sea (see [Visuals](#visuals)) | F8 |
| PrincessMode | `princess-mode` | Capture the Princess: a HUD with both princesses, the loadout menu, banners, cages and turrets you can see | F3 |

## Proximity Voice Chat

Voice travels over the game's existing network connection. No extra ports and no voice server are
needed.

- The client sends 16 kHz voice frames (event 240).
- The server checks each frame, limits it to 60 frames per second per speaker, stamps the real
  sender, and passes it only to living players within `game.voice_range` (50 m by default; 0 turns it
  off).
- Voice activation uses a sensitivity threshold. Push-to-talk is optional.
- Turning the mod off stops the microphone right away.

## Flying Vehicles

Set `game.unused_vehicles` (or apply the *Flying Vehicles* preset). The vehicles Landfall built but
never put on the map (helicopters, UFOs, hover bikes and cars, buggies) then spawn next to the
normal cars, up to `game.unused_vehicles_max`.

- **Helicopters** have their own health (30 in the air, 300 on the ground). A hard crash destroys
  them, and when the pilot leaves, a passenger takes over.
- **Missiles** lock on in 0.65 s and fly guided or straight. The server flies them and checks every
  hit. Three bullets shoot one down.
- **FPV drones** (`/fpv`) fly for 60 s with a range of 1 km. The server checks every move.
- **Hover taxis**: pick a destination and the server flies you along the roads, with a short boost.

Vanilla players see the map vehicles. Flying them needs FlyingControls.

## Multiplayer Shooting Range

Apply the **Shooting Range** preset. Every player who joins with the share code loads TABG's built-in
`WilhelmTest` range instead of the island. Press **F6** to take any item, and you respawn 3 s after
dying.

## Island Gun Game

Apply the **Island Gun Game** preset. You play Gun Game on TABG's hidden DevTest island, with **F6**
all-items and unlimited respawns.

## Custom Game All Skins

Set `game.skins = true` (it's off by default) and add `custom-game-skins` to the client mods. Press
**F7** in game to search every built-in skin, pick colours, or randomize an outfit. The server checks
each outfit, limits how often it can change, and shows it to everyone as normal gear.

It doesn't buy anything, doesn't change your PlayFab inventory or saved outfit, and stays locked on
servers that haven't enabled it.

## Destruction

Every explosion breaks the map where it hits: grenades, dynamite, rockets, the Gaussbow, air and
orbital strikes, knockback and black hole grenades. Buildings, walls, containers, pipes, fences,
furniture and rocks crack open, whatever loses its support comes down, and trees fall over.

- Falling pieces are solid: you can stand on them, push them and take cover behind them. They stay
  while anyone is near and disappear once nobody has been within 90 m for 25 s. They are local, so
  each player's pieces land a little differently; the holes are the same for everyone.
- Clients report blasts (event 246). The server merges reports of the same blast, keeps one ordered
  log for the match and sends it to everyone. Each client breaks the buildings the same way from
  that log, so all players with the mod see the same holes, also if they join later.
- Turn on **Destructible buildings** on the Rules tab (or `game.destruction = true`; it's off by
  default). The mod is then added to the share code automatically, so everyone who joins with it
  has it.

## Visuals

A client-side graphics upgrade on top of TABG's own look. It changes nothing other players see,
and every part has a setting in the **F8** menu. Admins can pick the weather for everyone with
`/weather`.

| Part | What it does |
|---|---|
| Light | Sharper soft shadows, stronger ambient occlusion, volumetric sunlight through trees and buildings, sun rays |
| Sky | Volumetric clouds in place of the painted ones, a softer sky gradient and haze |
| Weather | Follows the biome: snow and fog in the snow, rain and storms on grass (with puddles that mirror the world), sandstorms in the desert |
| Smoke | Real 3D smoke from smoke grenades, two to four times as big as the game's, lingering smoke and dust after blasts, a faint haze in grenade fields |
| Explosions | Light up their surroundings, with bloom and glow; flashbangs burn the view into your eyes |
| Sea | Foam, richer colour and the sun glowing through the waves |
| Image | A gentle colour grade and sharpening |

## Capture the Princess

Apply the **Capture the Princess** preset. Two teams play on the island, about 350 m apart:
**Actual Castle** (the blue princess) against **New Castle** (the pink princess, who may hide in its
round tower). Bots fill both teams up to `rules.team_size` (3 by default), so you can also play alone
with two bots against three. Players in one TABG squad play on the same team; everyone else is spread
over both.

- Each team starts next to its **cage** and respawns there. Its princess hides somewhere in its base.
- After every (re)spawn, and when you come home after at least 30 s away, you **choose a loadout**
  from three random ones (Sniper, Rusher, Minigun, Samurai…) with 1, 2 or 3 within 30 s. Without
  the `princess-mode` mod you get the first one straight away.
- **Hold a gun on the other team's princess** and she walks to your cage, slowly. Keep the gun on her
  the whole way (you have to walk; she never gets into a vehicle). Bring her into your cage to win.
- Hold a gun on your **own** princess to walk her: inside your territory she goes where you point,
  outside it she heads home. Both teams holding her at once: she stands still.
- Killed by the **other team**, a princess is back home at once and walks at her normal speed again.
  Killed by **her own team**, she comes back halfway between home and where she fell, but walks
  15 % faster for the other team (up to three times).
- Every few kills (about one per teammate: 3 in teams of three, 8 in teams of ten) a random player of
  the team gets a **helicopter smoke grenade** ("You can now spawn a helicopter"): throw it where the
  helicopter should land. Helicopters are fragile in the air.
- Each base has two **turrets** that fire slow homing missiles at the other team's helicopters; two
  hits bring one down.
- Grenades and rockets break buildings (the preset turns on [Destruction](#destruction)).
- Bots see in front of them (not behind), hear gunfire and running, turn at a human speed and take a
  moment to aim; harder bots see further, react faster and hit more.
- After a win everyone goes back to the menu; once they rejoin, **Start match now** starts the next round.

Settings on the Rules tab: players per team, bot skill (easy, normal, hard: how well the other
team's bots play; yours play at normal), the princess's walking speed, kills per helicopter and the
turrets. Friends need the `flying-controls` and `mod-settings` mods (added to the share code). The
optional `princess-mode` mod shows both princesses on a HUD (F3), the loadout menu, the banners, the
cages and the turrets (a model from Kenney's CC0 Tower Defense Kit); without it the banners arrive
as chat lines. TABG hides its join-code field, so friends find the server in **Server list** (set
`network.public = true`, ideally with a password).

## Configuration

Each server is one `tabg.toml`. The window, the dashboard and the CLI all edit it, and lobby rules reload
without a restart. Unknown keys are errors, so typos don't go unnoticed.

```bash
tabg config show
tabg config set game.max_players 24
tabg config set game.bots 10
tabg config set network.public true      # show in the community server list
```

```toml
name = "tabg server"
preset = "battle-royale"
loadout = [                  # what everyone lands with
  { item = 160, qty = 1 },   # M16
  { item = 6, qty = 240 },   # Normal Ammo
]

[network]
mode = "relay"               # join codes; "direct" for IP:port
port = 7777
upnp = true

[game]
mode = "battle_royale"
map = "island"
teams = "squad"
max_players = 70
password = ""                # empty: no password
players_to_start = 2
bots = 0
bot_radar = "off"            # "off", "admins" or "everyone" (AdminRadar mod)
voice_range = 50.0
skins = false                # all built-in skins (CustomGameSkins mod)
destruction = false          # buildings break; adds the Destruction mod to the share code
unused_vehicles = 0.0
inventory_check = "log"      # "off", "log" (report in the console) or "enforce" (refuse)

[ring]                       # empty lists keep the map's rings
sizes = [4240, 280]          # diameters, largest first
speeds = [25, 20]            # 200 s / speed per shrink: 20 closes in 10 s
center = [105, -65]          # [x, z]: every ring is centred here (Industry); omit for random rings

[players]
admins = []
banned = []

[mods]
client = []                  # e.g. ["proximity-chat", "flying-controls"]
```

Data lives in `~/.local/share/tabg` (Linux) or `%APPDATA%\tabg` (Windows).

## What happened to the old plugins

| Old BepInEx plugin | Now |
|---|---|
| Citruslib, AntiCheatBypass, SteamCMD, EOS DLL copy | Not needed |
| MatchCore (rings, loadouts, drops, spell drops, win rules) | Built into the server: Settings, Loadout & Ring, Rules |
| ServerLogger | Player history on the Players page |
| SoloTesting | `game.players_to_start = 1` |
| ProximityChat server | Built-in voice relay |
| RangeMap / DevTestMap servers | `game.map = "range"` / `"dev_test"` |
| CustomGameSkins server | `game.skins` (opt-in) |
| FakePlayers | `game.bots` |
| DummyDebugRadar | `game.bot_radar` (bots only) |
| UnusedVehicles | `game.unused_vehicles` |
| PerformanceServer | Not needed: the Rust server is efficient on its own |
| EnhancedClient, StarterPack, WeaponSpawnConfig, TestMod | Dropped |

## Building

You need [Rust](https://rustup.rs/) 1.85 or newer. The client mods also need the .NET SDK.

```bash
cargo build --release -p tabg            # target/release/tabg
scripts/build-release.sh                 # Linux + Windows release binaries into target/dist/

# client mods: point at a TABG client copy that has BepInEx installed
export TABG_CLIENT_DIR=~/.local/share/tabg/test-client
dotnet build mods/Mods.sln -c Release
scripts/release-mods.sh                  # mod catalog into target/mods-dist/
```

The Visuals mod embeds shaders built with the same Unity version TABG uses (2021.3.45f2).
`scripts/build-shaders.sh` downloads that editor once (about 4 GB) and builds the asset bundles
headless into `target/bundles/`. Unity needs a free Personal licence, signed in once with Unity Hub.

Game, Unity and BepInEx assemblies are never committed. The mods compile against your own install.

There is no CI. Run all checks locally before pushing:

```bash
scripts/check.sh          # Rust: fmt, clippy, tests
scripts/check.sh --mods   # also the C# client mods and their tests
```

Developer tools:

```bash
cargo run --release -p tabg-server -- --world target/tabg-world.json   # just the game server
cargo run --release -p tabg-probe -- --host 127.0.0.1                  # headless test client
scripts/reference-server.sh <dedicated-server-dir>  # Landfall's server, offline, as ground truth
scripts/real-client-test.sh                         # the real client joins automatically
scripts/dump-world.sh <dedicated-server-dir>        # export the map data by hand
```

For writing mods, these unpack your own install into `target/` (never commit or share the output):

```bash
scripts/decompile.sh [client|server|both]           # game code as C# projects
scripts/extract-assets.sh [client|server] [unity|content|sounds|shaders|all]
                                                    # UI, prefabs, models, textures, sounds, shaders
```

The design, all decisions and the milestones are in [docs/DESIGN.md](docs/DESIGN.md).

Want to help? Read [CONTRIBUTING.md](CONTRIBUTING.md) for the workflow and the code and design style.
Changes reach `main` through pull requests.

### Repo layout

```
crates/   Rust workspace: tabg (CLI), tabg-gui (window), tabg-host, tabg-web (dashboard), tabg-server and
          tabg-game (the game server), tabg-protocol, tabg-net, tabg-utp (relay), tabg-world
mods/     C# BepInEx client mods and the map dumper; mods/unity is the Unity project for mod shaders
presets/  built-in presets
scripts/  build, release and test scripts
docs/     design
```

## Disclaimer

This software is provided **as-is** with no warranty of any kind. Use at your own risk.

- This project is **not affiliated with, endorsed by, or associated with**
  [Landfall Games](https://landfall.se/) or Totally Accurate Battlegrounds in any way.
- tabg contains no Landfall code or assets. Map data is extracted from your own copy of the game, on
  your machine, and is never redistributed.

## Credits

### Third-Party Code

| Component | Author | Use |
|---|---|---|
| [ENet-CSharp](https://github.com/nxrighthere/ENet-CSharp) 2.4.8 | Stanislav Denisov and the ENet authors | Network library, vendored in `crates/tabg-enet-sys/vendor` (MIT) |
| [htmx](https://htmx.org/) | Big Sky Software | Dashboard, in `crates/tabg-web/assets` (BSD Zero) |
| [tabg-word-list](https://github.com/landfallgames/tabg-word-list) | Landfall Games | Server name filter for the community list (MIT) |
| [Inter](https://github.com/rsms/inter) | The Inter Project Authors | Window font, in `crates/tabg-gui/assets/fonts` (OFL) |
| [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) | The JetBrains Mono Project Authors | Window font for numbers and logs, in `crates/tabg-gui/assets/fonts` (OFL) |

### Downloaded at Runtime (pinned by sha256)

| Component | Author | Use |
|---|---|---|
| [BepInEx](https://github.com/BepInEx/BepInEx) 5.4.23.3 | BepInEx Team | Mod loader for the modded client copy and the map export |
| [HarmonyLib](https://github.com/pardeike/Harmony) | Andreas Pardeike | Runtime patching, bundled with BepInEx |
| [AntiCheatBootErrorRemover](https://github.com/C0mputery/AntiCheatBootErrorRemover) 1.0 | [C0mputery](https://github.com/C0mputery) | Lets the modded copy start without Easy Anti-Cheat |

### Developer Tools (used by the scripts, never shipped)

| Component | Author | Use |
|---|---|---|
| [Unity](https://unity.com/) 2021.3.45f2 | Unity Technologies | Builds the Visuals shaders (`build-shaders.sh`) |
| [AssetRipper](https://github.com/AssetRipper/AssetRipper) | AssetRipper contributors | Asset extraction (GPL-3.0) |
| [vgmstream](https://github.com/vgmstream/vgmstream) | vgmstream contributors | FMOD sound banks to wav (ISC) |
| [USCSandbox](https://github.com/nesrak1/USCSandbox) | nesrak1 | Shader decompiler, with our patch (GPL-3.0) |
| [ILSpy](https://github.com/icsharpcode/ILSpy) | ICSharpCode | Decompiling the game code (MIT) |

### Thanks

- [**CyrusTheLesser**](https://github.com/CyrusTheLesser) for [Citruslib](https://github.com/CyrusTheLesser/Citruslib).
  The old installer depended on it, and the admin command set people know comes from it.
- The Rust crates in `Cargo.lock`, especially egui, tokio, axum, maud, clap and ureq.

> **Know someone who should be credited?** Open an issue or DM me on Discord.

## Contact

Have questions, feature requests, or found a bug?

- **Discord:** `anonymer__hase_22156`
- **GitHub Issues:** [Open an issue](https://github.com/jonassneu/TABG-Server-Installer/issues)

## License

Released under the **MIT License**. See [`LICENSE`](./LICENSE).

Vendored and downloaded third-party components keep their own licenses.
