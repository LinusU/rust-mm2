//! Pedestrian skeletal runtime (F19-A.3/A.4): sampling `pedanim_*.anim`
//! clips onto a `pedmodel_*.skel` rig, stepping the authored
//! `pedmodel_*.csv` state model, and assembling the `pedmodel_*.mod`
//! skinned mesh so it can deform over sampled poses.
//!
//! `.mod` vertex positions are **bone-local** — measured on retail:
//! every authored `v` lies within ~0.55 m of the origin while the rig
//! stands ~1.15–1.8 m tall, and `T_world(bone) · v` reassembles each
//! mesh as a feet-on-the-ground standing figure. The vertex→bone map is
//! the `mtxv` row: per-matrix contiguous counts over the `v` array in
//! `.skel` pre-order (`mtxn` does the same for `n`). On the packet
//! dialect each adjunct additionally carries a slot into its packet's
//! `mtx` bone list — both records agree on every retail adjunct, so the
//! packet binding is preferred where present and `mtxv` is the fallback
//! (and the only binding on the flat dialect).
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
use mm2_formats::ped::{PedAnim, PedMod, PedModAdj, PedModPrim, PedSkel, PedStates};
use std::collections::HashMap;
use std::ops::Range;

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
        let i1 = i0.saturating_add(1).min(clip.frames - 1);
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

impl PedAnimState {
    /// Ground speed (m/s) a looping locomotion state implies: its
    /// authored `Y AXIS DISTANCE` over the time one pass of its window
    /// takes at `fps`, the window clamped against the clip exactly as
    /// [`PedAnimator::tick`] clamps it. A walker moved at this speed
    /// keeps its feet planted to the ground. `None` for a state that
    /// authors no forward travel, an empty window, or a non-finite or
    /// non-positive `fps`. The magnitude is returned: a backing-up
    /// state authors a negative distance and the caller owns direction.
    pub fn locomotion_speed(&self, clip_frames: u32, fps: f32) -> Option<f32> {
        if clip_frames == 0 || !fps.is_finite() || fps <= 0.0 || !self.y_distance.is_finite() {
            return None;
        }
        let end = self.last_frame.min(clip_frames - 1) as u64 + 1;
        let frames = end
            .checked_sub(self.first_frame as u64)
            .filter(|f| *f > 0)?;
        let speed = self.y_distance.abs() / (frames as f32 / fps);
        (speed.is_finite() && speed > 0.0).then_some(speed)
    }
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

/// One assembled `.mod` corner — an adjunct resolved to a bone-local
/// position/normal plus its authored colour and UVs, bound rigidly to
/// one rig bone (the whole corner follows a single matrix; an adjunct's
/// normal rotates with the same bone as its position).
#[derive(Debug, Clone)]
pub struct PedCorner {
    /// Rig bone index (`.skel` pre-order) this corner deforms with.
    pub bone: u32,
    /// Bone-local authored position (`v` row).
    pub position: Vec3,
    /// Bone-local authored normal (`n` row).
    pub normal: Vec3,
    /// RGBA vertex colour — `c[adj.color]`, white when the file carries
    /// no colour table (or the index is `0` into an empty one).
    pub color: [f32; 4],
    /// First UV set (`t1[adj.tex1]`, `[0, 0]` when unset).
    pub uv: [f32; 2],
    /// Second UV set (`t2[adj.tex2]`; no retail adjunct uses it).
    pub uv2: [f32; 2],
    /// 1-based source line.
    pub line: u32,
}

/// One `.mod` material group carried into the skin — name, authored
/// shading fields and the slice of [`PedSkin::triangles`] it owns.
#[derive(Debug, Clone)]
pub struct PedSkinMtl {
    /// `mtl` name, e.g. `Businessman1:SKIN`.
    pub name: String,
    /// `illum:` value (`diffuse`/`emit`).
    pub illum: Option<String>,
    /// `ambient:` fallback colour.
    pub ambient: Option<[f32; 3]>,
    /// `diffuse:` fallback colour.
    pub diffuse: Option<[f32; 3]>,
    /// `specular:` fallback colour.
    pub specular: Option<[f32; 3]>,
    /// `texture: <index> <name>` rows.
    pub texture_names: Vec<(i64, String)>,
    /// This material's contiguous slice of the skin's triangle list.
    pub tris: Range<usize>,
}

/// Why a [`PedMod`] cannot assemble into a [`PedSkin`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PedSkinError {
    /// A vert/normal index is negative or outside its resource list —
    /// unfaithful geometry, not degradable.
    BadIndex {
        /// Which index (`vert`/`normal`).
        what: &'static str,
        /// The offending index.
        index: i64,
        /// Length of the resource list.
        len: usize,
        /// 1-based adjunct line.
        line: u32,
    },
    /// A packet adjunct's matrix slot is outside the packet's `mtx`
    /// list.
    BadMatrixSlot {
        /// The authored slot.
        slot: i64,
        /// The packet's `mtx` list length.
        matrices: usize,
        /// 1-based adjunct line.
        line: u32,
    },
    /// The resolved bone index is outside the rig (a `.mod` authored
    /// against a different skeleton, or a corrupt `mtx`/`mtxv`).
    BoneOutOfRange {
        /// The resolved bone index.
        bone: i64,
        /// Rig bone count.
        bones: usize,
        /// 1-based adjunct line.
        line: u32,
    },
    /// No binding record covers this adjunct's vertex — the flat
    /// dialect has no per-adjunct slot, so a missing/short `mtxv`
    /// partition leaves it unbindable.
    UnboundVertex {
        /// The adjunct's vertex index.
        vert: i64,
        /// 1-based adjunct line.
        line: u32,
    },
}

