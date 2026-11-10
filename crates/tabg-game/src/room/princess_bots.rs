//! Bots in Capture the Princess. Each think a bot picks one job, in this order: win back its
//! own princess when the other team is taking her; bring her home when she's out on her own;
//! escort or guard the other princess when its team has her; go and take her (search the
//! hideouts until someone has seen her); or guard its own princess. Fights come first (in
//! `bots.rs`), except that an escort only fights enemies close to it.
//!
//! Long ways go down the flow fields towards the cages and hideouts, short ones by A* on the
//! grid, and a bot that stops making progress takes a short detour.

use super::{Outgoing, Room};
use crate::nav::NavGrid;

/// Beyond this a goal is reached by legs or flow fields, not one A* search.
const LOCAL: f32 = 60.0;
/// A bot holding a princess at gunpoint stays about this far from her.
const ESCORT_DISTANCE: f32 = 4.5;
/// Within this a bot takes the other princess (holds its gun on her).
const TAKE_DISTANCE: f32 = 9.0;
/// How far from home her team starts bringing her back.
const AWAY_FROM_HOME: f32 = 8.0;
/// Defenders rush to a princess being taken from this far.
const RESCUE_RANGE: f32 = 260.0;
/// Attackers wait for each other this far from the other princess (inner and outer edge).
const RALLY: (f32, f32) = (100.0, 150.0);
const PATH_BUDGET: usize = 9000;
/// A gateway is at most this long; a bot farther from the far end than that was pulled away.
const MAX_CROSSING: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    Attack,
    Defend,
}

/// A bot's memory for the mode.
#[derive(Clone, Debug, Default)]
pub(super) struct Mind {
    path_goal: Option<[f32; 3]>,
    path_at: f32,
    /// The last search for `path_goal` found no way.
    no_path: bool,
    /// The other base's hideout it searches next.
    search: usize,
    patrol: Option<([f32; 3], f32)>,
    /// Where it was and when (to notice being stuck).
    progress: Option<(f32, [f32; 3])>,
    detour: Option<([f32; 3], f32)>,
    /// The princess it holds at gunpoint (escorting her), if any.
    pub(super) holding: Option<u8>,
    /// Since when it waits for teammates before going into the other base.
    rally_since: Option<f32>,
    /// The way in it picked for this attack (`None`: not picked yet), and whether it got there.
    flank: Option<([f32; 3], bool)>,
    /// Through a gateway: the far side it walks to before following the field again.
    crossing: Option<[f32; 3]>,
}

impl Mind {
    pub(super) fn reset_errand(&mut self) {
        self.path_goal = None;
        self.patrol = None;
        self.progress = None;
        self.detour = None;
        self.flank = None;
        self.rally_since = None;
        self.crossing = None;
    }
}

/// What a bot wants this think.
#[derive(Clone, Debug, Default)]
pub(super) struct Plan {
    /// Where to walk (`None`: stay).
    pub goal: Option<[f32; 3]>,
    /// Close enough to the goal to stop.
    pub near: f32,
    /// Keep looking at this (holding a princess at gunpoint).
    pub face: Option<[f32; 3]>,
    pub escorting: bool,
    /// The princess it holds at gunpoint.
    pub holding: Option<u8>,
}

fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

fn toward(from: [f32; 3], to: [f32; 3], metres: f32) -> [f32; 3] {
    let d = flat(from, to).max(0.001);
    [
        from[0] + (to[0] - from[0]) / d * metres,
        from[1],
        from[2] + (to[2] - from[2]) / d * metres,
    ]
}

/// The nearest walkable spot within 6 m of `me` (on about its level) that `ok` accepts.
fn unstick(nav: &NavGrid, me: [f32; 3], ok: impl Fn([f32; 3]) -> bool) -> Option<[f32; 3]> {
    for r in 1..=6 {
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            let (x, z) = (me[0] + a.sin() * r as f32, me[2] + a.cos() * r as f32);
            if let Some(h) = nav.height(x, z)
                && (h - me[1]).abs() < 2.0
                && ok([x, h, z])
            {
                return Some([x, h, z]);
            }
        }
    }
    None
}

