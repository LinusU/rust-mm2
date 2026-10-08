//! Mod manifest (`mod.toml`) handling.
//!
//! Minimal manifest format:
//!
//! ```toml
//! [mod]
//! id = "example-hd-pack"
//! name = "Example HD Pack"
//! version = "0.1.0"
//! effect = "cosmetic" # optional: "cosmetic" or "gameplay"
//! ```
//!
//! `effect` is the author's *claim* about what the mod changes. It never
//! decides anything: the engine classifies a mod by the files it actually
//! wins (`mm2_content::fingerprint::mod_reports`) and reports a claim the
//! files contradict.

use std::path::Path;

use serde::Deserialize;

use crate::AssetsError;

/// Filename of the manifest inside a mod directory.
pub const MANIFEST_FILE: &str = "mod.toml";

/// Largest manifest accepted; a real one is a few hundred bytes.
pub const MAX_MANIFEST_SIZE: u64 = 1024 * 1024;

/// What a mod's author says it changes (`effect` in `mod.toml`).
///
/// A declaration, not a classification: the gameplay/cosmetic verdict
/// comes from the files the mod wins, and a mismatch is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeclaredEffect {
    /// Textures, audio, menu art or lighting only.
    Cosmetic,
    /// Tuning, bounds, geometry, world or race data: records and
    /// multiplayer compatibility are affected.
    Gameplay,
}

impl DeclaredEffect {
    /// The manifest spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cosmetic => "cosmetic",
            Self::Gameplay => "gameplay",
        }
    }
}

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
    /// An unknown spelling fails the parse rather than reading as "none".
    #[serde(default)]
    effect: Option<DeclaredEffect>,
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
    /// The author's claim about what the mod changes, if made.
    pub effect: Option<DeclaredEffect>,
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
            effect: file.section.effect,
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
    fn effect_is_optional_and_only_two_spellings_parse() {
        let parse = |effect: &str| {
            ModManifest::parse(&format!("[mod]\nid = \"m\"\n{effect}\n")).map(|m| m.effect)
        };
        assert_eq!(parse(""), Ok(None));
        assert_eq!(
            parse("effect = \"cosmetic\""),
            Ok(Some(DeclaredEffect::Cosmetic))
        );
        assert_eq!(
            parse("effect = \"gameplay\""),
            Ok(Some(DeclaredEffect::Gameplay))
        );
        // A typo must not read as "no claim".
        assert!(parse("effect = \"cosmectic\"").is_err());
        assert!(parse("effect = \"Cosmetic\"").is_err());
        assert!(parse("effect = true").is_err());
    }

    #[test]
    fn rejects_missing_section() {
        assert!(ModManifest::parse("[other]").is_err());
    }
}
