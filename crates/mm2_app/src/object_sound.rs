//! Positional object sounds: the drawbridge motor and bell, the
//! ferries' engines and horns and the Underground's rumble.
//!
//! An [`ObjectSound`] on a moving object binds one
//! `aud/ambient/<name>.csv` table and runs its
//! [`mm2_game::object_audio`] state; the owning driver switches rows
//! on and off (a drawbridge while it moves, the train while it runs),
//! and [`object_sound_voices`] turns the state's cues into voices —
//! loops kept as children while wanted, one-shots fired when no
//! earlier firing of the row still plays. Beyond the table's
//! `Max distance` from the listener the emitter is silent, as in the
//! original. The spatial falloff inside that range is designed (the
//! original's attenuation curve is unrecovered, UNK-25):
//! [`OBJECT_SPATIAL_EDGE`] sets how far down the inverse-square curve
//! the max distance sits.

use std::sync::Arc;

use bevy::audio::{
    AudioPlayer, PlaybackMode, PlaybackSettings, SpatialListener, SpatialScale, Volume,
};
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::cardata::{ObjectAudio, is_sample_sentinel};
use mm2_game::object_audio::{ObjectAudioSpec, ObjectAudioState};
use mm2_game::{Mm2Vfs, Session, SessionEntity, SessionPhase};
use tracing::warn;

use crate::audio::{AudioReport, AudioVoice, PcmAudio, VoiceKind, WaveBank};

/// The emitter's max distance lands this many spatial units from the
/// listener — inverse-square gain `1/edge²` (≈ −19 dB at 3) just
/// before the hard cut. Designed.
pub const OBJECT_SPATIAL_EDGE: f32 = 3.0;

/// One object's sound: its table, the table's runtime state and the
/// emitter speed the speed-gated rows test.
#[derive(Component, Debug, Clone)]
pub struct ObjectSound {
    /// The bound table.
    pub spec: Arc<ObjectAudioSpec>,
    /// Its runtime state; drivers flip rows through `set_active`.
    pub state: ObjectAudioState,
    /// The speed handed to the speed windows (the original passes 0
    /// for every retail object).
    pub speed: f32,
    /// Rows whose wave failed to resolve — retried never, warned once.
    failed: Vec<bool>,
}

impl ObjectSound {
    /// Bind `spec` with a fresh state.
    pub fn new(spec: Arc<ObjectAudioSpec>, seed: u32) -> Self {
        let state = ObjectAudioState::new(&spec, seed);
        let failed = vec![false; spec.samples.len()];
        Self {
            spec,
            state,
            speed: 0.0,
            failed,
        }
    }
}

/// A voice an [`ObjectSound`] owns — a child of the emitter so it
/// rides its transform and dies with it.
#[derive(Component, Debug, Clone, Copy)]
pub struct ObjectSoundVoice {
    /// The table row it plays.
    pub sample: usize,
    /// A loop (kept while wanted) rather than a one-shot.
    pub looped: bool,
}

/// Load `aud/ambient/<name>.csv`; `None` (warned) when it is missing
/// or does not parse — the object then stays silent.
pub fn load_object_audio(vfs: &Vfs, name: &str) -> Option<Arc<ObjectAudioSpec>> {
    let logical = ObjectAudioSpec::logical(name);
    let bytes = match vfs.read_path(&logical) {
        Ok((bytes, _)) => bytes,
        Err(e) => {
            warn!(path = %logical, error = %e, "object sound table unreadable");
            return None;
        }
    };
    match ObjectAudio::parse(&bytes) {
        Ok(table) => Some(Arc::new(ObjectAudioSpec::from_table(name, &table))),
        Err(e) => {
            warn!(path = %logical, error = %e, "object sound table failed to parse");
            None
        }
    }
}

