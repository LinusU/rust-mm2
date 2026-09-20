//! Typed decoder for `tune/banger/*.dgbangerdata` records — the authored
//! physical/breakage description of a knockable or breakable object
//! (`dgBangerData { ... }` in the [`crate::tune`] grammar).
//!
//! One record exists per banger name. The name maps to geometry three
//! ways (measured on the retail install, 2026-09-20):
//!
//! - `geometry/<name>.pkg` — a standalone banger prop (street furniture,
//!   `pt_*` markers, `giz_*` animated objects, `wpobj_gold`).
//! - `geometry/<name>.mtx` — a named part of a larger model (vehicle
//!   wheels, dashes, headlights, trailer wheels) whose `.mtx` gives the
//!   attach transform.
//! - `<base>_break<NN>` — an authored breakaway fragment; the fragment
//!   mesh is the `BREAK<NN>_*` chunk inside `geometry/<base>.pkg`
//!   (`NumParts` on the base record counts those chunks).
//!
//! `default.dgbangerdata` is the fallback record; whether the runtime
//! applies it to banger-named props without their own record is
//! unverified. `ImpulseLimit2` ≈ 1e30 on 15 retail records reads as
//! "effectively unbreakable" — inferred, not verified. See
//! `docs/research/banger.md`.

use std::fmt;

use crate::tune::{TuneBlock, TuneEntry, TuneFile};

/// Error while decoding a typed banger record.
#[derive(Debug)]
pub struct BangerError {
    /// Field/block context, e.g. `dgBangerData.BirthRule`.
    pub context: String,
    /// What went wrong.
    pub message: String,
}

impl fmt::Display for BangerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.context, self.message)
    }
}

impl std::error::Error for BangerError {}

type BangerResult<T> = Result<T, BangerError>;

fn err<T>(ctx: &str, msg: impl Into<String>) -> BangerResult<T> {
    Err(BangerError {
        context: ctx.into(),
        message: msg.into(),
    })
}

fn req_f32(b: &TuneBlock, ctx: &str, name: &str) -> BangerResult<f32> {
    match b.field(name) {
        Some(f) => match f.values.first().and_then(|v| v.number) {
            Some(n) => Ok(n as f32),
            None => err(ctx, format!("field {name:?} has no numeric value")),
        },
        None => err(ctx, format!("missing required field {name:?}")),
    }
}

/// Integer scalar: the authored value must exist and be integral —
/// silently truncating `3.5` to `3` would hide malformed data.
fn req_i64(b: &TuneBlock, ctx: &str, name: &str) -> BangerResult<i64> {
    match b.field(name) {
        Some(f) => match f.values.first().and_then(|v| v.number) {
            Some(n) if n.fract() == 0.0 && n.abs() <= i64::MAX as f64 => Ok(n as i64),
            Some(_) => err(ctx, format!("field {name:?} value is not an integer")),
            None => err(ctx, format!("field {name:?} has no numeric value")),
        },
        None => err(ctx, format!("missing required field {name:?}")),
    }
}

fn opt_i64(b: &TuneBlock, ctx: &str, name: &str) -> BangerResult<Option<i64>> {
    match b.field(name) {
        Some(f) => match f.values.first().and_then(|v| v.number) {
            Some(n) if n.fract() == 0.0 && n.abs() <= i64::MAX as f64 => Ok(Some(n as i64)),
            Some(n) => err(ctx, format!("field {name:?} value {n} is not an integer")),
            None => err(ctx, format!("field {name:?} has no numeric value")),
        },
        None => Ok(None),
    }
}

fn req_vec3(b: &TuneBlock, ctx: &str, name: &str) -> BangerResult<[f32; 3]> {
    match b.vec3(name) {
        Some(v) => Ok(v),
        None => err(ctx, format!("missing or malformed vec3 field {name:?}")),
    }
}

fn unknown_fields(b: &TuneBlock, ctx: &str, known: &[&str], warnings: &mut Vec<String>) {
    for name in b.field_names() {
        if !known.contains(&name) {
            warnings.push(format!("{ctx}: unrecognised field {name:?}"));
        }
    }
    for name in b.block_names() {
        if !known.contains(&name) {
            warnings.push(format!("{ctx}: unrecognised block {name:?}"));
        }
    }
}

