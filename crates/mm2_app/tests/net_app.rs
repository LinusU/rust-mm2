//! `mm2 --join` / `mm2 --host` — the app's Bevy-side lobby bridges
//! (F24-B.7 joined, F24-B.8 hosted).
//!
//! The in-process legs drive `drive_lobby`/`drive_host`/`drive_session`
//! inside a minimal `App` against a real loopback wire: the pump
//! thread, the sockets, the lobby's roster/start gates are all real —
//! only the world-load plugins are absent (a begun session parks in
//! `Loading`; the actual `load_session_world` legs are the
//! `headless_lobby`/`headless_host` in-process runs and the
//! `mm2 --join --headless`/`mm2 --host --headless` process legs below,
//! which carry the real asset stack).
//!
//! Loopback only — LAN/Internet reachability is F24-C. Remote players
//! are not spawned and nothing is replicated (F25/F26): these legs
//! prove the wire drives *this* app's session lifecycle, never a
//! parallel multiplayer world.

use std::net::{SocketAddr, TcpListener};
use std::thread;
use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use crate::support;

use mm2_app::net::{self, HostCommand, HostLink, LobbyLink, LobbyState};
use mm2_app::netdrive::{self, NetPlayer, RemotePick};
use mm2_app::session;
use mm2_app::session::{SelectedCar, SessionControl, TunedVehicle};
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_content::SurfaceTables;
use mm2_formats::materials::{MaterialMap, MaterialSet};
use mm2_game::{
    DevOverrides, ImpactEvent, ImpactId, Mm2Vfs, ObjectId, ObjectIdentity, Player, PlayerControl,
    PlayerVehicle, Session, SessionAuthority, SessionConfig, SessionMode, SessionPhase,
    SurfaceMaterial, SurfaceState, WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_net::{
    Client, Conn, DriveInput, Host, HostConfig, HostEvent, Impair, ImpairProxy, LateJoin,
    LeaveCause, LinkDir, Message, SNAP_NO_SURFACE, SnapEntry, SnapImpact, VehiclePick,
    accept_hello, hello, listen_loopback,
};
use mm2_vehicle::{ResetVehicle, Teleported, VehicleConfig, VehicleInput, VehicleState};
use support::{Proc, WAIT, listening, mount};

const HOST_EXE: &str = env!("CARGO_BIN_EXE_mm2-host");
const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// A dev-car-scale authored damage spec: the synthetic install's dev
/// car has no `vehcardamage` record, so the replication legs bind a
/// synthetic one to give the v8 damage byte a component to land on
/// (F25-B). Shaped like the retail bounds.
const DAMAGE_SPEC: mm2_game::DamageSpec = mm2_game::DamageSpec {
    impact_threshold: 1500.0,
    med_damage: 150_000.0,
    max_damage: 321_300.0,
    regenerate_rate: 0.0,
};

/// The wire session every leg advertises: a dev-world cruise — the
/// world the synthetic mounts can actually load.
fn dev_cruise() -> SessionConfig {
    SessionConfig {
        world: WorldMode::DevWorld,
        mode: SessionMode::Cruise,
        ..SessionConfig::default()
    }
}

/// A minimal event definition for the v13 race-row legs: the wire
/// field carries phase/countdown/clock whatever the route's shape, so
/// one checkpoint is enough. `countdown` names the authored countdown
/// length — `RaceState::new` opens with it.
fn wire_race_def(countdown: u32) -> mm2_game::RaceDefinition {
    mm2_game::RaceDefinition {
        checkpoints: vec![mm2_game::Checkpoint {
            center: Vec3::new(0.0, 0.0, -200.0),
            radius: 15.0,
            height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
            heading_deg: 0.0,
            require_direction: false,
        }],
        finish: None,
        rule: mm2_game::CheckpointRule::AnyOrder,
        laps: 0,
        time_limit_ticks: None,
        params: mm2_game::EventParams::default(),
        countdown_ticks: countdown,
        start_slots: Vec::new(),
    }
}

/// A loopback host advertising `config`, plus a joined `LobbyLink` and
/// the VFS the app checks sessions against. `fp` is this side's own
/// gameplay fingerprint — the in-process host simply trusts it.
fn host_and_link(
    dir: &std::path::Path,
    config: &SessionConfig,
    driver: &str,
) -> (Host, LobbyLink, Vfs) {
    let vfs = mount(dir);
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let host = Host::listen_loopback(&HostConfig::new(fp)).unwrap();
    host.set_session(net::advertise(config).unwrap()).unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), driver.to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    (host, link, vfs)
}

/// The shared half of both bridges: the session lifecycle
/// (`despawn_session_entities` → `drive_session`) and the resources the
/// lobby drain reads — minus the load/spawn systems that need the
/// asset stack.
fn lobby_app(vfs: Vfs) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Session::new())
        .init_resource::<SessionControl>()
        .init_resource::<LobbyState>()
        .init_resource::<ButtonInput<KeyCode>>()
        // F25-A.5: `track_reset_epochs` reads the same `ResetVehicle`
        // stream `vehicle_reset` applies.
        .add_message::<ResetVehicle>()
        // F25-B (v10): `publish_snapshots` reads the impact stream,
        // `apply_snapshots` writes the replicated one.
        .add_message::<mm2_game::ImpactEvent>()
        .add_message::<netdrive::RemoteImpact>()
        // F25-B (v13): `apply_snapshots` writes the release event when
        // the wire's race row runs the countdown out.
        .add_message::<mm2_game::RaceStarted>()
        // F25-B (v11): the breakaway reconcile claims pool slots and
        // writes the banger lifecycle stream like the authority does.
        .add_message::<mm2_game::BangerStateChanged>()
        .init_resource::<mm2_game::BangerPool>()
        // F25-B (v14): a replicated terminal edge records into the
        // session's result ledger like `advance_race` does.
        .init_resource::<mm2_game::ResultLedger>()
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(SelectedCar {
            def: None,
            paint: 0,
        })
        .insert_resource(TunedVehicle(VehicleConfig::default()))
        .insert_resource(session::SpawnPoint::new(Vec3::new(0.0, 1.5, 0.0), 0.0))
        .init_resource::<mm2_app::contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        // F25-A: the reconcile needs the asset stores to build remote
        // visuals (dev-car picks build meshes/materials here).
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .add_systems(
            Update,
            (
                despawn_session_entities.run_if(session::unloading),
                session::drive_session,
            )
                .chain(),
        )
        // The session clock, same registration the production app and
        // `run_headless` use: a hosted session's `Snap::tick` advances
        // like it does on the wire in a real process — a same-tick
        // republish is a receiver-side duplicate, which the impairment
        // matrix below measures as the clean baseline's stale floor.
        .add_systems(FixedUpdate, advance_session_tick);
    app
}

/// The joined-client bridge app, wired the way `run_headless` wires
/// it — the lobby drain settles after the session driver, the same
/// ordering the windowed app and `run_headless` use.
fn bridge_app(vfs: Vfs, link: LobbyLink) -> App {
    let mut app = lobby_app(vfs);
    app.insert_resource(link)
        .init_resource::<netdrive::RemoteSnaps>()
        .init_resource::<netdrive::InputSeq>()
        .init_resource::<netdrive::NetDriveReport>()
        .add_systems(
            Update,
            (
                net::lobby_input,
                net::drive_lobby.after(session::drive_session),
                // F25-A: same wiring as `run_headless` — reconcile and
                // snapshot application settle after the drain, the
                // input stream after the input owners. The apply runs
                // after the reconcile so the update's `NetPlayer`
                // stamps are visible to it — a snap held through the
                // load applies whole on the first live update.
                netdrive::reconcile_remote_players.after(net::drive_lobby),
                netdrive::apply_snapshots
                    .after(net::drive_lobby)
                    .after(netdrive::reconcile_remote_players),
                netdrive::drive_remote_lerp,
                // F26-A: replicated world props — production wiring.
                mm2_app::worldprops::apply_props.after(net::drive_lobby),
                // F26-A: replicated ambient cars — production wiring.
                mm2_app::worldtraffic::apply_traffic.after(net::drive_lobby),
                // F26-A: the host's world clock — production wiring.
                mm2_app::worldclock::apply_world_clock.after(net::drive_lobby),
                // F27-B.3: the host's Cops & Robbers match — production
                // wiring.
                mm2_app::cnrnet::apply_cnr.after(net::drive_lobby),
                // F25-B: `R` asks the authority under a predicted
                // session — production wiring.
                netdrive::send_reset_request,
                netdrive::send_drive_input,
            ),
        );
    app
}

/// The hosted-lobby bridge app — same wiring, `HostLink` side. The
/// `R`-bundle writer and the reset apply sit in the same ordering
/// contract the binary schedules (writer → `vehicle_reset` → epoch
/// tracker → `publish_snapshots`), so a test leg observes the real
/// same-frame epoch/pose coherence rather than a test-only stream.
fn host_app(vfs: Vfs, link: HostLink) -> App {
    let mut app = lobby_app(vfs);
    app.insert_resource(link)
        .init_resource::<netdrive::NetDriveReport>()
        .init_resource::<netdrive::WireStall>()
        // F25-B (v16): `surface_voices` resolves a wire seat's live
        // wheel contact into the `SurfaceContact` the publish encodes.
        // Its surface resources are `Option` — legs that never install
        // them run it as the no-table early return; the surface legs
        // insert `SurfaceAudio`/`SurfaceTables`/`WaveBank` themselves.
        .init_resource::<mm2_app::audio::AudioReport>()
        .init_resource::<Assets<mm2_app::audio::PcmAudio>>()
        .add_systems(
            Update,
            (
                mm2_app::input::reset_input.before(mm2_vehicle::systems::vehicle_reset),
                mm2_vehicle::systems::vehicle_reset,
                net::host_input,
                net::drive_host.after(session::drive_session),
                netdrive::reconcile_remote_players.after(net::drive_host),
                netdrive::apply_remote_inputs.after(net::drive_host),
                // F25-B: the stalled-seat retirement — production
                // ordering ahead of the publish.
                netdrive::retire_stalled_wire_seats
                    .after(net::drive_host)
                    .before(netdrive::publish_snapshots),
                // F25-B: the request drain is a `ResetVehicle` writer —
                // same ordering edge as `reset_input`.
                netdrive::apply_reset_requests
                    .after(net::drive_host)
                    .before(mm2_vehicle::systems::vehicle_reset),
                netdrive::track_reset_epochs
                    .after(net::drive_host)
                    .after(mm2_vehicle::systems::vehicle_reset)
                    .before(netdrive::publish_snapshots),
                // F25-B (v16): the live contact resolve — the same
                // `drive_session` ordering the windowed/headless
                // schedules keep (their `apply_snapshots` edge is
                // client-side; this app hosts).
                mm2_app::audio::surface_voices.after(session::drive_session),
                netdrive::publish_snapshots
                    .after(net::drive_host)
                    .after(mm2_vehicle::systems::vehicle_reset)
                    // F25-B (v16): the production contact→publish
                    // ordering — a seat's same-frame resolved contact
                    // publishes, not last frame's.
                    .after(mm2_app::audio::surface_voices),
                // F26-A: the world's prop state — production wiring.
                mm2_app::worldprops::publish_props.after(net::drive_host),
                // F26-A: the ambient population — production wiring.
                mm2_app::worldtraffic::publish_traffic.after(net::drive_host),
                // F26-A: the world clock — production wiring.
                mm2_app::worldclock::publish_world_clock.after(net::drive_host),
                // F27-B.3: the Cops & Robbers match — production wiring.
                mm2_app::cnrnet::publish_cnr.after(net::drive_host),
            ),
        );
    app
}

fn host_event(host: &Host) -> HostEvent {
    host.recv_timeout(WAIT).expect("no host event")
}

/// Drain host events until the `Started` verdict; returns the minted
/// generation. A `StartRefused` verdict is the answer too — panic with
/// its reason instead of timing out on a `Started` that never comes.
fn until_started(host: &Host) -> u64 {
    loop {
        match host_event(host) {
            HostEvent::Started { generation } => return generation,
            HostEvent::StartRefused { reason } => {
                panic!("the host refused the start: {reason}")
            }
            _ => {}
        }
    }
}

/// `app.update()` until `should_exit` reports — bounded so a broken
/// exit path fails instead of hanging.
fn until_exit(app: &mut App) -> AppExit {
    for _ in 0..200 {
        app.update();
        if let Some(exit) = app.should_exit() {
            return exit;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("the app never wrote AppExit");
}

fn session_phase(app: &App) -> SessionPhase {
    app.world().resource::<Session>().phase().clone()
}

/// Spin updates until a drained `Start` moves the session off `Menu`
/// — the host's `Started` verdict precedes the `Start` frame's trip
/// through the pump thread, so a single `update` can race it.
fn until_begun(app: &mut App) {
    for _ in 0..200 {
        app.update();
        if session_phase(app) != SessionPhase::Menu {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("the Start never began a session");
}

/// The roster leg: a joined link sees the advertised session and its
/// own roster entry, and the app stays parked at `Menu` — nothing
/// session-side moves on lobby traffic alone.
#[test]
fn joining_surfaces_the_lobby_without_starting_a_session() {
    let install = tempfile::tempdir().unwrap();
    let (mut host, link, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    let our_id = link.player_id();
    let mut app = bridge_app(vfs, link);

    app.update();

    let lobby = app.world().resource::<LobbyState>();
    assert_eq!(
        lobby.advertised.as_ref().map(|ad| ad.summary.as_str()),
        Some("dev world, cruise, amateur")
    );
    assert_eq!(lobby.roster.len(), 1);
    assert_eq!(lobby.roster[0].player_id, our_id);
    assert_eq!(lobby.roster[0].driver, "alice");
    assert_eq!(session_phase(&app), SessionPhase::Menu);
    assert!(app.should_exit().is_none());

    assert!(
        matches!(host_event(&host), HostEvent::Joined { .. }),
        "the host saw the handshake"
    );
    host.shutdown();
}

/// AC05's core leg: `Start` feeds the existing `Session` lifecycle —
/// `Loading` under the lobby's minted generation, carrying the wired
/// config (`Remote` authority, the advertised world/mode).
#[test]
fn a_start_begins_the_wired_session_under_the_lobby_generation() {
    let install = tempfile::tempdir().unwrap();
    let (mut host, link, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    link.ctl().set_vehicle("", 0).unwrap();
    link.ctl().set_ready(true).unwrap();
    let mut app = bridge_app(vfs, link);
    // join broadcast: Session ad + roster + pick echo — the echoed
    // ready flag is also the happens-before `start` needs.
    until_ready(&mut app);

    host.start(LateJoin::Open).unwrap();
    let generation = until_started(&host);
    until_begun(&mut app); // drains Start → begin_generation → Loading

    let session = app.world().resource::<Session>();
    assert_eq!(session.phase(), &SessionPhase::Loading);
    assert_eq!(session.wire_generation(), generation);
    let config = session.config().expect("the wired session is stored");
    assert_eq!(config.authority, SessionAuthority::Remote);
    assert!(matches!(config.world, WorldMode::DevWorld));
    assert!(matches!(config.mode, SessionMode::Cruise));
    let lobby = app.world().resource::<LobbyState>();
    assert_eq!(lobby.generation, Some(generation));
    host.shutdown();
}

/// `Cancel` for the running generation quits the live session through
/// the normal `Unloading → Menu` path — and the lobby, not the OS, is
/// where `Menu` lands: no `AppExit` is written.
#[test]
fn a_cancel_returns_the_session_to_the_lobby() {
    let install = tempfile::tempdir().unwrap();
    let (mut host, link, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    link.ctl().set_vehicle("", 0).unwrap();
    link.ctl().set_ready(true).unwrap();
    let mut app = bridge_app(vfs, link);
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app); // Start → Loading
    {
        // Stand the session up live — the load legs are elsewhere; the
        // lifecycle edge under test is `Playing → Unloading → Menu`.
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    host.cancel().unwrap();
    // Cancel → quit intent → Unloading → Menu: the frame's pump trip
    // can lag the host's `Cancelled` verdict, so spin to the terminal
    // phase rather than a fixed update count.
    for _ in 0..200 {
        app.update();
        if session_phase(&app) == SessionPhase::Menu {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    assert_eq!(session_phase(&app), SessionPhase::Menu);
    let lobby = app.world().resource::<LobbyState>();
    assert_eq!(lobby.generation, None, "the cancelled session cleared");
    assert!(lobby.pending_exit.is_none(), "a cancel is not an exit");
    assert!(
        lobby.advertised.is_some(),
        "the lobby keeps the next round's ad"
    );
    assert!(
        app.should_exit().is_none(),
        "quit-to-menu inside a lobby must not exit the app"
    );
    host.shutdown();
}

/// A `Start` drained while a session is still live cannot legally
/// `Menu → Loading` — it parks in `pending_start`, asks the lifecycle
/// for teardown, and begins only once `Menu` returns. (The host's own
/// gate refuses a second start mid-session, so this is the defensive
/// path: here the first session is a *local* begin the wire start
/// overtakes.)
#[test]
fn a_start_mid_session_parks_until_teardown_lands() {
    let install = tempfile::tempdir().unwrap();
    let (mut host, link, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    link.ctl().set_vehicle("", 0).unwrap();
    link.ctl().set_ready(true).unwrap();
    let mut app = bridge_app(vfs, link);
    // A session already underway — the wire `Start` must wait for its
    // teardown, not trample it.
    app.world_mut()
        .resource_mut::<Session>()
        .begin(dev_cruise())
        .unwrap();
    until_ready(&mut app);

    host.start(LateJoin::Open).unwrap();
    let wire_generation = until_started(&host);
    // The host's verdict precedes the frame's trip through the pump —
    // spin until the drained `Start` parks.
    for _ in 0..200 {
        app.update();
        if app.world().resource::<LobbyState>().pending_start.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    let lobby = app.world().resource::<LobbyState>();
    assert!(
        matches!(lobby.pending_start, Some((g, _)) if g == wire_generation),
        "the start parked: {:?}",
        lobby.pending_start
    );
    assert!(
        app.world().resource::<SessionControl>().quit,
        "the parked start asked the live session to yield"
    );
    assert_eq!(session_phase(&app), SessionPhase::Loading);

    // Let the live session reach a quittable phase and tear down —
    // the parked begin applies on the update that lands at Menu.
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    app.update(); // quit → Unloading
    app.update(); // Unloading → Menu; the parked start begins

    let session = app.world().resource::<Session>();
    assert_eq!(session.phase(), &SessionPhase::Loading);
    // The wire generation is the host's mint adopted verbatim — even
    // behind the local counter, which clamps forward instead: the
    // local begin above took generation 1, so the wire value here sits
    // at 1 while the local namespace is already at 2 (F25-B).
    assert_eq!(
        session.wire_generation(),
        wire_generation,
        "the wire generation is the host's mint, verbatim"
    );
    assert_eq!(
        session.generation(),
        wire_generation.max(2),
        "the local counter never regresses behind its own mints"
    );
    assert_eq!(
        session.config().unwrap().authority,
        SessionAuthority::Remote
    );
    host.shutdown();
}

/// A dead host is a terminal lobby condition, not a stuck one: the
/// link closes, the live session tears down through the lifecycle,
/// the notice says why, and the app exits nonzero.
#[test]
fn losing_the_host_tears_down_and_exits() {
    let install = tempfile::tempdir().unwrap();
    let (mut host, link, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    link.ctl().set_vehicle("", 0).unwrap();
    link.ctl().set_ready(true).unwrap();
    let mut app = bridge_app(vfs, link);
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app); // Start → Loading
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    host.shutdown(); // the socket dies under the pump
    let exit = until_exit(&mut app);

    assert!(
        matches!(exit, AppExit::Error(code) if code.get() == 1),
        "a lost host is a nonzero exit, got {exit:?}"
    );
    assert_eq!(session_phase(&app), SessionPhase::Menu);
    let lobby = app.world().resource::<LobbyState>();
    assert!(
        lobby
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("lost the host")),
        "the notice names the cause: {:?}",
        lobby.notice
    );
    assert!(lobby.roster.is_empty() && lobby.advertised.is_none());
}

/// The polite end: `LobbyLink::leave` reaches the host as a `Quit`
/// (not a dropped socket), the host's close ends the pump, and the
/// app exits 0.
#[test]
fn leaving_the_lobby_reports_quit_and_exits_cleanly() {
    let install = tempfile::tempdir().unwrap();
    let (mut host, link, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    let mut app = bridge_app(vfs, link);
    app.update();
    app.world_mut().resource_mut::<LobbyLink>().leave();

    // The host sees the deliberate quit, not a dropped connection.
    loop {
        match host_event(&host) {
            HostEvent::Left { cause, .. } => {
                assert_eq!(cause, LeaveCause::Quit);
                break;
            }
            HostEvent::Joined { .. } => continue,
            other => panic!("unexpected host event: {other:?}"),
        }
    }

    // The host closed our socket after the `Leave` — the pump's
    // `Closed` is the expected end of our own goodbye.
    let exit = until_exit(&mut app);
    assert_eq!(exit, AppExit::Success);
    host.shutdown();
}

/// A `quit` intent consumed at `Menu` while a lobby owns the surface
/// must not exit the app — `drive_session` defers to the link.
#[test]
fn a_menu_quit_stays_inside_the_lobby() {
    let install = tempfile::tempdir().unwrap();
    let (mut host, link, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    let mut app = bridge_app(vfs, link);
    app.update();

    app.world_mut().resource_mut::<SessionControl>().quit = true;
    app.update();

    assert!(
        app.should_exit().is_none(),
        "a Menu quit must return to the lobby, not the OS"
    );
    assert!(
        !app.world().resource::<SessionControl>().quit,
        "the intent was consumed, not left dangling"
    );
    host.shutdown();
}

/// The join-side content gate at the bridge: a host advertising a
/// session this mount cannot run gets a notice, a clean `Quit`
/// leave, and a nonzero exit — never a silent sit in a broken lobby.
#[test]
fn an_unrunnable_session_is_refused_with_a_clean_leave() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host = Host::listen_loopback(&HostConfig::new(fp)).unwrap();
    // Resolves to nothing on the client's mount — `check_session`'s
    // World leg refuses it (same gate mm2-join runs).
    host.set_session(
        net::advertise(&SessionConfig {
            world: WorldMode::City {
                psdl: "city/nothere.psdl".to_string(),
            },
            mode: SessionMode::Cruise,
            ..SessionConfig::default()
        })
        .unwrap(),
    )
    .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "bob".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .unwrap();
    let mut app = bridge_app(vfs, link);

    let exit = until_exit(&mut app);

    assert!(
        matches!(exit, AppExit::Error(code) if code.get() == 1),
        "a refused session is a nonzero exit, got {exit:?}"
    );
    let lobby = app.world().resource::<LobbyState>();
    assert!(
        lobby
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("cannot run here")),
        "the notice names the refusal: {:?}",
        lobby.notice
    );
    loop {
        match host_event(&host) {
            HostEvent::Left { cause, .. } => {
                assert_eq!(cause, LeaveCause::Quit);
                break;
            }
            HostEvent::Joined { .. } => continue,
            other => panic!("unexpected host event: {other:?}"),
        }
    }
    host.shutdown();
}

// ─── The in-app host (F24-B.8) ─────────────────────────────────────
//
// `HostLink` hosts the lobby inside the app: the host seat is the
// local player (never a wire roster entry — ids start at 1), and
// `drive_host` drains host-loop events plus operator commands into the
// same `LobbyState` the joined bridge fills.

/// A loopback `HostLink` advertising `config` — the in-app half of
/// `mm2 --host`. The fingerprint is the mount's own, so a peer built
/// from it satisfies the handshake gate.
fn host_link(dir: &std::path::Path, config: &SessionConfig) -> (HostLink, Vfs, u64) {
    let vfs = mount(dir);
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let link = HostLink::open(
        "127.0.0.1:0".parse().unwrap(),
        config,
        "host".to_string(),
        fp,
        None,
    )
    .expect("host open failed");
    (link, vfs, fp)
}

/// A bare wire client joined to a hosted lobby — the remote peer the
/// host-side roster mirror is built from.
fn remote_peer(addr: SocketAddr, driver: &str, fp: u64) -> Client {
    let client = Client::join(
        addr,
        &hello("net-app-test".to_string(), driver.to_string(), fp),
    )
    .expect("remote join failed");
    client.set_timeout(Some(WAIT)).unwrap();
    client
}

/// `peer.recv()` until `pred` holds — bounded by the peer's read
/// timeout, so a missing message fails rather than hanging.
fn until_wire(peer: &mut Client, pred: impl Fn(&Message) -> bool) -> Message {
    loop {
        let msg = peer.recv().expect("the peer stream ended");
        if pred(&msg) {
            return msg;
        }
    }
}

/// A remote peer already past the host's start gate — pick + ready
/// sent and the roster broadcast showing both — so a following
/// `start` cannot race the loop's intake.
fn ready_peer(addr: SocketAddr, driver: &str, fp: u64) -> Client {
    let mut peer = remote_peer(addr, driver, fp);
    let ctl = peer.ctl().unwrap();
    ctl.set_vehicle("", 0).unwrap();
    ctl.set_ready(true).unwrap();
    until_wire(
        &mut peer,
        |m| matches!(m, Message::Roster { players: r } if r.iter().any(|e| e.driver == driver && e.ready && e.pick.is_some())),
    );
    peer
}

/// `app.update()` until `pred` holds — bounded so a broken settle
/// fails rather than hanging.
fn spin(app: &mut App, pred: impl Fn(&App) -> bool) {
    for _ in 0..200 {
        app.update();
        if pred(app) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("the app never reached the expected state");
}

/// The `LobbyLink` half of `ready_peer`'s discipline: `set_*` writes
/// ride the socket through the host's reader thread, so a `Start` sent
/// before the roster echo showing our pick+ready can reach the loop's
/// control channel first and be refused. The loop broadcasts the
/// roster only after applying each update, so the echoed flag is the
/// happens-before `host.start` needs.
fn until_ready(app: &mut App) {
    spin(app, |a| {
        let world = a.world();
        let our_id = world.resource::<LobbyLink>().player_id();
        world
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.player_id == our_id && e.ready && e.pick.is_some())
    });
}

/// The minimal test app has no `InputPlugin` clearing `ButtonInput`
/// per frame — a `just_pressed` would linger into every later update
/// and `pressed` survives `clear()`, so a re-press needs the release.
/// Mirror the real app: `tap` is one discrete keypress.
fn clear_keys(app: &mut App) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .clear();
}

fn tap(app: &mut App, key: KeyCode) {
    {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(key);
        keys.press(key);
    }
    app.update();
    clear_keys(app);
}

/// A hosted lobby parks at `Menu` like a joined one, but the roster
/// mirror starts empty: the host seat is the local player, never a
/// wire entry. Remote joins/picks/readiness/leaves fill it from host
/// events — and the advertised session is the link's own config,
/// stamped `Host` authority.
#[test]
fn the_hosted_roster_mirrors_remote_players_only() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);

    app.update();
    {
        let lobby = app.world().resource::<LobbyState>();
        assert_eq!(
            lobby.advertised.as_ref().map(|ad| ad.summary.as_str()),
            Some("dev world, cruise, amateur")
        );
        assert!(
            lobby.roster.is_empty(),
            "the host seat is not a wire roster entry"
        );
    }
    assert_eq!(
        app.world().resource::<HostLink>().config().authority,
        SessionAuthority::Host
    );
    assert_eq!(session_phase(&app), SessionPhase::Menu);
    assert!(app.should_exit().is_none());

    let peer = remote_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world().resource::<LobbyState>().roster.len() == 1
    });
    {
        let lobby = app.world().resource::<LobbyState>();
        assert_eq!(lobby.roster[0].driver, "eve");
        assert!(
            lobby.roster[0].player_id > 0,
            "wire ids skip the host's slot"
        );
        assert!(!lobby.roster[0].ready);
        assert!(lobby.roster[0].pick.is_none());
    }
    let ctl = peer.ctl().unwrap();
    ctl.set_vehicle("", 0).unwrap();
    ctl.set_ready(true).unwrap();
    spin(&mut app, |a| {
        let lobby = a.world().resource::<LobbyState>();
        lobby.roster[0].ready && lobby.roster[0].pick.is_some()
    });

    peer.leave().unwrap();
    spin(&mut app, |a| {
        a.world().resource::<LobbyState>().roster.is_empty()
    });
    assert!(app.should_exit().is_none(), "a peer leaving is not an exit");
}

/// The operator's `start` mints the generation, broadcasts `Start`,
/// and begins the host seat's session — `Host` authority, the
/// advertised config — on the same drain the peer heard it on.
#[test]
fn a_hosted_start_begins_the_session_for_everyone() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let commands = link.command_sender();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world().resource::<LobbyState>().roster.len() == 1
    });

    commands.send(HostCommand::Start).unwrap();
    app.update(); // drains the command → ctl.start → the loop mints
    let generation = match until_wire(&mut peer, |m| matches!(m, Message::Start { .. })) {
        Message::Start { generation, .. } => generation,
        _ => unreachable!(),
    };
    spin(&mut app, |a| session_phase(a) == SessionPhase::Loading);

    let session = app.world().resource::<Session>();
    assert_eq!(session.wire_generation(), generation);
    let config = session.config().expect("the hosted session is stored");
    assert_eq!(config.authority, SessionAuthority::Host);
    assert!(matches!(config.world, WorldMode::DevWorld));
    assert_eq!(
        app.world().resource::<LobbyState>().generation,
        Some(generation)
    );
    assert!(app.should_exit().is_none());
}

/// `start` against an unready peer is the lobby gate's verdict: a
/// `StartRefused` surfaces as a lobby notice — nothing mints, nothing
/// begins, and the lobby keeps running.
#[test]
fn a_start_with_an_unready_peer_is_refused() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let commands = link.command_sender();
    let mut app = host_app(vfs, link);
    let _peer = remote_peer(addr, "eve", fp); // joined but never ready
    spin(&mut app, |a| {
        a.world().resource::<LobbyState>().roster.len() == 1
    });

    commands.send(HostCommand::Start).unwrap();
    spin(&mut app, |a| {
        a.world().resource::<LobbyState>().notice.is_some()
    });

    let lobby = app.world().resource::<LobbyState>();
    assert!(
        lobby
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("not ready")),
        "the refusal names the gate: {:?}",
        lobby.notice
    );
    assert_eq!(lobby.generation, None, "a refused start mints nothing");
    assert_eq!(session_phase(&app), SessionPhase::Menu);
    assert!(app.should_exit().is_none());
}

