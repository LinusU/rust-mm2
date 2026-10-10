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
//! Evidence level: real OS processes over real loopback sockets — the
//! first non-in-process driving legs, advancing F25-AC01/AC02's
//! multi-process side. Still loopback scope: no LAN, no rendered
//! output, no retail install (synthetic dev world). The third leg is
//! the measured impairment *matrix* (F25-AC03) at process level: the
//! same eight named recipe cells the in-process `net_app` grid runs,
//! each a fresh host + proxy + client-pair set so every cell's
//! counters start at zero.

use std::net::SocketAddr;
use std::time::Duration;

use crate::support;

use mm2_net::{Impair, ImpairProxy, LinkDir, LinkStats};
use support::Proc;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// The host's `--frames` cap, only a ceiling: every leg ends the host
/// with `quit`. A host that reaches its cap stops draining lobby events
/// (no further `event=left`), and a slow link or loaded runner makes
/// the clients take longer while the host keeps counting frames, so the
/// cap must sit well past any bounded wait rather than race it.
const HOST_FRAME_CEILING: u32 = 500_000;

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

/// The `net=in<s>s/<a>a/<x>x,snap<s>s/<a>a/<x>x,rem<r>,req<s>s/<g>g/<d>d,rspn<n>,dsyn<n>,tsyn<n>,imp<s>s/<a>a/<d>d,rb<d>d/<r>r,race<a>a/<d>d,prog<a>a/<d>d,stall<n>,surf<s>s/<a>a,fix<n>`
/// record field decoded — the wire counters the run actually moved.
/// `stall` (F25-B wire-seat retirements) is authority-side only and
/// reads 0 on a clean run — it is not parsed here since nothing in
/// this suite stalls a seat.
/// `snap<x>` counts pose frames the client dropped stale at push: a
/// duplicated or reordered `Snap` at/behind the staged-or-applied
/// watermark (F25-AC03 stale-state evidence). `dsyn` (v8 damage
/// writes) is parsed but not asserted: the dev car binds no authored
/// damage record, so a dev-world session has no `VehicleDamage` for
/// the byte to land on and 0 is the honest value.
/// `tsyn` (v9 trailer rows) is the same — the dev car tows nothing —
/// `imp` (v10 impact rows) likewise (the clean cruise never
/// collides), `rb` (v11 breakaway-mask transitions) too — the dev
/// car authors no breakable parts — and `race` (v13 race rows) plus
/// `prog` (v14 per-seat progress tails) read 0 on both sides: the dev
/// cruise carries no `RaceState` — so all six cells read 0 here.
/// `surf` (v16 surface tails carrying a resolved contact — sent on
/// the authority, applied on a client) reads 0 on a surface-table-
/// less install and counts on the fixture pair's.
#[derive(Debug, Default)]
struct NetField {
    inputs_sent: u64,
    inputs_applied: u64,
    snaps_sent: u64,
    snaps_applied: u64,
    snaps_staled: u64,
    remotes: u64,
    remote_spin: u64,
    #[allow(dead_code)]
    damage_synced: u64,
    #[allow(dead_code)]
    trailers_synced: u64,
    impacts_sent: u64,
    impacts_applied: u64,
    #[allow(dead_code)]
    breaks_detached: u64,
    #[allow(dead_code)]
    breaks_restored: u64,
    #[allow(dead_code)]
    race_applied: u64,
    #[allow(dead_code)]
    race_dropped: u64,
    #[allow(dead_code)]
    progress_applied: u64,
    #[allow(dead_code)]
    progress_dropped: u64,
    surfaces_sent: u64,
    surfaces_applied: u64,
    own_settles: u64,
    requests_sent: u64,
    requests_granted: u64,
    requests_dropped: u64,
    /// The trailing `rst<n>` cell, printed only once non-zero.
    resets: u64,
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
    let breaks = cells(parts[8], "rb");
    let race = cells(parts[9], "race");
    let prog = cells(parts[10], "prog");
    let surf = cells(parts[12], "surf");
    let own_settles = leading_u64(parts[13].strip_prefix("fix").unwrap());
    let req = cells(parts[3], "req");
    let resets = parts
        .get(14)
        .map_or(0, |p| leading_u64(p.strip_prefix("rst").unwrap()));
    NetField {
        inputs_sent: inputs[0],
        inputs_applied: inputs[1],
        snaps_sent: snaps[0],
        snaps_applied: snaps[1],
        snaps_staled: snaps[2],
        remotes: leading_u64(parts[2].strip_prefix("rem").unwrap()),
        remote_spin: leading_u64(parts[4].strip_prefix("rspn").unwrap()),
        damage_synced: leading_u64(parts[5].strip_prefix("dsyn").unwrap()),
        trailers_synced: leading_u64(parts[6].strip_prefix("tsyn").unwrap()),
        impacts_sent: impacts[0],
        impacts_applied: impacts[1],
        breaks_detached: breaks[0],
        breaks_restored: breaks[1],
        race_applied: race[0],
        race_dropped: race[1],
        progress_applied: prog[0],
        progress_dropped: prog[1],
        surfaces_sent: surf[0],
        surfaces_applied: surf[1],
        own_settles,
        requests_sent: req[0],
        requests_granted: req[1],
        requests_dropped: req[2],
        resets,
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
/// included — is what prints. Returns the decoded field so a leg can
/// assert counters beyond the convergence floor.
fn assert_client_drove(rec: &str, min_remotes: u64) -> NetField {
    assert_client_drove_round(rec, min_remotes, 1)
}

/// [`assert_client_drove`] for the `round`-th session of one lobby —
/// the record's `mp=gen<n>` names the generation the client is in.
fn assert_client_drove_round(rec: &str, min_remotes: u64, round: u64) -> NetField {
    assert_eq!(field(rec, "status"), "pass", "{rec}");
    assert_eq!(field(rec, "phase"), "playing", "{rec}");
    assert_eq!(field(rec, "mp"), format!("gen{round}"), "{rec}");
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
    net
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

/// The `aud=` field's surface cells — `/Nk/Ng` the loop voices
/// audible on the record update, `/NK/NG` the cumulative spawns.
/// Absent `aud=` (no audio activity) reads as zeros. The spawned
/// counts are what a leg asserts: the audible gauges sample one
/// update — a record landing on an airborne or contact-faded frame
/// reads 0 without disproving the voice ever resolved.
fn surface_counts(line: &str) -> (u64, u64, u64, u64) {
    let Some(aud) = line.split_whitespace().find_map(|t| t.strip_prefix("aud=")) else {
        return (0, 0, 0, 0);
    };
    let (mut k, mut g, mut sk, mut rg) = (0, 0, 0, 0);
    for tok in aud.split('/') {
        for (suffix, slot) in [('k', &mut k), ('g', &mut g), ('K', &mut sk), ('G', &mut rg)] {
            if let Some(n) = tok.strip_suffix(suffix) {
                *slot = n
                    .parse()
                    .unwrap_or_else(|_| panic!("bad surface gauge in {line}"));
            }
        }
    }
    (k, g, sk, rg)
}

/// The host's parked-lobby record after `quit` — the authority side of
/// the session: its `net=` shows the clients' inputs were applied to
/// the simulated remote seats and snapshots went back out. (`quit` is
/// also the host's Cancel→teardown→exit lifecycle leg.)
fn quit_and_assert_host_drove(mut host: Proc) -> NetField {
    host.cmd("quit");
    let rec = host.until_within("smoke=headless-physics", Duration::from_secs(60));
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
    net
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

/// One reset run: a host and two joined clients, alice with
/// `--reset-at 1200 --until-resets 1`, bob with `--until-peer-left`,
/// both ending on their conditions (frames are only the ceiling) under
/// a wall-clock deadline. With `impair`, both reach the host through a
/// seeded [`ImpairProxy`] armed on both directions once `Start` crossed
/// clean. Returns the three records and the proxy's final up/down
/// counters.
fn run_reset_pair(
    install: &std::path::Path,
    impair: Option<Impair>,
) -> (String, String, String, Option<(LinkStats, LinkStats)>) {
    let mut host = Proc::spawn(MM2_EXE, &host_args(install, HOST_FRAME_CEILING));
    let addr = listening_addr(&host);
    let proxy = impair.map(|_| ImpairProxy::loopback_seeded(addr, 0xAC04).unwrap());
    let join_addr = proxy.as_ref().map_or(addr, ImpairProxy::addr);

    let mut alice_args = join_args(install, join_addr, "alice", 6000);
    alice_args.extend(
        [
            "--reset-at",
            "1200",
            "--until-resets",
            "1",
            "--deadline",
            "90",
        ]
        .map(String::from),
    );
    let alice = Proc::spawn(MM2_EXE, &alice_args);
    let mut bob_args = join_args(install, join_addr, "bob", 6000);
    // Bob is the control seat: he asks nothing. On a clean link he stays
    // until alice has left; through a lossy relay alice's one-shot
    // `Leave` can be dropped and the relay keeps her socket open, so he
    // would never see her go — there he just runs his frame budget.
    if impair.is_none() {
        bob_args.extend(["--until-peer-left", "--deadline", "90"].map(String::from));
    }
    let bob = Proc::spawn(MM2_EXE, &bob_args);
    start_when_ready(&mut host, 2);
    if let (Some(proxy), Some(impair)) = (&proxy, impair) {
        // `Start` rides Down: let the clean lane deliver it first.
        std::thread::sleep(Duration::from_millis(400));
        proxy.set(LinkDir::Up, impair);
        proxy.set(LinkDir::Down, impair);
    }

    let bound = Duration::from_secs(90 + 60);
    let bob_rec = bob.until_within("smoke=headless-physics", bound);
    let alice_rec = alice.until_within("smoke=headless-physics", bound);
    alice.wait_success("alice", &alice_rec);
    bob.wait_success("bob", &bob_rec);
    let link = proxy
        .as_ref()
        .map(|p| (p.stats(LinkDir::Up), p.stats(LinkDir::Down)));
    // The relay holds socket clones until it drops: the host only sees
    // the clients leave once the proxy is down.
    let relayed = proxy.is_some();
    drop(proxy);
    for _ in 0..2 {
        let left = host.until("event=left");
        // Through the relay the departure reads as a closed socket.
        assert!(relayed || left.contains("cause=quit"), "{left}");
    }
    host.cmd("quit");
    let host_rec = host.until("smoke=headless-physics");
    assert!(host.wait().success(), "mm2 --host did not exit cleanly");
    (alice_rec, bob_rec, host_rec, link)
}

/// F25-C's reset leg at process level: a joined client run with
/// `--reset-at` asks the host for a reset over the real socket (the
/// wire form of the `R` key — it never teleports itself), the host
/// grants exactly that one ask to exactly that seat, and the epoch
/// declaration comes back on the `Snap` stream so the client applies
/// it to its own seat. The other client, run without the flag, asks
/// nothing, and the host drops nothing.
#[test]
fn a_scheduled_reset_crosses_two_processes_and_comes_back() {
    let install = tempfile::tempdir().unwrap();
    let (alice_rec, bob_rec, host_rec, _) = run_reset_pair(install.path(), None);

    assert_eq!(field(&bob_rec, "stop"), "peer-left", "{bob_rec}");
    // Whether bob also saw alice's copy teleport (`rst`) is a race with
    // her leaving, so it is reported, not asserted.
    let bob_net = assert_client_drove(&bob_rec, 1);
    assert_eq!(
        bob_net.requests_sent, 0,
        "the control client asked nothing: {bob_rec}"
    );

    assert_eq!(field(&alice_rec, "stop"), "resets", "{alice_rec}");
    // Not `assert_client_drove`: that one wants `moved > 0`, and the
    // reset is exactly what puts alice back at zero.
    assert_eq!(field(&alice_rec, "status"), "pass", "{alice_rec}");
    assert_eq!(field(&alice_rec, "phase"), "playing", "{alice_rec}");
    let alice_net = net_field(&alice_rec);
    assert!(
        alice_net.inputs_sent > 0 && alice_net.snaps_applied > 0,
        "alice streamed inputs and applied snapshots: {alice_rec}"
    );
    assert_eq!(
        alice_net.requests_sent, 1,
        "--reset-at asked once, not once per update: {alice_rec}"
    );
    assert!(
        alice_net.resets >= 1,
        "the granted reset came back as an epoch snap the client applied: {alice_rec}"
    );
    // The reset really moved her: she drove far (`travel=` is the
    // distance covered) and ended back on her grid slot (`moved=` is
    // the distance from where she started, `0m` once reseated).
    let travel: f64 = field(&alice_rec, "travel")
        .strip_suffix('m')
        .and_then(|n| n.parse().ok())
        .unwrap();
    assert!(travel > 50.0, "alice drove before the reset: {alice_rec}");
    assert!(
        moved_m(&alice_rec) < 5.0,
        "the granted reset put alice back on her slot: {alice_rec}"
    );

    assert_eq!(field(&host_rec, "status"), "pass", "{host_rec}");
    let host_net = net_field(&host_rec);
    assert_eq!(
        (host_net.requests_granted, host_net.requests_dropped),
        (1, 0),
        "the host granted alice's one ask and dropped nothing: {host_rec}"
    );
    assert!(
        host_net.resets >= 1,
        "the grant bumped the seat's wire epoch on the host: {host_rec}"
    );
}

/// The same reset on a bad link (AC02 × AC03): both clients reach the
/// host through a recipe with 30 % loss, duplication, reordering and
/// latency armed on both directions for the driving window. The ask is
/// one frame that the link may lose, so alice repeats it until her own
/// seat's reset comes back (`RESET_RETRY`) — the count of asks is
/// therefore a floor, not exact — and a duplicated or repeated ask the
/// host sees inside its cooldown is dropped counted, never granted
/// twice in a row. What must hold is the clean leg's outcome: she
/// travelled, the host granted at least one ask to her seat, the epoch
/// came back through the lossy snap stream, and she ended on her slot.
#[test]
fn a_scheduled_reset_comes_back_across_two_processes_on_an_impaired_link() {
    let install = tempfile::tempdir().unwrap();
    let recipe = Impair {
        delay: Duration::from_millis(40),
        jitter: Duration::from_millis(30),
        loss: 0.30,
        duplicate: 0.10,
        reorder: 0.10,
    };
    let (alice_rec, bob_rec, host_rec, link) = run_reset_pair(install.path(), Some(recipe));
    eprintln!("impaired reset alice={alice_rec}\n bob={bob_rec}\n host={host_rec}");

    // The recipe really bit while the reset played out.
    let (up, down) = link.expect("the impaired run reports its proxy counters");
    for (dir, stats) in [("up", up), ("down", down)] {
        assert!(stats.frames_in > 0, "{dir} carried nothing: {stats:?}");
        assert!(
            stats.dropped + stats.duplicated + stats.reordered > 0,
            "{dir} saw no impairment: {stats:?}"
        );
    }

    assert_eq!(field(&bob_rec, "status"), "pass", "{bob_rec}");
    assert_eq!(net_field(&bob_rec).requests_sent, 0, "{bob_rec}");

    assert_eq!(field(&alice_rec, "stop"), "resets", "{alice_rec}");
    assert_eq!(field(&alice_rec, "status"), "pass", "{alice_rec}");
    let alice_net = net_field(&alice_rec);
    assert!(
        alice_net.requests_sent >= 1 && alice_net.resets >= 1,
        "alice asked and her reset came back: {alice_rec}"
    );
    let travel: f64 = field(&alice_rec, "travel")
        .strip_suffix('m')
        .and_then(|n| n.parse().ok())
        .unwrap();
    assert!(travel > 50.0, "alice drove before the reset: {alice_rec}");
    assert!(
        moved_m(&alice_rec) < 5.0,
        "the granted reset put alice back on her slot: {alice_rec}"
    );

    assert_eq!(field(&host_rec, "status"), "pass", "{host_rec}");
    let host_net = net_field(&host_rec);
    assert!(
        host_net.requests_granted >= 1 && host_net.resets >= 1,
        "the host granted an ask and bumped the epoch: {host_rec}"
    );
    assert!(
        host_net.requests_granted + host_net.requests_dropped <= alice_net.requests_sent * 2,
        "the host cannot have seen more asks than were sent (plus duplicates): {host_rec}"
    );
}

/// F26-AC05's rematch leg at process level: one lobby, two sessions.
/// The host closes round 1 with `cancel` (the roster returns to the
/// lobby and every ready flag clears), the client — which asked for
/// `--ready` once, on the command line — readies itself again, and
/// `start` mints generation 2 for the *same* two processes without a
/// reconnect. The client's cap record then names `gen2`, driving and
/// applying snapshots; the host's names the clean lobby close. The
/// round boundary is gated on the host spawning the client's seat
/// (`remote participant spawned`), so round 1 really ran before it was
/// cancelled, and round 2's second spawn is the fresh world's.
#[test]
fn two_mm2_processes_play_a_rematch_without_reconnecting() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), 12000));
    let addr = listening_addr(&host);
    let alice = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "alice", 4500));
    start_when_ready(&mut host, 1);
    host.until("remote participant spawned");

    host.cmd("cancel");
    host.until("event=cancelled generation=1");
    // Nobody pressed anything: the readiness came from the client's
    // `--ready`, after the host's cancel cleared it.
    host.until("ready=true");
    host.cmd("start");
    host.until("event=started generation=2");
    host.until("remote participant spawned");

    // The cap lands whenever the client's frame budget runs out, which
    // on a loaded machine is later than the idle-time default allows.
    let rec = alice.until_within("smoke=headless-physics", Duration::from_secs(120));
    assert_client_drove_round(&rec, 1, 2);
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(host.until("event=left").contains("cause=quit"));
    quit_and_assert_host_drove(host);
}

