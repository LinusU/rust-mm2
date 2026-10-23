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
//! Evidence level: synthetic integration (real OS processes, loopback,
//! seeded proxy, dev world, no retail data).

use std::net::SocketAddr;
use std::time::Duration;

use crate::net_drive::{
    HOST_FRAME_CEILING, field, host_args, join_args, net_field, quit_and_assert_host_drove,
};
use crate::support::Proc;
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
    assert_eq!(net.requests_sent, 1, "the reset ask went out: {rec}");

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
    assert_eq!(host_net.requests_dropped, 0);
}
