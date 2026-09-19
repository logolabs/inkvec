// Inkvec for Swift: raster logos, icons and illustrations to compact, accurate SVG.
//
// A thin layer over the C library `inkvec_ffi` (crates/inkvec-ffi). Options cross it as one
// JSON object, which the Rust library parses, completes with its defaults and validates, so
// nothing here knows any option by name: `InkvecOptions` is generated from the options
// schema (bindings/codegen/swift.py), and `trace(_:optionsJSON:)` passes JSON through as is.

import Foundation
import InkvecFFI

/// The tracer. Every entry point is a static function.
///
/// ```swift
/// let traced = try Inkvec.trace(contentsOf: url, options: InkvecOptions(colors: 16))
/// try traced.svg.write(to: out, atomically: true, encoding: .utf8)
/// ```
///
/// **Threads.** Every function may be called from any number of threads at once; that is the
/// C library's guarantee (crates/inkvec-ffi). A trace is synchronous: it blocks the calling
/// thread until the SVG is written -- well under a second for an icon, seconds for a large
/// logo -- while the tracer spreads its own work over a thread pool it manages (one per
/// process). Call it off the main thread.
///
/// **Determinism.** On one build target (`buildTarget`), the same input and options give
/// byte-identical SVG across runs, thread counts and languages, as long as `timeBudget` is 0
/// (the default). Another target can write the same drawing slightly differently.
public enum Inkvec {
    /// The result of one trace.
    public struct Traced: Hashable, Sendable, CustomStringConvertible {
        /// The SVG document.
        public let svg: String
        /// Width of the input image, in pixels (the SVG's `width` unless a margin grew it).
        public let width: Int
        /// Height of the input image, in pixels.
        public let height: Int

        /// Creates a result. Traces are made by `Inkvec.trace`; this is for tests and mocks.
        public init(svg: String, width: Int, height: Int) {
            self.svg = svg
            self.width = width
            self.height = height
        }

        /// The SVG document.
        public var description: String { svg }
    }

    // MARK: Encoded images

    /// Traces an encoded image -- the bytes of a PNG, JPEG, WebP, GIF, BMP or TIFF file.
    ///
    /// The container is read as well as the pixels: JPEG and lossy WebP are traced with the
    /// noise-aware intake the command line uses for them.
    ///
    /// - Throws: `InkvecError`.
    public static func trace(_ image: Data, options: InkvecOptions = .init()) throws -> Traced {
        try trace(image, optionsJSON: options.jsonString())
    }

    /// Traces the image file at `url`.
    ///
    /// - Throws: the error of `Data(contentsOf:)` when the file cannot be read, otherwise
    ///   `InkvecError`.
    public static func trace(contentsOf url: URL, options: InkvecOptions = .init()) throws -> Traced {
        try trace(Data(contentsOf: url), options: options)
    }

    /// Traces an encoded image with its options given as a JSON object, passed through to the
    /// tracer as is: `{"colors": 16, "no_background": true}`. `nil` or `""` means every
    /// default. The names are the options schema's (`optionsSchema`); an unknown name, a
    /// wrong type or a value out of range is `InkvecError.invalidOptions`.
    ///
    /// - Throws: `InkvecError`.
    public static func trace(_ image: Data, optionsJSON: String?) throws -> Traced {
        try withCString(optionsJSON) { json in
            try withBytes(image) { bytes, count in
                try call { inkvec_trace(bytes, count, json, $0) }
            }
        }
    }

    // MARK: Raw pixels

    /// Traces raw pixels: straight (not premultiplied) RGBA, 8 bits per channel, row-major,
    /// tightly packed -- exactly `width * height * 4` bytes.
    ///
    /// The SVG is byte-identical to `trace(_:options:)` on a PNG holding the same pixels.
    /// Raw pixels have no container, so the pixels of a JPEG are not treated as lossily
    /// compressed; pass the file's bytes to `trace` to keep that.
    ///
    /// - Throws: `InkvecError.invalidImage` when the buffer does not match the size given.
    public static func traceRGBA(
        _ pixels: Data, width: Int, height: Int, options: InkvecOptions = .init()
    ) throws -> Traced {
        try traceRGBA(pixels, width: width, height: height, optionsJSON: options.jsonString())
    }

    /// Traces raw straight-RGBA8 pixels with the options given as a JSON object, passed
    /// through as is (see `trace(_:optionsJSON:)`).
    ///
    /// - Throws: `InkvecError`.
    public static func traceRGBA(
        _ pixels: Data, width: Int, height: Int, optionsJSON: String?
    ) throws -> Traced {
        guard let w = UInt32(exactly: width), let h = UInt32(exactly: height) else {
            throw InkvecError.invalidImage(
                "invalid image: width and height must be 0 to \(UInt32.max), not \(width) x \(height)")
        }
        return try withCString(optionsJSON) { json in
            try withBytes(pixels) { bytes, count in
                try call { inkvec_trace_rgba(bytes, count, w, h, json, $0) }
            }
        }
    }

    // MARK: About the library

    /// The library version, e.g. `"0.1.3"`: the Inkvec release this was built from.
    public static let version: String = String(cString: inkvec_version())