/// F26-AC05's changed rematch at process level, through the operator's
/// own surface: the host's stdin `session …` word. Mid-round it is
/// refused (the running round's peers are never handed another ad);
/// after `cancel` it re-advertises rainy weather, the same client
/// process (no reconnect) re-readies and plays generation 2 under it —
/// its cap record carries the wet-road `traction=0.8` round 1 never
/// had — and a malformed word is named on stderr without touching the
/// advertisement. Dev world over loopback: the retail city/mode change
/// is the in-process `net_app` leg's fixture, not this one.
#[test]
fn a_session_edit_typed_on_the_host_reaches_the_next_round() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), 12000));
    let addr = listening_addr(&host);
    let alice = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "alice", 4500));
    start_when_ready(&mut host, 1);
    host.until("remote participant spawned");

    host.cmd("session weather=3");
    let refused = host.until("event=session_refused");
    assert!(refused.contains("a session is running"), "{refused}");

    host.cmd("cancel");
    host.until("event=cancelled generation=1");
    host.until("ready=true");
    host.cmd("session weather=3 seed=4242");
    host.until("event=session_changed");
    host.cmd("start");
    host.until("event=started generation=2");
    host.until("remote participant spawned");

    let rec = alice.until_within("smoke=headless-physics", Duration::from_secs(120));
    assert_client_drove_round(&rec, 1, 2);
    assert!(rec.contains("traction=0.8"), "round 2 is not wet: {rec}");
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(host.until("event=left").contains("cause=quit"));
    quit_and_assert_host_drove(host);
}

/// F26-AC02's late-join leg at process level (req 4): the host starts
/// generation 1 *alone* (the empty-roster start gate passes, the open
/// cruise policy admits joiners — MP-5), plays on, and only then does a
/// client connect. It must be handed the running session — `Start`
/// carrying generation 1, not a fresh private one — load it, spawn the
/// host's seat as a remote copy and stream inputs, while the host
/// spawns the late seat and applies its inputs. Nothing about the
/// session is replayed from the start: the client's cap record names
/// `gen1`, the host's lobby never re-minted a generation.
#[test]
fn a_client_that_joins_a_running_session_is_handed_the_live_one() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), 12000));
    let addr = listening_addr(&host);

    host.cmd("start");
    host.until("event=started generation=1");
    // The session is running with nobody else in it.
    let late = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "late", 4500));
    host.until("remote participant spawned");

    let rec = late.until_within("smoke=headless-physics", Duration::from_secs(120));
    assert_client_drove(&rec, 1);
    assert!(
        late.wait().success(),
        "the late joiner did not exit cleanly"
    );
    assert!(host.until("event=left").contains("cause=quit"));
    quit_and_assert_host_drove(host);
}

/// F26-AC02's "not default noon" leg at process level: the host is
/// started with rain (`--weather 3`) and sits in generation 1 alone;
/// a client that connects *afterwards* must come up in the same wet
/// session, not in a default dry one. The wetness is read off each
/// record's `traction=` cell — `TireConditions.traction` is the
/// weather's gameplay consequence (F18-B.1), so a joiner that rebuilt
/// its session from defaults would read no cell at all (the record
/// omits a unit factor). The host's own record is no witness: by
/// the time it prints, the joiner's departure has already ended the
/// session (`phase=menu`).
///
/// Dev world, synthetic install: the visual half (which `.ltNN` preset
/// lit the scene) needs a city and is the retail leg below.
#[test]
fn a_late_joiner_inherits_the_hosts_weather_not_the_default() {
    let install = tempfile::tempdir().unwrap();
    let mut args = host_args(install.path(), 12000);
    args.extend(["--weather".into(), "3".into()]);
    let mut host = Proc::spawn(MM2_EXE, &args);
    let addr = listening_addr(&host);

    host.cmd("start");
    host.until("event=started generation=1");
    let late = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "late", 4500));
    host.until("remote participant spawned");

    let rec = late.until_within("smoke=headless-physics", Duration::from_secs(120));
    assert_client_drove(&rec, 1);
    assert_eq!(field(&rec, "mp"), "gen1", "{rec}");
    assert!(
        rec.contains("traction=0.8"),
        "the late joiner is not in the host's rain: {rec}"
    );
    assert!(
        late.wait().success(),
        "the late joiner did not exit cleanly"
    );
    assert!(host.until("event=left").contains("cause=quit"));
    quit_and_assert_host_drove(host);
}

/// F06-AC06's two-process proof: the authority is a *rainy* host
/// (`--weather 3`) and the joined client launches with its own
/// `--traction 0.4` pin, which `net::start` stamps onto the config the
/// accepted session hands it. The client's record must carry the
/// host's wetness factor instead — `session_traction` ignores a pin on
/// a `Remote` process and names the ignored override on the record
/// stream rather than dropping it silently — so both OS processes drive
/// one authority-owned surface state, and the multiplier its tire path
/// applies to every wheel's authored grip is the authority's, not its
/// own.
///
/// The pin can only ever be the *client's* here: `advertise` refuses a
/// session whose `dev` set is non-default, so a pinned host cannot be
/// joined at all. Dev world over loopback; the rainy `0.8` itself is
/// the designed DSN-59 factor (UNK-39 stands — the original's
/// weather→grip rule is unrecovered).
#[test]
fn a_pinned_client_follows_the_authoritys_surface_state() {
    let install = tempfile::tempdir().unwrap();
    let mut args = host_args(install.path(), 12000);
    args.extend(["--weather".into(), "3".into()]);
    let mut host = Proc::spawn(MM2_EXE, &args);
    let addr = listening_addr(&host);

    let mut alice_args = join_args(install.path(), addr, "alice", 4500);
    alice_args.extend(["--traction".into(), "0.4".into()]);
    let alice = Proc::spawn(MM2_EXE, &alice_args);
    start_when_ready(&mut host, 1);
    host.until("remote participant spawned");

    // The ignored pin is announced when the client's session loads —
    // an override that did not bind must never be silent.
    alice.until("--traction ignored");
    let rec = alice.until_within("smoke=headless-physics", Duration::from_secs(120));
    assert_client_drove(&rec, 1);
    assert!(
        rec.contains("traction=0.8"),
        "the client's tire path took the host's rain, not its own pin: {rec}"
    );
    assert!(
        !rec.contains("traction=0.4"),
        "the client's ignored pin leaked into its surface state: {rec}"
    );
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(host.until("event=left").contains("cause=quit"));
    quit_and_assert_host_drove(host);
}

/// The `env=lt<NN>(<preset>) fog=… sky=…` cells of a record — which
/// authored light preset, fog and sky the session bound.
fn env_cells(line: &str) -> String {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let at = tokens
        .iter()
        .position(|t| t.starts_with("env="))
        .unwrap_or_else(|| panic!("no env= field in {line}"));
    tokens[at..(at + 3).min(tokens.len())].join(" ")
}

/// The same on a real city, where the conditions have a visible
/// consequence: a retail sf Cruise hosted at a non-default time of day
/// and weather, a late joiner arriving after it started, and the
/// joiner's lighting preset must be the one those conditions bind in a
/// plain single-player run — and not the one a default-conditions run
/// binds. (The host's own record is no witness: it prints after the
/// session has ended.) Skipped without the operator's install
/// (`MM2_RETAIL=<dir>`), like the other retail legs.
///
/// Same machine, loopback, headless: it shows the joiner *bound* the
/// host's preset, not how it looks.
#[test]
fn a_late_joiner_lights_the_scene_like_the_host_on_a_retail_city() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let conditions = ["--time-of-day", "2", "--weather", "3"];
    // The preset a solo run binds, for the default and the hosted
    // conditions: the oracle the joiner is compared against.
    let solo = |extra: &[&str]| {
        let mut args: Vec<String> = vec![
            "--mm2-path".into(),
            retail.to_str().unwrap().into(),
            "--city".into(),
            "sf".into(),
            "--headless".into(),
            "--frames".into(),
            "60".into(),
        ];
        args.extend(extra.iter().map(|a| a.to_string()));
        let proc = Proc::spawn(MM2_EXE, &args);
        let rec = proc.until_within("smoke=headless-physics", Duration::from_secs(240));
        assert_eq!(field(&rec, "status"), "pass", "{rec}");
        env_cells(&rec)
    };
    let default_env = solo(&[]);
    let expected_env = solo(&conditions);
    assert_ne!(
        expected_env, default_env,
        "the conditions bind the default preset, so this leg proves nothing"
    );

    let mut host_args = host_args(&retail, 9000);
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(["--city".into(), "sf".into()]);
    host_args.extend(conditions.iter().map(|a| a.to_string()));
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    host.cmd("start");
    host.until("event=started generation=1");
    let late = Proc::spawn(MM2_EXE, &join_args(&retail, addr, "late", 1000));
    host.until("remote participant spawned");

    let rec = late.until_within("smoke=headless-physics", Duration::from_secs(240));
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert_eq!(field(&rec, "mp"), "gen1", "{rec}");
    assert_eq!(
        env_cells(&rec),
        expected_env,
        "the late joiner lit the scene differently from the hosted conditions: {rec}"
    );
    assert!(
        late.wait().success(),
        "the late joiner did not exit cleanly"
    );
    quit_and_assert_host_drove(host);
    eprintln!("default {default_env}\nhosted  {expected_env}\nlate    {rec}");
}

