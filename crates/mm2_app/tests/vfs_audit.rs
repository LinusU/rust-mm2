//! F29-A (requirement 1): no feature code opens an original file directly.
//!
//! Every original resource must resolve through the VFS so a mod can
//! replace it and provenance is recorded. The audit reads the workspace's
//! own source and finds every production filesystem touch (`std::fs`,
//! `File::…`, `read_dir`, `.exists()`/`.is_file()`/`.is_dir()`,
//! `canonicalize`, …). Each file that has one must be listed below with the
//! reason it is not an original-content read; a new one fails the test until
//! someone classifies it. A listed file that no longer touches the
//! filesystem fails too, so the table cannot rot into a blanket permission.
//!
//! This is a source scan: it proves where the filesystem is reached from,
//! not that every consumer honours an override (F29-AC01 still needs the
//! per-consumer mod runs).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Why a file may touch the filesystem. None of these is "reads original
/// game data": that is `Vfs` only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    /// The VFS itself: mounting, listing and reading sources.
    Vfs,
    /// The player's own writable data (profile, settings, bindings).
    UserData,
    /// Output the developer asked for (screenshot, perf log, smoke record,
    /// probe dump) or a guard over where such output may land.
    DevOutput,
    /// A file the developer named on the command line (script, tuning
    /// override); never an install path.
    DevInput,
    /// The app's own shipped synthetic assets, located before mounting.
    OwnAssets,
}

/// Every production file allowed to reach the filesystem, relative to the
/// repository root.
const ALLOWED: &[(&str, Reason)] = &[
    ("crates/mm2_assets/src/fingerprint.rs", Reason::Vfs),
    ("crates/mm2_assets/src/manifest.rs", Reason::Vfs),
    ("crates/mm2_assets/src/mount.rs", Reason::Vfs),
    ("crates/mm2_assets/src/source.rs", Reason::Vfs),
    ("crates/mm2_assets/src/vfs.rs", Reason::Vfs),
    ("crates/mm2_game/src/profile.rs", Reason::UserData),
    ("crates/mm2_app/src/settings.rs", Reason::UserData),
    ("crates/mm2_app/src/controls.rs", Reason::UserData),
    ("crates/mm2_app/src/perf.rs", Reason::DevOutput),
    ("crates/mm2_app/src/smoke.rs", Reason::DevOutput),
    ("crates/mm2_app/src/main.rs", Reason::DevOutput),
    ("crates/mm2_app/src/write_guard.rs", Reason::DevOutput),
    ("crates/mm2_app/examples/analyze_city.rs", Reason::DevOutput),
    ("crates/mm2_app/examples/drive_probe.rs", Reason::DevOutput),
    ("crates/mm2_app/src/scripted.rs", Reason::DevInput),
    ("crates/mm2_vehicle/src/config.rs", Reason::DevInput),
    (
        "crates/mm2_formats/examples/dump_psdl_attrs.rs",
        Reason::DevInput,
    ),
    ("crates/mm2_app/src/app_assets.rs", Reason::OwnAssets),
];

/// Substrings that mean "this line reaches the filesystem".
const FS_MARKERS: &[&str] = &[
    "std::fs",
    "File::open",
    "File::create",
    "OpenOptions::new",
    "read_dir",
    ".exists()",
    ".is_file()",
    ".is_dir()",
    ".canonicalize()",
    "read_to_string",
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// Whether a line names the `fs` module (`fs::read`, `{fs::File}`) — not an
/// identifier merely ending in it, such as `Vfs::new`.
fn names_fs_module(line: &str) -> bool {
    line.match_indices("fs::").any(|(i, _)| {
        !line[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// The production lines of a source file that touch the filesystem: stops at
/// the first `#[cfg(test)]` (test modules sit at the end of each file here)
/// and skips comment lines.
fn fs_touches(source: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    for (i, line) in source.lines().enumerate() {
        if line.starts_with("#[cfg(test)]") {
            break;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        if names_fs_module(line) || FS_MARKERS.iter().any(|m| line.contains(m)) {
            found.push((i + 1, line.trim()));
        }
    }
    found
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Production sources: each crate's `src/` and `examples/` (integration
/// tests are test code and not scanned).
fn production_sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let crates = repo().join("crates");
    for krate in std::fs::read_dir(&crates).expect("crates dir").flatten() {
        for sub in ["src", "examples"] {
            rust_files(&krate.path().join(sub), &mut files);
        }
    }
    files.sort();
    files
}

fn relative(path: &Path) -> String {
    path.strip_prefix(repo())
        .expect("under repo")
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn only_classified_files_reach_the_filesystem() {
    let allowed: BTreeMap<&str, Reason> = ALLOWED.iter().copied().collect();
    assert_eq!(allowed.len(), ALLOWED.len(), "a file is listed twice");

    let sources = production_sources();
    assert!(
        sources.len() > 100,
        "the scan found only {} files — the walk is broken, not the repo clean",
        sources.len()
    );

    let mut unclassified = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for path in &sources {
        let text = std::fs::read_to_string(path).expect("read source");
        let touches = fs_touches(&text);
        if touches.is_empty() {
            continue;
        }
        let rel = relative(path);
        seen.insert(rel.clone(), touches.len());
        if !allowed.contains_key(rel.as_str()) {
            for (line, text) in touches {
                unclassified.push(format!("{rel}:{line}: {text}"));
            }
        }
    }
    assert!(
        unclassified.is_empty(),
        "production code reaches the filesystem outside the audited set; if it reads \
         original content it must go through mm2_assets::Vfs, otherwise classify it in \
         ALLOWED:\n{}",
        unclassified.join("\n")
    );

    let stale: Vec<&str> = allowed
        .keys()
        .copied()
        .filter(|f| !seen.contains_key(*f))
        .collect();
    assert!(
        stale.is_empty(),
        "listed files no longer touch the filesystem (or moved); remove them: {stale:?}"
    );
}

#[test]
fn content_and_format_crates_never_touch_the_filesystem() {
    // The parsers and the content layer take bytes or a `Vfs`; neither may
    // gain a direct open, whatever the allowlist says elsewhere.
    for (file, reason) in ALLOWED {
        let in_pure_crate = ["crates/mm2_content/", "crates/mm2_net/"]
            .iter()
            .any(|c| file.starts_with(c))
            || (file.starts_with("crates/mm2_formats/src/"));
        assert!(
            !in_pure_crate,
            "{file} ({reason:?}) is in a filesystem-free crate"
        );
    }
    // Only the VFS crate may carry the `Vfs` reason.
    for (file, reason) in ALLOWED {
        if *reason == Reason::Vfs {
            assert!(file.starts_with("crates/mm2_assets/src/"), "{file}");
        }
    }
}

#[test]
fn the_scanner_flags_a_direct_open_and_ignores_tests_and_comments() {
    let source = "\
// std::fs::read in a comment is not a touch
fn load(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}
fn probe(p: &Path) -> bool { p.is_file() }
fn mount() -> Vfs { Vfs::new() }
fn short() -> Vec<u8> { fs::read(\"x\").unwrap() }
#[cfg(test)]
mod tests {
    fn fixture() { std::fs::write(\"x\", b\"y\").unwrap(); }
}
";
    let touches = fs_touches(source);
    assert_eq!(
        touches.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
        vec![3, 5, 7],
        "{touches:?}"
    );
    assert!(fs_touches("fn pure(bytes: &[u8]) -> usize { bytes.len() }").is_empty());
}
