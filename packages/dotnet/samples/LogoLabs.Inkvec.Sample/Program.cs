// A tiny sample:  dotnet run --project packages/dotnet/samples/LogoLabs.Inkvec.Sample -- <in.png> <out.svg>

using LogoLabs.Inkvec;

if (args.Length != 2)
{
    Console.Error.WriteLine("usage: LogoLabs.Inkvec.Sample <input image> <output.svg>");
    return 1;
}

var (input, output) = (args[0], args[1]);

Console.WriteLine($"inkvec {Inkvec.Version} ({Inkvec.BuildTarget})");

try
{
    var traced = Inkvec.TraceFile(input, new InkvecOptions { Colors = 16 });
    File.WriteAllText(output, traced.Svg);
    Console.WriteLine($"{input} ({traced.Width}x{traced.Height}) -> {output} ({traced.Svg.Length} chars)");
    return 0;
}
catch (InvalidImageException e)
{
    Console.Error.WriteLine($"not a decodable image: {e.Message}");
    return 2;
}
catch (InkvecException e)
{
    Console.Error.WriteLine($"inkvec: {e.Message}");
    return 1;
}
