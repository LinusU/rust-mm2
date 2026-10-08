//! San Francisco's cable cars (F28-A AC01): the circuits and start
//! sites a map's tram network yields, and the production
//! `drive_cable_cars` posing real Avian bodies along them — with no
//! teleport, no overlap between cars sharing a line, and nothing moving
//! before the session runs. Synthetic tram lines; the retail leg is
//! opt-in (`MM2_RETAIL=<dir>`) and reports what it audited.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::cablecar::{
    CableCar, CableCircuits, CableReport, drive_cable_cars, plan_cable_cars, spawn_cable_cars,
};
use mm2_app::traffic::{AmbientCar, AmbientDrive};
use mm2_assets::Vfs;
use mm2_formats::bai::Side;
use mm2_formats::bai::{Bai, Culling, Intersection, Road, RoadEnd, RoadSection, RoadSide};
use mm2_game::SessionEntity;
use mm2_game::cablecar::{CABLE_CRUISE_SPEED, CableMotion, CableSense};
use mm2_game::movers::mover_rotation;
use mm2_game::nav::{LaneId, LaneKind};
use mm2_game::{LaneCursor, Player, PlayerControl, PlayerId, StuckWindow};
use mm2_game::{Session, SessionPhase};

const FIXED_DT: f32 = 1.0 / 120.0;

fn side(tram: Option<Vec<[f32; 3]>>) -> RoadSide {
    let n = tram.as_ref().map_or(2, Vec::len);
    RoadSide {
        lane_count: 0,
        tram_count: u16::from(tram.is_some()),
        train_count: 0,
        sidewalk_count: 0,
        ambient_types: 0,
        lane_distances: Vec::new(),
        edge_distances: Vec::new(),
        misc: [0xCD; 40],
        lane_vertices: Vec::new(),
        tram_vertices: tram.into_iter().collect(),
        train_vertices: Vec::new(),
        sidewalk_inner: vec![[0.0; 3]; n],
        sidewalk_outer: vec![[0.0; 3]; n],
    }
}

fn end(intersection: u32, slot: u32) -> RoadEnd {
    RoadEnd {
        intersection,
        fill0: 0xCDCD,
        vehicle_rule_code: 3,
        unknown1: 0,
        intersection_road_index: slot,
        traffic_light_origin: [0.0; 3],
        traffic_light_axis: [0.0; 3],
    }
}

/// `n` roads of 100 m laid end to end along +x, each with a tram curve
/// per side (the left one a couple of metres over), joined at
/// intersections `0..=n`.
fn tram_line(n: u32) -> Bai {
    let roads = (0..n)
        .map(|k| {
            let x = k as f32 * 100.0;
            let curve = |z: f32| (0..=2).map(|i| [x + i as f32 * 50.0, 0.0, z]).collect();
            Road {
                id: k as u16,
                flags: 0,
                rooms: vec![1],
                half_width: 7.5,
                base_speed: 15.0,
                right: side(Some(curve(0.0))),
                left: side(Some(curve(2.0))),
                sections: (0..=2)
                    .map(|i| RoadSection {
                        distance: i as f32 * 50.0,
                        origin: [x + i as f32 * 50.0, 0.0, 1.0],
                        x_axis: [0.0, 0.0, 1.0],
                        y_axis: [0.0, 1.0, 0.0],
                        z_axis: [1.0, 0.0, 0.0],
                        tangent: [1.0, 0.0, 0.0],
                    })
                    .collect(),
                start: end(k, u32::from(k > 0)),
                end: end(k + 1, 0),
            }
        })
        .collect();
    let intersections = (0..=n)
        .map(|k| Intersection {
            id: k as u16,
            room: 1,
            center: [k as f32 * 100.0, 0.0, 1.0],
            roads: [k.checked_sub(1), (k < n).then_some(k)]
                .into_iter()
                .flatten()
                .collect(),
        })
        .collect();
    Bai {
        roads,
        intersections,
        culling: Culling {
            large: vec![Vec::new()],
            small: vec![Vec::new()],
        },
    }
}

