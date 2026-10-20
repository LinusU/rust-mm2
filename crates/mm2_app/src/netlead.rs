//! The client's prediction-lead and input-buffer feedback controller
//! (NETPLAY-02D, `docs/netplay-timeline.md`).
//!
//! Everything here is a design choice, not an original-game rule, and is
//! deterministic: callers pass monotonic wall nanoseconds in, so tests drive
//! it with a synthetic clock.
//!
//! # What is controlled
//!
//! The host consumes exactly one command per seat per fixed tick, so the
//! client must *deliver* commands at 60 Hz on average, with enough of a
//! cushion that jitter does not starve the host. The cushion is the host's
//! per-seat buffer depth, which the host reports in every snapshot
//! (`SnapEntry::input_depth`, protocol v30). [`LeadController`] steers that
//! depth to [`LeadConfig::target_depth`] whole ticks:
//!
//! * **Signal.** The reported depth, smoothed ([`LeadController::observe`]).
//!   Depth is path-independent, so it is stable on asymmetric links where
//!   `RTT / 2` is wrong. The RTT estimate (minimum filter plus deviation and
//!   an explicit clock-uncertainty term) only seeds
//!   [`LeadController::initial_lead_ticks`] before the first report.
//! * **Actuator.** A bounded rate adjustment, in parts per million of the
//!   *wall* time between whole fixed ticks ([`Pacer`]). The step itself is
//!   never retuned: the pacer only decides how many whole 1/60 s ticks are
//!   due in a frame, so physics stays on the production fixed step.
//! * **Hysteresis.** Inside a dead band around the target the rate decays to
//!   zero, so ±100 ppm clock drift does not make it hunt; outside it the
//!   rate follows the error, and the rate itself slews, so it cannot jump.
//!   Below [`LeadConfig::min_depth`] or above [`LeadConfig::max_depth`] the
//!   correction runs at full authority.
//! * **Stale acks.** With no ack advance for [`LeadConfig::stale_ack_ns`]
//!   the link is treated as stalled: the rate is held at zero (speeding up
//!   into a dead link only builds a backlog) and the filter is reseeded from
//!   the first report after the ack moves again.
//!
//! # Pacing, catch-up and rebase
//!
//! [`Pacer::advance`] turns each frame's wall time into a whole number of
//! ticks. After a stall it catches up at most
//! [`LeadConfig::max_catchup_ticks`] per call and keeps at most
//! [`LeadConfig::max_backlog_ticks`] of backlog; older wall time is dropped
//! and reported as a [`Rebase`]. A gap of [`LeadConfig::sleep_ns`] or more
//! (sleep/resume) drops everything. Dropping wall time never skips a seq —
//! seq is one per *simulated* tick — but it breaks the seq → server-tick
//! pairing, so [`SeqMap`] is cleared by [`LeadController::rebase`] and
//! re-derived from the next ack.

use std::collections::VecDeque;

use crate::netcmd::{HISTORY_CAP, TickRecord};

/// One fixed tick in nanoseconds (`FIXED_HZ` = 60), rounded.
pub const TICK_NS: i64 = 16_666_667;

const PPM: i64 = 1_000_000;

/// Controller and pacer bounds. [`Default`] is the shipped tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeadConfig {
    /// Host buffer depth the controller steers to, in ticks.
    pub target_depth: u32,
    /// Depth below this is an emergency (starvation is imminent).
    pub min_depth: u32,
    /// Depth above this is an emergency (latency and overflow risk). Must
    /// stay well under `mm2_net::BUFFER_CAP`.
    pub max_depth: u32,
    /// Half-width of the no-correction band around the target, in 1/256
    /// tick.
    pub deadband_q8: i64,
    /// Rate correction per tick of depth error, in ppm.
    pub gain_ppm_per_tick: i64,
    /// Largest rate adjustment either way, in ppm of wall time.
    pub max_adjust_ppm: i64,
    /// Largest change of the rate per second, in ppm.
    pub slew_ppm_per_s: i64,
    /// Most ticks one [`Pacer::advance`] call may release.
    pub max_catchup_ticks: u32,
    /// Most wall backlog kept after a call, in ticks; the rest is dropped.
    pub max_backlog_ticks: u32,
    /// A frame this long or longer drops all wall time (sleep/resume).
    pub sleep_ns: i64,
    /// No ack advance for this long = the link is stalled.
    pub stale_ack_ns: i64,
}

impl Default for LeadConfig {
    fn default() -> Self {
        Self {
            target_depth: 2,
            min_depth: 1,
            max_depth: 6,
            deadband_q8: 192,
            gain_ppm_per_tick: 20_000,
            max_adjust_ppm: 40_000,
            slew_ppm_per_s: 40_000,
            max_catchup_ticks: 4,
            max_backlog_ticks: 8,
            sleep_ns: 1_000_000_000,
            stale_ack_ns: 500_000_000,
        }
    }
}

