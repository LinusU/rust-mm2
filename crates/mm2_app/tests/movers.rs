//! Moving scenery against a real Avian world, through the production
//! `drive_movers` system (F28-AC02/AC03): a boat or train is posed with
//! no teleport discontinuity — across the path's wraparound and a
//! train's reversal — and a body resting on a moving platform rides it
//! and is left alone by a stopped one. Synthetic paths and boxes, no
//! retail model.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::layers::GameLayer;
use mm2_app::movers::{Mover, MoverFamily, Train, TrainCar, drive_movers};
use mm2_game::movers::{
    FERRY_SPEED, PathFollower, TRAIN_CARS, TRAIN_SPEED, TRAIN_WAIT, TrainMotion, mover_rotation,
};
use mm2_game::{Session, SessionPhase};

/// Fixed steps per rendered update (`60 Hz` frames over a `120 Hz`
/// fixed rate), so a per-update bound is two per-step bounds.
const STEPS_PER_UPDATE: f32 = 2.0;
const FIXED_DT: f32 = 1.0 / 120.0;
/// The most a step may exceed `speed · dt` on any retail path: the
/// Hermite parameter is not arc length, so a tight or overshooting
/// segment runs fast (measured worst 3.6 on retail trains' first
/// segment). A teleport is orders of magnitude past it.
const MAX_STEP_RATIO: f32 = 4.0;

fn app_in(phase: SessionPhase) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .add_plugins(TransformPlugin)
        .insert_resource(session_in(phase))
        // The app registers it after the solver step, in `FixedLast`.
        .add_systems(FixedLast, drive_movers);
    app.finish();
    app.cleanup();
    app
}

fn session_in(phase: SessionPhase) -> Session {
    let mut session = Session::default();
    for step in [
        SessionPhase::Loading,
        SessionPhase::Ready,
        SessionPhase::Countdown,
        SessionPhase::Playing,
    ] {
        let reached = step == phase;
        session.transition(step).unwrap();
        if reached {
            break;
        }
    }
    assert_eq!(*session.phase(), phase);
    session
}

/// A closed loop of `n` points on a circle of `radius` at height `y`.
fn ring(n: usize, radius: f32, y: f32) -> Vec<Vec3> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            Vec3::new(radius * a.cos(), y, radius * a.sin())
        })
        .collect()
}

/// A kinematic scenery box posed where its follower starts.
fn spawn_boat(app: &mut App, follower: PathFollower, size: Vec3, lift: f32) -> Entity {
    let (pos, dir) = follower.pose();
    let at = pos + Vec3::Y * lift;
    app.world_mut()
        .spawn((
            RigidBody::Kinematic,
            Collider::cuboid(size.x, size.y, size.z),
            GameLayer::scenery(),
            Position(at),
            Rotation(mover_rotation(dir)),
            Transform::from_translation(at).with_rotation(mover_rotation(dir)),
            Mover {
                family: MoverFamily::Ferry,
                follower,
                lift,
            },
        ))
        .id()
}

fn pose(app: &App, e: Entity) -> (Vec3, Quat) {
    (
        app.world().get::<Position>(e).unwrap().0,
        app.world().get::<Rotation>(e).unwrap().0,
    )
}

#[test]
fn a_boat_circles_its_loop_without_a_jump_at_the_wrap() {
    let mut app = app_in(SessionPhase::Playing);
    let speed = 6.0;
    let follower = PathFollower::new(ring(8, 60.0, 0.0), speed).unwrap();
    let boat = spawn_boat(&mut app, follower, Vec3::new(8.0, 2.0, 20.0), 0.0);

    let (mut prev, mut prev_rot) = pose(&app, boat);
    let mut wraps = 0;
    let mut last_index = 0;
    let (mut worst_step, mut worst_turn) = (0.0_f32, 0.0_f32);
    // Just over one lap: ~377 m of ring at 6 m/s.
    for _ in 0..(70 * 60) {
        app.update();
        let (p, q) = pose(&app, boat);
        worst_step = worst_step.max(p.distance(prev));
        worst_turn = worst_turn.max(q.angle_between(prev_rot));
        (prev, prev_rot) = (p, q);
        let index = app.world().get::<Mover>(boat).unwrap().follower.index();
        if index < last_index {
            wraps += 1;
        }
        last_index = index;
    }
    assert!(wraps >= 1, "the run never crossed the path's wraparound");
    // A step is `speed·dt` of arc; the Hermite parameterisation is not
    // arc-length, so allow half again — a teleport is metres.
    let bound = STEPS_PER_UPDATE * speed * FIXED_DT * 1.5;
    assert!(worst_step <= bound, "jumped {worst_step} m (bound {bound})");
    // 8 points on a ring turn 45° per segment (~47 m): well under a
    // degree a step. A snapped heading would show as a large angle.
    assert!(worst_turn < 0.02, "heading snapped {worst_turn} rad");
}