impl std::fmt::Display for PedSkinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadIndex {
                what,
                index,
                len,
                line,
            } => write!(
                f,
                "adjunct line {line}: {what} index {index} outside 0..{len}"
            ),
            Self::BadMatrixSlot {
                slot,
                matrices,
                line,
            } => write!(
                f,
                "adjunct line {line}: matrix slot {slot} outside the packet's {matrices} matrices"
            ),
            Self::BoneOutOfRange { bone, bones, line } => write!(
                f,
                "adjunct line {line}: bound to bone {bone} against a {bones}-bone rig"
            ),
            Self::UnboundVertex { vert, line } => write!(
                f,
                "adjunct line {line}: vertex {vert} has no `mtxv` bone bucket"
            ),
        }
    }
}

impl std::error::Error for PedSkinError {}

/// Why a [`PedSkin::deform`] call fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PedDeformError {
    /// The world-transform slice is shorter than the bones the skin
    /// references.
    MissingBones {
        /// Bone slots the skin needs.
        needed: usize,
        /// Bone slots the caller supplied.
        got: usize,
    },
    /// A referenced bone transform is non-finite — the output would be
    /// NaN corner positions.
    NonFiniteTransform {
        /// First offending bone index.
        bone: usize,
    },
}

impl std::fmt::Display for PedDeformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBones { needed, got } => {
                write!(f, "skin needs {needed} bone transforms, got {got}")
            }
            Self::NonFiniteTransform { bone } => {
                write!(f, "bone {bone}'s world transform is non-finite")
            }
        }
    }
}

impl std::error::Error for PedDeformError {}

/// World-space deform output — parallel to [`PedSkin::corners`].
#[derive(Debug, Clone)]
pub struct PedDeform {
    /// Corner positions after `T_world(bone) · position`.
    pub positions: Vec<Vec3>,
    /// Corner normals after `R_world(bone) · normal` (rotation only).
    pub normals: Vec<Vec3>,
}

impl PedDeform {
    /// Every component finite.
    pub fn is_finite(&self) -> bool {
        self.positions.iter().all(|p| p.is_finite()) && self.normals.iter().all(|n| n.is_finite())
    }
}

/// An assembled `.mod` mesh: corner-indexed geometry whose triangles
/// are grouped contiguously per material, ready to deform over a pose.
///
/// Assembly keeps the authored data verbatim — bone-local positions,
/// authored shading fields — and resolves only the indices (resource
/// lookups, bone bindings, primitive → corner mapping). What it cannot
/// honour faithfully is an error or a recorded [`issues`](Self::issues)
/// entry, never silently reshaped: out-of-range resource indices abort
/// the assembly, strip primitives are counted but not expanded (their
/// winding is unrecovered, UNK-41), and primitives no material group
/// owns land in [`orphan_tris`](Self::orphan_tris).
#[derive(Debug, Clone)]
pub struct PedSkin {
    corners: Vec<PedCorner>,
    tris: Vec<[u32; 3]>,
    materials: Vec<PedSkinMtl>,
    /// Triangles emitted by primitives outside every material group's
    /// declared ownership — appended after all material slices.
    orphan_tris: Range<usize>,
    /// One past the highest rig bone any corner references — the
    /// minimum `deform` input length.
    bones_needed: usize,
    /// Non-fatal assembly degradations.
    pub issues: Vec<String>,
}

