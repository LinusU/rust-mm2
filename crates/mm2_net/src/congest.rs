//! Congestion control for the datagram lane and the host's aggregate
//! uplink budget (NETPLAY-10B). Pure and clock-injected, like
//! [`crate::pace`]; `rt.rs` applies it to every seat's outbound side.
//!
//! The mechanism follows RFC 8085 (UDP usage guidelines) rather than
//! inventing one: the sender reduces its rate on congestion evidence, at
//! most once per hold interval (3.1), increases it only additively and only
//! while it actually uses the rate it has (3.1.6, 3.2), and never sends
//! below a floor that keeps the lane alive. All numbers are *designed*, not
//! recovered from the retail game.
//!
//! - [`PeerController`] — one per peer. Feedback is what the host can see
//!   without a receiver report: retransmissions of the reliable channel
//!   (loss), datagrams discarded unsent because they outlived their queue
//!   age (the link or the budget is slower than the offered load), the age
//!   of the oldest queued datagram, and the age of the newest
//!   acknowledgement. Its output is a byte rate and an [`Adapt`] level.
//! - [`Uplink`] — the host's whole uplink. It hands each peer a max-min
//!   fair share of the cap ([`Uplink::shares`]) and keeps a hard aggregate
//!   token bucket, both counted in *wire* bytes ([`wire_bytes`]: payload,
//!   lane header, UDP and IP).
//! - [`Adapt`] — how the traffic itself adapts as the rate falls: the
//!   snapshot cadence divisor, whether low-priority (bulk) traffic is sent,
//!   and the relevance radius scale. Congestion never queues older
//!   snapshots; it sends fewer, smaller and only the newest ones.
//!
//! See `docs/netcode-redesign.md`, "NETPLAY-10B".

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::pace::{Budget, PEER_BYTES_PER_SEC};
use crate::rt::HEADER_LEN;

