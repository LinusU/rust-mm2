//! F07-A.2: runtime audio — bounded PCM decode, authored wave
//! resolution, and the session-scoped voice lifecycle.
//!
//! The retail `aud/**` census is entirely uncompressed 16-bit PCM (F07-A),
//! so playback decodes through the project's own bounded
//! [`mm2_formats::wav`] parser rather than a codec chain: a [`PcmAudio`]
//! asset holds normalized f32 samples and feeds Bevy's mixer through
//! [`Decodable`], giving every voice a provable decode path and explicit
//! unsupported-format errors instead of a runtime "unrecognized format".
//!
//! Voices are entities stamped [`SessionEntity`] so the session
//! teardown's `despawn_session_entities` cleans them like every other
//! session object — a restart or unload can never strand a playing
//! sound (F07-AC06's lifecycle half). One-shot voices use
//! [`PlaybackMode::Despawn`]: the mixer removes the entity when the clip
//! ends, so nothing accumulates between presses.
//!
//! The first wired driver is the authored horn (CTL-1's ENTER): the
//! cardata `Horn wave name` resolves through the VFS stem index and
//! plays non-spatial at the authored volume.
//!
//! F07-B.1 adds the engine rig: each drivable `Engine wave name` row
//! spawns one looping voice as a child of the vehicle, and
//! [`engine_drive`] re-mixes every loop's volume/pitch from the sim's
//! engine RPM through the authored fade windows (`mm2_game`'s
//! [`EngineLoopSpec`] — the interpretation is inferred, UNK-25).
//! Voices are children so they despawn with their car and sit at its
//! transform; they loop at volume 0 outside their RPM band rather than
//! attaching/detaching.
//!
//! F07-B.2 extends the rig to every `VehicleAudio` car (opponents now
//! spawn with the component) and puts the mix in space: non-player
//! loops are [`PlaybackSettings::spatial`] emitters heard through a
//! single [`SpatialListener`] that [`audio_listener`] keeps on the
//! active 3-D camera — chase, free or cockpit all hear the field from
//! their own viewpoint. The player's own rig stays non-spatial: the
//! local car *is* the listener's anchor, so its engine does not fade
//! with camera distance (designed policy, DSN-37 — Bevy/rodio spatial
//! is inverse-square plus stereo panning, scaled by
//! [`ENGINE_SPATIAL_SCALE`]).
//!
//! F07-B.3 consumes the deduplicated [`ImpactEvent`] stream: each
//! event's *vehicle* participants each earn one bounded one-shot at the
//! contact point (a remote participant's voice belongs to its own
//! client, the same skip every F05 system applies). The sample comes
//! from the session's [`ImpactAudio`] — the player-side
//! `default_impacts.csv` — resolved through [`impact_category`] and
//! [`pick_impact`] (`mm2_game`): the struck side's `dgBangerData`
//! `AudioId` selects the authored `ID` category (everything else,
//! including all retail props at `AudioId` 0, reads the `WALL`
//! catch-all), `severity × striker mass` — the same impulse estimate
//! the knock pipeline weighs — selects the `min,max force` band,
//! `frequency` weights the pick and the authored volume range supplies
//! the gain. Voices are spatial emitters at the impact point for
//! everyone but the local player, whose impacts stay non-spatial like
//! its engine rig (DSN-37).
//!
//! F07-B.4 adds the surface rig: every `Vehicle` car resolves its
//! grounded wheels through [`SurfaceTables`] — `SurfaceMaterial` → the
//! material's authored `sound` class → a row of the session's
//! [`SurfaceAudio`] (the player-side `default_surface<variant>.csv`;
//! F07-B.8 binds the variant off the session weather — `rainy` →
//! `wet`, else `dry`, designed DSN-43; the exe never references
//! `surfaceice` so the authored ice tables are dead data, AUD-11).
//! The loudest covering
//! `skid wave` band and the loudest `surface wave` rolling loop each
//! earn lazily-spawned `PlaybackMode::Loop` voices, mixed from
//! [`tire_slippage`] (designed quantity — longitudinal over-demand or
//! lateral utilization, UNK-25) and `|forward_speed|` respectively.
//! A held brake at rest and airborne wheels resolve nothing
//! (F07-AC03).
//!
//! F07-B.5 voices the authored clutch sample: [`clutch_voices`] keeps a
//! [`GearWatch`] of the last `(gear, direction)` on every
//! `VehicleAudio` car and plays the cardata `clutch wave name` at
//! `clutch volume` when the committed pair changes — retail cars
//! author `REVERSE`, the trucks `TRUCKGEARSHIFT` (which transitions
//! the original voices is unverified, UNK-25 — the designed trigger is
//! one one-shot per committed drivetrain change, so a multi-gear jump
//! is a single actuation and the first observed state is not a shift).
//! F07-B.6 voices ambient traffic: `mm2_app::traffic` stamps the
//! class's resolved `*_engine.csv` table on each spawned car as
//! [`AmbientAudio`], [`ambient_engine_rigs`] turns it into one
//! [`PlaybackMode::Loop`] spatial child voice per car (bounded by
//! [`MAX_AMBIENT_VOICES`]), and [`ambient_engine_drive`] re-mixes
//! pitch through the authored speed bands off the car's
//! `LinearVelocity` — the same component lane followers and knocked
//! wrecks both publish. The ambient tables' application semantics
//! (which cars bind which table, how speed maps to pitch) are a
//! designed reading under UNK-25.
//!
//! F07-B.7 voices the authored siren programs: the session's
//! [`SirenAudio`] resolves `aud/cardata/player/<city>policesiren.csv`
//! for the local car and `aud/cardata/opponent/policesiren.csv` for
//! opponents — the same split the retail exe's hardcoded
//! `sfpolicesiren`/`londonpolicesiren`/`policesiren` names imply
//! (AUD-10). A horn-row `flags` bit 4 car (`vpcop` on retail) owns a
//! [`Siren`] toggle: [`siren_toggle`] turns each horn-control press
//! into program start/stop (designed, DSN-42 — the original trigger
//! is unverified, UNK-25) and [`siren_drive`] walks the authored
//! `(play time, next index)` chain off the session's fixed tick,
//! holding one `PlaybackMode::Loop` voice on the current sample.
//! Sustained-scrape semantics remain F07-B/C work.
//!
//! F18-B.3 voices the session's precipitation itself: a rainy
//! effective-weather pick (the same `Weather::precipitation` binding
//! the particle rig reads, DSN-60) inserts [`WeatherAudio`], and
//! [`weather_voices`] lazily resolves the authored
//! `<name>exterior`/`<name>interior` bed loops plus `thunder` the
//! exe's `Rainexterior`/`Raininterior`/`Thunder` strings name. The
//! beds crossfade on a shelter probe off the active camera (the
//! particle emitter's declared cover approximation, designed —
//! UNK-25) and the claps fire on a seeded schedule adopting the
//! `13.0`/`15.0` constants the exe stores beside the stems
//! (positional attribution inferred, designed).
//!
//! F18-B.5 voices the session's environmental pre-race commentary:
//! every city session inserts [`CommentaryAudio`], which binds the
//! city's `aud/spchdata/<city>.csv` announcer registry and the
//! `<stem>_prerace` cue tables the effective weather/time-of-day
//! selectors name (the exe's `aud\spchdata\as%d`/`al%d` +
//! `%s_prerace` strings; `weaclr`/`weacldy`/`weafog`/`wearain`,
//! `timemorn`/`timenoon`/`timeeve`/`timenight`).
//! [`commentary_voices`] resolves the registry, draws one speaker
//! from the seeded stream, resolves each cue's drawn wave through
//! the [`WaveBank`], then plays the queue as bounded
//! `PlaybackMode::Despawn` one-shots during `Countdown`/`Playing`.
//! A cue family or wave that resolves nothing counts `failed` once
//! and plays nothing — never substituted (F18-AC06).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use bevy::audio::{
    AudioPlayer, AudioSink, AudioSinkPlayback, ChannelCount, Decodable, PlaybackMode,
    PlaybackSettings, Sample, SampleRate, Source, SpatialAudioSink, SpatialListener, SpatialScale,
    Volume,
};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use tracing::warn;

use avian3d::prelude::{ComputedMass, LinearVelocity, Mass, SpatialQuery};
use mm2_assets::Vfs;
use mm2_content::SurfaceTables;
use mm2_formats::cardata::{self, CardataBody, ImpactTable, SurfaceTable, is_sample_sentinel};
use mm2_formats::spchdata::{AnnouncerIndex, CueTable};
use mm2_formats::wav::{FORMAT_PCM, Wav, lookup_stem};
use mm2_game::{
    AmbientAudio, AmbientEngineSpec, Banger, EngineLoopSpec, EngineMix, ImpactEvent, Mm2Vfs,
    NavRng, ObjectId, ObjectIdentity, Player, PlayerControl, PlayerVehicle, SIREN_FLAG, Session,
    SessionConditions, SessionEntity, SessionPhase, SirenPlayback, SirenSpec, SirenTransition,
    SkidUnit, SurfaceMaterial, SurfaceSpec, SurfaceVariant, VehicleAudio, Weather, cue_wave_stem,
    draw_cue_suffix, draw_speaker, impact_category, pick_impact, prerace_tod_stem,
    prerace_weather_stem, tire_slippage,
};
use mm2_vehicle::{DriveDirection, RemoteReplica, Vehicle, VehicleState};

/// Decode bound: samples (per channel-interleaved count) beyond this
/// are refused — retail waves top out under ~2.5 M samples; the cap
/// exists so a malformed size field can never drive a giant allocation
/// or an effectively endless voice.
const MAX_DECODE_SAMPLES: usize = 8 * 1024 * 1024;
/// Live one-shot horn voices the mixer will hold at once — past this a
/// press is counted and dropped rather than stacking voices (F07's
/// bounded-voices requirement; designed bound).
const MAX_HORN_VOICES: usize = 8;
/// Engine loops one vehicle's rig will spawn — retail tops out at 4
/// authored rows; the cap keeps a malformed giant table from flooding
/// the mixer (designed bound, same contract as `MAX_HORN_VOICES`).
const MAX_ENGINE_VOICES: usize = 8;
/// Engine rigs one session will build — retail rosters top out around
/// a dozen participants; the cap bounds total voice count against a
/// mod roster fielding hundreds of `VehicleAudio` cars (designed
/// bound, F07-B.2). Cars past it report once and stay silent.
const MAX_ENGINE_RIGS: usize = 16;
/// Position scale applied to spatial engine emitters (F07-B.2,
/// designed — DSN-37). Rodio's spatial panner attenuates per ear by
/// `min(1, 1/d²)` over the scaled distance, so 0.25 reads a car 4 m
/// off at ~full authored volume, a racing pack (5–15 m) clearly
/// audible, and a 50 m straggler near silence.
const ENGINE_SPATIAL_SCALE: f32 = 0.25;
/// Live one-shot impact voices the mixer will hold at once — a pile-up
/// beyond this is counted and dropped rather than stacking voices
/// (F07-AC04's bounded-voices requirement; designed bound).
const MAX_IMPACT_VOICES: usize = 12;
/// The impact table the session reads: the player-side
/// `default_impacts.csv` — the local listener's authored mix
/// (designed choice: the opponent file authors the same categories
/// but `WALL` bands two orders of magnitude smaller — an authored
/// inconsistency recorded under UNK-25, not a second binding).
const IMPACT_TABLE: &str = "aud/cardata/player/default_impacts.csv";
/// The probe order for the session's surface table (F07-B.8): the
/// exe's `%s_surface<variant>` format string ahead of
/// `default_surface<variant>` (AUD-11 — what `%s` binds is inferred:
/// the vehicle stem, the same per-name convention the `%s_engine`/
/// `%s_horn` ambient strings use; no `%s_surface*` file ships on
/// retail, so the default always resolves there). Only `dry`/`wet`
/// variants exist — the exe never references `surfaceice`, making the
/// authored ice tables dead data (AUD-11). Both candidate paths read
/// the player side: the local listener's authored mix (DSN-39).
fn surface_table_paths(variant: SurfaceVariant, vehicle: Option<&str>) -> Vec<String> {
    let mut paths = Vec::with_capacity(2);
    if let Some(id) = vehicle {
        paths.push(format!(
            "aud/cardata/player/{id}_surface{}.csv",
            variant.suffix()
        ));
    }
    paths.push(format!(
        "aud/cardata/player/default_surface{}.csv",
        variant.suffix()
    ));
    paths
}
/// Skid band voices one car's rig will hold — retail tops out at 3
/// authored bands per entry; the cap bounds a giant mod table
/// (designed bound, same contract as `MAX_ENGINE_VOICES`).
const MAX_SKID_VOICES: usize = 4;
/// Cars one session will give a surface rig — the same bound the
/// engine rigs take (designed, F07-B.4). Cars past it report once
/// and stay silent.
const MAX_SURFACE_RIGS: usize = 16;
/// Frames a different surface entry must hold before a rig rebuilds
/// its voices — a car straddling a surface boundary can't churn
/// attach/detach every frame (designed debounce).
const SURFACE_DWELL: u8 = 4;
/// Below this `|forward_speed|` a car authors no rolling noise — a
/// parked car on grass does not hum (designed gate; the authored
/// `min surface volume` is the moving car's floor, not a parked
/// drone). Matches the sim's low-speed clamp epsilon.
const ROLL_MIN_SPEED: f32 = 0.5;
/// Live one-shot clutch voices the mixer will hold at once — a field
/// of cars shifting in the same frame past this is counted and dropped
/// rather than stacking voices (designed bound, same contract as
/// [`MAX_HORN_VOICES`]).
const MAX_CLUTCH_VOICES: usize = 8;
/// Ambient-traffic engine loops live at once (F07-B.6) — ambient
/// fleets run dozens of cars, so the bound caps mixer load rather
/// than mirroring a roster size. Cars past it report once and stay
/// silent; a recycled car gets a fresh shot since its marker died
/// with the old body (designed bound, same contract as
/// [`MAX_ENGINE_RIGS`]).
const MAX_AMBIENT_VOICES: usize = 32;
/// Siren programs active at once (F07-B.7) — each active [`Siren`]
/// holds at most one loop voice, so this also bounds live siren
/// voices. Presses past it count a drop and leave the car unsounded
/// (designed bound, same contract as [`MAX_HORN_VOICES`]).
const MAX_SIRENS: usize = 8;

/// A decoded, playback-ready wave: normalized interleaved f32 samples
/// plus the authored rate/channel shape. Produced only by
/// [`decode_wave`], so a live `PcmAudio` always decodes.
#[derive(Asset, TypePath, Clone, Debug)]
pub struct PcmAudio {
    /// Interleaved samples normalized to `[-1.0, 1.0]` (`i16` → f32).
    pub samples: Arc<[f32]>,
    /// Channel count.
    pub channels: ChannelCount,
    /// Frames per second.
    pub sample_rate: SampleRate,
}

