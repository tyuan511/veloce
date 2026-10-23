# Veloce

[![CI](https://github.com/tyuan511/veloce/actions/workflows/ci.yml/badge.svg)](https://github.com/tyuan511/veloce/actions/workflows/ci.yml)

A GPU-accelerated, keyboard-first file manager for people who live in terminals but
still need to drag a file into Slack. Local disk, SSH/SFTP, S3/OSS and WebDAV behind
one interface — built on Rust + [GPUI](https://gpui.rs) (the engine behind Zed), so
the list keeps 120 FPS in a directory with 200,000 entries.

> Status: the local core is a daily driver, and SFTP, S3 and WebDAV connections are
> wired in for browsing, previewing, transferring and (optionally) mounting as local
> folders. macOS only for now. See [Remote connections](#remote-connections).

## Why

| | Finder / Explorer | FileZilla / Cyberduck | Yazi | **Veloce** |
|---|---|---|---|---|
| 100k-file directory | spins, beachballs | unusable | fast | **streams in, first screen < 10 ms** |
| keyboard flow | weak | none | excellent | **Vim keys + command palette** |
| cross-protocol move | — | download→upload | — | **streamed through memory, no temp file** |
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
    ops.rs           TaskManager: background copy/move/trash/transfer with progress
    remote.rs        RemoteProvider: SFTP, S3, WebDAV through OpenDAL, one per connection
    transfer.rs      copy/move between any two providers (local <-> remote <-> remote)
    mountfs.rs       the FUSE-independent half of mounting (paths, attrs, read clamping)
  veloce-mount/      read-only FUSE mount of a connection, over libfuse 2.9 (FUSE-T / macFUSE)
  store/             veloce-store: SQLite state (settings, folder views, session, favorites,
                     saved connections, thumbnail cache), no GUI dependency
  veloce/            GPUI front-end
    listing.rs       pure list logic: sort/filter/cursor/selection/path parsing
    workspace.rs     sidebar + two panes + inspector + task panel + palette, all actions
    pane.rs          one pane: tabs, virtual list, path bar, file ops
    preview.rs       Space-to-preview: bounded head read, images, folders
    palette.rs       ⌘P fuzzy jump / ⌘⇧P commands
    tasks_panel.rs   transfer progress, cancellation
    connections.rs   connection forms: validation, what is stored, how it is opened
    secrets.rs       passwords and secret keys in the macOS keychain
    mounts.rs        which connections are FUSE-mounted, and where
    text_input.rs    IME-correct single-line field (EntityInputHandler)
    theme.rs         light/dark palettes following the OS, one accent
```

The split is deliberate: `vfs-core` never imports `gpui`, so the same core can back a
CLI or a daemon later, and every non-visual behaviour is testable with `cargo test`.

### Why local files do not go through OpenDAL

OpenDAL's `fs` backend is fine for remote parity, but local files need things a generic
abstraction hides: the OS trash, extended attributes / Finder tags, native permission
bits, and the fastest possible `readdir`. So `LocalProvider` implements `VfsProvider`
directly, and OpenDAL backs only the remote schemes.

## Build

Requires macOS, Rust 1.90+ (stable), and the Xcode command line tools. FUSE-T or macFUSE
is optional and only needed to mount remote connections as folders.

```sh
cargo dev [dir]                    # debug build + launch the GUI
cargo build --release
./target/release/veloce ~/code     # opens both panes in that directory
```

GPUI normally compiles its Metal shaders at build time, which needs the full Metal
toolchain (`xcodebuild -downloadComponent MetalToolchain`). This crate enables the
`runtime_shaders` feature instead, so a plain Xcode CLT install is enough — the shaders
compile once at first launch.

## Remote connections

*Settings → Connections* saves SFTP, S3 (and compatible: R2, MinIO, OSS) and WebDAV
connections. They show up in the sidebar and open in the focused pane like any folder.

- **Secrets stay out of the database.** S3 secret keys and WebDAV passwords go to the
  macOS keychain; only non-secret fields are stored. SFTP uses the system `ssh`, so it
  authenticates with keys and the agent (no passwords) and trusts a host on first use.
- **What works remotely:** browsing, preview (a bounded head of the file, or one download
  rendered to a small picture, cached), open / open with (downloads first), rename, new
  folder / file, delete, and copy / move between the two panes (`F5` / `F6`), including
  local ↔ remote and remote ↔ remote, streamed with progress in the task panel.
- **What does not:** cut / copy / paste through the clipboard, shortcuts, archives and
  Open in Terminal on remote items; those rows are left out of the menu.
- **Remote delete is permanent.** A server has no Trash, so this is the one place
  deletion is not recoverable (local items always go to the Trash).
- **Remote thumbnails are off.** A folder of remote photos would mean downloading every
  one of them; a picture is only rendered when you preview a file by hand.

### Mount as a local folder

Tick *Mount as a local folder* on a connection and it is mounted read-only at
`~/Veloce/<name>` (visible in Finder) when the app starts and when the connection is
saved, and unmounted when you untick it, delete or edit the connection, or quit. This
needs [FUSE-T](https://www.fuse-t.org) (or macFUSE); without either the app runs as
usual and tells you to install one.

The mount talks to libfuse 2.9's high-level API directly instead of using the `fuser`
crate: FUSE-T serves NFS to the kernel and has no `/dev/fuse` protocol, so `fuser`
reports success while nothing is mounted. Listings and attributes are cached for five
seconds; reads fetch only the requested byte range.

## Keys

| Key | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | move cursor |
| `h` / `l`, `←` / `→`, `enter` | parent / open |
| `⌃D` / `⌃U` | page down / up |
| `gg` / `G` | first / last |
| `shift-j` | extend selection |
| `⌘A` / `esc` | select all / clear |
| `space` | show / hide the preview panel on the right (follows the cursor; its Open button picks an application) |
| `⇧⌘.` | show / hide dot-files (hidden by default, per pane) |
| `⌘1` / `⌘2` / `⌘3` | view as icons / list / columns |
| `⌥⌘I` | toggle the preview panel (same as `space`) |
| `⌃1` / `⌃2` | focus left / right pane |
| `/` | filter this folder (fuzzy, live) |
| `⌘P` | search box: fuzzy jump to any file in the tree, **or type a path** (`~/code`, `/usr/lo`, `../x`) to go there; `tab` completes, `enter` goes |
| `⌘⇧P` | command palette |
| `⌘\` | show / hide the second pane (single pane by default) |
| `tab` | switch pane (when split) |
| `F5` / `F6` | copy / move to the other pane |
| `⌘⌫` | move to trash (recoverable) |
| `⌘⇧N` / `shift-R` | new folder / rename |
| `⌘X` / `⌘C` / `⌘V` | cut / copy / paste, Explorer-style: cut + paste moves, copy + paste in the same folder makes `name - Copy`; `esc` cancels a cut |
| `Del` / `⌘⌫` | delete (local: to the trash; remote: permanent) |
| `F2` / `⌥↵` / `⌘O` | rename / properties / open |
| `⇧⌘C` | copy as path |
| right-click | Explorer-style menu: open, compress to ZIP, cut, copy, paste, create shortcut, delete, rename, properties |
| `⌘T` / `⌘W` | new / close tab |
| `⌘J` | task panel |
| `s` | cycle sort column (toolbar ⇅ menu: name, kind, date modified, date created, size; ascending/descending; folders on top) |
| `⌘R` | refresh |

## Saved state

Veloce keeps what you would miss in one SQLite file
(`~/Library/Application Support/Veloce/veloce.db`): window layout and toggles, the
view and sort of each folder (like Finder, an unvisited folder opens as a list),
the open tabs of both panes, the files and folders you starred (right-click → *Add to Favorites*, shown in the
sidebar), saved connections and generated thumbnails. Everything is loaded before the window opens;
writes happen on a background thread, so the UI never waits on disk. If the file
cannot be opened or is damaged it is moved aside and the app carries on without
remembering.

The icon view shows real previews: pictures through `sips`, everything else
(PDF, video, documents, web pages) through Quick Look. The preview panel's larger
renders of those files come from the same code. Both sizes are cached in that
database, told apart by pixel size in the key, and pruned least-recently-used past
256 MB. Nothing is cached as loose files; the only temporary files are the
short-lived work directories used while a thumbnail is being rendered.

## Settings

`⌘,` (or *Settings…* in the toolbar's "…" menu) opens a separate Settings window
with General, Appearance, Files, Preview, Search, Shortcuts, Connections, Storage
and About. Changes apply immediately in every window and are saved to the same
database. *Open in Terminal* offers the terminals found on this Mac, with their icons.
Shortcuts are editable: click **+** on a command and press the new keys (a key
already used elsewhere, or Esc/⌘Q/⌘,, is refused); changes apply at once and are
stored as overrides, so new defaults in later versions still reach you. Context-menu
hints still show the default keys. Connections (SSH, S3, WebDAV) are managed there too: the form validates
and stores the non-secret fields, and passwords and secret keys go to the system
keychain.

## Layout

Drag the dividers to resize the sidebar, the preview panel and, when split, the two panes.

## Design decisions worth knowing

- **Streaming, not collecting.** `list_stream` yields 512-entry chunks; the first chunk
  paints and the rest append. Nothing in the UI path waits for a full `readdir`.
- **Fixed row height + `uniform_list`.** Virtualization is arithmetic, so cost per frame
  is independent of directory size.
- **Bounded preview.** Text previews read 64 KiB and render at most 400 lines: a 10 GB
  log opens instantly instead of freezing the window. Markdown renders natively (headings, lists, tasks, quotes, tables, highlighted code blocks); source files are highlighted with tree-sitter (the engine Zed uses: Rust, JS/TS/TSX, Python, Go, JSON, TOML, YAML, Kotlin, Bash, C/C++/C#, Java, HTML, CSS, Ruby, Lua) with syntect covering the rest; JSON is pretty-printed; CSV/TSV become tables; HTML renders through a Quick Look snapshot on macOS. Archives list their first 500
  entries, binaries show a 512-byte hex dump, and on macOS PDFs, video, audio artwork,
  Office/iWork documents, HEIC, RAW, fonts and 3D models render through Quick Look
  thumbnails (cached, with a timeout) — all off the UI thread.
- **Archives.** Right-click compresses to ZIP, TAR.GZ, TAR.XZ, TAR.BZ2 and 7Z, and
  extracts zip / tar.* / 7z / rar / gz / bz2 / xz / zst with the system `tar`, `zip`
  and `gzip` (here or into a new `name/` folder, never merging into an existing one).
- **Trash, never `rm`.** Deleting a local item uses the `trash` crate; there is no
  hard-delete path for local files. The single exception is items on a remote
  connection, which have no Trash (`Pane::delete_remote`, see `AGENTS.md`).
- **Conflicts default to "keep both".** Copies rename to `name 2.ext` rather than
  silently clobbering a file you already had.
- **The watcher is debounced.** A `git checkout` emits thousands of FSEvents; they
  coalesce into one reload 250 ms later.

## Tests

```sh
cargo test              # no GPU, window server or network needed
cargo test -p vfs-core  # the core: providers, transfers, search, mount logic
```

**UI is not covered by automated tests** (see `AGENTS.md`): interaction is verified
by hand. What *is* tested is every decision the UI makes — `listing.rs` holds the
sort / filter / cursor / selection / path-parsing logic as pure functions, so the
list's behaviour is covered without a single `TestAppContext`.

Remote backends are tested against OpenDAL's in-memory service, and the FUSE glue by
hand (`cargo run -p veloce-mount --example mount_dir -- <dir> <mountpoint>` mounts a
local folder through the same code path).

Coverage includes path normalization, natural sorting, chunked listing, range reads,
copy/move/trash semantics, conflict policies, cancellation, `.gitignore`-aware search,
watcher coalescing, preview classification, cursor clamping, tab-index arithmetic,
and the theme's contrast and layout invariants (asserted at compile time).

## Roadmap

- Remote: write support for mounts, `~/.ssh/config` parsing, resumable transfers,
  clipboard copy / paste across backends.
- Multi-rename with regex preview, directory diff & sync, embedded SSH terminal.

See `docs/research.md` for the full product and engineering analysis.

## Credits

UI glyphs are from [Lucide](https://lucide.dev) (ISC) and file/folder icons from
[Material Icon Theme](https://github.com/material-extensions/vscode-material-icon-theme)
(MIT); licenses sit next to the assets. Regenerate the file-icon tables with
`scripts/gen-file-icons.py`.

Source highlighting uses [tree-sitter](https://tree-sitter.github.io) and its grammar
crates (MIT). The Kotlin highlight query is vendored from
[fwcd/tree-sitter-kotlin](https://github.com/fwcd/tree-sitter-kotlin), itself derived
from nvim-treesitter (Apache-2.0); see `crates/veloce/assets/queries/`.
