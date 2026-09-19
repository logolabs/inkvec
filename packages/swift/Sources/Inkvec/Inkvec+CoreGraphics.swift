// Tracing images the Apple frameworks already hold: CGImage, and UIImage / NSImage through
// their CGImage. The pixels are converted to straight (not premultiplied) sRGB RGBA8 by
// vImage and traced with `Inkvec.traceRGBA`. Only on Apple platforms.

#if canImport(CoreGraphics) && canImport(Accelerate)
import Accelerate
import CoreGraphics
import Foundation

extension Inkvec {
    /// Traces a `CGImage`. Its pixels are converted to straight sRGB RGBA8 and traced with
    /// `traceRGBA`, so a JPEG's pixels are not treated as lossily compressed; to keep that,
    /// pass the file's bytes to `trace(_:options:)` instead.
    ///
    /// - Throws: `InkvecError`.
    public static func trace(_ image: CGImage, options: InkvecOptions = .init()) throws -> Traced {
        let (pixels, width, height) = try straightRGBA(image)
        return try traceRGBA(pixels, width: width, height: height, options: options)
    }

    /// The image as straight sRGB RGBA8, tightly packed, with its width and height.
    static func straightRGBA(_ image: CGImage) throws -> (Data, Int, Int) {
        guard
            let srgb = CGColorSpace(name: CGColorSpace.sRGB),
            let format = vImage_CGImageFormat(
                bitsPerComponent: 8,
                bitsPerPixel: 32,
                colorSpace: srgb,
                bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.last.rawValue))
        else {
            throw InkvecError.internal("internal error: no straight-alpha sRGB RGBA8 format")
        }
        var buffer: vImage_Buffer
        do {
            buffer = try vImage_Buffer(cgImage: image, format: format)
        } catch {
            throw InkvecError.invalidImage("invalid image: cannot read the CGImage's pixels (\(error))")
        }
        defer { buffer.free() }
        let width = Int(buffer.width)
        let height = Int(buffer.height)
        let row = width * 4
        var pixels = Data(count: row * height)
        pixels.withUnsafeMutableBytes { out in
            guard let dst = out.baseAddress, let src = buffer.data else { return }
            for y in 0..<height {
                (dst + y * row).copyMemory(from: src + y * buffer.rowBytes, byteCount: row)
            }
        }
        return (pixels, width, height)
    }
}
#endif

#if canImport(UIKit)
import UIKit

extension Inkvec {
    /// Traces a `UIImage` through its `cgImage`. The pixels are traced as stored:
    /// `imageOrientation` is not applied.
    ///
    /// - Throws: `InkvecError.invalidImage` for an image without a `CGImage` (one backed by
    ///   a `CIImage`), otherwise as `trace(_: CGImage, options:)`.
    public static func trace(_ image: UIImage, options: InkvecOptions = .init()) throws -> Traced {
        guard let cgImage = image.cgImage else {
            throw InkvecError.invalidImage("invalid image: the UIImage has no CGImage")
        }
        return try trace(cgImage, options: options)
    }
}
#elseif canImport(AppKit)
import AppKit

extension Inkvec {
    /// Traces an `NSImage` through the `CGImage` it draws best at its own size.
    ///
    /// - Throws: `InkvecError.invalidImage` for an image that yields no `CGImage`, otherwise
    ///   as `trace(_: CGImage, options:)`.
    public static func trace(_ image: NSImage, options: InkvecOptions = .init()) throws -> Traced {
        guard let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            throw InkvecError.invalidImage("invalid image: the NSImage has no CGImage")
        }
        return try trace(cgImage, options: options)
    }
}
#endif
