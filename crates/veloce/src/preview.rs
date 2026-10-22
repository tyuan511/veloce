//! Quick preview (`Space`): what a file looks like without leaving the list.
//!
//! The point is speed on huge files, so text is read as a bounded head
//! (`PREVIEW_BYTES`, 64 KiB) rather than the whole file: a 10 GB log opens
//! instantly. Images are handed to GPUI's own loader.

mod code;
mod markdown;
mod table;
mod ts;
mod views;

use crate::i18n::{tr, trf};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use gpui::{
    AnyElement, Bounds, Context, Corners, Hsla, ObjectFit, ScrollHandle,
    StatefulInteractiveElement, canvas, div, fill, img, prelude::*, px, svg,
};
use veloce_store::Store;
use vfs_core::{
    FileEntry, ProviderRegistry, VfsProvider,
    archive::{ArchiveKind, list_entries},
    entry::{format_size, format_time},
};

use crate::{file_icons, icons, theme, thumbs};

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
    Loading {
        name: String,
    },
    Text {
        lines: Vec<String>,
        truncated: bool,
    },
    /// Syntax-highlighted source.
    Code {
        lines: Vec<code::Line>,
        truncated: bool,
    },
    Markdown {
        blocks: Vec<markdown::Block>,
        truncated: bool,
    },
    /// CSV / TSV rows (first row is the header).
    Table {
        rows: Vec<Vec<String>>,
        truncated: bool,
    },
    Image {
        path: PathBuf,
    },
    /// A rendered picture held in memory (a Quick Look thumbnail).
    Picture {
        image: Arc<gpui::Image>,
    },
    /// Not text: size plus a hex dump of the head so it is still inspectable.
    Binary {
        size: u64,
        dump: Vec<String>,
    },
    /// Entry names of an archive, as listed by `tar` (capped).
    Archive {
        entries: Vec<String>,
        truncated: bool,
    },
    /// Immediate children (name, is_dir), capped.
    Directory {
        children: usize,
        names: Vec<(String, bool)>,
    },
    Error {
        message: String,
    },
}

/// The preview panel's state. Owned by the workspace, not by a pane, so the two
/// panes can share one inspector.
pub struct Preview {
    pub content: PreviewContent,
    /// Entry currently shown, to avoid redundant reloads.
    pub loaded: Option<String>,
    token: Arc<std::sync::atomic::AtomicBool>,
    /// Where rendered thumbnails are cached (shared with the icon view).
    store: Option<Arc<Store>>,
    /// Where remote entries find the provider that can read them.
    registry: Arc<RwLock<ProviderRegistry>>,
}

