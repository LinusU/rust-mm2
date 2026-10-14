//! F22-C validation (F22-AC03): the two HUD instruments that can reveal
//! a participant other than the local driver — the checkpoint gantry
//! set and the opponent indicator pool — across teardown, restart and
//! stale state.
//!
//! The legs run the production marker systems (`update_checkpoint_markers`,
//! `drive_opponent_indicators`) and the production session lifecycle
//! (`despawn_session_entities` under [`session::unloading`],
//! `session::drive_session`), with a synthetic authored `hudmap_tri.pkg`
//! bound through the VFS. They assert the two invariants AC03 names:
//!
//! - **Progress is the local driver's.** Both instruments disambiguate
//!   the local seat (the same reading `update_race_warning` uses), so an
//!   opponent banking gates — or ranking ahead — never drives the
//!   course the local driver sees.
//! - **Nothing stale is ever revealed.** A despawned or vehicle-less
//!   participant frees its indicator slot the same update; a race
//!   stamped for another generation cannot drive a gantry; and a
//!   restart despawns every session-owned marker, then re-applies the
//!   driver's `I`/`H` choices to the freshly spawned pool.

use std::path::Path;
use std::time::Duration;

use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::oppind::{
    OppIndReport, OppIndicator, OpponentIndicators, drive_opponent_indicators, indicator_input,
    spawn_opponent_indicators,
};
use mm2_app::race::{CheckpointMarker, spawn_checkpoint_markers, update_checkpoint_markers};
use mm2_app::session::{self, SessionControl, SpawnPoint};
use mm2_assets::Vfs;
use mm2_game::{
    Checkpoint, CheckpointRule, EventParams, EventRef, EventTableKind, Player, PlayerControl,
    RaceDefinition, RaceProgress, RaceStart, RaceState, Session, SessionConfig, SessionEntity,
    SessionMode, SessionPhase, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::{Vehicle, VehicleConfig};

/// One fixed step at the binary's 120 Hz, presented as one 60 Hz frame.
const FRAME: f64 = 1.0 / 60.0;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

/// A two-gate `AnyOrder` course with a finish gate at the far end.
fn any_order_def() -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(100.0, 0.0)],
        finish: Some(cp(200.0, 0.0)),
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 0,
        start_slots: vec![RaceStart {
            position: Vec3::new(-50.0, 0.0, 0.0),
            yaw_deg: Some(0.0),
        }],
    }
}

fn event_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "london".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The production marker systems plus the real session teardown/restart
/// chain, minus windowing and world content. Resource init mirrors
/// `tests/race.rs`'s `race_app`: `drive_session` is the production
/// restart driver, so a restart here travels `Playing → Unloading →
/// Menu → begin` exactly as the binary's does.
fn markers_app(config: SessionConfig, def: RaceDefinition) -> App {
    let mut session = Session::new();
    session.begin(config).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    let generation = session.generation();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            FRAME,
        )))
        .insert_resource(session)
        .insert_resource(RaceState::new(def, generation))
        .insert_resource(SpawnPoint::new(Vec3::ZERO, 0.0))
        .init_resource::<SessionControl>()
        .init_resource::<OpponentIndicators>()
        .init_resource::<mm2_app::hud::HudVisible>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<mm2_app::contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            Update,
            (
                indicator_input,
                drive_opponent_indicators,
                update_checkpoint_markers,
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
            ),
        );
    app.finish();
    app.cleanup();
    app
}

/// Spawn a participant the drive systems can bind — `Player` +
/// `Vehicle` + `RaceProgress` + a `GlobalTransform` written to `pos`
/// verbatim (this minimal app runs no transform propagation), stamped
/// with the owning session generation like every production spawn.
fn participant(app: &mut App, def: &RaceDefinition, control: PlayerControl, pos: Vec3) -> Entity {
    let generation = app.world().resource::<Session>().generation();
    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let cfg = VehicleConfig::default();
    app.world_mut()
        .spawn((
            SessionEntity(generation),
            Player { id, control },
            RaceProgress::new(def),
            Vehicle { config: cfg },
            Transform::from_translation(pos),
            GlobalTransform::from(Transform::from_translation(pos)),
        ))
        .id()
}

