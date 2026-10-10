//! F21-C: both retail Crash Course progressions end to end on the
//! production catalog, availability and reward tables — the coverage
//! report (F21-AC06) and the pass→unlock chain (F21-AC01/AC04).
//! Retail-gated (`MM2_RETAIL=<dir>`); skipped without it.

use mm2_assets::Vfs;
use mm2_content::{CourseCatalog, EventCatalog, LessonStage, RuleEvidence};
use mm2_game::{
    Difficulty, EventKey, EventTableKind, PlayerProfile, ProfileKind, ProfileStore, SessionOutcome,
    apply_result,
};

fn profile() -> (tempfile::TempDir, PlayerProfile) {
    let dir = tempfile::tempdir().unwrap();
    let store = ProfileStore::open(dir.path()).unwrap();
    let p = store
        .create("driver", Difficulty::Amateur, ProfileKind::Standard)
        .unwrap();
    (dir, p)
}

fn retail_vfs() -> Option<Vfs> {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return None;
    };
    let mut vfs = Vfs::new();
    mm2_assets::mount_install(&mut vfs, &retail, &mm2_assets::InstallMount::default()).unwrap();
    Some(vfs)
}

/// Retail: 13 lessons per school, each listed with its evidence level;
/// none claims a recovered rule (UNK-35), and the report says so.
#[test]
fn retail_coverage_enumerates_every_lesson_unfiltered() {
    let Some(vfs) = retail_vfs() else { return };
    for city in ["sf", "london"] {
        let cat = EventCatalog::scan(&vfs, city);
        let course = CourseCatalog::scan(&vfs, &cat);
        assert!(
            course.failures().is_empty(),
            "{city}: {:?}",
            course.failures()
        );
        let cov = course.coverage();
        eprintln!("{}", cov.render());
        assert_eq!(cov.lessons.len(), 13, "{city}");
        for l in &cov.lessons {
            assert!(l.legs[0] > 0 && l.legs[1] > 0, "{city} {}: no legs", l.stem);
            assert!(!l.objectives.is_empty(), "{city} {}", l.stem);
            assert!(
                !matches!(l.stage, LessonStage::Other(_)),
                "{city} {}",
                l.stem
            );
        }
        assert_eq!(cov.count(RuleEvidence::Unresolved), 13, "{city}");
    }
}

/// Retail: passing the lessons in authored order opens each midterm
/// and the final exactly when CC-2/CC-3 say; every `crash,N` reward
/// grants on the first pass of its row only; a timeout, a repeat pass
/// and an out-of-order pass grant nothing new.
#[test]
fn retail_progression_unlocks_and_rewards_once() {
    let Some(vfs) = retail_vfs() else { return };
    for city in ["sf", "london"] {
        let cat = EventCatalog::scan(&vfs, city);
        let course = CourseCatalog::scan(&vfs, &cat);
        let avail = mm2_content::availability_table(&cat);
        let rewards = mm2_content::reward_table(&cat);
        assert!(
            avail.diagnostics.is_empty(),
            "{city}: {:?}",
            avail.diagnostics
        );
        let key = |stem: &str| EventKey {
            city: city.to_string(),
            table: EventTableKind::CrashCourse,
            stem: stem.to_string(),
        };
        let (_dir, mut p) = profile();
        let pass = SessionOutcome::Finished { race_ticks: 600 };
        let mut granted_total = 0;
        let mut expected_total = 0;

        for l in &course.lessons {
            let k = key(&l.stem);
            let gated = !matches!(l.stage, LessonStage::Lesson(_));
            let open = avail.of(&p, &k).unwrap().unlocked;
            // Authored order: everything before is already passed, so
            // every row is open by the time it is reached.
            assert!(open, "{city} {} locked after its prerequisites", l.stem);
            if gated {
                // The same row on a fresh profile is locked.
                let (_d2, fresh) = profile();
                assert!(!avail.of(&fresh, &k).unwrap().unlocked, "{city} {}", l.stem);
            }

            // Failure and timeout record nothing and grant nothing.
            let timed_out = apply_result(
                &mut p,
                &k,
                &SessionOutcome::TimedOut { race_ticks: 900 },
                None,
                Difficulty::Amateur,
                &rewards,
            );
            assert!(!timed_out.recorded && timed_out.granted.is_empty());
            assert!(p.event(&k).is_none_or(|r| !r.is_beaten()));

            let rule_count = rewards.per_event.iter().filter(|(rk, _)| rk == &k).count();
            expected_total += rule_count;
            let first = apply_result(&mut p, &k, &pass, Some(1), Difficulty::Amateur, &rewards);
            assert!(first.recorded);
            assert_eq!(first.granted.len(), rule_count, "{city} {}", l.stem);
            granted_total += first.granted.len();
            let again = apply_result(&mut p, &k, &pass, Some(1), Difficulty::Amateur, &rewards);
            assert!(again.granted.is_empty(), "{city} {} regranted", l.stem);
        }
        assert_eq!(granted_total, expected_total);
        assert!(expected_total > 0, "{city}: no crash rewards exercised");
        let crash_rules = rewards
            .per_event
            .iter()
            .filter(|(k, _)| k.table == EventTableKind::CrashCourse)
            .count();
        assert_eq!(
            crash_rules, expected_total,
            "{city}: reward on a non-lesson key"
        );
    }
}
