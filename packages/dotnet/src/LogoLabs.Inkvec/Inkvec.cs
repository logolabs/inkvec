// Inkvec: raster logos, icons and illustrations to compact, accurate SVG.
//
//   var traced = Inkvec.Trace(File.ReadAllBytes("logo.png"), new InkvecOptions { Colors = 16 });
//   File.WriteAllText("logo.svg", traced.Svg);
//
// Every call crosses to the native library (inkvec_ffi, see NativeMethods.cs) exactly once,
// options as one JSON object -- InkvecOptions.ToJson(), or your own JSON string -- which
// Options::from_json in Rust applies defaults to and validates. Nothing in this class knows
// an option by name; see docs/BINDINGS.md, "One source of truth".

using System;
using System.IO;
using System.Threading;

namespace LogoLabs.Inkvec
{
    /// <summary>
    /// The .NET binding's entry point: static methods over the native <c>inkvec_ffi</c>
    /// library. Every method may be called from any number of threads at once (see
    /// docs/BINDINGS.md, "Threading") and is synchronous -- a trace is CPU-bound work, not
    /// I/O, so there is no <c>async</c> surface here; wrap a call in <c>Task.Run</c> if you
    /// want it off a UI or server request thread.
    /// </summary>
    public static class Inkvec
    {
        private static readonly Lazy<string> LazyVersion = new(ReadVersion);
        private static readonly Lazy<string> LazyBuildTarget = new(ReadBuildTarget);
        private static readonly Lazy<string> LazyOptionsSchema = new(ReadOptionsSchema);
        private static readonly Lazy<string> LazyDefaults = new(ReadDefaultOptions);

        /// <summary>The tracer's version, e.g. <c>"0.1.3"</c>. Matches the NuGet package version.</summary>
        public static string Version => LazyVersion.Value;

        /// <summary>
        /// The target this native library was compiled for, e.g.
        /// <c>"x86_64-windows-msvc"</c>. Output is byte-identical between builds with the
        /// same target; another target can write the same drawing in slightly different
        /// bytes (see docs/BINDINGS.md, "Determinism").
        /// </summary>
        public static string BuildTarget => LazyBuildTarget.Value;

        /// <summary>The ABI version this library implements; changes only on a breaking change.</summary>
        public static uint AbiVersion => NativeMethods.inkvec_abi_version();

        /// <summary>
        /// The JSON Schema (draft 2020-12) of the options object, read from the native
        /// library itself -- the same document <c>InkvecOptions</c> was generated from.
        /// </summary>
        public static string OptionsSchema => LazyOptionsSchema.Value;

        /// <summary>Every option at its default, as a compact JSON object.</summary>
        public static string Defaults => LazyDefaults.Value;

        // ---- encoded images ---------------------------------------------------------------

        /// <summary>
        /// Traces an encoded image (PNG, JPEG, WebP, GIF, BMP or TIFF) to SVG.
        /// </summary>
        /// <param name="image">The file's bytes.</param>
        /// <param name="options">
        /// Options for this trace; a property left <see langword="null"/> takes the
        /// tracer's default. <see langword="null"/> (the default) traces with every option
        /// at its default.
        /// </param>
        /// <exception cref="InvalidImageException">The bytes are not a decodable image.</exception>
        /// <exception cref="InvalidOptionsException">An option is unknown, of the wrong type, or out of range.</exception>
        /// <exception cref="InkvecInternalException">The tracer failed. Not your fault; worth a bug report.</exception>
        public static Traced Trace(ReadOnlySpan<byte> image, InkvecOptions? options = null) =>
            Trace(image, options?.ToJson());

        /// <inheritdoc cref="Trace(ReadOnlySpan{byte}, InkvecOptions?)"/>
        public static Traced Trace(byte[] image, InkvecOptions? options = null) =>
            Trace(new ReadOnlySpan<byte>(image ?? throw new ArgumentNullException(nameof(image))), options);

        /// <summary>
        /// Traces an encoded image with options passed as a raw JSON object -- the same
        /// string every other binding sends across the C ABI. Useful for an option added to
        /// <c>inkvec::Options</c> before this package's generated <see cref="InkvecOptions"/>
        /// has been regenerated for it.
        /// </summary>
        public static Traced Trace(ReadOnlySpan<byte> image, string? optionsJson)
        {
            unsafe
            {
                fixed (byte* imagePtr = image)
                {
                    return TraceCore(imagePtr, (nuint)image.Length, optionsJson);
                }
            }
        }

        /// <inheritdoc cref="Trace(ReadOnlySpan{byte}, string?)"/>
        public static Traced Trace(byte[] image, string? optionsJson) =>
            Trace(new ReadOnlySpan<byte>(image ?? throw new ArgumentNullException(nameof(image))), optionsJson);

        /// <summary>
        /// Reads <paramref name="stream"/> to the end and traces it as an encoded image.
        /// </summary>
        public static Traced Trace(Stream stream, InkvecOptions? options = null)
        {
            if (stream is null) throw new ArgumentNullException(nameof(stream));
            return Trace(ReadAllBytes(stream), options);
        }

        /// <summary>Reads <paramref name="path"/> and traces it as an encoded image.</summary>
        public static Traced TraceFile(string path, InkvecOptions? options = null)
        {
            if (path is null) throw new ArgumentNullException(nameof(path));
            return Trace(File.ReadAllBytes(path), options);
        }

