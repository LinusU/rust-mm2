//! F01-C session-lifecycle integration: start, quit, failure and restart
//! through the production spawn/teardown systems — the same
//! `load_session_world`/`drive_session`/`despawn_session_entities` wiring
//! the `mm2` binary schedules, driven headlessly under real Avian physics.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::camera::{CameraMode, ChaseCamera};
use mm2_app::contracts::{self, ImpactFilter};
use mm2_app::hudmap;
use mm2_app::pause::{self, PauseMenu, PauseUi};
use mm2_app::results::{self, ResultsMenu};
use mm2_app::session::{
    self, ErrorText, Hud, SelectedCar, SessionControl, SessionNote, SpawnPoint, TunedVehicle,
};
use mm2_app::settings::{
    Antialiasing, GraphicsSettings, SettingsFile, ShadowQuality, settings_path,
};
use mm2_assets::Vfs;
use mm2_formats::hudmap::HudMapSpec;
use mm2_game::{
    BangerPool, DEFAULT_ACTIVE_POOL, DamageSignals, DevOverrides, HudMap, ImpactEvent, ImpactId,
    MapOrientation, MapView, Mm2Vfs, ObjectId, ObjectIdentity, PlayerVehicle, Session,
    SessionAuthority, SessionConfig, SessionEntity, SessionPhase, SpawnPose, WorldMode,
    advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{
    ResetAuthority, ResetVehicle, Teleported, Vehicle, VehicleConfig, VehicleInput, VehiclePlugin,
    VehicleState,
};

use crate::support;

/// A headless app wired exactly like the binary's session path: real
/// world spawning, teardown and contract pipeline, minus the window and
/// device input (tests drive `SessionControl`/`VehicleInput` directly).
fn test_app(config: SessionConfig, frame_secs: f64) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            frame_secs,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session)
        .insert_resource(Mm2Vfs(Vfs::new()))
        .insert_resource(TunedVehicle(VehicleConfig::default()))
        .insert_resource(SelectedCar {
            def: None,
            paint: 0,
        })
        .insert_resource(SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
        .insert_resource(CameraMode::Chase)
        // `load_session_world` fills the real asset stores; without render
        // plugins the stores are inert collections — enough to spawn.
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .init_resource::<SessionControl>()
        .init_resource::<SessionNote>()
        .init_resource::<PauseMenu>()
        .init_resource::<ResultsMenu>()
        .init_resource::<mm2_app::camera::RearView>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                contracts::publish_vehicle_telemetry,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                // The binary's `R`-reset reader — pad North reaches the
                // same `spawn_resets` bundle (F22-AC06's designed map).
                // Same ordering edge as the binary: every Update
                // `ResetVehicle` writer precedes the apply.
                mm2_app::input::reset_input.before(mm2_vehicle::systems::vehicle_reset),
                // F22-B.2: the binary's mirror toggle — Backspace must
                // reach `RearView`, not the restart intent, in every
                // phase the harness exercises — and the strip's
                // `is_active` follows like the binary's driver does.
                mm2_app::camera::mirror_input,
                mm2_app::camera::drive_mirror,
                // F22-A.1: same map/pause ordering as the binary — the
                // map's controls run ahead of `pause_input` so the
                // Q/Esc that closes a pause-map is never re-read as a
                // menu key, and its pause intent lands the same update.
                hudmap::hudmap_input
                    .after(session::session_control_input)
                    .before(pause::pause_input)
                    .before(session::drive_session),
                // Same ordering contract as the binary: pause owns
                // `Paused`, running between the intent reader (which
                // ignores `Paused`) and the driver.
                pause::pause_input
                    .after(session::session_control_input)
                    .before(session::drive_session),
                // `Results` input has the same ownership contract.
                results::results_input
                    .after(session::session_control_input)
                    .before(session::drive_session),
                (
                    despawn_session_entities.run_if(session::unloading),
                    pause::dev_pause_once,
                    hudmap::dev_pause_map_once,
                    results::dev_finish_once,
                    // F22-B.5: same ordering as the binary — the
                    // scheduled `R`-reset bundle lands ahead of the
                    // driver.
                    session::dev_reset_at,
                    session::drive_session,
                )
                    .chain()
                    // The binary orders every Update `ResetVehicle`
                    // writer (`dev_reset_at` here) ahead of the apply.
                    .before(mm2_vehicle::systems::vehicle_reset),
                // F25-B: the binary's trailer-reseat follower — between
                // the writers and the apply so a rig teleports as one.
                session::reseat_towed_trailers
                    .after(mm2_vehicle::systems::vehicle_self_right)
                    .after(mm2_app::input::reset_input)
                    .after(session::dev_reset_at)
                    .before(mm2_vehicle::systems::vehicle_reset),
                // Phase mirrors run after the driver — the update that
                // enters/leaves `Paused` sees the settled phase.
                pause::sync_physics_pause.after(session::drive_session),
                pause::pause_present.after(session::drive_session),
                results::results_present.after(session::drive_session),
            ),
        );
    app.finish();
    app.cleanup();
    app
}

fn dev_app() -> App {
    test_app(SessionConfig::default(), 1.0 / 60.0)
}

fn count<F: QueryFilter>(app: &mut App) -> usize {
    let world = app.world_mut();
    world.query_filtered::<Entity, F>().iter(world).count()
}

fn single<F: QueryFilter>(app: &mut App) -> Entity {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, F>()
        .single(world)
        .expect("expected exactly one match")
}

fn drain_impacts(app: &mut App) -> Vec<ImpactEvent> {
    app.world_mut()
        .resource_mut::<Messages<ImpactEvent>>()
        .drain()
        .collect()
}

/// Run updates until `pred` holds or `max` updates pass; returns whether
/// the predicate was reached.
fn run_until(app: &mut App, max: usize, mut pred: impl FnMut(&mut App) -> bool) -> bool {
    for _ in 0..max {
        app.update();
        if pred(app) {
            return true;
        }
    }
    false
}

fn phase_is(app: &mut App, phase: SessionPhase) -> bool {
    *app.world().resource::<Session>().phase() == phase
}

/// Press a key for exactly one update (no InputPlugin runs here, so the
/// input state is managed by hand). `reset_all`, not `clear` — `clear`
/// keeps `pressed`, so the same key would never re-fire `just_pressed`.
fn press_key(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
}

/// One pad-button edge through the first pad's `digital_mut` (bevy's
/// documented gamepad mocking surface), `press_key`-equivalent.
fn pad_press(app: &mut App, button: GamepadButton) {
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().press(button);
    }
    app.update();
    let mut pads = app.world_mut().query::<&mut Gamepad>();
    for mut pad in pads.iter_mut(app.world_mut()) {
        pad.digital_mut().reset_all();
    }
}

fn physics_paused(app: &mut App) -> bool {
    app.world().resource::<Time<Physics>>().is_paused()
}

fn pause_rows(app: &mut App) -> usize {
    let world = app.world_mut();
    world
        .query_filtered::<Entity, (With<PauseUi>, Without<ChildOf>)>()
        .iter(world)
        .count()
}

