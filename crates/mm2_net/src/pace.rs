//! Send cadence, per-peer budgets and priorities for the datagram lane
//! (NETPLAY-06D). Pure and clock-injected so every rule is testable
//! without sockets; `rt.rs` applies it to each seat's outbound side.
//!
//! The rules (all *designed*, none recovered from the retail game):
//!
//! - **Cadence.** Snapshots are published at [`SNAPSHOT_HZ`] simulation
//!   ticks per second, derived from the fixed tick counter, never from
//!   the render `Update` rate ([`Cadence`]).
//! - **Priority.** [`Priority::Movement`] (the current pose of cars)
//!   always drains before [`Priority::Bulk`] (world catch-up) and is never
//!   held back by the budget; bulk is sent only while the peer's byte
//!   budget has credit.
//! - **Bounds.** Each priority has its own bounded queue and a maximum
//!   queue age: an entry older than the age is discarded unsent, because
//!   a newer snapshot has superseded it. A slow link therefore never
//!   accumulates seconds of obsolete state.
//! - **Budget.** A token bucket of [`PEER_BYTES_PER_SEC`] with a
//!   [`PEER_BURST_BYTES`] burst. Movement may overdraw it (to at most
//!   [`MAX_DEBT_BYTES`]) so that bulk, not movement, pays for congestion.
//!   The congestion *policy* (adapting the rate to the link) is NETPLAY-10.

use std::time::{Duration, Instant};

/// Default snapshot rate, in snapshots per second.
pub const SNAPSHOT_HZ: u32 = 30;
/// Highest configurable snapshot rate.
pub const MAX_SNAPSHOT_HZ: u32 = 120;
/// Datagrams the movement queue holds before it drops the oldest.
pub const MOVEMENT_QUEUE_CAP: usize = 32;
/// Datagrams the bulk queue holds before it drops the oldest.
pub const BULK_QUEUE_CAP: usize = 32;
/// A queued movement datagram older than this is obsolete (three
/// snapshots at the default rate).
pub const MOVEMENT_MAX_AGE: Duration = Duration::from_millis(100);
/// A queued bulk datagram older than this is obsolete.
pub const BULK_MAX_AGE: Duration = Duration::from_millis(250);
/// Sustained bytes per second one peer's lane may carry (1 Mbit/s).
pub const PEER_BYTES_PER_SEC: u32 = 128 * 1024;
/// The most credit a peer's budget accumulates.
pub const PEER_BURST_BYTES: u32 = 16 * 1024;
/// How far movement may overdraw a peer's budget.
pub const MAX_DEBT_BYTES: u32 = 32 * 1024;

/// What a queued datagram is worth when the link cannot carry it all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Priority {
    /// Current movement: sent first, never held by the budget.
    Movement,
    /// World catch-up: sent from spare budget, expires sooner or later.
    Bulk,
}

/// Which simulation ticks produce a snapshot at a configured rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cadence {
    hz: u32,
}

impl Default for Cadence {
    fn default() -> Self {
        Self { hz: SNAPSHOT_HZ }
    }
}

impl Cadence {
    /// A cadence of `hz` snapshots per second, clamped to
    /// `1..=`[`MAX_SNAPSHOT_HZ`].
    pub fn new(hz: u32) -> Self {
        Self {
            hz: hz.clamp(1, MAX_SNAPSHOT_HZ),
        }
    }

    /// No pacing: every call to `due` is true. For deterministic
    /// harnesses that step the host one update at a time; a shipped host
    /// uses [`Cadence::default`].
    pub fn unpaced() -> Self {
        Self { hz: 0 }
    }

    /// Whether this cadence defers nothing.
    pub fn is_unpaced(&self) -> bool {
        self.hz == 0
    }

    /// The effective snapshot rate (0: unpaced).
    pub fn hz(&self) -> u32 {
        self.hz
    }

    /// Simulation ticks between snapshots at `tick_hz` ticks per second
    /// (at least 1: a rate above the tick rate publishes every tick).
    pub fn interval_ticks(&self, tick_hz: u32) -> u64 {
        u64::from((tick_hz / self.hz.max(1)).max(1))
    }

