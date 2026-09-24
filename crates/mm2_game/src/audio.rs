//! Vehicle audio contracts (F07) — the authored `aud/cardata` bindings
//! a spawned vehicle carries, shared between the content loader and the
//! app-side voice systems.
//!
//! Two layers live here: the *binding* ([`VehicleAudio`] — which samples
//! the vehicle references and at what authored volumes) and the
//! authored-data *interpretation* ([`EngineLoopSpec`] — what a
//! fade-window `Engine wave name` row computes for a given RPM). Voice
//! lifecycle, wave resolution and mixing are app concerns
//! (`mm2_app::audio`); the exact original application of the fade
//! windows is still inferred, not recovered (UNK-25).

use bevy::prelude::*;
use mm2_formats::cardata::{
    CarAudio, EngineSample, ImpactCategory, ImpactSample, ImpactTable, SkidSample, SurfaceEntry,
    is_sample_sentinel,
};

use crate::nav::NavRng;

/// The authored per-vehicle audio table attached to a spawned vehicle —
/// `aud/cardata/{player,opponent}/<id>.csv` verbatim (F07-A.2).
///
/// Absent when the vehicle's cardata record did not resolve or decode:
/// consumers treat a missing `VehicleAudio` as "no authored audio" and
/// report it rather than fabricating bindings.
#[derive(Component, Debug, Clone)]
pub struct VehicleAudio {
    /// The parsed cardata body (horn/clutch bindings + engine rows).
    pub spec: CarAudio,
}

/// One authored `Engine wave name` row resolved to mix parameters.
///
/// Only the canonical 10-column fade-window schema resolves — the two
/// development divisor layouts (`Pitch divisor`, `Volume divisor`/`vol
/// inverse RPM`) carry no RPM windows, so rows in those schemas return
/// `None` rather than being driven by a guessed mapping (UNK-25).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineLoopSpec {
    /// `Min Volume` — the loop's gain at a fade-in band edge.
    pub min_volume: f32,
    /// `Max Volume` — the loop's gain inside the band.
    pub max_volume: f32,
    /// `fade in start RPM` — envelope is 0 below this.
    pub fade_in_start: f32,
    /// `fade in end RPM` — envelope reaches 1 here.
    pub fade_in_end: f32,
    /// `fade out start RPM` — envelope starts falling here.
    pub fade_out_start: f32,
    /// `fade out end RPM` — envelope reaches 0 here.
    pub fade_out_end: f32,
    /// `Min Pitch` — playback speed at the pitch window's low edge.
    pub min_pitch: f32,
    /// `Max Pitch` — playback speed at the pitch window's high edge.
    pub max_pitch: f32,
    /// `Pitch shift start RPM`.
    pub pitch_start: f32,
    /// `Pitch shift end RPM`.
    pub pitch_end: f32,
}

/// The mixer state one engine loop should hold at a given RPM —
/// computed by [`EngineLoopSpec::mix`], applied to the voice's sink by
/// the app, and readable on the voice component for headless evidence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineMix {
    /// Loop gain, `0.0` outside the fade band (a silent voice keeps
    /// looping rather than churning sink attach/detach).
    pub volume: f32,
    /// Playback speed multiplier (pitch shift).
    pub speed: f32,
}

/// Progress through an `[start, end]` window: linear when the window
/// has width, a step at `end` when it is degenerate (authored
/// `fade out 15000,15000` = "no fade inside the reachable range", which
/// still reads as a hard cut past the bound rather than a missing
/// column).
fn window_progress(rpm: f32, start: f32, end: f32) -> f32 {
    if end > start {
        ((rpm - start) / (end - start)).clamp(0.0, 1.0)
    } else if rpm >= end {
        1.0
    } else {
        0.0
    }
}

impl EngineLoopSpec {
    /// Resolve a cardata engine row, or `None` when the row's schema
    /// lacks the fade-window columns or carries non-finite values.
    pub fn from_row(row: &EngineSample) -> Option<Self> {
        let spec = EngineLoopSpec {
            min_volume: row.column("min volume")?,
            max_volume: row.column("max volume")?,
            fade_in_start: row.column("fade in start")?,
            fade_in_end: row.column("fade in end")?,
            fade_out_start: row.column("fade out start")?,
            fade_out_end: row.column("fade out end")?,
            min_pitch: row.column("min pitch")?,
            max_pitch: row.column("max pitch")?,
            pitch_start: row.column("pitch shift start")?,
            pitch_end: row.column("pitch shift end")?,
        };
        let fields = [
            spec.min_volume,
            spec.max_volume,
            spec.fade_in_start,
            spec.fade_in_end,
            spec.fade_out_start,
            spec.fade_out_end,
            spec.min_pitch,
            spec.max_pitch,
            spec.pitch_start,
            spec.pitch_end,
        ];
        fields.iter().all(|v| v.is_finite()).then_some(spec)
    }

