//! Snapshot fragmentation, bounded assembly and ACKed baselines for the
//! datagram lane (NETPLAY-06C).
//!
//! A snapshot that outgrows one datagram ([`MAX_PAYLOAD`]) is cut into
//! **groups** and each group into **fragments**. A group is the unit the
//! producer says is physically self-contained — e.g. the owner's car plus
//! the cars it is in contact with, versus an unrelated part of the world —
//! so the receiver delivers each group the moment *its* fragments are all
//! there, and losing a fragment of one group never withholds another. The
//! transport only splits where the producer drew a boundary; it never
//! cuts inside a group's meaning, and a group is delivered whole or not
//! at all.
//!
//! Every fragment carries the coherent server tick it belongs to, so the
//! [`Assembler`] keys on it: assembly is bounded in ticks in flight,
//! groups, fragments and bytes; an assembly that does not complete within
//! [`FRAGMENT_TTL`] is expired and counted; a newer tick evicts the oldest
//! incomplete one instead of waiting for it.
//!
//! Delta compression is lossless over bytes ([`delta_encode`] /
//! [`delta_apply`]), so a continuation field (a velocity, an accumulator,
//! a sequence number) the producer wrote is reproduced exactly: there is
//! no field-level omission to get wrong. A delta is only ever applied
//! against a baseline the receiver still holds ([`BaselineReceiver`]) and
//! only ever *sent* against one the receiver has acknowledged and the
//! sender still holds ([`BaselineSender`]); a generation or reset-epoch
//! change drops every baseline, and a baseline older than
//! [`REFRESH_TICKS`] is replaced by a full state.
//!
//! Fragment payload (little-endian, [`FRAG_HEADER`] bytes then data):
//! `tick u64 | group u8 | groups u8 | index u8 | count u8`.
//! State payload: `kind u8 (0 full, 1 delta) | base_tick u64 | body`.
//! Ack payload: `epoch u8 | group u8 | tick u64`.
//! State frame (a fragment group carrying a baselined state instead of a
//! bare message): `STATE_FRAME u8 | group u8 | epoch u8 | state payload`.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use crate::rt::MAX_PAYLOAD;

/// Bytes of fragment header ahead of a fragment's data.
pub const FRAG_HEADER: usize = 12;
/// Data bytes one fragment carries.
pub const FRAG_DATA: usize = MAX_PAYLOAD - FRAG_HEADER;
/// Most groups a tick may be split into.
pub const MAX_GROUPS: usize = 8;
/// Most fragments one group may span (about 9 KB).
pub const MAX_FRAGMENTS: usize = 8;
/// Ticks the assembler holds incomplete at once.
pub const MAX_ASSEMBLIES: usize = 4;
/// A fragment this many ticks behind the newest seen is stale.
pub const MAX_TICK_LAG: u64 = 8;
/// How long an assembly may stay incomplete before it is expired.
pub const FRAGMENT_TTL: Duration = Duration::from_millis(250);
/// Baselines each side keeps. At least [`REFRESH_TICKS`]: the sender
/// deltas against an acknowledged state up to that many ticks old, and
/// the receiver must still hold it however many newer states it applied
/// while the acknowledgement was in flight or lost. (Eight was too few:
/// past a round trip of about eight snapshots every delta was refused
/// `MissingBase` and updates fell to the refresh cadence.)
pub const BASELINE_CAP: usize = 64;
const _: () = assert!(BASELINE_CAP as u64 >= REFRESH_TICKS);
/// A sender falls back to a full state when its newest acknowledged
/// baseline is more than this many ticks behind (designed bound: one
/// second at the 60 Hz fixed step).
pub const REFRESH_TICKS: u64 = 60;
/// Largest state a delta may describe.
pub const MAX_STATE: usize = 64 * 1024;

const KIND_FULL: u8 = 0;
const KIND_DELTA: u8 = 1;
const STATE_HEADER: usize = 9;

/// Why a split was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SplitError {
    /// No groups, or more than [`MAX_GROUPS`].
    #[error("a tick needs 1..={MAX_GROUPS} groups")]
    Groups,
    /// A group needs more than [`MAX_FRAGMENTS`] fragments.
    #[error("group {0} exceeds {MAX_FRAGMENTS} fragments")]
    GroupTooLarge(usize),
}

/// Cut `groups` of tick `tick` into fragment payloads, each at most
/// [`MAX_PAYLOAD`] bytes. Groups keep their order; fragments of a group
/// are contiguous.
pub fn split(tick: u64, groups: &[&[u8]]) -> Result<Vec<Vec<u8>>, SplitError> {
    if groups.is_empty() || groups.len() > MAX_GROUPS {
        return Err(SplitError::Groups);
    }
    let mut out = Vec::new();
    for (g, data) in groups.iter().enumerate() {
        let count = data.len().div_ceil(FRAG_DATA).max(1);
        if count > MAX_FRAGMENTS {
            return Err(SplitError::GroupTooLarge(g));
        }
        for i in 0..count {
            let chunk =
                &data[(i * FRAG_DATA).min(data.len())..((i + 1) * FRAG_DATA).min(data.len())];
            let mut p = Vec::with_capacity(FRAG_HEADER + chunk.len());
            p.extend_from_slice(&tick.to_le_bytes());
            p.extend_from_slice(&[g as u8, groups.len() as u8, i as u8, count as u8]);
            p.extend_from_slice(chunk);
            out.push(p);
        }
    }
    Ok(out)
}

/// A group whose fragments all arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    /// The server tick every fragment named.
    pub tick: u64,
    /// The group's index within the tick.
    pub group: u8,
    /// How many groups the tick has.
    pub groups: u8,
    /// The reassembled bytes.
    pub data: Vec<u8>,
    /// Whether this was the tick's last group to complete.
    pub tick_done: bool,
}

