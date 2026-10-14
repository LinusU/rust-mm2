//! F22-C validation (F22-AC01): the HUD instruments across the
//! pause → resume → reset → finish journey, always as a faithful read
//! of the authoritative state — the production publisher's
//! `VehicleTelemetry` snapshot plus the live `RaceState`/`RaceProgress`
//! and the result ledger. Nothing recomputes: every leg asserts a HUD
//! value against the snapshot and the race state the production
//! systems hold at that moment, so a HUD that drifts from the
//! simulation (or survives a reset with stale numbers) fails here.
//!
//! The legs run the real chain: `advance_race` drives the countdown
//! release, the swept crossings, the lap wrap, the clock and the
//! `Playing → Results` transition; `vehicle_reset` performs the
//! `R`-reset teleport; and `contracts::publish_vehicle_telemetry` —
//! the binary's own FixedLast publisher — copies `VehicleState` into
//! the snapshot the HUD reads. [`the_instrument_line_follows_the_authoritative_state_across_pause_reset_and_finish`]
//! covers the instrument line's speed/gear/RPM/checkpoint/clock read;
//! [`the_timer_place_and_lap_agree_with_the_race_state_across_pause_reset_and_finish`]
//! adds the authored timer row and the race-stat cluster (place, laps)
//! over an `Ordered` field of two. The per-instrument units (timer
//! glyph composition, speedometer dial, nav-arrow release) keep their
//! own modules; this file is the end-to-end agreement of the HUD with
//! the state machine.

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::contracts;
use mm2_app::hud;
use mm2_app::race::{advance_race, reanchor_teleported_participants};
use mm2_app::racestat;
use mm2_app::racetime;
use mm2_app::session::{self, SpawnPoint};
use mm2_assets::Vfs;
use mm2_game::{
    Checkpoint, CheckpointRule, EventParams, EventRef, EventTableKind, ObjectId, ObjectIdentity,
    ParticipantState, Player, PlayerControl, PlayerId, RaceDefinition, RacePhase, RaceProgress,
    RaceStart, RaceState, ResultLedger, Session, SessionConfig, SessionEntity, SessionMode,
    SessionPhase, TargetSelection, advance_session_tick, live_order,
};
use mm2_vehicle::{ResetVehicle, TireConditions, Vehicle, VehicleConfig, VehicleState};

/// One fixed step at the binary's 120 Hz, presented as one 60 Hz frame.
const FRAME: f64 = 1.0 / 60.0;

fn cp(x: f32, z: f32) -> Checkpoint {
    Checkpoint {
        center: Vec3::new(x, 0.0, z),
        radius: 15.0,
        height: mm2_game::DEFAULT_CHECKPOINT_HEIGHT,
        heading_deg: 0.0,
        require_direction: false,
    }
}

/// A two-gate AnyOrder course: enough to show `cp` counting, the
/// clock and the finish without a lap wrap.
fn course_def() -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(100.0, 0.0)],
        finish: None,
        rule: CheckpointRule::AnyOrder,
        laps: 1,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 120,
        start_slots: vec![RaceStart {
            position: Vec3::new(-50.0, 0.0, 0.0),
            yaw_deg: Some(0.0),
        }],
    }
}

fn event_config() -> SessionConfig {
    SessionConfig {
        mode: SessionMode::Event(EventRef {
            city: "london".into(),
            table: EventTableKind::Checkpoint,
            index: 0,
        }),
        ..SessionConfig::default()
    }
}

