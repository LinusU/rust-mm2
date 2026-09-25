//! Decoder for the authored HUD map tune record (`tune/<city>.mmhudmap`).
//!
//! Each stock city ships one `mmHudMap` block in the shared block grammar
//! (see [`crate::tune`]):
//!
//! ```text
//! type: a
//! mmHudMap {
//!   Size 0.210000 0.250000
//!   Pos 0.780000 0.750000
//!   ZoomIn 0
//!   Approach Rate 1.200000
//!   ZoomInDist 577.000000
//!   ZoomOutDist 1195.000000
//!   IconScaleMin 34.090019
//!   IconScaleMax 52.399994
//!   ZoomInDistFS 786.000000
//!   ZoomOutDistFS 1581.000000
//!   IconScaleMinFS 15.070000
//!   IconScaleMaxFS 18.000000
//!   Ocean Color 0.084 0.7 0.94
//! }
//! ```
//!
//! This is the parameter block the original `mmHudMap` class's `FileIO`
//! path reads — the class name and its `hudmap_%s.pkg` tile asset are
//! exe strings, and mm2hook (R4) recovers the class's map-mode/zoom/
//! orient members. `Size`/`Pos` are the corner inset's window fractions
//! (top-left origin — 0.78/0.75 puts the map in the bottom-right corner,
//! where the original draws it). The `*Dist` fields are the two zoom
//! levels' view extents and `IconScale*` the marker extents alongside
//! them — units read as world metres against the city-size packages
//! (inferred); the `*FS` quartet parameterises the full-screen map
//! (HUD-4's Q key) the same way. `ZoomIn` is authored `0` on both
//! cities — carried verbatim; whether it seeds the initial zoom level or
//! arms the control is unrecovered.
//!
//! Grammar note: two field names carry a qualifier word — `Approach
//! Rate` lexes as field `Approach` with values `Rate 1.2`, and `Ocean
//! Color` as `Ocean` with `Color r g b`. Reads here take the *numeric
//! tail* of the field, which is also correct for the unqualified names.

use crate::tune::{TuneEntry, TuneField, TuneFile};

/// A decoded `mmHudMap` record.
#[derive(Debug, Clone)]
pub struct HudMapSpec {
    /// `type:` header tag verbatim (`a` on both retail files).
    pub type_tag: Option<String>,
    /// `Size` — the inset map's window-fraction extent (w, h).
    pub size: [f32; 2],
    /// `Pos` — the inset map's window-fraction origin.
    pub pos: [f32; 2],
    /// `ZoomIn` verbatim — authored `0` on both cities; its semantics
    /// are unrecovered (initial level vs. arming flag).
    pub zoom_in: i64,
    /// `Approach Rate` — the zoom easing rate (per second).
    pub approach_rate: f32,
    /// `ZoomInDist` — the zoomed-in view extent.
    pub zoom_in_dist: f32,
    /// `ZoomOutDist` — the zoomed-out view extent.
    pub zoom_out_dist: f32,
    /// `IconScaleMin` — marker extent at the zoomed-in level.
    pub icon_scale_min: f32,
    /// `IconScaleMax` — marker extent at the zoomed-out level.
    pub icon_scale_max: f32,
    /// `ZoomInDistFS` — full-screen map's zoomed-in extent.
    pub zoom_in_dist_fs: f32,
    /// `ZoomOutDistFS` — full-screen map's zoomed-out extent.
    pub zoom_out_dist_fs: f32,
    /// `IconScaleMinFS` — full-screen marker extent, zoomed in.
    pub icon_scale_min_fs: f32,
    /// `IconScaleMaxFS` — full-screen marker extent, zoomed out.
    pub icon_scale_max_fs: f32,
    /// `Ocean Color` — the map's background colour (the water regions
    /// the tile artwork leaves unpainted read as water).
    pub ocean_color: [f32; 3],
    /// Field names outside the recovered schema, kept for diagnosis.
    pub extra_fields: Vec<String>,
}

/// Decode failure detail.
#[derive(Debug)]
pub struct HudMapError(pub String);

impl std::fmt::Display for HudMapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for HudMapError {}

/// The last `n` values of `field`, as floats. Qualified names
/// (`Approach Rate`, `Ocean Color`) lead their value list with a word,
/// so the authored numbers are the tail.
fn tail(field: Option<&TuneField>, n: usize) -> Option<Vec<f32>> {
    let f = field?;
    if f.values.len() < n {
        return None;
    }
    f.values[f.values.len() - n..]
        .iter()
        .map(|v| v.number.map(|x| x as f32))
        .collect()
}

