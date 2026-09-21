//! Persistent player profiles (F16-A): versioned storage, atomic
//! save/load and isolated identities.
//!
//! A driver profile is the identity progression, records and remembered
//! selections hang off (DRV-1 — several drivers share one install). The
//! store keeps one JSON file per profile under a caller-chosen root —
//! the OS data directory via [`ProfileStore::default_root`], never
//! inside the read-only original installation (DSN-15).
//!
//! Durability contract (F16-AC04):
//!
//! - `save` writes `<id>.json.tmp` and flushes it, rotates the existing
//!   `<id>.json` to `<id>.json.bak`, then renames the tmp over the
//!   target and fsyncs the directory — at every instant a complete
//!   file exists under one of the three names.
//! - `load` reads every surviving copy and keeps the highest
//!   `revision`: a `.tmp` left behind by an interrupted save is always
//!   a newer attempt than the main it never got renamed over, so the
//!   freshest complete document wins. Anything but a clean main-file
//!   read is reported through [`ProfileLoad::recovered_from_backup`].
//!   A corrupt file is never deleted or overwritten by a load; the
//!   next successful `save` heals the main file while the backup
//!   stays.
//! - `version` stamps every write; a file whose schema is not
//!   [`PROFILE_SCHEMA_VERSION`] is rejected rather than guessed at —
//!   migrations land explicitly when a v2 exists.
//! - Unknown fields are preserved verbatim in [`PlayerProfile::extra`]
//!   so a file written by a newer build survives a round trip through
//!   this one (spec req 4).
//!
//! Identity contract:
//!
//! - [`ProfileId`]s are `driver-<n>` allocated from a persisted
//!   high-water mark (`next-id`, advanced before the new profile's
//!   first save) floored at the highest surviving file suffix. A
//!   `.bak`/`.tmp` left behind by an interrupted save or delete still
//!   owns its id, and a deleted id is never reused — so a stale
//!   reference can never resolve to a different person. If the mark
//!   file itself is lost, allocation degrades to the file-scan floor:
//!   a deleted highest id could then be reissued, but no live profile
//!   is ever displaced.
//! - Progress is keyed by [`EventKey`] — the event's authored file stem
//!   (`race3`), not its table row index — so a mod inserting a table row
//!   cannot silently retarget a saved record (spec req 4). The consumer
//!   resolves stem → row through the event catalog.
//! - [`ProfileKind::Sandbox`] is the dev/unrestricted identity the spec
//!   (req 5) separates from legitimate progression;
//!   [`PlayerProfile::records_progress`] is the single gate F16-B
//!   consumers check before applying rewards or records.
//!
//! This is our own save format — no compatibility with the original
//! `players/*.sav`/`*.cfg` binaries is claimed (F16 non-goal).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Difficulty, EventTableKind};

/// Schema version written by this build. Files carrying any other
/// version are rejected by [`ProfileStore::load`].
pub const PROFILE_SCHEMA_VERSION: u32 = 1;

/// Display-name bound: non-empty, no control characters, at most this
/// many chars. A bound exists so a corrupt/hostile file cannot inject
/// an unbounded string into UI surfaces.
pub const MAX_NAME_CHARS: usize = 32;

/// Largest profile file `load` will read — a bound so a corrupt or
/// hostile file cannot force an unbounded allocation. Real profiles
/// are a few KiB.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// File extension of a live profile document inside the store root.
const PROFILE_EXT: &str = "json";
/// Name of the file recording the most recently selected profile.
const ACTIVE_FILE: &str = "active";
/// Name of the file recording the next `driver-<n>` suffix to hand
/// out — the high-water mark that keeps a deleted id retired even
/// when every file the profile owned is gone.
const NEXT_ID_FILE: &str = "next-id";
/// Read bound for the one-token marker files (`active`, `next-id`).
/// Profile documents get [`MAX_FILE_BYTES`]; a marker that runs past
/// this cap is garbage, not a longer selection.
const MAX_MARKER_BYTES: u64 = 4096;

/// Stable identity of one driver profile — `driver-<n>`; unique within
/// a store and never reused after deletion.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(String);

impl ProfileId {
    fn new(n: u64) -> Self {
        Self(format!("driver-{n}"))
    }

    /// The serialized form, also the store's file stem.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The numeric suffix, when the id has the `driver-<n>` shape.
    fn suffix(&self) -> Option<u64> {
        self.0.strip_prefix("driver-")?.parse().ok()
    }
}

impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ProfileId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ProfileId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Whether the profile takes part in real progression. The split is the
/// spec's dev/sandbox separation (req 5): results earned on a
/// `Sandbox` profile must never feed unlocks or records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    /// Normal driver — progression and records apply.
    Standard,
    /// Development/unrestricted identity — its results are ineligible
    /// for rewards and records.
    Sandbox,
}

/// Stable identity of one authored event inside a profile: the content
/// stem the table row maps to (e.g. `race3` for a Checkpoint row). The
/// stem — not the row index — is the saved identity because a mod
/// inserting a table row shifts indexes but never renames the row's
/// files (spec req 4). `table` namespaces the stem across the four
/// `mm*data.csv` tables.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventKey {
    /// City stem (`london`, `sf`, or a mod city).
    pub city: String,
    /// Which authored event table the row lives in.
    pub table: EventTableKind,
    /// Event stem the row maps to (`race3`, `blitz0`, `lesson1`'s
    /// linked stem).
    pub stem: String,
}

/// Recorded progress against one authored event. Semantics are the
/// F16-B/F16-C consumers' — this container only guarantees stable
/// keys and lossless persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    /// Which event this record belongs to.
    pub key: EventKey,
    /// Times the event's authoritative result stream produced a
    /// `Finished` outcome for this profile.
    pub finishes: u32,
    /// Best (lowest) recorded finish in race ticks.
    pub best_race_ticks: Option<u64>,
}

impl EventRecord {
    /// Note one authoritative finish at `race_ticks` — bumps the count
    /// and keeps the best (lowest) time. Idempotency is the caller's
    /// (the `ResultLedger` dedups by `ResultId`, spec req 3).
    pub fn record_finish(&mut self, race_ticks: u64) {
        self.finishes += 1;
        self.best_race_ticks = Some(
            self.best_race_ticks
                .map_or(race_ticks, |best| best.min(race_ticks)),
        );
    }
}

/// Everything a profile remembers about the world: per-event records
/// and granted unlock ids. Written only through authoritative results —
/// never by UI navigation (spec req 3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileProgress {
    /// Per-event records, kept sorted by key so the file layout is
    /// stable. A `Vec`, not a map keyed by `EventKey`, because JSON
    /// object keys must be strings.
    #[serde(default)]
    pub events: Vec<EventRecord>,
    /// Reward ids granted to this profile. Opaque here — F16-B defines
    /// the id space (`vehicle:<id>`, `paint:<id>/<n>`, …); the set
    /// makes re-grants idempotent.
    #[serde(default)]
    pub unlocks: BTreeSet<String>,
}

/// A remembered vehicle choice (`vpbug` + paint index).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VehicleChoice {
    /// Catalog vehicle id.
    pub id: String,
    /// Zero-based paint index.
    pub paint: u32,
}

/// Selections the profile remembers across restarts — what the app
/// restores before a session starts (spec req 6) and what DRV-8's
/// Quick Race reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileSelections {
    /// Last driven vehicle/paint.
    #[serde(default)]
    pub vehicle: Option<VehicleChoice>,
    /// Last played event (DRV-8: Quick Race jumps straight into it).
    #[serde(default)]
    pub last_event: Option<EventKey>,
}

