//! "Open With": which applications can open a file, and launching one.
//!
//! The application list comes from LaunchServices (via a tiny JXA script run by
//! `osascript`), because that is the only source Finder's own menu agrees with.
//! Parsing and filtering are pure; only `list_apps` / `open_with` spawn processes.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Applications offered in the submenu before it is cut off.
pub const MAX_APPS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub path: PathBuf,
}

/// JXA: the default app first, then every app LaunchServices knows can open it.
const SCRIPT: &str = r#"function run(argv) {
  ObjC.import("AppKit");
  var ws = $.NSWorkspace.sharedWorkspace;
  var url = $.NSURL.fileURLWithPath(argv[0]);
  var out = [];
  var d = ws.URLForApplicationToOpenURL(url);
  if (d && !d.isNil()) out.push(ObjC.unwrap(d.path));
  var all = ws.URLsForApplicationsToOpenURL(url);
  for (var i = 0; i < all.count; i++) out.push(ObjC.unwrap(all.objectAtIndex(i).path));
  return out.join("\n");
}"#;

/// Whether `path` is an app people expect to see in the menu. LaunchServices also
/// reports helper apps nested inside other bundles and browser test runtimes in
/// cache folders; Finder hides those and so do we.
fn is_listable_app(path: &Path, home: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("app") {
        return false;
    }
    let text = path.to_string_lossy();
    if text.contains("/Contents/") {
        return false;
    }
    path.starts_with("/Applications")
        || path.starts_with("/System/Applications")
        || path.starts_with("/System/Library/CoreServices")
        || path.starts_with(home.join("Applications"))
}

/// Turn the script's output into menu entries: default first, then the rest
/// alphabetically, duplicates and unlistable apps dropped, capped at `MAX_APPS`.
pub fn parse_apps(output: &str, home: &Path) -> Vec<AppInfo> {
    let mut apps: Vec<AppInfo> = Vec::new();
    for line in output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let path = PathBuf::from(line);
        if !is_listable_app(&path, home) || apps.iter().any(|app| app.path == path) {
            continue;
        }
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| line.to_string());
        apps.push(AppInfo { name, path });
    }
    // Keep whatever the system calls the default at the top, even if it is not
    // alphabetically first.
    if apps.len() > 1 {
        apps[1..].sort_by_key(|app| app.name.to_lowercase());
    }
    apps.truncate(MAX_APPS);
    apps
}

/// Applications that can open `path`. Off macOS there is no equivalent registry,
/// so the list is empty and only "Other…" is offered.
pub fn list_apps(path: &Path, home: &Path) -> Result<Vec<AppInfo>, String> {
    if !cfg!(target_os = "macos") {
        return Ok(Vec::new());
    }
    let output = Command::new("osascript")
        .args(["-l", "JavaScript", "-e", SCRIPT])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Cannot list applications: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Cannot list applications: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(parse_apps(&String::from_utf8_lossy(&output.stdout), home))
}

/// JXA: write each app's icon (as LaunchServices draws it, so asset-catalog icons
/// work too) to `argv[2n+1]`; `argv[2n]` is the app. Sizing happens afterwards in
/// `sips`, because the TIFF representation carries every resolution.
const ICON_SCRIPT: &str = r#"function run(argv) {
  ObjC.import("AppKit");
  var ws = $.NSWorkspace.sharedWorkspace;
  for (var i = 0; i + 1 < argv.length; i += 2) {
    var rep = $.NSBitmapImageRep.imageRepWithData(ws.iconForFile(argv[i]).TIFFRepresentation);
    if (!rep || rep.isNil()) continue;
    var png = rep.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $({}));
    if (png && !png.isNil()) png.writeToFileAtomically(argv[i + 1], true);
  }
  return "";
}"#;

/// Pixel size of the menu icons: 16pt drawn at 2x.
pub const MENU_ICON_PIXELS: u32 = 32;
/// Pixel size of the icons shown for `.app` entries: the 64pt grid tile at 2x.
pub const TILE_ICON_PIXELS: u32 = 128;

/// Whether a directory called `name` is an application bundle, which Finder shows
/// as a single launchable item rather than a folder.
pub fn is_app_bundle(name: &str) -> bool {
    name.len() > ".app".len() && name.to_lowercase().ends_with(".app")
}

