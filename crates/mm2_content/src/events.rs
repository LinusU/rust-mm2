//! The event catalog (F11-A): every authored event a city ships, keyed
//! by [`EventRef`], with its dependent records resolved through the VFS.
//!
//! The selectable events of a city are the rows of its four
//! `mm*data.csv` tables. Row `i` maps to file stem `<prefix><i>`
//! (`race3`, `blitz0`, `circuit5`, `crash9`) — the table rows carry no
//! name column (`none`, or a Crash Course lesson tag), so this mapping
//! is *inferred* from authored data, not documented. Discovered records
//! beyond the table roster (`race12`, `roam`, `cir1_strtpnts`, …) are
//! kept in [`EventCatalog::extras`] rather than dropped: they may be
//! unreachable from the tables, but they are part of the discovered
//! denominator.
//!
//! Record classification uses the shared [`mm2_formats::racefiles`]
//! grammar. Records with a parser (`*waypoints.csv`/event `.csv`,
//! `_strtpnts`, `.opp`, `crash<N>data.csv`, `<city>_rewards.csv`) are
//! parsed at scan time; `.aimap` (sectioned text) and `.pathset`
//! (binary `PTH1`) are resolved and recorded but not yet parsed.

use std::collections::{BTreeMap, BTreeSet};

use mm2_assets::Vfs;
use mm2_formats::crashdata::CrashDataFile;
use mm2_formats::opp::OppFile;
use mm2_formats::racedata::{EventRow, EventTable, RaceParams};
use mm2_formats::racefiles::{RaceFileKind, classify_race_file, opp_difficulty};
use mm2_formats::rewards::{RewardNum, RewardRow, RewardsFile};
use mm2_formats::waypoints::{StartPointsFile, WaypointFile};
use mm2_game::{EventRef, EventTableKind};

/// Scan order for the four tables: the order a menu presents them on
/// retail (Checkpoint, Blitz, Circuit, Crash Course). Determines
/// [`EventCatalog::events`] ordering — ids never depend on load order.
const TABLE_ORDER: [EventTableKind; 4] = [
    EventTableKind::Checkpoint,
    EventTableKind::Blitz,
    EventTableKind::Circuit,
    EventTableKind::CrashCourse,
];

/// Parse outcome for a record kind that has a parser. The parsed rows
/// are retained — producers (the race definition builder, future
/// opponent/crash-course loaders) consume them directly instead of
/// re-reading the VFS.
#[derive(Debug, Clone)]
pub enum RecordContent {
    /// `*waypoints.csv` / free-standing event `.csv`.
    Waypoints(WaypointFile),
    /// `_strtpnts` headerless start points.
    StartPoints(StartPointsFile),
    /// `.opp` opponent path.
    Opp(OppFile),
    /// `crash<N>data.csv` lesson sub-table; the rows' `Filename`
    /// column names the waypoint CSVs they reference.
    CrashData(CrashDataFile),
    /// Present and resolved; no parser exists for this kind yet
    /// (`.aimap`, `.pathset`, unclassified `.csv`, other records).
    Unparsed,
    /// A parser exists for the kind and it failed — also recorded in
    /// [`CatalogEvent::failed`].
    Failed(String),
}

/// One record attributed to an event's stem.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// Resolved logical path.
    pub logical: String,
    /// File kind per the shared `race/<city>` grammar.
    pub kind: RaceFileKind,
    /// `.opp` difficulty tag (`a`/`p`, inferred Amateur/Professional);
    /// `None` for untagged opps and other kinds.
    pub difficulty: Option<char>,
    /// Parse outcome where a parser exists.
    pub content: RecordContent,
}

/// A reference an event makes that did not resolve or parse.
#[derive(Debug, Clone)]
pub struct FailedRef {
    /// The logical path or referenced name that failed.
    pub reference: String,
    /// Why it failed.
    pub reason: String,
}

/// Completeness of a catalog event relative to the authored record set.
#[derive(Debug, Clone)]
pub enum EventStatus {
    /// Every record the authored convention requires resolved and
    /// parsed, and no reference failed.
    Ready,
    /// One or more required records are missing or failed.
    Incomplete {
        /// Human-readable reasons.
        missing: Vec<String>,
    },
}

