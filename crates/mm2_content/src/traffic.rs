//! `city/<city>.aimap` → [`AmbientRoster`] producer and ambient asset
//! audit (F10-A.1).
//!
//! A city's `[Ambient Types/Density]` rows name the ambient classes the
//! session draws from; each id's `tune/vehicle/<id>.aivehicledata` is
//! decoded through `mm2_formats::veh` and the pair becomes the shared
//! [`AmbientRoster`] contract the seeded planner consumes. A row whose
//! tune record does not resolve stays in the table with `tuning: None`
//! — the authored weight band is preserved and the planner counts draws
//! against it as unspawnable rather than rebalancing silently.
//!
//! [`TrafficAudit::scan`] is the audit side: the same producer's roster
//! plus a per-id check of the full asset set (`aivehicledata`,
//! `geometry/<id>.pkg`, `bound/<id>_bound.bnd`, `geometry/<id>_*.mtx`
//! transform count), the ambient tune files no roster references, and
//! every event `.aimap`/`.aimap_p` under `race/<city>/` that carries
//! ambient-relevant overrides (`[Density]`, `[Speed Limit]`,
//! `[Exceptions]`, `[Ambient Types/Density]`).

use std::collections::BTreeSet;
use std::fmt;

use mm2_assets::Vfs;
use mm2_formats::aimap::Aimap;
use mm2_formats::bnd::BndFile;
use mm2_formats::pkg::Pkg;
use mm2_formats::tune::TuneFile;
use mm2_formats::veh::AiVehicleData;
use mm2_game::traffic::{AmbientRoster, AmbientSpec};

use crate::expect::EXPECTED_AMBIENTS;

/// Why a city's ambient roster cannot be produced at all.
#[derive(Debug)]
pub enum TrafficLoadError {
    /// `city/<city>.aimap` does not resolve through the VFS — the roam
    /// aimap is expected content on a stock city.
    Resolve(String),
    /// The aimap resolved but could not be read or parsed.
    Aimap(String),
}

