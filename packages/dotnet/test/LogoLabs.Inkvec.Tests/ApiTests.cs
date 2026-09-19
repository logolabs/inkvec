using System;
using System.IO;
using System.Text.Json;
using System.Text.RegularExpressions;
using LogoLabs.Inkvec;
using Xunit;

namespace LogoLabs.Inkvec.Tests;

public class ApiTests
{
    private static readonly string ContractDir = Path.Combine(AppContext.BaseDirectory, "contract");
    private static readonly string RepoRoot = FindRepoRoot();

    private static string FindRepoRoot()
    {
        var dir = AppContext.BaseDirectory;
        for (var d = new DirectoryInfo(dir); d is not null; d = d.Parent)
        {
            if (File.Exists(Path.Combine(d.FullName, "Cargo.toml")) && Directory.Exists(Path.Combine(d.FullName, "bindings")))
            {
                return d.FullName;
            }
        }
        throw new InvalidOperationException("could not find the repository root above " + dir);
    }

    // ---- identification --------------------------------------------------------------------

    [Fact]
    public void Version_MatchesCargoToml()
    {
        var cargoToml = File.ReadAllText(Path.Combine(RepoRoot, "Cargo.toml"));
        var m = Regex.Match(cargoToml, "^version\\s*=\\s*\"([^\"]+)\"", RegexOptions.Multiline);
        Assert.True(m.Success, "no version in the workspace Cargo.toml");
        Assert.Equal(m.Groups[1].Value, Inkvec.Version);
    }

    [Fact]
    public void BuildTarget_IsReported()
    {
        Assert.False(string.IsNullOrWhiteSpace(Inkvec.BuildTarget));
        // This suite only runs on Windows x64 (see the toolchain caveat in the task/README).
        Assert.Contains("windows", Inkvec.BuildTarget, StringComparison.OrdinalIgnoreCase);
    }

    [Fact]
    public void AbiVersion_IsPositive()
    {
        Assert.True(Inkvec.AbiVersion >= 1);
    }

    [Fact]
    public void OptionsSchema_MatchesCommittedSchema()
    {
        var committed = File.ReadAllText(Path.Combine(RepoRoot, "bindings", "options.schema.json"));
        using var a = JsonDocument.Parse(Inkvec.OptionsSchema);
        using var b = JsonDocument.Parse(committed);
        Assert.True(JsonElementDeepEquals(a.RootElement, b.RootElement), "Inkvec.OptionsSchema != bindings/options.schema.json");
    }

    [Fact]
    public void Defaults_HasEveryOptionFromTheSchema()
    {
        using var schema = JsonDocument.Parse(Inkvec.OptionsSchema);
        using var defaults = JsonDocument.Parse(Inkvec.Defaults);
        var names = new System.Collections.Generic.HashSet<string>();
        foreach (var p in schema.RootElement.GetProperty("properties").EnumerateObject()) names.Add(p.Name);
        var defaultNames = new System.Collections.Generic.HashSet<string>();
        foreach (var p in defaults.RootElement.EnumerateObject()) defaultNames.Add(p.Name);
        Assert.Equal(names, defaultNames);
    }

    /// <summary>A structural compare that does not depend on property order.</summary>
    private static bool JsonElementDeepEquals(JsonElement a, JsonElement b)
    {
        if (a.ValueKind != b.ValueKind) return false;
        switch (a.ValueKind)
        {
            case JsonValueKind.Object:
                var ap = new System.Collections.Generic.Dictionary<string, JsonElement>();
                foreach (var p in a.EnumerateObject()) ap[p.Name] = p.Value;
                var bp = new System.Collections.Generic.Dictionary<string, JsonElement>();
                foreach (var p in b.EnumerateObject()) bp[p.Name] = p.Value;
                if (ap.Count != bp.Count) return false;
                foreach (var kv in ap)
                {
                    if (!bp.TryGetValue(kv.Key, out var bv) || !JsonElementDeepEquals(kv.Value, bv)) return false;
                }
                return true;
            case JsonValueKind.Array:
                var al = a.GetArrayLength();
                if (al != b.GetArrayLength()) return false;
                for (var i = 0; i < al; i++)
                {
                    if (!JsonElementDeepEquals(a[i], b[i])) return false;
                }
                return true;
            default:
                return a.GetRawText() == b.GetRawText();
        }
    }

