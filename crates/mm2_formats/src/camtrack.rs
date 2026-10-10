//! Decoder for the authored chase-camera records
//! `tune/camera/<car>_{near,far,ind}.camtrackcs` — the `camTrackCS`
//! parameter block every stock player vehicle ships once per view:
//!
//! ```text
//! type: a
//! camTrackCS {
//!   Offset 0.000000 1.000000 4.060000
//!   CollideType 1
//!   MinMaxOn 1
//!   TrackBreak 0
//!   MinAppXZPos 1.500000
//!   MaxAppXZPos 8.000000
//!   MinSpeed 0.000000
//!   MaxSpeed 15.350000
//!   ...
//!   TrackTo 0.000000 1.700000 0.000000
//!   MaxDist 5.300000
//!   MinDist 3.950000
//!   BlendTime 1.200000
//!   CameraFOV 70.000000
//!   CameraNear 0.500000
//!   CameraFar 600.000000
//! }
//! ```
//!
//! The recovered class (mm2hook `src/modules/mmcity/camera.h`) carries
//! `PreApproach`/`UpdateTrack`/`UpdateCar`/`UpdateHill`/`Collide`/
//! `SwingToRear` — an authored boom rig with distance bounds, a speed
//! window, hill pitch, steering swing, a delayed reverse view and
//! view-entry approach tuning. Only the fields the runtime consumes
//! (or that bound them) are surfaced typed here; the approach/dynamics
//! cluster (`AppInc`, `FrontRate`, `HillMin`, `ReverseOn`, ...) stays
//! as retained names in `extra_fields` — its semantics are
//! unrecovered, so nothing here asserts they are the originals' exact
//! meaning.
//!
//! `_near` and `_far` are the two chase views the `C` key cycles
//! through (HUD-3). `_ind` is a third per-car variant the original
//! loads for a different context — unverified, not bound.

use crate::tune::{TuneEntry, TuneFile};

