//! The window driven headlessly: real host state in a temp dir, a harness instead of a screen.

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use tabg_game::PlayerInfo;
use tabg_host::setup::Install;
use tabg_host::supervisor::{HostStatus, RunState};
use tabg_host::{Paths, ServerToml, Supervisor};

use crate::app::{BarState, Dialog, Gui, Screen, bar_state};
use crate::tabs::Tab;
use crate::{Host, theme};

struct Fixture {
    _dir: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    paths: Paths,
}

fn fixture(ready: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::new(dir.path().to_owned());
    std::fs::create_dir_all(paths.server()).unwrap();
    if ready {
        let world = dir.path().join("world.json");
        std::fs::write(&world, "{}").unwrap();
        Install {
            world: Some(world),
            ..Default::default()
        }
        .save(&paths)
        .unwrap();
    }
    Fixture {
        runtime: tokio::runtime::Runtime::new().unwrap(),
        paths,
        _dir: dir,
    }
}

fn player(index: u8, name: &str, team: u8, health: f32, kills: u16) -> PlayerInfo {
    PlayerInfo {
        index,
        name: name.into(),
        id: if name.starts_with("Bot") {
            format!("bot:{index}")
        } else {
            format!("epic:{index:032x}")
        },
        team,
        admin: index == 1,
        alive: health > 0.0,
        health,
        kills,
        position: [0.0; 3],
    }
}

fn demo_status() -> HostStatus {
    HostStatus {
        state: RunState::Running,
        join_code: Some("K7F3QX".into()),
        direct: None,
        phase: "match".into(),
        players: vec![
            player(1, "jonas_n", 0, 100.0, 4),
            player(2, "Mika", 0, 62.0, 2),
            player(3, "Pfannkuchen", 0, 0.0, 1),
            player(4, "Lenny", 1, 88.0, 3),
            player(5, "Bot 253", 1, 18.0, 0),
            player(6, "Kaito", 1, 0.0, 0),
            player(7, "chickenwing", 2, 100.0, 5),
            player(8, "Bot 261", 2, 100.0, 1),
            player(9, "Ronja", 3, 74.0, 2),
            player(10, "Bot 270", 3, 40.0, 0),
            player(11, "Tobi", 3, 55.0, 1),
        ],
        started_at: Some(tabg_host::paths::now() - 72 * 60),
        restarts: 0,
        retry_at: None,
        last_crash: None,
        error: None,
    }
}

fn new_gui(f: &Fixture, ctx: egui::Context) -> Gui {
    let supervisor = Supervisor::new(
        f.paths.clone(),
        Arc::new(|| std::process::Command::new("false")),
    );
    let app = Arc::new(tabg_web::App {
        paths: f.paths.clone(),
        supervisor,
        token: "test".into(),
        port: 0,
        setup_busy: Mutex::new(false),
        share: Mutex::new(None),
        sync_port: Mutex::new(None),
    });
    let mut gui = Gui::offline(
        Host {
            app,
            runtime: f.runtime.handle().clone(),
        },
        ctx,
    );
    let mut items = std::collections::BTreeMap::new();
    for (id, name) in [(52, "Bandage"), (211, "Bandana"), (160, "M16"), (6, "Ammo")] {
        items.insert(id, name.to_owned());
    }
    gui.map.items = items;
    gui
}

fn harness(gui: Gui) -> Harness<'static, Gui> {
    Harness::builder()
        .with_size(egui::vec2(1370.0, 896.0))
        .build_ui_state(frame(), gui)
}

/// The window's frame; fonts only take effect from the pass after they're installed.
fn frame() -> impl FnMut(&mut egui::Ui, &mut Gui) {
    let mut first = true;
    move |ui, gui| {
        if first {
            first = false;
            theme::install(ui.ctx());
            ui.ctx().request_repaint();
        } else {
            gui.draw(ui);
        }
    }
}

fn live(f: &Fixture) -> Harness<'static, Gui> {
    let ctx = egui::Context::default();
    let mut gui = new_gui(f, ctx);
    gui.frozen_status = true;
    gui.status = demo_status();
    harness(gui)
}

fn sync_ctx(h: &mut Harness<'static, Gui>) {
    // Background work reports to the context the frame runs on.
    let ctx = h.ctx.clone();
    h.state_mut().ctx = ctx;
}

