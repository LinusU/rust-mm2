//! `banger-bind` audit (F04-A.2): which placement sources stamp props
//! that carry authored `tune/banger/*.dgbangerdata` records.
//!
//! The measured binding rule: a placed asset name `N` is banger-bound
//! when `tune/banger/<N>.dgbangerdata` resolves. Sources audited, with
//! no filename filtering:
//!
//! - `city/**/*.inst` — INST placements (static architecture on
//!   retail: facades/monuments, zero bound names)
//! - `city/**/*.pathset` and `race/<city>/*.pathset` — stamped prop
//!   rows (the banger channel: ~every prop name bound on retail)
//! - `city/<dir>/propdefs.csv` + `proprules.csv` + `city/<dir>.psdl`
//!   `prop_rule` bytes — rule-driven stamping candidates
//! - `city/**/*.csv` group/prototype tables — `propdefs*`/`proprules*`/
//!   `props*` names
//!
//! The reverse check (unfiltered audit only) lists every banger
//! record whose owning PKG
//! ([`mm2_formats::banger::geometry_owner`]) is never named by an
//! audited placement — `vp*`/`va*` owners bind through the vehicle
//! pipeline (`AddBangerDataEntry(name, partName)` per mm2hook), not
//! world placement.
//!
//! Issues are placed names that resolve to no `geometry/*.pkg` at all
//! (dead placement refs). Grammar-internal validation already
//! reported by the dedicated `pathset`/`proprules` audits is not
//! re-counted. `--strict` exits nonzero on failures or issues.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mm2_assets::Vfs;
use mm2_formats::banger::geometry_owner;
use mm2_formats::inst;
use mm2_formats::pathset::Pathset;
use mm2_formats::proprules::{PropDefs, PropGroups, PropRules};
use mm2_formats::psdl::Psdl;

use crate::TEXTURE_EXTS;

/// Which placement-source grammar a file carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// `INST` binary placement records.
    Inst,
    /// `PTH1` pathset; prop names come from `Path::asset_name()`.
    Pathset,
    /// `propdefs*.csv` — prototype rows whose `file*` cells name PKGs.
    PropDefs,
    /// `proprules*.csv` — rule rows naming propdefs.
    PropRules,
    /// `props*.csv` — group→prop membership.
    PropGroups,
}

/// One discovered (or expected-but-missing) placement source file.
#[derive(Debug)]
struct SourceFile {
    logical: String,
    /// City bucket this file dresses (`london`, `sf`, `phys`,
    /// `sf/bak`, a root-file stem like `london_ai` or `race0`, …).
    bucket: String,
    kind: SourceKind,
    /// `expected` = part of a stock city's authored set, `overlay` =
    /// `race/<city>/` event overlay, `extra` = anything else.
    tag: &'static str,
}

/// Map a logical path to its placement-source role, or `None` when it
/// is not a placement file (`geometry/props.csv` is the LOD table, not
/// a placement source; `tune/` records are the *target* of binding).
fn classify_source(logical: &str) -> Option<(String, SourceKind)> {
    if let Some(rest) = logical.strip_prefix("city/") {
        if rest.ends_with(".inst") {
            let bucket = match rest.rsplit_once('/') {
                Some((dir, _)) => dir.to_string(),
                None => rest.trim_end_matches(".inst").to_string(),
            };
            return Some((bucket, SourceKind::Inst));
        }
        if rest.ends_with(".pathset") {
            let bucket = match rest.rsplit_once('/') {
                Some((dir, _)) => dir.to_string(),
                None => rest.trim_end_matches(".pathset").to_string(),
            };
            return Some((bucket, SourceKind::Pathset));
        }
        if rest.ends_with(".csv") || rest.ends_with(".csv.txt") {
            let (dir, name) = match rest.rsplit_once('/') {
                Some((d, n)) => (d.to_string(), n),
                // Root-level `city/*.csv` — no city dir to affiliate;
                // bucket by its own stem as an unaffiliated extra.
                None => (
                    rest.trim_end_matches(".csv.txt")
                        .trim_end_matches(".csv")
                        .to_string(),
                    rest,
                ),
            };
            let kind = if name.starts_with("propdefs") {
                SourceKind::PropDefs
            } else if name.starts_with("proprules") {
                SourceKind::PropRules
            } else if name.starts_with("props") {
                SourceKind::PropGroups
            } else {
                return None;
            };
            return Some((dir, kind));
        }
        return None;
    }
    if let Some(rest) = logical.strip_prefix("race/")
        && rest.ends_with(".pathset")
        && let Some((dir, _)) = rest.rsplit_once('/')
    {
        return Some((dir.to_string(), SourceKind::Pathset));
    }
    None
}

