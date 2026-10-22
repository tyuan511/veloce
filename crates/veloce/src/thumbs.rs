//! Thumbnails for the icon view: what to render, how to render it, and the cache.
//!
//! Generation shells out to macOS tools (`sips` for ordinary pictures, Quick
//! Look's `qlmanage` for everything else), always off the UI thread. Results are
//! kept in the SQLite store, keyed by path, size and modification time, so a
//! folder of photos is only ever thumbnailed once.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, UNIX_EPOCH},
};

use veloce_store::Store;
use vfs_core::{FileEntry, VfsProvider, VfsUri};

/// Edge length, in pixels, of icon-view thumbnails.
pub const THUMB_PIXELS: u32 = 256;
/// Edge length of the larger renders the preview panel shows. They share the
/// cache with the icon view, told apart by size in the key.
pub const PREVIEW_PIXELS: u32 = 1024;

/// Ordinary raster pictures, which `sips` converts in a few milliseconds.
const RASTER: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif", "heic", "heif", "webp", "ico",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generator {
    Sips,
    QuickLook,
}

/// Whether the icon view should try to show a picture of this file.
pub fn is_thumbnailable(ext: &str) -> bool {
    RASTER.contains(&ext) || ext == "svg" || crate::preview::is_thumbnail_extension(ext)
}

pub fn generator_for(ext: &str) -> Generator {
    if RASTER.contains(&ext) {
        Generator::Sips
    } else {
        Generator::QuickLook
    }
}

/// What identifies one version of a file; a change here means a new thumbnail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub size: u64,
    pub modified_secs: u64,
}

impl Stamp {
    /// From a listing entry's size and modification time (a missing time counts
    /// as the epoch, which still changes the key if it later appears).
    pub fn new(size: u64, modified: Option<std::time::SystemTime>) -> Self {
        Self {
            size,
            modified_secs: modified
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0),
        }
    }

    #[cfg(test)]
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self::new(metadata.len(), metadata.modified().ok()))
    }
}

/// Cache key for a picture rendered from a remote file (`kind` says which size).
/// Follows the file version like [`cache_key`], so a
/// changed remote file is fetched again.
pub fn remote_key(uri: &vfs_core::VfsUri, stamp: Stamp, kind: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    uri.hash(&mut hasher);
    stamp.size.hash(&mut hasher);
    stamp.modified_secs.hash(&mut hasher);
    format!("remote:{:016x}@{kind}", hasher.finish())
}

/// Cache key for one rendering of one file version: changes when the file does,
/// and differs per pixel size so icon-view and preview renders never collide.
pub fn cache_key(path: &Path, stamp: Stamp, pixels: u32) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    stamp.size.hash(&mut hasher);
    stamp.modified_secs.hash(&mut hasher);
    format!("{:016x}@{pixels}", hasher.finish())
}

pub fn sips_args(src: &Path, out: &Path, pixels: u32) -> Vec<OsString> {
    vec![
        "-Z".into(),
        pixels.to_string().into(),
        "-s".into(),
        "format".into(),
        "png".into(),
        src.as_os_str().to_os_string(),
        "--out".into(),
        out.as_os_str().to_os_string(),
    ]
}

pub fn quicklook_args(src: &Path, out_dir: &Path, pixels: u32) -> Vec<OsString> {
    vec![
        "-t".into(),
        "-s".into(),
        pixels.to_string().into(),
        "-o".into(),
        out_dir.as_os_str().to_os_string(),
        src.as_os_str().to_os_string(),
    ]
}