/// F17-B.1: `Esc` pauses a live session — the phase lands on `Paused`,
/// the physics clock stops, the overlay owns the screen, and the world
/// holds perfectly still until `Esc` resumes it.
#[test]
fn esc_pauses_then_resumes_a_frozen_world() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    // Let the car settle so a frozen position is meaningful.
    for _ in 0..30 {
        app.update();
    }
    let car = single::<With<PlayerVehicle>>(&mut app);
    assert!(
        app.world().resource::<Session>().tick() > 0,
        "the session clock should be counting"
    );

    press_key(&mut app, KeyCode::Escape);
    assert!(
        phase_is(&mut app, SessionPhase::Paused),
        "Esc on a live session pauses, got {:?}",
        app.world().resource::<Session>().phase()
    );
    assert!(physics_paused(&mut app), "the physics clock must stop");
    assert_eq!(pause_rows(&mut app), 1, "the pause overlay draws");
    // The same Esc press must not have been re-read as a resume —
    // `pause_input` runs between the intent reader and the driver.
    assert!(
        !app.world().resource::<SessionControl>().quit,
        "the entering Esc must not also queue a quit"
    );

    // A paused world is fully frozen: no physics steps, no session
    // ticks, no drift. Snapshot after one paused update: Avian's
    // schedule runner zeroes the physics delta only *after* its run
    // check, so the first paused FixedMain drains one stale-delta step
    // — part of the lead-in, not the freeze.
    app.update();
    let pos = app.world().get::<Position>(car).unwrap().0;
    let tick = app.world().resource::<Session>().tick();
    for _ in 0..30 {
        app.update();
    }
    assert_eq!(app.world().get::<Position>(car).unwrap().0, pos);
    assert_eq!(app.world().resource::<Session>().tick(), tick);
    assert!(phase_is(&mut app, SessionPhase::Paused));

    press_key(&mut app, KeyCode::Escape);
    assert!(
        phase_is(&mut app, SessionPhase::Playing),
        "Esc while paused resumes, got {:?}",
        app.world().resource::<Session>().phase()
    );
    assert!(!physics_paused(&mut app));
    assert_eq!(pause_rows(&mut app), 0, "the overlay is gone");
    for _ in 0..10 {
        app.update();
    }
    assert!(
        app.world().resource::<Session>().tick() > tick,
        "the session clock resumes"
    );
}

/// The pause overlay's rows drive the real session intents: Resume is
/// `Paused → Playing`, Quit lands at `Menu` (and, with no `MenuShell`,
/// writes `AppExit`), and a disabled row reports its reason without
/// navigating anywhere.
#[test]
fn pause_menu_rows_drive_the_session() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    // Enter on the focused row resumes.
    press_key(&mut app, KeyCode::Escape);
    assert!(phase_is(&mut app, SessionPhase::Paused));
    press_key(&mut app, KeyCode::Enter);
    assert!(
        phase_is(&mut app, SessionPhase::Playing),
        "Resume should land back in Playing"
    );

    // Without graphics settings (this rig has none) the Options row is
    // disabled: it explains itself and goes nowhere.
    press_key(&mut app, KeyCode::Escape);
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::Enter);
    assert!(
        phase_is(&mut app, SessionPhase::Paused),
        "a disabled row must not activate"
    );
    assert!(
        app.world()
            .resource::<PauseMenu>()
            .status
            .as_deref()
            .unwrap_or("")
            .contains("graphics settings unavailable"),
        "the disabled reason lands on the status line"
    );

    // The last row quits: Unloading → Menu, and with no menu shell the
    // driver writes AppExit.
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::Enter);
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "quit from pause never reached Menu: {:?}",
        app.world().resource::<Session>().phase()
    );
    assert_eq!(count::<With<SessionEntity>>(&mut app), 0);
    assert_eq!(pause_rows(&mut app), 0, "the overlay dies with the session");
    assert!(!physics_paused(&mut app), "physics unpauses at Menu");
    // The Menu arm consumes the intent on a later update — watch for
    // the exit message before it ages out of the buffer.
    let wrote_exit = run_until(&mut app, 6, |a| {
        !a.world().resource::<Messages<AppExit>>().is_empty()
    });
    assert!(wrote_exit, "quit should request AppExit");
    let exits: Vec<AppExit> = app
        .world_mut()
        .resource_mut::<Messages<AppExit>>()
        .drain()
        .collect();
    assert!(
        exits.iter().any(|e| matches!(e, AppExit::Success)),
        "quit with no menu shell should exit, got {exits:?}"
    );
}

/// Restart from the pause overlay rides the existing restart intent —
/// teardown, `Menu`, `begin` — so the new session lands clean and
/// unpaused with a bumped generation.
#[test]
fn pause_menu_restart_reloads_clean() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    press_key(&mut app, KeyCode::Escape);
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::Enter);
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "restart from pause never returned to Playing: {:?}",
        app.world().resource::<Session>().phase()
    );
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    assert!(
        !physics_paused(&mut app),
        "a restarted session is not paused"
    );
    assert_eq!(pause_rows(&mut app), 0);
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 1);
    assert_eq!(count::<With<Hud>>(&mut app), 1);
}

/// MP-6: a session under non-local authority cannot pause — `Esc`
/// keeps its quit meaning rather than going dead.
#[test]
fn esc_quits_when_the_authority_cannot_pause() {
    let mut app = test_app(
        SessionConfig {
            authority: SessionAuthority::Host,
            ..SessionConfig::default()
        },
        1.0 / 60.0,
    );
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    press_key(&mut app, KeyCode::Escape);
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "Esc on a non-pausable session should quit, got {:?}",
        app.world().resource::<Session>().phase()
    );
    assert_eq!(pause_rows(&mut app), 0, "no pause overlay for a host");
}

/// The `F4` binding is not the only `control.restart` producer — the
/// results screen's Restart row (`results_input`) and a Blitz/
/// Checkpoint `RestartEvent` disabled outcome queue the same intent.
/// Under a networked authority the wire owns restarts (the host's
/// `Cancel`/`Start`), so the intent must still tear the predicted
/// session down to `Menu` — the lobby's waiting state — but never
/// mint a local `begin`: no new generation, the retained config keeps
/// its authority, and nothing exits.
#[test]
fn a_networked_restart_returns_to_menu_without_a_local_begin() {
    for authority in [SessionAuthority::Remote, SessionAuthority::Host] {
        let mut app = test_app(
            SessionConfig {
                authority,
                ..SessionConfig::default()
            },
            1.0 / 60.0,
        );
        app.update();
        assert!(phase_is(&mut app, SessionPhase::Playing));
        let generation = app.world().resource::<Session>().generation();

        // The real producer path: `Results` → focus "Restart race" →
        // Enter queues `control.restart` — not a planted flag.
        app.world_mut()
            .resource_mut::<Session>()
            .transition(SessionPhase::Results)
            .unwrap();
        press_key(&mut app, KeyCode::ArrowDown);
        press_key(&mut app, KeyCode::Enter);
        assert!(
            app.world().resource::<SessionControl>().restart,
            "the Restart row queued the intent ({authority:?})"
        );
        assert!(
            run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
            "the restart intent should still tear down to Menu ({authority:?}): {:?}",
            app.world().resource::<Session>().phase()
        );
        // A few settled updates — no begin may fire late either.
        for _ in 0..4 {
            app.update();
        }
        let session = app.world().resource::<Session>();
        assert_eq!(
            session.phase(),
            &SessionPhase::Menu,
            "no local begin under {authority:?}"
        );
        assert_eq!(
            session.generation(),
            generation,
            "no generation was minted ({authority:?})"
        );
        assert_eq!(session.config().unwrap().authority, authority);
        assert!(
            !app.world().resource::<SessionControl>().restart,
            "the intent was consumed, not left dangling"
        );
        assert!(
            app.should_exit().is_none(),
            "no exit either — the lobby owns what comes next"
        );
    }
}

