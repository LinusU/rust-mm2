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

mod support;

use mm2_app::net::{self, HostCommand, HostLink, LobbyLink, LobbyState};
use mm2_app::netdrive::{self, NetPlayer, RemotePick};
use mm2_app::session;
use mm2_app::session::{SelectedCar, SessionControl, TunedVehicle};
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{
    DevOverrides, ImpactEvent, ImpactId, Mm2Vfs, ObjectId, ObjectIdentity, Player, PlayerControl,
    PlayerVehicle, Session, SessionAuthority, SessionConfig, SessionMode, SessionPhase,
    SurfaceState, WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_net::{
    Client, DriveInput, Host, HostConfig, HostEvent, Impair, ImpairProxy, LateJoin, LeaveCause,
    LinkDir, Message, SnapEntry, SnapImpact, VehiclePick, hello,
};
use mm2_vehicle::{ResetVehicle, Teleported, VehicleConfig, VehicleInput};
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
        // F25-B (v11): the breakaway reconcile claims pool slots and
        // writes the banger lifecycle stream like the authority does.
        .add_message::<mm2_game::BangerStateChanged>()
        .init_resource::<mm2_game::BangerPool>()
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
                // input stream after the input owners.
                netdrive::reconcile_remote_players.after(net::drive_lobby),
                netdrive::apply_snapshots.after(net::drive_lobby),
                netdrive::drive_remote_lerp,
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
        .add_systems(
            Update,
            (
                mm2_app::input::reset_input.before(mm2_vehicle::systems::vehicle_reset),
                mm2_vehicle::systems::vehicle_reset,
                net::host_input,
                net::drive_host.after(session::drive_session),
                netdrive::reconcile_remote_players.after(net::drive_host),
                netdrive::apply_remote_inputs.after(net::drive_host),
                // F25-B: the request drain is a `ResetVehicle` writer —
                // same ordering edge as `reset_input`.
                netdrive::apply_reset_requests
                    .after(net::drive_host)
                    .before(mm2_vehicle::systems::vehicle_reset),
                netdrive::track_reset_epochs
                    .after(net::drive_host)
                    .after(mm2_vehicle::systems::vehicle_reset)
                    .before(netdrive::publish_snapshots),
                netdrive::publish_snapshots
                    .after(net::drive_host)
                    .after(mm2_vehicle::systems::vehicle_reset),
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
    assert_eq!(session.generation(), generation);
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
    // The adopted generation is the host's mint — clamped never to
    // regress behind what this app already minted (the local begin
    // above took generation 1).
    assert_eq!(
        session.generation(),
        wire_generation.max(2),
        "wire {wire_generation} adopted monotonically"
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
    assert_eq!(session.generation(), generation);
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
/// generation is adopted verbatim, and a wire value that would regress
/// the local counter is clamped forward instead — staleness detection
/// on `generation`-keyed ids depends on never reusing one.
#[test]
fn a_host_generation_is_adopted_but_never_regresses() {
    let mut session = Session::new();
    session.begin_generation(dev_cruise(), 3).unwrap();
    assert_eq!(session.generation(), 3);
    session.transition(SessionPhase::Unloading).unwrap();
    session.transition(SessionPhase::Menu).unwrap();
    session.begin_generation(dev_cruise(), 1).unwrap();
    assert_eq!(
        session.generation(),
        4,
        "a regressed wire generation clamps to the local counter"
    );
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
    let generation = app.world().resource::<Session>().generation();
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
    let generation = app.world().resource::<Session>().generation();
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

    // A stale tick and a foreign generation both drop untouched.
    for tick in [3u64, 7] {
        host.ctl()
            .broadcast(&Message::Snap {
                impacts: Vec::new(),
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
                }],
            })
            .unwrap();
    }
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
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
    let generation = app.world().resource::<Session>().generation();
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
            generation: app.world().resource::<Session>().generation(),
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
    let generation = app.world().resource::<Session>().generation();
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
    };
    // An epoch-equal snap: the remote trailer blends, the own rig's
    // trailer stays exactly where the local sim left it — the wire row
    // lands close (4.2 m, inside the correction bound) and visibly off
    // its pose, so a buggy unconditional write would move it.
    host.ctl()
        .broadcast(&Message::Snap {
            impacts: Vec::new(),
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
    let generation = app.world().resource::<Session>().generation();
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
    // The seat-named hit (the remote car is participant 0 — its row
    // carries the mirrored outward normal), a pair no seat can name,
    // and a foreign-generation event: only the first rides the wire.
    write_impact(&mut app, 7, generation, remote_oid, ObjectId::WORLD);
    write_impact(&mut app, 8, generation, ObjectId::WORLD, ObjectId::WORLD);
    write_impact(&mut app, 9, generation + 9, remote_oid, ObjectId::WORLD);
    app.update();
    let snap = until_wire(
        &mut peer,
        |m| matches!(m, Message::Snap { impacts, .. } if !impacts.is_empty()),
    );
    let Message::Snap { impacts, .. } = snap else {
        unreachable!()
    };
    assert_eq!(impacts.len(), 1, "only the seat-named side rides");
    let row = impacts[0];
    assert_eq!((row.seat, row.id, row.tick), (1, 7, session_tick));
    assert_eq!(row.point, [1.0, 0.5, -2.0]);
    assert_eq!(
        row.normal,
        [0.0, 0.0, 1.0],
        "the seat-0 side's row carries the mirrored outward normal"
    );
    assert_eq!(row.severity, 12.5);
    assert_eq!(
        app.world()
            .resource::<netdrive::NetDriveReport>()
            .impacts_sent,
        1
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
    let generation = app.world().resource::<Session>().generation();
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
    };
    let row = |seat: u16, id: u64| SnapImpact {
        seat,
        id,
        tick: 7,
        point: [3.0, 0.4, -1.0],
        normal: [0.0, 0.0, 1.0],
        severity: 12.5,
    };
    // One valid remote-seat row plus the traps: the own seat's row
    // (skipped, never presented), a departed seat's row, and a
    // non-finite row the sanitize drops.
    host.ctl()
        .broadcast(&Message::Snap {
            generation,
            tick: 7,
            entries: vec![entry(0), entry(our_id)],
            trailers: Vec::new(),
            impacts: vec![
                row(0, 1),
                row(our_id, 1),
                row(7, 1),
                SnapImpact {
                    seat: 0,
                    id: 2,
                    tick: 7,
                    point: [f32::NAN; 3],
                    normal: [0.0, 1.0, 0.0],
                    severity: 1.0,
                },
            ],
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
    host.ctl()
        .broadcast(&Message::Snap {
            generation: generation + 9,
            tick: 9,
            entries: vec![entry(0)],
            trailers: Vec::new(),
            impacts: vec![row(0, 99)],
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
        assert!(drained.is_empty());
        let r = app.world().resource::<netdrive::NetDriveReport>();
        assert_eq!(r.impacts_applied, 1);
        assert_eq!(r.impacts_dropped, 3, "the foreign-generation row dropped");
    }

    host.shutdown();
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
    let generation = app.world().resource::<Session>().generation();
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
    let generation = app.world().resource::<Session>().generation();
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
