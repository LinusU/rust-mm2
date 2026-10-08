//! Frame-time recorder (`--perf-log`): where each frame's wall time goes.
//!
//! A stutter report ("Tower Tour hitches in release") needs a number per
//! frame, not an average. This records, for every frame, the whole
//! wall-clock frame time split into the three places it can hide:
//!
//! - `fixed` — the `FixedMain` loop (Avian's solver and every 120 Hz
//!   system) — [`RunFixedMainLoopSystems::FixedMainLoop`]. A slow step
//!   shows here, and a slow *average* step shows as `steps > 1`: the
//!   fixed clock runs again to catch up, so the next frame is later
//!   still.
//! - `update` — `Update`/`PostUpdate` and the rest of the main schedule
//!   up to `Last`.
//! - `render` — everything after `Last` up to the next frame's `First`:
//!   the render world, the GPU and the present wait (vsync lives here).
//!
//! Inside `fixed`, Avian's own timers are summed over the frame's steps —
//! broad phase, narrow phase (contact generation) and the solver — with
//! the contact count, so the CSV says *which part* of physics is slow
//! and whether it scales with what is on screen.
//!
//! Each row also carries the live-entity count (sampled every
//! [`ENTITY_SAMPLE_EVERY`] frames), and the summary and report give its
//! first, last and peak value past warm-up — a soak that leaks entities
//! shows `last` well above `first` (F30-AC02's growth signal). Live audio
//! voices (the `AudioVoice` entities, counted whether or not an output
//! device attached a sink) are sampled and reported the same way.
//!
//! The report also judges the run against declared soak budgets
//! (F30-AC02, `timings.soak`): the measured frames are split in thirds
//! and the last third's entity and voice peaks must not exceed the
//! middle third's by more than a stated slack, and the virtual clock may
//! not have discarded any game time. A run too short to fill three
//! windows with samples says `inconclusive`, never `pass`.
//!
//! A frame longer than the virtual clock's `max_delta` is *clamped* by
//! Bevy: the simulation advances by the cap and the rest of the wall
//! time is discarded, which is the engine's silent way of dropping
//! gameplay time under overload. The recorder measures that dropped time
//! per frame (`clamped_ms`) and the summary and report state it, with the
//! most fixed steps one frame had to catch up, so an overloaded run says
//! so instead of merely feeling slow.
//!
//! The recorder writes one CSV row per frame when the app drops and
//! prints a percentile summary, so a person can play the event and hand
//! over the file. It is a developer aid: nothing reads it back, and it
//! costs one `Instant::now()` pair per stage when enabled and nothing
//! when not.
//!
//! Next to the CSV it writes `<csv>.report.json` (F30-AC01): the numbers
//! a stranger needs to *reproduce or distrust* the measurement — engine
//! commit and build profile, OS/CPU/GPU, the content fingerprint and
//! mods, the scene and settings the run named, and the percentile
//! timings. A percentile with no hardware, build or content next to it
//! is an anecdote. The report records what the process could observe;
//! a field it could not observe says `null`, never a guess.

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use avian3d::collision::CollisionDiagnostics;
use avian3d::dynamics::solver::SolverDiagnostics;
use bevy::ecs::entity::Entities;
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use mm2_game::Mm2Vfs;

use crate::audio::AudioVoice;
use serde_json::{Value, json};

/// Schema tag of the JSON report; bump when a field changes meaning.
pub const REPORT_SCHEMA: &str = "mm2-perf-report/1";

/// What the caller says about the run. `scene` and `settings` are
/// ordered `key → value` rows the caller owns (what was asked for —
/// city, event, car, driver; MSAA, shadows, vsync); the recorder adds
/// everything it can observe itself.
#[derive(Clone, Debug, Default)]
pub struct RunContext {
    /// `dev-world` or the city's logical path.
    pub world: String,
    /// Whether a retail installation was mounted.
    pub install: bool,
    /// Mods mounted over it (ids, in mount order).
    pub mods: Vec<String>,
    pub scene: Vec<(String, String)>,
    pub settings: Vec<(String, String)>,
}

/// The content the run measured, hashed once before the first frame.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ContentFingerprint {
    /// `mm2_assets::fingerprint::catalog` — the resolution map.
    catalog: String,
    /// `mm2_content::fingerprint::gameplay` — the gameplay bytes.
    gameplay: Result<(String, usize, u64), String>,
}

/// What the render adapter said it is.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Gpu {
    name: String,
    backend: String,
    device_type: String,
    driver: String,
    driver_info: String,
}

/// A frame longer than this many times the median counts as a hitch in
/// the summary.
const HITCH_FACTOR: f64 = 1.5;

/// Frames dropped from the front of the record: world load, shader and
/// pipeline compilation, texture upload. They dominate the maximum and
/// say nothing about how the game *plays*.
const WARMUP_FRAMES: usize = 120;

/// The live-entity count is an O(n) walk of the entity table, so it is
/// sampled every this-many frames and carried forward between samples
/// rather than adding a per-frame cost to the thing being measured.
const ENTITY_SAMPLE_EVERY: u64 = 30;

/// Soak budgets (F30-AC02) — implementation choices, declared here so the
/// report states the limits it judged against instead of a reader
/// guessing them. The measured frames are split into thirds; the first
/// third is the population filling up (traffic, crowd, ambient voices
/// spawning in), so the growth check compares the *peak of the last
/// third* with the *peak of the middle third*. A steady state recycles
/// and the two peaks sit together; a leak is still climbing and the tail
/// peak lands above the middle's by more than the slack. The slack
/// absorbs ordinary spawn/recycle jitter between two windows.
const SOAK_ENTITY_SLACK: u32 = 32;
const SOAK_VOICE_SLACK: u32 = 8;

/// A window needs at least this many samples before its peak says
/// anything; a shorter run is `inconclusive`, never `pass`.
const SOAK_MIN_SAMPLES_PER_WINDOW: usize = 3;

/// One soak check's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail,
    /// Too short a run to judge. Not a pass.
    Inconclusive,
}

