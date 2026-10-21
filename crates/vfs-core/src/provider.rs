use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;

use crate::{
    entry::FileEntry,
    error::{Error, Result},
    local::LocalProvider,
    uri::{ProtocolScheme, VfsUri},
};

/// One chunk of a streamed directory listing.
///
/// The provider decides the chunk size (the local provider uses 512 entries) so the
/// UI can paint the first screen in a few milliseconds while the rest keeps arriving.
pub type ListChunk = Vec<FileEntry>;

/// The async contract every backend implements.
///
/// Everything is streaming and cancellation-friendly: no method may block the
/// caller's thread and no method returns "all entries" as one giant `Vec`.
#[async_trait]
pub trait VfsProvider: Send + Sync {
    fn scheme(&self) -> ProtocolScheme;

    /// Stream a directory, chunk by chunk, in natural order.
    async fn list_stream(&self, uri: &VfsUri) -> Result<BoxStream<'static, Result<ListChunk>>>;

    /// Read a single entry's metadata.
    async fn stat(&self, uri: &VfsUri) -> Result<FileEntry>;

    /// Read a byte range. `length: None` means "to end of file".
    async fn read_stream(
        &self,
        uri: &VfsUri,
        offset: u64,
        length: Option<u64>,
    ) -> Result<BoxStream<'static, Result<Bytes>>>;

    /// Write a whole file from a stream, reporting bytes written as they land.
    async fn write_stream(
        &self,
        uri: &VfsUri,
        stream: BoxStream<'static, Result<Bytes>>,
        total_size: Option<u64>,
        on_progress: Option<Box<dyn Fn(u64) + Send + Sync>>,
    ) -> Result<()>;

    /// Server-side move (rename). Zero-copy within one backend.
    async fn rename(&self, from: &VfsUri, to: &VfsUri) -> Result<()>;

    async fn delete(&self, uri: &VfsUri, recursive: bool) -> Result<()>;

    /// Create a directory (and missing parents).
    async fn create_dir(&self, _uri: &VfsUri) -> Result<()> {
        Err(Error::Unsupported {
            scheme: self.scheme().to_string(),
            operation: "create_dir",
        })
    }

    /// Cheap capability probe used by the UI to hide impossible actions.
    fn supports(&self, _capability: Capability) -> bool {
        true
    }
}

/// Optional backend features, so the UI never offers an action a backend cannot do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Server-side rename/move without a byte transfer.
    ServerSideRename,
    /// Real OS trash (vs. hard delete).
    Trash,
    /// Multipart upload with resumable parts.
    MultipartUpload,
    /// Filesystem change notifications.
    Watch,
    /// Permission bits / modes.
    Permissions,
}

/// Holds one provider per scheme and hands out the right one for a uri.
#[derive(Clone)]
pub struct ProviderRegistry {
    providers: HashMap<ProtocolScheme, Arc<dyn VfsProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Registry with only the native local provider wired up.
    pub fn with_local() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(LocalProvider::new()));
        registry
    }

    pub fn register(&mut self, provider: Arc<dyn VfsProvider>) {
        self.providers.insert(provider.scheme(), provider);
    }

    pub fn get(&self, scheme: ProtocolScheme) -> Result<Arc<dyn VfsProvider>> {
        self.providers
            .get(&scheme)
            .cloned()
            .ok_or_else(|| Error::Unsupported {
                scheme: scheme.to_string(),
                operation: "mount",
            })
    }

    pub fn for_uri(&self, uri: &VfsUri) -> Result<Arc<dyn VfsProvider>> {
        self.get(uri.scheme)
    }

    pub fn mounted(&self) -> Vec<ProtocolScheme> {
        let mut schemes: Vec<_> = self.providers.keys().copied().collect();
        schemes.sort();
        schemes
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalProvider;

    #[test]
    fn empty_registry_reports_unsupported() {
        let registry = ProviderRegistry::new();
        assert!(registry.mounted().is_empty());
        let Err(error) = registry.get(ProtocolScheme::File) else {
            panic!("an empty registry must not resolve file uris");
        };
        assert!(matches!(error, Error::Unsupported { .. }));
        assert!(error.to_string().contains("file"));
    }

    #[test]
    fn local_registry_resolves_file_uris() {
        let registry = ProviderRegistry::with_local();
        assert_eq!(registry.mounted(), vec![ProtocolScheme::File]);
        let provider = registry.for_uri(&VfsUri::local("/tmp")).unwrap();
        assert_eq!(provider.scheme(), ProtocolScheme::File);
    }

    #[test]
    fn remote_schemes_are_unsupported_until_mounted() {
        let registry = ProviderRegistry::with_local();
        for scheme in [
            ProtocolScheme::Sftp,
            ProtocolScheme::S3,
            ProtocolScheme::WebDav,
            ProtocolScheme::Ftp,
        ] {
            assert!(
                registry.get(scheme).is_err(),
                "{scheme} must not resolve before its provider is registered"
            );
        }
    }

    #[test]
    fn register_replaces_by_scheme() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(LocalProvider::new()));
        registry.register(Arc::new(LocalProvider::new()));
        assert_eq!(registry.mounted().len(), 1);
    }

    #[test]
    fn default_registry_is_empty() {
        assert!(ProviderRegistry::default().mounted().is_empty());
    }
}
