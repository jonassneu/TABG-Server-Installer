//! Editing `tabg.toml`: which part of the config each form owns, and how to say in words what a
//! change does.

use std::collections::BTreeMap;

use tabg_host::ServerToml;
use tabg_host::config::{BotRadar, BotSkill, GameMode, InventoryCheck, Map, NetMode, Teams};

pub type Items = BTreeMap<i32, String>;

/// A part of the config with its own Save button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Rules,
    Server,
    Ring,
    Loadout,
    GunGame,
    Mods,
}

impl Section {
    pub const ALL: [Self; 6] = [
        Self::Rules,
        Self::Server,
        Self::Ring,
        Self::Loadout,
        Self::GunGame,
        Self::Mods,
    ];

    /// Why the backup was made: "Before {reason}".
    pub fn reason(self) -> &'static str {
        match self {
            Self::Rules | Self::Server => "a settings change",
            Self::Ring => "a ring change",
            Self::Loadout => "a loadout change",
            Self::GunGame => "a gun game change",
            Self::Mods => "a mods change",
        }
    }
}

/// Copy what `section` owns from `from` into `to`.
pub fn copy_section(section: Section, from: &ServerToml, to: &mut ServerToml) {
    match section {
        Section::Rules => {
            let password = std::mem::take(&mut to.game.password);
            let max_players = to.game.max_players;
            let levels = std::mem::take(&mut to.rules.gun_game);
            to.game = from.game.clone();
            to.rules = from.rules.clone();
            to.game.password = password;
            to.game.max_players = max_players;
            to.rules.gun_game = levels;
        }
        Section::Server => {
            to.name.clone_from(&from.name);
            to.description.clone_from(&from.description);
            to.game.password.clone_from(&from.game.password);
            to.game.max_players = from.game.max_players;
            to.network = from.network.clone();
        }
        Section::Ring => to.ring = from.ring.clone(),
        Section::Loadout => to.loadout.clone_from(&from.loadout),
        Section::GunGame => to.rules.gun_game.clone_from(&from.rules.gun_game),
        Section::Mods => to.mods = from.mods.clone(),
    }
}

/// `to` with `section` taken from `from`.
pub fn merged(section: Section, from: &ServerToml, to: &ServerToml) -> ServerToml {
    let mut out = to.clone();
    copy_section(section, from, &mut out);
    out
}

/// Whether `a` and `b` agree on everything `section` owns.
pub fn same(section: Section, a: &ServerToml, b: &ServerToml) -> bool {
    merged(section, a, b) == *b
}

pub struct Field {
    pub label: &'static str,
    pub section: Section,
    pub get: fn(&ServerToml, &Items) -> String,
}

pub fn on_off(v: bool) -> String {
    if v { "on" } else { "off" }.to_owned()
}

fn seconds(v: f32) -> String {
    format!("{} s", crate::widgets::format_number(f64::from(v)))
}

pub fn percent(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

pub fn mode_name(m: GameMode) -> &'static str {
    match m {
        GameMode::BattleRoyale => "Battle Royale",
        GameMode::Test => "Test",
        GameMode::Brawl => "Brawl",
        GameMode::Bomb => "Bomb",
        GameMode::Princess => "Capture the Princess",
    }
}

pub fn skill_name(s: BotSkill) -> &'static str {
    match s {
        BotSkill::Easy => "Easy",
        BotSkill::Normal => "Normal",
        BotSkill::Hard => "Hard",
    }
}

pub fn teams_name(t: Teams) -> &'static str {
    match t {
        Teams::Solo => "Solo",
        Teams::Duo => "Duos",
        Teams::Squad => "Squads",
    }
}

pub fn map_name(m: Map) -> &'static str {
    match m {
        Map::Island => "Island",
        Map::Range => "Shooting range",
        Map::DevTest => "DevTest island",
    }
}

pub fn time_of_day(v: f32) -> &'static str {
    match v {
        v if v < 0.19 => "Night",
        v if v < 0.38 => "Morning",
        v if v < 0.62 => "Noon",
        v if v < 0.82 => "Evening",
        _ => "Night",
    }
}