/// Run every emitter's state and keep its voices in step: wanted loops
/// spawned and unwanted ones stopped, due one-shots fired unless the
/// row is still sounding, nothing at all beyond the table's max
/// distance. The state advances only while the session simulates
/// (countdown, play, results) — a pause holds both the timers and,
/// through `sync_audio_pause`, the sinks.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn object_sound_voices(
    mut commands: Commands,
    session: Res<Session>,
    time: Res<Time>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    mut emitters: Query<(Entity, &GlobalTransform, &mut ObjectSound)>,
    voices: Query<(Entity, &ObjectSoundVoice, &ChildOf)>,
) {
    if emitters.is_empty() {
        return;
    }
    let (Some(vfs), Some(mut bank)) = (vfs, bank) else {
        return;
    };
    let running = matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
    );
    let dt = if running { time.delta_secs() } else { 0.0 };
    let ear = listener.iter().next().map(|g| g.translation());
    for (emitter, at, mut sound) in &mut emitters {
        let sound = &mut *sound;
        let cues = if running {
            sound.state.step(&sound.spec, dt, sound.speed)
        } else {
            Default::default()
        };
        // No ear yet (the camera gains its listener a frame after
        // spawn) hears nothing.
        let in_range = ear.is_some_and(|e| e.distance(at.translation()) <= sound.spec.max_distance);
        let mine: Vec<(Entity, ObjectSoundVoice)> = voices
            .iter()
            .filter(|(_, _, parent)| parent.parent() == emitter)
            .map(|(e, v, _)| (e, *v))
            .collect();
        // Loops: keep exactly the wanted rows sounding.
        let wanted: Vec<usize> = if in_range && running {
            cues.loops.clone()
        } else if !running {
            // A held session keeps whatever loops it had (the sinks
            // are paused, not stopped).
            mine.iter()
                .filter(|(_, v)| v.looped)
                .map(|(_, v)| v.sample)
                .collect()
        } else {
            Vec::new()
        };
        for (entity, voice) in &mine {
            if voice.looped && !wanted.contains(&voice.sample) {
                commands.entity(*entity).despawn();
            }
        }
        let scale = OBJECT_SPATIAL_EDGE / sound.spec.max_distance.max(1.0);
        let mut spawn = |sample: usize, gain: f32, looped: bool, report: &mut AudioReport| {
            if sound.failed[sample] {
                return;
            }
            let row = &sound.spec.samples[sample];
            if is_sample_sentinel(&row.name) {
                sound.failed[sample] = true;
                return;
            }
            match bank.load(&vfs.0, &mut waves, &row.name) {
                Ok(handle) => {
                    let volume = if row.volume.is_finite() && row.volume >= 0.0 {
                        row.volume
                    } else {
                        1.0
                    } * gain;
                    commands.spawn((
                        AudioVoice {
                            kind: VoiceKind::Object,
                        },
                        ObjectSoundVoice { sample, looped },
                        SessionEntity(session.generation()),
                        ChildOf(emitter),
                        Transform::default(),
                        AudioPlayer(handle),
                        PlaybackSettings {
                            mode: if looped {
                                PlaybackMode::Loop
                            } else {
                                PlaybackMode::Despawn
                            },
                            volume: Volume::Linear(volume),
                            spatial: true,
                            spatial_scale: Some(SpatialScale::new(scale)),
                            ..Default::default()
                        },
                    ));
                    report.voices += 1;
                    report.objects += 1;
                }
                Err(e) => {
                    sound.failed[sample] = true;
                    report.failed += 1;
                    warn!(table = %sound.spec.name, "audio: {e}");
                }
            }
        };
        for &sample in &wanted {
            if !mine.iter().any(|(_, v)| v.looped && v.sample == sample) {
                spawn(sample, 1.0, true, &mut report);
            }
        }
        if in_range {
            for &(sample, gain) in &cues.fire {
                if !mine.iter().any(|(_, v)| !v.looped && v.sample == sample) {
                    spawn(sample, gain, false, &mut report);
                }
            }
        }
    }
}
