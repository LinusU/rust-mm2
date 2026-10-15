//! Which `mmlang.dll` string the Crash Course lesson object shows at
//! each step of a leg (F21 requirement 4, the subtitle leg).
//!
//! Recovered from `Midtown2.exe` (2026-10-15, static reading with
//! `objdump`): the lesson handlers show a message by calling `0x534770`
//! with a string id, a duration in seconds and a flag, and handing the
//! result to the player car's message display (`0x42e1e0` on the car
//! `+0x288`). The id is the literal string-table id, so no per-family
//! base is added. Every pre-race message holds for 1.5 s and every
//! outcome message for 5 s.
//!
//! This table binds only the ids whose handler site was read:
//!
//! - the pre-race sequence of the cop-chase, jump, follow, stop and
//!   maneuver handlers (each message advances the handler's state
//!   machine in this order), and
//! - the outcome messages whose condition `docs/original-rules.md` CC-9
//!   already records.
//!
//! Not bound, because the condition that picks them is unrecovered:
//! the cross-traffic (225–230), corner (231–240) and the clean-driving
//! family's (210–216) sequences, the follow `Game over` (223), the stop
//! `You lost!` (622), and the cop-chase alternative to the `Go` line
//! (652, 3 s). Where the lines are *drawn* on screen is unrecovered
//! too — see `docs/research/crashcourse.md`.

use crate::crashcourse::LessonObjective;

/// Seconds a pre-race message (instruction, ready, set, go) is held.
pub const PRERACE_MESSAGE_SECONDS: f32 = 1.5;
/// Seconds an outcome message is held.
pub const OUTCOME_MESSAGE_SECONDS: f32 = 5.0;

/// A step of a leg that shows a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LessonLine {
    /// The leg's one-line instruction (only some families have one).
    Instruction,
    /// `Ready...`
    Ready,
    /// `Set...`
    Set,
    /// The start call.
    Go,
    /// The leg cleared.
    Cleared,
    /// The timer ran out.
    TimedOut,
    /// The player's car was wrecked and the leg ended.
    Wrecked,
    /// The family's own failure: a follow leg whose lead escaped, a
    /// stop leg whose lead reached its destination.
    ObjectiveFailed,
    /// Cop chase: the course is complete but a cop still sees the car.
    CopsStillClose,
}

impl LessonLine {
    /// The pre-race lines, in the order the handler shows them.
    pub fn prerace(objective: LessonObjective) -> &'static [LessonLine] {
        use LessonLine::*;
        match objective {
            LessonObjective::CopChase | LessonObjective::Jump => &[Instruction, Ready, Go],
            LessonObjective::Follow | LessonObjective::Stop | LessonObjective::Maneuver => {
                &[Ready, Set, Go]
            }
            _ => &[],
        }
    }
}

/// The `mmlang.dll` string id the lesson shows for `line` on a leg of
/// `objective`, or `None` when that pairing is unbound (see the module
/// docs).
pub fn message_id(objective: LessonObjective, line: LessonLine) -> Option<u32> {
    use LessonLine::*;
    use LessonObjective as O;
    Some(match (objective, line) {
        (O::CopChase, Instruction) => 193,
        (O::CopChase, Ready) => 194,
        (O::CopChase, Go) => 195,
        (O::CopChase, Cleared) => 196,
        (O::CopChase, CopsStillClose) => 197,
        (O::CopChase, TimedOut) => 198,

        (O::Jump, Instruction) => 206,
        (O::Jump, Ready) => 207,
        (O::Jump, Go) => 208,
        (O::Jump, Cleared) => 209,
        (O::Jump, TimedOut) => 614,
        (O::Jump, Wrecked) => 615,

        (O::Follow, Ready) => 218,
        (O::Follow, Set) => 219,
        (O::Follow, Go) => 220,
        (O::Follow, Cleared) => 221,
        (O::Follow, ObjectiveFailed) => 222,

        (O::Stop, Ready) => 617,
        (O::Stop, Set) => 618,
        (O::Stop, Go) => 619,
        (O::Stop, Cleared) => 620,
        (O::Stop, ObjectiveFailed) => 621,

        (O::Maneuver, Ready) => 241,
        (O::Maneuver, Set) => 242,
        (O::Maneuver, Go) => 243,
        (O::Maneuver, Cleared) => 609,
        (O::Maneuver, TimedOut) => 244,
        (O::Maneuver, Wrecked) => 245,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prerace_line_is_bound() {
        for objective in [
            LessonObjective::CopChase,
            LessonObjective::Jump,
            LessonObjective::Follow,
            LessonObjective::Stop,
            LessonObjective::Maneuver,
        ] {
            let lines = LessonLine::prerace(objective);
            assert_eq!(lines.len(), 3);
            for line in lines {
                assert!(
                    message_id(objective, *line).is_some(),
                    "{objective:?} {line:?}"
                );
            }
        }
    }

    #[test]
    fn unrecovered_families_stay_unbound() {
        for objective in [
            LessonObjective::Corner,
            LessonObjective::CrossTraffic,
            LessonObjective::Map,
        ] {
            assert!(LessonLine::prerace(objective).is_empty());
            assert_eq!(message_id(objective, LessonLine::Cleared), None);
        }
    }

    #[test]
    fn ids_are_unique_within_a_family() {
        use LessonLine::*;
        let all = [
            Instruction,
            Ready,
            Set,
            Go,
            Cleared,
            TimedOut,
            Wrecked,
            ObjectiveFailed,
            CopsStillClose,
        ];
        for objective in [
            LessonObjective::CopChase,
            LessonObjective::Jump,
            LessonObjective::Follow,
            LessonObjective::Stop,
            LessonObjective::Maneuver,
        ] {
            let mut ids: Vec<u32> = all
                .iter()
                .filter_map(|l| message_id(objective, *l))
                .collect();
            let n = ids.len();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), n, "{objective:?}");
        }
    }
}
