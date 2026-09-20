//! Session lifecycle driving in the app (F01-C).
//!
//! `mm2_game::Session` owns the phase state machine; this module drives it
//! inside the real app:
//!
//! - [`load_session_world`] runs while the phase is `Loading` — it spawns
//!   the world, HUD, cameras and the player vehicle (all stamped with the
//!   session's [`SessionEntity`] generation) and drives `Loading → Ready →
//!   Playing`, or `→ Failed` on a load error.
//! - [`session_control_input`] maps keys onto [`SessionControl`] intents:
//!   `Esc` quits (tears down, then exits — there is no menu yet, F17),
//!   `Backspace` restarts the session with the same config.
//! - `despawn_session_entities` (mm2_game, scheduled while `Unloading`)
//!   removes every session-owned root; [`drive_session`] waits for the
//!   world to be observably empty, clears session-scoped caches
//!   ([`ImpactFilter`](crate::contracts::ImpactFilter), trailer spawn
//!   bookkeeping) and moves `Unloading → Menu`. At `Menu` a `restart`
//!   intent calls `Session::begin` again — which flips the phase back to
//!   `Loading` and re-runs [`load_session_world`] — while `quit` writes
//!   `AppExit`.
//!
//! The cycle is `Playing → Unloading → Menu → Loading → Playing`; every
//! start goes through `Menu`, so session-owned entities are always cleaned
//! between runs (AC01) and a failed load can never leave a live player
//! simulation (AC02).

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_content::VehicleDef;
use mm2_game::{
    DamageSignals, Mm2Vfs, ObjectIdentity, Player, PlayerControl, PlayerVehicle, RaceDefinition,
    RaceProgress, RaceState, Session, SessionEntity, SessionMode, SessionPhase, TargetSelection,
    WorldMode,
};
use mm2_vehicle::{VehicleConfig, vehicle_bundle};
use tracing::{error, info, warn};

use crate::camera::{CameraMode, ChaseCamera, FreeCamera};
use crate::car_visual::{self, WheelMount, WheelSpin};
use crate::contracts::ImpactFilter;
use crate::{city, dev_world, race};

/// Where the player vehicle (re)spawns. `trailers` holds each spawned
/// trailer's entity plus its car-space rest offset so a reset can place it
/// back behind the car instead of on top of it. Session-scoped: teardown
/// clears `trailers`, and the next session's spawn rewrites position/yaw.
#[derive(Resource)]
pub struct SpawnPoint {
    pub position: Vec3,
    pub yaw: f32,
    pub trailers: Vec<(Entity, Vec3)>,
}

/// The imported stock vehicle selected by `--car` or the deterministic
/// stock default (`vpbug`). `None` = synthetic dev car.
#[derive(Resource)]
pub struct SelectedCar {
    pub def: Option<VehicleDef>,
    pub paint: usize,
}

/// The validated vehicle configuration the player car was built from.
#[derive(Resource)]
pub struct TunedVehicle(pub VehicleConfig);

/// Marker for the on-screen HUD text.
#[derive(Component)]
pub struct Hud;

/// Marker for the big error line shown when the world fails to load.
#[derive(Component)]
pub struct ErrorText;

/// What the player asked the session to do next. Written by
/// [`session_control_input`], consumed by [`drive_session`]. `quit` wins
/// over `restart` if both are set in the same frame.
#[derive(Resource, Default)]
pub struct SessionControl {
    /// Tear down, reach `Menu`, then exit the app.
    pub quit: bool,
    /// Tear down, then `begin` a new session with the same config.
    pub restart: bool,
}

/// Run condition: the session is in `Loading` — gates
/// [`load_session_world`] so it runs exactly once per `begin`.
pub fn loading(session: Res<Session>) -> bool {
    matches!(session.phase(), SessionPhase::Loading)
}

/// Run condition: the session is in `Unloading` — gates
/// `despawn_session_entities` so teardown only runs while tearing down.
pub fn unloading(session: Res<Session>) -> bool {
    matches!(session.phase(), SessionPhase::Unloading)
}