/// The `--pause` dev override pauses the first `Playing` frame — how a
/// `--frames`/`--screenshot` capture (live input frozen) renders the
/// overlay. It is a one-shot: a resume afterwards stays resumed.
#[test]
fn dev_pause_pauses_once_at_playing() {
    let mut app = test_app(
        SessionConfig {
            dev: DevOverrides {
                pause: true,
                ..DevOverrides::default()
            },
            ..SessionConfig::default()
        },
        1.0 / 60.0,
    );
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Paused)),
        "--pause never paused the session: {:?}",
        app.world().resource::<Session>().phase()
    );
    assert!(physics_paused(&mut app));
    assert_eq!(pause_rows(&mut app), 1);

    press_key(&mut app, KeyCode::Escape);
    assert!(phase_is(&mut app, SessionPhase::Playing));
    for _ in 0..10 {
        app.update();
    }
    assert!(
        phase_is(&mut app, SessionPhase::Playing),
        "the one-shot must not re-fire after a resume"
    );
}

/// A `HudMapSpec` in the authored shape — the regression tests below
/// only drive `HudMap::fullscreen`, but `HudMap::new` needs a real
/// spec (every field required by the parser).
fn hudmap_spec() -> HudMapSpec {
    HudMapSpec::parse(
        "type: a\nmmHudMap {\n  Size 0.21 0.25\n  Pos 0.78 0.75\n  ZoomIn 0\n  Approach Rate 1.2\n  ZoomInDist 577\n  ZoomOutDist 1195\n  IconScaleMin 34\n  IconScaleMax 52\n  ZoomInDistFS 786\n  ZoomOutDistFS 1581\n  IconScaleMinFS 15\n  IconScaleMaxFS 18\n  Ocean Color 0.084 0.7 0.94\n}\n",
    )
    .unwrap()
}

/// F22-A.1/HUD-4: while the full-screen pause map replaces the pause
/// overlay, the hidden menu is input-dead — arrows cannot drift the
/// unseen focus, `Enter` cannot activate a row, and Backspace cannot
/// resume into a covered-over live session. Only the map's own Q/Esc
/// get through (handled by `hudmap_input` ahead of `pause_input`), and
/// they close straight back to play.
#[test]
fn pause_map_owns_the_keys_while_the_menu_is_hidden() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    // A bound city session carries this state; the dev world has none,
    // so the test plants what `load_session_world` would have.
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .insert_resource(HudMap::new(hudmap_spec(), generation));

    // Q opens the full-screen pause map — the same pause intent Esc
    // queues, so the session lands `Paused` with the overlay hidden.
    press_key(&mut app, KeyCode::KeyQ);
    assert!(
        phase_is(&mut app, SessionPhase::Paused),
        "Q should pause into the full-screen map, got {:?}",
        app.world().resource::<Session>().phase()
    );
    assert!(app.world().resource::<HudMap>().fullscreen);
    assert_eq!(
        pause_rows(&mut app),
        0,
        "the map replaces the pause overlay"
    );

    // Every menu key is dead while the map is up: the focus cannot
    // drift to Restart/Quit, `Enter` activates nothing, Backspace does
    // not resume — and, since F22-B.2 gave it to the rear-view mirror,
    // it must not toggle that either — the phase and the map must not
    // move.
    for key in [
        KeyCode::ArrowDown,
        KeyCode::ArrowDown,
        KeyCode::ArrowUp,
        KeyCode::Enter,
        KeyCode::Space,
        KeyCode::Backspace,
        KeyCode::KeyW,
    ] {
        press_key(&mut app, key);
    }
    assert!(
        phase_is(&mut app, SessionPhase::Paused),
        "menu keys reached the hidden pause rows: {:?}",
        app.world().resource::<Session>().phase()
    );
    assert!(
        !app.world().resource::<mm2_app::camera::RearView>().0,
        "Backspace while Paused must not reach the mirror toggle"
    );
    assert!(app.world().resource::<HudMap>().fullscreen);
    assert_eq!(
        app.world().resource::<PauseMenu>().focus,
        0,
        "the hidden focus must not drift"
    );
    {
        let control = app.world().resource::<SessionControl>();
        assert!(
            !control.quit && !control.restart && !control.pause,
            "no session intent may leak through the map"
        );
    }

    // Q closes the map straight back to play — and the pause keypress
    // is not re-read as anything else in the same update.
    press_key(&mut app, KeyCode::KeyQ);
    assert!(phase_is(&mut app, SessionPhase::Playing));
    assert!(!app.world().resource::<HudMap>().fullscreen);
}

/// F22-AC06 pad leg (designed `input::pad` map): `Select` is the TAB
/// view cycle and `DPadLeft`/`DPadRight` the `E`/`F` map toggles —
/// the same `hudmap_input` arms the keys use.
#[test]
fn pad_buttons_drive_the_map_controls() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .insert_resource(HudMap::new(hudmap_spec(), generation));
    app.world_mut().spawn(Gamepad::default());

    pad_press(&mut app, GamepadButton::Select);
    assert_eq!(app.world().resource::<HudMap>().view, MapView::Large);
    pad_press(&mut app, GamepadButton::Select);
    assert_eq!(app.world().resource::<HudMap>().view, MapView::Off);
    pad_press(&mut app, GamepadButton::Select);
    assert_eq!(app.world().resource::<HudMap>().view, MapView::Inset);

    pad_press(&mut app, GamepadButton::DPadLeft);
    assert!(app.world().resource::<HudMap>().zoomed_in);
    pad_press(&mut app, GamepadButton::DPadRight);
    assert_eq!(
        app.world().resource::<HudMap>().orientation,
        MapOrientation::Rotating
    );
}

/// The full-screen map only lives inside its pause: if the flag is up
/// while the session is `Playing` and no pause intent is pending — a
/// rejected intent, or any path that skipped the close arm — the next
/// update drops it rather than leaving the order-1 map camera over
/// live gameplay with no key that closes it.
#[test]
fn fullscreen_map_clears_itself_outside_pause() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    let generation = app.world().resource::<Session>().generation();
    let mut map = HudMap::new(hudmap_spec(), generation);
    map.fullscreen = true;
    app.world_mut().insert_resource(map);

    app.update();
    assert!(
        !app.world().resource::<HudMap>().fullscreen,
        "a full-screen map over a Playing session must self-clear"
    );
    assert!(phase_is(&mut app, SessionPhase::Playing));
}

/// MP-6: `--pause-map` on a non-pausable authority must not fire —
/// queuing the intent would be rejected by `drive_session`, leaving the
/// map full-screen over a session that is still `Playing`.
#[test]
fn pause_map_dev_override_respects_pause_authority() {
    let mut app = test_app(
        SessionConfig {
            authority: SessionAuthority::Host,
            dev: DevOverrides {
                pause_map: true,
                ..DevOverrides::default()
            },
            ..SessionConfig::default()
        },
        1.0 / 60.0,
    );
    // The bound-map state the dev flag looks for — planted before the
    // first frame so the gate, not resource timing, is what is tested.
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .insert_resource(HudMap::new(hudmap_spec(), generation));

    for _ in 0..10 {
        app.update();
    }
    assert!(
        phase_is(&mut app, SessionPhase::Playing),
        "a host session never pauses, got {:?}",
        app.world().resource::<Session>().phase()
    );
    assert!(
        !app.world().resource::<HudMap>().fullscreen,
        "--pause-map must not leave the map full-screen"
    );
    assert!(!app.world().resource::<SessionControl>().pause);
}

