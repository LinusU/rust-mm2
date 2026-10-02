//! `mm2 --host` / `mm2 --join` as separate OS processes *driving* one
//! session — the F25 data plane's first two-process evidence.
//!
//! `net_join`'s legs stop at `Start` (F24-B scope: a started session is
//! reported, not simulated). These legs run the whole session headless:
//! each process's `smoke=headless-physics` record carries the `net=`
//! wire counters, so the assertions read what actually crossed the
//! socket — inputs up, snapshots down, remote participants reconciled —
//! and the second leg runs the data plane through an armed
//! [`ImpairProxy`] (delay/jitter/loss/duplication/reorder, F25-B req 6)
//! instead of a clean loopback.
//!
//! Evidence level: two real processes over real loopback sockets — the
//! first non-in-process driving legs, advancing F25-AC01/AC02's
//! multi-process side. Still loopback scope: no LAN, no rendered
//! output, no retail install (synthetic dev world). The recorded
//! impairment *matrix* (F25-AC03) remains open — this is one recipe
//! exercised over a live session, not a measured grid.

use std::net::SocketAddr;
use std::time::Duration;

mod support;

use mm2_net::{Impair, ImpairProxy, LinkDir};
use support::Proc;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// The in-app host's flags: a dev-world cruise lobby, headless, playing
/// seat 0 once `start` fires. `frames` must outlast every client's
/// budget — a host that exits first drops the sockets and the clients'
/// records read `lost the host`, not a verdict on their own driving.
fn host_args(install: &std::path::Path, frames: u32) -> Vec<String> {
    vec![
        "--mm2-path".into(),
        install.to_str().unwrap().into(),
        "--host".into(),
        "--dev-world".into(),
        "--bind".into(),
        "127.0.0.1:0".into(),
        "--seed".into(),
        "11".into(),
        "--headless".into(),
        "--frames".into(),
        frames.to_string(),
    ]
}

/// A headless joined client: parks in the lobby, readies immediately,
/// offers the dev car, and drives whatever session the host starts.
/// `frames` bounds the whole run — the parked `Menu` wait burns ~4
/// ms/frame of it while the wire decides.
fn join_args(
    install: &std::path::Path,
    addr: SocketAddr,
    driver: &str,
    frames: u32,
) -> Vec<String> {
    vec![
        "--mm2-path".into(),
        install.to_str().unwrap().into(),
        "--join".into(),
        addr.to_string(),
        "--driver".into(),
        driver.into(),
        "--ready".into(),
        "--headless".into(),
        "--frames".into(),
        frames.to_string(),
    ]
}

/// The `mm2 --host` `listening=` record — tracing log lines share
/// stdout, so the record is scanned for rather than assumed first (the
/// dedicated `mm2-host` prints no logs; the app does).
fn listening_addr(host: &Proc) -> SocketAddr {
    host.until("listening=")
        .split_whitespace()
        .find_map(|t| t.strip_prefix("listening="))
        .and_then(|a| a.parse().ok())
        .expect("a listening= record")
}

/// One `smoke=` line's `key=value` field.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|t| t.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("no {key}= field in {line}"))
}

/// Leading digits of a `net=` counter cell (`"1095a"` → 1095).
fn leading_u64(s: &str) -> u64 {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap()
}

/// The `net=in<s>s/<a>a/<x>x,snap<s>s/<a>a,rem<r>,req<s>s/<g>g/<d>d,rspn<n>,dsyn<n>,tsyn<n>,imp<s>s/<a>a/<d>d`
/// record field decoded — the wire counters the run actually moved.
/// `dsyn` (v8 damage writes) is parsed but not asserted: the dev car
/// binds no authored damage record, so a dev-world session has no
/// `VehicleDamage` for the byte to land on and 0 is the honest value.
/// `tsyn` (v9 trailer rows) is the same — the dev car tows nothing —
/// and `imp` (v10 impact rows) likewise: the clean cruise never
/// collides, so all three cells read 0 here.
#[derive(Debug, Default)]
struct NetField {
    inputs_sent: u64,
    inputs_applied: u64,
    snaps_sent: u64,
    snaps_applied: u64,
    remotes: u64,
    remote_spin: u64,
    #[allow(dead_code)]
    damage_synced: u64,
    #[allow(dead_code)]
    trailers_synced: u64,
    #[allow(dead_code)]
    impacts_sent: u64,
    #[allow(dead_code)]
    impacts_applied: u64,
}

