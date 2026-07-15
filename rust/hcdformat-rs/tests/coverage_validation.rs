//! Frozen-oracle harness for the schema-coverage validators (`validate_coverage`).
//!
//! These validators (self-collision pair integrity, transmission endpoint resolution, motor sensor
//! refs, model selection, and transmission leaf-name integrity) are a Rust-only contract; the prior oracle was
//! retired before this surface existed. So, mirroring the `tests/validator/loop` + `tests/golden/
//! _loop_oracle` pattern, each hand-authored fixture under `tests/validator/coverage/<name>.hcdf` is
//! paired with a frozen `tests/golden/_coverage_oracle/<name>.oracle` line: the sorted `level:code,...`
//! set `validate_coverage` must emit (empty for a positive/"allow" fixture). A `cov-ok-*` fixture
//! proves an allow; a `cov-bad-*`/`cov-warn-*` fixture pins one error/warning code.
//!
//! Set `HCDF_REGEN_COVERAGE_ORACLE=1` to (re)write the oracle files from the current output; then the
//! diff is reviewed and committed deliberately, exactly like the other frozen goldens.

use hcdformat::{validate_coverage, Hcdf};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The sorted `level:code` set `validate_coverage` emits for `doc`.
fn coverage_codes(doc: &Hcdf) -> String {
    let mut codes: Vec<String> = validate_coverage(doc)
        .iter()
        .map(|i| format!("{}:{}", i.level.as_str(), i.code))
        .collect();
    codes.sort();
    codes.join(",")
}

/// Every first-party example corpus document must parse AND carry zero schema-coverage ERRORS
/// (warnings, e.g. a duplicate self-collision pair, are allowed). This is the real-world positive
/// coverage: a correct validator is silent (of errors)
/// on the valid corpus, whose transmissions, motor sensor refs, and self-collision pairs all resolve.
#[test]
fn example_corpus_has_no_coverage_errors() {
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
        let errors: Vec<_> = validate_coverage(&doc)
            .into_iter()
            .filter(|i| i.level == hcdformat::Level::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "{name}: valid corpus must have no schema-coverage errors, got {errors:?}"
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
fn coverage_fixtures_match_frozen_oracle() {
    let regen = std::env::var("HCDF_REGEN_COVERAGE_ORACLE").is_ok();
    let fixtures = repo_root().join("tests/validator/coverage");
    let oracle_dir = repo_root().join("tests/golden/_coverage_oracle");
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
        let got = coverage_codes(&doc);
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
                "{stem}: validate_coverage code-set != frozen oracle"
            );
            // A `cov-ok-*` fixture must be genuinely clean; a `cov-bad-*`/`cov-warn-*` must not be.
            if stem.starts_with("cov-ok-") {
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
        checked >= 8,
        "expected the coverage fixture set, checked {checked}"
    );
}

/// Every coverage fixture (`cov-ok-*`, `cov-bad-*`, AND `cov-warn-*`) must be XSD-SHAPE-valid.
///
/// These coverage validators exist precisely to catch the SEMANTIC defects the schema cannot express
/// (dangling pair/transmission/sensor refs and invalid model selection). A `cov-bad-*` fixture is
/// "bad" only at that semantic layer; its XML must still satisfy `hcdf.xsd`. If a fixture were itself
/// schema-INVALID, the XSD gate (and dendrite's Apply gate, which runs the XSD layer FIRST) would
/// reject it before `validate_coverage` ever ran, so it would never actually exercise the coverage
/// validator against otherwise-valid input. (`Hcdf::from_xml_str` is a lenient serde parse that ignores
/// missing required attributes, so the oracle harness above cannot see such a defect; this gate does.)
#[cfg(feature = "xsd")]
#[test]
fn coverage_fixtures_are_xsd_valid() {
    let fixtures = repo_root().join("tests/validator/coverage");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&fixtures)
        .expect("read fixtures dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("hcdf"))
        .collect();
    entries.sort();
    let mut checked = 0usize;
    for path in entries {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let xml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{stem}: read: {e}"));
        let issues = hcdformat::validate_xsd(&xml);
        assert!(
            issues.is_empty(),
            "{stem}: coverage fixture must be XSD-shape-valid (the coverage validators catch semantics, \
             not schema shape), got {issues:?}"
        );
        checked += 1;
    }
    assert!(
        checked >= 8,
        "expected the coverage fixture set, checked {checked}"
    );
}

/// CLEAN-1.0 NO-LEGACY: a dot is NOT a reference separator in a transmission motor ref either. A dotted
/// "case.m1" splits to nothing on the last slash, falls to the bare path, matches no motor, and dangles
/// as E_TRANS_MOTOR_REF, with the shared separator hint appended so the author is told to use '/'. This
/// pins the "hint, not acceptance" contract for the transmission ref family.
#[test]
fn dotted_transmission_ref_carries_separator_hint() {
    let path = repo_root().join("tests/validator/coverage/cov-bad-trans-motor-dotted.hcdf");
    let xml = std::fs::read_to_string(&path).expect("read dotted-ref fixture");
    let doc = Hcdf::from_xml_str(&xml).expect("fixture must parse");
    let issues = validate_coverage(&doc);
    let hit = issues
        .iter()
        .find(|i| i.code == "E_TRANS_MOTOR_REF")
        .unwrap_or_else(|| panic!("a dotted motor ref must dangle, got {issues:?}"));
    assert!(
        hit.message
            .contains("('.' is not a reference separator - use '/')"),
        "the dangling-ref message must carry the separator hint, got: {}",
        hit.message
    );
}
