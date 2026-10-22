//! Compressing and extracting archives by driving the system `tar` / `zip` /
//! `gzip` tools.
//!
//! macOS ships bsdtar (libarchive), which reads and writes zip, tar.*, 7z and
//! reads rar, so one tool covers the common formats. Everything that decides
//! *what* to run is a pure function returning a [`CommandSpec`], so it can be
//! tested without spawning; only [`run`] and [`list_entries`] touch processes.

use std::{
    ffi::OsString,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::ops::first_free;

/// What an existing archive file is, from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    SevenZip,
    Rar,
    /// A single compressed file, not a tar.
    Gz,
    Bz2,
    Xz,
    Zst,
}

/// Suffixes checked longest-first so `a.tar.gz` is not mistaken for `a.gz`.
const SUFFIXES: &[(&str, ArchiveKind)] = &[
    (".tar.gz", ArchiveKind::TarGz),
    (".tar.bz2", ArchiveKind::TarBz2),
    (".tar.xz", ArchiveKind::TarXz),
    (".tar.zst", ArchiveKind::TarZst),
    (".tgz", ArchiveKind::TarGz),
    (".tbz2", ArchiveKind::TarBz2),
    (".tbz", ArchiveKind::TarBz2),
    (".txz", ArchiveKind::TarXz),
    (".tzst", ArchiveKind::TarZst),
    (".tar", ArchiveKind::Tar),
    (".zip", ArchiveKind::Zip),
    // Zip containers that people still want to open as folders.
    (".jar", ArchiveKind::Zip),
    (".war", ArchiveKind::Zip),
    (".apk", ArchiveKind::Zip),
    (".ipa", ArchiveKind::Zip),
    (".whl", ArchiveKind::Zip),
    (".vsix", ArchiveKind::Zip),
    (".7z", ArchiveKind::SevenZip),
    (".rar", ArchiveKind::Rar),
    (".gz", ArchiveKind::Gz),
    (".bz2", ArchiveKind::Bz2),
    (".xz", ArchiveKind::Xz),
    (".zst", ArchiveKind::Zst),
];

impl ArchiveKind {
    /// `None` for anything that is not a recognised archive name.
    pub fn detect(name: &str) -> Option<Self> {
        let lower = name.to_lowercase();
        SUFFIXES
            .iter()
            .find(|(suffix, _)| lower.len() > suffix.len() && lower.ends_with(suffix))
            .map(|(_, kind)| *kind)
    }

    /// Single compressed files hold one file and cannot be listed by `tar`.
    pub fn is_single_file(self) -> bool {
        matches!(
            self,
            ArchiveKind::Gz | ArchiveKind::Bz2 | ArchiveKind::Xz | ArchiveKind::Zst
        )
    }
}

/// `a.tar.gz` -> `a`, `photos.zip` -> `photos`. Names that are not archives are
/// returned unchanged.
pub fn archive_stem(name: &str) -> String {
    let lower = name.to_lowercase();
    SUFFIXES
        .iter()
        .find(|(suffix, _)| lower.len() > suffix.len() && lower.ends_with(suffix))
        .map(|(suffix, _)| name[..name.len() - suffix.len()].to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Folder "Extract to" creates: `<stem>` next to the archive, numbered
/// (`<stem> (2)`) when it already exists so nothing is merged by accident.
pub fn extract_dir_path(archive: &Path) -> Option<PathBuf> {
    let parent = archive.parent()?;
    let stem = archive_stem(&archive.file_name()?.to_string_lossy());
    Some(first_free(parent, |index| match index {
        None => stem.clone(),
        Some(index) => format!("{stem} ({index})"),
    }))
}

/// Formats offered by "Compress to ...".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressFormat {
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    SevenZip,
}

impl CompressFormat {
    pub const ALL: [CompressFormat; 5] = [
        CompressFormat::Zip,
        CompressFormat::TarGz,
        CompressFormat::TarXz,
        CompressFormat::TarBz2,
        CompressFormat::SevenZip,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            CompressFormat::Zip => "zip",
            CompressFormat::TarGz => "tar.gz",
            CompressFormat::TarBz2 => "tar.bz2",
            CompressFormat::TarXz => "tar.xz",
            CompressFormat::SevenZip => "7z",
        }
    }

    /// Menu wording: "ZIP file", "TAR.GZ", ...
    pub fn label(self) -> String {
        match self {
            CompressFormat::Zip => "ZIP file".to_string(),
            other => other.extension().to_uppercase(),
        }
    }
}

