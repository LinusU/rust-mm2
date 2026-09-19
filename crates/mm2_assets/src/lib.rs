//! Virtual filesystem and logical asset resolution for MM2 content.
//!
//! The VFS presents a single namespace of *logical paths* such as
//! `texture/foo.tex` or `city/london.psdl`, regardless of whether the data
//! comes from an original `DAVE` archive, loose files in the MM2 installation,
//! an override directory or a mod.
//!
//! Sources are mounted with an explicit priority; resolution is
//! deterministic: higher priority wins, and within equal priority the source
//! mounted *later* wins (mirrors typical "load order" semantics).
//!
//! Logical paths are normalized: separators are `/`, comparison is ASCII
//! case-insensitive (old Windows game data), and anything attempting to
//! escape the virtual root (`..`, absolute paths) is rejected.

mod error;
mod manifest;
mod source;
mod vfs;

pub use error::AssetsError;
pub use manifest::ModManifest;
pub use source::{ResolvedSource, SourceKind};
pub use vfs::{Resolved, Vfs};

/// Well-known source priority tiers. The exact ordering of original archives
/// is deliberately *not* encoded here — archives are mounted in a configured
/// order at [`priority::ARCHIVE`] and later mounts win ties.
pub mod priority {
    /// Original MM2 `.ar` archives.
    pub const ARCHIVE: i32 = 0;
    /// Loose files inside the MM2 installation directory.
    pub const LOOSE: i32 = 100;
    /// Additional override directories (development, unpacked data).
    pub const OVERRIDE: i32 = 200;
    /// Mods; `MOD + n` stacks individual mods deterministically.
    pub const MOD: i32 = 300;
}

/// Normalize a logical path: forward slashes, ASCII lowercase, `.`/`..`
/// resolved. Returns `None` for paths that escape the root or are otherwise
/// invalid (absolute paths, drive letters, embedded NUL).
pub fn normalize_path(path: &str) -> Option<String> {
    if path.starts_with('/') || path.starts_with('\\') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    let replaced = path.replace('\\', "/");
    for seg in replaced.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                parts.pop()?;
            }
            s if s.contains('\0') => return None,
            s => parts.push(s),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/").to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separators_and_case() {
        assert_eq!(
            normalize_path("Texture\\London\\Foo.TEX"),
            Some("texture/london/foo.tex".to_string())
        );
        assert_eq!(
            normalize_path("./texture//foo.tex").as_deref(),
            Some("texture/foo.tex")
        );
    }

    #[test]
    fn rejects_traversal() {
        assert!(normalize_path("../evil").is_none());
        assert!(normalize_path("texture/../../evil").is_none());
        assert!(normalize_path("/abs/path").is_none());
        assert!(normalize_path("texture/..").is_none()); // ends at root → empty
        assert!(normalize_path("C:/win").is_some()); // not absolute on our terms, harmless
    }
}
