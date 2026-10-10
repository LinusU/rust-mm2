//! Decoders for the authored cockpit/dashboard records:
//! `tune/<car>_dash.asnode` (the `mmDashView` parameter block) and
//! `tune/camera/<car>_dash.campovcs` (the `camPovCS` cockpit camera spec).
//!
//! Every stock player vehicle ships both alongside `geometry/<car>_dash.pkg`
//! and per-part `geometry/<car>_dash_<part>.mtx` files. The recovered
//! `mmDashView` class (mm2hook `src/modules/mmgame/dash.h`) is an
//! `asLinearCS` node holding a `RadialGauge` per needle (`ValuePtr` /
//! `MaxValuePtr` bind straight into `vehCarSim` state) plus the
//! positions/offsets this file carries:
//!
//! ```text
//! type: a
//! asNode {
//!   DashPos 0.1314 -0.6049 -0.7799
//!   RoofPos 0.0233 -0.4842 -0.8720
//!   WheelPos 0.0860 -0.0121 0.0000
//!   SpeedOffset 0.0700 0.0870 0.1780
//!   SpeedPivotOffset 0.0350 0.0000 0.0000
//!   ...
//!   WheelFact 0.900000
//!   SpeedRotMin 0.000000
//!   SpeedRotMax 3.614002
//!   ...
//! }
//! ```
//!
//! The MM1 counterpart (`MMDASHVIEW`) carried explicit `MaxSpeed`/`MaxRPM`/
//! `MinSpeed` gauge scales and `SpeedPos`-style needle pivots; MM2 dropped
//! the scales from the file (mm2hook's FileIO hook re-adds them because the
//! members still exist — the retail runtime fills them from the carsim, so
//! the speedo/tach full-scale values come from `vehCarSim`, not this file)
//! and moved the needle pivots into the part `.mtx` records (verified:
//! `vpbug_dash_speed_needle.mtx`'s pivot is the quad's authored centre).
//! How `*Offset` composes with the authored geometry — whether each field
//! translates its part, replaces its authored position, or both — is not
//! recovered; the app-side composition is a designed reading (UNK-27).

use crate::tune::{TuneEntry, TuneFile};

/// A decoded `<car>_dash.asnode` record — the `mmDashView` parameters.
#[derive(Debug, Clone, Default)]
pub struct DashSpec {
    /// `type:` header tag verbatim (`a` on stock files).
    pub type_tag: Option<String>,
    /// `DashPos` — the dash cluster's anchor in cockpit-camera space.
    pub dash_pos: Option<[f32; 3]>,
    /// `RoofPos` — the roof strip's anchor (own cluster).
    pub roof_pos: Option<[f32; 3]>,
    /// `WheelPos` — steering-wheel quad placement.
    pub wheel_pos: Option<[f32; 3]>,
    /// `DmgOffset` — damage needle translation.
    pub dmg_offset: Option<[f32; 3]>,
    /// `SpeedOffset` — speedometer needle translation.
    pub speed_offset: Option<[f32; 3]>,
    /// `TachOffset` — tachometer needle translation.
    pub tach_offset: Option<[f32; 3]>,
    /// `DmgPivotOffset` — damage needle pivot nudge.
    pub dmg_pivot_offset: Option<[f32; 3]>,
    /// `SpeedPivotOffset` — speedometer needle pivot nudge.
    pub speed_pivot_offset: Option<[f32; 3]>,
    /// `TachPivotOffset` — tachometer needle pivot nudge.
    pub tach_pivot_offset: Option<[f32; 3]>,
    /// `WheelPivotOffset` — steering-wheel pivot nudge.
    pub wheel_pivot_offset: Option<[f32; 3]>,
    /// `GearPivotOffset` — gear-indicator pivot nudge.
    pub gear_pivot_offset: Option<[f32; 3]>,
    /// `WheelFact` — steering-wheel turn factor per unit input.
    pub wheel_fact: Option<f32>,
    /// `RPMRotMin`/`RPMRotMax` — tach needle sweep, radians.
    pub rpm_rot: Option<(f32, f32)>,
    /// `SpeedRotMin`/`SpeedRotMax` — speedo needle sweep, radians.
    pub speed_rot: Option<(f32, f32)>,
    /// `DamageRotMin`/`DamageRotMax` — damage needle sweep, radians.
    pub damage_rot: Option<(f32, f32)>,
    /// Field names outside the recovered schema, kept for diagnosis.
    pub extra_fields: Vec<String>,
}

