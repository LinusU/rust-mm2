//! Continuous-driving diagnostics for networked sessions (NETPLAY-01).
//!
//! A headless `mm2` process can write a per-update trajectory of its own
//! car (`--net-trace`), drive a reproducible input route through the
//! production `VehicleInput` path (`--net-route`), stall itself on a
//! schedule (`--net-stall`) and inject a deliberate fault into the real
//! own-seat correction path (`--net-fault`). [`analyze`] turns a trace
//! into the continuous-motion metrics `docs/netcode-redesign.md`
//! defines and [`check`] judges them against explicit [`Limits`].
//!
//! The analysis is oracle-side only: it reads finished logs and never
//! feeds back into prediction. Scripts supply inputs; nothing here
//! assigns a pose or replaces the car physics.
//!
//! A correction is measured from the trajectory alone — the part of a
//! tick's displacement that the car's own velocity does not explain — so
//! legitimate high-speed travel is not a teleport and a fault is caught
//! whatever label the log gives it. A sample pair that straddles an
//! authority reset (a lifecycle `epoch` change) is skipped and counted:
//! an explicit respawn is not a rubber-band.

use std::sync::{Mutex, OnceLock};

use bevy::math::{Quat, Vec3};
use mm2_vehicle::VehicleInput;
use serde_json::{Value, json};

/// The fixed step the production schedule runs at.
pub const DT: f64 = 1.0 / 60.0;

/// A reproducible input program, a pure function of the ticks since the
/// session began `Playing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Full throttle, a hard brake, then throttle again.
    Accel,
    /// Throttle through a sinusoidal slalom, then brake into reverse.
    Slalom,
}

impl Route {
    /// Parse a `--net-route` value.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "accel" => Ok(Self::Accel),
            "slalom" => Ok(Self::Slalom),
            _ => Err(format!("unknown route {s:?} (accel, slalom)")),
        }
    }

    /// Stable name for the record.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accel => "accel",
            Self::Slalom => "slalom",
        }
    }

    /// The driver's input at `tick` ticks into play.
    pub fn input(self, tick: u64) -> VehicleInput {
        let t = tick as f32;
        match self {
            Self::Accel => match tick % 1200 {
                0..600 => VehicleInput {
                    throttle: 1.0,
                    ..Default::default()
                },
                600..720 => VehicleInput {
                    brake: 1.0,
                    ..Default::default()
                },
                _ => VehicleInput {
                    throttle: 1.0,
                    ..Default::default()
                },
            },
            Self::Slalom => match tick % 1200 {
                0..480 => VehicleInput {
                    throttle: 0.8,
                    steering: (std::f32::consts::TAU * t / 180.0).sin() * 0.6,
                    ..Default::default()
                },
                480..600 => VehicleInput {
                    brake: 1.0,
                    ..Default::default()
                },
                // Held brake from rest drives the car backwards.
                600..780 => VehicleInput {
                    brake: 1.0,
                    steering: (std::f32::consts::TAU * t / 120.0).sin() * 0.5,
                    ..Default::default()
                },
                _ => VehicleInput {
                    throttle: 0.8,
                    steering: (std::f32::consts::TAU * t / 240.0).sin() * 0.4,
                    ..Default::default()
                },
            },
        }
    }
}

/// A scripted contact role (`--net-contact`): unlike a [`Route`] it is
/// keyed on the nearest other car as well as on the ticks since play
/// began, so it can evade, brake in front of, swipe or push a neighbour
/// wherever the grid put it. The laws live in `input::contact_input`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactScript {
    /// Hold still until a car closes inside the trigger range, then
    /// drive off hard away from it (a predicted contact that is dodged).
    Evade,
    /// Drive straight, then brake hard at a fixed tick and stop (the
    /// lead car of a rear-end).
    Brake,
    /// Charge the nearest car aiming beside it, then run alongside (a
    /// side swipe rather than a head-on).
    Swipe,
    /// Charge the nearest car and keep the throttle down through the
    /// contact, never parking (a push).
    Push,
    /// Hold the heading of +X (toward the dev world's east perimeter
    /// wall) and keep the throttle down through whatever stops it: the
    /// car that ends up pinned against the wall.
    Wall,
}