impl PcmAudio {
    /// Whole frames in the clip.
    fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.get())
    }

    /// Clip length in seconds — what the commentary sequencer waits
    /// between queued cues.
    pub fn duration(&self) -> f32 {
        self.frames() as f32 / self.sample_rate.get() as f32
    }
}

/// Streaming view over a [`PcmAudio`] — what the mixer pulls samples
/// from. Cheap to build: shares the `Arc`'d sample buffer.
pub struct PcmDecoder {
    audio: PcmAudio,
    pos: usize,
}

impl Iterator for PcmDecoder {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let s = *self.audio.samples.get(self.pos)?;
        self.pos += 1;
        Some(s)
    }
}

impl Source for PcmDecoder {
    fn current_span_len(&self) -> Option<usize> {
        Some((self.audio.samples.len() - self.pos) / usize::from(self.audio.channels.get()))
    }

    fn channels(&self) -> ChannelCount {
        self.audio.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.audio.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.audio.frames() as f64 / f64::from(self.audio.sample_rate.get()),
        ))
    }
}

impl Decodable for PcmAudio {
    type Decoder = PcmDecoder;

    fn decoder(&self) -> Self::Decoder {
        PcmDecoder {
            audio: self.clone(),
            pos: 0,
        }
    }
}

/// Decode one wave file into a [`PcmAudio`]. Errors are explicit:
/// structural RIFF failures, non-PCM/unsupported encodings and the
/// decode bound all surface as strings the caller counts and logs —
/// never a panic or a silent skip.
pub fn decode_wave(bytes: &[u8]) -> Result<PcmAudio, String> {
    let w = Wav::parse(bytes).map_err(|e| e.to_string())?;
    if w.fmt.tag != FORMAT_PCM {
        return Err(format!("unsupported wave format tag {}", w.fmt.tag));
    }
    if w.fmt.bits_per_sample != 16 {
        return Err(format!(
            "unsupported {}-bit wave (16-bit PCM only)",
            w.fmt.bits_per_sample
        ));
    }
    let Some(channels) = ChannelCount::new(w.fmt.channels) else {
        return Err("zero-channel wave".into());
    };
    let Some(sample_rate) = SampleRate::new(w.fmt.sample_rate) else {
        return Err("zero-rate wave".into());
    };
    // The bound is checked on the payload before materializing samples
    // — a giant authored data chunk never drives a giant allocation.
    if w.pcm.len() / 2 > MAX_DECODE_SAMPLES {
        return Err(format!(
            "{} samples exceeds the {}-sample decode bound",
            w.pcm.len() / 2,
            MAX_DECODE_SAMPLES
        ));
    }
    let samples = w
        .samples_i16()
        .ok_or_else(|| "no 16-bit PCM payload".to_string())?;
    Ok(PcmAudio {
        samples: samples
            .iter()
            .map(|s| f32::from(*s) * (1.0 / 32768.0))
            .collect(),
        channels,
        sample_rate,
    })
}

/// Session-scoped wave resolver + decoded-asset cache: indexes every
/// `aud/**.wav` the VFS exposes by [`lookup_stem`], preferring the
/// 22 kHz tree when retail ships a sample under both `aud/aud11/` and
/// `aud/aud22/` (designed preference — which tree the original picks
/// per reference is UNK-25; 22 kHz is the higher-fidelity copy of the
/// same authored sample). Inserted by `load_session_world`, removed on
/// teardown.
#[derive(Resource, Default)]
pub struct WaveBank {
    /// Lookup stem → logical wave path.
    stems: HashMap<String, String>,
    /// Stems under an `aud/*/sirens/` directory — the subtree the
    /// retail executable's `sirens\%s` format string scopes siren
    /// samples to (AUD-10). Siren resolution prefers this map so a
    /// same-named wave outside `sirens/` can never shadow an authored
    /// siren; a stem absent from it falls back to the global map.
    siren_stems: HashMap<String, String>,
    /// Logical path → decoded asset (handles outlive the bank; the
    /// asset store is app-global).
    cache: HashMap<String, Handle<PcmAudio>>,
}

impl WaveBank {
    /// Index the VFS. Preference per stem: `aud/aud22/` first, then any
    /// `aud/audNN/`, then the rest of `aud/`; within a tier the higher
    /// declared `.<n>k` rate wins, then the lexicographically smaller
    /// path — deterministic for any mount set.
    pub fn index(vfs: &Vfs) -> Self {
        let mut stems: HashMap<String, String> = HashMap::new();
        let mut siren_stems: HashMap<String, String> = HashMap::new();
        for logical in vfs.list() {
            if !(logical.starts_with("aud/") && logical.ends_with(".wav")) {
                continue;
            }
            let stem = lookup_stem(&logical);
            let better = match stems.get(&stem) {
                None => true,
                Some(cur) => {
                    let (new, old) = (wave_rank(&logical), wave_rank(cur));
                    new > old || (new == old && logical < *cur)
                }
            };
            if better {
                stems.insert(stem.clone(), logical.clone());
            }
            if logical.contains("/sirens/") {
                let better = match siren_stems.get(&stem) {
                    None => true,
                    Some(cur) => {
                        let (new, old) = (wave_rank(&logical), wave_rank(cur));
                        new > old || (new == old && logical < *cur)
                    }
                };
                if better {
                    siren_stems.insert(stem, logical);
                }
            }
        }
        WaveBank {
            stems,
            siren_stems,
            ..Default::default()
        }
    }

    /// Resolve a cardata sample name (no directory, no suffix) to a
    /// decoded [`PcmAudio`] handle, decoding on first use.
    pub fn load(
        &mut self,
        vfs: &Vfs,
        waves: &mut Assets<PcmAudio>,
        name: &str,
    ) -> Result<Handle<PcmAudio>, String> {
        let stem = name.to_ascii_lowercase();
        let logical = self
            .stems
            .get(&stem)
            .cloned()
            .ok_or_else(|| format!("no wave matches stem {stem:?}"))?;
        self.load_path(vfs, waves, &logical)
    }

    /// Resolve a *siren program* sample name: the `sirens/` subtree
    /// wins when it ships the stem — the `sirens\%s` scope the retail
    /// executable's strings name (AUD-10) — and the global index is
    /// the fallback for a stem shipped outside it (e.g. the flat
    /// `aud11` variants).
    pub fn load_siren(
        &mut self,
        vfs: &Vfs,
        waves: &mut Assets<PcmAudio>,
        name: &str,
    ) -> Result<Handle<PcmAudio>, String> {
        let stem = name.to_ascii_lowercase();
        let logical = self
            .siren_stems
            .get(&stem)
            .or_else(|| self.stems.get(&stem))
            .cloned()
            .ok_or_else(|| format!("no wave matches siren stem {stem:?}"))?;
        self.load_path(vfs, waves, &logical)
    }

    /// Decode (or fetch from cache) one logical wave path.
    fn load_path(
        &mut self,
        vfs: &Vfs,
        waves: &mut Assets<PcmAudio>,
        logical: &str,
    ) -> Result<Handle<PcmAudio>, String> {
        if let Some(h) = self.cache.get(logical) {
            return Ok(h.clone());
        }
        let bytes = vfs
            .read_logical(logical)
            .map_err(|e| format!("{logical}: {e}"))?;
        let audio = decode_wave(&bytes).map_err(|e| format!("{logical}: {e}"))?;
        let h = waves.add(audio);
        self.cache.insert(logical.to_string(), h.clone());
        Ok(h)
    }
}

/// Session-scoped impact table + selection stream (F07-B.3): the
/// parsed player-side `default_impacts.csv` plus the seeded draw the
/// band/frequency picks share. Inserted by `load_session_world` when
/// the table resolves and parses — a missing or malformed table
/// degrades to no resource (warned once, never fabricated), the same
/// absence policy every authored record applies. The draw is seeded
/// per session generation like the spark/texel rigs seed per object,
/// so a replayed session repeats the same picks.
#[derive(Resource)]
pub struct ImpactAudio {
    /// The authored categories in authored order.
    table: ImpactTable,
    /// Deterministic draw stream for band/volume picks.
    rng: NavRng,
}

impl ImpactAudio {
    /// Resolve and parse [`IMPACT_TABLE`] through the VFS; `None` (with
    /// a warn) when the file is absent or the grammar rejects it.
    pub fn load(vfs: &Vfs, generation: u64) -> Option<Self> {
        let bytes = match vfs.read_logical(IMPACT_TABLE) {
            Ok(b) => b,
            Err(e) => {
                warn!("audio: {IMPACT_TABLE}: {e}");
                return None;
            }
        };
        match cardata::parse(IMPACT_TABLE, &bytes) {
            Ok(file) => match file.body {
                CardataBody::Impacts(table) => Some(Self {
                    table,
                    rng: NavRng::new(generation),
                }),
                other => {
                    warn!("audio: {IMPACT_TABLE} parsed as {other:?} — no impact table");
                    None
                }
            },
            Err(e) => {
                warn!("audio: {IMPACT_TABLE}: {e}");
                None
            }
        }
    }
}

/// Session-scoped surface table (F07-B.4/B.8): the parsed player-side
/// `default_surface<variant>.csv` — or the `<vehicle>_surface
/// <variant>` override when a mod authors one (AUD-11's `%s_` probe,
/// vehicle-stem reading inferred) — plus every row's resolved
/// [`SurfaceSpec`], computed once at load so the drive loop stays a
/// numeric read. `variant` is the session weather's pick
/// ([`SurfaceVariant::for_weather`] — designed, DSN-43). Inserted by
/// `load_session_world` when a table on the probe chain resolves and
/// parses — same absence policy as [`ImpactAudio`]: a probe chain
/// that resolves nothing warns once and inserts nothing rather than
/// fabricating a surface row.
#[derive(Resource)]
pub struct SurfaceAudio {
    /// The authored rows — band lists live on `surfaces[i].skids`.
    table: SurfaceTable,
    /// `SurfaceSpec::from_entry` per row, parallel to
    /// `table.surfaces` (the index space `sound_index` produces).
    specs: Vec<SurfaceSpec>,
    /// Which table variant the session's weather bound.
    pub variant: SurfaceVariant,
    /// The logical path that resolved — evidence for the smoke record
    /// and for which probe step won.
    pub path: String,
}

impl SurfaceAudio {
    /// Resolve the session's surface table: `weather` picks the
    /// variant ([`SurfaceVariant::for_weather`], designed DSN-43) and
    /// `vehicle` (the player car's catalog id, when it has one) leads
    /// the per-name probe (AUD-11). A per-vehicle file that is absent
    /// probes silently — stock ships none, so its absence is ordinary —
    /// while one that exists but is unparseable or the wrong cardata
    /// kind warns and falls through to the shared `default_` — the
    /// authored baseline every installation carries; a `default_` that
    /// fails the same way warns and yields no resource. The other
    /// variant is never a substitute — a rainy session with no wet
    /// table gets no surface audio rather than a dry table mislabeled
    /// as wet.
    pub fn load(vfs: &Vfs, weather: Weather, vehicle: Option<&str>) -> Option<Self> {
        let variant = SurfaceVariant::for_weather(weather);
        let paths = surface_table_paths(variant, vehicle);
        let (last, specific) = paths.split_last().expect("the default always probes");
        for path in specific {
            if vfs.resolve(path).is_none() {
                continue;
            }
            match Self::load_path(vfs, path, variant) {
                Ok(table) => return Some(table),
                Err(e) => warn!("audio: {e}"),
            }
        }
        match Self::load_path(vfs, last, variant) {
            Ok(table) => Some(table),
            Err(e) => {
                warn!("audio: {e}");
                None
            }
        }
    }

    /// Read and parse one candidate path; `Err` carries a formatted
    /// reason (absent, malformed or the wrong cardata kind).
    fn load_path(vfs: &Vfs, path: &str, variant: SurfaceVariant) -> Result<Self, String> {
        let bytes = vfs.read_logical(path).map_err(|e| format!("{path}: {e}"))?;
        match cardata::parse(path, &bytes) {
            Ok(file) => match file.body {
                CardataBody::Surfaces(table) => Ok(Self {
                    specs: table.surfaces.iter().map(SurfaceSpec::from_entry).collect(),
                    variant,
                    path: path.to_string(),
                    table,
                }),
                other => Err(format!("{path} parsed as {other:?} — no surface table")),
            },
            Err(e) => Err(format!("{path}: {e}")),
        }
    }
}

/// The player-side siren program path — `aud/cardata/player/
/// <city>policesiren.csv`. The city is keyed by the PSDL stem, the
/// same convention the exe's hardcoded `sfpolicesiren`/
/// `londonpolicesiren` strings name (AUD-10); a mod city resolves its
/// own table or gets none — never another city's.
fn player_siren_path(city: &str) -> String {
    format!("aud/cardata/player/{city}policesiren.csv")
}

/// The opponent-side siren program path — `aud/cardata/opponent/
/// policesiren.csv`, shared by every city on retail.
const OPPONENT_SIREN_PATH: &str = "aud/cardata/opponent/policesiren.csv";

/// Resolve and parse one siren table; `None` (with a warn) when the
/// file is absent, the grammar rejects it, it parses as a different
/// cardata kind, or the program authors no samples — the same absence
/// policy every authored record applies, never a fabricated program.
fn load_siren_program(vfs: &Vfs, path: &str) -> Option<SirenSpec> {
    let bytes = match vfs.read_logical(path) {
        Ok(b) => b,
        Err(e) => {
            warn!("audio: {path}: {e}");
            return None;
        }
    };
    match cardata::parse(path, &bytes) {
        Ok(file) => match file.body {
            CardataBody::Sirens(program) => match SirenSpec::from_program(&program) {
                Some(spec) => Some(spec),
                None => {
                    warn!("audio: {path} authors no siren samples — no program");
                    None
                }
            },
            other => {
                warn!("audio: {path} parsed as {other:?} — no siren program");
                None
            }
        },
        Err(e) => {
            warn!("audio: {path}: {e}");
            None
        }
    }
}

/// Session-scoped siren programs (F07-B.7): the player-side
/// `<city>policesiren.csv` and the shared `opponent/policesiren.csv`,
/// resolved at load, plus the seeded draw the step picks share —
/// generation-seeded like [`ImpactAudio`]'s, so a replayed session
/// repeats the same program. Inserted by `load_session_world` when at
/// least one side resolves; a city with no authored program (or a
/// dev world) yields no resource and flagged cars report their
/// presses as failed instead of playing a substitute.
#[derive(Resource)]
pub struct SirenAudio {
    /// The city program [`PlayerVehicle`] cars read.
    pub player: Option<SirenSpec>,
    /// The shared program opponent cars read.
    pub opponent: Option<SirenSpec>,
    /// Deterministic draw stream for step picks.
    rng: NavRng,
}