/// Can a walker go straight from `a` to `b` (no step too high, no water)? Ground far above
/// counts as a wall: from the side, a house or a castle wall looks just like an arch, and
/// gateways are walked through their links instead.
fn straight_walkable(nav: &NavGrid, a: [f32; 3], b: [f32; 3]) -> bool {
    let d = flat(a, b);
    let steps = (d / 0.5).ceil().max(1.0) as usize;
    let mut y = a[1];
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let (x, z) = (a[0] + (b[0] - a[0]) * t, a[2] + (b[2] - a[2]) * t);
        match nav.height(x, z) {
            Some(h) if (h - y).abs() <= 0.6 => y = h,
            _ => return false,
        }
    }
    true
}

impl Room {
    /// Attack or defend: about a third of each team defends (bots only; players do as they like).
    fn princess_role(&self, bot: u8) -> Role {
        let Some(group) = self.team_of(bot) else {
            return Role::Attack;
        };
        let members = self
            .players
            .values()
            .filter(|p| p.group == group && !self.is_princess(p.index))
            .count();
        let bots: Vec<u8> = self
            .bots
            .keys()
            .copied()
            .filter(|b| self.players.get(b).is_some_and(|p| p.group == group))
            .collect();
        if bots.len() < 2 {
            return Role::Attack;
        }
        let defenders = ((members as f32 / 3.0).round() as usize).clamp(1, bots.len() - 1);
        match bots.iter().position(|&b| b == bot) {
            Some(i) if i < defenders => Role::Defend,
            _ => Role::Attack,
        }
    }

    /// The living bot of `group` nearest to `at`.
    fn nearest_bot(&self, group: u8, at: [f32; 3]) -> Option<u8> {
        self.bots
            .keys()
            .filter_map(|b| self.players.get(b))
            .filter(|p| p.group == group && p.alive && !p.downed)
            .min_by(|a, b| flat(a.position, at).total_cmp(&flat(b.position, at)))
            .map(|p| p.index)
    }

    pub(super) fn princess_bot_plan(&mut self, bot: u8, now: f32, nav: &NavGrid) -> Plan {
        let plan = self.choose_plan(bot, now, nav);
        if let Some(b) = self.bots.get_mut(&bot) {
            b.princess.holding = plan.holding;
        }
        plan
    }

