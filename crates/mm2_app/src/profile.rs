//! Profile wiring for the app (F16-A remainder): bind a driver profile
//! at startup, restore its remembered selections, and record the
//! session's choices back.
//!
//! The store itself is `mm2_game::profile`; this module owns the flow:
//!
//! - [`ProfileRequest`] is what the CLI asks for — an explicit
//!   `--profile <id|name>` selection, a `--new-profile <name>`
//!   creation, or nothing (the store's `active` marker).
//! - [`resolve`] turns a request plus an open [`ProfileStore`] into a
//!   bound [`ActiveProfile`] resource. Selecting a profile marks it
//!   `active` so later runs bind it implicitly; a profile recovered
//!   from a `.tmp`/`.bak` is re-saved once at bind so the main file
//!   heals (F16-AC04).
//! - [`choose_launch`] applies the remembered selections: the profile's
//!   vehicle/paint is used when `--car`/`--paint` are absent, and its
//!   rank (DRV-2) supplies the session difficulty unless `--pro`
//!   overrides. A remembered vehicle that no longer resolves (changed
//!   install, missing mod) degrades to the stock default with a
//!   warning — saved preferences never hard-fail a launch.
//! - [`note_session_start`] records the session's actual selections —
//!   the driven vehicle and the event's stable [`EventKey`] — and saves
//!   immediately, so a crash mid-session cannot lose them. A dev-car
//!   session leaves a remembered vehicle untouched.
//!
//! Create/select/delete *flows* with UI confirmation stay F17 scope;
//! these flags are the store's interim flow primitives. Progress
//! records and unlocks are written by F16-B's result pipeline —
//! nothing here writes `progress`.

use std::path::PathBuf;

use bevy::prelude::Resource;
use mm2_game::{
    Difficulty, EventKey, PlayerProfile, ProfileError, ProfileId, ProfileKind, ProfileStore,
    VehicleChoice,
};
use tracing::{info, warn};

use crate::session::SelectedCar;

/// The profile bound to this run. Inserted as a resource only when a
/// profile actually bound — profile-less runs (no store, `--no-profile`,
/// unrequested smoke runs) never carry one.
#[derive(Resource)]
pub struct ActiveProfile {
    /// The store the profile lives in — the persistence target.
    pub store: ProfileStore,
    /// The working copy; `selections` is updated as sessions start.
    pub profile: PlayerProfile,
    /// `true` when `load` recovered this profile from a `.tmp`/`.bak`
    /// rather than the main file — kept so the recovery stays
    /// observable after the heal.
    pub recovered_from_backup: bool,
}

/// Which profile a run should bind.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileRequest {
    /// `--profile <sel>`: an existing `driver-<n>` id or a unique
    /// display name. Matching nothing is an error — silently creating
    /// one would attach a typo to a fresh identity.
    Select(String),
    /// `--new-profile <name>`: create a profile and bind it. Duplicate
    /// display names are legal — the id is the identity.
    Create {
        /// Display name for the new profile.
        name: String,
        /// Driver rank (DRV-2): the `--pro` flag at creation.
        rank: Difficulty,
        /// Standard or sandbox identity (spec req 5).
        kind: ProfileKind,
    },
    /// No explicit selector: the store's `active` marker wins if it
    /// still resolves; otherwise the run goes profile-less.
    Active,
}

/// Why a profile could not be bound on an explicit request.
#[derive(Debug)]
pub enum ProfileBindError {
    /// `--profile <sel>` matched no profile.
    Unknown(String),
    /// `--profile <sel>` matched several display names — pick by id.
    Ambiguous(String, Vec<ProfileId>),
    /// Store I/O or content failure (corrupt profile, unwritable dir).
    Store(ProfileError),
}

