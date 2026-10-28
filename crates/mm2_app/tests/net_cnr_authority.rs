//! NETPLAY-08B: Cops & Robbers authority across real client processes
//! on impaired links (F27-AC02 / F27-AC05, F25 physics-replay safety).
//!
//! The gold's ownership, the scores and the verdict are decided once, by
//! the host's [`mm2_app::cnr::cnr_host_step`] on the forward timeline; a
//! client predicts and replays *physics* only (`mm2_vehicle::replay`
//! never runs that system) and reads the match from the host's frames.
//! This is the process-level proof that the two stay separate when the
//! clients' own cars are being corrected: a hosted retail `sf`
//! free-for-all with the host seat parked, one joined client the
//! thief and the other the carrier, each behind its own seeded lossy,
//! duplicating, reordering UDP proxy (so the own-seat prediction is
//! corrected throughout the contested transfer). The roles are then
//! swapped between the two seats.
//!
//! Asserted: the host's log is a legal gold history (nothing is picked
//! while carried, only the carrier drops or delivers, a delivery frees
//! the gold), the contested transfer happened between the two client
//! seats (the knock, the recovery and the delivery are the thief's),
//! each client's replica reads the host's verdict and the *same* board,
//! and that board is exactly the sum of the host's scoring events —
//! a replayed contact that re-awarded anything on a client or on the
//! host would show as a score the log never granted.
//!
//! Evidence level: original-content validation, real processes over
//! loopback with seeded impairment, headless. Skips without
//! `MM2_RETAIL`. Not covered: the host seat in either role (see
//! `net_drive`'s host-thief test), a third contender for one pickup,
//! rendered output.

use std::net::SocketAddr;
use std::time::Duration;

use mm2_net::{Impair, LinkDir, UdpImpair, UdpImpairProxy};

use crate::net_drive::{cnr_cell, field, host_args, join_args, until_live};
use crate::support::{self, Proc};

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");
/// The retail `sf` seed the other contested-transfer tests use: the
/// carrier is reached and the cycle completes within the point limit.
const SEED: u64 = 1291;
const STEP: Duration = Duration::from_secs(60);
const BOUND: Duration = Duration::from_secs(1200);

/// Loss, duplication and reordering on a link, with a small delay.
fn lossy(delay_ms: u64) -> UdpImpair {
    UdpImpair {
        link: Impair {
            delay: Duration::from_millis(delay_ms),
            jitter: Duration::from_millis(delay_ms / 4),
            loss: 0.03,
            duplicate: 0.03,
            reorder: 0.03,
        },
        ..UdpImpair::default()
    }
}

fn leading_u64(s: &str) -> u64 {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("no number in {s:?}"))
}

/// `player: PlayerId(n)` of a host event line.
fn player_of(line: &str) -> u16 {
    let at = line
        .find("player: PlayerId(")
        .expect("an event with a player")
        + "player: PlayerId(".len();
    let end = line[at..].find(')').unwrap() + at;
    line[at..end].parse().unwrap()
}

fn points_of(line: &str) -> u64 {
    let at = line.find("points: ").expect("an event with points") + "points: ".len();
    leading_u64(&line[at..])
}

/// The host's gold history must be a legal sequence: the gold is held
/// by at most one car, only its holder drops or delivers it, and a
/// pickup never lands on a car that already holds it. Returns the
/// score each seat was granted.
fn legal_history(events: &[String]) -> std::collections::BTreeMap<u16, u64> {
    let mut holder: Option<u16> = None;
    let mut awarded = std::collections::BTreeMap::new();
    for line in events {
        if line.contains("Picked {") {
            assert!(
                holder.is_none(),
                "picked while carried by {holder:?}: {line}"
            );
            holder = Some(player_of(line));
            *awarded.entry(player_of(line)).or_default() += points_of(line);
        } else if line.contains("Dropped {") {
            assert_eq!(
                holder,
                Some(player_of(line)),
                "dropped by a non-holder: {line}"
            );
            holder = None;
        } else if line.contains("Delivered {") {
            assert_eq!(
                holder,
                Some(player_of(line)),
                "delivered by a non-holder: {line}"
            );
            holder = None;
            *awarded.entry(player_of(line)).or_default() += points_of(line);
        }
    }
    awarded
}

