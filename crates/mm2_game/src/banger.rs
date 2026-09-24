//! Knockable-prop ("banger") state contract (F04).
//!
//! A *banger* is a stamped world prop that has a bound
//! `tune/banger/<name>.dgbangerdata` record — the binding verified by
//! `mm2-inspect banger-bind` (WLD-16): pathset and prop-rule placements
//! bind by name; of the INST channel only the `*_ai.inst` stop-sign
//! supplements bind (`sp_stop_f`), ordinary INST names do not. The
//! runtime model follows the structure recovered from MM2Hook (R4,
//! `docs/research/banger.md`):
//!
//! ```text
//!   Dormant (dgUnhitBangerInstance, static collider)
//!     ── impact above the authored threshold ──▶
//!   ┌ Active  (dgBangerActive, pooled dynamic body)
//!   │   ── body sleeps / pool reclaims the slot ──▶
//!   │ Settled (dgHitBangerInstance, static again at its rest pose)
//!   └ Broken  (a NumParts prop shatters into its BREAK<NN> chunks)
//! ```
//!
//! What the original compares `ImpulseLimit2` against is unverified
//! (UNK-22), as are the pool-reclaim order and the exact fragment
//! semantics — those stay provisional policy here: the estimate is
//! the striker's kinetic energy `½·m·v²` (the authored limit ladder
//! is quadratic in speed — a linear `m·v` estimate leaves every
//! authored-breakable tree and pole unreachable), the applied kick
//! leaves the prop at the transfer's launch speed, and a prop whose
//! PKG carries authored `BREAK<NN>` chunks shatters into them at
//! activation. The active-pool
//! bound (32) is the R4-recovered `dgBangerActiveManager` size; spawned
//! fragments count against it. `Settled`/`Broken` are terminal for the
//! session: the recovered `dgHitBangerInstance` has no further
//! transition, so a knocked-down or shattered prop stays that way until
//! the session restamps it.

use bevy::prelude::*;
use mm2_formats::banger::BangerData;

use crate::ids::ObjectId;

/// The R4-recovered `dgBangerActiveManager` pool size: at most this
/// many bangers are dynamic at once. Reclaim order is unverified —
/// [`crate::banger`] systems reclaim oldest-activated first.
pub const DEFAULT_ACTIVE_POOL: usize = 32;

/// Solver-level speed bound carried by every banger body, m/s
/// (implementation choice — the records carry no speed limit). A
/// legitimate transfer launch cannot exceed ~(1+e)·striker speed —
/// under ~180 m/s even for the fastest stock car — so 200 only clips
/// solver runaway like the sf-8 fragment cascade, where a
/// spin-inflated contact severity fed the transfer and compounded to
/// ~4×10⁶ m/s across break-piece generations.
pub const MAX_BANGER_LINEAR_SPEED: f32 = 200.0;

/// Angular companion of [`MAX_BANGER_LINEAR_SPEED`], rad/s — also the
/// bound [`BangerDefinition::angular_kick`] clamps to. The kick's
/// cuboid inertia estimate is tiny on small break pieces, so a real
/// hit can produce ω far past 10⁵ rad/s; that spin then re-enters a
/// later contact's `normal_speed` as inflated approach speed —
/// the cross-generation amplifier the sf-8 cascade rode. ~10 rev/s is
/// already a visual blur on a prop.
pub const MAX_BANGER_ANGULAR_SPEED: f32 = 60.0;

/// Lifecycle of one bound banger placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BangerPhase {
    /// The untouched placement: a static collider, the R4
    /// `dgUnhitBangerInstance` state.
    Dormant,
    /// Struck and converted to a dynamic body: the pooled
    /// `dgBangerActive` state.
    Active,
    /// Came to rest (or had its pool slot reclaimed): static again at
    /// its new pose, the R4 `dgHitBangerInstance` state. Terminal for
    /// the session.
    Settled,
    /// Shattered into its authored `BREAK<NN>` fragment chunks at
    /// activation: the placement's collider and unified mesh are gone,
    /// replaced by spawned fragment bodies. Terminal for the session —
    /// the broken placement remains only as the identity the break
    /// event belongs to. Provisional timing: R4 does not settle whether
    /// fragments spawn at activation or at a later break threshold
    /// (UNK-22), so this slice breaks on the activation edge.
    Broken,
}