/// A placed asset name's binding outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// `tune/banger/<name>.dgbangerdata` resolves.
    Bound,
    /// No record, but `geometry/<name>.pkg` resolves — a placed prop
    /// with no authored banger behavior.
    Unbound,
    /// Neither resolves — a dead placement reference (issue).
    Dead,
}

fn bind_name(name: &str, stems: &BTreeSet<String>, vfs: &Vfs) -> Binding {
    if stems.contains(name) {
        Binding::Bound
    } else if vfs.resolve(&format!("geometry/{name}.pkg")).is_some() {
        Binding::Unbound
    } else {
        Binding::Dead
    }
}

/// Per-file binding tally plus the placed names it contributed.
#[derive(Debug, Default)]
struct Tally {
    bound: BTreeSet<String>,
    unbound: BTreeSet<String>,
    dead: BTreeSet<String>,
}

impl Tally {
    fn add(&mut self, name: &str, binding: Binding) {
        match binding {
            Binding::Bound => self.bound.insert(name.to_string()),
            Binding::Unbound => self.unbound.insert(name.to_string()),
            Binding::Dead => self.dead.insert(name.to_string()),
        };
    }

    fn names(&self) -> usize {
        self.bound.len() + self.unbound.len() + self.dead.len()
    }

    fn detail(&self) -> String {
        format!(
            "{} bound / {} unbound / {} dead",
            self.bound.len(),
            self.unbound.len(),
            self.dead.len(),
        )
    }

    fn print_dead(&self) {
        if self.dead.is_empty() {
            return;
        }
        let shown: Vec<&str> = self.dead.iter().take(10).map(String::as_str).collect();
        println!(
            "    dead: {}{}",
            shown.join(", "),
            if self.dead.len() > 10 {
                format!(" (+{} more)", self.dead.len() - 10)
            } else {
                String::new()
            }
        );
    }
}

