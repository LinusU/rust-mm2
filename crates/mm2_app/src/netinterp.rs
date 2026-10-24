//! Bounded host-timestamped snapshot buffer for remote copies that are
//! drawn only (NETPLAY-07B, `docs/netcode-redesign.md`).
//!
//! The arrival-gap [`RemoteLerp`](crate::netdrive::RemoteLerp) retimes
//! each blend by how far apart two datagrams happened to land, so packet
//! jitter changes how fast a body moves. A [`SnapBuffer`] instead keys
//! every sample by the host's own tick time and plays the stream back on
//! a clock derived from arrivals: jitter changes how *deep* the buffer is
//! (the playout delay), never the speed of a body along its path.
//!
//! - **Clock.** `d = arrival - host_time` is the one-way delay plus an
//!   unknown constant. `base` tracks its floor (a lower sample is taken at
//!   once, the floor creeps up slowly so a route change is followed), so
//!   `now - base` is the host's clock as best the client can tell.
//! - **Jitter.** An RFC 3550 style mean deviation of `d` sizes the delay:
//!   `delay = BASE_DELAY + JITTER_GAIN * jitter`, clamped, and slewed at a
//!   bounded rate so playback speed changes by a few percent at most.
//! - **Sampling.** The render time `now - base - delay` is interpolated
//!   between the bracketing samples; past the newest sample the pose is
//!   extrapolated along the sample's velocity for at most
//!   [`MAX_EXTRAPOLATION`] and then held; before the oldest it holds.
//! - **Bounded.** At most [`CAPACITY`] samples; non-finite and
//!   non-increasing samples are refused.
//!
//! The buffer writes nothing: it is only ever sampled into the pose of a
//! copy that has no collider. All numbers are designed (the original game
//! has no network interpolation).

use std::collections::VecDeque;

use bevy::prelude::*;

/// Most samples kept (~0.5 s of a 60 Hz stream). Designed.
pub const CAPACITY: usize = 32;
/// Playout delay with no jitter, seconds (about three snapshot
/// periods, so one lost datagram still has both neighbours). Designed.
pub const BASE_DELAY: f64 = 0.05;
/// Delay added per second of mean jitter. Designed.
pub const JITTER_GAIN: f64 = 3.0;
/// Largest playout delay, seconds. Designed.
pub const MAX_DELAY: f64 = 0.25;
/// Largest rate the delay may change at (seconds of delay per second of
/// wall time): playback speed varies by at most this fraction. Designed.
pub const DELAY_SLEW: f64 = 0.1;
/// Longest velocity extrapolation past the newest sample, seconds. Designed.
pub const MAX_EXTRAPOLATION: f64 = 0.1;
/// Rate at which the delay floor is allowed to rise (seconds per second),
/// so a path that got slower is followed. Designed.
const BASE_DRIFT: f64 = 0.01;

#[derive(Debug, Clone, Copy)]
struct Sample {
    /// Host time, seconds.
    t: f64,
    pos: Vec3,
    rot: Quat,
    vel: Vec3,
}

/// The snapshot history of one drawn replica, plus the playout clock.
#[derive(Component, Debug, Clone, Default)]
pub struct SnapBuffer {
    samples: VecDeque<Sample>,
    /// Floor of `arrival - host_time` (seconds).
    base: f64,
    /// Last `arrival - host_time`, for the deviation.
    last_delay: f64,
    /// Mean deviation of `arrival - host_time`, seconds.
    jitter: f64,
    /// Playout delay currently in use, seconds.
    delay: f64,
    /// Session clock of the previous [`Self::sample`] / first push.
    clock: Option<f64>,
    /// Samples refused (non-finite or not newer than the newest).
    pub refused: u64,
}

impl SnapBuffer {
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Mean arrival jitter, seconds.
    pub fn jitter(&self) -> f64 {
        self.jitter
    }

    /// Playout delay in use, seconds.
    pub fn delay(&self) -> f64 {
        self.delay
    }

    fn target_delay(&self) -> f64 {
        (BASE_DELAY + JITTER_GAIN * self.jitter).min(MAX_DELAY)
    }