impl Preview {
    pub fn new(store: Option<Arc<Store>>, registry: Arc<RwLock<ProviderRegistry>>) -> Self {
        Self {
            store,
            registry,
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

        let store = self.store.clone();
        let provider = self
            .registry
            .read()
            .map_err(|_| tr("The connection list is unavailable").to_string())
            .and_then(|registry| {
                registry
                    .for_uri(&entry.uri)
                    .map_err(|error| error.to_string())
            });
        cx.spawn(async move |this, cx| {
            // Reading files and running `tar` / `qlmanage` must not block the UI.
            let content = cx
                .background_executor()
                .spawn(async move {
                    match provider {
                        Ok(provider) => {
                            build_preview(&entry, store.as_deref(), provider.as_ref()).await
                        }
                        Err(message) => PreviewContent::Error { message },
                    }
                })
                .await;
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
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "ico" | "avif" | "svg"
    )
}

/// Formats GPUI cannot decode but macOS Quick Look can render as a thumbnail:
/// documents, video, audio artwork, camera RAW, HEIC, fonts and 3D models.
pub(crate) fn is_thumbnail_extension(ext: &str) -> bool {
    matches!(
        ext,
        "pdf"
            | "html"
            | "htm"
            | "heic"
            | "heif"
            | "psd"
            | "ai"
            | "eps"
            | "cr2"
            | "cr3"
            | "nef"
            | "arw"
            | "dng"
            | "raf"
            | "orf"
            | "mp4"
            | "m4v"
            | "mov"
            | "avi"
            | "mkv"
            | "webm"
            | "mp3"
            | "m4a"
            | "wav"
            | "aac"
            | "flac"
            | "aiff"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "pages"
            | "numbers"
            | "key"
            | "rtf"
            | "rtfd"
            | "odt"
            | "ods"
            | "odp"
            | "epub"
            | "ttf"
            | "otf"
            | "ttc"
            | "usdz"
            | "obj"
            | "stl"
            | "dae"
    )
}

/// How a file is previewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// Decoded by GPUI directly.
    Image,
    /// Rendered to a PNG by the system's Quick Look.
    Thumbnail,
    Archive,
    Text,
    /// Unknown: look at the first bytes to decide text vs binary.
    Sniff,
}

fn route_for(name: &str, extension: &str, thumbnails: bool) -> Route {
    if is_image_extension(extension) {
        Route::Image
    } else if thumbnails && is_thumbnail_extension(extension) {
        Route::Thumbnail
    } else if ArchiveKind::detect(name).is_some() {
        Route::Archive
    } else if is_text_extension(extension) {
        Route::Text
    } else {
        Route::Sniff
    }
}

/// Archive entries shown before the list is cut off.
pub const ARCHIVE_PREVIEW_ENTRIES: usize = 500;
/// Folder entries shown before the list is cut off.
pub const DIRECTORY_PREVIEW_ENTRIES: usize = 300;
/// Bytes of a binary file shown as hex.
pub const HEX_PREVIEW_BYTES: usize = 512;

/// `xxd`-style rows: offset, 16 hex bytes, printable ASCII.
pub fn hex_dump(bytes: &[u8], max_bytes: usize) -> Vec<String> {
    bytes
        .iter()
        .take(max_bytes)
        .copied()
        .collect::<Vec<u8>>()
        .chunks(16)
        .enumerate()
        .map(|(row, chunk)| {
            let hex: Vec<String> = (0..16)
                .map(|i| {
                    chunk
                        .get(i)
                        .map(|byte| format!("{byte:02x}"))
                        .unwrap_or_else(|| "  ".to_string())
                })
                .collect();
            let ascii: String = chunk
                .iter()
                .map(|byte| {
                    if byte.is_ascii_graphic() || *byte == b' ' {
                        *byte as char
                    } else {
                        '.'
                    }
                })
                .collect();
            format!("{:08x}  {}  {}", row * 16, hex.join(" "), ascii)
        })
        .collect()
}

/// Whether this platform can render Quick Look thumbnails.
const THUMBNAILS: bool = cfg!(target_os = "macos");

/// Build the preview for any entry, local or remote: everything goes through the
/// entry's provider, and the only differences are the ones that are real. A local
/// file is already on disk, so images are shown straight from it and archives are
/// listed in place; a remote one has to be fetched, so only a bounded head is read,
/// or the whole file once to render a small picture (which is what gets cached).
async fn build_preview(
    entry: &FileEntry,
    store: Option<&Store>,
    provider: &dyn VfsProvider,
) -> PreviewContent {
    if entry.is_dir() {
        return directory_preview(provider, entry).await;
    }
    let extension = entry.extension().unwrap_or_default();
    let local = entry.uri.as_local_path();
    let is_text = is_text_extension(&extension);
    let from_head = |as_text: bool| head_preview(provider, entry, as_text);

    match route_for(&entry.name, &extension, THUMBNAILS) {
        Route::Image if local.is_some() => PreviewContent::Image {
            path: local.unwrap_or_default(),
        },
        Route::Image | Route::Thumbnail => {
            match thumbs::thumbnail_bytes(store, provider, entry, thumbs::PREVIEW_PIXELS).await {
                Some(png) => PreviewContent::Picture {
                    image: Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)),
                },
                // No picture (DRM media, odd codec, too big to fetch ...): HTML falls
                // back to its source, everything else to the bytes.
                None => from_head(is_text).await,
            }
        }
        Route::Archive => {
            // Listing needs the archive on disk; a remote one shows its bytes.
            let Some(path) = local else {
                return from_head(false).await;
            };
            let listed = ArchiveKind::detect(&entry.name)
                .ok_or_else(|| tr("Not an archive").to_string())
                .and_then(|kind| list_entries(kind, &path, ARCHIVE_PREVIEW_ENTRIES + 1));
            match listed {
                Ok(mut entries) => {
                    let truncated = entries.len() > ARCHIVE_PREVIEW_ENTRIES;
                    entries.truncate(ARCHIVE_PREVIEW_ENTRIES);
                    PreviewContent::Archive { entries, truncated }
                }
                // Single-file compressions and unreadable archives still get bytes.
                Err(_) => from_head(false).await,
            }
        }
        Route::Text => from_head(true).await,
        Route::Sniff => from_head(false).await,
    }
}

