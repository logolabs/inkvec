// The cross-language contract, bindings/contract/cases.json, checked the way
// packages/npm/test/contract.test.mjs and crates/inkvec-py/tests/test_inkvec.py check it:
// every case's reported size or error kind, its SVG's length and SHA-256 against this
// binding's build target (Inkvec.BuildTarget), and every `same_svg_as` pair. A target
// without recorded hashes is reported, not failed, unless INKVEC_CONTRACT_REQUIRE_HASH=1.
//
// Options are sent as the case's own raw JSON (the `string`-options overloads) rather than
// through InkvecOptions: the contract is about what the native library does with a given
// JSON object, independent of whether the generated typed surface has caught up with it.
// ApiTests exercises InkvecOptions itself.

using System;
using System.Collections.Generic;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using LogoLabs.Inkvec;
using Xunit;

namespace LogoLabs.Inkvec.Tests;

public class ContractTests
{
    private static readonly string ContractDir = Path.Combine(AppContext.BaseDirectory, "contract");
    private static readonly JsonDocument Doc = JsonDocument.Parse(File.ReadAllText(Path.Combine(ContractDir, "cases.json")));

    private sealed record Case(
        string Name,
        string Input,
        string Form,
        int? Width,
        int? Height,
        string OptionsJson,
        JsonElement Expect,
        JsonElement? Svg,
        string? SameSvgAs);

    private static IEnumerable<Case> Cases()
    {
        foreach (var c in Doc.RootElement.GetProperty("cases").EnumerateArray())
        {
            yield return new Case(
                Name: c.GetProperty("name").GetString()!,
                Input: c.GetProperty("input").GetString()!,
                Form: c.GetProperty("form").GetString()!,
                Width: c.TryGetProperty("width", out var w) ? w.GetInt32() : null,
                Height: c.TryGetProperty("height", out var h) ? h.GetInt32() : null,
                OptionsJson: c.GetProperty("options").GetRawText(),
                Expect: c.GetProperty("expect"),
                Svg: c.TryGetProperty("svg", out var s) ? s : null,
                SameSvgAs: c.TryGetProperty("same_svg_as", out var sa) ? sa.GetString() : null);
        }
    }

    [Fact]
    public void Contract_EveryCase()
    {
        var target = Inkvec.BuildTarget;
        Assert.False(string.IsNullOrEmpty(target));

        var svgs = new Dictionary<string, string>();
        var unhashed = new List<string>();
        var failures = new List<string>();

        foreach (var c in Cases())
        {
            try
            {
                CheckCase(c, target, svgs, unhashed);
            }
            catch (Exception e)
            {
                failures.Add($"{c.Name}: {e.Message}");
            }
        }

        foreach (var c in Cases())
        {
            if (c.SameSvgAs is null) continue;
            if (!svgs.TryGetValue(c.Name, out var mine) || !svgs.TryGetValue(c.SameSvgAs, out var other))
            {
                continue; // one of the pair errored; already recorded above
            }
            if (mine != other) failures.Add($"{c.Name}: SVG differs from {c.SameSvgAs}");
        }

        if (Environment.GetEnvironmentVariable("INKVEC_CONTRACT_REQUIRE_HASH") == "1" && unhashed.Count > 0)
        {
            failures.Add($"no SVG hashes recorded for {target}:\n  " + string.Join("\n  ", unhashed));
        }

        Assert.True(failures.Count == 0, string.Join("\n", failures));
    }

    private static void CheckCase(Case c, string target, Dictionary<string, string> svgs, List<string> unhashed)
    {
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, c.Input));
        Traced? traced = null;
        Exception? error = null;
        try
        {
            traced = c.Form == "rgba"
                ? Inkvec.TraceRgba(bytes, c.Width!.Value, c.Height!.Value, c.OptionsJson)
                : Inkvec.Trace(bytes, c.OptionsJson);
        }
        catch (InkvecException e)
        {
            error = e;
        }

        if (c.Expect.TryGetProperty("error", out var wantErrorProp))
        {
            var wantKind = wantErrorProp.GetString();
            if (error is null)
            {
                throw new Xunit.Sdk.XunitException($"expected error '{wantKind}' but traced successfully");
            }
            Assert.IsType(ExceptionTypeFor(wantKind!), error);
            return;
        }

        if (error is not null) throw new Xunit.Sdk.XunitException($"unexpected {error.GetType().Name}: {error.Message}");
        Assert.NotNull(traced);

        var hasMargin = TryGetOption(c.OptionsJson, "margin");
        if (!hasMargin)
        {
            Assert.Equal((uint)c.Expect.GetProperty("width").GetInt32(), traced!.Width);
            Assert.Equal((uint)c.Expect.GetProperty("height").GetInt32(), traced!.Height);
        }

        svgs[c.Name] = traced!.Svg;

        var svgBytes = Encoding.UTF8.GetByteCount(traced.Svg);
        var sha256 = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(traced.Svg))).ToLowerInvariant();
        if (c.Svg is { } svgMap && svgMap.TryGetProperty(target, out var want))
        {
            Assert.Equal(want.GetProperty("bytes").GetInt32(), svgBytes);
            Assert.Equal(want.GetProperty("sha256").GetString(), sha256);
        }
        else
        {
            unhashed.Add($"{c.Name}: {{\"bytes\":{svgBytes},\"sha256\":\"{sha256}\"}}");
        }
    }

    private static bool TryGetOption(string optionsJson, string name)
    {
        using var doc = JsonDocument.Parse(optionsJson);
        return doc.RootElement.ValueKind == JsonValueKind.Object && doc.RootElement.TryGetProperty(name, out _);
    }

    private static Type ExceptionTypeFor(string kind) => kind switch
    {
        "invalid_image" => typeof(InvalidImageException),
        "invalid_options" => typeof(InvalidOptionsException),
        "internal" => typeof(InkvecInternalException),
        _ => throw new ArgumentOutOfRangeException(nameof(kind), kind, "unknown error kind"),
    };
}
