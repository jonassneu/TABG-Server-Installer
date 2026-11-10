//! Dashboard pages: Overview · Console · Players · Settings · Loadout & Ring · Presets · Backups.
//! Forms post to `/ui/<action>` with htmx and get a small HTML answer back.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Form;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use tabg_host::apply::{self, Applied, ApplyError};
use tabg_host::config::{BotRadar, BotSkill, GameMode, InventoryCheck, Map, NetMode, Teams};
use tabg_host::supervisor::{HostStatus, LogLine, RunState};
use tabg_host::{ServerToml, backup, presets};

use crate::AppState;
use crate::api::{Grant, grant};

const CSS: &str = r#"
:root { --bg:#f6f7f9; --panel:#fff; --text:#1c2230; --muted:#667085; --line:#e3e6ec;
  --accent:#e8590c; --accent-text:#fff; --ok:#2b8a3e; --warn:#e67700; --bad:#c92a2a; --code:#f1f3f5; }
@media (prefers-color-scheme: dark) { :root { --bg:#14161b; --panel:#1d2027; --text:#e6e8ee;
  --muted:#98a2b3; --line:#2c313c; --accent:#ff7a2f; --accent-text:#14161b; --code:#262a33; } }
* { box-sizing:border-box }
body { margin:0; background:var(--bg); color:var(--text); font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif }
header { background:var(--panel); border-bottom:1px solid var(--line); padding:10px 20px; display:flex; gap:20px; align-items:center; flex-wrap:wrap }
header .brand { font-weight:700; font-size:18px } header .brand span { color:var(--accent) }
nav a { color:var(--muted); text-decoration:none; margin-right:14px; padding:4px 0 }
nav a.on { color:var(--text); border-bottom:2px solid var(--accent) }
main { max-width:1100px; margin:0 auto; padding:20px 16px 60px }
h1 { font-size:22px; margin:0 0 14px } h2 { font-size:17px; margin:22px 0 10px }
.card { background:var(--panel); border:1px solid var(--line); border-radius:10px; padding:16px 18px; margin-bottom:16px }
.row { display:flex; gap:16px; flex-wrap:wrap; align-items:center }
.grid { display:grid; grid-template-columns:repeat(auto-fit,minmax(240px,1fr)); gap:12px 18px }
label { display:block; font-size:13px; color:var(--muted); margin-bottom:3px }
input, select, textarea { width:100%; padding:7px 9px; border:1px solid var(--line); border-radius:7px; background:var(--bg); color:var(--text); font:inherit }
input[type=checkbox] { width:auto }
button, .btn { background:var(--accent); color:var(--accent-text); border:0; border-radius:7px; padding:7px 14px; font:inherit; font-weight:600; cursor:pointer }
button.plain { background:transparent; color:var(--text); border:1px solid var(--line) }
button.danger { background:var(--bad); color:#fff }
table { width:100%; border-collapse:collapse } th, td { text-align:left; padding:6px 8px; border-bottom:1px solid var(--line); font-size:14px }
th { color:var(--muted); font-weight:600 }
.pill { display:inline-block; padding:2px 10px; border-radius:99px; font-size:13px; font-weight:600; border:1px solid var(--line) }
.pill.running { color:var(--ok) } .pill.starting, .pill.restarting { color:var(--warn) } .pill.stopped { color:var(--muted) }
.code { font:600 34px/1.1 ui-monospace,monospace; letter-spacing:3px; background:var(--code); padding:8px 14px; border-radius:8px; display:inline-block }
.muted { color:var(--muted) } .bad { color:var(--bad) } .ok { color:var(--ok) }
.log { background:var(--code); border-radius:8px; padding:10px; height:420px; overflow-y:auto; font:13px/1.45 ui-monospace,monospace; white-space:pre-wrap; word-break:break-word }
.log .src { color:var(--muted) } .log .game { color:var(--ok) } .log .console { color:var(--accent) } .log .host { color:var(--warn) }
#toast:not(:empty) { margin:10px 0; padding:8px 12px; border-radius:8px; background:var(--code) }
.problems li { color:var(--bad) }
"#;

fn page(app: &AppState, active: &str, title: &str, body: Markup) -> Markup {
    let name = apply::read_config(&app.paths)
        .map(|c| c.name)
        .unwrap_or_default();
    let tabs = [
        ("/", "Overview"),
        ("/console", "Console"),
        ("/players", "Players"),
        ("/settings", "Settings"),
        ("/loadout", "Loadout & Ring"),
        ("/presets", "Presets"),
        ("/backups", "Backups"),
        ("/share", "Share"),
    ];
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " · tabg" }
                style { (PreEscaped(CSS)) }
                script src="/assets/htmx.js" {}
                script src="/assets/htmx-sse.js" {}
                script { (PreEscaped(PAGE_JS)) }
            }
            body hx-ext="sse" sse-connect="/events" {
                header {
                    div.brand { "tabg " span { "·" } " " (name) }
                    nav {
                        @for (href, label) in tabs {
                            a href=(href) class=[(href == active).then_some("on")] { (label) }
                        }
                    }
                }
                main {
                    div id="toast" {}
                    (body)
                }
            }
        }
    }
}

