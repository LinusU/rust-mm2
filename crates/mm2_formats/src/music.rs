//! Parser for the `aud/dmusic/csv_files/*.csv` music cue tables.
//!
//! Five small text tables ship beside the DirectMusic containers
//! (measured on retail 2026-10-07). Each is a column-header line
//! followed by data rows whose cells name DirectMusic artifacts by
//! stem — `EnemyStart` is `aud/dmusic/enemystart.sgt`:
//!
//! - `singlerace.csv` — one row per race soundtrack, columns
//!   `Start Music,Return Music,Idle Race Music,Idle Cop Music,Cop chase
//!   music,Pause Music,Race results Music,Big air Motif style,Big Air
//!   Motif name,Big Air Motif Band`.
//! - `singleroam.csv` — one row per cruise soundtrack, the same family
//!   without a results column and with the cop columns in the other
//!   order (`…,Cop Chase Music,idle cop music,…`), so a column is
//!   identified by its header, never its position.
//! - `sfambience.csv`, `londonambience.csv` (`SFX segment`) and
//!   `ui.csv` (`Music segment`) — a single named segment each.
//!
//! The grammar only: which row a session draws, when each state
//! starts, and how DirectMusic segments transition are unrecovered, and
//! the segments themselves are not decoded here.

use crate::racedata::TableDiagnostic;

/// What a column of a music table names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicRole {
    /// `Start Music` — the segment that opens the session.
    Start,
    /// `Return Music` — the segment on returning to the soundtrack
    /// (the roam rows author `…Restart` stems under this header).
    Return,
    /// `Idle Race Music` / `Idle Music` — the loop while driving.
    Idle,
    /// `Idle Cop Music` — the loop while police are near but not chasing.
    IdleCops,
    /// `Cop chase music` — the loop during a pursuit.
    CopChase,
    /// `Pause Music` — the loop under the pause menu.
    Pause,
    /// `Race results Music` — the loop on the results screen.
    Results,
    /// `Big air Motif style` — a `.sty` the airborne motif plays from.
    BigAirStyle,
    /// `Big Air Motif name` — the motif segment (`.sgt`).
    BigAirMotif,
    /// `Big Air Motif Band` — a `.bnd` instrument band.
    BigAirBand,
    /// `SFX segment` — an ambience table's single segment.
    SfxSegment,
    /// `Music segment` — the UI table's single segment.
    MusicSegment,
    /// A header this reader does not recognize; the column is kept.
    Unknown,
}

/// The DirectMusic artifact kind a cell names, deciding its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicArtifact {
    /// A `.sgt` segment.
    Segment,
    /// A `.sty` style.
    Style,
    /// A `.bnd` band.
    Band,
}

impl MusicArtifact {
    /// The file extension the stem resolves under (`aud/dmusic/`).
    pub fn extension(self) -> &'static str {
        match self {
            Self::Segment => "sgt",
            Self::Style => "sty",
            Self::Band => "bnd",
        }
    }
}

impl MusicRole {
    /// Classify a column header, case- and spacing-insensitively.
    pub fn from_header(header: &str) -> Self {
        let h = header.split_whitespace().collect::<Vec<_>>().join(" ");
        match h.to_ascii_lowercase().as_str() {
            "start music" => Self::Start,
            "return music" => Self::Return,
            "idle race music" | "idle music" => Self::Idle,
            "idle cop music" => Self::IdleCops,
            "cop chase music" => Self::CopChase,
            "pause music" => Self::Pause,
            "race results music" => Self::Results,
            "big air motif style" => Self::BigAirStyle,
            "big air motif name" => Self::BigAirMotif,
            "big air motif band" => Self::BigAirBand,
            "sfx segment" => Self::SfxSegment,
            "music segment" => Self::MusicSegment,
            _ => Self::Unknown,
        }
    }

    /// What a cell under this role names; `None` for an unknown column.
    pub fn artifact(self) -> Option<MusicArtifact> {
        match self {
            Self::Unknown => None,
            Self::BigAirStyle => Some(MusicArtifact::Style),
            Self::BigAirBand => Some(MusicArtifact::Band),
            _ => Some(MusicArtifact::Segment),
        }
    }
}

/// One column: the authored header and its recognized role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicColumn {
    /// The header text as authored (trimmed).
    pub header: String,
    /// What the column names.
    pub role: MusicRole,
}

/// One data row — a soundtrack (or the single ambience/UI segment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicRow {
    /// 1-based source line.
    pub line: u32,
    /// One cell per column, trimmed, in header order; a short row's
    /// missing cells are empty and a long row's extra cells are
    /// dropped (both flagged).
    pub cells: Vec<String>,
}

/// A parsed music cue table.
#[derive(Debug, Clone)]
pub struct MusicTable {
    /// The columns in authored order.
    pub columns: Vec<MusicColumn>,
    /// The data rows in authored order.
    pub rows: Vec<MusicRow>,
    /// Recoverable problems (ragged rows, unrecognized headers).
    pub diagnostics: Vec<TableDiagnostic>,
}

