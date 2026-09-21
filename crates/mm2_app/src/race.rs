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
//!   terminal resolution (`Finished`/`TimedOut`) also moves the session
//!   `Playing → Results` on the same step (UI-5: a results screen
//!   follows each race) — a remote/AI participant resolving while the
//!   local driver still races changes nothing.
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
    Checkpoint, CheckpointRule, Difficulty, EventRef, ParticipantState, Player, PlayerControl,
    ProgressOutcome, RACE_TICK_HZ, RaceDefinition, RacePhase, RaceProgress, RaceStarted, RaceState,
    ResultLedger, Session, SessionEntity, SessionOutcome, SessionPhase, SessionResult,
    TargetSelection, cycle_target, navigation_target, relative_bearing,
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
    /// runs, with the failure logged — since a missing `.aimap` never
    /// blocks an otherwise runnable event.
    pub roster: mm2_game::OpponentRoster,
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
    // overrides. A record that fails degrades both to empty/None —
    // the race still runs, with the failure logged.
    let (roster, aimap) = match mm2_content::event_aimap(vfs, event, difficulty) {
        Ok((aimap, picked)) => {
            let roster = match mm2_content::opponent_roster_from_aimap(
                event, difficulty, &aimap, &picked,
            ) {
                Ok(roster) => roster,
                Err(e) => {
                    warn!(error = %e, "opponent roster failed to build — racing without opponents");
                    mm2_game::OpponentRoster::default()
                }
            };
            (roster, Some(aimap))
        }
        Err(e) => {
            warn!(error = %e, "event aimap unreadable — racing without opponents; city ambient defaults apply");
            (mm2_game::OpponentRoster::default(), None)
        }
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
        rewards,
        availability,
        aimap,
    })
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

/// Spawn one translucent cylinder per trigger — gates orange, the
/// finish green. Decorative only: they carry no collider and the
/// swept-segment tests in `Checkpoint::crossed` own correctness. All
/// are stamped `owner` so session teardown removes them (AC03).
pub fn spawn_checkpoint_markers(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    definition: &RaceDefinition,
    owner: SessionEntity,
) {
    let gate_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.55, 0.1, 0.28),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let finish_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.2, 1.0, 0.4, 0.35),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let mut spawn_gate = |cp: &Checkpoint, gate: Option<usize>, mat: &Handle<StandardMaterial>| {
        let mesh = meshes.add(Cylinder::new(cp.radius, cp.height));
        commands.spawn((
            owner,
            CheckpointMarker { gate },
            Mesh3d(mesh),
            MeshMaterial3d(mat.clone()),
            // The finish starts hidden — it only appears once every
            // gate is cleared (RACE-7).
            if gate.is_none() {
                Visibility::Hidden
            } else {
                Visibility::Visible
            },
            // The trigger band is ±height around the authored point;
            // show the above-ground half so the gate reads as a column.
            Transform::from_translation(cp.center + Vec3::Y * (cp.height * 0.5)),
        ));
    };
    for (i, cp) in definition.checkpoints.iter().enumerate() {
        spawn_gate(cp, Some(i), &gate_mat);
    }
    if let Some(finish) = &definition.finish {
        spawn_gate(finish, None, &finish_mat);
    }
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
    mut session: ResMut<Session>,
    mut ledger: ResMut<ResultLedger>,
    mut started: MessageWriter<RaceStarted>,
    mut participants: Query<(&Player, &Position, &mut RaceProgress)>,
) {
    let Some(mut race) = race else {
        return;
    };
    if race.is_stale(session.generation()) || !session.authority_role().is_authority() {
        return;
    }
    match *session.phase() {
        // The race clock and its triggers only live while the session
        // runs them; Paused/Results/Unloading freeze everything.
        SessionPhase::Countdown | SessionPhase::Playing => {}
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
            for (_, _, mut progress) in &mut participants {
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
            // while Playing.
            if !session.is_playing() {
                return;
            }
            race.clock += 1;
            let mut pending = false;
            let mut local_resolved = false;
            for (player, position, mut progress) in &mut participants {
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
                if progress.advance(&race.definition, position.0) == ProgressOutcome::Finished {
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
                    if player.control == PlayerControl::Local {
                        local_resolved = true;
                    }
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
                for (player, _, mut progress) in &mut participants {
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
                        if player.control == PlayerControl::Local {
                            local_resolved = true;
                        }
                    }
                }
                pending = false;
            }
            if !pending && !participants.is_empty() {
                race.phase = RacePhase::Complete;
            }
            // UI-5's results-screen rule: the local driver's terminal
            // resolution ends the playing session — the race state and
            // ledger freeze with the phase change (the system no longer
            // runs once the session leaves `Playing`). A non-local
            // participant resolving while the local driver still races
            // never ends the local race.
            if local_resolved {
                session
                    .transition(SessionPhase::Results)
                    .expect("Playing → Results is a legal transition");
            }
        }
        RacePhase::Complete => {}
    }
}

/// Needle color while the arrow's target is ahead of the car
/// (RACE-6's green compass arrow).
pub const NAV_AHEAD: Color = Color::srgb(0.2, 1.0, 0.4);
/// Needle color while the target sits in the rear half-plane —
/// RACE-6's "turns yellow when that checkpoint is behind you".
pub const NAV_BEHIND: Color = Color::srgb(1.0, 0.85, 0.2);