/// A decoded `camPovCS` record — the cockpit camera's authored spec.
///
/// Only the fields the view consumes are surfaced; the approach/blend
/// parameters (`ApproachOn`, `AppRot`, `BlendTime`, ...) are transition
/// tuning the original camAI applies while *switching into* the view —
/// unrecovered, preserved in `extra_fields`.
#[derive(Debug, Clone, Default)]
pub struct PovCamSpec {
    /// `type:` header tag verbatim.
    pub type_tag: Option<String>,
    /// `Offset` — eye position in car space (metres; `-Z` is forward).
    pub offset: Option<[f32; 3]>,
    /// `ReverseOffset` — authored rear-facing eye position (the looking-
    /// backward view, e.g. numpad-2). Absent on `camTrackCS` variants.
    pub reverse_offset: Option<[f32; 3]>,
    /// `Pitch` — fixed downward tilt, radians.
    pub pitch: Option<f32>,
    /// `TrackTo` — authored look target; its blend semantics are
    /// unrecovered (kept verbatim).
    pub track_to: Option<[f32; 3]>,
    /// `CameraFOV` — degrees.
    pub camera_fov: Option<f32>,
    /// `CameraNear`/`CameraFar` — clip distances, metres.
    pub camera_near: Option<f32>,
    /// `CameraFar`.
    pub camera_far: Option<f32>,
    /// Field names outside the consumed schema, kept for diagnosis.
    pub extra_fields: Vec<String>,
}

/// Decode failure detail.
#[derive(Debug)]
pub struct DashError(pub String);

impl std::fmt::Display for DashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DashError {}

fn vec3(block: &crate::tune::TuneBlock, name: &str) -> Option<[f32; 3]> {
    let f = block.field_ci(name)?;
    let mut v = [0.0; 3];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = f.values.get(i)?.number.map(|n| n as f32)?;
    }
    Some(v)
}

fn scalar(block: &crate::tune::TuneBlock, name: &str) -> Option<f32> {
    block
        .field_ci(name)
        .and_then(|f| f.values.first()?.number.map(|n| n as f32))
}

fn extras(block: &crate::tune::TuneBlock, known: &[&str]) -> Vec<String> {
    block
        .entries
        .iter()
        .filter_map(|e| match e {
            TuneEntry::Field(f) if !known.iter().any(|k| f.name.eq_ignore_ascii_case(k)) => {
                Some(f.name.clone())
            }
            TuneEntry::Block(b) => Some(format!("block {}", b.name)),
            _ => None,
        })
        .collect()
}

fn rot_pair(block: &crate::tune::TuneBlock, min: &str, max: &str) -> Option<(f32, f32)> {
    Some((scalar(block, min)?, scalar(block, max)?))
}

impl DashSpec {
    /// Decode a `<car>_dash.asnode` record. `input` is the file text.
    pub fn parse(input: &str) -> Result<Self, DashError> {
        let file = TuneFile::parse(input).map_err(|e| DashError(e.to_string()))?;
        let root = &file.root;
        if !root.name.eq_ignore_ascii_case("asNode") {
            return Err(DashError(format!(
                "expected an asNode block, found {:?}",
                root.name
            )));
        }
        const KNOWN: &[&str] = &[
            "DashPos",
            "RoofPos",
            "WheelPos",
            "DmgOffset",
            "SpeedOffset",
            "TachOffset",
            "DmgPivotOffset",
            "SpeedPivotOffset",
            "TachPivotOffset",
            "WheelPivotOffset",
            "GearPivotOffset",
            "WheelFact",
            "RPMRotMin",
            "RPMRotMax",
            "SpeedRotMin",
            "SpeedRotMax",
            "DamageRotMin",
            "DamageRotMax",
        ];
        Ok(DashSpec {
            type_tag: file.type_tag.clone(),
            dash_pos: vec3(root, "DashPos"),
            roof_pos: vec3(root, "RoofPos"),
            wheel_pos: vec3(root, "WheelPos"),
            dmg_offset: vec3(root, "DmgOffset"),
            speed_offset: vec3(root, "SpeedOffset"),
            tach_offset: vec3(root, "TachOffset"),
            dmg_pivot_offset: vec3(root, "DmgPivotOffset"),
            speed_pivot_offset: vec3(root, "SpeedPivotOffset"),
            tach_pivot_offset: vec3(root, "TachPivotOffset"),
            wheel_pivot_offset: vec3(root, "WheelPivotOffset"),
            gear_pivot_offset: vec3(root, "GearPivotOffset"),
            wheel_fact: scalar(root, "WheelFact"),
            rpm_rot: rot_pair(root, "RPMRotMin", "RPMRotMax"),
            speed_rot: rot_pair(root, "SpeedRotMin", "SpeedRotMax"),
            damage_rot: rot_pair(root, "DamageRotMin", "DamageRotMax"),
            extra_fields: extras(root, KNOWN),
        })
    }

