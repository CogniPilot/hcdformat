//! Reader-side version gate (docs/design/versioning.md): `Hcdf::from_xml_str` reads an absent/empty
//! `@version` as 1.0 and accepts a same-MAJOR MINOR when its shape is understood, while rejecting a
//! malformed or foreign-MAJOR value with `Error::UnsupportedVersion`. The connectivity shape gate
//! remains strict at every version so retired or unknown core vocabulary cannot disappear silently.
//! The pre-standard dendrite fragments carry
//! bogus `version="2.0"`/`"1.2"` strings and were never valid HCDF; the gate is what stops them
//! from loading silently (raw XSD validation passes them: the version pattern checks shape only).
use hcdformat::{Error, Hcdf};

#[test]
fn foreign_major_rejects_before_structural_parse() {
    // Modeled on a pre-standard dendrite fragment: bogus version="2.0", no root @name, legacy
    // text-content pose. The gate rejects it by VERSION (not by shape), so the message says why.
    let legacy = r#"<?xml version="1.0"?>
<hcdf version="2.0">
  <comp name="flex-io-assembly" role="sensor">
    <visual name="flex-io_board">
      <pose>0 0 -0.0008 1.5708 0 1.5708</pose>
      <model href="models/95e587b1-flex_io.glb"/>
    </visual>
  </comp>
</hcdf>"#;
    let err = Hcdf::from_xml_str(legacy).expect_err("bogus version=2.0 must not load");
    assert!(
        matches!(&err, Error::UnsupportedVersion { found, .. } if found == "2.0"),
        "expected UnsupportedVersion, got {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("\"2.0\"") && msg.contains("1.0"),
        "message must name the found and the supported version: {msg}"
    );
}

#[test]
fn same_major_minor_loads_forward_compat() {
    // A hypothetical 1.3 document loads when every authored element is understood by this reader.
    let doc = Hcdf::from_xml_str(r#"<hcdf name="future" version="1.3"><comp name="a"/></hcdf>"#)
        .expect("same-MAJOR newer MINOR with known shape loads");
    assert_eq!(doc.version, "1.3");
    assert_eq!(doc.comp.len(), 1);
}

#[test]
fn future_minor_does_not_disable_the_core_shape_gate() {
    let err = Hcdf::from_xml_str(
        r#"<hcdf name="future" version="1.3"><comp name="a"/><new-in-1-3 knob="7"/></hcdf>"#,
    )
    .expect_err("unknown core vocabulary must not disappear silently");
    assert!(
        matches!(&err, Error::Xml(message) if message.contains("<new-in-1-3>")),
        "expected a precise shape error, got {err:?}"
    );
}

#[test]
fn malformed_version_rejects() {
    let err = Hcdf::from_xml_str(r#"<hcdf name="bad" version="nope"><comp name="a"/></hcdf>"#)
        .expect_err("malformed @version must not load");
    assert!(
        matches!(&err, Error::UnsupportedVersion { found, .. } if found == "nope"),
        "expected UnsupportedVersion, got {err:?}"
    );
}

#[test]
fn missing_version_loads_as_1_0() {
    // Python-produced HCDF omits @version; it reads as 1.0 (presence-preserving: stays empty).
    let doc = Hcdf::from_xml_str(r#"<hcdf name="plain"><comp name="a"/></hcdf>"#)
        .expect("absent @version loads");
    assert_eq!(doc.version, "");
    assert_eq!(doc.comp.len(), 1);
}