impl Verdict {
    fn name(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// Growth of one counter between the middle and the last third of the
/// measured frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Growth {
    middle_peak: u32,
    tail_peak: u32,
    slack: u32,
    verdict: Verdict,
}

impl Growth {
    fn of(readings: &[u32], slack: u32) -> Self {
        let third = readings.len() / 3;
        let enough = third >= SOAK_MIN_SAMPLES_PER_WINDOW * ENTITY_SAMPLE_EVERY as usize;
        let peak = |w: &[u32]| w.iter().copied().max().unwrap_or(0);
        let middle_peak = peak(&readings[third..2 * third]);
        let tail_peak = peak(&readings[2 * third..]);
        let verdict = if !enough {
            Verdict::Inconclusive
        } else if tail_peak > middle_peak.saturating_add(slack) {
            Verdict::Fail
        } else {
            Verdict::Pass
        };
        Self {
            middle_peak,
            tail_peak,
            slack,
            verdict,
        }
    }

    fn json(&self) -> Value {
        json!({
            "middle_third_peak": self.middle_peak,
            "last_third_peak": self.tail_peak,
            "slack": self.slack,
            "verdict": self.verdict.name(),
        })
    }
}

/// The soak verdict over the measured frames: no unbounded entity or
/// voice growth, and no game time discarded by the virtual clock's cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Soak {
    entities: Growth,
    voices: Growth,
    overload: Verdict,
    overall: Verdict,
}

impl Soak {
    fn judge(rows: &[Row]) -> Self {
        let entities = Growth::of(
            &rows.iter().map(|r| r.entities).collect::<Vec<_>>(),
            SOAK_ENTITY_SLACK,
        );
        let voices = Growth::of(
            &rows.iter().map(|r| r.voices).collect::<Vec<_>>(),
            SOAK_VOICE_SLACK,
        );
        let overload = if rows.iter().any(|r| !r.clamped.is_zero()) {
            Verdict::Fail
        } else {
            Verdict::Pass
        };
        let all = [entities.verdict, voices.verdict, overload];
        let overall = if all.contains(&Verdict::Fail) {
            Verdict::Fail
        } else if all.contains(&Verdict::Inconclusive) {
            Verdict::Inconclusive
        } else {
            Verdict::Pass
        };
        Self {
            entities,
            voices,
            overload,
            overall,
        }
    }

    fn json(&self) -> Value {
        json!({
            "verdict": self.overall.name(),
            "entities": self.entities.json(),
            "voices": self.voices.json(),
            "overload": { "verdict": self.overload.name() },
            "min_samples_per_window": SOAK_MIN_SAMPLES_PER_WINDOW,
        })
    }

    fn line(&self) -> String {
        format!(
            "perf: soak {} | entities: middle-third peak {} last-third peak {} (slack {}) {} \
             | voices: {} -> {} (slack {}) {} | overload {}",
            self.overall.name(),
            self.entities.middle_peak,
            self.entities.tail_peak,
            self.entities.slack,
            self.entities.verdict.name(),
            self.voices.middle_peak,
            self.voices.tail_peak,
            self.voices.slack,
            self.voices.verdict.name(),
            self.overload.name(),
        )
    }
}

/// One finished frame.
#[derive(Clone, Copy, Debug)]
struct Row {
    frame: u64,
    total: Duration,
    fixed: Duration,
    update: Duration,
    render: Duration,
    steps: u32,
    /// Avian's broad phase, narrow phase and solver time summed over the
    /// frame's fixed steps.
    broad: Duration,
    narrow: Duration,
    solver: Duration,
    /// The most contacts any one of the frame's steps held.
    contacts: u32,
    /// Live entities at the most recent sample ([`ENTITY_SAMPLE_EVERY`]).
    entities: u32,
    /// Live audio voices (`AudioVoice` entities) at the same sample.
    voices: u32,
    /// Wall time beyond the virtual clock's `max_delta` that the frame
    /// discarded instead of simulating (zero for a frame within the cap).
    clamped: Duration,
}

/// Wall time a frame of `real` length lost to a virtual clock capped at
/// `max_delta`: the simulation advances by at most the cap, so whatever
/// lies beyond it is gone, not queued for the next frame.
fn clamped_by(real: Duration, max_delta: Duration) -> Duration {
    real.saturating_sub(max_delta)
}

/// Aggregates over the post-warm-up frames, in milliseconds.
#[derive(Clone, Copy, Debug)]
struct Stats {
    frames: usize,
    median: f64,
    p95: f64,
    p99: f64,
    max: f64,
    mean_total: f64,
    hitches: usize,
    multi_step: usize,
    worst: Row,
    max_fixed: f64,
    max_update: f64,
    max_render: f64,
    mean_fixed: f64,
    mean_update: f64,
    mean_render: f64,
    broad_per_step: f64,
    narrow_per_step: f64,
    solver_per_step: f64,
    steps_per_frame: f64,
    max_contacts: u32,
    /// Live entities at the first measured frame, the largest sample, and
    /// the last frame: a soak that leaks shows `last` well above `first`
    /// and still climbing.
    entities_first: u32,
    entities_max: u32,
    entities_last: u32,
    /// The same three readings for live audio voices: a loop or one-shot
    /// that never despawns shows as `last` climbing past `first`.
    voices_first: u32,
    voices_max: u32,
    voices_last: u32,
    /// Frames that exceeded the virtual cap, the game time they discarded
    /// in total and in the single worst frame (ms), and the most fixed
    /// steps any one frame ran to catch up.
    clamped_frames: usize,
    clamped_total: f64,
    clamped_worst: f64,
    max_steps: u32,
    /// The soak budgets' verdict (F30-AC02).
    soak: Soak,
}

/// The recorder's state — present only when `--perf-log` was given.
#[derive(Resource)]
struct PerfLog {
    path: PathBuf,
    rows: Vec<Row>,
    frame: u64,
    /// `First` of the frame in flight; `None` before the first frame.
    frame_start: Option<Instant>,
    fixed_start: Option<Instant>,
    fixed: Duration,
    /// `Last` of the frame in flight.
    main_end: Option<Instant>,
    steps: u32,
    broad: Duration,
    narrow: Duration,
    solver: Duration,
    contacts: u32,
    entities: u32,
    voices: u32,
    /// The in-flight frame's discarded wall time, read from the clocks at
    /// `Last` and folded into its row when the next frame begins.
    clamped: Duration,
    /// The virtual clock's cap, as last seen; `None` before any frame.
    max_delta: Option<Duration>,
    context: RunContext,
    content: Option<ContentFingerprint>,
    /// Filled by [`capture_adapter`] once the renderer exists; stays
    /// `None` in a run that never created one.
    gpu: Option<Gpu>,
}