/// F26-AC02's reconnect-shaped leg at process level: a late joiner
/// plays and leaves, then a *second* process joins the same running
/// session. Identity is not carried across a reconnect (the wire id is
/// minted per connection), so the leg pins what must hold instead —
/// the leaver's seat is gone before the newcomer arrives, the
/// newcomer is handed the live generation 1 (not a re-minted one),
/// gets a seat of its own under a fresh wire id, and sees exactly one
/// other participant: the host. A leaver's remote copy resurrected for
/// the next joiner would read `rem2`.
#[test]
fn a_seat_freed_by_a_leaver_is_not_resurrected_for_the_next_joiner() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), 12000));
    let addr = listening_addr(&host);

    host.cmd("start");
    host.until("event=started generation=1");
    let first = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "first", 2500));
    let spawned_first = host.until("remote participant spawned");
    let rec = first.until_within("smoke=headless-physics", Duration::from_secs(120));
    assert_client_drove(&rec, 1);
    assert!(
        first.wait().success(),
        "the first joiner did not exit cleanly"
    );
    assert!(host.until("event=left").contains("cause=quit"));

    let second = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "second", 4500));
    let spawned_second = host.until("remote participant spawned");
    // The log line is colourised, so `player=<n>` is read off its tail.
    let wire_id = |line: &str| -> u64 {
        line.rsplit(|c: char| !c.is_ascii_digit())
            .find(|d| !d.is_empty())
            .and_then(|d| d.parse().ok())
            .unwrap_or_else(|| panic!("no wire id in {line}"))
    };
    assert_ne!(
        wire_id(&spawned_first),
        wire_id(&spawned_second),
        "the newcomer reused the leaver's wire id: {spawned_first} / {spawned_second}"
    );
    let rec = second.until_within("smoke=headless-physics", Duration::from_secs(120));
    // Not `assert_client_drove`: by now the host's own car has driven
    // to the end of the dev world and sits still, so the copy's wheels
    // legitimately read `spin0` (the first joiner, arriving at the
    // host's start, covers the spinning copy).
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert_eq!(field(&rec, "phase"), "playing", "{rec}");
    assert_eq!(field(&rec, "mp"), "gen1", "{rec}");
    assert!(
        moved_m(&rec) > 0.0,
        "the newcomer drove its own seat: {rec}"
    );
    let net = net_field(&rec);
    assert!(net.inputs_sent > 0, "the newcomer streamed inputs: {rec}");
    assert!(
        net.snaps_applied > 0,
        "the newcomer applied authority snapshots: {rec}"
    );
    assert_eq!(
        net.remotes, 1,
        "only the host is left to copy — the leaver's seat must not return: {rec}"
    );
    assert!(
        second.wait().success(),
        "the second joiner did not exit cleanly"
    );
    assert!(host.until("event=left").contains("cause=quit"));
    quit_and_assert_host_drove(host);
}

/// The v16 surface tail at process level — the review gap the v16
/// landing disclosed: every earlier leg either staged the
/// `SurfaceContact` by hand (in-process) or had nothing to resolve
/// (a dev world mounted no `materials` pair). With the fixture pair +
/// dry table on the install, this leg is live-resolved end to end:
/// the host's real `vehicle_simulation` grounds the remote seats'
/// wheels on the dev-world colliders, `surface_voices` resolves the
/// `Unspecified` → `_default` → `sound 1` (grass) chain into a real
/// `SurfaceContact`, `publish_snapshots` encodes it onto the wire,
/// and each client's `apply_snapshots` decodes it onto the remote
/// copies' `SurfaceContact` — which the client's own `surface_voices`
/// then replays into a rolling-loop voice off the same authored
/// waves. `surf<a>` counts rows that landed; `surf<s>` the rows the
/// authority broadcast; `aud=`'s `G` spawn counter is the replayed
/// voice itself. Nothing is staged: the same leg on a table-less
/// install (the clean leg above) reads `surf0s/0a`.
#[test]
fn two_mm2_processes_relay_a_live_resolved_surface_contact() {
    let install = tempfile::tempdir().unwrap();
    support::surface_audio(install.path());
    support::surface_materials(install.path());
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), 9000));
    let addr = listening_addr(&host);

    let alice = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "alice", 1400));
    let bob = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "bob", 500));
    start_when_ready(&mut host, 2);

    // Bob's shorter cap lands mid-drive — the record while the cars
    // still cruise, before they pin against the dev world's
    // perimeter wall like the clean leg's 900-frame end-state.
    let bob_rec = bob.until("smoke=headless-physics");
    let bob_net = assert_client_drove(&bob_rec, 2);
    assert!(
        bob_net.surfaces_applied > 0,
        "live-resolved surface tails landed on bob's remote copies: {bob_rec}"
    );
    // `rolling_voices` counts the client's own seat too — `>= 2`
    // pins at least one remote copy replaying its replicated
    // contact through bob's own table. Cumulative, not the live
    // gauge: the record update can land on an airborne or faded
    // frame, but a spawned voice is permanent proof the pick ran.
    let (_, _, _, rolling) = surface_counts(&bob_rec);
    assert!(
        rolling >= 2,
        "a remote copy voices its replicated rolling loop: {bob_rec}"
    );

    let alice_rec = alice.until("smoke=headless-physics");
    let alice_net = assert_client_drove(&alice_rec, 1);
    assert!(
        alice_net.surfaces_applied > 0,
        "live-resolved surface tails landed on alice's remote copies: {alice_rec}"
    );
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(bob.wait().success(), "bob did not exit cleanly");

    for _ in 0..2 {
        assert!(host.until("event=left").contains("cause=quit"));
    }

    let host_net = quit_and_assert_host_drove(host);
    assert!(
        host_net.surfaces_sent > 0,
        "the authority broadcast live-resolved surface contacts"
    );
}

/// The same session with the data plane riding an armed `ImpairProxy`:
/// each client dials the proxy, which relays to the host and applies a
/// seeded per-direction recipe. The lobby verbs cross clean — `Up`
/// arms only once both `ready` echoes prove them delivered, `Down`
/// once `event=started` plus a settle prove `Start` landed; after that
/// the entire driving phase (inputs up, snaps down) runs impaired, and
/// [`LinkStats`] proves the recipe fired on the wire — frame counts,
/// the payload `bytes_*` bandwidth leg, and the `delayed` count of
/// frames that paid the recipe's positive hold. Convergence is the
/// same client record: predicted driving kept the seat moving while
/// stale/duplicated snaps dropped counted at the push watermark
/// (`snap<x>`) instead of wedging the session.
#[test]
fn an_impaired_two_process_session_still_converges() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), HOST_FRAME_CEILING));
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
    // These processes are silent while driving. Bound the whole run rather
    // than a 15-second per-line wait that parallel children can exhaust.
    let bound = Duration::from_secs(90);
    let bob_net = assert_client_drove(&bob.until_within("smoke=headless-physics", bound), 2);
    let alice_net = assert_client_drove(&alice.until_within("smoke=headless-physics", bound), 1);
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(bob.wait().success(), "bob did not exit cleanly");

    // The recipe really fired — both directions carried data-plane
    // frames and each lost/duplicated/reordered some of them. The
    // byte counters are the payload volume behind those frame counts
    // (each duplicated copy re-pays bytes_out); `delayed` counts the
    // frames the 40 ms hold actually scheduled.
    let up = proxy.stats(LinkDir::Up);
    let down = proxy.stats(LinkDir::Down);
    assert!(up.frames_in > 0 && up.frames_out > 0, "{up:?}");
    assert!(down.frames_in > 0 && down.frames_out > 0, "{down:?}");
    assert!(up.bytes_in > 0 && up.bytes_out > 0, "{up:?}");
    assert!(down.bytes_in > 0 && down.bytes_out > 0, "{down:?}");
    assert!(up.delayed > 0, "the delay recipe held frames up: {up:?}");
    assert!(
        down.delayed > 0,
        "the delay recipe held frames down: {down:?}"
    );
    assert!(
        up.dropped + up.duplicated + up.reordered > 0,
        "no impairment observed upstream: {up:?}"
    );
    assert!(
        down.dropped + down.duplicated + down.reordered > 0,
        "no impairment observed downstream: {down:?}"
    );
    // The client side of the same evidence: duplicated and reordered
    // snaps arrive at/behind the push watermark and drop counted —
    // the AC03 stale-state measure.
    assert!(
        alice_net.snaps_staled + bob_net.snaps_staled > 0,
        "no stale snap drops recorded on either client"
    );
    drop(proxy);

    for _ in 0..2 {
        host.until("event=left");
    }
    quit_and_assert_host_drove(host);
}

/// One cell of the process-level impairment grid — a named [`Impair`]
/// recipe armed on both directions for the driving window. The table
/// is `net_app`'s in-process matrix verbatim — keep the two in step
/// so their recorded runs stay directly comparable.
struct MatrixCell {
    name: &'static str,
    impair: Impair,
}

/// Run one matrix cell end to end: a fresh host, a fresh seeded
/// [`ImpairProxy`], and a fresh client pair — every counter starts at
/// zero, so the cell's [`LinkStats`] are whole-connection sums like
/// the in-process grid's. Returns the cell's wire counters plus both
/// clients' decoded `net=` records for the per-cell floors.
fn run_matrix_cell(
    install: &std::path::Path,
    cell: &MatrixCell,
) -> (LinkStats, LinkStats, NetField, NetField) {
    let mut host = Proc::spawn(MM2_EXE, &host_args(install, HOST_FRAME_CEILING));
    let addr = listening_addr(&host);
    let proxy = ImpairProxy::loopback_seeded(addr, 0xAC03).unwrap();

    let alice = Proc::spawn(MM2_EXE, &join_args(install, proxy.addr(), "alice", 1400));
    let bob = Proc::spawn(MM2_EXE, &join_args(install, proxy.addr(), "bob", 1000));
    start_when_ready(&mut host, 2);
    // `Start` rides Down: let the still-clean lane deliver it before
    // the snap stream earns its recipe — the same one-shot-verb
    // reasoning as the single-recipe leg above. The lobby phase
    // crossing clean is what makes every armed frame a data-plane
    // frame (`Input` up, `Snap` down).
    std::thread::sleep(Duration::from_millis(400));
    proxy.set(LinkDir::Up, cell.impair);
    proxy.set(LinkDir::Down, cell.impair);

    // Same cap staggering as the other legs: bob's record pins rem2
    // while alice is still connected; alice's pins rem>=1.
    let bob_net = assert_client_drove(&bob.until("smoke=headless-physics"), 2);
    let alice_net = assert_client_drove(&alice.until("smoke=headless-physics"), 1);
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(bob.wait().success(), "bob did not exit cleanly");

    // A lane's writer finishes within a pump quantum of the client's
    // exit — a short grace lets the last `finish` flush publish so the
    // counters below are the cell's whole story.
    std::thread::sleep(Duration::from_millis(300));
    let up = proxy.stats(LinkDir::Up);
    let down = proxy.stats(LinkDir::Down);
    // The relay holds socket clones until it drops — the host only
    // sees the clients' disconnects (and their roster entries only
    // leave) once the proxy itself is down.
    drop(proxy);
    for _ in 0..2 {
        host.until("event=left");
    }
    quit_and_assert_host_drove(host);
    (up, down, alice_net, bob_net)
}

/// F25-AC03's measured matrix at process level — the recorded run
/// lives in `docs/research/net.md` under "Measured impairment
/// matrix". The cell table is the in-process `net_app` grid's
/// verbatim (clean, latency, jitter, loss, loss-heavy, duplicate,
/// reorder, combined), but every cell is a real `mm2 --host` process
/// plus two real `mm2 --join` clients relayed through a seeded
/// [`ImpairProxy`] — the lobby crosses clean, the driving window pays
/// the recipe in both directions, and each client's mid-session
/// `net=` record proves its predicted seat drove, authority snaps
/// applied, and both peers reconciled as remote copies.
///
/// Assertions are floors, not counts — the process-level traffic
/// volume is update-rate-bound and drifts between runs. The per-frame
/// ordering that makes the floors safe stays where the in-process
/// grid puts it: `impair`'s lane legs (dup copies emit adjacent, a
/// swap emits the held frame behind its successor) and `netdrive`'s
/// push legs (at-or-behind watermark → counted drop). The `clean`
/// cell's `snap<x>` reading is the publish-cadence dedup floor every
/// impaired cell is read against, not wire impairment.
#[test]
fn the_process_level_impairment_matrix_records_each_recipe_cell() {
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
        // Jitter — small fixed hold, wide spread.
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
        // Heavy loss — the "client falls behind" edge.
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
        // Combined — the recipe the single-recipe leg above runs.
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
    let install = tempfile::tempdir().unwrap();
    for cell in &cells {
        let (up, down, alice, bob) = run_matrix_cell(install.path(), cell);

        // The cell's record line — one measured row per recipe; the
        // doc's process-level table is harvested from these.
        eprintln!(
            "proc-matrix cell={} alice={alice:?} bob={bob:?} up={up:?} down={down:?}",
            cell.name,
        );

        // Convergence — every cell's data plane still moved state
        // both ways (the client-record floors ran inside the cell).
        assert!(
            up.frames_in > 0 && down.frames_in > 0,
            "cell {} moved no data-plane frames: {up:?} {down:?}",
            cell.name,
        );
        assert!(
            up.bytes_in > 0 && down.bytes_in > 0,
            "cell {} moved no payload bytes: {up:?} {down:?}",
            cell.name,
        );
        assert_eq!(
            up.overflowed + down.overflowed,
            0,
            "cell {} overflowed a lane: {up:?} {down:?}",
            cell.name,
        );

        // Each armed knob must show in the counter it claims to turn.
        let impair = cell.impair;
        let snaps_staled = alice.snaps_staled + bob.snaps_staled;
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
            // watermark — a counted stale drop on a real client.
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
            // A swap's held frame is older than the successor it
            // lands behind — it can never displace the newer pose.
            assert!(
                snaps_staled > 0,
                "cell {} recorded no stale drop off reordered snaps",
                cell.name,
            );
        }
        // The control cell: a transparent lane impairs nothing, so
        // its `snap<x>` row is publish-cadence dedup only — the floor
        // the impaired cells are read against.
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
    }
}

