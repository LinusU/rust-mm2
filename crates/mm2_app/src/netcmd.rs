//! Client-side fixed-tick command capture, history and send (NETPLAY-02C,
//! `docs/netplay-timeline.md`).
//!
//! One immutable [`TickCommand`] is captured per 60 Hz physics tick in
//! `FixedUpdate`, never per rendered frame, so the render rate (30, 60 or
//! 144 FPS) cannot drop, collapse or lengthen commands: Bevy runs the fixed
//! schedule zero, one or several times per frame and every run captures
//! exactly one command. The capture reads the `VehicleInput` that
//! `input::apply_player_input` just settled for the tick — the steering
//! ramp, pedal swapping and auto-reverse preference have run once, there —
//! and quantizes it with the same [`encode_input`] the legacy path and the
//! host decode (`decode_input`) use.
//!
//! * [`CommandHistory`] keeps a bounded ring of recent commands with their
//!   predicted tick, trimmed by [`CommandHistory::ack`] (the host's
//!   `SnapEntry::input_ack`, produced from NETPLAY-02B on). NETPLAY-04
//!   replays from it.
//! * Every [`SEND_INTERVAL_TICKS`] ticks the newest [`WINDOW`] commands go
//!   out as one `Message::InputWindow`. The cadence is fixed ticks, so the
//!   packet rate is independent of the render rate and each window
//!   repeats commands the previous ones carried: a short burst of loss
//!   costs nothing.
//! * **Nonblocking handoff.** Simulation and render systems never touch
//!   the socket or its writer mutex. They `try_send` a [`Job`] into a
//!   bounded channel ([`HANDOFF_DEPTH`]); a worker thread owns the
//!   blocking write. A full channel drops the job (counted) instead of
//!   waiting — the next window repeats its commands — and the worker sends
//!   only the newest queued job after a stall (older ones are counted as
//!   coalesced).
//! * **Escape / pause / focus.** A command is emitted every tick. While
//!   `vehicle_input` holds the car neutral (menu open, window unfocused,
//!   countdown lock, free camera) the command is the neutral one; ticks are
//!   never skipped, so seq stays one-per-tick.
//!
//! The legacy latest-only `Message::Input` stream keeps running beside this
//! one until NETPLAY-02B teaches the host to consume windows; the whole
//! path sits behind the [`NetplayTimeline`] selector.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};

use bevy::prelude::*;
use mm2_game::{PlayerVehicle, Session};
use mm2_net::{MAX_INPUT_WINDOW, NetError, TickCommand};
use mm2_vehicle::{HumanDriver, VehicleInput};

use crate::net::LobbyLink;
use crate::netdrive::{ResetEpoch, encode_input};
use crate::netplay_timeline::NetplayTimeline;

/// Most commands the history keeps: ~4.3 s at 60 Hz. Bounds memory when no
/// ack arrives; the replay consumer only needs the unacked tail.
pub const HISTORY_CAP: usize = 256;

/// Commands per sent window (the codec's [`MAX_INPUT_WINDOW`]).
pub const WINDOW: usize = MAX_INPUT_WINDOW;

/// A window leaves every this many fixed ticks (30 packets/s at 60 Hz).
/// Must stay `<= WINDOW` so consecutive windows overlap.
pub const SEND_INTERVAL_TICKS: u64 = 2;

/// Windows that may wait for the socket worker before new ones are dropped.
pub const HANDOFF_DEPTH: usize = 4;

const _: () = assert!(SEND_INTERVAL_TICKS as usize <= WINDOW);

/// One captured command with the bookkeeping 02D/04 pair it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickRecord {
    /// Command sequence, one per fixed tick, starting at 1.
    pub seq: u64,
    /// The client's own fixed-tick count (`Session::tick`) when captured.
    pub predicted_tick: u64,
    /// The sender's reset epoch when captured.
    pub epoch: u8,
    /// The quantized command.
    pub command: TickCommand,
}

/// Bounded history of recent, not yet acknowledged commands.
#[derive(Debug, Default)]
pub struct CommandHistory {
    generation: u64,
    next_seq: u64,
    acked: u64,
    records: VecDeque<TickRecord>,
    generated: u64,
    evicted: u64,
}