#[test]
fn both_ends_of_a_line_share_one_circuit_with_a_car_at_each() {
    let bai = tram_line(3);
    let mut report = CableReport::default();
    let (circuits, starts) = plan_cable_cars(&bai, &mut report);
    assert_eq!(circuits.len(), 1);
    // Out along 0, 1, 2, round, home along 2, 1, 0: the far terminus
    // car starts on the fourth leg.
    assert_eq!(circuits[0].legs.len(), 6);
    assert_eq!(starts, vec![(0, 0), (0, 3)]);
    assert_eq!(
        (
            report.termini,
            report.circuits,
            report.no_start,
            report.stranded
        ),
        (2, 1, 0, 0)
    );
    // A leg per road end, each naming the junction it ends at.
    assert_eq!(circuits[0].junctions.len(), 6);
    let first = circuits[0].junctions[0].unwrap();
    assert_eq!((first.intersection, first.road), (1, 0));
    // 6 legs of 100 m plus the two turnarounds and four joins.
    let len = circuits[0].route.length();
    assert!((600.0..640.0).contains(&len), "circuit of {len} m");
}

#[test]
fn a_terminus_with_no_drivable_curve_or_no_way_home_is_counted_not_dropped() {
    // The far terminus' road has no left tram curve: that car cannot
    // start (it would drive the left curve), the near one still does.
    let mut bai = tram_line(3);
    bai.roads[2].left.tram_vertices.clear();
    let mut report = CableReport::default();
    let (circuits, starts) = plan_cable_cars(&bai, &mut report);
    assert_eq!((report.termini, report.no_start), (2, 1));
    assert_eq!(
        starts.len(),
        0,
        "the near car's circuit needs the left curve too"
    );
    assert_eq!((circuits.len(), report.stranded), (0, 1));

    // A three-way tram junction leaves the original no next road.
    let mut bai = tram_line(3);
    bai.intersections[1].roads = vec![0, 1, 1];
    let mut report = CableReport::default();
    let (circuits, starts) = plan_cable_cars(&bai, &mut report);
    assert!(circuits.is_empty() && starts.is_empty());
    assert!(report.stranded >= 1, "{report:?}");

    // No trams, no cars.
    let mut report = CableReport::default();
    let mut bai = tram_line(3);
    for r in &mut bai.roads {
        r.left.tram_count = 0;
        r.right.tram_count = 0;
    }
    let (circuits, starts) = plan_cable_cars(&bai, &mut report);
    assert!(circuits.is_empty() && starts.is_empty() && report.termini == 0);
}

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
        .init_resource::<CableCircuits>()
        .add_systems(FixedLast, drive_cable_cars);
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
    app.insert_resource(session);
    app.finish();
    app.cleanup();
    app
}

/// The synthetic line's circuit with a kinematic body for each car in
/// `cars` (leg, acceleration draw), posed at its start.
fn spawn_line(app: &mut App, roads: u32, cars: &[(usize, f32)]) -> Vec<Entity> {
    let bai = tram_line(roads);
    let mut report = CableReport::default();
    let (circuits, _) = plan_cable_cars(&bai, &mut report);
    let nose = 4.0;
    let ids = cars
        .iter()
        .map(|&(leg, draw)| {
            let route = &circuits[0].route;
            let motion = CableMotion::new(route, leg, draw);
            let (p, d) = route.pose(motion.s);
            app.world_mut()
                .spawn((
                    RigidBody::Kinematic,
                    Position(p),
                    Rotation(mover_rotation(d)),
                    Transform::from_translation(p).with_rotation(mover_rotation(d)),
                    CableCar {
                        circuit: 0,
                        motion,
                        lift: 0.0,
                        nose,
                        half_width: 1.25,
                    },
                ))
                .id()
        })
        .collect();
    app.insert_resource(CableCircuits(circuits));
    ids
}

fn at(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Position>(e).unwrap().0
}