/// A hosted session ending locally is the lobby's `Cancel`: the wire
/// hears the generation close, the mirror clears, and the lobby
/// re-opens for the next `start` — which mints a fresh generation.
/// (Readiness resets on `Cancel`, so the peer re-consents first.)
#[test]
fn a_hosted_session_end_cancels_the_wire_session() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let commands = link.command_sender();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world().resource::<LobbyState>().roster.len() == 1
    });
    commands.send(HostCommand::Start).unwrap();
    app.update();
    until_wire(&mut peer, |m| matches!(m, Message::Start { .. }));
    spin(&mut app, |a| session_phase(a) == SessionPhase::Loading);
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    // The local session ends — finish/quit lands the phase at `Menu`,
    // where `drive_host` settles it as the wire's `Cancel`.
    app.world_mut().resource_mut::<SessionControl>().quit = true;
    spin(&mut app, |a| {
        session_phase(a) == SessionPhase::Menu
            && a.world().resource::<LobbyState>().generation.is_none()
    });
    match until_wire(&mut peer, |m| matches!(m, Message::Cancel { .. })) {
        Message::Cancel { generation } => assert_eq!(generation, 1),
        _ => unreachable!(),
    }
    assert_eq!(
        app.world().resource::<LobbyState>().roster.len(),
        1,
        "the lobby re-opens with the roster intact"
    );
    assert!(app.should_exit().is_none(), "a session end is not an exit");

    // Next round: `Cancel` reset the roster's readiness — re-ready and
    // the mint moves forward.
    let ctl = peer.ctl().unwrap();
    ctl.set_ready(true).unwrap();
    until_wire(
        &mut peer,
        |m| matches!(m, Message::Roster { players: r } if r.iter().all(|e| e.ready)),
    );
    commands.send(HostCommand::Start).unwrap();
    app.update();
    match until_wire(&mut peer, |m| matches!(m, Message::Start { .. })) {
        Message::Start { generation, .. } => assert_eq!(generation, 2),
        _ => unreachable!(),
    }
}

/// `quit` while a hosted session runs: the wire session is cancelled
/// ahead of the sockets dying (both ride the same control channel —
/// the `Cancel` is processed first), the live session tears down to
/// `Menu`, and the app exits cleanly.
#[test]
fn quitting_the_host_cancels_the_session_and_exits() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let commands = link.command_sender();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world().resource::<LobbyState>().roster.len() == 1
    });
    commands.send(HostCommand::Start).unwrap();
    app.update();
    until_wire(&mut peer, |m| matches!(m, Message::Start { .. }));
    spin(&mut app, |a| session_phase(a) == SessionPhase::Loading);
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    commands.send(HostCommand::Quit).unwrap();
    let exit = until_exit(&mut app);
    assert_eq!(exit, AppExit::Success, "a hosted quit is a clean exit");
    assert_eq!(session_phase(&app), SessionPhase::Menu);
    assert!(matches!(
        until_wire(&mut peer, |m| matches!(m, Message::Cancel { .. })),
        Message::Cancel { generation: 1 }
    ));
}

/// The host loop dying under the link is the hosted lobby's lost-host
/// equivalent: a live session tears down, the notice names it, and the
/// app exits nonzero — never a silent sit on a dead lobby.
#[test]
fn a_dead_host_loop_tears_down_and_exits() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let commands = link.command_sender();
    let mut app = host_app(vfs, link);
    let _peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world().resource::<LobbyState>().roster.len() == 1
    });
    commands.send(HostCommand::Start).unwrap();
    app.update();
    spin(&mut app, |a| session_phase(a) == SessionPhase::Loading);
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    // Kill the loop without going through `leave()` — the ctl-channel
    // shutdown is the same observable end an internal failure gives
    // the drain: the event channel disconnects.
    app.world().resource::<HostLink>().ctl().shutdown().unwrap();
    let exit = until_exit(&mut app);

    assert!(
        matches!(exit, AppExit::Error(code) if code.get() == 1),
        "a dead host loop is a nonzero exit, got {exit:?}"
    );
    assert_eq!(session_phase(&app), SessionPhase::Menu);
    let lobby = app.world().resource::<LobbyState>();
    assert!(
        lobby
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("host loop died")),
        "the notice names the cause: {:?}",
        lobby.notice
    );
}

/// A `quit` intent consumed at `Menu` while a hosted lobby owns the
/// surface must not exit the app — `drive_session` defers to the link.
#[test]
fn a_menu_quit_stays_inside_the_hosted_lobby() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, _fp) = host_link(install.path(), &dev_cruise());
    let mut app = host_app(vfs, link);
    app.update();

    app.world_mut().resource_mut::<SessionControl>().quit = true;
    app.update();

    assert!(
        app.should_exit().is_none(),
        "a Menu quit must return to the lobby, not the OS"
    );
    assert!(
        !app.world().resource::<SessionControl>().quit,
        "the intent was consumed, not left dangling"
    );
}

/// The windowed keys ride the same command channel stdin drives —
/// `Enter` requests a start (an empty roster passes the gate: the host
/// seat is a player), `Esc` at `Menu` takes the lobby down.
#[test]
fn host_input_keys_ride_the_command_channel() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, _fp) = host_link(install.path(), &dev_cruise());
    let mut app = host_app(vfs, link);
    app.update();

    tap(&mut app, KeyCode::Enter);
    spin(&mut app, |a| session_phase(a) == SessionPhase::Loading);
    assert_eq!(app.world().resource::<LobbyState>().generation, Some(1));

    // `Esc` mid-session is not the lobby's key — `host_input` only
    // acts while parked at `Menu`.
    tap(&mut app, KeyCode::Escape);
    assert!(app.should_exit().is_none());
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    app.world_mut().resource_mut::<SessionControl>().quit = true;
    spin(&mut app, |a| session_phase(a) == SessionPhase::Menu);

    // Back at the lobby: `Esc` is the host's shutdown — the auto-`Cancel`
    // already ran, so this is `leave` + the queued clean exit.
    tap(&mut app, KeyCode::Escape);
    let exit = until_exit(&mut app);
    assert_eq!(exit, AppExit::Success);
}

/// The end-to-end hosted leg: a real `headless_host` app — full plugin
/// stack, real `load_session_world` — runs the advertised session once
/// the operator's `start` mints, with a remote peer on the same wire.
#[test]
fn a_headless_host_runs_the_advertised_session() {
    let install = tempfile::tempdir().unwrap();
    let fp = mm2_content::fingerprint::gameplay(&mount(install.path()))
        .unwrap()
        .hash;
    let link = HostLink::open(
        "127.0.0.1:0".parse().unwrap(),
        &dev_cruise(),
        "host".to_string(),
        fp,
        None,
    )
    .unwrap();
    let addr = link.addr();
    let commands = link.command_sender();
    let run = thread::spawn(move || {
        smoke::headless_host(
            link,
            mount(install.path()),
            SelectedCar {
                def: None,
                paint: 0,
            },
            &VehicleConfig::default(),
            600,
            smoke::Driver::Hold,
            None,
        )
    });

    let mut peer = ready_peer(addr, "eve", fp);
    commands.send(HostCommand::Start).unwrap();
    let generation = match until_wire(&mut peer, |m| matches!(m, Message::Start { .. })) {
        Message::Start { generation, .. } => generation,
        _ => unreachable!(),
    };
    assert_eq!(generation, 1);

    let rec = run.join().expect("the headless host run panicked");
    assert_eq!(rec.status, SmokeStatus::Pass, "{}", rec.line());
    assert!(
        rec.line().contains(&format!("mp=gen{generation}")),
        "the record carries the hosted generation: {}",
        rec.line()
    );
    assert!(
        rec.line().contains("world=dev-world"),
        "the record names the advertised world: {}",
        rec.line()
    );
    // The run's `HostLink` drop already took the lobby down — a polite
    // `leave` at this point may race the closed socket.
    let _ = peer.leave();
}

/// The stock `F4` restart binding (CTL-1) is a single-player control:
/// under a `Remote`-authority session the wire owns restarts, so the
/// key must not queue `control.restart`.
#[test]
fn f4_restart_is_a_local_authority_binding() {
    for (authority, expected) in [
        (SessionAuthority::Local, true),
        (SessionAuthority::Remote, false),
    ] {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Session::new())
            .init_resource::<SessionControl>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, session::session_control_input);
        {
            let mut session = app.world_mut().resource_mut::<Session>();
            session
                .begin(SessionConfig {
                    authority,
                    ..dev_cruise()
                })
                .unwrap();
            session.transition(SessionPhase::Ready).unwrap();
            session.transition(SessionPhase::Playing).unwrap();
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F4);
        app.update();
        assert_eq!(
            app.world().resource::<SessionControl>().restart,
            expected,
            "{authority:?} restart binding"
        );
    }
}

/// The generation contract (`Session::begin_generation`): a host-minted
/// generation is adopted verbatim as the *wire* namespace, and a wire
/// value that would regress the local counter is clamped forward
/// instead — staleness detection on `generation`-keyed ids depends on
/// never reusing one, while the wire stamps follow the authority's
/// numbering even when it restarts.
#[test]
fn a_host_generation_is_adopted_but_never_regresses() {
    let mut session = Session::new();
    session.begin_generation(dev_cruise(), 3).unwrap();
    assert_eq!(session.generation(), 3);
    assert_eq!(session.wire_generation(), 3);
    session.transition(SessionPhase::Unloading).unwrap();
    session.transition(SessionPhase::Menu).unwrap();
    session.begin_generation(dev_cruise(), 1).unwrap();
    assert_eq!(
        session.generation(),
        4,
        "a regressed wire generation clamps to the local counter"
    );
    assert_eq!(
        session.wire_generation(),
        1,
        "the wire value is adopted verbatim — a different authority restarts its numbering"
    );
}

/// The authority-boundary contract on the snap stream (F25-B):
/// `RemoteSnaps` is process-lifetime state while the `(generation,
/// tick)` sequence belongs to the current *authority* — a fresh host
/// restarts its numbering from 1, so a rejoining client must not drop
/// the new stream under the dead stream's watermark. The leg runs the
/// real lifecycle: authority A's stream applies; the host dies
/// (`Closed` resets the inbox); authority B's `Start` adopts wire
/// generation 1 verbatim — behind the local counter — and its
/// restarted tick sequence applies.
#[test]
fn a_dead_authoritys_watermark_dies_with_the_link() {
    let install = tempfile::tempdir().unwrap();
    let (mut host_a, link_a, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    link_a.ctl().set_vehicle("", 0).unwrap();
    link_a.ctl().set_ready(true).unwrap();
    let mut app = bridge_app(vfs, link_a);
    until_ready(&mut app);
    host_a.start(LateJoin::Open).unwrap();
    let gen_a = until_started(&host_a);
    until_begun(&mut app);

    // The minimal app has no loader — move the session through the
    // two legal steps `load_session_world` would have run so the
    // stream applies (a `Loading` session holds it) and the
    // quit-to-teardown leg can land.
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    // The dead stream's watermark climbs high enough that a restarted
    // (generation 1, low tick) sequence would read as a straggler
    // without the boundary reset.
    host_a
        .ctl()
        .broadcast(&Message::Snap {
            generation: gen_a,
            tick: 900,
            entries: Vec::new(),
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<netdrive::RemoteSnaps>().applied() == Some((gen_a, 900))
    });

    host_a.shutdown();
    let exit = until_exit(&mut app);
    assert_ne!(
        exit,
        AppExit::Success,
        "a lost host is a nonzero exit: {exit:?}"
    );
    assert_eq!(
        app.world().resource::<netdrive::RemoteSnaps>().applied(),
        None,
        "the Closed boundary cleared the dead stream's watermark"
    );

    // Authority B: a fresh host process restarts its numbering at
    // generation 1 — the stream regression the watermark would have
    // swallowed.
    let (mut host_b, link_b, _vfs_b) = host_and_link(install.path(), &dev_cruise(), "alice");
    link_b.ctl().set_vehicle("", 0).unwrap();
    link_b.ctl().set_ready(true).unwrap();
    app.insert_resource(link_b);
    until_ready(&mut app);
    host_b.start(LateJoin::Open).unwrap();
    let gen_b = until_started(&host_b);
    assert_eq!(gen_b, 1, "the fresh authority restarts its numbering");
    until_begun(&mut app);
    {
        let session = app.world().resource::<Session>();
        assert_eq!(
            session.wire_generation(),
            1,
            "authority B's generation adopts verbatim"
        );
        assert_eq!(
            session.generation(),
            2,
            "the local counter climbed past the regressed wire value"
        );
    }
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    host_b
        .ctl()
        .broadcast(&Message::Snap {
            generation: gen_b,
            tick: 5,
            entries: Vec::new(),
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<netdrive::RemoteSnaps>().applied() == Some((gen_b, 5))
    });
    let report = app.world().resource::<netdrive::NetDriveReport>();
    assert_eq!(report.snaps_applied, 2, "one snap applied per authority");
    assert_eq!(
        report.snaps_staled, 0,
        "the restarted stream never dropped stale"
    );
    host_b.shutdown();
}

/// The other half of the boundary contract: the `Closed` reset is
/// not the only stream boundary — an accepted `Start` is too, because
/// a wire client can observe *no* close at all when the authority is
/// swapped underneath it. The link resource is replaced wholesale —
/// its pump dies with the `Closed` event still queued — so the only
/// boundary signal the drain sees is authority B's `Start`. Without
/// the accept-side reset the restarted tick sequence would drop under
/// authority A's watermark.
#[test]
fn a_start_resets_the_stream_without_a_close() {
    let install = tempfile::tempdir().unwrap();
    let (mut host_a, link_a, vfs) = host_and_link(install.path(), &dev_cruise(), "alice");
    link_a.ctl().set_vehicle("", 0).unwrap();
    link_a.ctl().set_ready(true).unwrap();
    let mut app = bridge_app(vfs, link_a);
    until_ready(&mut app);
    host_a.start(LateJoin::Open).unwrap();
    let gen_a = until_started(&host_a);
    until_begun(&mut app);
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    host_a
        .ctl()
        .broadcast(&Message::Snap {
            generation: gen_a,
            tick: 900,
            entries: Vec::new(),
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<netdrive::RemoteSnaps>().applied() == Some((gen_a, 900))
    });

    // The authority is swapped mid-session: a fresh host (its
    // numbering restarts at generation 1) and a fresh link. The old
    // `LobbyLink` drops with the resource — its pump and the `Closed`
    // event it would deliver die with it, so only `Start` can signal
    // the boundary.
    let (mut host_b, link_b, _vfs_b) = host_and_link(install.path(), &dev_cruise(), "alice");
    link_b.ctl().set_vehicle("", 0).unwrap();
    link_b.ctl().set_ready(true).unwrap();
    app.insert_resource(link_b);
    until_ready(&mut app);
    host_b.start(LateJoin::Open).unwrap();
    let gen_b = until_started(&host_b);
    assert_eq!(gen_b, 1, "the fresh authority restarts its numbering");
    // B's `Start` arrives while A's session is still `Playing`: it
    // parks in `pending_start` until the teardown lands at `Menu`,
    // then begins under the new wire generation.
    spin(&mut app, |a| {
        a.world().resource::<Session>().generation() == 2
    });
    {
        let session = app.world().resource::<Session>();
        assert_eq!(session.wire_generation(), 1);
        assert_eq!(session.phase(), &SessionPhase::Loading);
    }
    // The load's end a `Loading` session waits on before the stream
    // applies — staged here like every other leg.
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    host_b
        .ctl()
        .broadcast(&Message::Snap {
            generation: gen_b,
            tick: 5,
            entries: Vec::new(),
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<netdrive::RemoteSnaps>().applied() == Some((gen_b, 5))
    });
    let report = app.world().resource::<netdrive::NetDriveReport>();
    assert_eq!(report.snaps_applied, 2, "one snap applied per authority");
    assert_eq!(
        report.snaps_staled, 0,
        "the restarted stream never dropped stale"
    );
    host_a.shutdown();
    host_b.shutdown();
}

/// The mint's floor: a `Start` naming generation `0` is a
/// non-conforming peer, not a session — a conforming `mm2_net` host
/// mints from `1`, and `0` is the at-rest `wire_generation` every
/// gate reads as "no session has begun". `begin_generation` refuses
/// the adoption, so the boundary rides the same refusal path an
/// unacceptable session takes: notice, clean `Leave`, nonzero exit —
/// and the session never begins. The leg drives a raw socket: the
/// field is wire-legal (a plain `u64`), conforming hosts just never
/// send it, so a real `Host` cannot mint the bad value for the leg.
#[test]
fn a_zero_generation_start_is_refused() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let ad = net::advertise(&dev_cruise()).unwrap();
    let listener = listen_loopback().unwrap();
    let addr = listener.local_addr().unwrap();
    // The rogue host: handshake, `Welcome`, then a `Start` minted 0.
    // The refusal ends in the client's `Leave`; read it, then close.
    let rogue = thread::spawn(move || {
        let mut conn = Conn::accept(&listener).unwrap();
        accept_hello(&mut conn, fp).unwrap();
        conn.send(&Message::Welcome { player_id: 1 }).unwrap();
        conn.send(&Message::Start {
            generation: 0,
            session: ad,
            host_pick: None,
        })
        .unwrap();
        match conn.recv() {
            Ok(Message::Leave) => {}
            other => panic!("expected the client's Leave, got {other:?}"),
        }
    });
    let link = LobbyLink::join(
        addr,
        &hello("net-app-test".to_string(), "mallory".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .unwrap();
    let mut app = bridge_app(vfs, link);

    let exit = until_exit(&mut app);

    assert!(
        matches!(exit, AppExit::Error(code) if code.get() == 1),
        "a gen-0 Start is a refused session, got {exit:?}"
    );
    let session = app.world().resource::<Session>();
    assert_eq!(session.phase(), &SessionPhase::Menu);
    assert_eq!(
        session.wire_generation(),
        0,
        "the at-rest mint was never adopted"
    );
    assert!(session.config().is_none(), "no session config was stored");
    let lobby = app.world().resource::<LobbyState>();
    assert!(
        lobby
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("refused")),
        "the notice names the refusal: {:?}",
        lobby.notice
    );
    rogue.join().unwrap();
}

/// The end-to-end in-process leg: a real `headless_lobby` app — full
/// plugin stack, real `load_session_world` — joins an in-process host,
/// and the host's `Start` loads the wired world. This is the
/// `Start → Session::begin_generation → load_session_world` path the
/// windowed `mm2 --join` runs, minus the window.
#[test]
fn a_lobby_start_loads_the_wired_world_headless() {
    let install = tempfile::tempdir().unwrap();
    let fp = mm2_content::fingerprint::gameplay(&mount(install.path()))
        .unwrap()
        .hash;
    let mut host = Host::listen_loopback(&HostConfig::new(fp)).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "dave".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .unwrap();
    let ctl = link.ctl().clone();
    let run = thread::spawn(move || {
        smoke::headless_lobby(
            link,
            mount(install.path()),
            SelectedCar {
                def: None,
                paint: 0,
            },
            &VehicleConfig::default(),
            600,
            smoke::Driver::Hold,
            None,
        )
    });

    // The roster leg of the start gate — pick + ready through the
    // same `ClientCtl` `--ready`/`--car` drive from `main`.
    ctl.set_vehicle("", 0).unwrap();
    ctl.set_ready(true).unwrap();
    loop {
        match host_event(&host) {
            HostEvent::ReadyChanged { ready: true, .. } => break,
            _ => continue,
        }
    }
    host.start(LateJoin::Open).unwrap();
    let generation = until_started(&host);

    let rec = run.join().expect("the headless lobby run panicked");
    assert_eq!(rec.status, SmokeStatus::Pass, "{}", rec.line());
    assert!(
        rec.line().contains(&format!("mp=gen{generation}")),
        "the record carries the lobby's generation: {}",
        rec.line()
    );
    assert!(
        rec.line().contains("world=dev-world"),
        "the record names the world the wire loaded: {}",
        rec.line()
    );
    host.shutdown();
}

/// The separate-process leg (AC05): `mm2 --join --headless` against a
/// real `mm2-host` — the host's `start` mints the session this app's
/// own `Session` then loads and drives. `mp=gen1`/`world=dev-world`
/// on the smoke record are the bridge's evidence.
#[test]
fn mm2_join_loads_the_hosts_started_session() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(
        HOST_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--dev-world".to_string(),
            "--bind".to_string(),
            "127.0.0.1:0".to_string(),
            "--seed".to_string(),
            7.to_string(),
        ],
    );
    let (addr, _fp, _) = listening(&host);

    let client = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--join".to_string(),
            addr.to_string(),
            "--driver".to_string(),
            "carol".to_string(),
            "--ready".to_string(),
            "--headless".to_string(),
            "--frames".to_string(),
            "900".to_string(),
        ],
    );

    // The join handshake, pick offer and ready are the app bridge's —
    // the host's records are the wire's view of it.
    let joined = host.until("event=joined");
    assert!(joined.contains("driver=\"carol\""), "{joined}");
    assert_eq!(host.line(), "event=vehicle id=1 vehicle=\"\" paint=0");
    assert_eq!(host.line(), "event=ready id=1 ready=true");

    host.cmd("start");
    assert_eq!(host.until("event=started"), "event=started generation=1");

    let rec = client.until("smoke=");
    assert!(rec.contains("world=dev-world"), "{rec}");
    assert!(rec.contains("mp=gen1"), "{rec}");
    assert!(
        rec.contains("status=pass"),
        "the wired session loaded and ran: {rec}"
    );
    assert!(client.wait().success(), "the join client did not exit 0");

    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// The return leg at process level: `cancel` sends the running session
/// back through `Unloading → Menu` into the lobby — the smoke record
/// reports the parked lobby state, not an exit.
#[test]
fn mm2_join_returns_to_the_lobby_on_cancel() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(
        HOST_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--dev-world".to_string(),
            "--bind".to_string(),
            "127.0.0.1:0".to_string(),
        ],
    );
    let (addr, _fp, _) = listening(&host);
    let client = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--join".to_string(),
            addr.to_string(),
            "--ready".to_string(),
            "--headless".to_string(),
            "--frames".to_string(),
            "400".to_string(),
        ],
    );
    host.until("event=ready id=1 ready=true");
    host.cmd("start");
    host.until("event=started generation=1");
    host.cmd("cancel");
    host.until("event=cancelled generation=1");

    let rec = client.until("smoke=");
    assert!(
        rec.contains("mp=lobby(1p)"),
        "back in the lobby, roster of one: {rec}"
    );
    assert!(rec.contains("phase=menu"), "{rec}");
    assert!(
        rec.contains("status=pass") && rec.contains("returned to the lobby"),
        "a cancel is the lifecycle's end state, not a failure: {rec}"
    );
    assert!(client.wait().success());

    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// A host that goes away mid-lobby is reported, not sat on: the record
/// names the loss and the client exits nonzero.
#[test]
fn mm2_join_reports_a_lost_host() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(
        HOST_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--dev-world".to_string(),
            "--bind".to_string(),
            "127.0.0.1:0".to_string(),
        ],
    );
    let (addr, _fp, _) = listening(&host);
    let client = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            install.path().to_str().unwrap().to_string(),
            "--join".to_string(),
            addr.to_string(),
            "--ready".to_string(),
            "--headless".to_string(),
            "--frames".to_string(),
            "400".to_string(),
        ],
    );
    host.until("event=ready id=1 ready=true");
    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");

    let rec = client.until("smoke=");
    assert!(
        rec.contains("status=fail") && rec.contains("lost the host"),
        "a dead host is a named failure: {rec}"
    );
    assert_eq!(client.wait().code(), Some(3));
}

/// Join failures and usage conflicts are named exits: a refused
/// connection is exit 1, a session-shaping flag mixed with `--join`
/// is clap's exit 2 — never a silent local session.
#[test]
fn mm2_join_failures_and_conflicts_are_named_exits() {
    // Nothing listens — the handshake never begins.
    let dead: SocketAddr = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let client = Proc::spawn(
        MM2_EXE,
        &[
            "--join".to_string(),
            dead.to_string(),
            "--headless".to_string(),
        ],
    );
    assert_eq!(client.wait().code(), Some(1));

    for extra in [
        vec!["--city", "sf"],
        vec!["--dev-world"],
        vec!["--menu"],
        vec!["--event", "race:0"],
    ] {
        let mut args = vec!["--join".to_string(), "127.0.0.1:1".to_string()];
        args.extend(extra.iter().map(|s| s.to_string()));
        let client = Proc::spawn(MM2_EXE, &args);
        assert_eq!(
            client.wait().code(),
            Some(2),
            "--join + {extra:?} must be a usage error"
        );
    }
}

// ─── `mm2 --host` process legs (F24-B.8) ───────────────────────────
//
// Same loopback scope as the join legs: real child processes, real
// wire, real asset stack — the `listening=`/`event=` records are the
// same contract `mm2-host` prints.

/// Parse the `listening=` record an `mm2 --host` prints — unlike
/// `mm2-host` it follows the smoke header, so scan rather than taking
/// the first line.
fn host_addr(host: &Proc) -> SocketAddr {
    let line = host.until("listening=");
    line.strip_prefix("listening=")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("malformed listening record: {line:?}"))
        .parse()
        .unwrap()
}

/// The separate-process host leg: `mm2 --host` runs the lobby *and*
/// the host seat's local session — `mm2 --join` attaches, `start`
/// drives both ends, and the records carry the shared generation.
/// Loopback only — F24-C owns reachability.
#[test]
fn mm2_hosted_lobby_runs_the_in_app_session() {
    let install = tempfile::tempdir().unwrap();
    let path = install.path().to_str().unwrap().to_string();
    let mut host = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            path.clone(),
            "--dev-world".to_string(),
            "--host".to_string(),
            "--seed".to_string(),
            7.to_string(),
            "--headless".to_string(),
            "--frames".to_string(),
            5000.to_string(),
        ],
    );
    let addr = host_addr(&host);

    let client = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            path,
            "--join".to_string(),
            addr.to_string(),
            "--driver".to_string(),
            "carol".to_string(),
            "--ready".to_string(),
            "--headless".to_string(),
            "--frames".to_string(),
            600.to_string(),
        ],
    );

    // The `event=` contract is the dedicated host's — the joined peer,
    // its pick echo, its readiness.
    let joined = host.until("event=joined");
    assert!(joined.contains("driver=\"carol\""), "{joined}");
    host.until("event=ready id=1 ready=true");

    host.cmd("start");
    assert_eq!(host.until("event=started"), "event=started generation=1");

    // The joining client ran the wired session to its record.
    let rec = client.until("smoke=");
    assert!(rec.contains("world=dev-world"), "{rec}");
    assert!(rec.contains("mp=gen1"), "{rec}");
    assert!(rec.contains("status=pass"), "the wired session ran: {rec}");

    // Quit while the hosted session runs: the cancel is confirmed on
    // the wire's own contract before the sockets die, the local session
    // tears down to the lobby, and the run exits cleanly.
    host.cmd("quit");
    host.until("event=cancelled generation=1");
    let rec = host.until("smoke=");
    assert!(rec.contains("status=pass"), "{rec}");
    assert!(rec.contains("phase=menu"), "{rec}");
    assert!(
        rec.contains("mp=host("),
        "the parked record counts remote players only: {rec}"
    );
    assert!(host.wait().success(), "the hosted run did not exit 0");
    assert!(client.wait().success());
}

/// The wire's own start gate answers an operator `start` against an
/// unready roster — refused and named, and the host keeps running.
/// Then `quit` ends the host cleanly; the parked client reports the
/// loss as its named failure.
#[test]
fn mm2_hosted_lobby_refuses_a_start_against_an_unready_peer() {
    let install = tempfile::tempdir().unwrap();
    let path = install.path().to_str().unwrap().to_string();
    let mut host = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            path.clone(),
            "--dev-world".to_string(),
            "--host".to_string(),
            "--headless".to_string(),
            "--frames".to_string(),
            900.to_string(),
        ],
    );
    let addr = host_addr(&host);
    let client = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            path,
            "--join".to_string(),
            addr.to_string(),
            "--headless".to_string(),
            "--frames".to_string(),
            900.to_string(),
        ],
    );
    host.until("event=joined");

    host.cmd("start");
    let refused = host.until("event=start_refused");
    assert!(refused.contains("not ready"), "{refused}");

    host.cmd("quit");
    assert!(host.wait().success(), "the host did not exit cleanly");

    let rec = client.until("smoke=");
    assert!(
        rec.contains("status=fail") && rec.contains("lost the host"),
        "the parked client names the host loss: {rec}"
    );
    assert_eq!(client.wait().code(), Some(3));
}

/// `--host` flag-time gates are named exits: a dev-override config is
/// never advertised, `--join`/`--host` conflict, and `--bind`/`--seed`
/// are host flags — never a silent local session.
#[test]
fn mm2_host_flag_gates_are_named_exits() {
    let install = tempfile::tempdir().unwrap();
    let path = install.path().to_str().unwrap().to_string();

    // Dev overrides are never network-legal — the advertise check is
    // the flag-time gate.
    let host = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            path.clone(),
            "--dev-world".to_string(),
            "--host".to_string(),
            "--headless".to_string(),
            "--traction".to_string(),
            0.9.to_string(),
        ],
    );
    assert_eq!(host.wait().code(), Some(2));

    for extra in [
        vec!["--host", "--join", "127.0.0.1:1"],
        vec!["--host", "--menu"],
        vec!["--bind", "127.0.0.1:0"],
        vec!["--seed", "3"],
    ] {
        let mut args = vec![
            "--mm2-path".to_string(),
            path.clone(),
            "--dev-world".to_string(),
            "--headless".to_string(),
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        let proc = Proc::spawn(MM2_EXE, &args);
        assert_eq!(
            proc.wait().code(),
            Some(2),
            "{extra:?} must be a usage error"
        );
    }
}

