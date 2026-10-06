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
//! - `mods_active` — whether *this* process mounted mods is a local
//!   fact the session builder stamps on its own;
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
use mm2_content::{EntryStatus, VehicleCatalog};
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
        })
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
            LobbyEvent::Message(Message::Cancel { generation }) => {
                cancel(&mut lobby, &mut session, &mut control, generation)
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
            && let Err(e) = session.begin_generation(config, generation)
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
    // The pick is roster state, not session params — ours is what the
    // host last confirmed on our entry (a late joiner without a
    // committed pick drives the dev car).
    config.vehicle = lobby
        .roster
        .iter()
        .find(|e| e.player_id == link.player_id())
        .and_then(|e| e.pick.as_ref())
        .map(decode_pick)
        .unwrap_or_default();
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
        if let Err(e) = session.begin_generation(config, generation) {
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
    if *session.phase() != SessionPhase::Menu || link.leaving() || link.closed {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCommand {
    /// Request the session start — the lobby's own gate decides, the
    /// verdict arrives as `Started`/`StartRefused`.
    Start,
    /// End the running session — everyone returns to the lobby.
    Cancel,
    /// Shut the lobby down and exit the app.
    Quit,
}

/// Feed stdin `start`/`cancel`/`quit` lines into a hosted lobby — the
/// same operator surface `mm2-host` documents. A closed stdin just
/// ends the thread; unknown lines are named on stderr, never queued.
pub fn stdin_commands(tx: Sender<HostCommand>) {
    let _ = thread::Builder::new()
        .name("mm2-host-stdin".to_string())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { return };
                let command = match line.trim() {
                    "start" => HostCommand::Start,
                    "cancel" => HostCommand::Cancel,
                    "quit" => HostCommand::Quit,
                    "" => continue,
                    other => {
                        eprintln!("error: unknown command {other:?}");
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
    config: SessionConfig,
    /// What was advertised — `lobby.advertised`'s seed and the
    /// `listening=` record's summary.
    ad: SessionAdvertisement,
    /// The start's late-join policy — the session mode's (MP-5:
    /// event lobbies close once started, cruise stays open).
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
        let ad = advertise(&config)?;
        let late_join = match config.mode {
            SessionMode::Event(_) => LateJoin::Closed,
            SessionMode::Cruise => LateJoin::Open,
        };
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
            && let Err(e) = session.begin_generation(config, generation)
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
    if *session.phase() == SessionPhase::Menu {
        if let Err(e) = session.begin_generation(link.config.clone(), generation) {
            lobby.notice = Some(format!("hosted session could not begin: {e}"));
            link.cancel_sent = true;
            let _ = link.ctl.cancel();
        }
    } else {
        lobby.pending_start = Some((generation, link.config.clone()));
        control.quit = true;
    }
}

/// The windowed host lobby's keyboard surface — `Enter` requests the
/// start (the lobby's own gate answers `Started`/`StartRefused`),
/// `Esc` takes the lobby down. Both ride the command channel the
/// stdin driver feeds, so the keys and the operator words share one
/// intake. Only live while the session parks at `Menu`.
pub fn host_input(keys: Res<ButtonInput<KeyCode>>, session: Res<Session>, link: Res<HostLink>) {
    if *session.phase() != SessionPhase::Menu || link.leaving() {
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
            .and_then(|s| s.strip_suffix(".psdl"))
            .unwrap_or(psdl)
            .to_string(),
    };
    let mode = match &config.mode {
        SessionMode::Cruise => "cruise".to_string(),
        SessionMode::Event(r) => format!("{}:{}", r.table.stem_prefix(), r.index),
    };
    format!("{}, {}, {}", world, mode, config.difficulty.as_str())
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
