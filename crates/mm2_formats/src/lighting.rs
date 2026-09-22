//! Decoder for MM2 `.ltNN` time-weather lighting presets.
//!
//! Each stock city ships `city/<city>.lt00` .. `.lt15` — sixteen text
//! records in the shared block grammar (see [`crate::tune`]):
//!
//! ```text
//! type: a
//! clear-morning {
//!   KeyHeading 2.200000
//!   KeyPitch -0.200000
//!   KeyColor 0.900000 0.900000 0.800000
//!   Fill1Heading 3.140000
//!   ...
//!   Ambient -14803406
//! }
//! ```
//!
//! This is the authored data behind MM2Hook's recovered
//! `cityTimeWeatherLighting` / `timeWeathers[16]` (R4): a key light plus two
//! fills, each with a heading/pitch direction and an RGB colour, and a
//! packed ambient colour. The block name is the authored preset label.
//!
//! Naming grid measured on all 32 retail records (both cities identical):
//! `ltNN` = `tod_index * 4 + weather_index` — `morning` occupies lt00–03,
//! `noon` lt04–07, `evening` lt08–11, `night` lt12–15; inside each group
//! the order is `clear`, `cloudy`, `foggy`, `rainy`. The file index is the
//! table position the game reads (`timeOfDay` selects the entry); the name
//! is descriptive, and an authored rename would not change the slot. Both
//! are kept: [`LightingPreset::index`] classifies the name, callers compare
//! it against the file's `NN` suffix.

use crate::tune::{TuneEntry, TuneFile};

/// Number of lighting presets per city (`lt00`..=`lt15`).
pub const LIGHTING_PRESET_COUNT: usize = 16;

/// Authored weather classes, in authored index order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WeatherKind {
    /// `clear`
    Clear,
    /// `cloudy` (the `amb_p*` light defs write it `p` — partly cloudy)
    Cloudy,
    /// `foggy`
    Foggy,
    /// `rainy`
    Rainy,
}

/// Authored time-of-day classes, in authored index order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimeOfDay {
    /// `morning`
    Morning,
    /// `noon`
    Noon,
    /// `evening`
    Evening,
    /// `night`
    Night,
}

impl WeatherKind {
    /// All four kinds in authored index order.
    pub const ALL: [WeatherKind; 4] = [
        WeatherKind::Clear,
        WeatherKind::Cloudy,
        WeatherKind::Foggy,
        WeatherKind::Rainy,
    ];

    /// Authored index (0–3).
    pub fn index(self) -> usize {
        match self {
            WeatherKind::Clear => 0,
            WeatherKind::Cloudy => 1,
            WeatherKind::Foggy => 2,
            WeatherKind::Rainy => 3,
        }
    }

    /// The `<weather>-<tod>` name fragment used by `.ltNN` block names.
    pub fn name(self) -> &'static str {
        match self {
            WeatherKind::Clear => "clear",
            WeatherKind::Cloudy => "cloudy",
            WeatherKind::Foggy => "foggy",
            WeatherKind::Rainy => "rainy",
        }
    }

    /// Parse the weather fragment of a preset name.
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "clear" => WeatherKind::Clear,
            "cloudy" => WeatherKind::Cloudy,
            "foggy" => WeatherKind::Foggy,
            "rainy" => WeatherKind::Rainy,
            _ => return None,
        })
    }
}

impl TimeOfDay {
    /// All four kinds in authored index order.
    pub const ALL: [TimeOfDay; 4] = [
        TimeOfDay::Morning,
        TimeOfDay::Noon,
        TimeOfDay::Evening,
        TimeOfDay::Night,
    ];

    /// Authored index (0–3).
    pub fn index(self) -> usize {
        match self {
            TimeOfDay::Morning => 0,
            TimeOfDay::Noon => 1,
            TimeOfDay::Evening => 2,
            TimeOfDay::Night => 3,
        }
    }

    /// The `<weather>-<tod>` name fragment used by `.ltNN` block names.
    pub fn name(self) -> &'static str {
        match self {
            TimeOfDay::Morning => "morning",
            TimeOfDay::Noon => "noon",
            TimeOfDay::Evening => "evening",
            TimeOfDay::Night => "night",
        }
    }

    /// Parse the time-of-day fragment of a preset name.
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "morning" => TimeOfDay::Morning,
            "noon" => TimeOfDay::Noon,
            "evening" => TimeOfDay::Evening,
            "night" => TimeOfDay::Night,
            _ => return None,
        })
    }
}

