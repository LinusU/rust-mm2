//! Positional object sounds: the drawbridge motor and bell, the
//! ferries' engines and horns and the Underground's rumble.
//!
//! An [`ObjectSound`] on a moving object binds one
//! `aud/ambient/<name>.csv` table and runs its
//! [`mm2_game::object_audio`] state; the owning driver switches rows
//! on and off (a drawbridge while it moves, the train while it runs),
//! and [`object_sound_voices`] turns the state's cues into voices —
//! loops kept as children while wanted, one-shots fired when no
//! earlier firing of the row still plays. As in the original, the
//! emitter is silent (and its timers hold) from the table's
//! `Max distance` out — and wherever its `audible area` excludes the
//! listener ([`crate::underground`]) — and positional rows follow the table's
//! recovered falloff every frame
//! ([`ObjectAudioSpec::falloff`](mm2_game::object_audio::ObjectAudioSpec::falloff)).
//! Bevy's spatial audio supplies only the stereo pan: the voices'
//! spatial scale ([`PAN_ONLY_EDGE`]) keeps every audible distance
//! inside rodio's unattenuated unit sphere, so the inverse-square
//! curve never applies. The pan law is Bevy's (designed); the random
//! pan of type-1 rows is not reproduced — they play centred.

use std::sync::Arc;

use bevy::audio::{
    AudioPlayer, AudioSinkPlayback, PlaybackMode, PlaybackSettings, SpatialAudioSink,
    SpatialListener, SpatialScale, Volume,
};
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::cardata::{ObjectAudio, is_sample_sentinel};
use mm2_game::object_audio::{ObjectAudioSpec, ObjectAudioState};
use mm2_game::{Mm2Vfs, Session, SessionEntity, SessionPhase};
use tracing::warn;

use crate::audio::{AudioReport, AudioVoice, PcmAudio, VoiceKind, WaveBank};
use crate::underground::ListenerRooms;

/// A table's max distance lands this many spatial units from the
/// listener — under rodio's unit radius, inside which spatial voices
/// pan without attenuating, so the falloff is ours alone.
pub const PAN_ONLY_EDGE: f32 = 0.5;

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
/// row is still sounding, positional voices re-levelled to the
/// listener's distance, nothing at all from the table's max distance
/// out. The state advances only while the emitter is in range and the
/// session simulates (countdown, play, results) — the original runs a
/// table only while it holds a sound slot, and a pause holds both the
/// timers and, through `sync_audio_pause`, the sinks.
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
    rooms: Option<Res<ListenerRooms>>,
    mut emitters: Query<(Entity, &GlobalTransform, &mut ObjectSound)>,
    voices: Query<(Entity, &ObjectSoundVoice, &ChildOf)>,
    mut sinks: Query<&mut SpatialAudioSink, With<ObjectSoundVoice>>,
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
    let underground = rooms.is_some_and(|r| r.underground);
    for (emitter, at, mut sound) in &mut emitters {
        let sound = &mut *sound;
        // No ear yet (the camera gains its listener a frame after
        // spawn) hears nothing.
        let distance = ear.map(|e| e.distance(at.translation()));
        let in_range =
            sound.spec.area.admits(underground) && distance.is_some_and(|d| sound.spec.in_range(d));
        let falloff = distance.map_or(0.0, |d| sound.spec.falloff(d));
        let cues = if running && in_range {
            sound.state.step(&sound.spec, dt, sound.speed)
        } else {
            Default::default()
        };
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
        // Positional rows follow the distance every frame.
        for (entity, voice) in &mine {
            let row = &sound.spec.samples[voice.sample];
            if row.kind.is_positional()
                && let Ok(mut sink) = sinks.get_mut(*entity)
            {
                sink.set_volume(Volume::Linear(row_volume(row.volume) * falloff));
            }
        }
        let scale = PAN_ONLY_EDGE / sound.spec.max_distance.max(1.0);
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
                    let positional = row.kind.is_positional();
                    let level = if positional { falloff } else { 1.0 };
                    let volume = row_volume(row.volume) * gain * level;
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
                            spatial: positional,
                            spatial_scale: positional.then(|| SpatialScale::new(scale)),
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

/// An authored row volume, or 1 when the cell is unusable.
fn row_volume(volume: f32) -> f32 {
    if volume.is_finite() && volume >= 0.0 {
        volume
    } else {
        1.0
    }
}
