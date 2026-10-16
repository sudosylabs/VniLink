fn main() {
    // The addon enables dyn-symbols and resolves Node-API from the host.
    // GNU's static setup requires MSYS2 libnode.dll, which Electron does not ship.
    let windows_gnu = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");
    if !windows_gnu {
        napi_build::setup();
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // The retained ScreenCaptureKit adapter links the system Swift runtime.
        // A Node addon cannot rely on an application's existing Swift rpaths.
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
