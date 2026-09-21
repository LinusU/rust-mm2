//! Typed session configuration (F01-A).
//!
//! Everything needed to start a session lives in one `SessionConfig`
//! instead of loose CLI-derived resources. Fields that would be
//! progression- or network-legal are typed here; local developer
//! overrides (`--vehicle-config`, `--cam`) are quarantined in
//! [`DevOverrides`] so they cannot leak into progression or protocol
//! decisions by accident.
//!
//! Classification notes (`docs/original-rules.md`): weather and
//! time-of-day are authored *selectors* 0-3 (WLD-4) whose index→name
//! mapping is unverified (UNK-1), so the types below model the selector,
//! not invented names.

use std::path::PathBuf;

use bevy::prelude::Vec3;
use mm2_formats::racedata::{EventRow, EventTable, RaceParams};

use crate::WorldMode;

/// Top-level session configuration; `Session::begin` validates it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    /// Which world to load — synthetic dev world or a VFS city.
    pub world: WorldMode,
    /// Free roam or a cataloged authored event.
    pub mode: SessionMode,
    /// Amateur/Professional — the driver's rank (DRV-2/DRV-3).
    pub difficulty: Difficulty,
    /// Weather and time-of-day selectors.
    pub conditions: SessionConditions,
    /// Ambient traffic / pedestrian densities.
    pub densities: Densities,
    /// Deterministic seed for session-level randomness. Typed now so
    /// consumers (traffic, opponents, event shuffles) can share it
    /// instead of each rolling their own entropy source.
    pub seed: u64,
    /// The player's vehicle and paint.
    pub vehicle: VehicleSelection,
    /// Who owns game-rule authority for the session.
    pub authority: SessionAuthority,
    /// Mod content was mounted for this session (`--mods`). Mods are
    /// legitimate content, but a result produced under them is not
    /// comparable to a stock-content record — progression eligibility
    /// treats a modded session conservatively until F29 can classify
    /// per-mod impact (designed policy, not an original rule).
    pub mods_active: bool,
    /// Local-only developer overrides — never progression- or
    /// network-legal.
    pub dev: DevOverrides,
}

impl Default for SessionConfig {
    /// A zero-content session: dev world, cruise, amateur, local
    /// authority, no vehicle request, no developer overrides.
    fn default() -> Self {
        Self {
            world: WorldMode::DevWorld,
            mode: SessionMode::Cruise,
            difficulty: Difficulty::Amateur,
            conditions: SessionConditions::default(),
            densities: Densities::default(),
            seed: 0,
            vehicle: VehicleSelection::default(),
            authority: SessionAuthority::Local,
            mods_active: false,
            dev: DevOverrides::default(),
        }
    }
}

impl SessionConfig {
    /// Reject configurations that would fail or misbehave at load time.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.densities.validate()?;
        if let WorldMode::City { psdl } = &self.world
            && psdl.trim().is_empty()
        {
            return Err(ConfigError::EmptyCityPath);
        }
        if self
            .vehicle
            .id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err(ConfigError::EmptyVehicleId);
        }
        Ok(())
    }
}

/// A rejected `SessionConfig` field.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// A density fraction outside `0..=1` or non-finite.
    Density {
        /// Which density field overflowed.
        field: &'static str,
        /// The rejected value.
        value: f32,
    },
    /// `WorldMode::City` with a blank logical path.
    EmptyCityPath,
    /// A vehicle id that is present but blank.
    EmptyVehicleId,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Density { field, value } => {
                write!(f, "{field} density {value} is outside 0..=1")
            }
            Self::EmptyCityPath => write!(f, "city world has an empty logical path"),
            Self::EmptyVehicleId => write!(f, "vehicle id is empty"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// What kind of session the player is in.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SessionMode {
    /// Free roam — no opponents, no clock (CRZ-1). The only mode with a
    /// runtime today.
    #[default]
    Cruise,
    /// A cataloged authored event; the F11 event catalog resolves the
    /// reference into checkpoints, opponents and rules.
    Event(EventRef),
}

/// Identity of one authored event: a row in one city's `mm*data.csv`
/// table. This is the data model the shipped tables actually use —
/// content-driven, so mods adding a city or table rows stay referable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventRef {
    /// City stem the event table lives under (`london`, `sf`, or a
    /// mod-provided city). Deliberately a string, not a closed enum.
    pub city: String,
    /// Which `mm*data.csv` table the event is a row of.
    pub table: EventTableKind,
    /// 0-based row within the table, authored order.
    pub index: usize,
}

