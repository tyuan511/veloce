//! The libfuse 2.9 high-level interface, which FUSE-T and macFUSE both provide. Only
//! the callbacks a read-only filesystem needs are filled in; the rest stay null and
//! libfuse answers them with "not supported".
//!
//! This is deliberately not the low-level interface: FUSE-T speaks NFS to the kernel
//! and only implements libfuse's own session loop, not the raw `/dev/fuse` protocol.

use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
pub struct FuseArgs {
    pub argc: c_int,
    pub argv: *mut *mut c_char,
    pub allocated: c_int,
}

#[repr(C)]
pub struct FuseContext {
    pub fuse: *mut c_void,
    pub uid: u32,
    pub gid: u32,
    pub pid: i32,
    pub private_data: *mut c_void,
    pub umask: u32,
}

pub type Filler = unsafe extern "C" fn(
    buf: *mut c_void,
    name: *const c_char,
    stat: *const libc::stat,
    offset: libc::off_t,
) -> c_int;

type Unused = Option<unsafe extern "C" fn()>;

/// `struct fuse_operations` up to and including `readdir`; the size passed to
/// `fuse_new` tells libfuse the remaining members are absent.
#[repr(C)]
pub struct Operations {
    pub getattr: Option<unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int>,
    pub readlink: Unused,
    pub getdir: Unused,
    pub mknod: Unused,
    pub mkdir: Unused,
    pub unlink: Unused,
    pub rmdir: Unused,
    pub symlink: Unused,
    pub rename: Unused,
    pub link: Unused,
    pub chmod: Unused,
    pub chown: Unused,
    pub truncate: Unused,
    pub utime: Unused,
    pub open: Option<unsafe extern "C" fn(*const c_char, *mut c_void) -> c_int>,
    pub read: Option<
        unsafe extern "C" fn(*const c_char, *mut c_char, usize, libc::off_t, *mut c_void) -> c_int,
    >,
    pub write: Unused,
    pub statfs: Unused,
    pub flush: Unused,
    pub release: Unused,
    pub fsync: Unused,
    pub setxattr: Unused,
    pub getxattr: Unused,
    pub listxattr: Unused,
    pub removexattr: Unused,
    pub opendir: Unused,
    pub readdir: Option<
        unsafe extern "C" fn(
            *const c_char,
            *mut c_void,
            Filler,
            libc::off_t,
            *mut c_void,
        ) -> c_int,
    >,
}

unsafe extern "C" {
    pub fn fuse_opt_add_arg(args: *mut FuseArgs, arg: *const c_char) -> c_int;
    pub fn fuse_opt_free_args(args: *mut FuseArgs);
    pub fn fuse_mount(mountpoint: *const c_char, args: *mut FuseArgs) -> *mut c_void;
    pub fn fuse_unmount(mountpoint: *const c_char, channel: *mut c_void);
    pub fn fuse_new(
        channel: *mut c_void,
        args: *mut FuseArgs,
        operations: *const Operations,
        operations_size: usize,
        private_data: *mut c_void,
    ) -> *mut c_void;
    pub fn fuse_loop(fuse: *mut c_void) -> c_int;
    pub fn fuse_exit(fuse: *mut c_void);
    pub fn fuse_destroy(fuse: *mut c_void);
    pub fn fuse_get_context() -> *mut FuseContext;
}
