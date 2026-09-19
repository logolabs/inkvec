// swift-tools-version:5.9
//
// Inkvec for Swift, as built inside the Inkvec repository: the Swift API over the C library
// `inkvec_ffi` (crates/inkvec-ffi), linked as a system library. Build the library first and
// tell the linker where it is:
//
//     cargo build --release -p inkvec-ffi
//     cd packages/swift
//     LIB="$PWD/../../target/release"
//     swift test -Xlinker -L"$LIB" -Xlinker -rpath -Xlinker "$LIB"
//
// Apple apps depend on the mirror, github.com/logolabs/inkvec-swift, instead: the same
// sources with the library prebuilt as an XCFramework (see README.md and mirror/).

import PackageDescription

let package = Package(
    name: "Inkvec",
    platforms: [.macOS(.v10_15), .iOS(.v13)],
    products: [
        .library(name: "Inkvec", targets: ["Inkvec"]),
    ],
    targets: [
        // include/inkvec.h (a generated copy, see bindings/codegen/swift.py) and a module map
        // that links `inkvec_ffi`.
        .systemLibrary(name: "InkvecFFI", path: "Sources/InkvecFFI"),
        .target(name: "Inkvec", dependencies: ["InkvecFFI"], path: "Sources/Inkvec"),
        .testTarget(name: "InkvecTests", dependencies: ["Inkvec"], path: "Tests/InkvecTests"),
    ]
)
