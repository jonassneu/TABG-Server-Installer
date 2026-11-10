//! Client mods, the host's own DLLs, and the code friends need.

use eframe::egui::{
    self, Align, CornerRadius, Frame, Layout, Margin, Rect, Stroke, Ui, pos2, vec2,
};
use serde_json::Value;
use tabg_host::config::{DESTRUCTION_MOD, NetMode};
use tabg_host::share::CatalogEntry;

use crate::app::{Gui, Msg, Pick};
use crate::draft::Section;
use crate::theme::*;
use crate::widgets::*;

struct ModRow {
    id: &'static str,
    name: &'static str,
    key: &'static str,
    what: &'static str,
}

const fn m(id: &'static str, name: &'static str, key: &'static str, what: &'static str) -> ModRow {
    ModRow {
        id,
        name,
        key,
        what,
    }
}

const MODS: [ModRow; 14] = [
    m(
        "proximity-chat",
        "ProximityChat",
        "",
        "Voice chat with nearby players",
    ),
    m(
        "flying-controls",
        "FlyingControls",
        "",
        "Fly helicopters, UFOs, FPV drones and hover taxis",
    ),
    m(
        "range-map",
        "RangeMap",
        "F6",
        "Shooting range with every item",
    ),
    m(
        "dev-test-map",
        "DevTestMap",
        "F6",
        "Hidden DevTest island with every item",
    ),
    m(
        "custom-game-skins",
        "CustomGameSkins",
        "F7",
        "Every built-in outfit",
    ),
    m(
        "admin-radar",
        "AdminRadar",
        "F4",
        "Radar that shows bots only",
    ),
    m(
        "custom-grenades",
        "CustomGrenades",
        "",
        "Big smoke and flashbangs",
    ),
    m(
        "coords-display",
        "CoordsDisplay",
        "F5",
        "Coordinates on screen",
    ),
    m(
        "mod-settings",
        "ModSettings",
        "F9",
        "In-game settings for the other mods",
    ),
    m(
        "performance-client",
        "PerformanceClient",
        "F10",
        "FPS and network tweaks",
    ),
    m(
        "popup-blocker",
        "PopupBlocker",
        "",
        "Hides anti-cheat popups in the modded copy",
    ),
    m(
        "destruction",
        "Destruction",
        "",
        "Blow buildings apart with grenades and rockets",
    ),
    m(
        "visuals",
        "Visuals",
        "F8",
        "Better shadows, clouds, sea, smoke and explosions",
    ),
    m(
        "princess-mode",
        "PrincessMode",
        "F3",
        "Capture the Princess: cages, turrets, HUD and loadouts",
    ),
];

fn is_dll(entry: &str) -> bool {
    entry.to_ascii_lowercase().ends_with(".dll")
}

fn entry_id(entry: &str) -> &str {
    entry.split('@').next().unwrap_or(entry)
}

/// The mod that pulls `id` in, if another one needs it. Without a catalog (offline) the one
/// dependency that exists today still shows.
fn needed_by(id: &str, on: &[String], catalog: Option<&[CatalogEntry]>) -> Option<&'static str> {
    let wanted_by = |other: &str| match catalog {
        Some(catalog) => {
            let guid = catalog.iter().find(|c| c.id == id).map(|c| c.guid.as_str());
            catalog
                .iter()
                .find(|c| c.id == other)
                .is_some_and(|c| guid.is_some_and(|g| c.depends.iter().any(|d| d == g)))
        }
        None => id == "mod-settings" && other == "coords-display",
    };
    MODS.iter()
        .find(|m| m.id != id && on.iter().any(|e| e == m.id) && wanted_by(m.id))
        .map(|m| m.name)
}

fn mods_on(client: &[String]) -> Vec<String> {
    client
        .iter()
        .filter(|e| !is_dll(e))
        .map(|e| entry_id(e).to_owned())
        .collect()
}

/// How many mods friends get, dependencies included.
fn effective_count(on: &[String], catalog: Option<&[CatalogEntry]>) -> usize {
    MODS.iter()
        .filter(|m| on.iter().any(|e| e == m.id) || needed_by(m.id, on, catalog).is_some())
        .count()
}