/// Spawn the opponent pool through `spawn_opponent_indicators`, sized to
/// `roster` and owned by the current session. Takes the three asset
/// stores out of the world and puts them back — the same dance
/// `tests/oppind.rs` does with `resource_scope`, without the nested
/// borrows.
fn spawn_indicator_pool(app: &mut App, vfs: &Vfs, roster: usize) {
    let generation = app.world().resource::<Session>().generation();
    let mut meshes = app.world_mut().remove_resource::<Assets<Mesh>>().unwrap();
    let mut images = app.world_mut().remove_resource::<Assets<Image>>().unwrap();
    let mut materials = app
        .world_mut()
        .remove_resource::<Assets<StandardMaterial>>()
        .unwrap();
    let mut queue = CommandQueue::default();
    let report = {
        let world = app.world_mut();
        let mut commands = Commands::new(&mut queue, world);
        spawn_opponent_indicators(
            &mut commands,
            vfs,
            roster,
            &mut meshes,
            &mut images,
            &mut materials,
            SessionEntity(generation),
        )
    };
    queue.apply(app.world_mut());
    app.world_mut().insert_resource(meshes);
    app.world_mut().insert_resource(images);
    app.world_mut().insert_resource(materials);
    app.world_mut().insert_resource(report);
}

/// Spawn the checkpoint gantries through `spawn_checkpoint_markers` for
/// the current session.
fn spawn_gantries(app: &mut App, def: &RaceDefinition) {
    let generation = app.world().resource::<Session>().generation();
    let mut meshes = app.world_mut().remove_resource::<Assets<Mesh>>().unwrap();
    let mut images = app.world_mut().remove_resource::<Assets<Image>>().unwrap();
    let mut materials = app
        .world_mut()
        .remove_resource::<Assets<StandardMaterial>>()
        .unwrap();
    let mut queue = CommandQueue::default();
    {
        let world = app.world_mut();
        let mut commands = Commands::new(&mut queue, world);
        spawn_checkpoint_markers(
            &mut commands,
            &mut meshes,
            &mut images,
            &mut materials,
            def,
            SessionEntity(generation),
        );
    }
    queue.apply(app.world_mut());
    app.world_mut().insert_resource(meshes);
    app.world_mut().insert_resource(images);
    app.world_mut().insert_resource(materials);
}

fn report(app: &App) -> &OppIndReport {
    app.world().resource::<OppIndReport>()
}

/// The visible opponent markers and their world positions.
fn visible_indicators(app: &mut App) -> Vec<(Entity, Vec3)> {
    let mut q = app
        .world_mut()
        .query_filtered::<(Entity, &Transform, &Visibility), With<OppIndicator>>();
    q.iter(app.world())
        .filter(|(_, _, v)| **v == Visibility::Visible)
        .map(|(e, t, _)| (e, t.translation))
        .collect()
}

/// The root gantry's visibility for `gate` (`None` = the finish).
fn gate_vis(app: &mut App, gate: Option<usize>) -> Visibility {
    let mut q = app.world_mut().query::<(&CheckpointMarker, &Visibility)>();
    q.iter(app.world())
        .find(|(m, _)| m.gate == gate)
        .map(|(_, v)| *v)
        .unwrap_or_else(|| panic!("no marker for gate {gate:?}"))
}

/// Force a gantry's visibility out of band, so a following update can be
/// checked for having (or not having) re-driven it.
fn set_gate_vis(app: &mut App, gate: Option<usize>, vis: Visibility) {
    let mut q = app
        .world_mut()
        .query::<(&CheckpointMarker, &mut Visibility)>();
    let mut found = false;
    for (m, mut v) in q.iter_mut(app.world_mut()) {
        if m.gate == gate {
            *v = vis;
            found = true;
        }
    }
    assert!(found, "no marker for gate {gate:?}");
}

fn count_mesh3d(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<Mesh3d>>()
        .iter(app.world())
        .count()
}

fn count_indicators(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<Entity, With<OppIndicator>>()
        .iter(app.world())
        .count()
}

/// One `I` edge through the production `indicator_input` toggle.
fn press_i(app: &mut App) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyI);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyI);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
    // `indicator_input` and `drive_opponent_indicators` share the Update
    // schedule; the extra frame lets the pool observe the new toggle.
    app.update();
}