/// Let a background job finish and its message arrive.
fn settle(h: &mut Harness<'static, Gui>, done: impl Fn(&Gui) -> bool) {
    for _ in 0..500 {
        h.step();
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("background work never finished");
}

fn open(h: &mut Harness<'static, Gui>, tab: Tab) {
    h.get_by_label(tab.title()).click();
    h.run_steps(3);
    assert_eq!(h.state().tab, tab);
}

fn on_disk(f: &Fixture) -> ServerToml {
    ServerToml::parse(&std::fs::read_to_string(f.paths.config()).unwrap()).unwrap()
}

#[test]
fn first_run_starts_with_the_wizard() {
    let f = fixture(false);
    let mut h = harness(new_gui(&f, egui::Context::default()));
    h.run_steps(2);
    assert!(matches!(h.state().screen, Screen::Wizard(_)));
    h.get_by_label("Host a server");
    h.get_by_label("Join a friend").click();
    h.run_steps(2);
    h.get_by_label("Continue").click();
    h.run_steps(3);
    h.get_by_label("Back");
}

#[test]
fn the_top_bar_follows_the_server() {
    let mut s = demo_status();
    assert_eq!(bar_state(&s, true), BarState::Running);
    s.phase = "lobby".into();
    assert_eq!(bar_state(&s, true), BarState::Lobby);
    s.state = RunState::Restarting;
    assert_eq!(bar_state(&s, true), BarState::Crashed);
    s.state = RunState::Stopped;
    assert_eq!(bar_state(&s, true), BarState::Stopped);
    assert_eq!(bar_state(&s, false), BarState::SetupNeeded);
    s.error = Some("no map data".into());
    assert_eq!(bar_state(&s, true), BarState::Refused("no map data".into()));
}

#[test]
fn every_tab_draws() {
    let f = fixture(true);
    let mut h = live(&f);
    h.run_steps(2);
    for tab in Tab::ALL {
        open(&mut h, tab);
    }
}

#[test]
fn stop_asks_first_when_people_are_online() {
    let f = fixture(true);
    let mut h = live(&f);
    h.run_steps(2);
    h.get_by_label("Stop").click();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(Dialog::Stop)));
    h.get_by_label("Keep running").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
}

#[test]
fn bots_are_removed_and_people_are_kicked() {
    let f = fixture(true);
    let mut h = live(&f);
    h.run_steps(2);
    assert_eq!(h.query_all_by_label("Remove").count(), 3, "one per bot");
    assert_eq!(h.query_all_by_label("Kick").count(), 8);
    assert_eq!(h.query_all_by_label("Ban").count(), 8);
    h.get_by_label("Mika");
}

#[test]
fn rules_save_only_when_asked_and_keep_the_rest() {
    let f = fixture(true);
    let mut h = live(&f);
    sync_ctx(&mut h);
    h.run_steps(2);
    open(&mut h, Tab::Rules);
    h.get_by_label("Respawn after dying").click();
    h.run_steps(3);
    assert!(h.state().draft.rules.respawn);
    assert!(!on_disk(&f).rules.respawn, "nothing written yet");
    h.get_by_label("Discard").click();
    h.run_steps(3);
    assert!(!h.state().draft.rules.respawn);

    h.get_by_label("Respawn after dying").click();
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.step();
    settle(&mut h, |g| !g.applying && g.saved.rules.respawn);
    let saved = on_disk(&f);
    assert!(saved.rules.respawn);
    assert_eq!(saved.name, ServerToml::default().name);
    assert_eq!(h.state().backups[0].reason, "before a settings change");
}

#[test]
fn a_change_made_elsewhere_reaches_the_open_form() {
    let f = fixture(true);
    let mut h = live(&f);
    sync_ctx(&mut h);
    h.run_steps(2);
    open(&mut h, Tab::Rules);
    // The host is editing the rules while the CLI changes the name.
    h.state_mut().draft.rules.respawn = true;
    let mut outside = on_disk(&f);
    outside.name = "friday fight".into();
    // Filesystem timestamps can be coarse: make sure this write looks new.
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(f.paths.config(), outside.to_toml()).unwrap();
    h.state_mut().reload_config();
    assert_eq!(h.state().draft.name, "friday fight");
    assert!(h.state().draft.rules.respawn, "the unsaved edit survives");
}

#[test]
fn a_preset_shows_what_it_changes_before_applying() {
    let f = fixture(true);
    let mut h = live(&f);
    sync_ctx(&mut h);
    h.run_steps(2);
    open(&mut h, Tab::GameMode);
    h.get_by_label("Gun Game").click();
    h.run_steps(3);
    h.get_by_label("What changes");
    h.get_by_label("Respawn after dying");
    assert!(!on_disk(&f).rules.respawn);
    h.get_by_label("Use Gun Game").click();
    h.step();
    settle(&mut h, |g| g.saved.preset == "gun-game");
    assert_eq!(on_disk(&f).rules.gun_game.len(), 10);
    assert_eq!(h.state().backups[0].reason, "before the Gun Game preset");
}

