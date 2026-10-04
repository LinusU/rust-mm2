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
//!   `F4` restarts the session with the same config (the documented
//!   original binding, CTL-1 — `Backspace` is the F22-B.2 mirror).
//!   `Paused` and `Results` are absent: `pause_input`/`results_input`
//!   own the keyboard there (Esc is resume/continue).
//! - `despawn_session_entities` (mm2_game, scheduled while `Unloading`)
//!   removes every session-owned root; [`drive_session`] waits for the
//!   world to be observably empty, clears session-scoped caches
//!   ([`ImpactFilter`](crate::contracts::ImpactFilter), trailer spawn
//!   bookkeeping) and moves `Unloading → Menu`. At `Menu` a `restart`
//!   intent calls `Session::begin` again — which flips the phase back to
//!   `Loading` and re-runs [`load_session_world`] — while `quit` writes
//!   `AppExit`. The restart `begin` is `Local`-authority only: under a
//!   networked session the host's `Cancel`/`Start` owns restarts, so a
//!   queued intent is consumed at `Menu` without minting a generation.
//!
//! The cycle is `Playing → Unloading → Menu → Loading → Playing`; every
//! start goes through `Menu`, so session-owned entities are always cleaned
//! between runs (AC01) and a failed load can never leave a live player
//! simulation (AC02).

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_content::VehicleDef;
use mm2_game::{
    BangerPool, BreakPartSpec, DEFAULT_ACTIVE_POOL, DamageSignals, DamageSpec, Mm2Vfs,
    ObjectIdentity, Player, PlayerControl, PlayerVehicle, RaceDefinition, RaceProgress, RaceState,
    RecoveryPolicy, Session, SessionAuthority, SessionEntity, SessionMode, SessionPhase,
    SmokePolicy, SparkPolicy, StuckSpec, TargetSelection, VehicleAudio, VehicleBreaks,
    VehicleDamage, VehicleRecovery, VehicleSmoke, VehicleSparks, VehicleStuck, WorldMode,
};
use mm2_vehicle::{ResetAuthority, ResetVehicle, TireConditions, VehicleConfig, vehicle_bundle};
use tracing::{debug, error, info, warn};

use crate::camera::{CameraMode, ChaseCamera, FreeCamera};
use crate::car_visual;
use crate::contracts::ImpactFilter;
use crate::{city, dev_world, netdrive, opponents, race, scripted};

/// Where the player vehicle (re)spawns. `origin`/`origin_yaw` are the
/// pre-seat roam base — the world/authored pose before this
/// participant's grid seat applied (`position`/`yaw` carry the seated
/// pose; identical to the origin in a solo session). Network seat
/// resolution anchors on the origin, never on another participant's
/// slot (F25-A.2). `trailers` holds each spawned trailer's entity plus
/// its car-space rest offset so a reset can place it back behind the
/// car instead of on top of it. Session-scoped: teardown clears
/// `trailers`, and the next session's spawn rewrites the poses.
#[derive(Resource)]
pub struct SpawnPoint {
    pub position: Vec3,
    pub yaw: f32,
    pub origin: Vec3,
    pub origin_yaw: f32,
    pub trailers: Vec<(Entity, Vec3)>,
}

impl SpawnPoint {
    /// A spawn whose seated pose and roam base are the same point —
    /// the app's pre-load placeholder and every test fixture.
    pub fn new(position: Vec3, yaw: f32) -> Self {
        Self {
            position,
            yaw,
            origin: position,
            origin_yaw: yaw,
            trailers: Vec::new(),
        }
    }
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
    /// `Local`-authority sessions only — a networked session's
    /// restarts are the host's `Cancel`/`Start`, so `drive_session`
    /// consumes a non-`Local` restart intent at `Menu` without
    /// beginning anything.
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

/// `Esc` (or a gamepad `Start`) asks to pause, `F4` asks to restart —
/// the documented original binding (CTL-1); `Backspace` belongs to the
/// rear-view mirror now (F22-B.2). Intents are only read from the
/// phases a session can sit in
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
    // The stock restart binding is a single-player control: under a
    // networked session the wire owns restarts (the host's
    // `Cancel`/`Start` pair), so `F4` is local-authority only. A
    // future in-app host restart is a rematch intent, not a local
    // `begin`.
    if keys.just_pressed(KeyCode::F4)
        && session
            .config()
            .is_none_or(|c| c.authority == SessionAuthority::Local)
    {
        control.restart = true;
    }
}

/// `drive_session`'s view of what else could own "a `quit` intent
/// landed at `Menu`". A running [`MenuShell`](crate::menu::MenuShell)
/// owns exit (its Quit row / Esc at the root); a joined
/// [`LobbyLink`](crate::net::LobbyLink) or a hosted
/// [`HostLink`](crate::net::HostLink) means quit lands back in the
/// lobby and `net::drive_lobby`/`net::drive_host` writes the eventual
/// `AppExit`. Only with none of them is a `Menu` quit the process's
/// exit.
#[derive(bevy::ecs::system::SystemParam)]
pub struct MenuExit<'w> {
    menu: Option<Res<'w, crate::menu::MenuShell>>,
    lobby: Option<Res<'w, crate::net::LobbyLink>>,
    host: Option<Res<'w, crate::net::HostLink>>,
}

