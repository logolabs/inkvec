// The C ABI (crates/inkvec-ffi/include/inkvec.h) seen from .NET. This file is the only
// place that knows the shape of `inkvec_ffi`: struct layout, function signatures and status
// codes, copied from the header rather than from any option (there are none here -- options
// cross this boundary as one JSON string, same as every other binding; see
// InkvecOptions.generated.cs and Inkvec.cs).
//
// Two P/Invoke styles share one set of declarations: `LibraryImport` (source-generated,
// AOT- and trim-friendly) on net8.0, `DllImport` on netstandard2.0, where LibraryImport does
// not exist. Both bind the same library name, "inkvec_ffi", which .NET's own probing turns
// into inkvec_ffi.dll / libinkvec_ffi.so / libinkvec_ffi.dylib per platform.

using System;
using System.Runtime.InteropServices;

namespace LogoLabs.Inkvec
{
    /// <summary>
    /// Status codes returned by <c>inkvec_trace</c> / <c>inkvec_trace_rgba</c>, from
    /// <c>include/inkvec.h</c>.
    /// </summary>
    internal static class InkvecStatus
    {
        internal const int Ok = 0;
        internal const int InvalidArgument = 1;
        internal const int InvalidImage = 2;
        internal const int InvalidOptions = 3;
        internal const int Internal = 4;
    }

    /// <summary>
    /// Mirrors <c>InkvecResult</c> exactly (field order and size matter: the library reads
    /// <see cref="struct_size"/> to know how much of this struct it may write into, so a
    /// newer library never writes past an older caller's copy -- see
    /// <c>INKVEC_RESULT_INIT</c> in inkvec.h). Every pointer field is owned by the library
    /// until <see cref="NativeMethods.inkvec_result_free"/>.
    /// </summary>
    [StructLayout(LayoutKind.Sequential)]
    internal unsafe struct InkvecResultNative
    {
        public uint struct_size;
        public int status;
        public byte* svg;
        public UIntPtr svg_len;
        public uint width;
        public uint height;
        public byte* error;
    }

#if NET8_0_OR_GREATER
    internal static partial class NativeMethods
    {
        private const string LibName = "inkvec_ffi";

        static NativeMethods()
        {
            NativeLibrary.SetDllImportResolver(typeof(NativeMethods).Assembly, Resolve);
        }

        /// <summary>
        /// A consumer restoring this package as a <c>PackageReference</c> gets the right
        /// <c>runtimes/&lt;rid&gt;/native/</c> asset copied out automatically by the SDK's
        /// own RID-asset selection, and default DllImport probing then finds it next to the
        /// assembly. This resolver is only a fallback for the cases that skip that step --
        /// loading this assembly directly out of the NuGet global packages folder, a
        /// single-file publish, a host that does not run the SDK's asset-selection targets
        /// -- by looking under this assembly's own <c>runtimes/&lt;rid&gt;/native/</c> next
        /// to it, and its package layout one level up.
        /// </summary>
        private static IntPtr Resolve(string libraryName, System.Reflection.Assembly assembly, DllImportSearchPath? searchPath)
        {
            if (libraryName != LibName) return IntPtr.Zero;
            foreach (var candidate in RuntimeProbe.CandidatePaths(assembly))
            {
                if (System.IO.File.Exists(candidate) && NativeLibrary.TryLoad(candidate, out var handle))
                {
                    return handle;
                }
            }
            return IntPtr.Zero;
        }

        [LibraryImport(LibName)]
        internal static unsafe partial int inkvec_trace(byte* bytes, UIntPtr len, byte* optionsJson, InkvecResultNative* out_);

        [LibraryImport(LibName)]
        internal static unsafe partial int inkvec_trace_rgba(byte* rgba, UIntPtr len, uint width, uint height, byte* optionsJson, InkvecResultNative* out_);

        [LibraryImport(LibName)]
        internal static unsafe partial void inkvec_result_free(InkvecResultNative* result);

        [LibraryImport(LibName)]
        internal static unsafe partial byte* inkvec_version();

        [LibraryImport(LibName)]
        internal static unsafe partial byte* inkvec_build_target();

        [LibraryImport(LibName)]
        internal static partial uint inkvec_abi_version();

        [LibraryImport(LibName)]
        internal static unsafe partial byte* inkvec_options_schema();

        [LibraryImport(LibName)]
        internal static unsafe partial byte* inkvec_default_options();
    }
#else
    internal static class NativeMethods
    {
        private const string LibName = "inkvec_ffi";

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe int inkvec_trace(byte* bytes, UIntPtr len, byte* optionsJson, InkvecResultNative* out_);

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe int inkvec_trace_rgba(byte* rgba, UIntPtr len, uint width, uint height, byte* optionsJson, InkvecResultNative* out_);

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe void inkvec_result_free(InkvecResultNative* result);

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe byte* inkvec_version();

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe byte* inkvec_build_target();

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern uint inkvec_abi_version();

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe byte* inkvec_options_schema();

        [DllImport(LibName, CallingConvention = CallingConvention.Cdecl)]
        internal static extern unsafe byte* inkvec_default_options();
    }
#endif
}
