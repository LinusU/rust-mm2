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
use mm2_game::{
    Banger, BangerPhase, BangerStateChanged, ImpactEvent, Mm2Vfs, ParticipantState, PlayerVehicle,
    RaceProgress, RaceStarted, RaceState, Session, SessionConfig, SessionPhase, WorldMode,
    advance_session_tick, despawn_session_entities,
};
use mm2_vehicle::vehicle::{VehicleInput, VehicleState};
use mm2_vehicle::{VehicleConfig, VehiclePlugin};

use crate::{camera, contracts, opponents, race, scripted, session};

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
        .insert_resource(car)
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
                // F10-A.2: ambient lane-following + recycle/respawn —
                // the headless record's `traf=` field reads the state
                // these leave behind.
                crate::traffic::drive_ambient,
                crate::traffic::maintain_ambient,
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
                // `--finish` works headless too — the record still
                // reports the real resolved outcome.
                crate::results::dev_finish_once,
                opponents::opponent_drive,
                // F16-B: the same result → profile consumption the
                // windowed app runs — a bound profile in a headless
                // evidence run must record identically.
                crate::progression::record_session_results,
            ),
        );
    if driver == Driver::Scripted {
        app.insert_resource(scripted::ScriptedDrive);
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
    // counts in both). `bng_reclaims` separately counts settles whose
    // cause is the pool reclaiming a slot — otherwise a pool-settle and
    // an Avian-sleep settle are indistinguishable in the record.
    let mut bng_events = [0usize; 4];
    let mut bng_reclaims = 0usize;
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
            if e.cause == mm2_game::BangerCause::Reclaimed {
                bng_reclaims += 1;
            }
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
            let progress = world_ecs.get::<RaceProgress>(car);
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
            let local = world_ecs.get::<mm2_game::Player>(car).map(|p| p.id);
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
            // appears when one fired.
            let (opp, opp_done, opp_rec) =
                world_ecs
                    .iter_entities()
                    .fold((0usize, 0usize, 0usize), |(n, d, r), e| {
                        let Some(driver) = e.get::<opponents::OpponentDriver>() else {
                            return (n, d, r);
                        };
                        let resolved = e.get::<RaceProgress>().is_some_and(|p| {
                            matches!(
                                p.state,
                                ParticipantState::Finished { .. }
                                    | ParticipantState::TimedOut { .. }
                            )
                        });
                        (n + 1, d + resolved as usize, r + driver.reanchors as usize)
                    });
            let opp = if opp > 0 {
                let rec = if opp_rec > 0 {
                    format!(" opp_rec={opp_rec}")
                } else {
                    String::new()
                };
                format!(" opp={opp_done}/{opp}{rec}")
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
    // `--nav` evidence: the graph + overrides loaded through the real
    // session path (`dev.nav_overlay` on the session config).
    let nav_detail = world_ecs
        .get_resource::<crate::nav_overlay::CityNav>()
        .map(|n| {
            let s = crate::nav_overlay::hud_summary(n);
            format!(" nav={}", s.strip_prefix("nav ").unwrap_or(&s))
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
            format!(
                " traf={}/{} sp={} rec={} dead={} uns={} q={} jq={} stuck={}",
                active,
                t.target,
                t.spawned,
                t.recycled,
                t.dead_ends,
                t.unspawnable,
                t.queued,
                t.junction_held,
                t.stuck
            )
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
    let detail = |extra: &str| {
        format!(
            "updates={frames} ticks={ticks} driver={} phase={} impacts={impacts} dropped={dropped} peak={peak_speed:.1}m/s moved={moved:.0}m wheels={grounded_wheels}/{total} final=({x:.0},{y:.1},{z:.0}){race_detail}{nav_detail}{traf_detail}{bng_detail}{traction_detail}{profile_detail}{extra}",
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

/// The record's `outcome=`/`place=` field: the local participant's
/// result and its place in the ledger's standings for the *current*
/// session generation (F13-B). The ledger outlives one session —
/// results carry their generation — so both the result lookup and the
/// place scope to `generation`: an in-process restart's stale results
/// must not re-rank the live record. A participant with several
/// results in the generation (a retried event) reports the
/// best-ranked one, matching `place_of_in`. When the car is not a
/// participant the field still names the generation's leading result.
fn result_outcome(
    ledger: &mm2_game::ResultLedger,
    generation: u64,
    local: Option<mm2_game::PlayerId>,
) -> String {
    let standings = ledger.standings_in(generation);
    local
        .and_then(|id| standings.iter().copied().find(|s| s.id.participant == id))
        .or_else(|| standings.first().copied())
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
}