/// Run `command` and give up (killing it) after `limit`. True on a clean exit.
pub(crate) fn run_with_timeout(mut command: Command, limit: Duration) -> bool {
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Render a thumbnail PNG for `path`, or `None` when the tools are missing, the
/// file cannot be rendered, or the platform has no generator.
pub fn generate(path: &Path, ext: &str, key: &str, pixels: u32) -> Option<Vec<u8>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // `@` in the key is fine in a path, but keep the work directory tidy.
    let work = std::env::temp_dir()
        .join("veloce-thumb-work")
        .join(key.replace('@', "-"));
    std::fs::create_dir_all(&work).ok()?;
    let result = render_into(path, ext, &work, pixels);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn render_into(path: &Path, ext: &str, work: &Path, pixels: u32) -> Option<Vec<u8>> {
    let produced: PathBuf = match generator_for(ext) {
        Generator::Sips => {
            let out = work.join("thumb.png");
            let mut command = Command::new("sips");
            command.args(sips_args(path, &out, pixels));
            if run_with_timeout(command, Duration::from_secs(10)) {
                out
            } else {
                // sips cannot read everything (some WebP/ICO): Quick Look can.
                return quicklook_render(path, work, pixels);
            }
        }
        Generator::QuickLook => return quicklook_render(path, work, pixels),
    };
    std::fs::read(produced)
        .ok()
        .filter(|bytes| !bytes.is_empty())
}

fn quicklook_render(path: &Path, work: &Path, pixels: u32) -> Option<Vec<u8>> {
    let mut command = Command::new("qlmanage");
    command.args(quicklook_args(path, work, pixels));
    if !run_with_timeout(command, Duration::from_secs(8)) {
        return None;
    }
    // qlmanage names the output `<file name>.png`.
    let out = work.join(format!("{}.png", path.file_name()?.to_string_lossy()));
    std::fs::read(out).ok().filter(|bytes| !bytes.is_empty())
}

/// Largest remote file downloaded to be rendered. Only the small PNG is kept, but the
/// whole file has to cross the network once to make it.
pub const REMOTE_RENDER_BYTES: u64 = 32 * 1024 * 1024;

/// The cache key for `entry` at `pixels`. Local files keep their original keys, so
/// thumbnails made before remote support are still found.
pub fn thumb_key(uri: &VfsUri, stamp: Stamp, pixels: u32) -> String {
    match uri.as_local_path() {
        Some(path) => cache_key(&path, stamp, pixels),
        None => remote_key(uri, stamp, &format!("px{pixels}")),
    }
}

/// Thumbnail PNG bytes for any entry, local or remote: from the store if cached,
/// else rendered and cached. `None` means "show the ordinary icon".
///
/// The one path for both: a remote file is first fetched through its provider into
/// a scratch file, then rendered by the same tools as a local one. Failure is
/// remembered only where retrying is expensive, i.e. for a download.
pub async fn thumbnail_bytes(
    store: Option<&Store>,
    provider: &dyn VfsProvider,
    entry: &FileEntry,
    pixels: u32,
) -> Option<Vec<u8>> {
    use futures::StreamExt as _;
    let ext = entry.extension()?;
    let local = entry.uri.as_local_path();
    if local.is_none() && entry.size > REMOTE_RENDER_BYTES {
        return None;
    }
    let key = thumb_key(&entry.uri, Stamp::new(entry.size, entry.modified), pixels);
    if let Some(store) = store
        && let Ok(Some(png)) = store.get_thumbnail(&key)
    {
        // An empty entry is a remembered "this cannot be rendered".
        return (!png.is_empty()).then_some(png);
    }
    let png = match local {
        Some(path) => generate(&path, &ext, &key, pixels),
        None => {
            let work = std::env::temp_dir()
                .join("veloce-remote-thumb")
                .join(key.replace([':', '@'], "-"));
            let file = work.join(&entry.name);
            let fetched: Result<(), ()> = async {
                let mut stream = provider
                    .read_stream(&entry.uri, 0, Some(entry.size))
                    .await
                    .map_err(|_| ())?;
                std::fs::create_dir_all(&work).map_err(|_| ())?;
                let mut out = std::fs::File::create(&file).map_err(|_| ())?;
                while let Some(chunk) = stream.next().await {
                    std::io::Write::write_all(&mut out, &chunk.map_err(|_| ())?).map_err(|_| ())?;
                }
                Ok(())
            }
            .await;
            if fetched.is_err() {
                // The network failed, not the file: do not remember it.
                let _ = std::fs::remove_dir_all(&work);
                return None;
            }
            let png = generate(&file, &ext, &key, pixels);
            let _ = std::fs::remove_dir_all(&work);
            png
        }
    };
    if let Some(store) = store {
        match &png {
            Some(png) => {
                let _ = store.put_thumbnail(&key, png);
            }
            None if !entry.uri.scheme.is_local() => {
                let _ = store.put_thumbnail(&key, &[]);
            }
            None => {}
        }
    }
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid 1x1 PNG.
    const TINY_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 252, 207, 192, 80,
        15, 0, 4, 133, 1, 128, 132, 169, 140, 33, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    #[test]
    fn remote_keys_follow_the_connection_path_and_version() {
        use vfs_core::{ProtocolScheme, VfsUri};
        let uri = |id: &str, path: &str| VfsUri::remote(ProtocolScheme::S3, id, path);
        let stamp = Stamp {
            size: 10,
            modified_secs: 5,
        };
        let base = remote_key(&uri("1", "/a.png"), stamp, "head");
        assert_eq!(base, remote_key(&uri("1", "/a.png"), stamp, "head"));
        assert_ne!(
            base,
            remote_key(&uri("2", "/a.png"), stamp, "head"),
            "connection"
        );
        assert_ne!(base, remote_key(&uri("1", "/b.png"), stamp, "head"), "path");
        assert_ne!(
            base,
            remote_key(&uri("1", "/a.png"), stamp, "image"),
            "kind"
        );
        let newer = Stamp {
            modified_secs: 6,
            ..stamp
        };
        assert_ne!(
            base,
            remote_key(&uri("1", "/a.png"), newer, "head"),
            "version"
        );
        assert!(
            base.starts_with("remote:"),
            "cannot collide with local keys"
        );
    }

    #[test]
    fn pictures_and_documents_are_thumbnailable_and_plain_files_are_not() {
        for ext in ["png", "jpg", "heic", "svg", "pdf", "mp4", "docx"] {
            assert!(is_thumbnailable(ext), "{ext}");
        }
        for ext in ["rs", "txt", "zip", "", "json"] {
            assert!(!is_thumbnailable(ext), "{ext}");
        }
    }

    #[test]
    fn ordinary_pictures_use_sips_and_the_rest_quick_look() {
        assert_eq!(generator_for("jpg"), Generator::Sips);
        assert_eq!(generator_for("pdf"), Generator::QuickLook);
        assert_eq!(generator_for("svg"), Generator::QuickLook);
    }

    #[test]
    fn the_cache_key_follows_the_files_identity() {
        let path = Path::new("/a/b.png");
        let stamp = Stamp {
            size: 10,
            modified_secs: 5,
        };
        let key = |path: &Path, stamp: Stamp| cache_key(path, stamp, THUMB_PIXELS);
        assert_eq!(key(path, stamp), key(path, stamp), "deterministic");
        let touched = Stamp {
            modified_secs: 6,
            ..stamp
        };
        let grown = Stamp { size: 11, ..stamp };
        assert_ne!(key(path, stamp), key(path, touched), "mtime");
        assert_ne!(key(path, stamp), key(path, grown), "size");
        assert_ne!(key(path, stamp), key(Path::new("/a/c.png"), stamp), "path");
    }

    #[test]
    fn small_and_large_renders_of_one_file_have_different_keys() {
        let path = Path::new("/a/b.pdf");
        let stamp = Stamp {
            size: 10,
            modified_secs: 5,
        };
        let small = cache_key(path, stamp, THUMB_PIXELS);
        let large = cache_key(path, stamp, PREVIEW_PIXELS);
        assert_ne!(small, large);
        assert!(small.ends_with("@256") && large.ends_with("@1024"));
    }

    fn entry_for(path: &Path, stamp: Stamp) -> FileEntry {
        let mut entry = FileEntry::new(
            VfsUri::local(path),
            path.file_name().unwrap().to_string_lossy(),
            vfs_core::FileKind::File,
        );
        entry.size = stamp.size;
        entry.modified = Some(UNIX_EPOCH + Duration::from_secs(stamp.modified_secs));
        entry
    }

    fn thumbs_now(store: &Store, entry: &FileEntry, pixels: u32) -> Option<Vec<u8>> {
        futures::executor::block_on(thumbnail_bytes(
            Some(store),
            &vfs_core::LocalProvider::new(),
            entry,
            pixels,
        ))
    }

    #[test]
    fn remote_files_over_the_cap_are_never_downloaded() {
        let store = Store::in_memory().unwrap();
        let mut entry = FileEntry::new(
            VfsUri::remote(vfs_core::ProtocolScheme::S3, "1", "/huge.png"),
            "huge.png",
            vfs_core::FileKind::File,
        );
        entry.size = REMOTE_RENDER_BYTES + 1;
        // The local provider would fail on this uri; reaching it would show up as an
        // error entry in the store. Nothing is read and nothing is remembered.
        assert!(thumbs_now(&store, &entry, THUMB_PIXELS).is_none());
        assert_eq!(store.thumbnail_bytes().unwrap(), 0);
    }

    #[test]
    fn a_remembered_remote_failure_answers_without_fetching() {
        let store = Store::in_memory().unwrap();
        let mut entry = FileEntry::new(
            VfsUri::remote(vfs_core::ProtocolScheme::S3, "1", "/odd.png"),
            "odd.png",
            vfs_core::FileKind::File,
        );
        entry.size = 10;
        let key = thumb_key(
            &entry.uri,
            Stamp::new(entry.size, entry.modified),
            THUMB_PIXELS,
        );
        store.put_thumbnail(&key, &[]).unwrap();
        assert!(thumbs_now(&store, &entry, THUMB_PIXELS).is_none());
    }

    #[test]
    fn local_and_remote_keys_never_collide_and_local_keys_are_unchanged() {
        let stamp = Stamp {
            size: 1,
            modified_secs: 1,
        };
        let local = VfsUri::local("/a/b.png");
        let remote = VfsUri::remote(vfs_core::ProtocolScheme::S3, "1", "/a/b.png");
        assert_eq!(
            thumb_key(&local, stamp, THUMB_PIXELS),
            cache_key(Path::new("/a/b.png"), stamp, THUMB_PIXELS)
        );
        assert_ne!(
            thumb_key(&local, stamp, THUMB_PIXELS),
            thumb_key(&remote, stamp, THUMB_PIXELS)
        );
        assert_ne!(
            thumb_key(&remote, stamp, THUMB_PIXELS),
            thumb_key(&remote, stamp, PREVIEW_PIXELS)
        );
    }

    #[test]
    fn both_sizes_are_cached_side_by_side() {
        let store = Store::in_memory().unwrap();
        let path = Path::new("/a/b.pdf");
        let stamp = Stamp {
            size: 1,
            modified_secs: 1,
        };
        store
            .put_thumbnail(&cache_key(path, stamp, THUMB_PIXELS), &[1; 10])
            .unwrap();
        store
            .put_thumbnail(&cache_key(path, stamp, PREVIEW_PIXELS), &[2; 30])
            .unwrap();
        // The preview render is a cache hit even though the file is not on disk.
        let entry = entry_for(path, stamp);
        let hit = thumbs_now(&store, &entry, PREVIEW_PIXELS);
        assert_eq!(hit, Some(vec![2; 30]));
        let small = thumbs_now(&store, &entry, THUMB_PIXELS);
        assert_eq!(small, Some(vec![1; 10]));
        assert_eq!(store.thumbnail_bytes().unwrap(), 40);
    }

    #[test]
    fn tool_arguments_name_the_source_target_and_size() {
        let args = sips_args(Path::new("/in.jpg"), Path::new("/out.png"), THUMB_PIXELS);
        let strings: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(&strings[..2], ["-Z", "256"]);
        assert!(strings.contains(&"/in.jpg".to_string()));
        assert!(strings.ends_with(&["--out".to_string(), "/out.png".to_string()]));
        let ql = quicklook_args(Path::new("/in.pdf"), Path::new("/dir"), PREVIEW_PIXELS);
        assert_eq!(ql.last().unwrap().to_string_lossy(), "/in.pdf");
        assert!(ql.iter().any(|a| a.to_string_lossy() == "/dir"));
        assert!(
            ql.iter().any(|a| a.to_string_lossy() == "1024"),
            "size is a parameter"
        );
    }

    #[test]
    fn a_stamp_without_a_time_counts_as_the_epoch() {
        assert_eq!(Stamp::new(3, None).modified_secs, 0);
        let later = std::time::UNIX_EPOCH + Duration::from_secs(90);
        assert_eq!(Stamp::new(3, Some(later)).modified_secs, 90);
    }

    #[test]
    fn a_stamp_needs_the_file_to_exist_and_changes_when_it_does() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        assert!(Stamp::of(&file).is_none());
        std::fs::write(&file, b"12345").unwrap();
        let first = Stamp::of(&file).unwrap();
        assert_eq!(first.size, 5);
        std::fs::write(&file, b"123456789").unwrap();
        assert_ne!(Stamp::of(&file).unwrap(), first);
    }

    #[test]
    fn a_command_that_overruns_its_limit_is_killed() {
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = Instant::now();
        assert!(!run_with_timeout(command, Duration::from_millis(100)));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_missing_program_is_just_a_failure() {
        let command = Command::new("definitely-not-a-real-program-xyz");
        assert!(!run_with_timeout(command, Duration::from_secs(1)));
    }

    #[test]
    fn a_clean_exit_is_success_and_a_failing_one_is_not() {
        assert!(run_with_timeout(
            Command::new("true"),
            Duration::from_secs(2)
        ));
        assert!(!run_with_timeout(
            Command::new("false"),
            Duration::from_secs(2)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_real_png_is_thumbnailed_and_then_served_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pixel.png");
        std::fs::write(&file, TINY_PNG).unwrap();
        let stamp = Stamp::of(&file).unwrap();
        let store = Store::in_memory().unwrap();

        let entry = entry_for(&file, stamp);
        let first = thumbs_now(&store, &entry, THUMB_PIXELS).expect("sips renders it");
        assert_eq!(&first[..8], b"\x89PNG\r\n\x1a\n");
        assert!(store.thumbnail_bytes().unwrap() > 0, "cached");

        // With the file gone the cache still answers.
        std::fs::remove_file(&file).unwrap();
        let again = thumbs_now(&store, &entry, THUMB_PIXELS).expect("from the store");
        assert_eq!(again, first);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_that_is_not_a_picture_yields_no_thumbnail_and_no_cache_entry() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("broken.png");
        std::fs::write(&file, b"not an image").unwrap();
        let stamp = Stamp::of(&file).unwrap();
        let store = Store::in_memory().unwrap();
        let entry = entry_for(&file, stamp);
        assert!(thumbs_now(&store, &entry, THUMB_PIXELS).is_none());
        assert_eq!(store.thumbnail_bytes().unwrap(), 0);
    }
}
