use hcdformat::model::connectivity_xml::InstanceRef;
use hcdformat::model::{FrerSequenceEncoding, StreamProfileSelectionRole};
use hcdformat::{Hcdf, StreamProfileDocument};
use sha2::{Digest, Sha256};

const FIXTURE_WITH_NEWLINE: &str = include_str!("fixtures/typed-stream-profile.xml");
const FIXTURE_SHA256: &str = "5a494d6823a524881ac388df586435b106bff6635133e0e69e335a6fdc147378";
const OPERATIONAL_PROFILE: &str =
    include_str!("../../../examples/profiles/operational.streams.xml");

fn fixture() -> &'static str {
    FIXTURE_WITH_NEWLINE
        .strip_suffix('\n')
        .unwrap_or(FIXTURE_WITH_NEWLINE)
}

#[test]
fn operational_profile_exercises_single_and_multi_topology_routes() {
    let profile = StreamProfileDocument::from_xml_str(OPERATIONAL_PROFILE).unwrap();
    assert_eq!(profile.name, "operational");
    assert_eq!(profile.stream.len(), 10);
    assert_eq!(
        profile
            .stream
            .iter()
            .map(|stream| stream.path.forwarding.len())
            .sum::<usize>(),
        6
    );
    assert!(profile
        .stream
        .iter()
        .any(|stream| stream.path.network.len() == 1));
    assert!(profile
        .stream
        .iter()
        .any(|stream| stream.path.network.len() > 1));
}

#[test]
fn sidecar_round_trip_is_typed_and_a_byte_fixpoint() {
    let source = fixture();
    assert_eq!(format!("{:x}", Sha256::digest(source)), FIXTURE_SHA256);
    let document = StreamProfileDocument::from_xml_str(source).unwrap();

    assert_eq!(document.name, "operational");
    assert_eq!(document.version, "1.0");
    assert_eq!(document.dependency.len(), 2);
    assert!(document.dependency[0].required);
    assert!(!document.dependency[1].required);
    assert_eq!(document.stream_group.len(), 1);
    assert_eq!(document.stream.len(), 2);

    let chain_stream = &document.stream[0];
    assert_eq!(chain_stream.listener.len(), 2);
    assert_eq!(
        chain_stream.protocol.as_ref().unwrap().as_str(),
        "ieee:1722"
    );
    assert_eq!(
        chain_stream.frer.as_ref().unwrap().sequence_encoding,
        FrerSequenceEncoding::RTag
    );
    assert_eq!(
        chain_stream
            .group_ref
            .as_ref()
            .unwrap()
            .instance
            .as_ref()
            .unwrap()
            .segment[0]
            .occurrence,
        1
    );
    assert_eq!(chain_stream.path.network.len(), 2);
    assert_eq!(chain_stream.path.network[0].network, "left-arm");
    assert_eq!(
        chain_stream.path.network[0]
            .instance
            .as_ref()
            .unwrap()
            .segment[0]
            .name
            .as_deref(),
        Some("arm-module")
    );
    assert_eq!(chain_stream.path.network[1].network, "safety-backbone");
    assert_eq!(chain_stream.path.forwarding.len(), 1);
    assert_eq!(
        chain_stream.path.forwarding[0].from.participant.participant,
        "gateway-arm"
    );
    assert_eq!(
        chain_stream.path.forwarding[0].function.function,
        "arm-bridge"
    );
    assert_eq!(
        chain_stream.path.forwarding[0].to.participant.participant,
        "gateway-backbone"
    );
    assert_eq!(
        chain_stream
            .talker
            .participant
            .instance
            .as_ref()
            .unwrap()
            .segment[0]
            .occurrence,
        1
    );

    let network_stream = &document.stream[1];
    assert_eq!(
        network_stream.protocol.as_ref().unwrap().as_str(),
        "vendor:telemetry"
    );
    assert_eq!(network_stream.path.network.len(), 1);
    assert_eq!(network_stream.path.network[0].network, "telemetry-net");
    assert_eq!(
        network_stream.path.network[0]
            .instance
            .as_ref()
            .unwrap()
            .segment[0]
            .occurrence,
        2
    );
    assert!(network_stream.path.forwarding.is_empty());

    let serialized = document.to_xml_string().unwrap();
    assert_eq!(serialized, source);
    let reparsed = StreamProfileDocument::from_xml_str(&serialized).unwrap();
    assert_eq!(reparsed, document);
    assert_eq!(reparsed.to_xml_string().unwrap(), serialized);
}