/// `Esc` asks to quit, `Backspace` asks to restart. Intents are only read
/// from the phases a session can sit in — while `Loading`, `Unloading` or
/// `Menu` the driver is already working and input is ignored. `Countdown`
/// is quittable: a race that has not started still tears down like any
/// other live session.
pub fn session_control_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    mut control: ResMut<SessionControl>,
) {
    let quittable = matches!(
        session.phase(),
        SessionPhase::Countdown
            | SessionPhase::Playing
            | SessionPhase::Paused
            | SessionPhase::Results
            | SessionPhase::Failed(_)
    );
    if !quittable {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        control.quit = true;
    }
    if keys.just_pressed(KeyCode::Backspace) {
        control.restart = true;
    }
}

/// Advance the session lifecycle one step per call. Scheduled after
/// `despawn_session_entities` so an `Unloading` frame despawns first,
/// then this observes the empty world before declaring `Menu`:
///
/// - `Unloading`: once no session-owned roots remain, clear
///   session-scoped caches — the impact dedup map is keyed by `Entity`,
///   which the next session may recycle — and move to `Menu`.
/// - `Menu`: `quit` exits the process; `restart` calls `begin` with the
///   retained config, flipping the phase to `Loading` so the spawn system
///   builds the next session.
/// - `Countdown`/`Playing`/`Paused`/`Results`/`Failed`: a queued intent
///   moves the session to `Unloading`; teardown proceeds on later
///   frames.
pub fn drive_session(
    mut commands: Commands,
    mut session: ResMut<Session>,
    mut control: ResMut<SessionControl>,
    mut filter: ResMut<ImpactFilter>,
    mut spawn: ResMut<SpawnPoint>,
    roots: Query<Entity, (With<SessionEntity>, Without<ChildOf>)>,
    mut exit: MessageWriter<AppExit>,
) {
    match *session.phase() {
        SessionPhase::Unloading => {
            if !roots.is_empty() {
                // The chained despawn has not flushed yet — stay in
                // Unloading until teardown is observably complete.
                return;
            }
            filter.reset();
            spawn.trailers.clear();
            // Session-scoped resources die with the session: a race's
            // countdown/clock/progress and the city's nav overlay must
            // never survive into the next session (AC03 — no old
            // timer survives).
            commands.remove_resource::<RaceState>();
            commands.remove_resource::<crate::nav_overlay::CityNav>();
            session
                .transition(SessionPhase::Menu)
                .expect("Unloading → Menu is a legal transition");
        }
        SessionPhase::Menu => {
            if control.quit {
                control.quit = false;
                exit.write(AppExit::Success);
            } else if control.restart {
                control.restart = false;
                match session.config().cloned() {
                    Some(config) => {
                        if let Err(e) = session.begin(config) {
                            // The retained config already validated once;
                            // a failure here means the session state is
                            // inconsistent — log and stay at Menu.
                            error!(error = %e, "session restart rejected");
                        }
                    }
                    None => warn!("restart requested with no previous session"),
                }
            }
        }
        SessionPhase::Countdown
        | SessionPhase::Playing
        | SessionPhase::Paused
        | SessionPhase::Results
        | SessionPhase::Failed(_)
            if control.quit || control.restart =>
        {
            session
                .transition(SessionPhase::Unloading)
                .expect("live/failed session → Unloading is a legal transition");
        }
        // Loading/Ready: transient phases this app drives synchronously
        // — no queued intent handling here.
        _ => {}
    }
}

/// The asset collections world spawning writes into.
#[derive(bevy::ecs::system::SystemParam)]
pub struct AssetStores<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    images: ResMut<'w, Assets<Image>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