/// `--cnr` flag-time gates are named exits (F27-B.4c): an unknown
/// variant/gold/limit, the option flags without `--cnr`, a mode that
/// conflicts (`--event`, `--dev-world`, `--join`) and a city whose site
/// pool cannot seed a round are usage errors — never an advertised
/// session or a silent cruise.
#[test]
fn cnr_flag_gates_are_named_exits() {
    let install = support::event_install();
    let path = install.path().to_str().unwrap().to_string();
    let base = |extra: &[&str]| {
        let mut args = vec![
            "--mm2-path".to_string(),
            path.clone(),
            "--headless".to_string(),
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        args
    };
    for extra in [
        // Not one of the host menu's choices.
        vec!["--city", "testcity", "--cnr", "tag"],
        vec!["--city", "testcity", "--cnr", "ffa", "--cnr-gold", "heavy"],
        vec!["--city", "testcity", "--cnr", "ffa", "--cnr-limit", "7m"],
        // Options without the mode they configure.
        vec!["--city", "testcity", "--cnr-gold", "half"],
        vec!["--city", "testcity", "--cnr-limit", "5m"],
        // One session has one mode, and the dev world has no site pool.
        vec![
            "--cnr",
            "ffa",
            "--event",
            "checkpoint:0",
            "--city",
            "testcity",
        ],
        vec!["--cnr", "ffa", "--dev-world"],
        vec!["--cnr", "ffa", "--join", "127.0.0.1:1"],
        // The hosted gate: the fixture city authors no site pool.
        vec!["--city", "testcity", "--cnr", "cops", "--host"],
    ] {
        let proc = Proc::spawn(MM2_EXE, &base(&extra));
        assert_eq!(
            proc.wait().code(),
            Some(2),
            "{extra:?} must be a usage error"
        );
    }
}

/// The offer reaches the wire: against the retail install (skipped
/// without `MM2_RETAIL`), `mm2 --host --cnr cops` advertises the mode —
/// the `listening=` record's session summary names it. The lobby is
/// then abandoned (killed): starting needs a ready client, and the
/// two-process match is a separate, still-open leg.
#[test]
fn mm2_host_cnr_advertises_the_mode() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host = Proc::spawn(
        MM2_EXE,
        &[
            "--mm2-path".to_string(),
            retail.to_str().unwrap().to_string(),
            "--host".to_string(),
            "--city".to_string(),
            "sf".to_string(),
            "--cnr".to_string(),
            "cops".to_string(),
            "--cnr-limit".to_string(),
            "250pts".to_string(),
            "--headless".to_string(),
            "--frames".to_string(),
            "100000".to_string(),
        ],
    );
    let rec = host.until("listening=");
    host.kill();
    assert!(rec.contains("cops & robbers, "), "{rec}");
}

// ─── F25-A: the session data plane ─────────────────────────────────
//
// Inputs up, host-side remote simulation, snapshots down — over the
// same loopback socket the lobby already owns. These legs prove the
// wire drives real entities (mailbox-fed `VehicleInput` on the host,
// snapshot-lerped kinematic copies on the client, epoch-declared
// resets reconciling either side); they do not claim full prediction
// replay, drift correction between epochs, or damage/result
// replication — the named gaps of this slice.

/// Drive a hosted session to `Playing`: the operator `start` mints the
/// generation (the peer is already ready), the begin lands `Loading`,
/// and the manual transitions stand it live — the load legs are the
/// headless/process runs.
fn hosted_playing(app: &mut App) -> u64 {
    app.world()
        .resource::<HostLink>()
        .command_sender()
        .send(HostCommand::Start)
        .unwrap();
    spin(app, |a| a.world().resource::<Session>().config().is_some());
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    app.update();
    generation
}

/// `spin` for predicates that need `world_mut` (a query state is a
/// mutable borrow) — same bounded contract.
fn spin_mut(app: &mut App, mut pred: impl FnMut(&mut App) -> bool) {
    for _ in 0..200 {
        app.update();
        if pred(app) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("the app never reached the expected state");
}

/// The host's half of the data plane: the peer's roster pick spawns a
/// real remote participant (`Remote` control, `Authority` role — the
/// host simulates its truth), wire `Input` frames land in the mailbox
/// and become its `VehicleInput`, staleness coasts it, and snapshots
/// broadcast every live update.
#[test]
fn a_remote_players_inputs_drive_the_hosted_car() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    let generation = hosted_playing(&mut app);

    // The reconcile spawned the peer's seat — remote-controlled but
    // locally authoritative: the host runs its physics.
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<(&NetPlayer, &Player, &mm2_game::AuthorityRole), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    {
        let mut q = app
            .world_mut()
            .query_filtered::<(&NetPlayer, &Player, &mm2_game::AuthorityRole), With<RemotePick>>();
        let (wire, player, role) = q.single(app.world()).expect("the remote car");
        assert_eq!(wire.0, 1, "the peer's wire slot");
        assert_eq!(player.control, PlayerControl::Remote);
        assert!(role.is_authority(), "the host owns a remote car's truth");
    }
    // F25-A.4: the authority's rule pipeline covers the remote car —
    // the designed recovery detector rides every remote seat. The
    // dev-car pick has no authored records, so `VehicleDamage`/
    // `VehicleStuck` stay absent — authored absence is never a
    // fabricated spec.
    {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        let remote = q.single(app.world()).expect("the remote car");
        assert!(
            app.world()
                .get::<mm2_game::VehicleRecovery>(remote)
                .is_some(),
            "the remote car carries the recovery detector"
        );
        assert!(
            app.world().get::<mm2_game::VehicleDamage>(remote).is_none()
                && app.world().get::<mm2_game::VehicleStuck>(remote).is_none(),
            "a recordless pick stays undamageable/unstuckable"
        );
    }
    // F25-A.2: seats [0, 1] — the peer's wire id 1 ranks to seat 1,
    // which a race-less dev world resolves one seat-gap right of the
    // roam base (yaw 0 → +X), not the old fixed lateral offset.
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&avian3d::prelude::Position, With<RemotePick>>();
        let pos = q.single(app.world()).expect("the remote car").0;
        assert!(
            (pos.x - 4.0).abs() < 0.5 && pos.z.abs() < 0.5,
            "seat 1 lands right of the roam base, got {pos:?}"
        );
    }
    assert_eq!(
        app.world().resource::<netdrive::NetDriveReport>().remotes,
        1
    );

    // A throttle sample up the wire becomes its settled `VehicleInput`.
    peer.ctl()
        .unwrap()
        .send_input(DriveInput {
            generation,
            seq: 1,
            throttle: 255,
            brake: 0,
            steer: -64,
            handbrake: 0,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .inputs_applied
            > 0
    });
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&VehicleInput, With<RemotePick>>();
        let input = q.single(app.world()).expect("the remote car's input");
        assert!(
            input.throttle > 0.9,
            "the wire throttle drove the car: {}",
            input.throttle
        );
        assert!(input.steering < -0.4, "the wire steer drove the car");
    }

    // The host publishes — the peer sees its own seat's snapshot.
    let snap = until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| e.player == 1)),
    );
    let Message::Snap {
        generation: sg,
        tick,
        ..
    } = snap
    else {
        unreachable!()
    };
    assert_eq!(sg, generation, "the snapshot rides this session");
    let _ = tick;
    assert!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_sent
            > 0
    );

    // F25-A.5: an authority reset on the remote seat bumps its wire
    // epoch — the next `Snap` declares the teleport instead of
    // presenting only a jumped pose. F25-A.6 strengthens the leg: the
    // harness schedules the real `vehicle_reset` apply, so the same
    // `Snap` that first carries `epoch == 1` must already carry the
    // teleported pose — the tracker runs after the apply, the publish
    // after the tracker.
    {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        let remote = q.single(app.world()).expect("the remote car");
        app.world_mut()
            .resource_mut::<Messages<ResetVehicle>>()
            .write(ResetVehicle {
                entity: Some(remote),
                position: Vec3::new(0.0, 1.5, 0.0),
                yaw: 0.0,
            });
    }
    app.update();
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&netdrive::ResetEpoch, With<RemotePick>>();
        assert_eq!(
            q.single(app.world()).expect("the remote car").0,
            1,
            "the authority reset bumped the seat's epoch"
        );
    }
    until_wire(&mut peer, |m| {
        matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| {
            e.player == 1 && e.epoch == 1 && e.pos == [0.0, 1.5, 0.0]
        }))
    });
    assert_eq!(
        app.world().resource::<netdrive::NetDriveReport>().resets,
        1,
        "the tracked reset counted once"
    );

    // F25-A.6: an Update-scheduled writer — the host's own `R` — goes
    // through the same ordering edge, so its teleport and epoch bump
    // leave on one `Snap` too rather than the epoch trailing a frame.
    // No `PlayerVehicle` exists in this harness, so the `R` bundle is
    // the reset-all form — the remote seat still teleports and bumps.
    app.world_mut()
        .resource_mut::<session::SpawnPoint>()
        .position = Vec3::new(7.0, 1.5, -3.0);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyR);
    app.update();
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .reset_all();
    {
        let mut q = app
            .world_mut()
            .query_filtered::<(&netdrive::ResetEpoch, &Transform), With<RemotePick>>();
        let (epoch, transform) = q.single(app.world()).expect("the remote car");
        assert_eq!(epoch.0, 2, "the R bundle bumped the seat epoch again");
        assert_eq!(
            transform.translation,
            Vec3::new(7.0, 1.5, -3.0),
            "vehicle_reset applied the R bundle to the remote seat"
        );
    }
    until_wire(&mut peer, |m| {
        matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| {
            e.player == 1 && e.epoch == 2 && e.pos == [7.0, 1.5, -3.0]
        }))
    });
    assert_eq!(
        app.world().resource::<netdrive::NetDriveReport>().resets,
        2,
        "the Update-writer reset tracked the same frame"
    );

    // Silence past INPUT_STALE zeroes the input — a stalled driver
    // coasts rather than keeping its last throttle.
    thread::sleep(netdrive::INPUT_STALE + Duration::from_millis(60));
    app.update();
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&VehicleInput, With<RemotePick>>();
        let input = q.single(app.world()).expect("the remote car's input");
        assert_eq!(input.throttle, 0.0, "a stale driver coasts");
    }
    assert!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .inputs_staled
            > 0
    );

    // And the departed peer's car despawns with its roster slot.
    peer.leave().unwrap();
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<(), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_none()
    });
    assert_eq!(
        app.world().resource::<netdrive::NetDriveReport>().despawned,
        1
    );
}

/// The client's half: the host seat (wire id 0 — carried by `Start`'s
/// `host_pick`, never a roster entry) spawns as a *predicted* kinematic
/// copy, the local car's input streams up, and snapshots blend the copy
/// toward the host's asserted pose. Stale and foreign-generation frames
/// drop.
#[test]
fn a_client_streams_inputs_and_applies_the_host_snapshot() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let our_id = link.player_id();
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    // The host pick reconciles into a predicted copy: Remote control,
    // Predicted role, kinematic body, and a lerp the snapshots drive.
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<&NetPlayer, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    {
        let mut q = app.world_mut().query_filtered::<(
            &NetPlayer,
            &Player,
            &mm2_game::AuthorityRole,
            &avian3d::prelude::RigidBody,
        ), With<RemotePick>>();
        let (wire, player, role, body) = q.single(app.world()).expect("the host copy");
        assert_eq!(wire.0, 0, "the host seat is wire id 0");
        assert_eq!(player.control, PlayerControl::Remote);
        assert!(
            !role.is_authority(),
            "a client never owns the host car's truth"
        );
        assert_eq!(*body, avian3d::prelude::RigidBody::Kinematic);
    }
    // F25-A.2's other half: seats [0, 1] — the host's wire id 0 ranks
    // to seat 0, the roam-base pose itself, and our own id 1 is not
    // re-spawned as a remote.
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&avian3d::prelude::Position, With<RemotePick>>();
        let pos = q.single(app.world()).expect("the host copy").0;
        assert!(
            pos.x.abs() < 0.5 && pos.z.abs() < 0.5,
            "seat 0 lands on the roam base, got {pos:?}"
        );
    }

    // The local car's settled input rides up — the host's mailbox sees
    // this seat's wire id with this session's generation. It carries
    // the rigid row so its snapshot entry has a predicted pose to
    // reconcile (F25-A.5) and a `VehicleDamage` for the v8 byte to land
    // on — a dev car never grows one, so the spec is bound by hand.
    let local = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            Player {
                id: mm2_game::PlayerId(1),
                control: PlayerControl::Local,
            },
            mm2_game::AuthorityRole::Predicted,
            VehicleInput {
                throttle: 0.5,
                ..VehicleInput::default()
            },
            mm2_game::VehicleDamage::new(DAMAGE_SPEC),
            avian3d::prelude::Position::default(),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
            // The v16 sentinel: a live sim resolves its own
            // `SurfaceContact` — a snap row's junk tail must never
            // overwrite it like it never overwrites the input.
            mm2_app::audio::SurfaceContact {
                skid: Some(mm2_app::audio::SkidContact {
                    surface: 5,
                    slippage: 0.5,
                    wheel_speed: 9.0,
                }),
                roll: Some(7),
            },
        ))
        .id();
    // Same for the host copy — a dev-car pick binds no authored damage
    // record, so the replication target is attached by hand.
    {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        let copy = q.single(app.world()).expect("the host copy");
        app.world_mut()
            .entity_mut(copy)
            .insert(mm2_game::VehicleDamage::new(DAMAGE_SPEC));
    }
    spin(&mut app, |a| {
        a.world().resource::<netdrive::NetDriveReport>().inputs_sent > 0
    });
    let sent = host
        .remote_inputs()
        .latest(our_id)
        .expect("the mailbox saw our input");
    assert_eq!(sent.input.generation, generation);
    assert!(
        (sent.input.throttle as f32 / 255.0 - 0.5).abs() < 0.01,
        "the quantized throttle round-trips: {}",
        sent.input.throttle
    );

    // A snapshot for seat 0 retargets the copy's lerp; the lerp drives
    // its `Position` toward the asserted pose.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 7,
            entries: vec![
                SnapEntry {
                    player: 0,
                    pos: [9.0, 1.0, 9.0],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [1.0, 0.0, 0.0],
                    angvel: [0.0, 0.0, 0.0],
                    epoch: 0,
                    // The v7 presentation tail (F25-B): the host seat
                    // steered 0.25 rad, its wheels roll at 30 rad/s
                    // grounded at 0.4 travel, brake held.
                    steer: 250,
                    spin: 300,
                    compression: 102,
                    flags: mm2_net::SNAP_FLAG_BRAKE | mm2_net::SNAP_FLAG_GROUNDED,
                    // v8: the host copy is half-wrecked.
                    damage: 128,
                    breaks: 0,
                    // v15: the authority's engine is pulling 4321 rpm.
                    rpm: 4321,
                    ..SnapEntry::default()
                },
                // Our own seat's entry is received and, epoch-equal,
                // skipped — between authority resets the local sim
                // owns the pose (F25-A.5). Its presentation fields are
                // junk on purpose: a remote entry's drive fields must
                // never overwrite the local car's live state. The v8
                // damage byte is the deliberate exception — it is the
                // *only* own-seat field the snap applies, since under
                // prediction nothing local ever accumulates damage.
                SnapEntry {
                    player: our_id,
                    pos: [-50.0, 0.0, -50.0],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [0.0; 3],
                    angvel: [0.0; 3],
                    epoch: 0,
                    steer: i16::MAX,
                    spin: i16::MIN,
                    compression: 255,
                    flags: mm2_net::SNAP_FLAG_BRAKE | mm2_net::SNAP_FLAG_REVERSE,
                    damage: 200,
                    breaks: 0,
                    rpm: u16::MAX,
                    surf_skid: 9,
                    skid_slip: 255,
                    skid_speed: i16::MAX,
                    surf_roll: 9,
                    ..SnapEntry::default()
                },
            ],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > 0
    });
    // The lerp's first blend interval is zero — the next lerp update
    // lands the copy on the asserted pose.
    app.update();
    {
        let mut q = app.world_mut().query_filtered::<(
            &netdrive::RemoteLerp,
            &avian3d::prelude::Position,
        ), With<RemotePick>>();
        let (lerp, pos) = q.single(app.world()).expect("the host copy");
        assert!(
            (lerp.to_pos - Vec3::new(9.0, 1.0, 9.0)).length() < 1e-3,
            "the lerp targets the asserted pose: {:?}",
            lerp.to_pos
        );
        assert!(
            (pos.0 - Vec3::new(9.0, 1.0, 9.0)).length() < 1e-3,
            "the copy blends to the asserted pose: {:?}",
            pos.0
        );
    }
    // The v7 presentation tail landed on the copy — steer angle,
    // grounded compression on every wheel, the brake pedal the glow
    // system reads, and the wheel rate `drive_remote_lerp` integrates.
    // (`RemoteReplica` keeps the local sim from stepping this state.)
    {
        let mut q = app.world_mut().query_filtered::<(
            &mm2_vehicle::VehicleState,
            &VehicleInput,
            &netdrive::RemoteDrive,
        ), With<RemotePick>>();
        let (state, input, drive) = q.single(app.world()).expect("the host copy");
        assert!(
            (state.steer_angle - 0.25).abs() < 1e-3,
            "the replicated steer angle: {}",
            state.steer_angle
        );
        assert!(state.grounded);
        assert!(
            state
                .wheels
                .iter()
                .all(|w| w.grounded && w.compression > 0.0),
            "the replicated droop reached every wheel"
        );
        assert_eq!(input.brake, 1.0, "the brake flag drives the glows");
        assert_eq!(drive.spin_rate, 30.0, "300 x 0.1 rad/s");
        assert_eq!(
            state.rpm, 4321.0,
            "the v15 tail lands the rpm the copy's engine rig mixes"
        );
    }
    // The v8 damage byte landed too — 128/255 of the authored
    // `MaxDamage` on the copy, 200/255 on our own seat: under a
    // predicted session the replicated total is the only writer both
    // sides' `VehicleDamage` ever sees.
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&mm2_game::VehicleDamage, With<RemotePick>>();
        let damage = q.single(app.world()).expect("the host copy");
        assert!(
            (damage.total() - 128.0 / 255.0 * DAMAGE_SPEC.max_damage).abs() < 1.0,
            "the replicated fraction reconstituted the copy's total: {}",
            damage.total()
        );
        assert_eq!(damage.condition(), mm2_game::DamageTier::Damaged);
        let own = app.world().get::<mm2_game::VehicleDamage>(local).unwrap();
        assert!(
            (own.total() - 200.0 / 255.0 * DAMAGE_SPEC.max_damage).abs() < 1.0,
            "the own seat's replicated total is the meter's truth: {}",
            own.total()
        );
        assert!(
            app.world()
                .resource::<netdrive::NetDriveReport>()
                .damage_synced
                >= 2,
            "both seats' replicated writes counted"
        );
    }
    // The rate integrates into the copy's wheel spin each update — the
    // visuals' `WheelState::spin` accumulates like a live car's.
    // (`spin`, not fixed updates — a 0 delta step adds nothing.)
    spin(&mut app, |a| {
        a.world().resource::<netdrive::NetDriveReport>().remote_spin > 0.0
    });
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&mm2_vehicle::VehicleState, With<RemotePick>>();
        let state = q.single(app.world()).expect("the host copy");
        assert!(
            state.wheels.iter().all(|w| w.spin > 0.0),
            "the replicated rate turned the copy's wheels"
        );
    }

    // The local seat was not moved by its own entry — and its live
    // input kept its own pedal, never the entry's brake flag.
    assert!(
        app.world().get::<PlayerVehicle>(local).is_some(),
        "the local car stayed ours"
    );
    assert_eq!(
        app.world().get::<VehicleInput>(local).unwrap().brake,
        0.0,
        "the own-seat tail never overwrote the local input"
    );
    {
        let contact = app
            .world()
            .get::<mm2_app::audio::SurfaceContact>(local)
            .expect("the own seat's resolved contact");
        assert_eq!(
            contact.skid,
            Some(mm2_app::audio::SkidContact {
                surface: 5,
                slippage: 0.5,
                wheel_speed: 9.0,
            }),
            "the own-seat junk tail never touched the local contact"
        );
        assert_eq!(contact.roll, Some(7));
    }

    // A stale tick and a foreign generation both drop untouched.
    for tick in [3u64, 7] {
        host.ctl()
            .broadcast(&Message::Snap {
                impacts: Vec::new(),
                race: None,
                trailers: Vec::new(),
                generation,
                tick,
                entries: vec![SnapEntry {
                    player: 0,
                    pos: [0.0; 3],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [0.0; 3],
                    angvel: [0.0; 3],
                    epoch: 0,
                    steer: 0,
                    spin: 0,
                    compression: 0,
                    flags: 0,
                    damage: 0,
                    breaks: 0,
                    ..SnapEntry::default()
                }],
            })
            .unwrap();
    }
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation: generation + 9,
            tick: 99,
            entries: vec![SnapEntry {
                player: 0,
                pos: [0.0; 3],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                epoch: 0,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    // Let the frames drain — each `update` consumes the newest staged
    // snap, so spin until the report is settled.
    for _ in 0..5 {
        app.update();
    }
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&netdrive::RemoteLerp, With<RemotePick>>();
        let lerp = q.single(app.world()).expect("the host copy");
        assert!(
            (lerp.to_pos - Vec3::new(9.0, 1.0, 9.0)).length() < 1e-3,
            "stale/foreign snaps never retargeted the lerp"
        );
    }

    // F25-A.4: a teleport-scale correction snaps the copy to the
    // asserted pose at once — the host's reset/recovery outcomes move
    // a remote car past the blend bound, and a copy must not smear
    // through the world between the poses.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 8,
            entries: vec![SnapEntry {
                player: 0,
                pos: [80.0, 1.0, 80.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                epoch: 0,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            >= 2
    });
    {
        let mut q = app.world_mut().query_filtered::<(
            &netdrive::RemoteLerp,
            &avian3d::prelude::Position,
        ), With<RemotePick>>();
        let (lerp, pos) = q.single(app.world()).expect("the host copy");
        let target = Vec3::new(80.0, 1.0, 80.0);
        assert!(
            (pos.0 - target).length() < 1e-3,
            "a teleport correction lands at once, got {:?}",
            pos.0
        );
        assert!(
            (lerp.from_pos - target).length() < 1e-3 && (lerp.to_pos - target).length() < 1e-3,
            "the collapsed blend holds the landing"
        );
    }

    // A sub-bound correction still blends — ordinary motion never
    // snaps (bounded corrections cut the other way too).
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 9,
            entries: vec![SnapEntry {
                player: 0,
                pos: [84.0, 1.0, 80.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                epoch: 0,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            >= 3
    });
    {
        let mut q = app.world_mut().query_filtered::<(
            &netdrive::RemoteLerp,
            &avian3d::prelude::Position,
        ), With<RemotePick>>();
        let (lerp, pos) = q.single(app.world()).expect("the host copy");
        assert!(
            (lerp.to_pos - Vec3::new(84.0, 1.0, 80.0)).length() < 1e-3,
            "the blend still targets the asserted pose"
        );
        assert!(
            (lerp.from_pos - Vec3::new(80.0, 1.0, 80.0)).length() < 1e-3,
            "the blend restarts from the displayed pose: {:?}",
            lerp.from_pos
        );
        assert!(
            pos.0.x < 83.5,
            "a 4 m correction blends instead of snapping: {:?}",
            pos.0
        );
    }

    // F25-A.5: an epoch advance on *our own* seat is the authority
    // saying "I teleported your car" — the predicted pose yields
    // outright (position, rotation and velocities), marked `Teleported`
    // so swept-segment consumers re-anchor on the jump.
    spin(&mut app, |a| {
        a.world().get::<netdrive::ResetEpoch>(local).is_some()
    });
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 10,
            entries: vec![SnapEntry {
                player: our_id,
                pos: [-50.0, 0.0, -50.0],
                rot: [
                    0.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                    0.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                ],
                vel: [2.0, 0.0, 0.0],
                angvel: [0.0; 3],
                epoch: 1,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<netdrive::NetDriveReport>().resets >= 1
    });
    {
        let pos = app
            .world()
            .get::<avian3d::prelude::Position>(local)
            .unwrap()
            .0;
        assert!(
            (pos - Vec3::new(-50.0, 0.0, -50.0)).length() < 1e-3,
            "the authority's reset moved our own car: {pos:?}"
        );
        let vel = app
            .world()
            .get::<avian3d::prelude::LinearVelocity>(local)
            .unwrap()
            .0;
        assert!(
            (vel - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-3,
            "the asserted velocity lands too: {vel:?}"
        );
        assert_eq!(
            app.world().get::<netdrive::ResetEpoch>(local).unwrap().0,
            1,
            "the applied epoch is stamped on the seat"
        );
        assert!(
            app.world().get::<Teleported>(local).is_some(),
            "a reconciled teleport is marked so swept consumers re-anchor"
        );
    }

    // An epoch-equal entry naming our seat is ignored — between
    // authority resets the local sim owns the pose; a host copy that
    // merely lags must never rubber-band the driver.
    let applied = app
        .world()
        .resource::<netdrive::NetDriveReport>()
        .snaps_applied;
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 11,
            entries: vec![SnapEntry {
                player: our_id,
                pos: [999.0, 0.0, 999.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                epoch: 1,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > applied
    });
    {
        let pos = app
            .world()
            .get::<avian3d::prelude::Position>(local)
            .unwrap()
            .0;
        assert!(
            (pos - Vec3::new(-50.0, 0.0, -50.0)).length() < 1e-3,
            "an epoch-equal entry must not rubber-band the own seat: {pos:?}"
        );
    }

    // And the declared reset beats the blend bound on remote copies
    // too: an epoch bump snaps a sub-`CORRECTION_SNAP_DIST` correction
    // — an in-place wreck resolve is a teleport even when it lands on
    // the same spot.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 12,
            entries: vec![SnapEntry {
                player: 0,
                pos: [85.0, 1.0, 80.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                epoch: 1,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<netdrive::NetDriveReport>().resets >= 2
    });
    {
        let mut q = app.world_mut().query_filtered::<(
            &netdrive::RemoteLerp,
            &avian3d::prelude::Position,
        ), With<RemotePick>>();
        let (lerp, pos) = q.single(app.world()).expect("the host copy");
        let target = Vec3::new(85.0, 1.0, 80.0);
        assert!(
            (pos.0 - target).length() < 1e-3,
            "an epoch-declared reset snaps under the blend bound: {:?}",
            pos.0
        );
        assert!(
            (lerp.from_pos - target).length() < 1e-3 && (lerp.to_pos - target).length() < 1e-3,
            "the collapsed blend holds the landing"
        );
    }

    // Replication lowers the total too — the later snaps all carried
    // `damage: 0`, so both seats reconstituted the authority's repair
    // back to intact (a wrecked-then-reset seat is exactly this path).
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&mm2_game::VehicleDamage, With<RemotePick>>();
        let copy = q.single(app.world()).expect("the host copy");
        assert_eq!(
            copy.total(),
            0.0,
            "the copy's replicated total followed the authority back to intact"
        );
        let own = app.world().get::<mm2_game::VehicleDamage>(local).unwrap();
        assert_eq!(own.total(), 0.0, "the own seat's total repaired too");
    }

    host.shutdown();
}

/// F25-B (protocol v15), authority half: a remote seat whose pick
/// carries authored cardata binds `VehicleAudio` like a local or
/// opponent spawn — the component `engine_rigs` voices — and the
/// seat's live `VehicleState::rpm` publishes in its `SnapEntry` so a
/// client's copy can mix the same voice. Horn and clutch stay
/// local-owner behavior (`PlayerVehicle`/`clutch_voices` never voice a
/// `Remote` seat); surface loops stay unsupported — the wire carries
/// no wheel-contact truth.
#[test]
fn an_authored_remote_picks_engine_voice_publishes_its_rpm() {
    let install = tempfile::tempdir().unwrap();
    support::audio_car(install.path(), "vpt");
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    // The peer picks the authored car — `ready_peer`'s discipline with
    // `vpt` riding the roster instead of the dev car.
    let mut peer = remote_peer(addr, "eve", fp);
    let ctl = peer.ctl().unwrap();
    ctl.set_vehicle("vpt", 0).unwrap();
    ctl.set_ready(true).unwrap();
    until_wire(
        &mut peer,
        |m| matches!(m, Message::Roster { players: r } if r.iter().any(|e| e.driver == "eve" && e.ready && e.pick.as_ref().is_some_and(|p| p.vehicle == "vpt"))),
    );
    hosted_playing(&mut app);
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let remote = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the remote car")
    };
    // The cardata bound like a local or opponent spawn's — the
    // fixture's one engine row proves the authored table rode the
    // pick, not a fabricated spec.
    let audio = app
        .world()
        .get::<mm2_game::VehicleAudio>(remote)
        .expect("the authored cardata bound on the remote seat");
    assert_eq!(
        audio.spec.engine_samples.len(),
        1,
        "the authored engine rows rode the pick"
    );

    // The seat's live rpm publishes — quantized to whole revolutions.
    // Earlier snaps (idle rpm or zero) drain past the predicate.
    {
        let mut entity = app.world_mut().entity_mut(remote);
        let mut state = entity
            .get_mut::<mm2_vehicle::VehicleState>()
            .expect("the remote seat simulates");
        state.rpm = 4321.6;
    }
    app.update();
    let snap = until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| e.player == 1 && e.rpm == 4322)),
    );
    let Message::Snap { entries, .. } = snap else {
        unreachable!("the predicate matched the rpm-bearing row")
    };
    let entry = entries
        .iter()
        .find(|e| e.player == 1)
        .expect("the remote seat's row");
    assert_eq!(
        entry.rpm, 4322,
        "the live rpm publishes rounded to whole revolutions"
    );
}

