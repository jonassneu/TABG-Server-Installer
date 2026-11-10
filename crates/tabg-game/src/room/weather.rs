//! Weather for the Visuals client mod (our event 247, `WTHR` v1). Weather is cosmetic and drawn by
//! each client; by default every client picks it from the biome and the synced time of day. An
//! admin's `/weather <name>` overrides that for every modded player, including ones who join later
//! (they get it when they say hello), until `/weather auto`.

use tabg_protocol::{Packet, Writer};

use super::{Outgoing, Recipients, Room};

pub const WEATHER: u8 = 247;
const MAGIC: &[u8; 4] = b"WTHR";
const VERSION: u8 = 1;
const HELLO: u8 = 1;
const SET: u8 = 2;

/// The weathers the Visuals mod knows, as it names them.
pub const NAMES: [&str; 10] = [
    "Clear",
    "Overcast",
    "Mist",
    "Rain",
    "Storm",
    "Flurries",
    "Snowfall",
    "Blizzard",
    "Haze",
    "Sandstorm",
];

#[derive(Debug, Default)]
pub(super) struct WeatherState {
    /// Peers whose mod said hello.
    clients: Vec<u8>,
    /// The admin's choice; `None` lets every client follow the biome.
    forced: Option<&'static str>,
    /// The host's setting as last applied (it wins over `/weather` only when it changes).
    setting: Option<&'static str>,
}

/// `name` in any case, or `None` if the mod has no such weather.
pub(super) fn parse(name: &str) -> Option<&'static str> {
    NAMES.iter().copied().find(|n| n.eq_ignore_ascii_case(name))
}

fn set_packet(name: &str) -> Packet {
    let mut w = Writer::new();
    for &b in MAGIC {
        w.u8(b);
    }
    w.u8(VERSION).u8(SET).u8(name.len() as u8);
    for &b in name.as_bytes() {
        w.u8(b);
    }
    Packet {
        code: WEATHER,
        payload: w.into_inner(),
    }
}

impl Room {
    pub(super) fn weather_message(&mut self, peer: u8, payload: &[u8]) -> Vec<Outgoing> {
        if payload.len() < 6
            || &payload[..4] != MAGIC
            || payload[4] != VERSION
            || payload[5] != HELLO
        {
            return Vec::new();
        }
        if !self.weather.clients.contains(&peer) {
            self.weather.clients.push(peer);
        }
        match self.weather.forced {
            Some(name) => vec![Outgoing {
                to: Recipients::One(peer),
                packet: set_packet(name),
                reliable: true,
            }],
            None => Vec::new(),
        }
    }

    pub(super) fn weather_forget(&mut self, peer: u8) {
        self.weather.clients.retain(|&p| p != peer);
    }

    /// Force `name` for every modded player, or hand the weather back to the biome with `None`.
    /// Returns the packets and how many players will see it.
    pub(super) fn set_weather(&mut self, name: Option<&'static str>) -> (Vec<Outgoing>, usize) {
        self.weather.forced = name;
        let clients = self.weather.clients.len();
        if clients == 0 {
            return (Vec::new(), 0);
        }
        let out = vec![Outgoing {
            to: Recipients::Many(self.weather.clients.clone()),
            packet: set_packet(name.unwrap_or("Auto")),
            reliable: true,
        }];
        (out, clients)
    }

    pub(super) fn weather_name(&self) -> Option<&'static str> {
        self.weather.forced
    }

    /// The host picked another weather (`weather` in the settings, also after a reload): send
    /// it to everyone with Visuals.
    pub(super) fn follow_weather_setting(&mut self) -> Vec<Outgoing> {
        let want = self.settings.weather.as_deref().and_then(parse);
        if want == self.weather.setting {
            return Vec::new();
        }
        self.weather.setting = want;
        self.set_weather(want).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RoomSettings;

    fn hello() -> Packet {
        let mut w = Writer::new();
        for &b in MAGIC {
            w.u8(b);
        }
        w.u8(VERSION).u8(HELLO);
        Packet {
            code: WEATHER,
            payload: w.into_inner(),
        }
    }

    fn sent_name(o: &Outgoing) -> String {
        let p = &o.packet.payload;
        assert_eq!((o.packet.code, &p[..4], p[5]), (WEATHER, &MAGIC[..], SET));
        String::from_utf8(p[7..7 + p[6] as usize].to_vec()).unwrap()
    }

    #[test]
    fn weather_goes_to_modded_players_and_to_latecomers() {
        let mut room = Room::new(RoomSettings::default());
        assert!(room.handle(1, &hello()).is_empty(), "nothing forced yet");
        let (out, reply) = room.console("weather rAiN");
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0].to, Recipients::Many(p) if p == &vec![1]));
        assert_eq!(sent_name(&out[0]), "Rain");
        assert!(
            reply.contains("Rain") && reply.contains("1 player"),
            "{reply}"
        );
        let late = room.handle(2, &hello());
        assert_eq!(sent_name(&late[0]), "Rain");

        let (out, _) = room.console("weather auto");
        assert_eq!(sent_name(&out[0]), "Auto");
        assert!(room.handle(3, &hello()).is_empty(), "back to the biome");
    }

    #[test]
    fn unknown_weather_and_left_players_are_handled() {
        let mut room = Room::new(RoomSettings::default());
        room.handle(1, &hello());
        let (out, reply) = room.console("weather tornado");
        assert!(out.is_empty());
        assert!(reply.contains("no weather tornado"), "{reply}");
        room.disconnect(1);
        let (out, reply) = room.console("weather snowfall");
        assert!(out.is_empty());
        assert!(reply.contains("0 player"), "{reply}");
        assert!(room.console("weather").1.contains("Snowfall"));
    }

    #[test]
    fn the_hosts_weather_setting_applies_and_follows_reloads() {
        let mut room = Room::new(RoomSettings {
            weather: Some("storm".into()),
            ..RoomSettings::default()
        });
        room.tick(0.0);
        assert_eq!(sent_name(&room.handle(1, &hello())[0]), "Storm");
        // An admin's choice holds until the host changes the setting.
        room.console("weather clear");
        assert!(room.tick(0.1).iter().all(|o| o.packet.code != WEATHER));
        let mut next = room.settings().clone();
        next.weather = Some("Snowfall".into());
        room.apply_settings(next);
        let out = room.tick(0.2);
        let sent: Vec<String> = out
            .iter()
            .filter(|o| o.packet.code == WEATHER)
            .map(sent_name)
            .collect();
        assert_eq!(sent, ["Snowfall"]);
        let mut next = room.settings().clone();
        next.weather = None;
        room.apply_settings(next);
        let out = room.tick(0.3);
        assert!(
            out.iter()
                .any(|o| o.packet.code == WEATHER && sent_name(o) == "Auto")
        );
    }
}
