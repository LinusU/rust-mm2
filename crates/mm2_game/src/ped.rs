//! Pedestrian skeletal runtime (F19-A.3): sampling `pedanim_*.anim`
//! clips onto a `pedmodel_*.skel` rig and stepping the authored
//! `pedmodel_*.csv` state model.
//!
//! Frame layout (measured on retail `fnv1a64:e91e6cd4b2ae30d9`, matching
//! R3 `Pedestrian_animations.md`'s type-1 channel and mm2hook's
//! `crAnimFrame`/`crBone` layout): each `.anim` frame is one XYZ root
//! translation followed by `NumBones` Euler rotation triples, one per
//! skeleton bone **in `.skel` pre-order** — verified against the
//! standing clip, whose mirrored left/right arm rotations land exactly
//! on the clavicle/shoulder/elbow/wrist channel pairs. The triple
//! `(x, y, z)` composes the way `Matrix34::GetEulers` extracts it —
//! AGE row-vector form `Rx·Ry·Rz` — i.e. `Quat::from_euler(
//! EulerRot::XYZEx, x, y, z)`: a fixed-axis X then Y then Z rotation.
//! The dive-left clip's authored end pose lands the body prone and
//! aligned with the dive direction under this order; an intrinsic-XYZ
//! reading leaves it perpendicular.
//!
//! Fractional frames lerp the raw channel floats — mm2hook's
//! `crAnimFrame::Blend(fraction, first, second)` is exactly that buffer
//! shape (documented structure; its call sites are unrecovered, UNK-41).
//! Non-root bone translations are always the authored bind `offset`s;
//! the clip only carries rotations for them.
//!
//! The state machine (R3 `Pedestrian_state_models.md`): each state owns
//! a clip and an authored 1-based inclusive frame window, loops or
//! chains to a `default next` state, and transitions between states are
//! authored as `{FROM}_{TO}` clips. The original's playback rate and
//! when exactly it interrupts a running state are unrecovered — the
//! designed policies here (fixed [`PED_STATE_FPS`], requests switching
//! immediately through the authored transition when one exists, or
//! directly otherwise) are DSN-64 in the original-rules ledger.

use bevy::prelude::{EulerRot, Quat, Vec3};
use mm2_formats::ped::{PedAnim, PedSkel, PedStates};
use std::collections::HashMap;

/// Designed playback rate — animation frames per second. The original's
/// stepping rule is unrecovered (UNK-41); retail clips are sized such
/// that 30 fps puts walk/run cycle durations at plausible human cadences.
pub const PED_STATE_FPS: f32 = 30.0;

/// One flattened bone of a [`PedRig`].
#[derive(Debug, Clone)]
pub struct PedRigBone {
    /// Bone name (`root`, `spine`, `wrist_l`, …).
    pub name: String,
    /// Parent index — `None` on the single root.
    pub parent: Option<usize>,
    /// Authored bind-pose local translation (`offset` in `.skel`).
    pub bind_offset: Vec3,
}

/// A flattened skeleton: pre-order traversal of the `.skel` tree, which
/// is the order `.anim` rotation channels are authored in.
#[derive(Debug, Clone)]
pub struct PedRig {
    bones: Vec<PedRigBone>,
}

/// Why a [`PedSkel`] cannot be a [`PedRig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PedRigError {
    /// No bone records at all.
    Empty,
    /// More than one top-level bone — the clip format carries a single
    /// root channel.
    MultipleRoots(usize),
}

impl std::fmt::Display for PedRigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "skeleton has no bones"),
            Self::MultipleRoots(n) => {
                write!(
                    f,
                    "skeleton has {n} top-level bones (clip format carries one root)"
                )
            }
        }
    }
}

impl std::error::Error for PedRigError {}

/// One bone's local pose.
#[derive(Debug, Clone, Copy)]
pub struct PedBonePose {
    /// Local translation — the bind offset on non-root bones, the clip's
    /// root channel on the root.
    pub translation: Vec3,
    /// Local rotation.
    pub rotation: Quat,
}

/// A pose for every bone of a [`PedRig`], in rig order.
#[derive(Debug, Clone)]
pub struct PedPose {
    /// One entry per rig bone, same order as [`PedRig::bones`].
    pub bones: Vec<PedBonePose>,
}

impl PedPose {
    /// Every translation and quaternion component finite.
    pub fn is_finite(&self) -> bool {
        self.bones
            .iter()
            .all(|b| b.translation.is_finite() && b.rotation.is_finite())
    }

