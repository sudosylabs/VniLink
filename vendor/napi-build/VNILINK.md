Based on napi-build 2.4.1 from crates.io, MIT. Archive SHA-256: 60fdf9b392c50e7c4170fa633bd909490ed7835cea4c046776d1a4dd8d2ae0ab.

The Windows GNU setup requires and links libnode.dll even when the calling napi crate enables dyn-symbols. Skip that static-link setup only when the caller explicitly enables dyn-symbols; other GNU consumers preserve upstream linkage. VniLink uses dynamic Node-API resolution from its Electron/Node host and also skips GNU static setup in its own addon build script. Validate with native Windows Rust tests and both Node and Electron addon loads.
