//! The Cops & Robbers match readout (F27-B.4c, HUD leg): a short text
//! panel — clock or limit, the local side and points, the gold's
//! whereabouts, and the result once the match ends — drawn from the
//! match's observable state ([`GoldView`]).
//!
//! The same view feeds both ends: the authority reads its [`CnrHost`]
//! and a client its [`CnrReplica`], so a joined player sees exactly
//! what the host decided. The *data* is what the retail HUD shows
//! (clock, points, who holds the loot); the layout and wording are
//! *designed* — the original's instrument art and positions are
//! unrecovered, and no authored label art for this readout was found,
//! so it composes dev-font text rather than substituting art. The
//! original's spoken commentary families are the audio leg's, not this
//! one's.

use bevy::prelude::*;
use mm2_game::gold::{
    CnrVariant, DeliveryTarget, EndReason, EndRule, GoldState, GoldView, Side, Winner,
};
use mm2_game::{Player, PlayerControl, PlayerId, RACE_TICK_HZ, SessionEntity};

use crate::cnr::{CnrHost, participant_id};
use crate::cnrnet::CnrReplica;
use crate::netdrive::NetPlayer;

/// Marker on the session-owned readout. Listed among the HUD roots
/// `camera::retarget_hud` pins to the active camera.
#[derive(Component)]
pub struct CnrScoreboard;

/// Spawn the (empty) readout, session-owned, in the top-right corner —
/// the corner the race instruments use, which a free-roam match never
/// spawns. Filled by [`update_cnr_scoreboard`].
pub fn spawn_cnr_scoreboard(commands: &mut Commands, owner: SessionEntity) {
    commands.spawn((
        owner,
        CnrScoreboard,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::WHITE),
        TextLayout::justify(Justify::Right),
        BackgroundColor(Color::srgba(0.19, 0.188, 0.192, 0.82)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            right: Val::Px(10.0),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
            ..default()
        },
    ));
}

/// What a side is called on the board — the original's team labels
/// (`COPS`/`ROBBERS`, `RED`/`BLUE`); free-for-all has none.
pub fn side_label(side: Side) -> &'static str {
    match side {
        Side::Solo => "",
        Side::Robbers => "ROBBERS",
        Side::Cops => "COPS",
        Side::Red => "RED",
        Side::Blue => "BLUE",
    }
}

/// `m:ss` for a tick count at `hz`, rounded *up* to the second so a
/// countdown never reads `0:00` while time remains.
pub fn clock_text(ticks: u64, hz: u32) -> String {
    let secs = ticks.div_ceil(u64::from(hz.max(1)));
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A side's points: the sum over its members, leavers included (the
/// same total [`mm2_game::gold::GoldMatch::side_total`] keeps).
fn side_points(view: &GoldView, side: Side) -> u32 {
    view.standings
        .iter()
        .filter(|s| s.side == side)
        .fold(0u32, |a, s| a.saturating_add(s.score))
}

/// Why a match ended, in the readout's words.
fn reason_text(reason: EndReason) -> &'static str {
    match reason {
        EndReason::PointLimit => "point limit",
        EndReason::TimeLimit => "time",
    }
}

/// Who won, from `me`'s seat (`YOU WIN` only for the local participant).
fn verdict_text(winner: Winner, me: Option<PlayerId>) -> String {
    match winner {
        Winner::Tie => "TIE".to_string(),
        Winner::Player(p) if Some(p) == me => "YOU WIN".to_string(),
        Winner::Player(p) => format!("PLAYER {} WINS", p.0),
        Winner::Side(s) => format!("{} WIN", side_label(s)),
    }
}

/// The match this process can see: the authority's own, else a client's
/// replica of it, else none.
pub fn match_view(host: Option<&CnrHost>, replica: Option<&CnrReplica>) -> Option<GoldView> {
    match (host, replica) {
        (Some(host), _) => Some(host.game.view()),
        (None, Some(replica)) => Some(replica.0.clone()),
        (None, None) => None,
    }
}