/// The `BirthRule` particle-emission spec attached to a record — the
/// effect spawned when the banger activates or breaks. Field semantics
/// follow the authored names; what the original does with the variance
/// fields is unverified.
#[derive(Debug, Clone)]
pub struct BirthRule {
    pub position: [f32; 3],
    pub position_var: [f32; 3],
    pub velocity: [f32; 3],
    pub velocity_var: [f32; 3],
    pub life: f32,
    pub mass: f32,
    pub mass_var: f32,
    pub radius: f32,
    pub radius_var: f32,
    pub drag: f32,
    pub drag_var: f32,
    pub d_radius: f32,
    pub d_radius_var: f32,
    pub d_alpha: f32,
    pub d_alpha_var: f32,
    pub d_rotation: f32,
    pub d_rotation_var: f32,
    pub initial_blast: i64,
    pub spew_rate: f32,
    pub spew_time_limit: f32,
    pub gravity: f32,
    pub tex_frame_start: i64,
    pub tex_frame_end: i64,
    pub birth_flags: i64,
}

impl BirthRule {
    fn from_block(b: &TuneBlock, ctx: &str, warnings: &mut Vec<String>) -> BangerResult<Self> {
        unknown_fields(
            b,
            ctx,
            &[
                "Position",
                "PositionVar",
                "Velocity",
                "VelocityVar",
                "Life",
                "Mass",
                "MassVar",
                "Radius",
                "RadiusVar",
                "Drag",
                "DragVar",
                "DRadius",
                "DRadiusVar",
                "DAlpha",
                "DAlphaVar",
                "DRotation",
                "DRotationVar",
                "InitialBlast",
                "SpewRate",
                "SpewTimeLimit",
                "Gravity",
                "TexFrameStart",
                "TexFrameEnd",
                "BirthFlags",
            ],
            warnings,
        );
        Ok(BirthRule {
            position: req_vec3(b, ctx, "Position")?,
            position_var: req_vec3(b, ctx, "PositionVar")?,
            velocity: req_vec3(b, ctx, "Velocity")?,
            velocity_var: req_vec3(b, ctx, "VelocityVar")?,
            life: req_f32(b, ctx, "Life")?,
            mass: req_f32(b, ctx, "Mass")?,
            mass_var: req_f32(b, ctx, "MassVar")?,
            radius: req_f32(b, ctx, "Radius")?,
            radius_var: req_f32(b, ctx, "RadiusVar")?,
            drag: req_f32(b, ctx, "Drag")?,
            drag_var: req_f32(b, ctx, "DragVar")?,
            d_radius: req_f32(b, ctx, "DRadius")?,
            d_radius_var: req_f32(b, ctx, "DRadiusVar")?,
            d_alpha: req_f32(b, ctx, "DAlpha")?,
            d_alpha_var: req_f32(b, ctx, "DAlphaVar")?,
            d_rotation: req_f32(b, ctx, "DRotation")?,
            d_rotation_var: req_f32(b, ctx, "DRotationVar")?,
            initial_blast: req_i64(b, ctx, "InitialBlast")?,
            spew_rate: req_f32(b, ctx, "SpewRate")?,
            spew_time_limit: req_f32(b, ctx, "SpewTimeLimit")?,
            gravity: req_f32(b, ctx, "Gravity")?,
            tex_frame_start: req_i64(b, ctx, "TexFrameStart")?,
            tex_frame_end: req_i64(b, ctx, "TexFrameEnd")?,
            birth_flags: req_i64(b, ctx, "BirthFlags")?,
        })
    }
}

