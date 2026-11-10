//! The FlyingControls client mod's vehicles (our event 245, `THGV` v2), replacing the old
//! UnusedVehicles server plugin: the game's unused vehicle types (helicopter, UFO, hover bikes,
//! cars) join the map's cars, admins spawn more with `/vehicle`, and aircraft get their own
//! health, crash and explode, hand over to a passenger when the pilot leaves, and fall when
//! nobody flies them. Missiles and FPV drones are in `vehicles_combat.rs`, hover taxis in
//! `taxi.rs`.

use std::collections::BTreeMap;

use tabg_protocol::loot::chunk_of;
use tabg_protocol::messages::PackedQuaternion;
use tabg_protocol::vehicles::{SEAT_GET_IN, SEAT_GET_OUT, SeatRequest};
use tabg_protocol::{Packet, Reader, Writer};

use super::{Outgoing, Recipients, Room};
use crate::cars::{Car, CarSpawn};
use crate::vehicles::{
    self as rules, AircraftLife, CRASH_DAMAGE, CRASH_SPEED, V3, VehicleType, add, euler,
    euler_angles, length, scale, sub,
};

pub const VEHICLES: u8 = 245;
const MAGIC: u32 = 0x5647_4854;
const VERSION: u8 = 2;
pub(super) mod kind {
    pub const HELLO: u8 = 0;
    pub const SPAWN: u8 = 1;
    pub const ACK: u8 = 2;
    pub const CRASH: u8 = 3;
    pub const DETONATE: u8 = 4;
    pub const MISSILE_AIM: u8 = 5;
    pub const MISSILE_FIRE: u8 = 6;
    pub const MISSILE_STATE: u8 = 7;
    pub const MISSILE_REMOVE: u8 = 8;
    pub const MISSILE_HIT: u8 = 9;
    pub const MISSILE_REPLY: u8 = 10;
    pub const DRONE_LAUNCH: u8 = 11;
    pub const DRONE_STATE: u8 = 12;
    pub const DRONE_MOVE: u8 = 13;
    pub const DRONE_END: u8 = 14;
    pub const DRONE_FINISH: u8 = 15;
    pub const DRONE_DENIED: u8 = 16;
    pub const DRONE_HIT: u8 = 17;
    pub const AIRCRAFT_FALL: u8 = 18;
    pub const EJECT_REQUEST: u8 = 19;
    pub const EJECT_LAUNCH: u8 = 20;
    pub const PILOT_HANDOVER: u8 = 21;
    pub const EJECT_SHIELD: u8 = 22;
    pub const BLAST_CONFIRMED: u8 = 23;
    pub const TAXI_DESTINATION: u8 = 24;
    pub const TAXI_BOOST: u8 = 25;
    pub const TAXI_STATUS: u8 = 26;
    pub const AIRCRAFT_HEALTH: u8 = 27;
}
/// A client counts as a FlyingControls client this long after its last hello (sent every 3 s).
const CLIENT_TIMEOUT: f32 = 12.0;
const SPAWN_COOLDOWN: f32 = 3.0;
const MAX_CUSTOM: usize = 16;
/// Types the game can't spawn outside their mode.
const BROKEN: &[&str] = &["CannonCar", "DeceptionBossCar"];
const GRAVITY: f32 = 9.81;

/// Vehicle rules from the map data and `tabg.toml`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleRules {
    pub types: Vec<VehicleType>,
    /// Chance per map car to add an unused vehicle type near it (0 = off).
    pub unused_chance: f32,
    pub unused_max: usize,
    /// The road graph hover taxis drive on.
    pub roads: Option<std::sync::Arc<crate::roads::RoadGraph>>,
}

/// One aircraft's flight state on the server.
#[derive(Clone, Debug)]
pub(super) struct Aircraft {
    pub life: AircraftLife,
    previous: V3,
    pub velocity: V3,
    sample: f32,
    send_at: f32,
    ground_check: f32,
    ground_since: Option<f32>,
    health_sync: f32,
    pilot: Option<u8>,
    falling: bool,
    settled: bool,
}