fn speed_of(app: &App, e: Entity) -> f32 {
    app.world().get::<CableCar>(e).unwrap().motion.speed
}

#[test]
fn a_car_pulls_away_runs_the_whole_circuit_and_never_jumps() {
    let mut app = app_in(SessionPhase::Playing);
    let car = spawn_line(&mut app, 3, &[(0, 0.5)])[0];
    let start = at(&app, car);
    let (mut prev, mut worst, mut top) = (start, 0.0_f32, 0.0_f32);
    let mut legs = std::collections::BTreeSet::new();
    // A 600 m circuit: well over one lap in 90 s (15 m/s cruise).
    for _ in 0..(90 * 60) {
        app.update();
        let p = at(&app, car);
        worst = worst.max(p.distance(prev));
        prev = p;
        top = top.max(speed_of(&app, car));
        legs.insert(app.world().get::<CableCar>(car).unwrap().motion.leg());
    }
    // Two fixed steps per update; a step moves at most cruise·dt (the
    // turnaround curves are chords of the same arc).
    let bound = 2.0 * CABLE_CRUISE_SPEED * FIXED_DT * 1.2;
    assert!(worst <= bound, "jumped {worst} m (bound {bound})");
    assert!(worst > 0.0, "the car never moved");
    assert!((top - CABLE_CRUISE_SPEED).abs() < 0.01, "top speed {top}");
    assert_eq!(legs.len(), 6, "visited legs {legs:?}");
    // The kinematic body's velocity carries it: it is moving, in the
    // direction it faces, at the controller's speed.
    let v = app.world().get::<LinearVelocity>(car).unwrap().0.length();
    assert!((v - speed_of(&app, car)).abs() < 0.5, "velocity {v}");
}

#[test]
fn a_session_that_is_not_running_leaves_the_cars_where_they_start() {
    let mut app = app_in(SessionPhase::Ready);
    let car = spawn_line(&mut app, 3, &[(0, 0.5)])[0];
    let start = at(&app, car);
    for _ in 0..(5 * 60) {
        app.update();
    }
    assert_eq!(at(&app, car), start);
    assert_eq!(speed_of(&app, car), 0.0);
}

#[test]
fn two_cars_on_a_line_keep_their_distance_when_the_follower_is_quicker() {
    let mut app = app_in(SessionPhase::Playing);
    // Both on the first leg, the quicker (3.5 m/s²) 20 m behind the
    // slower (1.5 m/s²): without the lookahead it would run into it.
    let ids = spawn_line(&mut app, 4, &[(0, 0.0), (0, 1.0)]);
    let (lead, tail) = (ids[0], ids[1]);
    // Put the lead 20 m ahead of the tail on the route.
    {
        let mut car = app.world_mut().get_mut::<CableCar>(lead).unwrap();
        car.motion.s += 20.0;
    }
    let (mut closest, mut tail_top) = (f32::MAX, 0.0_f32);
    for _ in 0..(60 * 60) {
        app.update();
        let (a, b) = (
            app.world().get::<CableCar>(lead).unwrap(),
            app.world().get::<CableCar>(tail).unwrap(),
        );
        let route = &app.world().resource::<CableCircuits>().0[0].route;
        // Tail's nose to the lead's tail, along the route.
        let gap = route.gap_ahead(b.motion.s, a.motion.s) - a.nose - b.nose;
        if gap < route.length() / 2.0 {
            closest = closest.min(gap);
        }
        tail_top = tail_top.max(b.motion.speed);
    }
    assert!(
        closest > 2.0,
        "the follower closed to {closest} m of the car ahead"
    );
    assert!(tail_top > 5.0, "the follower never got going: {tail_top}");
}