/// Spawn the world, vehicle, cameras, HUD and lights per the session
/// config, driving `Loading → Ready → Playing` (or `Failed`). Everything
/// spawned is stamped with the session's `SessionEntity` generation so a
/// later `Unloading` removes the whole session, not a subset.
#[allow(clippy::too_many_arguments)]
pub fn load_session_world(
    mut commands: Commands,
    mut assets: AssetStores,
    mut session: ResMut<Session>,
    vfs: Res<Mm2Vfs>,
    vehicle_config: Res<TunedVehicle>,
    selected: Res<SelectedCar>,
    cam_mode: Res<CameraMode>,
    mut spawn: ResMut<SpawnPoint>,
) {
    let owner = SessionEntity(session.generation());
    let Some(config) = session.config().cloned() else {
        error!("load_session_world ran without a session config");
        return;
    };
    let mut world_ok = true;
    match &config.world {
        WorldMode::DevWorld => {
            dev_world::spawn_dev_world(
                &mut commands,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                &vfs.0,
                owner,
            );
            spawn.position = Vec3::new(0.0, 1.5, 0.0);
            spawn.yaw = 0.0;
        }
        WorldMode::City { psdl } => {
            match city::load_city(
                &mut commands,
                &vfs.0,
                psdl,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                owner,
                &mut session,
            ) {
                Ok(loaded) => {
                    spawn.position = loaded.spawn;
                    spawn.yaw = loaded.spawn_yaw;
                    info!(report = %loaded.report, "city ready");
                }
                Err(e) => {
                    error!(error = %e, "city failed to load");
                    session
                        .fail(format!("{e}"))
                        .expect("Loading → Failed is a legal transition");
                    world_ok = false;
                }
            }
            // F09-B diagnostics: the `--nav` overlay loads the city's
            // navigation graph + aimap overrides as a session resource.
            // A failed nav load logs and draws nothing — it never
            // sinks an otherwise loadable city.
            if world_ok && let Some(nav) = crate::nav_overlay::load_city_nav(&vfs.0, &config) {
                commands.insert_resource(nav);
            }
            // City lighting.
            commands.spawn((
                owner,
                DirectionalLight {
                    illuminance: 15_000.0,
                    shadow_maps_enabled: true,
                    ..default()
                },
                Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.6, -0.9, 0.0)),
            ));
            commands.insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.7, 0.75, 0.85),
                brightness: 400.0,
                affects_lightmapped_meshes: false,
            });
        }
    }
    // Event mode: resolve the catalog event into the shared race
    // definition before the session is declared Ready. An event that
    // cannot load fails the session — it never silently cruises. The
    // authored player slot replaces the world's roam spawn.
    let mut event_race: Option<RaceDefinition> = None;
    if world_ok && let SessionMode::Event(event_ref) = &config.mode {
        match race::event_race_setup(&vfs.0, event_ref, config.difficulty) {
            Ok(setup) => {
                let def = setup.definition;
                // The player slot overrides the world's roam spawn; an
                // event without slots keeps the roam spawn.
                if let Some(slot) = def.start_slots.get(mm2_content::PLAYER_SLOT) {
                    spawn.position = slot.position;
                    // `RaceStart.yaw_deg` uses the authored `a`
                    // convention (forward = (sin a, cos a) in XZ);
                    // spawn yaw is the `Quat::from_rotation_y` angle
                    // whose forward is (−sin θ, −cos θ) — vehicle
                    // forward is local −Z.
                    let a = slot.yaw_deg.to_radians();
                    let forward = Vec2::new(a.sin(), a.cos());
                    spawn.yaw = (-forward.x).atan2(-forward.y);
                }
                info!(
                    event = %format!("{:?}[{}]", event_ref.table, event_ref.index),
                    gates = def.checkpoints.len(),
                    "event race loaded"
                );
                // The event's `.pathset` overlays (F03-AC04): course
                // barricades, jumps and prop arrangements stamp as
                // session-owned placements, so teardown removes
                // exactly this event's objects — re-entry stamps the
                // overlay once again, never duplicated.
                let overlay = city::spawn_event_pathsets(
                    &mut commands,
                    &vfs.0,
                    &setup.pathsets,
                    &mut assets.meshes,
                    &mut assets.images,
                    &mut assets.materials,
                    owner,
                    &mut session,
                );
                if overlay.files > 0 {
                    info!(
                        files = overlay.files,
                        stamped = overlay.stats.spawned,
                        labels = overlay.stats.label_paths,
                        animated = overlay.stats.animated_paths,
                        decals = overlay.stats.decal_paths,
                        unresolved = overlay.stats.unresolved_paths,
                        capped = overlay.stats.capped,
                        issues = overlay.stats.issues,
                        failed_files = overlay.failed_files.len(),
                        "event pathset overlay stamped"
                    );
                }
                event_race = Some(def);
            }
            Err(e) => {
                error!(error = %e, event = ?event_ref, "event failed to load");
                session
                    .fail(format!(
                        "event {:?}[{}]: {e}",
                        event_ref.table, event_ref.index
                    ))
                    .expect("Loading → Failed is a legal transition");
                world_ok = false;
            }
        }
    }
    if world_ok {
        session
            .transition(SessionPhase::Ready)
            .expect("Loading → Ready is a legal transition");
    }

    // HUD + error text.
    commands.spawn((
        owner,
        Hud,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.95, 0.95)),
        // Keeps the line legible over bright facades and sky.
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            ..default()
        },
    ));
    commands.spawn((
        owner,
        ErrorText,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(22.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.4, 0.35)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(120.0),
            left: Val::Px(40.0),
            ..default()
        },
    ));

    // Cameras. The chase boom is sized to the selected vehicle so a city
    // bus and a roadster are both framed sensibly.
    let chase = match &selected.def {
        Some(def) => {
            let [_w, h, d] = def.config.chassis_size;
            ChaseCamera {
                distance: d * 0.85 + 3.5,
                height: h * 0.55 + 1.4,
                look_height: h * 0.45,
                ..default()
            }
        }
        None => ChaseCamera::default(),
    };
    commands.spawn((
        owner,
        Camera3d::default(),
        Camera {
            is_active: *cam_mode == CameraMode::Chase,
            ..default()
        },
        chase,
        Transform::from_translation(spawn.position + Vec3::new(0.0, 4.0, 9.0)),
    ));
    let (free_xf, free_cam) = match config.dev.camera.as_ref() {
        Some(c) => (
            Transform::from_translation(c.position).with_rotation(Quat::from_euler(
                EulerRot::YXZ,
                c.yaw,
                c.pitch,
                0.0,
            )),
            FreeCamera {
                yaw: c.yaw,
                pitch: c.pitch,
                ..default()
            },
        ),
        None => (
            Transform::from_translation(spawn.position + Vec3::new(0.0, 8.0, 12.0)),
            FreeCamera::default(),
        ),
    };
    commands.spawn((
        owner,
        Camera3d::default(),
        Camera {
            is_active: *cam_mode == CameraMode::Free,
            ..default()
        },
        free_cam,
        free_xf,
    ));

    // The dynamic player spawns only once the world is `Ready` — after the
    // static colliders above exist, so it can't fall through a half-built
    // city.
    if !world_ok {
        return;
    }
    let vehicle_cfg = &vehicle_config.0;

    // Spawn clearance: keep the collider hull's lowest point off the
    // ground plus a settle margin.
    if let Some(def) = &selected.def {
        let hull_min_y = def
            .config
            .collider_points
            .as_ref()
            .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::min))
            .unwrap_or(-def.config.chassis_size[1] * 0.5);
        spawn.position.y += (0.25 - hull_min_y).max(0.35);
    }
    // Stable identities + authority role for the contract consumers
    // (telemetry, impacts, results): the entity gets a session-minted
    // `ObjectId`, its driver a `PlayerId`, and its rules the session's
    // authority boundary — local play stamps `Authority`.
    let vehicle_object = session.mint_object_id();
    let player_id = session.mint_player_id();
    let role = session.authority_role();
    let vehicle = commands
        .spawn((
            PlayerVehicle,
            owner,
            ObjectIdentity(vehicle_object),
            Player {
                id: player_id,
                control: PlayerControl::Local,
            },
            role,
            DamageSignals::default(),
            vehicle_bundle(&vehicle_config.0),
            Transform::from_translation(spawn.position)
                .with_rotation(Quat::from_rotation_y(spawn.yaw)),
            TransformInterpolation,
            // Parents of renderable children need the visibility chain.
            Visibility::Visible,
        ))
        .id();

    match &selected.def {
        // Imported stock vehicle: the model carries the visuals.
        Some(def) => {
            let missing = car_visual::spawn_vehicle_model(
                &mut commands,
                &vfs.0,
                &def.model,
                selected.paint,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                vehicle,
            );
            if !missing.is_empty() {
                warn!(car = %def.id, "missing textures: {}", missing.join(", "));
            }
            if let Some(trailer) = &def.trailer {
                let car_xf = Transform::from_translation(spawn.position)
                    .with_rotation(Quat::from_rotation_y(spawn.yaw));
                let (te, tmissing) = car_visual::spawn_trailer(
                    &mut commands,
                    &vfs.0,
                    trailer,
                    selected.paint,
                    &mut assets.meshes,
                    &mut assets.images,
                    &mut assets.materials,
                    vehicle,
                    car_xf,
                    owner,
                );
                if !tmissing.is_empty() {
                    warn!(car = %def.id, "trailer missing textures: {}", tmissing.join(", "));
                }
                // The trailer is a simulated object too — stable id, the
                // session's authority role and its own damage signals,
                // but no player driver.
                commands.entity(te).insert((
                    ObjectIdentity(session.mint_object_id()),
                    role,
                    DamageSignals::default(),
                ));
                spawn.trailers.push((
                    te,
                    Vec3::from(trailer.car_hitch) - Vec3::from(trailer.trailer_hitch),
                ));
            }
        }
        // Synthetic dev car: cuboid body + cylinder wheels, same
        // mount/spin rig as imported wheels.
        None => {
            let body_mesh = assets
                .meshes
                .add(Cuboid::from_size(Vec3::from(vehicle_cfg.chassis_size)));
            let body_mat = assets.materials.add(StandardMaterial {
                base_color: Color::srgb(0.85, 0.15, 0.1),
                metallic: 0.3,
                perceptual_roughness: 0.5,
                ..default()
            });
            commands
                .entity(vehicle)
                .insert((Mesh3d(body_mesh), MeshMaterial3d(body_mat)));
            let wheel_mesh = assets.meshes.add(Cylinder::new(0.34, 0.25));
            let wheel_mat = assets.materials.add(StandardMaterial {
                base_color: Color::srgb(0.1, 0.1, 0.1),
                perceptual_roughness: 0.9,
                ..default()
            });
            for (i, w) in vehicle_cfg.wheels.iter().enumerate() {
                let mount = commands
                    .spawn((
                        WheelMount { vehicle, index: i },
                        Transform::from_translation(Vec3::from(w.position)),
                    ))
                    .id();
                commands.entity(vehicle).add_child(mount);
                let spin = commands.spawn((WheelSpin, Transform::IDENTITY)).id();
                commands.entity(mount).add_child(spin);
                commands.entity(spin).with_child((
                    Mesh3d(wheel_mesh.clone()),
                    MeshMaterial3d(wheel_mat.clone()),
                    // Cylinder is Y-aligned: rotate onto the axle (X) and
                    // scale to the configured radius.
                    Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                        .with_scale(Vec3::new(w.radius / 0.34, 1.0, w.radius / 0.34)),
                ));
            }
        }
    }

    // World built and the player exists — release control. Event
    // sessions go through the countdown instead: the race resource and
    // the participant's progress are inserted first so `advance_race`
    // can own the release (one `RaceStarted`, one unlock — AC03).
    match event_race {
        Some(def) => {
            commands
                .entity(vehicle)
                .insert((RaceProgress::new(&def), TargetSelection::default()));
            race::spawn_checkpoint_markers(
                &mut commands,
                &mut assets.meshes,
                &mut assets.materials,
                &def,
                owner,
            );
            race::spawn_nav_arrow(&mut commands, owner);
            race::spawn_race_warning(&mut commands, owner);
            commands.insert_resource(RaceState::new(def, session.generation()));
            session
                .transition(SessionPhase::Countdown)
                .expect("Ready → Countdown is a legal transition");
        }
        None => {
            session
                .transition(SessionPhase::Playing)
                .expect("Ready → Playing is a legal transition");
        }
    }
}
