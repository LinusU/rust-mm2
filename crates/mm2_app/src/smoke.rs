//! Reusable evidence smokes (F00-C).
//!
//! One `smoke=` report record per run, machine-greppable, so an evidence
//! log can tell the kinds apart (`smoke=headless-physics` vs
//! `smoke=visual`) and tell missing capability (`status=unavailable`)
//! apart from an actual failure (`status=fail`). Reports are versioned by
//! the engine commit embedded at build time.
//!
//! The headless runner drives the *real* session path —
//! `load_session_world`, `advance_race`, the session lifecycle systems —
//! on a `MinimalPlugins` app, no window, no GPU. An event session goes
//! `Loading → Ready → Countdown → Playing` through the same systems the
//! windowed app runs; a load failure lands in `Failed` and reports
//! `status=fail`, never a silent roam.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_assets::Vfs;
use mm2_content::VehicleDef;
use mm2_game::{
    Banger, BangerPhase, BangerStateChanged, ImpactEvent, Mm2Vfs, PlayerVehicle, RaceProgress,
    RaceStarted, RaceState, Session, SessionConfig, SessionPhase, WorldMode, advance_session_tick,
    despawn_session_entities,
};
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

use crate::{camera, contracts, race, scripted, session};

/// Engine commit embedded by `build.rs` — reports stay versioned by the
/// exact code that produced them.
pub const COMMIT: &str = match option_env!("MM2_BUILD_COMMIT") {
    Some(c) => c,
    None => "unknown",
};

/// Headless physics smoke: `MinimalPlugins` + Avian, `--frames` updates.
pub const KIND_HEADLESS_PHYSICS: &str = "headless-physics";
/// Windowed visual smoke: real render path, `--frames`/`--screenshot`.
pub const KIND_VISUAL: &str = "visual";

/// Report header printed once per smoke invocation.
pub fn header() -> String {
    format!("mm2-smoke commit={COMMIT}")
}

/// Outcome of a smoke. Missing data or a missing display is *not* a
/// failure — it is `Unavailable`, a different status per QUALITY-GATES.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmokeStatus {
    /// The smoke ran and its criteria held.
    Pass,
    /// The smoke ran and produced a real failure (load error, NaN pose,
    /// missing capture, …).
    Fail,
    /// The smoke could not run: required data or a display/GPU is absent.
    Unavailable,
}

impl SmokeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Unavailable => "unavailable",
        }
    }

    /// Process exit code for a finished smoke: 0 pass, 3 fail,
    /// 4 unavailable. (2 stays reserved for usage errors.)
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Pass => 0,
            Self::Fail => 3,
            Self::Unavailable => 4,
        }
    }
}

/// One line of smoke evidence.
#[derive(Debug, Clone)]
pub struct SmokeRecord {
    /// [`KIND_HEADLESS_PHYSICS`] or [`KIND_VISUAL`].
    pub kind: &'static str,
    /// `dev-world` or the city's logical path (`city/sf.psdl`).
    pub world: String,
    pub status: SmokeStatus,
    /// Free-form `k=v` metrics or a failure reason.
    pub detail: String,
}

impl SmokeRecord {
    pub fn line(&self) -> String {
        format!(
            "smoke={} world={} status={} {}",
            self.kind,
            self.world,
            self.status.as_str(),
            self.detail
        )
    }
}

/// Which driver writes the player vehicle's [`VehicleInput`] in a
/// headless run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Driver {
    /// Settle, then hold full throttle straight (the original smoke).
    #[default]
    Hold,
    /// The scripted course-follower (`--bot`): steers at the live race
    /// objective through the production `VehicleInput` path, so event
    /// sessions can reach a finish/result instead of running straight
    /// off the course.
    Scripted,
}

impl Driver {
    /// Stable lowercase name for the `driver=` record field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Scripted => "scripted",
        }
    }
}