/// IPv4 (20) plus UDP (8) header bytes added to every datagram on the wire.
pub const IP_UDP_OVERHEAD: usize = 28;
/// The default aggregate host uplink, in wire bytes per second (5 Mbit/s).
pub const HOST_UPLINK_BYTES_PER_SEC: u32 = 5_000_000 / 8;
/// The most credit the aggregate bucket banks (about 100 ms of uplink at
/// the default cap, so an idle second is not a burst).
pub const UPLINK_BURST_BYTES: u32 = 64 * 1024;
/// The lowest rate a peer's controller falls to: enough for ten snapshots
/// and the reliable channel's retransmissions per second.
pub const MIN_PEER_BYTES_PER_SEC: u32 = 12 * 1024;
/// The multiplicative decrease on congestion (rate x 0.7).
const DECREASE_NUM: u64 = 7;
const DECREASE_DEN: u64 = 10;
/// Congestion evidence reduces the rate at most this often.
pub const DECREASE_HOLD: Duration = Duration::from_millis(250);
/// A peer must be free of congestion evidence this long before the rate
/// rises one step.
pub const INCREASE_INTERVAL: Duration = Duration::from_millis(250);
/// Every decrease doubles that wait, up to this: a rate that was cut
/// right after probing upward is not probed again at once.
pub const MAX_INCREASE_INTERVAL: Duration = Duration::from_secs(4);
/// While the rate is below the last one that worked, the wait before a
/// step up is at most this, whatever the back-off: a short loss burst on
/// an adequate link (a join-time retransmit) must not keep bulk off for
/// seconds (#2126). The back-off still spaces the probes above it.
pub const MAX_RECOVERY_INTERVAL: Duration = Duration::from_millis(500);
/// A peer clean this long is trusted again and probes at the base
/// interval.
const TRUST_AFTER: Duration = Duration::from_secs(10);
/// Queue age at or above which a peer counts as congested: two snapshot
/// periods at the default cadence.
pub const CONGESTED_QUEUE_AGE: Duration = Duration::from_millis(66);
/// An acknowledgement older than this while datagrams are in flight counts
/// as congestion evidence.
pub const CONGESTED_ACK_AGE: Duration = Duration::from_millis(500);
/// Round-trip time above the lowest recently seen by more than this is a
/// standing queue somewhere on the path: congestion before any loss.
pub const RTT_SLACK: Duration = Duration::from_millis(50);
/// The slack grows by this many times the link's measured jitter: a path
/// whose samples spread over 200 ms (quarter-delay jitter on a 400 ms
/// link) puts its lowest recent sample well above its all-time lowest
/// with no queue (#2137).
const RTT_JITTER_FACTOR: u32 = 2;
/// Every round-trip sample over this long must stand above the baseline
/// before a standing queue counts: a scheduler stall on either end
/// inflates a few samples, and a real queue lifts all of them.
const STANDING_QUEUE_HOLD: Duration = Duration::from_millis(500);
/// Whether a standing queue in the round-trip time cuts the rate. Off:
/// the controller's verdict rests on loss, expiry, queue age and
/// passed-over snapshots. On a loaded shared-CPU runner a stall at either
/// end lifts every ack sample together, which the verdict cannot tell
/// from a queue, and it starved corrections in the net_contact cases
/// (#2049, #2137). The slack scales with measured jitter; enabling it waits
/// for the CI A/B in docs/netcode-redesign.md. The samples are still
/// taken and reported.
const RTT_VERDICT: bool = false;
/// The lowest-RTT baseline is re-taken this often, so a route change
/// that raises the floor is not read as congestion forever.
const RTT_BASELINE_WINDOW: Duration = Duration::from_secs(10);
/// The rate rises by this much per clean interval above the last good
/// rate (additive increase)...
pub const INCREASE_STEP: u32 = 4 * 1024;
/// ...and by a quarter of itself per interval while it is still below the
/// last rate that was known to work (recovery after a capacity dip).
const RECOVERY_NUM: u64 = 5;
const RECOVERY_DEN: u64 = 4;
/// The controller only raises a rate it uses: the bytes sent over a clean
/// interval must reach this share of it.
const USED_NUM: u64 = 1;
const USED_DEN: u64 = 2;

/// Bytes a datagram carrying `payload` bytes costs on the wire.
pub fn wire_bytes(payload: usize) -> usize {
    payload + HEADER_LEN + IP_UDP_OVERHEAD
}

/// How the traffic adapts to a peer's current rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adapt {
    /// Publish a snapshot to this peer every `snapshot_divisor`-th
    /// snapshot tick (1: full cadence).
    pub snapshot_divisor: u32,
    /// Whether low-priority (bulk) world catch-up is sent at all.
    pub bulk: bool,
    /// Percent of the full relevance radius whose entities are included
    /// (100: everything the cadence allows).
    pub relevance_percent: u32,
}

impl Default for Adapt {
    /// Full fidelity.
    fn default() -> Self {
        Self::for_rate(DEFAULT_PEER_CEILING, DEFAULT_PEER_CEILING)
    }
}

impl Adapt {
    /// The level a peer at `rate` bytes per second can sustain, against
    /// the lane's `ceiling`. Four steps, so a rate that wobbles around a
    /// threshold changes the traffic rarely:
    ///
    /// | rate vs ceiling | cadence | bulk | relevance |
    /// |---|---|---|---|
    /// | >= 1/2 | full | yes | 100% |
    /// | >= 1/4 | full | no | 100% |
    /// | >= 1/8 | 1/2 | no | 70% |
    /// | below | 1/3 | no | 40% |
    pub fn for_rate(rate: u32, ceiling: u32) -> Self {
        let r = u64::from(rate) * 8;
        let c = u64::from(ceiling.max(1));
        if r >= c * 4 {
            Self {
                snapshot_divisor: 1,
                bulk: true,
                relevance_percent: 100,
            }
        } else if r >= c * 2 {
            Self {
                snapshot_divisor: 1,
                bulk: false,
                relevance_percent: 100,
            }
        } else if r >= c {
            Self {
                snapshot_divisor: 2,
                bulk: false,
                relevance_percent: 70,
            }
        } else {
            Self {
                snapshot_divisor: 3,
                bulk: false,
                relevance_percent: 40,
            }
        }
    }
}