/// A decoded `dgBangerData` record.
#[derive(Debug, Clone)]
pub struct BangerData {
    /// `AudioId` — impact-sound selector (0 on every retail record).
    pub audio_id: i64,
    /// `Size` — the bound box's full extents (metres). Measured on
    /// retail: `CG.y = Size.y / 2` on every record, so the box
    /// `CG ± Size/2` rests its base on the instance origin — the
    /// authored placement point is the bound's *base contact point*,
    /// not its centre.
    pub size: [f32; 3],
    /// `CG` — the bound box's centre in prop-local space (also the
    /// authored centre of gravity). PKG geometry is authored centred
    /// at that centre, so instantiated content is offset by `+CG`.
    pub cg: [f32; 3],
    /// `NumGlows` declaration, when authored (light-glow count; absent
    /// on 2 retail records that still carry a `GlowOffset`).
    pub num_glows: Option<i64>,
    /// `GlowOffset` positions — one vec3 per authored field.
    pub glows: Vec<[f32; 3]>,
    /// `Mass` — kg.
    pub mass: f32,
    /// `Elasticity` — restitution (0.5 nearly everywhere; 1.5 is
    /// authored once on retail, so values above 1 are not anomalies).
    pub elasticity: f32,
    /// `Friction` — friction coefficient (0.9 on every retail record).
    pub friction: f32,
    /// `ImpulseLimit2` — impulse threshold, inferred activation/break
    /// limit (≈1e30 on 15 records reads as "effectively unbreakable").
    /// The name is preserved verbatim; no `ImpulseLimit` field exists
    /// on retail.
    pub impulse_limit2: f32,
    /// `SpinAxis` — 0 on every retail record.
    pub spin_axis: i64,
    /// `Flash` — inferred impact-flash sprite/frame id (0 = none).
    pub flash: i64,
    /// `NumParts` — count of `BREAK<NN>` geometry chunks in the
    /// record's own PKG; matches the chunk count on every standalone
    /// retail record.
    pub num_parts: i64,
    /// `BirthRule` (one retail record spells the block `asBirthRule`)
    /// — the activation/break particle spec. Every retail record
    /// carries one; `None` is anomalous (see `validate`).
    pub birth_rule: Option<BirthRule>,
    /// `TexNumber` — particle texture index (0-16 on retail).
    pub tex_number: i64,
    /// `BillFlags` — billboard flags for spawned effects.
    pub bill_flags: i64,
    /// `YRadius` — inferred cylindrical-bound radius (0 on most
    /// records, up to ~30 on retail).
    pub y_radius: f32,
    /// `ColliderId` — inferred collider/surface category (0-23 on
    /// retail); absent on 2 records.
    pub collider_id: Option<i64>,
    /// `CollisionPrim` — collision-primitive code (0/1/2 on retail);
    /// absent on 18 records.
    pub collision_prim: Option<i64>,
    /// `CollisionType` — collision-type code (4/16/48 on retail);
    /// absent on 30 records.
    pub collision_type: Option<i64>,
    /// Unknown/unmapped names encountered while decoding.
    pub warnings: Vec<String>,
}

/// Structural problems [`BangerData::validate`] reports. These are
/// authored-data anomalies or inconsistencies, not decode failures.
#[derive(Debug, Clone, PartialEq)]
pub enum BangerIssue {
    /// A field that must hold a finite number carries NaN/infinity.
    NonFinite { field: &'static str },
    /// A field that must be non-negative is negative.
    Negative { field: &'static str, value: f64 },
    /// `NumGlows` disagrees with the number of `GlowOffset` fields
    /// (`None` = the declaration is absent while offsets exist).
    GlowCount {
        declared: Option<i64>,
        fields: usize,
    },
    /// No `BirthRule`/`asBirthRule` block — every retail record has one.
    MissingBirthRule,
}

impl fmt::Display for BangerIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BangerIssue::NonFinite { field } => write!(f, "{field} is not finite"),
            BangerIssue::Negative { field, value } => {
                write!(f, "{field} is negative ({value})")
            }
            BangerIssue::GlowCount { declared, fields } => match declared {
                Some(n) => write!(
                    f,
                    "NumGlows declares {n} but {fields} GlowOffset field(s) exist"
                ),
                None => write!(f, "NumGlows absent but {fields} GlowOffset field(s) exist"),
            },
            BangerIssue::MissingBirthRule => write!(f, "no BirthRule block"),
        }
    }
}

