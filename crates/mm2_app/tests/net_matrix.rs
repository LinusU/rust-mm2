//! NETPLAY-02E: the loopback impairment matrix for the fixed-tick command
//! stream and its acknowledgements.
//!
//! Each cell is the production path end to end, in one process on loopback:
//! a client app runs `netcmd::capture_and_send` under the real fixed
//! schedule and worker handoff, windows cross an `ImpairProxy` lane into the
//! real `HostLink` mailbox, the host app's `netdrive::consume_seat_commands`
//! takes one slot per seat per tick, and `Snap.input_ack` / `input_depth`
//! return through the proxy's other lane to the client's `CommandHistory`.
//! The host and clients step in lock step on a paced wall clock (one tick of
//! each per 1/60 s), so the impairment recipes — which are wall-clock — are
//! what skews the two timelines.
//!
//! Evidence level: loopback only, seeded lossy proxy. It says nothing about a
//! real Internet path. The 02E cells run one fixed tick per host step; the
//! `lead_controller_*` cells (NETPLAY-02F, below) run the client under
//! `netpace`, where the lead controller and pacer decide the ticks. Not
//! covered: prediction replay/rebase (NETPLAY-04), so no slot is "replayed"
//! in these runs and every cell's counters report that as absent rather than
//! zero-by-construction.
//!
//! Every cell checks the same invariants against what the host consumed:
//! - a tick that consumed a driver command finalized exactly the next seq and
//!   applied exactly that seq's quantized controls (no skipped, doubled or
//!   reordered slot, whatever the wire did);
//! - the host ran one slot per tick (`finalized + fallback` = ticks);
//! - no control sticks: after the script releases everything the host's seat
//!   is neutral, and a substitute never outlives `HOLD_LAST_TICKS`;
//! - buffers stay bounded: no host overflow, host occupancy under the cell's
//!   bound, client history under `HISTORY_CAP`;
//! - the cumulative ack converges to the host's final ack.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use mm2_app::net::HostLink;
use mm2_app::netcmd::{self, CommandStream, HISTORY_CAP};
use mm2_app::netdrive::{self, NetPlayer, RemotePick};
use mm2_app::netlead::LeadConfig;
use mm2_app::netpace;
use mm2_app::netplay_timeline::NetplayTimeline;
use mm2_game::{PlayerVehicle, Session, SessionConfig, SessionPhase};
use mm2_net::{
    BUFFER_CAP, HOLD_LAST_TICKS, Impair, ImpairProxy, LinkDir, Message, RemoteInputs, SeatStats,
};
use mm2_vehicle::{HumanDriver, VehicleInput};

use crate::net_app::{dev_cruise, host_app, host_link, hosted_playing, ready_peer, spin, spin_mut};
use crate::support::WAIT;

const TICK: Duration = Duration::from_micros(16_667);

/// A seat's script: the controls for tick `n` (0-based within the
/// generation, so seq `n + 1`). Throttle carries the seq identity
/// (`1..=250`, wrapping) so the host's applied controls name the slot they
/// came from; brake and steering are the press/release the burst cells watch.
type Script = fn(usize, usize) -> VehicleInput;

fn ident(seq: u64) -> u8 {
    ((seq - 1) % 250) as u8 + 1
}

/// Brake held for ticks 100..106, steering for 98..112, then everything
/// released for good (the "no stuck control" tail).
fn press_release(_seat: usize, n: usize) -> VehicleInput {
    VehicleInput {
        throttle: ident(n as u64 + 1) as f32 / 255.0,
        brake: if (100..106).contains(&n) { 1.0 } else { 0.0 },
        steering: if (98..112).contains(&n) { 0.5 } else { 0.0 },
        ..VehicleInput::default()
    }
}

/// Brake and steering change often, to expose any slot that lands on the
/// wrong tick.
fn busy(_seat: usize, n: usize) -> VehicleInput {
    VehicleInput {
        throttle: ident(n as u64 + 1) as f32 / 255.0,
        brake: if (n / 7) % 3 == 1 { 1.0 } else { 0.0 },
        steering: ((n % 9) as f32 - 4.0) / 4.0,
        ..VehicleInput::default()
    }
}

/// The controls the host must apply for `seq`, as the wire quantizes them.
fn expected(script: Script, seat: usize, seq: u64) -> VehicleInput {
    let cmd = netcmd::tick_command(
        &script(seat, seq as usize - 1),
        Some(&HumanDriver::default()),
    );
    netdrive::decode_tick_command(&cmd)
}

fn same(a: &VehicleInput, b: &VehicleInput) -> bool {
    (a.throttle, a.brake, a.steering, a.handbrake, a.forced_gear)
        == (b.throttle, b.brake, b.steering, b.handbrake, b.forced_gear)
}

#[derive(Resource)]
struct Driver {
    script: Script,
    seat: usize,
    tick: usize,
}

fn feed(mut driver: ResMut<Driver>, mut car: Query<&mut VehicleInput, With<PlayerVehicle>>) {
    let input = (driver.script)(driver.seat, driver.tick);
    for mut car_input in &mut car {
        *car_input = input;
    }
    driver.tick += 1;
}

/// What a snapshot told the client about its seat.
#[derive(Debug, Clone, Copy)]
struct AckSample {
    generation: u64,
    tick: u64,
    ack: u64,
    depth: u8,
}