#[derive(Debug, Default)]
pub(super) struct VehicleState {
    /// FlyingControls clients: last hello.
    pub clients: BTreeMap<u8, f32>,
    /// `/vehicle` spawns (only FlyingControls clients know them).
    pub custom: Vec<i32>,
    pending: BTreeMap<i32, u8>,
    last_spawn: BTreeMap<u8, f32>,
    pub aircraft: BTreeMap<i32, Aircraft>,
}

pub(super) fn message(kind: u8) -> Writer {
    let mut w = Writer::new();
    w.u32(MAGIC).u8(VERSION).u8(kind);
    w
}

/// .NET `BinaryWriter.Write(string)`: 7-bit length, then UTF-8.
pub(super) fn dotnet_string(w: &mut Writer, s: &str) {
    let mut n = s.len();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            w.u8(byte);
            break;
        }
        w.u8(byte | 0x80);
    }
    w.bytes(s.as_bytes());
}

pub(super) fn read_v3(r: &mut Reader) -> Option<V3> {
    let v = [r.f32().ok()?, r.f32().ok()?, r.f32().ok()?];
    rules::finite(v).then_some(v)
}

fn packet(w: Writer) -> Packet {
    Packet {
        code: VEHICLES,
        payload: w.into_inner(),
    }
}

impl Room {
    /// Players running FlyingControls (they said hello recently).
    pub(super) fn vehicle_clients(&self) -> Vec<u8> {
        let now = self.last_now.unwrap_or(0.0);
        self.vehicles
            .clients
            .iter()
            .filter(|&(p, &seen)| now - seen < CLIENT_TIMEOUT && self.players.contains_key(p))
            .map(|(&p, _)| p)
            .collect()
    }

    pub(super) fn to_vehicle_clients(&self, w: Writer) -> Vec<Outgoing> {
        let to = self.vehicle_clients();
        if to.is_empty() {
            return Vec::new();
        }
        vec![Outgoing {
            to: Recipients::Many(to),
            packet: packet(w),
            reliable: true,
        }]
    }

    pub(super) fn to_one(peer: u8, w: Writer, reliable: bool) -> Outgoing {
        Outgoing {
            to: Recipients::One(peer),
            packet: packet(w),
            reliable,
        }
    }

    pub(super) fn vehicle_type(&self, car_type: i32) -> Option<&VehicleType> {
        self.settings
            .vehicles
            .types
            .iter()
            .find(|t| t.car_type == car_type)
    }

    pub(super) fn car_name(&self, car: i32) -> &str {
        self.cars
            .get(&car)
            .and_then(|c| self.vehicle_type(c.car_type))
            .map_or("", |t| t.name.as_str())
    }

    /// May `peer` be told about this car? `/vehicle` spawns exist only for FlyingControls
    /// clients (a vanilla client never got their spawn).
    pub(super) fn car_visible_to(&self, car: i32, peer: u8) -> bool {
        !self.vehicles.custom.contains(&car) || self.vehicles.clients.contains_key(&peer)
    }