/// What the sender saw since it last reported; cumulative counters, the
/// controller takes the differences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Feedback {
    /// Reliable-channel retransmissions ever emitted (loss evidence).
    pub retransmits: u64,
    /// Datagrams ever discarded unsent for outliving their queue age.
    pub expired: u64,
    /// Wire bytes ever sent to the peer.
    pub wire_sent: u64,
    /// How long the oldest queued datagram has waited.
    pub queue_age: Duration,
    /// How long since the peer last acknowledged anything; `None` when
    /// nothing is in flight to be acknowledged.
    pub ack_age: Option<Duration>,
    /// The newest round-trip sample since the last report: how long ago
    /// the snapshot the peer just acknowledged was queued.
    pub rtt: Option<Duration>,
    /// Snapshots ever passed over: a newer snapshot was acknowledged
    /// while an older one never was (loss, or a stale one dropped).
    pub skipped: u64,
}

/// A peer's send-rate controller: AIMD on congestion evidence.
#[derive(Debug, Clone)]
pub struct PeerController {
    ceiling: u32,
    floor: u32,
    rate: u32,
    /// The rate before the last decrease: the level recovery climbs back
    /// to quickly.
    last_good: u32,
    last_decrease: Option<Instant>,
    clean_since: Instant,
    interval_start: Instant,
    interval_sent: u64,
    seen: Feedback,
    decreases: u64,
    min_rtt: Option<(Duration, Instant)>,
    srtt: Option<Duration>,
    /// Slow-moving mean deviation of the samples from `srtt`: the link's
    /// own jitter, which the standing-queue slack must clear.
    rtt_jitter: Duration,
    /// The round-trip samples of the last `STANDING_QUEUE_HOLD`.
    recent_rtts: VecDeque<(Instant, Duration)>,
    /// The clean wait before the next step up (backs off after a cut).
    increase_every: Duration,
    /// Whether `take_rtt`'s standing-queue verdict counts as congestion.
    rtt_verdict: bool,
}

impl PeerController {
    /// A controller at its ceiling at `now`.
    pub fn new(ceiling: u32, now: Instant) -> Self {
        let ceiling = ceiling.max(MIN_PEER_BYTES_PER_SEC);
        Self {
            ceiling,
            floor: MIN_PEER_BYTES_PER_SEC,
            rate: ceiling,
            last_good: ceiling,
            last_decrease: None,
            clean_since: now,
            interval_start: now,
            interval_sent: 0,
            seen: Feedback::default(),
            decreases: 0,
            min_rtt: None,
            srtt: None,
            rtt_jitter: Duration::ZERO,
            recent_rtts: VecDeque::new(),
            increase_every: INCREASE_INTERVAL,
            rtt_verdict: RTT_VERDICT,
        }
    }

    /// Let the RTT standing-queue verdict cut the rate (off by default).
    #[cfg(test)]
    fn with_rtt_verdict(mut self) -> Self {
        self.rtt_verdict = true;
        self
    }

    /// The current send rate in wire bytes per second.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The most the rate may be.
    pub fn ceiling(&self) -> u32 {
        self.ceiling
    }

    /// Times the rate was cut.
    pub fn decreases(&self) -> u64 {
        self.decreases
    }

    /// The smoothed round-trip time, once acknowledgements have flowed.
    pub fn srtt(&self) -> Option<Duration> {
        self.srtt
    }

    /// The traffic level this rate sustains.
    pub fn adapt(&self) -> Adapt {
        Adapt::for_rate(self.rate, self.ceiling)
    }

