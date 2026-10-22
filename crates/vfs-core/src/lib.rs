//! Protocol-agnostic async virtual filesystem core for Veloce.
//!
//! The crate is deliberately GUI-free: everything here can be driven from a CLI,
//! a daemon or the GPUI front-end. The public surface is:
//!
//! - [`VfsUri`] / [`ProtocolScheme`]: a single resource identifier for every backend.
//! - [`VfsProvider`]: the async trait every backend implements (local, SFTP, S3, WebDAV, ...).
//! - [`RemoteProvider`]: SFTP, S3 and WebDAV through OpenDAL, one per saved connection.
//! - [`LocalProvider`]: a deeply optimized native provider for `file://` resources.
//! - [`Filter`] / [`search`]: `nucleo`-backed fuzzy filtering and recursive search.
//! - [`TaskManager`]: background transfer queue with progress, cancellation and byte accounting.

pub mod archive;
pub mod entry;
pub mod error;
pub mod filter;
pub mod local;
pub mod ops;
pub mod provider;
pub mod remote;
pub mod search;
pub mod transfer;
pub mod uri;
pub mod watch;

pub use entry::{FileEntry, FileKind, SortKey, SortSpec};
pub use error::{Error, Result};
pub use filter::Filter;
pub use local::LocalProvider;
pub use ops::{TaskKind, TaskManager, TaskSnapshot, TaskState};
pub use provider::{ProviderRegistry, VfsProvider};
pub use remote::{RemoteProvider, RemoteSpec};
pub use search::{SearchHit, search_recursive};
pub use uri::{ProtocolScheme, VfsUri};
pub use watch::{FsWatcher, WatchEvent};
