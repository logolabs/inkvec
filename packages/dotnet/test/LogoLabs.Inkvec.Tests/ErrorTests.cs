using System;
using System.IO;
using LogoLabs.Inkvec;
using Xunit;

namespace LogoLabs.Inkvec.Tests;

public class ErrorTests
{
    private static readonly string ContractDir = Path.Combine(AppContext.BaseDirectory, "contract");
    private static byte[] Tiny => File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
    private static byte[] WhiteOnClearRgba => File.ReadAllBytes(Path.Combine(ContractDir, "white_on_clear.rgba"));

    [Fact]
    public void UnknownOption_ThrowsInvalidOptionsException_FromRust()
    {
        // Not "colors" -- the native library rejects it, this binding contains no
        // per-option validation of its own (see docs/BINDINGS.md, "One source of truth").
        var e = Assert.Throws<InvalidOptionsException>(() => Inkvec.Trace(Tiny, "{\"colours\": 8}"));
        Assert.Contains("colours", e.Message);
    }

    [Fact]
    public void OutOfRangeOption_ThrowsInvalidOptionsException()
    {
        Assert.Throws<InvalidOptionsException>(() => Inkvec.Trace(Tiny, new InkvecOptions { Colors = 0 }));
    }

    [Fact]
    public void WrongTypeOption_ThrowsInvalidOptionsException()
    {
        Assert.Throws<InvalidOptionsException>(() => Inkvec.Trace(Tiny, "{\"cutout\": \"yes\"}"));
    }

    [Fact]
    public void UndecodableBytes_ThrowInvalidImageException()
    {
        // The raw RGBA fixture is not a container format (PNG/JPEG/...); Trace (not
        // TraceRgba) must reject it as an image.
        Assert.Throws<InvalidImageException>(() => Inkvec.Trace(WhiteOnClearRgba));
    }

    [Fact]
    public void GarbageBytes_ThrowInvalidImageException()
    {
        var garbage = new byte[] { 1, 2, 3, 4, 5 };
        Assert.Throws<InvalidImageException>(() => Inkvec.Trace(garbage));
    }

    [Fact]
    public void MismatchedRgbaSize_ThrowsInvalidImageException()
    {
        // white_on_clear.rgba is 64x64 pixels; claiming 60x64 does not divide evenly.
        Assert.Throws<InvalidImageException>(() => Inkvec.TraceRgba(WhiteOnClearRgba, 60, 64));
    }

    [Fact]
    public void EveryInkvecExceptionDerivesFromInkvecException()
    {
        Assert.True(typeof(InkvecException).IsAssignableFrom(typeof(InvalidImageException)));
        Assert.True(typeof(InkvecException).IsAssignableFrom(typeof(InvalidOptionsException)));
        Assert.True(typeof(InkvecException).IsAssignableFrom(typeof(InkvecInternalException)));
    }

    [Fact]
    public void NullImage_ThrowsArgumentNullException()
    {
        Assert.Throws<ArgumentNullException>(() => Inkvec.Trace((byte[])null!));
        Assert.Throws<ArgumentNullException>(() => Inkvec.TraceFile(null!));
        Assert.Throws<ArgumentNullException>(() => Inkvec.Trace((Stream)null!));
    }

    [Fact]
    public void NonPositiveDimensions_ThrowArgumentOutOfRangeException()
    {
        var rgba = WhiteOnClearRgba;
        Assert.Throws<ArgumentOutOfRangeException>(() => Inkvec.TraceRgba(rgba, 0, 64));
        Assert.Throws<ArgumentOutOfRangeException>(() => Inkvec.TraceRgba(rgba, 64, -1));
    }
}