#[test]
fn hcdf_resource_declarations_default_required_and_round_trip() {
    let source = concat!(
        r#"<hcdf name="robot" version="1.0">"#,
        r#"<stream-profile uri="profiles/operational.streams.xml" sha="sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" selection-role="default"/>"#,
        r#"<stream-profile uri="profiles/optional.streams.xml" required="false"/>"#,
        r#"</hcdf>"#,
    );
    let document = Hcdf::from_xml_str(source).unwrap();
    assert_eq!(document.stream_profile.len(), 2);
    assert!(document.stream_profile[0].required);
    assert_eq!(
        document.stream_profile[0].selection_role,
        Some(StreamProfileSelectionRole::Default)
    );
    assert!(!document.stream_profile[1].required);

    let serialized = document.to_xml_string().unwrap();
    assert!(serialized.contains(
        r#"<stream-profile uri="profiles/operational.streams.xml" sha="sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" selection-role="default"/>"#
    ));
    assert!(!serialized.contains(r#"required="true""#));
    assert!(serialized
        .contains(r#"<stream-profile uri="profiles/optional.streams.xml" required="false"/>"#));
    let reparsed = Hcdf::from_xml_str(&serialized).unwrap();
    assert_eq!(reparsed, document);
    assert_eq!(reparsed.to_xml_string().unwrap(), serialized);
}

#[test]
fn parser_rejects_wrong_root_unknown_retired_and_nested_reference_shapes() {
    let cases = [
        r#"<streams name="p" version="1.0"/>"#,
        r#"<stream-profile xmlns="urn:foreign" name="p" version="1.0"/>"#,
        r#"<stream-profile name="p" version="1.0" hcdf-ref="robot.hcdf"/>"#,
        r#"<stream-profile name="p" version="1.0"><dependency uri="x" active="true"/></stream-profile>"#,
        r#"<stream-profile name="p" version="1.0"><stream-group name="g"><stream name="nested"/></stream-group></stream-profile>"#,
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" talker="old" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><network-ref network="n"/></path><talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><leg-ref network="n" leg="x"/></path><talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><chain-ref chain="n"/></path><talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><network-ref network="n"/><transition/></path><talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><network-ref network="n"/></path><talker><participant-ref network="n" participant="t" component="old"/></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path xmlns="urn:foreign"><network-ref network="n"/></path>"#,
            r#"<talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><network-ref network="n"/></path><talker><participant-ref network="n" participant="t">"#,
            r#"<instance><ref occurrence="0"/></instance></participant-ref></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
    ];
    for source in cases {
        let error = StreamProfileDocument::from_xml_str(source)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("stream-profile") || error.contains("unknown"),
            "unexpected strict error for {source}: {error}"
        );
    }

    let hcdf_error = Hcdf::from_xml_str(
        r#"<hcdf name="r" version="1.0"><stream-profile uri="p" active="true"/></hcdf>"#,
    )
    .unwrap_err()
    .to_string();
    assert!(hcdf_error.contains("unknown HCDF core connectivity attribute @active"));
}

#[test]
fn parser_enforces_path_choice_listener_cardinality_and_qualified_protocol() {
    let no_listener = concat!(
        r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
        r#"<path><network-ref network="n"/></path><talker><participant-ref network="n" participant="t"/></talker>"#,
        r#"</stream></stream-profile>"#,
    );
    let error = StreamProfileDocument::from_xml_str(no_listener)
        .unwrap_err()
        .to_string();
    assert!(error.contains("at least one <listener>"), "{error}");

    let path_without_network = concat!(
        r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
        r#"<path></path>"#,
        r#"<talker><participant-ref network="n" participant="t"/></talker>"#,
        r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
    );
    assert!(StreamProfileDocument::from_xml_str(path_without_network).is_err());

    let unqualified_protocol = concat!(
        r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1" protocol="raw">"#,
        r#"<path><network-ref network="n"/></path><talker><participant-ref network="n" participant="t"/></talker>"#,
        r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
    );
    let error = StreamProfileDocument::from_xml_str(unqualified_protocol)
        .unwrap_err()
        .to_string();
    assert!(error.contains("qualified identifier"), "{error}");
}

fn scalar_profile(attributes: &str, frer: u32) -> String {
    format!(
        r#"<stream-profile name="p" version="1.0">
  <stream name="s" {attributes}>
    <path><network-ref network="n"/></path>
    <talker><participant-ref network="n" participant="t"/></talker>
    <listener><participant-ref network="n" participant="l"/></listener>
    <frer seamless-trees="{frer}" sequence-encoding="r-tag"/>
  </stream>
</stream-profile>"#
    )
}

#[test]
fn parser_and_programmatic_serialization_enforce_stream_scalar_boundaries() {
    for source in [
        scalar_profile(
            "vlan-id=\"0\" pcp=\"0\" max-frame-size-bytes=\"1\" interval-ns=\"1\" max-latency-ns=\"1\"",
            2,
        ),
        scalar_profile(
            "vlan-id=\"4094\" pcp=\"7\" max-frame-size-bytes=\"4294967295\" interval-ns=\"18446744073709551615\" max-latency-ns=\"18446744073709551615\"",
            u32::MAX,
        ),
    ] {
        StreamProfileDocument::from_xml_str(&source).unwrap();
    }

    for (source, expected) in [
        (
            scalar_profile(
                "vlan-id=\"4095\" pcp=\"0\" max-frame-size-bytes=\"1\" interval-ns=\"1\"",
                2,
            ),
            "vlan-id must be in 0..=4094",
        ),
        (
            scalar_profile(
                "vlan-id=\"0\" pcp=\"8\" max-frame-size-bytes=\"1\" interval-ns=\"1\"",
                2,
            ),
            "pcp must be in 0..=7",
        ),
        (
            scalar_profile(
                "vlan-id=\"0\" pcp=\"0\" max-frame-size-bytes=\"0\" interval-ns=\"1\"",
                2,
            ),
            "max-frame-size-bytes must be greater than zero",
        ),
        (
            scalar_profile(
                "vlan-id=\"0\" pcp=\"0\" max-frame-size-bytes=\"1\" interval-ns=\"0\"",
                2,
            ),
            "interval-ns must be greater than zero",
        ),
        (
            scalar_profile(
                "vlan-id=\"0\" pcp=\"0\" max-frame-size-bytes=\"1\" interval-ns=\"1\" max-latency-ns=\"0\"",
                2,
            ),
            "max-latency-ns must be greater than zero when present",
        ),
        (
            scalar_profile(
                "vlan-id=\"0\" pcp=\"0\" max-frame-size-bytes=\"1\" interval-ns=\"1\"",
                0,
            ),
            "frer seamless-trees must be at least 2",
        ),
        (
            scalar_profile(
                "vlan-id=\"0\" pcp=\"0\" max-frame-size-bytes=\"1\" interval-ns=\"1\"",
                1,
            ),
            "frer seamless-trees must be at least 2",
        ),
    ] {
        let error = StreamProfileDocument::from_xml_str(&source)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "expected {expected:?}, got {error:?}");
    }

    let mut document = StreamProfileDocument::from_xml_str(&scalar_profile(
        "vlan-id=\"0\" pcp=\"0\" max-frame-size-bytes=\"1\" interval-ns=\"1\" max-latency-ns=\"1\"",
        2,
    ))
    .unwrap();
    document.stream[0].vlan_id = Some(4095);
    assert!(document
        .to_xml_string()
        .unwrap_err()
        .to_string()
        .contains("vlan-id"));
    document.stream[0].vlan_id = Some(0);
    document.stream[0].pcp = Some(8);
    assert!(document
        .to_xml_string()
        .unwrap_err()
        .to_string()
        .contains("pcp"));
    document.stream[0].pcp = Some(0);
    document.stream[0].max_frame_size_bytes = 0;
    assert!(document
        .to_xml_string()
        .unwrap_err()
        .to_string()
        .contains("max-frame-size-bytes"));
    document.stream[0].max_frame_size_bytes = 1;
    document.stream[0].interval_ns = 0;
    assert!(document
        .to_xml_string()
        .unwrap_err()
        .to_string()
        .contains("interval-ns"));
    document.stream[0].interval_ns = 1;
    document.stream[0].max_latency_ns = Some(0);
    assert!(document
        .to_xml_string()
        .unwrap_err()
        .to_string()
        .contains("max-latency-ns"));
    document.stream[0].max_latency_ns = Some(1);
    document.stream[0].frer.as_mut().unwrap().seamless_trees = 1;
    assert!(document
        .to_xml_string()
        .unwrap_err()
        .to_string()
        .contains("seamless-trees"));
    document.stream[0].frer.as_mut().unwrap().seamless_trees = 2;
    document.stream[0].path.network[0].instance = Some(InstanceRef {
        segment: Vec::new(),
    });
    assert!(document
        .to_xml_string()
        .unwrap_err()
        .to_string()
        .contains("at least one <segment>"));
}

#[test]
fn parser_enforces_sidecar_sequence_cardinality_and_nonempty_instances() {
    let cases = [
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<path><network-ref network="n"/></path>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><network-ref network="n"/><network-ref network="m"/>"#,
            r#"<forwarding><to><participant-ref network="m" participant="x"/></to>"#,
            r#"<function-ref component="g" function="b"/><from><participant-ref network="n" participant="x"/></from></forwarding></path>"#,
            r#"<talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<listener><participant-ref network="m" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><network-ref network="n"><instance/></network-ref></path>"#,
            r#"<talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
        concat!(
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="1" interval-ns="1">"#,
            r#"<path><network-ref network="n"/></path>"#,
            r#"<talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<talker><participant-ref network="n" participant="t"/></talker>"#,
            r#"<listener><participant-ref network="n" participant="l"/></listener></stream></stream-profile>"#,
        ),
    ];
    for source in cases {
        let error = StreamProfileDocument::from_xml_str(source)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("out of order")
                || error.contains("at least one <segment>")
                || error.contains("occurs too many times"),
            "unexpected grammar error for {source}: {error}"
        );
    }
}
