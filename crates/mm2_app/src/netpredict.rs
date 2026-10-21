//! Own-car prediction history and replay reconciliation (NETPLAY-04,
//! `docs/netcode-redesign.md`, `docs/netplay-timeline.md`).
//!
//! Replaces the [`DriftWatch`](crate::netdrive::DriftWatch) /
//! [`SettleWatch`](crate::netdrive::SettleWatch) pulls and snaps as the
//! routine way a client's own car follows the host, behind the one
//! [`NetplayTimeline`] selector.
//!
//! **Timeline.** The client predicts its car on the production step and
//! records the whole post-step physics world after every command, keyed by
//! the command's `seq` ([`record_prediction`], `FixedLast`). A snapshot
//! entry for the own seat carries `input_ack`: the highest `seq` already
//! inside that completed host state. So the host state `S[server_tick]`
//! and the client state recorded for `seq == input_ack` are the *same
//! simulation instant* — no tick arithmetic, no "current car versus past
//! host" comparison. [`reconcile_own`] (`FixedFirst`, between steps) pulls
//! the newest staged assertion and:
//!
//! * compares position, orientation and linear/angular velocity with that
//!   recorded state; inside the tolerances nothing happens;
//! * otherwise restores the recorded world with the host's body state put
//!   in, replays the stored commands `ack+1..=newest` one fixed step at a
//!   time through [`mm2_vehicle::replay::replay_from`], and writes each
//!   re-simulated post-step state back into the history so the next
//!   snapshot compares against the corrected past.
//!
//! The vehicle's wheels, drivetrain and force history come from the
//! client's own recorded state (the wire carries only the body); a
//! divergence there shows up in the body and is corrected on the next
//! comparison. Remote replicas are put back to where they were before the
//! replay (they are driven by the snapshot lerp, not by the replay);
//! contact between predicted cars is NETPLAY-05.
//!
//! **Bounded recovery.** A routine correction is never reported as a
//! reset. The cases that cannot be replayed each take a distinct, counted
//! path ([`RecoveryCause`]) that adopts the host body state outright, and
//! snapshots that cannot be compared are skipped with a [`Skip`] reason.
//! An authority reset (new reset epoch) clears the history instead; the
//! commands issued before it are never replayed across it.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use mm2_game::{PlayerVehicle, Session};
use mm2_vehicle::capture::EpochRef;
use mm2_vehicle::physics_world::{WorldCapture, capture_world};
use mm2_vehicle::replay::{HistoryRing, MAX_REPLAY_STEPS, ReplayStep, replay_from};
use mm2_vehicle::{RemoteReplica, Teleported, VehicleInput};

use crate::net::LobbyLink;
use crate::netcmd::CommandStream;
use crate::netdrive::{RemoteSnaps, ResetEpoch, decode_tick_command};
use crate::netplay_timeline::NetplayTimeline;

/// Position difference (m) below which a replayed snapshot is "the same
/// state". Designed: the two simulations run the same step on the same
/// quantized commands, so any real agreement is far inside this, and a
/// millimetre is invisible; larger is a real divergence worth a replay.
pub const POS_TOLERANCE: f32 = 0.01;
/// Orientation difference (radians) below which states agree (~0.06°).
pub const ROT_TOLERANCE: f32 = 0.001;
/// Linear velocity difference (m/s) below which states agree.
pub const VEL_TOLERANCE: f32 = 0.02;
/// Angular velocity difference (rad/s) below which states agree.
pub const ANG_TOLERANCE: f32 = 0.02;

/// The host's body state for the own seat at its completed tick, staged
/// by `apply_snapshots` for [`reconcile_own`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnAssertion {
    /// Wire session generation the snapshot belongs to.
    pub generation: u64,
    /// The snapshot's host tick (diagnostics; the ack is the alignment).
    pub server_tick: u64,
    /// Highest own command `seq` inside this state.
    pub ack: u64,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub ang: Vec3,
}

