//! The match rules form. Edits stay in the draft until the bar at the bottom saves them.

use std::collections::BTreeSet;

use eframe::egui::{self, Rect, RichText, Ui, pos2, vec2};
use tabg_host::ServerToml;
use tabg_host::config::{BotRadar, BotSkill, GameMode, InventoryCheck, Map, Problem, Teams};

use crate::app::Gui;
use crate::draft::{self, percent, time_of_day};
use crate::tabs::Tab;
use crate::theme::*;
use crate::widgets::*;

const LEFT_LABEL: f32 = 264.0;
const RIGHT_LABEL: f32 = 268.0;
const NUM: f32 = 88.0;

fn changed_tag(ui: &mut Ui, changed: bool) {
    if changed {
        ui.label(RichText::new("changed").font(mono()).color(ACCENT));
    }
}

fn problem(ui: &mut Ui, problems: &[Problem], field: &str, width: f32) {
    for p in problems.iter().filter(|p| p.field == field) {
        hint(
            ui,
            width,
            text(crate::app::capitalized(&p.message)).color(DANGER),
        );
    }
}

struct Ctx<'a> {
    saved: &'a ServerToml,
    problems: &'a [Problem],
}

fn left(ui: &mut Ui, d: &mut ServerToml, cx: &Ctx, collapsed: &mut BTreeSet<String>) {
    let w = LEFT_LABEL;
    section(ui, collapsed, "Mode", true, |ui| {
        field(ui, "Mode", w, |ui| {
            select(
                ui,
                "mode",
                &mut d.game.mode,
                &[
                    (GameMode::BattleRoyale, "Battle Royale"),
                    (GameMode::Test, "Test (no plane, no ring)"),
                    (GameMode::Brawl, "Brawl"),
                    (GameMode::Bomb, "Bomb"),
                    (GameMode::Princess, "Capture the Princess"),
                ],
                170.0,
            );
        });
        hint(ui, w, muted("Changing the mode restarts the server"));
        field(ui, "Map", w, |ui| {
            select(
                ui,
                "map",
                &mut d.game.map,
                &[
                    (Map::Island, "Island"),
                    (Map::Range, "Shooting range"),
                    (Map::DevTest, "DevTest island"),
                ],
                170.0,
            );
        });
        problem(ui, cx.problems, "game.map", w);
        problem(ui, cx.problems, "mods.client", w);
        field(ui, "Teams", w, |ui| {
            select(
                ui,
                "teams",
                &mut d.game.teams,
                &[
                    (Teams::Solo, "Solo"),
                    (Teams::Duo, "Duos"),
                    (Teams::Squad, "Squads"),
                ],
                110.0,
            );
        });
    });
    section(ui, collapsed, "Starting a match", false, |ui| {
        field(ui, "Players needed", w, |ui| {
            number(ui, "start", &mut d.game.players_to_start, NUM, 1.0);
        });
        hint(
            ui,
            w,
            muted(format!(
                "Bots count. At most the max players, {}",
                d.game.max_players
            )),
        );
        problem(ui, cx.problems, "game.players_to_start", w);
        field(ui, "Countdown", w, |ui| {
            number(ui, "countdown", &mut d.game.countdown, NUM, 1.0);
            ui.label(text("s").color(MUTED));
        });
        problem(ui, cx.problems, "game.countdown", w);
        field(ui, "Start anyway after", w, |ui| {
            number(ui, "force", &mut d.game.force_start_after, NUM, 10.0);
            ui.label(text("s  with at least").color(MUTED));
            number(
                ui,
                "force-min",
                &mut d.game.force_start_min_players,
                64.0,
                1.0,
            );
        });
        hint(ui, w, muted("0 never forces a start"));
        problem(ui, cx.problems, "game.force_start_after", w);
    });
    section(ui, collapsed, "On the island", false, |ui| {
        field(ui, "Loot", w, |ui| {
            slider(ui, "Loot", &mut d.game.loot_amount, 180.0);
            let changed = d.game.loot_amount != cx.saved.game.loot_amount;
            ui.label(
                RichText::new(percent(d.game.loot_amount))
                    .font(body())
                    .color(if changed { ACCENT } else { TEXT }),
            );
        });
        let was = if d.game.loot_amount != cx.saved.game.loot_amount {
            format!("Was {}. ", percent(cx.saved.game.loot_amount))
        } else {
            String::new()
        };
        hint(
            ui,
            w,
            muted(format!(
                "{was}Share of loot spots that get an item, from the next match"
            )),
        );
        problem(ui, cx.problems, "game.loot_amount", w);
        field(ui, "Time of day", w, |ui| {
            slider(ui, "Time of day", &mut d.game.time_of_day, 180.0);
            ui.label(text(time_of_day(d.game.time_of_day)));
        });
        field(ui, "Bots", w, |ui| {
            number(ui, "bots", &mut d.game.bots, NUM, 1.0);
        });
        hint(ui, w, muted("Bots plus max players can be 250 at most"));
        problem(ui, cx.problems, "game.bots", w);
        field(ui, "Hidden vehicles", w, |ui| {
            let mut pct = d.game.unused_vehicles * 100.0;
            if number(ui, "vehicles", &mut pct, 72.0, 5.0) {
                d.game.unused_vehicles = (pct / 100.0).clamp(0.0, 1.0);
            }
            ui.label(text("% chance, up to").color(MUTED));
            number(
                ui,
                "vehicles-max",
                &mut d.game.unused_vehicles_max,
                64.0,
                1.0,
            );
        });
        hint(
            ui,
            w,
            muted("Helicopters, UFOs, hover bikes. Flying needs FlyingControls"),
        );
        problem(ui, cx.problems, "game.unused_vehicles", w);
        problem(ui, cx.problems, "game.unused_vehicles_max", w);
        field(ui, "Spell drops", w, |ui| {
            check(ui, "Spell drops", &mut d.game.spell_drops);
            changed_tag(ui, d.game.spell_drops != cx.saved.game.spell_drops);
        });
        field(ui, "Respawn minigame", w, |ui| {
            check(ui, "Respawn minigame", &mut d.game.respawn_minigame);
            changed_tag(
                ui,
                d.game.respawn_minigame != cx.saved.game.respawn_minigame,
            );
        });
        hint(
            ui,
            w,
            muted("An early death goes to a boss fight for a second chance"),
        );
    });
}

