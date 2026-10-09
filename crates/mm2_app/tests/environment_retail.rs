//! F18-C original-content validation: every retail weather × time-of-day
//! slot, in both cities, through the production binary's real session
//! path. Opt-in (`MM2_RETAIL=<dir>`); skipped without the operator's
//! install, and says so.
//!
//! Evidence level: original-content validation of the smoke record
//! (preset name, fog band, sky dome, precipitation counters, traction).
//! It proves each slot binds its own authored preset and that wetness and
//! precipitation follow `rainy` alone; it does not judge how any preset
//! looks (F18-AC05 captures are a separate, human-read artefact).

use std::collections::HashSet;
use std::process::Command;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// The exe-recovered live-drop ceiling (`mm2_game::PRECIP_MAX_LIVE`).
const MAX_LIVE: u64 = mm2_game::PRECIP_MAX_LIVE as u64;

const TIMES: [&str; 4] = ["morning", "noon", "evening", "night"];
const WEATHERS: [&str; 4] = ["clear", "cloudy", "foggy", "rainy"];

fn field<'a>(record: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!(" {key}=");
    let at = record.find(&pat)? + pat.len();
    record[at..].split_whitespace().next()
}

fn run(retail: &std::path::Path, city: &str, weather: u8, tod: u8, frames: u32) -> String {
    let out = Command::new(MM2_EXE)
        .args(["--headless", "--mm2-path"])
        .arg(retail)
        .args(["--city", city, "--weather", &weather.to_string()])
        .args(["--time-of-day", &tod.to_string()])
        .args(["--frames", &frames.to_string()])
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn mm2");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The record's own `status=` is a physics verdict (a short hold-driver
    // run never grounds), not a preset one: require the record and finite
    // state, not a zero exit.
    let record = log
        .lines()
        .find(|l| l.starts_with("smoke=headless-physics"))
        .unwrap_or_else(|| panic!("{city} w{weather} t{tod}: no smoke record:\n{log}"));
    assert_eq!(field(record, "finite"), Some("true"), "{record}");
    record.to_string()
}

/// `ppt=rain:<e>e/<x>x[+<c>c+<l>l+<u>u]` → (emitted, expired, covered, landed).
fn ppt(record: &str) -> Option<(String, u64, u64)> {
    let v = field(record, "ppt")?;
    let (name, counts) = v.split_once(':')?;
    let num = |suffix: char| -> u64 {
        counts
            .split(['/', '+'])
            .find_map(|p| p.strip_suffix(suffix)?.parse().ok())
            .unwrap_or(0)
    };
    Some((name.to_string(), num('e'), num('x') + num('l')))
}

#[test]
fn every_retail_preset_binds_distinctly_and_weather_alone_wets_the_road() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; retail weather/time matrix NOT run");
        return;
    };
    for city in ["london", "sf"] {
        let mut names = HashSet::new();
        let mut looks = HashSet::new();
        for tod in 0..4u8 {
            for weather in 0..4u8 {
                let slot = tod * 4 + weather;
                let rainy = weather == 3;
                // Rain needs time to spew and expire; dry slots only need to bind.
                let rec = run(&retail, city, weather, tod, if rainy { 300 } else { 30 });
                let ctx = format!("{city} slot {slot}: {rec}");

                // AC01/AC06: the slot's own authored preset, never the fallback.
                let want = format!(
                    "lt{slot:02}({}-{})",
                    WEATHERS[weather as usize], TIMES[tod as usize]
                );
                assert_eq!(field(&rec, "env"), Some(want.as_str()), "{ctx}");
                names.insert(want);

                // Fog and sky bound from authored rows, not absent.
                let fog = field(&rec, "fog").unwrap_or("none");
                let sky = field(&rec, "sky").unwrap_or("none");
                assert_ne!(fog, "none", "{ctx}");
                assert_ne!(sky, "none", "{ctx}");
                looks.insert((fog.to_string(), sky.to_string()));

                // AC02: traction changes under rain only; dry slots carry no
                // wetness field at all (unchanged dry-state behaviour).
                // AC03: precipitation is bound only for rain, and bounded.
                if rainy {
                    assert_eq!(field(&rec, "traction"), Some("0.8"), "{ctx}");
                    assert_eq!(field(&rec, "surf"), Some("wet"), "{ctx}");
                    let (name, emitted, gone) =
                        ppt(&rec).unwrap_or_else(|| panic!("no ppt on rain: {ctx}"));
                    assert_eq!(name, "rain", "{ctx}");
                    assert!(emitted > 0, "rain emitted nothing: {ctx}");
                    let live = emitted - gone.min(emitted);
                    assert!(live <= MAX_LIVE, "live {live} over bound: {ctx}");
                } else {
                    assert_eq!(field(&rec, "traction"), None, "{ctx}");
                    assert_eq!(field(&rec, "ppt"), None, "{ctx}");
                }
            }
        }
        assert_eq!(names.len(), 16, "{city}: presets are not all distinct");
        // The (fog, sky) pair is a coarse proxy for the look; it must
        // separate more than just the weather rows.
        assert!(
            looks.len() >= 8,
            "{city}: only {} distinct fog/sky looks over 16 slots: {looks:?}",
            looks.len()
        );
        eprintln!("{city}: 16/16 slots bound, {} fog/sky looks", looks.len());
    }
}

/// F18-AC06: an out-of-range selector is a usage error, never a silent
/// map to clear noon.
#[test]
fn out_of_range_selectors_are_rejected_not_defaulted() {
    for args in [["--weather", "4"], ["--time-of-day", "4"]] {
        let out = Command::new(MM2_EXE)
            .arg("--headless")
            .args(args)
            .env_remove("RUST_LOG")
            .output()
            .expect("spawn mm2");
        assert_eq!(out.status.code(), Some(2), "{args:?} must exit 2");
    }
}