/// What `apply_snapshots` hands the fixed schedule: the newest assertion
/// (latest wins; acks only move forward, so a superseded one carries
/// nothing the newer lacks) and whether an authority reset happened.
#[derive(Debug, Default)]
pub struct OwnStage {
    /// Set by `apply_snapshots` from the selector each run.
    pub enabled: bool,
    pub assertion: Option<OwnAssertion>,
    /// The host reset the own car: forget the history.
    pub reset: bool,
}

impl OwnStage {
    /// Drop everything staged (a new authority stream).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Why a snapshot was not compared. None of these touches the car.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// `input_ack == 0`: none of our commands is inside the host state.
    NoAck,
    /// Same or older ack than already compared: a duplicate or a starved
    /// host holding its ack. Nothing new to learn.
    Seen,
    /// The ack names a command we have not simulated: a corrupt or foreign
    /// snapshot.
    Future,
    /// The ack predates the last authority reset.
    PreReset,
    /// A non-finite or degenerate host state is never applied.
    Corrupt,
    /// A snapshot of another session generation.
    OtherGeneration,
}

/// Why the host state was adopted outright instead of replayed. Each is a
/// bounded, rare path with its own counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCause {
    /// The compared state is older than the history holds.
    HistoryExhausted,
    /// More steps to replay than [`MAX_REPLAY_STEPS`].
    TooDeep,
    /// A stored command in `ack+1..=newest` is missing.
    CommandGap,
    /// `replay_from` refused (stale lifetimes, missing history, restore).
    ReplayRefused,
}

/// One reconciliation's outcome, for rendering and the statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    /// The recorded state agreed with the host's.
    Matched,
    /// Restored and replayed.
    Replayed {
        steps: usize,
        /// Host-versus-recorded difference at the compared boundary.
        pos_error: f32,
        rot_error: f32,
        /// How far the *present* car moved because of the replay.
        shift: f32,
        turn: f32,
    },
    Recovered(RecoveryCause),
    Skipped(Skip),
    /// The authority reset the car: history cleared.
    Reset,
}

/// Counters and the last result; `Resource` so the HUD, probe and tests
/// read them. Correction *count and size* are exposed so a stream of tiny
/// corrections cannot hide behind the absence of snaps.
#[derive(Resource, Debug)]
pub struct OwnPrediction {
    generation: u64,
    ring: HistoryRing,
    /// Highest ack compared so far.
    last_ack: u64,
    /// First `seq` the history may be compared at (set by a reset).
    floor: u64,
    /// The most recent outcome.
    pub last: Option<Outcome>,
    pub compared: u64,
    pub matched: u64,
    pub replays: u64,
    pub replayed_steps: u64,
    /// Sum of present-car shifts caused by replays (m).
    pub shift_total: f32,
    /// Sum of present-car turns caused by replays (rad).
    pub turn_total: f32,
    pub resets: u64,
    pub skipped: u64,
    pub recoveries: u64,
    pub recovered_exhausted: u64,
    pub recovered_too_deep: u64,
    pub recovered_gap: u64,
    pub recovered_refused: u64,
}

impl Default for OwnPrediction {
    fn default() -> Self {
        Self {
            generation: 0,
            ring: HistoryRing::default(),
            last_ack: 0,
            floor: 0,
            last: None,
            compared: 0,
            matched: 0,
            replays: 0,
            replayed_steps: 0,
            shift_total: 0.0,
            turn_total: 0.0,
            resets: 0,
            skipped: 0,
            recoveries: 0,
            recovered_exhausted: 0,
            recovered_too_deep: 0,
            recovered_gap: 0,
            recovered_refused: 0,
        }
    }
}

impl OwnPrediction {
    /// Number of recorded post-step states.
    pub fn depth(&self) -> usize {
        self.ring.len()
    }