/// F25-B (protocol v15), client half: an authored host pick's cardata
/// binds `VehicleAudio` on the predicted copy the same way — its
/// `EngineVoice` rig then mixes off the `SnapEntry.rpm` tail
/// `apply_present` writes.
#[test]
fn a_remote_copys_engine_voice_mixes_off_the_replicated_rpm() {
    let install = tempfile::tempdir().unwrap();
    support::audio_car(install.path(), "vpt");
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: "vpt".to_string(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    // The authored host pick reconciles into a predicted copy whose
    // cardata bound — while the seat stays `Remote` and un-`PlayerVehicle`d,
    // so horn/clutch remain the owning process's business.
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let copy = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the host copy")
    };
    let audio = app
        .world()
        .get::<mm2_game::VehicleAudio>(copy)
        .expect("the authored cardata bound on the copy");
    assert_eq!(audio.spec.engine_samples.len(), 1);
    assert!(
        app.world().get::<PlayerVehicle>(copy).is_none(),
        "the copy is no horn/clutch owner"
    );

    // A snapshot's v15 tail feeds the copy's `VehicleState::rpm` — the
    // field `engine_drive` mixes the rig from.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 7,
            entries: vec![SnapEntry {
                player: 0,
                pos: [9.0, 1.0, 9.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                epoch: 0,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                rpm: 4321,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > 0
    });
    let state = app
        .world()
        .get::<mm2_vehicle::VehicleState>(copy)
        .expect("the copy's vehicle state");
    assert_eq!(
        state.rpm, 4321.0,
        "the replicated rpm is what the copy's engine rig reads"
    );

    host.shutdown();
}

/// `_default` → row 0, `grass` → row 1 — the `sound` class wiring
/// `SurfaceTables::sound_index` reads (the same two-material set
/// `tests/audio.rs`'s surface legs stage).
fn surface_tables() -> SurfaceTables {
    let set = MaterialSet::parse("mtl _default {\n    sound: 0\n}\nmtl grass {\n    sound: 1\n}\n")
        .unwrap();
    let map = MaterialMap::parse("texture,physics\n").unwrap();
    SurfaceTables { set, map }
}

/// F25-B (protocol v16), authority half: a wire seat's live wheel
/// contact resolves through `surface_voices` — the collider's
/// `SurfaceMaterial` → the material's authored `sound` class → the
/// session's `SurfaceAudio` row — and the `SurfaceContact` it records
/// publishes in the seat's `SnapEntry` tail the same update. Nothing
/// about the row is staged: the test writes only the wheel telemetry
/// the sim owns (`grounded`/`contact_entity`/`traction_demand`/
/// `vel_long`/`forward_speed`).
#[test]
fn a_wire_seats_surface_contact_publishes_in_its_snap() {
    let install = tempfile::tempdir().unwrap();
    support::audio_car(install.path(), "vpt");
    support::surface_audio(install.path());
    let config = dev_cruise();
    let (link, vfs, fp) = host_link(install.path(), &config);
    // The session resources `load_session_world` binds — the authored
    // dry table off this install's VFS and the wave bank indexing it.
    // This leg builds the app by hand rather than loading a session
    // world, so `SurfaceTables` is the test's own two-material set,
    // the same staging `audio.rs` uses (`support::surface_materials`
    // is what the session path would mount for this install).
    let audio = mm2_app::audio::SurfaceAudio::load(&vfs, config.conditions.weather, None)
        .expect("the fixture's dry table resolves");
    let bank = mm2_app::audio::WaveBank::index(&vfs);
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    app.insert_resource(audio)
        .insert_resource(surface_tables())
        .insert_resource(bank);
    let mut peer = remote_peer(addr, "eve", fp);
    let ctl = peer.ctl().unwrap();
    ctl.set_vehicle("vpt", 0).unwrap();
    ctl.set_ready(true).unwrap();
    until_wire(
        &mut peer,
        |m| matches!(m, Message::Roster { players: r } if r.iter().any(|e| e.driver == "eve" && e.ready && e.pick.as_ref().is_some_and(|p| p.vehicle == "vpt"))),
    );
    hosted_playing(&mut app);
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let remote = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the remote car")
    };
    // One more pass so `surface_voices` has provably resolved the seat —
    // its spawn lands mid-schedule, so this update is the first the
    // wheel loop can see it.
    app.update();
    assert!(
        app.world()
            .get::<mm2_app::audio::SurfaceContact>(remote)
            .is_some_and(|c| c.skid.is_none() && c.roll.is_none()),
        "airborne wheels resolve the empty contact — sentinel rows only"
    );

    // A grass slide: every wheel grounded on the class-1 collider and
    // 0.6 of the way past its longitudinal limit, wheel speed and
    // forward speed both 12.4 m/s so the rolling gate stays open.
    let grass = app.world_mut().spawn(SurfaceMaterial::Authored(1)).id();
    let config = &app
        .world()
        .get::<mm2_vehicle::vehicle::Vehicle>(remote)
        .unwrap()
        .config;
    let ratios: Vec<f32> = config
        .wheels
        .iter()
        .map(|w| w.tires.as_ref().unwrap_or(&config.tires).peak_slip_ratio)
        .collect();
    {
        let mut state = app.world_mut().get_mut::<VehicleState>(remote).unwrap();
        state.forward_speed = 12.4;
        for (w, ratio) in state.wheels.iter_mut().zip(ratios) {
            w.grounded = true;
            w.contact_entity = Some(grass);
            w.traction_demand = 1.0 + 0.6 * ratio;
            w.vel_long = 12.4;
        }
    }
    app.update();

    // `surface_voices`' live resolve wrote the contact — the record the
    // publish encodes, asserted here before the wire leg so a staged
    // row can never masquerade as a resolved one.
    let contact = app
        .world()
        .get::<mm2_app::audio::SurfaceContact>(remote)
        .expect("the resolve wrote the seat's contact");
    let skid = contact.skid.expect("the grass slide resolved a skid");
    assert_eq!(skid.surface, 1, "the collider's authored sound class");
    assert!((skid.slippage - 0.6).abs() < 1e-5, "{}", skid.slippage);
    assert_eq!(skid.wheel_speed, 12.4);
    assert_eq!(contact.roll, Some(1), "the moving car's rolling class");

    let snap = until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| e.player == 1 && e.surf_skid == 1)),
    );
    let Message::Snap { entries, .. } = snap else {
        unreachable!("the predicate matched the contact-bearing row")
    };
    let entry = entries
        .iter()
        .find(|e| e.player == 1)
        .expect("the remote seat's row");
    assert_eq!(
        (
            entry.surf_skid,
            entry.skid_slip,
            entry.skid_speed,
            entry.surf_roll
        ),
        (1, 153, 124, 1),
        "the resolved contact publishes quantized — 0.6×255→153, \
         12.4 m/s→124"
    );
    assert!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .surfaces_sent
            > 0,
        "the contact-bearing rows count on the authority"
    );

    // The same live chain keeps voicing: the resolved contact spawned
    // the grass skid band plus the rolling loop off the authored
    // waves — spatial emitters like any non-player car.
    spin(&mut app, |a| {
        let r = a.world().resource::<mm2_app::audio::AudioReport>();
        r.skids == 1 && r.rolling == 1
    });
    assert_eq!(
        app.world().resource::<mm2_app::audio::AudioReport>().failed,
        0,
        "every resolved voice decoded its authored wave"
    );

    // Airborne: the next resolve writes the empty contact and the wire
    // falls back to its sentinels — never a stale row.
    {
        let mut state = app.world_mut().get_mut::<VehicleState>(remote).unwrap();
        state.forward_speed = 0.0;
        for w in &mut state.wheels {
            w.grounded = false;
            w.contact_entity = None;
            w.traction_demand = 0.0;
            w.vel_long = 0.0;
        }
    }
    app.update();
    until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| e.player == 1 && e.surf_skid == SNAP_NO_SURFACE && e.surf_roll == SNAP_NO_SURFACE)),
    );
}

/// F25-B (protocol v16), client half: a remote copy's `SurfaceContact`
/// decodes off the `SnapEntry` tail — the component `surface_voices`
/// replays through this process's own `SurfaceAudio` — while the
/// rolling mix's forward speed derives from the wire velocity, not a
/// dedicated field.
#[test]
fn a_remote_copy_replays_the_replicated_surface_contact() {
    let install = tempfile::tempdir().unwrap();
    support::audio_car(install.path(), "vpt");
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: "vpt".to_string(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let copy = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the host copy")
    };
    assert!(
        app.world()
            .get::<mm2_app::audio::SurfaceContact>(copy)
            .is_some_and(|c| c.skid.is_none() && c.roll.is_none()),
        "the copy spawned with an empty contact"
    );

    // The v16 tail: a slide on class 2 at 0.5 slippage with the wheel
    // still spinning -12.4 m/s, class 2 rolling underneath; the pose
    // asserts 4 m/s forward (vel · the rot frame's -Z).
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 7,
            entries: vec![SnapEntry {
                player: 0,
                pos: [9.0, 1.0, 9.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0, 0.0, -4.0],
                angvel: [0.0; 3],
                epoch: 0,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                surf_skid: 2,
                skid_slip: 128,
                skid_speed: -124,
                surf_roll: 2,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > 0
    });
    let contact = app
        .world()
        .get::<mm2_app::audio::SurfaceContact>(copy)
        .expect("the copy's contact");
    let skid = contact.skid.expect("the replicated skid contact");
    assert_eq!(skid.surface, 2);
    assert!(
        (skid.slippage - 128.0 / 255.0).abs() < 1e-6,
        "the slip dequantizes: {}",
        skid.slippage
    );
    assert_eq!(skid.wheel_speed, -12.4, "the wheel speed dequantizes");
    assert_eq!(contact.roll, Some(2));
    let state = app
        .world()
        .get::<mm2_vehicle::VehicleState>(copy)
        .expect("the copy's vehicle state");
    assert_eq!(
        state.forward_speed, 4.0,
        "the rolling mix's speed derives off the wire velocity"
    );
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .surfaces_applied,
        1,
        "the contact-bearing row counted on the client"
    );

    // A quiet frame clears both halves — no stale contact replays.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            trailers: Vec::new(),
            generation,
            tick: 8,
            entries: vec![SnapEntry {
                player: 0,
                pos: [9.0, 1.0, 9.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                epoch: 0,
                steer: 0,
                spin: 0,
                compression: 0,
                flags: 0,
                damage: 0,
                breaks: 0,
                ..SnapEntry::default()
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            >= 2
    });
    let contact = app
        .world()
        .get::<mm2_app::audio::SurfaceContact>(copy)
        .expect("the copy's contact");
    assert!(
        contact.skid.is_none() && contact.roll.is_none(),
        "sentinel fields clear the copy's contact"
    );
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .surfaces_applied,
        1,
        "sentinel rows land silently — the counter only counts contacts"
    );

    host.shutdown();
}

/// F25-B, host half: a remote driver's reset request drains into the
/// shared `ResetVehicle` path — the requesting seat teleports back to
/// its grid slot and the bumped epoch declares it on the same `Snap`.
/// Out-of-phase asks, foreign generations and cooldown repeats drop.
#[test]
fn a_remote_drivers_reset_request_resets_its_seat() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });

    // `Start` mints the generation; an ask drained before the session
    // stands `Playing` drops — it never queues for later.
    app.world()
        .resource::<HostLink>()
        .command_sender()
        .send(HostCommand::Start)
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<Session>().config().is_some()
    });
    let generation = app.world().resource::<Session>().wire_generation();
    peer.ctl().unwrap().request_reset(generation).unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .requests_dropped
            == 1
    });

    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    app.update();
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<(), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    // The reconcile's seat pose is the ask's landing target — record it
    // rather than recomputing the hull lift.
    let seated = {
        let mut q = app
            .world_mut()
            .query_filtered::<&avian3d::prelude::Position, With<RemotePick>>();
        q.single(app.world()).expect("the remote car").0
    };

    // Move the authority pose off-seat — the harness has no physics, so
    // the write *is* the settled truth — then the driver asks.
    {
        let mut q = app
            .world_mut()
            .query_filtered::<(&mut avian3d::prelude::Position, &mut Transform), With<RemotePick>>(
            );
        let (mut pos, mut transform) = q.single_mut(app.world_mut()).expect("the remote car");
        pos.0 = Vec3::new(30.0, 1.5, -8.0);
        transform.translation = pos.0;
    }
    peer.ctl().unwrap().request_reset(generation).unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .requests_granted
            == 1
    });
    {
        let mut q = app
            .world_mut()
            .query_filtered::<(Entity, &netdrive::ResetEpoch, &Transform), With<RemotePick>>();
        let (remote, epoch, transform) = q.single(app.world()).expect("the remote car");
        assert_eq!(epoch.0, 1, "the granted ask bumped the seat's epoch");
        assert!(
            (transform.translation - seated).length() < 0.5,
            "the reset landed back on the seat: {:?} vs {seated:?}",
            transform.translation
        );
        assert!(
            app.world().get::<Teleported>(remote).is_some(),
            "the production apply stamped the teleport"
        );
    }
    // The same `Snap` that first carries `epoch == 1` already carries
    // the seat pose — the request writer runs before `vehicle_reset`,
    // the tracker after it, the publish after the tracker.
    until_wire(&mut peer, |m| {
        matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| {
            e.player == 1
                && e.epoch == 1
                && (Vec3::from_array(e.pos) - seated).length() < 0.5
        }))
    });

    // A repeat inside the designed cooldown drops rather than stacking
    // a second teleport, and a foreign generation is refused too.
    peer.ctl().unwrap().request_reset(generation).unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .requests_dropped
            == 2
    });
    peer.ctl().unwrap().request_reset(generation + 9).unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .requests_dropped
            == 3
    });
    {
        let mut q = app
            .world_mut()
            .query_filtered::<(&netdrive::ResetEpoch, &Transform), With<RemotePick>>();
        let (epoch, transform) = q.single(app.world()).expect("the remote car");
        assert_eq!(epoch.0, 1, "the drops left the seat untouched");
        assert!(
            (transform.translation - seated).length() < 0.5,
            "the car stayed on its seat"
        );
    }
}

/// F25-B, host half of the v7 presentation tail: `publish_snapshots`
/// encodes the seat's *drive state* — the peer's wire `Snap` carries
/// the steer angle, mean wheel rate, droop fraction and pedal/
/// direction flags the copy's visuals consume, quantized exactly the
/// way `encode_present` specifies.
#[test]
fn a_snap_carries_the_remote_cars_drive_state() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    app.world()
        .resource::<HostLink>()
        .command_sender()
        .send(HostCommand::Start)
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<Session>().config().is_some()
    });
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    app.update();
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<(), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });

    // The harness has no physics — hand-write the authority's truth
    // the way the sim leaves it: steered 0.25 rad, reversing, fronts
    // grounded at 20 rad/s and 0.4 of travel. The brake pedal comes
    // up the wire — `apply_remote_inputs` owns the seat's `VehicleInput`.
    peer.ctl()
        .unwrap()
        .send_input(DriveInput {
            generation: app.world().resource::<Session>().wire_generation(),
            seq: 1,
            throttle: 0,
            brake: 255,
            steer: 0,
            handbrake: 0,
        })
        .unwrap();
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<&VehicleInput, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some_and(|i| i.brake > 0.9)
    });
    {
        let mut q = app
            .world_mut()
            .query_filtered::<&mut mm2_vehicle::VehicleState, With<RemotePick>>();
        let mut state = q.single_mut(app.world_mut()).expect("the remote car");
        state.steer_angle = 0.25;
        state.direction = mm2_vehicle::DriveDirection::Reverse;
        state.grounded = true;
        for ws in &mut state.wheels[..2] {
            ws.grounded = true;
            ws.vel_long = 6.8;
            ws.compression = 0.14;
        }
    }
    // The v8 damage byte (F25-B): the authority's accumulated total
    // as a fraction of the seat's authored `MaxDamage` — the dev-car
    // pick binds no record, so the spec attaches by hand and half of
    // `max_damage` accumulates through the real `apply` path.
    let remote = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the remote car")
    };
    {
        let mut damage = mm2_game::VehicleDamage::new(DAMAGE_SPEC);
        damage.apply(mm2_game::ImpactId(1), DAMAGE_SPEC.max_damage * 0.5);
        app.world_mut().entity_mut(remote).insert(damage);
    }
    // The v9 trailer row (F25-B): a trailer towing the seat publishes
    // under the seat's wire id — hand-spawned the way `spawn_trailer`
    // leaves an authority-side trailer (the dev-car pick tows nothing,
    // so the rig is declared by hand), wheels grounded at 20 rad/s.
    let trailer = app
        .world_mut()
        .spawn((
            mm2_app::car_visual::Trailer {
                towing: remote,
                rest_offset: Vec3::new(0.0, -0.5, 4.0),
            },
            mm2_vehicle::vehicle_bundle(&VehicleConfig::default()),
            avian3d::prelude::Position(Vec3::new(3.0, 0.6, -7.0)),
            avian3d::prelude::Rotation(Quat::from_rotation_y(0.5)),
        ))
        .id();
    // `vehicle_bundle` already carries the rigid-body components — the
    // truth overrides insert over them rather than duplicate the spawn.
    app.world_mut().entity_mut(trailer).insert((
        avian3d::prelude::LinearVelocity(Vec3::new(4.0, 0.0, 0.0)),
        avian3d::prelude::AngularVelocity(Vec3::new(0.0, 0.25, 0.0)),
    ));
    {
        let mut state = app
            .world_mut()
            .get_mut::<mm2_vehicle::VehicleState>(trailer)
            .unwrap();
        state.grounded = true;
        for ws in &mut state.wheels {
            ws.grounded = true;
            ws.vel_long = 6.8;
        }
    }
    // Publish a handful of frames so the peer's buffer holds a snap
    // carrying the tail before the blocking recv drains it.
    for _ in 0..5 {
        app.update();
    }
    let msg = until_wire(&mut peer, |m| {
        matches!(m, Message::Snap { entries, trailers, .. }
            if entries.iter().any(|e| e.player == 1 && e.steer == 250)
                && trailers.iter().any(|t| t.owner == 1))
    });
    let Message::Snap {
        entries, trailers, ..
    } = msg
    else {
        unreachable!()
    };
    let e = entries.iter().find(|e| e.player == 1).unwrap();
    assert_eq!(e.steer, 250, "0.25 rad in milliradians");
    assert_eq!(e.spin, 200, "6.8 / 0.34 rad/s in 0.1 rad/s units");
    assert_eq!(e.compression, 51, "mean droop fraction x255");
    assert_eq!(
        e.flags,
        mm2_net::SNAP_FLAG_BRAKE | mm2_net::SNAP_FLAG_REVERSE | mm2_net::SNAP_FLAG_GROUNDED
    );
    assert_eq!(
        e.damage, 128,
        "half of MaxDamage rounds to 128 on the x255 byte"
    );
    // The trailer row keys off the towing seat's wire id and carries
    // the trailer's own pose/velocity/spin truth.
    let t = trailers.iter().find(|t| t.owner == 1).unwrap();
    assert_eq!(t.pos, [3.0, 0.6, -7.0]);
    assert_eq!(t.vel, [4.0, 0.0, 0.0]);
    assert_eq!(t.angvel, [0.0, 0.25, 0.0]);
    assert_eq!(t.spin, 200, "the trailer's grounded wheels rate");
    assert_eq!(
        t.flags,
        mm2_net::SNAP_FLAG_GROUNDED,
        "brake/reverse are seat state a trailer row does not carry"
    );
}

/// F25-B, client half of the v9 trailer rows: a `Snap.trailers` row
/// drives the remote rig's kinematic trailer copy exactly like its
/// seat — blend between arrivals, velocities and wheel rate written,
/// an owner-epoch advance snapping it outright — while the own rig's
/// real trailer drops epoch-equal rows (local physics owns it under
/// prediction) and snaps only when the authority reseats the rig.
/// Trailered picks are retail-only, so both rigs are declared by hand
/// the way the spawn paths leave them.
#[test]
fn a_snapshot_drives_a_remote_rigs_trailer() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let our_id = link.player_id();
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    // The host's seat reconciles into the kinematic copy the trailer
    // row will key off.
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<(), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let host_copy = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the host copy")
    };
    // The local seat — the reconcile stamps it `NetPlayer(our_id)`,
    // which is what the own-rig trailer keys its row off.
    let local = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            Player {
                id: mm2_game::PlayerId(1),
                control: PlayerControl::Local,
            },
            mm2_game::AuthorityRole::Predicted,
            avian3d::prelude::Position::default(),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();
    spin(&mut app, |a| a.world().get::<NetPlayer>(local).is_some());

    // The remote copy's trailer — the shape `spawn_remote`'s predicted
    // branch builds for a trailered pick: kinematic, snap-driven, a
    // `RemoteDrive`/`RemoteLerp` rig like the seat's.
    let trailer_copy = app
        .world_mut()
        .spawn((
            mm2_app::car_visual::Trailer {
                towing: host_copy,
                rest_offset: Vec3::new(0.0, -0.5, 4.0),
            },
            netdrive::RemoteTrailer { owner: 0 },
            RemotePick(VehiclePick {
                vehicle: String::new(),
                paint: 0,
            }),
            mm2_vehicle::vehicle_bundle(&VehicleConfig::default()),
            avian3d::prelude::Position(Vec3::new(0.0, 1.0, 4.0)),
            avian3d::prelude::Rotation::default(),
        ))
        .id();
    app.world_mut().entity_mut(trailer_copy).insert((
        avian3d::prelude::RigidBody::Kinematic,
        mm2_vehicle::RemoteReplica,
        netdrive::RemoteDrive::default(),
        netdrive::RemoteLerp {
            from_pos: Vec3::new(0.0, 1.0, 4.0),
            from_rot: Quat::IDENTITY,
            to_pos: Vec3::new(0.0, 1.0, 4.0),
            to_rot: Quat::IDENTITY,
            start: 0.0,
            end: 0.0,
        },
    ));
    // The own rig's trailer — a real body the local hitch joint owns:
    // no `RemoteTrailer` marker, no lerp, dynamic like `spawn_trailer`
    // leaves it.
    let own_trailer = app
        .world_mut()
        .spawn((
            mm2_app::car_visual::Trailer {
                towing: local,
                rest_offset: Vec3::new(0.0, -0.5, 4.0),
            },
            mm2_vehicle::vehicle_bundle(&VehicleConfig::default()),
            avian3d::prelude::Position(Vec3::new(1.0, 1.0, 5.0)),
            avian3d::prelude::Rotation::default(),
        ))
        .id();

    let seat_entry = |player: u16, epoch: u8| SnapEntry {
        player,
        pos: [9.0, 1.0, 9.0],
        rot: [0.0, 0.0, 0.0, 1.0],
        vel: [1.0, 0.0, 0.0],
        angvel: [0.0; 3],
        epoch,
        steer: 0,
        spin: 300,
        compression: 0,
        flags: mm2_net::SNAP_FLAG_GROUNDED,
        damage: 0,
        breaks: 0,
        ..SnapEntry::default()
    };
    // An epoch-equal snap: the remote trailer blends, the own rig's
    // trailer stays exactly where the local sim left it — the wire row
    // lands close (4.2 m, inside the correction bound) and visibly off
    // its pose, so a buggy unconditional write would move it.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            generation,
            tick: 7,
            entries: vec![seat_entry(0, 0), seat_entry(our_id, 0)],
            trailers: vec![
                mm2_net::SnapTrailer {
                    owner: 0,
                    pos: [9.0, 1.0, 9.7],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [1.0, 0.0, 0.0],
                    angvel: [0.0, 0.25, 0.0],
                    spin: 150,
                    flags: mm2_net::SNAP_FLAG_GROUNDED,
                },
                mm2_net::SnapTrailer {
                    owner: our_id,
                    pos: [4.0, 1.0, 8.0],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [9.0, 9.0, 9.0],
                    angvel: [9.0; 3],
                    spin: -400,
                    flags: 0,
                },
            ],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > 0
    });
    {
        let lerp = app
            .world()
            .get::<netdrive::RemoteLerp>(trailer_copy)
            .unwrap();
        assert!(
            (lerp.to_pos - Vec3::new(9.0, 1.0, 9.7)).length() < 1e-3,
            "the remote trailer's lerp targets the wire pose: {:?}",
            lerp.to_pos
        );
        assert_eq!(
            app.world()
                .get::<avian3d::prelude::LinearVelocity>(trailer_copy)
                .unwrap()
                .0,
            Vec3::new(1.0, 0.0, 0.0),
            "the row's velocity landed"
        );
        assert_eq!(
            app.world()
                .get::<netdrive::RemoteDrive>(trailer_copy)
                .unwrap()
                .spin_rate,
            15.0,
            "the row's wheel rate landed"
        );
    }
    // The first blend interval is zero — the next lerp update lands
    // the copy on the asserted pose.
    app.update();
    assert!(
        (app.world()
            .get::<avian3d::prelude::Position>(trailer_copy)
            .unwrap()
            .0
            - Vec3::new(9.0, 1.0, 9.7))
        .length()
            < 1e-3,
        "the remote trailer blended to the wire pose"
    );
    // The own rig's trailer ignored its epoch-equal row — pose and
    // velocities stay the local sim's.
    assert_eq!(
        app.world()
            .get::<avian3d::prelude::Position>(own_trailer)
            .unwrap()
            .0,
        Vec3::new(1.0, 1.0, 5.0),
        "an epoch-equal own-rig row never moves the local body"
    );
    assert_eq!(
        app.world()
            .get::<avian3d::prelude::LinearVelocity>(own_trailer)
            .unwrap()
            .0,
        Vec3::ZERO
    );

    // An owner-epoch advance — the authority's reset reseated the rig —
    // snaps the own trailer outright and marks the jump `Teleported`.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            generation,
            tick: 8,
            entries: vec![seat_entry(0, 0), seat_entry(our_id, 1)],
            trailers: vec![mm2_net::SnapTrailer {
                owner: our_id,
                pos: [4.0, 1.0, 8.0],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
                spin: 0,
                flags: 0,
            }],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > 1
    });
    assert_eq!(
        app.world()
            .get::<avian3d::prelude::Position>(own_trailer)
            .unwrap()
            .0,
        Vec3::new(4.0, 1.0, 8.0),
        "the owner's epoch advance reseats the own trailer"
    );
    assert!(
        app.world().get::<Teleported>(own_trailer).is_some(),
        "the wire-declared reseat is marked Teleported"
    );
    // One remote row + one own-rig snap landed; the dropped epoch-equal
    // own row never counted.
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .trailers_synced,
        2
    );

    host.shutdown();
}

/// F25-B, the trailer row's grounded bit: the row carries no per-wheel
/// compression, so the bit is the copy's only suspension truth — a
/// grounded row settles the kinematic copy's wheels at their authored
/// rest sag (the pose `update_wheel_visuals` draws against
/// `ws.compression`) and a clear one hangs them at full droop, instead
/// of the `VehicleState::new` droop they were spawned with.
#[test]
fn a_trailer_rows_grounded_bit_drives_the_copys_suspension() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    // The host's seat reconciles into the kinematic copy the trailer
    // copy hitches to.
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<(), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let host_copy = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the host copy")
    };
    // The remote copy's trailer — the shape `spawn_trailer_copy` builds
    // for a trailered pick on a predicted client (trailered picks are
    // retail-only, so the rig is declared by hand).
    let cfg = VehicleConfig {
        trailer: true,
        ..VehicleConfig::default()
    };
    let trailer_copy = app
        .world_mut()
        .spawn((
            mm2_app::car_visual::Trailer {
                towing: host_copy,
                rest_offset: Vec3::new(0.0, -0.5, 4.0),
            },
            netdrive::RemoteTrailer { owner: 0 },
            RemotePick(VehiclePick {
                vehicle: String::new(),
                paint: 0,
            }),
            mm2_vehicle::vehicle_bundle(&cfg),
            avian3d::prelude::Position(Vec3::new(0.0, 1.0, 4.0)),
            avian3d::prelude::Rotation::default(),
        ))
        .id();
    app.world_mut().entity_mut(trailer_copy).insert((
        avian3d::prelude::RigidBody::Kinematic,
        mm2_vehicle::RemoteReplica,
        netdrive::RemoteDrive::default(),
        netdrive::RemoteLerp {
            from_pos: Vec3::new(0.0, 1.0, 4.0),
            from_rot: Quat::IDENTITY,
            to_pos: Vec3::new(0.0, 1.0, 4.0),
            to_rot: Quat::IDENTITY,
            start: 0.0,
            end: 0.0,
        },
    ));
    let trailer_row = |flags: u8| mm2_net::SnapTrailer {
        owner: 0,
        pos: [9.0, 1.0, 9.7],
        rot: [0.0, 0.0, 0.0, 1.0],
        vel: [0.0; 3],
        angvel: [0.0; 3],
        spin: 0,
        flags,
    };
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            generation,
            tick: 7,
            entries: Vec::new(),
            trailers: vec![trailer_row(mm2_net::SNAP_FLAG_GROUNDED)],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > 0
    });
    let expect = mm2_vehicle::HandlingMetrics::of(&cfg);
    {
        let state = app
            .world()
            .get::<mm2_vehicle::VehicleState>(trailer_copy)
            .unwrap();
        assert!(state.grounded, "the row's grounded bit landed");
        for (ws, wm) in state.wheels.iter().zip(expect.wheels.iter()) {
            assert!(ws.grounded, "every wheel reads grounded");
            assert!(wm.rest_compression > 0.0, "the fixture config sags at rest");
            assert!(
                (ws.compression - wm.rest_compression).abs() < 1e-6,
                "the copy settles at the authored rest sag: {} vs {}",
                ws.compression,
                wm.rest_compression
            );
        }
    }
    // A clear bit hangs every wheel at full droop.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
            race: None,
            generation,
            tick: 8,
            entries: Vec::new(),
            trailers: vec![trailer_row(0)],
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > 1
    });
    {
        let state = app
            .world()
            .get::<mm2_vehicle::VehicleState>(trailer_copy)
            .unwrap();
        assert!(!state.grounded, "the cleared bit lands too");
        for ws in &state.wheels {
            assert!(!ws.grounded);
            assert_eq!(ws.compression, 0.0, "airborne hangs at full droop");
        }
    }

    host.shutdown();
}

