//! Integration tests for the F00-C evidence smokes.
//!
//! `headless_smoke` is the same runner the `mm2 --headless` binary path
//! uses, so these exercise the production smoke — dev world without MM2
//! data, explicit failure for a requested-but-missing city, and report
//! records whose kinds stay distinguishable.

use mm2_app::smoke::{self, SmokeRecord, SmokeStatus};
use mm2_assets::Vfs;
use mm2_game::{SessionConfig, WorldMode};
use mm2_vehicle::VehicleConfig;

/// The dev world must start with no original data at all — an empty VFS —
/// and the car must settle and drive.
#[test]
fn dev_world_headless_smoke_passes_without_mm2_data() {
    let vfs = Vfs::new();
    let rec = smoke::headless_smoke(
        &SessionConfig::default(),
        vfs,
        None,
        &VehicleConfig::default(),
        600,
        smoke::Driver::Hold,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Pass,
        "expected pass, got: {}",
        rec.line()
    );
    assert!(rec.line().starts_with("smoke=headless-physics"));
    assert!(rec.line().contains("world=dev-world"));
    // The session clock ran on the fixed step: ~2 ticks per 60 Hz update
    // at the 120 Hz timestep, minus the clock priming update (F01-AC03).
    let ticks: u64 = rec
        .line()
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("ticks=").and_then(|v| v.parse().ok()))
        .expect("record reports ticks=");
    assert!(
        (1100..=1200).contains(&ticks),
        "600 updates should produce ~1200 fixed ticks, got {ticks}"
    );
}

/// A specifically requested city that the VFS cannot provide must report
/// an explicit failure — never a pass and never a silent dev world.
#[test]
fn requested_missing_city_is_an_explicit_failure() {
    let vfs = Vfs::new();
    let config = SessionConfig {
        world: WorldMode::City {
            psdl: "city/atlantis.psdl".into(),
        },
        ..SessionConfig::default()
    };
    let rec = smoke::headless_smoke(
        &config,
        vfs,
        None,
        &VehicleConfig::default(),
        600,
        smoke::Driver::Hold,
    );
    assert_eq!(
        rec.status,
        SmokeStatus::Fail,
        "expected fail, got: {}",
        rec.line()
    );
    assert!(
        rec.line().contains("city/atlantis.psdl"),
        "record should name the missing logical path: {}",
        rec.line()
    );
}

/// AC05: a visual record and a headless-physics record must not be
/// confusable, and `unavailable` must stay distinct from `fail`.
#[test]
fn record_lines_distinguish_kinds_and_statuses() {
    let headless = SmokeRecord {
        kind: smoke::KIND_HEADLESS_PHYSICS,
        world: "dev-world".into(),
        status: SmokeStatus::Pass,
        detail: "updates=600".into(),
    };
    let visual = SmokeRecord {
        kind: smoke::KIND_VISUAL,
        world: "dev-world".into(),
        status: SmokeStatus::Pass,
        detail: "frames=done".into(),
    };
    assert!(headless.line().contains("smoke=headless-physics"));
    assert!(visual.line().contains("smoke=visual"));
    assert!(!visual.line().contains("smoke=headless"));

    let unavailable = SmokeRecord {
        status: SmokeStatus::Unavailable,
        ..visual.clone()
    };
    assert!(unavailable.line().contains("status=unavailable"));
    assert!(!unavailable.line().contains("status=fail"));

    // Exit codes: pass 0, fail 3, unavailable 4 — distinct, nonzero on
    // anything that is not a pass.
    assert_eq!(SmokeStatus::Pass.exit_code(), 0);
    assert_eq!(SmokeStatus::Fail.exit_code(), 3);
    assert_eq!(SmokeStatus::Unavailable.exit_code(), 4);
}
