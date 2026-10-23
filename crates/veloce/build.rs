// The final binary has to carry the same link flags as veloce-mount, because link
// arguments do not travel through dependencies; see that crate's build.rs for why the
// FUSE library is linked weakly and what happens when it is absent.
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