/// The production event path minus windowing and world content: the
/// same race driver, telemetry publisher and reset apply the binary's
/// schedules, with the HUD line spawned the way `load_session_world`
/// spawns it.
fn journey_app(def: &RaceDefinition) -> (App, Entity, PlayerId) {
    let mut session = Session::new();
    session.begin(event_config()).unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Countdown).unwrap();
    let generation = session.generation();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            FRAME,
        )))
        .insert_resource(session)
        .insert_resource(RaceState::new(def.clone(), generation))
        .insert_resource(SpawnPoint::new(Vec3::new(-50.0, 0.0, -20.0), 0.0))
        .init_resource::<ResultLedger>()
        .init_resource::<hud::HudVisible>()
        .init_resource::<Assets<Image>>()
        .init_resource::<TireConditions>()
        .add_message::<ResetVehicle>()
        .add_message::<mm2_game::RaceStarted>()
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            // The binary's own FixedLast order: the publisher snapshots
            // before the teleport re-anchor and the race driver (and
            // `reanchor` still precedes `advance_race`, so a reset
            // never sweeps a checkpoint).
            (
                contracts::publish_vehicle_telemetry,
                reanchor_teleported_participants,
                advance_race,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                (mm2_vehicle::systems::vehicle_reset, hud::update_hud).chain(),
                // The authored timer row and race-stat cluster read the
                // same `RaceState`/`RaceProgress` the line does; their
                // reports drive the assertions below. Absent resources
                // leave them inert for the line-only leg.
                racetime::update_race_timer,
                racestat::update_race_stats,
            ),
        );
    app.finish();
    app.cleanup();

    app.world_mut()
        .spawn((SessionEntity(generation), session::Hud, Text::new("")));

    let id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let cfg = VehicleConfig::default();
    let start = Vec3::new(-50.0, 0.0, -10.0);
    let car = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id,
                control: PlayerControl::Local,
            },
            mm2_game::PlayerVehicle,
            ObjectIdentity(ObjectId {
                generation: 1,
                slot: 0,
            }),
            RaceProgress::new(def),
            TargetSelection::default(),
            Vehicle {
                config: cfg.clone(),
            },
            VehicleState::new(&cfg),
            Position(start),
            Rotation::default(),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
            Transform::from_translation(start),
        ))
        .id();
    (app, car, id)
}

fn line(app: &mut App) -> String {
    app.world_mut()
        .query_filtered::<&Text, With<session::Hud>>()
        .single(app.world())
        .expect("the instrument line")
        .0
        .clone()
}

fn hud_visibility(app: &mut App) -> Visibility {
    let entity = app
        .world_mut()
        .query_filtered::<Entity, With<session::Hud>>()
        .single(app.world())
        .expect("the instrument line");
    *app.world().get::<Visibility>(entity).unwrap()
}

