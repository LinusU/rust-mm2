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
use mm2_formats::cardata::{CarAudio, EngineSample, ImpactCategory, ImpactSample, ImpactTable};

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
}