/// F25-B, protocol v10 host half: the authority's filtered
/// `ImpactEvent` stream rides the next `Snap` as per-seat
/// `SnapImpact` rows — one row per participant side that names a
/// `NetPlayer` seat — so a client can present a remote car's hits from
/// the replicated stream the damage byte cannot express. A hit naming
/// no seat emits nothing, and a foreign-generation event never rides.
/// v12: each row also carries the struck side's authored `AudioId`,
/// resolved on the authority — a receiver cannot resolve a struck
/// prop's `ObjectId` out of the authority's local id namespace.
#[test]
fn a_snap_carries_the_sessions_impact_rows() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    let generation = hosted_playing(&mut app);
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let remote_oid = {
        let mut q = app
            .world_mut()
            .query_filtered::<&ObjectIdentity, With<RemotePick>>();
        q.single(app.world()).expect("the remote car").0
    };
    // A struck prop carrying an authored `AudioId` — the v12 lookup
    // resolves its record for the row the seat emits.
    let prop_oid = app.world_mut().resource_mut::<Session>().mint_object_id();
    app.world_mut().spawn((
        ObjectIdentity(prop_oid),
        mm2_game::Banger::new(mm2_game::BangerDefinition {
            name: "sp_wireprop".into(),
            mass: 40.0,
            friction: 0.9,
            elasticity: 0.5,
            impulse_limit2: 0.0,
            size: [0.5, 0.5, 0.5],
            cg: [0.0, 0.0, 0.0],
            num_parts: 0,
            audio_id: 7,
        }),
    ));
    let session_tick = app.world().resource::<Session>().tick();
    let write_impact = |app: &mut App, id: u64, generation: u64, a: ObjectId, b: ObjectId| {
        app.world_mut().write_message(ImpactEvent {
            id: ImpactId(id),
            generation,
            tick: session_tick,
            participants: (a, b),
            point: Vec3::new(1.0, 0.5, -2.0),
            normal: Vec3::new(0.0, 0.0, -1.0),
            severity: 12.5,
            surface: SurfaceState::default(),
        });
    };
    // The seat-named world hit (the remote car is participant 0 — its
    // row carries the mirrored outward normal and the world's
    // catch-all selector), a seat-vs-prop hit whose row carries the
    // prop's authored `AudioId`, a pair no seat can name, and a
    // foreign-generation event: only the first two ride the wire.
    write_impact(&mut app, 7, generation, remote_oid, ObjectId::WORLD);
    write_impact(&mut app, 8, generation, ObjectId::WORLD, ObjectId::WORLD);
    write_impact(&mut app, 9, generation + 9, remote_oid, ObjectId::WORLD);
    write_impact(&mut app, 10, generation, remote_oid, prop_oid);
    app.update();
    let snap = until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { impacts, .. } if impacts.len() >= 2),
    );
    let Message::Snap { impacts, .. } = snap else {
        unreachable!()
    };
    assert_eq!(impacts.len(), 2, "only the seat-named sides ride");
    // Equal severities order by `(seat, id)`.
    let row = impacts[0];
    assert_eq!((row.seat, row.id, row.tick), (1, 7, session_tick));
    assert_eq!(row.point, [1.0, 0.5, -2.0]);
    assert_eq!(
        row.normal,
        [0.0, 0.0, 1.0],
        "the seat-0 side's row carries the mirrored outward normal"
    );
    assert_eq!(row.severity, 12.5);
    assert_eq!(row.audio_id, 0, "the world reads the catch-all selector");
    let prop_row = impacts[1];
    assert_eq!((prop_row.seat, prop_row.id), (1, 10));
    assert_eq!(
        prop_row.audio_id, 7,
        "the struck prop's authored AudioId rides its row"
    );
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .impacts_sent,
        2
    );
}

/// F25-B, protocol v10 client half: a `Snap.impacts` row resolves to
/// the named seat's remote copy and lands as a `RemoteImpact`
/// presentation event — deduped across duplicated frames, skipped for
/// the receiver's own seat (the predicted local stream already
/// rendered it), dropped for departed seats, foreign generations and
/// unsanitized fields.
#[test]
fn a_snapshot_feeds_the_remote_impact_stream() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let our_id = link.player_id();
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    // The host's seat reconciles into the remote copy the wire row
    // resolves to.
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let host_copy = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the host copy")
    };
    // The local seat — stamped `NetPlayer(our_id)` by the reconcile,
    // the identity the own-seat skip reads.
    let local = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            Player {
                id: mm2_game::PlayerId(1),
                control: PlayerControl::Local,
            },
            mm2_game::AuthorityRole::Predicted,
            avian3d::prelude::Position::default(),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();
    spin(&mut app, |a| a.world().get::<NetPlayer>(local).is_some());

    let entry = |player: u16| SnapEntry {
        player,
        pos: [9.0, 1.0, 9.0],
        rot: [0.0, 0.0, 0.0, 1.0],
        vel: [0.0; 3],
        angvel: [0.0; 3],
        epoch: 0,
        steer: 0,
        spin: 0,
        compression: 0,
        flags: 0,
        damage: 0,
        breaks: 0,
        ..SnapEntry::default()
    };
    let row = |seat: u16, id: u64| SnapImpact {
        seat,
        id,
        tick: 7,
        point: [3.0, 0.4, -1.0],
        normal: [0.0, 0.0, 1.0],
        severity: 12.5,
        audio_id: 0,
    };
    // One valid remote-seat row carrying the struck side's authored
    // selector (v12) plus the traps: the own seat's row (skipped,
    // never presented), a departed seat's row, and a non-finite row
    // the sanitize drops.
    host.ctl()
        .broadcast(&Message::Snap {
            generation,
            tick: 7,
            entries: vec![entry(0), entry(our_id)],
            trailers: Vec::new(),
            impacts: vec![
                SnapImpact {
                    audio_id: 7,
                    ..row(0, 1)
                },
                row(our_id, 1),
                row(7, 1),
                SnapImpact {
                    seat: 0,
                    id: 2,
                    tick: 7,
                    point: [f32::NAN; 3],
                    normal: [0.0, 1.0, 0.0],
                    severity: 1.0,
                    audio_id: 0,
                },
            ],
            race: None,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .impacts_applied
            > 0
    });
    {
        let drained: Vec<netdrive::RemoteImpact> = app
            .world_mut()
            .resource_mut::<Messages<netdrive::RemoteImpact>>()
            .drain()
            .collect();
        assert_eq!(drained.len(), 1, "only the remote-seat row lands");
        let impact = drained[0];
        assert_eq!(impact.entity, host_copy);
        assert_eq!(impact.point, Vec3::new(3.0, 0.4, -1.0));
        assert_eq!(impact.normal, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(impact.severity, 12.5);
        assert_eq!(
            impact.audio_id, 7,
            "the wire's struck-side selector lands on the event"
        );
    }
    {
        let r = app.world().resource::<netdrive::NetDriveReport>();
        assert_eq!(r.impacts_applied, 1);
        // Seat 7 has no spawned participant; the NaN row fails the
        // sanitize. The own-seat row is skipped by design, not a drop.
        assert_eq!(r.impacts_dropped, 2);
    }

    // A duplicated frame re-presents the same rows — the `(gen, seat,
    // id)` dedup absorbs them before they reach the apply pass.
    host.ctl()
        .broadcast(&Message::Snap {
            generation,
            tick: 8,
            entries: vec![entry(0), entry(our_id)],
            trailers: Vec::new(),
            impacts: vec![row(0, 1)],
            race: None,
        })
        .unwrap();
    app.update();
    app.update();
    {
        let drained: Vec<netdrive::RemoteImpact> = app
            .world_mut()
            .resource_mut::<Messages<netdrive::RemoteImpact>>()
            .drain()
            .collect();
        assert!(
            drained.is_empty(),
            "a duplicated frame's rows never double-fire"
        );
    }
    // A foreign-generation row drops at the apply-side session gate.
    // The drop count is the arrival signal: the row crosses the host
    // loop, the socket and the pump thread on wall-clock time, so a
    // fixed frame count can look before it lands.
    host.ctl()
        .broadcast(&Message::Snap {
            generation: generation + 9,
            tick: 9,
            entries: vec![entry(0)],
            trailers: Vec::new(),
            impacts: vec![row(0, 99)],
            race: None,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .impacts_dropped
            > 2
    });
    {
        let drained: Vec<netdrive::RemoteImpact> = app
            .world_mut()
            .resource_mut::<Messages<netdrive::RemoteImpact>>()
            .drain()
            .collect();
        assert!(drained.is_empty());
        let r = app.world().resource::<netdrive::NetDriveReport>();
        assert_eq!(r.impacts_applied, 1);
        assert_eq!(r.impacts_dropped, 3, "the foreign-generation row dropped");
    }

    host.shutdown();
}

/// F25-B, protocol v13 host half: while the session runs an event the
/// authority's `RaceState` rides every `Snap` — the lifecycle phase,
/// the live countdown remainder and the race clock — the state a
/// predicted client's authority-gated `advance_race` can never step
/// for itself. A stale resource minted under a dead generation
/// publishes nothing: receivers read `race: None`, never a foreign
/// numbering's row.
#[test]
fn a_snap_publishes_the_authoritys_race_state() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    // Begin under the lobby's minted generation and hold the session
    // mid-countdown: `publish_snapshots` emits through
    // `Ready|Countdown|Playing`, so the row rides before control
    // releases. `RaceState::new` is the resource `load_session_world`
    // inserts for an event session — staged by hand here because the
    // load legs need the asset stack.
    app.world()
        .resource::<HostLink>()
        .command_sender()
        .send(HostCommand::Start)
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<Session>().config().is_some()
    });
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Countdown).unwrap();
    }
    let generation = app.world().resource::<Session>().generation();
    app.world_mut()
        .insert_resource(mm2_game::RaceState::new(wire_race_def(180), generation));
    app.update();
    let snap = until_wire(&mut peer, |m| {
        matches!(m, Message::Snap { race: Some(..), .. })
    });
    let Message::Snap {
        race: Some(race), ..
    } = snap
    else {
        unreachable!("the predicate matched a race row")
    };
    assert_eq!(
        (race.phase, race.countdown, race.clock),
        (0, 180, 0),
        "the countdown remainder and the untouched clock ride the snap"
    );

    // The release rides the same row: mutate the resource the way
    // `advance_race` does and the next `Snap` carries it.
    {
        let mut race = app.world_mut().resource_mut::<mm2_game::RaceState>();
        race.phase = mm2_game::RacePhase::Running;
        race.clock = 3;
    }
    app.update();
    let snap = until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { race: Some(r), .. } if r.phase == 1),
    );
    let Message::Snap {
        race: Some(race), ..
    } = snap
    else {
        unreachable!("the predicate matched a running row")
    };
    assert_eq!(race.clock, 3, "the running clock ticks over the wire");

    // Teardown residue — a resource minted under a dead generation —
    // publishes nothing: the last rows read `race: None`, so no
    // receiver can mirror a dead session's numbering.
    app.world_mut()
        .resource_mut::<mm2_game::RaceState>()
        .generation = generation + 9;
    app.update();
    let snap = until_wire(&mut peer, |m| matches!(m, Message::Snap { race: None, .. }));
    let Message::Snap { race: None, .. } = snap else {
        unreachable!("the predicate matched a raceless row")
    };
}

/// F25-B, protocol v13 client half: the wire's race rows are the only
/// clock a predicted client's countdown runs on — `advance_race` is
/// authority-gated and never steps there, which is why joined clients
/// sat in `Countdown` forever (and `send_drive_input` never sent). A
/// countdown row mirrors the remainder; the `Running` row performs
/// the release — the session moves `Countdown → Playing`, awaiting
/// participants flip, one `RaceStarted` goes out; a regressed reorder
/// cannot re-hold control and a foreign-generation row drops counted.
#[test]
fn a_snap_race_row_releases_the_joined_clients_countdown() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        // The load legs stand the session `Ready → Countdown` and
        // insert the event's `RaceState` — staged by hand here, the
        // load systems need the asset stack.
        let local_generation = app.world().resource::<Session>().generation();
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Countdown).unwrap();
        app.world_mut().insert_resource(mm2_game::RaceState::new(
            wire_race_def(180),
            local_generation,
        ));
    }
    // A participant awaiting the countdown — the release flips it.
    let participant = app
        .world_mut()
        .spawn(mm2_game::RaceProgress::new(&wire_race_def(180)))
        .id();

    let race_snap = |generation, tick, race| Message::Snap {
        generation,
        tick,
        entries: Vec::new(),
        trailers: Vec::new(),
        impacts: Vec::new(),
        race,
    };
    let countdown = |remaining: u32| {
        Some(mm2_net::SnapRace {
            phase: 0,
            countdown: remaining,
            clock: 0,
        })
    };
    let running = |clock: u64| {
        Some(mm2_net::SnapRace {
            phase: 1,
            countdown: 0,
            clock,
        })
    };

    // Every countdown snap carries the same frozen session tick — the
    // pose side reads them all stale after the first; the race row
    // still has to move.
    host.ctl()
        .broadcast(&race_snap(generation, 7, countdown(170)))
        .unwrap();
    host.ctl()
        .broadcast(&race_snap(generation, 7, countdown(160)))
        .unwrap();
    spin(&mut app, |a| {
        matches!(
            a.world().resource::<mm2_game::RaceState>().phase,
            mm2_game::RacePhase::Countdown { remaining: 160 }
        )
    });
    assert_eq!(session_phase(&app), SessionPhase::Countdown);
    // Race rows are latest-wins *state*, not events — the 170 row may
    // apply once before the 160 row displaces it, or never at all.
    assert!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .race_applied
            >= 1
    );

    // The release: the wire's `Running` row is the client's
    // `advance_race` — the session stands live, awaiting participants
    // flip, the one `RaceStarted` goes out for the GO consumers.
    host.ctl()
        .broadcast(&race_snap(generation, 7, running(0)))
        .unwrap();
    spin(&mut app, |a| session_phase(a) == SessionPhase::Playing);
    {
        let race = app.world().resource::<mm2_game::RaceState>();
        assert_eq!(race.phase, mm2_game::RacePhase::Running);
        assert_eq!(race.clock, 0);
        assert_eq!(
            app.world()
                .get::<mm2_game::RaceProgress>(participant)
                .unwrap()
                .state,
            mm2_game::ParticipantState::Racing,
            "the release flips awaiting participants"
        );
        let started: Vec<mm2_game::RaceStarted> = app
            .world_mut()
            .resource_mut::<Messages<mm2_game::RaceStarted>>()
            .drain()
            .collect();
        assert_eq!(started.len(), 1, "one release event — the authority's word");
    }

    // A reorder's countdown straggler cannot re-hold control — its
    // freshness key ranks behind the applied `Running`, so it never
    // stages (idempotent state reads nothing).
    host.ctl()
        .broadcast(&race_snap(generation, 7, countdown(90)))
        .unwrap();
    app.update();
    app.update();
    {
        assert_eq!(
            app.world().resource::<mm2_game::RaceState>().phase,
            mm2_game::RacePhase::Running,
            "the regressed countdown row never staged"
        );
        assert_eq!(session_phase(&app), SessionPhase::Playing);
        assert_eq!(
            app.world()
                .resource::<netdrive::NetDriveReport>()
                .race_dropped,
            0,
            "a regressed state row is silent, not a drop"
        );
    }

    // The running clock keeps ticking over the wire.
    host.ctl()
        .broadcast(&race_snap(generation, 7, running(41)))
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<mm2_game::RaceState>().clock == 41
    });

    // The foreign generation's row stages past the key — a different
    // authority's numbering ranks ahead — but dies at the session
    // gate the queued rows share. (It must land after the gen-row
    // above applied: staged rows are latest-wins, so an in-flight
    // interleave would displace it.)
    host.ctl()
        .broadcast(&race_snap(generation + 9, 7, running(99)))
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .race_dropped
            == 1
    });
    {
        assert_eq!(
            app.world().resource::<mm2_game::RaceState>().phase,
            mm2_game::RacePhase::Running,
            "the foreign generation's row dropped at the session gate"
        );
        assert_eq!(
            app.world().resource::<mm2_game::RaceState>().clock,
            41,
            "the foreign clock never mirrored"
        );
        assert_eq!(session_phase(&app), SessionPhase::Playing);
    }

    host.shutdown();
}

/// F26-A: the race row also steers the scenery clock. A row that puts
/// this peer's world more than the tolerance away queues a re-seek to
/// the host's tick (`countdown_ticks + clock` once running); jitter
/// inside the tolerance leaves the clock alone.
#[test]
fn a_snap_race_row_re_seeks_the_scenery_clock() {
    use mm2_app::worldclock::{SYNC_TOLERANCE_TICKS, WorldClock};

    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let local_generation = app.world().resource::<Session>().generation();
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Countdown).unwrap();
        app.world_mut().insert_resource(mm2_game::RaceState::new(
            wire_race_def(180),
            local_generation,
        ));
    }
    // `bridge_app` runs no `advance_world_clock`, so a queued seek
    // stays visible until the test consumes it, as the system would.
    app.world_mut().insert_resource(WorldClock::default());
    let race_snap = |tick, race| Message::Snap {
        generation,
        tick,
        entries: Vec::new(),
        trailers: Vec::new(),
        impacts: Vec::new(),
        race: Some(race),
    };
    let running = |clock: u64| mm2_net::SnapRace {
        phase: 1,
        countdown: 0,
        clock,
    };
    let seek = |a: &App| a.world().resource::<WorldClock>().seek;

    // A joiner at tick 0 is 180 ticks (the countdown) behind a host
    // whose race clock has just started.
    host.ctl().broadcast(&race_snap(7, running(0))).unwrap();
    spin(&mut app, |a| seek(a).is_some());
    assert_eq!(seek(&app), Some(180));

    // The system's consumption: the clock stands at the seek target.
    *app.world_mut().resource_mut::<WorldClock>() = WorldClock {
        ticks: 180,
        seek: None,
    };

    // A row within the tolerance is jitter, not drift. Wait for the
    // row to be applied (the report counts it) before asserting that
    // nothing queued.
    let applied = app
        .world()
        .resource::<netdrive::NetDriveReport>()
        .race_applied;
    host.ctl()
        .broadcast(&race_snap(7, running(SYNC_TOLERANCE_TICKS)))
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .race_applied
            > applied
    });
    assert_eq!(seek(&app), None, "jitter inside the tolerance is ignored");

    // Real drift queues a seek to the host's tick.
    host.ctl().broadcast(&race_snap(7, running(600))).unwrap();
    spin(&mut app, |a| seek(a).is_some());
    assert_eq!(seek(&app), Some(780));
}

/// F26-A, protocol v20 host half: the authority publishes its world
/// clock the moment the countdown/playing clock runs and then once per
/// `PUBLISH_EVERY_TICKS` of world time — not on every frame. A Cruise
/// session carries no race row, so this frame is the only thing a
/// client's scenery can align to.
#[test]
fn the_host_publishes_its_world_clock_at_the_cadence() {
    use mm2_app::worldclock::{PUBLISH_EVERY_TICKS, WorldClock};

    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    app.insert_resource(WorldClock {
        ticks: 7,
        seek: None,
    });
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    let generation = hosted_playing(&mut app);

    // The first frame goes out at once, carrying the clock as it stands.
    let first = until_wire(&mut peer, |m| matches!(m, Message::World { .. }));
    assert_eq!(
        first,
        Message::World {
            generation,
            ticks: 7
        }
    );

    // A clock a hair short of the cadence sends nothing — the next
    // frame on the wire is the one at the cadence, not this one.
    app.world_mut().resource_mut::<WorldClock>().ticks = 7 + PUBLISH_EVERY_TICKS - 1;
    for _ in 0..5 {
        app.update();
    }
    app.world_mut().resource_mut::<WorldClock>().ticks = 7 + PUBLISH_EVERY_TICKS;
    app.update();
    let second = until_wire(&mut peer, |m| matches!(m, Message::World { .. }));
    assert_eq!(
        second,
        Message::World {
            generation,
            ticks: 7 + PUBLISH_EVERY_TICKS
        }
    );
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .world_sent,
        2
    );

    // A restart starts the clock over; the host announces it at once
    // rather than waiting a cadence past the old high-water mark.
    app.world_mut().resource_mut::<WorldClock>().ticks = 3;
    app.update();
    let third = until_wire(&mut peer, |m| matches!(m, Message::World { .. }));
    assert_eq!(
        third,
        Message::World {
            generation,
            ticks: 3
        }
    );
}

/// F26-A, protocol v20 client half: the host's clock frame re-seeks a
/// Cruise client's scenery (no race row involved), stale or reordered
/// frames and another generation's change nothing, jitter inside the
/// tolerance is ignored, and an absurd tick is refused without
/// poisoning the honest frames after it.
#[test]
fn a_world_clock_frame_re_seeks_a_cruise_clients_scenery() {
    use mm2_app::worldclock::{MAX_SEEK_TICKS, SYNC_TOLERANCE_TICKS, WorldClock, WorldLimits};

    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    // `bridge_app` runs no `advance_world_clock`, so a queued seek
    // stays visible until the test consumes it, as the system would.
    app.world_mut().insert_resource(WorldClock::default());
    // The legs below send frames back to back; the bound on how fast a
    // host may do that is exercised at the end, with limits chosen so
    // no wall-clock timing decides the outcome.
    app.world_mut()
        .resource_mut::<netdrive::RemoteSnaps>()
        .set_world_limits(WorldLimits::UNBOUNDED);
    let seek = |a: &App| a.world().resource::<WorldClock>().seek;
    let world = |a: &App| {
        let w = a.world().resource::<netdrive::RemoteSnaps>().world();
        (w.landed(), w.stale(), w.refused(), w.seeks())
    };
    let send = |ticks| {
        host.ctl()
            .broadcast(&Message::World { generation, ticks })
            .unwrap()
    };

    // A late joiner at tick 0 lands on the host's clock.
    send(5_000);
    spin(&mut app, |a| seek(a).is_some());
    assert_eq!(seek(&app), Some(5_000));
    assert_eq!(world(&app), (1, 0, 0, 1));
    *app.world_mut().resource_mut::<WorldClock>() = WorldClock {
        ticks: 5_000,
        seek: None,
    };

    // A reordered older frame is dropped, never applied.
    send(4_000);
    spin(&mut app, |a| world(a).1 == 1);
    assert_eq!(seek(&app), None, "an older frame must not drag it back");

    // Jitter inside the tolerance lands (the frame is current) but
    // queues no seek.
    send(5_000 + SYNC_TOLERANCE_TICKS);
    spin(&mut app, |a| world(a).0 == 2);
    assert_eq!(seek(&app), None);
    assert_eq!(world(&app).3, 1, "no seek queued inside the tolerance");

    // Another generation's frame is refused at apply time.
    host.ctl()
        .broadcast(&Message::World {
            generation: generation + 1,
            ticks: 9_000,
        })
        .unwrap();
    spin(&mut app, |a| world(a).2 == 1);
    assert_eq!(seek(&app), None);

    // An absurd tick is refused and does not become the watermark: the
    // honest frame after it still lands.
    send(MAX_SEEK_TICKS + 1);
    spin(&mut app, |a| world(a).2 == 2);
    assert_eq!(seek(&app), None);
    send(9_000);
    spin(&mut app, |a| seek(a).is_some());
    assert_eq!(seek(&app), Some(9_000));

    // A host cannot drive replays: with a minimum interval no test run
    // can outlast, the next frame is throttled, applies nothing and
    // leaves the watermark where it was.
    app.world_mut().resource_mut::<WorldClock>().seek = None;
    app.world_mut()
        .resource_mut::<netdrive::RemoteSnaps>()
        .set_world_limits(WorldLimits {
            min_interval: std::time::Duration::from_secs(3_600),
            ..WorldLimits::UNBOUNDED
        });
    send(12_000);
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::RemoteSnaps>()
            .world()
            .throttled()
            == 1
    });
    assert_eq!(seek(&app), None, "a throttled frame queues no replay");

    // And a clock that ran further than the host could have since the
    // last frame taken is refused, again without a watermark: the
    // modest frame after it (inside the 100-tick allowance) lands.
    let refused_before = world(&app).2;
    app.world_mut()
        .resource_mut::<netdrive::RemoteSnaps>()
        .set_world_limits(WorldLimits {
            min_interval: std::time::Duration::ZERO,
            max_ticks_per_second: 0.0,
            slack_ticks: 100,
        });
    send(9_101);
    spin(&mut app, |a| world(a).2 == refused_before + 1);
    assert_eq!(seek(&app), None);
    send(9_100);
    spin(&mut app, |a| seek(a).is_some());
    assert_eq!(seek(&app), Some(9_100));
}

/// F25-B, protocol v14 client half: a seat's `SnapEntry` progress
/// tail is the predicted client's only `RaceProgress` truth —
/// `advance_race` is authority-gated and never steps there, so the
/// wire mirrors every seat's standing. The tail lands the rule
/// counters verbatim, a terminal edge mints the same `SessionResult`
/// `advance_race` records on the authority into the client's own
/// `ResultLedger` — and only the *local* participant's resolution
/// moves the session `Playing → Results` (UI-5). A conflicting
/// terminal row drops counted rather than rewriting a recorded
/// result.
#[test]
fn a_snap_progress_tail_mirrors_and_resolves_the_joined_client() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let our_id = link.player_id();
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        // The load legs stand the session `Ready → Countdown` and
        // insert the event's `RaceState` — staged by hand here, the
        // load systems need the asset stack. The wire's rows then
        // carry the run forward.
        let local_generation = app.world().resource::<Session>().generation();
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let mut race = mm2_game::RaceState::new(wire_race_def(180), local_generation);
        race.phase = mm2_game::RacePhase::Running;
        app.world_mut().insert_resource(race);
    }
    // The host's seat reconciles into the kinematic copy; the race
    // tracks it as a participant, so it carries `RaceProgress` like
    // the real load path's seat spawn does.
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let host_copy = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the host copy")
    };
    app.world_mut()
        .entity_mut(host_copy)
        .insert(mm2_game::RaceProgress::new(&wire_race_def(180)));
    // The local seat — stamped `NetPlayer(our_id)` by the reconcile.
    let local = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            Player {
                id: mm2_game::PlayerId(1),
                control: PlayerControl::Local,
            },
            mm2_game::AuthorityRole::Predicted,
            mm2_game::RaceProgress::new(&wire_race_def(180)),
            avian3d::prelude::Position::default(),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();
    spin(&mut app, |a| a.world().get::<NetPlayer>(local).is_some());

    let progress_entry = |player: u16, state: u8, ticks: u64| SnapEntry {
        player,
        pos: [9.0, 1.0, 9.0],
        rot: [0.0, 0.0, 0.0, 1.0],
        vel: [0.0; 3],
        angvel: [0.0; 3],
        epoch: 0,
        steer: 0,
        spin: 0,
        compression: 0,
        flags: 0,
        damage: 0,
        breaks: 0,
        prog_state: state,
        prog_ticks: ticks,
        prog_cleared: 0b1,
        prog_crossings: 4,
        ..SnapEntry::default()
    };
    let snap = |tick: u64, entries: Vec<SnapEntry>| Message::Snap {
        generation,
        tick,
        entries,
        trailers: Vec::new(),
        impacts: Vec::new(),
        race: None,
    };

    // A mid-race row: the host seat's counters mirror verbatim.
    host.ctl()
        .broadcast(&snap(7, vec![progress_entry(0, 1, 0)]))
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .progress_applied
            >= 1
    });
    {
        let progress = app
            .world()
            .get::<mm2_game::RaceProgress>(host_copy)
            .unwrap();
        assert_eq!(progress.state, mm2_game::ParticipantState::Racing);
        assert!(progress.is_cleared(0), "the cleared mask landed");
        assert_eq!(progress.crossings, 4);
    }

    // The host's terminal edge mints a result on this process — the
    // ledger is the client's own, the participant's state carries
    // the minted id — while the local session stays `Playing`: a
    // remote seat resolving never ends *our* run (UI-5).
    host.ctl()
        .broadcast(&snap(8, vec![progress_entry(0, 2, 4200)]))
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<mm2_game::ResultLedger>().len() == 1
    });
    {
        let state = &app
            .world()
            .get::<mm2_game::RaceProgress>(host_copy)
            .unwrap()
            .state;
        let mm2_game::ParticipantState::Finished { race_ticks, result } = state else {
            panic!("expected Finished, got {state:?}")
        };
        assert_eq!(*race_ticks, 4200);
        let ledger = app.world().resource::<mm2_game::ResultLedger>();
        let recorded = ledger.get(result).unwrap();
        assert_eq!(
            recorded.outcome,
            mm2_game::SessionOutcome::Finished { race_ticks: 4200 },
            "the wire's word minted the same shape `advance_race` does"
        );
        assert_eq!(recorded.tick, 8, "the snap's tick stamps the result");
        assert_eq!(session_phase(&app), SessionPhase::Playing);
    }

    // The local participant's own finish on the wire's word is what
    // ends the local session — the mirrored UI-5 rule.
    host.ctl()
        .broadcast(&snap(9, vec![progress_entry(our_id, 2, 4300)]))
        .unwrap();
    spin(&mut app, |a| session_phase(a) == SessionPhase::Results);
    assert!(matches!(
        app.world()
            .get::<mm2_game::RaceProgress>(local)
            .unwrap()
            .state,
        mm2_game::ParticipantState::Finished {
            race_ticks: 4300,
            ..
        }
    ));
    assert_eq!(app.world().resource::<mm2_game::ResultLedger>().len(), 2);

    // A terminal row disagreeing with a recorded resolution is a
    // non-conforming authority's word — refused, not believed: the
    // host seat's recorded finish stands, the row counts as dropped.
    // The drop count is the arrival signal: the row crosses the host
    // loop, the socket and the pump thread on wall-clock time, so a
    // fixed frame count can look before it lands.
    let dropped_before = app
        .world()
        .resource::<netdrive::NetDriveReport>()
        .progress_dropped;
    host.ctl()
        .broadcast(&snap(10, vec![progress_entry(0, 3, 100)]))
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .progress_dropped
            > dropped_before
    });
    {
        assert!(matches!(
            app.world()
                .get::<mm2_game::RaceProgress>(host_copy)
                .unwrap()
                .state,
            mm2_game::ParticipantState::Finished {
                race_ticks: 4200,
                ..
            }
        ));
        assert_eq!(app.world().resource::<mm2_game::ResultLedger>().len(), 2);
        assert_eq!(
            app.world()
                .resource::<netdrive::NetDriveReport>()
                .progress_dropped,
            dropped_before + 1,
            "the rewound lifecycle dropped counted"
        );
    }

    host.shutdown();
}