impl PerfLog {
    fn new(path: PathBuf, context: RunContext) -> Self {
        Self {
            path,
            context,
            content: None,
            gpu: None,
            rows: Vec::new(),
            frame: 0,
            frame_start: None,
            fixed_start: None,
            fixed: Duration::ZERO,
            main_end: None,
            steps: 0,
            broad: Duration::ZERO,
            narrow: Duration::ZERO,
            solver: Duration::ZERO,
            contacts: 0,
            entities: 0,
            voices: 0,
            clamped: Duration::ZERO,
            max_delta: None,
        }
    }

    /// Close the frame that just ended at `now` and open the next one.
    fn begin_frame(&mut self, now: Instant) {
        if let (Some(start), Some(main_end)) = (self.frame_start, self.main_end) {
            let total = now - start;
            let main = main_end - start;
            self.rows.push(Row {
                frame: self.frame,
                total,
                fixed: self.fixed,
                update: main.saturating_sub(self.fixed),
                render: now - main_end,
                steps: self.steps,
                broad: self.broad,
                narrow: self.narrow,
                solver: self.solver,
                contacts: self.contacts,
                entities: self.entities,
                voices: self.voices,
                clamped: self.clamped,
            });
        }
        self.clamped = Duration::ZERO;
        self.frame += 1;
        self.frame_start = Some(now);
        self.fixed = Duration::ZERO;
        self.steps = 0;
        self.broad = Duration::ZERO;
        self.narrow = Duration::ZERO;
        self.solver = Duration::ZERO;
        self.contacts = 0;
    }

    /// Aggregate the post-warmup frames; `None` when the run was shorter
    /// than the warm-up.
    fn stats(&self) -> Option<Stats> {
        let rows = self.rows.get(WARMUP_FRAMES..).unwrap_or(&[]);
        if rows.is_empty() {
            return None;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut totals: Vec<f64> = rows.iter().map(|r| ms(r.total)).collect();
        totals.sort_by(f64::total_cmp);
        let pct = |p: f64| totals[((totals.len() - 1) as f64 * p).round() as usize];
        let median = pct(0.5);
        let hitches = rows
            .iter()
            .filter(|r| ms(r.total) > median * HITCH_FACTOR)
            .count();
        let worst = *rows.iter().max_by_key(|r| r.total).expect("rows non-empty");
        let stage_max = |f: fn(&Row) -> Duration| ms(rows.iter().map(f).max().unwrap());
        let mean = |f: fn(&Row) -> Duration| {
            rows.iter().map(|r| ms(f(r))).sum::<f64>() / rows.len() as f64
        };
        let steps: u64 = rows.iter().map(|r| u64::from(r.steps)).sum();
        let per_step = |f: fn(&Row) -> Duration| {
            rows.iter().map(|r| ms(f(r))).sum::<f64>() / steps.max(1) as f64
        };
        Some(Stats {
            frames: rows.len(),
            median,
            p95: pct(0.95),
            p99: pct(0.99),
            max: totals[totals.len() - 1],
            mean_total: totals.iter().sum::<f64>() / totals.len() as f64,
            hitches,
            multi_step: rows.iter().filter(|r| r.steps > 1).count(),
            worst,
            max_fixed: stage_max(|r| r.fixed),
            max_update: stage_max(|r| r.update),
            max_render: stage_max(|r| r.render),
            mean_fixed: mean(|r| r.fixed),
            mean_update: mean(|r| r.update),
            mean_render: mean(|r| r.render),
            broad_per_step: per_step(|r| r.broad),
            narrow_per_step: per_step(|r| r.narrow),
            solver_per_step: per_step(|r| r.solver),
            steps_per_frame: steps as f64 / rows.len() as f64,
            max_contacts: rows.iter().map(|r| r.contacts).max().unwrap_or(0),
            entities_first: rows[0].entities,
            entities_max: rows.iter().map(|r| r.entities).max().unwrap_or(0),
            entities_last: rows[rows.len() - 1].entities,
            voices_first: rows[0].voices,
            voices_max: rows.iter().map(|r| r.voices).max().unwrap_or(0),
            voices_last: rows[rows.len() - 1].voices,
            clamped_frames: rows.iter().filter(|r| !r.clamped.is_zero()).count(),
            clamped_total: rows.iter().map(|r| ms(r.clamped)).sum(),
            clamped_worst: stage_max(|r| r.clamped),
            max_steps: rows.iter().map(|r| r.steps).max().unwrap_or(0),
            soak: Soak::judge(rows),
        })
    }

    /// Percentile summary of the post-warmup frames.
    fn summary(&self) -> String {
        let Some(st) = self.stats() else {
            return format!(
                "perf: {} frames recorded, fewer than the {WARMUP_FRAMES} warm-up frames \
                 — run longer",
                self.rows.len()
            );
        };
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let worst = st.worst;
        format!(
            "perf: {n} frames after warm-up | frame ms: median {median:.2} p95 {p95:.2} \
             p99 {p99:.2} max {max:.2} | hitches (>{HITCH_FACTOR}x median): {h} ({hp:.1}%) \
             | frames with >1 fixed step: {multi_step} | worst stage max ms: fixed {fx:.2} \
             update {up:.2} render {rn:.2} | worst frame #{wf}: total {wt:.2} = fixed \
             {wfx:.2} + update {wup:.2} + render {wrn:.2} ({ws} steps)\n\
             perf: mean ms per frame: fixed {mfx:.2} update {mup:.2} render {mrn:.2} \
             | avian ms per fixed step: broad {sb:.2} narrow {sn:.2} solver {ss:.2} \
             | fixed steps per frame {spf:.2} | max contacts {mc} \
             | live entities first {ef} last {el} max {em} \
             | live voices first {vf} last {vl} max {vm}\n\
             perf: overload: {cf} frames over the {cap} virtual cap discarded {ct:.1} ms of \
             game time (worst frame {cw:.1} ms) | most fixed steps in one frame {msx}\n{soak}",
            n = st.frames,
            median = st.median,
            p95 = st.p95,
            p99 = st.p99,
            max = st.max,
            h = st.hitches,
            hp = 100.0 * st.hitches as f64 / st.frames as f64,
            multi_step = st.multi_step,
            fx = st.max_fixed,
            up = st.max_update,
            rn = st.max_render,
            wf = worst.frame,
            wt = ms(worst.total),
            wfx = ms(worst.fixed),
            wup = ms(worst.update),
            wrn = ms(worst.render),
            ws = worst.steps,
            mfx = st.mean_fixed,
            mup = st.mean_update,
            mrn = st.mean_render,
            sb = st.broad_per_step,
            sn = st.narrow_per_step,
            ss = st.solver_per_step,
            spf = st.steps_per_frame,
            mc = st.max_contacts,
            ef = st.entities_first,
            el = st.entities_last,
            em = st.entities_max,
            vf = st.voices_first,
            vl = st.voices_last,
            vm = st.voices_max,
            cf = st.clamped_frames,
            cap = self
                .max_delta
                .map_or_else(|| "unobserved".to_owned(), |d| format!("{:.0} ms", ms(d))),
            ct = st.clamped_total,
            cw = st.clamped_worst,
            msx = st.max_steps,
            soak = st.soak.line(),
        )
    }

    /// The reproducibility report (F30-AC01). Pure over what the recorder
    /// holds, so it is testable without a window or an install.
    fn report(&self) -> Value {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let kv = |rows: &[(String, String)]| {
            Value::Object(
                rows.iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            )
        };
        let content = match &self.content {
            None => Value::Null,
            Some(c) => {
                let gameplay = match &c.gameplay {
                    Ok((hash, files, bytes)) => {
                        json!({ "hash": hash, "files": files, "bytes": bytes })
                    }
                    Err(e) => json!({ "error": e }),
                };
                json!({ "catalog": c.catalog, "gameplay": gameplay })
            }
        };
        let gpu = self.gpu.as_ref().map_or(Value::Null, |g| {
            json!({
                "name": g.name,
                "backend": g.backend,
                "device_type": g.device_type,
                "driver": g.driver,
                "driver_info": g.driver_info,
            })
        });
        let timings = self.stats().map_or(Value::Null, |st| {
            json!({
                "frame_ms": {
                    "median": st.median, "p95": st.p95, "p99": st.p99,
                    "max": st.max, "mean": st.mean_total,
                },
                "hitches": {
                    "factor": HITCH_FACTOR,
                    "count": st.hitches,
                    "frames_with_multiple_fixed_steps": st.multi_step,
                },
                "stage_max_ms": {
                    "fixed": st.max_fixed, "update": st.max_update, "render": st.max_render,
                },
                "stage_mean_ms": {
                    "fixed": st.mean_fixed, "update": st.mean_update, "render": st.mean_render,
                },
                "avian_ms_per_fixed_step": {
                    "broad": st.broad_per_step,
                    "narrow": st.narrow_per_step,
                    "solver": st.solver_per_step,
                },
                "fixed_steps_per_frame": st.steps_per_frame,
                "max_contacts": st.max_contacts,
                "live_entities": {
                    "sample_every_frames": ENTITY_SAMPLE_EVERY,
                    "first": st.entities_first,
                    "last": st.entities_last,
                    "max": st.entities_max,
                },
                "live_voices": {
                    "sample_every_frames": ENTITY_SAMPLE_EVERY,
                    "first": st.voices_first,
                    "last": st.voices_last,
                    "max": st.voices_max,
                },
                "overload": {
                    "virtual_max_delta_ms": self.max_delta.map(ms),
                    "frames_clamped": st.clamped_frames,
                    "game_time_discarded_ms": st.clamped_total,
                    "worst_frame_discarded_ms": st.clamped_worst,
                    "max_fixed_steps_in_a_frame": st.max_steps,
                },
                "soak": st.soak.json(),
                "worst_frame": {
                    "frame": st.worst.frame,
                    "total_ms": ms(st.worst.total),
                    "fixed_steps": st.worst.steps,
                },
            })
        });
        let wall: Duration = self.rows.iter().map(|r| r.total).sum();
        json!({
            "schema": REPORT_SCHEMA,
            "engine": {
                "commit": crate::smoke::COMMIT,
                "version": env!("CARGO_PKG_VERSION"),
                "build": if cfg!(debug_assertions) { "debug" } else { "release" },
            },
            "host": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "cpu": cpu_brand(),
                "logical_cpus": std::thread::available_parallelism().ok().map(|n| n.get()),
                "gpu": gpu,
            },
            "content": {
                "world": self.context.world,
                "install": self.context.install,
                "mods": self.context.mods,
                "fingerprint": content,
            },
            "scene": kv(&self.context.scene),
            "settings": kv(&self.context.settings),
            "run": {
                "csv": self.path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "frames_recorded": self.rows.len(),
                "warmup_frames_dropped": WARMUP_FRAMES.min(self.rows.len()),
                "frames_measured": self.rows.len().saturating_sub(WARMUP_FRAMES),
                "wall_seconds": wall.as_secs_f64(),
            },
            "timings": timings,
        })
    }

