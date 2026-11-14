#!/bin/bash
# Builds the shared Rust engine (desktop/crates/ffi) for the iPhone, the
# iPhone Simulator and Apple Silicon Macs, and packages it as
# apple/Frameworks/ArcVoiceCore.xcframework for Xcode to link.
#
# Needs Rust from rustup.rs plus the iOS targets:
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
desktop="$root/desktop"
output="$root/apple/Frameworks/ArcVoiceCore.xcframework"
headers="$desktop/crates/ffi/include"

export IPHONEOS_DEPLOYMENT_TARGET=18.0
export MACOSX_DEPLOYMENT_TARGET=15.0

targets=(aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin)
for target in "${targets[@]}"; do
    echo "Building the engine for $target"
    cargo build --manifest-path "$desktop/Cargo.toml" --release -p arcvoice-ffi --target "$target"
done

rm -rf "$output"
mkdir -p "$(dirname "$output")"
arguments=()
for target in "${targets[@]}"; do
    arguments+=(-library "$desktop/target/$target/release/libarcvoice_ffi.a" -headers "$headers")
done
xcodebuild -create-xcframework "${arguments[@]}" -output "$output"
echo "Wrote $output"