#[test]
fn a_body_resting_on_a_moving_ferry_rides_it() {
    let mut app = app_in(SessionPhase::Playing);
    // A gentle arc (radius 300 m) at the ferry's own speed.
    let follower = PathFollower::new(ring(8, 300.0, 0.0), FERRY_SPEED).unwrap();
    let ferry = spawn_boat(&mut app, follower, Vec3::new(12.0, 1.0, 30.0), 0.0);
    let (start, _) = pose(&app, ferry);
    // A 2×1×4 crate standing on the deck: the deck top is +0.5.
    let crate_at = start + Vec3::new(0.0, 1.02, 0.0);
    let cargo = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(2.0, 1.0, 4.0),
            Position(crate_at),
            Transform::from_translation(crate_at),
        ))
        .id();

    for _ in 0..(30 * 60) {
        app.update();
    }

    let (deck, deck_rot) = pose(&app, ferry);
    let (load, _) = pose(&app, cargo);
    let travelled = deck.distance(start);
    assert!(
        travelled > 15.0,
        "the ferry barely moved: {travelled} m in 30 s"
    );
    // In the deck's frame the crate has hardly slid, and it sits on the
    // deck rather than through or above it.
    let local = deck_rot.inverse() * (load - deck);
    assert!(
        local.x.abs() < 0.5 && local.z.abs() < 0.5,
        "the crate slid on the deck: {local}"
    );
    assert!(
        (local.y - 1.0).abs() < 0.1,
        "the crate is not resting on the deck: {local}"
    );
    let speed = app.world().get::<LinearVelocity>(cargo).unwrap().0.length();
    assert!(
        (speed - FERRY_SPEED).abs() < 0.2,
        "the crate moves at {speed} m/s, the ferry at {FERRY_SPEED}"
    );
}

#[test]
fn a_platform_does_not_move_until_the_session_runs() {
    // `Ready`: the loop is posed but frozen; the rider is left alone.
    let mut app = app_in(SessionPhase::Ready);
    let follower = PathFollower::new(ring(8, 300.0, 0.0), FERRY_SPEED).unwrap();
    let ferry = spawn_boat(&mut app, follower, Vec3::new(12.0, 1.0, 30.0), 0.0);
    let (start, _) = pose(&app, ferry);
    let crate_at = start + Vec3::new(0.0, 1.02, 0.0);
    let cargo = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(2.0, 1.0, 4.0),
            Position(crate_at),
            Transform::from_translation(crate_at),
        ))
        .id();

    for _ in 0..(5 * 60) {
        app.update();
    }
    assert_eq!(
        app.world().get::<LinearVelocity>(ferry).unwrap().0,
        Vec3::ZERO
    );
    assert_eq!(pose(&app, ferry).0, start, "a waiting ferry drifted");
    let (load, _) = pose(&app, cargo);
    assert!(
        (load.x - crate_at.x).abs() < 0.05 && (load.z - crate_at.z).abs() < 0.05,
        "the crate was carried by a stopped deck: {load}"
    );

    // The countdown opens the gate: the same body now travels.
    let mut session = app.world_mut().resource_mut::<Session>();
    session.transition(SessionPhase::Countdown).unwrap();
    for _ in 0..(10 * 60) {
        app.update();
    }
    assert!(pose(&app, ferry).0.distance(start) > 3.0, "never set off");
}

#[test]
fn a_train_leaves_after_its_wait_and_turns_round_without_a_jump() {
    let mut app = app_in(SessionPhase::Playing);
    let line: Vec<Vec3> = (0..9)
        .map(|i| Vec3::new(i as f32 * 80.0, -13.0, 0.0))
        .collect();
    let motion = TrainMotion::new(&line).unwrap();
    let cars: Vec<Entity> = (0..TRAIN_CARS)
        .map(|i| {
            let (pos, dir) = motion.car_pose(i);
            app.world_mut()
                .spawn((
                    RigidBody::Kinematic,
                    Collider::cuboid(3.0, 3.0, 16.0),
                    GameLayer::scenery(),
                    Position(pos),
                    Rotation(mover_rotation(dir)),
                    Transform::from_translation(pos),
                    TrainCar,
                ))
                .id()
        })
        .collect();
    app.world_mut().spawn(Train {
        motion,
        cars: cars.clone(),
        lift: 0.0,
    });

    let first = cars[0];
    let (start, _) = pose(&app, first);
    let mut prev = start;
    let mut worst_step = 0.0_f32;
    let mut left_at = None;
    let mut reversed = false;
    let mut far_x = f32::MIN;
    for frame in 0..(90 * 60) {
        app.update();
        let (p, _) = pose(&app, first);
        worst_step = worst_step.max(p.distance(prev));
        prev = p;
        far_x = far_x.max(p.x);
        let t = (frame + 1) as f32 / 60.0;
        if left_at.is_none() && p.distance(start) > 0.5 {
            left_at = Some(t);
        }
        if far_x > 400.0 && p.x < far_x - 50.0 {
            reversed = true;
        }
    }
    let left_at = left_at.expect("the train never left");
    assert!(
        (left_at - TRAIN_WAIT).abs() < 1.0,
        "left after {left_at} s, expected the {TRAIN_WAIT} s wait"
    );
    assert!(reversed, "the train never came back from the far end");
    // 40 m/s is a third of a metre a step, and the closed-loop tangents
    // at the line's first segment run a car up to ~3.6× that on retail
    // (`retail_every_mover_path_steps_continuously`); the braking ramp
    // only slows it. A reversal that snapped a car across the line
    // would be hundreds of metres.
    let bound = STEPS_PER_UPDATE * TRAIN_SPEED * FIXED_DT * MAX_STEP_RATIO;
    assert!(worst_step <= bound, "jumped {worst_step} m (bound {bound})");
    // The coupled cars keep their gap through all of it.
    let gap = pose(&app, cars[0]).0.distance(pose(&app, cars[1]).0);
    assert!(gap > 10.0 && gap < 25.0, "cars {gap} m apart");
}