/// One artifact reference a table makes: `(row index, role, artifact,
/// stem)` — the stem as authored, never case-folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicRef<'a> {
    /// Index into [`MusicTable::rows`].
    pub row: usize,
    /// The column's role.
    pub role: MusicRole,
    /// Which kind of container the stem names.
    pub artifact: MusicArtifact,
    /// The stem as authored.
    pub stem: &'a str,
}

/// Whether `logical` is one of the music cue tables.
pub fn is_music_table(logical: &str) -> bool {
    let lower = logical.to_ascii_lowercase();
    lower.starts_with("aud/dmusic/csv_files/") && lower.ends_with(".csv")
}

impl MusicTable {
    /// Parse a table. `Err` on input with no header line; everything
    /// else degrades to a diagnostic.
    pub fn parse(input: &str) -> Result<Self, crate::FormatError> {
        // `str::trim` leaves U+FEFF, which would turn the first header
        // into an unrecognized column.
        let input = input.strip_prefix('\u{feff}').unwrap_or(input);
        let mut lines = input
            .lines()
            .enumerate()
            .map(|(i, l)| ((i + 1) as u32, l.trim()))
            .filter(|(_, l)| !l.is_empty());
        let Some((_, header)) = lines.next() else {
            return Err(crate::FormatError::parse(0, "empty music table"));
        };
        let mut diagnostics = Vec::new();
        let columns: Vec<MusicColumn> = header
            .split(',')
            .map(|h| MusicColumn {
                header: h.trim().to_string(),
                role: MusicRole::from_header(h),
            })
            .collect();
        for c in &columns {
            if c.role == MusicRole::Unknown {
                diagnostics.push(TableDiagnostic {
                    line: 1,
                    message: format!("unrecognized music column {:?}", c.header),
                });
            }
        }
        let mut rows = Vec::new();
        for (line, text) in lines {
            let mut cells: Vec<String> = text.split(',').map(|c| c.trim().to_string()).collect();
            if cells.len() != columns.len() {
                diagnostics.push(TableDiagnostic {
                    line,
                    message: format!(
                        "row has {} cells for {} columns",
                        cells.len(),
                        columns.len()
                    ),
                });
                cells.resize(columns.len(), String::new());
            }
            rows.push(MusicRow { line, cells });
        }
        if rows.is_empty() {
            diagnostics.push(TableDiagnostic {
                line: 0,
                message: "music table has no data rows".into(),
            });
        }
        Ok(Self {
            columns,
            rows,
            diagnostics,
        })
    }

    /// The stem under `role` in row `row`; `None` when the table has
    /// no such column/row or the cell is empty.
    pub fn stem(&self, row: usize, role: MusicRole) -> Option<&str> {
        let col = self.columns.iter().position(|c| c.role == role)?;
        self.rows
            .get(row)?
            .cells
            .get(col)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    /// Every non-empty artifact reference, row-major in authored order.
    /// Unknown columns name nothing this reader can resolve and are
    /// skipped.
    pub fn refs(&self) -> impl Iterator<Item = MusicRef<'_>> {
        self.rows.iter().enumerate().flat_map(move |(row, r)| {
            self.columns.iter().zip(&r.cells).filter_map(move |(c, s)| {
                (!s.is_empty()).then_some(())?;
                Some(MusicRef {
                    row,
                    role: c.role,
                    artifact: c.role.artifact()?,
                    stem: s.as_str(),
                })
            })
        })
    }