fn mods_card(gui: &mut Gui, ui: &mut Ui) {
    let on = mods_on(&gui.draft.client_mods());
    let catalog = gui.catalog.clone();
    let count = format!("{} on", effective_count(&on, catalog.as_deref()));
    card_fill(
        ui,
        "Client mods",
        |ui| {
            ui.label(muted(count));
        },
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let (x_name, x_key, x_what) = (62.0, 255.0, 318.0);
            row(ui, 38.0, None, |ui, rect| {
                cell(ui, rect, x_name, 120.0, muted("Mod"));
                cell(ui, rect, x_key, 60.0, muted("Key"));
                cell(ui, rect, x_what, 200.0, muted("What it does"));
            });
            hline(ui, LINE_SOFT);
            egui::ScrollArea::vertical()
                .id_salt("mods-list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for m in &MODS {
                        mod_row(gui, ui, m, &on, catalog.as_deref(), (x_name, x_key, x_what));
                    }
                });
        },
    );
}

fn mod_row(
    gui: &mut Gui,
    ui: &mut Ui,
    m: &ModRow,
    on: &[String],
    catalog: Option<&[CatalogEntry]>,
    (x_name, x_key, x_what): (f32, f32, f32),
) {
    let explicit = mods_on(&gui.draft.mods.client).iter().any(|e| e == m.id);
    // Destruction follows the Rules switch; it is only ticked here by hand in older configs.
    let rule = m.id == DESTRUCTION_MOD && !explicit;
    let auto = rule && gui.draft.game.destruction;
    let needer = needed_by(m.id, on, catalog);
    let published = catalog.is_none_or(|c| c.iter().any(|e| e.id == m.id));
    let note = if !published {
        Some("not published yet".to_owned())
    } else if auto {
        Some("added, destructible buildings are on (Rules)".to_owned())
    } else if rule {
        Some("turn on destructible buildings in Rules".to_owned())
    } else if !explicit {
        needer.map(|n| format!("added, {n} needs it"))
    } else {
        None
    };
    let height = if note.is_some() { 48.0 } else { 34.0 };
    row(ui, height, None, |ui, rect| {
        let mut value = explicit || auto || needer.is_some();
        let enabled = published && !gui.applying && (explicit || (needer.is_none() && !rule));
        let area = Rect::from_center_size(
            pos2(rect.left() + 31.0, rect.top() + 17.0),
            vec2(20.0, 20.0),
        );
        let changed = within(ui, area, |ui| {
            ui.add_enabled_ui(enabled, |ui| check(ui, m.name, &mut value).changed())
                .inner
        });
        if changed {
            if value {
                gui.draft.mods.client.push(m.id.to_owned());
            } else {
                gui.draft
                    .mods
                    .client
                    .retain(|e| is_dll(e) || entry_id(e) != m.id);
            }
            gui.save_section(Section::Mods);
        }
        let top = Rect::from_min_size(rect.min, vec2(rect.width(), 34.0));
        cell(ui, top, x_name, x_key - x_name - 8.0, text(m.name));
        cell(ui, top, x_key, 50.0, monospace(m.key).color(MUTED));
        cell(
            ui,
            top,
            x_what,
            rect.width() - x_what - 8.0,
            text(m.what).color(MUTED),
        );
        if let Some(note) = note {
            let below = Rect::from_min_size(
                pos2(rect.left(), rect.top() + 28.0),
                vec2(rect.width(), 18.0),
            );
            cell(ui, below, x_what, rect.width() - x_what - 8.0, muted(note));
        }
    });
    hline(ui, LINE_SOFT);
}

fn dlls_card(gui: &mut Gui, ui: &mut Ui) {
    let dlls: Vec<String> = gui
        .draft
        .mods
        .client
        .iter()
        .filter(|e| is_dll(e))
        .cloned()
        .collect();
    card(
        ui,
        "Your own DLLs",
        |_| {},
        |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add_space(6.0);
            for dll in &dlls {
                row(ui, 36.0, None, |ui, rect| {
                    cell(ui, rect, 16.0, rect.width() - 110.0, monospace(dll));
                    cell_actions(ui, rect, 16.0, |ui| {
                        if plain(ui, "Remove") {
                            gui.draft.mods.client.retain(|e| e != dll);
                            gui.save_section(Section::Mods);
                        }
                    });
                });
            }
            padded(ui, |ui| {
                ui.horizontal(|ui| {
                    if plain(ui, "Add a DLL…") {
                        gui.pick(Pick::Dll);
                    }
                    if dlls.is_empty() {
                        ui.label(muted("Sent to friends from this PC"));
                    }
                });
            });
        },
    );
}

