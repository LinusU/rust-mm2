//! `mm2 --join` — the app's Bevy-side lobby bridge (F24-B.7).
//!
//! The in-process legs drive `drive_lobby`/`drive_session` inside a
//! minimal `App` against a real loopback `mm2_net::Host`: the pump
//! thread, the wire, the host's roster gate are all real — only the
//! world-load plugins are absent (a begun session parks in `Loading`;
//! the actual `load_session_world` legs are the `headless_lobby`
//! in-process run and the `mm2 --join --headless` process legs below,
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

use mm2_app::net::{self, LobbyLink, LobbyState};
use mm2_app::session;
use mm2_app::session::{SelectedCar, SessionControl, TunedVehicle};
use mm2_app::smoke::{self, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{
    DevOverrides, Mm2Vfs, Session, SessionAuthority, SessionConfig, SessionMode, SessionPhase,
    WorldMode, despawn_session_entities,
};
use mm2_net::{Host, HostConfig, HostEvent, LateJoin, LeaveCause, hello};
use mm2_vehicle::VehicleConfig;
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

/// The app half of the bridge, wired the way `run_headless` wires it:
/// the session lifecycle (`despawn_session_entities` → `drive_session`)
/// then the lobby drain — minus the load/spawn systems that need the
/// asset stack.
fn bridge_app(vfs: Vfs, link: LobbyLink) -> App {
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
        .insert_resource(session::SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .init_resource::<mm2_app::contracts::ImpactFilter>()
        .init_resource::<mm2_app::damage::DamageReport>()
        .init_resource::<mm2_app::stuck::StuckReport>()
        .init_resource::<mm2_app::breakaway::BreakReport>()
        .init_resource::<mm2_app::recovery::RecoveryReport>()
        .init_resource::<mm2_app::damage_fx::SmokeFxReport>()
        .init_resource::<mm2_app::spark_fx::SparkFxReport>()
        .init_resource::<mm2_app::texel_fx::TexelDamageReport>()
        .insert_resource(link)
        .add_systems(
            Update,
            (
                net::lobby_input,
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
                // The bridge settles after the session driver — the
                // same ordering the windowed app and `run_headless` use.
                net::drive_lobby.after(session::drive_session),
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
