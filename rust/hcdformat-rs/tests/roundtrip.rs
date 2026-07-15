//! Foundation gate: the schema sha-pin verifies, poses are attribute-based (the core dialect fix), and
//! a minimal official document round-trips. The full `examples/*.hcdf` corpus round-trip lives in
//! `tests/examples.rs`; schema-validation parity (pure-Rust uppsala vs lxml) in `tests/xsd_parity.rs`.
use hcdformat::model::enums::CompRole;
use hcdformat::model::VisualAppearance;
use hcdformat::{Hcdf, Pose};

#[test]
fn schema_pin_is_single_sourced_and_verifies() {
    // re-hash the embedded schema; must equal the build-time pin generated from the vendored HASHES
    hcdformat::schema::verify_embedded_schema().expect("embedded schema matches the pin");
    assert_eq!(
        hcdformat::schema::HCDF_XSD_SHA256_1_0,
        "57207f49eea9a94275b35108626a42c3971095e651a4f2ddbd23fb32448e6ef2"
    );
}

#[test]
fn pose_is_attributes_not_text() {
    let p: Pose = quick_xml::de::from_str(r#"<pose xyz="1 2 3" rpy="0 0 1.5708"/>"#).unwrap();
    assert_eq!(p.xyz, Some([1.0, 2.0, 3.0]));
    // parse the same literal the XML carries (not the FRAC_PI_2 constant; this is exactly 1.5708).
    let yaw: f64 = "1.5708".parse().unwrap();
    assert_eq!(p.rpy, Some([0.0, 0.0, yaw]));
    assert!(p.quat.is_none());
    let xml = quick_xml::se::to_string(&p).unwrap();
    assert!(
        xml.contains(r#"xyz="1 2 3""#),
        "expected attribute pose, got {xml}"
    );
}

#[test]
fn quat_attribute_parses() {
    let p: Pose = quick_xml::de::from_str(r#"<pose quat="0 0 0 1"/>"#).unwrap();
    assert_eq!(p.quat, Some([0.0, 0.0, 0.0, 1.0]));
}

#[test]
fn minimal_official_hcdf_roundtrips() {
    let src = r#"<hcdf name="demo" version="1.0"><comp name="base" role="parent"><board>navq</board></comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).unwrap();
    assert_eq!(doc.name, "demo");
    assert_eq!(doc.version, "1.0");
    assert_eq!(doc.comp.len(), 1);
    assert_eq!(doc.comp[0].name, "base");
    assert_eq!(doc.comp[0].role, Some(CompRole::Parent));
    assert_eq!(doc.comp[0].board.as_deref(), Some("navq"));
    hcdformat::validate::validate_structural(&doc).expect("structurally valid");

    let out = doc.to_xml_string().unwrap();
    let doc2 = Hcdf::from_xml_str(&out).unwrap();
    assert_eq!(doc, doc2, "round-trip mismatch; serialized = {out}");
}

/// A native ball joint carrying an ELLIPTIC swing-cone (`swing1 != swing2`) + a twist bound
/// round-trips through serialize/parse as a fixpoint, and passes the semantic validator (swing/twist
/// are ball-only, so a ball is where they belong).
#[test]
fn ball_swing_twist_round_trips_native() {
    let src = r#"<hcdf name="b" version="1.0"><comp name="a" role="parent"/><comp name="c" role="parent"/>
      <joint name="j" type="ball"><parent comp="a"/><child comp="c"/>
        <swing_limit swing1="0.5" swing2="0.8" effort="3" velocity="2"/>
        <twist_limit lower="-1.57" upper="1.57"/></joint></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse ball");
    let j = &doc.joint[0];
    let sw = j.swing_limit.as_ref().expect("swing_limit present");
    assert_eq!(sw.swing1.as_deref(), Some("0.5"));
    assert_eq!(sw.swing2.as_deref(), Some("0.8"));
    let tw = j.twist_limit.as_ref().expect("twist_limit present");
    assert_eq!(tw.lower.as_deref(), Some("-1.57"));
    assert_eq!(tw.upper.as_deref(), Some("1.57"));
    // No swing/twist forbid on a ball.
    assert!(
        hcdformat::validate_semantic(&doc)
            .iter()
            .all(|i| i.code != "E_JOINT_SWING_FORBIDDEN"),
        "a ball must accept swing/twist"
    );
    let out = doc.to_xml_string().expect("serialize");
    assert_eq!(
        doc,
        Hcdf::from_xml_str(&out).expect("re-parse"),
        "ball swing/twist not a fixpoint: {out}"
    );
}

/// A native cylindrical joint carrying BOTH DOF bounds (`<limit>` = translation (m),
/// `<limit2>` = rotation (rad)) round-trips as a fixpoint and passes the semantic validator (the
/// rotation bound now has a home in `<limit2>`).
#[test]
fn cylindrical_dual_limit_round_trips_native() {
    let src = r#"<hcdf name="cy" version="1.0"><comp name="a" role="parent"/><comp name="c" role="parent"/>
      <joint name="j" type="cylindrical"><parent comp="a"/><child comp="c"/>
        <axis xyz="0 0 1"/>
        <limit lower="0" upper="0.3"/>
        <limit2 lower="-3.14" upper="3.14"/></joint></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse cylindrical");
    let j = &doc.joint[0];
    assert_eq!(
        j.limit.as_ref().and_then(|l| l.upper.as_deref()),
        Some("0.3")
    );
    assert_eq!(
        j.limit2.as_ref().and_then(|l| l.upper.as_deref()),
        Some("3.14")
    );
    let issues = hcdformat::validate_semantic(&doc);
    assert!(
        issues.iter().all(|i| !i.code.starts_with("E_JOINT")),
        "a cylindrical with limit+limit2 must validate; got {issues:?}"
    );
    let out = doc.to_xml_string().expect("serialize");
    assert_eq!(
        doc,
        Hcdf::from_xml_str(&out).expect("re-parse"),
        "cylindrical dual-limit not a fixpoint: {out}"
    );
}

/// Functional channels and physical connector positions are authored independently and round-trip.
#[test]
fn ports_and_connector_positions_round_trip_native() {
    let src = r#"<hcdf name="act" version="1.0"><comp name="actuator" role="parent">
      <port name="power"><channel name="vcc"/><channel name="gnd"/></port>
      <port name="can"><channel name="canh"/><channel name="canl"/></port>
      <port name="serial"><channel name="tx"/><channel name="rx"/><channel name="gnd"/></port>
      <connector name="power-connector" family="hcdf:xt30"><pin name="+"/><pin name="-"/></connector>
      <connector name="can-connector" family="hcdf:jst-gh-2"><pin name="1"/><pin name="2"/></connector>
      <connector name="serial-connector" family="hcdf:jst-gh-3"><pin name="1"/><pin name="2"/><pin name="3"/></connector>
    </comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse connectivity");
    hcdformat::validate::validate_structural(&doc).expect("structurally valid");
    let comp = &doc.comp[0];
    assert_eq!(comp.port.len(), 3);
    assert_eq!(comp.connector.len(), 3);

    let power_channels: Vec<_> = comp.port[0]
        .channel
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    assert_eq!(power_channels, ["vcc", "gnd"]);
    let power_positions: Vec<_> = comp.connector[0]
        .pin
        .iter()
        .map(|position| position.name.as_str())
        .collect();
    assert_eq!(power_positions, ["+", "-"]);

    let can_channels: Vec<_> = comp.port[1]
        .channel
        .iter()
        .map(|channel| channel.name.as_str())
        .collect();
    assert_eq!(can_channels, ["canh", "canl"]);
    assert!(
        !can_channels
            .iter()
            .any(|channel| matches!(*channel, "vcc" | "vin" | "gnd")),
        "CAN channels must not gain an inferred power channel"
    );
    assert_eq!(comp.connector[1].pin.len(), 2);

    let out = doc.to_xml_string().expect("serialize");
    assert!(
        out.contains(r#"<pin name="+""#),
        "non-numeric position name must serialize: {out}"
    );
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "connectivity not a round-trip fixpoint: {out}");
    assert_eq!(
        out,
        doc2.to_xml_string().expect("re-serialize"),
        "connectivity serialization not byte-stable"
    );
}

/// A motor carrying the two new datasheet leaves, `<rotor-inertia>` (kg·m²) and `<inductance>`
/// (phase, H), round-trips as a byte-stable fixpoint, proving both new measured-value elements parse,
/// re-serialize, and survive the JSON harness alongside the pre-existing resistance/torque-constant.
#[test]
fn motor_datasheet_fields_round_trip_native() {
    let src = r#"<hcdf name="m" version="1.0"><comp name="drive" role="parent">
      <motor name="qdd" type="bldc">
        <resistance unit="ohm">0.15</resistance>
        <inductance unit="H">0.000045</inductance>
        <torque-constant unit="Nm/A">0.09</torque-constant>
        <rotor-inertia unit="kg.m^2">0.000012</rotor-inertia>
      </motor></comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse motor");
    hcdformat::validate::validate_structural(&doc).expect("structurally valid");
    let motor = &doc.comp[0].motor[0];
    let ind = motor.inductance.as_ref().expect("inductance present");
    assert_eq!(ind.unit.as_deref(), Some("H"));
    assert_eq!(ind.value.as_deref(), Some("0.000045"));
    let j = motor.rotor_inertia.as_ref().expect("rotor-inertia present");
    assert_eq!(j.unit.as_deref(), Some("kg.m^2"));
    assert_eq!(j.value.as_deref(), Some("0.000012"));

    let out = doc.to_xml_string().expect("serialize");
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "motor datasheet fields not a fixpoint: {out}");
    assert_eq!(
        out,
        doc2.to_xml_string().expect("re-serialize"),
        "motor serialization not byte-stable"
    );

    round_trip_json(&doc);
}

/// An empty functional port remains empty and does not fabricate a physical connector or channel.
#[test]
fn empty_port_serializes_without_connector_or_channel() {
    let src = r#"<hcdf name="p" version="1.0"><comp name="c" role="parent"><port name="eth0"/></comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse empty port");
    assert!(doc.comp[0].port[0].channel.is_empty());
    assert!(doc.comp[0].connector.is_empty());
    let out = doc.to_xml_string().expect("serialize");
    assert!(
        !out.contains("<channel"),
        "empty port emitted a channel: {out}"
    );
    assert!(
        !out.contains("<connector"),
        "empty port emitted a connector: {out}"
    );
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "empty port not a fixpoint: {out}");
}

/// Helper: unwrap the ARM A `<model>` of the first comp's first visual, or panic.
fn model_of(doc: &Hcdf) -> &hcdformat::model::ModelRef {
    match &doc.comp[0].visual[0].appearance {
        VisualAppearance::Model { model, .. } => model,
        VisualAppearance::Primitive { .. } => panic!("expected ARM A model appearance"),
    }
}

/// Include mode: a `<model>` carrying a LIST of `<submesh>` selectors draws the UNION of the named
/// subtrees. Order is preserved, no exclusions leak in, and serialize/parse is a byte-stable fixpoint
/// that also survives the JSON harness. The include-list is the case-visual-as-several-parts shape.
#[test]
fn model_submesh_include_list_round_trips_native() {
    let src = r#"<hcdf name="v" version="1.0"><comp name="c" role="parent">
      <visual name="body"><model uri="drive.glb" sha="abc">
        <submesh name="case"/>
        <submesh name="xt30"/>
        <submesh name="jst_gh"/>
      </model></visual></comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse include-list");
    hcdformat::validate::validate_structural(&doc).expect("structurally valid");
    let model = model_of(&doc);
    let names: Vec<_> = model
        .submesh
        .iter()
        .filter_map(|s| s.name.as_deref())
        .collect();
    assert_eq!(
        names,
        ["case", "xt30", "jst_gh"],
        "union keeps order + every member"
    );
    assert!(
        model.exclude_submesh.is_empty(),
        "include mode carries no exclusions"
    );

    let out = doc.to_xml_string().expect("serialize");
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "include-list not a fixpoint: {out}");
    assert_eq!(
        out,
        doc2.to_xml_string().expect("re-serialize"),
        "include-list not byte-stable"
    );
    round_trip_json(&doc);
}