impl BangerData {
    /// Parse text into a [`TuneFile`] and decode it as `dgBangerData`.
    pub fn parse(text: &str) -> BangerResult<Self> {
        let file = TuneFile::parse(text).map_err(|e| BangerError {
            context: "tune".into(),
            message: e.to_string(),
        })?;
        Self::from_tune(&file)
    }

    /// Decode the root `dgBangerData` block of a parsed tune file.
    pub fn from_tune(file: &TuneFile) -> BangerResult<Self> {
        if file.root.name != "dgBangerData" {
            return err(
                "dgBangerData",
                format!(
                    "expected dgBangerData root block, found {:?}",
                    file.root.name
                ),
            );
        }
        let root = &file.root;
        let ctx = "dgBangerData";
        let mut warnings = Vec::new();

        let mut glows = Vec::new();
        for e in &root.entries {
            if let TuneEntry::Field(fl) = e
                && fl.name == "GlowOffset"
            {
                match fl
                    .values
                    .iter()
                    .take(3)
                    .map(|v| v.number.map(|n| n as f32))
                    .collect::<Option<Vec<f32>>>()
                {
                    Some(v) if v.len() == 3 && fl.values.len() == 3 => {
                        glows.push([v[0], v[1], v[2]]);
                    }
                    _ => warnings.push(format!(
                        "{ctx}: GlowOffset at line {} has {} value(s), expected 3 — skipped",
                        fl.line,
                        fl.values.len()
                    )),
                }
            }
        }

        // Every retail record carries `BirthRule`; one spells the block
        // `asBirthRule` — accepted, flagged as a warning.
        let birth_rule = match root.block("BirthRule") {
            Some(b) => Some(BirthRule::from_block(
                b,
                "dgBangerData.BirthRule",
                &mut warnings,
            )?),
            None => match root.block("asBirthRule") {
                Some(b) => {
                    warnings.push(
                        "dgBangerData: nonstandard block name \"asBirthRule\" (expected \"BirthRule\")"
                            .into(),
                    );
                    Some(BirthRule::from_block(
                        b,
                        "dgBangerData.asBirthRule",
                        &mut warnings,
                    )?)
                }
                None => None,
            },
        };

        unknown_fields(
            root,
            ctx,
            &[
                "AudioId",
                "Size",
                "CG",
                "NumGlows",
                "GlowOffset",
                "Mass",
                "Elasticity",
                "Friction",
                "ImpulseLimit2",
                "SpinAxis",
                "Flash",
                "NumParts",
                "BirthRule",
                "asBirthRule",
                "TexNumber",
                "BillFlags",
                "YRadius",
                "ColliderId",
                "CollisionPrim",
                "CollisionType",
            ],
            &mut warnings,
        );

        Ok(BangerData {
            audio_id: req_i64(root, ctx, "AudioId")?,
            size: req_vec3(root, ctx, "Size")?,
            cg: req_vec3(root, ctx, "CG")?,
            num_glows: opt_i64(root, ctx, "NumGlows")?,
            glows,
            mass: req_f32(root, ctx, "Mass")?,
            elasticity: req_f32(root, ctx, "Elasticity")?,
            friction: req_f32(root, ctx, "Friction")?,
            impulse_limit2: req_f32(root, ctx, "ImpulseLimit2")?,
            spin_axis: req_i64(root, ctx, "SpinAxis")?,
            flash: req_i64(root, ctx, "Flash")?,
            num_parts: req_i64(root, ctx, "NumParts")?,
            birth_rule,
            tex_number: req_i64(root, ctx, "TexNumber")?,
            bill_flags: req_i64(root, ctx, "BillFlags")?,
            y_radius: req_f32(root, ctx, "YRadius")?,
            collider_id: opt_i64(root, ctx, "ColliderId")?,
            collision_prim: opt_i64(root, ctx, "CollisionPrim")?,
            collision_type: opt_i64(root, ctx, "CollisionType")?,
            warnings,
        })
    }