    /// The recorded command range `(oldest, newest)`.
    pub fn span(&self) -> Option<(u64, u64)> {
        Some((self.ring.oldest()?, self.ring.newest()?))
    }

    fn restart(&mut self, generation: u64) {
        self.generation = generation;
        self.ring.clear();
        self.last_ack = 0;
        self.floor = 0;
    }
}

/// Everything a replay needs besides the world.
pub struct Request<'a> {
    pub own: Entity,
    pub assertion: &'a OwnAssertion,
    /// Newest simulated command `seq`.
    pub newest: u64,
    /// The stored commands as `(seq, input)`, oldest first, starting at
    /// `assertion.ack + 1` when complete.
    pub commands: &'a [(u64, VehicleInput)],
    pub epoch: EpochRef,
}

fn finite3(v: Vec3) -> bool {
    v.is_finite()
}

fn valid(a: &OwnAssertion) -> bool {
    finite3(a.pos)
        && finite3(a.vel)
        && finite3(a.ang)
        && a.rot.is_finite()
        && (a.rot.length_squared() - 1.0).abs() < 0.05
}

fn own_body(capture: &WorldCapture, own: Entity) -> Option<(Vec3, Quat, Vec3, Vec3)> {
    let (_, vc) = capture.vehicles.iter().find(|(e, _)| *e == own)?;
    let b = &vc.body;
    Some((
        Vec3::from(b.position),
        Quat::from_array(b.rotation),
        Vec3::from(b.linear_velocity),
        Vec3::from(b.angular_velocity),
    ))
}

/// Compare the host's `assertion` with the recorded state at its ack and,
/// when they differ, correct by restoring and replaying. The one place the
/// own car's history is rewritten.
pub fn reconcile(world: &mut World, pred: &mut OwnPrediction, req: &Request<'_>) -> Outcome {
    let a = req.assertion;
    let outcome = classify_and_run(world, pred, req);
    match outcome {
        Outcome::Skipped(_) => pred.skipped += 1,
        Outcome::Matched => {
            pred.compared += 1;
            pred.matched += 1;
            pred.last_ack = a.ack;
        }
        Outcome::Replayed {
            steps, shift, turn, ..
        } => {
            pred.compared += 1;
            pred.replays += 1;
            pred.replayed_steps += steps as u64;
            pred.shift_total += shift;
            pred.turn_total += turn;
            pred.last_ack = a.ack;
        }
        Outcome::Recovered(cause) => {
            pred.compared += 1;
            pred.recoveries += 1;
            match cause {
                RecoveryCause::HistoryExhausted => pred.recovered_exhausted += 1,
                RecoveryCause::TooDeep => pred.recovered_too_deep += 1,
                RecoveryCause::CommandGap => pred.recovered_gap += 1,
                RecoveryCause::ReplayRefused => pred.recovered_refused += 1,
            }
            pred.last_ack = a.ack;
        }
        Outcome::Reset => {}
    }
    pred.last = Some(outcome);
    outcome
}

fn classify_and_run(world: &mut World, pred: &mut OwnPrediction, req: &Request<'_>) -> Outcome {
    let a = req.assertion;
    if a.generation != pred.generation {
        return Outcome::Skipped(Skip::OtherGeneration);
    }
    if !valid(a) {
        return Outcome::Skipped(Skip::Corrupt);
    }
    if a.ack == 0 {
        return Outcome::Skipped(Skip::NoAck);
    }
    if a.ack <= pred.last_ack {
        return Outcome::Skipped(Skip::Seen);
    }
    if a.ack > req.newest {
        return Outcome::Skipped(Skip::Future);
    }
    if a.ack < pred.floor {
        return Outcome::Skipped(Skip::PreReset);
    }
    let Some(past) = pred.ring.get(a.ack) else {
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::HistoryExhausted);
    };
    let Some((pos, rot, vel, ang)) = own_body(past, req.own) else {
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::ReplayRefused);
    };
    let pos_error = pos.distance(a.pos);
    let rot_error = rot.angle_between(a.rot);
    if pos_error <= POS_TOLERANCE
        && rot_error <= ROT_TOLERANCE
        && vel.distance(a.vel) <= VEL_TOLERANCE
        && ang.distance(a.ang) <= ANG_TOLERANCE
    {
        return Outcome::Matched;
    }
    let depth = (req.newest - a.ack) as usize;
    let complete = req.commands.len() == depth
        && req
            .commands
            .iter()
            .zip(a.ack + 1..)
            .all(|((seq, _), want)| *seq == want);
    if !complete {
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::CommandGap);
    }
    if depth > MAX_REPLAY_STEPS {
        adopt(world, req);
        pred.ring.clear();
        pred.floor = req.newest + 1;
        return Outcome::Recovered(RecoveryCause::TooDeep);
    }
    replay(world, pred, req, pos_error, rot_error)
}