/// A kinematic body standing `ahead` metres past the car's nose on the
/// rails (`aside` metres off them), tagged as `who`.
fn stand_ahead(app: &mut App, car: Entity, ahead: f32, aside: f32, who: impl Bundle) -> Entity {
    let (s, nose) = {
        let c = app.world().get::<CableCar>(car).unwrap();
        (c.motion.s, c.nose)
    };
    let (p, d) = app.world().resource::<CableCircuits>().0[0]
        .route
        .pose(s + nose + ahead);
    let p = p + Vec3::Y.cross(d).normalize() * aside;
    app.world_mut()
        .spawn((
            who,
            RigidBody::Static,
            Position(p),
            Transform::from_translation(p),
        ))
        .id()
}

fn player() -> Player {
    Player {
        id: PlayerId(0),
        control: PlayerControl::Local,
    }
}

fn ambient() -> AmbientCar {
    AmbientCar {
        class: 0,
        drive: AmbientDrive::Knocked,
        cursor: LaneCursor::new(
            LaneId {
                road: 0,
                side: Side::Right,
                index: 0,
                kind: LaneKind::Vehicle,
            },
            0.0,
        ),
        target_speed: 0.0,
        speed: 0.0,
        stuck: StuckWindow::new([0.0; 3]),
    }
}

/// Run `seconds` and return the car's speed afterwards and its nose's
/// distance to `body` along the route.
fn settle(app: &mut App, car: Entity, seconds: u32) -> f32 {
    for _ in 0..(seconds * 60) {
        app.update();
    }
    speed_of(app, car)
}

fn nose_to(app: &App, car: Entity, body: Entity) -> f32 {
    let c = app.world().get::<CableCar>(car).unwrap();
    (at(app, body) - at(app, car)).length() - c.nose
}

#[test]
fn a_car_brakes_for_the_player_on_the_rails_and_goes_on_when_it_leaves() {
    let mut app = app_in(SessionPhase::Playing);
    let car = spawn_line(&mut app, 4, &[(0, 0.5)])[0];
    // Let it get up to speed, then put the player 28 m ahead of the nose.
    settle(&mut app, car, 8);
    let top = speed_of(&app, car);
    assert!((top - CABLE_CRUISE_SPEED).abs() < 0.01, "speed {top}");
    let body = stand_ahead(&mut app, car, 28.0, 0.0, player());
    let speed = settle(&mut app, car, 12);
    assert_eq!(speed, 0.0, "the car should be at rest");
    // Constant-rate braking to 2.5 m behind the blocker's front edge
    // (its centre is 2 m further): the nose stops 4.5 m short, give or
    // take the sensor's 1 m grain and the step error.
    let short = nose_to(&app, car, body);
    assert!((3.4..=5.6).contains(&short), "stopped {short} m short");
    // It stays put while the player is there.
    assert_eq!(settle(&mut app, car, 5), 0.0);
    app.world_mut().despawn(body);
    let speed = settle(&mut app, car, 10);
    assert!(speed > 10.0, "the car did not resume: {speed} m/s");
}

#[test]
fn an_ambient_car_on_the_rails_stops_the_car_too() {
    let mut app = app_in(SessionPhase::Playing);
    let car = spawn_line(&mut app, 4, &[(0, 0.5)])[0];
    let body = stand_ahead(&mut app, car, 24.0, 0.0, ambient());
    let speed = settle(&mut app, car, 15);
    assert_eq!(speed, 0.0);
    let short = nose_to(&app, car, body);
    assert!(short > 3.0, "the nose is {short} m from the car");
}

#[test]
fn a_body_beside_or_behind_the_rails_does_not_stop_the_car() {
    let mut app = app_in(SessionPhase::Playing);
    let car = spawn_line(&mut app, 4, &[(0, 0.5)])[0];
    // Well to the side of the line, and the other lane of the road.
    stand_ahead(&mut app, car, 20.0, 8.0, player());
    stand_ahead(&mut app, car, 20.0, -8.0, ambient());
    let speed = settle(&mut app, car, 8);
    assert!(
        (speed - CABLE_CRUISE_SPEED).abs() < 0.01,
        "a bystander slowed the car to {speed}"
    );
}