fn state_name(s: RunState) -> &'static str {
    match s {
        RunState::Stopped => "stopped",
        RunState::Starting => "starting",
        RunState::Running => "running",
        RunState::Restarting => "restarting",
    }
}

/// The live status card (re-rendered on every status update).
pub fn status_card(s: &HostStatus, app: &AppState) -> Markup {
    let install = tabg_host::setup::Install::load(&app.paths);
    let busy = *app.setup_busy.lock().unwrap_or_else(|e| e.into_inner());
    html! {
        div.row {
            span class={"pill " (state_name(s.state))} { (state_name(s.state)) }
            span.muted { "match: " (s.phase) " · " (s.players.len()) " online" }
            @if s.restarts > 0 { span.muted { " · " (s.restarts) " restarts" } }
        }
        @if let Some(code) = &s.join_code {
            div style="margin-top:14px" {
                label { "Join code — friends type it into TABG's “Join code” box" }
                span.code { (code) }
            }
        }
        @if let Some(d) = &s.direct {
            p { "Direct connect: your public IP, " (d) "." }
        }
        @if let Some(e) = &s.error { p.bad { (e) } }
        @if let Some(c) = &s.last_crash {
            p.muted { "Last crash: " (c.reason) " — log: " code { (c.log_file) } }
        }
        @if !install.ready() {
            div.card style="margin-top:14px" {
                p { b { "Setup needed. " } "tabg extracts the map data from your TABG Dedicated Server (free on Steam: Library → Tools) once." }
                @if busy { p.muted { "Setup is running — watch the console." } }
                @else { button hx-post="/ui/setup" hx-target="#toast" { "Run setup" } }
            }
        }
        div.row style="margin-top:14px" {
            @if matches!(s.state, RunState::Stopped) {
                button hx-post="/ui/start" hx-target="#toast" disabled[!install.ready()] { "Start server" }
            } @else {
                button.plain hx-post="/ui/restart" hx-target="#toast" { "Restart" }
                button.danger hx-post="/ui/stop" hx-target="#toast" { "Stop" }
            }
        }
    }
}

