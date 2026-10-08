//! The app-side driver for the shared race runtime (F11-B).
//!
//! The contract types live in `mm2_game::race`; [`advance_race`] is the
//! producer that feeds them real positions, because only `mm2_app` may
//! see both the game contracts and Avian. It runs in `FixedLast`, after
//! the physics step, so each `Position` it reads is the step the
//! session clock names — the same swept-segment path tests drive and
//! gameplay uses (AC02).
//!
//! Per fixed step, while a [`RaceState`] resource exists and the
//! session authority simulates rules:
//!
//! - session `Countdown` (or `Playing` — a race resource that outlived
//!   its gate still counts down): tick the countdown, release exactly
//!   once — participants flip `AwaitingStart → Racing`, the session
//!   moves `Countdown → Playing`, one [`RaceStarted`] message goes out
//!   (AC03).
//! - session `Playing` + race `Running`: advance the race clock and
//!   every `Racing` participant's swept segment; a `Finished` outcome
//!   mints and records exactly one [`SessionResult`] per participant
//!   per generation into [`ResultLedger`] (AC04). When the definition
//!   carries a `time_limit_ticks` (Blitz, F12-A) the deadline is
//!   inclusive — segments evaluate first, so a finish landing on the
//!   expiry tick itself still counts — then every participant still
//!   unresolved records one [`SessionOutcome::TimedOut`] (DSN-7).
//!   All-resolved marks the race `Complete`. A *local* participant's
//!   terminal resolution (`Finished`/`TimedOut`) moves the session
//!   `Playing → Results` (UI-5: a results screen follows each race) —
//!   deferred on a networked authority while a wire seat still races:
//!   the wire input feed and `publish_snapshots` a remote client lives
//!   on gate on `Playing`, so ending on the local edge alone would
//!   strand every unresolved remote — inputs, standings and its own
//!   terminal edge included. The deferral ends
//!   when the last `Remote` participant resolves, and one
//!   `Results`-phase publish carries the final rows. A remote/AI
//!   participant resolving while the local driver still races changes
//!   nothing.
//! - session `Results` + race `Running`: the same stepping for the
//!   participants still racing — the field races on behind the
//!   results screen (DSN-11), the clock runs on so a late finish
//!   records its real time, and the deadline still times out the rest.
//! - anything else (`Paused`, `Unloading`, …): frozen — the race clock
//!   and every swept segment hold still, so pause/resume is
//!   deterministic and no timer runs during teardown.
//!
//! A stale `RaceState` (generation mismatch, e.g. the frames between a
//! restart's `begin` and teardown's resource removal) is never stepped.
//!
//! [`reanchor_teleported_participants`] runs chained before
//! [`advance_race`] in `FixedLast`: a `ResetVehicle` teleport is not
//! motion, so the swept segment must be re-anchored rather than counted
//! as a crossing (AC02).

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_game::{
    CheckpointRule, Difficulty, EventRef, ParticipantState, Player, PlayerControl, ProgressOutcome,
    RACE_TICK_HZ, RaceDefinition, RacePhase, RaceProgress, RaceStarted, RaceState, ResultLedger,
    RouteGateLine, Session, SessionAuthority, SessionEntity, SessionOutcome, SessionPhase,
    SessionResult,
};
use mm2_vehicle::Teleported;
use tracing::warn;

