//! World-simulation readiness gate (#1446).
//!
//! [`load_session_world`](crate::session::load_session_world) builds the
//! whole session world inside one `Update`, driving `Loading → Ready →
//! Countdown`/`Playing` in a single frame. The time that frame took — how
//! long the map took to load — is the *next* frame's `Time<Real>` delta.
//! Bevy clamps that to `Time<Virtual>`'s `max_delta` (250 ms by default)
//! and `run_fixed_main_schedule` then replays it as up to fifteen
//! catch-up physics ticks the moment the world exists: opponents and
//! traffic advance by a machine-speed-dependent amount before the race
//! even starts, so a slower load hands the race a different initial
//! state and the countdown a head start.
//!
//! [`gate_world_simulation`] removes that replay. On the first frame the
//! freshly loaded world could simulate — the session generation's first
//! `Countdown`/`Playing` frame, which is exactly where the load's wall
//! time lands — it caps `Time<Virtual>` at one fixed timestep, so the
//! fixed loop steps once however long the load took, then restores the
//! clock. A steady frame (`delta == timestep`) is unaffected: the cap only
//! ever lowers the delta. Dropping blocked time with a `max_delta` ceiling
//! is Bevy's documented mechanism for exactly this, and it is honoured
//! under `TimeUpdateStrategy::ManualDuration`, so the regression test can
//! vary a frame's duration and compare the result.

use std::time::Duration;

use bevy::prelude::*;
use mm2_game::{Session, SessionPhase};

/// Bookkeeping for [`gate_world_simulation`]: which generation's post-load
/// frame has been clamped, and the `max_delta` its one-tick ceiling
/// replaced (lifted on the following frame).
#[derive(Resource, Debug, Default)]
pub struct SimulationGate {
    /// The session generation whose post-load frame is already clamped —
    /// `0` before the first load (`Session::begin` mints from `1`, so `0`
    /// names no session).
    clamped_generation: u64,
    /// The `max_delta` the one-tick ceiling replaced, restored next frame.
    restore_max_delta: Option<Duration>,
}

impl SimulationGate {
    /// The generation whose post-load frame the gate has clamped, or
    /// `None` before the first load completes.
    pub fn clamped_generation(&self) -> Option<u64> {
        (self.clamped_generation != 0).then_some(self.clamped_generation)
    }
}

/// Drop a map load's wall time instead of replaying it as catch-up fixed
/// steps (#1446).
///
/// Runs in `First` ahead of `bevy::time::TimeSystems` so the ceiling it
/// sets shapes the very frame it is decided in: `run_fixed_main_schedule`
/// only ever reads `Time<Virtual>::delta()`.
pub fn gate_world_simulation(
    session: Option<Res<Session>>,
    mut gate: ResMut<SimulationGate>,
    virtual_time: Option<ResMut<Time<Virtual>>>,
    fixed_time: Option<Res<Time<Fixed>>>,
) {
    // No time plugins (a bare logic app): nothing to gate.
    let Some(mut virtual_time) = virtual_time else {
        return;
    };
    // A one-tick ceiling set on the load's recovery frame lasts exactly
    // that frame — lift it before this frame's delta is computed.
    if let Some(previous) = gate.restore_max_delta.take() {
        virtual_time.set_max_delta(previous);
    }
    let Some(session) = session else {
        // No phase machine (a menu/test app): the gate owns nothing.
        return;
    };
    let generation = session.generation();
    // The load's wall time lands on the first frame the world could
    // simulate, not on the `Loading` frame itself (whose fixed loop still
    // runs before `load_session_world`). Clamp once per generation, when
    // that frame arrives.
    let post_load = matches!(
        session.phase(),
        SessionPhase::Countdown | SessionPhase::Playing
    );
    if !post_load || generation == 0 || gate.clamped_generation == generation {
        return;
    }
    gate.clamped_generation = generation;
    let Some(fixed_time) = fixed_time.as_deref() else {
        return;
    };
    // Only lower the ceiling: a small timestep never widens the accepted
    // delta.
    if fixed_time.timestep() < virtual_time.max_delta() {
        gate.restore_max_delta = Some(virtual_time.max_delta());
        virtual_time.set_max_delta(fixed_time.timestep());
    }
}

#[cfg(test)]
mod tests {
    use bevy::time::{TimeSystems, TimeUpdateStrategy};

    use super::*;

    #[derive(Resource, Default)]
    struct FixedSteps(u32);

    fn count_fixed_step(mut steps: ResMut<FixedSteps>) {
        steps.0 += 1;
    }

    fn one_tick() -> Duration {
        Duration::from_secs_f64(1.0 / 60.0)
    }

    fn timing_app() -> App {
        let mut session = Session::new();
        session
            .begin(mm2_game::SessionConfig::default())
            .expect("a default session begins");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .insert_resource(TimeUpdateStrategy::ManualDuration(one_tick()))
            .insert_resource(session)
            .init_resource::<SimulationGate>()
            .init_resource::<FixedSteps>()
            .add_systems(FixedUpdate, count_fixed_step)
            .add_systems(First, gate_world_simulation.before(TimeSystems));
        app
    }

    /// #1446: the frame carrying the map load's wall time runs one fixed
    /// step, not a quarter second of catch-up; the next frame is normal.
    #[test]
    fn a_loads_wall_time_is_dropped_not_replayed() {
        let mut app = timing_app();
        app.update(); // the first update primes `Time<Real>` at a zero delta
        app.update(); // the `Loading` frame
        assert_eq!(app.world().resource::<FixedSteps>().0, 1);

        app.world_mut()
            .resource_mut::<Session>()
            .transition(SessionPhase::Ready)
            .expect("Loading → Ready");
        app.world_mut()
            .resource_mut::<Session>()
            .transition(SessionPhase::Playing)
            .expect("Ready → Playing");

        app.world_mut().resource_mut::<FixedSteps>().0 = 0;
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            400,
        )));
        app.update(); // the recovery frame — the load's wall time lands here
        assert_eq!(
            app.world().resource::<FixedSteps>().0,
            1,
            "the load's wall time was replayed as catch-up fixed steps"
        );
        assert_eq!(
            app.world()
                .resource::<SimulationGate>()
                .clamped_generation(),
            Some(1),
            "the gate clamped the first post-load generation"
        );

        app.insert_resource(TimeUpdateStrategy::ManualDuration(one_tick()));
        app.world_mut().resource_mut::<FixedSteps>().0 = 0;
        app.update(); // the next steady frame is normal again
        assert_eq!(app.world().resource::<FixedSteps>().0, 1);
    }
}