impl fmt::Display for TrafficLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolve(p) => write!(f, "{p}: not found in the VFS"),
            Self::Aimap(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TrafficLoadError {}

/// Parse `city/<city>.aimap`; `Ok(None)` when the file does not
/// resolve (an absent aimap is no ambient config, not a failure).
pub fn load_city_aimap(vfs: &Vfs, city: &str) -> Result<Option<Aimap>, TrafficLoadError> {
    let logical = format!("city/{}.aimap", city.to_ascii_lowercase());
    let Some(resolved) = vfs.resolve(&logical) else {
        return Ok(None);
    };
    let bytes = vfs
        .read(&resolved)
        .map_err(|e| TrafficLoadError::Aimap(format!("{logical}: read failed: {e}")))?;
    let aimap = Aimap::parse(&String::from_utf8_lossy(&bytes))
        .map_err(|e| TrafficLoadError::Aimap(format!("{logical}: {e}")))?;
    Ok(Some(aimap))
}

/// Decode `tune/vehicle/<id>.aivehicledata`; `Ok(None)` when the record
/// does not resolve, `Err` when it resolves but does not parse or
/// decode.
fn load_tuning(vfs: &Vfs, id: &str) -> Result<Option<AiVehicleData>, String> {
    let logical = format!("tune/vehicle/{id}.aivehicledata");
    let Some(resolved) = vfs.resolve(&logical) else {
        return Ok(None);
    };
    let bytes = vfs
        .read(&resolved)
        .map_err(|e| format!("{logical}: read failed: {e}"))?;
    let tune =
        TuneFile::parse(&String::from_utf8_lossy(&bytes)).map_err(|e| format!("{logical}: {e}"))?;
    AiVehicleData::from_tune(&tune)
        .map(Some)
        .map_err(|e| format!("{logical}: {e}"))
}

/// Build the ambient roster a `city/<city>.aimap` authors, resolving
/// each row's tune record through the VFS. `Ok(None)` when the city
/// ships no aimap. Rows whose tuning fails keep their slot with
/// `tuning: None`; the failure is recorded in the returned diagnostics.
pub fn ambient_roster(
    vfs: &Vfs,
    city: &str,
) -> Result<Option<(AmbientRoster, Vec<String>)>, TrafficLoadError> {
    let Some(aimap) = load_city_aimap(vfs, city)? else {
        return Ok(None);
    };
    Ok(Some(ambient_roster_from_aimap(vfs, &aimap)))
}

/// Distill any parsed aimap's `[Ambient Types/Density]` table into a
/// roster — city aimaps and event aimaps share the shape, and retail
/// `race/london/roam.aimap{,_p}` authors a different roster than the
/// city file, so event sessions can feed their own aimap through here.
pub fn ambient_roster_from_aimap(vfs: &Vfs, aimap: &Aimap) -> (AmbientRoster, Vec<String>) {
    let mut diagnostics = Vec::new();
    let entries = aimap
        .ambient_types
        .iter()
        .map(|row| {
            let (tuning, reason) = match load_tuning(vfs, &row.name) {
                Ok(None) => (None, Some("no tune/vehicle record resolves".to_string())),
                Ok(t) => (t, None),
                Err(e) => (None, Some(e)),
            };
            if let Some(r) = reason {
                diagnostics.push(format!("{}: {r}", row.name));
            }
            AmbientSpec {
                id: row.name.clone(),
                cumulative_weight: row.weight,
                flag: row.flag,
                tuning,
            }
        })
        .collect();
    (AmbientRoster::new(entries), diagnostics)
}

/// Resolution state of one asset a rostered ambient class needs.
#[derive(Debug, Clone, PartialEq)]
pub enum AssetCheck {
    /// Nothing resolves at the logical path.
    Missing,
    /// The record resolves and reads but the parser/decoder rejects it.
    Failed(String),
    /// The record resolves and parses.
    Parsed,
}

/// One rostered class's full asset set.
#[derive(Debug, Clone)]
pub struct AmbientAssets {
    /// The `va_*` id.
    pub id: String,
    /// `tune/vehicle/<id>.aivehicledata`.
    pub tuning: AssetCheck,
    /// `geometry/<id>.pkg` — the model package.
    pub geometry: AssetCheck,
    /// `bound/<id>_bound.bnd` — the collision bound.
    pub bound: AssetCheck,
    /// `geometry/<id>_*.mtx` transform parts found (wheel/headlight
    /// placements; `va_cablecar_f` legitimately ships none).
    pub mtx_parts: usize,
    /// Decode warnings the tune record reported.
    pub tuning_warnings: Vec<String>,
}

/// The ambient-relevant sections one `race/<city>/*.aimap*` overrides.
#[derive(Debug, Clone)]
pub struct EventOverride {
    /// Logical path of the event aimap.
    pub logical: String,
    /// `[Density]` when authored.
    pub density: Option<f32>,
    /// `[Speed Limit]` when authored.
    pub speed_limit: Option<f32>,
    /// `[Exceptions]` row count (zero-density rows close roads).
    pub exceptions: usize,
    /// `[Ambient Types/Density]` row count — a non-empty table replaces
    /// the city's roster for that event.
    pub ambient_types: usize,
    /// Parse/read failure, when the file resolved but did not parse.
    pub failed: Option<String>,
}

/// Per-city ambient audit: roster, per-class assets, unrostered
/// ambient files and event-level overrides.
#[derive(Debug)]
pub struct TrafficAudit {
    /// City stem audited.
    pub city: String,
    /// `city/<city>.aimap` resolved.
    pub aimap_present: bool,
    /// Aimap read/parse failure, when any.
    pub aimap_error: Option<String>,
    /// The built roster (with structural issues), when the aimap parsed.
    pub roster: Option<AmbientRoster>,
    /// Non-fatal diagnostics: aimap row/validation problems and per-row
    /// tuning load failures.
    pub diagnostics: Vec<String>,
    /// Per-id asset status for every distinct rostered id.
    pub assets: Vec<AmbientAssets>,
    /// `tune/vehicle/*.aivehicledata` ids no row of this city's roster
    /// references (discovered extras — the pool is shared, so a city
    /// legitimately rosters a subset).
    pub unrostered: Vec<String>,
    /// Expected stock ids never discovered anywhere in the VFS.
    pub missing_expected: Vec<String>,
    /// Event aimaps under `race/<city>/` carrying ambient overrides,
    /// plus any that resolved but failed to parse.
    pub event_overrides: Vec<EventOverride>,
}

impl TrafficAudit {
    /// Scan one city through the production VFS/producer path. Never
    /// fails as a whole — a partial install reports honestly.
    pub fn scan(vfs: &Vfs, city: &str) -> Self {
        let mut diagnostics = Vec::new();
        let (aimap_present, aimap_error, roster) = match load_city_aimap(vfs, city) {
            Ok(Some(aimap)) => {
                diagnostics.extend(
                    aimap
                        .diagnostics
                        .iter()
                        .map(|d| format!("city/{city}.aimap line {}: {}", d.line, d.message)),
                );
                diagnostics.extend(
                    aimap
                        .validate()
                        .iter()
                        .map(|i| format!("city/{city}.aimap: {i}")),
                );
                let (roster, mut diags) = ambient_roster_from_aimap(vfs, &aimap);
                diagnostics.append(&mut diags);
                (true, None, Some(roster))
            }
            Ok(None) => (false, None, None),
            Err(e) => (false, Some(e.to_string()), None),
        };

        // Every discovered ambient tune file, whether rostered or not —
        // the discovered denominator is independent of the roster.
        let tune_ids: BTreeSet<String> = vfs
            .list()
            .iter()
            .filter_map(|p| {
                p.strip_prefix("tune/vehicle/")
                    .and_then(|s| s.strip_suffix(".aivehicledata"))
                    .map(str::to_string)
            })
            .collect();

        let rostered: BTreeSet<String> = roster
            .as_ref()
            .map(|r| r.entries.iter().map(|e| e.id.clone()).collect())
            .unwrap_or_default();

        let assets = rostered.iter().map(|id| audit_assets(vfs, id)).collect();

        let unrostered = tune_ids.difference(&rostered).cloned().collect();
        let missing_expected = EXPECTED_AMBIENTS
            .iter()
            .filter(|id| !tune_ids.contains(**id))
            .map(|id| id.to_string())
            .collect();

        let prefix = format!("race/{city}/");
        let mut event_overrides = Vec::new();
        for logical in vfs.list() {
            let Some(rest) = logical.strip_prefix(&prefix) else {
                continue;
            };
            if !(rest.ends_with(".aimap") || rest.ends_with(".aimap_p")) {
                continue;
            }
            let entry = match vfs.read_logical(&logical) {
                Ok(bytes) => match Aimap::parse(&String::from_utf8_lossy(&bytes)) {
                    Ok(a) => {
                        if a.density.is_none()
                            && a.speed_limit.is_none()
                            && a.exceptions.is_empty()
                            && a.ambient_types.is_empty()
                        {
                            continue; // no ambient-relevant override authored
                        }
                        EventOverride {
                            logical: logical.clone(),
                            density: a.density,
                            speed_limit: a.speed_limit,
                            exceptions: a.exceptions.len(),
                            ambient_types: a.ambient_types.len(),
                            failed: None,
                        }
                    }
                    Err(e) => EventOverride {
                        logical: logical.clone(),
                        density: None,
                        speed_limit: None,
                        exceptions: 0,
                        ambient_types: 0,
                        failed: Some(e.to_string()),
                    },
                },
                Err(e) => EventOverride {
                    logical: logical.clone(),
                    density: None,
                    speed_limit: None,
                    exceptions: 0,
                    ambient_types: 0,
                    failed: Some(format!("read failed: {e}")),
                },
            };
            event_overrides.push(entry);
        }

        Self {
            city: city.to_string(),
            aimap_present,
            aimap_error,
            roster,
            diagnostics,
            assets,
            unrostered,
            missing_expected,
            event_overrides,
        }
    }

    /// Records that failed their check: a missing or malformed expected
    /// aimap, per-class asset failures, undiscovered expected ambient
    /// ids, unparseable event aimaps, roster structural issues and any
    /// diagnostics — incomplete accounting counts.
    pub fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.aimap_present {
            out.push(format!("city/{}.aimap does not resolve", self.city));
        }
        if let Some(e) = &self.aimap_error {
            out.push(format!("city/{}.aimap: {e}", self.city));
        }
        for a in &self.assets {
            for (what, check) in [
                ("tuning", &a.tuning),
                ("geometry", &a.geometry),
                ("bound", &a.bound),
            ] {
                match check {
                    AssetCheck::Missing => out.push(format!("{}: {what} record missing", a.id)),
                    AssetCheck::Failed(e) => {
                        out.push(format!("{}: {what} record failed: {e}", a.id))
                    }
                    AssetCheck::Parsed => {}
                }
            }
        }
        for id in &self.missing_expected {
            out.push(format!("{id}: expected stock ambient never discovered"));
        }
        for o in &self.event_overrides {
            if let Some(e) = &o.failed {
                out.push(format!("{}: {e}", o.logical));
            }
        }
        if let Some(r) = &self.roster {
            for i in &r.issues {
                out.push(format!("roster: {i}"));
            }
        }
        out.extend(self.diagnostics.iter().cloned());
        out
    }

    /// Expected rostered-class count for the accounting line: the
    /// discovered tune files the city could draw from.
    pub fn expected(&self) -> usize {
        EXPECTED_AMBIENTS.len()
    }

    /// Discovered ambient tune files (rostered + unrostered).
    pub fn discovered(&self) -> usize {
        let rostered = self.assets.len();
        rostered + self.unrostered.len()
    }
}