impl CommandHistory {
    /// An empty history for `generation`; seqs start at 1.
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            next_seq: 1,
            ..Self::default()
        }
    }

    /// The session generation the seqs belong to.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Restart for a new session generation (seq back to 1, nothing kept).
    pub fn reset(&mut self, generation: u64) {
        self.generation = generation;
        self.next_seq = 1;
        self.acked = 0;
        self.records.clear();
    }

    /// Append the next command; returns the seq it was given. A full ring
    /// evicts its oldest entry (counted) rather than growing.
    pub fn push(&mut self, predicted_tick: u64, epoch: u8, command: TickCommand) -> u64 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        if self.records.len() == HISTORY_CAP {
            self.records.pop_front();
            self.evicted += 1;
        }
        self.records.push_back(TickRecord {
            seq,
            predicted_tick,
            epoch,
            command,
        });
        self.generated += 1;
        seq
    }

    /// The host finalized every command up to and including `seq`: drop
    /// them. A lower value than already seen is stale and ignored.
    pub fn ack(&mut self, seq: u64) {
        if seq <= self.acked {
            return;
        }
        self.acked = seq;
        while self.records.front().is_some_and(|r| r.seq <= seq) {
            self.records.pop_front();
        }
    }

    /// Highest acknowledged seq (0 = none).
    pub fn acked(&self) -> u64 {
        self.acked
    }

    /// The unacked records, oldest first.
    pub fn unacked(&self) -> impl Iterator<Item = &TickRecord> {
        self.records.iter()
    }

    /// Number of records kept.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether nothing is kept.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Commands captured this generation.
    pub fn generated(&self) -> u64 {
        self.generated
    }

    /// Unacked commands pushed out of a full ring.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    /// The newest window of up to [`WINDOW`] consecutive commands that
    /// share the newest record's reset epoch (a command captured before a
    /// reset must not ride a window stamped with the new epoch).
    pub fn window(&self) -> Option<Job> {
        let newest = self.records.back()?;
        let run: Vec<&TickRecord> = self
            .records
            .iter()
            .rev()
            .take(WINDOW)
            .take_while(|r| r.epoch == newest.epoch)
            .collect();
        let first = run.last()?;
        Some(Job {
            generation: self.generation,
            epoch: newest.epoch,
            first_seq: first.seq,
            commands: run.iter().rev().map(|r| r.command).collect(),
        })
    }
}

/// One window ready for the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// Session generation.
    pub generation: u64,
    /// Sender reset epoch.
    pub epoch: u8,
    /// Seq of `commands[0]`.
    pub first_seq: u64,
    /// Consecutive commands, oldest first.
    pub commands: Vec<TickCommand>,
}

/// What actually reached the transport, shared with the worker thread.
#[derive(Debug, Default)]
struct Wire {
    packets: AtomicU64,
    slots: AtomicU64,
    errors: AtomicU64,
    coalesced: AtomicU64,
}

/// The handoff from the fixed schedule to the thread that owns the socket
/// write. Dropping it closes the channel; the worker exits after its
/// current write, and is deliberately not joined (a stalled peer must not
/// stall teardown either).
pub struct Handoff {
    tx: SyncSender<Job>,
    wire: Arc<Wire>,
    // Held so the thread is owned by the handoff; never joined.
    _worker: JoinHandle<()>,
}

impl std::fmt::Debug for Handoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handoff").finish_non_exhaustive()
    }
}