/// Exclude mode: a `<model>` carrying `<exclude-submesh>` selectors draws the whole model MINUS the
/// named subtrees. The dedicated exclude type has no `@center`. Round-trips byte-stably through the
/// XML and JSON harnesses: the whole-model-minus-flange case visual of the shared-actuator-GLB story.
#[test]
fn model_exclude_submesh_list_round_trips_native() {
    let src = r#"<hcdf name="v" version="1.0"><comp name="c" role="parent">
      <visual name="case"><model uri="drive.glb" sha="abc">
        <exclude-submesh name="flange"/>
      </model></visual></comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse exclude-list");
    hcdformat::validate::validate_structural(&doc).expect("structurally valid");
    let model = model_of(&doc);
    assert!(model.submesh.is_empty(), "exclude mode carries no includes");
    let excl: Vec<_> = model
        .exclude_submesh
        .iter()
        .filter_map(|s| s.name.as_deref())
        .collect();
    assert_eq!(excl, ["flange"], "exclusion names the subtracted subtree");

    let out = doc.to_xml_string().expect("serialize");
    assert!(
        out.contains(r#"<exclude-submesh name="flange""#),
        "exclusion must serialize: {out}"
    );
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "exclude-list not a fixpoint: {out}");
    assert_eq!(
        out,
        doc2.to_xml_string().expect("re-serialize"),
        "exclude-list not byte-stable"
    );
    round_trip_json(&doc);
}

