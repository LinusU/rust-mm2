//! Cops & Robbers content and original option tables (F27-A).
//!
//! Everything the original ships *as data* for the mode lives in a
//! handful of ordinary files, and the rest of the rule surface is code
//! constants recovered from `Midtown2.exe`. This module keeps the two
//! apart: [`CnrContent`] is the VFS-resolved side (placement waypoints,
//! marker models, banger records, HUD textures, commentary tables) and
//! the constants below are the code-defined side, each with the address
//! it was read from in `docs/research/cnr.md`.
//!
//! Nothing here is a game-state machine; the authoritative gold object,
//! teams and scoring are F27-B. The tables exist so that slice (and the
//! lobby) draw their option lists and magnitudes from one sourced place
//! instead of inventing them.

use std::fmt;

use mm2_assets::Vfs;
use mm2_formats::spchdata::CueTable;
use mm2_formats::waypoints::WaypointFile;

/// The `race/<city>/` stem of the mode's record family: the original
/// builds `race\<city>\<stem>waypoints.csv` from its mode-name table
/// (`multicop` is entry 2, `0x5c46d8`; the executable's multiplayer
/// Cruise entry is `roam`). *verified_original* for the name.
pub const MODE_STEM: &str = "multicop";

/// Models the mode places in the world or on the map, looked up as
/// `geometry/<name>.pkg` and `tune/banger/<name>.dgbangerdata`.
/// *verified_original*: each name is a literal in `Midtown2.exe`
/// (`0x5c3440..0x5c347c`, `0x5c3f08`), the same strings the mode's setup
/// passes to its marker constructor.
pub const MARKER_MODELS: &[&str] = &["wpobj_gold", "pt_hideout", "pt_bank", "pt_red", "pt_blue"];

/// HUD/map dot textures (`texture/<name>.tga`) the mode's roles map to.
/// *documented*: the files ship and their names say what they mark, but
/// no code reference to them was traced — the binding of dot to role is
/// an inference, so a missing one is reported, not tolerated.
pub const MARKER_TEXTURES: &[&str] =
    &["gold_dot", "hideout_dot", "bank_dot", "red_dot", "blue_dot"];

/// The commentary cue families both stock `cnr<city>.csv` tables author,
/// in file order. *verified_original* as names (each table carries all
/// fourteen). Read as an event vocabulary: a role (`ROB`/`COP`) with
/// `GET`, `DROP`, `STASH` and `RECOVER`, and a team (`BLUE`/`RED`) with
/// `HAS`, `DROPPED` and `STASHED` — the trigger for each is not
/// recovered, only that the original announces these situations.
pub const COMMENTARY_CUES: &[&str] = &[
    "BLUETEAMHASGOLD",
    "REDTEAMHASGOLD",
    "BLUETEAMSTASHEDGOLD",
    "REDTEAMDROPPEDGOLD",
    "REDTEAMSTASHEDGOLD",
    "BLUETEAMDROPPEDGOLD",
    "ROBGETLOOT",
    "ROBDROPLOOT",
    "ROBSTASHLOOT",
    "ROBRECOVERLOOT",
    "COPGETLOOT",
    "COPDROPLOOT",
    "COPSTASHLOOT",
    "COPRECOVERLOOT",
];

/// The three variants the host chooses between, in the order the
/// executable's packed settings word numbers them (`word >> 6`,
/// `0x501510`). *Inferred* numbering: 0 scores individually and 2 builds
/// the red/blue marker pair (both read from code), so 1 is Cops vs.
/// Robbers by elimination against the three variants the help names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CnrVariant {
    /// Every player scores alone (`0x425db7` compares the player's own
    /// points against the limit).
    FreeForAll,
    /// Cops deliver to the bank, robbers to the hideout (help text;
    /// `0x42666c` picks the respawn marker by role).
    CopsVsRobbers,
    /// Two robber teams, red and blue, each with its own hideout marker
    /// (`pt_red`/`pt_blue`, `0x423e5a`); the match limit compares team
    /// totals.
    RobbersVsRobbers,
}

