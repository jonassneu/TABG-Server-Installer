//! Helicopter missiles and FPV drones (FlyingControls), ported from the old server plugin.
//! The server flies missiles (guided ones track a locked player or vehicle) and checks drone
//! flights the pilot's client reports; both explode as a native grenade on every client, and
//! the attacker's client reports the damage with a blast receipt, which earns a hit marker.
//! Bots have no client, so the server damages them itself. Walls are the walkability grid's
//! height field; vehicles are their hull boxes.

use std::collections::{BTreeMap, VecDeque};

use tabg_protocol::Reader;
use tabg_protocol::combat::DamageEvent;

use super::vehicles::{kind, message, read_v3};
use super::{Outgoing, Room};
use crate::vehicles::{
    self as rules, Armor, MissileWeapon, V3, add, blast, distance, dot, drone, length, lerp,
    missile, normal, scale, segment_sphere, sub, turn,
};

/// Seconds between missile position updates.
const MISSILE_SEND: f32 = 0.05;
const RECEIPT_MAGIC: i32 = 0x4254_4754;

#[derive(Debug)]
struct Missile {
    id: i32,
    target: i32,
    owner: u8,
    car: Option<i32>,
    position: V3,
    direction: V3,
    guided: bool,
    born: f32,
    armor: Armor,
    history: VecDeque<V3>,
    /// Fired by a base turret of this team (Capture the Princess), not by a player.
    turret: Option<u8>,
}

#[derive(Debug)]
struct Drone {
    id: i32,
    thrown: i32,
    owner: u8,
    launch_car: Option<i32>,
    position: V3,
    euler: V3,
    seen: f32,
    born: f32,
    sequence: i32,
    armor: Armor,
    history: VecDeque<V3>,
}

#[derive(Debug)]
struct ThrowToken {
    time: f32,
    origin: V3,
}

/// A recent explosion that may earn its owner hit markers.
#[derive(Debug)]
struct Blast {
    owner: u8,
    position: V3,
    expires: f32,
    confirmed: Vec<(bool, i32)>,
}

#[derive(Debug, Default)]
pub(super) struct CombatState {
    missiles: Vec<Missile>,
    weapons: BTreeMap<u8, MissileWeapon>,
    aim_rate: BTreeMap<u8, f32>,
    serial: i32,
    next_send: f32,
    drones: BTreeMap<u8, Drone>,
    pending: BTreeMap<u8, ThrowToken>,
    drone_serial: i32,
    /// `(missile?, id)` → blast.
    blasts: BTreeMap<(bool, i32), Blast>,
}

/// A FlyingControls blast receipt at the end of a damage packet: `[i32 magic][bool missile]
/// [i32 id]`. Returns the packet without it.
pub(super) fn strip_receipt(payload: &[u8]) -> Option<(&[u8], bool, i32)> {
    if payload.len() < 19 || payload.len() > 128 {
        return None;
    }
    let at = payload.len() - 9;
    let magic = i32::from_le_bytes(payload[at..at + 4].try_into().ok()?);
    let flag = payload[at + 4];
    let id = i32::from_le_bytes(payload[at + 5..].try_into().ok()?);
    (magic == RECEIPT_MAGIC && flag <= 1 && id > 0).then_some((&payload[..at], flag == 1, id))
}

fn exact(body: &[u8], len: usize) -> Option<Reader<'_>> {
    (body.len() == len).then(|| Reader::new(body))
}

impl Room {
    pub(super) fn reset_vehicle_combat(&mut self) {
        self.combat = CombatState::default();
    }

    pub(super) fn forget_vehicle_combat(&mut self, peer: u8) {
        self.combat.weapons.remove(&peer);
        self.combat.aim_rate.remove(&peer);
        self.combat.pending.remove(&peer);
    }

    fn alive_up(&self, p: u8) -> bool {
        self.players
            .get(&p)
            .is_some_and(|p| p.alive && !p.downed && p.health > 0.0)
    }

    fn car_of(&self, p: u8) -> Option<i32> {
        self.players.get(&p).and_then(|p| p.seat).map(|s| s.0)
    }

