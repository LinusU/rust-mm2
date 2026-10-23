//! Host-authored event identities and the bounded client presentation
//! ledger (NETPLAY-08A, #1974).
//!
//! A predicted client can learn of one physical impact twice: from its
//! own speculative sim (a local contact) and from the authority (a wire
//! row). Presenting both doubles the sparks and the sound; presenting
//! neither loses the hit. [`EventIdentity`] names an event by what the
//! authority decided — session generation, the seat's reset epoch, the
//! seat, the fixed tick and the ordinal among that seat's events of the
//! tick — and [`PresentationLedger`] turns the two arrivals into exactly
//! one presentation whichever comes first.
//!
//! The matching is deliberately not geometric. The authority's identity
//! is an exact key; the predicted sim's tick can trail or lead it by the
//! reconciliation error, so a speculative contact is *paired* with an
//! authoritative one on `(generation, epoch, seat)`, nearest tick, then
//! ordinal, then distance — and every pairing consumes both sides, so
//! two distinct impacts a metre apart are two presentations, not one.
//! The tolerance is a designed value, not an original-game rule.

use std::collections::VecDeque;

use bevy::prelude::*;

/// What the authority decided about one consequential event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventIdentity {
    /// Wire generation of the session the event belongs to.
    pub generation: u64,
    /// The seat's reset epoch when the event happened; an event of an
    /// older epoch than the seat's current one belongs to a pose the
    /// reset has since replaced.
    pub epoch: u8,
    /// Wire roster slot of the seat the event is presented on.
    pub seat: u16,
    /// Fixed-step session tick the event was recorded on.
    pub tick: u64,
    /// Position among this seat's events of `tick`, in the order the
    /// authority emitted them (strongest first).
    pub ordinal: u16,
}

/// Hands out per-`(seat, tick)` ordinals in emission order, so the host
/// and a replaying client number a tick's events the same way.
#[derive(Debug, Default)]
pub struct OrdinalCounter {
    tick: u64,
    /// `(seat, next ordinal)` for `tick`.
    seen: Vec<(u16, u16)>,
}

impl OrdinalCounter {
    /// The ordinal of the next event on `seat` at `tick`.
    pub fn next(&mut self, seat: u16, tick: u64) -> u16 {
        if tick != self.tick {
            self.tick = tick;
            self.seen.clear();
        }
        if let Some((_, n)) = self.seen.iter_mut().find(|(s, _)| *s == seat) {
            let ordinal = *n;
            *n = n.saturating_add(1);
            ordinal
        } else {
            self.seen.push((seat, 1));
            0
        }
    }
}

/// Which stream delivered an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrival {
    /// The predicted client's own sim (speculative until confirmed).
    Local,
    /// The authority's row.
    Authoritative,
}

/// What the presenter does with an arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Spawn the effects (damage texels, sound, sparks, shake).
    Present,
    /// The same event was already presented; spawn nothing.
    Suppress,
    /// The event belongs to a reset epoch that has been replaced.
    Stale,
}

/// Where an entry stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    /// Presented from the local sim, not yet confirmed by the authority.
    Speculative,
    /// Presented from the authority's row, no local twin has arrived.
    Confirmed,
    /// Both streams reported it; presented once.
    Paired,
}

/// One presented event: the identity each stream gave it, once it has.
#[derive(Debug, Clone, Copy)]
struct Entry {
    local: Option<EventIdentity>,
    authoritative: Option<EventIdentity>,
    point: Vec3,
}

impl Entry {
    fn state(&self) -> EntryState {
        match (self.local, self.authoritative) {
            (Some(_), Some(_)) => EntryState::Paired,
            (Some(_), None) => EntryState::Speculative,
            _ => EntryState::Confirmed,
        }
    }

    /// The identity either stream gave it (the authority's first).
    fn key(&self) -> EventIdentity {
        self.authoritative
            .or(self.local)
            .expect("one stream arrived")
    }

    fn side(&self, arrival: Arrival) -> Option<EventIdentity> {
        match arrival {
            Arrival::Local => self.local,
            Arrival::Authoritative => self.authoritative,
        }
    }
}

/// Entries the ledger holds before the oldest is forgotten.
pub const LEDGER_CAPACITY: usize = 256;

/// How far apart, in fixed ticks, a speculative contact and its
/// authoritative twin may be numbered. Designed: covers the prediction
/// lead plus reconciliation error on a bad link (half a second at 60 Hz).
pub const PAIR_TICK_TOLERANCE: u64 = 30;

/// Ticks an entry is kept after its own tick, relative to the newest
/// tick seen. Beyond it a twin can no longer arrive within tolerance.
pub const LEDGER_HORIZON_TICKS: u64 = 600;

