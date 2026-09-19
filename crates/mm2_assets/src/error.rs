use std::path::PathBuf;

/// Errors produced by the VFS and asset resolution layer.
#[derive(Debug, thiserror::Error)]
pub enum AssetsError {
    /// An underlying I/O error.
    #[error("io error on {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// The I/O error.
        source: std::io::Error,
    },

    /// A logical path failed normalization (traversal, absolute, ...).
    #[error("invalid logical path {0:?}")]
    InvalidPath(String),

    /// The requested asset exists in no mounted source.
    #[error("asset not found: {0}")]
    NotFound(String),

    /// A mounted archive could not be parsed.
    #[error("failed to parse archive {path}: {source}")]
    Archive {
        /// Archive path.
        path: PathBuf,
        /// Parse error.
        source: mm2_formats::FormatError,
    },

    /// Stored archive data failed to decompress.
    #[error("failed to decompress {logical}: {reason}")]
    Decompression {
        /// Logical path of the entry.
        logical: String,
        /// Reason.
        reason: String,
    },

    /// A mod manifest could not be read or parsed.
    #[error("failed to load mod manifest {path}: {reason}")]
    ModManifest {
        /// Manifest path.
        path: PathBuf,
        /// Reason.
        reason: String,
    },

    /// A mounted directory does not exist.
    #[error("directory not found: {0}")]
    MissingDirectory(PathBuf),
}

impl AssetsError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}
