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
    DevOverrides, Mm2Vfs, Player, PlayerControl, PlayerVehicle, Session, SessionAuthority,
    SessionConfig, SessionMode, SessionPhase, WorldMode, despawn_session_entities,
};
use mm2_net::{
    Client, DriveInput, Host, HostConfig, HostEvent, LateJoin, LeaveCause, Message, SnapEntry,
    VehiclePick, hello,
};
use mm2_vehicle::{VehicleConfig, VehicleInput};
use support::{Proc, WAIT, listening, mount};

const HOST_EXE: &str = env!("CARGO_BIN_EXE_mm2-host");
const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

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
        );
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
                netdrive::send_drive_input,
            ),
        );
    app
}

/// The hosted-lobby bridge app — same wiring, `HostLink` side.
fn host_app(vfs: Vfs, link: HostLink) -> App {
    let mut app = lobby_app(vfs);
    app.insert_resource(link)
        .init_resource::<netdrive::NetDriveReport>()
        .add_systems(
            Update,
            (
                net::host_input,
                net::drive_host.after(session::drive_session),
                netdrive::reconcile_remote_players.after(net::drive_host),
                netdrive::apply_remote_inputs.after(net::drive_host),
                netdrive::publish_snapshots.after(net::drive_host),
            ),
        );
    app
}

fn host_event(host: &Host) -> HostEvent {
    host.recv_timeout(WAIT).expect("no host event")
}

/// Drain host events until the `Started` verdict; returns the minted
/// generation.
fn until_started(host: &Host) -> u64 {
    loop {
        if let HostEvent::Started { generation } = host_event(host) {
            return generation;
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
    app.update(); // join broadcast: Session ad + roster + pick echo

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
    app.update();
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
    app.update();

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
    app.update();
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
// snapshot-lerped kinematic copies on the client); they do not claim
// prediction, reconciliation of the local seat, or damage/result
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
    app.update();
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
    // this seat's wire id with this session's generation.
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
        ))
        .id();
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
            generation,
            tick: 7,
            entries: vec![
                SnapEntry {
                    player: 0,
                    pos: [9.0, 1.0, 9.0],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [1.0, 0.0, 0.0],
                    angvel: [0.0, 0.0, 0.0],
                },
                // Our own seat's entry is received and skipped —
                // reconciliation of the local car is a later slice.
                SnapEntry {
                    player: our_id,
                    pos: [-50.0, 0.0, -50.0],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [0.0; 3],
                    angvel: [0.0; 3],
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
    // The local seat was not moved by its own entry.
    assert!(
        app.world().get::<PlayerVehicle>(local).is_some(),
        "the local car stayed ours"
    );

    // A stale tick and a foreign generation both drop untouched.
    for tick in [3u64, 7] {
        host.ctl()
            .broadcast(&Message::Snap {
                generation,
                tick,
                entries: vec![SnapEntry {
                    player: 0,
                    pos: [0.0; 3],
                    rot: [0.0, 0.0, 0.0, 1.0],
                    vel: [0.0; 3],
                    angvel: [0.0; 3],
                }],
            })
            .unwrap();
    }
    host.ctl()
        .broadcast(&Message::Snap {
            generation: generation + 9,
            tick: 99,
            entries: vec![SnapEntry {
                player: 0,
                pos: [0.0; 3],
                rot: [0.0, 0.0, 0.0, 1.0],
                vel: [0.0; 3],
                angvel: [0.0; 3],
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

    host.shutdown();
}