pub fn banger_bind(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = crate::build_vfs(dir, mods)?;
    let stem_filter = city.map(|c| c.to_ascii_lowercase());

    // Banger record stems — the binding target.
    let stems: BTreeSet<String> = vfs
        .list()
        .into_iter()
        .filter_map(|p| {
            p.strip_prefix("tune/banger/")
                .and_then(|s| s.strip_suffix(".dgbangerdata"))
                .map(|s| s.to_string())
        })
        .collect();

    // Each stock city's authored placement set is expected; every
    // other discovered source is an audited extra.
    let stock: Vec<String> = match &stem_filter {
        Some(c) => vec![c.clone()],
        None => mm2_content::EXPECTED_CITIES
            .iter()
            .map(|c| c.to_string())
            .collect(),
    };
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for c in &stock {
        for f in [
            format!("city/{c}.inst"),
            format!("city/{c}/props.pathset"),
            format!("city/{c}/propdefs.csv"),
            format!("city/{c}/proprules.csv"),
            format!("city/{c}/props.csv"),
        ] {
            expected.insert(f);
        }
    }

    let mut files: Vec<SourceFile> = Vec::new();
    for p in vfs.list() {
        let Some((bucket, kind)) = classify_source(&p) else {
            continue;
        };
        if stem_filter.as_ref().is_some_and(|c| &bucket != c) {
            continue;
        }
        let tag = if expected.contains(&p) {
            "expected"
        } else if p.starts_with("race/") {
            "overlay"
        } else {
            "extra"
        };
        files.push(SourceFile {
            logical: p,
            bucket,
            kind,
            tag,
        });
    }
    for e in &expected {
        if files.iter().all(|f| &f.logical != e)
            && let Some((bucket, kind)) = classify_source(e)
        {
            files.push(SourceFile {
                logical: e.clone(),
                bucket,
                kind,
                tag: "expected",
            });
        }
    }
    files.sort_by(|a, b| a.bucket.cmp(&b.bucket).then(a.logical.cmp(&b.logical)));

    let mut failures: Vec<String> = Vec::new();
    let mut issues: Vec<String> = Vec::new();
    let mut unsupported = 0usize;
    // bucket → union of placed prop names (for the reverse check).
    let mut placed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // bucket → parsed defs/rules for the PSDL reachability pass.
    let mut defs_by_bucket: BTreeMap<String, Vec<(String, PropDefs)>> = BTreeMap::new();
    let mut rules_by_bucket: BTreeMap<String, Vec<(String, PropRules)>> = BTreeMap::new();

    println!("== banger placement binding ==");
    println!("  binding rule: placed name N -> tune/banger/<N>.dgbangerdata");
    // Cross-check inputs that are not themselves placement files.
    for c in &stock {
        let present = vfs.resolve(&format!("city/{c}.psdl")).is_some();
        if !present {
            println!("    note: no city/{c}.psdl — prop_rule usage unchecked");
        }
    }
    if vfs.resolve("tune/banger/default.dgbangerdata").is_none() {
        println!("    note: tune/banger/default.dgbangerdata absent (fallback record)");
    }

    let mut cur_bucket = String::new();
    for f in &files {
        if f.bucket != cur_bucket {
            cur_bucket = f.bucket.clone();
            println!("bucket {cur_bucket}");
        }
        let Some(res) = vfs.resolve(&f.logical) else {
            println!("  {:<56} {:<9} missing", f.logical, f.tag);
            failures.push(format!("{}: expected file not found", f.logical));
            continue;
        };
        let bytes = vfs.read(&res)?;
        match f.kind {
            SourceKind::Inst => {
                let mut tally = Tally::default();
                match inst::parse(&bytes) {
                    Ok(comps) => {
                        let mut mods_hist: BTreeMap<u16, usize> = BTreeMap::new();
                        let mut flags: BTreeMap<u16, usize> = BTreeMap::new();
                        for c in &comps {
                            let name = c.package_name.to_ascii_lowercase();
                            *mods_hist.entry(c.modifiers).or_default() += 1;
                            for bit in [0x0100u16, 0x0200, 0x0400, 0x2000] {
                                if c.modifiers & bit != 0 {
                                    *flags.entry(bit).or_default() += 1;
                                }
                            }
                            tally.add(&name, bind_name(&name, &stems, &vfs));
                            placed.entry(f.bucket.clone()).or_default().insert(name);
                        }
                        println!(
                            "  {:<56} {:<9} ok — {} placements, {} names: {}",
                            f.logical,
                            f.tag,
                            comps.len(),
                            tally.names(),
                            tally.detail(),
                        );
                        let mut hist: Vec<(u16, usize)> = mods_hist.into_iter().collect();
                        hist.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                        let top: Vec<String> = hist
                            .iter()
                            .take(8)
                            .map(|(m, c)| format!("0x{m:04x}x{c}"))
                            .collect();
                        let flag_detail: Vec<String> = flags
                            .iter()
                            .map(|(b, c)| format!("0x{b:04x}={c}"))
                            .collect();
                        println!(
                            "    modifiers: {}{}; flag bits: {}",
                            top.join(" "),
                            if hist.len() > 8 {
                                format!(" (+{} more)", hist.len() - 8)
                            } else {
                                String::new()
                            },
                            if flag_detail.is_empty() {
                                "none".to_string()
                            } else {
                                flag_detail.join(" ")
                            },
                        );
                        report_dead(&f.logical, &tally, &mut issues);
                    }
                    Err(e) => source_fail(f, &e.to_string(), &mut failures, &mut unsupported),
                }
            }
            SourceKind::Pathset => {
                let mut tally = Tally::default();
                match Pathset::parse(&bytes) {
                    Ok(ps) => {
                        let (mut decal, mut label, mut animated, mut unresolved) =
                            (0usize, 0usize, 0usize, 0usize);
                        for path in &ps.paths {
                            let Some(name) = path.asset_name() else {
                                label += 1;
                                continue;
                            };
                            let name = name.to_ascii_lowercase();
                            if name.starts_with("giz_") {
                                animated += 1;
                                tally.add(&name, bind_name(&name, &stems, &vfs));
                                placed.entry(f.bucket.clone()).or_default().insert(name);
                            } else if vfs.resolve(&format!("geometry/{name}.pkg")).is_some() {
                                tally.add(&name, bind_name(&name, &stems, &vfs));
                                placed.entry(f.bucket.clone()).or_default().insert(name);
                            } else if TEXTURE_EXTS
                                .iter()
                                .any(|ext| vfs.resolve(&format!("texture/{name}.{ext}")).is_some())
                            {
                                decal += 1;
                            } else {
                                unresolved += 1;
                                placed
                                    .entry(f.bucket.clone())
                                    .or_default()
                                    .insert(name.clone());
                                // One issue per distinct dead name, no
                                // matter how many paths repeat it.
                                if tally.dead.insert(name.clone()) {
                                    issues.push(format!(
                                        "{}: path {:?} names {name:?} with no geometry or texture",
                                        f.logical, path.name
                                    ));
                                }
                            }
                        }
                        println!(
                            "  {:<56} {:<9} ok — {} paths, {} prop names: {} ({} decal, {} label, {} animated, {} unresolved)",
                            f.logical,
                            f.tag,
                            ps.paths.len(),
                            tally.names(),
                            tally.detail(),
                            decal,
                            label,
                            animated,
                            unresolved,
                        );
                        tally.print_dead();
                    }
                    Err(e) => source_fail(f, &e.to_string(), &mut failures, &mut unsupported),
                }
            }
            SourceKind::PropDefs => {
                let mut tally = Tally::default();
                match PropDefs::parse(&String::from_utf8_lossy(&bytes)) {
                    Ok(t) => {
                        for def in &t.defs {
                            for file in &def.files {
                                let name = file.to_ascii_lowercase();
                                tally.add(&name, bind_name(&name, &stems, &vfs));
                                placed.entry(f.bucket.clone()).or_default().insert(name);
                            }
                        }
                        println!(
                            "  {:<56} {:<9} ok — {} defs -> {} pkg names: {}",
                            f.logical,
                            f.tag,
                            t.defs.len(),
                            tally.names(),
                            tally.detail(),
                        );
                        report_dead(&f.logical, &tally, &mut issues);
                        defs_by_bucket
                            .entry(f.bucket.clone())
                            .or_default()
                            .push((f.logical.clone(), t));
                    }
                    Err(e) => source_fail(f, &e.to_string(), &mut failures, &mut unsupported),
                }
            }
            SourceKind::PropRules => match PropRules::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(t) => {
                    println!(
                        "  {:<56} {:<9} ok — {} rules",
                        f.logical,
                        f.tag,
                        t.rules.len(),
                    );
                    rules_by_bucket
                        .entry(f.bucket.clone())
                        .or_default()
                        .push((f.logical.clone(), t));
                }
                Err(e) => source_fail(f, &e.to_string(), &mut failures, &mut unsupported),
            },
            SourceKind::PropGroups => {
                let mut tally = Tally::default();
                match PropGroups::parse(&String::from_utf8_lossy(&bytes)) {
                    Ok(t) => {
                        for e in &t.entries {
                            let name = e.name.to_ascii_lowercase();
                            tally.add(&name, bind_name(&name, &stems, &vfs));
                            placed.entry(f.bucket.clone()).or_default().insert(name);
                        }
                        println!(
                            "  {:<56} {:<9} ok — {} group entries: {}",
                            f.logical,
                            f.tag,
                            t.entries.len(),
                            tally.detail(),
                        );
                        report_dead(&f.logical, &tally, &mut issues);
                    }
                    Err(e) => source_fail(f, &e.to_string(), &mut failures, &mut unsupported),
                }
            }
        }
    }

    // Per-bucket placed-name union.
    for (bucket, names) in &placed {
        let b = names.iter().filter(|n| stems.contains(*n)).count();
        let d = names
            .iter()
            .filter(|n| !stems.contains(*n) && vfs.resolve(&format!("geometry/{n}.pkg")).is_none())
            .count();
        println!(
            "  union {bucket}: {} placed names — {} bound / {} unbound / {} dead",
            names.len(),
            b,
            names.len() - b - d,
            d,
        );
    }

    // PSDL prop_rule bytes -> used rules -> reachable def files.
    if !rules_by_bucket.is_empty() {
        println!("rule reachability (psdl prop_rule bytes):");
    }
    for (bucket, rules) in &rules_by_bucket {
        let psdl_path = format!("city/{bucket}.psdl");
        let Some(pres) = vfs.resolve(&psdl_path) else {
            println!("  {bucket}: no {psdl_path} — rule usage unknown");
            continue;
        };
        let pbytes = vfs.read(&pres)?;
        let Ok(psdl) = Psdl::parse(&pbytes) else {
            println!("  {bucket}: {psdl_path} failed to parse — rule usage unknown");
            continue;
        };
        let used: BTreeSet<u8> = psdl
            .prop_rules
            .iter()
            .copied()
            .filter(|&b| b != 0)
            .collect();
        let used_defs: BTreeSet<&str> = rules
            .iter()
            .flat_map(|(_, t)| t.rules.iter())
            .filter(|r| r.rule_key().is_some_and(|(n, _)| used.contains(&n)))
            .flat_map(|r| r.props.iter().map(String::as_str))
            .collect();
        let mut files_used = Tally::default();
        for (_, defs) in defs_by_bucket.get(bucket).into_iter().flatten() {
            for def in &defs.defs {
                if used_defs.contains(def.name.as_str()) {
                    for file in &def.files {
                        files_used.add(&file.to_ascii_lowercase(), {
                            let name = file.to_ascii_lowercase();
                            bind_name(&name, &stems, &vfs)
                        });
                    }
                }
            }
        }
        println!(
            "  {bucket}: {} rule numbers used by {} rooms -> {} reachable defs -> {} files: {}",
            used.len(),
            psdl.prop_rules.iter().filter(|&&b| b != 0).count(),
            used_defs.len(),
            files_used.names(),
            files_used.detail(),
        );
    }

    // Reverse check (unfiltered audit): records never exercised by any
    // audited placement. vp*/va* owners bind through the vehicle
    // pipeline instead — counted separately, not failures.
    if stem_filter.is_none() {
        let union: BTreeSet<String> = placed.values().flatten().cloned().collect();
        let mut pkg_cache: BTreeMap<String, bool> = BTreeMap::new();
        let mut unplaced_vehicle = 0usize;
        let mut vehicle_owners: BTreeSet<String> = BTreeSet::new();
        let mut unplaced_other: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let (mut records_total, mut placed_records) = (0usize, 0usize);
        for stem in &stems {
            if stem == "default" || stem.starts_with(".#") {
                continue;
            }
            records_total += 1;
            let owner = geometry_owner(stem, |s| {
                *pkg_cache
                    .entry(s.to_string())
                    .or_insert_with(|| vfs.resolve(&format!("geometry/{s}.pkg")).is_some())
            });
            if union.contains(owner) || union.contains(stem) {
                placed_records += 1;
                continue;
            }
            if owner.starts_with("vp") || owner.starts_with("va") {
                unplaced_vehicle += 1;
                vehicle_owners.insert(owner.to_string());
            } else {
                unplaced_other
                    .entry(owner.to_string())
                    .or_default()
                    .push(stem.clone());
            }
        }
        println!("unplaced banger records:");
        println!("  {placed_records}/{records_total} records reachable through audited placements");
        println!(
            "  {unplaced_vehicle} records on {} vehicle owner pkgs (vp*/va* — bound by the vehicle pipeline, not placement)",
            vehicle_owners.len()
        );
        let other_total: usize = unplaced_other.values().map(Vec::len).sum();
        println!(
            "  {other_total} records on {} other unplaced owner pkgs:",
            unplaced_other.len()
        );
        for (owner, recs) in &unplaced_other {
            let shown: Vec<&str> = recs.iter().take(8).map(String::as_str).collect();
            println!(
                "    {owner} ({}): {}{}",
                recs.len(),
                shown.join(", "),
                if recs.len() > 8 {
                    format!(" (+{} more)", recs.len() - 8)
                } else {
                    String::new()
                }
            );
        }
    }

    println!(
        "  {} source files, {unsupported} unsupported, {} failures, {} issue(s)",
        files.len(),
        failures.len(),
        issues.len(),
    );
    for i in &issues {
        println!("    issue: {i}");
    }
    if strict && (!failures.is_empty() || !issues.is_empty()) {
        return Err(format!(
            "strict banger-bind audit: {} failures, {} issues",
            failures.len(),
            issues.len()
        )
        .into());
    }
    Ok(())
}