async fn directory_preview(provider: &dyn VfsProvider, entry: &FileEntry) -> PreviewContent {
    use futures::StreamExt as _;
    // A local folder is counted in full; a remote listing is a paid, slow request per
    // page, so it stops once the panel is full.
    let limit = if entry.uri.scheme.is_local() {
        usize::MAX
    } else {
        DIRECTORY_PREVIEW_ENTRIES + 1
    };
    let mut names: Vec<(String, bool)> = Vec::new();
    if let Ok(mut stream) = provider.list_stream(&entry.uri).await {
        while let Some(Ok(chunk)) = stream.next().await {
            names.extend(chunk.into_iter().map(|child| {
                let is_dir = child.is_dir();
                (child.name, is_dir)
            }));
            if names.len() >= limit {
                break;
            }
        }
    }
    let children = names.len();
    // Folders first, then names, so the cut-off keeps the most useful rows.
    names.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    names.truncate(DIRECTORY_PREVIEW_ENTRIES);
    PreviewContent::Directory { children, names }
}

/// Preview from the first bytes of the file. The bytes themselves are never cached,
/// local or remote: only the rendered picture is (see `thumbs::thumbnail_bytes`).
async fn head_preview(
    provider: &dyn VfsProvider,
    entry: &FileEntry,
    as_text: bool,
) -> PreviewContent {
    match read_head(provider, entry).await {
        Ok(head) if as_text => text_preview_from_head(entry, &head),
        Ok(head) => classify_head(&head, entry.size),
        Err(message) => PreviewContent::Error { message },
    }
}

fn text_preview_from_head(entry: &FileEntry, head: &[u8]) -> PreviewContent {
    let extension = entry.extension().unwrap_or_default();
    // Binary property lists are common on macOS; `plutil` turns them into XML.
    if extension == "plist"
        && let Some(path) = entry.uri.as_local_path()
        && let Some(xml) = plist_as_xml(&path)
    {
        return text_content(&entry.name, "xml", &xml, false);
    }
    if head.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0) {
        return classify_head(head, entry.size);
    }
    let text = String::from_utf8_lossy(head);
    text_content(&entry.name, &extension, &text, entry.size > PREVIEW_BYTES)
}

#[cfg(target_os = "macos")]
fn plist_as_xml(path: &Path) -> Option<String> {
    let output = std::process::Command::new("plutil")
        .args(["-convert", "xml1", "-o", "-"])
        .arg(path)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(not(target_os = "macos"))]
fn plist_as_xml(_path: &Path) -> Option<String> {
    None
}

/// Choose the richest view of already-decoded text: Markdown, a table, pretty
/// JSON, highlighted code, or plain lines. `file_truncated` says the text is only
/// the head of a longer file.
fn text_content(name: &str, extension: &str, text: &str, file_truncated: bool) -> PreviewContent {
    match extension {
        "md" | "markdown" | "mdown" | "mkd" => {
            return PreviewContent::Markdown {
                blocks: markdown::parse(text),
                truncated: file_truncated,
            };
        }
        "csv" | "tsv" => {
            let (rows, cut) = table::parse_delimited(
                text,
                table::delimiter_for(extension),
                TABLE_PREVIEW_ROWS,
                TABLE_PREVIEW_COLUMNS,
            );
            if !rows.is_empty() {
                return PreviewContent::Table {
                    rows,
                    truncated: cut || file_truncated,
                };
            }
        }
        _ => {}
    }

    // Pretty-print JSON only when the whole file is here: a cut-off document is
    // invalid and would silently show as raw text anyway.
    let pretty;
    let source = if extension == "json" && !file_truncated {
        match serde_json::from_str::<serde_json::Value>(text)
            .and_then(|value| serde_json::to_string_pretty(&value))
        {
            Ok(formatted) => {
                pretty = formatted;
                pretty.as_str()
            }
            Err(_) => text,
        }
    } else {
        text
    };

    let capped = source.lines().count() > PREVIEW_LINES;
    if let Some(lines) = code::highlight(source, name, extension, theme::is_dark(), PREVIEW_LINES) {
        return PreviewContent::Code {
            lines,
            truncated: capped || file_truncated,
        };
    }
    let (lines, over) = split_lines(source);
    PreviewContent::Text {
        lines,
        truncated: over || file_truncated,
    }
}

/// Rows and columns kept for CSV / TSV previews.
pub const TABLE_PREVIEW_ROWS: usize = 200;
pub const TABLE_PREVIEW_COLUMNS: usize = 40;