/// Lone-include `@center`: a single `<submesh center="true">`, the LONE-include case where `@center`
/// (SDF parity recenter) is legal, round-trips byte-stably. `@center` and the submesh name both
/// survive; no exclusions present. The validator is what rejects `@center` on a union.
#[test]
fn model_lone_submesh_center_round_trips_native() {
    let src = r#"<hcdf name="v" version="1.0"><comp name="c" role="parent">
      <visual name="flange"><model uri="drive.glb" sha="abc">
        <submesh name="flange" center="true"/>
      </model></visual></comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse lone submesh+center");
    hcdformat::validate::validate_structural(&doc).expect("structurally valid");
    let model = model_of(&doc);
    assert_eq!(model.submesh.len(), 1, "a LONE include");
    assert_eq!(model.submesh[0].name.as_deref(), Some("flange"));
    assert_eq!(
        model.submesh[0].center.as_deref(),
        Some("true"),
        "@center legal on a lone include"
    );
    assert!(model.exclude_submesh.is_empty());

    let out = doc.to_xml_string().expect("serialize");
    assert!(
        out.contains(r#"center="true""#),
        "@center must serialize: {out}"
    );
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "lone submesh+center not a fixpoint: {out}");
    assert_eq!(
        out,
        doc2.to_xml_string().expect("re-serialize"),
        "lone submesh not byte-stable"
    );
    round_trip_json(&doc);
}

