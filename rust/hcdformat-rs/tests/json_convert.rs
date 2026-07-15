//! HCDF XML <-> JSON converter gate (feature `json`).
//!
//! Two guarantees:
//!   1. ROUND-TRIP: for every first-party corpus document, `xml -> json -> xml -> json` reaches a JSON
//!      fixpoint (semantic identity), and the regenerated XML is XSD-VALID (uppsala, under feature `xsd`).
//!   2. FROZEN OUTPUT: the Rust `hcdf_xml_to_json` output is BYTE-IDENTICAL to a committed golden JSON
//!      per corpus document. The goldens were captured from the Rust converter (which was proven
//!      byte-identical to the removed `hcdf/convert.py`), so they pin the exact JSON projection with NO
//!      Python in the test path. Regenerate with `HCDF_REGEN_GOLDENS=1 cargo test`.
#![cfg(feature = "json")]

use std::path::PathBuf;

/// The repo root: this crate lives at `<repo>/rust/hcdformat-rs`, the corpus under `<repo>`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The crate-local frozen-golden directory (`<crate>/tests/golden`).
fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Compare `actual` to the committed golden at `tests/golden/<rel>`; when `HCDF_REGEN_GOLDENS` is set,
/// (re)write the golden instead of asserting. The Rust output IS the canonical golden.
fn assert_golden(rel: &str, actual: &str) {
    let path = golden_dir().join(rel);
    if std::env::var_os("HCDF_REGEN_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden {} ({e}); regenerate with HCDF_REGEN_GOLDENS=1",
            path.display()
        )
    });
    assert_eq!(actual, expected, "golden mismatch for {}", path.display());
}

/// The reachable first-party `.hcdf` corpus (`examples/`, `tests/valid/`, `tests/hcdf/`), sorted for a
/// stable order. Degrades to empty if the tree is not reachable (e.g. a packaged crate tarball).
fn corpus() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ["examples", "tests/valid", "tests/hcdf"] {
        if let Ok(rd) = std::fs::read_dir(root.join(dir)) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("hcdf") {
                    files.push(p);
                }
            }
        }
    }
    files.sort();
    files
}

#[test]
fn xml_json_roundtrip_is_a_fixpoint_and_the_regenerated_xml_is_valid() {
    let files = corpus();
    assert!(
        !files.is_empty(),
        "the first-party corpus must be reachable for the round-trip gate"
    );
    for p in &files {
        let d = p.display();
        let xml = std::fs::read_to_string(p).unwrap();
        let json1 = hcdformat::hcdf_xml_to_json(&xml)
            .unwrap_or_else(|e| panic!("{d}: xml -> json failed: {e}"));
        let xml2 = hcdformat::json_to_hcdf_xml(&json1)
            .unwrap_or_else(|e| panic!("{d}: json -> xml failed: {e}"));
        let json2 = hcdformat::hcdf_xml_to_json(&xml2)
            .unwrap_or_else(|e| panic!("{d}: xml2 -> json failed: {e}"));
        assert_eq!(
            json1, json2,
            "{d}: xml -> json -> xml must reach a JSON fixpoint"
        );

        // The regenerated XML must satisfy the real schema shape (uppsala == lxml `validate --xsd`).
        #[cfg(feature = "xsd")]
        {
            let issues = hcdformat::validate_xsd(&xml2);
            assert!(
                issues.is_empty(),
                "{d}: json -> xml output must be XSD-valid; got {issues:?}"
            );
        }
    }
}

/// A stable golden key for a corpus file: `<parent-dir>__<stem>.json` (the parent disambiguates the same
/// stem appearing under `examples/` vs `tests/valid/` etc.).
fn golden_key(p: &std::path::Path) -> String {
    let stem = p.file_stem().unwrap().to_string_lossy();
    let parent = p
        .parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("json/{parent}__{stem}.json")
}

#[test]
fn to_json_is_byte_identical_to_frozen_golden() {
    let files = corpus();
    assert!(
        !files.is_empty(),
        "expected at least one corpus doc for the frozen golden check"
    );
    for p in &files {
        let d = p.display();
        let xml = std::fs::read_to_string(p).unwrap();
        let rs_json = hcdformat::hcdf_xml_to_json(&xml)
            .unwrap_or_else(|e| panic!("{d}: xml -> json failed: {e}"));
        assert_golden(&golden_key(p), &rs_json);
    }
}
