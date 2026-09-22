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
//!   `Esc` pauses a live `Playing` session (F17-B — the pause overlay's
//!   Quit/Resume rows take it from there), quits a
//!   `Countdown`/`Failed` one (tears down, then exits — or returns to
//!   the menu when a `MenuShell` resource is running, F17-A.1), and
//!   `Backspace` restarts the session with the same config. `Paused`
//!   and `Results` are absent: `pause_input`/`results_input` own the
//!   keyboard there (Esc is resume/continue).
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
    BangerPool, DEFAULT_ACTIVE_POOL, DamageSignals, DamageSpec, Mm2Vfs, ObjectIdentity, Player,
    PlayerControl, PlayerVehicle, RaceDefinition, RaceProgress, RaceState, Session, SessionEntity,
    SessionMode, SessionPhase, TargetSelection, VehicleDamage, WorldMode,
};
use mm2_vehicle::{TireConditions, VehicleConfig, vehicle_bundle};
use tracing::{error, info, warn};

use crate::camera::{CameraMode, ChaseCamera, FreeCamera};
use crate::car_visual::{self, WheelMount, WheelSpin};
use crate::contracts::ImpactFilter;
use crate::{city, dev_world, opponents, race};

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
/// [`session_control_input`] (and `pause_input`'s/`results_input`'s
/// row activations while `Paused`/`Results`), consumed by
/// [`drive_session`]. `quit` wins over `restart` and `pause` if several
/// are set in the same frame.
#[derive(Resource, Default)]
pub struct SessionControl {
    /// Tear down, reach `Menu`, then exit the app.
    pub quit: bool,
    /// Tear down, then `begin` a new session with the same config.
    pub restart: bool,
    /// `Playing → Paused` (F17-B). Only ever set for a live session
    /// whose authority allows pause — `session_control_input` falls
    /// back to `quit` when `allows_pause` is false (MP-6).
    pub pause: bool,
}