    // ---- input kinds ------------------------------------------------------------------------

    [Fact]
    public void Trace_ByteArrayAndSpan_Agree()
    {
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        var fromArray = Inkvec.Trace(bytes);
        var fromSpan = Inkvec.Trace(new ReadOnlySpan<byte>(bytes));
        Assert.Equal(fromArray.Svg, fromSpan.Svg);
        Assert.Equal(96u, fromArray.Width);
        Assert.Equal(96u, fromArray.Height);
    }

    [Fact]
    public void Trace_Stream_MatchesByteArray()
    {
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        using var stream = new MemoryStream(bytes);
        var fromStream = Inkvec.Trace(stream);
        var fromArray = Inkvec.Trace(bytes);
        Assert.Equal(fromArray.Svg, fromStream.Svg);
    }

    [Fact]
    public void Trace_NonSeekableStream_MatchesByteArray()
    {
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        using var stream = new NonSeekableStream(bytes);
        var fromStream = Inkvec.Trace(stream);
        var fromArray = Inkvec.Trace(bytes);
        Assert.Equal(fromArray.Svg, fromStream.Svg);
    }

    [Fact]
    public void TraceFile_MatchesByteArray()
    {
        var path = Path.Combine(ContractDir, "tiny.png");
        var fromFile = Inkvec.TraceFile(path);
        var fromArray = Inkvec.Trace(File.ReadAllBytes(path));
        Assert.Equal(fromArray.Svg, fromFile.Svg);
    }

    [Fact]
    public void TraceRgba_MatchesEncodedPngOfSamePixels()
    {
        var rgba = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.rgba"));
        var png = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        var fromRgba = Inkvec.TraceRgba(rgba, 96, 96);
        var fromPng = Inkvec.Trace(png);
        Assert.Equal(fromPng.Svg, fromRgba.Svg);
    }

    // ---- typed options vs. raw JSON ----------------------------------------------------------

    [Fact]
    public void InkvecOptions_ToJson_MatchesHandWrittenEquivalent()
    {
        var options = new InkvecOptions { Colors = 8, Minify = true, NoBackground = true, Harmonize = false };
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        var typed = Inkvec.Trace(bytes, options);
        var raw = Inkvec.Trace(bytes, "{\"colors\": 8, \"minify\": true, \"no_background\": true, \"harmonize\": false}");
        Assert.Equal(raw.Svg, typed.Svg);
    }

    [Fact]
    public void InkvecOptions_EveryPropertyDefaultsToNull()
    {
        var o = new InkvecOptions();
        Assert.Equal("{}", o.ToJson());
        Assert.Null(o.Precision);
        Assert.Null(o.Colors);
        Assert.Null(o.NativeAlpha);
        Assert.Null(o.HarmonizeThreshold);
    }

    [Fact]
    public void NullOptions_TracesWithDefaults()
    {
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        var a = Inkvec.Trace(bytes, (InkvecOptions?)null);
        var b = Inkvec.Trace(bytes, (string?)null);
        var c = Inkvec.Trace(bytes);
        Assert.Equal(a.Svg, b.Svg);
        Assert.Equal(a.Svg, c.Svg);
    }

    private sealed class NonSeekableStream : Stream
    {
        private readonly MemoryStream _inner;
        public NonSeekableStream(byte[] data) => _inner = new MemoryStream(data);
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();
        public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }
        public override void Flush() => _inner.Flush();
        public override int Read(byte[] buffer, int offset, int count) => _inner.Read(buffer, offset, count);
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
        protected override void Dispose(bool disposing)
        {
            if (disposing) _inner.Dispose();
            base.Dispose(disposing);
        }
    }
}
