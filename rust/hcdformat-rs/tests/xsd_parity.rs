//! XSD-validation PARITY harness (feature `xsd`).
//!
//! The pure-Rust [`hcdformat::validate_xsd`] (uppsala over the embedded frozen `hcdf.xsd`) must produce
//! the FROZEN accept/reject verdict on the whole corpus. The verdicts were captured from the (now
//! removed) lxml oracle and are committed as expectations, so the gate runs with NO Python: the Rust
//! validator is the source of truth. Any future uppsala change that flips a verdict fails this gate.
//!
//! This is also the Rust home of the hcdf.xsd accept/reject matrix the retired `tests/run_tests.py`
//! owned: every `examples/*.hcdf` + `tests/valid/*.hcdf` must ACCEPT and every `tests/invalid/*.hcdf`
//! must REJECT (its `.expected` companions carried lxml-specific message substrings that uppsala does not
//! reproduce; the reject VERDICT is the load-bearing check and is asserted here).
//!
//! Corpus:
//!   * ACCEPT: every `examples/*.hcdf` + `tests/valid/*.hcdf` + the openarm_v20 converted example.
//!   * REJECT: `tests/invalid/*.hcdf` + the two schema-SHAPE fixtures (`tests/xsd_fixtures/*`): a `<box>`
//!     without a `<size>` child and a `<color>` with an `<rgba>` child instead of the `rgba=` attribute,
//!     the exact bugs the typed parse silently swallows and only XSD validation catches.
#![cfg(feature = "xsd")]

use hcdformat::validate_xsd;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // crate is at <repo>/rust/hcdformat-rs; the corpus lives under <repo>.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// True iff `validate_xsd` ACCEPTS this XML (no schema issues).
fn rust_accepts(xml: &str) -> bool {
    validate_xsd(xml).is_empty()
}

/// One corpus entry: an absolute path + the FROZEN expected verdict (captured from the removed lxml oracle).
struct Case {
    path: PathBuf,
    expect_accept: bool,
}

/// Build the corpus. Files that are not reachable (e.g. the openarm example from a sibling repo, or a
/// packaged crate tarball with no `examples/`) are skipped, so the gate degrades cleanly.
fn corpus() -> Vec<Case> {
    let root = repo_root();
    let mut cases = Vec::new();
    fn push_if(cases: &mut Vec<Case>, p: PathBuf, accept: bool) {
        if p.is_file() {
            cases.push(Case {
                path: p,
                expect_accept: accept,
            });
        }
    }
    // Push every `*.hcdf` directly under `dir` with the given verdict (order is irrelevant here).
    fn push_hcdf_dir(cases: &mut Vec<Case>, dir: PathBuf, accept: bool) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("hcdf") {
                    cases.push(Case {
                        path: p,
                        expect_accept: accept,
                    });
                }
            }
        }
    }
    // ACCEPT: the whole first-party valid corpus the retired `tests/run_tests.py` validated: every
    // `examples/*.hcdf` and `tests/valid/*.hcdf`.
    push_hcdf_dir(&mut cases, root.join("examples"), true);
    push_hcdf_dir(&mut cases, root.join("tests/valid"), true);
    // ACCEPT: the openarm_v20 converted example (the `~/git/hcdf-conversion-examples` corpus
    // checkout; skipped if absent).
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        push_if(
            &mut cases,
            home.join("git/hcdf-conversion-examples/openarm/openarm_v20.hcdf"),
            true,
        );
    }
    // REJECT: the XSD-invalid corpus.
    push_hcdf_dir(&mut cases, root.join("tests/invalid"), false);
    // REJECT: the two schema-SHAPE fixtures.
    push_if(
        &mut cases,
        root.join("rust/hcdformat-rs/tests/xsd_fixtures/shape-box-nosize.hcdf"),
        false,
    );
    push_if(
        &mut cases,
        root.join("rust/hcdformat-rs/tests/xsd_fixtures/shape-color-rgba-child.hcdf"),
        false,
    );
    cases
}

#[test]
fn validate_xsd_matches_frozen_verdicts() {
    let cases = corpus();
    assert!(
        cases.len() >= 24,
        "expected to exercise the full XSD corpus, found only {} reachable files",
        cases.len()
    );
    let mut accepts = 0;
    let mut rejects = 0;
    for case in &cases {
        let name = case
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let xml = std::fs::read_to_string(&case.path).unwrap();
        let rust = rust_accepts(&xml);

        // Rust vs the FROZEN expected verdict (captured from the removed lxml oracle; Rust is the truth).
        assert_eq!(
            rust,
            case.expect_accept,
            "{name}: validate_xsd verdict {} != expected {}; issues: {:?}",
            verdict(rust),
            verdict(case.expect_accept),
            validate_xsd(&xml),
        );

        if case.expect_accept {
            accepts += 1;
        } else {
            rejects += 1;
        }
    }
    assert!(
        accepts >= 15 && rejects >= 10,
        "corpus skew: {accepts} accept / {rejects} reject"
    );
    eprintln!("xsd parity: {accepts} ACCEPT / {rejects} REJECT (frozen verdicts)");
}

/// THE schema-shape bug #1, a `<box>` with raw text where a `<size>` CHILD is required, is REJECTED,
/// and the correctly-formed box (with a `<size>` child) is ACCEPTED. This is the case the typed parse
/// silently swallows (it deserializes to an empty `<box/>`), the whole reason for XSD validation.
#[test]
fn box_shape_reject_and_accept() {
    let bad = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base">
    <visual name="v"><geometry><box>.1 .1 .5</box></geometry></visual>
  </comp>
</hcdf>
"#;
    let good = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base">
    <visual name="v"><geometry><box><size>0.1 0.1 0.5</size></box></geometry></visual>
  </comp>
</hcdf>
"#;
    assert!(
        !rust_accepts(bad),
        "<box> without <size> child must be rejected"
    );
    assert!(
        rust_accepts(good),
        "<box><size>…</size></box> must be accepted: {:?}",
        validate_xsd(good)
    );
}

/// THE schema-shape bug #2, a top-level `<color>` with an `<rgba>` CHILD where the `rgba=` ATTRIBUTE is
/// required (the `color` complexType is attributes-only), is REJECTED, and the correctly-formed color
/// (with `rgba=`) is ACCEPTED. The typed parse silently swallows the malformed form (empty `<color/>`).
#[test]
fn color_shape_reject_and_accept() {
    let bad = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base"/>
  <color name="rubber"><rgba>0.1 0.1 0.1 1.0</rgba></color>
</hcdf>
"#;
    let good = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base"/>
  <color name="rubber" rgba="0.1 0.1 0.1 1.0"/>
</hcdf>
"#;
    assert!(
        !rust_accepts(bad),
        "<color> with <rgba> child must be rejected"
    );
    assert!(
        rust_accepts(good),
        "<color rgba=…> must be accepted: {:?}",
        validate_xsd(good)
    );
}

fn verdict(accept: bool) -> &'static str {
    if accept {
        "ACCEPT"
    } else {
        "REJECT"
    }
}
