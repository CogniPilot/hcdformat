//! Tests for the generated enums (`src/model/enums.rs`) and the residual enum-value validator.
//!
//! The enums are committed generator output (`hcdf regen enums`, the `hcdformat::xsdgen` module); these tests pin the contracts
//! the typed model relies on: (1) each variant `#[serde(rename)]`s to its exact XSD literal so it
//! round-trips through serde (so a typed `Option<EnumTy>` field serializes byte-identically to the old
//! `Option<String>`), (2) `FromStr`/`Display` convert to/from that literal (so consumers can map a
//! String UI value to/from the enum), and (3) the now-scoped `ENUM_ATTRS` table covers exactly the
//! residual model-untyped slots (sensor-category `@type` including `fluid`, the `lens`/`probe`
//! projection/probe `@type`, and transmission endpoint `joint`/`motor` `@role`) that
//! `validate_enums` enforces by document walk.
use hcdformat::model::enums::{
    valid_values_for, AxisValue, CompRole, EmSensorType, JointType, RoleType, ENUM_ATTRS,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// A minimal serde container so an enum value can round-trip through the same quick-xml engine the
/// model uses (the enum is serialized as an attribute, matching how it appears in real documents).
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct AttrHolder<T> {
    #[serde(rename = "@v")]
    v: T,
}

fn xml_roundtrip<T>(value: T, expect_literal: &str)
where
    T: Copy + Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    let xml = quick_xml::se::to_string(&AttrHolder { v: value }).unwrap();
    assert!(
        xml.contains(&format!(r#"v="{expect_literal}""#)),
        "expected literal {expect_literal:?} in serialized {xml}"
    );
    let back: AttrHolder<T> = quick_xml::de::from_str(&xml).unwrap();
    assert_eq!(
        back.v, value,
        "{expect_literal:?} did not round-trip through serde XML"
    );
}

#[test]
fn variants_serde_rename_to_xsd_literals() {
    // serialize: variant -> exact XSD literal; deserialize: literal -> variant. Covers a plain value,
    // a hyphenated/numeric one, and the negative-axis case that needs a non-colliding variant name.
    xml_roundtrip(CompRole::Sensor, "sensor");
    xml_roundtrip(RoleType::from_str("reference").unwrap(), "reference");
    xml_roundtrip(AxisValue::NegX, "-X");

    // explicit deserialize-from-literal checks (the inverse direction)
    let r: AttrHolder<CompRole> = quick_xml::de::from_str(r#"<AttrHolder v="actuator"/>"#).unwrap();
    assert_eq!(r.v, CompRole::Actuator);
    let a: AttrHolder<AxisValue> = quick_xml::de::from_str(r#"<AttrHolder v="-X"/>"#).unwrap();
    assert_eq!(a.v, AxisValue::NegX);
    // X and -X must be distinct variants (the collision the generator's `Neg` prefix avoids)
    assert_ne!(AxisValue::X, AxisValue::NegX);
}

#[test]
fn from_str_is_the_value_set_membership_test() {
    for v in JointType::valid_values() {
        assert!(
            JointType::from_str(v).is_ok(),
            "valid literal {v:?} must parse"
        );
    }
    assert!(JointType::from_str("bogus").is_err());
    // valid_values is exactly the schema set (sanity on a known enum)
    assert_eq!(
        CompRole::valid_values(),
        &["sensor", "compute", "actuator", "parent"]
    );
}

#[test]
fn display_is_the_inverse_of_from_str() {
    // Display emits the exact XSD literal and round-trips through FromStr, for every variant of a few
    // representative enums (a plain value, a hyphenated/numeric one, a negative-axis one). This is the
    // String <-> enum bridge consumers (hcdviz/dendrite_build) use to map UI/logic strings.
    for set in [
        JointType::valid_values(),
        RoleType::valid_values(),
        AxisValue::valid_values(),
    ] {
        for &lit in set {
            // a literal -> enum -> Display must reproduce the literal exactly.
            let parsed = AxisValue::from_str(lit)
                .map(|v| v.to_string())
                .or_else(|_| JointType::from_str(lit).map(|v| v.to_string()))
                .or_else(|_| RoleType::from_str(lit).map(|v| v.to_string()))
                .expect("literal parses in its own enum");
            assert_eq!(parsed, lit, "Display/FromStr not inverse for {lit:?}");
        }
    }
    assert_eq!(CompRole::Actuator.to_string(), "actuator");
    assert_eq!(AxisValue::NegX.to_string(), "-X");
}

#[test]
fn enum_attrs_table_is_scoped_to_model_untyped_slots() {
    // Typed `Option<EnumTy>` slots are enforced at PARSE, so they are NO LONGER in the run-time table.
    assert_eq!(valid_values_for("comp", "@role"), None);
    assert_eq!(valid_values_for("joint", "@type"), None);
    // The residual model-untyped slots (every enumerated slot whose enum has NO typed model field)
    // stay in the table and are enforced by `validate_enums`:
    //   * the sensor-category `@type` slots that share one `SensorCategory` struct,
    //   * the `lens`/`probe` projection/probe `@type` fields,
    //   * the string-typed transmission endpoint roles.
    assert_eq!(
        valid_values_for("em", "@type"),
        Some(EmSensorType::valid_values())
    );
    assert!(valid_values_for("tactile", "@type").is_some());
    // The fluid category and lens/probe @type fields are model-untyped too:
    assert!(valid_values_for("fluid", "@type").is_some());
    assert!(valid_values_for("lens", "@type").is_some());
    assert!(valid_values_for("probe", "@type").is_some());
    assert_eq!(valid_values_for("cipher", "#text"), None);
    assert_eq!(valid_values_for("dc", "@mode"), None);
    assert_eq!(valid_values_for("tunnel", "@protocol"), None);
    // The transmission endpoint `@role` (new HCDF-1.0 `RoleType`) is kept `String`-typed in the model
    // (presence-preserving), so both endpoint elements carry a model-untyped `@role` slot enforced here.
    assert_eq!(
        valid_values_for("joint", "@role"),
        Some(RoleType::valid_values())
    );
    assert_eq!(
        valid_values_for("motor", "@role"),
        Some(RoleType::valid_values())
    );
    // A non-enumerated (or now-typed) slot resolves to None (the validator leaves it alone).
    assert_eq!(valid_values_for("comp", "@hwid"), None);
    // Exactly the 10 sensor-category `@type` slots, lens/probe projection/probe `@type`, and the two
    // transmission endpoint `@role` slots.
    assert_eq!(
        ENUM_ATTRS.len(),
        14,
        "ENUM_ATTRS should hold sensor-category and lens/probe @type plus joint/motor @role"
    );
    let expected_non_type: &[(&str, &str)] = &[("joint", "@role"), ("motor", "@role")];
    assert!(ENUM_ATTRS
        .iter()
        .all(|(e, k, _)| *k == "@type" || expected_non_type.contains(&(*e, *k))));
}
