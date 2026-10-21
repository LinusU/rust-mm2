//! The host's per-seat bounded command buffer (NETPLAY-02B,
//! `docs/netplay-timeline.md`).
//!
//! A [`SeatBuffer`] holds one remote seat's not-yet-consumed
//! [`TickCommand`]s keyed by their sequence number. The host consumes
//! **exactly one slot per seat per fixed tick** ([`SeatBuffer::consume`]):
//! the next sequence in order if it is present, otherwise an explicit
//! substitute. Consumption never runs more than once per tick, so a burst
//! of arrivals can never create physical time; it only deepens the buffer
//! (bounded by [`BUFFER_CAP`]).
//!
//! # Substitution (design choice, not an original-game rule)
//!
//! When the next sequence is absent the tick still advances:
//!
//! - **Gap** — a *later* sequence is buffered, so the missing one is lost
//!   (or arrives out of order too late). The slot is **finalized** with a
//!   substitute and the ack advances over it; a late arrival for that
//!   sequence is then dropped as old and can never be applied twice.
//! - **Starved** — nothing at or past the next sequence is buffered, so the
//!   host cannot tell lost from merely late. A substitute drives the tick
//!   but **no sequence is finalized** and the ack does not move: the
//!   command is applied in order whenever it does arrive, and a stalled
//!   peer cannot run the host's sequence cursor ahead of the client's.
//!
//! The substitute is the last real command held for [`HOLD_LAST_TICKS`]
//! consecutive substituted ticks, then neutral. Every consumed slot reports
//! which of the three ([`Source`]) it was, so a fallback is never mistaken
//! for the driver's own input.
//!
//! # Acknowledgement
//!
//! [`SeatBuffer::ack`] is the highest finalized sequence: consumed, or
//! finalized by the gap rule. A received-but-unconsumed command is never
//! acked. It is monotonic.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::time::{Duration, Instant};

use crate::proto::TickCommand;

/// Most unconsumed commands one seat may hold. 32 ticks is ~533 ms at the
/// 60 Hz fixed step — twice the widest window a packet can carry, so a
/// healthy jitter buffer never touches it. A designed bound.
pub const BUFFER_CAP: usize = 32;

/// How many consecutive substituted ticks repeat the last real command
/// before the substitute becomes neutral: 6 ticks = 100 ms of coasting on
/// the driver's last controls. A designed bound, not an original rule.
pub const HOLD_LAST_TICKS: u32 = 6;

/// Sustained per-seat budget in *commands* per second. A window is charged
/// by its length, so a client resending a full [`MAX_INPUT_WINDOW`]
/// (16) window every 60 Hz tick (960/s) has 2x headroom. The #1236 flood
/// protection, made window-size aware.
///
/// [`MAX_INPUT_WINDOW`]: crate::MAX_INPUT_WINDOW
pub const COMMAND_RATE_HZ: u32 = 1920;

/// Commands a seat may land back-to-back before [`COMMAND_RATE_HZ`] applies
/// (eight full windows — a stalled link's catch-up burst).
pub const COMMAND_BURST: u32 = 128;

const TOKEN: u64 = 1000;

/// Where a consumed slot's controls came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Source {
    /// The driver's own command for the next sequence.
    Command,
    /// The last real command repeated (bounded by [`HOLD_LAST_TICKS`]).
    HeldLast,
    /// Neutral controls: no command ever arrived or the hold ran out.
    Neutral,
}

/// One tick's consumption result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consumed {
    /// The controls to apply this step.
    pub command: TickCommand,
    /// Whether they are the driver's or a substitute.
    pub source: Source,
    /// The sequence this tick finalized, if any (`None` for a starved tick,
    /// and before the stream has started).
    pub finalized: Option<u64>,
}

/// Neutral controls (what a coasting car gets).
pub const NEUTRAL: TickCommand = TickCommand {
    throttle: 0,
    brake: 0,
    steer: 0,
    handbrake: 0,
    auto_reverse: false,
    forced_gear: None,
};