impl ContactScript {
    /// Parse a `--net-contact` value.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "evade" => Ok(Self::Evade),
            "brake" => Ok(Self::Brake),
            "swipe" => Ok(Self::Swipe),
            "push" => Ok(Self::Push),
            "wall" => Ok(Self::Wall),
            _ => Err(format!(
                "unknown contact script {s:?} (evade, brake, swipe, push, wall)"
            )),
        }
    }

    /// Stable name for the record.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Evade => "evade",
            Self::Brake => "brake",
            Self::Swipe => "swipe",
            Self::Push => "push",
            Self::Wall => "wall",
        }
    }
}

/// A scheduled whole-process stall: sleep `ms` after every `every`-th
/// `Playing` update, so a peer sees a bounded input/snapshot gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stall {
    pub every: u32,
    pub ms: u64,
}

impl Stall {
    /// Upper bound on one injected stall (ms): a stall is a perturbation,
    /// not a hang.
    pub const MAX_MS: u64 = 2_000;

    /// Parse a `--net-stall EVERY:MS` value.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (every, ms) = s.split_once(':').ok_or("expected EVERY:MS")?;
        let every: u32 = every.parse().map_err(|e| format!("EVERY: {e}"))?;
        let ms: u64 = ms.parse().map_err(|e| format!("MS: {e}"))?;
        if every == 0 || ms > Self::MAX_MS {
            return Err(format!("EVERY must be > 0 and MS <= {}", Self::MAX_MS));
        }
        Ok(Self { every, ms })
    }
}

/// A deliberate defect in the own-seat correction path, for proving the
/// checker catches it. Applied when a snapshot reconciles the predicted
/// seat, so it travels the real correction code and its output is
/// indistinguishable from a trajectory discontinuity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// A small sideways displacement every [`Fault::PERIOD`] snapshots.
    Kicks,
    /// An alternating yaw kick every [`Fault::PERIOD`] snapshots.
    YawKicks,
}

impl Fault {
    /// Snapshots between two faults.
    pub const PERIOD: u64 = 15;
    /// Sideways displacement of a [`Fault::Kicks`] kick (m).
    pub const KICK_M: f32 = 0.6;
    /// Yaw of a [`Fault::YawKicks`] kick (radians, about 5 degrees).
    pub const KICK_YAW: f32 = 0.09;

    /// Parse a `--net-fault` value.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "kicks" => Ok(Self::Kicks),
            "yaw" => Ok(Self::YawKicks),
            _ => Err(format!("unknown fault {s:?} (kicks, yaw)")),
        }
    }
}

static FAULT: OnceLock<Fault> = OnceLock::new();
static FAULT_STEP: Mutex<u64> = Mutex::new(0);
static NOTES: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// Arm the fault for this process (once).
pub fn arm_fault(fault: Fault) {
    let _ = FAULT.set(fault);
}

/// The fault to apply to the own seat for this reconciliation, as a
/// position offset and a rotation to pre-multiply, if one is due.
/// `right` is the car's lateral axis.
pub fn own_fault(right: Vec3) -> Option<(Vec3, Quat)> {
    let fault = *FAULT.get()?;
    let mut step = FAULT_STEP.lock().ok()?;
    *step += 1;
    if !(*step).is_multiple_of(Fault::PERIOD) {
        return None;
    }
    let k = *step / Fault::PERIOD;
    Some(match fault {
        Fault::Kicks => (right * Fault::KICK_M, Quat::IDENTITY),
        Fault::YawKicks => {
            let sign = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
            (Vec3::ZERO, Quat::from_rotation_y(sign * Fault::KICK_YAW))
        }
    })
}