    /// Record-level problems a loader should report — every typed
    /// field must be finite and within
    /// [`USABLE_BOUND`](crate::camtrack::USABLE_BOUND), or absent.
    /// Reported, never silently repaired: the raw fields stay verbatim
    /// on the record and the app reads an unusable value as unauthored
    /// (the needle parks, the designed placement/factor stands in).
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for (name, v) in [
            ("DashPos", self.dash_pos),
            ("RoofPos", self.roof_pos),
            ("WheelPos", self.wheel_pos),
            ("DmgOffset", self.dmg_offset),
            ("SpeedOffset", self.speed_offset),
            ("TachOffset", self.tach_offset),
            ("DmgPivotOffset", self.dmg_pivot_offset),
            ("SpeedPivotOffset", self.speed_pivot_offset),
            ("TachPivotOffset", self.tach_pivot_offset),
            ("WheelPivotOffset", self.wheel_pivot_offset),
            ("GearPivotOffset", self.gear_pivot_offset),
        ] {
            if let Some(v) = v {
                issues.extend(crate::camtrack::vec_issue(name, &v));
            }
        }
        if let Some(f) = self.wheel_fact {
            issues.extend(crate::camtrack::scalar_issue("WheelFact", f));
        }
        for (name, pair) in [
            ("RPMRotMin/RPMRotMax", self.rpm_rot),
            ("SpeedRotMin/SpeedRotMax", self.speed_rot),
            ("DamageRotMin/DamageRotMax", self.damage_rot),
        ] {
            if let Some((lo, hi)) = pair {
                if !(lo.is_finite() && hi.is_finite()) {
                    issues.push(format!("{name} ({lo}, {hi}) is not finite"));
                } else if !(crate::camtrack::usable_f32(lo) && crate::camtrack::usable_f32(hi)) {
                    issues.push(format!(
                        "{name} ({lo}, {hi}) exceeds the usable bound ±{}",
                        crate::camtrack::USABLE_BOUND
                    ));
                }
            }
        }
        issues
    }
}

impl PovCamSpec {
    /// Decode a `tune/camera/<name>.campovcs` record (block `camPovCS`).
    pub fn parse(input: &str) -> Result<Self, DashError> {
        let file = TuneFile::parse(input).map_err(|e| DashError(e.to_string()))?;
        let root = &file.root;
        if !root.name.eq_ignore_ascii_case("camPovCS") {
            return Err(DashError(format!(
                "expected a camPovCS block, found {:?}",
                root.name
            )));
        }
        const KNOWN: &[&str] = &[
            "Offset",
            "ReverseOffset",
            "Pitch",
            "TrackTo",
            "CameraFOV",
            "CameraNear",
            "CameraFar",
        ];
        Ok(PovCamSpec {
            type_tag: file.type_tag.clone(),
            offset: vec3(root, "Offset"),
            reverse_offset: vec3(root, "ReverseOffset"),
            pitch: scalar(root, "Pitch"),
            track_to: vec3(root, "TrackTo"),
            camera_fov: scalar(root, "CameraFOV"),
            camera_near: scalar(root, "CameraNear"),
            camera_far: scalar(root, "CameraFar"),
            extra_fields: extras(root, KNOWN),
        })
    }