/// Retail: the circuits the original's init gives San Francisco's cars,
/// driven end to end with the controller (clear road, green lights):
/// continuous, bounded, and every leg reached. Reports its denominators.
/// Skipped without the operator's install (`MM2_RETAIL=<dir>`).
#[test]
fn retail_every_cable_car_runs_its_whole_circuit_without_a_jump() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut vfs = mm2_assets::Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let audit = |city: &str| {
        let (bytes, _) = vfs.read_path(&format!("city/{city}.bai")).unwrap();
        let bai = Bai::parse(&bytes).unwrap();
        let mut report = CableReport::default();
        let (circuits, starts) = plan_cable_cars(&bai, &mut report);
        (report, circuits, starts)
    };
    let (london, _, london_cars) = audit("london");
    assert_eq!(london.termini, 0, "London has no cable cars");
    assert!(london_cars.is_empty());

    let (sf, circuits, starts) = audit("sf");
    eprintln!(
        "sf: termini={} circuits={} cars={} no_start={} stranded={}",
        sf.termini,
        sf.circuits,
        starts.len(),
        sf.no_start,
        sf.stranded
    );
    assert_eq!((sf.termini, sf.no_start, sf.stranded), (4, 0, 0));
    assert_eq!((circuits.len(), starts.len()), (2, 4));
    for (ci, leg) in starts {
        let route = &circuits[ci].route;
        let mut car = CableMotion::new(route, leg, 0.5);
        let (mut prev, mut worst) = (route.pose(car.s).0, 0.0_f32);
        let (mut prev_dir, mut worst_turn) = (route.pose(car.s).1, 0.0_f32);
        let mut legs = std::collections::BTreeSet::new();
        let dt = FIXED_DT;
        // One and a half laps at cruise.
        let steps = (1.5 * route.length() / CABLE_CRUISE_SPEED / dt) as usize + 1000;
        for _ in 0..steps {
            car.step(route, dt, 4.0, CableSense::CLEAR);
            let (p, d) = route.pose(car.s);
            worst = worst.max(p.distance(prev));
            worst_turn = worst_turn.max(d.angle_between(prev_dir));
            (prev, prev_dir) = (p, d);
            legs.insert(car.leg());
        }
        eprintln!(
            "circuit {ci} leg {leg}: {:.0} m, worst step {worst:.3} m, worst turn {worst_turn:.3} rad, legs {}",
            route.length(),
            legs.len()
        );
        assert!(worst <= CABLE_CRUISE_SPEED * dt * 1.5, "jumped {worst} m");
        // The tightest bend is a terminus turnaround; a snapped heading
        // would be a large fraction of a radian in one step.
        assert!(worst_turn < 0.15, "heading snapped {worst_turn} rad");
        assert_eq!(legs.len(), route.legs().len(), "not every leg was driven");
    }
}

/// Retail: with every light red until the car has stopped at it, a car
/// stops at the end of each of its roads — all of them, all the way
/// round — with its nose short of the line and never past it.
/// Skipped without the operator's install (`MM2_RETAIL=<dir>`).
#[test]
fn retail_a_car_stops_at_the_end_of_every_road_on_red() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut vfs = mm2_assets::Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let (bytes, _) = vfs.read_path("city/sf.bai").unwrap();
    let bai = Bai::parse(&bytes).unwrap();
    let (circuits, starts) = plan_cable_cars(&bai, &mut CableReport::default());
    let nose = 4.0;
    let mut stops = 0;
    for (ci, leg) in starts {
        let route = &circuits[ci].route;
        let mut car = CableMotion::new(route, leg, 0.25);
        let mut red = true;
        let mut served = std::collections::BTreeSet::new();
        let mut last = usize::MAX;
        for _ in 0..(20 * 60 * 120) {
            let sense = CableSense {
                gate_open: !red,
                obstacle: None,
            };
            car.step(route, FIXED_DT, nose, sense);
            let short = route.legs()[car.leg()].line - (car.s + nose);
            if red && car.speed == 0.0 && !car.cleared() && short < 2.0 {
                // Stopped on red: nose short of its road's line.
                assert!(
                    (-0.01..=0.7).contains(&short),
                    "leg {}: stopped {short} m from the line",
                    car.leg()
                );
                served.insert(car.leg());
                (stops, red, last) = (stops + 1, false, car.leg());
            } else if !red && car.leg() != last {
                // On to the next road: red again.
                red = true;
            }
            if served.len() == route.legs().len() {
                break;
            }
        }
        assert_eq!(
            served.len(),
            route.legs().len(),
            "circuit {ci} car at leg {leg}"
        );
    }
    eprintln!("stopped on red {stops} times across 4 cars");
}

