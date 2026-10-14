//! The neural driver inside the game: the trained policies and the
//! per-car state they are evaluated with.
//!
//! [`DrivingPolicies`] holds one network per difficulty. The shipped
//! pair (`crates/mm2_app/policies/`) is compiled into the binary; a
//! mod replaces either by providing `ai/opponent_amateur.mm2p` or
//! `ai/opponent_professional.mm2p` through the VFS, like any other
//! file. [`NeuralState`] rides on each AI car: its guide, where it
//! is on it, and the action it is holding.
//!
//! The policy decides every [`DECISION_TICKS`] physics steps and holds
//! its action in between — the cadence it was trained at. Callers run
//! [`NeuralState::drive`] once per fixed step.

use std::sync::{Arc, OnceLock};

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::Difficulty;
use mm2_vehicle::{VehicleConfig, VehicleInput};
use tracing::{info, warn};

use super::guide::Guide;
use super::observe::{
    ACT_DIM, CarProfile, Motion, Neighbour, OBS_DIM, RayHit, action_input, guide_bearing, observe,
};
use super::policy::{Policy, Scratch};
use super::unstick::Unstick;

/// Physics steps per decision — 20 Hz at the game's 60 Hz step, the
/// rate the policies are trained at.
pub const DECISION_TICKS: u8 = 3;

/// Logical VFS path of the Amateur policy.
pub const AMATEUR_PATH: &str = "ai/opponent_amateur.mm2p";
/// Logical VFS path of the Professional policy.
pub const PROFESSIONAL_PATH: &str = "ai/opponent_professional.mm2p";

/// The shipped policies, compiled in.
const BUILT_IN_AMATEUR: &[u8] = include_bytes!("../../policies/opponent_amateur.mm2p");
const BUILT_IN_PROFESSIONAL: &[u8] = include_bytes!("../../policies/opponent_professional.mm2p");

/// How far behind its last known arc position (m) a moved car is looked
/// for on its guide — past the re-anchor's longest walk back, and far
/// enough for a recovery that returns a car to an earlier anchor.
const RESEAT_BACK: f32 = 400.0;