impl Gui {
    pub fn on_share(&mut self, result: Result<Value, String>, mods: Vec<String>) {
        let share = &mut self.t.share;
        share.busy = false;
        match result {
            Ok(v) => {
                share.code = v["code"].as_str().map(str::to_owned);
                share.join = v["join"].as_str().map(str::to_owned);
                share.mods = mods;
                share.error = None;
            }
            Err(e) => share.error = Some(crate::app::capitalized(&e)),
        }
    }

    fn make_share(&mut self) {
        self.t.share.busy = true;
        self.t.share.error = None;
        let mods = self.saved.client_mods();
        self.bg(move |h| {
            let result = h.runtime.block_on(tabg_web::api::make_share(&h.app));
            Msg::Share(result, mods)
        });
    }
}

fn share_card(gui: &mut Gui, ui: &mut Ui) {
    card(
        ui,
        "Share code",
        |_| {},
        |ui| {
            padded(ui, |ui| {
                let friends = if gui.saved.game.password.is_empty() {
                    "Friends without mods only need the join code."
                } else {
                    "Friends without mods only need the join code and password."
                };
                label(
                    ui,
                    text(format!("{friends} With mods on, they run this once.")).color(MUTED),
                );
                let code = gui.t.share.code.clone();
                let mut shown = code.clone().unwrap_or_default();
                ui.add(
                    egui::TextEdit::multiline(&mut shown)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY)
                        .interactive(code.is_some())
                        .hint_text("No code yet")
                        .font(small()),
                );
                let busy = gui.t.share.busy;
                ui.horizontal(|ui| {
                    match &code {
                        Some(code) => {
                            if primary(ui, "Copy share code") {
                                ui.ctx().copy_text(code.clone());
                                gui.tell("Share code copied.");
                            }
                            ui.add_enabled_ui(!busy, |ui| {
                                if plain(ui, "Remake") {
                                    gui.make_share();
                                }
                            });
                        }
                        None => {
                            ui.add_enabled_ui(!busy, |ui| {
                                if primary(ui, if busy { "Making…" } else { "Make share code" }) {
                                    gui.make_share();
                                }
                            });
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if code.is_some() {
                            let current_join = gui.status.join_code.as_deref();
                            let share = &gui.t.share;
                            let stale_mods = share.mods != gui.saved.client_mods();
                            let stale_join =
                                current_join.is_some() && share.join.as_deref() != current_join;
                            let on = effective_count(
                                &mods_on(&gui.saved.client_mods()),
                                gui.catalog.as_deref(),
                            );
                            let note = if stale_mods {
                                text("Mods changed, remake it").color(DANGER)
                            } else if stale_join {
                                text("Join code changed, remake it").color(DANGER)
                            } else {
                                muted(format!("Matches the {on} mods on"))
                            };
                            ui.label(note);
                        }
                    });
                });
                if let Some(e) = gui.t.share.error.clone() {
                    label(ui, text(e).color(DANGER));
                }
            });
        },
    );
}

/// The steps as plain text, for copying into a chat.
pub fn friends_message(
    mods: bool,
    join: Option<&str>,
    direct: bool,
    password: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    if mods {
        lines.push("My server uses a few mods. Once, before you join:".to_owned());
        lines.push("1. Download tabg from its releases page".to_owned());
        lines.push("2. Run tabg join and paste the share code".to_owned());
        lines.push("3. Run tabg launch to start the modded game".to_owned());
    } else {
        lines.push("Come play on my TABG server:".to_owned());
    }
    match join {
        Some(code) if !direct => lines.push(format!("The join code is {code}")),
        Some(address) => lines.push(format!("Connect to {address}")),
        None => {}
    }
    if !password.is_empty() {
        lines.push(format!("The password is {password}"));
    }
    lines
}

fn message_card(gui: &mut Gui, ui: &mut Ui) {
    let direct = gui.saved.network.mode == NetMode::Direct;
    let join = gui
        .status
        .join_code
        .clone()
        .or_else(|| gui.t.share.join.clone());
    let lines = friends_message(
        !gui.saved.client_mods().is_empty(),
        join.as_deref(),
        direct,
        &gui.saved.game.password,
    );
    card(
        ui,
        "Message for friends",
        |_| {},
        |ui| {
            padded(ui, |ui| {
                Frame::new()
                    .fill(HEADER)
                    .stroke(Stroke::new(1.0, LINE_SOFT))
                    .corner_radius(CornerRadius::same(FIELD_RADIUS))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for l in &lines {
                            label(ui, text(l));
                        }
                    });
                ui.add_space(4.0);
                if plain(ui, "Copy message") {
                    ui.ctx().copy_text(lines.join("\n"));
                    gui.tell("Message copied.");
                }
            });
        },
    );
}