/// Queue a correction record for the trace writer to drain with the
/// frame that produced it.
pub fn note_correction(kind: &str, snap_tick: u64, local: Vec3, wire: Vec3) {
    if let Ok(mut notes) = NOTES.lock() {
        notes.push(json!({
            "ev": "own_correction", "kind": kind, "snap_tick": snap_tick,
            "local": [local.x, local.y, local.z], "wire": [wire.x, wire.y, wire.z],
            "gap": local.distance(wire),
        }));
    }
}

/// Drain the queued correction records.
pub fn take_notes() -> Vec<Value> {
    NOTES
        .lock()
        .map(|mut n| std::mem::take(&mut *n))
        .unwrap_or_default()
}

/// One trace line: the player's car after a completed update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Update counter in this process.
    pub frame: u64,
    /// Session clock tick.
    pub tick: u64,
    /// Wall-clock milliseconds since the process's run began.
    pub wall_ms: f64,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f64,
    /// Lifecycle epoch: authority resets observed so far.
    pub epoch: u64,
    /// Highest snapshot tick applied, if the process receives any.
    pub snap_tick: Option<u64>,
}

impl Sample {
    /// The JSONL form.
    pub fn to_json(&self) -> Value {
        json!({
            "f": self.frame, "tick": self.tick, "wall_ms": self.wall_ms,
            "p": self.pos, "v": self.vel, "yaw": self.yaw, "ep": self.epoch,
            "snap_tick": self.snap_tick,
        })
    }

    fn from_json(v: &Value) -> Option<Self> {
        let vec3 = |k: &str| -> Option<[f64; 3]> {
            let a = v.get(k)?.as_array()?;
            Some([
                a.first()?.as_f64()?,
                a.get(1)?.as_f64()?,
                a.get(2)?.as_f64()?,
            ])
        };
        Some(Self {
            frame: v.get("f")?.as_u64()?,
            tick: v.get("tick")?.as_u64()?,
            wall_ms: v.get("wall_ms")?.as_f64()?,
            pos: vec3("p")?,
            vel: vec3("v")?,
            yaw: v.get("yaw")?.as_f64()?,
            epoch: v.get("ep")?.as_u64()?,
            snap_tick: v.get("snap_tick").and_then(Value::as_u64),
        })
    }
}

/// A parsed trace: samples plus the correction events the process logged.
#[derive(Debug, Default)]
pub struct Trace {
    pub samples: Vec<Sample>,
    pub events: Vec<Value>,
}

/// Parse a JSONL trace; a line that is not a sample or an `ev` record is
/// an error (a truncated log is not silently shortened).
pub fn parse_trace(text: &str) -> Result<Trace, String> {
    let mut trace = Trace::default();
    for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("trace line {}: {e}", i + 1))?;
        if v.get("ev").is_some() {
            trace.events.push(v);
        } else {
            trace.samples.push(
                Sample::from_json(&v).ok_or_else(|| format!("trace line {}: bad sample", i + 1))?,
            );
        }
    }
    Ok(trace)
}

/// Distribution summary of a set of non-negative values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dist {
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Dist {
    fn of(mut xs: Vec<f64>) -> Self {
        if xs.is_empty() {
            return Self::default();
        }
        xs.sort_by(f64::total_cmp);
        let q = |p: f64| xs[((xs.len() - 1) as f64 * p).round() as usize];
        Self {
            n: xs.len(),
            p50: q(0.5),
            p95: q(0.95),
            p99: q(0.99),
            max: *xs.last().unwrap(),
        }
    }

    fn to_json(&self) -> Value {
        json!({"n": self.n, "p50": self.p50, "p95": self.p95, "p99": self.p99, "max": self.max})
    }
}

/// Residuals below this (m) are integration noise, not a correction.
pub const NOISE_FLOOR_M: f64 = 0.02;
/// Correction-size thresholds (m) the report counts per minute.
pub const THRESHOLDS_M: [f64; 4] = [0.1, 0.25, 0.5, 2.0];
/// Ticks within which two opposite yaw steps count as a flip (1 s).
pub const YAW_FLIP_WINDOW: u64 = 60;
/// A yaw step (radians) beyond which a reversal counts as a flip.
pub const YAW_FLIP_RAD: f64 = 0.035;

