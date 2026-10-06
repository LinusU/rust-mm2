//! `cnr` audit (F27-A): the Cops & Robbers content each city must
//! carry — the gold/hideout/bank site pool, marker models and their
//! banger records, map dots, the commentary cue vocabulary and the
//! mode's loading image — resolved through the production
//! [`CnrContent`] loader.
//!
//! The denominator is fixed by [`CnrContent::dependency_list`] and the
//! stock cue list, not by what happens to resolve: a missing file is a
//! counted failure. `--strict` exits nonzero on any city with an issue.
//! A passing audit says the *data* is present and parses; it says
//! nothing about the mode's rules, which are code constants (see
//! `docs/research/cnr.md`).

use std::path::Path;

use mm2_content::cnr::{COMMENTARY_CUES, CnrContent};

use crate::build_vfs;

/// Cities to audit: the explicit one, or the stock cities plus any
/// `race/<city>/` directory discovered (a mod's city shows up too).
fn cities(vfs: &mm2_assets::Vfs, city: Option<&str>) -> Vec<String> {
    match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => mm2_content::race_cities(vfs),
    }
}

/// Print one city and return its issue lines (empty when complete).
fn report(content: &CnrContent) -> Vec<String> {
    println!("== cops & robbers: {} ==", content.city);
    for d in &content.dependencies {
        println!(
            "  {:<7} {:<44} {}",
            if d.found { "found" } else { "MISSING" },
            d.logical,
            d.role
        );
    }
    println!(
        "  site pool: {} waypoints (a round draws 3)",
        content.sites.len()
    );
    println!(
        "  commentary cues: {}/{} families with rows",
        content.cues_present,
        COMMENTARY_CUES.len()
    );
    println!(
        "  dependencies: {}/{} resolved",
        content.found_count(),
        content.dependencies.len()
    );
    let issues: Vec<String> = content
        .issues
        .iter()
        .map(|i| format!("{}: {i}", content.city))
        .collect();
    for i in &issues {
        println!("  issue: {i}");
    }
    println!();
    issues
}

/// Run the audit over every selected city.
pub fn run(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let cities = cities(&vfs, city);
    let mut failures = Vec::new();
    let mut complete = 0;
    for c in &cities {
        let content = CnrContent::load(&vfs, c);
        if content.is_complete() {
            complete += 1;
        }
        failures.extend(report(&content));
    }
    println!(
        "cnr: {} cities audited — {} complete, {} with issues ({} issues)",
        cities.len(),
        complete,
        cities.len() - complete,
        failures.len()
    );
    if strict && cities.is_empty() {
        return Err("strict cnr audit: no city to audit".into());
    }
    if strict && !failures.is_empty() {
        return Err(format!("strict cnr audit: {} issues", failures.len()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// Write every dependency of `city`, with a real cue table and a
    /// three-row pool.
    fn complete_city(dir: &Path, city: &str) {
        let mut cues = String::from("Name prefix/type header,end sufix value,sufix add value\n");
        for c in COMMENTARY_CUES {
            cues.push_str(&format!("{c} header,,\nAL1\\{c},1,0\n"));
        }
        for (logical, _) in CnrContent::dependency_list(city) {
            let body = if logical == CnrContent::placement_path(city) {
                "x,y,z,a,poly count,frane rate,state changes,texture changes,msg\n\
                 1,2,3,0,0,0,0,0,\n4,5,6,0,0,0,0,0,\n7,8,9,0,0,0,0,0,\n"
                    .to_string()
            } else if logical == CnrContent::commentary_path(city) {
                cues.clone()
            } else {
                "x".to_string()
            };
            write(dir, &logical, &body);
        }
    }

    fn vfs_of(dir: &Path) -> mm2_assets::Vfs {
        let mut vfs = mm2_assets::Vfs::new();
        vfs.mount_dir(dir, 0).unwrap();
        vfs
    }

    #[test]
    fn a_complete_city_reports_no_issue_lines() {
        let d = tempfile::tempdir().unwrap();
        complete_city(d.path(), "sf");
        let content = CnrContent::load(&vfs_of(d.path()), "sf");
        assert!(report(&content).is_empty());
    }

    #[test]
    fn a_missing_marker_model_is_an_issue_line_naming_the_city_and_file() {
        let d = tempfile::tempdir().unwrap();
        complete_city(d.path(), "sf");
        fs::remove_file(d.path().join("geometry/pt_red.pkg")).unwrap();
        let content = CnrContent::load(&vfs_of(d.path()), "sf");
        assert_eq!(
            report(&content),
            vec!["sf: missing geometry/pt_red.pkg".to_string()]
        );
    }

    #[test]
    fn the_stock_cities_are_audited_even_when_the_install_has_none() {
        // A city absent from disk still appears, so strict fails on it.
        let d = tempfile::tempdir().unwrap();
        let vfs = vfs_of(d.path());
        let cs = cities(&vfs, None);
        assert!(cs.contains(&"sf".to_string()) && cs.contains(&"london".to_string()));
        assert!(!CnrContent::load(&vfs, "sf").is_complete());
    }

    #[test]
    fn an_explicit_city_is_lower_cased_and_alone() {
        let d = tempfile::tempdir().unwrap();
        let vfs = vfs_of(d.path());
        assert_eq!(cities(&vfs, Some("SF")), vec!["sf".to_string()]);
    }
}
