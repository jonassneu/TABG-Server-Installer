//! Who's online, the match, and what just happened.

use std::collections::BTreeMap;

use eframe::egui::{self, CornerRadius, Rect, RichText, Sense, Ui, vec2};
use tabg_game::PlayerInfo;
use tabg_host::config::Teams;
use tabg_host::supervisor::LogLine;

use crate::app::{Dialog, Gui, clock_short, is_bot};
use crate::draft::{mode_name, teams_name};
use crate::theme::*;
use crate::widgets::*;

const ROW: f32 = 35.0;

pub fn health_text(p: &PlayerInfo) -> RichText {
    if !p.alive {
        monospace("dead").color(FAINT)
    } else if p.health < 25.0 {
        monospace(format!("{:.0}", p.health)).color(DANGER)
    } else {
        monospace(format!("{:.0}", p.health))
    }
}

pub fn suffix(p: &PlayerInfo) -> Option<&'static str> {
    if is_bot(p) {
        Some("bot")
    } else if p.admin {
        Some("admin")
    } else {
        None
    }
}

/// The heading: "14 players in 5 squads, 4 bots".
pub fn online_title(players: &[PlayerInfo], teams: Teams) -> String {
    let n = players.len();
    let mut groups: Vec<u8> = players.iter().map(|p| p.team).collect();
    groups.sort_unstable();
    groups.dedup();
    let bots = players.iter().filter(|p| is_bot(p)).count();
    let people = if n == 1 { "player" } else { "players" };
    let mut title = format!("Online now · {n} {people}");
    if teams != Teams::Solo && !groups.is_empty() {
        let word = match (teams, groups.len()) {
            (Teams::Duo, 1) => "duo",
            (Teams::Duo, _) => "duos",
            (_, 1) => "squad",
            _ => "squads",
        };
        title += &format!(" in {} {word}", groups.len());
    }
    if bots > 0 {
        title += &format!(", {bots} bot{}", if bots == 1 { "" } else { "s" });
    }
    title
}

/// Activity worth showing a host: what happened in the game and what they did themselves.
pub fn is_activity(l: &LogLine) -> bool {
    matches!(l.source.as_str(), "game" | "chat")
        || (l.source == "host" && l.text.starts_with("You "))
}

/// Log text without the player id on join lines.
pub fn tidy(text: &str) -> &str {
    match text.find(" joined (") {
        Some(i) if text.ends_with(')') => &text[..i + " joined".len()],
        _ => text,
    }
}

fn player_row(gui: &mut Gui, ui: &mut Ui, p: &PlayerInfo) {
    let name_color = if p.alive { TEXT } else { FAINT };
    let job = name_with(&p.name, suffix(p), name_color);
    let health = health_text(p);
    let kills = monospace(p.kills.to_string());
    row(ui, ROW, None, |ui, rect| {
        cell(ui, rect, 16.0, 290.0, job);
        cell_right(ui, rect, 398.0, health);
        cell_right(ui, rect, 492.0, kills);
        cell_actions(ui, rect, 16.0, |ui| {
            if is_bot(p) {
                if plain(ui, "Remove") {
                    gui.kick(p);
                }
            } else {
                if warn(ui, "Ban") {
                    gui.dialog = Some(Dialog::Ban {
                        id: p.id.clone(),
                        name: p.name.clone(),
                    });
                }
                if plain(ui, "Kick") {
                    gui.kick(p);
                }
            }
        });
    });
}

fn online(gui: &mut Gui, ui: &mut Ui) {
    let players = gui.status.players.clone();
    let princess = gui.saved.game.mode == tabg_host::config::GameMode::Princess;
    let title = online_title(&players, gui.saved.game.teams);
    card_fill(
        ui,
        &title,
        |_| {},
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            row(ui, 40.0, None, |ui, rect| {
                cell(ui, rect, 16.0, 200.0, muted("Player"));
                cell_right(ui, rect, 398.0, muted("Health"));
                cell_right(ui, rect, 492.0, muted("Kills"));
            });
            hline(ui, LINE_SOFT);
            if players.is_empty() {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(muted("Nobody online."));
                });
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("online")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if gui.saved.game.teams == Teams::Solo {
                        for p in &players {
                            player_row(gui, ui, p);
                        }
                        return;
                    }
                    let mut groups: BTreeMap<u8, Vec<&PlayerInfo>> = BTreeMap::new();
                    for p in &players {
                        groups.entry(p.team).or_default().push(p);
                    }
                    for (i, (team, members)) in groups.iter().enumerate() {
                        if i > 0 {
                            hline(ui, LINE_SOFT);
                        }
                        let title = if princess {
                            match team {
                                0 => "Actual Castle".to_owned(),
                                1 => "New Castle".to_owned(),
                                _ => "Princess".to_owned(),
                            }
                        } else {
                            format!("Squad {}", team + 1)
                        };
                        row(ui, 34.0, None, |ui, rect| {
                            cell(ui, rect, 16.0, 200.0, muted(title));
                        });
                        for p in members {
                            player_row(gui, ui, p);
                        }
                    }
                });
        },
    );
}

