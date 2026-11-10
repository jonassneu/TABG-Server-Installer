//! TABG game simulation.
//!
//! [`Room`] is pure: it takes player connections, packets and the current time, and returns the
//! packets to send. No sockets, threads or clocks, so every rule is unit-testable.
//!
//! Current scope: accepting players, sending them the world, replicating movement, and the match
//! flow (countdown, plane, drops, ring, end). Loot, weapons and damage come next.

pub mod brawl;
pub mod cars;
pub mod loot;
pub mod nav;
pub mod plane;
pub mod princess;
pub mod ring;
pub mod rng;
pub mod roads;
mod room;
mod settings;
pub mod vehicles;

pub use princess::{BotSkill, PrincessBase, PrincessRules};
pub use ring::RingRules;
pub use room::{
    Outgoing, PlayerInfo, RESTART_AFTER_END, Recipients, Room, RoomEvent, SpellDrops, VehicleRules,
};
pub use settings::{
    BombRules, BotRadar, ForceStart, InventoryCheck, Respawn, RoomSettings, SpawnPoint, TestMap,
};
