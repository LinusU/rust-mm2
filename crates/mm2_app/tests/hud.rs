//! F22-A.3: the documented `H` HUD toggle (HUD-3/CTL-1 "Toggle HUD
//! (Heads-Up Display)").
//!
//! The unit legs exercise the phase gating plus the layer contract the
//! recovered `mmHUD` layout shows — one gate suppresses the whole
//! driving HUD: the instrument line, the race banners/nav arrow, the
//! HUD map's corner views, the opponent indicators and the cockpit
//! dash cluster, each keeping its own state and re-showing on
//! re-enable. The pause map, `ErrorText` and the mirror strip are
//! outside the layer and stay up. The smoke legs run the real
//! `headless_smoke` pipeline so the record carries `hud=off` from
//! `--no-hud` and stays field-free otherwise.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_app::camera::CameraMode;
use mm2_app::dash::{self, CockpitPart};
use mm2_app::hud::{self, HudVisible};
use mm2_app::hudmap::{HudMapCamera, HudMapReport};
use mm2_app::navarrow::{NavArrow, NavArrowSprites};
use mm2_app::oppind::{OppIndReport, OppIndicator, OpponentIndicators};
use mm2_app::race::{CountdownBanner, CountdownBannerText, LOW_TIME_TICKS, LowTimeWarning};
use mm2_app::session::{self, SelectedCar};
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_formats::hudmap::HudMapSpec;
use mm2_game::{
    Checkpoint, CheckpointRule, DevOverrides, EventParams, Player, PlayerControl, RaceDefinition,
    RacePhase, RaceProgress, RaceStart, RaceState, ResultLedger, Session, SessionConfig,
    SessionEntity, SessionPhase, TargetSelection, despawn_session_entities,
};
use mm2_vehicle::{Vehicle, VehicleConfig};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn session_at(phase: SessionPhase) -> Session {
    use SessionPhase::*;
    let mut s = Session::new();
    if phase == Menu {
        return s;
    }
    s.begin(SessionConfig::default()).unwrap(); // Loading
    let path: &[SessionPhase] = match phase {
        Ready => &[Ready],
        Countdown => &[Ready, Countdown],
        Playing => &[Ready, Countdown, Playing],
        Paused => &[Ready, Countdown, Playing, Paused],
        Results => &[Ready, Countdown, Playing, Results],
        other => panic!("no session_at path to {other:?}"),
    };
    for step in path {
        s.transition(step.clone()).unwrap();
    }
    s
}

fn hud_app(phase: SessionPhase) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<HudVisible>()
        .insert_resource(session_at(phase))
        .add_systems(Update, hud::hud_input);
    app
}

fn press(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
}

fn transition(app: &mut App, to: SessionPhase) {
    app.world_mut()
        .resource_mut::<Session>()
        .transition(to)
        .unwrap();
}

fn set_hud(app: &mut App, on: bool) {
    app.world_mut().resource_mut::<HudVisible>().0 = on;
}

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

fn any_order_def() -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, -60.0), cp(100.0, 0.0)],
        finish: None,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: Some(60 * mm2_game::RACE_TICK_HZ),
        params: EventParams::default(),
        countdown_ticks: 0,
        start_slots: vec![RaceStart {
            position: Vec3::ZERO,
            yaw_deg: Some(0.0),
        }],
    }
}

const TUNE: &str = "type: a\nmmHudMap {\n  Size 0.21 0.25\n  Pos 0.78 0.75\n  ZoomIn 0\n  Approach Rate 1.2\n  ZoomInDist 350\n  ZoomOutDist 700\n  IconScaleMin 10\n  IconScaleMax 20\n  ZoomInDistFS 400\n  ZoomOutDistFS 900\n  IconScaleMinFS 8\n  IconScaleMaxFS 14\n  Ocean Color 0.1 0.5 0.8\n}\n";

// ---------------------------------------------------------------------------
// Toggle gating (HUD-3/CTL-1: `H` in the live phases only)
// ---------------------------------------------------------------------------

