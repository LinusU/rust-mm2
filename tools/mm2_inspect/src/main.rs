//! `mm2-inspect`: command line inspector for MM2 archives and assets.
//!
//! Uses the same `mm2_assets` mounting policy as the game: same archive
//! discovery, source order, path normalization and mod handling.
//!
//! Exit codes: 0 = success; 2 = a requested lookup failed, a parse failed,
//! or `--strict` found failures.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mm2_assets::{AssetsError, InstallMount, MountReport, Vfs, mount_install, mount_mods};
use mm2_formats::pkg::{Pkg, PkgChunk};
use mm2_formats::psdl::Psdl;
use mm2_formats::tex::TexFile;
use mm2_formats::{FormatError, inst};

mod inventory;

/// Extensions the texture pipeline tries, in preference order — the same
/// order `mm2_app` uses.
const TEXTURE_EXTS: &[&str] = &["png", "ktx2", "tga", "tex"];

#[derive(Parser)]
#[command(name = "mm2-inspect", about = "Inspect MM2 installations and assets")]
struct Cli {
    /// Directory containing mod folders (each with a mod.toml). Mounted
    /// above install content, exactly as in the game.
    #[arg(long, global = true)]
    mods: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan an MM2 installation: count files, formats and parse failures.
    Scan {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Strict validation: exit nonzero if any recognized file fails to
        /// parse or carries unparsed remnants.
        #[arg(long)]
        strict: bool,
    },
    /// List all logical paths known to the VFS.
    List {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Optional prefix filter, e.g. `texture/`.
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Resolve a logical path and show provenance.
    Resolve {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Logical path, e.g. `texture/vpcaddieblue_bk.tex`.
        logical: String,
    },
    /// Explain a logical texture lookup: every extension tried, the winning
    /// source and why it won.
    Lookup {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Logical stem without extension, e.g. `texture/vpcaddieblue_bk`.
        stem: String,
    },
    /// Parse and describe a TEX texture.
    Tex {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Logical path of the texture.
        logical: String,
    },
    /// Parse and describe a PKG object.
    Pkg {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Logical path of the package.
        logical: String,
    },
    /// Parse and describe a PSDL city file.
    Psdl {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Logical path of the city file.
        logical: String,
    },
    /// Write the raw (decompressed) bytes of a logical path to stdout.
    Dump {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Logical path, e.g. `tune/vpbug.info`.
        logical: String,
    },
    /// List the discovered vehicle roster with audit status.
    Cars {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
    },
    /// Load and describe one vehicle: metadata, converted handling, wheel
    /// rig, model parts, paint validation and source provenance.
    Car {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Vehicle id (e.g. `vpbug`) or unique display-name alias.
        id: String,
        /// Paint-job index to validate (zero-based).
        #[arg(long, default_value_t = 0)]
        paint: usize,
        /// Emit the full report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Audit vehicle handling: rollover margin, ride height, suspension
    /// and steering, for one car or the whole roster.
    Handling {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Vehicle id or alias. Omitted, every ready vehicle is audited.
        id: Option<String>,
        /// Exit nonzero when any audited vehicle reports a problem.
        #[arg(long)]
        strict: bool,
    },
    /// Validate vehicles end to end: metadata, tuning, model, wheel rig,
    /// collider and every declared paint variant.
    ValidateCars {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Validate every discovered entry, not just the expected stock
        /// roster.
        #[arg(long)]
        all: bool,
        /// Exit nonzero on any warning as well as failures.
        #[arg(long)]
        strict: bool,
    },
    /// List the authored event catalog per city: every `mm*data.csv`
    /// row with its resolved dependent records, parse status and failed
    /// references.
    Events {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem (default: every discovered
        /// `race/<city>/` directory).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero on an empty catalog, a missing/malformed table,
        /// or any incomplete event.
        #[arg(long)]
        strict: bool,
    },
    /// Versioned content inventory: expected/discovered/accepted/
    /// rejected/unverified counts per content family, fingerprinted by
    /// engine commit and resolved-path provenance.
    Inventory {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Emit the full report as JSON.
        #[arg(long)]
        json: bool,
        /// Exit nonzero on any rejected entry or an empty expected family.
        #[arg(long)]
        strict: bool,
    },
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    match &cli.command {
        Command::Scan { dir, strict } => scan(dir, cli.mods.as_deref(), *strict),
        Command::List { dir, prefix } => list(dir, cli.mods.as_deref(), prefix.as_deref()),
        Command::Resolve { dir, logical } => resolve(dir, cli.mods.as_deref(), logical),
        Command::Lookup { dir, stem } => lookup(dir, cli.mods.as_deref(), stem),
        Command::Tex { dir, logical } => tex(dir, cli.mods.as_deref(), logical),
        Command::Pkg { dir, logical } => pkg(dir, cli.mods.as_deref(), logical),
        Command::Psdl { dir, logical } => psdl(dir, cli.mods.as_deref(), logical),
        Command::Dump { dir, logical } => dump(dir, cli.mods.as_deref(), logical),
        Command::Cars { dir } => cars(dir, cli.mods.as_deref()),
        Command::Car {
            dir,
            id,
            paint,
            json,
        } => car(dir, cli.mods.as_deref(), id, *paint, *json),
        Command::Handling { dir, id, strict } => {
            handling(dir, cli.mods.as_deref(), id.as_deref(), *strict)
        }
        Command::ValidateCars { dir, all, strict } => {
            validate_cars(dir, cli.mods.as_deref(), *all, *strict)
        }
        Command::Events { dir, city, strict } => {
            events(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Inventory { dir, json, strict } => {
            inventory_cmd(dir, cli.mods.as_deref(), *json, *strict)
        }
    }
}

/// Build the VFS exactly like the game does: install (archives + loose
/// files) then mods. Returns the mount diagnostics alongside.
fn build_vfs_report(dir: &Path, mods: Option<&Path>) -> Result<(Vfs, MountReport), AssetsError> {
    let mut vfs = Vfs::new();
    let mut report = mount_install(&mut vfs, dir, &InstallMount::default())?;
    for (path, err) in &report.skipped {
        eprintln!("skipped archive {}: {err}", path.display());
    }
    if let Some(mods) = mods {
        let manifests = mount_mods(&mut vfs, mods)?;
        for m in &manifests {
            eprintln!("mounted mod {}", m.id);
        }
        report.mods = manifests;
    }
    Ok((vfs, report))
}

/// Build the VFS exactly like the game does: install (archives + loose
/// files) then mods.
fn build_vfs(dir: &Path, mods: Option<&Path>) -> Result<Vfs, AssetsError> {
    build_vfs_report(dir, mods).map(|(vfs, _)| vfs)
}

/// Per-file scan categories.
#[derive(Default)]
struct ScanStats {
    parsed_ok: usize,
    /// Parsed but carrying preserved unknown content (PKG raw chunks,
    /// PSDL unparsed attribute words, unknown pixel formats).
    partial: Vec<(String, String)>,
    /// Extensions we do not interpret at all.
    unsupported: BTreeMap<String, usize>,
    failures: Vec<(String, String)>,
}

fn scan(dir: &Path, mods: Option<&Path>, strict: bool) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let paths = vfs.list();
    let mut by_ext: BTreeMap<String, usize> = BTreeMap::new();
    let mut stats = ScanStats::default();
    for logical in &paths {
        let ext = logical.rsplit('.').next().unwrap_or("").to_string();
        *by_ext.entry(ext.clone()).or_default() += 1;
        // Read each recognized asset once and reuse the bytes.
        let bytes = match vfs.read_logical(logical) {
            Ok(b) => b,
            Err(e) => {
                stats.failures.push((logical.clone(), e.to_string()));
                continue;
            }
        };
        if bytes.is_empty() {
            continue; // zero-length entries occur in retail archives
        }
        match ext.as_str() {
            // .tga is a different format entirely; only .tex is TEX.
            "tex" => match TexFile::parse(&bytes) {
                Ok(t) => {
                    if matches!(t.header.format, mm2_formats::tex::PixelFormat::Unknown(_)) {
                        stats.partial.push((
                            logical.clone(),
                            format!("unknown pixel type {}", t.header.format.raw()),
                        ));
                    } else {
                        stats.parsed_ok += 1;
                    }
                }
                Err(e) => stats.failures.push((logical.clone(), e.to_string())),
            },
            "pkg" => match Pkg::parse(&bytes) {
                Ok(p) => {
                    let raw = p
                        .files
                        .iter()
                        .filter(|f| matches!(f.data, PkgChunk::Raw(_)))
                        .count();
                    if raw > 0 {
                        stats
                            .partial
                            .push((logical.clone(), format!("{raw} preserved raw chunk(s)")));
                    } else {
                        stats.parsed_ok += 1;
                    }
                }
                Err(e) => stats.failures.push((logical.clone(), e.to_string())),
            },
            "psdl" => match Psdl::parse(&bytes) {
                Ok(p) => {
                    let unparsed: usize = p.rooms.iter().map(|r| r.unparsed_attributes.len()).sum();
                    if unparsed > 0 {
                        stats.partial.push((
                            logical.clone(),
                            format!("{unparsed} unparsed attribute words"),
                        ));
                    } else {
                        stats.parsed_ok += 1;
                    }
                }
                Err(e) => stats.failures.push((logical.clone(), e.to_string())),
            },
            "inst" => match inst::parse(&bytes) {
                Ok(_) => stats.parsed_ok += 1,
                Err(e) => stats.failures.push((logical.clone(), e.to_string())),
            },
            _ => *stats.unsupported.entry(ext).or_insert(0) += 1,
        }
    }
    println!("== scan of {}", dir.display());
    println!("total logical files: {}", paths.len());
    println!("parsed & validated:    {}", stats.parsed_ok);
    println!("partially preserved:   {}", stats.partial.len());
    println!("parse failures:        {}", stats.failures.len());
    println!("\nby extension:");
    for (ext, count) in &by_ext {
        let tag = if stats.unsupported.contains_key(ext) {
            " (unsupported)"
        } else {
            ""
        };
        println!("  {ext:12} {count}{tag}");
    }
    if !stats.partial.is_empty() {
        println!("\npartial (parsed with preserved unknown content):");
        for (path, note) in stats.partial.iter().take(20) {
            println!("  {path}: {note}");
        }
        if stats.partial.len() > 20 {
            println!("  … and {} more", stats.partial.len() - 20);
        }
    }
    if !stats.failures.is_empty() {
        println!("\nfailures:");
        for (path, err) in stats.failures.iter().take(20) {
            println!("  {path}: {err}");
        }
        if stats.failures.len() > 20 {
            println!("  … and {} more", stats.failures.len() - 20);
        }
    }
    if strict && (!stats.failures.is_empty() || !stats.partial.is_empty()) {
        return Err(format!(
            "strict scan: {} failures, {} partial files",
            stats.failures.len(),
            stats.partial.len()
        )
        .into());
    }
    Ok(())
}

fn list(
    dir: &Path,
    mods: Option<&Path>,
    prefix: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let prefix = prefix.map(|p| p.to_ascii_lowercase());
    for p in vfs.list() {
        if prefix.as_ref().is_none_or(|pre| p.starts_with(pre)) {
            println!("{p}");
        }
    }
    Ok(())
}

fn resolve(
    dir: &Path,
    mods: Option<&Path>,
    logical: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    match vfs.resolve(logical) {
        Some(r) => {
            println!("logical : {}", r.logical);
            println!("kind    : {:?}", r.source.kind);
            println!("source  : {}", r.source.path.display());
            if let Some(label) = &r.source.label {
                println!("label   : {label}");
            }
            if let Some(off) = r.source.archive_offset {
                println!("offset  : {off:#x}");
            }
            let bytes = vfs.read(&r)?;
            println!("size    : {} bytes", bytes.len());
            Ok(())
        }
        None => Err(format!("not found: {logical}").into()),
    }
}

/// Explain which file wins a logical texture lookup and why.
fn lookup(dir: &Path, mods: Option<&Path>, stem: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    println!(
        "lookup  : {stem} (preference order: {})",
        TEXTURE_EXTS.join(", ")
    );
    for ext in TEXTURE_EXTS {
        let logical = format!("{stem}.{ext}");
        match vfs.resolve(&logical) {
            Some(r) => {
                println!(
                    "  {ext:5} → {:50} [{} {}]",
                    r.logical,
                    match r.source.kind {
                        mm2_assets::SourceKind::Archive => "archive",
                        mm2_assets::SourceKind::Directory => "dir",
                    },
                    r.source.path.display(),
                );
            }
            None => println!("  {ext:5} → (absent)"),
        }
    }
    match vfs.resolve_preferred(stem, TEXTURE_EXTS) {
        Some(r) => {
            println!("winner  : {}", r.logical);
            println!("kind    : {:?}", r.source.kind);
            println!("source  : {}", r.source.path.display());
            if let Some(label) = &r.source.label {
                println!("label   : {label}");
            }
            println!(
                "reason  : highest-priority source wins first; extension preference applies only within the winning source"
            );
            Ok(())
        }
        None => Err(format!("no candidate found for stem {stem}").into()),
    }
}

fn tex(dir: &Path, mods: Option<&Path>, logical: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let (bytes, r) = vfs.read_path(logical)?;
    let tex = TexFile::parse(&bytes).map_err(|e| attach(&r, e))?;
    println!(
        "{}: {}x{} format={:?} mips={} bits={:#x}",
        r.logical,
        tex.header.width,
        tex.header.height,
        tex.header.format,
        tex.header.mips,
        tex.header.bits
    );
    for (i, level) in tex.levels.iter().enumerate() {
        println!(
            "  level {i}: {}x{} ({} bytes)",
            level.width,
            level.height,
            level.data.len()
        );
    }
    Ok(())
}

fn pkg(dir: &Path, mods: Option<&Path>, logical: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let (bytes, r) = vfs.read_path(logical)?;
    let pkg = Pkg::parse(&bytes).map_err(|e| attach(&r, e))?;
    println!(
        "{}: {} chunks (version {})",
        r.logical,
        pkg.files.len(),
        std::str::from_utf8(&pkg.version).unwrap_or("?")
    );
    for file in &pkg.files {
        match &file.data {
            PkgChunk::Geometry(g) => {
                let verts: usize = g
                    .sections
                    .iter()
                    .flat_map(|s| &s.strips)
                    .map(|s| s.vertices.len())
                    .sum();
                let indices: usize = g
                    .sections
                    .iter()
                    .flat_map(|s| &s.strips)
                    .map(|s| s.indices.len())
                    .sum();
                println!(
                    "  {:20} geometry: {} sections, {} verts, {} indices, fvf={:#x}",
                    file.name,
                    g.sections.len(),
                    verts,
                    indices,
                    g.fvf
                );
            }
            PkgChunk::Shaders(s) => println!(
                "  {:20} shaders: {} paint jobs x {} shaders",
                file.name, s.paint_jobs, s.shaders_per_paint_job
            ),
            PkgChunk::Offset(o) => println!("  {:20} offset: {o:?}", file.name),
            PkgChunk::Xref(x) => println!("  {:20} xref: {} refs", file.name, x.len()),
            PkgChunk::Raw(b) => println!("  {:20} raw (preserved): {} bytes", file.name, b.len()),
        }
    }
    Ok(())
}

fn psdl(dir: &Path, mods: Option<&Path>, logical: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let (bytes, r) = vfs.read_path(logical)?;
    let psdl = Psdl::parse(&bytes).map_err(|e| attach(&r, e))?;
    let unparsed: usize = psdl.rooms.iter().map(|r| r.unparsed_attributes.len()).sum();
    println!(
        "{}: {} verts, {} heights, {} textures, {} rooms, {} paths",
        r.logical,
        psdl.vertices.len(),
        psdl.heights.len(),
        psdl.textures.len(),
        psdl.rooms.len(),
        psdl.paths.len()
    );
    println!(
        "bounds: {:?} .. {:?}, radius {}",
        psdl.bounds_min, psdl.bounds_max, psdl.bounds_radius
    );
    println!("unparsed attribute words remaining: {unparsed}");
    Ok(())
}

fn dump(dir: &Path, mods: Option<&Path>, logical: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let (bytes, _r) = vfs.read_path(logical)?;
    use std::io::Write;
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}

fn cars(dir: &Path, mods: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let catalog = mm2_content::VehicleCatalog::scan(&vfs);
    if catalog.entries.is_empty() {
        return Err("vehicle catalog is empty — no tune/geometry data discovered".into());
    }
    println!("== vehicle roster ({} entries) ==", catalog.entries.len());
    println!(
        "{:<14} {:<30} {:<8} {:<5} {:<6} status",
        "id", "name", "class", "lock", "paints"
    );
    for e in &catalog.entries {
        let class = match e.class {
            mm2_content::VehicleClass::Stock => "stock",
            mm2_content::VehicleClass::Mod => "mod",
            mm2_content::VehicleClass::ModOnly => "mod-only",
        };
        let status = match &e.status {
            mm2_content::EntryStatus::Ready => "ready".to_string(),
            mm2_content::EntryStatus::Incomplete { missing } => {
                format!("incomplete: {}", missing.join(", "))
            }
        };
        println!(
            "{:<14} {:<30} {:<8} {:<5} {:<6} {}",
            e.id,
            e.display_name,
            class,
            if e.locked { "yes" } else { "-" },
            e.paints.len(),
            status
        );
    }
    let failures = catalog.stock_audit_failures();
    if !failures.is_empty() {
        println!("\nexpected-stock audit failures:");
        for f in &failures {
            println!("  {f}");
        }
    }
    Ok(())
}

/// A compact per-record summary for the events listing.
fn record_tag(r: &mm2_content::EventRecord) -> String {
    use mm2_content::RecordContent as C;
    use mm2_formats::racefiles::RaceFileKind as K;
    match &r.content {
        C::Waypoints {
            rows, width_label, ..
        } => format!("wp:{rows}({width_label})"),
        C::StartPoints { rows, .. } => format!("strtpnts:{rows}"),
        C::Opp { rows, .. } => format!("opp{}:{rows}", r.difficulty.unwrap_or('-')),
        C::CrashData { rows, .. } => format!("data:{rows}"),
        C::Unparsed => match r.kind {
            K::Aimap => "aimap".into(),
            K::AimapP => "aimap_p".into(),
            K::Pathset => "pathset".into(),
            other => format!("{other:?}"),
        },
        C::Failed(_) => format!("!{:?}", r.kind),
    }
}

fn events(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;

    // Which cities to scan: the explicit one, or every `race/<city>/`
    // directory discovered (stock + any mod-provided cities).
    let cities: Vec<String> = match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => {
            let mut found: std::collections::BTreeSet<String> = mm2_content::EXPECTED_RACE_CITIES
                .iter()
                .map(|s| s.to_string())
                .collect();
            for p in vfs.list() {
                if let Some(rest) = p.strip_prefix("race/")
                    && let Some((c, _)) = rest.split_once('/')
                {
                    found.insert(c.to_string());
                }
            }
            found.into_iter().collect()
        }
    };

    let mut failures: Vec<String> = Vec::new();
    for city in &cities {
        let cat = mm2_content::EventCatalog::scan(&vfs, city);
        println!("== events: {city} ==");
        for t in &cat.tables {
            match &t.error {
                Some(e) => {
                    println!("  {:<18} ERROR: {e}", t.logical);
                    failures.push(format!("{city}: {} — {e}", t.logical));
                }
                None => println!(
                    "  {:<18} {:>2} rows ({} row diagnostics)",
                    t.logical, t.rows, t.diagnostics
                ),
            }
        }
        if cat.is_empty() {
            println!("  (no authored events cataloged)");
            failures.push(format!("{city}: event catalog is empty"));
        }
        for ev in &cat.events {
            let deps = ev
                .records
                .iter()
                .map(record_tag)
                .collect::<Vec<_>>()
                .join(" ");
            let status = match &ev.status {
                mm2_content::EventStatus::Ready => "ready".to_string(),
                mm2_content::EventStatus::Incomplete { missing } => {
                    failures.push(format!(
                        "{city}: {} — incomplete: {}",
                        ev.stem,
                        missing.join(", ")
                    ));
                    format!("incomplete ({})", missing.join(", "))
                }
            };
            println!(
                "  [{:>2}] {:<10} {:<9} {:<28} {}",
                ev.event_ref.index, ev.stem, ev.description, status, deps
            );
            for f in &ev.failed {
                println!("       failed ref: {} — {}", f.reference, f.reason);
            }
            for r in &ev.rewards {
                println!(
                    "       reward: {} {:?} {} variant {} — {}",
                    r.race_type, r.race_num, r.car, r.variant, r.message
                );
            }
        }
        if !cat.extras.is_empty() {
            println!(
                "  extras ({}): {}",
                cat.extras.len(),
                cat.extras
                    .iter()
                    .map(|e| e.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if !cat.milestone_rewards.is_empty() {
            println!("  milestone rewards:");
            for r in &cat.milestone_rewards {
                println!(
                    "    {} {:?} → {} variant {} — {}",
                    r.race_type, r.race_num, r.car, r.variant, r.message
                );
            }
        }
        for d in &cat.diagnostics {
            println!("  note: {d}");
        }
        println!();
    }
    if strict && !failures.is_empty() {
        return Err(format!("strict events audit: {} failures", failures.len()).into());
    }
    Ok(())
}

/// Paint-variant validation: every shader offset referenced by any mesh
/// group must land inside the shader table for that paint job.
fn paint_coverage(def: &mm2_content::VehicleDef) -> Vec<String> {
    let mut offsets = std::collections::BTreeSet::new();
    for part in &def.model.parts {
        for (_, groups) in &part.lods {
            for g in groups {
                offsets.insert(g.shader_offset);
            }
        }
    }
    let mut problems = Vec::new();
    let declared = def.paints.len().max(def.model.paint_jobs);
    for paint in 0..declared {
        if paint >= def.model.paint_jobs {
            problems.push(format!(
                "paint {paint}: declared but model only has {} paint job(s)",
                def.model.paint_jobs
            ));
            continue;
        }
        for off in &offsets {
            let idx = paint * def.model.shaders_per_paint_job + off;
            if idx >= def.model.shaders.len() {
                problems.push(format!(
                    "paint {paint}: shader offset {off} → index {idx} beyond {} shaders",
                    def.model.shaders.len()
                ));
            }
        }
    }
    problems
}

fn car(
    dir: &Path,
    mods: Option<&Path>,
    id: &str,
    paint: usize,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let def = mm2_content::load_by_id(&vfs, id, paint)?;
    let paint_problems = paint_coverage(&def);

    if json {
        let c = &def.config;
        let out = serde_json::json!({
            "id": def.id,
            "display_name": def.display_name,
            "paints": def.paints,
            "paint_jobs": def.model.paint_jobs,
            "handling": {
                "mass_kg": c.mass,
                "center_of_mass": c.center_of_mass,
                "inertia_kgm2": c.inertia,
                "wheelbase_m": c.wheelbase,
                "track_width_m": c.track_width,
                "chassis_size_m": c.chassis_size,
                "engine": {
                    "max_power_w": c.engine.max_power_w,
                    "idle_rpm": c.engine.idle_rpm,
                    "redline_rpm": c.engine.redline_rpm,
                    "peak_torque_nm": c.engine.peak_torque_nm,
                    "peak_torque_rpm": c.engine.peak_torque_rpm,
                },
                "transmission": {
                    "gear_ratios": c.transmission.gear_ratios,
                    "reverse_ratio": c.transmission.reverse_ratio,
                    "shift_time_s": c.transmission.shift_time,
                },
                "wheels": c.wheels.iter().map(|w| serde_json::json!({
                    "position": w.position,
                    "radius": w.radius,
                    "driven": w.driven,
                    "steered": w.steered,
                    "steer_scale": w.steer_scale,
                    "brake_bias": w.brake_bias,
                    "drive_share": w.drive_share,
                })).collect::<Vec<_>>(),
                "collider_points": c.collider_points.as_ref().map(|p| p.len()),
                "trailer": def.trailer.is_some(),
            },
            "model": {
                "paint_jobs": def.model.paint_jobs,
                "shaders_per_paint_job": def.model.shaders_per_paint_job,
                "shaders": def.model.shaders.iter().enumerate().map(|(i, s)| serde_json::json!({
                    "index": i,
                    "texture": s.texture,
                    "diffuse": s.diffuse,
                    "emissive": s.emissive,
                })).collect::<Vec<_>>(),
                "parts": def.model.parts.iter().map(|p| {
                    let bbox = p.best_nonempty_lod().and_then(|groups| {
                        let mut mn = [f32::MAX; 3];
                        let mut mx = [f32::MIN; 3];
                        for g in groups {
                            for v in &g.positions {
                                for i in 0..3 {
                                    mn[i] = mn[i].min(v[i]);
                                    mx[i] = mx[i].max(v[i]);
                                }
                            }
                        }
                        (mn[0] != f32::MAX).then_some([mn, mx])
                    });
                    serde_json::json!({
                        "name": p.name,
                        "role": format!("{:?}", p.role),
                        "lods": p.lods.iter().map(|(l, _)| format!("{l:?}")).collect::<Vec<_>>(),
                        "origin": p.origin,
                        "recenter": p.recenter,
                        "geom_bbox": bbox,
                    })
                }).collect::<Vec<_>>(),
                "wheel_visuals": def.model.wheels.iter().map(|w| serde_json::json!({
                    "index": w.index,
                    "trailer": w.trailer,
                    "origin": w.origin,
                    "radius": w.radius,
                    "width": w.width,
                })).collect::<Vec<_>>(),
            },
            "sources": def.sources,
            "conversion": def.report.entries.iter().map(|e| serde_json::json!({
                "source": e.source,
                "dest": e.dest,
                "provenance": format!("{:?}", e.provenance),
                "note": e.note,
            })).collect::<Vec<_>>(),
            "warnings": def.report.warnings,
            "model_warnings": def.model.warnings,
            "paint_problems": paint_problems,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        let c = &def.config;
        println!("== {} — {} ==", def.id, def.display_name);
        println!(
            "paints ({} declared, {} jobs): {}",
            def.paints.len(),
            def.model.paint_jobs,
            def.paints.join(", ")
        );
        println!(
            "mass {:.0} kg, com {:?}, size {:?}, wheelbase {:.2} m, track {:.2} m",
            c.mass, c.center_of_mass, c.chassis_size, c.wheelbase, c.track_width
        );
        if let Some(w) = c.engine.max_power_w {
            println!(
                "engine {:.0} kW at {:?} rpm, {:.0} N·m at {:.0} rpm, redline {:.0}",
                w / 1000.0,
                c.engine.peak_power_rpm,
                c.engine.peak_torque_nm,
                c.engine.peak_torque_rpm,
                c.engine.redline_rpm
            );
        }
        println!(
            "gears {:?} reverse {:.2}, shift {:.2} s",
            c.transmission
                .gear_ratios
                .iter()
                .map(|r| format!("{r:.2}"))
                .collect::<Vec<_>>(),
            c.transmission.reverse_ratio,
            c.transmission.shift_time
        );
        println!("wheels ({} physics):", c.wheels.len());
        for (i, w) in c.wheels.iter().enumerate() {
            println!(
                "  [{i}] pos {:?} r {:.3} driven={} steered={} steer×{:.2} brake {:.2} drive {:?}",
                w.position,
                w.radius,
                w.driven,
                w.steered,
                w.steer_scale,
                w.brake_bias,
                w.drive_share
            );
        }
        println!("model parts ({}):", def.model.parts.len());
        for p in &def.model.parts {
            let lods: Vec<String> = p.lods.iter().map(|(l, _)| format!("{l:?}")).collect();
            println!(
                "  {:<16} {:?} lods={:?} origin={:?}{}",
                p.name,
                p.role,
                lods,
                p.origin,
                if p.recenter.is_some() {
                    " (recentred)"
                } else {
                    ""
                }
            );
        }
        for w in &def.model.wheels {
            println!(
                "  wheel visual {} trailer={} origin {:?} r {:.3} w {:.3} parts {:?}",
                w.index, w.trailer, w.origin, w.radius, w.width, w.parts
            );
        }
        if let Some(t) = &def.trailer {
            println!(
                "trailer: {:.0} kg, {} wheels, hitch car {:?} trailer {:?}",
                t.config.mass,
                t.wheels.len(),
                t.car_hitch,
                t.trailer_hitch
            );
        }
        if !paint_problems.is_empty() {
            println!("paint problems:");
            for p in &paint_problems {
                println!("  {p}");
            }
        }
        if !def.report.warnings.is_empty() || !def.model.warnings.is_empty() {
            println!("warnings:");
            for w in def.report.warnings.iter().chain(&def.model.warnings) {
                println!("  {w}");
            }
        }
        println!("sources:");
        for s in &def.sources {
            println!("  {s}");
        }
    }
    if !paint_problems.is_empty() {
        return Err(format!("{} paint variant problem(s)", paint_problems.len()).into());
    }
    Ok(())
}

/// Audit handling for one vehicle or the whole ready roster.
///
/// Every column comes from [`mm2_vehicle::HandlingMetrics`], which solves
/// the car's resting state in closed form — no physics is run, so the whole
/// roster audits instantly and the numbers are reproducible.
fn handling(
    dir: &Path,
    mods: Option<&Path>,
    id: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;

    let ids: Vec<String> = match id {
        Some(query) => {
            let catalog = mm2_content::VehicleCatalog::scan(&vfs);
            vec![catalog.find(query)?.id.clone()]
        }
        None => mm2_content::VehicleCatalog::scan(&vfs)
            .entries
            .iter()
            .filter(|e| e.is_ready())
            .map(|e| e.id.clone())
            .collect(),
    };
    if ids.is_empty() {
        return Err("no ready vehicles to audit".into());
    }

    println!(
        "{:<14} {:<30} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7}",
        "id",
        "name",
        "mass",
        "com_h",
        "tip_g",
        "grip_g",
        "margin",
        "ride",
        "appr",
        "dep",
        "brkovr",
        "hz",
        "steer_g",
    );
    let mut flagged: Vec<(String, Vec<String>)> = Vec::new();
    for id in &ids {
        let def = match mm2_content::load_vehicle(&vfs, id, 0) {
            Ok(def) => def,
            Err(e) => {
                println!("{id:<14} {:<30} load failed: {e}", "-");
                flagged.push((id.clone(), vec![format!("load failed: {e}")]));
                continue;
            }
        };
        let m = mm2_vehicle::HandlingMetrics::of(&def.config);
        let hz = m.wheels.first().map(|w| w.natural_frequency).unwrap_or(0.0);
        println!(
            "{:<14} {:<30} {:>6.0} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>6.0} {:>6.0} {:>6.0} {:>6.2} {:>7.1}",
            def.id,
            truncate(&def.display_name, 30),
            def.config.mass,
            m.com_height,
            m.tip_threshold_g,
            m.peak_lateral_g,
            m.rollover_margin,
            m.belly_clearance,
            m.approach_angle.to_degrees(),
            m.departure_angle.to_degrees(),
            m.breakover_angle.to_degrees(),
            hz,
            m.high_speed_steer_demand_g,
        );
        let problems = m.problems();
        if !problems.is_empty() {
            flagged.push((def.id.clone(), problems));
        }
    }

    if flagged.is_empty() {
        println!("\nall {} vehicle(s) within the arcade envelope", ids.len());
        return Ok(());
    }
    println!("\n== problems ==");
    for (id, problems) in &flagged {
        for p in problems {
            println!("{id}: {p}");
        }
    }
    println!("\n{} of {} vehicle(s) flagged", flagged.len(), ids.len());
    if strict {
        return Err("handling audit found problems".into());
    }
    Ok(())
}

/// Clip `s` to `max` characters for table output.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max.saturating_sub(1)).collect::<String>() + "\u{2026}"
}

fn validate_cars(
    dir: &Path,
    mods: Option<&Path>,
    all: bool,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let catalog = mm2_content::VehicleCatalog::scan(&vfs);
    if catalog.entries.is_empty() {
        return Err("vehicle catalog is empty — cannot validate".into());
    }

    let ids: Vec<String> = if all {
        catalog.entries.iter().map(|e| e.id.clone()).collect()
    } else {
        mm2_content::EXPECTED_STOCK_ROSTER
            .iter()
            .map(|s| s.to_string())
            .collect()
    };

    let mut ok = 0usize;
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut warned: Vec<String> = Vec::new();
    println!(
        "{:<14} {:<6} {:<7} {:<6} {:<5} result",
        "id", "wheels", "paints", "parts", "mass"
    );
    for id in &ids {
        match catalog.entries.iter().find(|e| &e.id == id) {
            None => {
                failed.push((id.clone(), "not discovered in catalog".into()));
                println!(
                    "{id:<14} {:<6} {:<7} {:<6} {:<5} FAIL not discovered",
                    "-", "-", "-", "-"
                );
            }
            Some(entry) => {
                let paints_declared = entry.paints.len();
                match mm2_content::load_vehicle(&vfs, id, 0) {
                    Ok(def) => {
                        let mut problems = paint_coverage(&def);
                        problems.extend(def.report.warnings.iter().cloned());
                        problems.extend(def.model.warnings.iter().cloned());
                        let wheel_vis = def.model.wheels.iter().filter(|w| !w.trailer).count();
                        let status = if problems.is_empty() {
                            "ok".to_string()
                        } else {
                            warned.push(id.clone());
                            format!("ok (+{} warnings)", problems.len())
                        };
                        ok += 1;
                        println!(
                            "{:<14} {:<6} {:<7} {:<6} {:<5.0} {}",
                            id,
                            wheel_vis,
                            format!("{}/{}", paints_declared, def.model.paint_jobs),
                            def.model.parts.len(),
                            def.config.mass,
                            status
                        );
                        for p in &problems {
                            println!("    warn: {p}");
                        }
                    }
                    Err(e) => {
                        failed.push((id.clone(), e.to_string()));
                        println!(
                            "{id:<14} {:<6} {:<7} {:<6} {:<5} FAIL {e}",
                            "-", "-", "-", "-"
                        );
                    }
                }
            }
        }
    }

    let expected = mm2_content::EXPECTED_STOCK_ROSTER.len();
    println!(
        "\nexpected stock: {expected}, validated: {ok}, failed: {}, warnings on: {}",
        failed.len(),
        warned.len()
    );
    for f in catalog.stock_audit_failures() {
        println!("  audit: {f}");
    }
    if !failed.is_empty() {
        return Err(format!("{} vehicle(s) failed validation", failed.len()).into());
    }
    if strict && !warned.is_empty() {
        return Err(format!("strict: {} vehicle(s) carry warnings", warned.len()).into());
    }
    Ok(())
}

fn inventory_cmd(
    dir: &Path,
    mods: Option<&Path>,
    json: bool,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let (vfs, mount) = build_vfs_report(dir, mods)?;
    let report = inventory::build(&vfs, &mount, dir)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&inventory::to_json(&report))?
        );
    } else {
        inventory::print(&report);
    }
    let failures = inventory::strict_failures(&report);
    if strict && !failures.is_empty() {
        return Err(format!("strict inventory: {} finding(s)", failures.len()).into());
    }
    Ok(())
}

/// Add provenance context to a format error.
fn attach(r: &mm2_assets::Resolved, e: FormatError) -> String {
    format!(
        "failed to parse {}\n  source: {} ({})\n  reason: {e}",
        r.logical,
        r.source.path.display(),
        r.source
            .archive_offset
            .map(|o| format!("offset {o:#x}"))
            .unwrap_or_else(|| "loose".to_string())
    )
}