/// The `props=sites<count>:<digest>,landed<n>,mism<n>` record field —
/// this process's own stamped world (F26-A, protocol v18) and the prop
/// rows a client landed or refused for a world that differs.
fn props_field(line: &str) -> (u64, String, u64, u64) {
    let (sites, rest) = field(line, "props")
        .strip_prefix("sites")
        .and_then(|s| s.split_once(','))
        .unwrap_or_else(|| panic!("no props= sites in {line}"));
    let (count, digest) = sites.split_once(':').expect("count:digest");
    let cell = |prefix: &str| {
        rest.split(',')
            .find_map(|c| c.strip_prefix(prefix))
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no {prefix} cell in {line}"))
    };
    (
        count.parse().unwrap(),
        digest.to_string(),
        cell("landed"),
        cell("mism"),
    )
}

/// F26-A's open evidence step: do two real processes stamp the *same*
/// world on a retail install? A hosted sf session and a joined client,
/// each loading the city through the VFS on its own, must print the
/// same non-empty `SiteTable` — and the client must not have refused a
/// single prop row for a world that differs. Skipped without the
/// operator's install (`MM2_RETAIL=<dir>`), like `net_check`'s retail
/// leg: the original game's files never ride in git, so CI reports it
/// as vacuous and only an operator run with the install is evidence.
///
/// Same machine, same architecture, same binary: this does not show the
/// ordinals agree across platforms.
#[test]
fn two_retail_processes_stamp_the_same_prop_world() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 9000);
    // The retail world, not the dev cruise.
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(["--city".into(), "sf".into()]);
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let client = Proc::spawn(MM2_EXE, &join_args(&retail, addr, "bob", 1000));
    start_when_ready(&mut host, 1);

    let rec = client.until("smoke=headless-physics");
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    let (count, digest, _landed, mismatched) = props_field(&rec);
    assert!(count > 0, "the retail city stamps placements: {rec}");
    assert_eq!(mismatched, 0, "the client refused the host's rows: {rec}");
    assert!(client.wait().success(), "the client did not exit cleanly");

    host.cmd("quit");
    let host_rec = host.until("smoke=headless-physics");
    let (host_count, host_digest, ..) = props_field(&host_rec);
    assert_eq!(
        (host_count, host_digest),
        (count, digest),
        "host and client stamped different worlds:\nhost   {host_rec}\nclient {rec}"
    );
    // The original skips kerbside parked cars in a networked cruise.
    assert_eq!(parked_cars(&rec), 0, "{rec}");
    assert_eq!(parked_cars(&host_rec), 0, "{host_rec}");
}

/// The `parked<n>` cell of the `props=` record field: the kerbside
/// parked cars this process placed (F26-A / WLD-27).
fn parked_cars(line: &str) -> u64 {
    field(line, "props")
        .split(',')
        .find_map(|c| c.strip_prefix("parked"))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no parked cell in {line}"))
}

/// Report 6 follow-up 3's open question: do the seed-rolled kerbside
/// parked cars agree across peers in a networked *race*? They are
/// knockable bangers placed from the session seed, and the original
/// keeps them in races (WLD-27). A hosted retail `checkpoint:0` and a
/// joined client each roll their own, so both must have placed some,
/// the same number, inside one identical `SiteTable` digest, and the
/// client must have refused no prop row for a world that differs.
/// Skipped without `MM2_RETAIL=<dir>`, like the legs around it.
///
/// Same machine, same binary, loopback; cross-platform agreement of the
/// seed rolls is unobserved.
#[test]
fn two_retail_processes_roll_the_same_parked_cars_in_a_race() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 9000);
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(["--event".into(), "checkpoint:0".into()]);
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let mut client_args = join_args(&retail, addr, "bob", 1000);
    client_args.push("--parked".into());
    let client = Proc::spawn(MM2_EXE, &client_args);
    start_when_ready(&mut host, 1);

    let rec = client.until("smoke=headless-physics");
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    let (count, digest, _landed, mismatched) = props_field(&rec);
    assert_eq!(mismatched, 0, "the client refused the host's rows: {rec}");
    assert!(client.wait().success(), "the client did not exit cleanly");

    host.cmd("quit");
    let host_rec = host.until("smoke=headless-physics");
    let (host_count, host_digest, ..) = props_field(&host_rec);
    assert_eq!(
        (host_count, host_digest),
        (count, digest),
        "host and client stamped different worlds:\nhost   {host_rec}\nclient {rec}"
    );
    eprintln!("host   {host_rec}\nclient {rec}");
    let parked = parked_cars(&host_rec);
    assert!(parked > 0, "the race placed no parked cars: {host_rec}");
    assert_eq!(parked_cars(&rec), parked, "{host_rec}\n{rec}");
    assert!(count >= parked, "parked cars are part of the digest: {rec}");
}

/// The `scen=n<actors>,<tick>:<digest>,…` cell that follows `wclk=`: how
/// many clock-driven scenery actors the process sampled and the digest
/// of their poses at each sampled world tick.
fn scenery_field(line: &str) -> (u64, std::collections::BTreeMap<u64, String>) {
    let cells = line
        .split_whitespace()
        .find_map(|w| w.strip_prefix("scen=n"))
        .unwrap_or_else(|| panic!("no scen= cell in {line}"));
    let (actors, samples) = cells.split_once(',').expect("actors,samples");
    let samples = samples
        .split(',')
        .map(|s| {
            let (tick, digest) = s.split_once(':').expect("tick:digest");
            (tick.parse().expect("tick"), digest.to_string())
        })
        .collect();
    (actors.parse().expect("actors"), samples)
}

/// Report 6 follow-up 3's scenery half: where a hosted retail London
/// stands its clock-driven scenery (Thames drawbridge leaves, boats,
/// Underground trains) must be where a joined client stands it *at the
/// same world tick*. Each process samples a digest of those actors'
/// poses every 60 world ticks; the two records must share ticks, and
/// every shared tick must carry one digest. `extra` picks the session:
/// a Cruise (the host's clock frames align the client) or a race (the
/// race row does).
fn two_retail_processes_stand_the_same_scenery(extra: &[&str]) {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 9000);
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(extra.iter().map(|a| a.to_string()));
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let client = Proc::spawn(MM2_EXE, &join_args(&retail, addr, "bob", 1000));
    start_when_ready(&mut host, 1);

    let rec = client.until("smoke=headless-physics");
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert!(client.wait().success(), "the client did not exit cleanly");
    host.cmd("quit");
    let host_rec = host.until("smoke=headless-physics");
    eprintln!("host   {host_rec}\nclient {rec}");

    let (host_actors, host_poses) = scenery_field(&host_rec);
    let (actors, poses) = scenery_field(&rec);
    assert!(actors > 0, "the city fields no clock-driven scenery: {rec}");
    assert_eq!(host_actors, actors, "different actors: {host_rec}\n{rec}");
    let shared: Vec<u64> = poses
        .keys()
        .filter(|t| host_poses.contains_key(t))
        .copied()
        .collect();
    assert!(
        shared.len() >= 4,
        "too few shared world ticks to compare ({shared:?}):\n{host_rec}\n{rec}"
    );
    for tick in shared {
        assert_eq!(
            host_poses[&tick], poses[&tick],
            "the scenery differs at world tick {tick}:\n{host_rec}\n{rec}"
        );
    }
}

/// A Cruise: no race row, so the client's scenery clock aligns to the
/// host's `World` frames. `MM2_RETAIL`-gated.
#[test]
fn two_retail_processes_stand_the_same_scenery_in_a_cruise() {
    two_retail_processes_stand_the_same_scenery(&["--city", "london"]);
}

/// A race: the client's scenery clock aligns to the race row.
/// `MM2_RETAIL`-gated.
#[test]
fn two_retail_processes_stand_the_same_scenery_in_a_race() {
    two_retail_processes_stand_the_same_scenery(&["--event", "checkpoint:0"]);
}

/// The `cars=sent<n>,omit<n>,live<n>,landed<n>,mism<n>` record field —
/// the host's published ambient rows and, on a client, the traffic
/// copies it holds and the rows it landed or refused (F26-A, v19).
fn cars_field(line: &str) -> (u64, u64, u64, u64, u64) {
    let cells = field(line, "cars");
    let cell = |prefix: &str| {
        cells
            .split(',')
            .find_map(|c| c.strip_prefix(prefix))
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no {prefix} cell in {line}"))
    };
    (
        cell("sent"),
        cell("omit"),
        cell("live"),
        cell("landed"),
        cell("mism"),
    )
}

/// The `cable=sent<n>,live<n>` cells that follow `cars=` when cable cars
/// crossed the wire (F28-B.5): the host's published cable rows and a
/// client's cable copies. `None` when the record carries no such cell.
fn cable_field(line: &str) -> Option<(u64, u64)> {
    let cells = line
        .split_whitespace()
        .find_map(|w| w.strip_prefix("cable="))?;
    let cell = |prefix: &str| {
        cells
            .split(',')
            .find_map(|c| c.strip_prefix(prefix))
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no {prefix} cell in {line}"))
    };
    Some((cell("sent"), cell("live")))
}

/// The `wclk=sent<n>,landed<n>,seek<n>,ref<n>` record field — the
/// host's published world-clock frames and, on a client, the frames it
/// folded into its scenery clock, the re-seeks those queued and the
/// frames it refused (F26-A, v20).
fn world_field(line: &str) -> (u64, u64, u64, u64) {
    let cells = field(line, "wclk");
    let cell = |prefix: &str| {
        cells
            .split(',')
            .find_map(|c| c.strip_prefix(prefix))
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no {prefix} cell in {line}"))
    };
    (cell("sent"), cell("landed"), cell("seek"), cell("ref"))
}

/// F26-A.6's real-process evidence: a hosted retail sf Cruise fields
/// ambient traffic, publishes it, and a joined client — which simulates
/// no lane follower of its own — spawns copies of the host's cars and
/// lands rows on them without refusing any for a roster that differs.
/// Skipped without the operator's install (`MM2_RETAIL=<dir>`), like the
/// prop-world leg beside it.
///
/// Same machine, same binary, loopback: it shows the frames cross a real
/// socket between two processes and that both derive one roster from the
/// city; it does not show the copies match the host's cars *visually*
/// (headless) or hold up under impairment.
#[test]
fn two_retail_processes_replicate_the_hosts_traffic() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 9000);
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(["--city".into(), "sf".into()]);
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let client = Proc::spawn(MM2_EXE, &join_args(&retail, addr, "bob", 1000));
    start_when_ready(&mut host, 1);

    let rec = client.until("smoke=headless-physics");
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    let (_, _, _live, landed, mismatched) = cars_field(&rec);
    assert!(landed > 0, "no traffic row landed on the client: {rec}");
    assert_eq!(mismatched, 0, "the client refused the host's rows: {rec}");
    // The Cruise client has no race row: the host's world-clock frame
    // is what its scenery clock aligns to (v20).
    let (_, world_landed, _, world_refused) = world_field(&rec);
    assert!(world_landed > 0, "no world-clock frame landed: {rec}");
    assert_eq!(world_refused, 0, "the client refused the clock: {rec}");
    assert!(client.wait().success(), "the client did not exit cleanly");

    host.cmd("quit");
    let host_rec = host.until("smoke=headless-physics");
    let (sent, ..) = cars_field(&host_rec);
    assert!(sent > 0, "the host published no traffic: {host_rec}");
    let (world_sent, ..) = world_field(&host_rec);
    assert!(
        world_sent > 0,
        "the host published no world clock: {host_rec}"
    );
    // F28-B.5: sf's four cable cars run on the host and ride the same
    // frames; the client holds a copy of the ones near its car and never
    // ran one. F26-A.1: a client is sent only what is within the
    // relevancy radius of its vehicle, so which of the four it holds
    // depends on where it drove — at least one, at most all.
    let (cable_sent, _) = cable_field(&host_rec)
        .unwrap_or_else(|| panic!("the host published no cable car: {host_rec}"));
    assert!(cable_sent >= 4, "{host_rec}");
    let (_, cable_live) =
        cable_field(&rec).unwrap_or_else(|| panic!("the client holds no cable car copy: {rec}"));
    assert!((1..=4).contains(&cable_live), "{rec}");
    // The operator's evidence: both records, as the run printed them.
    eprintln!("host   {host_rec}\nclient {rec}");
}