    /// The loop's mixer state at `rpm` (the sim's drivetrain-derived
    /// engine RPM — already gear- and direction-aware, so shift and
    /// reverse legs fall out of the same quantity rather than a
    /// separate trigger).
    ///
    /// Interpretation (inferred — UNK-25): the fade-in window ramps the
    /// envelope `0 → 1`, the fade-out window ramps it back `1 → 0`, and
    /// the volume interpolates `min_volume → max_volume` over the
    /// combined envelope, so a loop outside its band is silent instead
    /// of idling at its authored minimum. Pitch interpolates
    /// `min_pitch → max_pitch` across the pitch window. A non-finite
    /// RPM silences the loop rather than poisoning the sink.
    pub fn mix(&self, rpm: f32) -> EngineMix {
        if !rpm.is_finite() {
            return EngineMix {
                volume: 0.0,
                speed: 1.0,
            };
        }
        let envelope = window_progress(rpm, self.fade_in_start, self.fade_in_end)
            * (1.0 - window_progress(rpm, self.fade_out_start, self.fade_out_end));
        let volume = if envelope <= 0.0 {
            0.0
        } else {
            (self.min_volume + (self.max_volume - self.min_volume) * envelope).max(0.0)
        };
        let speed = (self.min_pitch
            + (self.max_pitch - self.min_pitch)
                * window_progress(rpm, self.pitch_start, self.pitch_end))
        .clamp(0.01, 16.0);
        EngineMix { volume, speed }
    }
}

/// Resolve a `default_impacts.csv` category for a struck object: the
/// authored `AudioId` selects the `Banger name` section by its `ID`
/// column; an id no section carries — and anything with no banger
/// record at all — falls back to id 0 (retail's `WALL`, the generic
/// car-impact set). `None` when the table has no id-0 section either
/// (a table that cannot answer an impact is a data failure the caller
/// counts, never a guessed category). Whether the original binds
/// `AudioId`→`ID` this way is unverified (UNK-25) — but it is the only
/// binding the authored data itself names, and retail props author
/// `AudioId` 0 everywhere, so on retail every struck prop reads the
/// `WALL` set regardless.
pub fn impact_category(table: &ImpactTable, audio_id: i64) -> Option<&ImpactCategory> {
    table
        .categories
        .iter()
        .find(|c| c.id == audio_id)
        .or_else(|| table.categories.iter().find(|c| c.id == 0))
}

/// One resolved impact pick: the authored sample plus the playback
/// volume drawn inside its `min volume,max volume` range.
#[derive(Debug, Clone, Copy)]
pub struct ImpactPick<'a> {
    /// The selected sample row (name + authored bands).
    pub sample: &'a ImpactSample,
    /// The volume the mixer should play at, drawn from the authored
    /// range by `rng` (sanitized: a non-finite or inverted bound reads
    /// as its counterpart, both bad reads as 1.0).
    pub volume: f32,
}

/// Pick one sample inside `category` for a hit of `force` (the app's
/// impact measure — `severity × striker mass`, the designed reading
/// UNK-25 labels): every sample whose `min force,max force` band
/// covers the force competes, `frequency` weights the draw, and the
/// pick's volume range supplies the playback volume. `None` when no
/// band covers — a sub-floor touch is authored silent, not an error.
pub fn pick_impact<'a>(
    category: &'a ImpactCategory,
    force: f32,
    rng: &mut NavRng,
) -> Option<ImpactPick<'a>> {
    let covering: Vec<&ImpactSample> = category
        .samples
        .iter()
        .filter(|s| {
            s.min_force.is_finite()
                && s.max_force.is_finite()
                && force >= s.min_force
                && force <= s.max_force
        })
        .collect();
    if covering.is_empty() {
        return None;
    }
    let weight_of = |s: &ImpactSample| {
        if s.frequency.is_finite() {
            s.frequency.max(0.0)
        } else {
            0.0
        }
    };
    let total: f32 = covering.iter().map(|s| weight_of(s)).sum();
    // `frequency` is the authored selection weight; a table where every
    // covering row weights 0 falls back to a uniform draw so authored
    // silence is never manufactured.
    let pick = if total > 0.0 {
        let mut draw = rng.next_f32() * total;
        covering
            .iter()
            .copied()
            .find(|s| {
                draw -= weight_of(s);
                draw <= 0.0
            })
            .unwrap_or(covering[0])
    } else {
        *rng.pick(&covering).unwrap_or(&covering[0])
    };
    let (lo, hi) = (pick.min_volume, pick.max_volume);
    let volume = match (lo.is_finite(), hi.is_finite()) {
        (true, true) => lo + (hi - lo) * rng.next_f32(),
        (true, false) => lo,
        (false, true) => hi,
        (false, false) => 1.0,
    };
    Some(ImpactPick {
        sample: pick,
        volume: volume.max(0.0),
    })
}