pub fn item_name(items: &Items, id: i32) -> String {
    items
        .get(&id)
        .cloned()
        .unwrap_or_else(|| format!("Item {id}"))
}

fn gun_levels(c: &ServerToml, items: &Items) -> String {
    let levels = &c.rules.gun_game;
    let name = |level: Option<&Vec<tabg_host::config::LoadoutItem>>| {
        level
            .and_then(|l| l.first())
            .map(|i| item_name(items, i.item))
            .unwrap_or_default()
    };
    match levels.len() {
        0 => "none".into(),
        1 => format!("1, {}", name(levels.first())),
        n => format!(
            "{n}, from {} to {}",
            name(levels.first()),
            name(levels.last())
        ),
    }
}

fn ring_text(c: &ServerToml) -> String {
    let r = &c.ring;
    if r.times.is_empty() && r.sizes.is_empty() && r.speeds.is_empty() {
        return "map values".into();
    }
    let stages = r.times.len().max(r.sizes.len()).max(r.speeds.len());
    let times: f32 = r.times.iter().sum();
    if times > 0.0 {
        format!("custom, {stages} stages, {times:.0} s of waiting")
    } else {
        format!("custom, {stages} stages")
    }
}

/// Every setting a person would recognise, for "what changes" lists.
pub const FIELDS: &[Field] = &[
    Field {
        label: "Mode",
        section: Section::Rules,
        get: |c, _| mode_name(c.game.mode).into(),
    },
    Field {
        label: "Map",
        section: Section::Rules,
        get: |c, _| map_name(c.game.map).into(),
    },
    Field {
        label: "Teams",
        section: Section::Rules,
        get: |c, _| teams_name(c.game.teams).into(),
    },
    Field {
        label: "Players needed",
        section: Section::Rules,
        get: |c, _| c.game.players_to_start.to_string(),
    },
    Field {
        label: "Countdown",
        section: Section::Rules,
        get: |c, _| seconds(c.game.countdown),
    },
    Field {
        label: "Start anyway",
        section: Section::Rules,
        get: |c, _| {
            if c.game.force_start_after <= 0.0 {
                "never".into()
            } else {
                format!(
                    "after {} with at least {}",
                    seconds(c.game.force_start_after),
                    c.game.force_start_min_players
                )
            }
        },
    },
    Field {
        label: "Loot",
        section: Section::Rules,
        get: |c, _| percent(c.game.loot_amount),
    },
    Field {
        label: "Time of day",
        section: Section::Rules,
        get: |c, _| time_of_day(c.game.time_of_day).into(),
    },
    Field {
        label: "Bots",
        section: Section::Rules,
        get: |c, _| c.game.bots.to_string(),
    },
    Field {
        label: "Hidden vehicles",
        section: Section::Rules,
        get: |c, _| {
            if c.game.unused_vehicles <= 0.0 {
                "off".into()
            } else {
                format!(
                    "{} chance, up to {}",
                    percent(c.game.unused_vehicles),
                    c.game.unused_vehicles_max
                )
            }
        },
    },
    Field {
        label: "Spell drops",
        section: Section::Rules,
        get: |c, _| on_off(c.game.spell_drops),
    },
    Field {
        label: "Respawn minigame",
        section: Section::Rules,
        get: |c, _| on_off(c.game.respawn_minigame),
    },
    Field {
        label: "Respawn after dying",
        section: Section::Rules,
        get: |c, _| on_off(c.rules.respawn),
    },
    Field {
        label: "Respawn delay",
        section: Section::Rules,
        get: |c, _| seconds(c.rules.respawn_delay),
    },
    Field {
        label: "Kills to win",
        section: Section::Rules,
        get: |c, _| match c.rules.kills_to_win {
            0 => "no limit".into(),
            n => n.to_string(),
        },
    },
    Field {
        label: "Time limit",
        section: Section::Rules,
        get: |c, _| {
            if c.rules.time_limit <= 0.0 {
                "none".into()
            } else {
                format!("{:.0} min", c.rules.time_limit / 60.0)
            }
        },
    },
    Field {
        label: "Rounds to win",
        section: Section::Rules,
        get: |c, _| c.rules.rounds_to_win.to_string(),
    },
    Field {
        label: "Round time",
        section: Section::Rules,
        get: |c, _| seconds(c.rules.round_time),
    },
    Field {
        label: "Bomb timer",
        section: Section::Rules,
        get: |c, _| seconds(c.rules.bomb_time),
    },
    Field {
        label: "Defuse time",
        section: Section::Rules,
        get: |c, _| seconds(c.rules.defuse_time),
    },
    Field {
        label: "Team size",
        section: Section::Rules,
        get: |c, _| c.rules.team_size.to_string(),
    },
    Field {
        label: "Bot skill",
        section: Section::Rules,
        get: |c, _| skill_name(c.rules.bot_skill).into(),
    },
    Field {
        label: "Princess speed",
        section: Section::Rules,
        get: |c, _| {
            format!(
                "{} m/s",
                crate::widgets::format_number(f64::from(c.rules.princess_speed))
            )
        },
    },
    Field {
        label: "Kills per helicopter",
        section: Section::Rules,
        get: |c, _| match c.rules.heli_kills {
            0 => "by team size".into(),
            n => n.to_string(),
        },
    },
    Field {
        label: "Base turrets",
        section: Section::Rules,
        get: |c, _| on_off(c.rules.turrets),
    },
    Field {
        label: "Tick rate",
        section: Section::Rules,
        get: |c, _| c.game.tick_rate.to_string(),
    },
    Field {
        label: "Inventory check",
        section: Section::Rules,
        get: |c, _| {
            match c.game.inventory_check {
                InventoryCheck::Off => "Off",
                InventoryCheck::Log => "Log",
                InventoryCheck::Enforce => "Enforce",
            }
            .into()
        },
    },
    Field {
        label: "Voice range",
        section: Section::Rules,
        get: |c, _| format!("{:.0} m", c.game.voice_range),
    },
    Field {
        label: "Any outfit",
        section: Section::Rules,
        get: |c, _| on_off(c.game.skins),
    },
    Field {
        label: "Destructible buildings",
        section: Section::Rules,
        get: |c, _| on_off(c.game.destruction),
    },
    Field {
        label: "Bot radar",
        section: Section::Rules,
        get: |c, _| {
            match c.game.bot_radar {
                BotRadar::Off => "Off",
                BotRadar::Admins => "Admins",
                BotRadar::Everyone => "Everyone",
            }
            .into()
        },
    },
    Field {
        label: "Name",
        section: Section::Server,
        get: |c, _| c.name.clone(),
    },
    Field {
        label: "Description",
        section: Section::Server,
        get: |c, _| c.description.clone(),
    },
    Field {
        label: "Password",
        section: Section::Server,
        get: |c, _| c.game.password.clone(),
    },
    Field {
        label: "Max players",
        section: Section::Server,
        get: |c, _| c.game.max_players.to_string(),
    },
    Field {
        label: "Show in server browser",
        section: Section::Server,
        get: |c, _| on_off(c.network.public),
    },
    Field {
        label: "How friends join",
        section: Section::Server,
        get: |c, _| {
            match c.network.mode {
                NetMode::Relay => "Join code",
                NetMode::Direct => "Direct IP",
            }
            .into()
        },
    },
    Field {
        label: "Relay region",
        section: Section::Server,
        get: |c, _| c.network.region.clone(),
    },
    Field {
        label: "UDP port",
        section: Section::Server,
        get: |c, _| c.network.port.to_string(),
    },
    Field {
        label: "Open the port with UPnP",
        section: Section::Server,
        get: |c, _| on_off(c.network.upnp),
    },
    Field {
        label: "Ring",
        section: Section::Ring,
        get: |c, _| ring_text(c),
    },
    Field {
        label: "Landing loadout",
        section: Section::Loadout,
        get: |c, _| match c.loadout.len() {
            1 => "1 item".into(),
            n => format!("{n} items"),
        },
    },
    Field {
        label: "Gun game levels",
        section: Section::GunGame,
        get: gun_levels,
    },
    Field {
        label: "Client mods",
        section: Section::Mods,
        get: |c, _| match c.mods.client.len() {
            0 => "none".into(),
            _ => c.mods.client.join(", "),
        },
    },
];

