//! Capture the Princess, played through the room's packet interface on a flat test island.

use super::*;
use crate::princess::{BotSkill, PrincessRules};
use tabg_protocol::messages::WorldState;
use tabg_protocol::messages::game_mode;

fn flat_island() -> std::sync::Arc<crate::nav::NavGrid> {
    // 4 km × 4 km of flat ground at 120 m, 4 m cells.
    let n = 1000;
    std::sync::Arc::new(crate::nav::NavGrid {
        min_x: -2000.0,
        min_z: -2000.0,
        cell: 4.0,
        width: n,
        depth: n,
        heights: vec![20 * 64; n * n],
        ..Default::default()
    })
}

/// Bases 600 m apart (cages at x = ±300, z = 10; hideouts at z = -20), teams of three.
fn settings(team_size: u8) -> RoomSettings {
    RoomSettings {
        princess: Some(PrincessRules {
            team_size,
            ..PrincessRules::default()
        }),
        nav: Some(flat_island()),
        respawn: Some(crate::settings::Respawn { delay: 8.0 }),
        players_to_start: 1,
        countdown: 1.0,
        force_start: None,
        loadout: vec![(160, 1), (6, 240)],
        ..Default::default()
    }
}

fn events(out: &[Outgoing], event: EventCode) -> Vec<&Outgoing> {
    out.iter()
        .filter(|o| o.packet.event() == Some(event))
        .collect()
}

fn run(room: &mut Room, from: f32, to: f32) -> Vec<Outgoing> {
    let mut out = Vec::new();
    let mut t = from;
    while t <= to + 1e-4 {
        out.extend(room.tick(t));
        t += 0.1;
    }
    out
}

fn join(room: &mut Room, peer: u8, name: &str) {
    room.connect(peer);
    let req = RoomInitRequest {
        player_name: name.into(),
        squad_size: 1,
        ..Default::default()
    };
    room.handle(peer, &Packet::new(EventCode::RoomInit, req.encode()));
    let out = room.handle(peer, &Packet::new(EventCode::RequestWorldState, vec![]));
    WorldState::decode(&out[0].packet.payload, game_mode::BATTLE_ROYALE).expect("world state");
}

/// One player in a started match (bots off unless `team_size` > 1).
fn started(team_size: u8) -> Room {
    let mut room = Room::new(settings(team_size));
    join(&mut room, 0, "human");
    run(&mut room, 0.0, 1.5);
    assert_eq!(room.state(), game_state::STARTED);
    room
}

fn princess(room: &Room, team: u8) -> u8 {
    room.princess_index(team).expect("princess")
}

fn cage(room: &Room, team: u8) -> [f32; 3] {
    room.princess_rules().bases[usize::from(team)].cage
}