/// Diagnostic counters for one seat, over its buffer's lifetime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeatStats {
    /// New commands accepted into the buffer.
    pub received: u64,
    /// Commands ignored because that sequence was already buffered.
    pub duplicates: u64,
    /// Commands dropped because their sequence was already finalized.
    pub old: u64,
    /// Commands dropped for lying beyond [`BUFFER_CAP`] past the cursor.
    pub overflow: u64,
    /// Windows dropped for a generation other than the running one.
    pub wrong_generation: u64,
    /// Windows dropped for an older reset epoch than the buffer's.
    pub stale_epoch: u64,
    /// Windows dropped by the command-aware flood bound.
    pub rate_limited: u64,
    /// Slots finalized with the driver's own command.
    pub finalized: u64,
    /// Ticks driven by a substitute (gap or starved).
    pub fallback: u64,
    /// The subset of `fallback` where nothing newer was buffered.
    pub underflow: u64,
    /// The subset of `fallback` that finalized a lost sequence.
    pub gap_finalized: u64,
    /// Commands buffered right now.
    pub occupancy: usize,
    /// The most commands ever buffered at once.
    pub max_occupancy: usize,
    /// Age of the last consumed real command (arrival to consumption).
    pub last_age: Duration,
    /// The oldest age any consumed real command reached.
    pub max_age: Duration,
}

/// One seat's token bucket, in milli-tokens so the refill stays integer.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    milli_tokens: u64,
    at: Instant,
}

impl Bucket {
    fn full(at: Instant) -> Self {
        Bucket {
            milli_tokens: u64::from(COMMAND_BURST) * TOKEN,
            at,
        }
    }

    /// Refill to `now`, then take `n` tokens if all of them are there.
    fn take(&mut self, now: Instant, n: u64) -> bool {
        let micros = now.saturating_duration_since(self.at).as_micros();
        let refill = micros.saturating_mul(u128::from(COMMAND_RATE_HZ)) / 1000;
        let cap = u64::from(COMMAND_BURST) * TOKEN;
        self.milli_tokens = u64::try_from(u128::from(self.milli_tokens).saturating_add(refill))
            .unwrap_or(u64::MAX)
            .min(cap);
        self.at = self.at.max(now);
        let cost = n.saturating_mul(TOKEN);
        if self.milli_tokens >= cost {
            self.milli_tokens -= cost;
            true
        } else {
            false
        }
    }
}

/// `true` when reset epoch `new` is strictly ahead of `cur` on the wrapping
/// 8-bit counter (a forward distance of 1..=127).
fn epoch_ahead(new: u8, cur: u8) -> bool {
    matches!(new.wrapping_sub(cur), 1..=127)
}

/// One remote seat's bounded input buffer. See the module docs.
#[derive(Debug, Clone)]
pub struct SeatBuffer {
    epoch: Option<u8>,
    /// Next sequence to consume; `None` until the first command is accepted.
    next_seq: Option<u64>,
    pending: BTreeMap<u64, (TickCommand, Instant)>,
    last: Option<TickCommand>,
    held: u32,
    finalized: u64,
    bucket: Bucket,
    stats: SeatStats,
}

impl SeatBuffer {
    /// An empty buffer whose flood bucket starts full at `now`.
    pub fn new(now: Instant) -> Self {
        SeatBuffer {
            epoch: None,
            next_seq: None,
            pending: BTreeMap::new(),
            last: None,
            held: 0,
            finalized: 0,
            bucket: Bucket::full(now),
            stats: SeatStats::default(),
        }
    }

    /// The highest finalized sequence (`0` = none).
    pub fn ack(&self) -> u64 {
        self.finalized
    }

    /// Counters, with the live occupancy filled in.
    pub fn stats(&self) -> SeatStats {
        SeatStats {
            occupancy: self.pending.len(),
            ..self.stats
        }
    }

    /// Count a window dropped for a generation other than the running one.
    pub fn note_wrong_generation(&mut self) {
        self.stats.wrong_generation += 1;
    }

