// docs/BINDINGS.md, "Threading": every entry point may be called from any number of
// threads at once, and (docs/BINDINGS.md, "Determinism") the same input and options give
// byte-identical output across thread counts. This is a smoke test, not a stress test: it
// is here to catch a binding-level threading bug (missing synchronization around the lazy
// static strings, a shared buffer reused across calls), not to characterise the native
// library's own concurrency, which crates/inkvec's own tests own.

using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading.Tasks;
using LogoLabs.Inkvec;
using Xunit;

namespace LogoLabs.Inkvec.Tests;

public class ConcurrencyTests
{
    private static readonly string ContractDir = Path.Combine(AppContext.BaseDirectory, "contract");

    [Fact]
    public async Task ParallelTraces_OfTheSameInput_AllSucceedAndAgree()
    {
        var bytes = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        var expected = Inkvec.Trace(bytes).Svg;

        var tasks = Enumerable.Range(0, 32).Select(_ => Task.Run(() => Inkvec.Trace(bytes).Svg)).ToArray();
        var results = await Task.WhenAll(tasks);

        Assert.All(results, svg => Assert.Equal(expected, svg));
    }

    [Fact]
    public async Task ParallelTraces_OfDifferentInputs_DoNotCrossContaminate()
    {
        var tiny = File.ReadAllBytes(Path.Combine(ContractDir, "tiny.png"));
        var clear = File.ReadAllBytes(Path.Combine(ContractDir, "white_on_clear.png"));
        var expectedTiny = Inkvec.Trace(tiny).Svg;
        var expectedClear = Inkvec.Trace(clear).Svg;

        var tasks = Enumerable.Range(0, 40)
            .Select(i => Task.Run(() => (i, svg: (i % 2 == 0 ? Inkvec.Trace(tiny) : Inkvec.Trace(clear)).Svg)))
            .ToArray();
        var results = await Task.WhenAll(tasks);

        foreach (var (i, svg) in results)
        {
            Assert.Equal(i % 2 == 0 ? expectedTiny : expectedClear, svg);
        }
    }

    [Fact]
    public async Task ParallelStaticReads_AllAgree()
    {
        var tasks = Enumerable.Range(0, 16).Select(_ => Task.Run(() => (
            Inkvec.Version, Inkvec.BuildTarget, Inkvec.OptionsSchema, Inkvec.Defaults))).ToArray();
        var results = await Task.WhenAll(tasks);
        var first = results[0];
        Assert.All(results, r => Assert.Equal(first, r));
    }
}
