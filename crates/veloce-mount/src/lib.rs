//! Mount a remote connection as a read-only folder on this Mac through FUSE.
//!
//! Everything that can be decided without the kernel (paths, attributes, caching,
//! read clamping) lives in `vfs_core::mountfs` and is tested there; this crate turns
//! those decisions into libfuse callbacks. It needs FUSE-T or macFUSE installed, and
//! says so when they are not.

mod ffi;
mod fs;

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    os::unix::fs::MetadataExt,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, mpsc},
    time::UNIX_EPOCH,
};

use fs::RemoteFs;
use vfs_core::{
    VfsProvider, VfsUri,
    mountfs::{Node, NodeKind},
};

/// A live mount. Dropping it unmounts the folder.
pub struct Mounted {
    mountpoint: PathBuf,
    fuse: usize,
}

impl Mounted {
    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        // Unmount first, while the session loop is still answering the kernel: the
        // unmount itself talks to this process, so stopping the loop beforehand makes
        // it wait forever.
        if !umount(&self.mountpoint, false) {
            umount(&self.mountpoint, true);
        }
        unsafe { ffi::fuse_exit(self.fuse as *mut c_void) };
    }
}

/// Run `umount` with a time limit, so a stuck mount cannot freeze the app.
fn umount(mountpoint: &Path, force: bool) -> bool {
    let mut command = Command::new("/sbin/umount");
    if force {
        command.arg("-f");
    }
    let Ok(mut child) = command
        .arg(mountpoint)
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Where the FUSE library has to be for mounting to work: FUSE-T or macFUSE.
const LIBRARIES: [&str; 2] = [
    "/usr/local/lib/libfuse-t.dylib",
    "/usr/local/lib/libfuse.2.dylib",
];

/// Whether a FUSE implementation is installed. Checked before touching libfuse,
/// because it is linked weakly and calling into a missing one would crash.
pub fn fuse_installed() -> bool {
    LIBRARIES.iter().any(|path| Path::new(path).exists())
}

// ---- callbacks ----------------------------------------------------------------------

fn state<'a>() -> &'a RemoteFs {
    // SAFETY: `private_data` is the `RemoteFs` handed to `fuse_new`, which outlives the
    // session loop that makes these calls.
    unsafe { &*((*ffi::fuse_get_context()).private_data as *const RemoteFs) }
}

fn path_arg<'a>(path: *const c_char) -> Option<&'a str> {
    // SAFETY: libfuse passes a NUL-terminated path.
    unsafe { CStr::from_ptr(path) }.to_str().ok()
}

/// A panic must never unwind into libfuse's C frames.
fn guarded(call: impl FnOnce() -> c_int) -> c_int {
    catch_unwind(AssertUnwindSafe(call)).unwrap_or(-libc::EIO)
}

fn fill_stat(out: *mut libc::stat, node: &Node, owner: (u32, u32)) {
    // SAFETY: an all-zero `stat` is a valid value.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let kind = match node.kind {
        NodeKind::Dir => libc::S_IFDIR,
        NodeKind::File => libc::S_IFREG,
    };
    stat.st_mode = kind | node.perm;
    stat.st_nlink = if node.kind == NodeKind::Dir { 2 } else { 1 };
    stat.st_size = node.size as libc::off_t;
    stat.st_blocks = node.blocks() as libc::blkcnt_t;
    stat.st_blksize = 4096;
    stat.st_uid = owner.0;
    stat.st_gid = owner.1;
    let seconds = node
        .mtime
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as libc::time_t);
    stat.st_mtime = seconds;
    stat.st_atime = seconds;
    stat.st_ctime = seconds;
    // SAFETY: `out` is the buffer libfuse gave us for this call.
    unsafe { *out = stat };
}

unsafe extern "C" fn cb_getattr(path: *const c_char, out: *mut libc::stat) -> c_int {
    guarded(|| {
        let Some(path) = path_arg(path) else {
            return -libc::ENOENT;
        };
        match state().node(path) {
            Ok(node) => {
                fill_stat(out, &node, state().owner);
                0
            }
            Err(errno) => -errno,
        }
    })
}

unsafe extern "C" fn cb_readdir(
    path: *const c_char,
    buf: *mut c_void,
    filler: ffi::Filler,
    _offset: libc::off_t,
    _info: *mut c_void,
) -> c_int {
    guarded(|| {
        let Some(path) = path_arg(path) else {
            return -libc::ENOENT;
        };
        let entries = match state().listing(path) {
            Ok(entries) => entries,
            Err(errno) => return -errno,
        };
        let names = [".", ".."].map(str::to_string).into_iter().chain(
            vfs_core::mountfs::dir_names(&entries)
                .into_iter()
                .map(|(name, _)| name),
        );
        for name in names {
            let Ok(name) = CString::new(name) else {
                continue;
            };
            // Offset 0 for every entry: the whole listing goes out in one call and
            // the filler never reports "full".
            // SAFETY: `filler` and `buf` come from libfuse for this call.
            unsafe { filler(buf, name.as_ptr(), std::ptr::null(), 0) };
        }
        0
    })
}

unsafe extern "C" fn cb_open(path: *const c_char, _info: *mut c_void) -> c_int {
    guarded(|| {
        let Some(path) = path_arg(path) else {
            return -libc::ENOENT;
        };
        match state().node(path) {
            Ok(node) if node.kind == NodeKind::File => 0,
            Ok(_) => -libc::EISDIR,
            Err(errno) => -errno,
        }
    })
}

