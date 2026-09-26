//! The documented `H` HUD toggle (F22-A.3; HUD-3/CTL-1 "Toggle HUD
//! (Heads-Up Display)"): a master gate over the session's driving-HUD
//! layer, pressed while driving.
//!
//! Exe evidence (mm2hook `mmHUD`/`mmHudMap` layouts, 2026-09-26): the
//! original's HUD is a single `mmHUD` node owning the `mmHudMap` (which
//! itself carries the opponent icons and `DrawIndicator` for the
//! in-world opponent arrows), the `mmArrow` compass, the stopwatch/
//! countdown `mmTimer` pair, the `mmDashView` cluster and the `mmCRHUD`
//! — and it exposes `Enable`/`Disable`/`Toggle`, so one key suppresses
//! the whole unit. [`HudVisible`] is that gate: while it is off every
//! driving-HUD driver keeps computing but draws nothing — the
//! instrument line, the nav arrow, the race banners, the HUD map's
//! corner views, the opponent indicators and the cockpit dashboard all
//! suppress together, exactly the membership the recovered `mmHUD`
//! layout shows.
//!
//! What stays independent by design (DSN-52): elements with their own
//! documented toggles keep their own state and re-show when `H` turns
//! the layer back on — TAB/E/F/Q still drive the map state, `I` the
//! indicator toggle, BACKSPACE the mirror strip and `V` the cockpit
//! view. The mirror and the nav debug overlay are *views*/diagnostics,
//! not HUD instruments, so `H` never touches them; `ErrorText`, the
//! pause/results/menu overlays and the full-screen *pause* map are
//! session/menu surfaces, also outside the driving HUD.
//!
//! - [`HudVisible`] is session-agnostic like
//!   [`crate::camera::RearView`]: a restart respawns the session's HUD
//!   entities and the drivers re-apply the driver's choice. On by
//!   designed default — the original's start state is unrecovered.
//! - [`hud_input`] owns the `H` toggle in `Playing`/`Countdown` only,
//!   so overlays keep the key — the same contract `mirror_input`
//!   holds for BACKSPACE and `indicator_input` for `I`.
//! - [`update_hud`] writes the instrument line (moved here from the
//!   `mm2` bin target so tests can reach the visibility gate — the
//!   `active_cam_pose`/`retarget_hud` precedent).
//!
//! Still unverified (UNK-31): whether the original's `H` suppresses the
//! rear-view mirror and what `mmHUD::Toggle`'s exact flag scope is
//! (the member list is recovered, the `Cull` gate is inferred).

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{PlayerControl, PlayerVehicle, RacePhase, Session, SessionPhase, ordinal};

use crate::{camera, hudmap, nav_overlay, session};

/// Whether the driving-HUD layer is switched on (HUD-3/CTL-1: `H`
/// toggles). Session-agnostic like [`crate::camera::RearView`] and
/// [`crate::oppind::OpponentIndicators`]: a restart respawns the HUD
/// entities and the drivers re-apply the driver's choice. Defaults on
/// — a designed default: the original's start state is unrecovered,
/// and the HUD is the layer the instruments are meant to be read
/// through.
#[derive(Resource, Debug, Clone, Copy)]
pub struct HudVisible(pub bool);

impl Default for HudVisible {
    fn default() -> Self {
        Self(true)
    }
}

/// `H` toggles the driving HUD (HUD-3/CTL-1) in the live phases.
/// `Paused`/`Results`/menu contexts keep the key for their own owners —
/// the toggle would be invisible there either way — the same contract
/// `mirror_input` holds for BACKSPACE.
pub fn hud_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    mut hud: ResMut<HudVisible>,
) {
    if !matches!(
        session.phase(),
        SessionPhase::Playing | SessionPhase::Countdown
    ) {
        return;
    }
    if keys.just_pressed(KeyCode::KeyH) {
        hud.0 = !hud.0;
    }
}

/// The instrument-line node: text plus the [`HudVisible`] gate's
/// visibility write.
type HudLine<'w, 's> = Query<
    'w,
    's,
    (&'static mut Text, &'static mut Visibility),
    (With<session::Hud>, Without<session::ErrorText>),
>;