/// Measured authored index convention: `tod.index() * 4 + weather.index()`.
pub fn preset_index(weather: WeatherKind, tod: TimeOfDay) -> usize {
    tod.index() * 4 + weather.index()
}

/// Classify an authored preset label like `clear-morning` into
/// `(weather, tod)`; `None` when the name does not follow the grid.
pub fn classify_preset_name(name: &str) -> Option<(WeatherKind, TimeOfDay)> {
    let (w, t) = name.split_once('-')?;
    Some((WeatherKind::from_name(w)?, TimeOfDay::from_name(t)?))
}

/// One directional light's authored parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightSpec {
    /// Heading angle in radians (measured range on retail: −3.14..3.14).
    pub heading: f32,
    /// Pitch angle in radians (measured range on retail: −1.4..1.4).
    pub pitch: f32,
    /// RGB colour; retail values sit in 0.2..=1.0 but are not clamped here.
    pub color: [f32; 3],
}

impl LightSpec {
    /// The direction from a lit surface *toward* this light, in authored
    /// space — recovered from MM2Hook's `setLightDirectionInv` (R4), the
    /// same math the original shading loop used:
    ///
    /// ```text
    /// (−cos(heading)·cos(pitch), −sin(pitch), −sin(heading)·cos(pitch))
    /// ```
    ///
    /// Sanity against retail: noon presets author pitch ≈ −1.0 →
    /// to-light ≈ +Y (the sun overhead); morning/evening ≈ −0.2 (low
    /// sun); `rainy-night` keys pitch +1.4 → the key shines from below
    /// the horizon and contributes nothing to upward faces (the fills
    /// do the work — the authored values mean it).
    pub fn to_light_dir(&self) -> [f32; 3] {
        let cp = self.pitch.cos();
        [
            -self.heading.cos() * cp,
            -self.pitch.sin(),
            -self.heading.sin() * cp,
        ]
    }

    /// The direction the light *travels* — the negation of
    /// [`to_light_dir`](Self::to_light_dir). Renderers whose light
    /// direction is "where the rays point" (Bevy's `DirectionalLight`
    /// forward axis) want this one.
    pub fn travel_dir(&self) -> [f32; 3] {
        let d = self.to_light_dir();
        [-d[0], -d[1], -d[2]]
    }
}

/// A decoded `.ltNN` record.
#[derive(Debug, Clone)]
pub struct LightingPreset {
    /// Authored preset label (the root block name, e.g. `clear-morning`).
    pub name: String,
    /// `type:` header tag verbatim (`a` on every retail file).
    pub type_tag: Option<String>,
    /// Key (sun) light.
    pub key: LightSpec,
    /// First fill light.
    pub fill1: LightSpec,
    /// Second fill light.
    pub fill2: LightSpec,
    /// Packed ambient colour verbatim (signed i32 as authored).
    pub ambient_packed: i32,
    /// Field names present in the block that are not part of the recovered
    /// schema — kept verbatim for diagnosis.
    pub extra_fields: Vec<String>,
}

/// Decode failure detail when a required scalar/vector field is malformed.
#[derive(Debug)]
pub struct LightingError(pub String);

impl std::fmt::Display for LightingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LightingError {}

