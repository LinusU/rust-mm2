//! `crashcourse` lesson audit (F21-A.1): every `mmcrashdata.csv` row
//! in each discovered race city viewed through the production
//! [`CourseCatalog`] — the authored tag/stage, both parameter blocks,
//! each `crash<N>data{,_p}.csv` sub-event with its `Event`-code decode
//! and waypoint-link resolution, the own-stem aimap wiring per
//! difficulty (police spawns / opponent lead cars / chase distance /
//! road exceptions), `<object>_crash<N>` override records, rewards,
//! and the wired vehicle ids cross-checked against the vehicle
//! catalog.
//!
//! `--strict` exits nonzero on a catalog-level failure: an
//! incomplete event, an unresolved filename link, an empty or missing
//! difficulty table, an aimap error, a dead `.opp` wire, or a wired
//! vehicle id outside the catalog. Extra stems the lesson attribution
//! does not claim stay visible in the summary — counted, not hidden.

use std::collections::BTreeSet;
use std::path::Path;

use mm2_assets::Vfs;
use mm2_content::{
    CourseCatalog, CrashLesson, EventCatalog, LessonTableRole, VehicleCatalog, lesson_legs,
};
use mm2_game::Difficulty;

use crate::build_vfs;

/// One-line parameter summary (the audit prints both blocks verbatim).
fn describe_params(p: &mm2_formats::racedata::RaceParams) -> String {
    format!(
        "cartype {}, tod {}, weather {}, opp {}, cops {}, laps {}, limit {}s, ambient {:.2}, peds {:.2}, diff {}",
        p.car_type,
        p.time_of_day,
        p.weather,
        p.opponents,
        p.cops,
        p.num_laps,
        p.time_limit,
        p.ambient,
        p.peds,
        p.difficulty,
    )
}

/// Print one lesson: tag/stage, params, both difficulty tables with
/// each sub-event's decode, aimap wiring, overrides and rewards.
fn print_lesson(lesson: &CrashLesson) {
    let stage = match &lesson.stage {
        mm2_content::LessonStage::Lesson(n) => format!("lesson{n}"),
        mm2_content::LessonStage::Midterm(n) => format!("midterm{n}"),
        mm2_content::LessonStage::Final(n) => format!("final{n}"),
        mm2_content::LessonStage::Other(s) => format!("other:{s}"),
    };
    let status = match &lesson.status {
        mm2_content::EventStatus::Ready => "ready".to_string(),
        mm2_content::EventStatus::Incomplete { missing } => {
            format!("incomplete ({})", missing.join(", "))
        }
    };
    println!(
        "  [{}] {} {} — {} — {status}",
        lesson.event_ref.index, stage, lesson.stem, lesson.description
    );
    println!("    am: {}", describe_params(&lesson.amateur));
    println!("    pro: {}", describe_params(&lesson.professional));

    for table in &lesson.tables {
        let role = match table.role {
            LessonTableRole::Amateur => "amateur",
            LessonTableRole::Professional => "professional",
        };
        // Authored tail names beyond the fixed five — the only
        // in-file column-name evidence (empty names omitted).
        let named: Vec<String> = table
            .columns
            .iter()
            .enumerate()
            .skip(5)
            .filter(|(_, c)| !c.is_empty())
            .map(|(i, c)| format!("tail[{i}]={c}"))
            .collect();
        println!(
            "    {} table {} ({} sub-events{})",
            role,
            table.logical,
            table.sub_events.len(),
            if named.is_empty() {
                String::new()
            } else {
                format!(", named: {}", named.join(" "))
            },
        );
        if let Some(e) = &table.error {
            println!("      FAILED: {e}");
        }
        for d in &table.diagnostics {
            println!("      note: {d}");
        }
        for s in &table.sub_events {
            let link = s.resolved.clone().unwrap_or_else(|| "UNRESOLVED".into());
            let timed = if s.timed() {
                format!("{}s", s.time_limit)
            } else {
                "untimed".into()
            };
            println!(
                "      {:<20} -> {} — e{} ({}), {} chk, {}, amb {:.2}, tail {:?}",
                s.filename,
                link,
                s.code,
                s.objective.label(),
                s.checkpoints,
                timed,
                s.amb_density,
                s.extras,
            );
        }
    }

    for (slot, label) in ["amateur", "professional"].iter().enumerate() {
        if let Some(w) = &lesson.wiring[slot] {
            let chase = w
                .cop_chase_distance
                .map(|d| format!(", chase {d}"))
                .unwrap_or_default();
            println!(
                "    aimap {label} ({}): {} police [{}], {} opponents{}, {} exceptions, {} ambient types",
                w.logical,
                w.police,
                w.police_vehicles.join(","),
                w.opponents.len(),
                chase,
                w.exceptions,
                w.ambient_types,
            );
            for o in &w.opponents {
                let res = o.resolved.clone().unwrap_or_else(|| "UNRESOLVED".into());
                println!("        opp {} {} -> {}", o.vehicle, o.route, res);
            }
            for i in &w.issues {
                println!("        issue: {i}");
            }
        } else if let Some(e) = &lesson.wiring_errors[slot] {
            println!("    aimap {label}: {e}");
        } else {
            println!("    aimap {label}: no record");
        }
    }

    if !lesson.override_records.is_empty() {
        let labels: Vec<String> = lesson
            .override_records
            .iter()
            .map(|r| {
                let kinds: Vec<String> = r.kinds.iter().map(|k| format!("{k:?}")).collect();
                format!("{} [{}]", r.label, kinds.join(","))
            })
            .collect();
        println!("    overrides: {}", labels.join(", "));
    }
    if !lesson.rewards.is_empty() {
        println!("    rewards:");
        for r in &lesson.rewards {
            println!(
                "      {} {:?} {} variant {} — {}",
                r.race_type, r.race_num, r.car, r.variant, r.message
            );
        }
    }
    for i in &lesson.issues {
        println!("    issue: {i}");
    }
}