/// A decoded `camTrackCS` record — one authored chase lens.
#[derive(Debug, Clone, Default)]
pub struct TrackCamSpec {
    /// `type:` header tag verbatim (`a` on stock files).
    pub type_tag: Option<String>,
    /// `Offset` — boom anchor in car space, metres. `+Z` is rearward
    /// (behind the car), `+Y` up; its length is the rest distance.
    pub offset: Option<[f32; 3]>,
    /// `CollideType` — nonzero when the boom collides with world
    /// geometry (every stock record authors `1`).
    pub collide_type: Option<f32>,
    /// `MinMaxOn` — nonzero enables the original's ±5 m vertical
    /// ground/ceiling clamp of the eye. It does *not* gate the
    /// `MinDist`/`MaxDist` clamp (recovered runtime, UNK-36,
    /// `docs/research/camtrack.md`); the app does not bind the vertical
    /// clamp yet.
    pub min_max_on: Option<f32>,
    /// `TrackBreak` — authored flag for the boom breaking loose on
    /// hard manoeuvres; exact behaviour unrecovered (surfaced, not
    /// consumed).
    pub track_break: Option<f32>,
    /// `MinDist` — boom distance floor, metres. `0` on some records
    /// (e.g. the bus's near view).
    pub min_dist: Option<f32>,
    /// `MaxDist` — boom distance cap, metres; `MinDist`/`MaxDist` clamp
    /// the eye–aim distance after the approach (UNK-36).
    pub max_dist: Option<f32>,
    /// `MinSpeed`/`MaxSpeed` — vehicle-speed window (m/s) the follow
    /// rate `MaxAppXZPos`→`MinAppXZPos` lerps over (UNK-36).
    pub min_speed: Option<f32>,
    pub max_speed: Option<f32>,
    /// `TrackTo` — aim point in car space (metres; `+Y` up).
    pub track_to: Option<[f32; 3]>,
    /// `LookAbove`/`LookAt`/`VertOffset` — extra aim tuning whose exact
    /// composition with `TrackTo` is unrecovered (surfaced verbatim).
    pub look_above: Option<f32>,
    /// `LookAt`.
    pub look_at: Option<f32>,
    /// `VertOffset`.
    pub vert_offset: Option<f32>,
    /// `BlendTime` — view-switch blend seconds when entering this view.
    pub blend_time: Option<f32>,
    /// `BlendGoal`.
    pub blend_goal: Option<f32>,
    /// `CameraFOV` — degrees.
    pub camera_fov: Option<f32>,
    /// `CameraNear`/`CameraFar` — clip distances, metres.
    pub camera_near: Option<f32>,
    /// `CameraFar`.
    pub camera_far: Option<f32>,
    /// The approach cluster (`camAppCS`/`camTrackCS` follow dynamics,
    /// recovered UNK-36, `docs/research/camtrack.md` §1–2): the live
    /// follow-rate window and slew, the per-axis rates, and the
    /// `AppPosMin` knee / `AppApp` low-pass of the eye approach.
    pub approach_on: Option<f32>,
    /// `AppAppOn`.
    pub app_app_on: Option<f32>,
    /// `AppYPos` — vertical follow rate (1/s).
    pub app_y_pos: Option<f32>,
    /// `AppXZPos` — the record's initial horizontal follow rate; the
    /// runtime rewrites it every frame from the speed lerp.
    pub app_xz_pos: Option<f32>,
    /// `AppApp` — per-step low-pass factor on the approach rate state.
    pub app_app: Option<f32>,
    /// `AppPosMin` — soft-knee distance of the approach.
    pub app_pos_min: Option<f32>,
    /// `MinAppXZPos`/`MaxAppXZPos` — follow rate at/above `MaxSpeed`
    /// and at/below `MinSpeed`.
    pub min_app_xz_pos: Option<f32>,
    /// `MaxAppXZPos`.
    pub max_app_xz_pos: Option<f32>,
    /// `AppInc`/`AppDec` — follow-rate slew up/down, per second.
    pub app_inc: Option<f32>,
    /// `AppDec`.
    pub app_dec: Option<f32>,
    /// Field names outside the consumed schema (the approach/dynamics
    /// cluster), kept for diagnosis — values stay in the record.
    pub extra_fields: Vec<String>,
}

/// Decode failure detail.
#[derive(Debug)]
pub struct CamTrackError(pub String);

impl std::fmt::Display for CamTrackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CamTrackError {}

fn scalar(block: &crate::tune::TuneBlock, name: &str) -> Option<f32> {
    block
        .field_ci(name)
        .and_then(|f| f.values.first()?.number.map(|n| n as f32))
}

/// `CameraFOV` is authored in degrees; only the open `(0, 180)`
/// interval builds a drawable perspective projection — `0` or a
/// negative value collapses it to an infinite clip matrix, `180` or
/// more inverts it, and `nan`/`inf` poison it (designed bound; the
/// original's own guard is unrecovered). Shared by the `camTrackCS`
/// and `camPovCS` decoders.
pub(crate) fn drawable_fov(f: f32) -> bool {
    f.is_finite() && f > 0.0 && f < 180.0
}

/// Designed magnitude bound for the record family's other typed fields
/// (implementation choice — the original's own guard, if any, is
/// unrecovered). Component finiteness alone leaves an overflow
/// residual: `3e38 + 3e38`, a `(max − min) * frac` sweep over ±3e38
/// bounds, and the intermediate product sums inside `Quat * Vec3` all
/// reach `inf` from finite inputs, so a field also reads unauthored
/// past this bound. `1e6` sits orders above anything authored — the
/// largest retail value in the family is `CameraFar 1330` — and
/// orders below `f32::MAX`, so the composed transforms, projections
/// and sweeps cannot overflow. The `.mtx` part-transform record shares
/// the bound: its `origin`/`pivot`/`bounds_*` fields feed the same
/// `attach`/`pivot + offset` compositions through `build_model`.
pub const USABLE_BOUND: f32 = 1e6;

/// A scalar is usable only when finite *and* within [`USABLE_BOUND`].
pub fn usable_f32(v: f32) -> bool {
    v.is_finite() && v.abs() <= USABLE_BOUND
}