/// Bucket an element index into a `mtxv`/`mtxn`-style per-matrix
/// contiguous partition. `None` when the row is absent, carries a
/// negative count, or stops short of `index`.
fn matrix_bucket(partition: &[i64], index: i64) -> Option<u32> {
    if index < 0 || partition.iter().any(|&c| c < 0) {
        return None;
    }
    let mut at = 0i64;
    for (bone, &count) in partition.iter().enumerate() {
        // `count` is a full-range authored i64: a hostile row can push
        // the cursor past i64::MAX. Saturating still buckets correctly —
        // `index` is always an already-range-checked resource index
        // (< i64::MAX), so the first bucket whose running total reaches
        // saturation does own every not-yet-claimed index.
        at = at.saturating_add(count);
        if index < at {
            return Some(bone as u32);
        }
    }
    None
}

/// `i64` resource index → `usize`, bounds-checked.
fn resource_index(i: i64, len: usize) -> Option<usize> {
    usize::try_from(i).ok().filter(|&v| v < len)
}

impl PedSkin {
    /// Assemble `m` against `rig`. Both `.mod` dialects resolve the
    /// same way — packet adjuncts bind through their `mtx` slot (a
    /// missing slot falls back to the `mtxv` vertex partition), flat
    /// adjuncts bind through `mtxv` alone. A resolved bone must exist
    /// on the rig.
    pub fn from_mod(m: &PedMod, rig: &PedRig) -> Result<Self, PedSkinError> {
        let bones = rig.bone_count();
        let mut issues = Vec::new();
        let mut bones_needed = 0usize;
        let mut mtxn_mismatch = 0usize;

        // Corner resolution shared by both dialects. `packet_mtx` is
        // the owning packet's bone list when the adjunct carries a
        // slot; the `mtxv` vertex partition is the fallback and the
        // flat dialect's only record.
        let resolve = |adj: &PedModAdj,
                       packet_mtx: Option<&[i64]>,
                       issues: &mut Vec<String>,
                       mtxn_mismatch: &mut usize|
         -> Result<PedCorner, PedSkinError> {
            let vi = resource_index(adj.vert, m.verts.len()).ok_or(PedSkinError::BadIndex {
                what: "vert",
                index: adj.vert,
                len: m.verts.len(),
                line: adj.line,
            })?;
            let ni = resource_index(adj.normal, m.normals.len()).ok_or(PedSkinError::BadIndex {
                what: "normal",
                index: adj.normal,
                len: m.normals.len(),
                line: adj.line,
            })?;
            let bone = match (adj.matrix, packet_mtx) {
                (Some(slot), Some(mtx)) => resource_index(slot, mtx.len()).map(|s| mtx[s]).ok_or(
                    PedSkinError::BadMatrixSlot {
                        slot,
                        matrices: mtx.len(),
                        line: adj.line,
                    },
                )?,
                _ => matrix_bucket(&m.matrix_verts, adj.vert)
                    .map(i64::from)
                    .ok_or(PedSkinError::UnboundVertex {
                        vert: adj.vert,
                        line: adj.line,
                    })?,
            };
            if !(0..bones as i64).contains(&bone) {
                return Err(PedSkinError::BoneOutOfRange {
                    bone,
                    bones,
                    line: adj.line,
                });
            }
            // `mtxn` partitions normals by the same per-matrix rule;
            // on retail every adjunct's normal bucket equals its
            // corner bone. A disagreement is corrupt data — count it,
            // the corner still rigidly follows `bone`.
            if let Some(nb) = matrix_bucket(&m.matrix_normals, adj.normal)
                && nb as i64 != bone
            {
                *mtxn_mismatch += 1;
            }
            // Colour/tex indices degrade to defaults: index 0 into an
            // empty list is authored "unset" (retail `tex2s: 0` shape);
            // anything else out of range is a recorded degradation.
            let mut resource = |i: i64, len: usize, what: &str| {
                resource_index(i, len).or_else(|| {
                    if !(i == 0 && len == 0) {
                        issues.push(format!(
                            "line {}: adjunct {what} index {i} outside 0..{len}",
                            adj.line
                        ));
                    }
                    None
                })
            };
            Ok(PedCorner {
                bone: bone as u32,
                position: Vec3::from_array(m.verts[vi]),
                normal: Vec3::from_array(m.normals[ni]),
                color: resource(adj.color, m.colors.len(), "colour")
                    .map(|i| m.colors[i])
                    .unwrap_or([1.0; 4]),
                uv: resource(adj.tex1, m.tex1s.len(), "tex1")
                    .map(|i| m.tex1s[i])
                    .unwrap_or([0.0; 2]),
                uv2: resource(adj.tex2, m.tex2s.len(), "tex2")
                    .map(|i| m.tex2s[i])
                    .unwrap_or([0.0; 2]),
                line: adj.line,
            })
        };

        // Corners: flat adjuncts in authored order first (their global
        // indices are the flat `tri` indices), then each packet's
        // adjuncts; `packet_base` records each packet's first corner.
        let mut corners = Vec::with_capacity(m.all_adjuncts().count());
        for adj in &m.adjuncts {
            let c = resolve(adj, None, &mut issues, &mut mtxn_mismatch)?;
            bones_needed = bones_needed.max(c.bone as usize + 1);
            corners.push(c);
        }
        let mut packet_base = Vec::with_capacity(m.packets.len());
        for p in &m.packets {
            packet_base.push(corners.len());
            for adj in &p.adjuncts {
                let c = resolve(adj, Some(&p.matrices), &mut issues, &mut mtxn_mismatch)?;
                bones_needed = bones_needed.max(c.bone as usize + 1);
                corners.push(c);
            }
        }

        // Triangles grouped per material, then anything unclaimed.
        // `prim_owned`/`packet_owned` spot primitives and packet blocks
        // no `mtl` declared — their tris still assemble, into
        // `orphan_tris`.
        let mut tris: Vec<[u32; 3]> = Vec::new();
        let mut materials = Vec::with_capacity(m.materials.len());
        let mut prim_owned = vec![false; m.primitives.len()];
        let mut packet_owned = vec![false; m.packets.len()];
        let mut strips = 0usize;
        let emit = |prim: &PedModPrim,
                    bound: usize,
                    base: usize,
                    tris: &mut Vec<[u32; 3]>,
                    issues: &mut Vec<String>,
                    strips: &mut usize| {
            match prim {
                PedModPrim::Tri(t) => {
                    let resolved = t
                        .iter()
                        .map(|&i| resource_index(i, bound).map(|k| (base + k) as u32));
                    if let Some(tri) = resolved.collect::<Option<Vec<u32>>>() {
                        tris.push([tri[0], tri[1], tri[2]]);
                    } else {
                        issues.push(format!(
                            "primitive index outside 0..{bound}; triangle dropped"
                        ));
                    }
                }
                PedModPrim::Strip { .. } => *strips += 1,
            }
        };

        for mtl in &m.materials {
            let tri_base = tris.len();
            for pi in mtl.packet_range.clone() {
                packet_owned[pi] = true;
                let p = &m.packets[pi];
                for prim in &p.primitives {
                    emit(
                        prim,
                        p.adjuncts.len(),
                        packet_base[pi],
                        &mut tris,
                        &mut issues,
                        &mut strips,
                    );
                }
            }
            for pi in mtl.primitive_range.clone() {
                prim_owned[pi] = true;
                emit(
                    &m.primitives[pi],
                    m.adjuncts.len(),
                    0,
                    &mut tris,
                    &mut issues,
                    &mut strips,
                );
            }
            materials.push(PedSkinMtl {
                name: mtl.name.clone(),
                illum: mtl.illum.clone(),
                ambient: mtl.ambient,
                diffuse: mtl.diffuse,
                specular: mtl.specular,
                texture_names: mtl.texture_names.clone(),
                tris: tri_base..tris.len(),
            });
        }

        let orphan_start = tris.len();
        let mut orphan_prims = 0usize;
        for (pi, owned) in packet_owned.iter().enumerate() {
            if *owned {
                continue;
            }
            let p = &m.packets[pi];
            orphan_prims += p.primitives.len();
            for prim in &p.primitives {
                emit(
                    prim,
                    p.adjuncts.len(),
                    packet_base[pi],
                    &mut tris,
                    &mut issues,
                    &mut strips,
                );
            }
        }
        for (pi, owned) in prim_owned.iter().enumerate() {
            if !owned {
                orphan_prims += 1;
                emit(
                    &m.primitives[pi],
                    m.adjuncts.len(),
                    0,
                    &mut tris,
                    &mut issues,
                    &mut strips,
                );
            }
        }
        if orphan_prims > 0 {
            issues.push(format!(
                "{orphan_prims} primitives outside every material group's declared ownership"
            ));
        }
        if strips > 0 {
            issues.push(format!(
                "{strips} strip primitives not expanded (strip winding is unrecovered, UNK-41)"
            ));
        }
        if mtxn_mismatch > 0 {
            issues.push(format!(
                "{mtxn_mismatch} adjunct normals' `mtxn` buckets disagree with their corner bones"
            ));
        }
        let orphan_tris = orphan_start..tris.len();
        Ok(PedSkin {
            corners,
            tris,
            materials,
            orphan_tris,
            bones_needed,
            issues,
        })
    }

