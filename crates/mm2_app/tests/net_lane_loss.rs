//! NETPLAY-06E-5: the lane's reliable recovery across real `mm2`
//! processes. `mm2_net`'s `lane_lobby_tests` / `lane_reliable_tests` prove
//! the first-datagram-lost cases in one process; here a host and a joined
//! client are separate OS processes whose UDP lane runs through a seeded
//! [`UdpImpairProxy`] (`mm2 --join --lane-via`), with a datagram dropped
//! ahead of each lobby event and 15 % loss, duplication and reordering on
//! top. One lobby plays: `Start` (generation 1), a reset ask, `cancel`
//! and the client's re-ready (the rematch), `Start` again (generation 2),
//! then the client's `Leave`. Each must apply once: the host mints
//! exactly generations 1 and 2, the client ends in generation 2, the
//! host grants the reset it was asked for and reaps the seat once.
//!
//! The dropped datagram is armed with [`UdpImpairProxy::drop_next`] just
//! before the event; once a session runs, input and snapshot traffic can
//! take that drop instead of the lifecycle datagram, so the per-event
//! guarantee at the datagram level is `lane_lobby_tests`' and this leg
//! proves the same machinery survives a real lane between processes.
//!
//! A second leg holds a client on the `accel` route (full throttle for
//! ten seconds) through the same lossy lane and checks the host's copy
//! of the car against the client's prediction (#2288).
//!
//! Evidence level: synthetic integration (real OS processes, loopback,
//! seeded proxy, dev world, no retail data).

use std::net::SocketAddr;
use std::time::Duration;

use crate::net_drive::{
    HOST_FRAME_CEILING, field, host_args, join_args, net_field, quit_and_assert_host_drove,
};
use crate::support::{Proc, drive_bound};
use mm2_net::{Impair, LinkDir, UdpImpair, UdpImpairProxy};

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

fn lossy() -> UdpImpair {
    UdpImpair {
        link: Impair {
            loss: 0.15,
            duplicate: 0.10,
            reorder: 0.10,
            ..Impair::default()
        },
        ..UdpImpair::default()
    }
}

#[test]
fn start_reset_rematch_and_leave_apply_once_over_a_lossy_lane_between_processes() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), HOST_FRAME_CEILING));
    let listening = host.until("listening=");
    let addr: SocketAddr = field(&listening, "listening").parse().unwrap();
    let lane_port: u16 = field(&listening, "lane_port").parse().unwrap();
    let proxy =
        UdpImpairProxy::loopback_seeded(SocketAddr::new(addr.ip(), lane_port), 0x06E5).unwrap();
    let route = proxy.route();

    let mut args = join_args(install.path(), addr, "alice", 6000);
    args.extend([
        "--lane-via".to_string(),
        format!("{}/{}", route.dial, route.advertise_port),
        "--reset-at".into(),
        "300".into(),
        "--deadline".into(),
        "120".into(),
    ]);
    let alice = Proc::spawn(MM2_EXE, &args);

    // The join and the first ready cross a clean lane; loss starts here.
    host.until("ready=true");
    proxy.set(LinkDir::Up, lossy());
    proxy.set(LinkDir::Down, lossy());

    // `Start`, with the next downstream datagram lost.
    proxy.drop_next(LinkDir::Down, 1);
    host.cmd("start");
    host.until("event=started generation=1");
    host.until("remote participant spawned");

    // The rematch: `cancel` clears the ready flag, the client's
    // `--ready` re-readies over the lossy lane, `Start` mints generation 2.
    proxy.drop_next(LinkDir::Down, 1);
    host.cmd("cancel");
    host.until("event=cancelled generation=1");
    proxy.drop_next(LinkDir::Up, 1);
    host.until("ready=true");
    proxy.drop_next(LinkDir::Down, 1);
    host.cmd("start");
    host.until("event=started generation=2");
    host.until("remote participant spawned");

    let bound = Duration::from_secs(120 + 60);
    let rec = alice.until_within("smoke=headless-physics", bound);
    alice.wait_success("alice", &rec);
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert_eq!(
        field(&rec, "mp"),
        "gen2",
        "the client ended in the rematch: {rec}"
    );
    let net = net_field(&rec);
    assert!(net.snaps_applied > 0 && net.inputs_sent > 0, "{rec}");
    // The ask repeats every `RESET_RETRY_TICKS` until the epoch snap
    // comes back, and this lane drops both directions, so the count of
    // wire asks is a floor and a bound, not an exact figure. The
    // property is that the reset *applied* once.
    assert!(
        (1..mm2_app::netdrive::MAX_RESET_ASKS as u64).contains(&net.requests_sent),
        "the reset ask went out, but not once per update: {rec}"
    );
    assert_eq!(net.resets, 1, "the reset applied exactly once: {rec}");

    // The relay held up its end: it really dropped and duplicated.
    for dir in [LinkDir::Up, LinkDir::Down] {
        let stats = proxy.stats(dir).link;
        assert!(
            stats.dropped + stats.duplicated + stats.reordered > 0,
            "{dir:?} saw no impairment: {stats:?}"
        );
    }

    // Her `Leave` reaped the seat once, as a quit and not a lost socket.
    let left = host.until("event=left");
    assert!(left.contains("cause=quit"), "{left}");
    let host_net = quit_and_assert_host_drove(host);
    eprintln!(
        "lane-loss alice={rec}\n granted={} dropped={} sent={}",
        host_net.requests_granted, host_net.requests_dropped, net.requests_sent
    );
    assert_eq!(
        host_net.requests_granted, 1,
        "the host granted the one reset it was asked for"
    );
    // A repeat that crossed the grant is dropped (cooldown), never granted.
    assert!(
        host_net.requests_granted + host_net.requests_dropped <= net.requests_sent,
        "the host answered no more asks than the client sent"
    );
}