/// AC01: a restart through the real spawn/teardown path leaves exactly
/// one of every session-owned thing — no duplicated cameras, players,
/// physics bodies or UI — and clears session-scoped caches.
#[test]
fn restart_leaves_exactly_one_session() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    let car = single::<With<PlayerVehicle>>(&mut app);
    let bodies = count::<With<RigidBody>>(&mut app);
    assert_eq!(
        count::<With<Camera3d>>(&mut app),
        3,
        "chase + free + mirror"
    );
    assert_eq!(count::<With<Hud>>(&mut app), 1);
    assert_eq!(count::<With<ErrorText>>(&mut app), 1);
    let active = {
        let world = app.world_mut();
        world
            .query::<&Camera>()
            .iter(world)
            .filter(|c| c.is_active)
            .count()
    };
    assert_eq!(active, 1);

    // Bookkeeping entries must not leak into the next session: plant a
    // trailer record the teardown has to clear.
    let dummy = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<SpawnPoint>()
        .trailers
        .push((dummy, Vec3::ZERO));

    // Drive the restart through the real key path: F4 latches the
    // intent (the documented original binding — CTL-1; Backspace is
    // the F22-B.2 mirror now), then the input is cleared (no
    // InputPlugin runs here, so `just_pressed` would otherwise persist
    // into the next session).
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F4);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();

    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "restart never returned to Playing"
    );
    // The transition frame itself runs no Playing fixed steps — let the
    // new session's clock advance a little.
    for _ in 0..5 {
        app.update();
    }
    let session = app.world().resource::<Session>();
    assert_eq!(session.generation(), 2);
    // The clock restarted at `begin` and is counting the new session's
    // steps — not continuing the old session's.
    assert!(
        session.tick() > 0 && session.tick() <= 20,
        "tick={}",
        session.tick()
    );

    // One session's worth of entities, all owned by generation 2.
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 1);
    assert_eq!(count::<With<Camera3d>>(&mut app), 3);
    assert_eq!(count::<With<Hud>>(&mut app), 1);
    assert_eq!(count::<With<ErrorText>>(&mut app), 1);
    assert_eq!(count::<With<RigidBody>>(&mut app), bodies);
    {
        let world = app.world_mut();
        let stale = world
            .query::<&SessionEntity>()
            .iter(world)
            .filter(|e| e.0 != 2)
            .count();
        assert_eq!(stale, 0, "entities from generation 1 leaked");
    }
    assert!(
        app.world().get_entity(car).is_err(),
        "the old player entity was not torn down"
    );
    assert!(
        app.world().resource::<SpawnPoint>().trailers.is_empty(),
        "trailer bookkeeping leaked into the new session"
    );
}

/// F18-A.6: `CityWater` is session-scoped like `CityPvs` — a resource
/// planted mid-session must not survive teardown into the next
/// session (a city load inserts its own; the dev world inserts none).
#[test]
fn city_water_does_not_leak_across_restart() {
    use mm2_formats::psdl::{PerimeterPoint, Psdl, PsdlRoom};
    let psdl = Psdl {
        target_size: 2,
        vertices: vec![
            [0.0, -4.0, 0.0],
            [8.0, -4.0, 0.0],
            [8.0, -4.0, 8.0],
            [0.0, -4.0, 8.0],
        ],
        heights: Vec::new(),
        textures: Vec::new(),
        rooms: vec![PsdlRoom {
            perimeter: (0..4)
                .map(|k| PerimeterPoint { vertex: k, room: 0 })
                .collect(),
            attributes: Vec::new(),
            unparsed_attributes: Vec::new(),
        }],
        room_flags: Vec::new(),
        prop_rules: Vec::new(),
        junction_count: 0,
        bounds_min: [0.0; 3],
        bounds_max: [0.0; 3],
        bounds_center: [0.0; 3],
        bounds_radius: 0.0,
        paths: Vec::new(),
    };
    let water = mm2_app::water::CityWater::build(
        &mm2_formats::water::WaterDef {
            level: -3.8,
            refs: vec![1],
        },
        &psdl,
        None,
    );

    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    app.world_mut().insert_resource(water);
    assert!(
        app.world()
            .get_resource::<mm2_app::water::CityWater>()
            .is_some()
    );

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F4);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "restart never returned to Playing"
    );
    assert!(
        app.world()
            .get_resource::<mm2_app::water::CityWater>()
            .is_none(),
        "a dev-world session must not inherit a stale CityWater"
    );
}

/// `WorldFloor` is session-scoped like `CityPvs`/`CityWater` — a city
/// load inserts the authored bound, teardown removes it, and the dev
/// world must not inherit a stale floor (the smoke runner falls back
/// to a spawn-relative line when none is bound).
#[test]
fn world_floor_does_not_leak_across_restart() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    app.world_mut()
        .insert_resource(mm2_app::city::WorldFloor(-60.0));
    assert!(
        app.world()
            .get_resource::<mm2_app::city::WorldFloor>()
            .is_some()
    );

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::F4);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "restart never returned to Playing"
    );
    assert!(
        app.world()
            .get_resource::<mm2_app::city::WorldFloor>()
            .is_none(),
        "a dev-world session must not inherit a stale WorldFloor"
    );
}

/// AC01: quit also routes through teardown — the world despawns, the
/// session reaches `Menu`, and the app is asked to exit.
#[test]
fn quit_unloads_to_menu_and_writes_app_exit() {
    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    app.world_mut().resource_mut::<SessionControl>().quit = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "quit never reached Menu"
    );
    assert_eq!(count::<With<SessionEntity>>(&mut app), 0);
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 0);
    // The intent is consumed on a later update, once the phase is Menu —
    // watch for the exit message before it ages out of the buffer.
    let wrote_exit = run_until(&mut app, 6, |a| {
        !a.world().resource::<Messages<AppExit>>().is_empty()
    });
    assert!(wrote_exit, "quit should request AppExit");
    let exits: Vec<AppExit> = app
        .world_mut()
        .resource_mut::<Messages<AppExit>>()
        .drain()
        .collect();
    assert!(
        exits.iter().any(|e| matches!(e, AppExit::Success)),
        "quit should request AppExit::Success, got {exits:?}"
    );
}

/// AC02: a failed load leaves no active player simulation — and the
/// failure is still a session that can be retried or quit through the
/// same lifecycle.
#[test]
fn failed_load_leaves_no_player_simulation() {
    let config = SessionConfig {
        world: WorldMode::City {
            psdl: "city/bogus.psdl".into(),
        },
        ..SessionConfig::default()
    };
    let mut app = test_app(config, 1.0 / 60.0);
    app.update();
    assert!(
        matches!(
            app.world().resource::<Session>().phase(),
            SessionPhase::Failed(_)
        ),
        "a missing city must fail the load, got {:?}",
        app.world().resource::<Session>().phase()
    );
    // No player simulation exists in the incomplete world — the vehicle
    // is only spawned after the world reports Ready.
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 0);
    assert_eq!(count::<With<Vehicle>>(&mut app), 0);
    assert_eq!(count::<With<RigidBody>>(&mut app), 0);

    // Retry fails the same way (same config), still without a player.
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    assert!(
        run_until(&mut app, 12, |a| matches!(
            a.world().resource::<Session>().phase(),
            SessionPhase::Failed(_)
        )),
        "retry never re-failed"
    );
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    assert_eq!(count::<With<PlayerVehicle>>(&mut app), 0);

    // Quit still tears down cleanly: every session-owned entity goes.
    app.world_mut().resource_mut::<SessionControl>().quit = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Menu)),
        "failed session never unloaded"
    );
    assert_eq!(count::<With<SessionEntity>>(&mut app), 0);
}

/// Spawn a marked dynamic box that falls onto the dev-world ground and
/// produce its `ImpactEvent`. The player car's raycast wheels catch its
/// spawn drop without a chassis contact, so a box gives a deterministic
/// reportable impact (same trick as the contracts tests).
fn drop_box_and_collect(app: &mut App, pos: Vec3) -> (ObjectId, Vec<ImpactEvent>) {
    let object = app.world_mut().resource_mut::<Session>().mint_object_id();
    app.world_mut().spawn((
        ObjectIdentity(object),
        DamageSignals::default(),
        RigidBody::Dynamic,
        Collider::cuboid(1.0, 1.0, 1.0),
        CollisionEventsEnabled,
        Position(pos),
        Transform::from_translation(pos),
    ));
    let mut found = Vec::new();
    run_until(app, 90, |a| {
        found.extend(
            drain_impacts(a)
                .into_iter()
                .filter(|e| e.participants.0 == object || e.participants.1 == object),
        );
        !found.is_empty()
    });
    (object, found)
}

