"""The native module behind ``inkvec``. Use the functions in ``inkvec`` instead."""

from inkvec import (
    InkvecError as InkvecError,
    InternalError as InternalError,
    InvalidImageError as InvalidImageError,
    InvalidOptionsError as InvalidOptionsError,
    Traced as Traced,
)

__version__: str

def _trace(data: bytes, options_json: str) -> Traced: ...
def _trace_rgba(data: bytes, width: int, height: int, options_json: str) -> Traced: ...
def _options_schema_json() -> str: ...
def _default_options_json() -> str: ...
def _build_target() -> str: ...
