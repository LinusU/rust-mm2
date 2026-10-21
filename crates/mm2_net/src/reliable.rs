//! A bounded reliable-message channel over the unreliable datagram lane
//! (NETPLAY-06E-1).
//!
//! The lane drops, reorders and duplicates datagrams by design; a message
//! that matters and is sent once (Start, Cancel, roster removal, a reset
//! ask) must still arrive. This module is the transport-independent state
//! machine for that: it never touches a socket or a clock of its own —
//! callers pass `now` in milliseconds on whatever clock they drive (the
//! lane's wall clock, or a test's manual one) and carry the returned
//! payloads on [`LaneTag::Reliable`](crate::LaneTag) and
//! [`LaneTag::ReliableAck`](crate::LaneTag) datagrams.
//!
//! **Delivery policy: one ordered, exactly-once channel per direction per
//! seat.** Messages are numbered from 1, the receiver hands them to the
//! app in id order, each exactly once. Ordered, because the callers
//! (lifecycle, roster, reset epochs) are state machines that must not see
//! a Cancel before its Start.
//!
//! **Bounds, all counted, none silent** (designed numbers):
//! - at most [`WINDOW`] messages unacked: a send beyond it is refused with
//!   [`SendError::Full`] and counted in [`SenderStats::refused_full`];
//! - a message is transmitted at most [`MAX_ATTEMPTS`] times, spaced
//!   [`RETRY_BASE_MS`] doubling to [`RETRY_CAP_MS`]. When the oldest
//!   unacked message runs out of attempts the peer is a black hole: the
//!   channel goes **dead** (an ordered channel cannot skip a message),
//!   every unacked message is counted in [`SenderStats::given_up`], and
//!   later sends fail with [`SendError::Dead`] so the caller learns of it
//!   instead of queueing into a void;
//! - the receiver buffers at most [`WINDOW`] ids ahead and holds at most
//!   [`DELIVERED_CAP`] undelivered messages; beyond either it refuses the
//!   datagram (counted) and the sender simply retransmits — backpressure,
//!   never loss.
//!
//! Acks are cumulative plus selective and are sent for every reliable
//! datagram, duplicates included, so a lost ack is repaired by the next
//! retransmission.
//!
//! Wire payloads (little-endian): data `id u64 | body`, ack `cum u64 |
//! mask u32` where `cum` is the highest id delivered in order and bit `i`
//! of `mask` says id `cum + 2 + i` is buffered (`cum + 1` is by definition
//! the missing one).

use std::collections::{BTreeMap, VecDeque};

/// Span of ids the sender keeps in flight, and ids ahead of the delivered
/// prefix the receiver buffers (equal, so a conforming sender never
/// overruns the receiver).
pub const WINDOW: usize = 32;
/// Delivered-but-not-yet-popped messages the receiver holds before it
/// stops accepting new ones.
pub const DELIVERED_CAP: usize = 64;
/// Total transmissions of one message (first send included) before the
/// channel is declared dead.
pub const MAX_ATTEMPTS: u32 = 8;
/// Delay before the first retransmission, in milliseconds.
pub const RETRY_BASE_MS: u64 = 100;
/// Ceiling of the doubling retransmission delay, in milliseconds.
pub const RETRY_CAP_MS: u64 = 800;
/// Bytes of the data payload before the message body.
pub const DATA_HEADER: usize = 8;
/// Bytes of an ack payload.
pub const ACK_LEN: usize = 12;

/// Why a send was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// [`WINDOW`] messages are already unacked.
    #[error("reliable channel full: {WINDOW} messages unacknowledged")]
    Full,
    /// The body does not fit one datagram.
    #[error("reliable message exceeds one datagram")]
    TooLarge,
    /// The peer stopped acknowledging; the channel gave up.
    #[error("reliable channel dead: the peer stopped acknowledging")]
    Dead,
}