fn right(
    ui: &mut Ui,
    d: &mut ServerToml,
    cx: &Ctx,
    collapsed: &mut BTreeSet<String>,
    edit_levels: &mut bool,
) {
    let w = RIGHT_LABEL;
    section(ui, collapsed, "Respawn", true, |ui| {
        field(ui, "Respawn after dying", w, |ui| {
            check(ui, "Respawn after dying", &mut d.rules.respawn);
            changed_tag(ui, d.rules.respawn != cx.saved.rules.respawn);
        });
        hint(ui, w, muted("Turns the match into a deathmatch"));
        field(ui, "Respawn delay", w, |ui| {
            number(ui, "delay", &mut d.rules.respawn_delay, NUM, 1.0);
            ui.label(text("s").color(MUTED));
        });
        problem(ui, cx.problems, "rules.respawn_delay", w);
        field(ui, "Kills to win", w, |ui| {
            number(ui, "kills", &mut d.rules.kills_to_win, NUM, 1.0);
        });
        hint(ui, w, muted("0 means no limit"));
        field(ui, "Time limit", w, |ui| {
            let mut minutes = d.rules.time_limit / 60.0;
            if number(ui, "limit", &mut minutes, NUM, 1.0) {
                d.rules.time_limit = minutes * 60.0;
            }
            ui.label(text("min").color(MUTED));
        });
        hint(ui, w, muted("Most kills wins. 0 means none"));
        field(ui, "Gun game", w, |ui| {
            let levels = d.rules.gun_game.len();
            let state = if levels == 0 {
                "off".to_owned()
            } else {
                format!("{levels} levels")
            };
            ui.label(text(state).color(MUTED));
            if plain(ui, "Edit levels") {
                *edit_levels = true;
            }
        });
        problem(ui, cx.problems, "rules.gun_game", w);
    });
    if d.game.mode == GameMode::Princess {
        section(ui, collapsed, "Capture the Princess", true, |ui| {
            field(ui, "Players per team", w, |ui| {
                number(ui, "team-size", &mut d.rules.team_size, NUM, 1.0);
            });
            hint(ui, w, muted("Bots fill the free places. 0 means no bots"));
            problem(ui, cx.problems, "rules.team_size", w);
            field(ui, "Bot skill", w, |ui| {
                select(
                    ui,
                    "bot-skill",
                    &mut d.rules.bot_skill,
                    &[
                        (BotSkill::Easy, "Easy"),
                        (BotSkill::Normal, "Normal"),
                        (BotSkill::Hard, "Hard"),
                    ],
                    110.0,
                );
            });
            hint(
                ui,
                w,
                muted("For the other team's bots. Yours play at normal"),
            );
            field(ui, "Princess speed", w, |ui| {
                number(ui, "princess-speed", &mut d.rules.princess_speed, NUM, 0.5);
                ui.label(text("m/s").color(MUTED));
            });
            hint(ui, w, muted("How fast she walks while held at gunpoint"));
            problem(ui, cx.problems, "rules.princess_speed", w);
            field(ui, "Kills per helicopter", w, |ui| {
                number(ui, "heli-kills", &mut d.rules.heli_kills, NUM, 1.0);
            });
            hint(ui, w, muted("0 means one per teammate, 3 to 8"));
            problem(ui, cx.problems, "rules.heli_kills", w);
            field(ui, "Base turrets", w, |ui| {
                check(ui, "Base turrets", &mut d.rules.turrets);
                changed_tag(ui, d.rules.turrets != cx.saved.rules.turrets);
            });
            hint(
                ui,
                w,
                muted("Two per base, homing missiles at enemy helicopters"),
            );
        });
    }
    if d.game.mode == GameMode::Bomb {
        section(ui, collapsed, "Bomb", false, |ui| {
            field(ui, "Rounds to win", w, |ui| {
                number(ui, "rounds", &mut d.rules.rounds_to_win, NUM, 1.0);
            });
            problem(ui, cx.problems, "rules.rounds_to_win", w);
            for (name, key, value) in [
                ("Round time", "rules.round_time", &mut d.rules.round_time),
                ("Bomb timer", "rules.bomb_time", &mut d.rules.bomb_time),
                ("Defuse time", "rules.defuse_time", &mut d.rules.defuse_time),
            ] {
                field(ui, name, w, |ui| {
                    number(ui, key, value, NUM, 1.0);
                    ui.label(text("s").color(MUTED));
                });
                problem(ui, cx.problems, key, w);
            }
        });
    }
    section(ui, collapsed, "Advanced", false, |ui| {
        field(ui, "Tick rate", w, |ui| {
            number(ui, "tick", &mut d.game.tick_rate, NUM, 1.0);
        });
        problem(ui, cx.problems, "game.tick_rate", w);
        field(ui, "Inventory check", w, |ui| {
            select(
                ui,
                "inventory",
                &mut d.game.inventory_check,
                &[
                    (InventoryCheck::Off, "Off"),
                    (InventoryCheck::Log, "Log"),
                    (InventoryCheck::Enforce, "Enforce"),
                ],
                100.0,
            );
        });
        field(ui, "Voice range", w, |ui| {
            number(ui, "voice", &mut d.game.voice_range, NUM, 5.0);
            ui.label(text("m").color(MUTED));
        });
        hint(ui, w, muted("Needs ProximityChat"));
        problem(ui, cx.problems, "game.voice_range", w);
        field(ui, "Any outfit", w, |ui| {
            check(ui, "Any outfit", &mut d.game.skins);
            changed_tag(ui, d.game.skins != cx.saved.game.skins);
        });
        hint(ui, w, muted("Needs CustomGameSkins"));
        field(ui, "Destructible buildings", w, |ui| {
            check(ui, "Destructible buildings", &mut d.game.destruction);
            changed_tag(ui, d.game.destruction != cx.saved.game.destruction);
        });
        hint(ui, w, muted("Adds Destruction to the share code"));
        field(ui, "Bot radar", w, |ui| {
            select(
                ui,
                "radar",
                &mut d.game.bot_radar,
                &[
                    (BotRadar::Off, "Off"),
                    (BotRadar::Admins, "Admins"),
                    (BotRadar::Everyone, "Everyone"),
                ],
                110.0,
            );
        });
        hint(ui, w, muted("Needs AdminRadar"));
    });
}