    /// Fold in a round-trip sample; whether the path shows a standing
    /// queue (every recent sample well above the baseline).
    fn take_rtt(&mut self, sample: Option<Duration>, now: Instant) -> bool {
        if let Some(r) = sample {
            self.recent_rtts.push_back((now, r));
            if let Some(s) = self.srtt {
                // Slow (1/64), so a step up from a queue is not absorbed
                // as jitter before the hold has run out.
                let dev = r.abs_diff(s);
                self.rtt_jitter = (self.rtt_jitter * 63 + dev) / 64;
            }
            self.srtt = Some(self.srtt.map_or(r, |s| (s * 7 + r) / 8));
            match self.min_rtt {
                Some((m, at))
                    if r >= m && now.saturating_duration_since(at) < RTT_BASELINE_WINDOW => {}
                _ => self.min_rtt = Some((r, now)),
            }
        }
        while let Some(&(at, _)) = self.recent_rtts.front() {
            if now.saturating_duration_since(at) <= STANDING_QUEUE_HOLD {
                break;
            }
            self.recent_rtts.pop_front();
        }
        // A queue lifts every sample; a stall or jitter lifts some. So the
        // lowest recent sample, not the smoothed mean, must stand above the
        // baseline, and the samples must span most of the hold.
        let (Some((m, _)), Some(&(oldest, _))) = (self.min_rtt, self.recent_rtts.front()) else {
            return false;
        };
        let lowest = self.recent_rtts.iter().map(|&(_, r)| r).min();
        now.saturating_duration_since(oldest) >= STANDING_QUEUE_HOLD * 3 / 4
            && lowest.is_some_and(|l| l > m + RTT_SLACK + self.rtt_jitter * RTT_JITTER_FACTOR)
    }

    /// Fold in the sender's counters at `now` and return the new rate.
    pub fn on_feedback(&mut self, fb: Feedback, now: Instant) -> u32 {
        let lost = fb.retransmits > self.seen.retransmits;
        // Expiry and queue age at the floor are the rate's own doing: the
        // queue is behind a budget the controller set, with nothing left to
        // cut. Counting them would hold `clean_since` back for ever and the
        // rate could never probe upward again; a probe that really fails is
        // cut back down by the next report.
        let at_floor = self.rate <= self.floor;
        let expired = fb.expired > self.seen.expired && !at_floor;
        let slow_queue = fb.queue_age >= CONGESTED_QUEUE_AGE && !at_floor;
        let silent = fb.ack_age.is_some_and(|a| a >= CONGESTED_ACK_AGE);
        let skipped = fb.skipped > self.seen.skipped;
        let standing_queue = self.take_rtt(fb.rtt, now);
        let delayed = self.rtt_verdict && standing_queue;
        self.interval_sent += fb.wire_sent.saturating_sub(self.seen.wire_sent);
        self.seen = fb;

        if lost || expired || slow_queue || silent || skipped || delayed {
            self.clean_since = now;
            let held = self
                .last_decrease
                .is_some_and(|t| now.saturating_duration_since(t) < DECREASE_HOLD);
            if !held {
                if self.rate > self.last_good || self.last_decrease.is_none() {
                    self.last_good = self.rate;
                }
                let cut = u64::from(self.rate) * DECREASE_NUM / DECREASE_DEN;
                self.rate = (cut as u32).max(self.floor);
                self.last_decrease = Some(now);
                self.decreases += 1;
                self.increase_every = (self.increase_every * 2).min(MAX_INCREASE_INTERVAL);
            }
            self.interval_start = now;
            self.interval_sent = 0;
            return self.rate;
        }

        let since = now.saturating_duration_since(self.interval_start);
        let clean = now.saturating_duration_since(self.clean_since);
        if clean >= TRUST_AFTER {
            self.increase_every = INCREASE_INTERVAL;
        }
        let every = if self.rate < self.last_good {
            self.increase_every.min(MAX_RECOVERY_INTERVAL)
        } else {
            self.increase_every
        };
        if since >= every && clean >= every {
            // A rate is only raised while it is in use (RFC 8085 3.1.6)
            // or while the traffic is shed to fit it: at a degraded level
            // the demand is below the rate by design, and only a higher
            // rate lets the level rise (a failed probe is cut again).
            let degraded = self.adapt().snapshot_divisor > 1 || !self.adapt().bulk;
            let used = degraded
                || self.interval_sent * 1_000_000_000
                    >= u64::from(self.rate) * since.as_nanos() as u64 * USED_NUM / USED_DEN;
            if used && self.rate < self.ceiling {
                let next = if self.rate < self.last_good {
                    (u64::from(self.rate) * RECOVERY_NUM / RECOVERY_DEN) as u32
                } else {
                    self.rate.saturating_add(INCREASE_STEP)
                };
                self.rate = next.min(self.ceiling);
                self.last_good = self.last_good.max(self.rate);
            }
            self.interval_start = now;
            self.interval_sent = 0;
        }
        self.rate
    }
}

