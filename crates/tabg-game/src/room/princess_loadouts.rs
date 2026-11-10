//! Capture the Princess loadouts: after every (re)spawn, and when a player comes back into their
//! own territory after a while away, the server offers three random loadouts from a pool. With the
//! PrincessMode mod they pick one with 1, 2 or 3 within 30 s (else they get the first); without
//! it they get one straight away. Bots take a random one too, and fight with its main gun.

use super::bots::Gun;
use super::princess::{GOOD, INFO};
use super::{Outgoing, Room};

/// How long an offer stays open.
pub(super) const OFFER_SECONDS: f32 = 30.0;
/// Away from your territory at least this long before you get a new offer back home.
const AWAY_SECONDS: f32 = 30.0;

/// A loadout: what players see, what they get, and the gun bots fight with when they take it.
pub(super) struct Loadout {
    pub name: &'static str,
    pub summary: &'static str,
    pub items: &'static [(i32, i32)],
    pub bot_gun: Gun,
}

const fn gun(item: i32, ammo: i32, damage: f32, interval: f32, accuracy: f32) -> Gun {
    Gun {
        item,
        ammo,
        damage,
        interval,
        accuracy,
    }
}

// Ammo item ids: 1 big, 2 bolts, 6 normal, 8 shotgun, 9 small. Guns use the ammo their prefab
// names (AWP big, SCAR-H normal, Vector small, …).
pub(super) const LOADOUTS: &[Loadout] = &[
    Loadout {
        name: "Sniper",
        summary: "AWP, Glockinator, 3 bandages",
        items: &[(317, 1), (1, 30), (305, 1), (9, 120), (131, 3)],
        bot_gun: gun(317, 1, 60.0, 1.6, 0.8),
    },
    Loadout {
        name: "Assault",
        summary: "SCAR-H, Beretta 93R, 2 grenades, 3 bandages",
        items: &[(162, 1), (6, 180), (264, 1), (9, 90), (197, 2), (131, 3)],
        bot_gun: gun(162, 6, 17.0, 0.22, 0.55),
    },
    Loadout {
        name: "Rusher",
        summary: "Vector, AA-12, 3 bandages",
        items: &[(315, 1), (9, 240), (292, 1), (8, 40), (131, 3)],
        bot_gun: gun(315, 9, 10.0, 0.08, 0.45),
    },
    Loadout {
        name: "Heavy",
        summary: "MG-42, a grenade, a med kit",
        items: &[(220, 1), (6, 300), (197, 1), (132, 1)],
        bot_gun: gun(220, 6, 14.0, 0.07, 0.38),
    },
    Loadout {
        name: "Rocketeer",
        summary: "Missile launcher, P90, 2 bandages",
        items: &[(179, 1), (1, 6), (311, 1), (9, 200), (131, 2)],
        bot_gun: gun(311, 9, 9.0, 0.07, 0.45),
    },
    Loadout {
        name: "Samurai",
        summary: "Katana, Mossberg-500, 2 med kits",
        items: &[(254, 1), (297, 1), (8, 40), (132, 2)],
        bot_gun: gun(297, 8, 40.0, 1.0, 0.6),
    },
    Loadout {
        name: "Marksman",
        summary: "Kar98K, MP5-K, 3 bandages",
        items: &[(321, 1), (1, 40), (310, 1), (9, 150), (131, 3)],
        bot_gun: gun(321, 1, 50.0, 1.4, 0.75),
    },
    Loadout {
        name: "Gunslinger",
        summary: "Revolver, Glockinator, 3 grenades",
        items: &[(280, 1), (6, 60), (305, 1), (9, 120), (197, 3), (131, 2)],
        bot_gun: gun(280, 6, 30.0, 0.5, 0.6),
    },
    Loadout {
        name: "Hunter",
        summary: "Gaussbow, Famas, 3 bandages",
        items: &[(172, 1), (2, 20), (156, 1), (6, 150), (131, 3)],
        bot_gun: gun(156, 6, 13.0, 0.12, 0.5),
    },
    Loadout {
        name: "Grenadier",
        summary: "MGL, UMP-45, 2 dynamite",
        items: &[(203, 1), (1, 24), (314, 1), (9, 150), (214, 2), (131, 2)],
        bot_gun: gun(314, 9, 12.0, 0.1, 0.5),
    },
    Loadout {
        name: "Minigun",
        summary: "Mini Gun, 3 bandages",
        items: &[(178, 1), (6, 400), (131, 3)],
        bot_gun: gun(178, 6, 11.0, 0.05, 0.35),
    },
    Loadout {
        name: "Rifleman",
        summary: "M16, Tec-9, 2 grenades",
        items: &[(160, 1), (6, 240), (313, 1), (9, 120), (197, 2), (131, 2)],
        bot_gun: gun(160, 6, 14.0, 0.25, 0.55),
    },
];

