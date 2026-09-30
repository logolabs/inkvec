//! Diagnostics: the one place the library writes to standard error.
//!
//! The command line narrates a trace one line per stage on standard error (`--quiet`
//! silences it), and a few stages can dump developer detail when an `INKVEC_*` variable
//! asks for it (see `docs/internal/env-vars.md`). Neither is part of the result: the SVG
//! and the report lines in [`crate::Traced::stats`] are. Routing every such line through
//! here keeps the rule in one place -- stage lines obey `quiet`, dumps obey their flag,
//! warnings always print -- and gives an embedding that has no terminal (the browser build,
//! the desktop app) a single seam to look at. The line is only formatted when it will be
//! printed.

/// A stage line of the command line's narrative, unless `quiet`.
pub(crate) fn stage(quiet: bool, line: impl FnOnce() -> String) {
    if !quiet {
        eprintln!("{}", line());
    }
}

/// A developer dump, when `on` (normally an `inkvec_core::env::flag` the caller read).
pub(crate) fn debug(on: bool, line: impl FnOnce() -> String) {
    if on {
        eprintln!("{}", line());
    }
}

/// A warning the user should see whatever `--quiet` says: something they asked for did
/// not happen.
pub(crate) fn warn(line: impl FnOnce() -> String) {
    eprintln!("{}", line());
}