    pub(super) fn vehicle_message(&mut self, peer: u8, payload: &[u8]) -> Vec<Outgoing> {
        if payload.len() < 6 || payload.len() > 4096 {
            return Vec::new();
        }
        let mut r = Reader::new(payload);
        if r.u32().ok() != Some(MAGIC) || r.u8().ok() != Some(VERSION) {
            return Vec::new();
        }
        let Ok(k) = r.u8() else {
            return Vec::new();
        };
        if !self.players.contains_key(&peer) {
            return Vec::new();
        }
        let now = self.last_now.unwrap_or(0.0);
        let body = &payload[6..];
        match k {
            kind::HELLO if body.is_empty() => {
                self.vehicles.clients.insert(peer, now);
                Vec::new()
            }
            kind::ACK if body.len() == 4 => {
                let index = i32::from_le_bytes(body.try_into().expect("4 bytes"));
                if self.vehicles.pending.get(&index) == Some(&peer) {
                    self.vehicles.pending.remove(&index);
                    return vec![Self::chat_line(
                        peer,
                        &format!("New vehicle #{index} spawned. Walk to it and get in."),
                    )];
                }
                Vec::new()
            }
            kind::CRASH if body.len() == 8 => {
                let index = i32::from_le_bytes(body[..4].try_into().expect("4 bytes"));
                let impact = f32::from_le_bytes(body[4..].try_into().expect("4 bytes"));
                self.aircraft_crash(peer, index, impact)
            }
            kind::EJECT_REQUEST if body.len() == 4 => {
                let index = i32::from_le_bytes(body.try_into().expect("4 bytes"));
                self.eject_request(peer, index)
            }
            kind::MISSILE_AIM | kind::MISSILE_FIRE | kind::MISSILE_HIT => {
                self.missile_message(peer, k, body)
            }
            kind::DRONE_LAUNCH | kind::DRONE_MOVE | kind::DRONE_END | kind::DRONE_HIT => {
                self.drone_message(peer, k, body)
            }
            kind::TAXI_DESTINATION | kind::TAXI_BOOST => self.taxi_message(peer, k, body),
            _ => Vec::new(),
        }
    }

    /// Unused vehicle types near the map's cars (a new match).
    pub(super) fn spawn_unused_vehicles(&mut self) {
        self.vehicles.custom.clear();
        self.vehicles.pending.clear();
        self.vehicles.aircraft.clear();
        self.reset_vehicle_combat();
        self.taxis = Default::default();
        let rules = self.settings.vehicles.clone();
        let base: Vec<CarSpawn> = self.settings.cars.clone();
        let map_types: Vec<i32> = base.iter().map(|c| c.car_type).collect();
        let unused: Vec<&VehicleType> = rules
            .types
            .iter()
            .filter(|t| !map_types.contains(&t.car_type) && !BROKEN.contains(&t.name.as_str()))
            .collect();
        if rules.unused_chance <= 0.0 || unused.is_empty() {
            return;
        }
        let mut added = 0;
        for spawn in &base {
            if added >= rules.unused_max {
                break;
            }
            if self.rng.unit() > rules.unused_chance {
                continue;
            }
            let t = unused[self.rng.index(unused.len())];
            let mut at = add(
                spawn.position,
                [
                    self.rng.range(-15.0, 15.0),
                    0.0,
                    self.rng.range(-15.0, 15.0),
                ],
            );
            match self
                .settings
                .nav
                .as_ref()
                .and_then(|n| n.height(at[0], at[2]))
            {
                Some(ground) => at[1] = ground + t.hull.bottom() + 0.5,
                // Over water or without the grid: keep it where the map's car stands.
                None => at = spawn.position,
            }
            let yaw = self.rng.range(0.0, 360.0);
            let index = self.next_car_index();
            let car = Car::spawn(&CarSpawn {
                car_type: t.car_type,
                car_index: index,
                position: at,
                rotation: euler([0.0, yaw, 0.0]),
                seats: t.seats.clone(),
                parts: t.parts.clone(),
            });
            self.cars.insert(index, car);
            added += 1;
        }
        let now = self.last_now.unwrap_or(0.0);
        let aircraft: Vec<(i32, String)> = self
            .cars
            .values()
            .map(|c| (c.index, self.car_name(c.index).to_owned()))
            .filter(|(_, n)| rules::is_aircraft(n))
            .collect();
        for (index, name) in aircraft {
            self.track_aircraft(index, &name, now);
        }
    }

    fn next_car_index(&self) -> i32 {
        self.cars.keys().max().map_or(0, |m| m + 1)
    }

    pub(super) fn track_aircraft(&mut self, index: i32, name: &str, now: f32) {
        let position = self.cars.get(&index).map_or([0.0; 3], |c| c.position);
        let mut life = AircraftLife::new(now);
        if rules::is_heli(name) {
            life.set_grounded(true);
        }
        self.vehicles.aircraft.insert(
            index,
            Aircraft {
                life,
                previous: position,
                velocity: [0.0; 3],
                sample: now,
                send_at: 0.0,
                ground_check: 0.0,
                ground_since: None,
                health_sync: 0.0,
                pilot: None,
                falling: false,
                settled: false,
            },
        );
    }

