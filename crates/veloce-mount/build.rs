// Link FUSE-T (or macFUSE, which exposes the same libfuse 2.9 interface) weakly: the
// app must still start on a Mac without either, and mounting then says "install
// FUSE-T" instead of the whole program failing to launch.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-L/usr/local/lib");
        println!("cargo:rustc-link-arg=-Wl,-weak-lfuse-t");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/lib");
    }
}