    /// Blend toward `other` by `t` — translations lerp, rotations
    /// slerp. Both poses must belong to the same rig (same bone count).
    pub fn lerp(&self, other: &PedPose, t: f32) -> PedPose {
        assert_eq!(
            self.bones.len(),
            other.bones.len(),
            "cannot blend poses of different rigs"
        );
        PedPose {
            bones: self
                .bones
                .iter()
                .zip(&other.bones)
                .map(|(a, b)| PedBonePose {
                    translation: a.translation.lerp(b.translation, t),
                    rotation: a.rotation.slerp(b.rotation, t),
                })
                .collect(),
        }
    }
}

/// Why a clip cannot be sampled onto a rig.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PedSampleError {
    /// The clip declares zero frames.
    EmptyClip,
    /// `floats_per_frame` does not carry the rig's channels: it must be
    /// `3 * (bones + 1)` (root translation + one rotation triple per
    /// bone). Extra trailing channels are ignored rather than rejected.
    FrameWidth {
        /// Required `floats_per_frame`.
        needed: usize,
        /// The clip's actual `floats_per_frame`.
        actual: u32,
    },
}

impl std::fmt::Display for PedSampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyClip => write!(f, "clip declares zero frames"),
            Self::FrameWidth { needed, actual } => {
                write!(f, "clip has {actual} floats/frame, the rig needs {needed}")
            }
        }
    }
}

impl std::error::Error for PedSampleError {}

impl PedRig {
    /// Flatten a parsed `.skel` into runtime form. Fails on empty or
    /// multi-root hierarchies.
    pub fn from_skel(skel: &PedSkel) -> Result<Self, PedRigError> {
        if skel.roots.is_empty() {
            return Err(PedRigError::Empty);
        }
        if skel.roots.len() > 1 {
            return Err(PedRigError::MultipleRoots(skel.roots.len()));
        }
        let mut bones = Vec::with_capacity(skel.bone_count());
        fn walk(b: &mm2_formats::ped::PedBone, parent: Option<usize>, out: &mut Vec<PedRigBone>) {
            let idx = out.len();
            out.push(PedRigBone {
                name: b.name.clone(),
                parent,
                bind_offset: Vec3::from_array(b.offset),
            });
            for c in &b.children {
                walk(c, Some(idx), out);
            }
        }
        walk(&skel.roots[0], None, &mut bones);
        Ok(PedRig { bones })
    }

    /// Bone count.
    pub fn bone_count(&self) -> usize {
        self.bones.len()
    }

    /// The flattened bone list, in `.skel` pre-order.
    pub fn bones(&self) -> &[PedRigBone] {
        &self.bones
    }

    /// First bone index with this name, if any.
    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }

    /// All rotations at identity, all translations at bind offsets —
    /// the T-pose the mesh was authored against.
    pub fn bind_pose(&self) -> PedPose {
        PedPose {
            bones: self
                .bones
                .iter()
                .map(|b| PedBonePose {
                    translation: b.bind_offset,
                    rotation: Quat::IDENTITY,
                })
                .collect(),
        }
    }

    /// Sample `clip` at fractional frame `frame` (0-based, clamped into
    /// `0..frames`). Channel `0` is the root's world-space translation;
    /// channel `i + 1` is bone `i`'s local Euler triple.
    pub fn sample(&self, clip: &PedAnim, frame: f32) -> Result<PedPose, PedSampleError> {
        if clip.frames == 0 {
            return Err(PedSampleError::EmptyClip);
        }
        let needed = (self.bones.len() + 1) * 3;
        let fpf = clip.floats_per_frame as usize;
        if fpf < needed || !fpf.is_multiple_of(3) {
            return Err(PedSampleError::FrameWidth {
                needed,
                actual: clip.floats_per_frame,
            });
        }
        let f = if frame.is_finite() {
            frame.clamp(0.0, (clip.frames - 1) as f32)
        } else {
            0.0
        };
        let i0 = f.floor() as u32;
        let i1 = (i0 + 1).min(clip.frames - 1);
        let t = f - i0 as f32;
        let (Some(f0), Some(f1)) = (clip.frame(i0), clip.frame(i1)) else {
            return Err(PedSampleError::EmptyClip);
        };
        let lerp = |a: &[f32], b: &[f32], k: usize| a[k] + (b[k] - a[k]) * t;
        let mut pose = self.bind_pose();
        pose.bones[0].translation = Vec3::new(lerp(f0, f1, 0), lerp(f0, f1, 1), lerp(f0, f1, 2));
        for i in 0..self.bones.len() {
            let o = 3 + i * 3;
            pose.bones[i].rotation = Quat::from_euler(
                EulerRot::XYZEx,
                lerp(f0, f1, o),
                lerp(f0, f1, o + 1),
                lerp(f0, f1, o + 2),
            );
        }
        Ok(pose)
    }

    /// Forward kinematics — world-space `(translation, rotation)` per
    /// bone. The root's clip translation is world-absolute.
    pub fn world_transforms(&self, pose: &PedPose) -> Vec<(Vec3, Quat)> {
        let mut out = vec![(Vec3::ZERO, Quat::IDENTITY); self.bones.len()];
        for (i, b) in self.bones.iter().enumerate() {
            let local = pose.bones[i];
            out[i] = match b.parent {
                Some(p) => {
                    let (pp, pr) = out[p];
                    (pp + pr * local.translation, pr * local.rotation)
                }
                None => (local.translation, local.rotation),
            };
        }
        out
    }
}

