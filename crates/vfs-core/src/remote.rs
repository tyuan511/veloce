//! Remote backends (SFTP, S3, WebDAV) on top of OpenDAL.
//!
//! One [`RemoteProvider`] wraps one configured `Operator`, so every saved
//! connection gets its own provider and its own credentials. Local files stay on the
//! native [`crate::local::LocalProvider`]; only the network goes through here.
//!
//! OpenDAL's transports (reqwest, openssh) need a tokio reactor, but the app runs on
//! GPUI's executor. Every call is therefore spawned onto a small private runtime and
//! only the results cross back, which keeps this module executor-agnostic for callers.

use std::{future::Future, ops::Bound, path::PathBuf, sync::OnceLock, time::SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use futures::{StreamExt, stream::BoxStream};
use opendal::{EntryMode, ErrorKind, Operator, services};

use crate::{
    entry::{FileEntry, FileKind},
    error::{Error, Result},
    provider::{Capability, ListChunk, VfsProvider},
    uri::{ProtocolScheme, VfsUri},
};

/// Entries per streamed chunk, same as the local provider so the pane paints alike.
const CHUNK_SIZE: usize = 512;

/// Chunks buffered between the network task and the consumer. Small on purpose: a
/// slow consumer should slow the download down, not fill memory.
const CHANNEL_DEPTH: usize = 4;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("veloce-remote")
            .enable_all()
            .build()
            .expect("the remote I/O runtime must start")
    })
}

/// Run `future` on the remote runtime and hand its result back to the caller's
/// executor.
async fn on_runtime<T, F>(future: F) -> Result<T>
where
    T: Send + 'static,
    F: Future<Output = Result<T>> + Send + 'static,
{
    runtime()
        .spawn(future)
        .await
        .map_err(|error| Error::Other(format!("remote task failed: {error}")))?
}

/// Everything needed to open one remote connection, secrets included. Never
/// persisted: the secret parts come from the OS keychain at connect time.
#[derive(Clone, PartialEq, Eq)]
pub enum RemoteSpec {
    Sftp {
        host: String,
        port: Option<u16>,
        user: String,
        /// Private key file. Without one, the system `ssh` falls back to the agent
        /// and default keys.
        key_path: Option<String>,
    },
    S3 {
        bucket: String,
        region: Option<String>,
        /// Custom endpoint for MinIO, R2, OSS and friends; `None` means AWS.
        endpoint: Option<String>,
        access_key_id: String,
        secret_access_key: String,
    },
    WebDav {
        url: String,
        user: Option<String>,
        password: Option<String>,
    },
}

// Hand-written so a stray `{:?}` in a log line can never print a secret.
impl std::fmt::Debug for RemoteSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteSpec::Sftp {
                host, port, user, ..
            } => f
                .debug_struct("Sftp")
                .field("host", host)
                .field("port", port)
                .field("user", user)
                .finish_non_exhaustive(),
            RemoteSpec::S3 { bucket, .. } => f
                .debug_struct("S3")
                .field("bucket", bucket)
                .finish_non_exhaustive(),
            RemoteSpec::WebDav { url, .. } => f
                .debug_struct("WebDav")
                .field("url", url)
                .finish_non_exhaustive(),
        }
    }
}

impl RemoteSpec {
    pub fn scheme(&self) -> ProtocolScheme {
        match self {
            RemoteSpec::Sftp { .. } => ProtocolScheme::Sftp,
            RemoteSpec::S3 { .. } => ProtocolScheme::S3,
            RemoteSpec::WebDav { .. } => ProtocolScheme::WebDav,
        }
    }
}

/// `user@host` or `ssh://user@host:port`, the forms the system `ssh` understands.
fn sftp_endpoint(host: &str, port: Option<u16>, user: &str) -> String {
    match port {
        Some(port) => format!("ssh://{user}@{host}:{port}"),
        None => format!("{user}@{host}"),
    }
}

