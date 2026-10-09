//! F15-C original-content validation: headless soaks of the production
//! binary racing the authored opponent lineups on retail events. Opt-in
//! (`MM2_RETAIL=<dir>`); skipped without the operator's install, and
//! reports what it ran.
//!
//! The soak reads the smoke record's `opp=`, `opps=` and `finite=`
//! fields. It asserts F15-AC01/AC02/AC03/AC05 against the real authored
//! rosters and routes: the authored lineup spawns, opponents earn gates
//! through the swept triggers, no stationary spell outlives the bounded
//! recovery, and every vehicle state stays finite. It does not judge
//! how the field drives or whether the outcome matches the retail game.

use std::process::Command;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// Simulated frames per leg.
const FRAMES: &str = "3000";

/// The stuck window an opponent may spend before recovery (900 driving
/// frames, DSN-14) plus the escape/landing margin; a longer spell means
/// recovery did not fire.
const STUCK_BOUND: u32 = 1500;

fn field<'a>(record: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!(" {key}=");
    let at = record.find(&pat)? + pat.len();
    record[at..].split_whitespace().next()
}

fn soak(retail: &std::path::Path, city: &str, event: &str) -> String {
    let out = Command::new(MM2_EXE)
        .args(["--headless", "--mm2-path"])
        .arg(retail)
        .args(["--city", city, "--event", event, "--frames", FRAMES])
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn mm2");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{city} {event} soak failed:\n{log}");
    log.lines()
        .find(|l| l.starts_with("smoke=headless-physics"))
        .unwrap_or_else(|| panic!("{city} {event}: no smoke record:\n{log}"))
        .to_string()
}

/// One parsed `opps=` row: `<slot>:<vehicle>/<n>c[/<lap>l][/F|/T]...`.
struct Row {
    slot: usize,
    vehicle: String,
    cleared: usize,
    stuck: u32,
}

fn rows(record: &str) -> Vec<Row> {
    field(record, "opps")
        .unwrap_or_else(|| panic!("record lacks `opps=`:\n{record}"))
        .split(',')
        .map(|row| {
            let (slot, rest) = row.split_once(':').expect("slot:");
            let mut parts = rest.split('/');
            let vehicle = parts.next().expect("vehicle").to_string();
            let mut cleared = None;
            let mut stuck = 0;
            for p in parts {
                if let Some(n) = p.strip_suffix('c') {
                    cleared = n.parse().ok();
                } else if let Some(n) = p.strip_suffix('w') {
                    stuck = n.parse().expect("stuck frames");
                }
            }
            Row {
                slot: slot.parse().expect("slot index"),
                vehicle,
                cleared: cleared.unwrap_or_else(|| panic!("row lacks `<n>c`: {row}")),
                stuck,
            }
        })
        .collect()
}

#[test]
fn retail_events_race_the_authored_lineup_finitely_with_bounded_stuck_spells() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; retail opponent soak NOT run");
        return;
    };
    for (city, event) in [
        ("sf", "checkpoint:0"),
        ("london", "checkpoint:0"),
        ("sf", "circuit:0"),
    ] {
        let record = soak(&retail, city, event);
        eprintln!("{city} {event}: {record}");
        let leg = format!("{city} {event}");
        assert_eq!(field(&record, "status"), Some("pass"), "{leg}: {record}");
        assert_eq!(field(&record, "finite"), Some("true"), "{leg}: {record}");

        // AC01: the authored lineup spawned — `opp=<resolved>/<spawned>`
        // and one `opps=` row per spawned opponent, in distinct slots.
        let (_, spawned) = field(&record, "opp")
            .and_then(|o| o.split_once('/'))
            .unwrap_or_else(|| panic!("{leg}: no opp= field:\n{record}"));
        let spawned: usize = spawned.parse().unwrap();
        let rows = rows(&record);
        assert!(spawned > 0, "{leg}: no opponents spawned:\n{record}");
        assert_eq!(rows.len(), spawned, "{leg}: {record}");
        let mut slots: Vec<usize> = rows.iter().map(|r| r.slot).collect();
        slots.dedup();
        assert_eq!(slots.len(), spawned, "{leg}: duplicate slots:\n{record}");
        assert!(rows.iter().all(|r| !r.vehicle.is_empty()), "{leg}");

        // AC02: opponents earn gates through the real triggers.
        assert!(
            rows.iter().any(|r| r.cleared > 0),
            "{leg}: no opponent cleared a gate:\n{record}"
        );

        // AC03: no opponent sits stationary past the recovery bound.
        for r in &rows {
            assert!(
                r.stuck <= STUCK_BOUND,
                "{leg}: slot {} ({}) stuck {} frames:\n{record}",
                r.slot,
                r.vehicle,
                r.stuck
            );
        }
    }
}