/// How the last session ended, carried across teardown so the menu can
/// say *why* it is back (F17-AC04's return leg): `drive_session`
/// records a `Failed` reason as the session tears down, `menu_watch`
/// puts it on the reopened shell's status line, and
/// `load_session_world` clears it when a new session starts loading —
/// a restart bypasses the menu, so a stale note must never outlive the
/// run it describes.
#[derive(Resource, Default)]
pub struct SessionNote {
    /// The failed session's reason, if it ended in `Failed`.
    pub failure: Option<String>,
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

/// `Esc` (or a gamepad `Start`) asks to pause, `Backspace` asks to
/// restart. Intents are only read from the phases a session can sit in
/// — while `Loading`, `Unloading` or `Menu` the driver is already
/// working and input is ignored. `Paused` and `Results` are
/// deliberately absent: `pause_input`/`results_input` own the keyboard
/// there (`Esc` is resume/continue, and the overlays' Quit/Restart
/// rows set these same intents). `Countdown` is quittable: a race that
/// has not started still tears down like any other live session, and
/// since the lifecycle has no `Countdown → Paused` edge, `Esc` there
/// stays quit.
///
/// Pause is only requested when the session authority allows it
/// (MP-6) — a non-pausable session takes `Esc` as quit, so the key
/// always escapes a live session rather than going dead.
pub fn session_control_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    session: Res<Session>,
    mut control: ResMut<SessionControl>,
) {
    let quittable = matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Failed(_)
    );
    if !quittable {
        return;
    }
    let pause_key = keys.just_pressed(KeyCode::Escape)
        || pads
            .iter()
            .next()
            .is_some_and(|pad| pad.just_pressed(GamepadButton::Start));
    if pause_key {
        if *session.phase() == SessionPhase::Playing
            && session.config().is_none_or(|c| c.authority.allows_pause())
        {
            control.pause = true;
        } else {
            control.quit = true;
        }
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
/// - `Menu`: `quit` exits the process — unless a `MenuShell` resource
///   exists, in which case the menu owns exit and a quit here just
///   returns to it; `restart` calls `begin` with the retained config,
///   flipping the phase to `Loading` so the spawn system builds the
///   next session.
/// - `Playing`: a queued `pause` intent (Esc/Start or `--pause`) moves
///   the session to `Paused` — the pause overlay's Resume row and
///   `pause_input`'s Esc bring it straight back.
/// - `Countdown`/`Playing`/`Paused`/`Results`/`Failed`: a queued
///   quit/restart intent moves the session to `Unloading`; teardown
///   proceeds on later frames.
// The menu-shell presence adds one param past the lint's limit — a
// SystemParam bundle would hide `session`/`control`, the two handles
// every arm uses, for no real gain.
#[allow(clippy::too_many_arguments)]
pub fn drive_session(
    mut commands: Commands,
    mut session: ResMut<Session>,
    mut control: ResMut<SessionControl>,
    mut filter: ResMut<ImpactFilter>,
    mut damage_report: ResMut<crate::damage::DamageReport>,
    mut spawn: ResMut<SpawnPoint>,
    menu: Option<Res<crate::menu::MenuShell>>,
    roots: Query<Entity, (With<SessionEntity>, Without<ChildOf>)>,
    mut note: Option<ResMut<SessionNote>>,
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
            damage_report.reset();
            spawn.trailers.clear();
            // Session-scoped resources die with the session: a race's
            // countdown/clock/progress, its reward/report view and the
            // city's nav overlay must never survive into the next
            // session (AC03 — no old timer survives).
            commands.remove_resource::<RaceState>();
            commands.remove_resource::<crate::progression::EventRewards>();
            commands.remove_resource::<crate::progression::SessionReport>();
            commands.remove_resource::<crate::nav_overlay::CityNav>();
            commands.remove_resource::<crate::traffic::AmbientTraffic>();
            commands.remove_resource::<mm2_content::SurfaceTables>();
            // `TireConditions` stays: it is a system input (the impact
            // filter and telemetry read `Res` every frame), and
            // `load_session_world` re-stamps it from the next session's
            // config — removing it only opens a panic window.
            session
                .transition(SessionPhase::Menu)
                .expect("Unloading → Menu is a legal transition");
        }
        SessionPhase::Menu => {
            if control.quit {
                control.quit = false;
                // With a menu shell running, quit-to-menu lands back on
                // the menu — the shell itself owns process exit (its
                // Quit row / Esc at the root). Without one, Menu is
                // terminal: quit exits.
                if menu.is_none() {
                    exit.write(AppExit::Success);
                }
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
        SessionPhase::Playing if control.pause && !(control.quit || control.restart) => {
            control.pause = false;
            // The intent is only ever produced for a pausable
            // authority, so a rejection means the session state
            // drifted — log and keep playing rather than stranding
            // the driver.
            if let Err(e) = session.transition(SessionPhase::Paused) {
                warn!(error = %e, "pause intent rejected");
            }
        }
        SessionPhase::Countdown
        | SessionPhase::Playing
        | SessionPhase::Paused
        | SessionPhase::Results
        | SessionPhase::Failed(_)
            if control.quit || control.restart =>
        {
            // A failed session's reason rides along to the menu —
            // AC04's return leg needs to say *why* it is back.
            if let SessionPhase::Failed(reason) = session.phase()
                && let Some(note) = note.as_mut()
            {
                note.failure = Some(reason.clone());
            }
            // A queued pause must not outlive the session it was meant
            // for — the next `Playing` phase belongs to a new run.
            control.pause = false;
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
    mut active_profile: Option<ResMut<crate::profile::ActiveProfile>>,
    mut note: Option<ResMut<SessionNote>>,
) {
    // A session loading retires the last session's end-note — a
    // restart bypasses the menu, so a stale failure must not surface
    // after a successful reload.
    if let Some(note) = note.as_mut() {
        note.failure = None;
    }
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
                    // The surface tables' index space is what
                    // `SurfaceMaterial::Authored` on the city colliders
                    // refers to — session-scoped like `CityNav`.
                    if let Some(tables) = loaded.surfaces {
                        commands.insert_resource(tables);
                    }
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
    let mut event_race: Option<(
        RaceDefinition,
        mm2_game::OpponentRoster,
        mm2_game::RewardTable,
        mm2_game::AvailabilityTable,
        Option<mm2_formats::aimap::Aimap>,
    )> = None;
    // The event's stable save identity — recorded on the bound profile
    // once the session is live (F16 `selections.last_event`).
    let mut event_key = None;
    if world_ok && let SessionMode::Event(event_ref) = &config.mode {
        match race::event_race_setup(&vfs.0, event_ref, config.difficulty) {
            Ok(setup) => {
                event_key = Some(setup.key);
                let def = setup.definition;
                // The player slot overrides the world's roam spawn; an
                // event without slots keeps the roam spawn.
                if let Some(slot) = def.start_slots.get(mm2_content::PLAYER_SLOT) {
                    spawn.position = slot.position;
                    // `RaceStart.yaw_deg` is already the vehicle-yaw
                    // convention — forward is (−sin a, −cos a) in XZ,
                    // exactly what `Quat::from_rotation_y` produces for
                    // local −Z forward. The authored `_strtpnts` `a`
                    // column measures this way (retail `cir1` ≈ +92°
                    // faces the −X course); it is *not* the waypoint
                    // `a` bearing — the two sit 180° apart (UNK-16).
                    // `None` means the record authored no heading
                    // (`cir6_strtpnts`' all-zero column): fall back to
                    // the course facing, not a verbatim −Z.
                    spawn.yaw = slot
                        .yaw_deg
                        .map(f32::to_radians)
                        .or_else(|| def.course_yaw(slot.position))
                        .unwrap_or(spawn.yaw);
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
                        bangers = overlay.stats.bangers,
                        pieces = overlay.stats.pieces,
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
                event_race = Some((
                    def,
                    setup.roster,
                    setup.rewards,
                    setup.availability,
                    setup.aimap,
                ));
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
    // A `--spawn` dev pose replaces whatever the world or authored
    // event slot chose — quarantined like `--cam`, never a session
    // parameter (evidence/diagnostic runs only).
    if let Some(pose) = config.dev.spawn {
        spawn.position = pose.position;
        spawn.yaw = pose.yaw;
    }
    // A `--banger-pool` dev bound replaces the recovered ×32 default —
    // quarantined like `--spawn`, session-scoped so a restart re-stamps
    // the same bound (evidence/diagnostic runs only).
    commands.insert_resource(BangerPool {
        max_active: config.dev.banger_pool.unwrap_or(DEFAULT_ACTIVE_POOL),
    });
    // The session's environment traction modifier (F06-B): `--traction`
    // is a quarantined dev override like `--banger-pool`; every real
    // session drives unmodified (`1.0`) until F18's weather work owns a
    // session-legal writer. Re-stamped on every load, so it survives
    // teardown without leaking a stale value.
    commands.insert_resource(TireConditions {
        traction: config.dev.traction.unwrap_or(1.0).max(0.0),
    });
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
            // Authored damage bounds — `vehcardamage` decodes to the
            // spec the impact pipeline accumulates against (F05-B.1).
            // A vehicle with no authored record stays undamageable
            // rather than borrowing a fabricated spec.
            if let Some(d) = &def.damage {
                commands
                    .entity(vehicle)
                    .insert(VehicleDamage::new(DamageSpec::from(d)));
            }
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

    // F10-A.2: ambient traffic — the event aimap's authored overrides
    // (roster replacement, `[Density]`, closed roads, speed limits)
    // layer over the city's aimap; the final spawn pose is the bubble
    // centre. Non-city worlds and predicted sessions get `None`.
    if world_ok {
        let (event_aimap, authored_density) = match &event_race {
            Some((def, _, _, _, aimap)) => (aimap.as_ref(), Some(def.params.densities.traffic)),
            None => (None, None),
        };
        if let Some(t) = crate::traffic::load_ambient_traffic(
            &mut commands,
            &vfs.0,
            &config,
            event_aimap,
            authored_density,
            owner,
            &mut session,
            // Load-time interest is the local spawn alone — every
            // participant stages on the same grid, and the runtime
            // maintainer rebuilds the union live each tick.
            std::slice::from_ref(&spawn.position),
            &mut assets.meshes,
            &mut assets.images,
            &mut assets.materials,
        ) {
            commands.insert_resource(t);
        }
    }

    // World built and the player exists — release control. Event
    // sessions go through the countdown instead: the race resource and
    // the participant's progress are inserted first so `advance_race`
    // can own the release (one `RaceStarted`, one unlock — AC03).
    match event_race {
        Some((def, roster, rewards, availability, _aimap)) => {
            commands
                .entity(vehicle)
                .insert((RaceProgress::new(&def), TargetSelection::default()));
            // F15-A.2: the authored opponent lineup spawns as real
            // participants — own vehicles, own routes, AI control.
            opponents::spawn_opponents(
                &mut commands,
                &vfs.0,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                &roster,
                &def,
                owner,
                &mut session,
                spawn.position,
                spawn.yaw,
            );
            race::spawn_checkpoint_markers(
                &mut commands,
                &mut assets.meshes,
                &mut assets.materials,
                &def,
                owner,
            );
            race::spawn_nav_arrow(&mut commands, owner);
            race::spawn_race_warning(&mut commands, owner);
            race::spawn_countdown_banner(&mut commands, owner);
            // F16-B: the event's reward + availability surface —
            // consumed by `record_session_results` while the session
            // lives, removed by teardown so a following cruise never
            // sees it.
            let key = event_key.clone().expect("an event setup carries its key");
            // A `--event` launch bypasses the menu that enforces
            // availability (F17-A.1) — surface a still-locked event
            // honestly rather than pretending the profile selected it.
            if let Some(profile) = &active_profile
                && let Some(entry) = availability.of(&profile.profile, &key)
                && !entry.unlocked
            {
                warn!(
                    event = %key.stem,
                    blocked_by = %entry
                        .blocked_by
                        .iter()
                        .map(|k| k.stem.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    "event not unlocked for the bound profile"
                );
            }
            commands.insert_resource(crate::progression::EventRewards {
                key,
                table: rewards,
                availability,
            });
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

    // F16-A: the session is live — record what it launched on the bound
    // profile and persist immediately, so a crash mid-session cannot
    // lose the selections. A failed load never reaches here, so nothing
    // records an event the player never entered.
    if let Some(profile) = &mut active_profile {
        crate::profile::note_session_start(profile, &selected, event_key.as_ref());
    }
}
