//! Hostile-peer and abrupt-loss legs at process level — the F24-C slice
//! the lobby's in-process coverage cannot reach: a real `mm2-host` OS
//! process facing bytes that were never a protocol peer, and real
//! client processes watching their authority die mid-socket.
//!
//! AC03's process-level half: malformed first frames (an undecodable
//! payload, an oversize length word, a valid-but-not-`Hello` message)
//! are bounded at the door with a named reason, a peer that completes
//! TCP and then goes silent is refused on the handshake deadline, and
//! a rostered peer sending host-only messages is dropped
//! `cause=malformed` — after each hit the lobby still serves a real
//! `mm2-join`. AC04's abrupt half: SIGKILLing the host process — its
//! sockets die at the kernel, not through the lobby's own `quit`
//! disconnect that `net_app`'s lost-host leg already covers — closes a
//! parked `mm2-join` and a started one with `event=closed`/exit 1, and
//! an `mm2 --join` client mid-session reports `lost the host` and
//! exits nonzero.
//!
//! Loopback scope on synthetic installs — the same evidence level as
//! `net_host`/`net_join`. LAN/Internet reachability stays open under
//! F24-C; the impairment matrix lives in `net_app`/`net_drive`.

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use crate::support;

use mm2_net::{Client, MAX_FRAME, Message, RejectCode, hello, read_frame, write_frame};
use support::{Proc, WAIT, listening};

const HOST_EXE: &str = env!("CARGO_BIN_EXE_mm2-host");
const JOIN_EXE: &str = env!("CARGO_BIN_EXE_mm2-join");
const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// How long a client may take to notice a stopped host: the
/// `LIVENESS_TIMEOUT` (5 s) plus slack for process start and a loaded
/// runner. The legs spin on this deadline, never a fixed sleep.
const STOPPED_HOST_BOUND: Duration = Duration::from_secs(30);