/// Put `peer` next to `at`, gun out, looking at it.
fn hold_at_gunpoint(room: &mut Room, peer: u8, at: [f32; 3], from_x: f32) {
    let p = room.players.get_mut(&peer).unwrap();
    p.position = [at[0] + from_x, at[1], at[2]];
    p.weapon_type = 160;
    let yaw = (-from_x).atan2(0.0).to_degrees().rem_euclid(360.0);
    p.rotation = [0.0, yaw];
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

#[test]
fn the_lobby_gets_two_princesses_and_bots_to_fill_both_teams() {
    let mut room = Room::new(Room::lobby_like(settings(3)));
    join(&mut room, 0, "human");
    let out = room.tick(0.1);
    // Two princesses and five bots join.
    assert_eq!(events(&out, EventCode::Login).len(), 7);
    let team = room.players[&0].group;
    assert!(team < 2);
    for g in 0..2u8 {
        let members = room.players.values().filter(|p| p.group == g).count();
        assert_eq!(members, 3, "team {g}");
    }
    let blue = princess(&room, 0);
    let pink = princess(&room, 1);
    assert_eq!(room.players[&blue].group, 2);
    assert_eq!(room.players[&pink].group, 3);
    assert_ne!(
        room.players[&blue].gear, room.players[&pink].gear,
        "dressed apart"
    );
    assert!(room.is_bot(blue) && room.is_princess(blue));
    // A second human takes a bot's place on the other team.
    join(&mut room, 1, "friend");
    room.tick(0.2);
    assert_ne!(room.players[&1].group, team);
    let bots = room.bots.len();
    assert_eq!(bots, 4);
    // Players wait at their base.
    let base = room.players[&0].position;
    assert!(flat(base, cage(&room, team)) < 9.0, "{base:?}");
}

#[test]
fn the_princess_channel_is_its_own() {
    let ours = super::princess::PRINCESS;
    let theirs = [
        super::voice::VOICE,
        super::radar::RADAR,
        super::maps::RANGE,
        super::maps::DEV_TEST,
        super::skins::SKINS,
        super::vehicles::VEHICLES,
        super::destruction::DESTRUCTION,
        super::weather::WEATHER,
    ];
    assert!(!theirs.contains(&ours));
    assert_eq!(EventCode::from_u8(ours), None, "not a TABG event either");
}

#[test]
fn squad_mates_play_together_and_others_against_each_other() {
    let mut room = Room::new(Room::lobby_like(settings(3)));
    let squad = |room: &mut Room, peer: u8, squad_id: u64| {
        room.connect(peer);
        let req = RoomInitRequest {
            player_name: format!("p{peer}"),
            squad_size: 2,
            squad_id,
            ..Default::default()
        };
        room.handle(peer, &Packet::new(EventCode::RoomInit, req.encode()));
    };
    squad(&mut room, 0, 77);
    squad(&mut room, 1, 77);
    squad(&mut room, 2, 0);
    assert_eq!(room.players[&0].group, room.players[&1].group, "one squad");
    assert_ne!(
        room.players[&2].group, room.players[&0].group,
        "alone: the other team"
    );
    room.tick(0.1);
    let team = room.players[&0].group;
    let mates = room.players.values().filter(|p| p.group == team).count();
    assert_eq!(mates, 3, "two players and one bot");
}

impl Room {
    /// Test settings that stay in the lobby.
    fn lobby_like(mut s: RoomSettings) -> RoomSettings {
        s.players_to_start = 100;
        s
    }
}

#[test]
fn the_match_starts_at_the_cages_with_the_princesses_hidden() {
    let mut room = Room::new(settings(3));
    join(&mut room, 0, "human");
    let out = run(&mut room, 0.0, 1.5);
    assert_eq!(room.state(), game_state::STARTED);
    let respawns = events(&out, EventCode::PlayerRespawn);
    assert!(!respawns.is_empty());
    assert_eq!(respawns[0].packet.payload[0], 8, "everyone appears at once");
    for p in room.players.values() {
        if let Some(i) = room.princess_of(p.index) {
            let home = room.princess.princesses[i].home;
            assert_eq!(p.position, home);
        } else {
            let d = flat(p.position, cage(&room, p.group));
            assert!(d < 9.0, "{} at its cage: {d}", p.name);
        }
    }
    // The player got the loadout, nobody boarded a plane.
    assert!(!events(&out, EventCode::PlayerLootRecieved).is_empty());
    assert!(room.plane.is_none());
}

#[test]
fn holding_a_gun_on_the_other_princess_walks_her_to_your_cage_and_wins() {
    let mut room = started(0);
    let team = room.players[&0].group;
    let theirs = princess(&room, 1 - team);
    let at = room.players[&theirs].position;
    hold_at_gunpoint(&mut room, 0, at, 6.0);
    // Without a gun nothing happens.
    room.players.get_mut(&0).unwrap().weapon_type = NO_WEAPON;
    run(&mut room, 2.0, 3.0);
    assert_eq!(room.players[&theirs].position, at, "no gun, no walk");
    room.players.get_mut(&0).unwrap().weapon_type = 160;
    let out = run(&mut room, 3.0, 5.0);
    let moved = room.players[&theirs].position;
    assert!(flat(moved, at) > 3.0, "she walks: {moved:?}");
    let d0 = flat(at, cage(&room, team));
    assert!(flat(moved, cage(&room, team)) < d0, "towards our cage");
    assert!(
        !events(&out, EventCode::ChatMessage).is_empty(),
        "the player hears about it (as chat, without the mod)"
    );
    assert_eq!(room.princess_escort(1 - team).map(|e| e.team), Some(team));
    // Keep the gun on her all the way: about 600 m at 3.5 m/s.
    let mut t = 5.0;
    let mut ended = None;
    while t < 260.0 && ended.is_none() {
        let her = room.players[&theirs].position;
        let behind = if cage(&room, team)[0] > her[0] {
            -5.0
        } else {
            5.0
        };
        hold_at_gunpoint(&mut room, 0, her, behind);
        let out = room.tick(t);
        ended = events(&out, EventCode::GameStateChanged)
            .into_iter()
            .find(|o| o.packet.payload.first() == Some(&game_state::ENDED))
            .map(|o| o.packet.payload[1]);
        t += 0.1;
    }
    assert_eq!(ended, Some(team), "caged: we win");
}

#[test]
fn contested_princesses_stand_still() {
    let mut room = started(0);
    join_late_bot(&mut room);
    let team = room.players[&0].group;
    let theirs = princess(&room, 1 - team);
    let at = room.players[&theirs].position;
    hold_at_gunpoint(&mut room, 0, at, 6.0);
    let guard = room
        .players
        .values()
        .find(|p| p.group == 1 - team && !room.is_princess(p.index))
        .map(|p| p.index)
        .expect("a bot on their team");
    hold_at_gunpoint(&mut room, guard, at, -6.0);
    // The bot would walk off; keep it there.
    room.tick(2.0);
    hold_at_gunpoint(&mut room, guard, at, -6.0);
    room.bots.get_mut(&guard).unwrap().princess.holding = Some(theirs);
    hold_at_gunpoint(&mut room, 0, at, 6.0);
    room.tick(2.1);
    let i = room.princess_of(theirs).unwrap();
    assert!(room.princess.princesses[i].contested);
    let held = room.players[&theirs].position;
    for k in 0..5 {
        hold_at_gunpoint(&mut room, guard, at, -6.0);
        room.bots.get_mut(&guard).unwrap().princess.holding = Some(theirs);
        hold_at_gunpoint(&mut room, 0, at, 6.0);
        room.tick(2.2 + k as f32 * 0.1);
    }
    assert_eq!(room.players[&theirs].position, held, "she stays put");
}

/// A bot on the other team, for tests that start with bots off.
fn join_late_bot(room: &mut Room) {
    let team = 1 - room.players[&0].group;
    room.add_bot(team).expect("room for a bot");
    let bot = *room.bots.keys().next().unwrap();
    let p = room.players.get_mut(&bot).unwrap();
    p.dropped = true;
    p.landed = true;
}

fn kill(room: &mut Room, victim: u8, attacker: u8) -> Vec<Outgoing> {
    room.players.get_mut(&victim).unwrap().last_attacker = attacker;
    room.kill(victim, false)
}

#[test]
fn killing_your_own_princess_brings_her_halfway_back_but_faster() {
    let mut room = started(0);
    join_late_bot(&mut room);
    let team = room.players[&0].group;
    let theirs = princess(&room, 1 - team);
    let home = room.players[&theirs].position;
    // We walk her 60 m towards our cage.
    let mut t = 2.0;
    while flat(room.players[&theirs].position, home) < 60.0 && t < 60.0 {
        let her = room.players[&theirs].position;
        let behind = if cage(&room, team)[0] > her[0] {
            -5.0
        } else {
            5.0
        };
        hold_at_gunpoint(&mut room, 0, her, behind);
        room.tick(t);
        t += 0.1;
    }
    let fell_at = room.players[&theirs].position;
    // Her own team (the bot) shoots her by accident.
    let bot = *room.bots.keys().next().unwrap();
    room.players.get_mut(&0).unwrap().weapon_type = NO_WEAPON;
    let kills = room.players[&bot].kills;
    let out = kill(&mut room, theirs, bot);
    assert_eq!(events(&out, EventCode::PlayerDead).len(), 1);
    assert_eq!(room.players[&bot].kills, kills, "no score for that");
    // The bot would bring her home now; this test is about her speed.
    room.bots.remove(&bot);
    room.disconnect(bot);
    // And it may have shot us meanwhile.
    let me = room.players.get_mut(&0).unwrap();
    me.alive = true;
    me.health = 100.0;
    me.respawn_at = None;
    let out = run(&mut room, t, t + 5.0);
    let back_again = events(&out, EventCode::PlayerRespawn)
        .into_iter()
        .filter(|o| o.packet.payload[1] == theirs)
        .count();
    assert_eq!(back_again, 1);
    let back = room.players[&theirs].position;
    let half = [
        (home[0] + fell_at[0]) / 2.0,
        0.0,
        (home[2] + fell_at[2]) / 2.0,
    ];
    assert!(flat(back, half) < 2.0, "halfway: {back:?} vs {half:?}");
    let i = room.princess_of(theirs).unwrap();
    assert_eq!(room.princess.princesses[i].stacks, 1);
    // Now she walks 15 % faster for us.
    let start = room.players[&theirs].position;
    let t0 = t + 5.1;
    let mut t = t0;
    while t < t0 + 4.0 {
        let her = room.players[&theirs].position;
        let behind = if cage(&room, team)[0] > her[0] {
            -5.0
        } else {
            5.0
        };
        hold_at_gunpoint(&mut room, 0, her, behind);
        room.tick(t);
        t += 0.1;
    }
    let walked = flat(room.players[&theirs].position, start);
    assert!(walked > 3.5 * 1.1 * 3.8, "faster: {walked}");
    // We kill her: back home, normal speed again.
    room.players.get_mut(&0).unwrap().weapon_type = 160;
    kill(&mut room, theirs, 0);
    run(&mut room, t, t + 5.0);
    assert_eq!(room.players[&theirs].position, home);
    assert_eq!(room.princess.princesses[i].stacks, 0);
}

#[test]
fn kills_earn_a_helicopter_smoke_grenade() {
    let mut room = started(0);
    let team = room.players[&0].group;
    for n in 0..3 {
        join_late_bot(&mut room);
        let bot = *room
            .bots
            .keys()
            .find(|b| room.players[b].alive)
            .expect("a living bot");
        let _ = n;
        let out = kill(&mut room, bot, 0);
        let loot = events(&out, EventCode::PlayerLootRecieved);
        if n < 2 {
            assert!(loot.is_empty(), "kill {n}");
        } else {
            assert_eq!(loot.len(), 1, "the third kill in a team of three");
            assert_eq!(room.carried(0, 208), 1, "a smoke grenade");
            let chat = events(&out, EventCode::ChatMessage);
            assert!(!chat.is_empty(), "You can now spawn a helicopter");
        }
    }
    assert_eq!(room.princess.tokens.get(&0), Some(&1));
    // Throwing it without FlyingControls: no helicopter, the grenade comes back.
    let throw = tabg_protocol::actions::ThrowRequest {
        player_index: 0,
        item_id: 208,
        quantity: 1,
        position: room.players[&0].position,
        direction: [0.0, 0.3, 1.0],
    };
    room.handle(0, &Packet::new(EventCode::RequestItemThrow, throw.encode()));
    let out = run(&mut room, 2.0, 6.0);
    assert_eq!(room.princess.tokens.get(&0), Some(&1), "refunded");
    assert!(!events(&out, EventCode::PlayerLootRecieved).is_empty());
    let _ = team;
}

#[test]
fn turrets_shoot_down_enemy_helicopters() {
    let mut s = settings(1);
    let rules = s.princess.as_mut().unwrap();
    rules.bases[0].turrets = vec![[-300.0, 125.0, 0.0]];
    rules.bases[1].turrets = vec![[300.0, 125.0, 0.0]];
    s.vehicles.types = vec![crate::vehicles::VehicleType {
        car_type: 4,
        name: "Heli".into(),
        seats: vec![(0, true), (1, false)],
        parts: vec![tabg_protocol::messages::CarPart {
            index: 0,
            health: 1000.0,
            name: "Chassi".into(),
        }],
        hull: crate::vehicles::Hull::new(&[[-1.0, -1.3, -2.4, 1.0, 1.1, 2.4]]),
    }];
    let mut room = Room::new(s);
    join(&mut room, 0, "pilot");
    run(&mut room, 0.0, 1.5);
    let team = room.players[&0].group;
    // A helicopter of ours hovers 40 m above the other base, 100 m from its turret.
    let enemy_turret = room.princess_rules().bases[usize::from(1 - team)].turrets[0];
    let at = [enemy_turret[0] - 100.0, 170.0, enemy_turret[2]];
    room.cars.insert(
        50,
        crate::cars::Car::spawn(&crate::cars::CarSpawn {
            car_type: 4,
            car_index: 50,
            position: at,
            rotation: [0.0, 0.0, 0.0, 1.0],
            seats: vec![(0, true)],
            parts: vec![],
        }),
    );
    room.track_aircraft(50, "Heli", 0.0);
    room.players.get_mut(&0).unwrap().position = at;
    room.players.get_mut(&0).unwrap().seat = Some((50, 0, true));
    room.cars.get_mut(&50).unwrap().seats[0].occupant = Some(0);
    room.vehicles
        .aircraft
        .get_mut(&50)
        .unwrap()
        .life
        .set_grounded(false);
    let mut fired = 0;
    let mut t = 2.0;
    let hello = {
        let mut payload = 0x5647_4854u32.to_le_bytes().to_vec();
        payload.extend([2, 0]);
        Packet { code: 245, payload }
    };
    while t < 40.0 && room.cars.contains_key(&50) {
        room.handle(0, &hello);
        room.players.get_mut(&0).unwrap().position = at;
        let out = room.tick(t);
        fired += out
            .iter()
            .filter(|o| o.packet.code == 245 && o.packet.payload.get(5) == Some(&7))
            .count()
            .min(1);
        t += 1.0 / 30.0;
    }
    assert!(!room.cars.contains_key(&50), "shot down");
    assert!(t > 2.0 + 8.0, "it takes two missiles, 8 s apart ({t})");
    assert!(fired > 0);
}

/// A match with one idle player on each team: the bots do all the work.
fn bots_only(team_size: u8, skill: BotSkill, seed: u64) -> Room {
    let mut s = settings(team_size);
    s.princess.as_mut().unwrap().bot_skill = skill;
    s.seed = seed;
    let mut room = Room::new(s);
    join(&mut room, 0, "idle castle");
    join(&mut room, 1, "idle city");
    run(&mut room, 0.0, 1.5);
    assert_ne!(room.players[&0].group, room.players[&1].group);
    room
}

/// Run until someone wins (or `until`), keeping the idle players far away. Returns the winner
/// and how far from home each princess got.
fn play(room: &mut Room, from: f32, until: f32) -> (Option<u8>, [f32; 2]) {
    let mut t = from;
    let mut furthest = [0.0f32; 2];
    while t < until {
        for (i, x) in [(0u8, -900.0), (1, 900.0)] {
            if let Some(p) = room.players.get_mut(&i) {
                p.position = [x, 120.0, 900.0];
            }
        }
        let out = room.tick(t);
        for pr in &room.princess.princesses {
            let f = &mut furthest[usize::from(pr.team)];
            *f = f.max(flat(room.players[&pr.index].position, pr.home));
        }
        if let Some(o) = events(&out, EventCode::GameStateChanged)
            .into_iter()
            .find(|o| o.packet.payload.first() == Some(&game_state::ENDED))
        {
            return (Some(o.packet.payload[1]), furthest);
        }
        t += 0.1;
    }
    (None, furthest)
}

#[test]
fn bots_cage_a_princess_nobody_defends() {
    for skill in [BotSkill::Easy, BotSkill::Normal, BotSkill::Hard] {
        cage_undefended(skill);
    }
}

fn cage_undefended(skill: BotSkill) {
    let mut room = bots_only(3, skill, 5);
    // The city team's bots leave: only its idle player is left.
    let city = room.players[&1].group;
    let gone: Vec<u8> = room
        .bots
        .keys()
        .copied()
        .filter(|b| room.players[b].group == city)
        .collect();
    for b in gone {
        room.bots.remove(&b);
        room.disconnect(b);
    }
    let (winner, furthest) = play(&mut room, 1.6, 900.0);
    assert_eq!(
        winner,
        Some(1 - city),
        "{skill:?}: they walk her home ({furthest:?})"
    );
}

#[test]
fn bots_bring_their_princess_back_home() {
    let mut room = bots_only(3, BotSkill::Normal, 9);
    let team = room.players[&0].group;
    let mine = princess(&room, team);
    let i = room.princess_of(mine).unwrap();
    let home = room.princess.princesses[i].home;
    // She was left out in the open, 150 m from home; the other team's bots stay away.
    let away = [
        home[0] + if home[0] < 0.0 { 150.0 } else { -150.0 },
        120.0,
        home[2],
    ];
    room.players.get_mut(&mine).unwrap().position = away;
    let others: Vec<u8> = room
        .bots
        .keys()
        .copied()
        .filter(|b| room.players[b].group != team)
        .collect();
    for b in others {
        room.bots.remove(&b);
        room.disconnect(b);
    }
    let mut t = 1.6;
    while t < 150.0 && flat(room.players[&mine].position, home) > 8.0 {
        room.tick(t);
        t += 0.1;
    }
    assert!(
        flat(room.players[&mine].position, home) <= 8.0,
        "home after {t:.0} s: {:?}",
        room.players[&mine].position
    );
}

#[test]
fn bots_attack_defend_and_escort_in_a_full_match() {
    // Three bots a side, nobody else. Respawning defenders are strong, so a match can stall;
    // over a few matches normal bots get a princess all the way within 40 minutes.
    let wins = [11u64, 19, 13]
        .into_iter()
        .filter(|&seed| {
            let mut room = bots_only(4, BotSkill::Normal, seed);
            matches!(play(&mut room, 1.6, 2400.0).0, Some(0 | 1))
        })
        .count();
    assert!(wins >= 1, "normal bots cage a princess in some match");
    // Easy bots tend to meet in the middle and trade kills there; hard ones still get in.
    let mut room = bots_only(4, BotSkill::Hard, 3);
    let (winner, furthest) = play(&mut room, 1.6, 1800.0);
    assert!(
        furthest.iter().any(|&f| f > 20.0) || winner.is_some(),
        "hard bots take a princess ({furthest:?})"
    );
}

/// The real island (Actual Castle against New Castle) with bots only, to watch them find their way:
/// `TABG_NAV=<gunzipped world.nav> cargo test -p tabg-game island_bots -- --ignored --nocapture`.
#[test]
#[ignore = "needs the island's walkability grid (TABG_NAV, gunzipped), from your own game files"]
fn island_bots() {
    let path = std::env::var("TABG_NAV").expect("TABG_NAV");
    let nav = crate::nav::NavGrid::decode(&std::fs::read(path).expect("read")).expect("grid");
    let size: u8 = std::env::var("SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let minutes: f32 = std::env::var("MINUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);
    let mut s = settings(size);
    s.nav = Some(std::sync::Arc::new(nav));
    s.princess = Some(PrincessRules {
        team_size: size,
        ..PrincessRules::island()
    });
    s.seed = std::env::var("SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut room = Room::new(s);
    join(&mut room, 0, "idle castle");
    join(&mut room, 1, "idle city");
    run(&mut room, 0.0, 1.5);
    let mut t = 1.6;
    let mut furthest = [0.0f32; 2];
    let mut escorts: Vec<Option<(u8, u8)>> = vec![None, None];
    while t < minutes * 60.0 {
        for (i, at) in [(0u8, [-860.0, 120.0, 700.0]), (1, [-860.0, 120.0, 690.0])] {
            if let Some(p) = room.players.get_mut(&i) {
                p.position = at;
            }
        }
        let out = room.tick(t);
        for pr in &room.princess.princesses {
            let f = &mut furthest[usize::from(pr.team)];
            *f = f.max(flat(room.players[&pr.index].position, pr.home));
        }
        for o in events(&out, EventCode::PlayerDead) {
            let v = o.packet.payload[0];
            let p = &room.players[&v];
            let k = room
                .players
                .get(&p.last_attacker)
                .map_or("?".to_string(), |k| k.name.clone());
            println!(
                "{t:6.1} DEAD {:14} g{} at ({:.0},{:.0},{:.0}) by {k}",
                p.name, p.group, p.position[0], p.position[1], p.position[2]
            );
        }
        for pr in room.princess.princesses.clone() {
            let e = pr.escort.map(|e| (e.team, e.by));
            if e != escorts[usize::from(pr.team)] {
                let pos = room.players[&pr.index].position;
                println!(
                    "{t:6.1} princess {} escort {:?} at ({:.0},{:.0},{:.0}), {:.0} m from home",
                    pr.team,
                    e,
                    pos[0],
                    pos[1],
                    pos[2],
                    flat(pos, pr.home)
                );
                escorts[usize::from(pr.team)] = e;
            }
        }
        if (t * 10.0).round() as i32 % 300 == 0 {
            let line: Vec<String> = room
                .bots
                .keys()
                .map(|b| {
                    let p = &room.players[b];
                    format!(
                        "{}:g{}({:.0},{:.0},{:.0}){}",
                        &p.name[4..],
                        p.group,
                        p.position[0],
                        p.position[1],
                        p.position[2],
                        if p.alive { "" } else { "✝" }
                    )
                })
                .collect();
            println!("{t:6.0} {}", line.join(" "));
        }
        if let Some(o) = events(&out, EventCode::GameStateChanged)
            .into_iter()
            .find(|o| o.packet.payload.first() == Some(&game_state::ENDED))
        {
            println!("{t:.0} WON by {}", o.packet.payload[1]);
            break;
        }
        t += 0.1;
    }
    println!("furthest {furthest:?}");
}

fn princess_packet(op: u8, body: &[u8]) -> Packet {
    let mut payload = b"PRNC".to_vec();
    payload.extend([1, op]);
    payload.extend_from_slice(body);
    Packet {
        code: super::princess::PRINCESS,
        payload,
    }
}

fn offers(out: &[Outgoing]) -> Vec<&Outgoing> {
    out.iter()
        .filter(|o| o.packet.code == super::princess::PRINCESS && o.packet.payload[5] == 4)
        .collect()
}

#[test]
fn players_choose_a_loadout_after_spawning() {
    let mut room = Room::new(settings(0));
    join(&mut room, 0, "human");
    room.handle(0, &princess_packet(1, &[]));
    let out = run(&mut room, 0.0, 1.5);
    // With the mod: three to choose from, nothing granted yet.
    let offer = offers(&out);
    assert_eq!(offer.len(), 1);
    assert_eq!(offer[0].packet.payload[6], 3, "three loadouts");
    assert!(events(&out, EventCode::PlayerLootRecieved).is_empty());
    let out = room.handle(0, &princess_packet(5, &[1]));
    assert_eq!(
        events(&out, EventCode::PlayerLootRecieved).len(),
        1,
        "the second one"
    );
    assert_eq!(offers(&out)[0].packet.payload[6], 0, "menu closed");
    assert!(
        room.handle(0, &princess_packet(5, &[0])).is_empty(),
        "only once"
    );

    // Dying: a new offer on respawn, and the first choice when the time runs out.
    let out = {
        room.handle(0, &princess_packet(1, &[]));
        room.players.get_mut(&0).unwrap().last_attacker = NOBODY;
        let mut out = room.kill(0, false);
        let mut t = 2.0;
        while t < 12.0 {
            room.handle(0, &princess_packet(1, &[]));
            out.extend(room.tick(t));
            t += 0.5;
        }
        out
    };
    assert_eq!(offers(&out).len(), 1, "offered again after the respawn");
    let mut out = Vec::new();
    let mut t = 12.0;
    while t < 12.0 + super::princess_loadouts::OFFER_SECONDS + 1.0 {
        room.handle(0, &princess_packet(1, &[]));
        out.extend(room.tick(t));
        t += 0.5;
    }
    assert_eq!(
        events(&out, EventCode::PlayerLootRecieved).len(),
        1,
        "timed out: the first"
    );

    // Back home after a while away: another choice.
    let team = room.players[&0].group;
    let home = room.princess_rules().bases[usize::from(team)].centre;
    room.players.get_mut(&0).unwrap().position = [home[0], home[1], home[2] + 500.0];
    let mut out = Vec::new();
    for _ in 0..80 {
        room.handle(0, &princess_packet(1, &[]));
        out.extend(room.tick(t));
        t += 0.5;
    }
    assert!(offers(&out).is_empty(), "nothing while away");
    room.players.get_mut(&0).unwrap().position = home;
    room.handle(0, &princess_packet(1, &[]));
    let out = room.tick(t);
    assert_eq!(offers(&out).len(), 1, "back home after 40 s away");
}