/// Why a fragment was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragRefuse {
    /// Too short, or counts outside the bounds.
    Malformed,
    /// Older than the newest tick by more than [`MAX_TICK_LAG`], or older
    /// than every assembly held while the table is full.
    Stale,
    /// Disagrees with the tick's earlier fragments (group count or the
    /// group's fragment count).
    Inconsistent,
    /// A fragment already held, or of an already-delivered group.
    Duplicate,
}

/// What assembly has done, for measurement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssemblerStats {
    /// Groups delivered.
    pub groups_completed: u64,
    /// Ticks whose every group was delivered.
    pub ticks_completed: u64,
    /// Incomplete assemblies expired by age ([`FRAGMENT_TTL`]).
    pub expired: u64,
    /// Incomplete assemblies evicted for a newer tick.
    pub evicted: u64,
    /// Fragments refused as [`FragRefuse::Malformed`].
    pub malformed: u64,
    /// Fragments refused as [`FragRefuse::Stale`].
    pub stale: u64,
    /// Fragments refused as [`FragRefuse::Inconsistent`].
    pub inconsistent: u64,
    /// Fragments refused as [`FragRefuse::Duplicate`].
    pub duplicates: u64,
    /// The longest an assembly that did complete took, first fragment to
    /// last.
    pub max_complete_wait: Duration,
}

#[derive(Debug)]
struct GroupAsm {
    parts: Vec<Option<Vec<u8>>>,
    got: usize,
    done: bool,
}

#[derive(Debug)]
struct Assembly {
    first: Instant,
    groups: Vec<Option<GroupAsm>>,
    delivered: usize,
}

/// Bounded reassembly of fragment payloads.
#[derive(Debug, Default)]
pub struct Assembler {
    by_tick: BTreeMap<u64, Assembly>,
    newest: u64,
    stats: AssemblerStats,
}

impl Assembler {
    /// An empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counters so far.
    pub fn stats(&self) -> AssemblerStats {
        self.stats
    }

    /// Incomplete ticks held now (never more than [`MAX_ASSEMBLIES`]).
    pub fn in_flight(&self) -> usize {
        self.by_tick.len()
    }

    /// Drop every assembly older than [`FRAGMENT_TTL`]; returns how many.
    pub fn expire(&mut self, now: Instant) -> usize {
        let before = self.by_tick.len();
        self.by_tick
            .retain(|_, a| now.saturating_duration_since(a.first) < FRAGMENT_TTL);
        let n = before - self.by_tick.len();
        self.stats.expired += n as u64;
        n
    }

    /// Forget everything (generation or epoch boundary).
    pub fn clear(&mut self) {
        self.by_tick.clear();
        self.newest = 0;
    }

    /// Take one fragment payload. `Ok(Some(_))` when it completed a
    /// group; `Ok(None)` when it was stored; a refusal is counted.
    pub fn accept(
        &mut self,
        payload: &[u8],
        now: Instant,
    ) -> Result<Option<Completed>, FragRefuse> {
        self.expire(now);
        let r = self.accept_inner(payload, now);
        match r {
            Err(FragRefuse::Malformed) => self.stats.malformed += 1,
            Err(FragRefuse::Stale) => self.stats.stale += 1,
            Err(FragRefuse::Inconsistent) => self.stats.inconsistent += 1,
            Err(FragRefuse::Duplicate) => self.stats.duplicates += 1,
            Ok(_) => {}
        }
        r
    }

    fn accept_inner(
        &mut self,
        payload: &[u8],
        now: Instant,
    ) -> Result<Option<Completed>, FragRefuse> {
        if payload.len() < FRAG_HEADER {
            return Err(FragRefuse::Malformed);
        }
        let tick = u64::from_le_bytes(payload[..8].try_into().unwrap());
        let (group, groups, index, count) = (
            payload[8] as usize,
            payload[9] as usize,
            payload[10] as usize,
            payload[11] as usize,
        );
        if groups == 0
            || groups > MAX_GROUPS
            || group >= groups
            || count == 0
            || count > MAX_FRAGMENTS
            || index >= count
            || payload.len() - FRAG_HEADER > FRAG_DATA
        {
            return Err(FragRefuse::Malformed);
        }
        if tick.saturating_add(MAX_TICK_LAG) < self.newest {
            return Err(FragRefuse::Stale);
        }
        if !self.by_tick.contains_key(&tick) {
            if self.by_tick.len() >= MAX_ASSEMBLIES {
                let oldest = *self.by_tick.keys().next().unwrap();
                if tick < oldest {
                    return Err(FragRefuse::Stale);
                }
                self.by_tick.remove(&oldest);
                self.stats.evicted += 1;
            }
            self.by_tick.insert(
                tick,
                Assembly {
                    first: now,
                    groups: (0..groups).map(|_| None).collect(),
                    delivered: 0,
                },
            );
        }
        self.newest = self.newest.max(tick);
        let asm = self.by_tick.get_mut(&tick).unwrap();
        if asm.groups.len() != groups {
            return Err(FragRefuse::Inconsistent);
        }
        let slot = asm.groups[group].get_or_insert_with(|| GroupAsm {
            parts: vec![None; count],
            got: 0,
            done: false,
        });
        if slot.parts.len() != count {
            return Err(FragRefuse::Inconsistent);
        }
        if slot.done || slot.parts[index].is_some() {
            return Err(FragRefuse::Duplicate);
        }
        slot.parts[index] = Some(payload[FRAG_HEADER..].to_vec());
        slot.got += 1;
        if slot.got < count {
            return Ok(None);
        }
        slot.done = true;
        let data: Vec<u8> = slot
            .parts
            .iter_mut()
            .flat_map(|p| p.take().unwrap())
            .collect();
        asm.delivered += 1;
        self.stats.groups_completed += 1;
        let tick_done = asm.delivered == groups;
        if tick_done {
            self.stats.ticks_completed += 1;
            let wait = now.saturating_duration_since(asm.first);
            self.stats.max_complete_wait = self.stats.max_complete_wait.max(wait);
            // Keep nothing for a finished tick except a stale-guard: a
            // late duplicate of it is refused by `newest`/lag or starts
            // a fresh assembly that expires unused.
            self.by_tick.remove(&tick);
        }
        Ok(Some(Completed {
            tick,
            group: group as u8,
            groups: groups as u8,
            data,
            tick_done,
        }))
    }
}

