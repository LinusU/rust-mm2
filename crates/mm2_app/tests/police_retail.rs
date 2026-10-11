//! F20 original-content validation: a long chased Cruise soak in both
//! retail cities (F20-AC04). Opt-in (`MM2_RETAIL=<dir>`); skipped
//! without the operator's install, and reports what it ran.
//!
//! The soak drives the production binary headlessly in free-roam
//! Cruise, where the city's authored `roam` lineup stands at its posts
//! (F20-B.3b), and reads the police fields of the smoke record:
//!
//! - `pol=<spawned>/<authored>` — every authored cop became a car and
//!   none leaked over the run;
//! - `pur=<committed>/<gave_up>/<peak>[,…]` — a chase really started on
//!   real streets and the peak simultaneous pursuer count never passed
//!   the policy cap (the AC04 claim);
//! - `pur=…,e<N>` / `…,t<N>` — the fleet's bounded reverse-and-turn
//!   stuck escapes (detection + both manoeuvre halves) and deliberate
//!   turn-arounds (both halves, no detection), each bounded by its own
//!   window so the total is rate-bounded (the stuck-recovery leg);
//! - `vsk=<armed>/<detections>/<recovered>` — the whole-fleet stuck
//!   pipeline never recovers more episodes than it detected, so no
//!   detection loops.
//!
//! Each `--spawn` is a fixed pose ~24 m in front of an authored
//! `[Police]` row, so the held driver pulls away with a cop already in
//! notice range and the run reproduces the same chase. It does not
//! judge handling or the look of the chase.

use std::path::{Path, PathBuf};
use std::process::Command;

use mm2_game::PursuitPolicy;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// 60 s of game time at 60 Hz — long enough for a chase to commit, lose
/// contact, stand down and re-arm.
const FRAMES: u64 = 3600;

/// A wedged cop can only escape again once it has sat still `STUCK_FRAMES`
/// and then completed both halves of the reverse-and-turn manoeuvre — the
/// same 207-frame floor (90 + 72 + 45) the synthetic long-chase test
/// (`tests/police.rs`) sizes its per-cop bound with.
const ESCAPE_WINDOW: u64 = 90 + 72 + 45;

/// A deliberate turn-around needs only the two manoeuvre halves (the aim
/// is already behind a slow car, so no stuck-detection window precedes
/// it) — 117 frames between starts.
const TURNAROUND_WINDOW: u64 = 72 + 45;

/// `(city, --spawn)`. The spawn stands ~24 m in front of an authored
/// cop (`vpcop 58.99 0 71.63` sf, `vpcop 752.31 0 96.48` london), facing
/// the cop; the held driver drives straight at it, so a chase commits
/// within the reaction window on both cities.
const LEGS: [(&str, &str); 2] = [("sf", "59,1.5,95,0"), ("london", "752.3,1.5,120.5,0")];

fn field<'a>(record: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!(" {key}=");
    let at = record.find(&pat)? + pat.len();
    record[at..].split_whitespace().next()
}

/// A `,<letter><n>` suffix of the `pur=` value, or `0` when absent (the
/// record omits a counter while it is zero).
fn suffix(pur: &str, letter: char) -> u64 {
    pur.split(',')
        .filter_map(|tok| tok.strip_prefix(letter)?.parse::<u64>().ok())
        .next()
        .unwrap_or(0)
}

fn chase(retail: &Path, city: &str, spawn: &str) -> String {
    let out = Command::new(MM2_EXE)
        .args(["--headless", "--mm2-path"])
        .arg(retail)
        .args(["--city", city])
        .arg(format!("--spawn={spawn}"))
        .args(["--frames", &FRAMES.to_string()])
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn mm2");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{city} chase failed:\n{log}");
    log.lines()
        .find(|l| l.starts_with("smoke=headless-physics"))
        .unwrap_or_else(|| panic!("{city}: no smoke record:\n{log}"))
        .to_string()
}

#[test]
fn retail_cruise_keeps_pursuers_capped_and_cop_recovery_bounded_in_both_cities() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(PathBuf::from) else {
        eprintln!(
            "skipped: MM2_RETAIL is not set; retail long-chase soak NOT run \
             (set MM2_RETAIL to the Midtown Madness 2 install)"
        );
        return;
    };
    let cap = PursuitPolicy::default().max_pursuers as u64;
    for (city, spawn) in LEGS {
        let record = chase(&retail, city, spawn);
        eprintln!("{city} (--spawn={spawn}): {record}");

        assert_eq!(field(&record, "status"), Some("pass"), "{city}: {record}");
        assert_eq!(field(&record, "finite"), Some("true"), "{city}: {record}");

        // Every authored cop is fielded and none leaks over the run.
        let (spawned, authored) = field(&record, "pol")
            .and_then(|p| p.split_once('/'))
            .unwrap_or_else(|| panic!("{city}: no pol= field:\n{record}"));
        let (spawned, authored): (u64, u64) = (
            spawned.parse().unwrap(),
            authored.split(',').next().unwrap().parse().unwrap(),
        );
        assert!(
            authored > 0,
            "{city}: the city authored no police:\n{record}"
        );
        assert_eq!(
            spawned, authored,
            "{city}: a cop was lost or duplicated:\n{record}"
        );

        // A real chase on real streets, capped by the shared policy.
        let pur = field(&record, "pur").unwrap_or_else(|| panic!("{city}: no pur=:\n{record}"));
        let head: Vec<u64> = pur
            .split(',')
            .next()
            .unwrap()
            .split('/')
            .map(|n| n.parse().unwrap())
            .collect();
        let (committed, gave_up, peak) = (head[0], head[1], head[2]);
        assert!(committed >= 1, "{city}: no chase committed:\n{record}");
        assert!(
            peak >= 1,
            "{city}: a chase committed but nothing pursued:\n{record}"
        );
        assert!(
            peak <= cap,
            "{city}: {peak} pursuers exceeded the cap of {cap}:\n{record}"
        );
        assert!(
            peak <= committed && gave_up <= committed,
            "{city}: counts inconsistent with the chase state machine:\n{record}"
        );

        // Cop stuck recovery stays bounded: every escape needs the full
        // detection + manoeuvre window and every turn-around the two
        // manoeuvre halves, so each fleet total is bounded by the cop
        // count times how many windows fit in the run. The two are
        // bounded apart because their floors differ.
        let escapes = suffix(pur, 'e');
        let turnarounds = suffix(pur, 't');
        let escape_bound = authored * (FRAMES / ESCAPE_WINDOW + 1);
        assert!(
            escapes <= escape_bound,
            "{city}: {escapes} escapes over the {escape_bound} their windows allow:\n{record}"
        );
        let turnaround_bound = authored * (FRAMES / TURNAROUND_WINDOW + 1);
        assert!(
            turnarounds <= turnaround_bound,
            "{city}: {turnarounds} turn-arounds over the {turnaround_bound} their windows \
             allow:\n{record}"
        );

        // The whole-fleet stuck pipeline never resolves more episodes than
        // it detected, so no detection loops. `vsk=<armed>a/<detections>d/<recovered>r`.
        let vsk = field(&record, "vsk").unwrap_or_else(|| panic!("{city}: no vsk=:\n{record}"));
        let vsk: Vec<u64> = vsk
            .split('/')
            .map(|n| {
                n.trim_end_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .unwrap()
            })
            .collect();
        assert!(
            vsk[2] <= vsk[1],
            "{city}: recovered {} of {} detections:\n{record}",
            vsk[2],
            vsk[1]
        );
    }
}