/// F25-B, protocol v14 host half: while an event session runs, every
/// tracked seat's `RaceProgress` rides its `SnapEntry` — the
/// lifecycle discriminant, the terminal state's resolution tick, the
/// `Ordered` counters, the cleared-gate mask and the evidence
/// counters — the standing a predicted client's authority-gated rule
/// pipeline can never compute for itself. A seat the race does not
/// track publishes the all-zero tail: never a fabricated row.
#[test]
fn a_snap_publishes_the_seats_race_progress() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    app.world()
        .resource::<HostLink>()
        .command_sender()
        .send(HostCommand::Start)
        .unwrap();
    spin(&mut app, |a| {
        a.world().resource::<Session>().config().is_some()
    });
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    let generation = app.world().resource::<Session>().generation();
    let def = wire_race_def(180);
    let mut race = mm2_game::RaceState::new(def.clone(), generation);
    race.phase = mm2_game::RacePhase::Running;
    app.world_mut().insert_resource(race);

    // The host seat — the shape `load_session_world` leaves a
    // participant: wire id 0, the pose row publish reads, and a
    // `RaceProgress` mid-race (the gate cleared by a real crossing,
    // the rest verbatim counters).
    let mut progress = mm2_game::RaceProgress::new(&def);
    progress.state = mm2_game::ParticipantState::Racing;
    progress.advance(&def, Vec3::new(0.0, 0.0, -201.0));
    progress.advance(&def, Vec3::new(0.0, 0.0, -199.0));
    progress.route_clears = 2;
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        let result = session.mint_result_id(mm2_game::PlayerId(0));
        progress.state = mm2_game::ParticipantState::Finished {
            race_ticks: 4200,
            result,
        };
    }
    app.world_mut().spawn((
        netdrive::NetPlayer(0),
        Player {
            id: mm2_game::PlayerId(0),
            control: PlayerControl::Local,
        },
        netdrive::ResetEpoch(0),
        mm2_game::ObjectIdentity(mm2_game::ObjectId {
            generation: 1,
            slot: 100,
        }),
        progress,
        avian3d::prelude::Position(Vec3::new(1.0, 1.0, 1.0)),
        avian3d::prelude::Rotation::default(),
        avian3d::prelude::LinearVelocity::default(),
        avian3d::prelude::AngularVelocity::default(),
    ));
    app.update();

    let snap = until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| e.prog_state == 2)),
    );
    let Message::Snap { entries, .. } = snap else {
        unreachable!("the predicate matched a progress tail")
    };
    let entry = entries
        .iter()
        .find(|e| e.player == 0)
        .expect("the host seat's row");
    assert_eq!(
        (
            entry.prog_state,
            entry.prog_ticks,
            entry.prog_cleared,
            entry.prog_crossings,
            entry.prog_route_clears,
        ),
        (2, 4200, 0b1, 1, 2),
        "the authority's standing rides the seat's row verbatim"
    );

    // A seat the race does not track — no `RaceProgress` — publishes
    // the all-zero tail: "not tracked", never a fabricated row.
    // (Wire id 7 keeps it out of the roster's own numbering.)
    app.world_mut().spawn((
        netdrive::NetPlayer(7),
        Player {
            id: mm2_game::PlayerId(1),
            control: PlayerControl::Local,
        },
        netdrive::ResetEpoch(0),
        mm2_game::ObjectIdentity(mm2_game::ObjectId {
            generation: 1,
            slot: 101,
        }),
        avian3d::prelude::Position(Vec3::new(2.0, 1.0, 2.0)),
        avian3d::prelude::Rotation::default(),
        avian3d::prelude::LinearVelocity::default(),
        avian3d::prelude::AngularVelocity::default(),
    ));
    app.update();
    let snap = until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| e.player == 7)),
    );
    let Message::Snap { entries, .. } = snap else {
        unreachable!("the predicate matched the untracked seat's row")
    };
    let entry = entries
        .iter()
        .find(|e| e.player == 7)
        .expect("the untracked seat's row");
    assert_eq!(
        (
            entry.prog_state,
            entry.prog_ticks,
            entry.prog_cleared,
            entry.prog_crossings,
            entry.prog_route_clears,
        ),
        (0, 0, 0, 0, 0),
        "no fabricated progress for a seat the race does not track"
    );
}

/// F25-B (protocol v11): a `SnapEntry.breaks` bitmask is replicated
/// rig *state* — a client diffs it every snap against every named
/// seat's `VehicleBreaks`: a set bit sheds the part onto a pooled
/// fragment (intact node hides), a cleared bit re-attaches it (the
/// authority's repair arriving as state). The dev car authors no
/// breakable parts, so the copies' rigs are declared by hand exactly
/// the way `spawn_remote` + `car_visual::spawn_vehicle_model` build
/// them for a `dgbangerdata`-backed pick.
#[test]
fn a_snap_reconciles_the_remote_copys_breakaway_rig() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let our_id = link.player_id();
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let host_copy = {
        let mut q = app.world_mut().query_filtered::<Entity, With<RemotePick>>();
        q.single(app.world()).expect("the host copy")
    };
    // The local seat — stamped `NetPlayer(our_id)` by the reconcile.
    let local = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            Player {
                id: mm2_game::PlayerId(1),
                control: PlayerControl::Local,
            },
            mm2_game::AuthorityRole::Predicted,
            avian3d::prelude::Position::default(),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();
    spin(&mut app, |a| a.world().get::<NetPlayer>(local).is_some());

    // The authored-shaped rig both copies carry when the pick backs
    // it: `VehicleBreaks` on the car, one tagged node per part.
    let spec = mm2_game::BreakPartSpec {
        name: "break0".into(),
        def: mm2_game::BangerDefinition {
            name: "vpcar_break0".into(),
            mass: 100.0,
            friction: 0.9,
            elasticity: 0.3,
            impulse_limit2: 500.0,
            size: [0.8, 0.4, 1.2],
            cg: [0.0, 0.2, 0.0],
            num_parts: 0,
            audio_id: 0,
        },
    };
    let rig_node = |app: &mut App, car: Entity| {
        app.world_mut()
            .entity_mut(car)
            .insert(mm2_game::VehicleBreaks::new(vec![spec.clone()]));
        let node = app
            .world_mut()
            .spawn((
                Transform::from_xyz(0.0, 0.4, -1.0),
                Visibility::Visible,
                mm2_app::breakaway::BreakPartVisual {
                    part: "break0".into(),
                    local: Transform::from_xyz(0.0, 0.4, -1.0),
                    collider: Some(avian3d::prelude::Collider::cuboid(0.4, 0.2, 0.6)),
                    centroid: Vec3::ZERO,
                },
            ))
            .id();
        app.world_mut().entity_mut(car).add_child(node);
        node
    };
    let remote_node = rig_node(&mut app, host_copy);
    let own_node = rig_node(&mut app, local);
    app.update();

    let entry = |player: u16, breaks: u32| SnapEntry {
        player,
        pos: [9.0, 1.0, 9.0],
        rot: [0.0, 0.0, 0.0, 1.0],
        vel: [0.0; 3],
        angvel: [0.0; 3],
        epoch: 0,
        steer: 0,
        spin: 0,
        compression: 0,
        flags: 0,
        damage: 0,
        breaks,
        ..SnapEntry::default()
    };
    // Bit 0 set on both seats: the remote copy sheds like the wire
    // says, and the own seat mirrors it — its predicted sim never runs
    // `detach_breaks`, so the mask is its only detach truth (the same
    // contract the v8 damage byte established).
    host.ctl()
        .broadcast(&Message::Snap {
            generation,
            tick: 7,
            entries: vec![entry(0, 0b1), entry(our_id, 0b1)],
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .breaks_detached
            == 2
    });
    assert_eq!(
        *app.world().get::<Visibility>(remote_node).unwrap(),
        Visibility::Hidden,
        "the remote copy's intact node hides"
    );
    assert_eq!(
        *app.world().get::<Visibility>(own_node).unwrap(),
        Visibility::Hidden,
        "the own rig sheds off the same replicated state"
    );
    let fragments = app
        .world_mut()
        .query::<&mm2_app::breakaway::BreakFragment>()
        .iter(app.world())
        .map(|f| f.vehicle)
        .collect::<Vec<_>>();
    assert_eq!(fragments.len(), 2, "each shed part spawned one fragment");
    assert!(fragments.contains(&host_copy));
    assert!(fragments.contains(&local));

    // Re-sending the same mask is a no-op — it is state, not an event.
    host.ctl()
        .broadcast(&Message::Snap {
            generation,
            tick: 8,
            entries: vec![entry(0, 0b1), entry(our_id, 0b1)],
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        })
        .unwrap();
    app.update();
    app.update();
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .breaks_detached,
        2,
        "a repeated mask never re-detaches"
    );

    // The bits clear: the authority's repair arrives as state — the
    // fragments despawn and the intact nodes show again.
    host.ctl()
        .broadcast(&Message::Snap {
            generation,
            tick: 9,
            entries: vec![entry(0, 0), entry(our_id, 0)],
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: None,
        })
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .breaks_restored
            == 2
    });
    assert_eq!(
        *app.world().get::<Visibility>(remote_node).unwrap(),
        Visibility::Visible,
        "the repaired copy shows its panel again"
    );
    assert_eq!(
        *app.world().get::<Visibility>(own_node).unwrap(),
        Visibility::Visible
    );
    assert_eq!(
        app.world_mut()
            .query::<&mm2_app::breakaway::BreakFragment>()
            .iter(app.world())
            .count(),
        0,
        "the reconcile's fragments despawned"
    );
    assert!(
        app.world()
            .get::<mm2_game::VehicleBreaks>(host_copy)
            .unwrap()
            .detached_count()
            == 0
    );

    host.shutdown();
}

/// inert — it sends a `ResetRequest` minted against the running
/// generation, absorbed by the host's mailbox keyed to our slot. An
/// `R` press outside `Playing` sends nothing.
#[test]
fn r_under_a_remote_session_asks_the_authority() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host = Host::listen_loopback(&HostConfig::new(fp)).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let our_id = link.player_id();
    let mut app = bridge_app(vfs, link);

    // Parked at `Menu` an `R` press asks nothing — the reset request is
    // a session verb, not a lobby one.
    tap(&mut app, KeyCode::KeyR);
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .requests_sent,
        0,
        "a lobby-phase R asks nothing"
    );

    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    app.update();

    tap(&mut app, KeyCode::KeyR);
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .requests_sent,
        1,
        "R under the predicted session asked once"
    );
    // The host mailbox absorbed it, keyed by our roster slot — the
    // target is never a wire field to forge.
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let requests = host.remote_inputs().drain_resets();
        if let Some(&(player, requested)) = requests.first() {
            assert_eq!(player, our_id, "the ask is keyed to our slot");
            assert_eq!(requested, generation, "the ask rides this session");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the host mailbox never saw the reset request"
        );
        thread::sleep(Duration::from_millis(5));
    }

    // No edge, no re-send — one press is one ask; the host's cooldown
    // owns the rate limit.
    app.update();
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .requests_sent,
        1
    );

    host.shutdown();
}

/// F25-B: the data plane under real impairment. The `ImpairProxy`
/// relay sits between the peer and the in-app host, so once the recipe
/// is armed every `Input`/`Snap`/`ResetRequest` crosses a seeded mix of
/// delay, duplication and reorder — and the session still converges:
/// the mailbox keeps the freshest input by sender `seq`, the hosted
/// car drives on it, and a reset ask is granted exactly once no matter
/// how many copies the wire delivered. Loss stays off this leg — a
/// dropped `ResetRequest` is a press nothing answered by design, so
/// the assertion would be about timing, not correctness.
#[test]
fn an_impaired_link_still_converges_the_data_plane() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    // The proxy dials the hosted lobby; the peer dials the proxy.
    let proxy = ImpairProxy::loopback_seeded(link.addr(), 13).unwrap();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(proxy.addr(), "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    let generation = hosted_playing(&mut app);
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<(), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });
    let remote = {
        let mut q = app
            .world_mut()
            .query_filtered::<(Entity, &avian3d::prelude::Position), With<RemotePick>>();
        let (e, p) = q.single(app.world()).expect("the remote car");
        (e, p.0)
    };
    let seated = remote.1;
    // The remote rig's trailer (v9): marked like `spawn_remote`'s
    // authority branch leaves it — the reconcile must take it down
    // with its seat when the peer leaves (the dev-car pick tows
    // nothing, so the rig is declared by hand).
    let trailer = app
        .world_mut()
        .spawn((
            mm2_app::car_visual::Trailer {
                towing: remote.0,
                rest_offset: Vec3::new(0.0, -0.5, 4.0),
            },
            netdrive::RemoteTrailer { owner: 1 },
            RemotePick(VehiclePick {
                vehicle: String::new(),
                paint: 0,
            }),
            mm2_vehicle::vehicle_bundle(&VehicleConfig::default()),
            avian3d::prelude::Position(seated + Vec3::new(0.0, -0.5, 4.0)),
            avian3d::prelude::Rotation::default(),
        ))
        .id();

    // Arm the data plane: the lobby phase crossed clean, everything
    // below is impaired — delayed, jittered, duplicated and reordered
    // on the way up, duplicated on the way down.
    proxy.set(
        LinkDir::Up,
        Impair {
            delay: Duration::from_millis(4),
            jitter: Duration::from_millis(12),
            loss: 0.0,
            duplicate: 0.4,
            reorder: 0.4,
        },
    );
    proxy.set(
        LinkDir::Down,
        Impair {
            delay: Duration::from_millis(4),
            duplicate: 0.5,
            ..Impair::default()
        },
    );

    // A run of inputs up the storm: arrival order is no longer send
    // order, but `seq` names the freshest, and the hosted car ends up
    // driving on it.
    let ctl = peer.ctl().unwrap();
    for seq in 1..=12u64 {
        ctl.send_input(DriveInput {
            generation,
            seq,
            throttle: 255,
            brake: 0,
            steer: 90,
            handbrake: 0,
        })
        .unwrap();
    }
    spin(&mut app, |a| {
        a.world()
            .resource::<HostLink>()
            .remote_inputs()
            .latest(1)
            .is_some_and(|s| s.input.seq == 12)
    });
    {
        // `With<NetPlayer>` picks the seat out of the rig — the trailer
        // carries `RemotePick` too (it reconciles with its owner).
        let mut q = app
            .world_mut()
            .query_filtered::<&VehicleInput, (With<RemotePick>, With<NetPlayer>)>();
        let input = q.single(app.world()).expect("the remote car's input");
        assert!(
            input.throttle > 0.9,
            "the impaired stream still drove the car: {}",
            input.throttle
        );
    }
    let up = proxy.stats(LinkDir::Up);
    assert!(up.frames_in >= 12, "the lane saw the inputs: {up:?}");
    assert!(
        up.duplicated + up.reordered > 0,
        "the recipe really impaired this leg: {up:?}"
    );

    // Snapshots cross the impaired Down link — duplicated copies and
    // all — and still carry the remote seat's pose.
    until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { entries, .. } if entries.iter().any(|e| e.player == 1)),
    );

    // A reset ask through the storm: copied, delayed, reordered — the
    // mailbox collapses it to one ask and the grant lands once, the
    // seat teleported back to its grid slot with the epoch bump.
    {
        let mut q = app
            .world_mut()
            .query_filtered::<(&mut avian3d::prelude::Position, &mut Transform), With<RemotePick>>(
            );
        let (mut pos, mut transform) = q.single_mut(app.world_mut()).expect("the remote car");
        pos.0 = Vec3::new(30.0, 1.5, -8.0);
        transform.translation = pos.0;
    }
    ctl.request_reset(generation).unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::NetDriveReport>()
            .requests_granted
            == 1
    });
    // Let any duplicate copies drain through — a repeated ask inside
    // the cooldown drops, never re-grants.
    for _ in 0..5 {
        app.update();
        thread::sleep(Duration::from_millis(30));
    }
    {
        let report = app.world().resource::<netdrive::NetDriveReport>();
        assert_eq!(
            report.requests_granted, 1,
            "duplicated asks cannot double-grant"
        );
        let mut q = app
            .world_mut()
            .query_filtered::<(&netdrive::ResetEpoch, &Transform), With<RemotePick>>();
        let (epoch, transform) = q.single(app.world()).expect("the remote car");
        assert_eq!(epoch.0, 1, "one granted ask, one epoch");
        assert!(
            (transform.translation - seated).length() < 0.5,
            "the reset landed back on the seat: {:?} vs {seated:?}",
            transform.translation
        );
    }

    // Back to a clean link for the close — a `Leave` is a control verb
    // whose delivery the leg does not want to lottery.
    proxy.set(LinkDir::Up, Impair::default());
    proxy.set(LinkDir::Down, Impair::default());
    peer.leave().unwrap();
    spin_mut(&mut app, |a| {
        a.world_mut()
            .query_filtered::<(), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_none()
    });
    assert!(
        app.world().get_entity(trailer).is_err(),
        "the trailer copy despawned with its seat"
    );
}

/// One cell of the F25-AC03 impairment matrix: a named [`Impair`]
/// recipe armed on *both* directions for the measured window — the
/// spec's axes are exercised symmetric, and each direction's
/// [`LinkStats`] still reports what actually happened.
struct MatrixCell {
    name: &'static str,
    impair: Impair,
}

/// F25-AC03's measured matrix (the recorded run lives in
/// `docs/research/net.md` under "Measured impairment matrix"). Each
/// cell is a fresh in-process host + client over real loopback through
/// a seeded [`ImpairProxy`]: the lobby crosses clean, the session
/// reaches `Playing`, both directions arm for the measured window, and
/// a fixed run of paired updates moves real `Input` frames up and
/// real `Snap` frames down while the lanes' seeded decisions apply the
/// recipe. A `LinkStats` row per direction plus both
/// `NetDriveReport`s are the cell's measurement.
///
/// Per-cell assertions are floors, not exact counts — traffic volume
/// is update-rate-bound so frame counts drift between runs. The
/// airtight per-frame effects are already proven in halves:
/// `impair`'s lane legs pin the wire emission order (duplicates emit
/// adjacent, a swap emits the held frame behind its successor) and
/// `netdrive`'s push legs pin the watermark drop — so here each cell
/// proves the recipe really fired on the wire *and* the session still
/// converged, while duplicate/reorder cells must additionally land
/// counted stale drops on the client (`snap<x>`).
///
/// The `clean` control row is load-bearing for interpretation: a
/// `Snap` publishes once per Update while `session.tick` only advances
/// per fixed step, so a clean link already drops same-tick
/// republishes at the push watermark — the baseline stale floor every
/// impaired cell is read against, not evidence of wire impairment.
#[test]
fn the_impairment_matrix_records_each_recipe_cell() {
    let cells = [
        // The control: a transparent pair of lanes.
        MatrixCell {
            name: "clean",
            impair: Impair::default(),
        },
        // Latency — a fixed hold every frame pays.
        MatrixCell {
            name: "latency",
            impair: Impair {
                delay: Duration::from_millis(100),
                jitter: Duration::from_millis(20),
                ..Impair::default()
            },
        },
        // Jitter — small fixed hold, wide spread: releases overtake
        // each other, a real reorder source on a lane.
        MatrixCell {
            name: "jitter",
            impair: Impair {
                delay: Duration::from_millis(10),
                jitter: Duration::from_millis(60),
                ..Impair::default()
            },
        },
        // Loss — every fifth frame gone, both ways.
        MatrixCell {
            name: "loss",
            impair: Impair {
                loss: 0.20,
                ..Impair::default()
            },
        },
        // Heavy loss — the "client falls behind"/intermittent-loss
        // edge: six of ten frames never arrive.
        MatrixCell {
            name: "loss-heavy",
            impair: Impair {
                loss: 0.60,
                ..Impair::default()
            },
        },
        // Duplication — every other frame emits a second adjacent
        // copy; the second always lands at-or-behind the watermark.
        MatrixCell {
            name: "duplicate",
            impair: Impair {
                duplicate: 0.50,
                ..Impair::default()
            },
        },
        // Reorder — every other frame swaps with its successor; the
        // held frame always lands behind the newer tick it deferred to.
        MatrixCell {
            name: "reorder",
            impair: Impair {
                reorder: 0.50,
                ..Impair::default()
            },
        },
        // Combined — the recipe the two-process `net_drive` leg runs.
        MatrixCell {
            name: "combined",
            impair: Impair {
                delay: Duration::from_millis(40),
                jitter: Duration::from_millis(30),
                loss: 0.05,
                duplicate: 0.10,
                reorder: 0.10,
            },
        },
    ];
    for (index, cell) in cells.iter().enumerate() {
        run_matrix_cell(cell, index as u64 + 1);
    }
}