/// A peer's claim on the uplink for [`Uplink::shares`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim {
    /// The seat.
    pub seat: u16,
    /// The most it would use: its controller's rate.
    pub rate: u32,
}

/// The host's aggregate uplink: a hard token bucket in wire bytes plus
/// max-min fair shares for the peers.
#[derive(Debug, Clone)]
pub struct Uplink {
    cap: u32,
    bucket: Budget,
}

impl Uplink {
    /// An uplink of `cap` wire bytes per second at `now` (at least
    /// [`MIN_PEER_BYTES_PER_SEC`]).
    pub fn new(cap: u32, now: Instant) -> Self {
        let cap = cap.max(MIN_PEER_BYTES_PER_SEC);
        Self {
            cap,
            bucket: Budget::new(cap, UPLINK_BURST_BYTES, now),
        }
    }

    /// The cap in wire bytes per second.
    pub fn cap(&self) -> u32 {
        self.cap
    }

    /// Change the cap (the host learned of, or configured, a different
    /// uplink). Banked credit is clamped to the new burst, so a cut takes
    /// effect at once; the lifetime byte count is kept.
    pub fn set_cap(&mut self, cap: u32, now: Instant) {
        let cap = cap.max(MIN_PEER_BYTES_PER_SEC);
        self.cap = cap;
        self.bucket.retune(cap, UPLINK_BURST_BYTES, now);
    }

    /// Whether `wire` more bytes fit now; spends them when they do.
    pub fn try_spend(&mut self, wire: usize, now: Instant) -> bool {
        self.bucket.try_spend(wire, now)
    }

    /// Wire bytes ever spent.
    pub fn spent(&self) -> u64 {
        self.bucket.spent()
    }

    /// Max-min fair division of the cap over `claims`: a peer asking for
    /// less than an equal share gets what it asks, and the remainder is
    /// split among the rest. The shares sum to at most the cap.
    pub fn shares(&self, claims: &[Claim]) -> Vec<(u16, u32)> {
        let mut order: Vec<usize> = (0..claims.len()).collect();
        order.sort_by_key(|&i| claims[i].rate);
        let mut left = u64::from(self.cap);
        let mut out = vec![(0u16, 0u32); claims.len()];
        for (k, &i) in order.iter().enumerate() {
            let equal = left / (claims.len() - k) as u64;
            let give = u64::from(claims[i].rate).min(equal);
            out[i] = (claims[i].seat, give as u32);
            left -= give;
        }
        out
    }
}

/// The rate a peer's token bucket refills at: the lower of its
/// controller's rate and its uplink share, never below the floor.
pub fn effective_rate(controller: u32, share: u32) -> u32 {
    controller.min(share).max(MIN_PEER_BYTES_PER_SEC)
}

/// The default per-peer ceiling (1 Mbit/s).
pub const DEFAULT_PEER_CEILING: u32 = PEER_BYTES_PER_SEC;

