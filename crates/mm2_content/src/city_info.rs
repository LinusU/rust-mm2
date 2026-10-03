//! City display metadata from `tune/<city>.cinfo`: the localized city
//! name and the race names the original's menus show ("Racing 101",
//! "Take It Easy").
//!
//! The file is the same `Key=Value` text as vehicle `.info` metadata.
//! Retail ships one per stock city:
//!
//! ```text
//! LocalizedName=San Francisco
//! CheckpointNames=Racing 101|Deck The Hall|...
//! BlitzNames=Ignorance Is Blitz|Hold On Tight|...
//! CircuitNames=Take It Easy|Hang Time|...
//! ```
//!
//! Name `i` of a family list belongs to row `i` of that family's
//! `mm*data.csv` table (`CheckpointNames` ↔ `mmracedata.csv`). The
//! pairing is *inferred*: retail's `CheckpointCount`/`BlitzCount`/
//! `CircuitCount` equal each table's row count, and the first
//! checkpoint being "Racing 101" fits the open first set (CHK-2), but
//! no source documents the index mapping. Crash Course lessons carry
//! no names here — their table rows hold the lesson tags instead.

use mm2_assets::Vfs;
use mm2_formats::info::InfoFile;
use mm2_game::EventTableKind;

/// One city's display metadata. Every field is optional content: a mod
/// city without a `.cinfo` simply has no names, and the menu falls back
/// to the stem.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CityInfo {
    /// `LocalizedName` — "San Francisco" for `sf`.
    pub localized_name: Option<String>,
    /// `CheckpointNames`, in `mmracedata.csv` row order.
    pub checkpoint_names: Vec<String>,
    /// `BlitzNames`, in `mmblitzdata.csv` row order.
    pub blitz_names: Vec<String>,
    /// `CircuitNames`, in `mmcircuitdata.csv` row order.
    pub circuit_names: Vec<String>,
}

impl CityInfo {
    /// Read `tune/<city>.cinfo` through the VFS — `None` when the city
    /// ships none or it cannot be read.
    pub fn load(vfs: &Vfs, city: &str) -> Option<Self> {
        let resolved = vfs.resolve(&format!("tune/{city}.cinfo"))?;
        let bytes = vfs.read(&resolved).ok()?;
        Some(Self::parse(&String::from_utf8_lossy(&bytes)))
    }

    /// Parse `.cinfo` text. Unknown keys are ignored; absent lists stay
    /// empty.
    pub fn parse(text: &str) -> Self {
        let info = InfoFile::parse(text);
        let list = |key: &str| {
            info.get_ci(key)
                .map(|v| {
                    v.split('|')
                        .map(|s| s.trim().to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        Self {
            localized_name: info
                .get_ci("LocalizedName")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(Into::into),
            checkpoint_names: list("CheckpointNames"),
            blitz_names: list("BlitzNames"),
            circuit_names: list("CircuitNames"),
        }
    }

    /// The display name of row `index` in `table` — `None` past the
    /// authored list, for an empty entry, and for Crash Course.
    pub fn race_name(&self, table: EventTableKind, index: usize) -> Option<&str> {
        let names = match table {
            EventTableKind::Checkpoint => &self.checkpoint_names,
            EventTableKind::Blitz => &self.blitz_names,
            EventTableKind::Circuit => &self.circuit_names,
            EventTableKind::CrashCourse => return None,
        };
        names
            .get(index)
            .map(String::as_str)
            .filter(|n| !n.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SF: &str = "LocalizedName=San Francisco\r\n\
MapName=sf\r\n\
BlitzCount=2\r\n\
BlitzNames=Ignorance Is Blitz|Hold On Tight\r\n\
CircuitNames=Take It Easy||Gimme SOMA\r\n\
CheckpointNames=Racing 101|Panoz Pressure \r\n";

    #[test]
    fn names_follow_table_row_order() {
        let info = CityInfo::parse(SF);
        assert_eq!(info.localized_name.as_deref(), Some("San Francisco"));
        assert_eq!(
            info.race_name(EventTableKind::Checkpoint, 0),
            Some("Racing 101")
        );
        // Trailing whitespace in the authored list is not part of the
        // name.
        assert_eq!(
            info.race_name(EventTableKind::Checkpoint, 1),
            Some("Panoz Pressure")
        );
        assert_eq!(
            info.race_name(EventTableKind::Blitz, 1),
            Some("Hold On Tight")
        );
        // An empty slot keeps the later names on their own rows.
        assert_eq!(info.race_name(EventTableKind::Circuit, 1), None);
        assert_eq!(
            info.race_name(EventTableKind::Circuit, 2),
            Some("Gimme SOMA")
        );
    }

    #[test]
    fn missing_names_fall_through() {
        let info = CityInfo::parse(SF);
        assert_eq!(info.race_name(EventTableKind::Blitz, 2), None);
        assert_eq!(info.race_name(EventTableKind::CrashCourse, 0), None);
        assert_eq!(CityInfo::parse(""), CityInfo::default());
    }
}