/// The host's body state, written onto the own car.
fn adopt(world: &mut World, req: &Request<'_>) {
    let a = req.assertion;
    if let Ok(mut car) = world.get_entity_mut(req.own) {
        car.insert((
            Position(a.pos),
            Rotation(a.rot),
            LinearVelocity(a.vel),
            AngularVelocity(a.ang),
            Teleported,
        ));
    }
}

type Pose = (Entity, Vec3, Quat, Vec3, Vec3);

fn remote_poses(world: &mut World) -> Vec<Pose> {
    let mut q = world.query_filtered::<(
        Entity,
        &Position,
        &Rotation,
        &LinearVelocity,
        &AngularVelocity,
    ), With<RemoteReplica>>();
    q.iter(world)
        .map(|(e, p, r, v, w)| (e, p.0, r.0, v.0, w.0))
        .collect()
}

fn put_back(world: &mut World, poses: &[Pose]) {
    for (e, p, r, v, w) in poses {
        if let Ok(mut ent) = world.get_entity_mut(*e) {
            ent.insert((
                Position(*p),
                Rotation(*r),
                LinearVelocity(*v),
                AngularVelocity(*w),
            ));
        }
    }
}

fn present(world: &World, own: Entity) -> Option<(Vec3, Quat)> {
    Some((world.get::<Position>(own)?.0, world.get::<Rotation>(own)?.0))
}

fn replay(
    world: &mut World,
    pred: &mut OwnPrediction,
    req: &Request<'_>,
    pos_error: f32,
    rot_error: f32,
) -> Outcome {
    let a = req.assertion;
    let mut corrected = pred.ring.get(a.ack).expect("checked by the caller").clone();
    if let Some((_, vc)) = corrected.vehicles.iter_mut().find(|(e, _)| *e == req.own) {
        vc.body.position = a.pos.to_array();
        vc.body.rotation = a.rot.to_array();
        vc.body.linear_velocity = a.vel.to_array();
        vc.body.angular_velocity = a.ang.to_array();
    }
    let before = present(world, req.own);
    let remotes = remote_poses(world);

    // A two-deep scratch ring: replay_from reads the boundary it restores,
    // and each step's result is recorded for the next. The permanent
    // history is rewritten only when the whole replay succeeded.
    let mut scratch = HistoryRing::new(2);
    scratch.record(a.ack, corrected.clone());
    let mut staged: Vec<(u64, WorldCapture)> = Vec::with_capacity(req.commands.len());
    let mut from = a.ack;
    for (seq, input) in req.commands {
        let step = ReplayStep {
            inputs: vec![(req.own, *input)],
        };
        if replay_from(world, &scratch, from, std::slice::from_ref(&step)).is_err() {
            put_back(world, &remotes);
            adopt(world, req);
            pred.ring.clear();
            pred.floor = req.newest + 1;
            return Outcome::Recovered(RecoveryCause::ReplayRefused);
        }
        let capture = capture_world(world, req.epoch);
        scratch.record(*seq, capture.clone());
        staged.push((*seq, capture));
        from = *seq;
    }
    put_back(world, &remotes);
    pred.ring.record(a.ack, corrected);
    for (seq, capture) in staged {
        pred.ring.record(seq, capture);
    }
    let (shift, turn) = match (before, present(world, req.own)) {
        (Some((p0, r0)), Some((p1, r1))) => (p0.distance(p1), r0.angle_between(r1)),
        _ => (0.0, 0.0),
    };
    Outcome::Replayed {
        steps: req.commands.len(),
        pos_error,
        rot_error,
        shift,
        turn,
    }
}