/// A scalar field reads `Some` only when authored *and* [`usable_f32`]
/// — `nan`/`inf`/overflowing literals and beyond-bound values read as
/// unauthored so consumers take their designed defaults;
/// [`validate`](TrackCamSpec::validate) names the field instead of
/// repairing it.
pub fn usable1(v: Option<f32>) -> Option<f32> {
    v.filter(|f| usable_f32(*f))
}

/// Every component [`usable_f32`] — the gate for authored vectors that
/// feed transforms and camera math (`Offset`, `TrackTo`, the
/// `_dash.asnode` placements, the `.mtx` part transforms). Shared by
/// the `camTrackCS`, `camPovCS` and `asNode` decoders, the `Mtx`
/// record's readers in `build_model`, and their app-side readers.
pub fn usable3(v: &[f32; 3]) -> bool {
    v.iter().all(|c| usable_f32(*c))
}

/// [`usable3`] as an `Option` filter — see [`usable1`].
pub fn usable_vec(v: Option<[f32; 3]>) -> Option<[f32; 3]> {
    v.filter(usable3)
}

/// The `validate` wording for a vec3 field — `None` when usable.
/// Shared across the family's decoders.
pub(crate) fn vec_issue(name: &str, v: &[f32; 3]) -> Option<String> {
    if !v.iter().all(|c| c.is_finite()) {
        Some(format!("{name} {v:?} is not finite"))
    } else if !usable3(v) {
        Some(format!(
            "{name} {v:?} exceeds the usable bound ±{USABLE_BOUND}"
        ))
    } else {
        None
    }
}

/// [`vec_issue`] for a scalar field.
pub(crate) fn scalar_issue(name: &str, v: f32) -> Option<String> {
    if !v.is_finite() {
        Some(format!("{name} {v} is not finite"))
    } else if !usable_f32(v) {
        Some(format!(
            "{name} {v} exceeds the usable bound ±{USABLE_BOUND}"
        ))
    } else {
        None
    }
}

fn vec3(block: &crate::tune::TuneBlock, name: &str) -> Option<[f32; 3]> {
    let f = block.field_ci(name)?;
    let mut v = [0.0; 3];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = f.values.get(i)?.number.map(|n| n as f32)?;
    }
    Some(v)
}

impl TrackCamSpec {
    /// Decode a `tune/camera/<name>.camtrackcs` record (block
    /// `camTrackCS`). Sparse records are tolerated — `vpcoop2k_far`
    /// omits the whole `Reverse*` cluster, for instance.
    pub fn parse(input: &str) -> Result<Self, CamTrackError> {
        let file = TuneFile::parse(input).map_err(|e| CamTrackError(e.to_string()))?;
        let root = &file.root;
        if !root.name.eq_ignore_ascii_case("camTrackCS") {
            return Err(CamTrackError(format!(
                "expected a camTrackCS block, found {:?}",
                root.name
            )));
        }
        const KNOWN: &[&str] = &[
            "Offset",
            "CollideType",
            "MinMaxOn",
            "TrackBreak",
            "MinDist",
            "MaxDist",
            "MinSpeed",
            "MaxSpeed",
            "TrackTo",
            "LookAbove",
            "LookAt",
            "VertOffset",
            "BlendTime",
            "BlendGoal",
            "CameraFOV",
            "CameraNear",
            "CameraFar",
            "ApproachOn",
            "AppAppOn",
            "AppYPos",
            "AppXZPos",
            "AppApp",
            "AppPosMin",
            "MinAppXZPos",
            "MaxAppXZPos",
            "AppInc",
            "AppDec",
        ];
        Ok(TrackCamSpec {
            type_tag: file.type_tag.clone(),
            offset: vec3(root, "Offset"),
            collide_type: scalar(root, "CollideType"),
            min_max_on: scalar(root, "MinMaxOn"),
            track_break: scalar(root, "TrackBreak"),
            min_dist: scalar(root, "MinDist"),
            max_dist: scalar(root, "MaxDist"),
            min_speed: scalar(root, "MinSpeed"),
            max_speed: scalar(root, "MaxSpeed"),
            track_to: vec3(root, "TrackTo"),
            look_above: scalar(root, "LookAbove"),
            look_at: scalar(root, "LookAt"),
            vert_offset: scalar(root, "VertOffset"),
            blend_time: scalar(root, "BlendTime"),
            blend_goal: scalar(root, "BlendGoal"),
            camera_fov: scalar(root, "CameraFOV"),
            camera_near: scalar(root, "CameraNear"),
            camera_far: scalar(root, "CameraFar"),
            approach_on: scalar(root, "ApproachOn"),
            app_app_on: scalar(root, "AppAppOn"),
            app_y_pos: scalar(root, "AppYPos"),
            app_xz_pos: scalar(root, "AppXZPos"),
            app_app: scalar(root, "AppApp"),
            app_pos_min: scalar(root, "AppPosMin"),
            min_app_xz_pos: scalar(root, "MinAppXZPos"),
            max_app_xz_pos: scalar(root, "MaxAppXZPos"),
            app_inc: scalar(root, "AppInc"),
            app_dec: scalar(root, "AppDec"),
            extra_fields: root
                .entries
                .iter()
                .filter_map(|e| match e {
                    TuneEntry::Field(f)
                        if !KNOWN.iter().any(|k| f.name.eq_ignore_ascii_case(k)) =>
                    {
                        Some(f.name.clone())
                    }
                    TuneEntry::Block(b) => Some(format!("block {}", b.name)),
                    _ => None,
                })
                .collect(),
        })
    }

