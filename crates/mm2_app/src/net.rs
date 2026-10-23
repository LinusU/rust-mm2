//! The `mm2_net` bridge (F24-B): `SessionConfig` ↔ the lobby's session
//! advertisement, plus the Bevy-side lobby surfaces — [`LobbyLink`]
//! wraps an `mm2_net::Client` on a pump thread and [`drive_lobby`]
//! feeds its events (including `Start` → `Session::begin_generation`)
//! into app state; [`HostLink`] owns an in-app `mm2_net::Host` and
//! [`drive_host`] plays the host seat through the same lifecycle.
//!
//! `mm2_net` stays project-free — the wire's [`SessionAdvertisement`]
//! carries a bounded opaque `params` blob whose layout this module owns.
//! The blob is JSON so a captured advertisement is readable; its version
//! is the protocol's — peers that handshake built the same
//! `PROTOCOL_VERSION` code, so a layout change ships with a version
//! bump, never silently.
//!
//! Deliberately absent from the advertisement — the joining side fills
//! these in itself:
//!
//! - `authority` — a peer that accepted an advertisement is always
//!   `SessionAuthority::Remote`; the host alone is authoritative;
//! - `vehicle` — the driver's car/paint pick is per-player roster
//!   state, not session config; it travels separately via
//!   [`Message::SetVehicle`](mm2_net::Message::SetVehicle) and this
//!   module's [`encode_pick`]/[`decode_pick`]/[`vehicle_validator`]
//!   helpers;
//! - `mods_active` / `mods_cosmetic_only` — whether *this* process
//!   mounted mods (and whether they were all cosmetic) is a local fact
//!   the session builder stamps on its own; a networked session is
//!   record-ineligible either way;
//! - `dev` — developer overrides are never network-legal, so
//!   [`advertise`] refuses a config carrying any rather than dropping
//!   them silently.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use mm2_assets::Vfs;
use mm2_content::cnr::{CnrSettings, GoldMass, MatchLimit};
use mm2_content::{EntryStatus, VehicleCatalog};
use mm2_game::gold::CnrVariant;
use mm2_game::{
    CUSTOMIZE_LAP_MAX, CheckpointRule, ConfigError, Densities, DevOverrides, Difficulty, EventRef,
    EventTableKind, Mm2Vfs, RaceCustomization, SelectorError, Session, SessionAuthority,
    SessionConditions, SessionConfig, SessionCustomization, SessionMode, SessionPhase, TimeOfDay,
    VehicleSelection, Weather, WorldMode,
};
use mm2_net::{
    Client, ClientCtl, Hello, Host, HostConfig, HostCtl, HostEvent, KEEPALIVE_INTERVAL, LateJoin,
    LeaveCause, MAX_PLAYERS, Message, NetError, PickValidator, RosterEntry, SessionAdvertisement,
    Transport, VehiclePick,
};
use serde::{Deserialize, Serialize};

use crate::menu::MenuShell;
use crate::race;
use crate::session::{SelectedCar, SessionControl, TunedVehicle};

