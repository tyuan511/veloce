//! Quick preview (`Space`): what a file looks like without leaving the list.
//!
//! The point is speed on huge files, so text is read as a bounded head
//! (`PREVIEW_BYTES`, 64 KiB) rather than the whole file: a 10 GB log opens
//! instantly. Images are handed to GPUI's own loader.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    AnyElement, Context, Hsla, ObjectFit, StatefulInteractiveElement, div, img, prelude::*, px, svg,
};
use vfs_core::{
    FileEntry, LocalProvider, VfsProvider,
    entry::{format_size, format_time},
};

use crate::{icons, theme};

/// How much of a text file is read for the preview.
pub const PREVIEW_BYTES: u64 = 64 * 1024;

// The preview budgets are what keep a 10 GB log instant. Enforced at compile time
// so they cannot silently grow into "read the whole file".
const _: () = {
    assert!(PREVIEW_BYTES >= 16 * 1024, "too small to be useful");
    assert!(PREVIEW_BYTES <= 256 * 1024, "would block the UI thread");
    assert!(PREVIEW_LINES >= 100, "too few lines to read a file");
    assert!(PREVIEW_LINES <= 2000, "too many elements per frame");
};
/// Lines rendered before the preview truncates.
pub const PREVIEW_LINES: usize = 400;

/// What the preview panel currently shows.
#[derive(Debug, Clone)]
pub enum PreviewContent {
    Empty,
    Loading { name: String },
    Text { lines: Vec<String>, truncated: bool },
    Image { path: PathBuf },
    Binary { size: u64 },
    Directory { children: usize },
    Error { message: String },
}

/// The preview panel's state. Owned by the workspace, not by a pane, so the two
/// panes can share one inspector.
pub struct Preview {
    pub content: PreviewContent,
    /// Entry currently shown, to avoid redundant reloads.
    pub loaded: Option<String>,
    token: Arc<std::sync::atomic::AtomicBool>,
}

