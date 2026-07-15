//! Frozen-oracle harness for the loop-closure validators (`validate_loops`).
//!
//! These validators (predecessor/successor referential integrity, `<constraint-axes>` mask shape, and
//! predecessor/successor-vs-parent/child body agreement) are a Rust-only contract. Each hand-authored
//! fixture under `tests/validator/loop/<name>.hcdf` is paired with a frozen
//! `tests/golden/_loop_oracle/<name>.oracle` line: the sorted `level:code,...` set `validate_loops` must
//! emit (empty for a positive/"allow" fixture). A `loop-ok-*` fixture proves an allow; a
//! `loop-bad-*`/`loop-warn-*` fixture pins one error/warning code.
//!
//! Positive coverage over a REAL, geometrically-closed loop lives in the example corpus
//! (`examples/four-bar.hcdf`), held to the zero-loop-error bar by `example_corpus_has_no_loop_errors`
//! below; the `loop-ok-*` fixtures here are the minimal self-contained allows.
//!
//! Set `HCDF_REGEN_LOOP_ORACLE=1` to (re)write the oracle files from the current output, then the diff
//! is inspected and committed deliberately, exactly like the other frozen goldens.

use hcdformat::{validate_loops, Hcdf};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The sorted `level:code` set `validate_loops` emits for `doc`.
fn loop_codes(doc: &Hcdf) -> String {
    let mut codes: Vec<String> = validate_loops(doc)
        .iter()
        .map(|i| format!("{}:{}", i.level.as_str(), i.code))
        .collect();
    codes.sort();
    codes.join(",")
}

/// Every first-party example corpus document must parse AND carry zero loop-closure ERRORS. A document
/// with no `<loop>` at all trivially passes (the validator is silent); the four-bar example is the real
/// positive: a closed parallelogram whose closure joint names existing, matching bodies and a
/// well-formed `<constraint-axes>` mask. This is the corpus-wide positive: a correct validator is silent
/// (of errors) on the valid corpus.
#[test]
fn example_corpus_has_no_loop_errors() {
    // Every first-party `examples/*.hcdf`, discovered from the corpus so a new example is automatically
    // held to the zero-loop-error bar.
    let examples = repo_root().join("examples");
    let mut names: Vec<String> = std::fs::read_dir(&examples)
        .expect("read examples dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("hcdf"))
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut checked = 0usize;
    for name in &names {
        let path = examples.join(name);
        let xml = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let doc = Hcdf::from_xml_str(&xml).unwrap_or_else(|e| panic!("{name}: parse: {e}"));
        let errors: Vec<_> = validate_loops(&doc)
            .into_iter()
            .filter(|i| i.level == hcdformat::Level::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "{name}: valid corpus must have no loop-closure errors, got {errors:?}"
        );
        checked += 1;
    }
    assert!(
        checked >= 4,
        "expected the example corpus, checked {checked}"
    );
}

/// Each fixture reproduces its frozen `.oracle` code-set.
#[test]
fn loop_fixtures_match_frozen_oracle() {
    let regen = std::env::var("HCDF_REGEN_LOOP_ORACLE").is_ok();
    let fixtures = repo_root().join("tests/validator/loop");
    let oracle_dir = repo_root().join("tests/golden/_loop_oracle");
    if regen {
        std::fs::create_dir_all(&oracle_dir).expect("create oracle dir");
    }
    let mut checked = 0usize;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&fixtures)
        .expect("read fixtures dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("hcdf"))
        .collect();
    entries.sort();
    for path in entries {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let xml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{stem}: read: {e}"));
        let doc = Hcdf::from_xml_str(&xml)
            .unwrap_or_else(|e| panic!("{stem}: fixture must parse (XSD-valid input): {e}"));
        let got = loop_codes(&doc);
        let oracle_path = oracle_dir.join(format!("{stem}.oracle"));
        if regen {
            std::fs::write(&oracle_path, format!("{got}\n")).expect("write oracle");
        } else {
            let want = std::fs::read_to_string(&oracle_path)
                .unwrap_or_else(|e| panic!("{stem}: missing oracle {}: {e}", oracle_path.display()))
                .trim_end_matches('\n')
                .to_string();
            assert_eq!(
                got, want,
                "{stem}: validate_loops code-set != frozen oracle"
            );
            // A `loop-ok-*` fixture must be genuinely clean; a `loop-bad-*`/`loop-warn-*` must not be.
            if stem.starts_with("loop-ok-") {
                assert!(
                    want.is_empty(),
                    "{stem}: an ok-fixture must have an empty oracle"
                );
            } else {
                assert!(
                    !want.is_empty(),
                    "{stem}: a bad/warn-fixture must trip a code"
                );
            }
        }
        checked += 1;
    }
    assert!(
        checked >= 4,
        "expected the loop fixture set, checked {checked}"
    );
}
