//! NETPLAY-06F: the lobby lifecycle on the datagram lane, across real
//! `mm2` processes — a client that vanishes mid-session, and a client
//! that joins a session already running, the second one over a lossy
//! lane. (`net_lane_loss` plays start, reset, rematch and leave over the
//! same lane; `net_drive` has the clean-lane rematch and late join.)
//!
//! Every client here negotiates the lane (`transport=lane`), so these
//! are the lane's lifecycle, not a TCP-only path's. The legs:
//! - a SIGKILLed client (its sockets close) is reaped by the host as
//!   `cause=lost` and the host's session keeps running. A client that
//!   goes silent with its sockets open (SIGSTOP) is *not* reaped by the
//!   host today: only the client side has a liveness timeout; see
//!   `docs/netcode-redesign.md`, "Lifecycle on the lane";
//! - a second client then joins that running generation and drives it;
//! - a late joiner behind a seeded lossy [`UdpImpairProxy`] is handed the
//!   live generation and drives it.
//!
//! Evidence level: synthetic integration (real OS processes, loopback,
//! dev world, no retail data). Loopback says nothing about whether UDP
//! crosses a real router: that is OWNER #1249/#1240
//! (`docs/multiplayer-reachability.md`).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::net_drive::{
    HOST_FRAME_CEILING, field, host_args, join_args, listening_addr, net_field,
    quit_and_assert_host_drove,
};
use crate::support::Proc;
use mm2_net::{Impair, LinkDir, UdpImpair, UdpImpairProxy};

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// A closed socket is noticed at once; the bound is wall-clock slack for
/// a loaded machine without letting a missing reap pass.
const LOST_BOUND: Duration = Duration::from_secs(30);

#[test]
fn a_killed_client_is_reaped_and_a_new_one_joins_the_running_session() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), HOST_FRAME_CEILING));
    let addr = listening_addr(&host);

    // `doomed` plays long enough that only the kill ends it.
    let mut doomed = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "doomed", 600_000));
    host.until("ready=true");
    host.cmd("start");
    host.until("event=started generation=1");
    host.until("remote participant spawned");

    doomed.kill();
    let killed = Instant::now();
    let left = host.until_within("event=left", LOST_BOUND);
    let reaped_in = killed.elapsed();
    assert!(left.contains("cause=lost"), "{left}");
    eprintln!("lifecycle: killed client reaped as lost after {reaped_in:?}");

    // The session outlived its client: a newcomer gets generation 1.
    let late = Proc::spawn(MM2_EXE, &join_args(install.path(), addr, "late", 4500));
    host.until("remote participant spawned");
    let rec = late.until_within("smoke=headless-physics", Duration::from_secs(120));
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert_eq!(field(&rec, "mp"), "gen1", "{rec}");
    let net = net_field(&rec);
    assert!(net.snaps_applied > 0 && net.inputs_sent > 0, "{rec}");
    late.wait_success("late", &rec);
    assert!(host.until("event=left").contains("cause=quit"));
    quit_and_assert_host_drove(host);
}

#[test]
fn a_late_joiner_behind_a_lossy_lane_is_handed_the_live_session() {
    let install = tempfile::tempdir().unwrap();
    let mut host = Proc::spawn(MM2_EXE, &host_args(install.path(), HOST_FRAME_CEILING));
    let listening = host.until("listening=");
    let addr: SocketAddr = field(&listening, "listening").parse().unwrap();
    let lane_port: u16 = field(&listening, "lane_port").parse().unwrap();
    let proxy =
        UdpImpairProxy::loopback_seeded(SocketAddr::new(addr.ip(), lane_port), 0x06F1).unwrap();
    let route = proxy.route();

    host.cmd("start");
    host.until("event=started generation=1");

    let mut args = join_args(install.path(), addr, "late", 6000);
    args.extend([
        "--lane-via".to_string(),
        format!("{}/{}", route.dial, route.advertise_port),
        "--deadline".into(),
        "120".into(),
    ]);
    let late = Proc::spawn(MM2_EXE, &args);
    // The join and the establishing probe cross a clean lane; loss
    // starts once the late seat is in the running session.
    host.until("remote participant spawned");
    let lossy = UdpImpair {
        link: Impair {
            loss: 0.15,
            duplicate: 0.10,
            reorder: 0.10,
            ..Impair::default()
        },
        ..UdpImpair::default()
    };
    proxy.set(LinkDir::Up, lossy);
    proxy.set(LinkDir::Down, lossy);

    let rec = late.until_within("smoke=headless-physics", Duration::from_secs(180));
    late.wait_success("late", &rec);
    assert_eq!(field(&rec, "status"), "pass", "{rec}");
    assert_eq!(field(&rec, "mp"), "gen1", "{rec}");
    let net = net_field(&rec);
    assert!(net.snaps_applied > 0 && net.inputs_sent > 0, "{rec}");
    for dir in [LinkDir::Up, LinkDir::Down] {
        let stats = proxy.stats(dir).link;
        assert!(
            stats.dropped + stats.duplicated + stats.reordered > 0,
            "{dir:?} saw no impairment: {stats:?}"
        );
    }
    assert!(host.until("event=left").contains("cause=quit"));
    quit_and_assert_host_drove(host);
}