/// Why wall time was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rebase {
    /// A stall left more backlog than [`LeadConfig::max_backlog_ticks`];
    /// `dropped_ticks` whole ticks of wall time were discarded.
    Stall {
        /// Whole ticks of wall time dropped.
        dropped_ticks: u32,
    },
    /// The frame was [`LeadConfig::sleep_ns`] or longer: everything dropped.
    Sleep,
}

/// One [`Pacer::advance`] result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    /// Whole fixed ticks to simulate now.
    pub ticks: u32,
    /// Set when wall time was dropped; the caller must
    /// [`LeadController::rebase`].
    pub rebase: Option<Rebase>,
}

/// Schedules whole fixed ticks over wall time.
#[derive(Debug, Clone)]
pub struct Pacer {
    cfg: LeadConfig,
    /// Scaled wall time not yet turned into ticks, in ns.
    acc_ns: i64,
    ticks: u64,
    stall_rebases: u64,
    sleep_rebases: u64,
    dropped_ticks: u64,
}

impl Pacer {
    /// A pacer with no backlog.
    pub fn new(cfg: LeadConfig) -> Self {
        Self {
            cfg,
            acc_ns: 0,
            ticks: 0,
            stall_rebases: 0,
            sleep_rebases: 0,
            dropped_ticks: 0,
        }
    }

    /// Account `wall_dt_ns` of wall time at `rate_ppm` (positive = more
    /// ticks per second) and release the whole ticks now due. The tick
    /// length itself is untouched; only how many ticks fit in the wall time
    /// changes, by at most [`LeadConfig::max_adjust_ppm`].
    pub fn advance(&mut self, wall_dt_ns: i64, rate_ppm: i64) -> Pace {
        let wall_dt_ns = wall_dt_ns.max(0);
        if wall_dt_ns >= self.cfg.sleep_ns {
            self.acc_ns = 0;
            self.sleep_rebases += 1;
            return Pace {
                ticks: 0,
                rebase: Some(Rebase::Sleep),
            };
        }
        let rate = rate_ppm.clamp(-self.cfg.max_adjust_ppm, self.cfg.max_adjust_ppm);
        let scaled = i128::from(wall_dt_ns) * i128::from(PPM + rate) / i128::from(PPM);
        self.acc_ns = self
            .acc_ns
            .saturating_add(i64::try_from(scaled).unwrap_or(i64::MAX));
        let due = (self.acc_ns / TICK_NS) as u32;
        let ticks = due.min(self.cfg.max_catchup_ticks);
        self.acc_ns -= i64::from(ticks) * TICK_NS;
        let backlog = (self.acc_ns / TICK_NS) as u32;
        let mut rebase = None;
        if backlog > self.cfg.max_backlog_ticks {
            let dropped = backlog - self.cfg.max_backlog_ticks;
            self.acc_ns -= i64::from(dropped) * TICK_NS;
            self.stall_rebases += 1;
            self.dropped_ticks += u64::from(dropped);
            rebase = Some(Rebase::Stall {
                dropped_ticks: dropped,
            });
        }
        self.ticks += u64::from(ticks);
        Pace { ticks, rebase }
    }

    /// Whole ticks of wall time still owed.
    pub fn backlog_ticks(&self) -> u32 {
        (self.acc_ns / TICK_NS) as u32
    }

    /// Ticks released over the pacer's lifetime.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Stall rebases performed.
    pub fn stall_rebases(&self) -> u64 {
        self.stall_rebases
    }

    /// Sleep/resume rebases performed.
    pub fn sleep_rebases(&self) -> u64 {
        self.sleep_rebases
    }

    /// Whole ticks of wall time dropped by stall rebases.
    pub fn dropped_ticks(&self) -> u64 {
        self.dropped_ticks
    }
}

/// The seq → server-tick pairing, learned from acks.
///
/// The host consumes seq `s` for the step ending at server tick `n`, so
/// `n - s` is constant while consumption is steady and grows by one for each
/// starved tick that finalized nothing. The pairing is therefore *re-derived
/// from every ack* instead of assumed, and cleared by a rebase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeqMap {
    base: Option<i64>,
}

impl SeqMap {
    /// Record that the host finalized `seq` in the state at `server_tick`.
    /// `seq == 0` (nothing finalized) carries no pairing.
    pub fn note_ack(&mut self, seq: u64, server_tick: u64) {
        if seq > 0 {
            self.base = Some(server_tick as i64 - seq as i64);
        }
    }

