use hcdformat::connectivity::{
    built_in_profile_registry, NormalizationOptions, ProfileRegistryCompleteness, ValidatorKind,
};
use hcdformat::schema::{
    verify_embedded_connectivity_registry, CONNECTIVITY_PROFILES_JSON_1_0,
    CONNECTIVITY_PROFILE_REGISTRY_SCHEMA_JSON_1_0,
};
use serde_json::{json, Value};

fn external_profile_document() -> hcdformat::Hcdf {
    hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="external-profile" version="1.0">
          <comp name="a"><port name="p"/></comp>
          <comp name="b"><port name="p"/></comp>
          <link name="n">
            <selected purpose="communication" carrier="electrical">
              <profile id="vendor.example:external-link"/>
            </selected>
            <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
          </link>
        </hcdf>"#,
    )
    .unwrap()
}

fn can_profile_link_document(profile: &str) -> hcdformat::Hcdf {
    hcdformat::Hcdf::from_xml_str(&format!(
        r#"<hcdf name="can-profile" version="1.0">
          <comp name="a"><port name="p"/></comp>
          <comp name="b"><port name="p"/></comp>
          <link name="n">
            <selected purpose="communication" carrier="electrical">
              <profile id="{profile}"/>
            </selected>
            <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
          </link>
        </hcdf>"#
    ))
    .unwrap()
}

#[test]
fn embedded_registry_has_canonical_built_in_rulesets_and_verified_hashes() {
    verify_embedded_connectivity_registry().expect("embedded registry pins match");
    let registry = built_in_profile_registry().expect("built-in registry parses");
    assert_eq!(registry.registry_version().as_str(), "1.4.0");
    assert_eq!(registry.hcdf_schema().major, 1);
    assert_eq!(registry.hcdf_schema().minimum_minor, 0);
    assert_eq!(registry.rulesets().len(), 7);

    let feetech_sts = &registry.rulesets()[0];
    assert_eq!(feetech_sts.profile().as_str(), "feetech:sts");
    assert_eq!(feetech_sts.ruleset_version().as_str(), "1.0.0");
    assert_eq!(feetech_sts.validator(), ValidatorKind::FeetechSts);
    assert_eq!(feetech_sts.validator().id(), "hcdf:validator/feetech-sts");
    assert_eq!(
        feetech_sts
            .citations()
            .iter()
            .map(|citation| citation.key.as_str())
            .collect::<Vec<_>>(),
        [
            "serial-bus-protocol",
            "sts3215-evaluation",
            "sts3215-product"
        ]
    );

    let bidirectional_dshot = &registry.rulesets()[1];
    assert_eq!(
        bidirectional_dshot.profile().as_str(),
        "hcdf:bidirectional-dshot"
    );
    assert_eq!(
        bidirectional_dshot.validator(),
        ValidatorKind::BidirectionalDshot
    );
    assert_eq!(
        bidirectional_dshot.validator().id(),
        "hcdf:validator/bidirectional-dshot"
    );
    assert_eq!(
        bidirectional_dshot
            .citations()
            .iter()
            .map(|citation| citation.key.as_str())
            .collect::<Vec<_>>(),
        ["same-line-telemetry"]
    );

    let can = &registry.rulesets()[2];
    assert_eq!(can.profile().as_str(), "hcdf:can");
    assert_eq!(can.ruleset_version().as_str(), "1.0.0");
    assert_eq!(can.validator(), ValidatorKind::Can);
    assert_eq!(can.validator().id(), "hcdf:validator/can");
    assert_eq!(
        can.citations()
            .iter()
            .map(|citation| citation.key.as_str())
            .collect::<Vec<_>>(),
        ["can-lower-layers", "high-speed-pma", "network-topology"]
    );

    let dshot = &registry.rulesets()[3];
    assert_eq!(dshot.profile().as_str(), "hcdf:dshot");
    assert_eq!(dshot.validator(), ValidatorKind::Dshot);
    assert_eq!(dshot.validator().id(), "hcdf:validator/dshot");
    assert_eq!(
        dshot
            .citations()
            .iter()
            .map(|citation| citation.key.as_str())
            .collect::<Vec<_>>(),
        ["digital-motor-link", "frame-and-direction"]
    );

    let pwm = &registry.rulesets()[4];
    assert_eq!(pwm.profile().as_str(), "hcdf:pwm");
    assert_eq!(pwm.validator(), ValidatorKind::Pwm);
    assert_eq!(pwm.validator().id(), "hcdf:validator/pwm");
    assert_eq!(
        pwm.citations()
            .iter()
            .map(|citation| citation.key.as_str())
            .collect::<Vec<_>>(),
        ["point-to-point-control", "signal-definition"]
    );

    let rs485 = &registry.rulesets()[5];
    assert_eq!(rs485.profile().as_str(), "hcdf:rs-485");
    assert_eq!(rs485.ruleset_version().as_str(), "1.0.0");
    assert_eq!(rs485.validator(), ValidatorKind::Rs485);
    assert_eq!(
        rs485
            .citations()
            .iter()
            .map(|citation| citation.key.as_str())
            .collect::<Vec<_>>(),
        ["electrical-and-bus", "system-configurations"]
    );

    let uart = &registry.rulesets()[6];
    assert_eq!(uart.profile().as_str(), "hcdf:uart");
    assert_eq!(uart.ruleset_version().as_str(), "1.0.0");
    assert_eq!(uart.validator(), ValidatorKind::Uart);
    assert_eq!(uart.validator().id(), "hcdf:validator/uart");
    assert_eq!(
        uart.citations()
            .iter()
            .map(|citation| citation.key.as_str())
            .collect::<Vec<_>>(),
        ["link-interface", "multiprocessor-bus"]
    );

    assert_eq!(
        registry.to_canonical_json().as_bytes(),
        CONNECTIVITY_PROFILES_JSON_1_0
    );
}

