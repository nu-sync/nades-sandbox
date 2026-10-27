//! The facts: what a CS2 demo records, as the `cs2-throws` format names them (docs/standalone-api.md §3). Only an id
//! and the map per demo, and per throw its tick, grenade, side and start, plus a detonation or a flight to land it,
//! are required; everything else is optional, and each feature says which facts it needs (crate::features).

use std::fmt;

/// A point in Hammer units, z up.
pub type Vec3 = [f32; 3];

/// A server tick, 64 a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Tick(pub i32);

impl Tick {
    pub const PER_SECOND: i32 = 64;

    /// Seconds from `earlier` to this tick.
    pub fn seconds_since(self, earlier: Tick) -> f64 {
        f64::from(self.0 - earlier.0) / f64::from(Self::PER_SECOND)
    }

    pub fn plus(self, ticks: i32) -> Tick {
        Tick(self.0.saturating_add(ticks))
    }
}

/// The demo header's map name, e.g. `de_inferno`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MapName(pub String);

/// A demo's id; recommended to be the `.dem` file's SHA-256 in hex, so that producers agree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DemoId(pub String);

/// A throw's id, unique in its file (the producer's own).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThrowId(pub String);

/// The sandbox's own lineup id: the group's place in the order groups were started (crate::lineups). It holds for one
/// set of facts only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineupId(pub u32);

macro_rules! text_id {
    ($($t:ty),*) => {$(
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl From<&str> for $t {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
        impl $t {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    )*};
}
text_id!(MapName, DemoId, ThrowId);

impl fmt::Display for LineupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl LineupId {
    /// The id as `Display` writes it.
    pub fn parse(text: &str) -> Option<Self> {
        text.parse().ok().map(LineupId)
    }
}

/// A grenade, from the projectile's class; a fire is a molotov or an incendiary by `m_bIsIncGrenade`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grenade {
    Smoke,
    Flash,
    He,
    Molotov,
    Incendiary,
    Decoy,
}

impl Grenade {
    pub const ALL: [Grenade; 6] = [Grenade::Smoke, Grenade::Flash, Grenade::He, Grenade::Molotov, Grenade::Incendiary, Grenade::Decoy];

    /// The format's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Grenade::Smoke => "smoke",
            Grenade::Flash => "flash",
            Grenade::He => "he",
            Grenade::Molotov => "molotov",
            Grenade::Incendiary => "incendiary",
            Grenade::Decoy => "decoy",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.name() == name)
    }

    /// A molotov or an incendiary: both start a fire.
    pub fn is_fire(self) -> bool {
        matches!(self, Grenade::Molotov | Grenade::Incendiary)
    }

    /// What groups lineups: the grenade, with a molotov and an incendiary as one (the same throw, thrown by either side).
    pub fn kind(self) -> Grenade {
        if self == Grenade::Incendiary { Grenade::Molotov } else { self }
    }
}

impl fmt::Display for Grenade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A side, from the pawn's `m_iTeamNum` (2 T, 3 CT).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    T,
    Ct,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::T => "T",
            Side::Ct => "CT",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "T" => Some(Side::T),
            "CT" => Some(Side::Ct),
            _ => None,
        }
    }

    pub fn other(self) -> Side {
        match self {
            Side::T => Side::Ct,
            Side::Ct => Side::T,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A producer's labels, `('<namespace>.<name>', value)`, in the order the producer wrote them.
pub type Tags = Vec<(String, String)>;

/// The value of the first tag named `key`.
pub fn tag<'a>(tags: &'a Tags, key: &str) -> Option<&'a str> {
    tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Demo {
    pub id: DemoId,
    pub map: MapName,
    /// Unix ms, UTC, when known.
    pub played_at: Option<i64>,
    pub file_name: Option<String>,
    pub tags: Tags,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    pub demo: DemoId,
    pub number: u32,
    /// `round_freeze_end`.
    pub freeze_end: Option<Tick>,
    /// `bomb_planted`; None when the bomb was not planted.
    pub plant: Option<Tick>,
    /// `round_end`.
    pub end: Option<Tick>,
    pub winner: Option<Side>,
}

/// A player: `m_iszPlayerName` and `m_steamID` (SteamID64), as far as known.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Player {
    pub name: Option<String>,
    pub steamid: Option<u64>,
}