/// Run one matrix cell: a fresh hosted session and joined client
/// through a fresh proxy, so lane counters start at zero.
fn run_matrix_cell(cell: &MatrixCell, seed: u64) {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let proxy = ImpairProxy::loopback_seeded(link.addr(), seed).unwrap();
    let mut host = host_app(vfs, link);
    let link = LobbyLink::join(
        proxy.addr(),
        &hello("net-app-test".to_string(), cell.name.to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join through the proxy failed");
    let our_id = link.player_id();
    let mut client = bridge_app(mount(install.path()), link);
    {
        let link = client.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut client);
    hosted_playing(&mut host);
    until_begun(&mut client);
    {
        let mut session = client.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    // The driver's own seat, declared the way
    // `a_client_streams_inputs_and_applies_the_host_snapshot` does —
    // the fixture app's load systems never spawn it, and
    // `send_drive_input` needs a settled `VehicleInput` to stream.
    client.world_mut().spawn((
        PlayerVehicle,
        Player {
            id: mm2_game::PlayerId(our_id),
            control: PlayerControl::Local,
        },
        mm2_game::AuthorityRole::Predicted,
        VehicleInput {
            throttle: 0.5,
            ..VehicleInput::default()
        },
        avian3d::prelude::Position::default(),
        avian3d::prelude::Rotation::default(),
        avian3d::prelude::LinearVelocity::default(),
        avian3d::prelude::AngularVelocity::default(),
    ));
    // The host seat reconciles into a remote copy — the apply target
    // the snapshot stream drives.
    spin_mut(&mut client, |a| {
        a.world_mut()
            .query_filtered::<(), With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });

    // The measured window. Both directions arm together — the lobby
    // phase crossed clean, so only data-plane frames (`Input` up,
    // `Snap` down) pay the recipe. Paired real-time updates let the
    // lanes' release schedules fire while the session runs.
    proxy.set(LinkDir::Up, cell.impair);
    proxy.set(LinkDir::Down, cell.impair);
    for _ in 0..150 {
        host.update();
        client.update();
        thread::sleep(Duration::from_millis(4));
    }
    // Settle until every scheduled release is provably past: the
    // deepest hold a frame can still owe is delay + jitter plus the
    // reorder stall bound `HOLD_CAP` — at most 220 ms for these recipes.
    // Doubling it drains the tail and lets the client's pump push what
    // arrived before the counters are read.
    for _ in 0..100 {
        host.update();
        client.update();
        thread::sleep(Duration::from_millis(4));
    }

    let up = proxy.stats(LinkDir::Up);
    let down = proxy.stats(LinkDir::Down);
    let (snaps_applied, snaps_staled, inputs_sent, remotes, resets) = {
        let r = client.world().resource::<netdrive::NetDriveReport>();
        (
            r.snaps_applied,
            r.snaps_staled,
            r.inputs_sent,
            r.remotes,
            r.resets,
        )
    };
    let (inputs_applied, inputs_staled, snaps_sent) = {
        let r = host.world().resource::<netdrive::NetDriveReport>();
        (r.inputs_applied, r.inputs_staled, r.snaps_sent)
    };
    // The cell's record line — one measured row per recipe; the doc's
    // matrix table is harvested from these.
    eprintln!(
        "matrix cell={} seed={seed} snap={snaps_sent}s/{snaps_applied}a/{snaps_staled}x \
         input={inputs_sent}s/{inputs_applied}a/{inputs_staled}x rem={remotes} resets={resets} \
         up={up:?} down={down:?}",
        cell.name,
    );

    // Convergence — every cell's data plane still moved state both ways.
    assert!(
        up.frames_in > 0 && down.frames_in > 0,
        "cell {} moved no data-plane frames: {up:?} {down:?}",
        cell.name,
    );
    assert!(
        snaps_sent > 0 && snaps_applied > 0,
        "cell {} applied no snapshot: sent={snaps_sent} applied={snaps_applied}",
        cell.name,
    );
    assert!(
        inputs_sent > 0 && inputs_applied > 0,
        "cell {} drove nothing: sent={inputs_sent} applied={inputs_applied}",
        cell.name,
    );
    assert!(
        remotes >= 1,
        "cell {} never reconciled the host seat",
        cell.name
    );
    assert_eq!(
        up.overflowed + down.overflowed,
        0,
        "cell {} overflowed a lane: {up:?} {down:?}",
        cell.name,
    );

    // Each armed knob must show in the counter it claims to turn.
    let impair = cell.impair;
    if impair.delay > Duration::ZERO || impair.jitter > Duration::ZERO {
        assert!(
            up.delayed > 0 && down.delayed > 0,
            "cell {} scheduled no delay holds: {up:?} {down:?}",
            cell.name,
        );
    }
    if impair.loss >= 0.10 {
        assert!(
            up.dropped > 0 && down.dropped > 0,
            "cell {} dropped nothing: {up:?} {down:?}",
            cell.name,
        );
    }
    if impair.duplicate > 0.0 {
        assert!(
            up.duplicated > 0 && down.duplicated > 0,
            "cell {} duplicated nothing: {up:?} {down:?}",
            cell.name,
        );
        // Every duplicated `Snap` copy pushes at-or-behind the
        // watermark — a counted stale drop.
        assert!(
            snaps_staled > 0,
            "cell {} recorded no stale drop off duplicated snaps",
            cell.name,
        );
    }
    if impair.reorder > 0.0 {
        assert!(
            up.reordered > 0 && down.reordered > 0,
            "cell {} reordered nothing: {up:?} {down:?}",
            cell.name,
        );
        // A swap's held frame is older than the successor it lands
        // behind — it can never displace the newer staged pose.
        assert!(
            snaps_staled > 0,
            "cell {} recorded no stale drop off reordered snaps",
            cell.name,
        );
    }
    // The control cell: a transparent lane impairs nothing — every
    // recipe counter stays at zero, so `snaps_staled` on this row is
    // publish-cadence dedup only, the floor the others are read
    // against.
    if impair == Impair::default() {
        assert_eq!(
            up.delayed + up.dropped + up.duplicated + up.reordered,
            0,
            "cell {} impaired a clean lane: {up:?}",
            cell.name,
        );
        assert_eq!(
            down.delayed + down.dropped + down.duplicated + down.reordered,
            0,
            "cell {} impaired a clean lane: {down:?}",
            cell.name,
        );
    }

    // A polite close per cell — disarm so `Leave` isn't itself
    // impaired, let it cross, then drop the lane set.
    proxy.set(LinkDir::Up, Impair::default());
    proxy.set(LinkDir::Down, Impair::default());
    client.world_mut().resource_mut::<LobbyLink>().leave();
    for _ in 0..10 {
        host.update();
        client.update();
        thread::sleep(Duration::from_millis(4));
    }
}

/// F25-B repair leg, the v14 candidate's blocking finding over the
/// real path: the authority's own `Playing → Results` used to kill both
/// producers a racing remote client lives on — `advance_race`
/// early-returns outside `Playing`, `publish_snapshots` outside the
/// live phases — so a host-first finish or the event deadline stranded
/// every unresolved client on a dead stream. The repaired contract
/// holds the hosted session in `Playing` until every `Remote` wire
/// seat resolves, then owes the wire exactly one `Results`-phase snap
/// at the frozen transition tick. Both halves run real systems here —
/// the hosted app with `advance_race` in `FixedLast` like `main.rs`
/// schedules it, the joined client through `apply_snapshots` — nothing
/// stages a `Snap` by hand.
#[test]
fn the_deferred_authority_delivers_the_wire_seats_terminal_edge() {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut host = host_app(vfs, link);
    // The real race driver on the fixed step, scheduled like
    // `main.rs` does — the deferral and the deadline both live on
    // this system's stepping. One fixed step per update keeps the
    // clock's arrival order deterministic.
    host.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    host.add_systems(
        FixedLast,
        (
            mm2_app::race::reanchor_teleported_participants,
            mm2_app::race::advance_race,
        )
            .chain(),
    );

    // The joined client — the same bridge wiring `mm2 --join` runs.
    let link = LobbyLink::join(
        addr,
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut client = bridge_app(mount(install.path()), link);
    {
        let link = client.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut client);

    // The operator's start — `drive_host` mints the generation on the
    // host half, the `Start` frame begins the client's session.
    host.world()
        .resource::<HostLink>()
        .command_sender()
        .send(HostCommand::Start)
        .unwrap();
    spin(&mut host, |a| {
        a.world().resource::<Session>().config().is_some()
    });
    until_begun(&mut client);

    // Stand both halves live under a running, deadline-bearing race —
    // the `Ready → Playing` hop the load legs take, plus the
    // `RaceState` the event producer inserts. `wire_race_def`'s single
    // checkpoint is the host seat's finish; its `time_limit` is the
    // parked remote seat's only resolution.
    let mut def = wire_race_def(0);
    def.time_limit_ticks = Some(40);
    for app in [&mut host, &mut client] {
        let generation = {
            let mut session = app.world_mut().resource_mut::<Session>();
            session.transition(SessionPhase::Ready).unwrap();
            session.transition(SessionPhase::Playing).unwrap();
            session.generation()
        };
        let mut race = mm2_game::RaceState::new(def.clone(), generation);
        race.phase = mm2_game::RacePhase::Running;
        app.world_mut().insert_resource(race);
    }

    // The host's own seat — the shape `load_session_world` leaves it:
    // wire id 0, `Local` control, `Racing` under a running race.
    let mut seat_progress = mm2_game::RaceProgress::new(&def);
    seat_progress.state = mm2_game::ParticipantState::Racing;
    let host_seat = host
        .world_mut()
        .spawn((
            NetPlayer(0),
            Player {
                id: mm2_game::PlayerId(0),
                control: PlayerControl::Local,
            },
            netdrive::ResetEpoch(0),
            mm2_game::ObjectIdentity(mm2_game::ObjectId {
                generation: 1,
                slot: 100,
            }),
            seat_progress,
            avian3d::prelude::Position(Vec3::new(0.0, 0.0, -100.0)),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();

    // The client's own seat — `PlayerVehicle`-marked so the reconcile
    // stamps its `NetPlayer`, and `RaceProgress`-tracked like the
    // load path leaves it.
    let mut client_progress = mm2_game::RaceProgress::new(&def);
    client_progress.state = mm2_game::ParticipantState::Racing;
    let client_seat = client
        .world_mut()
        .spawn((
            PlayerVehicle,
            Player {
                id: mm2_game::PlayerId(1),
                control: PlayerControl::Local,
            },
            mm2_game::AuthorityRole::Predicted,
            client_progress,
            avian3d::prelude::Position::default(),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();
    spin(&mut client, |a| {
        a.world().get::<NetPlayer>(client_seat).is_some()
    });

    // The wire seat lands through the real reconcile — spawned into a
    // running race, `RaceProgress::join` scores it `Racing` on arrival
    // rather than stranding `AwaitingStart` past the missed release
    // flip.
    spin_mut(&mut host, |a| {
        a.world_mut()
            .query_filtered::<&mm2_game::RaceProgress, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some_and(|p| p.state == mm2_game::ParticipantState::Racing)
    });
    // The client meanwhile holds the authority's copy — same reconcile.
    spin_mut(&mut client, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });

    // The authority's own seat finishes first — the swept segment
    // through the lone checkpoint mints its result on the wire clock.
    host.update(); // anchor `last_position`
    *host
        .world_mut()
        .get_mut::<avian3d::prelude::Position>(host_seat)
        .unwrap() = avian3d::prelude::Position(Vec3::new(0.0, 0.0, -300.0));
    spin_mut(&mut host, |a| {
        matches!(
            a.world()
                .get::<mm2_game::RaceProgress>(host_seat)
                .map(|p| &p.state),
            Some(mm2_game::ParticipantState::Finished { .. })
        )
    });

    // The deferral: the host holds `Playing` — progress stepping and
    // the snap stream stay alive for the racing wire seat. The client
    // keeps applying frames through the window.
    let applied_at_finish = client
        .world()
        .resource::<netdrive::NetDriveReport>()
        .snaps_applied;
    for _ in 0..6 {
        host.update();
        client.update();
    }
    assert_eq!(
        session_phase(&host),
        SessionPhase::Playing,
        "the authority's own finish must not strand a racing wire seat"
    );
    assert_eq!(
        session_phase(&client),
        SessionPhase::Playing,
        "the wire's word leaves the racing client's session alone"
    );
    assert!(
        client
            .world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_applied
            > applied_at_finish,
        "the snap stream outlived the authority's own finish"
    );

    // The deadline resolves the parked wire seat — the mass `TimedOut`
    // mint and the `Playing → Results` transition land in one
    // `advance_race` step, so the snap stamped with that tick is the
    // terminal row's only carrier. (The clock is set to the limit's
    // edge rather than idled there — the deferral window above is the
    // behavior under test.)
    host.world_mut().resource_mut::<mm2_game::RaceState>().clock = 37;
    spin(&mut host, |a| session_phase(a) == SessionPhase::Results);
    assert_eq!(
        host.world().resource::<mm2_game::RaceState>().phase,
        mm2_game::RacePhase::Complete
    );

    // The owed `Results`-phase frame publishes exactly once — the
    // publisher does not stream a quiescent phase.
    let sent_at_results = host
        .world()
        .resource::<netdrive::NetDriveReport>()
        .snaps_sent;
    for _ in 0..4 {
        host.update();
    }
    assert_eq!(
        host.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_sent,
        sent_at_results,
        "the Results debt is the one unpublished transition frame"
    );

    // And it lands: the client's own terminal edge mints the same
    // `TimedOut` `advance_race` recorded on the authority and ends its
    // `Playing` — the stranded-client hang is the regression this
    // leg exists to kill.
    spin(&mut client, |a| session_phase(a) == SessionPhase::Results);
    {
        let state = &client
            .world()
            .get::<mm2_game::RaceProgress>(client_seat)
            .unwrap()
            .state;
        assert!(
            matches!(state, mm2_game::ParticipantState::TimedOut { .. }),
            "expected the replicated TimedOut, got {state:?}"
        );
        let ledger = client.world().resource::<mm2_game::ResultLedger>();
        assert!(
            ledger
                .iter()
                .any(|r| matches!(r.outcome, mm2_game::SessionOutcome::TimedOut { .. })),
            "the wire's word minted the client's own result row"
        );
    }
}

/// Shared staging for the deferral's endpoint legs — the same build
/// `the_deferred_authority_delivers_the_wire_seats_terminal_edge`
/// runs, stopped inside the held window: the host's own seat has
/// `Finished`, the joined client's wire seat still races (no deadline
/// — departure and death are the only releases these legs exercise),
/// and both sessions sit in `Playing` on a live snap stream. The
/// install rides the tuple because both mounts must outlive the apps.
fn staged_deferral() -> (tempfile::TempDir, App, App, Entity, Entity) {
    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut host = host_app(vfs, link);
    host.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    host.add_systems(
        FixedLast,
        (
            mm2_app::race::reanchor_teleported_participants,
            mm2_app::race::advance_race,
        )
            .chain(),
    );

    let link = LobbyLink::join(
        addr,
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut client = bridge_app(mount(install.path()), link);
    {
        let link = client.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut client);

    host.world()
        .resource::<HostLink>()
        .command_sender()
        .send(HostCommand::Start)
        .unwrap();
    spin(&mut host, |a| {
        a.world().resource::<Session>().config().is_some()
    });
    until_begun(&mut client);

    // Both halves live under a running, deadline-free race — the
    // endpoint under test is the only way out of the deferral.
    let def = wire_race_def(0);
    for app in [&mut host, &mut client] {
        let generation = {
            let mut session = app.world_mut().resource_mut::<Session>();
            session.transition(SessionPhase::Ready).unwrap();
            session.transition(SessionPhase::Playing).unwrap();
            session.generation()
        };
        let mut race = mm2_game::RaceState::new(def.clone(), generation);
        race.phase = mm2_game::RacePhase::Running;
        app.world_mut().insert_resource(race);
    }

    // The host's own seat — wire id 0, `Local`, `Racing`.
    let mut seat_progress = mm2_game::RaceProgress::new(&def);
    seat_progress.state = mm2_game::ParticipantState::Racing;
    let host_seat = host
        .world_mut()
        .spawn((
            NetPlayer(0),
            Player {
                id: mm2_game::PlayerId(0),
                control: PlayerControl::Local,
            },
            netdrive::ResetEpoch(0),
            mm2_game::ObjectIdentity(mm2_game::ObjectId {
                generation: 1,
                slot: 100,
            }),
            seat_progress,
            avian3d::prelude::Position(Vec3::new(0.0, 0.0, -100.0)),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();

    // The client's own seat — `PlayerVehicle` so the reconcile stamps
    // its `NetPlayer`, `RaceProgress`-tracked like the load leaves it.
    let mut client_progress = mm2_game::RaceProgress::new(&def);
    client_progress.state = mm2_game::ParticipantState::Racing;
    let client_seat = client
        .world_mut()
        .spawn((
            PlayerVehicle,
            Player {
                id: mm2_game::PlayerId(1),
                control: PlayerControl::Local,
            },
            mm2_game::AuthorityRole::Predicted,
            client_progress,
            avian3d::prelude::Position::default(),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();
    spin(&mut client, |a| {
        a.world().get::<NetPlayer>(client_seat).is_some()
    });

    // The wire seat lands through the real reconcile — `Racing` on
    // arrival under `RaceProgress::join`'s mid-race semantics.
    spin_mut(&mut host, |a| {
        a.world_mut()
            .query_filtered::<&mm2_game::RaceProgress, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some_and(|p| p.state == mm2_game::ParticipantState::Racing)
    });
    // The client holds the authority's copy through the same reconcile.
    spin_mut(&mut client, |a| {
        a.world_mut()
            .query_filtered::<Entity, With<RemotePick>>()
            .iter(a.world())
            .next()
            .is_some()
    });

    // The authority's own seat finishes — the deferral holds `Playing`
    // for the racing wire seat on both processes.
    host.update(); // anchor `last_position`
    *host
        .world_mut()
        .get_mut::<avian3d::prelude::Position>(host_seat)
        .unwrap() = avian3d::prelude::Position(Vec3::new(0.0, 0.0, -300.0));
    spin_mut(&mut host, |a| {
        matches!(
            a.world()
                .get::<mm2_game::RaceProgress>(host_seat)
                .map(|p| &p.state),
            Some(mm2_game::ParticipantState::Finished { .. })
        )
    });
    for _ in 0..3 {
        host.update();
        client.update();
    }
    assert_eq!(
        session_phase(&host),
        SessionPhase::Playing,
        "staging must leave the authority in the deferred Playing"
    );
    assert_eq!(
        session_phase(&client),
        SessionPhase::Playing,
        "staging must leave the client racing on the wire's word"
    );
    (install, host, client, host_seat, client_seat)
}

/// The deferral's departure edge over the real loopback path: the
/// wire seat's owner leaving mid-deferral drops its roster slot, the
/// reconcile despawns the participant, and the despawn — not a
/// replicated resolution — releases the host's held `Playing`. A quit
/// inside the window would otherwise strand the hosted session
/// exactly like a stalled seat.
#[test]
fn a_departing_wire_seat_releases_the_deferred_authority() {
    let (_install, mut host, mut client, _host_seat, _client_seat) = staged_deferral();

    // The wire seat's owner says goodbye.
    client.world_mut().resource_mut::<LobbyLink>().leave();

    spin(&mut host, |a| session_phase(a) == SessionPhase::Results);
    assert_eq!(
        host.world().resource::<mm2_game::RaceState>().phase,
        mm2_game::RacePhase::Complete
    );
    assert!(
        host.world()
            .resource::<netdrive::NetDriveReport>()
            .despawned
            >= 1,
        "the departed seat left through the reconcile's despawn, not a resolution"
    );

    // The transition still owes its one `Results`-phase frame — to an
    // empty wire now, but the debt bounds the stream the same.
    let sent_at_results = host
        .world()
        .resource::<netdrive::NetDriveReport>()
        .snaps_sent;
    for _ in 0..4 {
        host.update();
    }
    assert_eq!(
        host.world()
            .resource::<netdrive::NetDriveReport>()
            .snaps_sent,
        sent_at_results,
        "the Results debt stays exactly one unpublished transition frame"
    );
}

/// The stranded client's recovery over the same staging: a client
/// still `Playing` inside the deferral window when its host dies must
/// not sit on the dead stream — the link's `Closed` takes the session
/// down through the normal lifecycle and the app exits nonzero. The
/// bare-`Playing` teardown leg covers the mechanism; this leg covers
/// the stranded-in-deferral case it exists for.
#[test]
fn a_stranded_client_recovers_when_the_host_dies() {
    let (_install, host, mut client, _host_seat, _client_seat) = staged_deferral();
    assert_eq!(
        session_phase(&client),
        SessionPhase::Playing,
        "the staging leaves the client mid-deferral"
    );

    // The host process dies inside the window — the wire seat the
    // client was racing can never resolve now.
    host.world()
        .resource::<HostLink>()
        .ctl()
        .shutdown()
        .unwrap();

    let exit = until_exit(&mut client);
    assert!(
        matches!(exit, AppExit::Error(code) if code.get() == 1),
        "a lost host is a nonzero exit, got {exit:?}"
    );
    assert_eq!(session_phase(&client), SessionPhase::Menu);
    let lobby = client.world().resource::<LobbyState>();
    assert!(
        lobby
            .notice
            .as_deref()
            .is_some_and(|n| n.contains("lost the host")),
        "the notice names the cause: {:?}",
        lobby.notice
    );
}

/// F25-B: the deferral's stall edge over the real loopback path — a
/// wire seat whose input stream went live this generation and then
/// died inside the deferral window is retired with the deadline's own
/// `TimedOut` mint: the host's held `Playing` releases through the
/// ordinary "every wire seat resolved" edge, the owed `Results` frame
/// carries the seat's terminal tail, and the still-live client mints
/// its own result off it. Without the watchdog this exact staging
/// held `Playing` forever — the named gap iters 7–10 deferred.
#[test]
fn a_stalled_wire_seat_is_retired_and_releases_the_deferred_authority() {
    let (_install, mut host, mut client, _host_seat, client_seat) = staged_deferral();
    // Designed bounds short enough to wait out — production defaults
    // are 10 s live / 120 s grace.
    host.world_mut().insert_resource(netdrive::WireStall {
        live_silence: Duration::from_millis(200),
        join_grace: Duration::from_secs(60),
    });
    let generation = client.world().resource::<Session>().wire_generation();

    // First half of the leg — a stream that stays live holds the
    // deferral open past the silence bound: the retirement is about
    // *stopped* streams, not unresolved seats. `seq` climbs because
    // the mailbox's latest-wins store only refreshes `received` on an
    // accepted newer sample.
    let mut seq = 0u64;
    let hold_until = std::time::Instant::now() + Duration::from_millis(450);
    while std::time::Instant::now() < hold_until {
        seq += 1;
        client
            .world()
            .resource::<LobbyLink>()
            .ctl()
            .send_input(DriveInput {
                generation,
                seq,
                throttle: 0,
                brake: 0,
                steer: 0,
                handbrake: 0,
            })
            .unwrap();
        host.update();
        client.update();
        thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(
        session_phase(&host),
        SessionPhase::Playing,
        "a stream that kept streaming held the deferral past the bound"
    );
    assert_eq!(
        host.world()
            .resource::<netdrive::NetDriveReport>()
            .wire_seats_retired,
        0,
        "a live seat is never retired"
    );

    // The stream dies mid-deferral — the client keeps draining snaps
    // and lobby traffic like a wedged sim would, only its input feed
    // is gone. Silence past `live_silence` retires the seat: a
    // `TimedOut` mint, not a despawn and not a kick.
    spin(&mut host, |a| session_phase(a) == SessionPhase::Results);
    assert_eq!(
        host.world().resource::<mm2_game::RaceState>().phase,
        mm2_game::RacePhase::Complete
    );
    assert!(
        host.world()
            .resource::<netdrive::NetDriveReport>()
            .wire_seats_retired
            >= 1,
        "the stalled seat was retired, not departed"
    );
    assert_eq!(
        host.world()
            .resource::<netdrive::NetDriveReport>()
            .despawned,
        0,
        "a retirement keeps the seat — the parked car stays"
    );
    {
        let mut q = host
            .world_mut()
            .query_filtered::<&mm2_game::RaceProgress, With<RemotePick>>();
        let progress = q.single(host.world()).expect("the wire seat");
        assert!(
            matches!(progress.state, mm2_game::ParticipantState::TimedOut { .. }),
            "the retirement mints the deadline's own terminal edge: {:?}",
            progress.state
        );
    }

    // The owed `Results` frame delivers the seat's terminal tail —
    // the client mints its own `TimedOut` row and resolves through
    // the same replicated-edge path the deadline's legs exercise.
    spin(&mut client, |a| session_phase(a) == SessionPhase::Results);
    assert!(matches!(
        client
            .world()
            .get::<mm2_game::RaceProgress>(client_seat)
            .unwrap()
            .state,
        mm2_game::ParticipantState::TimedOut { .. }
    ));
    assert!(
        client
            .world()
            .resource::<mm2_game::ResultLedger>()
            .iter()
            .any(|r| matches!(r.outcome, mm2_game::SessionOutcome::TimedOut { .. })),
        "the wire's word minted the stalled seat's row client-side"
    );
}

/// The never-live arm of the same watchdog: a wire seat that never
/// produced a generation-matching sample — a joiner wedged mid-load
/// is indistinguishable from one until it streams — gets the longer
/// `join_grace` bound instead of `live_silence`, then retires the
/// same way. A retirement is still a result, not a kick: the roster
/// slot and link stay.
#[test]
fn a_never_live_wire_seat_is_retired_after_the_join_grace() {
    let (_install, mut host, _client, _host_seat, _client_seat) = staged_deferral();
    // The staged client's seat never streamed (its local car carries
    // no `VehicleInput` for `send_drive_input` to send), so the
    // mailbox slot is empty — the never-live arm. The watchdog ran
    // under the production defaults through the whole staging above,
    // so the seat's `first_seen` observation is already older than a
    // test-sized grace: a grace under that elapsed observation
    // retires it next pass. The `Playing` the staging asserted is
    // the inside-grace half — the default 120 s never came close.
    host.world_mut().insert_resource(netdrive::WireStall {
        live_silence: Duration::from_secs(60),
        join_grace: Duration::from_millis(1),
    });

    spin(&mut host, |a| session_phase(a) == SessionPhase::Results);
    assert!(
        host.world()
            .resource::<netdrive::NetDriveReport>()
            .wire_seats_retired
            >= 1,
        "past grace the never-live seat retires too"
    );
    assert_eq!(
        host.world()
            .resource::<netdrive::NetDriveReport>()
            .despawned,
        0,
        "a retirement keeps the seat — the parked car stays"
    );
}

/// F25-B repair leg, the reviewer's wider variant over the real
/// loopback path: a `LateJoin::Open` joiner whose first applied frame
/// is the owed `Results`-phase one. While the session is still
/// `Loading` the staged snap holds — its seat rows have no entities
/// and its race row no `RaceState` yet, so a consumed frame would
/// strand the joiner exactly like the swallowed terminal edge did.
/// The first live update applies it whole: the reconcile's
/// `NetPlayer` stamp lands ahead of the apply in the same update,
/// the local tail mints while still `Countdown`, the `Complete` row
/// releases, and the second look resolves the session to `Results`
/// off the one frame.
#[test]
fn a_joiners_held_snap_resolves_the_terminal_edge_after_the_load() {
    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let mut host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let our_id = link.player_id();
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app); // Start → begin_generation → Loading
    assert_eq!(session_phase(&app), SessionPhase::Loading);
    let generation = app.world().resource::<Session>().wire_generation();

    // The owed `Results`-phase frame lands mid-load: our seat's
    // `TimedOut` tail plus the `Complete` race row — the only frame a
    // dead stream carries.
    let mut own_row = SnapEntry {
        player: our_id,
        ..SnapEntry::default()
    };
    own_row.prog_state = 3; // timed out
    own_row.prog_ticks = 40;
    host.ctl()
        .broadcast(&Message::Snap {
            generation,
            tick: 7,
            entries: vec![own_row],
            trailers: Vec::new(),
            impacts: Vec::new(),
            race: Some(mm2_net::SnapRace {
                phase: 2, // complete
                countdown: 0,
                clock: 40,
            }),
        })
        .unwrap();
    // It drains off the wire and holds — a `Loading` session
    // consumes nothing: no pose apply, no raceless race-row drop, no
    // skipped terminal mint.
    for _ in 0..4 {
        app.update();
    }
    {
        let report = app.world().resource::<netdrive::NetDriveReport>();
        assert_eq!(report.snaps_applied, 0, "the load consumed nothing");
        assert_eq!(
            report.race_dropped, 0,
            "the race row held rather than dropping raceless"
        );
        assert_eq!(
            app.world().resource::<mm2_game::ResultLedger>().len(),
            0,
            "no tail landed — it arrives whole below"
        );
    }

    // The load's end — `Ready → Countdown` plus the event's
    // `RaceState` and the local seat the spawn leaves, staged by
    // hand here (the load legs need the asset stack). One live
    // update delivers the whole held frame: the reconcile's
    // `NetPlayer` stamp lands before the apply through the ordering
    // edge, the tail mints, the `Complete` row releases
    // `Countdown → Playing`, and the second look ends the session.
    {
        let local_generation = {
            let mut session = app.world_mut().resource_mut::<Session>();
            session.transition(SessionPhase::Ready).unwrap();
            session.transition(SessionPhase::Countdown).unwrap();
            session.generation()
        };
        app.world_mut().insert_resource(mm2_game::RaceState::new(
            wire_race_def(180),
            local_generation,
        ));
    }
    let local = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            Player {
                id: mm2_game::PlayerId(1),
                control: PlayerControl::Local,
            },
            mm2_game::AuthorityRole::Predicted,
            mm2_game::RaceProgress::new(&wire_race_def(180)),
            avian3d::prelude::Position::default(),
            avian3d::prelude::Rotation::default(),
            avian3d::prelude::LinearVelocity::default(),
            avian3d::prelude::AngularVelocity::default(),
        ))
        .id();
    app.update();
    assert!(
        app.world().get::<NetPlayer>(local).is_some(),
        "the reconcile stamped the wire id the snap names"
    );
    assert_eq!(
        session_phase(&app),
        SessionPhase::Results,
        "the held frame delivered its terminal edge — not a stranded `Playing`"
    );
    assert!(matches!(
        app.world()
            .get::<mm2_game::RaceProgress>(local)
            .unwrap()
            .state,
        mm2_game::ParticipantState::TimedOut { race_ticks: 40, .. }
    ));
    {
        let ledger = app.world().resource::<mm2_game::ResultLedger>();
        assert_eq!(ledger.len(), 1);
        assert_eq!(
            ledger.iter().next().unwrap().outcome,
            mm2_game::SessionOutcome::TimedOut { race_ticks: 40 },
            "the wire's word minted the client's own result row"
        );
    }

    host.shutdown();
}

// ── F26-A: replicated world props (protocol v17) ────────────────────

/// A distilled banger record for the world-prop legs.
fn prop_def(name: &str) -> mm2_game::BangerDefinition {
    mm2_game::BangerDefinition {
        name: name.into(),
        mass: 40.0,
        friction: 0.9,
        elasticity: 0.5,
        impulse_limit2: 100.0,
        size: [0.5, 0.5, 0.5],
        cg: [0.0, 0.0, 0.0],
        num_parts: 0,
        audio_id: 0,
    }
}

/// Stamp one banger placement the way `city::spawn_banger_prop` does —
/// the shared bundle plus the next [`mm2_game::BangerSite`] ordinal —
/// at `phase` and `pos`. `pieces` are the collidable `BREAK<NN>` pieces
/// the placement would carry (none for a plain prop).
fn stamp_prop(app: &mut App, phase: mm2_game::BangerPhase, pos: Vec3, pieces: usize) -> Entity {
    use avian3d::prelude::{Collider, Position, Rotation};
    let (object, role, site, owner) = {
        let mut session = app.world_mut().resource_mut::<Session>();
        (
            session.mint_object_id(),
            session.authority_role(),
            session.mint_banger_site(),
            mm2_game::SessionEntity(session.generation()),
        )
    };
    let mut banger = mm2_game::Banger::new(prop_def("prop"));
    banger.phase = phase;
    let entity = app
        .world_mut()
        .spawn(mm2_app::banger::banger_bundle(
            banger,
            object,
            role,
            owner,
            Collider::cuboid(1.0, 1.0, 1.0),
            Transform::from_translation(pos),
            format!("prop-{}", site.0),
        ))
        .insert((site, Position(pos), Rotation(Quat::IDENTITY)))
        .id();
    if pieces > 0 {
        app.world_mut()
            .entity_mut(entity)
            .insert(mm2_app::banger::BangerPieces {
                fragments: (0..pieces)
                    .map(|i| mm2_app::banger::FragmentPiece {
                        index: format!("{i:02}"),
                        def: prop_def("piece"),
                        parts: Vec::new(),
                        collider: Some(Collider::cuboid(0.5, 0.5, 0.5)),
                    })
                    .collect(),
            });
    }
    entity
}

/// The prop-state a leg asserts on: phase, body kind, pose.
fn prop_state(
    app: &App,
    entity: Entity,
) -> (
    mm2_game::BangerPhase,
    Option<avian3d::prelude::RigidBody>,
    Vec3,
) {
    let world = app.world();
    (
        world.get::<mm2_game::Banger>(entity).unwrap().phase,
        world.get::<avian3d::prelude::RigidBody>(entity).copied(),
        world.get::<avian3d::prelude::Position>(entity).unwrap().0,
    )
}

/// A joined client in `Playing` against a bare host — the harness the
/// client-side prop legs share. The dev world stamps no props, so each
/// leg stamps its own.
fn playing_client(install: &std::path::Path) -> (Host, App, u64) {
    let vfs = mount(install);
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    (host, app, generation)
}

/// The table a world of these `(name, home)` placements, stamped in
/// order, has — what a host that stamped them would send.
fn table_of(world: &[(&str, Vec3)]) -> mm2_net::SiteTable {
    let mut registry = mm2_app::worldprops::SiteRegistry::default();
    registry.begin(0);
    for (site, (name, home)) in world.iter().enumerate() {
        registry.note(site as u32, name, *home);
    }
    registry.refresh();
    registry.table()
}

/// The client's own stamped-world table — what a host that stamped the
/// same world would send alongside its rows.
fn local_table(app: &App) -> mm2_net::SiteTable {
    app.world()
        .resource::<netdrive::RemoteSnaps>()
        .props()
        .local_table()
}

fn props_frame(
    table: mm2_net::SiteTable,
    generation: u64,
    tick: u64,
    rows: Vec<mm2_net::SnapProp>,
) -> Message {
    Message::Props {
        generation,
        tick,
        table,
        rows,
    }
}

fn prop_row(site: u32, fragment: u8, phase: u8, pos: [f32; 3]) -> mm2_net::SnapProp {
    mm2_net::SnapProp {
        site,
        fragment,
        phase,
        pos,
        rot: [0.0, 0.0, 0.0, 1.0],
    }
}

/// F26-A: the client folds the host's prop rows into its own stamped
/// world — and refuses everything it should. Active drives a kinematic
/// pose, Settled is a static collider at the row's pose, a fragment row
/// proves its placement shattered and spawns the piece from the
/// placement's authored pieces, phases never regress under a reordered
/// row, and a row naming nothing (unknown site, unreadable phase,
/// foreign generation, fragment past the authored pieces) resolves to
/// nothing — counted, never applied.
#[test]
fn a_client_folds_prop_rows_into_its_stamped_world() {
    use avian3d::prelude::RigidBody;
    use mm2_app::worldprops::{PROP_ACTIVE, PROP_BROKEN, PROP_SETTLED};
    use mm2_game::BangerPhase;

    let install = tempfile::tempdir().unwrap();
    let (mut host, mut app, generation) = playing_client(install.path());
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    let knocked = stamp_prop(&mut app, BangerPhase::Dormant, Vec3::new(10.0, 0.0, 0.0), 0);
    let shattered = stamp_prop(&mut app, BangerPhase::Dormant, Vec3::new(20.0, 0.0, 0.0), 2);
    let untouched = stamp_prop(&mut app, BangerPhase::Dormant, Vec3::new(30.0, 0.0, 0.0), 0);
    app.update();
    let landed = |a: &App| {
        a.world()
            .resource::<netdrive::RemoteSnaps>()
            .props()
            .landed()
    };
    let unresolved = |a: &App| {
        a.world()
            .resource::<netdrive::RemoteSnaps>()
            .props()
            .unresolved()
    };

    // Active: kinematic, pose from the wire.
    host.ctl()
        .broadcast(&props_frame(
            local_table(&app),
            generation,
            5,
            vec![prop_row(
                0,
                mm2_net::SNAP_NO_FRAGMENT,
                PROP_ACTIVE,
                [10.0, 1.5, 0.5],
            )],
        ))
        .unwrap();
    spin(&mut app, |a| landed(a) >= 1);
    app.update();
    assert_eq!(
        prop_state(&app, knocked),
        (
            BangerPhase::Active,
            Some(RigidBody::Kinematic),
            Vec3::new(10.0, 1.5, 0.5)
        )
    );

    // Settled: a static collider at the resting pose.
    host.ctl()
        .broadcast(&props_frame(
            local_table(&app),
            generation,
            9,
            vec![prop_row(
                0,
                mm2_net::SNAP_NO_FRAGMENT,
                PROP_SETTLED,
                [11.0, 0.0, 2.0],
            )],
        ))
        .unwrap();
    spin(&mut app, |a| landed(a) >= 2);
    app.update();
    assert_eq!(
        prop_state(&app, knocked),
        (
            BangerPhase::Settled,
            Some(RigidBody::Static),
            Vec3::new(11.0, 0.0, 2.0)
        )
    );

    // A reordered older Active row is stale — dropped counted at the
    // inbox, so the settled prop stays put.
    let stale_before = app
        .world()
        .resource::<netdrive::RemoteSnaps>()
        .props()
        .stale();
    host.ctl()
        .broadcast(&props_frame(
            local_table(&app),
            generation,
            6,
            vec![prop_row(
                0,
                mm2_net::SNAP_NO_FRAGMENT,
                PROP_ACTIVE,
                [99.0, 9.0, 9.0],
            )],
        ))
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::RemoteSnaps>()
            .props()
            .stale()
            > stale_before
    });
    assert_eq!(
        prop_state(&app, knocked).0,
        BangerPhase::Settled,
        "a reordered Active row cannot un-settle the prop"
    );

    // A fragment row for a placement the client still holds dormant:
    // the placement shatters (collider and body gone) and the piece
    // spawns from its authored pieces, kinematic at the wire pose.
    host.ctl()
        .broadcast(&props_frame(
            local_table(&app),
            generation,
            12,
            vec![prop_row(1, 1, PROP_ACTIVE, [20.0, 2.0, 1.0])],
        ))
        .unwrap();
    spin(&mut app, |a| landed(a) >= 3);
    app.update();
    {
        let world = app.world();
        assert_eq!(
            world.get::<mm2_game::Banger>(shattered).unwrap().phase,
            BangerPhase::Broken
        );
        assert!(
            world.get::<avian3d::prelude::Collider>(shattered).is_none(),
            "the shattered placement keeps no collider"
        );
    }
    let fragment_of = |app: &mut App| {
        let mut q = app
            .world_mut()
            .query::<(Entity, &mm2_game::BangerFragment)>();
        q.iter(app.world())
            .filter(|(_, f)| f.parent == shattered)
            .map(|(e, f)| (e, f.index))
            .collect::<Vec<_>>()
    };
    let fragments = fragment_of(&mut app);
    assert_eq!(fragments.len(), 1, "one piece spawned, not the whole set");
    let (piece, index) = fragments[0];
    assert_eq!(index, 1);
    assert_eq!(
        prop_state(&app, piece),
        (
            BangerPhase::Active,
            Some(RigidBody::Kinematic),
            Vec3::new(20.0, 2.0, 1.0)
        )
    );
    assert!(
        !app.world()
            .get::<mm2_game::Banger>(piece)
            .unwrap()
            .def
            .name
            .is_empty()
    );

    // The same piece settles in place — no second spawn.
    host.ctl()
        .broadcast(&props_frame(
            local_table(&app),
            generation,
            15,
            vec![
                prop_row(1, 1, PROP_SETTLED, [21.0, 0.0, 1.0]),
                // The placement's own broken row: already so.
                prop_row(1, mm2_net::SNAP_NO_FRAGMENT, PROP_BROKEN, [20.0, 0.0, 0.0]),
            ],
        ))
        .unwrap();
    spin(&mut app, |a| landed(a) >= 5);
    app.update();
    assert_eq!(fragment_of(&mut app).len(), 1);
    assert_eq!(
        prop_state(&app, piece),
        (
            BangerPhase::Settled,
            Some(RigidBody::Static),
            Vec3::new(21.0, 0.0, 1.0)
        )
    );

    // Rows that name nothing resolve to nothing.
    let before = unresolved(&app);
    host.ctl()
        .broadcast(&props_frame(
            local_table(&app),
            generation,
            20,
            vec![
                // No such placement.
                prop_row(777, mm2_net::SNAP_NO_FRAGMENT, PROP_ACTIVE, [0.0; 3]),
                // An unreadable phase.
                prop_row(0, mm2_net::SNAP_NO_FRAGMENT, 9, [30.0, 1.0, 0.0]),
                // A non-finite pose.
                prop_row(
                    2,
                    mm2_net::SNAP_NO_FRAGMENT,
                    PROP_ACTIVE,
                    [f32::NAN, 0.0, 0.0],
                ),
                // A fragment past the authored pieces (2 authored).
                prop_row(1, 5, PROP_ACTIVE, [20.0, 0.0, 0.0]),
            ],
        ))
        .unwrap();
    // A frame for another generation is dropped whole.
    host.ctl()
        .broadcast(&props_frame(
            local_table(&app),
            generation + 1,
            21,
            vec![prop_row(2, 7, PROP_ACTIVE, [30.0, 1.0, 0.0])],
        ))
        .unwrap();
    spin(&mut app, |a| unresolved(a) >= before + 5);
    app.update();
    assert_eq!(
        prop_state(&app, untouched).0,
        BangerPhase::Dormant,
        "nothing unreadable or foreign touched the untouched prop"
    );
    assert_eq!(
        fragment_of(&mut app).len(),
        1,
        "the oversized index spawned nothing"
    );

    host.shutdown();
}

/// F26-A / F26-AC02: rows that arrive while the world is still being
/// stamped are held, not spent — a joiner whose first props frame beats
/// its load applies it whole once the session is live.
#[test]
fn prop_rows_hold_through_the_load_and_apply_after() {
    use mm2_app::worldprops::PROP_SETTLED;
    use mm2_game::BangerPhase;

    let install = tempfile::tempdir().unwrap();
    let (mut host, mut app, generation) = playing_client(install.path());
    assert_eq!(session_phase(&app), SessionPhase::Loading);
    // The world the host stamped: one placement, which this client has
    // not stamped yet.
    let hosts_world = table_of(&[("prop", Vec3::new(4.0, 0.0, 0.0))]);
    host.ctl()
        .broadcast(&props_frame(
            hosts_world,
            generation,
            3,
            vec![prop_row(
                0,
                mm2_net::SNAP_NO_FRAGMENT,
                PROP_SETTLED,
                [4.0, 0.0, 4.0],
            )],
        ))
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::RemoteSnaps>()
            .props()
            .staged()
            == 1
    });
    // Several more frames while loading: still held, not dropped.
    for _ in 0..5 {
        app.update();
    }
    {
        let props = app.world().resource::<netdrive::RemoteSnaps>().props();
        assert_eq!(
            (props.staged(), props.landed(), props.unresolved()),
            (1, 0, 0)
        );
    }
    // The world finishes loading; the held row lands on the stamped
    // placement.
    let prop = stamp_prop(&mut app, BangerPhase::Dormant, Vec3::new(4.0, 0.0, 0.0), 0);
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Countdown).unwrap();
    }
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::RemoteSnaps>()
            .props()
            .landed()
            == 1
    });
    app.update();
    assert_eq!(
        prop_state(&app, prop),
        (
            BangerPhase::Settled,
            Some(avian3d::prelude::RigidBody::Static),
            Vec3::new(4.0, 0.0, 4.0)
        )
    );
    host.shutdown();
}

/// F26-A: a client whose stamped world differs from the host's refuses
/// the host's rows instead of posing whichever prop now holds the
/// ordinal. The host stamped three placements; this client's third model
/// failed to load, so its table has two — every later ordinal would be
/// shifted. Nothing is applied and the disagreement is recorded; once a
/// frame arrives whose table agrees with the client's own, rows apply
/// again.
#[test]
fn a_client_refuses_rows_from_a_host_with_a_different_world() {
    use mm2_app::worldprops::PROP_SETTLED;
    use mm2_game::BangerPhase;

    let install = tempfile::tempdir().unwrap();
    let (mut host, mut app, generation) = playing_client(install.path());
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    let homes = [
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::new(20.0, 0.0, 0.0),
        Vec3::new(30.0, 0.0, 0.0),
    ];
    let first = stamp_prop(&mut app, BangerPhase::Dormant, homes[0], 0);
    let second = stamp_prop(&mut app, BangerPhase::Dormant, homes[1], 0);
    app.update();
    let ours = local_table(&app);
    assert_eq!(ours, table_of(&[("prop", homes[0]), ("prop", homes[1])]));

    let hosts_world = table_of(&[("prop", homes[0]), ("prop", homes[1]), ("prop", homes[2])]);
    assert_ne!(hosts_world, ours);
    host.ctl()
        .broadcast(&props_frame(
            hosts_world,
            generation,
            7,
            vec![
                prop_row(0, mm2_net::SNAP_NO_FRAGMENT, PROP_SETTLED, [10.0, 0.0, 5.0]),
                prop_row(1, mm2_net::SNAP_NO_FRAGMENT, PROP_SETTLED, [20.0, 0.0, 5.0]),
            ],
        ))
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::RemoteSnaps>()
            .props()
            .mismatched()
            == 2
    });
    {
        let props = app.world().resource::<netdrive::RemoteSnaps>().props();
        assert_eq!(props.divergence(), Some((hosts_world, ours)));
        assert_eq!(
            (props.landed(), props.unresolved(), props.staged()),
            (0, 0, 0)
        );
    }
    for entity in [first, second] {
        assert_eq!(
            prop_state(&app, entity).0,
            BangerPhase::Dormant,
            "a disagreeing world's rows pose nothing"
        );
    }

    // The same rows under a table that does agree are applied.
    host.ctl()
        .broadcast(&props_frame(
            ours,
            generation,
            8,
            vec![prop_row(
                1,
                mm2_net::SNAP_NO_FRAGMENT,
                PROP_SETTLED,
                [20.0, 0.0, 5.0],
            )],
        ))
        .unwrap();
    spin(&mut app, |a| {
        a.world()
            .resource::<netdrive::RemoteSnaps>()
            .props()
            .landed()
            == 1
    });
    let props = app.world().resource::<netdrive::RemoteSnaps>().props();
    assert!(props.divergence().is_none(), "agreement clears the flag");
    assert_eq!(props.mismatched(), 2, "the earlier refusals stay counted");
    assert_eq!(prop_state(&app, second).0, BangerPhase::Settled);
    host.shutdown();
}

