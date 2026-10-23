//! NETPLAY-05J: the host's finalized per-seat commands reach each client
//! over the real process boundary. Three `mm2` processes (the host and two
//! clients on the UDP datagram lane) run one dev-world session under the
//! fixed-tick timeline. Each client's record reports, per remote seat, the
//! `RemoteCommands` ticks it stored (`rcmd=<seat>:<stored>s/<consumed>c/
//! <conflicts>x`): a client must hold the host's and the *other* client's
//! commands and never its own, with no tick stored twice differently.
//!
//! Evidence level: synthetic integration (real processes over loopback,
//! headless, dev world).

use std::time::Duration;

use crate::net_drive::{
    HOST_FRAME_CEILING, field, host_args, join_args, listening_addr, until_live,
};
use crate::support::Proc;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");
const STEP: Duration = Duration::from_secs(60);
const DEADLINE: Duration = Duration::from_secs(150);
/// Simulated seconds the clients play before they stop.
const PLAY_SECS: u64 = 12;

/// `(stored, consumed, conflicts)` of `seat` in a record's `rcmd=` field.
fn cells(record: &str, seat: u16) -> Option<(u64, u64, u64)> {
    let rcmd = record
        .split_whitespace()
        .find_map(|t| t.strip_prefix("rcmd="))?;
    rcmd.split(',').find_map(|cell| {
        let (id, rest) = cell.split_once(':')?;
        if id.parse::<u16>().ok()? != seat {
            return None;
        }
        let mut n = rest
            .split('/')
            .map(|v| v.trim_end_matches(char::is_alphabetic).parse::<u64>());
        Some((n.next()?.ok()?, n.next()?.ok()?, n.next()?.ok()?))
    })
}

#[test]
fn a_client_stores_the_other_seats_commands_from_the_host_over_the_lane() {
    let install = tempfile::tempdir().unwrap();
    let flags = |mut args: Vec<String>| {
        args.extend(["--paced", "--netplay-timeline"].map(String::from));
        args
    };
    let mut host = Proc::spawn(
        MM2_EXE,
        &flags(host_args(install.path(), HOST_FRAME_CEILING)),
    );
    let addr = listening_addr(&host);
    let stop = (PLAY_SECS * mm2_game::RACE_TICK_HZ as u64).to_string();
    let client = |who: &str| {
        let mut args = flags(join_args(install.path(), addr, who, HOST_FRAME_CEILING));
        args.extend(["--until-snap-tick".to_string(), stop.clone()]);
        if who == "alice" {
            // Stay seated until bob leaves, as `net_contact` does.
            args.push("--hold-until-peer-left".into());
        }
        args.extend(["--deadline".to_string(), DEADLINE.as_secs().to_string()]);
        args
    };
    let alice = Proc::spawn(MM2_EXE, &client("alice"));
    host.until_within("ready=true", STEP);
    let bob = Proc::spawn(MM2_EXE, &client("bob"));
    host.until_within("ready=true", STEP);
    host.cmd("start");
    host.until_within("event=started generation=1", STEP);
    until_live(&alice);
    until_live(&bob);

    let bound = DEADLINE + Duration::from_secs(60);
    let alice_rec = alice.until_within("smoke=headless-physics", bound);
    let bob_rec = bob.until_within("smoke=headless-physics", bound);
    alice.wait_success("alice", &alice_rec);
    bob.wait_success("bob", &bob_rec);
    host.cmd("quit");
    let host_rec = host.until_within("smoke=headless-physics", STEP);
    host.wait_success("host", &host_rec);

    // The roster mints ids upward from 1 in join order; the host is 0.
    for (who, rec, other, own) in [("alice", &alice_rec, 2, 1), ("bob", &bob_rec, 1, 2)] {
        assert_eq!(field(rec, "status"), "pass", "{who}: {rec}");
        for seat in [0, other] {
            let (stored, _, conflicts) = cells(rec, seat)
                .unwrap_or_else(|| panic!("{who} stored nothing for seat {seat}: {rec}"));
            assert!(
                stored >= 60,
                "{who}: seat {seat} stored only {stored}: {rec}"
            );
            assert_eq!(conflicts, 0, "{who}: seat {seat} conflicted: {rec}");
        }
        assert!(
            cells(rec, own).is_none(),
            "{who} was sent its own seat's commands: {rec}"
        );
    }
}
