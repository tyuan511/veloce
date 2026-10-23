// The FUSE library is linked weakly (see veloce-mount/build.rs); the final binary has
// to carry the same flags, because link arguments do not travel through dependencies.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-L/usr/local/lib");
        println!("cargo:rustc-link-arg=-Wl,-weak-lfuse-t");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/lib");
    }
}
