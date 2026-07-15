//! Conformance gate over the CANONICAL first-party example corpus: the repo's `examples/*.hcdf`, the
//! same files the retired `tests/run_tests.py` round-tripped (now owned by this gate + the XML↔JSON
//! round-trip in `tests/json_convert.rs`). Read at runtime from the repo so there is ONE source of truth
//! and no duplicated fixtures; the model must parse, structurally validate, and round-trip its modeled
//! content for every file, with `<extension>` lax bodies preserved verbatim.
//!
//! Skips cleanly if `examples/` is not reachable (e.g. running from a packaged crate tarball).
//! Converted-robot examples (openarm, so_arm) are not duplicated here; their canonical home is the
//! `hcdf-conversion-examples` repo, which is the right place to test that converter output round-trips.
use hcdformat::Hcdf;
use std::path::PathBuf;

fn examples_dir() -> Option<PathBuf> {
    // crate is at <repo>/rust/hcdformat-rs; the canonical corpus is <repo>/examples
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    d.is_dir().then_some(d)
}

fn read_example(name: &str) -> Option<String> {
    examples_dir().and_then(|d| std::fs::read_to_string(d.join(name)).ok())
}

#[test]
fn corpus_parses_validates_and_roundtrips() {
    let Some(dir) = examples_dir() else {
        eprintln!("skipping: examples/ not reachable (packaged crate)");
        return;
    };
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("read examples dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("hcdf") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let xml = std::fs::read_to_string(&path).unwrap();
        let doc = Hcdf::from_xml_str(&xml).unwrap_or_else(|e| panic!("{name}: parse failed: {e}"));
        if let Err(issues) = hcdformat::validate::validate_structural(&doc) {
            panic!("{name}: structural validation failed: {issues:?}");
        }
        let out = doc
            .to_xml_string()
            .unwrap_or_else(|e| panic!("{name}: serialize failed: {e}"));
        let doc2 =
            Hcdf::from_xml_str(&out).unwrap_or_else(|e| panic!("{name}: re-parse failed: {e}"));
        assert_eq!(doc, doc2, "{name}: modeled content did not round-trip");
        assert!(!doc.comp.is_empty(), "{name}: expected at least one <comp>");
        // Every first-party example carries XML comments (license banners, section markers) that
        // must survive serialization, not be silently dropped by serde. Positional fidelity and
        // byte idempotency are pinned by the src/comments.rs tests and the ser.rs corpus gate.
        if xml.contains("<!--") {
            assert!(
                out.contains("<!--"),
                "{name}: XML comments dropped on round-trip"
            );
        }
        count += 1;
    }
    assert!(
        count >= 3,
        "expected to exercise the example corpus, found {count} files"
    );
}

#[test]
fn extension_bodies_preserved_verbatim() {
    // A lax xs:any extension body must survive a round-trip, not be silently dropped by serde.
    let xml = r#"<hcdf version="1.0" name="extension-roundtrip">
  <extension domain="org.gazebosim" version="1.0">
    <plugin name="aerodynamics" filename="libAerodynamicsPlugin.so">
      <gain>1.25</gain>
    </plugin>
  </extension>
</hcdf>"#;
    let doc = Hcdf::from_xml_str(xml).unwrap();
    let out = doc.to_xml_string().unwrap();
    assert!(
        out.contains("org.gazebosim"),
        "extension domain dropped on round-trip"
    );
    assert!(
        out.contains("libAerodynamicsPlugin.so"),
        "extension lax body (nested <plugin>) dropped on round-trip"
    );
}

#[test]
fn kinematics_typed() {
    let Some(xml) = read_example("humanoid-mobile-base.hcdf") else {
        return;
    };
    let doc = Hcdf::from_xml_str(&xml).unwrap();
    assert!(!doc.joint.is_empty(), "humanoid joints were not parsed");
}
