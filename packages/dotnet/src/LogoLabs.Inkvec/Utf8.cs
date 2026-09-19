using System;
using System.Text;

namespace LogoLabs.Inkvec
{
    /// <summary>
    /// The native library hands back UTF-8, not the platform's ANSI codepage, so the
    /// built-in <c>Marshal</c> string marshalling (which assumes ANSI, or needs
    /// <c>Marshal.PtrToStringUTF8</c>, absent on netstandard2.0) is not used here. These
    /// helpers read and write UTF-8 by hand instead.
    /// </summary>
    internal static unsafe class Utf8
    {
        /// <summary>Decodes a NUL-terminated UTF-8 C string. Null on a null pointer.</summary>
        internal static string? FromNulTerminated(byte* ptr)
        {
            if (ptr == null) return null;
            var len = 0;
            while (ptr[len] != 0) len++;
            return Encoding.UTF8.GetString(ptr, len);
        }

        /// <summary>Decodes exactly <paramref name="length"/> bytes of UTF-8.</summary>
        internal static string FromLength(byte* ptr, nuint length)
        {
            if (length == 0) return string.Empty;
            return Encoding.UTF8.GetString(ptr, checked((int)length));
        }

        /// <summary>
        /// A UTF-8, NUL-terminated copy of <paramref name="s"/> (or of <c>"{}"</c> for
        /// <see langword="null"/>/empty), ready to pin and pass as a C string.
        /// </summary>
        internal static byte[] ToNulTerminated(string? s)
        {
            var text = string.IsNullOrEmpty(s) ? "{}" : s!;
            var byteCount = Encoding.UTF8.GetByteCount(text);
            var buf = new byte[byteCount + 1];
            Encoding.UTF8.GetBytes(text, 0, text.Length, buf, 0);
            buf[byteCount] = 0;
            return buf;
        }
    }
}