fn run(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

fn set_position(app: &mut App, entity: Entity, pos: Vec3) {
    *app.world_mut().get_mut::<Position>(entity).unwrap() = Position(pos);
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = pos;
}

fn race_clock(app: &App) -> u64 {
    app.world().resource::<RaceState>().clock
}

/// The clock string `update_hud` composes for an untimed race — the
/// same authoritative `RaceState::clock` the deadline and timer use.
fn clock_text(app: &App) -> String {
    format!(
        "  {:.1}s",
        race_clock(app) as f32 / mm2_game::RACE_TICK_HZ as f32
    )
}

/// Cross one gate through its swept trigger the way the physics-driven
/// segment would: in, then out the far side.
fn cross_gate(app: &mut App, car: Entity, x: f32) {
    set_position(app, car, Vec3::new(x, 0.0, -10.0));
    run(app, 1);
    set_position(app, car, Vec3::new(x, 0.0, 10.0));
    run(app, 1);
}

#[test]
fn the_instrument_line_follows_the_authoritative_state_across_pause_reset_and_finish() {
    let def = course_def();
    let (mut app, car, id) = journey_app(&def);

    // --- Countdown: the banner comes from the race phase. ----------
    assert!(matches!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Countdown { .. }
    ));
    run(&mut app, 2);
    let text = line(&mut app);
    assert!(
        text.contains("GET READY"),
        "countdown shows the race phase: {text}"
    );

    // The release is the production driver's: session `Playing`,
    // participant `Racing`, one `RaceStarted` — bounded, never a sleep.
    assert!(
        (0..200).any(|_| {
            app.update();
            app.world().resource::<Session>().phase() == &SessionPhase::Playing
        }),
        "the countdown released through advance_race"
    );
    assert!(matches!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Running
    ));

    // --- Running: the line reads the publisher's snapshot. ---------
    *app.world_mut().get_mut::<LinearVelocity>(car).unwrap() =
        LinearVelocity(Vec3::new(10.0, 0.0, 0.0));
    {
        let mut state = app.world_mut().get_mut::<VehicleState>(car).unwrap();
        state.rpm = 3600.0;
        state.gear = 3;
    }
    run(&mut app, 2);
    let text = line(&mut app);
    let (speed_kmh, rpm) = {
        let veh = app
            .world()
            .get::<mm2_game::VehicleTelemetry>(car)
            .expect("the publisher wrote the snapshot");
        (veh.linear_velocity.length() * 3.6, veh.rpm)
    };
    assert!(
        text.contains(&format!("{speed_kmh:5.1} km/h")),
        "speed is the snapshot's, not a recompute: {text}"
    );
    assert!(text.contains("D4"), "gear is snapshot gear+1: {text}");
    assert!(
        text.contains(&format!("{:4.0} rpm", rpm)),
        "rpm is the snapshot's: {text}"
    );
    assert!(
        text.contains(&format!("cp 0/{}", def.checkpoints.len())),
        "cleared count starts at zero: {text}"
    );
    assert!(
        text.contains(&clock_text(&app)),
        "the clock is the race's own: {text}"
    );

    // Clear gate 0 through the swept trigger: the count follows.
    cross_gate(&mut app, car, 0.0);
    assert_eq!(
        app.world()
            .get::<RaceProgress>(car)
            .unwrap()
            .cleared_count(),
        1,
        "the sweep cleared gate 0"
    );
    assert!(
        line(&mut app).contains("cp 1/2"),
        "the line follows the progress: {}",
        line(&mut app)
    );

    // --- Pause: the world freezes, the line freezes with it. ------
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    let frozen = line(&mut app);
    let clock_at_pause = race_clock(&app);
    run(&mut app, 10);
    assert_eq!(race_clock(&app), clock_at_pause, "the paused clock froze");
    assert_eq!(
        line(&mut app),
        frozen,
        "a frozen race keeps a byte-identical line"
    );
    assert_eq!(
        hud_visibility(&mut app),
        Visibility::Visible,
        "pause does not hide the driving HUD (the pause menu is its own surface)"
    );

    // --- Resume: the clock runs on from where it paused. -----------
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    run(&mut app, 5);
    assert!(race_clock(&app) > clock_at_pause, "the clock resumed");
    assert!(
        line(&mut app).contains(&clock_text(&app)),
        "the resumed line tracks the live clock: {}",
        line(&mut app)
    );

    // --- Reset: the teleport zeroes the state the snapshot reads. --
    let spawn = app.world().resource::<SpawnPoint>().position;
    app.world_mut()
        .resource_mut::<Messages<ResetVehicle>>()
        .write(ResetVehicle {
            entity: Some(car),
            position: spawn,
            yaw: 0.0,
        });
    run(&mut app, 2);
    // `vehicle_reset` marks the jump `Teleported`; the FixedLast
    // re-anchor consumes the marker in the same pass (the swept
    // segment breaks at the spawn, as in `race.rs`'s reset leg), so
    // the observable outcome is the pose itself.
    assert_eq!(
        app.world().get::<Transform>(car).unwrap().translation,
        spawn,
        "the production reset path teleported the car back to the spawn point"
    );
    assert_eq!(
        app.world()
            .get::<mm2_game::VehicleTelemetry>(car)
            .expect("post-reset snapshot")
            .linear_velocity,
        Vec3::ZERO,
        "reset rests the car"
    );
    let text = line(&mut app);
    assert!(
        text.contains("  0.0 km/h"),
        "the line shows the post-reset snapshot, not the pre-reset speed: {text}"
    );
    assert!(
        text.contains("cp 1/2"),
        "a reset does not clear the race progress the line reports: {text}"
    );

    // --- Finish: the outcome comes from the ledger, once. ----------
    cross_gate(&mut app, car, 100.0);
    run(&mut app, 1);
    assert_eq!(
        app.world().resource::<Session>().phase(),
        &SessionPhase::Results,
        "a local finish moves the session to Results (UI-5)"
    );
    assert_eq!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Complete
    );
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    let ParticipantState::Finished { race_ticks, .. } = progress.state else {
        panic!("the local driver finished: {:?}", progress.state);
    };
    let text = line(&mut app);
    assert!(
        text.contains("FINISHED"),
        "the outcome line replaces the live readout: {text}"
    );
    // The displayed seconds are the recorded race-clock time, not a
    // wall clock: `race_ticks / RACE_TICK_HZ`, to the shown precision.
    let want = format!(
        "  {:.1}s",
        race_ticks as f32 / mm2_game::RACE_TICK_HZ as f32
    );
    assert!(
        text.contains(&want),
        "finish time is the recorded race time: {text} vs {want}"
    );

    // The place is the ledger's, and the ledger has one row.
    let ledger = app.world().resource::<ResultLedger>();
    assert_eq!(ledger.len(), 1, "exactly one recorded result");
    assert_eq!(ledger.place_of(id), Some(1));
    assert!(
        text.contains("1st"),
        "the outcome carries the ledger's place: {text}"
    );

    // --- Results stays put: further frames never re-record. --------
    let results_text = line(&mut app);
    run(&mut app, 6);
    assert_eq!(line(&mut app), results_text, "the outcome line is stable");
    assert_eq!(app.world().resource::<ResultLedger>().len(), 1);

    // Live-order agreement (HUD-2's place-indicator source): the order
    // the HUD would read over the same participants is this session's
    // single driver.
    let definition = app.world().resource::<RaceState>().definition.clone();
    let order: Vec<PlayerId> = live_order(
        &definition,
        app.world_mut()
            .query::<(&Player, &RaceProgress, &Position)>()
            .iter(app.world())
            .map(|(p, prog, pos)| (p.id, prog, pos.0)),
    );
    assert_eq!(order, vec![id]);
}