/// Drive a restart through the production lifecycle; returns once the
/// new session has re-begun.
fn restart(app: &mut App) {
    app.world_mut().resource_mut::<SessionControl>().restart = true;
    let mut reached = false;
    for _ in 0..12 {
        app.update();
        if app.world().resource::<Session>().phase() == &SessionPhase::Loading {
            reached = true;
            break;
        }
    }
    assert!(reached, "restart never re-began the session");
}

// ---------------------------------------------------------------------------
// Authored `hudmap_tri.pkg` fixture (the same shape tests/oppind.rs builds)
// ---------------------------------------------------------------------------

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn push_lp(out: &mut Vec<u8>, s: &str) {
    out.push(s.len() as u8 + 1);
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn push_f32s(out: &mut Vec<u8>, v: &[f32]) {
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
}

/// A minimal valid `PKG3` with one triangle and `paints` paint jobs —
/// `hudmap_tri` needs ≥9 for `TRI_PAINT_OPPONENTS` to index.
fn synthetic_pkg(verts: &[[f32; 3]], indices: &[u16], paints: u32) -> Vec<u8> {
    let fvf: u32 = 0x002; // FVF_XYZ
    let mut geo = Vec::new();
    geo.extend_from_slice(&1u32.to_le_bytes());
    geo.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    geo.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    geo.extend_from_slice(&0u32.to_le_bytes());
    geo.extend_from_slice(&fvf.to_le_bytes());
    geo.extend_from_slice(&1u16.to_le_bytes());
    geo.extend_from_slice(&0u16.to_le_bytes());
    geo.extend_from_slice(&0i32.to_le_bytes());
    geo.extend_from_slice(&3i32.to_le_bytes());
    geo.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    for v in verts {
        push_f32s(&mut geo, v);
    }
    geo.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for i in indices {
        geo.extend_from_slice(&i.to_le_bytes());
    }

    let mut shaders = Vec::new();
    shaders.extend_from_slice(&paints.to_le_bytes());
    shaders.extend_from_slice(&1u32.to_le_bytes());
    for _ in 0..paints {
        push_lp(&mut shaders, "");
        push_f32s(&mut shaders, &[0.9, 0.8, 0.2, 1.0]);
        push_f32s(&mut shaders, &[1.0, 1.0, 1.0, 1.0]);
        push_f32s(&mut shaders, &[0.0, 0.0, 0.0, 1.0]);
        push_f32s(&mut shaders, &[0.0, 0.0, 0.0, 1.0]);
        push_f32s(&mut shaders, &[0.0]);
    }

    let mut pkg = Vec::new();
    pkg.extend_from_slice(b"PKG3");
    for (name, payload) in [("H", geo), ("shaders", shaders)] {
        pkg.extend_from_slice(b"FILE");
        push_lp(&mut pkg, name);
        pkg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        pkg.extend_from_slice(&payload);
    }
    pkg
}

/// A mount carrying only the authored marker package the pool binds.
fn tri_mount() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "geometry/hudmap_tri.pkg",
        synthetic_pkg(
            &[[-8., 0., 14.], [8., 0., 14.], [0., 0., -14.]],
            &[0, 1, 2],
            10,
        ),
    );
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

// ---------------------------------------------------------------------------
// Opponent indicators: no invalid/stale participant, H/I survive restart
// ---------------------------------------------------------------------------

