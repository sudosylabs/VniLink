fn main() {
    napi_build::setup();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // The retained ScreenCaptureKit adapter links the system Swift runtime.
        // A Node addon cannot rely on an application's existing Swift rpaths.
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