    /// Whether the snapshot for `tick` is due, given the tick of the last
    /// one (`None`: none yet). A tick that moved backwards (a new session
    /// restarted the clock) is due.
    pub fn due(&self, tick: u64, last: Option<u64>, tick_hz: u32) -> bool {
        self.is_unpaced()
            || last.is_none_or(|l| tick < l || tick - l >= self.interval_ticks(tick_hz))
    }
}

/// A byte token bucket whose credit movement may overdraw.
#[derive(Debug, Clone)]
pub struct Budget {
    rate: u32,
    burst: u32,
    credit: i64,
    at: Instant,
    /// Bytes ever spent.
    spent: u64,
}

impl Budget {
    /// A full bucket at `now`.
    pub fn new(rate: u32, burst: u32, now: Instant) -> Self {
        Self {
            rate,
            burst,
            credit: i64::from(burst),
            at: now,
            spent: 0,
        }
    }

    fn refill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.at);
        self.at = self.at.max(now);
        let add = (dt.as_nanos() * u128::from(self.rate) / 1_000_000_000).min(u128::from(u32::MAX));
        self.credit = (self.credit + add as i64).min(i64::from(self.burst));
    }

    /// Spend `bytes` for bulk: only when the credit covers it.
    pub fn try_spend(&mut self, bytes: usize, now: Instant) -> bool {
        self.refill(now);
        let b = bytes as i64;
        if self.credit >= b {
            self.credit -= b;
            self.spent += bytes as u64;
            true
        } else {
            false
        }
    }

    /// Spend `bytes` for movement: always, down to [`MAX_DEBT_BYTES`] of
    /// debt (beyond which the debt stops growing and the bytes are still
    /// sent — movement is bounded by the cadence, not the budget).
    pub fn spend(&mut self, bytes: usize, now: Instant) {
        self.refill(now);
        self.credit = (self.credit - bytes as i64).max(-i64::from(MAX_DEBT_BYTES));
        self.spent += bytes as u64;
    }

    /// Credit now (negative: in debt).
    pub fn credit(&mut self, now: Instant) -> i64 {
        self.refill(now);
        self.credit
    }

    /// Bytes ever spent.
    pub fn spent(&self) -> u64 {
        self.spent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_is_tick_derived_and_clamped() {
        let c = Cadence::default();
        assert_eq!(c.interval_ticks(60), 2);
        assert_eq!(Cadence::new(0).hz(), 1);
        assert_eq!(Cadence::new(10_000).hz(), MAX_SNAPSHOT_HZ);
        assert_eq!(Cadence::new(120).interval_ticks(60), 1);
        // 600 render updates over 1 s of 60 Hz ticks still publish 30.
        let mut last = None;
        let mut n = 0;
        for update in 0..600u64 {
            let tick = update / 10;
            if c.due(tick, last, 60) {
                last = Some(tick);
                n += 1;
            }
        }
        assert_eq!(n, 30);
        assert!(c.due(0, Some(500), 60), "a restarted clock is due");
        assert!(!c.due(501, Some(500), 60));
        assert!(Cadence::unpaced().due(500, Some(500), 60));
    }

    #[test]
    fn budget_refills_caps_and_lets_movement_overdraw() {
        let t0 = Instant::now();
        let mut b = Budget::new(1000, 500, t0);
        assert!(b.try_spend(500, t0));
        assert!(!b.try_spend(1, t0), "empty");
        let t1 = t0 + Duration::from_millis(250);
        assert_eq!(b.credit(t1), 250);
        assert!(!b.try_spend(300, t1));
        assert!(b.try_spend(250, t1));
        // A long idle never banks more than the burst.
        assert_eq!(b.credit(t1 + Duration::from_secs(60)), 500);
        // Movement overdraws, bulk then waits for the debt to clear.
        let t2 = t1 + Duration::from_secs(60);
        b.spend(800, t2);
        assert_eq!(b.credit(t2), -300);
        assert!(!b.try_spend(1, t2));
        b.spend(1_000_000, t2);
        assert_eq!(b.credit(t2), -i64::from(MAX_DEBT_BYTES));
    }
}