/// One cell of the `cnr=sent<n>,landed<n>,stale<n>,ref<n>,seats<n>,
/// solo<n>,rob<n>,cop<n>,red<n>,blue<n>,dec<n>[,win=p<id>|s<side>|tie]` record field — the
/// host's published Cops & Robbers frames, a client's landed/stale/
/// refused ones, and (`seats`..`dec`) the match this process sees, its
/// own or its replica, seated per side (F27-B, v21). The match cells
/// are absent once the session is gone: a host `quit` back to its
/// lobby prints only the counters.
fn cnr_cell(line: &str, prefix: &str) -> Option<u64> {
    field(line, "cnr")
        .split(',')
        .find_map(|c| c.strip_prefix(prefix))
        .map(|n| {
            n.parse()
                .unwrap_or_else(|_| panic!("bad {prefix} in {line}"))
        })
}

/// F27-B's real-process evidence: a hosted retail sf Cops & Robbers
/// match (`--cnr cops`) built from the city's authored site pool, and a
/// joined client that lands the host's match frames and sees the match
/// seated on both sides. Skipped without the operator's install
/// (`MM2_RETAIL=<dir>`).
///
/// Same machine, same binary, loopback, headless: it shows the started
/// match crosses a real socket between two processes, that the host
/// stepped it (the periodic frames came from a running clock) and that
/// the client's replica seats the participants on the two sides; it
/// does not show a gold pickup, a delivery or a decided match (nobody
/// drives to the gold), the HUD, or behaviour under impairment.
#[test]
fn two_retail_processes_play_a_started_cops_and_robbers_match() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 9000);
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(
        ["--city", "sf", "--cnr", "cops", "--cnr-limit", "5m"]
            .into_iter()
            .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let client = Proc::spawn(MM2_EXE, &join_args(&retail, addr, "bob", 1000));
    start_when_ready(&mut host, 1);

    let rec = client.until("smoke=headless-physics");
    assert!(client.wait().success(), "the client did not exit cleanly");
    host.cmd("quit");
    let host_rec = host.until("smoke=headless-physics");
    // The operator's evidence: both records, as the run printed them.
    eprintln!("host   {host_rec}\nclient {rec}");

    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    let cell =
        |prefix: &str| cnr_cell(&rec, prefix).unwrap_or_else(|| panic!("no {prefix}: {rec}"));
    assert!(cell("landed") > 1, "only the opening frame landed: {rec}");
    assert_eq!(cell("ref"), 0, "the client refused the host's match: {rec}");
    assert!(cell("seats") >= 2, "the replica seats too few cars: {rec}");
    assert_eq!(cell("rob") + cell("cop"), cell("seats"), "{rec}");
    assert!(
        cell("rob") >= 1 && cell("cop") >= 1,
        "both sides are seated: {rec}"
    );
    assert_eq!(cell("dec"), 0, "nobody drove to the gold: {rec}");

    // More than the opening frame went out: the match clock ran on the
    // host (a repeat is due every 120 match ticks).
    let sent = cnr_cell(&host_rec, "sent").unwrap();
    assert!(sent > 1, "the host's match clock never ran: {host_rec}");
    assert!(
        cell("landed") <= sent,
        "the client landed frames the host never sent:\nhost   {host_rec}\nclient {rec}"
    );
}

/// F27's decided-match evidence over real processes: a hosted retail sf
/// free-for-all to 100 pts whose host seat is driven by the `--bot`
/// evidence driver (a road-graph route to the gold, then to the
/// hideout), beside a joined client that sits parked. The host's own
/// match rules take the pickup and the delivery from measured car
/// positions, decide the match, and the decided frame crosses the
/// socket: the client's replica reads `dec1` with the host's winner
/// seated. Skipped without the operator's install (`MM2_RETAIL=<dir>`).
///
/// What it is not: a contested steal, a client that carries or delivers
/// (the client is parked, so F27-AC01's multi-client cycle and AC02's
/// simultaneous requests stay open), packet loss, or a rendered match.
/// The seed is one the `mm2-inspect cnr` reach audit lists (every site
/// of its opening draw on a routable lane — most retail sf sites are
/// not, and the bot follows roads) that a local `--bot` run measured to
/// finish. The bot never re-anchors (no teleport onto the objective):
/// the delivery is driven. The wall-clock cost is about a minute.
#[test]
fn two_retail_processes_decide_a_cops_and_robbers_match() {
    const SEED: u64 = 1291;
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 40_000);
    host_args.retain(|a| a != "--dev-world");
    let seed = host_args.iter().position(|a| a == "--seed").unwrap();
    host_args[seed + 1] = SEED.to_string();
    host_args.extend(
        [
            "--city",
            "sf",
            "--cnr",
            "ffa",
            "--cnr-limit",
            "100pts",
            "--bot",
        ]
        .into_iter()
        .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let mut client_args = join_args(&retail, addr, "bob", 25_000);
    client_args.push("--parked".into());
    let client = Proc::spawn(MM2_EXE, &client_args);
    start_when_ready(&mut host, 1);

    // The host's own log names the delivery and the verdict it decided.
    let bound = Duration::from_secs(300);
    let delivered = host.until_within("Delivered {", bound);
    let ended = host.until_within("Ended(", bound);
    eprintln!("host   {delivered}\nhost   {ended}");
    assert!(ended.contains("reason: PointLimit"), "{ended}");
    assert!(delivered.contains("player: PlayerId(0)"), "{delivered}");
    let rec = client.until_within("smoke=headless-physics", bound);
    assert!(client.wait().success(), "the client did not exit cleanly");
    host.cmd("quit");
    let host_rec = host.until_within("smoke=headless-physics", bound);
    eprintln!("host   {host_rec}\nclient {rec}");

    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    let cell =
        |prefix: &str| cnr_cell(&rec, prefix).unwrap_or_else(|| panic!("no {prefix}: {rec}"));
    assert_eq!(cell("ref"), 0, "the client refused the host's match: {rec}");
    assert_eq!(cell("seats"), 2, "{rec}");
    assert_eq!(cell("solo"), 2, "a free-for-all seats everyone solo: {rec}");
    assert_eq!(
        cell("dec"),
        1,
        "the client never saw the decided match: {rec}"
    );
    // The host's verdict (player 0, its own seat) is the winner the
    // client's replica names, and the client's session is on the
    // match-over screen.
    assert!(
        field(&rec, "cnr").split(',').any(|c| c == "win=p0"),
        "the client's winner differs from the host's: {rec}"
    );
    assert_eq!(field(&rec, "phase"), "results", "{rec}");
    // The client's announcer voiced what the replica's changes showed —
    // the host's pickup and its delivery (the host's own record is read
    // after `quit` tore its match down, so it carries no `say`).
    assert!(
        cell("say") >= 2,
        "the client did not announce the pickup and the stash: {rec}"
    );
    // The parked client never touched the gold: the one delivery is
    // the host seat's.
    assert!(cell("landed") > 0, "{rec}");
}

/// F27-AC05's process-level leg: the decided Cops & Robbers match of
/// `two_retail_processes_decide_a_cops_and_robbers_match` (host `--bot`,
/// retail sf, seed 1291, `100pts`) with the parked client's whole link
/// riding an armed [`ImpairProxy`] — 30 % frame loss, duplicates,
/// reorders and a 40 ms ± 30 ms hold, both directions, from just after
/// `Start`. The match is host-authoritative and its state travels as
/// repeating whole-view frames (on change, on a cadence, and repeated
/// once decided), so the client must still read the decided match with
/// the host's winner and be on the match-over screen, and the proxy's
/// counters must show the recipe really bit the data plane. Loopback
/// only; the frame-level recipe is the harness's, over the real TCP
/// transport. Skipped without `MM2_RETAIL=<dir>`.
///
/// What it is not: a late joiner, a client that drives, a contested
/// pickup, or a rendered match.
#[test]
fn a_decided_cops_and_robbers_match_reaches_a_client_on_an_impaired_link() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 40_000);
    host_args.retain(|a| a != "--dev-world");
    let seed = host_args.iter().position(|a| a == "--seed").unwrap();
    host_args[seed + 1] = "1291".into();
    host_args.extend(
        [
            "--city",
            "sf",
            "--cnr",
            "ffa",
            "--cnr-limit",
            "100pts",
            "--bot",
        ]
        .into_iter()
        .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let proxy = ImpairProxy::loopback_seeded(addr, 0xF27A5).unwrap();
    let recipe = Impair {
        delay: Duration::from_millis(40),
        jitter: Duration::from_millis(30),
        loss: 0.30,
        duplicate: 0.10,
        reorder: 0.10,
    };
    let mut client_args = join_args(&retail, proxy.addr(), "bob", 25_000);
    client_args.push("--parked".into());
    let client = Proc::spawn(MM2_EXE, &client_args);
    start_when_ready(&mut host, 1);
    // `Start` is a one-shot verb with no retransmit: let the still-clean
    // link deliver it, then arm the recipe on the data plane.
    std::thread::sleep(Duration::from_millis(400));
    proxy.set(LinkDir::Up, recipe);
    proxy.set(LinkDir::Down, recipe);

    let bound = Duration::from_secs(300);
    let delivered = host.until_within("Delivered {", bound);
    let ended = host.until_within("Ended(", bound);
    eprintln!("host   {delivered}\nhost   {ended}");
    assert!(ended.contains("reason: PointLimit"), "{ended}");
    assert!(delivered.contains("player: PlayerId(0)"), "{delivered}");
    let rec = client.until_within("smoke=headless-physics", bound);
    assert!(client.wait().success(), "the client did not exit cleanly");
    let up = proxy.stats(LinkDir::Up);
    let down = proxy.stats(LinkDir::Down);
    eprintln!("client {rec}\nup {up:?}\ndown {down:?}");
    host.cmd("quit");
    host.until_within("smoke=headless-physics", bound);

    // The recipe really bit the match's own stream.
    assert!(down.frames_in > 0 && down.dropped > 0, "{down:?}");
    assert!(down.delayed > 0, "{down:?}");
    assert!(up.frames_in > 0, "{up:?}");

    let cell =
        |prefix: &str| cnr_cell(&rec, prefix).unwrap_or_else(|| panic!("no {prefix}: {rec}"));
    assert_eq!(cell("ref"), 0, "the client refused the host's match: {rec}");
    assert_eq!(cell("seats"), 2, "{rec}");
    assert_eq!(cell("dec"), 1, "the decided match never reached it: {rec}");
    assert!(
        field(&rec, "cnr").split(',').any(|c| c == "win=p0"),
        "the client's winner differs from the host's: {rec}"
    );
    assert_eq!(field(&rec, "phase"), "results", "{rec}");
    assert!(cell("landed") > 0, "{rec}");
}

/// F27-AC05's late-join leg over real processes: the decided-match run
/// (`two_retail_processes_decide_a_cops_and_robbers_match`: host `--bot`,
/// retail sf, seed 1291, `100pts`) with the only client connecting
/// *after* the host's seat has taken the gold. The host's `Start` was
/// sent to nobody (the empty-roster gate passes and the open policy
/// admits joiners), so the late client is handed the running session,
/// seated in the match mid-carry, and must then read the same ending
/// the host decides — the delivery, the point-limit verdict for player 0
/// — and sit on the match-over screen. Loopback only; no impairment (the
/// lossy variant is `a_late_joiner_reads_the_cops_and_robbers_verdict_on_an_impaired_link`).
/// Skipped without `MM2_RETAIL=<dir>`.
///
/// What it is not: a joiner that drives, a contested pickup, or a
/// rendered match.
#[test]
fn a_client_that_joins_a_cops_and_robbers_match_mid_carry_reads_the_verdict() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 40_000);
    host_args.retain(|a| a != "--dev-world");
    let seed = host_args.iter().position(|a| a == "--seed").unwrap();
    host_args[seed + 1] = "1291".into();
    host_args.extend(
        [
            "--city",
            "sf",
            "--cnr",
            "ffa",
            "--cnr-limit",
            "100pts",
            "--bot",
        ]
        .into_iter()
        .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    // The match starts with nobody else in it.
    start_when_ready(&mut host, 0);

    let bound = Duration::from_secs(300);
    let picked = host.until_within("Picked {", bound);
    assert!(picked.contains("player: PlayerId(0)"), "{picked}");
    let mut client_args = join_args(&retail, addr, "late", 25_000);
    client_args.push("--parked".into());
    let client = Proc::spawn(MM2_EXE, &client_args);

    let delivered = host.until_within("Delivered {", bound);
    let ended = host.until_within("Ended(", bound);
    eprintln!("host   {picked}\nhost   {delivered}\nhost   {ended}");
    assert!(ended.contains("reason: PointLimit"), "{ended}");
    assert!(delivered.contains("player: PlayerId(0)"), "{delivered}");
    let rec = client.until_within("smoke=headless-physics", bound);
    eprintln!("client {rec}");
    assert!(
        client.wait().success(),
        "the late joiner did not exit cleanly: {rec}"
    );
    host.cmd("quit");
    host.until_within("smoke=headless-physics", bound);

    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    let cell =
        |prefix: &str| cnr_cell(&rec, prefix).unwrap_or_else(|| panic!("no {prefix}: {rec}"));
    assert_eq!(cell("ref"), 0, "the late joiner refused the match: {rec}");
    assert_eq!(cell("seats"), 2, "{rec}");
    assert_eq!(cell("dec"), 1, "the verdict never reached it: {rec}");
    assert!(
        field(&rec, "cnr").split(',').any(|c| c == "win=p0"),
        "the late joiner's winner differs from the host's: {rec}"
    );
    assert_eq!(field(&rec, "phase"), "results", "{rec}");
    assert!(cell("landed") > 0, "{rec}");
}

