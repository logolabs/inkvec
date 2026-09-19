#!/usr/bin/env bash
# Build InkvecFFI.xcframework: the static library of crates/inkvec-ffi for macOS (arm64 +
# x86_64), iOS devices (arm64) and the iOS Simulator (arm64 + x86_64), each slice with the
# C header and a module map, then zip it for SwiftPM and record the zip's checksum.
#
#     packages/swift/scripts/build-xcframework.sh [OUT_DIR]     # default packages/swift/build
#
# Writes OUT_DIR/InkvecFFI.xcframework, OUT_DIR/InkvecFFI.xcframework.zip and
# OUT_DIR/InkvecFFI.xcframework.zip.checksum (`swift package compute-checksum` of the zip).
# Needs macOS with Xcode (xcodebuild, lipo, ditto, swift) and a Rust toolchain; the five
# targets are added with rustup when it is there. CARGO_TARGET_DIR is honoured.
#
# The header is crates/inkvec-ffi/include/inkvec.h itself, so the XCFramework cannot drift
# from the library. tvOS, watchOS and visionOS are tier-3 Rust targets (no prebuilt standard
# library) and are not built.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
out="${1:-$root/packages/swift/build}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
target_dir="${CARGO_TARGET_DIR:-$root/target}"

# The minimum versions of Package.swift (`platforms`); rustc reads these for Apple targets.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-10.15}"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-13.0}"

targets=(aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
if command -v rustup >/dev/null 2>&1; then
    rustup target add "${targets[@]}"
fi

# `cargo rustc` builds exactly what `cargo build` does and prints the system libraries the
# static library needs; on Apple platforms they should all be part of libSystem, which every
# binary links. Anything else must be added to the Inkvec target's linkerSettings in
# packages/swift/mirror/Package.swift.in.
for t in "${targets[@]}"; do
    echo "== inkvec-ffi for $t"
    log="$out/cargo-$t.log"
    # pipefail + set -e: a failed build stops the script here.
    cargo rustc --release --locked -p inkvec-ffi --target "$t" \
        --manifest-path "$root/Cargo.toml" -- --print native-static-libs 2>&1 | tee "$log"
    libs="$(sed -n 's/.*native-static-libs: //p' "$log" | tail -1)"
    for l in $libs; do
        case "$l" in
            -lSystem | -lc | -lm) ;;
            *) echo "::warning::libinkvec_ffi.a for $t also needs $l (not in libSystem)" ;;
        esac
    done
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
lib() { echo "$target_dir/$1/release/libinkvec_ffi.a"; }

# Headers/InkvecFFI/, not Headers/: Xcode copies every static XCFramework's headers into one
# include directory, and two frameworks each with a top-level module.modulemap collide. Clang
# still finds the module: `import InkvecFFI` looks for <include dir>/InkvecFFI/module.modulemap.
headers="$work/Headers"
mkdir -p "$headers/InkvecFFI"
cp "$root/crates/inkvec-ffi/include/inkvec.h" "$headers/InkvecFFI/"
cat >"$headers/InkvecFFI/module.modulemap" <<'MODULEMAP'
module InkvecFFI {
    header "inkvec.h"
    export *
}
MODULEMAP

mkdir -p "$work/macos" "$work/ios" "$work/ios-simulator"
lipo -create "$(lib aarch64-apple-darwin)" "$(lib x86_64-apple-darwin)" -output "$work/macos/libinkvec_ffi.a"
cp "$(lib aarch64-apple-ios)" "$work/ios/libinkvec_ffi.a"
lipo -create "$(lib aarch64-apple-ios-sim)" "$(lib x86_64-apple-ios)" -output "$work/ios-simulator/libinkvec_ffi.a"
for slice in macos ios ios-simulator; do
    lipo -info "$work/$slice/libinkvec_ffi.a"
done

xcf="$out/InkvecFFI.xcframework"
zip="$out/InkvecFFI.xcframework.zip"
rm -rf "$xcf" "$zip" "$zip.checksum"
xcodebuild -create-xcframework \
    -library "$work/macos/libinkvec_ffi.a" -headers "$headers" \
    -library "$work/ios/libinkvec_ffi.a" -headers "$headers" \
    -library "$work/ios-simulator/libinkvec_ffi.a" -headers "$headers" \
    -output "$xcf"

# The layout SwiftPM expects in a binary target's zip: InkvecFFI.xcframework/ at the top.
(cd "$out" && ditto -c -k --sequesterRsrc --keepParent InkvecFFI.xcframework InkvecFFI.xcframework.zip)
# SwiftPM's checksum of a binary target: the zip's SHA-256, as the manifest records it.
checksum="$(swift package compute-checksum "$zip")"
echo "$checksum" >"$zip.checksum"

echo "== $xcf"
find "$xcf" -maxdepth 3 | sed "s|$out/||" | sort
ls -l "$zip"
echo "checksum $checksum"
