//! The Cops & Robbers host options and rule magnitudes (F27-A/B).
//!
//! The option tables and code-defined constants the original packs into
//! its host-settings word, kept in `mm2_game` — beside [`GoldRules`] they
//! feed — so a session's configuration (`SessionMode::CopsAndRobbers`)
//! can carry the lobby's choices without `mm2_game` depending on content
//! parsing. `mm2_content::cnr` re-exports everything here; the sources
//! (executable addresses, `mmlang.dll` string ids) are in
//! `docs/research/cnr.md`.

use crate::gold::{CarrierLoad, CnrVariant, EndRule, GoldRules};

/// Time-limit choices the host can pick, minutes. *verified_original*:
/// the table at `0x5d0550` and the `5 minutes … 30 minutes` strings in
/// `mmlang.dll` (ids 0x150–0x153) agree.
pub const TIME_LIMIT_MINUTES: [u32; 4] = [5, 10, 20, 30];

/// Point-limit choices the host can pick. *verified_original*: the
/// table at `0x5d0560` and `100 pts … 1,000 pts` (`mmlang.dll`
/// 0x154–0x157) agree.
pub const POINT_LIMITS: [u32; 4] = [100, 250, 500, 1000];

/// Upper bound on a time limit a session config accepts, minutes. The
/// host menu offers [`TIME_LIMIT_MINUTES`]; the bound only keeps a wire
/// blob from naming a limit the tick clock cannot represent
/// (*implementation choice*).
pub const MAX_LIMIT_MINUTES: u32 = 24 * 60;

/// Upper bound on a point limit a session config accepts
/// (*implementation choice*, as [`MAX_LIMIT_MINUTES`]).
pub const MAX_LIMIT_POINTS: u32 = 1_000_000;

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

/// Points the original awards in the local-pickup handler (message
/// `0x25a`, `0x4266a0`). *verified_original* as a call-site constant;
/// whether it applies to the first pickup, to every pickup or only to a
/// steal is unknown, so the match applies it to every grant — an
/// *Implementation choice* that [`GoldRules::pickup_points`] lets a
/// caller zero.
pub const PICKUP_POINTS: u32 = 25;

/// Distance within which a car takes the gold, metres. *Enhanced
/// policy*: the gold marker's constructor receives `5.0` (`0x423d12`),
/// but its use as a pickup radius was not read.
pub const PICKUP_RADIUS_M: f32 = 5.0;

/// Seconds the car that lost the gold cannot retake it. *Enhanced
/// policy*; nothing of the original's behaviour is recovered.
pub const DROP_LOCKOUT_SECONDS: f32 = 1.0;

/// The host's Cops & Robbers choices: the three fields the original
/// packs into one settings word (`0x5014c1`, CNR-7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CnrSettings {
    /// Which variant is played.
    pub variant: CnrVariant,
    /// How heavy the gold is.
    pub gold_mass: GoldMass,
    /// When the match ends.
    pub limit: MatchLimit,
}

impl Default for CnrSettings {
    /// The executable's defaults (`0x523484…0x5234a0`): variant 0, no
    /// limit, weightless gold.
    fn default() -> Self {
        Self {
            variant: CnrVariant::FreeForAll,
            gold_mass: GoldMass::Weightless,
            limit: MatchLimit::None,
        }
    }
}

impl CnrSettings {
    /// The rules a match plays by under these settings, with the match
    /// clock running at `tick_hz` ticks per second. Every number comes
    /// from the tables above, so a lobby choice and the rules the host
    /// enforces cannot drift apart.
    pub fn rules(&self, tick_hz: u32) -> GoldRules {
        let hz = f64::from(tick_hz);
        let ticks = |seconds: f64| (seconds * hz).round() as u64;
        GoldRules {
            variant: self.variant,
            end: match self.limit {
                MatchLimit::None => EndRule::None,
                MatchLimit::Minutes(m) => EndRule::Ticks(ticks(f64::from(m) * 60.0)),
                MatchLimit::Points(p) => EndRule::Points(p),
            },
            load: CarrierLoad {
                added_mass_kg: self.gold_mass.added_mass_kg(),
                handling_scalar: self.gold_mass.handling_scalar(),
            },
            pickup_points: PICKUP_POINTS,
            delivery_points: DELIVERY_POINTS,
            pickup_radius: PICKUP_RADIUS_M,
            delivery_radius: DELIVERY_RADIUS_M,
            drop_lockout_ticks: ticks(f64::from(DROP_LOCKOUT_SECONDS)),
        }
    }
}

/// The names a command line (or any text surface) uses for the host's
/// Cops & Robbers choices. *Implementation choice*: the original picks
/// these from menus, so the spellings are ours; the choices they select
/// are the stock tables above and nothing outside them parses.
impl CnrVariant {
    /// The accepted spellings, for error messages.
    pub const NAMES: &'static str = "ffa|cops|robbers";

    /// The variant a command-line name selects: `ffa` (free for all),
    /// `cops` (Cops vs. Robbers) or `robbers` (Robbers vs. Robbers).
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "ffa" => Some(CnrVariant::FreeForAll),
            "cops" => Some(CnrVariant::CopsVsRobbers),
            "robbers" => Some(CnrVariant::RobbersVsRobbers),
            _ => None,
        }
    }
}