pub fn show(gui: &mut Gui, ui: &mut Ui) {
    let area = ui.available_rect_before_wrap();
    let gap = 10.0;
    let width = ((area.width() - gap) / 2.0).floor();
    let left_rect = Rect::from_min_size(area.min, vec2(width, area.height()));
    let right_rect = Rect::from_min_max(pos2(left_rect.right() + gap, area.top()), area.max);
    let saved = gui.saved.clone();
    let merged = draft::merged(draft::Section::Rules, &gui.draft, &saved);
    let problems = merged.validate();
    let cx = Ctx {
        saved: &saved,
        problems: &problems,
    };
    let mut d = std::mem::take(&mut gui.draft);
    let mut collapsed = std::mem::take(&mut gui.t.collapsed);
    let mut edit_levels = false;
    within(ui, left_rect, |ui| {
        card_fill(
            ui,
            "Match rules",
            |_| {},
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rules-left")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        left(ui, &mut d, &cx, &mut collapsed);
                    });
            },
        );
    });
    within(ui, right_rect, |ui| {
        card_fill(
            ui,
            "More rules",
            |_| {},
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("rules-right")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        right(ui, &mut d, &cx, &mut collapsed, &mut edit_levels);
                    });
            },
        );
    });
    gui.draft = d;
    gui.t.collapsed = collapsed;
    if edit_levels {
        gui.tab = Tab::Loadout;
    }
}