impl EventStatus {
    /// Whether every required record resolved.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// One selectable event: a table row plus everything its stem owns.
#[derive(Debug, Clone)]
pub struct CatalogEvent {
    /// Stable identity — `(city, table, row index)`; never load-order
    /// dependent.
    pub event_ref: EventRef,
    /// File stem the row maps to (`race3`); inferred convention.
    pub stem: String,
    /// Authored description tag (`none`, `lesson1`, `final13`).
    pub description: String,
    /// Amateur parameter block.
    pub amateur: RaceParams,
    /// Professional parameter block.
    pub professional: RaceParams,
    /// Every discovered record attributed to this event, sorted by
    /// logical path — includes the waypoint CSVs a Crash Course table
    /// references.
    pub records: Vec<EventRecord>,
    /// References that failed to resolve or parse.
    pub failed: Vec<FailedRef>,
    /// Indexed rewards attached to this event (`crash,N` rows of the
    /// rewards table).
    pub rewards: Vec<RewardRow>,
    /// Completeness against the authored record set.
    pub status: EventStatus,
}

impl CatalogEvent {
    /// The parameter block a difficulty selects on this event —
    /// amateur is the first authored block, professional the second
    /// (inferred split, see `mm2_formats::racedata`).
    pub fn race_params(&self, difficulty: mm2_game::Difficulty) -> &RaceParams {
        match difficulty {
            mm2_game::Difficulty::Amateur => &self.amateur,
            mm2_game::Difficulty::Professional => &self.professional,
        }
    }
}

/// Parse status of one `mm*data.csv` table.
#[derive(Debug, Clone)]
pub struct EventTableStatus {
    /// Which table.
    pub table: EventTableKind,
    /// Logical path scanned.
    pub logical: String,
    /// Rows parsed (0 on failure).
    pub rows: usize,
    /// Recoverable row diagnostics.
    pub diagnostics: usize,
    /// `Some` when the table was absent or unparseable.
    pub error: Option<String>,
}

/// A discovered record group no table row claims.
#[derive(Debug, Clone)]
pub struct ExtraRecord {
    /// Event stem or raw path label.
    pub label: String,
    /// File kinds seen for this stem.
    pub kinds: Vec<RaceFileKind>,
}

/// Why an [`EventRef`] cannot resolve to a usable event.
#[derive(Debug, Clone, PartialEq)]
pub enum EventResolveError {
    /// The ref names a different city than this catalog.
    WrongCity {
        /// Catalog's city.
        catalog: String,
        /// Ref's city.
        requested: String,
    },
    /// No authored row for this ref (missing table or out of bounds).
    UnknownEvent,
    /// The row exists but required records are missing or failed.
    Incomplete {
        /// Human-readable reasons.
        missing: Vec<String>,
    },
}

impl std::fmt::Display for EventResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongCity { catalog, requested } => {
                write!(
                    f,
                    "event is in city {requested:?}, this catalog is {catalog:?}"
                )
            }
            Self::UnknownEvent => write!(f, "no authored event row for this reference"),
            Self::Incomplete { missing } => {
                write!(f, "event dependencies incomplete: {}", missing.join(", "))
            }
        }
    }
}

impl std::error::Error for EventResolveError {}

/// The discovered event roster for one city.
#[derive(Debug, Clone)]
pub struct EventCatalog {
    /// City stem scanned (`london`, `sf`, or a mod city).
    pub city: String,
    /// One entry per authored table row, in `TABLE_ORDER` + row order.
    pub events: Vec<CatalogEvent>,
    /// Per-table scan status, in `TABLE_ORDER`.
    pub tables: Vec<EventTableStatus>,
    /// Milestone rewards (`half`/`all` rows) from
    /// `race/<city>/<city>_rewards.csv`, with their authored race type.
    pub milestone_rewards: Vec<RewardRow>,
    /// Discovered record stems no table row claims (events beyond the
    /// roster, aux records, unclassified files), sorted by label.
    pub extras: Vec<ExtraRecord>,
    /// Catalog-level problems that are not one event's fault (malformed
    /// rewards table, unreadable records).
    pub diagnostics: Vec<String>,
}

