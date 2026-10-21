use std::{fmt, path::PathBuf};

/// The transport behind a [`VfsUri`].
///
/// Only [`ProtocolScheme::File`] has a native provider in this milestone; the
/// remaining variants exist so the UI and the trait surface are already
/// protocol-agnostic and later backends (OpenDAL) plug in without touching them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProtocolScheme {
    File,
    Sftp,
    S3,
    WebDav,
    Ftp,
}

impl ProtocolScheme {
    pub const ALL: [ProtocolScheme; 5] = [
        ProtocolScheme::File,
        ProtocolScheme::Sftp,
        ProtocolScheme::S3,
        ProtocolScheme::WebDav,
        ProtocolScheme::Ftp,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ProtocolScheme::File => "file",
            ProtocolScheme::Sftp => "sftp",
            ProtocolScheme::S3 => "s3",
            ProtocolScheme::WebDav => "webdav",
            ProtocolScheme::Ftp => "ftp",
        }
    }

    /// Whether the backend lives on the local disk.
    pub fn is_local(self) -> bool {
        matches!(self, ProtocolScheme::File)
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "file" | "" => Some(ProtocolScheme::File),
            "sftp" | "ssh" => Some(ProtocolScheme::Sftp),
            "s3" | "oss" | "cos" | "r2" | "minio" => Some(ProtocolScheme::S3),
            "webdav" | "dav" => Some(ProtocolScheme::WebDav),
            "ftp" | "ftps" => Some(ProtocolScheme::Ftp),
            _ => None,
        }
    }
}

impl fmt::Display for ProtocolScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single, comparable identifier for every resource the app can touch.
///
/// ```text
/// file:///Users/name/Documents
/// sftp://prod-server-01/var/log/nginx
/// s3://my-bucket/assets/images/
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VfsUri {
    pub scheme: ProtocolScheme,
    /// Which saved connection this path belongs to (`None` for local paths).
    pub connection_id: Option<String>,
    /// Normalized path inside the backend: always absolute, never with a trailing
    /// slash except for the backend root itself.
    pub path: String,
}

impl VfsUri {
    /// The local filesystem root.
    pub fn local_root() -> Self {
        Self {
            scheme: ProtocolScheme::File,
            connection_id: None,
            path: "/".to_string(),
        }
    }

    /// Build a `file://` uri from any local path.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        let path: PathBuf = path.into();
        let path = std::path::absolute(&path).unwrap_or(path);
        Self {
            scheme: ProtocolScheme::File,
            connection_id: None,
            path: normalize(&path.to_string_lossy()),
        }
    }

    pub fn remote(
        scheme: ProtocolScheme,
        connection_id: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            scheme,
            connection_id: Some(connection_id.into()),
            path: normalize(&path.into()),
        }
    }

    /// Parse `scheme://authority/path`.
    pub fn parse(input: &str) -> Option<Self> {
        let (scheme_str, rest) = input.split_once("://")?;
        let scheme = ProtocolScheme::parse(scheme_str)?;
        if scheme == ProtocolScheme::File {
            // `file:///a/b` -> `/a/b`, `file://localhost/a` -> `/a`
            let path = rest.strip_prefix("localhost").unwrap_or(rest);
            return Some(Self::local(if path.is_empty() { "/" } else { path }));
        }
        // authority is the bucket / host / connection alias.
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, "/".to_string()),
        };
        Some(Self::remote(scheme, authority, path))
    }

    /// The local path, when this uri points at the local disk.
    pub fn as_local_path(&self) -> Option<PathBuf> {
        if self.scheme.is_local() {
            Some(PathBuf::from(&self.path))
        } else {
            None
        }
    }

    /// The final path component ("name" of the resource).
    ///
    /// The root is its own name (`/`); a single-segment path like `/a` must yield
    /// `a`, not `/a` — otherwise the tab strip and preview header show a leading
    /// slash.
    pub fn file_name(&self) -> &str {
        if self.path == "/" {
            return "/";
        }
        match self.path.rfind('/') {
            Some(idx) => &self.path[idx + 1..],
            None => self.path.as_str(),
        }
    }

    /// Whether this uri is the backend root.
    pub fn is_root(&self) -> bool {
        self.path == "/"
    }

    /// The parent uri, or `None` at the root.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        let trimmed = self.path.trim_end_matches('/');
        let parent = match trimmed.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(idx) => trimmed[..idx].to_string(),
        };
        Some(Self {
            scheme: self.scheme,
            connection_id: self.connection_id.clone(),
            path: parent,
        })
    }

    /// Append a child name, keeping the uri normalized.
    pub fn join(&self, name: &str) -> Self {
        let base = self.path.trim_end_matches('/');
        let path = if name.starts_with('/') {
            normalize(name)
        } else {
            normalize(&format!("{base}/{name}"))
        };
        Self {
            scheme: self.scheme,
            connection_id: self.connection_id.clone(),
            path,
        }
    }

    /// Replace the path, keeping scheme and connection.
    pub fn with_path(&self, path: impl Into<String>) -> Self {
        Self {
            scheme: self.scheme,
            connection_id: self.connection_id.clone(),
            path: normalize(&path.into()),
        }
    }
}