    /// Accept one window. `first_seq`/`commands` are already
    /// codec-validated. Returns nothing: every drop is counted.
    ///
    /// - The whole window is dropped if the flood bucket cannot pay its
    ///   length in commands.
    /// - A window under an older epoch is dropped; one under a newer epoch
    ///   restarts the stream (pending commands discarded, cursor re-adopted)
    ///   but keeps the ack, which only ever rises.
    /// - Per command: already finalized → old; already buffered →
    ///   duplicate; beyond [`BUFFER_CAP`] past the cursor → overflow.
    pub fn accept(&mut self, epoch: u8, first_seq: u64, commands: &[TickCommand], now: Instant) {
        if !self.bucket.take(now, commands.len() as u64) {
            self.stats.rate_limited += 1;
            return;
        }
        match self.epoch {
            None => self.epoch = Some(epoch),
            Some(cur) if cur == epoch => {}
            Some(cur) if epoch_ahead(epoch, cur) => {
                self.epoch = Some(epoch);
                self.pending.clear();
                self.next_seq = None;
                self.held = 0;
            }
            Some(_) => {
                self.stats.stale_epoch += 1;
                return;
            }
        }
        // The cursor starts at the first command the seat is seen to send
        // (commands before it was simulating were dropped by design), but
        // never behind what is already finalized.
        let cursor = *self
            .next_seq
            .get_or_insert_with(|| first_seq.max(self.finalized.saturating_add(1)));
        for (i, command) in commands.iter().enumerate() {
            // `first_seq + len` was checked not to overflow by the codec,
            // but the buffer does not rely on its caller.
            let Some(seq) = first_seq.checked_add(i as u64) else {
                break;
            };
            if seq < cursor || seq <= self.finalized {
                self.stats.old += 1;
            } else if seq - cursor >= BUFFER_CAP as u64 {
                self.stats.overflow += 1;
            } else if let Entry::Vacant(slot) = self.pending.entry(seq) {
                slot.insert((*command, now));
                self.stats.received += 1;
            } else {
                self.stats.duplicates += 1;
            }
        }
        self.stats.max_occupancy = self.stats.max_occupancy.max(self.pending.len());
    }

