//! TABG's `EventCode` values: the first byte of every packet.
//!
//! Names keep TABG's own spelling (including typos such as `PingRecieved`) so they can be
//! matched against the game. 126 codes; 239 and 240-247 are free and used by our client mods.

macro_rules! event_codes {
    ($($name:ident = $value:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum EventCode {
            $($name = $value,)*
        }

        impl EventCode {
            pub const ALL: &[EventCode] = &[$(EventCode::$name,)*];

            pub fn from_u8(value: u8) -> Option<Self> {
                match value {
                    $($value => Some(EventCode::$name),)*
                    _ => None,
                }
            }
        }
    };
}

event_codes! {
    NoCodeSet = 0,
    Info = 1,
    PlayerUpdate = 2,
    Login = 3,
    PlayerLeft = 4,
    WeaponPickUpAccepted = 5,
    PlayerDamaged = 6,
    PlayerRespawn = 7,
    SpawnGun = 8,
    Data = 9,
    GameStateChanged = 10,
    ServerRestart = 11,
    AllWeapons = 12,
    SeatAccepted = 13,
    PingRecieved = 14,
    CarUpdate = 15,
    PlayerMarkerEvent = 16,
    CarDamage = 17,
    TeamDead = 18,
    ItemDrop = 19,
    RingUpdate = 20,
    PlayerAirplaneDropped = 21,
    PlaneUpdate = 22,
    AllDrop = 23,
    RoomInitRequestResponse = 24,
    ServerShutDown = 25,
    ChunkEntry = 26,
    ChunkExit = 27,
    ReviveState = 28,
    PlayerEnteredChunk = 29,
    PlayerLeftChunk = 30,
    ChatMessage = 31,
    ItemThrown = 32,
    WeaponChanged = 33,
    GearChange = 34,
    ThrowChatMessage = 35,
    PlayerHealed = 36,
    PlayerDead = 37,
    PlayerStateChanged = 38,
    PlayerEffect = 39,
    PlayerDeathLootDrop = 40,
    PlayerFire = 41,
    ACMessage = 42,
    PlayerLand = 43,
    KickPlayer = 44,
    SpectatorForce = 45,
    ACRequestedData = 46,
    KickPlayerMessage = 47,
    PlayerRegMessage = 48,
    LootRemovedFromMap = 49,
    SoulDrop = 50,
    CatchPhrase = 51,
    BlessingRecieved = 52,
    CurseCleansed = 53,
    TakeDamageEvent = 54,
    PlayerHealthStateChanged = 55,
    DropSpawned = 56,
    DropOpened = 57,
    SyncProjectileEvent = 58,
    PlayerFireSyncReturn = 59,
    LootDropped = 60,
    ScoresChanged = 61,
    BombPlanted = 62,
    GunPurchasedAccepted = 63,
    MoneyChanged = 64,
    BombStartDefuse = 65,
    BombStopDefuse = 66,
    PlayerLootRecieved = 67,
    GraveStoneSpawned = 68,
    PlayerRespawnFromBoss = 69,
    NetworkPlayerTransmittedPackage = 70,
    LootPackGiven = 71,
    RecievedSteamID = 72,
    ServerBrowserHandshake = 100,
    SteamTicketAuthenticationResponse = 101,
    AssignSteamID = 163,
    FlagMatchOver = 164,
    BossFightResult = 166,
    PackedServerData = 167,
    RequestStopDefuse = 168,
    RequestDefuseBomb = 169,
    RequestPlantBomb = 170,
    RequestPurchaseGun = 171,
    RequestSyncProjectileEvent = 172,
    RequestClickInteract = 173,
    RequestTakeDamageEvent = 174,
    RequestCurseCleanse = 175,
    RequestHealthState = 176,
    RequestBlessing = 177,
    SendCatchPhrase = 178,
    RequestRespawnTeamMate = 179,
    RingDeath = 180,
    KickMessage = 181,
    ACCacheState = 182,
    SpectatorRequest = 184,
    CarTemporaryUpdate = 189,
    PassangerUpdate = 190,
    PlayerState = 193,
    RequestHealing = 194,
    PhotonCloseRoom = 195,
    WeaponChange = 198,
    RequestItemThrow = 199,
    RoomInit = 202,
    RequestAirplaneDrop = 203,
    RequestItemDrop = 206,
    AdminCommand = 207,
    PlayerMarkerAdded = 208,
    TABGPing = 210,
    RequestSeat = 211,
    DamageEvent = 214,
    RequestWeaponPickUp = 215,
    RequestWorldState = 218,
    LobbyStats = 224,
    GameServerOffline = 225,
    AppStats = 226,
    QueueState = 228,
    GameListUpdate = 229,
    GameList = 230,
    Ping = 248,
    EventCacheSlicePurged = 249,
    CacheSliceChanged = 250,
    ErrorInfo = 251,
    Disconnect = 252,
    PropertiesChanged = 253,
    Leave = 254,
    Join = 255,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_complete_and_round_trips() {
        assert_eq!(EventCode::ALL.len(), 126);
        for &code in EventCode::ALL {
            assert_eq!(EventCode::from_u8(code as u8), Some(code));
        }
        assert_eq!(EventCode::RoomInit as u8, 202);
        assert_eq!(EventCode::Join as u8, 255);
        for free in 239..=247 {
            assert_eq!(
                EventCode::from_u8(free),
                None,
                "code {free} must stay free for mods"
            );
        }
    }
}