async fn read_head(provider: &dyn VfsProvider, entry: &FileEntry) -> Result<Vec<u8>, String> {
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
        return PreviewContent::Binary {
            size: total_size,
            dump: hex_dump(head, HEX_PREVIEW_BYTES),
        };
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
    /// Panel width: the inspector is narrow. Ignored when `embedded`.
    pub width: f32,
    /// Shared by the text/code/hex bodies: only one is on screen at a time.
    h_scroll: ScrollHandle,
}

/// What the inspector asks of the workspace.
#[derive(Debug, Clone, Copy)]
pub enum PreviewEvent {
    /// Open the entry with its default application.
    Open,
    /// Show the application list below this point.
    OpenWith(gpui::Point<gpui::Pixels>),
}

impl gpui::EventEmitter<PreviewEvent> for PreviewView {}

impl gpui::Focusable for PreviewView {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl PreviewView {
    pub fn new(
        store: Option<Arc<Store>>,
        registry: Arc<RwLock<ProviderRegistry>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            preview: Preview::new(store, registry),
            focus_handle: cx.focus_handle(),
            entry: None,
            width: 340.0,
            h_scroll: ScrollHandle::new(),
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
        render_preview(
            &self.preview,
            self.entry.as_ref(),
            self.width,
            &self.h_scroll,
            cx,
        )
        .into_any_element()
    }
}

/// Render the preview panel. Called by `PreviewView::render` and directly by tests.
pub fn render_preview(
    preview: &Preview,
    entry: Option<&FileEntry>,
    width: f32,
    h_scroll: &ScrollHandle,
    cx: &mut Context<PreviewView>,
) -> AnyElement {
    let header = render_header(entry, cx);
    let body = match &preview.content {
        PreviewContent::Empty => empty_state(),
        PreviewContent::Loading { name } => loading_state(name),
        PreviewContent::Text { lines, truncated } => {
            with_hbar(text_state(lines, *truncated, h_scroll), h_scroll)
        }
        PreviewContent::Code { lines, truncated } => {
            with_hbar(views::code_state(lines, *truncated, h_scroll), h_scroll)
        }
        PreviewContent::Markdown { blocks, truncated } => views::markdown_state(blocks, *truncated),
        PreviewContent::Table { rows, truncated } => views::table_state(rows, *truncated),
        PreviewContent::Image { path } => image_state(path.clone()),
        PreviewContent::Picture { image } => image_state(image.clone()),
        PreviewContent::Binary { size, dump } => {
            with_hbar(binary_state(*size, dump, h_scroll), h_scroll)
        }
        PreviewContent::Archive { entries, truncated } => archive_state(entries, *truncated),
        PreviewContent::Directory { children, names } => directory_state(*children, names),
        PreviewContent::Error { message } => error_state(message),
    };

    div()
        .flex()
        .flex_col()
        .w(px(width))
        .flex_shrink_0()
        .h_full()
        .bg(theme::bg())
        .child(header)
        .child(body)
        .into_any_element()
}

/// The inspector's header: a large icon with the name and "kind · size", then a
/// quiet grid of dates and permissions, like Finder's preview pane.
fn render_header(entry: Option<&FileEntry>, cx: &mut Context<PreviewView>) -> AnyElement {
    let Some(entry) = entry else {
        return div().into_any_element();
    };
    let subtitle = if entry.is_dir() {
        crate::info::kind_label(entry)
    } else {
        format!(
            "{} · {}",
            crate::info::kind_label(entry),
            format_size(entry.size)
        )
    };
    let facts = header_facts(entry);

    div()
        .flex()
        .flex_col()
        .gap_3()
        .flex_shrink_0()
        .p_4()
        .border_b_1()
        .border_color(theme::border_soft())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(crate::pane::entry_icon(entry, 32.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::text())
                                .line_clamp(2)
                                .child(entry.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme::text_faint())
                                .truncate()
                                .child(subtitle),
                        ),
                )
                .child(open_button(cx)),
        )
        .when(!facts.is_empty(), |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(facts.into_iter().map(|(label, value)| {
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .gap_3()
                            .text_xs()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(theme::text_faint())
                                    .child(label),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme::text_muted())
                                    .child(value),
                            )
                    })),
            )
        })
        .into_any_element()
}