#[test]
fn h_toggles_only_in_the_live_phases() {
    let mut app = hud_app(SessionPhase::Menu);

    // Menu keeps the key for its own owner.
    press(&mut app, KeyCode::KeyH);
    assert!(app.world().resource::<HudVisible>().0);

    // Countdown and Playing toggle.
    transition(&mut app, SessionPhase::Loading);
    transition(&mut app, SessionPhase::Ready);
    transition(&mut app, SessionPhase::Countdown);
    press(&mut app, KeyCode::KeyH);
    assert!(!app.world().resource::<HudVisible>().0);
    transition(&mut app, SessionPhase::Playing);
    press(&mut app, KeyCode::KeyH);
    assert!(app.world().resource::<HudVisible>().0);

    // Paused and Results keep the key — the same contract
    // `mirror_input` holds for BACKSPACE.
    transition(&mut app, SessionPhase::Paused);
    press(&mut app, KeyCode::KeyH);
    assert!(app.world().resource::<HudVisible>().0);
    transition(&mut app, SessionPhase::Playing);
    transition(&mut app, SessionPhase::Results);
    press(&mut app, KeyCode::KeyH);
    assert!(app.world().resource::<HudVisible>().0);
}

// ---------------------------------------------------------------------------
// The instrument line: computes always, draws only under the gate
// ---------------------------------------------------------------------------

#[test]
fn the_instrument_line_hides_with_the_layer() {
    let mut app = hud_app(SessionPhase::Playing);
    app.init_resource::<ResultLedger>()
        .add_systems(Update, hud::update_hud);
    let line = app
        .world_mut()
        .spawn((
            SessionEntity(1),
            session::Hud,
            Text::new(""),
            Visibility::Visible,
        ))
        .id();
    // `ErrorText` is a load-failure surface, not a HUD instrument —
    // the gate must never touch it.
    let err = app
        .world_mut()
        .spawn((
            SessionEntity(1),
            session::ErrorText,
            Text::new(""),
            Visibility::Visible,
        ))
        .id();

    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(line).unwrap(),
        Visibility::Visible
    );
    assert_eq!(
        app.world().get::<Text>(line).unwrap().0,
        "no vehicle",
        "the line keeps computing while the gate is up"
    );

    set_hud(&mut app, false);
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(line).unwrap(),
        Visibility::Hidden,
        "H off suppresses the instrument line"
    );
    assert_eq!(
        *app.world().get::<Visibility>(err).unwrap(),
        Visibility::Visible,
        "the error surface is not a HUD instrument"
    );

    set_hud(&mut app, true);
    app.update();
    assert_eq!(
        *app.world().get::<Visibility>(line).unwrap(),
        Visibility::Visible
    );
}

// ---------------------------------------------------------------------------
// The race instruments: arrow, countdown banner, low-time warning
// ---------------------------------------------------------------------------

#[test]
fn the_race_instruments_hide_with_the_layer() {
    let mut app = hud_app(SessionPhase::Playing);
    let generation = app.world().resource::<Session>().generation();
    app.add_systems(
        Update,
        (
            mm2_app::navarrow::update_nav_arrow,
            mm2_app::race::update_countdown_banner,
            mm2_app::race::update_race_warning,
        ),
    );

    // A live race still counting down: the banner's digit and the nav
    // arrow's gate target want to show. (`GO!` needs a fresh `Running`
    // clock while the low-time pulse needs a spent one, so each
    // instrument gets the leg that raises it — the gate line is the
    // same `&& hud.0` in all three systems.)
    let def = any_order_def();
    let mut race = RaceState::new(def.clone(), generation);
    race.phase = RacePhase::Countdown { remaining: 60 };
    app.insert_resource(race);

    let banner = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            CountdownBanner,
            Visibility::Hidden,
        ))
        .id();
    app.world_mut()
        .spawn((CountdownBannerText, Text::new(""), TextColor(Color::WHITE)));
    let arrow = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            NavArrow,
            NavArrowSprites {
                ahead: Handle::default(),
                behind: Handle::default(),
            },
            UiTransform::default(),
            ImageNode::default(),
            Visibility::Hidden,
        ))
        .id();
    let warning = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            LowTimeWarning,
            TextColor(Color::WHITE),
            Visibility::Hidden,
        ))
        .id();
    // The local participant, still racing, with a live target ahead.
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    app.world_mut().spawn((
        SessionEntity(generation),
        Player {
            id,
            control: PlayerControl::Local,
        },
        RaceProgress::new(&def),
        TargetSelection::default(),
        Position(Vec3::ZERO),
        Rotation::default(),
    ));

    let vis_of = |app: &App, e: Entity| *app.world().get::<Visibility>(e).unwrap();

    // Leg A — the countdown digit and the arrow point at gate 0.
    app.update();
    assert_eq!(vis_of(&app, banner), Visibility::Visible);
    assert_eq!(vis_of(&app, arrow), Visibility::Visible);

    set_hud(&mut app, false);
    app.update();
    assert_eq!(vis_of(&app, banner), Visibility::Hidden);
    assert_eq!(vis_of(&app, arrow), Visibility::Hidden);

    set_hud(&mut app, true);
    app.update();
    assert_eq!(vis_of(&app, banner), Visibility::Visible);
    assert_eq!(vis_of(&app, arrow), Visibility::Visible);

    // Leg B — the race is running with a nearly-spent clock: the
    // low-time pulse raises, the arrow keeps pointing.
    {
        let mut race = app.world_mut().resource_mut::<RaceState>();
        race.phase = RacePhase::Running;
        race.clock = race.definition.time_limit_ticks.unwrap() as u64 - LOW_TIME_TICKS as u64;
    }
    app.update();
    assert_eq!(vis_of(&app, arrow), Visibility::Visible);
    assert_eq!(vis_of(&app, warning), Visibility::Visible);

    set_hud(&mut app, false);
    app.update();
    assert_eq!(vis_of(&app, arrow), Visibility::Hidden);
    assert_eq!(vis_of(&app, warning), Visibility::Hidden);

    set_hud(&mut app, true);
    app.update();
    assert_eq!(
        vis_of(&app, warning),
        Visibility::Visible,
        "each instrument keeps its own state and re-shows on re-enable"
    );
}