fn own_entity(world: &mut World) -> Option<(Entity, u8)> {
    let mut q = world.query_filtered::<(Entity, Option<&ResetEpoch>), With<PlayerVehicle>>();
    let mut it = q.iter(world);
    let (e, epoch) = it.next()?;
    it.next().is_none().then_some((e, epoch.map_or(0, |e| e.0)))
}

fn gate(world: &World) -> bool {
    world
        .get_resource::<NetplayTimeline>()
        .is_some_and(|t| t.enabled)
        && world.contains_resource::<LobbyLink>()
        && world.resource::<Session>().is_playing()
}

fn epoch_ref(session: &Session, epoch: u8) -> EpochRef {
    EpochRef {
        generation: session.wire_generation() as u32,
        reset_epoch: u32::from(epoch),
    }
}

/// `FixedLast`: record the whole physics world after this tick's step,
/// under the `seq` of the command that drove it.
pub fn record_prediction(world: &mut World) {
    if !gate(world) {
        return;
    }
    let generation = world.resource::<Session>().wire_generation();
    let Some((_, epoch)) = own_entity(world) else {
        return;
    };
    let seq = world.resource::<CommandStream>().history().last_seq();
    let epoch = epoch_ref(world.resource::<Session>(), epoch);
    let mut pred = world.remove_resource::<OwnPrediction>().unwrap_or_default();
    if pred.generation != generation {
        pred.restart(generation);
    }
    if seq != 0 && pred.ring.newest().is_none_or(|n| seq > n) {
        let capture = capture_world(world, epoch);
        pred.ring.record(seq, capture);
    }
    world.insert_resource(pred);
}

/// `FixedFirst`: apply the staged host assertion, between fixed steps.
pub fn reconcile_own(world: &mut World) {
    if !gate(world) {
        return;
    }
    let (assertion, reset) = {
        let Some(mut snaps) = world.get_resource_mut::<RemoteSnaps>() else {
            return;
        };
        let stage = snaps.own_stage_mut();
        (stage.assertion.take(), std::mem::take(&mut stage.reset))
    };
    let generation = world.resource::<Session>().wire_generation();
    let Some((own, epoch)) = own_entity(world) else {
        return;
    };
    let newest = world.resource::<CommandStream>().history().last_seq();
    let mut pred = world.remove_resource::<OwnPrediction>().unwrap_or_default();
    if pred.generation != generation {
        pred.restart(generation);
    }
    if reset {
        pred.ring.clear();
        pred.floor = newest + 1;
        pred.resets += 1;
        pred.last = Some(Outcome::Reset);
        println!("event=own_reconcile kind=reset newest={newest}");
    }
    if let Some(assertion) = assertion {
        let commands: Vec<(u64, VehicleInput)> = world
            .resource::<CommandStream>()
            .history()
            .unacked()
            .filter(|r| r.seq > assertion.ack)
            .map(|r| (r.seq, decode_tick_command(&r.command)))
            .collect();
        let epoch = epoch_ref(world.resource::<Session>(), epoch);
        let before = present(world, own);
        let outcome = reconcile(
            world,
            &mut pred,
            &Request {
                own,
                assertion: &assertion,
                newest,
                commands: &commands,
                epoch,
            },
        );
        report(&assertion, outcome, before, present(world, own));
        // Everything up to the ack is inside the host's state: it can
        // never be replayed again.
        if matches!(
            outcome,
            Outcome::Matched | Outcome::Replayed { .. } | Outcome::Recovered(_)
        ) {
            world
                .resource_mut::<CommandStream>()
                .history_mut()
                .ack(assertion.ack);
        }
    }
    world.insert_resource(pred);
}

