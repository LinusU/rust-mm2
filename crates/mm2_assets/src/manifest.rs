//! Mod manifest (`mod.toml`) handling.
//!
//! Minimal manifest format:
//!
//! ```toml
//! [mod]
//! id = "example-hd-pack"
//! name = "Example HD Pack"
//! version = "0.1.0"
//! ```

use std::path::Path;

use serde::Deserialize;

use crate::AssetsError;

/// Filename of the manifest inside a mod directory.
pub const MANIFEST_FILE: &str = "mod.toml";

/// Largest manifest accepted; a real one is a few hundred bytes.
pub const MAX_MANIFEST_SIZE: u64 = 1024 * 1024;

#[derive(Debug, Deserialize)]
struct ManifestFile {
    #[serde(rename = "mod")]
    section: ModSection,
}

#[derive(Debug, Deserialize)]
struct ModSection {
    id: String,
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
}

/// A loaded mod manifest.
#[derive(Debug, Clone)]
pub struct ModManifest {
    /// Unique mod identifier (directory-name-safe).
    pub id: String,
    /// Human-readable name; falls back to the id.
    pub name: String,
    /// Version string as written by the author.
    pub version: String,
    /// Optional description.
    pub description: Option<String>,
    /// Optional author list.
    pub authors: Vec<String>,
}

impl ModManifest {
    /// Load `dir/mod.toml`.
    pub fn load(dir: &Path) -> Result<Self, AssetsError> {
        let path = dir.join(MANIFEST_FILE);
        // Same containment policy as the mounted tree: a manifest that is a
        // link could point at any file the process can read.
        let meta = std::fs::symlink_metadata(&path).map_err(|e| AssetsError::ModManifest {
            path: path.clone(),
            reason: e.to_string(),
        })?;
        if !meta.is_file() {
            return Err(AssetsError::ModManifest {
                path,
                reason: "manifest is not a regular file (symlinks are not followed)".to_string(),
            });
        }
        let bytes = crate::source::read_bounded(&path, MANIFEST_FILE, MAX_MANIFEST_SIZE)?;
        let text = String::from_utf8(bytes).map_err(|e| AssetsError::ModManifest {
            path: path.clone(),
            reason: e.to_string(),
        })?;
        Self::parse(&text).map_err(|reason| AssetsError::ModManifest {
            path: path.clone(),
            reason,
        })
    }

    /// Parse manifest contents directly.
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: ManifestFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let id = file.section.id.trim().to_string();
        if id.is_empty() {
            return Err("mod id must not be empty".to_string());
        }
        Ok(Self {
            name: file.section.name.unwrap_or_else(|| id.clone()),
            version: file.section.version.unwrap_or_else(|| "0.0.0".to_string()),
            description: file.section.description,
            authors: file.section.authors,
            id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let m = ModManifest::parse(
            r#"
            [mod]
            id = "example-hd-pack"
            name = "Example HD Pack"
            version = "0.1.0"
            "#,
        )
        .unwrap();
        assert_eq!(m.id, "example-hd-pack");
        assert_eq!(m.name, "Example HD Pack");
        assert_eq!(m.version, "0.1.0");
    }

    #[test]
    fn rejects_missing_section() {
        assert!(ModManifest::parse("[other]").is_err());
    }
}