/// Build one lesson's runnable legs at both difficulties (F21-B.1),
/// printing each leg's gate/limit summary. Returns one failure per
/// difficulty that does not build — a lesson with a broken leg is not
/// playable, so `--strict` counts it.
fn print_legs(catalog: &EventCatalog, lesson: &CrashLesson) -> Vec<String> {
    let mut failures = Vec::new();
    for difficulty in [Difficulty::Amateur, Difficulty::Professional] {
        match lesson_legs(catalog, lesson, difficulty) {
            Ok(legs) => {
                println!("    legs ({}): {} built", difficulty.as_str(), legs.len());
                for leg in &legs {
                    let d = &leg.definition;
                    let limit = d
                        .time_limit_ticks
                        .map(|t| {
                            format!("{:.1}s", f64::from(t) / f64::from(mm2_game::RACE_TICK_HZ))
                        })
                        .unwrap_or_else(|| "untimed".into());
                    println!(
                        "      {:<20} {} gates, {limit}, amb {:.2}, start slot {} (gate run only; {} rule unrecovered)",
                        leg.filename,
                        d.checkpoints.len(),
                        d.params.densities.traffic,
                        d.start_slots.len(),
                        leg.objective.label(),
                    );
                }
            }
            Err(e) => {
                println!("    legs ({}): FAILED — {e}", difficulty.as_str());
                failures.push(format!(
                    "{} {}: legs do not build — {e}",
                    lesson.stem,
                    difficulty.as_str()
                ));
            }
        }
    }
    failures
}

/// Scan one city into a [`CourseCatalog`] and collect the wired
/// vehicle ids for the catalog cross-check.
fn report_city(
    vfs: &Vfs,
    city: &str,
    vehicle_ids: &BTreeSet<String>,
) -> (EventCatalog, CourseCatalog, Vec<String>) {
    let catalog = EventCatalog::scan(vfs, city);
    let course = CourseCatalog::scan(vfs, &catalog);
    let mut wired: BTreeSet<String> = BTreeSet::new();
    for lesson in &course.lessons {
        for w in lesson.wiring.iter().flatten() {
            wired.extend(w.police_vehicles.iter().cloned());
            wired.extend(w.opponents.iter().map(|o| o.vehicle.clone()));
        }
    }
    let unresolved: Vec<String> = wired
        .into_iter()
        .filter(|id| !vehicle_ids.contains(id))
        .collect();
    (catalog, course, unresolved)
}

