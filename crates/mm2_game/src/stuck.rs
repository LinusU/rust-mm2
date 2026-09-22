//! Stuck-detection contract (F05-B.2) — authored `vehstuck` consumption.
//!
//! MM2Hook's recovered `vehStuck` struct (R4,
//! `src/modules/vehicle/stuck.h`) shows what the original tracks: an
//! `m_State` machine accumulating `m_StuckTime` against `m_TimeThresh`
//! while the car stays near `m_LastImpactPos` — `m_PosThresh` and
//! `m_MoveThresh` are kept beside their pre-squared copies
//! (`ComputeConstants`), so both bound distance, and
//! `m_Turn`/`m_Rotation`/`m_Translation` sit next to them with
//! `m_InertialCSPtr` supplying the pose. `Update()` itself is a binary
//! thunk, so the exact test combination stays inferred (UNK-13); this
//! module implements the disclosed designed interpretation documented
//! in `docs/research/damage.md`:
//!
//! - an [`ImpactEvent`] arms the detector at the pose the car was in —
//!   the `m_LastImpactPos` anchor. Every new impact re-anchors and
//!   restarts the window: the question is always "couldn't get away
//!   from *this* hit";
//! - inside `pos_thresh` of the anchor the episode accrues; past
//!   `move_thresh` the car demonstrably escaped and the detector
//!   disarms (`move_thresh > pos_thresh` on every retail record — the
//!   pair reads as a hysteresis band);
//! - a pose that has rotated more than `turn` since the anchor is
//!   still tumbling, not stuck — the rotation leg re-anchors on it so
//!   the settle window only starts once the car stops spinning;
//! - `time_thresh` accrued inside the bound fires [`StuckVerdict::Stuck`]
//!   once — the detector disarms until the next impact, so the
//!   [`StuckEvent`] stream stays bounded.
//!
//! `rotation` (0 on every retail record) and `translation` (≈ 0.1)
//! decode verbatim into [`StuckSpec`] but are not consumed: the
//! recovered struct gives their names, not their tests, and no
//! defensible role is measurable yet (UNK-13). What the session does
//! with a fired detection — an in-place upright reset here, mode
//! policy for anything harsher — is the resolver's choice, not this
//! component's.

use bevy::prelude::*;
use mm2_formats::veh::VehStuck;

use crate::ids::ObjectId;

/// The authored stuck thresholds for one vehicle — `vehstuck`
/// (`tune/vehicle/<id>.vehstuck`), verbatim. 20 retail records share
/// the uniform six-field set (DMG-7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StuckSpec {
    /// `TimeThresh` — seconds the stuck condition must persist before
    /// the detector fires; ~1.0 on most retail records, 2.0 on six
    /// (`vpddbus` authors 1.1714).
    pub time_thresh: f32,
    /// `PosThresh` — the "hasn't moved" radius around the impact
    /// anchor, metres; 1.25 on every retail record.
    pub pos_thresh: f32,
    /// `MoveThresh` — the escape bound, metres; 1.75 on every retail
    /// record — always above `pos_thresh`, so the pair forms a
    /// hysteresis band rather than a single edge.
    pub move_thresh: f32,
    /// `Turn` — rotation bound, radians; ≈ π on half the roster
    /// (`vpford` 3.098593), 1.57 on six, and lower still on
    /// `vpbus`/`vpcentury`/`vpddbus`/`vpsemi`. Consumed as the
    /// detector's still-tumbling leg (designed reading, UNK-13): a
    /// pose rotated past `turn` since the anchor counts as moving and
    /// restarts the settle window. A non-positive `turn` disables the
    /// leg — position alone decides.
    pub turn: f32,
    /// `Rotation` — 0 on every retail record. Decoded verbatim; its
    /// role (a second angular leg? a recovery bound?) is unverified
    /// and it is not consumed (UNK-13).
    pub rotation: f32,
    /// `Translation` — ≈ 0.1 on every retail record (`vpcoop` 0.164).
    /// Decoded verbatim; reads as a small positional bound (per-sample
    /// movement floor or the recovery nudge limit) — not consumed yet
    /// (UNK-13).
    pub translation: f32,
}

impl From<&VehStuck> for StuckSpec {
    fn from(s: &VehStuck) -> Self {
        Self {
            time_thresh: s.time_thresh,
            pos_thresh: s.pos_thresh,
            move_thresh: s.move_thresh,
            turn: s.turn,
            rotation: s.rotation,
            translation: s.translation,
        }
    }
}