/// Why an `EventRef` cannot become a live race.
#[derive(Debug, thiserror::Error)]
pub enum EventSetupError {
    /// Catalog lookup failed — unknown row, wrong city, or required
    /// records missing (the resolve error carries the detail).
    #[error("event resolve failed: {0}")]
    Resolve(#[from] mm2_content::EventResolveError),
    /// The resolved event's records could not produce a runnable race.
    #[error("race definition failed: {0}")]
    Build(#[from] mm2_content::RaceBuildError),
    /// The event's selected `.aimap` record resolved but is unreadable
    /// or malformed. Refused rather than raced without opponents: a
    /// mod's broken lineup must not pass for a working one (F29-AC04).
    /// Every retail aimap parses, so this only ever names a mod's file.
    #[error("event aimap failed: {0}")]
    Aimap(mm2_content::RosterBuildError),
}

/// An event session's authored content beyond the race definition —
/// the `.pathset` overlay records the event's stem owns (F03-AC04:
/// course barricades, jumps, prop arrangements stamped only while the
/// event session lives) and the difficulty-selected opponent lineup
/// (F15-A).
pub struct EventSetup {
    /// The shared race runtime definition.
    pub definition: RaceDefinition,
    /// The event's stable save identity (F16): city + table + the
    /// authored file stem — the key `ProfileProgress` records and
    /// `selections.last_event` use, so a mod inserting a table row
    /// cannot retarget a saved record.
    pub key: mm2_game::EventKey,
    /// Logical paths of the event's `.pathset` records, in catalog
    /// order (records are stored sorted by logical path).
    pub pathsets: Vec<String>,
    /// The event's authored opponent lineup at the session difficulty.
    /// A roster that fails to build degrades to empty — the race still
    /// runs, with the failure logged. A present-but-malformed `.aimap`
    /// is not such a case: it is refused ([`EventSetupError::Aimap`]).
    pub roster: mm2_game::OpponentRoster,
    /// The event's authored `[Police]` lineup at the session difficulty
    /// (F20-A.1), read from the same aimap record as `roster`. Degrades
    /// to empty like `roster` — no cops, the failure logged.
    pub police: mm2_game::PoliceRoster,
    /// The `[Opponent]` lead cars a Crash Course lesson wires (F21-B.19)
    /// — fielded as non-participants that drive their `.opp` route
    /// without a `RaceProgress`. Always empty for a race event, whose
    /// opponents are `roster`.
    pub lead_cars: mm2_game::OpponentRoster,
    /// The city's normalized reward surface (F16-B): the authored
    /// `<city>_rewards.csv` rules the result consumer grants unlocks
    /// from, plus the authored family sizes `half`/`all` measure.
    pub rewards: mm2_game::RewardTable,
    /// The city's derived availability surface (F16-B): which authored
    /// events a progressing profile may select and what gates the
    /// rest. Enforcement is F17's menu flow — until then this is the
    /// honest record of what a `--event` launch bypassed.
    pub availability: mm2_game::AvailabilityTable,
    /// The event's difficulty-selected aimap, parsed — ambient-traffic
    /// overrides (`[Ambient Types/Density]`, `[Density]`,
    /// `[Exceptions]`, `[Speed Limit]`) live in the same record the
    /// opponent roster reads. `None` when no record resolves or parses
    /// — ambient setup then runs on the city aimap alone.
    pub aimap: Option<mm2_formats::aimap::Aimap>,
}

/// Resolve an `EventRef` through the VFS into the event's runtime
/// setup: catalog scan → dependency-checked resolve → the
/// `mm2_content` race-definition producer plus the authored overlay
/// records the event owns. Called once per event session load, so the
/// catalog stays a load-time object rather than a resource.
pub fn event_race_setup(
    vfs: &Vfs,
    event_ref: &EventRef,
    difficulty: Difficulty,
) -> Result<EventSetup, EventSetupError> {
    let catalog = mm2_content::EventCatalog::scan(vfs, &event_ref.city);
    let event = catalog.resolve(event_ref)?;
    let definition = mm2_content::race_definition(event, difficulty)?;
    let pathsets = event
        .records
        .iter()
        .filter(|r| r.kind == mm2_formats::racefiles::RaceFileKind::Pathset)
        .map(|r| r.logical.clone())
        .collect();
    // One aimap resolution+parse feeds both consumers: the roster
    // reads its `[Opponent]` rows, the ambient setup its traffic
    // overrides. A record that is present but unreadable or malformed
    // is refused; only an absent record degrades both to empty/None —
    // the race still runs, with the failure logged.
    let (roster, police, aimap) = match mm2_content::event_aimap(vfs, event, difficulty) {
        Ok((aimap, picked)) => {
            let police =
                match mm2_content::police_roster_from_aimap(event, difficulty, &aimap, &picked) {
                    Ok(police) => police,
                    Err(e) => {
                        warn!(error = %e, "police roster failed to build — no police");
                        mm2_game::PoliceRoster::default()
                    }
                };
            let roster = match mm2_content::opponent_roster_from_aimap(
                event, difficulty, &aimap, &picked,
            ) {
                Ok(roster) => roster,
                Err(e) => {
                    warn!(error = %e, "opponent roster failed to build — racing without opponents");
                    mm2_game::OpponentRoster::default()
                }
            };
            (roster, police, Some(aimap))
        }
        Err(mm2_content::RosterBuildError::NoAimapRecord) => {
            warn!(
                "event has no aimap record — racing without opponents; city ambient defaults apply"
            );
            (
                mm2_game::OpponentRoster::default(),
                mm2_game::PoliceRoster::default(),
                None,
            )
        }
        Err(e) => return Err(EventSetupError::Aimap(e)),
    };
    let rewards = mm2_content::reward_table(&catalog);
    for d in &rewards.diagnostics {
        warn!(diagnostic = %d, "reward row did not become a rule");
    }
    let availability = mm2_content::availability_table(&catalog);
    for d in &availability.diagnostics {
        warn!(diagnostic = %d, "event row did not become an availability gate");
    }
    Ok(EventSetup {
        definition,
        key: mm2_game::EventKey {
            city: event.event_ref.city.clone(),
            table: event.event_ref.table,
            stem: event.stem.clone(),
        },
        pathsets,
        roster,
        police,
        lead_cars: mm2_game::OpponentRoster::default(),
        rewards,
        availability,
        aimap,
    })
}

/// Why a Crash Course row cannot become a lesson run.
#[derive(Debug, thiserror::Error)]
pub enum LessonSetupError {
    /// Catalog lookup failed (unknown row, wrong city, missing records).
    #[error("event resolve failed: {0}")]
    Resolve(#[from] mm2_content::EventResolveError),
    /// The row resolves but is not a Crash Course lesson.
    #[error("{0:?}[{1}] is not a Crash Course row")]
    NotCrashCourse(mm2_game::EventTableKind, usize),
    /// A sub-event could not become a runnable leg — the whole lesson
    /// is refused rather than shortened.
    #[error("lesson legs failed: {0}")]
    Legs(#[from] mm2_content::LessonBuildError),
    /// The lesson has no legs to run.
    #[error(transparent)]
    NoLegs(#[from] mm2_game::NoLegs),
}

/// A Crash Course lesson's runnable setup (F21-B): the stable save
/// identity, the legs in authored order and the sequencer over them.
/// Each leg's `definition` is what the shared race runtime runs; the
/// session loader feeds the finish/expiry/disable verdict back through
/// [`mm2_game::LegReport::from_gate_run`] into `run`.
pub struct LessonSetup {
    /// The lesson's stable identity (city + table + file stem).
    pub key: mm2_game::EventKey,
    /// The legs, in authored order (`DSN-72`).
    pub legs: Vec<mm2_content::LessonLeg>,
    /// The sequencer over `legs` (`DSN-73`), on its first attempt.
    pub run: mm2_game::LessonRun,
    /// The city's normalized reward rules (CC-6's `crash,N` rows) —
    /// what a lesson's pass credit grants from. Leg clears never reach
    /// it (`record_session_results` keeps them off the profile).
    pub rewards: mm2_game::RewardTable,
    /// The city's derived availability surface (CC-3's chain) — the
    /// same table the menu gates a midterm/final on.
    pub availability: mm2_game::AvailabilityTable,
    /// The lesson's own difficulty-selected `crash<N>.aimap{,_p}`,
    /// parsed — the ambient-traffic layer the lesson authors (sf
    /// crash1/2/4/12 set a default speed limit of 15 and ten per-road
    /// `[Exceptions]` limits of 35, CC-7).
    /// The traffic overrides ride on the launch setup; the lesson's
    /// `[Police]` rows become [`LessonSetup::police`] and its
    /// `[Opponent]` lead cars [`LessonSetup::lead_cars`]. `None` when no
    /// record resolves or parses — the lesson then runs on the city's
    /// own aimap.
    pub aimap: Option<mm2_formats::aimap::Aimap>,
    /// The lesson's authored `[Police]` lineup at the chosen difficulty
    /// (F21-B.18; the cop-chase lessons — retail london crash10/11, sf
    /// crash5/7). Empty for every other lesson, and when the aimap is
    /// unreadable. The lineup is the lesson's, not a leg's: it stands at
    /// its posts for the whole session.
    pub police: mm2_game::PoliceRoster,
    /// The lesson's authored `[Opponent]` lead cars at the chosen
    /// difficulty (F21-B.19; the follow and stop lessons, london crash3
    /// and crash11 among them). Empty for a lesson that wires none, and
    /// when the aimap is unreadable. Like the police, the lineup is the
    /// lesson's, not a leg's: it drives its route for the whole session.
    pub lead_cars: mm2_game::OpponentRoster,
}

/// Resolve a Crash Course `EventRef` into its lesson legs and
/// sequencer: catalog scan → dependency-checked resolve → the lesson
/// view of the row → one `RaceDefinition` per sub-event at
/// `difficulty`. This is the Crash Course counterpart of
/// [`event_race_setup`], which still refuses these rows; nothing
/// launches a lesson through [`lesson_launch`], and a leg's clear is
/// the gate-run baseline, not a family evaluator (`UNK-35`).
pub fn lesson_race_setup(
    vfs: &Vfs,
    event_ref: &EventRef,
    difficulty: Difficulty,
) -> Result<LessonSetup, LessonSetupError> {
    let catalog = mm2_content::EventCatalog::scan(vfs, &event_ref.city);
    let event = catalog.resolve(event_ref)?;
    if event.event_ref.table != mm2_game::EventTableKind::CrashCourse {
        return Err(LessonSetupError::NotCrashCourse(
            event.event_ref.table,
            event.event_ref.index,
        ));
    }
    let lesson = mm2_content::crash_lesson(vfs, &catalog, event);
    let legs = mm2_content::lesson_legs(&catalog, &lesson, difficulty)?;
    let run = mm2_game::LessonRun::new(legs.len())?;
    let (aimap, police, lead_cars) = match mm2_content::event_aimap(vfs, event, difficulty) {
        Ok((aimap, picked)) => {
            let police = mm2_content::lesson_police_roster(event, difficulty, &aimap, &picked)
                .unwrap_or_else(|e| {
                    warn!(error = %e, "lesson police roster failed to build — no police");
                    mm2_game::PoliceRoster::default()
                });
            let lead_cars =
                mm2_content::lesson_opponent_roster(vfs, event, difficulty, &aimap, &picked)
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "lesson lead-car roster failed to build — no lead cars");
                        mm2_game::OpponentRoster::default()
                    });
            (Some(aimap), police, lead_cars)
        }
        Err(mm2_content::RosterBuildError::NoAimapRecord) => (
            None,
            mm2_game::PoliceRoster::default(),
            mm2_game::OpponentRoster::default(),
        ),
        Err(e) => {
            warn!(error = %e, "lesson aimap unreadable — city ambient defaults apply");
            (
                None,
                mm2_game::PoliceRoster::default(),
                mm2_game::OpponentRoster::default(),
            )
        }
    };
    Ok(LessonSetup {
        key: mm2_game::EventKey {
            city: event.event_ref.city.clone(),
            table: event.event_ref.table,
            stem: event.stem.clone(),
        },
        legs,
        run,
        rewards: mm2_content::reward_table(&catalog),
        availability: mm2_content::availability_table(&catalog),
        aimap,
        police,
        lead_cars,
    })
}

/// Resolve a Crash Course `EventRef` into what the session loader
/// installs for a lesson (F21-B.6, `DSN-75`): the loader's usual
/// [`EventSetup`] shape carrying leg 0's definition and the lesson's
/// stable key, plus the [`LessonDriver`](crate::lesson::LessonDriver)
/// that swaps in the later legs. A lesson fields no race
/// opponents; its own `[Police]` lineup (cop-chase lessons, F21-B.18)
/// rides as `police` and its `[Opponent]` lead cars (F21-B.19) as
/// `lead_cars`, and it stamps no `crash<N>.pathset` event overlay
/// (retail authors none; the `<object>_crash<N>` bridge/parked-car/
/// ferry sets load through the object managers off the lesson's key
/// stem). The lesson's own aimap rides along as `aimap`, so its
/// ambient-traffic overrides apply; the city's reward and availability
/// tables ride along so the lesson's *pass* (never a leg clear)
/// credits the profile (`record_session_results`); the driver is
/// built fresh per call, so a session restart re-enters on leg 0 with
/// cleared counters.
pub fn lesson_launch(
    vfs: &Vfs,
    event_ref: &EventRef,
    difficulty: Difficulty,
) -> Result<(EventSetup, crate::lesson::LessonDriver), LessonSetupError> {
    let mut lesson = lesson_race_setup(vfs, event_ref, difficulty)?;
    let rewards = std::mem::take(&mut lesson.rewards);
    let availability = std::mem::take(&mut lesson.availability);
    let aimap = lesson.aimap.take();
    let police = std::mem::take(&mut lesson.police);
    let lead_cars = std::mem::take(&mut lesson.lead_cars);
    let driver = crate::lesson::LessonDriver::new(lesson);
    let definition = driver
        .current_leg()
        .expect("a fresh lesson run is on leg 0")
        .definition
        .clone();
    let setup = EventSetup {
        definition,
        key: driver.key().clone(),
        pathsets: Vec::new(),
        roster: mm2_game::OpponentRoster::default(),
        police,
        lead_cars,
        rewards,
        availability,
        aimap,
    };
    Ok((setup, driver))
}

/// Marker on a session-owned checkpoint/finish marker entity —
/// [`update_checkpoint_markers`] reads it to reflect per-participant
/// progress on the mesh.
#[derive(Component, Debug, Clone, Copy)]
pub struct CheckpointMarker {
    /// `Some(i)` = gate `i` of `RaceState::definition.checkpoints`;
    /// `None` = the finish trigger.
    pub gate: Option<usize>,
}

/// Spawn decorative checkpoint gantries. No colliders: the shared swept
/// trigger tests still own crossing correctness. Session-owned roots control
/// visibility for all metalwork and signs together.
pub fn spawn_checkpoint_markers(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    definition: &RaceDefinition,
    owner: SessionEntity,
) {
    crate::checkpoint_gate::spawn(commands, meshes, images, materials, definition, owner);
}

/// Reflect progress on the markers: a cleared `AnyOrder` gate hides;
/// the finish stays hidden until every gate is cleared — RACE-7's
/// "the finish line appears" rule, visible, not just modeled.
/// `Ordered` gates stay visible (they reset each lap).
pub fn update_checkpoint_markers(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
    participants: Query<(&Player, &RaceProgress)>,
    mut markers: Query<(&CheckpointMarker, &mut Visibility)>,
) {
    let Some(race) = race else { return };
    if race.is_stale(session.generation()) {
        return;
    }
    // The markers show the local driver's view of the course — the
    // same disambiguation `update_race_warning` uses, since AI and
    // remote participants carry `Player` too and a plain `iter().next()`
    // would follow whichever archetype iterates first.
    let progress = participants
        .iter()
        .find(|(p, _)| p.control == PlayerControl::Local)
        .map(|(_, progress)| progress);
    let def = &race.definition;
    for (marker, mut vis) in &mut markers {
        let show = match marker.gate {
            Some(i) => match def.rule {
                CheckpointRule::Ordered => true,
                CheckpointRule::AnyOrder => !progress.is_some_and(|p| p.is_cleared(i)),
            },
            None => {
                matches!(race.phase, RacePhase::Complete)
                    || progress.is_some_and(|p| p.cleared_count() >= def.checkpoints.len())
            }
        };
        *vis = if show {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Re-anchor swept segments on teleported participants.
///
/// `vehicle_reset` marks each entity it teleports with [`Teleported`]
/// in the same pass that writes the new `Position`; a participant's
/// swept segment must break on that jump rather than consume every
/// checkpoint between the two poses (AC02 — the reset-near-finish
/// edge). The marker is consumed here, before [`advance_race`] in
/// `FixedLast`, so it applies in every session phase — a reset while
/// paused or mid-countdown still lands. Markers on entities without
/// `RaceProgress` are inert and despawn with the entity.
pub fn reanchor_teleported_participants(
    mut commands: Commands,
    mut participants: Query<(Entity, &mut RaceProgress), With<Teleported>>,
) {
    for (entity, mut progress) in &mut participants {
        progress.break_segment();
        commands.entity(entity).remove::<Teleported>();
    }
}

/// Fixed-step race driver — see module docs.
pub fn advance_race(
    race: Option<ResMut<RaceState>>,
    lesson: Option<Res<crate::lesson::LessonDriver>>,
    mut session: ResMut<Session>,
    mut ledger: ResMut<ResultLedger>,
    mut started: MessageWriter<RaceStarted>,
    mut participants: Query<(
        &Player,
        &Position,
        &mut RaceProgress,
        Option<&RouteGateLine>,
    )>,
) {
    let Some(mut race) = race else {
        return;
    };
    if race.is_stale(session.generation()) || !session.authority_role().is_authority() {
        return;
    }
    match *session.phase() {
        // The race clock and its triggers only live while the session
        // runs them — `Results` included, where the field races on
        // behind the results screen (DSN-11); Paused/Unloading freeze
        // everything.
        SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results => {}
        _ => return,
    }
    match &mut race.phase {
        RacePhase::Countdown { remaining } => {
            if *remaining > 0 {
                *remaining -= 1;
            }
            if *remaining > 0 {
                return;
            }
            race.phase = RacePhase::Running;
            for (_, _, mut progress, _) in &mut participants {
                if progress.state == ParticipantState::AwaitingStart {
                    progress.state = ParticipantState::Racing;
                }
            }
            if *session.phase() == SessionPhase::Countdown {
                session
                    .transition(SessionPhase::Playing)
                    .expect("Countdown → Playing is a legal transition");
            }
            started.write(RaceStarted);
        }
        RacePhase::Running => {
            // A race released by another path while the session still
            // counts down waits for the session — the clock only runs
            // while the field races (`Playing`, then `Results`).
            if !session.field_races() {
                return;
            }
            race.clock += 1;
            let mut pending = false;
            for (player, position, mut progress, line) in &mut participants {
                // A wire seat spawned after the countdown's release
                // flip — the reconcile's spawn lands on the next
                // command flush, which can sit past the releasing
                // fixed step — never saw it. It races on arrival like
                // `RaceProgress::join` scores a mid-race joiner rather
                // than pinning the race pending on a seat that can
                // never start.
                if progress.state == ParticipantState::AwaitingStart
                    && player.control == PlayerControl::Remote
                {
                    progress.state = ParticipantState::Racing;
                }
                // AwaitingStart during Running counts as pending: the
                // race cannot complete with a participant that never
                // started. Finished participants are done.
                if progress.state == ParticipantState::AwaitingStart {
                    pending = true;
                    continue;
                }
                if !matches!(progress.state, ParticipantState::Racing) {
                    continue;
                }
                let mut outcome = progress.advance(&race.definition, position.0);
                // Route-bound Ordered progress (DSN-45): an AI
                // participant whose authored line never threads a
                // gate's cylinder still earns it by driving past the
                // bound route position — physical crossings credit
                // first wherever the line does thread it.
                if outcome == ProgressOutcome::Racing
                    && let Some(line) = line
                {
                    outcome = progress.advance_route(&race.definition, line);
                }
                if outcome == ProgressOutcome::Finished {
                    let id = session.mint_result_id(player.id);
                    let result = SessionResult {
                        id: id.clone(),
                        tick: session.tick(),
                        outcome: SessionOutcome::Finished {
                            race_ticks: race.clock,
                        },
                    };
                    match ledger.record(result) {
                        Ok(()) => {}
                        // Impossible through this path — the id is minted
                        // fresh — but a rejected record is never retried
                        // silently: the finish is still terminal so it
                        // cannot re-emit every step.
                        Err(dup) => warn!(duplicate = %dup, "race result rejected"),
                    }
                    progress.state = ParticipantState::Finished {
                        race_ticks: race.clock,
                        result: id,
                    };
                } else {
                    pending = true;
                }
            }
            // The deadline is inclusive (DSN-7): the finish check above
            // already ran on this tick, so a crossing landing exactly
            // when the clock reaches the limit counts — only
            // participants still unresolved after it record `TimedOut`,
            // once each (BLZ-1/BLZ-5).
            if race
                .definition
                .time_limit_ticks
                .is_some_and(|limit| race.clock >= u64::from(limit))
            {
                for (player, _, mut progress, _) in &mut participants {
                    if matches!(
                        progress.state,
                        ParticipantState::Racing | ParticipantState::AwaitingStart
                    ) {
                        let id = session.mint_result_id(player.id);
                        let result = SessionResult {
                            id: id.clone(),
                            tick: session.tick(),
                            outcome: SessionOutcome::TimedOut {
                                race_ticks: race.clock,
                            },
                        };
                        if let Err(dup) = ledger.record(result) {
                            warn!(duplicate = %dup, "race result rejected");
                        }
                        progress.state = ParticipantState::TimedOut {
                            race_ticks: race.clock,
                            result: id,
                        };
                    }
                }
                pending = false;
            }
            if !pending && !participants.is_empty() {
                race.phase = RacePhase::Complete;
            }
            // UI-5's results-screen rule: the local driver's terminal
            // resolution ends the playing session. The rest of the
            // field races on behind the results screen — this system
            // keeps stepping in `Results`, so every opponent still
            // earns a recorded finish (DSN-11); the guard on
            // `is_playing` makes the edge fire once. A non-local
            // participant resolving while the local driver still races
            // never ends the local race.
            //
            // Networked-authority deferral (F25-B): on a *hosted*
            // session, `Playing → Results` waits until no `Remote`
            // participant is unresolved. What a remote client lives on
            // gates on `Playing` — the wire input feed (an unresolved
            // remote car would coast, never earning its terminal edge)
            // and `publish_snapshots` (even the rows minted on the
            // transition tick itself would die with the stream). The
            // deferral ends when the last wire seat resolves — the
            // deadline's mass-timeout clears them all in one pass — or
            // departs (a despawned entity stops counting). The
            // transition's own fixed step is the terminal rows' mint,
            // so `publish_snapshots` owes the wire exactly one
            // `Results`-phase frame at the frozen transition tick. A
            // wire seat that goes silent while `Playing` does not hold
            // this forever: `netdrive::retire_stalled_wire_seats` mints
            // its `TimedOut` past `WireStall`'s designed bounds, which
            // lands here as an ordinary resolution — the lobby link,
            // roster slot and parked car all stay (a retirement, not a
            // kick). A `Local`
            // session keeps UI-5's edge exactly: `Remote` there is a
            // simulated opponent's stamp, not a wire seat.
            let hosted = session
                .config()
                .is_some_and(|c| c.authority == SessionAuthority::Host);
            let local_done = participants.iter().any(|(player, _, progress, _)| {
                player.control == PlayerControl::Local
                    && matches!(
                        progress.state,
                        ParticipantState::Finished { .. } | ParticipantState::TimedOut { .. }
                    )
            });
            let wire_open = hosted
                && participants.iter().any(|(player, _, progress, _)| {
                    player.control == PlayerControl::Remote
                        && matches!(
                            progress.state,
                            ParticipantState::AwaitingStart | ParticipantState::Racing
                        )
                });
            // A Crash Course lesson clearing a non-final leg carries on
            // into the next one — only its last clear or a failure
            // ends the session (F21-B.4, `drive_lesson`).
            let lesson_continues = lesson.as_ref().is_some_and(|lesson| {
                participants.iter().any(|(player, _, progress, _)| {
                    player.control == PlayerControl::Local && lesson.holds_results(&progress.state)
                })
            });
            if local_done && !wire_open && !lesson_continues && session.is_playing() {
                session
                    .transition(SessionPhase::Results)
                    .expect("Playing → Results is a legal transition");
            }
        }
        RacePhase::Complete => {}
    }
}

/// How long the `GO!` cue stays up after the countdown releases —
/// measured in [`RaceState::clock`] ticks, the same authoritative
/// clock results timestamp, so the flash freezes with a pause and
/// ends deterministically. Designed presentation (DSN-19): no
/// documented original rule pins the start cue's look; the 3 s
/// countdown itself is DSN-5's provisional default.
pub const COUNTDOWN_GO_TICKS: u64 = RACE_TICK_HZ as u64;

/// Digit color while the start countdown runs.
pub const COUNTDOWN_DIGIT: Color = Color::srgb(1.0, 0.85, 0.2);

/// `GO!` color — the RACE-6 ahead green the authored arrow's
/// `s_hudarrow_green` tile carries.
pub const COUNTDOWN_GO: Color = Color::srgb(0.2, 1.0, 0.4);

/// Marker on the session-owned countdown banner's root — a full-screen
/// flex node that centers the cue [`update_countdown_banner`] writes
/// into [`CountdownBannerText`]. Dev-rig presentation (DSN-19), not a
/// claim about the original's HUD.
#[derive(Component)]
pub struct CountdownBanner;

/// Marker on the banner's text child — carries the digit/`GO!` label.
#[derive(Component)]
pub struct CountdownBannerText;

/// Spawn the countdown banner, session-owned and hidden until
/// [`update_countdown_banner`] drives it. A cruise session never shows
/// it — no `RaceState` exists — and session teardown removes it with
/// every other `SessionEntity` root.
pub fn spawn_countdown_banner(commands: &mut Commands, owner: SessionEntity) {
    commands
        .spawn((
            owner,
            CountdownBanner,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_child((
            CountdownBannerText,
            Text::new(""),
            TextFont {
                font_size: bevy::text::FontSize::Px(120.0),
                ..default()
            },
            TextColor(COUNTDOWN_DIGIT),
        ));
}

/// Drive the countdown cue off the authoritative race state: while the
/// race counts down, the banner shows `ceil(remaining / RACE_TICK_HZ)`
/// — one second per digit, matching the HUD line's convention — and
/// once released it flashes `GO!` for [`COUNTDOWN_GO_TICKS`] of the
/// race clock. Hidden whenever no live race wants it: a stale or
/// `Complete` race, a `remaining` of 0 (a zero-length countdown shows
/// only `GO!`), or a session phase where the cue does not belong —
/// `GO!` is a `Playing`-phase flash, so it cannot linger under the
/// pause or results overlays even when the frozen race clock still
/// sits inside the window. While the `H` HUD gate is off the cue
/// keeps computing but stays hidden (F22-A.3).
pub fn update_countdown_banner(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
    hud: Res<crate::hud::HudVisible>,
    mut banner: Query<&mut Visibility, With<CountdownBanner>>,
    mut text: Query<(&mut Text, &mut TextColor), With<CountdownBannerText>>,
) {
    let cue = race
        .filter(|r| !r.is_stale(session.generation()))
        .and_then(|r| match r.phase {
            RacePhase::Countdown { remaining } if remaining > 0 => Some((
                format!("{}", remaining.div_ceil(RACE_TICK_HZ)),
                COUNTDOWN_DIGIT,
            )),
            RacePhase::Running if session.is_playing() && r.clock < COUNTDOWN_GO_TICKS => {
                Some(("GO!".to_string(), COUNTDOWN_GO))
            }
            _ => None,
        });
    for mut vis in &mut banner {
        *vis = if cue.is_some() && hud.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if let Some((label, color)) = cue {
        for (mut t, mut c) in &mut text {
            *t = Text::new(label.clone());
            c.0 = color;
        }
    }
}

/// Remaining time at which the low-time warning starts pulsing —
/// a designed cue (DSN-9): the ledger documents no original Blitz
/// low-time warning (HUD-2 lists only the countdown timer), so 10 s
/// at the race tick rate is a presentation policy, not an
/// original-behavior claim. Inclusive, matching the deadline's own
/// convention.
pub const LOW_TIME_TICKS: u32 = 10 * RACE_TICK_HZ;

/// Half-period of the low-time pulse in race ticks (0.5 s). The
/// bright/dim phase is derived from the remaining ticks themselves,
/// so the pulse freezes with a pause and cannot drift from the
/// deadline clock (F12-AC04).
pub const LOW_TIME_FLASH_TICKS: u32 = RACE_TICK_HZ / 2;

/// Warning text color on the bright half of the pulse.
pub const LOW_TIME_BRIGHT: Color = Color::srgb(1.0, 0.28, 0.16);

/// Warning text color on the dim half — the banner pulses between
/// bright and dim rather than blinking out, so the cue stays readable
/// the whole warning window.
pub const LOW_TIME_DIM: Color = Color::srgb(0.6, 0.18, 0.12);

/// Marker on the session-owned low-time warning banner — a `LOW TIME`
/// text line under the nav arrow, spawned for event sessions and
/// driven by [`update_race_warning`]. Dev-rig presentation (DSN-9),
/// not a claim about the original's HUD.
#[derive(Component)]
pub struct LowTimeWarning;

/// Spawn the low-time warning banner, session-owned and hidden until
/// [`update_race_warning`] arms it. Untimed definitions never produce
/// a `time_remaining`, so it simply stays dark for them.
pub fn spawn_race_warning(commands: &mut Commands, owner: SessionEntity) {
    commands.spawn((
        owner,
        LowTimeWarning,
        Text::new("LOW TIME"),
        TextFont {
            font_size: bevy::text::FontSize::Px(26.0),
            ..default()
        },
        TextColor(LOW_TIME_BRIGHT),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            // Under the authored race timer (F22-A.4), which ends
            // ~160 px — the 108 px slot the warning used to take now
            // sits inside the timer plate.
            top: Val::Px(170.0),
            left: Val::Percent(50.0),
            margin: UiRect::left(Val::Px(-62.0)),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
            ..default()
        },
        Visibility::Hidden,
    ));
}

/// Drive the low-time warning off the authoritative race clock: while
/// a timed race runs and the local participant is still unresolved,
/// the banner pulses once [`RaceState::time_remaining`] reaches
/// [`LOW_TIME_TICKS`], alternating [`LOW_TIME_BRIGHT`]/[`LOW_TIME_DIM`]
/// every [`LOW_TIME_FLASH_TICKS`] of remaining time. Hidden whenever
/// no timed race is live — stale or `Complete` races, countdowns,
/// untimed definitions, or an already-resolved local participant.
/// While the `H` HUD gate is off the pulse keeps computing but stays
/// hidden (F22-A.3). Under the reduced-flashing option the banner stays
/// on [`LOW_TIME_BRIGHT`] for the whole window instead of pulsing.
pub fn update_race_warning(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
    hud: Res<crate::hud::HudVisible>,
    settings: Option<Res<crate::settings::GraphicsSettings>>,
    participants: Query<(&Player, &RaceProgress)>,
    mut warning: Query<(&mut Visibility, &mut TextColor), With<LowTimeWarning>>,
) {
    let live = race
        .filter(|r| !r.is_stale(session.generation()))
        .filter(|r| r.phase == RacePhase::Running);
    let unresolved_local = participants.iter().any(|(player, progress)| {
        player.control == PlayerControl::Local
            && matches!(
                progress.state,
                ParticipantState::AwaitingStart | ParticipantState::Racing
            )
    });
    let remaining = match (live, unresolved_local) {
        (Some(race), true) => race.time_remaining(),
        _ => None,
    };
    let steady = settings.is_some_and(|s| s.reduce_flashing);
    for (mut vis, mut color) in &mut warning {
        match remaining {
            Some(t) if t <= LOW_TIME_TICKS && hud.0 => {
                *vis = Visibility::Visible;
                let phase = ((LOW_TIME_TICKS - t) / LOW_TIME_FLASH_TICKS) % 2;
                color.0 = if phase == 0 || steady {
                    LOW_TIME_BRIGHT
                } else {
                    LOW_TIME_DIM
                };
            }
            _ => *vis = Visibility::Hidden,
        }
    }
}
