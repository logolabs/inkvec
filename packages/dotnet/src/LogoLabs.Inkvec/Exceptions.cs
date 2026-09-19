using System;

namespace LogoLabs.Inkvec
{
    /// <summary>
    /// Base class of every exception the Inkvec binding throws. The message is always the
    /// native library's own, human-readable wording (see docs/BINDINGS.md, "Errors").
    /// </summary>
    public abstract class InkvecException : Exception
    {
        protected InkvecException(string message) : base(message)
        {
        }
    }

    /// <summary>
    /// The input is not an image the tracer can decode, or raw RGBA pixels do not match the
    /// width and height given. C status <c>INKVEC_ERR_INVALID_IMAGE</c>.
    /// </summary>
    public sealed class InvalidImageException : InkvecException
    {
        public InvalidImageException(string message) : base(message)
        {
        }
    }

    /// <summary>
    /// An option is unknown, of the wrong type, or out of range. C status
    /// <c>INKVEC_ERR_INVALID_OPTIONS</c>.
    /// </summary>
    public sealed class InvalidOptionsException : InkvecException
    {
        public InvalidOptionsException(string message) : base(message)
        {
        }
    }

    /// <summary>
    /// The tracer itself failed or panicked -- not the caller's fault, and worth a bug
    /// report. C status <c>INKVEC_ERR_INTERNAL</c>.
    /// </summary>
    public sealed class InkvecInternalException : InkvecException
    {
        public InkvecInternalException(string message) : base(message)
        {
        }
    }
}