/// The thrower's pawn at the throw's tick.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pawn {
    /// Its origin: the feet.
    pub origin: Vec3,
    /// Released in the air: its origin at its last tick on the ground.
    pub takeoff: Option<Vec3>,
    /// `m_angEyeAngles`, degrees, pitch positive looking down.
    pub pitch: Option<f32>,
    pub yaw: Option<f32>,
    /// `m_vecVelocity`.
    pub velocity: Option<Vec3>,
    /// `m_hGroundEntity` is set.
    pub on_ground: Option<bool>,
    /// `m_flDuckAmount`: 0 standing, 1 crouched.
    pub duck: Option<f32>,
    /// `m_bIsWalking`.
    pub walking: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Throw {
    pub id: ThrowId,
    pub demo: DemoId,
    pub round: Option<u32>,
    /// The projectile's first tick.
    pub tick: Tick,
    pub grenade: Grenade,
    /// The thrower's side.
    pub side: Side,
    pub player: Player,
    /// `m_szClanTeamname` of the thrower's team.
    pub team: Option<String>,
    /// The thrower's `m_szLastPlaceName`.
    pub place: Option<String>,
    /// `m_vInitialPosition`.
    pub start: Vec3,
    /// `m_vInitialVelocity`.
    pub velocity: Option<Vec3>,
    pub thrower: Option<Pawn>,
    /// The grenade's detonate event: its tick and position.
    pub detonation: Option<(Tick, Vec3)>,
    /// Its expiry event (`smokegrenade_expired`, `inferno_expire`).
    pub end: Option<Tick>,
    /// The flight's last point, when the throw records a flight: what lands a throw that never detonated. The whole
    /// flight is in its [`Tracks`].
    pub flight_end: Option<(Tick, Vec3)>,
    pub tags: Tags,
}

/// A throw's large parts, loaded on demand.
#[derive(Debug, Clone, PartialEq)]
pub struct Tracks {
    /// The projectile's networked positions, release first.
    pub flight: Vec<(Tick, Vec3)>,
    pub effect: Effect,
}

/// What a throw left, one variant per kind of grenade, so a smoke cannot carry flames.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Smoke(Option<SmokeVoxels>),
    Fire(Vec<Flame>),
    Flash(Vec<Blind>),
    Explosion,
    Nothing,
}

/// A smoke's voxel log as networked (decoded by crate::smoke).
#[derive(Debug, Clone, PartialEq)]
pub struct SmokeVoxels {
    /// The tick `m_nVoxelFrameDataSize` first became non-zero.
    pub first: Tick,
    /// `m_vSmokeDetonationPos`, the point the voxel grid is centred on, when recorded.
    pub origin: Option<Vec3>,
    /// `m_VoxelFrameData[0 .. m_nVoxelFrameDataSize]`, as last networked.
    pub bytes: Vec<u8>,
}

/// One slot of an inferno's arrays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flame {
    pub slot: u32,
    /// `m_firePositions[slot]`.
    pub pos: Vec3,
    /// `m_BurnNormal[slot]`.
    pub normal: Option<Vec3>,
    /// While `m_bFireIsBurning[slot]`.
    pub first: Tick,
    pub last: Tick,
}

/// A `player_blind` event.
#[derive(Debug, Clone, PartialEq)]
pub struct Blind {
    pub player: Player,
    pub side: Option<Side>,
    /// The victim's origin at the event, when known.
    pub pos: Option<Vec3>,
    /// `blind_duration`.
    pub seconds: f32,
}

/// A player's pawn at one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub tick: Tick,
    pub player: Player,
    pub side: Side,
    /// The pawn's origin.
    pub origin: Vec3,
    pub pitch: Option<f32>,
    pub yaw: Option<f32>,
    pub duck: Option<f32>,
    /// `m_lifeState`.
    pub alive: bool,
    /// `m_szLastPlaceName`.
    pub place: Option<String>,
}

/// A player through one round, for replays: alive from the round's freeze end until `died`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerTrack {
    pub player: Player,
    pub side: Side,
    /// The first tick they were dead, when they died.
    pub died: Option<Tick>,
    pub points: Vec<TrackPoint>,
}

/// Where a player stood and looked at a tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPoint {
    pub tick: Tick,
    /// The pawn's origin.
    pub origin: Vec3,
    pub pitch: f32,
    pub yaw: f32,
}

/// A map's demos, rounds and throws, without flights or effects: what grouping and the filters read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MapFacts {
    pub demos: Vec<Demo>,
    pub rounds: Vec<Round>,
    pub throws: Vec<Throw>,
}