impl Handoff {
    /// Spawn the worker around `sink`, the only thing allowed to block.
    pub fn spawn<F>(mut sink: F) -> Self
    where
        F: FnMut(&Job) -> Result<(), NetError> + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel::<Job>(HANDOFF_DEPTH);
        let wire = Arc::new(Wire::default());
        let shared = Arc::clone(&wire);
        let worker = thread::Builder::new()
            .name("mm2-input-window".into())
            .spawn(move || {
                while let Ok(mut job) = rx.recv() {
                    // After a stall only the newest window matters: it
                    // repeats the recent commands the older ones carried.
                    while let Ok(newer) = rx.try_recv() {
                        shared.coalesced.fetch_add(1, Ordering::Relaxed);
                        job = newer;
                    }
                    match sink(&job) {
                        Ok(()) => {
                            shared.packets.fetch_add(1, Ordering::Relaxed);
                            shared
                                .slots
                                .fetch_add(job.commands.len() as u64, Ordering::Relaxed);
                        }
                        Err(_) => {
                            shared.errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            })
            .expect("spawn the input-window sender thread");
        Self {
            tx,
            wire,
            _worker: worker,
        }
    }

    /// A handoff that writes windows to `link`'s client send handle.
    pub fn for_link(link: &LobbyLink) -> Self {
        let ctl = link.ctl().clone();
        Self::spawn(move |job| {
            ctl.send_input_window(
                job.generation,
                job.epoch,
                job.first_seq,
                job.commands.clone(),
            )
        })
    }

    /// Queue a window without ever waiting. `false` = dropped (queue full
    /// or worker gone).
    fn offer(&self, job: Job) -> bool {
        match self.tx.try_send(job) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }
}

/// The client command stream: history, handoff and counters.
#[derive(Resource, Debug)]
pub struct CommandStream {
    history: CommandHistory,
    handoff: Option<Handoff>,
    queued_windows: u64,
    queued_slots: u64,
    handoff_dropped: u64,
}

impl Default for CommandStream {
    fn default() -> Self {
        Self {
            history: CommandHistory::new(0),
            handoff: None,
            queued_windows: 0,
            queued_slots: 0,
            handoff_dropped: 0,
        }
    }
}

impl CommandStream {
    /// Install the handoff (the production system does it from the link;
    /// tests install a fake sink).
    pub fn set_handoff(&mut self, handoff: Handoff) {
        self.handoff = Some(handoff);
    }

    /// Whether a transport is attached (the stream is sending).
    pub fn has_handoff(&self) -> bool {
        self.handoff.is_some()
    }

    /// The command history.
    pub fn history(&self) -> &CommandHistory {
        &self.history
    }

    /// Mutable history (the ack path).
    pub fn history_mut(&mut self) -> &mut CommandHistory {
        &mut self.history
    }

    /// Slots (commands) generated by the fixed schedule.
    pub fn generated_slots(&self) -> u64 {
        self.history.generated()
    }

    /// Slots handed to the worker queue (includes repeats).
    pub fn queued_slots(&self) -> u64 {
        self.queued_slots
    }

    /// Windows handed to the worker queue.
    pub fn queued_windows(&self) -> u64 {
        self.queued_windows
    }

    /// Windows dropped because the worker queue was full.
    pub fn handoff_dropped(&self) -> u64 {
        self.handoff_dropped
    }

    /// Slots the worker wrote to the transport (includes repeats).
    pub fn transmitted_slots(&self) -> u64 {
        self.handoff
            .as_ref()
            .map_or(0, |h| h.wire.slots.load(Ordering::Relaxed))
    }

    /// Windows the worker wrote to the transport.
    pub fn transmitted_windows(&self) -> u64 {
        self.handoff
            .as_ref()
            .map_or(0, |h| h.wire.packets.load(Ordering::Relaxed))
    }

    /// Windows the transport refused.
    pub fn send_errors(&self) -> u64 {
        self.handoff
            .as_ref()
            .map_or(0, |h| h.wire.errors.load(Ordering::Relaxed))
    }

    /// Queued windows superseded by a newer one after a worker stall.
    pub fn coalesced(&self) -> u64 {
        self.handoff
            .as_ref()
            .map_or(0, |h| h.wire.coalesced.load(Ordering::Relaxed))
    }
}

/// The wire command for a settled `VehicleInput`: the legacy quantizer,
/// minus the per-sample envelope.
pub fn tick_command(input: &VehicleInput, human: Option<&HumanDriver>) -> TickCommand {
    let q = encode_input(input, 0, 0);
    TickCommand {
        throttle: q.throttle,
        brake: q.brake,
        steer: q.steer,
        handbrake: q.handbrake,
        auto_reverse: human.is_none_or(|driver| driver.auto_reverse),
        forced_gear: q.forced_gear,
    }
}

/// The local car's settled controls, driver options and reset epoch.
type LocalCar = (
    &'static VehicleInput,
    Option<&'static HumanDriver>,
    Option<&'static ResetEpoch>,
);

/// `FixedUpdate`, after `input::apply_player_input`: capture this tick's
/// command, then hand the newest window to the socket worker every
/// [`SEND_INTERVAL_TICKS`]. Never blocks.
pub fn capture_and_send(
    link: Option<Res<LobbyLink>>,
    session: Res<Session>,
    mut stream: ResMut<CommandStream>,
    local: Query<LocalCar, With<PlayerVehicle>>,
) {
    if !session.is_playing() {
        return;
    }
    if link.as_ref().is_some_and(|l| l.closed || l.leaving()) {
        return;
    }
    let Ok((input, human, epoch)) = local.single() else {
        return;
    };
    if stream.history.generation() != session.wire_generation() {
        stream.history.reset(session.wire_generation());
    }
    if stream.handoff.is_none()
        && let Some(link) = link.as_ref()
    {
        stream.handoff = Some(Handoff::for_link(link));
    }
    let seq = stream.history.push(
        session.tick(),
        epoch.map_or(0, |e| e.0),
        tick_command(input, human),
    );
    if !seq.is_multiple_of(SEND_INTERVAL_TICKS) {
        return;
    }
    let (Some(job), Some(handoff)) = (stream.history.window(), stream.handoff.as_ref()) else {
        return;
    };
    let slots = job.commands.len() as u64;
    if handoff.offer(job) {
        stream.queued_windows += 1;
        stream.queued_slots += slots;
    } else {
        stream.handoff_dropped += 1;
    }
}

/// Register the stream behind the [`NetplayTimeline`] selector.
pub fn add_systems(app: &mut App) {
    app.init_resource::<CommandStream>().add_systems(
        FixedUpdate,
        capture_and_send
            .after(crate::input::apply_player_input)
            .run_if(|timeline: Option<Res<NetplayTimeline>>| timeline.is_some_and(|t| t.enabled))
            .run_if(resource_exists::<LobbyLink>),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(n: u8) -> TickCommand {
        TickCommand {
            throttle: n,
            brake: 0,
            steer: 0,
            handbrake: 0,
            auto_reverse: true,
            forced_gear: None,
        }
    }

    #[test]
    fn seqs_start_at_one_and_the_ring_is_bounded() {
        let mut h = CommandHistory::new(9);
        for i in 0..(HISTORY_CAP as u64 + 10) {
            assert_eq!(h.push(i, 0, cmd(i as u8)), i + 1);
        }
        assert_eq!(h.len(), HISTORY_CAP);
        assert_eq!(h.evicted(), 10);
        assert_eq!(h.generated(), HISTORY_CAP as u64 + 10);
        assert_eq!(h.unacked().next().unwrap().seq, 11);
    }

    #[test]
    fn acks_trim_and_stale_acks_are_ignored() {
        let mut h = CommandHistory::new(1);
        for i in 0..10 {
            h.push(i, 0, cmd(i as u8));
        }
        h.ack(4);
        assert_eq!(h.unacked().next().unwrap().seq, 5);
        h.ack(2);
        assert_eq!((h.acked(), h.len()), (4, 6));
        h.ack(100);
        assert!(h.is_empty());
    }

    #[test]
    fn windows_are_the_newest_consecutive_run() {
        let mut h = CommandHistory::new(1);
        assert!(h.window().is_none());
        for i in 0..40u8 {
            h.push(u64::from(i), 0, cmd(i));
        }
        let w = h.window().unwrap();
        assert_eq!((w.first_seq, w.commands.len()), (25, WINDOW));
        assert_eq!(w.commands[0], cmd(24));
        assert_eq!(w.commands[WINDOW - 1], cmd(39));
    }

    #[test]
    fn a_window_never_mixes_reset_epochs() {
        let mut h = CommandHistory::new(1);
        for i in 0..5u8 {
            h.push(u64::from(i), 0, cmd(i));
        }
        for i in 5..8u8 {
            h.push(u64::from(i), 1, cmd(i));
        }
        let w = h.window().unwrap();
        assert_eq!((w.epoch, w.first_seq, w.commands.len()), (1, 6, 3));
    }

    #[test]
    fn a_new_generation_restarts_the_sequence() {
        let mut h = CommandHistory::new(1);
        h.push(0, 0, cmd(1));
        h.ack(1);
        h.reset(2);
        assert_eq!((h.generation(), h.acked(), h.len()), (2, 0, 0));
        assert_eq!(h.push(0, 0, cmd(1)), 1);
    }

    #[test]
    fn a_blocked_socket_never_blocks_the_producer() {
        use std::sync::{Barrier, Mutex};
        let gate = Arc::new(Barrier::new(2));
        let sent = Arc::new(Mutex::new(Vec::<u64>::new()));
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (g, s, e) = (Arc::clone(&gate), Arc::clone(&sent), Arc::clone(&entered));
        let mut first = true;
        let handoff = Handoff::spawn(move |job| {
            if std::mem::take(&mut first) {
                e.store(true, Ordering::SeqCst);
                g.wait();
            }
            s.lock().unwrap().push(job.first_seq);
            Ok(())
        });
        let job = |n: u64| Job {
            generation: 1,
            epoch: 0,
            first_seq: n,
            commands: vec![cmd(0)],
        };
        // Park the worker inside its first write before flooding it.
        assert!(handoff.offer(job(1)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !entered.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(entered.load(Ordering::SeqCst));
        let started = std::time::Instant::now();
        let mut dropped = 0;
        // The worker is stuck inside its first write; the producer offers
        // far more than the queue holds and must return at once.
        for n in 2..=100 {
            if !handoff.offer(job(n)) {
                dropped += 1;
            }
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(dropped >= 99 - HANDOFF_DEPTH, "dropped {dropped}");
        gate.wait();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while handoff.wire.packets.load(Ordering::Relaxed) < 2
            && std::time::Instant::now() < deadline
        {
            thread::yield_now();
        }
        // Queued windows after the stall collapse to the newest queued.
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert_eq!(sent[0], 1);
        assert!(handoff.wire.coalesced.load(Ordering::Relaxed) >= 1);
    }
}