    /// The server tick `seq` maps to, or `None` while unpaired.
    pub fn server_tick(&self, seq: u64) -> Option<u64> {
        let tick = self.base? + seq as i64;
        u64::try_from(tick).ok()
    }

    /// Whether a pairing exists.
    pub fn is_paired(&self) -> bool {
        self.base.is_some()
    }

    fn clear(&mut self) {
        self.base = None;
    }
}

/// Counters the tests and the HUD read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeadStats {
    /// Reports with depth 0 (the host had nothing queued after its tick).
    pub empty_reports: u64,
    /// Reports above [`LeadConfig::max_depth`].
    pub high_reports: u64,
    /// Stale-ack episodes entered.
    pub stale_expiries: u64,
    /// Rebases noted ([`LeadController::rebase`]).
    pub rebases: u64,
    /// Reports folded into the filter.
    pub reports: u64,
}

/// The feedback controller. See the module docs.
#[derive(Debug, Clone)]
pub struct LeadController {
    cfg: LeadConfig,
    /// Smoothed depth, 1/256 tick; `None` until the first report.
    depth_q8: Option<i64>,
    rate_ppm: i64,
    last_update_ns: Option<i64>,
    last_ack: u64,
    last_ack_ns: Option<i64>,
    stalled: bool,
    rtt_samples: VecDeque<i64>,
    rtt_uncertainty_ns: i64,
    seq_map: SeqMap,
    stats: LeadStats,
}

/// RTT samples kept for the minimum filter.
const RTT_WINDOW: usize = 8;

impl LeadController {
    /// A controller with no signal yet.
    pub fn new(cfg: LeadConfig) -> Self {
        Self {
            cfg,
            depth_q8: None,
            rate_ppm: 0,
            last_update_ns: None,
            last_ack: 0,
            last_ack_ns: None,
            stalled: false,
            rtt_samples: VecDeque::new(),
            rtt_uncertainty_ns: 0,
            seq_map: SeqMap::default(),
            stats: LeadStats::default(),
        }
    }

    /// Fold in one snapshot's report for the local seat: the host's buffer
    /// `depth`, the seat's `ack` and the snapshot's `server_tick`, received
    /// at monotonic `now_ns`.
    pub fn observe(&mut self, depth: u8, ack: u64, server_tick: u64, now_ns: i64) {
        self.stats.reports += 1;
        let sample = i64::from(depth) * 256;
        let reseed = self.stalled;
        if ack > self.last_ack || self.last_ack_ns.is_none() {
            self.last_ack = self.last_ack.max(ack);
            self.last_ack_ns = Some(now_ns);
            self.stalled = false;
        }
        self.seq_map.note_ack(ack, server_tick);
        self.depth_q8 = Some(match self.depth_q8 {
            Some(d) if !reseed => d + (sample - d) / 4,
            _ => sample,
        });
        if depth == 0 {
            self.stats.empty_reports += 1;
        }
        if u32::from(depth) > self.cfg.max_depth {
            self.stats.high_reports += 1;
        }
    }

    /// Fold in a round-trip measurement. `uncertainty_ns` is the measurement
    /// error bound (clock resolution, reporting interval); it widens the
    /// initial lead rather than being assumed away.
    pub fn observe_rtt(&mut self, rtt_ns: i64, uncertainty_ns: i64) {
        if self.rtt_samples.len() == RTT_WINDOW {
            self.rtt_samples.pop_front();
        }
        self.rtt_samples.push_back(rtt_ns.max(0));
        self.rtt_uncertainty_ns = uncertainty_ns.max(0);
    }

    /// Lead, in whole ticks, to start with before any depth report: half of
    /// the minimum RTT (an *initial* estimate only — it is wrong on an
    /// asymmetric path, which is why feedback takes over), four mean
    /// deviations of jitter, the measurement uncertainty, plus the target
    /// depth. Clamped to `[target_depth, target_depth + max_depth]`.
    pub fn initial_lead_ticks(&self) -> u32 {
        let target = self.cfg.target_depth;
        let Some(&min) = self.rtt_samples.iter().min() else {
            return target;
        };
        let n = self.rtt_samples.len() as i64;
        let mean = self.rtt_samples.iter().sum::<i64>() / n;
        let dev = self
            .rtt_samples
            .iter()
            .map(|s| (s - mean).abs())
            .sum::<i64>()
            / n;
        let one_way = min / 2 + 4 * dev + self.rtt_uncertainty_ns;
        let ticks = ((one_way + TICK_NS - 1) / TICK_NS) as u32;
        (ticks + target).clamp(target, target + self.cfg.max_depth)
    }

