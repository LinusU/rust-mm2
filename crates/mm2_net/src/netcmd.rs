//! The client's per-seat store of the host's applied commands for other
//! seats (NETPLAY-05A-1, `docs/netcode-redesign.md`).
//!
//! [`RemoteCommands`] keeps, per remote seat, the [`RemoteCommand`]s a
//! [`Message::RemoteCommands`] frame carried, keyed by the host's
//! authoritative tick. It changes no body: it only stores, so a later
//! prediction policy can read "what did the host apply to seat S at tick
//! T, and how old is the newest one".
//!
//! # Contract
//!
//! - **Received commands are never altered.** The first copy of a tick is
//!   kept; a later copy with the same content is a counted duplicate and
//!   one with different content is a counted conflict that is dropped,
//!   never merged over the original.
//! - **A command is consumed at most once.** [`RemoteCommands::consume`]
//!   hands out a tick only if it is present and past the seat's consume
//!   cursor, and advances the cursor; ticks at or behind it are gone.
//! - **Loss is covered by repetition**, not acks: every frame repeats the
//!   seat's last [`MAX_REMOTE_RUN`](crate::MAX_REMOTE_RUN) ticks, so a
//!   tick is lost only if every frame carrying it was.
//! - **Reset epochs**: a newer epoch (wrapping) discards the seat's
//!   buffered ticks and cursor; an older epoch is dropped.
//! - Bounded: a seat holds at most [`REMOTE_BUFFER_CAP`] ticks, the oldest
//!   evicted first, and at most [`MAX_REMOTE_SEATS`](crate::MAX_REMOTE_SEATS)
//!   seats are tracked.

use std::collections::BTreeMap;

use crate::MAX_REMOTE_SEATS;
use crate::proto::{Message, RemoteCommand, RemoteSeat};

/// Most ticks one remote seat holds: 64 ticks is ~1.07 s at 60 Hz, well
/// past any prediction horizon. A designed bound.
pub const REMOTE_BUFFER_CAP: usize = 64;

/// Counters for one remote seat over its store's lifetime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoteSeatStats {
    /// Ticks stored for the first time.
    pub stored: u64,
    /// Ticks received again with identical content.
    pub duplicates: u64,
    /// Ticks received again with different content (first copy kept).
    pub conflicts: u64,
    /// Ticks dropped because they were at or behind the consume cursor.
    pub consumed_or_old: u64,
    /// Runs dropped for an older reset epoch.
    pub stale_epoch: u64,
    /// Ticks evicted by [`REMOTE_BUFFER_CAP`].
    pub evicted: u64,
    /// Ticks consumed.
    pub consumed: u64,
}

#[derive(Debug)]
struct Seat {
    epoch: u8,
    ticks: BTreeMap<u64, RemoteCommand>,
    /// Highest tick consumed (0: none; ticks start at 1 on the host).
    cursor: u64,
    stats: RemoteSeatStats,
}

/// Wrapping "`a` is older than `b`" for the 8-bit reset epoch.
fn epoch_older(a: u8, b: u8) -> bool {
    a != b && b.wrapping_sub(a) < 128
}

/// Received remote commands, per seat, keyed by authoritative tick.
#[derive(Debug, Default)]
pub struct RemoteCommands {
    generation: Option<u64>,
    seats: BTreeMap<u16, Seat>,
    wrong_generation: u64,
    seat_overflow: u64,
}

impl RemoteCommands {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames dropped for a generation other than the running one.
    pub fn wrong_generation(&self) -> u64 {
        self.wrong_generation
    }

    /// Runs dropped because the seat ceiling was reached.
    pub fn seat_overflow(&self) -> u64 {
        self.seat_overflow
    }

    /// Start a session generation: every seat's state is discarded, and
    /// only frames for `generation` are absorbed from now on.
    pub fn set_generation(&mut self, generation: Option<u64>) {
        if self.generation != generation {
            self.generation = generation;
            self.seats.clear();
        }
    }