/// Continuous-motion metrics of one seat's trace.
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    /// Consecutive-tick pairs examined.
    pub pairs: usize,
    /// Pairs skipped: a tick gap, or a lifecycle epoch change (respawn).
    pub gaps: usize,
    pub respawn_skips: usize,
    /// Simulated seconds covered by the examined pairs.
    pub seconds: f64,
    /// Peak speed seen (m/s): context for "legitimate high-speed movement".
    pub peak_speed: f64,
    /// Residual displacement per pair, full population.
    pub residual_all: Dist,
    /// Residual displacement of pairs above the noise floor only.
    pub residual_nonzero: Dist,
    /// Pairs whose residual exceeds each of [`THRESHOLDS_M`].
    pub over: [usize; 4],
    /// Sum of residuals above the noise floor, per minute (m/min).
    pub cumulative_per_min: f64,
    /// Back-and-forth yaw steps larger than [`YAW_FLIP_RAD`].
    pub yaw_flips: usize,
    /// Largest single-tick yaw step (radians).
    pub max_yaw_step: f64,
    /// `own_correction` events the process logged, by kind.
    pub logged: Vec<(String, usize)>,
}

fn wrap(a: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    (a + std::f64::consts::PI).rem_euclid(tau) - std::f64::consts::PI
}

/// Measure a trace's continuous motion.
pub fn analyze(trace: &Trace) -> Metrics {
    let mut m = Metrics::default();
    let mut residuals = Vec::new();
    // The last significant yaw step and its tick: a kick is a single
    // step with quiet ticks around it, so a flip is judged against the
    // previous significant step, not the previous tick.
    let mut prev_kick: Option<(u64, f64)> = None;
    for w in trace.samples.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        m.peak_speed = m.peak_speed.max(speed(&b.vel));
        if b.tick != a.tick + 1 {
            m.gaps += 1;
            prev_kick = None;
            continue;
        }
        if a.epoch != b.epoch {
            m.respawn_skips += 1;
            prev_kick = None;
            continue;
        }
        let mut r = 0.0f64;
        for k in 0..3 {
            let expect = 0.5 * (a.vel[k] + b.vel[k]) * DT;
            r += (b.pos[k] - a.pos[k] - expect).powi(2);
        }
        let r = r.sqrt();
        let dyaw = wrap(b.yaw - a.yaw);
        m.max_yaw_step = m.max_yaw_step.max(dyaw.abs());
        if dyaw.abs() > YAW_FLIP_RAD {
            if let Some((t, d)) = prev_kick
                && b.tick - t <= YAW_FLIP_WINDOW
                && d * dyaw < 0.0
            {
                m.yaw_flips += 1;
            }
            prev_kick = Some((b.tick, dyaw));
        }
        m.pairs += 1;
        residuals.push(r);
    }
    m.seconds = m.pairs as f64 * DT;
    let nonzero: Vec<f64> = residuals
        .iter()
        .copied()
        .filter(|&r| r > NOISE_FLOOR_M)
        .collect();
    for (i, t) in THRESHOLDS_M.iter().enumerate() {
        m.over[i] = residuals.iter().filter(|&&r| r > *t).count();
    }
    if m.seconds > 0.0 {
        m.cumulative_per_min = nonzero.iter().sum::<f64>() / (m.seconds / 60.0);
    }
    m.residual_all = Dist::of(residuals);
    m.residual_nonzero = Dist::of(nonzero);
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for e in &trace.events {
        let k = e.get("kind").and_then(Value::as_str).unwrap_or("?");
        match kinds.iter_mut().find(|(n, _)| n == k) {
            Some((_, c)) => *c += 1,
            None => kinds.push((k.to_string(), 1)),
        }
    }
    m.logged = kinds;
    m
}