impl LightingPreset {
    /// Decode an `.ltNN` record. `input` is the file text.
    pub fn parse(input: &str) -> Result<Self, LightingError> {
        let file = TuneFile::parse(input).map_err(|e| LightingError(e.to_string()))?;
        let root = &file.root;

        let light = |prefix: &str| -> Result<LightSpec, LightingError> {
            let heading = root
                .f32(&format!("{prefix}Heading"))
                .ok_or_else(|| LightingError(format!("missing/non-numeric {prefix}Heading")))?;
            let pitch = root
                .f32(&format!("{prefix}Pitch"))
                .ok_or_else(|| LightingError(format!("missing/non-numeric {prefix}Pitch")))?;
            let color = root
                .vec3(&format!("{prefix}Color"))
                .ok_or_else(|| LightingError(format!("missing/non-numeric {prefix}Color")))?;
            Ok(LightSpec {
                heading,
                pitch,
                color,
            })
        };

        let ambient_packed = root
            .field("Ambient")
            .and_then(|f| f.values.first())
            .and_then(|v| v.number)
            .ok_or_else(|| LightingError("missing/non-numeric Ambient".into()))?
            as i32;

        const KNOWN: &[&str] = &[
            "KeyHeading",
            "KeyPitch",
            "KeyColor",
            "Fill1Heading",
            "Fill1Pitch",
            "Fill1Color",
            "Fill2Heading",
            "Fill2Pitch",
            "Fill2Color",
            "Ambient",
        ];
        let extra_fields = root
            .entries
            .iter()
            .filter_map(|e| match e {
                TuneEntry::Field(f) if !KNOWN.contains(&f.name.as_str()) => Some(f.name.clone()),
                TuneEntry::Block(b) => Some(format!("block {}", b.name)),
                _ => None,
            })
            .collect();

        Ok(LightingPreset {
            name: root.name.clone(),
            type_tag: file.type_tag.clone(),
            key: light("Key")?,
            fill1: light("Fill1")?,
            fill2: light("Fill2")?,
            ambient_packed,
            extra_fields,
        })
    }

    /// `(weather, tod)` when the authored label follows the retail
    /// `<weather>-<tod>` grid; `None` for off-grid names.
    pub fn classify(&self) -> Option<(WeatherKind, TimeOfDay)> {
        classify_preset_name(&self.name)
    }

    /// The `ltNN` slot implied by the classified name, per the measured
    /// `tod*4 + weather` convention; `None` for off-grid names.
    pub fn index(&self) -> Option<usize> {
        self.classify().map(|(w, t)| preset_index(w, t))
    }

    /// Ambient colour unpacked to `[r, g, b, a]` bytes under MM2Hook's
    /// `PackColorBGRA` convention (packed `0xAARRGGBB`, ambient alpha forced
    /// opaque by the original's setter — recovered, not documented).
    pub fn ambient_rgba(&self) -> [u8; 4] {
        let u = self.ambient_packed as u32;
        [(u >> 16) as u8, (u >> 8) as u8, u as u8, (u >> 24) as u8]
    }

    /// Sanity findings. Unclassifiable names and extra fields are reported
    /// here; structural absence fails in [`LightingPreset::parse`].
    pub fn validate(&self) -> Vec<LightingIssue> {
        let mut issues = Vec::new();
        if self.classify().is_none() {
            issues.push(LightingIssue::OffGridName(self.name.clone()));
        }
        for (which, l) in [
            ("key", self.key),
            ("fill1", self.fill1),
            ("fill2", self.fill2),
        ] {
            if !l.heading.is_finite() || !l.pitch.is_finite() {
                issues.push(LightingIssue::NonFiniteAngle(which));
            }
            if l.color.iter().any(|c| !c.is_finite() || *c < 0.0) {
                issues.push(LightingIssue::BadColor(which));
            }
        }
        for f in &self.extra_fields {
            issues.push(LightingIssue::ExtraField(f.clone()));
        }
        issues
    }
}