    /// Absorb a received frame; anything other than a
    /// [`Message::RemoteCommands`] is ignored. Returns `false` when the
    /// whole frame was dropped for its generation.
    pub fn absorb(&mut self, msg: &Message) -> bool {
        let Message::RemoteCommands {
            generation, seats, ..
        } = msg
        else {
            return true;
        };
        if self.generation != Some(*generation) {
            self.wrong_generation += 1;
            return false;
        }
        for run in seats {
            self.absorb_run(run);
        }
        true
    }

    fn absorb_run(&mut self, run: &RemoteSeat) {
        if !self.seats.contains_key(&run.player) && self.seats.len() >= MAX_REMOTE_SEATS {
            self.seat_overflow += 1;
            return;
        }
        let seat = self.seats.entry(run.player).or_insert_with(|| Seat {
            epoch: run.epoch,
            ticks: BTreeMap::new(),
            cursor: 0,
            stats: RemoteSeatStats::default(),
        });
        if epoch_older(run.epoch, seat.epoch) {
            seat.stats.stale_epoch += 1;
            return;
        }
        if run.epoch != seat.epoch {
            seat.epoch = run.epoch;
            seat.ticks.clear();
            seat.cursor = 0;
        }
        for (i, c) in run.commands.iter().enumerate() {
            let tick = run.first_tick + i as u64;
            if tick <= seat.cursor {
                seat.stats.consumed_or_old += 1;
                continue;
            }
            match seat.ticks.get(&tick) {
                Some(kept) if kept == c => seat.stats.duplicates += 1,
                Some(_) => seat.stats.conflicts += 1,
                None => {
                    seat.ticks.insert(tick, *c);
                    seat.stats.stored += 1;
                }
            }
        }
        while seat.ticks.len() > REMOTE_BUFFER_CAP {
            seat.ticks.pop_first();
            seat.stats.evicted += 1;
        }
    }

    /// The stored command for `player` at `tick`, without consuming it.
    pub fn get(&self, player: u16, tick: u64) -> Option<RemoteCommand> {
        self.seats.get(&player)?.ticks.get(&tick).copied()
    }

    /// Newest stored tick for `player`.
    pub fn newest_tick(&self, player: u16) -> Option<u64> {
        self.seats
            .get(&player)?
            .ticks
            .last_key_value()
            .map(|(t, _)| *t)
    }

    /// Age in ticks of `player`'s newest stored command at host tick
    /// `now` (`now - newest_tick`, saturating). `None` with nothing stored.
    pub fn age(&self, player: u16, now: u64) -> Option<u64> {
        Some(now.saturating_sub(self.newest_tick(player)?))
    }

    /// Consume `player`'s command for `tick`: `Some` only if it is stored
    /// and past the cursor; the cursor then moves to `tick`, so this tick
    /// (and every earlier one) can never be consumed again.
    pub fn consume(&mut self, player: u16, tick: u64) -> Option<RemoteCommand> {
        let seat = self.seats.get_mut(&player)?;
        if tick <= seat.cursor {
            return None;
        }
        let c = seat.ticks.remove(&tick)?;
        seat.cursor = tick;
        seat.stats.consumed += 1;
        // Anything older than the cursor can no longer be consumed.
        let keep = seat.ticks.split_off(&tick);
        seat.ticks = keep;
        Some(c)
    }