/// The first distance (m) behind it a moved car is looked for on its
/// guide, and how near the guide it must then be to have been found.
const RESEAT_NEAR: f32 = 15.0;
/// How far behind the point it is heading for (m) a starting car is
/// looked for on its guide — past the longest authored leg — and how
/// far beyond it.
const SEAT_BACK: f32 = 250.0;
const SEAT_AHEAD: f32 = 5.0;

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

    /// The shipped pair, compiled into the binary.
    pub fn built_in() -> &'static Self {
        static BUILT_IN: OnceLock<DrivingPolicies> = OnceLock::new();
        BUILT_IN.get_or_init(|| {
            let parse = |bytes: &[u8]| {
                Arc::new(Self::parse(bytes).expect("the built-in policy matches this build"))
            };
            Self {
                amateur: parse(BUILT_IN_AMATEUR),
                professional: parse(BUILT_IN_PROFESSIONAL),
            }
        })
    }

    /// The policies for a session: each difficulty's file through the
    /// VFS when a mod provides one this build can read, else the
    /// shipped one. A mod's file trained for another observation layout
    /// is reported and passed over.
    pub fn load(vfs: &Vfs) -> Self {
        let read = |path: &str, built_in: &Arc<Policy>| -> Arc<Policy> {
            if let Ok(bytes) = vfs.read_logical(path) {
                match Self::parse(&bytes) {
                    Ok(policy) => {
                        info!(
                            path,
                            parameters = policy.parameters(),
                            "driving policy loaded"
                        );
                        return Arc::new(policy);
                    }
                    Err(e) => warn!(path, error = %e, "driving policy rejected"),
                }
            }
            built_in.clone()
        };
        let built_in = Self::built_in();
        Self {
            amateur: read(AMATEUR_PATH, &built_in.amateur),
            professional: read(PROFESSIONAL_PATH, &built_in.professional),
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

/// Mass (kg) from which a prop is something to drive round rather than
/// through. Retail parked cars weigh 725–2200 kg, street lights and
/// telephone poles 640–950 and trees 450–1260 — London's walled
/// street trees are the lightest, and a line that grazed one at a
/// corner wedged car after car against it. Under 400 are phone boxes,
/// awnings, dumpsters and the checkpoint markers themselves.
pub const HEAVY_PROP: f32 = 400.0;

/// Progress (m) past its best that counts as a car getting somewhere.
const PROGRESS_STEP: f32 = 2.0;

/// Guide distance (m) between two re-anchors within which the second
/// is the first one's predicament again.
pub const REANCHOR_SAME: f32 = 30.0;

/// One car's neural-driver state.
#[derive(Debug, Clone)]
pub struct NeuralState {
    /// The line this car follows.
    pub guide: Arc<Guide>,
    /// Its arc position on the guide.
    pub arc: f32,
    /// Its arc position when it started.
    start: f32,
    /// Net metres of guide driven since the start — negative moves
    /// (reversing, a re-anchor's walk back) included.
    pub driven: f32,
    /// Where the car was when `arc` was last taken.
    last_pos: Vec3,
    profile: CarProfile,
    /// The action being held.
    action: [f32; ACT_DIM],
    /// Physics steps left before the next decision.
    hold: u8,
    /// The stalled-car watch that backs it out of a dead end.
    pub unstick: Unstick,
    /// Whether the guide has been laid on the world's ground
    /// ([`NeuralState::ground`]).
    grounded: bool,
    /// Where on the guide the car was the last time it was
    /// re-anchored.
    last_reanchor: Option<f32>,
    /// The furthest its progress has reached.
    best: f32,
    /// Steps since `best` last grew.
    idle: u32,
}

impl NeuralState {
    /// State for a car of `config` starting at `pos` on `guide`,
    /// heading for the guide's source point `next` — which says which
    /// pass of a street the car is on where the guide runs it twice.
    pub fn new(guide: Arc<Guide>, config: &VehicleConfig, pos: Vec3, next: usize) -> Self {
        let arc = guide.locate_within(guide.source_arc(next), pos, SEAT_BACK, SEAT_AHEAD);
        Self {
            arc,
            start: arc,
            driven: 0.0,
            last_pos: pos,
            guide,
            profile: CarProfile::of(config),
            action: [0.0; ACT_DIM],
            hold: 0,
            unstick: Unstick::default(),
            grounded: false,
            last_reanchor: None,
            best: arc,
            idle: 0,
        }
    }

    /// Lay the guide on the world's ground, once
    /// ([`Guide::regrounded`]). The guide is built when the car is
    /// spawned, before the collision world can be asked anything; this
    /// is called every step until `cast` — a ray straight down, as
    /// [`ground_near`](super::guide::ground_near) wants it — finds
    /// ground under the car at `pos` itself, which is when the world
    /// has arrived. A car over a void waits; a line corrected nowhere
    /// costs one pass of rays.
    ///
    /// The same pass moves the line off what stands on it
    /// ([`Guide::cleared`]): `ray` looks along the way for things a
    /// car could not push aside. Returns whether the pass ran, so a
    /// caller with a field of cars can spread them over several steps.
    pub fn ground(
        &mut self,
        pos: Vec3,
        mut down: impl FnMut(Vec3, f32) -> Option<(f32, bool)>,
        ray: impl FnMut(Vec3, Vec3, f32) -> Option<(f32, Vec3)>,
    ) -> bool {
        if self.grounded || super::guide::ground_near(pos, &mut down).is_none() {
            return false;
        }
        self.guide = Arc::new(super::guide::laid_on_world(&self.guide, down, ray));
        self.grounded = true;
        true
    }

    /// Whether [`NeuralState::ground`] has run.
    pub fn is_grounded(&self) -> bool {
        self.grounded
    }

    /// This car as another driver sees it.
    pub fn half_extents(&self) -> (f32, f32) {
        (self.profile.half_width, self.profile.half_length)
    }

    /// The car's progress as an arc position that never wraps: where
    /// it started plus what it has driven. On a closed guide each lap
    /// adds the guide's length — the measure a
    /// [`RouteGateLine`](mm2_game::RouteGateLine) bound from this guide
    /// counts gates and laps in.
    pub fn progress(&self) -> f32 {
        self.start + self.driven
    }

    /// Follow the car along its guide for one step; returns the metres
    /// of guide left (`None` on a closed guide). The car's place moves
    /// only as far as the car did ([`Guide::follow`]).
    pub fn track(&mut self, pos: Vec3) -> Option<f32> {
        let arc = self
            .guide
            .follow(self.arc, pos.distance(self.last_pos), pos);
        self.driven += self.guide.delta(self.arc, arc);
        self.arc = arc;
        self.last_pos = pos;
        if self.progress() > self.best + PROGRESS_STEP {
            self.best = self.progress();
            self.idle = 0;
        } else {
            self.idle = self.idle.saturating_add(1);
        }
        self.guide.remaining(self.arc)
    }

    /// Tracked steps since the car last got further along its guide
    /// than it had ever been. A car can be busy and going nowhere:
    /// knocked a street away from its line, where its place on the
    /// guide stands still ([`Guide::follow`]) and it circles the
    /// block looking for it.
    pub fn idle_steps(&self) -> u32 {
        self.idle
    }

    /// Start the idle count again — after a re-anchor, which is the
    /// answer to it. Any other reset leaves it running: a car that
    /// keeps being righted where it stands is still going nowhere.
    pub fn restart_idle(&mut self) {
        self.idle = 0;
    }

    /// Note a re-anchor at the car's place, and say whether the last
    /// one was at the same place — within [`REANCHOR_SAME`] of guide.
    /// Whatever stopped the car then is still there, and still in the
    /// way.
    pub fn reanchored_here_before(&mut self) -> bool {
        let before = self
            .last_reanchor
            .is_some_and(|last| self.guide.delta(last, self.arc).abs() <= REANCHOR_SAME);
        self.last_reanchor = Some(self.arc);
        before
    }

    /// Put the car's place at `arc`, ahead of where it was: a
    /// re-anchor that lands it past what it could not get round
    /// (`opponents::opponent_drive`). The guide between counts as
    /// driven — the one case in which being moved takes a car along
    /// its route — so the gates on it are not left behind uncleared.
    pub fn pass_to(&mut self, arc: f32, pos: Vec3) {
        self.driven += self.guide.delta(self.arc, arc);
        self.arc = arc;
        self.last_pos = pos;
        self.action = [0.0; ACT_DIM];
        self.hold = 0;
        self.unstick = Unstick::default();
        self.idle = 0;
    }

    /// Find the car on its guide again after it was moved rather than
    /// drove — a re-anchor, a recovery from the water — and forget the
    /// action it was holding. It is looked for only *behind* where it
    /// was: being moved never puts a car further along its route.
    pub fn reseat(&mut self, pos: Vec3) {
        // Nearest first: where a guide runs a street twice, the pass
        // the car was on is the one just behind it, not the one a lap
        // of the block earlier that lies just as close.
        let mut arc = self.arc;
        for back in [RESEAT_NEAR, RESEAT_NEAR * 4.0, RESEAT_BACK] {
            arc = self.guide.locate_within(self.arc, pos, back, 0.0);
            if self.guide.point_at(arc).distance(pos) <= RESEAT_NEAR {
                break;
            }
        }
        self.driven += self.guide.delta(self.arc, arc);
        self.arc = arc;
        self.last_pos = pos;
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
            let bearing = guide_bearing(motion, &self.guide, self.arc);
            self.action = self
                .unstick
                .step(motion.position, &action, bearing)
                .unwrap_or(action);
            self.hold = DECISION_TICKS;
        }
        self.hold -= 1;
        action_input(&self.action, throttle_cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_policies_match_this_builds_layout() {
        for bytes in [BUILT_IN_AMATEUR, BUILT_IN_PROFESSIONAL] {
            let policy = DrivingPolicies::parse(bytes).expect("a current policy file");
            assert_eq!(policy.obs_dim(), OBS_DIM);
            assert_eq!(policy.act_dim(), ACT_DIM);
        }
    }

    #[test]
    fn a_session_without_policy_files_drives_on_the_built_in_ones() {
        let policies = DrivingPolicies::load(&Vfs::new());
        for difficulty in [Difficulty::Amateur, Difficulty::Professional] {
            assert_eq!(policies.for_difficulty(difficulty).obs_dim(), OBS_DIM);
        }
    }
}
