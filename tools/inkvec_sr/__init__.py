"""An optional super-resolution pre-pass for inkvec.

The x4 logo upscaler removes about two thirds of a JPEG's error energy, which
turns inkvec's worst input condition into its best. It is a mode rather than a
default because on undamaged input it is three times worse than tracing
directly. The measurements are summarised in the `clean` module docstring.
"""
from __future__ import annotations

__all__ = ["clean", "model", "scan"]