impl EventCatalog {
    /// Scan `race/<city>/` through the VFS: parse the four
    /// `mm*data.csv` tables, attribute every discovered record to an
    /// event stem, parse the records that have parsers, validate Crash
    /// Course `Filename` links and attach the rewards table.
    ///
    /// Never fails as a whole: absent/malformed tables land in
    /// [`EventCatalog::tables`]/`diagnostics`, so an empty or partial
    /// install reports honestly instead of erroring out.
    pub fn scan(vfs: &Vfs, city: &str) -> Self {
        let city = city.to_ascii_lowercase();
        let prefix = format!("race/{city}/");

        // Pass 1: classify every basename under race/<city>/ into
        // stem -> records using the shared grammar.
        let mut stems: BTreeMap<String, BTreeMap<String, RaceFileKind>> = BTreeMap::new();
        let mut unclassified: Vec<String> = Vec::new();
        for p in vfs.list() {
            if p == prefix.trim_end_matches('/') {
                unclassified.push(p);
                continue;
            }
            let Some(name) = p.strip_prefix(&prefix) else {
                continue;
            };
            if name.contains('/') {
                unclassified.push(p);
                continue;
            }
            let (kind, stem) = classify_race_file(name);
            match kind {
                RaceFileKind::Junk | RaceFileKind::Meta => {}
                _ => {
                    if let Some(stem) = stem {
                        stems
                            .entry(stem)
                            .or_default()
                            .insert(name.to_string(), kind);
                    }
                }
            }
        }

        let mut catalog = EventCatalog {
            city: city.clone(),
            events: Vec::new(),
            tables: Vec::new(),
            milestone_rewards: Vec::new(),
            extras: Vec::new(),
            diagnostics: Vec::new(),
        };
        let mut claimed: BTreeSet<String> = BTreeSet::new();

        // Pass 2: tables -> events.
        for table_kind in TABLE_ORDER {
            let logical = format!("{prefix}{}", table_kind.file_name());
            let mut status = EventTableStatus {
                table: table_kind,
                logical: logical.clone(),
                rows: 0,
                diagnostics: 0,
                error: None,
            };
            let table = match vfs.read_logical(&logical) {
                Ok(bytes) => match EventTable::parse(&String::from_utf8_lossy(&bytes)) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        status.error = Some(format!("malformed event-metadata table: {e}"));
                        None
                    }
                },
                Err(e) => {
                    status.error = Some(format!("table not resolvable: {e}"));
                    None
                }
            };
            let Some(table) = table else {
                catalog.tables.push(status);
                continue;
            };
            status.rows = table.rows.len();
            status.diagnostics = table.diagnostics.len();
            for d in &table.diagnostics {
                catalog.diagnostics.push(format!("{logical}: {d}"));
            }
            catalog.tables.push(status);
            for (index, row) in table.rows.iter().enumerate() {
                let event_ref = EventRef {
                    city: city.clone(),
                    table: table_kind,
                    index,
                };
                let stem = format!("{}{}", table_kind.stem_prefix(), index);
                claimed.insert(stem.clone());
                catalog.events.push(Self::build_event(
                    vfs,
                    &prefix,
                    event_ref,
                    stem,
                    row,
                    &stems,
                    &mut claimed,
                ));
            }
        }

        // Pass 3: rewards table — milestone rows stay on the catalog,
        // indexed rows attach to their Crash Course event. The table's
        // own stem is claimed so it never surfaces as an extra.
        claimed.insert(format!("{city}_rewards"));
        let rewards_path = format!("{prefix}{city}_rewards.csv");
        match vfs.read_logical(&rewards_path) {
            Ok(bytes) => match RewardsFile::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(rewards) => {
                    for d in &rewards.diagnostics {
                        catalog.diagnostics.push(format!("{rewards_path}: {d}"));
                    }
                    for row in rewards.rows {
                        let mut attached = false;
                        if let RewardNum::Index(i) = &row.race_num
                            && let Some(family) = EventTableKind::from_reward_token(&row.race_type)
                        {
                            let stem = format!("{}{i}", family.stem_prefix());
                            if let Some(ev) = catalog.events.iter_mut().find(|e| e.stem == stem) {
                                ev.rewards.push(row.clone());
                                attached = true;
                            }
                        }
                        if !attached {
                            catalog.milestone_rewards.push(row);
                        }
                    }
                }
                Err(e) => catalog
                    .diagnostics
                    .push(format!("{rewards_path}: malformed rewards table: {e}")),
            },
            Err(_) => catalog.diagnostics.push(format!(
                "{rewards_path}: no rewards table (mods may legitimately omit it)"
            )),
        }

        // Pass 4: everything unclaimed stays visible as extras.
        for (stem, records) in &stems {
            if claimed.contains(stem) {
                continue;
            }
            catalog.extras.push(ExtraRecord {
                label: stem.clone(),
                kinds: records.values().copied().collect(),
            });
        }
        for p in unclassified {
            catalog.extras.push(ExtraRecord {
                label: p,
                kinds: Vec::new(),
            });
        }

        catalog
    }

    /// Build one event from its table row and the stem's records.
    fn build_event(
        vfs: &Vfs,
        prefix: &str,
        event_ref: EventRef,
        stem: String,
        row: &EventRow,
        stems: &BTreeMap<String, BTreeMap<String, RaceFileKind>>,
        claimed: &mut BTreeSet<String>,
    ) -> CatalogEvent {
        let mut records = Vec::new();
        let mut failed = Vec::new();
        let mut missing = Vec::new();
        let mut crash_links: BTreeSet<String> = BTreeSet::new();

        if let Some(files) = stems.get(&stem) {
            for (name, kind) in files {
                let logical = format!("{prefix}{name}");
                let content = Self::parse_record(vfs, &logical, *kind, &mut failed);
                if let RecordContent::CrashData(file) = &content {
                    crash_links.extend(file.rows.iter().map(|r| r.filename.clone()));
                }
                records.push(EventRecord {
                    logical,
                    kind: *kind,
                    difficulty: if *kind == RaceFileKind::Opp {
                        opp_difficulty(name)
                    } else {
                        None
                    },
                    content,
                });
            }
        }

        // SF circuit start grids ship under the short `cir<N>` stem
        // (`cir1_strtpnts`), not `circuit<N>` — claim the sibling stem
        // for circuit events so the authored grid reaches the
        // producer. Same-index mapping is inferred (ledger WPT-3);
        // circuit0 and London ship none, so this is data-driven, not
        // a hard requirement.
        if event_ref.table == EventTableKind::Circuit {
            let alias = format!("cir{}", event_ref.index);
            if let Some(files) = stems.get(&alias) {
                claimed.insert(alias);
                for (name, kind) in files {
                    let logical = format!("{prefix}{name}");
                    let content = Self::parse_record(vfs, &logical, *kind, &mut failed);
                    records.push(EventRecord {
                        logical,
                        kind: *kind,
                        difficulty: None,
                        content,
                    });
                }
            }
        }

        // Crash Course rows reference waypoint CSVs by Filename; each
        // must resolve and parse. The referenced file's own stem is
        // claimed (`frogger0waypoints.csv` owns stem `frogger0`) so it
        // does not surface as an unclaimed extra, and every record the
        // linked stem owns is attached — `follow` pulls in
        // `follow-0.opp`, not just `follow.csv`.
        if event_ref.table == EventTableKind::CrashCourse {
            for link in &crash_links {
                let logical = format!("{prefix}{link}.csv");
                let Some(_r) = vfs.resolve(&logical) else {
                    failed.push(FailedRef {
                        reference: logical,
                        reason: "waypoint CSV referenced by crash data not found".into(),
                    });
                    missing.push(format!("referenced waypoint {link}.csv"));
                    continue;
                };
                let basename = logical.rsplit('/').next().unwrap_or(&logical);
                let (kind, linked_stem) = classify_race_file(basename);
                let mut names: Vec<(String, RaceFileKind)> = Vec::new();
                if let Some(s) = linked_stem {
                    claimed.insert(s.clone());
                    if let Some(files) = stems.get(&s) {
                        names.extend(files.iter().map(|(n, k)| (n.clone(), *k)));
                    }
                }
                if names.is_empty() {
                    names.push((basename.to_string(), kind));
                }
                names.sort();
                for (name, kind) in names {
                    let l = format!("{prefix}{name}");
                    if records.iter().any(|r| r.logical == l) {
                        continue;
                    }
                    let content = Self::parse_record(vfs, &l, kind, &mut failed);
                    records.push(EventRecord {
                        logical: l,
                        kind,
                        difficulty: if kind == RaceFileKind::Opp {
                            opp_difficulty(&name)
                        } else {
                            None
                        },
                        content,
                    });
                }
            }
            records.sort_by(|a, b| a.logical.cmp(&b.logical));
        }

        // Required records per authored convention (see module docs):
        // every stock event ships an .aimap, plus the kind-specific
        // primary record — waypoints for race families, both crash data
        // tables for Crash Course.
        let has = |kind: RaceFileKind, pred: &dyn Fn(&EventRecord) -> bool| {
            records.iter().any(|r| r.kind == kind && pred(r))
        };
        let parsed = |r: &EventRecord| !matches!(r.content, RecordContent::Failed(_));
        if !has(RaceFileKind::Aimap, &|_| true) {
            missing.push(format!("{stem}.aimap"));
        }
        match event_ref.table {
            EventTableKind::CrashCourse => {
                if !has(RaceFileKind::DataCsv, &|r| {
                    r.logical.ends_with("data.csv") && parsed(r)
                }) {
                    missing.push(format!("{stem}data.csv"));
                }
                if !has(RaceFileKind::DataCsv, &|r| {
                    r.logical.ends_with("data_p.csv") && parsed(r)
                }) {
                    missing.push(format!("{stem}data_p.csv"));
                }
            }
            _ => {
                if !has(RaceFileKind::Waypoints, &|r| parsed(r)) {
                    missing.push(format!("{stem}waypoints.csv"));
                }
            }
        }
        // A record that exists but failed to parse also blocks Ready.
        for r in &records {
            if let RecordContent::Failed(e) = &r.content {
                missing.push(format!("{} ({e})", r.logical));
            }
        }

        CatalogEvent {
            event_ref,
            stem,
            description: row.description.clone(),
            amateur: row.amateur.clone(),
            professional: row.professional.clone(),
            records,
            failed,
            rewards: Vec::new(),
            status: if missing.is_empty() {
                EventStatus::Ready
            } else {
                EventStatus::Incomplete { missing }
            },
        }
    }

    /// Parse one record if a parser exists for its kind. Parse failures
    /// are appended to `failed` and returned as
    /// [`RecordContent::Failed`].
    fn parse_record(
        vfs: &Vfs,
        logical: &str,
        kind: RaceFileKind,
        failed: &mut Vec<FailedRef>,
    ) -> RecordContent {
        let bytes = match vfs.read_logical(logical) {
            Ok(b) => b,
            Err(e) => {
                failed.push(FailedRef {
                    reference: logical.to_string(),
                    reason: format!("unreadable record: {e}"),
                });
                return RecordContent::Failed(format!("unreadable: {e}"));
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        match kind {
            RaceFileKind::Waypoints | RaceFileKind::Csv => match WaypointFile::parse(&text) {
                Ok(f) => RecordContent::Waypoints(f),
                Err(e) => {
                    failed.push(FailedRef {
                        reference: logical.to_string(),
                        reason: format!("waypoint parse failed: {e}"),
                    });
                    RecordContent::Failed(e.to_string())
                }
            },
            RaceFileKind::StartPoints => match StartPointsFile::parse(&text) {
                Ok(f) => RecordContent::StartPoints(f),
                Err(e) => {
                    failed.push(FailedRef {
                        reference: logical.to_string(),
                        reason: format!("start-points parse failed: {e}"),
                    });
                    RecordContent::Failed(e.to_string())
                }
            },
            RaceFileKind::Opp => match OppFile::parse(&text) {
                Ok(f) => RecordContent::Opp(f),
                Err(e) => {
                    failed.push(FailedRef {
                        reference: logical.to_string(),
                        reason: format!("opponent-path parse failed: {e}"),
                    });
                    RecordContent::Failed(e.to_string())
                }
            },
            RaceFileKind::DataCsv => match CrashDataFile::parse(&text) {
                Ok(f) => RecordContent::CrashData(f),
                Err(e) => {
                    failed.push(FailedRef {
                        reference: logical.to_string(),
                        reason: format!("crash-data parse failed: {e}"),
                    });
                    RecordContent::Failed(e.to_string())
                }
            },
            _ => RecordContent::Unparsed,
        }
    }

    /// Look up an event by ref. `None` for a different city, a missing
    /// table or an out-of-bounds row.
    pub fn get(&self, event_ref: &EventRef) -> Option<&CatalogEvent> {
        if event_ref.city.to_ascii_lowercase() != self.city {
            return None;
        }
        self.events.iter().find(|e| e.event_ref == *event_ref)
    }

    /// Resolve an [`EventRef`] to a usable event — the AC06 dependency
    /// check: the row must exist *and* every required record must have
    /// resolved.
    pub fn resolve(&self, event_ref: &EventRef) -> Result<&CatalogEvent, EventResolveError> {
        if event_ref.city.to_ascii_lowercase() != self.city {
            return Err(EventResolveError::WrongCity {
                catalog: self.city.clone(),
                requested: event_ref.city.clone(),
            });
        }
        let Some(ev) = self.get(event_ref) else {
            return Err(EventResolveError::UnknownEvent);
        };
        match &ev.status {
            EventStatus::Ready => Ok(ev),
            EventStatus::Incomplete { missing } => Err(EventResolveError::Incomplete {
                missing: missing.clone(),
            }),
        }
    }

    /// Whether any table row produced an event.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