struct Seat {
    name: &'static str,
    id: u16,
    proxy: ImpairProxy,
    client: App,
    entity: Entity,
    script: Script,
    acks: Arc<Mutex<Vec<AckSample>>>,
    /// The generation whose snapshots feed this seat's acks.
    want: Arc<AtomicU64>,
    /// Ticks this client skips (a frozen peer or a stalled renderer).
    frozen: bool,
    /// One-shot extra frame time for the next update, in ticks.
    catch_up: u32,
    /// What the host did each tick, in order.
    ticks: Vec<HostTick>,
    max_depth_reported: u8,
    max_history: usize,
    acks_received: u64,
    ack_regressions: u64,
    history_acked: u64,
    last_ack_seen: u64,
    /// Lead-controller cells: this client's wall clock runs this many ppm
    /// fast (negative = slow) against the host's.
    ppm: i64,
    /// Wall time that passed while the client was frozen, in ns; the next
    /// frame it runs carries it (a stall, or a sleep/resume).
    owed_ns: i64,
    /// Most fixed ticks the pacer released in one frame.
    max_frame_ticks: u64,
    prev_ran: u64,
}

/// One host tick for one seat, observed after `consume_seat_commands`.
#[derive(Debug, Clone, Copy)]
struct HostTick {
    input: VehicleInput,
    ack: u64,
    stats: SeatStats,
    applied_command: bool,
}

struct Rig {
    host: App,
    seats: Vec<Seat>,
    inputs: RemoteInputs,
    started: Instant,
    tick: u64,
    generation: u64,
    stop: Arc<AtomicBool>,
    /// The clients run under `netpace` (the lead controller) instead of
    /// one fixed step per host step.
    lead: bool,
}

/// One seat of a cell; its lanes are tuned per phase with `Rig::both`.
struct SeatSpec {
    name: &'static str,
    script: Script,
}

fn delay(ms: u64) -> Impair {
    Impair {
        delay: Duration::from_millis(ms),
        ..Impair::default()
    }
}

impl Rig {
    fn new(specs: &[SeatSpec], seed: u64) -> Self {
        Self::build(specs, seed, false)
    }

    /// A rig whose clients' fixed schedule is driven by the lead
    /// controller and pacer.
    fn paced(specs: &[SeatSpec], seed: u64) -> Self {
        Self::build(specs, seed, true)
    }

    fn build(specs: &[SeatSpec], seed: u64, lead: bool) -> Self {
        let install = tempfile::tempdir().unwrap();
        let (link, vfs, fp) = host_link(install.path(), &dev_cruise());
        let host_addr = link.addr();
        let mut host = host_app(vfs, link);
        host.insert_resource(NetplayTimeline { enabled: true });
        let stop = Arc::new(AtomicBool::new(false));

        let mut peers = Vec::new();
        for (i, spec) in specs.iter().enumerate() {
            let proxy = ImpairProxy::loopback_seeded(host_addr, seed + i as u64).unwrap();
            let peer = ready_peer(proxy.addr(), spec.name, fp);
            peers.push((proxy, peer));
        }
        spin(&mut host, |a| {
            a.world()
                .resource::<mm2_app::net::LobbyState>()
                .roster
                .iter()
                .filter(|e| e.pick.is_some())
                .count()
                >= specs.len()
        });
        let generation = hosted_playing(&mut host);
        spin_mut(&mut host, |a| {
            a.world_mut()
                .query_filtered::<Entity, With<RemotePick>>()
                .iter(a.world())
                .count()
                >= specs.len()
        });
        let inputs = host.world().resource::<HostLink>().remote_inputs();

        let mut seats = Vec::new();
        for (i, ((proxy, mut peer), spec)) in peers.into_iter().zip(specs).enumerate() {
            let id = peer.player_id();
            let ctl = peer.ctl().unwrap();
            let acks: Arc<Mutex<Vec<AckSample>>> = Arc::default();
            let sink = Arc::clone(&acks);
            let halt = Arc::clone(&stop);
            let want = Arc::new(AtomicU64::new(generation));
            let want_reader = Arc::clone(&want);
            thread::spawn(move || {
                // Blocks in `recv` until the host drops (EOF) or errors.
                while !halt.load(Ordering::Relaxed) {
                    let Ok(msg) = peer.recv() else { break };
                    // Like `apply_snapshots`: another generation's snapshot
                    // is never this session's ack.
                    if let Message::Snap {
                        generation,
                        tick,
                        entries,
                        ..
                    } = msg
                        && generation == want_reader.load(Ordering::Relaxed)
                        && let Some(e) = entries.iter().find(|e| e.player == id)
                    {
                        sink.lock().unwrap().push(AckSample {
                            generation,
                            tick,
                            ack: e.input_ack,
                            depth: e.input_depth,
                        });
                    }
                }
            });
            let client = client_app(generation, spec.script, i, ctl, lead);
            let entity_on_host = host
                .world_mut()
                .query_filtered::<(Entity, &NetPlayer), With<RemotePick>>()
                .iter(host.world())
                .find(|(_, p)| p.0 == id)
                .map(|(e, _)| e)
                .expect("the seat's remote participant");
            seats.push(Seat {
                name: spec.name,
                id,
                proxy,
                client,
                entity: entity_on_host,
                script: spec.script,
                acks,
                want,
                frozen: false,
                catch_up: 0,
                ticks: Vec::new(),
                max_depth_reported: 0,
                max_history: 0,
                acks_received: 0,
                ack_regressions: 0,
                history_acked: 0,
                last_ack_seen: 0,
                ppm: 0,
                owed_ns: 0,
                max_frame_ticks: 0,
                prev_ran: 0,
            });
        }
        // The first fixed step tells the mailbox which generation runs.
        let mut rig = Self {
            host,
            seats,
            inputs,
            started: Instant::now(),
            tick: 0,
            generation,
            stop,
            lead,
        };
        rig.consume();
        // Keep the tempdir alive for the rig's lifetime (the host mounted it).
        std::mem::forget(install);
        rig
    }

    fn consume(&mut self) {
        self.host
            .world_mut()
            .run_system_once(netdrive::consume_seat_commands)
            .unwrap();
    }

    /// Set a lane of one seat.
    fn set(&self, seat: usize, dir: LinkDir, impair: Impair) {
        self.seats[seat].proxy.set(dir, impair);
    }

