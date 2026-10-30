//! NETPLAY-10D-5 (#2227): the island criterion measured with a *driven*
//! client behind the seeded impairment proxy on a crowded retail city.
//! A retail `sf` free-for-all host with two lobby bots and `--ram`
//! (full throttle at the nearest other car, so the bots and the human
//! keep closing inside the 20 m + speed x 1 s island radius), one client
//! that drives a slalom with its lane behind a lossy [`UdpImpairProxy`].
//!
//! The host record's `grp=`/`flip=`/`gb=` cells give the baselined-state
//! saving, the owner/world group rows and bytes, and the owner-group
//! churn; the client's `corr=` cell the own-car correction gaps. The
//! measured numbers are in `docs/netcode-redesign.md`.
//!
//! Evidence level: original-content validation (real processes, retail
//! `sf`, loopback behind the proxy). `#[ignore]`d: it runs for a couple of
//! minutes of wall clock. Skips without `MM2_RETAIL`.

use std::net::SocketAddr;
use std::time::Duration;

use crate::net_drive::{HOST_FRAME_CEILING, field, host_args, join_args, start_when_ready};
use crate::support::{self, Proc};
use mm2_net::{Impair, LinkDir, UdpImpair, UdpImpairProxy};

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");
const SEED: u64 = 1291;
/// The client leaves at this session tick (60 Hz): two minutes.
const SNAP_TICK: u32 = 7200;
/// Bots start within this many metres of a human start (#2236).
const NEAR_M: u32 = 120;
const BOUND: Duration = Duration::from_secs(600);

fn cells(rec: &str, key: &str) -> Vec<u64> {
    field(rec, key)
        .split('/')
        .map(|c| c.trim_end_matches('c').parse().unwrap())
        .collect()
}

#[test]
#[ignore = "two minutes of wall clock over real processes: run with --ignored and MM2_RETAIL set"]
fn a_driven_client_behind_a_lossy_link_crosses_islands_on_retail_sf() {
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let mut args = host_args(&retail, HOST_FRAME_CEILING);
    args.retain(|a| a != "--dev-world");
    let at = args.iter().position(|a| a == "--seed").unwrap();
    args[at + 1] = SEED.to_string();
    args.extend(
        [
            "--city",
            "sf",
            "--cnr",
            "ffa",
            "--cnr-limit",
            "5m",
            "--paced",
            "--ram",
        ]
        .map(String::from),
    );
    // Bots are drawn within 120 m of a human start, so they close on the
    // human at once instead of starting about 130 m away (#2236).
    let near = NEAR_M.to_string();
    let mut host = Proc::spawn_env(MM2_EXE, &args, &[("MM2_BOTS_NEAR_HUMANS", &near)]);
    let listening = host.until("listening=");
    let addr: SocketAddr = field(&listening, "listening").parse().unwrap();
    let lane_port: u16 = field(&listening, "lane_port").parse().unwrap();
    host.cmd(&format!("session bots=2 seed={SEED}"));
    host.until("event=session_changed");

    let proxy =
        UdpImpairProxy::loopback_seeded(SocketAddr::new(addr.ip(), lane_port), 0x2227).unwrap();
    let lossy = UdpImpair {
        link: Impair {
            delay: Duration::from_millis(20),
            loss: 0.03,
            reorder: 0.02,
            ..Impair::default()
        },
        ..UdpImpair::default()
    };
    proxy.set(LinkDir::Down, lossy);
    proxy.set(LinkDir::Up, lossy);
    let route = proxy.route();
    let mut cargs = join_args(&retail, addr, "bob", HOST_FRAME_CEILING);
    cargs.extend(
        [
            "--paced",
            "--net-route",
            "slalom",
            "--until-snap-tick",
            &SNAP_TICK.to_string(),
            "--deadline",
            "900",
            "--lane-via",
            &format!("{}/{}", route.dial, route.advertise_port),
        ]
        .map(String::from),
    );
    let client = Proc::spawn(MM2_EXE, &cargs);
    start_when_ready(&mut host, 1);

    let crec = client.until_within("smoke=headless-physics", BOUND);
    client.wait_success("the client", &crec);
    host.cmd("quit");
    let hrec = host.until_within("smoke=headless-physics", BOUND);
    eprintln!("host   {hrec}\nclient {crec}");

    let grp = cells(&hrec, "grp");
    let (sent, raw) = (grp[2], grp[3]);
    assert!(sent > 0 && sent <= raw, "no baselined saving: {hrec}");
    let gb = cells(&hrec, "gb");
    assert_eq!(gb[0] + gb[2], sent, "per-group bytes must sum to the total");
    assert_eq!(gb[1] + gb[3], raw, "{hrec}");
    // The run only answers the crowded case if a bot reached a human.
    let bnear = field(&hrec, "bnear");
    eprintln!("bnear (closest m / ticks in island reach): {bnear}");
    let corr = cells(&crec, "corr");
    assert!(corr[0] > 0, "the driven client was never corrected: {crec}");
}