/// The indicator pool binds only live non-local participants — a
/// despawned or vehicle-less opponent frees its slot the same update, the
/// local car never carries a marker — and a restart despawns the pool
/// while the session-agnostic `I` choice rides across into a fresh pool
/// that re-applies it.
#[test]
fn opponent_markers_release_stale_participants_and_rebind_across_restart() {
    let def = any_order_def();
    let mut app = markers_app(event_config(), def.clone());
    let tmp = tri_mount();
    let vfs = vfs_of(tmp.path());
    spawn_indicator_pool(&mut app, &vfs, 2);
    assert_eq!(report(&app).markers, 2);
    assert!(
        report(&app).absent.is_none(),
        "the authored tri must bind: {:?}",
        report(&app).absent
    );

    // The world view the markers face.
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            is_active: true,
            ..default()
        },
        Transform::from_xyz(0.0, 30.0, 60.0),
        GlobalTransform::from_xyz(0.0, 30.0, 60.0),
    ));

    let local = participant(
        &mut app,
        &def,
        PlayerControl::Local,
        Vec3::new(10.0, 0.0, 0.0),
    );
    let ai_a = participant(&mut app, &def, PlayerControl::Ai, Vec3::ZERO);
    let ai_b = participant(&mut app, &def, PlayerControl::Ai, Vec3::new(0.0, 0.0, 20.0));
    app.update();

    let shown = visible_indicators(&mut app);
    assert_eq!(shown.len(), 2, "both opponents ride the two-slot pool");
    for (_, p) in &shown {
        assert!(
            (p.x - 10.0).abs() > 1.0,
            "the local car never carries a marker: {p:?}"
        );
    }
    assert_eq!(report(&app).bound, 2);

    // A despawned opponent frees its slot the same update — the
    // indicator can never hover over a stale participant.
    app.world_mut().despawn(ai_b);
    app.update();
    assert_eq!(visible_indicators(&mut app).len(), 1);
    assert_eq!(report(&app).bound, 1, "the stale slot is released");

    // A vehicle-less participant is no longer a valid target either.
    app.world_mut().entity_mut(ai_a).remove::<Vehicle>();
    app.update();
    assert!(visible_indicators(&mut app).is_empty());
    assert_eq!(report(&app).bound, 0);

    // A live opponent returns the marker.
    let ai_c = participant(&mut app, &def, PlayerControl::Ai, Vec3::new(0.0, 0.0, 30.0));
    app.update();
    assert_eq!(visible_indicators(&mut app).len(), 1);
    assert_eq!(report(&app).bound, 1);

    // `H` hides the whole layer but never loses the demand count.
    app.world_mut().resource_mut::<mm2_app::hud::HudVisible>().0 = false;
    app.update();
    assert!(
        visible_indicators(&mut app).is_empty(),
        "H suppresses the markers with the rest of the layer"
    );
    assert_eq!(report(&app).bound, 1, "H does not lose the demand count");
    app.world_mut().resource_mut::<mm2_app::hud::HudVisible>().0 = true;
    app.update();
    assert_eq!(visible_indicators(&mut app).len(), 1, "H off restores them");

    // `I` in the live phases toggles the pool off. The default is on, so
    // one press leaves the choice off — a session-agnostic resource that
    // outlives the restart.
    press_i(&mut app);
    assert!(
        !app.world().resource::<OpponentIndicators>().0,
        "I toggled the indicators off"
    );
    assert!(visible_indicators(&mut app).is_empty());

    // Restart through the production lifecycle: teardown despawns the
    // pool and the session-owned participants.
    restart(&mut app);
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    assert_eq!(
        count_indicators(&mut app),
        0,
        "teardown despawned the old pool"
    );
    assert!(
        app.world().get_entity(local).is_err(),
        "the session-owned local participant despawned"
    );
    assert!(
        app.world().get_entity(ai_c).is_err(),
        "no pre-restart opponent survives to be revealed"
    );
    assert!(
        !app.world().resource::<OpponentIndicators>().0,
        "the I choice survived the restart"
    );

    // The loader rebuilds the world for the new generation; the fresh
    // pool re-applies the driver's choice and binds only the new roster.
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Ready)
        .unwrap();
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Countdown)
        .unwrap();
    spawn_indicator_pool(&mut app, &vfs, 2);
    let new_local = participant(
        &mut app,
        &def,
        PlayerControl::Local,
        Vec3::new(10.0, 0.0, 0.0),
    );
    let new_ai = participant(&mut app, &def, PlayerControl::Ai, Vec3::ZERO);
    app.update();

    assert_eq!(
        report(&app).markers,
        2,
        "the fresh pool is sized to the roster"
    );
    assert_eq!(report(&app).bound, 1, "only the new opponent binds");
    assert!(
        visible_indicators(&mut app).is_empty(),
        "the persisted I=off is re-applied to the fresh pool"
    );

    press_i(&mut app);
    assert!(app.world().resource::<OpponentIndicators>().0);
    let shown = visible_indicators(&mut app);
    assert_eq!(shown.len(), 1, "I on reveals the new opponent");
    assert!(
        (shown[0].1.x - 10.0).abs() > 1.0,
        "the marker hovers the new opponent, not the new local: {:?}",
        shown[0].1
    );
    assert_ne!(new_local, new_ai);
}

