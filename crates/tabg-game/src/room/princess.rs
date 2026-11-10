//! Capture the Princess (rules and bases in `princess.rs`): two teams start at their cages, each
//! with a princess hidden in their base. Whoever holds a gun on a princess makes her walk: the
//! other team's princess walks to your cage (bring her in and you win), your own walks back
//! home, or where you push her inside your territory. Kills earn helicopters (a smoke grenade
//! that calls one in), and each base has turrets that fire homing missiles at enemy aircraft.
//!
//! Princesses are server-side players in their own groups (2 and 3), so both teams can hurt
//! them: killed by the other team she is back home at once and walks at her normal speed again;
//! killed by her own team she comes back halfway between home and where she fell, and walks a
//! little faster for the other team from then on.
//!
//! The PrincessMode client mod (our event 239, `PRNC` v1; 240–247 are taken) gets the state for
//! its HUD, the cages and turrets, and short banners; players without it get the banners as chat
//! lines.

use std::collections::BTreeMap;
use std::sync::Arc;

use tabg_protocol::combat::PlayerDead;
use tabg_protocol::loot::chunk_of;
use tabg_protocol::match_flow::GameStateChanged;
use tabg_protocol::messages::{NO_WEAPON, PackedDirection, PlayerJoined, game_state};
use tabg_protocol::{EventCode, Packet, Writer};

use super::vehicles::dotnet_string;
use super::{NOBODY, Outgoing, Recipients, Room, RoomEvent};
use crate::nav::{EYE, FlowField, NavGrid};
use crate::princess::{BotSkill, PrincessRules};

pub const PRINCESS: u8 = 239;
const MAGIC: &[u8; 4] = b"PRNC";
const VERSION: u8 = 1;
const HELLO: u8 = 1;
const STATE: u8 = 2;
const TOAST: u8 = 3;
/// Server → client: the loadouts to choose from (an empty list closes the menu).
const OFFER: u8 = 4;
/// Client → server: `[u8 choice]`.
const PICK: u8 = 5;
/// Mod clients count as such this long after their last hello (sent every few seconds).
const CLIENT_TIMEOUT: f32 = 12.0;

/// Princess players are in groups 2 (team 0's) and 3 (team 1's).
const PRINCESS_GROUP: u8 = 2;
/// How far from her someone can hold her at gunpoint.
const ESCORT_RANGE: f32 = 16.0;
/// How far off (degrees) the aim may be, plus her width at that distance.
const AIM_CONE: f32 = 12.0;
/// An escort holds this long after the aim slips (turning to shoot back, recoil, lag).
const ESCORT_GRACE: f32 = 2.0;
/// Close enough to the cage to count as brought in.
const CAPTURE_RADIUS: f32 = 4.0;
const PRINCESS_RESPAWN: f32 = 4.0;
/// Bots and players notice a princess this far away when nothing is in between.
const SIGHT: f32 = 70.0;
/// How long a team remembers where it last saw the other princess.
const MEMORY: f32 = 25.0;
const TURRET_RANGE: f32 = 220.0;
/// Seconds a turret needs a clear view of its target before it fires.
const TURRET_LOCK: f32 = 1.5;
/// Seconds between a turret's missiles.
const TURRET_INTERVAL: f32 = 8.0;
/// Share of an aircraft's health in the air each turret missile takes (two bring it down).
const TURRET_DAMAGE: f32 = 0.55;
/// Seconds from throwing the smoke grenade to the helicopter arriving.
const HELI_DELAY: f32 = 3.0;
const SYNC_INTERVAL: f32 = 0.25;

/// Banner colours on the mod's HUD.
pub(super) const INFO: u8 = 0;
pub(super) const GOOD: u8 = 1;
const BAD: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Escort {
    pub team: u8,
    pub by: u8,
    until: f32,
}

#[derive(Clone, Debug)]
pub(super) struct Princess {
    pub index: u8,
    pub team: u8,
    pub home: [f32; 3],
    /// Times her own team killed her since the other team last did.
    pub stacks: u8,
    pub escort: Option<Escort>,
    pub contested: bool,
    /// When and where she comes back after dying.
    respawn: Option<(f32, [f32; 3])>,
    /// Per team: where it last saw her, and when.
    pub seen: [Option<([f32; 3], f32)>; 2],
    /// Through a gateway: the far side she walks to before following the field again.
    crossing: Option<[f32; 3]>,
}

#[derive(Clone, Debug)]
struct Turret {
    team: u8,
    position: [f32; 3],
    target: Option<i32>,
    lock_since: f32,
    ready_at: f32,
}

#[derive(Debug, Default)]
pub(super) struct PrincessState {
    pub princesses: Vec<Princess>,
    /// Enemy kills per team toward the next helicopter.
    kills: [u32; 2],
    /// Helicopters earned while nobody on the team could take one.
    owed: [u32; 2],
    /// Players holding a helicopter smoke grenade.
    pub(super) tokens: BTreeMap<u8, u32>,
    /// Helicopters on their way: when, who called, where, facing.
    arriving: Vec<(f32, u8, [f32; 3], f32)>,
    turrets: Vec<Turret>,
    /// The grid with the bases' gates linked (shared with the bots).
    pub nav: Option<Arc<NavGrid>>,
    /// Flow fields by goal (cages and homes).
    fields: Vec<Arc<FlowField>>,
    /// Respawn spots around each team's cage.
    spawns: [Vec<[f32; 3]>; 2],
    /// PrincessMode clients: last hello.
    clients: BTreeMap<u8, f32>,
    /// Loadout choices open, the last three offered, and since when a player is out of their
    /// territory (`princess_loadouts.rs`).
    pub(super) offers: BTreeMap<u8, super::princess_loadouts::Offer>,
    pub(super) last_offer: BTreeMap<u8, [usize; 3]>,
    pub(super) away_since: BTreeMap<u8, f32>,
    next_sync: f32,
    pub winner: Option<u8>,
}

fn header(op: u8) -> Writer {
    let mut w = Writer::new();
    for &b in MAGIC {
        w.u8(b);
    }
    w.u8(VERSION).u8(op);
    w
}

fn flat_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

impl Room {
    pub(super) fn is_princess_mode(&self) -> bool {
        self.settings.princess.is_some()
    }