/// One driver profile on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerProfile {
    /// Save schema version — always [`PROFILE_SCHEMA_VERSION`] on
    /// write, checked on read.
    pub version: u32,
    /// Stable identity — matches the file stem.
    pub id: ProfileId,
    /// Display name; unique ids mean names may repeat (edge case:
    /// duplicate display names are legal).
    pub name: String,
    /// Fixed driver rank (DRV-2): Amateur or Professional — the event
    /// parameter block this profile races under (DRV-3).
    pub rank: Difficulty,
    /// Standard vs sandbox identity (spec req 5).
    pub kind: ProfileKind,
    /// Number of successful saves — bumps per `save`. `load` keeps the
    /// highest surviving revision, so an interrupted save's orphaned
    /// `.tmp` beats the older main it never got renamed over.
    pub revision: u64,
    /// Records and unlocks.
    #[serde(default)]
    pub progress: ProfileProgress,
    /// Remembered vehicle/event selections.
    #[serde(default)]
    pub selections: ProfileSelections,
    /// Fields written by a newer build that this version does not
    /// model — carried verbatim so a round trip does not erase them.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl PlayerProfile {
    fn new(id: ProfileId, name: String, rank: Difficulty, kind: ProfileKind) -> Self {
        Self {
            version: PROFILE_SCHEMA_VERSION,
            id,
            name,
            rank,
            kind,
            revision: 0,
            progress: ProfileProgress::default(),
            selections: ProfileSelections::default(),
            extra: BTreeMap::new(),
        }
    }

    /// Whether this profile's results may feed progression and records
    /// — the single check F16-B consumers make (spec req 5/AC03).
    pub fn records_progress(&self) -> bool {
        matches!(self.kind, ProfileKind::Standard)
    }

    /// Content problems that make a parsed file untrustworthy — an
    /// empty/over-long/control-character name, a malformed id, or a
    /// schema this build does not speak.
    fn validate(&self) -> Result<(), String> {
        if self.version != PROFILE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema version {} (this build writes {PROFILE_SCHEMA_VERSION})",
                self.version
            ));
        }
        if self.id.suffix().is_none() {
            return Err(format!("malformed profile id {:?}", self.id.as_str()));
        }
        // `event_mut` binary-searches this Vec; a hand-edited file with
        // unsorted or duplicated keys would silently split one event's
        // record, so the invariant is enforced at the boundary.
        if self
            .progress
            .events
            .windows(2)
            .any(|pair| pair[0].key >= pair[1].key)
        {
            return Err("event records are unsorted or repeat a key".to_string());
        }
        validate_name(&self.name)
    }

    /// `key`'s record, when the event has one.
    pub fn event(&self, key: &EventKey) -> Option<&EventRecord> {
        self.progress.events.iter().find(|r| &r.key == key)
    }

    /// Mutable access to `key`'s record, inserting a zeroed one when
    /// the event has no record yet. Keeps `events` sorted so the
    /// serialized layout stays stable.
    pub fn event_mut(&mut self, key: EventKey) -> &mut EventRecord {
        let pos = match self.progress.events.binary_search_by(|r| r.key.cmp(&key)) {
            Ok(pos) => pos,
            Err(pos) => {
                self.progress.events.insert(
                    pos,
                    EventRecord {
                        key,
                        finishes: 0,
                        best_race_ticks: None,
                    },
                );
                pos
            }
        };
        &mut self.progress.events[pos]
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("name is empty".to_string());
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("name exceeds {MAX_NAME_CHARS} characters"));
    }
    if name.chars().any(char::is_control) {
        return Err("name contains control characters".to_string());
    }
    Ok(())
}

/// What [`ProfileStore::load`] returned.
#[derive(Debug)]
pub struct ProfileLoad {
    /// The recovered or cleanly-read profile.
    pub profile: PlayerProfile,
    /// `true` when a surviving `.tmp`/`.bak` — not the main file —
    /// supplied this profile: observable evidence of the recovery path
    /// (F16-AC04). The caller can `save` to heal the main file.
    pub recovered_from_backup: bool,
}

/// A store listing entry. Corrupt files still list by their id so a
/// broken profile is inspectable and deletable rather than silently
/// disappearing (the expected-denominator rule). An id whose main file
/// is gone but whose `.tmp`/`.bak` survives still lists, carrying that
/// copy's metadata plus an error noting the recovery.
#[derive(Debug)]
pub struct ProfileSummary {
    /// Identity taken from the file stem — reliable even when the body
    /// is corrupt.
    pub id: ProfileId,
    /// Stored fields when a file parses; `None` when none does.
    pub meta: Option<ProfileMeta>,
    /// Why the entry is degraded — the main file's parse failure, or a
    /// note that the metadata came from a surviving backup copy.
    pub error: Option<String>,
}

/// The listing-relevant fields of a parseable profile.
#[derive(Debug)]
pub struct ProfileMeta {
    /// Display name.
    pub name: String,
    /// Driver rank.
    pub rank: Difficulty,
    /// Standard vs sandbox.
    pub kind: ProfileKind,
}

