use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::Parser;
use tabg_game::{RoomSettings, SpawnPoint};
use tabg_server::{ServerConfig, run};

/// Run a tabg game server (development entry point; `tabg` will manage this for you).
#[derive(Parser)]
#[command(version)]
struct Args {
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
    #[arg(long, default_value_t = 7777)]
    port: u16,
    #[arg(long, default_value_t = 70)]
    max_players: u8,
    #[arg(long, default_value = "")]
    password: String,
    /// Map data from `scripts/dump-world.sh`: lobby spawn points and ring timing.
    #[arg(long)]
    world: Option<std::path::PathBuf>,
    /// Lobby spawn point as x,y,z (repeatable); overrides --world's spawn points.
    #[arg(long = "spawn", value_parser = parse_vec3, allow_hyphen_values = true)]
    spawns: Vec<[f32; 3]>,
    /// Players needed before the countdown starts.
    #[arg(long, default_value_t = 2)]
    players_to_start: u8,
    /// Countdown length in seconds.
    #[arg(long, default_value_t = 20.0)]
    countdown: f32,
    /// Time of day for lighting, 0..1 (0.05 morning, 0.2 midday, 0.95 evening).
    #[arg(long, default_value_t = 0.25)]
    time_of_day: f32,
    /// Player name with admin rights (repeatable; `*` = everyone). Admins can force-start and
    /// use chat commands: /start, /give <item> [qty], /heal, /kick <name>, /weather <name|auto>.
    #[arg(long = "admin")]
    admins: Vec<String>,
    /// Seed for ring positions and the plane path (random by default).
    #[arg(long)]
    seed: Option<u64>,
    /// Host through Unity Relay: players type the printed join code into TABG's join box. No port
    /// forwarding needed. Needs the Unity project id, read from --game-dir.
    #[arg(long)]
    relay: bool,
    /// Relay region (e.g. europe-west4); Unity picks one if unset. `--relay-region list` lists them.
    #[arg(long)]
    relay_region: Option<String>,
    /// A TABG install (client or dedicated server) to read the Unity project id from.
    #[arg(long, env = "TABG_CLIENT_DIR")]
    game_dir: Option<std::path::PathBuf>,
    /// Show the server in TABG's community server browser (needs --world for the game version).
    #[arg(long)]
    list: bool,
    /// Server name in the browser.
    #[arg(long, default_value = "tabg server")]
    name: String,
    /// Server description in the browser.
    #[arg(long, default_value = "")]
    description: String,
    /// Direct mode: open the port on the router with UPnP.
    #[arg(long)]
    upnp: bool,
    /// Items everyone gets on landing, as `item:qty,item:qty` (e.g. `160:1,6:240`).
    #[arg(long, value_parser = parse_loadout)]
    loadout: Option<Loadout>,
    /// Respawn dead players after this many seconds (deathmatch).
    #[arg(long)]
    respawn: Option<f32>,
    /// With --respawn: the first team to this many kills wins (0 = no limit).
    #[arg(long, default_value_t = 20)]
    kills_to_win: u16,
    /// Play a client-mod map in Test mode: `range` or `dev-test` (sets its spawn points).
    #[arg(long)]
    test_map: Option<String>,
    /// Early Battle Royale deaths go to the boss fight (the respawn minigame).
    #[arg(long)]
    respawn_minigame: bool,
    /// Fill the match with this many bots (needs the walkability grid next to --world).
    #[arg(long, default_value_t = 0)]
    bots: u8,
    /// Bots jump at this fraction of the flight (testing: match TABG_TEST_JUMP to land together).
    #[arg(long)]
    bot_jump: Option<f32>,
    /// Chance per map car to add one of the unused vehicle types near it (0 = off).
    #[arg(long, default_value_t = 0.0)]
    unused_vehicles: f32,
    /// Show the bots on the admin-radar client mod: off, admins or everyone.
    #[arg(long, default_value = "off", value_parser = ["off", "admins", "everyone"])]
    bot_radar: String,
    /// Buildings stay whole, even for players with the Destruction client mod.
    #[arg(long)]
    no_destruction: bool,
    /// Check drops, throws and healing against what the server saw a player get:
    /// off, log (report only) or enforce (refuse).
    #[arg(long, default_value = "log", value_parser = ["off", "log", "enforce"])]
    inventory_check: String,
    /// Play Capture the Princess on the island (Actual Castle against New Castle; needs --world).
    #[arg(long)]
    princess: bool,
    /// Capture the Princess: players per team, bots fill the rest.
    #[arg(long, default_value_t = 3)]
    team_size: u8,
    /// Capture the Princess: how well the other team's bots play.
    #[arg(long, default_value = "normal", value_parser = ["easy", "normal", "hard"])]
    bot_skill: String,
    /// Capture the Princess, testing: both princesses wait next to team 0's cage, in view of
    /// whoever spawns there.
    #[arg(long)]
    princess_showcase: bool,
}