#[test]
fn canonical_registry_conforms_to_its_draft_2020_12_schema() {
    let schema: Value = serde_json::from_slice(CONNECTIVITY_PROFILE_REGISTRY_SCHEMA_JSON_1_0)
        .expect("registry JSON Schema parses");
    jsonschema::draft202012::meta::validate(&schema)
        .expect("registry JSON Schema conforms to its meta-schema");
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .expect("registry JSON Schema compiles");
    let canonical: Value =
        serde_json::from_slice(CONNECTIVITY_PROFILES_JSON_1_0).expect("registry JSON parses");
    assert!(validator.is_valid(&canonical));

    let mut extra_property = canonical.clone();
    extra_property["rules"] = json!([]);
    assert!(!validator.is_valid(&extra_property));

    let mut bad_version = canonical.clone();
    bad_version["registry-version"] = json!("01.0.0");
    assert!(!validator.is_valid(&bad_version));

    for field in ["major", "minimum-minor"] {
        let mut at_u32_maximum = canonical.clone();
        at_u32_maximum["hcdf-schema"][field] = json!(u32::MAX);
        assert!(validator.is_valid(&at_u32_maximum));

        let mut above_u32_maximum = canonical.clone();
        above_u32_maximum["hcdf-schema"][field] = json!(u64::from(u32::MAX) + 1);
        assert!(!validator.is_valid(&above_u32_maximum));
    }

    let mut executable_field = canonical.clone();
    executable_field["rulesets"] = json!([{
        "profile": "hcdf:example",
        "ruleset-version": "1.0.0",
        "validator": "hcdf:validator/generic",
        "citations": [{
            "key": "source",
            "organization": "Standards Body",
            "document": "STD-1",
            "locator": "Section 1"
        }],
        "formula": "voltage > 0"
    }]);
    assert!(!validator.is_valid(&executable_field));

    let mut unknown_validator = canonical;
    unknown_validator["rulesets"] = json!([{
        "profile": "hcdf:example",
        "ruleset-version": "1.0.0",
        "validator": "vendor:validator/dynamic",
        "citations": [{
            "key": "source",
            "organization": "Standards Body",
            "document": "STD-1",
            "locator": "Section 1"
        }]
    }]);
    assert!(!validator.is_valid(&unknown_validator));

    let mut blank_citation = json!({
        "format": "hcdf-connectivity-profile-registry",
        "format-version": 1,
        "registry-version": "1.0.0",
        "hcdf-schema": {"major": 1, "minimum-minor": 0},
        "rulesets": []
    });
    blank_citation["rulesets"] = json!([{
        "profile": "hcdf:example",
        "ruleset-version": "1.0.0",
        "validator": "hcdf:validator/generic",
        "citations": [{
            "key": "source",
            "organization": "   ",
            "document": "STD-1",
            "locator": "Section 1"
        }]
    }]);
    assert!(!validator.is_valid(&blank_citation));
}

#[test]
fn public_can_dispatch_preserves_exact_identity() {
    let exact = hcdformat::validate_network(&can_profile_link_document("hcdf:can"));
    let can_issues = exact
        .iter()
        .filter(|issue| issue.code.starts_with("E_CONN_CAN_"))
        .collect::<Vec<_>>();
    assert_eq!(can_issues.len(), 1);
    assert_eq!(can_issues[0].code, "E_CONN_CAN_TOPOLOGY");

    for different in ["vendor:can", "HCDF:can", "hcdf:CAN", "hcdf:can-fd"] {
        let issues = hcdformat::validate_network(&can_profile_link_document(different));
        assert!(
            issues
                .iter()
                .all(|issue| !issue.code.starts_with("E_CONN_CAN_")),
            "CAN validator dispatched for nearby identity {different}"
        );
        assert_eq!(
            issues
                .iter()
                .filter(|issue| issue.code == "W_CONN_PROFILE_INCOMPLETE")
                .count(),
            1,
            "unexpected profile completeness result for {different}"
        );
    }
}

#[test]
fn public_validation_reports_external_profiles_advisory_by_default_and_strict_on_request() {
    let document = external_profile_document();
    let advisory = hcdformat::validate_network(&document);
    assert_eq!(
        advisory
            .iter()
            .filter(|issue| issue.code == "W_CONN_PROFILE_INCOMPLETE")
            .count(),
        1
    );
    assert!(advisory
        .iter()
        .all(|issue| issue.level == hcdformat::Level::Warning));

    let strict = hcdformat::validate_network_with_options(
        &document,
        NormalizationOptions {
            profile_registry_completeness: ProfileRegistryCompleteness::Strict,
        },
    );
    let incomplete = strict
        .iter()
        .filter(|issue| issue.code == "E_CONN_PROFILE_INCOMPLETE")
        .collect::<Vec<_>>();
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].level, hcdformat::Level::Error);
    assert!(incomplete[0].message.contains("network=\"n\""));
    assert!(incomplete[0]
        .message
        .contains("vendor.example:external-link"));
}