/// Every retail `<city>_{sailboat,ferry,train}*.pathset` path, stepped at
/// the fixed rate for 400 s of world time. Returns `(files, paths,
/// stepped, worst ratio of a step to speed · dt)` per object family.
fn retail_step_audit(vfs: &mm2_assets::Vfs) -> Vec<(&'static str, usize, usize, usize, f32)> {
    let mut rows = Vec::new();
    for object in ["sailboat", "ferry", "train"] {
        let (mut files, mut paths, mut stepped, mut worst) = (0, 0, 0, 0.0_f32);
        let mut names: Vec<String> = vfs
            .list()
            .into_iter()
            .filter(|p| {
                p.starts_with("race/")
                    && p.ends_with(".pathset")
                    && (p.contains(&format!("_{object}.")) || p.contains(&format!("_{object}_")))
            })
            .collect();
        names.sort();
        for file in names {
            let ps = mm2_formats::pathset::Pathset::parse(&vfs.read_logical(&file).unwrap())
                .unwrap_or_else(|e| panic!("{file} does not parse: {e}"));
            files += 1;
            for path in &ps.paths {
                paths += 1;
                let pts: Vec<Vec3> = path.points.iter().map(|p| Vec3::from(p.position)).collect();
                if pts.len() < 2 {
                    continue;
                }
                let speed = match object {
                    "train" => TRAIN_SPEED,
                    "ferry" => FERRY_SPEED,
                    // The fastest draw `sailboat_speed` can make.
                    _ => mm2_game::movers::sailboat_speed(path.spacing_metres(), 1.0),
                };
                let nominal = speed * FIXED_DT;
                let mut car_poses: Vec<Box<dyn FnMut() -> Vec3>> = Vec::new();
                if object == "train" {
                    let train =
                        std::rc::Rc::new(std::cell::RefCell::new(TrainMotion::new(&pts).unwrap()));
                    // One closure per car; the first also steps the train.
                    for i in 0..TRAIN_CARS {
                        let t = train.clone();
                        car_poses.push(Box::new(move || {
                            if i == 0 {
                                t.borrow_mut().step(FIXED_DT);
                            }
                            t.borrow().car_pose(i).0
                        }));
                    }
                } else {
                    let mut f = PathFollower::new(pts, speed).unwrap();
                    car_poses.push(Box::new(move || {
                        f.advance(FIXED_DT);
                        f.pose().0
                    }));
                }
                let mut prev: Vec<Vec3> = Vec::new();
                for pose in &mut car_poses {
                    prev.push(pose());
                }
                for _ in 0..(400 * 120) {
                    for (pose, last) in car_poses.iter_mut().zip(&mut prev) {
                        let p = pose();
                        worst = worst.max(p.distance(*last) / nominal);
                        *last = p;
                    }
                }
                stepped += 1;
            }
        }
        rows.push((object, files, paths, stepped, worst));
    }
    rows
}

/// Retail: every authored sailboat, ferry and train path steps without a
/// jump (F28-AC02), and the audit's denominators are reported — files
/// found, paths read, paths with two or more points that were stepped.
/// Skipped without the operator's install (`MM2_RETAIL=<dir>`).
#[test]
fn retail_every_mover_path_steps_continuously() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut vfs = mm2_assets::Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    for (object, files, paths, stepped, worst) in retail_step_audit(&vfs) {
        eprintln!("{object}: files={files} paths={paths} stepped={stepped} worst={worst:.2}x");
        // Both cities ship sailboats and ferries; only London has a Tube
        // (`docs/research/movers.md`). A missing file fails, not skips.
        let expected = if object == "train" { 1 } else { 2 };
        assert!(
            files >= expected,
            "{object}: expected at least {expected} files, found {files}"
        );
        assert!(stepped > 0, "{object}: no path was stepped");
        assert!(
            worst <= MAX_STEP_RATIO,
            "{object}: a step ran {worst}x its nominal length"
        );
    }
}
