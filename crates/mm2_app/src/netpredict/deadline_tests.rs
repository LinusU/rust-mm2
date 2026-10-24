//! NETPLAY-05B-4: the fixed recovery deadline of [`RECOVERY_DEADLINE_TICKS`].
//!
//! Three cars on the production step: car 0 is the client's own, cars 1 and
//! 2 are remote contact bodies. Each test records the client's world every
//! tick and hands [`reconcile`] host snapshots built from that record with
//! a chosen remote body pushed off by half a metre, so a contact body
//! disagrees with the host. What is asserted is when the deadline starts
//! (the first usable *complete* state, never the contact itself and never a
//! partial or skipped snapshot) and when the named recovery fires.
//!
//! Evidence level: synthetic integration on the production step. No
//! sockets and no retail data; the over-the-wire assertion belongs to
//! `tests/net_contact.rs` once the client sends on the datagram lane.

use super::contact_tests::{EPOCH, GEN, PLAYER, input, q, spawn_car, world};
use super::*;

/// Ticks between the compared state and the newest command, kept shallow
/// so a replay is cheap and never `TooDeep`.
const LAG: u64 = 2;
const OFFSET: f32 = 0.5;

struct Run {
    app: App,
    pred: OwnPrediction,
    cars: [Entity; 3],
    t: u64,
    /// Added to every remote body's host velocity and angular velocity.
    noise: Vec3,
}

impl Run {
    fn new() -> Self {
        let mut app = world();
        let side = Vec3::X * 6.0;
        let cars = [
            spawn_car(&mut app, Vec3::Y * 1.2, false),
            spawn_car(&mut app, side + Vec3::Y * 1.2, false),
            spawn_car(&mut app, -side + Vec3::Y * 1.2, false),
        ];
        app.update();
        let mut pred = OwnPrediction::default();
        pred.restart(GEN);
        Self {
            app,
            pred,
            cars,
            t: 0,
            noise: Vec3::ZERO,
        }
    }

    fn cmd() -> VehicleInput {
        input(&q(0.3, 0.0, 0.0))
    }

    /// One client tick: drive every car, step, record under `seq == t`.
    fn tick(&mut self) {
        self.t += 1;
        for c in self.cars {
            *self.app.world_mut().get_mut::<VehicleInput>(c).unwrap() = Self::cmd();
        }
        self.app.update();
        let cap = capture_world(self.app.world_mut(), EPOCH);
        self.pred.record(self.t, cap);
    }

    fn ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.tick();
        }
    }

    /// A host snapshot of the state at `ack` (the host tick is `ack`) with
    /// the remote bodies in `pushed` off by [`OFFSET`] and those in
    /// `missing` absent from the snapshot.
    fn snapshot(&mut self, ack: u64, pushed: &[usize], missing: &[usize]) -> Outcome {
        let past = self.pred.ring.get(ack).expect("recorded").clone();
        let state = |e| own_body(&past, e).expect("in the capture");
        let (pos, rot, vel, ang) = state(self.cars[0]);
        let own = OwnAssertion {
            generation: GEN,
            server_tick: ack,
            ack,
            pos,
            rot,
            vel,
            ang,
        };
        let newest = self.t;
        let commands: Vec<(u64, VehicleInput)> =
            (ack + 1..=newest).map(|k| (k, Self::cmd())).collect();
        let remotes: Vec<RemoteDrive> = (1..3)
            .map(|i| {
                let (mut pos, rot, vel, ang) = state(self.cars[i]);
                if pushed.contains(&i) {
                    pos.x += OFFSET;
                }
                RemoteDrive {
                    body: self.cars[i],
                    assertion: (!missing.contains(&i)).then_some(RemoteAssertion {
                        wire: PLAYER + i as u16,
                        epoch: 0,
                        pos,
                        rot,
                        vel: vel + self.noise,
                        ang: ang + self.noise,
                    }),
                    commands: Some((ack + 1..=newest).map(|_| Self::cmd()).collect()),
                }
            })
            .collect();
        reconcile(
            self.app.world_mut(),
            &mut self.pred,
            &Request {
                own: self.cars[0],
                assertion: &own,
                newest,
                commands: &commands,
                remotes: &remotes,
                trailers: &[],
                epoch: EPOCH,
            },
        )
    }
}

fn replayed(o: Outcome) -> bool {
    matches!(o, Outcome::Replayed { .. })
}

/// A contact body that keeps disagreeing is replayed until exactly
/// [`RECOVERY_DEADLINE_TICKS`] host ticks after the first complete state,
/// then the host state is adopted by name, replayed forward so the own car
/// keeps its lead.
#[test]
fn a_persistent_divergence_recovers_exactly_at_the_deadline() {
    let mut r = Run::new();
    let first = 10;
    r.ticks(first + LAG);
    assert!(replayed(r.snapshot(first, &[1], &[])));
    assert_eq!(r.pred.deadline_start(), Some(first));
    for ack in first + 1..first + RECOVERY_DEADLINE_TICKS {
        r.tick();
        let out = r.snapshot(ack, &[1], &[]);
        assert!(replayed(out), "ack {ack}: {out:?}");
    }
    r.tick();
    let ack = first + RECOVERY_DEADLINE_TICKS;
    assert_eq!(
        r.snapshot(ack, &[1], &[]),
        Outcome::Recovered(RecoveryCause::DeadlineExceeded)
    );
    assert_eq!(r.pred.recovered_deadline, 1);
    assert_eq!(r.pred.deadline_start(), None);
    // The lead survives: the history still reaches the newest tick, and the
    // car was not thrown back to the host's state at the ack.
    assert_eq!(r.pred.ring.newest(), Some(r.t));
}

