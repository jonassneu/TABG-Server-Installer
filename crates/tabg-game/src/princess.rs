//! Capture the Princess: two teams, each with a base, a princess hidden in it and a cage next to
//! where the team spawns. A player who holds a gun on the other team's princess makes her walk
//! to their cage; bringing her in wins. This file holds the rules and the island's two bases
//! (Actual Castle and New Castle); the match logic is in `room/princess.rs`.

/// Outfit slots in gear order (head, torso, legs, feet, armour, helmet), each `(item, colour)`.
pub type Outfit = [i32; 12];

const PRINCESS_HAIR: i32 = 315;
const SWEET_DRESS: i32 = 483;
const CROWN: i32 = 105;
/// Gear colour index of a strong pink (the dress's own colour is turquoise blue).
const PINK: i32 = 25;

/// One team's home.
#[derive(Clone, Debug, PartialEq)]
pub struct PrincessBase {
    /// Team name players see ("Castle").
    pub name: String,
    /// The princess's player name ("Blue Princess").
    pub princess: String,
    pub outfit: Outfit,
    /// Middle of the base and how far its territory reaches.
    pub centre: [f32; 3],
    pub radius: f32,
    /// Where the other team's princess must be brought; the team respawns around it.
    pub cage: [f32; 3],
    /// Where the princess may hide at the start of a match (one is picked at random).
    pub hideouts: Vec<[f32; 3]>,
    /// Anti-aircraft turrets that fire homing missiles at the other team's helicopters.
    pub turrets: Vec<[f32; 3]>,
    /// Gateways into the base the walkability grid shows as walls (arches): pairs of points on
    /// either side.
    pub gates: Vec<([f32; 3], [f32; 3])>,
}

/// How well bots play.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BotSkill {
    Easy,
    #[default]
    Normal,
    Hard,
}

/// Capture the Princess settings.
#[derive(Clone, Debug, PartialEq)]
pub struct PrincessRules {
    /// Team 0 and team 1.
    pub bases: [PrincessBase; 2],
    /// Players per team; bots fill the free places (0 = no bots).
    pub team_size: u8,
    /// How well the bots of a team without players play (bots on a team with players play at
    /// normal, so a harder setting means harder opponents, not better helpers).
    pub bot_skill: BotSkill,
    /// The princess's walking speed in m/s while someone holds a gun on her.
    pub walk_speed: f32,
    /// Each time her own team kills her she walks this much faster for the other team
    /// (0.15 = 15 %), up to `own_kill_max` times. A kill by the other team resets it.
    pub own_kill_bonus: f32,
    pub own_kill_max: u8,
    /// Kills against the other team that earn a helicopter (0 = by team size).
    pub heli_kills: u8,
    /// The item that calls in a helicopter where it is thrown (a smoke grenade).
    pub heli_item: i32,
    /// Bases defend themselves against helicopters.
    pub turrets: bool,
}

/// Princess hair, a dress in `colour` (-1: its own turquoise blue) and a crown.
fn outfit(colour: i32) -> Outfit {
    [
        PRINCESS_HAIR,
        -1,
        SWEET_DRESS,
        colour,
        -1,
        -1,
        -1,
        -1,
        -1,
        -1,
        CROWN,
        -1,
    ]
}

