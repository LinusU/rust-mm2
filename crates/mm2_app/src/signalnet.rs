//! Traffic-signal phase over the wire (F26-A.4b, protocol v27).
//!
//! A signal head's aspect is a pure function of the junction
//! controller's clock, and only the authority runs that controller, so
//! a client has no phase of its own to show. The authority publishes
//! the phase as [`Message::Signals`] — one row per light-controlled
//! junction naming the member road currently green (or the all-red
//! clearance) — and a client applies it to the heads it spawned from
//! the same `.bai` both peers share. The head geometry and the
//! authored rule are deterministic scene data; only the phase the
//! authority decides crosses the wire.
//!
//! The frame is *state*: it is sent when any junction's phase changes
//! and about once a second of world time as healing for a lost frame,
//! so a late joiner lands on the host's phase within a frame's gap and
//! a dropped frame self-corrects on the next. A client never decides a
//! phase itself.
//!
//! What the original does with a networked signal is not recovered
//! (UNK-12): the aspect mapping is a designed presentation of the
//! authored rule, and carrying the authority's phase is a design
//! choice, not an original rule. See `docs/original-rules.md`.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_formats::bai::VehicleRule;
use mm2_game::{
    Session, SessionAuthority, SessionConfig, SessionEntity, SessionPhase, SignalAspect, WorldMode,
};
use mm2_net::{MAX_SNAP_SIGNALS, Message, SIGNAL_NO_GREEN, SnapSignal};

use crate::net::HostLink;
use crate::netdrive::RemoteSnaps;
use crate::traffic::{AmbientTraffic, SignalAssets, TrafficSignal, fields_ambient_traffic};
use crate::worldclock::{PUBLISH_EVERY_TICKS, WorldClock};

/// Generations the client-side inbox tracks at once, as the world
/// clock's and the leaves' do.
const STAGED_GENERATIONS: usize = 4;

#[derive(Debug, Clone)]
struct StagedSignals {
    generation: u64,
    tick: u64,
    rows: Vec<SnapSignal>,
    fresh: bool,
}

/// The client-side signals inbox, a field of [`RemoteSnaps`] so the
/// stream's authority boundaries reset it with everything else. Holds
/// the newest frame per generation: a frame is a complete snapshot of
/// the phase, so an older or equal one adds nothing and is dropped
/// counted.
#[derive(Default)]
pub struct SignalStage {
    frames: Vec<StagedSignals>,
    stale: u64,
    refused: u64,
    landed: u64,
    heads_applied: u64,
    heads_unresolved: u64,
}

impl SignalStage {
    /// Queue one received frame.
    pub fn push(&mut self, generation: u64, tick: u64, rows: Vec<SnapSignal>) {
        if let Some(entry) = self.frames.iter_mut().find(|f| f.generation == generation) {
            if tick <= entry.tick {
                self.stale += 1;
            } else {
                *entry = StagedSignals {
                    generation,
                    tick,
                    rows,
                    fresh: true,
                };
            }
            return;
        }
        if self.frames.len() >= STAGED_GENERATIONS
            && let Some(oldest) = (0..self.frames.len()).min_by_key(|&i| self.frames[i].generation)
        {
            self.frames.swap_remove(oldest);
        }
        self.frames.push(StagedSignals {
            generation,
            tick,
            rows,
            fresh: true,
        });
    }

    /// Drop everything staged — the authority's stream ended. Counters
    /// are evidence and survive.
    pub fn reset(&mut self) {
        self.frames.clear();
    }

    /// The rows waiting for `wire`'s generation; fresh frames of any
    /// other generation are dropped refused.
    fn take_for(&mut self, wire: u64) -> Option<Vec<SnapSignal>> {
        let mut found = None;
        for entry in &mut self.frames {
            if !entry.fresh {
                continue;
            }
            entry.fresh = false;
            if entry.generation == wire {
                found = Some(entry.rows.clone());
            } else {
                self.refused += 1;
            }
        }
        found
    }

    /// Frames dropped as no newer than one already seen.
    pub fn stale(&self) -> u64 {
        self.stale
    }

    /// Frames of another generation refused at apply time.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Frames folded into the heads.
    pub fn landed(&self) -> u64 {
        self.landed
    }

    /// Light heads whose aspect a frame set.
    pub fn heads_applied(&self) -> u64 {
        self.heads_applied
    }

    /// Light heads a frame left unnamed (a head at a junction the
    /// authority did not report).
    pub fn heads_unresolved(&self) -> u64 {
        self.heads_unresolved
    }
}

/// What [`publish_signals`] last sent: the generation, the world tick
/// and the phase rows (a change in any is what makes a frame due).
type Sent = (u64, u64, Vec<SnapSignal>);