/// The name Finder shows for an entry: bundles lose their `.app`.
pub fn display_name(name: &str) -> &str {
    if is_app_bundle(name) {
        &name[..name.len() - ".app".len()]
    } else {
        name
    }
}

/// Where the icon of `app` is cached. The path hash keeps two apps with the same
/// name (say /Applications and ~/Applications) from sharing a file; the size keeps
/// the menu's small icons and the tiles' large ones apart.
pub fn icon_cache_path(cache_dir: &Path, app: &Path, pixels: u32) -> PathBuf {
    let hash = app
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
    let stem = app
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    cache_dir.join(format!("{stem}-{hash:016x}-{pixels}.png"))
}

/// Icon PNGs for `apps` (same order), generated on first use and cached on disk.
/// `None` for an app whose icon could not be produced: the row just has no icon.
pub fn app_icons(apps: &[AppInfo], cache_dir: &Path, pixels: u32) -> Vec<Option<PathBuf>> {
    if !cfg!(target_os = "macos") || std::fs::create_dir_all(cache_dir).is_err() {
        return vec![None; apps.len()];
    }
    let targets: Vec<PathBuf> = apps
        .iter()
        .map(|app| icon_cache_path(cache_dir, &app.path, pixels))
        .collect();
    let missing: Vec<usize> = (0..apps.len()).filter(|i| !targets[*i].exists()).collect();
    if !missing.is_empty() {
        let mut command = Command::new("osascript");
        command.args(["-l", "JavaScript", "-e", ICON_SCRIPT]);
        for i in &missing {
            command.arg(&apps[*i].path).arg(&targets[*i]);
        }
        let _ = command.stdin(Stdio::null()).output();
        // 每个 sips 都是独立进程，串行跑一屏图标要好几秒，并行即可。
        std::thread::scope(|scope| {
            for i in &missing {
                let target = &targets[*i];
                if target.exists() {
                    scope.spawn(move || {
                        let _ = Command::new("sips")
                            .args(["-Z", &pixels.to_string()])
                            .arg(target)
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    });
                }
            }
        });
    }
    targets
        .into_iter()
        .map(|target| target.exists().then_some(target))
        .collect()
}

/// Where application icons are cached on disk.
pub fn icon_cache_dir(home: &Path) -> PathBuf {
    home.join("Library/Caches/veloce/app-icons")
}

/// The cached icon of `app`, if one is already on disk: a plain stat, cheap enough
/// for the UI thread, so a warm cache paints on the first frame.
pub fn cached_icon(cache_dir: &Path, app: &Path, pixels: u32) -> Option<PathBuf> {
    Some(icon_cache_path(cache_dir, app, pixels)).filter(|path| path.exists())
}

/// Files of one type are opened by the same applications, so the (slow) lookup is
/// cached per extension. Files without an extension are too varied to share:
/// they get a key of their own.
pub fn app_cache_key(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("ext:{}", ext.to_lowercase()),
        None => format!("path:{}", path.display()),
    }
}

/// `open -a <app> <files...>`.
pub fn open_with_args(app: &Path, files: &[PathBuf]) -> Vec<std::ffi::OsString> {
    let mut args = vec!["-a".into(), app.as_os_str().to_os_string()];
    args.extend(files.iter().map(|file| file.as_os_str().to_os_string()));
    args
}