/// One runtime row of the authored state model.
#[derive(Debug, Clone)]
pub struct PedAnimState {
    /// State name (`STAND`, `WALK`, `STAND_WALK`, …).
    pub name: String,
    /// Clip stem the state plays (`anim/<clip>.anim`).
    pub clip: String,
    /// First clip frame of the window, 0-based (the csv authors 1-based;
    /// negatives clamp to 0).
    pub first_frame: u32,
    /// Last clip frame of the window, 0-based, verbatim — may exceed the
    /// clip's length on authored `frames + 1` overshoot rows; `tick`
    /// clamps it against the actual clip.
    pub last_frame: u32,
    /// `Y AXIS Offset` / `X AXIS Offset` columns, verbatim (phase
    /// bookkeeping — measured to equal the entering clip's root-channel
    /// start distance on some rows, e.g. man `WALK` 0.281).
    pub y_offset: f32,
    /// `Y AXIS DISTANCE` — authored forward travel per full window,
    /// metres (measured equal to the clip's root-channel Z travel on
    /// retail; mm2hook calls the field `FSpeed`).
    pub y_distance: f32,
    /// `X AXIS Offset` column, verbatim.
    pub x_offset: f32,
    /// `X AXIS DISTANCE` — authored lateral travel per window, metres
    /// (±2.2 on the dive rows; mm2hook's `LSpeed`).
    pub x_distance: f32,
    /// Resolved `default next` index — `None` on a dangling authored
    /// link (the csv validation already reports it as an issue).
    pub next: Option<usize>,
}

/// Why a state model cannot become a [`PedAnimator`].
#[derive(Debug, Clone, PartialEq)]
pub enum PedAnimError {
    /// The csv parsed no states.
    EmptyTable,
    /// The requested start state is not authored.
    UnknownState(String),
    /// A non-finite or non-positive playback rate.
    BadFps(f32),
}

impl std::fmt::Display for PedAnimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyTable => write!(f, "state model has no states"),
            Self::UnknownState(s) => write!(f, "no state named {s:?}"),
            Self::BadFps(v) => write!(f, "playback rate {v} is not positive and finite"),
        }
    }
}

impl std::error::Error for PedAnimError {}

/// Result of one [`PedAnimator::tick`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PedTick {
    /// State boundaries crossed (a self-loop wrap counts).
    pub boundaries: u32,
    /// The state entered this tick, when `current` changed.
    pub entered: Option<usize>,
}

/// Stepping cursor over a `pedmodel_*.csv` state model.
///
/// Owns no clips — callers resolve `current().clip` to a parsed
/// [`PedAnim`] and hand [`tick`](Self::tick) the clip's frame count so
/// authored `frames + 1` overshoot windows clamp correctly.
#[derive(Debug, Clone)]
pub struct PedAnimator {
    states: Vec<PedAnimState>,
    by_name: HashMap<String, usize>,
    current: usize,
    /// Fractional 0-based frame cursor inside the current window.
    cursor: f32,
    fps: f32,
    /// Introspection only — the requested end state once [`request`]
    /// has been honoured; cleared when reached.
    target: Option<usize>,
}