impl CnrVariant {
    /// Every variant, in the executable's numbering.
    pub const ALL: [CnrVariant; 3] = [
        CnrVariant::FreeForAll,
        CnrVariant::CopsVsRobbers,
        CnrVariant::RobbersVsRobbers,
    ];

    /// Whether the variant is scored by team totals rather than by
    /// individual points (*verified_original*: the limit check at
    /// `0x425dbc` branches on `variant != 0`).
    pub fn team_scored(self) -> bool {
        !matches!(self, CnrVariant::FreeForAll)
    }
}

/// Time-limit choices the host can pick, minutes. *verified_original*:
/// the table at `0x5d0550` and the `5 minutes … 30 minutes` strings in
/// `mmlang.dll` (ids 0x150–0x153) agree.
pub const TIME_LIMIT_MINUTES: [u32; 4] = [5, 10, 20, 30];

/// Point-limit choices the host can pick. *verified_original*: the
/// table at `0x5d0560` and `100 pts … 1,000 pts` (`mmlang.dll`
/// 0x154–0x157) agree.
pub const POINT_LIMITS: [u32; 4] = [100, 250, 500, 1000];

/// The match-limit option: none, a time limit or a point limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchLimit {
    /// The match runs until the host ends it.
    None,
    /// Ends after this many minutes.
    Minutes(u32),
    /// Ends when a player (or team) reaches this many points.
    Points(u32),
}

impl MatchLimit {
    /// Every selectable limit: `None`, then the time choices, then the
    /// point choices — the same three groups the host-settings screen
    /// offers.
    pub fn choices() -> Vec<MatchLimit> {
        let mut v = vec![MatchLimit::None];
        v.extend(TIME_LIMIT_MINUTES.iter().map(|&m| MatchLimit::Minutes(m)));
        v.extend(POINT_LIMITS.iter().map(|&p| MatchLimit::Points(p)));
        v
    }

    /// Whether the value is one the original offers. A mod may widen the
    /// list; the audit and the lobby still use this to say which values
    /// are stock.
    pub fn is_stock(self) -> bool {
        match self {
            MatchLimit::None => true,
            MatchLimit::Minutes(m) => TIME_LIMIT_MINUTES.contains(&m),
            MatchLimit::Points(p) => POINT_LIMITS.contains(&p),
        }
    }
}

/// Points a delivery to the hideout or bank scores. *verified_original*
/// as a call-site constant: both delivery paths call the score adder
/// with `0x64` (`0x425b30`, `0x425c98`). Whether a mod or the host can
/// change it is not known.
pub const DELIVERY_POINTS: u32 = 100;

/// Radius around a hideout/bank marker inside which a carrier delivers,
/// metres. The marker constructor receives `12.0` (`0x423f46`) and the
/// delivery test compares the carrier's squared distance with the
/// marker's radius field (`0x425a65`); the inside/outside direction of
/// that comparison was read from x87 flag tests and is *inferred*.
pub const DELIVERY_RADIUS_M: f32 = 12.0;

/// The host's gold-mass option. *verified_original* for the three
/// choices and their numbers; the strings are `Weightless`, `Quarter
/// Ton`, `Half Ton` (`mmlang.dll` 0x14c–0x14e).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GoldMass {
    /// No added mass.
    Weightless,
    /// "Quarter ton".
    QuarterTon,
    /// "Half ton".
    HalfTon,
}

impl GoldMass {
    /// Every option, lightest first.
    pub const ALL: [GoldMass; 3] = [
        GoldMass::Weightless,
        GoldMass::QuarterTon,
        GoldMass::HalfTon,
    ];

    /// Mass the original adds to the carrier's body, **in the
    /// executable's own units** (table `0x5d0570`: 0, 100, 200). The
    /// unit is not established: the help names ¼ and ½ ton, and 100:200
    /// matches that ratio, but nothing recovered says 100 units is
    /// 250 kg. Convert through [`GoldMass::added_mass_kg`], never by
    /// reading this as kilograms.
    pub fn engine_units(self) -> u32 {
        match self {
            GoldMass::Weightless => 0,
            GoldMass::QuarterTon => 100,
            GoldMass::HalfTon => 200,
        }
    }