/// F27-AC05's late-join leg under loss: the mid-carry late join (above)
/// with the joiner's link through a seeded [`ImpairProxy`]. The joiner
/// connects clean, is handed the running session and has its pick
/// echoed (`event=vehicle` on the host), and only then does the recipe
/// arm — 30 % frame loss, duplicates, reorders and a 40 ms ± 30 ms hold,
/// both directions — so `Start` and the pick, which are one-shot verbs
/// with no retransmit, are not what is tested. The match state rides the
/// repeating whole-view frames; the joiner must still read the host's
/// delivery and point-limit verdict. Loopback only. Skipped without
/// `MM2_RETAIL=<dir>`.
///
/// What it is not: loss on the join handshake itself, a joiner that
/// drives, a contested pickup, or a rendered match.
#[test]
fn a_late_joiner_reads_the_cops_and_robbers_verdict_on_an_impaired_link() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 40_000);
    host_args.retain(|a| a != "--dev-world");
    let seed = host_args.iter().position(|a| a == "--seed").unwrap();
    host_args[seed + 1] = "1291".into();
    host_args.extend(
        [
            "--city",
            "sf",
            "--cnr",
            "ffa",
            "--cnr-limit",
            "100pts",
            "--bot",
        ]
        .into_iter()
        .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    start_when_ready(&mut host, 0);

    let bound = Duration::from_secs(300);
    let picked = host.until_within("Picked {", bound);
    assert!(picked.contains("player: PlayerId(0)"), "{picked}");
    let proxy = ImpairProxy::loopback_seeded(addr, 0xF27A6).unwrap();
    let recipe = Impair {
        delay: Duration::from_millis(40),
        jitter: Duration::from_millis(30),
        loss: 0.30,
        duplicate: 0.10,
        reorder: 0.10,
    };
    let mut client_args = join_args(&retail, proxy.addr(), "late", 25_000);
    client_args.push("--parked".into());
    let client = Proc::spawn(MM2_EXE, &client_args);
    host.until_within("event=vehicle id=1", bound);
    std::thread::sleep(Duration::from_millis(400));
    proxy.set(LinkDir::Up, recipe);
    proxy.set(LinkDir::Down, recipe);

    let delivered = host.until_within("Delivered {", bound);
    let ended = host.until_within("Ended(", bound);
    eprintln!("host   {picked}\nhost   {delivered}\nhost   {ended}");
    assert!(ended.contains("reason: PointLimit"), "{ended}");
    assert!(delivered.contains("player: PlayerId(0)"), "{delivered}");
    let rec = client.until_within("smoke=headless-physics", bound);
    assert!(
        client.wait().success(),
        "the late joiner did not exit cleanly: {rec}"
    );
    let up = proxy.stats(LinkDir::Up);
    let down = proxy.stats(LinkDir::Down);
    eprintln!("client {rec}\nup {up:?}\ndown {down:?}");
    host.cmd("quit");
    host.until_within("smoke=headless-physics", bound);

    // The recipe really bit the match's own stream.
    assert!(down.frames_in > 0 && down.dropped > 0, "{down:?}");
    assert!(down.delayed > 0, "{down:?}");
    assert!(up.frames_in > 0, "{up:?}");

    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    let cell =
        |prefix: &str| cnr_cell(&rec, prefix).unwrap_or_else(|| panic!("no {prefix}: {rec}"));
    assert_eq!(cell("ref"), 0, "the late joiner refused the match: {rec}");
    assert_eq!(cell("seats"), 2, "{rec}");
    assert_eq!(cell("dec"), 1, "the verdict never reached it: {rec}");
    assert!(
        field(&rec, "cnr").split(',').any(|c| c == "win=p0"),
        "the late joiner's winner differs from the host's: {rec}"
    );
    assert_eq!(field(&rec, "phase"), "results", "{rec}");
    assert!(cell("landed") > 0, "{rec}");
}

/// F27-AC01's client leg over real processes: the *joined client's*
/// `--bot` reads the host's replica, drives its own predicted car to the
/// gold, and the host — which parks its own seat — measures the contact
/// from the client's car as the wire places it and attributes the pickup
/// to wire seat 1. The client's replica then reads the gold carried and
/// its announcer voices the call. Retail sf, seed 1291 (the gold lies
/// a few metres from the spawn). Skipped without `MM2_RETAIL=<dir>`.
///
/// What it is not: a client that *delivers* (with this seat the bot
/// stalls on the sf hillside beyond the first leg — see the PLAN row;
/// the same seed's host-driven delivery is
/// `two_retail_processes_decide_a_cops_and_robbers_match`), a contested
/// steal, packet loss, or a rendered match.
#[test]
fn a_joined_clients_bot_picks_up_the_gold() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 40_000);
    host_args.retain(|a| a != "--dev-world");
    let at = host_args.iter().position(|a| a == "--seed").unwrap();
    host_args[at + 1] = "1291".into();
    host_args.extend(
        [
            "--city",
            "sf",
            "--cnr",
            "ffa",
            "--cnr-limit",
            "100pts",
            "--parked",
        ]
        .into_iter()
        .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let mut client_args = join_args(&retail, addr, "bob", 12_000);
    client_args.push("--bot".into());
    let client = Proc::spawn(MM2_EXE, &client_args);
    start_when_ready(&mut host, 1);

    // The host's own log attributes the pickup to the client's wire seat.
    let bound = Duration::from_secs(120);
    let picked = host.until_within("Picked {", bound);
    eprintln!("host   {picked}");
    assert!(picked.contains("player: PlayerId(1)"), "{picked}");
    assert!(picked.contains("recovered: false"), "{picked}");
    let rec = client.until_within("smoke=headless-physics", bound);
    assert!(client.wait().success(), "the client did not exit cleanly");
    host.cmd("quit");
    let host_rec = host.until_within("smoke=headless-physics", bound);
    eprintln!("host   {host_rec}\nclient {rec}");

    let cell =
        |prefix: &str| cnr_cell(&rec, prefix).unwrap_or_else(|| panic!("no {prefix}: {rec}"));
    assert_eq!(cell("ref"), 0, "the client refused the host's match: {rec}");
    assert_eq!(cell("seats"), 2, "{rec}");
    assert_eq!(cell("dec"), 0, "nobody delivered in this run: {rec}");
    // The announcer voiced what the replica showed: the client's own
    // pickup (a call exists only if the replica read the gold taken).
    assert!(cell("say") >= 1, "the client never heard the pickup: {rec}");
}

/// The cell after a record field's `key` inside a `,`-separated value
/// whose cells read `<n><suffix>` (`imp=1i/1r/1d` → `imp_cells("imp")`
/// gives `[1, 1, 1]`); empty when the record has no such field.
fn imp_cells(line: &str, key: &str) -> Vec<u64> {
    line.split_whitespace()
        .find_map(|t| t.strip_prefix(&format!("{key}=")))
        .map(|v| v.split('/').map(leading_u64).collect())
        .unwrap_or_default()
}

/// The `tick=<n>` field of a tracing line, with the formatter's colour
/// escapes stripped (they sit between a field's name and its `=`).
fn log_tick(line: &str) -> u64 {
    let mut plain = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    let at = plain
        .find("tick=")
        .unwrap_or_else(|| panic!("no tick= in {plain}"));
    leading_u64(&plain[at + "tick=".len()..])
}

/// What a breakdown run leaves to assert on: the client's record, the
/// host's two log lines naming the episode it ran on the remote seat
/// (its post-`quit` record carries no damage counters, so the C&R legs
/// read the log the same way), the host's record, and — through a relay
/// — the proxy's final up/down counters.
struct BreakdownRun {
    client: String,
    destroyed: String,
    repaired: String,
    host: String,
    link: Option<(LinkStats, LinkStats)>,
}

/// One breakdown run: the host runs retail sf `checkpoint:0` with
/// `--wreck-at` aimed at the client's wire seat (1), so the destruction
/// takes the production arm for a remote participant on the authority;
/// the parked client runs until its own engine has been repaired
/// (`--until-repaired`, bounded by a wall-clock deadline). With
/// `impair` the client reaches the host through a seeded
/// [`ImpairProxy`] armed on both directions once `Start` crossed clean.
fn run_breakdown(retail: &std::path::Path, event: &str, impair: Option<Impair>) -> BreakdownRun {
    let mut host_args = host_args(retail, 40_000);
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(
        ["--event", event, "--wreck-at", "900", "--wreck-seat", "1"]
            .into_iter()
            .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let proxy = impair.map(|_| ImpairProxy::loopback_seeded(addr, 0xB4EA).unwrap());
    let join_addr = proxy.as_ref().map_or(addr, ImpairProxy::addr);
    let mut client_args = join_args(retail, join_addr, "alice", 25_000);
    client_args
        .extend(["--parked", "--until-repaired", "1", "--deadline", "150"].map(String::from));
    let client = Proc::spawn(MM2_EXE, &client_args);
    start_when_ready(&mut host, 1);
    if let (Some(proxy), Some(impair)) = (&proxy, impair) {
        // `Start` rides Down: let the clean lane deliver it first.
        std::thread::sleep(Duration::from_millis(400));
        proxy.set(LinkDir::Up, impair);
        proxy.set(LinkDir::Down, impair);
    }

    let bound = Duration::from_secs(150 + 60);
    let rec = client.until_within("smoke=headless-physics", bound);
    assert!(client.wait().success(), "the client did not exit cleanly");
    let destroyed = host.until_within("remote vehicle destroyed", bound);
    let repaired = host.until_within("repaired after its breakdown", bound);
    host.cmd("quit");
    let host_rec = host.until_within("smoke=headless-physics", bound);
    eprintln!("host   {destroyed}\nhost   {repaired}\nhost   {host_rec}\nclient {rec}");
    let link = proxy
        .as_ref()
        .map(|p| (p.stats(LinkDir::Up), p.stats(LinkDir::Down)));
    BreakdownRun {
        client: rec,
        destroyed,
        repaired,
        host: host_rec,
        link,
    }
}

/// What a remote driver's breakdown must show whatever the link: the
/// client ended on its condition, not the frame ceiling, its own engine
/// went dead exactly once and came back, and the authority paid the
/// remote seat's breakdown through the production arm — the wreck
/// landed at the flag's tick and the repair came `BREAKDOWN_SECONDS`
/// at the shared `RACE_TICK_HZ`, in place; an instant reset would have logged
/// no such episode.
fn assert_breakdown(run: &BreakdownRun) {
    let (rec, host_rec) = (&run.client, &run.host);
    assert_eq!(field(rec, "status"), "pass", "{rec}");
    assert_eq!(field(rec, "stop"), "repaired", "{rec}");
    let imp = imp_cells(rec, "imp");
    assert_eq!(
        imp.len(),
        3,
        "no dead-engine episode in the client's record: {rec}"
    );
    assert_eq!(imp[2], 1, "exactly one dead episode on the own seat: {rec}");
    assert!(imp[1] >= 1, "the authority's repair lifted it: {rec}");
    assert!(
        net_field(rec).damage_synced > 0,
        "the damage byte rode the snap stream: {rec}"
    );

    assert_eq!(field(host_rec, "status"), "pass", "{host_rec}");
    let (down, up) = (log_tick(&run.destroyed), log_tick(&run.repaired));
    assert!(
        down >= 900,
        "the wreck fired before its tick: {}",
        run.destroyed
    );
    let expected_ticks =
        (mm2_game::BREAKDOWN_SECONDS * mm2_game::RACE_TICK_HZ as f32).round() as u64;
    // The wreck's own update can consume the first timer step. Permit one
    // tick of boundary rounding while retaining the five-second contract.
    assert!(
        (up - down).abs_diff(expected_ticks) <= 1,
        "the dead interval was {} ticks, not five seconds: {} / {}",
        up - down,
        run.destroyed,
        run.repaired
    );
}

/// Report 6 follow-up 1's two-process leg: a remote human's wreck in a
/// Checkpoint race costs them the same five dead seconds the host's own
/// driver pays, and the client's own seat shows it — a *dead* engine
/// episode in between, derived from the damage byte at the destruction
/// bound. The host's own record must show the wreck resolved as a
/// breakdown (one repair, no instant reset).
///
/// Skipped without the operator's install (`MM2_RETAIL=<dir>`). What it
/// is not: a driven wreck (the destruction is the `--wreck-at` knob's),
/// a rendered smoke plume, or a second client.
#[test]
fn a_remote_drivers_breakdown_crosses_two_processes() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    assert_breakdown(&run_breakdown(&retail, "checkpoint:0", None));
}

/// The Blitz leg of the same breakdown (F26-AC06): a networked Blitz
/// session pays a remote human's wreck the same five dead seconds,
/// through the same production arm (DMG-2/RACE-5 name the two modes
/// together). Blitz fields no opponents (BLZ-2), so the roster is the
/// two humans alone.
///
/// Skipped without `MM2_RETAIL`. Loopback scope; the destruction is the
/// knob's and nothing is rendered.
#[test]
fn a_remote_drivers_breakdown_crosses_two_processes_in_a_blitz() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    assert_breakdown(&run_breakdown(&retail, "blitz:0", None));
}

