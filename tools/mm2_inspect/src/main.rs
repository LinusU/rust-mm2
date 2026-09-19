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
use mm2_assets::{AssetsError, InstallMount, Vfs, mount_install, mount_mods};
use mm2_formats::pkg::{Pkg, PkgChunk};
use mm2_formats::psdl::Psdl;
use mm2_formats::tex::TexFile;
use mm2_formats::{FormatError, inst};

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
    }
}

/// Build the VFS exactly like the game does: install (archives + loose
/// files) then mods.
fn build_vfs(dir: &Path, mods: Option<&Path>) -> Result<Vfs, AssetsError> {
    let mut vfs = Vfs::new();
    let report = mount_install(&mut vfs, dir, &InstallMount::default())?;
    for (path, err) in &report.skipped {
        eprintln!("skipped archive {}: {err}", path.display());
    }
    if let Some(mods) = mods {
        let manifests = mount_mods(&mut vfs, mods)?;
        for m in &manifests {
            eprintln!("mounted mod {}", m.id);
        }
    }
    Ok(vfs)
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
