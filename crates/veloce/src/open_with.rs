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