    /// Consume this tick's slot. Called exactly once per seat per fixed
    /// step, before the step runs.
    pub fn consume(&mut self, now: Instant) -> Consumed {
        let Some(cursor) = self.next_seq else {
            // The stream has not started: neutral, nothing finalized.
            return Consumed {
                command: NEUTRAL,
                source: Source::Neutral,
                finalized: None,
            };
        };
        if let Some((command, at)) = self.pending.remove(&cursor) {
            self.next_seq = Some(cursor.saturating_add(1));
            self.finalized = self.finalized.max(cursor);
            self.last = Some(command);
            self.held = 0;
            self.stats.finalized += 1;
            self.stats.last_age = now.saturating_duration_since(at);
            self.stats.max_age = self.stats.max_age.max(self.stats.last_age);
            return Consumed {
                command,
                source: Source::Command,
                finalized: Some(cursor),
            };
        }
        let (command, source) = match self.last {
            Some(last) if self.held < HOLD_LAST_TICKS => {
                self.held += 1;
                (last, Source::HeldLast)
            }
            _ => (NEUTRAL, Source::Neutral),
        };
        self.stats.fallback += 1;
        let gap = self.pending.range(cursor..).next().is_some();
        let finalized = if gap {
            self.next_seq = Some(cursor.saturating_add(1));
            self.finalized = self.finalized.max(cursor);
            self.stats.gap_finalized += 1;
            Some(cursor)
        } else {
            self.stats.underflow += 1;
            None
        };
        Consumed {
            command,
            source,
            finalized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(n: u8) -> TickCommand {
        TickCommand {
            throttle: n,
            ..NEUTRAL
        }
    }

    fn window(first: u64, n: usize) -> Vec<TickCommand> {
        (0..n)
            .map(|i| cmd((first as u8).wrapping_add(i as u8)))
            .collect()
    }

    fn buf() -> (SeatBuffer, Instant) {
        let t0 = Instant::now();
        (SeatBuffer::new(t0), t0)
    }

    #[test]
    fn consumes_one_in_order_and_acks_only_consumed() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 4), t);
        assert_eq!(b.ack(), 0, "received but unconsumed is never acked");
        for seq in 1..=4u64 {
            let c = b.consume(t);
            assert_eq!(c.source, Source::Command);
            assert_eq!(c.finalized, Some(seq));
            assert_eq!(c.command.throttle, seq as u8);
            assert_eq!(b.ack(), seq, "ack names the step that just consumed");
        }
        assert_eq!(b.stats().finalized, 4);
        assert_eq!(b.stats().occupancy, 0);
    }

    #[test]
    fn burst_never_consumes_more_than_one_per_call() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 16), t);
        b.accept(0, 17, &window(17, 16), t);
        assert_eq!(b.stats().occupancy, 32);
        b.consume(t);
        assert_eq!(b.ack(), 1);
        assert_eq!(b.stats().occupancy, 31);
    }

    #[test]
    fn duplicates_and_old_are_dropped_and_counted() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 3), t);
        b.accept(0, 1, &window(1, 3), t);
        assert_eq!(b.stats().duplicates, 3);
        assert_eq!(b.stats().received, 3);
        b.consume(t);
        b.consume(t);
        b.accept(0, 1, &window(1, 3), t);
        let s = b.stats();
        assert_eq!(s.old, 2, "seqs 1 and 2 are finalized");
        assert_eq!(s.duplicates, 4, "seq 3 is still buffered");
    }

    #[test]
    fn reorder_fills_gaps_by_seq() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 1), t);
        b.accept(0, 4, &window(4, 2), t);
        b.accept(0, 2, &window(2, 2), t);
        let order: Vec<u8> = (0..6).map(|_| b.consume(t).command.throttle).collect();
        assert_eq!(order, [1, 2, 3, 4, 5, 5]);
        // The first window a seat is seen to send sets the cursor: earlier
        // sequences were sent before the host was simulating it.
    }

    #[test]
    fn lost_command_is_finalized_with_hold_last_and_late_arrival_is_dropped() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 1), t);
        b.accept(0, 3, &window(3, 1), t);
        assert_eq!(b.consume(t).source, Source::Command);
        let gap = b.consume(t);
        assert_eq!(gap.source, Source::HeldLast);
        assert_eq!(gap.command.throttle, 1);
        assert_eq!(gap.finalized, Some(2));
        assert_eq!(b.ack(), 2);
        // Seq 2 shows up after being finalized: dropped, never applied.
        b.accept(0, 2, &window(2, 1), t);
        assert_eq!(b.stats().old, 1);
        let next = b.consume(t);
        assert_eq!((next.source, next.finalized), (Source::Command, Some(3)));
        assert_eq!(b.stats().gap_finalized, 1);
    }

    #[test]
    fn starved_seat_coasts_then_neutral_without_running_the_cursor_ahead() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 1), t);
        b.consume(t);
        for _ in 0..HOLD_LAST_TICKS {
            let c = b.consume(t);
            assert_eq!(c.source, Source::HeldLast);
            assert_eq!(c.finalized, None);
        }
        for _ in 0..100 {
            let c = b.consume(t);
            assert_eq!(c.source, Source::Neutral);
            assert_eq!(c.finalized, None);
        }
        assert_eq!(b.ack(), 1, "a stall does not ack");
        // The client resumes at seq 2: it is consumed in order, not dropped.
        b.accept(0, 2, &window(2, 1), t);
        let c = b.consume(t);
        assert_eq!((c.source, c.finalized), (Source::Command, Some(2)));
        let s = b.stats();
        assert_eq!(s.underflow, u64::from(HOLD_LAST_TICKS) + 100);
        assert_eq!(s.fallback, s.underflow);
    }

    #[test]
    fn hold_resets_after_a_real_command() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 1), t);
        b.consume(t);
        for _ in 0..HOLD_LAST_TICKS + 2 {
            b.consume(t);
        }
        b.accept(0, 2, &window(2, 1), t);
        b.consume(t);
        assert_eq!(b.consume(t).source, Source::HeldLast);
    }

    #[test]
    fn unstarted_stream_is_neutral_and_unacked() {
        let (mut b, t) = buf();
        let c = b.consume(t);
        assert_eq!((c.source, c.finalized, b.ack()), (Source::Neutral, None, 0));
        assert_eq!(b.stats().fallback, 0, "no stream yet is not a fallback");
    }

    #[test]
    fn overflow_is_bounded_and_counted() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 16), t);
        b.accept(0, 17, &window(17, 16), t);
        b.accept(0, 33, &window(33, 16), t);
        let s = b.stats();
        assert_eq!(s.occupancy, BUFFER_CAP);
        assert_eq!(s.overflow, 16);
        assert_eq!(s.max_occupancy, BUFFER_CAP);
    }

    #[test]
    fn far_future_seq_does_not_move_the_cursor() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 1), t);
        b.accept(0, 1_000_000, &window(0, 1), t);
        assert_eq!(b.stats().overflow, 1);
        assert_eq!(b.consume(t).finalized, Some(1));
    }

    #[test]
    fn epoch_newer_restarts_older_is_dropped_ack_stays_monotonic() {
        let (mut b, t) = buf();
        b.accept(7, 1, &window(1, 3), t);
        b.consume(t);
        b.consume(t);
        assert_eq!(b.ack(), 2);
        // Stale epoch: dropped, buffer untouched.
        b.accept(6, 10, &window(10, 2), t);
        assert_eq!(b.stats().stale_epoch, 1);
        assert_eq!(b.stats().occupancy, 1);
        // Newer epoch: pending seq 3 discarded, cursor re-adopted past the ack.
        b.accept(8, 2, &window(2, 3), t);
        assert_eq!(b.stats().old, 1, "seq 2 is already finalized");
        let c = b.consume(t);
        assert_eq!(c.finalized, Some(3));
        assert_eq!(c.command.throttle, 3);
        assert_eq!(b.ack(), 3);
    }

    #[test]
    fn epoch_wraps() {
        assert!(epoch_ahead(0, 255));
        assert!(epoch_ahead(5, 4));
        assert!(!epoch_ahead(4, 5));
        assert!(!epoch_ahead(4, 4));
        assert!(!epoch_ahead(4, 4u8.wrapping_add(128)));
    }

    #[test]
    fn sequence_overflow_is_contained() {
        let (mut b, t) = buf();
        b.accept(0, u64::MAX - 1, &window(0, 3), t);
        assert_eq!(b.stats().received, 2, "the wrapping third is refused");
        assert_eq!(b.consume(t).finalized, Some(u64::MAX - 1));
        assert_eq!(b.consume(t).finalized, Some(u64::MAX));
        assert_eq!(b.ack(), u64::MAX);
        // The cursor saturates; nothing underflows, panics or re-applies.
        let c = b.consume(t);
        assert_ne!(c.source, Source::Command);
        assert_eq!(b.ack(), u64::MAX);
    }

    #[test]
    fn flood_is_charged_in_commands_and_refills() {
        let (mut b, t) = buf();
        // Eight full windows exhaust the 128-command burst exactly.
        for k in 0..8u64 {
            b.accept(0, 1 + 16 * k, &window(1, 16), t);
        }
        assert_eq!(b.stats().rate_limited, 0);
        b.accept(0, 1, &window(1, 16), t);
        assert_eq!(b.stats().rate_limited, 1);
        // 10 ms refills 19.2 commands: one full window fits again.
        b.accept(0, 1, &window(1, 16), t + Duration::from_millis(10));
        assert_eq!(b.stats().rate_limited, 1);
        // A single-command packet costs one command, not a window.
        let mut small = SeatBuffer::new(t);
        for k in 0..u64::from(COMMAND_BURST) {
            small.accept(0, 1 + k, &window(1, 1), t);
        }
        assert_eq!(small.stats().rate_limited, 0);
        small.accept(0, 500, &window(500, 1), t);
        assert_eq!(small.stats().rate_limited, 1);
    }

    #[test]
    fn age_is_measured_from_arrival_to_consumption() {
        let (mut b, t) = buf();
        b.accept(0, 1, &window(1, 2), t);
        b.consume(t + Duration::from_millis(30));
        b.consume(t + Duration::from_millis(50));
        let s = b.stats();
        assert_eq!(s.last_age, Duration::from_millis(50));
        assert_eq!(s.max_age, Duration::from_millis(50));
    }
}