/// What the sending half has done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SenderStats {
    /// Messages accepted for sending.
    pub queued: u64,
    /// Retransmissions emitted.
    pub retransmits: u64,
    /// Messages the peer acknowledged.
    pub acked: u64,
    /// Sends refused because the window was full.
    pub refused_full: u64,
    /// Sends refused because the body exceeds a datagram.
    pub refused_large: u64,
    /// Sends refused because the channel is dead.
    pub refused_dead: u64,
    /// Unacked messages abandoned when the channel died.
    pub given_up: u64,
    /// Acks that named an id never sent (ignored).
    pub bad_acks: u64,
    /// Unacked messages now.
    pub in_flight: usize,
    /// The channel gave up on the peer.
    pub dead: bool,
}

/// What the receiving half has done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReceiverStats {
    /// Messages handed to the app (each exactly once).
    pub delivered: u64,
    /// Datagrams for an id already delivered or buffered.
    pub duplicates: u64,
    /// Datagrams that arrived ahead of a missing id and were buffered.
    pub reordered: u64,
    /// Datagrams more than [`WINDOW`] ahead, refused.
    pub out_of_window: u64,
    /// Datagrams refused because [`DELIVERED_CAP`] messages await the app.
    pub backpressure: u64,
    /// Undecodable payloads (counted by the caller via [`decode_data`]).
    pub malformed: u64,
    /// Messages delivered and not yet popped.
    pub pending: usize,
}

/// Both halves' counters for one channel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReliableStats {
    /// The sending half.
    pub sender: SenderStats,
    /// The receiving half.
    pub receiver: ReceiverStats,
}

#[derive(Debug)]
struct Pending {
    id: u64,
    generation: u64,
    body: Vec<u8>,
    attempts: u32,
    due: u64,
}

fn retry_delay(attempts: u32) -> u64 {
    RETRY_BASE_MS
        .saturating_mul(1u64 << attempts.saturating_sub(1).min(16))
        .min(RETRY_CAP_MS)
}

/// The sending half: numbers messages, keeps the unacked set and decides
/// when each is repeated.
#[derive(Debug)]
pub struct ReliableSender {
    next_id: u64,
    pending: VecDeque<Pending>,
    max_body: usize,
    stats: SenderStats,
}

impl ReliableSender {
    /// A sender whose bodies may be at most `max_body` bytes.
    pub fn new(max_body: usize) -> Self {
        Self {
            next_id: 1,
            pending: VecDeque::new(),
            max_body,
            stats: SenderStats::default(),
        }
    }

    /// Accept `body` (to ride a datagram of `generation`) and return the
    /// data payload for its first transmission, which the caller sends
    /// now. Nonblocking: never waits, never allocates beyond the bounds.
    pub fn send(&mut self, now: u64, generation: u64, body: &[u8]) -> Result<Vec<u8>, SendError> {
        if self.stats.dead {
            self.stats.refused_dead += 1;
            return Err(SendError::Dead);
        }
        if body.len() > self.max_body {
            self.stats.refused_large += 1;
            return Err(SendError::TooLarge);
        }
        // The window is a span of ids, not a count: selectively acked ids
        // leave `pending`, but the receiver only buffers `WINDOW` ids past
        // its delivered prefix, which the oldest unacked id bounds.
        if self
            .pending
            .front()
            .is_some_and(|oldest| self.next_id - oldest.id >= WINDOW as u64)
        {
            self.stats.refused_full += 1;
            return Err(SendError::Full);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.pending.push_back(Pending {
            id,
            generation,
            body: body.to_vec(),
            attempts: 1,
            due: now.saturating_add(retry_delay(1)),
        });
        self.stats.queued += 1;
        Ok(encode_data(id, body))
    }

    /// Apply an acknowledgement: everything up to `cum` and the ids
    /// `mask` marks.
    pub fn on_ack(&mut self, cum: u64, mask: u32) {
        if cum >= self.next_id {
            self.stats.bad_acks += 1;
            return;
        }
        let before = self.pending.len();
        self.pending.retain(|p| {
            if p.id <= cum {
                return false;
            }
            let selected =
                p.id.checked_sub(cum + 2)
                    .is_some_and(|bit| bit < 32 && mask & (1 << bit) != 0);
            !selected
        });
        self.stats.acked += (before - self.pending.len()) as u64;
    }

    /// Messages due for another transmission at `now`, as
    /// `(generation, payload)`. A message out of attempts kills the
    /// channel instead.
    pub fn poll(&mut self, now: u64) -> Vec<(u64, Vec<u8>)> {
        let mut out = Vec::new();
        if self.stats.dead {
            return out;
        }
        if self
            .pending
            .iter()
            .any(|p| p.due <= now && p.attempts >= MAX_ATTEMPTS)
        {
            self.stats.given_up += self.pending.len() as u64;
            self.pending.clear();
            self.stats.dead = true;
            return out;
        }
        for p in &mut self.pending {
            if p.due <= now {
                p.attempts += 1;
                p.due = now.saturating_add(retry_delay(p.attempts));
                self.stats.retransmits += 1;
                out.push((p.generation, encode_data(p.id, &p.body)));
            }
        }
        out
    }

    /// Whether the channel gave up on the peer.
    pub fn is_dead(&self) -> bool {
        self.stats.dead
    }

    /// The counters.
    pub fn stats(&self) -> SenderStats {
        SenderStats {
            in_flight: self.pending.len(),
            ..self.stats
        }
    }
}

/// The receiving half: dedups, reorders and delivers exactly once.
#[derive(Debug, Default)]
pub struct ReliableReceiver {
    cum: u64,
    buffered: BTreeMap<u64, Vec<u8>>,
    delivered: VecDeque<Vec<u8>>,
    stats: ReceiverStats,
}

impl ReliableReceiver {
    /// An empty receiver expecting id 1.
    pub fn new() -> Self {
        Self::default()
    }

