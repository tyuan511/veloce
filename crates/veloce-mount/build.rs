// Link FUSE-T (or macFUSE, which exposes the same libfuse 2.9 interface) weakly: the
// app must still start on a Mac without either, and mounting then says "install
// FUSE-T" instead of the whole program failing to launch.
//
// When the library is not even present at build time (CI, a fresh checkout), the
// symbols are allowed to stay undefined instead, so the build still links; they are
// only ever called after `fuse_installed()` says the library exists.
const SYMBOLS: [&str; 9] = [
    "fuse_opt_add_arg",
    "fuse_opt_free_args",
    "fuse_mount",
    "fuse_unmount",
    "fuse_new",
    "fuse_loop",
    "fuse_exit",
    "fuse_destroy",
    "fuse_get_context",
];

fn main() {
    println!("cargo:rerun-if-env-changed=VELOCE_FUSE_LIB_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let dir = std::env::var("VELOCE_FUSE_LIB_DIR").unwrap_or_else(|_| "/usr/local/lib".into());
    if std::path::Path::new(&dir).join("libfuse-t.dylib").exists() {
        println!("cargo:rustc-link-arg=-L{dir}");
        println!("cargo:rustc-link-arg=-Wl,-weak-lfuse-t");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    } else {
        for symbol in SYMBOLS {
            println!("cargo:rustc-link-arg=-Wl,-U,_{symbol}");
        }
    }
}
