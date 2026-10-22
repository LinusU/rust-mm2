//! NETPLAY-05H: contact with a client's traffic copies inside a prediction
//! replay, through the app's own systems.
//!
//! 05C tested the replay's treatment of kinematic movers and of actors that
//! spawn inside the window on `mm2_vehicle`'s production step alone. Here the
//! mover is a real `TrafficCopy`, spawned and posed by the production
//! `worldtraffic::apply_traffic` from `Traffic` rows pushed into
//! `RemoteSnaps`, in a `Remote` city session, and the history is recorded in
//! `FixedLast` the way `record_prediction` does (before `Update`, so before
//! the next row lands). The own car is the session's player vehicle.
//!
//! Method: a *twin* client receives the same rows on the same ticks and at
//! tick `ACK` has its own car nudged sideways, standing in for the host
//! knowing something the client did not. The twin's own-car state right after
//! the nudge is the host assertion; the client, which never saw the nudge,
//! reconciles with `netpredict::reconcile` and must land where the twin did at
//! `NOW`. The control arms strip the mover history from the client's ring, or
//! show the twin's result had the actor been present from the start of the
//! window, so a pass cannot be the actor never mattering.
//!
//! Evidence level: synthetic integration (synthetic city install and `va_*`
//! classes, rows pushed in-process; no sockets and no retail data).

use std::collections::HashMap;
use std::rc::Rc;

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_app::netdrive::RemoteSnaps;
use mm2_app::netpredict::{Outcome, OwnAssertion, OwnPrediction, Request, reconcile};
use mm2_app::worldprops::{PROP_ACTIVE, apply_props, drive_prop_lerp};
use mm2_app::worldtraffic::{CAR_LANE, TrafficReplica};
use mm2_game::{PlayerVehicle, Session, SessionPhase};
use mm2_net::{SNAP_NO_FRAGMENT, SnapCar, SnapProp};
use mm2_vehicle::VehicleInput;
use mm2_vehicle::capture::EpochRef;
use mm2_vehicle::physics_world::{WorldCapture, capture_world};

use super::{client_app, copies, phase_is, run_until};
use crate::support::city_install;

const EPOCH: EpochRef = EpochRef {
    generation: 0,
    reset_epoch: 0,
};
/// Ticks of driving before the scenarios place anything.
const ANCHOR: u64 = 60;
/// The tick whose recorded state the host asserts.
const ACK: u64 = 70;
/// The tick the client reconciles at: `NOW - ACK` is inside `MAX_REPLAY_STEPS`.
const NOW: u64 = ACK + 100;
/// The nudge the host knows about and the client does not (metres).
const NUDGE: f32 = 0.3;
/// Oncoming traffic, m/s.
const SPEED: f32 = 20.0;
/// How far ahead of the anchor pose the copy is when the car meets it.
const MEETING: f32 = 20.8;
/// The tick the car meets the moving copy.
const MEETS: u64 = 150;

#[derive(Resource, Default)]
struct Seq(u64);

/// Record the mover history, or strip it (the pre-05C ring).
#[derive(Resource, Default)]
struct Strip(bool);

/// Every capture the ring was given, by `seq`, kept for the asserts.
#[derive(Resource, Default)]
struct Captures(HashMap<u64, WorldCapture>);

fn bump(mut seq: ResMut<Seq>) {
    seq.0 += 1;
}

/// The `record_prediction` step, minus the session gate.
fn record(world: &mut World) {
    let seq = world.resource::<Seq>().0;
    let mut capture = capture_world(world, EPOCH);
    if world.resource::<Strip>().0 {
        capture.movers = None;
    }
    world
        .resource_mut::<Captures>()
        .0
        .insert(seq, capture.clone());
    world.resource_mut::<OwnPrediction>().record(seq, capture);
}

fn drive() -> VehicleInput {
    VehicleInput {
        throttle: 1.0,
        ..VehicleInput::default()
    }
}

/// The own car's untouched trajectory: position and velocity after each
/// tick, from a run with no copy and no nudge.
struct Track {
    pos: Vec<Vec3>,
    vel: Vec<Vec3>,
}