/// A `SessionConfig` the advertisement could not carry, or a `params`
/// blob that could not be read back into one.
#[derive(Debug, thiserror::Error)]
pub enum SessionWireError {
    /// The advertised config carries `DevOverrides` — never
    /// network-legal, so they cannot ride a lobby advertisement.
    #[error("developer overrides cannot be advertised")]
    DevOverrides,
    /// The config fails `SessionConfig::validate` — on encode (the
    /// host's own config) or on decode (the received blob).
    #[error("invalid session config: {0}")]
    Invalid(#[from] ConfigError),
    /// A condition selector outside the authored 0-3 range.
    #[error("{0}")]
    Selector(#[from] SelectorError),
    /// The params blob is not the encoding this build produces.
    #[error("session params: {0}")]
    Params(#[from] serde_json::Error),
    /// A paint index the wire's `u8` field cannot carry.
    #[error("paint index {0} exceeds the wire's u8 bound")]
    Paint(usize),
}

/// Encode a session's configuration for the lobby to carry. The result
/// is opaque to `mm2_net` but complete: [`accept`] rebuilds the same
/// world/mode/difficulty/conditions/densities/customization/seed.
pub fn advertise(config: &SessionConfig) -> Result<SessionAdvertisement, SessionWireError> {
    if config.dev != DevOverrides::default() {
        return Err(SessionWireError::DevOverrides);
    }
    config.validate()?;
    Ok(SessionAdvertisement {
        summary: summarize(config),
        params: serde_json::to_vec(&SessionParams::from(config))?,
    })
}

/// Decode a received advertisement back into a `SessionConfig`, stamped
/// `authority: Remote` with `vehicle`/`mods_active`/`dev` at defaults —
/// the local session builder fills those in from its own state.
pub fn accept(ad: &SessionAdvertisement) -> Result<SessionConfig, SessionWireError> {
    let params: SessionParams = serde_json::from_slice(&ad.params)?;
    let config = params.into_config()?;
    config.validate()?;
    Ok(config)
}

/// Why a session a host advertised cannot run on *this* peer's mounted
/// content — the client-side half of the never-advertise-the-unrunnable
/// rule (F24-B.6). The handshake's gameplay fingerprint already proves
/// both peers resolve the same bytes; this is the defense-in-depth
/// against a host blob that disagrees with that shared content, plus the
/// range gaps the blob leaves latent until a session build would trip
/// on them.
#[derive(Debug, thiserror::Error)]
pub enum SessionContentError {
    /// `WorldMode::City` names a logical path nothing resolves to here.
    #[error("city world {0:?} does not resolve on this install")]
    World(String),
    /// The advertised event cannot be resolved into a runnable race by
    /// this install — the same [`race::event_race_setup`] gate the
    /// dedicated host runs at flag time (unknown row, missing records,
    /// an unbuildable definition, the Crash Course refusal).
    #[error("advertised event cannot run here: {0}")]
    Event(#[from] race::EventSetupError),
    /// A customized `laps` pick outside the designed picker range
    /// (`1..=CUSTOMIZE_LAP_MAX`; `laps == 0` is already refused by
    /// `SessionConfig::validate` inside [`accept`]). Consulted only on
    /// `Ordered` definitions — every other rule ignores the pick, so
    /// refusing one there would reject a session that runs identically.
    #[error("customized laps {0} exceeds the picker range (max {CUSTOMIZE_LAP_MAX})")]
    Laps(u32),
    /// A Cops & Robbers match on a city whose authored site pool
    /// (`multicopwaypoints.csv`) cannot seed a round — missing,
    /// unreadable or under the three sites a round draws. The same
    /// verdict `CnrHost::from_content` gives (`GoldError::PoolTooSmall`).
    #[error("city {city:?} has only {sites} Cops & Robbers sites (a round needs 3)")]
    CopsAndRobbers {
        /// The city stem the pool was looked up under.
        city: String,
        /// Usable sites found.
        sites: usize,
    },
    /// A customized `opponents` pick beyond the event's authored
    /// roster — the advertisement promises more opponents than the
    /// event's aimap can field.
    #[error("customized opponents {asked} exceeds the authored roster of {available}")]
    Opponents {
        /// The pick the advertisement carried.
        asked: u32,
        /// The event's authored roster size at the session difficulty.
        available: u32,
    },
}

/// Whether this peer's mounted content can run an advertised session —
/// the join-side complement of the flag-time gate `mm2-host` runs. A
/// host is trusted to gate itself; a joining peer is not obliged to
/// trust the host did. `SessionAdvertisement` is opaque to the wire
/// and `accept` only bounds the blob structurally, so the check that
/// the session's *content* is runnable here lives one layer up, where
/// the VFS is.
///
/// Deliberately *not* checked — anything `accept`'s decode +
/// `SessionConfig::validate` already bounds (selectors, densities,
/// zero laps), and picks the runtime itself ignores (any `race`
/// customization on a Cruise session, `laps` on a non-`Ordered`
/// event): refusing those would reject sessions that would have run
/// identically.
pub fn check_session(vfs: &Vfs, config: &SessionConfig) -> Result<(), SessionContentError> {
    if let WorldMode::City { psdl } = &config.world
        && vfs.resolve(psdl).is_none()
    {
        return Err(SessionContentError::World(psdl.clone()));
    }
    if let SessionMode::CopsAndRobbers(_) = &config.mode {
        // `validate` already required a city world; a path that is not
        // `city/<stem>.psdl` has no stem to look a pool up under.
        let city = match &config.world {
            WorldMode::City { psdl } => city_stem(psdl).unwrap_or_default().to_string(),
            WorldMode::DevWorld => String::new(),
        };
        let sites = mm2_content::CnrContent::load(vfs, &city).sites.len();
        if sites < 3 {
            return Err(SessionContentError::CopsAndRobbers { city, sites });
        }
    }
    if let SessionMode::Event(event_ref) = &config.mode {
        let setup = race::event_race_setup(vfs, event_ref, config.difficulty)?;
        if let Some(pick) = config.customization.as_ref().and_then(|c| c.race) {
            if setup.definition.rule == CheckpointRule::Ordered && pick.laps > CUSTOMIZE_LAP_MAX {
                return Err(SessionContentError::Laps(pick.laps));
            }
            let wired = setup.roster.entries.len() as u32;
            if pick.opponents > wired {
                return Err(SessionContentError::Opponents {
                    asked: pick.opponents,
                    available: wired,
                });
            }
        }
    }
    Ok(())
}

/// `VehicleSelection` → the wire pick the lobby carries: `id: None`
/// (the synthetic dev car) travels as the empty string — a real id is
/// never empty (`ConfigError::EmptyVehicleId` guards that), so `""` is
/// unambiguous — and `paint` must fit the wire's `u8` field.
pub fn encode_pick(selection: &VehicleSelection) -> Result<VehiclePick, SessionWireError> {
    Ok(VehiclePick {
        vehicle: selection.id.clone().unwrap_or_default(),
        paint: u8::try_from(selection.paint)
            .map_err(|_| SessionWireError::Paint(selection.paint))?,
    })
}

/// The car this process last selected, as the pick it offers the host:
/// a catalog id with its paint, or the dev car when none is loaded.
fn offered_selection(selected: &SelectedCar) -> VehicleSelection {
    VehicleSelection {
        id: selected.def.as_ref().map(|d| d.id.clone()),
        paint: selected.paint,
    }
}

/// The car a `Start` seats us in: what the host last confirmed on our
/// roster entry, else `offered` — the pick we sent on joining. A late
/// joiner's `Start` can outrun the echo of that pick (the host starts
/// it the moment it is admitted), and the dev car would not match the
/// host's copy of the seat: its wheelbase and ride height differ, so
/// the host's pose, asserted over the predicted car, buries the wheels.
fn start_pick(
    roster: &[mm2_net::RosterEntry],
    me: u16,
    offered: VehicleSelection,
) -> VehicleSelection {
    roster
        .iter()
        .find(|e| e.player_id == me)
        .and_then(|e| e.pick.as_ref())
        .map(decode_pick)
        .unwrap_or(offered)
}

/// The reverse of [`encode_pick`]: a roster pick back into the app's
/// `VehicleSelection` — the empty wire id is the dev car.
pub fn decode_pick(pick: &VehiclePick) -> VehicleSelection {
    VehicleSelection {
        id: if pick.vehicle.is_empty() {
            None
        } else {
            Some(pick.vehicle.clone())
        },
        paint: pick.paint as usize,
    }
}

/// The lobby's pick validator, built from the mounted content catalog —
/// the authoritative side of `SetVehicle` (F24-B.3). A host installs it
/// as `HostConfig::pick_validator` so a peer cannot roster a car it
/// could never spawn. Designed policy:
///
/// - `""` (the synthetic dev car) is always a legal pick — it is the
///   engine's no-content fallback — but it has exactly one paint job,
///   so `paint` must be 0;
/// - any other pick must name a catalog entry *exactly* — ids are the
///   canonical lowercase basenames; display-name aliases are a menu
///   convenience, not a wire identity — and must be `EntryStatus::Ready`
///   (an entry with missing deps cannot spawn for anyone);
/// - `paint` is bounded by the entry's metadata `Colors` list
///   (`paints.len()`, minimum one job) — the same bound the garage menu
///   presents. The model's `paint_jobs` check in
///   `mm2_content::load_vehicle` stays authoritative at spawn time; the
///   fingerprint gate guarantees every peer's catalog is identical, so
///   a pick legal here is legal everywhere.
pub fn vehicle_validator(catalog: &VehicleCatalog) -> PickValidator {
    // id → paint bound, or the refusal reason for a known-but-unloadable
    // entry — kept whole so a refusal can say *why* a listed car fails.
    let mut legal: BTreeMap<String, Result<usize, String>> = BTreeMap::new();
    for e in &catalog.entries {
        let bound = match &e.status {
            EntryStatus::Ready => Ok(e.paints.len().max(1)),
            EntryStatus::Incomplete { missing } => Err(format!(
                "vehicle {} is incomplete: missing {}",
                e.id,
                missing.join(", ")
            )),
        };
        legal.insert(e.id.clone(), bound);
    }
    Arc::new(move |vehicle, paint| {
        if vehicle.is_empty() {
            return if paint == 0 {
                Ok(())
            } else {
                Err("the dev car has a single paint job".to_string())
            };
        }
        match legal.get(vehicle) {
            None => Err(format!("unknown vehicle id {vehicle:?}")),
            Some(Err(reason)) => Err(reason.clone()),
            Some(Ok(bound)) if paint as usize >= *bound => Err(format!(
                "paint {paint} out of range: {vehicle} has {bound} paint job(s)"
            )),
            Some(Ok(_)) => Ok(()),
        }
    })
}

// ─── The in-app lobby client (F24-B.7) ───────────────────────────────
//
// `Client` is `!Sync` and blocks in `recv`, so it lives on a pump
// thread owned by [`LobbyLink`]: inbound frames become [`LobbyEvent`]s
// on a channel, outbound intents ride the shared [`ClientCtl`].
// [`drive_lobby`] drains that channel once per update into
// [`LobbyState`] and — the point of the bridge — feeds an accepted
// `Start` into the *existing* `Session` lifecycle: the world the host
// configured loads through the same `load_session_world` path a local
// session takes, never a parallel spawn. Remote players are not
// spawned and nothing about position/score/damage is replicated —
// that is F25/F26 scope, and nothing here pretends otherwise.
//
// Exit ownership: while a `LobbyLink` resource exists the bridge owns
// process exit — `drive_session`'s `quit → Menu` arm must not write
// `AppExit` (quitting a *session* returns to the lobby, not the OS);
// `drive_lobby` writes it instead, once the session is parked.

/// How long the bridge waits for the host's socket close after our
/// `Leave` before ending the link anyway — mirrors `mm2-join`'s
/// `LEAVE_WATCHDOG`; a host that never closes cannot wedge a quit.
const LEAVE_WATCHDOG: Duration = Duration::from_secs(5);

/// How often a leaving client repeats its `Leave` until the host's close
/// acknowledges it (designed, like `RESET_RETRY_TICKS`/`RESULTS_FRAME_REPEATS`
/// — nothing original is involved). A single `Leave` frame can be lost on
/// a lossy link; it carries no reply of its own, so the removal stays
/// invisible until the socket closes. Repeating it makes the host's close
/// the acknowledgement: the first copy the host reads reaps the seat.
/// `mm2-join` shares this cadence, and the whole retry window is bounded
/// by [`LEAVE_WATCHDOG`], so the worst case is a bounded handful of
/// copies against a host that never closes.
pub const LEAVE_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// One lobby transition the pump thread observed — the `Client`'s
/// inbound message stream plus the terminal link-death marker.
#[derive(Debug)]
pub enum LobbyEvent {
    /// A host→client message, verbatim.
    Message(Message),
    /// The link ended — the host went away, or it closed our socket
    /// after our `Leave`. The string is display-ready.
    Closed(String),
}

/// The app's handle on a joined lobby. Dropping it is a polite
/// disconnect — `Drop` sends `Leave` so the host records a `Quit`
/// (and the close it produces is also what ends the pump thread).
#[derive(Resource)]
pub struct LobbyLink {
    /// Pump→app events. `Receiver` is `!Sync`, so it sits behind a
    /// mutex to satisfy `Resource`'s bound; `drive_lobby` drains it.
    events: Mutex<Receiver<LobbyEvent>>,
    ctl: ClientCtl,
    /// Our roster slot — the `Welcome` id.
    player_id: u16,
    /// How this link reaches the host: the UDP lane, or TCP alone when
    /// [`LobbyLink::join_tcp_only`] was asked for explicitly.
    transport: Transport,
    /// The host's advertised address — lobby UI/test display only.
    peer: SocketAddr,
    /// Our roster name.
    driver: String,
    /// Whether *this* process mounted mods — the wire never carries
    /// it (`accept` stamps `mods_active: false`); a session the
    /// lobby starts gets our local fact.
    mods_active: bool,
    /// This launch's local dev overrides — applied to the accepted
    /// session config at `Start`. Never wire-legal (`advertise`
    /// refuses them), so they can only ever be a local stamp.
    pub dev: DevOverrides,
    /// `leave()` was sent — `Some` the instant it went out, doubling
    /// as the watchdog's clock.
    leaving: Option<Instant>,
    /// When the newest `Leave` copy went out — the clock the repeat
    /// rides (see [`LobbyLink::retry_leave`]).
    leave_sent: Option<Instant>,
    /// The pump reported `Closed` — the link is dead.
    pub closed: bool,
    /// Set by the pump thread the instant it ends — the link is dead
    /// even before an update has drained the terminal `Closed` event.
    /// A teardown that has stopped running updates reads this (see
    /// [`LobbyLink::leave_reliably`]).
    ended: Arc<AtomicBool>,
    /// Repeat the goodbye at [`LobbyLink`]'s drop until the host closes
    /// (see [`LobbyLink::arm_reliable_leave`]). Armed by a headless
    /// runner whose stop fired; the windowed app never arms it, so its
    /// resource-removal drop stays non-blocking.
    reliable_leave: bool,
    /// Re-ready after every `Cancel` — the host resets readiness when it
    /// closes a session, so a client that asked to be ready (`--ready`)
    /// would otherwise sit out the rematch with no one to press `Enter`.
    keep_ready: bool,
    /// The readiness we last asked for. `SetReady` is one frame with no
    /// reply, so the ask is level-triggered: [`drive_lobby`] repeats it
    /// until the roster echoes it (see [`LobbyLink::resend_intents`]).
    want_ready: Option<bool>,
    /// The pick we last asked for, repeated the same way until the
    /// roster shows it or the host refuses it.
    want_pick: Option<VehiclePick>,
    /// What the newest roster we processed said about our own seat. An ask
    /// that this already satisfies when it is made was not echoed by the
    /// host: the roster is older than the ask (the host's reset of
    /// readiness after a `Cancel` or an ad edit can still be in flight),
    /// so the match proves nothing (see [`LobbyLink::resend_intents`]).
    seen_ready: Option<bool>,
    seen_pick: Option<VehiclePick>,
    /// The ask was made against a roster that already matched it: it may
    /// only be cleared once a roster has disagreed with it or a repeat
    /// has gone out.
    unproven_ready: bool,
    unproven_pick: bool,
    /// When the newest intent copy went out — the clock the repeat rides.
    intent_sent: Option<Instant>,
    /// The newest `Start` generation this link adopted — entered (the
    /// session began) or parked (`pending_start`). Generations climb
    /// within one authority's link, so a `Start` naming a generation at
    /// or below this is the host's bounded re-send (F31-B.4 / #1489), not
    /// a new session: acting on it would restart the running one. The
    /// field lives on the link, not [`LobbyState`], because a fresh
    /// authority replaces the link and restarts its numbering at 1.
    entered_generation: Option<u64>,
    /// The newest `Cancel` generation this link answered. A `Cancel` newer
    /// than this is a new round's close even when the link never adopted
    /// that round (every `Start` repeat was lost), so it still owes the
    /// `keep_ready` re-ready; one at or below it is a re-send.
    cancelled_generation: Option<u64>,
}

impl LobbyLink {
    /// Join the lobby at `addr` over the UDP lane (NETPLAY-06A), then
    /// spawn the pump thread. The handshake runs inside
    /// `Client::join_lane` under its own bounds (`HANDSHAKE_TIMEOUT`, the
    /// lane echo deadline), so a silent host cannot wedge the caller past
    /// them. A blocked UDP path is [`NetError::LaneTimeout`], whose
    /// message names the port to open — there is no silent TCP fallback;
    /// see [`LobbyLink::join_tcp_only`] for the explicit lower-quality
    /// option.
    pub fn join(
        addr: SocketAddr,
        hello: &Hello,
        mods_active: bool,
        dev: DevOverrides,
    ) -> Result<Self, NetError> {
        Self::join_routed(addr, hello, mods_active, dev, None)
    }

    /// [`LobbyLink::join`] with the datagram path steered through a relay
    /// (`mm2 --lane-via`): a harness seam for the UDP impairment proxy,
    /// which the host's source-address check otherwise keeps off the path.
    /// `None` is the ordinary direct join.
    pub fn join_routed(
        addr: SocketAddr,
        hello: &Hello,
        mods_active: bool,
        dev: DevOverrides,
        route: Option<mm2_net::LaneRoute>,
    ) -> Result<Self, NetError> {
        Self::adopt(
            Client::join_lane_routed(addr, hello, mm2_net::LANE_ECHO_TIMEOUT, route)?,
            addr,
            hello,
            mods_active,
            dev,
        )
    }

    /// Join over TCP alone — no UDP lane, so `InputWindow`/`Snap` ride
    /// the control stream. **Lower quality** (head-of-line blocking) and
    /// never chosen implicitly: it exists for tests that route the link
    /// through the TCP-only impairment proxy (which cannot carry UDP
    /// until the datagram proxy of NETPLAY-06B lands). The link reports
    /// [`Transport::Tcp`] from [`LobbyLink::transport`].
    pub fn join_tcp_only(
        addr: SocketAddr,
        hello: &Hello,
        mods_active: bool,
        dev: DevOverrides,
    ) -> Result<Self, NetError> {
        warn!(%addr, "joining over TCP only: no UDP lane, lower quality");
        Self::adopt(Client::join(addr, hello)?, addr, hello, mods_active, dev)
    }

    fn adopt(
        client: Client,
        addr: SocketAddr,
        hello: &Hello,
        mods_active: bool,
        dev: DevOverrides,
    ) -> Result<Self, NetError> {
        let transport = client.transport();
        let player_id = client.player_id();
        let ctl = client.ctl()?;
        let (tx, rx) = mpsc::channel();
        let ended = Arc::new(AtomicBool::new(false));
        let pump_ended = Arc::clone(&ended);
        thread::Builder::new()
            .name("mm2-lobby".to_string())
            .spawn(move || pump(client, tx, pump_ended))?;
        Ok(Self {
            events: Mutex::new(rx),
            ctl,
            player_id,
            transport,
            peer: addr,
            driver: hello.driver.clone(),
            mods_active,
            dev,
            leaving: None,
            leave_sent: None,
            closed: false,
            ended,
            reliable_leave: false,
            keep_ready: false,
            cancelled_generation: None,
            want_ready: None,
            want_pick: None,
            seen_ready: None,
            seen_pick: None,
            unproven_ready: false,
            unproven_pick: false,
            intent_sent: None,
            entered_generation: None,
        })
    }

    /// The transport this link negotiated — [`Transport::Tcp`] only when
    /// the caller chose [`LobbyLink::join_tcp_only`].
    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Lane datagram counters; `None` on a TCP-only link.
    pub fn lane_counts(&self) -> Option<mm2_net::LaneCounts> {
        self.ctl.lane_counts()
    }

    /// Ask the host to mark us ready (or not). The ask is remembered and
    /// repeated by [`drive_lobby`] until the roster echoes it, so a
    /// dropped datagram cannot leave us unready on the host.
    pub fn set_ready(&mut self, ready: bool) -> Result<(), NetError> {
        self.want_ready = Some(ready);
        self.unproven_ready = self.seen_ready == Some(ready);
        self.intent_sent = Some(Instant::now());
        self.ctl.set_ready(ready)
    }

    /// Offer a vehicle pick, remembered and repeated like
    /// [`LobbyLink::set_ready`] until the roster shows it.
    pub fn set_vehicle(&mut self, vehicle: &str, paint: u8) -> Result<(), NetError> {
        let pick = VehiclePick {
            vehicle: vehicle.to_string(),
            paint,
        };
        self.unproven_pick = self.seen_pick.as_ref() == Some(&pick);
        self.want_pick = Some(pick);
        self.intent_sent = Some(Instant::now());
        self.ctl.set_vehicle(vehicle, paint)
    }

    /// Weigh one roster against the asks still outstanding: drop the ones
    /// it echoes. Called for every roster that arrives, not once per
    /// frame — two rosters drained in one frame (the host's reset, then
    /// its answer to our ask) must both be seen.
    fn observe_roster(&mut self, roster: &[RosterEntry]) {
        let ours = roster.iter().find(|e| e.player_id == self.player_id);
        self.seen_ready = ours.map(|e| e.ready);
        self.seen_pick = ours.and_then(|e| e.pick.clone());
        // A roster that matched the ask before it was made is not an
        // echo: the host may have reset our flags since, and the one
        // copy of the ask may be the one the link dropped. Wait for
        // proof that the stale roster is behind us — a roster that
        // disagrees (the reset arrived) or a repeat having gone out.
        if self
            .want_ready
            .is_some_and(|r| ours.is_some_and(|e| e.ready != r))
        {
            self.unproven_ready = false;
        }
        if self
            .want_pick
            .as_ref()
            .is_some_and(|p| ours.is_some_and(|e| e.pick.as_ref() != Some(p)))
        {
            self.unproven_pick = false;
        }
        if !self.unproven_ready
            && self
                .want_ready
                .is_some_and(|r| ours.is_some_and(|e| e.ready == r))
        {
            self.want_ready = None;
        }
        if !self.unproven_pick
            && self
                .want_pick
                .as_ref()
                .is_some_and(|p| ours.is_some_and(|e| e.pick.as_ref() == Some(p)))
        {
            self.want_pick = None;
        }
    }

    /// Repeat whichever intent the roster has not echoed yet, at most
    /// once per [`KEEPALIVE_INTERVAL`]. A no-op once both match.
    fn resend_intents(&mut self, roster: &[RosterEntry]) {
        if self.leaving() || self.closed {
            return;
        }
        self.observe_roster(roster);
        if (self.want_ready.is_none() && self.want_pick.is_none())
            || self
                .intent_sent
                .is_some_and(|at| at.elapsed() < KEEPALIVE_INTERVAL)
        {
            return;
        }
        self.intent_sent = Some(Instant::now());
        self.unproven_ready = false;
        self.unproven_pick = false;
        if let Some(pick) = self.want_pick.clone() {
            let _ = self.ctl.set_vehicle(&pick.vehicle, pick.paint);
        }
        if let Some(ready) = self.want_ready {
            let _ = self.ctl.set_ready(ready);
        }
    }

    /// Stay ready across rounds: send `ready` again each time the host
    /// closes a session (and so clears the roster's flags). The caller
    /// still sends the first `ready` itself.
    pub fn keep_ready(mut self, on: bool) -> Self {
        self.keep_ready = on;
        self
    }

    /// The roster id the host minted for us.
    pub fn player_id(&self) -> u16 {
        self.player_id
    }

    /// The address we joined.
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Our roster name — for the lobby surface.
    pub fn driver(&self) -> &str {
        &self.driver
    }

    /// Outbound intents (`set_ready`, `set_vehicle`) — the legal
    /// client→host set, serialized on the shared writer.
    pub fn ctl(&self) -> &ClientCtl {
        &self.ctl
    }

    /// Whether `leave()` has been sent.
    pub fn leaving(&self) -> bool {
        self.leaving.is_some()
    }

    /// Say goodbye — the host records a `Quit` and closes our socket;
    /// the pump then reports `Closed`. Idempotent. The `Leave` is
    /// repeated by [`LobbyLink::retry_leave`] until that close lands: a
    /// single copy can be dropped, and no reply tells us it was lost.
    pub fn leave(&mut self) {
        if self.leaving.is_none() {
            self.leaving = Some(Instant::now());
            self.send_leave();
        }
    }

    /// One `Leave` copy out, stamping the retry clock. The send is
    /// best-effort, like `Drop`'s: a broken socket is already a dead
    /// link, not a reason to skip the rest of the goodbye.
    fn send_leave(&mut self) {
        let _ = self.ctl.leave();
        self.leave_sent = Some(Instant::now());
    }

    /// Repeat our `Leave` until the host's close acknowledges it, at most
    /// once per [`LEAVE_RETRY_INTERVAL`] and never past the
    /// [`LEAVE_WATCHDOG`] window. Called once per frame by
    /// [`drive_lobby`]; a no-op until [`LobbyLink::leave`] has been
    /// called and once the pump has reported `Closed`.
    pub fn retry_leave(&mut self) {
        if self.leaving.is_none() || self.closed {
            return;
        }
        if self
            .leaving
            .is_some_and(|since| since.elapsed() > LEAVE_WATCHDOG)
        {
            // The watchdog is about to end the link; a copy past it can
            // only race the exit.
            return;
        }
        if self
            .leave_sent
            .is_some_and(|at| at.elapsed() < LEAVE_RETRY_INTERVAL)
        {
            return;
        }
        self.send_leave();
    }

    /// Say goodbye and *block* until the host's close acknowledges it —
    /// the blocking form of the [`LobbyLink::leave`] /
    /// [`LobbyLink::retry_leave`] pair, for a teardown that will not run
    /// another update. A `Leave` carries no reply of its own: a single
    /// copy can be dropped on a lossy link, and the host would then keep
    /// this seat (and its reduced-roster broadcast) until some later
    /// membership change — a client that ended its run would strand
    /// itself on the host and its peers. Repeating the copy until the
    /// pump sees the host's close (or the watchdog bounds the wait)
    /// makes the departure survive a dropped datagram, the standing rule
    /// for a one-shot message that matters.
    pub fn leave_reliably(&mut self) {
        self.leave();
        let start = Instant::now();
        while !self.ended.load(AtomicOrdering::Relaxed)
            && !self.closed
            && start.elapsed() < LEAVE_WATCHDOG
        {
            thread::sleep(LEAVE_RETRY_INTERVAL);
            self.retry_leave();
        }
    }

    /// Make this link's own [`Drop`] send its goodbye reliably (see
    /// [`LobbyLink::leave_reliably`]) instead of a single best-effort
    /// copy. A headless runner that stopped on a condition arms this so
    /// the *first* copy leaves when the run's world does — the same
    /// instant the old single-copy drop sent it — and the repeats then
    /// carry it past a dropped datagram. The windowed app never arms
    /// it, so removing its link mid-session stays a non-blocking drop.
    pub fn arm_reliable_leave(&mut self) {
        self.reliable_leave = true;
    }

    /// Drain whatever the pump has queued — non-blocking.
    fn drain(&self) -> Vec<LobbyEvent> {
        let rx = self.events.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }
}

impl Drop for LobbyLink {
    fn drop(&mut self) {
        if self.reliable_leave {
            self.leave_reliably();
        } else {
            self.leave();
        }
    }
}

/// The pump's whole job: block in `Client::recv`, forward each inbound
/// message as a [`LobbyEvent`], and end with `Closed` — on a dead
/// socket *or* when the app dropped the link (a failed `send` is the
/// receiver going away). Either way the client says `Leave` on its
/// way out so the host records `Quit`, not `Lost`.
fn pump(mut client: Client, tx: Sender<LobbyEvent>, ended: Arc<AtomicBool>) {
    loop {
        let event = match client.recv() {
            Ok(msg) => LobbyEvent::Message(msg),
            Err(e) => LobbyEvent::Closed(e.to_string()),
        };
        let terminal = matches!(event, LobbyEvent::Closed(_));
        if tx.send(event).is_err() || terminal {
            break;
        }
    }
    // The link is dead before the `Closed` event is drained — a teardown
    // no longer running updates reads this via [`LobbyLink::leave_reliably`].
    ended.store(true, AtomicOrdering::Relaxed);
    let _ = client.leave();
}

/// The app's mirror of lobby state — `drive_lobby` (joined) or
/// `drive_host` (hosted) writes it, a lobby surface or test reads it.
/// On the host side `roster` carries remote players only — the wire
/// roster never carries the host's own seat (player id 0). Everything
/// here is display/decision state; the session itself is never
/// mirrored (the `Session` resource stays authoritative).
#[derive(Resource, Default)]
pub struct LobbyState {
    /// Latest roster state — the wire broadcast verbatim on a joined
    /// link (includes our own entry), or the mirror `drive_host`
    /// rebuilds from [`HostEvent`] deltas on a hosted one (remote
    /// players only; the host's own seat is never on the wire).
    pub roster: Vec<RosterEntry>,
    /// The advertised session — the newest `Session` message a joined
    /// link received, or the advertisement a hosted link itself set.
    /// `Start` carries its own snapshot of the *running* session, so
    /// this is lobby-display state, not what a start begins.
    pub advertised: Option<SessionAdvertisement>,
    /// The running session's lobby generation once `Start` arrived —
    /// cleared by the matching `Cancel`.
    pub generation: Option<u64>,
    /// The host seat's vehicle pick as the last `Start` carried it —
    /// `None` on a dedicated seat-less host or before any start. The
    /// remote-spawn reconcile builds the host's car from this (F25-A).
    pub host_pick: Option<VehiclePick>,
    /// The latest display-ready notice — a refused pick, a refused or
    /// unrunnable session, a lost link. Replaced, not accumulated.
    pub notice: Option<String>,
    /// A validated session config parked until the session returns
    /// to `Menu` — a `Start` that arrived while we were mid-session
    /// or mid-teardown.
    pub pending_start: Option<(u64, SessionConfig)>,
    /// Exit the app once the session is back at `Menu` — refusal and
    /// leave paths queue teardown first, then this.
    pub pending_exit: Option<u8>,
    /// The exit code the bridge already wrote as an `AppExit` —
    /// `pending_exit` is consumed by the write, so this is the record
    /// surface's view of how the lobby ended.
    pub exit_sent: Option<u8>,
}

/// The drain system — runs once per update wherever a [`LobbyLink`]
/// exists. Never blocks: everything the pump queued is applied, then
/// the lifecycle intents that need `Menu` (a parked start, a queued
/// exit) settle there.
///
// The bridge genuinely threads the link, its mirror, the session
// lifecycle pair, the VFS and the car resources — a SystemParam
// bundle would hide the pieces every handler uses.
#[allow(clippy::too_many_arguments)]
pub fn drive_lobby(
    mut link: ResMut<LobbyLink>,
    mut lobby: ResMut<LobbyState>,
    mut session: ResMut<Session>,
    mut control: ResMut<SessionControl>,
    vfs: Res<Mm2Vfs>,
    mut selected: ResMut<SelectedCar>,
    mut tuned: ResMut<TunedVehicle>,
    menu: Option<Res<MenuShell>>,
    mut snaps: ResMut<crate::netdrive::RemoteSnaps>,
    mut lead: Option<ResMut<crate::netpace::ClientLead>>,
    mut feed: Option<ResMut<crate::netremote::RemoteFeed>>,
    mut exit: MessageWriter<AppExit>,
) {
    for event in link.drain() {
        match event {
            LobbyEvent::Message(Message::Session(ad)) => match gate(&vfs.0, &ad) {
                Ok(_) => lobby.advertised = Some(ad),
                Err(why) => refuse(&mut link, &mut lobby, why),
            },
            LobbyEvent::Message(Message::Roster { players }) => {
                link.observe_roster(&players);
                lobby.roster = players;
            }
            LobbyEvent::Message(Message::VehicleRefused { reason }) => {
                // A refused pick must not be repeated forever.
                link.want_pick = None;
                lobby.notice = Some(reason);
            }
            LobbyEvent::Message(Message::Start {
                generation,
                session: ad,
                host_pick,
            }) => start(
                &mut link,
                &mut lobby,
                &mut session,
                &mut control,
                &vfs.0,
                &mut selected,
                &mut tuned,
                &mut snaps,
                generation,
                &ad,
                host_pick,
            ),
            LobbyEvent::Message(Message::Snap {
                generation,
                tick,
                entries,
                trailers,
                impacts,
                race,
            }) => {
                if let Some(lead) = lead.as_mut() {
                    lead.note_snap(link.player_id(), generation, tick, &entries);
                }
                snaps.push(generation, tick, entries, trailers, impacts, race)
            }
            LobbyEvent::Message(msg @ Message::RemoteCommands { generation, .. }) => {
                // Namespace the store to the running session; a frame from
                // another generation is counted and dropped by `absorb`.
                if let Some(feed) = feed.as_mut()
                    && session.is_playing()
                    && session.wire_generation() == generation
                {
                    feed.store.set_generation(Some(generation));
                    feed.store.absorb(&msg);
                }
            }
            LobbyEvent::Message(Message::Impacts {
                generation,
                impacts,
            }) => snaps.queue_impacts(generation, impacts),
            LobbyEvent::Message(Message::Props {
                generation,
                tick,
                table,
                rows,
            }) => snaps.push_props(generation, tick, table, rows),
            LobbyEvent::Message(Message::Traffic {
                generation,
                tick,
                roster,
                rows,
            }) => snaps.push_traffic(generation, tick, roster, rows),
            LobbyEvent::Message(Message::World { generation, ticks }) => {
                snaps.push_world(generation, ticks)
            }
            LobbyEvent::Message(Message::Leaves {
                generation,
                tick,
                rows,
            }) => snaps.push_leaves(generation, tick, rows),
            LobbyEvent::Message(Message::Signals {
                generation,
                tick,
                rows,
            }) => snaps.push_signals(generation, tick, rows),
            LobbyEvent::Message(Message::Cnr { generation, frame }) => {
                snaps.push_cnr(generation, &frame)
            }
            LobbyEvent::Message(Message::Cancel { generation }) => {
                // A `Cancel` for a generation this link never entered is
                // the host's bounded re-send or a reordered duplicate
                // (F31-B.4 / #1489) — a no-op, never a re-ready.
                let acted = cancel(&mut lobby, &mut session, &mut control, generation);
                // The host cleared every ready flag before it sent this,
                // so our answer lands after the reset, never before it.
                // A repeated `Cancel` names a generation we no longer
                // hold — a no-op: re-readying (or clearing the intent we
                // re-armed) would answer the same frame twice.
                let newer = link.cancelled_generation.is_none_or(|g| generation > g);
                if acted || newer {
                    link.cancelled_generation = Some(generation);
                    // A readiness we still owed the host died with that
                    // reset unless we keep it.
                    link.want_ready = None;
                    if link.keep_ready && !link.leaving() {
                        let _ = link.set_ready(true);
                    }
                }
            }
            LobbyEvent::Closed(reason) => {
                link.closed = true;
                // The snap stream died with the link — its watermarks
                // and ledgers belong to the dead authority's
                // `(generation, tick)` sequence, which a new authority
                // restarts. Clear the inbox now rather than let a
                // staged frame or a stale watermark reach into the
                // next session.
                snaps.reset();
                if link.leaving() {
                    // The expected end of our own `Leave`.
                    lobby.pending_exit.get_or_insert(0);
                } else {
                    lobby.roster.clear();
                    lobby.advertised = None;
                    lobby.generation = None;
                    lobby.host_pick = None;
                    lobby.pending_start = None;
                    lobby.notice = Some(format!("lost the host: {reason}"));
                    lobby.pending_exit.get_or_insert(1);
                }
            }
            // Variants a client never receives (`Hello`/`Accept`/
            // `Reject`/`Welcome` are the handshake; `SetReady`/
            // `SetVehicle`/`Leave` are client→host) — a hostile host
            // sending them is already dropped server-side, and a bug
            // here must not invent meaning.
            LobbyEvent::Message(_) => {}
        }
    }

    // Our `Leave` is a frame with no reply: repeat it until the pump's
    // `Closed` says the host read a copy and closed the socket, so a
    // dropped copy cannot leave a ghost seat on the host or its other
    // clients.
    link.retry_leave();
    link.resend_intents(&lobby.roster);

    // A link ending — our `leave()` or a dead host — takes a live
    // session down with it; the teardown rides the normal
    // `Unloading → Menu` lifecycle.
    if (link.leaving() || link.closed)
        && !matches!(
            session.phase(),
            SessionPhase::Menu | SessionPhase::Unloading
        )
    {
        control.quit = true;
    }
    // The host should close our socket right after our `Leave` lands;
    // if it never does, the watchdog ends the link anyway — a quit
    // must not wait on a hung peer.
    if let Some(since) = link.leaving
        && since.elapsed() > LEAVE_WATCHDOG
    {
        lobby.pending_exit.get_or_insert(0);
    }

    // Lifecycle intents settle at `Menu` — the phase the lobby's
    // "waiting" state *is*.
    if *session.phase() == SessionPhase::Menu {
        if lobby.pending_exit.is_none()
            && let Some((generation, config)) = lobby.pending_start.take()
            && let Err(e) = begin_wired(&mut session, &mut control, config, generation)
        {
            // `accept` + `check_session` already ran, so a refusal
            // here means the lifecycle rejected the begin itself.
            lobby.notice = Some(format!("started session was refused: {e}"));
            lobby.pending_exit = Some(1);
            link.leave();
        }
        if menu.is_none()
            && lobby.pending_start.is_none()
            && let Some(code) = lobby.pending_exit.take()
        {
            lobby.exit_sent = Some(code);
            exit.write(AppExit::from_code(code));
        }
    }
}

/// An advertised session back into a validated config — the same
/// `accept` + `check_session` pair `mm2-join` runs, so the in-app
/// client trusts exactly what the headless one does.
fn gate(vfs: &Vfs, ad: &SessionAdvertisement) -> Result<SessionConfig, String> {
    let config = accept(ad).map_err(|e| format!("unacceptable session: {e}"))?;
    check_session(vfs, &config).map_err(|e| format!("session cannot run here: {e}"))?;
    Ok(config)
}

/// An advertised session we cannot run: record why, queue the exit,
/// and leave cleanly — the host records a `Quit`, not a dropped socket.
fn refuse(link: &mut LobbyLink, lobby: &mut LobbyState, why: String) {
    lobby.notice = Some(why);
    lobby.pending_start = None;
    lobby.pending_exit = Some(1);
    link.leave();
}

/// Begin the lobby's session under the wire's generation. The intents a
/// finished session's teardown left queued die with it: `quit` survives
/// `Unloading → Menu` by design (the `Menu` arm of `drive_session`
/// consumes it next frame), but a parked or immediate `Start` begins in
/// that very frame, so the stale flag would quit the new session the
/// moment it went live — a rematch cancelled on arrival.
fn begin_wired(
    session: &mut Session,
    control: &mut SessionControl,
    config: SessionConfig,
    generation: u64,
) -> Result<(), mm2_game::SessionError> {
    session.begin_generation(config, generation)?;
    control.quit = false;
    control.restart = false;
    control.next_event = None;
    control.pause = false;
    control.open_menu = false;
    control.menu = false;
    Ok(())
}

/// `Start` → the existing session lifecycle. The advertised config is
/// gated, stamped with the roster-echoed pick and this process's local
/// facts, then begun — parked first if a session is still live.
#[allow(clippy::too_many_arguments)]
fn start(
    link: &mut LobbyLink,
    lobby: &mut LobbyState,
    session: &mut Session,
    control: &mut SessionControl,
    vfs: &Vfs,
    selected: &mut SelectedCar,
    tuned: &mut TunedVehicle,
    snaps: &mut crate::netdrive::RemoteSnaps,
    generation: u64,
    ad: &SessionAdvertisement,
    host_pick: Option<VehiclePick>,
) {
    let mut config = match gate(vfs, ad) {
        Ok(config) => config,
        Err(why) => return refuse(link, lobby, why),
    };
    // A `Start` the host re-sends (F31-B.4 / #1489) names a generation
    // this link already adopted — entering or parking the same session
    // again would restart it. Ignore it; every genuine start mints a
    // higher generation within one link's authority.
    if link
        .entered_generation
        .is_some_and(|entered| generation <= entered)
    {
        return;
    }
    link.entered_generation = Some(generation);
    lobby.host_pick = host_pick;
    // The pick is roster state, not session params.
    config.vehicle = start_pick(&lobby.roster, link.player_id(), offered_selection(selected));
    config.mods_active = link.mods_active;
    config.dev = link.dev.clone();
    match resolve_selection(vfs, &config.vehicle) {
        Ok((car, vehicle_config)) => {
            *selected = car;
            *tuned = TunedVehicle(vehicle_config);
        }
        Err(why) => {
            return refuse(
                link,
                lobby,
                format!("our roster pick cannot load here: {why}"),
            );
        }
    }
    lobby.generation = Some(generation);
    // The accepted `Start` ends whatever snap stream preceded it —
    // even the same authority's restart mints a fresh generation, and
    // a *different* authority (a fresh host process, or this same
    // link's host after a `Cancel`) restarts its numbering entirely.
    // The inbox's watermarks and ledgers describe the stream that
    // produced them, so the new sequence starts clean rather than
    // stale-dropping under a dead stream's watermark. Reset on accept,
    // not on begin: impact rows the new stream queues while a parked
    // session tears down belong to it and must not be wiped.
    snaps.reset();
    if *session.phase() == SessionPhase::Menu {
        if let Err(e) = begin_wired(session, control, config, generation) {
            lobby.notice = Some(format!("started session was refused: {e}"));
            lobby.pending_exit = Some(1);
            link.leave();
        }
    } else {
        // A second `Start` should never arrive mid-session (the host's
        // own gate forbids it) — but if it does, queue it like a
        // restart instead of pretending the lifecycle allowed it.
        lobby.pending_start = Some((generation, config));
        control.quit = true;
    }
}

/// `Cancel` — the host closed the running session. The generation names
/// *which* session: a stray cancel for one we never entered (a pending
/// start we never began) only clears that pending begin. Returns whether
/// it named a session we held, so the caller re-readies only on a real
/// transition — a `Cancel` the host re-sends (F31-B.4 / #1489) is the
/// same frame again and must not re-ready a client that already answered.
fn cancel(
    lobby: &mut LobbyState,
    session: &mut Session,
    control: &mut SessionControl,
    generation: u64,
) -> bool {
    let mut acted = false;
    if lobby
        .pending_start
        .as_ref()
        .is_some_and(|(g, _)| *g == generation)
    {
        lobby.pending_start = None;
        acted = true;
    }
    if lobby.generation == Some(generation) {
        lobby.generation = None;
        lobby.host_pick = None;
        acted = true;
        if !matches!(
            session.phase(),
            SessionPhase::Menu | SessionPhase::Unloading
        ) {
            control.quit = true;
        }
    }
    acted
}

/// The roster-echoed pick into the car resources `load_session_world`
/// reads — the dev pick (`id: None`) clears `SelectedCar` like a fresh
/// launch with no `--car`.
fn resolve_selection(
    vfs: &Vfs,
    pick: &VehicleSelection,
) -> Result<(SelectedCar, mm2_vehicle::VehicleConfig), String> {
    let Some(id) = pick.id.as_deref() else {
        return Ok((
            SelectedCar {
                def: None,
                paint: 0,
            },
            mm2_vehicle::VehicleConfig::default(),
        ));
    };
    let def = mm2_content::load_vehicle(vfs, id, pick.paint)
        .map_err(|e| format!("{id:?} paint {}: {e}", pick.paint))?;
    // `VehicleDef` is `!Clone` — take the tuning before handing the
    // definition to `SelectedCar`.
    let config = def.config.clone();
    Ok((
        SelectedCar {
            paint: pick.paint,
            def: Some(def),
        },
        config,
    ))
}

/// The windowed lobby's keyboard surface: while the session sits at
/// `Menu` — the lobby's waiting phase — `Enter` toggles our roster
/// ready flag, the arrow keys pick the car and paint, and `Esc` leaves
/// the lobby (the session-lifecycle Esc lives in `session_control_input`,
/// which ignores `Menu`, so the two never collide). Gamepad equivalents are a menu-surface decision
/// deferred with the real lobby UI.
pub fn lobby_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    lobby: Res<LobbyState>,
    mut link: ResMut<LobbyLink>,
    vfs: Option<Res<Mm2Vfs>>,
    mut garage: Local<Option<Vec<(String, u8)>>>,
) {
    // A lobby the menu joined this frame ignores the keypress that did it.
    if *session.phase() != SessionPhase::Menu || link.leaving() || link.closed || link.is_added() {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        link.leave();
    }
    if keys.just_pressed(KeyCode::Enter) {
        let ready = !lobby
            .roster
            .iter()
            .any(|e| e.player_id == link.player_id() && e.ready);
        let _ = link.set_ready(ready);
    }
    // Left/Right step the car, Up/Down its paint: the pick offered is
    // always a garage entry, so the host's validator takes it.
    let step = |neg: KeyCode, pos: KeyCode| {
        i32::from(keys.just_pressed(pos)) - i32::from(keys.just_pressed(neg))
    };
    let (car, paint) = (
        step(KeyCode::ArrowLeft, KeyCode::ArrowRight),
        step(KeyCode::ArrowDown, KeyCode::ArrowUp),
    );
    if (car != 0 || paint != 0)
        && let Some(vfs) = vfs
    {
        let garage =
            garage.get_or_insert_with(|| crate::lobby_view::garage(&VehicleCatalog::scan(&vfs.0)));
        let ours = lobby
            .roster
            .iter()
            .find(|e| e.player_id == link.player_id())
            .and_then(|e| e.pick.as_ref());
        let current = ours.map_or(("", 0), |p| (p.vehicle.as_str(), p.paint));
        if let Some((id, paint)) =
            crate::lobby_view::cycle_pick(garage, current, car, if car != 0 { 0 } else { paint })
        {
            let _ = link.set_vehicle(&id, paint);
        }
    }
}

/// Marker for the persistent lobby status line. Session-scoped UI
/// cannot show the lobby (nothing session-owned exists at `Menu`), so
/// the app spawns this entity itself.
#[derive(Component)]
pub struct LobbyText;

/// The lobby status line's content — the lobby's whole surface until
/// a real lobby menu exists: where we are, what the host is offering,
/// the roster's readiness, our own state, and the latest refusal or
/// link-loss notice. The line empties while a session runs — the HUD
/// owns the screen then.
pub fn drive_lobby_text(
    link: Res<LobbyLink>,
    lobby: Res<LobbyState>,
    session: Res<Session>,
    mut texts: Query<&mut Text, With<LobbyText>>,
) {
    let Ok(mut text) = texts.single_mut() else {
        return;
    };
    if *session.phase() != SessionPhase::Menu {
        text.0.clear();
        return;
    }
    text.0 = crate::lobby_view::LobbyView::client(
        link.peer(),
        link.player_id(),
        &lobby.roster,
        lobby.advertised.as_ref().map(|ad| ad.summary.as_str()),
        lobby.notice.as_deref(),
    )
    .with_bots(
        lobby
            .advertised
            .as_ref()
            .and_then(|ad| accept(ad).ok())
            .map_or(0, |c| c.cnr_bots),
    )
    .render();
}

// ─── The in-app lobby host (F24-B.8) ───────────────────────────────
//
// The dedicated `mm2-host` binary runs a lobby with no player seat;
// [`HostLink`] hosts one *inside* the app, so the process that listens
// is also the process that drives. `Host` is `!Sync` (its event
// channel is a `Receiver`), so it sits behind a mutex — unlike the
// client side no pump thread is needed: `Host::try_recv` drains
// without blocking, and [`drive_host`] runs it once per update.
//
// Lifecycle ownership mirrors `drive_lobby`: while a `HostLink`
// exists the bridge owns `Menu`-time exit (`drive_session`'s quit arm
// stays out of it), and the wire owns session boundaries — `Started`
// begins the advertised config locally under the lobby's minted
// generation with `Host` authority, a hosted session reaching `Menu`
// sends `Cancel` so the roster returns to the lobby, and `Quit`
// (stdin `quit`, Esc at `Menu`, drop) cancels a live session before
// closing the sockets. Remote players are roster/display state only —
// nothing is spawned or replicated yet (F25/F26).

/// Why hosting could not be set up — the advertisement or the
/// listen loop failed before the app took ownership.
#[derive(Debug, thiserror::Error)]
pub enum HostOpenError {
    /// The session config cannot ride the wire (`advertise` refused
    /// it — e.g. dev overrides are never network-legal).
    #[error("session cannot be advertised: {0}")]
    Advertise(#[from] SessionWireError),
    /// The lobby transport failed — bind, session broadcast.
    #[error("{0}")]
    Net(#[from] NetError),
}

/// An operator intent for a hosted lobby — the stdin command surface
/// (`start`/`cancel`/`quit`, the same words `mm2-host` accepts) and
/// the windowed lobby keys feed one channel [`drive_host`] drains.
#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    /// Request the session start — the lobby's own gate decides, the
    /// verdict arrives as `Started`/`StartRefused`.
    Start,
    /// Re-advertise the session the *next* round runs (a rematch with
    /// a changed city, mode, difficulty or conditions) — see
    /// [`HostLink::set_session`] for what is gated and kept.
    Session(Box<SessionConfig>),
    /// An operator's *edit* of the advertised session (the stdin
    /// `session city=sf event=blitz:2 …` word): applied to the link's
    /// current config when drained, then handled exactly like
    /// [`Self::Session`] — same gates, same refusal notice.
    Change(SessionChange),
    /// End the running session — everyone returns to the lobby.
    Cancel,
    /// Shut the lobby down and exit the app.
    Quit,
}

/// The operator-facing edit of a hosted lobby's next-round session —
/// `key=value` words in the `--host` flags' vocabulary (`city`,
/// `event`, `cnr`/`gold`/`limit`, `difficulty`, `weather`, `tod`,
/// `seed`). Every unnamed field keeps the link's current value, so an
/// operator changes one thing at a time without restating the rest.
/// Parsing is pure and never touches the lobby; [`Self::apply`] builds
/// the config [`HostLink::set_session`] then gates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionChange {
    world: Option<WorldMode>,
    mode: Option<ModeChange>,
    difficulty: Option<Difficulty>,
    weather: Option<Weather>,
    time_of_day: Option<TimeOfDay>,
    seed: Option<u64>,
    cnr_bots: Option<u8>,
}

/// The mode a [`SessionChange`] names — the table/row stays unresolved
/// until the change is applied, because the event's city is the
/// session's (possibly just-changed) world.
#[derive(Debug, Clone, PartialEq)]
enum ModeChange {
    Cruise,
    Event(String),
    CopsAndRobbers(CnrSettings),
}

impl SessionChange {
    /// Parse the words after `session`. Unknown keys, repeated keys,
    /// malformed values and an empty list are named errors — an
    /// operator typo must never silently re-advertise something else.
    pub fn parse<'a>(words: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        let mut change = Self::default();
        let mut seen: Vec<&str> = Vec::new();
        let (mut cnr, mut gold, mut limit) = (None, None, None);
        let mut event = None;
        let mut cruise = false;
        for word in words {
            let (key, value) = word
                .split_once('=')
                .ok_or_else(|| format!("expected key=value, got {word:?}"))?;
            if seen.contains(&key) {
                return Err(format!("{key} given twice"));
            }
            seen.push(key);
            match key {
                "city" => {
                    change.world = Some(match value.to_ascii_lowercase().as_str() {
                        "dev" | "dev-world" => WorldMode::DevWorld,
                        stem if !stem.is_empty()
                            && stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
                        {
                            WorldMode::City {
                                psdl: format!("city/{stem}.psdl"),
                            }
                        }
                        _ => return Err(format!("invalid city {value:?}")),
                    });
                }
                "event" => match value {
                    "none" | "cruise" => cruise = true,
                    _ => event = Some(value.to_string()),
                },
                "cnr" => cnr = Some(value),
                "gold" => gold = Some(value),
                "limit" => limit = Some(value),
                "difficulty" => {
                    change.difficulty = Some(match value.to_ascii_lowercase().as_str() {
                        "amateur" => Difficulty::Amateur,
                        "pro" | "professional" => Difficulty::Professional,
                        _ => return Err(format!("unknown difficulty {value:?}: amateur|pro")),
                    });
                }
                "weather" => {
                    change.weather = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid weather {value:?}"))
                            .and_then(|v| Weather::new(v).map_err(|e| e.to_string()))?,
                    );
                }
                "tod" => {
                    change.time_of_day = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid tod {value:?}"))
                            .and_then(|v| TimeOfDay::new(v).map_err(|e| e.to_string()))?,
                    );
                }
                "bots" => {
                    change.cnr_bots = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid bots {value:?}"))?,
                    );
                }
                "seed" => {
                    change.seed = Some(
                        value
                            .parse()
                            .map_err(|_| format!("invalid seed {value:?}"))?,
                    );
                }
                other => return Err(format!("unknown key {other:?}")),
            }
        }
        if seen.is_empty() {
            return Err(
                "nothing to change: city= event= cnr= gold= limit= bots= difficulty= weather= tod= seed="
                    .to_string(),
            );
        }
        if (gold.is_some() || limit.is_some()) && cnr.is_none() {
            return Err("gold=/limit= need cnr=<variant>".to_string());
        }
        if usize::from(cruise) + usize::from(event.is_some()) + usize::from(cnr.is_some()) > 1 {
            return Err("event=, event=none and cnr= each name the mode; give one".to_string());
        }
        change.mode = if let Some(variant) = cnr {
            Some(ModeChange::CopsAndRobbers(CnrSettings::parse(
                variant, gold, limit,
            )?))
        } else if let Some(event) = event {
            Some(ModeChange::Event(event))
        } else if cruise {
            Some(ModeChange::Cruise)
        } else {
            None
        };
        Ok(change)
    }

    /// The config this edit asks for on top of `base`. `fresh_seed`
    /// stands in for an unnamed seed: a changed round should not replay
    /// the last round's seed-rolled world (name `seed=` to repeat it).
    /// A named event is read against the *resulting* city; an event
    /// session whose city changes without a new `event=` is refused
    /// rather than silently pointing the old row at another city.
    pub fn apply(&self, base: &SessionConfig, fresh_seed: u64) -> Result<SessionConfig, String> {
        let mut config = base.clone();
        if let Some(world) = &self.world {
            config.world = world.clone();
        }
        let city = match &config.world {
            WorldMode::City { psdl } => psdl
                .strip_prefix("city/")
                .and_then(crate::city::psdl_stem)
                .unwrap_or("london")
                .to_string(),
            // The CLI's `--event` over the dev world resolves London's
            // tables (the developer/test rig).
            WorldMode::DevWorld => "london".to_string(),
        };
        match &self.mode {
            Some(ModeChange::Cruise) => config.mode = SessionMode::Cruise,
            Some(ModeChange::CopsAndRobbers(settings)) => {
                config.mode = SessionMode::CopsAndRobbers(*settings);
            }
            Some(ModeChange::Event(arg)) => {
                let event = EventRef::parse(arg, &city).ok_or_else(|| {
                    format!("invalid event {arg:?}: expected checkpoint|blitz|circuit|crash:<row>")
                })?;
                config.mode = SessionMode::Event(event);
            }
            None => {
                if self.world.is_some() && matches!(base.mode, SessionMode::Event(_)) {
                    return Err(
                        "an event belongs to its city: name event=<table>:<row> (or event=none) with the new city"
                            .to_string(),
                    );
                }
            }
        }
        if let Some(difficulty) = self.difficulty {
            config.difficulty = difficulty;
        }
        if let Some(weather) = self.weather {
            config.conditions.weather = weather;
        }
        if let Some(time_of_day) = self.time_of_day {
            config.conditions.time_of_day = time_of_day;
        }
        // Lobby bots belong to Cops & Robbers alone: leaving the mode
        // drops the count rather than leaving it to fail the gate, and a
        // `bots=` aimed at another mode is refused, never ignored.
        if !matches!(config.mode, SessionMode::CopsAndRobbers(_)) {
            if self.cnr_bots.is_some_and(|n| n > 0) {
                return Err("bots= only applies to Cops & Robbers (give cnr=<variant>)".into());
            }
            config.cnr_bots = 0;
        }
        if let Some(bots) = self.cnr_bots {
            config.cnr_bots = bots;
        }
        config.seed = self.seed.unwrap_or(fresh_seed);
        Ok(config)
    }
}