/// Why a store operation failed.
#[derive(Debug)]
pub enum ProfileError {
    /// Underlying filesystem failure.
    Io {
        /// File or directory the operation touched.
        path: PathBuf,
        /// The OS error.
        source: std::io::Error,
    },
    /// `create`/`save` rejected the supplied data — an invalid display
    /// name or a profile that fails validation.
    Invalid(String),
    /// No profile file (main or backup) exists for the id.
    UnknownProfile(ProfileId),
    /// Every existing file for the id failed to parse or validate. The
    /// reasons per file are kept; nothing was deleted or overwritten.
    Corrupt {
        /// The profile whose files failed.
        id: ProfileId,
        /// Why the main file was rejected, when one existed.
        main: Option<String>,
        /// Why the interrupted-save file was rejected, when one
        /// existed.
        tmp: Option<String>,
        /// Why the backup file was rejected, when one existed.
        backup: Option<String>,
    },
    /// Deleting this profile would leave the store empty (DRV-7: the
    /// last remaining driver cannot be deleted).
    LastProfile(ProfileId),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Invalid(reason) => write!(f, "invalid profile: {reason}"),
            Self::UnknownProfile(id) => write!(f, "no profile {id}"),
            Self::Corrupt {
                id,
                main,
                tmp,
                backup,
            } => {
                write!(f, "profile {id} is unreadable")?;
                for (label, reason) in [("main", main), ("tmp", tmp), ("backup", backup)] {
                    if let Some(reason) = reason {
                        write!(f, " ({label}: {reason})")?;
                    }
                }
                Ok(())
            }
            Self::LastProfile(id) => {
                write!(f, "cannot delete {id}: it is the last remaining profile")
            }
        }
    }
}