impl EventRef {
    /// Logical VFS path of the table file, e.g.
    /// `race/london/mmblitzdata.csv`.
    pub fn table_path(&self) -> String {
        format!("race/{}/{}", self.city, self.table.file_name())
    }

    /// The event's row inside a parsed table; `None` when `index` is out
    /// of bounds.
    pub fn row<'a>(&self, table: &'a EventTable) -> Option<&'a EventRow> {
        table.rows.get(self.index)
    }
}

/// The four authored `mm*data.csv` event tables each stock city ships
/// (RACE-1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum EventTableKind {
    /// `mmblitzdata.csv` — solo checkpoint hunts against the clock.
    Blitz,
    /// `mmracedata.csv` — checkpoint races against opponents.
    Checkpoint,
    /// `mmcircuitdata.csv` — lapped races.
    Circuit,
    /// `mmcrashdata.csv` — Crash Course lessons/midterms/finals.
    CrashCourse,
}

impl EventTableKind {
    /// The authored file name inside `race/<city>/`.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Blitz => "mmblitzdata.csv",
            Self::Checkpoint => "mmracedata.csv",
            Self::Circuit => "mmcircuitdata.csv",
            Self::CrashCourse => "mmcrashdata.csv",
        }
    }

    /// The file-stem prefix the table's rows map to (`blitz3`,
    /// `race0`, `circuit5`, `crash9`) — the inferred `<prefix><index>`
    /// convention the event catalog uses.
    pub fn stem_prefix(self) -> &'static str {
        match self {
            Self::Blitz => "blitz",
            Self::Checkpoint => "race",
            Self::Circuit => "circuit",
            Self::CrashCourse => "crash",
        }
    }

    /// The `RaceType` token a `<city>_rewards.csv` row uses for this
    /// family — `race` names the Checkpoint table, matching the file
    /// stem prefix.
    pub fn reward_token(self) -> &'static str {
        match self {
            Self::Blitz => "blitz",
            Self::Checkpoint => "race",
            Self::Circuit => "circuit",
            Self::CrashCourse => "crash",
        }
    }

    /// Reverse of [`reward_token`](Self::reward_token) — `None` for a
    /// `RaceType` token no family claims (kept raw by producers as a
    /// diagnostic, never silently attached).
    pub fn from_reward_token(token: &str) -> Option<Self> {
        [
            Self::Blitz,
            Self::Checkpoint,
            Self::Circuit,
            Self::CrashCourse,
        ]
        .into_iter()
        .find(|kind| kind.reward_token() == token)
    }
}

/// Driver rank — the authored Amateur/Professional parameter split
/// (DRV-2/DRV-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    /// First parameter block: longer limits, lighter traffic.
    #[default]
    Amateur,
    /// Second parameter block: shorter limits, denser traffic.
    Professional,
}

impl Difficulty {
    /// The parameter block this difficulty selects on an event row.
    pub fn params(self, row: &EventRow) -> &RaceParams {
        match self {
            Self::Amateur => &row.amateur,
            Self::Professional => &row.professional,
        }
    }
}

/// A time-of-day selector. Authored values are 0-3 (WLD-4); which index
/// means which time is unverified (UNK-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimeOfDay(u8);

impl TimeOfDay {
    /// Largest authored selector value.
    pub const MAX: u8 = 3;

    pub fn new(value: u8) -> Result<Self, SelectorError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(SelectorError {
                name: "time-of-day",
                value,
            })
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

/// A weather selector. Authored values are 0-3 (WLD-4); which index
/// means which weather is unverified (UNK-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Weather(u8);

impl Weather {
    /// Largest authored selector value.
    pub const MAX: u8 = 3;

    pub fn new(value: u8) -> Result<Self, SelectorError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(SelectorError {
                name: "weather",
                value,
            })
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

/// A condition selector outside the authored 0-3 range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectorError {
    /// Which selector overflowed.
    pub name: &'static str,
    /// The rejected value.
    pub value: u8,
}

impl std::fmt::Display for SelectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} selector {} is outside 0-3", self.name, self.value)
    }
}

impl std::error::Error for SelectorError {}

/// The session's weather and time-of-day pair. Defaults to selector 0
/// for both — index 0's meaning is unverified, so this is a neutral
/// default, not a claim about what the original shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionConditions {
    pub time_of_day: TimeOfDay,
    pub weather: Weather,
}

