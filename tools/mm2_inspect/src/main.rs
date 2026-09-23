//! `mm2-inspect`: command line inspector for MM2 archives and assets.
//!
//! Uses the same `mm2_assets` mounting policy as the game: same archive
//! discovery, source order, path normalization and mod handling.
//!
//! Exit codes: 0 = success; 2 = a requested lookup failed, a parse failed,
//! or `--strict` found failures.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mm2_assets::{AssetsError, InstallMount, MountReport, Vfs, mount_install, mount_mods};
use mm2_formats::bai::{Bai, VehicleRule};
use mm2_formats::pathset::Pathset;
use mm2_formats::pkg::{Pkg, PkgChunk};
use mm2_formats::psdl::Psdl;
use mm2_formats::tex::TexFile;
use mm2_formats::{FormatError, inst};

mod bind;
mod event;
mod inventory;
mod placement;

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
    /// Structural race-definition audit: run every cataloged event
    /// through the production `CatalogEvent → RaceDefinition` builder at
    /// both difficulties and report the per-event result.
    RaceDefs {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem (default: every discovered
        /// `race/<city>/` directory).
        #[arg(long)]
        city: Option<String>,
        /// Restrict to one event table: `checkpoint`/`race`, `blitz`,
        /// `circuit`, `crash`/`crashcourse`.
        #[arg(long)]
        table: Option<String>,
        /// Exit nonzero on an empty catalog, a table scan error, or any
        /// event the producer cannot build. Crash Course events are
        /// reported as `unsupported` (deferred scope, F21), not
        /// failures.
        #[arg(long)]
        strict: bool,
    },
    /// Inspect one selected event: resolve a `<table>:<row>` row through
    /// the production catalog and validate its whole dependency closure —
    /// every attributed record (aimap/pathset deep-parsed), the
    /// `RaceDefinition` and `OpponentRoster` builds at both difficulties,
    /// and wired vehicle ids cross-checked against the vehicle catalog.
    Event {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// City stem the event lives in (`london`, `sf`).
        #[arg(long)]
        city: String,
        /// Event reference: `<table>:<row>` — same vocabulary as
        /// `mm2 --event` (`checkpoint`/`race`, `blitz`, `circuit`,
        /// `crash`/`crashcourse`).
        #[arg(long)]
        event: String,
        /// Exit nonzero on an incomplete event, a failed record or
        /// reference, a record validation issue, a failed production
        /// build, a roster issue, or a wired vehicle id outside the
        /// catalog.
        #[arg(long)]
        strict: bool,
    },
    /// Opponent-roster audit: run every cataloged event through the
    /// production `CatalogEvent → OpponentRoster` builder at both
    /// difficulties, cross-check wired `.opp` route references and
    /// vehicle ids, and count non-event stems carrying a wired lineup.
    Opponents {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem (default: every discovered
        /// `race/<city>/` directory).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero on an empty catalog, any event the producer
        /// cannot build, any roster issue, dead route references on
        /// extra rosters, or wired vehicle ids outside the vehicle
        /// catalog.
        #[arg(long)]
        strict: bool,
    },
    /// Audit the ambient-navigation files (`city/*.bai`): parse every
    /// discovered BAI, validate internal cross-references, and cross-check
    /// room references against the matching PSDL when one resolves.
    Bai {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem (default: every `city/*.bai` plus
        /// the expected `city/<name>.bai` for each stock city).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero when an expected city file is missing or fails to
        /// parse, or when any parsed file reports validation issues.
        #[arg(long)]
        strict: bool,
    },
    /// Audit the AI-map override files (`city/*.aimap`,
    /// `race/<city>/*.aimap`/`*.aimap_p`): parse every discovered file,
    /// validate section values, cross-check exception road ids against
    /// the city's BAI, and resolve opponent `.opp` references.
    Aimap {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem (default: every discovered aimap
        /// plus the expected `city/<name>.aimap` for each stock city).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero when an expected file is missing or fails to
        /// parse, or when any parsed file reports issues.
        #[arg(long)]
        strict: bool,
    },
    /// Build the shared navigation graph (`mm2_game::nav`) for each
    /// stock city's `city/<name>.bai`: directed arcs, lanes, dead ends,
    /// connected components and build issues, with an optional directed
    /// route probe between two road indices.
    Nav {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem (default: both stock cities).
        #[arg(long)]
        city: Option<String>,
        /// Route probe `from:to` as BAI road indices; anchors each end
        /// on the road's first arc lane and honours the city aimap's
        /// road closures.
        #[arg(long)]
        route: Option<String>,
        /// Route-constraint validation: a per-arc directed reachability
        /// census plus `n` seeded route probes between routable roads,
        /// each checked for chain consistency and closed-road
        /// traversal.
        #[arg(long)]
        routes: Option<usize>,
        /// Logical `.aimap` path supplying the routing overrides instead
        /// of `city/<name>.aimap` (e.g. an event file's `[Exceptions]`).
        /// Must resolve through the VFS.
        #[arg(long)]
        aimap: Option<String>,
        /// Reconcile each exit's geometric turn classification against
        /// the authored counterclockwise road-index delta, histogrammed
        /// by intersection arity.
        #[arg(long)]
        turns: bool,
        /// Junction-interior census: per legal lane transfer, the
        /// lane-end→landing-lane-start gap a crossing must cover (the
        /// distance a direct transfer would teleport), plus the
        /// generated crossing path's length.
        #[arg(long)]
        gaps: bool,
        /// Exit nonzero when an expected graph fails to build or any
        /// issue is reported.
        #[arg(long)]
        strict: bool,
    },
    /// Audit the roadside-prop rule tables (`propdefs.csv`,
    /// `proprules.csv`, `props.csv`, `geometry/props.csv`): parse every
    /// discovered file, cross-check rule prop references against
    /// propdefs, prop PKG names against `geometry/`, and the PSDL
    /// `prop_rule` bytes against defined rule numbers.
    Proprules {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem: `city/<stem>/` files only
        /// (unaffiliated dirs like `city/phys/` belong to the full
        /// audit).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero when any expected file is missing or fails to
        /// parse, or when any issue is reported.
        #[arg(long)]
        strict: bool,
    },
    /// Audit the surface-material tables (`city/materials.mtl`,
    /// `city/materials.csv`): parse every discovered `.mtl` /
    /// `materials*.csv`, cross-check `physics` refs against defined
    /// material names, and report how each stock city's PSDL texture
    /// table maps onto materials.
    Materials {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict the PSDL texture-table cross-check to one city stem.
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero when any expected file is missing or fails to
        /// parse, or when any issue is reported.
        #[arg(long)]
        strict: bool,
    },
    /// Audit prop/decal placement pathsets (`*.pathset`, binary PTH1):
    /// parse every discovered file, validate authored consistency, and
    /// resolve each path's asset name against `geometry/` and
    /// `texture/`.
    Pathset {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem: `city/<stem>/` and
        /// `race/<stem>/` files only (unaffiliated top-level files like
        /// `city/phys/` and `city/race0.pathset` belong to the full
        /// audit).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero when any discovered file fails to parse or any
        /// issue is reported.
        #[arg(long)]
        strict: bool,
    },
    /// Audit breakable/knockable object records (`tune/banger/
    /// *.dgbangerdata`): parse every discovered file, classify each
    /// record (fallback / standalone prop / `.mtx` part / embedded
    /// `BREAK<NN>` fragment), and resolve each name against
    /// `geometry/` PKG chunks and `.mtx` transforms.
    Banger {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Exit nonzero when the expected fallback record is missing,
        /// any record fails to parse, or any issue is reported.
        #[arg(long)]
        strict: bool,
    },
    /// Audit which placement sources stamp banger-bound props (F04-A):
    /// every INST record, stamped pathset path, prop-rule def file and
    /// prop-group entry is checked for a `tune/banger/<name>`
    /// `.dgbangerdata` record; the reverse check lists banger records
    /// no audited placement ever names.
    BangerBind {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city bucket (default: every discovered
        /// placement source plus the unplaced-records reverse check).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero when any expected file is missing or fails to
        /// parse, or any placed name resolves to no geometry.
        #[arg(long)]
        strict: bool,
    },
    /// Audit lateral prop placement (F03-C): expand each channel's
    /// stamps — INST records, `props.pathset` rows, and PSDL
    /// `prop_rule` roadside stamping — and classify every position
    /// against the city's authored PSDL carriageway regions, listing
    /// stamps that land on the drivable surface.
    Placement {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem (default: both stock cities).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero on missing/failed sources or channel issues.
        /// In-road counts are reported findings — retail authors
        /// legitimately stamp onto some drivable surfaces.
        #[arg(long)]
        strict: bool,
    },
    /// Audit ambient-traffic content (F10-A.1): each city's
    /// `[Ambient Types/Density]` roster, per-class
    /// `aivehicledata`/PKG/BND resolution, ambient tune files no roster
    /// references, and every event aimap carrying ambient overrides.
    Traffic {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict to one city stem (default: both stock cities plus
        /// every discovered `city/*.aimap` stem).
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero on a missing/failed expected aimap, a class
        /// asset failure, an undiscovered expected ambient id, an
        /// unparseable event aimap, or any roster/diagnostic issue.
        #[arg(long)]
        strict: bool,
    },
    /// Audit vehicle damage content (F05-A.1): every discovered
    /// `tune/vehicle/*.{vehcardamage,vehstuck,vehgyro}` decoded through
    /// the production parsers, each catalog vehicle's coverage and
    /// authored breakaway inventory (pkg BREAK chunks vs
    /// `_break*.mtx` parts vs `_break*` banger records), plus records
    /// on ids outside the vehicle catalog.
    Damage {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Exit nonzero on a decode reject or validation issue on any
        /// discovered record. Missing records (retail's `vpmoonrover`
        /// ships no `vehcardamage`), orphaned records and dead break
        /// fragments are reported findings, not failures.
        #[arg(long)]
        strict: bool,
    },
    /// Audit weather/environment content (F18-A.1): census every
    /// `.sky`, `.ltNN`, `.ldef`, `.cpvs`, `.pvshist`, `.water` and `.lmap`
    /// file; parse each through the production decoders; cross-check the
    /// measured 16-preset lighting grid, the `amb_*`/`sky_*` ldef/texture
    /// pairs, `.sky` dome geometry and per-city data against the PSDL
    /// room table.
    Weather {
        /// Path to the MM2 installation directory.
        dir: PathBuf,
        /// Restrict the per-city expected denominator and PSDL
        /// cross-checks to one city stem (default: both stock cities).
        /// Every discovered environment file is still audited.
        #[arg(long)]
        city: Option<String>,
        /// Exit nonzero when any expected file is missing or fails to
        /// parse, or when any issue is reported.
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
        Command::RaceDefs {
            dir,
            city,
            table,
            strict,
        } => race_defs(
            dir,
            cli.mods.as_deref(),
            city.as_deref(),
            table.as_deref(),
            *strict,
        ),
        Command::Event {
            dir,
            city,
            event: spec,
            strict,
        } => event::run(dir, cli.mods.as_deref(), city, spec, *strict),
        Command::Opponents { dir, city, strict } => {
            opponents(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Bai { dir, city, strict } => {
            bai(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Aimap { dir, city, strict } => {
            aimap(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Nav {
            dir,
            city,
            route,
            routes,
            aimap,
            turns,
            gaps,
            strict,
        } => nav(
            dir,
            cli.mods.as_deref(),
            city.as_deref(),
            &NavOptions {
                route: route.as_deref(),
                routes: *routes,
                aimap: aimap.as_deref(),
                turns: *turns,
                gaps: *gaps,
                strict: *strict,
            },
        ),
        Command::Proprules { dir, city, strict } => {
            proprules(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Materials { dir, city, strict } => {
            materials(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Pathset { dir, city, strict } => {
            pathset(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Banger { dir, strict } => banger(dir, cli.mods.as_deref(), *strict),
        Command::BangerBind { dir, city, strict } => {
            bind::banger_bind(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Placement { dir, city, strict } => {
            placement::placement(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Traffic { dir, city, strict } => {
            traffic(dir, cli.mods.as_deref(), city.as_deref(), *strict)
        }
        Command::Damage { dir, strict } => damage(dir, cli.mods.as_deref(), *strict),
        Command::Weather { dir, city, strict } => {
            weather(dir, cli.mods.as_deref(), city.as_deref(), *strict)
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
            "bai" => match Bai::parse(&bytes) {
                Ok(_) => stats.parsed_ok += 1,
                Err(e) => stats.failures.push((logical.clone(), e.to_string())),
            },
            "pathset" => match Pathset::parse(&bytes) {
                Ok(_) => stats.parsed_ok += 1,
                Err(e) => stats.failures.push((logical.clone(), e.to_string())),
            },
            "aimap" | "aimap_p" => match std::str::from_utf8(&bytes)
                .map_err(|e| FormatError::parse(0, format!("not UTF-8 text: {e}")))
                .and_then(mm2_formats::aimap::Aimap::parse)
            {
                Ok(_) => stats.parsed_ok += 1,
                Err(e) => stats.failures.push((logical.clone(), e.to_string())),
            },
            "mtl" => match std::str::from_utf8(&bytes)
                .map_err(|e| FormatError::parse(0, format!("not UTF-8 text: {e}")))
                .and_then(mm2_formats::materials::MaterialSet::parse)
            {
                Ok(_) => stats.parsed_ok += 1,
                Err(e) => stats.failures.push((logical.clone(), e.to_string())),
            },
            "dgbangerdata" => match std::str::from_utf8(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|t| mm2_formats::banger::BangerData::parse(t).map_err(|e| e.to_string()))
            {
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
    let garage = mm2_content::scan_garage(&vfs);
    println!("== vehicle roster ({} entries) ==", catalog.entries.len());
    println!(
        "{:<14} {:<30} {:<8} {:<8} {:<7} {:<10} status",
        "id", "name", "class", "gate", "paints", "unlock s/f"
    );
    for e in &catalog.entries {
        let class = match e.class {
            mm2_content::VehicleClass::Stock => "stock",
            mm2_content::VehicleClass::Mod => "mod",
            mm2_content::VehicleClass::ModOnly => "mod-only",
        };
        let (gate, gated_paints) = match garage.row(&e.id) {
            Some(row) => (
                if !row.listed {
                    "unlisted".to_string()
                } else {
                    match row.gate {
                        mm2_game::VehicleGate::Open => "open".to_string(),
                        mm2_game::VehicleGate::Reward => "reward".to_string(),
                    }
                },
                row.paint_gates
                    .iter()
                    .filter(|g| **g == mm2_game::PaintGate::Reward)
                    .count(),
            ),
            None => ("?".to_string(), 0),
        };
        let paints = if gated_paints > 0 {
            format!("{}+{gated_paints}g", e.paints.len() - gated_paints)
        } else {
            e.paints.len().to_string()
        };
        // The authored .info fields, verbatim — audit context for
        // UNK-6, not a gate (nonzero values do not correlate with the
        // reward-locked set).
        let unlock_sf = if e.unlock_score > 0 || e.unlock_flags > 0 {
            format!("{}/{}", e.unlock_score, e.unlock_flags)
        } else {
            "-".to_string()
        };
        let status = match &e.status {
            mm2_content::EntryStatus::Ready => "ready".to_string(),
            mm2_content::EntryStatus::Incomplete { missing } => {
                format!("incomplete: {}", missing.join(", "))
            }
        };
        println!(
            "{:<14} {:<30} {:<8} {:<8} {:<7} {:<10} {}",
            e.id, e.display_name, class, gate, paints, unlock_sf, status
        );
    }
    for d in &garage.diagnostics {
        println!("  garage diagnostic: {d}");
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
        C::Waypoints(f) => format!("wp:{}({})", f.rows.len(), f.width_label),
        C::StartPoints(f) => format!("strtpnts:{}", f.rows.len()),
        C::Opp(f) => format!("opp{}:{}", r.difficulty.unwrap_or('-'), f.rows.len()),
        C::CrashData(f) => format!("data:{}", f.rows.len()),
        C::Unparsed => match r.kind {
            K::Aimap => "aimap".into(),
            K::AimapP => "aimap_p".into(),
            K::Pathset => "pathset".into(),
            other => format!("{other:?}"),
        },
        C::Failed(_) => format!("!{:?}", r.kind),
    }
}

/// Which cities to scan: the explicit one, or every `race/<city>/`
/// directory discovered (stock + any mod-provided cities).
fn race_cities(vfs: &Vfs, city: Option<&str>) -> Vec<String> {
    match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => mm2_content::race_cities(vfs),
    }
}

fn events(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let cities = race_cities(&vfs, city);

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
        // F16-B: the derived availability surface — which rows a fresh
        // profile may select and what each gated row needs beaten
        // (CHK-2/CHK-3 sets of three, CC-3 lesson→midterm→final).
        let availability = mm2_content::availability_table(&cat);
        {
            let gated: Vec<_> = availability
                .rows
                .iter()
                .filter(|r| !matches!(r.gate, mm2_game::EventGate::Open))
                .collect();
            println!(
                "  availability: {} open / {} gated, {} diagnostics",
                availability.rows.len() - gated.len(),
                gated.len(),
                availability.diagnostics.len(),
            );
            for row in &gated {
                let mm2_game::EventGate::AfterAll(reqs) = &row.gate else {
                    continue;
                };
                println!(
                    "    {} ← beat {}",
                    row.key.stem,
                    reqs.iter()
                        .map(|k| k.stem.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            for d in &availability.diagnostics {
                println!("    availability note: {d}");
            }
        }
        // F16-B: the normalized session reward table — every authored
        // row becomes an event-bound or milestone rule, or a named
        // diagnostic (AC05: no silently dropped unlock rule). The
        // authored-row count is printed against the accounted total so
        // the audit states its denominator, and a city whose rows are
        // *all* diagnostics still reports them.
        let rewards = mm2_content::reward_table(&cat);
        let authored_rows =
            cat.events.iter().map(|e| e.rewards.len()).sum::<usize>() + cat.milestone_rewards.len();
        let accounted =
            rewards.per_event.len() + rewards.milestones.len() + rewards.diagnostics.len();
        if authored_rows > 0 || accounted > 0 {
            let sizes = rewards
                .family_sizes
                .iter()
                .map(|(t, n)| format!("{t:?}={n}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "  reward table: {authored_rows} authored rows → {} event-bound + {} milestone rules ({}), {} diagnostics",
                rewards.per_event.len(),
                rewards.milestones.len(),
                sizes,
                rewards.diagnostics.len(),
            );
            if accounted != authored_rows {
                failures.push(format!(
                    "{city}: reward coverage lost rows ({authored_rows} authored, {accounted} accounted)"
                ));
            }
            for d in &rewards.diagnostics {
                println!("    reward note: {d}");
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

/// `--table` filter vocabulary — same names `mm2 --event` accepts.
fn parse_table_filter(s: &str) -> Result<mm2_game::EventTableKind, String> {
    match s.to_ascii_lowercase().as_str() {
        "checkpoint" | "race" => Ok(mm2_game::EventTableKind::Checkpoint),
        "blitz" => Ok(mm2_game::EventTableKind::Blitz),
        "circuit" => Ok(mm2_game::EventTableKind::Circuit),
        "crash" | "crashcourse" => Ok(mm2_game::EventTableKind::CrashCourse),
        other => Err(format!(
            "unknown table {other:?}: expected checkpoint|race, blitz, circuit, crash|crashcourse"
        )),
    }
}

/// Compact one-cell description of a per-difficulty build outcome.
fn describe_build(build: &mm2_content::RaceDefBuild) -> String {
    use mm2_content::RaceDefBuild as B;
    match build {
        B::Built(s) => {
            let mut d = format!("{}g", s.gates);
            if s.finish {
                d.push_str("+fin");
            }
            if s.laps > 0 {
                d.push_str(&format!("x{}lap", s.laps));
            }
            if let Some(t) = s.time_limit_ticks {
                d.push_str(&format!(
                    " {:.1}s",
                    t as f32 / mm2_game::RACE_TICK_HZ as f32
                ));
            }
            d.push_str(&format!(" {}slt", s.start_slots));
            if s.opponents > 0 || s.cops > 0 {
                d.push_str(&format!(" {}opp/{}cop", s.opponents, s.cops));
            }
            d.push_str(&format!(" tod{}/w{}", s.time_of_day, s.weather));
            format!("ok: {d}")
        }
        B::Unsupported => "unsupported (crash course — F21)".to_string(),
        B::Failed(mm2_content::RaceBuildError::NotReady(
            mm2_content::EventStatus::Incomplete { missing },
        )) => format!("incomplete ({})", missing.join(", ")),
        B::Failed(e) => format!("failed: {e}"),
    }
}

/// The complete-catalog structural check (F12-C): every event's
/// authored objectives must convert into a validated `RaceDefinition`
/// at both difficulties through the production builder — the same one
/// the session loader calls.
fn race_defs(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    table: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let cities = race_cities(&vfs, city);
    let filter = table.map(parse_table_filter).transpose()?;

    let mut failures: Vec<String> = Vec::new();
    for city in &cities {
        let report = mm2_content::RaceDefReport::scan(&vfs, city);
        println!("== race definitions: {city} ==");
        for e in &report.table_errors {
            println!("  table error: {e}");
            failures.push(format!("{city}: {e}"));
        }
        for entry in &report.entries {
            if filter.is_some_and(|f| entry.event_ref.table != f) {
                continue;
            }
            println!(
                "  {:<10} {:>2} {:<12} am: {:<58} pro: {}",
                format!("{:?}", entry.event_ref.table).to_lowercase(),
                entry.event_ref.index,
                entry.stem,
                describe_build(&entry.amateur),
                describe_build(&entry.professional),
            );
        }
        println!(
            "  {city}: {} events — {} built, {} unsupported, {} failed builds across {} events",
            report.entries.len(),
            report.built(),
            report.unsupported(),
            report.failed(),
            report.failed_events(),
        );
        if report.entries.is_empty() {
            failures.push(format!("{city}: event catalog is empty"));
        }
        if report.failed() > 0 {
            failures.push(format!(
                "{city}: {} failed build(s) across {} event(s)",
                report.failed(),
                report.failed_events(),
            ));
        }
        println!();
    }
    if strict && !failures.is_empty() {
        return Err(format!("strict race-defs audit: {} failures", failures.len()).into());
    }
    Ok(())
}

/// Compact one-cell description of a per-difficulty roster build.
fn describe_roster(build: &mm2_content::RosterBuild) -> String {
    use mm2_content::RosterBuild as B;
    match build {
        B::Built(s) => {
            let mut d = format!("{}opp", s.wired);
            if s.table_opponents != s.wired as i64 {
                d.push_str(&format!("/{}tbl", s.table_opponents));
            }
            d.push_str(&format!(" {}rt", s.routes));
            if !s.vehicles.is_empty() {
                d.push_str(&format!(" [{}]", s.vehicles.join(",")));
            }
            if s.issues.is_empty() {
                format!("ok: {d}")
            } else {
                format!("ok: {d} — {} issue(s)", s.issues.len())
            }
        }
        B::Unsupported => "unsupported (crash course — F21)".to_string(),
        B::Failed(mm2_content::RosterBuildError::NotReady(
            mm2_content::EventStatus::Incomplete { missing },
        )) => format!("incomplete ({})", missing.join(", ")),
        B::Failed(e) => format!("failed: {e}"),
    }
}

/// Opponent-roster audit (F15-A.1): every cataloged event's
/// difficulty-selected `[Opponent]` lineup runs through the production
/// `CatalogEvent → OpponentRoster` producer — wired count vs the table
/// row's authored `Opponents`, `.opp` route resolution, difficulty-tag
/// consistency and unreferenced route records. Non-event stems whose
/// aimaps still wire a lineup are counted as extra rosters, and every
/// wired vehicle id is checked against the vehicle catalog.
fn opponents(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let cities = race_cities(&vfs, city);

    let mut failures: Vec<String> = Vec::new();
    for city in &cities {
        let report = mm2_content::OpponentReport::scan(&vfs, city);
        println!("== opponent rosters: {city} ==");
        if report.entries.is_empty() {
            println!("  (no authored events cataloged)");
            failures.push(format!("{city}: event catalog is empty"));
        }
        for entry in &report.entries {
            println!(
                "  {:<10} {:>2} {:<12} am: {:<44} pro: {}",
                format!("{:?}", entry.event_ref.table).to_lowercase(),
                entry.event_ref.index,
                entry.stem,
                describe_roster(&entry.amateur),
                describe_roster(&entry.professional),
            );
            for build in [&entry.amateur, &entry.professional] {
                if let mm2_content::RosterBuild::Built(s) = build {
                    for issue in &s.issues {
                        println!("       issue: {issue}");
                        failures.push(format!("{city}: {} — {issue}", entry.stem));
                    }
                }
            }
            if matches!(entry.amateur, mm2_content::RosterBuild::Failed(_))
                || matches!(entry.professional, mm2_content::RosterBuild::Failed(_))
            {
                failures.push(format!("{city}: {} — roster build failed", entry.stem));
            }
        }
        for x in &report.extra_rosters {
            println!(
                "  extra: {:<44} {x_wired} wired{dead}",
                x.logical,
                x_wired = x.wired,
                dead = if x.dead_refs > 0 {
                    format!(", {} dead route ref(s)", x.dead_refs)
                } else {
                    String::new()
                },
            );
            if x.dead_refs > 0 {
                failures.push(format!(
                    "{city}: {} — {} dead route ref(s)",
                    x.logical, x.dead_refs
                ));
            }
        }
        for v in &report.unresolved_vehicles {
            println!("  unresolved vehicle id: {v}");
            failures.push(format!(
                "{city}: wired vehicle {v} is not in the vehicle catalog"
            ));
        }
        println!(
            "  {city}: {} events — {} built ({} opponents wired), {} unsupported, {} failed, {} issue(s)",
            report.entries.len(),
            report.built(),
            report.wired(),
            report.unsupported(),
            report.failed(),
            report.issues(),
        );
        if report.failed() > 0 {
            failures.push(format!(
                "{city}: {} failed roster build(s)",
                report.failed()
            ));
        }
        println!();
    }
    if strict && !failures.is_empty() {
        return Err(format!("strict opponents audit: {} failures", failures.len()).into());
    }
    Ok(())
}

/// Ambient-traffic audit (F10-A.1): each city's `[Ambient
/// Types/Density]` roster through the production
/// `mm2_content::ambient_roster` producer, per-class asset resolution
/// (`aivehicledata` decode, `geometry/<id>.pkg`, `bound/<id>_bound.bnd`,
/// `.mtx` parts), ambient tune files no roster references, expected
/// stock ids never discovered, and every event aimap carrying
/// ambient-relevant overrides.
fn traffic(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    // The expected denominator is both stock cities plus every
    // discovered `city/*.aimap` stem (mod cities included).
    let cities: Vec<String> = match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => {
            let mut set: BTreeSet<String> = mm2_content::EXPECTED_CITIES
                .iter()
                .map(|c| c.to_string())
                .collect();
            for p in vfs.list() {
                if let Some(stem) = p
                    .strip_prefix("city/")
                    .and_then(|s| s.strip_suffix(".aimap"))
                    && !stem.contains('/')
                {
                    set.insert(stem.to_string());
                }
            }
            set.into_iter().collect()
        }
    };

    let mut failures: Vec<String> = Vec::new();
    for city in &cities {
        let audit = mm2_content::TrafficAudit::scan(&vfs, city);
        println!("== ambient traffic: {city} ==");
        if !audit.aimap_present {
            println!("  city/{city}.aimap: not resolved");
        } else if let Some(e) = &audit.aimap_error {
            println!("  city/{city}.aimap: {e}");
        }
        if let Some(roster) = &audit.roster {
            println!("  roster ({} rows):", roster.entries.len());
            for (i, e) in roster.entries.iter().enumerate() {
                let tuning = if e.tuning.is_some() {
                    "ok"
                } else {
                    "NO TUNING"
                };
                println!(
                    "    [{:>2}] {:<24} cum {:>5.2} flag {} {tuning}",
                    i, e.id, e.cumulative_weight, e.flag
                );
            }
            for issue in &roster.issues {
                println!("    issue: {issue}");
            }
        }
        for a in &audit.assets {
            let flag = |c: &mm2_content::AssetCheck| match c {
                mm2_content::AssetCheck::Parsed => "ok".to_string(),
                mm2_content::AssetCheck::Missing => "MISSING".to_string(),
                mm2_content::AssetCheck::Failed(e) => format!("FAILED {e}"),
            };
            println!(
                "  {:<24} tune:{} pkg:{} bnd:{} mtx:{}",
                a.id,
                flag(&a.tuning),
                flag(&a.geometry),
                flag(&a.bound),
                a.mtx_parts
            );
            for w in &a.tuning_warnings {
                println!("      warning: {w}");
            }
        }
        if !audit.unrostered.is_empty() {
            println!(
                "  unrostered ambient tunes: {}",
                audit.unrostered.join(", ")
            );
        }
        if !audit.missing_expected.is_empty() {
            println!(
                "  expected stock ambients not discovered: {}",
                audit.missing_expected.join(", ")
            );
        }
        if !audit.event_overrides.is_empty() {
            println!(
                "  event overrides ({}): {} with exceptions, {} with density, {} with ambient rosters",
                audit.event_overrides.len(),
                audit
                    .event_overrides
                    .iter()
                    .filter(|o| o.exceptions > 0)
                    .count(),
                audit
                    .event_overrides
                    .iter()
                    .filter(|o| o.density.is_some())
                    .count(),
                audit
                    .event_overrides
                    .iter()
                    .filter(|o| o.ambient_types > 0)
                    .count(),
            );
            for o in audit
                .event_overrides
                .iter()
                .filter(|o| o.density.is_some() || o.ambient_types > 0 || o.failed.is_some())
            {
                println!(
                    "    {:<40} exc {} amb {} {}",
                    o.logical,
                    o.exceptions,
                    o.ambient_types,
                    o.failed.as_deref().unwrap_or("")
                );
            }
        }
        for d in &audit.diagnostics {
            println!("  diagnostic: {d}");
        }
        let city_failures = audit.failures();
        println!(
            "  {city}: expected {} ambients — {} discovered, {} rostered, {} unrostered, {} failed check(s), {} diagnostic(s)",
            audit.expected(),
            audit.discovered(),
            audit.assets.len(),
            audit.unrostered.len(),
            city_failures.len(),
            audit.diagnostics.len(),
        );
        failures.extend(city_failures.into_iter().map(|f| format!("{city}: {f}")));
        println!();
    }
    if strict && !failures.is_empty() {
        for f in &failures {
            eprintln!("  strict: {f}");
        }
        return Err(format!("strict traffic audit: {} failures", failures.len()).into());
    }
    Ok(())
}

/// Vehicle-damage audit (F05-A.1): every discovered
/// `tune/vehicle/*.{vehcardamage,vehstuck,vehgyro}` through
/// [`mm2_content::DamageAudit`], per-vehicle coverage and breakaway
/// inventory, uncatalogued records kept in the denominator.
fn damage(dir: &Path, mods: Option<&Path>, strict: bool) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let audit = mm2_content::DamageAudit::scan(&vfs);

    println!("== vehicle damage (F05-A.1) ==");
    let flag = |c: &mm2_content::AssetCheck| match c {
        mm2_content::AssetCheck::Parsed => "ok".to_string(),
        mm2_content::AssetCheck::Missing => "—".to_string(),
        mm2_content::AssetCheck::Failed(_) => "FAILED".to_string(),
    };
    for a in &audit.assets {
        let breaks = match &a.break_chunks {
            Some(c) => c.join(","),
            None => "pkg?".to_string(),
        };
        println!(
            "  {:<14} damage:{} stuck:{} gyro:{} break[pkg:{} mtx:{} rec:{}]",
            a.id,
            flag(&a.damage),
            flag(&a.stuck),
            flag(&a.gyro),
            breaks,
            a.break_mtx.join(","),
            a.break_records.join(","),
        );
        for d in &a.dead_breaks {
            println!("      dead fragment: {d}");
        }
        for w in a.warnings.iter().chain(&a.issues) {
            println!("      {w}");
        }
    }
    if !audit.uncatalogued.is_empty() {
        println!("  uncatalogued records ({}):", audit.uncatalogued.len());
        for r in &audit.uncatalogued {
            println!(
                "    {:<48} {}",
                r.logical,
                match &r.status {
                    mm2_content::AssetCheck::Parsed => "ok".to_string(),
                    mm2_content::AssetCheck::Missing => "missing".to_string(),
                    mm2_content::AssetCheck::Failed(e) => format!("FAILED {e}"),
                }
            );
            for w in r.warnings.iter().chain(&r.issues) {
                println!("      {w}");
            }
        }
    }
    for d in &audit.diagnostics {
        println!("  diagnostic: {d}");
    }

    let parsed = audit
        .assets
        .iter()
        .flat_map(|a| [&a.damage, &a.stuck, &a.gyro])
        .filter(|c| **c == mm2_content::AssetCheck::Parsed)
        .count()
        + audit
            .uncatalogued
            .iter()
            .filter(|r| r.status == mm2_content::AssetCheck::Parsed)
            .count();
    let failures = audit.failures();
    let with_breaks = audit
        .assets
        .iter()
        .filter(|a| {
            !a.break_records.is_empty()
                || a.break_chunks.as_ref().is_some_and(|c| !c.is_empty())
                || !a.break_mtx.is_empty()
        })
        .count();
    println!(
        "  {} catalog vehicles — {} carry damage records, {parsed} records parsed, \
         {with_breaks} with breakaway parts, {} uncatalogued record(s), \
         {} failed check(s), {} diagnostic(s)",
        audit.assets.len(),
        audit.discovered(),
        audit.uncatalogued.len(),
        failures.len(),
        audit.diagnostics.len(),
    );
    if strict && !failures.is_empty() {
        for f in &failures {
            eprintln!("  strict: {f}");
        }
        return Err(format!("strict damage audit: {} failures", failures.len()).into());
    }
    Ok(())
}

/// Ambient-navigation audit (F09-A): every discovered `city/*.bai` is
/// parsed through `mm2_formats::bai`, internal cross-references are
/// validated by `Bai::validate`, and — when a same-stem `city/<stem>.psdl`
/// resolves — room references and the culling room count are checked
/// against it. `city/<name>.bai` for each stock city is the expected
/// denominator; other discovered files are audited extras whose parse
/// failures are reported as `unsupported`, not hidden.
fn bai(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;

    let mut expected: Vec<String> = match city {
        Some(c) => vec![format!("city/{}.bai", c.to_ascii_lowercase())],
        None => mm2_content::EXPECTED_CITIES
            .iter()
            .map(|c| format!("city/{c}.bai"))
            .collect(),
    };
    expected.sort();
    let prefix = city.map(|c| format!("city/{}", c.to_ascii_lowercase()));
    let mut extras: Vec<String> = vfs
        .list()
        .into_iter()
        .filter(|p| {
            p.starts_with("city/")
                && p.ends_with(".bai")
                && !expected.contains(p)
                && prefix.as_ref().is_none_or(|pre| p.starts_with(pre))
        })
        .collect();
    extras.sort();

    let mut failures: Vec<String> = Vec::new();
    let mut issues_total = 0usize;
    let mut parsed = 0usize;
    let mut unsupported = 0usize;

    println!("== ambient navigation (BAI) ==");
    for logical in expected.iter().chain(extras.iter()) {
        let is_expected = expected.contains(logical);
        let tag = if is_expected { "expected" } else { "extra" };
        let Some(res) = vfs.resolve(logical) else {
            println!("  {logical:<22} {tag:<9} missing");
            failures.push(format!("{logical}: expected file not found"));
            continue;
        };
        let bytes = vfs.read(&res)?;
        let bai = match Bai::parse(&bytes) {
            Ok(b) => b,
            Err(e) => {
                if is_expected {
                    println!("  {logical:<22} {tag:<9} failed: {e}");
                    failures.push(format!("{logical}: {e}"));
                } else {
                    println!("  {logical:<22} {tag:<9} unsupported: {e}");
                    unsupported += 1;
                }
                continue;
            }
        };
        parsed += 1;
        let issues = bai.validate();
        issues_total += issues.len();
        let cull_rooms = bai.culling.large.len();
        println!(
            "  {logical:<22} {tag:<9} ok — {} roads, {} intersections, culling {} rooms",
            bai.roads.len(),
            bai.intersections.len(),
            cull_rooms,
        );
        for issue in &issues {
            println!("    issue: {issue}");
        }

        // Cross-check room references when a same-stem PSDL resolves.
        let stem = logical.trim_start_matches("city/").trim_end_matches(".bai");
        let psdl_path = format!("city/{stem}.psdl");
        match vfs.resolve(&psdl_path) {
            Some(pres) => {
                let pbytes = vfs.read(&pres)?;
                match Psdl::parse(&pbytes) {
                    Ok(psdl) => {
                        let rooms = psdl.rooms.len();
                        let mut bad = Vec::new();
                        if cull_rooms != rooms + 1 {
                            bad.push(format!(
                                "culling covers {cull_rooms} rooms, psdl has {rooms} (+1)"
                            ));
                        }
                        let road_oor = bai
                            .roads
                            .iter()
                            .flat_map(|r| r.rooms.iter())
                            .filter(|&&r| r == 0 || r as usize > rooms)
                            .count();
                        if road_oor > 0 {
                            bad.push(format!("{road_oor} road room ref(s) out of range"));
                        }
                        let int_oor = bai
                            .intersections
                            .iter()
                            .filter(|i| i.room == 0 || i.room as usize > rooms)
                            .count();
                        if int_oor > 0 {
                            bad.push(format!("{int_oor} intersection room ref(s) out of range"));
                        }
                        if bad.is_empty() {
                            println!("    rooms: all refs within {psdl_path} ({rooms} rooms)");
                        } else {
                            for b in &bad {
                                println!("    issue: {b} (vs {psdl_path})");
                                issues_total += 1;
                            }
                        }
                    }
                    Err(e) => println!("    note: {psdl_path} failed to parse: {e}"),
                }
            }
            None => println!("    note: no {psdl_path} — room refs not cross-checked"),
        }

        // Traffic-light census (F10-B signal rendering): ends carrying a
        // nonzero `traffic_light_origin`, split by the end's authored
        // `vehicleRule` — R3 notes lights render only when the origin is
        // nonzero. Non-finite origins/axes are counted, not hidden.
        let (mut lit, mut lit_other, mut nonfinite) = (0usize, 0usize, 0usize);
        let (mut y_min, mut y_max) = (f32::MAX, f32::MIN);
        let (mut d_max, mut far, mut unconn) = (0.0f32, 0usize, 0usize);
        let mut sample: Option<([f32; 3], [f32; 3])> = None;
        for road in &bai.roads {
            for end in [&road.end, &road.start] {
                if !end.traffic_light_origin.iter().all(|c| c.is_finite())
                    || !end.traffic_light_axis.iter().all(|c| c.is_finite())
                {
                    nonfinite += 1;
                } else if end.traffic_light_origin.iter().any(|c| *c != 0.0) {
                    if end.vehicle_rule() == Some(VehicleRule::TrafficLight) {
                        lit += 1;
                    } else {
                        lit_other += 1;
                    }
                    y_min = y_min.min(end.traffic_light_origin[1]);
                    y_max = y_max.max(end.traffic_light_origin[1]);
                    if end.is_connected()
                        && let Some(ix) = bai.intersections.get(end.intersection as usize)
                    {
                        let dx = end.traffic_light_origin[0] - ix.center[0];
                        let dz = end.traffic_light_origin[2] - ix.center[2];
                        let d = (dx * dx + dz * dz).sqrt();
                        d_max = d_max.max(d);
                        if d > 60.0 {
                            far += 1;
                        }
                        if d <= 60.0 && sample.is_none() {
                            sample = Some((end.traffic_light_origin, ix.center));
                        }
                    } else {
                        unconn += 1;
                    }
                }
            }
        }
        println!(
            "    lights: {lit} light-ruled ends with origins, \
             {lit_other} origins on other-rule ends, {nonfinite} non-finite, \
             y {y_min:.1}..{y_max:.1}, {far} beyond 60 m of centre \
             (max {d_max:.1} m), {unconn} on unconnected ends"
        );
        if let Some((o, c)) = sample {
            println!(
                "      sample: light ({:.1},{:.1},{:.1}) at junction ({:.1},{:.1},{:.1})",
                o[0], o[1], o[2], c[0], c[1], c[2]
            );
        }

        // Routable-arc coverage (F10-B.7): a lit end only surfaces on
        // the nav graph as an arc's `exit_light` when the direction
        // that exits it carries a vehicle arc — one-way and arc-less
        // roads leave authored heads the runtime never sees.
        let graph = mm2_game::NavGraph::build(&bai).graph;
        let (mut exit_lit, mut entry_only, mut no_arc) = (0usize, 0usize, 0usize);
        for (ri, road) in bai.roads.iter().enumerate() {
            let lit_end = |end: &mm2_formats::bai::RoadEnd| {
                end.traffic_light_origin.iter().all(|c| c.is_finite())
                    && end.traffic_light_origin.iter().any(|c| *c != 0.0)
            };
            let nav = graph.road(ri as u16);
            // `end` exits the forward arc (index 0); `start` exits
            // the backward arc (index 1).
            for (end, exit_idx, entry_idx) in
                [(&road.end, 0usize, 1usize), (&road.start, 1usize, 0usize)]
            {
                if !lit_end(end) {
                    continue;
                }
                match nav {
                    Some(r) if r.arcs[exit_idx].is_some() => exit_lit += 1,
                    Some(r) if r.arcs[entry_idx].is_some() => entry_only += 1,
                    _ => no_arc += 1,
                }
            }
        }
        println!(
            "      coverage: {exit_lit} lit ends are arc exits, \
             {entry_only} only touch an upstream arc, {no_arc} on arc-less roads"
        );
    }
    println!(
        "  {parsed} parsed ({}/{} expected), {unsupported} unsupported extras, {issues_total} issue(s)",
        expected.iter().filter(|l| vfs.resolve(l).is_some()).count(),
        expected.len(),
    );
    if strict && (issues_total > 0 || !failures.is_empty()) {
        return Err(format!(
            "strict bai audit: {} failures, {issues_total} issues",
            failures.len()
        )
        .into());
    }
    Ok(())
}

/// The `nav` audit's optional report flags, bundled so `nav` stays
/// readable.
struct NavOptions<'a> {
    /// `--route from:to` probe spec.
    route: Option<&'a str>,
    /// `--routes n` census + seeded-probe count.
    routes: Option<usize>,
    /// `--aimap` override path.
    aimap: Option<&'a str>,
    /// `--turns` reconciliation report.
    turns: bool,
    /// `--gaps` junction-interior census.
    gaps: bool,
    /// `--strict` failure gate.
    strict: bool,
}

/// Navigation-graph audit (F09-B): each stock city's `city/<name>.bai`
/// is loaded through the production `mm2_content::load_nav_graph` path
/// and its `NavGraph` build reported — arc/lane counts, one-way roads,
/// dead ends, weakly connected components and every `NavIssue`. The
/// optional `--route from:to` probe anchors each road index on its
/// first arc's lane and runs the bounded A* route query — honouring
/// the aimap's road closures — printing the step sequence or the
/// specific `RouteError`. `--aimap` substitutes a different override
/// file (an explicit path must resolve). `--routes n` adds the F09-C
/// route-constraint validation: a per-arc directed reachability census
/// over authored exits plus `n` seeded `route_roads` probes, each
/// checked for chain consistency and closed-road traversal.
/// `--turns` reconciles every exit's geometric turn classification
/// against the authored counterclockwise road-index delta. `--gaps`
/// censuses the junction-interior distance every legal lane transfer
/// spans — the measure of what a direct transfer teleports (F10-B.9).
/// `--strict` fails on any load failure or issue.
fn nav(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    opts: &NavOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;

    let NavOptions {
        route,
        routes,
        aimap,
        turns,
        gaps,
        strict,
    } = *opts;
    let probe = match route {
        Some(spec) => {
            let (a, b) = spec
                .split_once(':')
                .and_then(|(a, b)| a.parse::<u16>().ok().zip(b.parse::<u16>().ok()))
                .ok_or_else(|| format!("--route expects <from>:<to> road indices, got {spec:?}"))?;
            Some((a, b))
        }
        None => None,
    };

    let cities: Vec<String> = match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => mm2_content::EXPECTED_CITIES
            .iter()
            .map(|c| c.to_string())
            .collect(),
    };

    // An explicit `--aimap` is operator input: it must resolve.
    if let Some(p) = aimap
        && vfs.resolve(p).is_none()
    {
        return Err(format!("--aimap {p} does not resolve in the VFS").into());
    }

    let mut failures: Vec<String> = Vec::new();
    let mut issues_total = 0usize;

    println!("== navigation graph (BAI) ==");
    for c in &cities {
        let logical = format!("city/{c}.bai");
        let build = match mm2_content::load_nav_graph(&vfs, c) {
            Ok(b) => b,
            Err(e) => {
                println!("  {logical:<22} failed: {e}");
                failures.push(format!("{logical}: {e}"));
                continue;
            }
        };
        let g = &build.graph;
        let s = g.stats();
        println!(
            "  {logical:<22} ok — {} roads ({} one-way), {} arcs, {} vehicle + {} sidewalk + {} rail lanes, {} intersections, {} dead ends, {} components",
            s.roads,
            s.one_way_roads,
            s.vehicle_arcs,
            s.vehicle_lanes,
            s.sidewalk_lanes,
            s.tram_lanes + s.train_lanes,
            s.intersections,
            s.dead_ends,
            s.components,
        );
        issues_total += build.issues.len();
        for issue in &build.issues {
            println!("    issue: {issue}");
        }

        // Aimap overrides: the default `city/<c>.aimap` may be absent
        // (a modded city may not ship one); an explicit `--aimap` path
        // was already checked to resolve. A malformed file is reported,
        // not fatal.
        let aimap_path = aimap.map_or_else(|| format!("city/{c}.aimap"), str::to_string);
        let overrides = match mm2_content::load_nav_overrides(&vfs, &aimap_path) {
            Ok(o) => o,
            Err(e) => {
                println!("    aimap {aimap_path}: failed to parse ({e}); overrides ignored");
                None
            }
        };
        if let Some(o) = &overrides {
            println!(
                "    aimap {aimap_path}: {} closed road(s), speed limit {:?}",
                o.closed_roads.len(),
                o.default_speed_limit
            );
        }

        if let Some((from, to)) = probe {
            let opts = overrides
                .as_ref()
                .map_or_else(mm2_game::RouteOptions::default, |o| o.route_options());
            match g.route_roads(from, to, &opts) {
                Ok(r) => {
                    let steps: Vec<String> = r
                        .steps
                        .iter()
                        .map(|id| {
                            let arc = g.arc(*id);
                            let dir = match arc.dir {
                                mm2_game::TravelDir::Forward => "+",
                                mm2_game::TravelDir::Backward => "-",
                            };
                            format!("{}{dir}", arc.road)
                        })
                        .collect();
                    println!(
                        "    route {from}→{to}: {} steps ({:.0} m) {}",
                        r.steps.len(),
                        r.length,
                        steps.join(" → "),
                    );
                }
                Err(e) => println!("    route {from}→{to}: {e}"),
            }
        }

        if let Some(n) = routes {
            let opts = overrides
                .as_ref()
                .map_or_else(mm2_game::RouteOptions::default, |o| o.route_options());
            // Directed reachability census: one bounded walk per arc
            // over authored exits — the walk can never leave authored
            // connectivity, so unreachable pairs are real constraints.
            let n_arcs = g.stats().vehicle_arcs as u32;
            let mut full = 0u32;
            let mut self_only = 0u32;
            let mut unreachable_pairs = 0u64;
            let mut reach: Vec<(usize, u32)> = Vec::with_capacity(n_arcs as usize);
            for i in 0..n_arcs {
                let r = g
                    .reachable_arcs(mm2_game::ArcId(i), &opts.closed_roads)
                    .len();
                unreachable_pairs += n_arcs as u64 - r as u64;
                if r as u32 == n_arcs {
                    full += 1;
                }
                if r == 1 {
                    self_only += 1;
                }
                reach.push((r, i));
            }
            reach.sort();
            println!(
                "    census: {full}/{n_arcs} arcs reach the whole graph, \
                 {self_only} reach only themselves, \
                 {unreachable_pairs} ordered arc pairs unreachable"
            );
            for &(r, i) in reach.iter().take(8) {
                if r as u32 == n_arcs {
                    break;
                }
                let a = g.arc(mm2_game::ArcId(i));
                let dir = match a.dir {
                    mm2_game::TravelDir::Forward => "+",
                    mm2_game::TravelDir::Backward => "-",
                };
                // Reach 1 splits two ways: an authored dead end, or a
                // junction every other arm only *enters* (one-way trap).
                if r == 1 {
                    let why = match a.exit {
                        mm2_game::ArcEnd::DeadEnd => "dead end",
                        mm2_game::ArcEnd::Intersection(_) => "no legal continuation",
                    };
                    println!("      smallest reach: {}{dir} reaches {r} ({why})", a.road);
                } else {
                    println!("      smallest reach: {}{dir} reaches {r}", a.road);
                }
            }

            // Seeded probes between routable roads: every query must
            // terminate, and every returned route must be a real chain
            // of authored turns — endpoints land on the asked roads and
            // consecutive arcs share a turn. An expansion-limit hit or
            // a closed-road traversal mid-route is a finding.
            let routable: Vec<u16> = g
                .roads()
                .iter()
                .enumerate()
                .filter(|(_, r)| r.arcs.iter().any(|a| a.is_some()))
                .map(|(i, _)| i as u16)
                .collect();
            let mut rng = mm2_game::NavRng::new(0xF09C);
            let mut tallies: BTreeMap<&str, u64> = BTreeMap::new();
            let mut violations = 0u64;
            let mut shown_errors = 0;
            for _ in 0..n {
                let (Some(&from), Some(&to)) = (rng.pick(&routable), rng.pick(&routable)) else {
                    break;
                };
                match g.route_roads(from, to, &opts) {
                    Ok(route) => {
                        *tallies.entry("ok").or_default() += 1;
                        let steps = &route.steps;
                        if g.arc(steps[0]).road != from || g.arc(*steps.last().unwrap()).road != to
                        {
                            violations += 1;
                            println!(
                                "      violation: route {from}→{to} ran roads {}→{}",
                                g.arc(steps[0]).road,
                                g.arc(*steps.last().unwrap()).road
                            );
                        }
                        for w in steps.windows(2) {
                            if !g.exits(w[0]).iter().any(|e| e.to == w[1]) {
                                violations += 1;
                                println!(
                                    "      violation: route {from}→{to} steps {}→{} share no turn",
                                    g.arc(w[0]).road,
                                    g.arc(w[1]).road
                                );
                            }
                        }
                        // Interior arcs are always entered through a
                        // turn; endpoints may legitimately sit on a
                        // closed road.
                        for a in steps.iter().take(steps.len().saturating_sub(1)).skip(1) {
                            let road = g.arc(*a).road;
                            if opts.closed_roads.contains(&road) {
                                violations += 1;
                                println!(
                                    "      violation: route {from}→{to} enters closed road {road}"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        let key = match e {
                            mm2_game::RouteError::NoStartLane => "no-start-lane",
                            mm2_game::RouteError::NoGoalLane => "no-goal-lane",
                            mm2_game::RouteError::Unreachable { .. } => "unreachable",
                            mm2_game::RouteError::ExpansionLimit { .. } => {
                                issues_total += 1;
                                "expansion-limit"
                            }
                        };
                        *tallies.entry(key).or_default() += 1;
                        if shown_errors < 10 {
                            println!("      probe {from}→{to}: {e}");
                            shown_errors += 1;
                        }
                    }
                }
            }
            let tally = if tallies.is_empty() {
                "none run".to_string()
            } else {
                tallies
                    .iter()
                    .map(|(k, v)| format!("{v} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            println!(
                "    routes: {n} seeded probes over {} routable roads — {tally}, {violations} violations",
                routable.len()
            );
            issues_total += violations as usize;
        }

        if turns {
            // Reconciliation report: the authored counterclockwise
            // road-index delta vs the geometric turn classification,
            // histogrammed by intersection arity. The original's index
            // arithmetic is only documented for 4-ways — this report is
            // how that claim gets measured against retail data.
            let mut table: std::collections::BTreeMap<(usize, u8), [u32; 3]> =
                std::collections::BTreeMap::new();
            let mut total = 0u32;
            for road in g.roads() {
                for arc in road.arcs.iter().flatten() {
                    for exit in g.exits(*arc) {
                        let arity = g.intersections()[exit.intersection as usize].roads.len();
                        let slot = match exit.turn {
                            mm2_game::TurnKind::Left => 0,
                            mm2_game::TurnKind::Straight => 1,
                            mm2_game::TurnKind::Right => 2,
                        };
                        table.entry((arity, exit.ccw_delta)).or_default()[slot] += 1;
                        total += 1;
                    }
                }
            }
            println!("    turns: {total} exits");
            for ((arity, delta), counts) in &table {
                println!(
                    "      {arity}-way Δccw={delta}: left={} straight={} right={}",
                    counts[0], counts[1], counts[2]
                );
            }
        }

        if gaps {
            // Junction-interior census (F10-B.9): every legal lane
            // transfer spans the distance between the approach lane's
            // end and the landing lane's start — the gap a crossing
            // path covers, and the distance a direct transfer would
            // teleport. Counts are per lane×exit: an arc's lanes share
            // exits but land on different lanes.
            let mut chords: Vec<f32> = Vec::new();
            let mut paths: Vec<f32> = Vec::new();
            let mut ends: Vec<f32> = Vec::new();
            let mut starts: Vec<f32> = Vec::new();
            let mut direct = 0usize;
            for lane in g.lanes() {
                if lane.arc.is_none() {
                    continue;
                }
                for exit in g.legal_exits(lane.id) {
                    let Some(to) = g.transfer_lane(lane.id, exit.to) else {
                        continue;
                    };
                    let centre = g.intersections()[exit.intersection as usize].center;
                    let (Some(a), Some(b)) =
                        (g.sample_lane(lane.id, lane.length), g.sample_lane(to, 0.0))
                    else {
                        continue;
                    };
                    let d = |p: [f32; 3]| {
                        ((p[0] - centre[0]).powi(2)
                            + (p[1] - centre[1]).powi(2)
                            + (p[2] - centre[2]).powi(2))
                        .sqrt()
                    };
                    ends.push(d(a.position));
                    starts.push(d(b.position));
                    match g.crossing_path(lane.id, to) {
                        Some(p) => {
                            let chord = ((b.position[0] - a.position[0]).powi(2)
                                + (b.position[1] - a.position[1]).powi(2)
                                + (b.position[2] - a.position[2]).powi(2))
                            .sqrt();
                            chords.push(chord);
                            paths.push(p.length);
                        }
                        None => direct += 1,
                    }
                }
            }
            let stats = |v: &mut Vec<f32>| {
                v.sort_by(f32::total_cmp);
                if v.is_empty() {
                    return "none".to_string();
                }
                format!(
                    "min {:.1} median {:.1} p90 {:.1} max {:.1}",
                    v[0],
                    v[v.len() / 2],
                    v[(v.len() * 9 / 10).min(v.len() - 1)],
                    v[v.len() - 1],
                )
            };
            println!(
                "    gaps: {} crossings ({} coincident direct) — chord {}; path {}",
                chords.len(),
                direct,
                stats(&mut chords),
                stats(&mut paths),
            );
            println!(
                "      endpoints to junction centre — approach-end {}; landing-start {}",
                stats(&mut ends),
                stats(&mut starts),
            );
            let mut buckets = [0usize; 6];
            for &c in &chords {
                buckets[match c {
                    c if c < 10.0 => 0,
                    c if c < 20.0 => 1,
                    c if c < 40.0 => 2,
                    c if c < 80.0 => 3,
                    c if c < 160.0 => 4,
                    _ => 5,
                }] += 1;
            }
            println!(
                "      chord buckets: <10m {} | 10-20 {} | 20-40 {} | 40-80 {} | 80-160 {} | >160 {}",
                buckets[0], buckets[1], buckets[2], buckets[3], buckets[4], buckets[5],
            );
            // Worst outliers: which transfers span more than a block?
            let mut worst: Vec<(f32, String)> = Vec::new();
            for lane in g.lanes() {
                if lane.arc.is_none() {
                    continue;
                }
                for exit in g.legal_exits(lane.id) {
                    let Some(to) = g.transfer_lane(lane.id, exit.to) else {
                        continue;
                    };
                    let (Some(a), Some(b)) =
                        (g.sample_lane(lane.id, lane.length), g.sample_lane(to, 0.0))
                    else {
                        continue;
                    };
                    let chord = ((b.position[0] - a.position[0]).powi(2)
                        + (b.position[1] - a.position[1]).powi(2)
                        + (b.position[2] - a.position[2]).powi(2))
                    .sqrt();
                    if chord > 60.0 {
                        let arc = g.arc(lane.arc.unwrap());
                        let to_arc = g.arc(exit.to);
                        let centre = g.intersections()[exit.intersection as usize].center;
                        worst.push((
                            chord,
                            format!(
                                "road {} lane {:?} -> arc {:?} (road {}) via int {} turn {:?} d{:.2}\n         lane_end {:?} arc_exit {:?}\n         land_start {:?} arc_entry {:?} centre {:?}",
                                arc.road, lane.id, exit.to, to_arc.road,
                                exit.intersection, exit.turn, exit.heading_change,
                                a.position.map(|v| v.round() as i32),
                                arc.exit_point.map(|v| v.round() as i32),
                                b.position.map(|v| v.round() as i32),
                                to_arc.entry_point.map(|v| v.round() as i32),
                                centre.map(|v| v.round() as i32),
                            ),
                        ));
                    }
                }
            }
            worst.sort_by(|a, b| b.0.total_cmp(&a.0));
            for (c, s) in worst.iter().take(12) {
                println!("      {c:7.1} m  {s}");
            }
        }
    }
    println!(
        "  {} cities, {} failures, {issues_total} issue(s)",
        cities.len(),
        failures.len(),
    );
    if strict && (issues_total > 0 || !failures.is_empty()) {
        return Err(format!(
            "strict nav audit: {} failures, {issues_total} issues",
            failures.len()
        )
        .into());
    }
    Ok(())
}

/// AI-map override audit (F09-A.2): the expected denominator is
/// `city/<name>.aimap` plus every discovered `race/<name>/*.aimap` /
/// `*.aimap_p` for each stock city — authored event data whose parse
/// failures are real failures. Any other discovered `*.aimap`/
/// `*.aimap_p` is an audited extra whose failures are reported
/// `unsupported`, never hidden. Parsed files get `Aimap::validate`
/// issues plus two cross-checks: `[Exceptions]` road ids against the
/// same-city `city/<city>.bai` road space, and `[Opponent]` waypoint
/// references resolved through the VFS.
fn aimap(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use mm2_formats::aimap::Aimap;

    let vfs = build_vfs(dir, mods)?;
    let stock: Vec<String> = match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => mm2_content::EXPECTED_CITIES
            .iter()
            .map(|c| c.to_string())
            .collect(),
    };
    let paths = vfs.list();
    let is_aimap = |p: &str| p.ends_with(".aimap") || p.ends_with(".aimap_p");

    let mut expected: Vec<String> = stock.iter().map(|c| format!("city/{c}.aimap")).collect();
    expected.extend(
        paths
            .iter()
            .filter(|p| stock.iter().any(|c| p.starts_with(&format!("race/{c}/"))) && is_aimap(p))
            .cloned(),
    );
    expected.sort();

    let mut extras: Vec<String> = match city {
        // A single-city audit sees no other city's files at all.
        Some(_) => Vec::new(),
        None => paths
            .iter()
            .filter(|p| is_aimap(p) && !expected.contains(p))
            .cloned()
            .collect(),
    };
    extras.sort();

    // BAI road-id spaces per city, loaded on first use.
    let mut bai_roads: BTreeMap<String, Option<std::collections::BTreeSet<u16>>> = BTreeMap::new();

    let mut failures: Vec<String> = Vec::new();
    let mut issues_total = 0usize;
    let mut parsed = 0usize;
    let mut unsupported = 0usize;

    println!("== AI-map overrides (aimap) ==");
    for logical in expected.iter().chain(extras.iter()) {
        let is_expected = expected.contains(logical);
        let tag = if is_expected { "expected" } else { "extra" };
        let Some(res) = vfs.resolve(logical) else {
            println!("  {logical:<34} {tag:<9} missing");
            failures.push(format!("{logical}: expected file not found"));
            continue;
        };
        let bytes = vfs.read(&res)?;
        let text = match std::str::from_utf8(&bytes) {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("not UTF-8 text: {e}");
                if is_expected {
                    println!("  {logical:<34} {tag:<9} failed: {msg}");
                    failures.push(format!("{logical}: {msg}"));
                } else {
                    println!("  {logical:<34} {tag:<9} unsupported: {msg}");
                    unsupported += 1;
                }
                continue;
            }
        };
        let aimap = match Aimap::parse(text) {
            Ok(a) => a,
            Err(e) => {
                if is_expected {
                    println!("  {logical:<34} {tag:<9} failed: {e}");
                    failures.push(format!("{logical}: {e}"));
                } else {
                    println!("  {logical:<34} {tag:<9} unsupported: {e}");
                    unsupported += 1;
                }
                continue;
            }
        };
        parsed += 1;
        let mut file_issues: Vec<String> = aimap.validate().iter().map(|i| i.to_string()).collect();
        file_issues.extend(aimap.diagnostics.iter().map(|d| d.to_string()));

        // Cross-checks against the file's own city: `city/<stem>.aimap`
        // and `race/<city>/…` both key off the path's second component.
        let file_city = logical
            .strip_prefix("city/")
            .map(|s| s.trim_end_matches(".aimap"))
            .or_else(|| logical.split('/').nth(1));
        if let Some(city) = file_city {
            match bai_road_ids(&mut bai_roads, &vfs, city) {
                Some(roads) => {
                    let missing: Vec<u32> = aimap
                        .exceptions
                        .iter()
                        .map(|e| e.road)
                        .filter(|id| !roads.contains(&(*id as u16)))
                        .collect();
                    if !missing.is_empty() {
                        file_issues.push(format!(
                            "{} exception road id(s) outside city/{city}.bai: {missing:?}",
                            missing.len()
                        ));
                    }
                }
                None => {
                    if !aimap.exceptions.is_empty() {
                        println!("    note: no city/{city}.bai — exception road ids not checked");
                    }
                }
            }
            for opp in &aimap.opponents {
                let opp_path = format!("race/{city}/{}", opp.waypoints);
                if vfs.resolve(&opp_path).is_none() {
                    file_issues.push(format!(
                        "opponent {}: waypoint file {} not found",
                        opp.geo, opp_path
                    ));
                }
            }
        }
        issues_total += file_issues.len();
        println!(
            "  {logical:<34} {tag:<9} ok — exc {}, police {}, opp {}, amb {} ({} issues)",
            aimap.exceptions.len(),
            aimap.police.len(),
            aimap.opponents.len(),
            aimap.ambient_types.len(),
            file_issues.len(),
        );
        for issue in &file_issues {
            println!("    issue: {issue}");
        }
    }
    println!(
        "  {parsed} parsed ({}/{} expected resolved), {unsupported} unsupported extras, {issues_total} issue(s)",
        expected.iter().filter(|l| vfs.resolve(l).is_some()).count(),
        expected.len(),
    );
    if strict && (issues_total > 0 || !failures.is_empty()) {
        return Err(format!(
            "strict aimap audit: {} failures, {issues_total} issues",
            failures.len()
        )
        .into());
    }
    Ok(())
}

/// Load (and cache) the road-id set of `city/<city>.bai` for the
/// aimap exception cross-check. `None` when the BAI is missing or
/// unparseable — the audit notes the skipped check per file.
fn bai_road_ids<'a>(
    cache: &'a mut BTreeMap<String, Option<std::collections::BTreeSet<u16>>>,
    vfs: &Vfs,
    city: &str,
) -> Option<&'a std::collections::BTreeSet<u16>> {
    if !cache.contains_key(city) {
        let ids = vfs
            .resolve(&format!("city/{city}.bai"))
            .and_then(|res| vfs.read(&res).ok())
            .and_then(|bytes| Bai::parse(&bytes).ok())
            .map(|bai| bai.roads.iter().map(|r| r.id).collect());
        cache.insert(city.to_string(), ids);
    }
    cache.get(city).and_then(|o| o.as_ref())
}

/// Whether a pathset path name resolves to a placeable asset:
/// `geometry/<name>.pkg` (props) or `texture/<name>.*` (decals).
/// Event-state decorations (`OPEN:`, `inactive:` — see
/// `docs/research/pathset.md`) are stripped for the lookup; the raw
/// name is tried first.
fn pathset_name_resolves(vfs: &Vfs, name: &str) -> bool {
    if vfs.resolve(&format!("geometry/{name}.pkg")).is_some() {
        return true;
    }
    TEXTURE_EXTS
        .iter()
        .any(|ext| vfs.resolve(&format!("texture/{name}.{ext}")).is_some())
}

/// Placement-pathset audit (F03-A): the expected denominator is every
/// discovered `*.pathset` — city prop/decal sets, `audio_pathsets/`
/// sound paths, `city/phys/` test sets, `bak/` snapshots and per-event
/// `race/<city>/` records are all authored content, so a parse failure
/// anywhere is a real failure (three truncated london files prove the
/// point). Parsed files get `Pathset::validate` issues plus a name
/// cross-check: non-`PATHnn` names must resolve to a PKG or texture.
/// `--city` restricts to `city/<stem>/` + `race/<stem>/`; `--strict`
/// exits nonzero on any failure or issue.
fn pathset(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let vfs = build_vfs(dir, mods)?;
    let stem = city.map(|c| c.to_ascii_lowercase());
    let mut logicals: Vec<String> = vfs
        .list()
        .into_iter()
        .filter(|p| p.ends_with(".pathset"))
        .filter(|p| {
            stem.as_ref().is_none_or(|c| {
                p.starts_with(&format!("city/{c}/")) || p.starts_with(&format!("race/{c}/"))
            })
        })
        .collect();
    logicals.sort();
    if logicals.is_empty() {
        return Err("pathset audit: no .pathset files discovered".into());
    }

    let mut failures: Vec<String> = Vec::new();
    let mut issues_total = 0usize;
    let mut parsed = 0usize;

    println!("== placement pathsets (PTH1) ==");
    for logical in &logicals {
        let res = vfs.resolve(logical).expect("listed path resolves");
        let bytes = vfs.read(&res)?;
        let ps = match Pathset::parse(&bytes) {
            Ok(p) => p,
            Err(e) => {
                println!("  {logical:<52} failed: {e}");
                failures.push(format!("{logical}: {e}"));
                continue;
            }
        };
        parsed += 1;
        let mut file_issues: Vec<String> = ps.validate().iter().map(|i| i.to_string()).collect();

        let points: usize = ps.paths.iter().map(|p| p.points.len()).sum();
        let mut kinds = [0usize; 3];
        let mut unknown_kinds = 0usize;
        for p in &ps.paths {
            match p.kind() {
                Some(mm2_formats::pathset::PathKind::Points) => kinds[0] += 1,
                Some(mm2_formats::pathset::PathKind::Directed) => kinds[1] += 1,
                Some(mm2_formats::pathset::PathKind::LineStrip) => kinds[2] += 1,
                None => unknown_kinds += 1,
            }
        }

        let mut checked = std::collections::BTreeSet::new();
        for path in &ps.paths {
            let Some(name) = path.asset_name() else {
                continue;
            };
            if !checked.insert(name) {
                continue;
            }
            if !pathset_name_resolves(&vfs, name) {
                file_issues.push(format!(
                    "path \"{name}\": resolves to no geometry/<n>.pkg or texture/<n>.*"
                ));
            }
        }

        issues_total += file_issues.len();
        println!(
            "  {logical:<52} ok — {} paths, {} points, kinds pts:{} dir:{} strip:{}{}",
            ps.paths.len(),
            points,
            kinds[0],
            kinds[1],
            kinds[2],
            if unknown_kinds > 0 {
                format!(" unknown:{unknown_kinds}")
            } else {
                String::new()
            },
        );
        for issue in &file_issues {
            println!("    issue: {issue}");
        }
    }
    println!(
        "  {parsed}/{} parsed, {} failures, {issues_total} issue(s)",
        logicals.len(),
        failures.len(),
    );
    if strict && (issues_total > 0 || !failures.is_empty()) {
        return Err(format!(
            "strict pathset audit: {} failures, {issues_total} issues",
            failures.len()
        )
        .into());
    }
    Ok(())
}

/// Roadside-prop rule-table audit (F03-A.2): the expected denominator
/// is `city/<stock>/{propdefs,proprules,props}.csv` for each stock
/// city; every other discovered `city/**` `propdefs*`/`proprules*`/
/// `props*` CSV (including `.csv.txt` exports and the `city/phys/`,
/// `bak/` dev sets) is an audited extra whose parse failures are
/// reported `unsupported`, never hidden. `geometry/props.csv` shares
/// the basename but is a different table (per-PKG LOD triangle
/// counts) and is parsed with its own layout. Cross-checks: rule prop
/// references must name a sibling `propdefs.csv` entry, def file refs
/// and `props.csv` names must resolve `geometry/<n>.pkg`, LOD rows
/// must resolve `geometry/<name>`, and each nonzero PSDL `prop_rule`
/// byte must name a defined `n{NN}` rule number. `--city` restricts
/// to `city/<stem>/`; `--strict` exits nonzero on any failure or
/// issue.
fn proprules(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use mm2_formats::proprules::{PropDefs, PropGroups, PropLodStats, PropRules};

    let vfs = build_vfs(dir, mods)?;
    let stem = city.map(|c| c.to_ascii_lowercase());
    let cities: Vec<String> = match &stem {
        Some(c) => vec![c.clone()],
        None => mm2_content::EXPECTED_CITIES
            .iter()
            .map(|c| c.to_string())
            .collect(),
    };
    let mut expected: Vec<String> = cities
        .iter()
        .flat_map(|c| {
            ["propdefs.csv", "proprules.csv", "props.csv"]
                .iter()
                .map(move |f| format!("city/{c}/{f}"))
        })
        .collect();
    expected.sort();

    let is_table = |p: &str| {
        let name = p.rsplit('/').next().unwrap_or(p);
        (name.ends_with(".csv") || name.ends_with(".csv.txt"))
            && (name.starts_with("propdefs")
                || name.starts_with("proprules")
                || name.starts_with("props"))
    };
    let mut logicals: Vec<String> = vfs
        .list()
        .into_iter()
        .filter(|p| p.starts_with("city/") && is_table(p))
        .filter(|p| {
            stem.as_ref()
                .is_none_or(|c| p.starts_with(&format!("city/{c}/")))
        })
        .collect();
    // geometry/props.csv is the LOD table — different schema, audited
    // in the unfiltered audit only.
    if stem.is_none() && vfs.resolve("geometry/props.csv").is_some() {
        logicals.push("geometry/props.csv".to_string());
    }
    for e in &expected {
        if !logicals.contains(e) {
            logicals.push(e.clone());
        }
    }
    logicals.sort();
    logicals.dedup();
    if logicals.is_empty() {
        return Err("proprules audit: no prop rule tables discovered".into());
    }

    let mut failures: Vec<String> = Vec::new();
    let mut issues_total = 0usize;
    let mut parsed = 0usize;
    let mut unsupported = 0usize;
    // Parsed tables grouped by containing directory for cross-checks.
    let mut defs_by_dir: BTreeMap<String, Vec<(String, PropDefs)>> = BTreeMap::new();
    let mut rules_by_dir: BTreeMap<String, Vec<(String, PropRules)>> = BTreeMap::new();
    let mut groups: Vec<(String, PropGroups)> = Vec::new();
    let mut lods: Vec<(String, PropLodStats)> = Vec::new();

    println!("== roadside prop rules (propdefs/proprules/props.csv) ==");
    for logical in &logicals {
        let is_expected = expected.contains(logical);
        let tag = if is_expected { "expected" } else { "extra" };
        let Some(res) = vfs.resolve(logical) else {
            println!("  {logical:<52} {tag:<9} missing");
            failures.push(format!("{logical}: expected file not found"));
            continue;
        };
        let bytes = vfs.read(&res)?;
        let text = String::from_utf8_lossy(&bytes);
        let name = logical.rsplit('/').next().unwrap_or(logical);
        let dir_key = logical
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default();
        let mut emit = |what: &str,
                        count: usize,
                        diagnostics: &[mm2_formats::racedata::TableDiagnostic],
                        issues: Vec<mm2_formats::proprules::PropRuleIssue>| {
            parsed += 1;
            issues_total += diagnostics.len() + issues.len();
            println!("  {logical:<52} {tag:<9} ok — {count} {what}");
            for d in diagnostics {
                println!("    issue: {d}");
            }
            for i in &issues {
                println!("    issue: {i}");
            }
        };
        let mut fail = |e: mm2_formats::FormatError| {
            if is_expected {
                println!("  {logical:<52} {tag:<9} failed: {e}");
                failures.push(format!("{logical}: {e}"));
            } else {
                println!("  {logical:<52} {tag:<9} unsupported: {e}");
                unsupported += 1;
            }
        };
        if logical == "geometry/props.csv" {
            match PropLodStats::parse(&text) {
                Ok(t) => {
                    let issues = t.validate();
                    emit("LOD rows", t.stats.len(), &t.diagnostics, issues);
                    lods.push((logical.clone(), t));
                }
                Err(e) => fail(e),
            }
        } else if name.starts_with("propdefs") {
            match PropDefs::parse(&text) {
                Ok(t) => {
                    let issues = t.validate();
                    emit("defs", t.defs.len(), &t.diagnostics, issues);
                    defs_by_dir
                        .entry(dir_key)
                        .or_default()
                        .push((logical.clone(), t));
                }
                Err(e) => fail(e),
            }
        } else if name.starts_with("proprules") {
            match PropRules::parse(&text) {
                Ok(t) => {
                    let issues = t.validate();
                    emit("rules", t.rules.len(), &t.diagnostics, issues);
                    rules_by_dir
                        .entry(dir_key)
                        .or_default()
                        .push((logical.clone(), t));
                }
                Err(e) => fail(e),
            }
        } else {
            match PropGroups::parse(&text) {
                Ok(t) => {
                    let issues = t.validate();
                    emit(
                        &format!("group entries ({},{})", t.header[0], t.header[1]),
                        t.entries.len(),
                        &t.diagnostics,
                        issues,
                    );
                    groups.push((logical.clone(), t));
                }
                Err(e) => fail(e),
            }
        }
    }

    // Cross-checks. Each is an issue (authored anomaly or broken
    // reference), not a load failure.
    let issue = |issues: &mut usize, msg: String| {
        *issues += 1;
        println!("    issue: {msg}");
    };
    println!("  cross-checks:");
    for (d, rules) in &rules_by_dir {
        let def_names: std::collections::BTreeSet<&str> = defs_by_dir
            .get(d)
            .map(|v| {
                v.iter()
                    .flat_map(|(_, t)| t.defs.iter().map(|d| d.name.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        if def_names.is_empty() {
            issue(
                &mut issues_total,
                format!("{d}: proprules has no sibling propdefs.csv to resolve refs against"),
            );
        }
        let mut reported = std::collections::BTreeSet::new();
        for (path, table) in rules {
            for rule in &table.rules {
                for prop in &rule.props {
                    if !def_names.contains(prop.as_str()) && reported.insert(prop.as_str()) {
                        issue(
                            &mut issues_total,
                            format!(
                                "{path}: rule {:?} refs undefined propdef {prop:?}",
                                rule.name
                            ),
                        );
                    }
                }
            }
        }
    }
    for defs in defs_by_dir.values().flatten() {
        let mut reported = std::collections::BTreeSet::new();
        for def in &defs.1.defs {
            for f in &def.files {
                if vfs.resolve(&format!("geometry/{f}.pkg")).is_none()
                    && reported.insert(f.as_str())
                {
                    issue(
                        &mut issues_total,
                        format!(
                            "{}: propdef {:?} file {f:?} resolves to no geometry PKG",
                            defs.0, def.name
                        ),
                    );
                }
            }
        }
    }
    for (path, table) in &groups {
        let mut reported = std::collections::BTreeSet::new();
        for e in &table.entries {
            if vfs.resolve(&format!("geometry/{}.pkg", e.name)).is_none()
                && reported.insert(e.name.as_str())
            {
                issue(
                    &mut issues_total,
                    format!(
                        "{path}: {} entry {:?} resolves to no geometry PKG",
                        e.group, e.name
                    ),
                );
            }
        }
    }
    for (path, table) in &lods {
        for s in &table.stats {
            if vfs.resolve(&format!("geometry/{}", s.name)).is_none() {
                issue(
                    &mut issues_total,
                    format!("{path}: LOD row {:?} resolves to no geometry file", s.name),
                );
            }
        }
    }
    // PSDL prop_rule byte ↔ rule-number check for direct city/<stem>
    // dirs that parsed a proprules table.
    for (d, rules) in &rules_by_dir {
        let Some(stem) = d.strip_prefix("city/").filter(|s| !s.contains('/')) else {
            continue;
        };
        let psdl_path = format!("city/{stem}.psdl");
        let defined: std::collections::BTreeSet<u8> = rules
            .iter()
            .flat_map(|(_, t)| t.rules.iter().filter_map(|r| r.rule_key().map(|(n, _)| n)))
            .collect();
        match vfs.resolve(&psdl_path) {
            Some(pres) => {
                let pbytes = vfs.read(&pres)?;
                match Psdl::parse(&pbytes) {
                    Ok(psdl) => {
                        let mut used: BTreeMap<u8, usize> = BTreeMap::new();
                        for &b in &psdl.prop_rules {
                            if b != 0 {
                                *used.entry(b).or_default() += 1;
                            }
                        }
                        let zero = psdl.prop_rules.iter().filter(|&&b| b == 0).count();
                        for (v, count) in &used {
                            if !defined.contains(v) {
                                issue(
                                    &mut issues_total,
                                    format!(
                                        "{psdl_path}: {count} room(s) reference undefined prop rule {v}"
                                    ),
                                );
                            }
                        }
                        let unused: Vec<u8> = defined
                            .iter()
                            .filter(|n| !used.contains_key(n))
                            .copied()
                            .collect();
                        println!(
                            "    psdl: {psdl_path} — {} rooms, {} rule-bearing, {} rules defined{}",
                            psdl.prop_rules.len(),
                            psdl.prop_rules.len() - zero,
                            defined.len(),
                            if unused.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    " (unused: {})",
                                    unused
                                        .iter()
                                        .map(|n| format!("n{n:02}"))
                                        .collect::<Vec<_>>()
                                        .join(",")
                                )
                            },
                        );
                    }
                    Err(e) => println!("    note: {psdl_path} failed to parse: {e}"),
                }
            }
            None => println!("    note: no {psdl_path} — prop_rule bytes not cross-checked"),
        }
    }
    println!(
        "  {parsed}/{} parsed, {} unsupported extras, {} failures, {issues_total} issue(s)",
        logicals.len(),
        unsupported,
        failures.len(),
    );
    if strict && (issues_total > 0 || !failures.is_empty()) {
        return Err(format!(
            "strict proprules audit: {} failures, {issues_total} issues",
            failures.len()
        )
        .into());
    }
    Ok(())
}

/// Surface-material audit (F06-A.1): the expected denominator is the
/// global pair `city/materials.mtl` + `city/materials.csv` — retail ships
/// exactly one pair shared by both cities (no per-city copies). Every
/// other discovered `*.mtl` / `city/**/materials*.csv` file is an
/// audited extra. `physics` refs in each parsed map are checked against
/// the union of defined material names; PSDL texture-table coverage is
/// reported per stock city (`--city` restricts it). `--strict` exits
/// nonzero on any failure or issue.
fn materials(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use mm2_formats::materials::{MaterialMap, MaterialSet, NONE_PHYSICS};

    let vfs = build_vfs(dir, mods)?;
    let expected = ["city/materials.mtl", "city/materials.csv"];

    let is_map = |p: &str| {
        let name = p.rsplit('/').next().unwrap_or(p);
        (name.ends_with(".csv") || name.ends_with(".csv.txt")) && name.starts_with("materials")
    };
    let mut logicals: Vec<String> = vfs
        .list()
        .into_iter()
        .filter(|p| p.ends_with(".mtl") || (p.starts_with("city/") && is_map(p)))
        .collect();
    for e in &expected {
        if !logicals.iter().any(|l| l == e) {
            logicals.push((*e).to_string());
        }
    }
    logicals.sort();
    logicals.dedup();

    let mut failures: Vec<String> = Vec::new();
    let mut issues_total = 0usize;
    let mut parsed = 0usize;
    let mut unsupported = 0usize;
    let mut sets: Vec<(String, MaterialSet)> = Vec::new();
    let mut maps: Vec<(String, MaterialMap)> = Vec::new();

    println!("== surface materials (materials.mtl / materials.csv) ==");
    for logical in &logicals {
        let is_expected = expected.contains(&logical.as_str());
        let tag = if is_expected { "expected" } else { "extra" };
        let Some(res) = vfs.resolve(logical) else {
            println!("  {logical:<52} {tag:<9} missing");
            failures.push(format!("{logical}: expected file not found"));
            continue;
        };
        let bytes = vfs.read(&res)?;
        let text = String::from_utf8_lossy(&bytes);
        if logical.ends_with(".mtl") {
            match MaterialSet::parse(&text) {
                Ok(t) => {
                    let issues = t.validate();
                    parsed += 1;
                    issues_total += issues.len();
                    println!(
                        "  {logical:<52} {tag:<9} ok — {} materials ({})",
                        t.defs.len(),
                        t.defs
                            .iter()
                            .map(|d| d.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    for i in &issues {
                        println!("    issue: {i}");
                    }
                    sets.push((logical.clone(), t));
                }
                Err(e) if is_expected => {
                    println!("  {logical:<52} {tag:<9} failed: {e}");
                    failures.push(format!("{logical}: {e}"));
                }
                Err(e) => {
                    println!("  {logical:<52} {tag:<9} unsupported: {e}");
                    unsupported += 1;
                }
            }
        } else {
            match MaterialMap::parse(&text) {
                Ok(t) => {
                    let issues = t.validate();
                    parsed += 1;
                    issues_total += t.diagnostics.len() + issues.len();
                    let named = t.rows.iter().filter(|r| !r.is_none()).count();
                    println!(
                        "  {logical:<52} {tag:<9} ok — {} rows ({} named, {} {})",
                        t.rows.len(),
                        named,
                        t.rows.len() - named,
                        NONE_PHYSICS
                    );
                    for d in &t.diagnostics {
                        println!("    issue: {d}");
                    }
                    for i in &issues {
                        println!("    issue: {i}");
                    }
                    maps.push((logical.clone(), t));
                }
                Err(e) if is_expected => {
                    println!("  {logical:<52} {tag:<9} failed: {e}");
                    failures.push(format!("{logical}: {e}"));
                }
                Err(e) => {
                    println!("  {logical:<52} {tag:<9} unsupported: {e}");
                    unsupported += 1;
                }
            }
        }
    }

    // Cross-checks. Each is an issue (authored anomaly or broken
    // reference), not a load failure.
    let issue = |issues: &mut usize, msg: String| {
        *issues += 1;
        println!("    issue: {msg}");
    };
    println!("  cross-checks:");

    // csv physics refs → defined material names (union across all
    // parsed .mtl sets; duplicate definitions across sets are issues).
    {
        let mut defined: BTreeMap<&str, &str> = BTreeMap::new();
        for (path, set) in &sets {
            for def in &set.defs {
                if let Some(first) = defined.insert(def.name.as_str(), path.as_str()) {
                    issue(
                        &mut issues_total,
                        format!("{path}: material {:?} also defined by {first}", def.name),
                    );
                }
            }
        }
        for (path, map) in &maps {
            for row in &map.rows {
                if row.is_none() {
                    continue;
                }
                match defined.get(row.physics.as_str()) {
                    Some(_) => {}
                    None => issue(
                        &mut issues_total,
                        format!(
                            "{path}: line {}: {:?} refs undefined material {:?}",
                            row.line, row.texture, row.physics
                        ),
                    ),
                }
            }
        }
        let used: std::collections::BTreeSet<&str> = maps
            .iter()
            .flat_map(|(_, m)| {
                m.rows
                    .iter()
                    .filter(|r| !r.is_none())
                    .map(|r| r.physics.as_str())
            })
            .collect();
        let unused: Vec<&str> = defined
            .keys()
            .filter(|n| !used.contains(**n))
            .copied()
            .collect();
        println!(
            "    defined: {} materials ({} used by maps{})",
            defined.len(),
            used.len(),
            if unused.is_empty() {
                String::new()
            } else {
                format!(", unused: {}", unused.join(","))
            }
        );
    }

    // csv texture stems → texture/ files: informational — names like
    // `s_ocean`/`s_thames` mark surface semantics on geometry whose
    // texture table entry has no file, so an unresolved stem is not a
    // defect on its own.
    for (path, map) in &maps {
        let unresolved: Vec<&str> = map
            .rows
            .iter()
            .filter(|r| {
                vfs.resolve_preferred(&format!("texture/{}", r.texture), TEXTURE_EXTS)
                    .is_none()
            })
            .map(|r| r.texture.as_str())
            .collect();
        println!(
            "    {path}: {}/{} texture stems resolve to texture/ files",
            map.rows.len() - unresolved.len(),
            map.rows.len()
        );
        if !unresolved.is_empty() {
            println!(
                "      unresolved (semantic-only names, not defects): {}{}",
                unresolved
                    .iter()
                    .take(15)
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
                if unresolved.len() > 15 {
                    format!(" … +{}", unresolved.len() - 15)
                } else {
                    String::new()
                }
            );
        }
    }

    // PSDL texture-table → material coverage per stock city.
    let stems: Vec<String> = match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => mm2_content::EXPECTED_CITIES
            .iter()
            .map(|c| c.to_string())
            .collect(),
    };
    for stem in stems {
        let psdl_path = format!("city/{stem}.psdl");
        let Some(pres) = vfs.resolve(&psdl_path) else {
            println!("    note: no {psdl_path} — texture table not cross-checked");
            continue;
        };
        let pbytes = vfs.read(&pres)?;
        match Psdl::parse(&pbytes) {
            Ok(psdl) => {
                let mut named: BTreeMap<String, usize> = BTreeMap::new();
                let mut none_count = 0usize;
                let mut unmapped: Vec<&str> = Vec::new();
                // A `<stem>-NNNN` table entry is one frame of an animated
                // texture sequence (s_thames-0001..30); the map names the
                // base stem, so fall back to it when the full name is
                // unmapped — the same stem convention
                // `mm2_app::city::load_image_sequence` decodes.
                let lookup = |tex: &str| -> Option<&str> {
                    maps.iter().find_map(|(_, m)| m.lookup(tex)).or_else(|| {
                        mm2_formats::tex::frame_base_stem(tex)
                            .and_then(|b| maps.iter().find_map(|(_, m)| m.lookup(b)))
                    })
                };
                let mut blank_count = 0usize;
                for tex in &psdl.textures {
                    if tex.is_empty() {
                        // Authored blank table slots — no texture, so no
                        // material coverage is expected of them.
                        blank_count += 1;
                        continue;
                    }
                    match lookup(tex) {
                        Some(p) if p != NONE_PHYSICS => {
                            *named.entry(p.to_string()).or_default() += 1;
                        }
                        Some(_) => none_count += 1,
                        None => unmapped.push(tex.as_str()),
                    }
                }
                println!(
                    "    psdl: {psdl_path} — {} texture names: {} named-material, {} {}, {} blank, {} not in map",
                    psdl.textures.len(),
                    named.values().sum::<usize>(),
                    none_count,
                    NONE_PHYSICS,
                    blank_count,
                    unmapped.len(),
                );
                if !unmapped.is_empty() {
                    println!(
                        "      not in map: {}{}",
                        unmapped
                            .iter()
                            .take(15)
                            .copied()
                            .collect::<Vec<_>>()
                            .join(", "),
                        if unmapped.len() > 15 {
                            format!(" … +{}", unmapped.len() - 15)
                        } else {
                            String::new()
                        }
                    );
                }
                if !named.is_empty() {
                    println!(
                        "      materials used: {}",
                        named
                            .iter()
                            .map(|(n, c)| format!("{n}×{c}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            Err(e) => println!("    note: {psdl_path} failed to parse: {e}"),
        }
    }

    println!(
        "  {parsed}/{} parsed, {} unsupported extras, {} failures, {issues_total} issue(s)",
        logicals.len(),
        unsupported,
        failures.len(),
    );
    if strict && (issues_total > 0 || !failures.is_empty()) {
        return Err(format!(
            "strict materials audit: {} failures, {issues_total} issues",
            failures.len()
        )
        .into());
    }
    Ok(())
}

/// Weather/environment audit (F18-A.1, F18-A.3): the expected
/// denominator is, per stock city, `<stem>.sky`, `<stem>.lt00`..`.lt15`
/// (the measured time×weather preset grid), `<stem>.cpvs`,
/// `<stem>.pvshist`, `<stem>.water`, `<stem>.lmap` and
/// `<stem>_fog.csv` (the authored per-preset fog table — F18-A.3), plus
/// the shared `city/amb_<w><t>_<v>.ldef` grid (measured `w ∈ {c,f,p,r}`,
/// `t ∈ {a,d,m,n}`, `v ∈ {f,l}` — 32 files). Every other discovered
/// environment file (numbered `.cpvs` fog variants, named `.ldef`s,
/// `sf_fog_orig.csv`, `city/phys/j01.sky`, `sf082100.pvshist`, …) is an
/// audited extra — the denominator is never filtered. Cross-checks:
/// `.sky` dome names resolve to `geometry/*.pkg`; `amb_<grid>.ldef`
/// pairs with `texture/sky_<grid>.tex` (measured 32/32 name alignment —
/// inferred pairing, not documented); `.ltNN` block names classify back
/// to the file's `NN` slot; the expected `_fog.csv` row labels are
/// checked against the `.ltNN` name at the same slot (the measured
/// positional mapping — verified 16/16 on both retail cities); and
/// per-city `.cpvs`/`.lmap`/`.pvshist`/`.water` room references are
/// checked against the PSDL room table.
/// `--city` narrows the per-city expected denominator and the PSDL
/// cross-checks; `--strict` exits nonzero on any failure or issue.
fn weather(
    dir: &Path,
    mods: Option<&Path>,
    city: Option<&str>,
    strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use mm2_formats::cpvs::{Cpvs, PvsHist};
    use mm2_formats::fog::FogTable;
    use mm2_formats::ldef::Ldef;
    use mm2_formats::lighting::{LIGHTING_PRESET_COUNT, LightingPreset};
    use mm2_formats::lmap::Lmap;
    use mm2_formats::sky::SkyDef;
    use mm2_formats::water::WaterDef;

    let vfs = build_vfs(dir, mods)?;
    let stems: Vec<String> = match city {
        Some(c) => vec![c.to_ascii_lowercase()],
        None => mm2_content::EXPECTED_CITIES
            .iter()
            .map(|c| c.to_string())
            .collect(),
    };

    let mut expected: Vec<String> = Vec::new();
    for c in &stems {
        for ext in ["sky", "cpvs", "pvshist", "water", "lmap"] {
            expected.push(format!("city/{c}.{ext}"));
        }
        expected.push(format!("city/{c}_fog.csv"));
        for i in 0..LIGHTING_PRESET_COUNT {
            expected.push(format!("city/{c}.lt{i:02}"));
        }
    }
    // Shared ambient-light-definition grid (city/ root, not
    // stem-affiliated): measured 4×4×2 = 32 retail files.
    for w in ['c', 'f', 'p', 'r'] {
        for t in ['a', 'd', 'm', 'n'] {
            for v in ['f', 'l'] {
                expected.push(format!("city/amb_{w}{t}_{v}.ldef"));
            }
        }
    }
    expected.sort();
    expected.dedup();

    let is_env = |p: &str| {
        if !p.starts_with("city/") {
            return false;
        }
        let n = p.rsplit('/').next().unwrap_or(p);
        n.ends_with(".sky")
            || n.ends_with(".ldef")
            || n.ends_with(".cpvs")
            || n.ends_with(".pvshist")
            || n.ends_with(".water")
            || n.ends_with(".lmap")
            || n.ends_with("_fog.csv")
            || (n.ends_with(".csv") && n.contains("_fog_"))
            || n.rsplit_once(".lt")
                .is_some_and(|(_, s)| s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit()))
    };
    let mut logicals: Vec<String> = vfs.list().into_iter().filter(|p| is_env(p)).collect();
    for e in &expected {
        if !logicals.contains(e) {
            logicals.push(e.clone());
        }
    }
    logicals.sort();
    logicals.dedup();
    if logicals.is_empty() {
        return Err("weather audit: no environment files discovered".into());
    }

    let mut failures: Vec<String> = Vec::new();
    let mut issues_total = 0usize;
    let mut parsed = 0usize;
    let mut unsupported = 0usize;

    // Parsed data kept for cross-checks, keyed by city stem.
    let mut skies: Vec<(String, SkyDef)> = Vec::new();
    let mut ldef_stems: Vec<(String, String)> = Vec::new();
    let mut lt_by_city: BTreeMap<String, BTreeMap<usize, (String, LightingPreset)>> =
        BTreeMap::new();
    let mut cpvs_by_city: BTreeMap<String, Vec<(String, Cpvs)>> = BTreeMap::new();
    let mut hist_by_city: BTreeMap<String, Vec<(String, PvsHist)>> = BTreeMap::new();
    let mut water_by_city: BTreeMap<String, (String, WaterDef)> = BTreeMap::new();
    let mut lmap_by_city: BTreeMap<String, (String, Lmap)> = BTreeMap::new();
    let mut fog_tables: Vec<(String, FogTable)> = Vec::new();

    let issue = |issues: &mut usize, msg: String| {
        *issues += 1;
        println!("    issue: {msg}");
    };

    println!("== weather/environment files (sky/ltNN/ldef/cpvs/pvshist/water/lmap/_fog.csv) ==");
    for logical in &logicals {
        let is_expected = expected.contains(logical);
        let tag = if is_expected { "expected" } else { "extra" };
        let Some(res) = vfs.resolve(logical) else {
            println!("  {logical:<52} {tag:<9} missing");
            failures.push(format!("{logical}: expected file not found"));
            continue;
        };
        let bytes = vfs.read(&res)?;
        let name = logical.rsplit('/').next().unwrap_or(logical);
        // City stem for `city/<stem>.<ext>` files directly under city/
        // (`<stem>` or `<stem>_<variant>` / `<stem><digits>`).
        let file_stem = logical
            .strip_prefix("city/")
            .filter(|rest| !rest.contains('/'))
            .and_then(|rest| rest.rsplit_once('.'))
            .map(|(s, _)| s);
        let city_of = |s: &str| -> Option<String> {
            mm2_content::EXPECTED_CITIES
                .iter()
                .find(|c| {
                    s == **c || s.starts_with(&format!("{c}_")) || {
                        s.starts_with(*c) && s[c.len()..].chars().all(|b| b.is_ascii_digit())
                    }
                })
                .map(|c| c.to_string())
        };
        macro_rules! fail {
            ($e:expr) => {
                if is_expected {
                    println!("  {logical:<52} {tag:<9} failed: {}", $e);
                    failures.push(format!("{logical}: {}", $e));
                } else {
                    println!("  {logical:<52} {tag:<9} unsupported: {}", $e);
                    unsupported += 1;
                }
            };
        }
        if name.ends_with(".sky") {
            match SkyDef::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(s) => {
                    parsed += 1;
                    let issues = s.validate();
                    issues_total += issues.len();
                    println!(
                        "  {logical:<52} {tag:<9} ok — dome {:?}, hat_y {}, y_mul {}, rot {}",
                        s.model, s.hat_y_offset, s.y_multiplier, s.rotation_rate
                    );
                    for i in &issues {
                        println!("    issue: {i:?}");
                    }
                    skies.push((logical.clone(), s));
                }
                Err(e) => fail!(e),
            }
        } else if name.ends_with(".ldef") {
            match Ldef::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(l) => {
                    parsed += 1;
                    let stem = l.texture_stem().unwrap_or("?").to_string();
                    println!(
                        "  {logical:<52} {tag:<9} ok — src {:?}, {} row(s)",
                        stem,
                        l.rows.len()
                    );
                    ldef_stems.push((logical.clone(), stem));
                }
                Err(e) => fail!(e),
            }
        } else if let Some((_, suffix)) = name.rsplit_once(".lt") {
            let file_index = suffix.parse::<usize>().unwrap_or(usize::MAX);
            match LightingPreset::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(p) => {
                    parsed += 1;
                    let issues = p.validate();
                    issues_total += issues.len();
                    println!(
                        "  {logical:<52} {tag:<9} ok — {:?}: key h{:.2}/p{:.2} {:?}, ambient 0x{:08X}",
                        p.name, p.key.heading, p.key.pitch, p.key.color, p.ambient_packed as u32
                    );
                    for i in &issues {
                        println!("    issue: {i:?}");
                    }
                    if p.index() != Some(file_index) {
                        issue(
                            &mut issues_total,
                            format!(
                                "{logical}: preset {:?} classifies to slot {:?}, file suffix says {file_index}",
                                p.name,
                                p.index()
                            ),
                        );
                    }
                    if let Some(c) = file_stem.and_then(city_of) {
                        lt_by_city
                            .entry(c)
                            .or_default()
                            .insert(file_index, (logical.clone(), p));
                    }
                }
                Err(e) => fail!(e),
            }
        } else if name.ends_with(".cpvs") {
            match Cpvs::parse(&bytes) {
                Ok(c) => {
                    parsed += 1;
                    // Format violations (unknown 2-bit codes) are issues;
                    // rooms that do not see themselves are authored
                    // anomalies — retail ships a handful per city.
                    let (violations, self_invisible): (Vec<_>, Vec<_>) =
                        c.validate().into_iter().partition(|i| {
                            !matches!(i, mm2_formats::cpvs::CpvsIssue::SelfInvisible { .. })
                        });
                    issues_total += violations.len();
                    let mut nonempty = 0usize;
                    let mut max_len = 0usize;
                    let mut undecodable = 0usize;
                    for i in 0..c.list_count() {
                        match c.decompress(i) {
                            Ok(l) => {
                                if l.iter().any(|&b| b != 0) {
                                    nonempty += 1;
                                }
                                max_len = max_len.max(l.len());
                            }
                            Err(_) => undecodable += 1,
                        }
                    }
                    if undecodable > 0 {
                        issue(
                            &mut issues_total,
                            format!("{logical}: {undecodable} list(s) fail to decompress"),
                        );
                    }
                    println!(
                        "  {logical:<52} {tag:<9} ok — {} lists ({} nonzero), max {} bytes{}",
                        c.list_count(),
                        nonempty,
                        max_len,
                        if self_invisible.is_empty() {
                            String::new()
                        } else {
                            format!(
                                ", {} room(s) not self-visible (authored)",
                                self_invisible.len()
                            )
                        }
                    );
                    for i in &violations {
                        println!("    issue: {i:?}");
                    }
                    if let Some(city) = file_stem.and_then(city_of) {
                        cpvs_by_city
                            .entry(city)
                            .or_default()
                            .push((logical.clone(), c));
                    }
                }
                Err(e) => fail!(e),
            }
        } else if name.ends_with(".pvshist") {
            match PvsHist::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(h) => {
                    parsed += 1;
                    let max_room = h.rows.iter().map(|r| r.from.max(r.to)).max().unwrap_or(0);
                    println!(
                        "  {logical:<52} {tag:<9} ok — {} rows, max room {}",
                        h.rows.len(),
                        max_room
                    );
                    if let Some(c) = file_stem.and_then(city_of) {
                        hist_by_city
                            .entry(c)
                            .or_default()
                            .push((logical.clone(), h));
                    }
                }
                Err(e) => fail!(e),
            }
        } else if name.ends_with("_fog.csv") || (name.ends_with(".csv") && name.contains("_fog_")) {
            match FogTable::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(t) => {
                    parsed += 1;
                    let issues = t.validate();
                    issues_total += issues.len() + t.diagnostics.len();
                    println!(
                        "  {logical:<52} {tag:<9} ok — {} rows{}",
                        t.rows.len(),
                        t.rows
                            .first()
                            .zip(t.rows.last())
                            .map(|(f, l)| format!(" ({:?} … {:?})", f.description, l.description))
                            .unwrap_or_default()
                    );
                    for i in &issues {
                        println!("    issue: {i:?}");
                    }
                    for d in &t.diagnostics {
                        println!("    issue: {d}");
                    }
                    fog_tables.push((logical.clone(), t));
                }
                Err(e) => fail!(e),
            }
        } else if name.ends_with(".water") {
            match WaterDef::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(w) => {
                    parsed += 1;
                    let issues = w.validate();
                    issues_total += issues.len();
                    println!(
                        "  {logical:<52} {tag:<9} ok — level {}, refs {:?}",
                        w.level, w.refs
                    );
                    for i in &issues {
                        println!("    issue: {i:?}");
                    }
                    if let Some(c) = file_stem.and_then(city_of) {
                        water_by_city.insert(c, (logical.clone(), w));
                    }
                }
                Err(e) => fail!(e),
            }
        } else if name.ends_with(".lmap") {
            match Lmap::parse(&bytes) {
                Ok(l) => {
                    parsed += 1;
                    let sentinel = l.entries.iter().filter(|&&v| v == -842150451).count();
                    println!(
                        "  {logical:<52} {tag:<9} ok — {} entries{}",
                        l.entries.len(),
                        if sentinel > 0 {
                            format!(" ({sentinel} 0xCDCDCDCD sentinel value(s))")
                        } else {
                            String::new()
                        }
                    );
                    if let Some(c) = file_stem.and_then(city_of) {
                        lmap_by_city.insert(c, (logical.clone(), l));
                    }
                }
                Err(e) => fail!(e),
            }
        }
    }

    // Cross-checks — authored anomalies and broken references, not load
    // failures.
    println!("  cross-checks:");

    // `.sky` dome → geometry/<model>.pkg.
    for (path, sky) in &skies {
        let pkg = format!("geometry/{}.pkg", sky.model);
        if vfs.resolve(&pkg).is_none() {
            issue(
                &mut issues_total,
                format!("{path}: dome {:?} resolves to no {pkg}", sky.model),
            );
        }
    }

    // `amb_<grid>.ldef` ↔ `texture/sky_<grid>.tex` pairing (measured
    // 32/32 grid-name alignment on retail — inferred).
    for (path, stem) in &ldef_stems {
        let name = path.rsplit('/').next().unwrap_or(path);
        if let Some(grid) = name
            .strip_prefix("amb_")
            .and_then(|s| s.strip_suffix(".ldef"))
        {
            let tex = format!("texture/sky_{grid}");
            if vfs.resolve_preferred(&tex, TEXTURE_EXTS).is_none() {
                issue(
                    &mut issues_total,
                    format!("{path}: no {tex}.* counterpart for grid {grid:?}"),
                );
            }
        } else {
            println!(
                "    note: {path}: bake-source {:?} is a dev path (provenance only, never shipped)",
                stem
            );
        }
    }

    // `.ltNN` slot coverage per city.
    for c in &stems {
        match lt_by_city.get(c) {
            Some(slots) => {
                let missing: Vec<usize> = (0..LIGHTING_PRESET_COUNT)
                    .filter(|i| !slots.contains_key(i))
                    .collect();
                println!(
                    "    lt: {c} — {}/{LIGHTING_PRESET_COUNT} preset slots{}",
                    slots.len(),
                    if missing.is_empty() {
                        String::new()
                    } else {
                        format!(", missing slots {missing:?}")
                    }
                );
            }
            None => println!("    note: {c}: no parsed .ltNN presets"),
        }
    }

    // `_fog.csv` positional mapping: the expected table's row i label
    // must equal the `.ltNN` preset name at slot i (verified 16/16 on
    // both retail cities — the mapping the recovered `lvlSky` fog
    // arrays index by `TimeWeatherType`). Extras like `sf_fog_orig.csv`
    // use a different labelling convention and are not cross-checked.
    for c in &stems {
        let fog_path = format!("city/{c}_fog.csv");
        let Some((_, table)) = fog_tables.iter().find(|(p, _)| *p == fog_path) else {
            continue;
        };
        let mut matched = 0usize;
        for (i, row) in table.rows.iter().enumerate().take(LIGHTING_PRESET_COUNT) {
            let Some(slots) = lt_by_city.get(c) else {
                break;
            };
            let Some((_, preset)) = slots.get(&i) else {
                continue;
            };
            if row.description != preset.name {
                issue(
                    &mut issues_total,
                    format!(
                        "{fog_path}: row {i} labelled {:?}, but {c}.lt{i:02} is {:?} — slot order mismatch",
                        row.description, preset.name
                    ),
                );
            } else {
                matched += 1;
            }
        }
        println!(
            "    fog: {fog_path} — {matched}/{} row labels match the .ltNN slot names",
            LIGHTING_PRESET_COUNT.min(table.rows.len())
        );
    }

    // PSDL room-table cross-checks per city.
    for c in &stems {
        let psdl_path = format!("city/{c}.psdl");
        let Some(pres) = vfs.resolve(&psdl_path) else {
            println!("    note: no {psdl_path} — room counts not cross-checked");
            continue;
        };
        let pbytes = vfs.read(&pres)?;
        let rooms = match Psdl::parse(&pbytes) {
            Ok(p) => p.rooms.len(),
            Err(e) => {
                println!("    note: {psdl_path} failed to parse: {e}");
                continue;
            }
        };
        // `<stem>.cpvs` list count is `rooms + 1` on retail (list 0 is
        // reserved, lists 1..=rooms map to rooms 1..=rooms — mm2hook
        // IsRoomVisible convention, verified against 1340/1341 London
        // self-visible rooms).
        if let Some(cpvs_list) = cpvs_by_city.get(c) {
            for (path, cpvs) in cpvs_list {
                let base = path.rsplit('/').next().unwrap_or(path) == format!("{c}.cpvs").as_str();
                if base && cpvs.list_count() != rooms + 1 {
                    issue(
                        &mut issues_total,
                        format!(
                            "{path}: {} lists but {psdl_path} has {rooms} rooms (expected rooms+1)",
                            cpvs.list_count()
                        ),
                    );
                }
            }
        }
        // `.lmap` is authored with fewer entries than rooms on sf —
        // a count difference is a measured authored fact, not a broken
        // reference.
        if let Some((path, lmap)) = lmap_by_city.get(c) {
            println!(
                "    lmap: {path} — {} entries vs {rooms} rooms{}",
                lmap.entries.len(),
                if lmap.entries.len() != rooms {
                    " (authored mismatch — not all rooms covered)"
                } else {
                    ""
                }
            );
        }
        if let Some(hists) = hist_by_city.get(c) {
            for (path, h) in hists {
                let max_room = h.rows.iter().map(|r| r.from.max(r.to)).max().unwrap_or(0);
                if max_room > rooms as u32 {
                    issue(
                        &mut issues_total,
                        format!(
                            "{path}: references room {max_room} but {psdl_path} has {rooms} rooms"
                        ),
                    );
                }
            }
        }
        if let Some((path, w)) = water_by_city.get(c) {
            for &r in &w.refs {
                if r > rooms as i64 {
                    issue(
                        &mut issues_total,
                        format!("{path}: reference {r} exceeds {rooms} {psdl_path} rooms"),
                    );
                }
            }
        }
        println!("    psdl: {psdl_path} — {rooms} rooms (cross-checked)");
    }

    println!(
        "  {parsed}/{} parsed, {} unsupported extras, {} failures, {issues_total} issue(s)",
        logicals.len(),
        unsupported,
        failures.len(),
    );
    if strict && (issues_total > 0 || !failures.is_empty()) {
        return Err(format!(
            "strict weather audit: {} failures, {issues_total} issues",
            failures.len()
        )
        .into());
    }
    Ok(())
}

/// Breakable/knockable object audit (F04-A.1): the expected denominator
/// is `tune/banger/default.dgbangerdata` (the fallback record the
/// name→record lookup needs); every other discovered `tune/banger/`
/// path is an audited extra — including `.#*` editor backup copies,
/// which parse but are exempt from geometry resolution. Each record
/// stem is classified by [`mm2_formats::banger::stem_role`] and
/// resolved against `geometry/`: own `.pkg` → standalone prop, own
/// `.mtx` → transformed part, `<base>_break<NN>` → a `BREAK<NN>` chunk
/// inside `geometry/<base>.pkg` (or a `.mtx` part transform), and
/// `<base>_<part>` → a `<PART>`/`<PART>_*` chunk inside
/// `geometry/<base>.pkg`. `NumParts` is cross-checked against the
/// standalone's own BREAK chunk count. Records with no resolvable
/// geometry are issues, never hidden. `--strict` exits nonzero on any
/// failure or issue.
fn banger(dir: &Path, mods: Option<&Path>, strict: bool) -> Result<(), Box<dyn std::error::Error>> {
    use mm2_formats::banger::{BangerData, BangerStem, stem_role};

    let vfs = build_vfs(dir, mods)?;
    const EXPECTED: &str = "tune/banger/default.dgbangerdata";
    let mut logicals: Vec<String> = vfs
        .list()
        .into_iter()
        .filter(|p| p.starts_with("tune/banger/"))
        .collect();
    if !logicals.iter().any(|p| p == EXPECTED) {
        logicals.push(EXPECTED.to_string());
    }
    logicals.sort();
    logicals.dedup();

    let mut failures: Vec<String> = Vec::new();
    let mut issues_total = 0usize;
    let mut parsed = 0usize;
    let mut unsupported = 0usize;
    let mut dead_refs = 0usize;
    // Uppercased PKG chunk names, cached per resolved pkg path.
    let mut chunk_cache: BTreeMap<String, Option<BTreeSet<String>>> = BTreeMap::new();
    let mut chunks_of = |vfs: &Vfs, pkg_path: &str| -> Option<BTreeSet<String>> {
        if let Some(hit) = chunk_cache.get(pkg_path) {
            return hit.clone();
        }
        let names = vfs
            .resolve(pkg_path)
            .and_then(|res| vfs.read(&res).ok())
            .and_then(|bytes| Pkg::parse(&bytes).ok())
            .map(|pkg| {
                pkg.files
                    .iter()
                    .map(|f| f.name.to_ascii_uppercase())
                    .collect::<BTreeSet<_>>()
            });
        chunk_cache.insert(pkg_path.to_string(), names.clone());
        names
    };
    // True when `chunks` holds `<prefix>` or `<prefix>_*`.
    let has_chunk = |chunks: &BTreeSet<String>, prefix: &str| {
        chunks.contains(prefix) || chunks.iter().any(|c| c.starts_with(&format!("{prefix}_")))
    };
    // Distinct BREAK<digits> indices among the chunk names.
    let break_indices = |chunks: &BTreeSet<String>| -> BTreeSet<String> {
        chunks
            .iter()
            .filter_map(|c| c.strip_prefix("BREAK"))
            .map(|rest| {
                rest.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .filter(|d| !d.is_empty())
            .collect()
    };

    println!("== banger records (tune/banger/*.dgbangerdata) ==");
    for logical in &logicals {
        let stem = logical
            .trim_start_matches("tune/banger/")
            .trim_end_matches(".dgbangerdata");
        let is_expected = logical == EXPECTED;
        let is_backup = stem.starts_with(".#");
        let tag = if is_expected {
            "expected"
        } else if is_backup {
            "backup"
        } else {
            "extra"
        };
        let Some(res) = vfs.resolve(logical) else {
            println!("  {logical:<58} {tag:<9} missing");
            failures.push(format!("{logical}: expected file not found"));
            continue;
        };
        let bytes = vfs.read(&res)?;
        let text = String::from_utf8_lossy(&bytes);
        let record = match BangerData::parse(&text) {
            Ok(d) => d,
            Err(e) => {
                if is_expected {
                    println!("  {logical:<58} {tag:<9} failed: {e}");
                    failures.push(format!("{logical}: {e}"));
                } else {
                    println!("  {logical:<58} {tag:<9} unsupported: {e}");
                    unsupported += 1;
                }
                continue;
            }
        };
        parsed += 1;
        let mut issues: Vec<String> = record.warnings.clone();
        issues.extend(record.validate().iter().map(|i| i.to_string()));

        // Geometry classification; `.#*` backups are editor leftovers —
        // parsed above, exempt from resolution.
        let class = if is_backup {
            "backup copy".to_string()
        } else {
            match stem_role(stem) {
                BangerStem::Default => "fallback record".to_string(),
                BangerStem::Fragment { base, index } => {
                    if record.num_parts > 0 {
                        issues.push(format!(
                            "fragment record carries NumParts={} with no pkg of its own",
                            record.num_parts
                        ));
                    }
                    let base_pkg = format!("geometry/{base}.pkg");
                    match chunks_of(&vfs, &base_pkg) {
                        Some(chunks) if break_indices(&chunks).iter().any(|d| d == index) => {
                            format!("fragment break{index} of {base} (pkg chunk)")
                        }
                        Some(_) if vfs.resolve(&format!("geometry/{stem}.mtx")).is_some() => {
                            format!("fragment break{index} of {base} (.mtx part)")
                        }
                        Some(_) => {
                            dead_refs += 1;
                            issues.push(format!(
                                "no BREAK{index} chunk in {base_pkg} and no geometry/{stem}.mtx"
                            ));
                            "dead fragment ref".to_string()
                        }
                        None if vfs.resolve(&format!("geometry/{stem}.mtx")).is_some() => {
                            format!("fragment break{index} of {base} (.mtx part)")
                        }
                        None => {
                            dead_refs += 1;
                            issues.push(format!(
                                "fragment of {base}: no {base_pkg} and no geometry/{stem}.mtx"
                            ));
                            "dead fragment ref".to_string()
                        }
                    }
                }
                BangerStem::Named(_) => {
                    if vfs.resolve(&format!("geometry/{stem}.pkg")).is_some() {
                        // Standalone prop: NumParts must equal the BREAK
                        // chunk count in its own pkg.
                        let pkg_path = format!("geometry/{stem}.pkg");
                        let n = chunks_of(&vfs, &pkg_path)
                            .map(|c| break_indices(&c).len())
                            .unwrap_or(0);
                        if record.num_parts != n as i64 {
                            issues.push(format!(
                                "NumParts={} but {pkg_path} carries {n} BREAK chunk(s)",
                                record.num_parts
                            ));
                        }
                        format!("standalone (geometry/{stem}.pkg)")
                    } else if vfs.resolve(&format!("geometry/{stem}.mtx")).is_some() {
                        if record.num_parts > 0 {
                            issues.push(format!(
                                "part record carries NumParts={} with no pkg of its own",
                                record.num_parts
                            ));
                        }
                        format!("part (geometry/{stem}.mtx)")
                    } else {
                        // `<base>_<part>` where the part is a chunk inside
                        // `geometry/<base>.pkg`; try every '_' split,
                        // longest base first.
                        let mut resolved = None;
                        for (i, _) in stem.match_indices('_').collect::<Vec<_>>().iter().rev() {
                            let (base, part) = stem.split_at(*i);
                            let part = &part[1..];
                            let base_pkg = format!("geometry/{base}.pkg");
                            if let Some(chunks) = chunks_of(&vfs, &base_pkg)
                                && has_chunk(&chunks, &part.to_ascii_uppercase())
                            {
                                resolved = Some(format!("part {part} of {base} (pkg chunk)"));
                                break;
                            }
                        }
                        match resolved {
                            Some(r) => r,
                            None => {
                                dead_refs += 1;
                                issues.push(format!(
                                    "no geometry/{stem}.pkg/.mtx and no matching part chunk"
                                ));
                                "dead geometry ref".to_string()
                            }
                        }
                    }
                }
            }
        };

        println!(
            "  {logical:<58} {tag:<9} ok — {class} (mass {:.1}, limit {:.0}, parts {})",
            record.mass, record.impulse_limit2, record.num_parts
        );
        issues_total += issues.len();
        for i in &issues {
            println!("    issue: {i}");
        }
    }

    println!(
        "  {parsed}/{} parsed, {unsupported} unsupported extras, {dead_refs} dead geometry refs, {} failures, {issues_total} issue(s)",
        logicals.len(),
        failures.len(),
    );
    if strict && (issues_total > 0 || !failures.is_empty()) {
        return Err(format!(
            "strict banger audit: {} failures, {issues_total} issues",
            failures.len()
        )
        .into());
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
                    "simulated": w.simulated,
                    "follows": w.follows,
                })).collect::<Vec<_>>(),
            },
            "damage": def.damage.as_ref().map(|d| serde_json::json!({
                "max_damage": d.max_damage,
                "med_damage": d.med_damage,
                "impact_threshold": d.impact_threshold,
                "regenerate_rate": d.regenerate_rate,
                "textel_damage_radius": d.textel_damage_radius,
                "smoke_offset": d.smoke_offset,
                "smoke_offset2": d.smoke_offset2,
            })),
            "stuck": def.stuck.as_ref().map(|s| serde_json::json!({
                "turn": s.turn,
                "rotation": s.rotation,
                "translation": s.translation,
                "time_thresh": s.time_thresh,
                "pos_thresh": s.pos_thresh,
                "move_thresh": s.move_thresh,
            })),
            "gyro": def.gyro.as_ref().map(|g| serde_json::json!({
                "drift": g.drift,
                "spin180": g.spin180,
                "reverse180": g.reverse180,
                "roll": g.roll,
                "pitch": g.pitch,
            })),
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
            let role = if w.simulated {
                "simulated".to_string()
            } else if let Some(r) = w.follows {
                format!("follows whl{r}")
            } else {
                "decorative".to_string()
            };
            println!(
                "  wheel visual {} trailer={} {role} origin {:?} r {:.3} w {:.3} parts {:?}",
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
        match &def.damage {
            Some(d) => println!(
                "damage: max {:.0} med {:.0} threshold {:.0} regen {:.1}/s \
                 decal r {:.2} m, smoke pivots {:?} / {:?}",
                d.max_damage,
                d.med_damage,
                d.impact_threshold,
                d.regenerate_rate,
                d.textel_damage_radius,
                d.smoke_offset,
                d.smoke_offset2
            ),
            None => println!("damage: no vehcardamage record"),
        }
        if let Some(s) = &def.stuck {
            println!(
                "stuck: turn {:.2} rot {:.2} trans {:.2}, time {:.1} s, pos {:.2} move {:.2}",
                s.turn, s.rotation, s.translation, s.time_thresh, s.pos_thresh, s.move_thresh
            );
        }
        if let Some(g) = &def.gyro {
            println!(
                "gyro: drift {:.2} spin180 {:.2} rev180 {:.2} roll {:?} pitch {:?}",
                g.drift, g.spin180, g.reverse180, g.roll, g.pitch
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
