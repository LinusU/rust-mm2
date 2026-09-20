//! Reusable evidence smokes (F00-C).
//!
//! One `smoke=` report record per run, machine-greppable, so an evidence
//! log can tell the kinds apart (`smoke=headless-physics` vs
//! `smoke=visual`) and tell missing capability (`status=unavailable`)
//! apart from an actual failure (`status=fail`). Reports are versioned by
//! the engine commit embedded at build time.
//!
//! The headless runner reproduces the binary's world/vehicle spawn on a
//! `MinimalPlugins` app — no window, no GPU — the same pattern
//! `mm2_vehicle/tests/drive.rs` and `examples/drive_probe.rs` use.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_assets::Vfs;
use mm2_content::VehicleDef;
use mm2_game::{
    PlayerVehicle, Session, SessionConfig, SessionEntity, SessionPhase, WorldMode,
    advance_session_tick,
};
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{VehicleConfig, VehiclePlugin, vehicle_bundle};

use crate::{city, dev_world};

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

/// Run the world + player vehicle headlessly for `frames` app updates
/// (60 Hz virtual time; physics ticks at 120 Hz internally).
///
/// The car settles for up to two seconds, then holds full throttle — the
/// smoke exercises input → simulation → telemetry, not just spawning.
/// Dev-world criteria require the car to actually drive; a city only has
/// to load, keep the car finite and grounded (props may legitimately
/// block its path — `moved=` reports how far it got either way).
///
/// The run drives the real `Session` lifecycle (`Menu → Loading → Ready
/// → Playing`, `Failed` on a load error) and reports `ticks=` — the
/// fixed-step session clock, which must outpace `updates=` at exactly
/// the 120/60 Hz ratio.
///
/// `selected` mirrors the binary's spawn rule: imported cars get extra
/// spawn clearance from their authored collider hull; the synthetic dev
/// car does not.
pub fn headless_smoke(
    config: &SessionConfig,
    vfs: &Vfs,
    selected: Option<&VehicleDef>,
    vehicle_config: &VehicleConfig,
    frames: u32,
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

    let mut session = Session::new();
    if let Err(e) = session.begin(config.clone()) {
        return record(SmokeStatus::Fail, format!("session begin: {e}"));
    }
    let owner = SessionEntity(session.generation());

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
        .insert_resource(session)
        .add_plugins(TransformPlugin)
        .add_plugins(VehiclePlugin)
        .add_systems(FixedUpdate, advance_session_tick);
    app.finish();
    app.cleanup();

    // Spawn the world through the same importers the windowed app uses.
    // The asset stores are throwaway — nothing renders — but the city and
    // dev-world builders take them by contract.
    let spawned = {
        let world_ecs = app.world_mut();
        let mut queue = CommandQueue::default();
        let result = {
            let mut commands = Commands::new(&mut queue, world_ecs);
            let (mut meshes, mut images, mut materials) = (
                Assets::<Mesh>::default(),
                Assets::<Image>::default(),
                Assets::<StandardMaterial>::default(),
            );
            match &config.world {
                WorldMode::DevWorld => {
                    dev_world::spawn_dev_world(
                        &mut commands,
                        &mut meshes,
                        &mut images,
                        &mut materials,
                        vfs,
                        owner,
                    );
                    Ok((Vec3::new(0.0, 1.5, 0.0), 0.0))
                }
                WorldMode::City { psdl } => city::load_city(
                    &mut commands,
                    vfs,
                    psdl,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    owner,
                )
                .map(|loaded| (loaded.spawn, loaded.spawn_yaw)),
            }
        };
        queue.apply(world_ecs);
        result
    };
    let (mut spawn_pos, spawn_yaw) = match spawned {
        Ok(v) => v,
        Err(e) => {
            // The session records the failure too — the phase is the
            // observable state, not just the record line.
            let _ = app
                .world_mut()
                .resource_mut::<Session>()
                .fail(format!("world load: {e}"));
            return record(SmokeStatus::Fail, format!("world load: {e}"));
        }
    };
    {
        let mut session = app.world_mut().resource_mut::<Session>();
        session
            .transition(SessionPhase::Ready)
            .expect("Loading → Ready is a legal transition");
        session
            .transition(SessionPhase::Playing)
            .expect("Ready → Playing is a legal transition");
    }

    // Same spawn clearance the binary gives imported vehicles.
    if selected.is_some() {
        let hull_min_y = vehicle_config
            .collider_points
            .as_ref()
            .and_then(|pts| pts.iter().map(|p| p[1]).reduce(f32::min))
            .unwrap_or(-vehicle_config.chassis_size[1] * 0.5);
        spawn_pos.y += (0.25 - hull_min_y).max(0.35);
    }
    let car = app
        .world_mut()
        .spawn((
            PlayerVehicle,
            owner,
            vehicle_bundle(vehicle_config),
            Transform::from_translation(spawn_pos).with_rotation(Quat::from_rotation_y(spawn_yaw)),
        ))
        .id();

    let settle = frames.min(120);
    let mut saw_grounded = false;
    let mut grounded_wheels = 0usize;
    let mut peak_speed = 0.0f32;
    for f in 0..frames {
        if let Some(mut input) = app.world_mut().get_mut::<VehicleInput>(car) {
            *input = if f >= settle {
                VehicleInput {
                    throttle: 1.0,
                    ..default()
                }
            } else {
                VehicleInput::default()
            };
        }
        app.update();
        if let Some(state) = app.world().get::<VehicleState>(car) {
            saw_grounded |= state.grounded;
            grounded_wheels = state.wheels.iter().filter(|w| w.grounded).count();
            peak_speed = peak_speed.max(state.forward_speed);
        }
    }

    let world_ecs = app.world();
    let ticks = world_ecs.resource::<Session>().tick();
    let pos = world_ecs.get::<Position>(car).map(|p| p.0);
    let vel = world_ecs.get::<LinearVelocity>(car).map(|v| v.0);
    let rot = world_ecs.get::<Rotation>(car).map(|r| r.0);
    let detail = |extra: &str| {
        format!(
            "updates={frames} ticks={ticks} peak={peak_speed:.1}m/s moved={moved:.0}m wheels={grounded_wheels}/{total} final=({x:.0},{y:.1},{z:.0}){extra}",
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
    if matches!(&config.world, WorldMode::DevWorld) && peak_speed < 5.0 {
        return record(SmokeStatus::Fail, detail(" car never drove"));
    }
    record(SmokeStatus::Pass, detail(""))
}