// ---------------------------------------------------------------------------
// F07-B.4: surface skid/rolling loops — `default_surface*.csv` rows
// resolved to mix parameters.
// ---------------------------------------------------------------------------

/// The unit a surface entry's skid bands trigger on — read off the
/// verbatim `skid wave` header. The two authored schemas disagree
/// in-column: the dry/wet files band `min slippage,max slippage` while
/// the ice files band `min speed,max speed` (per *variant*, both
/// sides — UNK-25; which quantity the original feeds either unit is
/// unrecovered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkidUnit {
    /// `min slippage,max slippage` — a 0..1 fraction of the tire's
    /// limit (see [`tire_slippage`] for the designed quantity fed in).
    Slippage,
    /// `min speed,max speed` — a contact speed, fed the wheel's
    /// `|vel_long|` m/s (designed reading).
    Speed,
}

/// A `surface wave` row's rolling loop resolved to mix parameters.
/// Only the canonical `max speed` schema resolves — the divisor layout
/// (ice files) carries `surface vol divisor`/`surface pitch divisor`
/// columns instead of a speed window, so its rolling loop returns
/// `None` rather than a guessed mapping, the same policy
/// [`EngineLoopSpec::from_row`] applies to divisor-schema rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollingSpec {
    /// `max speed` — the speed at which the envelope saturates.
    pub max_speed: f32,
    /// `min surface volume` — gain at rest.
    pub min_volume: f32,
    /// `max surface volume` — gain at `max speed`.
    pub max_volume: f32,
    /// `min surface pitch` — playback speed at rest.
    pub min_pitch: f32,
    /// `max surface pitch` — playback speed at `max speed`.
    pub max_pitch: f32,
}

impl RollingSpec {
    /// The loop's mixer state at `speed` (the car's `|forward_speed|`,
    /// m/s — the designed quantity, UNK-25): volume and pitch
    /// interpolate their authored min→max across `0..max speed` and
    /// saturate past it. A non-finite speed silences the loop.
    pub fn mix(&self, speed: f32) -> EngineMix {
        if !speed.is_finite() {
            return EngineMix {
                volume: 0.0,
                speed: 1.0,
            };
        }
        let f = window_progress(speed.abs(), 0.0, self.max_speed);
        EngineMix {
            volume: (self.min_volume + (self.max_volume - self.min_volume) * f).max(0.0),
            speed: (self.min_pitch + (self.max_pitch - self.min_pitch) * f).clamp(0.01, 16.0),
        }
    }
}

/// A surface entry's skid envelope + band trigger unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkidSpec {
    /// `min skid volume` — gain at a band's low edge.
    pub min_volume: f32,
    /// `max skid volume` — gain at a band's high edge.
    pub max_volume: f32,
    /// Which quantity the entry's bands compare (authored header unit).
    pub unit: SkidUnit,
}

/// One resolved skid pick: the covering band plus the gain the
/// `min,max skid volume` envelope computes inside it.
#[derive(Debug, Clone, Copy)]
pub struct SkidPick<'a> {
    /// The band's index in `entry.skids` — voice slots key off it.
    pub band: usize,
    /// The selected band row (name + band range).
    pub sample: &'a SkidSample,
    /// Playback gain inside `min,max skid volume`.
    pub volume: f32,
}