/// Teardown clears the impact pipeline's session-scoped state: the new
/// session's impact stream restarts its ids, and its evidence counters
/// describe only the new session.
#[test]
fn restart_resets_the_impact_stream() {
    let mut app = dev_app();
    app.update();
    let (_, gen1) = drop_box_and_collect(&mut app, Vec3::new(3.0, 2.0, 0.0));
    assert!(
        !gen1.is_empty(),
        "a dropped box should produce a generation-1 impact"
    );
    assert!(gen1.iter().all(|e| e.generation == 1));

    app.world_mut().resource_mut::<SessionControl>().restart = true;
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "restart never returned to Playing"
    );
    // Anything buffered from the old session is generation-1; drain it
    // before collecting the new stream.
    drain_impacts(&mut app);
    let (_, gen2) = drop_box_and_collect(&mut app, Vec3::new(6.0, 2.0, 0.0));
    assert!(
        !gen2.is_empty(),
        "the respawned session should produce its own impact"
    );
    assert!(gen2.iter().all(|e| e.generation == 2));
    assert_eq!(
        gen2[0].id,
        ImpactId(1),
        "impact ids restart per session — teardown must reset the stream"
    );
    assert_eq!(
        app.world().resource::<ImpactFilter>().emitted as usize,
        gen2.len(),
        "emitted counts only the current session's stream"
    );
}

/// A `--spawn` dev pose replaces the world's roam spawn — the player
/// lands exactly where the override says, at the override's yaw, and
/// `SpawnPoint` records it for resets.
#[test]
fn dev_spawn_override_pins_the_player_pose() {
    let config = SessionConfig {
        dev: DevOverrides {
            spawn: Some(SpawnPose {
                position: Vec3::new(5.0, 2.0, -7.0),
                yaw: 1.0,
            }),
            ..DevOverrides::default()
        },
        ..SessionConfig::default()
    };
    let mut app = test_app(config, 1.0 / 60.0);
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    let car = single::<With<PlayerVehicle>>(&mut app);
    assert_eq!(
        app.world().get::<Position>(car).unwrap().0,
        Vec3::new(5.0, 2.0, -7.0),
        "the override pose wins over the world spawn"
    );
    assert_eq!(
        app.world().get::<Rotation>(car).unwrap().0,
        Quat::from_rotation_y(1.0),
    );
    assert_eq!(
        app.world().resource::<SpawnPoint>().position,
        Vec3::new(5.0, 2.0, -7.0),
        "resets return to the override pose"
    );
}

/// A `--banger-pool` dev bound lands on the session-scoped pool through
/// the real load path — and stays quarantined: an unconfigured session
/// keeps the recovered ×32 default.
#[test]
fn dev_banger_pool_override_bounds_the_active_pool() {
    let mut app = test_app(
        SessionConfig {
            dev: DevOverrides {
                banger_pool: Some(4),
                ..DevOverrides::default()
            },
            ..SessionConfig::default()
        },
        1.0 / 60.0,
    );
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    assert_eq!(
        app.world().resource::<BangerPool>().max_active,
        4,
        "the dev bound replaces the default"
    );

    let mut app = dev_app();
    app.update();
    assert_eq!(
        app.world().resource::<BangerPool>().max_active,
        DEFAULT_ACTIVE_POOL,
        "no override keeps the recovered default"
    );
}

/// AC03: fixed-tick input produces equivalent gameplay regardless of how
/// updates are batched. Two apps pinned to identical throttle — one at
/// 60 updates/s (2 fixed steps each), one at 30/s (4 each) — must reach
/// the same state after the same number of fixed steps.
#[test]
fn fixed_tick_driving_is_update_rate_independent() {
    fn driven(update_secs: f64) -> (Vec3, f32, u64) {
        let mut app = test_app(SessionConfig::default(), update_secs);
        app.update();
        let car = single::<With<PlayerVehicle>>(&mut app);
        if let Some(mut vi) = app.world_mut().get_mut::<VehicleInput>(car) {
            vi.throttle = 1.0;
        }
        run_until(&mut app, 300, |a| {
            a.world().resource::<Session>().tick() >= 240
        });
        let s = app.world().resource::<Session>();
        assert_eq!(s.tick(), 240, "expected exactly 240 fixed steps");
        (
            app.world().get::<Position>(car).unwrap().0,
            app.world().get::<VehicleState>(car).unwrap().forward_speed,
            s.tick(),
        )
    }

    let (p60, v60, t60) = driven(1.0 / 60.0);
    let (p30, v30, t30) = driven(1.0 / 30.0);
    assert_eq!(t60, t30);
    assert!(
        (p60 - p30).length() < 1e-3,
        "same fixed steps, different batching diverged: {p60} vs {p30}"
    );
    assert!((v60 - v30).abs() < 1e-3, "speeds diverged: {v60} vs {v30}");
    // Sanity: the car actually drove, not two identically parked states.
    assert!(v60 > 5.0, "the drive probe should be moving ({v60} m/s)");
}

/// F22-B.1 review repair: `CameraMode::Cockpit` held at load — a
/// `--cockpit` launch, or a mode persisted across a reload into a
/// dashless car — must not leave zero active cameras. The effective
/// mode resolves before the session cameras spawn, so the load lands
/// on Chase with the chase camera active (the dev car carries no
/// authored `camPovCS`, and the `None`-def arm never runs
/// `spawn_dash`).
#[test]
fn cockpit_without_authored_camera_falls_back_to_an_active_chase() {
    let mut app = dev_app();
    *app.world_mut().resource_mut::<CameraMode>() = CameraMode::Cockpit;
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    assert_eq!(
        *app.world().resource::<CameraMode>(),
        CameraMode::Chase,
        "no authored camPovCS exists — the mode falls back at load"
    );
    let chase = single::<With<ChaseCamera>>(&mut app);
    assert!(
        app.world().get::<Camera>(chase).unwrap().is_active,
        "the fallback leaves the chase camera active — something renders"
    );
    let active = {
        let world = app.world_mut();
        world
            .query::<&Camera>()
            .iter(world)
            .filter(|c| c.is_active)
            .count()
    };
    assert_eq!(active, 1, "exactly one session camera renders");
}

/// F22-B.2: restart moved to the documented `F4` binding (CTL-1);
/// `Backspace` now belongs to the rear-view mirror and must not latch
/// the restart intent while `Playing` — it flips `RearView`, and the
/// strip camera (spawned under the player by the production load path)
/// activates through `drive_mirror`. The toggle is session-agnostic
/// like `CameraMode`: the restarted session's fresh strip camera
/// picks it back up.
#[test]
fn f4_restarts_and_backspace_is_the_mirror() {
    use mm2_app::camera::MirrorCamera;

    let mut app = dev_app();
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));
    let mirror_cam = single::<With<MirrorCamera>>(&mut app);
    assert!(
        !app.world().get::<Camera>(mirror_cam).unwrap().is_active,
        "the strip starts inactive"
    );

    // Backspace toggles the mirror, never the session.
    press_key(&mut app, KeyCode::Backspace);
    assert!(app.world().resource::<mm2_app::camera::RearView>().0);
    // The harness leaves the input reader and `drive_mirror` unordered
    // like the binary — the strip can settle a frame late.
    app.update();
    assert!(
        app.world().get::<Camera>(mirror_cam).unwrap().is_active,
        "drive_mirror activated the strip"
    );
    assert!(phase_is(&mut app, SessionPhase::Playing));
    assert!(!app.world().resource::<SessionControl>().restart);

    // F4 drives the restart through the same teardown/reload cycle the
    // old Backspace binding rode.
    press_key(&mut app, KeyCode::F4);
    assert!(
        run_until(&mut app, 12, |a| phase_is(a, SessionPhase::Playing)),
        "F4 restart never returned to Playing"
    );
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    app.update(); // the fresh strip settles like above
    let mirror_cam = single::<With<MirrorCamera>>(&mut app);
    assert!(
        app.world().get::<Camera>(mirror_cam).unwrap().is_active,
        "the new session's strip camera re-arms from `RearView`"
    );
}