fn report(
    a: &OwnAssertion,
    outcome: Outcome,
    before: Option<(Vec3, Quat)>,
    after: Option<(Vec3, Quat)>,
) {
    let kind = match outcome {
        Outcome::Replayed { .. } => "replay",
        Outcome::Recovered(cause) => match cause {
            RecoveryCause::HistoryExhausted => "recover_history",
            RecoveryCause::TooDeep => "recover_depth",
            RecoveryCause::CommandGap => "recover_gap",
            RecoveryCause::ReplayRefused => "recover_refused",
        },
        _ => return,
    };
    let (Some((p0, _)), Some((p1, _))) = (before, after) else {
        return;
    };
    crate::netprobe::note_correction(kind, a.server_tick, p0, p1);
    println!(
        "event=own_reconcile kind={kind} tick={} ack={} shift={:.3}",
        a.server_tick,
        a.ack,
        p0.distance(p1)
    );
}

/// Register the history and the two fixed-schedule systems.
pub fn add_systems(app: &mut App) {
    app.init_resource::<OwnPrediction>()
        .add_systems(FixedFirst, reconcile_own)
        .add_systems(FixedLast, record_prediction);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use avian3d::prelude::*;
    use bevy::time::TimeUpdateStrategy;
    use mm2_vehicle::capture::{capture_vehicle, restore_vehicle};
    use mm2_vehicle::{VehicleConfig, VehiclePlugin, VehicleProbeLayers, vehicle_bundle};

    use super::*;

    const EPOCH: EpochRef = EpochRef {
        generation: 1,
        reset_epoch: 0,
    };
    const GEN: u64 = 1;

    fn app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(bevy::mesh::MeshPlugin)
            .add_plugins(bevy::gizmos::GizmoPlugin)
            .add_plugins(PhysicsPlugins::default())
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / 60.0,
            )))
            .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
            .add_plugins(TransformPlugin)
            .add_plugins(VehiclePlugin)
            .insert_resource(VehicleProbeLayers(LayerMask(0b01)));
        app.finish();
        app.cleanup();
        let w = app.world_mut();
        w.spawn((
            RigidBody::Static,
            Collider::cuboid(600.0, 1.0, 600.0),
            Position(Vec3::new(0.0, -0.5, 0.0)),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        let car = w
            .spawn((
                vehicle_bundle(&VehicleConfig::default()),
                Position(Vec3::new(0.0, 1.2, 0.0)),
                Transform::from_xyz(0.0, 1.2, 0.0),
            ))
            .id();
        app.update();
        (app, car)
    }

    /// The scripted command of tick `seq`: a pedal-and-steering mix that
    /// changes several times, so a replay has real turns and brakes.
    fn command(seq: u64) -> VehicleInput {
        match seq {
            0..=8 => VehicleInput {
                throttle: 1.0,
                ..VehicleInput::default()
            },
            9..=14 => VehicleInput {
                throttle: 0.8,
                steering: 0.5,
                ..VehicleInput::default()
            },
            15..=19 => VehicleInput {
                brake: 0.7,
                steering: -0.4,
                ..VehicleInput::default()
            },
            _ => VehicleInput {
                throttle: 1.0,
                steering: 0.1,
                ..VehicleInput::default()
            },
        }
    }

    fn live(app: &mut App, car: Entity, seq: u64) {
        *app.world_mut().get_mut::<VehicleInput>(car).unwrap() = command(seq);
        app.update();
    }

    /// Run commands `1..=n`, recording the world after each.
    fn run(app: &mut App, car: Entity, pred: &mut OwnPrediction, n: u64) {
        for seq in 1..=n {
            live(app, car, seq);
            let cap = capture_world(app.world_mut(), EPOCH);
            pred.ring.record(seq, cap);
        }
    }

    fn fresh() -> OwnPrediction {
        let mut p = OwnPrediction::default();
        p.restart(GEN);
        p
    }

    fn assertion_from(pred: &OwnPrediction, car: Entity, ack: u64) -> OwnAssertion {
        let (pos, rot, vel, ang) = own_body(pred.ring.get(ack).unwrap(), car).unwrap();
        OwnAssertion {
            generation: GEN,
            server_tick: ack + 100,
            ack,
            pos,
            rot,
            vel,
            ang,
        }
    }

    fn commands(from: u64, to: u64) -> Vec<(u64, VehicleInput)> {
        (from..=to).map(|s| (s, command(s))).collect()
    }

    fn go(
        app: &mut App,
        pred: &mut OwnPrediction,
        car: Entity,
        a: &OwnAssertion,
        newest: u64,
    ) -> Outcome {
        let cmds = commands(a.ack + 1, newest);
        reconcile(
            app.world_mut(),
            pred,
            &Request {
                own: car,
                assertion: a,
                newest,
                commands: &cmds,
                epoch: EPOCH,
            },
        )
    }

    #[test]
    fn an_agreeing_snapshot_changes_nothing() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let before = *app.world().get::<Position>(car).unwrap();
        let a = assertion_from(&pred, car, 20);
        assert_eq!(go(&mut app, &mut pred, car, &a, 30), Outcome::Matched);
        assert_eq!(*app.world().get::<Position>(car).unwrap(), before);
        assert_eq!((pred.matched, pred.replays), (1, 0));
    }

    /// AC2: a divergent historical state, corrected after several pending
    /// turns and brakes, equals the same authoritative state run through
    /// those inputs — not the un-replayed host pose — and the history is
    /// rewritten so the next snapshot compares against the corrected past.
    #[test]
    fn a_divergent_past_is_replayed_through_the_pending_commands() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let mut a = assertion_from(&pred, car, 8);
        a.pos += Vec3::new(0.4, 0.0, -0.3);
        a.vel += Vec3::new(1.5, 0.0, 0.0);
        let wrong_present = app.world().get::<Position>(car).unwrap().0;
        let Outcome::Replayed { steps, .. } = go(&mut app, &mut pred, car, &a, 30) else {
            panic!("expected a replay, got {:?}", pred.last);
        };
        assert_eq!(steps, 22);
        let corrected = app.world().get::<Position>(car).unwrap().0;
        assert_ne!(corrected, wrong_present);

        // Reference: the same host state, run live through the same
        // commands in an independent world.
        let (mut twin, tcar) = self::app();
        let mut scratch = fresh();
        run(&mut twin, tcar, &mut scratch, 8);
        let mut cap = capture_vehicle(twin.world(), tcar, EPOCH).unwrap();
        cap.body.position = a.pos.to_array();
        cap.body.linear_velocity = a.vel.to_array();
        restore_vehicle(twin.world_mut(), tcar, &cap).unwrap();
        for seq in 9..=30 {
            live(&mut twin, tcar, seq);
        }
        let expected = twin.world().get::<Position>(tcar).unwrap().0;
        assert!(
            corrected.distance(expected) < 1e-3,
            "replayed {corrected} vs reference {expected}"
        );

        // The history was rewritten: newest entry is the corrected present.
        let (p, ..) = own_body(pred.ring.get(30).unwrap(), car).unwrap();
        assert!(p.distance(corrected) < 1e-3);
        // And a later snapshot of the corrected past now matches.
        let again = assertion_from(&pred, car, 25);
        assert_eq!(go(&mut app, &mut pred, car, &again, 30), Outcome::Matched);
    }

    /// AC3: a duplicated, older or starved ack never applies twice.
    #[test]
    fn duplicate_and_older_acks_are_skipped() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let mut a = assertion_from(&pred, car, 12);
        a.pos.x += 1.0;
        assert!(matches!(
            go(&mut app, &mut pred, car, &a, 30),
            Outcome::Replayed { .. }
        ));
        let at = app.world().get::<Position>(car).unwrap().0;
        assert_eq!(
            go(&mut app, &mut pred, car, &a, 30),
            Outcome::Skipped(Skip::Seen)
        );
        let mut older = assertion_from(&pred, car, 10);
        older.pos.x += 5.0;
        assert_eq!(
            go(&mut app, &mut pred, car, &older, 30),
            Outcome::Skipped(Skip::Seen)
        );
        assert_eq!(app.world().get::<Position>(car).unwrap().0, at);
        assert_eq!((pred.replays, pred.skipped), (1, 2));
    }

    #[test]
    fn unusable_snapshots_are_skipped_without_touching_the_car() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let at = app.world().get::<Position>(car).unwrap().0;
        let good = assertion_from(&pred, car, 20);
        let cases = [
            (OwnAssertion { ack: 0, ..good }, Skip::NoAck),
            (OwnAssertion { ack: 31, ..good }, Skip::Future),
            (
                OwnAssertion {
                    pos: Vec3::new(f32::NAN, 0.0, 0.0),
                    ..good
                },
                Skip::Corrupt,
            ),
            (
                OwnAssertion {
                    generation: GEN + 1,
                    ..good
                },
                Skip::OtherGeneration,
            ),
        ];
        for (a, why) in cases {
            assert_eq!(go(&mut app, &mut pred, car, &a, 30), Outcome::Skipped(why));
        }
        assert_eq!(app.world().get::<Position>(car).unwrap().0, at);
    }

    /// An authority reset forgets the history; the ack of a command from
    /// before it can no longer be compared or replayed across the map.
    #[test]
    fn nothing_replays_across_a_reset() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        pred.ring.clear();
        pred.floor = 31;
        let mut a = assertion_from(
            &{
                let mut p = fresh();
                run(&mut app, car, &mut p, 1);
                p
            },
            car,
            1,
        );
        a.ack = 12;
        a.pos.x += 50.0;
        assert_eq!(
            go(&mut app, &mut pred, car, &a, 31),
            Outcome::Skipped(Skip::PreReset)
        );
    }

    /// Exhausted history takes its own counted recovery and says so.
    #[test]
    fn a_state_older_than_the_history_is_a_named_recovery() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let mut a = assertion_from(&pred, car, 20);
        a.ack = 10;
        pred.ring = HistoryRing::new(10);
        run(&mut app, car, &mut pred, 0);
        let cap = capture_world(app.world_mut(), EPOCH);
        pred.ring.record(30, cap);
        let out = go(&mut app, &mut pred, car, &a, 30);
        assert_eq!(out, Outcome::Recovered(RecoveryCause::HistoryExhausted));
        assert_eq!(app.world().get::<Position>(car).unwrap().0, a.pos);
        assert!(app.world().get::<Teleported>(car).is_some());
        assert_eq!(
            (pred.recoveries, pred.recovered_exhausted, pred.replays),
            (1, 1, 0)
        );
    }

    #[test]
    fn a_missing_stored_command_is_a_named_recovery() {
        let (mut app, car) = app();
        let mut pred = fresh();
        run(&mut app, car, &mut pred, 30);
        let mut a = assertion_from(&pred, car, 10);
        a.pos.z += 1.0;
        let mut cmds = commands(11, 30);
        cmds.remove(4);
        let out = reconcile(
            app.world_mut(),
            &mut pred,
            &Request {
                own: car,
                assertion: &a,
                newest: 30,
                commands: &cmds,
                epoch: EPOCH,
            },
        );
        assert_eq!(out, Outcome::Recovered(RecoveryCause::CommandGap));
    }
}
