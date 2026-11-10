//! Bots (FakePlayers' successor): server-side players that join the lobby, ride the plane, jump,
//! land with a gun, walk the walkability grid towards loot and into the ring, and fight whoever
//! they can see. To clients they are ordinary players; their shots arrive as `PlayerFire` and
//! hits as `PlayerDamaged`. Players damage and kill them the normal way. In Capture the Princess
//! they play the mode (`princess_bots.rs`) at the host's chosen skill.

use std::collections::BTreeMap;
use std::sync::Arc;

use tabg_protocol::combat::DamageEvent;
use tabg_protocol::loot::chunk_of;
use tabg_protocol::match_flow::RequestAirplaneDrop;
use tabg_protocol::messages::{
    PackedDirection, PackedQuaternion, PlayerJoined, game_mode, game_state,
};
use tabg_protocol::{EventCode, Writer};

use super::{Outgoing, Recipients, Room};
use crate::nav::{EYE, NavGrid};
use crate::princess::BotSkill;

/// How often a bot thinks (seconds).
const THINK: f32 = 0.1;
const WALK_SPEED: f32 = 5.0;
const FALL_SPEED: f32 = 35.0;
/// How far a bot looks for someone to fight.
const SIGHT: f32 = 70.0;
const PATH_BUDGET: usize = 8000;
/// After landing a bot holds fire this long (players need a moment to grab a gun).
const LANDING_GRACE: f32 = 5.0;
/// Seconds before a bot opens fire on someone it just spotted (min, extra random).
const REACTION: (f32, f32) = (0.5, 0.7);
/// Bot damage relative to the gun's damage, and the best hit chance at any range.
const DAMAGE_SCALE: f32 = 0.7;
const MAX_HIT_CHANCE: f32 = 0.5;
/// Storm damage per second for bots outside the ring, by ring number (players' own clients
/// apply theirs; bots have none).
const RING_DAMAGE: [f32; 7] = [1.0, 1.0, 2.0, 3.0, 5.0, 8.0, 10.0];
/// Health a downed bot loses per second.
const BLEED: f32 = 6.0;
/// What a bot's bandage heals.
const BANDAGE: f32 = 15.0;
/// The client places a player's hip at the position it gets (its own updates send the hip, and
/// a respawn puts the feet 0.9387 m below it); bots stand on the grid with their feet.
pub(super) const HIP: f32 = 0.94;
/// Up close a bot notices anyone, whichever way it looks.
const AWARE: f32 = 5.0;
/// How long gunfire stays worth turning towards.
const NOISE_LIFE: f32 = 1.5;
/// Sprinting is this much faster than running.
const SPRINT: f32 = 1.3;
/// Strafing in a fight is this much slower.
const STRAFE: f32 = 0.7;

/// A gunshot someone may hear: where, when, and whose team.
#[derive(Clone, Copy, Debug)]
pub(super) struct Noise {
    pub at: [f32; 3],
    pub time: f32,
    pub group: u8,
}

/// How well a bot plays: reaction time (min, extra random), best hit chance, damage share, how
/// far it notices enemies, aim (times the gun's accuracy), walking speed and time between shots
/// (times the gun's).
#[derive(Clone, Copy, Debug)]
pub(super) struct Skill {
    pub reaction: (f32, f32),
    pub max_hit: f32,
    pub damage: f32,
    pub sight: f32,
    pub aim: f32,
    pub speed: f32,
    pub fire_interval: f32,
    /// Field of view in degrees (Capture the Princess; elsewhere bots see all around).
    pub fov: f32,
    /// How far gunfire is heard.
    pub hearing: f32,
    /// Fastest turn, degrees per second.
    pub turn: f32,
    /// Sidesteps in a fight.
    pub strafe: bool,
}

pub(super) fn skill(level: BotSkill) -> Skill {
    match level {
        BotSkill::Easy => Skill {
            reaction: (0.9, 0.9),
            max_hit: 0.3,
            damage: 0.5,
            sight: 50.0,
            aim: 0.65,
            speed: 4.6,
            fire_interval: 1.4,
            fov: 100.0,
            hearing: 45.0,
            turn: 240.0,
            strafe: false,
        },
        BotSkill::Normal => Skill {
            reaction: REACTION,
            max_hit: MAX_HIT_CHANCE,
            damage: DAMAGE_SCALE,
            sight: SIGHT,
            aim: 1.0,
            speed: WALK_SPEED,
            fire_interval: 1.0,
            fov: 120.0,
            hearing: 75.0,
            turn: 420.0,
            strafe: true,
        },
        BotSkill::Hard => Skill {
            reaction: (0.2, 0.35),
            max_hit: 0.7,
            damage: 0.85,
            sight: 90.0,
            aim: 1.35,
            speed: 5.5,
            fire_interval: 0.9,
            fov: 140.0,
            hearing: 100.0,
            turn: 620.0,
            strafe: true,
        },
    }
}