    /// `CameraFOV` in degrees when authored *and* drawable — a
    /// non-finite or out-of-range value reads `None` so consumers
    /// take their designed default instead of building a degenerate
    /// projection (the [`crate::camtrack::TrackCamSpec`] contract).
    /// The raw field stays verbatim; [`Self::validate`] reports it.
    pub fn camera_fov_deg(&self) -> Option<f32> {
        self.camera_fov
            .filter(|&f| crate::camtrack::drawable_fov(f))
    }

    /// `Offset` when authored *and*
    /// [`usable3`](crate::camtrack::usable3) — a `nan`/`inf` component
    /// would poison the cockpit eye and every anchor derived from it
    /// (and the mirror strip's eye), and a beyond-bound component would
    /// overflow the composed anchors, so it reads unauthored. The raw
    /// field stays verbatim; [`Self::validate`] names it.
    pub fn offset_vec(&self) -> Option<[f32; 3]> {
        crate::camtrack::usable_vec(self.offset)
    }

    /// `ReverseOffset` when authored *and* usable — see
    /// [`Self::offset_vec`].
    pub fn reverse_offset_vec(&self) -> Option<[f32; 3]> {
        crate::camtrack::usable_vec(self.reverse_offset)
    }

    /// `Pitch` when authored *and* usable — a `nan` tilt poisons the
    /// cockpit camera rotation, so it reads unauthored (level).
    /// [`Self::validate`] names it.
    pub fn pitch_rad(&self) -> Option<f32> {
        crate::camtrack::usable1(self.pitch)
    }

    /// The near plane `Midtown2.exe` actually uses for a loaded
    /// `camPovCS`, metres. `Load` (`0x4a1110`) calls the class's
    /// post-load virtual after every successful parse, and
    /// `camPovCS`'s (`0x51d6f0`) stores 0.1 into `CameraNear`, so the
    /// authored value is dead data (verified_original — UNK-37,
    /// `docs/research/camtrack.md`).
    pub const RUNTIME_NEAR_M: f32 = 0.1;

    /// `CameraNear` when authored *and* usable — a `nan` survives
    /// `f32::clamp` into the projection, so it reads unauthored.
    /// [`Self::validate`] names it.
    pub fn camera_near_m(&self) -> Option<f32> {
        crate::camtrack::usable1(self.camera_near)
    }

    /// `CameraFar` when authored *and* usable — a `nan` through a
    /// `.max(1.0)` sink becomes a 1 m far plane, so it reads
    /// unauthored. [`Self::validate`] names it.
    pub fn camera_far_m(&self) -> Option<f32> {
        crate::camtrack::usable1(self.camera_far)
    }