/// One changed setting.
pub struct Change {
    pub label: &'static str,
    pub before: String,
    pub after: String,
}

pub fn changes(before: &ServerToml, after: &ServerToml, items: &Items) -> Vec<Change> {
    FIELDS
        .iter()
        .filter_map(|f| {
            let (b, a) = ((f.get)(before, items), (f.get)(after, items));
            (a != b).then_some(Change {
                label: f.label,
                before: b,
                after: a,
            })
        })
        .collect()
}

pub fn section_changes(
    section: Section,
    before: &ServerToml,
    after: &ServerToml,
    items: &Items,
) -> Vec<&'static str> {
    FIELDS
        .iter()
        .filter(|f| f.section == section && (f.get)(before, items) != (f.get)(after, items))
        .map(|f| f.label)
        .collect()
}

/// "Loot and respawn after dying"
pub fn sentence(labels: &[&str]) -> String {
    let lower = |i: usize, s: &str| {
        if i == 0 {
            s.to_owned()
        } else {
            // Words that are names stay capitalised.
            let mut c = s.chars();
            match c.next() {
                Some(first) if !s.contains("UPnP") => {
                    first.to_lowercase().collect::<String>() + c.as_str()
                }
                _ => s.to_owned(),
            }
        }
    };
    let parts: Vec<String> = labels
        .iter()
        .enumerate()
        .map(|(i, s)| lower(i, s))
        .collect();
    match parts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A preset id for a name people typed: `Friday chaos` → `friday-chaos`.
pub fn slug(name: &str) -> String {
    let mapped: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    mapped
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_cover_only_what_they_own() {
        let saved = ServerToml::default();
        let mut draft = saved.clone();
        draft.game.loot_amount = 0.5;
        draft.game.password = "secret".into();
        assert!(!same(Section::Rules, &draft, &saved));
        assert!(!same(Section::Server, &draft, &saved));
        assert!(same(Section::Ring, &draft, &saved));

        // Saving the rules must not save the password typed on the server page.
        let next = merged(Section::Rules, &draft, &saved);
        assert_eq!(next.game.loot_amount, 0.5);
        assert_eq!(next.game.password, "");
    }

    #[test]
    fn gun_game_is_not_part_of_the_rules_form() {
        let saved = ServerToml::default();
        let mut draft = saved.clone();
        draft.rules.gun_game = vec![vec![tabg_host::config::LoadoutItem { item: 1, qty: 1 }]];
        assert!(same(Section::Rules, &draft, &saved));
        assert!(!same(Section::GunGame, &draft, &saved));
    }

    #[test]
    fn every_section_change_is_named() {
        // A form that can be dirty with no listed change would show a blank "unsaved" bar.
        let saved = ServerToml::default();
        let items = Items::new();
        let mut d = saved.clone();
        d.game.loot_amount = 0.25;
        d.rules.respawn = true;
        assert_eq!(
            section_changes(Section::Rules, &saved, &d, &items),
            ["Loot", "Respawn after dying"]
        );
        d = saved.clone();
        d.network.region = "europe-west4".into();
        assert_eq!(
            section_changes(Section::Server, &saved, &d, &items),
            ["Relay region"]
        );
    }

    #[test]
    fn words_for_people() {
        assert_eq!(
            sentence(&["Loot", "Respawn after dying"]),
            "Loot and respawn after dying"
        );
        assert_eq!(sentence(&["A", "B", "C"]), "A, b and c");
        assert_eq!(slug("Friday chaos!"), "friday-chaos");
        assert_eq!(slug("  --  "), "");
        assert_eq!(time_of_day(0.25), "Morning");
        assert_eq!(percent(0.1), "10%");
    }
}