const NAMES: &[&str] = &[
    "Bot Alva",
    "Bot Bertil",
    "Bot Cissi",
    "Bot Dante",
    "Bot Edda",
    "Bot Frans",
    "Bot Greta",
    "Bot Hugo",
    "Bot Iris",
    "Bot Jonte",
    "Bot Kaj",
    "Bot Lova",
    "Bot Malte",
    "Bot Nora",
    "Bot Otto",
    "Bot Pia",
    "Bot Ruben",
    "Bot Saga",
    "Bot Tage",
    "Bot Ulla",
    "Bot Vide",
    "Bot Wilma",
    "Bot Yngve",
    "Bot Zelda",
];

/// A gun a bot can carry: item, its ammo, damage per hit, seconds between shots, hit chance at
/// 10 m (falling off with distance).
#[derive(Clone, Copy, Debug)]
pub(super) struct Gun {
    pub(super) item: i32,
    pub(super) ammo: i32,
    pub(super) damage: f32,
    pub(super) interval: f32,
    pub(super) accuracy: f32,
}

const GUNS: &[Gun] = &[
    Gun {
        item: 160,
        ammo: 6,
        damage: 14.0,
        interval: 0.25,
        accuracy: 0.55,
    }, // M16
    Gun {
        item: 152,
        ammo: 1,
        damage: 18.0,
        interval: 0.3,
        accuracy: 0.5,
    }, // AK-47
    Gun {
        item: 310,
        ammo: 9,
        damage: 10.0,
        interval: 0.12,
        accuracy: 0.45,
    }, // MP5-K
    Gun {
        item: 297,
        ammo: 8,
        damage: 40.0,
        interval: 1.0,
        accuracy: 0.6,
    }, // Mossberg-500
    Gun {
        item: 277,
        ammo: 9,
        damage: 16.0,
        interval: 0.45,
        accuracy: 0.5,
    }, // m1911
];

#[derive(Debug)]
pub(super) struct Brain {
    /// Fraction of the flight after which to jump.
    jump_at: f32,
    pub(super) gun: Option<Gun>,
    pub(super) path: Vec<[f32; 3]>,
    think_at: f32,
    next_shot: f32,
    pub(super) target: Option<u8>,
    /// Give up on a goal after this.
    goal_until: f32,
    /// Holds fire until then (landing grace, reaction time).
    calm_until: f32,
    /// When it was last hit.
    pub(super) hurt_at: f32,
    /// Its next bandage.
    heal_at: f32,
    /// Capture the Princess: this bot's job and what it's walking to.
    pub(super) princess: super::princess_bots::Mind,
    /// Walking its path (every tick, smoothly), sprinting, and where it looks meanwhile.
    pub(super) walking: bool,
    pub(super) sprint: bool,
    pub(super) look: Option<[f32; 3]>,
    /// When it started aiming at its target (aim settles over the first second or so).
    aim_since: f32,
    /// Where it heard something, until when it keeps looking there.
    pub(super) alert: Option<([f32; 3], f32)>,
    /// A sidestep in a fight, until when.
    strafe_until: f32,
    /// The target's last seen position and time (to judge how fast it moves).
    seen: Option<([f32; 3], f32)>,
}

impl Room {
    /// Server-side players without a client: bots and the princesses.
    pub fn is_bot(&self, peer: u8) -> bool {
        self.bots.contains_key(&peer) || self.is_princess(peer)
    }

    /// How well this bot plays.
    pub(super) fn bot_skill(&self, bot: u8) -> Skill {
        if !self.is_princess_mode() {
            return skill(BotSkill::Normal);
        }
        let group = self.players.get(&bot).map_or(0, |p| p.group);
        skill(self.bot_skill_for(group))
    }

    /// Bots play Battle Royale on the island, where there's a walkability grid.
    fn bots_wanted(&self) -> usize {
        let island =
            self.settings.game_mode == game_mode::BATTLE_ROYALE && self.settings.test_map.is_none();
        if self.settings.nav.is_none() || !island {
            return 0;
        }
        let humans = self.players.keys().filter(|p| !self.is_bot(**p)).count();
        if humans == 0 {
            return 0;
        }
        usize::from(self.settings.bots)
    }

    /// Bot player indices count down from 253, above every network peer id.
    pub(super) fn free_bot_index(&self) -> Option<u8> {
        let lowest = u16::from(self.settings.max_players) + 2;
        (lowest..=253)
            .rev()
            .map(|i| i as u8)
            .find(|i| !self.players.contains_key(i) && !self.pending.contains(i))
    }

    /// Add or remove bots to match the setting (only in the lobby).
    pub(super) fn manage_bots(&mut self) -> Vec<Outgoing> {
        // Capture the Princess fills its teams until the match starts (the countdown may begin
        // with the first player).
        if self.is_princess_mode()
            && matches!(
                self.state,
                game_state::WAITING_FOR_PLAYERS | game_state::COUNT_DOWN
            )
        {
            return self.manage_princess_lobby();
        }
        if self.state != game_state::WAITING_FOR_PLAYERS {
            return Vec::new();
        }
        let wanted = self.bots_wanted();
        let mut out = Vec::new();
        while self.bots.len() > wanted {
            let Some(&bot) = self.bots.keys().next() else {
                break;
            };
            self.bots.remove(&bot);
            out.extend(self.disconnect(bot));
        }
        while self.bots.len() < wanted {
            let Some(index) = self.free_bot_index() else {
                break;
            };
            let squad = 0xB07_0000 + u64::from(index);
            let group = self.group_for(index, squad);
            match self.add_bot_as(index, group, squad) {
                Some(joined) => out.push(joined),
                None => break,
            }
        }
        out
    }

