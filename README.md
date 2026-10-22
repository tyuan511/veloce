# Veloce

A GPU-accelerated, keyboard-first file manager for people who live in terminals but
still need to drag a file into Slack. Local disk, SSH/SFTP, S3/OSS and WebDAV behind
one interface — built on Rust + [GPUI](https://gpui.rs) (the engine behind Zed), so
the list keeps 120 FPS in a directory with 200,000 entries.

> Status: **Milestone 1** — the local core is complete and usable as a daily driver.
> Remote backends are stubbed at the trait level (see `docs/research.md` §7).

## Why

| | Finder / Explorer | FileZilla / Cyberduck | Yazi | **Veloce** |
|---|---|---|---|---|
| 100k-file directory | spins, beachballs | unusable | fast | **streams in, first screen < 10 ms** |
| keyboard flow | weak | none | excellent | **Vim keys + command palette** |
| cross-protocol move | — | download→upload | — | **in-memory pipe (M3)** |
| local trash / OS integration | native | none | none | **native trash, `open`, clipboard** |
| UI stack | native | wxWidgets / Java | TUI | **GPUI, GPU, no WebView** |

## Architecture

```
crates/
  vfs-core/          no GUI dependency, fully unit-tested
    uri.rs           VfsUri + ProtocolScheme — one id for every backend
    entry.rs         FileEntry, natural sort, size/time formatting
    provider.rs      VfsProvider async trait (streaming list/read/write)
    local.rs         native local provider (chunked listing, 1 MiB copy loops)
    filter.rs        nucleo-backed in-folder fuzzy filter
    search.rs        parallel recursive search (ignore + nucleo)
    watch.rs         debounced filesystem watcher
    ops.rs           TaskManager: background copy/move/trash with progress
  veloce/            GPUI front-end
    listing.rs       pure list logic: sort/filter/cursor/selection/path parsing
    workspace.rs     sidebar + two panes + inspector + task panel + palette, all actions
    pane.rs          one pane: tabs, virtual list, path bar, file ops
    preview.rs       Space-to-preview: bounded head read, images, folders
    palette.rs       ⌘P fuzzy jump / ⌘⇧P commands
    tasks_panel.rs   transfer progress, cancellation
    text_input.rs    IME-correct single-line field (EntityInputHandler)
    theme.rs         light/dark palettes following the OS, one accent
```

The split is deliberate: `vfs-core` never imports `gpui`, so the same core can back a
CLI or a daemon later, and every non-visual behaviour is testable with `cargo test`.

### Why local files do not go through OpenDAL

OpenDAL's `fs` backend is fine for remote parity, but local files need things a generic
abstraction hides: the OS trash, extended attributes / Finder tags, native permission
bits, and the fastest possible `readdir`. So `LocalProvider` implements `VfsProvider`
directly, and OpenDAL will back only the remote schemes (milestone 3).

## Build

Requires macOS or Linux, Rust 1.90+, and Xcode command line tools.

```sh
cargo dev [dir]                    # debug build + launch the GUI
cargo build --release
./target/release/veloce ~/code     # opens both panes in that directory
```

GPUI normally compiles its Metal shaders at build time, which needs the full Metal
toolchain (`xcodebuild -downloadComponent MetalToolchain`). This crate enables the
`runtime_shaders` feature instead, so a plain Xcode CLT install is enough — the shaders
compile once at first launch.

## Keys

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | move cursor |
| `h` / `l`, `←` / `→`, `enter` | parent / open |
| `⌃D` / `⌃U` | page down / up |
| `gg` / `G` | first / last |
| `shift-j` | extend selection |
| `⌘A` / `esc` | select all / clear |
| `space` | Quick Look: large preview of the selected file (`space` / `esc` to close, arrows keep moving) |
| `⇧⌘.` | show / hide dot-files (hidden by default, per pane) |
| `⌘1` / `⌘2` / `⌘3` | view as icons / list / columns |
| `⌥⌘I` | toggle the inspector panel |
| `⌃1` / `⌃2` | focus left / right pane |
| `/` | filter this folder (fuzzy, live) |
| `⌘P` | fuzzy jump to any file in the tree |
| `⌘⇧P` | command palette |
| `⌘\` | show / hide the second pane (single pane by default) |
| `tab` | switch pane (when split) |
| `F5` / `F6` | copy / move to the other pane |
| `⌘⌫` | move to trash (recoverable) |
| `⌘⇧N` / `shift-R` | new folder / rename |
| `⌘X` / `⌘C` / `⌘V` | cut / copy / paste, Explorer-style: cut + paste moves, copy + paste in the same folder makes `name - Copy`; `esc` cancels a cut |
| `Del` / `⌘⌫` | delete (to the trash) |
| `F2` / `⌥↵` / `⌘O` | rename / properties / open |
| `⇧⌘C` | copy as path |
| right-click | Explorer-style menu: open, compress to ZIP, cut, copy, paste, create shortcut, delete, rename, properties |
| `⌘T` / `⌘W` | new / close tab |
| `⌘J` | task panel |
| `s` | cycle sort column (toolbar ⇅ menu: name, kind, date modified, date created, size; ascending/descending; folders on top) |
| `⌘R` | refresh |

## Design decisions worth knowing

- **Streaming, not collecting.** `list_stream` yields 512-entry chunks; the first chunk
  paints and the rest append. Nothing in the UI path waits for a full `readdir`.
- **Fixed row height + `uniform_list`.** Virtualization is arithmetic, so cost per frame
  is independent of directory size.
- **Bounded preview.** Text previews read 64 KiB and render at most 400 lines: a 10 GB
  log opens instantly instead of freezing the window. Markdown renders natively (headings, lists, tasks, quotes, tables, highlighted code blocks); source files are syntax-highlighted (syntect); JSON is pretty-printed; CSV/TSV become tables; HTML renders through a Quick Look snapshot on macOS. Archives list their first 500
  entries, binaries show a 512-byte hex dump, and on macOS PDFs, video, audio artwork,
  Office/iWork documents, HEIC, RAW, fonts and 3D models render through Quick Look
  thumbnails (cached, with a timeout) — all off the UI thread.
- **Archives.** Right-click compresses to ZIP, TAR.GZ, TAR.XZ, TAR.BZ2 and 7Z, and
  extracts zip / tar.* / 7z / rar / gz / bz2 / xz / zst with the system `tar`, `zip`
  and `gzip` (here or into a new `name/` folder, never merging into an existing one).
- **Trash, never `rm`.** Deleting uses the `trash` crate; there is no hard-delete path in
  the UI at all.
- **Conflicts default to "keep both".** Copies rename to `name 2.ext` rather than
  silently clobbering a file you already had.
- **The watcher is debounced.** A `git checkout` emits thousands of FSEvents; they
  coalesce into one reload 250 ms later.

## Tests

```sh
cargo test              # 358 tests, no GPU or window server needed
cargo test -p vfs-core  # 156 core tests
```

**UI is not covered by automated tests** (see `AGENTS.md`): interaction is verified
by hand. What *is* tested is every decision the UI makes — `listing.rs` holds the
sort / filter / cursor / selection / path-parsing logic as pure functions, so the
list's behaviour is covered without a single `TestAppContext`.

Coverage includes path normalization, natural sorting, chunked listing, range reads,
copy/move/trash semantics, conflict policies, cancellation, `.gitignore`-aware search,
watcher coalescing, preview classification, cursor clamping, tab-index arithmetic,
and the theme's contrast and layout invariants (asserted at compile time).

## Roadmap

- **M2** — syntax highlighting in the preview (`tree-sitter`), directory size rollups,
  open-in-terminal / open-in-editor.
- **M3** — OpenDAL backends (S3/OSS/R2/MinIO, SFTP, WebDAV), `~/.ssh/config` parsing,
  `keyring` credential storage, in-memory cross-protocol pipe.
- **M4** — multi-rename with regex preview, directory diff & sync, embedded SSH terminal.

See `docs/research.md` for the full product and engineering analysis.

## Credits

UI glyphs are from [Lucide](https://lucide.dev) (ISC) and file/folder icons from
[Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme)
(MIT); licenses sit next to the assets. Regenerate the file-icon tables with
`scripts/gen-file-icons.py`.