    fn both(&self, seat: usize, up_ms: u64, down_ms: u64) {
        self.set(seat, LinkDir::Up, delay(up_ms));
        self.set(seat, LinkDir::Down, delay(down_ms));
    }

    /// One tick of the world: each live client runs its fixed step(s), the
    /// host consumes one slot per seat and publishes, then the wall clock
    /// waits for the next tick instant.
    fn step(&mut self) {
        for seat in &mut self.seats {
            if self.lead {
                // One host step is one TICK of wall time. A frozen client
                // runs no frame (it still folds in the reports that
                // arrived, with no time passing) and owes the time to its
                // next frame; a running one scales its frame by its own
                // clock error. The pacer, not the harness, decides how many
                // fixed ticks that frame buys.
                let tick_ns = TICK.as_nanos() as i64;
                let dt_ns = if seat.frozen {
                    seat.owed_ns += tick_ns;
                    0
                } else {
                    let own = tick_ns + tick_ns * seat.ppm / 1_000_000;
                    own + std::mem::take(&mut seat.owed_ns)
                };
                seat.client
                    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_nanos(
                        dt_ns as u64,
                    )));
                seat.client.update();
                let ran = seat
                    .client
                    .world()
                    .resource::<netpace::ClientLead>()
                    .ticks_run();
                seat.max_frame_ticks = seat.max_frame_ticks.max(ran - seat.prev_ran);
                seat.prev_ran = ran;
            } else {
                if seat.frozen {
                    continue;
                }
                let ticks = 1 + seat.catch_up;
                seat.catch_up = 0;
                seat.client
                    .insert_resource(TimeUpdateStrategy::ManualDuration(TICK * ticks));
                seat.client.update();
            }
            // Let the worker hand the window to the socket (it is the only
            // thing allowed to block); bounded so a stuck sink fails.
            let deadline = Instant::now() + Duration::from_secs(5);
            let stream = seat.client.world().resource::<CommandStream>();
            while stream.transmitted_windows() + stream.send_errors() + stream.coalesced()
                < stream.queued_windows()
                && Instant::now() < deadline
            {
                thread::yield_now();
            }
        }
        self.consume();
        for i in 0..self.seats.len() {
            self.observe_seat_after(i);
        }
        self.host.update();
        for seat in &mut self.seats {
            let samples: Vec<AckSample> = std::mem::take(&mut *seat.acks.lock().unwrap());
            if self.lead {
                let mut lead = seat
                    .client
                    .world_mut()
                    .resource_mut::<netpace::ClientLead>();
                for s in &samples {
                    lead.note_report(s.generation, s.tick, s.ack, s.depth);
                }
            }
            let mut stream = seat.client.world_mut().resource_mut::<CommandStream>();
            for s in samples {
                seat.acks_received += 1;
                seat.max_depth_reported = seat.max_depth_reported.max(s.depth);
                if s.ack < seat.last_ack_seen {
                    seat.ack_regressions += 1;
                }
                seat.last_ack_seen = seat.last_ack_seen.max(s.ack);
                if !self.lead {
                    // With the controller active the client's own
                    // frame trims the history from the same report.
                    stream.history_mut().ack(s.ack);
                }
            }
            seat.max_history = seat.max_history.max(stream.history().len());
            let acked = stream.history().acked();
            assert!(
                acked >= seat.history_acked,
                "{}: a stale ack moved the history back {} -> {acked}",
                seat.name,
                seat.history_acked
            );
            seat.history_acked = acked;
        }
        self.tick += 1;
        let due = self.started + TICK * self.tick as u32;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
    }

    fn observe_seat_after(&mut self, i: usize) {
        let seat = &mut self.seats[i];
        let input = *self.host.world().get::<VehicleInput>(seat.entity).unwrap();
        let ack = self.inputs.command_ack(seat.id);
        let stats = self.inputs.seat_stats(seat.id).unwrap();
        // A new generation resets the seat's counters.
        let prev_finalized = seat
            .ticks
            .last()
            .map_or(0, |t| t.stats.finalized)
            .min(stats.finalized);
        seat.ticks.push(HostTick {
            input,
            ack,
            stats,
            applied_command: stats.finalized > prev_finalized,
        });
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Step until `pred` holds, failing after `max_ticks`.
    fn run_until(&mut self, what: &str, max_ticks: usize, pred: impl Fn(&Rig) -> bool) {
        let deadline = Instant::now() + WAIT;
        for _ in 0..max_ticks {
            if pred(self) {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: deadline");
            self.step();
        }
        assert!(pred(self), "{what}: not reached in {max_ticks} ticks");
    }

    fn dropped(&self, seat: usize, dir: LinkDir) -> u64 {
        self.seats[seat].proxy.stats(dir).dropped
    }

    /// Drop exactly `n` consecutive frames on a lane, ticking the world
    /// while the lane is armed.
    fn drop_burst(&mut self, seat: usize, dir: LinkDir, n: u64, keep: Impair) {
        let before = self.dropped(seat, dir);
        self.set(seat, dir, Impair { loss: 1.0, ..keep });
        let deadline = Instant::now() + WAIT;
        while self.dropped(seat, dir) < before + n {
            assert!(Instant::now() < deadline, "the lane never dropped {n}");
            self.step();
        }
        self.set(seat, dir, keep);
    }

    /// Freeze every client and tick the host until each seat's client has
    /// seen the ack the host finally reached; the end-of-cell drain.
    fn drain(&mut self) {
        for seat in &mut self.seats {
            seat.frozen = true;
        }
        for i in 0..self.seats.len() {
            self.set(i, LinkDir::Up, Impair::default());
            self.set(i, LinkDir::Down, Impair::default());
        }
        let deadline = Instant::now() + WAIT;
        loop {
            self.step();
            let done = self.seats.iter().all(|s| {
                let host_ack = self.inputs.command_ack(s.id);
                let stream = s.client.world().resource::<CommandStream>();
                let occupancy = self.inputs.seat_stats(s.id).unwrap().occupancy;
                occupancy == 0 && stream.history().acked() == host_ack && host_ack > 0
            });
            if done {
                break;
            }
            assert!(Instant::now() < deadline, "the acks never converged");
        }
    }

    /// The invariants every cell shares. `occupancy_bound` is the cell's
    /// own ceiling on the host buffer (always below `BUFFER_CAP`).
    fn check(&self, cell: &str, occupancy_bound: usize) {
        for (i, seat) in self.seats.iter().enumerate() {
            let tag = format!("{cell}/{}", seat.name);
            let stream = seat.client.world().resource::<CommandStream>();
            let last = seat.ticks.last().expect("the host ran");
            let s = last.stats;
            eprintln!(
                "{tag}: ticks={} generated={} queued={} transmitted={} coalesced={} \
                 handoff_dropped={} | host received={} dup={} old={} overflow={} \
                 finalized={} fallback={} (underflow={} gap={}) max_occ={} wrong_gen={} \
                 max_age={:?} | ack={} acks_rx={} ack_regress={} max_depth_rx={} \
                 max_history={} | up={:?} down={:?} | replayed=n/a",
                seat.ticks.len(),
                stream.generated_slots(),
                stream.queued_slots(),
                stream.transmitted_slots(),
                stream.coalesced(),
                stream.handoff_dropped(),
                s.received,
                s.duplicates,
                s.old,
                s.overflow,
                s.finalized,
                s.fallback,
                s.underflow,
                s.gap_finalized,
                s.max_occupancy,
                s.wrong_generation,
                s.max_age,
                last.ack,
                seat.acks_received,
                seat.ack_regressions,
                seat.max_depth_reported,
                seat.max_history,
                seat.proxy.stats(LinkDir::Up),
                seat.proxy.stats(LinkDir::Down),
            );
            assert_eq!(stream.send_errors(), 0, "{tag}: send errors");
            assert_eq!(s.overflow, 0, "{tag}: host overflow");
            assert_eq!(s.rate_limited, 0, "{tag}: flood bound hit");
            assert!(
                s.max_occupancy <= occupancy_bound && occupancy_bound < BUFFER_CAP,
                "{tag}: host occupancy {} over bound {occupancy_bound}",
                s.max_occupancy
            );
            assert!(
                seat.max_history < HISTORY_CAP && stream.history().evicted() == 0,
                "{tag}: client history {} (evicted {})",
                seat.max_history,
                stream.history().evicted()
            );
            // The lane can swap two snapshots, so a stale ack may reach the
            // client; `CommandHistory::ack` must ignore it (`history_acked`
            // is checked monotonic every step) and the swaps stay rare.
            assert!(
                seat.ack_regressions * 8 <= seat.acks_received,
                "{tag}: {} of {} acks arrived stale",
                seat.ack_regressions,
                seat.acks_received
            );
            assert!(
                seat.max_depth_reported as usize <= occupancy_bound,
                "{tag}: reported depth {}",
                seat.max_depth_reported
            );

            // One slot per tick once the stream started: everything the host
            // consumed was a command or a counted substitute.
            let started = seat.ticks.iter().position(|t| t.stats.received > 0);
            let started = started.expect("commands arrived");
            let consumed_ticks = (seat.ticks.len() - started) as u64;
            assert_eq!(
                s.finalized + s.fallback,
                consumed_ticks,
                "{tag}: one slot per tick"
            );
            assert_eq!(
                last.ack,
                s.finalized + s.gap_finalized,
                "{tag}: the ack names exactly the finalized slots"
            );

            // Slot-exact application: a Command tick finalized the next seq
            // and applied precisely its controls.
            let mut prev_ack = 0u64;
            let mut substitute_run = 0u32;
            for (n, t) in seat.ticks.iter().enumerate() {
                assert!(t.ack >= prev_ack, "{tag}: tick {n} ack went backwards");
                if t.applied_command {
                    assert_eq!(
                        t.ack,
                        prev_ack + 1,
                        "{tag}: tick {n} skipped or doubled a seq"
                    );
                    let want = expected(seat.script, i, t.ack);
                    assert!(
                        same(&t.input, &want),
                        "{tag}: tick {n} applied the wrong controls for seq {}: {:?} != {:?}",
                        t.ack,
                        t.input,
                        want
                    );
                    substitute_run = 0;
                } else {
                    substitute_run += 1;
                    // A substitute finalizing a gap advances the ack by one;
                    // a starved tick holds it.
                    assert!(t.ack <= prev_ack + 1, "{tag}: tick {n}");
                    if substitute_run > HOLD_LAST_TICKS + 1 && started <= n {
                        assert_eq!(
                            t.input.throttle, 0.0,
                            "{tag}: tick {n} still holding after {substitute_run} substitutes"
                        );
                    }
                }
                prev_ack = t.ack;
            }
            // The client's cumulative ack converged on the host's.
            assert_eq!(
                stream.history().acked(),
                last.ack,
                "{tag}: client ack != host ack"
            );
            // Nothing the client generated went unaccounted for: all but
            // the unsent tail was finalized.
            assert!(
                last.ack + netcmd::SEND_INTERVAL_TICKS >= stream.generated_slots(),
                "{tag}: ack {} behind generated {}",
                last.ack,
                stream.generated_slots()
            );
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn client_app(
    generation: u64,
    script: Script,
    seat: usize,
    ctl: mm2_net::ClientCtl,
    lead: bool,
) -> App {
    let mut session = Session::new();
    session
        .begin_generation(SessionConfig::default(), generation)
        .unwrap();
    session.transition(SessionPhase::Ready).unwrap();
    session.transition(SessionPhase::Playing).unwrap();
    let mut stream = CommandStream::default();
    stream.set_handoff(netcmd::Handoff::spawn(move |job| {
        ctl.send_input_window(
            job.generation,
            job.epoch,
            job.first_seq,
            job.commands.clone(),
        )
    }));
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(TICK))
        .insert_resource(session)
        .insert_resource(NetplayTimeline { enabled: true })
        .insert_resource(stream)
        .insert_resource(Driver {
            script,
            seat,
            tick: 0,
        })
        .add_systems(FixedUpdate, (feed, netcmd::capture_and_send).chain());
    if lead {
        netpace::add_systems(&mut app);
    }
    app.world_mut().spawn((
        PlayerVehicle,
        VehicleInput::default(),
        HumanDriver::default(),
    ));
    app
}

const SETTLE: usize = 30;
const RUN: usize = 240;

fn plain(name: &'static str, script: Script) -> SeatSpec {
    SeatSpec { name, script }
}

/// Two seats on one host, each behind its own lane: A at 20 ms RTT, B at
/// 150 ms. Neither's delay may leak into the other's slots.
#[test]
fn independently_impaired_seats_keep_their_own_timelines() {
    let mut rig = Rig::new(&[plain("near", busy), plain("far", busy)], 0x02E1);
    rig.run(SETTLE);
    rig.both(0, 10, 10);
    rig.both(1, 75, 75);
    rig.run(RUN);
    rig.drain();
    rig.check("two-seats", 12);
    let near = rig.seats[0].ticks.last().unwrap().stats;
    let far = rig.seats[1].ticks.last().unwrap().stats;
    assert!(
        near.fallback <= far.fallback + 2,
        "the near seat paid for the far seat's link: {near:?} vs {far:?}"
    );
}

/// 20/80 vs 80/20 ms directional splits of a 100 ms RTT.
#[test]
fn directional_splits_converge_the_same() {
    for (up, down) in [(20, 80), (80, 20)] {
        let mut rig = Rig::new(&[plain("split", busy)], 0x02E2 + up);
        rig.run(SETTLE);
        rig.both(0, up, down);
        rig.run(RUN);
        rig.drain();
        rig.check(&format!("split-{up}-{down}"), 12);
    }
}

/// RTT 50 -> 150 -> 50 ms: when the delay drops, later frames overtake
/// earlier ones in the lane (a real reorder), and the host must still apply
/// each seq once, in order.
#[test]
fn an_rtt_step_up_and_back_down_reorders_without_double_consumption() {
    let mut rig = Rig::new(&[plain("step", busy)], 0x02E3);
    rig.run(SETTLE);
    rig.both(0, 25, 25);
    rig.run(90);
    rig.both(0, 75, 75);
    rig.run(120);
    rig.both(0, 25, 25);
    rig.run(120);
    rig.drain();
    rig.check("rtt-step", 16);
    let up = rig.seats[0].proxy.stats(LinkDir::Up);
    assert!(up.delayed > 0, "the lane never delayed: {up:?}");
}

/// 3-6 consecutive dropped input packets with brake and steering pressed
/// before the burst and released inside it: the release must land, no
/// control sticks, no slot is applied twice, and the buffer settles.
#[test]
fn consecutive_input_packet_drops_never_stick_a_control() {
    for burst in 3..=6u64 {
        let mut rig = Rig::new(&[plain("burst", press_release)], 0x02E4 + burst);
        rig.both(0, 10, 10);
        // Run to just before the press, then drop `burst` windows: at one
        // window per two ticks the burst spans the press and the release.
        rig.run_until("approach the press", 400, |r| r.seats[0].ticks.len() >= 98);
        rig.drop_burst(0, LinkDir::Up, burst, delay(10));
        rig.run(RUN);
        rig.drain();
        // Redundancy covers the loss, so the late slots arrive and apply in
        // order; the backlog is at most the lost ticks plus a window.
        rig.check(&format!("drop-{burst}"), 2 * burst as usize + 8);
        let seat = &rig.seats[0];
        let last = seat.ticks.last().unwrap();
        assert_eq!(last.input.brake, 0.0, "brake stuck after the burst");
        assert_eq!(last.input.steering, 0.0, "steering stuck after the burst");
        assert!(
            seat.ticks.iter().any(|t| t.input.brake > 0.0),
            "the brake press never reached the host"
        );
        assert_eq!(
            seat.proxy.stats(LinkDir::Up).dropped,
            burst,
            "the burst dropped exactly {burst} frames"
        );
    }
}

/// Return-path ACK bursts: dropped `Snap`s only delay the cumulative ack.
/// The client's history stays bounded and converges once the lane heals.
#[test]
fn return_path_ack_bursts_only_delay_the_cumulative_ack() {
    let mut rig = Rig::new(&[plain("acks", busy)], 0x02E5);
    rig.run(SETTLE);
    rig.both(0, 10, 10);
    rig.run(60);
    let history_before = rig.seats[0].max_history;
    // Snapshots go out every tick: drop 90 of them (1.5 s of acks).
    rig.drop_burst(0, LinkDir::Down, 90, delay(10));
    rig.run(RUN);
    rig.drain();
    rig.check("ack-burst", 8);
    let seat = &rig.seats[0];
    assert!(
        seat.max_history > history_before,
        "the ack outage never showed in the history"
    );
    assert_eq!(
        seat.ticks.last().unwrap().stats.fallback,
        seat.ticks.last().unwrap().stats.underflow + seat.ticks.last().unwrap().stats.gap_finalized
    );
}

/// A stalled renderer: the client's fixed schedule catches up in one frame
/// (Bevy clamps a frame to 250 ms: 15 ticks). Every tick still generates a
/// command; the handoff queue coalesces rather than blocks or grows.
#[test]
fn a_stalled_renderer_catches_up_without_losing_a_tick() {
    let mut rig = Rig::new(&[plain("stall", busy)], 0x02E6);
    rig.both(0, 10, 10);
    rig.run(SETTLE + 60);
    rig.seats[0].frozen = true;
    rig.run(15);
    rig.seats[0].frozen = false;
    rig.seats[0].catch_up = 14;
    rig.run(RUN);
    rig.drain();
    rig.check("stall", 20);
    let seat = &rig.seats[0];
    let stream = seat.client.world().resource::<CommandStream>();
    let driver_ticks = seat.client.world().resource::<Driver>().tick as u64;
    assert_eq!(
        stream.generated_slots(),
        driver_ticks,
        "every client tick produced exactly one command"
    );
    let _ = stream.handoff_dropped();
}

/// A stale peer: no windows for far longer than the hold. The host holds
/// the last command for `HOLD_LAST_TICKS`, then coasts neutral, never moves
/// the ack, and applies the peer's commands in order when it returns.
#[test]
fn a_stale_peer_coasts_then_resumes_in_order() {
    let mut rig = Rig::new(&[plain("stale", busy)], 0x02E7);
    rig.both(0, 10, 10);
    rig.run(SETTLE + 60);
    let ack_before = rig.inputs.command_ack(rig.seats[0].id);
    rig.seats[0].frozen = true;
    let freeze_at = rig.seats[0].ticks.len();
    rig.run(120);
    {
        let seat = &rig.seats[0];
        let tail = &seat.ticks[seat.ticks.len() - 30..];
        assert!(
            tail.iter().all(|t| t.input.throttle == 0.0
                && t.input.brake == 0.0
                && t.input.steering == 0.0),
            "a stale seat must coast neutral"
        );
        // The ack held (modulo what was already in flight at the freeze).
        let ack_after = seat.ticks.last().unwrap().ack;
        assert!(
            ack_after <= ack_before + 2 + 16,
            "ack ran on while the peer was silent: {ack_before} -> {ack_after}"
        );
        assert!(seat.ticks.len() - freeze_at >= 120);
    }
    rig.seats[0].frozen = false;
    rig.run(RUN);
    rig.drain();
    rig.check("stale-peer", 8);
}

/// Session restart: a new generation resets the seat buffers on both ends;
/// windows still in flight under the old generation are dropped and
/// counted, never applied to the new session.
#[test]
fn a_session_restart_drops_the_old_generations_windows() {
    let mut rig = Rig::new(&[plain("restart", busy)], 0x02E8);
    rig.both(0, 40, 40);
    rig.run(SETTLE + 60);
    let old_generation = rig.generation;
    let generation = old_generation + 1;
    // Restart the host's session, then the client's: a window the client
    // sent just before its own restart is still in the 40 ms lane when the
    // host already runs the new generation.
    let restart = |app: &mut App| {
        let mut session = app.world_mut().resource_mut::<Session>();
        session.transition(SessionPhase::Unloading).unwrap();
        session.transition(SessionPhase::Menu).unwrap();
        session.begin_generation(dev_cruise(), generation).unwrap();
        session.transition(SessionPhase::Ready).unwrap();
        session.transition(SessionPhase::Playing).unwrap();
    };
    restart(&mut rig.host);
    rig.consume();
    // The host's new generation starts a fresh buffer; in-flight windows of
    // the old one arrive for a few ticks.
    rig.run(4);
    restart(&mut rig.seats[0].client);
    rig.seats[0]
        .client
        .world_mut()
        .resource_mut::<Driver>()
        .tick = 0;
    let inputs = rig.inputs.clone();
    let id = rig.seats[0].id;
    rig.generation = generation;
    rig.seats[0].want.store(generation, Ordering::Relaxed);
    rig.seats[0].history_acked = 0;
    rig.seats[0].last_ack_seen = 0;
    for seat in &mut rig.seats {
        seat.ticks.clear();
    }
    rig.run(RUN);
    let stats = inputs.seat_stats(id).unwrap();
    assert!(
        stats.wrong_generation > 0,
        "old-generation windows must be dropped and counted: {stats:?}"
    );
    rig.drain();
    // The seq space restarted at 1: the new session's slots are exactly
    // the new generation's commands.
    let seat = &rig.seats[0];
    let stream = seat.client.world().resource::<CommandStream>();
    assert_eq!(stream.history().generation(), generation);
    let last = seat.ticks.last().unwrap();
    eprintln!("restart: ack={} stats={:?}", last.ack, last.stats);
    let new_commands = seat.client.world().resource::<Driver>().tick as u64;
    assert!(
        last.ack + netcmd::SEND_INTERVAL_TICKS >= new_commands,
        "ack {} behind the new generation's {new_commands} commands",
        last.ack
    );
    for (n, t) in seat.ticks.iter().enumerate() {
        if t.applied_command {
            let want = expected(seat.script, 0, t.ack);
            assert!(
                same(&t.input, &want),
                "restart tick {n} applied the wrong slot"
            );
        }
    }
}

// ---------------------------------------------------------------------
// NETPLAY-02F: the same host path with the lead controller driving each
// client's fixed schedule (`netpace`). Loopback only; the host consumes one
// command per seat per tick and the controller steers the buffer depth the
// host reports back to `LeadConfig::target_depth`.

/// Ticks of host occupancy the cells allow beyond `max_depth`: the harness
/// sleeps and the client threads run in real time, and a loaded machine
/// (the whole suite runs in parallel) delivers windows in clumps.
const LOAD_SLACK: usize = 3;

fn lead_cfg() -> LeadConfig {
    LeadConfig::default()
}

fn occupancy(rig: &Rig, seat: usize) -> Vec<usize> {
    rig.seats[seat]
        .ticks
        .iter()
        .map(|t| t.stats.occupancy)
        .collect()
}

fn mean(xs: &[usize]) -> f64 {
    xs.iter().sum::<usize>() as f64 / xs.len().max(1) as f64
}

/// Index of the first host tick at or after `from` from which the buffer
/// stays within `[lo, hi]` to the end of the run.
fn settled_at(occ: &[usize], from: usize, lo: usize, hi: usize) -> Option<usize> {
    let bad = occ
        .iter()
        .rposition(|&o| o < lo || o > hi)
        .map_or(from, |i| i + 1);
    let at = bad.max(from);
    (at < occ.len()).then_some(at)
}

fn lead_of(rig: &Rig, seat: usize) -> &netpace::ClientLead {
    rig.seats[seat]
        .client
        .world()
        .resource::<netpace::ClientLead>()
}

fn report_lead(tag: &str, rig: &Rig, seat: usize) {
    let lead = lead_of(rig, seat);
    let occ = occupancy(rig, seat);
    eprintln!(
        "{tag}: lead ticks_run={} host_ticks={} pacer(stall={} sleep={} dropped={}) \
         ctl={:?} rate={}ppm depth={:.2} max_frame_ticks={} occ_max={} occ_tail_mean={:.2}",
        lead.ticks_run(),
        occ.len(),
        lead.pacer().stall_rebases(),
        lead.pacer().sleep_rebases(),
        lead.pacer().dropped_ticks(),
        lead.controller().stats(),
        lead.controller().rate_ppm(),
        lead.controller().depth_ticks(),
        rig.seats[seat].max_frame_ticks,
        occ.iter().max().unwrap_or(&0),
        mean(&occ[occ.len().saturating_sub(200)..]),
    );
}

/// The controller engages on a client that is sending, and a steady 20 ms
/// RTT settles the host buffer at the target and holds it there.
#[test]
fn lead_controller_settles_the_host_buffer_at_the_target() {
    let cfg = lead_cfg();
    let mut rig = Rig::paced(&[plain("lead", busy)], 0x02F1);
    rig.both(0, 10, 10);
    rig.run(900);
    assert!(lead_of(&rig, 0).engaged(), "the pacer never took over");
    rig.drain();
    rig.check("lead-steady", 12);
    report_lead("lead-steady", &rig, 0);
    let occ = occupancy(&rig, 0);
    let started = rig.seats[0]
        .ticks
        .iter()
        .position(|t| t.stats.received > 0)
        .unwrap();
    let at = settled_at(&occ[..900], started, 0, cfg.max_depth as usize + LOAD_SLACK)
        .expect("the buffer never settled");
    assert!(
        at - started <= 300,
        "settled only after {} ticks",
        at - started
    );
    let tail = &occ[600..900];
    assert!(
        (cfg.target_depth as f64 - 1.0..=cfg.target_depth as f64 + 2.0).contains(&mean(tail)),
        "steady depth {} is not near the target",
        mean(tail)
    );
    assert!(*tail.iter().max().unwrap() <= cfg.max_depth as usize + LOAD_SLACK);
}

/// Settle a paced rig, then check the buffer is back in band and near the
/// target over the last `window` ticks.
fn assert_in_band(tag: &str, rig: &Rig, window: usize) {
    let cfg = lead_cfg();
    let occ = occupancy(rig, 0);
    let tail = &occ[occ.len() - window..];
    assert!(
        *tail.iter().max().unwrap() <= cfg.max_depth as usize + LOAD_SLACK,
        "{tag}: depth {} over max over the last {window} ticks",
        tail.iter().max().unwrap()
    );
    let m = mean(tail);
    assert!(
        (cfg.target_depth as f64 - 1.0..=cfg.target_depth as f64 + 2.0).contains(&m),
        "{tag}: mean depth {m} is not near the target"
    );
}

/// A starved burst (consecutive dropped input packets) leaves the host with
/// a backlog when the redundant windows land; the controller drains it back
/// to the target in bounded time, and the host never overflows.
fn drain_after_a_burst(burst: u64) {
    let cfg = lead_cfg();
    {
        let mut rig = Rig::paced(&[plain("burst", busy)], 0x02F2 + burst);
        rig.both(0, 10, 10);
        rig.run(400);
        let before = rig.seats[0].ticks.len();
        rig.drop_burst(0, LinkDir::Up, burst, delay(10));
        rig.run(900);
        rig.drain();
        let tag = format!("lead-burst-{burst}");
        rig.check(&tag, cfg.max_depth as usize + 2 * burst as usize);
        report_lead(&tag, &rig, 0);
        let occ = occupancy(&rig, 0);
        let settled = settled_at(
            &occ[..before + 900],
            before,
            0,
            cfg.max_depth as usize + LOAD_SLACK,
        )
        .unwrap_or_else(|| panic!("{tag}: never back inside the depth band"));
        assert!(
            settled - before <= 600,
            "{tag}: took {} ticks to come back",
            settled - before
        );
        assert_in_band(&tag, &rig, 200);
        assert_eq!(rig.seats[0].ticks.last().unwrap().stats.overflow, 0);
    }
}

/// A 4-packet burst.
#[test]
fn lead_controller_drains_the_backlog_a_4_packet_burst_leaves() {
    drain_after_a_burst(4);
}

/// An 8-packet burst: the backlog peaks above `max_depth`.
#[test]
fn lead_controller_drains_the_backlog_an_8_packet_burst_leaves() {
    drain_after_a_burst(8);
}

/// +-100 ppm of client clock error must neither expand the buffer nor make
/// the controller hunt: the rate stays at zero inside the dead band.
fn hold_under_clock_error(ppm: i64) {
    {
        let mut rig = Rig::paced(&[plain("ppm", busy)], 0x02F3);
        rig.seats[0].ppm = ppm;
        rig.both(0, 10, 10);
        rig.run(1500);
        // Host steps before the drain are wall time the client lived through.
        let wall = rig.seats[0].ticks.len() as i64;
        rig.drain();
        let tag = format!("lead-ppm{ppm}");
        rig.check(&tag, lead_cfg().max_depth as usize + LOAD_SLACK);
        report_lead(&tag, &rig, 0);
        assert_in_band(&tag, &rig, 600);
        let lead = lead_of(&rig, 0);
        assert_eq!(
            lead.pacer().stall_rebases() + lead.pacer().sleep_rebases(),
            0
        );
        assert!(
            lead.controller().rate_ppm().abs() <= 100,
            "the rate hunted: {}",
            lead.controller().rate_ppm()
        );
        // No uncontrolled fast-forward: the client ran as many ticks as the
        // wall time bought, plus the lead the controller holds.
        let host_ticks = wall;
        let ran = lead.ticks_run() as i64;
        assert!(
            (ran - host_ticks).abs() <= lead_cfg().max_depth as i64,
            "client ran {ran} ticks against the host's {host_ticks}"
        );
    }
}

/// Client clock 100 ppm slow.
#[test]
fn lead_controller_holds_the_buffer_under_minus_100_ppm() {
    hold_under_clock_error(-100);
}

/// Client clock 100 ppm fast.
#[test]
fn lead_controller_holds_the_buffer_under_plus_100_ppm() {
    hold_under_clock_error(100);
}

/// 100, 150 and 250 ms client stalls (and the frame that follows them): catch-up
/// is bounded per frame, wall time beyond the backlog is dropped and
/// rebased, the buffer is not expanded for good, and nothing overflows.
fn catch_up_after_a_stall(stall_ticks: usize) {
    let cfg = lead_cfg();
    {
        let mut rig = Rig::paced(&[plain("stall", busy)], 0x02F4 + stall_ticks as u64);
        rig.both(0, 10, 10);
        rig.run(400);
        let before = rig.seats[0].ticks.len();
        rig.seats[0].frozen = true;
        rig.run(stall_ticks);
        rig.seats[0].frozen = false;
        rig.run(900);
        // Host steps before the drain are wall time the client lived through.
        let wall = rig.seats[0].ticks.len() as i64;
        rig.drain();
        let tag = format!("lead-stall-{stall_ticks}");
        rig.check(
            &tag,
            cfg.max_depth as usize + cfg.max_catchup_ticks as usize + 6,
        );
        report_lead(&tag, &rig, 0);
        let seat = &rig.seats[0];
        assert!(
            seat.max_frame_ticks <= u64::from(cfg.max_catchup_ticks),
            "{tag}: a frame ran {} ticks",
            seat.max_frame_ticks
        );
        let occ = occupancy(&rig, 0);
        let settled = settled_at(
            &occ[..before + 900],
            before,
            0,
            cfg.max_depth as usize + LOAD_SLACK,
        )
        .unwrap_or_else(|| panic!("{tag}: never back inside the depth band"));
        assert!(settled - before <= 600, "{tag}: slow to recover");
        assert_in_band(&tag, &rig, 200);
        // No uncontrolled fast-forward: the client never ran more ticks
        // than its wall time bought, and every tick it did not run is wall
        // time the pacer dropped or buffer the controller drained.
        let lead = lead_of(&rig, 0);
        let host_ticks = wall;
        let ran = lead.ticks_run() as i64;
        assert!(
            ran <= host_ticks + 1,
            "{tag}: ran {ran} ticks against {host_ticks}"
        );
        // The most the controller can slow a client is `max_adjust_ppm` of
        // the wall time.
        let slowed = host_ticks * cfg.max_adjust_ppm / 1_000_000;
        assert!(
            host_ticks - ran <= lead.pacer().dropped_ticks() as i64 + slowed,
            "{tag}: ran {ran} ticks against {host_ticks} (dropped {}, slowed at most {slowed})",
            lead.pacer().dropped_ticks()
        );
    }
}

/// 100 ms stall.
#[test]
fn lead_controller_bounds_the_catch_up_after_a_100ms_stall() {
    catch_up_after_a_stall(6);
}

/// 150 ms stall.
#[test]
fn lead_controller_bounds_the_catch_up_after_a_150ms_stall() {
    catch_up_after_a_stall(9);
}

/// 250 ms stall: more wall time than the pacer keeps, so some is dropped.
#[test]
fn lead_controller_bounds_the_catch_up_after_a_250ms_stall() {
    catch_up_after_a_stall(15);
}

/// Sleep/resume: a frame of seconds drops all wall time at once, releases no
/// ticks, rebases the controller, and the stream resumes in order.
#[test]
fn lead_controller_rebases_on_sleep_and_resume() {
    let cfg = lead_cfg();
    let mut rig = Rig::paced(&[plain("sleep", busy)], 0x02F5);
    rig.both(0, 10, 10);
    rig.run(400);
    let before = rig.seats[0].ticks.len();
    rig.seats[0].frozen = true;
    rig.seats[0].owed_ns = 30_000_000_000;
    rig.run(20);
    rig.seats[0].frozen = false;
    rig.run(900);
    rig.drain();
    rig.check(
        "lead-sleep",
        cfg.max_depth as usize + cfg.max_catchup_ticks as usize + 6,
    );
    report_lead("lead-sleep", &rig, 0);
    let lead = lead_of(&rig, 0);
    assert_eq!(lead.pacer().sleep_rebases(), 1);
    assert!(lead.controller().stats().rebases >= 1);
    assert!(rig.seats[0].max_frame_ticks <= u64::from(cfg.max_catchup_ticks));
    let occ = occupancy(&rig, 0);
    let settled = settled_at(
        &occ[..before + 900],
        before,
        0,
        cfg.max_depth as usize + LOAD_SLACK,
    )
    .expect("never back inside the depth band");
    assert!(settled - before <= 600, "slow to recover");
    assert_in_band("lead-sleep", &rig, 200);
}