    pub(super) fn princess_rules(&self) -> &PrincessRules {
        self.settings.princess.as_ref().expect("princess mode")
    }

    /// Which team's princess this player is.
    pub(super) fn princess_of(&self, index: u8) -> Option<usize> {
        self.princess
            .princesses
            .iter()
            .position(|p| p.index == index)
    }

    pub(super) fn is_princess(&self, index: u8) -> bool {
        self.princess_of(index).is_some()
    }

    /// Team 0 or 1 of a player (not a princess).
    pub(super) fn team_of(&self, index: u8) -> Option<u8> {
        self.players
            .get(&index)
            .map(|p| p.group)
            .filter(|&g| g < PRINCESS_GROUP)
    }

    /// New players join their TABG squad's team if a squad mate is here, else the team with
    /// fewer players (bots make room).
    pub(super) fn princess_group(&self, squad_id: u64) -> u8 {
        if squad_id != 0
            && let Some(mate) = self.players.values().find(|p| {
                p.squad_id == squad_id && p.group < PRINCESS_GROUP && !self.is_bot(p.index)
            })
        {
            return mate.group;
        }
        let humans = |g: u8| {
            self.players
                .values()
                .filter(|p| p.group == g && !self.is_bot(p.index))
                .count()
        };
        let all = |g: u8| self.players.values().filter(|p| p.group == g).count();
        match humans(0).cmp(&humans(1)) {
            std::cmp::Ordering::Less => 0,
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Equal => u8::from(all(1) < all(0)),
        }
    }

    /// The walkability grid with the bases' gateways linked (made once).
    pub(super) fn princess_nav(&mut self) -> Option<Arc<NavGrid>> {
        if let Some(nav) = &self.princess.nav {
            return Some(Arc::clone(nav));
        }
        let base = self.settings.nav.as_ref()?;
        let mut nav = NavGrid::clone(base);
        for b in &self.princess_rules().bases {
            for &(a, c) in &b.gates {
                nav.add_link(a, c);
            }
        }
        let nav = Arc::new(nav);
        self.princess.nav = Some(Arc::clone(&nav));
        Some(nav)
    }

    /// The flow field towards `goal` (made on first use).
    pub(super) fn field_to(&mut self, goal: [f32; 3]) -> Option<Arc<FlowField>> {
        if let Some(f) = self
            .princess
            .fields
            .iter()
            .find(|f| flat_distance(f.goal, goal) < 0.5)
        {
            return Some(Arc::clone(f));
        }
        let nav = self.princess_nav()?;
        let (min, max) = self.princess_rules().region();
        let field = Arc::new(nav.flow_field(goal, min, max)?);
        self.princess.fields.push(Arc::clone(&field));
        Some(field)
    }

    /// Where flow fields lead: both cages and every hideout.
    pub(super) fn field_goals(&self) -> Vec<[f32; 3]> {
        self.princess_rules()
            .bases
            .iter()
            .flat_map(|b| std::iter::once(b.cage).chain(b.hideouts.iter().copied()))
            .collect()
    }

    /// The flow field that leads close to `at` (its goal within 35 m), if any.
    pub(super) fn field_near(&mut self, at: [f32; 3]) -> Option<Arc<FlowField>> {
        let goal = self
            .field_goals()
            .into_iter()
            .filter(|g| flat_distance(*g, at) < 35.0)
            .min_by(|a, b| flat_distance(*a, at).total_cmp(&flat_distance(*b, at)))?;
        self.field_to(goal)
    }

    /// Spots around a team's cage to respawn on (walkable, on the cage's level).
    fn cage_spawns(&mut self, team: usize) -> Vec<[f32; 3]> {
        if !self.princess.spawns[team].is_empty() {
            return self.princess.spawns[team].clone();
        }
        let cage = self.princess_rules().bases[team].cage;
        let mut spots = Vec::new();
        if let Some(nav) = self.princess_nav() {
            for ring in [3.0f32, 5.0, 7.0] {
                for k in 0..12 {
                    let a = k as f32 / 12.0 * std::f32::consts::TAU;
                    let (x, z) = (cage[0] + ring * a.sin(), cage[2] + ring * a.cos());
                    if let Some(h) = nav.height(x, z)
                        && (h - cage[1]).abs() < 1.5
                    {
                        spots.push([x, h, z]);
                    }
                }
            }
        }
        if spots.is_empty() {
            spots.push(cage);
        }
        self.princess.spawns[team].clone_from(&spots);
        spots
    }

    /// Capture the Princess: players (re)spawn looking at their cage, so they know where it is.
    pub(super) fn cage_facing(&self, group: u8, at: [f32; 3]) -> Option<f32> {
        let rules = self.settings.princess.as_ref()?;
        let cage = rules.bases.get(usize::from(group))?.cage;
        let (dx, dz) = (cage[0] - at[0], cage[2] - at[2]);
        (dx.hypot(dz) > 0.5).then(|| dx.atan2(dz).to_degrees().rem_euclid(360.0))
    }

    /// Where a player of `group` (re)spawns: next to their cage.
    pub(super) fn princess_spawn(&mut self, group: u8) -> [f32; 3] {
        let spots = self.cage_spawns(usize::from(group.min(1)));
        let p = spots[self.rng.index(spots.len())];
        [p[0], p[1] + 0.3, p[2]]
    }

    // ------------------------------------------------------------------ lobby