impl Track {
    fn at(&self, tick: u64) -> Vec3 {
        self.pos[tick as usize]
    }

    /// Horizontal unit heading at the anchor tick.
    fn fwd(&self) -> Vec3 {
        let v = self.vel[ANCHOR as usize];
        Vec3::new(v.x, 0.0, v.z).normalize()
    }
}

/// What the host's copy does at a tick: its pose and velocity, `None` while it
/// does not exist.
type Script<'a> = &'a dyn Fn(u64) -> Option<(Vec3, Vec3)>;

/// What the host says about the props at a tick: `(site, phase, pose)`.
type PropRows = dyn Fn(u64) -> Vec<(u32, u8, Vec3)>;

/// Stamped prop placements and the rows about them, pushed every second tick
/// like the traffic.
#[derive(Clone)]
struct Props {
    /// Home positions of the placements the city stamp minted, in order.
    sites: Vec<Vec3>,
    rows: Rc<PropRows>,
}

struct Scene {
    app: App,
    props: Option<Props>,
    own: Entity,
    wire: u64,
    digest: u64,
    n: u64,
    _install: tempfile::TempDir,
}

impl Scene {
    fn new(strip: bool) -> Self {
        Self::with(strip, None)
    }

    /// Stamp one banger placement the way `city::spawn_banger_prop` does:
    /// the shared bundle and the next `BangerSite` ordinal, dormant.
    fn stamp_prop(app: &mut App, pos: Vec3) {
        let (object, role, site, owner) = {
            let mut session = app.world_mut().resource_mut::<Session>();
            (
                session.mint_object_id(),
                session.authority_role(),
                session.mint_banger_site(),
                mm2_game::SessionEntity(session.generation()),
            )
        };
        let def = mm2_game::BangerDefinition {
            name: "prop".into(),
            mass: 40.0,
            friction: 0.9,
            elasticity: 0.5,
            impulse_limit2: 100.0,
            size: [0.5, 0.5, 0.5],
            cg: [0.0, 0.0, 0.0],
            num_parts: 0,
            audio_id: 0,
        };
        app.world_mut()
            .spawn(mm2_app::banger::banger_bundle(
                mm2_game::Banger::new(def),
                object,
                role,
                owner,
                Collider::cuboid(1.0, 1.0, 1.0),
                Transform::from_translation(pos),
                format!("prop-{}", site.0),
            ))
            .insert((site, Position(pos), Rotation(Quat::IDENTITY)));
    }

    fn with(strip: bool, props: Option<Props>) -> Self {
        let install = city_install();
        let mut app = client_app(install.path());
        app.insert_resource(Time::<Fixed>::from_hz(60.0));
        if props.is_some() {
            app.add_systems(Update, (apply_props, drive_prop_lerp));
        }
        assert!(run_until(&mut app, 12, |a| phase_is(
            a,
            SessionPhase::Playing
        )));
        // Stamped once the session owns its generation; the next update
        // lets `apply_props` note the sites into the local table.
        if let Some(props) = &props {
            for site in &props.sites {
                Self::stamp_prop(&mut app, *site);
            }
            app.update();
        }
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(4000.0, 1.0, 4000.0),
            Position(Vec3::new(0.0, -0.5, 0.0)),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        let own = app
            .world_mut()
            .query_filtered::<Entity, With<PlayerVehicle>>()
            .single(app.world())
            .expect("one player vehicle");
        {
            let mut e = app.world_mut().entity_mut(own);
            let at = Vec3::new(0.0, 1.2, 0.0);
            e.get_mut::<Position>().unwrap().0 = at;
            e.get_mut::<Rotation>().unwrap().0 = Quat::IDENTITY;
            e.get_mut::<LinearVelocity>().unwrap().0 = Vec3::ZERO;
            e.get_mut::<AngularVelocity>().unwrap().0 = Vec3::ZERO;
            *e.get_mut::<Transform>().unwrap() = Transform::from_translation(at);
        }
        let wire = app.world().resource::<Session>().wire_generation();
        let digest = app.world().resource::<TrafficReplica>().digest();
        app.init_resource::<Seq>()
            .insert_resource(Strip(strip))
            .init_resource::<Captures>()
            .init_resource::<OwnPrediction>()
            .add_systems(FixedFirst, bump)
            .add_systems(FixedLast, record);
        Self {
            app,
            props,
            own,
            wire,
            digest,
            n: 0,
            _install: install,
        }
    }

