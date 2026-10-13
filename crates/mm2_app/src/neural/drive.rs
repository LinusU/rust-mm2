//! The neural driver inside the game: the trained policies and the
//! per-car state they are evaluated with.
//!
//! [`DrivingPolicies`] holds one network per difficulty, read through
//! the VFS from the app's own assets (`ai/opponent_amateur.mm2p`,
//! `ai/opponent_professional.mm2p`) so a mod can replace them like any
//! other file. [`NeuralState`] rides on each AI car: its guide, where it
//! is on it, and the action it is holding.
//!
//! The policy decides every [`DECISION_TICKS`] physics steps and holds
//! its action in between — the cadence it was trained at. Callers run
//! [`NeuralState::drive`] once per fixed step.

use std::sync::Arc;

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::Difficulty;
use mm2_vehicle::{VehicleConfig, VehicleInput};
use tracing::{info, warn};

use super::guide::Guide;
use super::observe::{
    ACT_DIM, CarProfile, Motion, Neighbour, OBS_DIM, RayHit, action_input, observe,
};
use super::policy::{Policy, Scratch};

/// Physics steps per decision — 20 Hz at the game's 60 Hz step, the
/// rate the policies are trained at.
pub const DECISION_TICKS: u8 = 3;

/// Logical VFS path of the Amateur policy.
pub const AMATEUR_PATH: &str = "ai/opponent_amateur.mm2p";
/// Logical VFS path of the Professional policy.
pub const PROFESSIONAL_PATH: &str = "ai/opponent_professional.mm2p";

/// How far behind its last known arc position (m) a moved car is looked
/// for on its guide — past the re-anchor's longest walk back.
const RESEAT_BACK: f32 = 120.0;

/// The trained driving policies, one per difficulty.
#[derive(Resource, Clone)]
pub struct DrivingPolicies {
    amateur: Arc<Policy>,
    professional: Arc<Policy>,
}

impl DrivingPolicies {
    /// Both difficulties on one network — a development override for
    /// trying a freshly trained policy.
    pub fn single(policy: Policy) -> Self {
        let policy = Arc::new(policy);
        Self {
            amateur: policy.clone(),
            professional: policy,
        }
    }

    /// Parse a `.mm2p` file against this build's observation layout.
    pub fn parse(bytes: &[u8]) -> Result<Policy, super::policy::PolicyError> {
        Policy::parse(bytes, OBS_DIM, ACT_DIM)
    }

    /// Read a policy file named on the command line.
    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&bytes)
            .map(Self::single)
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Read both policies through the VFS. A difficulty whose file is
    /// missing or stale borrows the other's; `None` when neither loads.
    pub fn load(vfs: &Vfs) -> Option<Self> {
        let read = |path: &str| -> Option<Arc<Policy>> {
            let bytes = vfs.read_logical(path).ok()?;
            match Self::parse(&bytes) {
                Ok(policy) => {
                    info!(
                        path,
                        parameters = policy.parameters(),
                        "driving policy loaded"
                    );
                    Some(Arc::new(policy))
                }
                Err(e) => {
                    warn!(path, error = %e, "driving policy rejected");
                    None
                }
            }
        };
        let amateur = read(AMATEUR_PATH);
        let professional = read(PROFESSIONAL_PATH);
        match (amateur, professional) {
            (Some(amateur), Some(professional)) => Some(Self {
                amateur,
                professional,
            }),
            (Some(only), None) | (None, Some(only)) => Some(Self {
                amateur: only.clone(),
                professional: only,
            }),
            (None, None) => None,
        }
    }

    /// The policy that drives at `difficulty`.
    pub fn for_difficulty(&self, difficulty: Difficulty) -> &Policy {
        match difficulty {
            Difficulty::Amateur => &self.amateur,
            Difficulty::Professional => &self.professional,
        }
    }
}

/// One car's neural-driver state.
#[derive(Debug, Clone)]
pub struct NeuralState {
    /// The line this car follows.
    pub guide: Arc<Guide>,
    /// Its arc position on the guide.
    pub arc: f32,
    profile: CarProfile,
    /// The action being held.
    action: [f32; ACT_DIM],
    /// Physics steps left before the next decision.
    hold: u8,
}

impl NeuralState {
    /// State for a car of `config` starting at `pos` on `guide`.
    pub fn new(guide: Arc<Guide>, config: &VehicleConfig, pos: Vec3) -> Self {
        Self {
            arc: guide.locate(pos),
            guide,
            profile: CarProfile::of(config),
            action: [0.0; ACT_DIM],
            hold: 0,
        }
    }

    /// This car as another driver sees it.
    pub fn half_extents(&self) -> (f32, f32) {
        (self.profile.half_width, self.profile.half_length)
    }

    /// Follow the car along its guide; returns the metres of guide left
    /// (`None` on a closed guide).
    pub fn track(&mut self, pos: Vec3) -> Option<f32> {
        self.arc = self.guide.advance(self.arc, pos);
        self.guide.remaining(self.arc)
    }

    /// Find the car on its guide again after it was moved (a re-anchor
    /// or reset), and forget the action it was holding.
    pub fn reseat(&mut self, pos: Vec3) {
        self.arc = self
            .guide
            .locate_within(self.arc, pos, RESEAT_BACK, RESEAT_BACK);
        self.action = [0.0; ACT_DIM];
        self.hold = 0;
    }

    /// One fixed step of driving: decide when the held action has run
    /// its course, and return the pedals to apply this step.
    #[allow(clippy::too_many_arguments)] // one decision's sources, as `observe` takes them
    pub fn drive(
        &mut self,
        policy: &Policy,
        scratch: &mut Scratch,
        motion: &Motion,
        neighbours: &[Neighbour],
        throttle_cap: f32,
        cast: impl FnMut(Vec3, Vec3, f32) -> Option<RayHit>,
    ) -> VehicleInput {
        if self.hold == 0 {
            let mut obs = [0.0; OBS_DIM];
            observe(
                &mut obs,
                &self.profile,
                motion,
                &self.guide,
                self.arc,
                &self.action,
                neighbours,
                throttle_cap,
                cast,
            );
            let mut action = [0.0; ACT_DIM];
            policy.act(&obs, &mut action, scratch);
            self.action = action;
            self.hold = DECISION_TICKS;
        }
        self.hold -= 1;
        action_input(&self.action, throttle_cap)
    }
}