impl SirenAudio {
    /// Load both programs through the VFS; `city` is the session's
    /// PSDL stem (`london`, `sf`, a mod city) or `None` for a non-city
    /// world, which has no player-side table by design.
    pub fn load(vfs: &Vfs, generation: u64, city: Option<&str>) -> Option<Self> {
        let player = city.and_then(|c| load_siren_program(vfs, &player_siren_path(c)));
        let opponent = load_siren_program(vfs, OPPONENT_SIREN_PATH);
        if player.is_none() && opponent.is_none() {
            return None;
        }
        Some(SirenAudio {
            player,
            opponent,
            rng: NavRng::new(generation),
        })
    }
}

/// The authored bed stems a precipitation selector binds (F18-B.3):
/// the exe's `Rainexterior`/`Raininterior`/`Thunder` strings sit in
/// one block beside the floats `0.65`, `0.85`, `13.0`, `15.0` and
/// `1.0`, read as the weather-audio struct's constants (positional
/// attribution is an inference — the designed volumes/delays below
/// adopt those values rather than inventing others; the original's
/// mix/trigger semantics are unrecovered, UNK-25).
fn weather_stem(name: &str, suffix: &str) -> String {
    format!("{name}{suffix}")
}

/// Bed-loop volume while the listener sits in the open — the `0.85`
/// constant adjacent to the stems, designed adoption (see above).
const RAIN_EXTERIOR_VOLUME: f32 = 0.85;
/// Bed-loop volume while the listener is sheltered — the `0.65`
/// constant, same provenance.
const RAIN_INTERIOR_VOLUME: f32 = 0.65;
/// Thunder one-shot gain — the `1.0` constant, same provenance.
const THUNDER_VOLUME: f32 = 1.0;
/// Seeded clap-delay bounds in seconds — the `13.0`/`15.0` constants,
/// same provenance.
const THUNDER_MIN_DELAY: f32 = 13.0;
const THUNDER_MAX_DELAY: f32 = 15.0;
/// Exterior↔interior crossfade rate (per second) — designed; no
/// original value is recoverable.
const RAIN_CROSSFADE_PER_SEC: f32 = 4.0;
/// Live thunder one-shots the session will hold at once — claps are
/// seconds apart, so the bound is a contract, not a real constraint
/// (designed, like every other voice bound here).
const MAX_THUNDER_VOICES: usize = 4;
/// Domain separation for the thunder schedule's `NavRng` — the precip
/// rig seeds `NavRng::new(seed)` directly, so this stream gets its own
/// mix of the session seed instead of replaying the drop jitter's
/// draw order (designed).
const WEATHER_AUDIO_DOMAIN: u64 = 0x7765_6120_6175_6469;

/// Session-scoped precipitation ambience (F18-B.3): inserted by
/// `load_session_world` when the shared effective-weather pick names a
/// precipitation spec — the same `Weather::precipitation()` binding
/// the particle rig reads (DSN-60). Carries no decoded waves: the
/// beds' `<name>exterior`/`<name>interior` and `thunder` stems resolve
/// through the session [`WaveBank`] on the drive system's first pass,
/// like every other lazy voice build. `None` on a non-precipitating
/// session, so a dry run carries no ambience state at all.
#[derive(Resource)]
pub struct WeatherAudio {
    /// The bound precipitation name (`"rain"` on retail).
    pub name: &'static str,
    /// Each bed's resolve was attempted (spawn or counted failure —
    /// never retried per frame).
    exterior_tried: bool,
    interior_tried: bool,
    /// The resolved thunder clip, `Some` once its stem decoded.
    thunder_tried: bool,
    thunder: Option<Handle<PcmAudio>>,
    /// Exterior↔interior crossfade position, 0 = fully exposed.
    interior_mix: f32,
    /// Playing-phase seconds accumulated for the thunder schedule.
    elapsed: f32,
    /// `elapsed` at which the next clap fires.
    next_thunder: f32,
    /// Seeded draw stream for the clap delays.
    rng: NavRng,
}

impl WeatherAudio {
    /// Bind the weather selector's precipitation ambience; `None` when
    /// it names no spec (every dry session). The seeded schedule draws
    /// the first clap delay here so a restart replays identically.
    pub fn bind(weather: Weather, seed: u64) -> Option<Self> {
        let name = weather.precipitation()?;
        let mut rng = NavRng::new(seed.wrapping_add(WEATHER_AUDIO_DOMAIN));
        let next_thunder = Self::draw_delay(&mut rng);
        Some(Self {
            name,
            exterior_tried: false,
            interior_tried: false,
            thunder_tried: false,
            thunder: None,
            interior_mix: 0.0,
            elapsed: 0.0,
            next_thunder,
            rng,
        })
    }

    fn draw_delay(rng: &mut NavRng) -> f32 {
        THUNDER_MIN_DELAY + rng.next_f32() * (THUNDER_MAX_DELAY - THUNDER_MIN_DELAY)
    }
}

/// One precipitation bed loop — the `<name>exterior`/`<name>interior`
/// voice [`weather_voices`] spawned. `mix` is rewritten every drive
/// pass whether or not a sink exists — headless runs carry the
/// computed mixer state on the component, which is what tests and the
/// smoke record read (the [`EngineVoice`]/[`SurfaceVoice`] contract).
#[derive(Component)]
pub struct WeatherVoice {
    /// Which authored stem this bed plays.
    pub role: WeatherRole,
    /// The mixer state [`weather_voices`] last computed (speed is
    /// always 1.0 — the authored clips are untempoed beds).
    pub mix: EngineMix,
}

/// Which half of the precipitation bed a [`WeatherVoice`] plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherRole {
    /// The `<name>exterior` open-air bed.
    Exterior,
    /// The `<name>interior` sheltered bed.
    Interior,
}

/// Variant preference for one stem — `(tree tier, declared rate kHz)`.
fn wave_rank(logical: &str) -> (u32, u32) {
    let tier = if logical.starts_with("aud/aud22/") {
        2
    } else if logical.starts_with("aud/aud") {
        1
    } else {
        0
    };
    let rate = logical
        .rsplit_once('.')
        .and_then(|(s, _)| s.rsplit_once('.'))
        .and_then(|(_, sfx)| {
            sfx.strip_suffix(['k', 'K'])
                .and_then(|n| n.parse::<u32>().ok())
        })
        .unwrap_or(0);
    (tier, rate)
}

/// Seconds between queued commentary cues — a designed sequencing
/// gap; the original's cue cadence is unrecovered (UNK-25).
const COMMENTARY_GAP: f32 = 0.25;
/// Commentary one-shot gain — authored level, same policy as the
/// authored horn/impact plays (designed; the original's speech mix
/// is unrecovered, UNK-25).
const COMMENTARY_VOLUME: f32 = 1.0;
/// Domain separation for the commentary `NavRng` — the thunder
/// schedule, precip jitter and wheel-effect streams each hold their
/// own mix of the session seed, so this draw can never alias another
/// system's replay (designed, same contract as
/// [`WEATHER_AUDIO_DOMAIN`]).
const COMMENTARY_DOMAIN: u64 = 0x7370_6368_6461_7461;
/// The `header` section the weather table binds — the exe's cue-type
/// string.
const WEATHER_SECTION: &str = "WEATHER";
/// The `header` section the time-of-day table binds — same block.
const TOD_SECTION: &str = "TIMEOFDAY";

/// Session-scoped environmental commentary (F18-B.5): inserted by
/// `load_session_world` on every city session — the bound registry
/// path plus the two `<stem>_prerace` cue tables the effective
/// selectors name. `commentary_voices` lazily resolves the registry
/// and each cue's drawn wave once inside the pre-race window, then
/// sequences the decoded clips. Dev worlds and cities with no
/// `spchdata` registry carry no resource — no cue is ever fabricated
/// (F18-AC06).
#[derive(Resource)]
pub struct CommentaryAudio {
    /// `aud/spchdata/<city>.csv` — the announcer registry.
    registry: String,
    /// `(section, table stem)` pairs the session plays, weather then
    /// time-of-day (the exe's cue-block order).
    cues: [(&'static str, &'static str); 2],
    /// The registry→speaker→cue→wave resolve ran (each failure
    /// counted once — never retried per frame).
    resolved: bool,
    /// Decoded one-shots awaiting their slot, in authored order —
    /// bounded at the authored cue count by construction.
    queue: VecDeque<(String, Handle<PcmAudio>)>,
    /// Pre-race seconds accumulated (`Countdown`/`Playing` only).
    elapsed: f32,
    /// `elapsed` the next queued cue may start at — the first cue
    /// fires on the first live frame; later ones wait out the
    /// previous clip plus [`COMMENTARY_GAP`].
    next_at: f32,
    /// Seeded draw stream — the speaker index and each cue's suffix
    /// draw ride it, so a replayed session hears the same announcer
    /// and the same lines.
    rng: NavRng,
}

impl CommentaryAudio {
    /// Bind the session's environmental cues; `city` is the PSDL stem
    /// (`sf`, `london`, a mod city) or `None` for a dev world — no
    /// registry, no commentary. The cue pair is the shared
    /// `effective_conditions` pick, so the commentary names the same
    /// weather the lighting/ambience already committed to.
    pub fn bind(city: Option<&str>, conditions: SessionConditions, seed: u64) -> Option<Self> {
        Some(Self {
            registry: format!("aud/spchdata/{}.csv", city?),
            cues: [
                (WEATHER_SECTION, prerace_weather_stem(conditions.weather)),
                (TOD_SECTION, prerace_tod_stem(conditions.time_of_day)),
            ],
            resolved: false,
            queue: VecDeque::new(),
            elapsed: 0.0,
            next_at: 0.0,
            rng: NavRng::new(seed.wrapping_add(COMMENTARY_DOMAIN)),
        })
    }
}

/// A spawned commentary one-shot — the wave stem is kept on the
/// component for the smoke record and tests (headless runs cannot
/// hear the clip, but the stem proves which authored cue played).
#[derive(Component)]
pub struct CommentaryVoice {
    /// The resolved bank stem (`as1wearain02`).
    pub stem: String,
}

/// One playing/queued sound entity. Despawns with its session via
/// [`SessionEntity`]; one-shots additionally self-despawn at clip end
/// through [`PlaybackMode::Despawn`].
#[derive(Component)]
pub struct AudioVoice {
    /// Which authored binding drives this voice.
    pub kind: VoiceKind,
}

/// The authored binding a voice plays (grows with F07-B drivers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceKind {
    /// Listener-side checkpoint/countdown/result effect.
    RaceCue,
    /// The cardata horn sample.
    Horn,
    /// One `Engine wave name` loop of a vehicle's engine rig.
    Engine,
    /// One `default_impacts.csv` sample a deduplicated impact spawned.
    Impact,
    /// One `skid wave` band loop of a surface rig (F07-B.4).
    Skid,
    /// One `surface wave` rolling loop of a surface rig (F07-B.4).
    Rolling,
    /// One `clutch wave name` one-shot a committed gear/direction
    /// change spawned (F07-B.5).
    Clutch,
    /// An ambient-traffic engine loop an `AmbientAudio` car carries
    /// (F07-B.6).
    AmbientEngine,
    /// One siren-program sample loop a [`Siren`] car is playing
    /// (F07-B.7).
    Siren,
    /// One precipitation bed loop (`<name>exterior`/`<name>interior`)
    /// the session's [`WeatherAudio`] crossfades (F18-B.3).
    Weather,
    /// One `thunder` clap the session's [`WeatherAudio`] schedule
    /// spawned (F18-B.3).
    Thunder,
    /// One `spchdata` cue the session's [`CommentaryAudio`] queue
    /// spawned (F18-B.5).
    Commentary,
    /// One row of a moving object's `aud/ambient` table — a
    /// drawbridge, ferry or Underground sound
    /// (`crate::object_sound`).
    Object,
}

/// Marker on a vehicle whose engine rig was built — set once whether
/// or not any row produced a voice, so a car whose rows all fail
/// reports once instead of retrying (and re-warning) every frame.
#[derive(Component)]
pub struct EngineRig;

/// One looping voice bound to an authored `Engine wave name` row, a
/// child of the vehicle so it despawns with the car and rides its
/// transform for the spatial leg. `mix` is rewritten every drive tick
/// whether or not a sink exists — headless runs carry the computed
/// mixer state on the component, which is what tests and the smoke
/// record read.
#[derive(Component)]
pub struct EngineVoice {
    /// Index into the parent's `VehicleAudio::spec.engine_samples`.
    pub row: usize,
    /// The resolved fade-window spec, captured at rig build.
    pub spec: EngineLoopSpec,
    /// The mixer state [`engine_drive`] last computed.
    pub mix: EngineMix,
}

/// Per-car surface voice state (F07-B.4): which `sound`-index entries
/// the skid and rolling halves are committed to, plus the voice slots
/// of the committed entry. Band loops are spawned lazily — a `skid
/// wave` row's voice exists only once that band has covered — and an
/// entry rebuild waits for the new entry to hold [`SURFACE_DWELL`]
/// frames, so a car straddling a surface boundary can't churn sink
/// attach/detach every frame. Voices are children of the car like the
/// engine rig's, so they despawn with it and ride its transform.
#[derive(Component, Default)]
pub struct SurfaceRig {
    /// The committed skid entry (`sound` index into the table) —
    /// `None` until a band first covers.
    skid_entry: Option<u16>,
    /// `(entry, consecutive frames)` a different winning entry has
    /// held — the dwell counter for the swap.
    skid_pending: Option<(u16, u8)>,
    /// One slot per committed-entry band, capped at
    /// [`MAX_SKID_VOICES`]. `tried` stops a resolve/decode failure
    /// re-warning every frame.
    skid_slots: Vec<SkidSlot>,
    /// The committed rolling entry, `None` until a rolling surface
    /// first resolves under a moving car.
    rolling_entry: Option<u16>,
    /// Same dwell counter for the rolling half.
    rolling_pending: Option<(u16, u8)>,
    /// Whether the committed rolling sample was attempted (success or
    /// counted failure — never retried per frame).
    rolling_tried: bool,
    /// The rolling voice entity, when spawned.
    rolling_voice: Option<Entity>,
    /// [`MAX_SURFACE_RIGS`] refused this car a rig — counted once.
    muted: bool,
}