/// Encode an acknowledgement of the state of logical `group` at
/// (`epoch`, `tick`).
pub fn encode_ack(epoch: u8, group: u8, tick: u64) -> Vec<u8> {
    let mut p = vec![epoch, group];
    p.extend_from_slice(&tick.to_le_bytes());
    p
}

/// Parse an acknowledgement payload into `(epoch, group, tick)`. A group
/// at or past [`MAX_GROUPS`] is malformed.
pub fn decode_ack(p: &[u8]) -> Option<(u8, u8, u64)> {
    (p.len() == 10 && usize::from(p[1]) < MAX_GROUPS)
        .then(|| (p[0], p[1], u64::from_le_bytes(p[2..10].try_into().unwrap())))
}

/// The first byte of a fragment group that carries a baselined state
/// ([`encode_state_frame`]) rather than a bare encoded message. Message
/// tags are all below `0x80`, so the two never collide.
pub const STATE_FRAME: u8 = 0xFE;

/// Wrap a [`BaselineSender::encode`] payload for logical `group` of the
/// reset `epoch` as the data of a fragment group.
pub fn encode_state_frame(group: u8, epoch: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![STATE_FRAME, group, epoch];
    f.extend_from_slice(payload);
    f
}

/// Parse a state frame into `(group, epoch, payload)`; `None` when `data`
/// is not one.
pub fn decode_state_frame(data: &[u8]) -> Option<(u8, u8, &[u8])> {
    match data {
        [STATE_FRAME, group, epoch, payload @ ..] if usize::from(*group) < MAX_GROUPS => {
            Some((*group, *epoch, payload))
        }
        _ => None,
    }
}

/// Why a state or delta was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    /// Short, unknown kind, or a delta that does not fit its base.
    #[error("malformed state or delta")]
    Malformed,
    /// A delta against a baseline the receiver does not hold (never
    /// received, evicted, from another epoch, or never acknowledged).
    #[error("delta against missing baseline at tick {0}")]
    MissingBase(u64),
    /// An older generation, or a state not newer than the newest held.
    #[error("stale state")]
    Stale,
}

/// Lossless byte delta of `new` against an equal-length `base`: the new
/// length, then `(gap u16, run u16, bytes)` for each changed run.
/// `None` when the lengths differ or the state is oversize.
pub fn delta_encode(base: &[u8], new: &[u8]) -> Option<Vec<u8>> {
    if base.len() != new.len() || new.len() > MAX_STATE {
        return None;
    }
    let mut out = (new.len() as u32).to_le_bytes().to_vec();
    let (mut i, mut last_end) = (0, 0);
    while i < new.len() {
        if base[i] == new[i] {
            i += 1;
            continue;
        }
        // Extend the run across short equal stretches: cheaper than a
        // new 4-byte record.
        let start = i;
        let mut end = i + 1;
        let mut j = end;
        while j < new.len() && j - end < 4 {
            if base[j] != new[j] {
                end = j + 1;
            }
            j += 1;
        }
        i = end;
        let (mut s, e) = (start, end);
        while s < e {
            let run = (e - s).min(u16::MAX as usize);
            out.extend_from_slice(&((s - last_end) as u16).to_le_bytes());
            out.extend_from_slice(&(run as u16).to_le_bytes());
            out.extend_from_slice(&new[s..s + run]);
            s += run;
            last_end = s;
        }
    }
    Some(out)
}

/// Rebuild the new state from `base` and a [`delta_encode`] result.
pub fn delta_apply(base: &[u8], delta: &[u8]) -> Result<Vec<u8>, StateError> {
    if delta.len() < 4 {
        return Err(StateError::Malformed);
    }
    let len = u32::from_le_bytes(delta[..4].try_into().unwrap()) as usize;
    if len != base.len() || len > MAX_STATE {
        return Err(StateError::Malformed);
    }
    let mut out = base.to_vec();
    let (mut at, mut pos) = (4, 0usize);
    while at < delta.len() {
        if at + 4 > delta.len() {
            return Err(StateError::Malformed);
        }
        let gap = u16::from_le_bytes([delta[at], delta[at + 1]]) as usize;
        let run = u16::from_le_bytes([delta[at + 2], delta[at + 3]]) as usize;
        at += 4;
        pos += gap;
        if at + run > delta.len() || pos + run > len {
            return Err(StateError::Malformed);
        }
        out[pos..pos + run].copy_from_slice(&delta[at..at + run]);
        pos += run;
        at += run;
    }
    Ok(out)
}

/// The identity a baseline is only valid within.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Scope {
    generation: u64,
    epoch: u8,
}

/// What [`BaselineSender::encode`] chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// A complete state: no usable acknowledged baseline.
    Full,
    /// A delta against the acknowledged baseline at this tick.
    Delta(u64),
}

/// The sending side: remembers recent states and which one the receiver
/// has acknowledged.
#[derive(Debug, Default)]
pub struct BaselineSender {
    scope: Scope,
    held: VecDeque<(u64, Vec<u8>)>,
    acked: Option<u64>,
}

impl BaselineSender {
    /// An empty sender.
    pub fn new() -> Self {
        Self::default()
    }