/// Bounded record of the events a client has presented.
#[derive(Debug, Default)]
pub struct PresentationLedger {
    entries: VecDeque<Entry>,
    newest_tick: u64,
    /// Arrivals refused as a repeat of an identity already held.
    pub repeats: u64,
    /// Arrivals refused as belonging to a replaced epoch.
    pub stale: u64,
    /// Speculative entries that were cancelled by an epoch change.
    pub cancelled: u64,
    /// Entries forgotten to stay inside [`LEDGER_CAPACITY`].
    pub evicted: u64,
}

impl PresentationLedger {
    /// Entries currently held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the ledger holds nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The state of the entry holding exactly `id`, if any.
    pub fn state_of(&self, id: &EventIdentity) -> Option<EntryState> {
        self.entries
            .iter()
            .find(|e| e.local == Some(*id) || e.authoritative == Some(*id))
            .map(Entry::state)
    }

    /// Forget everything, e.g. when the generation changes.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.newest_tick = 0;
    }

    /// The seat reset: speculative entries of older epochs are cancelled
    /// (their presentation already happened, but the authority will
    /// never confirm a hit on a pose it replaced, so they must not pair
    /// with a later event). Entries of the new epoch stay.
    pub fn seat_reset(&mut self, generation: u64, seat: u16, epoch: u8) {
        let before = self.entries.len();
        self.entries.retain(|e| {
            let id = e.key();
            !(id.generation == generation
                && id.seat == seat
                && epoch_older(id.epoch, epoch)
                && e.state() == EntryState::Speculative)
        });
        self.cancelled += (before - self.entries.len()) as u64;
    }

    /// Record an arrival and decide its presentation. `current_epoch` is
    /// the receiving side's present epoch for `id.seat`.
    pub fn arrive(
        &mut self,
        arrival: Arrival,
        id: EventIdentity,
        point: Vec3,
        current_epoch: u8,
    ) -> Verdict {
        if epoch_older(id.epoch, current_epoch) {
            self.stale += 1;
            return Verdict::Stale;
        }
        self.newest_tick = self.newest_tick.max(id.tick);
        self.expire();
        // The same identity again: a replay of the same tick, or a
        // second copy of the same row.
        if self.entries.iter().any(|e| e.side(arrival) == Some(id)) {
            self.repeats += 1;
            return Verdict::Suppress;
        }
        let twin = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let other = e.key();
                e.side(arrival).is_none()
                    && other.generation == id.generation
                    && other.epoch == id.epoch
                    && other.seat == id.seat
                    && other.tick.abs_diff(id.tick) <= PAIR_TICK_TOLERANCE
            })
            .min_by(|(_, a), (_, b)| {
                let key = |e: &Entry| {
                    let other = e.key();
                    (
                        other.tick.abs_diff(id.tick),
                        other.ordinal.abs_diff(id.ordinal),
                    )
                };
                key(a)
                    .cmp(&key(b))
                    .then_with(|| a.point.distance(point).total_cmp(&b.point.distance(point)))
            })
            .map(|(i, _)| i);
        if let Some(i) = twin {
            match arrival {
                Arrival::Local => self.entries[i].local = Some(id),
                Arrival::Authoritative => self.entries[i].authoritative = Some(id),
            }
            return Verdict::Suppress;
        }
        let mut entry = Entry {
            local: None,
            authoritative: None,
            point,
        };
        match arrival {
            Arrival::Local => entry.local = Some(id),
            Arrival::Authoritative => entry.authoritative = Some(id),
        }
        self.push(entry);
        Verdict::Present
    }

    fn push(&mut self, entry: Entry) {
        if self.entries.len() >= LEDGER_CAPACITY {
            self.entries.pop_front();
            self.evicted += 1;
        }
        self.entries.push_back(entry);
    }

    fn expire(&mut self) {
        let floor = self.newest_tick.saturating_sub(LEDGER_HORIZON_TICKS);
        self.entries.retain(|e| e.key().tick >= floor);
    }
}