    /// Cells that are empty under a recognized column — a state the row
    /// leaves without music.
    pub fn empty_cells(&self) -> Vec<(u32, MusicRole)> {
        self.rows
            .iter()
            .flat_map(|r| {
                self.columns
                    .iter()
                    .zip(&r.cells)
                    .filter(|(c, s)| s.is_empty() && c.role.artifact().is_some())
                    .map(|(c, _)| (r.line, c.role))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RACE: &str = "Start Music,Return Music,Idle Race Music,Idle Cop Music,Cop chase music,Pause Music,Race results Music,Big air Motif style,Big Air Motif name,Big Air Motif Band\r\nEnemyStart,EnemyReturn,EnemyIdle,EnemyIdleCops,EnemyCops,Pause,Results1,GrooverStyle,BigAir,BigAir\r\nLondonStart,LondonReturn,LondonIdle,LondonCopsIdle,LondonCops,Pause,results2,GrooverStyle,BigAir,BigAir\r\n";
    const ROAM: &str = "Start Music,Return Music,Idle Music,Cop Chase Music,idle cop music,Pause Music,Big air Motif style,Big Air Motif name,Big Air Motif Band\r\nSunroofStart,SunRoofReturn,SunRoofIdle,SunRoofCops,SunRoofIdle,Pause,GrooverStyle,BigAir,BigAir\r\n";

    #[test]
    fn a_race_table_reads_one_soundtrack_per_row() {
        let t = MusicTable::parse(RACE).unwrap();
        assert!(t.diagnostics.is_empty(), "{:?}", t.diagnostics);
        assert_eq!(t.columns.len(), 10);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.stem(0, MusicRole::Start), Some("EnemyStart"));
        assert_eq!(t.stem(1, MusicRole::IdleCops), Some("LondonCopsIdle"));
        assert_eq!(t.stem(1, MusicRole::Results), Some("results2"));
        assert_eq!(t.stem(0, MusicRole::BigAirBand), Some("BigAir"));
    }

    #[test]
    fn the_cop_columns_are_found_by_header_not_position() {
        let race = MusicTable::parse(RACE).unwrap();
        let roam = MusicTable::parse(ROAM).unwrap();
        // The race table authors idle-cops before chase; roam the reverse.
        assert_eq!(race.stem(0, MusicRole::CopChase), Some("EnemyCops"));
        assert_eq!(roam.stem(0, MusicRole::CopChase), Some("SunRoofCops"));
        assert_eq!(roam.stem(0, MusicRole::IdleCops), Some("SunRoofIdle"));
        assert_eq!(roam.stem(0, MusicRole::Results), None);
    }

    #[test]
    fn refs_name_the_artifact_kind_each_cell_resolves_as() {
        let t = MusicTable::parse(ROAM).unwrap();
        let refs: Vec<_> = t.refs().collect();
        assert_eq!(refs.len(), 9);
        let style = refs
            .iter()
            .find(|r| r.role == MusicRole::BigAirStyle)
            .unwrap();
        assert_eq!(
            (style.artifact, style.stem),
            (MusicArtifact::Style, "GrooverStyle")
        );
        assert_eq!(style.artifact.extension(), "sty");
        let band = refs
            .iter()
            .find(|r| r.role == MusicRole::BigAirBand)
            .unwrap();
        assert_eq!(band.artifact.extension(), "bnd");
        assert!(
            refs.iter()
                .filter(|r| !matches!(r.role, MusicRole::BigAirStyle | MusicRole::BigAirBand))
                .all(|r| r.artifact == MusicArtifact::Segment)
        );
    }

    #[test]
    fn the_single_segment_tables_read_one_cell() {
        let sfx = MusicTable::parse("SFX segment\r\nSFAMbience\r\n").unwrap();
        assert_eq!(sfx.stem(0, MusicRole::SfxSegment), Some("SFAMbience"));
        let ui = MusicTable::parse("Music segment\r\nUI\r\n").unwrap();
        assert_eq!(ui.stem(0, MusicRole::MusicSegment), Some("UI"));
        assert!(ui.diagnostics.is_empty());
    }

    #[test]
    fn a_ragged_row_is_padded_and_flagged() {
        let t = MusicTable::parse("Start Music,Return Music\nA\nB,C,D\n").unwrap();
        assert_eq!(t.diagnostics.len(), 2);
        assert_eq!(t.rows[0].cells, ["A", ""]);
        assert_eq!(t.rows[1].cells, ["B", "C"], "the extra cell is dropped");
        assert_eq!(t.stem(0, MusicRole::Return), None);
        assert_eq!(t.empty_cells(), vec![(2, MusicRole::Return)]);
    }

    #[test]
    fn an_unrecognized_header_is_kept_and_flagged_but_names_nothing() {
        let t = MusicTable::parse("Start Music,Mystery\nA,B\n").unwrap();
        assert_eq!(t.columns[1].role, MusicRole::Unknown);
        assert_eq!(t.diagnostics.len(), 1);
        assert_eq!(t.refs().count(), 1);
    }

    #[test]
    fn a_utf8_byte_order_mark_does_not_hide_the_first_column() {
        let t = MusicTable::parse(&format!("\u{feff}{RACE}")).unwrap();
        assert!(t.diagnostics.is_empty(), "{:?}", t.diagnostics);
        assert_eq!(t.stem(0, MusicRole::Start), Some("EnemyStart"));
    }

    #[test]
    fn empty_input_is_an_error_and_a_header_only_table_is_flagged() {
        assert!(MusicTable::parse("").is_err());
        assert!(MusicTable::parse("\r\n  \r\n").is_err());
        let t = MusicTable::parse("Start Music\n").unwrap();
        assert!(t.rows.is_empty());
        assert_eq!(t.diagnostics.len(), 1);
    }

    #[test]
    fn only_the_dmusic_csv_folder_is_a_music_table() {
        assert!(is_music_table("aud/dmusic/csv_files/singlerace.csv"));
        assert!(is_music_table("AUD/DMusic/csv_files/UI.CSV"));
        assert!(!is_music_table("aud/dmusic/enemystart.sgt"));
        assert!(!is_music_table("aud/spchdata/as1/blitz.csv"));
    }
}