    /// A new bot in `group` (Capture the Princess teams), announced to everyone.
    pub(super) fn add_bot(&mut self, group: u8) -> Option<Outgoing> {
        let index = self.free_bot_index()?;
        self.add_bot_as(index, group, 0xB07_0000 + u64::from(index))
    }

    fn add_bot_as(&mut self, index: u8, group: u8, squad: u64) -> Option<Outgoing> {
        let name = NAMES[usize::from(253 - index) % NAMES.len()].to_owned();
        let spawn = self.spawn_for(group);
        let mut p = super::Player::new(index, group, name.clone(), format!("bot:{index}"), squad);
        p.in_world = true;
        p.position = spawn.position;
        p.rotation = [0.0, spawn.rotation];
        p.chunk = chunk_of(spawn.position);
        self.players.insert(index, p);
        self.bots.insert(
            index,
            Brain {
                jump_at: 0.0,
                gun: None,
                path: Vec::new(),
                think_at: 0.0,
                next_shot: 0.0,
                target: None,
                goal_until: 0.0,
                calm_until: 0.0,
                hurt_at: f32::MIN,
                heal_at: 0.0,
                princess: Default::default(),
                walking: false,
                sprint: false,
                look: None,
                aim_since: 0.0,
                alert: None,
                strafe_until: 0.0,
                seen: None,
            },
        );
        Some(Outgoing::reliable(
            Recipients::All,
            EventCode::Login,
            PlayerJoined {
                player_index: index,
                group_index: group,
                player_name: name,
                gear: Vec::new(),
                is_dev: false,
                colour: 0,
            }
            .encode(),
        ))
    }

    /// A new match: every bot picks when to jump and drops its gun.
    pub(super) fn bots_board(&mut self) {
        let jumps: Vec<(u8, f32)> = self
            .bots
            .keys()
            .map(|&b| {
                (
                    b,
                    self.settings
                        .bot_jump
                        .unwrap_or(0.15 + 0.7 * self.rng.unit()),
                )
            })
            .collect();
        for (bot, at) in jumps {
            if let Some(brain) = self.bots.get_mut(&bot) {
                brain.jump_at = at;
                brain.gun = None;
                brain.path.clear();
                brain.target = None;
                brain.calm_until = 0.0;
                brain.princess = Default::default();
            }
        }
    }

    /// A bot died: it takes out a new gun when it's back, and starts its errand over.
    pub(super) fn bot_died(&mut self, bot: u8) {
        if let Some(b) = self.bots.get_mut(&bot) {
            b.gun = None;
            b.path.clear();
            b.target = None;
            b.walking = false;
            b.look = None;
            b.alert = None;
            b.seen = None;
            b.princess.reset_errand();
        }
    }

    fn nav(&self) -> Option<Arc<NavGrid>> {
        self.settings.nav.clone()
    }

    pub(super) fn tick_bots(&mut self, now: f32, dt: f32) -> Vec<Outgoing> {
        let mut out = self.manage_bots();
        if self.state != game_state::STARTED || self.bots.is_empty() {
            return out;
        }
        let nav = if self.is_princess_mode() {
            self.princess_nav()
        } else {
            self.nav()
        };
        let Some(nav) = nav else {
            return out;
        };
        self.noises.retain(|n| now - n.time < NOISE_LIFE);
        let bots: Vec<u8> = self.bots.keys().copied().collect();
        for bot in bots {
            out.extend(self.tick_bot(bot, now, dt, &nav));
        }
        if now >= self.bot_ring_at {
            self.bot_ring_at = now + 1.0;
            if self.is_princess_mode() {
                out.extend(self.bots_bleed());
            } else {
                out.extend(self.bot_ring_damage(now));
            }
        }
        out
    }

    /// Out of a fight for a few seconds, a hurt bot uses a bandage now and then, as players do
    /// (Capture the Princess, where fights go on all match).
    fn bot_patch_up(&mut self, bot: u8, now: f32) -> Vec<Outgoing> {
        let Some(b) = self.bots.get(&bot) else {
            return Vec::new();
        };
        let quiet = now - b.hurt_at > 4.0 && now >= b.heal_at;
        let Some(p) = self
            .players
            .get_mut(&bot)
            .filter(|p| quiet && p.alive && !p.downed && p.health < 75.0)
        else {
            return Vec::new();
        };
        p.health = (p.health + BANDAGE).min(100.0);
        let health = p.health;
        if let Some(b) = self.bots.get_mut(&bot) {
            b.heal_at = now + 2.5;
        }
        vec![Outgoing::reliable(
            Recipients::Many(self.near(bot)),
            EventCode::PlayerHealed,
            tabg_protocol::actions::encode_player_health(bot, health),
        )]
    }