/// Authored 1-based frame number → 0-based `u32` index, saturating at
/// both ends: a hostile csv (`i64::MIN`, or a window beyond
/// `u32::MAX`) must neither overflow the rebasing subtraction nor
/// truncate into a wrong-but-in-range window. `PedStates::validate`
/// already reports such rows as issues.
fn authored_window_frame(authored: i64) -> u32 {
    authored.saturating_sub(1).clamp(0, u32::MAX as i64) as u32
}

impl PedAnimator {
    /// Build an animator over `states`, parked at `start`'s first frame.
    /// `fps` is the frame-stepping rate ([`PED_STATE_FPS`] is the
    /// designed default).
    pub fn new(states: &PedStates, start: &str, fps: f32) -> Result<Self, PedAnimError> {
        if states.states.is_empty() {
            return Err(PedAnimError::EmptyTable);
        }
        if !fps.is_finite() || fps <= 0.0 {
            return Err(PedAnimError::BadFps(fps));
        }
        let mut by_name = HashMap::with_capacity(states.states.len());
        for (i, s) in states.states.iter().enumerate() {
            by_name.entry(s.name.clone()).or_insert(i);
        }
        let mut rt = Vec::with_capacity(states.states.len());
        for s in &states.states {
            rt.push(PedAnimState {
                name: s.name.clone(),
                clip: s.anim.clone(),
                // Authored 1-based inclusive → 0-based frame indices.
                first_frame: authored_window_frame(s.first_frame),
                last_frame: authored_window_frame(s.last_frame),
                y_offset: s.y_offset,
                y_distance: s.y_distance,
                x_offset: s.x_offset,
                x_distance: s.x_distance,
                next: if s.next.is_empty() {
                    None
                } else {
                    by_name.get(&s.next).copied()
                },
            });
        }
        let Some(&current) = by_name.get(start) else {
            return Err(PedAnimError::UnknownState(start.to_string()));
        };
        let cursor = rt[current].first_frame as f32;
        Ok(PedAnimator {
            states: rt,
            by_name,
            current,
            cursor,
            fps,
            target: None,
        })
    }

    /// The current state.
    pub fn current(&self) -> &PedAnimState {
        &self.states[self.current]
    }

    /// Current state index into [`states`](Self::states).
    pub fn current_index(&self) -> usize {
        self.current
    }

    /// The fractional frame cursor — feed [`PedRig::sample`].
    pub fn frame(&self) -> f32 {
        self.cursor
    }

    /// The state a [`request`](Self::request) is travelling toward, if
    /// still in transit.
    pub fn target(&self) -> Option<&str> {
        self.target.map(|t| self.states[t].name.as_str())
    }

    /// Every runtime state row.
    pub fn states(&self) -> &[PedAnimState] {
        &self.states
    }

    /// Sample the current pose: `rig.sample(clip, self.frame())`.
    pub fn pose(&self, rig: &PedRig, clip: &PedAnim) -> Result<PedPose, PedSampleError> {
        rig.sample(clip, self.cursor)
    }

    /// Head for state `target`. When an authored `{current}_{target}`
    /// transition state exists it is entered immediately at its first
    /// frame; otherwise the target is entered directly (designed —
    /// the original's interruption timing is unrecovered). Unknown
    /// targets are refused: returns `false`, nothing changes.
    pub fn request(&mut self, target: &str) -> bool {
        let Some(&t) = self.by_name.get(target) else {
            return false;
        };
        if t == self.current {
            self.target = None;
            return true;
        }
        self.target = Some(t);
        let tr = format!("{}_{}", self.states[self.current].name, target);
        self.enter(self.by_name.get(&tr).copied().unwrap_or(t));
        if self.target == Some(self.current) {
            self.target = None;
        }
        true
    }

    /// Advance the cursor by `dt` seconds at the configured rate.
    /// `clip_frames` is the *current* state's clip length; the window's
    /// authored last frame clamps against it. Crossing the window end
    /// follows `default next` — a self-loop wraps the cursor back into
    /// the window keeping the sub-frame phase; a state change lands on
    /// the entered state's first frame.
    pub fn tick(&mut self, dt: f32, clip_frames: u32) -> PedTick {
        let mut out = PedTick::default();
        if !dt.is_finite() || dt <= 0.0 || clip_frames == 0 {
            return out;
        }
        self.cursor += dt * self.fps;
        // One boundary per state bounds degenerate empty-window chains.
        let guard = self.states.len() as u32 + 1;
        while out.boundaries < guard {
            let (first, last, next) = {
                let st = &self.states[self.current];
                (st.first_frame as f32, st.last_frame, st.next)
            };
            let end = last.min(clip_frames - 1) as f32 + 1.0;
            if self.cursor < end {
                break;
            }
            out.boundaries += 1;
            let next = next.unwrap_or(self.current);
            if next == self.current {
                self.cursor = first + (self.cursor - end).max(0.0);
            } else {
                self.enter(next);
                out.entered = Some(next);
            }
        }
        if self.target == Some(self.current) {
            self.target = None;
        }
        out
    }