/// A clock-minted seed — what `--host` uses unless `--seed` is given,
/// and what a [`SessionChange`] without `seed=` re-advertises.
pub fn fresh_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Why a hosted lobby refused to re-advertise the next round's session
/// (F26-AC05) — the flag-time gates `--host` runs, applied again.
#[derive(Debug, thiserror::Error)]
pub enum SetSessionError {
    /// A round is live: its `Start` carried the running config and
    /// every peer is playing it, so the next round's ad waits for the
    /// `Cancel` (a peer that cannot run a changed ad leaves the lobby,
    /// and that must not happen mid-race).
    #[error("a session is running; change the next round's session after it ends")]
    SessionRunning,
    /// The config fails `SessionConfig::validate`.
    #[error("invalid session configuration: {0}")]
    Invalid(#[from] ConfigError),
    /// The host's own install cannot run it (unresolved city, event
    /// that cannot be built, Cops & Robbers pool too small).
    #[error("{0}")]
    Unrunnable(#[from] SessionContentError),
    /// The config cannot ride the wire.
    #[error("session cannot be advertised: {0}")]
    Advertise(#[from] SessionWireError),
    /// The lobby loop is gone.
    #[error("{0}")]
    Net(#[from] NetError),
}

/// Feed stdin `start`/`cancel`/`quit`/`session …` lines into a hosted
/// lobby — `mm2-host`'s operator words plus the in-app host's
/// [`SessionChange`] edit. A closed stdin just ends the thread;
/// unknown or malformed lines are named on stderr, never queued.
pub fn stdin_commands(tx: Sender<HostCommand>) {
    let _ = thread::Builder::new()
        .name("mm2-host-stdin".to_string())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                let mut words = line.split_whitespace();
                let command = match words.next() {
                    Some("start") => HostCommand::Start,
                    Some("cancel") => HostCommand::Cancel,
                    Some("quit") => HostCommand::Quit,
                    Some("session") => match SessionChange::parse(words) {
                        Ok(change) => HostCommand::Change(change),
                        Err(e) => {
                            eprintln!("error: session: {e}");
                            continue;
                        }
                    },
                    None => continue,
                    Some(_) => {
                        eprintln!("error: unknown command {:?}", line.trim());
                        continue;
                    }
                };
                if tx.send(command).is_err() {
                    return;
                }
            }
        });
}

/// The `mm2-host` record contract for one [`HostEvent`], shared so the
/// dedicated binary and the in-app host's headless event log speak
/// identical lines (`event=joined id=<n> driver="<name>" …`).
pub fn describe_host_event(event: &HostEvent) -> String {
    match event {
        HostEvent::Joined { id, driver, build } => {
            format!("event=joined id={id} driver={driver:?} build={build:?}")
        }
        HostEvent::Left { id, driver, cause } => {
            let cause = match cause {
                LeaveCause::Quit => "quit",
                LeaveCause::Lost => "lost",
                LeaveCause::Malformed => "malformed",
            };
            format!("event=left id={id} driver={driver:?} cause={cause}")
        }
        HostEvent::ReadyChanged { id, ready } => {
            format!("event=ready id={id} ready={ready}")
        }
        HostEvent::VehicleChanged { id, vehicle, paint } => {
            format!("event=vehicle id={id} vehicle={vehicle:?} paint={paint}")
        }
        HostEvent::VehicleRefused {
            id,
            vehicle,
            paint,
            reason,
        } => {
            format!(
                "event=pick_refused id={id} vehicle={vehicle:?} paint={paint} reason={reason:?}"
            )
        }
        HostEvent::JoinFailed { peer, reason } => {
            format!("event=join_failed peer={peer} reason={reason:?}")
        }
        HostEvent::Started { generation } => {
            format!("event=started generation={generation}")
        }
        HostEvent::StartRefused { reason } => {
            format!("event=start_refused reason={reason:?}")
        }
        HostEvent::Cancelled { generation } => {
            format!("event=cancelled generation={generation}")
        }
    }
}

/// Split the overrides an authority may keep to itself off `dev`,
/// leaving the rest for [`advertise`] to refuse. Only the authority's
/// own replicated state qualifies: `--wreck-at` destroys a car the
/// host simulates and the damage byte carries the result to every
/// peer, so nothing a client predicts depends on it; `--bot-thief` only
/// picks which objective the host seat's own evidence driver chases. A physics pin
/// (`--traction`, `--spawn`, ...) is the opposite — peers could never
/// agree on it — and stays refused.
fn host_local_dev(dev: &mut DevOverrides) -> DevOverrides {
    DevOverrides {
        wreck_at: dev.wreck_at.take(),
        wreck_seat: dev.wreck_seat.take(),
        bot_thief: std::mem::take(&mut dev.bot_thief),
        ..DevOverrides::default()
    }
}

/// The app's handle on a hosted lobby. Dropping it is the operator's
/// `quit` — `Drop` runs [`leave`](Self::leave), which cancels a live
/// session before the sockets close.
#[derive(Resource)]
pub struct HostLink {
    /// `Mutex` for `Sync` — `Host` is `!Sync` (its event channel is a
    /// `Receiver`). Held only for `try_recv` drains and `shutdown`.
    host: Mutex<Host>,
    ctl: HostCtl,
    /// Operator commands (stdin thread, `host_input`, tests).
    commands_rx: Mutex<Receiver<HostCommand>>,
    commands_tx: Sender<HostCommand>,
    /// The session this lobby advertises and the host seat plays —
    /// stamped `Host` authority at `open`; `Started` begins a clone.
    /// Carries no developer overrides — those never ride the wire.
    config: SessionConfig,
    /// The host-local dev overrides this launch may keep
    /// ([`host_local_dev`]: `--wreck-at`/`--wreck-seat`), split off the
    /// advertised config at `open` and stamped back onto the host
    /// seat's own session at `Started`. Peers never see them; every
    /// other override still refuses to host (see [`advertise`]).
    pub dev: DevOverrides,
    /// What was advertised — `lobby.advertised`'s seed and the
    /// `listening=` record's summary.
    ad: SessionAdvertisement,
    /// The start's late-join policy — the session mode's (MP-5:
    /// event lobbies close once started, cruise and Cops & Robbers
    /// stay open).
    late_join: LateJoin,
    /// The host driver's display name — never on the wire roster.
    driver: String,
    /// `leave()` ran — the link is coming down (cancel, shutdown,
    /// then `pending_exit` at `Menu`).
    leaving: bool,
    /// A `cancel` request is in flight to the host loop — suppresses
    /// the auto-cancel edge re-firing until `Cancelled` lands.
    cancel_sent: bool,
    /// The host loop's event channel ended — the lobby is dead under
    /// us, so the bridge exits nonzero like a client's lost host.
    dead: bool,
    /// Print each drained `HostEvent` as a `describe_host_event`
    /// record line — the headless run's operator surface (the same
    /// contract `mm2-host` prints); off on the windowed path, where
    /// the `HostText` surface is the display.
    log_events: bool,
}

impl HostLink {
    /// Open a hosted lobby on `bind`, advertising `config`. The host
    /// seat is itself a player, so the remote-client ceiling keeps one
    /// of the wire's [`MAX_PLAYERS`] free (MP-1's eight *total*).
    /// `config` is cloned and stamped `Host` authority; `pick_validator`
    /// is the catalog gate `mm2-host` installs. Nothing here loads the
    /// world — the event gate is the caller's flag-time check, the
    /// same one `mm2-host` runs.
    pub fn open(
        bind: SocketAddr,
        config: &SessionConfig,
        driver: String,
        gameplay_fingerprint: u64,
        pick_validator: Option<PickValidator>,
    ) -> Result<Self, HostOpenError> {
        let mut config = config.clone();
        config.authority = SessionAuthority::Host;
        let dev = host_local_dev(&mut config.dev);
        let ad = advertise(&config)?;
        let late_join = late_join_policy(&config.mode);
        let host = Host::listen(
            bind,
            &HostConfig {
                gameplay_fingerprint,
                max_clients: MAX_PLAYERS as u16 - 1,
                pick_validator,
                // The host seat is a player — peers learn its pick
                // from `Start` so they can spawn its car.
                host_pick: Some(encode_pick(&config.vehicle)?),
                liveness_timeout: mm2_net::HOST_LIVENESS_TIMEOUT,
                peer_stall_limit: mm2_net::PEER_STALL_LIMIT,
                uplink_bytes_per_sec: mm2_net::HOST_UPLINK_BYTES_PER_SEC,
            },
        )?;
        let ctl = host.ctl();
        host.set_session(ad.clone())?;
        let (commands_tx, commands_rx) = mpsc::channel();
        Ok(Self {
            host: Mutex::new(host),
            ctl,
            commands_rx: Mutex::new(commands_rx),
            commands_tx,
            config,
            dev,
            ad,
            late_join,
            driver,
            leaving: false,
            cancel_sent: false,
            dead: false,
            log_events: false,
        })
    }

