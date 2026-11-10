use std::collections::BTreeMap;

use tabg_protocol::actions::{
    ADMIN_START_GAME, ChatMessage, HealRequest, HealthStateRequest, ProjectileSyncHeader,
    ThrowRequest, decode_admin_command, encode_item_thrown, encode_player_health,
    encode_throw_chat,
};
use tabg_protocol::combat::{
    DamageEvent, PlayerDamaged, PlayerDead, PlayerFireHeader, ReviveRequest, ReviveStage, TeamDead,
    encode_revive_state, firing_mode,
};
use tabg_protocol::match_flow::{
    GameStateChanged, PlayerAirplaneDropped, RequestAirplaneDrop, RingUpdate, encode_all_drop,
    encode_plane_update,
};
use tabg_protocol::messages::{
    CarUpdate as MovedCar, ClientPlayerUpdate, EntityBody, EntityUpdate, NO_WEAPON,
    PackedDirection, PlayerJoined, PlayerUpdateBatch, RoomInitRequest, RoomInitResponse,
    WorldPhase, WorldPlayer, WorldState, container_flags, driving_state, game_state,
    server_response,
};
use tabg_protocol::vehicles::{
    CarDamageReport, CarTemporaryUpdate, CarUpdate, PassengerUpdate, SEAT_GET_IN, SEAT_GET_OUT,
    SeatRequest, encode_car_damage, encode_seat_accepted,
};
use tabg_protocol::{EventCode, Packet};

use tabg_protocol::loot::{
    ChunkEntry, ChunkExit, ChunkPlayer, DropRequest, EQUIPMENT_LEN, NO_EQUIPMENT, PickupRequest,
    area_around, chunk_of, encode_item_drop, encode_pickup_accepted, encode_player_entered_chunk,
    encode_player_left_chunk,
};

use crate::cars::{Car, TEMPORARY_OWNER_SECONDS};
use crate::loot::LootWorld;
use crate::plane::{self, Plane};

/// "No player" in TABG's byte player fields.
const NOBODY: u8 = u8::MAX;
use crate::ring::{Ring, RingEvent};
use crate::rng::Rng;
use crate::settings::{RoomSettings, SpawnPoint};

mod bomb;
mod bots;
mod console;
mod destruction;
mod extras;
mod inventory;
mod maps;
mod princess;
mod princess_bots;
mod princess_loadouts;
#[cfg(test)]
mod princess_tests;
mod radar;
mod skins;
mod taxi;
mod vehicle_combat;
mod vehicles;
mod voice;
mod weather;
pub use console::{PlayerInfo, RoomEvent};
pub use extras::SpellDrops;
pub use vehicles::VehicleRules;

/// After a match ends, players are disconnected this long later and the room resets (the official
/// server terminates 15 s after the end).
pub const RESTART_AFTER_END: f32 = 15.0;

/// Who a packet goes to. Player ids are ENet peer ids (TABG's player index).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recipients {
    One(u8),
    Many(Vec<u8>),
    All,
    AllExcept(u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub to: Recipients,
    pub packet: Packet,
    pub reliable: bool,
}

impl Outgoing {
    fn reliable(to: Recipients, event: EventCode, payload: Vec<u8>) -> Self {
        Self {
            to,
            packet: Packet::new(event, payload),
            reliable: true,
        }
    }
}

#[derive(Clone, Debug)]
struct Player {
    index: u8,
    group: u8,
    name: String,
    /// Stable identity: the Epic product user id when the client sends one.
    id: String,
    gear: Vec<i32>,
    colour: i32,
    squad_id: u64,
    /// Received the world state and is in the game.
    in_world: bool,
    position: [f32; 3],
    rotation: [f32; 2],
    ads: bool,
    direction: PackedDirection,
    movement: u8,
    /// Moved since the last tick.
    dirty: bool,
    /// Left the plane (or never had to: not in a match yet).
    dropped: bool,
    landed: bool,
    alive: bool,
    health: f32,
    downed: bool,
    /// When the player went down (damage within 1 s of that is ignored).
    down_time: f32,
    /// 255 = nobody.
    last_attacker: u8,
    kills: u16,
    /// Chunk the player is in, and the 3×3 chunks they can see (empty until their first chunk
    /// entry after jumping).
    chunk: (i8, i8),
    area: Vec<(i8, i8)>,
    equipment: [i16; EQUIPMENT_LEN],
    attachments: Vec<i16>,
    weapon_type: i32,
    /// `(car index, seat network index, is driver)` while in a car.
    seat: Option<(i32, i32, bool)>,
    is_admin: bool,
    /// When the parrot last spoke (rate limit 0.5 s).
    last_parrot: f32,
    /// With respawns: when this dead player comes back.
    respawn_at: Option<f32>,
    /// Gun game level (index into `gun_game`).
    level: usize,
    /// Voice rate limit: (window start, frames in it).
    voice_window: (f32, u16),
    /// When the CustomGameSkins outfit last changed.
    last_outfit: f32,
    /// Item id → quantity carried (see `room/inventory.rs`).
    inventory: BTreeMap<i32, i32>,
    /// Items already reported by the inventory check (one log line per item).
    inventory_reported: std::collections::BTreeSet<i32>,
    /// Dead and playing the respawn minigame.
    boss_fight: bool,
    /// Had the respawn minigame this match.
    boss_done: bool,
}

impl Player {
    fn new(index: u8, group: u8, name: String, id: String, squad_id: u64) -> Self {
        Self {
            index,
            group,
            name,
            id,
            gear: Vec::new(),
            colour: 0,
            squad_id,
            in_world: false,
            position: [0.0; 3],
            rotation: [0.0; 2],
            ads: false,
            direction: PackedDirection::ZERO,
            movement: 0,
            dirty: false,
            dropped: true,
            landed: true,
            alive: true,
            health: 100.0,
            downed: false,
            down_time: 0.0,
            last_attacker: NOBODY,
            kills: 0,
            chunk: (0, 0),
            area: Vec::new(),
            equipment: NO_EQUIPMENT,
            attachments: Vec::new(),
            weapon_type: NO_WEAPON,
            seat: None,
            is_admin: false,
            last_parrot: f32::MIN,
            respawn_at: None,
            level: 0,
            voice_window: (f32::MIN, 0),
            last_outfit: f32::MIN,
            inventory: BTreeMap::new(),
            inventory_reported: std::collections::BTreeSet::new(),
            boss_fight: false,
            boss_done: false,
        }
    }
}

/// One TABG match.
#[derive(Debug)]
pub struct Room {
    settings: RoomSettings,
    /// Connected peers that have not been accepted yet.
    pending: Vec<u8>,
    players: BTreeMap<u8, Player>,
    state: u8,
    next_spawn: usize,
    last_tick: f32,
    rng: Rng,
    /// Time of the previous `tick`, to advance timers.
    last_now: Option<f32>,
    countdown_left: f32,
    plane: Option<Plane>,
    ring: Ring,
    ended_at: Option<f32>,
    /// Peers the transport should disconnect (taken by [`Room::take_disconnects`]).
    to_disconnect: Vec<u8>,
    loot: LootWorld,
    cars: BTreeMap<i32, Car>,
    /// Match modifier of the current match ("Normal game", …).
    modifier: u8,
    /// Things worth telling the host about (taken by [`Room::take_events`]).
    events: Vec<RoomEvent>,
    /// When the current match started.
    match_started_at: f32,
    /// Peers whose CustomGameSkins hello was accepted.
    skins_authorized: Vec<u8>,
    /// Peers whose radar mod said hello.
    radar_watchers: Vec<u8>,
    destruction: destruction::DestructionState,
    weather: weather::WeatherState,
    vehicles: vehicles::VehicleState,
    combat: vehicle_combat::CombatState,
    taxis: taxi::TaxiState,
    next_radar_at: f32,
    /// Bomb mode round state.
    bomb: Option<bomb::BombState>,
    /// Soul loot network id → whose soul it is.
    souls: BTreeMap<i32, u8>,
    /// Spell drops this match, by network index.
    drops: Vec<extras::Drop>,
    next_drop_at: Option<f32>,
    /// Server-side players and their minds.
    bots: bots::Brains,
    bot_ring_at: f32,
    /// Capture the Princess: princesses, helicopters earned, turrets, the bots' maps.
    princess: princess::PrincessState,
    /// Recent gunshots bots can hear (Capture the Princess).
    noises: Vec<bots::Noise>,
}

impl Room {
    pub fn new(settings: RoomSettings) -> Self {
        let mut rng = Rng::new(settings.seed);
        let ring = Ring::new(settings.ring.clone(), &mut rng);
        let mut room = Self {
            settings,
            pending: Vec::new(),
            players: BTreeMap::new(),
            state: game_state::WAITING_FOR_PLAYERS,
            next_spawn: 0,
            last_tick: 0.0,
            rng,
            last_now: None,
            countdown_left: 0.0,
            plane: None,
            ring,
            ended_at: None,
            to_disconnect: Vec::new(),
            loot: LootWorld::default(),
            cars: BTreeMap::new(),
            modifier: 0,
            events: Vec::new(),
            match_started_at: 0.0,
            skins_authorized: Vec::new(),
            radar_watchers: Vec::new(),
            destruction: Default::default(),
            weather: Default::default(),
            vehicles: Default::default(),
            combat: Default::default(),
            taxis: Default::default(),
            next_radar_at: 0.0,
            bomb: None,
            souls: BTreeMap::new(),
            drops: Vec::new(),
            next_drop_at: None,
            bots: BTreeMap::new(),
            bot_ring_at: 0.0,
            princess: Default::default(),
            noises: Vec::new(),
        };
        room.spawn_cars();
        room
    }

    /// Put every car back where the map data has it.
    fn spawn_cars(&mut self) {
        self.cars = self
            .settings
            .cars
            .iter()
            .map(|s| (s.car_index, Car::spawn(s)))
            .collect();
        self.spawn_unused_vehicles();
    }

    /// Cars standing in `chunks` (none in the lobby, as on the official server).
    fn cars_in(&self, chunks: &[(i8, i8)], peer: u8) -> Vec<tabg_protocol::vehicles::ChunkCar> {
        if self.state == game_state::WAITING_FOR_PLAYERS {
            return Vec::new();
        }
        self.cars
            .values()
            .filter(|c| {
                chunks.contains(&chunk_of(c.position)) && self.car_visible_to(c.index, peer)
            })
            .map(Car::chunk_entry)
            .collect()
    }

    /// Loot currently lying in the world.
    pub fn loot(&self) -> &LootWorld {
        &self.loot
    }

    /// Current `GameState` byte.
    pub fn state(&self) -> u8 {
        self.state
    }

