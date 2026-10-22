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

/// Embedded-asset path for an icon id rendered at `size` logical pixels.
///
/// The size is part of the path because GPUI rasterises an SVG once, at twice its
/// intrinsic size, and then scales that bitmap: a 32-unit icon drawn at 96px would
/// be an upscaled 64px bitmap. Baking the target size into the asset makes each
/// size its own (cached) sharp raster.
pub fn asset_path(id: &str, size: f32) -> String {
    format!("file-icons/{id}@{}.svg", size.round().max(1.0) as u32)
}

/// Embedded SVG with its root `width`/`height` set to `px`.
fn sized_svg(svg: &[u8], px: u32) -> Vec<u8> {
    const ROOT: &[u8] = b"<svg";
    let Some(start) = svg.windows(ROOT.len()).position(|w| w == ROOT) else {
        return svg.to_vec();
    };
    let tag_end = svg[start..]
        .iter()
        .position(|&b| b == b'>')
        .map_or(svg.len(), |i| start + i);
    let root = &svg[start..tag_end];
    if root.windows(7).any(|w| w == b" width=") {
        return svg.to_vec();
    }
    let at = start + ROOT.len();
    let mut out = Vec::with_capacity(svg.len() + 32);
    out.extend_from_slice(&svg[..at]);
    out.extend_from_slice(format!(" width=\"{px}\" height=\"{px}\"").as_bytes());
    out.extend_from_slice(&svg[at..]);
    out
}

/// Bytes of an embedded icon, looked up by an asset path from [`asset_path`].
pub fn asset_bytes(path: &str) -> Option<Vec<u8>> {
    let (id, size) = path
        .strip_prefix("file-icons/")?
        .strip_suffix(".svg")?
        .rsplit_once('@')?;
    let px: u32 = size.parse().ok()?;
    ASSETS
        .binary_search_by(|(k, _)| (*k).cmp(id))
        .ok()
        .map(|index| sized_svg(ASSETS[index].1, px))
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
            let bytes = asset_bytes(&asset_path(id, 16.0)).expect(id);
            assert!(
                std::str::from_utf8(&bytes).unwrap().contains("<svg"),
                "{id}"
            );
        }
        assert!(asset_bytes("file-icons/nope@16.svg").is_none());
        assert!(asset_bytes("file-icons/default_file.svg").is_none());
        assert!(asset_bytes("icons/folder.svg").is_none());
    }

    #[test]
    fn bakes_the_target_size_into_the_root_element() {
        let svg = br#"<svg xmlns="x" viewBox="0 0 32 32"><path d="M0 0"/></svg>"#;
        let sized = String::from_utf8(sized_svg(svg, 96)).unwrap();
        assert!(
            sized.starts_with(r#"<svg width="96" height="96" xmlns="x""#),
            "{sized}"
        );
        assert!(sized.ends_with("</svg>"));
    }

    #[test]
    fn keeps_svgs_that_already_declare_a_size() {
        let svg = br#"<svg width="24" height="24" viewBox="0 0 32 32"/>"#;
        assert_eq!(sized_svg(svg, 96), svg);
        assert_eq!(sized_svg(b"not svg", 96), b"not svg");
    }

    #[test]
    fn different_sizes_get_different_asset_paths() {
        assert_ne!(asset_path("x", 16.0), asset_path("x", 96.0));
        assert_eq!(asset_path("x", 15.6), "file-icons/x@16.svg");
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