impl SkidSpec {
    /// The band covering `q` (a slippage fraction or wheel speed per
    /// [`SkidSpec::unit`]); `None` below every band — authored silence,
    /// not an error. The gain interpolates `min,max skid volume` across
    /// the covering band's own range (designed reading, UNK-25).
    pub fn pick<'a>(&self, bands: &'a [SkidSample], q: f32) -> Option<SkidPick<'a>> {
        if !q.is_finite() {
            return None;
        }
        let (band, sample) = bands
            .iter()
            .enumerate()
            .find(|(_, s)| s.min.is_finite() && s.max.is_finite() && q >= s.min && q <= s.max)?;
        let progress = window_progress(q, sample.min, sample.max);
        Some(SkidPick {
            band,
            sample,
            volume: (self.min_volume + (self.max_volume - self.min_volume) * progress).max(0.0),
        })
    }
}

/// A `surface wave` row resolved to its two mix halves — the rolling
/// loop and the skid envelope — each independently `None` when its
/// part of the row cannot answer (sentinel name, missing schema
/// columns, unclassifiable skid unit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceSpec {
    /// Rolling-loop envelope (`surface wave` + `max speed` schema).
    pub rolling: Option<RollingSpec>,
    /// Skid envelope + unit (`min,max skid volume` + the `skid wave`
    /// header's unit column).
    pub skid: Option<SkidSpec>,
}

impl SurfaceSpec {
    /// Resolve one authored surface row. A `NOSOUND` rolling name or a
    /// schema without the `max speed` window leaves `rolling` empty;
    /// an absent/unclassifiable `skid wave` header or empty band list
    /// leaves `skid` empty. Non-finite cells reject their half — a bad
    /// row degrades that half to silence, never a guessed value.
    pub fn from_entry(entry: &SurfaceEntry) -> Self {
        let rolling = (|| {
            if is_sample_sentinel(&entry.name) {
                return None;
            }
            let spec = RollingSpec {
                max_speed: entry.column("max speed")?,
                min_volume: entry.column("min surface volume")?,
                max_volume: entry.column("max surface volume")?,
                min_pitch: entry.column("min surface pitch")?,
                max_pitch: entry.column("max surface pitch")?,
            };
            [
                spec.max_speed,
                spec.min_volume,
                spec.max_volume,
                spec.min_pitch,
                spec.max_pitch,
            ]
            .iter()
            .all(|v| v.is_finite())
            .then_some(spec)
        })();
        let skid = (|| {
            if entry.skids.is_empty() {
                return None;
            }
            let unit = entry.skid_columns.iter().find_map(|c| {
                let c = c.to_ascii_lowercase();
                if c.contains("slippage") {
                    Some(SkidUnit::Slippage)
                } else if c.contains("speed") {
                    Some(SkidUnit::Speed)
                } else {
                    None
                }
            })?;
            let spec = SkidSpec {
                min_volume: entry.column("min skid volume")?,
                max_volume: entry.column("max skid volume")?,
                unit,
            };
            [spec.min_volume, spec.max_volume]
                .iter()
                .all(|v| v.is_finite())
                .then_some(spec)
        })();
        SurfaceSpec { rolling, skid }
    }
}

