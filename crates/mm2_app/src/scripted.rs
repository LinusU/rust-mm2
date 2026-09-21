//! Scripted course-follower for evidence runs (F12-C).
//!
//! The `--bot` driver steers the player vehicle at the *live* race
//! objective through the same [`VehicleInput`] component the
//! keyboard/gamepad mapping writes — so a headless run exercises the
//! production checkpoint/finish/result path (`advance_race` +
//! `RaceProgress::advance`) instead of idling or running straight off
//! the course. It is an evidence driver, not a gameplay feature: no
//! authored `.opp` routes, no opponent AI (F15 scope), just enough
//! control to finish authored courses the throttle-hold driver cannot.
//!
//! The objective the bot chases is the race's own: under `AnyOrder`
//! the earliest un-cleared gate in authored order (the waypoint rows
//! encode the intended route — chasing the RACE-6 arrow's *nearest*
//! pick would send the pursuit across the map into buildings), then
//! the armed finish trigger (RACE-7); under `Ordered` the
//! participant's `next` required gate. A proportional steer aims the
//! nose at it, with a corner brake band and a reverse-and-turn escape
//! when the car is stopped against something — city courses have
//! walls the straight line to a gate does not see.

use avian3d::prelude::*;
use bevy::prelude::*;
use mm2_game::{
    CheckpointRule, ParticipantState, PlayerVehicle, RaceDefinition, RaceProgress, RaceState,
    Session, relative_bearing,
};
use mm2_vehicle::{VehicleInput, VehicleState};

/// Presence enables the scripted driver: `--bot` inserts it, and
/// [`scripted_drive`] owns the player vehicle's [`VehicleInput`] while
/// it exists. A resource (not a CLI argument threaded everywhere) so
/// both the windowed app and the headless smoke gate the same system.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ScriptedDrive;

/// Per-vehicle controller state — a component on the car so session
/// teardown despawns it with everything else the session owns: a
/// restart cannot inherit a half-finished recovery or a stale stuck
/// timer.
#[derive(Component, Debug, Clone, Copy)]
pub struct ScriptedBot {
    /// Consecutive frames demanding drive while grounded and barely
    /// moving — the stuck detector.
    pub stuck_frames: u32,
    /// Frames left in the reverse half of an escape.
    pub reverse_frames: u32,
    /// Frames left in the forward full-lock half of an escape — the
    /// car backs off the wall, then turns away from it.
    pub turn_frames: u32,
    /// Which way the current escape turns (±1). Alternates each
    /// recovery so a car that keeps failing tries the other side —
    /// the nav line can't see walls, so escapes are blind guesses.
    pub recovery_side: f32,
}

impl Default for ScriptedBot {
    fn default() -> Self {
        Self {
            stuck_frames: 0,
            reverse_frames: 0,
            turn_frames: 0,
            recovery_side: 1.0,
        }
    }
}

/// Radians of bearing → normalized steering. Positive bearing is to the
/// driver's right ([`relative_bearing`]) and positive steering is right
/// ([`VehicleInput::steering`]), so the signs line up directly.
const STEER_GAIN: f32 = 1.5;

/// Full throttle while the target is inside this |bearing| cone.
const STRAIGHT_RAD: f32 = 0.35;
/// Reduced throttle out to here; beyond it the crawl band applies.
const TURN_RAD: f32 = 1.0;
/// Corner brake: above this bearing *and* [`CORNER_SPEED`] the car
/// brakes instead of throttling, so it can rotate into the turn.
const CORNER_RAD: f32 = 1.1;
/// Speed (m/s) above which the corner brake engages.
const CORNER_SPEED: f32 = 16.0;
/// Soft speed cap (m/s): coast above it so the car does not out-run its
/// own steering on long straights. `vpbug` peaks ~37 m/s flat out.
const SPEED_CAP: f32 = 30.0;
/// |forward speed| below which the car counts as not moving (m/s).
const STUCK_SPEED: f32 = 1.5;
/// Frames (60 Hz updates) of grounded no-motion with throttle demanded
/// before a recovery starts — about 1.5 s.
const STUCK_FRAMES: u32 = 90;
/// Recovery reverse length in frames — about 1.2 s of backing off.
const REVERSE_FRAMES: u32 = 72;
/// Recovery forward-turn length in frames — about 0.7 s of full lock
/// after the reverse, turning the nose away from the wall.
const TURN_FRAMES: u32 = 45;