    /// One fixed step with the host's copy row for it (rows travel every
    /// second tick), then the nudge if this is its tick.
    fn tick(&mut self, copy: Option<(Vec3, Vec3)>, nudge: Option<(u64, Vec3)>) {
        self.n += 1;
        *self
            .app
            .world_mut()
            .get_mut::<VehicleInput>(self.own)
            .unwrap() = drive();
        if let Some((pos, vel)) = copy
            && self.n.is_multiple_of(2)
        {
            let row = SnapCar {
                id: 1,
                class: 0,
                state: CAR_LANE,
                pos: pos.to_array(),
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: vel.to_array(),
            };
            self.app
                .world_mut()
                .resource_mut::<RemoteSnaps>()
                .push_traffic(self.wire, self.n, self.digest, vec![row]);
        }
        if let Some(props) = &self.props
            && self.n.is_multiple_of(2)
        {
            let rows: Vec<SnapProp> = (props.rows)(self.n)
                .into_iter()
                .map(|(site, phase, pos)| SnapProp {
                    site,
                    fragment: SNAP_NO_FRAGMENT,
                    phase,
                    pos: pos.to_array(),
                    rot: [0.0, 0.0, 0.0, 1.0],
                })
                .collect();
            if !rows.is_empty() {
                let snaps = self.app.world_mut().resource_mut::<RemoteSnaps>();
                let table = snaps.props().local_table();
                self.app
                    .world_mut()
                    .resource_mut::<RemoteSnaps>()
                    .push_props(self.wire, self.n, table, rows);
            }
        }
        self.app.update();
        assert_eq!(
            self.app.world().resource::<Seq>().0,
            self.n,
            "one fixed step per update"
        );
        if let Some((at, by)) = nudge
            && at == self.n
        {
            let mut e = self.app.world_mut().entity_mut(self.own);
            e.get_mut::<Position>().unwrap().0 += by;
            e.get_mut::<Transform>().unwrap().translation += by;
        }
    }

    fn run(&mut self, until: u64, script: Script, nudge: Option<(u64, Vec3)>) {
        while self.n < until {
            self.tick(script(self.n + 1), nudge);
        }
    }

    fn own_state(&self) -> (Vec3, Quat, Vec3, Vec3) {
        let w = self.app.world();
        (
            w.get::<Position>(self.own).unwrap().0,
            w.get::<Rotation>(self.own).unwrap().0,
            w.get::<LinearVelocity>(self.own).unwrap().0,
            w.get::<AngularVelocity>(self.own).unwrap().0,
        )
    }

    /// The actor under test: the traffic copy, or the first stamped prop.
    fn copy(&mut self) -> Option<Entity> {
        if self.props.is_some() {
            return self
                .app
                .world_mut()
                .query_filtered::<Entity, With<mm2_game::BangerSite>>()
                .iter(self.app.world())
                .next();
        }
        copies(&mut self.app).first().map(|c| c.3)
    }
}

fn track() -> Track {
    let mut s = Scene::new(false);
    let mut t = Track {
        pos: vec![Vec3::ZERO],
        vel: vec![Vec3::ZERO],
    };
    while s.n < NOW {
        s.tick(None, None);
        let (p, _, v, _) = s.own_state();
        t.pos.push(p);
        t.vel.push(v);
    }
    t
}

/// Run a twin to `NOW` with the nudge at `ACK`; its assertion is its own-car
/// state right after the nudge, and its final state is the reference.
fn twin(script: Script, by: Vec3) -> (OwnAssertion, Vec3) {
    twin_in(None, script, by)
}

fn twin_in(props: Option<Props>, script: Script, by: Vec3) -> (OwnAssertion, Vec3) {
    let mut s = Scene::with(false, props);
    s.run(ACK, script, Some((ACK, by)));
    let (pos, rot, vel, ang) = s.own_state();
    let a = OwnAssertion {
        generation: 0,
        server_tick: ACK,
        ack: ACK,
        pos,
        rot,
        vel,
        ang,
    };
    s.run(NOW, script, None);
    (a, s.own_state().0)
}

