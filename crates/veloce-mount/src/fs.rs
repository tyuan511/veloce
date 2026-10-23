//! The filesystem behind the FUSE callbacks: a read-only view of a `VfsProvider`.
//! Written against plain paths and errno values, so it has no FUSE types in it.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use futures::{StreamExt, executor::block_on};
use vfs_core::{
    Error, FileEntry, VfsProvider, VfsUri,
    mountfs::{Cached, Node, child_path, clamp_read, parent_path},
};

#[derive(Default)]
struct Caches {
    listings: HashMap<String, Cached<Vec<FileEntry>>>,
    nodes: HashMap<String, Cached<Node>>,
}

pub struct RemoteFs {
    provider: Arc<dyn VfsProvider>,
    root: VfsUri,
    pub owner: (u32, u32),
    caches: Mutex<Caches>,
}

/// A positive errno, or the reason it could not be decided.
pub type Errno = i32;

fn errno(error: &Error) -> Errno {
    match error {
        Error::NotFound { .. } => libc::ENOENT,
        Error::PermissionDenied { .. } => libc::EACCES,
        Error::NotADirectory { .. } => libc::ENOTDIR,
        _ => libc::EIO,
    }
}

impl RemoteFs {
    pub fn new(provider: Arc<dyn VfsProvider>, root: VfsUri, owner: (u32, u32)) -> Self {
        Self {
            provider,
            root,
            owner,
            caches: Mutex::new(Caches::default()),
        }
    }

    /// The uri of `path` (relative to the mounted folder) on the provider. The mounted
    /// folder need not be the backend root, so its own path is the prefix.
    fn uri(&self, path: &str) -> VfsUri {
        let base = self.root.path.trim_end_matches('/');
        self.root.with_path(format!("{base}{path}"))
    }

    /// The listing of a directory, from the cache while it is fresh.
    pub fn listing(&self, path: &str) -> Result<Vec<FileEntry>, Errno> {
        let now = Instant::now();
        if let Some(cached) = self
            .caches
            .lock()
            .map_err(|_| libc::EIO)?
            .listings
            .get(path)
            .filter(|cached| cached.is_fresh(now))
        {
            return Ok(cached.value.clone());
        }
        let uri = self.uri(path);
        let entries = block_on(async {
            let mut stream = self.provider.list_stream(&uri).await?;
            let mut all = Vec::new();
            while let Some(chunk) = stream.next().await {
                all.extend(chunk?);
            }
            Ok::<_, Error>(all)
        })
        .map_err(|error| errno(&error))?;
        let mut caches = self.caches.lock().map_err(|_| libc::EIO)?;
        for entry in &entries {
            if let Some(child) = child_path(path, &entry.name) {
                caches
                    .nodes
                    .insert(child, Cached::new(Node::from_entry(entry), now));
            }
        }
        caches
            .listings
            .insert(path.to_string(), Cached::new(entries.clone(), now));
        Ok(entries)
    }

    /// What `stat` says about `path`. A fresh listing of the parent answers without a
    /// request, including "no such file".
    pub fn node(&self, path: &str) -> Result<Node, Errno> {
        if path == "/" {
            return Ok(Node::dir());
        }
        let now = Instant::now();
        {
            let caches = self.caches.lock().map_err(|_| libc::EIO)?;
            if let Some(cached) = caches.nodes.get(path).filter(|c| c.is_fresh(now)) {
                return Ok(cached.value.clone());
            }
            if caches
                .listings
                .get(parent_path(path))
                .is_some_and(|listing| listing.is_fresh(now))
            {
                return Err(libc::ENOENT);
            }
        }
        let entry = block_on(self.provider.stat(&self.uri(path))).map_err(|e| errno(&e))?;
        let node = Node::from_entry(&entry);
        self.caches
            .lock()
            .map_err(|_| libc::EIO)?
            .nodes
            .insert(path.to_string(), Cached::new(node.clone(), now));
        Ok(node)
    }

    /// Up to `size` bytes of the file at `offset`.
    pub fn read(&self, path: &str, offset: u64, size: u32) -> Result<Vec<u8>, Errno> {
        let node = self.node(path)?;
        let Some((start, length)) = clamp_read(node.size, offset, size) else {
            return Ok(Vec::new());
        };
        let uri = self.uri(path);
        block_on(async {
            let mut stream = self.provider.read_stream(&uri, start, Some(length)).await?;
            let mut bytes = Vec::with_capacity(length as usize);
            while let Some(chunk) = stream.next().await {
                bytes.extend_from_slice(&chunk?);
            }
            Ok::<_, Error>(bytes)
        })
        .map_err(|error| errno(&error))
    }
}