/// One band's voice slot inside a committed skid entry.
#[derive(Default)]
struct SkidSlot {
    /// The band's sample was attempted (spawn or counted failure).
    tried: bool,
    /// The spawned band-loop voice.
    voice: Option<Entity>,
}

/// The dominant wheel contact [`surface_voices`] resolved for a car —
/// F25-B protocol v16's wire source on the authority and a remote
/// copy's replicated presentation state on a client. The system writes
/// it on every car its wheel loop resolves (player, wire seat, AI
/// opponent, ambient), so `netdrive::publish_snapshots` can encode a
/// seat's `SnapEntry` tail from it; `netdrive`'s apply half instead
/// writes it on a `RemoteReplica` copy off the wire, and this system
/// reads the copy's back rather than running the wheel loop — a
/// kinematic replica has no contact truth of its own.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct SurfaceContact {
    /// The loudest skid contact's truth — `None` while no grounded
    /// wheel's pick resolves.
    pub skid: Option<SkidContact>,
    /// The loudest rolling contact's `sound` class — the loop's speed
    /// is the car's `forward_speed`, so nothing else rides.
    pub roll: Option<u16>,
}

/// One skid contact's truth — see [`SurfaceContact::skid`]. `surface`
/// is the *resolved* `sound` class (`SurfaceTables::sound_index`'s row
/// — the authored selector city content pins, so identical on every
/// peer), while the quantities are the winning wheel's own so a
/// receiver replays `SkidSpec::pick` under its spec's unit rather than
/// trusting the authority's band.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkidContact {
    /// `SurfaceTables::sound_index`'s row for the winning wheel's
    /// material — the class space `SnapEntry::surf_skid` carries.
    pub surface: u16,
    /// `tire_slippage`'s clamped 0..1 utilization on that wheel.
    pub slippage: f32,
    /// The wheel's `vel_long`, m/s — `Speed`-unit picks read
    /// `|vel_long|`.
    pub wheel_speed: f32,
}

/// A looping surface voice — a skid band or a rolling loop, a child
/// of the car like [`EngineVoice`]. `mix` is rewritten every drive
/// pass whether or not a sink exists, which is what headless runs and
/// tests read for evidence.
#[derive(Component)]
pub struct SurfaceVoice {
    /// Which half of the surface row this voice plays.
    pub role: SurfaceRole,
    /// The mixer state [`surface_voices`] last computed.
    pub mix: EngineMix,
}

/// Which half of a committed surface entry a [`SurfaceVoice`] plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// The `skid wave` band at this index in the committed entry.
    Skid(usize),
    /// The entry's `surface wave` rolling loop.
    Rolling,
}

/// The last drivetrain state [`clutch_voices`] observed on this car —
/// the shift trigger's dedup. A voice is earned only when the
/// committed `(gear, direction)` pair differs from the cached one;
/// first sight inserts the watch without a voice (a spawned car has
/// not "shifted into" its initial gear), and the watch updates even
/// while not `Playing` so a countdown or reset change never flushes
/// as a stale clunk.
#[derive(Component)]
pub struct GearWatch {
    /// `VehicleState::gear` at the last observation.
    gear: usize,
    /// `VehicleState::direction` at the last observation.
    direction: DriveDirection,
}

/// Marker on an ambient car whose engine voice was attempted — set
/// once whether the resolve spawned a voice or failed, so a car whose
/// sample cannot decode reports once instead of retrying (and
/// re-warning) every frame. Despawns with the car, so a recycled
/// body re-attempts its class's sample (F07-B.6).
#[derive(Component)]
pub struct AmbientRig;

/// An ambient car's looping engine voice — a child of the car like
/// [`EngineVoice`], so it despawns with it and rides its transform as
/// the spatial emitter. `mix` is rewritten every drive tick whether or
/// not a sink exists — headless runs carry the computed mixer state on
/// the component, which is what tests and the smoke record read.
#[derive(Component)]
pub struct AmbientEngineVoice {
    /// The resolved table spec, captured at rig build.
    pub spec: AmbientEngineSpec,
    /// The mixer state [`ambient_engine_drive`] last computed.
    pub mix: EngineMix,
}

/// An active siren program on a flagged car (F07-B.7). [`siren_toggle`]
/// inserts it when a `SIREN_FLAG` car's horn control is pressed and
/// removes it on the next press (the designed press-to-toggle reading,
/// DSN-42 — the original's trigger is unverified, UNK-25) or when the
/// program's authored chain ends. [`siren_drive`] advances the machine
/// off the session's fixed tick — a pause or a pre-`Playing` phase
/// freezes the program where it stands, matching the sink hold — and
/// keeps one `PlaybackMode::Loop` voice on the current sample: a child
/// of the car so it despawns with it and rides its transform (spatial
/// for non-local cars, non-spatial on the player's — the DSN-37
/// anchor rule).
#[derive(Component)]
pub struct Siren {
    /// Program position the drive loop advances.
    pub play: SirenPlayback,
    /// Session tick the machine last consumed — the program clock is
    /// the fixed-step clock, not wall time, so a replayed session
    /// repeats the same switch frames and a paused one holds.
    last_tick: u64,
    /// The loop voice playing `play.sample`, when it resolved.
    voice: Option<Entity>,
    /// `play.sample`'s resolve was attempted this visit — success
    /// leaves `voice` set, a sentinel means authored silence and a
    /// failure was counted; all three stay off the resolve path until
    /// the next `Switch`.
    resolved: bool,
    /// Stems that failed to resolve this activation — a bad sample
    /// warns once per activation, then plays silent for its dwell.
    failed_stems: Vec<String>,
}

impl Siren {
    /// Enter `spec`'s program at the authored first sample — `None`
    /// when the program has no usable first step. The activation path
    /// for the horn toggle now and opponent AI later (F20).
    pub fn activate(spec: &SirenSpec, rng: &mut NavRng, tick: u64) -> Option<Self> {
        SirenPlayback::start(spec, rng).map(|play| Siren {
            play,
            last_tick: tick,
            voice: None,
            resolved: false,
            failed_stems: Vec::new(),
        })
    }
}

/// A horn actuation intent — written by [`horn_input`] (live input) and
/// `dev_horn_once` (`--horn` evidence runs, where capture freezes input
/// upstream), consumed by [`horn_voices`] (unflagged cars → horn
/// voices) and [`siren_toggle`] (`SIREN_FLAG` cars → program toggles).
/// One message = one press.
#[derive(Message)]
pub struct HornRequest;

/// Session-scoped audio counters the smoke record reports (`aud=`).
#[derive(Resource, Default, Debug)]
pub struct AudioReport {
    /// Horn presses consumed this session.
    pub horns: u64,
    /// Voice entities spawned this session.
    pub voices: u64,
    /// Voices the output device attached a sink to (0 headless/no-device).
    pub sunk: u64,
    /// Presses/loops refused by a voice bound.
    pub dropped: u64,
    /// Resolves/decodes that failed this session.
    pub failed: u64,
    /// Engine loop voices spawned this session (a subset of `voices`).
    pub loops: u64,
    /// Engine loops whose last computed mix is audible — a gauge
    /// rewritten every drive pass, not a cumulative count.
    pub audible: u64,
    /// Engine rigs built this session (one per `VehicleAudio` car).
    pub rigs: u64,
    /// Impact voices spawned this session (a subset of `voices`).
    pub impacts: u64,
    /// Skid band voices whose last computed mix is audible — a gauge
    /// rewritten every drive pass, not a cumulative count (F07-B.4).
    pub skids: u64,
    /// Rolling-loop voices whose last computed mix is audible — the
    /// same gauge for the rolling half.
    pub rolling: u64,
    /// Skid band voices spawned this session — a subset of `voices`.
    /// Unlike `skids` this never rewinds: a voice that resolved stays
    /// counted after its mix fades or the contact clears (F07-B.4).
    pub skid_voices: u64,
    /// Rolling-loop voices spawned this session — a subset of
    /// `voices`; the same cumulative measure for the rolling half.
    pub rolling_voices: u64,
    /// Clutch voices spawned this session — a subset of `voices`
    /// (F07-B.5).
    pub clutch: u64,
    /// Ambient-traffic engine voices spawned this session — a subset
    /// of `voices` (F07-B.6).
    pub ambient: u64,
    /// Ambient engine loops whose last computed mix is audible — a
    /// gauge rewritten every drive pass, not a cumulative count
    /// (F07-B.6).
    pub ambient_live: u64,
    /// Siren loop voices spawned this session — a subset of `voices`;
    /// each authored program switch respawns one (F07-B.7).
    pub sirens: u64,
    /// Siren programs currently active — a gauge rewritten every
    /// drive pass, not a cumulative count (F07-B.7).
    pub siren_live: u64,
    /// Precipitation bed voices spawned this session — a subset of
    /// `voices` (F18-B.3); at most one exterior + one interior.
    pub weather: u64,
    /// Thunder clap voices spawned this session — a subset of
    /// `voices` (F18-B.3).
    pub thunder: u64,
    /// Whether the shelter probe last held the bed at interior — a
    /// gauge rewritten every drive pass, not a cumulative count
    /// (F18-B.3).
    pub interior: bool,
    /// Commentary cue voices spawned this session — a subset of
    /// `voices` (F18-B.5); at most one per bound cue family.
    pub commentary: u64,
    /// Countdown, checkpoint, low-time and terminal race effects.
    pub race_cues: u64,
    /// Object-sound voices spawned this session — a subset of
    /// `voices` (drawbridges, ferries, the Underground).
    pub objects: u64,
}

impl AudioReport {
    /// Any activity worth reporting (`aud=` stays absent otherwise).
    pub fn active(&self) -> bool {
        self.horns
            + self.voices
            + self.dropped
            + self.failed
            + self.loops
            + self.rigs
            + self.skids
            + self.rolling
            + self.clutch
            + self.ambient
            + self.ambient_live
            + self.sirens
            + self.siren_live
            + self.weather
            + self.thunder
            + self.interior as u64
            + self.commentary
            + self.race_cues
            + self.objects
            > 0
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Session-scoped reset — scheduled `run_if(session::unloading)`
/// alongside the other report resets `drive_session` performs (kept
/// separate so the driver stays under Bevy's system-parameter limit).
pub fn reset_audio_report(mut report: ResMut<AudioReport>) {
    report.reset();
}

/// Horn press → [`HornRequest`] — ENTER per CTL-1, gated like driving
/// input: `Playing` session and a focused window (headless has no
/// windows → focused). Capture runs freeze this system with the other
/// input systems; `--horn` reaches the same consumer through
/// [`dev_horn_once`].
pub fn horn_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    session: Res<Session>,
    windows: Query<&Window>,
    mut requests: MessageWriter<HornRequest>,
) {
    if crate::input::control_just_pressed(
        &keys,
        &pads,
        &windows,
        KeyCode::Enter,
        crate::input::pad::HORN,
    ) && session.is_playing()
    {
        requests.write(HornRequest);
    }
}

/// `--horn` driver: one [`HornRequest`] on the first `Playing` frame,
/// reading the flag through the session config so headless runs reach
/// it. Mirrors `pause::dev_pause_once`.
pub fn dev_horn_once(
    session: Res<Session>,
    mut fired: Local<bool>,
    mut requests: MessageWriter<HornRequest>,
) {
    if *fired || !session.config().is_some_and(|c| c.dev.horn) {
        return;
    }
    if session.is_playing() {
        requests.write(HornRequest);
        *fired = true;
    }
}

/// Consume [`HornRequest`]s: resolve the local player's authored horn
/// through the [`WaveBank`] and spawn one bounded one-shot voice per
/// press. Player-only for now — opponent/ambient horn wiring (and the
/// spatial mix) is F07-B.
///
/// A car whose horn-row `flags` carry [`SIREN_FLAG`] is skipped here:
/// its presses belong to [`siren_toggle`] — the flag modifies the horn
/// control's behavior, so a siren-capable car toggles its program
/// rather than sounding the authored horn sample (designed, DSN-42).
#[allow(clippy::too_many_arguments)]
pub fn horn_voices(
    mut commands: Commands,
    mut requests: MessageReader<HornRequest>,
    session: Res<Session>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    cars: Query<&VehicleAudio, With<PlayerVehicle>>,
    voices: Query<&AudioVoice>,
) {
    let pending = requests.read().count();
    if pending == 0 {
        return;
    }
    report.horns += pending as u64;
    let Some(car) = cars.iter().next() else {
        return;
    };
    if car.spec.flags & SIREN_FLAG != 0 {
        // Siren-capable: `siren_toggle` owns these presses.
        return;
    }
    let (Some(vfs), Some(mut bank)) = (vfs, bank) else {
        // No mounted content or no session world: presses are counted,
        // nothing can resolve.
        report.failed += pending as u64;
        return;
    };
    let mut live = voices.iter().filter(|v| v.kind == VoiceKind::Horn).count();
    for _ in 0..pending {
        if live >= MAX_HORN_VOICES {
            report.dropped += 1;
            continue;
        }
        match bank.load(&vfs.0, &mut waves, &car.spec.horn.name) {
            Ok(handle) => {
                commands.spawn((
                    AudioVoice {
                        kind: VoiceKind::Horn,
                    },
                    SessionEntity(session.generation()),
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: PlaybackMode::Despawn,
                        volume: Volume::Linear(authored_volume(car.spec.horn.volume)),
                        ..Default::default()
                    },
                ));
                report.voices += 1;
                live += 1;
            }
            Err(e) => {
                report.failed += 1;
                warn!("audio: {e}");
            }
        }
    }
}

/// Authored volume sanitized for the mixer — a non-finite or negative
/// cardata value binds 1.0 rather than poisoning the sink (the parse
/// anomaly is already surfaced at load; designed guard).
fn authored_volume(v: f32) -> f32 {
    if v.is_finite() && v >= 0.0 { v } else { 1.0 }
}