// ---------------------------------------------------------------------------
// The authored timer and race-stat cluster over the same journey
// ---------------------------------------------------------------------------

/// An `Ordered` circuit of two gates and three laps — the rule the
/// authored laps record is scoped to, with enough laps to show a wrap
/// before the finish. Untimed, so the timer counts up on the race
/// clock (HUD-2's stopwatch side).
fn ordered_course() -> RaceDefinition {
    RaceDefinition {
        checkpoints: vec![cp(0.0, 0.0), cp(100.0, 0.0)],
        finish: None,
        rule: CheckpointRule::Ordered,
        laps: 3,
        time_limit_ticks: None,
        params: EventParams::default(),
        countdown_ticks: 120,
        start_slots: vec![RaceStart {
            position: Vec3::new(-50.0, 0.0, 0.0),
            yaw_deg: Some(0.0),
        }],
    }
}

fn write(dir: &Path, rel: &str, contents: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

/// A minimal uncompressed 32bpp TGA — top-left origin, opaque fill.
/// `decode_buffer_image` routes it through Bevy's TGA loader like the
/// authored `digitac_*` files.
fn tga32(w: u16, h: u16) -> Vec<u8> {
    let mut t = vec![0u8; 18];
    t[2] = 2; // uncompressed true-colour
    t[12..14].copy_from_slice(&w.to_le_bytes());
    t[14..16].copy_from_slice(&h.to_le_bytes());
    t[16] = 32;
    t[17] = 0x28; // 8 alpha bits + top-left origin
    for _ in 0..(w as usize * h as usize) {
        t.extend_from_slice(&[0, 255, 0, 255]); // BGRA green
    }
    t
}

/// Every stem the timer row and the stat cluster resolve — the full
/// `digitac_*` set plus the two colons (the timer's only extra art).
fn write_glyph_set(dir: &Path) {
    for d in 0..10 {
        write(dir, &format!("texture/digitac_{d}.tga"), tga32(41, 56));
        write(dir, &format!("texture/digitac_{d}_half.tga"), tga32(20, 27));
    }
    write(dir, "texture/digi_colon.tga", tga32(13, 56));
    write(dir, "texture/digi_colon_half.tga", tga32(7, 27));
}

/// A mount carrying only the authored glyph set the instruments bind.
fn glyph_mount() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    write_glyph_set(tmp.path());
    tmp
}

fn vfs_of(dir: &Path) -> Vfs {
    let mut vfs = Vfs::new();
    vfs.mount_dir(dir, 0).unwrap();
    vfs
}

