//! Hand test for the FUSE glue: mount a local folder through the same code path a
//! remote connection uses. `cargo run -p veloce-mount --example mount_dir -- <src> <mountpoint>`
use std::sync::Arc;

use vfs_core::{LocalProvider, VfsUri};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(source), Some(mountpoint)) = (args.next(), args.next()) else {
        eprintln!("usage: mount_dir <source dir> <mount point>");
        return;
    };
    let mounted = veloce_mount::mount(
        Arc::new(LocalProvider::new()),
        VfsUri::local(&source),
        std::path::Path::new(&mountpoint),
        "veloce-test",
    )
    .expect("mount");
    println!("mounted at {}", mounted.mountpoint().display());
    std::thread::sleep(std::time::Duration::from_secs(12));
}
