//! Deterministic content fingerprints over the VFS (F24-A, F29-AC05).
//!
//! A fingerprint lets two processes compare "what content would we
//! resolve" without exchanging the content itself — the network
//! handshake's compatibility gate and `mm2-inspect`'s install audit
//! share this implementation so they can never disagree about the
//! answer.
//!
//! [`catalog`] is a *structural* fingerprint: it covers the resolved
//! logical-path set, each winner's provenance (physical path, archive
//! offset, mod label) and the winning file's size — but not its bytes.
//! It is cheap and identifies an installation's resolution map exactly;
//! it is not a content hash.
//!
//! Content-level fingerprints (hashing resolved bytes for a classified
//! path subset, e.g. gameplay-relevant files) are built by
//! `mm2_content::fingerprint` on top of [`fnv`].

use std::collections::BTreeMap;

use crate::Vfs;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One FNV-1a-64 step: fold `bytes` into `h`.
pub fn fnv(h: u64, bytes: impl AsRef<[u8]>) -> u64 {
    bytes
        .as_ref()
        .iter()
        .fold(h, |h, b| h.wrapping_mul(FNV_PRIME) ^ u64::from(*b))
}

/// The FNV-1a-64 offset basis — the seed every fingerprint starts from.
pub const FNV_OFFSET_BASIS: u64 = FNV_OFFSET;

/// Format a completed hash the way reports present it (`fnv1a64:<hex>`).
pub fn display(h: u64) -> String {
    format!("fnv1a64:{h:016x}")
}

/// FNV-1a-64 over every resolved logical path, its provenance and the
/// winning source's file size. Deterministic for a given catalog; not a
/// content hash (bytes are not read).
pub fn catalog(vfs: &Vfs) -> String {
    let mut h = FNV_OFFSET;
    let mut size_cache: BTreeMap<std::path::PathBuf, u64> = BTreeMap::new();
    for logical in vfs.list() {
        h = fnv(h, &logical);
        let Some(r) = vfs.resolve(&logical) else {
            continue;
        };
        h = fnv(h, r.source.path.to_string_lossy().as_bytes());
        if let Some(off) = r.source.archive_offset {
            h = fnv(h, (off as u64).to_le_bytes());
        }
        if let Some(label) = &r.source.label {
            h = fnv(h, label);
        }
        let size = size_cache.entry(r.source.path.clone()).or_insert_with(|| {
            std::fs::metadata(&r.source.path)
                .map(|m| m.len())
                .unwrap_or(0)
        });
        h = fnv(h, size.to_le_bytes());
    }
    display(h)
}