#[test]
fn saving_a_restart_field_warns_about_the_players() {
    let f = fixture(true);
    let mut h = live(&f);
    sync_ctx(&mut h);
    h.run_steps(2);
    open(&mut h, Tab::Server);
    h.state_mut().draft.game.max_players = 40;
    h.run_steps(3);
    h.get_by_label("Save and restart").click();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(Dialog::SaveRestart { .. })));
    assert_eq!(on_disk(&f).game.max_players, 70, "nothing saved yet");
}

#[test]
fn a_bad_ring_can_not_be_saved() {
    let f = fixture(true);
    let mut h = live(&f);
    h.run_steps(2);
    open(&mut h, Tab::Loadout);
    h.state_mut().draft.ring.sizes = vec![100.0, 200.0];
    h.run_steps(3);
    assert!(h.get_by_label("Save ring").accesskit_node().is_disabled());
    h.get_by_label("Each ring must be smaller than the one before");
}

#[test]
fn a_backup_comes_back() {
    let f = fixture(true);
    let mut h = live(&f);
    sync_ctx(&mut h);
    h.run_steps(2);
    let original = on_disk(&f);
    let mut changed = original.clone();
    changed.name = "friday fight".into();
    h.state_mut().apply(changed, "a settings change");
    settle(&mut h, |g| !g.applying);
    assert_eq!(on_disk(&f).name, "friday fight");

    open(&mut h, Tab::Server);
    h.get_by_label("Restore").click();
    h.run_steps(3);
    let buttons: Vec<_> = h.query_all_by_label("Restore").collect();
    buttons.last().unwrap().click();
    h.step();
    settle(&mut h, |g| !g.applying && g.saved.name == original.name);
    assert_eq!(on_disk(&f).name, original.name);
    // The state it replaced is one more backup, not two.
    assert_eq!(h.state().backups.len(), 2);
}

#[test]
fn a_refused_mod_does_not_stay_ticked() {
    let f = fixture(true);
    let mut config = on_disk_or_default(&f);
    config.game.mode = tabg_host::config::GameMode::Test;
    config.game.map = tabg_host::config::Map::Range;
    config.mods.client = vec!["range-map".into()];
    std::fs::write(f.paths.config(), config.to_toml()).unwrap();
    let mut h = live(&f);
    sync_ctx(&mut h);
    h.run_steps(2);
    open(&mut h, Tab::Mods);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "RangeMap")
        .click();
    h.step();
    settle(&mut h, |g| !g.applying && g.notice.is_some());
    assert_eq!(h.state().draft.mods.client, ["range-map"]);
    assert_eq!(on_disk(&f).mods.client, ["range-map"]);
}

fn on_disk_or_default(f: &Fixture) -> ServerToml {
    std::fs::read_to_string(f.paths.config())
        .ok()
        .and_then(|t| ServerToml::parse(&t).ok())
        .unwrap_or_default()
}