/// F22-B.5: `--reset-at` emits the `R`-key reset bundle at its
/// session tick — the player teleports back to the spawn point
/// through the production `ResetVehicle` path (`Teleported` stamped)
/// mid-run, with the session still `Playing` afterwards.
#[test]
fn reset_at_teleports_the_player_back_to_spawn() {
    let config = SessionConfig {
        dev: DevOverrides {
            reset_at: Some(600),
            ..DevOverrides::default()
        },
        ..SessionConfig::default()
    };
    let mut app = test_app(config, 1.0 / 60.0);
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let car = single::<With<PlayerVehicle>>(&mut app);
    // Drive off the spawn while the flag's tick is still far away —
    // the fixed-step sim reads `VehicleInput` directly.
    app.world_mut()
        .get_mut::<VehicleInput>(car)
        .unwrap()
        .throttle = 1.0;

    // 600 session ticks at 120 Hz is ~300 updates of driving — plenty
    // of distance to prove the teleport rather than a parked car.
    assert!(
        run_until(&mut app, 400, |a| a
            .world()
            .get::<Teleported>(car)
            .is_some()),
        "the scheduled reset never landed"
    );
    let pos = app.world().get::<Transform>(car).unwrap().translation;
    assert!(
        pos.distance(Vec3::new(0.0, 1.5, 0.0)) < 3.0,
        "the reset put the car back on the spawn point, got {pos:?}"
    );
    assert!(phase_is(&mut app, SessionPhase::Playing));
}

/// A threshold the run never reaches never resets — the gate is the
/// session clock like `--restart-at`, not mere `Playing`.
#[test]
fn reset_at_beyond_the_run_never_fires() {
    let config = SessionConfig {
        dev: DevOverrides {
            reset_at: Some(u64::MAX),
            ..DevOverrides::default()
        },
        ..SessionConfig::default()
    };
    let mut app = test_app(config, 1.0 / 60.0);
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let car = single::<With<PlayerVehicle>>(&mut app);
    app.world_mut()
        .get_mut::<VehicleInput>(car)
        .unwrap()
        .throttle = 1.0;
    for _ in 0..200 {
        app.update();
    }
    let pos = app.world().get::<Transform>(car).unwrap().translation;
    assert!(
        pos.distance(Vec3::new(0.0, 1.5, 0.0)) > 5.0,
        "control leg: the car actually left the spawn, still at {pos:?}"
    );
    assert!(
        app.world().get::<Teleported>(car).is_none(),
        "an unreached threshold must not teleport"
    );
}

/// F22-AC06 pad leg (designed `input::pad` map): `North` is the `R`
/// reset — the pad press emits the production `spawn_resets` bundle
/// and the teleport lands through `vehicle_reset` (`Teleported`)
/// exactly like the key and `--reset-at`.
#[test]
fn pad_north_resets_the_player_to_spawn() {
    let mut app = dev_app();
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let car = single::<With<PlayerVehicle>>(&mut app);
    app.world_mut()
        .get_mut::<VehicleInput>(car)
        .unwrap()
        .throttle = 1.0;
    for _ in 0..120 {
        app.update();
    }
    let pos = app.world().get::<Transform>(car).unwrap().translation;
    assert!(
        pos.distance(Vec3::new(0.0, 1.5, 0.0)) > 5.0,
        "control leg: the car actually left the spawn, still at {pos:?}"
    );

    app.world_mut().spawn(Gamepad::default());
    pad_press(&mut app, GamepadButton::North);
    assert!(
        run_until(&mut app, 10, |a| a.world().get::<Teleported>(car).is_some()),
        "the pad reset never landed"
    );
    let pos = app.world().get::<Transform>(car).unwrap().translation;
    assert!(
        pos.distance(Vec3::new(0.0, 1.5, 0.0)) < 3.0,
        "the pad reset put the car back on the spawn point, got {pos:?}"
    );
    assert!(phase_is(&mut app, SessionPhase::Playing));
}

/// The review's pad-focus wart (F22-AC06 follow-up): gilrs-style
/// backends deliver pad input while the window is unfocused where the
/// OS would never deliver a key — the shared `control_just_pressed`
/// gate makes an unfocused window inert for both devices, and the
/// controls answer again after refocus.
#[test]
fn unfocused_window_gates_the_pad_map() {
    let mut app = dev_app();
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let car = single::<With<PlayerVehicle>>(&mut app);
    app.world_mut().spawn(Gamepad::default());
    let window = app
        .world_mut()
        .spawn(Window {
            focused: false,
            ..default()
        })
        .id();

    // While unfocused the pad's reset/mirror edges — and the key's,
    // the gate sits on the control not the device — are all inert.
    pad_press(&mut app, GamepadButton::North);
    pad_press(&mut app, GamepadButton::East);
    press_key(&mut app, KeyCode::KeyR);
    assert!(
        app.world().get::<Teleported>(car).is_none(),
        "an unfocused pad must not reset the car"
    );
    assert!(
        !app.world().resource::<mm2_app::camera::RearView>().0,
        "an unfocused pad must not toggle the mirror"
    );

    // Refocus and both devices answer again — the reset lands through
    // the same production `Teleported` path `pad_north_*` proves.
    app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
    pad_press(&mut app, GamepadButton::East);
    assert!(app.world().resource::<mm2_app::camera::RearView>().0);
    pad_press(&mut app, GamepadButton::North);
    assert!(
        run_until(&mut app, 10, |a| a.world().get::<Teleported>(car).is_some()),
        "the refocused pad reset never landed"
    );
}

/// F25-A.6: the `R` bundle is authority-only — under a `Remote`
/// (predicted) session the key writes no `ResetVehicle` and the
/// predicted car never self-teleports. The remote driver's reset is
/// the host's: it arrives as an epoch-declared `Snap`, not as a local
/// pose write a wire copy could never learn (F25-A.5's gate, proven
/// here rather than inspected).
#[test]
fn r_is_inert_under_remote_authority() {
    let mut app = test_app(
        SessionConfig {
            authority: SessionAuthority::Remote,
            ..SessionConfig::default()
        },
        1.0 / 60.0,
    );
    assert!(run_until(&mut app, 12, |a| phase_is(
        a,
        SessionPhase::Playing
    )));
    let car = single::<With<PlayerVehicle>>(&mut app);
    app.world_mut()
        .get_mut::<VehicleInput>(car)
        .unwrap()
        .throttle = 1.0;
    for _ in 0..120 {
        app.update();
    }
    let pos = app.world().get::<Transform>(car).unwrap().translation;
    assert!(
        pos.distance(Vec3::new(0.0, 1.5, 0.0)) > 5.0,
        "control leg: the car actually left the spawn, still at {pos:?}"
    );
    // Anything buffered before the key drains here so the assertion
    // below counts only what `R` itself wrote.
    let _ = app
        .world_mut()
        .resource_mut::<Messages<ResetVehicle>>()
        .drain()
        .count();

    press_key(&mut app, KeyCode::KeyR);
    for _ in 0..10 {
        app.update();
    }

    let resets = app
        .world_mut()
        .resource_mut::<Messages<ResetVehicle>>()
        .drain()
        .count();
    assert_eq!(resets, 0, "a Remote-authority R wrote a reset message");
    assert!(
        app.world().get::<Teleported>(car).is_none(),
        "a Remote-authority R teleported the predicted car"
    );
    let pos = app.world().get::<Transform>(car).unwrap().translation;
    assert!(
        pos.distance(Vec3::new(0.0, 1.5, 0.0)) > 5.0,
        "the predicted car stayed off the spawn point, got {pos:?}"
    );
    assert!(phase_is(&mut app, SessionPhase::Playing));
}