/// The same breakdown on a bad link (F25-B follow-up 1 × AC03): the
/// client reaches the host through the matrix's lossy recipe (30 %
/// loss, duplication, reordering, latency, both directions) from just
/// after `Start`. The destruction is the authority's and the damage
/// byte rides every snap, so what must hold is the clean leg's
/// outcome: the authority still pays exactly five dead seconds, and
/// the client — whose snaps arrive late, twice, out of order or not at
/// all — still sees one dead episode and its repair, never a second
/// one from a stale pre-repair frame.
///
/// Skipped without `MM2_RETAIL`. Loopback scope; the destruction is the
/// knob's and nothing is rendered.
#[test]
fn a_remote_drivers_breakdown_survives_an_impaired_link() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let recipe = Impair {
        delay: Duration::from_millis(40),
        jitter: Duration::from_millis(30),
        loss: 0.30,
        duplicate: 0.10,
        reorder: 0.10,
    };
    let run = run_breakdown(&retail, "checkpoint:0", Some(recipe));
    // The recipe really bit while the breakdown played out.
    let (up, down) = run
        .link
        .expect("the impaired run reports its proxy counters");
    for (dir, stats) in [("up", up), ("down", down)] {
        assert!(stats.frames_in > 0, "{dir} carried nothing: {stats:?}");
        assert!(
            stats.dropped + stats.duplicated + stats.reordered > 0,
            "{dir} saw no impairment: {stats:?}"
        );
    }
    assert_breakdown(&run);
}

/// One lobby of three processes on the dev world where the host and
/// bob sit parked (the victims) and alice either rams the nearest car
/// (`alice_rams`) or sits parked too (the control). Returns the host's
/// post-`quit` `net=` field and the two clients' mid-session records.
fn run_collision_trio(install: &std::path::Path, alice_rams: bool) -> (NetField, String, String) {
    let mut host_flags = host_args(install, HOST_FRAME_CEILING);
    host_flags.push("--parked".into());
    let mut host = Proc::spawn(MM2_EXE, &host_flags);
    let addr = listening_addr(&host);

    // The ram is a pursuit through the wire's input latency, so its
    // first contact lands anywhere from host tick ~700 to ~1850
    // depending on load, and a frame budget is not a clock: a loaded
    // host advances fewer ticks per client frame. The driven run
    // therefore waits on its conditions, not on frames. Bob (the
    // uninvolved client) runs until he has applied the replicated
    // impact; alice runs until bob has left and her own predicted sim has
    // registered the contact, so she is still connected
    // while the impact row rides a snap to him. Frames are only a
    // ceiling, and a wall-clock deadline bounds a condition that never
    // comes (the exact-count assertions then fail on the missing
    // impact, not on a hang). The control run has no condition to wait
    // for — nothing may collide — so it runs a fixed span.
    let deadline = Duration::from_secs(120);
    let with = |mut flags: Vec<String>, extra: &[&str]| {
        flags.extend(extra.iter().map(|f| f.to_string()));
        flags
    };
    let (mut alice_flags, mut bob_flags) = if alice_rams {
        (
            with(
                join_args(install, addr, "alice", 100_000),
                &[
                    "--until-peer-left",
                    "--with-impacts",
                    "1",
                    "--deadline",
                    "150",
                ],
            ),
            with(
                join_args(install, addr, "bob", 100_000),
                &["--until-impacts", "1", "--deadline", "120"],
            ),
        )
    } else {
        (
            join_args(install, addr, "alice", 2800),
            join_args(install, addr, "bob", 2500),
        )
    };
    alice_flags.push(if alice_rams { "--ram" } else { "--parked" }.into());
    bob_flags.push("--parked".into());
    let alice = Proc::spawn(MM2_EXE, &alice_flags);
    let bob = Proc::spawn(MM2_EXE, &bob_flags);
    start_when_ready(&mut host, 2);

    // The clients are quiet for their whole run, which outlasts the
    // per-line wait on a loaded machine.
    let bound = deadline + Duration::from_secs(60);
    let bob_rec = bob.until_within("smoke=headless-physics", bound);
    let alice_rec = alice.until_within("smoke=headless-physics", bound);
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(bob.wait().success(), "bob did not exit cleanly");
    for _ in 0..2 {
        host.until("event=left");
    }
    let host_net = quit_and_assert_host_drove(host);
    (host_net, alice_rec, bob_rec)
}

/// F25-C's first collision leg at process level: three real `mm2`
/// processes where one client *drives into* a parked neighbour, not
/// just alongside it. Alice (`--ram`) streams her pursuit inputs up,
/// the host's authority simulates the contact against the parked host
/// and bob seats, and the resulting impact stream and displaced poses
/// come back down. The control run parks alice too: the same grid,
/// the same frame budgets, no driven contact — so every difference
/// below is the collision's, not the spawn landing's.
#[test]
fn a_driven_collision_replicates_across_three_processes() {
    let install = tempfile::tempdir().unwrap();
    let (control_host, control_alice, control_bob) = run_collision_trio(install.path(), false);
    let (host, alice, bob) = run_collision_trio(install.path(), true);
    eprintln!(
        "collision control host={control_host:?}\n alice={control_alice}\n bob={control_bob}"
    );
    eprintln!("collision ram host={host:?}\n alice={alice}\n bob={bob}");

    for rec in [&control_alice, &control_bob, &alice, &bob] {
        assert_eq!(field(rec, "status"), "pass", "{rec}");
        assert_eq!(field(rec, "phase"), "playing", "{rec}");
        assert_eq!(field(rec, "finite"), "true", "{rec}");
    }
    let local_impacts = |rec: &str| -> u64 { field(rec, "impacts").parse().unwrap() };
    let peak = |rec: &str| -> f64 {
        field(rec, "peak")
            .strip_suffix("m/s")
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no numeric peak= in {rec}"))
    };

    // The control: nobody drives, so nothing collides anywhere.
    assert_eq!(control_host.impacts_sent, 0, "{control_host:?}");
    assert_eq!(local_impacts(&control_alice), 0, "{control_alice}");
    assert_eq!(local_impacts(&control_bob), 0, "{control_bob}");

    // The driven run: alice's pursuit really drove (her own peak), the
    // contact happened in her predicted sim and on the authority, and
    // the authority's impact stream reached bob — a client that never
    // touched its controls and learned of the collision from the wire
    // alone. Alice's own rows are not asserted: a client replays the
    // authority's rows only for seats it does not predict itself, so
    // her applied count is legitimately 0 or more by timing.
    assert!(peak(&alice) > 5.0, "alice never got up to speed: {alice}");
    assert!(local_impacts(&alice) > 0, "{alice}");
    assert!(
        host.impacts_sent > 0,
        "the authority published no impact for the collision: {host:?}"
    );
    let bob_net = net_field(&bob);
    assert!(
        bob_net.impacts_applied > 0,
        "the replicated impact never reached the uninvolved client: {bob_net:?} / {bob}"
    );
    // Both clients ended on their conditions, not the wall-clock
    // deadline: bob on the impact, alice on seeing him leave.
    assert_eq!(field(&bob, "stop"), "impacts", "{bob}");
    assert_eq!(field(&alice, "stop"), "peer-left", "{alice}");
}

/// Every `seats=<id>:<x>,<z>/…` cell of a record, by wire seat.
fn seat_poses(rec: &str) -> Vec<(u16, f64, f64)> {
    field(rec, "seats")
        .split('/')
        .map(|cell| {
            let (id, xz) = cell.split_once(':').expect("id:x,z");
            let (x, z) = xz.split_once(',').expect("x,z");
            (id.parse().unwrap(), x.parse().unwrap(), z.parse().unwrap())
        })
        .collect()
}

/// One lobby of three processes on the dev world where the *host*
/// drives (`--ram`, or `--parked` for the control) at the neighbouring
/// grid seat, and two parked clients join in a fixed order — alice
/// first, so she holds the seat beside the host. The host is the one
/// car with no prediction and no input latency, so its pursuit is the
/// run's only variable. Returns the two clients' mid-session records
/// (bob prints while alice is still connected, so he holds copies of
/// everyone). With `impair`, both clients reach the host through a
/// seeded [`ImpairProxy`] whose recipe is armed on both directions once
/// the lobby has crossed clean (as in the matrix cells), so the shove
/// itself is paid for on a bad link; the proxy's final up/down counters
/// come back too, so the caller can assert the recipe really bit.
fn run_shove_trio(
    install: &std::path::Path,
    host_rams: bool,
    alice_frames: u32,
    bob_frames: u32,
    impair: Option<Impair>,
) -> (String, String, Option<(LinkStats, LinkStats)>) {
    // Each lifecycle step is waited on for an observed line, bounded and
    // named in the failure (a loaded runner is slow, not wrong).
    const STEP: Duration = Duration::from_secs(60);
    let mut host_flags = host_args(install, HOST_FRAME_CEILING);
    host_flags.push(if host_rams { "--ram" } else { "--parked" }.into());
    let mut host = Proc::spawn(MM2_EXE, &host_flags);
    let addr = listening_addr(&host);
    let proxy = impair.map(|_| ImpairProxy::loopback_seeded(addr, 0xAC02).unwrap());
    let join_addr = proxy.as_ref().map_or(addr, ImpairProxy::addr);
    // Bob's frames set when the record is taken; alice's are only a
    // ceiling well past them, since the 90 s deadline is what bounds her.
    let mut alice_flags = join_args(
        install,
        join_addr,
        "alice",
        alice_frames.max(bob_frames * 4),
    );
    alice_flags.push("--parked".into());
    // Alice's record must be taken while bob is still a seat she holds
    // and after he has printed: end her run when bob leaves (her frame
    // count is only the ceiling), not on a frame count that a slower
    // bob can lose the race against on a loaded runner. A client receives
    // the authority's impact without necessarily emitting a local contact;
    // the replicated shove is asserted below, not a local-impact stop gate.
    alice_flags.extend(["--until-peer-left", "--deadline", "90"].map(String::from));
    let alice = Proc::spawn(MM2_EXE, &alice_flags);
    host.until_within("ready=true", STEP);
    let mut bob_flags = join_args(install, join_addr, "bob", bob_frames);
    bob_flags.push("--parked".into());
    let bob = Proc::spawn(MM2_EXE, &bob_flags);
    host.until_within("ready=true", STEP);
    host.cmd("start");
    host.until_within("event=started generation=1", STEP);
    if let (Some(proxy), Some(impair)) = (&proxy, impair) {
        // `Start` rides Down: let the clean lane deliver it first.
        std::thread::sleep(Duration::from_millis(400));
        proxy.set(LinkDir::Up, impair);
        proxy.set(LinkDir::Down, impair);
    }
    // The clients print nothing for ~2.5k frames, longer than the
    // per-line wait under a loaded full-suite run: bound the whole wait.
    let bound = Duration::from_secs(90);
    let bob_rec = bob.until_within("smoke=headless-physics", bound);
    let alice_rec = alice.until_within("smoke=headless-physics", bound);
    alice.wait_success("alice", &alice_rec);
    bob.wait_success("bob", &bob_rec);
    assert_eq!(field(&alice_rec, "stop"), "peer-left", "{alice_rec}");
    if host_rams {
        assert!(net_field(&alice_rec).impacts_applied > 0, "{alice_rec}");
        assert!(net_field(&bob_rec).impacts_applied > 0, "{bob_rec}");
    }
    let link = proxy
        .as_ref()
        .map(|p| (p.stats(LinkDir::Up), p.stats(LinkDir::Down)));
    // The relay holds socket clones until it drops: the host only sees
    // the clients leave once the proxy is down.
    drop(proxy);
    for _ in 0..2 {
        host.until_within("event=left", STEP);
    }
    quit_and_assert_host_drove(host);
    (alice_rec, bob_rec, link)
}

/// F25-AC02's pose leg: a *shoved* car converges to one place on every
/// process. The host rams the next grid seat (alice, who joined first)
/// head-on while both clients sit parked; once the field has come to
/// rest, each client prints where it holds every wire seat — its own
/// car (predicted, reconciled to the authority) and the copies — and a
/// seat both of them hold must agree. The control run parks the host
/// too, giving the nominal grid the shove is measured against.
///
/// The authority's own record is not compared: it prints after the
/// clients left, with the remote seats gone. The host's car is seen
/// through both clients' copies of seat 0 instead.
#[test]
fn a_shoved_seat_converges_across_three_processes() {
    let install = tempfile::tempdir().unwrap();
    let (_, control_bob, _) = run_shove_trio(install.path(), false, 1100, 1000, None);
    let (alice, bob, _) = run_shove_trio(install.path(), true, 2800, 2500, None);
    eprintln!("shove control bob={control_bob}\nshove ram alice={alice}\n bob={bob}");
    assert_shove_converged(&control_bob, &alice, &bob);
}

/// The same shove paid for on a bad link: both clients reach the host
/// through the matrix's `combined` recipe (latency, jitter, loss,
/// duplication, reordering) armed on both directions for the driving
/// window. The control run is clean — it only supplies the nominal
/// grid. Convergence is judged at rest exactly as on the clean link, so
/// what the recipe may cost is the *route* to the rest pose, never the
/// pose itself.
#[test]
fn a_shoved_seat_converges_across_three_processes_on_an_impaired_link() {
    let install = tempfile::tempdir().unwrap();
    let recipe = Impair {
        delay: Duration::from_millis(40),
        jitter: Duration::from_millis(30),
        loss: 0.05,
        duplicate: 0.10,
        reorder: 0.10,
    };
    let (_, control_bob, _) = run_shove_trio(install.path(), false, 1100, 1000, None);
    let (alice, bob, link) = run_shove_trio(install.path(), true, 2800, 2500, Some(recipe));
    eprintln!("impaired shove control bob={control_bob}\n ram alice={alice}\n bob={bob}");
    // The convergence below only means something if the link really
    // misbehaved while the shove played out: both directions carried
    // the data plane and each lost, duplicated and reordered frames.
    let (up, down) = link.expect("the impaired run reports its proxy counters");
    for (dir, stats) in [("up", up), ("down", down)] {
        assert!(stats.frames_in > 0, "{dir} carried nothing: {stats:?}");
        assert!(stats.delayed > 0, "{dir} was never held: {stats:?}");
        assert!(
            stats.dropped + stats.duplicated + stats.reordered > 0,
            "{dir} saw no impairment: {stats:?}"
        );
    }
    assert_shove_converged(&control_bob, &alice, &bob);
}