/// Spawn the authored timer row and race-stat cluster through the same
/// VFS path the event loader uses, inserting their reports. The drive
/// systems are already registered by [`journey_app`]; without these
/// resources they stay inert, so the line-only leg is unaffected.
fn spawn_glyph_instruments(app: &mut App, vfs: &Vfs, def: &RaceDefinition) {
    let generation = app.world().resource::<Session>().generation();
    let owner = SessionEntity(generation);
    let w = app.world_mut();
    w.resource_scope(|w, mut images: Mut<Assets<Image>>| {
        let mut queue = CommandQueue::default();
        let (timer, stats) = {
            let mut commands = Commands::new(&mut queue, w);
            let timer = racetime::spawn_race_timer(&mut commands, vfs, &mut images, owner);
            let stats = racestat::spawn_race_stats(&mut commands, vfs, &mut images, owner, def);
            (timer, stats)
        };
        queue.apply(w);
        w.insert_resource(timer);
        w.insert_resource(stats);
    });
}

/// The authored `m:ss:hh` text a race clock composes — the same
/// centisecond layout the timer glyphs render (DSN-53).
fn timer_text(clock: u64) -> String {
    let cs = (clock * 100 / u64::from(mm2_game::RACE_TICK_HZ)).min(999 * 6000 + 5999);
    format!("{}:{:02}:{:02}", cs / 6000, (cs % 6000) / 100, cs % 100)
}

fn timer_of(app: &App) -> &racetime::RaceTimerReport {
    app.world().resource::<racetime::RaceTimerReport>()
}

fn stats_of(app: &App) -> &racestat::RaceStatReport {
    app.world().resource::<racestat::RaceStatReport>()
}

fn timer_visible(app: &mut App) -> bool {
    let mut q = app
        .world_mut()
        .query_filtered::<&Visibility, With<racetime::RaceTimer>>();
    q.single(app.world())
        .is_ok_and(|v| *v == Visibility::Visible)
}

fn stats_visible(app: &mut App) -> bool {
    let mut q = app
        .world_mut()
        .query_filtered::<&Visibility, With<racestat::RaceStats>>();
    q.single(app.world())
        .is_ok_and(|v| *v == Visibility::Visible)
}

/// The place the HUD would show, straight from the production running
/// order over the same participants.
fn production_place(app: &mut App, def: &RaceDefinition, id: PlayerId) -> Option<(u32, u32)> {
    let order = live_order(
        def,
        app.world_mut()
            .query::<(&Player, &RaceProgress, &Position)>()
            .iter(app.world())
            .map(|(p, prog, pos)| (p.id, prog, pos.0)),
    );
    order
        .iter()
        .position(|p| *p == id)
        .map(|i| (i as u32 + 1, order.len() as u32))
}

fn lap_of(app: &App, def: &RaceDefinition, car: Entity) -> Option<(u32, u32)> {
    app.world()
        .get::<RaceProgress>(car)
        .map(|p| (p.lap.saturating_add(1).min(def.laps), def.laps))
}