/// One contested transfer with `thief` (wire seat 1 or 2) chasing the
/// other client, both behind lossy proxies; `thief == 1` is alice.
fn contested_transfer(thief: u16, seeds: [u64; 2], delays: [u64; 2]) {
    assert!(thief == 1 || thief == 2);
    let Some((retail, _slot)) = support::retail_slot() else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let carrier = 3 - thief;
    let mut args = host_args(&retail, 100_000);
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
            "100pts",
            "--parked",
            "--paced",
        ]
        .map(String::from),
    );
    let mut host = Proc::spawn(MM2_EXE, &args);
    let listening = host.until("listening=");
    let addr: SocketAddr = field(&listening, "listening").parse().unwrap();
    let lane_port: u16 = field(&listening, "lane_port").parse().unwrap();
    let lane = SocketAddr::new(addr.ip(), lane_port);
    let proxies = [
        UdpImpairProxy::loopback_seeded(lane, seeds[0]).unwrap(),
        UdpImpairProxy::loopback_seeded(lane, seeds[1]).unwrap(),
    ];

    // Join order fixes the wire seats: alice 1, bob 2.
    let client = |who: &str, seat: u16, via: &UdpImpairProxy| {
        let mut flags = join_args(&retail, addr, who, 60_000);
        let route = via.route();
        flags.extend([
            "--lane-via".to_string(),
            format!("{}/{}", route.dial, route.advertise_port),
            "--paced".into(),
            "--bot".into(),
        ]);
        if seat == thief {
            flags.push("--bot-thief".into());
        } else {
            flags.extend(["--bot-speed".into(), "6".into()]);
        }
        flags
    };
    let alice = Proc::spawn(MM2_EXE, &client("alice", 1, &proxies[0]));
    host.until_within("ready=true", STEP);
    let bob = Proc::spawn(MM2_EXE, &client("bob", 2, &proxies[1]));
    host.until_within("ready=true", STEP);
    host.cmd("start");
    host.until_within("event=started generation=1", STEP);
    // `Start` is a one-shot verb: let the clean lane deliver it, then
    // arm the impairment for everything that follows.
    until_live(&alice);
    until_live(&bob);
    for (proxy, ms) in proxies.iter().zip(delays) {
        proxy.set(LinkDir::Up, lossy(ms));
        proxy.set(LinkDir::Down, lossy(ms));
    }

    let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (first, _) = host.until_within_collect("Picked {", BOUND);
        let (ended, between) = host.until_within_collect("Ended(", BOUND);
        let mut all = vec![first];
        all.extend(between);
        all.push(ended);
        all
    }));
    let lines = waited.unwrap_or_else(|panic| {
        eprintln!("--- alice ---\n{}", alice.pending());
        eprintln!("--- bob ---\n{}", bob.pending());
        eprintln!("--- host ---\n{}", host.pending());
        std::panic::resume_unwind(panic)
    });
    let events: Vec<String> = lines
        .into_iter()
        .filter(|l| l.contains("cops and robbers event"))
        .collect();
    for line in &events {
        eprintln!("host   {line}");
    }
    let awarded = legal_history(&events);

    let picked = &events[0];
    assert_eq!(
        player_of(picked),
        carrier,
        "the carrier picked first: {picked}"
    );
    assert!(picked.contains("recovered: false"), "{picked}");
    let knock = events
        .iter()
        .find(|l| l.contains("Dropped {"))
        .expect("the thief never knocked the gold loose");
    assert_eq!(player_of(knock), carrier, "{knock}");
    assert!(
        knock.contains(&format!("cause: Knocked {{ by: Some(PlayerId({thief})) }}")),
        "the gold was not rammed loose by the client thief: {knock}"
    );
    assert!(
        events.iter().any(|l| l.contains("Picked {")
            && l.contains("recovered: true")
            && player_of(l) == thief),
        "the thief never recovered the gold"
    );
    assert!(
        events
            .iter()
            .any(|l| l.contains("Delivered {") && player_of(l) == thief),
        "the thief never delivered"
    );
    let ended = events.last().unwrap();
    assert!(ended.contains("Ended("), "{ended}");
    assert!(ended.contains("reason: PointLimit"), "{ended}");
    assert!(
        ended.contains(&format!("winner: Player(PlayerId({thief}))")),
        "{ended}"
    );

    let alice_rec = alice.until_within("smoke=headless-physics", BOUND);
    let bob_rec = bob.until_within("smoke=headless-physics", BOUND);
    alice.wait_success("alice", &alice_rec);
    bob.wait_success("bob", &bob_rec);
    host.cmd("quit");
    let host_rec = host.until_within("smoke=headless-physics", BOUND);
    eprintln!("alice  {alice_rec}\nbob    {bob_rec}\nhost   {host_rec}");

    let board = |rec: &str| {
        field(rec, "cnr")
            .split(',')
            .find_map(|c| c.strip_prefix("pts="))
            .unwrap_or_else(|| panic!("no pts: {rec}"))
            .to_string()
    };
    for (who, rec) in [("alice", &alice_rec), ("bob", &bob_rec)] {
        assert_eq!(field(rec, "status"), "pass", "{who}: {rec}");
        let cell =
            |prefix: &str| cnr_cell(rec, prefix).unwrap_or_else(|| panic!("no {prefix}: {rec}"));
        assert_eq!(cell("ref"), 0, "{who} refused the host's match: {rec}");
        assert_eq!(cell("seats"), 3, "{who}: {rec}");
        assert_eq!(cell("dec"), 1, "{who} never read the verdict: {rec}");
        assert!(
            field(rec, "cnr")
                .split(',')
                .any(|c| c == format!("win=p{thief}")),
            "{who}'s winner differs from the host's: {rec}"
        );
        assert_eq!(field(rec, "phase"), "results", "{who}: {rec}");
    }
    assert_eq!(board(&alice_rec), board(&bob_rec), "the boards differ");
    // Every seat's score is exactly what the host's log granted it.
    let board = board(&alice_rec);
    for id in 0..3u16 {
        let got = board
            .split('/')
            .find_map(|c| c.strip_prefix(&format!("{id}:")))
            .unwrap_or_else(|| panic!("seat {id} is not on the board {board}"));
        assert_eq!(
            got.parse::<u64>().unwrap(),
            awarded.get(&id).copied().unwrap_or(0),
            "seat {id} holds a score the host never granted: {board}"
        );
    }
    assert_eq!(
        awarded.get(&0),
        None,
        "the parked host seat scored: {board}"
    );

    // The relays held up their end: both links really impaired.
    for (name, p) in ["alice", "bob"].iter().zip(&proxies) {
        for dir in [LinkDir::Up, LinkDir::Down] {
            let s = p.stats(dir).link;
            assert!(
                s.dropped + s.duplicated + s.reordered > 0,
                "{name} {dir:?} saw no impairment: {s:?}"
            );
        }
    }
}

#[test]
fn a_client_thief_robs_a_client_carrier_once_over_lossy_links() {
    contested_transfer(1, [0xC0A1, 0xB0B2], [20, 60]);
}

#[test]
fn the_same_robbery_with_the_roles_swapped_commits_once_over_lossy_links() {
    contested_transfer(2, [0xD1B3, 0xA2C4], [60, 20]);
}