    fn enter(&mut self, state: usize) {
        self.current = state;
        self.cursor = self.states[state].first_frame as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent synthetic rig — root + two chained limbs, all with
    /// distinct non-axis-aligned bind offsets.
    const SKEL: &str = "\
NumBones 3
bone root {
\toffset 0.2 1.0 -0.1
\tbone upper {
\t\toffset 0.4 0.1 0.0
\t\tbone lower {
\t\t\toffset 0.0 0.5 0.2
\t\t}
\t}
}
";

    fn rig() -> PedRig {
        PedRig::from_skel(&PedSkel::parse(SKEL).unwrap()).unwrap()
    }

    /// fpf = (bones + 1) * 3 = 12 for the 3-bone fixture.
    fn clip(frames: &[[f32; 12]]) -> PedAnim {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(frames.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&12u32.to_le_bytes());
        bytes.extend_from_slice(&0f32.to_le_bytes());
        bytes.push(1);
        for f in frames {
            for v in f {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        PedAnim::parse(&bytes).unwrap()
    }

    fn csv(body: &str) -> PedStates {
        PedStates::parse(body).unwrap()
    }

    #[test]
    fn rig_flattens_pre_order_and_fk_accumulates_bind_offsets() {
        let r = rig();
        assert_eq!(r.bone_count(), 3);
        assert_eq!(r.bones()[0].name, "root");
        assert_eq!(r.bones()[0].parent, None);
        assert_eq!(r.bones()[1].name, "upper");
        assert_eq!(r.bones()[1].parent, Some(0));
        assert_eq!(r.bones()[2].name, "lower");
        assert_eq!(r.bones()[2].parent, Some(1));
        let w = r.world_transforms(&r.bind_pose());
        assert!(w[0].0.abs_diff_eq(Vec3::new(0.2, 1.0, -0.1), 1e-6));
        assert!(w[1].0.abs_diff_eq(Vec3::new(0.6, 1.1, -0.1), 1e-6));
        assert!(w[2].0.abs_diff_eq(Vec3::new(0.6, 1.6, 0.1), 1e-6));
        assert!(w.iter().all(|(_, q)| *q == Quat::IDENTITY));
    }

    #[test]
    fn rig_rejects_empty_and_multi_root_skeletons() {
        assert_eq!(
            PedRig::from_skel(&PedSkel::parse("NumBones 0\n").unwrap()).unwrap_err(),
            PedRigError::Empty
        );
        let multi = PedSkel::parse(
            "NumBones 2\nbone a {\n\toffset 0 0 0\n}\nbone b {\n\toffset 0 0 0\n}\n",
        )
        .unwrap();
        assert_eq!(
            PedRig::from_skel(&multi).unwrap_err(),
            PedRigError::MultipleRoots(2)
        );
    }

    #[test]
    fn sample_maps_channels_to_bones_and_root_translation() {
        // frame 0: root at (5,1,0), bone 1 ("upper") rotated +90° about
        // Z — bone-local rotation must swing its child's world offset.
        let mut f = [0.0f32; 12];
        f[0] = 5.0;
        f[1] = 1.0;
        // channel for bone i sits at 3 + i*3; upper is bone 1 → offset 6.
        f[8] = std::f32::consts::FRAC_PI_2; // bone 1 z-rotation
        let c = clip(&[f]);
        let r = rig();
        let pose = r.sample(&c, 0.0).unwrap();
        assert!(
            pose.bones[0]
                .translation
                .abs_diff_eq(Vec3::new(5.0, 1.0, 0.0), 1e-6)
        );
        assert_eq!(pose.bones[0].rotation, Quat::IDENTITY);
        assert!(
            pose.bones[1]
                .rotation
                .abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), 1e-6)
        );
        // Rotating "upper" by +90° about Z turns its +Y child offset to -X.
        let w = r.world_transforms(&pose);
        assert!(w[1].0.abs_diff_eq(Vec3::new(5.4, 1.1, 0.0), 1e-6));
        assert!(w[2].0.abs_diff_eq(Vec3::new(4.9, 1.1, 0.2), 1e-6));
    }

    #[test]
    fn euler_triple_composes_fixed_axis_x_then_y_then_z() {
        // AGE's Matrix34::GetEulers convention: row-vector Rx·Ry·Rz —
        // fixed-axis X applied first == column product Rz·Ry·Rx.
        let mut f = [0.0f32; 12];
        f[3] = 0.4; // root bone x-rotation
        f[4] = -0.7;
        f[5] = 0.9;
        let c = clip(&[f]);
        let pose = rig().sample(&c, 0.0).unwrap();
        let want =
            Quat::from_rotation_z(0.9) * Quat::from_rotation_y(-0.7) * Quat::from_rotation_x(0.4);
        assert!(pose.bones[0].rotation.abs_diff_eq(want, 1e-6));
    }

    #[test]
    fn sample_lerps_channels_between_frames() {
        let a = [0.0f32; 12];
        let mut b = [0.0f32; 12];
        b[0] = 2.0; // root x moves 0→2
        b[8] = 1.0; // bone 1 z-rotation 0→1 rad
        let c = clip(&[a, b]);
        let r = rig();
        let pose = r.sample(&c, 0.5).unwrap();
        assert!(
            pose.bones[0]
                .translation
                .abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-6)
        );
        assert!(
            pose.bones[1]
                .rotation
                .abs_diff_eq(Quat::from_rotation_z(0.5), 1e-5)
        );
    }

