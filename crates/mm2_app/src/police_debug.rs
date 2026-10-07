//! `--police-debug` overlay (F20, requirement 6): what each cop is
//! doing, drawn through Bevy gizmos.
//!
//! Per cop: a pole over the car coloured by its pursuit phase, and —
//! while it holds a chase goal — a line from the car to the place it
//! last saw the target (never the target's true position: that is all
//! the cop knows) plus the road line it is following, from the cursor
//! onward. [`debug_lines`] is pure, so the geometry and the "only what
//! the cop knows" rule are asserted without a renderer. Presentation
//! only: it reads the chase and writes nothing.

use avian3d::prelude::Position;
use bevy::prelude::*;
use mm2_game::{ChaseMode, ChaseNav, Pursuit, PursuitPhase, Session};

use crate::police::PoliceCar;

/// Height of the phase pole over a cop.
const POLE: f32 = 6.0;
/// Route and goal lines hover this far above the road so they read over
/// it (the nav overlay's route lift is 0.9).
const LIFT: f32 = 1.2;
/// Half-width of the goal cross.
const GOAL_ARM: f32 = 2.5;

/// What a segment depicts; [`draw_police_debug`] maps it to a colour and
/// tests assert on the classes themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugClass {
    /// Pole over a cop at its post.
    Idle,
    /// Pole over a cop that has the target in sight and is reacting.
    Engaged,
    /// Pole over a cop that is chasing.
    Pursuing,
    /// Pole over a cop that gave up.
    Lost,
    /// Cop → the place it last saw the target.
    Goal,
    /// The planned road line, cursor onward.
    Route,
    /// Straight aim because the cop has no road line (no graph, a failed
    /// query, or none asked for yet): the cop → goal line in this class
    /// instead of [`DebugClass::Goal`].
    Unrouted,
}

/// One drawable segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugSegment {
    /// Segment start.
    pub a: Vec3,
    /// Segment end.
    pub b: Vec3,
    /// What it depicts.
    pub class: DebugClass,
}

/// One cop as the overlay sees it — plain data so tests need no ECS.
#[derive(Debug, Clone, PartialEq)]
pub struct CopView {
    /// The car's position.
    pub position: Vec3,
    /// Its pursuit phase.
    pub phase: PursuitPhase,
    /// Where it last had the target in contact.
    pub last_seen: Option<Vec3>,
    /// The law that chose its latest aim.
    pub mode: ChaseMode,
    /// The part of its road line still ahead.
    pub route: Vec<Vec3>,
}

/// The pole's class for a phase.
pub fn phase_class(phase: PursuitPhase) -> DebugClass {
    match phase {
        PursuitPhase::Idle => DebugClass::Idle,
        PursuitPhase::Engaged(_) => DebugClass::Engaged,
        PursuitPhase::Pursuing(_) => DebugClass::Pursuing,
        PursuitPhase::Lost(_) => DebugClass::Lost,
    }
}

/// The overlay's segments for the given cops, in the order given. Only
/// a pursuing cop shows its goal and route — an idle, reacting or
/// stood-down cop has no chase to depict, even when it remembers a
/// `last_seen` from an earlier one.
pub fn debug_lines(cops: &[CopView]) -> Vec<DebugSegment> {
    let lift = |p: Vec3| p + Vec3::Y * LIFT;
    let mut out = Vec::new();
    for cop in cops {
        out.push(DebugSegment {
            a: cop.position,
            b: cop.position + Vec3::Y * POLE,
            class: phase_class(cop.phase),
        });
        let PursuitPhase::Pursuing(_) = cop.phase else {
            continue;
        };
        if let Some(goal) = cop.last_seen {
            let g = lift(goal);
            out.push(DebugSegment {
                a: g - Vec3::X * GOAL_ARM,
                b: g + Vec3::X * GOAL_ARM,
                class: DebugClass::Goal,
            });
            out.push(DebugSegment {
                a: g - Vec3::Z * GOAL_ARM,
                b: g + Vec3::Z * GOAL_ARM,
                class: DebugClass::Goal,
            });
            // Following a road line, the line itself says where the cop
            // is going; otherwise the straight aim is the whole story.
            if cop.mode != ChaseMode::Road {
                out.push(DebugSegment {
                    a: lift(cop.position),
                    b: g,
                    class: if cop.mode == ChaseMode::Unrouted {
                        DebugClass::Unrouted
                    } else {
                        DebugClass::Goal
                    },
                });
            }
        }
        if cop.mode == ChaseMode::Road {
            out.extend(cop.route.windows(2).map(|w| DebugSegment {
                a: lift(w[0]),
                b: lift(w[1]),
                class: DebugClass::Route,
            }));
        }
    }
    out
}

/// Whether the running session asked for the overlay.
pub fn enabled(session: Res<Session>) -> bool {
    session.config().is_some_and(|c| c.dev.police_debug)
}