/// The siren half of the horn control (F07-B.7): each [`HornRequest`]
/// press on a `SIREN_FLAG` player car toggles its [`Siren`] — the
/// designed press-to-toggle reading of the horn-row flag (DSN-42; the
/// original's trigger is unverified, UNK-25). On: [`SirenPlayback::start`]
/// enters the role's program at the authored first sample and
/// [`siren_drive`] voices it the same update. Off: the component and
/// its loop voice die together. A press on a flagged car with no
/// resolved program counts and warns like a failed horn resolve —
/// never a substitute sound — and presses past [`MAX_SIRENS`] drop.
pub fn siren_toggle(
    mut commands: Commands,
    mut requests: MessageReader<HornRequest>,
    session: Res<Session>,
    mut siren_audio: Option<ResMut<SirenAudio>>,
    mut report: ResMut<AudioReport>,
    cars: Query<(Entity, &VehicleAudio, Option<&Siren>), With<PlayerVehicle>>,
    sirens: Query<(), With<Siren>>,
) {
    let pending = requests.read().count();
    if pending == 0 {
        return;
    }
    let Some((car, audio, current)) = cars.iter().next() else {
        return;
    };
    if audio.spec.flags & SIREN_FLAG == 0 {
        return;
    }
    let mut active = current.is_some();
    // The voice the active component owns — tracked locally because a
    // same-frame off→on cycle must not despawn the *new* activation's
    // (not yet spawned) voice or double-despawn the old one's.
    let mut voice = current.and_then(|s| s.voice);
    let mut live = sirens.iter().count();
    for _ in 0..pending {
        if active {
            if let Some(v) = voice.take() {
                commands.entity(v).despawn();
            }
            commands.entity(car).remove::<Siren>();
            active = false;
            continue;
        }
        if live >= MAX_SIRENS {
            report.dropped += 1;
            warn!("audio: siren bound reached, {car:?} stays silent");
            continue;
        }
        let Some(audio) = siren_audio.as_deref_mut() else {
            // The session resolved no siren table at all.
            report.failed += 1;
            warn!("audio: {car:?} is siren-flagged but no siren program loaded");
            continue;
        };
        // The toggle is the player's control — the player-side program
        // drives the start (field borrow keeps `rng` disjoint).
        let Some(spec) = audio.player.as_ref() else {
            report.failed += 1;
            warn!("audio: {car:?} is siren-flagged but the city ships no program");
            continue;
        };
        match Siren::activate(spec, &mut audio.rng, session.tick()) {
            Some(siren) => {
                commands.entity(car).insert(siren);
                active = true;
                live += 1;
            }
            None => {
                // An empty program start — the resolve already warned.
                report.failed += 1;
            }
        }
    }
}

/// Spawn (or mark silent) the loop voice for `siren`'s current program
/// sample. A sentinel name is authored silence; a resolve/decode
/// failure warns once per stem per activation and stays silent for the
/// step's dwell; a success replaces `siren.voice` with a
/// `PlaybackMode::Loop` child of the car.
#[allow(clippy::too_many_arguments)] // the spawn bundle threads the shared stores through
fn resolve_siren_sample(
    commands: &mut Commands,
    session: &Session,
    vfs: &Vfs,
    bank: &mut WaveBank,
    waves: &mut Assets<PcmAudio>,
    report: &mut AudioReport,
    car: Entity,
    player: bool,
    siren: &mut Siren,
    spec: &SirenSpec,
) {
    siren.resolved = true;
    let Some(sample) = spec.samples.get(siren.play.sample) else {
        return;
    };
    if is_sample_sentinel(&sample.name) {
        return;
    }
    match bank.load_siren(vfs, waves, &sample.name) {
        Ok(handle) => {
            let voice = commands
                .spawn((
                    AudioVoice {
                        kind: VoiceKind::Siren,
                    },
                    SessionEntity(session.generation()),
                    ChildOf(car),
                    Transform::default(),
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: PlaybackMode::Loop,
                        volume: Volume::Linear(authored_volume(sample.volume)),
                        // Non-local sirens are world emitters; the
                        // player's own siren stays non-spatial — the
                        // same DSN-37 anchor rule the engine rig uses.
                        spatial: !player,
                        spatial_scale: (!player).then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                        ..Default::default()
                    },
                ))
                .id();
            siren.voice = Some(voice);
            report.voices += 1;
            report.sirens += 1;
        }
        Err(e) => {
            let stem = sample.name.to_ascii_lowercase();
            if !siren.failed_stems.contains(&stem) {
                siren.failed_stems.push(stem);
                report.failed += 1;
                warn!("audio: {e}");
            }
        }
    }
}

/// Advance every active [`Siren`] off the session's fixed-step clock
/// and keep its voice on the sample the machine landed on (F07-B.7).
/// The program clock is `Session::tick` × the fixed timestep: real
/// playback time while `Playing`, frozen through `Paused`/
/// `Countdown`/`Results` like the sinks [`sync_audio_pause`] holds —
/// and identical across replays. A `Switch` respawns the loop on the
/// new sample; `End` (a malformed program's only exit — retail chains
/// cycle) deactivates like a second press. With no [`SirenAudio`]
/// resource — mid-teardown — active sirens deactivate rather than
/// stepping blind.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn siren_drive(
    mut commands: Commands,
    session: Res<Session>,
    time: Res<Time<Fixed>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    siren_audio: Option<ResMut<SirenAudio>>,
    mut report: ResMut<AudioReport>,
    mut cars: Query<(Entity, &mut Siren, Has<PlayerVehicle>)>,
) {
    report.siren_live = 0;
    if cars.is_empty() {
        return;
    }
    let (Some(vfs), Some(mut bank), Some(mut siren_audio)) = (vfs, bank, siren_audio) else {
        // The session's audio world is gone (teardown window): the
        // voices die with the despawn sweep; drop the state components
        // so nothing advances against a missing table.
        for (car, mut siren, _) in &mut cars {
            if let Some(v) = siren.voice.take() {
                commands.entity(v).despawn();
            }
            commands.entity(car).remove::<Siren>();
        }
        return;
    };
    let tick = session.tick();
    let step = time.timestep().as_secs_f32();
    let siren_audio = &mut *siren_audio;
    for (car, mut siren, player) in &mut cars {
        // Field borrow, not `spec()` — `rng` stays disjoint-mutably
        // borrowable while the program is read.
        let spec = if player {
            siren_audio.player.as_ref()
        } else {
            siren_audio.opponent.as_ref()
        };
        let Some(spec) = spec else {
            // The car's role has no program — it can only have gotten
            // here through a spec that later vanished; deactivate.
            if let Some(v) = siren.voice.take() {
                commands.entity(v).despawn();
            }
            commands.entity(car).remove::<Siren>();
            continue;
        };
        let dt = tick.saturating_sub(siren.last_tick) as f32 * step;
        siren.last_tick = tick;
        if !siren.resolved {
            resolve_siren_sample(
                &mut commands,
                &session,
                &vfs.0,
                &mut bank,
                &mut waves,
                &mut report,
                car,
                player,
                &mut siren,
                spec,
            );
        }
        match siren.play.advance(spec, dt, &mut siren_audio.rng) {
            SirenTransition::Hold => {}
            SirenTransition::Switch(_) => {
                if let Some(v) = siren.voice.take() {
                    commands.entity(v).despawn();
                }
                siren.resolved = false;
                resolve_siren_sample(
                    &mut commands,
                    &session,
                    &vfs.0,
                    &mut bank,
                    &mut waves,
                    &mut report,
                    car,
                    player,
                    &mut siren,
                    spec,
                );
            }
            SirenTransition::End => {
                if let Some(v) = siren.voice.take() {
                    commands.entity(v).despawn();
                }
                commands.entity(car).remove::<Siren>();
                continue;
            }
        }
        report.siren_live += 1;
    }
}

/// Deduplicated impact → one-shot voices (F07-B.3, spec req 4). Each
/// event's *vehicle* participants each earn a voice: the original
/// drives impact audio off the car's own impact callback, so a
/// two-car crash sounds once per car, each at its own impulse. A
/// remote participant belongs to its authority's client — the same
/// skip every F05 consumer applies. A participant that is not a
/// vehicle (a knocked prop meeting the world, banger-on-banger)
/// produces no voice — the authored table is car-audio data and there
/// is no striker to weigh.
///
/// Per striker: the *other* side's `dgBangerData` `AudioId` selects the
/// authored category (`ID` column; anything without a record falls to
/// the id-0 `WALL` catch-all — binding designed, UNK-25), the impulse
/// `severity × striker mass` picks the `min,max force` band (the same
/// estimate `impulse_estimate` feeds the knock pipeline; whether the
/// original weighs this quantity is unverified), `frequency` weights
/// the covering samples and the authored volume range draws the gain —
/// all through `mm2_game`'s [`pick_impact`] on the session's seeded
/// [`ImpactAudio`]. The voice is a spatial emitter at the impact
/// point for everyone but the local player (DSN-37 anchor), bounded
/// by [`MAX_IMPACT_VOICES`] so a pile-up counts drops instead of
/// stacking voices; a sub-floor force resolves no band and stays
/// authored-silent. Events on a stale generation are skipped; while
/// not `Playing` both readers drain without emitting (spark_fx's
/// contract — a buffered stale impact never flushes sound into a
/// pause or the next session).
///
/// A remote-controlled seat voices on the authority like an AI car —
/// it is a locally simulated participant there. On a predicted client
/// its local-stream hits stay skipped: the same impact arrives as a
/// [`crate::netdrive::RemoteImpact`] row off the v10 `Snap.impacts`
/// tail, resolved to this process's copy, so the sound plays once —
/// from the authority's stream. The v12 row carries the struck side's
/// authored `AudioId` resolved on the authority — a receiver cannot
/// resolve a struck prop itself, its `ObjectId` lives in the
/// authority's local id namespace — so a replicated prop hit picks the
/// same authored category the authority played (still the id-0
/// catch-all for world/seat/recordless struck sides; the striker
/// side's mass is the copy's own).
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn impact_voices(
    mut commands: Commands,
    mut reader: MessageReader<ImpactEvent>,
    mut remote_reader: MessageReader<crate::netdrive::RemoteImpact>,
    session: Res<Session>,
    table: Option<ResMut<ImpactAudio>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    identities: Query<(Entity, &ObjectIdentity, Option<&Player>)>,
    cars: Query<(Option<&Vehicle>, Option<&ComputedMass>, Option<&Mass>)>,
    bangers: Query<&Banger>,
    voices: Query<&AudioVoice>,
) {
    if !session.is_playing() {
        reader.read().for_each(drop);
        remote_reader.read().for_each(drop);
        return;
    }
    let (Some(mut table), Some(vfs), Some(mut bank)) = (table, vfs, bank) else {
        // No authored table (or no session world yet): the stream is
        // still consumed next frame — an absent record degrades to
        // silence rather than fabricating a category.
        return;
    };
    // Disjoint field borrows (`&table.table` vs `&mut table.rng`) do
    // not split through `ResMut`'s Deref — reborrow the inner struct.
    let table = &mut *table;
    let generation = session.generation();
    let authority = session.authority_role().is_authority();
    let index: HashMap<ObjectId, (Entity, Option<PlayerControl>)> = identities
        .iter()
        .map(|(entity, id, player)| (id.0, (entity, player.map(|p| p.control))))
        .collect();
    let mut live = voices
        .iter()
        .filter(|v| v.kind == VoiceKind::Impact)
        .count();
    // The shared voice tail: `point`/`severity` and the striker's
    // resolved mass pick the authored band whichever stream delivered
    // the hit; `audio_id` is the struck side's selector (resolved
    // locally, or carried on the wire row — protocol v12).
    let mut voice_at = |entity: Entity,
                        point: Vec3,
                        severity: f32,
                        audio_id: i64,
                        spatial: bool,
                        live: &mut usize| {
        let Ok((vehicle, computed, mass)) = cars.get(entity) else {
            return;
        };
        let Some(vehicle) = vehicle else { return };
        // The designed force quantity: approach speed × the striker's
        // resolved mass (computed → authored → config), matching
        // `impulse_estimate`'s contract. Each source is checked before
        // the fallback — a not-yet-computed `Mass(0)` must fall through
        // to the config mass, not collapse the pick to a 1 kg touch.
        let valid = |m: f32| (m.is_finite() && m > 0.0).then_some(m);
        let striker_mass = computed
            .and_then(|m| valid(m.value()))
            .or_else(|| mass.and_then(|m| valid(m.0)))
            .or_else(|| valid(vehicle.config.mass))
            .unwrap_or(1.0);
        let force = severity * striker_mass;
        let Some(category) = impact_category(&table.table, audio_id) else {
            // The table cannot answer this selector at all — a
            // data failure, counted once per event side.
            report.failed += 1;
            warn!("audio: no impact category for AudioId {audio_id}");
            return;
        };
        let Some(pick) = pick_impact(category, force, &mut table.rng) else {
            // No band covers the force — authored silence for a
            // sub-floor touch, not a failure.
            return;
        };
        if *live >= MAX_IMPACT_VOICES {
            report.dropped += 1;
            return;
        }
        match bank.load(&vfs.0, &mut waves, &pick.sample.name) {
            Ok(handle) => {
                // The local player's hits anchor the mix non-spatially
                // like its engine rig (DSN-37); everyone else is a
                // world emitter at the contact.
                commands.spawn((
                    AudioVoice {
                        kind: VoiceKind::Impact,
                    },
                    SessionEntity(generation),
                    Transform::from_translation(point),
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: PlaybackMode::Despawn,
                        volume: Volume::Linear(pick.volume),
                        spatial,
                        spatial_scale: spatial.then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                        ..Default::default()
                    },
                ));
                report.voices += 1;
                report.impacts += 1;
                *live += 1;
            }
            Err(e) => {
                report.failed += 1;
                warn!("audio: {e}");
            }
        }
    };
    for event in reader.read() {
        if event.generation != generation {
            continue;
        }
        for side in 0..2 {
            let (me, other) = if side == 0 {
                (event.participants.0, event.participants.1)
            } else {
                (event.participants.1, event.participants.0)
            };
            let Some(&(entity, control)) = index.get(&me) else {
                continue;
            };
            if !authority && control == Some(PlayerControl::Remote) {
                continue;
            }
            // The struck side's authored selector — its banger record's
            // `AudioId`; world geometry and recordless bodies read the
            // id-0 catch-all.
            let audio_id = index
                .get(&other)
                .and_then(|(e, _)| bangers.get(*e).ok())
                .map(|b| b.def.audio_id)
                .unwrap_or(0);
            let spatial = control != Some(PlayerControl::Local);
            voice_at(
                entity,
                event.point,
                event.severity,
                audio_id,
                spatial,
                &mut live,
            );
        }
    }
    // The replicated stream — remote copies' impacts delivered by the
    // v10 snap tail (`apply_snapshots` already resolved, sanitized and
    // deduped each row, and the receiver's own seat never appears).
    // Always a world emitter at the contact; `audio_id` is the v12
    // wire field — the struck side's authored selector resolved on the
    // authority.
    for impact in remote_reader.read() {
        voice_at(
            impact.entity,
            impact.point,
            impact.severity,
            impact.audio_id,
            true,
            &mut live,
        );
    }
}