/// A split button: the label opens with the default application, the arrow lists
/// every application that can open the file (the same list as "Open With").
fn open_button(cx: &mut Context<PreviewView>) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .h(px(24.))
        .rounded_md()
        .bg(theme::bg_hover())
        .text_xs()
        .text_color(theme::text())
        .child(
            div()
                .id("inspector-open")
                .h_full()
                .px_2()
                .flex()
                .items_center()
                .rounded_l_md()
                .cursor_pointer()
                .hover(|this| this.bg(theme::border()))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(PreviewEvent::Open)))
                .child(tr("Open")),
        )
        .child(div().w(px(1.)).h(px(12.)).bg(theme::border()))
        .child(
            div()
                .id("inspector-open-with")
                .h_full()
                .w(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_r_md()
                .cursor_pointer()
                .hover(|this| this.bg(theme::border()))
                .on_click(cx.listener(|_, event: &gpui::ClickEvent, _, cx| {
                    cx.emit(PreviewEvent::OpenWith(event.position()));
                }))
                .child(
                    svg()
                        .path(icons::CHEVRON_DOWN)
                        .w(px(10.))
                        .h(px(10.))
                        .text_color(theme::text_muted()),
                ),
        )
        .into_any_element()
}

/// Rows under the header: only what is known, so an entry without a creation
/// time does not show an empty line.
pub(crate) fn header_facts(entry: &FileEntry) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    if entry.modified.is_some() {
        facts.push((tr("Modified"), format_time(entry.modified)));
    }
    if entry.created.is_some() {
        facts.push((tr("Created"), format_time(entry.created)));
    }
    if let Some(mode) = crate::info::permissions_label(entry.permissions) {
        facts.push((tr("Permissions"), mode));
    }
    facts
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
                .child(tr("Select a file and press Space")),
        )
        .child(
            div().flex().flex_col().gap_1().mt_3().children(
                crate::actions::SHORTCUTS
                    .iter()
                    .take(6)
                    .map(|(key, label)| shortcut_row(key, tr(label))),
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
        .px_4()
        .min_w_0()
        .child(
            // A long file name must be cut with an ellipsis, not run past the panel.
            div()
                .max_w_full()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(theme::text_faint())
                .child(trf("Loading {}…", &[name])),
        )
        .into_any_element()
}

fn text_state(lines: &[String], truncated: bool, handle: &ScrollHandle) -> AnyElement {
    let gutter_width = px(38.0);
    div()
        .id("preview-body")
        .track_scroll(handle)
        // 横向 flex：子列按内容宽度排布而不是被拉伸到容器宽，滚动范围才取得到
        .flex()
        .flex_1()
        .min_h_0()
        .overflow_scroll()
        .px_2()
        .py_2()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
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
                                .flex_shrink_0()
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

fn image_state(source: impl Into<gpui::ImageSource>) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .p_3()
        .flex()
        .items_center()
        .justify_center()
        .child(
            img(source)
                .object_fit(ObjectFit::Contain)
                .max_w_full()
                .max_h_full(),
        )
        .into_any_element()
}

fn binary_state(size: u64, dump: &[String], handle: &ScrollHandle) -> AnyElement {
    if dump.is_empty() {
        return centered_hint(
            icons::FILE,
            tr("Binary file"),
            &trf("{} · no preview", &[&format_size(size)]),
            theme::text_faint(),
        );
    }
    div()
        .id("preview-hex")
        .track_scroll(handle)
        .flex()
        .flex_1()
        .min_h_0()
        .overflow_scroll()
        .px_3()
        .py_2()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .child(
                    div()
                        .mb_2()
                        .text_xs()
                        .text_color(theme::text_faint())
                        .child(trf(
                            "Binary · {} · first {} bytes",
                            &[&format_size(size), &HEX_PREVIEW_BYTES.to_string()],
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .font_family(theme::MONO_FAMILY)
                        .text_size(px(11.))
                        .line_height(px(15.))
                        .text_color(theme::text_muted())
                        .children(
                            dump.iter()
                                .map(|row| div().whitespace_nowrap().child(row.clone())),
                        ),
                ),
        )
        .into_any_element()
}

/// One icon + name row, used by the archive and folder listings.
fn list_row(name: &str, is_dir: bool, depth: usize) -> AnyElement {
    let id = if is_dir {
        file_icons::folder_icon_id(name, false)
    } else {
        file_icons::file_icon_id(name)
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(22.))
        .pl(px(12.0 * depth as f32))
        .child(
            img(file_icons::asset_path(id, 14.0))
                .w(px(14.))
                .h(px(14.))
                .flex_shrink_0(),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(theme::text())
                .child(name.to_string()),
        )
        .into_any_element()
}

/// Archive entry paths become indented names: `src/main.rs` shows as `main.rs`
/// one level in, which keeps the tree readable without building one.
fn archive_rows(entries: &[String]) -> Vec<(String, bool, usize)> {
    entries
        .iter()
        .map(|entry| {
            let is_dir = entry.ends_with('/');
            let trimmed = entry.trim_end_matches('/');
            let depth = trimmed.matches('/').count();
            let name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
            (name, is_dir, depth)
        })
        .collect()
}

fn archive_state(entries: &[String], truncated: bool) -> AnyElement {
    let rows = archive_rows(entries);
    div()
        .id("preview-archive")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px_3()
        .py_2()
        .child(
            div()
                .mb_2()
                .text_xs()
                .text_color(theme::text_faint())
                .child(if truncated {
                    trf("Archive · first {} entries", &[&entries.len().to_string()])
                } else {
                    trf("Archive · {} entries", &[&entries.len().to_string()])
                }),
        )
        .children(
            rows.iter()
                .map(|(name, is_dir, depth)| list_row(name, *is_dir, *depth)),
        )
        .into_any_element()
}

fn directory_state(children: usize, names: &[(String, bool)]) -> AnyElement {
    if names.is_empty() {
        return centered_hint(
            icons::FOLDER,
            tr("Folder"),
            &trf("{} items inside", &[&children.to_string()]),
            theme::blue(),
        );
    }
    div()
        .id("preview-folder")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px_3()
        .py_2()
        .child(
            div()
                .mb_2()
                .text_xs()
                .text_color(theme::text_faint())
                .child(if children > names.len() {
                    format!("{children} items · showing {}", names.len())
                } else {
                    format!("{children} items")
                }),
        )
        .children(
            names
                .iter()
                .map(|(name, is_dir)| list_row(name, *is_dir, 0)),
        )
        .into_any_element()
}

fn error_state(message: &str) -> AnyElement {
    centered_hint(icons::CLOSE, tr("Cannot preview"), message, theme::red())
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
        let content = futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        ));
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
        let content = futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        ));
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
        let content = futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        ));
        match content {
            PreviewContent::Directory { children, names } => {
                assert_eq!(children, 3);
                assert_eq!(names.len(), 3);
            }
            other => panic!("expected directory preview, got {other:?}"),
        }
    }

    #[test]
    fn image_extension_routes_to_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        fs::write(&path, b"not really a png").unwrap();
        let entry = entry_for(&path);
        let content = futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        ));
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

    #[test]
    fn markdown_files_become_markdown_content() {
        match text_content("README.md", "md", "# Hi\n\ntext", false) {
            PreviewContent::Markdown { blocks, truncated } => {
                assert!(!truncated);
                assert!(blocks.len() >= 2);
            }
            other => panic!("expected markdown, got {other:?}"),
        }
    }

    #[test]
    fn a_cut_off_file_marks_markdown_as_truncated() {
        match text_content("a.md", "md", "# Hi", true) {
            PreviewContent::Markdown { truncated, .. } => assert!(truncated),
            other => panic!("expected markdown, got {other:?}"),
        }
    }

    #[test]
    fn csv_files_become_tables_and_tsv_uses_tabs() {
        match text_content("d.csv", "csv", "a,b\n1,2\n", false) {
            PreviewContent::Table { rows, .. } => assert_eq!(rows, [["a", "b"], ["1", "2"]]),
            other => panic!("expected table, got {other:?}"),
        }
        match text_content("d.tsv", "tsv", "a\tb\n", false) {
            PreviewContent::Table { rows, .. } => assert_eq!(rows[0], ["a", "b"]),
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn empty_csv_falls_back_to_text() {
        assert!(matches!(
            text_content("d.csv", "csv", "", false),
            PreviewContent::Text { .. }
        ));
    }

    #[test]
    fn complete_json_is_pretty_printed_before_highlighting() {
        match text_content("a.json", "json", "{\"a\":[1,2]}", false) {
            PreviewContent::Code { lines, .. } => {
                assert_eq!(
                    code::language_name("a.json", "json").as_deref(),
                    Some("JSON")
                );
                assert!(lines.len() > 1, "expanded onto several lines");
            }
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn truncated_json_is_shown_as_is_not_reformatted() {
        match text_content("a.json", "json", "{\"a\":[1,", true) {
            PreviewContent::Code {
                lines, truncated, ..
            } => {
                assert_eq!(lines.len(), 1);
                assert!(truncated);
            }
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_stays_raw() {
        match text_content("a.json", "json", "{oops", false) {
            PreviewContent::Code { lines, .. } => assert_eq!(lines.len(), 1),
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn source_files_are_highlighted_and_unknown_text_stays_plain() {
        assert!(matches!(
            text_content("m.rs", "rs", "fn main() {}", false),
            PreviewContent::Code { .. }
        ));
        assert!(matches!(
            text_content("x.log", "log", "line", false),
            PreviewContent::Text { .. }
        ));
    }

    #[test]
    fn long_source_is_capped_and_flagged() {
        let text = "let a = 1;\n".repeat(PREVIEW_LINES + 50);
        match text_content("a.js", "js", &text, false) {
            PreviewContent::Code {
                lines, truncated, ..
            } => {
                assert_eq!(lines.len(), PREVIEW_LINES);
                assert!(truncated);
            }
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn html_is_rendered_by_quick_look_when_available_and_read_as_source_otherwise() {
        assert_eq!(route_for("a.html", "html", true), Route::Thumbnail);
        assert_eq!(route_for("a.html", "html", false), Route::Text);
        assert!(matches!(
            text_content("a.html", "html", "<p>hi</p>", false),
            PreviewContent::Code { .. }
        ));
    }

    #[test]
    fn markdown_file_on_disk_previews_as_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        fs::write(&path, "# Title\n\n- a\n- b\n").unwrap();
        let entry = entry_for(&path);
        let content = futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        ));
        assert!(
            matches!(content, PreviewContent::Markdown { .. }),
            "{content:?}"
        );
    }

    #[test]
    fn binary_data_with_a_text_extension_still_shows_hex() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("weird.json");
        fs::write(&path, [0u8, 159, 146, 150]).unwrap();
        let entry = entry_for(&path);
        let content = futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        ));
        assert!(
            matches!(content, PreviewContent::Binary { .. }),
            "{content:?}"
        );
    }

    #[test]
    fn header_facts_list_only_what_is_known() {
        let mut entry = FileEntry::new(
            vfs_core::VfsUri::local("/tmp/a.txt"),
            "a.txt",
            vfs_core::FileKind::File,
        );
        assert!(header_facts(&entry).is_empty());
        entry.modified = Some(std::time::SystemTime::now());
        entry.permissions = Some(0o100644);
        let facts = header_facts(&entry);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].0, "Modified");
        assert_eq!(facts[1], ("Permissions", "644".to_string()));
        entry.created = Some(std::time::SystemTime::now());
        assert_eq!(header_facts(&entry)[1].0, "Created");
    }

    #[test]
    fn folder_preview_lists_folders_before_files() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("root");
        fs::create_dir_all(folder.join("zeta")).unwrap();
        fs::write(folder.join("alpha.txt"), b"x").unwrap();
        let entry = entry_for(&folder);
        match futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        )) {
            PreviewContent::Directory { names, .. } => {
                assert_eq!(names[0], ("zeta".to_string(), true));
                assert_eq!(names[1], ("alpha.txt".to_string(), false));
            }
            other => panic!("expected directory preview, got {other:?}"),
        }
    }

    #[test]
    fn archive_preview_lists_entries() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("pkg");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("readme.md"), b"hi").unwrap();
        let archive = dir.path().join("pkg.tar.gz");
        vfs_core::archive::run(&vfs_core::archive::compress_command(
            vfs_core::archive::CompressFormat::TarGz,
            &archive,
            &[std::ffi::OsString::from("pkg")],
        ))
        .unwrap();
        let entry = entry_for(&archive);
        match futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        )) {
            PreviewContent::Archive { entries, truncated } => {
                assert!(!truncated);
                assert!(entries.iter().any(|e| e.ends_with("readme.md")));
            }
            other => panic!("expected archive preview, got {other:?}"),
        }
    }

    #[test]
    fn corrupt_archive_falls_back_to_the_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.zip");
        fs::write(&path, [0u8, 1, 2, 3]).unwrap();
        let entry = entry_for(&path);
        let content = futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        ));
        assert!(
            matches!(content, PreviewContent::Binary { .. }),
            "{content:?}"
        );
    }

    #[test]
    fn binary_preview_includes_a_hex_dump() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.dat");
        fs::write(&path, [0u8, 1, 2, 0x41]).unwrap();
        let entry = entry_for(&path);
        match futures::executor::block_on(build_preview(
            &entry,
            None,
            &vfs_core::LocalProvider::new(),
        )) {
            PreviewContent::Binary { dump, .. } => {
                assert_eq!(dump.len(), 1);
                assert!(dump[0].contains("00 01 02 41"));
            }
            other => panic!("expected binary preview, got {other:?}"),
        }
    }

    #[test]
    fn hex_dump_formats_offset_hex_and_ascii_rows() {
        let rows = hex_dump(b"Hello, world!!!!XYZ", 64);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].starts_with("00000000  48 65 6c 6c 6f"));
        assert!(rows[0].ends_with("Hello, world!!!!"));
        assert!(rows[1].starts_with("00000010  58 59 5a"));
        assert!(rows[1].ends_with("XYZ"));
    }

    #[test]
    fn hex_dump_masks_unprintable_bytes_and_respects_the_cap() {
        let rows = hex_dump(&[0x00, 0x7f, b'a'], 64);
        assert!(rows[0].ends_with("..a"));
        assert_eq!(hex_dump(&[0u8; 100], 32).len(), 2);
        assert!(hex_dump(&[], 32).is_empty());
    }

    #[test]
    fn routing_prefers_direct_images_then_thumbnails_then_archives() {
        assert_eq!(route_for("a.png", "png", true), Route::Image);
        assert_eq!(route_for("a.svg", "svg", true), Route::Image);
        assert_eq!(route_for("a.pdf", "pdf", true), Route::Thumbnail);
        assert_eq!(route_for("a.mp4", "mp4", true), Route::Thumbnail);
        assert_eq!(route_for("a.zip", "zip", true), Route::Archive);
        assert_eq!(route_for("a.tar.gz", "gz", true), Route::Archive);
        assert_eq!(route_for("a.rs", "rs", true), Route::Text);
        assert_eq!(route_for("a.weird", "weird", true), Route::Sniff);
    }

    #[test]
    fn without_thumbnail_support_documents_fall_back_to_sniffing() {
        assert_eq!(route_for("a.pdf", "pdf", false), Route::Sniff);
        assert_eq!(route_for("a.docx", "docx", false), Route::Sniff);
        assert_eq!(route_for("a.png", "png", false), Route::Image);
    }

    #[test]
    fn office_documents_are_thumbnailed_not_listed_as_zips() {
        assert_eq!(route_for("report.docx", "docx", true), Route::Thumbnail);
    }

    #[test]
    fn archive_rows_indent_by_path_depth_and_flag_folders() {
        let rows = archive_rows(&[
            "pkg/".to_string(),
            "pkg/src/".to_string(),
            "pkg/src/main.rs".to_string(),
        ]);
        assert_eq!(rows[0], ("pkg".to_string(), true, 0));
        assert_eq!(rows[1], ("src".to_string(), true, 1));
        assert_eq!(rows[2], ("main.rs".to_string(), false, 2));
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
            PreviewContent::Binary { size: 3, .. }
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
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "svg"] {
            assert!(is_image_extension(ext), "{ext}");
        }
        for ext in ["pdf", "mp4", "rs"] {
            assert!(!is_image_extension(ext), "{ext}");
        }
    }

    #[test]
    fn sniff_window_fits_inside_the_head_read() {
        assert!(BINARY_SNIFF_BYTES <= PREVIEW_BYTES as usize);
    }
}

/// gpui draws no scrollbars, so a scrolling body gets a thin thumb overlay;
/// without it nothing hints that clipped lines can be scrolled sideways.
fn with_hbar(scroller: AnyElement, handle: &ScrollHandle) -> AnyElement {
    let handle = handle.clone();
    div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(scroller)
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let max = f32::from(handle.max_offset().width);
                    let view = f32::from(bounds.size.width);
                    if max <= 0.0 || view <= 0.0 {
                        return;
                    }
                    let thumb = (view * view / (view + max)).max(24.0).min(view);
                    let progress = (-f32::from(handle.offset().x) / max).clamp(0.0, 1.0);
                    let left = bounds.left() + px(progress * (view - thumb));
                    let top = bounds.bottom() - px(7.0);
                    let mut quad = fill(
                        Bounds::new(gpui::point(left, top), gpui::size(px(thumb), px(4.0))),
                        theme::text_faint(),
                    );
                    quad.corner_radii = Corners::all(px(2.0));
                    window.paint_quad(quad);
                },
            )
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .h(px(8.0)),
        )
        .into_any_element()
}