/// Draw the overlay — emitted fresh every frame. Cops are walked in
/// authored order so the output is stable.
pub fn draw_police_debug(
    cops: Query<(&PoliceCar, &Pursuit, &ChaseNav, &Position)>,
    mut gizmos: Gizmos,
) {
    let mut views: Vec<(usize, CopView)> = cops
        .iter()
        .map(|(car, pursuit, chase, pos)| {
            (
                car.index,
                CopView {
                    position: pos.0,
                    phase: pursuit.phase,
                    last_seen: pursuit.last_seen,
                    mode: chase.mode,
                    route: chase
                        .route()
                        .map_or_else(Vec::new, |r| r.remaining().to_vec()),
                },
            )
        })
        .collect();
    views.sort_unstable_by_key(|v| v.0);
    let views: Vec<CopView> = views.into_iter().map(|v| v.1).collect();
    for s in debug_lines(&views) {
        gizmos.line(s.a, s.b, class_color(s.class));
    }
}

fn class_color(class: DebugClass) -> Color {
    match class {
        DebugClass::Idle => Color::srgb(0.6, 0.6, 0.6),
        DebugClass::Engaged => Color::srgb(1.0, 0.85, 0.1),
        DebugClass::Pursuing => Color::srgb(1.0, 0.15, 0.15),
        DebugClass::Lost => Color::srgb(0.2, 0.5, 1.0),
        DebugClass::Goal => Color::srgb(1.0, 0.5, 0.1),
        DebugClass::Route => Color::srgb(0.2, 1.0, 0.9),
        DebugClass::Unrouted => Color::srgb(1.0, 0.2, 0.8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cop(phase: PursuitPhase, mode: ChaseMode) -> CopView {
        CopView {
            position: Vec3::new(10.0, 0.0, 10.0),
            phase,
            last_seen: Some(Vec3::new(50.0, 0.0, 10.0)),
            mode,
            route: vec![
                Vec3::new(10.0, 0.0, 10.0),
                Vec3::new(30.0, 0.0, 10.0),
                Vec3::new(50.0, 0.0, 10.0),
            ],
        }
    }

    fn count(lines: &[DebugSegment], class: DebugClass) -> usize {
        lines.iter().filter(|s| s.class == class).count()
    }

    #[test]
    fn each_phase_gets_its_own_pole_and_only_a_chase_shows_a_goal() {
        for (phase, class) in [
            (PursuitPhase::Idle, DebugClass::Idle),
            (PursuitPhase::Engaged(0.3), DebugClass::Engaged),
            (PursuitPhase::Lost(2.0), DebugClass::Lost),
        ] {
            // A remembered `last_seen` from an earlier chase is not drawn.
            let lines = debug_lines(&[cop(phase, ChaseMode::Road)]);
            assert_eq!(lines.len(), 1, "{phase:?}");
            assert_eq!(lines[0].class, class);
            assert_eq!(lines[0].b - lines[0].a, Vec3::Y * POLE);
        }
        let lines = debug_lines(&[cop(PursuitPhase::Pursuing(0.0), ChaseMode::Direct)]);
        assert_eq!(count(&lines, DebugClass::Pursuing), 1);
        assert_eq!(count(&lines, DebugClass::Goal), 3, "cross + straight aim");
        assert_eq!(count(&lines, DebugClass::Route), 0);
    }

    #[test]
    fn a_road_chase_draws_the_line_not_a_straight_aim() {
        let lines = debug_lines(&[cop(PursuitPhase::Pursuing(0.0), ChaseMode::Road)]);
        assert_eq!(
            count(&lines, DebugClass::Route),
            2,
            "three points, two legs"
        );
        assert_eq!(count(&lines, DebugClass::Goal), 2, "just the cross");
        assert_eq!(count(&lines, DebugClass::Unrouted), 0);
        // Lifted off the road, from the first remaining point.
        let first = lines.iter().find(|s| s.class == DebugClass::Route).unwrap();
        assert_eq!(first.a, Vec3::new(10.0, LIFT, 10.0));
    }

    #[test]
    fn an_unrouted_chase_is_marked_so_a_missing_line_is_visible() {
        let lines = debug_lines(&[cop(PursuitPhase::Pursuing(1.0), ChaseMode::Unrouted)]);
        assert_eq!(count(&lines, DebugClass::Unrouted), 1);
        assert_eq!(count(&lines, DebugClass::Route), 0);
    }

    #[test]
    fn the_goal_is_the_last_seen_place_and_a_cop_with_none_draws_only_its_pole() {
        let mut c = cop(PursuitPhase::Pursuing(0.0), ChaseMode::Unrouted);
        c.last_seen = None;
        let lines = debug_lines(&[c]);
        assert_eq!(lines.len(), 1);
        let lines = debug_lines(&[cop(PursuitPhase::Pursuing(0.0), ChaseMode::Road)]);
        let cross = lines.iter().find(|s| s.class == DebugClass::Goal).unwrap();
        assert_eq!((cross.a + cross.b) * 0.5, Vec3::new(50.0, LIFT, 10.0));
    }

    #[test]
    fn no_cops_draw_nothing() {
        assert!(debug_lines(&[]).is_empty());
    }
}
