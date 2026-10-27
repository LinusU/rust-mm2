//! CNR-MP-BOTS-D: a hosted Cops & Robbers match with host-controlled
//! bots, played by separate OS processes over loopback — a retail `sf`
//! (and London) host with two lobby bots and one joined client. Both machines must
//! agree on who the bots are, where they drive, who holds the gold, the
//! teams, the scores and the verdict, and a rematch must field the same
//! bots again.
//!
//! Evidence level: original-content validation over real processes and
//! real loopback sockets (headless, no rendering, no audio). Skips
//! without `MM2_RETAIL`.

use std::time::Duration;

use crate::net_drive::{
    HOST_FRAME_CEILING, cnr_cell, field, host_args, join_args, listening_addr, start_when_ready,
};
use crate::support::{self, Proc};

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");
const BOUND: Duration = Duration::from_secs(600);
/// A retail `sf` seed on which the lobby bots drive to the gold within
/// the five-minute limit (measured locally; the bots take the pickup
/// by contact, the same rule a human's car plays under).
///
/// A `session` edit without `seed=` mints a fresh clock seed, so the
/// command must name it again or the host's `--seed` is lost and each
/// run fields different bots.
const SEED: u64 = 1291;
/// Host-controlled bots hold wire ids from here up.
const BOT_A: u16 = mm2_app::cnr_bots::NET_BOT_PLAYER_BASE;
const BOT_B: u16 = BOT_A + 1;

/// The retail Cops & Robbers host, bots to be set with `session
/// bots=2` before it starts.
///
/// Every process runs `--paced`: an unpaced headless sim steps about 5x
/// faster than wall clock and feeds the wall-clock rate controller 5x the
/// demand a real 60 Hz player produces, so the lane budget is judged
/// against traffic no player sends.
fn host_proc(
    retail: &std::path::Path,
    city: &str,
    variant: &str,
    limit: &str,
    extra: &[&str],
) -> Proc {
    let mut args = host_args(retail, HOST_FRAME_CEILING);
    args.retain(|a| a != "--dev-world");
    let at = args.iter().position(|a| a == "--seed").unwrap();
    args[at + 1] = SEED.to_string();
    args.extend(
        [
            "--city",
            city,
            "--cnr",
            variant,
            "--cnr-limit",
            limit,
            "--paced",
        ]
        .into_iter()
        .chain(extra.iter().copied())
        .map(String::from),
    );
    Proc::spawn(MM2_EXE, &args)
}

/// `cbots=<tick>:<id>@x:z/…;…` as `tick → [(id, x, z)]`.
fn bot_poses(line: &str) -> std::collections::BTreeMap<u64, Vec<(u16, f32, f32)>> {
    field(line, "cbots")
        .split(';')
        .map(|group| {
            let (tick, bots) = group.split_once(':').expect("tick:bots");
            let bots = bots
                .split('/')
                .map(|b| {
                    let (id, xz) = b.split_once('@').expect("id@x:z");
                    let (x, z) = xz.split_once(':').expect("x:z");
                    (id.parse().unwrap(), x.parse().unwrap(), z.parse().unwrap())
                })
                .collect();
            (tick.parse().unwrap(), bots)
        })
        .collect()
}

/// The `key=` cell of the record's `cnr=` field (`pts=`, `sides=`, …).
fn cnr_text<'a>(line: &'a str, key: &str) -> &'a str {
    field(line, "cnr")
        .split(',')
        .find_map(|c| c.strip_prefix(key))
        .unwrap_or_else(|| panic!("no {key} in {line}"))
}

/// Host and client sampled the same session ticks and saw the bots at
/// the same places there, within the wire's own precision.
fn assert_bots_agree(host: &str, client: &str) -> usize {
    let (h, c) = (bot_poses(host), bot_poses(client));
    let mut shared = 0;
    for (tick, theirs) in &c {
        let Some(ours) = h.get(tick) else { continue };
        shared += 1;
        assert_eq!(ours.len(), theirs.len(), "tick {tick}: {ours:?} {theirs:?}");
        for (a, b) in ours.iter().zip(theirs) {
            assert_eq!(a.0, b.0, "tick {tick}: bot identities differ");
            assert!(
                (a.1 - b.1).abs() < 0.5 && (a.2 - b.2).abs() < 0.5,
                "tick {tick}: bot {} is at {:?} on the host, {:?} on the client",
                a.0,
                (a.1, a.2),
                (b.1, b.2)
            );
        }
    }
    shared
}