    /// Authored-consistency checks; empty on a well-formed record.
    pub fn validate(&self) -> Vec<BangerIssue> {
        let mut issues = Vec::new();
        for (field, v) in [
            ("Mass", self.mass),
            ("Elasticity", self.elasticity),
            ("Friction", self.friction),
            ("ImpulseLimit2", self.impulse_limit2),
            ("YRadius", self.y_radius),
        ] {
            if !v.is_finite() {
                issues.push(BangerIssue::NonFinite { field });
            } else if v < 0.0 {
                issues.push(BangerIssue::Negative {
                    field,
                    value: v as f64,
                });
            }
        }
        if self.size.iter().any(|c| !c.is_finite()) {
            issues.push(BangerIssue::NonFinite { field: "Size" });
        } else if self.size.iter().any(|c| *c < 0.0) {
            issues.push(BangerIssue::Negative {
                field: "Size",
                value: self.size.iter().copied().fold(f32::MAX, f32::min) as f64,
            });
        }
        if self.cg.iter().any(|c| !c.is_finite()) {
            issues.push(BangerIssue::NonFinite { field: "CG" });
        }
        if self.num_parts < 0 {
            issues.push(BangerIssue::Negative {
                field: "NumParts",
                value: self.num_parts as f64,
            });
        }
        if self.num_glows.is_some_and(|n| n < 0) {
            issues.push(BangerIssue::Negative {
                field: "NumGlows",
                value: self.num_glows.unwrap_or(0) as f64,
            });
        }
        if self.num_glows.unwrap_or(0) != self.glows.len() as i64 {
            issues.push(BangerIssue::GlowCount {
                declared: self.num_glows,
                fields: self.glows.len(),
            });
        }
        if self.birth_rule.is_none() {
            issues.push(BangerIssue::MissingBirthRule);
        }
        issues
    }
}

/// How a record stem names its object — the classification step of
/// F04-A. Resolution to actual geometry is the consumer's job (it needs
/// VFS access); this only decodes the naming convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BangerStem<'a> {
    /// `default` — the fallback record.
    Default,
    /// `<base>_break<NN>` — an authored breakaway fragment of `base`;
    /// the fragment mesh is the `BREAK<NN>` chunk inside
    /// `geometry/<base>.pkg` (or a `.mtx`-transformed part).
    Fragment { base: &'a str, index: &'a str },
    /// Any other stem — a standalone prop or a named part.
    Named(&'a str),
}

/// Classify a `tune/banger/<stem>.dgbangerdata` stem.
pub fn stem_role(stem: &str) -> BangerStem<'_> {
    if stem == "default" {
        return BangerStem::Default;
    }
    if let Some((base, index)) = stem.rsplit_once("_break")
        && !base.is_empty()
        && !index.is_empty()
        && index.bytes().all(|b| b.is_ascii_digit())
    {
        return BangerStem::Fragment { base, index };
    }
    BangerStem::Named(stem)
}

