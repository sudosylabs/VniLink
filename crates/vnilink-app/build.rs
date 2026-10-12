fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Standalone adapter tests also link the retained Swift platform shims.
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