    /// Advance the controller to `now_ns` and return the rate adjustment in
    /// ppm for the next frames. Call once per frame.
    pub fn update(&mut self, now_ns: i64) -> i64 {
        let dt = self.last_update_ns.map_or(0, |t| (now_ns - t).max(0));
        self.last_update_ns = Some(now_ns);
        if let Some(at) = self.last_ack_ns
            && !self.stalled
            && now_ns - at > self.cfg.stale_ack_ns
        {
            self.stalled = true;
            self.stats.stale_expiries += 1;
        }
        let desired = if self.stalled { 0 } else { self.desired_ppm() };
        let max_step = self.cfg.slew_ppm_per_s * dt / 1_000_000_000;
        let step = (desired - self.rate_ppm).clamp(-max_step, max_step);
        self.rate_ppm += step;
        if self.stalled {
            // Nothing to steer on: stop at once, do not coast on the slew.
            self.rate_ppm = 0;
        }
        self.rate_ppm
    }

    fn desired_ppm(&self) -> i64 {
        let Some(depth) = self.depth_q8 else {
            return 0;
        };
        let cfg = &self.cfg;
        let error = i64::from(cfg.target_depth) * 256 - depth;
        let emergency =
            depth < i64::from(cfg.min_depth) * 256 || depth > i64::from(cfg.max_depth) * 256;
        let raw = if error.abs() <= cfg.deadband_q8 && !emergency {
            0
        } else if emergency {
            error.signum() * cfg.max_adjust_ppm
        } else {
            error * cfg.gain_ppm_per_tick / 256
        };
        raw.clamp(-cfg.max_adjust_ppm, cfg.max_adjust_ppm)
    }

    /// Wall time was dropped ([`Pace::rebase`]): the seq → server-tick
    /// pairing and the depth filter no longer describe the stream, so they
    /// are cleared and re-derived from the next report. The rate is zeroed.
    pub fn rebase(&mut self) {
        self.stats.rebases += 1;
        self.seq_map.clear();
        self.depth_q8 = None;
        self.rate_ppm = 0;
    }

    /// The current rate adjustment, ppm.
    pub fn rate_ppm(&self) -> i64 {
        self.rate_ppm
    }

    /// Smoothed host buffer depth in ticks (0.0 before the first report).
    pub fn depth_ticks(&self) -> f64 {
        self.depth_q8.map_or(0.0, |d| d as f64 / 256.0)
    }

    /// Whether the ack has gone stale.
    pub fn is_stalled(&self) -> bool {
        self.stalled
    }

    /// The seq → server-tick pairing.
    pub fn seq_map(&self) -> &SeqMap {
        &self.seq_map
    }

    /// Counters.
    pub fn stats(&self) -> LeadStats {
        self.stats
    }
}

/// Age, in client ticks, of the newest acked command: `predicted_now` minus
/// the predicted tick it was captured at. Read it from the history *before*
/// `CommandHistory::ack` trims the record.
pub fn confirmation_age(acked: &TickRecord, predicted_now: u64) -> u64 {
    predicted_now.saturating_sub(acked.predicted_tick)
}

/// Records the history must hold so replay can start from the oldest
/// unconfirmed command: confirmation age, plus the lead, plus one snapshot
/// interval (the next ack is that far away) and a jitter margin, plus one.
pub fn history_ticks(
    confirmation_age: u64,
    lead: u64,
    snapshot_interval: u64,
    jitter_margin: u64,
) -> u64 {
    confirmation_age
        .saturating_add(lead)
        .saturating_add(snapshot_interval)
        .saturating_add(jitter_margin)
        .saturating_add(1)
}