/// Run the production spawn against `vfs` and report what it made.
fn spawn_into_world(vfs: &Vfs, city: &str, eligible: bool) -> (CableReport, usize, usize) {
    let mut world = World::new();
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let mut meshes: Assets<Mesh> = Assets::default();
    let mut images: Assets<Image> = Assets::default();
    let mut materials: Assets<StandardMaterial> = Assets::default();
    let report = {
        let mut commands = Commands::new(&mut queue, &world);
        spawn_cable_cars(
            &mut commands,
            vfs,
            city,
            eligible,
            7,
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(1),
        )
    };
    queue.apply(&mut world);
    let cars = world.query::<&CableCar>().iter(&world).count();
    for car in world.query::<&CableCar>().iter(&world) {
        // The body the obstacle sensor measures: a tram, not the
        // fallback sizes.
        eprintln!("cable car nose {} half-width {}", car.nose, car.half_width);
        assert!((1.0..=12.0).contains(&car.nose), "nose {}", car.nose);
        assert!(
            (0.5..=4.0).contains(&car.half_width),
            "half-width {}",
            car.half_width
        );
    }
    let circuits = world.resource::<CableCircuits>().0.len();
    (report, cars, circuits)
}

#[test]
fn a_session_with_no_map_or_a_broken_one_or_a_network_peer_spawns_no_cars() {
    // No `city/sf.bai` at all.
    let dir = tempfile::tempdir().unwrap();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    let (report, cars, circuits) = spawn_into_world(&vfs, "sf", true);
    assert_eq!((cars, circuits, report.termini), (0, 0, 0));
    assert!(report.eligible && !report.bai_unreadable);

    // A map that does not parse is reported, not papered over.
    std::fs::create_dir_all(dir.path().join("city")).unwrap();
    std::fs::write(dir.path().join("city/sf.bai"), b"not a map").unwrap();
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir.path(), 0).unwrap();
    let (report, cars, _) = spawn_into_world(&vfs, "sf", true);
    assert_eq!(cars, 0);
    assert!(report.bai_unreadable, "{report:?}");

    // A networked session is not eligible: its stops would follow this
    // process's signal clock. Nothing is read, nothing spawned.
    let (report, cars, circuits) = spawn_into_world(&vfs, "sf", false);
    assert_eq!((cars, circuits), (0, 0));
    assert!(!report.eligible && !report.bai_unreadable);
}

/// Retail: the production spawn puts four `va_cablecar_f` bodies on San
/// Francisco's two circuits with every texture resolved, none in London,
/// and none for an ineligible session. Skipped without the operator's
/// install (`MM2_RETAIL=<dir>`).
#[test]
fn retail_the_production_spawn_fields_four_cars_in_san_francisco_and_none_in_london() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    let (sf, cars, circuits) = spawn_into_world(&vfs, "sf", true);
    eprintln!("sf spawn: {sf:?} bodies={cars} circuits={circuits}");
    assert_eq!((sf.cars, cars, circuits), (4, 4, 2));
    assert!(!sf.model_missing && !sf.bai_unreadable);
    assert_eq!(sf.missing_textures, 0);
    let (london, cars, circuits) = spawn_into_world(&vfs, "london", true);
    assert_eq!((london.termini, cars, circuits), (0, 0, 0));
    let (_, cars, _) = spawn_into_world(&vfs, "sf", false);
    assert_eq!(cars, 0);
}