    /// Take in one data datagram and return the ack payload to send back
    /// (always, duplicates and refusals included: the ack is how a sender
    /// whose earlier ack was lost converges).
    pub fn on_data(&mut self, id: u64, body: &[u8]) -> Vec<u8> {
        if id <= self.cum || self.buffered.contains_key(&id) {
            self.stats.duplicates += 1;
        } else if id - self.cum > WINDOW as u64 {
            self.stats.out_of_window += 1;
        } else if self.delivered.len() >= DELIVERED_CAP {
            self.stats.backpressure += 1;
        } else {
            if id != self.cum + 1 {
                self.stats.reordered += 1;
            }
            self.buffered.insert(id, body.to_vec());
            while let Some(next) = self.buffered.remove(&(self.cum + 1)) {
                self.cum += 1;
                self.delivered.push_back(next);
                self.stats.delivered += 1;
            }
        }
        self.ack()
    }

    /// Count an undecodable reliable payload.
    pub fn note_malformed(&mut self) {
        self.stats.malformed += 1;
    }

    /// The current ack payload.
    pub fn ack(&self) -> Vec<u8> {
        let mut mask = 0u32;
        for id in self.buffered.keys() {
            // Buffered ids are > cum + 1 and within WINDOW (= 32) of cum.
            if let Some(bit) = id.checked_sub(self.cum + 2)
                && bit < 32
            {
                mask |= 1 << bit;
            }
        }
        encode_ack(self.cum, mask)
    }

    /// The next delivered message, in id order.
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        self.delivered.pop_front()
    }

    /// The next delivered message when `take` accepts it, else leave it
    /// (and everything behind it, to keep id order) queued. Lets the lane
    /// consume the message classes it owns while the rest stay for
    /// [`pop`](Self::pop).
    pub fn pop_if(&mut self, take: impl FnOnce(&[u8]) -> bool) -> Option<Vec<u8>> {
        if take(self.delivered.front()?) {
            self.delivered.pop_front()
        } else {
            None
        }
    }

    /// The counters.
    pub fn stats(&self) -> ReceiverStats {
        ReceiverStats {
            pending: self.delivered.len(),
            ..self.stats
        }
    }
}

/// One seat's channel in one direction pair.
#[derive(Debug)]
pub struct ReliableChannel {
    /// Outbound half.
    pub sender: ReliableSender,
    /// Inbound half.
    pub receiver: ReliableReceiver,
}