/// The designed quantity fed to `min slippage` bands (UNK-25): the
/// larger of the tire's longitudinal over-demand (`traction_demand` —
/// the fraction of its grip limit the controller asked for) and its
/// lateral utilization (`|slip angle| / peak slip angle`), clamped to
/// `0..=1` so utilization saturates at the authored limit the bands
/// domain. The arcade tire model has no wheel-speed state, so there is
/// no measured slip ratio to read; this is the closest "how far past
/// the tire's limit" quantity the sim publishes — a handbrake slide
/// reads high on the lateral term, a burnout on the longitudinal, and
/// a brake held at rest reads ~0 (its demand is zero while `vel_long`
/// is zero), which is what keeps AC03's no-squeal legs silent.
/// Non-finite inputs read as 0 rather than poisoning the pick.
pub fn tire_slippage(traction_demand: f32, slip_angle: f32, peak_slip_angle: f32) -> f32 {
    let long = if traction_demand.is_finite() {
        traction_demand.abs()
    } else {
        0.0
    };
    let lat = if slip_angle.is_finite() && peak_slip_angle.is_finite() && peak_slip_angle > 0.0 {
        (slip_angle / peak_slip_angle).abs()
    } else {
        0.0
    };
    long.max(lat).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retail `aud/cardata/player/vpbug.csv` engine table, verbatim.
    const VP_BUG: &[u8] = b"Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume,,,,,\r\nVWHORN,0.95,0,4,REVERSE,0.93,,,,,\r\nEngine wave name,Min Volume,Max Volume,fade in  start RPM,fade in end RPM,fade out start RPM,fade out end RPM,Min Pitch,Max Pitch,Pitch shift start RPM,Pitch shift end RPM\r\nVWIDLE,0.55,0.835,1,800,2500,7000,0.85,2,1,7000\r\nVWDRIVE,0.55,0.9,500,2800,6500,10000,0.6,2.5,500,12000\r\nVWMID,0.55,0.85,500,4000,7000,11000,1,2.25,500,11000\r\nVWHIGH,0.55,0.91,3000,8000,15000,15000,0.65,2.25,3000,12000\r\n";

    fn vpbug() -> CarAudio {
        CarAudio::parse(VP_BUG).unwrap()
    }

    #[test]
    fn retail_rows_resolve_to_fade_window_specs() {
        let car = vpbug();
        assert_eq!(car.engine_samples.len(), 4);
        let specs: Vec<_> = car
            .engine_samples
            .iter()
            .map(|r| EngineLoopSpec::from_row(r).unwrap())
            .collect();
        let idle = specs[0];
        assert_eq!(idle.min_volume, 0.55);
        assert_eq!(idle.max_volume, 0.835);
        assert_eq!(idle.fade_in_start, 1.0);
        assert_eq!(idle.fade_out_end, 7000.0);
        assert_eq!(idle.min_pitch, 0.85);
        assert_eq!(idle.pitch_end, 7000.0);
    }

    #[test]
    fn idle_rpm_sounds_only_the_low_loops() {
        let car = vpbug();
        let mixes: Vec<_> = car
            .engine_samples
            .iter()
            .map(|r| EngineLoopSpec::from_row(r).unwrap().mix(900.0))
            .collect();
        // At a 900 rpm idle the high loop's band (3000+) is silent while
        // the low loops are already fading in — the authored envelope
        // floors, not zero.
        assert!(mixes[0].volume > 0.8, "idle loop at max volume: {mixes:?}");
        assert!(mixes[1].volume > 0.5 && mixes[1].volume < 0.7);
        assert!(mixes[2].volume > 0.5 && mixes[2].volume < 0.7);
        assert_eq!(mixes[3].volume, 0.0, "high loop below its band");
        // Pitch sits near the bottom of each authored window.
        assert!(mixes[0].speed < 1.0 && mixes[0].speed > 0.85);
    }

    #[test]
    fn rising_rpm_crossfades_and_pitch_shifts() {
        let car = vpbug();
        let mix = |i: usize, rpm: f32| {
            EngineLoopSpec::from_row(&car.engine_samples[i])
                .unwrap()
                .mix(rpm)
        };
        // The idle loop's fade-out runs 2500→7000 — halfway it is down
        // but not gone; the high loop is into its fade-in.
        let idle_mid = mix(0, 4750.0);
        assert!(idle_mid.volume > 0.55 && idle_mid.volume < 0.835);
        let idle_gone = mix(0, 7500.0);
        assert_eq!(idle_gone.volume, 0.0);
        // Pitch climbs monotonically across the pitch window.
        let (lo, hi) = (mix(0, 1000.0).speed, mix(0, 6000.0).speed);
        assert!(hi > lo, "pitch rises with rpm: {lo} -> {hi}");
        assert!(
            (mix(0, 7000.0).speed - 2.0).abs() < 1e-5,
            "pitch tops at max pitch"
        );
    }

    #[test]
    fn degenerate_windows_cut_instead_of_dividing_by_zero() {
        let car = vpbug();
        // VWHIGH authors fade-out 15000,15000 — no width — so the loop
        // holds until that bound then cuts.
        let high = EngineLoopSpec::from_row(&car.engine_samples[3]).unwrap();
        assert!(high.mix(14999.0).volume > 0.0);
        assert_eq!(high.mix(15000.0).volume, 0.0);
    }

    #[test]
    fn divisor_schema_rows_do_not_resolve() {
        // `copy of default.csv` — the development divisor layout has no
        // RPM windows to drive; it is skipped, not guessed.
        let data = b"Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume,,\r\nRACECARHORN,0.95,0,2,REVERSE,0.93,,\r\nEngine wave name,Min Volume,Max Volume,Volume Divisor,Min Pitch,Max Pitch,Pitch Divisor,vol inverse RPM\r\nRACECARIDLE,0.1,0.96,1700.0,0.8,1.64,1849.999955,0.0\r\n";
        let car = CarAudio::parse(data).unwrap();
        assert!(EngineLoopSpec::from_row(&car.engine_samples[0]).is_none());
    }

    #[test]
    fn non_finite_rpm_and_values_are_safe() {
        let car = vpbug();
        let idle = EngineLoopSpec::from_row(&car.engine_samples[0]).unwrap();
        let m = idle.mix(f32::NAN);
        assert_eq!(m.volume, 0.0);
        assert_eq!(m.speed, 1.0);
        // A non-finite authored cell (`1e999` parses to inf) rejects
        // the row, never produces a non-finite mix.
        let data = b"Horn wave name,Horn volume,flags,Num Engine Samples,clutch wave name,clutch volume\nH,0.9,0,1,C,0.5\nEngine wave name,Min Volume,Max Volume,fade in start RPM,fade in end RPM,fade out start RPM,fade out end RPM,Min Pitch,Max Pitch,Pitch shift start RPM,Pitch shift end RPM\nE,1e999,0.9,1,800,2500,7000,0.85,2,1,7000\n";
        let bad = CarAudio::parse(data).unwrap();
        assert!(EngineLoopSpec::from_row(&bad.engine_samples[0]).is_none());
    }

    // -------------------------------------------------------------------
    // F07-B.3: impact category/band/frequency/volume interpretation.
    // -------------------------------------------------------------------

    /// A small authored `default_impacts.csv`: id-0 WALL with two
    /// force bands, id-7 LIGHT with one, the `ENDOFDATA` terminator.
    const IMPACTS: &[u8] = b"***\nBanger name,Num samples,ID\nWALL,2,0\nsample name,min volume,max volume,min force,max force,frequency\nSOFT,0.5,0.6,1000,8000,1.0\nHUGE,0.9,1.0,8000,999999,1.0\n***\nBanger name,Num samples,ID\nLIGHT,1,7\nsample name,min volume,max volume,min force,max force,frequency\nPROP,0.4,0.4,0,999999,1.0\n***\nBanger name,Num samples,ID\nENDOFDATA,0,0\n";

    fn impacts() -> ImpactTable {
        ImpactTable::parse(IMPACTS).unwrap()
    }

    #[test]
    fn the_audio_id_selects_the_authored_category() {
        let table = impacts();
        assert_eq!(impact_category(&table, 7).unwrap().name, "LIGHT");
        assert_eq!(impact_category(&table, 0).unwrap().name, "WALL");
        // An id no record carries — and a struck world body — fall to
        // the id-0 catch-all rather than a fabricated category.
        assert_eq!(impact_category(&table, 42).unwrap().name, "WALL");
        // A table with no id-0 section cannot answer at all.
        let orphan = b"***\nBanger name,Num samples,ID\nLIGHT,1,7\nPROP,0.4,0.4,0,999999,1.0\n***\nBanger name,Num samples,ID\nENDOFDATA,0,0\n";
        let orphan = ImpactTable::parse(orphan).unwrap();
        assert!(impact_category(&orphan, 3).is_none());
    }

    #[test]
    fn the_force_band_selects_and_the_floor_stays_silent() {
        let table = impacts();
        let wall = impact_category(&table, 0).unwrap();
        let mut rng = NavRng::new(1);
        assert_eq!(
            pick_impact(wall, 3900.0, &mut rng).unwrap().sample.name,
            "SOFT"
        );
        assert_eq!(
            pick_impact(wall, 39000.0, &mut rng).unwrap().sample.name,
            "HUGE"
        );
        // Below the softest band: authored silence, not an error.
        assert!(pick_impact(wall, 500.0, &mut rng).is_none());
    }

    #[test]
    fn frequency_weights_the_covering_samples() {
        // Both rows cover every force; the zero-frequency row must
        // never win while any weighted row stands.
        let data = b"***\nBanger name,Num samples,ID\nWALL,2,0\nsample name,min volume,max volume,min force,max force,frequency\nRARE,0.5,0.6,0,999999,0.0\nCOMMON,0.5,0.6,0,999999,1.0\n***\nBanger name,Num samples,ID\nENDOFDATA,0,0\n";
        let table = ImpactTable::parse(data).unwrap();
        let cat = impact_category(&table, 0).unwrap();
        let mut rng = NavRng::new(7);
        for _ in 0..32 {
            assert_eq!(
                pick_impact(cat, 1.0, &mut rng).unwrap().sample.name,
                "COMMON"
            );
        }
        // All-zero weights draw uniformly instead of manufacturing
        // silence — every sample stays reachable.
        let flat = b"***\nBanger name,Num samples,ID\nWALL,2,0\nsample name,min volume,max volume,min force,max force,frequency\nA,0.5,0.6,0,999999,0.0\nB,0.5,0.6,0,999999,0.0\n***\nBanger name,Num samples,ID\nENDOFDATA,0,0\n";
        let flat = ImpactTable::parse(flat).unwrap();
        let cat = impact_category(&flat, 0).unwrap();
        let mut rng = NavRng::new(7);
        let seen: std::collections::HashSet<String> = (0..32)
            .map(|_| pick_impact(cat, 1.0, &mut rng).unwrap().sample.name.clone())
            .collect();
        assert_eq!(seen.len(), 2, "uniform draw reaches both rows");
    }

    #[test]
    fn the_volume_draw_stays_inside_the_authored_range() {
        let table = impacts();
        let wall = impact_category(&table, 0).unwrap();
        let mut rng = NavRng::new(3);
        for _ in 0..32 {
            let pick = pick_impact(wall, 3900.0, &mut rng).unwrap();
            assert!((0.5..=0.6).contains(&pick.volume), "{pick:?}");
        }
        // An inverted or non-finite bound reads as its counterpart,
        // never a negative or NaN gain.
        let bad = b"***\nBanger name,Num samples,ID\nWALL,1,0\nsample name,min volume,max volume,min force,max force,frequency\nX,1e999,0.5,0,999999,1.0\n***\nBanger name,Num samples,ID\nENDOFDATA,0,0\n";
        let bad = ImpactTable::parse(bad).unwrap();
        let cat = impact_category(&bad, 0).unwrap();
        assert_eq!(pick_impact(cat, 1.0, &mut rng).unwrap().volume, 0.5);
    }

    // -------------------------------------------------------------------
    // F07-B.4: surface skid/rolling interpretation.
    // -------------------------------------------------------------------

    /// The retail `default_surfacedry.csv` shape: entry 0 is the
    /// NOSOUND default surface with three slippage-banded skids, entry
    /// 2 a rolling loop with one band.
    const SURFACE_DRY: &[u8] = b"Tunnel sound index\n0\nsurface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\nNOSOUND,125,0,0,0,0,0.5,0.88,3\nskid wave,min slippage,max slippage\ntireskid1,0.55,0.65\ntireskid2,0.65,0.75\ntireskid3,0.75,1\nsurface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\nROLL,25,0.35,0.75,0.85,1.25,0.5,0.72,1\nskid wave,min slippage,max slippage\nROLLSKID,0.25,1\n";

    /// The retail `default_surfaceice.csv` shape: the 12-column divisor
    /// layout whose bands key on speed instead of slippage.
    const SURFACE_ICE: &[u8] = b"tunnel sound index\n5\nsurface wave,min surface volume,max surface volume,surface vol divisor,min surface pitch,max surface pitch,surface pitch divisor,min skid volume,max skid volume,skid vol divisor,num skid samples,for tunnels\nSNOW,0.88,0.88,2,0.85,2,30,0.5,0.88,2,1,0\nskid wave ,min speed,max speed\nSNOWSKID,0,1000\n";

    fn entries(data: &[u8]) -> mm2_formats::cardata::SurfaceTable {
        mm2_formats::cardata::SurfaceTable::parse(data).unwrap()
    }

    #[test]
    fn the_canonical_schema_resolves_both_halves() {
        let t = entries(SURFACE_DRY);
        let e0 = SurfaceSpec::from_entry(&t.surfaces[0]);
        // NOSOUND authors no rolling loop; the skid half still reads.
        assert!(e0.rolling.is_none());
        let skid = e0.skid.unwrap();
        assert_eq!(skid.unit, SkidUnit::Slippage);
        assert_eq!((skid.min_volume, skid.max_volume), (0.5, 0.88));
        let e1 = SurfaceSpec::from_entry(&t.surfaces[1]);
        let rolling = e1.rolling.unwrap();
        assert_eq!(rolling.max_speed, 25.0);
        assert_eq!((rolling.min_pitch, rolling.max_pitch), (0.85, 1.25));
    }

    #[test]
    fn the_divisor_schema_skips_rolling_but_keeps_speed_bands() {
        let t = entries(SURFACE_ICE);
        let spec = SurfaceSpec::from_entry(&t.surfaces[0]);
        // No `max speed` column — the rolling half cannot resolve a
        // speed window, so it is skipped like a divisor-schema engine
        // row rather than driven by a guessed divisor formula.
        assert!(spec.rolling.is_none());
        let skid = spec.skid.unwrap();
        assert_eq!(skid.unit, SkidUnit::Speed);
        assert!(skid.pick(&t.surfaces[0].skids, 40.0).is_some());
    }

    #[test]
    fn an_unclassifiable_skid_header_resolves_no_skid_half() {
        let data = b"surface wave,max speed,min surface volume,max surface volume,min surface pitch,max surface pitch,min skid volume,max skid volume,num skid samples\nR,125,0.1,0.9,0.8,1.2,0.5,0.9,1\nskid wave,min flex,max flex\nK,0,1\n";
        let t = entries(data);
        let spec = SurfaceSpec::from_entry(&t.surfaces[0]);
        assert!(spec.rolling.is_some());
        assert!(spec.skid.is_none());
    }

    #[test]
    fn the_slip_band_selects_and_the_floor_stays_silent() {
        let t = entries(SURFACE_DRY);
        let spec = SurfaceSpec::from_entry(&t.surfaces[0]).skid.unwrap();
        let pick = spec.pick(&t.surfaces[0].skids, 0.6).unwrap();
        assert_eq!((pick.band, pick.sample.name.as_str()), (0, "tireskid1"));
        assert!((0.5..=0.88).contains(&pick.volume));
        let top = spec.pick(&t.surfaces[0].skids, 0.9).unwrap();
        assert_eq!(top.sample.name, "tireskid3");
        // Below the softest band: authored silence. A non-finite
        // quantity picks nothing rather than poisoning the mix.
        assert!(spec.pick(&t.surfaces[0].skids, 0.3).is_none());
        assert!(spec.pick(&t.surfaces[0].skids, f32::NAN).is_none());
    }

    #[test]
    fn the_skid_gain_interpolates_inside_the_covering_band() {
        let t = entries(SURFACE_DRY);
        let spec = SurfaceSpec::from_entry(&t.surfaces[0]).skid.unwrap();
        let lo = spec.pick(&t.surfaces[0].skids, 0.55).unwrap().volume;
        let hi = spec.pick(&t.surfaces[0].skids, 0.65).unwrap().volume;
        assert!(
            (lo - 0.5).abs() < 1e-5 && (hi - 0.88).abs() < 1e-5,
            "{lo}/{hi}"
        );
    }

    #[test]
    fn rolling_mix_ramps_volume_and_pitch_with_speed() {
        let t = entries(SURFACE_DRY);
        let spec = SurfaceSpec::from_entry(&t.surfaces[1]).rolling.unwrap();
        let rest = spec.mix(0.0);
        assert_eq!(rest.volume, 0.35);
        assert_eq!(rest.speed, 0.85);
        let top = spec.mix(25.0);
        assert_eq!(top.volume, 0.75);
        assert_eq!(top.speed, 1.25);
        // Saturates past `max speed` — reverse reads the same |speed|.
        assert_eq!(spec.mix(-80.0).volume, 0.75);
        assert_eq!(spec.mix(f32::NAN).volume, 0.0);
    }

    #[test]
    fn tire_slippage_is_the_larger_utilization_saturated_at_one() {
        // Longitudinal over-demand and lateral utilization compete.
        assert_eq!(tire_slippage(0.7, 0.05, 0.16), 0.7);
        assert!((tire_slippage(0.2, 0.12, 0.16) - 0.75).abs() < 1e-5);
        // Over-limit demand saturates at the band domain's top.
        assert_eq!(tire_slippage(1.4, 0.0, 0.16), 1.0);
        // A held brake at rest demands nothing — no squeal.
        assert_eq!(tire_slippage(0.0, 0.0, 0.16), 0.0);
        // Non-finite inputs and a degenerate peak read as 0.
        assert_eq!(tire_slippage(f32::NAN, f32::INFINITY, 0.16), 0.0);
        assert_eq!(tire_slippage(0.0, 0.5, 0.0), 0.0);
    }
}