/// Validation findings for a [`LightingPreset`].
#[derive(Debug, Clone, PartialEq)]
pub enum LightingIssue {
    /// The preset name does not match `<weather>-<tod>` on the retail grid.
    OffGridName(String),
    /// A heading or pitch is NaN/infinite.
    NonFiniteAngle(&'static str),
    /// A colour channel is negative or non-finite.
    BadColor(&'static str),
    /// A field outside the recovered schema is present.
    ExtraField(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "type: a\nclear-morning {\n  KeyHeading 2.200000\n  KeyPitch -0.200000\n  KeyColor 0.900000\t0.900000\t0.800000\n  Fill1Heading 3.140000\n  Fill1Pitch -0.025000\n  Fill1Color 0.500000\t0.300000\t0.200000\n  Fill2Heading -1.200000\n  Fill2Pitch -0.500000\n  Fill2Color 0.600000\t0.600000\t0.800000\n  Ambient -14803406\n}\n";

    #[test]
    fn parses_retail_shape() {
        let p = LightingPreset::parse(SAMPLE).unwrap();
        assert_eq!(p.name, "clear-morning");
        assert_eq!(p.type_tag.as_deref(), Some("a"));
        assert_eq!(p.key.heading, 2.2);
        assert_eq!(p.key.pitch, -0.2);
        assert_eq!(p.key.color, [0.9, 0.9, 0.8]);
        assert_eq!(p.fill2.color, [0.6, 0.6, 0.8]);
        assert_eq!(p.ambient_packed, -14803406);
        // 0xFF1E1E32 → B=0x32 G=0x1E R=0x1E under BGRA packing.
        assert_eq!(p.ambient_rgba(), [0x1E, 0x1E, 0x32, 0xFF]);
        assert!(p.validate().is_empty());
    }

    #[test]
    fn measured_index_grid() {
        assert_eq!(preset_index(WeatherKind::Clear, TimeOfDay::Morning), 0);
        assert_eq!(preset_index(WeatherKind::Rainy, TimeOfDay::Morning), 3);
        assert_eq!(preset_index(WeatherKind::Clear, TimeOfDay::Noon), 4);
        assert_eq!(preset_index(WeatherKind::Rainy, TimeOfDay::Night), 15);
        let p = LightingPreset::parse(SAMPLE).unwrap();
        assert_eq!(p.index(), Some(0));
        assert_eq!(
            classify_preset_name("rainy-night"),
            Some((WeatherKind::Rainy, TimeOfDay::Night))
        );
        assert_eq!(classify_preset_name("partly-cloudy-morning"), None);
        assert_eq!(classify_preset_name("clear"), None);
    }

    #[test]
    fn recovered_light_direction() {
        // `setLightDirectionInv` convention: to-light =
        // (−cos h·cos p, −sin p, −sin h·cos p). Noon pitch −1.0 → the
        // source sits overhead (+Y); travel direction is its negation.
        let noon = LightSpec {
            heading: -2.0,
            pitch: -1.0,
            color: [1.0; 3],
        };
        let d = noon.to_light_dir();
        assert!(d[1] > 0.8, "noon to-light points up: {d:?}");
        let t = noon.travel_dir();
        assert!(t[1] < -0.8, "noon light travels downward: {t:?}");
        for i in 0..3 {
            assert!((d[i] + t[i]).abs() < 1e-6);
        }
        // Zero angles → to-light = (−1, 0, 0); unit length always.
        let flat = LightSpec {
            heading: 0.0,
            pitch: 0.0,
            color: [0.0; 3],
        };
        assert_eq!(flat.to_light_dir(), [-1.0, 0.0, 0.0]);
        let up = LightSpec {
            heading: 0.0,
            pitch: 1.4,
            color: [0.0; 3],
        };
        let du = up.to_light_dir();
        let len = (du[0] * du[0] + du[1] * du[1] + du[2] * du[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-6, "unit vector: {du:?}");
        assert!(
            du[1] < 0.0,
            "positive pitch puts the source below the horizon"
        );
    }

    #[test]
    fn missing_field_fails() {
        let bad = SAMPLE.replace("  KeyPitch -0.200000\n", "");
        assert!(LightingPreset::parse(&bad).is_err());
        let bad = SAMPLE.replace("  Ambient -14803406\n", "");
        assert!(LightingPreset::parse(&bad).is_err());
    }

    #[test]
    fn off_grid_name_and_extra_fields_validate() {
        let src = SAMPLE
            .replace("clear-morning", "sepia")
            .replace("  Ambient -14803406", "  SepiaTone 1\n  Ambient -14803406");
        let p = LightingPreset::parse(&src).unwrap();
        let issues = p.validate();
        assert!(issues.contains(&LightingIssue::OffGridName("sepia".into())));
        assert!(issues.contains(&LightingIssue::ExtraField("SepiaTone".into())));
    }

    #[test]
    fn non_finite_angle_and_negative_color_validate() {
        let src = SAMPLE
            .replace("  KeyPitch -0.200000", "  KeyPitch NaN")
            .replace(
                "  Fill1Color 0.500000\t0.300000\t0.200000",
                "  Fill1Color -0.5 0.3 0.2",
            );
        let p = LightingPreset::parse(&src).unwrap();
        let issues = p.validate();
        assert!(issues.contains(&LightingIssue::NonFiniteAngle("key")));
        assert!(issues.contains(&LightingIssue::BadColor("fill1")));
    }
}