    fn choose_plan(&mut self, bot: u8, now: f32, nav: &NavGrid) -> Plan {
        let Some(team) = self.team_of(bot) else {
            return Plan::default();
        };
        let enemy = 1 - team;
        let me = self.players[&bot].position;
        let role = self.princess_role(bot);
        let chest = |p: [f32; 3]| [p[0], p[1] + 0.9, p[2]];

        // Our princess.
        if let Some(mine) = self.princess_index(team)
            && let Some(pos) = self
                .players
                .get(&mine)
                .filter(|p| p.alive)
                .map(|p| p.position)
        {
            let escort = self.princess_escort(team);
            let home = self
                .princess
                .princesses
                .iter()
                .find(|p| p.team == team)
                .map_or(pos, |p| p.home);
            if escort.is_some_and(|e| e.team == enemy) {
                if role == Role::Defend || flat(me, pos) < RESCUE_RANGE {
                    return Plan {
                        goal: Some(pos),
                        near: 5.0,
                        ..Plan::default()
                    };
                }
            } else if flat(pos, home) > AWAY_FROM_HOME {
                let ours_holding = escort.is_some_and(|e| e.team == team);
                let holder_is_me = escort.is_some_and(|e| e.by == bot);
                let human_holds = escort.is_some_and(|e| e.team == team && !self.is_bot(e.by));
                if holder_is_me
                    || (!human_holds && !ours_holding && self.nearest_bot(team, pos) == Some(bot))
                {
                    // Stand on the far side of her from home: holding the gun on her walks
                    // (or pushes) her home.
                    let stand = toward(pos, home, -3.5);
                    return Plan {
                        goal: (flat(me, stand) > 1.5).then_some(stand),
                        near: 1.2,
                        face: Some(chest(pos)),
                        escorting: true,
                        holding: Some(mine),
                    };
                }
            }
        }

        // The other team's princess.
        if let Some(theirs) = self.princess_index(enemy)
            && let Some(pos) = self
                .players
                .get(&theirs)
                .filter(|p| p.alive)
                .map(|p| p.position)
        {
            let escort = self.princess_escort(enemy);
            match escort {
                Some(e) if e.team == team && e.by == bot => {
                    // Escorting: follow a few metres behind, gun on her.
                    let d = flat(me, pos);
                    let goal = if d > ESCORT_DISTANCE + 2.0 || d < ESCORT_DISTANCE - 2.0 {
                        Some(toward(pos, me, ESCORT_DISTANCE))
                    } else {
                        None
                    };
                    return Plan {
                        goal,
                        near: 1.0,
                        face: Some(chest(pos)),
                        escorting: true,
                        holding: Some(theirs),
                    };
                }
                Some(e) if e.team == team => {
                    // Someone else escorts: guard her, spread around.
                    let a = f32::from(bot) * 2.4;
                    let spot = [pos[0] + a.sin() * 8.0, pos[1], pos[2] + a.cos() * 8.0];
                    let goal = nav
                        .random_point_near(spot, 2.0, &mut self.rng)
                        .unwrap_or(pos);
                    return Plan {
                        goal: Some(goal),
                        near: 3.0,
                        ..Plan::default()
                    };
                }
                _ => {}
            }
            let known = self.princess_known(enemy, team);
            let close = known.is_some_and(|(at, _)| flat(me, at) < 40.0);
            if role == Role::Attack || close {
                let target = known.map(|(at, _)| at).unwrap_or_else(|| {
                    let b = &self.princess_rules().bases[usize::from(enemy)];
                    b.hideouts.first().copied().unwrap_or(b.centre)
                });
                if let Some(way_in) = self.flank(bot, team, target, nav) {
                    return way_in;
                }
                if let Some(wait) = self.rally(bot, team, target, now) {
                    return wait;
                }
                if let Some((at, _)) = known {
                    if flat(me, pos) < TAKE_DISTANCE && nav.line_of_sight(me, pos) {
                        return Plan {
                            goal: None,
                            near: 0.0,
                            face: Some(chest(pos)),
                            escorting: true,
                            holding: Some(theirs),
                        };
                    }
                    return Plan {
                        goal: Some(at),
                        near: 4.0,
                        ..Plan::default()
                    };
                }
                // Nobody has seen her: search the hideouts one after another.
                let hideouts = self.princess_rules().bases[usize::from(enemy)]
                    .hideouts
                    .clone();
                if !hideouts.is_empty() {
                    let mind = &mut self.bots.get_mut(&bot).expect("bot").princess;
                    let mut h = hideouts[mind.search % hideouts.len()];
                    if flat(me, h) < 7.0 {
                        mind.search += 1;
                        h = hideouts[mind.search % hideouts.len()];
                    }
                    return Plan {
                        goal: Some(h),
                        near: 4.0,
                        ..Plan::default()
                    };
                }
            }
        }

        // Guard our princess (or our cage while she's away).
        let anchor = self
            .princess_index(team)
            .and_then(|i| self.players.get(&i))
            .filter(|p| p.alive)
            .map(|p| p.position)
            .unwrap_or(self.princess_rules().bases[usize::from(team)].cage);
        let patrol = self.bots[&bot].princess.patrol;
        let goal = match patrol {
            Some((g, until)) if now < until && flat(g, anchor) < 16.0 => g,
            _ => {
                let a = self.rng.range(0.0, std::f32::consts::TAU);
                let r = self.rng.range(7.0, 14.0);
                let spot = [anchor[0] + a.sin() * r, anchor[1], anchor[2] + a.cos() * r];
                let g = nav
                    .random_point_near(spot, 2.0, &mut self.rng)
                    .unwrap_or(anchor);
                let until = now + self.rng.range(6.0, 11.0);
                self.bots.get_mut(&bot).expect("bot").princess.patrol = Some((g, until));
                g
            }
        };
        Plan {
            goal: Some(goal),
            near: 2.0,
            ..Plan::default()
        }
    }