impl PrincessRules {
    /// The island: Actual Castle (blue princess) against New Castle (pink princess), about
    /// 350 m apart; each cage is on the side facing the other castle.
    pub fn island() -> Self {
        Self {
            bases: [
                PrincessBase {
                    name: "Actual Castle".into(),
                    princess: "Blue Princess".into(),
                    outfit: outfit(-1),
                    centre: [-698.0, 135.0, 545.0],
                    radius: 95.0,
                    cage: [-658.5, 139.4, 555.5],
                    // In the courtyard, on the far side from the cage.
                    hideouts: vec![
                        [-724.0, 135.0, 553.0],
                        [-699.5, 135.1, 510.5],
                        [-731.5, 138.5, 516.5],
                    ],
                    turrets: vec![[-678.5, 145.5, 497.5], [-656.5, 146.0, 564.5]],
                    // The five gates in the outer wall (the passage lies under the gatehouse).
                    gates: vec![
                        ([-737.6, 134.8, 542.1], [-742.4, 134.3, 543.7]),
                        ([-705.6, 135.0, 503.4], [-707.4, 134.7, 498.8]),
                        ([-709.2, 134.8, 580.9], [-711.8, 134.1, 585.1]),
                        ([-669.9, 135.3, 576.8], [-673.1, 137.4, 573.0]),
                        ([-644.9, 138.3, 523.1], [-653.6, 138.9, 526.8]),
                    ],
                },
                PrincessBase {
                    name: "New Castle".into(),
                    princess: "Pink Princess".into(),
                    outfit: outfit(PINK),
                    centre: [-387.0, 121.0, 388.0],
                    radius: 75.0,
                    cage: [-398.5, 120.3, 380.5],
                    // Inside the round tower, and in the courtyard's east part.
                    hideouts: vec![
                        [-346.0, 120.3, 392.5],
                        [-362.5, 120.3, 365.5],
                        [-359.5, 120.3, 383.5],
                    ],
                    turrets: vec![[-388.5, 129.5, 384.5], [-359.5, 128.6, 414.5]],
                    // The round tower's four gates and the gate to the courtyard's south.
                    gates: vec![
                        ([-372.0, 120.3, 392.2], [-367.0, 120.3, 392.4]),
                        ([-355.9, 120.3, 391.1], [-350.9, 120.3, 391.3]),
                        ([-356.6, 120.3, 392.4], [-356.4, 120.3, 397.4]),
                        ([-356.9, 120.3, 385.2], [-356.7, 120.3, 390.2]),
                        ([-398.8, 120.3, 362.6], [-399.0, 120.3, 357.1]),
                    ],
                },
            ],
            ..Self::default()
        }
    }

    /// Enemy kills a team needs for a helicopter: the setting, or about one per teammate
    /// (3 in teams of three, 8 in teams of ten).
    pub fn kills_per_heli(&self, team_size: usize) -> u32 {
        if self.heli_kills > 0 {
            return u32::from(self.heli_kills);
        }
        (team_size as u32).clamp(3, 8)
    }

    /// The area both bases and the way between them lie in (`[x, z]` min and max), for the
    /// bots' flow fields.
    pub fn region(&self) -> ([f32; 2], [f32; 2]) {
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for b in &self.bases {
            let r = b.radius + 150.0;
            min = [min[0].min(b.centre[0] - r), min[1].min(b.centre[2] - r)];
            max = [max[0].max(b.centre[0] + r), max[1].max(b.centre[2] + r)];
        }
        (min, max)
    }
}

impl Default for PrincessRules {
    fn default() -> Self {
        let base = |name: &str, princess: &str, x: f32| PrincessBase {
            name: name.into(),
            princess: princess.into(),
            outfit: outfit(if x < 0.0 { -1 } else { PINK }),
            centre: [x, 120.0, 0.0],
            radius: 90.0,
            cage: [x, 120.0, 40.0],
            hideouts: vec![[x - 20.0, 120.0, -30.0], [x + 20.0, 120.0, -30.0]],
            turrets: Vec::new(),
            gates: Vec::new(),
        };
        Self {
            bases: [
                base("Castle", "Blue Princess", -300.0),
                base("City", "Pink Princess", 300.0),
            ],
            team_size: 3,
            bot_skill: BotSkill::Normal,
            walk_speed: 3.5,
            own_kill_bonus: 0.15,
            own_kill_max: 3,
            heli_kills: 0,
            heli_item: 208,
            turrets: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helicopters_need_about_one_kill_per_teammate() {
        let r = PrincessRules::default();
        assert_eq!(r.kills_per_heli(3), 3);
        assert_eq!(r.kills_per_heli(5), 5);
        assert_eq!(r.kills_per_heli(10), 8);
        assert_eq!(r.kills_per_heli(1), 3);
        let fixed = PrincessRules { heli_kills: 6, ..r };
        assert_eq!(fixed.kills_per_heli(3), 6);
    }

    #[test]
    fn the_island_bases_are_apart_and_dressed_differently() {
        let r = PrincessRules::island();
        let [a, b] = &r.bases;
        let d = (a.cage[0] - b.cage[0]).hypot(a.cage[2] - b.cage[2]);
        assert!(d > a.radius + b.radius, "territories don't overlap: {d}");
        assert_ne!(a.outfit, b.outfit);
        let (min, max) = r.region();
        for base in &r.bases {
            for p in base.hideouts.iter().chain([&base.cage]) {
                assert!(p[0] > min[0] && p[0] < max[0] && p[2] > min[1] && p[2] < max[1]);
                let from_centre = (p[0] - base.centre[0]).hypot(p[2] - base.centre[2]);
                assert!(
                    from_centre < base.radius,
                    "{} inside its territory",
                    base.name
                );
            }
        }
    }
}
