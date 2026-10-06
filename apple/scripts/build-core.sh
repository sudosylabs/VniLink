#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
export MACOSX_DEPLOYMENT_TARGET=15.0
case "$(uname -m)" in
  arm64) RUST_TARGET=aarch64-apple-darwin ;;
  x86_64) RUST_TARGET=x86_64-apple-darwin ;;
  *) echo "Unsupported Mac architecture" >&2; exit 1 ;;
esac
# The UI can remain a Debug build, but pixel conversion and media workers
# must be optimized for real desktop sessions. Opt into dev for Rust debugging.
CORE_PROFILE="${VNILINK_CORE_PROFILE:-release}"
case "$CORE_PROFILE" in
  release) cargo build --release --locked -p vnilink-ffi --target "$RUST_TARGET" ;;
  debug) cargo build --locked -p vnilink-ffi --target "$RUST_TARGET" ;;
  *) echo "VNILINK_CORE_PROFILE must be release or debug" >&2; exit 1 ;;
esac
CORE_LIBRARY="$ROOT/target/$RUST_TARGET/$CORE_PROFILE/libvnilink_ffi"
cargo build --locked -p uniffi-bindgen
mkdir -p apple/.generated apple/VnilinkCore/Sources/VnilinkCore
target/debug/uniffi-bindgen generate --library "$CORE_LIBRARY.dylib" --language swift --out-dir apple/.generated
cp apple/.generated/vnilink_ffi.swift apple/VnilinkCore/Sources/VnilinkCore/
mkdir -p apple/.generated/headers
cp apple/.generated/vnilink_ffiFFI.h apple/.generated/headers/
cp apple/.generated/vnilink_ffiFFI.modulemap apple/.generated/headers/module.modulemap
rm -rf apple/VnilinkCore/vnilink_ffiFFI.xcframework
xcodebuild -create-xcframework -library "$CORE_LIBRARY.a" -headers "$ROOT/apple/.generated/headers" -output "$ROOT/apple/VnilinkCore/vnilink_ffiFFI.xcframework"