unsafe extern "C" fn cb_read(
    path: *const c_char,
    buf: *mut c_char,
    size: usize,
    offset: libc::off_t,
    _info: *mut c_void,
) -> c_int {
    guarded(|| {
        let Some(path) = path_arg(path) else {
            return -libc::ENOENT;
        };
        let size = u32::try_from(size).unwrap_or(u32::MAX);
        match state().read(path, offset.max(0) as u64, size) {
            Ok(bytes) => {
                // SAFETY: libfuse's buffer holds at least `size` bytes, and `bytes`
                // is never longer than that.
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), bytes.len()) };
                bytes.len() as c_int
            }
            Err(errno) => -errno,
        }
    })
}

// ---- mounting -----------------------------------------------------------------------

/// Mount `provider`'s tree (rooted at `root`) on `mountpoint`, which is created if
/// needed. `volume_name` is what Finder shows.
pub fn mount(
    provider: Arc<dyn VfsProvider>,
    root: VfsUri,
    mountpoint: &Path,
    volume_name: &str,
) -> Result<Mounted, String> {
    if !fuse_installed() {
        return Err("FUSE-T is not installed (https://www.fuse-t.org)".to_string());
    }
    std::fs::create_dir_all(mountpoint)
        .map_err(|error| format!("{}: {error}", mountpoint.display()))?;
    // A crash leaves the previous mount behind, which makes this folder hang. Nothing
    // else mounts here, so clear it first; failing because nothing is mounted is fine.
    // Forced straight away: a plain unmount of a dead server just waits for it.
    umount(mountpoint, true);
    let owner = std::fs::metadata(mountpoint)
        .map(|meta| (meta.uid(), meta.gid()))
        .map_err(|error| format!("{}: {error}", mountpoint.display()))?;
    let point = CString::new(mountpoint.as_os_str().as_encoded_bytes())
        .map_err(|_| "the mount point contains a NUL byte".to_string())?;
    // `ro`: nothing here can write. `volname` is the name Finder shows;
    // `noappledouble` stops macOS inventing `._*` metadata files.
    let volume = volume_name.replace([',', '\0'], " ");
    let options = format!("ro,volname={volume},fsname=veloce:{volume},noappledouble");

    let filesystem = Box::new(RemoteFs::new(provider, root, owner));
    let (ready, started) = mpsc::channel::<Result<usize, String>>();
    std::thread::Builder::new()
        .name("veloce-fuse".into())
        .spawn(move || run_session(filesystem, point, options, ready))
        .map_err(|error| error.to_string())?;
    match started.recv() {
        Ok(Ok(fuse)) => Ok(Mounted {
            mountpoint: mountpoint.to_path_buf(),
            fuse,
        }),
        Ok(Err(message)) => Err(message),
        Err(_) => Err("the mount thread stopped before it finished starting".to_string()),
    }
}

/// Own the whole life of one mount on its own thread: mount, serve until unmounted,
/// then tear down.
fn run_session(
    filesystem: Box<RemoteFs>,
    point: CString,
    options: String,
    ready: mpsc::Sender<Result<usize, String>>,
) {
    let private = Box::into_raw(filesystem);
    let operations = ffi::Operations {
        getattr: Some(cb_getattr),
        readlink: None,
        getdir: None,
        mknod: None,
        mkdir: None,
        unlink: None,
        rmdir: None,
        symlink: None,
        rename: None,
        link: None,
        chmod: None,
        chown: None,
        truncate: None,
        utime: None,
        open: Some(cb_open),
        read: Some(cb_read),
        write: None,
        statfs: None,
        flush: None,
        release: None,
        fsync: None,
        setxattr: None,
        getxattr: None,
        listxattr: None,
        removexattr: None,
        opendir: None,
        readdir: Some(cb_readdir),
    };
    // SAFETY: plain libfuse calls in the documented order; every pointer passed is
    // alive for the call, and `private` is reclaimed exactly once at the end.
    unsafe {
        let mut args = ffi::FuseArgs {
            argc: 0,
            argv: std::ptr::null_mut(),
            allocated: 0,
        };
        for arg in ["veloce", "-o", options.as_str()] {
            let arg = CString::new(arg).unwrap_or_default();
            ffi::fuse_opt_add_arg(&mut args, arg.as_ptr());
        }
        let channel = ffi::fuse_mount(point.as_ptr(), &mut args);
        if channel.is_null() {
            ffi::fuse_opt_free_args(&mut args);
            drop(Box::from_raw(private));
            let _ = ready.send(Err("the FUSE mount was refused".to_string()));
            return;
        }
        let fuse = ffi::fuse_new(
            channel,
            &mut args,
            &operations,
            std::mem::size_of::<ffi::Operations>(),
            private.cast::<c_void>(),
        );
        if fuse.is_null() {
            ffi::fuse_unmount(point.as_ptr(), channel);
            ffi::fuse_opt_free_args(&mut args);
            drop(Box::from_raw(private));
            let _ = ready.send(Err("FUSE could not create the filesystem".to_string()));
            return;
        }
        let _ = ready.send(Ok(fuse as usize));
        ffi::fuse_loop(fuse);
        ffi::fuse_unmount(point.as_ptr(), channel);
        ffi::fuse_destroy(fuse);
        ffi::fuse_opt_free_args(&mut args);
        drop(Box::from_raw(private));
    }
}