impl BangerPhase {
    /// Stable lowercase name for logs and smoke records.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dormant => "dormant",
            Self::Active => "active",
            Self::Settled => "settled",
            Self::Broken => "broken",
        }
    }
}

/// The runtime parameters a stamped placement needs from its bound
/// `dgBangerData` record. The same distilled shape serves fragment
/// (`<name>_break<NN>`) records — one per `BREAK<NN>` chunk the prop's
/// PKG carries. Fields the record keeps for features that do not exist
/// yet (birth-rule particles, glows, ids) are not distilled here — the
/// parsed record remains the source of truth for that work.
#[derive(Debug, Clone, PartialEq)]
pub struct BangerDefinition {
    /// The bound record stem (e.g. `sp_lightstreet_rt_f`) — kept for
    /// diagnostics and the replication contract.
    pub name: String,
    /// `Mass` — dynamic-body mass once activated. Non-finite or
    /// non-positive authored values collapse to `DEFAULT_MASS`.
    pub mass: f32,
    /// `Friction` — collider friction coefficient.
    pub friction: f32,
    /// `Elasticity` — collider restitution.
    pub elasticity: f32,
    /// `ImpulseLimit2` — authored activation threshold; the quantity it
    /// is compared against is UNK-22, so [`Self::activates_on`] applies
    /// the provisional estimate documented on the module (the striker's
    /// kinetic energy on the prop path). `0` activates on any
    /// approaching contact; ≈1e30 never activates.
    pub impulse_limit2: f32,
    /// `Size` — the authored bound box's full extents (metres); the
    /// bound is `cg ± size/2`. Measured on retail: `cg.y = size.y/2`
    /// on every record, so the bound's base rests on the instance
    /// origin — stamped placements put their *contact point* on the
    /// path, not their centre.
    pub size: [f32; 3],
    /// `CG` — the bound box's centre in prop-local space (also the
    /// authored centre of gravity). PKG geometry is authored centred
    /// at that centre, so stamping offsets content by `+CG`.
    pub cg: [f32; 3],
    /// `NumParts` — authored break-fragment count. Carried for
    /// diagnostics; fragment spawning is driven by the `BREAK<NN>`
    /// chunks the prop's PKG actually contains (the two agree on every
    /// standalone retail record — the audit cross-checks them).
    pub num_parts: i64,
    /// `AudioId` — the authored impact-sound selector: the
    /// `default_impacts.csv` category `ID` this prop claims when struck
    /// (0 on every retail record, so retail props all read the id-0
    /// `WALL` category — the binding itself is unverified, UNK-25).
    /// Kept verbatim so impact audio and mods read the authored value.
    pub audio_id: i64,
}

/// Fallback mass for records whose `Mass` is non-finite or
/// non-positive — a designed default (no authored equivalent exists).
pub const DEFAULT_MASS: f32 = 50.0;