// ---------------------------------------------------------------------------
// Checkpoint gantries: local progress, never an opponent or a stale race
// ---------------------------------------------------------------------------

/// The gantries read the local driver's progress — an opponent banking
/// every gate changes nothing — and clearing a gate hides exactly that
/// gate, revealing the finish only once the local has cleared them all.
#[test]
fn checkpoint_markers_follow_the_local_driver_not_an_opponent() {
    let def = any_order_def();
    let mut app = markers_app(event_config(), def.clone());
    spawn_gantries(&mut app, &def);
    // The opponent spawns *first*: both share one archetype, so entity
    // order would otherwise hand the local the first slot and a
    // `participants.iter().next()` regression would pass unnoticed. The
    // disambiguation `update_checkpoint_markers` performs is the whole
    // point of this leg, so the local must not be the naive pick.
    let ai = participant(&mut app, &def, PlayerControl::Ai, Vec3::ZERO);
    let local = participant(
        &mut app,
        &def,
        PlayerControl::Local,
        Vec3::new(-10.0, 0.0, 0.0),
    );
    app.update();

    assert_eq!(gate_vis(&mut app, Some(0)), Visibility::Visible);
    assert_eq!(gate_vis(&mut app, Some(1)), Visibility::Visible);
    assert_eq!(gate_vis(&mut app, None), Visibility::Hidden);

    // An opponent banking all of them leaves the local's view intact —
    // the markers never reveal the opponent's progress.
    app.world_mut()
        .get_mut::<RaceProgress>(ai)
        .unwrap()
        .apply_replicated(0b11, 2, 0, 2, 0);
    app.update();
    assert_eq!(
        gate_vis(&mut app, Some(0)),
        Visibility::Visible,
        "an opponent's clear never hides the local's gate"
    );
    assert_eq!(gate_vis(&mut app, Some(1)), Visibility::Visible);
    assert_eq!(gate_vis(&mut app, None), Visibility::Hidden);

    // The local clearing gate 0 hides exactly that gate.
    app.world_mut()
        .get_mut::<RaceProgress>(local)
        .unwrap()
        .apply_replicated(0b01, 1, 0, 1, 0);
    app.update();
    assert_eq!(gate_vis(&mut app, Some(0)), Visibility::Hidden);
    assert_eq!(gate_vis(&mut app, Some(1)), Visibility::Visible);
    assert_eq!(gate_vis(&mut app, None), Visibility::Hidden);

    // Clearing the second reveals the finish.
    app.world_mut()
        .get_mut::<RaceProgress>(local)
        .unwrap()
        .apply_replicated(0b11, 2, 0, 2, 0);
    app.update();
    assert_eq!(gate_vis(&mut app, Some(0)), Visibility::Hidden);
    assert_eq!(gate_vis(&mut app, Some(1)), Visibility::Hidden);
    assert_eq!(
        gate_vis(&mut app, None),
        Visibility::Visible,
        "clearing every gate reveals the finish line"
    );
}

/// Under `Ordered` the gates reset each lap, so they never hide mid-race
/// (the finish's "appears when the course is complete" rule is the
/// `AnyOrder`/phase one).
#[test]
fn ordered_gates_stay_visible_through_the_lap() {
    let mut def = any_order_def();
    def.rule = CheckpointRule::Ordered;
    let mut app = markers_app(event_config(), def.clone());
    spawn_gantries(&mut app, &def);
    let local = participant(&mut app, &def, PlayerControl::Local, Vec3::ZERO);
    app.update();
    app.world_mut()
        .get_mut::<RaceProgress>(local)
        .unwrap()
        .apply_replicated(0b11, 0, 1, 2, 0);
    app.update();
    assert_eq!(
        gate_vis(&mut app, Some(0)),
        Visibility::Visible,
        "Ordered gates reset each lap and stay up"
    );
    assert_eq!(gate_vis(&mut app, Some(1)), Visibility::Visible);
}