/// Host: tell the clients which member road is green at every
/// light-controlled junction. Sends when the phase changes and about
/// every [`PUBLISH_EVERY_TICKS`] of world time, only for a session
/// whose world renders signal heads.
pub fn publish_signals(
    host: Res<HostLink>,
    session: Res<Session>,
    clock: Option<Res<WorldClock>>,
    traffic: Option<Res<AmbientTraffic>>,
    signals: Query<&TrafficSignal>,
    mut last: Local<Option<Sent>>,
) {
    let (Some(clock), Some(traffic)) = (clock, traffic) else {
        return;
    };
    if !session.authority_role().is_authority()
        || !matches!(
            session.phase(),
            SessionPhase::Countdown | SessionPhase::Playing | SessionPhase::Results
        )
    {
        return;
    }
    // Only a `TrafficLight` head's aspect depends on the phase, so a
    // junction with no such head never needs a row.
    let junctions: BTreeSet<u16> = signals
        .iter()
        .filter(|s| s.rule == Some(VehicleRule::TrafficLight))
        .map(|s| s.junction)
        .collect();
    if junctions.is_empty() {
        return;
    }
    let rows: Vec<SnapSignal> = junctions
        .iter()
        .take(MAX_SNAP_SIGNALS as usize)
        .map(|&ix| SnapSignal {
            junction: ix,
            green: traffic
                .junctions
                .green_road(traffic.graph(), ix)
                .unwrap_or(SIGNAL_NO_GREEN),
        })
        .collect();
    let generation = session.wire_generation();
    let due = match &*last {
        Some((g, t, p)) if *g == generation => {
            *p != rows || clock.ticks < *t || clock.ticks - *t >= PUBLISH_EVERY_TICKS
        }
        _ => true,
    };
    if !due {
        return;
    }
    let frame = Message::Signals {
        generation,
        tick: clock.ticks,
        rows: rows.clone(),
    };
    if host.ctl().broadcast(&frame).is_ok() {
        *last = Some((generation, clock.ticks, rows));
    }
}

