//! The engine's environment variables, read in one place.
//!
//! Inkvec is configured through `inkvec::Options` and the command line, never through the
//! environment. What is left in the environment is diagnostics (`INKVEC_TIMING`, the
//! `*DBG` traces, the dump files) and a handful of A/B switches that the changelog or a
//! benchmark script uses to price a shipped stage (`INKVEC_BOPT=0`, `INKVEC_NO_CARVE=1`,
//! ...). Experiments read theirs only in a `research` build. `docs/internal/env-vars.md`
//! lists every variable, what it does, and why it is still there.
//!
//! Every read goes through this module so that all of them mean the same thing:
//!
//! - a variable is read **once per process**, the first time it is asked for, and the
//!   answer is kept: a trace never sees a value change halfway through;
//! - a switch is off when unset, empty or `0`, and on for any other value
//!   ([`flag`]); a stage that is on by default is switched off by `0` ([`switch`]);
//! - a number that does not parse, or is not finite, is ignored as if unset.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Mutex;

/// Every variable asked for so far, and what it held when first asked. The values are
/// leaked on purpose: there are a few dozen names at most, each read once, and a
/// `'static` borrow lets a hot caller test a flag without allocating.
static CACHE: Mutex<Vec<(&'static str, Option<&'static OsStr>)>> = Mutex::new(Vec::new());

/// The value of `name`, read from the environment the first time it is asked for.
pub fn raw(name: &'static str) -> Option<&'static OsStr> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, v)) = cache.iter().find(|(n, _)| *n == name) {
        return v;
    }
    let v: Option<&'static OsStr> =
        std::env::var_os(name).map(|s| &*Box::leak(s.into_boxed_os_str()));
    cache.push((name, v));
    v
}

/// The value of `name` as text, if it is set and valid Unicode.
pub fn text(name: &'static str) -> Option<&'static str> {
    raw(name).and_then(OsStr::to_str)
}

/// The value of `name` as a path, if it is set and not empty.
pub fn path(name: &'static str) -> Option<PathBuf> {
    raw(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// An opt-in switch: on for any value except empty and `0`.
pub fn flag(name: &'static str) -> bool {
    parse_switch(raw(name), false)
}

/// A switch with a default: `default` when unset or empty, off for `0`, on otherwise.
pub fn switch(name: &'static str, default: bool) -> bool {
    parse_switch(raw(name), default)
}

/// A finite number, or `None` when unset or unparseable.
pub fn number(name: &'static str) -> Option<f64> {
    parse_number(raw(name))
}

/// A non-negative integer, or `None` when unset or unparseable.
pub fn count(name: &'static str) -> Option<usize> {
    raw(name)?.to_str()?.trim().parse().ok()
}

fn parse_switch(v: Option<&OsStr>, default: bool) -> bool {
    match v {
        None => default,
        Some(v) if v.is_empty() => default,
        Some(v) => v != "0",
    }
}

fn parse_number(v: Option<&OsStr>) -> Option<f64> {
    v?.to_str()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_switch_means_the_same_thing_whatever_its_default() {
        let s = |v: Option<&str>, d| parse_switch(v.map(OsStr::new), d);
        for d in [false, true] {
            assert_eq!(s(None, d), d, "unset keeps the default");
            assert_eq!(s(Some(""), d), d, "empty keeps the default");
            assert!(!s(Some("0"), d), "0 is always off");
            assert!(s(Some("1"), d));
            assert!(s(Some("yes"), d));
        }
    }

    #[test]
    fn a_number_that_is_not_one_reads_as_unset() {
        let n = |v: &str| parse_number(Some(OsStr::new(v)));
        assert_eq!(n("1.5"), Some(1.5));
        assert_eq!(n(" 2 "), Some(2.0));
        assert_eq!(n("abc"), None);
        assert_eq!(n("inf"), None);
        assert_eq!(n("NaN"), None);
        assert_eq!(parse_number(None), None);
    }

    #[test]
    fn a_variable_is_read_once() {
        // Nothing sets this name, so the first read caches `None` and later reads agree.
        let name = "INKVEC_ENV_TEST_NEVER_SET";
        assert!(raw(name).is_none());
        assert!(!flag(name));
        assert!(switch(name, true));
        assert_eq!(number(name), None);
        assert_eq!(count(name), None);
        assert!(path(name).is_none());
        let cache = CACHE.lock().unwrap();
        assert_eq!(cache.iter().filter(|(n, _)| *n == name).count(), 1);
    }
}