/// F25-A.7 repair (the iteration-009 review's blocking finding):
/// `--reset-at` emits the same `spawn_resets` bundle as the `R` key
/// and it *does* reach a `Remote` session — the flag is absent from
/// `--join`'s conflicts list and `net::start` stamps the client's
/// `dev` flags onto the accepted config — so the scheduled teleport
/// shares the key's authority gate. A predicted session writes no
/// `ResetVehicle` and never self-teleports; the `Host` leg proves the
/// gate is the authority boundary, not "dev flags die under
/// networking".
#[test]
fn reset_at_is_inert_under_remote_authority() {
    for authority in [SessionAuthority::Remote, SessionAuthority::Host] {
        let mut app = test_app(
            SessionConfig {
                authority,
                dev: DevOverrides {
                    reset_at: Some(100),
                    ..DevOverrides::default()
                },
                ..SessionConfig::default()
            },
            1.0 / 60.0,
        );
        assert!(run_until(&mut app, 12, |a| phase_is(
            a,
            SessionPhase::Playing
        )));
        let car = single::<With<PlayerVehicle>>(&mut app);
        app.world_mut()
            .get_mut::<VehicleInput>(car)
            .unwrap()
            .throttle = 1.0;
        // Anything buffered at spawn drains here so the count below is
        // the scheduled flag alone.
        let _ = app
            .world_mut()
            .resource_mut::<Messages<ResetVehicle>>()
            .drain()
            .count();

        match authority {
            SessionAuthority::Remote => {
                // Tick 100 at 120 Hz is ~50 updates — drive well past
                // the threshold; the flag must never have fired.
                for _ in 0..200 {
                    app.update();
                }
                let pos = app.world().get::<Transform>(car).unwrap().translation;
                assert!(
                    pos.distance(Vec3::new(0.0, 1.5, 0.0)) > 5.0,
                    "control leg: the car actually left the spawn, still at {pos:?}"
                );
                let resets = app
                    .world_mut()
                    .resource_mut::<Messages<ResetVehicle>>()
                    .drain()
                    .count();
                assert_eq!(
                    resets, 0,
                    "a Remote-authority --reset-at wrote a reset message"
                );
                assert!(
                    app.world().get::<Teleported>(car).is_none(),
                    "a Remote-authority --reset-at teleported the predicted car"
                );
            }
            _ => {
                assert!(
                    run_until(&mut app, 120, |a| a
                        .world()
                        .get::<Teleported>(car)
                        .is_some()),
                    "the scheduled reset never landed under Host authority"
                );
                let pos = app.world().get::<Transform>(car).unwrap().translation;
                assert!(
                    pos.distance(Vec3::new(0.0, 1.5, 0.0)) < 3.0,
                    "the reset put the car back on the spawn point, got {pos:?}"
                );
            }
        }
        assert!(phase_is(&mut app, SessionPhase::Playing));
    }
}

/// F25-A.7: the self-right assist is authority-gated like the `R`
/// bundle. `load_session_world` stamps `ResetAuthority` from the
/// session's authority, so under `Remote` an upended predicted car
/// writes no `ResetVehicle` and never flops — its recovery is the
/// host's epoch-declared reset — while a `Host` session keeps the
/// assist (the hosted remote drivers' flops resolve through it).
#[test]
fn self_right_is_inert_under_remote_authority() {
    let delay = VehicleConfig::default().assists.self_right_delay;
    let wait = ((delay + 1.5) * 60.0) as usize;
    for authority in [SessionAuthority::Remote, SessionAuthority::Host] {
        let mut app = test_app(
            SessionConfig {
                authority,
                ..SessionConfig::default()
            },
            1.0 / 60.0,
        );
        assert!(run_until(&mut app, 12, |a| phase_is(
            a,
            SessionPhase::Playing
        )));
        let car = single::<With<PlayerVehicle>>(&mut app);
        assert_eq!(
            app.world().resource::<ResetAuthority>().0,
            authority.is_authoritative(),
            "the session load stamped the reset authority from {authority:?}"
        );
        // Anything buffered at spawn drains here so the count below is
        // the upended wait alone.
        let _ = app
            .world_mut()
            .resource_mut::<Messages<ResetVehicle>>()
            .drain()
            .count();

        // On its roof and nearly still — the assist's trigger pose.
        let flipped = Quat::from_rotation_z(std::f32::consts::PI);
        {
            let world = app.world_mut();
            world.get_mut::<Rotation>(car).unwrap().0 = flipped;
            world.get_mut::<Transform>(car).unwrap().rotation = flipped;
            world.get_mut::<LinearVelocity>(car).unwrap().0 = Vec3::ZERO;
            world.get_mut::<AngularVelocity>(car).unwrap().0 = Vec3::ZERO;
        }
        for _ in 0..wait {
            app.update();
        }

        let upright = (app.world().get::<Rotation>(car).unwrap().0 * Vec3::Y).y;
        let resets = app
            .world_mut()
            .resource_mut::<Messages<ResetVehicle>>()
            .drain()
            .count();
        match authority {
            SessionAuthority::Remote => {
                assert_eq!(
                    resets, 0,
                    "a Remote-authority self-right wrote a reset message"
                );
                assert!(
                    app.world().get::<Teleported>(car).is_none(),
                    "a Remote-authority self-right teleported the predicted car"
                );
                assert!(
                    upright < 0.0,
                    "the predicted car stayed upended, up.y {upright}"
                );
                assert_eq!(
                    app.world().get::<VehicleState>(car).unwrap().upended_for,
                    0.0,
                    "the gated detector never armed"
                );
            }
            _ => {
                assert!(
                    upright > 0.9,
                    "the hosted session's car flopped back onto its wheels, up.y {upright}"
                );
                assert!(
                    app.world().get::<Teleported>(car).is_some(),
                    "the hosted flop went through the reset path (Teleported)"
                );
            }
        }
    }
}

/// The `R`-key bundle is just the player reset — trailer reseats are
/// the stream follower's job (F25-B), not the caller's.
#[test]
fn spawn_resets_is_the_player_row() {
    let mut world = World::new();
    let player = world.spawn_empty().id();
    let trailer = world.spawn_empty().id();
    let spawn = SpawnPoint {
        trailers: vec![(trailer, Vec3::new(0.0, -0.5, 8.0))],
        ..SpawnPoint::new(Vec3::new(10.0, 1.0, -5.0), std::f32::consts::FRAC_PI_2)
    };
    let msgs = session::spawn_resets(&spawn, Some(player));
    assert_eq!(msgs.len(), 1, "player only — trailers follow the stream");
    assert_eq!(msgs[0].entity, Some(player));
    assert_eq!(msgs[0].position, Vec3::new(10.0, 1.0, -5.0));
    assert_eq!(msgs[0].yaw, std::f32::consts::FRAC_PI_2);
}

