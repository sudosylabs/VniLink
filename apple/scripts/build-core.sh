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
cargo build --locked -p vnilink-ffi --target "$RUST_TARGET"
cargo build --locked -p uniffi-bindgen
mkdir -p apple/.generated apple/VnilinkCore/Sources/VnilinkCore
target/debug/uniffi-bindgen generate --library "target/$RUST_TARGET/debug/libvnilink_ffi.dylib" --language swift --out-dir apple/.generated
cp apple/.generated/vnilink_ffi.swift apple/VnilinkCore/Sources/VnilinkCore/
mkdir -p apple/.generated/headers
cp apple/.generated/vnilink_ffiFFI.h apple/.generated/headers/
cp apple/.generated/vnilink_ffiFFI.modulemap apple/.generated/headers/module.modulemap
rm -rf apple/VnilinkCore/vnilink_ffiFFI.xcframework
xcodebuild -create-xcframework -library "$ROOT/target/$RUST_TARGET/debug/libvnilink_ffi.a" -headers "$ROOT/apple/.generated/headers" -output "$ROOT/apple/VnilinkCore/vnilink_ffiFFI.xcframework"