/// HUD line: speed, gear/direction, RPM, grounded wheels — read from the
/// `VehicleTelemetry` snapshot, the presentation-side contract, not the
/// mutable simulation state. While [`HudVisible`] is off the line keeps
/// computing but draws nothing — the `mmHUD::Toggle` gate suppresses
/// the layer's draw, not its update. `ErrorText` is a load-failure
/// surface, not a HUD instrument, so the gate never touches it.
// Bevy systems thread one parameter per borrowed resource/query; the
// HUD legitimately reads several.
#[allow(clippy::too_many_arguments)]
pub fn update_hud(
    session: Res<Session>,
    hud_on: Res<HudVisible>,
    race: Option<Res<mm2_game::RaceState>>,
    nav: Option<Res<nav_overlay::CityNav>>,
    ledger: Res<mm2_game::ResultLedger>,
    mut hud: HudLine,
    mut err: Query<&mut Text, (With<session::ErrorText>, Without<session::Hud>)>,
    vehicles: Query<&mm2_game::VehicleTelemetry, With<PlayerVehicle>>,
    progress: Query<(Option<&mm2_game::Player>, &mm2_game::RaceProgress), With<PlayerVehicle>>,
    participants: Query<(&mm2_game::Player, &mm2_game::RaceProgress, &Position)>,
    // The HUD map's own camera never counts as "the" camera — same
    // filter `camera::retarget_hud`/`active_cam_pose` apply (F22-A.1) —
    // and the pose read is `GlobalTransform`: the cockpit camera is a
    // child of the vehicle, so its `Transform` is car-local.
    cameras: Query<(&Camera, &GlobalTransform), hudmap::WorldCamera3d>,
) {
    for mut text in &mut err {
        *text = match session.phase() {
            SessionPhase::Failed(m) => Text::new(format!("world failed to load:\n{m}")),
            _ => Text::new(""),
        };
    }
    let hz = mm2_game::RACE_TICK_HZ as f32;
    // The local participant's terminal state, rendered once the race is
    // over — the finish carries its recorded race-clock time and its
    // place in the ledger's standings (UI-5's "placing + total time",
    // F13-B). The ledger outlives one session, so the place scopes to
    // the current generation — a finished restart's stale results must
    // not re-rank the live race. `TimedOut` participants rank but show
    // no place — a DNF banner is clearer than an ordinal.
    let participant_count = participants.iter().count();
    let outcome = |id: Option<mm2_game::PlayerId>, state: Option<&mm2_game::ParticipantState>| {
        let placing = id
            .and_then(|id| ledger.place_of_in(session.generation(), id))
            .map(|p| match participant_count {
                n if n > 1 => format!(" {} of {n}", ordinal(p)),
                _ => format!(" {}", ordinal(p)),
            })
            .unwrap_or_default();
        match state {
            Some(mm2_game::ParticipantState::Finished { race_ticks, .. }) => {
                format!("  FINISHED{placing}  {:.1}s", *race_ticks as f32 / hz)
            }
            Some(mm2_game::ParticipantState::TimedOut { .. }) => "  OUT OF TIME".to_string(),
            _ => "  FINISHED".to_string(),
        }
    };
    // The local participant's identity for the live place indicator —
    // the PlayerVehicle's `Player`, or any `Local`-controlled
    // participant if the vehicle entity is not a participant.
    let local_id = progress
        .iter()
        .next()
        .and_then(|(p, _)| p.map(|p| p.id))
        .or_else(|| {
            participants
                .iter()
                .find(|(p, _, _)| p.control == PlayerControl::Local)
                .map(|(p, _, _)| p.id)
        });
    let race_text = race
        .filter(|r| !r.is_stale(session.generation()))
        .map(|r| {
            // A resolved local driver ends the session at `Results`
            // (UI-5) even while other participants' progress keeps the
            // race itself `Running` — the outcome text wins either way.
            if *session.phase() == SessionPhase::Results {
                let (id, state) = progress
                    .iter()
                    .next()
                    .map(|(p, progress)| (p.map(|p| p.id), &progress.state))
                    .unzip();
                return outcome(id.flatten(), state);
            }
            // HUD-2's place indicator: the live running order (DSN-13).
            // Only a competitive field gets one — a lone participant
            // has no placing to show.
            let order = mm2_game::live_order(
                &r.definition,
                participants
                    .iter()
                    .map(|(p, prog, pos)| (p.id, prog, pos.0)),
            );
            let place = if order.len() > 1 {
                local_id
                    .and_then(|id| order.iter().position(|p| *p == id))
                    .map(|i| format!("  {} of {}", ordinal(i as u32 + 1), order.len()))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            match r.phase {
                RacePhase::Countdown { remaining } => {
                    format!("  GET READY {:.0}{place}", (remaining as f32 / hz).ceil())
                }
                RacePhase::Running => {
                    let cleared = progress.iter().next().map_or(0, |(_, p)| p.cleared_count());
                    let lap = progress.iter().next().map_or(0, |(_, p)| p.lap + 1);
                    // A timed event counts down the same authoritative race
                    // clock the deadline is judged on (AC04); untimed races
                    // show elapsed.
                    let clock = match r.time_remaining() {
                        Some(t) => format!("  time {:.1}s", t as f32 / hz),
                        None => format!("  {:.1}s", r.clock as f32 / hz),
                    };
                    if r.definition.rule == mm2_game::CheckpointRule::Ordered {
                        format!(
                            "  lap {lap}/{}  cp {cleared}/{}{place}{clock}",
                            r.definition.laps,
                            r.definition.checkpoints.len(),
                        )
                    } else {
                        format!(
                            "  cp {cleared}/{}{place}{clock}",
                            r.definition.checkpoints.len()
                        )
                    }
                }
                RacePhase::Complete => {
                    let (id, state) = progress
                        .iter()
                        .next()
                        .map(|(p, progress)| (p.map(|p| p.id), &progress.state))
                        .unzip();
                    outcome(id.flatten(), state)
                }
            }
        })
        .unwrap_or_default();
    let Ok(veh) = vehicles.single() else {
        for (mut text, mut vis) in &mut hud {
            *vis = if hud_on.0 {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
            *text = Text::new(match session.phase() {
                SessionPhase::Failed(_) => String::new(),
                SessionPhase::Playing => "no vehicle".to_string(),
                _ => format!("loading…{race_text}"),
            });
        }
        return;
    };
    let speed = veh.linear_velocity.length() * 3.6;
    let dir = if veh.reverse {
        "R".to_string()
    } else {
        format!("D{}", veh.gear + 1)
    };
    let grounded = veh.wheels.iter().filter(|w| w.grounded).count();
    let cam = camera::active_cam_pose(&cameras).unwrap_or_default();
    let nav_text = nav.map_or_else(String::new, |n| {
        format!("  {}", nav_overlay::hud_summary(&n))
    });
    for (mut text, mut vis) in &mut hud {
        *vis = if hud_on.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        *text = Text::new(format!(
            "{speed:5.1} km/h  {dir}  {rpm:4.0} rpm  wheels {grounded}/{total}  cam {cam}{race_text}{nav_text}",
            rpm = veh.rpm,
            total = veh.wheels.len(),
        ));
    }
}
