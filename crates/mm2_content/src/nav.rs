//! VFS producer for a city's [`NavGraph`]: resolve and parse
//! `city/<name>.bai`, then build the shared navigation structure the
//! `mm2_game` contract defines. Like `race_def`, this crate reads the
//! VFS and runs `mm2_formats` parsers; the domain type it fills in
//! stays in `mm2_game`.

use mm2_assets::{AssetsError, Vfs};
use mm2_formats::FormatError;
use mm2_formats::aimap::Aimap;
use mm2_formats::bai::Bai;
use mm2_game::nav::NavBuild;
use mm2_game::nav::NavGraph;
use mm2_game::nav::NavOverrides;
use std::fmt;

/// Why a city's navigation data could not be produced.
#[derive(Debug)]
pub enum NavLoadError {
    /// The logical path did not resolve through the VFS.
    Resolve(String),
    /// The resolved bytes were not readable.
    Read(AssetsError),
    /// The file is not a parseable BAI.
    Parse(FormatError),
    /// The file is not a parseable `.aimap`.
    ParseAimap(FormatError),
}

impl fmt::Display for NavLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NavLoadError::Resolve(p) => write!(f, "{p}: not found in the VFS"),
            NavLoadError::Read(e) => write!(f, "read failed: {e}"),
            NavLoadError::Parse(e) => write!(f, "BAI parse failed: {e}"),
            NavLoadError::ParseAimap(e) => write!(f, "aimap parse failed: {e}"),
        }
    }
}

impl std::error::Error for NavLoadError {}

/// Load `city/<city>.bai` through the VFS and build its navigation
/// graph. Structural problems inside the file are reported on
/// [`NavBuild::issues`], never hidden.
pub fn load_nav_graph(vfs: &Vfs, city: &str) -> Result<NavBuild, NavLoadError> {
    let logical = format!("city/{}.bai", city.to_ascii_lowercase());
    let resolved = vfs
        .resolve(&logical)
        .ok_or(NavLoadError::Resolve(logical))?;
    let bytes = vfs.read(&resolved).map_err(NavLoadError::Read)?;
    let bai = Bai::parse(&bytes).map_err(NavLoadError::Parse)?;
    Ok(NavGraph::build(&bai))
}

/// Load an `.aimap` file through the VFS and distill its navigation
/// overrides (F09-B): closed roads from `[Exceptions]` and the
/// `[Speed Limit]` default. `path` is any logical `.aimap` path —
/// `city/<city>.aimap` for roam traffic, `race/<city>/<event>.aimap`
/// for an event's override set. A path that does not resolve is
/// `Ok(None)` — an absent aimap means *no overrides*, not a failure;
/// a file that resolves but does not parse is `Err`.
pub fn load_nav_overrides(vfs: &Vfs, path: &str) -> Result<Option<NavOverrides>, NavLoadError> {
    let Some(resolved) = vfs.resolve(path) else {
        return Ok(None);
    };
    let bytes = vfs.read(&resolved).map_err(NavLoadError::Read)?;
    let text = String::from_utf8_lossy(&bytes);
    let aimap = Aimap::parse(&text).map_err(NavLoadError::ParseAimap)?;
    Ok(Some(NavOverrides::from_aimap(&aimap)))
}