/// The `ResetVehicle` follower reseats every trailer towing the reset
/// entity at its authored car-space offset (F25-B) — the generalized
/// form of the `SpawnPoint.trailers` loops the writers used to repeat,
/// so a remote/AI tractor's reset carries its rig exactly like the
/// local `R` bundle does.
#[test]
fn reseat_towed_trailers_follows_any_tractor_reset() {
    // Same plugin floor as the session/stuck/recovery harnesses —
    // Avian's collider cache reads `AssetEvent<Mesh>` messages and
    // `debug_draw` a `Gizmos` param, both needing the asset/gizmo
    // plugins the binary's DefaultPlugins supplies.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(VehiclePlugin)
        .add_systems(
            Update,
            session::reseat_towed_trailers.before(mm2_vehicle::systems::vehicle_reset),
        );
    app.finish();
    app.cleanup();
    let car = app
        .world_mut()
        .spawn((
            mm2_vehicle::vehicle_bundle(&VehicleConfig::default()),
            Position(Vec3::new(0.0, 1.0, 0.0)),
            Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
        ))
        .id();
    let offset = Vec3::new(0.0, -0.5, 8.0);
    let trailer = app
        .world_mut()
        .spawn((
            mm2_app::car_visual::Trailer {
                towing: car,
                rest_offset: offset,
            },
            mm2_vehicle::vehicle_bundle(&VehicleConfig::default()),
            Position(Vec3::new(9.0, 1.0, 9.0)),
            Transform::from_translation(Vec3::new(9.0, 1.0, 9.0)),
        ))
        .id();
    app.world_mut().write_message(ResetVehicle {
        entity: Some(car),
        position: Vec3::new(10.0, 1.0, -5.0),
        yaw: std::f32::consts::FRAC_PI_2,
    });
    app.update();
    // yaw π/2 maps the +Z rest offset onto +X — the trailer lands the
    // same update, behind the tractor's new pose.
    let want = Vec3::new(18.0, 0.5, -5.0);
    let got = app.world().get::<Position>(trailer).unwrap().0;
    assert!(
        (got - want).length() < 1e-4,
        "trailer re-seated at its authored offset, got {got:?}"
    );
    assert_eq!(
        app.world().get::<Position>(car).unwrap().0,
        Vec3::new(10.0, 1.0, -5.0)
    );
    assert!(
        app.world().get::<Teleported>(trailer).is_some(),
        "the reseat went through `vehicle_reset` (Teleported)"
    );
}

/// A dev world mounts the global `materials.{mtl,csv}` pair the same
/// way a city session does — the pair is not city-scoped, and every
/// dev-world collider is `SurfaceMaterial::Unspecified`, so the
/// `_default` block is what its contacts read. An authored pair binds
/// the resource; an absent pair binds nothing; a half pair (the
/// loader's broken-table error) warns and binds nothing rather than
/// substituting — the same policy `city::load_city` keeps.
#[test]
fn a_dev_world_mounts_the_authored_surface_tables() {
    let install = tempfile::tempdir().unwrap();
    support::surface_materials(install.path());
    let mut app = dev_app();
    app.insert_resource(Mm2Vfs(support::mount(install.path())));
    app.update();
    let tables = app
        .world()
        .get_resource::<mm2_content::SurfaceTables>()
        .expect("the authored pair mounted for the dev world");
    assert_eq!(
        tables.sound_index(mm2_game::SurfaceMaterial::Unspecified),
        Some(1),
        "the unmarked colliders' fallback reads _default's sound class"
    );

    let install = tempfile::tempdir().unwrap();
    let mut app = dev_app();
    app.insert_resource(Mm2Vfs(support::mount(install.path())));
    app.update();
    assert!(
        app.world()
            .get_resource::<mm2_content::SurfaceTables>()
            .is_none(),
        "an install without the pair mounts nothing"
    );

    let install = tempfile::tempdir().unwrap();
    support::write(install.path(), "city/materials.csv", b"texture,physics\n");
    let mut app = dev_app();
    app.insert_resource(Mm2Vfs(support::mount(install.path())));
    app.update();
    assert!(
        app.world()
            .get_resource::<mm2_content::SurfaceTables>()
            .is_none(),
        "a half pair is a broken table, not an absent one — never substituted"
    );
}

/// The pause overlay's Options row opens the graphics page: Left/Right
/// and Enter change a value on the live settings and save the file at
/// once, Esc backs out to the pause rows (not to the game), and Reset
/// restores the defaults.
#[test]
fn pause_options_change_save_and_back_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = settings_path(dir.path());
    let mut app = dev_app();
    app.insert_resource(GraphicsSettings::default())
        .insert_resource(SettingsFile(Some(path.clone())));
    app.update();
    assert!(phase_is(&mut app, SessionPhase::Playing));

    press_key(&mut app, KeyCode::Escape);
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::Enter);
    assert!(phase_is(&mut app, SessionPhase::Paused));
    assert!(
        app.world().resource::<PauseMenu>().options,
        "the page opens"
    );

    // High -> Off wraps; the resource and the file follow at once.
    press_key(&mut app, KeyCode::ArrowRight);
    let live = *app.world().resource::<GraphicsSettings>();
    assert_eq!(live.shadows, ShadowQuality::Off);
    assert_eq!(GraphicsSettings::load(&path), live);
    // Left steps the other way: Off -> High.
    press_key(&mut app, KeyCode::ArrowLeft);
    assert_eq!(
        app.world().resource::<GraphicsSettings>().shadows,
        ShadowQuality::High
    );

    // Enter on the anti-aliasing row cycles forward (4x -> Off).
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::Enter);
    let live = *app.world().resource::<GraphicsSettings>();
    assert_eq!(live.antialiasing, Antialiasing::Off);
    assert_eq!(GraphicsSettings::load(&path), live);

    // Reset restores both and disables itself.
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(
        *app.world().resource::<GraphicsSettings>(),
        GraphicsSettings::default()
    );
    assert_eq!(GraphicsSettings::load(&path), GraphicsSettings::default());
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.world().resource::<PauseMenu>().status.as_deref(),
        Some("already at the defaults")
    );

    // Esc leaves the page, not the pause: the game stays frozen with
    // Options focused; a second Esc resumes.
    press_key(&mut app, KeyCode::Escape);
    assert!(phase_is(&mut app, SessionPhase::Paused));
    {
        let pause = app.world().resource::<PauseMenu>();
        assert!(!pause.options);
        assert_eq!(pause.focus, 2, "focus returns to the Options row");
    }
    press_key(&mut app, KeyCode::Escape);
    assert!(phase_is(&mut app, SessionPhase::Playing));
}

/// The graphics page's own Back row returns to the pause rows, and a
/// pause that was left on the page opens on the pause rows next time.
#[test]
fn pause_options_back_row_and_a_fresh_pause_start_at_the_top() {
    let mut app = dev_app();
    app.insert_resource(GraphicsSettings::default());
    app.update();

    press_key(&mut app, KeyCode::Escape);
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::ArrowDown);
    press_key(&mut app, KeyCode::Enter);
    assert!(app.world().resource::<PauseMenu>().options);
    for _ in 0..3 {
        press_key(&mut app, KeyCode::ArrowDown);
    }
    press_key(&mut app, KeyCode::Enter);
    assert!(!app.world().resource::<PauseMenu>().options);
    assert!(phase_is(&mut app, SessionPhase::Paused));

    // Back into the page, then resume with Esc twice; the next pause
    // must show the pause rows at the top, not a stale page.
    press_key(&mut app, KeyCode::Enter);
    assert!(app.world().resource::<PauseMenu>().options);
    press_key(&mut app, KeyCode::Escape);
    press_key(&mut app, KeyCode::Escape);
    assert!(phase_is(&mut app, SessionPhase::Playing));
    press_key(&mut app, KeyCode::Escape);
    let pause = app.world().resource::<PauseMenu>();
    assert!(!pause.options);
    assert_eq!(pause.focus, 0);
}