/// The timer row and race-stat cluster read the same authoritative
/// `RaceState`/`RaceProgress` the line does: over a two-participant
/// `Ordered` field they compose the led `m:ss:hh`, the live place and
/// the lap counter, and every value tracks the race through pause,
/// resume, reset, a lap wrap and the local finish.
#[test]
fn the_timer_place_and_lap_agree_with_the_race_state_across_pause_reset_and_finish() {
    let def = ordered_course();
    let (mut app, car, id) = journey_app(&def);
    let generation = app.world().resource::<Session>().generation();

    // Bind the authored instruments on a synthetic mount.
    let tmp = glyph_mount();
    let vfs = vfs_of(tmp.path());
    spawn_glyph_instruments(&mut app, &vfs, &def);
    assert_eq!(timer_of(&app).glyphs, racetime::TIMER_GLYPH_COUNT);
    assert_eq!(stats_of(&app).glyphs, racestat::STAT_GLYPH_COUNT);

    // A second participant: the running order only has a place to show
    // for a competitive field (DSN-13). An AI that never drives is a
    // valid opponent — the rule authority still releases it `Racing`.
    let ai_id = app.world_mut().resource_mut::<Session>().mint_player_id();
    let ai_start = Vec3::new(0.0, 0.0, -200.0);
    let ai = app
        .world_mut()
        .spawn((
            SessionEntity(generation),
            Player {
                id: ai_id,
                control: PlayerControl::Ai,
            },
            RaceProgress::new(&def),
            Position(ai_start),
            Rotation::default(),
            Transform::from_translation(ai_start),
        ))
        .id();

    // Release the countdown through the production driver.
    assert!(
        (0..200).any(|_| {
            app.update();
            app.world().resource::<Session>().phase() == &SessionPhase::Playing
        }),
        "the countdown released through advance_race"
    );
    assert!(matches!(
        app.world().resource::<RaceState>().phase,
        RacePhase::Running
    ));

    // --- Running baseline: both racers on lap 1, the local closest. --
    app.update();
    assert_eq!(lap_of(&app, &def, car), Some((1, 3)));
    assert_eq!(stats_of(&app).lap, Some((1, 3)));
    assert_eq!(stats_of(&app).checkpoints, Some((0, 2)));
    assert!(stats_visible(&mut app));
    assert!(timer_visible(&mut app));
    let want_place = production_place(&mut app, &def, id);
    assert_eq!(stats_of(&app).place, want_place);
    assert_eq!(want_place, Some((1, 2)), "the idle AI ranks behind");
    let want = timer_text(race_clock(&app));
    assert_eq!(timer_of(&app).display.as_deref(), Some(want.as_str()));
    let text = line(&mut app);
    assert!(text.contains("lap 1/3"), "the lap row reads live: {text}");
    assert!(
        text.contains("cp 0/2"),
        "the cleared list reads live: {text}"
    );
    assert!(text.contains("1st of 2"), "the place reads live: {text}");
    assert!(
        text.contains(&clock_text(&app)),
        "the clock reads live: {text}"
    );

    // --- An opponent ahead moves the local place, nothing else. ------
    set_position(&mut app, ai, Vec3::new(0.0, 0.0, -10.0));
    run(&mut app, 1);
    set_position(&mut app, ai, Vec3::new(0.0, 0.0, 10.0));
    run(&mut app, 1);
    assert_eq!(
        app.world().get::<RaceProgress>(ai).unwrap().next,
        1,
        "the AI cleared gate 0 through the swept trigger"
    );
    assert_eq!(stats_of(&app).place, Some((2, 2)));
    assert!(line(&mut app).contains("2nd of 2"), "{}", line(&mut app));

    // --- Pause: the frozen race keeps every value byte-identical. ----
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Paused)
        .unwrap();
    app.update();
    let frozen_clock = race_clock(&app);
    let frozen_line = line(&mut app);
    let frozen_timer = timer_of(&app).display.clone();
    run(&mut app, 10);
    assert_eq!(race_clock(&app), frozen_clock, "the paused clock froze");
    assert_eq!(line(&mut app), frozen_line, "the line froze with it");
    assert_eq!(
        timer_of(&app).display,
        frozen_timer,
        "the timer froze with the race, not a wall clock"
    );
    assert_eq!(
        hud_visibility(&mut app),
        Visibility::Visible,
        "pause does not hide the driving HUD"
    );

    // --- Resume: every instrument tracks the live race clock. --------
    app.world_mut()
        .resource_mut::<Session>()
        .transition(SessionPhase::Playing)
        .unwrap();
    run(&mut app, 5);
    assert!(race_clock(&app) > frozen_clock, "the clock resumed");
    let want = timer_text(race_clock(&app));
    assert_eq!(timer_of(&app).display.as_deref(), Some(want.as_str()));
    assert!(
        line(&mut app).contains(&clock_text(&app)),
        "the line clock resumed with the race: {}",
        line(&mut app)
    );

    // --- Reset: the teleport zeroes the snapshot, never the race. ----
    *app.world_mut().get_mut::<LinearVelocity>(car).unwrap() =
        LinearVelocity(Vec3::new(10.0, 0.0, 0.0));
    run(&mut app, 2);
    assert!(
        line(&mut app).contains("km/h") && !line(&mut app).contains("  0.0 km/h"),
        "the car is moving before the reset: {}",
        line(&mut app)
    );
    let spawn = app.world().resource::<SpawnPoint>().position;
    app.world_mut()
        .resource_mut::<Messages<ResetVehicle>>()
        .write(ResetVehicle {
            entity: Some(car),
            position: spawn,
            yaw: 0.0,
        });
    run(&mut app, 2);
    let text = line(&mut app);
    assert!(
        text.contains("  0.0 km/h"),
        "the line shows the post-reset snapshot: {text}"
    );
    assert!(
        text.contains("cp 0/2") && text.contains("lap 1/3"),
        "a reset does not clear the race the instruments report: {text}"
    );
    assert_eq!(
        stats_of(&app).place,
        Some((2, 2)),
        "the reset did not re-rank"
    );
    let want = timer_text(race_clock(&app));
    assert_eq!(
        timer_of(&app).display.as_deref(),
        Some(want.as_str()),
        "the reset did not touch the race clock"
    );

    // --- Lap wrap: the local clears both gates for lap 2. ------------
    cross_gate(&mut app, car, 0.0);
    assert_eq!(stats_of(&app).checkpoints, Some((1, 2)));
    assert_eq!(stats_of(&app).lap, Some((1, 3)), "still mid-lap");
    assert!(line(&mut app).contains("lap 1/3"));
    cross_gate(&mut app, car, 100.0);
    assert_eq!(
        stats_of(&app).lap,
        Some((2, 3)),
        "the lap wrap is the completed laps + 1"
    );
    assert_eq!(
        stats_of(&app).checkpoints,
        Some((0, 2)),
        "an Ordered lap wraps the cleared prefix"
    );
    assert!(line(&mut app).contains("lap 2/3"), "{}", line(&mut app));
    let want_place = production_place(&mut app, &def, id);
    assert_eq!(stats_of(&app).place, want_place);

    // --- Finish: two more laps resolve the local driver. -------------
    for _ in 0..2 {
        cross_gate(&mut app, car, 0.0);
        cross_gate(&mut app, car, 100.0);
    }
    run(&mut app, 1);
    assert_eq!(
        app.world().resource::<Session>().phase(),
        &SessionPhase::Results,
        "the local finish moves the session to Results (UI-5)"
    );
    let progress = app.world().get::<RaceProgress>(car).unwrap();
    let ParticipantState::Finished { race_ticks, .. } = progress.state else {
        panic!("the local driver finished: {:?}", progress.state);
    };
    assert_eq!(stats_of(&app).lap, Some((3, 3)), "a resolved lap parks");
    assert_eq!(stats_of(&app).place, Some((1, 2)));
    let text = line(&mut app);
    assert!(text.contains("FINISHED"), "the outcome wins: {text}");
    assert!(text.contains("1st of 2"), "the field size shows: {text}");
    let finish_ticks = format!(
        "  {:.1}s",
        race_ticks as f32 / mm2_game::RACE_TICK_HZ as f32
    );
    assert!(
        text.contains(&finish_ticks),
        "finish time is the recorded race time: {text} vs {finish_ticks}"
    );
    assert_eq!(
        app.world()
            .resource::<ResultLedger>()
            .place_of_in(generation, id),
        Some(1)
    );
    // The field races on behind the results screen (DSN-11): the timer
    // still reads the live race clock, never a frozen finish value.
    let want = timer_text(race_clock(&app));
    assert_eq!(timer_of(&app).display.as_deref(), Some(want.as_str()));

    // --- The `H` gate hides the instruments but keeps composing. -----
    app.world_mut().resource_mut::<hud::HudVisible>().0 = false;
    app.update();
    assert!(!timer_visible(&mut app));
    assert!(!stats_visible(&mut app));
    assert!(
        timer_of(&app).display.is_some(),
        "tmr= still reads demand while H hides the row"
    );
    assert!(stats_of(&app).place.is_some());
    app.world_mut().resource_mut::<hud::HudVisible>().0 = true;
    app.update();
    assert!(timer_visible(&mut app));
    assert!(stats_visible(&mut app));
}