/// Client: land the newest staged signals frame on the light heads. An
/// authority never applies, a `Loading` or paused session holds the
/// frame, and a session that is gone drops it.
pub fn apply_signals(
    mut snaps: ResMut<RemoteSnaps>,
    session: Res<Session>,
    replica: Option<Res<SignalReplica>>,
    mut signals: Query<(&mut TrafficSignal, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    if session.authority_role().is_authority() {
        return;
    }
    match session.phase() {
        SessionPhase::Loading | SessionPhase::Paused => return,
        SessionPhase::Ready
        | SessionPhase::Countdown
        | SessionPhase::Playing
        | SessionPhase::Results => {}
        _ => {
            snaps.signals.reset();
            return;
        }
    }
    let Some(replica) = replica else { return };
    let stage = &mut snaps.signals;
    let Some(rows) = stage.take_for(session.wire_generation()) else {
        return;
    };
    stage.landed += 1;
    let phases: HashMap<u16, u16> = rows.iter().map(|r| (r.junction, r.green)).collect();
    for (mut sig, mut material) in &mut signals {
        if sig.rule != Some(VehicleRule::TrafficLight) {
            continue;
        }
        let Some(&green) = phases.get(&sig.junction) else {
            stage.heads_unresolved += 1;
            continue;
        };
        let green = (green != SIGNAL_NO_GREEN).then_some(green);
        let aspect = SignalAspect::of_phase(sig.member, green, sig.road, sig.rule);
        if aspect != sig.aspect {
            sig.aspect = aspect;
            material.0 = replica.assets.material(aspect).clone();
        }
        stage.heads_applied += 1;
    }
}

/// The client's signal world: the shared lamp mesh and per-aspect
/// materials its heads draw with. Inserted by the session load on a
/// `Remote` city session; removed with the session.
#[derive(Resource)]
pub struct SignalReplica {
    assets: SignalAssets,
}

/// Build the client's signal world for `config`: nothing unless the
/// session is a `Remote` city session that fields traffic. Spawns the
/// same authored heads the authority draws, from the same `.bai`.
pub fn load_signal_replica(
    commands: &mut Commands,
    vfs: &Vfs,
    config: &SessionConfig,
    owner: SessionEntity,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    if config.authority != SessionAuthority::Remote || !fields_ambient_traffic(config) {
        return;
    }
    let WorldMode::City { psdl } = &config.world else {
        return;
    };
    let stem = std::path::Path::new(psdl)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(psdl.as_str());
    if let Some(assets) =
        crate::traffic::spawn_client_signals(commands, vfs, stem, owner, meshes, materials)
    {
        commands.insert_resource(SignalReplica { assets });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::{SessionAuthority, SessionConfig};

    fn row(junction: u16, green: u16) -> SnapSignal {
        SnapSignal { junction, green }
    }

    #[test]
    fn the_stage_keeps_the_newest_frame_per_generation() {
        let mut stage = SignalStage::default();
        stage.push(1, 10, vec![row(0, 1)]);
        stage.push(1, 10, vec![row(1, 1)]);
        stage.push(1, 5, vec![row(2, 1)]);
        assert_eq!(stage.stale(), 2);
        stage.push(1, 20, vec![row(3, SIGNAL_NO_GREEN)]);
        let rows = stage.take_for(1).unwrap();
        assert_eq!(rows[0].junction, 3);
        assert!(stage.take_for(1).is_none(), "a frame lands once");
    }

    #[test]
    fn another_generation_is_refused_not_applied() {
        let mut stage = SignalStage::default();
        stage.push(2, 10, vec![row(0, 1)]);
        assert!(stage.take_for(1).is_none());
        assert_eq!(stage.refused(), 1);
    }

    #[test]
    fn the_stage_tracks_a_bounded_set_of_generations() {
        let mut stage = SignalStage::default();
        for g in 0..10 {
            stage.push(g, 1, Vec::new());
        }
        assert_eq!(stage.frames.len(), STAGED_GENERATIONS);
    }

    fn client(generation: u64) -> App {
        let mut app = App::new();
        let mut session = Session::default();
        session
            .begin_generation(
                SessionConfig {
                    authority: SessionAuthority::Remote,
                    ..SessionConfig::default()
                },
                generation,
            )
            .unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Countdown).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let assets = SignalAssets::new(&mut meshes, &mut materials);
        app.insert_resource(session)
            .init_resource::<RemoteSnaps>()
            .insert_resource(SignalReplica { assets })
            .add_systems(Update, apply_signals);
        app
    }

    fn head(junction: u16, road: u16, member: bool) -> TrafficSignal {
        TrafficSignal {
            junction,
            road,
            rule: Some(VehicleRule::TrafficLight),
            member,
            aspect: SignalAspect::Red,
        }
    }

    /// The lamp component the apply query matches on — the client only
    /// swaps the material handle it already holds.
    fn lamp() -> MeshMaterial3d<StandardMaterial> {
        MeshMaterial3d(Handle::default())
    }

    #[test]
    fn a_client_takes_the_hosts_phase_for_its_light_heads() {
        let mut app = client(3);
        let a = app.world_mut().spawn((head(0, 7, true), lamp())).id();
        let b = app.world_mut().spawn((head(0, 8, true), lamp())).id();
        // A head whose road is not a member ignores the phase (the
        // green fallback), and a non-light head is not a phase consumer.
        let c = app.world_mut().spawn((head(1, 9, false), lamp())).id();
        let d = app
            .world_mut()
            .spawn((
                TrafficSignal {
                    junction: 1,
                    road: 10,
                    rule: Some(VehicleRule::NeverStop),
                    member: false,
                    // As the client spawns it: a non-light head starts at
                    // its phase-independent aspect.
                    aspect: SignalAspect::Green,
                },
                lamp(),
            ))
            .id();
        app.world_mut().resource_mut::<RemoteSnaps>().push_signals(
            3,
            100,
            vec![row(0, 7), row(1, SIGNAL_NO_GREEN)],
        );
        app.update();
        let aspect = |app: &mut App, e: Entity| app.world().get::<TrafficSignal>(e).unwrap().aspect;
        assert_eq!(aspect(&mut app, a), SignalAspect::Green);
        assert_eq!(aspect(&mut app, b), SignalAspect::Red);
        // A non-member light head is always green, whatever the phase.
        assert_eq!(aspect(&mut app, c), SignalAspect::Green);
        // A never-stop head keeps its constant green, untouched.
        assert_eq!(aspect(&mut app, d), SignalAspect::Green);
        let stage = app.world().resource::<RemoteSnaps>().signals();
        assert_eq!(stage.landed(), 1);
        assert_eq!(stage.heads_applied(), 3, "the three light heads");
        assert_eq!(stage.heads_unresolved(), 0);
    }

    #[test]
    fn a_light_head_at_an_unreported_junction_is_left_alone_and_counted() {
        let mut app = client(3);
        let a = app.world_mut().spawn((head(5, 1, true), lamp())).id();
        app.world_mut()
            .resource_mut::<RemoteSnaps>()
            .push_signals(3, 100, vec![row(0, 1)]);
        app.update();
        assert_eq!(
            app.world().get::<TrafficSignal>(a).unwrap().aspect,
            SignalAspect::Red,
            "no row for junction 5 leaves the head as it stood"
        );
        let stage = app.world().resource::<RemoteSnaps>().signals();
        assert_eq!((stage.heads_applied(), stage.heads_unresolved()), (0, 1));
    }

    #[test]
    fn a_frame_of_another_generation_changes_nothing() {
        let mut app = client(3);
        let a = app.world_mut().spawn((head(0, 1, true), lamp())).id();
        app.world_mut()
            .resource_mut::<RemoteSnaps>()
            .push_signals(4, 100, vec![row(0, 1)]);
        app.update();
        assert_eq!(
            app.world().get::<TrafficSignal>(a).unwrap().aspect,
            SignalAspect::Red
        );
        assert_eq!(app.world().resource::<RemoteSnaps>().signals().refused(), 1);
    }
}