    /// `CameraFOV` in degrees when authored *and* drawable — a
    /// non-finite or out-of-range value reads `None` so consumers
    /// take their designed default instead of building a degenerate
    /// projection. The raw field stays verbatim; [`Self::validate`]
    /// reports it.
    pub fn camera_fov_deg(&self) -> Option<f32> {
        self.camera_fov.filter(|&f| drawable_fov(f))
    }

    /// `Offset` when authored *and* [`usable3`] — a `nan`/`inf`
    /// component would poison the boom's rest length (and through it
    /// the whole chase transform), and a beyond-bound component would
    /// overflow the composed pose, so it reads unauthored instead. The
    /// raw field stays verbatim; [`Self::validate`] names it.
    pub fn offset_vec(&self) -> Option<[f32; 3]> {
        usable_vec(self.offset)
    }

    /// `TrackTo` when authored *and* [`usable3`] — see
    /// [`Self::offset_vec`]. An astronomical-but-finite aim would
    /// overflow `veh_rot * aim` into a non-finite look target.
    pub fn track_to_vec(&self) -> Option<[f32; 3]> {
        usable_vec(self.track_to)
    }

    /// `MinDist`/`MaxDist`/`MinSpeed`/`MaxSpeed`/`CameraNear`/
    /// `CameraFar` when authored *and* [`usable_f32`] — the non-finite
    /// reads that a `.max()` sink would silently coerce (a `nan`
    /// `CameraFar` becomes a 1 m far plane) instead read unauthored,
    /// as does a beyond-bound value that could overflow the composed
    /// boom length. [`Self::validate`] names the field.
    pub fn min_dist_m(&self) -> Option<f32> {
        usable1(self.min_dist)
    }

    /// [`Self::min_dist_m`] for `MaxDist`.
    pub fn max_dist_m(&self) -> Option<f32> {
        usable1(self.max_dist)
    }

    /// [`Self::min_dist_m`] for `MinSpeed`.
    pub fn min_speed_mps(&self) -> Option<f32> {
        usable1(self.min_speed)
    }

    /// [`Self::min_dist_m`] for `MaxSpeed`.
    pub fn max_speed_mps(&self) -> Option<f32> {
        usable1(self.max_speed)
    }

    /// The near plane `Midtown2.exe` actually uses for a loaded
    /// `camTrackCS`, metres: the class's post-load virtual (`0x51dad0`)
    /// stores 0.5 into `CameraNear` after every successful parse, so the
    /// authored value is dead data (verified_original — UNK-37).
    pub const RUNTIME_NEAR_M: f32 = 0.5;