/// Zero-churn guard: a `<model>` with NO selectors serializes exactly as it did before submesh
/// selectors were added: both selector lists are `skip_serializing_if` empty, so the output carries no
/// `submesh` (or `exclude-submesh`) token at all and the whole existing selector-less corpus is byte-unaffected.
#[test]
fn model_without_selectors_serializes_without_submesh_element() {
    let src = r#"<hcdf name="v" version="1.0"><comp name="c" role="parent"><visual name="body"><model uri="drive.glb" sha="abc"/></visual></comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(src).expect("parse selector-less");
    let model = model_of(&doc);
    assert!(model.submesh.is_empty() && model.exclude_submesh.is_empty());
    let out = doc.to_xml_string().expect("serialize");
    assert!(
        !out.contains("submesh"),
        "selector-less model must not emit any submesh token: {out}"
    );
    let doc2 = Hcdf::from_xml_str(&out).expect("re-parse");
    assert_eq!(doc, doc2, "selector-less model not a fixpoint: {out}");
}

/// XML->JSON->XML harness (feature `json`): the JSON projection is a lossless view of pins and motor
/// datasheet fields, so the regenerated XML re-parses to the SAME model, and the JSON is a fixpoint.
#[cfg(feature = "json")]
fn round_trip_json(doc: &Hcdf) {
    let xml = doc.to_xml_string().expect("serialize");
    let json1 = hcdformat::hcdf_xml_to_json(&xml).expect("xml->json");
    let xml2 = hcdformat::json_to_hcdf_xml(&json1).expect("json->xml");
    let doc2 = Hcdf::from_xml_str(&xml2).expect("re-parse json->xml");
    assert_eq!(*doc, doc2, "XML->JSON->XML dropped modeled content");
    let json2 = hcdformat::hcdf_xml_to_json(&xml2).expect("xml->json (2)");
    assert_eq!(json1, json2, "JSON projection is not a fixpoint");
}

/// No-op when the `json` converter feature is off (the default test build); the fixpoint is still
/// proven at the XML layer above.
#[cfg(not(feature = "json"))]
fn round_trip_json(_doc: &Hcdf) {}