impl HudMapSpec {
    /// Decode an `.mmhudmap` record. `input` is the file text.
    pub fn parse(input: &str) -> Result<Self, HudMapError> {
        let file = TuneFile::parse(input).map_err(|e| HudMapError(e.to_string()))?;
        let root = &file.root;
        if !root.name.eq_ignore_ascii_case("mmHudMap") {
            return Err(HudMapError(format!(
                "expected an mmHudMap block, found {:?}",
                root.name
            )));
        }
        let need = |name: &str, n: usize| -> Result<Vec<f32>, HudMapError> {
            tail(root.field_ci(name), n)
                .ok_or_else(|| HudMapError(format!("missing/non-numeric {name}")))
        };
        let vec2 = |name: &str| -> Result<[f32; 2], HudMapError> {
            let v = need(name, 2)?;
            Ok([v[0], v[1]])
        };
        let vec3 = |name: &str| -> Result<[f32; 3], HudMapError> {
            let v = need(name, 3)?;
            Ok([v[0], v[1], v[2]])
        };
        let f32_of = |name: &str| -> Result<f32, HudMapError> { Ok(need(name, 1)?[0]) };

        let zoom_in = tail(root.field_ci("ZoomIn"), 1)
            .ok_or_else(|| HudMapError("missing/non-numeric ZoomIn".into()))?[0]
            as i64;

        const KNOWN: &[&str] = &[
            "Size",
            "Pos",
            "ZoomIn",
            "Approach",
            "ZoomInDist",
            "ZoomOutDist",
            "IconScaleMin",
            "IconScaleMax",
            "ZoomInDistFS",
            "ZoomOutDistFS",
            "IconScaleMinFS",
            "IconScaleMaxFS",
            "Ocean",
        ];
        let extra_fields = root
            .entries
            .iter()
            .filter_map(|e| match e {
                TuneEntry::Field(f) if !KNOWN.iter().any(|k| f.name.eq_ignore_ascii_case(k)) => {
                    Some(f.name.clone())
                }
                TuneEntry::Block(b) => Some(format!("block {}", b.name)),
                _ => None,
            })
            .collect();

        Ok(HudMapSpec {
            type_tag: file.type_tag.clone(),
            size: vec2("Size")?,
            pos: vec2("Pos")?,
            zoom_in,
            approach_rate: f32_of("Approach")?,
            zoom_in_dist: f32_of("ZoomInDist")?,
            zoom_out_dist: f32_of("ZoomOutDist")?,
            icon_scale_min: f32_of("IconScaleMin")?,
            icon_scale_max: f32_of("IconScaleMax")?,
            zoom_in_dist_fs: f32_of("ZoomInDistFS")?,
            zoom_out_dist_fs: f32_of("ZoomOutDistFS")?,
            icon_scale_min_fs: f32_of("IconScaleMinFS")?,
            icon_scale_max_fs: f32_of("IconScaleMaxFS")?,
            ocean_color: vec3("Ocean")?,
            extra_fields,
        })
    }

    /// Sanity findings — structural absence fails in
    /// [`HudMapSpec::parse`]; this layer reports values that decode but
    /// would make a nonsense map (negative extents, inverted zoom
    /// levels, an out-of-screen inset).
    pub fn validate(&self) -> Vec<HudMapIssue> {
        let mut issues = Vec::new();
        let finite_pos = |v: f32| v.is_finite() && v >= 0.0;
        if self.size.iter().any(|v| !finite_pos(*v))
            || self.pos.iter().any(|v| !finite_pos(*v))
            || self.pos[0] + self.size[0] > 1.0
            || self.pos[1] + self.size[1] > 1.0
        {
            issues.push(HudMapIssue::BadLayout);
        }
        for (name, v) in [
            ("ZoomInDist", self.zoom_in_dist),
            ("ZoomOutDist", self.zoom_out_dist),
            ("ZoomInDistFS", self.zoom_in_dist_fs),
            ("ZoomOutDistFS", self.zoom_out_dist_fs),
        ] {
            if !v.is_finite() || v <= 0.0 {
                issues.push(HudMapIssue::BadExtent(name));
            }
        }
        if self.zoom_in_dist >= self.zoom_out_dist || self.zoom_in_dist_fs >= self.zoom_out_dist_fs
        {
            issues.push(HudMapIssue::ZoomOrder);
        }
        for (name, v) in [
            ("IconScaleMin", self.icon_scale_min),
            ("IconScaleMax", self.icon_scale_max),
            ("IconScaleMinFS", self.icon_scale_min_fs),
            ("IconScaleMaxFS", self.icon_scale_max_fs),
        ] {
            if !v.is_finite() || v <= 0.0 {
                issues.push(HudMapIssue::BadIconScale(name));
            }
        }
        if !self.approach_rate.is_finite() || self.approach_rate < 0.0 {
            issues.push(HudMapIssue::BadRate);
        }
        if self.ocean_color.iter().any(|c| !c.is_finite() || *c < 0.0) {
            issues.push(HudMapIssue::BadColor);
        }
        for f in &self.extra_fields {
            issues.push(HudMapIssue::ExtraField(f.clone()));
        }
        issues
    }
}