/// The clock counts from the first usable complete state, not from the
/// contact: snapshots that never arrive (the contact and everything up to
/// ack 40 lost) delay the start instead of consuming the budget.
#[test]
fn late_snapshots_start_the_deadline_at_the_first_usable_state() {
    let mut r = Run::new();
    r.ticks(40 + LAG);
    // Diverged since tick 1, first heard of at host tick 40.
    assert!(replayed(r.snapshot(40, &[1], &[])));
    assert_eq!(r.pred.deadline_start(), Some(40));
    r.ticks(RECOVERY_DEADLINE_TICKS - 1);
    let out = r.snapshot(40 + RECOVERY_DEADLINE_TICKS - 1, &[1], &[]);
    assert!(replayed(out), "{out:?}");
    r.tick();
    assert_eq!(
        r.snapshot(40 + RECOVERY_DEADLINE_TICKS, &[1], &[]),
        Outcome::Recovered(RecoveryCause::DeadlineExceeded)
    );
}

/// A snapshot missing a contact body is not a complete state: it neither
/// starts the clock nor, once it is running, restarts or stops it.
#[test]
fn an_incomplete_snapshot_does_not_start_the_deadline() {
    let mut r = Run::new();
    r.ticks(30 + LAG);
    // Car 1 diverges but car 2's row is missing.
    assert!(replayed(r.snapshot(10, &[1], &[2])));
    assert_eq!(r.pred.deadline_start(), None);
    assert!(replayed(r.snapshot(11, &[1], &[2])));
    assert_eq!(r.pred.deadline_start(), None);
    // The first complete one starts it.
    assert!(replayed(r.snapshot(12, &[1], &[])));
    assert_eq!(r.pred.deadline_start(), Some(12));
    // A later partial one neither restarts nor stops it.
    assert!(replayed(r.snapshot(13, &[1], &[2])));
    assert_eq!(r.pred.deadline_start(), Some(12));
}

/// Skipped snapshots (a duplicate ack, a stale one) are not usable states.
#[test]
fn a_skipped_snapshot_does_not_start_the_deadline() {
    let mut r = Run::new();
    r.ticks(20 + LAG);
    assert!(matches!(r.snapshot(10, &[], &[]), Outcome::Matched));
    // The same ack again is `Seen` whatever it says.
    assert_eq!(r.snapshot(10, &[1], &[]), Outcome::Skipped(Skip::Seen));
    assert_eq!(r.pred.deadline_start(), None);
}

/// A complete state that agrees again ends the episode, so a later,
/// separate divergence gets its own full budget.
#[test]
fn an_agreeing_complete_state_stops_the_clock() {
    let mut r = Run::new();
    r.ticks(40 + RECOVERY_DEADLINE_TICKS + LAG);
    assert!(replayed(r.snapshot(10, &[1], &[])));
    assert_eq!(r.pred.deadline_start(), Some(10));
    assert!(matches!(r.snapshot(30, &[], &[]), Outcome::Matched));
    assert_eq!(r.pred.deadline_start(), None);
    // Ack 40 + budget is far past the first episode's budget (10 + budget).
    assert!(replayed(r.snapshot(40, &[1], &[])));
    assert_eq!(r.pred.deadline_start(), Some(40));
    let out = r.snapshot(40 + RECOVERY_DEADLINE_TICKS - 1, &[1], &[]);
    assert!(replayed(out), "{out:?}");
    assert_eq!(
        r.snapshot(40 + RECOVERY_DEADLINE_TICKS, &[1], &[]),
        Outcome::Recovered(RecoveryCause::DeadlineExceeded)
    );
}

/// A resting body's replay keeps a residue in velocity (the host's
/// suspension state is not on the wire): replayed, but never a reason to
/// give up the recorded past. Measured 0.22 m/s and 0.14 rad/s at the 99th
/// percentile in `net_contact`.
#[test]
fn a_velocity_residue_with_the_pose_agreeing_never_runs_the_clock() {
    let mut r = Run::new();
    r.noise = Vec3::new(0.2, 0.0, 0.1);
    r.ticks(10 + LAG);
    for ack in 10..10 + 3 * RECOVERY_DEADLINE_TICKS {
        r.tick();
        let out = r.snapshot(ack, &[], &[]);
        assert!(replayed(out), "ack {ack}: {out:?}");
        assert_eq!(r.pred.deadline_start(), None, "ack {ack}");
    }
    assert_eq!(r.pred.recovered_deadline, 0);
}

/// The deadline replays the host's state forward: the own car ends at the
/// tick it was at, not at the host's state `LAG` ticks behind it.
#[test]
fn the_deadline_keeps_the_own_cars_lead() {
    let mut r = Run::new();
    let first = 10;
    r.ticks(first + LAG);
    r.snapshot(first, &[1], &[]);
    for ack in first + 1..first + RECOVERY_DEADLINE_TICKS {
        r.tick();
        r.snapshot(ack, &[1], &[]);
    }
    r.tick();
    let before = present(r.app.world(), r.cars[0]).expect("posed").0;
    let ack = first + RECOVERY_DEADLINE_TICKS;
    let host = own_body(r.pred.ring.get(ack).expect("recorded"), r.cars[0])
        .expect("in the capture")
        .0;
    assert_eq!(
        r.snapshot(ack, &[1], &[]),
        Outcome::Recovered(RecoveryCause::DeadlineExceeded)
    );
    let after = present(r.app.world(), r.cars[0]).expect("posed").0;
    assert!(
        before.distance(after) < 0.05,
        "the car moved {} m",
        before.distance(after)
    );
    assert!(
        after.distance(host) > 0.05,
        "the car sits at the host's state at the ack"
    );
}