/// `mm2-join`'s flags — the same shape `net_join` drives.
fn join_args(
    install: &std::path::Path,
    addr: SocketAddr,
    driver: &str,
    extra: &[&str],
) -> Vec<String> {
    let mut args = vec![
        "--mm2-path".to_string(),
        install.to_str().unwrap().to_string(),
        "--connect".to_string(),
        addr.to_string(),
        "--driver".to_string(),
        driver.to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

/// A dev-world `mm2-host` on an ephemeral loopback port.
fn dev_host_args(install: &std::path::Path) -> Vec<String> {
    vec![
        "--mm2-path".to_string(),
        install.to_str().unwrap().to_string(),
        "--dev-world".to_string(),
        "--bind".to_string(),
        "127.0.0.1:0".to_string(),
        "--seed".to_string(),
        7.to_string(),
    ]
}

/// The in-app host's flags — `net_drive`'s shape: a dev-world cruise
/// lobby, headless, playing seat 0 once `start` fires.
fn mm2_host_args(install: &std::path::Path, frames: u32) -> Vec<String> {
    vec![
        "--mm2-path".into(),
        install.to_str().unwrap().into(),
        "--host".into(),
        "--dev-world".into(),
        "--bind".into(),
        "127.0.0.1:0".into(),
        "--seed".into(),
        11.to_string(),
        "--headless".into(),
        "--frames".into(),
        frames.to_string(),
    ]
}

/// A headless `mm2 --join` client: parks in the lobby, readies, offers
/// the dev car, drives whatever session the host starts.
fn mm2_join_args(install: &std::path::Path, addr: SocketAddr, frames: u32) -> Vec<String> {
    vec![
        "--mm2-path".into(),
        install.to_str().unwrap().into(),
        "--join".into(),
        addr.to_string(),
        "--driver".into(),
        "edge".into(),
        "--ready".into(),
        "--headless".into(),
        "--frames".into(),
        frames.to_string(),
    ]
}

/// The `mm2 --host` `listening=` record — tracing log lines share
/// stdout, so scan rather than assuming it is first.
fn mm2_listening_addr(host: &Proc) -> SocketAddr {
    host.until("listening=")
        .split_whitespace()
        .find_map(|t| t.strip_prefix("listening="))
        .and_then(|a| a.parse().ok())
        .expect("a listening= record")
}

/// A raw socket with a bounded read — the door legs speak the wire
/// themselves, and a leg that hangs waiting on a silent host fails in
/// seconds rather than stalling the suite.
fn raw(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
}

/// The survivor leg every hostile leg ends with: a real `mm2-join`
/// still completes the lobby after the door took the hit — the
/// malformed traffic cost the host nothing but a `join_failed` line.
fn assert_lobby_still_serves(host: &mut Proc, install: &std::path::Path, addr: SocketAddr) {
    let mut survivor = Proc::spawn(JOIN_EXE, &join_args(install, addr, "survivor", &[]));
    assert!(survivor.line().starts_with("connected="));
    survivor.cmd("quit");
    assert!(
        survivor.wait_timeout(WAIT).success(),
        "the survivor join did not leave cleanly"
    );
    assert!(host.until("cause=quit").contains("cause=quit"));
}

/// An `mm2-join` dialled at an address nothing listens on fails the
/// handshake with a named reason and exits nonzero — the "wrong port"
/// edge AC04 wants a usable error for. (`mm2 --join`'s unreachable leg
/// already lives in `net_app`; this is the headless client's.)
#[test]
fn an_unreachable_address_fails_the_join() {
    let install = tempfile::tempdir().unwrap();
    // A port nothing listens on: bind to learn a free one, drop the
    // listener.
    let dead: SocketAddr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let join = Proc::spawn(JOIN_EXE, &join_args(install.path(), dead, "nobody", &[]));
    let failed = join.line();
    assert!(failed.starts_with("event=join_failed"), "{failed}");
    assert_eq!(join.wait_timeout(WAIT).code(), Some(1));
}

/// The `--until-*` stop conditions watch the wire. A lone headless run
/// has none, so it refuses them with a failed record instead of
/// silently running its `--frames` and reporting a pass.
#[test]
fn a_lone_headless_run_refuses_wire_stop_conditions() {
    let install = tempfile::tempdir().unwrap();
    for flag in [
        ["--until-impacts", "1"],
        ["--until-resets", "1"],
        ["--deadline", "5"],
    ] {
        let out = std::process::Command::new(MM2_EXE)
            .args(["--mm2-path", install.path().to_str().unwrap()])
            .args(["--dev-world", "--headless", "--frames", "5"])
            .args(flag)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("status=fail") && stdout.contains("need --join or --host"),
            "{flag:?}: {stdout}"
        );
        assert_eq!(out.status.code(), Some(smoke_fail_code()), "{flag:?}");
    }
}

fn smoke_fail_code() -> i32 {
    mm2_app::smoke::SmokeStatus::Fail.exit_code()
}

/// Three kinds of bad first contact against a real `mm2-host`, each
/// bounded at the door:
///
/// - a well-formed frame whose payload decodes to no `Message` — the
///   peer reads a `Reject{Malformed}` back, and the host logs the
///   protocol reason;
/// - a length word declaring more than `MAX_FRAME` — refused before
///   allocation, no reply owed;
/// - a decodable first frame that is not `Hello` — the peer reads a
///   `Reject{Malformed}` naming the contract.
///
/// A real client then joins the same lobby — the hits cost nothing
/// but their `join_failed` records.
#[test]
fn the_door_bounds_garbage_frames() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(HOST_EXE, &dev_host_args(install.path()));
    let (addr, _fp, _) = listening(&host);

    let mut peer = raw(addr);
    write_frame(&mut peer, &[0xff]).unwrap();
    let reply = read_frame(&mut peer).unwrap();
    assert!(
        matches!(
            Message::decode(&reply),
            Ok(Message::Reject {
                code: RejectCode::Malformed,
                ..
            })
        ),
        "an undecodable payload earns a Malformed reject: {reply:?}"
    );
    drop(peer);
    let failed = host.until("event=join_failed");
    assert!(failed.contains("protocol"), "{failed}");

    let mut peer = raw(addr);
    peer.write_all(&(MAX_FRAME + 1).to_le_bytes()).unwrap();
    drop(peer);
    let failed = host.until("event=join_failed");
    assert!(failed.contains("frame declares"), "{failed}");

    let mut peer = raw(addr);
    write_frame(
        &mut peer,
        &Message::SetReady { ready: true }.encode().unwrap(),
    )
    .unwrap();
    let reply = read_frame(&mut peer).unwrap();
    let Message::Reject { code, message } = Message::decode(&reply).unwrap() else {
        panic!("expected a Reject, decoded {reply:?}")
    };
    assert_eq!(code, RejectCode::Malformed);
    assert!(message.contains("first message"), "{message}");
    drop(peer);
    let failed = host.until("event=join_failed");
    assert!(failed.contains("first message"), "{failed}");

    assert_lobby_still_serves(&mut host, install.path(), addr);
    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// A peer that completes the TCP handshake and then never speaks
/// cannot hold a lobby slot: the host-side handshake deadline
/// (`HANDSHAKE_TIMEOUT`, 10 s — inside `WAIT`) fires and the conn is
/// refused `join_failed`, then the lobby serves a real join. The
/// in-process `a_silent_client_cannot_stall_accept_hello` proves the
/// mechanism; this leg proves it fires inside a real host process.
#[test]
fn a_silent_peer_is_refused_on_the_deadline() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(HOST_EXE, &dev_host_args(install.path()));
    let (addr, _fp, _) = listening(&host);

    // Keep the socket open and send nothing — held until the host's
    // refusal lands, then dropped.
    let _silent = TcpStream::connect(addr).unwrap();
    let failed = host.until("event=join_failed");
    assert!(failed.contains("I/O"), "{failed}");
    drop(_silent);

    assert_lobby_still_serves(&mut host, install.path(), addr);
    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// A rostered peer that starts speaking for the *host* — sending a
/// host→client-only variant — is dropped `cause=malformed` and its
/// slot pruned, while the lobby keeps serving honest joins. The
/// in-process `out_of_turn_messages_drop_the_peer` covers the drop;
/// this leg runs it against the shipped binary.
#[test]
fn a_rostered_peer_speaking_for_the_host_is_dropped() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(HOST_EXE, &dev_host_args(install.path()));
    let (addr, fp, _) = listening(&host);

    let mut bad = Client::join(addr, &hello("edge".to_string(), "bad".to_string(), fp)).unwrap();
    bad.set_timeout(Some(WAIT)).unwrap();
    // `Roster` is host→client — no client may ever send it.
    bad.send(&Message::Roster { players: vec![] }).unwrap();
    let dropped = host.until("event=left");
    assert!(dropped.contains("cause=malformed"), "{dropped}");

    assert_lobby_still_serves(&mut host, install.path(), addr);
    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}