/// Marker on the session-owned navigation-arrow needle: the thin bar
/// pivoted at screen top-center whose `UiTransform.rotation` is the
/// signed bearing to the player's current objective. The embedded
/// font is ASCII-only, so the arrow is drawn from UI nodes, not a
/// glyph (designed dev-rig visual, not the original bitmap arrow).
#[derive(Component)]
pub struct NavArrow;

/// Marker on every colored piece of the arrow — the needle and the
/// diamond child at its tip (a child of [`NavArrow`], so it inherits
/// the needle's rotation and stays on the pointing end).
/// [`update_nav_arrow`] writes one shared color to all of them.
#[derive(Component)]
pub struct NavArrowPart;

/// Spawn the RACE-6 navigation arrow: a thin bar at screen top-center
/// with a rotated square as the head diamond. Hidden until a live
/// race hands it a target — [`update_nav_arrow`] owns visibility,
/// rotation and color every frame.
pub fn spawn_nav_arrow(commands: &mut Commands, owner: SessionEntity) {
    commands
        .spawn((
            owner,
            NavArrow,
            NavArrowPart,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(56.0),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-3.0)),
                width: Val::Px(6.0),
                height: Val::Px(34.0),
                ..default()
            },
            BackgroundColor(NAV_AHEAD),
            Visibility::Hidden,
        ))
        .with_child((
            NavArrowPart,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(-12.0),
                left: Val::Px(-7.0),
                width: Val::Px(20.0),
                height: Val::Px(20.0),
                ..default()
            },
            // The child rides in the needle's rotated frame, so its own
            // 45° makes it a diamond sitting on the pointing end.
            UiTransform::from_rotation(Rot2::degrees(45.0)),
            BackgroundColor(NAV_AHEAD),
        ));
}

/// `X`/`Z` cycle the arrow's target through the remaining gates
/// (RACE-6). The original binds this to X/S (CTL-1), but `S` is brake
/// under this app's added WASD mapping, so the backward cycle moved to
/// `Z` — an input-map departure, not a rules one (designed). Cycling
/// is allowed while the race counts down: the arrow is already live.
pub fn nav_target_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    mut players: Query<(&Position, &RaceProgress, &mut TargetSelection)>,
) {
    let dir = if keys.just_pressed(KeyCode::KeyX) {
        1
    } else if keys.just_pressed(KeyCode::KeyZ) {
        -1
    } else {
        return;
    };
    let Some(race) = race else { return };
    if race.is_stale(session.generation()) || race.phase == RacePhase::Complete {
        return;
    }
    for (pos, progress, mut selection) in &mut players {
        selection.picked = cycle_target(&race.definition, progress, selection.picked, pos.0, dir);
    }
}

/// Drive the arrow to the player's current objective every frame: the
/// needle's rotation is the signed bearing to the target (clockwise,
/// so `+` bearing = right), green while the target is in the forward
/// half-plane and yellow behind (RACE-6). Hidden whenever there is no
/// live target — no race, a stale/complete race, an `Ordered`
/// definition, or a participant already resolved.
pub fn update_nav_arrow(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
    players: Query<(&Position, &Rotation, &RaceProgress, &TargetSelection)>,
    mut arrow: Query<(&mut UiTransform, &mut Visibility), With<NavArrow>>,
    mut parts: Query<&mut BackgroundColor, With<NavArrowPart>>,
) {
    let live = race
        .filter(|r| !r.is_stale(session.generation()))
        .filter(|r| r.phase != RacePhase::Complete);
    let target = live.and_then(|r| {
        players.iter().find_map(|(pos, rot, progress, selection)| {
            if !matches!(
                progress.state,
                ParticipantState::AwaitingStart | ParticipantState::Racing
            ) {
                return None;
            }
            let target = navigation_target(&r.definition, progress, selection.picked, pos.0)?;
            let target_pos = target.position(&r.definition)?;
            // Heading from the physics rotation, projected to XZ:
            // vehicle forward is local −Z (same convention
            // `relative_bearing` documents).
            let fwd = rot.0 * Vec3::NEG_Z;
            let yaw = (-fwd.x).atan2(-fwd.z);
            Some(relative_bearing(yaw, pos.0, target_pos))
        })
    });
    for (mut ui, mut vis) in &mut arrow {
        match target {
            Some(bearing) => {
                *vis = Visibility::Visible;
                ui.rotation = Rot2::radians(bearing);
            }
            None => *vis = Visibility::Hidden,
        }
    }
    let color = match target {
        Some(b) if b.abs() > std::f32::consts::FRAC_PI_2 => NAV_BEHIND,
        _ => NAV_AHEAD,
    };
    for mut bg in &mut parts {
        *bg = BackgroundColor(color);
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

/// `GO!` color — the same green the nav arrow uses for "ahead".
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
/// sits inside the window.
pub fn update_countdown_banner(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
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
        *vis = if cue.is_some() {
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
            top: Val::Px(108.0),
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
pub fn update_race_warning(
    race: Option<Res<RaceState>>,
    session: Res<Session>,
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
    for (mut vis, mut color) in &mut warning {
        match remaining {
            Some(t) if t <= LOW_TIME_TICKS => {
                *vis = Visibility::Visible;
                let phase = ((LOW_TIME_TICKS - t) / LOW_TIME_FLASH_TICKS) % 2;
                color.0 = if phase == 0 {
                    LOW_TIME_BRIGHT
                } else {
                    LOW_TIME_DIM
                };
            }
            _ => *vis = Visibility::Hidden,
        }
    }
}