impl std::error::Error for ProfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A directory of versioned profile files. The root is the caller's
/// choice — [`default_root`](Self::default_root) supplies the
/// OS-appropriate user data location; tests supply a temp dir.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    /// Open (creating if needed) the store at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ProfileError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|source| ProfileError::Io {
            path: root.clone(),
            source,
        })?;
        Ok(Self { root })
    }

    /// The OS-appropriate user data directory for profiles (spec req 1):
    /// `~/Library/Application Support/rust-mm2/profiles` on macOS,
    /// `%APPDATA%\rust-mm2\profiles` on Windows,
    /// `$XDG_DATA_HOME/rust-mm2/profiles` or `~/.local/share/…` on other
    /// Unix. `None` when no base directory can be determined. Never a
    /// path inside the original installation — installs stay read-only.
    pub fn default_root() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        let base =
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
        #[cfg(target_os = "windows")]
        let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
            std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("AppData/Roaming"))
        });
        #[cfg(all(unix, not(target_os = "macos")))]
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
        #[cfg(not(any(unix, target_os = "windows")))]
        let base: Option<PathBuf> = None;
        base.map(|b| b.join("rust-mm2").join("profiles"))
    }

    /// The directory the store writes into.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn main_path(&self, id: &ProfileId) -> PathBuf {
        self.root.join(format!("{}.{PROFILE_EXT}", id.as_str()))
    }

    fn backup_path(&self, id: &ProfileId) -> PathBuf {
        self.root.join(format!("{}.{PROFILE_EXT}.bak", id.as_str()))
    }

    fn tmp_path(&self, id: &ProfileId) -> PathBuf {
        self.root.join(format!("{}.{PROFILE_EXT}.tmp", id.as_str()))
    }

    /// Whether `id` is usable as a file stem inside the store root.
    /// `ProfileId`'s public constructors accept arbitrary strings, so
    /// callers can hand in `../x`-shaped values; every operation that
    /// turns an id into a path checks this first rather than probing
    /// outside the root.
    fn is_safe_stem(id: &ProfileId) -> bool {
        let s = id.as_str();
        !s.is_empty() && !s.contains('/') && !s.contains('\\') && s != "." && s != ".."
    }

    /// Whether any file — main, tmp or backup — exists for `id`.
    fn id_has_file(&self, id: &ProfileId) -> bool {
        [self.main_path(id), self.tmp_path(id), self.backup_path(id)]
            .iter()
            .any(|p| p.exists())
    }

    /// Every profile id present in the store, highest suffix last —
    /// filename-derived, so corrupt files still count. All three file
    /// names occupy the id: a `.bak`/`.tmp` orphaned by an interrupted
    /// save or delete is still that profile's data, and reallocating
    /// the id would attach it to a different identity.
    fn existing_ids(&self) -> Result<Vec<ProfileId>, ProfileError> {
        let mut ids = Vec::new();
        let entries = std::fs::read_dir(&self.root).map_err(|source| ProfileError::Io {
            path: self.root.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| ProfileError::Io {
                path: self.root.clone(),
                source,
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let stem = name
                .strip_suffix(&format!(".{PROFILE_EXT}.bak"))
                .or_else(|| name.strip_suffix(&format!(".{PROFILE_EXT}.tmp")))
                .or_else(|| name.strip_suffix(&format!(".{PROFILE_EXT}")));
            if let Some(stem) = stem
                && stem.starts_with("driver-")
            {
                ids.push(ProfileId(stem.to_string()));
            }
        }
        // Numeric order — lexicographic would put `driver-10` ahead of
        // `driver-2`; non-standard names (a hand-created `driver-x.json`)
        // sort last.
        ids.sort_by_key(|id| (id.suffix().unwrap_or(u64::MAX), id.as_str().to_string()));
        ids.dedup();
        Ok(ids)
    }

    /// The recorded allocation high-water mark — the lowest suffix
    /// `create` may hand out. `None` when the mark file does not exist
    /// or does not parse; both degrade to the surviving-files floor.
    fn next_id_mark(&self) -> Result<Option<u64>, ProfileError> {
        Ok(self
            .read_marker(NEXT_ID_FILE)?
            .and_then(|text| text.trim().parse().ok()))
    }

    /// Allocate a fresh `driver-<n>` suffix, advancing the on-disk
    /// high-water mark before any file for the new id exists. The mark
    /// is what retires deleted ids: `existing_ids` sees only surviving
    /// files, so without it deleting the highest-numbered profile
    /// would free its suffix for the next `create`. Advancing first is
    /// deliberate — a crash between the mark write and the profile
    /// save wastes a suffix, while the opposite order could hand out
    /// a live id twice.
    fn allocate_id(&self) -> Result<u64, ProfileError> {
        let floor = self
            .existing_ids()?
            .iter()
            .filter_map(ProfileId::suffix)
            .max()
            .map_or(0, |max| max + 1);
        let next = self.next_id_mark()?.unwrap_or(0).max(floor);
        let after = next
            .checked_add(1)
            .ok_or_else(|| ProfileError::Invalid("profile id space exhausted".to_string()))?;
        self.write_marker(NEXT_ID_FILE, &format!("{after}\n"))?;
        Ok(next)
    }

    /// Read a one-token marker file (`active`, `next-id`) with a size
    /// cap — the same bound discipline profile documents get, scaled
    /// to a marker. `None` when the file does not exist.
    fn read_marker(&self, name: &str) -> Result<Option<String>, ProfileError> {
        use std::io::Read;
        let path = self.root.join(name);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ProfileError::Io { path, source }),
        };
        let mut text = String::new();
        file.take(MAX_MARKER_BYTES)
            .read_to_string(&mut text)
            .map_err(|source| ProfileError::Io { path, source })?;
        Ok(Some(text))
    }

    /// Write `contents` to `root/name` atomically — tmp sibling +
    /// `sync_all` + rename + directory fsync — the same durability
    /// shape `save` gives profile documents.
    fn write_marker(&self, name: &str, contents: &str) -> Result<(), ProfileError> {
        let tmp = self.root.join(format!("{name}.tmp"));
        let mut file = std::fs::File::create(&tmp).map_err(|source| ProfileError::Io {
            path: tmp.clone(),
            source,
        })?;
        file.write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|source| ProfileError::Io {
                path: tmp.clone(),
                source,
            })?;
        drop(file);
        std::fs::rename(&tmp, self.root.join(name)).map_err(|source| ProfileError::Io {
            path: self.root.join(name),
            source,
        })?;
        sync_dir(&self.root)
    }

    /// Read every surviving file for `id`. Returns the newest readable
    /// copy — highest `revision`, with earlier slots winning ties —
    /// plus the slot that supplied it (0 = main, 1 = tmp, 2 = backup)
    /// and each existing file's rejection reason.
    fn read_candidates(
        &self,
        id: &ProfileId,
    ) -> (Option<(PlayerProfile, usize)>, [Option<String>; 3]) {
        let paths = [self.main_path(id), self.tmp_path(id), self.backup_path(id)];
        let mut errors: [Option<String>; 3] = [None, None, None];
        let mut best: Option<(PlayerProfile, usize)> = None;
        for (slot, path) in paths.iter().enumerate() {
            if !path.exists() {
                continue;
            }
            match read_profile(path, id) {
                Ok(profile) => {
                    let fresher = best
                        .as_ref()
                        .is_none_or(|(current, _)| profile.revision > current.revision);
                    if fresher {
                        best = Some((profile, slot));
                    }
                }
                Err(reason) => errors[slot] = Some(reason),
            }
        }
        (best, errors)
    }

    /// Every profile in the store, id-ordered. A file that fails to
    /// parse still lists — by id, with its error attached — and an id
    /// surviving only as a `.tmp`/`.bak` lists with that copy's
    /// metadata marked as recovered.
    pub fn list(&self) -> Result<Vec<ProfileSummary>, ProfileError> {
        let mut summaries = Vec::new();
        for id in self.existing_ids()? {
            let (meta, error) = self.summarize(&id);
            summaries.push(ProfileSummary { id, meta, error });
        }
        Ok(summaries)
    }

    /// Listing data for one id: metadata from the newest readable copy
    /// and an error describing any degradation — an unreadable or
    /// missing main file the summary had to recover around.
    fn summarize(&self, id: &ProfileId) -> (Option<ProfileMeta>, Option<String>) {
        let (best, [main_err, tmp_err, backup_err]) = self.read_candidates(id);
        let meta = |p: &PlayerProfile| ProfileMeta {
            name: p.name.clone(),
            rank: p.rank,
            kind: p.kind,
        };
        match best {
            Some((profile, 0)) => (Some(meta(&profile)), None),
            Some((profile, _)) => {
                let main_state = match main_err {
                    Some(reason) => format!("main file unreadable ({reason})"),
                    // A parseable main that lost on `revision` is
                    // superseded, not missing — the recovery picked the
                    // newer surviving copy.
                    None if self.main_path(id).exists() => {
                        "main file holds an older revision".to_string()
                    }
                    None => "main file missing".to_string(),
                };
                (
                    Some(meta(&profile)),
                    Some(format!(
                        "{main_state}; metadata recovered from a surviving copy"
                    )),
                )
            }
            None => {
                let detail = [main_err, tmp_err, backup_err]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("; ");
                (
                    None,
                    Some(if detail.is_empty() {
                        "no readable file survives".to_string()
                    } else {
                        detail
                    }),
                )
            }
        }
    }

    /// Create a fresh profile under the next free `driver-<n>` id and
    /// persist it. Display names may repeat — identity is the id.
    pub fn create(
        &self,
        name: impl Into<String>,
        rank: Difficulty,
        kind: ProfileKind,
    ) -> Result<PlayerProfile, ProfileError> {
        let name = name.into();
        validate_name(&name).map_err(ProfileError::Invalid)?;
        let mut profile = PlayerProfile::new(ProfileId::new(self.allocate_id()?), name, rank, kind);
        self.save(&mut profile)?;
        Ok(profile)
    }

    /// Load `id` from the newest surviving copy — the highest
    /// `revision` among the main, `.tmp` and `.bak` files. An orphaned
    /// `.tmp` is a complete save whose rename never ran, so it is
    /// always fresher than the main it would have replaced; a missing
    /// or unparseable main falls back the same way. Recovery is
    /// reported through [`ProfileLoad::recovered_from_backup`]; corrupt
    /// files are left untouched on disk.
    pub fn load(&self, id: &ProfileId) -> Result<ProfileLoad, ProfileError> {
        if !Self::is_safe_stem(id) {
            return Err(ProfileError::Invalid(format!(
                "malformed profile id {:?}",
                id.as_str()
            )));
        }
        let (best, [main, tmp, backup]) = self.read_candidates(id);
        match best {
            Some((profile, slot)) => Ok(ProfileLoad {
                profile,
                recovered_from_backup: slot != 0,
            }),
            None if main.is_none() && tmp.is_none() && backup.is_none() => {
                Err(ProfileError::UnknownProfile(id.clone()))
            }
            None => Err(ProfileError::Corrupt {
                id: id.clone(),
                main,
                tmp,
                backup,
            }),
        }
    }

    /// Persist `profile` atomically: write `<id>.json.tmp` (flushed to
    /// disk), rotate the current file to `.bak`, then rename the tmp
    /// over it. A crash at any point leaves a complete document under
    /// one of the three names. Bumps `revision` and re-stamps
    /// `version` before serializing.
    pub fn save(&self, profile: &mut PlayerProfile) -> Result<(), ProfileError> {
        profile.version = PROFILE_SCHEMA_VERSION;
        profile.revision += 1;
        profile.validate().map_err(ProfileError::Invalid)?;
        let bytes =
            serde_json::to_vec_pretty(profile).map_err(|e| ProfileError::Invalid(e.to_string()))?;
        let (tmp, main, backup) = (
            self.tmp_path(&profile.id),
            self.main_path(&profile.id),
            self.backup_path(&profile.id),
        );
        let mut file = std::fs::File::create(&tmp).map_err(|source| ProfileError::Io {
            path: tmp.clone(),
            source,
        })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| ProfileError::Io {
                path: tmp.clone(),
                source,
            })?;
        drop(file);
        if main.exists() {
            if backup.exists() {
                std::fs::remove_file(&backup).map_err(|source| ProfileError::Io {
                    path: backup.clone(),
                    source,
                })?;
            }
            std::fs::rename(&main, &backup).map_err(|source| ProfileError::Io {
                path: main.clone(),
                source,
            })?;
        }
        std::fs::rename(&tmp, &main).map_err(|source| ProfileError::Io { path: main, source })?;
        sync_dir(&self.root)?;
        Ok(())
    }

    /// Delete `id`'s files (main, backup, stale tmp). Refuses when it is
    /// the store's last profile (DRV-7). Unknown ids are an explicit
    /// error — deleting nothing silently would hide a bad reference.
    /// The main file unlinks last, so a crash mid-delete leaves either
    /// an intact profile or a recoverable backup — never a
    /// half-removed identity whose id could be reallocated.
    pub fn delete(&self, id: &ProfileId) -> Result<(), ProfileError> {
        if !Self::is_safe_stem(id) {
            return Err(ProfileError::Invalid(format!(
                "malformed profile id {:?}",
                id.as_str()
            )));
        }
        let ids = self.existing_ids()?;
        if !ids.contains(id) {
            return Err(ProfileError::UnknownProfile(id.clone()));
        }
        if ids.len() == 1 {
            return Err(ProfileError::LastProfile(id.clone()));
        }
        for path in [self.tmp_path(id), self.backup_path(id), self.main_path(id)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(ProfileError::Io { path, source }),
            }
        }
        sync_dir(&self.root)?;
        Ok(())
    }

    /// The most recently selected profile, when the `active` marker
    /// names an id that still has a file. A marker that is malformed
    /// or points at a deleted or never-written profile reads as `None`
    /// rather than a stale id.
    pub fn active(&self) -> Result<Option<ProfileId>, ProfileError> {
        let Some(text) = self.read_marker(ACTIVE_FILE)? else {
            return Ok(None);
        };
        let id = ProfileId(text.trim().to_string());
        if Self::is_safe_stem(&id) && self.id_has_file(&id) {
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Record `id` as the selected profile — written like the profile
    /// files (tmp + flush + rename + directory fsync) so a crash cannot
    /// leave a torn marker. `id` must be a well-formed stem with at
    /// least one surviving file.
    pub fn set_active(&self, id: &ProfileId) -> Result<(), ProfileError> {
        if !Self::is_safe_stem(id) {
            return Err(ProfileError::Invalid(format!(
                "malformed profile id {:?}",
                id.as_str()
            )));
        }
        if !self.id_has_file(id) {
            return Err(ProfileError::UnknownProfile(id.clone()));
        }
        self.write_marker(ACTIVE_FILE, &format!("{id}\n"))
    }
}

/// Flush the directory itself so the renames and removals above
/// survive a power loss. Directory fsync is a Unix facility; on other
/// platforms this is a no-op and the file-level `sync_all` calls still
/// stand.
fn sync_dir(dir: &Path) -> Result<(), ProfileError> {
    #[cfg(unix)]
    std::fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|source| ProfileError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Read, size-bound, parse and validate one profile file. The reason
/// string is what [`ProfileError::Corrupt`] surfaces. A parsed profile
/// must agree with the file stem it was loaded from — a hand-renamed
/// or cross-copied file is corrupt, not a different identity.
fn read_profile(path: &Path, expected: &ProfileId) -> Result<PlayerProfile, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("file exceeds {} bytes", MAX_FILE_BYTES));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let profile: PlayerProfile =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid JSON: {e}"))?;
    if &profile.id != expected {
        return Err("file names a different profile id".to_string());
    }
    profile.validate()?;
    Ok(profile)
}