/// Dead names from a non-pathset source become issues (pathset
/// unresolved names are already issued where they are classified).
fn report_dead(logical: &str, tally: &Tally, issues: &mut Vec<String>) {
    for name in &tally.dead {
        issues.push(format!(
            "{logical}: placed name {name:?} resolves to no geometry PKG"
        ));
    }
    tally.print_dead();
}

/// A source that failed to read/parse: `expected` files are failures,
/// everything else is `unsupported` (reported, not hidden).
fn source_fail(f: &SourceFile, e: &str, failures: &mut Vec<String>, unsupported: &mut usize) {
    if f.tag == "expected" {
        println!("  {:<56} {:<9} failed: {e}", f.logical, f.tag);
        failures.push(format!("{}: {e}", f.logical));
    } else {
        println!("  {:<56} {:<9} unsupported: {e}", f.logical, f.tag);
        *unsupported += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_source_buckets() {
        use SourceKind::*;
        assert_eq!(
            classify_source("city/london.inst"),
            Some(("london".to_string(), Inst))
        );
        assert_eq!(
            classify_source("city/london_ai.inst"),
            Some(("london_ai".to_string(), Inst))
        );
        assert_eq!(
            classify_source("city/sf.sdl_ai.inst"),
            Some(("sf.sdl_ai".to_string(), Inst))
        );
        assert_eq!(
            classify_source("city/london/props.pathset"),
            Some(("london".to_string(), Pathset))
        );
        assert_eq!(
            classify_source("city/sf/bak/circuit0.pathset"),
            Some(("sf/bak".to_string(), Pathset))
        );
        assert_eq!(
            classify_source("city/race0.pathset"),
            Some(("race0".to_string(), Pathset))
        );
        assert_eq!(
            classify_source("race/london/circuit0.pathset"),
            Some(("london".to_string(), Pathset))
        );
        assert_eq!(
            classify_source("city/london/propdefs.csv"),
            Some(("london".to_string(), PropDefs))
        );
        assert_eq!(
            classify_source("city/phys/proprules.csv"),
            Some(("phys".to_string(), PropRules))
        );
        assert_eq!(
            classify_source("city/sf/props.csv.txt"),
            Some(("sf".to_string(), PropGroups))
        );
        assert_eq!(
            classify_source("city/props.csv"),
            Some(("props".to_string(), PropGroups))
        );
        // Not placement sources.
        assert_eq!(classify_source("geometry/props.csv"), None);
        assert_eq!(classify_source("tune/banger/default.dgbangerdata"), None);
        assert_eq!(classify_source("city/london.bai"), None);
        assert_eq!(classify_source("city/london/racedata.csv"), None);
        assert_eq!(classify_source("race/sf/stunt0.aimap"), None);
        assert_eq!(classify_source("race/london/race6.opp"), None);
    }
}