    /// Added mass in kilograms by the *documented* reading (help text:
    /// weightless / ¼ ton / ½ ton, taking a ton as 1000 kg). A design
    /// reading of a documented label, not a measurement of the unit
    /// behind [`GoldMass::engine_units`].
    pub fn added_mass_kg(self) -> f32 {
        match self {
            GoldMass::Weightless => 0.0,
            GoldMass::QuarterTon => 250.0,
            GoldMass::HalfTon => 500.0,
        }
    }

    /// The scalar the original writes to the *local* carrier's
    /// `carsim+0x40c` on pickup (`0x424a61`, table `0x5c341c`: 1.0, 0.9,
    /// 0.81 for 0/100/200 units) and resets to 1.0 when the gold leaves
    /// (a negative mass delta). Read by the car's force code at
    /// `0x4d5c22`/`0x4d5d00`; what that force is was not identified, so
    /// the number is sourced and its physical meaning is open.
    pub fn handling_scalar(self) -> f32 {
        match self {
            GoldMass::Weightless => 1.0,
            GoldMass::QuarterTon => 0.9,
            GoldMass::HalfTon => 0.81,
        }
    }
}

/// One file the mode depends on and whether the VFS resolved it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CnrDependency {
    /// Logical path.
    pub logical: String,
    /// What the file is for.
    pub role: &'static str,
    /// Whether it resolved through the VFS.
    pub found: bool,
}

/// Why a city's Cops & Robbers content is not usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CnrIssue {
    /// A required file did not resolve.
    Missing(String),
    /// The placement CSV exists but could not be read or parsed.
    Unreadable(String),
    /// Fewer waypoints than the three distinct sites (gold, hideout,
    /// bank) a round needs. The original falls back to arbitrary world
    /// positions below three rows (`0x4248c4`), which this loader does
    /// not reproduce.
    TooFewSites {
        /// Rows found.
        found: usize,
    },
    /// The commentary table does not parse.
    CommentaryUnreadable(String),
    /// A commentary cue family the stock tables author is absent.
    MissingCue(&'static str),
    /// A commentary cue family exists but has no cue rows.
    EmptyCue(&'static str),
    /// A waypoint carried a non-finite coordinate.
    NonFiniteSite {
        /// 1-based source line.
        line: u32,
    },
}

impl fmt::Display for CnrIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CnrIssue::Missing(p) => write!(f, "missing {p}"),
            CnrIssue::Unreadable(why) => write!(f, "placement file unreadable: {why}"),
            CnrIssue::TooFewSites { found } => {
                write!(f, "only {found} placement waypoints, a round needs 3")
            }
            CnrIssue::CommentaryUnreadable(why) => {
                write!(f, "commentary table unreadable: {why}")
            }
            CnrIssue::MissingCue(c) => write!(f, "commentary cue {c} absent"),
            CnrIssue::EmptyCue(c) => write!(f, "commentary cue {c} has no rows"),
            CnrIssue::NonFiniteSite { line } => {
                write!(f, "non-finite placement waypoint on line {line}")
            }
        }
    }
}

/// A city's Cops & Robbers data, resolved through the VFS.
#[derive(Clone, Debug)]
pub struct CnrContent {
    /// City stem (`sf`, `london`).
    pub city: String,
    /// The authored pool gold/hideout/bank positions are drawn from, in
    /// file order (MM2 world axes, unmirrored, as authored).
    pub sites: Vec<[f32; 3]>,
    /// Every dependency checked, found or not.
    pub dependencies: Vec<CnrDependency>,
    /// Commentary cue families found with at least one row, out of
    /// [`COMMENTARY_CUES`].
    pub cues_present: usize,
    /// Problems; empty means the city's mode content is complete.
    pub issues: Vec<CnrIssue>,
}

impl CnrContent {
    /// Logical path of the placement pool for `city`.
    pub fn placement_path(city: &str) -> String {
        format!("race/{city}/{MODE_STEM}waypoints.csv")
    }

    /// Logical path of the per-city commentary table (`cnrsf`,
    /// `cnrlondon`; the executable names `CNRLONDON`, `0x5d2c68`).
    pub fn commentary_path(city: &str) -> String {
        format!("aud/spchdata/cnr{city}.csv")
    }