impl Preview {
    pub fn new() -> Self {
        Self {
            content: PreviewContent::Empty,
            loaded: None,
            token: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn clear(&mut self) {
        self.loaded = None;
        self.content = PreviewContent::Empty;
    }

    /// Load a preview for `entry`. Cheap no-op when it is already shown.
    pub fn load(&mut self, entry: Option<&FileEntry>, cx: &mut Context<PreviewView>) {
        let Some(entry) = entry else {
            self.clear();
            cx.notify();
            return;
        };
        let id = entry.id();
        if self.loaded.as_deref() == Some(id.as_str()) {
            return;
        }
        self.loaded = Some(id);
        let entry = entry.clone();
        self.content = PreviewContent::Loading {
            name: entry.name.clone(),
        };
        cx.notify();

        self.token.store(true, std::sync::atomic::Ordering::Relaxed);
        let token = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.token = token.clone();

        cx.spawn(async move |this, cx| {
            let content = build_preview(&entry).await;
            this.update(cx, |preview, cx| {
                if token.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                preview.preview.content = content;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Default for Preview {
    fn default() -> Self {
        Self::new()
    }
}

/// Text extensions rendered as text; everything else unknown is sniffed.
fn is_text_extension(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "go"
            | "py"
            | "js"
            | "mjs"
            | "cjs"
            | "ts"
            | "tsx"
            | "jsx"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "cc"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "php"
            | "sh"
            | "zsh"
            | "bash"
            | "fish"
            | "lua"
            | "zig"
            | "sql"
            | "json"
            | "jsonc"
            | "toml"
            | "yaml"
            | "yml"
            | "ini"
            | "conf"
            | "env"
            | "md"
            | "markdown"
            | "txt"
            | "log"
            | "csv"
            | "tsv"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "less"
            | "vue"
            | "svelte"
            | "lock"
            | "gitignore"
            | "dockerfile"
            | "makefile"
    )
}

fn is_image_extension(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "ico" | "avif"
    )
}

async fn build_preview(entry: &FileEntry) -> PreviewContent {
    let name = entry.name.clone();
    if entry.is_dir() {
        // Count the immediate children: cheap, and it tells you if the folder is
        // worth opening.
        let children = entry
            .uri
            .as_local_path()
            .and_then(|path| std::fs::read_dir(path).ok())
            .map(|entries| entries.flatten().count())
            .unwrap_or(0);
        let _ = name;
        return PreviewContent::Directory { children };
    }

    let extension = entry.extension().unwrap_or_default();
    if is_image_extension(&extension) {
        let Some(path) = entry.uri.as_local_path() else {
            return PreviewContent::Error {
                message: "Remote previews arrive with the SFTP backend".to_string(),
            };
        };
        return PreviewContent::Image { path };
    }

    let provider = LocalProvider::new();
    if is_text_extension(&extension) {
        return read_text_preview(&provider, entry).await;
    }

    // Unknown extension: sniff the first block for NUL bytes.
    match read_head(&provider, entry).await {
        Ok(head) => classify_head(&head, entry.size),
        Err(error) => PreviewContent::Error { message: error },
    }
}

async fn read_text_preview(provider: &LocalProvider, entry: &FileEntry) -> PreviewContent {
    match read_head(provider, entry).await {
        Ok(head) => classify_head(&head, entry.size),
        Err(message) => PreviewContent::Error { message },
    }
}

async fn read_head(provider: &LocalProvider, entry: &FileEntry) -> Result<Vec<u8>, String> {
    use futures::StreamExt as _;
    let stream = provider
        .read_stream(&entry.uri, 0, Some(PREVIEW_BYTES))
        .await
        .map_err(|error| error.to_string())?;
    let mut stream = stream;
    let mut head = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        head.extend_from_slice(&chunk);
        if head.len() as u64 >= PREVIEW_BYTES {
            break;
        }
    }
    Ok(head)
}

/// Decide what a head buffer contains.
///
/// A NUL byte in the first 4 KiB is the cheap, reliable "this is binary" signal —
/// the same heuristic `git` uses. Anything else is shown as text with lossy UTF-8
/// decoding, because a preview must never fail on a stray invalid byte.
fn classify_head(head: &[u8], total_size: u64) -> PreviewContent {
    if head.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0) {
        return PreviewContent::Binary { size: total_size };
    }
    let text = String::from_utf8_lossy(head).to_string();
    let (lines, capped) = split_lines(&text);
    PreviewContent::Text {
        lines,
        truncated: capped || total_size > PREVIEW_BYTES,
    }
}

/// How many leading bytes are inspected for NULs.
pub const BINARY_SNIFF_BYTES: usize = 4096;

/// Split text into display lines, capping the count so a minified bundle cannot
/// blow up the element tree.
fn split_lines(text: &str) -> (Vec<String>, bool) {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if lines.len() >= PREVIEW_LINES {
            return (lines, true);
        }
        // Tabs at 4 columns, control characters stripped.
        let line: String = line
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .collect::<String>()
            .replace('\t', "    ");
        lines.push(line);
    }
    (lines, false)
}

// ---------------------------------------------------------------------------
// view
// ---------------------------------------------------------------------------

/// The preview panel entity.
pub struct PreviewView {
    pub preview: Preview,
    pub focus_handle: gpui::FocusHandle,
    /// Entry metadata shown in the header (kept so the panel can render the
    /// footer without asking the pane again).
    pub entry: Option<FileEntry>,
}

impl gpui::Focusable for PreviewView {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl PreviewView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            preview: Preview::new(),
            focus_handle: cx.focus_handle(),
            entry: None,
        }
    }

    /// Show `entry`, reloading only when it changed.
    pub fn show(&mut self, entry: Option<FileEntry>, cx: &mut Context<Self>) {
        self.entry = entry.clone();
        self.preview.load(entry.as_ref(), cx);
    }
}

impl gpui::Render for PreviewView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        render_preview(&self.preview, self.entry.as_ref(), cx).into_any_element()
    }
}

/// Render the preview panel. Called by `PreviewView::render` and directly by tests.
pub fn render_preview(
    preview: &Preview,
    entry: Option<&FileEntry>,
    _cx: &mut Context<PreviewView>,
) -> AnyElement {
    let header = render_header(entry, &preview.content);
    let body = match &preview.content {
        PreviewContent::Empty => empty_state(),
        PreviewContent::Loading { name } => loading_state(name),
        PreviewContent::Text { lines, truncated } => text_state(lines, *truncated),
        PreviewContent::Image { path } => image_state(path),
        PreviewContent::Binary { size } => binary_state(*size),
        PreviewContent::Directory { children } => directory_state(*children),
        PreviewContent::Error { message } => error_state(message),
    };

    div()
        .flex()
        .flex_col()
        .w(px(340.0))
        .flex_shrink_0()
        .h_full()
        .bg(theme::bg())
        .border_l_1()
        .border_color(theme::border())
        .child(header)
        .child(body)
        .into_any_element()
}

