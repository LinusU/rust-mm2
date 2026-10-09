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
    Client, ClientCtl, Hello, Host, HostConfig, HostCtl, HostEvent, LateJoin, LeaveCause,
    MAX_PLAYERS, Message, NetError, PickValidator, RosterEntry, SessionAdvertisement, VehiclePick,
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
    /// The pump reported `Closed` — the link is dead.
    pub closed: bool,
    /// Re-ready after every `Cancel` — the host resets readiness when it
    /// closes a session, so a client that asked to be ready (`--ready`)
    /// would otherwise sit out the rematch with no one to press `Enter`.
    keep_ready: bool,
}

impl LobbyLink {
    /// Join the lobby at `addr`, then spawn the pump thread. The
    /// handshake runs inside `Client::join` under its own bound
    /// (`HANDSHAKE_TIMEOUT`), so a silent host cannot wedge the
    /// caller past it.
    pub fn join(
        addr: SocketAddr,
        hello: &Hello,
        mods_active: bool,
        dev: DevOverrides,
    ) -> Result<Self, NetError> {
        let client = Client::join(addr, hello)?;
        let player_id = client.player_id();
        let ctl = client.ctl()?;
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("mm2-lobby".to_string())
            .spawn(move || pump(client, tx))?;
        Ok(Self {
            events: Mutex::new(rx),
            ctl,
            player_id,
            peer: addr,
            driver: hello.driver.clone(),
            mods_active,
            dev,
            leaving: None,
            closed: false,
            keep_ready: false,
        })
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
    /// the pump then reports `Closed`. Idempotent.
    pub fn leave(&mut self) {
        if self.leaving.is_none() {
            self.leaving = Some(Instant::now());
            let _ = self.ctl.leave();
        }
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
        self.leave();
    }
}

/// The pump's whole job: block in `Client::recv`, forward each inbound
/// message as a [`LobbyEvent`], and end with `Closed` — on a dead
/// socket *or* when the app dropped the link (a failed `send` is the
/// receiver going away). Either way the client says `Leave` on its
/// way out so the host records `Quit`, not `Lost`.
fn pump(mut client: Client, tx: Sender<LobbyEvent>) {
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
    mut exit: MessageWriter<AppExit>,
) {
    for event in link.drain() {
        match event {
            LobbyEvent::Message(Message::Session(ad)) => match gate(&vfs.0, &ad) {
                Ok(_) => lobby.advertised = Some(ad),
                Err(why) => refuse(&mut link, &mut lobby, why),
            },
            LobbyEvent::Message(Message::Roster { players }) => lobby.roster = players,
            LobbyEvent::Message(Message::VehicleRefused { reason }) => {
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
            }) => snaps.push(generation, tick, entries, trailers, impacts, race),
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
            LobbyEvent::Message(Message::Cnr { generation, frame }) => {
                snaps.push_cnr(generation, &frame)
            }
            LobbyEvent::Message(Message::Cancel { generation }) => {
                cancel(&mut lobby, &mut session, &mut control, generation);
                // The host cleared every ready flag before it sent this,
                // so our answer lands after the reset, never before it.
                if link.keep_ready && !link.leaving() {
                    let _ = link.ctl.set_ready(true);
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
    control.pause = false;
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
/// start we never began) only clears that pending begin.
fn cancel(
    lobby: &mut LobbyState,
    session: &mut Session,
    control: &mut SessionControl,
    generation: u64,
) {
    if lobby
        .pending_start
        .as_ref()
        .is_some_and(|(g, _)| *g == generation)
    {
        lobby.pending_start = None;
    }
    if lobby.generation == Some(generation) {
        lobby.generation = None;
        lobby.host_pick = None;
        if !matches!(
            session.phase(),
            SessionPhase::Menu | SessionPhase::Unloading
        ) {
            control.quit = true;
        }
    }
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
/// ready flag and `Esc` leaves the lobby (the session-lifecycle Esc
/// lives in `session_control_input`, which ignores `Menu`, so the two
/// never collide). Gamepad equivalents are a menu-surface decision
/// deferred with the real lobby UI.
pub fn lobby_input(
    keys: Res<ButtonInput<KeyCode>>,
    session: Res<Session>,
    lobby: Res<LobbyState>,
    mut link: ResMut<LobbyLink>,
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
        let _ = link.ctl().set_ready(ready);
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
    let offered = lobby
        .advertised
        .as_ref()
        .map(|ad| ad.summary.clone())
        .unwrap_or_else(|| "waiting for the host to advertise a session".to_string());
    let ours = lobby
        .roster
        .iter()
        .find(|e| e.player_id == link.player_id());
    let picked = ours
        .and_then(|e| e.pick.as_ref())
        .map(|p| {
            if p.vehicle.is_empty() {
                "dev car".to_string()
            } else {
                format!("{} (paint {})", p.vehicle, p.paint)
            }
        })
        .unwrap_or_else(|| "nothing yet".to_string());
    let ready = ours.is_some_and(|e| e.ready);
    let ready_count = lobby.roster.iter().filter(|e| e.ready).count();
    let notice = lobby
        .notice
        .as_ref()
        .map(|n| format!("\n{n}"))
        .unwrap_or_default();
    text.0 = format!(
        "Lobby {} — {}\nsession: {offered}\nplayers: {} ({} ready)\nyou: {} — {picked}{}\n\nenter: toggle ready    esc: leave lobby",
        link.peer(),
        link.driver(),
        lobby.roster.len(),
        ready_count,
        if ready { "ready" } else { "not ready" },
        notice,
    );
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
                "nothing to change: city= event= cnr= gold= limit= difficulty= weather= tod= seed="
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
/// peer, so nothing a client predicts depends on it. A physics pin
/// (`--traction`, `--spawn`, ...) is the opposite — peers could never
/// agree on it — and stays refused.
fn host_local_dev(dev: &mut DevOverrides) -> DevOverrides {
    DevOverrides {
        wreck_at: dev.wreck_at.take(),
        wreck_seat: dev.wreck_seat.take(),
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
/// `menu_watch` reopens the shell. Only acts under a [`MenuShell`] — a
/// `--host` app has no menu to return to and exits through
/// [`drive_host`] instead.
#[allow(clippy::too_many_arguments)] // the resources a hosted lobby owns, by name
pub fn close_menu_host(
    mut commands: Commands,
    link: Res<HostLink>,
    mut lobby: ResMut<LobbyState>,
    session: Res<Session>,
    menu: Option<Res<MenuShell>>,
    texts: Query<Entity, With<HostText>>,
) {
    if menu.is_none()
        || !link.leaving()
        || *session.phase() != SessionPhase::Menu
        || lobby.pending_start.is_some()
    {
        return;
    }
    if let Some(notice) = lobby.notice.take() {
        warn!(notice, "menu-hosted lobby closed");
    }
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
}

/// Join the lobby at `addr` for the menu's *Join lobby* row: the same
/// fingerprint-bound handshake `--join` runs, then the menu's vehicle
/// pick offered to the roster. A refused or unreachable lobby is a named
/// error for the menu's status line, never a silent local session. The
/// handshake is bounded by `HANDSHAKE_TIMEOUT`.
pub fn open_menu_join(
    vfs: &Vfs,
    addr: SocketAddr,
    driver: String,
    mods_active: bool,
    vehicle: &VehicleSelection,
) -> Result<LobbyLink, MenuJoinError> {
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
    let hello = mm2_net::hello(crate::smoke::COMMIT.to_string(), driver, fingerprint.hash);
    let link = LobbyLink::join(addr, &hello, mods_active, DevOverrides::default())?;
    // Offer the pick at once, as `--join` does: the roster shows what we
    // will drive and the host's catalog gate confirms it can spawn.
    let _ = link.ctl().set_vehicle(&pick.vehicle, pick.paint);
    Ok(link)
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
    let offered = link.summary();
    let pick = match &link.config.vehicle.id {
        Some(id) => format!("{id} (paint {})", link.config.vehicle.paint),
        None => "dev car".to_string(),
    };
    let ready_count = lobby.roster.iter().filter(|e| e.ready).count();
    let notice = lobby
        .notice
        .as_ref()
        .map(|n| format!("\n{n}"))
        .unwrap_or_default();
    text.0 = format!(
        "Hosting {} — {}\nsession: {offered}\nplayers: {} connected ({} ready)\nyou: {} — {pick} (host)\n\nenter: start session    esc: stop hosting{}",
        link.addr(),
        link.driver(),
        lobby.roster.len(),
        ready_count,
        link.driver(),
        notice,
    );
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
    format!("{}, {}, {}", world, mode, config.difficulty.as_str())
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
            pause: true,
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
            },
            customization: Some(SessionCustomization {
                conditions: SessionConditions {
                    time_of_day: TimeOfDay::new(1).unwrap(),
                    weather: Weather::new(0).unwrap(),
                },
                densities: Densities {
                    traffic: 0.0,
                    pedestrians: 1.0,
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