impl BangerDefinition {
    /// Distil a parsed `dgBangerData` record into runtime parameters.
    /// Authored anomalies `BangerData::validate` reports stay the
    /// audit's business — this only keeps the runtime values usable:
    /// non-finite/negative physicals collapse to documented defaults
    /// rather than poisoning the solver.
    pub fn from_record(name: impl Into<String>, data: &BangerData) -> Self {
        let mass = if data.mass.is_finite() && data.mass > 0.0 {
            data.mass
        } else {
            DEFAULT_MASS
        };
        let friction = if data.friction.is_finite() && data.friction >= 0.0 {
            data.friction
        } else {
            0.9
        };
        let elasticity = if data.elasticity.is_finite() && data.elasticity >= 0.0 {
            data.elasticity
        } else {
            0.5
        };
        let impulse_limit2 = if data.impulse_limit2.is_finite() && data.impulse_limit2 >= 0.0 {
            data.impulse_limit2
        } else {
            0.0
        };
        let clean = |v: [f32; 3]| v.map(|c| if c.is_finite() { c } else { 0.0 });
        Self {
            name: name.into(),
            mass,
            friction,
            elasticity,
            impulse_limit2,
            size: clean(data.size),
            cg: clean(data.cg),
            num_parts: data.num_parts,
            audio_id: data.audio_id,
        }
    }

    /// Provisional activation rule (UNK-22): `estimate` is the
    /// caller's impact measure — striker kinetic energy `½·m·v²` in
    /// joules on the prop path (the authored limit ladder is quadratic
    /// in speed; the vehicle breakaway rig keeps its own linear
    /// reading under UNK-13); the banger activates when it exceeds
    /// the authored limit. `0` (most retail props) activates on any
    /// approaching contact; ≈1e30 (bridge gates, monuments) never
    /// activates.
    pub fn activates_on(&self, estimate: f32) -> bool {
        estimate > self.impulse_limit2
    }

    /// The spin kick a point impulse adds, via the solid-cuboid
    /// inertia estimate of the authored `Size` bound (full extents —
    /// verified on retail: `cg.y = size.y/2`). Provisional feel — the
    /// record carries no inertia field, so this derives one instead of
    /// applying a fixed spin. Returns zero for degenerate inputs
    /// rather than NaNs.
    pub fn angular_kick(&self, lever: Vec3, impulse: Vec3) -> Vec3 {
        let dims = Vec3::from(self.size.map(|h| h.abs().max(0.01)));
        let m = self.mass.max(0.001);
        let inertia = Vec3::new(
            m * (dims.y * dims.y + dims.z * dims.z) / 12.0,
            m * (dims.x * dims.x + dims.z * dims.z) / 12.0,
            m * (dims.x * dims.x + dims.y * dims.y) / 12.0,
        );
        let torque = lever.cross(impulse);
        // Bound the kick: the cuboid inertia shrinks quadratically on
        // small pieces, and an unbounded spin re-enters a later
        // contact's approach speed as free energy (sf-8 cascade).
        let w = (torque / inertia).clamp_length_max(MAX_BANGER_ANGULAR_SPEED);
        if w.is_finite() { w } else { Vec3::ZERO }
    }
}

/// A stamped prop placement bound to a banger record — the state
/// machine component the app systems drive. Spawns `Dormant`.
#[derive(Component, Debug)]
pub struct Banger {
    /// Current lifecycle phase.
    pub phase: BangerPhase,
    /// Distilled authored parameters.
    pub def: BangerDefinition,
    /// Session tick the prop activated on — the pool's age ordering.
    pub activated: Option<u64>,
}

impl Banger {
    /// A fresh dormant placement.
    pub fn new(def: BangerDefinition) -> Self {
        Self {
            phase: BangerPhase::Dormant,
            def,
            activated: None,
        }
    }
}

/// Why a [`BangerStateChanged`] fired.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BangerCause {
    /// An impact exceeded the authored activation threshold. Carries
    /// the contact's approach speed (m/s) and the impact measure
    /// (joules — the striker's kinetic energy) that `ImpulseLimit2`
    /// was compared against — the provisional UNK-22 quantity.
    Impact { severity: f32, estimate: f32 },
    /// The dynamic body fell asleep — it settles where it lies.
    Slept,
    /// The active-pool bound reclaimed this prop's slot for a newer
    /// activation.
    Reclaimed,
}