/// One match to its results screen, read on both machines: the host
/// with two lobby bots, the client parked.
fn play_to_results(city: &str, variant: &str) -> (String, String) {
    let Some((retail, _slot)) = support::retail_slot() else {
        panic!("callers check MM2_RETAIL");
    };
    let mut host = host_proc(
        &retail,
        city,
        variant,
        "5m",
        &["--until-results", "--deadline", "900"],
    );
    let addr = listening_addr(&host);
    host.cmd(&format!("session bots=2 seed={SEED}"));
    host.until("event=session_changed");
    let mut client_args = join_args(&retail, addr, "bob", HOST_FRAME_CEILING);
    client_args.extend(
        [
            "--paced",
            "--parked",
            "--until-results",
            "--deadline",
            "900",
        ]
        .into_iter()
        .map(String::from),
    );
    let client = Proc::spawn(MM2_EXE, &client_args);
    start_when_ready(&mut host, 1);

    let crec = client.until_within("smoke=headless-physics", BOUND);
    let hrec = host.until_within("smoke=headless-physics", BOUND);
    eprintln!("host   {hrec}\nclient {crec}");
    assert!(client.wait().success(), "the client did not exit cleanly");
    assert!(host.wait().success(), "the host did not exit cleanly");
    (hrec, crec)
}

/// What the host and the client must read identically at the results
/// screen, however the match went.
fn assert_machines_agree(host: &str, client: &str) {
    for rec in [host, client] {
        assert_eq!(field(rec, "status"), "pass", "{rec}");
        assert_eq!(field(rec, "phase"), "results", "{rec}");
        assert_eq!(field(rec, "stop"), "results", "{rec}");
        assert_eq!(
            cnr_cell(rec, "seats"),
            Some(4),
            "human, client, 2 bots: {rec}"
        );
        assert_eq!(cnr_cell(rec, "dec"), Some(1), "{rec}");
        assert_eq!(cnr_cell(rec, "ref"), Some(0), "{rec}");
    }
    assert!(cnr_cell(client, "landed").unwrap() > 0, "{client}");
    // The host's lane held its rate ceiling all match (#2085, #2101): a
    // collapse here means the controller read the sim's demand as
    // congestion.
    let lane = field(host, "lane");
    let mut cells = lane.split('/');
    let (min, ceiling, cuts) = (
        cells.next().unwrap().parse::<u32>().unwrap(),
        cells.next().unwrap().parse::<u32>().unwrap(),
        cells
            .next()
            .unwrap()
            .trim_end_matches('c')
            .parse::<u64>()
            .unwrap(),
    );
    assert!(
        min == ceiling && cuts == 0,
        "host lane left its ceiling: {host}"
    );
    // The same four seats, bots under the reserved ids, on the same
    // teams, with the same points and the same gold holder and winner.
    for key in ["sides=", "pts=", "gold="] {
        assert_eq!(cnr_text(host, key), cnr_text(client, key), "{key}");
    }
    let ids = |rec: &str| -> Vec<u16> {
        let mut ids: Vec<u16> = cnr_text(rec, "sides=")
            .split('/')
            .map(|s| s.split(':').next().unwrap().parse().unwrap())
            .collect();
        ids.sort_unstable();
        ids
    };
    assert_eq!(ids(host), vec![0, 1, BOT_A, BOT_B], "{host}");
    assert_eq!(ids(client), ids(host));
    assert_eq!(
        cnr_text(host, "win="),
        cnr_text(client, "win="),
        "the verdicts differ"
    );
    let shared = assert_bots_agree(host, client);
    assert!(shared >= 3, "only {shared} sampled ticks in common");
}

fn retail_or_skip() -> bool {
    let set = support::retail_slot().is_some();
    if !set {
        eprintln!("skipped: MM2_RETAIL is not set");
    }
    set
}