pub fn show(gui: &mut Gui, ui: &mut Ui) {
    let area = ui.available_rect_before_wrap();
    let left_w = (area.width() * 0.56).floor();
    let left = Rect::from_min_size(area.min, vec2(left_w, area.height()));
    let right = Rect::from_min_max(pos2(left.right() + 10.0, area.top()), area.max);
    within(ui, left, |ui| {
        let dll_h = 40.0
            + 16.0
            + 60.0
            + gui.draft.mods.client.iter().filter(|e| is_dll(e)).count() as f32 * 36.0;
        let list = Rect::from_min_max(left.min, pos2(left.right(), left.bottom() - dll_h - 10.0));
        let bottom = Rect::from_min_max(pos2(left.left(), list.bottom() + 10.0), left.max);
        within(ui, list, |ui| mods_card(gui, ui));
        within(ui, bottom, |ui| dlls_card(gui, ui));
    });
    within(ui, right, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("mods-right")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.set_width(ui.available_width());
                share_card(gui, ui);
                message_card(gui, ui);
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, guid: &str, depends: &[&str]) -> CatalogEntry {
        CatalogEntry {
            id: id.into(),
            name: id.into(),
            guid: guid.into(),
            depends: depends.iter().map(|d| (*d).to_owned()).collect(),
            version: "1.0.0".into(),
            file: format!("{id}.dll"),
            sha256: String::new(),
            size: 0,
        }
    }

    #[test]
    fn a_dependency_shows_as_added() {
        let catalog = vec![
            entry("coords-display", "x.coords", &["x.settings"]),
            entry("mod-settings", "x.settings", &[]),
        ];
        let on = vec!["coords-display".to_owned()];
        assert_eq!(
            needed_by("mod-settings", &on, Some(&catalog)),
            Some("CoordsDisplay")
        );
        assert_eq!(needed_by("mod-settings", &[], Some(&catalog)), None);
        assert_eq!(needed_by("coords-display", &on, Some(&catalog)), None);
        assert_eq!(effective_count(&on, Some(&catalog)), 2);
        // Offline, the known dependency still shows.
        assert_eq!(needed_by("mod-settings", &on, None), Some("CoordsDisplay"));
    }

    #[test]
    fn versions_and_dlls_are_not_mod_ids() {
        let client = vec![
            "range-map@1.1.1".to_owned(),
            "D:\\mods\\Mine.DLL".to_owned(),
        ];
        assert_eq!(mods_on(&client), ["range-map"]);
    }

    #[test]
    fn the_message_says_only_what_friends_need() {
        let plain = friends_message(false, Some("AB12CD"), false, "");
        assert_eq!(
            plain,
            ["Come play on my TABG server:", "The join code is AB12CD"]
        );
        let modded = friends_message(true, Some("1.2.3.4:7777"), true, "pizza");
        assert_eq!(modded.len(), 6);
        assert!(modded[0].starts_with("My server uses"));
        assert_eq!(modded[4], "Connect to 1.2.3.4:7777");
        assert_eq!(modded[5], "The password is pizza");
    }

    #[test]
    fn the_mod_list_matches_the_ids_the_release_script_makes() {
        // scripts/release-mods.sh derives ids from the project names; the window lists them by
        // hand for the descriptions.
        for (name, id) in [
            ("AdminRadar.Client", "admin-radar"),
            ("CoordsDisplay", "coords-display"),
            ("CustomGameSkins.Client", "custom-game-skins"),
            ("CustomGrenades", "custom-grenades"),
            ("Destruction", "destruction"),
            ("DevTestMap.Client", "dev-test-map"),
            ("FlyingControls", "flying-controls"),
            ("ModSettings", "mod-settings"),
            ("PerformanceClient", "performance-client"),
            ("PopupBlocker", "popup-blocker"),
            ("PrincessMode", "princess-mode"),
            ("ProximityChat.Client", "proximity-chat"),
            ("RangeMap.Client", "range-map"),
            ("Visuals", "visuals"),
        ] {
            assert!(MODS.iter().any(|m| m.id == id), "{name}: {id}");
        }
        assert_eq!(MODS.len(), 14);
    }
}