/// Per-driver tuning of the control law (F15-B.2). The player
/// evidence bot always uses [`ScriptedTuning::DEFAULT`]; opponents
/// resolve theirs from the authored `[Opponent]` parameter tail, so
/// authored skill shows up as throttle demand and carried corner
/// speed rather than a different car or a different law.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptedTuning {
    /// Ceiling applied to every throttle demand — the authored
    /// `maxThrottle` column (default 1.0 = unchanged).
    pub throttle_cap: f32,
    /// Speed (m/s) above which a sharp bearing brakes instead of
    /// throttling — [`CORNER_SPEED`] scaled by the authored
    /// corner-speed multiplier (default = `CORNER_SPEED`).
    pub corner_speed: f32,
}

impl ScriptedTuning {
    /// The shared constants — what every driver used before authored
    /// tuning existed.
    pub const DEFAULT: Self = Self {
        throttle_cap: 1.0,
        corner_speed: CORNER_SPEED,
    };
}

impl Default for ScriptedTuning {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn steer_cmd(bearing: f32) -> f32 {
    (bearing * STEER_GAIN).clamp(-1.0, 1.0)
}

/// One frame of the control law under the driver's [`ScriptedTuning`]:
/// `corner_speed` sets the corner-brake engage speed and
/// `throttle_cap` ceilings every throttle demand, recovery included —
/// an authored `maxThrottle` of 0.8 limits the car everywhere, not
/// just on straights.
///
/// The stuck recovery is a blind three-point escape: reverse while
/// swinging the nose toward `recovery_side` (reversing pivots the nose
/// opposite the steer, hence `-recovery_side`), then drive forward at
/// full lock the same way. When the bearing already points somewhere
/// useful the reverse steers `-steer_cmd` instead — straight at the
/// target. `recovery_side` alternates between escapes.
pub fn scripted_input_tuned(
    bot: &mut ScriptedBot,
    bearing: f32,
    forward_speed: f32,
    grounded: bool,
    tuning: &ScriptedTuning,
) -> VehicleInput {
    if bot.reverse_frames > 0 {
        bot.reverse_frames -= 1;
        let steer = -steer_cmd(bearing);
        return VehicleInput {
            brake: 1.0,
            steering: if steer.abs() >= 0.5 {
                steer
            } else {
                -bot.recovery_side
            },
            ..default()
        };
    }
    if bot.turn_frames > 0 {
        bot.turn_frames -= 1;
        return VehicleInput {
            throttle: 0.5_f32.min(tuning.throttle_cap),
            steering: bot.recovery_side,
            ..default()
        };
    }
    let mut input = VehicleInput {
        steering: steer_cmd(bearing),
        ..default()
    };
    let b = bearing.abs();
    if b > CORNER_RAD && forward_speed > tuning.corner_speed {
        input.brake = 0.6;
    } else if forward_speed < SPEED_CAP {
        let band: f32 = if b <= STRAIGHT_RAD {
            1.0
        } else if b <= TURN_RAD {
            0.45
        } else {
            0.25
        };
        input.throttle = band.min(tuning.throttle_cap);
    }
    if grounded && input.throttle > 0.0 && forward_speed.abs() < STUCK_SPEED {
        bot.stuck_frames += 1;
        if bot.stuck_frames >= STUCK_FRAMES {
            bot.stuck_frames = 0;
            bot.reverse_frames = REVERSE_FRAMES;
            bot.turn_frames = TURN_FRAMES;
            bot.recovery_side = -bot.recovery_side;
        }
    } else {
        bot.stuck_frames = 0;
    }
    input
}

/// One frame of the control law at [`ScriptedTuning::DEFAULT`].
/// `bearing` is the signed angle to the target (positive = right),
/// `forward_speed` the signed speed along the nose (m/s), `grounded`
/// whether any wheel has contact.
pub fn scripted_input(
    bot: &mut ScriptedBot,
    bearing: f32,
    forward_speed: f32,
    grounded: bool,
) -> VehicleInput {
    scripted_input_tuned(
        bot,
        bearing,
        forward_speed,
        grounded,
        &ScriptedTuning::DEFAULT,
    )
}

/// The world position the bot drives at — the live objective, never a
/// memorised route. `AnyOrder` follows the *earliest* un-cleared gate
/// in authored order: the waypoint rows encode the intended course
/// sequence, so chasing the next row keeps the pursuit on the roads
/// between consecutive gates — the RACE-6 arrow's "nearest" pick is
/// the player's freedom, but a scripted route that ping-pongs across
/// the map just drives into buildings. Gates clipped out of order
/// still clear through the same swept path. Once every gate is
/// cleared the armed finish trigger (RACE-7) is the target. `Ordered`
/// aims at the next required gate in authored order. `None` when the
/// definition has no current objective — e.g. an `Ordered`
/// participant whose `next` ran past the gate list — which the caller
/// treats as "stop".
pub fn drive_target(definition: &RaceDefinition, progress: &RaceProgress) -> Option<Vec3> {
    match definition.rule {
        CheckpointRule::AnyOrder => progress.remaining().next().map_or_else(
            || definition.finish.as_ref().map(|c| c.center),
            |i| Some(definition.checkpoints[i].center),
        ),
        CheckpointRule::Ordered => definition.checkpoints.get(progress.next).map(|c| c.center),
    }
}

/// Write [`VehicleInput`] on the player vehicle from the live race
/// objective every frame. Scheduled `after` [`crate::input::vehicle_input`]
/// and gated on [`ScriptedDrive`], so while `--bot` is on the bot
/// deterministically owns the input — and while it is off this system
/// never runs.
///
/// Honors the same gates the keyboard path does: the session must be
/// `Playing` and the race countdown's `input_locked` holds the car
/// still (anything else writes a zeroed input so the car rolls to a
/// stop rather than holding stale controls). Once the participant
/// resolves — finished or timed out — the bot yields and the car
/// coasts. Cruise sessions have no race objective; the bot drives a
/// point far ahead of the nose, which is the throttle-hold driver's
/// straight line with the same recovery.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub fn scripted_drive(
    mut commands: Commands,
    session: Res<Session>,
    race: Option<Res<RaceState>>,
    mut cars: Query<
        (
            Entity,
            &mut VehicleInput,
            &Position,
            &Rotation,
            &VehicleState,
            Option<&RaceProgress>,
            Option<&mut ScriptedBot>,
        ),
        With<PlayerVehicle>,
    >,
) {
    let race = race
        .as_deref()
        .filter(|r| !r.is_stale(session.generation()));
    let locked = race.is_some_and(|r| r.input_locked());
    for (entity, mut input, pos, rot, vstate, progress, bot) in &mut cars {
        let racing = progress.is_some_and(|p| {
            matches!(
                p.state,
                ParticipantState::AwaitingStart | ParticipantState::Racing
            )
        });
        let target = if !session.is_playing() || locked {
            None
        } else if let Some(race) = race {
            if !racing {
                None
            } else {
                progress.and_then(|p| drive_target(&race.definition, p))
            }
        } else {
            // No race: aim far ahead of the nose — the straight-line
            // drive with the same recovery the event path gets.
            Some(pos.0 + rot.0 * Vec3::NEG_Z * 200.0)
        };
        let Some(target) = target else {
            *input = VehicleInput::default();
            continue;
        };
        let fwd = rot.0 * Vec3::NEG_Z;
        let yaw = (-fwd.x).atan2(-fwd.z);
        let bearing = relative_bearing(yaw, pos.0, target);
        let mut fresh = ScriptedBot::default();
        let bot = match bot {
            Some(b) => b.into_inner(),
            None => {
                commands.entity(entity).insert(ScriptedBot::default());
                &mut fresh
            }
        };
        *input = scripted_input(bot, bearing, vstate.forward_speed, vstate.grounded);
    }
}
