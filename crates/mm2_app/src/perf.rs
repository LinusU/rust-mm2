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
//! The recorder writes one CSV row per frame when the app drops and
//! prints a percentile summary, so a person can play the event and hand
//! over the file. It is a developer aid: nothing reads it back, and it
//! costs one `Instant::now()` pair per stage when enabled and nothing
//! when not.

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use avian3d::collision::CollisionDiagnostics;
use avian3d::dynamics::solver::SolverDiagnostics;
use bevy::prelude::*;

/// A frame longer than this many times the median counts as a hitch in
/// the summary.
const HITCH_FACTOR: f64 = 1.5;

/// Frames dropped from the front of the record: world load, shader and
/// pipeline compilation, texture upload. They dominate the maximum and
/// say nothing about how the game *plays*.
const WARMUP_FRAMES: usize = 120;

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
}

impl PerfLog {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
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
            });
        }
        self.frame += 1;
        self.frame_start = Some(now);
        self.fixed = Duration::ZERO;
        self.steps = 0;
        self.broad = Duration::ZERO;
        self.narrow = Duration::ZERO;
        self.solver = Duration::ZERO;
        self.contacts = 0;
    }

    /// Percentile summary of the post-warmup frames.
    fn summary(&self) -> String {
        let rows = self.rows.get(WARMUP_FRAMES..).unwrap_or(&[]);
        if rows.is_empty() {
            return format!(
                "perf: {} frames recorded, fewer than the {WARMUP_FRAMES} warm-up frames \
                 — run longer",
                self.rows.len()
            );
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut totals: Vec<f64> = rows.iter().map(|r| ms(r.total)).collect();
        totals.sort_by(f64::total_cmp);
        let pct = |p: f64| totals[((totals.len() - 1) as f64 * p).round() as usize];
        let median = pct(0.5);
        let hitches: Vec<&Row> = rows
            .iter()
            .filter(|r| ms(r.total) > median * HITCH_FACTOR)
            .collect();
        let multi_step = rows.iter().filter(|r| r.steps > 1).count();
        let worst = rows.iter().max_by_key(|r| r.total).expect("rows non-empty");
        let stage_max = |f: fn(&Row) -> Duration| ms(rows.iter().map(f).max().unwrap());
        let mean = |f: fn(&Row) -> Duration| {
            rows.iter().map(|r| ms(f(r))).sum::<f64>() / rows.len() as f64
        };
        let steps: u64 = rows.iter().map(|r| u64::from(r.steps)).sum();
        let per_step = |f: fn(&Row) -> Duration| {
            rows.iter().map(|r| ms(f(r))).sum::<f64>() / steps.max(1) as f64
        };
        format!(
            "perf: {n} frames after warm-up | frame ms: median {median:.2} p95 {p95:.2} \
             p99 {p99:.2} max {max:.2} | hitches (>{HITCH_FACTOR}x median): {h} ({hp:.1}%) \
             | frames with >1 fixed step: {multi_step} | worst stage max ms: fixed {fx:.2} \
             update {up:.2} render {rn:.2} | worst frame #{wf}: total {wt:.2} = fixed \
             {wfx:.2} + update {wup:.2} + render {wrn:.2} ({ws} steps)\n\
             perf: mean ms per frame: fixed {mfx:.2} update {mup:.2} render {mrn:.2} \
             | avian ms per fixed step: broad {sb:.2} narrow {sn:.2} solver {ss:.2} \
             | fixed steps per frame {spf:.2} | max contacts {mc}",
            n = rows.len(),
            p95 = pct(0.95),
            p99 = pct(0.99),
            max = totals[totals.len() - 1],
            h = hitches.len(),
            hp = 100.0 * hitches.len() as f64 / rows.len() as f64,
            fx = stage_max(|r| r.fixed),
            up = stage_max(|r| r.update),
            rn = stage_max(|r| r.render),
            wf = worst.frame,
            wt = ms(worst.total),
            wfx = ms(worst.fixed),
            wup = ms(worst.update),
            wrn = ms(worst.render),
            ws = worst.steps,
            mfx = mean(|r| r.fixed),
            mup = mean(|r| r.update),
            mrn = mean(|r| r.render),
            sb = per_step(|r| r.broad),
            sn = per_step(|r| r.narrow),
            ss = per_step(|r| r.solver),
            spf = steps as f64 / rows.len() as f64,
            mc = rows.iter().map(|r| r.contacts).max().unwrap_or(0),
        )
    }

    fn write_csv(&self) -> std::io::Result<()> {
        let mut out = BufWriter::new(File::create(&self.path)?);
        writeln!(
            out,
            "frame,total_ms,fixed_ms,update_ms,render_ms,fixed_steps,broad_ms,narrow_ms,solver_ms,contacts"
        )?;
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        for r in &self.rows {
            writeln!(
                out,
                "{},{:.3},{:.3},{:.3},{:.3},{},{:.3},{:.3},{:.3},{}",
                r.frame,
                ms(r.total),
                ms(r.fixed),
                ms(r.update),
                ms(r.render),
                r.steps,
                ms(r.broad),
                ms(r.narrow),
                ms(r.solver),
                r.contacts
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

fn frame_main_end(mut log: ResMut<PerfLog>) {
    log.main_end = Some(Instant::now());
}

/// Record per-frame stage timings into `path` (CSV) and print a summary
/// when the app exits.
pub fn enable(app: &mut App, path: PathBuf) {
    use bevy::app::RunFixedMainLoopSystems::{AfterFixedMainLoop, BeforeFixedMainLoop};
    app.insert_resource(PerfLog::new(path))
        .add_systems(First, frame_begin)
        .add_systems(
            RunFixedMainLoop,
            fixed_loop_begin.in_set(BeforeFixedMainLoop),
        )
        .add_systems(RunFixedMainLoop, fixed_loop_end.in_set(AfterFixedMainLoop))
        .add_systems(FixedFirst, fixed_step)
        .add_systems(FixedLast, fixed_step_physics)
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
        }
    }

    #[test]
    fn summary_counts_hitches_past_warmup() {
        let mut log = PerfLog::new(std::env::temp_dir().join("mm2_perf_summary_test.csv"));
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
        let mut log = PerfLog::new(std::env::temp_dir().join("mm2_perf_stages_test.csv"));
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
}
