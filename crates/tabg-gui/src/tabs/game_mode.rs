//! Presets: pick one, see what it changes, use it.

use eframe::egui::{self, Align, Layout, Rect, Ui, pos2, vec2};
use tabg_host::ServerToml;
use tabg_host::presets::{self, Preset};

use crate::app::Gui;
use crate::draft::{self, Items};
use crate::theme::*;
use crate::widgets::*;

const GROUPS: [(&str, &[&str]); 3] = [
    (
        "Battle royale",
        &[
            "battle-royale",
            "solo-royale",
            "quick-match",
            "loot-frenzy",
            "armed-drop",
            "flying-vehicles",
        ],
    ),
    (
        "Respawn modes",
        &["deathmatch", "gun-game", "island-gun-game"],
    ),
    (
        "Other modes",
        &["capture-the-princess", "brawl", "bomb", "shooting-range"],
    ),
];

fn after(gui: &Gui, preset: &Preset) -> Result<ServerToml, String> {
    presets::apply(&gui.saved, preset)
}

fn preset_row(gui: &mut Gui, ui: &mut Ui, preset: &Preset) {
    let selected = gui.t.preset == preset.id;
    let in_use = gui.saved.preset == preset.id;
    let restarts = after(gui, preset).is_ok_and(|a| gui.saved.needs_restart(&a));
    let response = row(ui, 32.0, selected.then_some(SELECTED), |ui, rect| {
        let title = if in_use {
            egui::RichText::new(&preset.title)
                .font(semi(15.0))
                .color(TEXT)
        } else {
            text(&preset.title)
        };
        cell(ui, rect, 16.0, rect.width() - 140.0, title);
        let mut x = rect.width() - 16.0;
        if restarts {
            cell_right(ui, rect, x, muted("restarts"));
            x -= 62.0;
        }
        if in_use {
            cell_right(
                ui,
                rect,
                x,
                egui::RichText::new("in use").font(semi(13.0)).color(MUTED),
            );
        }
    });
    if response.clicked() {
        gui.t.preset = preset.id.clone();
    }
}

fn list_card(gui: &mut Gui, ui: &mut Ui) {
    let presets = gui.presets.clone();
    card_fill(
        ui,
        "Presets",
        |_| {},
        |ui| {
            egui::ScrollArea::vertical()
                .id_salt("presets")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add_space(8.0);
                    for (name, ids) in GROUPS {
                        row(ui, 30.0, None, |ui, rect| {
                            cell(ui, rect, 16.0, 200.0, muted(name));
                        });
                        for id in ids {
                            if let Some(p) = presets.iter().find(|p| p.id == *id) {
                                preset_row(gui, ui, p);
                            }
                        }
                        ui.add_space(8.0);
                    }
                    let own: Vec<&Preset> = presets.iter().filter(|p| !p.builtin).collect();
                    if !own.is_empty() {
                        row(ui, 30.0, None, |ui, rect| {
                            cell(ui, rect, 16.0, 200.0, muted("Your presets"));
                        });
                        for p in own {
                            preset_row(gui, ui, p);
                        }
                        ui.add_space(8.0);
                    }
                });
        },
    );
}

fn new_preset(gui: &mut Gui, ui: &mut Ui) {
    card(
        ui,
        "New preset",
        |_| {},
        |ui| {
            padded(ui, |ui| {
                ui.label(text("Save the current rules as a preset"));
                right_row(ui, |ui| {
                    let id = draft::slug(&gui.t.preset_name);
                    let save = ui
                        .add_enabled_ui(!id.is_empty(), |ui| plain(ui, "Save"))
                        .inner;
                    let width = ui.available_width() - 2.0;
                    input(ui, &mut gui.t.preset_name, width, "Name");
                    if save {
                        let title = gui.t.preset_name.trim().to_owned();
                        let paths = gui.host.app.paths.presets();
                        match presets::save(&paths, &id, &title, "", &gui.saved) {
                            Ok(()) => {
                                gui.presets = presets::all(&paths);
                                gui.t.preset = id;
                                gui.t.preset_name.clear();
                                gui.tell("Preset saved.");
                            }
                            Err(e) => gui.complain(e),
                        }
                    }
                });
            });
        },
    );
}