    /// Where the report lands: the CSV's name plus `.report.json`, so a
    /// CSV literally called `x.json` is not overwritten by its own report.
    fn report_path(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".report.json");
        self.path.with_file_name(name)
    }

    fn write_report(&self) -> std::io::Result<PathBuf> {
        let path = self.report_path();
        let mut text =
            serde_json::to_string_pretty(&self.report()).map_err(std::io::Error::other)?;
        text.push('\n');
        std::fs::write(&path, text)?;
        Ok(path)
    }

    fn write_csv(&self) -> std::io::Result<()> {
        let mut out = BufWriter::new(File::create(&self.path)?);
        writeln!(
            out,
            "frame,total_ms,fixed_ms,update_ms,render_ms,fixed_steps,broad_ms,narrow_ms,solver_ms,contacts,entities,voices,clamped_ms"
        )?;
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        for r in &self.rows {
            writeln!(
                out,
                "{},{:.3},{:.3},{:.3},{:.3},{},{:.3},{:.3},{:.3},{},{},{},{:.3}",
                r.frame,
                ms(r.total),
                ms(r.fixed),
                ms(r.update),
                ms(r.render),
                r.steps,
                ms(r.broad),
                ms(r.narrow),
                ms(r.solver),
                r.contacts,
                r.entities,
                r.voices,
                ms(r.clamped)
            )?;
        }
        out.flush()
    }
}

impl Drop for PerfLog {
    /// The app drops its world on every exit path `main` takes after
    /// `run()`, so the file lands however the session ended.
    fn drop(&mut self) {
        match self.write_csv() {
            Ok(()) => info!(path = %self.path.display(), "perf log written"),
            Err(e) => warn!(path = %self.path.display(), error = %e, "perf log not written"),
        }
        match self.write_report() {
            Ok(path) => info!(path = %path.display(), "perf report written"),
            Err(e) => warn!(error = %e, "perf report not written"),
        }
        println!("{}", self.summary());
    }
}