/// Renders every screen to `target/gui-shots` (needs a GPU or software Vulkan, so not in `check.sh`).
#[test]
#[ignore = "writes screenshots; run with --ignored"]
fn screenshots() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/gui-shots");
    std::fs::create_dir_all(&out).unwrap();
    let f = fixture(true);
    let ctx = egui::Context::default();
    let mut gui = new_gui(&f, ctx);
    gui.frozen_status = true;
    gui.status = demo_status();
    gui.saved.name = "Friday Night Drops".into();
    gui.saved.game.password = "pizzanight".into();
    gui.saved.preset = "quick-match".into();
    gui.saved.players.admins = vec!["jonas_n".into(), "Lenny".into()];
    gui.saved.players.banned = vec!["griefer99".into()];
    gui.saved.mods.client = vec![
        "coords-display".into(),
        "proximity-chat".into(),
        "D:\\mods\\BiggerFootsteps.dll".into(),
    ];
    gui.saved.loadout = vec![
        tabg_host::config::LoadoutItem { item: 160, qty: 1 },
        tabg_host::config::LoadoutItem { item: 6, qty: 240 },
        tabg_host::config::LoadoutItem { item: 52, qty: 3 },
    ];
    gui.saved.rules.respawn = true;
    gui.saved.game.destruction = true;
    gui.saved.rules.gun_game = (0..10)
        .map(|_| vec![tabg_host::config::LoadoutItem { item: 160, qty: 1 }])
        .collect();
    gui.draft = gui.saved.clone();
    gui.password = "pizzanight".into();
    gui.t.selected = gui.status.players.get(1).map(|p| p.id.clone());
    gui.t.give_query = "Bandage".into();
    gui.t.give_item = Some(52);
    gui.t.give_qty = 3;
    gui.t.preset = "gun-game".into();
    gui.t.ring_stage = 2;
    for (i, (source, text)) in [
        ("game", "chickenwing joined (epic:abc)"),
        ("game", "Match started"),
        ("game", "Lenny killed Bot 249"),
        ("host", "You removed Bot 249"),
        ("chat", "Ronja: who has spare bandages"),
        ("game", "Mika killed Pfannkuchen"),
        ("game", "Kaito died in the ring"),
        ("host", "You kicked griefer2"),
        (
            "game",
            "inventory check: Tobi wanted to drop 1 × item 52 but carries 0",
        ),
        ("console", "> players"),
        ("console", "14 players: #1 jonas_n, #2 Mika"),
        ("server", "INFO tabg_server: tick"),
    ]
    .iter()
    .enumerate()
    {
        gui.log.push_back(tabg_host::supervisor::LogLine {
            seq: i as u64 + 1,
            at: tabg_host::paths::now() - 600 + i as u64 * 40,
            source: (*source).into(),
            text: (*text).into(),
        });
    }
    let shot = |h: &mut Harness<'static, Gui>, name: &str| {
        h.run_steps(4);
        let image = h.render().unwrap();
        image.save(out.join(format!("{name}.png"))).unwrap();
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(1370.0, 896.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_ui_state(frame(), gui);
    sync_ctx(&mut h);
    // The (empty) test world finishes loading first, then real item names go in.
    h.run_steps(2);
    std::thread::sleep(std::time::Duration::from_millis(300));
    h.run_steps(2);
    let items = [
        (52, "Bandage"),
        (211, "Bandana"),
        (160, "M16"),
        (6, "Ammo"),
        (273, "Hand Gun"),
        (254, "Katana"),
    ];
    h.state_mut().map.items = items.iter().map(|(i, n)| (*i, (*n).to_owned())).collect();
    for tab in Tab::ALL {
        h.state_mut().tab = tab;
        if tab == Tab::Rules {
            h.state_mut().draft.game.loot_amount = 0.25;
            h.state_mut().draft.game.skins = true;
        }
        if tab == Tab::Server {
            h.state_mut().draft = h.state().saved.clone();
            h.state_mut().draft.name = "Friday Night Pizza Drops".into();
            h.state_mut().draft.network.public = true;
        }
        shot(&mut h, tab.title());
    }
    h.state_mut().draft = h.state().saved.clone();
    h.state_mut().tab = Tab::Overview;
    h.state_mut().dialog = Some(Dialog::Stop);
    shot(&mut h, "dialog");
    h.state_mut().dialog = None;
    h.state_mut().screen = Screen::Wizard(crate::wizard::Wizard::welcome());
    shot(&mut h, "wizard-welcome");
    h.state_mut().screen = Screen::Wizard(crate::wizard::Wizard::Missing {
        client_found: false,
    });
    shot(&mut h, "wizard-missing");
}