    fn driving(&self, p: u8) -> bool {
        self.players
            .get(&p)
            .and_then(|p| p.seat)
            .is_some_and(|s| s.2)
    }

    /// Someone `owner` may hit: alive, another team, not in the same vehicle.
    fn enemy_of(&self, p: u8, owner: u8) -> bool {
        let (Some(a), Some(b)) = (self.players.get(&p), self.players.get(&owner)) else {
            return false;
        };
        a.alive
            && a.health > 0.0
            && p != owner
            && a.group != b.group
            && (a.seat.is_none() || self.car_of(p) != self.car_of(owner))
    }

    /// How far along `direction` from `start` (up to `max`) the way is clear: the height field
    /// and vehicle hulls (except `skip` cars) stop it.
    pub(super) fn wall_distance(
        &self,
        start: V3,
        direction: V3,
        max: f32,
        skip: &[Option<i32>],
        radius: f32,
    ) -> f32 {
        let mut nearest = max;
        if let Some(nav) = self.settings.nav.as_ref() {
            let steps = (max / 0.5).ceil().max(1.0) as usize;
            for i in 1..=steps {
                let d = max * i as f32 / steps as f32;
                let p = add(start, scale(direction, d));
                if nav.height(p[0], p[2]).is_some_and(|h| h > p[1] + radius) {
                    nearest = (d - 0.5).max(0.0);
                    break;
                }
            }
        }
        let end = add(start, scale(direction, max));
        for car in self.cars.values() {
            if skip.contains(&Some(car.index)) {
                continue;
            }
            let Some(t) = self.vehicle_type(car.car_type) else {
                continue;
            };
            if let Some(f) = t
                .hull
                .hit(start, end, car.position, car.rotation.unpack(), radius)
            {
                nearest = nearest.min(max * f);
            }
        }
        nearest
    }

    // ---------------------------------------------------------------- missiles

    fn missile_eligible(&self, p: u8) -> bool {
        self.alive_up(p)
            && self
                .car_of(p)
                .is_some_and(|c| rules::is_heli(self.car_name(c)))
    }

    /// Where a target is: a player's chest, or the vehicle's aim point. With the car it sits in.
    fn missile_target(&self, id: i32, owner: u8) -> Option<(V3, Option<i32>)> {
        if missile::is_vehicle_target(id) {
            let car = self.cars.get(&missile::car_index(id))?;
            if Some(car.index) == self.car_of(owner) {
                return None;
            }
            let hull = &self.vehicle_type(car.car_type)?.hull;
            return Some((
                hull.aim(car.position, car.rotation.unpack()),
                Some(car.index),
            ));
        }
        let p = u8::try_from(id).ok()?;
        if !self.enemy_of(p, owner) {
            return None;
        }
        Some((self.players[&p].position, self.car_of(p)))
    }

    fn can_lock(&self, p: u8, id: i32, direction: V3) -> bool {
        let start = add(self.players[&p].position, [0.0, 0.5, 0.0]);
        let own = self.car_of(p);
        if missile::is_vehicle_target(id) {
            let Some(car) = self
                .cars
                .get(&missile::car_index(id))
                .filter(|c| Some(c.index) != own)
            else {
                return false;
            };
            let Some(t) = self.vehicle_type(car.car_type) else {
                return false;
            };
            return t
                .hull
                .aim_points(start, direction, car.position, car.rotation.unpack())
                .into_iter()
                .any(|aim| {
                    let diff = sub(aim, start);
                    let d = length(diff);
                    d <= missile::RANGE
                        && dot(normal(diff), direction) > 0.95
                        && self.wall_distance(start, normal(diff), d, &[own, Some(car.index)], 0.12)
                            >= d - 0.5
                });
        }
        let Some((point, target_car)) = self.missile_target(id, p) else {
            return false;
        };
        let delta = sub(point, start);
        let d = length(delta);
        d <= missile::RANGE
            && dot(normal(delta), direction) > 0.965
            && self.wall_distance(start, normal(delta), d, &[own, target_car], 0.12) >= d - 0.5
    }

