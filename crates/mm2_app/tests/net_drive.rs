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
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
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
    let bob_net = assert_client_drove(&bob.until("smoke=headless-physics"), 2);
    let alice_net = assert_client_drove(&alice.until("smoke=headless-physics"), 1);
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
    let mut host = Proc::spawn(MM2_EXE, &host_args(install, 9000));
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
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
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
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
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
    // frames; the client holds a copy of each and never ran one.
    let (cable_sent, _) = cable_field(&host_rec)
        .unwrap_or_else(|| panic!("the host published no cable car: {host_rec}"));
    assert!(cable_sent >= 4, "{host_rec}");
    let (_, cable_live) =
        cable_field(&rec).unwrap_or_else(|| panic!("the client holds no cable car copy: {rec}"));
    assert_eq!(cable_live, 4, "{rec}");
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
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
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
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
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

/// One lobby of three processes on the dev world where the host and
/// bob sit parked (the victims) and alice either rams the nearest car
/// (`alice_rams`) or sits parked too (the control). Returns the host's
/// post-`quit` `net=` field and the two clients' mid-session records.
fn run_collision_trio(install: &std::path::Path, alice_rams: bool) -> (NetField, String, String) {
    let mut host_flags = host_args(install, 9000);
    host_flags.push("--parked".into());
    let mut host = Proc::spawn(MM2_EXE, &host_flags);
    let addr = listening_addr(&host);

    let mut alice_flags = join_args(install, addr, "alice", 1400);
    alice_flags.push(if alice_rams { "--ram" } else { "--parked" }.into());
    let mut bob_flags = join_args(install, addr, "bob", 1000);
    bob_flags.push("--parked".into());
    let alice = Proc::spawn(MM2_EXE, &alice_flags);
    let bob = Proc::spawn(MM2_EXE, &bob_flags);
    start_when_ready(&mut host, 2);

    // Bob's cap is shorter so his record prints while alice is still
    // connected, as in the other legs.
    let bob_rec = bob.until("smoke=headless-physics");
    let alice_rec = alice.until("smoke=headless-physics");
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
    let mut host_flags = host_args(install, 9000);
    host_flags.push(if host_rams { "--ram" } else { "--parked" }.into());
    let mut host = Proc::spawn(MM2_EXE, &host_flags);
    let addr = listening_addr(&host);
    let proxy = impair.map(|_| ImpairProxy::loopback_seeded(addr, 0xAC02).unwrap());
    let join_addr = proxy.as_ref().map_or(addr, ImpairProxy::addr);
    let mut alice_flags = join_args(install, join_addr, "alice", alice_frames);
    alice_flags.push("--parked".into());
    let alice = Proc::spawn(MM2_EXE, &alice_flags);
    host.until("ready=true");
    let mut bob_flags = join_args(install, join_addr, "bob", bob_frames);
    bob_flags.push("--parked".into());
    let bob = Proc::spawn(MM2_EXE, &bob_flags);
    host.until("ready=true");
    host.cmd("start");
    host.until("event=started generation=1");
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
    assert!(alice.wait().success(), "alice did not exit cleanly");
    assert!(bob.wait().success(), "bob did not exit cleanly");
    let link = proxy
        .as_ref()
        .map(|p| (p.stats(LinkDir::Up), p.stats(LinkDir::Down)));
    // The relay holds socket clones until it drops: the host only sees
    // the clients leave once the proxy is down.
    drop(proxy);
    for _ in 0..2 {
        host.until("event=left");
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