fn phases(ui: &mut Ui, phase: &str) {
    const PHASES: [(&str, &str); 4] = [
        ("lobby", "Lobby"),
        ("countdown", "Countdown"),
        ("match", "In match"),
        ("ended", "Ended"),
    ];
    let width = (ui.available_width() - 15.0) / 4.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        for (id, title) in PHASES {
            let (rect, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
            let on = id == phase;
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(FIELD_RADIUS),
                if on { ACCENT } else { CHIP },
            );
            let (font, color) = if on {
                (semi(15.0), egui::Color32::WHITE)
            } else {
                (body(), MUTED)
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                title,
                font,
                color,
            );
        }
    });
}

fn fact(ui: &mut Ui, name: &str, value: RichText) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(70.0, 22.0), Sense::hover());
        ui.painter().text(
            egui::pos2(rect.left(), rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            body(),
            MUTED,
        );
        ui.label(value);
    });
}

fn match_card(gui: &mut Gui, ui: &mut Ui) {
    let phase = gui.status.phase.clone();
    let preset = gui
        .presets
        .iter()
        .find(|p| p.id == gui.saved.preset)
        .map_or_else(|| gui.saved.preset.clone(), |p| p.title.clone());
    let game = &gui.saved.game;
    let mode = format!(
        "{}, {}",
        mode_name(game.mode),
        teams_name(game.teams).to_lowercase()
    );
    let restarts = gui.status.restarts;
    card(
        ui,
        "Match",
        |_| {},
        |ui| {
            padded(ui, |ui| {
                phases(ui, &phase);
                ui.add_space(2.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                fact(ui, "Preset", text(preset));
                fact(ui, "Mode", text(mode));
                fact(ui, "Restarts", monospace(restarts.to_string()));
                if phase == "ended" {
                    ui.label(muted(
                        "Players go back to the menu in a moment; start the next round once they rejoin",
                    ));
                }
            });
        },
    );
}

fn message_card(gui: &mut Gui, ui: &mut Ui) {
    card(
        ui,
        "Message everyone",
        |_| {},
        |ui| {
            padded(ui, |ui| {
                right_row(ui, |ui| {
                    let can_send = !gui.t.message.trim().is_empty();
                    let send = ui.add_enabled_ui(can_send, |ui| primary(ui, "Send")).inner;
                    let width = ui.available_width() - 2.0;
                    let edit = input(ui, &mut gui.t.message, width, "Shows in game chat");
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (send || enter) && can_send {
                        let message = std::mem::take(&mut gui.t.message);
                        gui.console(
                            format!("say {message}"),
                            Some(format!("You said: {message}")),
                        );
                    }
                });
            });
        },
    );
}

fn activity(gui: &mut Gui, ui: &mut Ui) {
    let lines: Vec<&LogLine> = gui.log.iter().filter(|l| is_activity(l)).collect();
    card_fill(
        ui,
        "Latest activity",
        |_| {},
        |ui| {
            ui.add_space(8.0);
            let line_height = 26.0;
            egui::ScrollArea::vertical()
                .id_salt("activity")
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show_rows(ui, line_height, lines.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for l in &lines[range] {
                        let (rect, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), line_height),
                            Sense::hover(),
                        );
                        cell(
                            ui,
                            rect,
                            16.0,
                            54.0,
                            monospace(clock_short(l.at)).color(MUTED),
                        );
                        cell(
                            ui,
                            rect,
                            70.0,
                            rect.width() - 80.0,
                            monospace(tidy(&l.text)),
                        );
                    }
                });
        },
    );
}

pub fn show(gui: &mut Gui, ui: &mut Ui) {
    let area = ui.available_rect_before_wrap();
    let left_w = (area.width() * 0.583).floor();
    let left = Rect::from_min_size(area.min, vec2(left_w, area.height()));
    let right = Rect::from_min_max(egui::pos2(left.right() + 10.0, area.top()), area.max);
    within(ui, left, |ui| online(gui, ui));
    within(ui, right, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        match_card(gui, ui);
        message_card(gui, ui);
        activity(gui, ui);
    });
}