    /// Every logical path the mode depends on for `city`, with its role.
    pub fn dependency_list(city: &str) -> Vec<(String, &'static str)> {
        let mut v = vec![
            (Self::placement_path(city), "gold/hideout/bank site pool"),
            (Self::commentary_path(city), "commentary cue table"),
            (format!("jpg/{city}_{MODE_STEM}.jpg"), "mode loading image"),
        ];
        for m in MARKER_MODELS {
            v.push((format!("geometry/{m}.pkg"), "marker model"));
            v.push((
                format!("tune/banger/{m}.dgbangerdata"),
                "marker banger record",
            ));
        }
        for t in MARKER_TEXTURES {
            v.push((format!("texture/{t}.tga"), "map/HUD dot"));
        }
        v
    }

    /// Resolve and read everything the mode needs for `city`. Never
    /// fails outright: a missing or broken piece is an entry in
    /// [`CnrContent::issues`], so an audit counts it instead of losing
    /// it.
    pub fn load(vfs: &Vfs, city: &str) -> Self {
        let mut out = CnrContent {
            city: city.to_string(),
            sites: Vec::new(),
            dependencies: Vec::new(),
            cues_present: 0,
            issues: Vec::new(),
        };
        out.check_commentary(vfs);
        for (logical, role) in Self::dependency_list(city) {
            let found = vfs.resolve(&logical).is_some();
            if !found {
                out.issues.push(CnrIssue::Missing(logical.clone()));
            }
            out.dependencies.push(CnrDependency {
                logical,
                role,
                found,
            });
        }
        let path = Self::placement_path(city);
        if vfs.resolve(&path).is_none() {
            return out;
        }
        let text = match vfs.read_logical(&path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => {
                out.issues.push(CnrIssue::Unreadable(e.to_string()));
                return out;
            }
        };
        match WaypointFile::parse(&text) {
            Ok(file) => {
                for row in &file.rows {
                    if row.position.iter().all(|c| c.is_finite()) {
                        out.sites.push(row.position);
                    } else {
                        out.issues.push(CnrIssue::NonFiniteSite { line: row.line });
                    }
                }
                for d in &file.diagnostics {
                    out.issues.push(CnrIssue::Unreadable(format!(
                        "line {}: {}",
                        d.line, d.message
                    )));
                }
                if out.sites.len() < 3 {
                    out.issues.push(CnrIssue::TooFewSites {
                        found: out.sites.len(),
                    });
                }
            }
            Err(e) => out.issues.push(CnrIssue::Unreadable(e.to_string())),
        }
        out
    }

    /// Parse the commentary table and count the stock cue families. A
    /// missing file is reported by the dependency pass, not twice.
    fn check_commentary(&mut self, vfs: &Vfs) {
        let path = Self::commentary_path(&self.city);
        if vfs.resolve(&path).is_none() {
            return;
        }
        let text = match vfs.read_logical(&path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => {
                self.issues
                    .push(CnrIssue::CommentaryUnreadable(e.to_string()));
                return;
            }
        };
        let table = match CueTable::parse(&text) {
            Ok(t) => t,
            Err(e) => {
                self.issues
                    .push(CnrIssue::CommentaryUnreadable(e.to_string()));
                return;
            }
        };
        for &cue in COMMENTARY_CUES {
            match table.section(cue) {
                None => self.issues.push(CnrIssue::MissingCue(cue)),
                Some(s) if s.rows.is_empty() => self.issues.push(CnrIssue::EmptyCue(cue)),
                Some(_) => self.cues_present += 1,
            }
        }
    }

    /// Whether every dependency resolved and the pool can seed a round.
    pub fn is_complete(&self) -> bool {
        self.issues.is_empty()
    }