struct Replayed {
    outcome: Outcome,
    own: Vec3,
    /// The copy was not in the capture of the compared tick.
    entrant: bool,
    copy_in_ring: bool,
    disabled_left: usize,
    copy_at_end: Option<Vec3>,
}

fn client(script: Script, a: &OwnAssertion, strip: bool) -> Replayed {
    client_in(None, script, a, strip)
}

fn client_in(props: Option<Props>, script: Script, a: &OwnAssertion, strip: bool) -> Replayed {
    let mut s = Scene::with(strip, props);
    s.run(NOW, script, None);
    let commands: Vec<(u64, VehicleInput)> = (ACK + 1..=NOW).map(|q| (q, drive())).collect();
    let copy = s.copy();
    let (entrant, copy_in_ring) = {
        let caps = &s.app.world().resource::<Captures>().0;
        let has =
            |t: u64| copy.is_some_and(|c| caps[&t].movers.iter().flatten().any(|(e, _)| *e == c));
        (!has(ACK), has(NOW))
    };
    let own = s.own;
    let n = s.n;
    let outcome = s
        .app
        .world_mut()
        .resource_scope(|world, mut pred: Mut<OwnPrediction>| {
            reconcile(
                world,
                &mut pred,
                &Request {
                    own,
                    assertion: a,
                    newest: n,
                    commands: &commands,
                    remotes: &[],
                    trailers: &[],
                    epoch: EPOCH,
                },
            )
        });
    let disabled_left = s
        .app
        .world_mut()
        .query_filtered::<Entity, With<RigidBodyDisabled>>()
        .iter(s.app.world())
        .count();
    let copy_at_end = copy.and_then(|c| s.app.world().get::<Position>(c).map(|p| p.0));
    Replayed {
        outcome,
        own: s.own_state().0,
        entrant,
        copy_in_ring,
        disabled_left,
        copy_at_end,
    }
}

fn nudge(t: &Track) -> Vec3 {
    t.fwd().cross(Vec3::Y) * NUDGE
}

fn replayed_ok(r: &Replayed) {
    assert!(
        matches!(r.outcome, Outcome::Replayed { .. }),
        "the nudge must force a replay, got {:?}",
        r.outcome
    );
    assert_eq!(r.disabled_left, 0, "the replay leaves no body disabled");
}

/// A traffic copy appears in the window ahead of the car, drives on at
/// oncoming and stops mid-window; the car runs into it. Replayed against
/// the recorded copy (entry tick and velocity change) the car ends where the
/// twin's did; with the copy history stripped it does not.
#[test]
fn a_replay_meets_the_traffic_copy_as_it_was_recorded() {
    let t = track();
    let by = nudge(&t);
    let (appear, stops) = (ACK + 10, NOW - 5);
    let (fwd, anchor) = (t.fwd(), t.at(ANCHOR));
    let script = move |n: u64| {
        (n >= appear).then(|| {
            let travelled = MEETING - (n.min(stops) as f32 - MEETS as f32) / 60.0 * SPEED;
            let v = if n < stops { -fwd * SPEED } else { Vec3::ZERO };
            (anchor + fwd * travelled, v)
        })
    };
    let (a, reference) = twin(&script, by);
    let (_, plain) = twin(&script, Vec3::ZERO);
    let moved = reference.distance(plain);
    eprintln!("nudge changes the end by {moved:.3} m");

    let ok = client(&script, &a, false);
    replayed_ok(&ok);
    assert!(ok.entrant, "the copy spawned after the compared tick");
    assert!(ok.copy_in_ring, "and the recorder holds it afterwards");
    let err = ok.own.distance(reference);
    eprintln!("with history: {err:.4} m ({:?})", ok.outcome);

    let stripped = client(&script, &a, true);
    let err_stripped = stripped.own.distance(reference);
    eprintln!(
        "history stripped: {err_stripped:.4} m ({:?})",
        stripped.outcome
    );
    assert!(err < 0.05, "replay vs twin: {err}");
    assert!(
        err_stripped > err + 0.05,
        "control: {err_stripped} vs {err}"
    );
}

