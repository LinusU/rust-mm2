//! `mm2-inspect`: command line inspector for MM2 archives and assets.
//!
//! Uses the same `mm2_assets`/`mm2_formats` crates as the game.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use mm2_assets::{AssetsError, Vfs, priority};
use mm2_formats::pkg::{Pkg, PkgChunk};
use mm2_formats::psdl::Psdl;
use mm2_formats::tex::TexFile;
use mm2_formats::{FormatError, inst};

/// Original archive names, in mounting order. The exact precedence the
/// original engine used is still an open question (`docs/research/dave.md`),
/// so the list is a default, not an assumption baked into the VFS.
const DEFAULT_ARCHIVES: &[&str] = &["mm2aud.ar", "mm2audex.ar", "mm2tex.ar", "mm2core.ar"];

#[derive(Parser)]
#[command(name = "mm2-inspect", about = "Inspect MM2 installations and assets")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan an MM2 installation: count files, formats and parse failures.
    Scan {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Scan { dir } => scan(&dir),
        Command::List { dir, prefix } => list(&dir, prefix.as_deref()),
        Command::Resolve { dir, logical } => resolve(&dir, &logical),
        Command::Tex { dir, logical } => tex(&dir, &logical),
        Command::Pkg { dir, logical } => pkg(&dir, &logical),
        Command::Psdl { dir, logical } => psdl(&dir, &logical),
    }
}

/// Build a VFS over an MM2 install: archives lowest, loose files above.
fn build_vfs(dir: &Path) -> Result<Vfs, AssetsError> {
    let mut vfs = Vfs::new();
    vfs.mount_archives(dir, DEFAULT_ARCHIVES, priority::ARCHIVE)?;
    vfs.mount_dir(dir, priority::LOOSE)?;
    Ok(vfs)
}

fn scan(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir)?;
    let paths = vfs.list();
    let mut by_ext: BTreeMap<String, usize> = BTreeMap::new();
    let mut recognized = 0usize;
    let mut failures: Vec<(String, String)> = Vec::new();
    for logical in &paths {
        let ext = logical.rsplit('.').next().unwrap_or("").to_string();
        *by_ext.entry(ext.clone()).or_default() += 1;
        if matches!(
            vfs.read_logical(logical),
            Ok(b) if b.is_empty()
        ) {
            continue; // zero-length entries occur in retail archives
        }
        let parsed: Option<Result<(), String>> = match ext.as_str() {
            // .tga is a different format entirely; only .tex is TEX.
            "tex" => Some(
                vfs.read_logical(logical)
                    .map_err(|e| e.to_string())
                    .and_then(|b| TexFile::parse(&b).map(|_| ()).map_err(|e| e.to_string())),
            ),
            "pkg" => Some(
                vfs.read_logical(logical)
                    .map_err(|e| e.to_string())
                    .and_then(|b| Pkg::parse(&b).map(|_| ()).map_err(|e| e.to_string())),
            ),
            "psdl" => Some(
                vfs.read_logical(logical)
                    .map_err(|e| e.to_string())
                    .and_then(|b| Psdl::parse(&b).map(|_| ()).map_err(|e| e.to_string())),
            ),
            "inst" => Some(
                vfs.read_logical(logical)
                    .map_err(|e| e.to_string())
                    .and_then(|b| inst::parse(&b).map(|_| ()).map_err(|e| e.to_string())),
            ),
            _ => None,
        };
        if let Some(res) = parsed {
            match res {
                Ok(()) => recognized += 1,
                Err(e) => failures.push((logical.clone(), e)),
            }
        }
    }
    println!("== scan of {}", dir.display());
    println!("total logical files: {}", paths.len());
    println!("recognized & parsed OK: {recognized}");
    println!("parse failures: {}", failures.len());
    println!("\nby extension:");
    for (ext, count) in &by_ext {
        println!("  {ext:12} {count}");
    }
    if !failures.is_empty() {
        println!("\nfirst failures:");
        for (path, err) in failures.iter().take(20) {
            println!("  {path}: {err}");
        }
    }
    Ok(())
}

fn list(dir: &Path, prefix: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir)?;
    let prefix = prefix.map(|p| p.to_ascii_lowercase());
    for p in vfs.list() {
        if prefix.as_ref().is_none_or(|pre| p.starts_with(pre)) {
            println!("{p}");
        }
    }
    Ok(())
}

fn resolve(dir: &Path, logical: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir)?;
    match vfs.resolve(logical) {
        Some(r) => {
            println!("logical : {}", r.logical);
            println!("kind    : {:?}", r.source.kind);
            println!("source  : {}", r.source.path.display());
            if let Some(off) = r.source.archive_offset {
                println!("offset  : {off:#x}");
            }
            let bytes = vfs.read(&r)?;
            println!("size    : {} bytes", bytes.len());
        }
        None => println!("not found: {logical}"),
    }
    Ok(())
}

fn tex(dir: &Path, logical: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir)?;
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

fn pkg(dir: &Path, logical: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir)?;
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
            PkgChunk::Raw(b) => println!("  {:20} raw: {} bytes", file.name, b.len()),
        }
    }
    Ok(())
}

fn psdl(dir: &Path, logical: &str) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir)?;
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