/// Where compressing `sources` writes: `<first item's stem>.<ext>` next to the
/// first source, numbered (`notes (2).zip`) if the name is taken.
pub fn compress_archive_path(sources: &[PathBuf], format: CompressFormat) -> Option<PathBuf> {
    let first = sources.first()?;
    let parent = first.parent()?;
    let stem = if first.is_dir() {
        first.file_name()?.to_string_lossy().into_owned()
    } else {
        first
            .file_stem()
            .or_else(|| first.file_name())?
            .to_string_lossy()
            .into_owned()
    };
    let ext = format.extension();
    Some(first_free(parent, |index| match index {
        None => format!("{stem}.{ext}"),
        Some(index) => format!("{stem} ({index}).{ext}"),
    }))
}

/// A process to run: program, arguments and working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

fn spec(program: &str, args: Vec<OsString>, cwd: Option<PathBuf>) -> CommandSpec {
    CommandSpec {
        program: program.to_string(),
        args,
        cwd,
    }
}

fn os(value: &str) -> OsString {
    OsString::from(value)
}

/// Build the command that writes `archive` from `names` (paths relative to the
/// archive's folder, so the archive does not embed absolute paths).
pub fn compress_command(format: CompressFormat, archive: &Path, names: &[OsString]) -> CommandSpec {
    let cwd = archive.parent().map(Path::to_path_buf);
    let mut args: Vec<OsString> = match format {
        CompressFormat::Zip => vec![os("-r"), os("-q"), os("-y")],
        CompressFormat::TarGz => vec![os("-czf")],
        CompressFormat::TarBz2 => vec![os("-cjf")],
        CompressFormat::TarXz => vec![os("-cJf")],
        // bsdtar can write 7z; GNU tar cannot and reports an error we surface.
        CompressFormat::SevenZip => vec![os("--format"), os("7zip"), os("-cf")],
    };
    args.push(archive.as_os_str().to_os_string());
    args.extend(names.iter().cloned());
    let program = if format == CompressFormat::Zip {
        "zip"
    } else {
        "tar"
    };
    spec(program, args, cwd)
}

/// Build the command that extracts `archive` into `dest` (which must exist).
pub fn extract_command(kind: ArchiveKind, archive: &Path, dest: &Path) -> CommandSpec {
    if kind.is_single_file() {
        // gzip & co. only filter streams, so redirect into a file named after the
        // archive. Positional arguments keep odd file names out of the shell.
        let tool = match kind {
            ArchiveKind::Gz => "gzip -dc",
            ArchiveKind::Bz2 => "bzip2 -dc",
            ArchiveKind::Xz => "xz -dc",
            _ => "zstd -dc",
        };
        let stem = archive_stem(&archive.file_name().unwrap_or_default().to_string_lossy());
        let out = dest.join(stem);
        return spec(
            "sh",
            vec![
                os("-c"),
                OsString::from(format!("{tool} \"$1\" > \"$2\"")),
                os("sh"),
                archive.as_os_str().to_os_string(),
                out.into_os_string(),
            ],
            None,
        );
    }
    spec(
        "tar",
        vec![
            os("-xf"),
            archive.as_os_str().to_os_string(),
            os("-C"),
            dest.as_os_str().to_os_string(),
        ],
        None,
    )
}

/// Build the command that lists an archive's entries, one per line. Single-file
/// compressions have nothing to list.
pub fn list_command(kind: ArchiveKind, archive: &Path) -> Option<CommandSpec> {
    if kind.is_single_file() {
        return None;
    }
    Some(spec(
        "tar",
        vec![os("-tf"), archive.as_os_str().to_os_string()],
        None,
    ))
}

fn command(spec: &CommandSpec) -> Command {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    command
}

/// Run to completion; the error carries the tool's own message.
pub fn run(spec: &CommandSpec) -> Result<(), String> {
    let output = command(spec)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Cannot run {}: {error}", spec.program))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    Err(if detail.is_empty() {
        format!("{} failed ({})", spec.program, output.status)
    } else {
        format!("{} failed: {detail}", spec.program)
    })
}