    /// Everyone walking the shortest way meets in the middle: normal and hard bots come in from
    /// the side, through a point off the direct line about 110 m before the target.
    fn flank(&mut self, bot: u8, team: u8, target: [f32; 3], nav: &NavGrid) -> Option<Plan> {
        if self.bot_skill_for(team) == crate::princess::BotSkill::Easy {
            return None;
        }
        let me = self.players[&bot].position;
        let home = self.princess_rules().bases[usize::from(team)].cage;
        let picked = self.bots[&bot].princess.flank;
        let (point, reached) = match picked {
            Some(f) => f,
            None => {
                if flat(me, target) < 170.0 {
                    // Already close (respawned next to it, or chasing): no detour.
                    self.bots.get_mut(&bot).expect("bot").princess.flank = Some((target, true));
                    return None;
                }
                let back = (home[0] - target[0]).atan2(home[2] - target[2]);
                let turn = self.rng.range(-0.9, 0.9);
                let a = back + turn;
                let spot = [
                    target[0] + a.sin() * 125.0,
                    target[1],
                    target[2] + a.cos() * 125.0,
                ];
                let point = nav
                    .random_point_near(spot, 15.0, &mut self.rng)
                    .unwrap_or(target);
                // Only a way in that really leads there (not across water or up a cliff).
                let usable = self
                    .field_near(target)
                    .and_then(|f| nav.flow_distance(&f, point))
                    .is_some_and(|d| d < 220.0);
                let flank = if usable {
                    (point, false)
                } else {
                    (target, true)
                };
                self.bots.get_mut(&bot).expect("bot").princess.flank = Some(flank);
                flank
            }
        };
        if reached || flat(me, point) < 12.0 || flat(me, target) < flat(point, target) {
            if !reached {
                self.bots.get_mut(&bot).expect("bot").princess.flank = Some((point, true));
            }
            return None;
        }
        Some(Plan {
            goal: Some(point),
            near: 8.0,
            ..Plan::default()
        })
    }

    /// Going into the other base alone is how bots die: wait at its edge for a teammate (normal)
    /// or two (hard) for a while. Easy bots just run in.
    fn rally(&mut self, bot: u8, team: u8, target: [f32; 3], now: f32) -> Option<Plan> {
        let me = self.players[&bot].position;
        let d = flat(me, target);
        let (patience, wanted) = match self.bot_skill_for(team) {
            crate::princess::BotSkill::Easy => return None,
            crate::princess::BotSkill::Normal => (12.0, 2),
            crate::princess::BotSkill::Hard => (20.0, 3),
        };
        if !(RALLY.0..RALLY.1).contains(&d) {
            if d < RALLY.0 {
                self.bots.get_mut(&bot).expect("bot").princess.rally_since = None;
            }
            return None;
        }
        let near = self
            .players
            .values()
            .filter(|p| {
                p.group == team
                    && p.alive
                    && !p.downed
                    && (flat(p.position, me) < 30.0 || flat(p.position, target) < RALLY.0)
            })
            .count();
        let attackers = self
            .players
            .values()
            .filter(|p| p.group == team && p.alive && !self.is_princess(p.index))
            .count()
            .saturating_sub(1)
            .max(1);
        let since = *self
            .bots
            .get_mut(&bot)
            .expect("bot")
            .princess
            .rally_since
            .get_or_insert(now);
        if near >= wanted.min(attackers) || now - since > patience {
            return None;
        }
        Some(Plan {
            goal: None,
            near: 0.0,
            face: Some([target[0], target[1] + 1.0, target[2]]),
            ..Plan::default()
        })
    }

    pub(super) fn princess_bot_move(
        &mut self,
        bot: u8,
        plan: &Plan,
        now: f32,
        nav: &NavGrid,
    ) -> Vec<Outgoing> {
        let me = self.players[&bot].position;
        let out = Vec::new();
        let mut goal = plan.goal;
        // Stuck for a few seconds: a short detour.
        if goal.is_some_and(|g| flat(me, g) > plan.near) {
            let mind = &mut self.bots.get_mut(&bot).expect("bot").princess;
            match mind.progress {
                Some((since, at)) if now - since > 3.0 => {
                    if flat(at, me) < 1.2 && mind.detour.is_none() {
                        let point = nav.random_point_near(me, 10.0, &mut self.rng);
                        let mind = &mut self.bots.get_mut(&bot).expect("bot").princess;
                        mind.detour = point.map(|p| (p, now + 2.5));
                        mind.path_goal = None;
                    }
                    let mind = &mut self.bots.get_mut(&bot).expect("bot").princess;
                    mind.progress = Some((now, me));
                }
                None => mind.progress = Some((now, me)),
                _ => {}
            }
        } else {
            self.bots.get_mut(&bot).expect("bot").princess.progress = None;
        }
        {
            let mind = &mut self.bots.get_mut(&bot).expect("bot").princess;
            match mind.detour {
                Some((p, until)) if now < until && flat(me, p) > 1.0 => goal = Some(p),
                Some(_) => mind.detour = None,
                None => {}
            }
        }
        if let Some(goal) = goal.filter(|g| flat(me, *g) > plan.near) {
            self.route(bot, me, goal, now, nav);
            // Long ways are sprinted, unless it holds a princess at gunpoint.
            let sprint = !plan.escorting && flat(me, goal) > 25.0;
            self.bot_walk(bot, sprint);
        } else if let Some(b) = self.bots.get_mut(&bot) {
            b.path.clear();
            b.walking = false;
        }
        if let Some(b) = self.bots.get_mut(&bot) {
            // Looking: at the princess it holds, else where it walks (or what it heard).
            if plan.face.is_some() {
                b.look = plan.face;
            } else if b.alert.is_none() {
                b.look = None;
            }
        }
        out
    }