/// Where the original lossy C&R round snapped the client's car (#2287):
/// the host's copy trailed the prediction by about this much.
const SNAP_GAP_M: f64 = 20.0;

/// A client holding full throttle through a seeded lossy lane keeps its
/// prediction and the host's copy of its car together. The client's
/// `own_correction` records (written only when the two parted far enough
/// to pull, settle or snap, so a clean run has none) carry the distance
/// between the two; none may reach the snap threshold, and the host must have
/// run the seat on its own inputs for most of the drive (a seat starved
/// of input shows as `inputs_staled`). Deterministic: the seeded proxy,
/// the scripted route and a bounded wait on the client's record — no
/// wall-clock-paced bots.
#[test]
fn the_host_copy_of_a_throttling_client_stays_with_its_prediction_over_a_lossy_lane() {
    const FRAMES: u32 = 1500;
    let install = tempfile::tempdir().unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/captures/netcode-lossy-throttle");
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("alice.jsonl");

    let mut args = host_args(install.path(), HOST_FRAME_CEILING);
    args.push("--paced".into());
    let mut host = Proc::spawn(MM2_EXE, &args);
    let listening = host.until("listening=");
    let addr: SocketAddr = field(&listening, "listening").parse().unwrap();
    let lane_port: u16 = field(&listening, "lane_port").parse().unwrap();
    let proxy =
        UdpImpairProxy::loopback_seeded(SocketAddr::new(addr.ip(), lane_port), 213).unwrap();
    let route = proxy.route();

    let mut args = join_args(install.path(), addr, "alice", FRAMES);
    args.extend([
        "--lane-via".to_string(),
        format!("{}/{}", route.dial, route.advertise_port),
        "--paced".into(),
        "--net-route".into(),
        "accel".into(),
        "--net-trace".into(),
        trace.to_str().unwrap().into(),
    ]);
    let alice = Proc::spawn(MM2_EXE, &args);
    host.until("ready=true");
    host.cmd("start");
    host.until("event=started generation=1");
    // `Start` is one-shot: let the clean lane deliver it, then impair.
    crate::net_drive::until_live(&alice);
    proxy.set(LinkDir::Up, lossy());
    proxy.set(LinkDir::Down, lossy());

    let rec = alice.until_within("smoke=headless-physics", drive_bound(FRAMES));
    alice.wait_success("alice", &rec);
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    for dir in [LinkDir::Up, LinkDir::Down] {
        let stats = proxy.stats(dir).link;
        assert!(stats.dropped > 0, "{dir:?} dropped nothing: {stats:?}");
    }
    let net = net_field(&rec);
    let host_net = quit_and_assert_host_drove(host);
    eprintln!(
        "lossy throttle alice={rec}\n host applied={} staled={}",
        host_net.inputs_applied, host_net.inputs_staled
    );
    assert!(
        host_net.inputs_applied > host_net.inputs_staled,
        "the host ran the seat on substitutes more often than on its inputs: applied={} staled={}",
        host_net.inputs_applied,
        host_net.inputs_staled
    );
    assert!(net.snaps_applied > 0 && net.inputs_sent > 0, "{rec}");

    let text = std::fs::read_to_string(&trace).unwrap();
    let gaps: Vec<f64> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["ev"] == "own_correction")
        .filter_map(|v| v["gap"].as_f64())
        .collect();
    let worst = gaps.iter().copied().fold(0.0, f64::max);
    eprintln!("{} corrections, worst gap {worst:.1} m", gaps.len());
    assert!(
        worst < SNAP_GAP_M,
        "the host's copy trailed the prediction by {worst:.1} m (snap threshold {SNAP_GAP_M} m)"
    );
}