    /// Number of dependencies that resolved.
    pub fn found_count(&self) -> usize {
        self.dependencies.iter().filter(|d| d.found).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    const HEADER: &str = "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n";

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// A synthetic install carrying every dependency except as told.
    fn install(city: &str, rows: &str, skip: &[&str]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (logical, _) in CnrContent::dependency_list(city) {
            if skip.contains(&logical.as_str()) {
                continue;
            }
            let body = if logical == CnrContent::placement_path(city) {
                format!("{HEADER}{rows}")
            } else if logical == CnrContent::commentary_path(city) {
                commentary(COMMENTARY_CUES)
            } else {
                "x".to_string()
            };
            write(d.path(), &logical, &body);
        }
        d
    }

    /// A cue table with one authored row under each named family.
    fn commentary(cues: &[&str]) -> String {
        let mut t = String::from("Name prefix/type header,end sufix value,sufix add value\n");
        for c in cues {
            t.push_str(&format!("{c} header,,\nAL1\\{c},1,0\n"));
        }
        t
    }

    fn vfs_of(dir: &Path) -> Vfs {
        let mut vfs = Vfs::new();
        vfs.mount_dir(dir, 0).unwrap();
        vfs
    }

    const THREE: &str = "1,2,3,10,0,0,0,0,\n4,5,6,20,0,0,0,0,\n7,8,9,30,0,0,0,0,\n";

    #[test]
    fn a_complete_city_loads_its_pool_in_file_order() {
        let d = install("sf", THREE, &[]);
        let c = CnrContent::load(&vfs_of(d.path()), "sf");
        assert!(c.is_complete(), "{:?}", c.issues);
        assert_eq!(c.sites, vec![[1., 2., 3.], [4., 5., 6.], [7., 8., 9.]]);
        assert_eq!(c.found_count(), c.dependencies.len());
        assert_eq!(c.cues_present, COMMENTARY_CUES.len());
    }

    #[test]
    fn a_cue_family_the_stock_tables_author_must_be_present_and_non_empty() {
        let d = install("sf", THREE, &[]);
        let path = CnrContent::commentary_path("sf");
        // Drop ROBRECOVERLOOT entirely and leave COPGETLOOT without rows.
        let mut body = commentary(
            &COMMENTARY_CUES
                .iter()
                .copied()
                .filter(|c| !matches!(*c, "ROBRECOVERLOOT" | "COPGETLOOT"))
                .collect::<Vec<_>>(),
        );
        body.push_str("COPGETLOOT header,,\n");
        write(d.path(), &path, &body);
        let c = CnrContent::load(&vfs_of(d.path()), "sf");
        assert_eq!(c.cues_present, COMMENTARY_CUES.len() - 2);
        assert!(c.issues.contains(&CnrIssue::MissingCue("ROBRECOVERLOOT")));
        assert!(c.issues.contains(&CnrIssue::EmptyCue("COPGETLOOT")));
    }

    #[test]
    fn an_empty_commentary_table_is_unreadable_not_silently_complete() {
        let d = install("sf", THREE, &[]);
        write(d.path(), &CnrContent::commentary_path("sf"), "");
        let c = CnrContent::load(&vfs_of(d.path()), "sf");
        assert!(
            c.issues
                .iter()
                .any(|i| matches!(i, CnrIssue::CommentaryUnreadable(_))),
            "{:?}",
            c.issues
        );
        assert_eq!(c.cues_present, 0);
    }

    #[test]
    fn every_dependency_is_counted_even_when_absent() {
        let missing = "geometry/pt_bank.pkg";
        let d = install("sf", THREE, &[missing]);
        let c = CnrContent::load(&vfs_of(d.path()), "sf");
        assert_eq!(
            c.dependencies.len(),
            CnrContent::dependency_list("sf").len()
        );
        assert_eq!(c.found_count(), c.dependencies.len() - 1);
        assert_eq!(c.issues, vec![CnrIssue::Missing(missing.to_string())]);
        assert!(!c.is_complete());
        // The pool still loads: one missing model does not hide it.
        assert_eq!(c.sites.len(), 3);
    }

    #[test]
    fn a_missing_pool_is_a_missing_dependency_with_no_sites() {
        let p = CnrContent::placement_path("london");
        let d = install("london", THREE, &[&p]);
        let c = CnrContent::load(&vfs_of(d.path()), "london");
        assert!(c.sites.is_empty());
        assert!(c.issues.contains(&CnrIssue::Missing(p)));
    }

    #[test]
    fn fewer_than_three_sites_cannot_seed_a_round() {
        let d = install("sf", "1,2,3,10,0,0,0,0,\n4,5,6,20,0,0,0,0,\n", &[]);
        let c = CnrContent::load(&vfs_of(d.path()), "sf");
        assert_eq!(c.issues, vec![CnrIssue::TooFewSites { found: 2 }]);
    }

    #[test]
    fn a_non_finite_site_is_dropped_and_reported() {
        let rows = format!("NaN,2,3,10,0,0,0,0,\n{THREE}");
        let d = install("sf", &rows, &[]);
        let c = CnrContent::load(&vfs_of(d.path()), "sf");
        assert_eq!(c.sites.len(), 3);
        assert!(
            c.issues
                .iter()
                .any(|i| matches!(i, CnrIssue::NonFiniteSite { line: 2 })),
            "{:?}",
            c.issues
        );
    }

    #[test]
    fn a_pool_without_the_waypoint_header_is_unreadable() {
        let d = tempfile::tempdir().unwrap();
        for (logical, _) in CnrContent::dependency_list("sf") {
            write(d.path(), &logical, "1,2,3,4,0,0,0,0,\n");
        }
        let c = CnrContent::load(&vfs_of(d.path()), "sf");
        assert!(
            c.issues
                .iter()
                .any(|i| matches!(i, CnrIssue::Unreadable(_))),
            "{:?}",
            c.issues
        );
        assert!(c.sites.is_empty());
    }

    #[test]
    fn an_empty_install_reports_every_dependency_as_missing() {
        let d = tempfile::tempdir().unwrap();
        let c = CnrContent::load(&vfs_of(d.path()), "sf");
        let n = CnrContent::dependency_list("sf").len();
        assert_eq!(c.found_count(), 0);
        assert_eq!(c.issues.len(), n);
    }

    #[test]
    fn option_tables_match_the_recovered_values() {
        assert_eq!(TIME_LIMIT_MINUTES, [5, 10, 20, 30]);
        assert_eq!(POINT_LIMITS, [100, 250, 500, 1000]);
        let units: Vec<u32> = GoldMass::ALL.iter().map(|g| g.engine_units()).collect();
        assert_eq!(units, vec![0, 100, 200]);
        let scalars: Vec<f32> = GoldMass::ALL.iter().map(|g| g.handling_scalar()).collect();
        assert_eq!(scalars, vec![1.0, 0.9, 0.81]);
        // 0.9 per 100 units, compounding.
        assert!((0.9f32 * 0.9 - GoldMass::HalfTon.handling_scalar()).abs() < 1e-6);
    }

    #[test]
    fn documented_kilograms_keep_the_engine_ratio() {
        let q = GoldMass::QuarterTon;
        let h = GoldMass::HalfTon;
        assert_eq!(h.engine_units(), 2 * q.engine_units());
        assert_eq!(h.added_mass_kg(), 2.0 * q.added_mass_kg());
        assert_eq!(GoldMass::Weightless.added_mass_kg(), 0.0);
    }

    #[test]
    fn match_limit_choices_are_none_plus_both_groups_and_all_stock() {
        let c = MatchLimit::choices();
        assert_eq!(c.len(), 1 + TIME_LIMIT_MINUTES.len() + POINT_LIMITS.len());
        assert!(c.iter().all(|l| l.is_stock()));
        assert!(!MatchLimit::Minutes(7).is_stock());
        assert!(!MatchLimit::Points(300).is_stock());
    }

    #[test]
    fn only_free_for_all_is_scored_individually() {
        let team: Vec<bool> = CnrVariant::ALL.iter().map(|v| v.team_scored()).collect();
        assert_eq!(team, vec![false, true, true]);
    }

    #[test]
    fn paths_follow_the_executables_format() {
        assert_eq!(
            CnrContent::placement_path("sf"),
            "race/sf/multicopwaypoints.csv"
        );
        assert_eq!(
            CnrContent::commentary_path("london"),
            "aud/spchdata/cnrlondon.csv"
        );
    }
}
