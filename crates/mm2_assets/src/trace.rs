//! Dependency tracing: which files a load actually read, and from which
//! source. The VFS records every read made while a trace is open, so a
//! mod author can see what a car or a city pulled in without the loader
//! having to report it.

use std::collections::BTreeMap;

use crate::ResolvedSource;

/// One read made through the VFS while a trace was open.
#[derive(Debug, Clone)]
pub struct Access {
    /// Normalized logical path that was asked for.
    pub logical: String,
    /// The source that served it; `None` when no mounted source provides
    /// the path (a miss — optional files show up here).
    pub source: Option<ResolvedSource>,
    /// Bytes returned; `None` when the read failed (a miss, or a source
    /// error such as an oversize or truncated file).
    pub len: Option<usize>,
}

impl Access {
    /// Heading naming the source: the mod id, the install, or `(not found)`.
    pub fn origin(&self) -> String {
        match &self.source {
            None => "(not found)".to_string(),
            Some(s) => match &s.label {
                Some(id) => format!("mod `{id}`"),
                None => format!("original ({})", s.path.display()),
            },
        }
    }
}

/// Every read of one traced load, in the order they happened.
#[derive(Debug, Clone, Default)]
pub struct ReadTrace {
    /// The reads, repeated reads of one path included.
    pub accesses: Vec<Access>,
}

impl ReadTrace {
    /// The reads grouped by origin, each file once (the first read's
    /// outcome stands), origins in name order and files in logical order.
    pub fn by_origin(&self) -> BTreeMap<String, Vec<&Access>> {
        let mut groups: BTreeMap<String, Vec<&Access>> = BTreeMap::new();
        for a in &self.accesses {
            let group = groups.entry(a.origin()).or_default();
            if !group.iter().any(|seen| seen.logical == a.logical) {
                group.push(a);
            }
        }
        for files in groups.values_mut() {
            files.sort_by(|a, b| a.logical.cmp(&b.logical));
        }
        groups
    }

    /// Distinct logical paths read from mod `id`.
    pub fn from_mod(&self, id: &str) -> Vec<&str> {
        let mut paths: Vec<&str> = self
            .accesses
            .iter()
            .filter(|a| a.source.as_ref().and_then(|s| s.label.as_deref()) == Some(id))
            .map(|a| a.logical.as_str())
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }

    /// Distinct logical paths that were asked for and not provided.
    pub fn missing(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self
            .accesses
            .iter()
            .filter(|a| a.source.is_none())
            .map(|a| a.logical.as_str())
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }

    /// The mods among `expected` that served no file — the ones a
    /// `--expect-mod` check reports as not live for this load.
    pub fn absent_mods<'a>(&self, expected: &'a [String]) -> Vec<&'a String> {
        expected
            .iter()
            .filter(|m| self.from_mod(m).is_empty())
            .collect()
    }

    /// The lines that follow [`render`](Self::render): each path no source
    /// provided, then one summary naming `label`, the files read, the
    /// sources that served them and the paths nobody provided.
    pub fn render_summary(&self, label: &str) -> String {
        let missing = self.missing();
        let mut out = String::new();
        for logical in &missing {
            out.push_str(&format!("(not found): {logical}\n"));
        }
        let origins = self.by_origin();
        out.push_str(&format!(
            "{label}: {} file(s) from {} source(s), {} not provided\n",
            origins.values().map(Vec::len).sum::<usize>() - missing.len(),
            origins.keys().filter(|k| *k != "(not found)").count(),
            missing.len()
        ));
        out
    }

    /// Human-readable report: a block per origin listing its files.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (origin, files) in self.by_origin() {
            out.push_str(&format!("{origin}: {} file(s)\n", files.len()));
            for a in files {
                match a.len {
                    Some(n) => out.push_str(&format!("  {} ({n} bytes)\n", a.logical)),
                    None => out.push_str(&format!("  {} (read failed)\n", a.logical)),
                }
            }
        }
        out
    }
}