fn speed(v: &[f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

impl Metrics {
    /// Corrections above threshold index `i` per minute of simulated time.
    pub fn over_per_min(&self, i: usize) -> f64 {
        if self.seconds > 0.0 {
            self.over[i] as f64 / (self.seconds / 60.0)
        } else {
            0.0
        }
    }

    /// The machine-readable report.
    pub fn to_json(&self) -> Value {
        json!({
            "pairs": self.pairs, "gaps": self.gaps, "respawn_skips": self.respawn_skips,
            "sim_seconds": self.seconds, "peak_speed_mps": self.peak_speed,
            "residual_m_all": self.residual_all.to_json(),
            "residual_m_nonzero": self.residual_nonzero.to_json(),
            "over": THRESHOLDS_M.iter().enumerate().map(|(i, t)| json!({
                "threshold_m": t, "count": self.over[i], "per_min": self.over_per_min(i),
            })).collect::<Vec<_>>(),
            "cumulative_correction_m_per_min": self.cumulative_per_min,
            "yaw_flips": self.yaw_flips, "max_yaw_step_rad": self.max_yaw_step,
            "logged_corrections": self.logged.iter().map(|(k, c)| json!({"kind": k, "count": c})).collect::<Vec<_>>(),
        })
    }
}

/// Pass/fail limits. Defaults are the baseline-mode ceilings: loose
/// enough that the current production code is *reported*, not gated, on
/// quality (NETPLAY-09 tightens them into the regression gate).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Largest tolerated single-tick residual (m).
    pub max_residual_m: f64,
    /// Tolerated corrections above [`THRESHOLDS_M`]`[1]` per minute.
    pub max_visible_per_min: f64,
    /// Tolerated yaw flips per minute.
    pub max_yaw_flips_per_min: f64,
}

impl Limits {
    /// The strict form: no visible kick, no yaw oscillation.
    pub const STRICT: Self = Self {
        max_residual_m: 0.5,
        max_visible_per_min: 2.0,
        max_yaw_flips_per_min: 2.0,
    };
}

/// Judge `m`; every breach is one line.
pub fn check(m: &Metrics, limits: &Limits) -> Vec<String> {
    let mut bad = Vec::new();
    if m.pairs == 0 {
        bad.push("no consecutive samples to judge".to_string());
        return bad;
    }
    if m.residual_all.max > limits.max_residual_m {
        bad.push(format!(
            "largest single-tick correction {:.2} m exceeds {:.2} m",
            m.residual_all.max, limits.max_residual_m
        ));
    }
    if m.over_per_min(1) > limits.max_visible_per_min {
        bad.push(format!(
            "{:.1} corrections/min above {:.2} m exceeds {:.1}",
            m.over_per_min(1),
            THRESHOLDS_M[1],
            limits.max_visible_per_min
        ));
    }
    let flips = m.yaw_flips as f64 / (m.seconds / 60.0);
    if flips > limits.max_yaw_flips_per_min {
        bad.push(format!(
            "{flips:.1} yaw flips/min exceeds {:.1}",
            limits.max_yaw_flips_per_min
        ));
    }
    bad
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight drive at `speed` for `n` ticks, optionally mutated.
    fn drive(n: u64, speed: f64, mutate: impl Fn(u64, &mut Sample)) -> Trace {
        let mut samples = Vec::new();
        for i in 0..n {
            let mut s = Sample {
                frame: i,
                tick: i + 1,
                wall_ms: i as f64 * 16.7,
                pos: [speed * DT * i as f64, 0.5, 0.0],
                vel: [speed, 0.0, 0.0],
                yaw: 0.0,
                epoch: 0,
                snap_tick: None,
            };
            mutate(i, &mut s);
            samples.push(s);
        }
        Trace {
            samples,
            events: vec![],
        }
    }

    #[test]
    fn high_speed_travel_is_not_a_teleport() {
        // 70 m/s moves 1.2 m a tick; the velocity explains it all.
        let m = analyze(&drive(1800, 70.0, |_, _| {}));
        assert!(m.residual_all.max < 1e-9, "{m:?}");
        assert!(check(&m, &Limits::STRICT).is_empty());
        assert!(m.peak_speed > 69.0);
    }

    #[test]
    fn an_explicit_respawn_is_skipped_not_flagged() {
        let t = drive(600, 20.0, |i, s| {
            if i >= 300 {
                s.epoch = 1;
                s.pos[0] += 400.0;
                s.yaw = 2.0;
            }
        });
        let m = analyze(&t);
        assert_eq!(m.respawn_skips, 1);
        assert!(check(&m, &Limits::STRICT).is_empty(), "{m:?}");
    }

    #[test]
    fn the_same_jump_without_an_epoch_is_a_teleport() {
        let t = drive(600, 20.0, |i, s| {
            if i >= 300 {
                s.pos[0] += 400.0;
            }
        });
        let bad = check(&analyze(&t), &Limits::STRICT);
        assert!(bad.iter().any(|b| b.contains("single-tick")), "{bad:?}");
    }

    #[test]
    fn repeated_small_corrections_fail_though_each_is_modest() {
        // 0.6 m every 15 ticks: no one of them is a teleport.
        let t = drive(1800, 20.0, |i, s| {
            s.pos[2] += 0.6 * (i / 15) as f64;
        });
        let m = analyze(&t);
        assert!(m.residual_all.max < 0.7);
        let bad = check(&m, &Limits::STRICT);
        assert!(bad.iter().any(|b| b.contains("corrections/min")), "{bad:?}");
    }

    #[test]
    fn alternating_yaw_kicks_fail() {
        let t = drive(1800, 20.0, |i, s| {
            let k = i / 15;
            s.yaw += if k.is_multiple_of(2) { 0.0 } else { 0.09 };
        });
        let m = analyze(&t);
        assert!(m.yaw_flips > 100, "{m:?}");
        let bad = check(&m, &Limits::STRICT);
        assert!(bad.iter().any(|b| b.contains("yaw flips")), "{bad:?}");
    }

    #[test]
    fn zero_samples_do_not_hide_a_bad_correction() {
        let t = drive(3000, 10.0, |i, s| {
            if i >= 1500 {
                s.pos[2] += 3.0;
            }
        });
        let m = analyze(&t);
        // Almost every residual is zero, yet the nonzero population shows it.
        assert!(m.residual_all.p50 < NOISE_FLOOR_M);
        assert!(m.residual_nonzero.max > 2.9, "{m:?}");
        assert!(!check(&m, &Limits::STRICT).is_empty());
    }

    #[test]
    fn a_trace_round_trips_and_rejects_garbage() {
        let t = drive(5, 3.0, |_, _| {});
        let text: String = t
            .samples
            .iter()
            .map(|s| s.to_json().to_string() + "\n")
            .collect();
        assert_eq!(parse_trace(&text).unwrap().samples, t.samples);
        assert!(parse_trace("{\"f\":1}\n").is_err());
        assert!(parse_trace("not json\n").is_err());
    }

    #[test]
    fn contact_scripts_parse_and_round_trip() {
        for name in ["evade", "brake", "swipe", "push", "wall"] {
            assert_eq!(ContactScript::parse(name).unwrap().as_str(), name);
        }
        assert!(ContactScript::parse("ram").is_err());
    }

    #[test]
    fn routes_and_specs_parse() {
        assert_eq!(Route::parse("slalom"), Ok(Route::Slalom));
        assert!(Route::parse("x").is_err());
        assert_eq!(Stall::parse("120:80"), Ok(Stall { every: 120, ms: 80 }));
        assert!(Stall::parse("0:5").is_err());
        assert!(Stall::parse("5:99999").is_err());
        assert!(Stall::parse("5").is_err());
        assert_eq!(Fault::parse("yaw"), Ok(Fault::YawKicks));
        assert!(Route::Slalom.input(700).brake > 0.0);
        assert!(Route::Accel.input(10).throttle > 0.0);
    }
}