impl std::fmt::Display for ProfileBindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(sel) => write!(
                f,
                "no profile matches {sel:?} (ids look like driver-0; \
                 --new-profile creates one)"
            ),
            Self::Ambiguous(sel, ids) => write!(
                f,
                "profile name {sel:?} is ambiguous: {}; select by id",
                ids.iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProfileBindError {}

/// Resolve a [`ProfileRequest`] against an open store.
///
/// `Ok(Some)` is a bound profile, already recorded `active`. `Ok(None)`
/// is only reachable on the implicit `Active` path — an empty store or a
/// marker that no longer resolves degrades to a profile-less run rather
/// than blocking launch; the same path re-heals nothing because nothing
/// was loaded. Explicit `Select`/`Create` requests surface every failure.
pub fn resolve(
    store: &ProfileStore,
    request: &ProfileRequest,
) -> Result<Option<ActiveProfile>, ProfileBindError> {
    let loaded = match request {
        ProfileRequest::Create { name, rank, kind } => {
            let profile = store
                .create(name.clone(), *rank, *kind)
                .map_err(ProfileBindError::Store)?;
            info!(profile = %profile.id, name = %profile.name, "created driver profile");
            mm2_game::ProfileLoad {
                profile,
                recovered_from_backup: false,
            }
        }
        ProfileRequest::Select(sel) => {
            let id = lookup(store, sel)?;
            store.load(&id).map_err(ProfileBindError::Store)?
        }
        ProfileRequest::Active => {
            let Some(id) = store.active().map_err(ProfileBindError::Store)? else {
                return Ok(None);
            };
            match store.load(&id) {
                Ok(loaded) => loaded,
                // The implicit path must not gate the game on a damaged
                // file — run profile-less and leave the files untouched.
                Err(e) => {
                    warn!(profile = %id, error = %e, "active profile unreadable; running without one");
                    return Ok(None);
                }
            }
        }
    };
    let id = loaded.profile.id.clone();
    if let Err(e) = store.set_active(&id) {
        warn!(profile = %id, error = %e, "could not record the active profile");
    }
    // A `.tmp`/`.bak` recovery means the main file is stale or missing;
    // one save heals it now that the data is in hand.
    if loaded.recovered_from_backup {
        let mut profile = loaded.profile;
        match store.save(&mut profile) {
            Ok(()) => info!(profile = %id, "repaired profile main file from the surviving copy"),
            Err(e) => warn!(profile = %id, error = %e, "profile recovery save failed"),
        }
        return Ok(Some(ActiveProfile {
            store: store.clone(),
            profile,
            recovered_from_backup: true,
        }));
    }
    Ok(Some(ActiveProfile {
        store: store.clone(),
        profile: loaded.profile,
        recovered_from_backup: false,
    }))
}

/// Resolve a `--profile` selector to an id: exact `driver-<n>` id first,
/// then a display-name match that must be unique.
fn lookup(store: &ProfileStore, sel: &str) -> Result<ProfileId, ProfileBindError> {
    let list = store.list().map_err(ProfileBindError::Store)?;
    if let Some(entry) = list.iter().find(|e| e.id.as_str() == sel) {
        return Ok(entry.id.clone());
    }
    let named: Vec<ProfileId> = list
        .iter()
        .filter(|e| e.meta.as_ref().is_some_and(|m| m.name == sel))
        .map(|e| e.id.clone())
        .collect();
    match named.len() {
        1 => Ok(named.into_iter().next().unwrap()),
        0 => Err(ProfileBindError::Unknown(sel.to_string())),
        _ => Err(ProfileBindError::Ambiguous(sel.to_string(), named)),
    }
}

/// Where the vehicle choice comes from — controls how a load failure is
/// treated (explicit `--car` is a usage error; a remembered car is a
/// soft fallback).
#[derive(Debug, Clone, PartialEq)]
pub enum VehicleSource<'a> {
    /// `--car <query>` — explicit, hard failure on load error.
    Explicit(&'a str),
    /// The bound profile's remembered vehicle — soft failure: retry at
    /// paint 0, then the stock default.
    Remembered(&'a VehicleChoice),
    /// No choice anywhere — the stock default / dev car.
    Default,
}

/// Merge CLI flags with the bound profile's remembered selections.
///
/// Returns `(vehicle source, paint, difficulty)`: `--car` beats the
/// remembered vehicle; `--paint` beats the remembered paint (which only
/// applies while the remembered vehicle is what loads); `--pro` beats
/// the profile's rank (DRV-2). `last_event` is *not* launched — its
/// consumer is F17's Quick Race/menu flow (DRV-8).
pub fn choose_launch<'a>(
    cli_car: Option<&'a str>,
    cli_paint: Option<usize>,
    pro: bool,
    profile: Option<&'a PlayerProfile>,
) -> (VehicleSource<'a>, usize, Difficulty) {
    let (source, saved_paint) = match (cli_car, profile) {
        (Some(query), _) => (VehicleSource::Explicit(query), None),
        (None, Some(p)) => match &p.selections.vehicle {
            Some(choice) => (
                VehicleSource::Remembered(choice),
                Some(choice.paint as usize),
            ),
            None => (VehicleSource::Default, None),
        },
        (None, None) => (VehicleSource::Default, None),
    };
    let paint = cli_paint.or(saved_paint).unwrap_or(0);
    let difficulty = match (pro, profile) {
        (true, _) => Difficulty::Professional,
        (false, Some(p)) => p.rank,
        (false, None) => Difficulty::Amateur,
    };
    (source, paint, difficulty)
}

/// Why the launch selection breaches the garage's gates (F16-B.3).
/// `--car`/a remembered vehicle bypass the menu that will enforce
/// these gates in F17 — the note surfaces the breach honestly, the
/// same warn-don't-enforce interim the locked `--event` launch uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VehicleGateNote {
    /// The vehicle id is not in the catalog at all.
    Uncatalogued,
    /// The entry is not on the select roster (fallback-extension or
    /// metadata-less — vpmoonrover's `.inf` is the retail case, UNK-3).
    Unlisted,
    /// The vehicle is reward-gated and the profile lacks its unlock.
    Locked,
    /// The vehicle is open but this paint index is reward-gated and
    /// the profile lacks `paint:<id>:<index>`.
    LockedPaint,
}