    fn enter(&mut self, generation: u64, epoch: u8) {
        let scope = Scope { generation, epoch };
        if scope != self.scope {
            self.scope = scope;
            self.held.clear();
            self.acked = None;
        }
    }

    /// Record the receiver's acknowledgement. Ignored unless it names the
    /// current generation and epoch, a tick still held, and is newer than
    /// the last one.
    pub fn ack(&mut self, generation: u64, epoch: u8, tick: u64) -> bool {
        if (Scope { generation, epoch }) != self.scope
            || !self.held.iter().any(|(t, _)| *t == tick)
            || self.acked.is_some_and(|a| a >= tick)
        {
            return false;
        }
        self.acked = Some(tick);
        true
    }

    /// The acknowledged baseline tick, if still held.
    pub fn acked(&self) -> Option<u64> {
        self.acked
    }

    /// Encode the state `new` at `tick`: a delta against the newest
    /// acknowledged baseline when one is held, in this scope, no more than
    /// [`REFRESH_TICKS`] behind and the delta is smaller; otherwise a
    /// full state. The state is then held as a future baseline. Returns
    /// the payload and the plan.
    pub fn encode(&mut self, generation: u64, epoch: u8, tick: u64, new: &[u8]) -> (Vec<u8>, Plan) {
        self.enter(generation, epoch);
        let delta = self
            .acked
            .filter(|a| *a < tick && tick - *a <= REFRESH_TICKS)
            .and_then(|a| self.held.iter().find(|(t, _)| *t == a))
            .and_then(|(t, base)| delta_encode(base, new).map(|d| (*t, d)))
            .filter(|(_, d)| d.len() < new.len());
        let (mut payload, plan) = match delta {
            Some((base, d)) => {
                let mut p = vec![KIND_DELTA];
                p.extend_from_slice(&base.to_le_bytes());
                p.extend_from_slice(&d);
                (p, Plan::Delta(base))
            }
            None => (vec![KIND_FULL, 0, 0, 0, 0, 0, 0, 0, 0], Plan::Full),
        };
        if plan == Plan::Full {
            payload.extend_from_slice(new);
        }
        self.held.push_back((tick, new.to_vec()));
        while self.held.len() > BASELINE_CAP {
            // Never evict the acknowledged baseline while a newer
            // unacknowledged one can go instead.
            let drop_at = match self.acked {
                Some(a) if self.held.front().is_some_and(|(t, _)| *t == a) => 1,
                _ => 0,
            };
            self.held.remove(drop_at);
        }
        (payload, plan)
    }
}

/// The receiving side: applies full states and deltas, and only deltas
/// whose base it still holds.
#[derive(Debug, Default)]
pub struct BaselineReceiver {
    scope: Scope,
    held: VecDeque<(u64, Vec<u8>)>,
    /// The newest tick applied in the current generation, across epochs:
    /// an older state of an earlier epoch that overtook the epoch change on
    /// the wire must not move the scope back, or every delta of the new
    /// epoch would be refused `MissingBase` until the sender's refresh.
    newest: Option<u64>,
}

impl BaselineReceiver {
    /// An empty receiver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the state at `tick` is held in the current scope.
    pub fn holds(&self, tick: u64) -> bool {
        self.held.iter().any(|(t, _)| *t == tick)
    }