    /// `player`'s counters, if the seat is tracked.
    pub fn stats(&self, player: u16) -> Option<RemoteSeatStats> {
        self.seats.get(&player).map(|s| s.stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmdbuf::Source;
    use crate::proto::TickCommand;

    fn rc(n: u8) -> RemoteCommand {
        RemoteCommand {
            source: Source::Command,
            command: TickCommand {
                throttle: n,
                brake: 0,
                steer: 0,
                handbrake: 0,
                auto_reverse: false,
                forced_gear: None,
            },
        }
    }

    fn frame(epoch: u8, first: u64, vals: &[u8]) -> Message {
        let last = first + vals.len() as u64 - 1;
        Message::RemoteCommands {
            generation: 5,
            tick: last,
            seats: vec![RemoteSeat {
                player: 2,
                epoch,
                first_tick: first,
                commands: vals.iter().map(|v| rc(*v)).collect(),
            }],
        }
    }

    fn store() -> RemoteCommands {
        let mut s = RemoteCommands::new();
        s.set_generation(Some(5));
        s
    }

    #[test]
    fn overlapping_runs_store_each_tick_once_and_leave_the_first_copy() {
        let mut s = store();
        s.absorb(&frame(0, 1, &[1, 2, 3]));
        s.absorb(&frame(0, 2, &[2, 3, 4]));
        // A conflicting copy of tick 3 never replaces the original.
        s.absorb(&frame(0, 3, &[99]));
        assert_eq!(s.get(2, 3), Some(rc(3)));
        assert_eq!(s.get(2, 4), Some(rc(4)));
        let st = s.stats(2).unwrap();
        assert_eq!((st.stored, st.duplicates, st.conflicts), (4, 2, 1));
        assert_eq!(s.newest_tick(2), Some(4));
        assert_eq!(s.age(2, 9), Some(5));
    }

    #[test]
    fn a_tick_is_consumed_once_and_never_after_the_cursor_passed_it() {
        let mut s = store();
        s.absorb(&frame(0, 1, &[1, 2, 3]));
        assert_eq!(s.consume(2, 2), Some(rc(2)));
        assert_eq!(s.consume(2, 2), None);
        assert_eq!(s.consume(2, 1), None, "behind the cursor");
        // A repeat of consumed ticks is dropped, not re-stored.
        s.absorb(&frame(0, 1, &[1, 2, 3]));
        assert_eq!(s.consume(2, 2), None);
        assert_eq!(s.consume(2, 3), Some(rc(3)));
        assert_eq!(s.stats(2).unwrap().consumed, 2);
    }

    #[test]
    fn a_lost_frame_is_covered_by_the_next_redundant_run() {
        let mut s = store();
        s.absorb(&frame(0, 1, &[1, 2, 3, 4]));
        // The frame for ticks 5..=8 is lost; the next repeats 5..=12's tail.
        s.absorb(&frame(0, 5, &[5, 6, 7, 8, 9, 10, 11, 12]));
        for t in 1..=12 {
            assert_eq!(s.consume(2, t), Some(rc(t as u8)), "tick {t}");
        }
    }

    #[test]
    fn epochs_reset_the_seat_and_stale_epochs_are_dropped() {
        let mut s = store();
        s.absorb(&frame(0, 1, &[1, 2]));
        s.consume(2, 1);
        s.absorb(&frame(1, 1, &[7]));
        assert_eq!(s.get(2, 2), None, "old epoch's ticks are gone");
        assert_eq!(s.consume(2, 1), Some(rc(7)), "cursor restarted");
        s.absorb(&frame(0, 5, &[9]));
        assert_eq!(s.get(2, 5), None);
        assert_eq!(s.stats(2).unwrap().stale_epoch, 1);
        // Wrap: 255 -> 0 is newer.
        let mut s = store();
        s.absorb(&frame(255, 1, &[1]));
        s.absorb(&frame(0, 1, &[2]));
        assert_eq!(s.get(2, 1), Some(rc(2)));
    }

    #[test]
    fn other_generations_are_dropped_and_the_buffer_is_bounded() {
        let mut s = store();
        let mut other = frame(0, 1, &[1]);
        if let Message::RemoteCommands { generation, .. } = &mut other {
            *generation = 6;
        }
        assert!(!s.absorb(&other));
        assert_eq!(s.wrong_generation(), 1);
        assert_eq!(s.get(2, 1), None);
        let mut t = 1;
        while t <= 3 * REMOTE_BUFFER_CAP as u64 {
            s.absorb(&frame(0, t, &[1; 16]));
            t += 16;
        }
        assert!(s.stats(2).unwrap().evicted > 0);
        assert_eq!(s.get(2, 1), None);
        assert!(s.get(2, 3 * REMOTE_BUFFER_CAP as u64).is_some());
    }
}