/// Committed drivetrain change → authored clutch one-shot (F07-B.5,
/// spec req 2's shift/reverse leg). Every `VehicleAudio` car carries a
/// [`GearWatch`] of the last observed `(gear, direction)`; a differing
/// pair plays the cardata `clutch wave name` at `clutch volume` —
/// retail's shift/reverse clunk (`REVERSE` on the cars,
/// `TRUCKGEARSHIFT` on the trucks). Which exact transitions the
/// original voices is unverified (UNK-25); the designed trigger is one
/// one-shot per committed change — a multi-gear jump the selector
/// takes in one step is a single clutch actuation, first sight is not
/// a shift, and the watch updates while not `Playing` so a buffered
/// change never flushes as a stale clunk (the `impact_voices` drain
/// contract applied to state rather than a message stream).
///
/// A remote participant's clutch belongs to its own client — watched
/// but never voiced, the same skip every F05/F07 consumer applies. A
/// sentinel or empty clutch name is authored silence, not a failure;
/// a name that resolves no wave counts `failed` per change (the same
/// per-event semantics horn presses and impact picks apply — a mod's
/// broken binding warns on each shift rather than once at load).
/// Voices are `PlaybackMode::Despawn` children of the car — they ride
/// its transform for the clip and sweep with the session; non-local
/// cars emit spatially under [`ENGINE_SPATIAL_SCALE`], the local
/// player's stays non-spatial (DSN-37). Live voices bound at
/// [`MAX_CLUTCH_VOICES`].
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system — the borrows are the contract.
pub fn clutch_voices(
    mut commands: Commands,
    session: Res<Session>,
    vfs: Option<Res<Mm2Vfs>>,
    mut bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    mut cars: Query<(
        Entity,
        &VehicleAudio,
        &VehicleState,
        Option<&Player>,
        Option<&mut GearWatch>,
    )>,
    voices: Query<&AudioVoice>,
) {
    let generation = session.generation();
    let playing = session.is_playing();
    let mut live = voices
        .iter()
        .filter(|v| v.kind == VoiceKind::Clutch)
        .count();
    for (car, audio, state, player, watch) in &mut cars {
        let Some(mut w) = watch else {
            commands.entity(car).insert(GearWatch {
                gear: state.gear,
                direction: state.direction,
            });
            continue;
        };
        if w.gear == state.gear && w.direction == state.direction {
            continue;
        }
        w.gear = state.gear;
        w.direction = state.direction;
        if !playing
            || player.is_some_and(|p| p.control == PlayerControl::Remote)
            || cardata::is_sample_sentinel(&audio.spec.clutch.name)
        {
            continue;
        }
        let (Some(vfs), Some(bank)) = (vfs.as_deref(), bank.as_deref_mut()) else {
            // No mounted content or no session bank: the shift was
            // observed, nothing can resolve — degrades to silence like
            // an absent authored record.
            continue;
        };
        if live >= MAX_CLUTCH_VOICES {
            report.dropped += 1;
            continue;
        }
        match bank.load(&vfs.0, &mut waves, &audio.spec.clutch.name) {
            Ok(handle) => {
                let local = player.is_some_and(|p| p.control == PlayerControl::Local);
                commands.spawn((
                    AudioVoice {
                        kind: VoiceKind::Clutch,
                    },
                    SessionEntity(generation),
                    ChildOf(car),
                    Transform::default(),
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: PlaybackMode::Despawn,
                        volume: Volume::Linear(authored_volume(audio.spec.clutch.volume)),
                        spatial: !local,
                        spatial_scale: (!local).then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                        ..Default::default()
                    },
                ));
                report.voices += 1;
                report.clutch += 1;
                live += 1;
            }
            Err(e) => {
                report.failed += 1;
                warn!("audio: {e}");
            }
        }
    }
}

/// Build engine rigs on every `VehicleAudio` car: one
/// [`PlaybackMode::Loop`] voice per `Engine wave name` row that
/// resolves both a fade-window spec and a wave, parented to the car so
/// it despawns with it and rides its transform as the emitter.
/// Non-player rigs (race opponents — ambient traffic carries no
/// `VehicleAudio`) are spatial emitters heard through the camera
/// listener; the player's own rig stays non-spatial because the local
/// car anchors the mix (DSN-37). A row that cannot drive (no
/// fade-window schema) or cannot decode is counted and warned, never
/// silently skipped; cars past [`MAX_ENGINE_RIGS`] report once and
/// stay silent rather than flooding the mixer.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system signature — the filtered query is the system's real input
pub fn engine_rigs(
    mut commands: Commands,
    session: Res<Session>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    cars: Query<(Entity, &VehicleAudio, Has<PlayerVehicle>), Without<EngineRig>>,
    rigs: Query<(), With<EngineRig>>,
) {
    if cars.is_empty() {
        return;
    }
    let (Some(vfs), Some(mut bank)) = (vfs, bank) else {
        // No mounted content or no session world yet — retry next
        // frame rather than stamping a half-built rig.
        return;
    };
    let mut live_rigs = rigs.iter().count();
    for (car, audio, player) in &cars {
        if live_rigs >= MAX_ENGINE_RIGS {
            report.dropped += 1;
            warn!("audio: engine rig bound reached, {car:?} stays silent");
            commands.entity(car).insert(EngineRig);
            continue;
        }
        let mut spawned = 0usize;
        for (i, row) in audio.spec.engine_samples.iter().enumerate() {
            if spawned >= MAX_ENGINE_VOICES {
                report.dropped += 1;
                warn!("audio: engine rig full, dropping row {:?}", row.name);
                continue;
            }
            let Some(spec) = EngineLoopSpec::from_row(row) else {
                report.failed += 1;
                warn!(
                    "audio: engine sample {:?} has no fade-window schema — skipped",
                    row.name
                );
                continue;
            };
            match bank.load(&vfs.0, &mut waves, &row.name) {
                Ok(handle) => {
                    commands.spawn((
                        AudioVoice {
                            kind: VoiceKind::Engine,
                        },
                        EngineVoice {
                            row: i,
                            spec,
                            mix: EngineMix {
                                volume: 0.0,
                                speed: 1.0,
                            },
                        },
                        SessionEntity(session.generation()),
                        ChildOf(car),
                        Transform::default(),
                        AudioPlayer(handle),
                        PlaybackSettings {
                            mode: PlaybackMode::Loop,
                            // Silent until `engine_drive` computes the
                            // real mix — a sink attaching between spawn
                            // and the first drive tick plays nothing.
                            volume: Volume::Linear(0.0),
                            spatial: !player,
                            spatial_scale: (!player)
                                .then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                            ..Default::default()
                        },
                    ));
                    spawned += 1;
                    report.voices += 1;
                    report.loops += 1;
                }
                Err(e) => {
                    report.failed += 1;
                    warn!("audio: {e}");
                }
            }
        }
        commands.entity(car).insert(EngineRig);
        live_rigs += 1;
        report.rigs += 1;
    }
}

/// Re-mix every engine loop from its parent vehicle's RPM and apply it
/// to whichever sink the device attached (plain for the player rig,
/// [`SpatialAudioSink`] for opponent emitters). Ungated by phase:
/// voices only exist inside a live session, `Paused` sinks are held by
/// [`sync_audio_pause`], and `Countdown`/`Results` legitimately keep
/// the engines sounding — the cars are live, just not drivable.
pub fn engine_drive(
    mut report: ResMut<AudioReport>,
    cars: Query<&VehicleState>,
    mut voices: Query<(
        &ChildOf,
        &mut EngineVoice,
        Option<&mut AudioSink>,
        Option<&mut SpatialAudioSink>,
    )>,
) {
    report.audible = 0;
    for (parent, mut voice, sink, spatial) in &mut voices {
        let Ok(state) = cars.get(parent.parent()) else {
            // The car despawned and the cascade has not flushed — the
            // voice dies with it; leave the last computed mix.
            continue;
        };
        voice.mix = voice.spec.mix(state.rpm);
        if voice.mix.volume > 0.0 {
            report.audible += 1;
        }
        if let Some(mut sink) = sink {
            sink.set_volume(Volume::Linear(voice.mix.volume));
            sink.set_speed(voice.mix.speed);
        }
        if let Some(mut sink) = spatial {
            sink.set_volume(Volume::Linear(voice.mix.volume));
            sink.set_speed(voice.mix.speed);
        }
    }
}

/// Build ambient engine voices on every `AmbientAudio` car: one
/// [`PlaybackMode::Loop`] voice resolving the table's authored sample,
/// parented to the car so it despawns with it and rides its transform
/// as the spatial emitter — ambient cars are never the local listener
/// anchor, so every one is a world emitter under
/// [`ENGINE_SPATIAL_SCALE`] (F07-B.6; DSN-37's anchor rule does not
/// apply to traffic). Live ambient loops bound at
/// [`MAX_AMBIENT_VOICES`]: a car past it counts one drop and keeps
/// its [`AmbientRig`] marker so it never re-warns, while a recycled
/// body re-attempts. A failed resolve/decode counts once per car
/// (the marker prevents a per-frame warn); a missing `AmbientAudio`
/// is authored silence upstream and never reaches this system.
///
/// Retries while no VFS/bank exists like `engine_rigs` — a half-built
/// rig is never stamped.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn ambient_engine_rigs(
    mut commands: Commands,
    session: Res<Session>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    cars: Query<(Entity, &AmbientAudio), Without<AmbientRig>>,
    voices: Query<&AudioVoice>,
) {
    if cars.is_empty() {
        return;
    }
    let (Some(vfs), Some(mut bank)) = (vfs, bank) else {
        // No mounted content or no session world yet — retry next
        // frame rather than stamping a half-built rig.
        return;
    };
    let mut live = voices
        .iter()
        .filter(|v| v.kind == VoiceKind::AmbientEngine)
        .count();
    for (car, audio) in &cars {
        if live >= MAX_AMBIENT_VOICES {
            report.dropped += 1;
            warn!("audio: ambient engine bound reached, {car:?} stays silent");
            commands.entity(car).insert(AmbientRig);
            continue;
        }
        match bank.load(&vfs.0, &mut waves, &audio.spec.name) {
            Ok(handle) => {
                commands.spawn((
                    AudioVoice {
                        kind: VoiceKind::AmbientEngine,
                    },
                    AmbientEngineVoice {
                        spec: audio.spec.clone(),
                        mix: EngineMix {
                            volume: 0.0,
                            speed: 1.0,
                        },
                    },
                    SessionEntity(session.generation()),
                    ChildOf(car),
                    Transform::default(),
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: PlaybackMode::Loop,
                        // Silent until `ambient_engine_drive` computes
                        // the real mix — a sink attaching between
                        // spawn and the first drive tick plays nothing.
                        volume: Volume::Linear(0.0),
                        spatial: true,
                        spatial_scale: Some(SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                        ..Default::default()
                    },
                ));
                report.voices += 1;
                report.ambient += 1;
                live += 1;
            }
            Err(e) => {
                report.failed += 1;
                warn!("audio: {e}");
            }
        }
        commands.entity(car).insert(AmbientRig);
    }
}

/// Re-mix every ambient engine loop from its parent car's
/// `LinearVelocity` magnitude and apply it to whichever sink the
/// device attached (F07-B.6). Lane followers publish their surface
/// velocity through the same component knocked wrecks do, so the
/// drive needs no drive-state branch — a wreck sliding to a stop
/// winds its note down on its own. Ungated by phase like
/// `engine_drive`: voices only exist inside a live session,
/// `sync_audio_pause` holds the sinks, and countdown ambience keeps
/// the street sounding.
pub fn ambient_engine_drive(
    mut report: ResMut<AudioReport>,
    cars: Query<&LinearVelocity>,
    mut voices: Query<(
        &ChildOf,
        &mut AmbientEngineVoice,
        Option<&mut AudioSink>,
        Option<&mut SpatialAudioSink>,
    )>,
) {
    report.ambient_live = 0;
    for (parent, mut voice, sink, spatial) in &mut voices {
        let Ok(vel) = cars.get(parent.parent()) else {
            // The car despawned and the cascade has not flushed — the
            // voice dies with it; leave the last computed mix.
            continue;
        };
        voice.mix = voice.spec.mix(vel.length());
        if voice.mix.volume > 0.0 {
            report.ambient_live += 1;
        }
        push_mix(voice.mix, sink, spatial);
    }
}