/// One banger lifecycle transition — the semantic stream
/// audio/particle/replication consumers read instead of watching
/// physics. Deduplicated by construction: every transition moves the
/// phase, so each fires at most once per cause (AC06's deterministic-
/// id contract rides the participant's `ObjectId`).
#[derive(Message, Debug, Clone, Copy)]
pub struct BangerStateChanged {
    /// Stable identity of the banger placement.
    pub object: ObjectId,
    /// Session generation the transition belongs to.
    pub generation: u64,
    /// Fixed-step session tick it happened on.
    pub tick: u64,
    /// The phase entered.
    pub phase: BangerPhase,
    /// What drove the transition.
    pub cause: BangerCause,
}

/// Bound on simultaneously dynamic bangers — the recovered
/// `dgBangerActiveManager` pool. Session-scoped: cleared on teardown
/// like the other session resources, so a new session counts from
/// zero.
#[derive(Resource, Debug, Clone, Copy)]
pub struct BangerPool {
    /// Maximum [`BangerPhase::Active`] placements at once.
    pub max_active: usize,
}

impl Default for BangerPool {
    fn default() -> Self {
        Self {
            max_active: DEFAULT_ACTIVE_POOL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(mass: f32, limit: f32) -> BangerData {
        BangerData {
            audio_id: 0,
            size: [0.5, 1.0, 0.5],
            cg: [0.0, 0.5, 0.0],
            num_glows: None,
            glows: Vec::new(),
            mass,
            elasticity: 0.5,
            friction: 0.9,
            impulse_limit2: limit,
            spin_axis: 0,
            flash: 0,
            num_parts: 0,
            birth_rule: None,
            tex_number: 0,
            bill_flags: 0,
            y_radius: 0.0,
            collider_id: None,
            collision_prim: None,
            collision_type: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn limit_zero_activates_on_any_approach() {
        let def = BangerDefinition::from_record("sp_test_f", &record(50.0, 0.0));
        assert!(def.activates_on(1e-6));
        assert!(def.activates_on(30_000.0));
        // A resting overlap (no approach speed) does not activate.
        assert!(!def.activates_on(0.0));
    }

    #[test]
    fn huge_limits_never_activate() {
        let def = BangerDefinition::from_record("giz_bridge_l", &record(7.26e7, 1e30));
        assert!(!def.activates_on(1e6));
        assert!(!def.activates_on(1e29));
    }

    #[test]
    fn midrange_limits_discriminate() {
        let def = BangerDefinition::from_record("sp_test_f", &record(50.0, 100.0));
        assert!(!def.activates_on(100.0), "at the limit does not exceed");
        assert!(def.activates_on(100.1));
    }

    #[test]
    fn from_record_sanitizes_authored_junk() {
        let mut r = record(f32::NAN, f32::INFINITY);
        r.friction = -1.0;
        r.elasticity = f32::NAN;
        r.size = [f32::NAN, 1.0, 1.0];
        r.cg = [0.0, f32::INFINITY, 0.0];
        let def = BangerDefinition::from_record("bad", &r);
        assert_eq!(def.mass, DEFAULT_MASS);
        assert_eq!(def.friction, 0.9);
        assert_eq!(def.elasticity, 0.5);
        assert_eq!(def.impulse_limit2, 0.0);
        assert_eq!(def.size, [0.0, 1.0, 1.0]);
        assert_eq!(def.cg, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn angular_kick_is_finite_and_reasonable() {
        let def = BangerDefinition::from_record("sp_test_f", &record(50.0, 0.0));
        // A forward impulse applied at the prop's top spins it around X.
        let w = def.angular_kick(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, -50.0));
        assert!(w.is_finite());
        assert!(w.x.abs() > 0.1, "a top-edge hit should tumble, got {w:?}");
        // Degenerate inputs yield zero, not NaN.
        assert_eq!(def.angular_kick(Vec3::ZERO, Vec3::ZERO), Vec3::ZERO);
    }
}
