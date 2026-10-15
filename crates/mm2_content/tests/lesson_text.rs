//! Retail check of the Crash Course message ids against `mmlang.dll`.
//! Skipped without the operator's install (`MM2_RETAIL=<dir>`).

use mm2_content::crashcourse::LessonObjective;
use mm2_content::lesson_text::{LessonLine, message_id};
use mm2_formats::langdll::StringTable;

const LINES: [LessonLine; 9] = [
    LessonLine::Instruction,
    LessonLine::Ready,
    LessonLine::Set,
    LessonLine::Go,
    LessonLine::Cleared,
    LessonLine::TimedOut,
    LessonLine::Wrecked,
    LessonLine::ObjectiveFailed,
    LessonLine::CopsStillClose,
];

#[test]
fn retail_every_bound_lesson_message_has_text() {
    let Some(retail) = std::env::var_os("MM2_RETAIL").map(std::path::PathBuf::from) else {
        eprintln!("skipped: MM2_RETAIL is not set");
        return;
    };
    let table = StringTable::parse(&std::fs::read(retail.join("mmlang.dll")).unwrap()).unwrap();
    assert!(table.len() > 600, "{} strings", table.len());
    let mut bound = 0;
    for objective in [
        LessonObjective::CopChase,
        LessonObjective::Jump,
        LessonObjective::Follow,
        LessonObjective::Stop,
        LessonObjective::Maneuver,
    ] {
        for line in LINES {
            if let Some(id) = message_id(objective, line) {
                bound += 1;
                let text = table
                    .get(id)
                    .unwrap_or_else(|| panic!("{objective:?} {line:?}: id {id}"));
                assert!(!text.trim().is_empty(), "{objective:?} {line:?}: id {id}");
            }
        }
    }
    assert_eq!(bound, 28);
}