/// The PKG stem a record's geometry lives in — the name a placement
/// source must stamp for this record to be exercised. Fragments belong
/// to `<base>` when `geometry/<base>.pkg` exists, otherwise to their own
/// stem (a `.mtx` part or a dead ref). Named records belong to their own
/// stem when it has a PKG; otherwise to the longest `_`-prefixed base
/// that has one (a part chunk), else to their own stem (a `.mtx` part or
/// dead ref). `pkg_exists` answers whether `geometry/<stem>.pkg`
/// resolves; chunk-level correctness stays with the `banger` audit.
pub fn geometry_owner(stem: &str, mut pkg_exists: impl FnMut(&str) -> bool) -> &str {
    match stem_role(stem) {
        BangerStem::Default => stem,
        BangerStem::Fragment { base, .. } => {
            if pkg_exists(base) {
                base
            } else {
                stem
            }
        }
        BangerStem::Named(_) => {
            if pkg_exists(stem) {
                return stem;
            }
            for (i, _) in stem.match_indices('_').collect::<Vec<_>>().iter().rev() {
                let base = &stem[..*i];
                if pkg_exists(base) {
                    return base;
                }
            }
            stem
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: &str = "type: a\ndgBangerData {\n  AudioId 0\n  Size 1.0 2.0 3.0\n  CG 0.0 1.0 0.0\n  NumGlows 1\n  Mass 50.0\n  Elasticity 0.5\n  Friction 0.9\n  ImpulseLimit2 2500.0\n  SpinAxis 0\n  Flash 264\n  NumParts 2\n  BirthRule {\n    Position 0 0 0\n    PositionVar 0 0 0\n    Velocity 0 0 0\n    VelocityVar 0 0 0\n    Life 1.0\n    Mass 1.0\n    MassVar 0.0\n    Radius 1.0\n    RadiusVar 0.0\n    Drag 0.0\n    DragVar 0.0\n    DRadius 0.0\n    DRadiusVar 0.0\n    DAlpha 0\n    DAlphaVar 0\n    DRotation 0\n    DRotationVar 0\n    InitialBlast 0\n    SpewRate 0.0\n    SpewTimeLimit 0.0\n    Gravity -9.8\n    TexFrameStart 0\n    TexFrameEnd 0\n    BirthFlags 17352956\n  }\n  TexNumber 3\n  BillFlags 64\n  YRadius 0.5\n  ColliderId 6\n  CollisionPrim 1\n  CollisionType 16\n  GlowOffset 0.1 0.2 0.3\n}\n";

    #[test]
    fn parses_retail_shaped_record() {
        let d = BangerData::parse(RECORD).unwrap();
        assert_eq!(d.audio_id, 0);
        assert_eq!(d.size, [1.0, 2.0, 3.0]);
        assert_eq!(d.cg, [0.0, 1.0, 0.0]);
        assert_eq!(d.num_glows, Some(1));
        assert_eq!(d.glows, vec![[0.1, 0.2, 0.3]]);
        assert_eq!(d.mass, 50.0);
        assert_eq!(d.impulse_limit2, 2500.0);
        assert_eq!(d.num_parts, 2);
        assert_eq!(d.tex_number, 3);
        assert_eq!(d.bill_flags, 64);
        assert_eq!(d.y_radius, 0.5);
        assert_eq!(d.collider_id, Some(6));
        assert_eq!(d.collision_prim, Some(1));
        assert_eq!(d.collision_type, Some(16));
        let br = d.birth_rule.as_ref().unwrap();
        assert_eq!(br.gravity, -9.8);
        assert_eq!(br.birth_flags, 17352956);
        assert!(d.warnings.is_empty());
        assert!(d.validate().is_empty());
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let text = RECORD
            .replace("  NumGlows 1\n", "")
            .replace("  GlowOffset 0.1 0.2 0.3\n", "")
            .replace("  ColliderId 6\n", "")
            .replace("  CollisionPrim 1\n", "")
            .replace("  CollisionType 16\n", "");
        let d = BangerData::parse(&text).unwrap();
        assert_eq!(d.num_glows, None);
        assert_eq!(d.collider_id, None);
        assert_eq!(d.collision_prim, None);
        assert_eq!(d.collision_type, None);
        assert!(d.validate().is_empty());
    }

    #[test]
    fn rejects_wrong_root_block() {
        let err = BangerData::parse("type: a\nvehCarSim {\n  Mass 1\n}\n").unwrap_err();
        assert!(err.message.contains("expected dgBangerData"));
    }

    #[test]
    fn rejects_missing_required_field() {
        let err = BangerData::parse(&RECORD.replace("  Mass 50.0\n", "")).unwrap_err();
        assert!(err.message.contains("Mass"));
    }

    #[test]
    fn rejects_non_integer_id_field() {
        let err = BangerData::parse(&RECORD.replace("AudioId 0", "AudioId 0.5")).unwrap_err();
        assert!(err.message.contains("not an integer"));
    }

    #[test]
    fn accepts_as_birth_rule_with_warning() {
        let d = BangerData::parse(&RECORD.replace("BirthRule {", "asBirthRule {")).unwrap();
        assert!(d.birth_rule.is_some());
        assert!(d.warnings.iter().any(|w| w.contains("asBirthRule")));
    }

    #[test]
    fn flags_missing_birth_rule() {
        let start = RECORD.find("  BirthRule {").unwrap();
        let end = RECORD.find("  }\n  TexNumber").unwrap() + "  }\n".len();
        let mut text = RECORD.to_string();
        text.replace_range(start..end, "");
        let d = BangerData::parse(&text).unwrap();
        assert!(d.birth_rule.is_none());
        assert_eq!(d.validate(), vec![BangerIssue::MissingBirthRule]);
    }

    #[test]
    fn flags_glow_count_mismatch() {
        // NumGlows 1 but two GlowOffset fields.
        let mut text = RECORD.to_string();
        text.push_str("");
        let text = text.replace(
            "  GlowOffset 0.1 0.2 0.3\n",
            "  GlowOffset 0.1 0.2 0.3\n  GlowOffset 0.4 0.5 0.6\n",
        );
        let d = BangerData::parse(&text).unwrap();
        assert_eq!(
            d.validate(),
            vec![BangerIssue::GlowCount {
                declared: Some(1),
                fields: 2
            }]
        );
    }

    #[test]
    fn flags_glow_offset_without_num_glows() {
        let text = RECORD.replace("  NumGlows 1\n", "");
        let d = BangerData::parse(&text).unwrap();
        assert_eq!(
            d.validate(),
            vec![BangerIssue::GlowCount {
                declared: None,
                fields: 1
            }]
        );
    }

    #[test]
    fn flags_negative_and_nonfinite() {
        let d = BangerData::parse(&RECORD.replace("Mass 50.0", "Mass -2.0")).unwrap();
        assert_eq!(
            d.validate(),
            vec![BangerIssue::Negative {
                field: "Mass",
                value: -2.0
            }]
        );
        let d = BangerData::parse(&RECORD.replace("Friction 0.9", "Friction inf")).unwrap();
        assert_eq!(
            d.validate(),
            vec![BangerIssue::NonFinite { field: "Friction" }]
        );
    }

    #[test]
    fn stem_role_classification() {
        assert_eq!(stem_role("default"), BangerStem::Default);
        assert_eq!(
            stem_role("sp_benchwood_f_break03"),
            BangerStem::Fragment {
                base: "sp_benchwood_f",
                index: "03"
            }
        );
        assert_eq!(
            stem_role("vp4x4_break12"),
            BangerStem::Fragment {
                base: "vp4x4",
                index: "12"
            }
        );
        assert_eq!(
            stem_role("sp_lightstreet_rt_f"),
            BangerStem::Named("sp_lightstreet_rt_f")
        );
        // "_break" not followed by digits is just a name.
        assert_eq!(
            stem_role("foo_break_bar"),
            BangerStem::Named("foo_break_bar")
        );
        assert_eq!(stem_role("_break01"), BangerStem::Named("_break01"));
        assert_eq!(
            stem_role("vpvwcup_angel_break2"),
            BangerStem::Fragment {
                base: "vpvwcup_angel",
                index: "2"
            }
        );
    }

    #[test]
    fn geometry_owner_resolution() {
        let pkgs = ["sp_tree1_s", "vpsemi", "sp_benchwood_f"];
        let has = |s: &str| pkgs.contains(&s);
        // Standalone prop owns itself.
        assert_eq!(geometry_owner("sp_tree1_s", has), "sp_tree1_s");
        // Fragment joins its base's pkg when it exists.
        assert_eq!(
            geometry_owner("sp_benchwood_f_break03", has),
            "sp_benchwood_f"
        );
        // Fragment whose base pkg is absent keeps its own stem
        // (`.mtx` part or dead ref — the banger audit classifies which).
        assert_eq!(
            geometry_owner("sp_roundbout_l_break01", has),
            "sp_roundbout_l_break01"
        );
        // Named part joins the longest base prefix that has a pkg.
        assert_eq!(geometry_owner("vpsemi_whl3", has), "vpsemi");
        assert_eq!(geometry_owner("vpsemi_dash_wheel", has), "vpsemi");
        // No pkg at any split → owns itself.
        assert_eq!(geometry_owner("vpeagle_whl1", has), "vpeagle_whl1");
        assert_eq!(geometry_owner("default", has), "default");
    }
}