    /// Fill the bot's path towards `goal`: straight when clear, A* nearby, flow fields or legs
    /// further away.
    fn route(&mut self, bot: u8, me: [f32; 3], goal: [f32; 3], now: f32, nav: &NavGrid) {
        let d = flat(me, goal);
        if d < 15.0 && straight_walkable(nav, me, goal) {
            let b = self.bots.get_mut(&bot).expect("bot");
            b.path = vec![goal];
            b.princess.path_goal = Some(goal);
            return;
        }
        // Halfway through a gateway: finish it first (unless a fight or a detour took it
        // away from the gate; then walking straight there would go through walls).
        if let Some(far) = self.bots[&bot].princess.crossing {
            if flat(far, me) > 0.6 && flat(far, me) < MAX_CROSSING {
                self.bots.get_mut(&bot).expect("bot").path = vec![far];
                return;
            }
            self.bots.get_mut(&bot).expect("bot").princess.crossing = None;
        }
        if d > LOCAL {
            // A flow field whose goal is near ours (cages and hideouts) leads all the way.
            if let Some(field) = self.field_near(goal) {
                // Off the field (pressed into a tree or a ledge): step back onto it first.
                let next = nav.flow_next(&field, me, 4).or_else(|| {
                    unstick(nav, me, |p| nav.flow_distance(&field, p).is_some()).map(|p| (p, false))
                });
                if let Some((next, gateway)) = next {
                    let b = self.bots.get_mut(&bot).expect("bot");
                    b.path = vec![next];
                    b.princess.path_goal = None;
                    if gateway {
                        b.princess.crossing = Some(next);
                    }
                    return;
                }
            }
        }
        // A search that found nothing isn't repeated every think (it's the most expensive).
        let fresh = {
            let b = &self.bots[&bot];
            b.princess.path_goal.is_some_and(|g| flat(g, goal) < 3.0)
                && if b.path.is_empty() {
                    b.princess.no_path && now - b.princess.path_at < 1.5
                } else {
                    now - b.princess.path_at < 4.0
                }
        };
        if fresh {
            return;
        }
        // A leg of at most LOCAL metres towards the goal.
        let target = if d > LOCAL {
            let leg = toward(me, goal, LOCAL * 0.8);
            nav.random_point_near(leg, 8.0, &mut self.rng)
                .unwrap_or(leg)
        } else {
            goal
        };
        let path = nav.path(me, target, PATH_BUDGET).or_else(|| {
            // Unreachable as asked: try a point near it.
            let near = nav.random_point_near(target, 6.0, &mut self.rng)?;
            nav.path(me, near, PATH_BUDGET)
        });
        // Standing somewhere the grid can't leave (a tree's edge): step off it.
        let path = path.or_else(|| {
            let free = unstick(nav, me, |p| nav.path(p, target, 400).is_some())?;
            Some(vec![free])
        });
        // No way across (water, cliffs): head for the base nearest the goal the long way round.
        let path = path.or_else(|| {
            let cages: Vec<[f32; 3]> = self.princess_rules().bases.iter().map(|b| b.cage).collect();
            let cage = cages
                .into_iter()
                .min_by(|a, b| flat(*a, goal).total_cmp(&flat(*b, goal)))?;
            let field = self.field_to(cage)?;
            nav.flow_step(&field, me, 4).map(|next| vec![next])
        });
        let b = self.bots.get_mut(&bot).expect("bot");
        b.princess.no_path = path.is_none();
        b.path = path.unwrap_or_default();
        b.princess.path_goal = Some(goal);
        b.princess.path_at = now;
    }
}
