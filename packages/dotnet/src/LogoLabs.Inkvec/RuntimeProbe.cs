#if NET8_0_OR_GREATER
// Fallback search paths for the net8.0 DllImportResolver in NativeMethods.cs. Only used
// when default probing (the assembly's own directory, which is where a plain build or our
// own test copy puts inkvec_ffi.dll -- see LogoLabs.Inkvec.csproj) does not find it: a
// consumer loading this assembly out of the NuGet global packages folder, or a publish
// layout that kept the standard runtimes/<rid>/native/ shape instead of flattening it.

using System.Collections.Generic;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;

namespace LogoLabs.Inkvec
{
    internal static class RuntimeProbe
    {
        internal static IEnumerable<string> CandidatePaths(Assembly assembly)
        {
            var (os, fileName) = OsAndFileName();
            if (os is null) yield break;
            var arch = ArchName();
            if (arch is null) yield break;
            var rid = $"{os}-{arch}";

            var asmDir = Path.GetDirectoryName(assembly.Location);
            if (string.IsNullOrEmpty(asmDir)) yield break;

            // lib/<tfm>/  (a restored nupkg: runtimes/ is two levels up, a sibling of lib/)
            yield return Path.Combine(asmDir, "..", "..", "runtimes", rid, "native", fileName);
            // this assembly's own directory (a flat publish, or our own build/test copy)
            yield return Path.Combine(asmDir, "runtimes", rid, "native", fileName);
            yield return Path.Combine(asmDir, fileName);
        }

        private static (string? os, string fileName) OsAndFileName()
        {
            if (RuntimeInformation.IsOSPlatform(OSPlatform.Windows)) return ("win", "inkvec_ffi.dll");
            if (RuntimeInformation.IsOSPlatform(OSPlatform.Linux)) return ("linux", "libinkvec_ffi.so");
            if (RuntimeInformation.IsOSPlatform(OSPlatform.OSX)) return ("osx", "libinkvec_ffi.dylib");
            return (null, "");
        }

        private static string? ArchName() => RuntimeInformation.ProcessArchitecture switch
        {
            Architecture.X64 => "x64",
            Architecture.Arm64 => "arm64",
            Architecture.X86 => "x86",
            Architecture.Arm => "arm",
            _ => null,
        };
    }
}
#endif