    /// Peers to disconnect now (e.g. after a match ended). The room already forgot them.
    pub fn take_disconnects(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.to_disconnect)
    }

    pub fn settings(&self) -> &RoomSettings {
        &self.settings
    }

    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// Accepted players. [`Recipients::All`] means exactly these (pending peers get nothing).
    pub fn player_ids(&self) -> Vec<u8> {
        self.players.keys().copied().collect()
    }

    pub fn player_names(&self) -> impl Iterator<Item = (u8, &str)> {
        self.players.values().map(|p| (p.index, p.name.as_str()))
    }

    /// A transport-level connection was made. The player is added once it sends `RoomInit`.
    pub fn connect(&mut self, peer: u8) {
        if !self.pending.contains(&peer) {
            self.pending.push(peer);
        }
    }

    /// A peer disconnected or timed out.
    pub fn disconnect(&mut self, peer: u8) -> Vec<Outgoing> {
        self.pending.retain(|&p| p != peer);
        self.skins_authorized.retain(|&p| p != peer);
        self.radar_watchers.retain(|&p| p != peer);
        self.destruction_forget(peer);
        self.weather_forget(peer);
        self.forget_vehicle_client(peer);
        self.forget_princess_client(peer);
        self.eject(peer);
        // Leaving mid-match drops your loot, like dying (official `PlayerLeaveCommand`).
        let mut dropped = Vec::new();
        if self.state == game_state::STARTED
            && self
                .players
                .get(&peer)
                .is_some_and(|p| p.alive && p.dropped)
            && !self.is_bomb()
        {
            dropped = self.drop_inventory(peer);
        }
        let Some(p) = self.players.remove(&peer) else {
            return Vec::new();
        };
        self.events.push(RoomEvent::Left {
            index: peer,
            name: p.name,
        });
        // PlayerLeft: [player index, 1 if they left the game for good].
        dropped.push(Outgoing::reliable(
            Recipients::All,
            EventCode::PlayerLeft,
            vec![peer, 1],
        ));
        dropped
    }

    /// Handle one packet from `peer`. Unknown or malformed packets are ignored.
    pub fn handle(&mut self, peer: u8, packet: &Packet) -> Vec<Outgoing> {
        if packet.code == voice::VOICE {
            return self.voice(peer, &packet.payload);
        }
        if packet.code == skins::SKINS {
            return self.skins(peer, &packet.payload);
        }
        if packet.code == radar::RADAR {
            return self.radar_hello(peer, &packet.payload);
        }
        if packet.code == destruction::DESTRUCTION {
            return self.destruction_message(peer, &packet.payload);
        }
        if packet.code == weather::WEATHER {
            return self.weather_message(peer, &packet.payload);
        }
        if packet.code == vehicles::VEHICLES {
            return self.vehicle_message(peer, &packet.payload);
        }
        if packet.code == princess::PRINCESS {
            return self.princess_message(peer, &packet.payload);
        }
        if matches!(packet.code, maps::RANGE | maps::DEV_TEST) {
            return self.test_map(peer, packet.code, &packet.payload);
        }
        match packet.event() {
            Some(EventCode::RoomInit) => match RoomInitRequest::decode(&packet.payload) {
                Ok(req) => self.room_init(peer, req),
                Err(_) => Vec::new(),
            },
            Some(EventCode::RequestWorldState) => self.world_state(peer),
            Some(EventCode::RequestPurchaseGun) => self.bomb_purchase(peer, &packet.payload),
            Some(EventCode::RequestBlessing) => self.blessing(peer, &packet.payload),
            Some(EventCode::RequestClickInteract) => self.open_drop(peer, &packet.payload),
            Some(EventCode::BossFightResult) => self.boss_fight_result(peer, &packet.payload),
            Some(EventCode::RequestCurseCleanse) => self.cleanse_curse(peer, &packet.payload),
            Some(EventCode::RequestRespawnTeamMate) => self.respawn_teammate(peer, &packet.payload),
            Some(EventCode::RequestPlantBomb) => self.bomb_plant(peer, &packet.payload),
            Some(EventCode::RequestDefuseBomb) => self.bomb_defuse(peer),
            Some(EventCode::RequestStopDefuse) => self.bomb_stop_defuse(peer),
            Some(EventCode::RequestAirplaneDrop) => {
                match RequestAirplaneDrop::decode(&packet.payload) {
                    Ok(req) => self.airplane_drop(peer, req),
                    Err(_) => Vec::new(),
                }
            }
            // Landing after skydiving: remembered and relayed unchanged, like the official server.
            Some(EventCode::DamageEvent) => {
                // A FlyingControls blast receipt may trail the packet (missile / FPV hits).
                let receipt = vehicle_combat::strip_receipt(&packet.payload);
                let payload = receipt.map_or(&packet.payload[..], |r| r.0);
                match DamageEvent::decode(payload) {
                    Ok(ev) => {
                        let before = self.players.get(&ev.victim).map(|p| (p.health, p.position));
                        let mut out = self.damage(peer, ev.clone());
                        if let (Some((_, is_missile, id)), Some((health, at))) = (receipt, before) {
                            let after = self.players.get(&ev.victim).map_or(0.0, |p| p.health);
                            out.extend(self.confirm_blast(
                                ev.attacker,
                                is_missile,
                                id,
                                false,
                                i32::from(ev.victim),
                                at,
                                (health - after).max(0.0),
                            ));
                        }
                        out
                    }
                    Err(_) => Vec::new(),
                }
            }
            // The client reports its own death (falling, …) or a death by the ring: `[player index]`.
            Some(EventCode::PlayerDead) | Some(EventCode::RingDeath)
                if packet.payload.first() == Some(&peer) =>
            {
                let ring_death = packet.event() == Some(EventCode::RingDeath);
                self.kill(peer, ring_death)
            }
            Some(EventCode::ReviveState) => match ReviveRequest::decode(&packet.payload) {
                Ok(req) => self.revive(peer, req),
                Err(_) => Vec::new(),
            },
            Some(EventCode::PlayerFire) => self.player_fire(peer, packet),
            // Simple relays: the first byte names the player, which must be the sender.
            Some(
                event @ (EventCode::WeaponChange | EventCode::GearChange | EventCode::PlayerEffect),
            ) => {
                let relayed = match event {
                    EventCode::WeaponChange => {
                        if !self.track_equipment(peer, &packet.payload) {
                            return Vec::new();
                        }
                        EventCode::WeaponChanged
                    }
                    other => other,
                };
                self.relay_from(peer, packet, relayed, Recipients::All)
            }
            Some(EventCode::ChatMessage) => match ChatMessage::decode(&packet.payload) {
                Ok(msg) if msg.player_index == peer => self.chat(peer, msg, packet),
                _ => Vec::new(),
            },
            Some(EventCode::ThrowChatMessage) => self.parrot(peer, packet),
            Some(EventCode::RequestHealing) => match HealRequest::decode(&packet.payload) {
                Ok(req) if req.player_index == peer => self.heal(peer, req.item_id),
                _ => Vec::new(),
            },
            Some(EventCode::RequestHealthState) => {
                match HealthStateRequest::decode(&packet.payload) {
                    Ok(req) if req.player_index == peer => self.health_state(peer, req.health),
                    _ => Vec::new(),
                }
            }
            Some(EventCode::RequestItemThrow) => match ThrowRequest::decode(&packet.payload) {
                Ok(req) if req.player_index == peer => self.throw(peer, req),
                _ => Vec::new(),
            },
            Some(EventCode::SendCatchPhrase) if self.players.contains_key(&peer) => {
                vec![Outgoing::reliable(
                    Recipients::Many(self.same_chunk(peer)),
                    EventCode::CatchPhrase,
                    vec![peer],
                )]
            }
            // A dead player picks someone to spectate.
            Some(EventCode::SpectatorRequest) => {
                match (self.players.get(&peer), packet.payload.first()) {
                    (Some(p), Some(&target))
                        if !p.alive && self.players.get(&target).is_some_and(|t| t.alive) =>
                    {
                        vec![Outgoing::reliable(
                            Recipients::One(peer),
                            EventCode::SpectatorForce,
                            vec![target],
                        )]
                    }
                    _ => Vec::new(),
                }
            }
            Some(EventCode::RequestTakeDamageEvent) if self.players.contains_key(&peer) => {
                vec![Outgoing::reliable(
                    Recipients::Many(self.near(peer)),
                    EventCode::TakeDamageEvent,
                    vec![peer],
                )]
            }
            Some(EventCode::NetworkPlayerTransmittedPackage)
                if self.players.contains_key(&peer) =>
            {
                let mut payload = vec![peer];
                payload.extend_from_slice(&packet.payload);
                let to: Vec<u8> = self.near(peer).into_iter().filter(|&w| w != peer).collect();
                vec![Outgoing::reliable(
                    Recipients::Many(to),
                    EventCode::NetworkPlayerTransmittedPackage,
                    payload,
                )]
            }
            Some(EventCode::RequestSyncProjectileEvent) => {
                match ProjectileSyncHeader::decode(&packet.payload) {
                    Ok(h) if self.players.contains_key(&peer) => {
                        let to = if h.broadcast {
                            if h.include_sender {
                                Recipients::All
                            } else {
                                Recipients::AllExcept(peer)
                            }
                        } else {
                            Recipients::Many(
                                self.near(peer)
                                    .into_iter()
                                    .filter(|&w| h.include_sender || w != peer)
                                    .collect(),
                            )
                        };
                        vec![Outgoing::reliable(
                            to,
                            EventCode::SyncProjectileEvent,
                            packet.payload.clone(),
                        )]
                    }
                    _ => Vec::new(),
                }
            }
            Some(EventCode::AdminCommand) => match decode_admin_command(&packet.payload) {
                Ok(cmd)
                    if cmd == ADMIN_START_GAME
                        && self.players.get(&peer).is_some_and(|p| p.is_admin) =>
                {
                    self.force_start()
                }
                _ => Vec::new(),
            },
            Some(EventCode::RequestSeat) => match SeatRequest::decode(&packet.payload) {
                Ok(req) => self.seat(peer, req),
                Err(_) => Vec::new(),
            },
            Some(EventCode::CarUpdate) => match CarUpdate::decode(&packet.payload) {
                Ok(up) => self.car_update(peer, up),
                Err(_) => Vec::new(),
            },
            Some(EventCode::CarTemporaryUpdate) => {
                if let Ok(up) = CarTemporaryUpdate::decode(&packet.payload) {
                    self.car_push(peer, up);
                }
                Vec::new()
            }
            Some(EventCode::CarDamage) => {
                let receipt = vehicle_combat::strip_receipt(&packet.payload);
                let payload = receipt.map_or(&packet.payload[..], |r| r.0);
                match CarDamageReport::decode(payload) {
                    Ok(d) => {
                        let health = |room: &Room| {
                            room.cars
                                .get(&d.car_index)
                                .and_then(|c| c.parts.iter().find(|p| p.index == d.part))
                                .map(|p| p.health)
                        };
                        let (before, at) = (
                            health(self),
                            self.cars.get(&d.car_index).map(|c| c.position),
                        );
                        let mut out = self.car_damage(peer, d.clone());
                        if let (Some((_, is_missile, id)), Some(before), Some(at)) =
                            (receipt, before, at)
                        {
                            let lost = (before - health(self).unwrap_or(0.0)).max(0.0);
                            out.extend(self.confirm_blast(
                                peer,
                                is_missile,
                                id,
                                true,
                                d.car_index,
                                at,
                                lost,
                            ));
                        }
                        out
                    }
                    Err(_) => Vec::new(),
                }
            }
            Some(EventCode::PassangerUpdate) => {
                if let Ok(up) = PassengerUpdate::decode(&packet.payload) {
                    if let Some(p) = self
                        .players
                        .get_mut(&peer)
                        .filter(|p| up.player_index == peer && p.seat.is_some())
                    {
                        p.rotation = up.rotation;
                        p.ads = up.aiming_down_sights;
                        p.dirty = true;
                    }
                }
                Vec::new()
            }
            Some(EventCode::RequestWeaponPickUp) => match PickupRequest::decode(&packet.payload) {
                Ok(req) => self.pickup(peer, req),
                Err(_) => Vec::new(),
            },
            Some(EventCode::RequestItemDrop) => match DropRequest::decode(&packet.payload) {
                Ok(req) => self.item_drop(peer, req),
                Err(_) => Vec::new(),
            },
            Some(EventCode::PlayerState) => {
                self.relay_from(peer, packet, EventCode::PlayerStateChanged, Recipients::All)
            }
            Some(EventCode::PlayerMarkerAdded) => {
                if self.players.contains_key(&peer) {
                    vec![Self::broadcast(
                        EventCode::PlayerMarkerEvent,
                        packet.payload.clone(),
                    )]
                } else {
                    Vec::new()
                }
            }
            Some(EventCode::PlayerLand) => match self.players.get_mut(&peer) {
                Some(p) if p.dropped && !p.landed => {
                    p.landed = true;
                    let mut out = vec![Outgoing::reliable(
                        Recipients::All,
                        EventCode::PlayerLand,
                        packet.payload.clone(),
                    )];
                    if self.state == game_state::STARTED {
                        out.extend(self.give_loadout(peer));
                    }
                    out
                }
                _ => Vec::new(),
            },
            Some(EventCode::PlayerUpdate) => match ClientPlayerUpdate::decode(&packet.payload) {
                Ok(up) => self.player_update(peer, up),
                Err(_) => Vec::new(),
            },
            // TABGPing is echoed back unchanged as PingRecieved.
            Some(EventCode::TABGPing) => {
                vec![Outgoing::reliable(
                    Recipients::One(peer),
                    EventCode::PingRecieved,
                    packet.payload.clone(),
                )]
            }
            _ => Vec::new(),
        }
    }

    fn room_init(&mut self, peer: u8, req: RoomInitRequest) -> Vec<Outgoing> {
        if self.players.contains_key(&peer) {
            return Vec::new();
        }
        let refuse = |response: u8, room: &Self| {
            let msg = RoomInitResponse::Refused {
                response,
                players_in_room: u8::try_from(room.players.len()).unwrap_or(u8::MAX),
                squad_size: req.squad_size,
                reserved_slots: 0,
            };
            vec![Outgoing::reliable(
                Recipients::One(peer),
                EventCode::RoomInitRequestResponse,
                msg.encode(),
            )]
        };
        if !matches!(
            self.state,
            game_state::WAITING_FOR_PLAYERS | game_state::COUNT_DOWN
        ) {
            return refuse(
                server_response::ACCEPTED | server_response::MATCH_ALREADY_STARTED,
                self,
            );
        }
        if self.players.len() >= usize::from(self.settings.max_players) {
            return refuse(
                server_response::ACCEPTED | server_response::SQUAD_DONT_FIT,
                self,
            );
        }
        if !self.settings.password.is_empty()
            && req.password.as_deref() != Some(self.settings.password.as_str())
        {
            return refuse(
                server_response::ACCEPTED | server_response::WRONG_PASSWORD,
                self,
            );
        }

        let name = clean_name(&req.player_name);
        let id = player_id(&req);
        if self.is_banned(&name, &id) {
            self.events.push(RoomEvent::Refused {
                name,
                id,
                reason: "banned".into(),
            });
            return refuse(server_response::ERROR, self);
        }
        let group = self.group_for(peer, req.squad_id);
        self.pending.retain(|&p| p != peer);
        self.players.insert(
            peer,
            Player {
                gear: req.gear.clone(),
                colour: req.colour,
                is_admin: self.is_admin_name(&name, &id),
                ..Player::new(peer, group, name.clone(), id.clone(), req.squad_id)
            },
        );

        self.events.push(RoomEvent::Joined {
            index: peer,
            name: name.clone(),
            id,
        });
        // Same order as the official server: announce the player to everyone, then accept them.
        let joined = PlayerJoined {
            player_index: peer,
            group_index: group,
            player_name: name.clone(),
            gear: req.gear,
            is_dev: false,
            colour: req.colour,
        };
        let accepted = RoomInitResponse::Accepted {
            game_mode: self.settings.game_mode,
            match_mode: self.settings.match_mode,
            player_index: peer,
            group_index: group,
            player_name: name,
        };
        vec![
            Outgoing::reliable(Recipients::All, EventCode::Login, joined.encode()),
            Outgoing::reliable(
                Recipients::One(peer),
                EventCode::RoomInitRequestResponse,
                accepted.encode(),
            ),
        ]
    }

    /// Squad members (same non-zero squad id) share a group; everyone else gets their own.
    fn group_for(&self, peer: u8, squad_id: u64) -> u8 {
        if self.is_bomb() {
            return self.bomb_group();
        }
        if self.is_princess_mode() {
            return self.princess_group(squad_id);
        }
        if squad_id != 0 {
            if let Some(mate) = self.players.values().find(|p| p.squad_id == squad_id) {
                return mate.group;
            }
        }
        peer
    }

    fn world_state(&mut self, peer: u8) -> Vec<Outgoing> {
        let group = self.players.get(&peer).map_or(0, |p| p.group);
        let spawn = self.spawn_for(group);
        let Some(player) = self.players.get_mut(&peer) else {
            return Vec::new();
        };
        if player.in_world {
            return Vec::new();
        }
        player.in_world = true;
        player.position = spawn.position;
        player.rotation = [0.0, spawn.rotation];
        let (index, group, name, is_admin) = (
            player.index,
            player.group,
            player.name.clone(),
            player.is_admin,
        );

        let players = self
            .players
            .values()
            .map(|p| WorldPlayer {
                player_index: p.index,
                group_index: p.group,
                player_name: p.name.clone(),
                weapon_type: p.weapon_type,
                gear: p.gear.clone(),
                is_admin: p.is_admin,
                colour: p.colour,
            })
            .collect();
        let s = &self.settings;
        let world = WorldState {
            player_index: index,
            group_index: group,
            player_name: name,
            is_admin,
            position: spawn.position,
            rotation: spawn.rotation,
            is_dead: false,
            is_downed: false,
            health: 100.0,
            car_seat: None,
            deception_colour: None,
            players,
            weapons: Vec::new(),
            cars: self.cars.values().map(Car::world_entry).collect(),
            time_of_day: s.time_of_day,
            time_before_first_ring: s.time_before_first_ring,
            base_ring_time: s.ring.base_time,
            rings: s
                .ring
                .sizes
                .iter()
                .copied()
                .zip(s.ring.speeds.iter().copied())
                .collect(),
            lives_per_team: s.lives_per_team,
            kills_to_win: s.kills_to_win,
            // Late joiners are refused once the match started, so only these two matter.
            phase: if self.state == game_state::COUNT_DOWN {
                WorldPhase::CountDown {
                    tick: self.countdown_left,
                }
            } else {
                WorldPhase::Plain(self.state)
            },
        };
        vec![Outgoing::reliable(
            Recipients::One(peer),
            EventCode::Login,
            world.encode(s.game_mode),
        )]
    }

    fn solo(&self) -> bool {
        self.settings.match_mode == tabg_protocol::messages::match_mode::SOLO
    }

    fn is_brawl(&self) -> bool {
        self.settings.game_mode == tabg_protocol::messages::game_mode::BRAWL
            && !self.settings.brawl.houses.is_empty()
    }

    /// Where a player of `group` (re)spawns: their Brawl house, else the next spawn point.
    fn spawn_for(&mut self, group: u8) -> SpawnPoint {
        if self.is_bomb()
            && let Some(spawn) = self.bomb_spawn(group)
        {
            return spawn;
        }
        if self.is_princess_mode() && self.settings.nav.is_some() {
            // Everyone waits at their own base, next to the cage.
            return SpawnPoint {
                position: self.princess_spawn(group),
                rotation: self.rng.range(0.0, 360.0),
            };
        }
        if self.is_brawl()
            && let Some(house) = self.settings.brawl.house(self.solo(), group)
            && !house.spawns.is_empty()
        {
            let i = self.rng.index(house.spawns.len());
            return house.spawns[i];
        }
        self.next_spawn_point()
    }

    fn next_spawn_point(&mut self) -> SpawnPoint {
        let points = &self.settings.spawn_points;
        if points.is_empty() {
            return SpawnPoint {
                position: [0.0, 100.0, 0.0],
                rotation: 0.0,
            };
        }
        let p = points[self.next_spawn % points.len()];
        self.next_spawn = self.next_spawn.wrapping_add(1);
        p
    }

    fn player_update(&mut self, peer: u8, up: ClientPlayerUpdate) -> Vec<Outgoing> {
        // Clients name themselves in the packet; only trust the transport's peer id.
        let in_match = self.state == game_state::STARTED;
        let Some(p) = self.players.get_mut(&peer) else {
            return Vec::new();
        };
        if !p.in_world
            || up.player_index != peer
            || !up
                .position
                .iter()
                .chain(&up.rotation)
                .all(|v| v.is_finite())
        {
            return Vec::new();
        }
        p.position = up.position;
        p.rotation = up.rotation;
        p.ads = up.aiming_down_sights;
        p.direction = up.direction;
        p.movement = up.movement;
        p.dirty = true;
        let new_chunk = chunk_of(up.position);
        if in_match && p.dropped && !p.area.is_empty() && new_chunk != p.chunk {
            return self.chunk_changed(peer, new_chunk);
        }
        Vec::new()
    }

    /// The player record other clients get in chunk messages.
    fn chunk_player(p: &Player) -> ChunkPlayer {
        ChunkPlayer {
            player_index: p.index,
            state: 0,
            weapon_type: p.weapon_type,
            health: p.health,
            downed: p.downed,
            skydiving: !p.landed,
            gear: p.gear.clone(),
            equipment: p.equipment,
            attachments: p.attachments.clone(),
        }
    }

    /// Players (who jumped) that currently see `chunk`.
    fn watchers(&self, chunk: (i8, i8)) -> Vec<u8> {
        self.players
            .values()
            .filter(|w| w.dropped && w.area.contains(&chunk))
            .map(|w| w.index)
            .collect()
    }

    /// Living players who jumped and stand in one of `chunks`, except `skip`.
    fn players_in(&self, chunks: &[(i8, i8)], skip: u8) -> impl Iterator<Item = &Player> {
        self.players
            .values()
            .filter(move |o| o.index != skip && o.dropped && o.alive && chunks.contains(&o.chunk))
    }

    /// After jumping: send the player everything around them and announce them to watchers
    /// (official `ForceChunkEntry`).
    fn force_chunk_entry(&mut self, peer: u8) -> Vec<Outgoing> {
        let Some(p) = self.players.get_mut(&peer) else {
            return Vec::new();
        };
        p.chunk = chunk_of(p.position);
        p.area = area_around(p.chunk).to_vec();
        let (chunk, area) = (p.chunk, p.area.clone());
        let entry = ChunkEntry {
            player_index: peer,
            players: self
                .players_in(&area, peer)
                .map(Self::chunk_player)
                .collect(),
            loot: self.loot.in_chunks(&area),
            cars: self.cars_in(&area, peer),
        };
        let mut out = vec![Outgoing::reliable(
            Recipients::One(peer),
            EventCode::ChunkEntry,
            entry.encode(),
        )];
        out.extend(self.announce_in_chunk(peer, chunk));
        out
    }

    /// `PlayerEnteredChunk` for `peer` to everyone else watching `chunk`.
    fn announce_in_chunk(&self, peer: u8, chunk: (i8, i8)) -> Vec<Outgoing> {
        let watchers: Vec<u8> = self
            .watchers(chunk)
            .into_iter()
            .filter(|&w| w != peer)
            .collect();
        match self.players.get(&peer) {
            Some(p) if p.alive && !watchers.is_empty() => vec![Outgoing::reliable(
                Recipients::Many(watchers),
                EventCode::PlayerEnteredChunk,
                encode_player_entered_chunk(&Self::chunk_player(p), [-1, -1, -1]),
            )],
            _ => Vec::new(),
        }
    }

    /// The player crossed into `new_chunk`: send what came into and went out of view, and tell
    /// the watchers (official `PlayerChangedChunk`).
    fn chunk_changed(&mut self, peer: u8, new_chunk: (i8, i8)) -> Vec<Outgoing> {
        let Some(p) = self.players.get_mut(&peer) else {
            return Vec::new();
        };
        let (old_chunk, old_area) = (p.chunk, std::mem::take(&mut p.area));
        let new_area = area_around(new_chunk).to_vec();
        p.chunk = new_chunk;
        p.area = new_area.clone();
        let appeared: Vec<_> = new_area
            .iter()
            .copied()
            .filter(|c| !old_area.contains(c))
            .collect();
        let vanished: Vec<_> = old_area
            .iter()
            .copied()
            .filter(|c| !new_area.contains(c))
            .collect();

        let entry = ChunkEntry {
            player_index: peer,
            players: self
                .players_in(&appeared, peer)
                .map(Self::chunk_player)
                .collect(),
            loot: self.loot.in_chunks(&appeared),
            cars: self.cars_in(&appeared, peer),
        };
        let exit = ChunkExit {
            player_index: peer,
            players: self.players_in(&vanished, peer).map(|o| o.index).collect(),
            loot: self.loot.ids_in_chunks(&vanished),
            cars: self
                .cars
                .values()
                .filter(|c| vanished.contains(&chunk_of(c.position)))
                .map(|c| c.index)
                .collect(),
        };
        let mut out = vec![
            Outgoing::reliable(Recipients::One(peer), EventCode::ChunkEntry, entry.encode()),
            Outgoing::reliable(Recipients::One(peer), EventCode::ChunkExit, exit.encode()),
        ];
        out.extend(self.announce_in_chunk(peer, new_chunk));
        let old_watchers: Vec<u8> = self
            .watchers(old_chunk)
            .into_iter()
            .filter(|&w| w != peer)
            .collect();
        let lost_sight: Vec<u8> = old_watchers
            .iter()
            .copied()
            .filter(|w| !self.players[w].area.contains(&new_chunk))
            .collect();
        if !old_watchers.is_empty() {
            out.push(Outgoing::reliable(
                Recipients::Many(old_watchers),
                EventCode::PlayerLeftChunk,
                encode_player_left_chunk(peer, &lost_sight),
            ));
        }
        out
    }

    /// Who hears what `peer` does: everyone in the lobby; during the match, players watching their
    /// chunk plus their team.
    fn near(&self, peer: u8) -> Vec<u8> {
        let Some(p) = self.players.get(&peer) else {
            return Vec::new();
        };
        if self.state != game_state::STARTED || p.area.is_empty() {
            return self.player_ids();
        }
        let mut to = self.watchers(p.chunk);
        to.extend(
            self.players
                .values()
                .filter(|m| m.group == p.group)
                .map(|m| m.index),
        );
        to.sort_unstable();
        to.dedup();
        to
    }

    /// Players standing in the same chunk as `peer` (chat and catchphrases are local).
    fn same_chunk(&self, peer: u8) -> Vec<u8> {
        let Some(p) = self.players.get(&peer) else {
            return Vec::new();
        };
        let chunk = chunk_of(p.position);
        self.players
            .values()
            .filter(|o| chunk_of(o.position) == chunk)
            .map(|o| o.index)
            .collect()
    }

    /// Start the countdown now (admin "Start Game" or `/start`).
    fn force_start(&mut self) -> Vec<Outgoing> {
        if self.state != game_state::WAITING_FOR_PLAYERS || self.players.is_empty() {
            return Vec::new();
        }
        self.state = game_state::COUNT_DOWN;
        self.countdown_left = self.settings.countdown;
        vec![Self::state_changed(GameStateChanged::CountDown {
            seconds: self.countdown_left,
        })]
    }

    /// `ChatMessage`: relayed to players in the same chunk; admins' `/commands` run instead.
    fn chat(&mut self, peer: u8, msg: ChatMessage, packet: &Packet) -> Vec<Outgoing> {
        let Some(p) = self.players.get(&peer) else {
            return Vec::new();
        };
        let is_admin = p.is_admin;
        self.events.push(RoomEvent::Chat {
            index: peer,
            name: p.name.clone(),
            text: msg.text.clone(),
        });
        if let Some(command) = msg.text.strip_prefix('/').filter(|_| is_admin) {
            return self.player_command(peer, command);
        }
        vec![Outgoing::reliable(
            Recipients::Many(self.same_chunk(peer)),
            EventCode::ChatMessage,
            packet.payload.clone(),
        )]
    }

    /// `ThrowChatMessage`: the parrot speaks at the player's position (at most every 0.5 s).
    fn parrot(&mut self, peer: u8, packet: &Packet) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        let Some(p) = self.players.get_mut(&peer) else {
            return Vec::new();
        };
        if now - p.last_parrot < 0.5 || packet.payload.is_empty() {
            return Vec::new();
        }
        p.last_parrot = now;
        let payload = encode_throw_chat(peer, p.position, p.rotation, &packet.payload[1..]);
        vec![Outgoing::reliable(
            Recipients::Many(self.near(peer)),
            EventCode::ThrowChatMessage,
            payload,
        )]
    }

    /// `RequestHealing`: a bandage or med kit adds its healing amount.
    fn heal(&mut self, peer: u8, item_id: i32) -> Vec<Outgoing> {
        let Some(&amount) = self.settings.heal_amounts.get(&item_id) else {
            return Vec::new();
        };
        if !self
            .players
            .get(&peer)
            .is_some_and(|p| p.alive && !p.downed)
        {
            return Vec::new();
        }
        if self.inventory_checked(peer, "heal", item_id, 1).is_none() {
            return Vec::new();
        }
        let Some(p) = self.players.get_mut(&peer) else {
            return Vec::new();
        };
        p.health += amount;
        let health = p.health;
        self.inventory_remove(peer, item_id, 1);
        vec![Outgoing::reliable(
            Recipients::Many(self.near(peer)),
            EventCode::PlayerHealed,
            encode_player_health(peer, health),
        )]
    }

    /// `RequestHealthState`: the client reports a higher health (only increases are accepted).
    fn health_state(&mut self, peer: u8, health: f32) -> Vec<Outgoing> {
        let Some(p) = self.players.get_mut(&peer).filter(|p| p.alive) else {
            return Vec::new();
        };
        if !health.is_finite() || health <= p.health {
            return Vec::new();
        }
        p.health = health;
        vec![Outgoing::reliable(
            Recipients::Many(self.near(peer)),
            EventCode::PlayerHealthStateChanged,
            encode_player_health(peer, health),
        )]
    }

    /// `RequestItemThrow`: thrown items land as loot, except grenades.
    fn throw(&mut self, peer: u8, req: ThrowRequest) -> Vec<Outgoing> {
        if !self.players.get(&peer).is_some_and(|p| p.alive)
            || req.quantity <= 0
            || !req
                .position
                .iter()
                .chain(&req.direction)
                .all(|v| v.is_finite())
        {
            return Vec::new();
        }
        let mut req = req;
        match self.inventory_checked(peer, "throw", req.item_id, req.quantity) {
            Some(quantity) => req.quantity = quantity,
            None => return Vec::new(),
        }
        let drone = req.item_id == crate::vehicles::drone::ITEM
            && self.drone_thrown(peer, &mut req.quantity, req.position);
        self.inventory_remove(peer, req.item_id, req.quantity);
        let heli = self.princess_throw(peer, req.item_id, req.position, req.direction);
        let network_id = if drone || self.settings.grenades.contains(&req.item_id) {
            self.loot.reserve_id()
        } else {
            self.loot
                .add(req.item_id, req.quantity, req.position)
                .network_id
        };
        let payload = encode_item_thrown(
            peer,
            network_id,
            req.item_id,
            req.quantity,
            req.position,
            req.direction,
        );
        let mut out = vec![Outgoing::reliable(
            Recipients::Many(self.near(peer)),
            EventCode::ItemThrown,
            payload,
        )];
        out.extend(heli);
        out
    }

    /// `RequestSeat`: get into or out of a car seat (official `RequestSeatCommand`).
    fn seat(&mut self, peer: u8, req: SeatRequest) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        if req.player_index != peer
            || !self
                .players
                .get(&peer)
                .is_some_and(|p| p.dropped && p.alive && !p.downed)
        {
            return Vec::new();
        }
        let Some(car) = self.cars.get_mut(&req.car_index) else {
            return Vec::new();
        };
        let Some(seat) = car.seat_mut(req.seat_index) else {
            return Vec::new();
        };
        let (network_index, driver) = (seat.network_index, seat.driver);
        match req.action {
            SEAT_GET_IN => {
                if seat.occupant.is_some_and(|o| o != peer) {
                    return Vec::new();
                }
                seat.occupant = Some(peer);
                car.temporary_owner = None;
            }
            SEAT_GET_OUT => {
                if seat.occupant.is_some_and(|o| o != peer) {
                    return Vec::new();
                }
                seat.occupant = None;
                car.temporary_owner = Some((peer, now + TEMPORARY_OWNER_SECONDS));
            }
            _ => return Vec::new(),
        }
        let pose = (req.action == SEAT_GET_IN).then(|| (car.position, car.rotation.clone()));
        let car_chunk = chunk_of(car.position);
        let payload = encode_seat_accepted(
            peer,
            req.car_index,
            network_index,
            req.action,
            pose.as_ref().map(|(p, r)| (*p, r)),
        );
        // Leave any previous seat (switching seats), then take the new one.
        let previous = self.players[&peer].seat;
        if let Some((old_car, old_seat, _)) = previous.filter(|_| req.action == SEAT_GET_IN) {
            if (old_car, old_seat) != (req.car_index, network_index) {
                if let Some(c) = self.cars.get_mut(&old_car) {
                    if let Some(s) = c.seat_mut(old_seat).filter(|s| s.occupant == Some(peer)) {
                        s.occupant = None;
                    }
                }
            }
        }
        let p = self.players.get_mut(&peer).expect("checked above");
        p.seat = (req.action == SEAT_GET_IN).then_some((req.car_index, network_index, driver));
        p.dirty = true;
        if req.action == SEAT_GET_IN {
            self.vehicle_seated(req.car_index, driver);
        }
        let mut to = self.watchers(car_chunk);
        if !to.contains(&peer) {
            to.push(peer);
        }
        vec![Outgoing::reliable(
            Recipients::Many(to),
            EventCode::SeatAccepted,
            payload,
        )]
    }

    /// `CarUpdate` from the driver: the car (and its driver) moved.
    fn car_update(&mut self, peer: u8, up: CarUpdate) -> Vec<Outgoing> {
        if up.player_index != peer || !up.position.iter().all(|v| v.is_finite()) {
            return Vec::new();
        }
        let Some(&(car_index, _, true)) = self.players.get(&peer).and_then(|p| p.seat.as_ref())
        else {
            return Vec::new();
        };
        if self.taxi_owns_motion(car_index) {
            return Vec::new();
        }
        let Some(car) = self.cars.get_mut(&car_index) else {
            return Vec::new();
        };
        car.position = up.position;
        car.rotation = up.rotation;
        car.input = up.input;
        car.driving_state = up.driving_state;
        // Everyone in the car moves with it (for chunk visibility).
        let occupants: Vec<u8> = car.seats.iter().filter_map(|s| s.occupant).collect();
        let mut out = Vec::new();
        for o in occupants {
            if let Some(p) = self.players.get_mut(&o) {
                p.position = up.position;
                p.dirty = true;
                let new_chunk = chunk_of(up.position);
                if self.state == game_state::STARTED
                    && p.dropped
                    && !p.area.is_empty()
                    && new_chunk != p.chunk
                {
                    out.extend(self.chunk_changed(o, new_chunk));
                }
            }
        }
        out
    }

    /// `CarTemporaryUpdate`: the last driver pushes the empty car for a few seconds.
    fn car_push(&mut self, peer: u8, up: CarTemporaryUpdate) {
        let now = self.last_now.unwrap_or(0.0);
        if self.taxi_owns_motion(up.car_index) {
            return;
        }
        let Some(car) = self.cars.get_mut(&up.car_index) else {
            return;
        };
        if up.player_index != peer || !up.position.iter().all(|v| v.is_finite()) {
            return;
        }
        if car
            .temporary_owner
            .is_some_and(|(owner, until)| owner == peer && now <= until)
        {
            if car.position != up.position {
                car.temporary_changed = true;
            }
            car.position = up.position;
            car.rotation = up.rotation;
            car.input = PackedDirection::ZERO;
        }
    }

    /// `CarDamage`: a car part was hit; the reporter sends the part's new health.
    fn car_damage(&mut self, peer: u8, d: CarDamageReport) -> Vec<Outgoing> {
        if self.state != game_state::STARTED
            || !self.players.contains_key(&peer)
            || !d.new_health.is_finite()
        {
            return Vec::new();
        }
        if self.is_aircraft_car(d.car_index) {
            let known = self
                .cars
                .get(&d.car_index)
                .is_some_and(|c| c.parts.iter().any(|p| p.index == d.part));
            if !known {
                return Vec::new();
            }
            return self
                .aircraft_damage(peer, d.car_index, d.new_health)
                .unwrap_or_default();
        }
        let Some(car) = self.cars.get_mut(&d.car_index) else {
            return Vec::new();
        };
        let Some(part) = car.parts.iter_mut().find(|p| p.index == d.part) else {
            return Vec::new();
        };
        let dmg = (part.health - d.new_health).max(0.0);
        part.health -= dmg;
        let payload = encode_car_damage(d.part, d.car_index, part.health);
        let car_chunk = chunk_of(car.position);
        let mut to = self.watchers(car_chunk);
        if !to.contains(&peer) {
            to.push(peer);
        }
        vec![Outgoing::reliable(
            Recipients::Many(to),
            EventCode::CarDamage,
            payload,
        )]
    }

    /// Take a player out of their car seat (downed, dead or gone).
    fn eject(&mut self, peer: u8) {
        let Some(p) = self.players.get_mut(&peer) else {
            return;
        };
        if let Some((car, _, _)) = p.seat.take() {
            if let Some(c) = self.cars.get_mut(&car) {
                c.eject(peer);
            }
        }
    }

    /// `RequestWeaponPickUp`: take the item off the map and tell everyone around.
    fn pickup(&mut self, peer: u8, req: PickupRequest) -> Vec<Outgoing> {
        let ok = self
            .players
            .get(&peer)
            .is_some_and(|p| p.alive && p.dropped && !p.downed);
        if req.player_index != peer || !ok {
            return Vec::new();
        }
        let item_id = self.loot.get(req.network_id).map_or(-1, |i| i.item_id);
        if !self.bomb_pickup_allowed(peer, item_id) {
            return Vec::new();
        }
        let Some(item) = self.loot.take(req.network_id) else {
            return Vec::new();
        };
        self.inventory_add(peer, item.item_id, item.quantity);
        let mut to = self.watchers(chunk_of(item.position));
        if !to.contains(&peer) {
            to.push(peer);
        }
        vec![Outgoing::reliable(
            Recipients::Many(to),
            EventCode::WeaponPickUpAccepted,
            encode_pickup_accepted(peer, &item, req.slot),
        )]
    }

    /// `RequestItemDrop`: the player drops something; it becomes new loot.
    fn item_drop(&mut self, peer: u8, req: DropRequest) -> Vec<Outgoing> {
        let ok = self.players.get(&peer).is_some_and(|p| p.alive);
        if req.player_index != peer
            || !ok
            || req.quantity <= 0
            || !req.position.iter().all(|v| v.is_finite())
        {
            return Vec::new();
        }
        let mut req = req;
        match self.inventory_checked(peer, "drop", req.item_id, req.quantity) {
            Some(quantity) => req.quantity = quantity,
            None => return Vec::new(),
        }
        self.bomb_dropped(peer, req.item_id);
        self.inventory_remove(peer, req.item_id, req.quantity);
        let item = self.loot.add(req.item_id, req.quantity, req.position);
        let mut to = self.watchers(chunk_of(item.position));
        if !to.contains(&peer) {
            to.push(peer);
        }
        vec![Outgoing::reliable(
            Recipients::Many(to),
            EventCode::ItemDrop,
            encode_item_drop(&item),
        )]
    }

    /// Remember what a player holds, from `WeaponChange`
    /// (`[player, slot, w1, w1 attachment, w2, w2 attachment, w3, n, attachments…]`).
    /// With `game.inventory_check`, the weapons must be ones the player got; `false` refuses the
    /// change (enforce only).
    fn track_equipment(&mut self, peer: u8, payload: &[u8]) -> bool {
        let mut r = tabg_protocol::Reader::new(payload);
        let parsed = (|| -> Result<_, tabg_protocol::DecodeError> {
            let (index, slot) = (r.u8()?, r.u8()?);
            let mut shorts = [0i16; 5];
            for s in &mut shorts {
                *s = r.u16()? as i16;
            }
            let n = r.u8()?;
            let attachments = (0..n)
                .map(|_| r.u16().map(|v| v as i16))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((index, slot, shorts, attachments))
        })();
        let Ok((index, slot, w, attachments)) = parsed else {
            return true;
        };
        if index != peer {
            return true;
        }
        for weapon in [w[0], w[2], w[4]] {
            if weapon >= 0
                && self
                    .inventory_checked(peer, "equip", i32::from(weapon), 1)
                    .is_none()
            {
                return false;
            }
        }
        let Some(p) = self.players.get_mut(&peer) else {
            return true;
        };
        p.equipment = [w[0], w[1], w[2], w[3], w[4], i16::from(slot)];
        p.attachments = attachments;
        p.weapon_type = match slot {
            0 => i32::MIN,
            1 => i32::from(w[0]),
            2 => i32::from(w[2]),
            _ => i32::from(w[4]),
        };
        true
    }

    fn airplane_drop(&mut self, peer: u8, req: RequestAirplaneDrop) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        let Some(plane) = self.plane.as_ref().filter(|p| !p.finished) else {
            return Vec::new();
        };
        let position = plane.drop_position(now);
        let Some(p) = self.players.get_mut(&peer) else {
            return Vec::new();
        };
        if req.player_index != peer || p.dropped || !req.direction.iter().all(|v| v.is_finite()) {
            return Vec::new();
        }
        p.dropped = true;
        p.position = position;
        let msg = PlayerAirplaneDropped {
            player_index: peer,
            position,
            direction: req.direction,
        };
        let mut out = vec![Outgoing::reliable(
            Recipients::All,
            EventCode::PlayerAirplaneDropped,
            msg.encode(),
        )];
        out.extend(self.force_chunk_entry(peer));
        out
    }

    /// Forward a packet whose first byte is the sending player's index, unchanged.
    fn relay_from(
        &self,
        peer: u8,
        packet: &Packet,
        event: EventCode,
        to: Recipients,
    ) -> Vec<Outgoing> {
        if !self.players.contains_key(&peer) || packet.payload.first() != Some(&peer) {
            return Vec::new();
        }
        vec![Outgoing::reliable(to, event, packet.payload.clone())]
    }

    /// `PlayerFire`: relayed to everyone else; a sync reply goes back to the shooter if asked.
    fn player_fire(&mut self, peer: u8, packet: &Packet) -> Vec<Outgoing> {
        let Ok(header) = PlayerFireHeader::decode(&packet.payload) else {
            return Vec::new();
        };
        if header.player_index != peer || !self.players.get(&peer).is_some_and(|p| p.alive) {
            return Vec::new();
        }
        if self.is_princess_mode()
            && let Some(p) = self.players.get(&peer)
        {
            let noise = bots::Noise {
                at: p.position,
                time: self.last_now.unwrap_or(0.0),
                group: p.group,
            };
            self.noises.push(noise);
        }
        if header.ammo_type > -1 && header.shots > -1 {
            // Whatever a gun fires is ammo, even without the world's item names.
            self.settings.ammo.insert(header.ammo_type);
            self.inventory_remove(peer, header.ammo_type, header.shots);
        }
        let mut out = Vec::new();
        if header.mode & firing_mode::WANTS_TO_BE_SYNCED != 0 {
            if let Some(reply) = PlayerFireHeader::sync_return(&packet.payload) {
                out.push(Outgoing::reliable(
                    Recipients::One(peer),
                    EventCode::PlayerFireSyncReturn,
                    reply,
                ));
            }
        }
        out.push(Outgoing::reliable(
            Recipients::AllExcept(peer),
            EventCode::PlayerFire,
            packet.payload.clone(),
        ));
        out
    }

    /// `DamageEvent`, following the official rules: only during the match, only between players
    /// who left the plane, a short grace period right after going down, and reduced damage while
    /// downed. Reaching 0 health downs a player first; a downed player at 0 dies.
    fn damage(&mut self, peer: u8, ev: DamageEvent) -> Vec<Outgoing> {
        let now = self.last_now.unwrap_or(0.0);
        if self.state != game_state::STARTED
            || (peer != ev.attacker && peer != ev.victim)
            || !ev.new_health.is_finite()
        {
            return Vec::new();
        }
        let attacker_ok = self.players.get(&ev.attacker).is_some_and(|a| a.dropped);
        let Some(victim) = self.players.get_mut(&ev.victim) else {
            return Vec::new();
        };
        if !attacker_ok
            || !victim.dropped
            || !victim.alive
            || (victim.downed && now - victim.down_time < 1.0)
        {
            return Vec::new();
        }
        let mut dmg = (victim.health - ev.new_health).max(0.0);
        if victim.downed {
            dmg = dmg.clamp(0.0, 50.0) * 0.12;
        }
        if let Some(b) = self.bots.get_mut(&ev.victim) {
            b.hurt_at = now;
        }
        let Some(victim) = self.players.get_mut(&ev.victim) else {
            return Vec::new();
        };
        victim.health -= dmg;
        if !victim.downed {
            victim.last_attacker = ev.attacker;
        }
        let damaged = PlayerDamaged {
            victim: ev.victim,
            attacker: ev.attacker,
            health: victim.health,
            downed: victim.downed,
            damaged: dmg != 0.0,
            direction: PackedDirection::pack(ev.direction),
            hit: ev.hit.clone(),
        };
        let to = if peer != ev.attacker && ev.exclude_victim {
            Recipients::AllExcept(ev.victim)
        } else {
            Recipients::All
        };
        let damaged = Outgoing::reliable(to, EventCode::PlayerDamaged, damaged.encode());

        if victim.health > 0.0 {
            return vec![damaged];
        }
        if victim.downed {
            return self.kill(ev.victim, false);
        }
        // Down instead of dead (and out of any car)…
        victim.seat = None;
        for c in self.cars.values_mut() {
            c.eject(ev.victim);
        }
        let victim = self.players.get_mut(&ev.victim).expect("still here");
        victim.downed = true;
        victim.health = 100.0;
        victim.down_time = now;
        let group = victim.group;
        // …unless the whole team is now down or dead: then everyone in it dies.
        let team_down = self
            .players
            .values()
            .filter(|p| p.group == group)
            .all(|p| !p.alive || p.downed);
        if !team_down {
            return vec![damaged];
        }
        let members: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.group == group && p.alive)
            .map(|p| p.index)
            .collect();
        members
            .into_iter()
            .flat_map(|m| self.kill(m, false))
            .collect()
    }

    /// Kill a player: `PlayerDead` to everyone, and `TeamDead` with the placement when that was
    /// the team's last player.
    fn kill(&mut self, victim: u8, ring_death: bool) -> Vec<Outgoing> {
        if self.state != game_state::STARTED {
            return Vec::new();
        }
        if !self.players.get(&victim).is_some_and(|p| p.alive) {
            return Vec::new();
        }
        if let Some(i) = self.princess_of(victim) {
            return self.princess_killed(victim, i);
        }
        self.bot_died(victim);
        let Some(p) = self.players.get_mut(&victim) else {
            return Vec::new();
        };
        p.alive = false;
        p.downed = false;
        p.health = 0.0;
        if let Some((car, _, _)) = p.seat.take() {
            if let Some(c) = self.cars.get_mut(&car) {
                c.eject(victim);
            }
        }
        let p = self.players.get_mut(&victim).expect("still here");
        let (group, last_attacker, name) = (p.group, p.last_attacker, p.name.clone());
        let mut killer_name = None;
        let mut killer_group = None;
        if let Some(killer) = self
            .players
            .get_mut(&last_attacker)
            .filter(|k| k.group != group)
        {
            killer.kills += 1;
            killer_name = Some(killer.name.clone());
            killer_group = Some(killer.group);
        }
        self.events.push(RoomEvent::Killed {
            victim: name.clone(),
            killer: killer_name,
            ring: ring_death,
        });
        let spectate = if self.enters_boss_fight(victim) {
            let p = self.players.get_mut(&victim).expect("still here");
            p.boss_fight = true;
            p.boss_done = true;
            extras::BOSS_FIGHT
        } else {
            self.players
                .values()
                .find(|m| m.group == group && m.alive && m.index != victim)
                .map_or(NOBODY, |m| m.index)
        };
        let dead = PlayerDead {
            victim,
            last_attacker,
            spectate,
            victim_name: name,
            killer_weapon: NO_WEAPON,
            ring_death,
        };
        let mut out = vec![Self::broadcast(EventCode::PlayerDead, dead.encode())];
        if self.is_bomb() {
            if let Some(p) = self.players.get_mut(&victim) {
                p.inventory.clear();
            }
            out.extend(self.bomb_kill(victim, last_attacker));
            return out;
        }
        // Official BattleRoyale/Brawl/Test: the dead drop everything (and a soul). Not in gun
        // game, where it would hand out other levels' weapons.
        if self.settings.drop_loot_on_death && self.settings.gun_game.is_empty() {
            out.extend(self.drop_inventory(victim));
        }
        if self.is_princess_mode() {
            // Back at the cage after the respawn delay; the kill counts toward a helicopter.
            let now = self.last_now.unwrap_or(0.0);
            let delay = self.settings.respawn.map_or(5.0, |r| r.delay.max(0.0));
            if let Some(p) = self.players.get_mut(&victim) {
                p.respawn_at = Some(now + delay);
            }
            if let Some(g) = killer_group {
                out.extend(self.princess_kill_credit(g));
            }
            return out;
        }
        if self.is_brawl() {
            // A wiped team comes back at its house (official `BrawlGameMode.TeamDead`).
            if !self.players.values().any(|m| m.group == group && m.alive) {
                let at = self.last_now.unwrap_or(0.0)
                    + self.settings.respawn.map_or(3.0, |r| r.delay.max(0.0));
                for m in self.players.values_mut().filter(|m| m.group == group) {
                    m.respawn_at = Some(at);
                }
            }
            return out;
        }
        if let Some(respawn) = self.settings.respawn {
            let now = self.last_now.unwrap_or(0.0);
            if let Some(p) = self.players.get_mut(&victim) {
                p.respawn_at = Some(now + respawn.delay.max(0.0));
            }
            if !self.settings.gun_game.is_empty() && last_attacker != victim {
                if let Some(k) = self
                    .players
                    .get_mut(&last_attacker)
                    .filter(|k| k.group != group)
                {
                    k.level += 1;
                    let level = k.level;
                    if let Some(items) = self.settings.gun_game.get(level).cloned() {
                        out.push(self.grant(last_attacker, &items));
                    }
                }
            }
            return out;
        }
        out.extend(self.team_dead_if_out(group));
        out
    }

    /// `TeamDead` once nobody in `group` is alive or still fighting the boss.
    fn team_dead_if_out(&self, group: u8) -> Vec<Outgoing> {
        if self
            .players
            .values()
            .any(|m| m.group == group && (m.alive || m.boss_fight))
        {
            return Vec::new();
        }
        let placement = u8::try_from(self.alive_teams().len() + 1).unwrap_or(u8::MAX);
        vec![Self::broadcast(
            EventCode::TeamDead,
            TeamDead {
                group,
                placement,
                finished: Vec::new(),
            }
            .encode(),
        )]
    }

    /// `ReviveState`: a teammate starts, stops or finishes reviving a downed player.
    fn revive(&mut self, peer: u8, req: ReviveRequest) -> Vec<Outgoing> {
        if req.reviver != peer {
            return Vec::new();
        }
        let reviver_group = match self.players.get(&req.reviver) {
            Some(r) if r.alive && !r.downed => r.group,
            _ => return Vec::new(),
        };
        let Some(target) = self.players.get_mut(&req.revived) else {
            return Vec::new();
        };
        if !target.alive || !target.downed || target.group != reviver_group {
            return Vec::new();
        }
        if req.stage == ReviveStage::Finished {
            target.downed = false;
            target.health = 15.0;
        }
        let payload = encode_revive_state(req.stage, req.revived, req.reviver, target.health);
        vec![Self::broadcast(EventCode::ReviveState, payload)]
    }

    fn broadcast(event: EventCode, payload: Vec<u8>) -> Outgoing {
        Outgoing::reliable(Recipients::All, event, payload)
    }

    fn state_changed(msg: GameStateChanged) -> Outgoing {
        Self::broadcast(EventCode::GameStateChanged, msg.encode())
    }

    fn ring_events(events: Vec<RingEvent>) -> impl Iterator<Item = Outgoing> {
        events.into_iter().map(|e| {
            let msg = match e {
                RingEvent::NextRing {
                    ring_id,
                    center,
                    size,
                } => RingUpdate::NextRing {
                    ring_id,
                    center,
                    size,
                },
                RingEvent::StartedMoving => RingUpdate::StartedMoving,
            };
            Self::broadcast(EventCode::RingUpdate, msg.encode())
        })
    }

    /// The landing/respawn items: the loadout plus the player's gun game level.
    fn give_loadout(&mut self, peer: u8) -> Vec<Outgoing> {
        if self.is_princess_mode() {
            return self.princess_loadout(peer);
        }
        let mut items = self.settings.loadout.clone();
        if let Some(level) = self.players.get(&peer).and_then(|p| {
            self.settings
                .gun_game
                .get(p.level.min(self.settings.gun_game.len().saturating_sub(1)))
        }) {
            items.extend(level.iter().copied());
        }
        if items.is_empty() {
            return Vec::new();
        }
        vec![self.grant(peer, &items)]
    }

    /// Somewhere on the ground inside the ring: a random loot spot (they sit on floors and
    /// terrain), else a lobby spawn.
    fn respawn_position(&mut self, group: u8) -> [f32; 3] {
        if self.is_brawl() || self.is_princess_mode() {
            return self.spawn_for(group).position;
        }
        let (c, r) = (self.ring.white_center, self.ring.white_size * 0.45);
        let inside: Vec<[f32; 3]> = self
            .settings
            .loot
            .spots
            .iter()
            .map(|s| s.position)
            .filter(|p| (p[0] - c[0]).hypot(p[2] - c[2]) < r)
            .collect();
        let pool: Vec<[f32; 3]> = if inside.is_empty() {
            self.settings
                .loot
                .spots
                .iter()
                .map(|s| s.position)
                .collect()
        } else {
            inside
        };
        if pool.is_empty() {
            return self.next_spawn_point().position;
        }
        let p = pool[self.rng.index(pool.len())];
        [p[0], p[1] + 1.0, p[2]]
    }

    /// Bring back dead players whose respawn time has come (`PlayerRespawn`).
    fn respawn_due(&mut self, now: f32) -> Vec<Outgoing> {
        let due: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.respawn_at.is_some_and(|t| now >= t))
            .map(|p| p.index)
            .collect();
        let mut out = Vec::new();
        for peer in due {
            let group = self.players[&peer].group;
            let position = self.respawn_position(group);
            let rotation = self
                .cage_facing(group, position)
                .unwrap_or_else(|| self.rng.range(0.0, 360.0));
            let p = self.players.get_mut(&peer).expect("listed above");
            p.respawn_at = None;
            p.alive = true;
            p.downed = false;
            p.health = 100.0;
            p.dropped = true;
            p.landed = true;
            p.position = position;
            p.last_attacker = NOBODY;
            p.equipment = NO_EQUIPMENT;
            p.attachments.clear();
            p.weapon_type = NO_WEAPON;
            p.seat = None;
            p.inventory.clear();
            // [count][index][new index][health][x][y][z][rotation][curse 255 = none]
            let mut w = tabg_protocol::codec::Writer::new();
            w.u8(1).u8(peer).u8(peer).f32(100.0);
            w.f32(position[0])
                .f32(position[1])
                .f32(position[2])
                .f32(rotation)
                .u8(u8::MAX);
            out.push(Self::broadcast(EventCode::PlayerRespawn, w.into_inner()));
            out.extend(self.chunk_changed(peer, chunk_of(position)));
            out.extend(self.give_loadout(peer));
            out.extend(self.princess_regrant(peer));
        }
        out
    }

    /// Brawl: first team to `kills_to_win`, or the last team left (official `BrawlGameMode`).
    fn brawl_winner(&self) -> Option<u8> {
        let mut team_kills: BTreeMap<u8, u32> = BTreeMap::new();
        for p in self.players.values() {
            *team_kills.entry(p.group).or_default() += u32::from(p.kills);
        }
        let best = team_kills
            .iter()
            .max_by_key(|(_, k)| **k)
            .map(|(&t, &k)| (t, k));
        let limit = u32::from(self.settings.kills_to_win.max(1));
        match best {
            Some((team, kills)) if kills >= limit => Some(team),
            Some((team, _)) if team_kills.len() <= 1 => Some(team),
            None => Some(GameStateChanged::NO_WINNER),
            _ => None,
        }
    }

    /// With respawns: has a team reached the kill limit, finished gun game, or is time up?
    fn respawn_winner(&self, now: f32) -> Option<u8> {
        let mut team_kills: BTreeMap<u8, u32> = BTreeMap::new();
        for p in self.players.values() {
            *team_kills.entry(p.group).or_default() += u32::from(p.kills);
        }
        let best = team_kills
            .iter()
            .max_by_key(|(_, k)| **k)
            .map(|(&team, &kills)| (team, kills));
        let levels = self.settings.gun_game.len();
        if levels > 0
            && let Some(p) = self.players.values().find(|p| p.level >= levels)
        {
            return Some(p.group);
        }
        let limit = u32::from(self.settings.kills_to_win);
        if limit > 0
            && levels == 0
            && let Some((team, kills)) = best
            && kills >= limit
        {
            return Some(team);
        }
        if self.settings.time_limit > 0.0 && now - self.match_started_at >= self.settings.time_limit
        {
            return Some(best.map_or(GameStateChanged::NO_WINNER, |(t, _)| t));
        }
        if self.players.is_empty() {
            return Some(GameStateChanged::NO_WINNER);
        }
        None
    }

    /// Groups that still have a living player.
    fn alive_teams(&self) -> Vec<u8> {
        let mut teams: Vec<u8> = self
            .players
            .values()
            .filter(|p| p.alive || p.boss_fight)
            .map(|p| p.group)
            .collect();
        teams.sort_unstable();
        teams.dedup();
        teams
    }

    /// Advance the match: countdown, plane, ring, end of match, restart.
    fn tick_match(&mut self, now: f32, dt: f32) -> Vec<Outgoing> {
        if self.is_bomb() && self.state != game_state::ENDED {
            return self.tick_bomb(now, dt);
        }
        let mut out = Vec::new();
        match self.state {
            game_state::WAITING_FOR_PLAYERS => {
                // Capture the Princess fills its teams with bots: only people count.
                let players = if self.is_princess_mode() {
                    self.players.keys().filter(|p| !self.is_bot(**p)).count()
                } else {
                    self.players.len()
                };
                let enough = players >= usize::from(self.settings.players_to_start.max(1));
                let forced = self.settings.force_start.is_some_and(|f| {
                    now + 2.0 > f.after_seconds && players >= usize::from(f.min_players.max(1))
                });
                if enough || forced {
                    self.state = game_state::COUNT_DOWN;
                    self.events.push(RoomEvent::MatchState {
                        state: "countdown".into(),
                    });
                    self.countdown_left = self.settings.countdown;
                    out.push(Self::state_changed(GameStateChanged::CountDown {
                        seconds: self.countdown_left,
                    }));
                }
            }
            game_state::COUNT_DOWN => {
                self.countdown_left -= dt;
                if self.countdown_left <= 0.0 {
                    out.extend(self.start_match(now));
                }
            }
            game_state::STARTED if self.is_princess_mode() => {
                out.extend(self.respawn_due(now));
                out.extend(self.tick_princess(now, dt));
                if let Some(winner) = self.princess_winner() {
                    out.extend(self.end_match(now, winner));
                }
            }
            game_state::STARTED => {
                if let Some(plane) = self.plane.as_mut().filter(|p| !p.finished) {
                    if now - plane.last_sync >= plane::SYNC_INTERVAL {
                        plane.last_sync = now;
                        out.push(Self::broadcast(
                            EventCode::PlaneUpdate,
                            encode_plane_update(plane.progress(now)),
                        ));
                    }
                    if now - plane.took_off_at >= plane::FLIGHT_TIME {
                        plane.finished = true;
                        let position = plane.drop_position(now);
                        let still_aboard: Vec<u8> = self
                            .players
                            .values()
                            .filter(|p| !p.dropped)
                            .map(|p| p.index)
                            .collect();
                        for &i in &still_aboard {
                            let p = self.players.get_mut(&i).expect("listed above");
                            p.dropped = true;
                            p.position = position;
                        }
                        out.extend(still_aboard.iter().map(|&i| {
                            Outgoing::reliable(
                                Recipients::One(i),
                                EventCode::AllDrop,
                                encode_all_drop(position),
                            )
                        }));
                        for i in still_aboard {
                            out.extend(self.force_chunk_entry(i));
                        }
                    }
                }
                let ring_events = self.ring.tick(now, &mut self.rng);
                out.extend(Self::ring_events(ring_events));
                out.extend(self.tick_spell_drops(now));
                let winner = if self.is_brawl() {
                    out.extend(self.respawn_due(now));
                    self.brawl_winner()
                } else if self.settings.respawn.is_some() {
                    out.extend(self.respawn_due(now));
                    self.respawn_winner(now)
                } else {
                    let teams = self.alive_teams();
                    (teams.len() <= 1).then(|| {
                        teams
                            .first()
                            .copied()
                            .unwrap_or(GameStateChanged::NO_WINNER)
                    })
                };
                if let Some(winner) = winner {
                    out.extend(self.end_match(now, winner));
                }
            }
            game_state::ENDED => {
                if self.ended_at.is_some_and(|t| now - t >= RESTART_AFTER_END) {
                    let humans: Vec<u8> = self
                        .players
                        .keys()
                        .copied()
                        .filter(|p| !self.is_bot(*p))
                        .collect();
                    self.to_disconnect.extend(humans);
                    self.players.clear();
                    self.bots.clear();
                }
                if self.players.is_empty() {
                    self.reset();
                }
            }
            _ => {}
        }
        out
    }

    /// The match is over: `winner` (a group, or `NO_WINNER`) gets the win screen.
    fn end_match(&mut self, now: f32, winner: u8) -> Vec<Outgoing> {
        self.state = game_state::ENDED;
        self.ended_at = Some(now);
        let players = self
            .players
            .values()
            .filter(|p| p.group == winner)
            .map(|p| p.name.clone())
            .collect();
        self.events.push(RoomEvent::Won {
            team: winner,
            players,
        });
        vec![Self::state_changed(GameStateChanged::Ended { winner })]
    }

    /// Countdown over: everyone boards the plane and the ring starts.
    fn start_match(&mut self, now: f32) -> Vec<Outgoing> {
        for p in self.players.values_mut() {
            p.dropped = false;
            p.landed = false;
            p.alive = true;
            p.health = 100.0;
            p.downed = false;
            p.last_attacker = NOBODY;
            p.kills = 0;
            p.area.clear();
            p.respawn_at = None;
            p.level = 0;
            p.inventory.clear();
            p.boss_fight = false;
            p.boss_done = false;
        }
        self.souls.clear();
        self.drops.clear();
        self.next_drop_at = None;
        self.match_started_at = now;
        self.bots_board();
        self.destruction.reset_pending = true;
        let (modifier, loot) =
            LootWorld::spawn(&self.settings.loot, self.settings.loot_keep, &mut self.rng);
        self.modifier = modifier;
        self.loot = loot;
        if self.is_princess_mode() {
            return self.start_princess_match();
        }
        if self.is_brawl() {
            // Brawl: same loot in every house, then the doors open (official `OpenDoors`).
            self.loot = self.settings.brawl.spawn_loot(self.solo(), &mut self.rng);
            self.state = game_state::STARTED;
            self.events.push(RoomEvent::MatchState {
                state: "started".into(),
            });
            let mut out = vec![Self::state_changed(GameStateChanged::Other(
                game_state::OPEN_DOORS,
            ))];
            let peers: Vec<u8> = self.players.keys().copied().collect();
            for &peer in &peers {
                let p = self.players.get_mut(&peer).expect("listed");
                p.dropped = true;
                p.landed = true;
            }
            for peer in peers {
                out.extend(self.force_chunk_entry(peer));
                out.extend(self.give_loadout(peer));
            }
            return out;
        }
        if self.settings.game_mode == tabg_protocol::messages::game_mode::TEST {
            // Test mode: no plane and no ring; everyone plays from where they stand.
            self.state = game_state::STARTED;
            self.events.push(RoomEvent::MatchState {
                state: "started".into(),
            });
            let mut out = vec![Self::state_changed(GameStateChanged::Started)];
            let peers: Vec<u8> = self.players.keys().copied().collect();
            for &peer in &peers {
                let p = self.players.get_mut(&peer).expect("listed");
                p.dropped = true;
                p.landed = true;
            }
            for peer in peers {
                out.extend(self.force_chunk_entry(peer));
                out.extend(self.give_loadout(peer));
            }
            return out;
        }
        let plane = Plane::take_off(self.ring.white_center, now, &mut self.rng);
        let flying = GameStateChanged::Flying {
            start: plane.start,
            end: plane.end,
            time_of_day: self.settings.time_of_day,
            modifier,
        };
        self.plane = Some(plane);
        // The official server is internally `Started` while players are still in the plane.
        self.state = game_state::STARTED;
        self.events.push(RoomEvent::MatchState {
            state: "started".into(),
        });
        let ring_events = self.ring.start(now);
        self.schedule_first_drop(now);
        let mut out = vec![Self::state_changed(flying)];
        out.extend(Self::ring_events(ring_events));
        out
    }

    /// Back to the lobby for the next match.
    fn reset(&mut self) {
        self.state = game_state::WAITING_FOR_PLAYERS;
        self.events.push(RoomEvent::MatchState {
            state: "lobby".into(),
        });
        self.plane = None;
        self.ended_at = None;
        self.countdown_left = 0.0;
        self.ring = Ring::new(self.settings.ring.clone(), &mut self.rng);
        self.spawn_cars();
        self.destruction.reset_pending = true;
        self.princess
            .princesses
            .retain(|p| self.players.contains_key(&p.index));
    }

    /// Advance to `now` (seconds since the room started): run the match, and at the tick rate send
    /// every in-world player a `PlayerUpdate` batch with everyone else's movement.
    pub fn tick(&mut self, now: f32) -> Vec<Outgoing> {
        let dt = self.last_now.map_or(0.0, |t| (now - t).max(0.0));
        self.last_now = Some(now);
        let mut out = self.tick_match(now, dt);
        out.extend(self.tick_bots(now, dt));
        out.extend(self.tick_radar(now));
        out.extend(self.tick_destruction());
        if self.state == game_state::STARTED {
            out.extend(self.tick_aircraft(now, dt));
            out.extend(self.tick_missiles(now, dt));
            out.extend(self.tick_drones(now));
            out.extend(self.tick_taxis(now));
        }
        out.extend(self.tick_movement(now));
        out
    }

    fn tick_movement(&mut self, now: f32) -> Vec<Outgoing> {
        let interval = 1.0 / self.settings.tick_rate.max(1) as f32;
        if now - self.last_tick < interval {
            return Vec::new();
        }
        self.last_tick = now;

        // During a match, players still in the plane (or dead) are not replicated.
        let in_match = self.state == game_state::STARTED;
        let movers: Vec<(EntityUpdate, (i8, i8))> = self
            .players
            .values()
            .filter(|p| p.in_world && p.dirty && (!in_match || (p.dropped && p.alive)))
            .map(|p| (self.entity_update(p), chunk_of(p.position)))
            .collect();
        // Empty cars pushed around by their last driver.
        let pushed: Vec<(MovedCar, (i8, i8))> = self
            .cars
            .values()
            .filter(|c| c.temporary_changed)
            .map(|c| {
                (
                    MovedCar {
                        car_index: c.index,
                        position: c.position,
                        rotation: c.rotation.clone(),
                    },
                    chunk_of(c.position),
                )
            })
            .collect();
        if movers.is_empty() && pushed.is_empty() {
            return Vec::new();
        }
        for p in self.players.values_mut() {
            p.dirty = false;
        }
        for c in self.cars.values_mut() {
            c.temporary_changed = false;
        }

        // In the lobby everyone sees everyone; during the match only what is in the receiver's
        // 3×3 chunk area (players still in the plane see nobody).
        self.players
            .values()
            .filter(|r| r.in_world)
            .filter_map(|receiver| {
                let visible = |chunk: &(i8, i8)| !in_match || receiver.area.contains(chunk);
                let entities: Vec<_> = movers
                    .iter()
                    .filter(|(e, c)| e.player_index != receiver.index && visible(c))
                    .map(|(e, _)| e.clone())
                    .collect();
                let cars: Vec<_> = pushed
                    .iter()
                    .filter(|(_, c)| visible(c))
                    .map(|(m, _)| m.clone())
                    .collect();
                (!entities.is_empty() || !cars.is_empty()).then(|| Outgoing {
                    to: Recipients::One(receiver.index),
                    packet: Packet::new(
                        EventCode::PlayerUpdate,
                        PlayerUpdateBatch {
                            timestamp: now,
                            entities,
                            cars,
                        }
                        .encode(),
                    ),
                    reliable: false,
                })
            })
            .collect()
    }

    /// What others see of `p`: on foot, as a passenger, or as the driver (with the car's pose).
    fn entity_update(&self, p: &Player) -> EntityUpdate {
        // Bots stand on the grid with their feet; the client wants the hip.
        let mut position = p.position;
        if self.is_bot(p.index) {
            position[1] += bots::HIP;
        }
        match p.seat {
            Some((car_index, _, true)) if self.cars.contains_key(&car_index) => {
                let car = &self.cars[&car_index];
                EntityUpdate {
                    player_index: p.index,
                    flags: container_flags::ALL,
                    driving_state: driving_state::DRIVING,
                    body: EntityBody::Driving {
                        car_position: Some(car.position),
                        car_rotation: Some(car.rotation.clone()),
                        car_input: Some(car.input),
                        rotation: Some(p.rotation),
                        car_driving_state: car.driving_state,
                    },
                }
            }
            Some(_) => EntityUpdate::on_foot(
                p.index,
                position,
                p.rotation,
                p.ads,
                p.direction,
                p.movement,
                driving_state::INSIDE_CAR,
            ),
            None => EntityUpdate::on_foot(
                p.index,
                position,
                p.rotation,
                p.ads,
                p.direction,
                p.movement,
                0,
            ),
        }
    }
}