    /// The target the native library was compiled for, e.g. `"aarch64-macos"` or
    /// `"x86_64-linux-gnu"`. Output is byte-identical between builds with the same target;
    /// another target can write the same drawing slightly differently.
    public static let buildTarget: String = String(cString: inkvec_build_target())

    /// The JSON Schema (draft 2020-12) of the options object: every option's name, type,
    /// default, description and range, read from the library.
    public static let optionsSchema: String = String(cString: inkvec_options_schema())

    /// Every option at its default, as a compact JSON object, read from the library.
    public static let defaultsJSON: String = String(cString: inkvec_default_options())

    /// Every option at its default, read from the library: all properties are set.
    public static let defaults: InkvecOptions = {
        // A decode failure could only mean a generated struct older than the library; the
        // empty options then still mean "every default".
        (try? JSONDecoder().decode(InkvecOptions.self, from: Data(defaultsJSON.utf8))) ?? InkvecOptions()
    }()

    // MARK: The C boundary

    /// Refuses to call a library built for another C ABI than the header compiled in here.
    private static let abiMismatch: InkvecError? = {
        let library = inkvec_abi_version()
        let header = UInt32(INKVEC_ABI_VERSION)
        return library == header
            ? nil
            : .internal("internal error: the inkvec_ffi library has C ABI \(library), this package expects \(header)")
    }()

    /// Runs one trace entry point on a fresh result and turns it into a `Traced` or an error.
    /// The result's strings are always released.
    private static func call(_ body: (UnsafeMutablePointer<InkvecResult>) -> Int32) throws -> Traced {
        if let error = abiMismatch { throw error }
        var result = inkvec_result_init()
        defer { inkvec_result_free(&result) }
        let status = withUnsafeMutablePointer(to: &result) { body($0) }
        if status == INKVEC_OK, let svg = result.svg {
            let bytes = UnsafeRawBufferPointer(start: svg, count: result.svg_len)
            return Traced(
                svg: String(decoding: bytes, as: UTF8.self),
                width: Int(result.width),
                height: Int(result.height))
        }
        let message = result.error.map { String(cString: $0) } ?? "inkvec: status \(status)"
        switch status {
        case INKVEC_ERR_INVALID_IMAGE: throw InkvecError.invalidImage(message)
        case INKVEC_ERR_INVALID_OPTIONS: throw InkvecError.invalidOptions(message)
        case INKVEC_ERR_INTERNAL: throw InkvecError.internal(message)
        // INKVEC_ERR_INVALID_ARGUMENT (a NULL pointer, a short result struct) or a status
        // this package does not know: a fault of this binding, not of the caller.
        default: throw InkvecError.internal("internal error in the Swift binding (status \(status)): \(message)")
        }
    }

    /// `body` with the bytes of `data`, never with a NULL pointer: an empty buffer reaches the
    /// library as zero bytes, which it reports as an invalid image.
    private static func withBytes<R>(
        _ data: Data, _ body: (UnsafePointer<UInt8>, Int) throws -> R
    ) rethrows -> R {
        try data.withUnsafeBytes { raw in
            if let base = raw.bindMemory(to: UInt8.self).baseAddress {
                return try body(base, raw.count)
            }
            let empty: UInt8 = 0
            return try withUnsafePointer(to: empty) { try body($0, 0) }
        }
    }

    /// `body` with `text` as a NUL-terminated C string, or with NULL for `nil`.
    private static func withCString<R>(
        _ text: String?, _ body: (UnsafePointer<CChar>?) throws -> R
    ) rethrows -> R {
        guard let text else { return try body(nil) }
        return try text.withCString { try body($0) }
    }
}

/// The result of one trace; the same type as `Inkvec.Traced`.
public typealias Traced = Inkvec.Traced

/// Why a trace failed. The cases are the error kinds every Inkvec binding reports.
public enum InkvecError: Error, Hashable, Sendable, CustomStringConvertible {
    /// Not an image the tracer can decode, or raw pixels that do not match the size given.
    case invalidImage(String)
    /// An unknown option, a value of the wrong type or out of range, or malformed JSON.
    case invalidOptions(String)
    /// The tracer failed. Not the caller's fault; worth a bug report.
    case `internal`(String)

    /// The kind of error as the shared contract names it: `"invalid_image"`,
    /// `"invalid_options"` or `"internal"`.
    public var kind: String {
        switch self {
        case .invalidImage: return "invalid_image"
        case .invalidOptions: return "invalid_options"
        case .internal: return "internal"
        }
    }

    /// The library's human-readable message.
    public var message: String {
        switch self {
        case .invalidImage(let m), .invalidOptions(let m), .internal(let m): return m
        }
    }

    public var description: String { message }
}

extension InkvecError: LocalizedError {
    public var errorDescription: String? { message }
}

extension InkvecOptions {
    /// These options as the JSON object the tracer receives: only the options that are set,
    /// under the schema's names. Throws `InkvecError.invalidOptions` for a number JSON cannot
    /// hold (NaN or an infinity).
    public func jsonString() throws -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        do {
            return String(decoding: try encoder.encode(self), as: UTF8.self)
        } catch let EncodingError.invalidValue(value, context) {
            let name = context.codingPath.map(\.stringValue).joined(separator: ".")
            throw InkvecError.invalidOptions("invalid options: `\(name)` is \(value), which JSON cannot hold")
        }
    }
}