    /// `/vehicle <name>`: a new vehicle on clear ground in front of the admin.
    pub(super) fn vehicle_command(&mut self, peer: u8, query: &str) -> (Vec<Outgoing>, String) {
        let usable: Vec<&VehicleType> = self
            .settings
            .vehicles
            .types
            .iter()
            .filter(|t| !BROKEN.contains(&t.name.as_str()))
            .collect();
        if usable.is_empty() {
            return (Vec::new(), "no vehicle data; run tabg setup again".into());
        }
        let names = || {
            usable
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if query.is_empty() {
            return (Vec::new(), format!("vehicles: {}", names()));
        }
        let exact = usable.iter().find(|t| t.name.eq_ignore_ascii_case(query));
        let matching: Vec<&&VehicleType> = usable
            .iter()
            .filter(|t| {
                t.name
                    .to_ascii_lowercase()
                    .contains(&query.to_ascii_lowercase())
            })
            .collect();
        let Some(t) = exact.or(if matching.len() == 1 {
            Some(matching[0])
        } else {
            None
        }) else {
            return (
                Vec::new(),
                format!("no single vehicle {query:?}; vehicles: {}", names()),
            );
        };
        let t = (*t).clone();
        let now = self.last_now.unwrap_or(0.0);
        if !self.vehicle_clients().contains(&peer) {
            return (
                Vec::new(),
                "spawning vehicles needs the flying-controls mod (restart TABG with it and wait 3 s)"
                    .into(),
            );
        }
        if self
            .vehicles
            .last_spawn
            .get(&peer)
            .is_some_and(|&t| now - t < SPAWN_COOLDOWN)
        {
            return (Vec::new(), "wait 3 seconds between spawns".into());
        }
        self.vehicles.custom.retain(|i| self.cars.contains_key(i));
        if self.vehicles.custom.len() >= MAX_CUSTOM {
            return (Vec::new(), format!("at most {MAX_CUSTOM} extra vehicles"));
        }
        let Some(p) = self.players.get(&peer).filter(|p| p.alive) else {
            return (Vec::new(), "you need to be alive".into());
        };
        let (me, yaw) = (p.position, p.rotation[1]);
        let Some(position) = self.find_space(me, yaw, &t) else {
            return (
                Vec::new(),
                "no clear, flat ground nearby; move to an open area".into(),
            );
        };
        let out = self.spawn_custom(peer, &t, position, yaw);
        let d = length(sub(position, me));
        (out, format!("spawning a new {} {d:.0} m away", t.name))
    }

    /// A vehicle of type `name` on clear, flat ground around `at` (Capture the Princess calls
    /// in helicopters this way). Only FlyingControls clients see it.
    pub(super) fn spawn_vehicle_near(
        &mut self,
        owner: u8,
        name: &str,
        at: V3,
        yaw: f32,
    ) -> Result<Vec<Outgoing>, String> {
        let t = self
            .settings
            .vehicles
            .types
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .cloned()
            .ok_or("no vehicle data; run tabg setup again")?;
        if !self.vehicle_clients().contains(&owner) {
            return Err("flying needs the flying-controls mod".into());
        }
        if !self.players.get(&owner).is_some_and(|p| p.alive) {
            return Err("you need to be alive".into());
        }
        self.vehicles.custom.retain(|i| self.cars.contains_key(i));
        if self.vehicles.custom.len() >= MAX_CUSTOM {
            return Err(format!("at most {MAX_CUSTOM} extra vehicles"));
        }
        // Around the smoke first, then in front of the thrower.
        let back = add(at, scale(euler_forward(yaw), -10.0));
        let position = self
            .find_space(back, yaw, &t)
            .or_else(|| {
                let me = self.players.get(&owner).map_or(at, |p| p.position);
                self.find_space(me, yaw, &t)
            })
            .ok_or("no clear, flat ground there; throw it onto an open area")?;
        Ok(self.spawn_custom(owner, &t, position, yaw))
    }

    fn spawn_custom(&mut self, peer: u8, t: &VehicleType, position: V3, yaw: f32) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        let index = self.next_car_index();
        let rotation = euler([0.0, yaw, 0.0]);
        let mut car = Car::spawn(&CarSpawn {
            car_type: t.car_type,
            car_index: index,
            position,
            rotation,
            seats: t.seats.clone(),
            parts: t.parts.clone(),
        });
        car.temporary_owner = Some((peer, now + 3.0));
        // Spawn: [i32 index][i32 type][u8 owner][position][f32 yaw][u8 parts]([u8][f32][string])*
        let mut w = message(kind::SPAWN);
        w.i32(index)
            .i32(t.car_type)
            .u8(peer)
            .vec3(position)
            .f32(yaw);
        w.u8(t.parts.len().min(255) as u8);
        for part in t.parts.iter().take(255) {
            w.u8(part.index).f32(part.health);
            dotnet_string(&mut w, &part.name);
        }
        self.cars.insert(index, car);
        self.vehicles.custom.push(index);
        self.vehicles.pending.insert(index, peer);
        self.vehicles.last_spawn.insert(peer, now);
        if rules::is_aircraft(&t.name) {
            self.track_aircraft(index, &t.name, now);
        }
        self.to_vehicle_clients(w)
    }