    #[test]
    fn sample_clamps_frame_and_rejects_bad_shapes() {
        let c = clip(&[[0.0; 12]; 3]);
        let r = rig();
        assert!(r.sample(&c, 99.0).is_ok());
        assert!(r.sample(&c, -5.0).is_ok());
        assert!(r.sample(&c, f32::NAN).is_ok());

        // Empty clip and narrow channel are errors, not panics.
        let mut empty = clip(&[[0.0; 12]; 3]);
        empty.frames = 0;
        assert_eq!(
            r.sample(&empty, 0.0).unwrap_err(),
            PedSampleError::EmptyClip
        );
        let narrow = PedAnim {
            floats_per_frame: 9,
            ..clip(&[[0.0; 12]; 1])
        };
        assert_eq!(
            r.sample(&narrow, 0.0).unwrap_err(),
            PedSampleError::FrameWidth {
                needed: 12,
                actual: 9
            }
        );
        // Extra channels beyond the rig are ignored, not fatal.
        let wide = PedAnim {
            floats_per_frame: 15,
            samples: vec![0.0; 15],
            ..clip(&[[0.0; 12]; 1])
        };
        assert!(r.sample(&wide, 0.0).is_ok());
    }

    #[test]
    fn animator_steps_the_window_and_wraps_on_self_loop() {
        let states = csv("STAND,stand,1,4,0,0,0,0,STAND\nWALK,walk,1,2,0,1,0,0,WALK\n");
        let mut a = PedAnimator::new(&states, "STAND", 10.0).unwrap();
        assert_eq!(a.current().name, "STAND");
        assert_eq!(a.frame(), 0.0);
        let t = a.tick(0.1, 4); // 1 frame
        assert_eq!(t, PedTick::default());
        assert_eq!(a.frame(), 1.0);
        // 4-frame window: after 4 ticks the cursor wraps to first.
        a.tick(0.1, 4);
        a.tick(0.1, 4);
        let t = a.tick(0.1, 4);
        assert_eq!(t.boundaries, 1);
        assert_eq!(t.entered, None);
        assert_eq!(a.current().name, "STAND");
        assert_eq!(a.frame(), 0.0);
    }

    #[test]
    fn animator_routes_requests_through_authored_transitions() {
        let states = csv("STAND,stand,1,30,0,0,0,0,STAND\n\
             STAND_WALK,st2w,1,4,0,0,0,0,WALK\n\
             WALK,walk,1,20,0,1,0,0,WALK\n");
        let mut a = PedAnimator::new(&states, "STAND", 30.0).unwrap();
        assert!(a.request("WALK"));
        assert_eq!(a.current().name, "STAND_WALK");
        assert_eq!(a.target(), Some("WALK"));
        assert_eq!(a.frame(), 0.0);
        // Play through the 4-frame transition window → lands on WALK.
        let t = a.tick(4.0 / 30.0, 4);
        assert_eq!(a.current().name, "WALK");
        assert_eq!(t.entered, Some(2));
        assert_eq!(a.target(), None);
    }