/// First `limit` entries of an archive. The process is killed once enough lines
/// arrived, so listing a huge archive stays cheap.
pub fn list_entries(
    kind: ArchiveKind,
    archive: &Path,
    limit: usize,
) -> Result<Vec<String>, String> {
    let Some(spec) = list_command(kind, archive) else {
        return Err("This format holds a single file".to_string());
    };
    let mut child = command(&spec)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot run {}: {error}", spec.program))?;
    let Some(stdout) = child.stdout.take() else {
        return Err("No output from tar".to_string());
    };
    let lines: Vec<String> = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .take(limit)
        .collect();
    let _ = child.kill();
    let _ = child.wait();
    if lines.is_empty() {
        return Err("Cannot read this archive".to_string());
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_common_archive_names_case_insensitively() {
        let cases = [
            ("a.zip", ArchiveKind::Zip),
            ("A.ZIP", ArchiveKind::Zip),
            ("a.tar.gz", ArchiveKind::TarGz),
            ("a.tgz", ArchiveKind::TarGz),
            ("a.tar.bz2", ArchiveKind::TarBz2),
            ("a.tar.xz", ArchiveKind::TarXz),
            ("a.tar", ArchiveKind::Tar),
            ("a.7z", ArchiveKind::SevenZip),
            ("a.rar", ArchiveKind::Rar),
            ("a.gz", ArchiveKind::Gz),
            ("app.jar", ArchiveKind::Zip),
        ];
        for (name, kind) in cases {
            assert_eq!(ArchiveKind::detect(name), Some(kind), "{name}");
        }
    }

    #[test]
    fn tar_gz_is_not_mistaken_for_plain_gz() {
        assert_eq!(ArchiveKind::detect("x.tar.gz"), Some(ArchiveKind::TarGz));
        assert!(!ArchiveKind::TarGz.is_single_file());
        assert!(ArchiveKind::Gz.is_single_file());
    }

    #[test]
    fn non_archives_and_bare_suffixes_are_not_detected() {
        assert_eq!(ArchiveKind::detect("notes.txt"), None);
        assert_eq!(ArchiveKind::detect("zip"), None);
        assert_eq!(ArchiveKind::detect(".zip"), None, "a name, not an archive");
        assert_eq!(ArchiveKind::detect("archive"), None);
    }

    #[test]
    fn stem_strips_the_whole_archive_suffix() {
        assert_eq!(archive_stem("photos.tar.gz"), "photos");
        assert_eq!(archive_stem("photos.ZIP"), "photos");
        assert_eq!(archive_stem("v1.2.zip"), "v1.2");
        assert_eq!(archive_stem("plain.txt"), "plain.txt");
    }

    #[test]
    fn extract_dir_is_numbered_when_taken() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("pack.zip");
        assert_eq!(extract_dir_path(&archive), Some(dir.path().join("pack")));
        fs::create_dir(dir.path().join("pack")).unwrap();
        assert_eq!(
            extract_dir_path(&archive),
            Some(dir.path().join("pack (2)"))
        );
    }

    #[test]
    fn compress_path_uses_the_first_item_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.md");
        fs::write(&file, b"x").unwrap();
        let sources = vec![file];
        assert_eq!(
            compress_archive_path(&sources, CompressFormat::TarGz),
            Some(dir.path().join("notes.tar.gz"))
        );
        fs::write(dir.path().join("notes.tar.gz"), b"x").unwrap();
        assert_eq!(
            compress_archive_path(&sources, CompressFormat::TarGz),
            Some(dir.path().join("notes (2).tar.gz"))
        );
        assert_eq!(compress_archive_path(&[], CompressFormat::Zip), None);
    }

    #[test]
    fn compress_commands_pick_tool_and_flags_per_format() {
        let archive = Path::new("/p/out.tar.xz");
        let names = [OsString::from("a"), OsString::from("b c")];
        let xz = compress_command(CompressFormat::TarXz, archive, &names);
        assert_eq!(xz.program, "tar");
        assert_eq!(xz.cwd, Some(PathBuf::from("/p")));
        assert_eq!(
            xz.args,
            [os("-cJf"), os("/p/out.tar.xz"), os("a"), os("b c")]
        );
        assert_eq!(
            compress_command(CompressFormat::Zip, Path::new("/p/o.zip"), &names).program,
            "zip"
        );
        let seven = compress_command(CompressFormat::SevenZip, Path::new("/p/o.7z"), &names);
        assert_eq!(&seven.args[..3], [os("--format"), os("7zip"), os("-cf")]);
    }

    #[test]
    fn extract_uses_tar_for_containers_and_a_filter_for_single_files() {
        let tar = extract_command(ArchiveKind::Zip, Path::new("/p/a.zip"), Path::new("/d"));
        assert_eq!(tar.program, "tar");
        assert_eq!(tar.args, [os("-xf"), os("/p/a.zip"), os("-C"), os("/d")]);

        let gz = extract_command(ArchiveKind::Gz, Path::new("/p/log.txt.gz"), Path::new("/d"));
        assert_eq!(gz.program, "sh");
        assert_eq!(gz.args[2], os("sh"));
        assert_eq!(gz.args[3], os("/p/log.txt.gz"));
        assert_eq!(gz.args[4], os("/d/log.txt"), "output named after the stem");
    }

    #[test]
    fn single_file_compressions_cannot_be_listed() {
        assert!(list_command(ArchiveKind::Gz, Path::new("/p/a.gz")).is_none());
        assert!(list_command(ArchiveKind::Zip, Path::new("/p/a.zip")).is_some());
    }

    fn make_tree(root: &Path) -> PathBuf {
        let src = root.join("project");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"alpha").unwrap();
        fs::write(src.join("sub/b.txt"), b"beta").unwrap();
        src
    }

    #[test]
    fn tar_gz_round_trips_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let src = make_tree(dir.path());
        let sources = vec![src];
        let archive = compress_archive_path(&sources, CompressFormat::TarGz).unwrap();
        let names = [OsString::from("project")];
        run(&compress_command(CompressFormat::TarGz, &archive, &names)).unwrap();
        assert!(archive.is_file());

        let listing = list_entries(ArchiveKind::TarGz, &archive, 100).unwrap();
        assert!(listing.iter().any(|line| line.ends_with("a.txt")));

        let dest = extract_dir_path(&archive).unwrap();
        fs::create_dir(&dest).unwrap();
        run(&extract_command(ArchiveKind::TarGz, &archive, &dest)).unwrap();
        assert_eq!(
            fs::read(dest.join("project/sub/b.txt")).unwrap(),
            b"beta".to_vec()
        );
    }

    #[test]
    fn gzip_single_file_extracts_under_its_stem() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("log.txt");
        fs::write(&plain, b"hello").unwrap();
        run(&spec(
            "gzip",
            vec![os("-k"), plain.as_os_str().to_os_string()],
            None,
        ))
        .unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        run(&extract_command(
            ArchiveKind::Gz,
            &dir.path().join("log.txt.gz"),
            &out,
        ))
        .unwrap();
        assert_eq!(fs::read(out.join("log.txt")).unwrap(), b"hello".to_vec());
    }

    #[test]
    fn listing_stops_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("many");
        fs::create_dir(&src).unwrap();
        for index in 0..20 {
            fs::write(src.join(format!("f{index}.txt")), b"x").unwrap();
        }
        let archive = dir.path().join("many.tar.gz");
        run(&compress_command(
            CompressFormat::TarGz,
            &archive,
            &[OsString::from("many")],
        ))
        .unwrap();
        let lines = list_entries(ArchiveKind::TarGz, &archive, 5).unwrap();
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn running_a_failing_command_reports_the_tools_message() {
        let dir = tempfile::tempdir().unwrap();
        let error = run(&extract_command(
            ArchiveKind::TarGz,
            &dir.path().join("missing.tar.gz"),
            dir.path(),
        ))
        .unwrap_err();
        assert!(error.starts_with("tar failed"), "{error}");
    }

    #[test]
    fn listing_garbage_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.tar.gz");
        fs::write(&bad, b"definitely not an archive").unwrap();
        assert!(list_entries(ArchiveKind::TarGz, &bad, 10).is_err());
    }

    #[test]
    fn every_compress_format_has_an_extension_and_label() {
        for format in CompressFormat::ALL {
            assert!(!format.extension().is_empty());
            assert!(!format.label().is_empty());
        }
        assert_eq!(CompressFormat::Zip.label(), "ZIP file");
        assert_eq!(CompressFormat::TarGz.label(), "TAR.GZ");
    }
}