/// TABG strips surrounding `<`/`>` (rich-text tags) and falls back to "Player".
/// A stable id for a player: the Epic product user id, else PlayFab, else Unity, else the name.
fn player_id(req: &RoomInitRequest) -> String {
    let epic: Vec<u8> = req
        .epic_product_user_id
        .iter()
        .copied()
        .take_while(|&b| b != 0)
        .collect();
    let epic = String::from_utf8_lossy(&epic).trim().to_owned();
    if !epic.is_empty() {
        return format!("epic:{epic}");
    }
    if !req.playfab_id.trim().is_empty() {
        return format!("playfab:{}", req.playfab_id.trim());
    }
    if let Some(unity) = req
        .unity_id
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        return format!("unity:{unity}");
    }
    format!("name:{}", clean_name(&req.player_name))
}

fn clean_name(raw: &str) -> String {
    let trimmed = raw.trim_matches(|c| c == '<' || c == '>').trim();
    if trimmed.is_empty() {
        "Player".into()
    } else {
        trimmed.chars().take(32).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tabg_protocol::actions::encode_player_loot;
    use tabg_protocol::combat::{DamageEvent, PlayerDamaged, PlayerDead, TeamDead};
    use tabg_protocol::loot::{ChunkEntry, ChunkExit, DropRequest, PickupRequest};
    use tabg_protocol::match_flow::{GameStateChanged, PlayerAirplaneDropped, RingUpdate};
    use tabg_protocol::messages::PackedQuaternion;
    use tabg_protocol::messages::{EntityBody, game_mode};

    /// Settings for lobby-only tests: the match never starts.
    fn lobby() -> RoomSettings {
        RoomSettings {
            players_to_start: 100,
            force_start: None,
            ..Default::default()
        }
    }

    fn events(out: &[Outgoing], event: EventCode) -> Vec<&Outgoing> {
        out.iter()
            .filter(|o| o.packet.event() == Some(event))
            .collect()
    }

    /// Two players in the world of a room that starts with 2 players after a 10 s countdown.
    fn match_room() -> Room {
        let mut room = Room::new(RoomSettings {
            countdown: 10.0,
            ..Default::default()
        });
        for (peer, name) in [(0, "a"), (1, "b")] {
            join(&mut room, peer, name);
            enter(&mut room, peer);
        }
        room
    }

    /// Tick from `from` to `to` in 0.1 s steps and collect everything sent.
    fn run(room: &mut Room, from: f32, to: f32) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= to + 1e-4 {
            out.extend(room.tick(t));
            t += 0.1;
        }
        out
    }

    fn join(room: &mut Room, peer: u8, name: &str) -> Vec<Outgoing> {
        room.connect(peer);
        let req = RoomInitRequest {
            player_name: name.into(),
            squad_size: 1,
            ..Default::default()
        };
        room.handle(peer, &Packet::new(EventCode::RoomInit, req.encode()))
    }

    fn enter(room: &mut Room, peer: u8) -> WorldState {
        let out = room.handle(peer, &Packet::new(EventCode::RequestWorldState, vec![]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, Recipients::One(peer));
        WorldState::decode(&out[0].packet.payload, game_mode::BATTLE_ROYALE).unwrap()
    }

    #[test]
    fn join_announces_then_accepts_like_the_official_server() {
        let mut room = Room::new(lobby());
        let out = join(&mut room, 3, "<Jonas>");
        assert_eq!(out.len(), 2);
        assert_eq!(
            (out[0].to.clone(), out[0].packet.event()),
            (Recipients::All, Some(EventCode::Login))
        );
        assert_eq!(
            PlayerJoined::decode(&out[0].packet.payload)
                .unwrap()
                .player_name,
            "Jonas"
        );
        match RoomInitResponse::decode(&out[1].packet.payload).unwrap() {
            RoomInitResponse::Accepted { player_index, .. } => assert_eq!(player_index, 3),
            other => panic!("refused: {other:?}"),
        }
    }

    #[test]
    fn wrong_password_and_full_room_are_refused() {
        let mut room = Room::new(RoomSettings {
            password: "pw".into(),
            max_players: 1,
            ..Default::default()
        });
        let out = join(&mut room, 1, "a");
        assert!(
            matches!(RoomInitResponse::decode(&out[0].packet.payload).unwrap(), RoomInitResponse::Refused { response, .. } if response & server_response::WRONG_PASSWORD != 0)
        );

        room.connect(1);
        let ok = RoomInitRequest {
            player_name: "a".into(),
            password: Some("pw".into()),
            unity_id: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            room.handle(1, &Packet::new(EventCode::RoomInit, ok.encode()))
                .len(),
            2
        );
        let full = join(&mut room, 2, "b");
        assert!(matches!(
            RoomInitResponse::decode(&full[0].packet.payload).unwrap(),
            RoomInitResponse::Refused { .. }
        ));
    }

    #[test]
    fn world_state_spawns_the_player_and_lists_everyone() {
        let spawn = SpawnPoint {
            position: [1.0, 2.0, 3.0],
            rotation: 45.0,
        };
        let mut room = Room::new(RoomSettings {
            spawn_points: vec![spawn],
            ..Default::default()
        });
        join(&mut room, 0, "a");
        join(&mut room, 1, "b");
        let world = enter(&mut room, 1);
        assert_eq!(
            (world.player_index, world.position, world.rotation),
            (1, spawn.position, 45.0)
        );
        assert_eq!(world.players.len(), 2);
        assert_eq!(world.rings.len(), 6);
        assert!(
            room.handle(1, &Packet::new(EventCode::RequestWorldState, vec![]))
                .is_empty(),
            "only once"
        );
    }

    #[test]
    fn movement_reaches_other_players_only() {
        let mut room = Room::new(lobby());
        for (peer, name) in [(0, "a"), (1, "b")] {
            join(&mut room, peer, name);
            enter(&mut room, peer);
        }
        let up = ClientPlayerUpdate {
            player_index: 0,
            position: [5.0, 6.0, 7.0],
            rotation: [0.0, 90.0],
            aiming_down_sights: false,
            direction: PackedDirection::ZERO,
            movement: 0,
        };
        room.handle(0, &Packet::new(EventCode::PlayerUpdate, up.encode()));

        let out = room.tick(1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, Recipients::One(1));
        assert!(!out[0].reliable);
        let batch = PlayerUpdateBatch::decode(&out[0].packet.payload).unwrap();
        assert!(matches!(
            &batch.entities[..],
            [EntityUpdate {
                player_index: 0,
                body: EntityBody::OnFoot {
                    position: Some([5.0, 6.0, 7.0]),
                    ..
                },
                ..
            }]
        ));

        assert!(room.tick(1.01).is_empty(), "rate limited");
        assert!(room.tick(2.0).is_empty(), "nothing moved");
    }

    #[test]
    fn spoofed_or_broken_updates_are_ignored() {
        let mut room = Room::new(lobby());
        for peer in [0, 1] {
            join(&mut room, peer, "x");
            enter(&mut room, peer);
        }
        let spoof = ClientPlayerUpdate {
            player_index: 1,
            position: [9.0; 3],
            rotation: [0.0; 2],
            aiming_down_sights: false,
            direction: PackedDirection::ZERO,
            movement: 0,
        };
        room.handle(0, &Packet::new(EventCode::PlayerUpdate, spoof.encode()));
        let nan = ClientPlayerUpdate {
            player_index: 0,
            position: [f32::NAN, 0.0, 0.0],
            ..spoof
        };
        room.handle(0, &Packet::new(EventCode::PlayerUpdate, nan.encode()));
        room.handle(0, &Packet::new(EventCode::PlayerUpdate, vec![1, 2, 3]));
        assert!(room.tick(1.0).is_empty());
    }

    #[test]
    fn leaving_is_broadcast_and_ping_is_echoed() {
        let mut room = Room::new(lobby());
        join(&mut room, 4, "a");
        assert_eq!(
            room.disconnect(4),
            vec![Outgoing::reliable(
                Recipients::All,
                EventCode::PlayerLeft,
                vec![4, 1]
            )]
        );
        assert!(room.disconnect(4).is_empty());
        let ping = room.handle(2, &Packet::new(EventCode::TABGPing, vec![9, 9]));
        assert_eq!(
            ping,
            vec![Outgoing::reliable(
                Recipients::One(2),
                EventCode::PingRecieved,
                vec![9, 9]
            )]
        );
    }

    #[test]
    fn squad_members_share_a_group() {
        let mut room = Room::new(lobby());
        for peer in [0, 1] {
            room.connect(peer);
            let req = RoomInitRequest {
                player_name: format!("p{peer}"),
                squad_id: 77,
                ..Default::default()
            };
            room.handle(peer, &Packet::new(EventCode::RoomInit, req.encode()));
        }
        let groups: Vec<_> = (0..2).map(|p| enter(&mut room, p).group_index).collect();
        assert_eq!(groups[0], groups[1]);
    }

    #[test]
    fn countdown_starts_with_enough_players_then_the_plane_takes_off() {
        let mut room = match_room();
        let out = run(&mut room, 0.0, 0.0);
        let cd = events(&out, EventCode::GameStateChanged);
        assert_eq!(
            GameStateChanged::decode(&cd[0].packet.payload).unwrap(),
            GameStateChanged::CountDown { seconds: 10.0 }
        );
        assert_eq!(room.state(), game_state::COUNT_DOWN);

        // Joining during the countdown is still allowed, with the countdown in the world state.
        join(&mut room, 2, "late");
        assert!(matches!(
            enter(&mut room, 2).phase,
            WorldPhase::CountDown { .. }
        ));

        let out = run(&mut room, 0.1, 10.5);
        let changes: Vec<_> = events(&out, EventCode::GameStateChanged)
            .iter()
            .map(|o| GameStateChanged::decode(&o.packet.payload).unwrap())
            .collect();
        let [GameStateChanged::Flying { start, end, .. }] = changes[..] else {
            panic!("{changes:?}")
        };
        assert_eq!((start[1], end[1]), (200.0, 200.0));
        let rings: Vec<_> = events(&out, EventCode::RingUpdate)
            .iter()
            .map(|o| RingUpdate::decode(&o.packet.payload).unwrap())
            .collect();
        assert!(
            matches!(
                rings[..],
                [
                    RingUpdate::NextRing { ring_id: 1, .. },
                    RingUpdate::StartedMoving
                ]
            ),
            "{rings:?}"
        );
        assert_eq!(room.state(), game_state::STARTED);

        // Too late now.
        let refused = join(&mut room, 3, "too late");
        assert!(
            matches!(RoomInitResponse::decode(&refused[0].packet.payload).unwrap(), RoomInitResponse::Refused { response, .. } if response & server_response::MATCH_ALREADY_STARTED != 0)
        );
    }

    #[test]
    fn players_jump_or_are_dropped_when_the_plane_ends() {
        let mut room = match_room();
        run(&mut room, 0.0, 10.5);
        // Player 0 jumps; player 1 stays aboard.
        let req = RequestAirplaneDrop {
            player_index: 0,
            direction: [0.0, -1.0, 0.0],
        };
        let out = room.handle(
            0,
            &Packet::new(EventCode::RequestAirplaneDrop, req.encode()),
        );
        let dropped = PlayerAirplaneDropped::decode(&out[0].packet.payload).unwrap();
        assert_eq!(
            (out[0].to.clone(), dropped.player_index, dropped.position[1]),
            (Recipients::All, 0, 205.0)
        );
        assert!(
            room.handle(
                0,
                &Packet::new(EventCode::RequestAirplaneDrop, req.encode())
            )
            .is_empty(),
            "only once"
        );
        // A spoofed request for someone else is ignored.
        let spoof = RequestAirplaneDrop {
            player_index: 1,
            ..req
        };
        assert!(
            room.handle(
                0,
                &Packet::new(EventCode::RequestAirplaneDrop, spoof.encode())
            )
            .is_empty()
        );

        let out = run(&mut room, 10.6, 10.5 + plane::FLIGHT_TIME + 0.5);
        assert!(
            !events(&out, EventCode::PlaneUpdate).is_empty(),
            "plane progress is synced"
        );
        let all_drop = events(&out, EventCode::AllDrop);
        assert_eq!(all_drop.len(), 1);
        assert_eq!(all_drop[0].to, Recipients::One(1));
    }

    #[test]
    fn players_in_the_plane_are_not_replicated() {
        let mut room = match_room();
        run(&mut room, 0.0, 10.5);
        let up = ClientPlayerUpdate {
            player_index: 0,
            position: [1.0; 3],
            rotation: [0.0; 2],
            aiming_down_sights: false,
            direction: PackedDirection::ZERO,
            movement: 0,
        };
        room.handle(0, &Packet::new(EventCode::PlayerUpdate, up.encode()));
        assert!(events(&room.tick(11.0), EventCode::PlayerUpdate).is_empty());
        let req = RequestAirplaneDrop {
            player_index: 0,
            direction: [0.0, -1.0, 0.0],
        };
        let out = room.handle(
            0,
            &Packet::new(EventCode::RequestAirplaneDrop, req.encode()),
        );
        let at = PlayerAirplaneDropped::decode(&out[0].packet.payload)
            .unwrap()
            .position;
        // Player 1 is still in the plane: it sees nobody.
        let near = ClientPlayerUpdate { position: at, ..up };
        room.handle(0, &Packet::new(EventCode::PlayerUpdate, near.encode()));
        assert!(events(&room.tick(11.2), EventCode::PlayerUpdate).is_empty());
        // Once it jumped too (same spot), it sees player 0 move.
        let req1 = RequestAirplaneDrop {
            player_index: 1,
            ..req
        };
        room.handle(
            1,
            &Packet::new(EventCode::RequestAirplaneDrop, req1.encode()),
        );
        room.handle(0, &Packet::new(EventCode::PlayerUpdate, near.encode()));
        assert_eq!(events(&room.tick(11.4), EventCode::PlayerUpdate).len(), 1);
    }

    /// A match with loot: one spot at the drop point and one far away.
    fn loot_room() -> (Room, [f32; 3]) {
        let mut room = match_room();
        run(&mut room, 0.0, 10.5);
        let plane = room.plane.clone().unwrap();
        let here = plane.drop_position(10.5);
        let modifier = crate::loot::Modifier {
            index: 4,
            rarity: 0,
            drops: vec![crate::loot::Drop {
                weight: 1.0,
                items: vec![(6, 30)],
            }],
        };
        room.settings.loot = crate::loot::LootRules {
            spots: vec![
                crate::loot::Spot {
                    position: [here[0], 0.0, here[2]],
                    force: true,
                },
                crate::loot::Spot {
                    position: [here[0] + 2000.0, 0.0, here[2]],
                    force: true,
                },
            ],
            modifiers: vec![modifier],
        };
        let (_, loot) = LootWorld::spawn(&room.settings.loot, 1.0, &mut room.rng);
        room.loot = loot;
        (room, here)
    }

    #[test]
    fn jumping_sends_the_loot_around_you() {
        let (mut room, _) = loot_room();
        let req = RequestAirplaneDrop {
            player_index: 0,
            direction: [0.0, -1.0, 0.0],
        };
        let out = room.handle(
            0,
            &Packet::new(EventCode::RequestAirplaneDrop, req.encode()),
        );
        let entry = events(&out, EventCode::ChunkEntry);
        assert_eq!(entry[0].to, Recipients::One(0));
        let entry = ChunkEntry::decode(&entry[0].packet.payload).unwrap();
        assert_eq!(entry.loot.len(), 1, "only the nearby spot: {entry:?}");
        assert_eq!((entry.loot[0].item_id, entry.loot[0].quantity), (6, 30));
    }

    #[test]
    fn walking_into_a_new_chunk_sends_entry_and_exit() {
        let (mut room, here) = loot_room();
        let req = RequestAirplaneDrop {
            player_index: 0,
            direction: [0.0, -1.0, 0.0],
        };
        room.handle(
            0,
            &Packet::new(EventCode::RequestAirplaneDrop, req.encode()),
        );
        let far = [here[0] + 2000.0, 0.0, here[2]];
        let up = ClientPlayerUpdate {
            player_index: 0,
            position: far,
            rotation: [0.0; 2],
            aiming_down_sights: false,
            direction: PackedDirection::ZERO,
            movement: 0,
        };
        let out = room.handle(0, &Packet::new(EventCode::PlayerUpdate, up.encode()));
        let entry =
            ChunkEntry::decode(&events(&out, EventCode::ChunkEntry)[0].packet.payload).unwrap();
        let exit =
            ChunkExit::decode(&events(&out, EventCode::ChunkExit)[0].packet.payload).unwrap();
        assert_eq!(entry.loot.len(), 1, "the far spot came into view");
        assert_eq!(exit.loot.len(), 1, "the near spot went out of view");
        // Moving within the same chunk sends nothing.
        assert!(
            room.handle(0, &Packet::new(EventCode::PlayerUpdate, up.encode()))
                .is_empty()
        );
    }

    #[test]
    fn pickup_removes_loot_once_and_drop_adds_it() {
        let (mut room, here) = loot_room();
        let req = RequestAirplaneDrop {
            player_index: 0,
            direction: [0.0, -1.0, 0.0],
        };
        let out = room.handle(
            0,
            &Packet::new(EventCode::RequestAirplaneDrop, req.encode()),
        );
        let item = ChunkEntry::decode(&events(&out, EventCode::ChunkEntry)[0].packet.payload)
            .unwrap()
            .loot[0]
            .clone();
        let take = PickupRequest {
            player_index: 0,
            network_id: item.network_id,
            slot: 1,
        };
        let out = room.handle(
            0,
            &Packet::new(EventCode::RequestWeaponPickUp, take.encode()),
        );
        assert_eq!(out[0].packet.event(), Some(EventCode::WeaponPickUpAccepted));
        assert!(
            room.handle(
                0,
                &Packet::new(EventCode::RequestWeaponPickUp, take.encode())
            )
            .is_empty(),
            "gone"
        );
        // Player 1 is still in the plane and can't pick anything up.
        let drop = DropRequest {
            player_index: 0,
            item_id: 6,
            quantity: 10,
            position: here,
        };
        let out = room.handle(0, &Packet::new(EventCode::RequestItemDrop, drop.encode()));
        assert_eq!(out[0].packet.event(), Some(EventCode::ItemDrop));
        assert_eq!(room.loot().len(), 2);
    }

    #[test]
    fn last_team_standing_wins_and_the_room_restarts() {
        let mut room = match_room();
        run(&mut room, 0.0, 10.5);
        let out = room.disconnect(1);
        assert_eq!(events(&out, EventCode::PlayerLeft).len(), 1);
        let out = run(&mut room, 10.6, 10.8);
        let ended: Vec<_> = events(&out, EventCode::GameStateChanged)
            .iter()
            .map(|o| GameStateChanged::decode(&o.packet.payload).unwrap())
            .collect();
        assert_eq!(ended, [GameStateChanged::Ended { winner: 0 }]);
        assert_eq!(room.state(), game_state::ENDED);

        run(&mut room, 10.9, 10.8 + RESTART_AFTER_END + 0.2);
        assert_eq!(room.take_disconnects(), [0]);
        assert_eq!(room.state(), game_state::WAITING_FOR_PLAYERS);
        assert_eq!(room.player_count(), 0);
    }

    #[test]
    fn timed_force_start_needs_its_minimum() {
        let settings = RoomSettings {
            players_to_start: 10,
            force_start: Some(crate::ForceStart {
                after_seconds: 30.0,
                min_players: 2,
            }),
            ..Default::default()
        };
        let mut room = Room::new(settings);
        join(&mut room, 0, "a");
        run(&mut room, 0.0, 40.0);
        assert_eq!(
            room.state(),
            game_state::WAITING_FOR_PLAYERS,
            "one player is not enough"
        );
        join(&mut room, 1, "b");
        run(&mut room, 40.1, 40.2);
        assert_eq!(room.state(), game_state::COUNT_DOWN);
    }

    /// A started match where every player in `peers` has jumped. Squads via `squad`.
    fn fight_room(players: &[(u8, u64)]) -> Room {
        fight_room_with(RoomSettings::default(), players)
    }

    fn fight_room_with(settings: RoomSettings, players: &[(u8, u64)]) -> Room {
        let mut room = Room::new(RoomSettings {
            countdown: 1.0,
            players_to_start: players.len() as u8,
            ..settings
        });
        for &(peer, squad) in players {
            room.connect(peer);
            let req = RoomInitRequest {
                player_name: format!("p{peer}"),
                squad_id: squad,
                ..Default::default()
            };
            room.handle(peer, &Packet::new(EventCode::RoomInit, req.encode()));
            enter(&mut room, peer);
        }
        run(&mut room, 0.0, 1.5);
        assert_eq!(room.state(), game_state::STARTED);
        for &(peer, _) in players {
            let req = RequestAirplaneDrop {
                player_index: peer,
                direction: [0.0, -1.0, 0.0],
            };
            room.handle(
                peer,
                &Packet::new(EventCode::RequestAirplaneDrop, req.encode()),
            );
        }
        room
    }

    fn hit(room: &mut Room, attacker: u8, victim: u8, new_health: f32) -> Vec<Outgoing> {
        let ev = DamageEvent {
            attacker,
            victim,
            new_health,
            direction: [0.0, 0.0, 1.0],
            hit: None,
            exclude_victim: false,
        };
        room.handle(attacker, &Packet::new(EventCode::DamageEvent, ev.encode()))
    }

    #[test]
    fn damage_needs_a_started_match() {
        let mut room = Room::new(lobby());
        join(&mut room, 0, "a");
        join(&mut room, 1, "b");
        assert!(hit(&mut room, 0, 1, 10.0).is_empty());
    }

    #[test]
    fn damage_is_applied_and_broadcast() {
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        let out = hit(&mut room, 0, 1, 60.0);
        let d = PlayerDamaged::decode(&out[0].packet.payload).unwrap();
        assert_eq!(
            (d.victim, d.attacker, d.health, d.downed, d.damaged),
            (1, 0, 60.0, false, true)
        );
        // Only the attacker or victim may report it.
        let ev = DamageEvent {
            attacker: 0,
            victim: 1,
            new_health: 0.0,
            direction: [0.0; 3],
            hit: None,
            exclude_victim: false,
        };
        assert!(
            room.handle(2, &Packet::new(EventCode::DamageEvent, ev.encode()))
                .is_empty()
        );
    }

    #[test]
    fn solo_player_at_zero_is_downed_team_and_dies() {
        // Solo team: going down means the whole team is down, so the player dies right away.
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        let out = hit(&mut room, 0, 1, 0.0);
        let dead =
            PlayerDead::decode(&events(&out, EventCode::PlayerDead)[0].packet.payload).unwrap();
        assert_eq!(
            (dead.victim, dead.last_attacker, dead.spectate),
            (1, 0, NOBODY)
        );
        let team = TeamDead::decode(&events(&out, EventCode::TeamDead)[0].packet.payload).unwrap();
        assert_eq!((team.group, team.placement), (1, 2));
        let ended = run(&mut room, 1.6, 1.7);
        let end = GameStateChanged::decode(
            &events(&ended, EventCode::GameStateChanged)[0]
                .packet
                .payload,
        )
        .unwrap();
        assert_eq!(end, GameStateChanged::Ended { winner: 0 });
    }

    #[test]
    fn squad_member_goes_down_takes_reduced_damage_and_can_be_revived() {
        // 0 and 1 are a squad, 2 is alone.
        let mut room = fight_room(&[(0, 7), (1, 7), (2, 9)]);
        let out = hit(&mut room, 2, 0, -5.0);
        assert!(
            events(&out, EventCode::PlayerDead).is_empty(),
            "downed, not dead"
        );
        // Within a second of going down, hits are ignored.
        assert!(hit(&mut room, 2, 0, 0.0).is_empty());
        run(&mut room, 1.6, 3.0);
        // Downed damage is capped at 50 and scaled by 0.12: 100 -> 94.
        let d = PlayerDamaged::decode(&hit(&mut room, 2, 0, 0.0)[0].packet.payload).unwrap();
        assert!(d.downed && (d.health - 94.0).abs() < 1e-3, "{d:?}");

        // The teammate revives them to 15 health.
        let req = ReviveRequest {
            stage: ReviveStage::Finished,
            reviver: 1,
            revived: 0,
        };
        let out = room.handle(1, &Packet::new(EventCode::ReviveState, req.encode()));
        assert_eq!(
            out[0].packet.payload[..3],
            [ReviveStage::Finished as u8, 0, 1]
        );
        let d = PlayerDamaged::decode(&hit(&mut room, 2, 0, 10.0)[0].packet.payload).unwrap();
        assert_eq!((d.downed, d.health), (false, 10.0));
        // An enemy can't "revive".
        let fake = ReviveRequest {
            stage: ReviveStage::Finished,
            reviver: 2,
            revived: 0,
        };
        assert!(
            room.handle(2, &Packet::new(EventCode::ReviveState, fake.encode()))
                .is_empty()
        );
    }

    #[test]
    fn whole_squad_down_means_everyone_dies() {
        let mut room = fight_room(&[(0, 7), (1, 7), (2, 9)]);
        hit(&mut room, 2, 0, 0.0);
        let out = hit(&mut room, 2, 1, 0.0);
        assert_eq!(events(&out, EventCode::PlayerDead).len(), 2);
        assert_eq!(events(&out, EventCode::TeamDead).len(), 1);
    }

    #[test]
    fn ring_and_fall_deaths_are_self_reported_only() {
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        assert!(
            room.handle(0, &Packet::new(EventCode::RingDeath, vec![1]))
                .is_empty(),
            "can't report others"
        );
        let out = room.handle(1, &Packet::new(EventCode::RingDeath, vec![1]));
        assert!(
            PlayerDead::decode(&events(&out, EventCode::PlayerDead)[0].packet.payload)
                .unwrap()
                .ring_death
        );
    }

    #[test]
    fn fire_is_relayed_to_others_and_synced_back() {
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        let mut fire = vec![0, tabg_protocol::combat::firing_mode::WANTS_TO_BE_SYNCED];
        fire.extend(5i32.to_le_bytes());
        fire.extend(42i32.to_le_bytes());
        let out = room.handle(0, &Packet::new(EventCode::PlayerFire, fire.clone()));
        assert_eq!(out.len(), 2);
        assert_eq!(
            (out[0].to.clone(), out[0].packet.event()),
            (Recipients::One(0), Some(EventCode::PlayerFireSyncReturn))
        );
        assert_eq!(
            (out[1].to.clone(), out[1].packet.payload.clone()),
            (Recipients::AllExcept(0), fire.clone())
        );
        // Someone else's fire packet from player 1 is dropped.
        assert!(
            room.handle(1, &Packet::new(EventCode::PlayerFire, fire))
                .is_empty()
        );
    }

    /// A started match with one car next to where everyone jumped, and two players on foot there.
    fn car_room() -> (Room, i32) {
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        let here = room.players[&0].position;
        let spawn = crate::cars::CarSpawn {
            car_type: 3,
            car_index: 7,
            position: here,
            rotation: [0.0, 0.0, 0.0, 1.0],
            seats: vec![(0, true), (1, false)],
            parts: vec![tabg_protocol::messages::CarPart {
                index: 0,
                health: 500.0,
                name: "Chassi".into(),
            }],
        };
        room.cars.insert(7, Car::spawn(&spawn));
        (room, 7)
    }

    fn seat(room: &mut Room, peer: u8, car: i32, seat: i32, action: u8) -> Vec<Outgoing> {
        let req = SeatRequest {
            player_index: peer,
            car_index: car,
            seat_index: seat,
            action,
        };
        room.handle(peer, &Packet::new(EventCode::RequestSeat, req.encode()))
    }

    #[test]
    fn seats_are_exclusive_and_announced() {
        let (mut room, car) = car_room();
        let out = seat(&mut room, 0, car, 0, SEAT_GET_IN);
        assert_eq!(out[0].packet.event(), Some(EventCode::SeatAccepted));
        assert_eq!(
            out[0].packet.payload.len(),
            10 + 12 + 1,
            "getting in includes the car pose"
        );
        assert!(
            seat(&mut room, 1, car, 0, SEAT_GET_IN).is_empty(),
            "driver seat taken"
        );
        assert!(
            !seat(&mut room, 1, car, 1, SEAT_GET_IN).is_empty(),
            "passenger seat is free"
        );
        assert_eq!(room.cars[&car].seats[1].occupant, Some(1));
        let out = seat(&mut room, 0, car, 0, SEAT_GET_OUT);
        assert_eq!(out[0].packet.payload.len(), 10);
        assert_eq!(room.cars[&car].temporary_owner.map(|(o, _)| o), Some(0));
    }

    #[test]
    fn driving_moves_the_car_and_replicates_the_driver() {
        let (mut room, car) = car_room();
        seat(&mut room, 0, car, 0, SEAT_GET_IN);
        let mut pos = room.cars[&car].position;
        pos[0] += 3.0;
        let up = CarUpdate {
            player_index: 0,
            position: pos,
            rotation: PackedQuaternion::pack([0.0, 0.0, 0.0, 1.0]),
            input: PackedDirection::ZERO,
            driving_state: 1,
        };
        room.handle(0, &Packet::new(EventCode::CarUpdate, up.encode()));
        assert_eq!(room.cars[&car].position, pos);
        let out = room.tick(5.0);
        let batch =
            PlayerUpdateBatch::decode(&events(&out, EventCode::PlayerUpdate)[0].packet.payload)
                .unwrap();
        assert!(
            matches!(&batch.entities[0], EntityUpdate { player_index: 0, body: EntityBody::Driving { car_position: Some(p), .. }, .. } if *p == pos)
        );
        // A passenger can't drive.
        seat(&mut room, 1, car, 1, SEAT_GET_IN);
        let fake = CarUpdate {
            player_index: 1,
            ..up
        };
        room.handle(1, &Packet::new(EventCode::CarUpdate, fake.encode()));
        assert_eq!(room.cars[&car].position, pos);
    }

    #[test]
    fn only_the_temporary_owner_can_push_and_only_briefly() {
        let (mut room, car) = car_room();
        seat(&mut room, 0, car, 0, SEAT_GET_IN);
        seat(&mut room, 0, car, 0, SEAT_GET_OUT);
        let push = |room: &mut Room, peer: u8, dx: f32| {
            let mut p = room.cars[&car].position;
            p[0] += dx;
            let up = CarTemporaryUpdate {
                player_index: peer,
                position: p,
                rotation: PackedQuaternion::pack([0.0, 0.0, 0.0, 1.0]),
                car_index: car,
            };
            room.handle(
                peer,
                &Packet::new(EventCode::CarTemporaryUpdate, up.encode()),
            );
            room.cars[&car].position
        };
        let before = room.cars[&car].position;
        assert_eq!(push(&mut room, 1, 1.0), before, "not the owner");
        assert_ne!(push(&mut room, 0, 1.0), before);
        let out = room.tick(4.0);
        let batch =
            PlayerUpdateBatch::decode(&events(&out, EventCode::PlayerUpdate)[0].packet.payload)
                .unwrap();
        assert_eq!(batch.cars[0].car_index, car);
        room.tick(10.0);
        let at = room.cars[&car].position;
        assert_eq!(push(&mut room, 0, 1.0), at, "ownership expired");
    }

    #[test]
    fn car_damage_and_downed_drivers_are_ejected() {
        let (mut room, car) = car_room();
        let d = CarDamageReport {
            part: 0,
            car_index: car,
            new_health: 300.0,
        };
        let out = room.handle(1, &Packet::new(EventCode::CarDamage, d.encode(1)));
        assert_eq!(out[0].packet.payload, encode_car_damage(0, car, 300.0));
        seat(&mut room, 0, car, 0, SEAT_GET_IN);
        // Player 0 is alone in team 1: going down kills them, and they leave the car.
        hit(&mut room, 1, 0, 0.0);
        assert_eq!(room.cars[&car].seats[0].occupant, None);
    }

    #[test]
    fn cars_appear_in_chunk_entries_once_the_match_runs() {
        let (mut room, car) = car_room();
        let out = room.force_chunk_entry(0);
        let raw = &events(&out, EventCode::ChunkEntry)[0].packet.payload;
        assert_eq!(u16::from_le_bytes([raw[4], raw[5]]), 1, "one car in view");
        assert_eq!(
            room.cars_in(&[chunk_of(room.cars[&car].position)], 0).len(),
            1
        );
    }

    fn chat(room: &mut Room, peer: u8, text: &str) -> Vec<Outgoing> {
        let msg = tabg_protocol::actions::ChatMessage {
            player_index: peer,
            text: text.into(),
        };
        room.handle(peer, &Packet::new(EventCode::ChatMessage, msg.encode()))
    }

    #[test]
    fn healing_items_and_health_states() {
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        room.settings.heal_amounts.insert(131, 15.0);
        hit(&mut room, 1, 0, 50.0);
        let req = tabg_protocol::actions::HealRequest {
            player_index: 0,
            item_id: 131,
        };
        let out = room.handle(0, &Packet::new(EventCode::RequestHealing, req.encode()));
        assert_eq!(
            out[0].packet.payload,
            tabg_protocol::actions::encode_player_health(0, 65.0)
        );
        let unknown = tabg_protocol::actions::HealRequest {
            item_id: 999,
            ..req
        };
        assert!(
            room.handle(0, &Packet::new(EventCode::RequestHealing, unknown.encode()))
                .is_empty()
        );
        let mut lower = vec![0];
        lower.extend(10.0f32.to_le_bytes());
        assert!(
            room.handle(0, &Packet::new(EventCode::RequestHealthState, lower))
                .is_empty(),
            "only increases"
        );
        let mut higher = vec![0];
        higher.extend(90.0f32.to_le_bytes());
        assert_eq!(
            room.handle(0, &Packet::new(EventCode::RequestHealthState, higher))
                .len(),
            1
        );
    }

    #[test]
    fn inventory_check_logs_or_refuses_what_a_player_never_got() {
        use crate::settings::InventoryCheck;
        let drop = |item_id, quantity| {
            let d = DropRequest {
                player_index: 0,
                item_id,
                quantity,
                position: [0.0; 3],
            };
            Packet::new(EventCode::RequestItemDrop, d.encode())
        };
        let reported = |room: &mut Room| {
            room.take_events()
                .into_iter()
                .filter_map(|e| match e {
                    RoomEvent::Inventory {
                        item,
                        wanted,
                        carried,
                        refused,
                        ..
                    } => Some((item, wanted, carried, refused)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        // Log (the default): allowed, reported once per item.
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        assert_eq!(room.settings.inventory_check, InventoryCheck::Log);
        room.take_events();
        assert_eq!(room.handle(0, &drop(132, 3)).len(), 1);
        assert_eq!(room.handle(0, &drop(132, 3)).len(), 1);
        assert_eq!(reported(&mut room), vec![(132, 3, 0, false)]);

        // Enforce: refused without the item, clamped to what is carried.
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        room.settings.inventory_check = InventoryCheck::Enforce;
        room.take_events();
        assert!(room.handle(0, &drop(132, 3)).is_empty());
        assert_eq!(reported(&mut room), vec![(132, 3, 0, true)]);
        room.inventory_add(0, 131, 2);
        let out = room.handle(0, &drop(131, 5));
        assert_eq!(events(&out, EventCode::ItemDrop).len(), 1);
        let all = room.loot().in_chunks(&[(0, 0), (-1, -1), (-1, 0), (0, -1)]);
        assert_eq!(
            all.iter()
                .filter(|l| l.item_id == 131)
                .map(|l| l.quantity)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(room.carried(0, 131), 0);
        // Ammo is never checked: guns come loaded, so its count drifts.
        room.settings.ammo.insert(6);
        assert_eq!(room.handle(0, &drop(6, 30)).len(), 1);
        // Healing needs the item too.
        room.settings.heal_amounts.insert(133, 15.0);
        hit(&mut room, 1, 0, 50.0);
        let heal = tabg_protocol::actions::HealRequest {
            player_index: 0,
            item_id: 133,
        };
        assert!(
            room.handle(0, &Packet::new(EventCode::RequestHealing, heal.encode()))
                .is_empty()
        );
        room.inventory_add(0, 133, 1);
        assert_eq!(
            room.handle(0, &Packet::new(EventCode::RequestHealing, heal.encode()))
                .len(),
            1
        );
    }

    #[test]
    fn inventory_check_covers_equipped_weapons() {
        use crate::settings::InventoryCheck;
        let change = |weapon: i16| {
            let mut w = tabg_protocol::Writer::new();
            w.u8(0).u8(1);
            for v in [weapon, -1, -1, -1, -1] {
                w.u16(v as u16);
            }
            w.u8(0);
            Packet::new(EventCode::WeaponChange, w.into_inner())
        };
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        room.settings.inventory_check = InventoryCheck::Enforce;
        assert!(room.handle(0, &change(160)).is_empty(), "never picked up");
        room.inventory_add(0, 160, 1);
        assert_eq!(
            events(&room.handle(0, &change(160)), EventCode::WeaponChanged).len(),
            1
        );
        assert_eq!(
            events(&room.handle(0, &change(-1)), EventCode::WeaponChanged).len(),
            1,
            "empty hands are always fine"
        );
    }

    #[test]
    fn thrown_items_become_loot_but_grenades_do_not() {
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        room.settings.grenades.insert(50);
        let before = room.loot().len();
        let throw = |room: &mut Room, item| {
            let t = tabg_protocol::actions::ThrowRequest {
                player_index: 0,
                item_id: item,
                quantity: 1,
                position: [1.0; 3],
                direction: [0.0, 1.0, 0.0],
            };
            room.handle(0, &Packet::new(EventCode::RequestItemThrow, t.encode()))
        };
        assert_eq!(
            throw(&mut room, 50)[0].packet.event(),
            Some(EventCode::ItemThrown)
        );
        assert_eq!(room.loot().len(), before);
        throw(&mut room, 6);
        assert_eq!(room.loot().len(), before + 1);
    }

    #[test]
    fn chat_is_local_and_admins_get_commands() {
        let mut room = Room::new(RoomSettings {
            admins: vec!["boss".into()],
            ..lobby()
        });
        join(&mut room, 0, "boss");
        enter(&mut room, 0);
        join(&mut room, 1, "guest");
        enter(&mut room, 1);
        // Plain chat is relayed; a guest's "/start" is just text.
        assert_eq!(
            chat(&mut room, 1, "/start")[0].packet.event(),
            Some(EventCode::ChatMessage)
        );
        assert_eq!(room.state(), game_state::WAITING_FOR_PLAYERS);
        // The admin's runs.
        let out = chat(&mut room, 0, "/start");
        assert!(events(&out, EventCode::GameStateChanged).len() == 1);
        assert_eq!(room.state(), game_state::COUNT_DOWN);
        let out = chat(&mut room, 0, "/give 6 30");
        assert_eq!(events(&out, EventCode::ItemDrop).len(), 1);
        assert!(
            chat(&mut room, 0, "/kick guest")
                .iter()
                .any(|o| o.to == Recipients::One(0))
        );
        assert_eq!(room.take_disconnects(), [1]);
    }

    #[test]
    fn admin_start_game_command_needs_an_admin() {
        let mut room = Room::new(RoomSettings {
            admins: vec!["boss".into()],
            ..lobby()
        });
        join(&mut room, 0, "boss");
        join(&mut room, 1, "guest");
        let mut w = tabg_protocol::Writer::new();
        w.string("Start Game");
        let cmd = Packet::new(EventCode::AdminCommand, w.into_inner());
        assert!(room.handle(1, &cmd).is_empty());
        assert_eq!(room.handle(0, &cmd).len(), 1);
        assert_eq!(room.state(), game_state::COUNT_DOWN);
    }

    #[test]
    fn parrot_is_rate_limited_and_spectating_needs_a_dead_player() {
        let mut room = fight_room(&[(0, 1), (1, 2)]);
        let say = Packet::new(EventCode::ThrowChatMessage, vec![0, 2, b'h', 0]);
        assert_eq!(room.handle(0, &say).len(), 1);
        assert!(room.handle(0, &say).is_empty(), "too soon");
        assert!(
            room.handle(0, &Packet::new(EventCode::SpectatorRequest, vec![1]))
                .is_empty(),
            "alive"
        );
        room.handle(0, &Packet::new(EventCode::PlayerDead, vec![0]));
        let out = room.handle(0, &Packet::new(EventCode::SpectatorRequest, vec![1]));
        assert_eq!(
            (out[0].to.clone(), out[0].packet.payload.clone()),
            (Recipients::One(0), vec![1])
        );
    }

    #[test]
    fn events_report_joins_chat_and_leaves_with_ids() {
        let mut room = Room::new(lobby());
        room.connect(0);
        let req = RoomInitRequest {
            player_name: "alice".into(),
            squad_size: 1,
            epic_product_user_id: b"0002abc\0".to_vec(),
            ..Default::default()
        };
        room.handle(0, &Packet::new(EventCode::RoomInit, req.encode()));
        chat(&mut room, 0, "hi");
        room.disconnect(0);
        assert_eq!(
            room.take_events(),
            vec![
                RoomEvent::Joined {
                    index: 0,
                    name: "alice".into(),
                    id: "epic:0002abc".into()
                },
                RoomEvent::Chat {
                    index: 0,
                    name: "alice".into(),
                    text: "hi".into()
                },
                RoomEvent::Left {
                    index: 0,
                    name: "alice".into()
                },
            ]
        );
        assert!(room.take_events().is_empty());
    }

    #[test]
    fn console_bans_and_banned_players_are_refused() {
        let mut room = Room::new(lobby());
        join(&mut room, 0, "griefer");
        room.take_events();
        let (_, reply) = room.console("ban griefer");
        assert_eq!(reply, "banned griefer");
        assert_eq!(room.take_disconnects(), vec![0]);
        assert_eq!(
            room.take_events(),
            vec![RoomEvent::Banned {
                name: "griefer".into(),
                id: "name:griefer".into()
            }]
        );
        room.disconnect(0);
        let out = join(&mut room, 0, "griefer");
        assert!(matches!(
            RoomInitResponse::decode(&out[0].packet.payload).unwrap(),
            RoomInitResponse::Refused { .. }
        ));
        // Banning by name works too, case-insensitively.
        room.apply_settings(RoomSettings {
            banned: vec!["SOMEONE".into()],
            ..lobby()
        });
        assert!(matches!(
            RoomInitResponse::decode(&join(&mut room, 1, "someone")[0].packet.payload).unwrap(),
            RoomInitResponse::Refused { .. }
        ));
    }

    #[test]
    fn console_commands_reply_and_find_players() {
        let mut room = Room::new(lobby());
        join(&mut room, 0, "alice");
        enter(&mut room, 0);
        assert!(room.console("players").1.contains("#0 alice"));
        let (out, reply) = room.console("give alice 6 30");
        assert_eq!(reply, "gave 30 × item 6 to alice");
        assert_eq!(out[0].packet.event(), Some(EventCode::ItemDrop));
        let (out, _) = room.console("say hello there");
        assert_eq!(out.len(), 1);
        let said = tabg_protocol::actions::ChatMessage::decode(&out[0].packet.payload).unwrap();
        assert_eq!(said.text, "[tabg] hello there");
        assert!(room.console("kick #7").1.contains("no player"));
        assert!(room.console("frobnicate").1.contains("unknown command"));
        assert_eq!(room.console("/start").1, "starting");
    }

    #[test]
    fn apply_settings_updates_admins_of_present_players() {
        let mut room = Room::new(lobby());
        join(&mut room, 0, "alice");
        assert!(!room.players()[0].admin);
        room.apply_settings(RoomSettings {
            admins: vec!["Alice".into()],
            ..lobby()
        });
        assert!(room.players()[0].admin);
    }

    #[test]
    fn loadout_is_given_on_landing() {
        let mut room = Room::new(RoomSettings {
            countdown: 10.0,
            loadout: vec![(6, 120), (131, 2)],
            ..Default::default()
        });
        for (peer, name) in [(0, "a"), (1, "b")] {
            join(&mut room, peer, name);
            enter(&mut room, peer);
        }
        run(&mut room, 0.0, 10.5);
        let req = RequestAirplaneDrop {
            player_index: 0,
            direction: [0.0, -1.0, 0.0],
        };
        room.handle(
            0,
            &Packet::new(EventCode::RequestAirplaneDrop, req.encode()),
        );
        let out = room.handle(0, &Packet::new(EventCode::PlayerLand, vec![0]));
        let loot: Vec<_> = events(&out, EventCode::PlayerLootRecieved);
        assert_eq!(loot.len(), 1);
        assert_eq!(loot[0].to, Recipients::One(0));
        assert_eq!(
            loot[0].packet.payload,
            tabg_protocol::actions::encode_player_loot(&[(6, 120), (131, 2)])
        );
    }

    fn respawn_settings() -> RoomSettings {
        RoomSettings {
            respawn: Some(crate::settings::Respawn { delay: 3.0 }),
            kills_to_win: 2,
            loot: crate::loot::LootRules {
                spots: vec![crate::loot::Spot {
                    position: [10.0, 50.0, 10.0],
                    force: false,
                }],
                modifiers: Vec::new(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn with_respawns_the_dead_come_back_and_kills_decide() {
        let mut room = fight_room_with(respawn_settings(), &[(0, 1), (1, 2)]);
        let out = hit(&mut room, 0, 1, 0.0);
        assert!(events(&out, EventCode::PlayerDead).len() == 1);
        assert!(
            events(&out, EventCode::TeamDead).is_empty(),
            "no elimination with respawns"
        );
        assert_eq!(room.state(), game_state::STARTED);
        // Back after the delay, on the ground at a loot spot.
        let out = run(&mut room, 1.5, 5.0);
        let respawn = events(&out, EventCode::PlayerRespawn);
        assert_eq!(respawn.len(), 1);
        let payload = &respawn[0].packet.payload;
        assert_eq!(payload.len(), 1 + 23);
        assert_eq!(&payload[..3], &[1, 1, 1]);
        let y = f32::from_le_bytes(payload[11..15].try_into().unwrap());
        assert_eq!(y, 51.0);
        assert_eq!(*payload.last().unwrap(), u8::MAX, "no curse");
        assert!(
            events(&out, EventCode::ChunkEntry)
                .iter()
                .any(|o| o.to == Recipients::One(1))
        );
        assert!(room.players()[1].alive);
        // Second kill reaches kills_to_win = 2.
        hit(&mut room, 0, 1, 0.0);
        let out = run(&mut room, 5.0, 5.5);
        let ended = events(&out, EventCode::GameStateChanged);
        assert_eq!(room.state(), game_state::ENDED);
        assert_eq!(
            GameStateChanged::decode(&ended[0].packet.payload).unwrap(),
            GameStateChanged::Ended {
                winner: room.players()[0].team
            }
        );
    }

    #[test]
    fn gun_game_levels_up_the_killer_and_the_last_level_wins() {
        let settings = RoomSettings {
            gun_game: vec![vec![(160, 1)], vec![(152, 1)]],
            kills_to_win: 0,
            ..respawn_settings()
        };
        let mut room = fight_room_with(settings, &[(0, 1), (1, 2)]);
        // Landing hands out level 0.
        let out = room.handle(0, &Packet::new(EventCode::PlayerLand, vec![0]));
        let loot = events(&out, EventCode::PlayerLootRecieved);
        assert_eq!(loot[0].packet.payload, encode_player_loot(&[(160, 1)]));
        // A kill gives the next weapon right away.
        let out = hit(&mut room, 0, 1, 0.0);
        let loot = events(&out, EventCode::PlayerLootRecieved);
        assert_eq!(loot.len(), 1);
        assert_eq!(loot[0].to, Recipients::One(0));
        assert_eq!(loot[0].packet.payload, encode_player_loot(&[(152, 1)]));
        run(&mut room, 1.5, 5.0);
        hit(&mut room, 0, 1, 0.0);
        run(&mut room, 5.0, 5.5);
        assert_eq!(room.state(), game_state::ENDED, "past the last level");
    }

    #[test]
    fn time_limit_ends_respawn_matches() {
        let settings = RoomSettings {
            kills_to_win: 0,
            time_limit: 10.0,
            ..respawn_settings()
        };
        let mut room = fight_room_with(settings, &[(0, 1), (1, 2)]);
        hit(&mut room, 1, 0, 0.0);
        run(&mut room, 1.5, 9.0);
        assert_eq!(room.state(), game_state::STARTED);
        run(&mut room, 9.0, 12.0);
        assert_eq!(room.state(), game_state::ENDED);
    }

    fn voice_frame(sender: u8) -> Packet {
        let mut payload = vec![1, sender, 7, 0, 0x80, 0x3e, 1, 1, 0x40, 0x01];
        payload.extend(std::iter::repeat_n(128u8, 320));
        Packet { code: 240, payload }
    }

    #[test]
    fn voice_reaches_players_in_range_with_the_real_sender() {
        let mut room = Room::new(lobby());
        for (peer, name) in [(0, "a"), (1, "b"), (2, "c")] {
            join(&mut room, peer, name);
            enter(&mut room, peer);
        }
        room.players.get_mut(&2).unwrap().position = [500.0, 0.0, 0.0];
        room.players.get_mut(&0).unwrap().position = [0.0; 3];
        room.players.get_mut(&1).unwrap().position = [10.0, 0.0, 0.0];
        // A spoofed sender byte is replaced.
        let out = room.handle(0, &voice_frame(9));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, Recipients::Many(vec![1]), "c is too far away");
        assert!(!out[0].reliable);
        assert_eq!(out[0].packet.payload[1], 0);
        // Rate limit: at most 60 frames a second.
        let sent = (0..100)
            .filter(|_| !room.handle(0, &voice_frame(0)).is_empty())
            .count();
        assert_eq!(sent, 59, "60 per second including the first one");
        // Garbage is dropped.
        assert!(
            room.handle(
                1,
                &Packet {
                    code: 240,
                    payload: vec![2, 0, 0]
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn test_mode_starts_without_plane_and_range_hands_out_items() {
        let mut room = Room::new(RoomSettings {
            game_mode: tabg_protocol::messages::game_mode::TEST,
            test_map: Some(crate::settings::TestMap::Range),
            respawn: Some(crate::settings::Respawn { delay: 3.0 }),
            kills_to_win: 0,
            countdown: 1.0,
            players_to_start: 1,
            loadout: vec![(6, 100)],
            ..Default::default()
        });
        // The Range client says hello before RoomInit.
        let hello = Packet {
            code: 242,
            payload: b"RNGE\x02\x01".to_vec(),
        };
        room.connect(0);
        let out = room.handle(0, &hello);
        assert_eq!(out[0].packet.code, 242);
        assert_eq!(out[0].packet.payload, b"RNGE\x02\x02");
        join(&mut room, 0, "a");
        enter(&mut room, 0);
        let out = run(&mut room, 0.0, 1.5);
        assert_eq!(room.state(), game_state::STARTED);
        let started = events(&out, EventCode::GameStateChanged);
        assert_eq!(
            GameStateChanged::decode(&started.last().unwrap().packet.payload).unwrap(),
            GameStateChanged::Started
        );
        assert!(events(&out, EventCode::PlaneUpdate).is_empty());
        assert_eq!(
            events(&out, EventCode::PlayerLootRecieved).len(),
            1,
            "loadout at start"
        );
        assert!(!events(&out, EventCode::ChunkEntry).is_empty());
        // F6 item request: into the inventory, and confirmed.
        let mut give = b"RNGE\x02\x03".to_vec();
        give.extend_from_slice(&160i32.to_le_bytes());
        give.push(1);
        let out = room.handle(
            0,
            &Packet {
                code: 242,
                payload: give,
            },
        );
        assert_eq!(out[0].packet.payload, encode_player_loot(&[(160, 1)]));
        assert_eq!(&out[1].packet.payload[..6], b"RNGE\x02\x04");
        // One player alone doesn't end an endless respawn match.
        run(&mut room, 1.5, 10.0);
        assert_eq!(room.state(), game_state::STARTED);
        // Wrong magic or a map that isn't on: ignored.
        assert!(
            room.handle(
                0,
                &Packet {
                    code: 243,
                    payload: b"DVTM\x01\x01".to_vec()
                }
            )
            .is_empty()
        );
    }

    fn skins_msg(op: u8, gear: Option<&[i32]>) -> Packet {
        let mut payload = b"SKIN\x01".to_vec();
        payload.push(op);
        if let Some(g) = gear {
            payload.extend_from_slice(&(g.len() as i32).to_le_bytes());
            for v in g {
                payload.extend_from_slice(&v.to_le_bytes());
            }
        }
        Packet { code: 244, payload }
    }

    #[test]
    fn skins_are_opt_in_and_broadcast_as_gear_changes() {
        let mut room = Room::new(lobby());
        room.connect(0);
        // Off by default: denied (reason 1 = disabled).
        let out = room.handle(0, &skins_msg(1, None));
        assert_eq!(out[0].packet.payload, b"SKIN\x01\x05\x01");
        let mut room = Room::new(RoomSettings {
            skins: true,
            ..lobby()
        });
        room.connect(0);
        assert_eq!(
            room.handle(0, &skins_msg(1, None))[0].packet.payload,
            b"SKIN\x01\x02"
        );
        join(&mut room, 0, "a");
        join(&mut room, 1, "b");
        let outfit = [5, 2, -1, -1, 7, -1, 9, 0, 11, 3, 13, 1];
        let out = room.handle(0, &skins_msg(3, Some(&outfit)));
        assert_eq!(out[0].to, Recipients::All);
        assert_eq!(out[0].packet.event(), Some(EventCode::GearChange));
        assert_eq!(&out[1].packet.payload[..6], b"SKIN\x01\x04");
        // Rate limited right after, and garbage outfits are refused.
        assert_eq!(
            room.handle(0, &skins_msg(3, Some(&outfit)))[0]
                .packet
                .payload[6],
            5
        );
        room.tick(5.0);
        let bad = [-7; 12];
        assert_eq!(
            room.handle(0, &skins_msg(3, Some(&bad)))[0].packet.payload[6],
            4
        );
        // A client that never said hello isn't authorized.
        assert_eq!(
            room.handle(1, &skins_msg(3, Some(&outfit)))[0]
                .packet
                .payload[6],
            2
        );
    }

    fn brawl_settings() -> RoomSettings {
        let house = |x: f32| crate::brawl::House {
            spawns: vec![SpawnPoint {
                position: [x, 130.0, 0.0],
                rotation: 0.0,
            }],
            loot: vec![(crate::brawl::HIGH, [x, 131.0, 1.0])],
        };
        RoomSettings {
            game_mode: tabg_protocol::messages::game_mode::BRAWL,
            kills_to_win: 2,
            brawl: crate::brawl::BrawlRules {
                houses: vec![house(0.0), house(300.0)],
                solo_houses: Vec::new(),
                presets: vec![crate::brawl::Preset {
                    tiers: [
                        vec![crate::loot::Drop {
                            weight: 1.0,
                            items: vec![(152, 1)],
                        }],
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    ],
                    redo_chance: 0.0,
                }],
            },
            ..Default::default()
        }
    }

    #[test]
    fn brawl_houses_doors_loot_and_team_respawns() {
        let mut room = Room::new(RoomSettings {
            countdown: 1.0,
            players_to_start: 2,
            ..brawl_settings()
        });
        for (peer, squad) in [(0u8, 1u64), (1, 2)] {
            room.connect(peer);
            let req = RoomInitRequest {
                player_name: format!("p{peer}"),
                squad_id: squad,
                ..Default::default()
            };
            room.handle(peer, &Packet::new(EventCode::RoomInit, req.encode()));
            let w = enter(&mut room, peer);
            let house_x = if room.players[&peer].group % 2 == 0 {
                0.0
            } else {
                300.0
            };
            assert_eq!(w.position[0], house_x, "spawned in the team's house");
        }
        let out = run(&mut room, 0.0, 1.5);
        assert_eq!(room.state(), game_state::STARTED);
        let doors = events(&out, EventCode::GameStateChanged);
        assert_eq!(
            doors.last().unwrap().packet.payload,
            vec![game_state::OPEN_DOORS]
        );
        assert!(events(&out, EventCode::PlaneUpdate).is_empty());
        assert_eq!(room.loot().len(), 2, "the same AK in both houses");
        // A wiped (solo) team comes back at its house; two kills win.
        hit(&mut room, 0, 1, 0.0);
        let out = run(&mut room, 1.5, 5.0);
        assert_eq!(events(&out, EventCode::PlayerRespawn).len(), 1);
        hit(&mut room, 0, 1, 0.0);
        run(&mut room, 5.0, 5.5);
        assert_eq!(room.state(), game_state::ENDED);
    }

    fn bomb_room() -> Room {
        let spawn = |x: f32| {
            vec![SpawnPoint {
                position: [x, 1.0, 0.0],
                rotation: 0.0,
            }]
        };
        let mut room = Room::new(RoomSettings {
            game_mode: tabg_protocol::messages::game_mode::BOMB,
            countdown: 5.0,
            players_to_start: 2,
            bomb: Some(crate::settings::BombRules {
                spawn: [-5.0, 0.25, -0.65],
                bomb_item: 168,
                start_weapon: 277,
                team_spawns: [spawn(-50.0), spawn(50.0)],
                rounds_to_win: 2,
                round_time: 90.0,
                bomb_time: 30.0,
                defuse_time: 5.0,
            }),
            ..Default::default()
        });
        for (peer, name) in [(0, "t"), (1, "ct")] {
            join(&mut room, peer, name);
            enter(&mut room, peer);
        }
        room
    }

    fn bomb_loot_id(room: &Room) -> i32 {
        room.loot
            .in_chunks(&tabg_protocol::loot::area_around(chunk_of([
                -5.0, 0.25, -0.65,
            ])))
            .into_iter()
            .find(|i| i.item_id == 168)
            .map(|i| i.network_id)
            .expect("bomb on the map")
    }

    #[test]
    fn bomb_rounds_buy_plant_defuse_and_win() {
        let mut room = bomb_room();
        assert_eq!(
            (room.players[&0].group, room.players[&1].group),
            (0, 1),
            "teams balanced"
        );
        let out = run(&mut room, 0.0, 0.5);
        assert_eq!(room.state(), game_state::COUNT_DOWN);
        assert!(!events(&out, EventCode::MoneyChanged).is_empty());
        assert_eq!(
            events(&out, EventCode::PlayerRespawn)[0].packet.payload[0],
            2,
            "both respawn"
        );
        assert_eq!(
            events(&out, EventCode::GunPurchasedAccepted).len(),
            2,
            "start guns"
        );
        // Buying works in the buy phase only, and costs money.
        let mut buy = tabg_protocol::Writer::new();
        buy.f32(400.0).i32(152).i32(1).i32(90).i32(0);
        let buy = buy.into_inner();
        let out = room.handle(1, &Packet::new(EventCode::RequestPurchaseGun, buy.clone()));
        assert_eq!(events(&out, EventCode::GunPurchasedAccepted).len(), 1);
        assert!(
            room.handle(1, &Packet::new(EventCode::RequestPurchaseGun, buy.clone()))
                .is_empty(),
            "100 left, can't afford"
        );
        let out = run(&mut room, 0.5, 6.0);
        assert_eq!(room.state(), game_state::STARTED);
        let started = events(&out, EventCode::GameStateChanged);
        assert_eq!(
            started.last().unwrap().packet.payload[..1],
            [game_state::STARTED]
        );
        assert_eq!(
            started.last().unwrap().packet.payload.len(),
            5,
            "with the round time"
        );
        assert!(
            room.handle(1, &Packet::new(EventCode::RequestPurchaseGun, buy))
                .is_empty(),
            "no buying now"
        );
        // CT can't pick the bomb up; T can, then plants; CT defuses.
        let bomb = bomb_loot_id(&room);
        let pick = |p: u8| PickupRequest {
            player_index: p,
            network_id: bomb,
            slot: 0,
        };
        assert!(
            room.handle(
                1,
                &Packet::new(EventCode::RequestWeaponPickUp, pick(1).encode())
            )
            .is_empty()
        );
        assert!(
            !room
                .handle(
                    0,
                    &Packet::new(EventCode::RequestWeaponPickUp, pick(0).encode())
                )
                .is_empty()
        );
        let mut at = tabg_protocol::Writer::new();
        at.vec3([1.0, 0.0, 1.0]);
        let out = room.handle(
            0,
            &Packet::new(EventCode::RequestPlantBomb, at.into_inner()),
        );
        assert_eq!(out[0].packet.event(), Some(EventCode::BombPlanted));
        assert!(
            room.handle(0, &Packet::new(EventCode::RequestDefuseBomb, vec![]))
                .is_empty(),
            "T can't defuse"
        );
        let out = room.handle(1, &Packet::new(EventCode::RequestDefuseBomb, vec![]));
        assert_eq!(out[0].packet.event(), Some(EventCode::BombStartDefuse));
        let out = run(&mut room, 6.0, 12.0);
        let over = events(&out, EventCode::GameStateChanged);
        assert_eq!(
            over[0].packet.payload,
            vec![6, 1, 3],
            "CT (team 1) won by defuse"
        );
        assert!(!events(&out, EventCode::ScoresChanged).is_empty());
        // Intermission, then round 2 (sides switched after round 1 of a first-to-2).
        let out = run(&mut room, 12.0, 23.0);
        assert!(
            !events(&out, EventCode::PlayerRespawn).is_empty(),
            "next round"
        );
        run(&mut room, 23.0, 29.0);
        assert_eq!(room.state(), game_state::STARTED);
        hit(&mut room, 0, 1, 0.0);
        let out = run(&mut room, 29.0, 29.5);
        let over = events(&out, EventCode::GameStateChanged);
        assert_eq!(
            over[0].packet.payload,
            vec![6, 0, 1],
            "team 0 won by elimination"
        );
        // 1-1: one more round decides.
        run(&mut room, 29.5, 46.0);
        hit(&mut room, 1, 0, 0.0);
        let out = run(&mut room, 46.0, 46.5);
        assert_eq!(room.state(), game_state::ENDED);
        assert_eq!(
            events(&out, EventCode::TeamDead).len(),
            2,
            "placements for both teams"
        );
    }

    #[test]
    fn the_dead_drop_their_loot_and_a_soul_that_revives_a_teammate() {
        let mut room = fight_room_with(
            RoomSettings {
                soul_item: Some(10),
                curses: vec![3, 4],
                ..Default::default()
            },
            &[(0, 1), (1, 1), (2, 2)],
        );
        // Player 1 picks up a gun and ammo, fires 30 rounds and heals with a bandage.
        room.settings.heal_amounts.insert(131, 15.0);
        let gun = room.loot.add(160, 1, [0.0; 3]);
        let ammo = room.loot.add(6, 90, [0.0; 3]);
        let bandage = room.loot.add(131, 2, [0.0; 3]);
        for item in [&gun, &ammo, &bandage] {
            let req = PickupRequest {
                player_index: 1,
                network_id: item.network_id,
                slot: 0,
            };
            room.handle(
                1,
                &Packet::new(EventCode::RequestWeaponPickUp, req.encode()),
            );
        }
        let mut fire = tabg_protocol::Writer::new();
        fire.u8(1).u8(firing_mode::BURST).i32(6).i32(30);
        room.handle(1, &Packet::new(EventCode::PlayerFire, fire.into_inner()));
        hit(&mut room, 2, 1, 50.0);
        let heal = tabg_protocol::actions::HealRequest {
            player_index: 1,
            item_id: 131,
        };
        room.handle(1, &Packet::new(EventCode::RequestHealing, heal.encode()));
        assert_eq!(
            (
                room.carried(1, 160),
                room.carried(1, 6),
                room.carried(1, 131)
            ),
            (1, 60, 1)
        );
        // Down and out: everything falls, plus a soul.
        hit(&mut room, 2, 1, 0.0);
        run(&mut room, 2.0, 4.0);
        hit(&mut room, 2, 1, 0.0);
        if room.players()[1].alive {
            // Squad mates go down first; finish the downed player.
            room.kill(1, false);
        }
        assert_eq!(room.carried(1, 6), 0, "inventory emptied");
        let here = tabg_protocol::loot::area_around(chunk_of(room.players[&1].position));
        let on_ground = room.loot.in_chunks(&here);
        let soul = on_ground
            .iter()
            .find(|i| i.item_id == 10)
            .expect("a soul on the ground")
            .clone();
        assert!(
            on_ground.iter().any(|i| i.item_id == 6 && i.quantity == 60),
            "the ammo left"
        );
        // Teammate 0 picks the soul up and brings 1 back with a curse.
        let req = PickupRequest {
            player_index: 0,
            network_id: soul.network_id,
            slot: 0,
        };
        room.handle(
            0,
            &Packet::new(EventCode::RequestWeaponPickUp, req.encode()),
        );
        assert_eq!(room.carried(0, 10), 1);
        let mut w = tabg_protocol::Writer::new();
        w.i32(soul.network_id).u8(1);
        let out = room.handle(
            0,
            &Packet::new(EventCode::RequestRespawnTeamMate, w.into_inner()),
        );
        let respawn = events(&out, EventCode::PlayerRespawn);
        assert_eq!(respawn.len(), 1);
        assert!(
            [3u8, 4].contains(respawn[0].packet.payload.last().unwrap()),
            "a random curse"
        );
        assert!(room.players()[1].alive);
        assert_eq!(room.carried(0, 10), 0, "the soul is spent");
    }

    #[test]
    fn spell_drops_fall_in_the_ring_and_spill_blessings() {
        let mut room = fight_room_with(
            RoomSettings {
                spell_drops: Some(SpellDrops {
                    interval: [100.0, 100.0],
                    rare: vec![106],
                    epic: vec![59],
                    legendary: vec![83],
                }),
                loot: crate::loot::LootRules {
                    spots: vec![crate::loot::Spot {
                        position: [0.0, 150.0, 0.0],
                        force: false,
                    }],
                    modifiers: Vec::new(),
                },
                ..Default::default()
            },
            &[(0, 1), (1, 2)],
        );
        room.ring.white_center = [0.0; 3];
        room.ring.white_size = 1000.0;
        let out = run(&mut room, 1.5, 105.0);
        let spawned = events(&out, EventCode::DropSpawned);
        assert_eq!(spawned.len(), 1, "first drop 60–100 s after the start");
        let payload = &spawned[0].packet.payload;
        assert_eq!(&payload[..4], &0i32.to_le_bytes());
        let rarity = payload[16];
        let mut w = tabg_protocol::Writer::new();
        w.u8(0).i32(0);
        let click = w.into_inner();
        let out = room.handle(
            0,
            &Packet::new(EventCode::RequestClickInteract, click.clone()),
        );
        let opened = &out[0].packet.payload;
        let expected = match rarity {
            3 => 1,
            2 => 2,
            _ => 3,
        };
        assert_eq!(opened[4], expected);
        assert!(
            room.handle(1, &Packet::new(EventCode::RequestClickInteract, click))
                .is_empty(),
            "only once"
        );
    }

    #[test]
    fn early_deaths_go_to_the_boss_fight() {
        let mut room = fight_room_with(
            RoomSettings {
                respawn_minigame: true,
                ..Default::default()
            },
            &[(0, 1), (1, 2), (2, 3)],
        );
        let out = hit(&mut room, 0, 1, 0.0);
        let dead =
            PlayerDead::decode(&events(&out, EventCode::PlayerDead)[0].packet.payload).unwrap();
        assert_eq!(dead.spectate, 254, "into the boss fight");
        assert!(
            events(&out, EventCode::TeamDead).is_empty(),
            "not out while fighting the boss"
        );
        let out = room.handle(1, &Packet::new(EventCode::BossFightResult, vec![1, 1]));
        assert_eq!(
            out[0].packet.event(),
            Some(EventCode::PlayerRespawnFromBoss)
        );
        assert!(room.players()[1].alive);
        // Only once per match: the next death is final.
        let out = hit(&mut room, 0, 1, 0.0);
        let dead =
            PlayerDead::decode(&events(&out, EventCode::PlayerDead)[0].packet.payload).unwrap();
        assert_ne!(dead.spectate, 254);
        assert_eq!(events(&out, EventCode::TeamDead).len(), 1);
        // A lost fight puts the team out.
        let out = hit(&mut room, 0, 2, 0.0);
        assert!(events(&out, EventCode::TeamDead).is_empty());
        let out = room.handle(2, &Packet::new(EventCode::BossFightResult, vec![0, 0]));
        assert_eq!(events(&out, EventCode::TeamDead).len(), 1);
    }

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

    #[test]
    fn bots_join_jump_land_armed_and_fight() {
        let mut room = Room::new(RoomSettings {
            bots: 2,
            nav: Some(flat_island()),
            players_to_start: 3,
            countdown: 1.0,
            ..Default::default()
        });
        join(&mut room, 0, "human");
        enter(&mut room, 0);
        let out = room.tick(0.1);
        assert_eq!(events(&out, EventCode::Login).len(), 2, "two bots joined");
        assert!(room.is_bot(253) && room.is_bot(252) && !room.is_bot(0));
        // Countdown, plane; everyone is out of the plane by the end of the flight.
        let out = run(&mut room, 0.1, 60.0);
        assert_eq!(room.state(), game_state::STARTED);
        let landed: Vec<_> = events(&out, EventCode::PlayerLand)
            .iter()
            .filter(|o| room.is_bot(o.packet.payload[0]))
            .map(|o| o.packet.payload[0])
            .collect();
        assert_eq!(landed.len(), 2, "both bots landed");
        assert_eq!(
            events(&out, EventCode::WeaponChanged).len(),
            2,
            "and took out a gun"
        );
        // Put the human right next to a bot: it turns and shoots.
        let bot_pos = room.players[&253].position;
        let p = room.players.get_mut(&0).unwrap();
        p.position = [bot_pos[0] + 8.0, bot_pos[1], bot_pos[2]];
        p.dropped = true;
        p.landed = true;
        let out = run(&mut room, 60.0, 60.4);
        assert!(
            room.players[&0].health == 100.0
                && events(&out, EventCode::PlayerDamaged)
                    .iter()
                    .all(|o| o.packet.payload.first() != Some(&0)),
            "a bot takes a moment to react"
        );
        let out = run(&mut room, 60.4, 70.0);
        assert!(
            !events(&out, EventCode::PlayerFire).is_empty(),
            "bots shoot"
        );
        assert!(
            !events(&out, EventCode::PlayerDamaged).is_empty(),
            "and hit"
        );
        assert!(room.players[&0].health < 100.0 || !room.players[&0].alive);
    }

    #[test]
    fn the_radar_shows_bots_to_those_allowed() {
        let hello = Packet {
            code: 241,
            payload: b"ARDR\x02\x01".to_vec(),
        };
        let radar = |out: &[Outgoing]| -> Vec<Outgoing> {
            out.iter()
                .filter(|o| o.packet.code == 241)
                .cloned()
                .collect()
        };
        // Off by default: denied.
        let mut room = Room::new(RoomSettings {
            bots: 1,
            nav: Some(flat_island()),
            ..Default::default()
        });
        let out = room.handle(0, &hello);
        assert_eq!(out[0].packet.payload, b"ARDR\x02\x04\x01");
        // Admins only: a non-admin is denied once in, an admin gets snapshots.
        let mut room = Room::new(RoomSettings {
            bots: 2,
            nav: Some(flat_island()),
            bot_radar: crate::settings::BotRadar::Admins,
            admins: vec!["boss".into()],
            players_to_start: 5,
            ..Default::default()
        });
        join(&mut room, 0, "boss");
        enter(&mut room, 0);
        join(&mut room, 1, "guest");
        enter(&mut room, 1);
        assert_eq!(room.handle(0, &hello)[0].packet.payload, b"ARDR\x02\x02");
        assert_eq!(
            room.handle(1, &hello)[0].packet.payload,
            b"ARDR\x02\x04\x02"
        );
        let out = radar(&room.tick(0.1));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, Recipients::Many(vec![0]), "only the admin");
        let snap = &out[0].packet.payload;
        assert_eq!(&snap[..6], b"ARDR\x02\x03");
        assert_eq!(snap[6], 2, "two bots, no humans");
        assert!(room.is_bot(snap[7]));
        assert!(radar(&room.tick(0.2)).is_empty(), "four times a second");
        assert_eq!(radar(&room.tick(0.4)).len(), 1);
        // Everyone, after a hot reload: the guest gets it too, without saying hello again.
        let mut settings = room.settings.clone();
        settings.bot_radar = crate::settings::BotRadar::Everyone;
        room.apply_settings(settings);
        let out = radar(&room.tick(0.7));
        assert_eq!(out[0].to, Recipients::Many(vec![0, 1]));
        room.disconnect(0);
        room.disconnect(1);
        assert!(radar(&room.tick(1.0)).is_empty());
    }

    #[test]
    fn bots_walk_when_nobody_is_around() {
        let mut room = Room::new(RoomSettings {
            bots: 1,
            nav: Some(flat_island()),
            players_to_start: 2,
            countdown: 1.0,
            ..Default::default()
        });
        join(&mut room, 0, "human");
        enter(&mut room, 0);
        run(&mut room, 0.0, 60.0);
        // Keep the human far away so the bot wanders.
        room.players.get_mut(&0).unwrap().position = [1900.0, 120.0, 1900.0];
        let before = room.players[&253].position;
        run(&mut room, 60.0, 80.0);
        let after = room.players[&253].position;
        let moved = (after[0] - before[0]).hypot(after[2] - before[2]);
        assert!(moved > 5.0, "the bot walked {moved} m");
        assert_eq!(after[1], 120.0, "on the ground");
    }

    fn vehicle_types() -> Vec<crate::vehicles::VehicleType> {
        use crate::vehicles::{Hull, VehicleType};
        let part = |i: u8, health: f32, name: &str| tabg_protocol::messages::CarPart {
            index: i,
            health,
            name: name.into(),
        };
        vec![
            VehicleType {
                car_type: 0,
                name: "Bike".into(),
                seats: vec![(0, true), (1, false)],
                parts: vec![part(0, 500.0, "Chassi")],
                hull: Hull::new(&[[-0.5, -0.5, -1.0, 0.5, 0.5, 1.0]]),
            },
            VehicleType {
                car_type: 4,
                name: "Heli".into(),
                seats: vec![(0, true), (1, false)],
                parts: vec![part(0, 1000.0, "Chassi"), part(1, 50.0, "Wheel")],
                hull: Hull::new(&[[-1.0, -1.3, -2.4, 1.0, 1.1, 2.4]]),
            },
        ]
    }

    /// Two enemies on a flat island with one map bike; FlyingControls hellos from both.
    fn vehicle_room() -> Room {
        let mut settings = RoomSettings {
            nav: Some(flat_island()),
            cars: vec![crate::cars::CarSpawn {
                car_type: 0,
                car_index: 0,
                position: [0.0, 121.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                seats: vec![(0, true), (1, false)],
                parts: vec![],
            }],
            ..Default::default()
        };
        settings.vehicles.types = vehicle_types();
        let mut room = fight_room_with(settings, &[(0, 1), (1, 2)]);
        for p in [0, 1] {
            let pl = room.players.get_mut(&p).unwrap();
            pl.landed = true;
            pl.position = [10.0 * f32::from(p), 121.0, 0.0];
        }
        for p in [0, 1] {
            room.handle(p, &vehicle_packet(0, &[]));
        }
        room
    }

    fn vehicle_packet(kind: u8, body: &[u8]) -> Packet {
        let mut payload = 0x5647_4854u32.to_le_bytes().to_vec();
        payload.extend([2, kind]);
        payload.extend_from_slice(body);
        Packet { code: 245, payload }
    }

    fn vehicle_events(out: &[Outgoing], kind: u8) -> Vec<&Outgoing> {
        out.iter()
            .filter(|o| o.packet.code == 245 && o.packet.payload.get(5) == Some(&kind))
            .collect()
    }

    #[test]
    fn unused_vehicles_join_the_map() {
        let mut settings = RoomSettings {
            nav: Some(flat_island()),
            cars: (0..20)
                .map(|i| crate::cars::CarSpawn {
                    car_type: 0,
                    car_index: i,
                    position: [i as f32 * 50.0, 121.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    seats: vec![(0, true)],
                    parts: vec![],
                })
                .collect(),
            ..Default::default()
        };
        settings.vehicles.types = vehicle_types();
        settings.vehicles.unused_chance = 1.0;
        settings.vehicles.unused_max = 5;
        let room = Room::new(settings);
        let helis: Vec<_> = room.cars.values().filter(|c| c.car_type == 4).collect();
        assert_eq!(room.cars.len(), 25);
        assert_eq!(helis.len(), 5, "only unused types are added");
        assert!(
            helis
                .iter()
                .all(|c| (c.position[1] - (120.0 + 1.3 + 0.5)).abs() < 1e-3)
        );
        assert!(helis.iter().all(|c| room.is_aircraft_car(c.index)));
        assert!(
            helis
                .iter()
                .all(|c| c.seats.len() == 2 && c.parts.len() == 2)
        );
    }

    #[test]
    fn admins_spawn_vehicles_for_flying_controls_clients() {
        let mut room = vehicle_room();
        room.settings.admins = vec!["p0".into()];
        room.apply_settings(room.settings.clone());
        let out = chat(&mut room, 0, "/vehicle heli");
        let spawn = vehicle_events(&out, 1);
        assert_eq!(spawn.len(), 1, "{out:?}");
        assert_eq!(spawn[0].to, Recipients::Many(vec![0, 1]));
        let index = i32::from_le_bytes(spawn[0].packet.payload[6..10].try_into().unwrap());
        assert_eq!(room.cars[&index].car_type, 4);
        assert!(room.is_aircraft_car(index));
        // The spawner's client confirms.
        let out = room.handle(0, &vehicle_packet(2, &index.to_le_bytes()));
        assert_eq!(events(&out, EventCode::ChatMessage).len(), 1);
        // Too soon for another one, and a vanilla client can't spawn.
        let out = chat(&mut room, 0, "/vehicle heli");
        assert!(vehicle_events(&out, 1).is_empty());
        room.vehicles.clients.remove(&0);
        let out = chat(&mut room, 0, "/vehicle bike");
        assert!(vehicle_events(&out, 1).is_empty());
        // A /vehicle spawn is never shown to vanilla clients.
        assert!(!room.car_visible_to(index, 7));
        assert!(room.car_visible_to(index, 1));
    }

    #[test]
    fn aircraft_crash_explode_and_hand_over() {
        let mut room = vehicle_room();
        room.cars.insert(
            5,
            crate::cars::Car::spawn(&crate::cars::CarSpawn {
                car_type: 4,
                car_index: 5,
                position: [3.0, 140.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                seats: vec![(0, true), (1, false)],
                parts: vec![],
            }),
        );
        room.track_aircraft(5, "Heli", 0.0);
        seat(&mut room, 0, 5, 0, SEAT_GET_IN);
        seat(&mut room, 1, 5, 1, SEAT_GET_IN);
        // The pilot leaves: the passenger takes over.
        seat(&mut room, 0, 5, 0, SEAT_GET_OUT);
        let out = room.tick(3.0);
        assert_eq!(vehicle_events(&out, 21).len(), 1, "pilot handover");
        assert_eq!(room.players[&1].seat, Some((5, 0, true)));
        // Too slow a bump is nothing; a hard crash (after the start protection) destroys it.
        let crash = |speed: f32| {
            let mut b = 5i32.to_le_bytes().to_vec();
            b.extend(speed.to_le_bytes());
            vehicle_packet(3, &b)
        };
        assert!(room.handle(1, &crash(10.0)).is_empty());
        room.tick(6.0);
        let out = room.handle(1, &crash(30.0));
        let boom = vehicle_events(&out, 4);
        assert_eq!(boom.len(), 1);
        assert_eq!(
            *boom[0].packet.payload.last().unwrap(),
            1,
            "the occupant is listed"
        );
        assert!(!room.cars.contains_key(&5));
        assert_eq!(room.players[&1].seat, None);
    }

    #[test]
    fn unpiloted_helicopters_fall() {
        let mut room = vehicle_room();
        room.cars.insert(
            5,
            crate::cars::Car::spawn(&crate::cars::CarSpawn {
                car_type: 4,
                car_index: 5,
                position: [3.0, 200.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                seats: vec![(0, true)],
                parts: vec![],
            }),
        );
        room.track_aircraft(5, "Heli", 0.0);
        seat(&mut room, 0, 5, 0, SEAT_GET_IN);
        room.tick(2.0);
        room.tick(2.1);
        seat(&mut room, 0, 5, 0, SEAT_GET_OUT);
        let out = run(&mut room, 3.0, 4.0);
        assert!(!vehicle_events(&out, 18).is_empty(), "AircraftFall updates");
        assert!(room.cars[&5].position[1] < 200.0);
        // 80 m up: it hits the ground fast enough to explode.
        let out = run(&mut room, 4.1, 12.0);
        assert_eq!(vehicle_events(&out, 4).len(), 1);
        assert!(!room.cars.contains_key(&5));
    }

    #[test]
    fn helicopter_missiles_fly_and_explode() {
        let mut room = vehicle_room();
        room.cars.insert(
            5,
            crate::cars::Car::spawn(&crate::cars::CarSpawn {
                car_type: 4,
                car_index: 5,
                position: [-30.0, 121.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                seats: vec![(0, true)],
                parts: vec![],
            }),
        );
        room.track_aircraft(5, "Heli", 0.0);
        room.players.get_mut(&0).unwrap().position = [-30.0, 121.0, 0.0];
        seat(&mut room, 0, 5, 0, SEAT_GET_IN);
        room.players.get_mut(&1).unwrap().position = [0.0, 121.0, 0.0];
        // Unguided, straight at the enemy 30 m away.
        let mut b = (-1i32).to_le_bytes().to_vec();
        for f in [1.0f32, 0.0, 0.0] {
            b.extend(f.to_le_bytes());
        }
        b.push(0);
        let out = room.handle(0, &vehicle_packet(6, &b));
        assert_eq!(vehicle_events(&out, 7).len(), 1, "launched");
        let reply = vehicle_events(&out, 10);
        assert_eq!(*reply[0].packet.payload.last().unwrap(), 1, "fired");
        let mut out = Vec::new();
        let mut t = 2.0;
        while t < 4.0 {
            out.extend(room.tick(t));
            t += 1.0 / 30.0;
        }
        let removed = vehicle_events(&out, 8);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].packet.payload[6 + 16], 2, "exploded");
        // Cooldown: can't fire again right away.
        let out = room.handle(0, &vehicle_packet(6, &b));
        assert!(vehicle_events(&out, 7).is_empty());
    }

    #[test]
    fn fpv_drones_launch_fly_and_crash() {
        let mut room = vehicle_room();
        room.settings.admins = vec!["p0".into()];
        room.apply_settings(room.settings.clone());
        let out = chat(&mut room, 0, "/fpv");
        assert_eq!(events(&out, EventCode::PlayerLootRecieved).len(), 1);
        assert_eq!(room.carried(0, 30001), 1);
        let t = tabg_protocol::actions::ThrowRequest {
            player_index: 0,
            item_id: 30001,
            quantity: 1,
            position: [0.0, 122.0, 0.0],
            direction: [0.0, 1.0, 0.0],
        };
        room.handle(0, &Packet::new(EventCode::RequestItemThrow, t.encode()));
        assert_eq!(room.carried(0, 30001), 0);
        let v3 = |b: &mut Vec<u8>, v: [f32; 3]| {
            for f in v {
                b.extend(f.to_le_bytes());
            }
        };
        let mut b = 9i32.to_le_bytes().to_vec();
        v3(&mut b, [0.0, 125.0, 0.0]);
        v3(&mut b, [0.0; 3]);
        let out = room.handle(0, &vehicle_packet(11, &b));
        let state = vehicle_events(&out, 12);
        assert_eq!(state.len(), 1, "{out:?}");
        let id = i32::from_le_bytes(state[0].packet.payload[6..10].try_into().unwrap());
        room.tick(2.5);
        // Fly up a bit, then into the ground.
        let mut b = id.to_le_bytes().to_vec();
        b.extend(1i32.to_le_bytes());
        v3(&mut b, [0.0, 130.0, 2.0]);
        v3(&mut b, [0.0; 3]);
        assert_eq!(
            vehicle_events(&room.handle(0, &vehicle_packet(13, &b)), 12).len(),
            1
        );
        let mut b = id.to_le_bytes().to_vec();
        b.push(1);
        v3(&mut b, [0.0, 126.0, 4.0]);
        let out = room.handle(0, &vehicle_packet(14, &b));
        assert!(out.is_empty(), "nothing there to crash into");
        room.tick(2.8);
        let mut b = id.to_le_bytes().to_vec();
        b.extend(2i32.to_le_bytes());
        v3(&mut b, [0.0, 121.5, 3.0]);
        v3(&mut b, [0.0; 3]);
        assert_eq!(
            vehicle_events(&room.handle(0, &vehicle_packet(13, &b)), 12).len(),
            1
        );
        let mut b = id.to_le_bytes().to_vec();
        b.push(1);
        v3(&mut b, [0.0, 120.2, 3.0]);
        let out = room.handle(0, &vehicle_packet(14, &b));
        let finish = vehicle_events(&out, 15);
        assert_eq!(finish.len(), 1);
        assert_eq!(*finish[0].packet.payload.last().unwrap(), 1, "exploded");
        // Launching again without a throw is refused.
        let mut b = 9i32.to_le_bytes().to_vec();
        v3(&mut b, [0.0, 125.0, 0.0]);
        v3(&mut b, [0.0; 3]);
        assert_eq!(
            vehicle_events(&room.handle(0, &vehicle_packet(11, &b)), 16).len(),
            1
        );
    }

    #[test]
    fn hover_taxis_drive_the_roads() {
        let mut room = vehicle_room();
        room.settings
            .vehicles
            .types
            .push(crate::vehicles::VehicleType {
                car_type: 6,
                name: "Hover_Car".into(),
                seats: vec![(0, true)],
                parts: vec![],
                hull: crate::vehicles::Hull::new(&[[-1.0, -0.8, -1.2, 1.0, 0.7, 1.2]]),
            });
        let points: Vec<[f32; 3]> = (0..=20).map(|i| [i as f32 * 10.0, 120.0, 30.0]).collect();
        let links: Vec<[usize; 2]> = (0..20).map(|i| [i, i + 1]).collect();
        room.settings.vehicles.roads = Some(std::sync::Arc::new(crate::roads::RoadGraph::new(
            points, &links,
        )));
        room.cars.insert(
            7,
            crate::cars::Car::spawn(&crate::cars::CarSpawn {
                car_type: 6,
                car_index: 7,
                position: [0.0, 121.5, 30.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                seats: vec![(0, true)],
                parts: vec![],
            }),
        );
        room.players.get_mut(&0).unwrap().position = [0.0, 121.0, 30.0];
        seat(&mut room, 0, 7, 0, SEAT_GET_IN);
        let mut b = vec![1u8];
        for f in [150.0f32, 120.0, 30.0] {
            b.extend(f.to_le_bytes());
        }
        room.handle(0, &vehicle_packet(24, &b));
        // A client update can't move it.
        let up = CarUpdate {
            player_index: 0,
            position: [500.0, 121.0, 30.0],
            rotation: PackedQuaternion::pack([0.0, 0.0, 0.0, 1.0]),
            input: PackedDirection::ZERO,
            driving_state: 0,
        };
        room.handle(0, &Packet::new(EventCode::CarUpdate, up.encode()));
        let out = run(&mut room, 2.0, 6.0);
        assert!(!vehicle_events(&out, 26).is_empty(), "status to the driver");
        let at = room.cars[&7].position;
        assert!(at[0] > 30.0 && at[0] < 160.0, "flew along the road: {at:?}");
        assert!((at[2] - 30.0).abs() < 3.0, "on the road: {at:?}");
        assert!(at[1] > 121.0 && at[1] < 125.0, "hovering: {at:?}");
        assert!(
            (room.players[&0].position[0] - at[0]).abs() < 0.01,
            "the driver goes with it"
        );
    }

    #[test]
    fn the_storm_hurts_bots_outside_the_ring() {
        let mut room = Room::new(RoomSettings {
            bots: 1,
            nav: Some(flat_island()),
            players_to_start: 2,
            countdown: 1.0,
            ..Default::default()
        });
        join(&mut room, 0, "human");
        enter(&mut room, 0);
        run(&mut room, 0.0, 60.0);
        let (centre, size) = room.ring.current(60.0);
        let far = [centre[0] + size, 120.0, centre[2]];
        let p = room.players.get_mut(&253).unwrap();
        p.position = far;
        p.landed = true;
        let before = room.players[&253].health;
        room.players.get_mut(&253).unwrap().position = far;
        room.bot_ring_at = 0.0;
        let out = room.tick(60.5);
        assert!(room.players[&253].health < before, "{out:?}");
        room.players.get_mut(&253).unwrap().health = 0.5;
        room.players.get_mut(&253).unwrap().position = far;
        room.bot_ring_at = 0.0;
        let out = room.tick(61.6);
        assert!(!room.players[&253].alive, "a ring death");
        assert_eq!(events(&out, EventCode::PlayerDead).len(), 1);
    }
}
