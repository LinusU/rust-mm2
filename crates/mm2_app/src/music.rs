//! The session's music state (F08-B.1): explicitly **unsupported**.
//!
//! Retail music is DirectMusic (`aud/dmusic/*.sgt` segments, `.sty`
//! styles, `.dls` banks) steered by five cue tables under
//! `aud/dmusic/csv_files/`. The tables parse
//! ([`mm2_formats::music::MusicTable`]) but the containers are not
//! decoded and the original's row choice and state transitions are
//! unrecovered (UNK-25), so nothing plays: no looping stand-in, no
//! fabricated segment. What this module does is bind the cue table the
//! session would draw from and report that binding — with the
//! `unsupported` state — so a capture or smoke record can tell "music
//! is not implemented" from "music failed". The [`AudioBus::Music`]
//! level exists and is stored; nothing is mixed on it yet.
//!
//! Which table a mode reads is a binding by name, not a recovered
//! rule: a cruise reads `singleroam.csv`, a race event
//! `singlerace.csv`, and a city with an ambience table adds
//! `<city>ambience.csv`. Cops & Robbers binds none (its music source is
//! unrecovered).
//!
//! [`AudioBus::Music`]: crate::settings::AudioBus::Music

use bevy::prelude::*;
use mm2_formats::music::MusicTable;
use mm2_game::{Mm2Vfs, SessionMode};

use crate::audio::AudioReport;

/// The directory the cue tables live in.
const TABLE_DIR: &str = "aud/dmusic/csv_files/";

/// Why no music plays — the one state this build can be in.
pub const UNSUPPORTED: &str = "music: unsupported (DirectMusic not decoded)";

/// The cue tables a session binds and whether they were resolved yet.
#[derive(Resource, Debug)]
pub struct MusicAudio {
    tables: Vec<String>,
    resolved: bool,
}

impl MusicAudio {
    /// Bind the tables for a session of `mode` in `city` (the PSDL
    /// stem; `None` for a dev world, which binds nothing).
    pub fn bind(city: Option<&str>, mode: &SessionMode) -> Option<Self> {
        let city = city?;
        let mut tables = Vec::new();
        match mode {
            SessionMode::Cruise => tables.push(format!("{TABLE_DIR}singleroam.csv")),
            SessionMode::Event(_) => tables.push(format!("{TABLE_DIR}singlerace.csv")),
            SessionMode::CopsAndRobbers(_) => {}
        }
        tables.push(format!(
            "{TABLE_DIR}{}ambience.csv",
            city.to_ascii_lowercase()
        ));
        Some(Self {
            tables,
            resolved: false,
        })
    }
}

/// One bound table as resolved from the VFS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicBinding {
    /// Logical path of the table.
    pub path: String,
    /// Data rows parsed; `None` when the table is missing, unreadable or
    /// unparseable.
    pub rows: Option<usize>,
    /// DirectMusic artifact references the table names.
    pub refs: usize,
}

/// The music half of [`AudioReport`]: bound tables, always unsupported.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MusicReport {
    /// Every table the session bound, in bind order.
    pub tables: Vec<MusicBinding>,
}

impl MusicReport {
    /// The report line, e.g. `music: unsupported (DirectMusic not
    /// decoded); cue tables: singleroam.csv 4 rows/12 refs`.
    pub fn line(&self) -> String {
        let tables: Vec<String> = self
            .tables
            .iter()
            .map(|t| {
                let name = t.path.rsplit('/').next().unwrap_or(&t.path);
                match t.rows {
                    Some(rows) => format!("{name} {rows} rows/{} refs", t.refs),
                    None => format!("{name} missing"),
                }
            })
            .collect();
        if tables.is_empty() {
            format!("{UNSUPPORTED}; no cue table bound")
        } else {
            format!("{UNSUPPORTED}; cue tables: {}", tables.join(", "))
        }
    }

    /// The compact smoke-record field value, e.g.
    /// `unsupported:singleroam=4r/12x`.
    pub fn smoke(&self) -> String {
        let tables: Vec<String> = self
            .tables
            .iter()
            .map(|t| {
                let name = t.path.rsplit('/').next().unwrap_or(&t.path);
                let name = name.strip_suffix(".csv").unwrap_or(name);
                match t.rows {
                    Some(rows) => format!("{name}={rows}r/{}x", t.refs),
                    None => format!("{name}=missing"),
                }
            })
            .collect();
        format!("unsupported:{}", tables.join("+"))
    }
}

/// Resolve the bound tables once per session into the [`AudioReport`].
pub fn music_state(
    music: Option<ResMut<MusicAudio>>,
    vfs: Option<Res<Mm2Vfs>>,
    mut report: ResMut<AudioReport>,
) {
    let (Some(mut music), Some(vfs)) = (music, vfs) else {
        return;
    };
    if music.resolved {
        return;
    }
    music.resolved = true;
    let mut out = MusicReport::default();
    for path in &music.tables {
        let parsed = vfs
            .0
            .read_logical(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                MusicTable::parse(&String::from_utf8_lossy(&bytes)).map_err(|e| e.to_string())
            });
        out.tables.push(match parsed {
            Ok(table) => {
                for d in &table.diagnostics {
                    debug!("music: {path}:{}: {}", d.line, d.message);
                }
                MusicBinding {
                    path: path.clone(),
                    rows: Some(table.rows.len()),
                    refs: table.refs().count(),
                }
            }
            Err(e) => {
                debug!("music: {path}: {e}");
                MusicBinding {
                    path: path.clone(),
                    rows: None,
                    refs: 0,
                }
            }
        });
    }
    report.music = Some(out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cruise_binds_the_roam_table_and_the_city_ambience() {
        let m = MusicAudio::bind(Some("SF"), &SessionMode::Cruise).unwrap();
        assert_eq!(
            m.tables,
            [
                "aud/dmusic/csv_files/singleroam.csv",
                "aud/dmusic/csv_files/sfambience.csv"
            ]
        );
        assert!(MusicAudio::bind(None, &SessionMode::Cruise).is_none());
    }

    #[test]
    fn the_report_line_names_the_state_and_the_tables() {
        let report = MusicReport {
            tables: vec![
                MusicBinding {
                    path: "aud/dmusic/csv_files/singleroam.csv".into(),
                    rows: Some(4),
                    refs: 12,
                },
                MusicBinding {
                    path: "aud/dmusic/csv_files/xambience.csv".into(),
                    rows: None,
                    refs: 0,
                },
            ],
        };
        assert_eq!(
            report.line(),
            "music: unsupported (DirectMusic not decoded); cue tables: \
             singleroam.csv 4 rows/12 refs, xambience.csv missing"
        );
        assert_eq!(
            report.smoke(),
            "unsupported:singleroam=4r/12x+xambience=missing"
        );
        assert!(MusicReport::default().line().starts_with(UNSUPPORTED));
    }
}