/// What one [`VehicleStuck::observe`] call decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StuckVerdict {
    /// Not armed — no impact has anchored the detector, or it already
    /// fired or watched the car escape.
    Free,
    /// Armed: counting inside `pos_thresh`, holding in the hysteresis
    /// band, or still tumbling past the `turn` leg.
    Watching,
    /// The pose held inside `pos_thresh` for `time_thresh` — the
    /// detection fires once and the detector disarms until the next
    /// impact.
    Stuck,
}

/// A simulated vehicle's authored stuck detector — the runtime half of
/// `vehstuck`. Spawned only when the record decoded (like
/// [`crate::VehicleDamage`], authored absence means no component, never
/// a fabricated spec). Authority-owned like the pose it watches: a
/// predicted client never declares itself stuck.
///
/// State mirrors the recovered struct: `anchor` is `m_LastImpactPos`
/// plus the matching orientation, `accrued` is `m_StuckTime`.
#[derive(Component, Debug, Clone, Copy)]
pub struct VehicleStuck {
    /// The authored bounds (`vehstuck`).
    pub spec: StuckSpec,
    anchor: Option<(Vec3, Quat)>,
    accrued: f32,
}

impl VehicleStuck {
    /// A fresh, disarmed detector bound by `spec`.
    pub fn new(spec: StuckSpec) -> Self {
        Self {
            spec,
            anchor: None,
            accrued: 0.0,
        }
    }

    /// Whether an impact currently anchors the detector.
    pub fn armed(&self) -> bool {
        self.anchor.is_some()
    }

    /// Seconds accrued inside `pos_thresh` this episode.
    pub fn accrued(&self) -> f32 {
        self.accrued
    }

    /// `m_LastImpactPos` — anchor the detector at the pose the car was
    /// in when the impact arrived. Every new impact re-anchors and
    /// restarts the window: the question is always whether the car
    /// could not get away from *this* hit.
    pub fn impact(&mut self, position: Vec3, rotation: Quat) {
        self.anchor = Some((position, rotation));
        self.accrued = 0.0;
    }

    /// Drop the anchor — session teardown and reset paths call this so
    /// a stale episode cannot fire into a fresh pose.
    pub fn disarm(&mut self) {
        self.anchor = None;
        self.accrued = 0.0;
    }

    /// Advance the detector one fixed step.
    ///
    /// - past `move_thresh` from the anchor the car escaped — disarm;
    /// - rotated past `turn` since the anchor it is still tumbling —
    ///   re-anchor the orientation on the pose it lands in and restart
    ///   the window (the *place* anchor stays the impact's). A
    ///   non-positive `turn` disables this leg;
    /// - inside `pos_thresh` the episode accrues `dt` — reaching
    ///   `time_thresh` fires [`StuckVerdict::Stuck`] once;
    /// - between `pos_thresh` and `move_thresh` the accrued time holds
    ///   without growing — the hysteresis band.
    pub fn observe(&mut self, position: Vec3, rotation: Quat, dt: f32) -> StuckVerdict {
        let Some((anchor_pos, anchor_rot)) = self.anchor else {
            return StuckVerdict::Free;
        };
        if !position.is_finite() || !rotation.is_finite() || !dt.is_finite() || dt <= 0.0 {
            return StuckVerdict::Watching;
        }
        let dist = position.distance(anchor_pos);
        if dist >= self.spec.move_thresh {
            self.disarm();
            return StuckVerdict::Free;
        }
        if self.spec.turn > 0.0 && rotation.angle_between(anchor_rot) >= self.spec.turn {
            // Still tumbling: the settle window restarts on the pose
            // it lands in — the place anchor stays the impact's.
            self.anchor = Some((anchor_pos, rotation));
            self.accrued = 0.0;
            return StuckVerdict::Watching;
        }
        if dist <= self.spec.pos_thresh {
            self.accrued += dt;
            // Fixed-step dt accumulates float error — `time_thresh`
            // worth of 1/120 steps lands a hair under the bound, so the
            // fire edge tolerates it (1e-3 s ≪ one step).
            if self.accrued + 1e-3 >= self.spec.time_thresh {
                self.disarm();
                return StuckVerdict::Stuck;
            }
        }
        StuckVerdict::Watching
    }
}

/// One fired stuck detection — the recovery resolver and a future
/// "STUCK" HUD cue consume it. Bounded: one per armed episode per
/// vehicle (the detector disarms on fire).
#[derive(Message, Debug, Clone, Copy)]
pub struct StuckEvent {
    /// The vehicle that could not leave its impact pose.
    pub object: ObjectId,
    /// Session generation the detection belongs to.
    pub generation: u64,
    /// Fixed-step session tick the detection fired on.
    pub tick: u64,
}
