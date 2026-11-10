//! `tabg.toml`: everything about one server, as the host edits it. The dashboard and CLI only
//! change this file and then apply it (§5.2 of the design).

use serde::{Deserialize, Serialize};
use tabg_game::{ForceStart, RoomSettings};

/// How players reach the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetMode {
    /// Unity Relay: players type a join code. No port forwarding.
    #[default]
    Relay,
    /// Players connect to `your-ip:port`. Needs UPnP or a forwarded UDP port.
    Direct,
}

/// Who sees the bots on the admin-radar client mod.
/// What the server does when a client drops, throws or uses an item it never got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InventoryCheck {
    /// Trust clients, like the official server.
    Off,
    /// Allow it and write a line to the server log.
    #[default]
    Log,
    /// Refuse it.
    Enforce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BotRadar {
    #[default]
    Off,
    Admins,
    Everyone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Teams {
    Solo,
    Duo,
    #[default]
    Squad,
}

impl Teams {
    pub fn match_mode(self) -> u8 {
        use tabg_server::listing::squad_mode_name;
        let byte = match self {
            Self::Solo => 0,
            Self::Squad => 1,
            Self::Duo => 2,
        };
        debug_assert!(!squad_mode_name(byte).is_empty());
        byte
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GameMode {
    #[default]
    BattleRoyale,
    /// TABG's Test mode: no plane, no ring, everyone starts where they stand.
    Test,
    /// TABG's Brawl: teams start in spawn houses on the castle map; a wiped team respawns; most
    /// kills (`rules.kills_to_win`) wins.
    Brawl,
    /// TABG's Bomb (the TABG_CS map): two teams, rounds with a buy phase, plant and defuse.
    Bomb,
    /// Capture the Princess on the island: Actual Castle against New Castle. Hold a gun on the
    /// other team's princess to walk her to your cage. Friends need the flying-controls mod.
    Princess,
}

/// How well the bots of a team without players play (Capture the Princess).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BotSkill {
    Easy,
    #[default]
    Normal,
    Hard,
}

/// Which map to play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Map {
    /// The main island.
    #[default]
    Island,
    /// WilhelmTest shooting range (friends need the `range-map` client mod).
    Range,
    /// The DevTest island (friends need the `dev-test-map` client mod).
    DevTest,
}

impl Map {
    /// Spawn points of the mod maps (from the old, verified presets).
    pub fn spawns(self) -> &'static [[f32; 3]] {
        match self {
            Self::Island => &[],
            Self::Range => &[[6.32, 117.10, -25.41]],
            Self::DevTest => &[
                [-37.0, 111.0, -11.0],
                [6.0, 112.0, 21.0],
                [-2.0, 111.0, 2.0],
            ],
        }
    }

    /// The client mod friends need for this map.
    pub fn client_mod(self) -> Option<&'static str> {
        match self {
            Self::Island => None,
            Self::Range => Some("range-map"),
            Self::DevTest => Some("dev-test-map"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Network {
    pub mode: NetMode,
    /// Direct mode: the UDP port.
    pub port: u16,
    /// Direct mode: open the port on the router with UPnP.
    pub upnp: bool,
    /// Relay mode: region like `europe-west4`; empty lets Unity pick.
    pub region: String,
    /// Show the server in TABG's community server browser.
    pub public: bool,
}

impl Default for Network {
    fn default() -> Self {
        Self {
            mode: NetMode::Relay,
            port: 7777,
            upnp: true,
            region: String::new(),
            public: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Game {
    pub mode: GameMode,
    /// The island, or a map from a client mod (played in Test mode).
    pub map: Map,
    pub teams: Teams,
    pub max_players: u8,
    /// Empty: no password.
    pub password: String,
    /// The countdown starts once this many players are in.
    pub players_to_start: u8,
    /// Countdown length in seconds.
    pub countdown: f32,
    /// Start anyway after this many seconds in the lobby (0 = never)…
    pub force_start_after: f32,
    /// …if at least this many players are there.
    pub force_start_min_players: u8,
    /// Lighting, 0..1 (0.25 = morning).
    pub time_of_day: f32,
    /// Share of loot spots that get loot each match (the official default is 0.1).
    pub loot_amount: f32,
    /// Movement updates per second.
    pub tick_rate: u32,
    /// Proximity voice range in metres for friends with the ProximityChat mod (0 = off).
    pub voice_range: f32,
    /// Let friends with the custom-game-skins mod wear any built-in outfit.
    pub skins: bool,
    /// Blessing drops fall from the sky during Battle Royale (official default: on).
    pub spell_drops: bool,
    /// An early death in Battle Royale goes to the boss fight for a second chance.
    pub respawn_minigame: bool,
    /// Bots that join the island's matches to fill them up.
    pub bots: u8,
    /// Show the bots on the admin-radar client mod: off, admins, everyone.
    pub bot_radar: BotRadar,
    /// Buildings can be blown apart with grenades and rockets. Adds the destruction client mod
    /// to the share code.
    pub destruction: bool,
    /// Check drops, throws and healing against what the server saw a player pick up:
    /// off, log (report in the server log) or enforce (refuse). Ammo is never checked.
    pub inventory_check: InventoryCheck,
    /// Chance per map car to add one of the game's unused vehicles near it (helicopter, UFO,
    /// hover bike and car, …); 0 = off. Flying them needs the flying-controls mod.
    pub unused_vehicles: f32,
    /// At most this many unused vehicles per match.
    pub unused_vehicles_max: u8,
}

impl Default for Game {
    fn default() -> Self {
        let d = RoomSettings::default();
        let f = d.force_start.unwrap_or(ForceStart {
            after_seconds: 0.0,
            min_players: 2,
        });
        Self {
            mode: GameMode::BattleRoyale,
            map: Map::Island,
            teams: Teams::Squad,
            max_players: d.max_players,
            password: String::new(),
            players_to_start: d.players_to_start,
            countdown: d.countdown,
            force_start_after: f.after_seconds,
            force_start_min_players: f.min_players,
            time_of_day: d.time_of_day,
            loot_amount: d.loot_keep,
            tick_rate: d.tick_rate,
            voice_range: d.voice_range,
            skins: false,
            spell_drops: true,
            respawn_minigame: false,
            bots: 0,
            bot_radar: BotRadar::Off,
            destruction: false,
            inventory_check: InventoryCheck::Log,
            unused_vehicles: 0.0,
            unused_vehicles_max: 15,
        }
    }
}

/// Ring overrides; empty lists keep the map's own values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Ring {
    /// Pause after each ring, in seconds.
    pub times: Vec<f32>,
    /// Ring diameters, largest first.
    pub sizes: Vec<f32>,
    /// How fast each ring closes in.
    pub speeds: Vec<f32>,
    /// `[x, z]`: every ring is centred here (a fixed arena) instead of at a random spot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center: Option<[f32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadoutItem {
    pub item: i32,
    #[serde(default = "one")]
    pub qty: i32,
}

fn one() -> i32 {
    1
}

/// Match rules beyond plain Battle Royale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rules {
    /// Dead players come back (deathmatch, gun game).
    pub respawn: bool,
    /// Seconds before coming back.
    pub respawn_delay: f32,
    /// With respawns: first team to this many kills wins (0 = no limit).
    pub kills_to_win: u16,
    /// With respawns: the match ends after this many seconds; most kills wins (0 = no limit).
    pub time_limit: f32,
    /// Gun game levels: each kill moves you to the next level's items; finishing wins.
    pub gun_game: Vec<Vec<LoadoutItem>>,
    /// Bomb: rounds a team needs to win (official default 13).
    pub rounds_to_win: u8,
    /// Bomb: seconds per round.
    pub round_time: f32,
    /// Bomb: seconds from plant to explosion.
    pub bomb_time: f32,
    /// Bomb: seconds to defuse.
    pub defuse_time: f32,
    /// Capture the Princess: players per team; bots fill the free places (0 = no bots).
    pub team_size: u8,
    /// Capture the Princess: how well the other team's bots play (easy, normal, hard). Bots on
    /// your own team play at normal.
    pub bot_skill: BotSkill,
    /// Capture the Princess: the princess's walking speed in m/s while held at gunpoint.
    pub princess_speed: f32,
    /// Capture the Princess: enemy kills that earn a helicopter (0 = one per teammate, 3 to 8).
    pub heli_kills: u8,
    /// Capture the Princess: each base has two turrets that fire homing missiles at enemy
    /// helicopters.
    pub turrets: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            respawn: false,
            respawn_delay: 5.0,
            kills_to_win: 20,
            time_limit: 0.0,
            gun_game: Vec::new(),
            rounds_to_win: 13,
            round_time: 90.0,
            bomb_time: 30.0,
            defuse_time: 5.0,
            team_size: 3,
            bot_skill: BotSkill::Normal,
            princess_speed: 3.5,
            heli_kills: 0,
            turrets: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Players {
    /// Names or player ids (`epic:…`); `*` makes everyone an admin.
    pub admins: Vec<String>,
    /// Names or player ids that may not join.
    pub banned: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Mods {
    /// Client mods friends need (shared with `tabg share`): catalog `id@version`.
    pub client: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerToml {
    /// Name in the server browser (only words from TABG's word list).
    pub name: String,
    pub description: String,
    /// The preset this config was last based on (informational).
    pub preset: String,
    pub network: Network,
    pub game: Game,
    pub ring: Ring,
    pub rules: Rules,
    /// Items every player gets on landing (and on every respawn).
    pub loadout: Vec<LoadoutItem>,
    pub players: Players,
    pub mods: Mods,
}

impl Default for ServerToml {
    fn default() -> Self {
        Self {
            name: "tabg server".into(),
            description: String::new(),
            preset: "battle-royale".into(),
            network: Network::default(),
            game: Game::default(),
            ring: Ring::default(),
            rules: Rules::default(),
            loadout: Vec::new(),
            players: Players::default(),
            mods: Mods::default(),
        }
    }
}

/// One problem with a config: which field and what's wrong.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub field: String,
    pub message: String,
}

fn problem(field: &str, message: impl Into<String>) -> Problem {
    Problem {
        field: field.into(),
        message: message.into(),
    }
}

const WORDS: &str = include_str!("../data/tabg-words.txt");

/// Words the community server list refuses (it only allows Landfall's word list).
pub fn unlisted_words(text: &str) -> Vec<String> {
    let mut bad = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let lower = word.to_lowercase();
        if !WORDS.lines().any(|w| w == lower) && !bad.contains(&lower) {
            bad.push(lower);
        }
    }
    bad
}

pub const HEADER: &str = "\
# tabg server config. Edit here or in the dashboard, then `tabg apply` (the dashboard applies for
# you). Unknown keys are errors. See docs/DESIGN.md §5.2.
";

/// Catalog id of the client mod `game.destruction` needs.
pub const DESTRUCTION_MOD: &str = "destruction";
/// Catalog id of the mod that flies helicopters (Capture the Princess needs it).
pub const FLYING_MOD: &str = "flying-controls";
/// Catalog id of the in-game settings mod (FlyingControls fails to load without it).
pub const SETTINGS_MOD: &str = "mod-settings";

impl ServerToml {
    /// The client mods friends need: `mods.client`, plus the Destruction mod when
    /// `game.destruction` is on and FlyingControls for Capture the Princess (unless listed).
    pub fn client_mods(&self) -> Vec<String> {
        let mut mods = self.mods.client.clone();
        let mut need = |id: &str, on: bool| {
            let listed = mods.iter().any(|e| e.split('@').next() == Some(id));
            if on && !listed {
                mods.push(id.into());
            }
        };
        need(DESTRUCTION_MOD, self.game.destruction);
        // FlyingControls doesn't load without ModSettings (it uses its types), so both.
        need(FLYING_MOD, self.game.mode == GameMode::Princess);
        need(SETTINGS_MOD, self.game.mode == GameMode::Princess);
        mods
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> String {
        format!(
            "{HEADER}\n{}",
            toml::to_string_pretty(self).expect("config serializes")
        )
    }

    /// Everything wrong with this config (empty when it's fine to apply).
    pub fn validate(&self) -> Vec<Problem> {
        let mut p = Vec::new();
        if self.name.trim().is_empty() {
            p.push(problem("name", "give the server a name"));
        }
        if self.name.chars().count() > 64 {
            p.push(problem("name", "at most 64 characters"));
        }
        if self.network.public {
            for (field, text) in [("name", &self.name), ("description", &self.description)] {
                let bad = unlisted_words(text);
                if !bad.is_empty() {
                    p.push(problem(
                        field,
                        format!(
                            "the server browser only allows TABG's word list; not allowed: {}",
                            bad.join(", ")
                        ),
                    ));
                }
            }
        }
        if self.network.mode == NetMode::Direct && self.network.port < 1024 {
            p.push(problem("network.port", "use a port from 1024 to 65535"));
        }
        let g = &self.game;
        if !(1..=250).contains(&g.max_players) {
            p.push(problem("game.max_players", "1 to 250 players"));
        }
        if g.players_to_start == 0 || g.players_to_start > g.max_players {
            p.push(problem(
                "game.players_to_start",
                "at least 1 and at most max_players",
            ));
        }
        if !(0.0..=600.0).contains(&g.countdown) {
            p.push(problem("game.countdown", "0 to 600 seconds"));
        }
        if g.force_start_after < 0.0 {
            p.push(problem(
                "game.force_start_after",
                "0 (never) or more seconds",
            ));
        }
        if !(0.0..=1.0).contains(&g.time_of_day) {
            p.push(problem("game.time_of_day", "between 0 and 1"));
        }
        if !(0.0..=1.0).contains(&g.loot_amount) {
            p.push(problem("game.loot_amount", "between 0 and 1"));
        }
        if !(0.0..=1000.0).contains(&g.voice_range) {
            p.push(problem("game.voice_range", "0 (off) to 1000 metres"));
        }
        if !(0.0..=1.0).contains(&g.unused_vehicles) {
            p.push(problem("game.unused_vehicles", "between 0 (off) and 1"));
        }
        if g.unused_vehicles_max > 50 {
            p.push(problem("game.unused_vehicles_max", "at most 50"));
        }
        if g.bots > 50 || usize::from(g.bots) + usize::from(g.max_players) > 250 {
            p.push(problem(
                "game.bots",
                "at most 50 bots, and bots + max players at most 250",
            ));
        }
        if !(1..=60).contains(&g.tick_rate) {
            p.push(problem("game.tick_rate", "1 to 60 per second"));
        }
        for (field, list) in [
            ("ring.times", &self.ring.times),
            ("ring.sizes", &self.ring.sizes),
            ("ring.speeds", &self.ring.speeds),
        ] {
            if list.iter().any(|v| !v.is_finite() || *v <= 0.0) {
                p.push(problem(field, "only positive numbers"));
            }
        }
        if self
            .ring
            .center
            .is_some_and(|c| c.iter().any(|v| !v.is_finite() || v.abs() > 5000.0))
        {
            p.push(problem(
                "ring.center",
                "[x, z] inside the map (-5000 to 5000)",
            ));
        }
        if !self.ring.sizes.is_empty() && self.ring.sizes.windows(2).any(|w| w[1] > w[0]) {
            p.push(problem(
                "ring.sizes",
                "each ring must be smaller than the one before",
            ));
        }
        if g.mode == GameMode::Test && g.map == Map::Island {
            p.push(problem(
                "game.map",
                "Test mode needs the range or dev_test map (TABG loads its DevTest scene for Test mode)",
            ));
        }
        if matches!(g.mode, GameMode::Brawl | GameMode::Bomb) && g.map != Map::Island {
            p.push(problem(
                "game.map",
                "Brawl and Bomb have their own maps; use map = \"island\"",
            ));
        }
        if g.mode == GameMode::Princess && g.map != Map::Island {
            p.push(problem(
                "game.map",
                "Capture the Princess is played on the island (Actual Castle against New Castle)",
            ));
        }
        if self.rules.team_size > 10 {
            p.push(problem("rules.team_size", "0 (no bots) to 10 per team"));
        }
        if !(1.0..=8.0).contains(&self.rules.princess_speed) {
            p.push(problem("rules.princess_speed", "1 to 8 m/s"));
        }
        if self.rules.heli_kills > 50 {
            p.push(problem("rules.heli_kills", "0 (by team size) to 50"));
        }
        if !(1..=30).contains(&self.rules.rounds_to_win) {
            p.push(problem("rules.rounds_to_win", "1 to 30 rounds"));
        }
        for (field, v) in [
            ("rules.round_time", self.rules.round_time),
            ("rules.bomb_time", self.rules.bomb_time),
            ("rules.defuse_time", self.rules.defuse_time),
        ] {
            if !(1.0..=600.0).contains(&v) {
                p.push(problem(field, "1 to 600 seconds"));
            }
        }
        if let Some(m) = g.map.client_mod()
            && !self
                .mods
                .client
                .iter()
                .any(|c| c.split('@').next() == Some(m))
        {
            p.push(problem(
                "mods.client",
                format!("this map needs the {m} client mod for everyone; add it under Share"),
            ));
        }
        let r = &self.rules;
        if !(0.0..=120.0).contains(&r.respawn_delay) {
            p.push(problem("rules.respawn_delay", "0 to 120 seconds"));
        }
        if r.time_limit < 0.0 {
            p.push(problem("rules.time_limit", "0 (none) or more seconds"));
        }
        if !r.gun_game.is_empty() && !r.respawn {
            p.push(problem("rules.gun_game", "gun game needs respawn = true"));
        }
        for (level, items) in r.gun_game.iter().enumerate() {
            if items.is_empty()
                || items
                    .iter()
                    .any(|i| i.item < 0 || !(1..=999).contains(&i.qty))
            {
                p.push(problem(
                    &format!("rules.gun_game[{level}]"),
                    "each level needs items (id ≥ 0, quantity 1 to 999)",
                ));
            }
        }
        for (i, item) in self.loadout.iter().enumerate() {
            if item.item < 0 || !(1..=999).contains(&item.qty) {
                p.push(problem(
                    &format!("loadout[{i}]"),
                    "item id ≥ 0 and quantity 1 to 999",
                ));
            }
        }
        p
    }

    /// Room settings: these rules on top of the map data in `base` (from [`tabg_server::apply_world`]).
    pub fn room_settings(&self, base: &RoomSettings) -> RoomSettings {
        let g = &self.game;
        let mut s = base.clone();
        s.game_mode = tabg_protocol_game_mode(g.mode);
        s.match_mode = g.teams.match_mode();
        s.max_players = g.max_players;
        s.password = g.password.clone();
        s.players_to_start = g.players_to_start;
        s.countdown = g.countdown;
        s.force_start = (g.force_start_after > 0.0).then_some(ForceStart {
            after_seconds: g.force_start_after,
            min_players: g.force_start_min_players.max(1),
        });
        s.time_of_day = g.time_of_day;
        s.loot_keep = g.loot_amount;
        s.tick_rate = g.tick_rate;
        s.voice_range = g.voice_range;
        s.skins = g.skins;
        if !g.spell_drops {
            s.spell_drops = None;
        }
        s.respawn_minigame = g.respawn_minigame;
        s.bots = g.bots;
        s.vehicles.unused_chance = g.unused_vehicles;
        s.vehicles.unused_max = usize::from(g.unused_vehicles_max);
        s.destruction = g.destruction;
        s.bot_radar = match g.bot_radar {
            BotRadar::Off => tabg_game::BotRadar::Off,
            BotRadar::Admins => tabg_game::BotRadar::Admins,
            BotRadar::Everyone => tabg_game::BotRadar::Everyone,
        };
        s.inventory_check = match g.inventory_check {
            InventoryCheck::Off => tabg_game::InventoryCheck::Off,
            InventoryCheck::Log => tabg_game::InventoryCheck::Log,
            InventoryCheck::Enforce => tabg_game::InventoryCheck::Enforce,
        };
        if g.map != Map::Island {
            // Mod maps: Test mode at the map's own spawns; the island's loot and cars don't apply.
            s.game_mode = 2;
            s.test_map = Some(match g.map {
                Map::Range => tabg_game::TestMap::Range,
                _ => tabg_game::TestMap::DevTest,
            });
            s.spawn_points = g
                .map
                .spawns()
                .iter()
                .map(|&position| tabg_game::SpawnPoint {
                    position,
                    rotation: 0.0,
                })
                .collect();
            s.loot.spots.clear();
            s.cars.clear();
        }
        if !self.ring.times.is_empty() {
            s.ring.ring_times.clone_from(&self.ring.times);
        }
        if !self.ring.sizes.is_empty() {
            s.ring.sizes.clone_from(&self.ring.sizes);
        }
        if !self.ring.speeds.is_empty() {
            s.ring.speeds.clone_from(&self.ring.speeds);
        }
        s.ring.fixed_center = self.ring.center.map(|[x, z]| [x, 0.0, z]);
        s.loadout = self.loadout.iter().map(|l| (l.item, l.qty)).collect();
        let r = &self.rules;
        s.respawn = r.respawn.then_some(tabg_game::Respawn {
            delay: r.respawn_delay,
        });
        s.kills_to_win = r.kills_to_win;
        if let Some(b) = s.bomb.as_mut() {
            b.rounds_to_win = r.rounds_to_win;
            b.round_time = r.round_time;
            b.bomb_time = r.bomb_time;
            b.defuse_time = r.defuse_time;
        }
        if g.mode == GameMode::Princess {
            s.princess = Some(tabg_game::PrincessRules {
                team_size: r.team_size,
                bot_skill: match r.bot_skill {
                    BotSkill::Easy => tabg_game::BotSkill::Easy,
                    BotSkill::Normal => tabg_game::BotSkill::Normal,
                    BotSkill::Hard => tabg_game::BotSkill::Hard,
                },
                walk_speed: r.princess_speed,
                heli_kills: r.heli_kills,
                turrets: r.turrets,
                ..tabg_game::PrincessRules::island()
            });
            // Everyone comes back at their cage; the bots are the team fill, not `game.bots`.
            s.respawn = Some(tabg_game::Respawn {
                delay: r.respawn_delay,
            });
            s.bots = 0;
            s.spell_drops = None;
        }
        s.time_limit = r.time_limit;
        s.gun_game = r
            .gun_game
            .iter()
            .map(|level| level.iter().map(|l| (l.item, l.qty)).collect())
            .collect();
        s.admins.clone_from(&self.players.admins);
        s.banned.clone_from(&self.players.banned);
        s
    }

    /// Whether going from `self` to `next` needs a server restart (network changes do).
    pub fn needs_restart(&self, next: &Self) -> bool {
        self.network != next.network
            || self.name != next.name
            || self.description != next.description
            || self.game.max_players != next.game.max_players
            || self.game.mode != next.game.mode
    }
}

fn tabg_protocol_game_mode(mode: GameMode) -> u8 {
    match mode {
        GameMode::BattleRoyale => 0,
        GameMode::Test => 2,
        GameMode::Brawl => 1,
        GameMode::Bomb => 3,
        // The island's scene loads only in Battle Royale.
        GameMode::Princess => 0,
    }
}

/// Set a dotted key (`game.max_players`) from text, as `tabg config set` does. The value is
/// parsed as TOML (`5`, `true`, `"text"`, `[1, 2]`), falling back to a plain string.
pub fn set_key(config: &ServerToml, key: &str, value: &str) -> Result<ServerToml, String> {
    let mut root = toml::Table::try_from(config).map_err(|e| e.to_string())?;
    let parsed: toml::Value = toml::from_str::<toml::Table>(&format!("v = {value}"))
        .ok()
        .and_then(|mut t| t.remove("v"))
        .unwrap_or_else(|| toml::Value::String(value.to_owned()));
    let parts: Vec<&str> = key.split('.').collect();
    let (last, path) = parts.split_last().ok_or("empty key")?;
    let mut table = &mut root;
    for part in path {
        table = table
            .get_mut(*part)
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| format!("no section {part:?}"))?;
    }
    if !table.contains_key(*last) {
        return Err(format!("unknown key {key:?}"));
    }
    table.insert((*last).to_owned(), parsed);
    toml::Value::Table(root)
        .try_into()
        .map_err(|e: toml::de::Error| format!("{key}: {}", e.message()))
}

/// Read a dotted key as TOML text.
pub fn get_key(config: &ServerToml, key: &str) -> Result<String, String> {
    let root = toml::Value::try_from(config).map_err(|e| e.to_string())?;
    let mut v = &root;
    for part in key.split('.').filter(|p| !p.is_empty()) {
        v = v.get(part).ok_or_else(|| format!("unknown key {key:?}"))?;
    }
    Ok(match v {
        toml::Value::Table(t) => toml::to_string_pretty(t).unwrap_or_default(),
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destruction_adds_its_client_mod_once() {
        let mut c = ServerToml::default();
        assert!(c.client_mods().is_empty());
        c.game.destruction = true;
        c.mods.client = vec!["visuals".into()];
        assert_eq!(c.client_mods(), ["visuals", "destruction"]);
        c.mods.client = vec!["destruction@1.0.0".into()];
        assert_eq!(c.client_mods(), ["destruction@1.0.0"]);
    }

    #[test]
    fn default_round_trips_and_is_valid() {
        let c = ServerToml::default();
        assert_eq!(ServerToml::parse(&c.to_toml()).unwrap(), c);
        assert!(c.validate().is_empty(), "{:?}", c.validate());
    }

    #[test]
    fn partial_files_fill_in_defaults_and_unknown_keys_fail() {
        let c = ServerToml::parse("name = \"good fight\"\n[game]\nteams = \"solo\"\n").unwrap();
        assert_eq!(c.game.teams, Teams::Solo);
        assert_eq!(c.game.max_players, 70);
        assert!(ServerToml::parse("[game]\nmax_player = 3\n").is_err());
    }

    #[test]
    fn validation_names_the_field() {
        let mut c = ServerToml::default();
        c.game.players_to_start = 0;
        c.ring.sizes = vec![100.0, 200.0];
        c.network.public = true;
        c.name = "automated tabg".into();
        let fields: Vec<_> = c.validate().into_iter().map(|p| p.field).collect();
        assert_eq!(fields, ["name", "game.players_to_start", "ring.sizes"]);
        assert_eq!(unlisted_words("tabg dev test, 2!"), Vec::<String>::new());
        assert_eq!(unlisted_words("(ignore) me zzz"), vec!["ignore", "zzz"]);
    }

    #[test]
    fn room_settings_apply_rules_over_map_data() {
        let mut base = RoomSettings::default();
        base.ring.ring_times = vec![1.0];
        let mut c = ServerToml::default();
        c.game.teams = Teams::Duo;
        c.game.force_start_after = 0.0;
        c.loadout = vec![LoadoutItem { item: 6, qty: 60 }];
        c.players.admins = vec!["me".into()];
        let s = c.room_settings(&base);
        assert_eq!(s.match_mode, 2);
        assert_eq!(s.force_start, None);
        assert_eq!(s.ring.ring_times, vec![1.0], "map value kept");
        assert_eq!(s.loadout, vec![(6, 60)]);
        assert_eq!(s.admins, vec!["me".to_string()]);
    }

    #[test]
    fn set_and_get_dotted_keys() {
        let c = ServerToml::default();
        let c = set_key(&c, "game.max_players", "12").unwrap();
        assert_eq!(c.game.max_players, 12);
        let c = set_key(&c, "name", "friday fight").unwrap();
        assert_eq!(c.name, "friday fight");
        let c = set_key(&c, "network.mode", "direct").unwrap();
        assert_eq!(c.network.mode, NetMode::Direct);
        assert_eq!(get_key(&c, "game.max_players").unwrap(), "12");
        assert!(set_key(&c, "game.nope", "1").is_err());
        assert!(set_key(&c, "game.max_players", "lots").is_err());
    }
}