/// `mm2-inspect crashcourse <dir> [--city <stem>] [--strict]` —
/// audits every discovered race city's Crash Course lessons.
pub fn run(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let vehicle_ids: BTreeSet<String> = VehicleCatalog::scan(&vfs)
        .entries
        .iter()
        .map(|e| e.id.clone())
        .collect();
    let mut failures = Vec::new();
    for city in crate::race_cities(&vfs, city) {
        let (catalog, course, unresolved) = report_city(&vfs, &city, &vehicle_ids);
        println!("== crash course: {} ==", course.city);
        for lesson in &course.lessons {
            print_lesson(lesson);
            failures.extend(
                print_legs(&catalog, lesson)
                    .into_iter()
                    .map(|f| format!("{city}: {f}")),
            );
        }
        let ready = course
            .lessons
            .iter()
            .filter(|l| l.status.is_ready())
            .count();
        println!(
            "  {}: {} lessons — {} ready, {} incomplete, {} extras unclaimed by lessons",
            course.city,
            course.lessons.len(),
            ready,
            course.lessons.len() - ready,
            course.remaining_extras.len(),
        );
        for u in &unresolved {
            println!("  issue: wired vehicle {u} is not in the vehicle catalog");
            failures.push(format!(
                "{city}: wired vehicle {u} is not in the vehicle catalog"
            ));
        }
        failures.extend(
            course
                .failures()
                .into_iter()
                .map(|f| format!("{city}: {f}")),
        );
        println!();
    }
    if strict && !failures.is_empty() {
        return Err(format!("strict crash-course audit: {} failures", failures.len()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    const MM_HEADER: &str = "Description, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty, CarType, TimeofDay, Weather, Opponents, Cops, Ambient, Peds, NumLaps, TimeLimit, Difficulty";
    const WAYPOINTS: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n1,2,3,4,15,0,0,0,\n5,6,7,8,10,0,0,0,\n";
    const OPP: &str = "x,y,z,brake,forward offset,side offset,target speed,speed start,side start\n1,2,3,175,0,0,0,0,0\n";
    const CRASHDATA: &str =
        "Filename,Event,Checkpoints,TimeLimit,AmbDensity,extra,extra,extra,extra,etra,\n";

    fn vfs_of(dir: &Path) -> Vfs {
        let mut vfs = Vfs::new();
        vfs.mount_dir(dir, 0).unwrap();
        vfs
    }

    /// One complete crash0 wiring `lead_car` onto `Follow-1.opp`
    /// (case-insensitive against `follow-1.opp` on disk).
    fn synthetic_install(lead_car: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        write(
            dir,
            "race/london/mmcrashdata.csv",
            &format!("{MM_HEADER}\nlesson1,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1\n"),
        );
        write(
            dir,
            "race/london/crash0.aimap",
            &format!("[Opponent]\n1\n{lead_car} Follow-1.opp 1.00 0 50.0 0.7 1 1 1 1 0 1.0\n"),
        );
        write(dir, "race/london/crash0.aimap_p", "[Opponent]\n0\n");
        write(
            dir,
            "race/london/crash0data.csv",
            &format!("{CRASHDATA}follow,2,1,36,0,0,0,1,0,0,0\n"),
        );
        write(
            dir,
            "race/london/crash0data_p.csv",
            &format!("{CRASHDATA}follow,2,1,30,0.2,0,0,1,0,0,0\n"),
        );
        write(dir, "race/london/follow.csv", WAYPOINTS);
        write(dir, "race/london/follow-1.opp", OPP);
        write(dir, "tune/vpbug.info", "vpbug\n");
        d
    }

    #[test]
    fn complete_course_has_no_failures_and_every_wire_resolves() {
        let d = synthetic_install("vpbug");
        let vfs = vfs_of(d.path());
        let ids = crate::event::vehicle_ids(&vfs);
        let (catalog, course, unresolved) = report_city(&vfs, "london", &ids);
        assert_eq!(course.lessons.len(), 1);
        assert!(unresolved.is_empty(), "{unresolved:?}");
        assert!(course.failures().is_empty(), "{:?}", course.failures());
        let legs = print_legs(&catalog, &course.lessons[0]);
        assert!(legs.is_empty(), "{legs:?}");
    }

    #[test]
    fn a_lesson_whose_leg_cannot_build_is_a_strict_failure_per_difficulty() {
        let d = synthetic_install("vpbug");
        // One waypoint row is a start with nowhere to go.
        write(
            d.path(),
            "race/london/follow.csv",
            "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n1,2,3,4,15,0,0,0,\n",
        );
        let vfs = vfs_of(d.path());
        let ids = crate::event::vehicle_ids(&vfs);
        let (catalog, course, _) = report_city(&vfs, "london", &ids);
        let failures = print_legs(&catalog, &course.lessons[0]);
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(failures[0].contains("crash0 amateur"), "{failures:?}");
        assert!(failures[1].contains("crash0 professional"), "{failures:?}");
    }

    #[test]
    fn a_wire_to_an_unknown_vehicle_is_a_cross_check_failure() {
        let d = synthetic_install("vpghost");
        let vfs = vfs_of(d.path());
        let ids = crate::event::vehicle_ids(&vfs);
        let (_catalog, _course, unresolved) = report_city(&vfs, "london", &ids);
        assert_eq!(unresolved, vec!["vpghost".to_string()]);
    }

    #[test]
    fn a_city_without_a_course_table_reports_the_empty_denominator() {
        let d = synthetic_install("vpbug");
        let vfs = vfs_of(d.path());
        let ids = crate::event::vehicle_ids(&vfs);
        // The install carries no `race/sf/` data at all.
        let (_catalog, course, _unresolved) = report_city(&vfs, "sf", &ids);
        assert!(course.lessons.is_empty());
        assert!(
            course
                .failures()
                .iter()
                .any(|f| f.contains("no crash-course lessons"))
        );
    }
}