/// An entrant the car has already passed: the copy first appears after the
/// compared tick, behind the car. The replay keeps it out until its tick, so
/// the car does not hit a body that was not there. Had the copy been present
/// from the start of the window the twin's car would have hit it.
#[test]
fn a_copy_that_appears_behind_the_car_is_not_hit_by_the_replay() {
    let t = track();
    let by = nudge(&t);
    let spot = t.at(ACK + 10);
    let appear = ACK + 55;
    let late = move |n: u64| (n >= appear).then_some((spot, Vec3::ZERO));
    let early = move |n: u64| (n >= ANCHOR).then_some((spot, Vec3::ZERO));
    let (a, reference) = twin(&late, by);
    let (_, early_end) = twin(&early, by);
    eprintln!(
        "present from the start of the window instead: {:.3} m away",
        early_end.distance(reference)
    );
    assert!(
        early_end.distance(reference) > 0.5,
        "control: the early copy must change the outcome"
    );

    let ok = client(&late, &a, false);
    replayed_ok(&ok);
    assert!(ok.entrant);
    let err = ok.own.distance(reference);
    eprintln!("replay vs twin: {err:.4} m ({:?})", ok.outcome);
    assert!(err < 0.05, "replay vs twin: {err}");
    assert!(ok.copy_at_end.is_some_and(|p| p.distance(spot) < 0.5));
}

/// A stamped prop in the car's path. Dormant at its home until the host's
/// first `Active` row (inside the replay window, so an entrant: the body
/// turns kinematic and loses its sensor), where the car runs into it; a
/// last row, a few ticks before the end, throws it clear of the path. A
/// replay that held the prop at its latest pose all along would miss it.
fn prop_scene(t: &Track) -> Props {
    let (fwd, anchor) = (t.fwd(), t.at(ANCHOR));
    let home = anchor + fwd * 40.0 + Vec3::Y * 0.5;
    let ahead = t.at(MEETS + 10) + fwd * 1.5;
    let in_path = Vec3::new(ahead.x, 0.5, ahead.z);
    let clear = in_path + fwd.cross(Vec3::Y) * 6.0;
    let (first, thrown) = (ACK + 10, NOW - 4);
    Props {
        sites: vec![home],
        rows: Rc::new(move |n| match n {
            n if n >= thrown => vec![(0, PROP_ACTIVE, clear)],
            n if n >= first => vec![(0, PROP_ACTIVE, in_path)],
            _ => Vec::new(),
        }),
    }
}

/// Prop contact through the production `apply_props`: the replay meets the
/// prop as it was recorded (activation and the throw inside the window);
/// stripped of the mover history it does not.
#[test]
fn a_replay_meets_the_prop_as_it_was_recorded() {
    let t = track();
    let by = nudge(&t);
    let none = |_: u64| None;
    let props = prop_scene(&t);
    let (a, reference) = twin_in(Some(props.clone()), &none, by);
    let (_, plain) = twin_in(Some(props.clone()), &none, Vec3::ZERO);
    eprintln!(
        "nudge changes the end by {:.3} m",
        reference.distance(plain)
    );

    let (_, bare) = twin_in(None, &none, by);
    let mattered = reference.distance(bare);
    eprintln!("prop changes the end by {mattered:.3} m");
    assert!(mattered > 0.5, "control: the prop must change the outcome");
    let ok = client_in(Some(props.clone()), &none, &a, false);
    replayed_ok(&ok);
    assert!(ok.entrant, "the prop turned active after the compared tick");
    assert!(ok.copy_in_ring, "and the recorder holds it afterwards");
    let err = ok.own.distance(reference);
    eprintln!("with history: {err:.4} m ({:?})", ok.outcome);

    let stripped = client_in(Some(props), &none, &a, true);
    let err_stripped = stripped.own.distance(reference);
    eprintln!(
        "history stripped: {err_stripped:.4} m ({:?})",
        stripped.outcome
    );
    assert!(err < 0.05, "replay vs twin: {err}");
    assert!(
        err_stripped > err + 0.05,
        "control: {err_stripped} vs {err}"
    );
}