/// Run the world + player vehicle headlessly for `frames` app updates
/// (60 Hz virtual time; physics ticks at 120 Hz internally) through the
/// same session systems the windowed binary runs.
///
/// The `Hold` driver settles for up to two seconds, then holds full
/// throttle — the smoke exercises input → simulation → telemetry, not
/// just spawning. The `Scripted` driver inserts [`ScriptedDrive`] and
/// lets `scripted_drive` steer at the live objective instead; it owns
/// the input from the first update. Dev-world criteria require the car
/// to actually drive; a city only has to load, keep the car finite and
/// grounded (props may legitimately block its path — `moved=` reports
/// how far it got either way).
///
/// Event sessions honor the countdown's input lock (throttle stays zero
/// until `RaceStarted`'s release) and report `race=`/`cp=` evidence.
/// `vfs`/`selected` are taken by value — the process exits on return.
pub fn headless_smoke(
    config: &SessionConfig,
    vfs: Vfs,
    selected: Option<VehicleDef>,
    vehicle_config: &VehicleConfig,
    frames: u32,
    driver: Driver,
) -> SmokeRecord {
    let world = match &config.world {
        WorldMode::DevWorld => "dev-world".to_string(),
        WorldMode::City { psdl } => psdl.clone(),
    };
    let record = |status: SmokeStatus, detail: String| SmokeRecord {
        kind: KIND_HEADLESS_PHYSICS,
        world: world.clone(),
        status,
        detail,
    };

    let mut session_res = Session::new();
    if let Err(e) = session_res.begin(config.clone()) {
        return record(SmokeStatus::Fail, format!("session begin: {e}"));
    }

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        // Collider systems need the mesh asset store + events; gizmo
        // storage is needed by the vehicle debug system.
        .add_plugins(AssetPlugin::default())
        .add_plugins(bevy::mesh::MeshPlugin)
        .add_plugins(bevy::gizmos::GizmoPlugin)
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Time::<Fixed>::from_hz(120.0))
        // Deterministic: every app.update() is exactly one 60 Hz frame.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )))
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .insert_resource(session_res)
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_message::<ImpactEvent>()
        .add_message::<RaceStarted>()
        .add_message::<BangerStateChanged>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<mm2_game::ResultLedger>()
        .init_resource::<mm2_game::BangerPool>()
        .init_resource::<session::SessionControl>()
        .init_resource::<ButtonInput<KeyCode>>()
        // `load_session_world` writes marker/HUD meshes into the shared
        // asset stores — nothing renders, but the system contract needs
        // them present.
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(camera::CameraMode::Chase)
        .insert_resource(session::SpawnPoint {
            position: Vec3::new(0.0, 1.5, 0.0),
            yaw: 0.0,
            trailers: Vec::new(),
        })
        .insert_resource(Mm2Vfs(vfs))
        .insert_resource(session::TunedVehicle(vehicle_config.clone()))
        .insert_resource(session::SelectedCar {
            def: selected,
            paint: 0,
        })
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                crate::banger::activate_bangers,
                crate::banger::settle_bangers,
                contracts::publish_vehicle_telemetry,
                race::reanchor_teleported_participants,
                race::advance_race,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                session::load_session_world.run_if(session::loading),
                session::session_control_input,
                (
                    despawn_session_entities.run_if(session::unloading),
                    session::drive_session,
                )
                    .chain(),
                race::update_checkpoint_markers,
                scripted::scripted_drive.run_if(resource_exists::<scripted::ScriptedDrive>),
            ),
        );
    if driver == Driver::Scripted {
        app.insert_resource(scripted::ScriptedDrive);
    }
    app.finish();
    app.cleanup();

    // The first update runs the world/event load through the real
    // session driver — `Loading → Ready → Countdown` for an event,
    // `→ Playing` for cruise, `→ Failed` on any load error.
    app.update();
    if let SessionPhase::Failed(m) = app.world().resource::<Session>().phase() {
        return record(SmokeStatus::Fail, format!("load: {m}"));
    }
    let Some(car) = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>()
        .iter(app.world())
        .next()
    else {
        return record(SmokeStatus::Fail, "no player vehicle spawned".into());
    };
    let spawn_pos = app.world().resource::<session::SpawnPoint>().position;

    let settle = frames.min(120);
    let mut saw_grounded = false;
    let mut grounded_wheels = 0usize;
    let mut peak_speed = 0.0f32;
    // Emitted banger transitions by phase — the event stream, not just
    // the end-state buckets below (a prop that activated then settled
    // counts in both).
    let mut bng_events = [0usize; 4];
    for f in 0..frames {
        // The `Hold` driver writes input directly; `Scripted` is owned
        // by `scripted_drive` inside the update. Either way the
        // countdown lock must not be bypassed (AC03) — `scripted_drive`
        // gates on it the same way `vehicle_input` does.
        if driver == Driver::Hold {
            let driving = {
                let session = app.world().resource::<Session>();
                let locked = app
                    .world()
                    .get_resource::<RaceState>()
                    .is_some_and(|r| r.input_locked() && !r.is_stale(session.generation()));
                session.is_playing() && !locked
            };
            if let Some(mut input) = app.world_mut().get_mut::<VehicleInput>(car) {
                *input = if driving && f >= settle {
                    VehicleInput {
                        throttle: 1.0,
                        ..default()
                    }
                } else {
                    VehicleInput::default()
                };
            }
        }
        app.update();
        for e in app
            .world_mut()
            .resource_mut::<Messages<BangerStateChanged>>()
            .drain()
        {
            bng_events[match e.phase {
                BangerPhase::Dormant => 0,
                BangerPhase::Active => 1,
                BangerPhase::Settled => 2,
                BangerPhase::Broken => 3,
            }] += 1;
        }
        if let Some(state) = app.world().get::<VehicleState>(car) {
            saw_grounded |= state.grounded;
            grounded_wheels = state.wheels.iter().filter(|w| w.grounded).count();
            peak_speed = peak_speed.max(state.forward_speed);
        }
    }

    let world_ecs = app.world();
    let session = world_ecs.resource::<Session>();
    let ticks = session.tick();
    // Impact evidence: the contract pipeline emitted N events over the
    // run (the spawn drop is usually one on flat ground).
    let filter = world_ecs.resource::<contracts::ImpactFilter>();
    let impacts = filter.emitted;
    let dropped = filter.dropped;
    let pos = world_ecs.get::<Position>(car).map(|p| p.0);
    let vel = world_ecs.get::<LinearVelocity>(car).map(|v| v.0);
    let rot = world_ecs.get::<Rotation>(car).map(|r| r.0);
    let race_detail = world_ecs
        .get_resource::<RaceState>()
        .map_or_else(String::new, |r| {
            let cleared = world_ecs
                .get::<RaceProgress>(car)
                .map_or(0, |p| p.cleared_count());
            let ledger = world_ecs.resource::<mm2_game::ResultLedger>();
            let limit = r
                .time_remaining()
                .map(|t| format!(" tl={:.1}s", t as f32 / mm2_game::RACE_TICK_HZ as f32))
                .unwrap_or_default();
            let outcome = ledger
                .iter()
                .next()
                .map(|s| format!(" outcome={}", s.outcome.name()))
                .unwrap_or_default();
            format!(
                " race={:?} cp={}/{} results={}{}{}",
                r.phase,
                cleared,
                r.definition.checkpoints.len(),
                ledger.len(),
                limit,
                outcome,
            )
        });
    // `--nav` evidence: the graph + overrides loaded through the real
    // session path (`dev.nav_overlay` on the session config).
    let nav_detail = world_ecs
        .get_resource::<crate::nav_overlay::CityNav>()
        .map(|n| {
            let s = crate::nav_overlay::hud_summary(n);
            format!(" nav={}", s.strip_prefix("nav ").unwrap_or(&s))
        })
        .unwrap_or_default();
    // Banger evidence: how many bound placements exist and how the
    // dormant → active → settled/broken machine left them at the
    // frame cap.
    let bng_detail = {
        let mut counts = [0usize; 4];
        for e in world_ecs.iter_entities() {
            let Some(b) = e.get::<Banger>() else {
                continue;
            };
            counts[match b.phase {
                BangerPhase::Dormant => 0,
                BangerPhase::Active => 1,
                BangerPhase::Settled => 2,
                BangerPhase::Broken => 3,
            }] += 1;
        }
        if counts.iter().sum::<usize>() > 0 {
            format!(
                " bng={}d/{}a/{}s/{}b bng_ev={}a/{}s/{}b",
                counts[0],
                counts[1],
                counts[2],
                counts[3],
                bng_events[1],
                bng_events[2],
                bng_events[3],
            )
        } else {
            String::new()
        }
    };
    let detail = |extra: &str| {
        format!(
            "updates={frames} ticks={ticks} driver={} phase={} impacts={impacts} dropped={dropped} peak={peak_speed:.1}m/s moved={moved:.0}m wheels={grounded_wheels}/{total} final=({x:.0},{y:.1},{z:.0}){race_detail}{nav_detail}{bng_detail}{extra}",
            driver.as_str(),
            session.phase().name(),
            moved = pos.map(|p| (p - spawn_pos).length()).unwrap_or(f32::NAN),
            total = vehicle_config.wheels.len(),
            x = pos.map(|p| p.x).unwrap_or(f32::NAN),
            y = pos.map(|p| p.y).unwrap_or(f32::NAN),
            z = pos.map(|p| p.z).unwrap_or(f32::NAN),
        )
    };

    let finite = pos.is_some_and(|p| p.is_finite())
        && vel.is_some_and(|v| v.is_finite())
        && rot.is_some_and(|r| r.is_finite());
    if !finite {
        return record(SmokeStatus::Fail, detail(" non-finite pose"));
    }
    if !saw_grounded {
        return record(SmokeStatus::Fail, detail(" never grounded"));
    }
    if let Some(p) = pos
        && p.y < spawn_pos.y - 25.0
    {
        return record(SmokeStatus::Fail, detail(" fell through the world"));
    }
    // The dev world is flat and empty ahead of spawn — a healthy car must
    // be able to drive. A city can legitimately wall the car in, so its
    // bar is load + finite + grounded only.
    if matches!(&config.world, WorldMode::DevWorld)
        && !matches!(&config.mode, mm2_game::SessionMode::Event(_))
        && peak_speed < 5.0
    {
        return record(SmokeStatus::Fail, detail(" car never drove"));
    }
    record(SmokeStatus::Pass, detail(""))
}