/// A `RaceState` stamped for another generation is never the markers'
/// input: a stale pass leaves them exactly as they were, and the live
/// generation drives them again.
#[test]
fn a_stale_race_generation_never_drives_the_markers() {
    let def = any_order_def();
    let mut app = markers_app(event_config(), def.clone());
    spawn_gantries(&mut app, &def);
    let local = participant(&mut app, &def, PlayerControl::Local, Vec3::ZERO);
    app.world_mut()
        .get_mut::<RaceProgress>(local)
        .unwrap()
        .apply_replicated(0b11, 2, 0, 2, 0);
    app.update();
    assert_eq!(gate_vis(&mut app, Some(0)), Visibility::Hidden);
    assert_eq!(gate_vis(&mut app, None), Visibility::Visible);

    // Bump the race out of step and force a marker out of band; a stale
    // pass must not re-drive it.
    set_gate_vis(&mut app, Some(0), Visibility::Visible);
    app.world_mut().resource_mut::<RaceState>().generation += 1;
    app.update();
    assert_eq!(
        gate_vis(&mut app, Some(0)),
        Visibility::Visible,
        "a stale race never re-drives the markers"
    );

    // The live generation still owns them.
    app.world_mut().resource_mut::<RaceState>().generation -= 1;
    app.update();
    assert_eq!(gate_vis(&mut app, Some(0)), Visibility::Hidden);
    assert_eq!(gate_vis(&mut app, None), Visibility::Visible);
}

/// A restart tears the gantries down with the session — no mesh, no
/// marker survives — and the rebuilt race's fresh gantry set follows the
/// new session's local driver.
#[test]
fn checkpoint_markers_despawn_on_teardown_and_follow_the_fresh_race() {
    let def = any_order_def();
    let mut app = markers_app(event_config(), def.clone());
    spawn_gantries(&mut app, &def);
    let local = participant(&mut app, &def, PlayerControl::Local, Vec3::ZERO);
    app.world_mut()
        .get_mut::<RaceProgress>(local)
        .unwrap()
        .apply_replicated(0b01, 1, 0, 1, 0);
    app.update();
    assert_eq!(gate_vis(&mut app, Some(0)), Visibility::Hidden);
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<CheckpointMarker>>()
            .iter(app.world())
            .count(),
        3,
        "two gates plus the finish"
    );

    restart(&mut app);
    assert_eq!(app.world().resource::<Session>().generation(), 2);
    assert!(
        app.world().get_resource::<RaceState>().is_none(),
        "the old race must not survive the restart"
    );
    assert_eq!(
        app.world_mut()
            .query_filtered::<Entity, With<CheckpointMarker>>()
            .iter(app.world())
            .count(),
        0,
        "teardown despawned every gantry"
    );
    assert_eq!(
        count_mesh3d(&mut app),
        0,
        "no session-owned mesh survived to reveal a stale course"
    );
    assert!(
        app.world().get_entity(local).is_err(),
        "the session-owned participant despawned"
    );

    // The loader rebuilds the race for the new generation; the fresh
    // gantry set follows only the new session's driver.
    let new_gen = app.world().resource::<Session>().generation();
    app.world_mut()
        .insert_resource(RaceState::new(def.clone(), new_gen));
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Ready)
        .unwrap();
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Countdown)
        .unwrap();
    spawn_gantries(&mut app, &def);
    let new_local = participant(
        &mut app,
        &def,
        PlayerControl::Local,
        Vec3::new(5.0, 0.0, 0.0),
    );
    app.update();
    assert_eq!(
        gate_vis(&mut app, Some(0)),
        Visibility::Visible,
        "the fresh race shows a fresh course"
    );
    assert_eq!(gate_vis(&mut app, None), Visibility::Hidden);

    app.world_mut()
        .get_mut::<RaceProgress>(new_local)
        .unwrap()
        .apply_replicated(0b11, 2, 0, 2, 0);
    app.update();
    assert_eq!(gate_vis(&mut app, Some(0)), Visibility::Hidden);
    assert_eq!(gate_vis(&mut app, None), Visibility::Visible);
}
