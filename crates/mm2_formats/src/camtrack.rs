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
    /// `MinMaxOn` — authored gate on the `MinDist`/`MaxDist` clamp.
    pub min_max_on: Option<f32>,
    /// `TrackBreak` — authored flag for the boom breaking loose on
    /// hard manoeuvres; exact behaviour unrecovered (surfaced, not
    /// consumed).
    pub track_break: Option<f32>,
    /// `MinDist` — boom distance floor, metres. `0` on some records
    /// (e.g. the bus's near view).
    pub min_dist: Option<f32>,
    /// `MaxDist` — boom distance cap, metres. The speed window drives
    /// the boom toward it.
    pub max_dist: Option<f32>,
    /// `MinSpeed`/`MaxSpeed` — vehicle-speed window (m/s) the boom
    /// extension scales over.
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
        // Approach/dynamics tuning stays verbatim in extras.
        assert!(s.extra_fields.iter().any(|f| f == "AppInc"));
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
}