/// Launch `app` on `files` without waiting for it.
pub fn open_with(app: &Path, files: &[PathBuf]) -> Result<(), String> {
    Command::new("open")
        .args(open_with_args(app, files))
        .stdin(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Cannot open with {}: {error}", app.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/me";

    fn parse(output: &str) -> Vec<AppInfo> {
        parse_apps(output, Path::new(HOME))
    }

    fn names(apps: &[AppInfo]) -> Vec<&str> {
        apps.iter().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn default_app_stays_first_and_the_rest_sort_by_name() {
        let apps = parse(
            "/Applications/Xcode.app\n/Applications/Zed.app\n/System/Applications/TextEdit.app\n/Applications/Atom.app\n",
        );
        assert_eq!(names(&apps), ["Xcode", "Atom", "TextEdit", "Zed"]);
    }

    #[test]
    fn duplicates_and_blank_lines_are_dropped() {
        let apps = parse("/Applications/Zed.app\n\n/Applications/Zed.app\n  \n");
        assert_eq!(names(&apps), ["Zed"]);
    }

    #[test]
    fn nested_helper_apps_and_cache_runtimes_are_hidden() {
        let apps = parse(
            "/Applications/Xcode.app\n\
             /Applications/Xcode.app/Contents/Applications/Instruments.app\n\
             /Users/me/Library/Caches/ms-playwright/chrome-mac/Chromium.app\n\
             /Users/me/.cache/runtime/Office.app\n",
        );
        assert_eq!(names(&apps), ["Xcode"]);
    }

    #[test]
    fn apps_in_the_users_applications_folder_are_kept() {
        let apps = parse("/Users/me/Applications/Mine.app\n/Users/other/Applications/Theirs.app\n");
        assert_eq!(names(&apps), ["Mine"]);
    }

    #[test]
    fn non_app_paths_are_ignored() {
        assert!(parse("/Applications/readme.txt\n/usr/bin/vim\n").is_empty());
    }

    #[test]
    fn names_with_spaces_and_unicode_survive() {
        let apps = parse("/Applications/Google Chrome.app\n/Applications/元宝.app\n");
        assert_eq!(names(&apps), ["Google Chrome", "元宝"]);
    }

    #[test]
    fn the_list_is_capped() {
        let many: String = (0..50)
            .map(|i| format!("/Applications/App{i:02}.app\n"))
            .collect();
        assert_eq!(parse(&many).len(), MAX_APPS);
    }

    #[test]
    fn empty_output_means_no_apps() {
        assert!(parse("").is_empty());
    }

    #[test]
    fn icon_paths_differ_for_same_named_apps_in_different_folders() {
        let dir = Path::new("/cache");
        let a = icon_cache_path(dir, Path::new("/Applications/Zed.app"), 32);
        let b = icon_cache_path(dir, Path::new("/Users/me/Applications/Zed.app"), 32);
        assert_ne!(a, b);
        assert_eq!(
            a,
            icon_cache_path(dir, Path::new("/Applications/Zed.app"), 32)
        );
        assert!(a.starts_with(dir));
    }

    #[test]
    fn icon_paths_differ_per_size() {
        let dir = Path::new("/cache");
        let app = Path::new("/Applications/Zed.app");
        assert_ne!(
            icon_cache_path(dir, app, 32),
            icon_cache_path(dir, app, 128)
        );
    }

    #[test]
    fn app_bundles_are_recognised_by_suffix_and_lose_it_for_display() {
        assert!(is_app_bundle("Safari.app"));
        assert!(is_app_bundle("Safari.APP"));
        assert!(!is_app_bundle(".app"));
        assert!(!is_app_bundle("Utilities"));
        assert!(!is_app_bundle("app"));
        assert_eq!(display_name("Google Chrome.app"), "Google Chrome");
        assert_eq!(display_name("元宝.app"), "元宝");
        assert_eq!(display_name("Utilities"), "Utilities");
        assert_eq!(display_name(".app"), ".app");
    }

    #[test]
    fn cache_keys_group_files_by_lowercased_extension() {
        let key = |p: &str| app_cache_key(Path::new(p));
        assert_eq!(key("/a/Photo.PNG"), key("/b/other.png"));
        assert_ne!(key("/a/x.png"), key("/a/x.jpg"));
    }

    #[test]
    fn extensionless_files_do_not_share_a_cache_entry() {
        let key = |p: &str| app_cache_key(Path::new(p));
        assert_ne!(key("/a/Makefile"), key("/a/LICENSE"));
    }

    #[test]
    fn open_args_put_the_app_before_every_file() {
        let args = open_with_args(
            Path::new("/Applications/Zed.app"),
            &[PathBuf::from("/a b.txt"), PathBuf::from("/c.txt")],
        );
        let strings: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            strings,
            ["-a", "/Applications/Zed.app", "/a b.txt", "/c.txt"]
        );
    }
}