/// Deep-check one class's assets: tune parse+decode, PKG parse, BND
/// parse, `.mtx` part count.
fn audit_assets(vfs: &Vfs, id: &str) -> AmbientAssets {
    let tuning_logical = format!("tune/vehicle/{id}.aivehicledata");
    let (tuning, tuning_warnings) = match vfs.resolve(&tuning_logical) {
        None => (AssetCheck::Missing, Vec::new()),
        Some(r) => match vfs.read(&r) {
            Err(e) => (AssetCheck::Failed(format!("read failed: {e}")), Vec::new()),
            Ok(bytes) => match TuneFile::parse(&String::from_utf8_lossy(&bytes)) {
                Err(e) => (AssetCheck::Failed(e.to_string()), Vec::new()),
                Ok(t) => match AiVehicleData::from_tune(&t) {
                    Err(e) => (AssetCheck::Failed(e.to_string()), Vec::new()),
                    Ok(d) => (AssetCheck::Parsed, d.warnings.clone()),
                },
            },
        },
    };

    let geometry = check_parseable(vfs, &format!("geometry/{id}.pkg"), |b| {
        Pkg::parse(b).map(|_| ())
    });
    let bound = check_parseable(vfs, &format!("bound/{id}_bound.bnd"), |b| {
        BndFile::parse(&String::from_utf8_lossy(b)).map(|_| ())
    });
    let mtx_parts = vfs
        .list()
        .iter()
        .filter(|p| p.starts_with(&format!("geometry/{id}_")) && p.ends_with(".mtx"))
        .count();

    AmbientAssets {
        id: id.to_string(),
        tuning,
        geometry,
        bound,
        mtx_parts,
        tuning_warnings,
    }
}

fn check_parseable<E: fmt::Display>(
    vfs: &Vfs,
    logical: &str,
    parse: impl Fn(&[u8]) -> Result<(), E>,
) -> AssetCheck {
    let Some(resolved) = vfs.resolve(logical) else {
        return AssetCheck::Missing;
    };
    match vfs.read(&resolved) {
        Err(e) => AssetCheck::Failed(format!("read failed: {e}")),
        Ok(bytes) => match parse(&bytes) {
            Ok(()) => AssetCheck::Parsed,
            Err(e) => AssetCheck::Failed(e.to_string()),
        },
    }
}