/// An open choice: three loadouts (indices into [`LOADOUTS`]) until a time.
#[derive(Clone, Copy, Debug)]
pub(super) struct Offer {
    pub choices: [usize; 3],
    pub until: f32,
}

impl Room {
    /// Three loadouts, none of them the ones offered last time if the pool allows.
    fn draw_loadouts(&mut self, peer: u8) -> [usize; 3] {
        let last = self.princess.last_offer.get(&peer).copied();
        let mut pool: Vec<usize> = (0..LOADOUTS.len())
            .filter(|i| last.is_none_or(|l| !l.contains(i)))
            .collect();
        if pool.len() < 3 {
            pool = (0..LOADOUTS.len()).collect();
        }
        let mut pick = [0; 3];
        for slot in &mut pick {
            let i = self.rng.index(pool.len());
            *slot = pool.swap_remove(i);
        }
        self.princess.last_offer.insert(peer, pick);
        pick
    }

    /// After a (re)spawn: bots take a random loadout, players get to choose.
    pub(super) fn princess_loadout(&mut self, peer: u8) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        self.princess.away_since.remove(&peer);
        if self.bots.contains_key(&peer) {
            let pick = self.rng.index(LOADOUTS.len());
            return self.set_bot_gun(peer, LOADOUTS[pick].bot_gun);
        }
        self.offer_loadouts(peer, now)
    }

    fn offer_loadouts(&mut self, peer: u8, now: f32) -> Vec<Outgoing> {
        let choices = self.draw_loadouts(peer);
        if !self.princess_client(peer) {
            // Without the mod there is no menu: the first of the three.
            return self.take_loadout(peer, choices[0]);
        }
        self.princess.offers.insert(
            peer,
            Offer {
                choices,
                until: now + OFFER_SECONDS,
            },
        );
        let names: Vec<(&str, &str)> = choices
            .iter()
            .map(|&i| (LOADOUTS[i].name, LOADOUTS[i].summary))
            .collect();
        let mut out = vec![self.offer_message(peer, &names, OFFER_SECONDS)];
        out.extend(self.toast(peer, "Choose your loadout: press 1, 2 or 3", INFO, 3.0));
        out
    }

    /// `PICK` from the mod: `[u8 choice 0..3]`.
    pub(super) fn pick_loadout(&mut self, peer: u8, choice: u8) -> Vec<Outgoing> {
        let Some(offer) = self.princess.offers.remove(&peer) else {
            return Vec::new();
        };
        let Some(&index) = offer.choices.get(usize::from(choice)) else {
            self.princess.offers.insert(peer, offer);
            return Vec::new();
        };
        if !self.players.get(&peer).is_some_and(|p| p.alive) {
            return Vec::new();
        }
        self.take_loadout(peer, index)
    }

    fn take_loadout(&mut self, peer: u8, index: usize) -> Vec<Outgoing> {
        let l = &LOADOUTS[index];
        let mut out = vec![self.grant(peer, l.items)];
        out.extend(self.toast(
            peer,
            &format!("Loadout: {} ({})", l.name, l.summary),
            GOOD,
            3.0,
        ));
        out.push(self.offer_message(peer, &[], 0.0));
        out
    }

    /// Offers that ran out give the first choice; players back home after a while get a new one.
    pub(super) fn tick_loadouts(&mut self, now: f32) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let expired: Vec<(u8, usize)> = self
            .princess
            .offers
            .iter()
            .filter(|(_, o)| now >= o.until)
            .map(|(&p, o)| (p, o.choices[0]))
            .collect();
        for (peer, index) in expired {
            self.princess.offers.remove(&peer);
            if self.players.get(&peer).is_some_and(|p| p.alive) {
                out.extend(self.take_loadout(peer, index));
            }
        }
        let rules = self.princess_rules().clone();
        let humans: Vec<(u8, u8, [f32; 3], bool)> = self
            .players
            .values()
            .filter(|p| p.group < 2 && !self.is_bot(p.index))
            .map(|p| (p.index, p.group, p.position, p.alive))
            .collect();
        for (peer, team, at, alive) in humans {
            let base = &rules.bases[usize::from(team)];
            let home = (at[0] - base.centre[0]).hypot(at[2] - base.centre[2]) <= base.radius;
            if !alive || self.princess.offers.contains_key(&peer) {
                continue;
            }
            match (home, self.princess.away_since.get(&peer).copied()) {
                (false, None) => {
                    self.princess.away_since.insert(peer, now);
                }
                (true, Some(since)) => {
                    self.princess.away_since.remove(&peer);
                    if now - since >= AWAY_SECONDS && self.princess_client(peer) {
                        out.extend(self.offer_loadouts(peer, now));
                    }
                }
                _ => {}
            }
        }
        out
    }
}
