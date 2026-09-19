// White artwork on a transparent ground, defaults (native_alpha: true; see
// docs/BINDINGS.md, "Transparency (native, on by default)"): the shape should trace as
// white ink, and there should be no face covering the whole canvas (a "background rect")
// the way there would if the image had first been composited onto a matte.

using System;
using System.Globalization;
using System.IO;
using System.Text.RegularExpressions;
using LogoLabs.Inkvec;
using Xunit;

namespace LogoLabs.Inkvec.Tests;

public class TransparencyTests
{
    private static readonly string ContractDir = Path.Combine(AppContext.BaseDirectory, "contract");

    [Fact]
    public void WhiteOnClear_Defaults_NoBackgroundRect_WhiteInkPresent()
    {
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "white_on_clear.png"));
        var traced = Inkvec.Trace(bytes);

        Assert.Equal(64u, traced.Width);
        Assert.Equal(64u, traced.Height);
        Assert.Contains("#ffffff", traced.Svg, StringComparison.OrdinalIgnoreCase);

        foreach (Match m in Regex.Matches(traced.Svg, "<rect[^>]*\\bwidth=\"([0-9.]+)\"[^>]*\\bheight=\"([0-9.]+)\""))
        {
            var w = double.Parse(m.Groups[1].Value, CultureInfo.InvariantCulture);
            var h = double.Parse(m.Groups[2].Value, CultureInfo.InvariantCulture);
            Assert.False(w >= traced.Width * 0.95 && h >= traced.Height * 0.95,
                $"found a rect spanning the whole canvas ({w}x{h} of {traced.Width}x{traced.Height}): {m.Value}");
        }
    }

    [Fact]
    public void WhiteOnClear_CompositedWithoutCutout_IsNotEmpty()
    {
        // native_alpha: false without cutout used to lose the white-on-transparent shape
        // entirely (composited onto black, then the near-black ink discarded); it is kept
        // here only as a smoke test that the option round-trips, not a claim about its
        // geometry -- see bindings/contract/cases.json's clear_composited.
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "white_on_clear.png"));
        var traced = Inkvec.Trace(bytes, new InkvecOptions { NativeAlpha = false });
        Assert.NotEmpty(traced.Svg);
    }

    [Fact]
    public void NoBackground_KnocksOutAnOpaqueCanvasFillingFace()
    {
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        var withBackground = Inkvec.Trace(bytes);
        var withoutBackground = Inkvec.Trace(bytes, new InkvecOptions { NoBackground = true });
        Assert.NotEqual(withBackground.Svg, withoutBackground.Svg);
    }
}