/// Free-for-all: every seat, bots included, scores alone. The bots
/// reach the gold and score on this seed, so the equal `pts=`, `gold=`
/// and `win=` cells compare something other than zeros.
#[test]
fn a_host_and_a_client_read_the_same_free_for_all_with_bots() {
    if !retail_or_skip() {
        return;
    }
    let (host, client) = play_to_results("sf", "ffa");
    assert_machines_agree(&host, &client);
    assert_eq!(cnr_cell(&client, "solo"), Some(4), "{client}");
    let pts = cnr_text(&client, "pts=");
    assert!(
        pts.split('/').any(|p| {
            let (id, score) = p.split_once(':').unwrap();
            id.parse::<u16>().unwrap() >= BOT_A && score != "0"
        }),
        "no bot scored by taking the gold: {pts}"
    );
}

/// The same free-for-all on the retail London map: the bots are fielded
/// on London's street graph and both machines read the same match. No
/// score is required of the bots here — London's site pool and streets
/// are not the ones the `sf` seed was measured on.
#[test]
fn a_host_and_a_client_read_the_same_london_match_with_bots() {
    if !retail_or_skip() {
        return;
    }
    let (host, client) = play_to_results("london", "ffa");
    assert_machines_agree(&host, &client);
    assert_eq!(field(&client, "world"), "city/london.psdl", "{client}");
}

/// Cops & Robbers proper: the host fields the bots on teams and the
/// client's replica seats the same split.
#[test]
fn a_host_and_a_client_read_the_same_teams_with_bots() {
    if !retail_or_skip() {
        return;
    }
    let (host, client) = play_to_results("sf", "cops");
    assert_machines_agree(&host, &client);
    for cell in ["rob", "cop"] {
        assert!(cnr_cell(&client, cell).unwrap() >= 1, "{cell}: {client}");
    }
    assert_eq!(
        cnr_cell(&client, "rob").unwrap() + cnr_cell(&client, "cop").unwrap(),
        4,
        "{client}"
    );
}

/// A rematch on the same two processes: round 1 is cancelled once the
/// bots drive, `start` mints round 2, and the client's replica fields
/// the same two bots again, at the places the host drives them.
#[test]
fn a_rematch_fields_the_same_bots_on_both_machines() {
    if !retail_or_skip() {
        return;
    }
    let (retail, _slot) = support::retail_slot().unwrap();
    let mut host = host_proc(&retail, "sf", "ffa", "5m", &[]);
    let addr = listening_addr(&host);
    host.cmd(&format!("session bots=2 seed={SEED}"));
    host.until("event=session_changed");
    let mut client_args = join_args(&retail, addr, "bob", HOST_FRAME_CEILING);
    client_args.extend(
        [
            "--paced",
            "--parked",
            "--until-snap-tick",
            "1500",
            "--deadline",
            "900",
        ]
        .into_iter()
        .map(String::from),
    );
    let client = Proc::spawn(MM2_EXE, &client_args);
    start_when_ready(&mut host, 1);
    // The host's log names each round's bots as it fields them (the
    // tracing fields carry colour codes, so match the message alone).
    host.until_within("cops and robbers bots spawned", BOUND);

    host.cmd("cancel");
    host.until("event=cancelled generation=1");
    host.until("ready=true");
    host.cmd("start");
    host.until_within("cops and robbers bots spawned", BOUND);

    let crec = client.until_within("smoke=headless-physics", BOUND);
    assert!(client.wait().success(), "the client did not exit cleanly");
    host.cmd("quit");
    let hrec = host.until_within("smoke=headless-physics", BOUND);
    eprintln!("host   {hrec}\nclient {crec}");

    assert_eq!(field(&crec, "stop"), "snap-tick", "{crec}");
    assert_eq!(field(&crec, "mp"), "gen2", "round 2 never ran: {crec}");
    for seat in [BOT_A, BOT_B] {
        // Only the client still holds the match: the host's `quit`
        // record comes after its session was torn down.
        assert!(
            field(&crec, "seats")
                .split('/')
                .any(|s| s.starts_with(&format!("{seat}:"))),
            "bot {seat} is not seated: {crec}"
        );
    }
    assert_eq!(cnr_cell(&crec, "seats"), Some(4), "{crec}");
    // The client's samples are the rematch's own: they restart with
    // the new generation's clock, and name the same two bots.
    let poses = bot_poses(&crec);
    assert!(!poses.is_empty(), "{crec}");
    for (tick, bots) in &poses {
        assert!(*tick <= 1500 + 60, "round 1's samples leaked: {crec}");
        let ids: Vec<u16> = bots.iter().map(|b| b.0).collect();
        assert_eq!(ids, vec![BOT_A, BOT_B], "tick {tick}");
    }
}