/// `~/` is a shell convention, not something `ssh -i` expands for us.
fn expand_home(path: &str, home: Option<PathBuf>) -> String {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => path.to_string(),
    }
}

fn config_error(scheme: ProtocolScheme, error: opendal::Error) -> Error {
    Error::Other(format!("{scheme}: {error}"))
}

fn build_operator(spec: &RemoteSpec) -> Result<Operator> {
    // OpenDAL ships without a default HTTP client until one is installed; S3 and
    // WebDAV fail every request with "default HTTP transport is not installed"
    // otherwise. First install wins, so repeating this is harmless.
    static TRANSPORT: std::sync::Once = std::sync::Once::new();
    TRANSPORT.call_once(opendal::install_default);
    let scheme = spec.scheme();
    let operator = match spec {
        RemoteSpec::Sftp {
            host,
            port,
            user,
            key_path,
        } => {
            let mut builder = services::Sftp::default()
                .endpoint(&sftp_endpoint(host, *port, user))
                // No terminal to ask "trust this host?" on, so a strict check would
                // just fail. Trust-on-first-use records the key and still rejects a
                // changed one, like `StrictHostKeyChecking=accept-new`.
                .known_hosts_strategy("Add");
            if let Some(key) = key_path {
                builder = builder.key(&expand_home(key, dirs_home()));
            }
            Operator::new(builder)
        }
        RemoteSpec::S3 {
            bucket,
            region,
            endpoint,
            access_key_id,
            secret_access_key,
        } => {
            let mut builder = services::S3::default()
                .bucket(bucket)
                .access_key_id(access_key_id)
                .secret_access_key(secret_access_key)
                // Credentials are explicit per connection; ambient AWS config or the
                // instance metadata service must not leak into an unrelated bucket.
                .disable_config_load()
                .disable_ec2_metadata();
            if let Some(region) = region {
                builder = builder.region(region);
            }
            if let Some(endpoint) = endpoint {
                builder = builder.endpoint(endpoint);
            }
            Operator::new(builder)
        }
        RemoteSpec::WebDav {
            url,
            user,
            password,
        } => {
            let mut builder = services::Webdav::default().endpoint(url);
            if let Some(user) = user {
                builder = builder.username(user);
            }
            if let Some(password) = password {
                builder = builder.password(password);
            }
            Operator::new(builder)
        }
    };
    operator.map_err(|error| config_error(scheme, error))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// OpenDAL wants root-relative paths, and a trailing slash to mean "directory".
fn file_path(uri: &VfsUri) -> String {
    uri.path.trim_start_matches('/').to_string()
}

fn dir_path(uri: &VfsUri) -> String {
    let relative = uri.path.trim_matches('/');
    if relative.is_empty() {
        "/".to_string()
    } else {
        format!("{relative}/")
    }
}

fn kind_of(mode: EntryMode) -> FileKind {
    match mode {
        EntryMode::DIR => FileKind::Directory,
        _ => FileKind::File,
    }
}

fn entry_from_metadata(uri: VfsUri, name: &str, metadata: &opendal::Metadata) -> FileEntry {
    let kind = kind_of(metadata.mode());
    let mut entry = FileEntry::new(uri, name, kind);
    if kind == FileKind::File {
        entry.size = metadata.content_length();
    }
    entry.modified = metadata.last_modified().map(SystemTime::from);
    entry.mime_type = metadata.content_type().map(str::to_string);
    entry
}

/// End of a read of `length` bytes from `offset` in a file of `size` bytes, never past
/// the end and never before `offset`.
fn clamp_range_end(offset: u64, length: u64, size: u64) -> u64 {
    offset.saturating_add(length).min(size).max(offset.min(size))
}

fn map_error(uri: &VfsUri, error: opendal::Error) -> Error {
    match error.kind() {
        ErrorKind::NotFound => Error::NotFound { uri: uri.clone() },
        ErrorKind::PermissionDenied => Error::PermissionDenied { uri: uri.clone() },
        ErrorKind::AlreadyExists => Error::AlreadyExists { uri: uri.clone() },
        ErrorKind::NotADirectory => Error::NotADirectory { uri: uri.clone() },
        _ => Error::Other(format!("{uri}: {error}")),
    }
}

/// A provider for one remote connection.
pub struct RemoteProvider {
    scheme: ProtocolScheme,
    operator: Operator,
}

impl RemoteProvider {
    /// Open a connection. SFTP and WebDAV connect lazily, so a wrong host shows up on
    /// the first listing rather than here.
    pub fn connect(spec: &RemoteSpec) -> Result<Self> {
        Ok(Self {
            scheme: spec.scheme(),
            operator: build_operator(spec)?,
        })
    }

    /// Wrap an existing operator; used by tests with OpenDAL's in-memory service.
    pub fn with_operator(scheme: ProtocolScheme, operator: Operator) -> Self {
        Self { scheme, operator }
    }

    /// Whether `uri` names a directory, and the OpenDAL path that reaches it.
    /// Object stores have no real directories, so a file lookup that misses is
    /// retried as a prefix.
    async fn resolve(operator: &Operator, uri: &VfsUri) -> Result<(String, opendal::Metadata)> {
        if uri.is_root() {
            let path = "/".to_string();
            let metadata = operator.stat(&path).await.map_err(|e| map_error(uri, e))?;
            return Ok((path, metadata));
        }
        match operator.stat(&file_path(uri)).await {
            Ok(metadata) => {
                let path = if metadata.is_dir() {
                    dir_path(uri)
                } else {
                    file_path(uri)
                };
                Ok((path, metadata))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let path = dir_path(uri);
                let metadata = operator.stat(&path).await.map_err(|e| map_error(uri, e))?;
                Ok((path, metadata))
            }
            Err(error) => Err(map_error(uri, error)),
        }
    }
}

#[async_trait]
impl VfsProvider for RemoteProvider {
    fn scheme(&self) -> ProtocolScheme {
        self.scheme
    }

    async fn list_stream(&self, uri: &VfsUri) -> Result<BoxStream<'static, Result<ListChunk>>> {
        let operator = self.operator.clone();
        let base = uri.clone();
        let (sender, receiver) = async_channel::bounded::<Result<ListChunk>>(CHANNEL_DEPTH);
        runtime().spawn(async move {
            let mut lister = match operator.lister(&dir_path(&base)).await {
                Ok(lister) => lister,
                Err(error) => {
                    let _ = sender.send(Err(map_error(&base, error))).await;
                    return;
                }
            };
            let mut chunk: ListChunk = Vec::with_capacity(CHUNK_SIZE);
            while let Some(next) = lister.next().await {
                let entry = match next {
                    Ok(entry) => entry,
                    Err(error) => {
                        let _ = sender.send(Err(map_error(&base, error))).await;
                        return;
                    }
                };
                let name = entry.name().trim_end_matches('/');
                // Some servers list the directory itself as its first child.
                if name.is_empty() || entry.path().trim_matches('/') == base.path.trim_matches('/')
                {
                    continue;
                }
                let child = base.join(name);
                chunk.push(entry_from_metadata(child, name, entry.metadata()));
                if chunk.len() >= CHUNK_SIZE {
                    let full = std::mem::replace(&mut chunk, Vec::with_capacity(CHUNK_SIZE));
                    if sender.send(Ok(full)).await.is_err() {
                        return;
                    }
                }
            }
            if !chunk.is_empty() {
                let _ = sender.send(Ok(chunk)).await;
            }
        });
        Ok(receiver.boxed())
    }

    async fn stat(&self, uri: &VfsUri) -> Result<FileEntry> {
        let operator = self.operator.clone();
        let target = uri.clone();
        on_runtime(async move {
            let (_, metadata) = Self::resolve(&operator, &target).await?;
            let name = target.file_name().to_string();
            Ok(entry_from_metadata(target, &name, &metadata))
        })
        .await
    }

    async fn read_stream(
        &self,
        uri: &VfsUri,
        offset: u64,
        length: Option<u64>,
    ) -> Result<BoxStream<'static, Result<Bytes>>> {
        let operator = self.operator.clone();
        let target = uri.clone();
        let (sender, receiver) = async_channel::bounded::<Result<Bytes>>(CHANNEL_DEPTH);
        runtime().spawn(async move {
            let opened = async {
                // A range reaching past the end makes S3 return fewer bytes than the
                // reader was promised, which OpenDAL reports as "too little data".
                // Callers ask for "the first 64 KiB" of files that may be smaller, so
                // the range is clamped to the real size here.
                let end = match length {
                    Some(length) => {
                        let size = operator
                            .stat(&file_path(&target))
                            .await
                            .map_err(|e| map_error(&target, e))?
                            .content_length();
                        let end = clamp_range_end(offset, length, size);
                        if end <= offset {
                            // Nothing to read: an empty range is not a valid request.
                            return Ok(None);
                        }
                        Bound::Excluded(end)
                    }
                    None => Bound::Unbounded,
                };
                let reader = operator
                    .reader(&file_path(&target))
                    .await
                    .map_err(|e| map_error(&target, e))?;
                reader
                    .into_bytes_stream((Bound::Included(offset), end))
                    .await
                    .map(Some)
                    .map_err(|e| map_error(&target, e))
            }
            .await;
            let mut stream = match opened {
                Ok(Some(stream)) => stream,
                Ok(None) => return,
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    return;
                }
            };
            while let Some(next) = stream.next().await {
                let item = next.map_err(|e| Error::from_io(&target, e));
                let failed = item.is_err();
                if sender.send(item).await.is_err() || failed {
                    return;
                }
            }
        });
        Ok(receiver.boxed())
    }

    async fn write_stream(
        &self,
        uri: &VfsUri,
        mut stream: BoxStream<'static, Result<Bytes>>,
        _total_size: Option<u64>,
        on_progress: Option<Box<dyn Fn(u64) + Send + Sync>>,
    ) -> Result<()> {
        let operator = self.operator.clone();
        let target = uri.clone();
        on_runtime(async move {
            let mut writer = operator
                .writer(&file_path(&target))
                .await
                .map_err(|e| map_error(&target, e))?;
            let mut written = 0u64;
            while let Some(chunk) = stream.next().await {
                let outcome = match chunk {
                    Ok(chunk) => {
                        let length = chunk.len() as u64;
                        writer
                            .write(chunk)
                            .await
                            .map(|_| length)
                            .map_err(|e| map_error(&target, e))
                    }
                    Err(error) => Err(error),
                };
                match outcome {
                    Ok(length) => {
                        written += length;
                        if let Some(report) = &on_progress {
                            report(written);
                        }
                    }
                    Err(error) => {
                        // Drop the half-written object instead of leaving it behind.
                        let _ = writer.abort().await;
                        return Err(error);
                    }
                }
            }
            writer
                .close()
                .await
                .map(|_| ())
                .map_err(|e| map_error(&target, e))
        })
        .await
    }

    async fn rename(&self, from: &VfsUri, to: &VfsUri) -> Result<()> {
        let operator = self.operator.clone();
        let (from, to) = (from.clone(), to.clone());
        let scheme = self.scheme;
        on_runtime(async move {
            let (source, metadata) = Self::resolve(&operator, &from).await?;
            let destination = if metadata.is_dir() {
                dir_path(&to)
            } else {
                file_path(&to)
            };
            let capability = operator.info().capability();
            if capability.rename {
                return operator
                    .rename(&source, &destination)
                    .await
                    .map_err(|e| map_error(&from, e));
            }
            // Object stores have no rename: copy then delete, one object at a time.
            // A "directory" there is a prefix of many objects, so refuse it rather
            // than half-move it.
            if metadata.is_dir() || !capability.copy {
                return Err(Error::Unsupported {
                    scheme: scheme.to_string(),
                    operation: "rename",
                });
            }
            operator
                .copy(&source, &destination)
                .await
                .map_err(|e| map_error(&from, e))?;
            operator
                .delete(&source)
                .await
                .map_err(|e| map_error(&from, e))
        })
        .await
    }

    async fn delete(&self, uri: &VfsUri, recursive: bool) -> Result<()> {
        let operator = self.operator.clone();
        let target = uri.clone();
        on_runtime(async move {
            let (path, metadata) = Self::resolve(&operator, &target).await?;
            if !metadata.is_dir() {
                return operator
                    .delete(&path)
                    .await
                    .map_err(|e| map_error(&target, e));
            }
            if recursive {
                let options = opendal::options::DeleteOptions {
                    recursive: true,
                    ..Default::default()
                };
                return operator
                    .delete_options(&path, options)
                    .await
                    .map_err(|e| map_error(&target, e));
            }
            // WebDAV deletes a collection with everything in it, so emptiness has to
            // be checked here to keep "delete, not recursive" honest.
            let mut lister = operator
                .lister(&path)
                .await
                .map_err(|e| map_error(&target, e))?;
            while let Some(next) = lister.next().await {
                let entry = next.map_err(|e| map_error(&target, e))?;
                if entry.path().trim_matches('/') != path.trim_matches('/') {
                    return Err(Error::Other(format!("Directory not empty: {target}")));
                }
            }
            operator
                .delete(&path)
                .await
                .map_err(|e| map_error(&target, e))
        })
        .await
    }

    async fn create_dir(&self, uri: &VfsUri) -> Result<()> {
        let operator = self.operator.clone();
        let target = uri.clone();
        on_runtime(async move {
            operator
                .create_dir(&dir_path(&target))
                .await
                .map_err(|e| map_error(&target, e))
        })
        .await
    }

    fn supports(&self, capability: Capability) -> bool {
        let info = self.operator.info().capability();
        match capability {
            Capability::ServerSideRename => info.rename,
            Capability::MultipartUpload => info.write_can_multi,
            Capability::Trash | Capability::Watch | Capability::Permissions => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;

    fn memory() -> RemoteProvider {
        let operator = Operator::new(services::Memory::default()).unwrap();
        RemoteProvider::with_operator(ProtocolScheme::WebDav, operator)
    }

    fn uri(path: &str) -> VfsUri {
        VfsUri::remote(ProtocolScheme::WebDav, "1", path)
    }

    fn put(provider: &RemoteProvider, path: &str, body: &'static [u8]) {
        let stream = futures::stream::iter([Ok(Bytes::from_static(body))]).boxed();
        block_on(provider.write_stream(&uri(path), stream, None, None)).unwrap();
    }

    fn read(provider: &RemoteProvider, path: &str, offset: u64, length: Option<u64>) -> Vec<u8> {
        block_on(async {
            let mut stream = provider
                .read_stream(&uri(path), offset, length)
                .await
                .unwrap();
            let mut out = Vec::new();
            while let Some(chunk) = stream.next().await {
                out.extend_from_slice(&chunk.unwrap());
            }
            out
        })
    }

    fn list(provider: &RemoteProvider, path: &str) -> Result<Vec<FileEntry>> {
        block_on(async {
            let mut stream = provider.list_stream(&uri(path)).await?;
            let mut all = Vec::new();
            while let Some(chunk) = stream.next().await {
                all.extend(chunk?);
            }
            all.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(all)
        })
    }

    #[test]
    fn paths_map_to_opendal_form() {
        assert_eq!(file_path(&uri("/a/b.txt")), "a/b.txt");
        assert_eq!(dir_path(&uri("/a/b")), "a/b/");
        assert_eq!(dir_path(&uri("/")), "/");
    }

    #[test]
    fn sftp_endpoint_includes_the_port_only_when_given() {
        assert_eq!(sftp_endpoint("h.example", None, "me"), "me@h.example");
        assert_eq!(
            sftp_endpoint("h.example", Some(2222), "me"),
            "ssh://me@h.example:2222"
        );
    }

    #[test]
    fn expands_a_leading_tilde_only() {
        let home = Some(PathBuf::from("/Users/me"));
        assert_eq!(expand_home("~/.ssh/id", home.clone()), "/Users/me/.ssh/id");
        assert_eq!(expand_home("/abs/key", home.clone()), "/abs/key");
        assert_eq!(expand_home("~/.ssh/id", None), "~/.ssh/id");
    }

    #[test]
    fn debug_output_never_contains_secrets() {
        let spec = RemoteSpec::S3 {
            bucket: "assets".into(),
            region: None,
            endpoint: None,
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "hunter2-secret".into(),
        };
        let text = format!("{spec:?}");
        assert!(text.contains("assets"));
        assert!(!text.contains("hunter2-secret"));
        assert!(!text.contains("AKIDEXAMPLE"));
    }

    #[test]
    fn builds_a_provider_for_every_kind_without_touching_the_network() {
        let specs = [
            RemoteSpec::Sftp {
                host: "h.example".into(),
                port: Some(22),
                user: "me".into(),
                key_path: Some("~/.ssh/id_ed25519".into()),
            },
            RemoteSpec::S3 {
                bucket: "b".into(),
                region: Some("eu-west-1".into()),
                endpoint: Some("https://s3.example.com".into()),
                access_key_id: "id".into(),
                secret_access_key: "secret".into(),
            },
            RemoteSpec::WebDav {
                url: "https://dav.example.com/files".into(),
                user: Some("me".into()),
                password: None,
            },
        ];
        for spec in specs {
            let provider = RemoteProvider::connect(&spec).unwrap();
            assert_eq!(provider.scheme(), spec.scheme());
        }
    }

    #[test]
    fn writes_then_reads_back_whole_and_ranged() {
        let provider = memory();
        put(&provider, "/docs/hello.txt", b"hello world");
        assert_eq!(read(&provider, "/docs/hello.txt", 0, None), b"hello world");
        assert_eq!(read(&provider, "/docs/hello.txt", 6, None), b"world");
        assert_eq!(read(&provider, "/docs/hello.txt", 0, Some(5)), b"hello");
    }

    #[test]
    fn range_ends_are_clamped_to_the_file_size() {
        assert_eq!(clamp_range_end(0, 65536, 2_406_913), 65536);
        assert_eq!(clamp_range_end(0, 8 * 1024 * 1024, 2_406_913), 2_406_913);
        assert_eq!(clamp_range_end(0, 100, 0), 0, "empty file");
        assert_eq!(clamp_range_end(50, 100, 60), 60);
        assert_eq!(clamp_range_end(90, 10, 60), 60, "offset past the end stays empty");
    }

    #[test]
    fn a_head_read_longer_than_the_file_returns_the_whole_file() {
        let provider = memory();
        put(&provider, "/small.txt", b"tiny");
        assert_eq!(read(&provider, "/small.txt", 0, Some(64 * 1024)), b"tiny");
        assert!(read(&provider, "/small.txt", 10, Some(5)).is_empty());
    }

    #[test]
    fn reports_progress_as_bytes_land() {
        use std::sync::{Arc, Mutex};
        let provider = memory();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let stream = futures::stream::iter([
            Ok(Bytes::from_static(b"abc")),
            Ok(Bytes::from_static(b"de")),
        ])
        .boxed();
        block_on(provider.write_stream(
            &uri("/p.bin"),
            stream,
            Some(5),
            Some(Box::new(move |n| sink.lock().unwrap().push(n))),
        ))
        .unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![3, 5]);
    }

    #[test]
    fn lists_files_and_folders_without_the_directory_itself() {
        let provider = memory();
        put(&provider, "/docs/a.txt", b"a");
        put(&provider, "/docs/sub/b.txt", b"bb");
        let entries = list(&provider, "/docs").unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "sub"]);
        assert_eq!(entries[0].kind, FileKind::File);
        assert_eq!(entries[0].size, 1);
        assert_eq!(entries[1].kind, FileKind::Directory);
        assert_eq!(entries[1].uri.path, "/docs/sub");
        assert_eq!(entries[1].uri.connection_id.as_deref(), Some("1"));
    }

    #[test]
    fn an_empty_root_lists_as_empty() {
        assert!(list(&memory(), "/").unwrap().is_empty());
    }

    #[test]
    fn stat_tells_files_from_folders_and_misses_cleanly() {
        let provider = memory();
        put(&provider, "/docs/a.txt", b"abc");
        let file = block_on(provider.stat(&uri("/docs/a.txt"))).unwrap();
        assert_eq!((file.kind, file.size), (FileKind::File, 3));
        let dir = block_on(provider.stat(&uri("/docs"))).unwrap();
        assert_eq!(dir.kind, FileKind::Directory);
        let missing = block_on(provider.stat(&uri("/nope"))).unwrap_err();
        assert!(matches!(missing, Error::NotFound { .. }), "{missing}");
    }

    #[test]
    fn creates_a_folder_that_then_lists_empty() {
        let provider = memory();
        block_on(provider.create_dir(&uri("/made"))).unwrap();
        assert!(list(&provider, "/made").unwrap().is_empty());
        let stat = block_on(provider.stat(&uri("/made"))).unwrap();
        assert_eq!(stat.kind, FileKind::Directory);
    }

    #[test]
    fn rename_on_a_store_without_move_or_copy_is_refused_and_keeps_the_source() {
        // OpenDAL's in-memory service offers neither, like a bare key-value store.
        let provider = memory();
        put(&provider, "/old.txt", b"data");
        let error = block_on(provider.rename(&uri("/old.txt"), &uri("/new.txt"))).unwrap_err();
        assert!(matches!(error, Error::Unsupported { .. }), "{error}");
        assert_eq!(read(&provider, "/old.txt", 0, None), b"data");
        assert!(!provider.supports(Capability::ServerSideRename));
    }

    #[test]
    fn deletes_a_file() {
        let provider = memory();
        put(&provider, "/gone.txt", b"x");
        block_on(provider.delete(&uri("/gone.txt"), false)).unwrap();
        assert!(block_on(provider.stat(&uri("/gone.txt"))).is_err());
    }

    #[test]
    fn non_recursive_delete_refuses_a_folder_with_contents() {
        let provider = memory();
        put(&provider, "/full/a.txt", b"x");
        let error = block_on(provider.delete(&uri("/full"), false)).unwrap_err();
        assert!(error.to_string().contains("not empty"), "{error}");
        assert!(block_on(provider.stat(&uri("/full/a.txt"))).is_ok());
    }

    #[test]
    fn recursive_delete_removes_the_whole_tree() {
        let provider = memory();
        put(&provider, "/tree/a.txt", b"x");
        put(&provider, "/tree/deep/b.txt", b"y");
        block_on(provider.delete(&uri("/tree"), true)).unwrap();
        assert!(block_on(provider.stat(&uri("/tree/a.txt"))).is_err());
        assert!(block_on(provider.stat(&uri("/tree/deep/b.txt"))).is_err());
    }

    #[test]
    fn deleting_something_missing_is_not_found() {
        let error = block_on(memory().delete(&uri("/missing"), false)).unwrap_err();
        assert!(matches!(error, Error::NotFound { .. }), "{error}");
    }

    #[test]
    fn a_failing_source_stream_leaves_no_partial_file() {
        let provider = memory();
        let stream = futures::stream::iter([
            Ok(Bytes::from_static(b"half")),
            Err(Error::Other("source broke".into())),
        ])
        .boxed();
        let error =
            block_on(provider.write_stream(&uri("/partial.bin"), stream, None, None)).unwrap_err();
        assert!(error.to_string().contains("source broke"));
        assert!(block_on(provider.stat(&uri("/partial.bin"))).is_err());
    }
}
