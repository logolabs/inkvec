//! The committed `bindings/options.schema.json` must be exactly what [`inkvec::Options`]
//! generates. Every language binding is generated from that file, so a stale copy would
//! ship bindings that disagree with the library.

use std::path::PathBuf;

#[test]
fn the_committed_options_schema_matches_the_code() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bindings/options.schema.json");
    let generated = inkvec::options_schema_json();
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if committed != generated {
        std::fs::write(&path, generated).expect("write bindings/options.schema.json");
        panic!(
            "bindings/options.schema.json did not match inkvec::Options and has been \
             regenerated. Next: `python bindings/codegen/generate.py` to regenerate the \
             language stubs from it, review `git diff bindings crates/inkvec-py`, and commit. \
             Re-running `cargo test -p inkvec --test schema` now passes."
        );
    }
}