#[derive(Clone, Debug)]
struct Loadout(Vec<(i32, i32)>);

fn parse_loadout(s: &str) -> Result<Loadout, String> {
    s.split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            let (item, qty) = p.split_once(':').unwrap_or((p, "1"));
            Ok((
                item.trim()
                    .parse()
                    .map_err(|_| format!("bad item {item:?}"))?,
                qty.trim()
                    .parse()
                    .map_err(|_| format!("bad quantity {qty:?}"))?,
            ))
        })
        .collect::<Result<_, String>>()
        .map(Loadout)
}

/// Used when neither --world nor --spawn is given: one lobby spawn of the default map, as seen from
/// Landfall's server in our reference run.
const FALLBACK_SPAWN: [f32; 3] = [-35.7, 127.3, -624.4];

fn parse_vec3(s: &str) -> Result<[f32; 3], String> {
    let parts: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse::<f32>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    parts.try_into().map_err(|_| "expected x,y,z".to_string())
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let seed = args.seed.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64)
    });
    let loadout = args.loadout.clone().map(|l| l.0).unwrap_or_default();
    let mut app_version = String::new();
    let mut settings = RoomSettings {
        max_players: args.max_players,
        password: args.password,
        players_to_start: args.players_to_start,
        countdown: args.countdown,
        time_of_day: args.time_of_day.clamp(0.0, 1.0),
        seed,
        admins: args.admins.clone(),
        loadout,
        respawn: args.respawn.map(|delay| tabg_game::Respawn { delay }),
        kills_to_win: args.kills_to_win,
        respawn_minigame: args.respawn_minigame,
        bots: args.bots,
        bot_jump: args.bot_jump,
        destruction: !args.no_destruction,
        bot_radar: match args.bot_radar.as_str() {
            "admins" => tabg_game::BotRadar::Admins,
            "everyone" => tabg_game::BotRadar::Everyone,
            _ => tabg_game::BotRadar::Off,
        },
        inventory_check: match args.inventory_check.as_str() {
            "off" => tabg_game::InventoryCheck::Off,
            "enforce" => tabg_game::InventoryCheck::Enforce,
            _ => tabg_game::InventoryCheck::Log,
        },
        spawn_points: vec![SpawnPoint {
            position: FALLBACK_SPAWN,
            rotation: 0.0,
        }],
        ..Default::default()
    };

    if let Some(path) = &args.world {
        let world = tabg_world::World::load(path).unwrap_or_else(|e| fail(e));
        app_version.clone_from(&world.app_version);
        if let Err(e) = tabg_server::apply_world(&mut settings, &world) {
            fail(format!("{}: {e}", path.display()));
        }
        settings.nav = tabg_server::load_nav(path);
        if settings.nav.is_none() && args.bots > 0 {
            tracing::warn!(
                "no walkability grid next to {}; bots are off",
                path.display()
            );
        }
        tracing::info!(
            spawns = settings.spawn_points.len(),
            loot_spots = settings.loot.spots.len(),
            cars = settings.cars.len(),
            "loaded map data"
        );
    }
    settings.vehicles.unused_chance = args.unused_vehicles.clamp(0.0, 1.0);
    if !args.spawns.is_empty() {
        settings.spawn_points = args
            .spawns
            .into_iter()
            .map(|position| SpawnPoint {
                position,
                rotation: 0.0,
            })
            .collect();
    }

    let relay = if args.relay || args.relay_region.is_some() {
        let dir = args
            .game_dir
            .as_deref()
            .unwrap_or_else(|| fail("--relay needs --game-dir (or TABG_CLIENT_DIR)"));
        let project_id = tabg_utp::project_id_from_game(dir).unwrap_or_else(|e| fail(e));
        if args.relay_region.as_deref() == Some("list") {
            let session = tabg_utp::ugs::Session::sign_in(&project_id).unwrap_or_else(|e| fail(e));
            for r in session.regions().unwrap_or_else(|e| fail(e)) {
                println!("{:24} {}", r.id, r.description);
            }
            return;
        }
        Some(tabg_utp::RelayConfig {
            project_id,
            region: args.relay_region,
            max_players: u32::from(args.max_players),
        })
    } else {
        None
    };

    let listing = args.list.then(|| {
        if app_version.is_empty() {
            fail(
                "--list needs the game version from a --world dump (re-run scripts/dump-world.sh)",
            );
        }
        tabg_server::listing::ListingConfig {
            name: args.name.clone(),
            description: args.description.clone(),
            version: app_version.clone(),
        }
    });

    match args.test_map.as_deref() {
        None => {}
        Some(name) => {
            let (map, spawns): (tabg_game::TestMap, &[[f32; 3]]) = match name {
                "range" => (tabg_game::TestMap::Range, &[[6.32, 117.10, -25.41]]),
                "dev-test" => (
                    tabg_game::TestMap::DevTest,
                    &[
                        [-37.0, 111.0, -11.0],
                        [6.0, 112.0, 21.0],
                        [-2.0, 111.0, 2.0],
                    ],
                ),
                other => fail(format!("unknown test map {other:?} (range, dev-test)")),
            };
            settings.game_mode = 2;
            settings.test_map = Some(map);
            settings.spawn_points = spawns
                .iter()
                .map(|&position| SpawnPoint {
                    position,
                    rotation: 0.0,
                })
                .collect();
            settings.loot.spots.clear();
            settings.cars.clear();
        }
    }

    if args.princess {
        settings.princess = Some(tabg_game::PrincessRules {
            team_size: args.team_size,
            bot_skill: match args.bot_skill.as_str() {
                "easy" => tabg_game::BotSkill::Easy,
                "hard" => tabg_game::BotSkill::Hard,
                _ => tabg_game::BotSkill::Normal,
            },
            ..tabg_game::PrincessRules::island()
        });
        if args.princess_showcase
            && let Some(rules) = settings.princess.as_mut()
        {
            let cage = rules.bases[0].cage;
            rules.bases[0].hideouts = vec![[cage[0] + 2.0, cage[1], cage[2] - 2.0]];
            rules.bases[1].hideouts = vec![[cage[0] + 2.0, cage[1], cage[2] + 2.0]];
            rules.team_size = 1;
        }
        settings.respawn = Some(tabg_game::Respawn {
            delay: args.respawn.unwrap_or(8.0),
        });
        settings.bots = 0;
        settings.spell_drops = None;
        if settings.nav.is_none() {
            tracing::warn!("Capture the Princess needs the walkability grid next to --world");
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        // Stop cleanly on Ctrl-C so the UPnP mapping is removed.
        let _ = ctrlc::set_handler(move || stop.store(true, std::sync::atomic::Ordering::Relaxed));
    }
    if let Err(e) = run(
        ServerConfig {
            bind: args.bind,
            port: args.port,
            relay,
            listing,
            upnp: args.upnp,
            settings,
        },
        stop,
    ) {
        fail(e);
    }
}