impl MenuExit<'_> {
    /// Whether quit-to-`Menu` stays inside the app.
    fn keeps_running(&self) -> bool {
        self.menu.is_some() || self.lobby.is_some() || self.host.is_some()
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
///   or a [`LobbyLink`](crate::net::LobbyLink)/
///   [`HostLink`](crate::net::HostLink) exists, in which case
///   that surface owns exit and a quit here just returns to it;
///   `restart` calls `begin` with the retained config, flipping the
///   phase to `Loading` so the spawn system builds the next session —
///   `Local` authority only, since a networked session's restarts are
///   the host's `Cancel`/`Start`, not a locally minted generation.
/// - `Playing`: a queued `pause` intent (Esc/Start or `--pause`) moves
///   the session to `Paused` — the pause overlay's Resume row and
///   `pause_input`'s Esc bring it straight back.
/// - `Countdown`/`Playing`/`Paused`/`Results`/`Failed`: a queued
///   quit/restart intent moves the session to `Unloading`; teardown
///   proceeds on later frames.
// The lifecycle genuinely threads every report/caches handle — a
// SystemParam bundle would hide `session`/`control`, the two handles
// every arm uses, for no real gain.
#[allow(clippy::too_many_arguments)]
pub fn drive_session(
    mut commands: Commands,
    mut session: ResMut<Session>,
    mut control: ResMut<SessionControl>,
    mut filter: ResMut<ImpactFilter>,
    mut damage_report: ResMut<crate::damage::DamageReport>,
    mut stuck_report: ResMut<crate::stuck::StuckReport>,
    mut break_report: ResMut<crate::breakaway::BreakReport>,
    mut recovery_report: ResMut<crate::recovery::RecoveryReport>,
    mut smoke_fx_report: ResMut<crate::damage_fx::SmokeFxReport>,
    mut spark_fx_report: ResMut<crate::spark_fx::SparkFxReport>,
    mut texel_report: ResMut<crate::texel_fx::TexelDamageReport>,
    mut spawn: ResMut<SpawnPoint>,
    menu_exit: MenuExit,
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
            stuck_report.reset();
            break_report.reset();
            recovery_report.reset();
            smoke_fx_report.reset();
            spark_fx_report.reset();
            texel_report.reset();
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
            commands.remove_resource::<crate::environment::EnvironmentReport>();
            commands.remove_resource::<crate::damage_fx::SmokeFx>();
            commands.remove_resource::<crate::spark_fx::SparkFx>();
            // F18-B.2: the precipitation rig and its render assets die
            // with the session too — the drops themselves are
            // `SessionEntity`-stamped and chain-despawn.
            commands.remove_resource::<crate::precip::PrecipFx>();
            commands.remove_resource::<mm2_game::Precipitation>();
            // F18-B.4: the wheel-effect table dies with the session —
            // the puffs themselves are `SessionEntity`-stamped and
            // chain-despawn; the report resets on unload like the
            // other evidence counters.
            commands.remove_resource::<crate::wheel_fx::WheelFx>();
            commands.remove_resource::<crate::audio::WaveBank>();
            commands.remove_resource::<crate::audio::ImpactAudio>();
            commands.remove_resource::<crate::audio::SurfaceAudio>();
            commands.remove_resource::<crate::audio::SirenAudio>();
            // F18-B.3: the precipitation ambience binding dies with
            // the session like the other audio state — its voices are
            // `SessionEntity`-stamped and chain-despawn.
            commands.remove_resource::<crate::audio::WeatherAudio>();
            // F18-B.5: the commentary binding dies with the session
            // like the ambience — its voices are `SessionEntity`-
            // stamped and chain-despawn.
            commands.remove_resource::<crate::audio::CommentaryAudio>();
            commands.remove_resource::<crate::pvs::CityPvs>();
            commands.remove_resource::<crate::water::CityWater>();
            commands.remove_resource::<crate::city::WorldFloor>();
            commands.remove_resource::<crate::underground::ListenerRooms>();
            // The HUD map pair dies with the session like `CityNav`
            // (F22-A.1) — the entities it serves are `SessionEntity`
            // stamped, so the restart that removes them also drops the
            // state/report.
            commands.remove_resource::<crate::hudmap::HudMapReport>();
            commands.remove_resource::<mm2_game::HudMap>();
            // Same for the opponent-indicator report (F22-A.2) — the
            // marker pool itself is `SessionEntity`-stamped.
            commands.remove_resource::<crate::oppind::OppIndReport>();
            // Same for the cockpit rig's spawn report (F22-B.1) — the
            // dash subtree itself is `SessionEntity`-stamped.
            commands.remove_resource::<crate::dash::DashReport>();
            // Same for the chase-rig lens report (F22-B.3).
            commands.remove_resource::<crate::camera::TrackReport>();
            // Same for the race timer's report (F22-A.4) — the row
            // itself is `SessionEntity`-stamped.
            commands.remove_resource::<crate::racetime::RaceTimerReport>();
            // Same for the nav arrow's report (F22-A.5) — the node
            // itself is `SessionEntity`-stamped.
            commands.remove_resource::<crate::navarrow::NavArrowReport>();
            // Same for the standings cluster's report (F22-A.6) — the
            // root itself is `SessionEntity`-stamped.
            commands.remove_resource::<crate::racestat::RaceStatReport>();
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
                // Quit row / Esc at the root). The same goes for a
                // joined lobby: quit-to-menu lands back in the lobby,
                // and `net::drive_lobby` owns the eventual AppExit.
                // Without either, Menu is terminal: quit exits.
                if !menu_exit.keeps_running() {
                    exit.write(AppExit::Success);
                }
            } else if control.restart {
                control.restart = false;
                match session.config().cloned() {
                    Some(config) if config.authority == SessionAuthority::Local => {
                        if let Err(e) = session.begin(config) {
                            // The retained config already validated once;
                            // a failure here means the session state is
                            // inconsistent — log and stay at Menu.
                            error!(error = %e, "session restart rejected");
                        }
                    }
                    // The wire owns a networked session's restarts —
                    // the host's `Cancel`/`Start` pair — the same
                    // `Local`-authority predicate the `F4` binding
                    // applies at intent time. Producers the binding
                    // does not cover (the results Restart row, a
                    // Blitz/Checkpoint `RestartEvent` disabled
                    // outcome, `--restart`) still queue the intent and
                    // teardown has already landed the client at `Menu`;
                    // a local `begin` here would mint a generation the
                    // lobby never issued and diverge the prediction
                    // from lobby authority until the next wire message.
                    Some(config) => {
                        debug!(
                            authority = ?config.authority,
                            "restart intent consumed without a begin — the wire owns this session's restarts"
                        );
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

/// `--restart` (quarantined `DevOverrides`, evidence runs only): queue
/// the session's own restart intent on the first `Playing` frame. The
/// restart then travels the production lifecycle — `drive_session`
/// takes it `Playing → Unloading → Menu → begin` — so a headless
/// `--frames` run exercises the same teardown/rebuild a
/// disabled-in-Blitz/Checkpoint restart or a Backspace restart takes.
/// One-shot: a session started by the restart stays running.
pub fn dev_restart_once(
    session: Res<Session>,
    mut control: ResMut<SessionControl>,
    mut fired: Local<bool>,
) {
    if *fired {
        return;
    }
    if session.is_playing() && session.config().is_some_and(|c| c.dev.restart) {
        *fired = true;
        control.restart = true;
    }
}

/// `--restart-at TICK` (quarantined `DevOverrides`, evidence runs
/// only): the delayed form of [`dev_restart_once`] — queues the
/// session's restart intent on the first `Playing` frame where the
/// session clock has reached `restart_at` fixed ticks (120 Hz, the
/// `smoke` record's `ticks=` unit). The deferral is the point: an
/// event run at tick 0 has nothing banked, so `--restart` can only
/// prove the teardown/rebuild mechanics, not that a *mid-race*
/// restart resets progress. This override lets a leg bank real gates,
/// laps and race ticks first, then takes the same production
/// `Unloading → Menu → begin` path. One-shot: generation 2's clock
/// would reach the threshold too, so the latch keeps it running.
pub fn dev_restart_at(
    session: Res<Session>,
    mut control: ResMut<SessionControl>,
    mut fired: Local<bool>,
) {
    if *fired {
        return;
    }
    let at = session.config().and_then(|c| c.dev.restart_at);
    if session.is_playing() && at.is_some_and(|at| session.tick() >= at) {
        *fired = true;
        control.restart = true;
    }
}

/// The `R`-key reset bundle: teleport the player vehicle to the spawn
/// point — the same [`ResetVehicle`] path the water/stuck/disabled
/// recoveries and scripted re-anchors take (`Teleported` breaks race
/// segments, and `camera::chase_follow` snaps the boom on the jump).
/// Shared by `reset_input` and [`dev_reset_at`] so the key and the
/// scheduled flag emit identical messages. Trailers reseat through
/// [`reseat_towed_trailers`], the stream follower every tractor reset
/// picks up — the bundle no longer has to name them itself.
pub fn spawn_resets(spawn: &SpawnPoint, player: Option<Entity>) -> Vec<ResetVehicle> {
    vec![ResetVehicle {
        entity: player,
        position: spawn.position,
        yaw: spawn.yaw,
    }]
}

/// Follower on the [`ResetVehicle`] stream (F25-B): a tractor's reset
/// reseats every trailer towing it — `Trailer::rest_offset` rotated into
/// the reset pose's frame — so the rig leaves `vehicle_reset` as one
/// landed teleport. This is the generalized form of the per-caller
/// `SpawnPoint.trailers` loops: it covers every writer the stream sees
/// (the `R` bundle, `--reset-at`, recovery/stuck/disabled resolves,
/// scripted and opponent re-anchors, the self-right assist, wire
/// `ResetRequest`s) and every tractor — a trailer's owner is the
/// `Trailer` relation, not the local `PlayerVehicle`, so a remote
/// participant's rig reseats on the authority exactly like the local
/// one.
///
/// Scheduled after every Update-side writer and before
/// `vehicle_reset`, whose message cursor then reads the trailer rows in
/// the same pass. The emitted rows target trailer entities, which tow
/// nothing, so the follower cannot re-trigger itself; trailers carry no
/// `ResetEpoch` — on the wire they ride their owner's epoch
/// (`Message::Snap::trailers` keys rows by the seat's wire id).
pub fn reseat_towed_trailers(
    // A mutator, not reader+writer: the follower reads and extends the
    // same stream, and separate `MessageReader`/`MessageWriter` params
    // of one message type conflict over `Messages<ResetVehicle>`.
    mut resets: MessageMutator<ResetVehicle>,
    trailers: Query<(Entity, &car_visual::Trailer)>,
) {
    // Collect before writing — the emitted trailer rows land on the
    // same stream, and reading a fresh batch in the same pass would
    // re-observe them a frame early (harmless, a trailer tows nothing,
    // but wasteful).
    let msgs: Vec<ResetVehicle> = resets.read().map(|m| *m).collect();
    for msg in msgs {
        // `entity: None` resets every vehicle — trailers included —
        // directly onto `msg.position`; a tractor-relative reseat would
        // only write a different pose over the same row.
        let Some(towing) = msg.entity else {
            continue;
        };
        let rot = Quat::from_rotation_y(msg.yaw);
        for (entity, trailer) in &trailers {
            if trailer.towing == towing {
                resets.write(ResetVehicle {
                    entity: Some(entity),
                    position: msg.position + rot * trailer.rest_offset,
                    yaw: msg.yaw,
                });
            }
        }
    }
}

/// `--reset-at TICK` (quarantined `DevOverrides`, evidence runs
/// only): emit the `R`-key reset bundle once the session clock
/// reaches `reset_at` fixed ticks (120 Hz, the `smoke` record's
/// `ticks=` unit). A `--frames`/`--screenshot` capture freezes live
/// input, so this is how a mid-run `ResetVehicle` teleport gets
/// exercised on real content — the reset-transition camera leg.
/// One-shot; unlike the `R` key itself the scheduled teleport is a
/// dev-timed intervention, so `record_eligibility` names `reset-at`.
/// Authority-gated like the `R` key: the flag reaches a `Remote`
/// session — `--reset-at` does not conflict with `--join`, and
/// `net::start` stamps the client's `dev` flags onto the accepted
/// config — where the scheduled teleport would be the same
/// unannounced self-teleport the key's gate forbids (a pose the
/// host's copy can never learn, with the local `ResetEpoch` never
/// bumped), so the writer stays inert there.
pub fn dev_reset_at(
    session: Res<Session>,
    spawn: Res<SpawnPoint>,
    player: Query<Entity, With<PlayerVehicle>>,
    mut resets: MessageWriter<ResetVehicle>,
    mut fired: Local<bool>,
) {
    if *fired || !session.authority_role().is_authority() {
        return;
    }
    let at = session.config().and_then(|c| c.dev.reset_at);
    if session.is_playing() && at.is_some_and(|at| session.tick() >= at) {
        *fired = true;
        for msg in spawn_resets(&spawn, player.iter().next()) {
            resets.write(msg);
        }
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
    seats: netdrive::NetSeats,
    mut active_profile: Option<ResMut<crate::profile::ActiveProfile>>,
    mut note: Option<ResMut<SessionNote>>,
    scripted_drive: Option<Res<scripted::ScriptedDrive>>,
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
    // Validate before spawning anything: direct SessionConfig callers and a
    // changed-on-disk guide get the same strict failure as the CLI parser.
    let explicit_bot_route = match config.dev.bot_route.as_deref() {
        Some(path) => {
            let loaded = if scripted_drive.is_none() {
                Err("bot route requires the scripted driver".to_string())
            } else if matches!(config.mode, SessionMode::Event(_)) {
                scripted::load_bot_route(path)
            } else {
                Err("bot route requires an event session".to_string())
            };
            match loaded {
                Ok(route) => Some(route),
                Err(reason) => {
                    error!(%reason, "bot route failed to load");
                    session.fail(reason).expect("Loading may fail");
                    return;
                }
            }
        }
        None => None,
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
            // The `materials.{mtl,csv}` pair is global — not city-scoped
            // — so a dev world mounts it too: every dev-world collider
            // is `SurfaceMaterial::Unspecified`, which resolves the
            // `_default` block's `sound` class / `ptx` channels through
            // the same inheritance an unmarked city collider applies.
            // Same failure policy as the city loader: a broken pair
            // warns and stays unmounted rather than substituting.
            match mm2_content::load_surface_tables(&vfs.0) {
                Ok(Some(tables)) => {
                    commands.insert_resource(tables);
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(error = %e, "surface tables failed; dev world colliders stay Unspecified");
                }
            }
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
                    // F18-A.5: the authored room-PVS table is
                    // session-scoped like `CityNav`; `--no-pvs`
                    // (`config.dev.no_pvs`) maps to retail's
                    // `EnablePVS(false)` (default enabled). Carrying the
                    // flag in the config is what lets the headless
                    // smoke's own app see it.
                    if let Some(mut pvs) = loaded.pvs {
                        pvs.enabled = !config.dev.no_pvs;
                        commands.insert_resource(pvs);
                    }
                    // F18-A.6: the authored deadly-water record is
                    // session-scoped like `CityPvs` — dev worlds and
                    // cities without a usable `.water` get no
                    // resource, so `track_recovery` falls back to the
                    // wheel-`drag` classification alone.
                    if let Some(water) = loaded.water {
                        commands.insert_resource(water);
                    }
                    // Whether the camera stands underground — the
                    // object sounds' `audible area` gate.
                    commands.insert_resource(loaded.listener_rooms);
                    // The authored world floor (`Psdl::bounds_min.y`)
                    // is session-scoped the same way — the smoke
                    // runner's below-world verdict reads it; sessions
                    // without a bound keep the spawn-relative line.
                    if let Some(floor) = loaded.floor {
                        commands.insert_resource(floor);
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
        }
    }
    // The roam base the world authored — kept as the seat map's anchor
    // while `position`/`yaw` take this participant's grid seat below.
    spawn.origin = spawn.position;
    spawn.origin_yaw = spawn.yaw;
    // Event mode: resolve the catalog event into the shared race
    // definition before the session is declared Ready. An event that
    // cannot load fails the session — it never silently cruises. The
    // authored grid seats every participant below.
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
                let mut def = setup.definition;
                let mut roster = setup.roster;
                // RACE-3's Circuit parenthetical (UI-2): the player's
                // laps + opponents picks rewrite the built setup
                // before anything consumes it — grid slots, the
                // minimap's opponent pool and the HUD laps readout
                // all follow the effective counts. A session whose
                // picks equal the authored row never sets
                // `customization` at all (DRV-6).
                if let Some(picks) = config.customization.and_then(|c| c.race) {
                    let applied = mm2_game::apply_race_picks(&mut def, &mut roster, picks);
                    info!(
                        laps = ?applied.laps,
                        opponents = applied.opponents,
                        opponents_clamped = applied.opponents_clamped,
                        "race-shape picks applied"
                    );
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
                event_race = Some((def, roster, setup.rewards, setup.availability, setup.aimap));
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
    // Drawbridge leaves (Tower Bridge, Waterloo, SF's Chinatown gate):
    // the PSDL suppresses the road under them, so they are part of
    // the drivable world. The event's own bridge file wins over the
    // city default, which is why this waits for the event setup.
    if world_ok && let WorldMode::City { psdl } = &config.world {
        let city_stem = std::path::Path::new(psdl)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let report = crate::drawbridge::spawn_drawbridges(
            &mut commands,
            &vfs.0,
            &city_stem,
            event_key.as_ref().map(|k| k.stem.as_str()),
            &mut assets.meshes,
            &mut assets.images,
            &mut assets.materials,
            owner,
        );
        commands.insert_resource(report);
        // Kerbside parked cars, from the same per-event/default file
        // pair. The original skips them in networked cruise and cops
        // & robbers; networked races keep them.
        let networked_roam = config.authority != mm2_game::SessionAuthority::Local
            && !matches!(config.mode, SessionMode::Event(_));
        let parked = if networked_roam {
            city::ParkedCarReport::default()
        } else {
            city::spawn_parked_cars(
                &mut commands,
                &vfs.0,
                &city_stem,
                event_key.as_ref().map(|k| k.stem.as_str()),
                config.seed,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                owner,
                &mut session,
            )
        };
        commands.insert_resource(parked);
        // Tugs, water taxis, ducks, ferries and the Underground — the
        // original's moving-object managers, same file lookup.
        let movers = crate::movers::spawn_movers(
            &mut commands,
            &vfs.0,
            &city_stem,
            event_key.as_ref().map(|k| k.stem.as_str()),
            config.authority != mm2_game::SessionAuthority::Local,
            config.seed,
            &mut assets.meshes,
            &mut assets.images,
            &mut assets.materials,
            owner,
        );
        commands.insert_resource(movers);
        // The city's ambience tables — the river, the Tube stations,
        // the bay — from its ambience container.
        let ambience =
            crate::object_sound::spawn_ambience(&mut commands, &vfs.0, &city_stem, owner);
        info!(city = %city_stem, tables = ambience, "ambience emitters");
    }
    // F25-A.2: the lobby's humans share the event's authored grid —
    // the local participant's seat resolves through the same
    // `seat_pose` the remote reconcile applies (`start_slots[seat]`,
    // authored `yaw_deg` or the derived course facing — `a` is the
    // vehicle-yaw convention, `None`/`a = 0` is no heading, WPT-4).
    // Solo — no link — seats 0, the slot the player took before; a
    // seat past the authored grid keeps fanning off the last row's
    // right vector, and a session with no grid at all fans off the
    // roam `origin` (designed; the original's row→participant mapping
    // is UNK-17).
    netdrive::apply_seat(
        &mut spawn,
        event_race.as_ref().map(|(def, ..)| def),
        seats.self_seat(),
    );
    // The session's effective weather/time-of-day — resolved once so
    // the environment preset (F18-A.2) and the surface-audio variant
    // (F07-B.8) read the same pick: player customization > authored
    // event params > the configured fallback (RACE-2/3/4, the
    // precedence `mm2_game::effective_conditions` encodes).
    let session_conditions =
        mm2_game::effective_conditions(&config, event_race.as_ref().map(|(def, ..)| &def.params));
    // F18-A.2: the session's environment lighting. A preset that
    // cannot load spawns the fallback rig and reports it — an
    // explicit diagnostic, never a silent default (F18-AC06).
    // F18-A.3: the same call resolves the authored fog row; its
    // `DistanceFog` is attached to the session's cameras below.
    let mut camera_fog: Option<bevy::pbr::DistanceFog> = None;
    if world_ok && let WorldMode::City { psdl } = &config.world {
        let conditions = session_conditions;
        let source = if config.customization.is_some() {
            crate::environment::ConditionsSource::Customized
        } else if event_race.is_some() {
            crate::environment::ConditionsSource::Authored
        } else {
            crate::environment::ConditionsSource::Configured
        };
        let mut report = crate::environment::spawn_environment(
            &mut commands,
            &vfs.0,
            psdl,
            conditions,
            source,
            owner,
        );
        // F18-A.4: the `.sky` dome binds at the same effective slot —
        // authored-event and customization precedence already resolved
        // into `report.slot`.
        report.sky = crate::environment::spawn_sky_dome(
            &mut commands,
            &vfs.0,
            psdl,
            report.slot,
            &mut assets.meshes,
            &mut assets.images,
            &mut assets.materials,
            owner,
        );
        camera_fog = report.fog.bound.map(|f| f.distance_fog());
        commands.insert_resource(report);
    }
    // F22-A.1: the authored HUD minimap (HUD-4) — city sessions only,
    // session-scoped like `CityPvs`. The tune spec, world-space tiles
    // and marker set bind together; a city without them records
    // `absent` on the report rather than drawing a substitute.
    if world_ok && let WorldMode::City { psdl } = &config.world {
        let (report, map) = crate::hudmap::spawn_hud_map(
            &mut commands,
            &vfs.0,
            psdl,
            event_race.as_ref().map(|(def, ..)| def),
            event_race
                .as_ref()
                .map(|(_, roster, ..)| roster.entries.len())
                .unwrap_or(0),
            &mut assets.meshes,
            &mut assets.images,
            &mut assets.materials,
            owner,
            session.generation(),
        );
        commands.insert_resource(report);
        if let Some(map) = map {
            commands.insert_resource(map);
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
    // The session's environment traction modifier (F06-B, F18-B.1):
    // the effective weather selector's designed wetness factor —
    // `rainy` scales every tire contact, symmetric across player,
    // opponents and trailers on the shared tire path — resolved from
    // the same `effective_conditions` pick the lighting/fog and the
    // wet surface-audio table read. `--traction` stays a quarantined
    // dev pin *over* the weather factor for evidence runs (a `1.0`
    // pin dries a rainy session). Re-stamped on every load, so it
    // survives teardown without leaking a stale value.
    commands.insert_resource(TireConditions {
        traction: config
            .dev
            .traction
            .unwrap_or_else(|| session_conditions.weather.traction_factor())
            .max(0.0),
    });
    // F25-A.7: whether this process may originate a `ResetVehicle`
    // itself. `Local`/`Host` sessions resolve their own teleports;
    // under a `Remote` session every teleport is the wire's declared
    // epoch, so the self-right assist stays inert there like the
    // app-side writers already are (`reset_input`, the scripted and
    // opponent re-anchors). Re-stamped on every load like
    // `TireConditions` — a networked session cannot leak its gate into
    // the next local one.
    commands.insert_resource(ResetAuthority(config.authority.is_authoritative()));
    // F05-B.6: the smoke sprite assets — resolved through the VFS
    // like every texture; a missing `fxpt8` warns and emits
    // untextured puffs, never sinks the session. Session-scoped:
    // teardown removes it and the next load re-resolves.
    commands.insert_resource(crate::damage_fx::SmokeFx {
        assets: crate::damage_fx::smoke_assets(
            &vfs.0,
            &mut assets.meshes,
            &mut assets.images,
            &mut assets.materials,
            SmokePolicy::default().atlas_tiles,
        ),
    });
    // F05-B.8: the spark streak assets — `spark.tga` through the same
    // VFS path; a missing texture warns and emits untextured streaks.
    commands.insert_resource(crate::spark_fx::SparkFx {
        assets: crate::spark_fx::spark_assets(
            &vfs.0,
            &mut assets.meshes,
            &mut assets.images,
            &mut assets.materials,
        ),
    });
    // F18-B.2: authored precipitation — the same effective weather
    // pick the lighting/fog/traction legs read binds the
    // `tune/<name>.asbirthrule` record through the VFS (designed
    // selector→rule binding, DSN-60). A selector naming nothing —
    // every non-rainy session — returns no fx/rig; a named record
    // that cannot resolve or parse marks the report's `absent`
    // instead of silently degrading to dry (F18-AC06). The rig is
    // seeded from the session config so the emission stream replays
    // identically on restart (F18 req 5).
    let (precip_report, precip_fx, precip_rig) = crate::precip::precip_session(
        &vfs.0,
        session_conditions.weather,
        config.seed,
        &mut assets.meshes,
        &mut assets.images,
        &mut assets.materials,
    );
    commands.insert_resource(precip_report);
    if let Some(fx) = precip_fx {
        commands.insert_resource(fx);
    }
    if let Some(rig) = precip_rig {
        commands.insert_resource(rig);
    }
    // F18-B.4: authored wheel surface particles — `materials.mtl`'s
    // `ptxindex`/`ptxthreshold` channels select `tune/effects/<name>
    // .asbirthrule` specs (the index space is the exe's `ptx_wheel`
    // string table) drawn onto `texture/ptx_wheel`. The rules and the
    // atlas resolve through the VFS now so a missing record counts
    // `failed` once at bind time rather than per frame (F18-AC06);
    // slots whose index loads nothing stay dark — never substituted.
    let (wheel_fx_report, wheel_fx) = crate::wheel_fx::wheel_fx_session(
        &vfs.0,
        &mut assets.meshes,
        &mut assets.images,
        &mut assets.materials,
    );
    commands.insert_resource(wheel_fx_report);
    commands.insert_resource(wheel_fx);
    // F07-A.2: the session's wave stem index — cardata sample names
    // resolve through it into decoded `PcmAudio` voices. Session-scoped
    // like the effect banks: teardown removes it and the next load
    // re-indexes, so a mod set that changed between sessions can never
    // leave a stale stem map.
    commands.insert_resource(crate::audio::WaveBank::index(&vfs.0));
    // F07-B.3: the authored impact table — the player-side
    // `default_impacts.csv` the deduplicated impact stream picks
    // through. Same absence policy as every authored record: a table
    // that does not resolve or parse yields no resource, not a
    // fabricated category.
    // The seed is the wire generation — the authority's minted value —
    // so a replicated `RemoteImpact` picks the same authored variant on
    // every process the way the wire-seeded smoke/spark/texel rigs do
    // (the local id counter can diverge across processes).
    if let Some(table) = crate::audio::ImpactAudio::load(&vfs.0, session.wire_generation()) {
        commands.insert_resource(table);
    }
    // F07-B.4/B.8: the authored surface table — the player-side
    // `default_surface<variant>.csv` the wheel-contact picks read
    // through, the variant bound off the session's effective weather
    // (designed, DSN-43) with the exe's `%s_` per-vehicle probe ahead
    // of the shared default (AUD-11). Same absence policy: a probe
    // chain that resolves nothing yields no resource, not a
    // fabricated surface row.
    if let Some(table) = crate::audio::SurfaceAudio::load(
        &vfs.0,
        session_conditions.weather,
        config.vehicle.id.as_deref(),
    ) {
        commands.insert_resource(table);
    }
    // F07-B.7: the authored siren programs — the player side keys off
    // the session city's PSDL stem (`london` → `londonpolicesiren.csv`,
    // the naming the exe's hardcoded pair implies), the opponent side
    // is the shared `policesiren.csv`. A dev world or a mod city with
    // no authored program yields no resource — flagged cars then count
    // their presses as failed rather than sounding a substitute.
    let siren_city = match &config.world {
        WorldMode::City { psdl } => Some(
            std::path::Path::new(psdl)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(psdl.as_str()),
        ),
        WorldMode::DevWorld => None,
    };
    if let Some(programs) =
        crate::audio::SirenAudio::load(&vfs.0, session.wire_generation(), siren_city)
    {
        commands.insert_resource(programs);
    }
    // F18-B.3: the session's precipitation ambience — the same
    // `effective_conditions` pick the particle rig and the wet
    // surface table read names the `<name>exterior`/`<name>interior`
    // bed stems (exe-verified `Rainexterior`/`Raininterior`/
    // `Thunder`); a dry selector binds nothing. `weather_voices`
    // resolves the stems lazily through the WaveBank above — the
    // resource carries the binding and the seeded clap schedule.
    if let Some(ambience) =
        crate::audio::WeatherAudio::bind(session_conditions.weather, config.seed)
    {
        commands.insert_resource(ambience);
    }
    // F18-B.5: the session's environmental pre-race commentary — the
    // same effective-conditions pick names the `WEATHER`/`TIMEOFDAY`
    // `<stem>_prerace` cue tables the city's `spchdata` registry
    // scopes to a seeded speaker draw. `commentary_voices` resolves
    // the chain lazily inside the pre-race window; a dev world binds
    // nothing.
    if let Some(commentary) =
        crate::audio::CommentaryAudio::bind(siren_city, session_conditions, config.seed)
    {
        commands.insert_resource(commentary);
    }
    if world_ok {
        session
            .transition(SessionPhase::Ready)
            .expect("Loading → Ready is a legal transition");
    }

    crate::speedometer::spawn_speedometer(&mut commands, &mut assets.images, owner);

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

    // Resolve the effective camera mode before the session cameras
    // spawn: `CameraMode::Cockpit` exists only while an authored
    // `camPovCS` binds and `CameraMode::ChaseFar` only while an
    // authored `_far.camtrackcs` binds. Without the record — a
    // dashless/dev car, or a mode persisted across a session reload —
    // no camera would ever activate (`--cockpit` or a menu-phase `C`
    // press left the mode pointing at nothing: zero active cameras,
    // dead render), so the mode falls back to Chase *here*.
    let pov = selected
        .def
        .as_ref()
        .and_then(|def| crate::dash::load_pov_cam(&vfs.0, &def.id));
    let tracks = selected
        .def
        .as_ref()
        .map(|def| crate::camera::load_track_cams(&vfs.0, &def.id));

    // Cameras. The chase rig binds the authored `camTrackCS` lenses
    // when the records exist — near/far are per-vehicle authored views
    // (HUD-3) — and falls back to a boom sized to the chassis so a
    // city bus and a roadster are both framed sensibly.
    let chase = match &selected.def {
        Some(def) => {
            let [_w, h, d] = def.config.chassis_size;
            let near = tracks
                .as_ref()
                .and_then(|t| t.near.as_ref())
                .map(crate::camera::ChaseLens::authored)
                .unwrap_or_else(|| crate::camera::ChaseLens::sized(h, d));
            let far = tracks
                .as_ref()
                .and_then(|t| t.far.as_ref())
                .map(crate::camera::ChaseLens::authored);
            ChaseCamera {
                near,
                far,
                ..default()
            }
        }
        None => ChaseCamera::default(),
    };
    if selected.def.is_some() {
        // F22-B.3 evidence (`trk=` on the smoke record): which lenses
        // bound authored records vs the designed fallback.
        commands.insert_resource(crate::camera::TrackReport {
            near_authored: tracks.as_ref().is_some_and(|t| t.near.is_some()),
            far_authored: tracks.as_ref().is_some_and(|t| t.far.is_some()),
        });
    }
    let resolved = match *cam_mode {
        CameraMode::Cockpit if pov.is_none() => CameraMode::Chase,
        CameraMode::ChaseFar if chase.far.is_none() => CameraMode::Chase,
        m => m,
    };
    if resolved != *cam_mode {
        commands.insert_resource(resolved);
    }
    let cam_mode = resolved;
    let mut chase_cam = commands.spawn((
        owner,
        Camera3d::default(),
        Camera {
            is_active: matches!(cam_mode, CameraMode::Chase | CameraMode::ChaseFar),
            ..default()
        },
        // The active lens authors the projection — a far-view reload
        // starts on the far FOV rather than waiting a frame.
        Projection::Perspective(chase.lens(cam_mode).projection()),
        chase,
        Transform::from_translation(spawn.position + Vec3::new(0.0, 4.0, 9.0)),
    ));
    if let Some(fog) = camera_fog.clone() {
        chase_cam.insert(fog);
    }
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
    let mut free_cam_ent = commands.spawn((
        owner,
        Camera3d::default(),
        Camera {
            is_active: cam_mode == CameraMode::Free,
            ..default()
        },
        free_cam,
        free_xf,
    ));
    if let Some(fog) = camera_fog.clone() {
        free_cam_ent.insert(fog);
    }

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

    // Water/out-of-bounds recovery (F05-B.5): no authored record gates
    // it — the designed policy rides on every player vehicle, anchored
    // at its spawn pose so the fall leg has a landing before the first
    // dry contact is observed.
    commands
        .entity(vehicle)
        .insert(VehicleRecovery::with_anchor(
            RecoveryPolicy::default(),
            spawn.position,
            spawn.yaw,
        ));

    // F22-B.2: every player vehicle carries the rear-view mirror strip
    // camera (HUD-3/CTL-1 `BACKSPACE`) — presentation, not authored
    // content, so no record gates it. The eye rides at the authored
    // `camPovCS` seat position when the car has one; the fallback is a
    // designed seat height off the chassis size.
    crate::camera::spawn_mirror(
        &mut commands,
        pov.as_ref(),
        selected
            .def
            .as_ref()
            .map(|d| Vec3::new(0.0, d.config.chassis_size[1] * 0.55, -0.3))
            .unwrap_or(Vec3::new(0.0, 1.2, -0.3)),
        vehicle,
        owner,
        camera_fog.clone(),
    );

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
                // F05-B.6: the authored engine-smoke rig rides with
                // the damage spec — authored pivots + particle spec,
                // designed emission policy (DSN-24). Seeded from the
                // object id so the emission stream replays
                // identically per spawn.
                commands.entity(vehicle).insert(VehicleSmoke::new(
                    d,
                    SmokePolicy::default(),
                    (vehicle_object.generation << 32) | vehicle_object.slot as u64,
                ));
                // F05-B.8: the authored damage record also owns the
                // impact-spark renderer (`asLineSparks`) — same seed
                // domain, same authored-presence gate (DSN-26).
                commands.entity(vehicle).insert(VehicleSparks::new(
                    SparkPolicy::default(),
                    (vehicle_object.generation << 32) | vehicle_object.slot as u64,
                ));
            }
            // Authored stuck thresholds — `vehstuck` decodes to the
            // spec the impact-armed detector runs against (F05-B.2).
            // Same absence policy as damage: no record, no component.
            if let Some(s) = &def.stuck {
                commands
                    .entity(vehicle)
                    .insert(VehicleStuck::new(StuckSpec::from(s)));
            }
            // Authored audio bindings (F07-A.2): the cardata table
            // verbatim — `horn_voices` resolves its stems through the
            // session's `WaveBank`. Same absence policy as damage: no
            // record, no component.
            if let Some(a) = &def.audio {
                commands
                    .entity(vehicle)
                    .insert(VehicleAudio { spec: a.clone() });
            }
            // Authored breakaway inventory (F05-B.3): only
            // `dgbangerdata`-backed BREAK chunks make a rig, so a car
            // without authored fragments detaches nothing.
            if !def.breaks.is_empty() {
                commands.entity(vehicle).insert(VehicleBreaks::new(
                    def.breaks
                        .iter()
                        .map(|b| BreakPartSpec {
                            name: b.name.clone(),
                            def: b.def.clone(),
                        })
                        .collect(),
                ));
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
                // F05-B.9: the authored record also gates the texel
                // rig — same seed domain as smoke/sparks.
                def.damage.as_ref().map(|d| {
                    (
                        d,
                        (vehicle_object.generation << 32) | vehicle_object.slot as u64,
                    )
                }),
            );
            if !missing.is_empty() {
                warn!(car = %def.id, "missing textures: {}", missing.join(", "));
            }
            // F22-B.1: the authored cockpit rig — `_dash.pkg` interior
            // geometry, `_dash.asnode` gauge calibration and the
            // `camPovCS` eye position. Each record gates independently;
            // a car without them simply has no cockpit (HUD-1/HUD-3).
            let dash_report = crate::dash::spawn_dash(
                &mut commands,
                &vfs.0,
                &def.id,
                selected.paint,
                pov,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                vehicle,
                owner,
                cam_mode,
                camera_fog,
            );
            commands.insert_resource(dash_report);
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
            car_visual::spawn_dev_car(
                &mut commands,
                vehicle_cfg,
                &mut assets.meshes,
                &mut assets.materials,
                vehicle,
            );
        }
    }

    // F10-A.2: ambient traffic — the event aimap's authored overrides
    // (roster replacement, `[Density]`, closed roads, speed limits)
    // layer over the city's aimap; the final spawn pose is the bubble
    // centre. Non-city worlds get `None`, and networked sessions get
    // `None` under MP-4 (documented) — `load_ambient_traffic` owns the
    // authority gate.
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
            // F15-B.6: the road graph re-paths sparse `.opp` legs that
            // leave the street corridor — `.opp` anchors are course
            // intent, and the retail straight line is not the driven
            // line (sf circuit:0's p3→p4 crosses Telegraph Hill
            // rooftops where the drivable course is the L-shaped
            // street around the block). A failed/absent graph leaves
            // every route verbatim rather than sinking the session.
            let nav = match &config.world {
                WorldMode::City { psdl } => {
                    let stem = std::path::Path::new(psdl)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(psdl.as_str());
                    match mm2_content::load_nav_graph(&vfs.0, stem) {
                        Ok(b) => Some(b.graph),
                        Err(e) => {
                            warn!(
                                error = %e,
                                "nav graph failed to load — opponent routes stay verbatim"
                            );
                            None
                        }
                    }
                }
                WorldMode::DevWorld => None,
            };
            let nav = nav.as_ref();
            commands
                .entity(vehicle)
                .insert((RaceProgress::new(&def), TargetSelection::default()));
            // F15-B.5: hand the scripted evidence bot the authored
            // explicit diagnostic guide, or the `.opp` driving line
            // staged nearest its slot — its aim
            // between gates then follows the course's own road geometry
            // instead of a straight line that can leave elevated or
            // depressed roads. Dormant unless `--bot` drives the car;
            // session-owned like `RaceProgress` beside it.
            let bot_route = explicit_bot_route
                .clone()
                .or_else(|| scripted::pick_bot_route(&roster, spawn.position));
            if let Some(route) = bot_route {
                commands
                    .entity(vehicle)
                    .insert(scripted::ScriptedRoute::new(
                        if explicit_bot_route.is_some() {
                            route
                        } else {
                            opponents::driving_route(&route, nav, &def.checkpoints)
                        },
                        spawn.position,
                        spawn.yaw,
                    ));
            }
            // F15-A.2: the authored opponent lineup spawns as real
            // participants — own vehicles, own routes, AI control.
            // MP-4 (documented): a networked race fields none of them —
            // the lobby's humans replace the roster. A remote-side
            // spawn would be worse than absent anyway: nothing
            // replicates opponents yet, so each process would diverge
            // its own set.
            if config.authority == SessionAuthority::Local {
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
                    nav,
                );
            }
            race::spawn_checkpoint_markers(
                &mut commands,
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                &def,
                owner,
            );
            // F22-A.2: the documented opponent indicator (HUD-3/CTL-1
            // `I`) — a session-owned marker pool sized to the authored
            // roster, bound per-frame to live non-local participants
            // only. Authored `hudmap_tri` content through the VFS;
            // `absent` on the report when it cannot bind.
            let oppind_report = crate::oppind::spawn_opponent_indicators(
                &mut commands,
                &vfs.0,
                roster.entries.len(),
                &mut assets.meshes,
                &mut assets.images,
                &mut assets.materials,
                owner,
            );
            commands.insert_resource(oppind_report);
            let key = event_key.clone().expect("an event setup carries its key");
            // F22-A.5: the RACE-6 compass arrow binds the authored
            // `hudarrow*` package for the event's family (green
            // Checkpoint/Circuit, red Blitz, violet Crash Course) —
            // rasterized into the ahead/behind sprite pair the
            // original's two paint jobs carry. `absent` on the report
            // when the authored content cannot bind.
            let arrow_report = crate::navarrow::spawn_nav_arrow(
                &mut commands,
                &vfs.0,
                &mut assets.images,
                owner,
                key.table,
            );
            if arrow_report.absent.is_none() {
                crate::navarrow3d::spawn_nav_arrow_view(
                    &mut commands,
                    &mut assets.meshes,
                    &mut assets.images,
                    &mut assets.materials,
                    owner,
                );
            }
            commands.insert_resource(arrow_report);
            race::spawn_race_warning(&mut commands, owner);
            race::spawn_countdown_banner(&mut commands, owner);
            // F22-A.4: the authored race timer (HUD-2's
            // stopwatch/countdown pair — `mmHUD`'s `mmTimer`s) —
            // binds the authored `digitac_*`/`digi_colon` glyph set
            // through the VFS; `absent` on the report when it cannot
            // bind.
            let timer_report =
                crate::racetime::spawn_race_timer(&mut commands, &vfs.0, &mut assets.images, owner);
            commands.insert_resource(timer_report);
            // F22-A.6: the remaining HUD-2 instruments — place
            // indicator, laps record (`Ordered`) and the checkpoint
            // list — bound to the authored `digitac_*_half` glyphs
            // through the same VFS path; `absent` on the report when
            // they cannot bind.
            let stat_report = crate::racestat::spawn_race_stats(
                &mut commands,
                &vfs.0,
                &mut assets.images,
                owner,
                &def,
            );
            commands.insert_resource(stat_report);
            // F16-B: the event's reward + availability surface —
            // consumed by `record_session_results` while the session
            // lives, removed by teardown so a following cruise never
            // sees it. (`key` was cloned out above for the HUD
            // spawns.)
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