impl ReliableChannel {
    /// A channel whose outbound bodies fit `max_body` bytes.
    pub fn new(max_body: usize) -> Self {
        Self {
            sender: ReliableSender::new(max_body),
            receiver: ReliableReceiver::new(),
        }
    }

    /// Both halves' counters.
    pub fn stats(&self) -> ReliableStats {
        ReliableStats {
            sender: self.sender.stats(),
            receiver: self.receiver.stats(),
        }
    }
}

fn encode_data(id: u64, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(DATA_HEADER + body.len());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Split a data payload into `(id, body)`; `None` when malformed (short,
/// or id 0).
pub fn decode_data(payload: &[u8]) -> Option<(u64, &[u8])> {
    let id = u64::from_le_bytes(payload.get(..DATA_HEADER)?.try_into().ok()?);
    (id != 0).then(|| (id, &payload[DATA_HEADER..]))
}

fn encode_ack(cum: u64, mask: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(ACK_LEN);
    out.extend_from_slice(&cum.to_le_bytes());
    out.extend_from_slice(&mask.to_le_bytes());
    out
}

/// Split an ack payload into `(cum, mask)`; `None` unless exactly
/// [`ACK_LEN`] bytes.
pub fn decode_ack(payload: &[u8]) -> Option<(u64, u32)> {
    if payload.len() != ACK_LEN {
        return None;
    }
    Some((
        u64::from_le_bytes(payload[..8].try_into().ok()?),
        u32::from_le_bytes(payload[8..].try_into().ok()?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deliver(rx: &mut ReliableReceiver, tx: &mut ReliableSender, payload: &[u8]) {
        let (id, body) = decode_data(payload).unwrap();
        let ack = rx.on_data(id, body);
        let (cum, mask) = decode_ack(&ack).unwrap();
        tx.on_ack(cum, mask);
    }

    #[test]
    fn in_order_delivery_and_ack() {
        let mut tx = ReliableSender::new(100);
        let mut rx = ReliableReceiver::new();
        for i in 0..5u8 {
            let p = tx.send(0, 1, &[i]).unwrap();
            deliver(&mut rx, &mut tx, &p);
        }
        let got: Vec<_> = std::iter::from_fn(|| rx.pop()).collect();
        assert_eq!(got, (0..5u8).map(|i| vec![i]).collect::<Vec<_>>());
        assert_eq!(tx.stats().in_flight, 0);
        assert_eq!(tx.stats().acked, 5);
    }

    #[test]
    fn reorder_and_duplicates_deliver_once_in_order() {
        let mut tx = ReliableSender::new(100);
        let mut rx = ReliableReceiver::new();
        let p: Vec<_> = (0..4u8).map(|i| tx.send(0, 1, &[i]).unwrap()).collect();
        for k in [2, 0, 2, 3, 1, 0, 1] {
            deliver(&mut rx, &mut tx, &p[k]);
        }
        let got: Vec<_> = std::iter::from_fn(|| rx.pop()).collect();
        assert_eq!(got, (0..4u8).map(|i| vec![i]).collect::<Vec<_>>());
        let s = rx.stats();
        assert_eq!((s.delivered, s.duplicates, s.reordered), (4, 3, 2));
        assert_eq!(tx.stats().in_flight, 0);
    }

    #[test]
    fn selective_ack_stops_repeating_what_arrived() {
        let mut tx = ReliableSender::new(100);
        let mut rx = ReliableReceiver::new();
        let p: Vec<_> = (0..3u8).map(|i| tx.send(0, 1, &[i]).unwrap()).collect();
        // Message 1 is lost; 2 and 3 arrive.
        deliver(&mut rx, &mut tx, &p[1]);
        deliver(&mut rx, &mut tx, &p[2]);
        assert_eq!(tx.stats().in_flight, 1);
        let again = tx.poll(RETRY_BASE_MS);
        assert_eq!(again.len(), 1);
        assert_eq!(decode_data(&again[0].1).unwrap().0, 1);
    }

    #[test]
    fn retry_backs_off_then_gives_up_dead_and_counted() {
        let mut tx = ReliableSender::new(100);
        tx.send(0, 1, b"x").unwrap();
        let mut now = 0;
        let mut sends = 1;
        let mut gaps = Vec::new();
        while !tx.is_dead() {
            let next = now + 1;
            now = next;
            let n = tx.poll(now).len();
            if n > 0 {
                gaps.push(now);
                sends += n;
            }
            assert!(now < 60_000, "never gave up");
        }
        assert_eq!(sends as u32, MAX_ATTEMPTS);
        assert_eq!(gaps[0], RETRY_BASE_MS);
        assert_eq!(gaps[1] - gaps[0], 200);
        assert_eq!(gaps[2] - gaps[1], 400);
        assert_eq!(gaps[3] - gaps[2], RETRY_CAP_MS);
        let s = tx.stats();
        assert_eq!((s.given_up, s.in_flight, s.retransmits), (1, 0, 7));
        assert_eq!(tx.send(now, 1, b"y"), Err(SendError::Dead));
        assert_eq!(tx.stats().refused_dead, 1);
    }

    #[test]
    fn window_and_size_are_bounded_and_counted() {
        let mut tx = ReliableSender::new(4);
        for _ in 0..WINDOW {
            tx.send(0, 1, b"ab").unwrap();
        }
        assert_eq!(tx.send(0, 1, b"ab"), Err(SendError::Full));
        assert_eq!(tx.send(0, 1, b"abcde"), Err(SendError::TooLarge));
        let s = tx.stats();
        assert_eq!((s.refused_full, s.refused_large), (1, 1));
        tx.on_ack(1, 0);
        assert!(tx.send(0, 1, b"ab").is_ok());
    }

    #[test]
    fn selectively_acked_ids_do_not_widen_the_window() {
        let mut tx = ReliableSender::new(10);
        let first = tx.send(0, 1, b"0").unwrap();
        for _ in 1..WINDOW {
            tx.send(0, 1, b"x").unwrap();
        }
        // Everything but id 1 arrives: the unacked set is one message,
        // but the receiver cannot buffer past id 1 + WINDOW.
        let mut rx = ReliableReceiver::new();
        for id in 2..=WINDOW as u64 {
            let (cum, mask) = decode_ack(&rx.on_data(id, b"x")).unwrap();
            tx.on_ack(cum, mask);
        }
        assert_eq!(tx.stats().in_flight, 1);
        assert_eq!(tx.send(0, 1, b"y"), Err(SendError::Full));
        let (id, body) = decode_data(&first).unwrap();
        let (cum, mask) = decode_ack(&rx.on_data(id, body)).unwrap();
        tx.on_ack(cum, mask);
        assert!(tx.send(0, 1, b"y").is_ok());
    }

    #[test]
    fn receiver_refuses_beyond_window_and_backpressure() {
        let mut rx = ReliableReceiver::new();
        rx.on_data(WINDOW as u64 + 1, b"far");
        assert_eq!(rx.stats().out_of_window, 1);
        assert_eq!(rx.stats().delivered, 0);
        // Fill the delivered queue without popping.
        let mut id = 1;
        while rx.stats().pending < DELIVERED_CAP {
            rx.on_data(id, b"m");
            id += 1;
        }
        let ack = rx.on_data(id, b"m");
        assert_eq!(rx.stats().backpressure, 1);
        assert_eq!(decode_ack(&ack).unwrap().0, DELIVERED_CAP as u64);
        // Popping lets the retransmission through.
        rx.pop();
        rx.on_data(id, b"m");
        assert_eq!(rx.stats().delivered, DELIVERED_CAP as u64 + 1);
    }

    #[test]
    fn nonsense_acks_and_payloads_are_rejected() {
        let mut tx = ReliableSender::new(10);
        tx.send(0, 1, b"x").unwrap();
        tx.on_ack(5, 0);
        assert_eq!(tx.stats().bad_acks, 1);
        assert_eq!(tx.stats().in_flight, 1);
        assert!(decode_data(&[0; 8]).is_none());
        assert!(decode_data(&[1; 3]).is_none());
        assert!(decode_ack(&[0; 11]).is_none());
    }
}