/// A sorted sample set for the p50/p95/p99 reports. Bounded: it keeps the
/// newest `cap` samples.
#[derive(Debug, Clone)]
pub struct Quantiles {
    samples: std::collections::VecDeque<u64>,
    cap: usize,
}

impl Quantiles {
    /// An empty set remembering at most `cap` samples (at least 1).
    pub fn new(cap: usize) -> Self {
        Self {
            samples: std::collections::VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Add a sample, forgetting the oldest when full.
    pub fn add(&mut self, v: u64) {
        if self.samples.len() >= self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back(v);
    }

    /// Samples held.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether nothing was added.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The `p`-th percentile (0..=100, nearest rank); 0 when empty.
    pub fn percentile(&self, p: u32) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut v: Vec<u64> = self.samples.iter().copied().collect();
        v.sort_unstable();
        let rank = (v.len() as u64 * u64::from(p.min(100)))
            .div_ceil(100)
            .max(1) as usize;
        v[rank - 1]
    }

    /// The largest sample; 0 when empty.
    pub fn max(&self) -> u64 {
        self.samples.iter().copied().max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(retx: u64, expired: u64, sent: u64, age_ms: u64) -> Feedback {
        Feedback {
            retransmits: retx,
            expired,
            wire_sent: sent,
            queue_age: Duration::from_millis(age_ms),
            ack_age: None,
            rtt: None,
            skipped: 0,
        }
    }

    #[test]
    fn wire_bytes_count_lane_header_and_ip_udp() {
        assert_eq!(wire_bytes(0), HEADER_LEN + 28);
        assert_eq!(wire_bytes(100), 100 + HEADER_LEN + 28);
    }

    #[test]
    fn congestion_cuts_once_per_hold_and_floors() {
        let t0 = Instant::now();
        let mut c = PeerController::new(100_000, t0);
        assert_eq!(c.on_feedback(fb(1, 0, 0, 0), t0), 70_000);
        // More evidence inside the hold is the same congestion event.
        let t1 = t0 + Duration::from_millis(100);
        assert_eq!(c.on_feedback(fb(2, 0, 0, 0), t1), 70_000);
        let t2 = t0 + DECREASE_HOLD;
        assert_eq!(c.on_feedback(fb(3, 0, 0, 0), t2), 49_000);
        let mut t = t2;
        for n in 4..60 {
            t += DECREASE_HOLD;
            c.on_feedback(fb(n, 0, 0, 0), t);
        }
        assert_eq!(c.rate(), MIN_PEER_BYTES_PER_SEC);
        assert_eq!(c.adapt().snapshot_divisor, 3);
    }

    #[test]
    fn rate_rises_only_when_clean_and_used_and_never_past_the_ceiling() {
        let t0 = Instant::now();
        let mut c = PeerController::new(100_000, t0);
        c.on_feedback(fb(1, 0, 0, 0), t0);
        assert_eq!(c.rate(), 70_000);
        // Clean but idle: the rate is not used, so it is not raised.
        let mut t = t0;
        let mut retx = 1;
        for _ in 0..8 {
            t += INCREASE_INTERVAL;
            c.on_feedback(fb(retx, 0, 0, 0), t);
        }
        assert_eq!(c.rate(), 70_000, "an unused rate is not probed upward");
        // Clean and used: it recovers towards the last good rate quickly,
        // then additively, and stops at the ceiling.
        let mut sent = 0u64;
        let mut prev = c.rate();
        for _ in 0..40 {
            t += INCREASE_INTERVAL;
            sent += u64::from(c.rate()) / 4 + 1;
            c.on_feedback(fb(retx, 0, sent, 0), t);
            assert!(c.rate() >= prev);
            prev = c.rate();
        }
        assert_eq!(c.rate(), 100_000);
        retx += 1;
        c.on_feedback(fb(retx, 0, sent, 0), t + INCREASE_INTERVAL);
        assert!(c.rate() < 100_000);
    }

    #[test]
    fn queue_age_and_expiry_and_a_silent_peer_are_congestion() {
        let t0 = Instant::now();
        let mut c = PeerController::new(100_000, t0);
        c.on_feedback(fb(0, 0, 0, 70), t0);
        assert_eq!(c.rate(), 70_000);
        let t1 = t0 + DECREASE_HOLD;
        c.on_feedback(fb(0, 1, 0, 0), t1);
        assert_eq!(c.rate(), 49_000);
        let t2 = t1 + DECREASE_HOLD;
        let silent = Feedback {
            ack_age: Some(CONGESTED_ACK_AGE),
            ..Feedback::default()
        };
        let silent = Feedback {
            expired: 1,
            ..silent
        };
        c.on_feedback(silent, t2);
        assert_eq!(c.rate(), 34_300);
    }

    #[test]
    fn a_jittery_round_trip_without_a_standing_queue_is_not_congestion() {
        let t0 = Instant::now();
        let mut c = PeerController::new(100_000, t0).with_rtt_verdict();
        let mut t = t0;
        // 180 ms +-45 ms, as a quarter-delay jitter on a 90 ms link gives.
        for i in 0..200u64 {
            t += Duration::from_millis(33);
            let ms = [135, 225, 160, 200, 180, 150][(i % 6) as usize];
            c.on_feedback(
                Feedback {
                    rtt: Some(Duration::from_millis(ms)),
                    ..Feedback::default()
                },
                t,
            );
        }
        assert_eq!(c.decreases(), 0, "jitter alone cut the rate");
    }

    #[test]
    fn a_link_whose_jitter_spans_two_hundred_ms_is_not_a_standing_queue() {
        let t0 = Instant::now();
        let mut c = PeerController::new(100_000, t0).with_rtt_verdict();
        let mut t = t0;
        // 800-1000 ms (a 400 ms link with quarter-delay jitter each way):
        // a 9-step pattern with a rare low sample and slow stretches.
        let pattern = [905, 840, 960, 925, 800, 975, 880, 935, 850];
        for i in 0..600usize {
            t += Duration::from_millis(33);
            // Slow stretches: a dozen samples all above the middle.
            let ms = if (i / 12) % 5 == 4 {
                900 + pattern[i % 9] % 100
            } else {
                pattern[i % 9]
            };
            c.on_feedback(
                Feedback {
                    rtt: Some(Duration::from_millis(ms)),
                    ..Feedback::default()
                },
                t,
            );
        }
        assert_eq!(c.decreases(), 0, "jitter alone cut the rate");
    }

    #[test]
    fn the_round_trip_verdict_is_off_by_default() {
        let t0 = Instant::now();
        let mut c = PeerController::new(100_000, t0);
        let mut t = t0;
        for ms in std::iter::repeat_n(40, 20).chain(std::iter::repeat_n(300, 80)) {
            t += Duration::from_millis(33);
            c.on_feedback(
                Feedback {
                    rtt: Some(Duration::from_millis(ms)),
                    ..Feedback::default()
                },
                t,
            );
        }
        assert_eq!(c.decreases(), 0, "RTT alone cut the rate");
        assert!(c.srtt().is_some(), "the samples are still reported");
    }

    /// Tests the verdict with it enabled; it is off in production.
    #[test]
    fn a_standing_queue_in_the_round_trip_time_is_congestion_before_loss() {
        let t0 = Instant::now();
        let mut c = PeerController::new(100_000, t0).with_rtt_verdict();
        let rtt = |ms| Feedback {
            rtt: Some(Duration::from_millis(ms)),
            ..Feedback::default()
        };
        let mut t = t0;
        for _ in 0..20 {
            t += Duration::from_millis(33);
            c.on_feedback(rtt(40), t);
        }
        assert_eq!(c.rate(), 100_000, "a steady RTT is not congestion");
        for _ in 0..80 {
            t += Duration::from_millis(33);
            c.on_feedback(rtt(300), t);
        }
        assert!(
            c.rate() < 50_000,
            "queueing delay cut the rate: {}",
            c.rate()
        );
        let skip = Feedback {
            skipped: 1,
            ..Feedback::default()
        };
        let before = c.decreases();
        t += DECREASE_HOLD;
        c.on_feedback(skip, t);
        assert!(c.decreases() > before, "a passed-over snapshot is loss");
    }

    #[test]
    fn a_rate_at_the_floor_climbs_again_though_its_own_queue_keeps_expiring() {
        let t0 = Instant::now();
        let mut c = PeerController::new(131_072, t0);
        let mut now = t0;
        let mut expired = 0;
        // Cut to the floor by genuine loss.
        for i in 1..=40u64 {
            now += Duration::from_millis(250);
            c.on_feedback(fb(i, 0, 0, 0), now);
        }
        assert_eq!(c.rate(), MIN_PEER_BYTES_PER_SEC);
        // The loss is over, but the throttled queue expires every report.
        let mut probed = false;
        for _ in 0..400 {
            now += Duration::from_millis(250);
            expired += 3;
            probed |= c.on_feedback(fb(40, expired, 0, 100), now) > MIN_PEER_BYTES_PER_SEC;
        }
        assert!(probed, "rate stuck at the floor");
    }

    #[test]
    fn adapt_steps_down_with_the_rate() {
        let a = |r| Adapt::for_rate(r, 128 * 1024);
        assert!(a(128 * 1024).bulk);
        assert!(!a(40 * 1024).bulk);
        assert_eq!(a(40 * 1024).snapshot_divisor, 1);
        assert_eq!(a(20 * 1024).snapshot_divisor, 2);
        assert_eq!(a(12 * 1024).snapshot_divisor, 3);
        assert!(a(12 * 1024).relevance_percent < a(20 * 1024).relevance_percent);
    }

    #[test]
    fn shares_are_max_min_fair_and_never_exceed_the_cap() {
        let up = Uplink::new(100_000, Instant::now());
        let claims = [
            Claim {
                seat: 1,
                rate: 10_000,
            },
            Claim {
                seat: 2,
                rate: 90_000,
            },
            Claim {
                seat: 3,
                rate: 90_000,
            },
            Claim {
                seat: 4,
                rate: 90_000,
            },
        ];
        let s = up.shares(&claims);
        assert_eq!(s[0], (1, 10_000), "a small ask is met in full");
        for (_, v) in &s[1..] {
            assert_eq!(*v, 30_000, "the rest split what is left equally");
        }
        assert!(s.iter().map(|(_, v)| u64::from(*v)).sum::<u64>() <= 100_000);
        let lone = up.shares(&claims[1..2]);
        assert_eq!(lone, vec![(2, 90_000)]);
        assert!(up.shares(&[]).is_empty());
    }

    #[test]
    fn the_aggregate_bucket_admits_no_more_than_the_cap_allows() {
        let t0 = Instant::now();
        let mut up = Uplink::new(100_000, t0);
        let mut sent = 0u64;
        let mut t = t0;
        for _ in 0..1000 {
            t += Duration::from_millis(1);
            while up.try_spend(1000, t) {
                sent += 1000;
            }
        }
        // One second at the cap plus the initial burst.
        assert!(sent <= 100_000 + u64::from(UPLINK_BURST_BYTES));
        assert!(sent >= 100_000 - 1000);
        assert_eq!(up.spent(), sent);
        up.set_cap(40_000, t);
        assert_eq!(up.cap(), 40_000);
        assert_eq!(up.spent(), sent, "the lifetime count survives a retune");
    }

    #[test]
    fn quantiles_use_nearest_rank_and_stay_bounded() {
        let mut q = Quantiles::new(100);
        assert_eq!(q.percentile(95), 0);
        for v in 1..=200u64 {
            q.add(v);
        }
        assert_eq!(q.len(), 100);
        assert_eq!(q.percentile(50), 150);
        assert_eq!(q.percentile(99), 199);
        assert_eq!(q.max(), 200);
    }
}
