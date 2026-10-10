//! F19 original-content validation: a headless soak of the production
//! binary in both retail cities with the sidewalk crowd fielded.
//! Opt-in (`MM2_RETAIL=<dir>`); skipped without the operator's
//! install, and reports what it ran.
//!
//! The soak reads the smoke record's crowd fields (`peds=`,
//! `phop=`, `pwary=`, `pdive=`, `prx=`) and the run's `finite=`
//! verdict, so it asserts the F19-AC05 contract — a fixed-seed crowded
//! scene stays finite and inside the actor/animation budgets — and the
//! F19-AC03 movement half (walkers really walked, every loaded
//! archetype carries the reaction states) against the real authored
//! sidewalk networks and archetypes. It does not judge the look of the
//! figures or a reaction; that is F19-C's rendered leg.

use std::process::Command;

const MM2_EXE: &str = env!("CARGO_BIN_EXE_mm2");

/// Simulated frames per city: 60 s of wall-clock game time at 60 Hz,
/// long enough for walkers to reach kerb corners, recycle out of the
/// driving player's bubble and be refilled.
const FRAMES: &str = "3600";

/// The frame-budget ceiling: an unaccelerated headless soak measures
/// ~8–18 ms/frame with the crowd fielded (see
/// `docs/research/pedanim.md`), so this generous bound only fails a
/// gross per-frame blow-up — it is not a performance claim.
const FRAME_MS_CEILING: f64 = 250.0;

/// `WalkPolicy::max_active` — the crowd's actor budget. The density
/// target must stay inside it and the live count inside the target.
const MAX_PED_ACTORS: usize = 64;

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

/// What one city's crowd soak measured: the smoke record plus the
/// wall-clock seconds the whole run took (the frame-budget evidence).
fn soak(retail: &std::path::Path, city: &str) -> (String, f64) {
    let started = std::time::Instant::now();
    let out = Command::new(MM2_EXE)
        .args(["--headless", "--mm2-path"])
        .arg(retail)
        .args(["--city", city, "--frames", FRAMES])
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn mm2");
    let elapsed = started.elapsed().as_secs_f64();
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{city} soak failed:\n{log}");
    let record = log
        .lines()
        .find(|l| l.starts_with("smoke=headless-physics"))
        .unwrap_or_else(|| panic!("{city}: no smoke record:\n{log}"))
        .to_string();
    (record, elapsed)
}

#[test]
fn retail_crowd_soak_walks_the_real_sidewalks_stays_finite_and_bounded() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set; retail pedestrian soak NOT run");
        return;
    };
    for city in ["london", "sf"] {
        let (record, elapsed) = soak(&retail, city);
        let frames: f64 = FRAMES.parse().unwrap();
        let frame_ms = elapsed / frames * 1000.0;
        eprintln!("{city}: {record}");
        eprintln!(
            "{city}: {elapsed:.1}s wall for {FRAMES} frames \
             ({frame_ms:.2} ms/frame, unaccelerated headless)"
        );
        assert!(
            frame_ms < FRAME_MS_CEILING,
            "{city}: {frame_ms:.2} ms/frame with the crowd exceeds \
             the {FRAME_MS_CEILING} ms ceiling:\n{record}"
        );
        assert_eq!(field(&record, "finite"), Some("true"), "{city}: {record}");

        // The crowd exists, walks and stays inside its budgets.
        let (live, target) = field(&record, "peds")
            .and_then(|p| p.split_once('/'))
            .unwrap_or_else(|| panic!("{city}: no peds= field:\n{record}"));
        let (live, target): (usize, usize) = (live.parse().unwrap(), target.parse().unwrap());
        assert!(target > 0, "{city}: density target is zero:\n{record}");
        assert!(
            target <= MAX_PED_ACTORS,
            "{city}: target {target} above the actor budget:\n{record}"
        );
        assert!(live <= target, "{city}: {live} live > target {target}");
        // Real sidewalk curves carried real walkers that rounded real
        // kerb corners — the F19-AC04 movement half on retail data.
        assert!(
            num(&record, "psp") > 0,
            "{city}: nothing spawned:\n{record}"
        );
        assert!(
            num(&record, "phop") > 0,
            "{city}: no corner hops:\n{record}"
        );
        // Every stock archetype must carry the reaction states, so the
        // whole crowd can react (the F19-AC03 precondition).
        let prx =
            field(&record, "prx").unwrap_or_else(|| panic!("{city}: no prx= field:\n{record}"));
        let (reacting, loaded) = prx
            .split_once('/')
            .unwrap_or_else(|| panic!("{city}: prx={prx} is not `<n>/<m>`:\n{record}"));
        let (reacting, loaded): (usize, usize) =
            (reacting.parse().unwrap(), loaded.parse().unwrap());
        assert!(loaded > 0, "{city}: no archetypes loaded:\n{record}");
        assert_eq!(
            reacting, loaded,
            "{city}: an archetype lacks reaction states:\n{record}"
        );
        // The reaction counters must be present and numeric whether or
        // this soak provoked one — the field contract, not a claim
        // that ambient traffic scared anyone today.
        num(&record, "pwary");
        num(&record, "pdive");
        num(&record, "prej");
    }
}