fn render_header(entry: Option<&FileEntry>, content: &PreviewContent) -> AnyElement {
    let name = entry.map(|entry| entry.name.clone()).unwrap_or_default();
    let (icon, tint) = match content {
        PreviewContent::Directory { .. } => (icons::FOLDER, theme::blue()),
        PreviewContent::Image { .. } => (icons::FILE, theme::green()),
        _ => (
            icons::FILE,
            entry.map(theme::kind_color).unwrap_or(theme::text_faint()),
        ),
    };

    let mut details: Vec<String> = Vec::new();
    if let Some(entry) = entry {
        if !entry.is_dir() {
            details.push(format_size(entry.size));
        }
        if entry.modified.is_some() {
            details.push(format_time(entry.modified));
        }
        if let Some(mode) = entry.permissions {
            details.push(format!("{:o}", mode & 0o777));
        }
    }

    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_shrink_0()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(theme::border_soft())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    svg()
                        .path(icon)
                        .w(px(13.))
                        .h(px(13.))
                        .flex_shrink_0()
                        .text_color(tint),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme::text())
                        .truncate()
                        .min_w_0()
                        .child(name),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::text_faint())
                .child(details.join(" · ")),
        )
        .into_any_element()
}

fn empty_state() -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_5()
        .child(
            svg()
                .path(icons::EYE)
                .w(px(22.))
                .h(px(22.))
                .text_color(theme::text_faint()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::text_faint())
                .child("Select a file and press Space"),
        )
        .child(
            div().flex().flex_col().gap_1().mt_3().children(
                crate::actions::SHORTCUTS
                    .iter()
                    .take(6)
                    .map(|(key, label)| shortcut_row(key, label)),
            ),
        )
        .into_any_element()
}

fn shortcut_row(key: &str, label: &str) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            div()
                .px_1()
                .rounded_sm()
                .bg(theme::bg_raised())
                .text_xs()
                .text_color(theme::text_muted())
                .font_family(theme::MONO_FAMILY)
                .child(key.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme::text_faint())
                .child(label.to_string()),
        )
        .into_any_element()
}

fn loading_state(name: &str) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_xs()
                .text_color(theme::text_faint())
                .child(format!("Loading {name}…")),
        )
        .into_any_element()
}

fn text_state(lines: &[String], truncated: bool) -> AnyElement {
    let gutter_width = px(38.0);
    div()
        .id("preview-body")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px_2()
        .py_2()
        .child(
            div()
                .flex()
                .flex_col()
                .font_family(theme::MONO_FAMILY)
                .text_size(px(11.5))
                .line_height(px(16.0))
                .children(lines.iter().enumerate().map(|(index, line)| {
                    let number = index + 1;
                    div()
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .w(gutter_width)
                                .flex_shrink_0()
                                .pr_2()
                                .flex()
                                .justify_end()
                                .text_color(theme::text_faint())
                                .child(number.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_color(theme::text())
                                .child(line.clone()),
                        )
                }))
                .when(truncated, |this| {
                    this.child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(theme::text_faint())
                            .child("… truncated preview"),
                    )
                }),
        )
        .into_any_element()
}

fn image_state(path: &Path) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .p_3()
        .flex()
        .items_center()
        .justify_center()
        .child(
            img(path.to_path_buf())
                .object_fit(ObjectFit::Contain)
                .max_w_full()
                .max_h_full(),
        )
        .into_any_element()
}

fn binary_state(size: u64) -> AnyElement {
    centered_hint(
        icons::FILE,
        "Binary file",
        &format!("{} · no text preview", format_size(size)),
        theme::text_faint(),
    )
}

fn directory_state(children: usize) -> AnyElement {
    centered_hint(
        icons::FOLDER,
        "Folder",
        &format!("{children} items inside"),
        theme::blue(),
    )
}

fn error_state(message: &str) -> AnyElement {
    centered_hint(icons::CLOSE, "Cannot preview", message, theme::red())
}