fn frame_begin(mut log: ResMut<PerfLog>) {
    log.begin_frame(Instant::now());
}

fn fixed_loop_begin(mut log: ResMut<PerfLog>) {
    log.fixed_start = Some(Instant::now());
}

fn fixed_loop_end(mut log: ResMut<PerfLog>) {
    if let Some(start) = log.fixed_start.take() {
        log.fixed = start.elapsed();
    }
}

fn fixed_step(mut log: ResMut<PerfLog>) {
    log.steps += 1;
}

/// Sum Avian's per-step timers once the step's physics has run.
fn fixed_step_physics(
    collision: Option<Res<CollisionDiagnostics>>,
    solver: Option<Res<SolverDiagnostics>>,
    mut log: ResMut<PerfLog>,
) {
    if let Some(c) = collision {
        log.broad += c.broad_phase;
        log.narrow += c.narrow_phase;
        log.contacts = log.contacts.max(c.contact_count);
    }
    if let Some(s) = solver {
        log.solver += s.prepare_constraints
            + s.update_velocity_increments
            + s.integrate_velocities
            + s.warm_start
            + s.solve_constraints
            + s.integrate_positions
            + s.relax_velocities
            + s.apply_restitution
            + s.finalize
            + s.store_impulses
            + s.swept_ccd;
    }
}

/// Count live entities every [`ENTITY_SAMPLE_EVERY`] frames.
fn sample_entities(entities: &Entities, mut log: ResMut<PerfLog>) {
    if log.frame.is_multiple_of(ENTITY_SAMPLE_EVERY) {
        log.entities = entities.count_spawned();
    }
}

/// Count live audio voices on the same cadence. Every authored voice
/// carries [`AudioVoice`] whether or not an output device attached a sink,
/// so a headless soak counts them too.
fn sample_voices(voices: Query<(), With<AudioVoice>>, mut log: ResMut<PerfLog>) {
    if log.frame.is_multiple_of(ENTITY_SAMPLE_EVERY) {
        log.voices = voices.iter().count() as u32;
    }
}

fn frame_main_end(
    real: Res<Time<Real>>,
    virtual_time: Res<Time<Virtual>>,
    mut log: ResMut<PerfLog>,
) {
    log.main_end = Some(Instant::now());
    log.max_delta = Some(virtual_time.max_delta());
    log.clamped = clamped_by(real.delta(), virtual_time.max_delta());
}