    /// `/vehicles`: what's on the map.
    pub(super) fn vehicles_list(&self) -> String {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for c in self.cars.values() {
            *counts.entry(self.car_name(c.index)).or_default() += 1;
        }
        let list: Vec<String> = counts
            .iter()
            .map(|(n, c)| format!("{}({c})", if n.is_empty() { "?" } else { n }))
            .collect();
        format!("vehicles: {}", list.join(" "))
    }

    /// Flat, free ground in front of `me` (8–22 m, five angles), like the old plugin.
    fn find_space(&self, me: V3, yaw: f32, t: &VehicleType) -> Option<V3> {
        let (ex, ez) = t.hull.extents();
        let radius = ex.max(ez);
        let nav = self.settings.nav.as_ref();
        for distance in [
            (radius + 4.0).max(8.0),
            (radius + 10.0).max(14.0),
            (radius + 18.0).max(22.0),
        ] {
            for angle in [0.0, -35.0, 35.0, -70.0, 70.0] {
                let a = (yaw + angle).to_radians();
                let (x, z) = (me[0] + a.sin() * distance, me[2] + a.cos() * distance);
                let ground = match nav {
                    Some(n) => {
                        let Some(g) = n.height(x, z) else { continue };
                        // Footprint corners on the same level (no slopes, walls or roofs).
                        let flat = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
                            .iter()
                            .all(|&(sx, sz)| {
                                n.height(x + sx * ex * 0.7, z + sz * ez * 0.7)
                                    .is_some_and(|h| (h - g).abs() <= 0.8)
                            });
                        if !flat {
                            continue;
                        }
                        g
                    }
                    None => me[1],
                };
                let at = [x, ground + t.hull.bottom() + 0.6, z];
                let clear = self
                    .cars
                    .values()
                    .all(|c| length(sub(c.position, at)) >= radius + 5.0);
                if clear {
                    return Some(at);
                }
            }
        }
        None
    }

    /// A pilot got in (aircraft: start protection).
    pub(super) fn vehicle_seated(&mut self, car: i32, driver: bool) {
        let now = self.last_now.unwrap_or(0.0);
        if driver && let Some(a) = self.vehicles.aircraft.get_mut(&car) {
            a.life.start(now);
        }
    }