    #[test]
    fn animator_switches_directly_without_an_authored_transition() {
        let states = csv("STAND,stand,1,30,0,0,0,0,STAND\nRUN,run,1,12,0,2,0,0,RUN\n");
        let mut a = PedAnimator::new(&states, "STAND", 30.0).unwrap();
        assert!(a.request("RUN"));
        assert_eq!(a.current().name, "RUN");
        assert_eq!(a.target(), None);
    }

    #[test]
    fn animator_refuses_unknown_targets_and_idle_dt() {
        let states = csv("STAND,stand,1,30,0,0,0,0,STAND\n");
        let mut a = PedAnimator::new(&states, "STAND", 30.0).unwrap();
        assert!(!a.request("FLY"));
        assert_eq!(a.current().name, "STAND");
        assert_eq!(a.tick(0.0, 30), PedTick::default());
        assert_eq!(a.tick(-1.0, 30), PedTick::default());
        assert_eq!(a.tick(1.0, 0), PedTick::default());
        assert_eq!(a.frame(), 0.0);
        // Construction failures don't panic.
        assert_eq!(
            PedAnimator::new(&states, "NOPE", 30.0).unwrap_err(),
            PedAnimError::UnknownState("NOPE".into())
        );
        assert_eq!(
            PedAnimator::new(&states, "STAND", 0.0).unwrap_err(),
            PedAnimError::BadFps(0.0)
        );
        assert_eq!(
            PedAnimator::new(
                &PedStates {
                    states: vec![],
                    diagnostics: vec![]
                },
                "X",
                30.0
            )
            .unwrap_err(),
            PedAnimError::EmptyTable
        );
    }

    #[test]
    fn animator_clamps_authored_overshoot_window() {
        // The authored `frames + 1` quirk: last frame 5 on a 4-frame
        // clip — the window must clamp so sampling never escapes it.
        let states = csv("A,ca,1,5,0,0,0,0,B\nB,cb,1,2,0,0,0,0,B\n");
        let mut a = PedAnimator::new(&states, "A", 10.0).unwrap();
        // Without clamping the window would span 5 frames; with the
        // clamp it ends at frame index 3 → boundary on the 4th step.
        a.tick(0.1, 4);
        a.tick(0.1, 4);
        a.tick(0.1, 4);
        let t = a.tick(0.1, 4);
        assert_eq!(a.current().name, "B");
        assert_eq!(t.entered, Some(1));
    }

    #[test]
    fn animator_saturates_hostile_frame_windows() {
        // csv frame fields parse as i64 — authored extremes must
        // saturate into the u32 window, not overflow the 1→0 rebase
        // (`i64::MIN - 1`) or truncate through an `as u32` cast.
        let states = csv(&format!(
            "A,ca,{},{},0,0,0,0,A\nB,cb,{},5,0,0,0,0,B\n",
            i64::MIN,
            i64::MAX,
            i64::MAX
        ));
        assert!(!states.validate().is_empty());
        let mut a = PedAnimator::new(&states, "A", 30.0).unwrap();
        assert_eq!(a.states()[0].first_frame, 0);
        assert_eq!(a.states()[0].last_frame, u32::MAX);
        assert_eq!(a.states()[1].first_frame, u32::MAX);
        assert_eq!(a.states()[1].last_frame, 4);
        // Stepping clamps the saturated window against the real clip.
        assert_eq!(a.tick(0.05, 2), PedTick::default());
        let t = a.tick(0.05, 2);
        assert_eq!(t.boundaries, 1);
        assert_eq!(t.entered, None);
        assert_eq!(a.current().name, "A");
        assert!(a.frame().is_finite());
    }

    #[test]
    fn pose_lerp_blends_translations_and_rotations() {
        let r = rig();
        let mut a = r.bind_pose();
        let mut b = r.bind_pose();
        b.bones[0].translation = Vec3::new(2.0, 0.0, 0.0);
        b.bones[1].rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        a.bones[0].translation = Vec3::ZERO;
        let mid = a.lerp(&b, 0.5);
        assert!(
            mid.bones[0]
                .translation
                .abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-6)
        );
        assert!(
            mid.bones[1]
                .rotation
                .abs_diff_eq(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4), 1e-5)
        );
    }
}