    /// Drop the history: the authority moved the car, the old life must
    /// not be interpolated from. The clock estimate is kept.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Record a host sample taken at `host_t` seconds that arrived at
    /// session time `arrival`. Returns whether it was kept.
    pub fn push(&mut self, host_t: f64, arrival: f64, pos: Vec3, rot: Quat, vel: Vec3) -> bool {
        let finite = host_t.is_finite()
            && arrival.is_finite()
            && pos.is_finite()
            && rot.is_finite()
            && vel.is_finite();
        if !finite || self.samples.back().is_some_and(|s| host_t <= s.t) {
            self.refused += 1;
            return false;
        }
        let d = arrival - host_t;
        if self.clock.is_none() {
            self.base = d;
            self.last_delay = d;
            self.delay = BASE_DELAY;
        } else {
            if d < self.base {
                self.base = d;
            }
            self.jitter += ((d - self.last_delay).abs() - self.jitter) / 16.0;
            self.last_delay = d;
        }
        self.clock.get_or_insert(arrival);
        if self.samples.len() >= CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample {
            t: host_t,
            pos,
            rot: rot.normalize(),
            vel,
        });
        true
    }

    /// The pose to draw at session time `now`, or `None` with no sample.
    /// Advances the playout delay towards its jitter-sized target.
    pub fn sample(&mut self, now: f64) -> Option<(Vec3, Quat)> {
        let last_now = self.clock.replace(now)?;
        let dt = (now - last_now).clamp(0.0, 0.5);
        // The floor rises slowly so a slower path is followed.
        self.base += BASE_DRIFT * dt;
        let target = self.target_delay();
        let step = DELAY_SLEW * dt;
        self.delay += (target - self.delay).clamp(-step, step);
        let render_t = now - self.base - self.delay;
        let first = self.samples.front()?;
        if render_t <= first.t {
            return Some((first.pos, first.rot));
        }
        let last = self.samples.back()?;
        if render_t >= last.t {
            let ahead = (render_t - last.t).min(MAX_EXTRAPOLATION) as f32;
            return Some((last.pos + last.vel * ahead, last.rot));
        }
        let hi = self.samples.partition_point(|s| s.t <= render_t);
        let (a, b) = (self.samples[hi - 1], self.samples[hi]);
        let f = ((render_t - a.t) / (b.t - a.t)) as f32;
        Some((a.pos.lerp(b.pos, f), a.rot.slerp(b.rot, f).normalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 1.0 / 60.0;

    /// A car at 20 m/s along +X, one sample per tick.
    fn pos_at(t: f64) -> Vec3 {
        Vec3::new(20.0 * t as f32, 0.0, 0.0)
    }

    /// Feed `ticks` samples, arrivals `latency(i)` after their host time,
    /// sampling every tick of wall time; returns the drawn x per frame.
    fn run(ticks: usize, latency: impl Fn(usize) -> f64) -> Vec<f32> {
        let mut buf = SnapBuffer::default();
        let mut arrivals: Vec<(f64, usize)> = (0..ticks)
            .map(|i| (i as f64 * DT + latency(i), i))
            .collect();
        arrivals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut next = 0;
        let mut drawn = Vec::new();
        let mut now = 0.0;
        while now < ticks as f64 * DT {
            while next < arrivals.len() && arrivals[next].0 <= now {
                let (_, i) = arrivals[next];
                let t = i as f64 * DT;
                buf.push(
                    t,
                    arrivals[next].0,
                    pos_at(t),
                    Quat::IDENTITY,
                    Vec3::X * 20.0,
                );
                next += 1;
            }
            if let Some((p, _)) = buf.sample(now) {
                drawn.push(p.x);
            }
            now += DT;
        }
        drawn
    }

    /// Speed of the drawn body, per frame, once the buffer has filled.
    fn speeds(drawn: &[f32]) -> Vec<f32> {
        drawn
            .windows(2)
            .skip(30)
            .map(|w| (w[1] - w[0]) / DT as f32)
            .collect()
    }

    #[test]
    fn a_steady_stream_plays_back_at_the_hosts_speed() {
        let s = speeds(&run(240, |_| 0.03));
        assert!(
            s.iter().all(|v| (v - 20.0).abs() < 0.5),
            "{:?}",
            s.iter()
                .cloned()
                .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)))
        );
    }

    #[test]
    fn jitter_deepens_the_buffer_instead_of_retiming_the_body() {
        // +-15 ms of deterministic jitter, never reordering a tick past
        // the next by more than the buffer holds.
        let jitter = |i: usize| 0.03 + 0.015 * (((i * 7) % 5) as f64 / 2.0 - 1.0);
        let drawn = run(600, jitter);
        let s = speeds(&drawn);
        // The legacy blend would stall and surge with each arrival gap;
        // the buffered body stays within a few percent of 20 m/s once
        // the delay has settled.
        let late = &s[s.len() / 2..];
        assert!(
            late.iter().all(|v| (v - 20.0).abs() < 3.0),
            "{:?}",
            late.iter()
                .cloned()
                .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)))
        );
    }

    #[test]
    fn jitter_raises_the_delay_and_a_clean_stream_does_not() {
        let mut clean = SnapBuffer::default();
        let mut noisy = SnapBuffer::default();
        for i in 0..200 {
            let t = i as f64 * DT;
            clean.push(t, t + 0.03, pos_at(t), Quat::IDENTITY, Vec3::ZERO);
            let j = if i % 2 == 0 { 0.0 } else { 0.04 };
            noisy.push(t, t + 0.03 + j, pos_at(t), Quat::IDENTITY, Vec3::ZERO);
        }
        assert!(noisy.jitter() > 10.0 * clean.jitter().max(1e-6));
        assert!(noisy.target_delay() > clean.target_delay());
        assert!(noisy.target_delay() <= MAX_DELAY);
    }

    #[test]
    fn the_delay_slews_at_a_bounded_rate() {
        let mut buf = SnapBuffer::default();
        buf.push(0.0, 0.0, Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO);
        buf.sample(0.0);
        buf.jitter = 0.1;
        let before = buf.delay();
        buf.sample(0.1);
        assert!((buf.delay() - before - DELAY_SLEW * 0.1).abs() < 1e-9);
    }

    #[test]
    fn extrapolation_is_bounded_then_holds() {
        let mut buf = SnapBuffer::default();
        buf.push(0.0, 0.0, Vec3::ZERO, Quat::IDENTITY, Vec3::X * 10.0);
        buf.sample(0.0);
        // Far past the only sample: at most MAX_EXTRAPOLATION of travel.
        let (p, _) = buf.sample(10.0).unwrap();
        assert!((p.x - 10.0 * MAX_EXTRAPOLATION as f32).abs() < 1e-4, "{p}");
    }

    #[test]
    fn it_is_bounded_and_refuses_bad_or_old_samples() {
        let mut buf = SnapBuffer::default();
        for i in 0..(CAPACITY * 3) {
            let t = i as f64 * DT;
            assert!(buf.push(t, t, pos_at(t), Quat::IDENTITY, Vec3::ZERO));
        }
        assert_eq!(buf.len(), CAPACITY);
        assert!(!buf.push(0.0, 5.0, Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO));
        assert!(!buf.push(9.0, 9.0, Vec3::NAN, Quat::IDENTITY, Vec3::ZERO));
        assert!(!buf.push(f64::NAN, 9.0, Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO));
        assert_eq!(buf.refused, 3);
        assert_eq!(buf.len(), CAPACITY);
    }

    #[test]
    fn a_cleared_buffer_never_draws_the_old_life() {
        let mut buf = SnapBuffer::default();
        buf.push(0.0, 0.0, Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO);
        buf.push(DT, DT, Vec3::X, Quat::IDENTITY, Vec3::ZERO);
        buf.clear();
        assert!(buf.sample(1.0).is_none());
        buf.push(5.0, 1.0, Vec3::Y * 100.0, Quat::IDENTITY, Vec3::ZERO);
        let (p, _) = buf.sample(1.1).unwrap();
        assert_eq!(p, Vec3::Y * 100.0);
    }

    #[test]
    fn rotation_takes_the_short_way() {
        let mut buf = SnapBuffer::default();
        let a = Quat::from_rotation_y(std::f32::consts::PI - 0.05);
        let b = Quat::from_rotation_y(-std::f32::consts::PI + 0.05);
        buf.push(0.0, 0.0, Vec3::ZERO, a, Vec3::ZERO);
        buf.push(DT, DT, Vec3::ZERO, b, Vec3::ZERO);
        buf.sample(0.0);
        // Render time lands between the two samples.
        let (_, r) = buf.sample(buf.base + buf.delay + DT * 0.5).unwrap();
        assert!(r.angle_between(a) < 0.06 && r.angle_between(b) < 0.06);
    }
}