    /// The address peers dial — `127.0.0.1:<ephemeral>` by default.
    pub fn addr(&self) -> SocketAddr {
        self.host.lock().unwrap_or_else(|e| e.into_inner()).addr()
    }

    /// The UDP port the lane listens on — the port a firewall must admit
    /// besides the TCP one.
    pub fn lane_port(&self) -> u16 {
        self.host
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lane_port()
    }

    /// The host loop's control handle (`start`/`cancel`/`shutdown`).
    pub fn ctl(&self) -> &HostCtl {
        &self.ctl
    }

    /// The per-player input mailbox the lobby's reader threads fill —
    /// the host's remote cars read their `VehicleInput` from it.
    pub fn remote_inputs(&self) -> mm2_net::RemoteInputs {
        self.host
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remote_inputs()
    }

    /// Replace the session the lobby advertises for the *next* round
    /// (F26-AC05: rematch with a changed city/mode). The gates are the
    /// ones `--host` runs at flag time — `validate` then
    /// [`check_session`] against the host's own install — so a peer on
    /// the same content (the handshake fingerprint) can run what is
    /// advertised and never leaves over it. Refused while a session is
    /// live (`running`: the lobby's `generation` mirror).
    ///
    /// The host seat's *pick* and the local-only fields stay the link's:
    /// the lobby's `Start` announces the pick it was opened with, so a
    /// changed `vehicle` would disagree with what peers spawn. The seed
    /// is the caller's (the same seed replays the same seed-rolled
    /// world; mint a fresh one for a new one). The start's late-join
    /// policy follows the new mode.
    pub fn set_session(
        &mut self,
        vfs: &Vfs,
        next: &SessionConfig,
        running: bool,
    ) -> Result<(), SetSessionError> {
        if running {
            return Err(SetSessionError::SessionRunning);
        }
        let mut config = next.clone();
        config.authority = SessionAuthority::Host;
        config.vehicle = self.config.vehicle.clone();
        config.mods_active = self.config.mods_active;
        config.mods_cosmetic_only = self.config.mods_cosmetic_only;
        config.validate()?;
        check_session(vfs, &config)?;
        let ad = advertise(&config)?;
        self.host
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_session(ad.clone())?;
        self.late_join = late_join_policy(&config.mode);
        self.config = config;
        self.ad = ad;
        Ok(())
    }