        // ---- raw pixels ---------------------------------------------------------------------

        /// <summary>
        /// Traces raw pixels -- straight (not premultiplied) RGBA, 8 bits per channel,
        /// row-major, tightly packed, exactly <c>width * height * 4</c> bytes -- to SVG.
        /// Byte-identical to <see cref="Trace(ReadOnlySpan{byte}, InkvecOptions?)"/> on a
        /// PNG holding the same pixels.
        /// </summary>
        /// <exception cref="InvalidImageException"><paramref name="rgba"/>'s length does not match <paramref name="width"/> * <paramref name="height"/> * 4.</exception>
        public static Traced TraceRgba(ReadOnlySpan<byte> rgba, int width, int height, InkvecOptions? options = null) =>
            TraceRgba(rgba, width, height, options?.ToJson());

        /// <inheritdoc cref="TraceRgba(ReadOnlySpan{byte}, int, int, InkvecOptions?)"/>
        public static Traced TraceRgba(byte[] rgba, int width, int height, InkvecOptions? options = null) =>
            TraceRgba(new ReadOnlySpan<byte>(rgba ?? throw new ArgumentNullException(nameof(rgba))), width, height, options);

        /// <summary>Raw-JSON-options overload; see <see cref="Trace(ReadOnlySpan{byte}, string?)"/>.</summary>
        public static Traced TraceRgba(ReadOnlySpan<byte> rgba, int width, int height, string? optionsJson)
        {
            if (width <= 0) throw new ArgumentOutOfRangeException(nameof(width));
            if (height <= 0) throw new ArgumentOutOfRangeException(nameof(height));
            unsafe
            {
                fixed (byte* rgbaPtr = rgba)
                {
                    return TraceRgbaCore(rgbaPtr, (nuint)rgba.Length, (uint)width, (uint)height, optionsJson);
                }
            }
        }

        /// <inheritdoc cref="TraceRgba(ReadOnlySpan{byte}, int, int, string?)"/>
        public static Traced TraceRgba(byte[] rgba, int width, int height, string? optionsJson) =>
            TraceRgba(new ReadOnlySpan<byte>(rgba ?? throw new ArgumentNullException(nameof(rgba))), width, height, optionsJson);

        // ---- native calls ---------------------------------------------------------------------

        private static unsafe Traced TraceCore(byte* imagePtr, nuint len, string? optionsJson)
        {
            var optionsBytes = Utf8.ToNulTerminated(optionsJson);
            fixed (byte* optionsPtr = optionsBytes)
            {
                var result = default(InkvecResultNative);
                result.struct_size = (uint)sizeof(InkvecResultNative);
                var status = NativeMethods.inkvec_trace(imagePtr, len, optionsPtr, &result);
                return FinishOrThrow(status, &result);
            }
        }

        private static unsafe Traced TraceRgbaCore(byte* rgbaPtr, nuint len, uint width, uint height, string? optionsJson)
        {
            var optionsBytes = Utf8.ToNulTerminated(optionsJson);
            fixed (byte* optionsPtr = optionsBytes)
            {
                var result = default(InkvecResultNative);
                result.struct_size = (uint)sizeof(InkvecResultNative);
                var status = NativeMethods.inkvec_trace_rgba(rgbaPtr, len, width, height, optionsPtr, &result);
                return FinishOrThrow(status, &result);
            }
        }

        private static unsafe Traced FinishOrThrow(int status, InkvecResultNative* result)
        {
            try
            {
                if (status == InkvecStatus.Ok)
                {
                    var svg = Utf8.FromLength(result->svg, result->svg_len);
                    return new Traced(svg, result->width, result->height);
                }

                var message = Utf8.FromNulTerminated(result->error) ?? $"inkvec: status {status}";
                throw status switch
                {
                    InkvecStatus.InvalidImage => new InvalidImageException(message),
                    InkvecStatus.InvalidOptions => new InvalidOptionsException(message),
                    InkvecStatus.InvalidArgument => new InkvecInternalException(
                        $"inkvec: invalid call ({message}); this indicates a bug in the .NET binding, not your input"),
                    _ => new InkvecInternalException(message),
                };
            }
            finally
            {
                NativeMethods.inkvec_result_free(result);
            }
        }

        private static unsafe string ReadVersion() => Utf8.FromNulTerminated(NativeMethods.inkvec_version()) ?? string.Empty;

        private static unsafe string ReadBuildTarget() => Utf8.FromNulTerminated(NativeMethods.inkvec_build_target()) ?? string.Empty;

        private static unsafe string ReadOptionsSchema() => Utf8.FromNulTerminated(NativeMethods.inkvec_options_schema()) ?? string.Empty;

        private static unsafe string ReadDefaultOptions() => Utf8.FromNulTerminated(NativeMethods.inkvec_default_options()) ?? string.Empty;

        private static byte[] ReadAllBytes(Stream stream)
        {
            if (stream.CanSeek)
            {
                var buffer = new byte[stream.Length - stream.Position];
                var offset = 0;
                int read;
                while (offset < buffer.Length && (read = stream.Read(buffer, offset, buffer.Length - offset)) > 0)
                {
                    offset += read;
                }
                return buffer;
            }
            using var mem = new MemoryStream();
            stream.CopyTo(mem);
            return mem.ToArray();
        }
    }
}