/// The match-over screen's body: the verdict, how long the match ran,
/// the team totals where it is team-scored, then every participant best
/// first (leavers marked). `None` while the match is undecided — the
/// screen has nothing to say before there is a result.
pub fn result_lines(view: &GoldView, me: Option<PlayerId>, hz: u32) -> Option<Vec<String>> {
    let outcome = view.outcome?;
    let mut lines = vec![
        verdict_text(outcome.winner, me),
        format!(
            "{} - played {}",
            reason_text(outcome.reason),
            clock_text(outcome.at_tick, hz)
        ),
    ];
    if view.variant.team_scored() {
        let totals: Vec<String> = view
            .variant
            .sides()
            .iter()
            .map(|&s| format!("{} {}", side_label(s), side_points(view, s)))
            .collect();
        lines.push(totals.join("   "));
    }
    for (i, s) in view.standings.iter().enumerate() {
        let name = if Some(s.player) == me {
            "You".to_string()
        } else {
            format!("Player {}", s.player.0)
        };
        let side = match side_label(s.side) {
            "" => String::new(),
            label => format!(" ({label})"),
        };
        let left = if s.connected { "" } else { " - left" };
        lines.push(format!("{}. {name}{side} - {} pts{left}", i + 1, s.score));
    }
    Some(lines)
}

/// The readout's lines for one participant's view of a match. `me` is
/// the local participant, if one is seated (`None` shows the match
/// without a personal line — a spectating or not-yet-seated process).
pub fn scoreboard_lines(view: &GoldView, me: Option<PlayerId>, hz: u32) -> Vec<String> {
    let mut lines = Vec::new();

    // Clock / limit. A decided match freezes at the tick it ended on.
    let now = view.outcome.map_or(view.elapsed, |o| o.at_tick);
    let clock = match view.end {
        EndRule::Ticks(limit) => clock_text(limit.saturating_sub(now), hz),
        EndRule::Points(limit) => format!("first to {limit}"),
        EndRule::None => clock_text(now, hz),
    };
    lines.push(format!("COPS & ROBBERS  {clock}"));

    // Points: the team totals where the match is team-scored, the
    // local participant's own rank and points otherwise.
    let mine = me.and_then(|id| view.standings.iter().position(|s| s.player == id));
    if view.variant.team_scored() {
        let totals: Vec<String> = view
            .variant
            .sides()
            .iter()
            .map(|&s| format!("{} {}", side_label(s), side_points(view, s)))
            .collect();
        lines.push(totals.join("   "));
        if let Some(i) = mine {
            let s = &view.standings[i];
            lines.push(format!("YOU: {}  {} pts", side_label(s.side), s.score));
        }
    } else if let Some(i) = mine {
        let s = &view.standings[i];
        if view.standings.len() > 1 {
            lines.push(format!(
                "YOU: {} of {}  {} pts",
                ordinal(i + 1),
                view.standings.len(),
                s.score
            ));
        } else {
            lines.push(format!("YOU: {} pts", s.score));
        }
    }

    // The gold, or the result.
    if let Some(outcome) = view.outcome {
        lines.push(format!(
            "MATCH OVER - {} ({})",
            verdict_text(outcome.winner, me),
            reason_text(outcome.reason)
        ));
    } else {
        let my_side = mine.map(|i| view.standings[i].side);
        lines.push(match view.state {
            GoldState::Carried { by } if Some(by) == me => {
                let to = match my_side.map(Side::delivery_target) {
                    Some(DeliveryTarget::Bank) => "the bank",
                    _ => "the hideout",
                };
                format!("YOU HAVE THE GOLD - take it to {to}")
            }
            GoldState::Carried { by } => {
                let who = view
                    .standings
                    .iter()
                    .find(|s| s.player == by)
                    .filter(|_| view.variant != CnrVariant::FreeForAll)
                    .map_or_else(
                        || format!("PLAYER {}", by.0),
                        |s| format!("A {} DRIVER", side_label(s.side)),
                    );
                format!("{who} HAS THE GOLD")
            }
            GoldState::Dropped { .. } => "THE GOLD IS LOOSE".to_string(),
            GoldState::Resting { .. } => "THE GOLD IS UP FOR GRABS".to_string(),
        });
    }
    lines
}