    /// The advertised session's display summary.
    pub fn summary(&self) -> &str {
        &self.ad.summary
    }

    /// The session the host seat plays once `Started` lands.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// The host driver's display name.
    pub fn driver(&self) -> &str {
        &self.driver
    }

    /// Send-side handle for operator commands — clone it for the
    /// stdin thread or a test.
    pub fn command_sender(&self) -> Sender<HostCommand> {
        self.commands_tx.clone()
    }

    /// Whether the link is coming down (`leave()` ran or the loop died).
    pub fn leaving(&self) -> bool {
        self.leaving
    }

    /// Print drained host events as record lines (the headless run's
    /// operator surface — the `mm2-host` contract).
    pub fn log_events(&mut self, on: bool) {
        self.log_events = on;
    }

    /// Take the lobby down: a running session gets a `Cancel` first
    /// (queued ahead of the shutdown on the same control channel, so
    /// peers see the session end before the sockets die), then the
    /// loop is joined. Idempotent.
    pub fn leave(&mut self) {
        if self.leaving {
            return;
        }
        self.leaving = true;
        self.cancel_sent = true;
        let _ = self.ctl.cancel();
        self.host
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .shutdown();
    }

    /// Drain queued operator commands — non-blocking.
    fn drain_commands(&self) -> Vec<HostCommand> {
        let rx = self.commands_rx.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        while let Ok(command) = rx.try_recv() {
            out.push(command);
        }
        out
    }