/// Whether `ticks` fit [`HISTORY_CAP`].
pub fn history_fits(ticks: u64) -> bool {
    ticks <= HISTORY_CAP as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    const MS: i64 = 1_000_000;

    /// Cap the host buffer holds (`mm2_net::BUFFER_CAP`).
    const HOST_CAP: usize = mm2_net::BUFFER_CAP;

    /// Snapshots reach the client every 33 ms (30 Hz).
    const SNAP_NS: i64 = 33 * MS;

    /// A 1 ms-step loopback model: client pacer + controller, a FIFO uplink
    /// with a settable one-way delay, a host that consumes one command per
    /// host tick, and a downlink carrying depth reports.
    struct Sim {
        cfg: LeadConfig,
        now: i64,
        pacer: Pacer,
        ctl: LeadController,
        up_delay: i64,
        down_delay: i64,
        /// Host clock error: the host's tick period is `TICK_NS * (1 + e)`.
        host_ppm: i64,
        /// Client wall clock error vs the host's, in ppm.
        client_ppm: i64,
        next_seq: u64,
        link: VecDeque<(i64, u64)>,
        host_queue: VecDeque<u64>,
        host_next_at: i64,
        host_tick: u64,
        host_ack: u64,
        reports: VecDeque<(i64, u8, u64, u64)>,
        next_report_at: i64,
        /// Frozen client: wall time passes, no frames run until then.
        client_off_until: i64,
        last_frame: i64,
        // Outcomes.
        underflow: u64,
        overflow: u64,
        max_depth: usize,
        depth_trace: Vec<(i64, usize)>,
        rebases: u64,
    }

    impl Sim {
        fn new(up_delay_ms: i64, down_delay_ms: i64) -> Self {
            let cfg = LeadConfig::default();
            Sim {
                cfg,
                now: 0,
                pacer: Pacer::new(cfg),
                ctl: LeadController::new(cfg),
                up_delay: up_delay_ms * MS,
                down_delay: down_delay_ms * MS,
                host_ppm: 0,
                client_ppm: 0,
                next_seq: 1,
                link: VecDeque::new(),
                host_queue: VecDeque::new(),
                host_next_at: 0,
                host_tick: 0,
                host_ack: 0,
                reports: VecDeque::new(),
                next_report_at: SNAP_NS,
                client_off_until: 0,
                last_frame: 0,
                underflow: 0,
                overflow: 0,
                max_depth: 0,
                depth_trace: Vec::new(),
                rebases: 0,
            }
        }

        /// Run to wall time `end` (ns) with 1 ms steps; the client renders a
        /// frame every `frame_ms`.
        fn run(&mut self, end: i64, frame_ms: i64) {
            while self.now < end {
                self.now += MS;
                let now = self.now;
                // Uplink delivery (FIFO: arrival never overtakes an earlier
                // packet).
                while self.link.front().is_some_and(|&(at, _)| at <= now) {
                    let (_, seq) = self.link.pop_front().unwrap();
                    if self.host_queue.len() == HOST_CAP {
                        self.overflow += 1;
                    } else {
                        self.host_queue.push_back(seq);
                    }
                }
                // Host ticks.
                while self.host_next_at <= now {
                    self.host_tick += 1;
                    match self.host_queue.pop_front() {
                        Some(seq) => self.host_ack = seq,
                        None => self.underflow += 1,
                    }
                    self.max_depth = self.max_depth.max(self.host_queue.len());
                    self.host_next_at += TICK_NS * (PPM + self.host_ppm) / PPM;
                }
                // Snapshots.
                if now >= self.next_report_at {
                    self.next_report_at += SNAP_NS;
                    self.reports.push_back((
                        now + self.down_delay,
                        self.host_queue.len().min(255) as u8,
                        self.host_ack,
                        self.host_tick,
                    ));
                }
                while self.reports.front().is_some_and(|r| r.0 <= now) {
                    let (_, depth, ack, tick) = self.reports.pop_front().unwrap();
                    self.ctl.observe(depth, ack, tick, now);
                }
                // Client frame.
                if now >= self.client_off_until && (now % (frame_ms * MS) == 0) {
                    let dt = (now - self.last_frame) * (PPM + self.client_ppm) / PPM;
                    self.last_frame = now;
                    let rate = self.ctl.update(now);
                    let pace = self.pacer.advance(dt, rate);
                    if pace.rebase.is_some() {
                        self.rebases += 1;
                        self.ctl.rebase();
                    }
                    for _ in 0..pace.ticks {
                        let seq = self.next_seq;
                        self.next_seq += 1;
                        let at = (now + self.up_delay).max(self.link.back().map_or(0, |&(a, _)| a));
                        self.link.push_back((at, seq));
                    }
                }
                if now % (100 * MS) == 0 {
                    self.depth_trace.push((now, self.host_queue.len()));
                }
            }
        }

        fn depth_range(&self, from: i64, to: i64) -> (usize, usize) {
            let v: Vec<usize> = self
                .depth_trace
                .iter()
                .filter(|(t, _)| *t >= from && *t < to)
                .map(|&(_, d)| d)
                .collect();
            (*v.iter().min().unwrap(), *v.iter().max().unwrap())
        }

        /// First time at/after `from` after which every sample until `to`
        /// stays within `[min_depth, max_depth]`.
        fn settle_time(&self, from: i64, to: i64) -> Option<i64> {
            let lo = self.cfg.min_depth as usize;
            let hi = self.cfg.max_depth as usize;
            let mut settled = None;
            for &(t, d) in self
                .depth_trace
                .iter()
                .filter(|(t, _)| *t >= from && *t < to)
            {
                if (lo..=hi).contains(&d) {
                    settled.get_or_insert(t);
                } else {
                    settled = None;
                }
            }
            settled
        }
    }

    #[test]
    fn startup_settles_to_the_target_within_bounds() {
        let mut sim = Sim::new(25, 25);
        sim.run(20_000 * MS / 1000 * 1000, 16);
        let settle = sim.settle_time(0, 20_000 * MS).expect("settles");
        assert!(settle <= 6_000 * MS, "settled at {settle} ns");
        let (lo, hi) = sim.depth_range(8_000 * MS, 20_000 * MS);
        assert!(lo >= 1 && hi <= 4, "steady depth {lo}..={hi}");
        assert_eq!(sim.overflow, 0);
        assert!(sim.underflow <= 6, "startup underflows {}", sim.underflow);
        assert_eq!(sim.rebases, 0);
    }

    #[test]
    fn delay_step_50_150_50_stays_bounded_and_settles() {
        let mut sim = Sim::new(25, 25);
        sim.run(10_000 * MS, 16);
        sim.up_delay = 75 * MS; // 50 ms RTT -> 150 ms RTT, uplink share +50 ms
        sim.down_delay = 75 * MS;
        let before = sim.underflow;
        sim.run(25_000 * MS, 16);
        let step_up_underflow = sim.underflow - before;
        assert!(
            sim.settle_time(10_000 * MS, 25_000 * MS)
                .is_some_and(|t| t <= 16_000 * MS),
            "no settle after the up step"
        );
        // Starvation lasts about the added delay (3 ticks) plus the ramp.
        assert!(
            step_up_underflow <= 12,
            "up-step underflows {step_up_underflow}"
        );
        sim.up_delay = 25 * MS;
        sim.down_delay = 25 * MS;
        sim.run(45_000 * MS, 16);
        assert_eq!(sim.overflow, 0, "the shrink burst overflowed the host");
        assert!(
            sim.max_depth < HOST_CAP,
            "depth {} hit the cap",
            sim.max_depth
        );
        let t = sim
            .settle_time(25_000 * MS, 45_000 * MS)
            .expect("settles after the down step");
        assert!(t <= 32_000 * MS, "down-step settle at {t} ns");
        let (lo, hi) = sim.depth_range(38_000 * MS, 45_000 * MS);
        assert!(lo >= 1 && hi <= 4, "final depth {lo}..={hi}");
    }

    #[test]
    fn asymmetric_paths_settle_the_same() {
        // 100 ms up, 5 ms down and the reverse: RTT/2 is wrong by 45 ms in
        // both; the depth feedback does not care.
        for (up, down) in [(100, 5), (5, 100)] {
            let mut sim = Sim::new(up, down);
            sim.run(20_000 * MS, 16);
            let (lo, hi) = sim.depth_range(10_000 * MS, 20_000 * MS);
            assert!(lo >= 1 && hi <= 4, "up {up} down {down}: depth {lo}..={hi}");
            assert_eq!(sim.overflow, 0);
        }
    }

    #[test]
    fn clock_drift_of_100_ppm_either_way_holds_the_depth() {
        for ppm in [100, -100] {
            let mut sim = Sim::new(25, 25);
            sim.host_ppm = ppm;
            sim.run(120_000 * MS, 16);
            let (lo, hi) = sim.depth_range(10_000 * MS, 120_000 * MS);
            assert!(lo >= 1 && hi <= 4, "{ppm} ppm: depth {lo}..={hi}");
            assert_eq!(sim.overflow, 0);
            assert!(
                sim.ctl.rate_ppm().abs() <= 20_000,
                "{ppm} ppm: rate {}",
                sim.ctl.rate_ppm()
            );
            assert_eq!(sim.rebases, 0);
        }
    }

    #[test]
    fn a_client_clock_that_runs_slow_is_corrected_by_the_pacer() {
        // -100 ppm client wall clock against the host: the same bound.
        let mut sim = Sim::new(25, 25);
        sim.client_ppm = -100;
        sim.run(120_000 * MS, 16);
        let (lo, hi) = sim.depth_range(10_000 * MS, 120_000 * MS);
        assert!(lo >= 1 && hi <= 4, "depth {lo}..={hi}");
    }

    #[test]
    fn stalls_of_100_to_250_ms_catch_up_in_whole_bounded_ticks() {
        for stall_ms in [100, 150, 250] {
            let mut sim = Sim::new(25, 25);
            sim.run(10_000 * MS, 16);
            sim.client_off_until = sim.now + stall_ms * MS;
            let ticks_before = sim.pacer.ticks();
            sim.run(30_000 * MS, 16);
            assert_eq!(sim.overflow, 0, "{stall_ms} ms stall overflowed");
            assert!(sim.max_depth < HOST_CAP);
            // The catch-up after the stall never released more than the
            // configured per-call bound (ticks released <= wall ticks owed).
            let owed = (20_000 + stall_ms) * MS / TICK_NS;
            let released = (sim.pacer.ticks() - ticks_before) as i64;
            assert!(
                released <= owed + 8,
                "{stall_ms} ms: released {released} of {owed}"
            );
            let t = sim
                .settle_time(10_000 * MS + stall_ms * MS, 30_000 * MS)
                .expect("settles after the stall");
            assert!(t <= 20_000 * MS, "{stall_ms} ms stall settled at {t} ns");
            let (lo, hi) = sim.depth_range(24_000 * MS, 30_000 * MS);
            assert!(lo >= 1 && hi <= 4, "{stall_ms} ms: final depth {lo}..={hi}");
            if stall_ms == 250 {
                assert!(sim.rebases >= 1, "a 250 ms stall exceeds the backlog bound");
            }
        }
    }

    #[test]
    fn sleep_and_resume_rebases_without_a_fast_forward() {
        let mut sim = Sim::new(25, 25);
        sim.run(10_000 * MS, 16);
        let ticks_before = sim.pacer.ticks();
        sim.client_off_until = sim.now + 30_000 * MS; // laptop lid
        sim.run(50_000 * MS, 16);
        assert!(sim.pacer.sleep_rebases() >= 1);
        assert!(!sim.ctl.seq_map().is_paired() || sim.ctl.stats().rebases >= 1);
        // 10 s of running after the lid opens: at most that many ticks, never
        // the 30 s backlog.
        let released = (sim.pacer.ticks() - ticks_before) as i64;
        assert!(
            released <= 10_100 * MS / TICK_NS,
            "fast-forwarded {released} ticks"
        );
        assert_eq!(sim.overflow, 0);
        let (lo, hi) = sim.depth_range(44_000 * MS, 50_000 * MS);
        assert!(lo >= 1 && hi <= 4, "post-sleep depth {lo}..={hi}");
    }

    #[test]
    fn pacer_releases_whole_ticks_and_never_exceeds_the_rate_bound() {
        let cfg = LeadConfig::default();
        let mut p = Pacer::new(cfg);
        // One second of 16 ms frames at the maximum speed-up: between 60 and
        // 60 * 1.04 ticks, and never a fractional step.
        let mut total = 0;
        for _ in 0..62 {
            total += p.advance(16 * MS, 1_000_000).ticks; // clamped to +4%
        }
        let expect = 62 * 16 * MS * 104 / 100 / TICK_NS;
        assert!((total as i64 - expect).abs() <= 1, "{total} vs {expect}");
        // Neutral rate: ticks match wall time.
        let mut p = Pacer::new(cfg);
        let total: u32 = (0..600).map(|_| p.advance(10 * MS, 0).ticks).sum();
        assert!((total as i64 - 360).abs() <= 1, "{total}");
    }

    #[test]
    fn stall_catch_up_is_bounded_per_call_and_drops_the_excess() {
        let cfg = LeadConfig::default();
        let mut p = Pacer::new(cfg);
        let pace = p.advance(250 * MS, 0); // 15 ticks owed
        assert_eq!(pace.ticks, cfg.max_catchup_ticks);
        assert_eq!(
            pace.rebase,
            Some(Rebase::Stall {
                dropped_ticks: (250 * MS / TICK_NS) as u32
                    - cfg.max_catchup_ticks
                    - cfg.max_backlog_ticks
            })
        );
        assert_eq!(p.backlog_ticks(), cfg.max_backlog_ticks);
        // The kept backlog drains over the next calls, bounded each time.
        let a = p.advance(0, 0);
        assert_eq!(a.ticks, cfg.max_catchup_ticks);
        assert_eq!(p.advance(0, 0).ticks, cfg.max_catchup_ticks);
        assert_eq!(p.advance(0, 0).ticks, 0);
        // 100 ms (6 ticks) fits: no rebase.
        let mut p = Pacer::new(cfg);
        assert_eq!(p.advance(100 * MS, 0).rebase, None);
        // Sleep drops everything.
        let sleep = p.advance(2_000 * MS, 0);
        assert_eq!((sleep.ticks, sleep.rebase), (0, Some(Rebase::Sleep)));
        assert_eq!(p.backlog_ticks(), 0);
    }

    #[test]
    fn stale_acks_hold_the_rate_and_recover_without_a_backlog() {
        let cfg = LeadConfig::default();
        let mut c = LeadController::new(cfg);
        // Starved reports push the rate up.
        let mut now = 0;
        for i in 0..40 {
            now += 33 * MS;
            c.observe(0, i, i + 3, now);
            c.update(now);
        }
        assert!(c.rate_ppm() > 0);
        // The ack stops advancing.
        for _ in 0..40 {
            now += 33 * MS;
            c.observe(0, 39, 42, now);
            c.update(now);
        }
        assert!(c.is_stalled());
        assert_eq!(c.rate_ppm(), 0);
        assert_eq!(c.stats().stale_expiries, 1);
        // The ack moves again with a healthy depth: recovery reseeds the
        // filter instead of averaging stale data, and the rate stays small.
        now += 33 * MS;
        c.observe(2, 41, 46, now);
        c.update(now);
        assert!(!c.is_stalled());
        assert!((c.depth_ticks() - 2.0).abs() < 0.01);
        assert_eq!(c.rate_ppm(), 0);
    }

    #[test]
    fn rate_is_bounded_and_slews() {
        let cfg = LeadConfig::default();
        let mut c = LeadController::new(cfg);
        let mut now = 0;
        let mut prev = 0;
        for i in 0..200 {
            now += 16 * MS;
            c.observe(0, i + 1, i + 3, now);
            let r = c.update(now);
            assert!(r.abs() <= cfg.max_adjust_ppm);
            assert!((r - prev).abs() <= cfg.slew_ppm_per_s * 16 / 1000 + 1);
            prev = r;
        }
        assert_eq!(prev, cfg.max_adjust_ppm);
    }

    #[test]
    fn deadband_holds_a_depth_near_the_target() {
        let cfg = LeadConfig::default();
        let mut c = LeadController::new(cfg);
        let mut now = 0;
        for i in 0..100 {
            now += 33 * MS;
            // Depth wobbles 2/3/2/2: inside the half-tick-ish dead band.
            c.observe(if i % 4 == 1 { 3 } else { 2 }, i + 1, i + 3, now);
            c.update(now);
        }
        assert_eq!(c.rate_ppm(), 0);
    }

    #[test]
    fn rtt_estimate_seeds_the_initial_lead_with_its_uncertainty() {
        let cfg = LeadConfig::default();
        let mut c = LeadController::new(cfg);
        assert_eq!(c.initial_lead_ticks(), cfg.target_depth);
        for rtt in [50, 52, 49, 51] {
            c.observe_rtt(rtt * MS, 0);
        }
        let base = c.initial_lead_ticks();
        assert!(base > cfg.target_depth);
        c.observe_rtt(50 * MS, 40 * MS);
        assert!(c.initial_lead_ticks() >= base + 2);
        // Never beyond its clamp.
        c.observe_rtt(10_000 * MS, 0);
        assert!(c.initial_lead_ticks() <= cfg.target_depth + cfg.max_depth);
    }

    #[test]
    fn seq_map_follows_acks_and_clears_on_rebase() {
        let mut c = LeadController::new(LeadConfig::default());
        assert_eq!(c.seq_map().server_tick(5), None);
        c.observe(2, 10, 12, 0);
        assert_eq!(c.seq_map().server_tick(10), Some(12));
        assert_eq!(c.seq_map().server_tick(14), Some(16));
        // Two starved host ticks: the same seq acks two ticks later.
        c.observe(2, 11, 15, 33 * MS);
        assert_eq!(c.seq_map().server_tick(11), Some(15));
        // ack 0 carries no pairing.
        c.observe(2, 0, 20, 66 * MS);
        assert_eq!(c.seq_map().server_tick(11), Some(15));
        c.rebase();
        assert_eq!(c.seq_map().server_tick(11), None);
        c.observe(2, 12, 16, 99 * MS);
        assert_eq!(c.seq_map().server_tick(12), Some(16));
    }

    #[test]
    fn history_sizing_covers_the_default_loop() {
        // 250 ms confirmation + 6 tick lead + 4 tick snapshot interval +
        // 10 ticks jitter fits the ring; an absurd lead does not.
        let need = history_ticks(15, 6, 4, 10);
        assert_eq!(need, 36);
        assert!(history_fits(need));
        assert!(!history_fits(history_ticks(200, 40, 10, 10)));
        assert_eq!(history_ticks(u64::MAX, 1, 1, 1), u64::MAX);
    }

    #[test]
    fn confirmation_age_reads_the_captured_tick() {
        let rec = TickRecord {
            seq: 9,
            predicted_tick: 100,
            epoch: 0,
            command: mm2_net::TickCommand {
                throttle: 0,
                brake: 0,
                steer: 0,
                handbrake: 0,
                auto_reverse: false,
                forced_gear: None,
            },
        };
        assert_eq!(confirmation_age(&rec, 112), 12);
        assert_eq!(confirmation_age(&rec, 90), 0);
    }
}
