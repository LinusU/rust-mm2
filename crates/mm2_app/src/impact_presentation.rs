//! The client's presentation ledger as a Bevy resource (NETPLAY-08A,
//! #1974; the pure state machine is [`mm2_game::PresentationLedger`]).
//!
//! Behind the [`crate::netplay_timeline::NetplayTimeline`] selector a
//! predicted client presents each own-seat impact exactly once, whichever
//! of its own sim (a *local* contact) and the authority's row arrives
//! first. [`ImpactPresentation::admit_local`] is called by every consumer
//! of the local [`ImpactEvent`] stream (sound, sparks, texel damage); the
//! answer for an event is decided once and remembered, so the consumers
//! agree whatever order they run in. [`ImpactPresentation::admit_row`] is
//! the authoritative half, called as wire rows drain.

use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;
use mm2_game::{
    Arrival, EventIdentity, ImpactEvent, ImpactId, ObjectId, OrdinalCounter, PresentationLedger,
    PresentationVerdict,
};
use mm2_net::SnapImpact;

/// Local verdicts remembered so every consumer of one event agrees.
const MEMO_CAPACITY: usize = 512;

/// Counters the tests and the HUD read.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PresentationCounts {
    /// Local contacts presented (speculative).
    pub local_presented: u64,
    /// Local contacts withheld because the authority's row already played.
    pub local_suppressed: u64,
    /// Rows presented (no local twin existed).
    pub row_presented: u64,
    /// Rows withheld because a local contact already played.
    pub row_suppressed: u64,
    /// Rows refused as belonging to a replaced reset epoch.
    pub row_stale: u64,
}

/// Own-seat impact presentation state. Disabled (every arrival is
/// presented, the legacy matcher in `netdrive` decides) unless
/// [`Self::sync`] enables it.
#[derive(Resource, Default)]
pub struct ImpactPresentation {
    enabled: bool,
    wire_generation: u64,
    generation: u64,
    /// Locally controlled seats: object → (wire seat, reset epoch).
    seats: HashMap<ObjectId, (u16, u8)>,
    ledger: PresentationLedger,
    ordinals: OrdinalCounter,
    memo: VecDeque<(ImpactId, bool)>,
    /// What has been decided so far.
    pub counts: PresentationCounts,
}

impl ImpactPresentation {
    /// Whether the ledger path is active.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The ledger, for inspection.
    pub fn ledger(&self) -> &PresentationLedger {
        &self.ledger
    }

    /// Refresh from the world: whether the staged path is selected, the
    /// session's two generation namespaces and the locally controlled
    /// seats with their present reset epochs. A new generation starts a
    /// fresh ledger; a seat whose epoch moved cancels its speculative
    /// entries of the replaced pose.
    pub fn sync(
        &mut self,
        enabled: bool,
        generation: u64,
        wire_generation: u64,
        seats: impl IntoIterator<Item = (ObjectId, u16, u8)>,
    ) {
        self.enabled = enabled;
        if !enabled {
            return;
        }
        if self.generation != generation || self.wire_generation != wire_generation {
            self.ledger.clear();
            self.memo.clear();
            self.seats.clear();
        }
        self.generation = generation;
        self.wire_generation = wire_generation;
        let mut next = HashMap::new();
        for (object, seat, epoch) in seats {
            if self
                .seats
                .get(&object)
                .is_some_and(|&(_, old)| old != epoch)
            {
                self.ledger.seat_reset(wire_generation, seat, epoch);
            }
            next.insert(object, (seat, epoch));
        }
        self.seats = next;
    }

    /// Whether the local `event` is to be presented. Events that touch
    /// no locally controlled seat always are; one that does is decided
    /// once against the ledger and remembered.
    pub fn admit_local(&mut self, event: &ImpactEvent) -> bool {
        if !self.enabled || event.generation != self.generation {
            return true;
        }
        if let Some(&(_, verdict)) = self.memo.iter().find(|(id, _)| *id == event.id) {
            return verdict;
        }
        let mut sides = 0;
        let mut present = false;
        for who in [event.participants.0, event.participants.1] {
            let Some(&(seat, epoch)) = self.seats.get(&who) else {
                continue;
            };
            sides += 1;
            let id = EventIdentity {
                generation: self.wire_generation,
                epoch,
                seat,
                tick: event.tick,
                ordinal: self.ordinals.next(seat, event.tick),
            };
            if self.ledger.arrive(Arrival::Local, id, event.point, epoch)
                == PresentationVerdict::Present
            {
                present = true;
            }
        }
        let verdict = sides == 0 || present;
        if sides > 0 {
            if verdict {
                self.counts.local_presented += 1;
            } else {
                self.counts.local_suppressed += 1;
            }
            if self.memo.len() >= MEMO_CAPACITY {
                self.memo.pop_front();
            }
            self.memo.push_back((event.id, verdict));
        }
        verdict
    }

    /// The replay is about to re-simulate a tick: restart the per-tick
    /// ordinal count so the re-detected contacts are numbered as they
    /// were the first time, and the ledger recognises them as repeats
    /// instead of presenting them again. (Replayed physics emits no
    /// packets, score or effects of its own; this is the one hook it
    /// needs.)
    pub fn begin_replay_tick(&mut self) {
        self.ordinals = OrdinalCounter::default();
    }