/// Whether epoch `a` is strictly older than `b` on the wrapping `u8`
/// counter (the half-range convention serial numbers use).
pub fn epoch_older(a: u8, b: u8) -> bool {
    a != b && b.wrapping_sub(a) < 128
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(seat: u16, tick: u64, ordinal: u16) -> EventIdentity {
        EventIdentity {
            generation: 1,
            epoch: 0,
            seat,
            tick,
            ordinal,
        }
    }

    const P: Vec3 = Vec3::new(10.0, 0.0, 5.0);

    #[test]
    fn local_first_presents_once_and_the_row_confirms() {
        let mut l = PresentationLedger::default();
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 0), P, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 103, 0), P, 0),
            Verdict::Suppress
        );
        assert_eq!(l.state_of(&id(2, 100, 0)), Some(EntryState::Paired));
    }

    #[test]
    fn authoritative_first_suppresses_the_late_local_twin() {
        let mut l = PresentationLedger::default();
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 100, 0), P, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 98, 0), P, 0),
            Verdict::Suppress
        );
        // A later local contact of the same seat is a new event.
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 200, 0), P, 0),
            Verdict::Present
        );
    }

    #[test]
    fn a_replayed_local_contact_is_not_presented_twice() {
        let mut l = PresentationLedger::default();
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 0), P, 0),
            Verdict::Present
        );
        // The reconcile replay walks tick 100 again and re-detects it.
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 0), P, 0),
            Verdict::Suppress
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 100, 0), P, 0),
            Verdict::Suppress
        );
        assert_eq!(l.len(), 1);
        // Replay after confirmation changes nothing either.
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 0), P, 0),
            Verdict::Suppress
        );
        assert_eq!(l.repeats, 2);
    }

    #[test]
    fn a_delayed_row_of_an_older_epoch_is_never_presented() {
        let mut l = PresentationLedger::default();
        let mut old = id(2, 100, 0);
        old.epoch = 3;
        assert_eq!(l.arrive(Arrival::Authoritative, old, P, 4), Verdict::Stale);
        assert!(l.is_empty());
        // And a speculative contact from before the reset is cancelled,
        // so it cannot absorb a post-reset row.
        let mut spec = id(2, 100, 0);
        spec.epoch = 3;
        assert_eq!(l.arrive(Arrival::Local, spec, P, 3), Verdict::Present);
        l.seat_reset(1, 2, 4);
        assert!(l.is_empty());
        assert_eq!(l.cancelled, 1);
        let mut fresh = id(2, 110, 0);
        fresh.epoch = 4;
        assert_eq!(
            l.arrive(Arrival::Authoritative, fresh, P, 4),
            Verdict::Present
        );
        // Wrapping counter: 255 is older than 0.
        assert!(epoch_older(255, 0) && !epoch_older(0, 255) && !epoch_older(7, 7));
    }

    #[test]
    fn two_nearby_distinct_impacts_are_both_presented_in_every_order() {
        let near = P + Vec3::new(0.5, 0.0, 0.0);
        // Local, local, then both rows: two presentations in total.
        let mut l = PresentationLedger::default();
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 0), P, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 112, 0), near, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 100, 0), P, 0),
            Verdict::Suppress
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 112, 0), near, 0),
            Verdict::Suppress
        );
        // Rows first, then the local twins.
        let mut l = PresentationLedger::default();
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 100, 0), P, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 112, 0), near, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 101, 0), P, 0),
            Verdict::Suppress
        );
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 113, 0), near, 0),
            Verdict::Suppress
        );
        // One local contact only: the second row is still presented.
        let mut l = PresentationLedger::default();
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 0), P, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 100, 0), P, 0),
            Verdict::Suppress
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 112, 0), near, 0),
            Verdict::Present
        );
    }

    #[test]
    fn two_impacts_on_one_tick_pair_by_ordinal() {
        let mut l = PresentationLedger::default();
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 0), P, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 1), P, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 100, 1), P, 0),
            Verdict::Suppress
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(2, 100, 0), P, 0),
            Verdict::Suppress
        );
        assert_eq!(l.state_of(&id(2, 100, 0)), Some(EntryState::Paired));
        assert_eq!(l.state_of(&id(2, 100, 1)), Some(EntryState::Paired));
    }

    #[test]
    fn other_seats_and_generations_never_pair() {
        let mut l = PresentationLedger::default();
        assert_eq!(
            l.arrive(Arrival::Local, id(2, 100, 0), P, 0),
            Verdict::Present
        );
        assert_eq!(
            l.arrive(Arrival::Authoritative, id(3, 100, 0), P, 0),
            Verdict::Present
        );
        let mut other = id(2, 100, 0);
        other.generation = 2;
        assert_eq!(
            l.arrive(Arrival::Authoritative, other, P, 0),
            Verdict::Present
        );
        // Past the tolerance a row is a new event.
        assert_eq!(
            l.arrive(
                Arrival::Authoritative,
                id(2, 100 + PAIR_TICK_TOLERANCE + 1, 0),
                P,
                0
            ),
            Verdict::Present
        );
    }

    #[test]
    fn the_ledger_stays_bounded_and_forgets_old_ticks() {
        let mut l = PresentationLedger::default();
        for i in 0..(LEDGER_CAPACITY as u64 * 2) {
            l.arrive(Arrival::Local, id(2, i * 100, 0), P, 0);
        }
        assert!(l.len() <= LEDGER_CAPACITY);
        let before = l.len();
        l.arrive(Arrival::Local, id(2, 1_000_000, 0), P, 0);
        assert!(l.len() <= before);
        assert_eq!(l.len(), 1, "everything older than the horizon expired");
    }

    #[test]
    fn ordinals_count_per_seat_and_tick_in_emission_order() {
        let mut c = OrdinalCounter::default();
        assert_eq!(c.next(2, 10), 0);
        assert_eq!(c.next(2, 10), 1);
        assert_eq!(c.next(3, 10), 0);
        assert_eq!(c.next(2, 10), 2);
        assert_eq!(c.next(2, 11), 0);
    }
}