    /// Drain the host loop's events — non-blocking. A disconnected
    /// channel means the loop died under us: `dead` marks it so the
    /// bridge can exit rather than sit on a lobby that no longer runs.
    fn drain_events(&mut self) -> Vec<HostEvent> {
        let host = self.host.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        loop {
            match host.try_recv() {
                Ok(event) => out.push(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.dead = true;
                    break;
                }
            }
        }
        out
    }
}

impl Drop for HostLink {
    fn drop(&mut self) {
        self.leave();
    }
}

/// The host-side drain system — runs once per update wherever a
/// [`HostLink`] exists, after `drive_session` so a teardown landing at
/// `Menu` this frame can settle the lobby's follow-ups immediately.
/// Operator commands go first (their verdicts ride the event channel
/// either way), then host events drive the mirror and the local
/// lifecycle, then the `Menu`-phase settlements: a parked start, the
/// session-ended auto-`Cancel`, and the queued exit.
pub fn drive_host(
    mut link: ResMut<HostLink>,
    mut lobby: ResMut<LobbyState>,
    mut session: ResMut<Session>,
    mut control: ResMut<SessionControl>,
    vfs: Res<Mm2Vfs>,
    menu: Option<Res<MenuShell>>,
    mut exit: MessageWriter<AppExit>,
) {
    // The advertised session is the link's own — seed the mirror once.
    if lobby.advertised.is_none() {
        lobby.advertised = Some(link.ad.clone());
    }
    for command in link.drain_commands() {
        match command {
            HostCommand::Start => {
                // A start queued while the lobby comes down must not
                // mint a session nobody is left to cancel.
                if !link.leaving {
                    let _ = link.ctl.start(link.late_join);
                }
            }
            HostCommand::Session(_) | HostCommand::Change(_) => {
                if link.leaving {
                    continue;
                }
                let next = match command {
                    HostCommand::Session(next) => Ok(*next),
                    HostCommand::Change(change) => change.apply(&link.config, fresh_seed()),
                    _ => unreachable!("matched above"),
                };
                let running = lobby.generation.is_some();
                let verdict = next
                    .and_then(|next| {
                        link.set_session(&vfs.0, &next, running)
                            .map_err(|e| e.to_string())
                    })
                    .map(|()| link.ad.clone());
                match verdict {
                    Ok(ad) => {
                        if link.log_events {
                            println!("event=session_changed summary={:?}", ad.summary);
                        }
                        lobby.advertised = Some(ad);
                    }
                    Err(e) => {
                        if link.log_events {
                            println!("event=session_refused reason={e:?}");
                        }
                        lobby.notice = Some(format!("next session refused: {e}"));
                    }
                }
            }
            HostCommand::Cancel => {
                link.cancel_sent = true;
                let _ = link.ctl.cancel();
            }
            HostCommand::Quit => {
                link.leave();
                lobby.pending_exit.get_or_insert(0);
            }
        }
    }
    for event in link.drain_events() {
        if link.log_events {
            println!("{}", describe_host_event(&event));
        }
        match event {
            HostEvent::Joined { id, driver, build } => {
                lobby.roster.push(RosterEntry {
                    player_id: id,
                    driver,
                    build,
                    ready: false,
                    pick: None,
                });
                lobby.roster.sort_by_key(|e| e.player_id);
            }
            HostEvent::Left { id, .. } => {
                lobby.roster.retain(|e| e.player_id != id);
            }
            HostEvent::ReadyChanged { id, ready } => {
                if let Some(entry) = lobby.roster.iter_mut().find(|e| e.player_id == id) {
                    entry.ready = ready;
                }
            }
            HostEvent::VehicleChanged { id, vehicle, paint } => {
                if let Some(entry) = lobby.roster.iter_mut().find(|e| e.player_id == id) {
                    entry.pick = Some(VehiclePick { vehicle, paint });
                }
            }
            HostEvent::VehicleRefused {
                id,
                vehicle,
                paint,
                reason,
            } => {
                let who = lobby
                    .roster
                    .iter()
                    .find(|e| e.player_id == id)
                    .map(|e| e.driver.as_str())
                    .unwrap_or("a player");
                lobby.notice = Some(format!(
                    "{who}'s pick was refused ({vehicle}:{paint}): {reason}"
                ));
            }
            HostEvent::JoinFailed { peer, reason } => {
                lobby.notice = Some(format!("rejected {peer}: {reason}"));
            }
            HostEvent::Started { generation } => {
                host_started(
                    &mut link,
                    &mut lobby,
                    &mut session,
                    &mut control,
                    generation,
                );
            }
            HostEvent::StartRefused { reason } => {
                lobby.notice = Some(reason);
            }
            HostEvent::Cancelled { generation } => {
                link.cancel_sent = false;
                cancel(&mut lobby, &mut session, &mut control, generation);
            }
        }
    }
    // A dead loop or a requested leave takes a live session down with
    // it — teardown rides the normal `Unloading → Menu` lifecycle. A
    // parked `Start` is moot once the lobby is coming down: drop it so
    // it cannot wedge the queued exit behind an unplayable session.
    if link.leaving || link.dead {
        lobby.pending_start = None;
        if !matches!(
            session.phase(),
            SessionPhase::Menu | SessionPhase::Unloading
        ) {
            control.quit = true;
        }
    }
    // The loop dying under us is the host's lost-lobby equivalent —
    // a named failure, never a silent sit. A `dead` that lands while
    // `leaving` is our own `leave()` draining dry: cover the bare-`leave`
    // path (Drop, a test) that never queued the exit itself.
    if link.dead {
        if link.leaving {
            lobby.pending_exit.get_or_insert(0);
        } else {
            lobby.notice = Some("the lobby host loop died".to_string());
            link.leave();
            lobby.pending_exit.get_or_insert(1);
        }
    }
    // Lifecycle intents settle at `Menu` — the lobby's "waiting" phase.
    if *session.phase() == SessionPhase::Menu {
        if lobby.pending_exit.is_none()
            && let Some((generation, config)) = lobby.pending_start.take()
            && let Err(e) = begin_wired(&mut session, &mut control, config, generation)
        {
            // The advertised config was flag-time gated, so a refusal
            // here is lifecycle-internal — end the lobby's session
            // rather than leave peers playing one we are not in.
            lobby.notice = Some(format!("hosted session could not begin: {e}"));
            link.cancel_sent = true;
            let _ = link.ctl.cancel();
        }
        // The hosted session ended locally (finish → quit → Menu) —
        // end it for everyone; the lobby re-opens for the next round.
        if *session.phase() == SessionPhase::Menu
            && !link.leaving
            && lobby.pending_start.is_none()
            && lobby.pending_exit.is_none()
            && lobby.generation.is_some()
            && !link.cancel_sent
        {
            link.cancel_sent = true;
            let _ = link.ctl.cancel();
        }
        if menu.is_none()
            && lobby.pending_start.is_none()
            && let Some(code) = lobby.pending_exit.take()
        {
            lobby.exit_sent = Some(code);
            exit.write(AppExit::from_code(code));
        }
    }
}

/// `Started` — the lobby minted a generation and `Start` went to every
/// peer. The host seat begins the same advertised session under the
/// same generation (authority `Host`); a `Started` that lands while a
/// previous session is still tearing down parks until `Menu` returns
/// (the loop's own gate makes this defensive — a second `start` while
/// in-session is refused before it mints). One that lands after
/// `leave` is already cancelled on the wire — begin nothing here.
fn host_started(
    link: &mut HostLink,
    lobby: &mut LobbyState,
    session: &mut Session,
    control: &mut SessionControl,
    generation: u64,
) {
    lobby.generation = Some(generation);
    link.cancel_sent = false;
    if link.leaving {
        return;
    }
    let mut config = link.config.clone();
    config.dev = link.dev.clone();
    if *session.phase() == SessionPhase::Menu {
        if let Err(e) = begin_wired(session, control, config, generation) {
            lobby.notice = Some(format!("hosted session could not begin: {e}"));
            link.cancel_sent = true;
            let _ = link.ctl.cancel();
        }
    } else {
        lobby.pending_start = Some((generation, config));
        control.quit = true;
    }
}

/// Where a menu-hosted lobby listens — the `--bind` the app was started
/// with (loopback + ephemeral port unless the operator chose a wider
/// bind), so choosing *Host lobby* in the menu never widens exposure
/// by itself.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuHostBind(pub SocketAddr);

impl Default for MenuHostBind {
    fn default() -> Self {
        Self(SocketAddr::from(([127, 0, 0, 1], 0)))
    }
}

/// Why a menu could not open a lobby for the session it configured.
#[derive(Debug, thiserror::Error)]
pub enum MenuHostError {
    /// The session fails `validate` or the install cannot run it.
    #[error("{0}")]
    Session(String),
    /// Fingerprinting the content for the handshake failed.
    #[error("fingerprinting content: {0}")]
    Fingerprint(String),
    /// Binding or advertising failed.
    #[error("{0}")]
    Open(#[from] HostOpenError),
}

/// Open a lobby for a session the menu configured, behind the gates
/// `--host` and `mm2-host` run at flag time: `validate`, then
/// [`check_session`] (world resolves, a Cops & Robbers city has its
/// site pool, an event survives `event_race_setup`), the gameplay
/// fingerprint the handshake compares, and the catalog the roster's
/// pick validator reads. A fresh seed is minted, so each hosted
/// lobby replays a different world.
pub fn open_menu_host(
    vfs: &Vfs,
    config: &SessionConfig,
    bind: SocketAddr,
    driver: String,
) -> Result<HostLink, MenuHostError> {
    let mut config = config.clone();
    config.seed = fresh_seed();
    config
        .validate()
        .map_err(|e| MenuHostError::Session(e.to_string()))?;
    check_session(vfs, &config).map_err(|e| MenuHostError::Session(e.to_string()))?;
    let fingerprint = mm2_content::fingerprint::gameplay(vfs)
        .map_err(|e| MenuHostError::Fingerprint(e.to_string()))?;
    let catalog = VehicleCatalog::scan(vfs);
    Ok(HostLink::open(
        bind,
        &config,
        driver,
        fingerprint.hash,
        Some(vehicle_validator(&catalog)),
    )?)
}

/// Hand a menu-opened lobby to the app: the link, the resources its
/// systems read beside it (removed again with it by
/// [`close_menu_host`], so a closed lobby leaves no networked state
/// behind) and the status line that is the lobby's surface.
pub fn adopt_menu_host(commands: &mut Commands, link: HostLink) {
    commands.insert_resource(link);
    commands.init_resource::<LobbyState>();
    commands.init_resource::<crate::netdrive::NetDriveReport>();
    commands.init_resource::<crate::netdrive::WireStall>();
    spawn_host_text(commands);
}

/// Spawn the hosted lobby's status line ([`HostText`]).
pub fn spawn_host_text(commands: &mut Commands) {
    commands.spawn((
        HostText,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

/// Close a menu-hosted lobby once it has come down (`Esc`, `quit`, or a
/// dead host loop): at `Menu`, with the lobby leaving and nothing left
/// to start, the link and everything that existed only for it go, and
/// `menu_watch` reopens the shell. The lobby's closing notice (its
/// last gate notice, or the bare `closed the lobby`) is handed to the
/// menu first, exactly as [`close_menu_join`] does for a joined lobby —
/// a dead host loop is a named return, not a frozen lobby. Only acts
/// under a [`MenuShell`] — a `--host` app has no menu to return to and
/// exits through [`drive_host`] instead.
#[allow(clippy::too_many_arguments)] // the resources a hosted lobby owns, by name
pub fn close_menu_host(
    mut commands: Commands,
    link: Res<HostLink>,
    mut lobby: ResMut<LobbyState>,
    session: Res<Session>,
    menu: Option<Res<MenuShell>>,
    note: Option<ResMut<crate::session::SessionNote>>,
    texts: Query<Entity, With<HostText>>,
) {
    let (Some(_), Some(mut note)) = (menu, note) else {
        return;
    };
    if !link.leaving() || *session.phase() != SessionPhase::Menu || lobby.pending_start.is_some() {
        return;
    }
    note.lobby = Some(
        lobby
            .notice
            .take()
            .unwrap_or_else(|| "closed the lobby".to_string()),
    );
    for text in &texts {
        commands.entity(text).despawn();
    }
    commands.remove_resource::<HostLink>();
    commands.remove_resource::<LobbyState>();
    commands.remove_resource::<crate::netdrive::NetDriveReport>();
    commands.remove_resource::<crate::netdrive::WireStall>();
}

/// Why a menu could not join the lobby at an address.
#[derive(Debug, thiserror::Error)]
pub enum MenuJoinError {
    /// Fingerprinting the content for the handshake failed.
    #[error("fingerprinting content: {0}")]
    Fingerprint(String),
    /// The driver name or car pick cannot travel on the wire.
    #[error("{0}")]
    Pick(String),
    /// The dial or handshake failed (unreachable, refused, mismatched).
    #[error("{0}")]
    Join(#[from] NetError),
    /// The dial thread ended without reporting (it panicked).
    #[error("the dial ended without an answer")]
    Lost,
    /// The player backed out of the address field before the host answered.
    #[error("cancelled")]
    Cancelled,
}

/// What a menu join sends once the socket is up: the handshake `Hello`
/// and the car pick offered to the roster behind it.
struct JoinPlan {
    hello: Hello,
    pick: VehiclePick,
    mods_active: bool,
}

/// Validate and fingerprint everything a join sends, before any socket
/// is touched: the checks that need the VFS and the menu's selection.
fn plan_menu_join(
    vfs: &Vfs,
    driver: String,
    mods_active: bool,
    vehicle: &VehicleSelection,
) -> Result<JoinPlan, MenuJoinError> {
    if driver.len() > mm2_net::MAX_STRING {
        return Err(MenuJoinError::Pick(format!(
            "driver name is {} bytes; the wire bound is {}",
            driver.len(),
            mm2_net::MAX_STRING
        )));
    }
    let pick = encode_pick(vehicle).map_err(|e| MenuJoinError::Pick(e.to_string()))?;
    let fingerprint = mm2_content::fingerprint::gameplay(vfs)
        .map_err(|e| MenuJoinError::Fingerprint(e.to_string()))?;
    Ok(JoinPlan {
        hello: mm2_net::hello(crate::smoke::COMMIT.to_string(), driver, fingerprint.hash),
        pick,
        mods_active,
    })
}

/// Dial `addr` and run the fingerprint-bound handshake `--join` runs,
/// then offer the pick at once: the roster shows what we will drive and
/// the host's catalog gate confirms it can spawn. Blocks up to
/// `HANDSHAKE_TIMEOUT` on a silent peer, so the menu runs it on
/// [`MenuDial`]'s thread. A refused or unreachable lobby is a named
/// error for the menu's status line, never a silent local session.
fn dial_menu_join(addr: SocketAddr, plan: JoinPlan) -> Result<LobbyLink, MenuJoinError> {
    let mut link = LobbyLink::join(addr, &plan.hello, plan.mods_active, DevOverrides::default())?;
    let _ = link.set_vehicle(&plan.pick.vehicle, plan.pick.paint);
    Ok(link)
}

/// A menu join in flight: the dial runs on its own thread so a silent
/// host (the handshake waits up to `HANDSHAKE_TIMEOUT`) cannot freeze
/// the window, and `menu_dial` adopts the answer when it lands. Present
/// exactly while a dial is unanswered; the menu refuses a second lobby,
/// or a launch, meanwhile.
#[derive(Resource)]
pub struct MenuDial {
    addr: SocketAddr,
    answer: Mutex<Receiver<Result<LobbyLink, MenuJoinError>>>,
    cancelled: AtomicBool,
}

impl MenuDial {
    /// The address being dialled.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Abandon the dial: the next [`poll`](Self::poll) answers
    /// [`MenuJoinError::Cancelled`]. The thread stays bounded by the
    /// handshake timeout; its late answer is dropped with the resource.
    pub fn cancel(&self) {
        self.cancelled.store(true, AtomicOrdering::Release);
    }

    /// The dial's answer once it has landed, `None` while it is still
    /// in flight. A dial thread that died without answering is
    /// [`MenuJoinError::Lost`], not an eternal wait.
    pub fn poll(&self) -> Option<Result<LobbyLink, MenuJoinError>> {
        // A cancelled dial answers `Cancelled` whatever the thread found,
        // so a host that replies after the player left the field is never
        // adopted; removing the resource then drops the late link.
        if self.cancelled.load(AtomicOrdering::Acquire) {
            return Some(Err(MenuJoinError::Cancelled));
        }
        let rx = self.answer.lock().ok()?;
        match rx.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(MenuJoinError::Lost)),
        }
    }
}

/// Start the menu's join without waiting for the handshake: the checks
/// that need the VFS run here (their errors are immediate), the socket
/// work moves to a thread the returned [`MenuDial`] reads back.
pub fn begin_menu_dial(
    vfs: &Vfs,
    addr: SocketAddr,
    driver: String,
    mods_active: bool,
    vehicle: &VehicleSelection,
) -> Result<MenuDial, MenuJoinError> {
    let plan = plan_menu_join(vfs, driver, mods_active, vehicle)?;
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("mm2-menu-dial".to_string())
        .spawn(move || {
            let _ = tx.send(dial_menu_join(addr, plan));
        })
        .map_err(NetError::from)?;
    Ok(MenuDial {
        addr,
        answer: Mutex::new(rx),
        cancelled: AtomicBool::new(false),
    })
}

/// Hand a menu-joined lobby to the app: the link plus the resources the
/// client systems read beside it (removed again by [`close_menu_join`])
/// and the status line that is the lobby's surface.
pub fn adopt_menu_join(commands: &mut Commands, link: LobbyLink) {
    commands.insert_resource(link);
    commands.init_resource::<LobbyState>();
    commands.init_resource::<crate::netdrive::RemoteSnaps>();
    commands.init_resource::<crate::netdrive::InputSeq>();
    commands.init_resource::<crate::netdrive::NetDriveReport>();
    spawn_lobby_text(commands);
}

/// Spawn the joined lobby's status line ([`LobbyText`]).
pub fn spawn_lobby_text(commands: &mut Commands) {
    commands.spawn((
        LobbyText,
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

/// Close a menu-joined lobby once it has ended (`Esc`, or a lost host):
/// at `Menu`, with `drive_lobby` having queued the exit it would
/// otherwise write as `AppExit` and nothing left to start, the link and
/// everything that existed only for it go, and `menu_watch` reopens the
/// shell carrying why. Only acts under a [`MenuShell`] — a `--join` app
/// has no menu to return to and exits through [`drive_lobby`] instead.
#[allow(clippy::too_many_arguments)] // the resources a joined lobby owns, by name
pub fn close_menu_join(
    mut commands: Commands,
    mut lobby: ResMut<LobbyState>,
    session: Res<Session>,
    menu: Option<Res<MenuShell>>,
    note: Option<ResMut<crate::session::SessionNote>>,
    texts: Query<Entity, With<LobbyText>>,
) {
    let (Some(_), Some(mut note)) = (menu, note) else {
        return;
    };
    if lobby.pending_exit.is_none()
        || lobby.pending_start.is_some()
        || *session.phase() != SessionPhase::Menu
    {
        return;
    }
    note.lobby = Some(
        lobby
            .notice
            .take()
            .unwrap_or_else(|| "left the lobby".to_string()),
    );
    for text in &texts {
        commands.entity(text).despawn();
    }
    commands.remove_resource::<LobbyLink>();
    commands.remove_resource::<LobbyState>();
    commands.remove_resource::<crate::netdrive::RemoteSnaps>();
    commands.remove_resource::<crate::netdrive::InputSeq>();
    commands.remove_resource::<crate::netdrive::NetDriveReport>();
}

/// The windowed host lobby's keyboard surface — `Enter` requests the
/// start (the lobby's own gate answers `Started`/`StartRefused`),
/// `Esc` takes the lobby down. Both ride the command channel the
/// stdin driver feeds, so the keys and the operator words share one
/// intake. Only live while the session parks at `Menu`. A lobby the
/// menu opened this frame ignores the keypress that opened it.
pub fn host_input(keys: Res<ButtonInput<KeyCode>>, session: Res<Session>, link: Res<HostLink>) {
    if *session.phase() != SessionPhase::Menu || link.leaving() || link.is_added() {
        return;
    }
    if keys.just_pressed(KeyCode::Enter) {
        let _ = link.command_sender().send(HostCommand::Start);
    }
    if keys.just_pressed(KeyCode::Escape) {
        let _ = link.command_sender().send(HostCommand::Quit);
    }
}

/// Marker for the hosted lobby's status line — the counterpart of
/// [`LobbyText`], spawned by the app itself (the lobby is `Menu`-time
/// state, so no session-owned UI can show it).
#[derive(Component)]
pub struct HostText;

/// The camera a lobby's status line renders through. At `Menu` no world
/// camera exists, and a menu that opened or joined a lobby has closed
/// and taken its own camera with it — without this one the window
/// keeps its last frame and the lobby looks frozen.
#[derive(Component)]
pub struct LobbyCamera;

/// Either lobby's status line.
type LobbyLine = Or<(With<HostText>, With<LobbyText>)>;

/// Keep exactly one [`LobbyCamera`] while a lobby line ([`HostText`] or
/// [`LobbyText`]) is up at `Menu` and the menu has no camera of its own;
/// despawn it otherwise, so the menu's and the world's cameras never
/// share the screen with it.
pub fn lobby_camera(
    mut commands: Commands,
    session: Res<Session>,
    lines: Query<(), LobbyLine>,
    menu: Query<(), With<crate::menu::MenuCamera>>,
    cameras: Query<Entity, With<LobbyCamera>>,
) {
    let wanted = *session.phase() == SessionPhase::Menu && !lines.is_empty() && menu.is_empty();
    let mut existing = cameras.iter();
    if wanted && existing.next().is_none() {
        commands.spawn((LobbyCamera, Camera2d));
    }
    // Past the one kept when wanted, every camera goes.
    for camera in existing {
        commands.entity(camera).despawn();
    }
}

/// The hosted lobby's status line — where peers dial, what the lobby
/// runs, the remote roster's readiness, and the latest gate notice.
/// The line empties while a session runs — the HUD owns the screen.
pub fn drive_host_text(
    link: Res<HostLink>,
    lobby: Res<LobbyState>,
    session: Res<Session>,
    mut texts: Query<&mut Text, With<HostText>>,
) {
    let Ok(mut text) = texts.single_mut() else {
        return;
    };
    if *session.phase() != SessionPhase::Menu {
        text.0.clear();
        return;
    }
    let pick = encode_pick(&link.config.vehicle).unwrap_or(VehiclePick {
        vehicle: String::new(),
        paint: 0,
    });
    text.0 = crate::lobby_view::LobbyView::host(
        link.addr(),
        link.driver(),
        &pick,
        &lobby.roster,
        link.summary(),
        lobby.notice.as_deref(),
    )
    .with_bots(link.config().cnr_bots)
    .render();
}

/// The display line for lobby UIs/CLIs (`"sf, circuit:3, professional"`).
fn summarize(config: &SessionConfig) -> String {
    let world = match &config.world {
        WorldMode::DevWorld => "dev world".to_string(),
        WorldMode::City { psdl } => psdl
            .strip_prefix("city/")
            .and_then(crate::city::psdl_stem)
            .unwrap_or(psdl)
            .to_string(),
    };
    let mode = match &config.mode {
        SessionMode::Cruise => "cruise".to_string(),
        SessionMode::Event(r) => format!("{}:{}", r.table.stem_prefix(), r.index),
        SessionMode::CopsAndRobbers(c) => format!("cops & robbers, {}", variant_label(c.variant)),
    };
    let bots = match config.cnr_bots {
        0 => String::new(),
        1 => ", 1 lobby bot".to_string(),
        n => format!(", {n} lobby bots"),
    };
    format!(
        "{}, {}{}, {}",
        world,
        mode,
        bots,
        config.difficulty.as_str()
    )
}

/// Whether a session of this mode takes joins once started (MP-5): an
/// event's roster is fixed at the start, while Cruise and Cops &
/// Robbers stay open ("join/leave at any time"). A late joiner to a
/// match is seated by [`crate::cnr::enroll_cnr_participants`] as soon as
/// its car appears on the authority, and learns the match from the next
/// repeating whole-view frame ([`crate::cnrnet`]) — there is no
/// join-time unicast to wait for.
pub fn late_join_policy(mode: &SessionMode) -> LateJoin {
    match mode {
        SessionMode::Cruise | SessionMode::CopsAndRobbers(_) => LateJoin::Open,
        SessionMode::Event(_) => LateJoin::Closed,
    }
}

/// `city/<stem>.psdl` → `<stem>`.
pub(crate) fn city_stem(psdl: &str) -> Option<&str> {
    crate::city::psdl_stem(psdl.strip_prefix("city/")?)
}

fn variant_label(variant: CnrVariant) -> &'static str {
    match variant {
        CnrVariant::FreeForAll => "free for all",
        CnrVariant::CopsVsRobbers => "cops vs robbers",
        CnrVariant::RobbersVsRobbers => "robbers vs robbers",
    }
}

/// The `params` payload layout: a session's full configuration minus
/// per-player and local-only fields.
#[derive(Debug, Serialize, Deserialize)]
struct SessionParams {
    world: WorldParams,
    mode: ModeParams,
    difficulty: Difficulty,
    conditions: ConditionsParams,
    densities: DensitiesParams,
    customization: Option<CustomizationParams>,
    seed: u64,
    /// The host's Cops & Robbers lobby bot count (protocol v31). Absent
    /// is zero, and only a Cops & Robbers session may carry more.
    #[serde(default)]
    cnr_bots: u8,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WorldParams {
    DevWorld,
    City { psdl: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ModeParams {
    Cruise,
    Event {
        city: String,
        table: EventTableKind,
        index: usize,
    },
    CopsAndRobbers {
        variant: CnrVariantParams,
        gold_mass: GoldMassParams,
        limit: CnrLimitParams,
    },
}

/// Cops & Robbers choices travel by name, not by the executable's
/// numbering: the variant numbering is only inferred (ledger CNR-7) and a
/// name cannot silently mean a different rule if that is ever corrected.
/// An unknown name fails the decode.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CnrVariantParams {
    FreeForAll,
    CopsVsRobbers,
    RobbersVsRobbers,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GoldMassParams {
    Weightless,
    QuarterTon,
    HalfTon,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CnrLimitParams {
    None,
    Minutes { minutes: u32 },
    Points { points: u32 },
}

impl From<&CnrSettings> for ModeParams {
    fn from(c: &CnrSettings) -> Self {
        ModeParams::CopsAndRobbers {
            variant: match c.variant {
                CnrVariant::FreeForAll => CnrVariantParams::FreeForAll,
                CnrVariant::CopsVsRobbers => CnrVariantParams::CopsVsRobbers,
                CnrVariant::RobbersVsRobbers => CnrVariantParams::RobbersVsRobbers,
            },
            gold_mass: match c.gold_mass {
                GoldMass::Weightless => GoldMassParams::Weightless,
                GoldMass::QuarterTon => GoldMassParams::QuarterTon,
                GoldMass::HalfTon => GoldMassParams::HalfTon,
            },
            limit: match c.limit {
                MatchLimit::None => CnrLimitParams::None,
                MatchLimit::Minutes(minutes) => CnrLimitParams::Minutes { minutes },
                MatchLimit::Points(points) => CnrLimitParams::Points { points },
            },
        }
    }
}

/// Selectors travel as raw `u8`s; [`accept`] re-bounds them through
/// `TimeOfDay::new`/`Weather::new` so a hostile or stale blob cannot
/// inject an out-of-range index.
#[derive(Debug, Serialize, Deserialize)]
struct ConditionsParams {
    time_of_day: u8,
    weather: u8,
}

#[derive(Debug, Serialize, Deserialize)]
struct DensitiesParams {
    traffic: f32,
    pedestrians: f32,
    /// Absent from advertisements that predate the cop-density pick:
    /// the whole authored lineup, today's behaviour.
    #[serde(default = "full_cop_density")]
    cops: f32,
}

fn full_cop_density() -> f32 {
    Densities::DEFAULT.cops
}

#[derive(Debug, Serialize, Deserialize)]
struct CustomizationParams {
    conditions: ConditionsParams,
    densities: DensitiesParams,
    race: Option<RaceParams>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RaceParams {
    laps: u32,
    opponents: u32,
}

impl From<&SessionConfig> for SessionParams {
    fn from(config: &SessionConfig) -> Self {
        Self {
            world: match &config.world {
                WorldMode::DevWorld => WorldParams::DevWorld,
                WorldMode::City { psdl } => WorldParams::City { psdl: psdl.clone() },
            },
            mode: match &config.mode {
                SessionMode::Cruise => ModeParams::Cruise,
                SessionMode::Event(r) => ModeParams::Event {
                    city: r.city.clone(),
                    table: r.table,
                    index: r.index,
                },
                SessionMode::CopsAndRobbers(c) => ModeParams::from(c),
            },
            difficulty: config.difficulty,
            conditions: ConditionsParams::from(config.conditions),
            densities: DensitiesParams::from(config.densities),
            customization: config.customization.as_ref().map(|c| CustomizationParams {
                conditions: ConditionsParams::from(c.conditions),
                densities: DensitiesParams::from(c.densities),
                race: c.race.map(|r| RaceParams {
                    laps: r.laps,
                    opponents: r.opponents,
                }),
            }),
            seed: config.seed,
            cnr_bots: config.cnr_bots,
        }
    }
}

impl From<SessionConditions> for ConditionsParams {
    fn from(c: SessionConditions) -> Self {
        Self {
            time_of_day: c.time_of_day.get(),
            weather: c.weather.get(),
        }
    }
}

impl From<Densities> for DensitiesParams {
    fn from(d: Densities) -> Self {
        Self {
            traffic: d.traffic,
            pedestrians: d.pedestrians,
            cops: d.cops,
        }
    }
}

impl SessionParams {
    fn into_config(self) -> Result<SessionConfig, SessionWireError> {
        Ok(SessionConfig {
            world: match self.world {
                WorldParams::DevWorld => WorldMode::DevWorld,
                WorldParams::City { psdl } => WorldMode::City { psdl },
            },
            mode: match self.mode {
                ModeParams::Cruise => SessionMode::Cruise,
                ModeParams::Event { city, table, index } => {
                    SessionMode::Event(EventRef { city, table, index })
                }
                ModeParams::CopsAndRobbers {
                    variant,
                    gold_mass,
                    limit,
                } => SessionMode::CopsAndRobbers(CnrSettings {
                    variant: match variant {
                        CnrVariantParams::FreeForAll => CnrVariant::FreeForAll,
                        CnrVariantParams::CopsVsRobbers => CnrVariant::CopsVsRobbers,
                        CnrVariantParams::RobbersVsRobbers => CnrVariant::RobbersVsRobbers,
                    },
                    gold_mass: match gold_mass {
                        GoldMassParams::Weightless => GoldMass::Weightless,
                        GoldMassParams::QuarterTon => GoldMass::QuarterTon,
                        GoldMassParams::HalfTon => GoldMass::HalfTon,
                    },
                    limit: match limit {
                        CnrLimitParams::None => MatchLimit::None,
                        CnrLimitParams::Minutes { minutes } => MatchLimit::Minutes(minutes),
                        CnrLimitParams::Points { points } => MatchLimit::Points(points),
                    },
                }),
            },
            difficulty: self.difficulty,
            conditions: self.conditions.into_conditions()?,
            densities: self.densities.into_densities(),
            customization: self
                .customization
                .map(|c| {
                    Ok::<_, SessionWireError>(SessionCustomization {
                        conditions: c.conditions.into_conditions()?,
                        densities: c.densities.into_densities(),
                        race: c.race.map(|r| RaceCustomization {
                            laps: r.laps,
                            opponents: r.opponents,
                        }),
                    })
                })
                .transpose()?,
            seed: self.seed,
            // Per-player and local-only fields never ride the wire.
            vehicle: VehicleSelection::default(),
            authority: SessionAuthority::Remote,
            mods_active: false,
            mods_cosmetic_only: false,
            cnr_bots: self.cnr_bots,
            dev: DevOverrides::default(),
        })
    }
}

impl ConditionsParams {
    fn into_conditions(self) -> Result<SessionConditions, SelectorError> {
        Ok(SessionConditions {
            time_of_day: TimeOfDay::new(self.time_of_day)?,
            weather: Weather::new(self.weather)?,
        })
    }
}

impl DensitiesParams {
    fn into_densities(self) -> Densities {
        Densities {
            traffic: self.traffic,
            pedestrians: self.pedestrians,
            cops: self.cops,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_game::SpawnPose;
    use serde_json::json;

    fn change(line: &str) -> Result<SessionChange, String> {
        SessionChange::parse(line.split_whitespace())
    }

    /// `bots=` sets the lobby's Cops & Robbers bot count; leaving the
    /// mode drops it, and naming it for another mode is refused.
    #[test]
    fn a_session_edit_carries_the_lobby_bot_count_for_cops_and_robbers_only() {
        let cnr = SessionConfig {
            world: WorldMode::City {
                psdl: "city/sf.psdl".into(),
            },
            mode: SessionMode::CopsAndRobbers(CnrSettings::default()),
            ..SessionConfig::default()
        };
        let with = change("bots=4").unwrap().apply(&cnr, 1).unwrap();
        assert_eq!(with.cnr_bots, 4);
        assert_eq!(
            change("limit=none cnr=ffa")
                .unwrap()
                .apply(&with, 1)
                .unwrap()
                .cnr_bots,
            4,
            "an edit that does not name bots keeps them"
        );
        let cruise = change("event=none").unwrap().apply(&with, 1).unwrap();
        assert_eq!(cruise.cnr_bots, 0);
        assert!(cruise.validate().is_ok());
        assert!(
            change("event=none bots=2")
                .unwrap()
                .apply(&with, 1)
                .is_err()
        );
        assert!(change("bots=x").is_err());
        let full = change("bots=9").unwrap().apply(&cnr, 1).unwrap();
        let host = SessionConfig {
            authority: SessionAuthority::Host,
            ..full
        };
        assert!(matches!(
            host.validate(),
            Err(ConfigError::CnrLobbyBotsFull(9))
        ));
    }

    /// The operator's words become exactly the fields they name; every
    /// unnamed field keeps the host's current value (the vehicle pick,
    /// the conditions the operator did not touch) and only the seed is
    /// re-rolled.
    #[test]
    fn a_session_edit_changes_only_what_it_names() {
        let base = SessionConfig {
            world: WorldMode::City {
                psdl: "city/london.psdl".into(),
            },
            difficulty: Difficulty::Professional,
            conditions: SessionConditions {
                time_of_day: TimeOfDay::new(2).unwrap(),
                weather: Weather::new(1).unwrap(),
            },
            seed: 7,
            ..SessionConfig::default()
        };
        let next = change("city=SF event=blitz:3 weather=3")
            .unwrap()
            .apply(&base, 99)
            .unwrap();
        assert_eq!(
            next.world,
            WorldMode::City {
                psdl: "city/sf.psdl".into()
            }
        );
        assert_eq!(
            next.mode,
            SessionMode::Event(EventRef {
                city: "sf".into(),
                table: EventTableKind::Blitz,
                index: 3
            })
        );
        assert_eq!(next.conditions.weather.get(), 3);
        // Untouched: difficulty and time of day.
        assert_eq!(next.difficulty, Difficulty::Professional);
        assert_eq!(next.conditions.time_of_day.get(), 2);
        // Unnamed seed → the caller's fresh one; a named one wins.
        assert_eq!(next.seed, 99);
        let again = change("seed=5 difficulty=amateur")
            .unwrap()
            .apply(&base, 99)
            .unwrap();
        assert_eq!((again.seed, again.difficulty), (5, Difficulty::Amateur));
        assert_eq!(again.world, base.world);
    }

    /// Cops & Robbers rides the `--cnr` vocabulary, `event=none` returns
    /// to cruise, and an event reads the dev world as London (the CLI's
    /// `--event` default).
    #[test]
    fn a_session_edit_picks_the_mode_in_the_cli_vocabulary() {
        let base = SessionConfig::default();
        let cnr = change("cnr=cops gold=half limit=5m")
            .unwrap()
            .apply(&base, 1)
            .unwrap();
        assert_eq!(
            cnr.mode,
            SessionMode::CopsAndRobbers(
                CnrSettings::parse("cops", Some("half"), Some("5m")).unwrap()
            )
        );
        let back = change("event=none").unwrap().apply(&cnr, 1).unwrap();
        assert_eq!(back.mode, SessionMode::Cruise);
        let dev_event = change("event=race:0").unwrap().apply(&base, 1).unwrap();
        assert_eq!(
            dev_event.mode,
            SessionMode::Event(EventRef {
                city: "london".into(),
                table: EventTableKind::Checkpoint,
                index: 0
            })
        );
    }

    /// An event session cannot be moved to another city by accident: the
    /// row belongs to its city, so the operator must name the new one.
    #[test]
    fn moving_an_event_session_to_another_city_needs_a_new_event() {
        let base = SessionConfig {
            world: WorldMode::City {
                psdl: "city/london.psdl".into(),
            },
            mode: SessionMode::Event(EventRef::parse("blitz:1", "london").unwrap()),
            ..SessionConfig::default()
        };
        let err = change("city=sf").unwrap().apply(&base, 1).unwrap_err();
        assert!(err.contains("belongs to its city"), "{err}");
        // Cruise has no row to strand, so a bare city change is fine.
        let cruise = SessionConfig::default();
        assert!(change("city=sf").unwrap().apply(&cruise, 1).is_ok());
    }

    /// Typos never re-advertise something else: unknown/repeated keys,
    /// bad values, out-of-range selectors, conflicting modes and the
    /// empty edit are all named errors, and a city stem cannot smuggle a
    /// path into the VFS lookup.
    #[test]
    fn a_malformed_session_edit_is_named_not_guessed() {
        for bad in [
            "",
            "city",
            "colour=red",
            "weather=4",
            "weather=x",
            "tod=9",
            "seed=-1",
            "difficulty=hard",
            "seed=1 seed=2",
            "gold=half",
            "limit=5m",
            "cnr=cops event=blitz:1",
            "event=none cnr=cops",
            "cnr=nonsense",
            "city=../x",
            "city=a/b",
            "city=",
        ] {
            assert!(change(bad).is_err(), "{bad:?} should be refused");
        }
        // A well-formed word with a malformed event only fails on apply,
        // where the city it belongs to is known.
        let c = change("event=blitz").unwrap();
        assert!(c.apply(&SessionConfig::default(), 1).is_err());
        assert!(
            change("event=banana:1")
                .unwrap()
                .apply(&SessionConfig::default(), 1)
                .is_err()
        );
    }

    /// The rematch race: a cancelled session's teardown leaves its
    /// `quit` queued (the `Menu` arm of `drive_session` consumes it a
    /// frame later), and a `Start` landing in that window begins the
    /// next session at once. The new session must not inherit the dead
    /// one's intents — a stale `quit` would end it the moment it went
    /// live (found by the two-process rematch leg, `net_drive`).
    #[test]
    fn a_wired_begin_drops_the_previous_sessions_queued_intents() {
        let mut session = Session::default();
        let mut control = SessionControl {
            quit: true,
            restart: true,
            next_event: None,
            pause: true,
            ..Default::default()
        };
        begin_wired(&mut session, &mut control, SessionConfig::default(), 4).unwrap();
        assert_eq!(*session.phase(), SessionPhase::Loading);
        assert_eq!(session.wire_generation(), 4);
        assert!(!control.quit && !control.restart && !control.pause);
    }

    /// A refused begin changes nothing: the intents belong to the
    /// session that is still there.
    #[test]
    fn a_refused_wired_begin_keeps_the_queued_intents() {
        let mut session = Session::default();
        let mut control = SessionControl {
            quit: true,
            ..Default::default()
        };
        // Generation 0 is the at-rest value no lobby mints.
        assert!(begin_wired(&mut session, &mut control, SessionConfig::default(), 0).is_err());
        assert!(control.quit);
        assert_eq!(*session.phase(), SessionPhase::Menu);
    }

    fn lobby_cameras(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<(), With<LobbyCamera>>()
            .iter(app.world())
            .count()
    }

    /// A lobby line at `Menu` with the menu closed gets one camera to
    /// render through — the window froze without it — and gives it up
    /// once the menu has its own camera again or the line is gone.
    #[test]
    fn a_lobby_line_at_menu_keeps_one_camera() {
        let mut app = App::new();
        app.init_resource::<Session>();
        app.add_systems(Update, lobby_camera);
        let line = app.world_mut().spawn(HostText).id();
        app.update();
        app.update();
        assert_eq!(lobby_cameras(&mut app), 1);

        let menu = app.world_mut().spawn(crate::menu::MenuCamera).id();
        app.update();
        assert_eq!(lobby_cameras(&mut app), 0);

        app.world_mut().despawn(menu);
        app.world_mut().despawn(line);
        let line = app.world_mut().spawn(LobbyText).id();
        app.update();
        assert_eq!(lobby_cameras(&mut app), 1);

        app.world_mut().despawn(line);
        app.update();
        assert_eq!(lobby_cameras(&mut app), 0);
    }

    fn city_event_config() -> SessionConfig {
        SessionConfig {
            world: WorldMode::City {
                psdl: "city/sf.psdl".to_string(),
            },
            mode: SessionMode::Event(EventRef {
                city: "sf".to_string(),
                table: EventTableKind::Circuit,
                index: 3,
            }),
            difficulty: Difficulty::Professional,
            conditions: SessionConditions {
                time_of_day: TimeOfDay::new(2).unwrap(),
                weather: Weather::new(3).unwrap(),
            },
            densities: Densities {
                traffic: 0.25,
                pedestrians: 0.75,
                cops: 0.5,
            },
            customization: Some(SessionCustomization {
                conditions: SessionConditions {
                    time_of_day: TimeOfDay::new(1).unwrap(),
                    weather: Weather::new(0).unwrap(),
                },
                densities: Densities {
                    traffic: 0.0,
                    pedestrians: 1.0,
                    cops: 0.5,
                },
                race: Some(RaceCustomization {
                    laps: 4,
                    opponents: 5,
                }),
            }),
            seed: 0xfeed_beef,
            authority: SessionAuthority::Host,
            ..SessionConfig::default()
        }
    }

    #[test]
    fn a_dev_world_cruise_roundtrips() {
        let config = SessionConfig {
            seed: 42,
            ..SessionConfig::default()
        };
        let ad = advertise(&config).unwrap();
        assert_eq!(ad.summary, "dev world, cruise, amateur");
        assert!(!ad.params.is_empty());

        let back = accept(&ad).unwrap();
        assert_eq!(back.world, WorldMode::DevWorld);
        assert_eq!(back.mode, SessionMode::Cruise);
        assert_eq!(back.seed, 42);
        // Stamped by `accept`, not carried by the wire.
        assert_eq!(back.authority, SessionAuthority::Remote);
        assert_eq!(back.vehicle, VehicleSelection::default());
        assert!(!back.mods_active);
        assert_eq!(back.dev, DevOverrides::default());
    }

    #[test]
    fn a_full_config_roundtrips() {
        let config = city_event_config();
        let ad = advertise(&config).unwrap();
        assert_eq!(ad.summary, "sf, circuit:3, professional");
        let back = accept(&ad).unwrap();
        assert_eq!(back.world, config.world);
        assert_eq!(back.mode, config.mode);
        assert_eq!(back.difficulty, config.difficulty);
        assert_eq!(back.conditions, config.conditions);
        assert_eq!(back.densities, config.densities);
        assert_eq!(back.customization, config.customization);
        assert_eq!(back.seed, config.seed);
        assert_eq!(back.authority, SessionAuthority::Remote);
    }

    /// `DevOverrides` are never network-legal: a config carrying any
    /// refuses to advertise rather than silently dropping them.
    #[test]
    fn developer_overrides_are_never_advertised() {
        let mut config = SessionConfig::default();
        config.dev.traction = Some(0.9);
        assert!(matches!(
            advertise(&config),
            Err(SessionWireError::DevOverrides)
        ));
        let mut config = SessionConfig::default();
        config.dev.spawn = Some(SpawnPose {
            position: bevy::prelude::Vec3::ZERO,
            yaw: 0.0,
        });
        assert!(matches!(
            advertise(&config),
            Err(SessionWireError::DevOverrides)
        ));
    }

    #[test]
    fn an_invalid_host_config_is_refused() {
        let mut config = SessionConfig::default();
        config.densities.traffic = 2.0;
        assert!(matches!(
            advertise(&config),
            Err(SessionWireError::Invalid(ConfigError::Density { .. }))
        ));
    }

    fn cnr_config(settings: CnrSettings) -> SessionConfig {
        SessionConfig {
            world: WorldMode::City {
                psdl: "city/sf.psdl".to_string(),
            },
            mode: SessionMode::CopsAndRobbers(settings),
            seed: 77,
            ..SessionConfig::default()
        }
    }

    /// Every variant × gold mass × limit choice the lobby can make
    /// survives the wire (F27-B.4): the client rebuilds the host's exact
    /// match settings, so a lobby choice and the rules the host enforces
    /// cannot drift apart.
    #[test]
    fn every_cops_and_robbers_choice_round_trips() {
        let mut seen = 0;
        for variant in CnrVariant::ALL {
            for gold_mass in GoldMass::ALL {
                for limit in MatchLimit::choices() {
                    let settings = CnrSettings {
                        variant,
                        gold_mass,
                        limit,
                    };
                    let ad = advertise(&cnr_config(settings)).unwrap();
                    assert!(ad.summary.contains("cops & robbers"), "{}", ad.summary);
                    let back = accept(&ad).unwrap();
                    assert_eq!(back.mode, SessionMode::CopsAndRobbers(settings));
                    assert_eq!(back.world, cnr_config(settings).world);
                    assert_eq!(back.seed, 77);
                    seen += 1;
                }
            }
        }
        assert_eq!(seen, 3 * 3 * 9, "the whole option grid, none skipped");
    }

    /// The host refuses to advertise a match the world cannot host, and
    /// a blob that smuggles one in is refused on decode.
    #[test]
    fn a_cops_and_robbers_match_needs_a_city_and_a_sane_limit() {
        let mut dev = cnr_config(CnrSettings::default());
        dev.world = WorldMode::DevWorld;
        assert!(matches!(
            advertise(&dev),
            Err(SessionWireError::Invalid(
                ConfigError::CopsAndRobbersNeedsCity
            ))
        ));
        for limit in [
            MatchLimit::Minutes(0),
            MatchLimit::Points(0),
            MatchLimit::Minutes(u32::MAX),
            MatchLimit::Points(u32::MAX),
        ] {
            let config = cnr_config(CnrSettings {
                limit,
                ..CnrSettings::default()
            });
            assert!(
                matches!(
                    advertise(&config),
                    Err(SessionWireError::Invalid(
                        ConfigError::CopsAndRobbersLimit(l)
                    )) if l == limit
                ),
                "{limit:?}"
            );
        }
        // The same limit hand-written into a blob is refused by `accept`.
        let mut params =
            serde_json::to_value(SessionParams::from(&cnr_config(CnrSettings::default()))).unwrap();
        params["mode"]["limit"] = json!({"kind": "points", "points": 0});
        let ad = SessionAdvertisement {
            summary: "x".to_string(),
            params: serde_json::to_vec(&params).unwrap(),
        };
        assert!(matches!(
            accept(&ad),
            Err(SessionWireError::Invalid(ConfigError::CopsAndRobbersLimit(
                MatchLimit::Points(0)
            )))
        ));
    }

    /// Choices travel by name: an unnamed variant, mass or limit kind
    /// fails the decode instead of becoming some other rule.
    #[test]
    fn an_unnamed_cops_and_robbers_choice_does_not_decode() {
        let base =
            serde_json::to_value(SessionParams::from(&cnr_config(CnrSettings::default()))).unwrap();
        for (pointer, value) in [
            ("/mode/variant", json!("capture_the_flag")),
            ("/mode/variant", json!(1)),
            ("/mode/gold_mass", json!("one_ton")),
            ("/mode/limit", json!({"kind": "laps", "laps": 3})),
            ("/mode/limit", json!({"kind": "minutes"})),
            ("/mode/limit", json!({"kind": "minutes", "minutes": -5})),
        ] {
            let mut params = base.clone();
            *params.pointer_mut(pointer).unwrap() = value.clone();
            let ad = SessionAdvertisement {
                summary: "x".to_string(),
                params: serde_json::to_vec(&params).unwrap(),
            };
            assert!(
                matches!(accept(&ad), Err(SessionWireError::Params(_))),
                "{pointer}={value} must not decode"
            );
        }
    }

    /// MP-5 policy: Cruise and Cops & Robbers stay open once started;
    /// an event's roster is fixed at the start.
    #[test]
    fn cruise_and_cops_and_robbers_stay_joinable_but_events_do_not() {
        assert_eq!(late_join_policy(&SessionMode::Cruise), LateJoin::Open);
        assert_eq!(
            late_join_policy(&SessionMode::CopsAndRobbers(CnrSettings::default())),
            LateJoin::Open
        );
        assert_eq!(
            late_join_policy(&SessionMode::Event(EventRef {
                city: "sf".into(),
                table: EventTableKind::Blitz,
                index: 0,
            })),
            LateJoin::Closed
        );
    }

    #[test]
    fn garbage_params_do_not_decode() {
        for params in [
            b"not json".to_vec(),
            b"{}".to_vec(),
            b"{\"world\":1}".to_vec(),
            vec![0xff, 0x00],
        ] {
            let ad = SessionAdvertisement {
                summary: "x".to_string(),
                params,
            };
            assert!(
                matches!(accept(&ad), Err(SessionWireError::Params(_))),
                "params {ad:?} must not decode"
            );
        }
    }

    /// A blob is untrusted input: out-of-range selectors and density
    /// fractions are rejected on decode, not clamped into validity.
    #[test]
    fn out_of_range_wire_values_are_rejected() {
        let base = serde_json::to_value(SessionParams::from(&SessionConfig::default())).unwrap();
        for (pointer, value) in [
            ("/conditions/weather", json!(9)),
            ("/conditions/time_of_day", json!(4)),
            ("/densities/traffic", json!(1.5)),
            ("/densities/pedestrians", json!(-0.1)),
        ] {
            let mut params = base.clone();
            *params.pointer_mut(pointer).unwrap() = value;
            let ad = SessionAdvertisement {
                summary: "x".to_string(),
                params: serde_json::to_vec(&params).unwrap(),
            };
            assert!(
                accept(&ad).is_err(),
                "params with {pointer}={:?} must be rejected",
                params.pointer(pointer)
            );
        }
    }

    /// A `laps: 0` customization on the wire surfaces as the shared
    /// `ZeroLaps` config error, not a bespoke net-layer complaint.
    #[test]
    fn a_zero_laps_pick_fails_validation() {
        let mut params =
            serde_json::to_value(SessionParams::from(&SessionConfig::default())).unwrap();
        params["customization"] = json!({
            "conditions": {"time_of_day": 0, "weather": 0},
            "densities": {"traffic": 0.5, "pedestrians": 0.5},
            "race": {"laps": 0, "opponents": 3},
        });
        let ad = SessionAdvertisement {
            summary: "x".to_string(),
            params: serde_json::to_vec(&params).unwrap(),
        };
        assert!(matches!(
            accept(&ad),
            Err(SessionWireError::Invalid(ConfigError::ZeroLaps))
        ));
    }

    fn catalog_entry(id: &str, paints: &[&str], ready: bool) -> mm2_content::CatalogEntry {
        mm2_content::CatalogEntry {
            id: id.to_string(),
            display_name: id.to_string(),
            paints: paints.iter().map(|p| p.to_string()).collect(),
            canonical_info: true,
            unlock_score: 0,
            unlock_flags: 0,
            stats: mm2_content::DisplayStats::default(),
            class: mm2_content::VehicleClass::Stock,
            deps: mm2_content::DepSet::default(),
            status: if ready {
                EntryStatus::Ready
            } else {
                EntryStatus::Incomplete {
                    missing: vec!["model (geometry/<id>.pkg)".to_string()],
                }
            },
            notes: Vec::new(),
        }
    }

    fn test_catalog() -> VehicleCatalog {
        VehicleCatalog {
            entries: vec![
                catalog_entry("vpbug", &["red", "blue", "green", "yellow"], true),
                catalog_entry("vpcab", &["taxi"], false),
                // A ready entry with no Colors metadata gets the
                // one-paint-job floor like `paint_jobs.max(1)`.
                catalog_entry("vpbare", &[], true),
            ],
        }
    }

    /// The dev car (empty wire id ↔ `VehicleSelection::id = None`) and
    /// a catalog pick both survive the pick codec; a paint index that
    /// does not fit the wire's `u8` is refused, not clamped.
    #[test]
    fn picks_roundtrip_between_selection_and_wire() {
        let sel = VehicleSelection {
            id: Some("vpbug".to_string()),
            paint: 2,
        };
        let pick = encode_pick(&sel).unwrap();
        assert_eq!(pick.vehicle, "vpbug");
        assert_eq!(pick.paint, 2);
        assert_eq!(decode_pick(&pick), sel);

        let dev = VehicleSelection::default();
        let pick = encode_pick(&dev).unwrap();
        assert_eq!(pick.vehicle, "");
        assert_eq!(decode_pick(&pick), dev);

        let wild = VehicleSelection {
            id: Some("vpbug".to_string()),
            paint: 300,
        };
        assert!(matches!(
            encode_pick(&wild),
            Err(SessionWireError::Paint(300))
        ));
    }

    /// A `Start` seats us in the host's echo of our pick; a late
    /// joiner's `Start` that beat the echo keeps the car it offered
    /// rather than the dev car.
    #[test]
    fn a_start_seats_the_echoed_pick_else_the_offered_one() {
        let offered = VehicleSelection {
            id: Some("vpbug".into()),
            paint: 2,
        };
        let entry = |id: u16, vehicle: Option<&str>, paint: u8| mm2_net::RosterEntry {
            player_id: id,
            driver: format!("p{id}"),
            build: String::new(),
            ready: true,
            pick: vehicle.map(|v| VehiclePick {
                vehicle: v.into(),
                paint,
            }),
        };
        // The echo wins, whatever was offered.
        let echoed = [entry(1, Some("vpmustang"), 1)];
        assert_eq!(
            start_pick(&echoed, 1, offered.clone()).id.as_deref(),
            Some("vpmustang")
        );
        // No pick on our entry yet, or no entry at all: the offer.
        for roster in [
            vec![entry(1, None, 0)],
            vec![entry(2, Some("vpmustang"), 1)],
        ] {
            assert_eq!(start_pick(&roster, 1, offered.clone()), offered);
        }
        // An offered dev car stays the dev car.
        assert_eq!(
            start_pick(&[], 1, VehicleSelection::default()),
            VehicleSelection::default()
        );
    }

    /// The validator applies the designed policy: the dev car is always
    /// legal (paint 0 only), catalog ids must match exactly, incomplete
    /// entries refuse with their missing deps, and paint is bounded by
    /// the entry's `Colors` list.
    #[test]
    fn the_vehicle_validator_gates_picks_by_catalog() {
        let validate = vehicle_validator(&test_catalog());

        validate("", 0).unwrap();
        assert_eq!(
            validate("", 1).unwrap_err(),
            "the dev car has a single paint job"
        );

        validate("vpbug", 0).unwrap();
        validate("vpbug", 3).unwrap();
        assert_eq!(
            validate("vpbug", 4).unwrap_err(),
            "paint 4 out of range: vpbug has 4 paint job(s)"
        );

        // No Colors metadata: paint 0 alone is legal.
        validate("vpbare", 0).unwrap();
        assert!(validate("vpbare", 1).unwrap_err().contains("out of range"));

        // Unknown ids and non-canonical spellings refuse alike — the
        // wire carries catalog ids, not menu aliases.
        assert_eq!(
            validate("nosuch", 0).unwrap_err(),
            "unknown vehicle id \"nosuch\""
        );
        assert_eq!(
            validate("VPBUG", 0).unwrap_err(),
            "unknown vehicle id \"VPBUG\""
        );

        // A cataloged-but-incomplete entry refuses with the reason.
        let err = validate("vpcab", 0).unwrap_err();
        assert!(err.contains("incomplete"), "got {err}");
        assert!(err.contains("geometry"), "got {err}");
    }

    /// An empty catalog still admits the dev car — a content-free host
    /// (`mm2-host --dev-world` on an empty install) lobbies fine.
    #[test]
    fn the_validator_on_empty_content_admits_only_the_dev_car() {
        let validate = vehicle_validator(&VehicleCatalog::default());
        validate("", 0).unwrap();
        assert!(validate("vpbug", 0).is_err());
    }
}