/// What the local driver is heading for, from the match's observable
/// state — the one answer the navigation arrow and the map's dots both
/// read, so the two instruments cannot disagree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CnrObjective {
    /// Where the gold is: its drawn/drop site while nobody carries it,
    /// the carrier's car while another driver does, and `None` while the
    /// local car carries it (the gold is *in* the car — there is nothing
    /// to point at or mark) or when the carrier's car is not in view.
    pub gold: Option<Vec3>,
    /// The hideout marker's site.
    pub hideout: Vec3,
    /// The bank marker's site.
    pub bank: Vec3,
    /// The arrow's target: the delivery site of the local side while the
    /// local car carries the gold, otherwise the gold. `None` once the
    /// match is decided, or while the local participant is not seated
    /// (it has no side to deliver for).
    pub target: Option<Vec3>,
}

/// Work out the local driver's [`CnrObjective`]. `cars` yields every
/// car's participant id and position, so a carried gold can be followed
/// to its carrier. *Implementation choice*: the original's arrow
/// retargeting while a rival holds the gold is unrecovered; chasing the
/// carrier is the only sensible reading of "point at the gold" once it
/// is inside a car, and a robber/cop who is not carrying has nothing
/// else to hunt.
pub fn objective(
    view: &GoldView,
    me: Option<PlayerId>,
    cars: impl IntoIterator<Item = (PlayerId, Vec3)>,
) -> CnrObjective {
    let sites = view.sites;
    let carrier = view.carrier();
    let gold = match view.state {
        GoldState::Carried { by } if Some(by) == me => None,
        GoldState::Carried { by } => cars.into_iter().find(|(id, _)| *id == by).map(|(_, at)| at),
        GoldState::Resting { at } | GoldState::Dropped { at, .. } => Some(at),
    };
    let target = if view.outcome.is_some() {
        None
    } else if carrier.is_some() && carrier == me {
        me.and_then(|id| view.side_of(id))
            .map(|side| sites.target(side.delivery_target()))
    } else {
        // Not carrying: seated participants hunt the gold; an unseated
        // process has no part in the match yet.
        me.and_then(|id| view.side_of(id)).and(gold)
    };
    CnrObjective {
        gold,
        hideout: sites.hideout,
        bank: sites.bank,
        target,
    }
}

/// Everything a presentation system needs to read the match's
/// [`objective`]: whichever end of the match this process is, plus the
/// cars (local and remote) the carrier is found among.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CnrScene<'w, 's> {
    host: Option<Res<'w, CnrHost>>,
    replica: Option<Res<'w, CnrReplica>>,
    cars: Query<
        'w,
        's,
        (
            &'static Player,
            Option<&'static NetPlayer>,
            &'static GlobalTransform,
        ),
    >,
}

impl CnrScene<'_, '_> {
    /// The local driver's objective, or `None` with no match in this
    /// process (a race or cruise session, or a client before its first
    /// frame).
    pub fn objective(&self) -> Option<CnrObjective> {
        let view = match_view(self.host.as_deref(), self.replica.as_deref())?;
        let me = self
            .cars
            .iter()
            .find(|(p, ..)| p.control == PlayerControl::Local)
            .map(|(p, net, _)| participant_id(p, net));
        Some(objective(
            &view,
            me,
            self.cars
                .iter()
                .map(|(p, net, gt)| (participant_id(p, net), gt.translation())),
        ))
    }
}

