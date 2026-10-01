//! `net::check_session` — the client-side session-content gate
//! `mm2-join` runs on every advertised session blob — exercised
//! directly over mounted installs: the synthetic `testcity` fixture
//! and, when `MM2_RETAIL` points at one, the real installation.
//!
//! The fixture install has `city/testcity.psdl` plus authored `race:0`
//! (Checkpoint, AnyOrder) and `circuit:0` (resolves but cannot build:
//! `NumLaps` 0) / `circuit:1` (buildable Ordered) rows.

mod support;

use mm2_app::net;
use mm2_game::{
    Densities, Difficulty, EventRef, EventTableKind, RaceCustomization, SessionConditions,
    SessionConfig, SessionCustomization, SessionMode, WorldMode,
};
use support::{event, event_install, mount};

fn check(
    install: &std::path::Path,
    config: &SessionConfig,
) -> Result<(), net::SessionContentError> {
    net::check_session(&mount(install), config)
}

/// Runnable sessions pass: dev world needs no files; the authored
/// checkpoint and the buildable circuit resolve and build.
#[test]
fn check_accepts_runnable_sessions() {
    let empty = tempfile::tempdir().unwrap();
    assert!(check(empty.path(), &SessionConfig::default()).is_ok());

    let install = event_install();
    for mode in [
        SessionMode::Cruise,
        SessionMode::Event(event(EventTableKind::Checkpoint, 0)),
        SessionMode::Event(event(EventTableKind::Circuit, 1)),
    ] {
        let config = SessionConfig {
            world: WorldMode::City {
                psdl: "city/testcity.psdl".to_string(),
            },
            mode: mode.clone(),
            ..SessionConfig::default()
        };
        check(install.path(), &config).unwrap_or_else(|e| panic!("{mode:?} must pass: {e}"));
    }
}

/// Unrunnable sessions refuse: a world the mount cannot resolve, an
/// event row beyond the table, a table that does not exist and an
/// event that resolves but cannot build all fail with the gate's
/// typed errors — the same `event_race_setup` verdict `mm2-host`'s
/// flag-time gate uses.
#[test]
fn check_refuses_unrunnable_sessions() {
    let install = event_install();
    let city = |mode| SessionConfig {
        world: WorldMode::City {
            psdl: "city/testcity.psdl".to_string(),
        },
        mode,
        ..SessionConfig::default()
    };

    assert!(matches!(
        check(
            install.path(),
            &SessionConfig {
                world: WorldMode::City {
                    psdl: "city/nothere.psdl".to_string(),
                },
                ..SessionConfig::default()
            }
        ),
        Err(net::SessionContentError::World(_))
    ));
    // Row 9 is beyond the single-row checkpoint table.
    assert!(matches!(
        check(
            install.path(),
            &city(SessionMode::Event(event(EventTableKind::Checkpoint, 9)))
        ),
        Err(net::SessionContentError::Event(_))
    ));
    // No authored Crash Course table exists in the fixture — and one
    // that did resolve would refuse anyway (`CrashCourseUnsupported`).
    assert!(matches!(
        check(
            install.path(),
            &city(SessionMode::Event(event(EventTableKind::CrashCourse, 0)))
        ),
        Err(net::SessionContentError::Event(_))
    ));
    // circuit:0 resolves Ready but `NumLaps` 0 cannot build an Ordered
    // definition.
    assert!(matches!(
        check(
            install.path(),
            &city(SessionMode::Event(event(EventTableKind::Circuit, 0)))
        ),
        Err(net::SessionContentError::Event(_))
    ));
}

/// The customization bounds the structural decode leaves latent:
/// `laps` beyond the designed picker range and `opponents` beyond the
/// authored roster refuse — but only where the pick actually applies
/// (an `Ordered` rule; Cruise ignores `race` picks entirely, as does a
/// non-`Ordered` rule for `laps`).
#[test]
fn check_bounds_customization_picks() {
    let install = event_install();
    let circuit = |customization: Option<SessionCustomization>| SessionConfig {
        world: WorldMode::City {
            psdl: "city/testcity.psdl".to_string(),
        },
        mode: SessionMode::Event(event(EventTableKind::Circuit, 1)),
        customization,
        ..SessionConfig::default()
    };
    let custom = |laps, opponents| {
        Some(SessionCustomization {
            conditions: SessionConditions::default(),
            densities: Densities::default(),
            race: Some(RaceCustomization { laps, opponents }),
        })
    };

    // circuit:1's fixture aimap carries no opponent entries.
    assert!(matches!(
        check(install.path(), &circuit(custom(11, 0))),
        Err(net::SessionContentError::Laps(11))
    ));
    assert!(matches!(
        check(install.path(), &circuit(custom(5, 1))),
        Err(net::SessionContentError::Opponents {
            asked: 1,
            available: 0
        })
    ));
    check(install.path(), &circuit(custom(10, 0))).unwrap();

    // The same picks on a Checkpoint (AnyOrder) event: `laps` is not
    // consulted — the pick is inert there.
    let mut checkpoint = circuit(custom(u32::MAX, 0));
    checkpoint.mode = SessionMode::Event(event(EventTableKind::Checkpoint, 0));
    check(install.path(), &checkpoint).unwrap();

    // Cruise has no event to bound a pick against — the runtime
    // ignores it, so the gate does too.
    let mut cruise = circuit(custom(u32::MAX, u32::MAX));
    cruise.mode = SessionMode::Cruise;
    check(install.path(), &cruise).unwrap();
}

/// The gate also bounds against real authored content where the local
/// install supplies it — no asserts on retail internals beyond
/// `check_session` agreeing a stock session is runnable. Skipped
/// without the fixture install.
#[test]
fn check_accepts_a_retail_session() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        return;
    };
    let config = SessionConfig {
        world: WorldMode::City {
            psdl: "city/sf.psdl".to_string(),
        },
        mode: SessionMode::Event(EventRef {
            city: "sf".to_string(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        difficulty: Difficulty::Amateur,
        ..SessionConfig::default()
    };
    check(&retail, &config).unwrap();
}