    /// Car damage for an aircraft: the FlyingControls client reports the damage itself (not the
    /// part's new health), and the aircraft's own health decides.
    pub(super) fn aircraft_damage(
        &mut self,
        attacker: u8,
        car: i32,
        amount: f32,
    ) -> Option<Vec<Outgoing>> {
        let now = self.last_now.unwrap_or(0.0);
        let a = self.vehicles.aircraft.get_mut(&car)?;
        let before = a.life.health;
        if a.life.damage(amount, now) {
            return Some(self.detonate(car, attacker));
        }
        Some(if a.life.health != before {
            self.aircraft_health(car)
        } else {
            Vec::new()
        })
    }

    /// `AircraftHealth` to FlyingControls clients, and every part at the aircraft's health
    /// (vanilla `CarDamage`) to those near it.
    fn aircraft_health(&mut self, car: i32) -> Vec<Outgoing> {
        let Some(a) = self.vehicles.aircraft.get(&car) else {
            return Vec::new();
        };
        let (health, maximum) = (a.life.health, a.life.maximum);
        let mut w = message(kind::AIRCRAFT_HEALTH);
        w.i32(car).f32(health).f32(maximum);
        let mut out = self.to_vehicle_clients(w);
        if let Some(c) = self.cars.get_mut(&car) {
            for part in &mut c.parts {
                part.health = health;
            }
            let parts: Vec<u8> = c.parts.iter().map(|p| p.index).collect();
            let chunk = chunk_of(c.position);
            let to = self.watchers(chunk);
            if !to.is_empty() {
                for part in parts {
                    out.push(Outgoing::reliable(
                        Recipients::Many(to.clone()),
                        tabg_protocol::EventCode::CarDamage,
                        tabg_protocol::vehicles::encode_car_damage(part, car, health),
                    ));
                }
            }
        }
        out
    }

    /// `Crash` from the pilot: a hard landing destroys the aircraft.
    fn aircraft_crash(&mut self, peer: u8, index: i32, impact: f32) -> Vec<Outgoing> {
        let driving = self
            .players
            .get(&peer)
            .and_then(|p| p.seat)
            .is_some_and(|(c, _, driver)| c == index && driver);
        let now = self.last_now.unwrap_or(0.0);
        if !driving || !(CRASH_SPEED..=500.0).contains(&impact) {
            return Vec::new();
        }
        let destroyed = self
            .vehicles
            .aircraft
            .get_mut(&index)
            .is_some_and(|a| a.life.damage(CRASH_DAMAGE, now));
        if destroyed {
            self.detonate(index, peer)
        } else {
            Vec::new()
        }
    }

    /// The aircraft explodes: FlyingControls clients play the blast (sparing the occupants),
    /// and the car is gone.
    pub(super) fn detonate(&mut self, index: i32, attacker: u8) -> Vec<Outgoing> {
        let Some(car) = self.cars.remove(&index) else {
            return Vec::new();
        };
        self.vehicles.aircraft.remove(&index);
        self.vehicles.custom.retain(|&c| c != index);
        let occupants: Vec<u8> = car.seats.iter().filter_map(|s| s.occupant).collect();
        let mut w = message(kind::DETONATE);
        w.i32(index).u8(attacker).vec3(car.position);
        w.u8(occupants.len() as u8);
        for &o in &occupants {
            w.u8(o);
        }
        let out = self.to_vehicle_clients(w);
        for o in occupants {
            if let Some(p) = self.players.get_mut(&o) {
                p.seat = None;
                p.dirty = true;
            }
        }
        out
    }

    /// `EjectRequest`: out of the aircraft with a shield against its explosion.
    fn eject_request(&mut self, peer: u8, index: i32) -> Vec<Outgoing> {
        let Some(p) = self.players.get(&peer).filter(|p| p.alive && !p.downed) else {
            return Vec::new();
        };
        let Some((car, seat, _)) = p.seat.filter(|s| s.0 == index) else {
            return Vec::new();
        };
        if !rules::is_aircraft(self.car_name(car)) {
            return Vec::new();
        }
        let mut w = message(kind::EJECT_SHIELD);
        w.i32(index).u8(peer);
        let mut out = self.to_vehicle_clients(w);
        out.extend(self.seat_action(peer, car, seat, SEAT_GET_OUT));
        if self.players.get(&peer).is_some_and(|p| p.seat.is_none()) {
            let mut w = message(kind::EJECT_LAUNCH);
            w.i32(index);
            out.push(Self::to_one(peer, w, true));
        }
        out
    }

