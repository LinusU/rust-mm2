//! F07-AC02 evidence driver: a staged `idle → accelerate → coast →
//! brake → reverse` input program through the production
//! [`VehicleInput`] path (`--seq`).
//!
//! The spec's scripted sequence must show the audio rig answering the
//! drivetrain — engine loops re-mixing off RPM and the clutch one-shot
//! firing on committed gear/direction changes — not a component
//! poked by hand. [`sequence_drive`] owns the player vehicle's input
//! while [`SequenceDrive`] exists (the same resource-gated pattern
//! [`crate::scripted::ScriptedDrive`] and [`crate::input::ParkedDrive`]
//! hold), walks the fixed stage table below, and banks one
//! [`SeqSample`] at every stage boundary: the car's RPM/gear/direction,
//! the loudest engine loop's computed mix, and the clutch one-shots
//! the stage produced. The headless smoke record prints the samples as
//! `seq=`; a windowed `--seq` runs the same program live.
//!
//! The AC's "shift" leg has no stage of its own: the gearbox is
//! automatic, so committed `(gear, direction)` changes land *inside*
//! `accelerate` (upshifts), `coast` (downshifts) and the `brake →
//! reverse` hand-off — the per-stage `+Nc` clutch deltas in the record
//! are where they show. The stage timers only run while the session
//! actually lets the car drive (`Playing` and not countdown-locked —
//! the same gate [`crate::scripted::scripted_drive`] honors), so an
//! event countdown does not burn the idle stage.

use bevy::prelude::*;
use mm2_game::{PlayerVehicle, RaceState, Session};
use mm2_vehicle::{DriveDirection, VehicleInput, VehicleState};

use crate::audio::{AudioReport, EngineVoice};

/// Frames (60 Hz app updates) the idle stage holds — the car settles
/// onto its suspension and the engine sits at the authored idle band.
const IDLE_FRAMES: u32 = 180;
/// Full-throttle run long enough for the automatic gearbox to walk
/// several upshifts — the retail roster reaches top gear well inside
/// seven seconds.
const ACCEL_FRAMES: u32 = 420;
/// Closed-throttle coast — RPM decays toward idle and the gearbox
/// downshifts through it.
const COAST_FRAMES: u32 = 240;
/// Bound on the brake stage: the stage ends the frame the car reports
/// the sim's own "nearly stopped" band, or here — a car penned against
/// a wall still reaches reverse rather than stalling the program.
const BRAKE_MAX_FRAMES: u32 = 600;
/// Reverse hold — the engaged `Reverse` direction turns the brake
/// pedal into the reverse throttle and the car backs through its
/// single reverse band.
const REVERSE_FRAMES: u32 = 300;
/// The forward speed at which the brake stage hands off to reverse —
/// the same `<= 0.25` "nearly stopped" edge the drivetrain itself
/// engages `DriveDirection::Reverse` on, so the held brake becomes
/// reverse throttle the same step the stage flips.
const STOPPED_SPEED: f32 = 0.25;

/// One stage of the sequence program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SeqStage {
    /// Zero input — settled idle.
    #[default]
    Idle,
    /// Full throttle — the gearbox upshifts through the run.
    Accelerate,
    /// Zero input — RPM decays, downshifts land here.
    Coast,
    /// Held brake until the car reaches the nearly-stopped band (or
    /// the bound caps it).
    Brake,
    /// Brake still held — now the reverse throttle.
    Reverse,
    /// Program complete — zero input for the rest of the run.
    Done,
}

impl SeqStage {
    /// The input the stage writes while it holds.
    fn input(self) -> VehicleInput {
        match self {
            Self::Accelerate => VehicleInput {
                throttle: 1.0,
                ..Default::default()
            },
            Self::Brake | Self::Reverse => VehicleInput {
                brake: 1.0,
                ..Default::default()
            },
            _ => VehicleInput::default(),
        }
    }

    /// Stable lowercase name for the `seq=` record field.
    fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Accelerate => "acc",
            Self::Coast => "coast",
            Self::Brake => "brake",
            Self::Reverse => "rev",
            Self::Done => "done",
        }
    }
}

/// One stage-boundary snapshot the [`sequence_drive`] system banks
/// when a stage ends — the evidence row the `seq=` record prints.
#[derive(Debug, Clone, Copy)]
pub struct SeqSample {
    /// Stage the snapshot closes.
    pub stage: &'static str,
    /// `VehicleState::rpm` at the boundary.
    pub rpm: f32,
    /// `VehicleState::gear` at the boundary.
    pub gear: usize,
    /// `VehicleState::direction` at the boundary.
    pub direction: DriveDirection,
    /// `VehicleState::forward_speed` at the boundary, m/s.
    pub forward_speed: f32,
    /// The loudest engine loop's mixed volume on the player car (0
    /// when no rig exists — an honest silent car, not a hidden one).
    pub mix_volume: f32,
    /// That same loop's mixed playback speed (the pitch multiplier).
    pub mix_speed: f32,
    /// Clutch one-shots the stage produced (`AudioReport::clutch`
    /// delta over the stage — the committed-shift events).
    pub clutch: u64,
}