    /// Apply a state payload for (`generation`, `epoch`, `tick`) and
    /// return the reconstructed state. The caller acknowledges the tick
    /// only after this succeeds.
    pub fn apply(
        &mut self,
        generation: u64,
        epoch: u8,
        tick: u64,
        payload: &[u8],
    ) -> Result<Vec<u8>, StateError> {
        if generation < self.scope.generation {
            return Err(StateError::Stale);
        }
        if payload.len() < STATE_HEADER {
            return Err(StateError::Malformed);
        }
        let same = (Scope { generation, epoch }) == self.scope;
        if generation == self.scope.generation && self.newest.is_some_and(|n| tick <= n) {
            return Err(StateError::Stale);
        }
        let base_tick = u64::from_le_bytes(payload[1..9].try_into().unwrap());
        let body = &payload[STATE_HEADER..];
        let state = match payload[0] {
            KIND_FULL if body.len() <= MAX_STATE => body.to_vec(),
            KIND_DELTA => {
                let base = same
                    .then(|| self.held.iter().find(|(t, _)| *t == base_tick))
                    .flatten()
                    .ok_or(StateError::MissingBase(base_tick))?;
                delta_apply(&base.1, body)?
            }
            _ => return Err(StateError::Malformed),
        };
        if !same {
            self.scope = Scope { generation, epoch };
            self.held.clear();
        }
        self.newest = Some(tick);
        self.held.push_back((tick, state.clone()));
        while self.held.len() > BASELINE_CAP {
            self.held.pop_front();
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    fn blob(n: usize, seed: u8) -> Vec<u8> {
        (0..n)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    #[test]
    fn fragments_fit_the_datagram_and_reassemble_in_any_order() {
        let (a, b) = (blob(3000, 1), blob(10, 2));
        let mut frags = split(7, &[&a, &b]).unwrap();
        assert!(frags.iter().all(|f| f.len() <= MAX_PAYLOAD));
        frags.reverse();
        let mut asm = Assembler::new();
        let t = now();
        let mut got = Vec::new();
        for f in &frags {
            if let Some(c) = asm.accept(f, t).unwrap() {
                got.push(c);
            }
        }
        got.sort_by_key(|c| c.group);
        assert_eq!((got[0].data.clone(), got[1].data.clone()), (a, b));
        assert!(got.iter().any(|c| c.tick_done));
        assert_eq!(asm.in_flight(), 0);
        assert_eq!(asm.stats().ticks_completed, 1);
    }

    #[test]
    fn a_lost_fragment_withholds_only_its_own_group() {
        let (owner, world) = (blob(200, 1), blob(3000, 2));
        let frags = split(9, &[&owner, &world]).unwrap();
        let mut asm = Assembler::new();
        let t = now();
        let mut delivered = Vec::new();
        // Drop the world group's second fragment.
        for (i, f) in frags.iter().enumerate() {
            if i == 2 {
                continue;
            }
            if let Some(c) = asm.accept(f, t).unwrap() {
                delivered.push(c);
            }
        }
        assert_eq!(delivered.len(), 1);
        assert_eq!((delivered[0].group, &delivered[0].data), (0, &owner));
        assert!(!delivered[0].tick_done);
        assert_eq!(
            asm.in_flight(),
            1,
            "the partial world group is held, bounded"
        );
    }

    #[test]
    fn partial_assemblies_expire_and_never_block_newer_ticks() {
        let mut asm = Assembler::new();
        let t0 = now();
        let old = split(1, &[&blob(3000, 1)]).unwrap();
        asm.accept(&old[0], t0).unwrap();
        let later = t0 + FRAGMENT_TTL + Duration::from_millis(1);
        let new = split(2, &[&blob(50, 2)]).unwrap();
        let c = asm.accept(&new[0], later).unwrap().unwrap();
        assert_eq!(c.tick, 2);
        assert_eq!(asm.stats().expired, 1);
        assert_eq!(asm.in_flight(), 0);
        // The rest of the expired tick starts a fresh assembly that
        // can still never complete without its first fragment.
        assert_eq!(asm.accept(&old[1], later).unwrap(), None);
    }

    #[test]
    fn interleaved_out_of_order_ticks_with_losses_deliver_only_what_is_whole() {
        let mut asm = Assembler::new();
        let t = now();
        let data: Vec<Vec<u8>> = (1..=3).map(|k| blob(3000, k)).collect();
        let mut frags: Vec<(u64, Vec<u8>)> = Vec::new();
        for (k, d) in data.iter().enumerate() {
            let tick = 10 + k as u64;
            for f in split(tick, &[d]).unwrap() {
                frags.push((tick, f));
            }
        }
        // Tick 11 loses its middle fragment; the rest arrives reversed
        // and interleaved across ticks.
        frags.retain(|(tick, f)| !(*tick == 11 && f[10] == 1));
        frags.reverse();
        let mut done = Vec::new();
        for (_, f) in &frags {
            if let Some(c) = asm.accept(f, t).unwrap() {
                done.push((c.tick, c.data));
            }
        }
        done.sort();
        assert_eq!(done, vec![(10, data[0].clone()), (12, data[2].clone())]);
        assert_eq!(asm.in_flight(), 1, "only the broken tick is still held");
        assert_eq!(asm.expire(t + FRAGMENT_TTL), 1);
        assert_eq!(asm.in_flight(), 0);
    }

    /// A path that drops every datagram over a size threshold: with the
    /// threshold under a full fragment, no multi-fragment group ever
    /// completes, and the assembler stays bounded and counts the expiry
    /// instead of holding the leftovers.
    #[test]
    fn a_size_threshold_that_eats_full_fragments_starves_assembly_boundedly() {
        let threshold = FRAG_HEADER + FRAG_DATA - 1;
        let mut asm = Assembler::new();
        let mut t = now();
        let big = blob(3 * FRAG_DATA - 5, 9);
        let small = blob(100, 4);
        let mut small_done = 0;
        for tick in 1..=200u64 {
            // Group 0 is the big one, group 1 the one-datagram one.
            for f in split(tick, &[&big, &small]).unwrap() {
                if f.len() > threshold {
                    continue;
                }
                small_done += asm.accept(&f, t).unwrap().is_some_and(|c| c.group == 1) as u64;
            }
            assert!(asm.in_flight() <= MAX_ASSEMBLIES);
            t += Duration::from_millis(16);
        }
        let st = asm.stats();
        assert_eq!(small_done, 200, "the one-datagram group is unaffected");
        assert_eq!(st.groups_completed, 200, "the big group never completes");
        assert_eq!(st.ticks_completed, 0);
        assert!(asm.in_flight() <= MAX_ASSEMBLIES);
        // 200 ticks x 16 ms outlives the TTL many times over: held
        // leftovers were expired or evicted, never accumulated.
        assert!(st.expired + st.evicted >= 200 - MAX_ASSEMBLIES as u64);
    }

    #[test]
    fn assembly_is_bounded_and_newer_ticks_evict_the_oldest() {
        let mut asm = Assembler::new();
        let t = now();
        for tick in 1..=(MAX_ASSEMBLIES as u64 + 3) {
            let f = split(tick, &[&blob(3000, 1)]).unwrap();
            asm.accept(&f[0], t).unwrap();
            assert!(asm.in_flight() <= MAX_ASSEMBLIES);
        }
        assert_eq!(asm.stats().evicted, 3);
        let old = split(1, &[&blob(3000, 1)]).unwrap();
        assert_eq!(asm.accept(&old[0], t), Err(FragRefuse::Stale));
    }

    #[test]
    fn hostile_fragments_are_refused() {
        let mut asm = Assembler::new();
        let t = now();
        let f = split(5, &[&blob(3000, 1), &blob(3000, 2)]).unwrap();
        asm.accept(&f[0], t).unwrap();
        assert_eq!(asm.accept(&f[0], t), Err(FragRefuse::Duplicate));
        let mut wrong_groups = f[1].clone();
        wrong_groups[9] = 3;
        assert_eq!(asm.accept(&wrong_groups, t), Err(FragRefuse::Inconsistent));
        let mut wrong_count = f[1].clone();
        wrong_count[11] = 7;
        assert_eq!(asm.accept(&wrong_count, t), Err(FragRefuse::Inconsistent));
        for bad in [&f[0][..5], &[0u8; FRAG_HEADER][..]] {
            assert_eq!(asm.accept(bad, t), Err(FragRefuse::Malformed));
        }
        let mut oversize = f[0].clone();
        oversize[11] = (MAX_FRAGMENTS + 1) as u8;
        assert_eq!(asm.accept(&oversize, t), Err(FragRefuse::Malformed));
        assert_eq!(
            split(1, &[&vec![0; FRAG_DATA * MAX_FRAGMENTS + 1]]),
            Err(SplitError::GroupTooLarge(0))
        );
        assert_eq!(split(1, &[]), Err(SplitError::Groups));
        let mut asm = Assembler::new();
        asm.accept(&split(100, &[b"x"]).unwrap()[0], t).unwrap();
        assert_eq!(
            asm.accept(&split(100 - MAX_TICK_LAG - 1, &[b"x"]).unwrap()[0], t),
            Err(FragRefuse::Stale)
        );
    }

    #[test]
    fn delta_is_lossless_and_rejects_bad_input() {
        let base = blob(500, 1);
        let mut new = base.clone();
        new[3] ^= 0xff;
        new[4] ^= 0xff;
        new[400] = 9;
        let d = delta_encode(&base, &new).unwrap();
        assert!(d.len() < new.len() / 4);
        assert_eq!(delta_apply(&base, &d).unwrap(), new);
        assert_eq!(
            delta_apply(&base, &delta_encode(&base, &base).unwrap()).unwrap(),
            base
        );
        assert!(delta_encode(&base, &new[1..]).is_none());
        assert_eq!(delta_apply(&base[1..], &d), Err(StateError::Malformed));
        assert_eq!(
            delta_apply(&base, &d[..d.len() - 1]),
            Err(StateError::Malformed)
        );
        let mut over = d.clone();
        over[4] = 0xff;
        over[5] = 0xff;
        assert_eq!(delta_apply(&base, &over), Err(StateError::Malformed));
        let all = blob(500, 77);
        assert_eq!(
            delta_apply(&base, &delta_encode(&base, &all).unwrap()).unwrap(),
            all
        );
    }

    fn drive(tx: &mut BaselineSender, rx: &mut BaselineReceiver, tick: u64, s: &[u8]) -> Plan {
        let (p, plan) = tx.encode(1, 0, tick, s);
        assert_eq!(rx.apply(1, 0, tick, &p).unwrap(), s);
        plan
    }

    #[test]
    fn delta_only_after_an_ack_and_full_otherwise() {
        let (mut tx, mut rx) = (BaselineSender::new(), BaselineReceiver::new());
        let mut s = blob(400, 1);
        assert_eq!(drive(&mut tx, &mut rx, 1, &s), Plan::Full);
        s[10] = 200;
        assert_eq!(drive(&mut tx, &mut rx, 2, &s), Plan::Full, "tick 1 unacked");
        assert!(tx.ack(1, 0, 2));
        s[11] = 201;
        assert_eq!(drive(&mut tx, &mut rx, 3, &s), Plan::Delta(2));
        assert!(!tx.ack(1, 0, 2), "not newer");
        assert!(!tx.ack(1, 0, 99), "never sent");
        assert!(!tx.ack(1, 1, 3), "other epoch");
        assert!(!tx.ack(2, 0, 3), "other generation");
    }

    #[test]
    fn a_stale_or_evicted_base_is_rejected_safely() {
        let (mut tx, mut rx) = (BaselineSender::new(), BaselineReceiver::new());
        let mut s = blob(400, 1);
        drive(&mut tx, &mut rx, 1, &s);
        tx.ack(1, 0, 1);
        s[5] = 1;
        let (delta, plan) = tx.encode(1, 0, 2, &s);
        assert_eq!(plan, Plan::Delta(1));
        // A receiver that never saw tick 1 (its full was lost).
        let mut fresh = BaselineReceiver::new();
        assert_eq!(
            fresh.apply(1, 0, 2, &delta),
            Err(StateError::MissingBase(1))
        );
        assert!(!fresh.holds(2), "a rejected delta stores nothing");
        // A receiver whose base has been evicted.
        for t in 3..3 + BASELINE_CAP as u64 {
            s[6] = t as u8;
            let (p, _) = tx.encode(1, 0, t, &s);
            rx.apply(1, 0, t, &p).unwrap();
        }
        assert!(!rx.holds(1));
        let later = 3 + BASELINE_CAP as u64 + 10;
        assert_eq!(
            rx.apply(1, 0, later, &delta),
            Err(StateError::MissingBase(1))
        );
        // Epoch and generation boundaries.
        let full = |tick| BaselineSender::new().encode(1, 0, tick, &s).0;
        let mut other = BaselineReceiver::new();
        other.apply(1, 0, 30, &full(30)).unwrap();
        assert!(matches!(
            other.apply(1, 1, 31, &delta),
            Err(StateError::MissingBase(_))
        ));
        assert_eq!(other.apply(0, 0, 40, &full(40)), Err(StateError::Stale));
        // A replayed old state is stale, not applied.
        assert_eq!(
            rx.apply(1, 0, 4, &tx.encode(1, 0, 4, &s).0),
            Err(StateError::Stale)
        );
    }

    /// A state of the previous epoch that overtakes the epoch change on the
    /// wire is stale: it neither applies nor moves the scope back, so the
    /// new epoch's deltas still find their base.
    #[test]
    fn an_old_epoch_state_arriving_late_does_not_move_the_scope_back() {
        let (mut tx, mut rx) = (BaselineSender::new(), BaselineReceiver::new());
        let mut s = blob(400, 1);
        let old = tx.encode(1, 4, 10, &s).0;
        s[3] = 9;
        let new_full = tx.encode(1, 5, 11, &s).0;
        rx.apply(1, 5, 11, &new_full).unwrap();
        assert!(tx.ack(1, 5, 11));
        assert_eq!(rx.apply(1, 4, 10, &old), Err(StateError::Stale));
        s[4] = 7;
        let (delta, plan) = tx.encode(1, 5, 12, &s);
        assert_eq!(plan, Plan::Delta(11));
        assert_eq!(rx.apply(1, 5, 12, &delta).unwrap(), s);
        // A newer generation may restart the tick count.
        let restart = BaselineSender::new().encode(2, 0, 1, &s).0;
        assert_eq!(rx.apply(2, 0, 1, &restart).unwrap(), s);
    }

    #[test]
    fn epoch_change_drops_baselines_and_forces_a_full() {
        let (mut tx, mut rx) = (BaselineSender::new(), BaselineReceiver::new());
        let s = blob(300, 3);
        drive(&mut tx, &mut rx, 1, &s);
        tx.ack(1, 0, 1);
        let (p, plan) = tx.encode(1, 1, 2, &s);
        assert_eq!(plan, Plan::Full);
        assert_eq!(tx.acked(), None);
        assert_eq!(rx.apply(1, 1, 2, &p).unwrap(), s);
        assert!(!rx.holds(1));
    }

    #[test]
    fn a_baseline_past_the_refresh_bound_is_replaced_by_a_full() {
        let (mut tx, mut rx) = (BaselineSender::new(), BaselineReceiver::new());
        let mut s = blob(400, 1);
        drive(&mut tx, &mut rx, 1, &s);
        tx.ack(1, 0, 1);
        s[0] = 9;
        assert_eq!(tx.encode(1, 0, 1 + REFRESH_TICKS, &s).1, Plan::Delta(1));
        assert_eq!(tx.encode(1, 0, 2 + REFRESH_TICKS, &s).1, Plan::Full);
    }

    #[test]
    fn the_acked_baseline_survives_eviction_pressure() {
        let mut tx = BaselineSender::new();
        let s = blob(100, 1);
        tx.encode(1, 0, 1, &s);
        tx.ack(1, 0, 1);
        for t in 2..2 + 3 * BASELINE_CAP as u64 {
            tx.encode(1, 0, t, &s);
            assert!(tx.held.len() <= BASELINE_CAP);
        }
        assert_eq!(tx.acked(), Some(1));
        assert!(tx.held.iter().any(|(t, _)| *t == 1));
    }

    /// A seeded xorshift so the loss pattern is reproducible.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn eight_seat_group(tick: u64, first: u16, n: usize, impacts: usize) -> Vec<u8> {
        use crate::proto::{Message, SnapEntry};
        let t = tick as f32;
        Message::Snap {
            generation: 1,
            tick,
            entries: (0..n)
                .map(|i| {
                    let k = (first as usize + i) as f32;
                    SnapEntry {
                        player: first + i as u16,
                        pos: [t * 0.3 + k, 0.5, t * 0.1 - k],
                        rot: [0.0, (t * 0.01 + k).sin(), 0.0, (t * 0.01 + k).cos()],
                        vel: [18.0 + (t * 0.05 + k).sin(), 0.0, 6.0],
                        angvel: [0.0, 0.02 * (t * 0.05).sin(), 0.0],
                        prog_ticks: tick,
                        input_ack: tick,
                        rpm: (3000.0 + 800.0 * (t * 0.07 + k).sin()) as u16,
                        ..SnapEntry::default()
                    }
                })
                .collect(),
            trailers: Vec::new(),
            impacts: (0..impacts)
                .map(|i| crate::proto::SnapImpact {
                    seat: 1 + (i % 7) as u16,
                    id: tick * 64 + i as u64,
                    tick,
                    epoch: 0,
                    ordinal: 0,
                    point: [t * 0.3 + i as f32, 0.5, t * 0.1],
                    normal: [0.0, 1.0, 0.0],
                    severity: 1.0 + (t + i as f32).sin(),
                    audio_id: 0,
                })
                .collect(),
            race: None,
        }
        .encode()
        .unwrap()
    }

    #[derive(Default, Debug)]
    struct Run {
        ticks: u64,
        owner_applied: u64,
        world_applied: u64,
        both_applied: u64,
        datagrams: u64,
        owner_frags: u64,
        owner_full: u64,
        owner_delta: u64,
        owner_bytes: u64,
        world_bytes: u64,
        rejected_base: u64,
        expired: u64,
    }

    /// 60 Hz, eight seats: the owner's car (and contacts) is group 0,
    /// the other seven cars group 1. Loss hits every fragment and every
    /// ack independently; an ack takes `RTT_TICKS` to come back.
    fn simulate(loss: f64, seed: u64, impacts: usize) -> Run {
        const RTT_TICKS: u64 = 4;
        const TICKS: u64 = 3600;
        let mut rng = Rng(seed);
        let mut tx = [BaselineSender::new(), BaselineSender::new()];
        let mut rx = [BaselineReceiver::new(), BaselineReceiver::new()];
        let mut asm = Assembler::new();
        let t0 = now();
        let mut acks: Vec<(u64, usize, u64)> = Vec::new();
        let mut run = Run::default();
        for tick in 1..=TICKS {
            let at = t0 + Duration::from_micros(tick * 16_667);
            acks.retain(|&(due, g, t)| {
                if due <= tick {
                    tx[g].ack(1, 0, t);
                    false
                } else {
                    true
                }
            });
            let states = [
                eight_seat_group(tick, 0, 1, 0),
                eight_seat_group(tick, 1, 7, impacts),
            ];
            let mut payloads = Vec::new();
            for g in 0..2 {
                let (p, plan) = tx[g].encode(1, 0, tick, &states[g]);
                if g == 0 {
                    run.owner_bytes += p.len() as u64;
                    match plan {
                        Plan::Full => run.owner_full += 1,
                        Plan::Delta(_) => run.owner_delta += 1,
                    }
                } else {
                    run.world_bytes += p.len() as u64;
                }
                payloads.push(p);
            }
            let slices: Vec<&[u8]> = payloads.iter().map(Vec::as_slice).collect();
            let frags = split(tick, &slices).unwrap();
            run.ticks += 1;
            let (mut owner, mut world) = (false, false);
            for f in &frags {
                run.datagrams += 1;
                if f[8] == 0 {
                    run.owner_frags += 1;
                }
                if rng.next() < loss {
                    continue;
                }
                if let Ok(Some(c)) = asm.accept(f, at) {
                    let g = c.group as usize;
                    match rx[g].apply(1, 0, c.tick, &c.data) {
                        Ok(state) => {
                            if c.tick == tick {
                                assert_eq!(state, states[g], "lossless reconstruction");
                                if g == 0 { owner = true } else { world = true }
                            }
                            if rng.next() >= loss {
                                acks.push((tick + RTT_TICKS, g, c.tick));
                            }
                        }
                        Err(StateError::MissingBase(_)) => run.rejected_base += 1,
                        Err(_) => {}
                    }
                }
            }
            run.owner_applied += owner as u64;
            run.world_applied += world as u64;
            run.both_applied += (owner && world) as u64;
        }
        run.expired = asm.stats().expired + asm.stats().evicted;
        run
    }

    /// One state per tick over a slow, lossy ack path: a delta is never
    /// refused for a missing base, and deltas are actually used.
    #[test]
    fn a_slow_lossy_ack_path_never_strands_a_delta() {
        for (rtt, ack_loss) in [(4u64, 0.9), (30, 0.4), (55, 0.0)] {
            let mut rng = Rng(0x1234_5678_9abc_def0 ^ rtt);
            let (mut tx, mut rx) = (BaselineSender::new(), BaselineReceiver::new());
            let mut acks: Vec<(u64, u64)> = Vec::new();
            let (mut deltas, mut fulls, mut missing) = (0u64, 0u64, 0u64);
            let mut s = blob(600, 3);
            for tick in 1..=1200u64 {
                acks.retain(|&(due, t)| {
                    if due <= tick {
                        tx.ack(1, 0, t);
                        false
                    } else {
                        true
                    }
                });
                s[(tick % 600) as usize] = tick as u8;
                let (payload, plan) = tx.encode(1, 0, tick, &s);
                match plan {
                    Plan::Full => fulls += 1,
                    Plan::Delta(_) => deltas += 1,
                }
                if rng.next() < 0.1 {
                    continue;
                }
                match rx.apply(1, 0, tick, &payload) {
                    Ok(state) => {
                        assert_eq!(state, s);
                        if rng.next() >= ack_loss {
                            acks.push((tick + rtt, tick));
                        }
                    }
                    Err(StateError::MissingBase(_)) => missing += 1,
                    Err(e) => panic!("{e:?}"),
                }
            }
            assert_eq!(missing, 0, "rtt {rtt} ack_loss {ack_loss}");
            assert!(deltas > fulls, "rtt {rtt}: {deltas} deltas, {fulls} full");
        }
    }

    #[test]
    fn eight_seat_correction_cadence_under_loss() {
        let mut quiet = Vec::new();
        for impacts in [0usize, 64] {
            let owner_bytes = eight_seat_group(1, 0, 1, 0).len();
            let world_bytes = eight_seat_group(1, 1, 7, impacts).len();
            println!(
                "world carries {impacts} impacts: full owner group {owner_bytes} B ({} fragment), \
                 full world group {world_bytes} B ({} fragments)",
                owner_bytes.div_ceil(FRAG_DATA),
                world_bytes.div_ceil(FRAG_DATA)
            );
            println!(
                "loss  owner_Hz world_Hz whole_Hz dgrams/tick owner_B/tick world_B/tick delta% base_rej expired"
            );
            let mut hz = Vec::new();
            for loss in [0.0, 0.01, 0.05, 0.10, 0.20] {
                let r = simulate(loss, 0x9e37_79b9_7f4a_7c15, impacts);
                let f = |n: u64| n as f64 * 60.0 / r.ticks as f64;
                println!(
                    "{:>3.0}% {:>8.1} {:>8.1} {:>8.1} {:>10.2} {:>12.1} {:>12.1} {:>5.0}% {:>8} {:>7}",
                    loss * 100.0,
                    f(r.owner_applied),
                    f(r.world_applied),
                    f(r.both_applied),
                    r.datagrams as f64 / r.ticks as f64,
                    r.owner_bytes as f64 / r.ticks as f64,
                    r.world_bytes as f64 / r.ticks as f64,
                    100.0 * r.owner_delta as f64 / r.ticks as f64,
                    r.rejected_base,
                    r.expired,
                );
                hz.push((f(r.owner_applied), f(r.world_applied), f(r.both_applied)));
            }
            assert_eq!(hz[0], (60.0, 60.0, 60.0), "lossless is every tick");
            // The owner's one-fragment group is never held up by the
            // world's loss: its cadence tracks (1 - loss) and is at least
            // the whole tick's.
            for (&(owner, _, both), loss) in hz.iter().zip([0.0, 0.01, 0.05, 0.10, 0.20]) {
                assert!(owner >= both, "loss {loss}");
                assert!(
                    owner >= 60.0 * (1.0 - loss) - 4.0,
                    "loss {loss}: owner {owner}"
                );
            }
            assert!(hz[3].0 > hz[3].2, "at 10% loss groups beat whole ticks");
            quiet.push(hz);
        }
    }

    #[test]
    fn ack_roundtrips() {
        assert_eq!(decode_ack(&encode_ack(3, 2, 77)), Some((3, 2, 77)));
        assert_eq!(decode_ack(&encode_ack(3, MAX_GROUPS as u8, 77)), None);
        assert_eq!(decode_ack(&[1, 2]), None);
    }
}
