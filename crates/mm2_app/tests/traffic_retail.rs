//! F10-C original-content validation: a headless soak of the production
//! binary in both retail cities. Opt-in (`MM2_RETAIL=<dir>`); skipped
//! without the operator's install, and reports what it ran.
//!
//! The soak reads the smoke record's ambient fields (`traf=`, `sp=`,
//! `rec=`, `stuck=`, `crx=` ...) and the run's `finite=` verdict, so it
//! asserts the F10-AC05 contract — finite states, bounded entity count,
//! stuck-car outcomes reported — against the real authored networks.
//! It does not judge handling or the look of the traffic.

use std::process::Command;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// Simulated frames per city: 60 s of wall-clock game time at 60 Hz,
/// long enough for cars to reach junctions and recycle.
const FRAMES: &str = "3600";

fn field<'a>(record: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!(" {key}=");
    let at = record.find(&pat)? + pat.len();
    record[at..].split_whitespace().next()
}

fn num(record: &str, key: &str) -> u64 {
    field(record, key)
        .unwrap_or_else(|| panic!("record lacks `{key}=`:\n{record}"))
        .parse()
        .unwrap_or_else(|_| panic!("`{key}=` is not a number:\n{record}"))
}

fn soak(retail: &std::path::Path, city: &str) -> String {
    let out = Command::new(MM2_EXE)
        .args(["--headless", "--mm2-path"])
        .arg(retail)
        .args(["--city", city, "--frames", FRAMES])
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn mm2");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{city} soak failed:\n{log}");
    log.lines()
        .find(|l| l.starts_with("smoke=headless-physics"))
        .unwrap_or_else(|| panic!("{city}: no smoke record:\n{log}"))
        .to_string()
}

#[test]
fn retail_soak_keeps_traffic_finite_and_bounded_in_both_cities() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; retail traffic soak NOT run");
        return;
    };
    for city in ["london", "sf"] {
        let record = soak(&retail, city);
        eprintln!("{city}: {record}");
        assert_eq!(field(&record, "finite"), Some("true"), "{city}: {record}");

        let (active, target) = field(&record, "traf")
            .and_then(|t| t.split_once('/'))
            .unwrap_or_else(|| panic!("{city}: no traf= field:\n{record}"));
        let (active, target): (u64, u64) = (active.parse().unwrap(), target.parse().unwrap());
        assert!(target > 0, "{city}: density target is zero:\n{record}");
        assert!(active <= target, "{city}: {active} cars > target {target}");
        assert!(num(&record, "sp") > 0, "{city}: nothing spawned:\n{record}");
        // The stuck outcome counter must be reported (present, numeric).
        num(&record, "stuck");
        num(&record, "crx");
    }
}