impl SeqSample {
    /// The `seq=` fragment for one stage:
    /// `acc:5210r/F3/42.1m/0.99v/1.60p+2c`.
    pub fn format(&self) -> String {
        let dir = match self.direction {
            DriveDirection::Forward => "F",
            DriveDirection::Reverse => "R",
        };
        let clutch = if self.clutch > 0 {
            format!("+{}c", self.clutch)
        } else {
            String::new()
        };
        format!(
            "{}:{:.0}r/{}{}/{:.1}m/{:.2}v/{:.2}p{}",
            self.stage,
            self.rpm,
            dir,
            self.gear,
            self.forward_speed,
            self.mix_volume,
            self.mix_speed,
            clutch
        )
    }
}

/// Presence enables the sequence driver: `--seq` inserts it and
/// [`sequence_drive`] owns the player vehicle's [`VehicleInput`] while
/// it exists — the same resource gate [`ScriptedDrive`](
/// crate::scripted::ScriptedDrive)/[`ParkedDrive`](crate::input::ParkedDrive)
/// use, so the windowed app and the headless smoke run the same system.
/// Carries the stage machine and the banked [`SeqSample`]s the smoke
/// record reads.
#[derive(Resource, Debug, Default)]
pub struct SequenceDrive {
    /// The stage currently holding.
    stage: SeqStage,
    /// Driving frames spent in `stage` — locked/paused frames do not
    /// count, so a countdown cannot burn the idle stage.
    stage_frames: u32,
    /// `AudioReport::clutch` when the current stage began — the
    /// per-stage delta baseline.
    clutch_at_start: u64,
    /// The session generation the machine belongs to — a mid-run
    /// restart (`rs=`) resets the program and its samples so a record
    /// never mixes two sessions' stages.
    generation: u64,
    /// One [`SeqSample`] per completed stage, in order — the smoke
    /// record prints them as `seq=`.
    pub samples: Vec<SeqSample>,
}

/// The stage transition test: the fixed frame-count stages advance on
/// their bound, `Brake` waits for the car to reach the nearly-stopped
/// band (or its own cap), `Done` never leaves.
fn advance(stage: SeqStage, stage_frames: u32, forward_speed: f32) -> Option<SeqStage> {
    match stage {
        SeqStage::Idle if stage_frames >= IDLE_FRAMES => Some(SeqStage::Accelerate),
        SeqStage::Accelerate if stage_frames >= ACCEL_FRAMES => Some(SeqStage::Coast),
        SeqStage::Coast if stage_frames >= COAST_FRAMES => Some(SeqStage::Brake),
        SeqStage::Brake if forward_speed <= STOPPED_SPEED || stage_frames >= BRAKE_MAX_FRAMES => {
            Some(SeqStage::Reverse)
        }
        SeqStage::Reverse if stage_frames >= REVERSE_FRAMES => Some(SeqStage::Done),
        _ => None,
    }
}

/// Write the sequence's staged [`VehicleInput`] on the player vehicle
/// every frame, banking a [`SeqSample`] at each stage boundary.
/// Scheduled after [`crate::input::vehicle_input`] like the other
/// evidence drivers, so `--seq` deterministically owns the input while
/// the resource exists.
///
/// Honors the same gates the keyboard path does: only a `Playing`,
/// countdown-released session advances the program — anything else
/// (menu, countdown, pause, results, teardown) writes a zeroed input
/// and holds the stage, so the sequence cannot put a stale throttle
/// onto a car the race has not released.
pub fn sequence_drive(
    mut seq: ResMut<SequenceDrive>,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    report: Res<AudioReport>,
    mut cars: Query<(Entity, &mut VehicleInput, &VehicleState), With<PlayerVehicle>>,
    voices: Query<(&ChildOf, &EngineVoice)>,
) {
    // A restart begins a fresh program: the fresh car has not run the
    // early stages, so keeping them would mislabel the record.
    if seq.generation != session.generation() {
        seq.generation = session.generation();
        seq.stage = SeqStage::Idle;
        seq.stage_frames = 0;
        seq.clutch_at_start = report.clutch;
        seq.samples.clear();
    }
    let locked = race.is_some_and(|r| r.input_locked() && !r.is_stale(session.generation()));
    let driving = session.is_playing() && !locked;
    let Ok((car, mut input, state)) = cars.single_mut() else {
        // No live player (teardown window or not yet spawned): nothing
        // to drive, nothing to advance.
        return;
    };
    if !driving {
        *input = VehicleInput::default();
        return;
    }
    *input = seq.stage.input();
    seq.stage_frames += 1;
    if let Some(next) = advance(seq.stage, seq.stage_frames, state.forward_speed) {
        // The loudest loop's computed mix is the stage's audio state —
        // headless runs carry it on the component (no sink exists),
        // the same place `engine_drive` writes it every frame.
        let (mix_volume, mix_speed) = voices
            .iter()
            .filter(|(child, _)| child.parent() == car)
            .map(|(_, v)| (v.mix.volume, v.mix.speed))
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((0.0, 1.0));
        let sample = SeqSample {
            stage: seq.stage.name(),
            rpm: state.rpm,
            gear: state.gear,
            direction: state.direction,
            forward_speed: state.forward_speed,
            mix_volume,
            mix_speed,
            clutch: report.clutch - seq.clutch_at_start,
        };
        seq.samples.push(sample);
        seq.stage = next;
        seq.stage_frames = 0;
        seq.clutch_at_start = report.clutch;
    }
}