/// A host process that dies abruptly — SIGKILL, so its sockets close
/// at the kernel rather than through the lobby's own disconnect — is
/// a lost peer, not a wedge: a parked `mm2-join`'s blocked `recv`
/// errors out, the client reports `event=closed` and exits nonzero.
#[test]
fn a_killed_host_closes_a_parked_client() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(HOST_EXE, &dev_host_args(install.path()));
    let (addr, _fp, _) = listening(&host);

    let client = Proc::spawn(JOIN_EXE, &join_args(install.path(), addr, "alice", &[]));
    assert!(client.line().starts_with("connected="));

    host.kill();
    let closed = client.until("event=closed");
    assert!(closed.starts_with("event=closed"), "{closed}");
    assert_eq!(client.wait_timeout(WAIT).code(), Some(1));
}

/// The dead-but-open half of AC04: SIGSTOP leaves the host's sockets
/// open and silent, so no read ever errors on its own. The lobby's
/// keepalive makes the silence itself the signal — a parked
/// `mm2-join` reports `event=closed` and exits 1 inside the liveness
/// bound (a few keepalive intervals, with slack for a loaded runner).
#[test]
fn a_stopped_host_closes_a_parked_client() {
    let install = tempfile::tempdir().unwrap();
    let host = Proc::spawn(HOST_EXE, &dev_host_args(install.path()));
    let (addr, _fp, _) = listening(&host);

    let client = Proc::spawn(JOIN_EXE, &join_args(install.path(), addr, "alice", &[]));
    assert!(client.line().starts_with("connected="));

    host.stop();
    let closed = client.until_within("event=closed", STOPPED_HOST_BOUND);
    assert!(closed.starts_with("event=closed"), "{closed}");
    assert_eq!(client.wait_timeout(WAIT).code(), Some(1));
}