// ---------------------------------------------------------------------------
// The HUD map's corner views vs the full-screen pause map
// ---------------------------------------------------------------------------

#[test]
fn the_map_camera_parks_with_the_layer() {
    let mut app = hud_app(SessionPhase::Playing);
    let generation = app.world().resource::<Session>().generation();
    app.insert_resource(mm2_game::HudMap::new(
        HudMapSpec::parse(TUNE).unwrap(),
        generation,
    ));
    app.insert_resource(HudMapReport {
        spec_path: "tune/test.mmhudmap".into(),
        pkg_path: "geometry/hudmap_test.pkg".into(),
        tiles: 1,
        markers: 1,
        absent: None,
        dot_materials: Vec::new(),
        marker_y: 5.0,
    });
    app.add_systems(Update, mm2_app::hudmap::drive_hud_map);

    let map_cam = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            HudMapCamera,
            Camera3d::default(),
            Camera {
                is_active: false,
                ..default()
            },
            Projection::Orthographic(OrthographicProjection::default_3d()),
            Transform::default(),
        ))
        .id();
    // The local participant the camera tracks.
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    app.world_mut().spawn((
        SessionEntity(generation),
        Player {
            id,
            control: PlayerControl::Local,
        },
        GlobalTransform::IDENTITY,
    ));

    let active = |app: &mut App| app.world().get::<Camera>(map_cam).unwrap().is_active;

    app.update();
    assert!(active(&mut app), "the inset view is up with the layer");

    set_hud(&mut app, false);
    app.update();
    assert!(
        !active(&mut app),
        "the corner map suppresses with the layer — mmHudMap is an mmHUD member"
    );

    // The full-screen map is the pause overlay's surface, not a
    // driving instrument: it still opens while the layer is off.
    app.world_mut()
        .resource_mut::<mm2_game::HudMap>()
        .fullscreen = true;
    app.update();
    assert!(active(&mut app), "the pause map is exempt from the gate");

    app.world_mut()
        .resource_mut::<mm2_game::HudMap>()
        .fullscreen = false;
    set_hud(&mut app, true);
    app.update();
    assert!(active(&mut app));
}

// ---------------------------------------------------------------------------
// The opponent indicators and the cockpit dash — mmHUD members too
// ---------------------------------------------------------------------------