fn net_field(line: &str) -> NetField {
    let tok = field(line, "net");
    // `inSs/Aa/Xx`-style cells — strip the name, split the counters,
    // read each cell's leading digits (the suffix letter is a marker).
    let cells = |spec: &str, prefix: &str| -> Vec<u64> {
        spec.strip_prefix(prefix)
            .unwrap_or_else(|| panic!("bad net= cell {spec:?} in {line}"))
            .split('/')
            .map(leading_u64)
            .collect()
    };
    let parts: Vec<&str> = tok.split(',').collect();
    let inputs = cells(parts[0], "in");
    let snaps = cells(parts[1], "snap");
    let impacts = cells(parts[7], "imp");
    NetField {
        inputs_sent: inputs[0],
        inputs_applied: inputs[1],
        snaps_sent: snaps[0],
        snaps_applied: snaps[1],
        remotes: leading_u64(parts[2].strip_prefix("rem").unwrap()),
        remote_spin: leading_u64(parts[4].strip_prefix("rspn").unwrap()),
        damage_synced: leading_u64(parts[5].strip_prefix("dsyn").unwrap()),
        trailers_synced: leading_u64(parts[6].strip_prefix("tsyn").unwrap()),
        impacts_sent: impacts[0],
        impacts_applied: impacts[1],
    }
}

/// Metres the run's own car drove — the `moved=` field (`none` when
/// the session never produced a player).
fn moved_m(line: &str) -> f64 {
    field(line, "moved")
        .strip_suffix('m')
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no numeric moved= in {line}"))
}

/// A client's mid-session record: the frame cap lands while the session
/// is still `Playing`, so the full driving record — `net=` counters
/// included — is what prints.
fn assert_client_drove(rec: &str, min_remotes: u64) {
    assert_eq!(field(rec, "status"), "pass", "{rec}");
    assert_eq!(field(rec, "phase"), "playing", "{rec}");
    assert_eq!(field(rec, "mp"), "gen1", "{rec}");
    assert!(
        moved_m(rec) > 0.0,
        "the client drove its predicted seat: {rec}"
    );
    let net = net_field(rec);
    assert!(net.inputs_sent > 0, "the client streamed inputs: {rec}");
    assert!(
        net.snaps_applied > 0,
        "the client applied authority snapshots: {rec}"
    );
    assert!(
        net.remotes >= min_remotes,
        "expected >= {min_remotes} remote copies: {rec}"
    );
    assert!(
        net.remote_spin > 0,
        "the v7 presentation tail turned a remote copy's wheels: {rec}"
    );
}

/// Wait until both `mm2 --join` clients have readied on the host's
/// record stream (`event=ready id=<n> ready=true`), then start.
fn start_when_ready(host: &mut Proc, clients: u32) {
    for _ in 0..clients {
        host.until("ready=true");
    }
    host.cmd("start");
    host.until("event=started generation=1");
}

/// The host's parked-lobby record after `quit` — the authority side of
/// the session: its `net=` shows the clients' inputs were applied to
/// the simulated remote seats and snapshots went back out. (`quit` is
/// also the host's Cancel→teardown→exit lifecycle leg.)
fn quit_and_assert_host_drove(mut host: Proc) {
    host.cmd("quit");
    let rec = host.until("smoke=headless-physics");
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert_eq!(field(&rec, "phase"), "menu", "{rec}");
    assert!(rec.contains("lobby closed"), "{rec}");
    let net = net_field(&rec);
    assert!(
        net.inputs_applied > 0,
        "the authority applied remote inputs: {rec}"
    );
    assert!(
        net.snaps_sent > 0,
        "the authority broadcast snapshots: {rec}"
    );
    assert!(
        host.wait().success(),
        "mm2 --host did not exit cleanly: {rec}"
    );
}