    /// [`Self::min_dist_m`] for `CameraNear`.
    pub fn camera_near_m(&self) -> Option<f32> {
        usable1(self.camera_near)
    }

    /// [`Self::min_dist_m`] for `CameraFar`.
    pub fn camera_far_m(&self) -> Option<f32> {
        usable1(self.camera_far)
    }

    /// `ApproachOn` as an authored flag (constructor default 1 when
    /// absent or unusable — `0x51d750`, UNK-36).
    pub fn approach_enabled(&self) -> bool {
        usable1(self.approach_on).is_none_or(|v| v != 0.0)
    }

    /// `AppAppOn` as an authored flag (constructor default 1).
    pub fn app_app_enabled(&self) -> bool {
        usable1(self.app_app_on).is_none_or(|v| v != 0.0)
    }

    /// `CollideType` as the authored gate: nonzero *and*
    /// [`usable_f32`]. A `nan` flag reads `!= 0.0` — true — so the
    /// unguarded read would silently enable the occlusion pull-in; an
    /// unusable value reads unauthored (off) and [`Self::validate`]
    /// names it.
    pub fn collides(&self) -> bool {
        usable1(self.collide_type).is_some_and(|c| c != 0.0)
    }

    /// `MinMaxOn` as an authored flag — same unusable-field rule as
    /// [`Self::collides`]. What it gates in the original is the vertical
    /// eye clamp, not the distance clamp (UNK-36).
    pub fn min_max_gated(&self) -> bool {
        usable1(self.min_max_on).is_some_and(|v| v != 0.0)
    }