/// Validation findings for a [`HudMapSpec`].
#[derive(Debug, Clone, PartialEq)]
pub enum HudMapIssue {
    /// `Pos`/`Size` do not bound a rect inside the window.
    BadLayout,
    /// A zoom extent is non-finite or non-positive.
    BadExtent(&'static str),
    /// A zoomed-in extent is not smaller than its zoomed-out pair.
    ZoomOrder,
    /// An icon scale is non-finite or non-positive.
    BadIconScale(&'static str),
    /// `Approach Rate` is negative or non-finite.
    BadRate,
    /// `Ocean Color` carries a negative or non-finite channel.
    BadColor,
    /// A field outside the recovered schema is present.
    ExtraField(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // The retail SF record's shape verbatim (values from
    // `tune/sf.mmhudmap`), including the qualifier-word field names.
    const SAMPLE: &str = "type: a\nmmHudMap {\n  Size 0.210000\t0.250000 \n  Pos 0.780000\t0.750000 \n  ZoomIn 0 \n  Approach Rate 1.200000 \n  ZoomInDist 577.000000 \n  ZoomOutDist 1195.000000 \n  IconScaleMin 34.090019 \n  IconScaleMax 52.399994 \n  ZoomInDistFS 786.000000 \n  ZoomOutDistFS 1581.000000 \n  IconScaleMinFS 15.070000 \n  IconScaleMaxFS 18.000000 \n  Ocean Color 0.084  0.7 0.94 \n}\n";

    #[test]
    fn parses_retail_shape() {
        let s = HudMapSpec::parse(SAMPLE).unwrap();
        assert_eq!(s.type_tag.as_deref(), Some("a"));
        assert_eq!(s.size, [0.21, 0.25]);
        assert_eq!(s.pos, [0.78, 0.75]);
        assert_eq!(s.zoom_in, 0);
        assert_eq!(s.approach_rate, 1.2);
        assert_eq!(s.zoom_in_dist, 577.0);
        assert_eq!(s.zoom_out_dist, 1195.0);
        assert_eq!(s.icon_scale_min, 34.090_02);
        assert_eq!(s.icon_scale_max, 52.399_994);
        assert_eq!(s.zoom_in_dist_fs, 786.0);
        assert_eq!(s.zoom_out_dist_fs, 1581.0);
        assert_eq!(s.icon_scale_min_fs, 15.07);
        assert_eq!(s.icon_scale_max_fs, 18.0);
        assert_eq!(s.ocean_color, [0.084, 0.7, 0.94]);
        assert!(s.validate().is_empty());
    }

    #[test]
    fn qualifier_words_are_not_values() {
        // `Approach Rate 1.2` lexes as field `Approach` whose values are
        // `Rate 1.2` — the read must land on 1.2, not choke on `Rate`.
        // Same for `Ocean Color r g b`.
        let s = HudMapSpec::parse(SAMPLE).unwrap();
        assert_eq!(s.approach_rate, 1.2, "qualifier `Rate` must be skipped");
        assert_eq!(s.ocean_color[0], 0.084, "qualifier `Color` skipped");
    }

    #[test]
    fn missing_field_fails() {
        let bad = SAMPLE.replace("  ZoomOutDist 1195.000000 \n", "");
        assert!(HudMapSpec::parse(&bad).is_err());
        let bad = SAMPLE.replace("  Ocean Color 0.084  0.7 0.94 \n", "");
        assert!(HudMapSpec::parse(&bad).is_err());
    }

    #[test]
    fn wrong_root_block_fails() {
        let bad = SAMPLE.replace("mmHudMap {", "mmSomethingElse {");
        assert!(HudMapSpec::parse(&bad).is_err());
    }

    #[test]
    fn degenerate_values_validate() {
        let src = SAMPLE.replace(
            "  ZoomInDist 577.000000",
            "  ZoomInDist 1995.000000", // in >= out
        );
        let s = HudMapSpec::parse(&src).unwrap();
        assert!(s.validate().contains(&HudMapIssue::ZoomOrder));
        let src = SAMPLE.replace("  Size 0.210000\t0.250000", "  Size 0.9 0.9");
        let s = HudMapSpec::parse(&src).unwrap();
        assert!(s.validate().contains(&HudMapIssue::BadLayout));
        let src = SAMPLE.replace(
            "  IconScaleMin 34.090019",
            "  IconScaleMin -1\n  Mystery 3\n  IconScaleMin 34.090019",
        );
        // Duplicate name reads the first `IconScaleMin` — still -1? No:
        // `field_ci` returns the FIRST match, so -1 wins → BadIconScale,
        // plus the unknown field is reported.
        let s = HudMapSpec::parse(&src).unwrap();
        let issues = s.validate();
        assert!(issues.contains(&HudMapIssue::BadIconScale("IconScaleMin")));
        assert!(issues.contains(&HudMapIssue::ExtraField("Mystery".into())));
    }
}