    /// In the lobby (once a human is there): the two princesses, and bots to fill both teams.
    pub(super) fn manage_princess_lobby(&mut self) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let humans = self.players.keys().filter(|p| !self.is_bot(**p)).count();
        if humans == 0 {
            // Nobody to play with: bots and princesses leave.
            let gone: Vec<u8> = self
                .bots
                .keys()
                .copied()
                .chain(self.princess.princesses.iter().map(|p| p.index))
                .collect();
            self.princess.princesses.clear();
            for b in gone {
                self.bots.remove(&b);
                out.extend(self.disconnect(b));
            }
            return out;
        }
        for team in 0..2u8 {
            if self.princess.princesses.iter().all(|p| p.team != team) {
                out.extend(self.add_princess(team));
            }
        }
        let size = usize::from(self.princess_rules().team_size);
        for team in 0..2u8 {
            let members = |room: &Self| {
                room.players
                    .values()
                    .filter(|p| p.group == team && !room.is_princess(p.index))
                    .count()
            };
            let humans_in = self
                .players
                .values()
                .filter(|p| p.group == team && !self.is_bot(p.index))
                .count();
            let wanted_bots = size.saturating_sub(humans_in);
            let bots_in: Vec<u8> = self
                .bots
                .keys()
                .copied()
                .filter(|b| self.players.get(b).is_some_and(|p| p.group == team))
                .collect();
            for &b in bots_in.iter().skip(wanted_bots) {
                self.bots.remove(&b);
                out.extend(self.disconnect(b));
            }
            while members(self) < size.max(humans_in) {
                match self.add_bot(team) {
                    Some(joined) => out.push(joined),
                    None => break,
                }
            }
        }
        out
    }

    fn add_princess(&mut self, team: u8) -> Vec<Outgoing> {
        let Some(index) = self.free_bot_index() else {
            return Vec::new();
        };
        let base = self.princess_rules().bases[usize::from(team)].clone();
        let home = if base.hideouts.is_empty() {
            base.centre
        } else {
            base.hideouts[self.rng.index(base.hideouts.len())]
        };
        let group = PRINCESS_GROUP + team;
        let mut p = super::Player::new(
            index,
            group,
            base.princess.clone(),
            format!("bot:princess-{}", base.name.to_lowercase()),
            0,
        );
        p.in_world = true;
        p.position = home;
        p.gear = base.outfit.to_vec();
        p.chunk = chunk_of(home);
        self.players.insert(index, p);
        self.princess.princesses.push(Princess {
            index,
            team,
            home,
            stacks: 0,
            escort: None,
            contested: false,
            respawn: None,
            seen: [None, None],
            crossing: None,
        });
        vec![Outgoing::reliable(
            Recipients::All,
            EventCode::Login,
            PlayerJoined {
                player_index: index,
                group_index: group,
                player_name: base.princess,
                gear: base.outfit.to_vec(),
                is_dev: false,
                colour: 0,
            }
            .encode(),
        )]
    }

    // ------------------------------------------------------------------ the match

    /// Countdown over: everyone at their cage, princesses hidden, go.
    pub(super) fn start_princess_match(&mut self) -> Vec<Outgoing> {
        self.state = game_state::STARTED;
        self.events.push(RoomEvent::MatchState {
            state: "started".into(),
        });
        let nav = self.princess_nav();
        let rules = self.princess_rules().clone();
        let mut joined = Vec::new();
        for team in 0..2u8 {
            if self.princess.princesses.iter().all(|p| p.team != team) {
                joined.extend(self.add_princess(team));
            }
        }
        self.princess.kills = [0, 0];
        self.princess.owed = [0, 0];
        self.princess.tokens.clear();
        self.princess.arriving.clear();
        self.princess.offers.clear();
        self.princess.away_since.clear();
        self.princess.winner = None;
        self.princess.turrets = if rules.turrets && nav.is_some() {
            rules
                .bases
                .iter()
                .enumerate()
                .flat_map(|(team, b)| {
                    b.turrets.iter().map(move |&position| Turret {
                        team: team as u8,
                        position,
                        target: None,
                        lock_since: 0.0,
                        ready_at: 0.0,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        // A new hiding place for each princess.
        for i in 0..self.princess.princesses.len() {
            let team = usize::from(self.princess.princesses[i].team);
            let hideouts = &rules.bases[team].hideouts;
            let home = if hideouts.is_empty() {
                rules.bases[team].centre
            } else {
                hideouts[self.rng.index(hideouts.len())]
            };
            let p = &mut self.princess.princesses[i];
            p.home = home;
            p.stacks = 0;
            p.escort = None;
            p.contested = false;
            p.respawn = None;
            p.crossing = None;
            p.seen = [None, None];
            p.seen[team] = Some((home, f32::MAX));
        }
        // Everyone appears at once: players at their cage, princesses in hiding.
        let peers: Vec<u8> = self.players.keys().copied().collect();
        let mut entries = Vec::new();
        for &peer in &peers {
            let group = self.players[&peer].group;
            let at = match self.princess_of(peer) {
                Some(i) => self.princess.princesses[i].home,
                None => self.princess_spawn(group),
            };
            let facing = self
                .cage_facing(group, at)
                .unwrap_or_else(|| self.rng.range(0.0, 360.0));
            let p = self.players.get_mut(&peer).expect("listed");
            p.position = at;
            p.rotation = [0.0, facing];
            p.dropped = true;
            p.landed = true;
            p.dirty = true;
            entries.push((peer, at, facing));
        }
        let mut out = joined;
        out.push(Self::state_changed(GameStateChanged::Started));
        out.push(respawn_packet(&entries));
        for &peer in &peers {
            out.extend(self.force_chunk_entry(peer));
            if !self.is_princess(peer) {
                out.extend(self.give_loadout(peer));
            }
        }
        // Make the ways to every cage and hideout now, not when the first bot needs them.
        for goal in self.field_goals() {
            self.field_to(goal);
        }
        for team in 0..2u8 {
            let mine = &rules.bases[usize::from(team)];
            let theirs = &rules.bases[usize::from(1 - team)];
            let text = format!(
                "Capture the Princess! Bring the {} to your cage. Protect the {}.",
                theirs.princess, mine.princess
            );
            out.extend(self.toast_team(team, &text, INFO, 6.0));
        }
        out
    }

    /// The winning team, once a princess is in a cage.
    pub(super) fn princess_winner(&self) -> Option<u8> {
        if let Some(w) = self.princess.winner {
            return Some(w);
        }
        if self.players.keys().all(|p| self.is_bot(*p)) {
            return Some(GameStateChanged::NO_WINNER);
        }
        if self.settings.time_limit > 0.0
            && self.last_now.unwrap_or(0.0) - self.match_started_at >= self.settings.time_limit
        {
            return Some(GameStateChanged::NO_WINNER);
        }
        None
    }

    pub(super) fn tick_princess(&mut self, now: f32, dt: f32) -> Vec<Outgoing> {
        let mut out = Vec::new();
        if self.state != game_state::STARTED || self.princess.winner.is_some() {
            return out;
        }
        let Some(nav) = self.princess_nav() else {
            return out;
        };
        for i in 0..self.princess.princesses.len() {
            out.extend(self.princess_respawn_due(i, now));
            out.extend(self.update_escort(i, now));
            out.extend(self.walk_princess(i, dt, &nav));
            if self.princess.winner.is_some() {
                return out;
            }
        }
        self.update_sightings(now, &nav);
        out.extend(self.pay_owed_helicopters());
        out.extend(self.helicopters_arrive(now));
        out.extend(self.tick_turrets(now));
        out.extend(self.tick_loadouts(now));
        if now >= self.princess.next_sync {
            self.princess.next_sync = now + SYNC_INTERVAL;
            out.extend(self.princess_sync(now));
        }
        out
    }

    /// Is `index` holding a gun on the princess `target` standing at `at` (close, on foot,
    /// facing her)? Bots only when they mean to (one walking past isn't escorting her).
    fn aims_at(&self, index: u8, target: u8, at: [f32; 3]) -> bool {
        let Some(p) = self.players.get(&index) else {
            return false;
        };
        if self
            .bots
            .get(&index)
            .is_some_and(|b| b.princess.holding != Some(target))
        {
            return false;
        }
        let armed = p.weapon_type != NO_WEAPON && p.weapon_type != i32::MIN && p.weapon_type >= 0;
        if !p.alive || p.downed || p.seat.is_some() || !armed {
            return false;
        }
        let (dx, dz) = (at[0] - p.position[0], at[2] - p.position[2]);
        let d = dx.hypot(dz);
        if d > ESCORT_RANGE {
            return false;
        }
        if d < 1.0 {
            return true;
        }
        let yaw = p.rotation[1].to_radians();
        let cos = (yaw.sin() * dx + yaw.cos() * dz) / d;
        let off = cos.clamp(-1.0, 1.0).acos().to_degrees();
        let allowed = AIM_CONE + (0.6 / d).atan().to_degrees();
        // Pitch: positive looks down (Unity); the target's chest against the eyes.
        let want = -((at[1] + 1.0 - (p.position[1] + EYE)) / d)
            .atan()
            .to_degrees();
        let pitch = p.rotation[0];
        let pitch = if pitch > 180.0 { pitch - 360.0 } else { pitch };
        off <= allowed && (pitch - want).abs() <= 35.0
    }

    /// Who holds her at gunpoint now: one team escorts, both teams contest.
    fn update_escort(&mut self, i: usize, now: f32) -> Vec<Outgoing> {
        let (index, team) = (
            self.princess.princesses[i].index,
            self.princess.princesses[i].team,
        );
        let Some(at) = self
            .players
            .get(&index)
            .filter(|p| p.alive)
            .map(|p| p.position)
        else {
            return Vec::new();
        };
        let mut holders: Vec<(f32, u8, u8)> = self
            .players
            .values()
            .filter(|p| p.group < PRINCESS_GROUP && self.aims_at(p.index, index, at))
            .map(|p| (flat_distance(p.position, at), p.group, p.index))
            .collect();
        holders.sort_by(|a, b| a.0.total_cmp(&b.0));
        let teams = (
            holders.iter().any(|h| h.1 == 0),
            holders.iter().any(|h| h.1 == 1),
        );
        let before = self.princess.princesses[i].escort;
        let p = &mut self.princess.princesses[i];
        p.contested = teams.0 && teams.1;
        if p.contested {
            p.escort = None;
        } else if let Some(&(_, group, by)) = holders.first() {
            p.escort = Some(Escort {
                team: group,
                by,
                until: now + ESCORT_GRACE,
            });
        } else if p.escort.is_some_and(|e| now > e.until) {
            p.escort = None;
        }
        let after = p.escort;
        let mut out = Vec::new();
        // Tell both teams when the other side starts taking her.
        let taken = |e: Option<Escort>| e.is_some_and(|e| e.team != team);
        if taken(after) && !taken(before) {
            let rules = self.princess_rules();
            let name = rules.bases[usize::from(team)].princess.clone();
            let thief = after
                .map(|e| e.by)
                .and_then(|b| self.players.get(&b))
                .map_or_else(String::new, |p| p.name.clone());
            out.extend(self.toast_team(
                team,
                &format!("The {name} is being taken by {thief}!"),
                BAD,
                3.0,
            ));
            out.extend(self.toast_team(
                1 - team,
                &format!("You have the {name}. Keep your gun on her!"),
                GOOD,
                3.0,
            ));
        }
        out
    }

    /// One step of walking: to the escorts' cage, home, or where her own team pushes her.
    fn walk_princess(&mut self, i: usize, dt: f32, nav: &NavGrid) -> Vec<Outgoing> {
        let pr = self.princess.princesses[i].clone();
        let Some(me) = self
            .players
            .get(&pr.index)
            .filter(|p| p.alive)
            .map(|p| p.position)
        else {
            return Vec::new();
        };
        // Standing still unless she takes a step below (legs at rest for the client).
        if let Some(p) = self.players.get_mut(&pr.index)
            && (p.movement != 0 || p.direction != PackedDirection::ZERO)
        {
            p.movement = 0;
            p.direction = PackedDirection::ZERO;
            p.dirty = true;
        }
        let Some(escort) = pr.escort.filter(|_| !pr.contested) else {
            return Vec::new();
        };
        let rules = self.princess_rules().clone();
        let base = &rules.bases[usize::from(pr.team)];
        let taken = escort.team != pr.team;
        // Taken, she walks faster each time her own team killed her.
        let speed = if taken {
            1.0 + rules.own_kill_bonus * f32::from(pr.stacks)
        } else {
            1.0
        };
        let step = rules.walk_speed * dt * speed;
        if taken && flat_distance(me, rules.bases[usize::from(escort.team)].cage) <= CAPTURE_RADIUS
        {
            return self.princess_captured(i, escort.team);
        }
        // Halfway through a gateway: finish it first.
        if let Some(far) = pr.crossing.filter(|f| flat_distance(*f, me) > 0.4) {
            return self.step_princess(i, me, far, step, nav);
        }
        self.princess.princesses[i].crossing = None;
        // A player of her own team pushes her around inside the territory; their bots (and
        // anyone outside it) walk her home, the other team to their cage.
        let pushed =
            !taken && !self.is_bot(escort.by) && flat_distance(me, base.centre) <= base.radius;
        let next = if pushed {
            let yaw = self.players.get(&escort.by).map_or(0.0, |p| p.rotation[1]);
            push_target(nav, me, yaw, base.centre, base.radius).map(|to| (to, false))
        } else {
            let goal = if taken {
                rules.bases[usize::from(escort.team)].cage
            } else if flat_distance(me, pr.home) < 1.0 {
                return Vec::new();
            } else {
                pr.home
            };
            let Some(field) = self.field_to(goal) else {
                return Vec::new();
            };
            nav.flow_next(&field, me, 3)
        };
        let Some((to, gateway)) = next else {
            return Vec::new();
        };
        if gateway {
            self.princess.princesses[i].crossing = Some(to);
        }
        self.step_princess(i, me, to, step, nav)
    }

    /// Move her up to `step` metres towards `to`, facing the way she walks.
    fn step_princess(
        &mut self,
        i: usize,
        me: [f32; 3],
        to: [f32; 3],
        step: f32,
        nav: &NavGrid,
    ) -> Vec<Outgoing> {
        let index = self.princess.princesses[i].index;
        let (dx, dz) = (to[0] - me[0], to[2] - me[2]);
        let d = dx.hypot(dz);
        if d < 0.01 || step <= 0.0 {
            return Vec::new();
        }
        let k = (step / d).min(1.0);
        let (x, z) = (me[0] + dx * k, me[2] + dz * k);
        let y = nav.walk_height(me[1], x, z);
        let yaw = dx.atan2(dz).to_degrees().rem_euclid(360.0);
        let Some(p) = self.players.get_mut(&index) else {
            return Vec::new();
        };
        let old_chunk = p.chunk;
        p.position = [x, y, z];
        p.rotation = [0.0, yaw];
        // Walking forward, the way she faces.
        p.direction = PackedDirection::pack([dx / d, 0.0, dz / d]);
        p.movement = 0x01;
        p.dirty = true;
        let chunk = chunk_of(p.position);
        if chunk != old_chunk {
            return self.chunk_changed(index, chunk);
        }
        Vec::new()
    }

    fn princess_captured(&mut self, i: usize, team: u8) -> Vec<Outgoing> {
        self.princess.winner = Some(team);
        let rules = self.princess_rules();
        let name = rules.bases[usize::from(self.princess.princesses[i].team)]
            .princess
            .clone();
        let winners = rules.bases[usize::from(team)].name.clone();
        let mut out = self.toast_team(
            team,
            &format!("The {name} is in your cage. You win!"),
            GOOD,
            8.0,
        );
        out.extend(self.toast_team(
            1 - team,
            &format!("{winners} caged the {name}. You lose."),
            BAD,
            8.0,
        ));
        out
    }

    /// Who on each team can see the other team's princess (bots use this to find her).
    fn update_sightings(&mut self, now: f32, nav: &NavGrid) {
        for i in 0..self.princess.princesses.len() {
            let (index, team) = (
                self.princess.princesses[i].index,
                self.princess.princesses[i].team,
            );
            let Some(at) = self
                .players
                .get(&index)
                .filter(|p| p.alive)
                .map(|p| p.position)
            else {
                continue;
            };
            self.princess.princesses[i].seen[usize::from(team)] = Some((at, now));
            let enemy = 1 - team;
            let escorted = self.princess.princesses[i]
                .escort
                .is_some_and(|e| e.team == enemy);
            let spotted = escorted
                || self.players.values().any(|p| {
                    p.group == enemy
                        && p.alive
                        && flat_distance(p.position, at) < SIGHT
                        && nav.line_of_sight(p.position, at)
                });
            if spotted {
                self.princess.princesses[i].seen[usize::from(enemy)] = Some((at, now));
            } else if self.princess.princesses[i].seen[usize::from(enemy)]
                .is_some_and(|(_, t)| now - t > MEMORY)
            {
                self.princess.princesses[i].seen[usize::from(enemy)] = None;
            }
        }
    }

    /// Where `team` believes the princess of `owner` is (and how fresh that is).
    pub(super) fn princess_known(&self, owner: u8, team: u8) -> Option<([f32; 3], f32)> {
        self.princess
            .princesses
            .iter()
            .find(|p| p.team == owner)
            .and_then(|p| p.seen[usize::from(team.min(1))])
    }

    pub(super) fn princess_index(&self, owner: u8) -> Option<u8> {
        self.princess
            .princesses
            .iter()
            .find(|p| p.team == owner)
            .map(|p| p.index)
    }

    pub(super) fn princess_escort(&self, owner: u8) -> Option<Escort> {
        self.princess
            .princesses
            .iter()
            .find(|p| p.team == owner)
            .and_then(|p| p.escort)
    }

    // ------------------------------------------------------------------ deaths

    /// A princess died: back home (killed by the other team) or halfway home and a bit faster
    /// (killed by her own). Not a kill for anyone's score.
    pub(super) fn princess_killed(&mut self, victim: u8, i: usize) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        let Some(p) = self.players.get_mut(&victim) else {
            return Vec::new();
        };
        p.alive = false;
        p.downed = false;
        p.health = 0.0;
        let (at, last_attacker, name) = (p.position, p.last_attacker, p.name.clone());
        let killer_team = self.team_of(last_attacker);
        let killer_name = self.players.get(&last_attacker).map(|k| k.name.clone());
        let rules = self.princess_rules().clone();
        let pr = &mut self.princess.princesses[i];
        let team = pr.team;
        pr.escort = None;
        pr.contested = false;
        let (back_at, own) = match killer_team {
            Some(t) if t == team => {
                pr.stacks = (pr.stacks + 1).min(rules.own_kill_max);
                let half = [
                    (pr.home[0] + at[0]) * 0.5,
                    (pr.home[1] + at[1]) * 0.5,
                    (pr.home[2] + at[2]) * 0.5,
                ];
                (half, Some(true))
            }
            Some(_) => {
                pr.stacks = 0;
                (pr.home, Some(false))
            }
            None => (at, None),
        };
        let stacks = pr.stacks;
        let home = pr.home;
        let back_at = self.walkable_near(back_at, home);
        self.princess.princesses[i].respawn = Some((now + PRINCESS_RESPAWN, back_at));
        self.events.push(RoomEvent::Killed {
            victim: name.clone(),
            killer: killer_name,
            ring: false,
        });
        let dead = PlayerDead {
            victim,
            last_attacker,
            spectate: NOBODY,
            victim_name: name.clone(),
            killer_weapon: NO_WEAPON,
            ring_death: false,
        };
        let mut out = vec![Self::broadcast(EventCode::PlayerDead, dead.encode())];
        let teams = rules.bases.clone();
        match own {
            Some(true) => {
                let faster = (rules.own_kill_bonus * f32::from(stacks) * 100.0).round();
                out.extend(self.toast_team(
                    team,
                    &format!("You shot your own princess! She now walks {faster:.0} % faster for the {}.", teams[usize::from(1 - team)].name),
                    BAD,
                    4.0,
                ));
                out.extend(self.toast_team(
                    1 - team,
                    &format!(
                        "The {} shot their own princess.",
                        teams[usize::from(team)].name
                    ),
                    GOOD,
                    3.0,
                ));
            }
            Some(false) => {
                out.extend(self.toast_team(
                    team,
                    &format!(
                        "The {} killed the {name}. She is back home.",
                        teams[usize::from(1 - team)].name
                    ),
                    INFO,
                    3.0,
                ));
                out.extend(self.toast_team(
                    1 - team,
                    &format!("You killed the {name}. She is back home and walks at her normal speed again."),
                    BAD,
                    4.0,
                ));
            }
            None => {}
        }
        out
    }

    /// `at` if a princess can stand there, else the nearest point towards `fallback` that she
    /// can.
    fn walkable_near(&self, at: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
        let Some(nav) = self.princess.nav.as_ref() else {
            return at;
        };
        for k in 0..=10 {
            let t = k as f32 / 10.0;
            let (x, z) = (
                at[0] + (fallback[0] - at[0]) * t,
                at[2] + (fallback[2] - at[2]) * t,
            );
            if let Some(h) = nav.height(x, z)
                && (h - (at[1] + (fallback[1] - at[1]) * t)).abs() < 3.0
            {
                return [x, h, z];
            }
        }
        fallback
    }

    fn princess_respawn_due(&mut self, i: usize, now: f32) -> Vec<Outgoing> {
        let Some((when, at)) = self.princess.princesses[i].respawn else {
            return Vec::new();
        };
        if now < when {
            return Vec::new();
        }
        let index = self.princess.princesses[i].index;
        self.princess.princesses[i].respawn = None;
        let Some(p) = self.players.get_mut(&index) else {
            return Vec::new();
        };
        p.alive = true;
        p.downed = false;
        p.health = 100.0;
        p.position = at;
        p.last_attacker = NOBODY;
        p.dirty = true;
        let facing = self.rng.range(0.0, 360.0);
        let mut out = vec![respawn_packet(&[(index, at, facing)])];
        out.extend(self.chunk_changed(index, chunk_of(at)));
        out
    }

    /// A kill against the other team: counts toward the killer's team's next helicopter.
    pub(super) fn princess_kill_credit(&mut self, killer_group: u8) -> Vec<Outgoing> {
        if killer_group >= PRINCESS_GROUP {
            return Vec::new();
        }
        let team = usize::from(killer_group);
        let size = self
            .players
            .values()
            .filter(|p| p.group == killer_group)
            .count();
        let needed = self.princess_rules().kills_per_heli(size);
        self.princess.kills[team] += 1;
        if self.princess.kills[team] < needed {
            return Vec::new();
        }
        self.princess.kills[team] = 0;
        self.princess.owed[team] += 1;
        self.pay_owed_helicopters()
    }

    /// Hand earned helicopters to a random living player of the team (bots can't fly).
    fn pay_owed_helicopters(&mut self) -> Vec<Outgoing> {
        let mut out = Vec::new();
        for team in 0..2u8 {
            while self.princess.owed[usize::from(team)] > 0 {
                let alive: Vec<u8> = self
                    .players
                    .values()
                    .filter(|p| p.group == team && p.alive && !self.is_bot(p.index))
                    .map(|p| p.index)
                    .collect();
                if alive.is_empty() {
                    // An all-bot team can't use it.
                    if !self
                        .players
                        .values()
                        .any(|p| p.group == team && !self.is_bot(p.index))
                    {
                        self.princess.owed[usize::from(team)] = 0;
                    }
                    break;
                }
                self.princess.owed[usize::from(team)] -= 1;
                let who = alive[self.rng.index(alive.len())];
                let item = self.princess_rules().heli_item;
                *self.princess.tokens.entry(who).or_default() += 1;
                out.push(self.grant(who, &[(item, 1)]));
                out.extend(self.toast(
                    who,
                    "You can now spawn a helicopter! Throw the smoke grenade where it should land.",
                    GOOD,
                    4.0,
                ));
                let name = self.players[&who].name.clone();
                let mates: Vec<u8> = self
                    .players
                    .values()
                    .filter(|p| p.group == team && p.index != who)
                    .map(|p| p.index)
                    .collect();
                for m in mates {
                    out.extend(self.toast(
                        m,
                        &format!("{name} can call in a helicopter."),
                        INFO,
                        3.0,
                    ));
                }
            }
        }
        out
    }

    /// After a respawn: the helicopter smoke grenades the player still has a right to.
    pub(super) fn princess_regrant(&mut self, peer: u8) -> Vec<Outgoing> {
        if !self.is_princess_mode() {
            return Vec::new();
        }
        let n = self.princess.tokens.get(&peer).copied().unwrap_or(0);
        if n == 0 {
            return Vec::new();
        }
        let item = self.princess_rules().heli_item;
        vec![self.grant(peer, &[(item, n.min(9) as i32)])]
    }

    /// A thrown item: a helicopter smoke grenade calls one in where it was thrown.
    pub(super) fn princess_throw(
        &mut self,
        peer: u8,
        item: i32,
        from: [f32; 3],
        direction: [f32; 3],
    ) -> Vec<Outgoing> {
        if !self.is_princess_mode() || item != self.princess_rules().heli_item {
            return Vec::new();
        }
        let Some(tokens) = self.princess.tokens.get_mut(&peer).filter(|t| **t > 0) else {
            return Vec::new();
        };
        *tokens -= 1;
        let flat = direction[0].hypot(direction[2]).max(0.001);
        let yaw = direction[0].atan2(direction[2]).to_degrees();
        let land = [
            from[0] + direction[0] / flat * 12.0,
            from[1],
            from[2] + direction[2] / flat * 12.0,
        ];
        let now = self.last_now.unwrap_or(0.0);
        self.princess
            .arriving
            .push((now + HELI_DELAY, peer, land, yaw));
        self.toast(peer, "Helicopter on its way!", GOOD, 2.5)
    }

    fn helicopters_arrive(&mut self, now: f32) -> Vec<Outgoing> {
        let due: Vec<(f32, u8, [f32; 3], f32)> = self
            .princess
            .arriving
            .iter()
            .copied()
            .filter(|a| now >= a.0)
            .collect();
        self.princess.arriving.retain(|a| now < a.0);
        let mut out = Vec::new();
        for (_, owner, at, yaw) in due {
            match self.spawn_vehicle_near(owner, "Heli", at, yaw) {
                Ok(spawned) => out.extend(spawned),
                Err(why) => {
                    // Give the grenade back so they can try somewhere else.
                    *self.princess.tokens.entry(owner).or_default() += 1;
                    let item = self.princess_rules().heli_item;
                    out.push(self.grant(owner, &[(item, 1)]));
                    out.extend(self.toast(owner, &format!("No helicopter: {why}"), BAD, 4.0));
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------ turrets

    fn tick_turrets(&mut self, now: f32) -> Vec<Outgoing> {
        let mut out = Vec::new();
        for t in 0..self.princess.turrets.len() {
            let turret = self.princess.turrets[t].clone();
            let target = self.turret_target(&turret);
            let tr = &mut self.princess.turrets[t];
            if target != tr.target {
                tr.target = target;
                tr.lock_since = now;
            }
            let Some(car) = target else {
                continue;
            };
            if now - tr.lock_since < TURRET_LOCK || now < tr.ready_at {
                continue;
            }
            tr.ready_at = now + TURRET_INTERVAL;
            out.extend(self.fire_turret_missile(turret.team, turret.position, car));
        }
        out
    }

    /// The nearest enemy aircraft in the air that the turret can see.
    fn turret_target(&self, turret: &Turret) -> Option<i32> {
        let nav = self.princess.nav.as_ref()?;
        let enemy = 1 - turret.team;
        let mut best: Option<(f32, i32)> = None;
        for &index in self.vehicles.aircraft.keys() {
            let Some(car) = self.cars.get(&index) else {
                continue;
            };
            let crewed = car.seats.iter().filter_map(|s| s.occupant).any(|o| {
                self.players
                    .get(&o)
                    .is_some_and(|p| p.alive && p.group == enemy)
            });
            if !crewed {
                continue;
            }
            let ground = nav
                .height(car.position[0], car.position[2])
                .unwrap_or(100.0);
            if car.position[1] - ground < 4.0 {
                continue;
            }
            let delta = crate::vehicles::sub(car.position, turret.position);
            let d = crate::vehicles::length(delta);
            if d > TURRET_RANGE {
                continue;
            }
            let clear = self.wall_distance(
                turret.position,
                crate::vehicles::normal(delta),
                d,
                &[Some(index)],
                0.12,
            ) >= d - 1.0;
            if clear && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, index));
            }
        }
        best.map(|(_, i)| i)
    }

    /// A turret missile reached an aircraft: two of them bring it down.
    pub(super) fn turret_hit(&mut self, car: i32) -> Vec<Outgoing> {
        let Some(maximum) = self.vehicles.aircraft.get(&car).map(|a| a.life.maximum) else {
            return Vec::new();
        };
        // Grounded aircraft are much tougher; the share is of the airborne health.
        let airborne = maximum.min(crate::vehicles::AIRCRAFT_HEALTH);
        self.aircraft_damage(NOBODY, car, airborne * TURRET_DAMAGE)
            .unwrap_or_default()
    }

    // ------------------------------------------------------------------ the client mod

    /// Event 239 from a PrincessMode client.
    pub(super) fn princess_message(&mut self, peer: u8, payload: &[u8]) -> Vec<Outgoing> {
        if payload.len() < 6 || &payload[..4] != MAGIC || payload[4] != VERSION {
            return Vec::new();
        }
        if payload[5] == HELLO && self.players.contains_key(&peer) {
            let now = self.last_now.unwrap_or(0.0);
            self.princess.clients.insert(peer, now);
        }
        if payload[5] == PICK && payload.len() == 7 {
            return self.pick_loadout(peer, payload[6]);
        }
        Vec::new()
    }

    /// `OFFER`: `[u8 n]([string name][string items])*[f32 seconds]` (n = 0 closes the menu).
    pub(super) fn offer_message(
        &self,
        peer: u8,
        choices: &[(&str, &str)],
        seconds: f32,
    ) -> Outgoing {
        let mut w = header(OFFER);
        w.u8(choices.len().min(9) as u8);
        for (name, items) in choices.iter().take(9) {
            dotnet_string(&mut w, name);
            dotnet_string(&mut w, items);
        }
        w.f32(seconds);
        Outgoing {
            to: Recipients::One(peer),
            packet: Packet {
                code: PRINCESS,
                payload: w.into_inner(),
            },
            reliable: true,
        }
    }

    pub(super) fn princess_client(&self, peer: u8) -> bool {
        let now = self.last_now.unwrap_or(0.0);
        self.princess
            .clients
            .get(&peer)
            .is_some_and(|&seen| now - seen < CLIENT_TIMEOUT)
    }

    /// A short banner: on the mod's HUD, or as a chat line without it.
    pub(super) fn toast(&self, peer: u8, text: &str, kind: u8, seconds: f32) -> Vec<Outgoing> {
        if self.is_bot(peer) || !self.players.contains_key(&peer) {
            return Vec::new();
        }
        if !self.princess_client(peer) {
            return vec![Self::chat_line(peer, text)];
        }
        let mut w = header(TOAST);
        dotnet_string(&mut w, text);
        w.u8(kind).f32(seconds);
        vec![Outgoing {
            to: Recipients::One(peer),
            packet: Packet {
                code: PRINCESS,
                payload: w.into_inner(),
            },
            reliable: true,
        }]
    }

    fn toast_team(&self, team: u8, text: &str, kind: u8, seconds: f32) -> Vec<Outgoing> {
        self.players
            .values()
            .filter(|p| p.group == team)
            .flat_map(|p| self.toast(p.index, text, kind, seconds))
            .collect()
    }

    /// The HUD state for each team's mod clients: bases, cages, turrets, both princesses (the
    /// other team's only where the team knows she is), and the helicopter progress.
    fn princess_sync(&mut self, now: f32) -> Vec<Outgoing> {
        let clients: Vec<u8> = self
            .princess
            .clients
            .keys()
            .copied()
            .filter(|&c| self.princess_client(c) && self.players.contains_key(&c))
            .collect();
        if clients.is_empty() {
            return Vec::new();
        }
        let rules = self.princess_rules().clone();
        let mut out = Vec::new();
        for team in 0..2u8 {
            let to: Vec<u8> = clients
                .iter()
                .copied()
                .filter(|c| self.players[c].group == team)
                .collect();
            if to.is_empty() {
                continue;
            }
            let size = self.players.values().filter(|p| p.group == team).count();
            let mut w = header(STATE);
            w.u8(team);
            for (b, base) in rules.bases.iter().enumerate() {
                dotnet_string(&mut w, &base.name);
                dotnet_string(&mut w, &base.princess);
                w.vec3(base.centre).f32(base.radius).vec3(base.cage);
                let turrets: Vec<&Turret> = self
                    .princess
                    .turrets
                    .iter()
                    .filter(|t| usize::from(t.team) == b)
                    .collect();
                w.u8(turrets.len() as u8);
                for t in turrets {
                    w.vec3(t.position)
                        .f32((t.ready_at - now).max(0.0))
                        .u8(u8::from(t.target.is_some()));
                }
                // The princess: index, state, known position (or none), stacks, speed, metres to go.
                let pr = self
                    .princess
                    .princesses
                    .iter()
                    .find(|p| usize::from(p.team) == b);
                let Some(pr) = pr else {
                    w.u8(NOBODY).u8(0).u8(0).u8(0).f32(1.0).f32(-1.0);
                    continue;
                };
                let alive = self.players.get(&pr.index).is_some_and(|p| p.alive);
                let state = if !alive {
                    4
                } else if pr.contested {
                    3
                } else {
                    match pr.escort {
                        Some(e) if e.team != pr.team => 2,
                        Some(_) => 1,
                        None => 0,
                    }
                };
                w.u8(pr.index).u8(state);
                let known = pr.seen[usize::from(team)]
                    .filter(|(_, t)| now - *t < 3.0 || u8::try_from(b).ok() == Some(team));
                match known {
                    Some((at, _)) => {
                        w.u8(1).vec3(at);
                    }
                    None => {
                        w.u8(0);
                    }
                }
                let speed = 1.0 + rules.own_kill_bonus * f32::from(pr.stacks);
                let to_go = pr
                    .escort
                    .filter(|e| e.team != pr.team)
                    .and_then(|e| {
                        let cage = rules.bases[usize::from(e.team)].cage;
                        let field = self
                            .princess
                            .fields
                            .iter()
                            .find(|f| flat_distance(f.goal, cage) < 0.5)?;
                        let at = self.players.get(&pr.index)?.position;
                        self.princess.nav.as_ref()?.flow_distance(field, at)
                    })
                    .unwrap_or(-1.0);
                w.u8(pr.stacks).f32(speed).f32(to_go);
            }
            let needed = rules.kills_per_heli(size);
            w.u16(self.princess.kills[usize::from(team)].min(u32::from(u16::MAX)) as u16)
                .u16(needed.min(u32::from(u16::MAX)) as u16);
            let common = w.into_inner();
            for &c in &to {
                let mut payload = common.clone();
                payload.push(self.princess.tokens.get(&c).copied().unwrap_or(0).min(255) as u8);
                out.push(Outgoing {
                    to: Recipients::One(c),
                    packet: Packet {
                        code: PRINCESS,
                        payload,
                    },
                    reliable: false,
                });
            }
        }
        out
    }

    /// A player left: forget their mod and helicopter state.
    pub(super) fn forget_princess_client(&mut self, peer: u8) {
        self.princess.clients.remove(&peer);
        self.princess.tokens.remove(&peer);
        self.princess.offers.remove(&peer);
        self.princess.last_offer.remove(&peer);
        self.princess.away_since.remove(&peer);
        if let Some(i) = self.princess_of(peer) {
            self.princess.princesses.remove(i);
        }
    }

    /// Bot skill for a team: the setting for teams without players, normal for their mates.
    pub(super) fn bot_skill_for(&self, group: u8) -> BotSkill {
        let Some(rules) = self.settings.princess.as_ref() else {
            return BotSkill::Normal;
        };
        let has_humans = self
            .players
            .values()
            .any(|p| p.group == group && !self.is_bot(p.index));
        let both = (0..2u8).all(|g| {
            self.players
                .values()
                .any(|p| p.group == g && !self.is_bot(p.index))
        });
        if has_humans && !both {
            BotSkill::Normal
        } else {
            rules.bot_skill
        }
    }
}

/// `PlayerRespawn` for several players: `[count]([index][new index][health][position][facing][curse])*`.
fn respawn_packet(entries: &[(u8, [f32; 3], f32)]) -> Outgoing {
    let mut w = Writer::new();
    w.u8(u8::try_from(entries.len()).unwrap_or(u8::MAX));
    for &(index, at, facing) in entries.iter().take(255) {
        w.u8(index).u8(index).f32(100.0);
        w.vec3(at).f32(facing).u8(u8::MAX);
    }
    Room::broadcast(EventCode::PlayerRespawn, w.into_inner())
}

/// A point a few metres ahead in the direction her own team points (`yaw`), if she can walk
/// there without leaving the territory; small turns around obstacles.
fn push_target(
    nav: &NavGrid,
    me: [f32; 3],
    yaw: f32,
    centre: [f32; 3],
    radius: f32,
) -> Option<[f32; 3]> {
    for turn in [0.0f32, 25.0, -25.0, 50.0, -50.0] {
        let a = (yaw + turn).to_radians();
        let mut last = me;
        let mut ok = true;
        for step in 1..=3 {
            let t = step as f32;
            let (x, z) = (me[0] + a.sin() * t, me[2] + a.cos() * t);
            match nav.height(x, z) {
                Some(h) if (h - last[1]).abs() <= 0.8 => last = [x, h, z],
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && flat_distance(last, centre) <= radius {
            return Some(last);
        }
    }
    None
}