#[test]
fn the_indicator_markers_hide_with_the_layer() {
    let mut app = hud_app(SessionPhase::Playing);
    app.init_resource::<OpponentIndicators>()
        .insert_resource(OppIndReport {
            markers: 1,
            bound: 0,
            absent: None,
        })
        .add_systems(Update, mm2_app::oppind::drive_opponent_indicators);

    let marker = app
        .world_mut()
        .spawn((
            SessionEntity(1),
            OppIndicator,
            Transform::default(),
            Visibility::Hidden,
        ))
        .id();
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            is_active: true,
            ..default()
        },
        Transform::from_xyz(0.0, 30.0, 60.0),
        GlobalTransform::from(Transform::from_xyz(0.0, 30.0, 60.0)),
    ));
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    app.world_mut().spawn((
        SessionEntity(1),
        Player {
            id,
            control: PlayerControl::Ai,
        },
        Vehicle {
            config: VehicleConfig::default(),
        },
        Transform::default(),
        GlobalTransform::IDENTITY,
    ));

    let vis = |app: &mut App| *app.world().get::<Visibility>(marker).unwrap();

    app.update();
    assert_eq!(vis(&mut app), Visibility::Visible);

    set_hud(&mut app, false);
    app.update();
    assert_eq!(
        vis(&mut app),
        Visibility::Hidden,
        "the original draws its indicators inside mmHUD — they suppress with the layer"
    );
    assert_eq!(
        app.world().resource::<OppIndReport>().bound,
        1,
        "`bound` still reports live demand while the layer is off"
    );

    set_hud(&mut app, true);
    app.update();
    assert_eq!(vis(&mut app), Visibility::Visible);
}

#[test]
fn the_dash_cluster_hides_with_the_layer() {
    let mut app = hud_app(SessionPhase::Playing);
    app.insert_resource(CameraMode::Cockpit)
        .add_systems(Update, dash::sync_dash_visibility);

    let cluster = app
        .world_mut()
        .spawn((CockpitPart, Visibility::Visible, Transform::default()))
        .id();
    let exterior = app
        .world_mut()
        .spawn((Visibility::Visible, Transform::default()))
        .id();
    let player = app
        .world_mut()
        .spawn((
            mm2_game::PlayerVehicle,
            Visibility::Visible,
            Transform::default(),
        ))
        .id();
    app.world_mut().entity_mut(player).add_child(cluster);
    app.world_mut().entity_mut(player).add_child(exterior);

    let vis = |app: &mut App, e: Entity| *app.world().get::<Visibility>(e).unwrap();

    app.update();
    assert_eq!(vis(&mut app, cluster), Visibility::Visible);
    assert_eq!(vis(&mut app, exterior), Visibility::Hidden);

    set_hud(&mut app, false);
    app.update();
    assert_eq!(
        vis(&mut app, cluster),
        Visibility::Hidden,
        "mmDashView is an mmHUD member — the cluster suppresses with the layer"
    );
    assert_eq!(
        vis(&mut app, exterior),
        Visibility::Hidden,
        "the cockpit split still owns the exterior either way"
    );

    set_hud(&mut app, true);
    app.update();
    assert_eq!(vis(&mut app, cluster), Visibility::Visible);
}

// ---------------------------------------------------------------------------
// Lifecycle + record
// ---------------------------------------------------------------------------

/// The toggle is session-agnostic like `RearView`: teardown owns the
/// `SessionEntity` roots, not the driver's choice — a restart respawns
/// the HUD entities and the drivers re-apply the same state.
#[test]
fn the_gate_survives_session_teardown() {
    let mut app = hud_app(SessionPhase::Playing);
    app.add_systems(Update, despawn_session_entities.run_if(session::unloading));
    let root = app
        .world_mut()
        .spawn((SessionEntity(1), Visibility::Visible))
        .id();
    set_hud(&mut app, false);

    transition(&mut app, SessionPhase::Unloading);
    app.update();

    assert!(
        app.world().get_entity(root).is_err(),
        "teardown owns the session roots"
    );
    assert!(
        !app.world().resource::<HudVisible>().0,
        "the driver's choice survives the teardown — session-agnostic"
    );
}

/// `--no-hud` starts the layer off and the record says so; a default
/// run prints no `hud` field so every earlier record stays
/// bit-identical.
#[test]
fn no_hud_records_the_gate() {
    let rec = smoke::headless_smoke(
        &SessionConfig {
            dev: DevOverrides {
                no_hud: true,
                ..DevOverrides::default()
            },
            ..SessionConfig::default()
        },
        Vfs::new(),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        60,
        smoke::Driver::Parked,
        None,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "expected pass, got: {}",
        rec.line()
    );
    assert!(
        rec.line().contains(" hud=off"),
        "the suppressed layer reports on the record: {}",
        rec.line()
    );

    let rec = smoke::headless_smoke(
        &SessionConfig::default(),
        Vfs::new(),
        SelectedCar {
            def: None,
            paint: 0,
        },
        &VehicleConfig::default(),
        60,
        smoke::Driver::Parked,
        None,
    );
    assert!(
        !rec.line().contains(" hud="),
        "the default-on gate prints no field: {}",
        rec.line()
    );
}
