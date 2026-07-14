//! [`StoreError`] — the typed error surface of `luabox-store`.
//!
//! This crate is a library, so its public API returns a concrete error enum
//! (like the resolver's `ProviderError` and the bundler's `BundleError`) rather
//! than `anyhow::Result`: callers can match on *what* went wrong. `anyhow`
//! lives only in the CLI binary, where errors are rendered, not inspected.
//!
//! Following the workspace convention, the enum is hand-rolled — a `Display`
//! that carries the whole message plus an empty [`std::error::Error`] impl, no
//! `thiserror`. `StoreError` implements `std::error::Error`, so the CLI's `?`
//! and `.context()` keep working unchanged.

use std::fmt;
use std::io;

/// Anything the store's public surface (and its internals) can fail with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// A filesystem/OS operation failed. `context` describes what the store was
    /// doing (verb plus the path(s) it touched); `message` is the OS error
    /// text. An empty `context` renders as the bare OS message.
    Io {
        /// What the store was doing, e.g. `staging "/a" -> "/tmp/a"`.
        context: String,
        /// The underlying OS error, rendered.
        message: String,
    },
    /// [`crate::Store::materialize`] found a manifest entry whose backing object
    /// is absent from the store. Run [`crate::Store::verify`] to tell a missing
    /// object from a corrupt one.
    MissingObject {
        /// The object address that could not be found.
        hash: String,
        /// One tree path that references it, for diagnostics.
        path: String,
    },
    /// A tree path was rejected: it escaped the tree root while interning, or
    /// was unsafe (empty, `.`, `..`) while materializing, or held a component
    /// the store cannot represent.
    InvalidPath {
        /// The full diagnostic (it already names the offending path).
        message: String,
    },
    /// An object was offered under an implausibly short hash — a guard against
    /// a caller passing a non-digest as an address.
    InvalidHash {
        /// The rejected hash.
        hash: String,
    },
    /// Another `gc` holds the advisory lock; refusing to collect concurrently.
    GcLocked {
        /// The lock file another collector owns.
        path: String,
    },
    /// A package-manifest *file* failed to parse or validate; `source` is the
    /// specific reason, and `path` is the file it came from.
    ManifestFile {
        /// The manifest file that could not be loaded.
        path: std::path::PathBuf,
        /// Why it was rejected (a manifest-shaped `StoreError`).
        source: Box<StoreError>,
    },
    /// A manifest was not well-formed JSON (or otherwise structurally invalid);
    /// `message` is the parser's diagnostic.
    InvalidManifest {
        /// The parser/validation diagnostic.
        message: String,
    },
    /// A manifest declared a schema version this build does not understand.
    SchemaVersion {
        /// The version found in the document.
        found: u64,
    },
    /// A required manifest field was absent. `entry` distinguishes a per-file
    /// entry field from a top-level document field.
    MissingField {
        /// The name of the missing field.
        field: String,
        /// Whether the field belongs to a file entry (vs. the document root).
        entry: bool,
    },
    /// A manifest's stored tree hash disagreed with the hash recomputed from its
    /// entries — the file is corrupt or was tampered with.
    TreeHashMismatch {
        /// The hash recorded in the file.
        stored: String,
        /// The hash recomputed from the parsed entries.
        computed: String,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, message } if context.is_empty() => write!(f, "{message}"),
            Self::Io { context, message } => write!(f, "{context}: {message}"),
            Self::MissingObject { hash, path } => {
                write!(f, "object {hash} for {path} is missing from the store")
            }
            Self::InvalidPath { message } | Self::InvalidManifest { message } => {
                write!(f, "{message}")
            }
            Self::InvalidHash { hash } => {
                write!(
                    f,
                    "refusing to store object with implausibly short hash {hash:?}"
                )
            }
            Self::GcLocked { path } => {
                write!(
                    f,
                    "another luabox gc holds {path} — refusing to run concurrently"
                )
            }
            Self::ManifestFile { path, source } => {
                write!(f, "parsing manifest {}: {source}", path.display())
            }
            Self::SchemaVersion { found } => {
                write!(f, "unsupported manifest schema version {found}")
            }
            Self::MissingField { field, entry: true } => write!(f, "entry missing '{field}'"),
            Self::MissingField {
                field,
                entry: false,
            } => write!(f, "missing '{field}'"),
            Self::TreeHashMismatch { stored, computed } => {
                write!(
                    f,
                    "manifest tree-hash mismatch: stored {stored}, computed {computed}"
                )
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    /// A bare `?` on an OS error carries no operation context — the message is
    /// the OS error alone (matching how these sites read before the switch off
    /// `anyhow`). Prefer [`IoResultExt::io_context`] to attach a verb.
    fn from(err: io::Error) -> Self {
        Self::Io {
            context: String::new(),
            message: err.to_string(),
        }
    }
}

/// Attach an operation context to an [`io::Error`], mirroring the shape of
/// `anyhow`'s `.with_context()` so the store's error sites read as they did.
pub(crate) trait IoResultExt<T> {
    /// Map an OS error into [`StoreError::Io`] with a lazily-built context.
    fn io_context(self, context: impl FnOnce() -> String) -> Result<T, StoreError>;
}

impl<T> IoResultExt<T> for Result<T, io::Error> {
    fn io_context(self, context: impl FnOnce() -> String) -> Result<T, StoreError> {
        self.map_err(|err| StoreError::Io {
            context: context(),
            message: err.to_string(),
        })
    }
}
