//! Finding the terminal applications installed on this Mac, and their icons, for
//! the Settings picker.
//!
//! Discovery is a pure scan of application folders against a list of well-known
//! terminals (so it is tested on temp directories); icon extraction shells out to
//! `plutil` and `sips` and therefore runs on the background executor.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Terminal emulators worth offering, in the order they are listed.
pub const KNOWN: &[&str] = &[
    "Terminal",
    "iTerm",
    "Ghostty",
    "WezTerm",
    "Alacritty",
    "kitty",
    "Warp",
    "Hyper",
    "Tabby",
    "Rio",
    "Wave",
    "Kaku",
];

/// An installed terminal application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    /// The name `open -a` understands (the bundle name without `.app`).
    pub name: String,
    pub bundle: PathBuf,
}

/// Where applications normally live.
pub fn default_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots
}

/// Known terminals present under `roots`, in [`KNOWN`] order. When the same name
/// is installed in several places the earlier root wins.
pub fn find_installed(roots: &[PathBuf]) -> Vec<Terminal> {
    KNOWN
        .iter()
        .filter_map(|name| {
            roots.iter().find_map(|root| {
                let bundle = root.join(format!("{name}.app"));
                bundle.is_dir().then(|| Terminal {
                    name: (*name).to_string(),
                    bundle,
                })
            })
        })
        .collect()
}

/// The `.icns` file an app declares in `CFBundleIconFile`, which may omit the
/// extension.
pub fn icon_file(bundle: &Path, declared: &str) -> Option<PathBuf> {
    let declared = declared.trim();
    if declared.is_empty() || declared.contains('/') {
        return None;
    }
    let file = if declared.ends_with(".icns") {
        declared.to_string()
    } else {
        format!("{declared}.icns")
    };
    Some(bundle.join("Contents/Resources").join(file))
}

/// PNG bytes of the app's icon at `pixels`, or `None` when it cannot be read.
pub fn icon_png(bundle: &Path, pixels: u32) -> Option<Vec<u8>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = Command::new("plutil")
        .args(["-extract", "CFBundleIconFile", "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    let declared = String::from_utf8(output.stdout).ok()?;
    let icns = icon_file(bundle, &declared).filter(|path| path.is_file())?;
    let work = std::env::temp_dir().join(format!("veloce-appicon-{}", std::process::id()));
    std::fs::create_dir_all(&work).ok()?;
    let out = work.join(format!("{}.png", bundle.file_stem()?.to_string_lossy()));
    let mut command = Command::new("sips");
    command
        .args(["-s", "format", "png", "-Z", &pixels.to_string()])
        .arg(&icns)
        .arg("--out")
        .arg(&out);
    let png = if crate::thumbs::run_with_timeout(command, Duration::from_secs(5)) {
        std::fs::read(&out).ok().filter(|bytes| !bytes.is_empty())
    } else {
        None
    };
    let _ = std::fs::remove_file(&out);
    png
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join(format!("{name}.app/Contents"))).unwrap();
    }

    #[test]
    fn finds_known_terminals_in_the_known_order() {
        let root = tempfile::tempdir().unwrap();
        app(root.path(), "Ghostty");
        app(root.path(), "Terminal");
        app(root.path(), "Safari");
        let found = find_installed(&[root.path().to_path_buf()]);
        let names: Vec<&str> = found.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Terminal", "Ghostty"], "Safari is not a terminal");
    }

    #[test]
    fn an_earlier_root_wins_for_duplicates() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        app(first.path(), "iTerm");
        app(second.path(), "iTerm");
        let found = find_installed(&[first.path().to_path_buf(), second.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].bundle.starts_with(first.path()));
    }

    #[test]
    fn a_plain_file_named_like_an_app_does_not_count() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Warp.app"), b"").unwrap();
        assert!(find_installed(&[root.path().to_path_buf()]).is_empty());
    }

    #[test]
    fn missing_roots_and_empty_roots_find_nothing() {
        assert!(find_installed(&[]).is_empty());
        assert!(find_installed(&[PathBuf::from("/definitely/not/here")]).is_empty());
    }

    #[test]
    fn icon_names_get_their_extension_but_cannot_escape_the_bundle() {
        let bundle = Path::new("/Applications/Ghostty.app");
        assert_eq!(
            icon_file(bundle, "Ghostty\n").unwrap(),
            Path::new("/Applications/Ghostty.app/Contents/Resources/Ghostty.icns")
        );
        assert_eq!(
            icon_file(bundle, "App.icns").unwrap().file_name().unwrap(),
            "App.icns"
        );
        assert!(icon_file(bundle, "").is_none());
        assert!(icon_file(bundle, "../../evil").is_none());
    }
}