/// Precipitation ambience (F18-B.3, spec req 3's weather→audio leg):
/// the session's [`WeatherAudio`] lazily resolves the authored
/// `<name>exterior`/`<name>interior` bed stems and `thunder` through
/// the [`WaveBank`], then drives a coverage crossfade and the seeded
/// clap schedule.
///
/// - **Beds**: one `PlaybackMode::Loop` non-spatial voice per resolved
///   bed stem — an ambient bed anchored on the listener like the local
///   car's own rig (DSN-37) — spawned `SessionEntity`-stamped so
///   teardown sweeps them. A stem that resolves nothing counts one
///   `failed` (never retried, never substituted — F18-AC06).
/// - **Coverage**: the listener rides the active `Camera3d`
///   ([`audio_listener`]), so the same upward probe the particle
///   emitter declares (`crate::precip::COVER_PROBE` — a designed
///   approximation of the original's interior pick, UNK-25/UNK-40)
///   reads shelter off the camera's position: covered pulls
///   `interior_mix` toward 1, exposed toward 0, at
///   [`RAIN_CROSSFADE_PER_SEC`]. The bed volumes are the crossfaded
///   authored-constant reads [`RAIN_EXTERIOR_VOLUME`]/
///   [`RAIN_INTERIOR_VOLUME`]. An unresolved half simply stays silent.
/// - **Thunder**: while `Playing`, `elapsed` accumulates frame time;
///   crossing `next_thunder` spawns a bounded
///   (`MAX_THUNDER_VOICES`) `PlaybackMode::Despawn` one-shot and
///   redraws the delay from the session-seeded stream — a replayed
///   session repeats the same schedule. Non-spatial like the beds.
///   The original's trigger (a schedule at all, and its bounds) is
///   unrecovered — the delays adopt the `13.0`/`15.0` constants the
///   exe stores beside the stems (designed, see `WeatherAudio`).
///
/// Ungated by phase like `engine_drive` — voices only exist inside a
/// live session and `sync_audio_pause` holds the sinks; the clap
/// schedule alone gates on `is_playing` so a pause/countdown banks no
/// elapsed time.
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn weather_voices(
    mut commands: Commands,
    session: Res<Session>,
    time: Res<Time>,
    weather: Option<ResMut<WeatherAudio>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    spatial: Option<SpatialQuery>,
    cameras: Query<(&Camera, &GlobalTransform), crate::hudmap::WorldCamera3d>,
    mut beds: Query<(&mut WeatherVoice, Option<&mut AudioSink>)>,
    voices: Query<&AudioVoice>,
) {
    let (Some(mut weather), Some(vfs), Some(mut bank)) = (weather, vfs, bank) else {
        return;
    };
    // `ResMut` derefs borrow the whole resource — reborrow so the two
    // `tried` flags can be held disjointly through the loop below.
    let weather = &mut *weather;
    let generation = session.generation();
    // The lazy bed build — the ambient-rig pattern without a host car:
    // each stem resolves once, spawning its loop at volume 0 until the
    // mix below computes the real level.
    for (role, stem, tried) in [
        (
            WeatherRole::Exterior,
            weather_stem(weather.name, "exterior"),
            &mut weather.exterior_tried,
        ),
        (
            WeatherRole::Interior,
            weather_stem(weather.name, "interior"),
            &mut weather.interior_tried,
        ),
    ] {
        if *tried {
            continue;
        }
        *tried = true;
        match bank.load(&vfs.0, &mut waves, &stem) {
            Ok(handle) => {
                commands.spawn((
                    AudioVoice {
                        kind: VoiceKind::Weather,
                    },
                    WeatherVoice {
                        role,
                        mix: EngineMix {
                            volume: 0.0,
                            speed: 1.0,
                        },
                    },
                    SessionEntity(generation),
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: PlaybackMode::Loop,
                        volume: Volume::Linear(0.0),
                        ..Default::default()
                    },
                ));
                report.voices += 1;
                report.weather += 1;
            }
            Err(e) => {
                report.failed += 1;
                warn!("audio: {e}");
            }
        }
    }
    // The thunder clip resolves once the same way; a missing stem
    // counts and never retries.
    if !weather.thunder_tried {
        weather.thunder_tried = true;
        match bank.load(&vfs.0, &mut waves, "thunder") {
            Ok(handle) => weather.thunder = Some(handle),
            Err(e) => {
                report.failed += 1;
                warn!("audio: {e}");
            }
        }
    }
    // Coverage — the listener's camera decides shelter. No active
    // world camera or no physics yet keeps the last mix rather than
    // snapping to exposed.
    if let Some((sq, focus)) = spatial.as_ref().and_then(|sq| {
        cameras
            .iter()
            .find(|(cam, _)| cam.is_active)
            .map(|(_, xf)| (sq, xf.translation()))
            .filter(|(_, p)| p.is_finite())
    }) {
        let covered = sq
            .cast_ray(focus, Dir3::Y, crate::precip::COVER_PROBE, true, &default())
            .is_some();
        report.interior = covered;
        let target = if covered { 1.0 } else { 0.0 };
        let step = RAIN_CROSSFADE_PER_SEC * time.delta_secs();
        weather.interior_mix += (target - weather.interior_mix).clamp(-step, step);
    }
    let mix = weather.interior_mix;
    for (mut bed, sink) in &mut beds {
        let level = match bed.role {
            WeatherRole::Exterior => (1.0 - mix) * RAIN_EXTERIOR_VOLUME,
            WeatherRole::Interior => mix * RAIN_INTERIOR_VOLUME,
        };
        bed.mix = EngineMix {
            volume: level,
            speed: 1.0,
        };
        push_mix(bed.mix, sink, None);
    }
    // The clap schedule — seeded delays, bounded concurrent voices.
    if session.is_playing() {
        weather.elapsed += time.delta_secs();
        if weather.elapsed >= weather.next_thunder
            && let Some(handle) = weather.thunder.clone()
        {
            if voices
                .iter()
                .filter(|v| v.kind == VoiceKind::Thunder)
                .count()
                >= MAX_THUNDER_VOICES
            {
                report.dropped += 1;
            } else {
                commands.spawn((
                    AudioVoice {
                        kind: VoiceKind::Thunder,
                    },
                    SessionEntity(generation),
                    AudioPlayer(handle),
                    PlaybackSettings {
                        mode: PlaybackMode::Despawn,
                        volume: Volume::Linear(THUNDER_VOLUME),
                        ..Default::default()
                    },
                ));
                report.voices += 1;
                report.thunder += 1;
            }
            weather.next_thunder = weather.elapsed + WeatherAudio::draw_delay(&mut weather.rng);
        }
    }
}

/// Environmental pre-race commentary (F18-B.5): the session's
/// [`CommentaryAudio`] resolves its registry → speaker → cue → wave
/// chain once inside the pre-race window, then plays the queued
/// clips as `PlaybackMode::Despawn` one-shots — `SessionEntity`-
/// stamped so teardown sweeps any still-playing voice.
///
/// The resolve runs on the first `Countdown`/`Playing` frame —
/// commentary is a pre-race binding, so the clock only ticks inside
/// that window; a session that never reaches it resolves nothing.
/// Order: the `aud/spchdata/<city>.csv` registry authors `Num
/// announcers` + `prefix`, one seeded draw picks the speaker dir
/// (`as1`…), each bound `<stem>_prerace.csv` yields its `WEATHER`/
/// `TIMEOFDAY` section's first row, and a second seeded draw picks
/// the wave suffix inside the authored `1..=end` range — two clips
/// at most, weather before time-of-day (the exe's cue order).
///
/// Every resolve step that fails — missing registry, a speaker dir
/// that ships no tables (SF's authored `as3` gap), a missing
/// section, a missing or undecodable wave — counts one `failed` and
/// plays nothing: no substitute cue, no re-draw, no invented line
/// (F18-AC06). Non-spatial one-shots like the other listener-side
/// voices (DSN-37); the sequencing gap between clips is designed —
/// the original's cadence and its handling of the `as3` gap are
/// unrecovered (UNK-25).
#[allow(clippy::too_many_arguments)] // Bevy system — the borrows are the contract.
pub fn commentary_voices(
    mut commands: Commands,
    session: Res<Session>,
    time: Res<Time>,
    commentary: Option<ResMut<CommentaryAudio>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
) {
    let (Some(mut commentary), Some(vfs), Some(mut bank)) = (commentary, vfs, bank) else {
        return;
    };
    let commentary = &mut *commentary;
    if !matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing
    ) {
        return;
    }
    commentary.elapsed += time.delta_secs();
    if !commentary.resolved {
        commentary.resolved = true;
        resolve_commentary(commentary, &vfs.0, &mut bank, &mut waves, &mut report);
    }
    // Sequencing — the first cue fires immediately; each later one
    // waits out the previous clip's duration plus the gap.
    if commentary.elapsed >= commentary.next_at
        && let Some((stem, handle)) = commentary.queue.pop_front()
    {
        let clip_secs = waves.get(&handle).map(|w| w.duration()).unwrap_or(0.0);
        commands.spawn((
            AudioVoice {
                kind: VoiceKind::Commentary,
            },
            CommentaryVoice { stem },
            SessionEntity(session.generation()),
            AudioPlayer(handle),
            PlaybackSettings {
                mode: PlaybackMode::Despawn,
                volume: Volume::Linear(COMMENTARY_VOLUME),
                ..Default::default()
            },
        ));
        report.voices += 1;
        report.commentary += 1;
        commentary.next_at = commentary.elapsed + clip_secs + COMMENTARY_GAP;
    }
}

/// The one-shot registry→speaker→cue→wave resolve
/// [`commentary_voices`] performs on its first live frame. Each
/// failure warns and counts `failed` once; a successful wave enters
/// the queue in authored cue order.
fn resolve_commentary(
    commentary: &mut CommentaryAudio,
    vfs: &Vfs,
    bank: &mut WaveBank,
    waves: &mut Assets<PcmAudio>,
    report: &mut AudioReport,
) {
    let fail = |report: &mut AudioReport, msg: String| {
        report.failed += 1;
        warn!("audio: {msg}");
    };
    // Registry — `aud/spchdata/<city>.csv` authors the speaker count
    // and the `AS`/`AL` dir prefix.
    let index = vfs
        .read_logical(&commentary.registry)
        .map_err(|e| format!("{}: {e}", commentary.registry))
        .and_then(|bytes| {
            AnnouncerIndex::parse(&String::from_utf8_lossy(&bytes))
                .map_err(|e| format!("{}: {e}", commentary.registry))
        });
    let index = match index {
        Ok(index) => index,
        Err(e) => {
            fail(report, e);
            return;
        }
    };
    for d in &index.diagnostics {
        warn!("audio: {}:{}: {}", commentary.registry, d.line, d.message);
    }
    let Some(d) = draw_speaker(index.announcers, &mut commentary.rng) else {
        fail(
            report,
            format!(
                "{} authors {} announcers — no speaker",
                commentary.registry, index.announcers
            ),
        );
        return;
    };
    let speaker = format!("{}{d}", index.prefix.to_lowercase());
    for (section_name, stem) in commentary.cues {
        let path = format!("aud/spchdata/{speaker}/{stem}_prerace.csv");
        let bytes = match vfs.read_logical(&path) {
            Ok(b) => b,
            Err(e) => {
                fail(report, format!("{path}: {e}"));
                continue;
            }
        };
        let table = match CueTable::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(t) => t,
            Err(e) => {
                fail(report, format!("{path}: {e}"));
                continue;
            }
        };
        for d in &table.diagnostics {
            warn!("audio: {path}:{}: {}", d.line, d.message);
        }
        let Some(row) = table.section(section_name).and_then(|s| s.rows.first()) else {
            fail(report, format!("{path} authors no {section_name} cue"));
            continue;
        };
        let Some(suffix) = draw_cue_suffix(row.end, row.add, &mut commentary.rng) else {
            fail(
                report,
                format!(
                    "{path} cue {} has an undrawable range (end {} add {})",
                    row.prefix, row.end, row.add
                ),
            );
            continue;
        };
        let wave = cue_wave_stem(&speaker, &row.prefix, suffix);
        match bank.load(vfs, waves, &wave) {
            Ok(handle) => commentary.queue.push_back((wave, handle)),
            Err(e) => fail(report, format!("{path} cue {wave}: {e}")),
        }
    }
}

/// Fresh band slots for a committed skid entry — capped at
/// [`MAX_SKID_VOICES`], the overflow counted once per commit rather
/// than re-dropped every frame.
fn skid_slots(audio: &SurfaceAudio, idx: u16, report: &mut AudioReport) -> Vec<SkidSlot> {
    let bands = audio
        .table
        .surfaces
        .get(idx as usize)
        .map(|e| e.skids.len())
        .unwrap_or(0);
    let overflow = bands.saturating_sub(MAX_SKID_VOICES);
    if overflow > 0 {
        report.dropped += overflow as u64;
    }
    (0..bands.min(MAX_SKID_VOICES))
        .map(|_| SkidSlot::default())
        .collect()
}

/// Push a computed mix onto whichever sink the device attached — a
/// plain sink for the player's own rig, [`SpatialAudioSink`] for
/// everyone else's (the apply half of `engine_drive`'s contract).
fn push_mix(mix: EngineMix, sink: Option<Mut<AudioSink>>, spatial: Option<Mut<SpatialAudioSink>>) {
    if let Some(mut s) = sink {
        s.set_volume(Volume::Linear(mix.volume));
        s.set_speed(mix.speed);
    }
    if let Some(mut s) = spatial {
        s.set_volume(Volume::Linear(mix.volume));
        s.set_speed(mix.speed);
    }
}

/// A `contact_pick`/`contact_wins` result: the loudest covering skid
/// pick `(sound class, band, gain)` and rolling pick `(class, gain)`
/// across the car — `None` on either half when nothing covers.
#[derive(Default)]
struct SurfaceWins {
    skid: Option<(u16, usize, f32)>,
    roll: Option<(u16, f32)>,
}

/// [`surface_voices`]' per-car wheel loop distilled to a value: the
/// [`SurfaceWins`] plus the [`SurfaceContact`] truth that produced them
/// — the winning wheel's class/slippage/`vel_long`, the fields protocol
/// v16's `SnapEntry` tail carries so a remote copy can replay the same
/// pick through its own table.
fn contact_pick(
    vehicle: &Vehicle,
    state: &VehicleState,
    tables: &SurfaceTables,
    audio: &SurfaceAudio,
    collider_surfaces: &Query<&SurfaceMaterial>,
) -> (SurfaceWins, SurfaceContact) {
    // The loudest covering skid band and rolling loop across the
    // grounded wheels — `(sound index, band, gain)` / `(index, _)`.
    let mut skid_win: Option<(u16, usize, f32)> = None;
    let mut skid_contact: Option<SkidContact> = None;
    let mut roll_win: Option<(u16, f32)> = None;
    let moving = state.forward_speed.abs() > ROLL_MIN_SPEED;
    for (i, w) in state.wheels.iter().enumerate() {
        if !w.grounded {
            continue;
        }
        let material = w
            .contact_entity
            .and_then(|e| collider_surfaces.get(e).ok())
            .copied()
            .unwrap_or_default();
        let Some(idx) = tables.sound_index(material) else {
            continue;
        };
        let (Some(spec), Some(entry)) = (
            audio.specs.get(idx as usize),
            audio.table.surfaces.get(idx as usize),
        ) else {
            continue;
        };
        if let Some(skid) = spec.skid {
            let tires = vehicle
                .config
                .wheels
                .get(i)
                .and_then(|c| c.tires.as_ref())
                .unwrap_or(&vehicle.config.tires);
            let slippage = tire_slippage(
                w.traction_demand,
                w.slip_angle,
                tires.peak_slip_angle,
                tires.peak_slip_ratio,
            );
            let q = match skid.unit {
                SkidUnit::Slippage => slippage,
                SkidUnit::Speed => w.vel_long.abs(),
            };
            if let Some(pick) = skid.pick(&entry.skids[..entry.skids.len().min(MAX_SKID_VOICES)], q)
                && skid_win.is_none_or(|(.., g)| pick.volume > g)
            {
                skid_win = Some((idx, pick.band, pick.volume));
                skid_contact = Some(SkidContact {
                    surface: idx,
                    slippage,
                    wheel_speed: w.vel_long,
                });
            }
        }
        if moving && let Some(rolling) = spec.rolling {
            let gain = rolling.mix(state.forward_speed).volume;
            if roll_win.is_none_or(|(_, g)| gain > g) {
                roll_win = Some((idx, gain));
            }
        }
    }
    (
        SurfaceWins {
            skid: skid_win,
            roll: roll_win,
        },
        SurfaceContact {
            skid: skid_contact,
            roll: roll_win.map(|(idx, _)| idx),
        },
    )
}