    /// Every assembled corner, in authored order (flat adjuncts first,
    /// then each packet's adjuncts).
    pub fn corners(&self) -> &[PedCorner] {
        &self.corners
    }

    /// Every assembled triangle — corner indices — grouped
    /// contiguously per material, then the orphan tail.
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.tris
    }

    /// Materials in authored `.mod` order; each owns a contiguous
    /// slice of [`triangles`](Self::triangles).
    pub fn materials(&self) -> &[PedSkinMtl] {
        &self.materials
    }

    /// The slice of [`triangles`](Self::triangles) emitted by
    /// primitives no material group claimed.
    pub fn orphan_tris(&self) -> Range<usize> {
        self.orphan_tris.clone()
    }

    /// Bone transforms `deform` needs (one past the highest bound
    /// bone).
    pub fn bones_needed(&self) -> usize {
        self.bones_needed
    }

    /// How many drawn triangles have a winding-order normal on the same
    /// side as the authored corner normals — a measurable check of the
    /// front-face convention over a deformed pose. Returns
    /// `(agreeing, counted)`; degenerate triangles are not counted.
    pub fn winding_agreement(&self, deform: &PedDeform) -> (usize, usize) {
        let mut agree = 0;
        let mut total = 0;
        for tri in &self.tris {
            let [a, b, c] = tri.map(|i| deform.positions[i as usize]);
            let geometric = (b - a).cross(c - a);
            if geometric.length_squared() <= f32::EPSILON {
                continue;
            }
            let authored: Vec3 = tri.iter().map(|&i| deform.normals[i as usize]).sum();
            total += 1;
            if geometric.dot(authored) > 0.0 {
                agree += 1;
            }
        }
        (agree, total)
    }

    /// Rigid-skin every corner: `position' = t + r · position`,
    /// `normal' = r · normal`, where `(t, r)` is the corner bone's
    /// world transform — [`PedRig::world_transforms`] output over the
    /// sampled [`PedPose`]. `.mod` verts are bone-local, so the posed
    /// transform applies directly (no inverse-bind matrix).
    pub fn deform(&self, world: &[(Vec3, Quat)]) -> Result<PedDeform, PedDeformError> {
        if world.len() < self.bones_needed {
            return Err(PedDeformError::MissingBones {
                needed: self.bones_needed,
                got: world.len(),
            });
        }
        for (bone, (t, r)) in world[..self.bones_needed].iter().enumerate() {
            if !t.is_finite() || !r.is_finite() {
                return Err(PedDeformError::NonFiniteTransform { bone });
            }
        }
        let mut positions = Vec::with_capacity(self.corners.len());
        let mut normals = Vec::with_capacity(self.corners.len());
        for c in &self.corners {
            let (t, r) = world[c.bone as usize];
            positions.push(t + r * c.position);
            normals.push(r * c.normal);
        }
        Ok(PedDeform { positions, normals })
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
    fn locomotion_speed_is_window_travel_over_window_time() {
        let states = csv("STAND,stand,1,30,0,0,0,0,STAND\n\
             WALK,walk,1,20,0,1.5,0,0,WALK\n\
             BACKUP,back,1,10,0,-1.0,0,0,BACKUP\n");
        let a = PedAnimator::new(&states, "STAND", 30.0).unwrap();
        let by = |n: &str| a.states().iter().find(|s| s.name == n).unwrap();
        // 1.5 m over 20 frames at 30 fps (2/3 s).
        let walk = by("WALK").locomotion_speed(20, 30.0).unwrap();
        assert!((walk - 2.25).abs() < 1e-5, "{walk}");
        // Overshoot windows clamp against the clip: only 10 frames exist.
        let short = by("WALK").locomotion_speed(10, 30.0).unwrap();
        assert!((short - 4.5).abs() < 1e-5, "{short}");
        // Magnitude only; no travel, bad inputs: nothing.
        assert!((by("BACKUP").locomotion_speed(10, 30.0).unwrap() - 3.0).abs() < 1e-5);
        assert_eq!(by("STAND").locomotion_speed(30, 30.0), None);
        assert_eq!(by("WALK").locomotion_speed(0, 30.0), None);
        assert_eq!(by("WALK").locomotion_speed(20, 0.0), None);
        assert_eq!(by("WALK").locomotion_speed(20, f32::NAN), None);
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

    /// Flat-dialect `.mod` fixture: the global `adj`/`tri` lists, the
    /// `mtxv`/`mtxn` partitions binding vert `i`/normal `i` to bone
    /// `i`. Bind-pose bone origins: root (0.2,1,-0.1), upper
    /// (0.6,1.1,-0.1), lower (0.6,1.6,0.1).
    const MOD_FLAT: &str = "\
version: 1.09
verts: 3
normals: 3
colors: 1
tex1s: 1
tex2s: 0
tangents: 0
materials: 1
adjuncts: 3
primitives: 1
matrices: 3

v	0.1	0.0	0.0
v	0.0	0.2	0.0
v	0.0	0.0	0.3
n	0.0	0.0	1.0
n	0.0	1.0	0.0
n	1.0	0.0	0.0
c	0.5	0.6	0.7	1.0
t1	0.25	0.75

mtl A:SKIN {
	adjuncts:	3
	primitives:	1
	textures:	0
	illum: diffuse
}

adj	0	0	0	0	0
adj	1	1	0	0	0
adj	2	2	0	0	0
tri	0	1	2

mtxv 1 1 1
mtxn 1 1 1
";

    /// Packet-dialect fixture: each packet's `mtx` list is the bone
    /// table its adjunct slots index — packet 0 binds bones 0 and 2,
    /// packet 1 binds bone 1 (same corner bones as MOD_FLAT, reached
    /// through the per-adjunct slot instead of `mtxv`).
    const MOD_PACKET: &str = "\
version: 1.09
verts: 3
normals: 3
colors: 1
tex1s: 1
tex2s: 0
tangents: 0
materials: 2
adjuncts: 3
primitives: 2
matrices: 3

v	0.1	0.0	0.0
v	0.0	0.2	0.0
v	0.0	0.0	0.3
n	0.0	0.0	1.0
n	0.0	1.0	0.0
n	1.0	0.0	0.0
c	0.5	0.6	0.7	1.0
t1	0.25	0.75

mtl A:SKIN {
	packets:	1
	primitives:	1
	textures:	1
	texture:	0 BEARD64
	illum: emit
}
mtl B:SKIN {
	packets:	1
	primitives:	1
	textures:	0
}

packet 2 1 2 {
	adj	0	0	0	0	0	0
	adj	1	2	0	0	0	1
	tri	0	1	0
	mtx	0	2
}

packet 1 1 1 {
	adj	2	1	0	0	0	0
	tri	0	0	0
	mtx	1
}

mtxv 1 1 1
mtxn 1 1 1
";

    fn skin(m: &str) -> PedSkin {
        PedSkin::from_mod(&PedMod::parse(m).unwrap(), &rig()).unwrap()
    }

    #[test]
    fn skin_assembles_flat_dialect_and_deforms_at_bind() {
        let s = skin(MOD_FLAT);
        assert!(s.issues.is_empty(), "{:?}", s.issues);
        assert_eq!(s.corners().len(), 3);
        assert_eq!(s.triangles(), &[[0, 1, 2]]);
        assert_eq!(s.materials().len(), 1);
        assert_eq!(s.materials()[0].name, "A:SKIN");
        assert_eq!(s.materials()[0].tris, 0..1);
        assert!(s.orphan_tris().is_empty());
        assert_eq!(s.bones_needed(), 3);
        // Corner binding comes from the `mtxv` vertex partition:
        // vert i → bone i.
        assert_eq!(s.corners()[0].bone, 0);
        assert_eq!(s.corners()[1].bone, 1);
        assert_eq!(s.corners()[2].bone, 2);
        assert_eq!(s.corners()[0].color, [0.5, 0.6, 0.7, 1.0]);
        assert_eq!(s.corners()[0].uv, [0.25, 0.75]);
        assert_eq!(s.corners()[0].uv2, [0.0; 2]); // tex2s: 0 → unset
        // `v` rows are bone-local: at bind they sit on their bone's
        // world offset (identity rotations).
        let d = s
            .deform(&rig().world_transforms(&rig().bind_pose()))
            .unwrap();
        assert!(d.is_finite());
        assert!(d.positions[0].abs_diff_eq(Vec3::new(0.3, 1.0, -0.1), 1e-6));
        assert!(d.positions[1].abs_diff_eq(Vec3::new(0.6, 1.3, -0.1), 1e-6));
        assert!(d.positions[2].abs_diff_eq(Vec3::new(0.6, 1.6, 0.4), 1e-6));
        assert!(d.normals[0].abs_diff_eq(Vec3::new(0.0, 0.0, 1.0), 1e-6));
    }

    #[test]
    fn skin_assembles_packet_dialect_and_deforms_at_bind() {
        let s = skin(MOD_PACKET);
        assert!(s.issues.is_empty(), "{:?}", s.issues);
        // Corners: packet 0's two adjuncts, then packet 1's one.
        assert_eq!(s.corners().len(), 3);
        assert_eq!(s.corners()[0].bone, 0); // slot 0 → mtx[0] = 0
        assert_eq!(s.corners()[1].bone, 2); // slot 1 → mtx[1] = 2
        assert_eq!(s.corners()[2].bone, 1); // slot 0 → mtx[0] = 1
        // Packet-local tri indices land on the packet's corner base.
        assert_eq!(s.triangles(), &[[0, 1, 0], [2, 2, 2]]);
        assert_eq!(s.materials()[0].tris, 0..1);
        assert_eq!(s.materials()[1].tris, 1..2);
        assert_eq!(s.materials()[0].illum.as_deref(), Some("emit"));
        assert_eq!(s.materials()[0].texture_names, vec![(0, "BEARD64".into())]);
        let d = s
            .deform(&rig().world_transforms(&rig().bind_pose()))
            .unwrap();
        assert!(d.positions[0].abs_diff_eq(Vec3::new(0.3, 1.0, -0.1), 1e-6));
        assert!(d.positions[1].abs_diff_eq(Vec3::new(0.6, 1.8, 0.1), 1e-6));
        assert!(d.positions[2].abs_diff_eq(Vec3::new(0.6, 1.1, 0.2), 1e-6));
    }

    #[test]
    fn skin_deform_rotates_corners_about_their_bone() {
        // Root pitched +90° about X — corner positions rotate about
        // their own bone's origin, normals rotate too.
        let r = rig();
        let mut pose = r.bind_pose();
        pose.bones[0].rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let s = skin(MOD_FLAT);
        let d = s.deform(&r.world_transforms(&pose)).unwrap();
        // bone 0 corner: local (0.1,0,0) is on the rotation axis.
        assert!(d.positions[0].abs_diff_eq(Vec3::new(0.3, 1.0, -0.1), 1e-6));
        // bone 1 world origin (0.6,1,0); local (0,0.2,0) → (0,0,0.2).
        assert!(d.positions[1].abs_diff_eq(Vec3::new(0.6, 1.0, 0.2), 1e-6));
        // bone 2 world origin (0.6,0.8,0.5); local (0,0,0.3) → (0,-0.3,0).
        assert!(d.positions[2].abs_diff_eq(Vec3::new(0.6, 0.5, 0.5), 1e-6));
        assert!(d.normals[0].abs_diff_eq(Vec3::new(0.0, -1.0, 0.0), 1e-6));
        assert!(d.normals[1].abs_diff_eq(Vec3::new(0.0, 0.0, 1.0), 1e-6));
    }

    #[test]
    fn skin_rejects_unfaithful_geometry() {
        let bad_vert = MOD_FLAT.replace("adj\t0\t0\t0\t0\t0", "adj\t9\t0\t0\t0\t0");
        assert_eq!(
            PedSkin::from_mod(&PedMod::parse(&bad_vert).unwrap(), &rig()).unwrap_err(),
            PedSkinError::BadIndex {
                what: "vert",
                index: 9,
                len: 3,
                line: 29
            }
        );
        let bad_normal = MOD_FLAT.replace("adj\t0\t0\t0\t0\t0", "adj\t0\t-1\t0\t0\t0");
        assert_eq!(
            PedSkin::from_mod(&PedMod::parse(&bad_normal).unwrap(), &rig()).unwrap_err(),
            PedSkinError::BadIndex {
                what: "normal",
                index: -1,
                len: 3,
                line: 29
            }
        );
        // Packet slot beyond the packet's `mtx` list.
        let bad_slot = MOD_PACKET.replace("adj\t1\t2\t0\t0\t0\t1", "adj\t1\t2\t0\t0\t0\t9");
        assert_eq!(
            PedSkin::from_mod(&PedMod::parse(&bad_slot).unwrap(), &rig()).unwrap_err(),
            PedSkinError::BadMatrixSlot {
                slot: 9,
                matrices: 2,
                line: 37
            }
        );
        // A bone the rig doesn't have.
        let bad_bone = MOD_PACKET.replace("mtx\t0\t2", "mtx\t0\t7");
        assert_eq!(
            PedSkin::from_mod(&PedMod::parse(&bad_bone).unwrap(), &rig()).unwrap_err(),
            PedSkinError::BoneOutOfRange {
                bone: 7,
                bones: 3,
                line: 37
            }
        );
        // Flat dialect with no usable `mtxv` partition at all, and one
        // that stops short of the vertex list.
        let no_mtxv = MOD_FLAT.replace("mtxv 1 1 1\n", "");
        assert_eq!(
            PedSkin::from_mod(&PedMod::parse(&no_mtxv).unwrap(), &rig()).unwrap_err(),
            PedSkinError::UnboundVertex { vert: 0, line: 29 }
        );
        let short_mtxv = MOD_FLAT.replace("mtxv 1 1 1", "mtxv 1 1");
        assert_eq!(
            PedSkin::from_mod(&PedMod::parse(&short_mtxv).unwrap(), &rig()).unwrap_err(),
            PedSkinError::UnboundVertex { vert: 2, line: 31 }
        );
    }

    #[test]
    fn skin_records_degraded_primitives_and_mtxn_disagreement() {
        // A strip (winding unrecovered), a primitive no material owns,
        // a tri indexing outside the adjunct list, and an `mtxn`
        // partition disagreeing with the corner binding — all
        // recorded, none fatal.
        let m = MOD_PACKET
            .replace("tri\t0\t0\t0", "stp\t3\t0\t0\t0")
            .replace(
                "packet 1 1 1 {\n\tadj\t2\t1\t0\t0\t0\t0",
                "packet 1 1 1 {\n\tadj\t2\t1\t0\t0\t0\t0\n\ttri\t0\t0\t9\n\tstp\t3\t0\t0\t0",
            );
        // Give the file a third, unowned packet whose tri lands in
        // `orphan_tris`, and flip `mtxn` so every normal sits in bone
        // 2's bucket while corners ride 0/1/2.
        let m = m.replace(
            "mtxv 1 1 1\nmtxn 1 1 1",
            "packet 1 1 1 {\n\tadj\t0\t0\t0\t0\t0\t0\n\ttri\t0\t0\t0\n\tmtx\t0\n}\n\nmtxv 1 1 1\nmtxn 0 0 3",
        );
        let s = skin(&m);
        // Material A owns 1 tri; B's two prims are a bad `tri` (dropped
        // — index 9 outside the packet's 1 adjunct) and a skipped
        // `stp`; the unowned third packet's tri is the orphan tail.
        assert_eq!(s.triangles(), &[[0, 1, 0], [3, 3, 3]]);
        assert_eq!(s.orphan_tris(), 1..2);
        let joined = s.issues.join("\n");
        assert!(joined.contains("triangle dropped"), "{joined}");
        assert!(joined.contains("strip primitives"), "{joined}");
        assert!(joined.contains("outside every material group"), "{joined}");
        assert!(joined.contains("mtxn"), "{joined}");
    }

    #[test]
    fn skin_buckets_indices_past_a_saturating_mtxv_count() {
        // `mtxv`/`mtxn` counts are full-range authored i64s: a huge
        // entry must saturate the bucket cursor, not overflow it —
        // overflow panics under `overflow-checks` and wraps to a
        // wrong-but-in-range bone in release.
        let m = MOD_FLAT
            .replace("mtxv 1 1 1", "mtxv 1 9223372036854775807 1")
            .replace("mtxn 1 1 1", "mtxn 1 9223372036854775807 1");
        let s = skin(&m);
        assert!(s.issues.is_empty(), "{:?}", s.issues);
        // Vert 0 stays in bone 0's bucket; verts 1 and 2 land in
        // bone 1's enormous span, so bone 2's bucket is never reached.
        // The `mtxn` partition agrees, so no mismatch is recorded.
        assert_eq!(s.corners()[0].bone, 0);
        assert_eq!(s.corners()[1].bone, 1);
        assert_eq!(s.corners()[2].bone, 1);
    }

    #[test]
    fn skin_deform_bounds_world_input() {
        let s = skin(MOD_FLAT);
        assert_eq!(
            s.deform(&[(Vec3::ZERO, Quat::IDENTITY); 2]).unwrap_err(),
            PedDeformError::MissingBones { needed: 3, got: 2 }
        );
        let mut w = vec![(Vec3::ZERO, Quat::IDENTITY); 3];
        w[1].0 = Vec3::NAN;
        assert_eq!(
            s.deform(&w).unwrap_err(),
            PedDeformError::NonFiniteTransform { bone: 1 }
        );
    }
}