    /// Record-level problems a loader should report — every typed
    /// field must be finite and within
    /// [`USABLE_BOUND`](crate::camtrack::USABLE_BOUND), or absent
    /// (`CameraFOV` additionally must sit inside the drawable range).
    /// Reported, never silently repaired: the raw fields stay verbatim
    /// on the record.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for (name, v) in [
            ("Offset", self.offset),
            ("ReverseOffset", self.reverse_offset),
            ("TrackTo", self.track_to),
        ] {
            if let Some(v) = v {
                issues.extend(crate::camtrack::vec_issue(name, &v));
            }
        }
        for (name, v) in [
            ("Pitch", self.pitch),
            ("CameraNear", self.camera_near),
            ("CameraFar", self.camera_far),
        ] {
            if let Some(v) = v {
                issues.extend(crate::camtrack::scalar_issue(name, v));
            }
        }
        if let Some(f) = self.camera_fov
            && !crate::camtrack::drawable_fov(f)
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

    const DASH: &str = "type: a\nasNode {\n  DashPos 0.1314 -0.6049 -0.7799\n  RoofPos 0.0233 -0.4842 -0.8720\n  WheelPos 0.0860 -0.0121 0.0000\n  DmgOffset 0.1107 0.1310 0.2305\n  SpeedOffset 0.0700 0.0870 0.1780\n  TachOffset 0.0609 0.1268 0.2272\n  DmgPivotOffset 0.0140 0.0000 0.0000\n  SpeedPivotOffset 0.0350 0.0000 0.0000\n  TachPivotOffset 0.0150 0.0000 0.0000\n  WheelPivotOffset -0.0010 0.0030 0.0000\n  WheelFact 0.900000\n  RPMRotMin 0.000000\n  RPMRotMax 3.408997\n  SpeedRotMin 0.000000\n  SpeedRotMax 3.614002\n  DamageRotMin 0.000000\n  DamageRotMax 3.177000\n  GearPivotOffset 0.0222 0.0157 0.0534\n  BonusField 1.0\n}\n";

    const POV: &str = "type: a\ncamPovCS {\n  Offset 0.000000 1.190000 -0.551900\n  ReverseOffset 0.000000 1.700000 0.750000\n  Pitch 0.020000\n  POVJitterAmp 0.000000\n  TrackTo 0.000000 2.893000 0.000000\n  MaxDist 1.337000\n  MinDist 1.253000\n  BlendTime 1.200000\n  CameraFOV 70.000000\n  CameraNear 0.100000\n  CameraFar 600.000000\n}\n";

    #[test]
    fn parses_dash_spec() {
        let s = DashSpec::parse(DASH).unwrap();
        assert_eq!(s.dash_pos.unwrap(), [0.1314, -0.6049, -0.7799]);
        assert_eq!(s.speed_offset.unwrap(), [0.07, 0.087, 0.178]);
        assert_eq!(s.speed_pivot_offset.unwrap(), [0.035, 0.0, 0.0]);
        assert_eq!(s.wheel_fact.unwrap(), 0.9);
        assert_eq!(s.speed_rot.unwrap(), (0.0, 3.614002));
        assert_eq!(s.rpm_rot.unwrap(), (0.0, 3.408997));
        assert_eq!(s.damage_rot.unwrap(), (0.0, 3.177));
        assert_eq!(s.gear_pivot_offset.unwrap(), [0.0222, 0.0157, 0.0534]);
        assert_eq!(s.extra_fields, ["BonusField"]);
    }

    #[test]
    fn parses_pov_cam_spec() {
        let s = PovCamSpec::parse(POV).unwrap();
        assert_eq!(s.offset.unwrap(), [0.0, 1.19, -0.5519]);
        assert_eq!(s.reverse_offset.unwrap(), [0.0, 1.7, 0.75]);
        assert_eq!(s.pitch.unwrap(), 0.02);
        assert_eq!(s.camera_fov.unwrap(), 70.0);
        assert_eq!(s.camera_near.unwrap(), 0.1);
        assert_eq!(s.camera_far.unwrap(), 600.0);
        // Approach/blend tuning stays verbatim in extras, never dropped.
        assert!(s.extra_fields.iter().any(|f| f == "POVJitterAmp"));
        assert!(s.extra_fields.iter().any(|f| f == "BlendTime"));
    }

    #[test]
    fn rejects_wrong_blocks() {
        assert!(DashSpec::parse(POV).is_err());
        assert!(PovCamSpec::parse(DASH).is_err());
    }

    #[test]
    fn tolerates_sparse_records() {
        let s = DashSpec::parse("type: a\nasNode {\n  DashPos 0.1 0.2 0.3\n}\n").unwrap();
        assert_eq!(s.dash_pos.unwrap(), [0.1, 0.2, 0.3]);
        assert!(s.speed_rot.is_none());
        assert!(s.extra_fields.is_empty());
    }

    /// Every typed `asNode` field must be usable (finite, within
    /// [`USABLE_BOUND`](crate::camtrack::USABLE_BOUND)) or absent — a
    /// `nan`/`inf`/astronomical value stays verbatim on the record but
    /// is named by `validate` so the app can read it as unauthored
    /// (the needle parks, the designed placement stands in) instead of
    /// writing a NaN/`inf` transform.
    #[test]
    fn dash_spec_non_finite_fields_are_named() {
        let s = DashSpec::parse(
            "type: a\nasNode {\n  DashPos 0.1 nan 0.3\n  SpeedPivotOffset 0.0 1e999 0.0\n  WheelFact inf\n  SpeedRotMin 0.0\n  SpeedRotMax nan\n  DamageRotMin -inf\n  DamageRotMax 3.0\n}\n",
        )
        .unwrap();
        // Verbatim on the record — reported, not repaired.
        assert!(s.dash_pos.unwrap()[1].is_nan());
        assert!(s.speed_rot.unwrap().1.is_nan());
        let issues = s.validate();
        let named: Vec<&str> = issues
            .iter()
            .map(|i| i.split(' ').next().unwrap())
            .collect();
        for name in [
            "DashPos",
            "SpeedPivotOffset",
            "WheelFact",
            "SpeedRotMin/SpeedRotMax",
            "DamageRotMin/DamageRotMax",
        ] {
            assert!(named.contains(&name), "{name} not named: {named:?}");
        }
        // The clean record reports nothing.
        assert!(DashSpec::parse(DASH).unwrap().validate().is_empty());
    }

    /// The `camPovCS` fields beyond `CameraFOV` get the same
    /// unauthored-on-non-finite reading: a `nan` `Offset` or `Pitch`
    /// would otherwise poison the cockpit camera transform, and a
    /// `nan` `CameraFar` sinks through `.max(1.0)` into a 1 m far
    /// plane.
    #[test]
    fn pov_cam_spec_non_finite_fields_read_unauthored() {
        let s = PovCamSpec::parse(
            "type: a\ncamPovCS {\n  Offset nan 1.0 0.0\n  ReverseOffset 0.0 inf 0.0\n  Pitch nan\n  CameraNear inf\n  CameraFar nan\n}\n",
        )
        .unwrap();
        assert!(s.offset.unwrap()[0].is_nan()); // verbatim
        assert_eq!(s.offset_vec(), None);
        assert_eq!(s.reverse_offset_vec(), None);
        assert_eq!(s.pitch_rad(), None);
        assert_eq!(s.camera_near_m(), None);
        assert_eq!(s.camera_far_m(), None);
        let issues = s.validate();
        let named: Vec<&str> = issues
            .iter()
            .map(|i| i.split(' ').next().unwrap())
            .collect();
        for name in [
            "Offset",
            "ReverseOffset",
            "Pitch",
            "CameraNear",
            "CameraFar",
        ] {
            assert!(named.contains(&name), "{name} not named: {named:?}");
        }
        // The clean record binds verbatim and reports nothing.
        let s = PovCamSpec::parse(POV).unwrap();
        assert!(s.validate().is_empty());
        assert_eq!(s.offset_vec().unwrap(), [0.0, 1.19, -0.5519]);
        assert_eq!(s.pitch_rad(), Some(0.02));
        assert_eq!(s.camera_near_m(), Some(0.1));
        assert_eq!(s.camera_far_m(), Some(600.0));
    }

    /// Finite-but-overflowing values — `3e38`-class components — get
    /// the same unauthored read: two finite placements summing past
    /// `f32::MAX` would still write an `inf` translation, so the gate
    /// bounds magnitude, not just finiteness
    /// ([`crate::camtrack::USABLE_BOUND`]).
    #[test]
    fn beyond_bound_fields_are_named_and_read_unauthored() {
        let s = DashSpec::parse(
            "type: a\nasNode {\n  DashPos 3e38 -0.6 -0.78\n  SpeedPivotOffset 2e6 0.0 0.0\n  WheelFact 1e9\n  SpeedRotMin 0.0\n  SpeedRotMax 3e38\n}\n",
        )
        .unwrap();
        // Verbatim on the record — reported, not repaired.
        assert_eq!(s.dash_pos.unwrap()[0], 3e38);
        let issues = s.validate();
        let named: Vec<&str> = issues
            .iter()
            .map(|i| i.split(' ').next().unwrap())
            .collect();
        for name in [
            "DashPos",
            "SpeedPivotOffset",
            "WheelFact",
            "SpeedRotMin/SpeedRotMax",
        ] {
            assert!(named.contains(&name), "{name} not named: {named:?}");
        }
        let s = PovCamSpec::parse(
            "type: a\ncamPovCS {\n  Offset 3e38 1.0 0.0\n  ReverseOffset 0.0 2e6 0.0\n  Pitch 1e9\n  CameraFar 3e38\n}\n",
        )
        .unwrap();
        assert_eq!(s.offset_vec(), None);
        assert_eq!(s.reverse_offset_vec(), None);
        assert_eq!(s.pitch_rad(), None);
        assert_eq!(s.camera_far_m(), None);
        assert_eq!(s.validate().len(), 4);
    }
}
