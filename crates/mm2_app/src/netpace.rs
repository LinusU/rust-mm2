//! The client's fixed-tick schedule under the lead controller (NETPLAY-02F,
//! `docs/netplay-timeline.md`).
//!
//! Behind the [`NetplayTimeline`] selector, a client that is sending a
//! command stream stops using Bevy's own fixed-time accumulator: each frame
//! [`netlead::Pacer::advance`] turns the *real* frame time (Bevy's virtual
//! clock clamps a long stall to 250 ms, which would hide a sleep/resume)
//! into a whole number of production-length 1/60 s ticks, and the
//! controller's rate trims how many fit. The tick length itself is the
//! unchanged `Time<Fixed>` step, and the `FixedMain` schedule runs exactly
//! as Bevy would run it.
//!
//! The controller is fed from the snapshots the client receives: the local
//! seat's `input_depth` and `input_ack` ([`ClientLead::note_snap`], called by
//! `net::drive_lobby`), and the ack also trims the `CommandHistory`.
//! A [`netlead::Rebase`] clears the controller's filter and `SeqMap`.
//!
//! With the selector off, or no command stream attached (a host, a
//! single-player session), nothing here runs and Bevy's accumulator is in
//! charge, as before.

use std::collections::VecDeque;
use std::time::Duration;

use bevy::app::{FixedMain, RunFixedMainLoop, RunFixedMainLoopSystems};
use bevy::prelude::*;
use mm2_game::Session;
use mm2_net::SnapEntry;

use crate::netcmd::CommandStream;
use crate::netlead::{LeadConfig, LeadController, Pacer};
use crate::netplay_timeline::NetplayTimeline;

/// Reports kept between two frames (30 Hz snapshots; a frame never waits
/// this long).
const MAX_REPORTS: usize = 64;

/// What one snapshot said about the local seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapReport {
    generation: u64,
    server_tick: u64,
    ack: u64,
    depth: u8,
}

/// The pacer and controller for this client, and the snapshot reports
/// waiting for the next frame.
#[derive(Resource, Debug)]
pub struct ClientLead {
    cfg: LeadConfig,
    ctl: LeadController,
    pacer: Pacer,
    reports: VecDeque<SnapReport>,
    engaged: bool,
    generation: Option<u64>,
    /// Ticks released for the frame in flight.
    due: u32,
    /// Ticks run over the controller's lifetime (this engagement).
    ran: u64,
}

impl Default for ClientLead {
    fn default() -> Self {
        Self::new(LeadConfig::default())
    }
}

impl ClientLead {
    /// A controller and pacer with `cfg`.
    pub fn new(cfg: LeadConfig) -> Self {
        Self {
            cfg,
            ctl: LeadController::new(cfg),
            pacer: Pacer::new(cfg),
            reports: VecDeque::new(),
            engaged: false,
            generation: None,
            due: 0,
            ran: 0,
        }
    }

    /// Record a snapshot's report for seat `player` (ignored when the
    /// snapshot has no row for it). Cheap and bounded, so it is safe to
    /// call whether or not the timeline is selected.
    pub fn note_snap(&mut self, player: u16, generation: u64, tick: u64, entries: &[SnapEntry]) {
        if let Some(e) = entries.iter().find(|e| e.player == player) {
            self.note_report(generation, tick, e.input_ack, e.input_depth);
        }
    }

    /// Record the local seat's `ack` and host buffer `depth` from the
    /// snapshot of `generation` at `server_tick`.
    pub fn note_report(&mut self, generation: u64, server_tick: u64, ack: u64, depth: u8) {
        if self.reports.len() == MAX_REPORTS {
            self.reports.pop_front();
        }
        self.reports.push_back(SnapReport {
            generation,
            server_tick,
            ack,
            depth,
        });
    }

    /// Whether the pacer currently owns the fixed schedule.
    pub fn engaged(&self) -> bool {
        self.engaged
    }

    /// The feedback controller.
    pub fn controller(&self) -> &LeadController {
        &self.ctl
    }

    /// The pacer.
    pub fn pacer(&self) -> &Pacer {
        &self.pacer
    }

    /// Fixed ticks run under the pacer since it last started.
    pub fn ticks_run(&self) -> u64 {
        self.ran
    }

    fn restart(&mut self) {
        self.ctl = LeadController::new(self.cfg);
        self.pacer = Pacer::new(self.cfg);
        self.reports.clear();
        self.due = 0;
        self.ran = 0;
    }
}

fn engaged(lead: Res<ClientLead>) -> bool {
    lead.engaged
}

fn not_engaged(lead: Res<ClientLead>) -> bool {
    !lead.engaged
}

/// Decide whether the pacer owns the schedule this frame: the selector is
/// on and a command stream is attached to a transport in a playing
/// session. On the engaging frame Bevy's accumulated remainder is dropped
/// so the two schedulers never both owe ticks.
fn sync_engaged(
    timeline: Option<Res<NetplayTimeline>>,
    session: Option<Res<Session>>,
    stream: Res<CommandStream>,
    mut lead: ResMut<ClientLead>,
    mut fixed: ResMut<Time<Fixed>>,
) {
    let want = timeline.is_some_and(|t| t.enabled)
        && stream.has_handoff()
        && session.is_some_and(|s| s.is_playing());
    if want == lead.engaged {
        return;
    }
    lead.engaged = want;
    lead.generation = None;
    lead.restart();
    if want {
        let over = fixed.overstep();
        fixed.discard_overstep(over);
    }
}