/// Fill the readout from whichever end of the match this process is —
/// the authority's [`CnrHost`] or a client's [`CnrReplica`]; empty (and
/// hidden) with neither, and hidden while the `H` HUD gate is off.
pub fn update_cnr_scoreboard(
    host: Option<Res<CnrHost>>,
    replica: Option<Res<CnrReplica>>,
    hud: Res<crate::hud::HudVisible>,
    cars: Query<(&Player, Option<&NetPlayer>)>,
    mut board: Query<(&mut Text, &mut Visibility), With<CnrScoreboard>>,
) {
    let view = match_view(host.as_deref(), replica.as_deref());
    let me = cars
        .iter()
        .find(|(p, _)| p.control == PlayerControl::Local)
        .map(|(p, net)| participant_id(p, net));
    for (mut text, mut visibility) in &mut board {
        let want = match view.as_ref().filter(|_| hud.0) {
            Some(view) => {
                let body = scoreboard_lines(view, me, RACE_TICK_HZ).join("\n");
                if text.0 != body {
                    text.0 = body;
                }
                Visibility::Inherited
            }
            None => Visibility::Hidden,
        };
        if *visibility != want {
            *visibility = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::gold::{CarrierLoad, GoldMatch, GoldRules};
    use mm2_game::{ObjectId, Session, SessionConfig, SessionPhase};

    const A: PlayerId = PlayerId(1);
    const B: PlayerId = PlayerId(2);
    const C: PlayerId = PlayerId(3);

    fn rules(variant: CnrVariant, end: EndRule) -> GoldRules {
        GoldRules {
            variant,
            end,
            load: CarrierLoad {
                added_mass_kg: 250.0,
                handling_scalar: 0.9,
            },
            pickup_points: 25,
            delivery_points: 100,
            pickup_radius: 5.0,
            delivery_radius: 12.0,
            drop_lockout_ticks: 120,
        }
    }

    fn game(
        variant: CnrVariant,
        end: EndRule,
        seats: &[(PlayerId, Side)],
    ) -> (GoldMatch, ObjectId) {
        let mut session = Session::new();
        session.begin(SessionConfig::default()).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let gold = session.mint_object_id();
        let pool = (0..8)
            .map(|i| Vec3::new(i as f32 * 100.0, 0.0, (i * i) as f32 * 7.0))
            .collect();
        let m = GoldMatch::new(
            session.generation(),
            gold,
            rules(variant, end),
            pool,
            5,
            seats,
        )
        .unwrap();
        (m, gold)
    }

    #[test]
    fn the_clock_rounds_up_and_formats_minutes() {
        assert_eq!(clock_text(0, 60), "0:00");
        assert_eq!(clock_text(1, 60), "0:01");
        assert_eq!(clock_text(60 * 90, 60), "1:30");
        assert_eq!(clock_text(60 * 300 - 1, 60), "5:00");
        assert_eq!(clock_text(5, 0), "0:05", "a zero rate does not divide by 0");
    }

    #[test]
    fn a_timed_free_for_all_shows_the_countdown_rank_and_loose_gold() {
        let (mut m, _) = game(
            CnrVariant::FreeForAll,
            EndRule::Ticks(60 * 300),
            &[(A, Side::Solo), (B, Side::Solo)],
        );
        for _ in 0..(60 * 10) {
            m.tick();
        }
        let lines = scoreboard_lines(&m.view(), Some(A), 60);
        assert_eq!(lines[0], "COPS & ROBBERS  4:50");
        assert_eq!(lines[1], "YOU: 1st of 2  0 pts");
        assert_eq!(lines[2], "THE GOLD IS UP FOR GRABS");
        // Not seated: the match shows, the personal line does not.
        let lines = scoreboard_lines(&m.view(), None, 60);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn a_point_limit_names_the_target_and_a_lone_driver_has_no_rank() {
        let (m, _) = game(
            CnrVariant::FreeForAll,
            EndRule::Points(250),
            &[(A, Side::Solo)],
        );
        let lines = scoreboard_lines(&m.view(), Some(A), 60);
        assert_eq!(lines[0], "COPS & ROBBERS  first to 250");
        assert_eq!(lines[1], "YOU: 0 pts");
    }

    #[test]
    fn a_team_match_shows_both_totals_and_where_each_side_delivers() {
        let (mut m, _) = game(
            CnrVariant::CopsVsRobbers,
            EndRule::None,
            &[(A, Side::Robbers), (B, Side::Cops)],
        );
        let g = m.gold_position().unwrap();
        let contact = |p| mm2_game::gold::Contact {
            player: p,
            position: g,
            round: m.round(),
        };
        m.resolve_pickups(&[contact(A)]);
        let robber = scoreboard_lines(&m.view(), Some(A), 60);
        assert_eq!(robber[0], "COPS & ROBBERS  0:00");
        assert!(robber[1].starts_with("ROBBERS ") && robber[1].contains("COPS "));
        assert_eq!(robber[2], "YOU: ROBBERS  25 pts");
        assert_eq!(robber[3], "YOU HAVE THE GOLD - take it to the hideout");
        // The cop sees a robber, not an id, and no personal claim.
        let cop = scoreboard_lines(&m.view(), Some(B), 60);
        assert_eq!(cop[3], "A ROBBERS DRIVER HAS THE GOLD");
    }

    #[test]
    fn a_cop_is_sent_to_the_bank_and_a_free_for_all_names_the_rival() {
        let (mut m, _) = game(
            CnrVariant::CopsVsRobbers,
            EndRule::None,
            &[(A, Side::Cops), (B, Side::Robbers)],
        );
        let g = m.gold_position().unwrap();
        m.resolve_pickups(&[mm2_game::gold::Contact {
            player: A,
            position: g,
            round: m.round(),
        }]);
        assert_eq!(
            scoreboard_lines(&m.view(), Some(A), 60)[3],
            "YOU HAVE THE GOLD - take it to the bank"
        );

        let (mut m, _) = game(
            CnrVariant::FreeForAll,
            EndRule::None,
            &[(A, Side::Solo), (C, Side::Solo)],
        );
        let g = m.gold_position().unwrap();
        m.resolve_pickups(&[mm2_game::gold::Contact {
            player: C,
            position: g,
            round: m.round(),
        }]);
        assert_eq!(
            scoreboard_lines(&m.view(), Some(A), 60)[2],
            "PLAYER 3 HAS THE GOLD"
        );
    }

    #[test]
    fn a_finished_match_freezes_its_clock_and_announces_the_result() {
        let (mut m, _) = game(
            CnrVariant::FreeForAll,
            EndRule::Ticks(120),
            &[(A, Side::Solo), (B, Side::Solo)],
        );
        for _ in 0..300 {
            m.tick();
        }
        let view = m.view();
        let o = view.outcome.expect("the time limit has passed");
        assert_eq!(o.winner, Winner::Tie);
        let lines = scoreboard_lines(&view, Some(A), 60);
        assert_eq!(lines[0], "COPS & ROBBERS  0:00");
        assert_eq!(lines.last().unwrap(), "MATCH OVER - TIE (time)");

        let mut won = view.clone();
        won.outcome = Some(mm2_game::gold::Outcome {
            winner: Winner::Player(A),
            ..o
        });
        assert_eq!(
            scoreboard_lines(&won, Some(A), 60).last().unwrap(),
            "MATCH OVER - YOU WIN (time)"
        );
        assert_eq!(
            scoreboard_lines(&won, Some(B), 60).last().unwrap(),
            "MATCH OVER - PLAYER 1 WINS (time)"
        );
        won.outcome = Some(mm2_game::gold::Outcome {
            winner: Winner::Side(Side::Cops),
            reason: EndReason::PointLimit,
            ..o
        });
        assert_eq!(
            scoreboard_lines(&won, None, 60).last().unwrap(),
            "MATCH OVER - COPS WIN (point limit)"
        );
    }

    #[test]
    fn the_match_over_body_needs_a_result_and_ranks_everyone() {
        let (mut m, _) = game(
            CnrVariant::CopsVsRobbers,
            EndRule::Ticks(60 * 90),
            &[(A, Side::Robbers), (B, Side::Cops)],
        );
        assert_eq!(result_lines(&m.view(), Some(A), 60), None, "undecided");
        let g = m.gold_position().unwrap();
        m.resolve_pickups(&[mm2_game::gold::Contact {
            player: A,
            position: g,
            round: m.round(),
        }]);
        for _ in 0..(60 * 90) {
            m.tick();
        }
        let lines = result_lines(&m.view(), Some(A), 60).expect("decided");
        assert_eq!(lines[0], "ROBBERS WIN");
        assert_eq!(lines[1], "time - played 1:30");
        assert!(lines[2].starts_with("ROBBERS 25") && lines[2].contains("COPS 0"));
        assert_eq!(lines[3], "1. You (ROBBERS) - 25 pts");
        assert_eq!(lines[4], "2. Player 2 (COPS) - 0 pts");
        // A leaver keeps their row, marked; the other side's verdict
        // is not "you".
        let mut view = m.view();
        view.standings[1].connected = false;
        let lines = result_lines(&view, Some(B), 60).unwrap();
        assert_eq!(lines[0], "ROBBERS WIN");
        assert_eq!(lines[3], "1. Player 1 (ROBBERS) - 25 pts");
        assert_eq!(lines[4], "2. You (COPS) - 0 pts - left");
    }

    #[test]
    fn a_free_for_all_body_has_no_team_line() {
        let (mut m, _) = game(
            CnrVariant::FreeForAll,
            EndRule::Ticks(60),
            &[(A, Side::Solo), (B, Side::Solo)],
        );
        for _ in 0..120 {
            m.tick();
        }
        let lines = result_lines(&m.view(), Some(A), 60).unwrap();
        assert_eq!(lines[0], "TIE");
        assert_eq!(lines.len(), 4, "verdict, reason, two rows");
        assert_eq!(lines[2], "1. You - 0 pts");
    }

    #[test]
    fn ordinals_handle_the_teens() {
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(2), "2nd");
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(21), "21st");
    }

    /// The system end to end: a host's local car sees its own board,
    /// a replica-only process sees the host's, the `H` gate hides it
    /// and a missing match leaves it hidden.
    #[test]
    fn the_readout_follows_the_host_then_the_replica_and_honours_the_hud_gate() {
        let (m, _) = game(
            CnrVariant::FreeForAll,
            EndRule::Points(500),
            &[(A, Side::Solo)],
        );
        let view = m.view();
        let mut app = App::new();
        app.insert_resource(crate::hud::HudVisible(true))
            .add_systems(Update, update_cnr_scoreboard);
        app.world_mut().spawn(Player {
            id: A,
            control: PlayerControl::Local,
        });
        let board = app
            .world_mut()
            .spawn((CnrScoreboard, Text::new(""), Visibility::Hidden))
            .id();
        let read = |app: &App| {
            let w = app.world();
            (
                w.get::<Text>(board).unwrap().0.clone(),
                *w.get::<Visibility>(board).unwrap(),
            )
        };

        app.update();
        assert_eq!(read(&app).1, Visibility::Hidden, "no match, nothing drawn");

        app.insert_resource(CnrReplica(view.clone()));
        app.update();
        let (text, vis) = read(&app);
        assert_eq!(vis, Visibility::Inherited);
        assert!(text.contains("first to 500") && text.contains("YOU: 0 pts"));

        app.insert_resource(crate::hud::HudVisible(false));
        app.update();
        assert_eq!(read(&app).1, Visibility::Hidden);

        // The authority's own match wins over a stale replica.
        let (mut hosted, _) = game(
            CnrVariant::FreeForAll,
            EndRule::Points(250),
            &[(A, Side::Solo)],
        );
        hosted.tick();
        app.insert_resource(crate::hud::HudVisible(true))
            .insert_resource(CnrHost::new(hosted));
        app.update();
        assert!(read(&app).0.contains("first to 250"));
    }
    /// Grant the gold to `who` at the gold's own site.
    fn give_gold(m: &mut GoldMatch, who: PlayerId) {
        let round = m.round();
        let position = m.gold_position().unwrap();
        m.resolve_pickups(&[mm2_game::gold::Contact {
            player: who,
            round,
            position,
        }]);
        assert_eq!(m.carrier(), Some(who));
    }

    #[test]
    fn the_objective_is_the_gold_until_the_local_car_carries_it() {
        let (mut m, _) = game(
            CnrVariant::CopsVsRobbers,
            EndRule::None,
            &[(A, Side::Robbers), (B, Side::Cops)],
        );
        let sites = m.sites();
        // Resting: both sides hunt the gold, and the map marks it.
        for me in [A, B] {
            let o = objective(&m.view(), Some(me), []);
            assert_eq!(o.gold, Some(sites.gold));
            assert_eq!(o.target, Some(sites.gold));
            assert_eq!((o.hideout, o.bank), (sites.hideout, sites.bank));
        }
        // A robber carries it: the robber is sent to the hideout, with
        // no gold dot on its own car; the cop follows the carrier's car.
        give_gold(&mut m, A);
        let cars = [
            (A, Vec3::new(5.0, 0.0, 6.0)),
            (B, Vec3::new(-9.0, 0.0, 2.0)),
        ];
        let mine = objective(&m.view(), Some(A), cars);
        assert_eq!((mine.gold, mine.target), (None, Some(sites.hideout)));
        let cop = objective(&m.view(), Some(B), cars);
        assert_eq!(cop.gold, Some(cars[0].1));
        assert_eq!(cop.target, Some(cars[0].1));
        // A carrier whose car is not in view leaves nothing to point at.
        assert_eq!(
            objective(&m.view(), Some(B), [(B, Vec3::ZERO)]).target,
            None
        );
    }

    #[test]
    fn a_cop_carrying_the_gold_is_sent_to_the_bank() {
        let (mut m, _) = game(
            CnrVariant::CopsVsRobbers,
            EndRule::None,
            &[(A, Side::Robbers), (B, Side::Cops)],
        );
        give_gold(&mut m, B);
        let o = objective(&m.view(), Some(B), [(B, Vec3::ZERO)]);
        assert_eq!(o.target, Some(m.sites().bank));
    }

    #[test]
    fn nobody_is_pointed_anywhere_when_unseated_or_after_the_match() {
        let (m, _) = game(
            CnrVariant::FreeForAll,
            EndRule::Points(100),
            &[(A, Side::Solo)],
        );
        // Not seated (or no local car): the map still has its dots.
        let o = objective(&m.view(), None, []);
        assert_eq!(o.target, None);
        assert_eq!(o.gold, Some(m.sites().gold));
        assert_eq!(objective(&m.view(), Some(C), []).target, None);
        // A decided match points nowhere.
        let mut view = m.view();
        view.outcome = Some(mm2_game::gold::Outcome {
            winner: Winner::Player(A),
            reason: EndReason::PointLimit,
            at_tick: 10,
        });
        assert_eq!(objective(&view, Some(A), []).target, None);
    }

    /// The arrow system end to end on a match with no race: it shows,
    /// turns to the gold's bearing, flips to the behind sprite when the
    /// gold is astern, and follows the carry to the delivery site.
    #[test]
    fn the_nav_arrow_follows_a_cops_and_robbers_match() {
        use crate::navarrow::{NavArrow, NavArrowSprites, update_nav_arrow};
        use avian3d::prelude::{Position, Rotation};
        let (m, _) = game(
            CnrVariant::CopsVsRobbers,
            EndRule::None,
            &[(A, Side::Robbers)],
        );
        let sites = m.sites();
        let mut app = App::new();
        app.init_resource::<Session>()
            .insert_resource(crate::hud::HudVisible(true))
            .insert_resource(CnrHost::new(m))
            .add_systems(Update, update_nav_arrow);
        let ahead = Handle::<Image>::default();
        let node = app
            .world_mut()
            .spawn((
                NavArrow,
                NavArrowSprites {
                    ahead: ahead.clone(),
                    behind: ahead,
                },
                UiTransform::default(),
                ImageNode::default(),
                Visibility::Hidden,
            ))
            .id();
        // Facing −Z (the default rotation) at a spot well off the sites.
        let at = Vec3::new(1000.0, 0.0, 1000.0);
        app.world_mut().spawn((
            Player {
                id: A,
                control: PlayerControl::Local,
            },
            Position(at),
            Rotation::default(),
            GlobalTransform::from_translation(at),
        ));
        let expect = |goal: Vec3| mm2_game::relative_bearing(0.0, at, goal);
        app.update();
        let shown = |app: &App| {
            let w = app.world();
            (
                *w.get::<Visibility>(node).unwrap(),
                w.get::<UiTransform>(node).unwrap().rotation.as_radians(),
            )
        };
        let (vis, rot) = shown(&app);
        assert_eq!(vis, Visibility::Visible);
        assert!((rot - expect(sites.gold)).abs() < 1e-4, "{rot}");

        // Carrying it: the arrow swings to the hideout.
        {
            let mut host = app.world_mut().resource_mut::<CnrHost>();
            give_gold(&mut host.game, A);
        }
        app.update();
        let (_, rot) = shown(&app);
        assert!((rot - expect(sites.hideout)).abs() < 1e-4, "{rot}");

        // The `H` gate hides it without losing the target.
        app.insert_resource(crate::hud::HudVisible(false));
        app.update();
        assert_eq!(shown(&app).0, Visibility::Hidden);

        // No match, no arrow.
        app.insert_resource(crate::hud::HudVisible(true));
        app.world_mut().remove_resource::<CnrHost>();
        app.update();
        assert_eq!(shown(&app).0, Visibility::Hidden);
    }
}
