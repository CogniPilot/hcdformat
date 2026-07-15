//! CLI-level guarantees for `hcdf validate` (feature `cli`, native-only), driving the REAL binary:
//!
//!   * `validate --xsd` on a native `.hcdf` validates the RAW input bytes. The typed parse silently
//!     DROPS a legacy text-content `<pose>` (the doc round-trips to a schema-valid re-serialization
//!     while losing every pose), so a re-serialization-validating CLI reported "VALID" on it: the
//!     laundering this suite pins down. Raw uppsala rejects it line-accurately ("Element should have
//!     empty content but contains text").
//!   * A document the reader REJECTS (bogus `@version`) reports as a VALIDATION failure (`E_LOAD` +
//!     INVALID summary, exit 1) on BOTH validate paths, never the generic `error:` CLI path.
#![cfg(feature = "cli")]

use std::path::PathBuf;
use std::process::{Command, Output};

/// A doc that is schema-valid EXCEPT for its legacy text-content `<pose>` (the official 1.0 pose is
/// attribute-based and must have EMPTY content). Version is a readable 1.0 so the reader gate stays
/// out of the way: the typed parse ACCEPTS this file (dropping the pose), only raw XSD catches it.
const TEXT_POSE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="legacy" version="1.0">
  <comp name="flex-io-assembly">
    <visual name="board">
      <pose>0 0 -0.0008 1.5708 0 1.5708</pose>
      <geometry><box><size>0.1 0.1 0.01</size></box></geometry>
    </visual>
  </comp>
</hcdf>
"#;

/// The same doc with the pose in the official attribute form: the positive control.
const ATTR_POSE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="legacy" version="1.0">
  <comp name="flex-io-assembly">
    <visual name="board">
      <pose xyz="0 0 -0.0008" rpy="1.5708 0 1.5708"/>
      <geometry><box><size>0.1 0.1 0.01</size></box></geometry>
    </visual>
  </comp>
</hcdf>
"#;

/// Schema-shape-valid but with a foreign-MAJOR `@version`: raw XSD ACCEPTS it (the version pattern
/// checks MAJOR.MINOR shape only, by design), so the reader-side gate is the ONLY thing rejecting it.
const BOGUS_VERSION: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="future" version="2.0">
  <comp name="base"/>
</hcdf>
"#;

/// Write `xml` as `name` under a per-process temp dir and hand back the path.
fn fixture(name: &str, xml: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hcdf_cli_validate_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, xml).unwrap();
    path
}

/// Run the real `hcdf` binary with `args` and return its output (stderr carries the report).
fn hcdf(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hcdf"))
        .args(args)
        .output()
        .expect("hcdf binary runs")
}

#[test]
fn xsd_rejects_legacy_text_content_pose_on_raw_bytes() {
    let path = fixture("text-pose.hcdf", TEXT_POSE);
    let out = hcdf(&["validate", "--xsd", path.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "a text-content pose must FAIL validate --xsd (raw bytes), got:\n{stderr}"
    );
    // uppsala's precise raw-bytes diagnosis: the official pose element takes attributes only.
    assert!(
        stderr.contains("empty content"),
        "expected the uppsala 'empty content' schema error, got:\n{stderr}"
    );
    assert!(
        stderr.contains("INVALID"),
        "summary must say INVALID, got:\n{stderr}"
    );
}

#[test]
fn xsd_accepts_the_attribute_pose_control() {
    let path = fixture("attr-pose.hcdf", ATTR_POSE);
    let out = hcdf(&["validate", "--xsd", path.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the attribute-pose control must PASS validate --xsd, got:\n{stderr}"
    );
    assert!(
        stderr.contains("VALID: 0 issue(s), 0 error(s)"),
        "expected a clean VALID summary, got:\n{stderr}"
    );
}

#[test]
fn xsd_reports_bogus_version_as_validation_failure() {
    let path = fixture("bogus-version-xsd.hcdf", BOGUS_VERSION);
    let out = hcdf(&["validate", "--xsd", path.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "version=2.0 must FAIL validate --xsd, got:\n{stderr}"
    );
    // The reader gate surfaces as an E_LOAD validation issue + INVALID summary, not `error: ...`.
    assert!(
        stderr.contains("[error] E_LOAD:") && stderr.contains("\"2.0\""),
        "expected E_LOAD naming the bogus version, got:\n{stderr}"
    );
    assert!(
        stderr.contains("INVALID: 1 issue(s), 1 error(s)"),
        "expected the INVALID validation summary, got:\n{stderr}"
    );
}

#[test]
fn plain_validate_reports_bogus_version_as_validation_failure() {
    let path = fixture("bogus-version-plain.hcdf", BOGUS_VERSION);
    let out = hcdf(&["validate", path.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "version=2.0 must FAIL plain validate, got:\n{stderr}"
    );
    assert!(
        stderr.contains("[error] E_LOAD:") && stderr.contains("INVALID: 1 issue(s), 1 error(s)"),
        "expected E_LOAD + the INVALID validation summary, got:\n{stderr}"
    );
}

#[test]
fn plain_validate_still_accepts_a_valid_doc() {
    let path = fixture("attr-pose-plain.hcdf", ATTR_POSE);
    let out = hcdf(&["validate", path.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "the valid control must PASS plain validate, got:\n{stderr}"
    );
    assert!(
        stderr.contains("OK: 0 issue(s), 0 error(s)"),
        "expected the clean OK summary, got:\n{stderr}"
    );
}