pub fn online_table(s: &HostStatus) -> Markup {
    html! {
        @if s.players.is_empty() {
            p.muted { "Nobody online." }
        } @else {
            table {
                tr { th { "#" } th { "Name" } th { "Team" } th { "HP" } th { "Kills" } th { "" } }
                @for p in &s.players {
                    tr {
                        td { (p.index) }
                        td { (p.name) @if p.admin { " " span.pill { "admin" } } br; span.muted { (p.id) } }
                        td { (p.team) }
                        td { @if p.alive { (format!("{:.0}", p.health)) } @else { "dead" } }
                        td { (p.kills) }
                        td.row {
                            button.plain hx-post="/ui/kick" hx-vals=(format!(r#"{{"who":"{}"}}"#, p.index)) hx-target="#toast" { "Kick" }
                            button.danger hx-post="/ui/ban" hx-vals=(serde_json::json!({"who": p.id, "on": "true"}).to_string()) hx-target="#toast" hx-confirm=(format!("Ban {}?", p.name)) { "Ban" }
                        }
                    }
                }
            }
        }
    }
}

fn time(t: u64) -> String {
    let secs = t % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

pub fn log_line(l: &LogLine) -> Markup {
    html! {
        div class=(l.source) { span.src data-t=(l.at) { (time(l.at)) } span.src { " " (l.source) " " } (l.text) }
    }
}

/// Show log times in the viewer's time zone and keep logs scrolled to the newest line.
const PAGE_JS: &str = r#"
function tabgTimes(root){root.querySelectorAll('[data-t]').forEach(e=>{if(e.dataset.done)return;e.dataset.done=1;
 e.textContent=new Date(e.dataset.t*1000).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit',second:'2-digit'});});}
function tabgScroll(){document.querySelectorAll('.log').forEach(l=>{if(l.scrollHeight-l.scrollTop-l.clientHeight<80||!l.dataset.seen){l.scrollTop=l.scrollHeight;l.dataset.seen=1;}});}
document.addEventListener('DOMContentLoaded',()=>{tabgTimes(document);tabgScroll();});
document.addEventListener('htmx:sseMessage',()=>{tabgTimes(document);tabgScroll();});
"#;

pub async fn overview(State(app): State<AppState>) -> Markup {
    let s = app.supervisor.status();
    let lines = app.supervisor.logs(30);
    page(
        &app,
        "/",
        "Overview",
        html! {
            h1 { "Overview" }
            div.card id="status" sse-swap="status" { (status_card(&s, &app)) }
            div.card {
                h2 style="margin-top:0" { "Online" }
                div id="online" sse-swap="players" { (online_table(&s)) }
            }
            div.card {
                h2 style="margin-top:0" { "Recent" }
                div.log style="height:220px" sse-swap="log" hx-swap="beforeend" {
                    @for l in &lines { (log_line(l)) }
                }
            }
        },
    )
}

pub async fn console(State(app): State<AppState>) -> Markup {
    let lines = app.supervisor.logs(500);
    page(
        &app,
        "/console",
        "Console",
        html! {
            h1 { "Console" }
            div.log id="log" sse-swap="log" hx-swap="beforeend" {
                @for l in &lines { (log_line(l)) }
            }
            form.row style="margin-top:10px" hx-post="/ui/console" hx-target="#toast" hx-on--after-request="this.reset()" {
                input name="line" placeholder="start · say <text> · players · kick <player> · ban <player> · give <player> <item> [qty] · heal <player>" autofocus style="flex:1";
                button { "Run" }
            }
        },
    )
}

fn ago(t: u64) -> String {
    let d = tabg_host::paths::now().saturating_sub(t);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86_400),
    }
}

pub async fn players(State(app): State<AppState>) -> Markup {
    let s = app.supervisor.status();
    let history = app.supervisor.player_history();
    let c = apply::read_config(&app.paths).unwrap_or_default();
    page(
        &app,
        "/players",
        "Players",
        html! {
            h1 { "Players" }
            div.card {
                h2 style="margin-top:0" { "Online" }
                div sse-swap="players" { (online_table(&s)) }
            }
            div.grid {
                div.card {
                    h2 style="margin-top:0" { "Admins" }
                    p.muted { "Admins can use chat commands like /start, /give, /kick." }
                    @for a in &c.players.admins {
                        div.row { code { (a) } button.plain hx-post="/ui/admin" hx-vals=(serde_json::json!({"who": a, "on": "false"}).to_string()) hx-target="#toast" { "Remove" } }
                    }
                    form.row hx-post="/ui/admin" hx-target="#toast" style="margin-top:8px" {
                        input type="hidden" name="on" value="true";
                        input name="who" placeholder="name or id" style="flex:1";
                        button { "Add admin" }
                    }
                }
                div.card {
                    h2 style="margin-top:0" { "Banned" }
                    @for b in &c.players.banned {
                        div.row { code { (b) } button.plain hx-post="/ui/ban" hx-vals=(serde_json::json!({"who": b, "on": "false"}).to_string()) hx-target="#toast" { "Unban" } }
                    }
                    @if c.players.banned.is_empty() { p.muted { "Nobody." } }
                    form.row hx-post="/ui/ban" hx-target="#toast" style="margin-top:8px" {
                        input type="hidden" name="on" value="true";
                        input name="who" placeholder="name or id" style="flex:1";
                        button.danger { "Ban" }
                    }
                }
            }
            div.card {
                h2 style="margin-top:0" { "History" }
                @if history.is_empty() { p.muted { "Nobody has played yet." } } @else {
                    table {
                        tr { th { "Name" } th { "Id" } th { "Last seen" } th { "Joins" } th { "Kills" } th { "Deaths" } th { "Wins" } }
                        @for p in &history {
                            tr {
                                td { (p.names.first().map_or("", String::as_str)) @if p.names.len() > 1 { br; span.muted { "also " (p.names[1..].join(", ")) } } }
                                td.muted { (p.id) }
                                td { (ago(p.last_seen)) }
                                td { (p.joins) } td { (p.kills) } td { (p.deaths) } td { (p.wins) }
                            }
                        }
                    }
                }
            }
        },
    )
}

fn field(label: &str, name: &str, value: impl std::fmt::Display, hint: &str) -> Markup {
    html! {
        div {
            label for=(name) { (label) }
            input id=(name) name=(name) value=(value.to_string());
            @if !hint.is_empty() { span.muted style="font-size:12px" { (hint) } }
        }
    }
}

fn check(label: &str, name: &str, on: bool) -> Markup {
    html! {
        div { label { input type="checkbox" name=(name) value="true" checked[on]; " " (label) } }
    }
}

pub async fn settings(State(app): State<AppState>) -> Markup {
    let c = apply::read_config(&app.paths).unwrap_or_default();
    let g = &c.game;
    page(
        &app,
        "/settings",
        "Settings",
        html! {
            h1 { "Settings" }
            form hx-post="/ui/settings" hx-target="#result" {
                div.card {
                    h2 style="margin-top:0" { "Server" }
                    div.grid {
                        (field("Name", "name", &c.name, "In the server browser only words from TABG's word list work."))
                        (field("Description", "description", &c.description, ""))
                        (field("Password", "password", &g.password, "Empty: no password."))
                        (field("Max players", "max_players", g.max_players, ""))
                    }
                }
                div.card {
                    h2 style="margin-top:0" { "Network" }
                    div.grid {
                        div {
                            label { "How friends join" }
                            select name="mode" {
                                option value="relay" selected[c.network.mode == NetMode::Relay] { "Join code (no port forwarding)" }
                                option value="direct" selected[c.network.mode == NetMode::Direct] { "Direct IP (open UDP port)" }
                            }
                        }
                        (field("Relay region", "region", &c.network.region, "Empty: Unity picks. E.g. europe-west4, us-east1."))
                        (field("UDP port (direct)", "port", c.network.port, ""))
                        (check("Open the port with UPnP (direct)", "upnp", c.network.upnp))
                        (check("Show in TABG's server browser", "public", c.network.public))
                    }
                }
                div.card {
                    h2 style="margin-top:0" { "Match" }
                    div.grid {
                        div {
                            label { "Mode" }
                            select name="game_mode" {
                                option value="battle_royale" selected[g.mode == GameMode::BattleRoyale] { "Battle Royale" }
                                option value="test" selected[g.mode == GameMode::Test] { "Test (no plane, no ring)" }
                                option value="brawl" selected[g.mode == GameMode::Brawl] { "Brawl (castle, spawn houses)" }
                                option value="bomb" selected[g.mode == GameMode::Bomb] { "Bomb (plant and defuse, rounds)" }
                                option value="princess" selected[g.mode == GameMode::Princess] { "Capture the Princess (Actual Castle against New Castle)" }
                            }
                        }
                        div {
                            label { "Map" }
                            select name="map" {
                                option value="island" selected[g.map == Map::Island] { "Island" }
                                option value="range" selected[g.map == Map::Range] { "Shooting range (range-map mod)" }
                                option value="dev_test" selected[g.map == Map::DevTest] { "DevTest island (dev-test-map mod)" }
                            }
                        }
                        div {
                            label { "Teams" }
                            select name="teams" {
                                option value="solo" selected[g.teams == Teams::Solo] { "Solo" }
                                option value="duo" selected[g.teams == Teams::Duo] { "Duos" }
                                option value="squad" selected[g.teams == Teams::Squad] { "Squads" }
                            }
                        }
                        (field("Players to start", "players_to_start", g.players_to_start, ""))
                        (field("Countdown (s)", "countdown", g.countdown, ""))
                        (field("Force start after (s)", "force_start_after", g.force_start_after, "0: never."))
                        (field("…with at least", "force_start_min_players", g.force_start_min_players, "players"))
                        (field("Loot amount", "loot_amount", g.loot_amount, "Share of loot spots used, 0–1 (default 0.1)."))
                        (field("Time of day", "time_of_day", g.time_of_day, "0–1 (0.25 = morning)."))
                        (field("Tick rate", "tick_rate", g.tick_rate, "Movement updates per second."))
                    (field("Voice range (m)", "voice_range", g.voice_range, "For friends with the ProximityChat mod; 0: off."))
                    (check("Allow any outfit (custom-game-skins mod)", "skins", g.skins))
                    (check("Destructible buildings (adds the destruction mod to the share code)", "destruction", g.destruction))
                    (field("Bots", "bots", g.bots, "Fill island matches with bots (they count towards players to start)."))
                    (field("Unused vehicles", "unused_vehicles", g.unused_vehicles, "Chance per map car to add a helicopter, UFO, hover bike… next to it (0: off). Flying needs the flying-controls mod."))
                    div {
                        label { "Bot radar (admin-radar mod)" }
                        select name="bot_radar" {
                            option value="off" selected[g.bot_radar == BotRadar::Off] { "Off" }
                            option value="admins" selected[g.bot_radar == BotRadar::Admins] { "Admins" }
                            option value="everyone" selected[g.bot_radar == BotRadar::Everyone] { "Everyone" }
                        }
                    }
                    div {
                        label { "Inventory check" }
                        select name="inventory_check" {
                            option value="off" selected[g.inventory_check == InventoryCheck::Off] { "Off: trust clients" }
                            option value="log" selected[g.inventory_check == InventoryCheck::Log] { "Log: report in the console" }
                            option value="enforce" selected[g.inventory_check == InventoryCheck::Enforce] { "Enforce: refuse" }
                        }
                        small { "Drops, throws and healing with items a player never picked up. Ammo isn't checked." }
                    }
                    (check("Spell drops (blessings from the sky)", "spell_drops", g.spell_drops))
                    (check("Respawn minigame (boss fight after an early death)", "respawn_minigame", g.respawn_minigame))
                    }
                }
                div.card {
                    h2 style="margin-top:0" { "Rules" }
                    div.grid {
                        (check("Respawn after dying (deathmatch)", "respawn", c.rules.respawn))
                        (field("Respawn delay (s)", "respawn_delay", c.rules.respawn_delay, ""))
                        (field("Kills to win", "kills_to_win", c.rules.kills_to_win, "With respawns or Brawl; 0: no limit."))
                        (field("Time limit (s)", "time_limit", c.rules.time_limit, "With respawns; 0: none. Most kills wins."))
                        (field("Bomb: rounds to win", "rounds_to_win", c.rules.rounds_to_win, ""))
                        (field("Bomb: round time (s)", "round_time", c.rules.round_time, ""))
                        (field("Bomb: bomb timer (s)", "bomb_time", c.rules.bomb_time, ""))
                        (field("Bomb: defuse time (s)", "defuse_time", c.rules.defuse_time, ""))
                        (field("Princess: players per team", "team_size", c.rules.team_size, "Bots fill the free places; 0: no bots."))
                        div {
                            label { "Princess: bot skill" }
                            select name="bot_skill" {
                                option value="easy" selected[c.rules.bot_skill == BotSkill::Easy] { "Easy" }
                                option value="normal" selected[c.rules.bot_skill == BotSkill::Normal] { "Normal" }
                                option value="hard" selected[c.rules.bot_skill == BotSkill::Hard] { "Hard" }
                            }
                            small { "For the other team's bots; yours play at normal." }
                        }
                        (field("Princess: walking speed (m/s)", "princess_speed", c.rules.princess_speed, "While held at gunpoint."))
                        (field("Princess: kills per helicopter", "heli_kills", c.rules.heli_kills, "0: one per teammate (3 to 8)."))
                        (check("Princess: turrets defend the bases from helicopters", "turrets", c.rules.turrets))
                    }
                }
                div.row { button { "Save and apply" } span id="result" {} }
            }
        },
    )
}

pub async fn loadout(State(app): State<AppState>) -> Markup {
    let c = apply::read_config(&app.paths).unwrap_or_default();
    let items = app.items();
    let list = |v: &[f32]| {
        v.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let rows = c.loadout.len() + 3;
    page(
        &app,
        "/loadout",
        "Loadout & Ring",
        html! {
            h1 { "Loadout & Ring" }
            form.card hx-post="/ui/loadout" hx-target="#loadout-result" {
                h2 style="margin-top:0" { "Landing loadout" }
                p.muted { "Every player gets these items in their inventory when they land." }
                @if items.is_empty() { p.bad { "Run setup first: item names come from your game's map data." } }
                table {
                    tr { th { "Item" } th style="width:120px" { "Quantity" } }
                    @for i in 0..rows {
                        @let current = c.loadout.get(i);
                        tr {
                            td {
                                select name=(format!("item_{i}")) {
                                    option value="" { "—" }
                                    @for (id, name) in &items {
                                        option value=(id) selected[current.is_some_and(|l| l.item == *id)] { (name) " (" (id) ")" }
                                    }
                                }
                            }
                            td { input name=(format!("qty_{i}")) value=(current.map_or(1, |l| l.qty)); }
                        }
                    }
                }
                div.row style="margin-top:10px" { button { "Save loadout" } span id="loadout-result" {} }
            }
            form.card hx-post="/ui/gun_game" hx-target="#gg-result" {
            h2 style="margin-top:0" { "Gun game levels" }
            p.muted { "One level per line as " code { "item:qty, item:qty" } ". Each kill moves the killer to the next line; finishing the last one wins. Needs respawns (Settings → Rules). Empty: off." }
            textarea name="levels" rows="6" {
                (c.rules.gun_game.iter().map(|level| level.iter().map(|i| format!("{}:{}", i.item, i.qty)).collect::<Vec<_>>().join(", ")).collect::<Vec<_>>().join("\n"))
            }
            div.row style="margin-top:10px" { button { "Save levels" } span id="gg-result" {} }
        }
        form.card hx-post="/ui/ring" hx-target="#ring-result" {
                h2 style="margin-top:0" { "Ring" }
                p.muted { "Comma-separated, one value per ring (6 rings on the main map). Empty keeps the map's own values." }
                div.grid {
                    (field("Pause after each ring (s)", "times", list(&c.ring.times), "Map default: 40, 40, 30, 30, 20, 20"))
                    (field("Ring diameters (m)", "sizes", list(&c.ring.sizes), "Default: 4240, 3450, 1710, 830, 360, 140"))
                    (field("Closing speeds", "speeds", list(&c.ring.speeds), "Default: 25, 3, 1.5, 1.5, 2, 2"))
                }
                div.row style="margin-top:10px" { button { "Save ring" } span id="ring-result" {} }
            }
        },
    )
}

pub async fn presets(State(app): State<AppState>) -> Markup {
    let all = presets::all(&app.paths.presets());
    let current = apply::read_config(&app.paths)
        .map(|c| c.preset)
        .unwrap_or_default();
    page(
        &app,
        "/presets",
        "Presets",
        html! {
            h1 { "Presets" }
            p.muted { "A preset sets the match rules (teams, countdown, loot, ring, loadout). Name, network and admins stay as they are." }
            div.grid {
                @for p in &all {
                    div.card {
                        h2 style="margin-top:0" { (p.title) @if p.id == current { " " span.pill { "current" } } }
                        p { (p.description) }
                        p.muted { code { (p.id) } @if !p.builtin { " · saved" } }
                        button hx-post="/ui/preset_apply" hx-vals=(serde_json::json!({"id": p.id}).to_string()) hx-target="#toast" { "Apply" }
                    }
                }
            }
            form.card hx-post="/ui/preset_save" hx-target="#toast" {
                h2 style="margin-top:0" { "Save current rules as a preset" }
                div.grid {
                    (field("Id", "id", "", "lowercase-with-dashes"))
                    (field("Title", "title", "", ""))
                    (field("Description", "description", "", ""))
                }
                button style="margin-top:10px" { "Save preset" }
            }
        },
    )
}

pub async fn backups(State(app): State<AppState>) -> Markup {
    let list = backup::list(&app.paths);
    page(
        &app,
        "/backups",
        "Backups",
        html! {
            h1 { "Backups" }
            p.muted { "tabg.toml and the player history are saved before every change (last " (backup::KEEP) " kept)." }
            div.card {
                @if list.is_empty() { p.muted { "No backups yet." } } @else {
                    table {
                        tr { th { "When" } th { "Why" } th {} }
                        @for b in &list {
                            tr {
                                td { (ago(b.created)) }
                                td { (b.reason) }
                                td { button.plain hx-post="/ui/backup_restore" hx-vals=(serde_json::json!({"id": b.id}).to_string()) hx-target="#toast" hx-confirm="Restore this backup?" { "Restore" } }
                            }
                        }
                    }
                }
            }
        },
    )
}

pub async fn share(State(app): State<AppState>) -> Markup {
    let c = apply::read_config(&app.paths).unwrap_or_default();
    let current = app
        .share
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|p| p.code.encode());
    page(
        &app,
        "/share",
        "Share",
        html! {
            h1 { "Share" }
            div.card {
                h2 style="margin-top:0" { "Friends with the plain game" }
                p { "Nothing to install: they type the join code (Overview) into TABG's join box." }
            }
            form.card hx-post="/ui/mods" hx-target="#mods-result" {
                h2 style="margin-top:0" { "Client mods" }
                p.muted { "Optional extras friends need on their side. One per line: a catalog mod as " code { "id@version" } ", or a path to a DLL of your own (served to friends from this PC)." }
                textarea name="mods" rows="5" { (c.mods.client.join("\n")) }
                @if c.game.destruction && c.client_mods().len() > c.mods.client.len() {
                    p.muted { code { "destruction" } " is added too, because destructible buildings are on (Settings)." }
                }
                div.row style="margin-top:10px" { button { "Save mods" } span id="mods-result" {} }
            }
            div.card {
                h2 style="margin-top:0" { "Share code" }
                p.muted { "Friends run " code { "tabg join <share code>" } ": it makes a modded copy of their TABG with exactly these mods (hash-checked) and tells them the join code." }
                div id="share-code" { @if let Some(code) = current { (share_box(&code)) } }
                button hx-post="/ui/share" hx-target="#share-code" { "Make share code" }
            }
        },
    )
}

fn share_box(code: &str) -> Markup {
    html! {
        textarea readonly rows="3" onclick="this.select()" { (code) }
        p.muted { "Send it with: " code { "tabg join " (code) } }
    }
}

fn applied_message(a: &Applied) -> &'static str {
    match a {
        Applied::Saved => "Saved.",
        Applied::Reloaded => "Saved — the running server took the change.",
        Applied::Restarted => "Saved — the server restarted to apply it.",
    }
}

fn answer(m: Markup) -> Response {
    m.into_response()
}

/// Answer and reload the page (for actions that change lists on it).
fn answer_refresh(m: Markup) -> Response {
    let mut r = m.into_response();
    r.headers_mut()
        .insert("HX-Refresh", HeaderValue::from_static("true"));
    r
}

fn apply_answer(r: Result<Applied, ApplyError>) -> Markup {
    match r {
        Ok(a) => html! { span.ok { (applied_message(&a)) } },
        Err(ApplyError::Invalid { problems }) => html! {
            ul.problems { @for p in problems { li { b { (p.field) } ": " (p.message) } } }
        },
        Err(ApplyError::Failed { error }) => html! { span.bad { (error) } },
    }
}

fn parse<T: std::str::FromStr>(
    form: &HashMap<String, String>,
    key: &str,
    into: &mut T,
) -> Result<(), String> {
    if let Some(v) = form.get(key) {
        *into = v
            .trim()
            .parse()
            .map_err(|_| format!("{key}: {v:?} is not a valid value"))?;
    }
    Ok(())
}

fn numbers(text: &str) -> Result<Vec<f32>, String> {
    text.split([',', ' '])
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.trim()
                .parse::<f32>()
                .map_err(|_| format!("{s:?} is not a number"))
        })
        .collect()
}

fn settings_from_form(
    mut c: ServerToml,
    f: &HashMap<String, String>,
) -> Result<ServerToml, String> {
    let text = |k: &str| f.get(k).map(|v| v.trim().to_owned());
    if let Some(v) = text("name") {
        c.name = v;
    }
    if let Some(v) = text("description") {
        c.description = v;
    }
    if let Some(v) = f.get("password") {
        c.game.password.clone_from(v);
    }
    parse(f, "max_players", &mut c.game.max_players)?;
    c.network.mode = match f.get("mode").map(String::as_str) {
        Some("direct") => NetMode::Direct,
        _ => NetMode::Relay,
    };
    if let Some(v) = text("region") {
        c.network.region = v;
    }
    parse(f, "port", &mut c.network.port)?;
    c.network.upnp = f.contains_key("upnp");
    c.network.public = f.contains_key("public");
    c.game.mode = match f.get("game_mode").map(String::as_str) {
        Some("test") => GameMode::Test,
        Some("brawl") => GameMode::Brawl,
        Some("bomb") => GameMode::Bomb,
        Some("princess") => GameMode::Princess,
        _ => GameMode::BattleRoyale,
    };
    c.game.map = match f.get("map").map(String::as_str) {
        Some("range") => Map::Range,
        Some("dev_test") => Map::DevTest,
        _ => Map::Island,
    };
    c.game.teams = match f.get("teams").map(String::as_str) {
        Some("solo") => Teams::Solo,
        Some("duo") => Teams::Duo,
        _ => Teams::Squad,
    };
    parse(f, "players_to_start", &mut c.game.players_to_start)?;
    parse(f, "countdown", &mut c.game.countdown)?;
    parse(f, "force_start_after", &mut c.game.force_start_after)?;
    parse(
        f,
        "force_start_min_players",
        &mut c.game.force_start_min_players,
    )?;
    parse(f, "loot_amount", &mut c.game.loot_amount)?;
    parse(f, "time_of_day", &mut c.game.time_of_day)?;
    parse(f, "tick_rate", &mut c.game.tick_rate)?;
    parse(f, "voice_range", &mut c.game.voice_range)?;
    c.game.skins = f.contains_key("skins");
    c.game.destruction = f.contains_key("destruction");
    c.game.spell_drops = f.contains_key("spell_drops");
    parse(f, "bots", &mut c.game.bots)?;
    parse(f, "unused_vehicles", &mut c.game.unused_vehicles)?;
    c.game.bot_radar = match f.get("bot_radar").map(String::as_str) {
        Some("admins") => BotRadar::Admins,
        Some("everyone") => BotRadar::Everyone,
        _ => BotRadar::Off,
    };
    c.game.inventory_check = match f.get("inventory_check").map(String::as_str) {
        Some("off") => InventoryCheck::Off,
        Some("enforce") => InventoryCheck::Enforce,
        _ => InventoryCheck::Log,
    };
    c.game.respawn_minigame = f.contains_key("respawn_minigame");
    c.rules.respawn = f.contains_key("respawn");
    parse(f, "respawn_delay", &mut c.rules.respawn_delay)?;
    parse(f, "kills_to_win", &mut c.rules.kills_to_win)?;
    parse(f, "time_limit", &mut c.rules.time_limit)?;
    parse(f, "rounds_to_win", &mut c.rules.rounds_to_win)?;
    parse(f, "round_time", &mut c.rules.round_time)?;
    parse(f, "bomb_time", &mut c.rules.bomb_time)?;
    parse(f, "defuse_time", &mut c.rules.defuse_time)?;
    parse(f, "team_size", &mut c.rules.team_size)?;
    c.rules.bot_skill = match f.get("bot_skill").map(String::as_str) {
        Some("easy") => BotSkill::Easy,
        Some("hard") => BotSkill::Hard,
        _ => BotSkill::Normal,
    };
    parse(f, "princess_speed", &mut c.rules.princess_speed)?;
    parse(f, "heli_kills", &mut c.rules.heli_kills)?;
    c.rules.turrets = f.contains_key("turrets");
    Ok(c)
}

async fn run_apply(
    app: &AppState,
    next: ServerToml,
    reason: String,
) -> Result<Applied, ApplyError> {
    let app = Arc::clone(app);
    tokio::task::spawn_blocking(move || apply::apply(&app.paths, &app.supervisor, &next, &reason))
        .await
        .expect("task ran")
}

pub async fn action(
    State(app): State<AppState>,
    Path(action): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let get = |k: &str| form.get(k).cloned().unwrap_or_default();
    let current = match apply::read_config(&app.paths) {
        Ok(c) => c,
        Err(e) => return answer(html! { span.bad { (e) } }),
    };
    match action.as_str() {
        "start" | "stop" | "restart" => {
            let sup = app.supervisor.clone();
            let a = action.clone();
            tokio::task::spawn_blocking(move || match a.as_str() {
                "start" => sup.start(),
                "stop" => sup.stop(),
                _ => sup.restart(),
            })
            .await
            .expect("task ran");
            let s = app.supervisor.status();
            answer(match &s.error {
                Some(e) => html! { span.bad { (e) } },
                None => html! { span { "Server " (state_name(s.state)) "." } },
            })
        }
        "setup" => answer(match app.start_setup(get("force") == "true") {
            Ok(()) => html! { span { "Setup started — progress shows in the console." } },
            Err(e) => html! { span.bad { (e) } },
        }),
        "console" => {
            let line = get("line");
            if line.trim().is_empty() {
                return answer(html! {});
            }
            answer(match app.supervisor.console(&line) {
                Ok(()) => html! {},
                Err(e) => html! { span.bad { (e) } },
            })
        }
        "kick" => answer(
            match app.supervisor.console(&format!("kick {}", get("who"))) {
                Ok(()) => html! { span { "Kicked." } },
                Err(e) => html! { span.bad { (e) } },
            },
        ),
        "admin" | "ban" => {
            let g = Grant {
                who: get("who"),
                on: get("on") == "true",
            };
            let kick = (action == "ban" && g.on).then(|| g.who.clone());
            let app2 = Arc::clone(&app);
            let is_ban = action == "ban";
            let r = tokio::task::spawn_blocking(move || {
                if is_ban {
                    grant(&app2, &g, |c| &mut c.players.banned, "ban")
                } else {
                    grant(&app2, &g, |c| &mut c.players.admins, "admin")
                }
            })
            .await
            .expect("task ran");
            if let Some(who) = kick {
                let _ = app.supervisor.console(&format!("kick {who}"));
            }
            match r {
                Ok(a) => answer_refresh(html! { span.ok { (applied_message(&a)) } }),
                Err(e) => answer(html! { span.bad { (e) } }),
            }
        }
        "settings" => match settings_from_form(current, &form) {
            Ok(next) => answer(apply_answer(run_apply(&app, next, "settings".into()).await)),
            Err(e) => answer(html! { span.bad { (e) } }),
        },
        "loadout" => {
            let mut next = current;
            next.loadout.clear();
            for i in 0..64 {
                let (Some(item), qty) = (
                    form.get(&format!("item_{i}"))
                        .and_then(|v| v.parse::<i32>().ok()),
                    form.get(&format!("qty_{i}"))
                        .and_then(|v| v.trim().parse::<i32>().ok())
                        .unwrap_or(1),
                ) else {
                    continue;
                };
                next.loadout
                    .push(tabg_host::config::LoadoutItem { item, qty });
            }
            answer(apply_answer(run_apply(&app, next, "loadout".into()).await))
        }
        "gun_game" => {
            let mut next = current;
            let mut levels = Vec::new();
            for (n, line) in get("levels").lines().enumerate() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let mut level = Vec::new();
                for part in line.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    let (item, qty) = part.split_once(':').unwrap_or((part, "1"));
                    match (item.trim().parse::<i32>(), qty.trim().parse::<i32>()) {
                        (Ok(item), Ok(qty)) => {
                            level.push(tabg_host::config::LoadoutItem { item, qty })
                        }
                        _ => {
                            return answer(
                                html! { span.bad { "line " (n + 1) ": " (part) " is not item:qty" } },
                            );
                        }
                    }
                }
                levels.push(level);
            }
            next.rules.gun_game = levels;
            answer(apply_answer(run_apply(&app, next, "gun game".into()).await))
        }
        "ring" => {
            let mut next = current;
            let parsed = (
                numbers(&get("times")),
                numbers(&get("sizes")),
                numbers(&get("speeds")),
            );
            match parsed {
                (Ok(t), Ok(s), Ok(v)) => {
                    next.ring.times = t;
                    next.ring.sizes = s;
                    next.ring.speeds = v;
                    answer(apply_answer(run_apply(&app, next, "ring".into()).await))
                }
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    answer(html! { span.bad { (e) } })
                }
            }
        }
        "preset_apply" => {
            let Some(p) = presets::find(&app.paths.presets(), &get("id")) else {
                return answer(html! { span.bad { "no such preset" } });
            };
            match presets::apply(&current, &p) {
                Ok(next) => {
                    let r = run_apply(&app, next, format!("preset {}", p.id)).await;
                    match r {
                        Ok(a) => answer_refresh(html! { span.ok { (applied_message(&a)) } }),
                        Err(e) => answer(apply_answer(Err(e))),
                    }
                }
                Err(e) => answer(html! { span.bad { (e) } }),
            }
        }
        "preset_save" => {
            let id = get("id");
            let title = if get("title").is_empty() {
                id.clone()
            } else {
                get("title")
            };
            match presets::save(
                &app.paths.presets(),
                &id,
                &title,
                &get("description"),
                &current,
            ) {
                Ok(()) => answer_refresh(html! { span.ok { "Saved." } }),
                Err(e) => answer(html! { span.bad { (e) } }),
            }
        }
        "mods" => {
            let mut next = current;
            next.mods.client = get("mods")
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            answer(apply_answer(
                run_apply(&app, next, "client mods".into()).await,
            ))
        }
        "share" => match crate::api::make_share(&app).await {
            Ok(v) => answer(share_box(v["code"].as_str().unwrap_or(""))),
            Err(e) => answer(html! { span.bad { (e) } }),
        },
        "backup_restore" => match backup::restore(&app.paths, &get("id")) {
            Ok(()) => {
                let restored = apply::read_config(&app.paths).unwrap_or_default();
                let r = run_apply(&app, restored, format!("restore {}", get("id"))).await;
                answer_refresh(apply_answer(r))
            }
            Err(e) => answer(html! { span.bad { (e) } }),
        },
        _ => answer(html! { span.bad { "unknown action" } }),
    }
}