    fn seat_action(&mut self, peer: u8, car: i32, seat: i32, action: u8) -> Vec<Outgoing> {
        self.seat(
            peer,
            SeatRequest {
                player_index: peer,
                car_index: car,
                seat_index: seat,
                action,
            },
        )
    }

    /// Every tick: aircraft health sync, helicopter grounding, pilot handover, and unpiloted
    /// helicopters falling.
    pub(super) fn tick_aircraft(&mut self, now: f32, dt: f32) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let indices: Vec<i32> = self.vehicles.aircraft.keys().copied().collect();
        for index in indices {
            if !self.cars.contains_key(&index) {
                self.vehicles.aircraft.remove(&index);
                continue;
            }
            out.extend(self.tick_one_aircraft(index, now, dt));
        }
        out
    }

    fn tick_one_aircraft(&mut self, index: i32, now: f32, dt: f32) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let heli = rules::is_heli(self.car_name(index));
        let clearance = self
            .cars
            .get(&index)
            .and_then(|c| self.vehicle_type(c.car_type))
            .map_or(0.7, |t| t.hull.ground_clearance);
        let position = self.cars[&index].position;
        if now >= self.vehicles.aircraft[&index].health_sync {
            self.vehicles
                .aircraft
                .get_mut(&index)
                .expect("tracked")
                .health_sync = now + 2.0;
            let a = &self.vehicles.aircraft[&index];
            let mut w = message(kind::AIRCRAFT_HEALTH);
            w.i32(index).f32(a.life.health).f32(a.life.maximum);
            out.extend(self.to_vehicle_clients(w));
        }
        // A helicopter resting on the ground is tough; in the air it isn't.
        if heli && now >= self.vehicles.aircraft[&index].ground_check {
            let ground = self
                .settings
                .nav
                .as_ref()
                .and_then(|n| n.height(position[0], position[2]))
                .unwrap_or(f32::NEG_INFINITY);
            let a = self.vehicles.aircraft.get_mut(&index).expect("tracked");
            a.ground_check = now + 0.25;
            let near = position[1] - ground <= clearance + 0.65 && length(a.velocity) < 5.0;
            let before = a.life.health;
            if !near {
                a.ground_since = None;
                a.life.set_grounded(false);
            } else {
                let since = *a.ground_since.get_or_insert(now);
                if now - since > 0.8 {
                    a.life.set_grounded(true);
                }
            }
            if a.life.health != before {
                out.extend(self.aircraft_health(index));
            }
        }
        // Who's in it; with no pilot, the first passenger takes over.
        let car = &self.cars[&index];
        let mut occupants: Vec<(i32, u8)> = car
            .seats
            .iter()
            .filter_map(|s| s.occupant.map(|o| (s.network_index, o)))
            .filter(|(_, o)| self.players.get(o).is_some_and(|p| p.alive && !p.downed))
            .collect();
        occupants.sort();
        let driver_seat = car.seats.iter().find(|s| s.driver).map(|s| s.network_index);
        let mut driver = car
            .seats
            .iter()
            .find(|s| s.driver)
            .and_then(|s| s.occupant)
            .filter(|o| occupants.iter().any(|(_, x)| x == o));
        if driver.is_none()
            && let (Some(seat), Some(&(from, who))) = (driver_seat, occupants.first())
        {
            out.extend(self.seat_action(who, index, from, SEAT_GET_OUT));
            out.extend(self.seat_action(who, index, seat, SEAT_GET_IN));
            if self.players.get(&who).and_then(|p| p.seat).map(|s| s.1) == Some(seat) {
                driver = Some(who);
                let v = self.vehicles.aircraft[&index].velocity;
                let mut w = message(kind::PILOT_HANDOVER);
                w.i32(index).vec3(v);
                out.push(Self::to_one(who, w, true));
            }
        }
        let a = self.vehicles.aircraft.get_mut(&index).expect("tracked");
        if let Some(pilot) = driver {
            a.pilot = Some(pilot);
            if a.falling {
                a.falling = false;
                a.settled = false;
                out.extend(self.aircraft_fall_state(index));
            }
            let a = self.vehicles.aircraft.get_mut(&index).expect("tracked");
            let elapsed = now - a.sample;
            if position != a.previous && elapsed > 0.001 {
                let v = scale(sub(position, a.previous), 1.0 / elapsed);
                let l = length(v);
                a.velocity = if l > 60.0 { scale(v, 60.0 / l) } else { v };
                a.previous = position;
                a.sample = now;
            } else if elapsed > 0.3 {
                a.velocity = [0.0; 3];
            }
            return out;
        }
        // Nobody flying: a helicopter that was flown falls.
        let Some(pilot) = a.pilot.filter(|_| heli) else {
            return out;
        };
        if !a.falling {
            a.falling = true;
            a.settled = false;
            a.send_at = 0.0;
            if let Some(c) = self.cars.get_mut(&index) {
                c.temporary_owner = None;
            }
        }
        let a = self.vehicles.aircraft.get_mut(&index).expect("tracked");
        if !a.settled && dt > 0.0 {
            a.velocity[1] -= GRAVITY * dt;
            a.velocity = scale(a.velocity, (-0.035 * dt).exp());
            let step = scale(a.velocity, dt);
            let next = add(position, step);
            let ground = self
                .settings
                .nav
                .as_ref()
                .and_then(|n| n.height(next[0], next[2]))
                .unwrap_or(100.0);
            let car = self.cars.get_mut(&index).expect("exists");
            if next[1] - 1.2 <= ground {
                car.position = [next[0], ground + 1.2, next[2]];
                car.temporary_changed = true;
                let speed = a.velocity[1].abs();
                a.velocity = [0.0; 3];
                a.settled = true;
                if speed >= CRASH_SPEED && a.life.damage(CRASH_DAMAGE, now) {
                    return {
                        out.extend(self.detonate(index, pilot));
                        out
                    };
                }
            } else {
                car.position = next;
                let spin = euler([8.0 * dt, 25.0 * dt, 5.0 * dt]);
                let q = car.rotation.unpack();
                car.rotation = PackedQuaternion::pack(quat_mul(q, spin));
                car.temporary_changed = true;
            }
        }
        let a = self.vehicles.aircraft.get_mut(&index).expect("tracked");
        if now >= a.send_at {
            a.send_at = now + 0.05;
            out.extend(self.aircraft_fall_state(index));
        }
        out
    }

    /// `AircraftFall` `[i32 car][bool falling][position][euler]`.
    fn aircraft_fall_state(&self, index: i32) -> Vec<Outgoing> {
        let (Some(car), Some(a)) = (self.cars.get(&index), self.vehicles.aircraft.get(&index))
        else {
            return Vec::new();
        };
        let mut w = message(kind::AIRCRAFT_FALL);
        w.i32(index).u8(u8::from(a.falling)).vec3(car.position);
        w.vec3(euler_angles(car.rotation.unpack()));
        self.to_vehicle_clients(w)
    }

    /// A player left the game: forget their FlyingControls state.
    pub(super) fn forget_vehicle_client(&mut self, peer: u8) {
        self.vehicles.clients.remove(&peer);
        self.vehicles.last_spawn.remove(&peer);
        self.forget_vehicle_combat(peer);
    }

    pub(super) fn is_aircraft_car(&self, car: i32) -> bool {
        self.vehicles.aircraft.contains_key(&car)
    }
}

/// Horizontal unit vector for a yaw in degrees (Unity: 0 = +z).
fn euler_forward(yaw: f32) -> V3 {
    let a = yaw.to_radians();
    [a.sin(), 0.0, a.cos()]
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