/// The shove leg's verdicts: `control_bob` is the all-parked grid,
/// `alice`/`bob` the shoved run's mid-session records.
fn assert_shove_converged(control_bob: &str, alice: &str, bob: &str) {
    for rec in [alice, bob] {
        assert_eq!(field(rec, "status"), "pass", "{rec}");
        assert_eq!(field(rec, "phase"), "playing", "{rec}");
        assert_eq!(field(rec, "finite"), "true", "{rec}");
    }
    let dist = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let at = |rec: &str, id: u16| -> Option<(f64, f64)> {
        seat_poses(rec)
            .into_iter()
            .find(|(i, ..)| *i == id)
            .map(|(_, x, z)| (x, z))
    };
    let grid = seat_poses(control_bob);
    assert_eq!(
        grid.len(),
        3,
        "the control holds all three seats: {control_bob}"
    );

    // Bob printed while alice was connected, so he holds all three
    // seats; alice holds the host's and her own (bob had left).
    assert_eq!(seat_poses(bob).len(), 3, "{bob}");
    assert!(at(alice, 0).is_some() && at(alice, 1).is_some(), "{alice}");

    // The shove really moved both cars on the authority: the host
    // drove off its grid slot and alice's car was pushed clear of hers
    // (judged on bob's complete view, against the control's grid).
    let nominal = |id: u16| at(control_bob, id).unwrap();
    for id in [0, 1] {
        let moved = dist(at(bob, id).unwrap(), nominal(id));
        assert!(
            moved > 1.5,
            "seat {id} stayed on its slot ({moved:.1} m): {bob}"
        );
    }

    // Convergence: every seat both clients hold sits in the same place,
    // to within the interpolation and the prediction's residual.
    let mut compared = 0;
    for (id, x, z) in seat_poses(alice) {
        let other = at(bob, id).unwrap_or_else(|| panic!("bob lacks seat {id}: {bob}"));
        let apart = dist((x, z), other);
        assert!(
            apart < 1.0,
            "seat {id} diverged by {apart:.2} m between processes:\n {alice}\n {bob}"
        );
        compared += 1;
    }
    assert!(compared >= 2, "{alice}");
    // The settled-divergence bound (`fix`) may have reseated alice's own
    // car — it did in the rare run her local sim shoved her harder than
    // the authority did — but never more than the one shove warrants,
    // and a bystander that was never shoved is never reseated.
    assert!(net_field(alice).own_settles <= 1, "{alice}");
    assert_eq!(net_field(bob).own_settles, 0, "{bob}");
}

/// F26-AC03's real impaired run: a hosted retail race whose data plane
/// rides a seeded [`ImpairProxy`] recipe (30 % loss, duplication and
/// reordering with a 40 ms ± 30 ms hold, both directions — the reset
/// leg's recipe) while the race plays out. The host seat is the `--bot`
/// course-follower racing the authored gates; the joined client is
/// `--parked`, so it never moves and the authority decides everything
/// it ever sees. A networked race fields none of the authored
/// opponents (MP-4), so the two humans are the whole field.
///
/// The leg pins what every wall-clock window must show: race rows and
/// per-seat progress tails crossed the lossy link and landed
/// (`race>0a`, `prog>0a`), the client races the hosted generation of
/// the same two-seat field (`mp=gen1`, `pos=n/2` — never a silently
/// re-minted private race), the two processes name the same course
/// (`cp=…/N` on both), and at least one recorded result reached the
/// client over the impaired link (`results>=1` — in a measured run
/// the host's finish landed while the client's own seat was still
/// racing). A resolved client seat carries the wire's word for itself
/// (`outcome=timed-out`), never a local one. Full terminal standings
/// agreement across every recipe is the in-process `net_app`
/// race-results matrix's pin; this leg is the real-socket,
/// real-content, real-impairment run beside it.
///
/// Frame budgets stagger as the other legs do: the client's cap lands
/// mid-race (measured: after the host's bot had finished), the host's
/// later cap prints while its session — and `RaceState` — is still
/// up, the only window a host record carries the race cells at all.
/// The `--parked` seat can be reseated across a gate by the recovery
/// path, so its `cp=` count is whatever the authority credited — the
/// pin is the shared denominator, never a zero numerator. Skipped
/// without `MM2_RETAIL=<dir>`. Same machine, loopback: the recipe is
/// synthetic impairment on real sockets, not a measured WAN, and
/// headless runs show no rendering.
#[test]
fn a_networked_retail_race_agrees_across_an_impaired_link() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, 90_000);
    // The retail world and a real event race, not the dev cruise.
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(
        ["--city", "sf", "--event", "checkpoint:0", "--bot"]
            .into_iter()
            .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let proxy = ImpairProxy::loopback_seeded(addr, 0xF26_AC03).unwrap();
    let mut client_args = join_args(&retail, proxy.addr(), "bob", 45_000);
    client_args.push("--parked".into());
    let client = Proc::spawn(MM2_EXE, &client_args);
    start_when_ready(&mut host, 1);
    // `Start` rides Down: let the still-clean lane deliver it (a
    // one-shot verb has no retransmit), then arm both directions for
    // the whole race window.
    std::thread::sleep(Duration::from_millis(400));
    let recipe = Impair {
        delay: Duration::from_millis(40),
        jitter: Duration::from_millis(30),
        loss: 0.30,
        duplicate: 0.10,
        reorder: 0.10,
    };
    proxy.set(LinkDir::Up, recipe);
    proxy.set(LinkDir::Down, recipe);

    // Bounds are failure guards, not expectations: a retail load on a
    // contended machine is minutes, and the caps — not the bounds —
    // decide when each record prints.
    let bound = Duration::from_secs(600);
    let rec = client.until_within("smoke=headless-physics", bound);
    assert!(
        client.wait().success(),
        "the client did not exit cleanly: {rec}"
    );
    let host_rec = host.until_within("smoke=headless-physics", bound);
    assert!(
        host.wait().success(),
        "the host did not exit cleanly: {host_rec}"
    );
    // The operator's evidence: both records, as the runs printed them.
    eprintln!("host   {host_rec}\nclient {rec}");

    // The recipe really bit on the wire in both directions.
    let (up, down) = (proxy.stats(LinkDir::Up), proxy.stats(LinkDir::Down));
    assert!(up.frames_in > 0 && down.frames_in > 0, "{up:?} {down:?}");
    assert!(
        up.dropped + up.duplicated + up.reordered > 0,
        "no upstream impairment observed: {up:?}"
    );
    assert!(
        down.dropped + down.duplicated + down.reordered > 0,
        "no downstream impairment observed: {down:?}"
    );
    drop(proxy);

    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert_eq!(field(&host_rec, "status"), "pass", "{host_rec}");

    // Race rows and per-seat progress tails crossed the lossy link
    // and landed on the client — the impaired run is a data-plane run.
    let net = net_field(&rec);
    assert!(net.race_applied > 0, "no race row landed: {rec}");
    assert!(net.progress_applied > 0, "no progress tail landed: {rec}");

    // The client races the hosted generation of the same two-seat
    // field and the same course as the host — no silently re-minted
    // private race, no divergent definition.
    assert_eq!(field(&rec, "mp"), "gen1", "{rec}");
    assert!(
        field(&rec, "pos").ends_with("/2"),
        "the client's live order does not hold both seats: {rec}"
    );
    fn course(line: &str) -> &str {
        let cp = field(line, "cp");
        &cp[cp.find('/').unwrap()..]
    }
    assert_eq!(course(&rec), course(&host_rec), "{host_rec}\n{rec}");

    // At least one recorded result reached the client over the
    // impaired link (measured: the host's finish crossed while the
    // client's own seat was still racing), and the host holds a
    // recorded result of its own (measured: `outcome=finished` — the
    // bot raced the course to the end).
    let results: u64 = field(&rec, "results").parse().unwrap();
    assert!(results >= 1, "no result crossed the link: {rec}");
    let host_results: u64 = field(&host_rec, "results").parse().unwrap();
    assert!(host_results >= 1, "the host recorded no result: {host_rec}");

    // A resolved client seat carries the authority's word only: the
    // parked car is timed out by the wire (or still racing when its
    // cap lands), never finished and never self-awarded.
    if let Some(outcome) = rec
        .split_whitespace()
        .find_map(|t| t.strip_prefix("outcome="))
    {
        assert_eq!(outcome, "timed-out", "{rec}");
    }
}

/// F26-AC05/AC02's rematch leg at process level on a retail city: the
/// same two OS processes play two rounds of one lobby without
/// reconnecting. Round one runs for a bounded wall-clock window — the
/// host's full-throttle seat drives the retail city — then the host
/// `cancel`s back to the lobby, the client readies itself again (its
/// `--ready` was a command-line flag) and `start` mints generation 2.
/// The client's frame cap is sized to land inside round two (round one
/// burns ~30 s of frames; even a much faster headless run leaves the
/// cap inside round two), so its record must show the round-two
/// world: the same non-empty `SiteTable` digest the host stamps (the
/// world re-stamped intact across the restart), every prop row
/// accepted (`mism0` — a prop stage or ledger carried over from round
/// one is scoped to the old generation and would be refused or
/// applied as stale), and the cruise's parked-car skip (`parked0`)
/// still in force. Both records are printed as the operator's
/// evidence, the client's live `bng` census included.
///
/// What stays the in-process `net_app` leg's
/// (`a_rematch_does_not_carry_the_last_rounds_broken_props_to_the_client`):
/// the phase-level dormancy pin — a round-two world that stays
/// dormant until the host breaks something new in it. An end-of-run
/// record cannot watch two mid-round worlds, and prop phases are not
/// on the wire record. Skipped without `MM2_RETAIL=<dir>`.
#[test]
fn a_rematch_on_a_retail_city_restamps_the_world_for_the_same_client() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut host_args = host_args(&retail, HOST_FRAME_CEILING);
    // The retail world, not the dev cruise.
    host_args.retain(|a| a != "--dev-world");
    host_args.extend(["--city".into(), "sf".into()]);
    let mut host = Proc::spawn(MM2_EXE, &host_args);
    let addr = listening_addr(&host);
    let client = Proc::spawn(MM2_EXE, &join_args(&retail, addr, "alice", 35_000));
    start_when_ready(&mut host, 1);
    host.until("remote participant spawned");

    // Round one: a bounded window of real driving on the retail city.
    std::thread::sleep(Duration::from_secs(25));
    host.cmd("cancel");
    host.until("event=cancelled generation=1");
    // Nobody pressed anything: the readiness came from the client's
    // `--ready`, after the host's cancel cleared it.
    host.until("ready=true");
    host.cmd("start");
    host.until("event=started generation=2");
    host.until("remote participant spawned");

    // Round two plays on (the host stays up): the client's cap lands
    // mid-round-2 and its record reads the restarted world. The bound
    // is a failure guard, not an expectation — a retail reload on a
    // contended machine is minutes.
    let rec = client.until_within("smoke=headless-physics", Duration::from_secs(600));
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert_eq!(
        field(&rec, "mp"),
        "gen2",
        "the cap landed outside round two: {rec}"
    );
    assert_eq!(field(&rec, "phase"), "playing", "{rec}");
    assert!(
        client.wait().success(),
        "the client did not exit cleanly: {rec}"
    );

    host.cmd("quit");
    let host_rec = host.until("smoke=headless-physics");
    assert!(
        host.wait().success(),
        "the host did not exit cleanly: {host_rec}"
    );
    // The operator's evidence: both records, as the runs printed them
    // (the client's `bng=` census shows its round-two world really
    // evolved).
    eprintln!("host   {host_rec}\nclient {rec}");

    // Round two stamped a real, identical world on both processes —
    // the restart re-stamped it, it did not carry a stale table.
    let (count, digest, _landed, mismatched) = props_field(&rec);
    assert!(count > 0, "the retail city stamps placements: {rec}");
    assert_eq!(mismatched, 0, "the client refused the host's rows: {rec}");
    let (host_count, host_digest, ..) = props_field(&host_rec);
    assert_eq!(
        (host_count, host_digest),
        (count, digest),
        "host and client stamped different round-two worlds:\nhost   {host_rec}\nclient {rec}"
    );
    // Every round-two row was scoped to round two: nothing the host
    // re-sent for the ended generation slipped through as stale, and
    // nothing the client's own stage still held was misapplied. (The
    // `mism0` cell asserted above is that count.)
    // The networked cruise kept skipping kerbside parked cars in the
    // restarted round, like the clean leg's first one.
    assert_eq!(parked_cars(&rec), 0, "{rec}");
    assert_eq!(parked_cars(&host_rec), 0, "{host_rec}");
}