    /// Downed bots bleed out (players' own clients do that for them; nobody revives a bot).
    fn bots_bleed(&mut self) -> Vec<Outgoing> {
        let downed: Vec<u8> = self
            .bots
            .keys()
            .copied()
            .filter(|b| self.players.get(b).is_some_and(|p| p.alive && p.downed))
            .collect();
        let mut out = Vec::new();
        for bot in downed {
            let p = self.players.get_mut(&bot).expect("filtered");
            p.health -= BLEED;
            if p.health <= 0.0 {
                out.extend(self.kill(bot, false));
            } else {
                let health = p.health;
                out.push(Outgoing::reliable(
                    Recipients::Many(self.near(bot)),
                    EventCode::PlayerHealthStateChanged,
                    tabg_protocol::actions::encode_player_health(bot, health),
                ));
            }
        }
        out
    }

    /// Bots outside the storm's edge lose health every second, and die a ring death at 0.
    fn bot_ring_damage(&mut self, now: f32) -> Vec<Outgoing> {
        if self.settings.game_mode != game_mode::BATTLE_ROYALE || self.settings.test_map.is_some() {
            return Vec::new();
        }
        let (centre, size) = self.ring.current(now);
        let per_second = RING_DAMAGE[self.ring.id.min(RING_DAMAGE.len() - 1)];
        let outside: Vec<u8> = self
            .bots
            .keys()
            .copied()
            .filter(|b| {
                self.players.get(b).is_some_and(|p| {
                    p.alive
                        && p.landed
                        && (p.position[0] - centre[0]).hypot(p.position[2] - centre[2]) > size * 0.5
                })
            })
            .collect();
        let mut out = Vec::new();
        for bot in outside {
            let p = self.players.get_mut(&bot).expect("filtered");
            p.health = (p.health - per_second).max(0.0);
            let health = p.health;
            if health <= 0.0 {
                out.extend(self.kill(bot, true));
            } else {
                out.push(Outgoing::reliable(
                    Recipients::Many(self.near(bot)),
                    EventCode::PlayerHealthStateChanged,
                    tabg_protocol::actions::encode_player_health(bot, health),
                ));
            }
        }
        out
    }

    fn tick_bot(&mut self, bot: u8, now: f32, dt: f32, nav: &NavGrid) -> Vec<Outgoing> {
        let Some(p) = self.players.get(&bot) else {
            return Vec::new();
        };
        if !p.alive || p.downed {
            return Vec::new();
        }
        let mut out = Vec::new();
        // In the plane: jump when it's time.
        if !p.dropped {
            let progress = self.plane.as_ref().map_or(1.0, |plane| plane.progress(now));
            // Jump at the chosen moment, but only over land (else wait; the plane drops
            // everyone at the end anyway).
            let over_land = self.plane.as_ref().is_some_and(|plane| {
                let at = plane.drop_position(now);
                nav.height(at[0], at[2]).is_some()
            });
            if progress >= self.bots[&bot].jump_at && over_land {
                let req = RequestAirplaneDrop {
                    player_index: bot,
                    direction: [0.0, -1.0, 0.0],
                };
                out.extend(self.airplane_drop(bot, req));
            }
            return out;
        }
        // Falling: straight down to the ground, then land and take out the gun.
        if !p.landed {
            let pos = p.position;
            let ground = nav.height(pos[0], pos[2]).unwrap_or(109.0);
            let y = (pos[1] - FALL_SPEED * dt).max(ground);
            let p = self.players.get_mut(&bot).expect("checked");
            p.position[1] = y;
            p.dirty = true;
            if y <= ground + 0.01 {
                p.landed = true;
                let mut w = Writer::new();
                w.u8(bot).vec3(p.position);
                out.push(Outgoing::reliable(
                    Recipients::All,
                    EventCode::PlayerLand,
                    w.into_inner(),
                ));
                if let Some(b) = self.bots.get_mut(&bot) {
                    b.calm_until = now + LANDING_GRACE;
                }
                out.extend(self.bot_arm(bot));
            }
            return out;
        }
        // Started on the ground (Test mode, respawns): take out a gun first.
        if self.bots[&bot].gun.is_none() {
            out.extend(self.bot_arm(bot));
        }
        // Moving and turning happen every tick, so others see a smooth walk.
        out.extend(self.bot_motion(bot, dt, nav));
        if now < self.bots[&bot].think_at {
            return out;
        }
        if let Some(b) = self.bots.get_mut(&bot) {
            b.think_at = now + THINK;
        }
        let step = THINK.max(dt);
        let me = self.players[&bot].position;
        let skill = self.bot_skill(bot);
        let princess_mode = self.is_princess_mode();
        // Capture the Princess: what to do (an escort ignores fights that are far away).
        let plan = princess_mode.then(|| self.princess_bot_plan(bot, now, nav));
        // An escort keeps its gun on the princess unless someone is close or shooting at it.
        let hurt = now - self.bots[&bot].hurt_at < 3.0;
        let sight = match &plan {
            Some(plan) if plan.escorting && !hurt => skill.sight.min(22.0),
            _ => skill.sight,
        };
        let enemy = self.bot_spot(bot, sight, hurt, nav);
        if let Some((target, pos, d)) = enemy {
            if let Some(b) = self.bots.get_mut(&bot) {
                b.look = Some([pos[0], pos[1] + 1.0, pos[2]]);
                b.alert = None;
            }
            let reaction = skill.reaction.0 + skill.reaction.1 * self.rng.unit() + d / 150.0;
            if let Some(b) = self.bots.get_mut(&bot)
                && b.target != Some(target)
            {
                b.target = Some(target);
                b.calm_until = b.calm_until.max(now + reaction);
                b.aim_since = now + reaction;
                b.seen = None;
            }
            if now >= self.bots[&bot].calm_until {
                out.extend(self.bot_shoot(bot, target, pos, d, now));
            }
            let face = Some([pos[0], pos[1] + 1.0, pos[2]]);
            match plan {
                // Capture the Princess: keep going while shooting at someone far off
                // (standing in a long-range duel wins nothing), still facing them.
                Some(plan) if d > 20.0 && plan.goal.is_some() && !plan.escorting => {
                    let plan = super::princess_bots::Plan { face, ..plan };
                    out.extend(self.princess_bot_move(bot, &plan, now, nav));
                    if let Some(b) = self.bots.get_mut(&bot) {
                        b.sprint = false;
                    }
                }
                Some(plan) if !plan.escorting && skill.strafe => {
                    self.bot_strafe(bot, pos, now, nav)
                }
                _ => self.bot_stand(bot),
            }
            return out;
        }
        if let Some(b) = self.bots.get_mut(&bot) {
            b.target = None;
            b.seen = None;
        }
        if princess_mode {
            self.bot_listen(bot, now);
            out.extend(self.bot_patch_up(bot, now));
        }
        // In the water: swim to the nearest shore.
        if nav.height(me[0], me[2]).is_none() {
            self.bot_stand(bot);
            out.extend(self.bot_swim(bot, step, nav));
            return out;
        }
        if let Some(plan) = plan {
            out.extend(self.princess_bot_move(bot, &plan, now, nav));
            return out;
        }
        // Otherwise walk: into the ring if outside it, else somewhere nearby.
        let needs_goal = {
            let b = &self.bots[&bot];
            b.path.is_empty() || now > b.goal_until
        };
        if needs_goal {
            let goal = self.bot_goal(me, nav);
            let path = goal
                .and_then(|g| nav.path(me, g, PATH_BUDGET))
                .unwrap_or_default();
            if let Some(b) = self.bots.get_mut(&bot) {
                b.path = path;
                b.goal_until = now + 30.0;
                b.target = None;
            }
        }
        if let Some(b) = self.bots.get_mut(&bot) {
            b.look = None;
        }
        self.bot_walk(bot, false);
        out
    }