/// Ambient population densities, authored per event as 0-1 fractions
/// (WLD-1: `mm*data.csv` Ambient/Peds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Densities {
    /// Ambient traffic density.
    pub traffic: f32,
    /// Pedestrian density.
    pub pedestrians: f32,
}

impl Densities {
    /// Designed default — no authored source; authored events override
    /// it (and Circuit rows force pedestrians to 0, CIR-3).
    pub const DEFAULT: Self = Self {
        traffic: 0.5,
        pedestrians: 0.5,
    };

    /// Densities are fractions; anything outside `0..=1` or non-finite
    /// is a config error, not something to clamp silently.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, value) in [("traffic", self.traffic), ("pedestrians", self.pedestrians)] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(ConfigError::Density { field, value });
            }
        }
        Ok(())
    }
}

impl Default for Densities {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The player's chosen vehicle.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VehicleSelection {
    /// Catalog id (`vpbug`); `None` = synthetic dev car.
    pub id: Option<String>,
    /// Zero-based paint index.
    pub paint: usize,
}

/// Who owns the session's game-rule authority. `Local` is the only
/// authority a session can have today; the networked variants exist so
/// rule systems are written against the contract, not against a
/// single-player assumption (F24 fills them in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionAuthority {
    /// This process runs the rules — single player and development.
    #[default]
    Local,
    /// This process hosts a networked session and stays authoritative.
    Host,
    /// A remote server is authoritative; this client predicts.
    Remote,
}

impl SessionAuthority {
    /// Single-player pause is legal; multiplayer never pauses (MP-6:
    /// "no pausing in multiplayer").
    pub fn allows_pause(self) -> bool {
        matches!(self, Self::Local)
    }

    /// Whether a session under this authority simulates its own game
    /// rules (`Local` offline and a `Host` server) rather than predicting
    /// a remote server's (`Remote`). The boundary F01-B objects are
    /// stamped with via `Session::authority_role`.
    pub fn is_authoritative(self) -> bool {
        !matches!(self, Self::Remote)
    }
}

/// A free-camera spawn pose (`--cam x,y,z[,yaw,pitch]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    pub position: Vec3,
    /// Yaw in radians.
    pub yaw: f32,
    /// Pitch in radians.
    pub pitch: f32,
}

/// A player-vehicle spawn pose (`--spawn x,y,z[,yaw]`): the
/// `Quat::from_rotation_y` convention — forward is local −Z, so yaw 0
/// drives toward −Z and yaw π/2 toward −X.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpawnPose {
    pub position: Vec3,
    /// Yaw in radians.
    pub yaw: f32,
}

/// Local developer tweaks that must never count for progression or be
/// legal in a networked session. Quarantined here — off the
/// session-legal fields — so nothing downstream confuses them with real
/// session parameters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DevOverrides {
    /// `--vehicle-config` TOML: replaces the selected car's handling.
    pub vehicle_config: Option<PathBuf>,
    /// `--cam` fixed free-camera start pose.
    pub camera: Option<CameraPose>,
    /// `--nav` BAI navigation debug overlay over an imported city
    /// (F09-B diagnostics — never a gameplay input).
    pub nav_overlay: Option<NavOverlay>,
    /// `--spawn` fixed player-vehicle start pose: replaces whatever the
    /// world or authored event slot chose (evidence/diagnostic runs —
    /// never a session-legal parameter).
    pub spawn: Option<SpawnPose>,
    /// `--banger-pool` bound on simultaneously active bangers: replaces
    /// the recovered ×32 default (evidence/diagnostic runs — never a
    /// session-legal parameter).
    pub banger_pool: Option<usize>,
    /// `--traction` fixed environment traction multiplier applied to
    /// every tire contact: a wetness/ice stand-in for evidence runs
    /// (the authored weather→wetness mapping is UNK-1, so no
    /// session-legal parameter drives this yet — F18 owns the real
    /// writer). `None`/`1.0` is unmodified.
    pub traction: Option<f32>,
}

/// Configuration of the `--nav` debug overlay: draw the city's BAI
/// navigation graph over the imported geometry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NavOverlay {
    /// Optional route probe `<from>:<to>` as BAI road indices, snapped
    /// like `mm2-inspect nav --route` and highlighted over the lanes.
    pub route: Option<(u16, u16)>,
}
