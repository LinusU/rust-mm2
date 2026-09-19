//! Parser for MM2 vehicle metadata files (`tune/<id>.info`, `.inf`,
//! `.vinfo`).
//!
//! The format is line-oriented `Key=Value` text. Keys may contain spaces
//! (`Top Speed`), values may contain spaces and pipe-separated lists
//! (`Colors=Red|Blue`). Encoding is effectively ASCII/Latin-1; callers should
//! pass `String::from_utf8_lossy` output.

use std::fmt;

/// A parsed metadata file: ordered fields plus non-fatal diagnostics.
#[derive(Debug, Clone)]
pub struct InfoFile {
    pub fields: Vec<InfoField>,
    /// Recoverable problems (lines without `=`, duplicate keys, ...).
    pub diagnostics: Vec<InfoDiagnostic>,
}

/// One `Key=Value` line.
#[derive(Debug, Clone)]
pub struct InfoField {
    pub key: String,
    pub value: String,
    pub line: u32,
}

/// Non-fatal parse problem.
#[derive(Debug, Clone)]
pub struct InfoDiagnostic {
    pub line: u32,
    pub message: String,
}

impl fmt::Display for InfoDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl InfoFile {
    pub fn parse(input: &str) -> Self {
        let mut fields = Vec::new();
        let mut diagnostics = Vec::new();
        for (idx, raw_line) in input.lines().enumerate() {
            let line = (idx + 1) as u32;
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // `//` comment lines.
            if trimmed.starts_with("//") {
                continue;
            }
            let Some(eq) = trimmed.find('=') else {
                diagnostics.push(InfoDiagnostic {
                    line,
                    message: format!("ignoring line without '=': {trimmed:?}"),
                });
                continue;
            };
            let key = trimmed[..eq].trim().to_string();
            let value = trimmed[eq + 1..].trim().to_string();
            if key.is_empty() {
                diagnostics.push(InfoDiagnostic {
                    line,
                    message: "ignoring line with empty key".into(),
                });
                continue;
            }
            if fields.iter().any(|f: &InfoField| f.key == key) {
                diagnostics.push(InfoDiagnostic {
                    line,
                    message: format!("duplicate key {key:?}; first value wins"),
                });
            }
            fields.push(InfoField { key, value, line });
        }
        InfoFile {
            fields,
            diagnostics,
        }
    }

    /// First value for an exact key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.as_str())
    }

    /// First value for a case-insensitive key.
    pub fn get_ci(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.key.eq_ignore_ascii_case(key))
            .map(|f| f.value.as_str())
    }

    /// Value parsed as `f32`.
    pub fn f32(&self, key: &str) -> Option<f32> {
        self.get(key)?.trim().parse().ok()
    }

    /// Value parsed as `u32`.
    pub fn u32(&self, key: &str) -> Option<u32> {
        self.get(key)?.trim().parse().ok()
    }

    /// Pipe-separated list value (e.g. `Colors=Yellow|Blue`).
    pub fn list(&self, key: &str) -> Vec<String> {
        self.get(key)
            .map(|v| {
                v.split('|')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// All keys present, for unknown-field diagnostics.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(|f| f.key.as_str())
    }
}