/// Evaluate the launch selection against the bound profile's garage
/// gates. `None` means selectable (also the sandbox view — an
/// unrestricted identity never produces a note). Builds the garage
/// table on demand; callers run it once per launch, not per frame.
pub fn vehicle_gate_note(
    vfs: &mm2_assets::Vfs,
    profile: &PlayerProfile,
    id: &str,
    paint: usize,
) -> Option<VehicleGateNote> {
    let garage = mm2_content::scan_garage(vfs);
    let Some(row) = garage.row(id) else {
        return Some(VehicleGateNote::Uncatalogued);
    };
    if !row.listed {
        return Some(VehicleGateNote::Unlisted);
    }
    let avail = garage
        .of(profile, id)
        .expect("a catalog row always evaluates");
    if !avail.unlocked {
        return Some(VehicleGateNote::Locked);
    }
    if avail.paints.get(paint) == Some(&false) {
        return Some(VehicleGateNote::LockedPaint);
    }
    None
}

/// Record the session's selections on the bound profile and persist
/// immediately — at session start, not exit, so a crash mid-session
/// cannot lose them.
///
/// `vehicle` is only written when a catalog vehicle actually drives
/// (a dev-car run does not erase the remembered car); `last_event` is
/// only written by an event session (a cruise does not erase the last
/// played event). Save failures are logged, never fatal — profile I/O
/// must never sink a session.
pub fn note_session_start(
    slot: &mut ActiveProfile,
    selected: &SelectedCar,
    event_key: Option<&EventKey>,
) {
    if let Some(def) = &selected.def {
        slot.profile.selections.vehicle = Some(VehicleChoice {
            id: def.id.clone(),
            paint: selected.paint.min(u32::MAX as usize) as u32,
        });
    }
    if let Some(key) = event_key {
        slot.profile.selections.last_event = Some(key.clone());
    }
    if let Err(e) = slot.store.save(&mut slot.profile) {
        warn!(profile = %slot.profile.id, error = %e, "profile save failed");
    }
}

/// Resolve the store root: an explicit `--profile-dir` wins, else the
/// OS user-data dir.
pub fn store_root(dir: Option<PathBuf>) -> Option<PathBuf> {
    dir.or_else(ProfileStore::default_root)
}