/// F25-AC01/AC02's first real leg: three `mm2` processes — an in-app
/// host (`--host --headless`, seat 0) and two joined clients (`--join
/// --headless --ready`) — drive one dev-world cruise over plain
/// loopback. Each client's record is read mid-session (its frame cap
/// lands while `phase=playing`): it streamed inputs, applied the host's
/// snapshots, spawned the other participants as remote copies and drove
/// its own predicted seat. Their exits land on the host as clean quits;
/// the host's `quit` record then carries the authority-side counters.
///
/// Bob's cap is shorter so his record prints while alice is still
/// connected — `rem2` there deterministically covers both other
/// participants. Alice caps later: bob's departure may already have
/// pruned his remote from her roster (by design — a leaving peer's
/// remote despawns), so her record only pins `rem>=1`.
#[test]
fn two_mm2_processes_drive_one_session_over_loopback() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), 9000));
    let addr = listening_addr(&host);

    let alice = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "alice", 1400));
    let bob = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "bob", 900));
    start_when_ready(&mut host, 2);

    assert_client_drove(&bob.until("smoke=headless-physics"), 2);
    assert_client_drove(&alice.until("smoke=headless-physics"), 1);
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(bob.wait().success(), "bob did not exit cleanly");

    // The departures land on the wire as the clients' link `Drop`s —
    // a deliberate `Leave`, so a clean quit rather than a lost socket.
    for _ in 0..2 {
        assert!(host.until("event=left").contains("cause=quit"));
    }

    quit_and_assert_host_drove(host);
}

/// The same session with the data plane riding an armed `ImpairProxy`:
/// each client dials the proxy, which relays to the host and applies a
/// seeded per-direction recipe. The lobby verbs cross clean — `Up`
/// arms only once both `ready` echoes prove them delivered, `Down`
/// once `event=started` plus a settle prove `Start` landed; after that
/// the entire driving phase (inputs up, snaps down) runs impaired, and
/// [`LinkStats`] proves the recipe fired on the wire. Convergence is
/// the same client record: predicted driving kept the seat moving while
/// stale/duplicated snaps dropped instead of wedging the session.
#[test]
fn an_impaired_two_process_session_still_converges() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), 9000));
    let addr = listening_addr(&host);
    let proxy = ImpairProxy::loopback_seeded(addr, 0xC0FFEE).unwrap();
    let recipe = Impair {
        delay: Duration::from_millis(40),
        jitter: Duration::from_millis(30),
        loss: 0.05,
        duplicate: 0.10,
        reorder: 0.10,
    };

    let alice = Proc::spawn(
        MM2_EXE,
        &join_args(install.path(), proxy.addr(), "alice", 1600),
    );
    let bob = Proc::spawn(
        MM2_EXE,
        &join_args(install.path(), proxy.addr(), "bob", 1100),
    );
    // `ready=true` echoing means every client→host verb already crossed;
    // arming Up now impairs only what flows next — `Input` frames.
    for _ in 0..2 {
        host.until("ready=true");
    }
    proxy.set(LinkDir::Up, recipe);
    host.cmd("start");
    host.until("event=started generation=1");
    // `Start` rides Down: let the still-clean lane deliver it before
    // the snap stream earns its recipe (a one-shot verb has no
    // retransmit — the data plane is what the harness is for).
    std::thread::sleep(Duration::from_millis(400));
    proxy.set(LinkDir::Down, recipe);

    // Same cap staggering as the clean leg: bob's record pins rem2
    // while alice is still connected; alice's pins rem>=1.
    assert_client_drove(&bob.until("smoke=headless-physics"), 2);
    assert_client_drove(&alice.until("smoke=headless-physics"), 1);
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(bob.wait().success(), "bob did not exit cleanly");

    // The recipe really fired — both directions carried data-plane
    // frames and each lost/duplicated/reordered some of them.
    let up = proxy.stats(LinkDir::Up);
    let down = proxy.stats(LinkDir::Down);
    assert!(up.frames_in > 0 && up.frames_out > 0, "{up:?}");
    assert!(down.frames_in > 0 && down.frames_out > 0, "{down:?}");
    assert!(
        up.dropped + up.duplicated + up.reordered > 0,
        "no impairment observed upstream: {up:?}"
    );
    assert!(
        down.dropped + down.duplicated + down.reordered > 0,
        "no impairment observed downstream: {down:?}"
    );
    drop(proxy);

    for _ in 0..2 {
        host.until("event=left");
    }
    quit_and_assert_host_drove(host);
}