/// The receiving half of the v16 surface tail: replay a replicated
/// [`SurfaceContact`] through this process's own [`SurfaceAudio`].
/// `aud/` rides no gameplay fingerprint — a peer may carry a modded
/// table — so each process re-runs `SkidSpec::pick` under its spec's
/// unit and mixes `RollingSpec` off the copy's forward speed rather
/// than trusting the authority's band or gain. A class the local table
/// cannot answer — out of range or spec-less — resolves to silence
/// like an unmapped material.
fn contact_wins(audio: &SurfaceAudio, contact: &SurfaceContact, forward_speed: f32) -> SurfaceWins {
    let skid_win = contact.skid.and_then(|sk| {
        let (Some(spec), Some(entry)) = (
            audio.specs.get(sk.surface as usize),
            audio.table.surfaces.get(sk.surface as usize),
        ) else {
            return None;
        };
        let skid = spec.skid?;
        let q = match skid.unit {
            SkidUnit::Slippage => sk.slippage,
            SkidUnit::Speed => sk.wheel_speed.abs(),
        };
        skid.pick(&entry.skids[..entry.skids.len().min(MAX_SKID_VOICES)], q)
            .map(|pick| (sk.surface, pick.band, pick.volume))
    });
    let roll_win = if forward_speed.abs() > ROLL_MIN_SPEED {
        contact.roll.and_then(|idx| {
            let spec = audio.specs.get(idx as usize)?;
            audio.table.surfaces.get(idx as usize)?;
            spec.rolling.map(|r| (idx, r.mix(forward_speed).volume))
        })
    } else {
        None
    };
    SurfaceWins {
        skid: skid_win,
        roll: roll_win,
    }
}

/// Wheel contact → surface voices (F07-B.4, spec req 3). Every
/// `Vehicle` car resolves its grounded wheels against the session's
/// [`SurfaceTables`] — `SurfaceMaterial` → the material's authored
/// `sound` class → the [`SurfaceAudio`] row — and voices the loudest
/// result per role:
///
/// - **Skid**: per grounded wheel the designed [`tire_slippage`]
///   quantity (UNK-25 — tire-limit utilization saturating at one)
///   picks a `skid wave` band of that wheel's entry; `min speed`
///   units read `|vel_long|` instead. The loudest covering band wins.
///   A held brake at rest demands nothing (its force is
///   `vel_long`-scaled) and an airborne wheel has no contact — the
///   F07-AC03 legs stay silent by construction, not by a gate.
/// - **Rolling**: the loudest `surface wave` loop under a moving car
///   (`|forward_speed|` ≥ [`ROLL_MIN_SPEED`], so a parked car on grass
///   does not hum), mixed by the authored `max speed` window.
///
/// Band loops spawn lazily per committed entry and idle at volume 0
/// rather than churning attach/detach (the engine rig's policy); a
/// different winning entry rebuilds only after holding
/// [`SURFACE_DWELL`] frames. Non-player cars are spatial emitters at
/// the car's transform under [`ENGINE_SPATIAL_SCALE`]; the local
/// player's stay non-spatial (DSN-37). Cars past [`MAX_SURFACE_RIGS`]
/// report once and stay silent. Ungated by phase like `engine_drive`
/// — voices only exist inside a live session and `sync_audio_pause`
/// holds the sinks.
///
/// The resolved contact also lands on the car's [`SurfaceContact`]
/// (F25-B protocol v16) — `netdrive::publish_snapshots` encodes a wire
/// seat's `SnapEntry` tail from it. A `RemoteReplica` copy runs the
/// inverse arm instead: no wheel loop (a kinematic replica has no
/// contact truth), just [`contact_wins`] replaying the replicated
/// contact through this process's own table.
#[allow(clippy::too_many_arguments, clippy::type_complexity)] // Bevy system — the borrows are the contract.
pub fn surface_voices(
    mut commands: Commands,
    session: Res<Session>,
    surface_audio: Option<Res<SurfaceAudio>>,
    tables: Option<Res<SurfaceTables>>,
    vfs: Option<Res<Mm2Vfs>>,
    bank: Option<ResMut<WaveBank>>,
    mut waves: ResMut<Assets<PcmAudio>>,
    mut report: ResMut<AudioReport>,
    mut cars: Query<(
        Entity,
        &Vehicle,
        &VehicleState,
        Option<&mut SurfaceRig>,
        Has<PlayerVehicle>,
        Has<RemoteReplica>,
        Option<&mut SurfaceContact>,
    )>,
    collider_surfaces: Query<&SurfaceMaterial>,
    mut voices: Query<(
        &mut SurfaceVoice,
        Option<&mut AudioSink>,
        Option<&mut SpatialAudioSink>,
    )>,
    rigs: Query<(), With<SurfaceRig>>,
) {
    report.skids = 0;
    report.rolling = 0;
    let (Some(audio), Some(tables), Some(vfs), Some(mut bank)) = (surface_audio, tables, vfs, bank)
    else {
        // No authored surface data (or no session world yet) — a wheel
        // cannot resolve a surface row, so nothing plays.
        return;
    };
    let generation = session.generation();
    let mut live_rigs = rigs.iter().count();
    for (car, vehicle, state, rig_slot, player, remote, mut contact) in &mut cars {
        // The loudest covering skid band and rolling loop —
        // `(sound index, band, gain)` / `(index, _)`. A simulated car
        // resolves its wheels ([`contact_pick`]) and publishes the
        // winning contact on its `SurfaceContact`; a `RemoteReplica`
        // copy replays the replicated one ([`contact_wins`]).
        let wins = if remote {
            contact_wins(
                &audio,
                &contact.as_deref().copied().unwrap_or_default(),
                state.forward_speed,
            )
        } else {
            let (wins, resolved) =
                contact_pick(vehicle, state, &tables, &audio, &collider_surfaces);
            match contact.as_deref_mut() {
                Some(slot) => *slot = resolved,
                None => {
                    commands.entity(car).insert(resolved);
                }
            }
            wins
        };
        let (skid_win, roll_win) = (wins.skid, wins.roll);
        if skid_win.is_none() && roll_win.is_none() && rig_slot.is_none() {
            // Never resolved a surface — no rig to silence either.
            continue;
        }
        let Some(mut rig) = rig_slot else {
            // First resolution — bound the session's rig count. A
            // muted marker keeps the refusal counted once.
            if live_rigs >= MAX_SURFACE_RIGS {
                report.dropped += 1;
                warn!("audio: surface rig bound reached, {car:?} stays silent");
                commands.entity(car).insert(SurfaceRig {
                    muted: true,
                    ..Default::default()
                });
            } else {
                live_rigs += 1;
                commands.entity(car).insert(SurfaceRig::default());
            }
            continue;
        };
        if rig.muted {
            continue;
        }

        // -- skid half: commit the winning entry (dwell-gated swap),
        //    lazily build the winning band's voice, mix every slot.
        match (rig.skid_entry, skid_win) {
            (_, None) => {}
            (None, Some((idx, ..))) => {
                rig.skid_entry = Some(idx);
                rig.skid_slots = skid_slots(&audio, idx, &mut report);
                rig.skid_pending = None;
            }
            (Some(c), Some((idx, ..))) if c == idx => rig.skid_pending = None,
            (Some(_), Some((idx, ..))) => {
                match &mut rig.skid_pending {
                    Some((p, n)) if *p == idx => *n += 1,
                    _ => rig.skid_pending = Some((idx, 1)),
                }
                if rig.skid_pending.is_some_and(|(_, n)| n >= SURFACE_DWELL) {
                    for slot in &mut rig.skid_slots {
                        if let Some(v) = slot.voice.take() {
                            commands.entity(v).despawn();
                        }
                    }
                    rig.skid_entry = Some(idx);
                    rig.skid_slots = skid_slots(&audio, idx, &mut report);
                    rig.skid_pending = None;
                }
            }
        }
        let winning = skid_win.filter(|(idx, ..)| Some(*idx) == rig.skid_entry);
        if let Some(idx) = rig.skid_entry {
            if let Some((_, band, ..)) = winning
                && let Some(slot) = rig.skid_slots.get_mut(band)
                && !slot.tried
            {
                slot.tried = true;
                let name = audio.table.surfaces[idx as usize].skids[band].name.clone();
                match bank.load(&vfs.0, &mut waves, &name) {
                    Ok(handle) => {
                        let v = commands
                            .spawn((
                                AudioVoice {
                                    kind: VoiceKind::Skid,
                                },
                                SurfaceVoice {
                                    role: SurfaceRole::Skid(band),
                                    mix: EngineMix {
                                        volume: 0.0,
                                        speed: 1.0,
                                    },
                                },
                                SessionEntity(generation),
                                ChildOf(car),
                                Transform::default(),
                                AudioPlayer(handle),
                                PlaybackSettings {
                                    mode: PlaybackMode::Loop,
                                    volume: Volume::Linear(0.0),
                                    spatial: !player,
                                    spatial_scale: (!player)
                                        .then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                                    ..Default::default()
                                },
                            ))
                            .id();
                        slot.voice = Some(v);
                        report.voices += 1;
                        report.skid_voices += 1;
                    }
                    Err(e) => {
                        report.failed += 1;
                        warn!("audio: {e}");
                    }
                }
            }
            for (b, slot) in rig.skid_slots.iter().enumerate() {
                let target = winning
                    .filter(|(_, band, _)| *band == b)
                    .map(|(.., g)| g)
                    .unwrap_or(0.0);
                if let Some(v) = slot.voice
                    && let Ok((mut voice, sink, spatial)) = voices.get_mut(v)
                {
                    voice.mix = EngineMix {
                        volume: target,
                        speed: 1.0,
                    };
                    if target > 0.0 {
                        report.skids += 1;
                    }
                    push_mix(voice.mix, sink, spatial);
                }
            }
        }

        // -- rolling half: the same commit/dwell/lazy-voice shape.
        match (rig.rolling_entry, roll_win) {
            (_, None) => {}
            (None, Some((idx, _))) => {
                rig.rolling_entry = Some(idx);
                rig.rolling_tried = false;
                rig.rolling_pending = None;
            }
            (Some(c), Some((idx, _))) if c == idx => rig.rolling_pending = None,
            (Some(_), Some((idx, _))) => {
                match &mut rig.rolling_pending {
                    Some((p, n)) if *p == idx => *n += 1,
                    _ => rig.rolling_pending = Some((idx, 1)),
                }
                if rig.rolling_pending.is_some_and(|(_, n)| n >= SURFACE_DWELL) {
                    if let Some(v) = rig.rolling_voice.take() {
                        commands.entity(v).despawn();
                    }
                    rig.rolling_entry = Some(idx);
                    rig.rolling_tried = false;
                    rig.rolling_pending = None;
                }
            }
        }
        if let Some(idx) = rig.rolling_entry {
            let playing = roll_win.is_some_and(|(w, _)| w == idx);
            if playing && !rig.rolling_tried {
                rig.rolling_tried = true;
                let name = audio.table.surfaces[idx as usize].name.clone();
                match bank.load(&vfs.0, &mut waves, &name) {
                    Ok(handle) => {
                        let v = commands
                            .spawn((
                                AudioVoice {
                                    kind: VoiceKind::Rolling,
                                },
                                SurfaceVoice {
                                    role: SurfaceRole::Rolling,
                                    mix: EngineMix {
                                        volume: 0.0,
                                        speed: 1.0,
                                    },
                                },
                                SessionEntity(generation),
                                ChildOf(car),
                                Transform::default(),
                                AudioPlayer(handle),
                                PlaybackSettings {
                                    mode: PlaybackMode::Loop,
                                    volume: Volume::Linear(0.0),
                                    spatial: !player,
                                    spatial_scale: (!player)
                                        .then(|| SpatialScale::new(ENGINE_SPATIAL_SCALE)),
                                    ..Default::default()
                                },
                            ))
                            .id();
                        rig.rolling_voice = Some(v);
                        report.voices += 1;
                        report.rolling_voices += 1;
                    }
                    Err(e) => {
                        report.failed += 1;
                        warn!("audio: {e}");
                    }
                }
            }
            if let Some(v) = rig.rolling_voice
                && let Ok((mut voice, sink, spatial)) = voices.get_mut(v)
            {
                voice.mix = if playing {
                    audio.specs[idx as usize].rolling.map_or(
                        EngineMix {
                            volume: 0.0,
                            speed: 1.0,
                        },
                        |r| r.mix(state.forward_speed),
                    )
                } else {
                    EngineMix {
                        volume: 0.0,
                        speed: 1.0,
                    }
                };
                if voice.mix.volume > 0.0 {
                    report.rolling += 1;
                }
                push_mix(voice.mix, sink, spatial);
            }
        }
    }
}
/// 5): `toggle_camera` flips `Camera::is_active` between the session's
/// chase and free cameras, and Bevy wants exactly one
/// [`SpatialListener`] — this inserts it on the active camera and
/// strips it from the inactive one, so a mode switch moves the ear
/// with the view rather than doubling listeners or leaving it on a
/// camera the session is about to despawn. `Camera3d`-filtered so a
/// menu `Camera2d` surviving a transition frame can't become the ear
/// (the same guard `apply_city_pvs` applies to source rooms). With no
/// active 3-D camera the field simply has no listener.
pub fn audio_listener(
    mut commands: Commands,
    // The HUD map camera is an active `Camera3d` while a view is up —
    // it's a second render pass, not an ear (F22-A.1).
    cams: Query<(Entity, &Camera, Has<SpatialListener>), crate::hudmap::WorldCamera3d>,
) {
    for (entity, cam, listening) in &cams {
        match (cam.is_active, listening) {
            (true, false) => {
                commands.entity(entity).insert(SpatialListener::default());
            }
            (false, true) => {
                commands.entity(entity).remove::<SpatialListener>();
            }
            _ => {}
        }
    }
}

/// Count voices the output device has attached a sink to — the
/// observable difference between "voice spawned" and "mixer accepted
/// it" (F07-AC05's evidence hook: headless runs stay `0s`, a device
/// run shows the voice sink).
pub fn count_sinks(
    mut report: ResMut<AudioReport>,
    plain: Query<(), (With<AudioVoice>, Added<AudioSink>)>,
    spatial: Query<(), (With<AudioVoice>, Added<SpatialAudioSink>)>,
) {
    report.sunk += (plain.iter().count() + spatial.iter().count()) as u64;
}

/// Pause/resume live voices with the session — voice entities whose
/// sink exists yet follow the phase each update, so a pause between
/// spawn and sink attach still lands paused (F07's pause half; unload
/// is covered by [`SessionEntity`] teardown).
pub fn sync_audio_pause(
    session: Res<Session>,
    voices: Query<(Option<&AudioSink>, Option<&SpatialAudioSink>), With<AudioVoice>>,
) {
    let paused = matches!(session.phase(), SessionPhase::Paused);
    for (sink, spatial) in &voices {
        for s in [
            sink.map(|s| -> &dyn AudioSinkPlayback { s }),
            spatial.map(|s| -> &dyn AudioSinkPlayback { s }),
        ]
        .into_iter()
        .flatten()
        {
            match (paused, s.is_paused()) {
                (true, false) => s.pause(),
                (false, true) => s.play(),
                _ => {}
            }
        }
    }
}