    /// Record-level problems a loader should report — every typed
    /// field must be finite and within [`USABLE_BOUND`], or absent
    /// (`CameraFOV` additionally must sit inside the drawable range).
    /// Reported, never silently repaired: the raw fields stay verbatim
    /// on the record.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for (name, v) in [("Offset", self.offset), ("TrackTo", self.track_to)] {
            if let Some(v) = v {
                issues.extend(vec_issue(name, &v));
            }
        }
        for (name, v) in [
            ("CollideType", self.collide_type),
            ("MinMaxOn", self.min_max_on),
            ("TrackBreak", self.track_break),
            ("MinDist", self.min_dist),
            ("MaxDist", self.max_dist),
            ("MinSpeed", self.min_speed),
            ("MaxSpeed", self.max_speed),
            ("LookAbove", self.look_above),
            ("LookAt", self.look_at),
            ("VertOffset", self.vert_offset),
            ("BlendTime", self.blend_time),
            ("BlendGoal", self.blend_goal),
            ("CameraNear", self.camera_near),
            ("CameraFar", self.camera_far),
            ("ApproachOn", self.approach_on),
            ("AppAppOn", self.app_app_on),
            ("AppYPos", self.app_y_pos),
            ("AppXZPos", self.app_xz_pos),
            ("AppApp", self.app_app),
            ("AppPosMin", self.app_pos_min),
            ("MinAppXZPos", self.min_app_xz_pos),
            ("MaxAppXZPos", self.max_app_xz_pos),
            ("AppInc", self.app_inc),
            ("AppDec", self.app_dec),
        ] {
            if let Some(v) = v {
                issues.extend(scalar_issue(name, v));
            }
        }
        if let Some(f) = self.camera_fov
            && !drawable_fov(f)
        {
            issues.push(format!(
                "CameraFOV {f} is outside the drawable (0, 180) degree range"
            ));
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A representative `_near.camtrackcs` — field set and values as
    // authored for the VW New Beetle (`vpbug_near`).
    const NEAR: &str = "type: a\ncamTrackCS {\n  Offset 0.000000 1.000000 4.060000\n  CollideType 1\n  MinMaxOn 1\n  TrackBreak 0\n  MinAppXZPos 1.500000\n  MaxAppXZPos 8.000000\n  MinSpeed 0.000000\n  MaxSpeed 15.350000\n  AppInc 6.650000\n  AppDec 3.350000\n  MinHardSteer 0.800000\n  DriftDelay 0.300000\n  VertOffset -0.300000\n  FrontRate 0.550000\n  RearRate 0.500000\n  FlipDelay 0.500000\n  SteerOn 0\n  SteerMin 0.500000\n  SteerAmt 3.500000\n  HillMin -0.280000\n  HillMax 0.280000\n  HillLerp 0.050000\n  ReverseOn 1\n  RevDelay 2.000000\n  RevOnApp 2.000000\n  RevOffApp 4.000000\n  ApproachOn 1\n  AppAppOn 1\n  AppRot 30.000000\n  AppXRot 10.000000\n  AppYPos 4.000000\n  AppXZPos 8.000000\n  AppApp 0.700000\n  AppRotMin 0.010000\n  AppPosMin 0.250000\n  LookAbove -0.757500\n  TrackTo 0.000000 1.700000 0.000000\n  MaxDist 5.300000\n  MinDist 3.950000\n  LookAt 1.000000\n  BlendTime 1.200000\n  BlendGoal 1.000000\n  CameraFOV 70.000000\n  CameraNear 0.500000\n  CameraFar 600.000000\n}\n";

    const POV: &str = "type: a\ncamPovCS {\n  Offset 0.000000 1.190000 -0.551900\n}\n";

    #[test]
    fn parses_track_cam_spec() {
        let s = TrackCamSpec::parse(NEAR).unwrap();
        assert_eq!(s.offset.unwrap(), [0.0, 1.0, 4.06]);
        assert_eq!(s.track_to.unwrap(), [0.0, 1.7, 0.0]);
        assert_eq!(s.collide_type.unwrap(), 1.0);
        assert_eq!(s.min_max_on.unwrap(), 1.0);
        assert_eq!(s.min_dist.unwrap(), 3.95);
        assert_eq!(s.max_dist.unwrap(), 5.3);
        assert_eq!(s.min_speed.unwrap(), 0.0);
        assert_eq!(s.max_speed.unwrap(), 15.35);
        assert_eq!(s.blend_time.unwrap(), 1.2);
        assert_eq!(s.camera_fov.unwrap(), 70.0);
        assert_eq!(s.camera_near.unwrap(), 0.5);
        assert_eq!(s.camera_far.unwrap(), 600.0);
        assert_eq!(s.app_inc, Some(6.65));
        assert_eq!(s.app_dec, Some(3.35));
        assert_eq!(s.min_app_xz_pos, Some(1.5));
        assert_eq!(s.max_app_xz_pos, Some(8.0));
        assert_eq!(
            (s.app_y_pos, s.app_app, s.app_pos_min),
            (Some(4.0), Some(0.7), Some(0.25))
        );
        assert!(s.approach_enabled() && s.app_app_enabled());
        // The rest of the dynamics cluster stays verbatim in extras.
        assert!(s.extra_fields.iter().any(|f| f == "ReverseOn"));
        assert!(s.extra_fields.iter().any(|f| f == "HillLerp"));
    }

    #[test]
    fn rejects_wrong_block() {
        assert!(TrackCamSpec::parse(POV).is_err());
    }

    #[test]
    fn tolerates_sparse_records() {
        // `vpcoop2k_far` ships no `Reverse*` cluster — sparse is legal.
        let s = TrackCamSpec::parse(
            "type: a\ncamTrackCS {\n  Offset 0.0 1.5 5.6\n  TrackBreak 1\n  MaxDist 6.5\n  MinDist 2.6\n  CameraFOV 70.1\n}\n",
        )
        .unwrap();
        assert_eq!(s.offset.unwrap(), [0.0, 1.5, 5.6]);
        assert_eq!(s.track_break.unwrap(), 1.0);
        assert!(s.track_to.is_none());
        assert!(s.min_speed.is_none());
        assert!(s.collide_type.is_none());
    }

    #[test]
    fn short_offset_is_treated_absent() {
        // A truncated vector must not partially decode.
        let s = TrackCamSpec::parse("type: a\ncamTrackCS {\n  Offset 0.0 1.0\n}\n").unwrap();
        assert!(s.offset.is_none());
    }

    /// A `CameraFOV` outside the drawable `(0, 180)` degree range —
    /// or non-finite — stays verbatim on the record but is named by
    /// `validate` and reads as unauthored through `camera_fov_deg`,
    /// so consumers take their designed lens instead of a degenerate
    /// projection (authored-numbers audit finding 11).
    #[test]
    fn undrawable_camera_fov_is_named_and_reads_unauthored() {
        for bad in ["nan", "inf", "-inf", "0.0", "-30.0", "180.0", "720.0"] {
            let text = format!("type: a\ncamTrackCS {{\n  CameraFOV {bad}\n  CameraNear 0.5\n}}\n");
            let s = TrackCamSpec::parse(&text).unwrap();
            assert_eq!(s.camera_fov_deg(), None, "{bad}");
            assert_eq!(s.validate().len(), 1, "{bad}");
            assert!(s.validate()[0].contains("CameraFOV"), "{}", s.validate()[0]);
        }
        // Drawable authored values and an absent field stay clean.
        let s =
            TrackCamSpec::parse("type: a\ncamTrackCS {\n  CameraFOV 179.9\n  CameraNear 0.5\n}\n")
                .unwrap();
        assert_eq!(s.camera_fov_deg(), Some(179.9));
        assert!(s.validate().is_empty());
        let s = TrackCamSpec::parse("type: a\ncamTrackCS {\n  CameraNear 0.5\n}\n").unwrap();
        assert_eq!(s.camera_fov_deg(), None);
        assert!(s.validate().is_empty());
    }

    /// The rest of the typed schema follows the `camera_fov_deg`
    /// contract: a `nan`/`inf` value stays verbatim on the record, is
    /// named by `validate`, and reads unauthored through the field
    /// accessors — a `nan` boom `Offset` never reaches the chase
    /// transform, and a `nan` flag reads off rather than silently
    /// `!= 0.0` truthy.
    #[test]
    fn non_finite_fields_are_named_and_read_unauthored() {
        let s = TrackCamSpec::parse(
            "type: a\ncamTrackCS {\n  Offset 0.0 nan 4.0\n  TrackTo inf 1.0 0.0\n  CollideType nan\n  MinMaxOn nan\n  TrackBreak inf\n  MinDist -inf\n  MaxDist 1e999\n  MinSpeed nan\n  MaxSpeed inf\n  LookAbove nan\n  LookAt inf\n  VertOffset nan\n  BlendTime -inf\n  BlendGoal nan\n  CameraNear nan\n  CameraFar inf\n}\n",
        )
        .unwrap();
        // Verbatim on the record — reported, not repaired.
        assert!(s.offset.unwrap()[1].is_nan());
        assert_eq!(s.offset_vec(), None);
        assert_eq!(s.track_to_vec(), None);
        assert!(!s.collides(), "nan CollideType must not read as on");
        assert!(!s.min_max_gated(), "nan MinMaxOn must not read as on");
        assert_eq!(s.min_dist_m(), None);
        assert_eq!(s.max_dist_m(), None);
        assert_eq!(s.min_speed_mps(), None);
        assert_eq!(s.max_speed_mps(), None);
        assert_eq!(s.camera_near_m(), None);
        assert_eq!(s.camera_far_m(), None);
        let issues = s.validate();
        let named: Vec<&str> = issues
            .iter()
            .map(|i| i.split(' ').next().unwrap())
            .collect();
        for name in [
            "Offset",
            "TrackTo",
            "CollideType",
            "MinMaxOn",
            "TrackBreak",
            "MinDist",
            "MaxDist",
            "MinSpeed",
            "MaxSpeed",
            "LookAbove",
            "LookAt",
            "VertOffset",
            "BlendTime",
            "BlendGoal",
            "CameraNear",
            "CameraFar",
        ] {
            assert!(named.contains(&name), "{name} not named: {issues:?}");
        }

        // Finite authored values still bind verbatim.
        let s = TrackCamSpec::parse(NEAR).unwrap();
        assert!(s.validate().is_empty());
        assert_eq!(s.offset_vec().unwrap(), [0.0, 1.0, 4.06]);
        assert_eq!(s.track_to_vec().unwrap(), [0.0, 1.7, 0.0]);
        assert!(s.collides() && s.min_max_gated());
        assert_eq!(s.min_dist_m(), Some(3.95));
        assert_eq!(s.max_dist_m(), Some(5.3));
        assert_eq!(s.min_speed_mps(), Some(0.0));
        assert_eq!(s.max_speed_mps(), Some(15.35));
        assert_eq!(s.camera_near_m(), Some(0.5));
        assert_eq!(s.camera_far_m(), Some(600.0));
    }

    /// The gate also rejects finite-but-overflowing values: a `3e38`
    /// `TrackTo` has finite components yet `veh_rot * aim` can still
    /// reach `inf` inside the quaternion product, and a `3e38`
    /// `MaxDist` overflows `dir * dist` the same way — so past
    /// [`USABLE_BOUND`] a field reads unauthored and `validate` names
    /// it. The bound sits far above anything authored (retail max is
    /// `CameraFar 1330`).
    #[test]
    fn beyond_bound_fields_are_named_and_read_unauthored() {
        let s = TrackCamSpec::parse(
            "type: a\ncamTrackCS {\n  Offset 0.0 2e6 4.0\n  TrackTo 3e38 0.0 0.0\n  CollideType 1e9\n  MinMaxOn 3e38\n  TrackBreak 1e9\n  MinDist -2e6\n  MaxDist 3e38\n  MinSpeed 2e6\n  MaxSpeed 1e9\n  LookAbove 3e38\n  CameraNear 2e6\n  CameraFar 1e7\n}\n",
        )
        .unwrap();
        // Verbatim on the record — reported, not repaired.
        assert_eq!(s.track_to.unwrap()[0], 3e38);
        assert_eq!(s.offset_vec(), None);
        assert_eq!(s.track_to_vec(), None);
        assert!(!s.collides(), "beyond-bound CollideType must read off");
        assert!(!s.min_max_gated());
        assert_eq!(s.min_dist_m(), None);
        assert_eq!(s.max_dist_m(), None);
        assert_eq!(s.min_speed_mps(), None);
        assert_eq!(s.max_speed_mps(), None);
        assert_eq!(s.camera_near_m(), None);
        assert_eq!(s.camera_far_m(), None);
        let issues = s.validate();
        let named: Vec<&str> = issues
            .iter()
            .map(|i| i.split(' ').next().unwrap())
            .collect();
        for name in [
            "Offset",
            "TrackTo",
            "CollideType",
            "MinMaxOn",
            "TrackBreak",
            "MinDist",
            "MaxDist",
            "MinSpeed",
            "MaxSpeed",
            "LookAbove",
            "CameraNear",
            "CameraFar",
        ] {
            assert!(named.contains(&name), "{name} not named: {issues:?}");
        }
        assert!(
            issues
                .iter()
                .all(|i| i.contains("exceeds the usable bound")),
            "finite beyond-bound values are named as such: {issues:?}"
        );

        // The bound's own edge: ±USABLE_BOUND binds, past it reads
        // unauthored.
        let edge = |v: f32| {
            TrackCamSpec::parse(&format!(
                "type: a\ncamTrackCS {{\n  MaxDist {v}\n  TrackTo 0.0 {v} 0.0\n}}\n"
            ))
            .unwrap()
        };
        let s = edge(USABLE_BOUND);
        assert_eq!(s.max_dist_m(), Some(USABLE_BOUND));
        assert_eq!(s.track_to_vec().unwrap()[1], USABLE_BOUND);
        assert!(s.validate().is_empty());
        let s = edge(USABLE_BOUND * 1.01);
        assert_eq!(s.max_dist_m(), None);
        assert_eq!(s.track_to_vec(), None);
        assert_eq!(s.validate().len(), 2);
    }
}