/// F26-A end to end: a hosted app's knocked, shattered and settled props
/// publish as `Props` frames and a joined client's app — the production
/// `publish_props`/`apply_props` over a real loopback socket — converges
/// on the host's world. Dormant props send nothing, and the rolling
/// resend window carries a prop the first frames' fresh pass already
/// delivered a second time (state, not events).
#[test]
fn a_hosts_prop_state_converges_on_a_joined_clients_world() {
    use avian3d::prelude::RigidBody;
    use mm2_game::BangerPhase;

    let install = tempfile::tempdir().unwrap();
    let (link, host_vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut host_app = host_app(host_vfs, link);
    let client_vfs = mount(install.path());
    let client_link = LobbyLink::join(
        addr,
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut client = bridge_app(client_vfs, client_link);
    {
        let link = client.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    // Both apps step until the roster shows the client ready, then the
    // host starts the session and the client follows.
    for _ in 0..200 {
        host_app.update();
        client.update();
        let our_id = client.world().resource::<LobbyLink>().player_id();
        let ready = host_app
            .world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.player_id == our_id && e.ready && e.pick.is_some());
        if ready {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    hosted_playing(&mut host_app);
    until_begun(&mut client);
    {
        let mut session = client.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }

    // Both processes stamp the same four placements in the same order —
    // the stamp ordinal is the shared identity. The host's world has
    // moved on: site 1 is flying, site 2 shattered with one piece in
    // the air, site 3 at rest; site 0 is untouched.
    let host_props = [
        stamp_prop(
            &mut host_app,
            BangerPhase::Dormant,
            Vec3::new(10.0, 0.0, 0.0),
            0,
        ),
        stamp_prop(
            &mut host_app,
            BangerPhase::Active,
            Vec3::new(20.0, 0.0, 0.0),
            0,
        ),
        stamp_prop(
            &mut host_app,
            BangerPhase::Broken,
            Vec3::new(30.0, 0.0, 0.0),
            2,
        ),
        stamp_prop(
            &mut host_app,
            BangerPhase::Settled,
            Vec3::new(40.0, 0.0, 0.0),
            0,
        ),
    ];
    // Impacts moved two of them off their stamped homes: a prop's
    // replication identity is its home, its pose is the state.
    for (prop, pose) in [
        (host_props[1], Vec3::new(20.0, 3.0, 0.0)),
        (host_props[3], Vec3::new(40.0, 0.0, 6.0)),
    ] {
        host_app
            .world_mut()
            .get_mut::<avian3d::prelude::Position>(prop)
            .unwrap()
            .0 = pose;
    }
    let client_props = [
        stamp_prop(
            &mut client,
            BangerPhase::Dormant,
            Vec3::new(10.0, 0.0, 0.0),
            0,
        ),
        stamp_prop(
            &mut client,
            BangerPhase::Dormant,
            Vec3::new(20.0, 0.0, 0.0),
            0,
        ),
        stamp_prop(
            &mut client,
            BangerPhase::Dormant,
            Vec3::new(30.0, 0.0, 0.0),
            2,
        ),
        stamp_prop(
            &mut client,
            BangerPhase::Dormant,
            Vec3::new(40.0, 0.0, 0.0),
            0,
        ),
    ];
    // The host's one airborne fragment, index 1 of site 2.
    {
        let (object, role, owner) = {
            let mut session = host_app.world_mut().resource_mut::<Session>();
            (
                session.mint_object_id(),
                session.authority_role(),
                mm2_game::SessionEntity(session.generation()),
            )
        };
        let mut banger = mm2_game::Banger::new(prop_def("piece"));
        banger.phase = BangerPhase::Active;
        host_app.world_mut().spawn((
            mm2_app::banger::banger_bundle(
                banger,
                object,
                role,
                owner,
                avian3d::prelude::Collider::cuboid(0.5, 0.5, 0.5),
                Transform::from_xyz(31.0, 2.0, 0.5),
                "piece-1".into(),
            ),
            mm2_game::BangerFragment {
                parent: host_props[2],
                index: 1,
            },
            avian3d::prelude::Position(Vec3::new(31.0, 2.0, 0.5)),
            avian3d::prelude::Rotation(Quat::IDENTITY),
        ));
    }

    let step = |host_app: &mut App, client: &mut App| {
        host_app.update();
        client.update();
        thread::sleep(Duration::from_millis(5));
    };
    let mut converged = false;
    for _ in 0..400 {
        step(&mut host_app, &mut client);
        let flying = prop_state(&client, client_props[1]);
        let shattered = prop_state(&client, client_props[2]).0;
        let rest = prop_state(&client, client_props[3]);
        if flying.0 == BangerPhase::Active
            && shattered == BangerPhase::Broken
            && rest.0 == BangerPhase::Settled
        {
            converged = true;
            break;
        }
    }
    assert!(converged, "the client never reached the host's prop state");
    assert_eq!(
        prop_state(&client, client_props[0]).0,
        BangerPhase::Dormant,
        "an untouched prop is not replicated"
    );
    let flying = prop_state(&client, client_props[1]);
    assert_eq!(
        (flying.1, flying.2),
        (Some(RigidBody::Kinematic), Vec3::new(20.0, 3.0, 0.0))
    );
    let rest = prop_state(&client, client_props[3]);
    assert_eq!(
        (rest.1, rest.2),
        (Some(RigidBody::Static), Vec3::new(40.0, 0.0, 6.0))
    );
    // The airborne fragment arrives as a spawned piece of site 2.
    let mut found = false;
    for _ in 0..200 {
        step(&mut host_app, &mut client);
        let mut q = client.world_mut().query::<(
            &mm2_game::BangerFragment,
            &mm2_game::Banger,
            &avian3d::prelude::Position,
        )>();
        if let Some((frag, banger, pos)) = q.iter(client.world()).next() {
            assert_eq!(frag.parent, client_props[2]);
            assert_eq!(frag.index, 1);
            assert_eq!(banger.phase, BangerPhase::Active);
            assert_eq!(pos.0, Vec3::new(31.0, 2.0, 0.5));
            found = true;
            break;
        }
    }
    assert!(
        found,
        "the host's airborne fragment never reached the client"
    );

    // The host's prop comes to rest: the client follows it to the
    // settled pose and stops treating it as a live body.
    {
        let world = host_app.world_mut();
        let mut banger = world.get_mut::<mm2_game::Banger>(host_props[1]).unwrap();
        banger.phase = BangerPhase::Settled;
        world
            .get_mut::<avian3d::prelude::Position>(host_props[1])
            .unwrap()
            .0 = Vec3::new(22.0, 0.0, 1.0);
    }
    let mut settled = false;
    for _ in 0..400 {
        step(&mut host_app, &mut client);
        if prop_state(&client, client_props[1]).0 == BangerPhase::Settled {
            settled = true;
            break;
        }
    }
    assert!(settled, "the client never followed the prop's settle");
    let (_, body, pos) = prop_state(&client, client_props[1]);
    assert_eq!(
        (body, pos),
        (Some(RigidBody::Static), Vec3::new(22.0, 0.0, 1.0))
    );

    let props = client.world().resource::<netdrive::RemoteSnaps>().props();
    assert_eq!(props.unresolved(), 0, "every row named a real prop");
    assert_eq!(props.refused(), 0);
    assert_eq!(props.mismatched(), 0, "both peers stamped the same world");
    assert!(props.divergence().is_none());
    assert_eq!(props.local_table().count, 4);
}

/// F26-A: the publish frame is bounded and self-healing. Twenty settled
/// props all ride the first frame as fresh changes; every later frame
/// carries only the live body plus the rolling resend window — never
/// the whole inventory — and the window's cursor still brings every
/// settled prop around again, which is what heals a dropped frame and
/// catches a late joiner up. A dormant prop never appears.
#[test]
fn the_prop_publish_window_cycles_without_growing_the_frame() {
    use mm2_app::worldprops::{PROP_ACTIVE, PROP_SETTLED, RESEND_WINDOW};
    use mm2_game::BangerPhase;
    use std::collections::BTreeSet;

    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    hosted_playing(&mut app);

    // Site 0 dormant, site 1 live, sites 2..22 settled.
    stamp_prop(&mut app, BangerPhase::Dormant, Vec3::ZERO, 0);
    stamp_prop(&mut app, BangerPhase::Active, Vec3::new(5.0, 2.0, 0.0), 0);
    for i in 0..20 {
        stamp_prop(
            &mut app,
            BangerPhase::Settled,
            Vec3::new(10.0 + i as f32, 0.0, 0.0),
            0,
        );
    }
    for _ in 0..80 {
        app.update();
    }

    let mut frames: Vec<Vec<mm2_net::SnapProp>> = Vec::new();
    // Every frame names the world its ordinals are relative to: the 22
    // placements stamped above, at their homes (the live one's home is
    // where it was stamped, not where it flies).
    let mut world = vec![("prop", Vec3::ZERO), ("prop", Vec3::new(5.0, 2.0, 0.0))];
    world.extend((0..20).map(|i| ("prop", Vec3::new(10.0 + i as f32, 0.0, 0.0))));
    let hosts_world = table_of(&world);
    assert_eq!(hosts_world.count, 22);
    while frames.len() < 6 {
        if let Message::Props { rows, table, .. } =
            until_wire(&mut peer, |m| matches!(m, Message::Props { .. }))
        {
            assert_eq!(table, hosts_world);
            frames.push(rows);
        }
    }
    let first_sites: BTreeSet<u32> = frames[0].iter().map(|r| r.site).collect();
    assert_eq!(
        first_sites,
        (1..22).collect::<BTreeSet<u32>>(),
        "the first frame carries every changed prop and not the dormant one"
    );
    let mut cycled = BTreeSet::new();
    for rows in &frames[1..] {
        assert!(
            rows.len() <= 1 + RESEND_WINDOW,
            "a steady-state frame is the live body plus the window, got {}",
            rows.len()
        );
        let live: Vec<_> = rows.iter().filter(|r| r.phase == PROP_ACTIVE).collect();
        assert_eq!(live.len(), 1, "the live body rides every frame");
        assert_eq!((live[0].site, live[0].pos), (1, [5.0, 2.0, 0.0]));
        for row in rows.iter().filter(|r| r.phase == PROP_SETTLED) {
            assert!(row.site >= 2, "only settled props ride the window");
            cycled.insert(row.site);
        }
        assert!(rows.iter().all(|r| r.site != 0), "dormant never publishes");
    }
    assert!(
        cycled.len() >= 2 * RESEND_WINDOW,
        "the cursor advances through the inventory, saw {cycled:?}"
    );
}

/// A three-player free-for-all over a synthetic pool, minted in
/// `generation` — the host's match the F27-B.3 legs publish.
fn cnr_match(generation: u64) -> mm2_game::gold::GoldMatch {
    use mm2_game::gold::{CarrierLoad, CnrVariant, EndRule, GoldMatch, GoldRules, Side};
    GoldMatch::new(
        generation,
        mm2_game::ObjectId {
            generation,
            slot: 90,
        },
        GoldRules {
            variant: CnrVariant::FreeForAll,
            end: EndRule::Points(500),
            load: CarrierLoad::NONE,
            pickup_points: 25,
            delivery_points: 100,
            pickup_radius: 4.0,
            delivery_radius: 12.0,
            drop_lockout_ticks: 10,
        },
        (0..6)
            .map(|i| Vec3::new(i as f32 * 40.0, 0.0, -(i as f32) * 25.0))
            .collect(),
        11,
        &[
            (mm2_game::PlayerId(0), Side::Solo),
            (mm2_game::PlayerId(1), Side::Solo),
        ],
    )
    .unwrap()
}

/// F27-B.3 host half: with a `CnrHost` the authority publishes the
/// match at once, again on every change of state, and otherwise only
/// once per `PUBLISH_EVERY_TICKS` of match time — never on every frame
/// — and what a peer decodes off the real socket is exactly the host's
/// own view of the match.
#[test]
fn the_host_publishes_the_cops_and_robbers_match_on_change_and_at_the_cadence() {
    use mm2_app::cnr::CnrHost;
    use mm2_app::cnrnet::{PUBLISH_EVERY_TICKS, decode_view};
    use mm2_game::gold::Contact;

    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    let generation = hosted_playing(&mut app);
    let sent = |a: &App| a.world().resource::<netdrive::NetDriveReport>().cnr_sent;

    // No match, no frame — a session that is not Cops & Robbers is
    // untouched by the leg.
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(sent(&app), 0);

    app.insert_resource(CnrHost::new(cnr_match(generation)));
    app.update();
    let first = until_wire(&mut peer, |m| matches!(m, Message::Cnr { .. }));
    let Message::Cnr {
        generation: g,
        frame,
    } = first
    else {
        unreachable!()
    };
    assert_eq!(g, generation);
    let want = app.world().resource::<CnrHost>().game.view();
    assert_eq!(decode_view(g, &frame).unwrap(), want);
    assert_eq!(sent(&app), 1);

    // Idle frames send nothing while the clock stays inside the cadence.
    for _ in 0..PUBLISH_EVERY_TICKS - 1 {
        app.world_mut().resource_mut::<CnrHost>().game.tick();
    }
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(sent(&app), 1, "inside the cadence nothing goes out");

    // The tick that reaches it does.
    app.world_mut().resource_mut::<CnrHost>().game.tick();
    app.update();
    let Message::Cnr { frame, .. } = until_wire(&mut peer, |m| matches!(m, Message::Cnr { .. }))
    else {
        unreachable!()
    };
    assert_eq!(frame.elapsed, PUBLISH_EVERY_TICKS);
    assert_eq!(sent(&app), 2);

    // A change of state goes out the same frame, cadence or not: the
    // pickup makes player 0 the carrier.
    {
        let game = &mut app.world_mut().resource_mut::<CnrHost>().game;
        let c = Contact {
            player: mm2_game::PlayerId(0),
            round: game.round(),
            position: game.gold_position().unwrap(),
        };
        game.resolve_pickups(&[c]);
    }
    app.update();
    let Message::Cnr { frame, .. } = until_wire(&mut peer, |m| matches!(m, Message::Cnr { .. }))
    else {
        unreachable!()
    };
    let view = decode_view(generation, &frame).unwrap();
    assert_eq!(view.carrier(), Some(mm2_game::PlayerId(0)));
    assert_eq!(view, app.world().resource::<CnrHost>().game.view());
    assert_eq!(sent(&app), 3);

    // Frames are only for a running session: once it is over (here the
    // host's own session leaves `Playing` for the results screen the
    // frame still rides) nothing about the cadence changes, but with no
    // `CnrHost` the leg is idle again.
    app.world_mut().remove_resource::<CnrHost>();
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(sent(&app), 3);
}

/// F27-B.4c: a hosted match that is decided ends the host's session in
/// `Results` — and the decided frame still reaches the peers from
/// there, so a joined client can open its own match-over screen.
#[test]
fn a_decided_hosted_match_still_publishes_from_its_results_screen() {
    use mm2_app::cnr::{CnrHost, end_decided_match};
    use mm2_app::cnrnet::decode_view;
    use mm2_game::gold::{CarrierLoad, CnrVariant, EndRule, GoldMatch, GoldRules, Side};

    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let addr = link.addr();
    let mut app = host_app(vfs, link);
    app.add_systems(Update, end_decided_match);
    let mut peer = ready_peer(addr, "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    let generation = hosted_playing(&mut app);

    let mut game = GoldMatch::new(
        generation,
        mm2_game::ObjectId {
            generation,
            slot: 90,
        },
        GoldRules {
            variant: CnrVariant::FreeForAll,
            end: EndRule::Ticks(5),
            load: CarrierLoad::NONE,
            pickup_points: 25,
            delivery_points: 100,
            pickup_radius: 4.0,
            delivery_radius: 12.0,
            drop_lockout_ticks: 10,
        },
        (0..6)
            .map(|i| Vec3::new(i as f32 * 40.0, 0.0, -(i as f32) * 25.0))
            .collect(),
        11,
        &[(mm2_game::PlayerId(0), Side::Solo)],
    )
    .unwrap();
    for _ in 0..10 {
        game.tick();
    }
    assert!(game.outcome().is_some());
    app.insert_resource(CnrHost::new(game));
    app.update();
    assert_eq!(
        *app.world().resource::<Session>().phase(),
        SessionPhase::Results,
        "the decided hosted match opens its results screen"
    );
    // Whatever frame went out while the phase flipped, the last one the
    // peer can read says the match is decided.
    for _ in 0..3 {
        app.update();
    }
    let mut decided = false;
    for _ in 0..8 {
        let Message::Cnr { frame, .. } =
            until_wire(&mut peer, |m| matches!(m, Message::Cnr { .. }))
        else {
            unreachable!()
        };
        if decode_view(generation, &frame).unwrap().outcome.is_some() {
            decided = true;
            break;
        }
    }
    assert!(decided, "the peer never saw the decided match");
}

/// F27-AC05: a decided match's clock has stopped, so only the change
/// that decided it ever publishes — and over a lossy link that one frame
/// is the whole result. The proxy swallows the first decided frame; the
/// host repeats the final frame every `DECIDED_REPEAT_RUNS` runs, and
/// the peer learns the verdict from the repeat. Loss is the proxy's
/// whole-frame drop, the application-level effect of a lost datagram on
/// loopback; the real transport is reliable TCP.
#[test]
fn a_decided_match_survives_its_first_frame_being_lost() {
    use mm2_app::cnr::{CnrHost, end_decided_match};
    use mm2_app::cnrnet::{DECIDED_REPEAT_RUNS, decode_view};
    use mm2_game::gold::{CarrierLoad, CnrVariant, EndRule, GoldMatch, GoldRules, Side};

    let install = tempfile::tempdir().unwrap();
    let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
    let proxy = ImpairProxy::loopback_seeded(link.addr(), 17).unwrap();
    let mut app = host_app(vfs, link);
    app.add_systems(Update, end_decided_match);
    let mut peer = ready_peer(proxy.addr(), "eve", fp);
    spin(&mut app, |a| {
        a.world()
            .resource::<LobbyState>()
            .roster
            .iter()
            .any(|e| e.pick.is_some())
    });
    let generation = hosted_playing(&mut app);
    let sent = |a: &App| a.world().resource::<netdrive::NetDriveReport>().cnr_sent;

    let mut game = GoldMatch::new(
        generation,
        mm2_game::ObjectId {
            generation,
            slot: 90,
        },
        GoldRules {
            variant: CnrVariant::FreeForAll,
            end: EndRule::Ticks(5),
            load: CarrierLoad::NONE,
            pickup_points: 25,
            delivery_points: 100,
            pickup_radius: 4.0,
            delivery_radius: 12.0,
            drop_lockout_ticks: 10,
        },
        (0..6)
            .map(|i| Vec3::new(i as f32 * 40.0, 0.0, -(i as f32) * 25.0))
            .collect(),
        11,
        &[(mm2_game::PlayerId(0), Side::Solo)],
    )
    .unwrap();
    for _ in 0..10 {
        game.tick();
    }
    assert!(game.outcome().is_some());
    let decided = game.view();

    // The data plane goes dark, and the decided frame goes out into it.
    proxy.set(
        LinkDir::Down,
        Impair {
            loss: 1.0,
            ..Impair::default()
        },
    );
    app.insert_resource(CnrHost::new(game));
    app.update();
    assert_eq!(sent(&app), 1, "the decision publishes once at once");
    spin(&mut app, |_| proxy.stats(LinkDir::Down).dropped > 0);
    assert_eq!(sent(&app), 1);

    // The link heals. Nothing changes in the match — the repeat alone
    // carries the result, and not before its cadence.
    proxy.set(LinkDir::Down, Impair::default());
    let mut runs = 0;
    while sent(&app) == 1 {
        app.update();
        runs += 1;
        assert!(runs <= DECIDED_REPEAT_RUNS, "the repeat never came");
    }
    // The drop-wait above already ran a few of the cadence's runs.
    assert!(
        runs > DECIDED_REPEAT_RUNS / 2,
        "the repeat waits its cadence, not every frame: {runs}"
    );
    let mut seen = None;
    for _ in 0..16 {
        let Message::Cnr { frame, .. } =
            until_wire(&mut peer, |m| matches!(m, Message::Cnr { .. }))
        else {
            unreachable!()
        };
        if let Ok(view) = decode_view(generation, &frame)
            && view.outcome.is_some()
        {
            seen = Some(view);
            break;
        }
    }
    assert_eq!(
        seen.expect("the peer never learned the verdict"),
        decided,
        "the repeat is the decided match, whole"
    );
}

/// F27-B.3 client half: the host's match frame lands as a replica for
/// the session's generation; a reordered older frame, a repeat,
/// another generation's frame and a self-contradicting one change
/// nothing (each counted), and the replica dies with the session.
#[test]
fn a_cops_and_robbers_frame_lands_as_a_replica_and_stale_or_foreign_ones_do_not() {
    use mm2_app::cnrnet::{CnrReplica, encode_view};
    use mm2_game::gold::Contact;

    let install = tempfile::tempdir().unwrap();
    let vfs = mount(install.path());
    let fp = mm2_content::fingerprint::gameplay(&vfs).unwrap().hash;
    let mut host_config = HostConfig::new(fp);
    host_config.host_pick = Some(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    let host = Host::listen_loopback(&host_config).unwrap();
    host.set_session(net::advertise(&dev_cruise()).unwrap())
        .unwrap();
    let link = LobbyLink::join(
        host.addr(),
        &hello("net-app-test".to_string(), "alice".to_string(), fp),
        false,
        DevOverrides::default(),
    )
    .expect("join failed");
    let mut app = bridge_app(vfs, link);
    {
        let link = app.world().resource::<LobbyLink>();
        link.ctl().set_vehicle("", 0).unwrap();
        link.ctl().set_ready(true).unwrap();
    }
    until_ready(&mut app);
    host.start(LateJoin::Open).unwrap();
    until_started(&host);
    until_begun(&mut app);
    let generation = app.world().resource::<Session>().wire_generation();
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    }
    let counts = |a: &App| {
        let c = a.world().resource::<netdrive::RemoteSnaps>().cnr();
        (c.landed(), c.stale(), c.refused())
    };
    let replica = |a: &App| a.world().get_resource::<CnrReplica>().map(|r| r.0.clone());
    assert!(replica(&app).is_none(), "no frame, no replica");

    let mut game = cnr_match(generation);
    let first = encode_view(&game.view());
    let send = |generation, frame: &mm2_net::SnapCnr| {
        host.ctl()
            .broadcast(&Message::Cnr {
                generation,
                frame: frame.clone(),
            })
            .unwrap()
    };

    // A late joiner lands on the match as it stands.
    send(generation, &first);
    spin(&mut app, |a| replica(a).is_some());
    assert_eq!(replica(&app).unwrap(), game.view());
    assert_eq!(counts(&app), (1, 0, 0));

    // The pickup arrives; a reordered older frame and a repeat after it
    // change nothing.
    game.tick();
    let c = Contact {
        player: mm2_game::PlayerId(1),
        round: game.round(),
        position: game.gold_position().unwrap(),
    };
    game.resolve_pickups(&[c]);
    let newer = encode_view(&game.view());
    send(generation, &newer);
    spin(&mut app, |a| counts(a).0 == 2);
    assert_eq!(
        replica(&app).unwrap().carrier(),
        Some(mm2_game::PlayerId(1))
    );
    send(generation, &first);
    send(generation, &newer);
    spin(&mut app, |a| counts(a).1 == 2);
    assert_eq!(counts(&app), (2, 2, 0));
    assert_eq!(
        replica(&app).unwrap().carrier(),
        Some(mm2_game::PlayerId(1)),
        "an older frame must not hand the gold back"
    );

    // Another generation's frame is refused at apply time; a frame that
    // contradicts itself (carried by a stranger) is refused at push and
    // cannot become the watermark.
    send(generation + 1, &newer);
    spin(&mut app, |a| counts(a).2 == 1);
    let mut bad = newer.clone();
    bad.holder = 99;
    bad.revision = u64::MAX;
    send(generation, &bad);
    spin(&mut app, |a| counts(a).2 == 2);
    game.tick();
    send(generation, &encode_view(&game.view()));
    spin(&mut app, |a| counts(a).0 == 3);
    assert_eq!(counts(&app), (3, 2, 2));
    assert_eq!(replica(&app).unwrap().elapsed, 2);

    // The replica dies with the session.
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Results).unwrap();
        session.transition(SessionPhase::Unloading).unwrap();
    }
    app.update();
    assert!(replica(&app).is_none(), "no match outlives its session");
}