/// Fold the frame's reports into the controller and the history, then ask
/// the pacer how many ticks the frame's real time buys.
fn plan_ticks(
    real: Res<Time<Real>>,
    virt: Res<Time<Virtual>>,
    session: Res<Session>,
    mut stream: ResMut<CommandStream>,
    mut lead: ResMut<ClientLead>,
) {
    let lead = &mut *lead;
    let generation = session.wire_generation();
    if lead.generation != Some(generation) {
        // A new session generation restarts both the stream and its
        // controller; reports of the old one are never this one's acks.
        lead.restart();
        lead.generation = Some(generation);
    }
    let now_ns = i64::try_from(real.elapsed().as_nanos()).unwrap_or(i64::MAX);
    let dt_ns = if virt.is_paused() {
        0
    } else {
        i64::try_from(real.delta().as_nanos()).unwrap_or(i64::MAX)
    };
    while let Some(r) = lead.reports.pop_front() {
        if r.generation != generation {
            continue;
        }
        lead.ctl.observe(r.depth, r.ack, r.server_tick, now_ns);
        if stream.history().generation() == generation {
            stream.history_mut().ack(r.ack);
        }
    }
    let rate = lead.ctl.update(now_ns);
    let pace = lead.pacer.advance(dt_ns, rate);
    if pace.rebase.is_some() {
        lead.ctl.rebase();
    }
    lead.due = pace.ticks;
}

/// Run the released ticks the way Bevy's `run_fixed_main_schedule` does.
fn run_ticks(world: &mut World) {
    let due = std::mem::take(&mut world.resource_mut::<ClientLead>().due);
    if due == 0 {
        return;
    }
    let step: Duration = world.resource::<Time<Fixed>>().timestep();
    let _ = world.try_schedule_scope(FixedMain, |world, schedule| {
        for _ in 0..due {
            world.resource_mut::<Time<Fixed>>().advance_by(step);
            *world.resource_mut::<Time>() = world.resource::<Time<Fixed>>().as_generic();
            schedule.run(world);
        }
    });
    *world.resource_mut::<Time>() = world.resource::<Time<Virtual>>().as_generic();
    world.resource_mut::<ClientLead>().ran += u64::from(due);
}

/// Register the pacer behind the [`NetplayTimeline`] selector. Bevy's own
/// fixed-time loop is skipped only while the pacer is engaged.
pub fn add_systems(app: &mut App) {
    use RunFixedMainLoopSystems::{AfterFixedMainLoop, BeforeFixedMainLoop};
    app.init_resource::<ClientLead>()
        .configure_sets(
            RunFixedMainLoop,
            RunFixedMainLoopSystems::FixedMainLoop.run_if(not_engaged),
        )
        .add_systems(
            RunFixedMainLoop,
            (
                sync_engaged.in_set(BeforeFixedMainLoop),
                (plan_ticks, run_ticks)
                    .chain()
                    .run_if(engaged)
                    .after(BeforeFixedMainLoop)
                    .before(AfterFixedMainLoop),
            ),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use mm2_game::{SessionConfig, SessionPhase};

    #[derive(Resource, Default)]
    struct Ran(u32);

    fn count(mut ran: ResMut<Ran>) {
        ran.0 += 1;
    }

    fn app(selector: bool, frame: Duration) -> App {
        let mut session = Session::new();
        session
            .begin_generation(SessionConfig::default(), 1)
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let mut stream = CommandStream::default();
        stream.set_handoff(crate::netcmd::Handoff::spawn(|_| Ok(())));
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .insert_resource(TimeUpdateStrategy::ManualDuration(frame))
            .insert_resource(session)
            .insert_resource(stream)
            .insert_resource(NetplayTimeline { enabled: selector })
            .init_resource::<Ran>()
            .add_systems(FixedUpdate, count);
        add_systems(&mut app);
        app
    }

    fn run(app: &mut App, frames: u32) -> u32 {
        for _ in 0..frames {
            app.update();
        }
        app.world().resource::<Ran>().0
    }

    /// Selector off: Bevy's accumulator runs the schedule, the pacer never
    /// engages.
    #[test]
    fn selector_off_leaves_the_legacy_schedule() {
        let mut app = app(false, Duration::from_millis(50));
        let ran = run(&mut app, 21);
        assert!(!app.world().resource::<ClientLead>().engaged());
        // 20 timed frames of 50 ms = 3 ticks each (Bevy's rounding of the
        // 1/60 s step may leave one in the remainder).
        assert!((59..=60).contains(&ran), "ran {ran}");
        assert_eq!(app.world().resource::<ClientLead>().ticks_run(), 0);
    }

    /// Selector on with a stream attached: the pacer owns the schedule,
    /// releases whole ticks (at most the catch-up bound per frame) and the
    /// per-tick `Time` is the production step.
    #[test]
    fn selector_on_paces_whole_production_ticks() {
        let mut app = app(true, Duration::from_millis(50));
        let ran = run(&mut app, 21);
        let lead = app.world().resource::<ClientLead>();
        assert!(lead.engaged());
        assert_eq!(u64::from(ran), lead.ticks_run());
        // 20 timed frames of 50 ms: 3 ticks each, the pacer's 4-tick cap
        // never binds, and no tick is lost to rounding beyond one.
        assert!((59..=60).contains(&ran), "ran {ran}");
        assert_eq!(
            app.world().resource::<Time<Fixed>>().timestep(),
            Duration::from_secs_f64(1.0 / 60.0)
        );
    }

    /// Without a command stream attached (a host, a single-player
    /// session) the selector alone changes nothing.
    #[test]
    fn no_stream_no_pacer() {
        let mut app = app(true, Duration::from_millis(50));
        app.insert_resource(CommandStream::default());
        let ran = run(&mut app, 21);
        assert!(!app.world().resource::<ClientLead>().engaged());
        assert!((59..=60).contains(&ran), "ran {ran}");
    }
}
