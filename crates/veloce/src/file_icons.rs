//! File and folder icons from the Material Icon Theme (MIT, the set most VS Code
//! users know). The tables in `file_icons_data` are generated; this module only
//! decides which entry gets which icon.
//!
//! Unlike the Lucide UI glyphs these are full-colour SVGs, so they are drawn with
//! `img()` rather than the tinted `svg()`.

use crate::file_icons_data::{
    ASSETS, DEFAULT_FILE, DEFAULT_FOLDER, DEFAULT_FOLDER_OPEN, FILE_EXTENSIONS, FILE_NAMES,
    FOLDER_NAMES, FOLDER_NAMES_EXPANDED,
};

fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .binary_search_by(|(k, _)| (*k).cmp(key))
        .ok()
        .map(|index| table[index].1)
}

/// Icon id for a file name: exact names (`Cargo.toml`, `.gitignore`) win, then
/// the longest matching extension (`a.test.ts` tries `test.ts`, then `ts`).
pub fn file_icon_id(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if let Some(id) = lookup(FILE_NAMES, &lower) {
        return id;
    }
    let mut rest = lower.as_str();
    while let Some(dot) = rest.find('.') {
        rest = &rest[dot + 1..];
        if let Some(id) = lookup(FILE_EXTENSIONS, rest) {
            return id;
        }
    }
    DEFAULT_FILE
}

/// Icon id for a folder name (`src`, `node_modules`, `.git`, ...).
pub fn folder_icon_id(name: &str, expanded: bool) -> &'static str {
    let lower = name.to_lowercase();
    if expanded {
        lookup(FOLDER_NAMES_EXPANDED, &lower).unwrap_or(DEFAULT_FOLDER_OPEN)
    } else {
        lookup(FOLDER_NAMES, &lower).unwrap_or(DEFAULT_FOLDER)
    }
}

/// Embedded-asset path for an icon id.
pub fn asset_path(id: &str) -> String {
    format!("file-icons/{id}.svg")
}

/// Bytes of an embedded icon, looked up by the id taken from an asset path.
pub fn asset_bytes(path: &str) -> Option<&'static [u8]> {
    let id = path.strip_prefix("file-icons/")?.strip_suffix(".svg")?;
    ASSETS
        .binary_search_by(|(k, _)| (*k).cmp(id))
        .ok()
        .map(|index| ASSETS[index].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_file_names_beat_extensions() {
        assert_ne!(file_icon_id("package.json"), file_icon_id("other.json"));
    }

    #[test]
    fn matches_extensions_case_insensitively() {
        assert_eq!(file_icon_id("MAIN.RS"), file_icon_id("main.rs"));
        assert_ne!(file_icon_id("main.rs"), DEFAULT_FILE);
    }

    #[test]
    fn falls_back_to_the_longest_known_suffix() {
        assert_eq!(
            file_icon_id("app.component.ts"),
            file_icon_id("x.ts").to_owned()
        );
    }

    #[test]
    fn unknown_files_get_the_default_icon() {
        assert_eq!(file_icon_id("data.zzzunknown"), DEFAULT_FILE);
        assert_eq!(file_icon_id("README_NOEXT_XYZ"), DEFAULT_FILE);
    }

    #[test]
    fn special_folders_get_their_own_icon_and_open_variant() {
        assert_ne!(folder_icon_id("src", false), DEFAULT_FOLDER);
        assert_eq!(folder_icon_id("plain-folder", false), DEFAULT_FOLDER);
        assert_eq!(folder_icon_id("plain-folder", true), DEFAULT_FOLDER_OPEN);
    }

    #[test]
    fn every_icon_id_resolves_to_an_embedded_svg() {
        for id in [
            file_icon_id("a.rs"),
            file_icon_id("zzz"),
            folder_icon_id("src", false),
            folder_icon_id("zzz", true),
        ] {
            let bytes = asset_bytes(&asset_path(id)).expect(id);
            assert!(std::str::from_utf8(bytes).unwrap().contains("<svg"), "{id}");
        }
        assert!(asset_bytes("file-icons/nope.svg").is_none());
        assert!(asset_bytes("icons/folder.svg").is_none());
    }

    #[test]
    fn tables_are_sorted_for_binary_search() {
        for table in [
            FILE_EXTENSIONS,
            FILE_NAMES,
            FOLDER_NAMES,
            FOLDER_NAMES_EXPANDED,
        ] {
            assert!(table.windows(2).all(|w| w[0].0 <= w[1].0));
        }
        assert!(ASSETS.windows(2).all(|w| w[0].0 <= w[1].0));
    }
}