    pub(super) fn missile_message(&mut self, peer: u8, k: u8, body: &[u8]) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        if k == kind::MISSILE_HIT {
            let Some(mut r) = exact(body, 20) else {
                return Vec::new();
            };
            let (Ok(id), Ok(bullet), Some(point)) = (r.i32(), r.i32(), read_v3(&mut r)) else {
                return Vec::new();
            };
            if !self.alive_up(peer) {
                return Vec::new();
            }
            let me = self.players[&peer].position;
            let own = self.car_of(peer);
            let Some(i) = self.combat.missiles.iter().position(|m| m.id == id) else {
                return Vec::new();
            };
            let m = &self.combat.missiles[i];
            let close = distance(m.position, point) < 2.0
                || m.history.iter().any(|&h| distance(h, point) < 2.0);
            let start = add(me, [0.0, 0.5, 0.0]);
            let delta = sub(point, start);
            if distance(me, point) > 450.0
                || !close
                || self.wall_distance(start, normal(delta), length(delta), &[own], 0.12)
                    < length(delta) - 1.0
            {
                return Vec::new();
            }
            let m = &mut self.combat.missiles[i];
            if m.armor.hit(peer, bullet) && m.armor.destroyed() {
                return self.remove_missile(i, 1);
            }
            return Vec::new();
        }
        let len = if k == kind::MISSILE_AIM { 16 } else { 17 };
        let Some(mut r) = exact(body, len) else {
            return Vec::new();
        };
        let (Ok(target), Some(direction)) = (r.i32(), read_v3(&mut r)) else {
            return Vec::new();
        };
        let guided = k == kind::MISSILE_AIM || r.bool().unwrap_or(false);
        let l2 = dot(direction, direction);
        if !(0.9..=1.1).contains(&l2) || !self.players.contains_key(&peer) {
            return Vec::new();
        }
        let direction = normal(direction);
        let eligible = self.missile_eligible(peer);
        let can_lock = eligible && self.can_lock(peer, target, direction);
        let weapon = self.combat.weapons.entry(peer).or_default();
        if k == kind::MISSILE_AIM {
            if self
                .combat
                .aim_rate
                .get(&peer)
                .is_some_and(|&t| now - t < 0.06)
            {
                return Vec::new();
            }
            self.combat.aim_rate.insert(peer, now);
            let progress = weapon.aim(now, target, can_lock);
            return vec![self.missile_reply(peer, now, progress, target, false)];
        }
        if !weapon.try_fire(now, eligible && (!guided || can_lock), guided, target) {
            return vec![self.missile_reply(peer, now, 0.0, target, false)];
        }
        self.combat.serial += 1;
        let m = Missile {
            id: self.combat.serial,
            target: if guided { target } else { -1 },
            owner: peer,
            car: self.car_of(peer),
            position: add(self.players[&peer].position, [0.0, 0.6, 0.0]),
            direction,
            guided,
            born: now,
            armor: Armor::default(),
            history: VecDeque::new(),
            turret: None,
        };
        let mut out = self.missile_state(&m, now);
        self.combat.missiles.push(m);
        out.push(self.missile_reply(peer, now, 1.0, target, true));
        out
    }

    /// A base turret fires a homing missile at an aircraft (`car`). It flies like a player's,
    /// passes through people and only hurts aircraft.
    pub(super) fn fire_turret_missile(&mut self, team: u8, from: V3, car: i32) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        let Some(target) = self.cars.get(&car).map(|c| c.position) else {
            return Vec::new();
        };
        self.combat.serial += 1;
        let start = add(from, [0.0, 1.0, 0.0]);
        let m = Missile {
            id: self.combat.serial,
            target: missile::vehicle_target(car),
            owner: super::NOBODY,
            car: None,
            position: start,
            direction: normal(sub(target, start)),
            guided: true,
            born: now,
            armor: Armor::default(),
            history: VecDeque::new(),
            turret: Some(team),
        };
        let out = self.missile_state(&m, now);
        self.combat.missiles.push(m);
        out
    }

    /// A turret missile went off: aircraft right there take the hit.
    fn turret_blast(&mut self, at: V3) -> Vec<Outgoing> {
        let hit: Vec<i32> = self
            .vehicles
            .aircraft
            .keys()
            .copied()
            .filter(|c| {
                self.cars
                    .get(c)
                    .is_some_and(|car| distance(car.position, at) < 7.0)
            })
            .collect();
        hit.into_iter().flat_map(|c| self.turret_hit(c)).collect()
    }

    /// `MissileReply` `[f32 cooldown][f32 lock progress][i32 target][bool fired]`.
    fn missile_reply(
        &self,
        peer: u8,
        now: f32,
        progress: f32,
        target: i32,
        fired: bool,
    ) -> Outgoing {
        let remaining = self
            .combat
            .weapons
            .get(&peer)
            .map_or(0.0, |w| w.remaining(now));
        let mut w = message(kind::MISSILE_REPLY);
        w.f32(remaining)
            .f32(progress)
            .i32(target)
            .u8(u8::from(fired));
        Self::to_one(peer, w, true)
    }

    /// `MissileState` `[i32 id][position][direction][bool guided][u8 owner][i32 target][f32 age]`.
    fn missile_state(&self, m: &Missile, now: f32) -> Vec<Outgoing> {
        let mut w = message(kind::MISSILE_STATE);
        w.i32(m.id)
            .vec3(m.position)
            .vec3(m.direction)
            .u8(u8::from(m.guided));
        w.u8(m.owner).i32(m.target).f32(now - m.born);
        self.to_vehicle_clients(w)
    }

    /// `MissileRemove` `[i32 id][position][u8 reason: 0 expired, 1 shot down, 2 exploded][u8 owner]`.
    fn remove_missile(&mut self, i: usize, reason: u8) -> Vec<Outgoing> {
        let m = self.combat.missiles.remove(i);
        let mut out = Vec::new();
        if reason == 2 {
            if m.turret.is_some() {
                out.extend(self.turret_blast(m.position));
            } else {
                out.extend(self.explode(m.owner, m.position, true, m.id));
            }
        }
        let mut w = message(kind::MISSILE_REMOVE);
        w.i32(m.id).vec3(m.position).u8(reason).u8(m.owner);
        out.extend(self.to_vehicle_clients(w));
        out
    }

    pub(super) fn tick_missiles(&mut self, now: f32, dt: f32) -> Vec<Outgoing> {
        if self.combat.missiles.is_empty() || dt <= 0.0 {
            return Vec::new();
        }
        let send = now >= self.combat.next_send;
        if send {
            self.combat.next_send = now + MISSILE_SEND;
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.combat.missiles.len() {
            let (owner, born, guided, turret) = {
                let m = &self.combat.missiles[i];
                (m.owner, m.born, m.guided, m.turret)
            };
            let orphaned = turret.is_none() && !self.players.contains_key(&owner);
            if now - born >= missile::lifetime(guided) || orphaned {
                out.extend(self.remove_missile(i, 0));
                continue;
            }
            let speed = if guided {
                missile::GUIDED_SPEED
            } else {
                missile::STRAIGHT_SPEED
            };
            let (position, mut direction, target, car) = {
                let m = &self.combat.missiles[i];
                (m.position, m.direction, m.target, m.car)
            };
            let mut target = target;
            if guided && let Some((mut point, target_car)) = self.missile_target(target, owner) {
                if let Some(c) = target_car.and_then(|c| self.cars.get(&c))
                    && let Some(t) = self.vehicle_type(c.car_type)
                {
                    let aims =
                        t.hull
                            .aim_points(position, direction, c.position, c.rotation.unpack());
                    if let Some(best) = aims
                        .into_iter()
                        .min_by(|a, b| distance(*a, position).total_cmp(&distance(*b, position)))
                    {
                        point = best;
                    }
                }
                let desired = sub(point, position);
                // Cover breaks the lock for good; the rocket flies on straight.
                if self.wall_distance(
                    position,
                    normal(desired),
                    length(desired),
                    &[car, target_car],
                    0.12,
                ) < length(desired) - 0.5
                {
                    target = -1;
                } else {
                    direction = turn(direction, desired, missile::TURN_RADIANS * dt);
                }
            }
            let step = speed * dt;
            let wall = self.wall_distance(position, direction, step, &[car], 0.12);
            let end = add(position, scale(direction, wall));
            let mut first = 1.0f32;
            let mut victim = None;
            for p in self.players.values() {
                if !self.enemy_of(p.index, owner) || (car.is_some() && self.car_of(p.index) == car)
                {
                    continue;
                }
                // Two spheres: torso and legs.
                for h in 0..2 {
                    let centre = sub(p.position, [0.0, 0.65 * h as f32, 0.0]);
                    if let Some(f) = segment_sphere(position, end, centre, 0.85)
                        && f <= first
                    {
                        first = f;
                        victim = Some(p.index);
                    }
                }
            }
            let m = &mut self.combat.missiles[i];
            m.target = target;
            m.direction = direction;
            m.position = lerp(position, end, first);
            m.history.push_back(m.position);
            while m.history.len() > 18 {
                m.history.pop_front();
            }
            if victim.is_some() || wall < step - 0.001 {
                out.extend(self.remove_missile(i, 2));
                continue;
            }
            if send {
                let m = &self.combat.missiles[i];
                out.extend(self.missile_state(m, now));
            }
            i += 1;
        }
        out
    }

    // ---------------------------------------------------------------- drones

    /// `/fpv`: one FPV drone (FlyingControls registers the item on its clients).
    pub(super) fn give_drone(&mut self, peer: u8) -> (Vec<Outgoing>, String) {
        if !self.vehicle_clients().contains(&peer) {
            return (
                Vec::new(),
                "the FPV drone needs the flying-controls mod".into(),
            );
        }
        (
            vec![self.grant(peer, &[(drone::ITEM, 1)])],
            "FPV drone added: select it as a grenade and throw it upwards; Escape ends the flight"
                .into(),
        )
    }

    /// A drone throw: exactly one is used up (vanilla would throw the whole stack), and the
    /// launch that follows must come from here.
    pub(super) fn drone_thrown(&mut self, peer: u8, quantity: &mut i32, origin: V3) -> bool {
        if self.carried(peer, drone::ITEM) < 1 || self.driving(peer) || !self.alive_up(peer) {
            return false;
        }
        *quantity = 1;
        let now = self.last_now.unwrap_or(0.0);
        self.combat
            .pending
            .insert(peer, ThrowToken { time: now, origin });
        true
    }

    /// Does `start → end` hit anything (height field, a vehicle, a player)? Returns the impact.
    fn drone_collision(
        &self,
        start: V3,
        end: V3,
        owner: u8,
        carrier: Option<i32>,
        radius: f32,
    ) -> Option<V3> {
        let delta = sub(end, start);
        let len = length(delta);
        let mut first = 1.0f32;
        let mut found = false;
        if len > 0.003 {
            let wall = self.wall_distance(start, normal(delta), len, &[carrier], radius);
            if wall < len - 0.001 {
                first = wall / len;
                found = true;
            }
        } else if let Some(nav) = self.settings.nav.as_ref()
            && nav
                .height(start[0], start[2])
                .is_some_and(|h| h > start[1] - radius)
        {
            return Some(start);
        }
        for car in self.cars.values() {
            if Some(car.index) == carrier {
                continue;
            }
            if let Some(t) = self.vehicle_type(car.car_type)
                && let Some(f) = t
                    .hull
                    .hit(start, end, car.position, car.rotation.unpack(), radius)
                && f <= first
            {
                first = f;
                found = true;
            }
        }
        for p in self.players.values() {
            if p.index == owner
                || !p.alive
                || (carrier.is_some() && self.car_of(p.index) == carrier)
            {
                continue;
            }
            if let Some(f) = segment_sphere(start, end, p.position, 0.8)
                && f < first
            {
                first = f;
                found = true;
            }
        }
        found.then(|| lerp(start, end, first))
    }

    fn deny_drone(peer: u8) -> Outgoing {
        Self::to_one(peer, message(kind::DRONE_DENIED), true)
    }

    pub(super) fn drone_message(&mut self, peer: u8, k: u8, body: &[u8]) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        let me = self.players.get(&peer).map_or([0.0; 3], |p| p.position);
        match k {
            kind::DRONE_HIT => {
                let Some(mut r) = exact(body, 20) else {
                    return Vec::new();
                };
                let (Ok(id), Ok(bullet), Some(point)) = (r.i32(), r.i32(), read_v3(&mut r)) else {
                    return Vec::new();
                };
                if !self.alive_up(peer) || distance(me, point) > 450.0 {
                    return Vec::new();
                }
                let Some(owner) = self
                    .combat
                    .drones
                    .iter()
                    .find(|(_, d)| d.id == id)
                    .map(|(&o, _)| o)
                else {
                    return Vec::new();
                };
                let d = &self.combat.drones[&owner];
                let close = distance(d.position, point) < 1.6
                    || d.history.iter().any(|&h| distance(h, point) < 1.6);
                let start = add(me, [0.0, 0.5, 0.0]);
                let delta = sub(point, start);
                let covered = self.wall_distance(
                    start,
                    normal(delta),
                    length(delta),
                    &[self.car_of(peer)],
                    0.0,
                ) < length(delta) - 1.0;
                if !close || covered {
                    return Vec::new();
                }
                let d = self.combat.drones.get_mut(&owner).expect("found");
                if d.armor.hit(peer, bullet) && d.armor.destroyed() {
                    return self.finish_drone(owner, true);
                }
                Vec::new()
            }
            kind::DRONE_LAUNCH => {
                let Some(mut r) = exact(body, 28) else {
                    return vec![Self::deny_drone(peer)];
                };
                let (Ok(thrown), Some(position), Some(euler)) =
                    (r.i32(), read_v3(&mut r), read_v3(&mut r))
                else {
                    return vec![Self::deny_drone(peer)];
                };
                let can = self.alive_up(peer)
                    && !self.driving(peer)
                    && !self.combat.drones.contains_key(&peer);
                let token = self.combat.pending.get(&peer);
                let ok = can
                    && token.is_some_and(|t| {
                        now - t.time <= 4.0 && distance(t.origin, position) <= 25.0
                    });
                if !ok {
                    return vec![Self::deny_drone(peer)];
                }
                let origin = self.combat.pending.remove(&peer).expect("checked").origin;
                let carrier = self.car_of(peer);
                if self
                    .drone_collision(origin, position, peer, carrier, drone::RADIUS)
                    .is_some()
                {
                    return vec![Self::deny_drone(peer)];
                }
                self.combat.drone_serial += 1;
                let d = Drone {
                    id: self.combat.drone_serial,
                    thrown,
                    owner: peer,
                    launch_car: carrier,
                    position,
                    euler,
                    seen: now,
                    born: now,
                    sequence: -1,
                    armor: Armor::default(),
                    history: VecDeque::from([position]),
                };
                let out = self.drone_state(&d, 0);
                self.combat.drones.insert(peer, d);
                out
            }
            kind::DRONE_END => {
                let Some(mut r) = exact(body, 17) else {
                    return Vec::new();
                };
                let (Ok(id), Ok(crash), Some(point)) = (r.i32(), r.bool(), read_v3(&mut r)) else {
                    return Vec::new();
                };
                let Some(d) = self.combat.drones.get(&peer).filter(|d| d.id == id) else {
                    return Vec::new();
                };
                if !crash {
                    return self.finish_drone(peer, false);
                }
                let (current, born, launch_car) = (d.position, d.born, d.launch_car);
                if distance(current, point) > 5.0 {
                    return Vec::new();
                }
                let carrier = if now - born < 0.75 { launch_car } else { None };
                let impact =
                    match self.drone_collision(current, point, peer, carrier, drone::RADIUS) {
                        Some(at) => at,
                        None => {
                            // A crash needs something there: ground, a vehicle or a player.
                            let surface = self
                                .drone_collision(point, point, peer, carrier, drone::RADIUS + 0.4)
                                .is_some()
                                || self.settings.nav.as_ref().is_some_and(|n| {
                                    n.height(point[0], point[2])
                                        .is_none_or(|h| (point[1] - h) <= drone::RADIUS + 0.6)
                                })
                                || self.players.values().any(|p| {
                                    p.index != peer && p.alive && distance(p.position, point) < 1.5
                                });
                            if !surface {
                                return Vec::new();
                            }
                            point
                        }
                    };
                self.combat.drones.get_mut(&peer).expect("found").position = impact;
                self.finish_drone(peer, true)
            }
            kind::DRONE_MOVE => {
                let Some(mut r) = exact(body, 32) else {
                    return Vec::new();
                };
                let (Ok(id), Ok(sequence), Some(next), Some(euler)) =
                    (r.i32(), r.i32(), read_v3(&mut r), read_v3(&mut r))
                else {
                    return Vec::new();
                };
                let Some(d) = self.combat.drones.get(&peer).filter(|d| d.id == id) else {
                    return Vec::new();
                };
                let (position, seen, born, launch_car, last) =
                    (d.position, d.seen, d.born, d.launch_car, d.sequence);
                if !self.alive_up(peer) || self.driving(peer) {
                    return self.finish_drone(peer, false);
                }
                if drone::remaining(born, now) <= 0.0 || distance(next, me) >= drone::MAX_RANGE {
                    return self.finish_drone(peer, false);
                }
                if !drone::valid_step(distance(position, next), now - seen) || sequence <= last {
                    return Vec::new();
                }
                let carrier = if now - born < 0.75 { launch_car } else { None };
                if let Some(hit) =
                    self.drone_collision(position, next, peer, carrier, drone::RADIUS)
                {
                    let d = self.combat.drones.get_mut(&peer).expect("found");
                    d.position = hit;
                    d.sequence = sequence;
                    return self.finish_drone(peer, true);
                }
                let d = self.combat.drones.get_mut(&peer).expect("found");
                d.position = next;
                d.euler = euler;
                d.seen = now;
                d.sequence = sequence;
                d.history.push_back(next);
                while d.history.len() > 6 {
                    d.history.pop_front();
                }
                let d = &self.combat.drones[&peer];
                self.drone_state(d, sequence)
            }
            _ => Vec::new(),
        }
    }

    /// `DroneState` `[i32 id][u8 owner][i32 thrown][i32 sequence][position][euler]`.
    fn drone_state(&self, d: &Drone, sequence: i32) -> Vec<Outgoing> {
        let mut w = message(kind::DRONE_STATE);
        w.i32(d.id)
            .u8(d.owner)
            .i32(d.thrown)
            .i32(sequence)
            .vec3(d.position)
            .vec3(d.euler);
        self.to_vehicle_clients(w)
    }

    /// `DroneFinish` `[i32 id][u8 owner][position][bool exploded]`.
    fn finish_drone(&mut self, owner: u8, explode: bool) -> Vec<Outgoing> {
        let Some(d) = self.combat.drones.remove(&owner) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if explode {
            out.extend(self.explode(owner, d.position, false, d.id));
        }
        let mut w = message(kind::DRONE_FINISH);
        w.i32(d.id).u8(owner).vec3(d.position).u8(u8::from(explode));
        out.extend(self.to_vehicle_clients(w));
        out
    }

    pub(super) fn tick_drones(&mut self, now: f32) -> Vec<Outgoing> {
        let ended: Vec<u8> = self
            .combat
            .drones
            .values()
            .filter(|d| {
                let me = self.players.get(&d.owner).map(|p| p.position);
                !self.alive_up(d.owner)
                    || me.is_none()
                    || now - d.seen > drone::LINK_TIMEOUT
                    || drone::remaining(d.born, now) <= 0.0
                    || me.is_some_and(|m| distance(d.position, m) >= drone::MAX_RANGE)
            })
            .map(|d| d.owner)
            .collect();
        let mut out = Vec::new();
        for owner in ended {
            out.extend(self.finish_drone(owner, false));
        }
        self.combat.pending.retain(|_, t| now - t.time <= 4.0);
        let expired: Vec<(bool, i32)> = self
            .combat
            .blasts
            .iter()
            .filter(|(_, b)| b.expires < now)
            .map(|(&k, _)| k)
            .collect();
        for k in expired {
            self.combat.blasts.remove(&k);
        }
        out
    }

    // ---------------------------------------------------------------- blasts

    /// An explosion: remember it for hit markers, and damage bots (they have no client).
    fn explode(&mut self, owner: u8, position: V3, is_missile: bool, id: i32) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        self.combat.blasts.insert(
            (is_missile, id),
            Blast {
                owner,
                position,
                expires: now + 5.0,
                confirmed: Vec::new(),
            },
        );
        let (radius, base) = if is_missile {
            (blast::HAND_RADIUS, missile::DAMAGE)
        } else {
            (blast::DYNAMITE_RADIUS, blast::DYNAMITE_DAMAGE)
        };
        let bots: Vec<(u8, V3, f32)> = self
            .players
            .values()
            .filter(|p| self.is_bot(p.index) && p.alive && p.health > 0.0)
            .map(|p| (p.index, p.position, p.health))
            .collect();
        let mut out = Vec::new();
        for (bot, at, health) in bots {
            let delta = sub(at, position);
            let d = length(delta);
            if d > radius {
                continue;
            }
            let blocked = d > 0.6
                && self.wall_distance(
                    add(position, scale(normal(delta), 0.1)),
                    normal(delta),
                    d - 0.5,
                    &[],
                    0.0,
                ) < d - 0.6;
            let amount = blast::damage(base, radius, d, blocked);
            if amount <= 0.0 {
                continue;
            }
            let ev = DamageEvent {
                attacker: owner,
                victim: bot,
                new_health: (health - amount).max(0.0),
                direction: normal(delta),
                hit: None,
                exclude_victim: false,
            };
            out.extend(self.damage(bot, ev));
            let after = self.players.get(&bot).map_or(0.0, |p| p.health);
            out.extend(self.confirm_blast(
                owner,
                is_missile,
                id,
                false,
                i32::from(bot),
                at,
                (health - after).max(0.0),
            ));
        }
        out
    }

    /// A blast receipt: the attacker's client reports what its explosion did; a real hit (near
    /// the blast, first time per target) earns a hit marker (`BlastConfirmed`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn confirm_blast(
        &mut self,
        attacker: u8,
        is_missile: bool,
        id: i32,
        vehicle: bool,
        target: i32,
        at: V3,
        amount: f32,
    ) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        if amount.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
            || !amount.is_finite()
            || self.is_bot(attacker)
            || (!vehicle && target == i32::from(attacker))
        {
            return Vec::new();
        }
        let Some(b) = self.combat.blasts.get_mut(&(is_missile, id)) else {
            return Vec::new();
        };
        let radius = if is_missile {
            blast::HAND_RADIUS
        } else {
            blast::DYNAMITE_RADIUS
        } + if vehicle { 15.0 } else { 3.0 };
        if b.owner != attacker
            || now > b.expires
            || distance(at, b.position) > radius
            || b.confirmed.contains(&(vehicle, target))
        {
            return Vec::new();
        }
        b.confirmed.push((vehicle, target));
        let mut w = message(kind::BLAST_CONFIRMED);
        w.u8(u8::from(is_missile))
            .u8(u8::from(vehicle))
            .f32(amount)
            .i32(id);
        vec![Self::to_one(attacker, w, true)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipts_are_stripped() {
        let mut p = vec![1u8; 10];
        p.extend_from_slice(&RECEIPT_MAGIC.to_le_bytes());
        p.push(1);
        p.extend_from_slice(&7i32.to_le_bytes());
        let (native, missile, id) = strip_receipt(&p).expect("receipt");
        assert_eq!((native.len(), missile, id), (10, true, 7));
        assert!(strip_receipt(&[0; 10]).is_none());
        assert!(strip_receipt(&[0; 25]).is_none());
    }
}