/// The same silent host seen by a headless `mm2 --join`: the loss is
/// the named `lost the host` failure and the app exits nonzero.
#[test]
fn a_stopped_host_ends_a_parked_app_client() {
    let install = tempfile::tempdir().unwrap();
    let host = Proc::spawn(HOST_EXE, &dev_host_args(install.path()));
    let (addr, _fp, _) = listening(&host);

    let client = Proc::spawn(MM2_EXE, &mm2_join_args(install.path(), addr, 100_000));
    host.until("event=joined");

    host.stop();
    let rec = client.until_within("smoke=", STOPPED_HOST_BOUND);
    assert!(
        rec.contains("status=fail") && rec.contains("lost the host"),
        "a silent host is a named failure, not a wedge: {rec}"
    );
    assert_eq!(client.wait_timeout(WAIT).code(), Some(3));
}

/// Same kill, one state later: the lobby already minted a session, so
/// the client has observed `event=started` when the host dies — the
/// close is still reported and still nonzero.
#[test]
fn a_killed_host_closes_a_started_client() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(HOST_EXE, &dev_host_args(install.path()));
    let (addr, _fp, _) = listening(&host);

    let client = Proc::spawn(
        JOIN_EXE,
        &join_args(install.path(), addr, "alice", &["--vehicle", "", "--ready"]),
    );
    assert!(client.line().starts_with("connected="));
    host.until("event=ready id=1 ready=true");
    host.cmd("start");
    host.until("event=started generation=1");
    assert_eq!(
        client.until("event=started"),
        "event=started generation=1 session=\"dev world, cruise, amateur\""
    );

    host.kill();
    let closed = client.line();
    assert!(closed.starts_with("event=closed"), "{closed}");
    assert_eq!(client.wait_timeout(WAIT).code(), Some(1));
}

/// The abrupt-kill leg at the data plane: an `mm2 --host` driving a
/// started session is SIGKILLed mid-drive, and the `mm2 --join`
/// client's link dies with it — the session tears down through the
/// normal lifecycle, the smoke record names the loss and the app
/// exits nonzero. `net_app`'s `mm2_join_reports_a_lost_host` covers a
/// polite `quit` while parked; this is a crash mid-session.
#[test]
fn a_killed_host_ends_a_driving_client() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &mm2_host_args(install.path(), 9000));
    let addr = mm2_listening_addr(&host);

    let client = Proc::spawn(MM2_EXE, &mm2_join_args(install.path(), addr, 3000));
    host.until("ready=true");
    host.cmd("start");
    host.until("event=started generation=1");
    // Let the session actually run — snapshots flowing — before the kill.
    std::thread::sleep(Duration::from_secs(2));

    host.kill();
    let rec = client.until("smoke=");
    assert!(
        rec.contains("status=fail") && rec.contains("lost the host"),
        "a crashed host is a named failure, not a wedge: {rec}"
    );
    assert_eq!(client.wait_timeout(WAIT).code(), Some(3));
}

/// A client that leaves and dials again gets a fresh roster slot —
/// the host mints monotonically, so `alice`'s return is `id=2`, never
/// her recycled `id=1` (the process-level half of the spec's
/// reconnect edge; `a_rejoin_mints_a_fresh_slot` covers it
/// in-process).
#[test]
fn a_rejoined_client_gets_a_fresh_slot() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(HOST_EXE, &dev_host_args(install.path()));
    let (addr, _fp, _) = listening(&host);

    let mut first = Proc::spawn(JOIN_EXE, &join_args(install.path(), addr, "alice", &[]));
    let connected = first.line();
    assert!(connected.contains("id=1"), "{connected}");
    first.cmd("quit");
    assert!(first.wait_timeout(WAIT).success());
    host.until("cause=quit");

    let mut second = Proc::spawn(
        JOIN_EXE,
        &join_args(install.path(), addr, "alice-again", &[]),
    );
    let connected = second.line();
    assert!(connected.contains("id=2"), "{connected}");
    second.cmd("quit");
    assert!(second.wait_timeout(WAIT).success());
    host.until("cause=quit");

    host.cmd("quit");
    assert!(host.wait().success(), "mm2-host did not exit cleanly");
}