    /// The authority's verdict for a wire row on a locally controlled
    /// seat.
    pub fn admit_row(&mut self, row_generation: u64, row: &SnapImpact) -> PresentationVerdict {
        let epoch = self
            .seats
            .values()
            .find(|&&(seat, _)| seat == row.seat)
            .map_or(row.epoch, |&(_, e)| e);
        let id = EventIdentity {
            generation: row_generation,
            epoch: row.epoch,
            seat: row.seat,
            tick: row.tick,
            ordinal: row.ordinal,
        };
        let verdict = self.ledger.arrive(
            Arrival::Authoritative,
            id,
            Vec3::from_array(row.point),
            epoch,
        );
        match verdict {
            PresentationVerdict::Present => self.counts.row_presented += 1,
            PresentationVerdict::Suppress => self.counts.row_suppressed += 1,
            PresentationVerdict::Stale => self.counts.row_stale += 1,
        }
        verdict
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::SurfaceState;

    const fn oid(slot: u32) -> ObjectId {
        ObjectId {
            generation: 1,
            slot,
        }
    }

    const ME: ObjectId = oid(7);
    const OTHER: ObjectId = oid(8);

    fn presentation(epoch: u8) -> ImpactPresentation {
        let mut p = ImpactPresentation::default();
        p.sync(true, 1, 5, [(ME, 2, epoch)]);
        p
    }

    fn local(id: u64, tick: u64, ordinal_hint: f32) -> ImpactEvent {
        ImpactEvent {
            id: ImpactId(id),
            generation: 1,
            tick,
            participants: (ME, OTHER),
            point: Vec3::new(ordinal_hint, 0.0, 0.0),
            normal: Vec3::Y,
            severity: 5.0,
            surface: SurfaceState::default(),
        }
    }

    fn row(id: u64, tick: u64, ordinal: u16, epoch: u8, x: f32) -> SnapImpact {
        SnapImpact {
            seat: 2,
            id,
            tick,
            epoch,
            ordinal,
            point: [x, 0.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            severity: 5.0,
            audio_id: 0,
        }
    }

    /// Production identities, all four arrival orders: the effect count
    /// is what the consumers would spawn (one per `Present`).
    #[test]
    fn each_arrival_order_presents_one_hit_once() {
        // Local first, then the row.
        let mut p = presentation(0);
        assert!(p.admit_local(&local(1, 100, 0.0)));
        assert_eq!(
            p.admit_row(5, &row(90, 102, 0, 0, 0.1)),
            PresentationVerdict::Suppress
        );

        // Authoritative first, then the local twin.
        let mut p = presentation(0);
        assert_eq!(
            p.admit_row(5, &row(90, 102, 0, 0, 0.1)),
            PresentationVerdict::Present
        );
        assert!(!p.admit_local(&local(1, 100, 0.0)));
        // A second consumer asking about the same event agrees.
        assert!(!p.admit_local(&local(1, 100, 0.0)));

        // Replay first: the replayed tick re-detects the contact (a new
        // local ImpactId, same tick and ordinal), then the row arrives.
        let mut p = presentation(0);
        assert!(p.admit_local(&local(1, 100, 0.0)));
        p.begin_replay_tick();
        assert!(!p.admit_local(&local(2, 100, 0.0)));
        assert_eq!(
            p.admit_row(5, &row(90, 100, 0, 0, 0.0)),
            PresentationVerdict::Suppress
        );
        assert_eq!(p.ledger().len(), 1);

        // A delayed row of the epoch before a reset is dropped, not shown.
        let mut p = presentation(4);
        assert_eq!(
            p.admit_row(5, &row(90, 100, 0, 3, 0.0)),
            PresentationVerdict::Stale
        );
        assert!(p.ledger().is_empty());
    }

    #[test]
    fn two_nearby_distinct_impacts_both_present() {
        let mut p = presentation(0);
        assert!(p.admit_local(&local(1, 100, 0.0)));
        assert!(p.admit_local(&local(2, 112, 0.4)));
        assert_eq!(
            p.admit_row(5, &row(90, 100, 0, 0, 0.0)),
            PresentationVerdict::Suppress
        );
        assert_eq!(
            p.admit_row(5, &row(91, 112, 0, 0, 0.4)),
            PresentationVerdict::Suppress
        );
        // The authority saw a third hit the local sim did not: shown.
        assert_eq!(
            p.admit_row(5, &row(92, 130, 0, 0, 0.8)),
            PresentationVerdict::Present
        );
        assert_eq!(p.counts.local_presented, 2);
        assert_eq!(p.counts.row_suppressed, 2);
        assert_eq!(p.counts.row_presented, 1);
    }

    #[test]
    fn a_reset_cancels_a_speculative_contact_of_the_old_pose() {
        let mut p = presentation(0);
        assert!(p.admit_local(&local(1, 100, 0.0)));
        p.sync(true, 1, 5, [(ME, 2, 1)]);
        assert!(p.ledger().is_empty());
        assert_eq!(
            p.admit_row(5, &row(90, 105, 0, 1, 0.0)),
            PresentationVerdict::Present
        );
    }

    #[test]
    fn disabled_or_unrelated_events_always_present() {
        let mut off = ImpactPresentation::default();
        assert!(off.admit_local(&local(1, 100, 0.0)));
        let mut p = presentation(0);
        let mut elsewhere = local(1, 100, 0.0);
        elsewhere.participants = (OTHER, oid(9));
        assert!(p.admit_local(&elsewhere));
        assert!(p.ledger().is_empty());
    }
}