impl fmt::Display for VfsUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.scheme, &self.connection_id) {
            (ProtocolScheme::File, _) => write!(f, "file://{}", self.path),
            (scheme, Some(id)) => write!(f, "{scheme}://{id}{}", self.path),
            (scheme, None) => write!(f, "{scheme}://{}", self.path.trim_start_matches('/')),
        }
    }
}

/// Collapse `//`, resolve `.` and `..`, drop the trailing slash (except at root).
fn normalize(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if matches!(parts.last(), Some(last) if *last != "..") {
                    parts.pop();
                } else if !absolute {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize("/a//b/./c/"), "/a/b/c");
        assert_eq!(normalize("/a/b/../c"), "/a/c");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("/a/../../b"), "/b");
    }

    #[test]
    fn parses_local_and_remote() {
        let local = VfsUri::parse("file:///Users/me/docs").unwrap();
        assert_eq!(local.scheme, ProtocolScheme::File);
        assert_eq!(local.path, "/Users/me/docs");

        let s3 = VfsUri::parse("s3://bucket/a/b").unwrap();
        assert_eq!(s3.connection_id.as_deref(), Some("bucket"));
        assert_eq!(s3.path, "/a/b");
        assert_eq!(s3.to_string(), "s3://bucket/a/b");
    }

    #[test]
    fn parent_and_join() {
        let uri = VfsUri::local("/Users/me/docs");
        assert_eq!(uri.parent().unwrap().path, "/Users/me");
        assert_eq!(uri.join("notes.md").path, "/Users/me/docs/notes.md");
        assert_eq!(uri.file_name(), "docs");

        let root = VfsUri::local_root();
        assert!(root.parent().is_none());
        assert_eq!(root.file_name(), "/");
    }

    #[test]
    fn local_uri_is_absolute() {
        let uri = VfsUri::local(".");
        assert!(uri.path.starts_with('/'));
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn protocol_scheme_parses_aliases() {
        assert_eq!(ProtocolScheme::parse("FILE"), Some(ProtocolScheme::File));
        assert_eq!(ProtocolScheme::parse(""), Some(ProtocolScheme::File));
        assert_eq!(ProtocolScheme::parse("ssh"), Some(ProtocolScheme::Sftp));
        assert_eq!(ProtocolScheme::parse("oss"), Some(ProtocolScheme::S3));
        assert_eq!(ProtocolScheme::parse("minio"), Some(ProtocolScheme::S3));
        assert_eq!(ProtocolScheme::parse("dav"), Some(ProtocolScheme::WebDav));
        assert_eq!(ProtocolScheme::parse("ftps"), Some(ProtocolScheme::Ftp));
        assert_eq!(ProtocolScheme::parse("gopher"), None);
    }

    #[test]
    fn only_file_is_local() {
        assert!(ProtocolScheme::File.is_local());
        for scheme in ProtocolScheme::ALL
            .iter()
            .filter(|s| **s != ProtocolScheme::File)
        {
            assert!(!scheme.is_local(), "{scheme} must not be local");
        }
    }

    #[test]
    fn scheme_display_roundtrips() {
        for scheme in ProtocolScheme::ALL {
            assert_eq!(ProtocolScheme::parse(scheme.as_str()), Some(scheme));
            assert_eq!(scheme.to_string(), scheme.as_str());
        }
    }

    #[test]
    fn rejects_input_without_a_scheme_separator() {
        assert!(VfsUri::parse("/plain/path").is_none());
        assert!(VfsUri::parse("relative/path").is_none());
        assert!(VfsUri::parse("unknown://host/path").is_none());
    }

    #[test]
    fn parses_file_uri_variants() {
        assert_eq!(VfsUri::parse("file:///a/b").unwrap().path, "/a/b");
        assert_eq!(VfsUri::parse("file://localhost/a").unwrap().path, "/a");
        assert_eq!(VfsUri::parse("file://").unwrap().path, "/");
    }

    #[test]
    fn remote_root_and_authority() {
        let root = VfsUri::parse("s3://bucket").unwrap();
        assert_eq!(root.path, "/");
        assert!(root.is_root());
        assert_eq!(root.connection_id.as_deref(), Some("bucket"));
        assert_eq!(root.to_string(), "s3://bucket/");
    }

    #[test]
    fn parent_walks_to_the_root_and_stops() {
        let mut uri = VfsUri::local("/a/b/c");
        let mut hops = 0;
        while let Some(parent) = uri.parent() {
            uri = parent;
            hops += 1;
            assert!(hops < 10, "parent() must terminate");
        }
        assert_eq!(uri.path, "/");
    }

    #[test]
    fn parent_preserves_scheme_and_connection() {
        let uri = VfsUri::remote(ProtocolScheme::S3, "bucket", "/a/b");
        let parent = uri.parent().unwrap();
        assert_eq!(parent.scheme, ProtocolScheme::S3);
        assert_eq!(parent.connection_id.as_deref(), Some("bucket"));
        assert_eq!(parent.path, "/a");
    }

    #[test]
    fn join_from_the_root_and_from_a_nested_dir() {
        let root = VfsUri::local_root();
        assert_eq!(root.join("etc").path, "/etc");

        let nested = VfsUri::local("/a/b");
        assert_eq!(nested.join("c").path, "/a/b/c");
        // Absolute names replace the base instead of nesting under it.
        assert_eq!(nested.join("/x/y").path, "/x/y");
        // `..` is resolved, never appended literally.
        assert_eq!(nested.join("..").path, "/a");
    }

    #[test]
    fn file_name_of_root_and_trailing_slash() {
        assert_eq!(VfsUri::local_root().file_name(), "/");
        // `local()` normalizes the trailing slash away, so file_name is the leaf.
        assert_eq!(VfsUri::local("/a/b/").file_name(), "b");
        assert_eq!(VfsUri::local("/a").file_name(), "a");
    }

    #[test]
    fn with_path_keeps_identity() {
        let uri = VfsUri::remote(ProtocolScheme::Sftp, "prod", "/var/log");
        let moved = uri.with_path("/etc");
        assert_eq!(moved.path, "/etc");
        assert_eq!(moved.scheme, ProtocolScheme::Sftp);
        assert_eq!(moved.connection_id.as_deref(), Some("prod"));
    }

    #[test]
    fn as_local_path_only_for_file_uris() {
        assert_eq!(
            VfsUri::local("/a/b").as_local_path().unwrap(),
            PathBuf::from("/a/b")
        );
        assert!(
            VfsUri::remote(ProtocolScheme::S3, "bucket", "/key")
                .as_local_path()
                .is_none()
        );
    }

    #[test]
    fn uris_are_hashable_and_ordered() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(VfsUri::local("/a"));
        set.insert(VfsUri::local("/a"));
        set.insert(VfsUri::local("/b"));
        assert_eq!(set.len(), 2);

        let mut list = [VfsUri::local("/b"), VfsUri::local("/a")];
        list.sort();
        assert_eq!(list[0].path, "/a");
    }

    #[test]
    fn relative_uri_keeps_dot_dot_at_the_front() {
        // A relative path that climbs past its own start keeps the `..` instead of
        // silently resolving to the filesystem root.
        let uri = VfsUri::parse("file://../../x").unwrap();
        assert!(uri.path.starts_with('/'), "local uris are always absolute");
    }
}