fn centered_hint(icon: &'static str, title: &str, detail: &str, tint: Hsla) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_5()
        .child(svg().path(icon).w(px(20.)).h(px(20.)).text_color(tint))
        .child(
            div()
                .text_sm()
                .text_color(theme::text_muted())
                .child(title.to_string()),
        )
        .child(
            div()
                .text_xs()
                .max_w(px(260.))
                .text_color(theme::text_faint())
                .child(detail.to_string()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use vfs_core::VfsUri;

    fn entry_for(path: &Path) -> FileEntry {
        let metadata = fs::symlink_metadata(path).unwrap();
        let mut entry = FileEntry::new(
            VfsUri::local(path),
            path.file_name().unwrap().to_string_lossy().to_string(),
            if metadata.is_dir() {
                vfs_core::FileKind::Directory
            } else {
                vfs_core::FileKind::File
            },
        );
        entry.size = metadata.len();
        entry
    }

    #[test]
    fn text_preview_reads_head_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        let line = "x".repeat(80) + "\n";
        fs::write(&path, line.repeat(5000)).unwrap();

        let entry = entry_for(&path);
        let content = futures::executor::block_on(build_preview(&entry));
        match content {
            PreviewContent::Text {
                lines, truncated, ..
            } => {
                assert!(truncated);
                assert_eq!(lines.len(), PREVIEW_LINES);
            }
            other => panic!("expected text preview, got {other:?}"),
        }
    }

    #[test]
    fn binary_detected_by_nul_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.dat");
        fs::write(&path, [0u8, 1, 2, 3, 0, 9]).unwrap();
        let entry = entry_for(&path);
        let content = futures::executor::block_on(build_preview(&entry));
        assert!(matches!(content, PreviewContent::Binary { .. }));
    }

    #[test]
    fn directory_preview_counts_children() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        fs::create_dir(&folder).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(folder.join(name), b"x").unwrap();
        }
        let entry = entry_for(&folder);
        let content = futures::executor::block_on(build_preview(&entry));
        match content {
            PreviewContent::Directory { children, .. } => assert_eq!(children, 3),
            other => panic!("expected directory preview, got {other:?}"),
        }
    }

    #[test]
    fn image_extension_routes_to_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        fs::write(&path, b"not really a png").unwrap();
        let entry = entry_for(&path);
        let content = futures::executor::block_on(build_preview(&entry));
        assert!(matches!(content, PreviewContent::Image { .. }));
    }

    #[test]
    fn split_lines_caps_and_expands_tabs() {
        let text = "a\tb\nc\n".to_string() + &"d\n".repeat(PREVIEW_LINES + 10);
        let (lines, truncated) = split_lines(&text);
        assert!(truncated);
        assert_eq!(lines.len(), PREVIEW_LINES);
        assert_eq!(lines[0], "a    b");
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    fn text_of(content: PreviewContent) -> (Vec<String>, bool) {
        match content {
            PreviewContent::Text { lines, truncated } => (lines, truncated),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn classify_treats_plain_utf8_as_text() {
        let (lines, truncated) = text_of(classify_head(b"hello\nworld\n", 12));
        assert_eq!(lines, ["hello", "world"]);
        assert!(!truncated);
    }

    #[test]
    fn classify_flags_binary_on_the_first_nul() {
        assert!(matches!(
            classify_head(&[b'a', 0, b'b'], 3),
            PreviewContent::Binary { size: 3 }
        ));
    }

    #[test]
    fn classify_ignores_nul_bytes_beyond_the_sniff_window() {
        // A NUL deep inside a text file must not flip it to "binary".
        let mut head = vec![b'a'; BINARY_SNIFF_BYTES];
        head.push(0);
        let (_, _) = text_of(classify_head(&head, head.len() as u64));
    }

    #[test]
    fn classify_survives_invalid_utf8() {
        // Lossy decoding: a preview must never fail because of one bad byte.
        let (lines, _) = text_of(classify_head(&[0xff, 0xfe, b'o', b'k'], 4));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("ok"));
    }

    #[test]
    fn classify_marks_truncated_when_the_file_is_bigger_than_the_head() {
        let (_, truncated) = text_of(classify_head(b"short", 10 * PREVIEW_BYTES));
        assert!(truncated);
    }

    #[test]
    fn split_lines_strips_control_characters_but_keeps_tabs() {
        let (lines, _) = split_lines("a\u{7}b\tc");
        assert_eq!(lines, ["ab    c"]);
    }

    #[test]
    fn split_lines_handles_crlf() {
        let (lines, _) = split_lines("one\r\ntwo\r\n");
        assert_eq!(lines, ["one", "two"]);
    }

    #[test]
    fn split_lines_of_empty_text_is_empty() {
        let (lines, truncated) = split_lines("");
        assert!(lines.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn split_lines_keeps_exactly_the_line_cap() {
        let text = "x\n".repeat(PREVIEW_LINES);
        let (lines, truncated) = split_lines(&text);
        assert_eq!(lines.len(), PREVIEW_LINES);
        assert!(!truncated, "exactly at the cap is not truncated");

        let text = "x\n".repeat(PREVIEW_LINES + 1);
        let (lines, truncated) = split_lines(&text);
        assert_eq!(lines.len(), PREVIEW_LINES);
        assert!(truncated);
    }

    #[test]
    fn text_extensions_cover_code_and_config() {
        for ext in ["rs", "go", "ts", "tsx", "json", "toml", "yaml", "md", "log"] {
            assert!(is_text_extension(ext), "{ext} should preview as text");
        }
        for ext in ["png", "jpg", "zip", "bin", ""] {
            assert!(!is_text_extension(ext), "{ext} should not");
        }
    }

    #[test]
    fn image_extensions_are_recognized() {
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff"] {
            assert!(is_image_extension(ext), "{ext}");
        }
        for ext in ["svg", "pdf", "mp4", "rs"] {
            assert!(!is_image_extension(ext), "{ext}");
        }
    }

    #[test]
    fn sniff_window_fits_inside_the_head_read() {
        assert!(BINARY_SNIFF_BYTES <= PREVIEW_BYTES as usize);
    }
}