/// The CPU's marketing name, if the OS will say. Best-effort: `None`
/// is reported as `null`, not a made-up string.
fn cpu_brand() -> Option<String> {
    let text = if cfg!(target_os = "macos") {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())?
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/proc/cpuinfo")
            .ok()?
            .lines()
            .find_map(|l| l.strip_prefix("model name").map(str::to_owned))?
            .trim_start_matches([' ', '\t', ':'])
            .to_owned()
    } else {
        std::env::var("PROCESSOR_IDENTIFIER").ok()?
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Hash the mounted content once, before the first frame, so the cost
/// lands in start-up and never in the measurement.
fn fingerprint(vfs: &mm2_assets::Vfs) -> ContentFingerprint {
    ContentFingerprint {
        catalog: mm2_assets::fingerprint::catalog(vfs),
        gameplay: mm2_content::fingerprint::gameplay(vfs)
            .map(|g| (g.display(), g.files, g.bytes))
            .map_err(|e| e.to_string()),
    }
}

/// Read the adapter once the renderer has produced it.
fn capture_adapter(info: Option<Res<RenderAdapterInfo>>, mut log: ResMut<PerfLog>) {
    if log.gpu.is_some() {
        return;
    }
    if let Some(info) = info {
        log.gpu = Some(Gpu {
            name: info.name.clone(),
            backend: format!("{:?}", info.backend),
            device_type: format!("{:?}", info.device_type),
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
        });
    }
}

/// Record per-frame stage timings into `path` (CSV), with a
/// reproducibility report beside it, and print a summary when the app
/// exits.
pub fn enable(app: &mut App, path: PathBuf, context: RunContext) {
    use bevy::app::RunFixedMainLoopSystems::{AfterFixedMainLoop, BeforeFixedMainLoop};
    let mut log = PerfLog::new(path, context);
    log.content = app
        .world()
        .get_resource::<Mm2Vfs>()
        .map(|vfs| fingerprint(&vfs.0));
    app.insert_resource(log)
        .add_systems(First, frame_begin)
        .add_systems(
            RunFixedMainLoop,
            fixed_loop_begin.in_set(BeforeFixedMainLoop),
        )
        .add_systems(RunFixedMainLoop, fixed_loop_end.in_set(AfterFixedMainLoop))
        .add_systems(FixedFirst, fixed_step)
        .add_systems(FixedLast, fixed_step_physics)
        .add_systems(Update, (capture_adapter, sample_entities, sample_voices))
        .add_systems(Last, frame_main_end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(frame: u64, total_ms: u64, steps: u32) -> Row {
        Row {
            frame,
            total: Duration::from_millis(total_ms),
            fixed: Duration::from_millis(total_ms / 2),
            update: Duration::from_millis(total_ms / 4),
            render: Duration::from_millis(total_ms / 4),
            steps,
            broad: Duration::ZERO,
            narrow: Duration::ZERO,
            solver: Duration::ZERO,
            contacts: 0,
            entities: 0,
            voices: 0,
            clamped: Duration::ZERO,
        }
    }

    #[test]
    fn summary_counts_hitches_past_warmup() {
        let mut log = PerfLog::new(
            std::env::temp_dir().join("mm2_perf_summary_test.csv"),
            RunContext::default(),
        );
        // A 1000 ms warm-up frame must not reach the maximum.
        log.rows.push(row(0, 1000, 1));
        for f in 1..WARMUP_FRAMES as u64 {
            log.rows.push(row(f, 16, 1));
        }
        for f in 0..98 {
            log.rows.push(row(WARMUP_FRAMES as u64 + f, 16, 1));
        }
        log.rows.push(row(900, 40, 3));
        log.rows.push(row(901, 40, 1));
        let s = log.summary();
        assert!(s.contains("100 frames after warm-up"), "{s}");
        assert!(s.contains("hitches (>1.5x median): 2 (2.0%)"), "{s}");
        assert!(s.contains("frames with >1 fixed step: 1"), "{s}");
        assert!(s.contains("max 40.00"), "{s}");
    }

    #[test]
    fn begin_frame_splits_stages() {
        let mut log = PerfLog::new(
            std::env::temp_dir().join("mm2_perf_stages_test.csv"),
            RunContext::default(),
        );
        let t0 = Instant::now();
        log.begin_frame(t0);
        log.fixed = Duration::from_millis(3);
        log.main_end = Some(t0 + Duration::from_millis(10));
        log.steps = 2;
        log.begin_frame(t0 + Duration::from_millis(16));
        let r = log.rows[0];
        assert_eq!(r.total, Duration::from_millis(16));
        assert_eq!(r.fixed, Duration::from_millis(3));
        assert_eq!(r.update, Duration::from_millis(7));
        assert_eq!(r.render, Duration::from_millis(6));
        assert_eq!(r.steps, 2);
    }

    fn long_log(context: RunContext) -> PerfLog {
        let mut log = PerfLog::new(
            std::env::temp_dir().join("mm2_perf_report_test.csv"),
            context,
        );
        for f in 0..(WARMUP_FRAMES as u64 + 100) {
            log.rows.push(row(f, 16, 1));
        }
        log
    }

    #[test]
    fn the_report_names_the_build_the_host_the_scene_and_the_percentiles() {
        let log = long_log(RunContext {
            world: "city/sf.psdl".into(),
            install: true,
            mods: vec!["paint-pack".into()],
            scene: vec![
                ("car".into(), "vpbug".into()),
                ("driver".into(), "bot".into()),
            ],
            settings: vec![("msaa".into(), "4".into()), ("vsync".into(), "off".into())],
        });
        let r = log.report();
        assert_eq!(r["schema"], REPORT_SCHEMA);
        assert_eq!(r["engine"]["commit"], crate::smoke::COMMIT);
        let build = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        assert_eq!(r["engine"]["build"], build);
        assert_eq!(r["host"]["os"], std::env::consts::OS);
        assert_eq!(r["host"]["arch"], std::env::consts::ARCH);
        assert!(r["host"]["logical_cpus"].as_u64().unwrap() >= 1);
        assert_eq!(r["content"]["world"], "city/sf.psdl");
        assert_eq!(r["content"]["install"], true);
        assert_eq!(r["content"]["mods"][0], "paint-pack");
        assert_eq!(r["scene"]["car"], "vpbug");
        assert_eq!(r["settings"]["vsync"], "off");
        assert_eq!(r["run"]["frames_recorded"], WARMUP_FRAMES + 100);
        assert_eq!(r["run"]["warmup_frames_dropped"], WARMUP_FRAMES);
        assert_eq!(r["run"]["frames_measured"], 100);
        assert_eq!(r["run"]["csv"], "mm2_perf_report_test.csv");
        assert_eq!(r["timings"]["frame_ms"]["median"], 16.0);
        assert_eq!(r["timings"]["frame_ms"]["p99"], 16.0);
        assert_eq!(r["timings"]["frame_ms"]["max"], 16.0);
    }

    #[test]
    fn a_field_the_run_could_not_observe_is_null_not_invented() {
        // No renderer ever reported an adapter, no VFS was fingerprinted,
        // and the run was too short to leave the warm-up.
        let mut log = PerfLog::new(
            std::env::temp_dir().join("mm2_perf_null_test.csv"),
            RunContext::default(),
        );
        log.rows.push(row(0, 16, 1));
        let r = log.report();
        assert!(r["host"]["gpu"].is_null());
        assert!(r["content"]["fingerprint"].is_null());
        assert!(r["timings"].is_null());
        assert_eq!(r["run"]["frames_measured"], 0);
        assert_eq!(r["run"]["warmup_frames_dropped"], 1);
    }

    #[test]
    fn the_report_carries_the_gpu_and_the_content_fingerprint_when_known() {
        let mut log = long_log(RunContext::default());
        log.gpu = Some(Gpu {
            name: "Apple M-test".into(),
            backend: "Metal".into(),
            device_type: "IntegratedGpu".into(),
            driver: String::new(),
            driver_info: String::new(),
        });
        log.content = Some(ContentFingerprint {
            catalog: "fnv1a64:0000000000000001".into(),
            gameplay: Ok(("fnv1a64:0000000000000002".into(), 3, 40)),
        });
        let r = log.report();
        assert_eq!(r["host"]["gpu"]["name"], "Apple M-test");
        assert_eq!(r["host"]["gpu"]["backend"], "Metal");
        let c = &r["content"]["fingerprint"];
        assert_eq!(c["catalog"], "fnv1a64:0000000000000001");
        assert_eq!(c["gameplay"]["hash"], "fnv1a64:0000000000000002");
        assert_eq!(c["gameplay"]["files"], 3);
        assert_eq!(c["gameplay"]["bytes"], 40);
    }

    #[test]
    fn the_fingerprint_follows_the_mounted_gameplay_content() {
        let dir = tempfile::tempdir().unwrap();
        let tune = dir.path().join("tune");
        std::fs::create_dir_all(&tune).unwrap();
        std::fs::write(tune.join("a.asnode"), b"mass 1000").unwrap();
        let mount = |dir: &std::path::Path| {
            let mut vfs = mm2_assets::Vfs::new();
            vfs.mount_dir(dir, 0).unwrap();
            fingerprint(&vfs)
        };
        let before = mount(dir.path());
        let (hash, files, bytes) = before.gameplay.clone().unwrap();
        assert_eq!((files, bytes), (1, 9));
        assert_eq!(before, mount(dir.path()), "same content, same fingerprint");
        std::fs::write(tune.join("a.asnode"), b"mass 2000").unwrap();
        let after = mount(dir.path()).gameplay.unwrap();
        assert_ne!(after.0, hash, "an edited tuning file must move the hash");
    }

    #[test]
    fn the_report_lands_beside_the_csv_and_never_on_it() {
        let dir = tempfile::tempdir().unwrap();
        // A CSV that is itself called `x.json` must keep its own file.
        let csv = dir.path().join("x.json");
        let mut log = PerfLog::new(csv.clone(), RunContext::default());
        for f in 0..(WARMUP_FRAMES as u64 + 2) {
            log.rows.push(row(f, 16, 1));
        }
        assert_eq!(log.report_path(), dir.path().join("x.json.report.json"));
        log.write_csv().unwrap();
        let written = log.write_report().unwrap();
        assert_ne!(written, csv);
        let parsed: Value =
            serde_json::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
        assert_eq!(parsed["schema"], REPORT_SCHEMA);
        assert!(
            std::fs::read_to_string(&csv)
                .unwrap()
                .starts_with("frame,total_ms"),
            "the CSV survives its report"
        );
    }

    #[test]
    fn entity_growth_is_reported_first_last_and_peak() {
        let mut log = long_log(RunContext::default());
        for (i, r) in log.rows.iter_mut().enumerate() {
            r.entities = match i {
                // Warm-up churn must not set the baseline or the peak.
                0..=119 => 9_000,
                120..=159 => 1_000,
                160..=179 => 1_500,
                _ => 1_200,
            };
        }
        let report = log.report();
        let live = &report["timings"]["live_entities"];
        assert_eq!(live["first"], 1_000);
        assert_eq!(live["max"], 1_500);
        assert_eq!(live["last"], 1_200);
        assert_eq!(live["sample_every_frames"], ENTITY_SAMPLE_EVERY);
        let s = log.summary();
        assert!(
            s.contains("live entities first 1000 last 1200 max 1500"),
            "{s}"
        );
    }

    /// A log with `measured` post-warm-up frames whose entity and voice
    /// readings come from `f(measured_index)`.
    fn soak_log(measured: usize, f: impl Fn(usize) -> (u32, u32)) -> PerfLog {
        let mut log = PerfLog::new(
            std::env::temp_dir().join("mm2_perf_soak_test.csv"),
            RunContext::default(),
        );
        for i in 0..WARMUP_FRAMES + measured {
            let mut r = row(i as u64, 16, 1);
            // Warm-up churn must never count toward a window.
            (r.entities, r.voices) = if i < WARMUP_FRAMES {
                (50_000, 500)
            } else {
                f(i - WARMUP_FRAMES)
            };
            log.rows.push(r);
        }
        log
    }

    #[test]
    fn a_population_that_fills_then_recycles_passes_the_soak() {
        // Fills during the first third, then jitters inside the slack.
        let log = soak_log(900, |i| {
            let fill = (i as u32).min(300);
            (1_000 + fill + (i as u32 % 7), 10 + fill / 50)
        });
        let soak = log.stats().unwrap().soak;
        assert_eq!(soak.overall, Verdict::Pass, "{}", soak.line());
        let report = log.report();
        let j = &report["timings"]["soak"];
        assert_eq!(j["verdict"], "pass");
        assert_eq!(j["entities"]["slack"], SOAK_ENTITY_SLACK);
        assert!(log.summary().contains("perf: soak pass"));
    }

    #[test]
    fn a_leak_that_keeps_climbing_fails_the_soak_and_says_which_counter() {
        let log = soak_log(900, |i| (1_000 + i as u32, 10));
        let soak = log.stats().unwrap().soak;
        assert_eq!(soak.entities.verdict, Verdict::Fail);
        assert_eq!(soak.voices.verdict, Verdict::Pass);
        assert_eq!(soak.overall, Verdict::Fail);
        assert!(soak.entities.tail_peak > soak.entities.middle_peak + SOAK_ENTITY_SLACK);
        let leaky_voices = soak_log(900, |i| (1_000, 10 + i as u32 / 20));
        let v = leaky_voices.stats().unwrap().soak;
        assert_eq!(
            (v.entities.verdict, v.voices.verdict, v.overall),
            (Verdict::Pass, Verdict::Fail, Verdict::Fail)
        );
        assert_eq!(
            leaky_voices.report()["timings"]["soak"]["voices"]["verdict"],
            "fail"
        );
    }

    #[test]
    fn a_climb_inside_the_slack_is_jitter_not_a_leak() {
        let log = soak_log(900, |i| {
            (1_000 + (i as u32 / 300) * (SOAK_ENTITY_SLACK / 2), 10)
        });
        assert_eq!(log.stats().unwrap().soak.entities.verdict, Verdict::Pass);
        let over = soak_log(900, |i| {
            (1_000 + (i as u32 / 300) * (SOAK_ENTITY_SLACK + 1), 10)
        });
        assert_eq!(over.stats().unwrap().soak.entities.verdict, Verdict::Fail);
    }

    #[test]
    fn a_run_too_short_for_three_windows_is_inconclusive_not_a_pass() {
        let enough = 3 * SOAK_MIN_SAMPLES_PER_WINDOW * ENTITY_SAMPLE_EVERY as usize;
        let steady = |n| soak_log(n, |_| (1_000, 10));
        assert_eq!(steady(enough).stats().unwrap().soak.overall, Verdict::Pass);
        let short = steady(enough - 3).stats().unwrap().soak;
        assert_eq!(short.overall, Verdict::Inconclusive);
        // A short run that is visibly leaking is still inconclusive, not a
        // fabricated verdict either way.
        let short_leak = soak_log(enough - 3, |i| (1_000 + 10 * i as u32, 10));
        assert_eq!(
            short_leak.stats().unwrap().soak.entities.verdict,
            Verdict::Inconclusive
        );
        assert!(steady(100).summary().contains("perf: soak inconclusive"));
    }

    #[test]
    fn discarded_game_time_fails_the_soak_but_warmup_discards_do_not() {
        let mut log = soak_log(900, |_| (1_000, 10));
        log.rows[5].clamped = Duration::from_millis(900);
        assert_eq!(log.stats().unwrap().soak.overall, Verdict::Pass);
        log.rows[WARMUP_FRAMES + 400].clamped = Duration::from_millis(900);
        let soak = log.stats().unwrap().soak;
        assert_eq!(soak.overload, Verdict::Fail);
        assert_eq!(soak.overall, Verdict::Fail);
        assert_eq!(
            log.report()["timings"]["soak"]["overload"]["verdict"],
            "fail"
        );
    }

    #[test]
    fn a_failure_outranks_an_inconclusive_check() {
        // Overload fails while the windows are far too short to judge.
        let mut log = soak_log(100, |_| (1_000, 10));
        log.rows[WARMUP_FRAMES + 50].clamped = Duration::from_millis(900);
        let soak = log.stats().unwrap().soak;
        assert_eq!(soak.entities.verdict, Verdict::Inconclusive);
        assert_eq!(soak.overall, Verdict::Fail);
    }

    #[test]
    fn voice_growth_is_reported_first_last_and_peak() {
        let mut log = long_log(RunContext::default());
        for (i, r) in log.rows.iter_mut().enumerate() {
            r.voices = match i {
                // Warm-up voices must not set the baseline or the peak.
                0..=119 => 90,
                120..=159 => 12,
                160..=179 => 30,
                _ => 20,
            };
        }
        let report = log.report();
        let live = &report["timings"]["live_voices"];
        assert_eq!(live["first"], 12);
        assert_eq!(live["max"], 30);
        assert_eq!(live["last"], 20);
        let s = log.summary();
        assert!(s.contains("live voices first 12 last 20 max 30"), "{s}");
    }

    #[test]
    fn the_voice_sampler_counts_audio_voice_entities_on_the_cadence() {
        use crate::audio::VoiceKind;
        let mut app = App::new();
        app.insert_resource(PerfLog::new(
            std::env::temp_dir().join("mm2_perf_voices_test.csv"),
            RunContext::default(),
        ))
        .add_systems(Update, sample_voices);
        let voices: Vec<Entity> = (0..4)
            .map(|_| {
                app.world_mut()
                    .spawn(AudioVoice {
                        kind: VoiceKind::Horn,
                    })
                    .id()
            })
            .collect();
        // An entity that is not a voice never counts.
        app.world_mut().spawn_empty();
        app.world_mut().resource_mut::<PerfLog>().frame = 1;
        app.update();
        assert_eq!(app.world().resource::<PerfLog>().voices, 0);
        app.world_mut().resource_mut::<PerfLog>().frame = ENTITY_SAMPLE_EVERY;
        app.update();
        assert_eq!(app.world().resource::<PerfLog>().voices, 4);
        for e in voices.into_iter().take(3) {
            app.world_mut().despawn(e);
        }
        app.world_mut().resource_mut::<PerfLog>().frame = 2 * ENTITY_SAMPLE_EVERY;
        app.update();
        assert_eq!(app.world().resource::<PerfLog>().voices, 1);
    }

    #[test]
    fn a_frame_beyond_the_virtual_cap_reports_the_time_it_discarded() {
        let cap = Duration::from_millis(250);
        assert_eq!(clamped_by(Duration::from_millis(16), cap), Duration::ZERO);
        assert_eq!(clamped_by(cap, cap), Duration::ZERO);
        assert_eq!(
            clamped_by(Duration::from_millis(900), cap),
            Duration::from_millis(650)
        );
    }

    #[test]
    fn the_frame_end_system_reads_the_real_and_virtual_clocks() {
        let mut app = App::new();
        app.insert_resource(PerfLog::new(
            std::env::temp_dir().join("mm2_perf_clamp_test.csv"),
            RunContext::default(),
        ))
        .init_resource::<Time<Real>>()
        .init_resource::<Time<Virtual>>()
        .add_systems(Last, frame_main_end);
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(1000));
        app.update();
        let log = app.world().resource::<PerfLog>();
        assert_eq!(log.max_delta, Some(Duration::from_millis(250)));
        assert_eq!(log.clamped, Duration::from_millis(750));
        // A within-cap frame afterwards clears it rather than accumulating.
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(16));
        app.update();
        assert_eq!(app.world().resource::<PerfLog>().clamped, Duration::ZERO);
    }

    #[test]
    fn overload_is_summarised_past_warmup_and_not_for_warmup_churn() {
        let mut log = long_log(RunContext::default());
        log.max_delta = Some(Duration::from_millis(250));
        // A load hitch in warm-up must not count.
        log.rows[3].clamped = Duration::from_millis(5000);
        log.rows[130].clamped = Duration::from_millis(100);
        log.rows[200].clamped = Duration::from_millis(400);
        log.rows[200].steps = 9;
        let report = log.report();
        let o = &report["timings"]["overload"];
        assert_eq!(o["virtual_max_delta_ms"], 250.0);
        assert_eq!(o["frames_clamped"], 2);
        assert_eq!(o["game_time_discarded_ms"], 500.0);
        assert_eq!(o["worst_frame_discarded_ms"], 400.0);
        assert_eq!(o["max_fixed_steps_in_a_frame"], 9);
        let s = log.summary();
        assert!(
            s.contains("2 frames over the 250 ms virtual cap discarded 500.0 ms"),
            "{s}"
        );
        assert!(s.contains("most fixed steps in one frame 9"), "{s}");
    }

    #[test]
    fn a_run_that_never_ran_a_frame_does_not_invent_the_cap() {
        let log = long_log(RunContext::default());
        assert!(log.report()["timings"]["overload"]["virtual_max_delta_ms"].is_null());
        assert!(log.summary().contains("over the unobserved virtual cap"));
    }

    #[test]
    fn the_csv_has_an_entities_column_matching_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = PerfLog::new(dir.path().join("e.csv"), RunContext::default());
        let mut r = row(0, 16, 1);
        r.entities = 321;
        r.voices = 7;
        log.rows.push(r);
        log.write_csv().unwrap();
        let text = std::fs::read_to_string(&log.path).unwrap();
        let mut lines = text.lines();
        let header: Vec<_> = lines.next().unwrap().split(',').collect();
        let cells: Vec<_> = lines.next().unwrap().split(',').collect();
        let at = |name: &str| header.iter().position(|h| *h == name).unwrap();
        assert_eq!(header.len(), cells.len());
        assert_eq!(header.last(), Some(&"clamped_ms"));
        assert_eq!(cells.last(), Some(&"0.000"));
        assert_eq!(cells[at("voices")], "7");
        assert_eq!(cells[at("entities")], "321");
    }

    #[test]
    fn the_sampler_counts_what_is_spawned_and_not_what_was_despawned() {
        let mut app = App::new();
        app.insert_resource(PerfLog::new(
            std::env::temp_dir().join("mm2_perf_entities_test.csv"),
            RunContext::default(),
        ))
        .add_systems(Update, sample_entities);
        let baseline = app.world().entities().count_spawned();
        let spawned: Vec<Entity> = (0..50)
            .map(|_| app.world_mut().spawn_empty().id())
            .collect();
        // Off the sampling cadence: nothing is counted.
        app.world_mut().resource_mut::<PerfLog>().frame = 1;
        app.update();
        assert_eq!(app.world().resource::<PerfLog>().entities, 0);
        app.world_mut().resource_mut::<PerfLog>().frame = ENTITY_SAMPLE_EVERY;
        app.update();
        assert_eq!(app.world().resource::<PerfLog>().entities, baseline + 50);
        for e in spawned.into_iter().take(20) {
            app.world_mut().despawn(e);
        }
        app.world_mut().resource_mut::<PerfLog>().frame = 2 * ENTITY_SAMPLE_EVERY;
        app.update();
        assert_eq!(app.world().resource::<PerfLog>().entities, baseline + 30);
    }
}