impl GoldMass {
    /// The accepted spellings, for error messages.
    pub const NAMES: &'static str = "weightless|quarter|half";

    /// The option a command-line name selects (`quarter` is the Quarter
    /// Ton choice, `half` the Half Ton one).
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "weightless" => Some(GoldMass::Weightless),
            "quarter" => Some(GoldMass::QuarterTon),
            "half" => Some(GoldMass::HalfTon),
            _ => None,
        }
    }
}

impl MatchLimit {
    /// The accepted spellings, for error messages.
    pub const NAMES: &'static str =
        "none|<minutes>m|<points>pts (stock: 5m 10m 20m 30m, 100pts 250pts 500pts 1000pts)";

    /// The limit a command-line name selects: `none`, `<n>m` for a time
    /// limit or `<n>pts` for a point limit. Only the values the host
    /// menu offers ([`MatchLimit::is_stock`]) parse — a limit outside
    /// the tables is a typo here, not a custom match.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        let limit = if name == "none" {
            MatchLimit::None
        } else if let Some(m) = name.strip_suffix("pts") {
            MatchLimit::Points(m.parse().ok()?)
        } else {
            MatchLimit::Minutes(name.strip_suffix('m')?.parse().ok()?)
        };
        limit.is_stock().then_some(limit)
    }
}

impl CnrSettings {
    /// Settings from the three command-line choices. An absent gold or
    /// limit keeps the executable's default ([`CnrSettings::default`]);
    /// an unrecognised one is an error naming what would have parsed.
    pub fn parse(variant: &str, gold: Option<&str>, limit: Option<&str>) -> Result<Self, String> {
        let mut settings = CnrSettings {
            variant: CnrVariant::parse(variant).ok_or_else(|| {
                format!(
                    "unknown variant {variant:?}: expected {}",
                    CnrVariant::NAMES
                )
            })?,
            ..CnrSettings::default()
        };
        if let Some(g) = gold {
            settings.gold_mass = GoldMass::parse(g)
                .ok_or_else(|| format!("unknown gold mass {g:?}: expected {}", GoldMass::NAMES))?;
        }
        if let Some(l) = limit {
            settings.limit = MatchLimit::parse(l).ok_or_else(|| {
                format!("unknown match limit {l:?}: expected {}", MatchLimit::NAMES)
            })?;
        }
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_mass_and_stock_limit_has_a_name_that_parses() {
        for (name, v) in [
            ("ffa", CnrVariant::FreeForAll),
            ("COPS", CnrVariant::CopsVsRobbers),
            ("robbers", CnrVariant::RobbersVsRobbers),
        ] {
            assert_eq!(CnrVariant::parse(name), Some(v));
        }
        assert_eq!(CnrVariant::ALL.len(), 3, "a new variant needs a name");
        for (name, m) in [
            ("weightless", GoldMass::Weightless),
            ("quarter", GoldMass::QuarterTon),
            ("half", GoldMass::HalfTon),
        ] {
            assert_eq!(GoldMass::parse(name), Some(m));
        }
        assert_eq!(GoldMass::ALL.len(), 3, "a new mass needs a name");
        assert_eq!(MatchLimit::parse("none"), Some(MatchLimit::None));
        for m in TIME_LIMIT_MINUTES {
            assert_eq!(
                MatchLimit::parse(&format!("{m}m")),
                Some(MatchLimit::Minutes(m))
            );
        }
        for p in POINT_LIMITS {
            assert_eq!(
                MatchLimit::parse(&format!("{p}pts")),
                Some(MatchLimit::Points(p))
            );
        }
        assert_eq!(MatchLimit::choices().len(), 9, "a new limit needs a name");
    }

    #[test]
    fn off_table_or_malformed_names_do_not_parse() {
        for bad in [
            "", "7m", "0m", "99pts", "m", "pts", "-5m", "5", "5min", "1e2pts", " 5m",
        ] {
            assert_eq!(MatchLimit::parse(bad), None, "{bad:?}");
        }
        assert_eq!(CnrVariant::parse("teams"), None);
        assert_eq!(CnrVariant::parse(""), None);
        assert_eq!(GoldMass::parse("ton"), None);
    }

    #[test]
    fn settings_default_what_is_not_named_and_name_what_is_wrong() {
        assert_eq!(
            CnrSettings::parse("ffa", None, None).unwrap(),
            CnrSettings::default()
        );
        let s = CnrSettings::parse("cops", Some("half"), Some("250pts")).unwrap();
        assert_eq!(s.variant, CnrVariant::CopsVsRobbers);
        assert_eq!(s.gold_mass, GoldMass::HalfTon);
        assert_eq!(s.limit, MatchLimit::Points(250));
        let e = CnrSettings::parse("tag", None, None).unwrap_err();
        assert!(e.contains("tag") && e.contains(CnrVariant::NAMES), "{e}");
        let e = CnrSettings::parse("ffa", Some("heavy"), None).unwrap_err();
        assert!(e.contains("heavy") && e.contains("quarter"), "{e}");
        let e = CnrSettings::parse("ffa", None, Some("7m")).unwrap_err();
        assert!(e.contains("7m") && e.contains("10m"), "{e}");
    }
}