fn changes_table(ui: &mut Ui, changes: &[draft::Change]) {
    ui.label(muted("What changes"));
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, LINE))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let (x1, x2) = (16.0, ui.available_width() * 0.335);
            let x3 = ui.available_width() * 0.56;
            let columns = |ui: &mut Ui, rect: Rect, a, b, c| {
                cell(ui, rect, x1, x2 - x1 - 8.0, a);
                cell(ui, rect, x2, x3 - x2 - 8.0, b);
                cell(ui, rect, x3, rect.width() - x3 - 8.0, c);
            };
            row(ui, 34.0, Some(HEADER), |ui, rect| {
                columns(ui, rect, muted("Setting"), muted("Now"), muted("After"));
            });
            for c in changes {
                hline(ui, LINE_SOFT);
                row(ui, 32.0, None, |ui, rect| {
                    columns(
                        ui,
                        rect,
                        text(c.label),
                        text(&c.before).color(MUTED),
                        text(&c.after),
                    );
                });
            }
        });
}

fn preview(gui: &mut Gui, ui: &mut Ui) {
    let selected = gui.presets.iter().find(|p| p.id == gui.t.preset).cloned();
    card_fill(
        ui,
        "Preview",
        |_| {},
        |ui| {
            let Some(preset) = selected else {
                padded(ui, |ui| {
                    ui.label(muted("Pick a preset on the left."));
                });
                return;
            };
            let result = after(gui, &preset);
            let area = ui.available_rect_before_wrap();
            let footer_top = area.bottom() - 56.0;
            let body = Rect::from_min_max(area.min, pos2(area.right(), footer_top));
            within(ui, body, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("preset-preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        padded(ui, |ui| {
                            ui.label(
                                egui::RichText::new(&preset.title)
                                    .font(semi(19.0))
                                    .color(TEXT),
                            );
                            label(ui, text(&preset.description).color(MUTED));
                            ui.add_space(6.0);
                            match &result {
                                Ok(next) => details(gui, ui, next),
                                Err(e) => {
                                    label(ui, text(e).color(DANGER));
                                }
                            }
                        });
                    });
            });
            let footer = Rect::from_min_max(pos2(area.left(), footer_top), area.max);
            let can_use = result.as_ref().is_ok_and(|n| n.validate().is_empty()) && !gui.applying;
            let already =
                gui.saved.preset == preset.id && result.as_ref().is_ok_and(|n| *n == gui.saved);
            within(ui, footer.shrink2(vec2(20.0, 0.0)), |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(can_use && !already, |ui| {
                        let title = format!("Use {}", preset.title);
                        if primary(ui, &title)
                            && let Ok(next) = result.clone()
                        {
                            let what = format!("Switching to {}", preset.title);
                            gui.apply_confirmed(next, format!("the {} preset", preset.title), what);
                        }
                    });
                });
            });
        },
    );
}

fn details(gui: &Gui, ui: &mut Ui, next: &ServerToml) {
    let restart = gui.saved.needs_restart(next);
    let items: &Items = &gui.map.items;
    let tail = if restart {
        let n = gui.online();
        if n > 0 {
            format!("Restarts the server, which disconnects the {n} players online.")
        } else {
            "Restarts the server if it's running.".to_owned()
        }
    } else {
        "Applies without a restart; the change starts with the next match.".to_owned()
    };
    label(
        ui,
        muted(format!(
            "Keeps the name, password, admins and bans. tabg saves a backup first. {tail}"
        )),
    );
    for p in next.validate() {
        label(
            ui,
            text(format!("{}: {}", p.field, p.message)).color(DANGER),
        );
    }
    ui.add_space(6.0);
    let changes = draft::changes(&gui.saved, next, items);
    if changes.is_empty() {
        ui.label(muted("This preset matches your current settings."));
    } else {
        changes_table(ui, &changes);
    }
}

pub fn show(gui: &mut Gui, ui: &mut Ui) {
    let area = ui.available_rect_before_wrap();
    let left_w = 372.0;
    let left = Rect::from_min_size(area.min, vec2(left_w, area.height()));
    let right = Rect::from_min_max(pos2(left.right() + 10.0, area.top()), area.max);
    within(ui, left, |ui| {
        let new_h = 122.0;
        let list = Rect::from_min_max(left.min, pos2(left.right(), left.bottom() - new_h - 10.0));
        let new = Rect::from_min_max(pos2(left.left(), list.bottom() + 10.0), left.max);
        within(ui, list, |ui| list_card(gui, ui));
        within(ui, new, |ui| new_preset(gui, ui));
    });
    within(ui, right, |ui| preview(gui, ui));
}