    /// The enemy this bot notices: the nearest one in sight. In Capture the Princess only in its
    /// field of view (or right next to it, or the one it already fights, or the one shooting
    /// it).
    fn bot_spot(
        &self,
        bot: u8,
        sight: f32,
        hurt: bool,
        nav: &NavGrid,
    ) -> Option<(u8, [f32; 3], f32)> {
        let me = self.players[&bot].position;
        let group = self.players[&bot].group;
        let facing = self.players[&bot].rotation[1];
        let shooter = self.players[&bot].last_attacker;
        let princess_mode = self.is_princess_mode();
        let skill = self.bot_skill(bot);
        let current = self.bots[&bot].target;
        self.players
            .values()
            .filter(|o| {
                o.index != bot
                    && o.alive
                    && o.dropped
                    && o.group != group
                    && !(princess_mode && self.is_princess(o.index))
                    // A downed enemy can't fight back; finish it only on the way.
                    && !(princess_mode && o.downed && dist(me, o.position) > 10.0)
            })
            .map(|o| (o.index, o.position, dist(me, o.position)))
            .filter(|&(index, pos, d)| {
                if d >= sight {
                    return false;
                }
                if !princess_mode || d < AWARE || (hurt && index == shooter) {
                    return true;
                }
                let toward = (pos[0] - me[0]).atan2(pos[2] - me[2]).to_degrees();
                // The one it fights stays seen a little outside the view while it turns.
                let wide = if Some(index) == current { 1.4 } else { 1.0 };
                angle_between(facing, toward) <= skill.fov * 0.5 * wide
            })
            .filter(|(_, pos, d)| {
                // Under an arch or a roof edge the grid sees a wall; up close, trust eyes.
                (princess_mode && *d < 12.0 && (pos[1] - me[1]).abs() < 3.0)
                    || nav.line_of_sight(me, *pos)
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
    }

    /// Gunfire and running feet nearby: turn that way for a while.
    fn bot_listen(&mut self, bot: u8, now: f32) {
        let me = self.players[&bot].position;
        let group = self.players[&bot].group;
        let hearing = self.bot_skill(bot).hearing;
        let shot = self
            .noises
            .iter()
            .filter(|n| n.group != group && now - n.time < NOISE_LIFE && dist(me, n.at) < hearing)
            .min_by(|a, b| dist(me, a.at).total_cmp(&dist(me, b.at)))
            .map(|n| n.at);
        // Footsteps: anyone very close, sprinting players a bit further (movement bit 3).
        let steps = self
            .players
            .values()
            .filter(|o| {
                o.group != group
                    && o.alive
                    && o.index != bot
                    && !self.is_princess(o.index)
                    && (dist(me, o.position) < 4.0
                        || (o.movement & 0x08 != 0 && dist(me, o.position) < 12.0))
            })
            .map(|o| o.position)
            .next();
        if let Some(at) = shot.or(steps)
            && let Some(b) = self.bots.get_mut(&bot)
        {
            b.alert = Some((at, now + 3.5));
        }
        if let Some(b) = self.bots.get_mut(&bot) {
            match b.alert {
                Some((at, until)) if now < until => b.look = Some([at[0], at[1] + 1.0, at[2]]),
                Some(_) => {
                    b.alert = None;
                    b.look = None;
                }
                None => {}
            }
        }
    }

    /// Stand still (fighting, waiting).
    fn bot_stand(&mut self, bot: u8) {
        if let Some(b) = self.bots.get_mut(&bot) {
            b.walking = false;
            b.sprint = false;
        }
    }

    /// Sidestep left or right while fighting `at`, a few metres at a time.
    fn bot_strafe(&mut self, bot: u8, at: [f32; 3], now: f32, nav: &NavGrid) {
        if now < self.bots[&bot].strafe_until {
            return;
        }
        let me = self.players[&bot].position;
        let side = if self.rng.unit() < 0.5 { 1.0 } else { -1.0 };
        let (dx, dz) = (at[0] - me[0], at[2] - me[2]);
        let d = dx.hypot(dz).max(0.01);
        let far = 2.0 + 2.5 * self.rng.unit();
        let (x, z) = (me[0] + dz / d * side * far, me[2] - dx / d * side * far);
        let ground = nav.height(x, z).filter(|h| (h - me[1]).abs() < 0.8);
        let until = now + 0.7 + 0.9 * self.rng.unit();
        let b = self.bots.get_mut(&bot).expect("bot");
        b.strafe_until = until;
        b.sprint = false;
        match ground {
            Some(h) => {
                b.path = vec![[x, h, z]];
                b.walking = true;
            }
            None => b.walking = false,
        }
    }

    /// One tick of moving along the path and turning towards where it looks (or walks), with
    /// what the client needs to animate it: the walking direction (world space) and the
    /// forward/backward/strafe/sprint bits (its `InputHandler` movement flags).
    fn bot_motion(&mut self, bot: u8, dt: f32, nav: &NavGrid) -> Vec<Outgoing> {
        let skill = self.bot_skill(bot);
        let (walking, sprint, look, next, fighting) = {
            let b = &self.bots[&bot];
            (
                b.walking,
                b.sprint,
                b.look,
                b.path.first().copied(),
                b.target.is_some(),
            )
        };
        let me = self.players[&bot].position;
        let mut moved = [0.0f32; 3];
        let mut out = Vec::new();
        if walking
            && let Some(next) = next
            && dt > 0.0
        {
            let speed = skill.speed
                * if sprint {
                    SPRINT
                } else if fighting {
                    STRAFE
                } else {
                    1.0
                };
            let travel = speed * dt;
            let (dx, dz) = (next[0] - me[0], next[2] - me[2]);
            let d = dx.hypot(dz);
            let pos = if d <= travel {
                if let Some(b) = self.bots.get_mut(&bot) {
                    b.path.remove(0);
                }
                [next[0], nav.walk_height(me[1], next[0], next[2]), next[2]]
            } else {
                let (x, z) = (me[0] + dx / d * travel, me[2] + dz / d * travel);
                [x, nav.walk_height(me[1], x, z), z]
            };
            moved = [pos[0] - me[0], 0.0, pos[2] - me[2]];
            let old_chunk = self.players[&bot].chunk;
            let p = self.players.get_mut(&bot).expect("bot");
            p.position = pos;
            let chunk = chunk_of(pos);
            if chunk != old_chunk {
                out.extend(self.chunk_changed(bot, chunk));
            }
        }
        let flat = moved[0].hypot(moved[2]);
        let me = self.players[&bot].position;
        let p = self.players.get_mut(&bot).expect("bot");
        // Turn towards where it looks, else where it walks, no faster than it can.
        let want = match look {
            Some(at) if (at[0] - me[0]).hypot(at[2] - me[2]) > 0.2 => {
                Some((at[0] - me[0]).atan2(at[2] - me[2]).to_degrees())
            }
            _ if flat > 0.001 => Some(moved[0].atan2(moved[2]).to_degrees()),
            _ => None,
        };
        let before = (p.rotation, p.direction, p.movement, p.ads);
        if let Some(want) = want {
            let yaw = p.rotation[1];
            let limit = skill.turn * dt.max(0.0);
            let turned = yaw + signed_angle(yaw, want).clamp(-limit, limit);
            p.rotation[1] = turned.rem_euclid(360.0);
        }
        p.rotation[0] = match look {
            Some(at) => {
                let d = (at[0] - me[0]).hypot(at[2] - me[2]).max(0.01);
                -((at[1] - (me[1] + EYE)) / d).atan().to_degrees()
            }
            None => 0.0,
        };
        // Legs: the way it walks, relative to the way it faces.
        if flat > 0.001 {
            let walk = moved[0].atan2(moved[2]).to_degrees();
            let rel = angle_between(p.rotation[1], walk);
            let mut bits = if rel < 50.0 {
                0x01
            } else if rel > 130.0 {
                0x02
            } else {
                0x04
            };
            if sprint && rel < 50.0 {
                bits |= 0x08;
            }
            p.direction = PackedDirection::pack([moved[0] / flat, 0.0, moved[2] / flat]);
            p.movement = bits;
        } else {
            p.direction = PackedDirection::ZERO;
            p.movement = 0;
        }
        p.ads = fighting;
        if flat > 0.001 || before != (p.rotation, p.direction, p.movement, p.ads) {
            p.dirty = true;
        }
        out
    }

    fn bot_goal(&mut self, me: [f32; 3], nav: &NavGrid) -> Option<[f32; 3]> {
        let (centre, size) = (self.ring.white_center, self.ring.white_size * 0.45);
        let outside = dist(me, [centre[0], me[1], centre[2]]) > size;
        let target = if outside {
            // A step towards the ring centre (paths are local; long trips go in legs).
            let dir = norm([centre[0] - me[0], 0.0, centre[2] - me[2]]);
            [me[0] + dir[0] * 60.0, me[1], me[2] + dir[2] * 60.0]
        } else {
            me
        };
        nav.random_point_near(target, if outside { 20.0 } else { 50.0 }, &mut self.rng)
    }

    /// Walk the path from now on (the moving happens every tick in `bot_motion`).
    pub(super) fn bot_walk(&mut self, bot: u8, sprint: bool) {
        if let Some(b) = self.bots.get_mut(&bot) {
            b.walking = true;
            b.sprint = sprint;
        }
    }

    fn bot_swim(&mut self, bot: u8, step: f32, nav: &NavGrid) -> Vec<Outgoing> {
        let me = self.players[&bot].position;
        // Nearest land on rings of growing radius.
        let mut shore = None;
        'search: for r in (4..400).step_by(4) {
            let r = r as f32;
            for k in 0..32 {
                let a = k as f32 / 32.0 * std::f32::consts::TAU;
                let (x, z) = (me[0] + r * a.sin(), me[2] + r * a.cos());
                if let Some(y) = nav.height(x, z) {
                    shore = Some([x, y, z]);
                    break 'search;
                }
            }
        }
        let Some(shore) = shore else {
            return Vec::new();
        };
        let dir = norm([shore[0] - me[0], 0.0, shore[2] - me[2]]);
        let travel = WALK_SPEED * 0.6 * step;
        let pos = [me[0] + dir[0] * travel, 109.0, me[2] + dir[2] * travel];
        self.face(bot, shore);
        let old_chunk = self.players[&bot].chunk;
        let p = self.players.get_mut(&bot).expect("bot");
        p.position = pos;
        p.direction = PackedDirection::pack([0.0, 0.0, 1.0]);
        p.dirty = true;
        if let Some(b) = self.bots.get_mut(&bot) {
            b.path.clear();
        }
        let chunk = chunk_of(pos);
        if chunk != old_chunk {
            return self.chunk_changed(bot, chunk);
        }
        Vec::new()
    }

    /// Look at `at` (it turns there over the next ticks).
    pub(super) fn face(&mut self, bot: u8, at: [f32; 3]) {
        if let Some(b) = self.bots.get_mut(&bot) {
            b.look = Some(at);
        }
    }

    /// On landing: a random gun (and its ammo) in hand.
    fn bot_arm(&mut self, bot: u8) -> Vec<Outgoing> {
        let gun = GUNS[self.rng.index(GUNS.len())];
        self.set_bot_gun(bot, gun)
    }

    /// `gun` (and its ammo) in hand, shown to everyone.
    pub(super) fn set_bot_gun(&mut self, bot: u8, gun: Gun) -> Vec<Outgoing> {
        if let Some(b) = self.bots.get_mut(&bot) {
            b.gun = Some(gun);
        }
        self.inventory_add(bot, gun.item, 1);
        self.inventory_add(bot, gun.ammo, 120);
        // WeaponChanged: [player][slot][w1][w1 attachment][w2][w2 attachment][w3][n attachments]
        let mut w = Writer::new();
        w.u8(bot).u8(1);
        w.u16(gun.item as u16)
            .u16(u16::MAX)
            .u16(u16::MAX)
            .u16(u16::MAX)
            .u16(u16::MAX)
            .u8(0);
        let payload = w.into_inner();
        self.track_equipment(bot, &payload);
        vec![Outgoing::reliable(
            Recipients::All,
            EventCode::WeaponChanged,
            payload,
        )]
    }

    fn bot_shoot(&mut self, bot: u8, target: u8, at: [f32; 3], d: f32, now: f32) -> Vec<Outgoing> {
        let Some(gun) = self.bots[&bot].gun else {
            return Vec::new();
        };
        if now < self.bots[&bot].next_shot {
            return Vec::new();
        }
        let skill = self.bot_skill(bot);
        // Only once it actually faces them (it turns at a human speed).
        let me = self.players[&bot].position;
        let toward = (at[0] - me[0]).atan2(at[2] - me[2]).to_degrees();
        let off = angle_between(self.players[&bot].rotation[1], toward);
        if self.is_princess_mode() && off > 12.0 + (1.5 / d.max(1.0)).atan().to_degrees() {
            return Vec::new();
        }
        if let Some(b) = self.bots.get_mut(&bot) {
            b.next_shot = now + gun.interval * skill.fire_interval * (0.8 + 0.4 * self.rng.unit());
        }
        let eye = [me[0], me[1] + EYE, me[2]];
        let aim = norm([at[0] - eye[0], at[1] + 1.2 - eye[1], at[2] - eye[2]]);
        // PlayerFire: [player][mode Semi|ContainsDirection][i32 ammo][position][rotation]
        let mut w = Writer::new();
        w.u8(bot).u8(0x01 | 0x10).i32(gun.ammo).vec3(eye);
        let rotation = look_rotation(aim);
        w.bytes(&PackedQuaternion::pack(rotation).0);
        let mut to = self.near(bot);
        to.retain(|&p| p != bot);
        let mut out = vec![Outgoing {
            to: Recipients::Many(to),
            packet: tabg_protocol::Packet::new(EventCode::PlayerFire, w.into_inner()),
            reliable: false,
        }];
        let mut chance =
            (gun.accuracy * skill.aim * (10.0 / d.max(10.0)).powf(0.6)).clamp(0.05, skill.max_hit);
        if self.is_princess_mode() {
            let group = self.players[&bot].group;
            self.noises.push(Noise {
                at: eye,
                time: now,
                group,
            });
            // The aim settles over the first second and a half; moving targets and moving
            // shooters miss more.
            let b = &self.bots[&bot];
            let settle = ((now - b.aim_since) / 1.5).clamp(0.0, 1.0);
            chance *= 0.35 + 0.65 * settle;
            if let Some((was, when)) = b.seen
                && now - when > 0.05
                && dist(was, at) / (now - when) > 2.5
            {
                chance *= 0.7;
            }
            if b.walking {
                chance *= 0.8;
            }
            self.bots.get_mut(&bot).expect("bot").seen = Some((at, now));
        }
        if self.rng.unit() < chance {
            let health = self.players.get(&target).map_or(0.0, |p| p.health);
            let ev = DamageEvent {
                attacker: bot,
                victim: target,
                // Not clamped at 0: a downed target only takes a share of it, and clamping
                // would make every hit take a share of what's left, so it never died.
                new_health: health - gun.damage * skill.damage,
                direction: aim,
                hit: None,
                exclude_victim: false,
            };
            out.extend(self.damage(bot, ev));
        }
        out
    }

    /// What a bot is up to, for the radar: state, where it's walking, whom it's fighting, and
    /// its gun.
    pub(super) fn bot_view(&self, bot: u8) -> Option<BotView> {
        let (b, p) = (self.bots.get(&bot)?, self.players.get(&bot)?);
        let swimming = self
            .settings
            .nav
            .as_ref()
            .is_some_and(|n| n.height(p.position[0], p.position[2]).is_none());
        let state = if !p.dropped {
            BotState::Plane
        } else if !p.landed {
            BotState::Falling
        } else if b.target.is_some() {
            BotState::Fighting
        } else if swimming {
            BotState::Swimming
        } else {
            BotState::Walking
        };
        Some(BotView {
            state,
            goal: b.path.last().copied(),
            target: b.target,
            gun: b.gun.map(|g| g.item),
        })
    }

    /// Forget bots that left (they are removed with the room's players on reset).
    pub(super) fn forget_bot(&mut self, peer: u8) {
        self.bots.remove(&peer);
    }
}

/// Smallest angle between two headings, in degrees (0..180).
fn angle_between(a: f32, b: f32) -> f32 {
    signed_angle(a, b).abs()
}

/// Turn from heading `a` to `b` the short way, in degrees (-180..180).
fn signed_angle(a: f32, b: f32) -> f32 {
    (b - a + 540.0).rem_euclid(360.0) - 180.0
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// Quaternion (x, y, z, w) looking along `dir` (Unity's LookRotation with up = Y).
fn look_rotation(dir: [f32; 3]) -> [f32; 4] {
    let yaw = dir[0].atan2(dir[2]);
    let pitch = -dir[1].asin();
    let (sy, cy) = (yaw * 0.5).sin_cos();
    let (sp, cp) = (pitch * 0.5).sin_cos();
    // yaw about Y, then pitch about X
    [cy * sp, sy * cp, -sy * sp, cy * cp]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum BotState {
    Plane = 0,
    Falling = 1,
    Walking = 2,
    Fighting = 3,
    Swimming = 4,
}

pub(super) struct BotView {
    pub state: BotState,
    pub goal: Option<[f32; 3]>,
    pub target: Option<u8>,
    pub gun: Option<i32>,
}

pub(super) type Brains = BTreeMap<u8, Brain>;
