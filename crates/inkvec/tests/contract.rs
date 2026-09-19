//! The cross-language contract, from the side that defines it.
//!
//! `bindings/contract/cases.json` lists inputs and options. For each case, `expect` holds
//! what every build must report (the size, or the kind of error), `svg` holds the SVG's
//! length and SHA-256 per build target (see `inkvec::build_target`), and `same_svg_as` names
//! a case whose SVG must be identical. Every language binding runs the same cases.
//!
//! After a deliberate change to the tracer's output:
//!
//! ```text
//! INKVEC_BLESS=1 cargo test -p inkvec --release --test contract
//! ```
//!
//! rewrites `expect` and this target's hashes and drops every other target's, which are
//! stale by definition. On another platform, with the output unchanged,
//! `INKVEC_BLESS=add` records that target's hashes beside the others. A target with no
//! recorded hashes is checked on everything else and reported; set
//! `INKVEC_CONTRACT_REQUIRE_HASH=1` to make that a failure.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const REGENERATE: &str = "INKVEC_BLESS=1 cargo test -p inkvec --release --test contract";

fn contract_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bindings/contract")
}

/// A case's result: the platform-independent `expect` block and, on success, the SVG.
fn run_case(dir: &Path, case: &Value) -> (Value, Option<String>) {
    let input = case["input"].as_str().expect("case.input");
    let bytes = std::fs::read(dir.join(input)).unwrap_or_else(|e| panic!("{input}: {e}"));
    let result =
        inkvec::Options::from_json(&case["options"].to_string()).and_then(|opts| {
            match case["form"].as_str() {
                Some("encoded") => inkvec::trace(&bytes, &opts),
                Some("rgba") => {
                    let dim =
                        |k: &str| case[k].as_u64().expect("rgba case needs width/height") as u32;
                    inkvec::trace_rgba(&bytes, dim("width"), dim("height"), &opts)
                }
                other => panic!("unknown form {other:?}"),
            }
        });
    match result {
        Ok(t) => (json!({ "width": t.width, "height": t.height }), Some(t.svg)),
        Err(e) => (json!({ "error": e.code() }), None),
    }
}

fn digest(svg: &str) -> Value {
    let hash: String = Sha256::digest(svg.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    json!({ "bytes": svg.len(), "sha256": hash })
}

#[test]
fn the_facade_reproduces_the_contract() {
    let dir = contract_dir();
    let path = dir.join("cases.json");
    let text = std::fs::read_to_string(&path).expect("read cases.json");
    let mut doc: Value = serde_json::from_str(&text).expect("parse cases.json");
    let mode = std::env::var("INKVEC_BLESS").unwrap_or_default();
    let require_hash = std::env::var("INKVEC_CONTRACT_REQUIRE_HASH").is_ok_and(|v| v == "1");
    let target = inkvec::build_target();

    let mut failures = Vec::new();
    let mut unhashed = Vec::new();
    let mut svgs: HashMap<String, String> = HashMap::new();
    for case in doc["cases"].as_array_mut().expect("cases") {
        let name = case["name"].as_str().expect("case.name").to_string();
        let (expect, svg) = run_case(&dir, case);
        let got_digest = svg.as_deref().map(digest);
        match mode.as_str() {
            "1" => {
                case["expect"] = expect;
                let obj = case.as_object_mut().expect("case object");
                match &got_digest {
                    Some(d) => {
                        let mut per_target = Map::new();
                        per_target.insert(target.to_string(), d.clone());
                        obj.insert("svg".into(), Value::Object(per_target));
                    }
                    None => {
                        obj.remove("svg");
                    }
                }
            }
            "add" => {
                if case["expect"] != expect {
                    failures.push(format!(
                        "{name}: expect {} but got {expect}; the output changed, use INKVEC_BLESS=1",
                        case["expect"]
                    ));
                }
                if let Some(d) = &got_digest {
                    let obj = case.as_object_mut().expect("case object");
                    let per_target = obj.entry("svg").or_insert_with(|| json!({}));
                    per_target[target] = d.clone();
                }
            }
            _ => {
                if case["expect"] != expect {
                    failures.push(format!("{name}: expected {}, got {expect}", case["expect"]));
                }
                match (case["svg"].get(target), &got_digest) {
                    (Some(want), Some(got)) if want != got => {
                        failures.push(format!("{name} on {target}: expected {want}, got {got}"))
                    }
                    (None, Some(got)) => unhashed.push(format!("{name}: {got}")),
                    _ => {}
                }
            }
        }
        if let Some(s) = svg {
            svgs.insert(name, s);
        }
    }

    // `same_svg_as` holds on every target, recorded hashes or not.
    for case in doc["cases"].as_array().expect("cases") {
        if let Some(other) = case["same_svg_as"].as_str() {
            let name = case["name"].as_str().unwrap_or("?");
            if svgs.get(name) != svgs.get(other) || !svgs.contains_key(other) {
                failures.push(format!("{name}: SVG differs from {other}'s"));
            }
        }
    }

    if mode == "1" || mode == "add" {
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        let mut out = serde_json::to_string_pretty(&doc).expect("serialise");
        out.push('\n');
        std::fs::write(&path, out).expect("write cases.json");
        return;
    }
    if !unhashed.is_empty() {
        let msg = format!(
            "no SVG hashes recorded for build target {target}; checked everything else. \
             Record them with INKVEC_BLESS=add. This target's hashes:\n  {}",
            unhashed.join("\n  ")
        );
        if require_hash {
            failures.push(msg);
        } else {
            eprintln!("{msg}");
        }
    }
    assert!(
        failures.is_empty(),
        "{} contract failure(s):\n  {}\nIf the change to the output is intended, regenerate the \
         expectations with `{REGENERATE}` and commit bindings/contract/cases.json.",
        failures.len(),
        failures.join("\n  ")
    );
}
