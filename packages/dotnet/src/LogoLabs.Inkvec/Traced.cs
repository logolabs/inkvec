namespace LogoLabs.Inkvec
{
    /// <summary>
    /// The result of one trace: the SVG document and the input's size in pixels.
    /// </summary>
    public sealed class Traced
    {
        internal Traced(string svg, uint width, uint height)
        {
            Svg = svg;
            Width = width;
            Height = height;
        }

        /// <summary>The SVG document, as UTF-8 decoded text.</summary>
        public string Svg { get; }

        /// <summary>
        /// Width of the input image, in pixels. Matches the SVG's own <c>width</c>
        /// attribute unless <c>margin</c> grew it.
        /// </summary>
        public uint Width { get; }

        /// <summary>Height of the input image, in pixels.</summary>
        public uint Height { get; }

        /// <summary>The SVG document.</summary>
        public override string ToString() => Svg;
    }
}