/// The window against a real `tabg serve`: start it, see the bots arrive, remove one, stop it.
/// Needs `TABG_TEST_BIN` (a built `tabg`), `TABG_TEST_PROBE` (a built `tabg-probe`) and
/// `TABG_TEST_WORLD` (dumped map data with its `.nav`).
#[test]
#[ignore = "needs a tabg binary and map data"]
fn drives_a_real_server() {
    let bin = std::env::var("TABG_TEST_BIN").expect("TABG_TEST_BIN");
    let probe = std::env::var("TABG_TEST_PROBE").expect("TABG_TEST_PROBE");
    let world = std::env::var("TABG_TEST_WORLD").expect("TABG_TEST_WORLD");
    let f = fixture(false);
    Install {
        world: Some(world.into()),
        ..Default::default()
    }
    .save(&f.paths)
    .unwrap();
    let mut config = ServerToml::default();
    config.network.mode = tabg_host::config::NetMode::Direct;
    config.network.port = 47811;
    config.network.upnp = false;
    config.game.bots = 4;
    config.game.players_to_start = 9;
    config.game.countdown = 1.0;
    std::fs::write(f.paths.config(), config.to_toml()).unwrap();

    let root = f.paths.root.clone();
    let supervisor = Supervisor::new(
        f.paths.clone(),
        Arc::new(move || {
            let mut c = std::process::Command::new(&bin);
            c.arg("serve").env("TABG_HOME", &root);
            c
        }),
    );
    let app = Arc::new(tabg_web::App {
        paths: f.paths.clone(),
        supervisor: supervisor.clone(),
        token: "test".into(),
        port: 0,
        setup_busy: Mutex::new(false),
        share: Mutex::new(None),
        sync_port: Mutex::new(None),
    });
    let gui = Gui::offline(
        Host {
            app,
            runtime: f.runtime.handle().clone(),
        },
        egui::Context::default(),
    );
    let mut h = harness(gui);
    sync_ctx(&mut h);
    h.run_steps(2);
    h.get_by_label("Start server").click();
    h.step();

    let wait = |h: &mut Harness<'static, Gui>, what: &str, done: &dyn Fn(&Gui) -> bool| {
        for _ in 0..3000 {
            h.step();
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        supervisor.stop();
        let log: Vec<String> = h.state().log.iter().map(|l| l.text.clone()).collect();
        panic!(
            "never saw {what}; status {:?}; log {log:#?}",
            h.state().status
        );
    };
    let bots = |g: &Gui| {
        g.status
            .players
            .iter()
            .filter(|p| crate::app::is_bot(p))
            .count()
    };

    wait(&mut h, "the server listening", &|g| {
        g.status.state == RunState::Running
    });
    assert!(h.state().status.direct.is_some());

    // Bots only come once a person is there.
    let mut person = std::process::Command::new(probe)
        .args(["--port", "47811", "--name", "Mika", "--seconds", "90"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait(&mut h, "the bots", &|g| bots(g) == 4);
    h.run_steps(3);
    assert_eq!(h.query_all_by_label("Remove").count(), 4);
    h.get_by_label("Mika");
    h.get_by_label("Kick");

    // In the lobby the server would refill a kicked bot, so Remove lowers the setting.
    h.query_all_by_label("Remove").next().unwrap().click();
    h.step();
    wait(&mut h, "one bot fewer", &|g| {
        bots(g) == 3 && g.saved.game.bots == 3
    });

    // A saved rule reaches the running game without a restart.
    open(&mut h, Tab::Rules);
    h.state_mut().draft.game.loot_amount = 0.5;
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.step();
    wait(&mut h, "the rule saved", &|g| {
        !g.applying && g.saved.game.loot_amount == 0.5
    });
    assert_eq!(h.state().status.restarts, 0);
    assert_eq!(on_disk(&f).game.loot_amount, 0.5);

    // The console and the top bar's button both reach the room.
    open(&mut h, Tab::Overview);
    h.get_by_label("Start match now").click();
    h.step();
    wait(&mut h, "the match", &|g| g.status.phase == "match");
    h.run_steps(3);
    h.query_all_by_label("Remove").next().unwrap().click();
    h.step();
    wait(&mut h, "the bot gone for good", &|g| {
        bots(g) == 2 && g.log.iter().any(|l| l.text.starts_with("You removed"))
    });

    // People are online, so Stop asks first.
    h.get_by_label("Stop").click();
    h.run_steps(3);
    h.get_by_label("Stop server").click();
    h.step();
    wait(&mut h, "the server stopped", &|g| {
        g.status.state == RunState::Stopped
    });
    let _ = person.kill();
    let _ = person.wait();
}

#[test]
fn capture_the_princess_shows_its_settings_and_a_start_button() {
    let f = fixture(true);
    let mut h = live(&f);
    sync_ctx(&mut h);
    h.state_mut().status.phase = "lobby".into();
    h.run_steps(2);
    // A new round starts from the lobby with the bar's button.
    h.get_by_label("Start match now");
    open(&mut h, Tab::GameMode);
    h.get_by_label("Capture the Princess").click();
    h.run_steps(3);
    h.get_by_label("What changes");
    h.get_by_label("Use Capture the Princess").click();
    h.run_steps(3);
    // A new mode restarts the server, which asks first with people online.
    h.get_by_label("Save and restart").click();
    h.step();
    settle(&mut h, |g| g.saved.preset == "capture-the-princess");
    let saved = on_disk(&f);
    assert_eq!(saved.game.mode, tabg_host::config::GameMode::Princess);
    let mods = saved.client_mods();
    assert!(
        mods.iter().any(|m| m == "flying-controls") && mods.iter().any(|m| m == "mod-settings")
    );
    open(&mut h, Tab::Rules);
    h.get_by_label("Base turrets");
}
