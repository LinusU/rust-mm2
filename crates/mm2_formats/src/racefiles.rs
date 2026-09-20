//! Filename grammar for `race/<city>/` records.
//!
//! Event-related files ship under one directory per city and are bound
//! to an event *stem* purely by name — `blitz3.aimap`,
//! `blitz3waypoints.csv`, `circuit0-a-2.opp`. This module is the shared
//! classification both the inventory audit and the event catalog use, so
//! the two never disagree about which record belongs to which event.
//!
//! Semantics of the kinds are *documented* (observed on retail data),
//! not claims about what the original engine requires: partial entries —
//! an event stem carrying only some of the records its siblings carry —
//! are exactly what audits exist to surface.

/// Authored-file kinds found under `race/<city>/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RaceFileKind {
    /// `<stem>.aimap` — text AI-map / event-side configuration sections.
    Aimap,
    /// `<stem>.aimap_p` — `_p` variant (difficulty vs multiplayer role
    /// unverified).
    AimapP,
    /// `<stem>.pathset` — binary `PTH1` prop/path placement.
    Pathset,
    /// `<stem>waypoints.csv` — checkpoint/waypoint records.
    Waypoints,
    /// `<stem>data.csv` / `<stem>data_p.csv` — Crash Course per-lesson
    /// event tables.
    DataCsv,
    /// Any other `<stem>.csv` record.
    Csv,
    /// `<stem>.opp`, `<stem>-N.opp`, `<stem>-a-N.opp`, `<stem>-p-N.opp` —
    /// opponent path records.
    Opp,
    /// `<stem>_strtpnts` — headerless start-point records.
    StartPoints,
    /// `mm*data.csv` — the city event-metadata tables (no event stem).
    Meta,
    /// Backup/conflict/dev leftovers — not content.
    Junk,
    /// Anything else; the whole basename is the stem.
    Other,
}

/// Classify a `race/<city>/` basename; returns the kind and the event
/// stem the record belongs to (records with no stem — metadata tables,
/// junk — return `None`).
pub fn classify_race_file(name: &str) -> (RaceFileKind, Option<String>) {
    if name.starts_with(".#") {
        return (RaceFileKind::Junk, None); // version-control conflict artifact
    }
    for ext in [
        ".bak", ".old", ".csvs", ".ps2", ".short", ".pt", ".bat", ".tmp",
    ] {
        if name.ends_with(ext) {
            return (RaceFileKind::Junk, None);
        }
    }
    if name.starts_with("mm") && name.ends_with("data.csv") {
        return (RaceFileKind::Meta, None); // e.g. mmracedata.csv city event table
    }
    let (kind, stem) = if let Some(s) = stem_of(name, &[".aimap_p"]) {
        (RaceFileKind::AimapP, s)
    } else if let Some(s) = stem_of(name, &[".aimap"]) {
        (RaceFileKind::Aimap, s)
    } else if let Some(s) = stem_of(name, &[".pathset"]) {
        (RaceFileKind::Pathset, s)
    } else if let Some(s) = stem_of(name, &["waypoints.csv"]) {
        (RaceFileKind::Waypoints, s)
    } else if let Some(s) = stem_of(name, &["data_p.csv", "data.csv"]) {
        (RaceFileKind::DataCsv, s)
    } else if let Some(s) = stem_of(name, &["_strtpnts"]) {
        (RaceFileKind::StartPoints, s)
    } else if let Some(s) = stem_of(name, &[".opp"]) {
        // `x-a-N.opp` / `x-p-N.opp` / `x-N.opp` all belong to event `x`.
        let mut s = s;
        if let Some((base, tail)) = s.rsplit_once('-')
            && tail.chars().all(|c| c.is_ascii_digit())
        {
            s = base;
        }
        if let Some((base, tail)) = s.rsplit_once('-')
            && matches!(tail, "a" | "p")
        {
            s = base;
        }
        (RaceFileKind::Opp, s)
    } else if let Some(s) = stem_of(name, &[".csv"]) {
        (RaceFileKind::Csv, s)
    } else {
        (RaceFileKind::Other, name)
    };
    (kind, Some(stem.to_string()))
}

/// Strip one trailing `.ext` (or a known multi-part tail) from `name`.
fn stem_of<'a>(name: &'a str, suffixes: &[&str]) -> Option<&'a str> {
    suffixes.iter().find_map(|s| name.strip_suffix(s))
}

/// The difficulty tag an `.opp` filename carries, if any.
///
/// `-a-N` / `-p-N` suffixes are *inferred* to be the Amateur/Professional
/// opponent sets (the `-a-` set is easier on retail data); a bare `-N`
/// or no suffix returns `None`.
pub fn opp_difficulty(name: &str) -> Option<char> {
    let stem = name.strip_suffix(".opp")?;
    let (base, tail) = stem.rsplit_once('-')?;
    if tail.chars().all(|c| c.is_ascii_digit()) {
        let (_, diff) = base.rsplit_once('-')?;
        matches!(diff, "a" | "p").then(|| diff.chars().next().unwrap())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn race_file_classification() {
        use RaceFileKind as K;
        assert_eq!(
            classify_race_file("blitz0.aimap"),
            (K::Aimap, Some("blitz0".into()))
        );
        assert_eq!(
            classify_race_file("roam.aimap_p"),
            (K::AimapP, Some("roam".into()))
        );
        assert_eq!(
            classify_race_file("circuit0-a-3.opp"),
            (K::Opp, Some("circuit0".into()))
        );
        assert_eq!(
            classify_race_file("circuit0-p-7.opp"),
            (K::Opp, Some("circuit0".into()))
        );
        assert_eq!(
            classify_race_file("exam1_2.opp"),
            (K::Opp, Some("exam1_2".into()))
        );
        assert_eq!(
            classify_race_file("follow-0.opp"),
            (K::Opp, Some("follow".into()))
        );
        assert_eq!(
            classify_race_file("final.opp"),
            (K::Opp, Some("final".into()))
        );
        assert_eq!(
            classify_race_file("crash0data_p.csv"),
            (K::DataCsv, Some("crash0".into()))
        );
        assert_eq!(
            classify_race_file("blitz0waypoints.csv"),
            (K::Waypoints, Some("blitz0".into()))
        );
        assert_eq!(
            classify_race_file("cir1_strtpnts"),
            (K::StartPoints, Some("cir1".into()))
        );
        assert_eq!(
            classify_race_file("london_bridge_multi.pathset"),
            (K::Pathset, Some("london_bridge_multi".into()))
        );
        assert_eq!(
            classify_race_file("reverse180_p.csv"),
            (K::Csv, Some("reverse180_p".into()))
        );
        assert_eq!(classify_race_file("mmracedata.csv").0, K::Meta);
        assert_eq!(classify_race_file(".#race6.opp.1.1").0, K::Junk);
        assert_eq!(classify_race_file("crash12data.csv.old").0, K::Junk);
        assert_eq!(classify_race_file("blitz12waypoints.csvs").0, K::Junk);
        assert_eq!(classify_race_file("dbugps2.ps2").0, K::Junk);
    }

    #[test]
    fn opp_difficulty_tags() {
        assert_eq!(opp_difficulty("circuit0-a-3.opp"), Some('a'));
        assert_eq!(opp_difficulty("circuit0-p-7.opp"), Some('p'));
        assert_eq!(opp_difficulty("follow-0.opp"), None);
        assert_eq!(opp_difficulty("final.opp"), None);
    }
}
