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

use std::path::Path;
use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_assets::Vfs;
use mm2_game::{
    Banger, BangerPhase, BangerStateChanged, DamageEvent, ImpactEvent, Mm2Vfs, ParticipantState,
    PlayerVehicle, RaceProgress, RaceStarted, RaceState, Session, SessionConfig, SessionPhase,
    WorldMode, advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

use crate::{camera, city, contracts, damage, opponents, race, scripted, session};

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

/// Clear a previous capture at `path` so a later write is the only
/// thing a "file landed" check can see — a file already at the target
/// would satisfy it on stale pixels before this run's screenshot is
/// even written. `NotFound` is the expected case (a fresh target); any
/// other error means the target cannot be made fresh and the caller
/// must fail rather than risk reporting a stale capture as this run's.
pub fn clear_stale_screenshot(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
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
    /// The stationary control (`--parked`): holds the handbrake for the
    /// whole session through the production `VehicleInput` path, so an
    /// event run measures what the opponents do with no competing local
    /// driver — the isolation leg `Hold` (blind full throttle) cannot
    /// provide.
    Parked,
}

impl Driver {
    /// Stable lowercase name for the `driver=` record field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Scripted => "scripted",
            Self::Parked => "parked",
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
/// the input from the first update. The `Parked` driver inserts
/// [`crate::input::ParkedDrive`] so `parked_drive` holds the handbrake —
/// the stationary control leg. Dev-world criteria require the car to
/// actually drive under a driver that requests motion (Parked is
/// exempt — its evidence is the car staying put); a city only has to
/// load, keep the car finite and grounded (props may legitimately block
/// its path — `moved=` reports how far it got either way).
///
/// Event sessions honor the countdown's input lock (throttle stays zero
/// until `RaceStarted`'s release) and report `race=`/`cp=` evidence.
/// `vfs`/`car` are taken by value — the process exits on return. A
/// bound `profile` is inserted as a resource so the session-load path
/// records its selections through the same code the windowed app runs.
pub fn headless_smoke(
    config: &SessionConfig,
    vfs: Vfs,
    car: session::SelectedCar,
    vehicle_config: &VehicleConfig,
    frames: u32,
    driver: Driver,
    profile: Option<crate::profile::ActiveProfile>,
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
        .add_message::<DamageEvent>()
        .add_message::<mm2_game::StuckEvent>()
        .add_message::<mm2_game::PartDetached>()
        .add_message::<mm2_game::RecoveryEvent>()
        .add_message::<RaceStarted>()
        .add_message::<BangerStateChanged>()
        .init_resource::<contracts::ImpactFilter>()
        .init_resource::<damage::DamageReport>()
        .init_resource::<crate::stuck::StuckReport>()
        .init_resource::<crate::breakaway::BreakReport>()
        .init_resource::<crate::recovery::RecoveryReport>()
        .init_resource::<crate::damage_fx::SmokeFxReport>()
        .init_resource::<crate::spark_fx::SparkFxReport>()
        .init_resource::<crate::texel_fx::TexelDamageReport>()
        .init_resource::<crate::audio::AudioReport>()
        .init_resource::<Assets<crate::audio::PcmAudio>>()
        .add_message::<crate::audio::HornRequest>()
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
        .insert_resource(car)
        .add_systems(FixedUpdate, advance_session_tick)
        .add_systems(
            FixedLast,
            (
                contracts::collect_impacts,
                // F05-B.1: impact→damage apply + disabled outcome — the
                // headless record's `dmg=` field reads the report.
                damage::apply_impact_damage,
                // F05-B.9: texel splats off the same stream — the
                // headless record's `txl=` field reads the report.
                crate::texel_fx::apply_texel_damage,
                // F05-B.2: `vehstuck` detection + in-place recovery —
                // the headless record's `vsk=` field reads the report.
                crate::stuck::track_stuck,
                // F05-B.3: authored breakaway detachment — the
                // headless record's `brk=` field reads the report.
                crate::breakaway::detach_breaks,
                damage::resolve_disabled,
                // F05-B.7: damage→engine impairment (DSN-25) — the
                // headless record's `imp=` field reads the report.
                damage::sync_impairment,
                crate::stuck::resolve_stuck,
                // F05-B.5: water/OOB recovery — the headless record's
                // `rcv=` field reads the report.
                crate::recovery::track_recovery,
                crate::recovery::resolve_recovery,
                crate::banger::activate_bangers,
                crate::banger::settle_bangers,
                contracts::publish_vehicle_telemetry,
                race::reanchor_teleported_participants,
                race::advance_race,
                // F10-A.2: ambient lane-following + recycle/respawn —
                // the headless record's `traf=` field reads the state
                // these leave behind. F10-B.6's handover runs before
                // the driver (knock → drive → maintain); the B.7
                // signal update reads the controller state the driver
                // just advanced, so it runs last.
                crate::traffic::knock_ambient,
                crate::traffic::drive_ambient,
                crate::traffic::maintain_ambient,
                crate::traffic::drive_signals,
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
                    // `--restart` queues the session's own restart
                    // intent on the first `Playing` frame — evidence
                    // for the production teardown/begin lifecycle.
                    session::dev_restart_once,
                    session::drive_session,
                )
                    .chain(),
                race::update_checkpoint_markers,
                scripted::scripted_drive.run_if(resource_exists::<scripted::ScriptedDrive>),
                // F13-C.2: the parked control owns `VehicleInput` under
                // `--parked` — the same resource-gated pattern
                // `ScriptedDrive` holds for `--bot`.
                crate::input::parked_drive.run_if(resource_exists::<crate::input::ParkedDrive>),
                // F18-A.5: the chase camera tracks the player headlessly
                // so the authored room-PVS pass resolves a live source
                // room exactly as the windowed run does — the record's
                // `pvs=` field reports what it culled.
                camera::chase_follow,
                crate::pvs::apply_city_pvs
                    .after(camera::chase_follow)
                    .run_if(resource_exists::<crate::pvs::CityPvs>),
                // `--finish` works headless too — the record still
                // reports the real resolved outcome.
                crate::results::dev_finish_once,
                // F07-A.2/B.1/B.2: the authored-horn voice path plus
                // the engine loop rigs and the camera listener — the
                // record's `aud=` field reads the report. No
                // AudioPlugin runs here, so voices spawn and are
                // counted but never attach a sink (`sunk` stays 0 —
                // honest headless evidence of the request→voice half
                // only; the mix still computes on the components).
                (
                    crate::audio::horn_input,
                    crate::audio::dev_horn_once,
                    crate::audio::horn_voices,
                    // F07-B.7: `SIREN_FLAG` car presses toggle the
                    // authored siren program, then the drive keeps the
                    // voice on the machine's current sample — the
                    // record's `aud=` w/y fields read it headless.
                    (crate::audio::siren_toggle, crate::audio::siren_drive)
                        .chain()
                        .after(session::drive_session),
                    // `.after(drive_session)` — rig/listener commands
                    // must not queue on cars or cameras
                    // `despawn_session_entities` just killed this
                    // update (the unload chain flushes first).
                    (crate::audio::engine_rigs, crate::audio::engine_drive)
                        .chain()
                        .after(session::drive_session),
                    // F07-B.3: deduplicated impacts → bounded one-shot
                    // voices — same despawn ordering as the rigs.
                    crate::audio::impact_voices.after(session::drive_session),
                    // F07-B.5: committed gear/direction changes →
                    // clutch one-shots — the record's `aud=` `c` field
                    // counts them.
                    crate::audio::clutch_voices.after(session::drive_session),
                    // F07-B.4: wheel contact → skid/rolling loop
                    // voices — the mix computes on the components, so
                    // the record's `aud=` k/g gauges read it headless.
                    crate::audio::surface_voices.after(session::drive_session),
                    // F07-B.6: ambient engine tables → looping voices —
                    // the record's `aud=` e/n fields read the mix the
                    // same headless way.
                    (
                        crate::audio::ambient_engine_rigs,
                        crate::audio::ambient_engine_drive,
                    )
                        .chain()
                        .after(session::drive_session),
                    crate::audio::audio_listener.after(session::drive_session),
                    crate::audio::count_sinks,
                    crate::audio::sync_audio_pause,
                    crate::audio::reset_audio_report.run_if(session::unloading),
                ),
                opponents::opponent_drive,
                // F05-B.6: authored engine smoke — the headless
                // record's `ptx=` field reads the report. Assets are
                // real (VFS `fxpt2` + quads); nothing rasterizes.
                (
                    crate::damage_fx::drive_smoke,
                    crate::damage_fx::advance_smoke,
                )
                    .chain(),
                // F05-B.8: authored impact sparks — the headless
                // record's `spk=` field reads the report.
                (
                    crate::spark_fx::emit_sparks,
                    crate::spark_fx::advance_sparks,
                )
                    .chain(),
                // F16-B: the same result → profile consumption the
                // windowed app runs — a bound profile in a headless
                // evidence run must record identically.
                crate::progression::record_session_results,
            ),
        );
    if driver == Driver::Scripted {
        app.insert_resource(scripted::ScriptedDrive);
    }
    if driver == Driver::Parked {
        app.insert_resource(crate::input::ParkedDrive);
    }
    if let Some(profile) = profile {
        app.insert_resource(profile);
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
    let mut player_query = app
        .world_mut()
        .query_filtered::<Entity, With<PlayerVehicle>>();
    if player_query.iter(app.world()).next().is_none() {
        return record(SmokeStatus::Fail, "no player vehicle spawned".into());
    }
    let spawn_pos = app.world().resource::<session::SpawnPoint>().position;
    // The generation the run started with: a session that restarts
    // mid-run (a `RestartEvent` disabled outcome, `--restart`, a
    // Backspace intent) bumps it, and the record reports the delta as
    // `rs=` rather than mistaking the fresh session for the first.
    let initial_generation = app.world().resource::<Session>().generation();

    let diag = std::env::var_os("MM2_SMOKE_DIAG").is_some();
    let settle = frames.min(120);
    let mut saw_grounded = false;
    let mut grounded_wheels = 0usize;
    let mut peak_speed = 0.0f32;
    // Emitted banger transitions by phase — the event stream, not just
    // the end-state buckets below (a prop that activated then settled
    // counts in both). `bng_reclaims` separately counts settles whose
    // cause is the pool reclaiming a slot — otherwise a pool-settle and
    // an Avian-sleep settle are indistinguishable in the record.
    let mut bng_events = [0usize; 4];
    let mut bng_reclaims = 0usize;
    for f in 0..frames {
        // The player entity is re-resolved every frame: a mid-run
        // session restart (`RestartEvent` disabled outcome, `--restart`)
        // despawns the session-owned car and spawns a new one — a
        // cached entity would silently read nothing (or a recycled
        // slot), which is how a legitimate restart once reported
        // `final=(NaN,NaN,NaN)` as a "non-finite pose".
        let player = player_query.iter(app.world()).next();
        // The `Hold` driver writes input directly; `Scripted`/`Parked`
        // are owned by `scripted_drive`/`parked_drive` inside the
        // update. Either way the countdown lock must not be bypassed
        // (AC03) — `scripted_drive` gates on it the same way
        // `vehicle_input` does (`parked_drive` writes only a held
        // handbrake, which cannot launch the car).
        if driver == Driver::Hold {
            let driving = {
                let session = app.world().resource::<Session>();
                let locked = app
                    .world()
                    .get_resource::<RaceState>()
                    .is_some_and(|r| r.input_locked() && !r.is_stale(session.generation()));
                session.is_playing() && !locked
            };
            if let Some(mut input) = player.and_then(|e| app.world_mut().get_mut::<VehicleInput>(e))
            {
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
        let t0 = std::time::Instant::now();
        app.update();
        if diag {
            let mut q = app.world_mut().query::<(
                Entity,
                &LinearVelocity,
                Option<&Position>,
                Option<&Name>,
                Has<Banger>,
                Has<mm2_vehicle::vehicle::Vehicle>,
            )>();
            let mut top: Vec<(f32, Entity, String, Vec3, bool, bool)> = q
                .iter(app.world())
                .filter(|(_, v, _, _, _, _)| v.0.length() > 150.0)
                .map(|(e, v, p, n, b, veh)| {
                    (
                        v.0.length(),
                        e,
                        n.map(|x| x.to_string()).unwrap_or_default(),
                        p.map(|p| p.0).unwrap_or(Vec3::ZERO),
                        b,
                        veh,
                    )
                })
                .collect();
            top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            for (s, e, n, p, b, veh) in top.iter().take(5) {
                eprintln!("FAST f={f} e={e:?} name={n} |v|={s:.0} pos={p:?} bng={b} veh={veh}");
            }
        }
        if diag && (f % 50 == 0 || t0.elapsed() > Duration::from_millis(100)) {
            let ents = app.world().entity_count();
            let (ap, sp, edges) = app
                .world()
                .get_resource::<ContactGraph>()
                .map(|g| {
                    (
                        g.iter_active().count(),
                        g.iter_sleeping().count(),
                        g.edges.edge_count(),
                    )
                })
                .unwrap_or((0, 0, 0));
            let cs = app
                .world()
                .get_resource::<Messages<CollisionStart>>()
                .map(|m| m.len())
                .unwrap_or(0);
            let ce = app
                .world()
                .get_resource::<Messages<CollisionEnd>>()
                .map(|m| m.len())
                .unwrap_or(0);
            let bangers = {
                let mut q = app.world_mut().query::<&Banger>();
                let mut c = [0usize; 4];
                for b in q.iter(app.world()) {
                    c[match b.phase {
                        BangerPhase::Dormant => 0,
                        BangerPhase::Active => 1,
                        BangerPhase::Settled => 2,
                        BangerPhase::Broken => 3,
                    }] += 1;
                }
                c
            };
            eprintln!(
                "diag f={f} dt={:?} ents={ents} pairs={ap}+{sp} edges={edges} cstart={cs} cend={ce} bng={}/{}/{}/{}",
                t0.elapsed(),
                bangers[0],
                bangers[1],
                bangers[2],
                bangers[3]
            );
        }
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
            if e.cause == mm2_game::BangerCause::Reclaimed {
                bng_reclaims += 1;
            }
        }
        // Re-resolve after the update: teardown/respawn happened inside
        // it, so the pre-update entity may be gone or replaced.
        let player = player_query.iter(app.world()).next();
        if let Some(state) = player.and_then(|e| app.world().get::<VehicleState>(e)) {
            saw_grounded |= state.grounded;
            grounded_wheels = state.wheels.iter().filter(|w| w.grounded).count();
            peak_speed = peak_speed.max(state.forward_speed);
        }
    }

    let world_ecs = app.world();
    let session = world_ecs.resource::<Session>();
    let ticks = session.tick();
    // Session restarts observed over the run — `begin` bumps the
    // generation, so the delta counts teardown/begin cycles the run
    // went through (`rs=` only appears when one did).
    let restarts = session.generation().saturating_sub(initial_generation);
    // The live player at the frame cap: `None` while the session sits
    // in the teardown/rebuild window, which is a lifecycle state — not
    // a missing pose.
    let player = player_query.iter(world_ecs).next();
    // Impact evidence: the contract pipeline emitted N events over the
    // run (the spawn drop is usually one on flat ground).
    let filter = world_ecs.resource::<contracts::ImpactFilter>();
    let impacts = filter.emitted;
    let dropped = filter.dropped;
    let pos = player
        .and_then(|e| world_ecs.get::<Position>(e))
        .map(|p| p.0);
    let vel = player
        .and_then(|e| world_ecs.get::<LinearVelocity>(e))
        .map(|v| v.0);
    let rot = player
        .and_then(|e| world_ecs.get::<Rotation>(e))
        .map(|r| r.0);
    let race_detail = world_ecs
        .get_resource::<RaceState>()
        .map_or_else(String::new, |r| {
            let progress = player.and_then(|e| world_ecs.get::<RaceProgress>(e));
            let cleared = progress.map_or(0, |p| p.cleared_count());
            // Ordered (Circuit) runs also name the lap in progress —
            // `lap` counts completed laps, so `lap+1` is the lap under
            // way, clamped at the finish. Any-order records stay
            // bit-identical.
            let lap = if r.definition.rule == mm2_game::CheckpointRule::Ordered {
                let current = progress.map_or(1, |p| (p.lap + 1).min(r.definition.laps));
                format!(" lap={current}/{}", r.definition.laps)
            } else {
                String::new()
            };
            let ledger = world_ecs.resource::<mm2_game::ResultLedger>();
            // The ledger outlives one session, so the result count
            // scopes to the current generation like the standings
            // below — a restart's stale results must not inflate it.
            let result_count = ledger.standings_in(session.generation()).len();
            let limit = r
                .time_remaining()
                .map(|t| format!(" tl={:.1}s", t as f32 / mm2_game::RACE_TICK_HZ as f32))
                .unwrap_or_default();
            // The local participant (there is only ever one in a real
            // run today, but pick by id so the record names the driver,
            // not an arbitrary participant).
            let local = player
                .and_then(|e| world_ecs.get::<mm2_game::Player>(e))
                .map(|p| p.id);
            // The live running order — the local participant's place
            // in it (DSN-13). Unlike the standings `place=`, this
            // exists before anyone resolves.
            let order = mm2_game::live_order(
                &r.definition,
                world_ecs.iter_entities().filter_map(|e| {
                    match (
                        e.get::<mm2_game::Player>(),
                        e.get::<RaceProgress>(),
                        e.get::<Position>(),
                    ) {
                        (Some(p), Some(prog), Some(pos)) => Some((p.id, prog, pos.0)),
                        _ => None,
                    }
                }),
            );
            let pos = local
                .and_then(|id| order.iter().position(|p| *p == id))
                .map(|i| format!(" pos={}/{}", i + 1, order.len()))
                .unwrap_or_default();
            // The local participant's result plus its place in the
            // ledger's standings (F13-B) — scoped to the current
            // session generation.
            let outcome = result_outcome(ledger, session.generation(), local);
            // F15-A.2: spawned opponents and how many resolved
            // (finished/timed out). Absent on runs without a roster so
            // older records stay bit-identical. `opp_rec` counts the
            // field's disclosed re-anchor teleports (F15-B.3) and only
            // appears when one fired; `cu` counts drivers whose
            // designed catch-up assist is currently lifting their
            // demand ceiling (F15-B.4, DSN-27) — likewise only on
            // activity, so an unassisted run stays identical.
            // `opps=` (F15-B.7, spec req 6) is the per-opponent
            // breakdown in authored roster order — progress, stuck
            // duration and recovery actions per driver, so a soak no
            // longer needs out-of-tree instrumentation to say *which*
            // cars cleared their gates.
            let ordered = r.definition.rule == mm2_game::CheckpointRule::Ordered;
            let mut rows: Vec<OppRow> = Vec::new();
            let (opp, opp_done, opp_rec, opp_cu) = world_ecs.iter_entities().fold(
                (0usize, 0usize, 0usize, 0usize),
                |(n, d, rec, c), e| {
                    let Some(driver) = e.get::<opponents::OpponentDriver>() else {
                        return (n, d, rec, c);
                    };
                    let progress = e.get::<RaceProgress>();
                    let resolved = progress.and_then(|p| match p.state {
                        ParticipantState::Finished { .. } => Some('F'),
                        ParticipantState::TimedOut { .. } => Some('T'),
                        _ => None,
                    });
                    rows.push(OppRow {
                        index: driver.index,
                        vehicle: driver.spec.vehicle.as_str(),
                        cleared: progress.map_or(0, |p| p.cleared_count()),
                        lap: if ordered {
                            progress.map(|p| (p.lap + 1).min(r.definition.laps))
                        } else {
                            None
                        },
                        resolved,
                        escapes: driver.recovery.escapes,
                        reanchors: driver.reanchors,
                        stuck: driver.stuck_peak,
                    });
                    (
                        n + 1,
                        d + usize::from(resolved.is_some()),
                        rec + driver.reanchors as usize,
                        c + usize::from(driver.catch_up > 0.0),
                    )
                },
            );
            let opp = if opp > 0 {
                let rec = if opp_rec > 0 {
                    format!(" opp_rec={opp_rec}")
                } else {
                    String::new()
                };
                let cu = if opp_cu > 0 {
                    format!(" cu={opp_cu}")
                } else {
                    String::new()
                };
                format!(
                    " opp={opp_done}/{opp}{rec}{cu}{}",
                    opponent_detail(&mut rows)
                )
            } else {
                String::new()
            };
            format!(
                " race={:?} cp={}/{} results={}{}{}{}{}{}",
                r.phase,
                cleared,
                r.definition.checkpoints.len(),
                result_count,
                lap,
                limit,
                pos,
                outcome,
                opp,
            )
        });
    // F15-B.5 scripted-player evidence: the bounded re-anchor
    // teleports (`r`) and three-point escapes (`e`) the `--bot` driver
    // took this session — the per-player counterpart of `opp_rec=`,
    // previously `info!`-only. On activity only, so a clean run stays
    // bit-identical.
    let p_rec_detail = {
        let reanchors = player
            .and_then(|e| world_ecs.get::<scripted::ScriptedRoute>(e))
            .map(|r| r.reanchors)
            .unwrap_or(0);
        let escapes = player
            .and_then(|e| world_ecs.get::<scripted::ScriptedBot>(e))
            .map(|b| b.escapes)
            .unwrap_or(0);
        if reanchors + escapes > 0 {
            format!(" p_rec={reanchors}r/{escapes}e")
        } else {
            String::new()
        }
    };
    // `--nav` evidence: the graph + overrides loaded through the real
    // session path (`dev.nav_overlay` on the session config).
    let nav_detail = world_ecs
        .get_resource::<crate::nav_overlay::CityNav>()
        .map(|n| {
            let s = crate::nav_overlay::hud_summary(n);
            format!(" nav={}", s.strip_prefix("nav ").unwrap_or(&s))
        })
        .unwrap_or_default();
    // F18-A.2 environment evidence: which `.ltNN` preset the session's
    // effective conditions bound (`ltNN(<name>)`), or `ltNN(fallback)`
    // when the preset could not load and the fallback rig spawned
    // (F18-AC06's explicit diagnostic). Absent on the dev world so
    // those records stay bit-identical.
    let env_detail = world_ecs
        .get_resource::<crate::environment::EnvironmentReport>()
        .map(|r| format!(" env={}", r.smoke_detail()))
        .unwrap_or_default();
    // F18-A.5 PVS evidence: the resolved source room and how many
    // room-tagged render entities the authored table culled
    // (`pvs=<room>r/<culled>h/<tagged>`). Absent without a loaded table
    // so dev-world/mod-city records stay bit-identical.
    let pvs_detail = world_ecs
        .get_resource::<crate::pvs::CityPvs>()
        .map(|p| {
            if p.enabled {
                format!(" pvs={}r/{}h/{}", p.source_room(), p.culled, p.tagged)
            } else {
                " pvs=off".to_string()
            }
        })
        .unwrap_or_default();
    // F18-A.6/.7 deadly-water evidence: the authored level, how many
    // `.water` refs resolved to rooms and how many rooms the SDL pass
    // marked (`wtr=<level>/<refs>r`, plus `+Nsdl` for SDL marks and
    // `+Ns` when refs were skipped). Absent without a loaded record so
    // dev-world and mod-city runs stay bit-identical.
    let wtr_detail = world_ecs
        .get_resource::<crate::water::CityWater>()
        .map(|w| {
            let sdl = if w.sdl_rooms() > 0 {
                format!("+{}sdl", w.sdl_rooms())
            } else {
                String::new()
            };
            let skipped = if w.skipped() > 0 {
                format!("+{}s", w.skipped())
            } else {
                String::new()
            };
            format!(
                " wtr={}/{}r{}{}",
                w.level(),
                w.room_count() - w.sdl_rooms(),
                sdl,
                skipped
            )
        })
        .unwrap_or_default();
    // F10-A.2 ambient evidence: live/target population plus the
    // recycler counters. Absent on worlds without a rostered aimap so
    // those records stay bit-identical.
    let traf_detail = world_ecs
        .get_resource::<crate::traffic::AmbientTraffic>()
        .map(|t| {
            let active = world_ecs
                .iter_entities()
                .filter(|e| e.get::<crate::traffic::AmbientCar>().is_some())
                .count();
            let mut s = format!(
                " traf={}/{} sp={} rec={} dead={} uns={} q={} jq={} stuck={} crx={} jmp={}",
                active,
                t.target,
                t.spawned,
                t.recycled,
                t.dead_ends,
                t.unspawnable,
                t.queued,
                t.junction_held,
                t.stuck,
                t.crossings,
                t.jumps,
            );
            // F10-B.6 handover count only when nonzero — records from
            // knock-free runs stay bit-identical to earlier ones.
            if t.knocked > 0 {
                s.push_str(&format!(" kn={}", t.knocked));
            }
            // F10-B.7 authored signal indicators — `sig` only when
            // any spawned (a BAI without authored light origins stays
            // bit-identical), `sigd` only when the sanity bound
            // dropped outliers.
            if t.signals > 0 {
                s.push_str(&format!(" sig={}", t.signals));
            }
            if t.signals_dropped > 0 {
                s.push_str(&format!(" sigd={}", t.signals_dropped));
            }
            s
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
            // The dev `--banger-pool` bound is recorded when set so a
            // reclaim run is self-describing; default runs stay
            // bit-identical to earlier records.
            let pool = config
                .dev
                .banger_pool
                .map(|n| format!(" bng_pool={n}"))
                .unwrap_or_default();
            let rec = if bng_reclaims > 0 {
                format!(" bng_rec={bng_reclaims}")
            } else {
                String::new()
            };
            format!(
                " bng={}d/{}a/{}s/{}b bng_ev={}a/{}s/{}b{pool}{rec}",
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
    // F05-B.1 damage evidence: applied/disabled/recovered counts.
    // Only recorded once the pipeline saw any delivery, so impact-free
    // runs stay bit-identical to earlier records.
    let dmg_detail = world_ecs
        .get_resource::<damage::DamageReport>()
        .filter(|r| r.applied + r.rejected + r.duplicate > 0)
        .map(|r| {
            format!(
                " dmg={}a/{}d/{}r rej={} dup={}",
                r.applied, r.disabled, r.recovered, r.rejected, r.duplicate
            )
        })
        .unwrap_or_default();
    // F05-B.2 stuck evidence: armed/detected/recovered counts. Same
    // presence rule as `dmg=` — recorded only once the pipeline saw a
    // delivery, so impact-free runs stay bit-identical.
    let vsk_detail = world_ecs
        .get_resource::<crate::stuck::StuckReport>()
        .filter(|r| r.armed + r.detections + r.recovered > 0)
        .map(|r| format!(" vsk={}a/{}d/{}r", r.armed, r.detections, r.recovered))
        .unwrap_or_default();
    // F05-B.3 breakaway evidence: detached/restored counts. Same
    // presence rule — recorded only once the pipeline saw a delivery.
    let brk_detail = world_ecs
        .get_resource::<crate::breakaway::BreakReport>()
        .filter(|r| r.detached + r.restored > 0)
        .map(|r| format!(" brk={}d/{}r", r.detached, r.restored))
        .unwrap_or_default();
    // F05-B.4 gyro evidence: latched spin activations/completions on
    // the local car. Same presence rule — a run whose driver never
    // pulled a gyro maneuver stays bit-identical.
    let gyr_detail = player
        .and_then(|e| world_ecs.get::<VehicleState>(e))
        .filter(|s| s.gyro_spins + s.gyro_completed > 0)
        .map(|s| format!(" gyr={}/{}", s.gyro_spins, s.gyro_completed))
        .unwrap_or_default();
    // F05-B.5 recovery evidence: submersion/out-of-bounds detections
    // and the recoveries they resolved to. Same presence rule — a run
    // that never left dry ground stays bit-identical.
    let rcv_detail = world_ecs
        .get_resource::<crate::recovery::RecoveryReport>()
        .filter(|r| r.submerged + r.out_of_bounds + r.recovered > 0)
        .map(|r| {
            format!(
                " rcv={}w/{}f/{}r",
                r.submerged, r.out_of_bounds, r.recovered
            )
        })
        .unwrap_or_default();
    // F05-B.6 smoke evidence: emitted/expired puff counts. Same
    // presence rule — an undamaged run stays bit-identical.
    let ptx_detail = world_ecs
        .get_resource::<crate::damage_fx::SmokeFxReport>()
        .filter(|r| r.emitted + r.expired > 0)
        .map(|r| format!(" ptx={}e/{}x", r.emitted, r.expired))
        .unwrap_or_default();
    // F05-B.7 impairment evidence: episodes entering/leaving the
    // impaired band (DSN-25). Same presence rule — a run under
    // MedDamage the whole way stays bit-identical.
    let imp_detail = world_ecs
        .get_resource::<damage::DamageReport>()
        .filter(|r| r.impaired + r.restored > 0)
        .map(|r| format!(" imp={}i/{}r", r.impaired, r.restored))
        .unwrap_or_default();
    // F05-B.8 spark evidence: bursts/emitted/expired counts. Same
    // presence rule — an impact-free run stays bit-identical.
    let spk_detail = world_ecs
        .get_resource::<crate::spark_fx::SparkFxReport>()
        .filter(|r| r.bursts + r.emitted + r.expired > 0)
        .map(|r| format!(" spk={}b/{}e/{}x", r.bursts, r.emitted, r.expired))
        .unwrap_or_default();
    // F05-B.9 texel evidence: splatting impacts/splat stamps/repairs.
    // Same presence rule — an impact-free or unpaired-texture run
    // stays bit-identical.
    let txl_detail = world_ecs
        .get_resource::<crate::texel_fx::TexelDamageReport>()
        .filter(|r| r.impacts + r.splats + r.resets > 0)
        .map(|r| format!(" txl={}i/{}s/{}r", r.impacts, r.splats, r.resets))
        .unwrap_or_default();
    // F07-A.2/B.1/B.2/B.3/B.5/B.6 audio evidence: horn presses /
    // voices spawned / sinks the device attached / engine loops live /
    // loops audible at record time / engine rigs built, plus `/Ni`
    // impact voices, `/Nc` clutch one-shots, `/Ne` `/Nn` ambient
    // engine voices spawned/audible and `+Nd` bound-drops / `+Nx`
    // resolve/decode failures when nonzero. Activity-gated — an
    // audio-free run stays bit-identical, and headless `0s` honestly
    // reports that no output device ever saw the voice.
    let aud_detail = world_ecs
        .get_resource::<crate::audio::AudioReport>()
        .filter(|r| r.active())
        .map(|r| {
            let dropped = if r.dropped > 0 {
                format!("+{}d", r.dropped)
            } else {
                String::new()
            };
            let failed = if r.failed > 0 {
                format!("+{}x", r.failed)
            } else {
                String::new()
            };
            // F07-B.3: impact voices spawned — appended only when
            // nonzero so impact-free records stay bit-identical.
            let impacts = if r.impacts > 0 {
                format!("/{}i", r.impacts)
            } else {
                String::new()
            };
            // F07-B.5: clutch one-shots spawned — the same
            // activity-gated append.
            let clutch = if r.clutch > 0 {
                format!("/{}c", r.clutch)
            } else {
                String::new()
            };
            // F07-B.4: skid/rolling loop voices currently audible —
            // gauges like `a`, appended only when nonzero.
            let surface = if r.skids + r.rolling > 0 {
                format!("/{}k/{}g", r.skids, r.rolling)
            } else {
                String::new()
            };
            // F07-B.6: ambient engine voices spawned / currently
            // audible — spawned like `i`/`c`, audible a gauge like `a`.
            let ambient = if r.ambient + r.ambient_live > 0 {
                format!("/{}e/{}n", r.ambient, r.ambient_live)
            } else {
                String::new()
            };
            // F07-B.7: siren loop voices spawned / programs currently
            // active — spawned like `i`/`c`, live a gauge like `n`.
            let sirens = if r.sirens + r.siren_live > 0 {
                format!("/{}w/{}y", r.sirens, r.siren_live)
            } else {
                String::new()
            };
            format!(
                " aud={}h/{}v/{}s/{}l/{}a/{}r{impacts}{clutch}{surface}{ambient}{sirens}{dropped}{failed}",
                r.horns, r.voices, r.sunk, r.loops, r.audible, r.rigs
            )
        })
        .unwrap_or_default();
    // F07-B.8 surface-variant evidence: `surf=wet` when the session's
    // effective weather bound the wet table (or a per-vehicle
    // override of it). Dry/absent stays bit-identical.
    let surf_detail = world_ecs
        .get_resource::<crate::audio::SurfaceAudio>()
        .filter(|s| s.variant == mm2_game::SurfaceVariant::Wet)
        .map(|_| " surf=wet".to_string())
        .unwrap_or_default();
    // The dev `--traction` modifier is recorded when set so a wetness
    // run is self-describing; unmodified runs stay bit-identical.
    let traction_detail = config
        .dev
        .traction
        .map(|t| format!(" traction={t}"))
        .unwrap_or_default();
    // A bound driver profile is recorded so a run under persisted
    // selections is self-describing; absent without one, keeping
    // existing records bit-identical.
    let profile_detail = world_ecs
        .get_resource::<crate::profile::ActiveProfile>()
        .map(|p| format!(" profile={}", p.profile.id))
        .unwrap_or_default();
    // `rs=` only appears when the run restarted — records without a
    // teardown/begin cycle stay bit-identical.
    let rs_detail = if restarts > 0 {
        format!(" rs={restarts}")
    } else {
        String::new()
    };
    // Pose fields: `none` while no player entity exists — the run ended
    // inside the teardown/rebuild window, a lifecycle state rather than
    // a missing pose. With an entity present, a missing or non-finite
    // component still formats NaN and fails the finite check below: a
    // live player without a pose is a broken spawn, not an absent one.
    let pose_detail = if player.is_some() {
        format!(
            "moved={:.0}m wheels={}/{} final=({:.0},{:.1},{:.0})",
            pos.map(|p| (p - spawn_pos).length()).unwrap_or(f32::NAN),
            grounded_wheels,
            vehicle_config.wheels.len(),
            pos.map(|p| p.x).unwrap_or(f32::NAN),
            pos.map(|p| p.y).unwrap_or(f32::NAN),
            pos.map(|p| p.z).unwrap_or(f32::NAN),
        )
    } else {
        format!(
            "moved=none wheels={}/{} final=none",
            grounded_wheels,
            vehicle_config.wheels.len()
        )
    };
    // `diff=` names the session's configured difficulty — a run-config
    // field like `driver=`, printed unconditionally so an evidence
    // record is self-describing about which authored parameter block
    // (DRV-2/DRV-3) and aimap variant (RACE-11) selected its content.
    let detail = |extra: &str| {
        format!(
            "updates={frames} ticks={ticks}{rs_detail} driver={} diff={} phase={} impacts={impacts} dropped={dropped} peak={peak_speed:.1}m/s {pose_detail}{race_detail}{p_rec_detail}{nav_detail}{env_detail}{pvs_detail}{wtr_detail}{traf_detail}{bng_detail}{dmg_detail}{vsk_detail}{brk_detail}{gyr_detail}{rcv_detail}{ptx_detail}{imp_detail}{spk_detail}{txl_detail}{surf_detail}{aud_detail}{traction_detail}{profile_detail}{extra}",
            driver.as_str(),
            config.difficulty.as_str(),
            session.phase().name(),
        )
    };

    // Absent is only legitimate inside the transient teardown/rebuild
    // window — `Unloading → Menu → Loading` — and only while a restart
    // is actually in flight. A `Failed` cap means a reload parked on a
    // load error (`load_session_world` runs on every `begin`, and
    // `Failed` only leaves through a queued intent nothing issued), a
    // `Menu` cap with nothing queued means the session parked (a
    // rejected re-begin, a consumed quit), and a live phase with no
    // player is a world with no driver — defects, not lifecycle
    // windows (AC02's mirror).
    if player.is_none() {
        let control = world_ecs.resource::<session::SessionControl>();
        if !absent_player_is_transient(session.phase(), control.quit || control.restart, restarts) {
            let why = match session.phase() {
                SessionPhase::Failed(m) => format!(" session failed: {m}"),
                _ => " no player vehicle".to_string(),
            };
            return record(SmokeStatus::Fail, detail(&why));
        }
    }
    // No player entity at the cap means the session was mid-teardown —
    // there is no pose to fault. With an entity present the check is
    // unchanged: missing or non-finite components fail.
    let finite = player.is_none()
        || (pos.is_some_and(|p| p.is_finite())
            && vel.is_some_and(|v| v.is_finite())
            && rot.is_some_and(|r| r.is_finite()));
    if !finite {
        return record(SmokeStatus::Fail, detail(" non-finite pose"));
    }
    if !saw_grounded {
        return record(SmokeStatus::Fail, detail(" never grounded"));
    }
    // "Below the world" is the loaded world's authored floor — the
    // PSDL bounding-box minimum — minus the margin, when the session
    // carries a `WorldFloor`. A spawn-relative line false-fails on
    // sessions that legitimately descend: retail `sf/circuit0`'s route
    // bottoms ~28 m under its start grid, and London's subway reaches
    // −22 under a street-level spawn. Sessions with no world bound
    // (the flat dev world) keep the spawn-relative line.
    let below_world = world_ecs
        .get_resource::<city::WorldFloor>()
        .map_or(spawn_pos.y - 25.0, |floor| floor.0 - 25.0);
    if let Some(p) = pos
        && p.y < below_world
    {
        return record(SmokeStatus::Fail, detail(" fell through the world"));
    }
    // The dev world is flat and empty ahead of spawn — a healthy car must
    // be able to drive *when its driver requests motion*. `Parked` never
    // does (it is the stationary control leg), so the check is inert for
    // it: the parked evidence is `moved=`/`peak=` staying at zero. A city
    // can legitimately wall the car in, so its bar is load + finite +
    // grounded only.
    if driver != Driver::Parked
        && matches!(&config.world, WorldMode::DevWorld)
        && !matches!(&config.mode, mm2_game::SessionMode::Event(_))
        && peak_speed < 5.0
    {
        return record(SmokeStatus::Fail, detail(" car never drove"));
    }
    record(SmokeStatus::Pass, detail(""))
}

/// Whether a missing player entity at the frame cap is a legitimate
/// teardown/rebuild window rather than a defect. `Unloading` always is
/// — the session is mid-despawn by definition. `Menu` counts only
/// while a quit/restart intent is still queued: with nothing pending
/// the session is parked there (a re-begin was rejected or already
/// consumed), not passing through. `Loading` counts only once a
/// re-begin bumped the generation — the first load never reaches the
/// frame loop. Everything else is a defect: `Failed` parks until an
/// intent arrives, `Ready` cannot outlive the update that enters it,
/// and the live phases are worlds missing their driver.
fn absent_player_is_transient(phase: &SessionPhase, teardown_queued: bool, restarts: u64) -> bool {
    match phase {
        SessionPhase::Unloading => true,
        SessionPhase::Menu => teardown_queued,
        SessionPhase::Loading => restarts > 0,
        _ => false,
    }
}

/// One opponent's row in the `opps=` field (F15 req 6): which roster
/// slot and vehicle, how much of the course it earned, and what the
/// bounded recovery machinery did for it.
struct OppRow<'a> {
    /// Authored roster slot — a vehicle that failed to load keeps its
    /// slot skipped, so the printed index still names the lineup entry.
    index: usize,
    /// Authored vehicle id.
    vehicle: &'a str,
    /// Gates cleared (`RaceProgress::cleared_count` — per-lap under
    /// `Ordered`, total under `AnyOrder`).
    cleared: usize,
    /// Lap in progress under `Ordered` (`lap + 1` clamped, the same
    /// convention as the record's `lap=` field); `None` on AnyOrder.
    lap: Option<u32>,
    /// `F` finished / `T` timed out; `None` while still racing.
    resolved: Option<char>,
    /// Three-point reverse-and-turn escapes attempted (`ScriptedBot`).
    escapes: u32,
    /// Bounded re-anchor teleports (DSN-14).
    reanchors: u32,
    /// Longest continuous spell inside one displacement bubble, in
    /// update frames (`OpponentDriver::stuck_peak`) — the stuck
    /// duration, surviving window resets and re-anchors.
    stuck: u32,
}

/// The `opps=` record field — one row per spawned opponent in authored
/// roster order: `<slot>:<vehicle>/<cleared>c[/<lap>l][/F|/T]`
/// followed by the recovery counters when nonzero (`<escapes>e`,
/// `<reanchors>r`, `<stuck>w` — `w` counts update frames inside the
/// stuck bubble). Recovery subfields only print on activity so a
/// clean run stays short; rows sort by roster slot so entity-archetype
/// order cannot scramble the report.
fn opponent_detail(rows: &mut [OppRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    rows.sort_by_key(|r| r.index);
    let mut s = String::from(" opps=");
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!("{}:{}/{}c", r.index, r.vehicle, r.cleared));
        if let Some(l) = r.lap {
            s.push_str(&format!("/{l}l"));
        }
        if let Some(res) = r.resolved {
            s.push_str(&format!("/{res}"));
        }
        if r.escapes > 0 {
            s.push_str(&format!("/{}e", r.escapes));
        }
        if r.reanchors > 0 {
            s.push_str(&format!("/{}r", r.reanchors));
        }
        if r.stuck > 0 {
            s.push_str(&format!("/{}w", r.stuck));
        }
    }
    s
}

/// The record's `outcome=`/`place=` field: the local participant's
/// result and its place in the ledger's standings for the *current*
/// session generation (F13-B). The ledger outlives one session —
/// results carry their generation — so both the result lookup and the
/// place scope to `generation`: an in-process restart's stale results
/// must not re-rank the live record. A participant with several
/// results in the generation (a retried event) reports the
/// best-ranked one, matching `place_of_in`.
///
/// The field names the *local* participant's result or nothing: a
/// participant still racing must not borrow the field leader's
/// outcome — `outcome=finished place=1` beside a mid-race `cp=` reads
/// as a win the record never observed. Only when the car is not a
/// participant at all does the field fall back to the generation's
/// leading result.
fn result_outcome(
    ledger: &mm2_game::ResultLedger,
    generation: u64,
    local: Option<mm2_game::PlayerId>,
) -> String {
    let standings = ledger.standings_in(generation);
    let result = match local {
        Some(id) => standings.iter().copied().find(|s| s.id.participant == id),
        None => standings.first().copied(),
    };
    result
        .map(|s| match ledger.place_of_in(generation, s.id.participant) {
            Some(place) => format!(" outcome={} place={}", s.outcome.name(), place),
            None => format!(" outcome={}", s.outcome.name()),
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::{PlayerId, ResultId, SessionOutcome, SessionResult};

    fn result(generation: u64, participant: u16, sequence: u32, race_ticks: u64) -> SessionResult {
        SessionResult {
            id: ResultId {
                generation,
                participant: PlayerId(participant),
                event: None,
                sequence,
            },
            tick: 0,
            outcome: SessionOutcome::Finished { race_ticks },
        }
    }

    /// The ledger is `init_resource`'d once and never cleared, and a
    /// restart reuses the same `PlayerId` — the record must rank the
    /// *current* generation only. Regression: the unscoped lookup let
    /// a slower refinish keep the prior generation's better place.
    #[test]
    fn outcome_scopes_to_the_session_generation() {
        let local = PlayerId(0);
        let mut ledger = mm2_game::ResultLedger::default();
        // Generation 1: an opponent beat the local driver.
        ledger.record(result(1, 1, 0, 50)).unwrap();
        ledger.record(result(1, 0, 0, 100)).unwrap();
        // Generation 2 (restarted session): the local driver finished
        // alone — slower than either generation-1 result.
        ledger.record(result(2, 0, 0, 150)).unwrap();

        assert_eq!(
            ledger.place_of(local),
            Some(2),
            "unscoped standings still see the stale win"
        );
        assert_eq!(
            result_outcome(&ledger, 2, Some(local)),
            " outcome=finished place=1",
            "the live record ranks only generation 2"
        );
        // A generation with no results records no outcome at all —
        // the fallback must not reach back into a finished session.
        assert_eq!(result_outcome(&ledger, 3, Some(local)), "");
    }

    /// A *racing* local participant must not borrow the leader's
    /// result: `outcome=finished` beside a mid-race `cp=` reads as a
    /// win nobody recorded (observed on retail `sf checkpoint:0` —
    /// `cp=2/6 pos=7/7 phase=playing outcome=finished place=1` while
    /// four opponents had resolved and the player had not). Only a
    /// non-participant car gets the leader fallback.
    #[test]
    fn a_racing_participant_reports_no_outcome() {
        let local = PlayerId(0);
        let mut ledger = mm2_game::ResultLedger::default();
        ledger.record(result(1, 1, 0, 50)).unwrap();
        ledger.record(result(1, 2, 0, 60)).unwrap();

        assert_eq!(
            result_outcome(&ledger, 1, Some(local)),
            "",
            "the local participant has no result — the leader's must not print"
        );
        // No local participant at all: the leading result still names
        // the field (the documented non-participant fallback).
        assert_eq!(
            result_outcome(&ledger, 1, None),
            " outcome=finished place=1"
        );
    }

    /// `opps=` (F15-B.7): one row per spawned opponent in authored
    /// roster order — progress, the resolved marker, and the recovery
    /// counters only when they fired. This is the field that replaced
    /// out-of-tree instrumentation for "which cars cleared their
    /// gates" (F15 req 6).
    #[test]
    fn opponent_detail_reports_progress_and_recovery_per_slot() {
        let mut rows = vec![
            // Deliberately out of order: entity iteration order is
            // archetype order; the record must sort by roster slot.
            OppRow {
                index: 2,
                vehicle: "vpanoz",
                cleared: 0,
                lap: Some(1),
                resolved: None,
                escapes: 0,
                reanchors: 2,
                stuck: 1040,
            },
            OppRow {
                index: 0,
                vehicle: "vpcoop",
                cleared: 4,
                lap: Some(2),
                resolved: None,
                escapes: 1,
                reanchors: 0,
                stuck: 0,
            },
            OppRow {
                index: 1,
                vehicle: "vpbug",
                cleared: 9,
                lap: Some(3),
                resolved: Some('F'),
                escapes: 0,
                reanchors: 0,
                stuck: 24,
            },
        ];
        assert_eq!(
            opponent_detail(&mut rows),
            " opps=0:vpcoop/4c/2l/1e,1:vpbug/9c/3l/F/24w,2:vpanoz/0c/1l/2r/1040w"
        );
        // An AnyOrder row carries no lap; a timeout marks `T`.
        let mut timed_out = vec![OppRow {
            index: 0,
            vehicle: "vpbug",
            cleared: 3,
            lap: None,
            resolved: Some('T'),
            escapes: 0,
            reanchors: 0,
            stuck: 0,
        }];
        assert_eq!(opponent_detail(&mut timed_out), " opps=0:vpbug/3c/T");
        assert_eq!(opponent_detail(&mut []), "");
    }

    /// Regression for the review-flagged false pass: an absent player
    /// at the frame cap is legitimate only inside the transient
    /// teardown/rebuild window. Previously every phase outside the
    /// live list passed, so a mid-run restart whose reload failed
    /// (`load_session_world` → `Failed`, which only leaves through a
    /// queued intent nothing issues on a dev/disabled restart)
    /// reported `status=pass … phase=failed moved=none final=none`.
    /// A `Menu` cap with no teardown intent queued is likewise parked
    /// (a rejected re-begin leaves it there), not a lifecycle window.
    #[test]
    fn absent_player_is_transient_only_in_the_teardown_window() {
        use mm2_game::SessionPhase::*;
        // Inside the window: Unloading always; Menu only while a
        // quit/restart intent is still queued; Loading only once the
        // re-begin bumped the generation.
        assert!(absent_player_is_transient(&Unloading, false, 0));
        assert!(absent_player_is_transient(&Menu, true, 0));
        assert!(absent_player_is_transient(&Loading, false, 1));
        // Parked, not transient: a Menu cap with nothing queued, a
        // first-load Loading (restarts still 0), a Ready that somehow
        // outlived its update.
        assert!(!absent_player_is_transient(&Menu, false, 0));
        assert!(!absent_player_is_transient(&Menu, false, 1));
        assert!(!absent_player_is_transient(&Loading, false, 0));
        assert!(!absent_player_is_transient(&Ready, true, 0));
        // The flagged defect: a Failed cap fails — even with a
        // teardown intent queued, the session parked on a load error.
        assert!(!absent_player_is_transient(
            &Failed("load".into()),
            false,
            1
        ));
        assert!(!absent_player_is_transient(&Failed("load".into()), true, 1));
        // The live phases were already covered by the old check.
        for phase in [Countdown, Playing, Paused, Results] {
            assert!(!absent_player_is_transient(&phase, true, 1));
        }
    }

    /// A reused `--screenshot` path must not pass the capture wait on
    /// the previous run's file: the runner clears the target before
    /// requesting the new capture so the `landed` check can only see
    /// this run's write. Regression: a pre-existing file reported
    /// `status=pass bytes=<stale>` and exited while the fresh
    /// screenshot was still in flight.
    #[test]
    fn a_stale_capture_is_cleared_before_the_new_request() {
        let dir = std::env::temp_dir().join(format!("mm2-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("capture.png");
        std::fs::write(&target, b"stale pixels").unwrap();
        clear_stale_screenshot(&target).unwrap();
        assert!(
            !target.exists(),
            "the stale file survived — the wait would pass on it"
        );
        // A fresh target is the expected case — never an error.
        clear_stale_screenshot(&target).unwrap();
        // A target in a missing directory is likewise fresh.
        clear_stale_screenshot(&dir.join("absent/capture.png")).unwrap();
        // An uncleanable target (a directory at the path) fails rather
        // than risking a stale pass.
        assert!(clear_stale_screenshot(&dir).is_err());
        std::fs::remove_dir(&dir).unwrap();
    }
}
