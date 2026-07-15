use hcdformat::connectivity::{
    normalize_connectivity, ConnectivityNodeData, EdgeExactness, EdgeKind, IdentityPart,
    NormalizedConnectivityGraph, ObjectIdentity, ObjectKind, StableEdgeId, StableObjectId,
};
use hcdformat::model::connectivity as c;
use std::collections::{BTreeMap, BTreeSet};

fn document_identity() -> c::DocumentIdentity {
    c::DocumentIdentity::new("memory://contract/root.hcdf").unwrap()
}

fn profile(value: &str) -> c::QualifiedId {
    c::QualifiedId::new(value).unwrap()
}

fn capabilities(carrier: c::Carrier, profile_id: Option<&str>) -> c::Capabilities {
    let mut capabilities = c::Capabilities::default();
    capabilities.purposes.insert(c::Purpose::Communication);
    capabilities.carriers.insert(carrier);
    if let Some(profile_id) = profile_id {
        capabilities.profiles.insert(profile(profile_id));
    }
    capabilities
}

fn network_selection() -> c::NetworkSelection {
    c::NetworkSelection {
        purpose: c::Purpose::Communication,
        carrier: c::Carrier::Electrical,
        profiles: BTreeSet::new(),
        rate: None,
        voltage: None,
        current: None,
        power: None,
        impedance: None,
        frequency: None,
        rf: None,
        pressure: None,
        flow: None,
        temperature: None,
    }
}

fn port(name: &str, channel: &str, carrier: c::Carrier) -> c::Port {
    c::Port {
        name: name.to_owned(),
        capabilities: capabilities(carrier, Some("hcdf:test-link")),
        channels: vec![c::Channel {
            role: None,
            local_group: None,
            name: channel.to_owned(),
            capabilities: capabilities(carrier, Some("hcdf:test-link")),
        }],
    }
}

fn connector(name: &str, position: &str) -> c::Connector {
    c::Connector {
        name: name.to_owned(),
        family: Some(profile("hcdf:test-connector")),
        positions: vec![c::Position {
            role: None,
            local_group: None,
            name: position.to_owned(),
            kind: c::PositionKind::Contact,
            representation: None,
        }],
        representation: None,
    }
}

fn component(name: &str) -> c::ComponentConnectivity {
    c::ComponentConnectivity {
        component: name.to_owned(),
        ports: vec![port("bus", "signal", c::Carrier::Electrical)],
        connectors: vec![connector("J1", "1")],
        antennas: Vec::new(),
        functions: Vec::new(),
        paths: Vec::new(),
        junctions: Vec::new(),
        terminations: Vec::new(),
    }
}

fn declare_component(scope: &mut c::ConnectivityScope, component: c::ComponentConnectivity) {
    let component_name = component.component.clone();
    scope.components.push(component);
    scope.structural_anchors.push(c::StructuralAnchors {
        component: component_name,
        visuals: Vec::new(),
        frames: Vec::new(),
    });
}

fn declare_components(
    scope: &mut c::ConnectivityScope,
    components: impl IntoIterator<Item = c::ComponentConnectivity>,
) {
    for component in components {
        declare_component(scope, component);
    }
}

fn included_network_document() -> c::ConnectivityDocument {
    let root = c::IncludeInstanceId::root();
    let left = root.child("wheel", 0);
    let right = root.child("wheel", 1);
    let mut root_scope = c::ConnectivityScope::root();
    root_scope.networks.push(c::Network {
        name: "drive-bus".to_owned(),
        structure: c::NetworkStructure::Bus,
        description: Some("two included wheel drives".to_owned()),
        configuration: c::NetworkConfiguration::default(),
        selected: c::NetworkSelection {
            profiles: BTreeSet::from([profile("hcdf:test-link")]),
            ..network_selection()
        },
        participants: vec![
            c::Participant {
                name: "left-drive".to_owned(),
                endpoint: c::FunctionalEndpointRef::Channel(c::ChannelRef {
                    scope: c::ReferenceScope::Instance(left.clone()),
                    component: "drive".to_owned(),
                    port: "bus".to_owned(),
                    channel: "signal".to_owned(),
                }),
                role: Some(profile("hcdf:node")),
            },
            c::Participant {
                name: "right-drive".to_owned(),
                endpoint: c::FunctionalEndpointRef::Channel(c::ChannelRef {
                    scope: c::ReferenceScope::Instance(right.clone()),
                    component: "drive".to_owned(),
                    port: "bus".to_owned(),
                    channel: "signal".to_owned(),
                }),
                role: Some(profile("hcdf:node")),
            },
        ],
    });

    let mut left_scope = c::ConnectivityScope::new(left);
    declare_component(&mut left_scope, component("drive"));
    let mut right_scope = c::ConnectivityScope::new(right);
    declare_component(&mut right_scope, component("drive"));

    c::ConnectivityDocument {
        document: document_identity(),
        scopes: vec![root_scope, left_scope, right_scope],
    }
}

#[test]
fn stable_ids_are_structured_and_include_instance_aware() {
    let document = document_identity();
    let root = c::IncludeInstanceId::root();
    let left = root.child("wheel", 0);
    let right = root.child("wheel", 1);
    let left_port = ObjectIdentity::new(
        document.clone(),
        left,
        ObjectKind::Port,
        vec![
            IdentityPart::new("component", "drive"),
            IdentityPart::new("port", "bus"),
        ],
    );
    let right_port = ObjectIdentity::new(
        document,
        right,
        ObjectKind::Port,
        vec![
            IdentityPart::new("component", "drive"),
            IdentityPart::new("port", "bus"),
        ],
    );
    assert_ne!(left_port.stable_id(), right_port.stable_id());

    let ambiguous_a = ObjectIdentity::new(
        document_identity(),
        root.clone(),
        ObjectKind::Port,
        vec![IdentityPart::new("a", "bc")],
    );
    let ambiguous_b = ObjectIdentity::new(
        document_identity(),
        root,
        ObjectKind::Port,
        vec![IdentityPart::new("ab", "c")],
    );
    assert_ne!(ambiguous_a.stable_id(), ambiguous_b.stable_id());
}

#[test]
fn repeated_include_instances_resolve_without_aliasing() {
    let document = included_network_document();
    let graph = normalize_connectivity(&document).unwrap();
    let root = c::IncludeInstanceId::root();
    let left = root.child("wheel", 0);
    let right = root.child("wheel", 1);
    let resolver = graph.resolver(root);
    let left_port = resolver
        .port(&c::PortRef::in_instance(left, "drive", "bus"))
        .unwrap();
    let right_port = resolver
        .port(&c::PortRef::in_instance(right, "drive", "bus"))
        .unwrap();
    assert_ne!(left_port.id(), right_port.id());
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::Participant)
            .count(),
        2
    );
    assert_eq!(
        graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::ParticipantEndpoint)
            .count(),
        2
    );
}

#[test]
fn declarations_reorder_to_the_same_canonical_graph() {
    let first = included_network_document();
    let mut second = first.clone();
    second.scopes.reverse();
    assert_eq!(
        normalize_connectivity(&first)
            .unwrap()
            .to_canonical_json()
            .unwrap(),
        normalize_connectivity(&second)
            .unwrap()
            .to_canonical_json()
            .unwrap()
    );
}

#[test]
fn canonical_graph_matches_golden_fixture() {
    let graph = normalize_connectivity(&included_network_document()).unwrap();
    let actual = graph.to_canonical_json().unwrap();
    let expected = include_str!("golden/connectivity_contract_graph.json");
    assert_eq!(actual.trim(), expected.trim());
}

#[test]
fn exact_refinement_cannot_compete_with_a_coarse_binding() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let root = &mut document.scopes[0];
    declare_component(root, component("device"));
    root.bindings.push(c::Binding {
        name: "coarse".to_owned(),
        functional: c::FunctionalEndpointRef::Port(c::PortRef::local("device", "bus")),
        physical: c::PhysicalEndpointRef::Connector(c::ConnectorRef::local_component(
            "device", "J1",
        )),
        fidelity: c::Fidelity::Presented,
    });
    root.bindings.push(c::Binding {
        name: "exact".to_owned(),
        functional: c::FunctionalEndpointRef::Channel(c::ChannelRef::local(
            "device", "bus", "signal",
        )),
        physical: c::PhysicalEndpointRef::Position(c::PositionRef::local_component(
            "device", "J1", "1",
        )),
        fidelity: c::Fidelity::Exact,
    });

    let error = normalize_connectivity(&document).unwrap_err();
    let issue = error
        .issues()
        .iter()
        .find(|issue| issue.code() == "E_CONN_DUAL_FIDELITY")
        .unwrap();
    assert_eq!(issue.subject().kind(), ObjectKind::Binding);
    assert_eq!(issue.related().len(), 1);
    assert_eq!(issue.related()[0].kind(), ObjectKind::Binding);
    assert!(issue.subject_id().as_str().starts_with("hcdf-object-v1:"));
}

#[test]
fn mate_fidelity_and_position_mapping_are_explicit() {
    let mut presented = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut presented.scopes[0],
        [component("first"), component("second")],
    );
    presented.scopes[0].mates.push(c::Mate {
        name: "shells".to_owned(),
        first: c::ConnectorRef::local_component("first", "J1"),
        second: c::ConnectorRef::local_component("second", "J1"),
        mappings: Vec::new(),
        fidelity: c::Fidelity::Presented,
    });
    let graph = normalize_connectivity(&presented).unwrap();
    let shell_edge = graph
        .edges()
        .iter()
        .find(|edge| edge.kind() == EdgeKind::Mate)
        .unwrap();
    assert_eq!(shell_edge.exactness(), EdgeExactness::Coarse);
    assert!(!graph
        .edges()
        .iter()
        .any(|edge| edge.kind() == EdgeKind::MappedMate));

    let mut exact = presented.clone();
    exact.scopes[0].mates[0].fidelity = c::Fidelity::Exact;
    exact.scopes[0].mates[0].mappings = vec![c::PositionMapping {
        first: c::PositionRef::local_component("first", "J1", "1"),
        second: c::PositionRef::local_component("second", "J1", "1"),
    }];
    let graph = normalize_connectivity(&exact).unwrap();
    let shell_edge = graph
        .edges()
        .iter()
        .find(|edge| edge.kind() == EdgeKind::Mate)
        .unwrap();
    assert_eq!(shell_edge.exactness(), EdgeExactness::Exact);
    assert!(graph.edges().iter().any(|edge| {
        edge.kind() == EdgeKind::MappedMate && edge.exactness() == EdgeExactness::Exact
    }));

    let mut missing_mapping = presented.clone();
    missing_mapping.scopes[0].mates[0].fidelity = c::Fidelity::Exact;
    let error = normalize_connectivity(&missing_mapping).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_MATE_FIDELITY"));

    let mut wrong_shell = exact;
    declare_component(&mut wrong_shell.scopes[0], component("third"));
    wrong_shell.scopes[0].mates[0].mappings[0].second =
        c::PositionRef::local_component("third", "J1", "1");
    let error = normalize_connectivity(&wrong_shell).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_MATE_MAPPING"));
}

#[test]
fn physical_assembly_owns_exact_paths_and_passive_objects_are_not_participants() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let root = &mut document.scopes[0];
    root.assemblies.push(c::PhysicalAssembly {
        name: "leg-harness".to_owned(),
        kind: c::AssemblyKind::Harness,
        connectors: vec![connector("P1", "1"), connector("P2", "1")],
        representation: None,
        paths: vec![c::PhysicalPath {
            role: None,
            local_group: None,
            name: "signal-wire".to_owned(),
            kind: c::PathKind::Wire,
            first: c::PhysicalEndpointRef::Position(c::PositionRef {
                owner: c::OwnerRef::local_assembly("leg-harness"),
                connector: "P1".to_owned(),
                position: "1".to_owned(),
            }),
            second: c::PhysicalEndpointRef::Position(c::PositionRef {
                owner: c::OwnerRef::local_assembly("leg-harness"),
                connector: "P2".to_owned(),
                position: "1".to_owned(),
            }),
            fidelity: c::Fidelity::Exact,
            representation: None,
        }],
        junctions: vec![c::Junction {
            name: "splice".to_owned(),
            kind: c::JunctionKind::Splice,
            fidelity: c::Fidelity::Exact,
            attachments: vec![
                c::PhysicalEndpointRef::Position(c::PositionRef {
                    owner: c::OwnerRef::local_assembly("leg-harness"),
                    connector: "P1".to_owned(),
                    position: "1".to_owned(),
                }),
                c::PhysicalEndpointRef::Position(c::PositionRef {
                    owner: c::OwnerRef::local_assembly("leg-harness"),
                    connector: "P2".to_owned(),
                    position: "1".to_owned(),
                }),
            ],
            representation: None,
        }],
        terminations: Vec::new(),
    });

    let graph = normalize_connectivity(&document).unwrap();
    assert!(graph.edges().iter().any(|edge| {
        edge.kind() == EdgeKind::PhysicalPathEndpoint && edge.exactness() == EdgeExactness::Exact
    }));
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::Participant)
            .count(),
        0
    );
}

#[test]
fn antenna_relates_conducted_and_radiated_ports_without_becoming_a_participant() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    declare_component(
        &mut document.scopes[0],
        c::ComponentConnectivity {
            component: "radio".to_owned(),
            ports: vec![
                port("rf-in", "feed", c::Carrier::ConductedRf),
                port("air", "beam", c::Carrier::RadiatedRf),
            ],
            connectors: Vec::new(),
            antennas: vec![c::Antenna {
                name: "array".to_owned(),
                conducted_port: Some(c::PortRef::local("radio", "rf-in")),
                radiated_port: c::PortRef::local("radio", "air"),
                representation: None,
            }],
            functions: Vec::new(),
            paths: Vec::new(),
            junctions: Vec::new(),
            terminations: Vec::new(),
        },
    );

    let graph = normalize_connectivity(&document).unwrap();
    assert!(graph
        .edges()
        .iter()
        .any(|edge| edge.kind() == EdgeKind::AntennaFeed));
    assert!(graph
        .edges()
        .iter()
        .any(|edge| edge.kind() == EdgeKind::AntennaRadiation));
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::Participant)
            .count(),
        0
    );
}

#[test]
fn selected_capabilities_use_port_and_channel_evidence_layers() {
    let mut channel_supported = included_network_document();
    for scope in &mut channel_supported.scopes {
        for component in &mut scope.components {
            for port in &mut component.ports {
                port.capabilities.profiles = BTreeSet::new();
            }
        }
    }
    let channel_supported_graph = normalize_connectivity(&channel_supported).unwrap();
    assert_eq!(channel_supported_graph.warnings().len(), 1);
    assert_eq!(
        channel_supported_graph.warnings()[0].code(),
        "W_CONN_PROFILE_INCOMPLETE"
    );

    let mut port_fallback = included_network_document();
    for scope in &mut port_fallback.scopes {
        for component in &mut scope.components {
            for port in &mut component.ports {
                for channel in &mut port.channels {
                    channel.capabilities.profiles = BTreeSet::new();
                }
            }
        }
    }
    let port_fallback_graph = normalize_connectivity(&port_fallback).unwrap();
    assert_eq!(port_fallback_graph.warnings().len(), 1);
    assert_eq!(
        port_fallback_graph.warnings()[0].code(),
        "W_CONN_PROFILE_INCOMPLETE"
    );

    let mut unspecified = channel_supported;
    for scope in &mut unspecified.scopes {
        for component in &mut scope.components {
            for port in &mut component.ports {
                for channel in &mut port.channels {
                    channel.capabilities.profiles = BTreeSet::new();
                }
            }
        }
    }
    let graph = normalize_connectivity(&unspecified).unwrap();
    assert!(graph.warnings().iter().all(|issue| matches!(
        issue.code(),
        "W_CONN_SELECTION_UNVERIFIED" | "W_CONN_PROFILE_INCOMPLETE"
    )));
    assert!(graph
        .warnings()
        .iter()
        .any(|issue| issue.code() == "W_CONN_PROFILE_INCOMPLETE"));
    assert!(!graph.warnings().is_empty());

    let mut channel_mismatch = included_network_document();
    channel_mismatch.scopes[1].components[0].ports[0].channels[0]
        .capabilities
        .carriers = BTreeSet::from([c::Carrier::RadiatedRf]);
    let error = normalize_connectivity(&channel_mismatch).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_SELECTION_UNSUPPORTED"));
}

#[test]
fn typed_references_round_trip_without_parsing_display_paths() {
    let reference = c::ChannelRef::local("left/module", "servo-bus", "half-duplex");
    let json = serde_json::to_string(&reference).unwrap();
    assert!(json.contains(r#""component":"left/module""#));
    assert!(json.contains(r#""port":"servo-bus""#));
    assert!(json.contains(r#""channel":"half-duplex""#));
    assert_eq!(
        serde_json::from_str::<c::ChannelRef>(&json).unwrap(),
        reference
    );
}

#[test]
fn explicit_instance_paths_rebase_under_each_containing_module() {
    let root = c::IncludeInstanceId::root();
    let left = root.child("module", 0);
    let right = root.child("module", 1);
    let relative_sensor = root.child("sensor", 0);
    let left_sensor = left.child("sensor", 0);
    let right_sensor = right.child("sensor", 0);

    let module_scope = |instance: c::IncludeInstanceId| {
        let mut scope = c::ConnectivityScope::new(instance);
        declare_component(&mut scope, component("host"));
        scope.networks.push(c::Network {
            name: "internal".to_owned(),
            structure: c::NetworkStructure::Link,
            description: None,
            configuration: c::NetworkConfiguration::default(),
            selected: network_selection(),
            participants: vec![
                c::Participant {
                    name: "host".to_owned(),
                    endpoint: c::FunctionalEndpointRef::Port(c::PortRef::local("host", "bus")),
                    role: None,
                },
                c::Participant {
                    name: "sensor".to_owned(),
                    endpoint: c::FunctionalEndpointRef::Port(c::PortRef {
                        scope: c::ReferenceScope::Instance(relative_sensor.clone()),
                        component: "sensor".to_owned(),
                        port: "bus".to_owned(),
                    }),
                    role: None,
                },
            ],
        });
        scope
    };
    let sensor_scope = |instance| {
        let mut scope = c::ConnectivityScope::new(instance);
        declare_component(&mut scope, component("sensor"));
        scope
    };
    let document = c::ConnectivityDocument {
        document: document_identity(),
        scopes: vec![
            c::ConnectivityScope::root(),
            module_scope(left.clone()),
            module_scope(right.clone()),
            sensor_scope(left_sensor),
            sensor_scope(right_sensor),
        ],
    };
    let graph = normalize_connectivity(&document).unwrap();
    let relative_ref = c::PortRef {
        scope: c::ReferenceScope::Instance(relative_sensor),
        component: "sensor".to_owned(),
        port: "bus".to_owned(),
    };
    let left_port = graph
        .resolver(left)
        .port(&relative_ref)
        .expect("left nested sensor");
    let right_port = graph
        .resolver(right)
        .port(&relative_ref)
        .expect("right nested sensor");
    assert_ne!(left_port.id(), right_port.id());
}

#[test]
fn qualified_ids_reject_unsafe_registry_tokens() {
    for invalid in [
        "hcdf",
        ":profile",
        "hcdf:",
        "1vendor:profile",
        "hcdf:two words",
        "hcdf:profile:extra",
        "hcdf:profile?query",
        "hcdf:\nprofile",
    ] {
        assert!(
            c::QualifiedId::new(invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
    for valid in [
        "hcdf:rs-485",
        "ieee:802.3cg/10base-t1s",
        "vendor.example:mode_v2+fd",
    ] {
        assert!(c::QualifiedId::new(valid).is_ok(), "rejected {valid:?}");
    }
}

#[test]
fn unresolved_includes_cannot_produce_a_falsely_complete_graph() {
    let mut document = hcdformat::Hcdf {
        name: "root".to_owned(),
        ..Default::default()
    };
    document.include.push(hcdformat::model::Include {
        uri: Some("module.hcdf".to_owned()),
        name: Some("module".to_owned()),
        ..Default::default()
    });
    let error = document
        .to_connectivity_document(document_identity())
        .unwrap_err();
    assert_eq!(error.path(), "hcdf/include");
    assert!(error
        .reason()
        .contains("requires a flattened document or a document-set loader"));
}

fn participant(name: &str, component: &str, channel: Option<&str>) -> c::Participant {
    let endpoint = match channel {
        Some(channel) => {
            c::FunctionalEndpointRef::Channel(c::ChannelRef::local(component, "bus", channel))
        }
        None => c::FunctionalEndpointRef::Port(c::PortRef::local(component, "bus")),
    };
    c::Participant {
        name: name.to_owned(),
        endpoint,
        role: None,
    }
}

fn network(name: &str, participants: Vec<c::Participant>) -> c::Network {
    c::Network {
        name: name.to_owned(),
        structure: c::NetworkStructure::Link,
        description: None,
        configuration: c::NetworkConfiguration::default(),
        selected: network_selection(),
        participants,
    }
}

#[test]
fn scope_identity_is_unique_and_segments_are_nonempty() {
    let mut duplicate = c::ConnectivityDocument::new(document_identity());
    duplicate.scopes.push(c::ConnectivityScope::root());
    let error = normalize_connectivity(&duplicate).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_DUPLICATE_SCOPE"));

    let mut empty = c::ConnectivityDocument::new(document_identity());
    empty.scopes.push(c::ConnectivityScope::new(
        c::IncludeInstanceId::root().child("", 0),
    ));
    let error = normalize_connectivity(&empty).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_INSTANCE_SEGMENT"));
}

#[test]
fn participant_endpoint_overlap_laws_are_explicit() {
    let mut base = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.ports[0].channels.push(c::Channel {
        role: None,
        local_group: None,
        name: "aux".to_owned(),
        capabilities: capabilities(c::Carrier::Electrical, Some("hcdf:test-link")),
    });
    declare_component(&mut base.scopes[0], device);

    let mut duplicate = base.clone();
    duplicate.scopes[0].networks.push(network(
        "duplicate",
        vec![
            participant("first", "device", Some("signal")),
            participant("second", "device", Some("signal")),
        ],
    ));
    let error = normalize_connectivity(&duplicate).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_DUPLICATE_PARTICIPANT_ENDPOINT"));

    let mut overlap = base.clone();
    overlap.scopes[0].networks.push(network(
        "overlap",
        vec![
            participant("whole", "device", None),
            participant("channel", "device", Some("signal")),
        ],
    ));
    let error = normalize_connectivity(&overlap).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_PARTICIPANT_ENDPOINT_OVERLAP"));

    let mut distinct = base;
    distinct.scopes[0].networks.push(network(
        "distinct",
        vec![
            participant("signal", "device", Some("signal")),
            participant("aux", "device", Some("aux")),
        ],
    ));
    normalize_connectivity(&distinct).unwrap();
}

#[test]
fn one_endpoint_may_participate_in_multiple_networks() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut document.scopes[0],
        [component("device"), component("peer")],
    );
    for name in ["control", "diagnostic"] {
        document.scopes[0].networks.push(network(
            name,
            vec![
                participant("device", "device", Some("signal")),
                participant("peer", "peer", Some("signal")),
            ],
        ));
    }
    normalize_connectivity(&document).unwrap();
}

#[test]
fn topology_junction_and_route_arities_are_enforced() {
    let mut topology = c::ConnectivityDocument::new(document_identity());
    declare_component(&mut topology.scopes[0], component("device"));
    topology.scopes[0].networks.push(network(
        "short-link",
        vec![participant("only", "device", Some("signal"))],
    ));
    let error = normalize_connectivity(&topology).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_TOPOLOGY_ARITY"));

    let mut junction = c::ConnectivityDocument::new(document_identity());
    let mut assembly = c::PhysicalAssembly {
        name: "harness".to_owned(),
        connectors: vec![connector("P1", "1")],
        kind: c::AssemblyKind::Harness,
        representation: None,
        paths: Vec::new(),
        junctions: Vec::new(),
        terminations: Vec::new(),
    };
    assembly.junctions.push(c::Junction {
        name: "lonely".to_owned(),
        kind: c::JunctionKind::Splice,
        fidelity: c::Fidelity::Exact,
        attachments: vec![c::PhysicalEndpointRef::Position(c::PositionRef {
            owner: c::OwnerRef::local_assembly("harness"),
            connector: "P1".to_owned(),
            position: "1".to_owned(),
        })],
        representation: None,
    });
    junction.scopes[0].assemblies.push(assembly);
    let error = normalize_connectivity(&junction).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_JUNCTION_ARITY"));

    let mut route = c::ConnectivityDocument::new(document_identity());
    let mut route_component = component("device");
    route_component.connectors[0].representation = Some(c::Representation::DerivedRoute(
        c::DerivedRouteRepresentation {
            waypoints: vec![c::RoutePoint {
                frame: c::RouteFrameRef::World,
                xyz: [0.0, 0.0, 0.0],
                rotation: None,
            }],
            section: None,
        },
    ));
    declare_component(&mut route.scopes[0], route_component);
    let error = normalize_connectivity(&route).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_ROUTE_ARITY"));
}

fn primitive_representation(rotation: c::PlacementRotation) -> c::Representation {
    c::Representation::Primitive {
        primitive: c::PrimitiveRepresentation::Sphere { radius: 0.01 },
        placement: c::Placement {
            frame: c::RouteFrameRef::World,
            xyz: [0.0, 0.0, 0.0],
            rotation,
        },
    }
}

#[test]
fn placement_quaternions_are_finite_nonzero_and_unit_length() {
    let check = |values: [f64; 4]| {
        let mut document = c::ConnectivityDocument::new(document_identity());
        let mut device = component("device");
        device.connectors[0].representation = Some(primitive_representation(
            c::PlacementRotation::Quaternion(values),
        ));
        declare_component(&mut document.scopes[0], device);
        normalize_connectivity(&document)
    };
    check([0.0, 0.0, 0.0, 1.0]).unwrap();
    for invalid in [
        [0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 2.0],
        [0.0, 0.0, 0.0, f64::NAN],
    ] {
        let error = check(invalid).unwrap_err();
        assert!(error
            .issues()
            .iter()
            .any(|issue| issue.code() == "E_CONN_REPRESENTATION_ROTATION"));
    }
}

#[test]
fn structured_representation_references_must_resolve() {
    let mut placement = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.connectors[0].representation = Some(c::Representation::Primitive {
        primitive: c::PrimitiveRepresentation::Sphere { radius: 0.01 },
        placement: c::Placement {
            frame: c::RouteFrameRef::ComponentOrigin {
                component: c::ComponentRef::local("missing"),
            },
            xyz: [0.0, 0.0, 0.0],
            rotation: c::PlacementRotation::Rpy([0.0, 0.0, 0.0]),
        },
    });
    declare_component(&mut placement.scopes[0], device);
    let error = normalize_connectivity(&placement).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_UNRESOLVED_REFERENCE"));

    let mut component_root = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.connectors[0].representation =
        Some(c::Representation::ModelPart(c::ModelPartRepresentation {
            model_root: c::ModelRootRef::ComponentVisual {
                component: c::ComponentRef::local("missing"),
                visual: "housing".to_owned(),
            },
            node_path: "pins/1".to_owned(),
            submesh_fallback: None,
        }));
    declare_component(&mut component_root.scopes[0], device);
    let error = normalize_connectivity(&component_root).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_UNRESOLVED_REFERENCE"));

    let mut assembly_root = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.connectors[0].representation =
        Some(c::Representation::ModelPart(c::ModelPartRepresentation {
            model_root: c::ModelRootRef::AssemblyModel {
                assembly: c::AssemblyRef::local("missing"),
            },
            node_path: "contacts/1".to_owned(),
            submesh_fallback: None,
        }));
    declare_component(&mut assembly_root.scopes[0], device);
    let error = normalize_connectivity(&assembly_root).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_UNRESOLVED_REFERENCE"));

    let mut route = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.connectors[0].representation = Some(c::Representation::DerivedRoute(
        c::DerivedRouteRepresentation {
            waypoints: vec![
                c::RoutePoint {
                    frame: c::RouteFrameRef::World,
                    xyz: [0.0, 0.0, 0.0],
                    rotation: None,
                },
                c::RoutePoint {
                    frame: c::RouteFrameRef::ComponentOrigin {
                        component: c::ComponentRef::local("missing"),
                    },
                    xyz: [1.0, 0.0, 0.0],
                    rotation: None,
                },
            ],
            section: None,
        },
    ));
    declare_component(&mut route.scopes[0], device);
    let error = normalize_connectivity(&route).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_UNRESOLVED_REFERENCE"));
}

fn representation_document(representation: c::Representation) -> c::ConnectivityDocument {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.connectors[0].representation = Some(representation);
    declare_component(&mut document.scopes[0], device);
    document
}

fn world_placement() -> c::Placement {
    c::Placement {
        frame: c::RouteFrameRef::World,
        xyz: [0.0, 0.0, 0.0],
        rotation: c::PlacementRotation::Rpy([0.0, 0.0, 0.0]),
    }
}

fn derived_route(section: Option<c::RouteSection>) -> c::Representation {
    let rotation = matches!(&section, Some(c::RouteSection::Rectangular { .. })).then_some(
        c::PlacementRotation::Rpy([0.0, std::f64::consts::FRAC_PI_2, 0.0]),
    );
    c::Representation::DerivedRoute(c::DerivedRouteRepresentation {
        waypoints: vec![
            c::RoutePoint {
                frame: c::RouteFrameRef::World,
                xyz: [0.0, 0.0, 0.0],
                rotation: rotation.clone(),
            },
            c::RoutePoint {
                frame: c::RouteFrameRef::World,
                xyz: [1.0, 0.0, 0.0],
                rotation,
            },
        ],
        section,
    })
}

#[test]
fn canonical_route_sections_require_finite_positive_dimensions() {
    for section in [
        c::RouteSection::Round { diameter: 0.01 },
        c::RouteSection::Rectangular {
            width: 0.02,
            height: 0.01,
        },
    ] {
        normalize_connectivity(&representation_document(derived_route(Some(section)))).unwrap();
    }

    for section in [
        c::RouteSection::Round { diameter: 0.0 },
        c::RouteSection::Round { diameter: f64::NAN },
        c::RouteSection::Rectangular {
            width: -0.01,
            height: 0.01,
        },
        c::RouteSection::Rectangular {
            width: 0.01,
            height: f64::INFINITY,
        },
    ] {
        let error = normalize_connectivity(&representation_document(derived_route(Some(section))))
            .unwrap_err();
        assert!(error
            .issues()
            .iter()
            .any(|issue| issue.code() == "E_CONN_ROUTE_SECTION"));
    }
}

fn route_mut(representation: &mut c::Representation) -> &mut c::DerivedRouteRepresentation {
    let c::Representation::DerivedRoute(route) = representation else {
        panic!("expected a derived-route representation");
    };
    route
}

fn issue_codes(error: &hcdformat::connectivity::NormalizationError) -> Vec<&str> {
    error.issues().iter().map(|issue| issue.code()).collect()
}

#[test]
fn rectangular_route_orientation_is_exact_and_tangent_checked() {
    let rectangular = || {
        derived_route(Some(c::RouteSection::Rectangular {
            width: 0.02,
            height: 0.01,
        }))
    };

    let mut missing = rectangular();
    route_mut(&mut missing).waypoints[1].rotation = None;
    let error = normalize_connectivity(&representation_document(missing)).unwrap_err();
    assert!(issue_codes(&error).contains(&"E_CONN_ROUTE_ORIENTATION"));

    for section in [Some(c::RouteSection::Round { diameter: 0.01 }), None] {
        let mut disallowed = derived_route(section);
        route_mut(&mut disallowed).waypoints[0].rotation =
            Some(c::PlacementRotation::Rpy([0.0, 0.0, 0.0]));
        let error = normalize_connectivity(&representation_document(disallowed)).unwrap_err();
        assert!(issue_codes(&error).contains(&"E_CONN_ROUTE_ORIENTATION"));
    }

    let mut invalid_quaternion = rectangular();
    route_mut(&mut invalid_quaternion).waypoints[0].rotation =
        Some(c::PlacementRotation::Quaternion([0.0, 0.0, 0.0, 2.0]));
    let error = normalize_connectivity(&representation_document(invalid_quaternion)).unwrap_err();
    assert!(issue_codes(&error).contains(&"E_CONN_REPRESENTATION_ROTATION"));

    let mut non_tangent = rectangular();
    for waypoint in &mut route_mut(&mut non_tangent).waypoints {
        waypoint.rotation = Some(c::PlacementRotation::Rpy([0.0, 0.0, 0.0]));
    }
    let error = normalize_connectivity(&representation_document(non_tangent)).unwrap_err();
    assert!(issue_codes(&error).contains(&"E_CONN_ROUTE_TANGENT"));

    let mut zero_length = rectangular();
    route_mut(&mut zero_length).waypoints[1].xyz = [0.0, 0.0, 0.0];
    let error = normalize_connectivity(&representation_document(zero_length)).unwrap_err();
    assert!(issue_codes(&error).contains(&"E_CONN_ROUTE_TANGENT"));
}

#[test]
fn cross_frame_route_tangent_validation_is_deferred() {
    let mut representation = derived_route(Some(c::RouteSection::Rectangular {
        width: 0.02,
        height: 0.01,
    }));
    route_mut(&mut representation).waypoints[1].frame = c::RouteFrameRef::ComponentOrigin {
        component: c::ComponentRef::local("device"),
    };
    normalize_connectivity(&representation_document(representation)).unwrap();
}

#[test]
fn canonical_model_representation_text_values_must_not_be_empty() {
    let valid = c::Representation::Model {
        model: c::ModelRepresentation {
            uri: "package://robot/connector.glb".to_owned(),
            sha: Some("sha256:abc".to_owned()),
            node_path: Some("connector".to_owned()),
        },
        placement: world_placement(),
    };
    normalize_connectivity(&representation_document(valid)).unwrap();

    let invalid_models = [
        c::ModelRepresentation {
            uri: "".to_owned(),
            sha: None,
            node_path: None,
        },
        c::ModelRepresentation {
            uri: "package://robot/connector.glb".to_owned(),
            sha: Some(" ".to_owned()),
            node_path: None,
        },
        c::ModelRepresentation {
            uri: "package://robot/connector.glb".to_owned(),
            sha: None,
            node_path: Some("".to_owned()),
        },
    ];
    for model in invalid_models {
        let error = normalize_connectivity(&representation_document(c::Representation::Model {
            model,
            placement: world_placement(),
        }))
        .unwrap_err();
        assert!(error
            .issues()
            .iter()
            .any(|issue| issue.code() == "E_CONN_REPRESENTATION_LEXICAL"));
    }

    for (node_path, submesh_fallback) in [("", None), ("pins/1", Some(" ".to_owned()))] {
        let model_part = c::Representation::ModelPart(c::ModelPartRepresentation {
            model_root: c::ModelRootRef::ComponentVisual {
                component: c::ComponentRef::local("missing"),
                visual: "housing".to_owned(),
            },
            node_path: node_path.to_owned(),
            submesh_fallback,
        });
        let error = normalize_connectivity(&representation_document(model_part)).unwrap_err();
        assert!(error
            .issues()
            .iter()
            .any(|issue| issue.code() == "E_CONN_REPRESENTATION_LEXICAL"));
    }
}

fn authored_representation(fragment: &str) -> hcdformat::Hcdf {
    let xml = format!(
        r#"<hcdf name="d" version="1.0"><comp name="device"><connector name="J"><representation>{fragment}</representation></connector></comp></hcdf>"#
    );
    hcdformat::Hcdf::from_xml_str(&xml).unwrap()
}

#[test]
fn xml_authored_wrappers_and_representation_are_exact_choices() {
    let source = concat!(
        r#"<hcdf name="d" version="1.0"><comp name="device">"#,
        r#"<port name="p"><channel name="c"/></port>"#,
        r#"<connector name="J"><pin name="1"><representation>"#,
        r#"<sphere radius="0.005"><placement xyz="0 0 0"><frame><component-frame component="device" frame="mount"/></frame><rotation><rpy value="0 0 0"/></rotation></placement></sphere>"#,
        r#"</representation></pin></connector>"#,
        r#"<switch name="router"><input><port-ref component="device" port="p"/></input>"#,
        r#"<output><channel-ref component="device" port="p" channel="c"/></output></switch>"#,
        r#"</comp><binding name="b" fidelity="exact">"#,
        r#"<functional><channel-ref component="device" port="p" channel="c"/></functional>"#,
        r#"<physical><position-ref connector="J" position="1"><component-ref component="device"/></position-ref></physical>"#,
        r#"</binding></hcdf>"#,
    );
    let authored = hcdformat::Hcdf::from_xml_str(source).unwrap();
    assert!(matches!(
        authored.comp[0].connector[0].pin[0]
            .representation
            .as_ref()
            .unwrap()
            .variant,
        hcdformat::model::RepresentationChoice::Sphere(_)
    ));
    assert!(matches!(
        authored.comp[0].switch[0].input[0].endpoint,
        hcdformat::model::FunctionalEndpointChoice::Port(_)
    ));
    assert!(matches!(
        authored.binding[0].physical.endpoint,
        hcdformat::model::PhysicalEndpointChoice::Position(_)
    ));

    let serialized = authored.to_xml_string().unwrap();
    let sphere = serialized.find("<sphere radius=\"0.005\"").unwrap();
    let placement = sphere + serialized[sphere..].find("<placement").unwrap();
    let sphere_end = placement + serialized[placement..].find("</sphere>").unwrap();
    assert!(sphere < placement && placement < sphere_end);
    assert!(serialized.contains("<functional>"));
    assert!(serialized.contains("<channel-ref"));
    assert!(serialized.contains("<physical>"));
    assert!(serialized.contains("<position-ref"));
    assert_eq!(
        hcdformat::Hcdf::from_xml_str(&serialized).unwrap(),
        authored
    );

    for invalid in [
        r#"<hcdf name="d" version="1.0"><comp name="device"><connector name="J"><representation><sphere radius="1"><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/></rotation></placement></sphere><model-part node-path="n"><model-root><component-visual component="device" visual="v"/></model-root></model-part></representation></connector></comp></hcdf>"#,
        r#"<hcdf name="d" version="1.0"><comp name="device"><switch name="s"><input><port-ref component="device" port="p"/><channel-ref component="device" port="p" channel="c"/></input></switch></comp></hcdf>"#,
    ] {
        assert!(
            hcdformat::Hcdf::from_xml_str(invalid).is_err(),
            "multiple authored choices must fail: {invalid}"
        );
    }
}

#[test]
fn xml_physical_owners_are_exact_direct_reference_choices() {
    let source = concat!(
        r#"<hcdf name="d" version="1.0">"#,
        r#"<binding name="position" fidelity="exact"><functional><port-ref component="device" port="p"/></functional>"#,
        r#"<physical><position-ref connector="J" position="1"><component-ref component="device"/></position-ref></physical></binding>"#,
        r#"<binding name="connector" fidelity="exact"><functional><port-ref component="device" port="p"/></functional>"#,
        r#"<physical><connector-ref connector="H"><assembly-ref assembly="loom"><instance><segment name="child" occurrence="0"/></instance></assembly-ref></connector-ref></physical></binding>"#,
        r#"<binding name="junction" fidelity="exact"><functional><port-ref component="device" port="p"/></functional>"#,
        r#"<physical><junction-ref junction="S"><assembly-ref assembly="loom"/></junction-ref></physical></binding>"#,
        r#"<mate name="m" fidelity="exact"><first connector="J"><component-ref component="device"/></first>"#,
        r#"<second connector="H"><assembly-ref assembly="loom"/></second><position-mapping>"#,
        r#"<first connector="J" position="1"><component-ref component="device"/></first>"#,
        r#"<second connector="H" position="A"><assembly-ref assembly="loom"/></second>"#,
        r#"</position-mapping></mate></hcdf>"#,
    );
    let authored = hcdformat::Hcdf::from_xml_str(source).unwrap();

    let hcdformat::model::PhysicalEndpointChoice::Position(position) =
        &authored.binding[0].physical.endpoint
    else {
        panic!("expected position reference");
    };
    assert!(matches!(
        position.owner,
        hcdformat::model::PhysicalOwnerChoice::Component(_)
    ));
    let hcdformat::model::PhysicalEndpointChoice::Connector(connector) =
        &authored.binding[1].physical.endpoint
    else {
        panic!("expected connector reference");
    };
    let hcdformat::model::PhysicalOwnerChoice::Assembly(assembly) = &connector.owner else {
        panic!("expected assembly owner");
    };
    assert_eq!(assembly.assembly, "loom");
    assert_eq!(assembly.instance.as_ref().unwrap().segment.len(), 1);
    assert!(matches!(
        authored.binding[2].physical.endpoint,
        hcdformat::model::PhysicalEndpointChoice::Junction(_)
    ));
    assert!(matches!(
        authored.mate[0].first.owner,
        hcdformat::model::PhysicalOwnerChoice::Component(_)
    ));
    assert!(matches!(
        authored.mate[0].second.owner,
        hcdformat::model::PhysicalOwnerChoice::Assembly(_)
    ));

    let serialized = authored.to_xml_string().unwrap();
    assert!(!serialized.contains("owner-kind"), "{serialized}");
    assert!(serialized.contains(r#"<position-ref connector="J" position="1">"#));
    assert!(serialized.contains(r#"<component-ref component="device"/>"#));
    assert!(serialized.contains(r#"<connector-ref connector="H">"#));
    assert!(serialized.contains(r#"<assembly-ref assembly="loom">"#));
    assert_eq!(
        hcdformat::Hcdf::from_xml_str(&serialized).unwrap(),
        authored
    );
}

#[test]
fn xml_physical_owner_choice_rejects_retired_missing_and_multiple_arms() {
    let invalid = [
        r#"<connector-ref owner-kind="component" component="device" connector="J"/>"#,
        r#"<connector-ref connector="J"/>"#,
        r#"<connector-ref connector="J"><component-ref component="device"/><assembly-ref assembly="loom"/></connector-ref>"#,
        r#"<position-ref connector="J" position="1"><component-ref component="device"/><component-ref component="other"/></position-ref>"#,
        r#"<junction-ref junction="S"><port-ref component="device" port="p"/></junction-ref>"#,
    ];
    for physical in invalid {
        let source = format!(
            r#"<hcdf name="d" version="1.0"><binding name="b" fidelity="exact"><functional><port-ref component="device" port="p"/></functional><physical>{physical}</physical></binding></hcdf>"#
        );
        assert!(
            hcdformat::Hcdf::from_xml_str(&source).is_err(),
            "invalid physical owner choice was accepted: {physical}"
        );
    }
}

#[test]
fn xml_physical_reference_fields_must_not_be_empty() {
    let invalid = [
        r#"<connector-ref connector="J"><component-ref component=""/></connector-ref>"#,
        r#"<connector-ref connector="J"><assembly-ref assembly=" "/></connector-ref>"#,
        r#"<connector-ref connector=""><component-ref component="device"/></connector-ref>"#,
        r#"<position-ref connector="J" position=""><component-ref component="device"/></position-ref>"#,
        r#"<junction-ref junction=" "><component-ref component="device"/></junction-ref>"#,
    ];
    for physical in invalid {
        let source = format!(
            r#"<hcdf name="d" version="1.0"><binding name="b" fidelity="exact"><functional><port-ref component="device" port="p"/></functional><physical>{physical}</physical></binding></hcdf>"#
        );
        let error = hcdformat::Hcdf::from_xml_str(&source)
            .unwrap()
            .to_connectivity_document(document_identity())
            .unwrap_err();
        assert_eq!(error.code(), "E_CONN_EMPTY_REFERENCE", "{physical}");
    }
}

#[test]
fn xml_physical_owner_and_target_references_must_resolve() {
    let invalid = [
        (
            r#"<connector-ref connector="J"><component-ref component="missing"/></connector-ref>"#,
            "E_CONN_BINDING_OWNER",
        ),
        (
            r#"<connector-ref connector="missing"><component-ref component="device"/></connector-ref>"#,
            "E_CONN_UNRESOLVED_REFERENCE",
        ),
        (
            r#"<connector-ref connector="H"><assembly-ref assembly="missing"/></connector-ref>"#,
            "E_CONN_BINDING_OWNER",
        ),
    ];
    for (physical, expected_code) in invalid {
        let source = format!(
            r#"<hcdf name="d" version="1.0"><comp name="device"><port name="p"/><connector name="J"/></comp><binding name="b" fidelity="exact"><functional><port-ref component="device" port="p"/></functional><physical>{physical}</physical></binding></hcdf>"#
        );
        let authored = hcdformat::Hcdf::from_xml_str(&source)
            .unwrap()
            .to_connectivity_document(document_identity())
            .unwrap();
        let error = normalize_connectivity(&authored).unwrap_err();
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.code() == expected_code),
            "dangling physical reference lacked {expected_code}: {physical}"
        );
    }
}

#[test]
fn xml_model_root_frame_and_rotation_payloads_are_exact_choices() {
    let valid = [
        r#"<model-part node-path="pins"><model-root><component-visual component="device" visual="housing"/></model-root></model-part>"#,
        r#"<model-part node-path="pins"><model-root><assembly-model assembly="harness"/></model-root></model-part>"#,
        r#"<sphere radius="1"><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/></rotation></placement></sphere>"#,
        r#"<sphere radius="1"><placement xyz="0 0 0"><frame><component-origin component="device"/></frame><rotation><quaternion value="0 0 0 1"/></rotation></placement></sphere>"#,
        r#"<sphere radius="1"><placement xyz="0 0 0"><frame><component-frame component="device" frame="mount"/></frame><rotation><rpy value="0 0 0"/></rotation></placement></sphere>"#,
    ];
    for fragment in valid {
        let authored = authored_representation(fragment);
        let serialized = authored.to_xml_string().unwrap();
        assert_eq!(
            hcdformat::Hcdf::from_xml_str(&serialized).unwrap(),
            authored,
            "round-trip failed for {fragment}"
        );
    }

    let invalid = [
        r#"<model-part node-path="pins"><model-root/></model-part>"#,
        r#"<model-part node-path="pins"><model-root><component-visual component="device" visual="housing"/><assembly-model assembly="harness"/></model-root></model-part>"#,
        r#"<sphere radius="1"><placement xyz="0 0 0"><frame/><rotation><rpy value="0 0 0"/></rotation></placement></sphere>"#,
        r#"<sphere radius="1"><placement xyz="0 0 0"><frame><world/><component-origin component="device"/></frame><rotation><rpy value="0 0 0"/></rotation></placement></sphere>"#,
        r#"<sphere radius="1"><placement xyz="0 0 0"><frame><world/></frame><rotation/></placement></sphere>"#,
        r#"<sphere radius="1"><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/><quaternion value="0 0 0 1"/></rotation></placement></sphere>"#,
    ];
    for fragment in invalid {
        let source = format!(
            r#"<hcdf name="d" version="1.0"><comp name="device"><connector name="J"><representation>{fragment}</representation></connector></comp></hcdf>"#
        );
        assert!(
            hcdformat::Hcdf::from_xml_str(&source).is_err(),
            "zero or multiple payload arms must fail: {fragment}"
        );
    }
}
#[test]
fn xml_route_section_is_an_exact_typed_choice() {
    let cases = [
        (
            r#"<derived-route><round-section diameter="0.01"/><waypoint xyz="0 0 0"><frame><world/></frame></waypoint><waypoint xyz="1 0 0"><frame><world/></frame></waypoint></derived-route>"#,
            "<round-section diameter=\"0.01\"",
            c::RouteSection::Round { diameter: 0.01 },
        ),
        (
            r#"<derived-route><rectangular-section width="0.02" height="0.01"/><waypoint xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 1.5707963267948966 0"/></rotation></waypoint><waypoint xyz="1 0 0"><frame><world/></frame><rotation><quaternion value="0 0.7071067811865476 0 0.7071067811865476"/></rotation></waypoint></derived-route>"#,
            "<rectangular-section width=\"0.02\" height=\"0.01\"",
            c::RouteSection::Rectangular {
                width: 0.02,
                height: 0.01,
            },
        ),
    ];
    for (fragment, serialized_section, expected) in cases {
        let authored = authored_representation(fragment);
        let serialized = authored.to_xml_string().unwrap();
        assert!(serialized.contains(serialized_section), "{serialized}");
        assert!(!serialized.contains("<derived-route section="));
        let canonical = authored
            .to_connectivity_document(document_identity())
            .unwrap();
        let Some(c::Representation::DerivedRoute(route)) = canonical.scopes[0].components[0]
            .connectors[0]
            .representation
            .as_ref()
        else {
            panic!("expected a derived-route representation");
        };
        assert_eq!(route.section.as_ref(), Some(&expected));
        match expected {
            c::RouteSection::Rectangular { .. } => {
                assert_eq!(serialized.matches("<rotation>").count(), 2);
                assert!(route
                    .waypoints
                    .iter()
                    .all(|waypoint| waypoint.rotation.is_some()));
            }
            c::RouteSection::Round { .. } => {
                assert!(!serialized.contains("<rotation>"));
                assert!(route
                    .waypoints
                    .iter()
                    .all(|waypoint| waypoint.rotation.is_none()));
            }
        }
        normalize_connectivity(&canonical).unwrap();
    }

    let duplicate = format!(
        r#"<hcdf name="d" version="1.0"><comp name="device"><connector name="J"><representation>{}</representation></connector></comp></hcdf>"#,
        r#"<derived-route><round-section diameter="0.01"/><rectangular-section width="0.02" height="0.01"/><waypoint xyz="0 0 0"><frame><world/></frame></waypoint><waypoint xyz="1 0 0"><frame><world/></frame></waypoint></derived-route>"#,
    );
    let error = hcdformat::Hcdf::from_xml_str(&duplicate)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("at most one round-section or rectangular-section"),
        "unexpected parse error: {error}"
    );

    let duplicate_rotation = r#"<hcdf name="d" version="1.0"><comp name="device"><connector name="J"><representation><derived-route><rectangular-section width="0.02" height="0.01"/><waypoint xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 1.5707963267948966 0"/></rotation><rotation><quaternion value="0 0.7071067811865476 0 0.7071067811865476"/></rotation></waypoint><waypoint xyz="1 0 0"><frame><world/></frame><rotation><rpy value="0 1.5707963267948966 0"/></rotation></waypoint></derived-route></representation></connector></comp></hcdf>"#;
    assert!(hcdformat::Hcdf::from_xml_str(duplicate_rotation).is_err());

    let invalid = authored_representation(
        r#"<derived-route><round-section diameter="0"/><waypoint xyz="0 0 0"><frame><world/></frame></waypoint><waypoint xyz="1 0 0"><frame><world/></frame></waypoint></derived-route>"#,
    );
    let error = invalid
        .to_connectivity_document(document_identity())
        .unwrap_err();
    assert!(error
        .reason()
        .contains("route section dimensions must be finite and greater than zero"));
}

#[test]
fn xml_model_representation_text_values_must_not_be_empty() {
    for fragment in [
        r#"<model uri=""><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/></rotation></placement></model>"#,
        r#"<model uri="package://robot/connector.glb" sha=""><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/></rotation></placement></model>"#,
        r#"<model uri="package://robot/connector.glb" node-path=""><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/></rotation></placement></model>"#,
        r#"<model-part node-path="pins" submesh-fallback=""><model-root><component-visual component="device" visual="housing"/></model-root></model-part>"#,
    ] {
        let authored = authored_representation(fragment);
        let error = authored
            .to_connectivity_document(document_identity())
            .unwrap_err();
        assert!(
            error.reason().contains("must not be empty"),
            "unexpected error for {fragment}: {error}"
        );
    }
}

#[test]
fn quantity_facts_are_finite_unit_bearing_and_ordered() {
    let mut valid = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.ports[0].capabilities.limits.voltage = Some(c::QuantityRange {
        minimum: Some(c::Quantity {
            value: 20.0,
            unit: "V".to_owned(),
        }),
        nominal: Some(c::Quantity {
            value: 24.0,
            unit: "V".to_owned(),
        }),
        maximum: Some(c::Quantity {
            value: 28.0,
            unit: "V".to_owned(),
        }),
    });
    declare_component(&mut valid.scopes[0], device);
    normalize_connectivity(&valid).unwrap();

    let mut invalid = valid;
    let range = invalid.scopes[0].components[0].ports[0]
        .capabilities
        .limits
        .voltage
        .as_mut()
        .unwrap();
    range.minimum.as_mut().unwrap().unit.clear();
    range.maximum.as_mut().unwrap().value = f64::INFINITY;
    invalid.scopes[0].components[0].ports[0]
        .capabilities
        .limits
        .current = Some(quantity_range(2.0, 1.0, "A"));
    let error = normalize_connectivity(&invalid).unwrap_err();
    for code in ["E_CONN_QUANTITY_VALUE", "E_CONN_UNIT", "E_CONN_RANGE_ORDER"] {
        assert!(
            error.issues().iter().any(|issue| issue.code() == code),
            "missing {code}"
        );
    }
}

#[test]
fn connectivity_functions_require_two_endpoints() {
    let mut invalid = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.functions.push(c::ConnectivityFunction {
        name: "lonely".to_owned(),
        kind: c::FunctionKind::Switch,
        inputs: Vec::new(),
        outputs: Vec::new(),
        bidirectional: vec![c::FunctionalEndpointRef::Port(c::PortRef::local(
            "device", "bus",
        ))],
    });
    declare_component(&mut invalid.scopes[0], device);
    let error = normalize_connectivity(&invalid).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_FUNCTION_ARITY"));

    let mut valid = invalid;
    valid.scopes[0].components[0].functions[0]
        .bidirectional
        .push(c::FunctionalEndpointRef::Channel(c::ChannelRef::local(
            "device", "bus", "signal",
        )));
    normalize_connectivity(&valid).unwrap();
}

#[test]
fn duplicate_profile_declarations_are_not_silently_collapsed() {
    for xml in [
        r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p">
          <capabilities><profile id="hcdf:test"/><profile id="hcdf:test"/></capabilities>
        </port></comp></hcdf>"#,
        r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"/></comp>
          <comp name="b"><port name="p"/></comp><link name="n">
            <selected purpose="communication" carrier="electrical"><profile id="hcdf:test"/><profile id="hcdf:test"/></selected>
            <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
          </link></hcdf>"#,
    ] {
        let document = hcdformat::Hcdf::from_xml_str(xml).unwrap();
        let error = document
            .to_connectivity_document(document_identity())
            .unwrap_err();
        assert!(error.reason().contains("duplicate profile declaration"));
    }
}

fn quantity_range(minimum: f64, maximum: f64, unit: &str) -> c::QuantityRange {
    c::QuantityRange {
        minimum: Some(c::Quantity {
            value: minimum,
            unit: unit.to_owned(),
        }),
        nominal: None,
        maximum: Some(c::Quantity {
            value: maximum,
            unit: unit.to_owned(),
        }),
    }
}

fn selected_quantity_document(value: f64, unit: &str) -> c::ConnectivityDocument {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut first = component("first");
    let mut second = component("second");
    for component in [&mut first, &mut second] {
        component.ports[0].capabilities.limits.voltage = Some(quantity_range(20.0, 28.0, "V"));
    }
    declare_components(&mut document.scopes[0], [first, second]);
    let mut link = network(
        "power",
        vec![
            participant("first", "first", None),
            participant("second", "second", None),
        ],
    );
    link.selected.voltage = Some(c::SelectionQuantity::Nominal(c::Quantity {
        value,
        unit: unit.to_owned(),
    }));
    document.scopes[0].networks.push(link);
    document
}

fn nominal_only_voltage_document(value: f64, unit: &str) -> c::ConnectivityDocument {
    let mut document = selected_quantity_document(value, unit);
    for component in &mut document.scopes[0].components {
        component.ports[0].capabilities.limits.voltage = Some(c::QuantityRange {
            minimum: None,
            nominal: Some(c::Quantity {
                value: 24.0,
                unit: "V".to_owned(),
            }),
            maximum: None,
        });
    }
    document
}

#[test]
fn nominal_only_capabilities_are_exact_points_with_unit_and_ulp_tolerance() {
    normalize_connectivity(&nominal_only_voltage_document(24_000.0, "mV")).unwrap();

    let base = 24.0_f64.to_bits();
    for selected in [f64::from_bits(base - 8), f64::from_bits(base + 8)] {
        normalize_connectivity(&nominal_only_voltage_document(selected, "V")).unwrap();
    }

    for selected in [25.0, f64::from_bits(base - 9), f64::from_bits(base + 9)] {
        let error =
            normalize_connectivity(&nominal_only_voltage_document(selected, "V")).unwrap_err();
        assert!(error
            .issues()
            .iter()
            .any(|issue| issue.code() == "E_CONN_SELECTION_RANGE"));
    }
}

#[test]
fn selected_quantities_normalize_compatible_units_and_enforce_dimensions_and_ranges() {
    normalize_connectivity(&selected_quantity_document(24.0, "V")).unwrap();

    let error = normalize_connectivity(&selected_quantity_document(30.0, "V")).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_SELECTION_RANGE"));

    normalize_connectivity(&selected_quantity_document(24_000.0, "mV")).unwrap();

    let error = normalize_connectivity(&selected_quantity_document(24.0, "A")).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_DIMENSION"));
}

#[test]
fn empty_quantity_ranges_are_rejected() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.ports[0].capabilities.limits.rate = Some(c::QuantityRange::default());
    declare_component(&mut document.scopes[0], device);
    let error = normalize_connectivity(&document).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_RANGE_EMPTY"));
}

#[test]
fn public_network_validation_propagates_warning_severity() {
    let document = hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0">
          <comp name="a"><port name="p"/></comp>
          <comp name="b"><port name="p"/></comp>
          <link name="n">
            <selected purpose="communication" carrier="electrical"><profile id="hcdf:test"/></selected>
            <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
          </link>
        </hcdf>"#,
    )
    .unwrap();
    let issues = hcdformat::validate_network(&document);
    assert!(!issues.is_empty());
    assert!(issues
        .iter()
        .all(|issue| issue.level == hcdformat::Level::Warning));
    assert!(issues
        .iter()
        .any(|issue| issue.code == "W_CONN_SELECTION_UNVERIFIED"));
    assert_eq!(
        issues
            .iter()
            .filter(|issue| issue.code == "W_CONN_PROFILE_INCOMPLETE")
            .count(),
        1
    );
}

fn structural_anchors(
    component: &str,
    visuals: &[(&str, bool)],
    frames: &[&str],
) -> c::StructuralAnchors {
    c::StructuralAnchors {
        component: component.to_owned(),
        visuals: visuals
            .iter()
            .map(|(name, model_backed)| c::StructuralVisual {
                name: (*name).to_owned(),
                model_backed: *model_backed,
            })
            .collect(),
        frames: frames.iter().map(|name| (*name).to_owned()).collect(),
    }
}

#[test]
fn converted_structural_visuals_and_frames_have_exact_resolver_nodes() {
    let authored = hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><comp name="device">
          <visual name="housing"><model uri="assets/device.glb"/></visual>
          <visual name="primitive"/>
          <frame name="mount"/>
          <connector name="part"><representation>
            <model-part node-path="pins"><model-root><component-visual component="device" visual="housing"/></model-root></model-part>
          </representation></connector>
          <connector name="placed"><representation>
            <sphere radius="0.01"><placement xyz="0 0 0"><frame><component-frame component="device" frame="mount"/></frame><rotation><rpy value="0 0 0"/></rotation></placement></sphere>
          </representation></connector>
        </comp></hcdf>"#,
    )
    .unwrap();
    let canonical = authored
        .to_connectivity_document(document_identity())
        .unwrap();
    let anchors = &canonical.scopes[0].structural_anchors[0];
    assert_eq!(anchors.component, "device");
    assert_eq!(anchors.visuals.len(), 2);
    assert!(anchors.visuals[0].model_backed);
    assert!(!anchors.visuals[1].model_backed);
    assert_eq!(anchors.frames, ["mount"]);

    let graph = normalize_connectivity(&canonical).unwrap();
    let resolver = graph.resolver(c::IncludeInstanceId::root());
    let housing = resolver
        .visual_root(&c::StructuralVisualRef::local("device", "housing"))
        .unwrap();
    let primitive = resolver
        .visual_root(&c::StructuralVisualRef::local("device", "primitive"))
        .unwrap();
    let mount = resolver
        .frame(&c::StructuralFrameRef::local("device", "mount"))
        .unwrap();
    assert_eq!(housing.kind(), ObjectKind::StructuralVisualRoot);
    assert_eq!(primitive.kind(), ObjectKind::StructuralVisualRoot);
    assert_eq!(mount.kind(), ObjectKind::StructuralFrame);
    assert!(matches!(
        housing.data(),
        hcdformat::connectivity::ConnectivityNodeData::StructuralVisualRoot { model_backed: true }
    ));
    assert!(matches!(
        primitive.data(),
        hcdformat::connectivity::ConnectivityNodeData::StructuralVisualRoot {
            model_backed: false
        }
    ));

    let model_root = graph
        .edges()
        .iter()
        .find(|edge| edge.kind() == EdgeKind::RepresentationModelRoot)
        .unwrap();
    assert_eq!(model_root.from(), housing.id());
    let placement_frame = graph
        .edges()
        .iter()
        .find(|edge| edge.kind() == EdgeKind::RepresentationFrame)
        .unwrap();
    assert_eq!(placement_frame.from(), mount.id());
}

#[test]
fn primitive_visuals_and_missing_structural_frames_are_rejected() {
    let authored = hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><comp name="device">
          <visual name="primitive"/>
          <connector name="part"><representation>
            <model-part node-path="pins"><model-root><component-visual component="device" visual="primitive"/></model-root></model-part>
          </representation></connector>
        </comp></hcdf>"#,
    )
    .unwrap();
    let error = authored
        .to_connectivity_document(document_identity())
        .unwrap_err();
    assert!(error
        .reason()
        .contains("component-visual selector requires a visual backed by a model"));

    let mut canonical =
        representation_document(c::Representation::ModelPart(c::ModelPartRepresentation {
            model_root: c::ModelRootRef::ComponentVisual {
                component: c::ComponentRef::local("device"),
                visual: "primitive".to_owned(),
            },
            node_path: "pins".to_owned(),
            submesh_fallback: None,
        }));
    canonical.scopes[0].structural_anchors[0] =
        structural_anchors("device", &[("primitive", false)], &[]);
    let error = normalize_connectivity(&canonical).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_MODEL_ROOT_TYPE"));

    let missing_frame = representation_document(c::Representation::Primitive {
        primitive: c::PrimitiveRepresentation::Sphere { radius: 0.01 },
        placement: c::Placement {
            frame: c::RouteFrameRef::ComponentFrame {
                component: c::ComponentRef::local("device"),
                frame: "missing".to_owned(),
            },
            xyz: [0.0, 0.0, 0.0],
            rotation: c::PlacementRotation::Rpy([0.0, 0.0, 0.0]),
        },
    });
    let error = normalize_connectivity(&missing_frame).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_UNRESOLVED_REFERENCE"));
}

#[test]
fn repeated_instances_resolve_distinct_structural_visuals_and_frames() {
    let root = c::IncludeInstanceId::root();
    let left = root.child("module", 0);
    let right = root.child("module", 1);
    let mut document = c::ConnectivityDocument::new(document_identity());
    for instance in [left.clone(), right.clone()] {
        let mut scope = c::ConnectivityScope::new(instance);
        scope.components.push(component("device"));
        scope.structural_anchors.push(structural_anchors(
            "device",
            &[("housing", true)],
            &["mount"],
        ));
        document.scopes.push(scope);
    }
    let graph = normalize_connectivity(&document).unwrap();
    let resolver = graph.resolver(root);
    let left_visual = resolver
        .visual_root(&c::StructuralVisualRef::in_instance(
            left.clone(),
            "device",
            "housing",
        ))
        .unwrap();
    let right_visual = resolver
        .visual_root(&c::StructuralVisualRef::in_instance(
            right.clone(),
            "device",
            "housing",
        ))
        .unwrap();
    let left_frame = resolver
        .frame(&c::StructuralFrameRef::in_instance(left, "device", "mount"))
        .unwrap();
    let right_frame = resolver
        .frame(&c::StructuralFrameRef::in_instance(
            right, "device", "mount",
        ))
        .unwrap();
    assert_ne!(left_visual.id(), right_visual.id());
    assert_ne!(left_frame.id(), right_frame.id());
}

#[test]
fn structural_catalog_owner_and_uniqueness_are_validated() {
    let mut duplicate = c::ConnectivityDocument::new(document_identity());
    duplicate.scopes[0].components.push(component("device"));
    duplicate.scopes[0]
        .structural_anchors
        .push(structural_anchors("device", &[], &[]));
    duplicate.scopes[0]
        .structural_anchors
        .push(structural_anchors("device", &[], &[]));
    let error = normalize_connectivity(&duplicate).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_DUPLICATE_STRUCTURAL_CATALOG"));

    let mut missing_owner = c::ConnectivityDocument::new(document_identity());
    missing_owner.scopes[0]
        .structural_anchors
        .push(structural_anchors("ghost", &[("housing", true)], &[]));
    let error = normalize_connectivity(&missing_owner).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_STRUCTURAL_OWNER"));
}

#[test]
fn assembly_model_roots_target_the_standalone_representation_node() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.connectors[0].representation =
        Some(c::Representation::ModelPart(c::ModelPartRepresentation {
            model_root: c::ModelRootRef::AssemblyModel {
                assembly: c::AssemblyRef::local("harness"),
            },
            node_path: "contacts/1".to_owned(),
            submesh_fallback: None,
        }));
    declare_component(&mut document.scopes[0], device);
    document.scopes[0].assemblies.push(c::PhysicalAssembly {
        name: "harness".to_owned(),
        connectors: Vec::new(),
        kind: c::AssemblyKind::Harness,
        representation: Some(c::Representation::Model {
            model: c::ModelRepresentation {
                uri: "assets/harness.glb".to_owned(),
                sha: None,
                node_path: None,
            },
            placement: world_placement(),
        }),
        paths: Vec::new(),
        junctions: Vec::new(),
        terminations: Vec::new(),
    });

    let graph = normalize_connectivity(&document).unwrap();
    let assembly = graph
        .resolver(c::IncludeInstanceId::root())
        .assembly(&c::AssemblyRef::local("harness"))
        .unwrap();
    let primary = graph
        .edges()
        .iter()
        .find(|edge| edge.kind() == EdgeKind::Representation && edge.from() == assembly.id())
        .unwrap();
    let model_root = graph
        .edges()
        .iter()
        .find(|edge| edge.kind() == EdgeKind::RepresentationModelRoot)
        .unwrap();
    assert_eq!(model_root.from(), primary.to());
    assert_ne!(model_root.from(), assembly.id());
    assert_eq!(
        graph.node(model_root.from()).unwrap().kind(),
        ObjectKind::Representation
    );

    let mut invalid = document;
    invalid.scopes[0].assemblies[0].representation =
        Some(primitive_representation(c::PlacementRotation::Rpy([
            0.0, 0.0, 0.0,
        ])));
    let error = normalize_connectivity(&invalid).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_MODEL_ROOT_TYPE"));
}

#[test]
fn identity_strong_types_validate_during_deserialization() {
    let document: c::DocumentIdentity = serde_json::from_str(r#""memory://valid""#).unwrap();
    assert_eq!(document.as_str(), "memory://valid");
    assert!(serde_json::from_str::<c::DocumentIdentity>(r#"""#).is_err());

    let profile: c::QualifiedId = serde_json::from_str(r#""hcdf:rs-485""#).unwrap();
    assert_eq!(profile.as_str(), "hcdf:rs-485");
    assert!(serde_json::from_str::<c::QualifiedId>(r#""not qualified""#).is_err());
}

#[test]
fn include_instance_name_variants_have_exact_display_serde_and_hash_tags() {
    let root = c::IncludeInstanceId::root();
    let unnamed = root.unnamed_child(0);
    let reserved_text = root.named_child("$include", 0);
    let whitespace = root.named_child("   ", 0);

    assert_eq!(unnamed.display_path(), "unnamed#0");
    assert_eq!(reserved_text.display_path(), "named(\"$include\")#0");
    assert_eq!(whitespace.display_path(), "named(\"   \")#0");
    assert_eq!(unnamed.segments()[0], c::IncludeInstanceSegment::unnamed(0));
    assert_eq!(
        reserved_text.segments()[0],
        c::IncludeInstanceSegment::named("$include", 0)
    );

    assert_eq!(
        serde_json::to_value(c::IncludeSegmentName::unnamed()).unwrap(),
        serde_json::json!({"kind": "unnamed"})
    );
    assert_eq!(
        serde_json::to_value(c::IncludeSegmentName::named("$include")).unwrap(),
        serde_json::json!({"kind": "named", "value": "$include"})
    );
    assert_eq!(
        serde_json::to_value(&reserved_text).unwrap(),
        serde_json::json!([
            {
                "name": {"kind": "named", "value": "$include"},
                "occurrence": 0
            }
        ])
    );
    assert_eq!(
        serde_json::from_value::<c::IncludeInstanceId>(
            serde_json::to_value(&reserved_text).unwrap()
        )
        .unwrap(),
        reserved_text
    );

    let unnamed_id =
        ObjectIdentity::new(document_identity(), unnamed, ObjectKind::Scope, Vec::new())
            .stable_id();
    let named_id = ObjectIdentity::new(
        document_identity(),
        reserved_text,
        ObjectKind::Scope,
        Vec::new(),
    )
    .stable_id();
    assert_eq!(
        unnamed_id.as_str(),
        "hcdf-object-v1:fddbc7d644a43ff7c0dd60f44a57591ba72246f0f1d2a32e4f6480e94346282d"
    );
    assert_eq!(
        named_id.as_str(),
        "hcdf-object-v1:d3af6497ba1d348d18df9ea0e8cfb021f770fb3a501e338a5811ef77ebbbc862"
    );
    assert_ne!(unnamed_id, named_id);

    let document = c::ConnectivityDocument {
        document: document_identity(),
        scopes: vec![
            c::ConnectivityScope::root(),
            c::ConnectivityScope::new(whitespace),
        ],
    };
    normalize_connectivity(&document).unwrap();
}

#[test]
fn empty_explicit_instance_scopes_are_rejected_at_every_boundary() {
    let empty_scope_json = r#"{"scope":"instance","instance":[]}"#;
    assert!(serde_json::from_str::<c::ReferenceScope>(empty_scope_json).is_err());
    assert!(serde_json::from_str::<c::ComponentRef>(
        r#"{"scope":"instance","instance":[],"component":"device"}"#
    )
    .is_err());

    let nonempty = c::ReferenceScope::Instance(c::IncludeInstanceId::root().child("module", 0));
    let json = serde_json::to_string(&nonempty).unwrap();
    assert_eq!(
        serde_json::from_str::<c::ReferenceScope>(&json).unwrap(),
        nonempty
    );

    let empty_scope = || c::ReferenceScope::Instance(c::IncludeInstanceId::root());
    let mut documents = Vec::new();

    let mut network_document = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut network_document.scopes[0],
        [component("device"), component("peer")],
    );
    network_document.scopes[0].networks.push(network(
        "invalid-scope",
        vec![
            participant("device", "device", None),
            participant("peer", "peer", None),
        ],
    ));
    match &mut network_document.scopes[0].networks[0].participants[0].endpoint {
        c::FunctionalEndpointRef::Port(reference) => reference.scope = empty_scope(),
        c::FunctionalEndpointRef::Channel(_) => {
            unreachable!("helper created a whole-port endpoint")
        }
    }
    documents.push(network_document);

    let mut binding_document = c::ConnectivityDocument::new(document_identity());
    declare_component(&mut binding_document.scopes[0], component("device"));
    binding_document.scopes[0].bindings.push(c::Binding {
        name: "invalid-scope".to_owned(),
        functional: c::FunctionalEndpointRef::Port(c::PortRef::local("device", "bus")),
        physical: c::PhysicalEndpointRef::Connector(c::ConnectorRef {
            owner: c::OwnerRef::Component(c::ComponentRef {
                scope: empty_scope(),
                component: "device".to_owned(),
            }),
            connector: "J1".to_owned(),
        }),
        fidelity: c::Fidelity::Presented,
    });
    documents.push(binding_document);

    let mut function_document = c::ConnectivityDocument::new(document_identity());
    let mut function_component = component("device");
    function_component.functions.push(c::ConnectivityFunction {
        name: "invalid-scope".to_owned(),
        kind: c::FunctionKind::Bridge,
        inputs: vec![c::FunctionalEndpointRef::Port(c::PortRef {
            scope: empty_scope(),
            component: "device".to_owned(),
            port: "bus".to_owned(),
        })],
        outputs: vec![c::FunctionalEndpointRef::Channel(c::ChannelRef::local(
            "device", "bus", "signal",
        ))],
        bidirectional: Vec::new(),
    });
    declare_component(&mut function_document.scopes[0], function_component);
    documents.push(function_document);

    let mut antenna_document = c::ConnectivityDocument::new(document_identity());
    let mut antenna_component = component("device");
    antenna_component.antennas.push(c::Antenna {
        name: "invalid-scope".to_owned(),
        conducted_port: Some(c::PortRef::local("device", "bus")),
        radiated_port: c::PortRef {
            scope: empty_scope(),
            component: "device".to_owned(),
            port: "bus".to_owned(),
        },
        representation: None,
    });
    declare_component(&mut antenna_document.scopes[0], antenna_component);
    documents.push(antenna_document);

    documents.push(representation_document(c::Representation::Primitive {
        primitive: c::PrimitiveRepresentation::Sphere { radius: 0.01 },
        placement: c::Placement {
            frame: c::RouteFrameRef::ComponentOrigin {
                component: c::ComponentRef {
                    scope: empty_scope(),
                    component: "device".to_owned(),
                },
            },
            xyz: [0.0, 0.0, 0.0],
            rotation: c::PlacementRotation::Rpy([0.0, 0.0, 0.0]),
        },
    }));

    documents.push(representation_document(c::Representation::ModelPart(
        c::ModelPartRepresentation {
            model_root: c::ModelRootRef::AssemblyModel {
                assembly: c::AssemblyRef {
                    scope: empty_scope(),
                    assembly: "harness".to_owned(),
                },
            },
            node_path: "contacts".to_owned(),
            submesh_fallback: None,
        },
    )));

    let mut mate_document = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut mate_document.scopes[0],
        [component("first"), component("second")],
    );
    mate_document.scopes[0].mates.push(c::Mate {
        name: "invalid-scope".to_owned(),
        first: c::ConnectorRef {
            owner: c::OwnerRef::Component(c::ComponentRef {
                scope: empty_scope(),
                component: "first".to_owned(),
            }),
            connector: "J1".to_owned(),
        },
        second: c::ConnectorRef::local_component("second", "J1"),
        mappings: Vec::new(),
        fidelity: c::Fidelity::Presented,
    });
    documents.push(mate_document);

    for document in documents {
        let error = normalize_connectivity(&document).unwrap_err();
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.code() == "E_CONN_EMPTY_INSTANCE_SCOPE"),
            "programmatic empty instance scope was not rejected: {error:?}"
        );
    }
}

#[test]
fn scope_tree_requires_one_root_and_complete_parent_chain() {
    let no_scopes = c::ConnectivityDocument {
        document: document_identity(),
        scopes: Vec::new(),
    };
    let error = normalize_connectivity(&no_scopes).unwrap_err();
    assert!(issue_codes(&error).contains(&"E_CONN_SCOPE_ROOT"));

    let root = c::IncludeInstanceId::root();
    let child = root.child("module", 0);
    let grandchild = child.child("sensor", 0);
    assert_eq!(grandchild.parent(), Some(child.clone()));
    assert_eq!(child.parent(), Some(root.clone()));
    assert_eq!(root.parent(), None);

    let child_only = c::ConnectivityDocument {
        document: document_identity(),
        scopes: vec![c::ConnectivityScope::new(child.clone())],
    };
    let error = normalize_connectivity(&child_only).unwrap_err();
    assert!(issue_codes(&error).contains(&"E_CONN_SCOPE_ROOT"));
    assert!(issue_codes(&error).contains(&"E_CONN_SCOPE_PARENT"));

    let orphan = c::ConnectivityDocument {
        document: document_identity(),
        scopes: vec![
            c::ConnectivityScope::root(),
            c::ConnectivityScope::new(grandchild.clone()),
        ],
    };
    let error = normalize_connectivity(&orphan).unwrap_err();
    assert!(issue_codes(&error).contains(&"E_CONN_SCOPE_PARENT"));

    let complete = c::ConnectivityDocument {
        document: document_identity(),
        scopes: vec![
            c::ConnectivityScope::new(grandchild),
            c::ConnectivityScope::root(),
            c::ConnectivityScope::new(child),
        ],
    };
    normalize_connectivity(&complete).unwrap();

    let repeated_siblings = c::ConnectivityDocument {
        document: document_identity(),
        scopes: vec![
            c::ConnectivityScope::new(root.child("module", 1)),
            c::ConnectivityScope::root(),
            c::ConnectivityScope::new(root.child("module", 0)),
        ],
    };
    normalize_connectivity(&repeated_siblings).unwrap();
}

#[test]
fn every_component_requires_one_even_if_empty_structural_catalog() {
    let mut missing = c::ConnectivityDocument::new(document_identity());
    missing.scopes[0].components.push(component("device"));
    let error = normalize_connectivity(&missing).unwrap_err();
    assert!(issue_codes(&error).contains(&"E_CONN_MISSING_STRUCTURAL_CATALOG"));

    let mut empty = c::ConnectivityDocument::new(document_identity());
    declare_component(&mut empty.scopes[0], component("device"));
    assert!(empty.scopes[0].structural_anchors[0].visuals.is_empty());
    assert!(empty.scopes[0].structural_anchors[0].frames.is_empty());
    normalize_connectivity(&empty).unwrap();
}

#[test]
fn stable_ids_deserialize_only_the_exact_canonical_shape() {
    let graph = normalize_connectivity(&included_network_document()).unwrap();
    let object = graph.nodes()[0].id();
    let edge = graph.edges()[0].id();
    assert_eq!(
        serde_json::from_str::<StableObjectId>(&serde_json::to_string(object).unwrap()).unwrap(),
        *object
    );
    assert_eq!(
        serde_json::from_str::<StableEdgeId>(&serde_json::to_string(edge).unwrap()).unwrap(),
        *edge
    );

    let rejects_object = |value: &str| {
        let json = serde_json::to_string(value).unwrap();
        assert!(
            serde_json::from_str::<StableObjectId>(&json).is_err(),
            "accepted invalid object ID {value}"
        );
    };
    let rejects_edge = |value: &str| {
        let json = serde_json::to_string(value).unwrap();
        assert!(
            serde_json::from_str::<StableEdgeId>(&json).is_err(),
            "accepted invalid edge ID {value}"
        );
    };

    for value in [
        format!("wrong-object-v1:{}", "0".repeat(64)),
        format!("hcdf-object-v1:{}", "0".repeat(63)),
        format!("hcdf-object-v1:{}", "0".repeat(65)),
        format!("hcdf-object-v1:{}", "A".repeat(64)),
        format!("hcdf-object-v1:{}", "g".repeat(64)),
        format!("hcdf-object-v1:{}x", "0".repeat(64)),
    ] {
        rejects_object(&value);
    }
    for value in [
        format!("wrong-edge-v1:{}", "0".repeat(64)),
        format!("hcdf-edge-v1:{}", "0".repeat(63)),
        format!("hcdf-edge-v1:{}", "0".repeat(65)),
        format!("hcdf-edge-v1:{}", "A".repeat(64)),
        format!("hcdf-edge-v1:{}", "g".repeat(64)),
        format!("hcdf-edge-v1:{}x", "0".repeat(64)),
    ] {
        rejects_edge(&value);
    }
}

fn repeated_structural_reference_document() -> c::ConnectivityDocument {
    let root = c::IncludeInstanceId::root();
    let left = root.child("module", 0);
    let right = root.child("module", 1);
    let mut document = c::ConnectivityDocument::new(document_identity());

    let mut observer = component("observer");
    observer.connectors.clear();
    for (label, instance) in [("left", left.clone()), ("right", right.clone())] {
        observer.connectors.push(c::Connector {
            name: format!("{label}-visual"),
            family: None,
            positions: Vec::new(),
            representation: Some(c::Representation::ModelPart(c::ModelPartRepresentation {
                model_root: c::ModelRootRef::ComponentVisual {
                    component: c::ComponentRef::in_instance(instance.clone(), "device"),
                    visual: "housing".to_owned(),
                },
                node_path: "connector".to_owned(),
                submesh_fallback: None,
            })),
        });
        observer.connectors.push(c::Connector {
            name: format!("{label}-frame"),
            family: None,
            positions: Vec::new(),
            representation: Some(c::Representation::Primitive {
                primitive: c::PrimitiveRepresentation::Sphere { radius: 0.01 },
                placement: c::Placement {
                    frame: c::RouteFrameRef::ComponentFrame {
                        component: c::ComponentRef::in_instance(instance.clone(), "device"),
                        frame: "mount".to_owned(),
                    },
                    xyz: [0.0, 0.0, 0.0],
                    rotation: c::PlacementRotation::Rpy([0.0, 0.0, 0.0]),
                },
            }),
        });
        observer.connectors.push(c::Connector {
            name: format!("{label}-assembly"),
            family: None,
            positions: Vec::new(),
            representation: Some(c::Representation::ModelPart(c::ModelPartRepresentation {
                model_root: c::ModelRootRef::AssemblyModel {
                    assembly: c::AssemblyRef::in_instance(instance, "harness"),
                },
                node_path: "contacts".to_owned(),
                submesh_fallback: None,
            })),
        });
    }
    declare_component(&mut document.scopes[0], observer);

    for instance in [left, right] {
        let mut scope = c::ConnectivityScope::new(instance);
        scope.components.push(component("device"));
        scope.structural_anchors.push(structural_anchors(
            "device",
            &[("housing", true)],
            &["mount"],
        ));
        scope.assemblies.push(c::PhysicalAssembly {
            name: "harness".to_owned(),
            connectors: Vec::new(),
            kind: c::AssemblyKind::Harness,
            representation: Some(c::Representation::Model {
                model: c::ModelRepresentation {
                    uri: "assets/harness.glb".to_owned(),
                    sha: None,
                    node_path: None,
                },
                placement: world_placement(),
            }),
            paths: Vec::new(),
            junctions: Vec::new(),
            terminations: Vec::new(),
        });
        document.scopes.push(scope);
    }
    document
}

#[test]
fn parent_references_to_repeated_child_structural_targets_are_stable() {
    let first_document = repeated_structural_reference_document();
    let first = normalize_connectivity(&first_document).unwrap();
    let root = c::IncludeInstanceId::root();
    let left = root.child("module", 0);
    let right = root.child("module", 1);
    let resolver = first.resolver(root);

    let left_visual = resolver
        .visual_root(&c::StructuralVisualRef::in_instance(
            left.clone(),
            "device",
            "housing",
        ))
        .unwrap();
    let right_visual = resolver
        .visual_root(&c::StructuralVisualRef::in_instance(
            right.clone(),
            "device",
            "housing",
        ))
        .unwrap();
    let left_frame = resolver
        .frame(&c::StructuralFrameRef::in_instance(
            left.clone(),
            "device",
            "mount",
        ))
        .unwrap();
    let right_frame = resolver
        .frame(&c::StructuralFrameRef::in_instance(
            right.clone(),
            "device",
            "mount",
        ))
        .unwrap();
    let left_assembly = resolver
        .assembly(&c::AssemblyRef::in_instance(left, "harness"))
        .unwrap();
    let right_assembly = resolver
        .assembly(&c::AssemblyRef::in_instance(right, "harness"))
        .unwrap();

    assert_ne!(left_visual.id(), right_visual.id());
    assert_ne!(left_frame.id(), right_frame.id());
    assert_ne!(left_assembly.id(), right_assembly.id());

    let visual_sources = first
        .edges()
        .iter()
        .filter(|edge| edge.kind() == EdgeKind::RepresentationModelRoot)
        .filter(|edge| {
            first
                .node(edge.from())
                .is_some_and(|node| node.kind() == ObjectKind::StructuralVisualRoot)
        })
        .map(|edge| edge.from().as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let assembly_model_sources = first
        .edges()
        .iter()
        .filter(|edge| edge.kind() == EdgeKind::RepresentationModelRoot)
        .filter(|edge| {
            first
                .node(edge.from())
                .is_some_and(|node| node.kind() == ObjectKind::Representation)
        })
        .map(|edge| edge.from().as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let frame_sources = first
        .edges()
        .iter()
        .filter(|edge| edge.kind() == EdgeKind::RepresentationFrame)
        .map(|edge| edge.from().as_str().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(visual_sources.len(), 2);
    assert_eq!(assembly_model_sources.len(), 2);
    assert_eq!(frame_sources.len(), 2);

    let mut reordered = first_document;
    reordered.scopes.reverse();
    for scope in &mut reordered.scopes {
        scope.components.reverse();
        scope.structural_anchors.reverse();
        scope.assemblies.reverse();
        for component in &mut scope.components {
            component.connectors.reverse();
        }
    }
    assert_eq!(
        first.to_canonical_json().unwrap(),
        normalize_connectivity(&reordered)
            .unwrap()
            .to_canonical_json()
            .unwrap()
    );
}

#[test]
fn bindings_require_same_component_physical_ownership() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut document.scopes[0],
        [component("device"), component("peer")],
    );
    let binding = |name: &str, owner: c::OwnerRef| c::Binding {
        name: name.to_owned(),
        functional: c::FunctionalEndpointRef::Port(c::PortRef::local("device", "bus")),
        physical: c::PhysicalEndpointRef::Connector(c::ConnectorRef {
            owner,
            connector: "J1".to_owned(),
        }),
        fidelity: c::Fidelity::Presented,
    };
    document.scopes[0].bindings.extend([
        binding("cross-component", c::OwnerRef::local_component("peer")),
        binding("assembly-owned", c::OwnerRef::local_assembly("loom")),
    ]);

    let error = normalize_connectivity(&document).unwrap_err();
    let ownership_issues = error
        .issues()
        .iter()
        .filter(|issue| issue.code() == "E_CONN_BINDING_OWNER")
        .collect::<Vec<_>>();
    assert_eq!(ownership_issues.len(), 2, "issues: {:?}", error.issues());
    assert!(ownership_issues
        .iter()
        .all(|issue| issue.subject().kind() == ObjectKind::Binding));
}

#[test]
fn exact_positions_reject_distinct_channels_but_allow_channel_fanout() {
    let rich_component = || {
        let mut value = component("device");
        value.ports[0].channels.push(c::Channel {
            role: None,
            local_group: None,
            name: "aux".to_owned(),
            capabilities: capabilities(c::Carrier::Electrical, Some("hcdf:test-link")),
        });
        value.connectors[0].positions.push(c::Position {
            role: None,
            local_group: None,
            name: "2".to_owned(),
            kind: c::PositionKind::Contact,
            representation: None,
        });
        value
    };
    let exact_binding = |name: &str, channel: &str, position: &str| c::Binding {
        name: name.to_owned(),
        functional: c::FunctionalEndpointRef::Channel(c::ChannelRef::local(
            "device", "bus", channel,
        )),
        physical: c::PhysicalEndpointRef::Position(c::PositionRef::local_component(
            "device", "J1", position,
        )),
        fidelity: c::Fidelity::Exact,
    };

    let mut conflict = c::ConnectivityDocument::new(document_identity());
    declare_component(&mut conflict.scopes[0], rich_component());
    conflict.scopes[0].bindings.extend([
        exact_binding("signal-to-one", "signal", "1"),
        exact_binding("aux-to-one", "aux", "1"),
    ]);
    let error = normalize_connectivity(&conflict).unwrap_err();
    let issue = error
        .issues()
        .iter()
        .find(|issue| issue.code() == "E_CONN_POSITION_CHANNEL_CONFLICT")
        .expect("two distinct channels on one position must conflict");
    assert_eq!(issue.subject().kind(), ObjectKind::Binding);
    assert_eq!(issue.related().len(), 1);
    assert_eq!(issue.related()[0].kind(), ObjectKind::Binding);

    let mut fanout = c::ConnectivityDocument::new(document_identity());
    declare_component(&mut fanout.scopes[0], rich_component());
    fanout.scopes[0].bindings.extend([
        exact_binding("signal-to-one", "signal", "1"),
        exact_binding("signal-to-two", "signal", "2"),
    ]);
    normalize_connectivity(&fanout).expect("one channel may fan out to multiple exact positions");
}

#[test]
fn mixed_compatible_units_work_across_capability_and_selected_ranges() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut first = component("first");
    let mut second = component("second");
    for component in [&mut first, &mut second] {
        component.ports[0].capabilities.limits.voltage = Some(c::QuantityRange {
            minimum: Some(c::Quantity {
                value: 20_000.0,
                unit: "mV".to_owned(),
            }),
            maximum: Some(c::Quantity {
                value: 0.028,
                unit: "kV".to_owned(),
            }),
            nominal: Some(c::Quantity {
                value: 24.0,
                unit: "V".to_owned(),
            }),
        });
    }
    declare_components(&mut document.scopes[0], [first, second]);
    let mut link = network(
        "power",
        vec![
            participant("first", "first", None),
            participant("second", "second", None),
        ],
    );
    link.selected.voltage = Some(c::SelectionQuantity::Range(c::SelectionRange {
        minimum: c::Quantity {
            value: 22.0,
            unit: "V".to_owned(),
        },
        maximum: c::Quantity {
            value: 26_000.0,
            unit: "mV".to_owned(),
        },
        nominal: Some(c::Quantity {
            value: 0.024,
            unit: "kV".to_owned(),
        }),
    }));
    document.scopes[0].networks.push(link);
    normalize_connectivity(&document).unwrap();

    let mut outside = document.clone();
    let Some(c::SelectionQuantity::Range(range)) =
        outside.scopes[0].networks[0].selected.voltage.as_mut()
    else {
        panic!("expected selected voltage range");
    };
    range.maximum.value = 29.0;
    range.maximum.unit = "V".to_owned();
    let error = normalize_connectivity(&outside).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_SELECTION_RANGE"));

    let mut incompatible = document;
    incompatible.scopes[0].components[0].ports[0]
        .capabilities
        .limits
        .voltage
        .as_mut()
        .unwrap()
        .maximum
        .as_mut()
        .unwrap()
        .unit = "A".to_owned();
    let error = normalize_connectivity(&incompatible).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_DIMENSION"));
}

#[test]
fn xml_selected_quantities_are_exact_nominal_or_bounded_range_choices() {
    let sources = [
        (
            r#"<voltage><nominal value="24" unit="V"/></voltage>"#,
            "<nominal value=\"24\" unit=\"V\"/>",
        ),
        (
            r#"<voltage><range min="20" max="28" nominal="24" unit="V"/></voltage>"#,
            "<range min=\"20\" max=\"28\" nominal=\"24\" unit=\"V\"/>",
        ),
    ];
    for (selection, expected) in sources {
        let source = format!(
            r#"<hcdf name="d" version="1.0">
              <comp name="a"><port name="p"/></comp>
              <comp name="b"><port name="p"/></comp>
              <link name="n"><selected purpose="communication" carrier="electrical">{selection}</selected>
                <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
                <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#
        );
        let authored = hcdformat::Hcdf::from_xml_str(&source).unwrap();
        let voltage = authored.link[0]
            .selected
            .as_ref()
            .unwrap()
            .voltage
            .as_ref()
            .unwrap();
        match (&voltage.selection, selection.contains("<nominal")) {
            (hcdformat::model::connectivity_xml::SelectionQuantityChoice::Nominal(_), true)
            | (hcdformat::model::connectivity_xml::SelectionQuantityChoice::Range(_), false) => {}
            _ => panic!("unexpected public selected quantity choice"),
        }
        let serialized = authored.to_xml_string().unwrap();
        assert!(serialized.contains(expected), "{serialized}");
        assert_eq!(
            hcdformat::Hcdf::from_xml_str(&serialized).unwrap(),
            authored
        );
        let canonical = authored
            .to_connectivity_document(document_identity())
            .unwrap();
        normalize_connectivity(&canonical).unwrap();
    }

    for selection in [
        r#"<voltage/>"#,
        r#"<voltage value="24" unit="V"/>"#,
        r#"<voltage><range min="20" unit="V"/></voltage>"#,
        r#"<voltage><nominal value="24" unit="V"/><range min="20" max="28" unit="V"/></voltage>"#,
    ] {
        let source = format!(
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"/></comp><comp name="b"><port name="p"/></comp><link name="n"><selected purpose="communication" carrier="electrical">{selection}</selected><participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant><participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant></link></hcdf>"#
        );
        assert!(
            hcdformat::Hcdf::from_xml_str(&source).is_err(),
            "malformed selected quantity must fail: {selection}"
        );
    }
}

fn termination_attachment(position: &str) -> c::PhysicalEndpointRef {
    c::PhysicalEndpointRef::Position(c::PositionRef::local_component("device", "J1", position))
}

fn termination(
    name: &str,
    mounting: c::TerminationMounting,
    attachments: Vec<c::PhysicalEndpointRef>,
) -> c::Termination {
    c::Termination {
        name: name.to_owned(),
        kind: profile("hcdf:resistor"),
        mounting,
        attachments,
        quantities: Vec::new(),
        fidelity: c::Fidelity::Exact,
        profile: None,
        representation: None,
    }
}

fn termination_component() -> c::ComponentConnectivity {
    let mut device = component("device");
    device.connectors[0].positions.extend([
        c::Position {
            role: None,
            local_group: None,
            name: "2".to_owned(),
            kind: c::PositionKind::Contact,
            representation: None,
        },
        c::Position {
            role: None,
            local_group: None,
            name: "3".to_owned(),
            kind: c::PositionKind::Contact,
            representation: None,
        },
    ]);
    device
}

#[test]
fn strong_terminations_preserve_kind_mounting_attachments_values_and_edges() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut device = termination_component();
    let mut endpoint = termination(
        "endpoint",
        c::TerminationMounting::Endpoint,
        vec![termination_attachment("1"), termination_attachment("2")],
    );
    endpoint.quantities.extend([
        c::NamedQuantity {
            property: profile("hcdf:resistance"),
            quantity: c::Quantity {
                value: 120.0,
                unit: "ohm".to_owned(),
            },
        },
        c::NamedQuantity {
            property: profile("hcdf:capacitance"),
            quantity: c::Quantity {
                value: 47.0,
                unit: "nF".to_owned(),
            },
        },
        c::NamedQuantity {
            property: profile("hcdf:inductance"),
            quantity: c::Quantity {
                value: 10.0,
                unit: "uH".to_owned(),
            },
        },
    ]);
    device.terminations.extend([
        endpoint,
        termination(
            "inline",
            c::TerminationMounting::Inline,
            vec![termination_attachment("1"), termination_attachment("2")],
        ),
        termination(
            "branch",
            c::TerminationMounting::Branch,
            vec![termination_attachment("1")],
        ),
        termination(
            "closure",
            c::TerminationMounting::Closure,
            vec![termination_attachment("2"), termination_attachment("3")],
        ),
    ]);
    declare_component(&mut document.scopes[0], device);
    let graph = normalize_connectivity(&document).unwrap();
    assert_eq!(
        graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::TerminationAttachment)
            .count(),
        7
    );
    let endpoint_node = graph
        .nodes()
        .iter()
        .find(|node| {
            node.kind() == ObjectKind::Termination
                && node.identity().local().last().unwrap().value == "endpoint"
        })
        .unwrap();
    let hcdformat::connectivity::ConnectivityNodeData::Termination {
        kind,
        mounting,
        quantities,
        ..
    } = endpoint_node.data()
    else {
        panic!("expected termination node data");
    };
    assert_eq!(kind.as_str(), "hcdf:resistor");
    assert_eq!(*mounting, c::TerminationMounting::Endpoint);
    assert_eq!(quantities[0].property.as_str(), "hcdf:resistance");
    assert_eq!(quantities[0].quantity.unit, "ohm");
    assert_eq!(quantities[1].property.as_str(), "hcdf:capacitance");
    assert_eq!(quantities[1].quantity.unit, "nF");
    assert_eq!(quantities[2].property.as_str(), "hcdf:inductance");
    assert_eq!(quantities[2].quantity.unit, "uH");
}

#[test]
fn strong_terminations_reject_attachment_count_duplicates_owner_fidelity_and_bad_values() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut device = termination_component();
    device.terminations.push(termination(
        "empty",
        c::TerminationMounting::Endpoint,
        Vec::new(),
    ));
    device.terminations.push(termination(
        "short-inline",
        c::TerminationMounting::Inline,
        vec![termination_attachment("2")],
    ));
    device.terminations.push(termination(
        "duplicate-attachment",
        c::TerminationMounting::Inline,
        vec![termination_attachment("1"), termination_attachment("1")],
    ));
    let mut duplicate_property = termination(
        "duplicate-property",
        c::TerminationMounting::Endpoint,
        vec![termination_attachment("1")],
    );
    duplicate_property.quantities = vec![
        c::NamedQuantity {
            property: profile("hcdf:resistance"),
            quantity: c::Quantity {
                value: 120.0,
                unit: "Ohm".to_owned(),
            },
        },
        c::NamedQuantity {
            property: profile("hcdf:resistance"),
            quantity: c::Quantity {
                value: 121.0,
                unit: "ohm".to_owned(),
            },
        },
        c::NamedQuantity {
            property: profile("hcdf:temperature"),
            quantity: c::Quantity {
                value: -1.0,
                unit: "K".to_owned(),
            },
        },
    ];
    device.terminations.push(duplicate_property);
    let mut wrong_fidelity = termination(
        "wrong-fidelity",
        c::TerminationMounting::Closure,
        vec![termination_attachment("2")],
    );
    wrong_fidelity.fidelity = c::Fidelity::Presented;
    device.terminations.push(wrong_fidelity);
    device.terminations.push(termination(
        "foreign",
        c::TerminationMounting::Endpoint,
        vec![c::PhysicalEndpointRef::Position(
            c::PositionRef::local_component("other", "J1", "1"),
        )],
    ));
    declare_components(&mut document.scopes[0], [device, component("other")]);
    let error = normalize_connectivity(&document).unwrap_err();
    for code in [
        "E_CONN_TERMINATION_ATTACHMENT_COUNT",
        "E_CONN_TERMINATION_DUPLICATE_ATTACHMENT",
        "E_CONN_TERMINATION_DUPLICATE_PROPERTY",
        "E_CONN_UNIT_UNKNOWN",
        "E_CONN_PHYSICAL_RANGE",
        "E_CONN_PHYSICAL_FIDELITY",
        "E_CONN_OWNER_SCOPE",
    ] {
        assert!(
            error.issues().iter().any(|issue| issue.code() == code),
            "missing {code}"
        );
    }
}

#[test]
fn xml_strong_termination_round_trips_and_normalizes() {
    let source = r#"<hcdf name="d" version="1.0"><comp name="device"><connector name="J1"><contact name="1"/></connector><termination name="R1" kind="hcdf:resistor" mounting="endpoint" fidelity="exact" profile="hcdf:rs-485"><attachment><position-ref connector="J1" position="1"><component-ref component="device"/></position-ref></attachment><quantity property="hcdf:resistance" value="120" unit="ohm"/></termination></comp></hcdf>"#;
    let authored = hcdformat::Hcdf::from_xml_str(source).unwrap();
    let serialized = authored.to_xml_string().unwrap();
    for fragment in [
        "kind=\"hcdf:resistor\"",
        "mounting=\"endpoint\"",
        "<attachment>",
        "property=\"hcdf:resistance\"",
    ] {
        assert!(serialized.contains(fragment), "{serialized}");
    }
    assert!(!serialized.contains("placed-at"));
    assert_eq!(
        hcdformat::Hcdf::from_xml_str(&serialized).unwrap(),
        authored
    );
    let canonical = authored
        .to_connectivity_document(document_identity())
        .unwrap();
    normalize_connectivity(&canonical).unwrap();
}

#[test]
fn conversion_errors_expose_stable_specific_codes() {
    let missing_selected = hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"/></comp><comp name="b"><port name="p"/></comp><link name="n"><participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant><participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant></link></hcdf>"#,
    )
    .unwrap()
    .to_connectivity_document(document_identity())
    .unwrap_err();
    assert_eq!(missing_selected.code(), "E_CONN_SELECTED_REQUIRED");

    let bad_profile = hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"><capabilities><profile id="unqualified"/></capabilities></port></comp></hcdf>"#,
    )
    .unwrap()
    .to_connectivity_document(document_identity())
    .unwrap_err();
    assert_eq!(bad_profile.code(), "E_CONN_QUALIFIED_ID");

    let duplicate_profile = hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"><capabilities><profile id="hcdf:x"/><profile id="hcdf:x"/></capabilities></port></comp></hcdf>"#,
    )
    .unwrap()
    .to_connectivity_document(document_identity())
    .unwrap_err();
    assert_eq!(duplicate_profile.code(), "E_CONN_DUPLICATE_PROFILE");

    let bad_shape = authored_representation(
        r#"<derived-route><round-section diameter="0"/><waypoint xyz="0 0 0"><frame><world/></frame></waypoint><waypoint xyz="1 0 0"><frame><world/></frame></waypoint></derived-route>"#,
    )
    .to_connectivity_document(document_identity())
    .unwrap_err();
    assert_eq!(bad_shape.code(), "E_CONN_REPRESENTATION_SHAPE");

    let unresolved = authored_representation(
        r#"<model-part node-path="pin"><model-root><component-visual component="missing" visual="body"/></model-root></model-part>"#,
    )
    .to_connectivity_document(document_identity())
    .unwrap_err();
    assert_eq!(unresolved.code(), "E_CONN_UNRESOLVED_REFERENCE");

    let wrong_root_type = hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><comp name="device"><visual name="primitive"/><connector name="J"><representation><model-part node-path="pin"><model-root><component-visual component="device" visual="primitive"/></model-root></model-part></representation></connector></comp></hcdf>"#,
    )
    .unwrap()
    .to_connectivity_document(document_identity())
    .unwrap_err();
    assert_eq!(wrong_root_type.code(), "E_CONN_MODEL_ROOT_TYPE");

    let bad_frame = authored_representation(
        r#"<sphere radius="1"><placement xyz="0 0 0"><frame><component-frame component="device" frame="missing"/></frame><rotation><rpy value="0 0 0"/></rotation></placement></sphere>"#,
    )
    .to_connectivity_document(document_identity())
    .unwrap_err();
    assert_eq!(bad_frame.code(), "E_CONN_FRAME_REFERENCE");
}

fn nominal_selection(value: f64, unit: &str) -> c::SelectionQuantity {
    c::SelectionQuantity::Nominal(c::Quantity {
        value,
        unit: unit.to_owned(),
    })
}

fn numbered_rf_channel(
    number: u32,
    center_frequency: Option<c::SelectionQuantity>,
    bandwidth: Option<c::SelectionQuantity>,
) -> c::RfChannelSelection {
    c::RfChannelSelection::Numbered {
        number,
        center_frequency,
        bandwidth,
    }
}

fn rf_channel_mut(document: &mut c::ConnectivityDocument) -> &mut c::RfChannelSelection {
    &mut document.scopes[0].networks[0]
        .selected
        .rf
        .as_mut()
        .unwrap()
        .channel
}

fn set_numbered_rf_center(
    document: &mut c::ConnectivityDocument,
    value: Option<c::SelectionQuantity>,
) {
    let c::RfChannelSelection::Numbered {
        center_frequency, ..
    } = rf_channel_mut(document)
    else {
        panic!("expected numbered RF channel");
    };
    *center_frequency = value;
}

fn set_rf_bandwidth(document: &mut c::ConnectivityDocument, value: Option<c::SelectionQuantity>) {
    match rf_channel_mut(document) {
        c::RfChannelSelection::Numbered { bandwidth, .. }
        | c::RfChannelSelection::FrequencyDefined { bandwidth, .. } => *bandwidth = value,
    }
}

fn rf_selection_document() -> c::ConnectivityDocument {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut first = component("first");
    let mut second = component("second");
    for component in [&mut first, &mut second] {
        let port = &mut component.ports[0];
        port.capabilities.carriers = BTreeSet::from([c::Carrier::RadiatedRf]);
        port.capabilities.limits.frequency = Some(quantity_range(57.0, 71.0, "GHz"));
        port.capabilities.limits.bandwidth = Some(quantity_range(0.5, 4.0, "GHz"));
        let channel = &mut port.channels[0];
        channel.capabilities.carriers = BTreeSet::from([c::Carrier::RadiatedRf]);
        channel.capabilities.limits.frequency = Some(quantity_range(57.0, 71.0, "GHz"));
        channel.capabilities.limits.bandwidth = Some(quantity_range(0.5, 4.0, "GHz"));
    }
    declare_components(&mut document.scopes[0], [first, second]);
    let mut link = network(
        "radio",
        vec![
            participant("first", "first", Some("signal")),
            participant("second", "second", Some("signal")),
        ],
    );
    link.selected.carrier = c::Carrier::RadiatedRf;
    link.selected.rf = Some(c::RfSelection {
        channel: numbered_rf_channel(
            7,
            Some(nominal_selection(60.0, "GHz")),
            Some(nominal_selection(2.0, "GHz")),
        ),
    });
    document.scopes[0].networks.push(link);
    document
}

#[test]
fn rf_channel_choices_carriers_dimensions_and_capabilities_are_enforced() {
    let document = rf_selection_document();
    normalize_connectivity(&document).unwrap();

    let mut frequency_defined = document.clone();
    *rf_channel_mut(&mut frequency_defined) = c::RfChannelSelection::FrequencyDefined {
        center_frequency: nominal_selection(60.0, "GHz"),
        bandwidth: Some(nominal_selection(2.0, "GHz")),
    };
    normalize_connectivity(&frequency_defined).unwrap();

    let mut number_only = document.clone();
    *rf_channel_mut(&mut number_only) = numbered_rf_channel(7, None, None);
    let graph = normalize_connectivity(&number_only).unwrap();
    assert!(graph
        .warnings()
        .iter()
        .any(|issue| issue.code() == "W_CONN_SELECTION_UNVERIFIED"));

    let mut conflict = document.clone();
    conflict.scopes[0].networks[0].selected.frequency = Some(nominal_selection(60.0, "GHz"));
    let error = normalize_connectivity(&conflict).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_RF_CENTER_CONFLICT"));

    let mut wrong_carrier = document.clone();
    wrong_carrier.scopes[0].networks[0].selected.carrier = c::Carrier::Electrical;
    let error = normalize_connectivity(&wrong_carrier).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_RF_CARRIER"));

    let mut wrong_center_dimension = document.clone();
    set_numbered_rf_center(
        &mut wrong_center_dimension,
        Some(nominal_selection(60.0, "V")),
    );
    let error = normalize_connectivity(&wrong_center_dimension).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_DIMENSION"));

    let mut wrong_bandwidth_dimension = document.clone();
    set_rf_bandwidth(
        &mut wrong_bandwidth_dimension,
        Some(nominal_selection(2.0, "bit/s")),
    );
    let error = normalize_connectivity(&wrong_bandwidth_dimension).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_DIMENSION"));

    let mut outside_center = document.clone();
    set_numbered_rf_center(&mut outside_center, Some(nominal_selection(72.0, "GHz")));
    let error = normalize_connectivity(&outside_center).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_SELECTION_RANGE"));

    let mut outside_bandwidth = document.clone();
    set_rf_bandwidth(&mut outside_bandwidth, Some(nominal_selection(5.0, "GHz")));
    let error = normalize_connectivity(&outside_bandwidth).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_SELECTION_RANGE"));
}

#[test]
fn rf_occupied_spectrum_uses_worst_case_band_edges() {
    let document = rf_selection_document();

    let mut exact_upper_edge = document.clone();
    set_numbered_rf_center(&mut exact_upper_edge, Some(nominal_selection(70.0, "GHz")));
    normalize_connectivity(&exact_upper_edge).unwrap();

    let mut outside_upper_edge = document.clone();
    set_numbered_rf_center(
        &mut outside_upper_edge,
        Some(nominal_selection(70.5, "GHz")),
    );
    let error = normalize_connectivity(&outside_upper_edge).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_SELECTION_RANGE"));

    let mut generic_center = document.clone();
    set_numbered_rf_center(&mut generic_center, None);
    generic_center.scopes[0].networks[0].selected.frequency = Some(nominal_selection(70.0, "GHz"));
    normalize_connectivity(&generic_center).unwrap();

    let mut nonfinite_envelope = document.clone();
    set_numbered_rf_center(
        &mut nonfinite_envelope,
        Some(nominal_selection(f64::MAX, "Hz")),
    );
    set_rf_bandwidth(
        &mut nonfinite_envelope,
        Some(nominal_selection(f64::MAX, "Hz")),
    );
    for component in &mut nonfinite_envelope.scopes[0].components {
        let port = &mut component.ports[0];
        port.capabilities.limits.bandwidth = Some(quantity_range(0.0, f64::MAX, "Hz"));
        port.channels[0].capabilities.limits.bandwidth = Some(quantity_range(0.0, f64::MAX, "Hz"));
    }
    let error = normalize_connectivity(&nonfinite_envelope).unwrap_err();
    let quantity_issues = error
        .issues()
        .iter()
        .filter(|issue| issue.code() == "E_CONN_QUANTITY_VALUE")
        .collect::<Vec<_>>();
    assert_eq!(quantity_issues.len(), 1);
    assert_eq!(quantity_issues[0].subject().kind(), ObjectKind::Network);
    assert!(!error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_SELECTION_RANGE"));

    let mut negative_lower_edge = document;
    set_numbered_rf_center(&mut negative_lower_edge, Some(nominal_selection(0.5, "Hz")));
    set_rf_bandwidth(&mut negative_lower_edge, Some(nominal_selection(2.0, "Hz")));
    let error = normalize_connectivity(&negative_lower_edge).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_PHYSICAL_RANGE"));
}

#[test]
fn xml_rf_channel_is_an_exact_numbered_or_frequency_defined_choice() {
    let numbered = r#"<hcdf name="d" version="1.0">
      <comp name="a"><port name="p"/></comp>
      <comp name="b"><port name="p"/></comp>
      <link name="radio">
        <selected purpose="communication" carrier="radiated-rf">
          <rf><channel><numbered number="7">
            <center-frequency><nominal value="60" unit="GHz"/></center-frequency>
            <bandwidth><nominal value="2" unit="GHz"/></bandwidth>
          </numbered></channel></rf>
        </selected>
        <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
        <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
      </link>
    </hcdf>"#;
    let authored = hcdformat::Hcdf::from_xml_str(numbered).unwrap();
    let rf = authored.link[0]
        .selected
        .as_ref()
        .unwrap()
        .rf
        .as_ref()
        .unwrap();
    let hcdformat::model::connectivity_xml::RfChannelSelectionChoice::Numbered(channel) =
        &rf.channel.selection
    else {
        panic!("expected numbered RF channel");
    };
    assert_eq!(channel.number, 7);
    assert!(channel.center_frequency.is_some());
    assert!(channel.bandwidth.is_some());

    let center: &hcdformat::model::ConnectivitySelectionQuantity =
        channel.center_frequency.as_ref().unwrap();
    let _: &hcdformat::model::ConnectivitySelectionQuantityChoice = &center.selection;
    let _: Option<&hcdformat::model::ConnectivitySelectionRange> = None;
    let serialized = authored.to_xml_string().unwrap();
    for fragment in [
        "<rf>",
        "<channel>",
        "<numbered number=\"7\">",
        "<center-frequency>",
        "<bandwidth>",
    ] {
        assert!(serialized.contains(fragment), "{serialized}");
    }
    assert_eq!(
        hcdformat::Hcdf::from_xml_str(&serialized).unwrap(),
        authored
    );
    let canonical = authored
        .to_connectivity_document(document_identity())
        .unwrap();
    let c::RfChannelSelection::Numbered { number, .. } = &canonical.scopes[0].networks[0]
        .selected
        .rf
        .as_ref()
        .unwrap()
        .channel
    else {
        panic!("expected canonical numbered RF channel");
    };
    assert_eq!(*number, 7);
    normalize_connectivity(&canonical).unwrap();

    let frequency_defined = numbered.replace(
        r#"<numbered number="7">
            <center-frequency><nominal value="60" unit="GHz"/></center-frequency>
            <bandwidth><nominal value="2" unit="GHz"/></bandwidth>
          </numbered>"#,
        r#"<frequency-defined>
            <center-frequency><nominal value="60" unit="GHz"/></center-frequency>
            <bandwidth><nominal value="2" unit="GHz"/></bandwidth>
          </frequency-defined>"#,
    );
    let authored = hcdformat::Hcdf::from_xml_str(&frequency_defined).unwrap();
    assert!(matches!(
        authored.link[0]
            .selected
            .as_ref()
            .unwrap()
            .rf
            .as_ref()
            .unwrap()
            .channel
            .selection,
        hcdformat::model::connectivity_xml::RfChannelSelectionChoice::FrequencyDefined(_)
    ));
    let canonical = authored
        .to_connectivity_document(document_identity())
        .unwrap();
    assert!(matches!(
        canonical.scopes[0].networks[0]
            .selected
            .rf
            .as_ref()
            .unwrap()
            .channel,
        c::RfChannelSelection::FrequencyDefined { .. }
    ));

    for malformed in [
        r#"<rf><channel/></rf>"#,
        r#"<rf><channel number="7"/></rf>"#,
        r#"<rf><channel><numbered/></channel></rf>"#,
        r#"<rf><channel><frequency-defined><bandwidth><nominal value="2" unit="GHz"/></bandwidth></frequency-defined></channel></rf>"#,
        r#"<rf><channel><numbered number="7"/><frequency-defined><center-frequency><nominal value="60" unit="GHz"/></center-frequency></frequency-defined></channel></rf>"#,
    ] {
        let source = format!(
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"/></comp><comp name="b"><port name="p"/></comp><link name="radio"><selected purpose="communication" carrier="radiated-rf">{malformed}</selected><participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant><participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant></link></hcdf>"#
        );
        assert!(
            hcdformat::Hcdf::from_xml_str(&source).is_err(),
            "malformed RF selection must fail: {malformed}"
        );
    }
}

#[test]
fn affine_temperature_equivalence_is_used_for_capability_containment() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut first = component("first");
    let mut second = component("second");
    for component in [&mut first, &mut second] {
        component.ports[0].capabilities.limits.temperature = Some(c::QuantityRange {
            minimum: None,
            nominal: Some(c::Quantity {
                value: -459.6682,
                unit: "degF".to_owned(),
            }),
            maximum: None,
        });
    }
    declare_components(&mut document.scopes[0], [first, second]);
    let mut link = network(
        "thermal",
        vec![
            participant("first", "first", None),
            participant("second", "second", None),
        ],
    );
    link.selected.temperature = Some(nominal_selection(-273.149, "degC"));
    document.scopes[0].networks.push(link);
    normalize_connectivity(&document).unwrap();

    document.scopes[0].networks[0].selected.temperature = Some(nominal_selection(-273.148, "degC"));
    let error = normalize_connectivity(&document).unwrap_err();
    assert!(error
        .issues()
        .iter()
        .any(|issue| issue.code() == "E_CONN_SELECTION_RANGE"));
}

#[test]
fn termination_builtin_properties_require_their_physical_dimensions() {
    let invalid_properties = [
        ("hcdf:rate", "V"),
        ("hcdf:bit-rate", "V"),
        ("hcdf:symbol-rate", "V"),
        ("hcdf:frequency", "V"),
        ("hcdf:bandwidth", "V"),
        ("hcdf:voltage", "Hz"),
        ("hcdf:current", "V"),
        ("hcdf:power", "V"),
        ("hcdf:resistance", "V"),
        ("hcdf:impedance", "V"),
        ("hcdf:capacitance", "V"),
        ("hcdf:inductance", "V"),
        ("hcdf:pressure", "V"),
        ("hcdf:flow", "V"),
        ("hcdf:volumetric-flow", "V"),
        ("hcdf:mass-flow", "V"),
        ("hcdf:temperature", "V"),
        ("hcdf:length", "V"),
        ("hcdf:wavelength", "V"),
        ("hcdf:duration", "V"),
        ("hcdf:angle", "V"),
        ("hcdf:ratio", "V"),
        ("hcdf:loss", "V"),
        ("hcdf:isotropic-gain", "V"),
    ];
    let mut invalid = c::ConnectivityDocument::new(document_identity());
    let mut device = termination_component();
    let mut value = termination(
        "bad-builtins",
        c::TerminationMounting::Endpoint,
        vec![termination_attachment("1")],
    );
    value.quantities = invalid_properties
        .iter()
        .map(|(property, unit)| c::NamedQuantity {
            property: profile(property),
            quantity: c::Quantity {
                value: 1.0,
                unit: (*unit).to_owned(),
            },
        })
        .collect();
    device.terminations.push(value);
    declare_component(&mut invalid.scopes[0], device);
    let error = normalize_connectivity(&invalid).unwrap_err();
    assert_eq!(
        error
            .issues()
            .iter()
            .filter(|issue| issue.code() == "E_CONN_DIMENSION")
            .count(),
        invalid_properties.len()
    );

    let mut external = c::ConnectivityDocument::new(document_identity());
    let mut device = termination_component();
    let mut value = termination(
        "vendor-property",
        c::TerminationMounting::Endpoint,
        vec![termination_attachment("1")],
    );
    value.quantities.push(c::NamedQuantity {
        property: profile("vendor.example:custom-bias"),
        quantity: c::Quantity {
            value: 5.0,
            unit: "V".to_owned(),
        },
    });
    device.terminations.push(value);
    declare_component(&mut external.scopes[0], device);
    normalize_connectivity(&external).unwrap();
}

#[test]
fn termination_passive_values_are_nonnegative_and_zero_is_valid() {
    let properties = [
        ("hcdf:resistance", "ohm"),
        ("hcdf:impedance", "ohm"),
        ("hcdf:capacitance", "F"),
        ("hcdf:inductance", "H"),
    ];
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut device = termination_component();
    let mut value = termination(
        "passive-values",
        c::TerminationMounting::Endpoint,
        vec![termination_attachment("1")],
    );
    value.quantities = properties
        .iter()
        .map(|(property, unit)| c::NamedQuantity {
            property: profile(property),
            quantity: c::Quantity {
                value: 0.0,
                unit: (*unit).to_owned(),
            },
        })
        .collect();
    device.terminations.push(value);
    declare_component(&mut document.scopes[0], device);
    normalize_connectivity(&document).unwrap();

    for (index, (property, _)) in properties.iter().enumerate() {
        let mut negative = document.clone();
        negative.scopes[0].components[0].terminations[0].quantities[index]
            .quantity
            .value = -1.0;
        let error = normalize_connectivity(&negative).unwrap_err();
        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.code() == "E_CONN_PHYSICAL_RANGE"),
            "negative {} was accepted",
            property
        );
    }
}

#[test]
fn public_network_validation_preserves_conversion_error_codes() {
    let authored = hcdformat::Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"/></comp><comp name="b"><port name="p"/></comp><link name="n"><participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant><participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant></link></hcdf>"#,
    )
    .unwrap();
    let issues = hcdformat::validate_network(&authored);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, "E_CONN_SELECTED_REQUIRED");
}

fn topology_participant(name: &str, component: &str, port_name: &str) -> c::Participant {
    c::Participant {
        name: name.to_owned(),
        endpoint: c::FunctionalEndpointRef::Channel(c::ChannelRef::local(
            component, port_name, "signal",
        )),
        role: None,
    }
}

fn topology_hop(name: &str, component: &str) -> c::Hop {
    c::Hop {
        name: name.to_owned(),
        owner: c::HopOwnerRef::Component(c::ComponentRef::local(component)),
        role: None,
        description: None,
        processing_delay_ns: None,
    }
}

fn topology_leg(
    network_name: &str,
    name: &str,
    from_hop: &str,
    from_participant: &str,
    to_hop: &str,
    to_participant: &str,
) -> c::Leg {
    c::Leg {
        name: name.to_owned(),
        from: c::LegEnd {
            hop: c::HopRef::local(network_name, from_hop),
            participant: c::ParticipantRef::local(network_name, from_participant),
        },
        to: c::LegEnd {
            hop: c::HopRef::local(network_name, to_hop),
            participant: c::ParticipantRef::local(network_name, to_participant),
        },
    }
}

fn chain_network(name: &str, left: &str, right: &str) -> c::Network {
    c::Network {
        name: name.to_owned(),
        structure: c::NetworkStructure::Chain(c::PathTopology {
            hops: vec![
                topology_hop("left-hop", left),
                topology_hop("right-hop", right),
            ],
            legs: vec![topology_leg(
                name,
                "left-to-right",
                "left-hop",
                "left",
                "right-hop",
                "right",
            )],
        }),
        description: None,
        configuration: c::NetworkConfiguration::default(),
        selected: network_selection(),
        participants: vec![
            topology_participant("left", left, "bus"),
            topology_participant("right", right, "bus"),
        ],
    }
}

fn chain_document() -> c::ConnectivityDocument {
    let mut document = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut document.scopes[0],
        [component("left"), component("right")],
    );
    document.scopes[0]
        .networks
        .push(chain_network("path", "left", "right"));
    document
}

fn has_issue(error: &hcdformat::connectivity::NormalizationError, code: &str) -> bool {
    error.issues().iter().any(|issue| issue.code() == code)
}

#[test]
fn named_chain_objects_resolve_and_emit_exact_graph_edges() {
    let mut document = chain_document();
    let c::NetworkStructure::Chain(path) = &mut document.scopes[0].networks[0].structure else {
        unreachable!()
    };
    path.hops[0].role = Some(profile("hcdf:forwarder"));
    path.hops[0].description = Some("left processing stage".to_owned());
    path.hops[0].processing_delay_ns = Some(25);

    let graph = normalize_connectivity(&document).unwrap();
    let resolver = graph.resolver(c::IncludeInstanceId::root());
    let left_participant = resolver
        .participant(&c::ParticipantRef::local("path", "left"))
        .unwrap();
    let right_participant = resolver
        .participant(&c::ParticipantRef::local("path", "right"))
        .unwrap();
    let left_hop = resolver.hop(&c::HopRef::local("path", "left-hop")).unwrap();
    let right_hop = resolver
        .hop(&c::HopRef::local("path", "right-hop"))
        .unwrap();
    let leg = resolver
        .leg(&c::LegRef::local("path", "left-to-right"))
        .unwrap();

    match left_hop.data() {
        hcdformat::connectivity::ConnectivityNodeData::Hop {
            role,
            description,
            processing_delay_ns,
            ..
        } => {
            assert_eq!(role.as_ref(), Some(&profile("hcdf:forwarder")));
            assert_eq!(description.as_deref(), Some("left processing stage"));
            assert_eq!(*processing_delay_ns, Some(25));
        }
        other => panic!("unexpected hop data: {other:?}"),
    }
    match leg.data() {
        hcdformat::connectivity::ConnectivityNodeData::Leg { from, to } => {
            assert_eq!(from.hop, c::HopRef::local("path", "left-hop"));
            assert_eq!(from.participant, c::ParticipantRef::local("path", "left"));
            assert_eq!(to.hop, c::HopRef::local("path", "right-hop"));
            assert_eq!(to.participant, c::ParticipantRef::local("path", "right"));
        }
        other => panic!("unexpected leg data: {other:?}"),
    }

    let expected_directions = [
        (EdgeKind::LegFromHop, left_hop.id(), leg.id()),
        (
            EdgeKind::LegFromParticipant,
            left_participant.id(),
            leg.id(),
        ),
        (EdgeKind::LegToHop, leg.id(), right_hop.id()),
        (EdgeKind::LegToParticipant, leg.id(), right_participant.id()),
    ];
    for (kind, from, to) in expected_directions {
        let edge = graph
            .edges()
            .iter()
            .find(|edge| edge.kind() == kind)
            .unwrap_or_else(|| panic!("missing {kind:?} edge"));
        assert_eq!(edge.from(), from);
        assert_eq!(edge.to(), to);
    }
    assert_eq!(
        graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::TopologyMembership)
            .count(),
        3
    );
    assert_eq!(
        graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::HopOwnership)
            .count(),
        2
    );
}

#[test]
fn chain_order_participant_use_and_owner_rules_are_independent() {
    let mut reversed = chain_document();
    let c::NetworkStructure::Chain(path) = &mut reversed.scopes[0].networks[0].structure else {
        unreachable!()
    };
    let leg = &mut path.legs[0];
    std::mem::swap(&mut leg.from, &mut leg.to);
    let error = normalize_connectivity(&reversed).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ADJACENCY"));

    let mut missing = chain_document();
    let c::NetworkStructure::Chain(path) = &mut missing.scopes[0].networks[0].structure else {
        unreachable!()
    };
    path.legs.clear();
    let error = normalize_connectivity(&missing).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ARITY"));
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ADJACENCY"));

    let mut reused = chain_document();
    let c::NetworkStructure::Chain(path) = &mut reused.scopes[0].networks[0].structure else {
        unreachable!()
    };
    path.legs[0].to.participant = c::ParticipantRef::local("path", "left");
    let error = normalize_connectivity(&reused).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_COVERAGE"));

    let mut wrong_owner = chain_document();
    let c::NetworkStructure::Chain(path) = &mut wrong_owner.scopes[0].networks[0].structure else {
        unreachable!()
    };
    path.hops[0].owner = c::HopOwnerRef::Component(c::ComponentRef::local("right"));
    let error = normalize_connectivity(&wrong_owner).unwrap_err();
    assert!(has_issue(&error, "E_CONN_LEG_ENDPOINT_OWNER"));

    let mut short_link = chain_document();
    short_link.scopes[0].networks[0].structure = c::NetworkStructure::Link;
    short_link.scopes[0].networks[0].participants.pop();
    let error = normalize_connectivity(&short_link).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ARITY"));
}

fn tree_document() -> c::ConnectivityDocument {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut root_component = component("root-device");
    root_component
        .ports
        .push(port("branch", "signal", c::Carrier::Electrical));
    declare_components(
        &mut document.scopes[0],
        [
            root_component,
            component("left-leaf"),
            component("right-leaf"),
        ],
    );
    document.scopes[0].networks.push(c::Network {
        name: "tree-net".to_owned(),
        structure: c::NetworkStructure::Tree(c::TreeTopology {
            root: c::HopRef::local("tree-net", "root-hop"),
            hops: vec![
                topology_hop("root-hop", "root-device"),
                topology_hop("left-hop", "left-leaf"),
                topology_hop("right-hop", "right-leaf"),
            ],
            legs: vec![
                topology_leg(
                    "tree-net",
                    "root-left",
                    "root-hop",
                    "root-left-end",
                    "left-hop",
                    "left-end",
                ),
                topology_leg(
                    "tree-net",
                    "root-right",
                    "root-hop",
                    "root-right-end",
                    "right-hop",
                    "right-end",
                ),
            ],
        }),
        description: Some("rooted distribution".to_owned()),
        configuration: c::NetworkConfiguration::default(),
        selected: network_selection(),
        participants: vec![
            topology_participant("root-left-end", "root-device", "bus"),
            topology_participant("left-end", "left-leaf", "bus"),
            topology_participant("root-right-end", "root-device", "branch"),
            topology_participant("right-end", "right-leaf", "bus"),
        ],
    });
    document
}

#[test]
fn tree_root_is_retained_and_its_graph_is_fully_resolvable() {
    let graph = normalize_connectivity(&tree_document()).unwrap();
    let resolver = graph.resolver(c::IncludeInstanceId::root());
    let network = resolver.network(&c::NetworkRef::local("tree-net")).unwrap();
    match network.data() {
        hcdformat::connectivity::ConnectivityNodeData::Network {
            root,
            hop_order,
            leg_order,
            ..
        } => {
            assert_eq!(
                root.as_ref(),
                Some(&c::HopRef::local("tree-net", "root-hop"))
            );
            assert_eq!(hop_order, &["root-hop", "left-hop", "right-hop"]);
            assert_eq!(leg_order, &["root-left", "root-right"]);
        }
        other => panic!("unexpected tree network data: {other:?}"),
    }
    assert_eq!(
        graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::TreeRoot)
            .count(),
        1
    );
    let root_hop = resolver
        .hop(&c::HopRef::local("tree-net", "root-hop"))
        .unwrap();
    let tree_root_edge = graph
        .edges()
        .iter()
        .find(|edge| edge.kind() == EdgeKind::TreeRoot)
        .unwrap();
    assert_eq!(tree_root_edge.from(), root_hop.id());
    assert_eq!(tree_root_edge.to(), network.id());
}

#[test]
fn tree_rejects_dangling_root_cycles_incoming_errors_and_one_hop_shape() {
    let mut dangling = tree_document();
    let c::NetworkStructure::Tree(tree) = &mut dangling.scopes[0].networks[0].structure else {
        unreachable!()
    };
    tree.root = c::HopRef::local("tree-net", "missing");
    let error = normalize_connectivity(&dangling).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ROOT"));
    assert!(has_issue(&error, "E_CONN_UNRESOLVED_REFERENCE"));

    let mut cycle = tree_document();
    let c::NetworkStructure::Tree(tree) = &mut cycle.scopes[0].networks[0].structure else {
        unreachable!()
    };
    tree.legs[1].from.hop = c::HopRef::local("tree-net", "left-hop");
    tree.legs[1].to.hop = c::HopRef::local("tree-net", "root-hop");
    let error = normalize_connectivity(&cycle).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_CYCLE"));
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_REACHABILITY"));

    let mut multiple_incoming = tree_document();
    let c::NetworkStructure::Tree(tree) = &mut multiple_incoming.scopes[0].networks[0].structure
    else {
        unreachable!()
    };
    tree.legs[0].to.hop = c::HopRef::local("tree-net", "right-hop");
    tree.legs[1].from.hop = c::HopRef::local("tree-net", "left-hop");
    let error = normalize_connectivity(&multiple_incoming).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_INCOMING"));
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_REACHABILITY"));

    let mut one_hop = tree_document();
    let c::NetworkStructure::Tree(tree) = &mut one_hop.scopes[0].networks[0].structure else {
        unreachable!()
    };
    tree.hops.truncate(1);
    tree.legs.clear();
    let error = normalize_connectivity(&one_hop).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ARITY"));
}

#[test]
fn star_coordinator_is_retained_and_has_a_dedicated_membership_edge() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut document.scopes[0],
        [component("coordinator"), component("member")],
    );
    document.scopes[0].networks.push(c::Network {
        name: "star-net".to_owned(),
        structure: c::NetworkStructure::Star(c::StarTopology {
            coordinator: c::ParticipantRef::local("star-net", "hub"),
        }),
        description: None,
        configuration: c::NetworkConfiguration::default(),
        selected: network_selection(),
        participants: vec![
            topology_participant("hub", "coordinator", "bus"),
            topology_participant("leaf", "member", "bus"),
        ],
    });
    let graph = normalize_connectivity(&document).unwrap();
    let network = graph
        .resolver(c::IncludeInstanceId::root())
        .network(&c::NetworkRef::local("star-net"))
        .unwrap();
    match network.data() {
        hcdformat::connectivity::ConnectivityNodeData::Network { coordinator, .. } => {
            assert_eq!(
                coordinator.as_ref(),
                Some(&c::ParticipantRef::local("star-net", "hub"))
            );
        }
        other => panic!("unexpected star network data: {other:?}"),
    }
    assert_eq!(
        graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::StarCoordinator)
            .count(),
        1
    );

    let mut dangling = document;
    let c::NetworkStructure::Star(star) = &mut dangling.scopes[0].networks[0].structure else {
        unreachable!()
    };
    star.coordinator = c::ParticipantRef::local("star-net", "missing");
    let error = normalize_connectivity(&dangling).unwrap_err();
    assert!(has_issue(&error, "E_CONN_UNRESOLVED_REFERENCE"));
}

#[test]
fn topology_refs_that_resolve_in_another_network_still_fail_scope() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut document.scopes[0],
        [
            component("a"),
            component("b"),
            component("c"),
            component("d"),
        ],
    );
    document.scopes[0]
        .networks
        .push(chain_network("first", "a", "b"));
    document.scopes[0]
        .networks
        .push(chain_network("second", "c", "d"));

    let c::NetworkStructure::Chain(first) = &mut document.scopes[0].networks[0].structure else {
        unreachable!()
    };
    first.legs[0].from.hop.network.network = "second".to_owned();
    first.legs[0].from.participant.network.network = "second".to_owned();
    first.legs[0].to.hop.network.network = "second".to_owned();
    first.legs[0].to.participant.network.network = "second".to_owned();
    let error = normalize_connectivity(&document).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_SCOPE"));
}

fn function_owned_component(name: &str) -> c::ComponentConnectivity {
    let mut value = component(name);
    value.ports[0].channels.push(c::Channel {
        name: "aux".to_owned(),
        role: None,
        local_group: None,
        capabilities: capabilities(c::Carrier::Electrical, Some("hcdf:test-link")),
    });
    value.functions.push(c::ConnectivityFunction {
        name: "forwarder".to_owned(),
        kind: c::FunctionKind::Bridge,
        inputs: vec![c::FunctionalEndpointRef::Channel(c::ChannelRef::local(
            name, "bus", "signal",
        ))],
        outputs: vec![c::FunctionalEndpointRef::Channel(c::ChannelRef::local(
            name, "bus", "aux",
        ))],
        bidirectional: Vec::new(),
    });
    value
}

fn instance_channel(
    instance: c::IncludeInstanceId,
    component: &str,
    channel: &str,
) -> c::FunctionalEndpointRef {
    c::FunctionalEndpointRef::Channel(c::ChannelRef {
        scope: c::ReferenceScope::Instance(instance),
        component: component.to_owned(),
        port: "bus".to_owned(),
        channel: channel.to_owned(),
    })
}

fn instance_function(
    instance: c::IncludeInstanceId,
    component: &str,
) -> c::ConnectivityFunctionRef {
    c::ConnectivityFunctionRef {
        component: c::ComponentRef {
            scope: c::ReferenceScope::Instance(instance),
            component: component.to_owned(),
        },
        function: "forwarder".to_owned(),
    }
}

fn repeated_function_owner_document() -> c::ConnectivityDocument {
    let root = c::IncludeInstanceId::root();
    let left = root.child("module", 0);
    let right = root.child("module", 1);
    let mut root_scope = c::ConnectivityScope::root();
    root_scope.networks.push(c::Network {
        name: "parent-path".to_owned(),
        structure: c::NetworkStructure::Chain(c::PathTopology {
            hops: vec![
                c::Hop {
                    name: "left-hop".to_owned(),
                    owner: c::HopOwnerRef::Function(instance_function(left.clone(), "device")),
                    role: None,
                    description: None,
                    processing_delay_ns: None,
                },
                c::Hop {
                    name: "right-hop".to_owned(),
                    owner: c::HopOwnerRef::Function(instance_function(right.clone(), "device")),
                    role: None,
                    description: None,
                    processing_delay_ns: None,
                },
            ],
            legs: vec![topology_leg(
                "parent-path",
                "left-right",
                "left-hop",
                "left-end",
                "right-hop",
                "right-end",
            )],
        }),
        description: None,
        configuration: c::NetworkConfiguration::default(),
        selected: network_selection(),
        participants: vec![
            c::Participant {
                name: "left-end".to_owned(),
                endpoint: instance_channel(left.clone(), "device", "signal"),
                role: None,
            },
            c::Participant {
                name: "right-end".to_owned(),
                endpoint: instance_channel(right.clone(), "device", "signal"),
                role: None,
            },
        ],
    });

    let mut left_scope = c::ConnectivityScope::new(left);
    declare_component(&mut left_scope, function_owned_component("device"));
    let mut right_scope = c::ConnectivityScope::new(right);
    declare_component(&mut right_scope, function_owned_component("device"));
    c::ConnectivityDocument {
        document: document_identity(),
        scopes: vec![root_scope, left_scope, right_scope],
    }
}

#[test]
fn repeated_include_function_owners_are_order_independent_and_resolvable() {
    let first = repeated_function_owner_document();
    let first_graph = normalize_connectivity(&first).unwrap();
    let mut directional = first.clone();
    let left_function = &mut directional.scopes[1].components[0].functions[0];
    left_function
        .outputs
        .push(left_function.inputs.pop().expect("left signal input"));
    let right_function = &mut directional.scopes[2].components[0].functions[0];
    right_function
        .bidirectional
        .extend(std::mem::take(&mut right_function.inputs));
    right_function
        .bidirectional
        .extend(std::mem::take(&mut right_function.outputs));
    normalize_connectivity(&directional).unwrap();
    let mut reversed = first.clone();
    reversed.scopes.reverse();
    let reversed_graph = normalize_connectivity(&reversed).unwrap();
    assert_eq!(
        first_graph.to_canonical_json().unwrap(),
        reversed_graph.to_canonical_json().unwrap()
    );

    let root_resolver = first_graph.resolver(c::IncludeInstanceId::root());
    let left_instance = c::IncludeInstanceId::root().child("module", 0);
    let function = instance_function(left_instance, "device");
    assert_eq!(
        root_resolver.function(&function).unwrap().kind(),
        ObjectKind::ConnectivityFunction
    );
    assert_eq!(
        root_resolver
            .hop_owner(&c::HopOwnerRef::Function(function))
            .unwrap()
            .kind(),
        ObjectKind::ConnectivityFunction
    );
    assert_eq!(
        first_graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::HopOwnership)
            .count(),
        2
    );
}

fn ring_document() -> c::ConnectivityDocument {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut components = Vec::new();
    for name in ["a", "b", "c"] {
        let mut value = component(name);
        value
            .ports
            .push(port("ring", "signal", c::Carrier::Electrical));
        components.push(value);
    }
    declare_components(&mut document.scopes[0], components);
    document.scopes[0].networks.push(c::Network {
        name: "ring-net".to_owned(),
        structure: c::NetworkStructure::Ring(c::PathTopology {
            hops: vec![
                topology_hop("a-hop", "a"),
                topology_hop("b-hop", "b"),
                topology_hop("c-hop", "c"),
            ],
            legs: vec![
                topology_leg("ring-net", "a-b", "a-hop", "a-out", "b-hop", "b-in"),
                topology_leg("ring-net", "b-c", "b-hop", "b-out", "c-hop", "c-in"),
                topology_leg("ring-net", "c-a", "c-hop", "c-out", "a-hop", "a-in"),
            ],
        }),
        description: None,
        configuration: c::NetworkConfiguration::default(),
        selected: network_selection(),
        participants: vec![
            topology_participant("a-out", "a", "bus"),
            topology_participant("b-in", "b", "bus"),
            topology_participant("b-out", "b", "ring"),
            topology_participant("c-in", "c", "bus"),
            topology_participant("c-out", "c", "ring"),
            topology_participant("a-in", "a", "ring"),
        ],
    });
    document
}

#[test]
fn ring_requires_the_declared_closing_orientation_and_exact_leg_count() {
    normalize_connectivity(&ring_document()).unwrap();

    let mut reversed_closure = ring_document();
    let c::NetworkStructure::Ring(ring) = &mut reversed_closure.scopes[0].networks[0].structure
    else {
        unreachable!()
    };
    let closing_leg = &mut ring.legs[2];
    std::mem::swap(&mut closing_leg.from, &mut closing_leg.to);
    let error = normalize_connectivity(&reversed_closure).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ADJACENCY"));

    let mut missing_closure = ring_document();
    let c::NetworkStructure::Ring(ring) = &mut missing_closure.scopes[0].networks[0].structure
    else {
        unreachable!()
    };
    ring.legs.pop();
    let error = normalize_connectivity(&missing_closure).unwrap_err();
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ARITY"));
    assert!(has_issue(&error, "E_CONN_TOPOLOGY_ADJACENCY"));
}

#[test]
fn bus_and_mesh_still_require_at_least_two_participants() {
    for structure in [c::NetworkStructure::Bus, c::NetworkStructure::Mesh] {
        let mut document = c::ConnectivityDocument::new(document_identity());
        declare_component(&mut document.scopes[0], component("only"));
        document.scopes[0].networks.push(c::Network {
            name: "short".to_owned(),
            structure,
            description: None,
            configuration: c::NetworkConfiguration::default(),
            selected: network_selection(),
            participants: vec![topology_participant("only", "only", "bus")],
        });
        let error = normalize_connectivity(&document).unwrap_err();
        assert!(has_issue(&error, "E_CONN_TOPOLOGY_ARITY"));
    }
}

#[test]
fn role_and_local_group_metadata_survives_the_graph_and_blank_groups_fail() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut device = component("device");
    device.ports[0].channels[0].role = Some(profile("hcdf:signal"));
    device.ports[0].channels[0].local_group = Some("pair-a".to_owned());
    device.connectors[0].positions[0].role = Some(profile("hcdf:positive"));
    device.connectors[0].positions[0].local_group = Some("pair-a".to_owned());
    device.connectors.push(connector("J2", "1"));
    device.paths.push(c::PhysicalPath {
        name: "signal-path".to_owned(),
        kind: c::PathKind::Wire,
        first: c::PhysicalEndpointRef::Position(c::PositionRef::local_component(
            "device", "J1", "1",
        )),
        second: c::PhysicalEndpointRef::Position(c::PositionRef::local_component(
            "device", "J2", "1",
        )),
        fidelity: c::Fidelity::Exact,
        role: Some(profile("hcdf:differential-pair")),
        local_group: Some("pair-a".to_owned()),
        representation: None,
    });
    declare_component(&mut document.scopes[0], device);
    let graph = normalize_connectivity(&document).unwrap();
    let json = graph.to_canonical_json().unwrap();
    assert!(json.contains(r#""role": "hcdf:signal""#));
    assert!(json.contains(r#""role": "hcdf:positive""#));
    assert!(json.contains(r#""role": "hcdf:differential-pair""#));
    assert_eq!(json.matches(r#""local_group": "pair-a""#).count(), 3);

    let mut blank = document;
    blank.scopes[0].components[0].ports[0].channels[0].local_group = Some(" ".to_owned());
    blank.scopes[0].components[0].connectors[0].positions[0].local_group = Some("\t".to_owned());
    blank.scopes[0].components[0].paths[0].local_group = Some("\n".to_owned());
    let error = normalize_connectivity(&blank).unwrap_err();
    assert_eq!(
        error
            .issues()
            .iter()
            .filter(|issue| issue.code() == "E_CONN_LOCAL_GROUP")
            .count(),
        3
    );
}
fn timed_network_document() -> c::ConnectivityDocument {
    let mut document = c::ConnectivityDocument::new(document_identity());
    declare_component(&mut document.scopes[0], component("a"));
    declare_component(&mut document.scopes[0], component("b"));
    let control = c::TrafficClassRef::local("timed", "control");
    let best_effort = c::TrafficClassRef::local("timed", "best-effort");
    document.scopes[0].networks.push(c::Network {
        name: "timed".to_owned(),
        structure: c::NetworkStructure::Link,
        description: Some("scheduled link".to_owned()),
        selected: network_selection(),
        participants: vec![
            topology_participant("a", "a", "bus"),
            topology_participant("b", "b", "bus"),
        ],
        configuration: c::NetworkConfiguration {
            gptp_domains: vec![c::GptpDomain {
                name: "default".to_owned(),
                number: 0,
                port_defaults: Some(c::GptpPortDefaults {
                    log_sync_interval: Some(-3),
                    log_announce_interval: Some(0),
                    log_pdelay_req_interval: Some(-4),
                    announce_receipt_timeout: Some(3),
                    neighbor_prop_delay_threshold_ns: Some(800),
                }),
                clocks: vec![
                    c::GptpClock {
                        name: "a-clock".to_owned(),
                        participant: c::ParticipantRef::local("timed", "a"),
                        kind: c::GptpClockKind::Ordinary,
                        gm_capable: true,
                        priority1: Some(128),
                        priority2: Some(128),
                        clock_class: Some(248),
                        clock_accuracy: Some(254),
                    },
                    c::GptpClock {
                        name: "b-clock".to_owned(),
                        participant: c::ParticipantRef::local("timed", "b"),
                        kind: c::GptpClockKind::Ordinary,
                        gm_capable: false,
                        priority1: None,
                        priority2: None,
                        clock_class: None,
                        clock_accuracy: None,
                    },
                ],
            }],
            traffic_classes: vec![
                c::TrafficClass {
                    name: "control".to_owned(),
                    number: 7,
                    description: Some("Control traffic".to_owned()),
                    preemption: Some(c::TrafficPreemption::Express),
                    pcp: BTreeSet::from([6, 7]),
                },
                c::TrafficClass {
                    name: "best-effort".to_owned(),
                    number: 0,
                    description: None,
                    preemption: Some(c::TrafficPreemption::Preemptable),
                    pcp: BTreeSet::from([0]),
                },
            ],
            gate_schedules: vec![c::GateSchedule {
                name: "main".to_owned(),
                cycle_time_ns: 1_000,
                entries: vec![
                    c::GateControlEntry {
                        duration_ns: 400,
                        open: BTreeSet::from([control]),
                    },
                    c::GateControlEntry {
                        duration_ns: 600,
                        open: BTreeSet::from([best_effort]),
                    },
                ],
            }],
            schedule_assignments: vec![c::ScheduleAssignment {
                name: "main-ports".to_owned(),
                schedule: c::ScheduleRef::local("timed", "main"),
                targets: vec![
                    c::ParticipantRef::local("timed", "a"),
                    c::ParticipantRef::local("timed", "b"),
                ],
            }],
            plca: None,
            macsec: None,
            eee: None,
        },
    });
    document
}

#[test]
fn timing_configuration_normalizes_to_ordered_resolvable_graph_objects() {
    let document = timed_network_document();
    let graph = normalize_connectivity(&document).unwrap();
    let resolver = graph.resolver(c::IncludeInstanceId::root());
    assert_eq!(
        resolver
            .traffic_class(&c::TrafficClassRef::local("timed", "control"))
            .unwrap()
            .kind(),
        ObjectKind::TrafficClass
    );
    assert_eq!(
        resolver
            .gate_schedule(&c::ScheduleRef::local("timed", "main"))
            .unwrap()
            .kind(),
        ObjectKind::GateSchedule
    );
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::GptpClock)
            .count(),
        2
    );
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::GateControlEntry)
            .count(),
        2
    );
    for kind in [
        EdgeKind::GptpClockParticipant,
        EdgeKind::GateEntryTrafficClass,
        EdgeKind::ScheduleAssignmentParticipant,
        EdgeKind::ScheduleAssignmentSchedule,
    ] {
        assert!(graph.edges().iter().any(|edge| edge.kind() == kind));
    }
    let network_node = resolver.network(&c::NetworkRef::local("timed")).unwrap();
    let ConnectivityNodeData::Network {
        gptp_domain_order,
        traffic_class_order,
        gate_schedule_order,
        schedule_assignment_order,
        ..
    } = network_node.data()
    else {
        unreachable!()
    };
    assert_eq!(gptp_domain_order, &["default"]);
    assert_eq!(traffic_class_order, &["control", "best-effort"]);
    assert_eq!(gate_schedule_order, &["main"]);
    assert_eq!(schedule_assignment_order, &["main-ports"]);

    let domain_node = graph
        .nodes()
        .iter()
        .find(|node| node.kind() == ObjectKind::GptpDomain)
        .unwrap();
    let ConnectivityNodeData::GptpDomain {
        number,
        port_defaults,
        clock_order,
    } = domain_node.data()
    else {
        unreachable!()
    };
    assert_eq!(*number, 0);
    assert_eq!(clock_order, &["a-clock", "b-clock"]);
    let defaults = port_defaults.as_ref().unwrap();
    assert_eq!(defaults.log_sync_interval, Some(-3));
    assert_eq!(defaults.neighbor_prop_delay_threshold_ns, Some(800));

    let grandmaster_capable_clock = graph
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                node.data(),
                ConnectivityNodeData::GptpClock {
                    gm_capable: true,
                    ..
                }
            )
        })
        .unwrap();
    let ConnectivityNodeData::GptpClock {
        priority1,
        priority2,
        clock_class,
        clock_accuracy,
        ..
    } = grandmaster_capable_clock.data()
    else {
        unreachable!()
    };
    assert_eq!(*priority1, Some(128));
    assert_eq!(*priority2, Some(128));
    assert_eq!(*clock_class, Some(248));
    assert_eq!(*clock_accuracy, Some(254));

    let control_node = resolver
        .traffic_class(&c::TrafficClassRef::local("timed", "control"))
        .unwrap();
    let ConnectivityNodeData::TrafficClass {
        number,
        description,
        preemption,
        ..
    } = control_node.data()
    else {
        unreachable!()
    };
    assert_eq!(*number, 7);
    assert_eq!(description.as_deref(), Some("Control traffic"));
    assert_eq!(*preemption, Some(c::TrafficPreemption::Express));

    let assignment_node = graph
        .nodes()
        .iter()
        .find(|node| node.kind() == ObjectKind::ScheduleAssignment)
        .unwrap();
    let ConnectivityNodeData::ScheduleAssignment { target_order, .. } = assignment_node.data()
    else {
        unreachable!()
    };
    assert_eq!(
        target_order,
        &[
            c::ParticipantRef::local("timed", "a"),
            c::ParticipantRef::local("timed", "b"),
        ]
    );
    assert_eq!(
        graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::ScheduleAssignmentParticipant)
            .count(),
        2
    );

    let schedule_node = resolver
        .gate_schedule(&c::ScheduleRef::local("timed", "main"))
        .unwrap();
    let ConnectivityNodeData::GateSchedule {
        cycle_time_ns,
        entry_order,
    } = schedule_node.data()
    else {
        unreachable!()
    };
    assert_eq!(*cycle_time_ns, 1_000);
    assert_eq!(entry_order, &[0, 1]);
}

#[test]
fn timing_configuration_enforces_ranges_exact_sums_and_ownership() {
    let mut domain_range = timed_network_document();
    domain_range.scopes[0].networks[0]
        .configuration
        .gptp_domains[0]
        .number = 128;
    assert!(has_issue(
        &normalize_connectivity(&domain_range).unwrap_err(),
        "E_CONN_GPTP_DOMAIN_RANGE"
    ));

    let mut duplicate_domain = timed_network_document();
    let mut duplicate = duplicate_domain.scopes[0].networks[0]
        .configuration
        .gptp_domains[0]
        .clone();
    duplicate.name = "secondary".to_owned();
    duplicate_domain.scopes[0].networks[0]
        .configuration
        .gptp_domains
        .push(duplicate);
    assert!(has_issue(
        &normalize_connectivity(&duplicate_domain).unwrap_err(),
        "E_CONN_GPTP_DUPLICATE_DOMAIN"
    ));

    let mut duplicate_domain_name = timed_network_document();
    let mut duplicate = duplicate_domain_name.scopes[0].networks[0]
        .configuration
        .gptp_domains[0]
        .clone();
    duplicate.number = 1;
    duplicate_domain_name.scopes[0].networks[0]
        .configuration
        .gptp_domains
        .push(duplicate);
    assert!(has_issue(
        &normalize_connectivity(&duplicate_domain_name).unwrap_err(),
        "E_CONN_GPTP_DUPLICATE_DOMAIN_NAME"
    ));

    let mut empty_domain_name = timed_network_document();
    empty_domain_name.scopes[0].networks[0]
        .configuration
        .gptp_domains[0]
        .name = " ".to_owned();
    assert!(has_issue(
        &normalize_connectivity(&empty_domain_name).unwrap_err(),
        "E_CONN_GPTP_DOMAIN_NAME"
    ));

    let mut no_modeled_grandmaster = timed_network_document();
    for clock in &mut no_modeled_grandmaster.scopes[0].networks[0]
        .configuration
        .gptp_domains[0]
        .clocks
    {
        clock.gm_capable = false;
    }
    normalize_connectivity(&no_modeled_grandmaster).unwrap();

    let mut empty_clock_name = timed_network_document();
    empty_clock_name.scopes[0].networks[0]
        .configuration
        .gptp_domains[0]
        .clocks[0]
        .name = "".to_owned();
    assert!(has_issue(
        &normalize_connectivity(&empty_clock_name).unwrap_err(),
        "E_CONN_GPTP_CLOCK_NAME"
    ));

    let mut duplicate_clock_name = timed_network_document();
    duplicate_clock_name.scopes[0].networks[0]
        .configuration
        .gptp_domains[0]
        .clocks[1]
        .name = "a-clock".to_owned();
    assert!(has_issue(
        &normalize_connectivity(&duplicate_clock_name).unwrap_err(),
        "E_CONN_GPTP_DUPLICATE_CLOCK_NAME"
    ));

    let mut class_range = timed_network_document();
    class_range.scopes[0].networks[0]
        .configuration
        .traffic_classes[0]
        .number = 8;
    assert!(has_issue(
        &normalize_connectivity(&class_range).unwrap_err(),
        "E_CONN_TRAFFIC_CLASS_RANGE"
    ));

    let mut duplicate_class_number = timed_network_document();
    duplicate_class_number.scopes[0].networks[0]
        .configuration
        .traffic_classes[1]
        .number = 7;
    assert!(has_issue(
        &normalize_connectivity(&duplicate_class_number).unwrap_err(),
        "E_CONN_TRAFFIC_CLASS_NUMBER"
    ));

    let mut pcp_range = timed_network_document();
    pcp_range.scopes[0].networks[0]
        .configuration
        .traffic_classes[0]
        .pcp
        .insert(8);
    assert!(has_issue(
        &normalize_connectivity(&pcp_range).unwrap_err(),
        "E_CONN_PCP_RANGE"
    ));

    let mut ambiguous_pcp = timed_network_document();
    ambiguous_pcp.scopes[0].networks[0]
        .configuration
        .traffic_classes[1]
        .pcp
        .insert(6);
    assert!(has_issue(
        &normalize_connectivity(&ambiguous_pcp).unwrap_err(),
        "E_CONN_PCP_AMBIGUOUS"
    ));

    let mut zero_cycle = timed_network_document();
    zero_cycle.scopes[0].networks[0]
        .configuration
        .gate_schedules[0]
        .cycle_time_ns = 0;
    assert!(has_issue(
        &normalize_connectivity(&zero_cycle).unwrap_err(),
        "E_CONN_GATE_CYCLE"
    ));

    let mut all_closed_guard = timed_network_document();
    all_closed_guard.scopes[0].networks[0]
        .configuration
        .gate_schedules[0]
        .entries[1]
        .open
        .clear();
    normalize_connectivity(&all_closed_guard).unwrap();

    let mut zero_duration = timed_network_document();
    zero_duration.scopes[0].networks[0]
        .configuration
        .gate_schedules[0]
        .entries[0]
        .duration_ns = 0;
    let error = normalize_connectivity(&zero_duration).unwrap_err();
    assert!(has_issue(&error, "E_CONN_GATE_DURATION"));
    assert!(has_issue(&error, "E_CONN_GATE_SUM"));

    let mut exact_sum = timed_network_document();
    exact_sum.scopes[0].networks[0].configuration.gate_schedules[0].entries[0].duration_ns =
        u64::MAX;
    exact_sum.scopes[0].networks[0].configuration.gate_schedules[0].entries[1].duration_ns =
        u64::MAX;
    assert!(has_issue(
        &normalize_connectivity(&exact_sum).unwrap_err(),
        "E_CONN_GATE_SUM"
    ));

    let mut foreign_class = timed_network_document();
    foreign_class.scopes[0].networks[0]
        .configuration
        .gate_schedules[0]
        .entries[0]
        .open
        .clear();
    foreign_class.scopes[0].networks[0]
        .configuration
        .gate_schedules[0]
        .entries[0]
        .open
        .insert(c::TrafficClassRef::local("other", "control"));
    assert!(has_issue(
        &normalize_connectivity(&foreign_class).unwrap_err(),
        "E_CONN_CONFIGURATION_SCOPE"
    ));

    let mut duplicate_assignment_name = timed_network_document();
    let duplicate = duplicate_assignment_name.scopes[0].networks[0]
        .configuration
        .schedule_assignments[0]
        .clone();
    duplicate_assignment_name.scopes[0].networks[0]
        .configuration
        .schedule_assignments
        .push(duplicate);
    assert!(has_issue(
        &normalize_connectivity(&duplicate_assignment_name).unwrap_err(),
        "E_CONN_DUPLICATE_SCHEDULE_ASSIGNMENT_NAME"
    ));

    let mut duplicate_assignment_target = timed_network_document();
    let mut duplicate = duplicate_assignment_target.scopes[0].networks[0]
        .configuration
        .schedule_assignments[0]
        .clone();
    duplicate.name = "secondary-ports".to_owned();
    duplicate.targets = vec![c::ParticipantRef::local("timed", "a")];
    duplicate_assignment_target.scopes[0].networks[0]
        .configuration
        .schedule_assignments
        .push(duplicate);
    assert!(has_issue(
        &normalize_connectivity(&duplicate_assignment_target).unwrap_err(),
        "E_CONN_DUPLICATE_SCHEDULE_ASSIGNMENT"
    ));

    let mut empty_assignment_name = timed_network_document();
    empty_assignment_name.scopes[0].networks[0]
        .configuration
        .schedule_assignments[0]
        .name = " ".to_owned();
    assert!(has_issue(
        &normalize_connectivity(&empty_assignment_name).unwrap_err(),
        "E_CONN_SCHEDULE_ASSIGNMENT_NAME"
    ));

    let mut no_assignment_targets = timed_network_document();
    no_assignment_targets.scopes[0].networks[0]
        .configuration
        .schedule_assignments[0]
        .targets
        .clear();
    assert!(has_issue(
        &normalize_connectivity(&no_assignment_targets).unwrap_err(),
        "E_CONN_SCHEDULE_ASSIGNMENT_TARGETS"
    ));

    let mut empty_assignment_schedule = timed_network_document();
    empty_assignment_schedule.scopes[0].networks[0]
        .configuration
        .schedule_assignments[0]
        .schedule
        .schedule = " ".to_owned();
    assert!(has_issue(
        &normalize_connectivity(&empty_assignment_schedule).unwrap_err(),
        "E_CONN_EMPTY_REFERENCE"
    ));

    let mut empty_assignment_target = timed_network_document();
    empty_assignment_target.scopes[0].networks[0]
        .configuration
        .schedule_assignments[0]
        .targets[0]
        .participant = "".to_owned();
    assert!(has_issue(
        &normalize_connectivity(&empty_assignment_target).unwrap_err(),
        "E_CONN_EMPTY_REFERENCE"
    ));

    let mut foreign_assignment_target = timed_network_document();
    foreign_assignment_target.scopes[0].networks[0]
        .configuration
        .schedule_assignments[0]
        .targets[0]
        .network = c::NetworkRef::local("other");
    assert!(has_issue(
        &normalize_connectivity(&foreign_assignment_target).unwrap_err(),
        "E_CONN_CONFIGURATION_SCOPE"
    ));
}

#[test]
fn timing_xml_wrappers_round_trip_and_strictly_reject_unknown_shape() {
    let source = r#"<hcdf name="timed" version="1.0">
      <comp name="a"><port name="p"/></comp>
      <comp name="b"><port name="p"/></comp>
      <link name="timed">
        <selected purpose="communication" carrier="electrical"/>
        <configuration>
          <gptp-domain name="default" number="0">
            <clock name="a-clock" kind="ordinary" gm-capable="true" priority1="128" priority2="128" clock-class="248" clock-accuracy="254"><participant-ref network="timed" participant="a"/></clock>
            <clock name="b-clock" kind="ordinary" gm-capable="false"><participant-ref network="timed" participant="b"/></clock>
            <port-defaults log-sync-interval="-3" log-announce-interval="0" log-pdelay-req-interval="-4" announce-receipt-timeout="3" neighbor-prop-delay-threshold-ns="800"/>
          </gptp-domain>
          <traffic-class name="control" number="7" preemption="express"><description>Control traffic</description><pcp value="6"/><pcp value="7"/></traffic-class>
          <gate-schedule name="main" cycle-time-ns="1000">
            <gate duration-ns="750">
              <open><traffic-class-ref network="timed" traffic-class="control"/></open>
            </gate>
            <gate duration-ns="250"><open/></gate>
          </gate-schedule>
          <schedule-assignment name="main-ports">
            <schedule-ref network="timed" schedule="main"/>
            <target><participant-ref network="timed" participant="a"/></target>
            <target><participant-ref network="timed" participant="b"/></target>
          </schedule-assignment>
        </configuration>
        <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
        <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
      </link>
    </hcdf>"#;
    let authored = hcdformat::Hcdf::from_xml_str(source).unwrap();
    let serialized = authored.to_xml_string().unwrap();
    assert_eq!(
        hcdformat::Hcdf::from_xml_str(&serialized).unwrap(),
        authored
    );
    let canonical = authored
        .to_connectivity_document(document_identity())
        .unwrap();
    normalize_connectivity(&canonical).unwrap();
    let configuration = &canonical.scopes[0].networks[0].configuration;
    assert_eq!(configuration.gptp_domains[0].name, "default");
    assert_eq!(
        configuration.gptp_domains[0]
            .port_defaults
            .as_ref()
            .unwrap()
            .log_sync_interval,
        Some(-3)
    );
    assert_eq!(configuration.schedule_assignments[0].targets.len(), 2);
    assert_eq!(configuration.gate_schedules[0].entries.len(), 2);
    assert!(configuration.gate_schedules[0].entries[1].open.is_empty());
    assert!(serialized.contains("<open/>"));

    let unknown_attribute = source.replace(
        r#"cycle-time-ns="1000""#,
        r#"cycle-time-ns="1000" cycle="1000""#,
    );
    assert!(hcdformat::Hcdf::from_xml_str(&unknown_attribute).is_err());

    let retired_cycle_name = source.replace("cycle-time-ns=", "cycle-ns=");
    assert!(hcdformat::Hcdf::from_xml_str(&retired_cycle_name).is_err());

    let out_of_range_interval =
        source.replace(r#"log-sync-interval="-3""#, r#"log-sync-interval="128""#);
    assert!(hcdformat::Hcdf::from_xml_str(&out_of_range_interval).is_err());

    let invalid_preemption = source.replace(r#"preemption="express""#, r#"preemption="urgent""#);
    assert!(hcdformat::Hcdf::from_xml_str(&invalid_preemption).is_err());

    let missing_gm_capable = source.replace(r#" gm-capable="true""#, "");
    assert!(hcdformat::Hcdf::from_xml_str(&missing_gm_capable).is_err());

    let unknown_child = source.replace(
        r#"<gate duration-ns="750">"#,
        r#"<gate duration-ns="750"><window/>"#,
    );
    assert!(hcdformat::Hcdf::from_xml_str(&unknown_child).is_err());

    let retired_entry = source
        .replace("<gate duration-ns=\"750\">", "<entry duration-ns=\"750\">")
        .replace("</gate>\n            <gate", "</entry>\n            <gate");
    assert!(hcdformat::Hcdf::from_xml_str(&retired_entry).is_err());

    let bare_target = source.replace(
        r#"<target><participant-ref network="timed" participant="a"/></target>"#,
        r#"<participant-ref network="timed" participant="a"/>"#,
    );
    assert!(hcdformat::Hcdf::from_xml_str(&bare_target).is_err());

    let duplicate_pcp = source.replace(
        r#"<pcp value="6"/><pcp value="7"/>"#,
        r#"<pcp value="6"/><pcp value="6"/>"#,
    );
    let duplicate = hcdformat::Hcdf::from_xml_str(&duplicate_pcp).unwrap();
    let error = duplicate
        .to_connectivity_document(document_identity())
        .unwrap_err();
    assert_eq!(error.code(), "E_CONN_DUPLICATE_PCP");
}

fn secured_bus_document() -> c::ConnectivityDocument {
    let mut document = c::ConnectivityDocument::new(document_identity());
    declare_components(
        &mut document.scopes[0],
        [component("controller"), component("sensor")],
    );
    document.scopes[0].networks.push(c::Network {
        name: "control-bus".to_owned(),
        structure: c::NetworkStructure::Bus,
        description: None,
        selected: network_selection(),
        participants: vec![
            topology_participant("controller", "controller", "bus"),
            topology_participant("sensor", "sensor", "bus"),
        ],
        configuration: c::NetworkConfiguration {
            plca: Some(c::PlcaConfiguration {
                max_node_id: 7,
                to_timer_bit_times: 32,
                nodes: vec![
                    c::PlcaNode {
                        node_id: 0,
                        burst_count: 1,
                        burst_timer_bit_times: 16,
                        participant: c::ParticipantRef::local("control-bus", "controller"),
                    },
                    c::PlcaNode {
                        node_id: 1,
                        burst_count: 0,
                        burst_timer_bit_times: 16,
                        participant: c::ParticipantRef::local("control-bus", "sensor"),
                    },
                ],
            }),
            macsec: Some(c::MacsecConfiguration {
                policies: vec![
                    c::MacsecPolicyDefinition {
                        name: "secure".to_owned(),
                        enforcement: c::MacsecEnforcement::MustSecure,
                        cipher: Some(profile("ieee:gcm-aes-128")),
                        key_agreement: Some(profile("ieee:mka")),
                        confidentiality_offset: Some(0),
                        rekey_interval_ns: Some(1_000_000_000),
                        credential_store_ref: Some("keystore:control-bus".to_owned()),
                    },
                    c::MacsecPolicyDefinition {
                        name: "off".to_owned(),
                        enforcement: c::MacsecEnforcement::Disabled,
                        cipher: None,
                        key_agreement: None,
                        confidentiality_offset: None,
                        rekey_interval_ns: None,
                        credential_store_ref: None,
                    },
                ],
                default_policy: Some(c::MacsecPolicyRef::local("control-bus", "secure")),
                overrides: vec![c::MacsecOverride {
                    target: c::TopologySegmentRef::Network(c::NetworkRef::local("control-bus")),
                    policy: c::MacsecPolicyRef::local("control-bus", "off"),
                }],
            }),
            eee: Some(c::EeeConfiguration {
                default_mode: c::EeeMode::Disabled,
                overrides: vec![c::EeeOverride {
                    participant: c::ParticipantRef::local("control-bus", "sensor"),
                    mode: c::EeeMode::Enabled,
                }],
            }),
            ..c::NetworkConfiguration::default()
        },
    });
    document
}

#[test]
fn plca_macsec_and_eee_normalize_to_exact_resolvable_objects() {
    let graph = normalize_connectivity(&secured_bus_document()).unwrap();
    let resolver = graph.resolver(c::IncludeInstanceId::root());
    let secure = resolver
        .macsec_policy(&c::MacsecPolicyRef::local("control-bus", "secure"))
        .unwrap();
    assert_eq!(secure.kind(), ObjectKind::MacsecPolicy);
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::PlcaNode)
            .count(),
        2
    );
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::MacsecPolicy)
            .count(),
        2
    );
    for kind in [
        EdgeKind::PlcaNodeParticipant,
        EdgeKind::PlcaCoordinator,
        EdgeKind::MacsecDefaultPolicy,
        EdgeKind::MacsecOverridePolicy,
        EdgeKind::MacsecOverrideTarget,
        EdgeKind::EeeOverrideParticipant,
    ] {
        assert!(
            graph.edges().iter().any(|edge| edge.kind() == kind),
            "missing {kind:?}"
        );
    }
    let plca = graph
        .nodes()
        .iter()
        .find(|node| node.kind() == ObjectKind::PlcaConfiguration)
        .unwrap();
    let ConnectivityNodeData::PlcaConfiguration {
        max_node_id,
        to_timer_bit_times,
    } = plca.data()
    else {
        unreachable!()
    };
    assert_eq!(*max_node_id, 7);
    assert_eq!(*to_timer_bit_times, 32);
    let coordinator = graph
        .nodes()
        .iter()
        .find(|node| {
            matches!(
                node.data(),
                ConnectivityNodeData::PlcaNode { node_id: 0, .. }
            )
        })
        .unwrap();
    let ConnectivityNodeData::PlcaNode {
        burst_count,
        burst_timer_bit_times,
        ..
    } = coordinator.data()
    else {
        unreachable!()
    };
    assert_eq!(*burst_count, 1);
    assert_eq!(*burst_timer_bit_times, 16);

    let policy = secure.data();
    let ConnectivityNodeData::MacsecPolicy {
        enforcement,
        confidentiality_offset,
        rekey_interval_ns,
        credential_store_ref,
        ..
    } = policy
    else {
        unreachable!()
    };
    assert_eq!(*enforcement, c::MacsecEnforcement::MustSecure);
    assert_eq!(*confidentiality_offset, Some(0));
    assert_eq!(*rekey_interval_ns, Some(1_000_000_000));
    assert_eq!(
        credential_store_ref.as_deref(),
        Some("keystore:control-bus")
    );
}

#[test]
fn plca_enforces_bus_scope_unique_ids_coordinator_and_coverage() {
    let mut wrong_topology = secured_bus_document();
    wrong_topology.scopes[0].networks[0].structure = c::NetworkStructure::Link;
    assert!(has_issue(
        &normalize_connectivity(&wrong_topology).unwrap_err(),
        "E_CONN_PLCA_TOPOLOGY"
    ));

    let mut no_coordinator = secured_bus_document();
    no_coordinator.scopes[0].networks[0]
        .configuration
        .plca
        .as_mut()
        .unwrap()
        .nodes[0]
        .node_id = 2;
    assert!(has_issue(
        &normalize_connectivity(&no_coordinator).unwrap_err(),
        "E_CONN_PLCA_COORDINATOR"
    ));

    let mut duplicate = secured_bus_document();
    duplicate.scopes[0].networks[0]
        .configuration
        .plca
        .as_mut()
        .unwrap()
        .nodes[1]
        .node_id = 0;
    assert!(has_issue(
        &normalize_connectivity(&duplicate).unwrap_err(),
        "E_CONN_PLCA_DUPLICATE_NODE_ID"
    ));

    let mut reserved = secured_bus_document();
    let plca = reserved.scopes[0].networks[0]
        .configuration
        .plca
        .as_mut()
        .unwrap();
    plca.max_node_id = 255;
    plca.nodes[1].node_id = 255;
    let error = normalize_connectivity(&reserved).unwrap_err();
    assert!(has_issue(&error, "E_CONN_PLCA_MAX_NODE_ID"));
    assert!(has_issue(&error, "E_CONN_PLCA_NODE_ID"));

    let mut incomplete = secured_bus_document();
    incomplete.scopes[0].networks[0]
        .configuration
        .plca
        .as_mut()
        .unwrap()
        .nodes
        .pop();
    assert!(has_issue(
        &normalize_connectivity(&incomplete).unwrap_err(),
        "E_CONN_PLCA_PARTICIPANT_COVERAGE"
    ));
}

#[test]
fn macsec_enforces_policy_parameters_and_exact_segment_targets() {
    let mut missing = secured_bus_document();
    missing.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .policies[0]
        .cipher = None;
    assert!(has_issue(
        &normalize_connectivity(&missing).unwrap_err(),
        "E_CONN_MACSEC_REQUIRED_PARAMETER"
    ));

    let mut xpn_offset = secured_bus_document();
    xpn_offset.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .policies[0]
        .cipher = Some(profile("ieee:gcm-aes-xpn-128"));
    assert!(has_issue(
        &normalize_connectivity(&xpn_offset).unwrap_err(),
        "E_CONN_MACSEC_XPN_OFFSET"
    ));

    let mut vendor_xpn = secured_bus_document();
    vendor_xpn.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .policies[0]
        .cipher = Some(profile("vendor:gcm-aes-xpn-128"));
    normalize_connectivity(&vendor_xpn).unwrap();

    let mut integrity_offset = secured_bus_document();
    integrity_offset.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .policies[0]
        .enforcement = c::MacsecEnforcement::IntegrityOnly;
    assert!(has_issue(
        &normalize_connectivity(&integrity_offset).unwrap_err(),
        "E_CONN_MACSEC_INTEGRITY_OFFSET"
    ));

    let mut invalid_offset = secured_bus_document();
    invalid_offset.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .policies[0]
        .confidentiality_offset = Some(17);
    assert!(has_issue(
        &normalize_connectivity(&invalid_offset).unwrap_err(),
        "E_CONN_MACSEC_CONFIDENTIALITY_OFFSET"
    ));

    let mut zero_rekey = secured_bus_document();
    zero_rekey.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .policies[0]
        .rekey_interval_ns = Some(0);
    assert!(has_issue(
        &normalize_connectivity(&zero_rekey).unwrap_err(),
        "E_CONN_MACSEC_REKEY_INTERVAL"
    ));

    let mut blank_store = secured_bus_document();
    blank_store.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .policies[0]
        .credential_store_ref = Some(" ".to_owned());
    assert!(has_issue(
        &normalize_connectivity(&blank_store).unwrap_err(),
        "E_CONN_MACSEC_CREDENTIAL_STORE"
    ));

    let mut disabled = secured_bus_document();
    disabled.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .policies[1]
        .cipher = Some(profile("ieee:gcm-aes-128"));
    assert!(has_issue(
        &normalize_connectivity(&disabled).unwrap_err(),
        "E_CONN_MACSEC_DISABLED_CONTRADICTION"
    ));

    let mut duplicate_override = secured_bus_document();
    let macsec = duplicate_override.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap();
    macsec.overrides.push(macsec.overrides[0].clone());
    assert!(has_issue(
        &normalize_connectivity(&duplicate_override).unwrap_err(),
        "E_CONN_MACSEC_DUPLICATE_OVERRIDE"
    ));

    let mut path = chain_document();
    path.scopes[0].networks[0].configuration.macsec = Some(c::MacsecConfiguration {
        policies: vec![c::MacsecPolicyDefinition {
            name: "secure".to_owned(),
            enforcement: c::MacsecEnforcement::ShouldSecure,
            cipher: Some(profile("ieee:gcm-aes-128")),
            key_agreement: Some(profile("ieee:mka")),
            confidentiality_offset: Some(30),
            rekey_interval_ns: Some(10),
            credential_store_ref: Some("keystore:path".to_owned()),
        }],
        default_policy: None,
        overrides: vec![c::MacsecOverride {
            target: c::TopologySegmentRef::Leg(c::LegRef::local("path", "left-to-right")),
            policy: c::MacsecPolicyRef::local("path", "secure"),
        }],
    });
    normalize_connectivity(&path).unwrap();
    path.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .overrides[0]
        .target = c::TopologySegmentRef::Network(c::NetworkRef::local("path"));
    assert!(has_issue(
        &normalize_connectivity(&path).unwrap_err(),
        "E_CONN_MACSEC_TARGET_KIND"
    ));
}

#[test]
fn eee_overrides_are_unique_and_resolve_on_the_containing_network() {
    let mut duplicate = secured_bus_document();
    let eee = duplicate.scopes[0].networks[0]
        .configuration
        .eee
        .as_mut()
        .unwrap();
    eee.overrides.push(eee.overrides[0].clone());
    assert!(has_issue(
        &normalize_connectivity(&duplicate).unwrap_err(),
        "E_CONN_EEE_DUPLICATE_OVERRIDE"
    ));

    let mut foreign = secured_bus_document();
    foreign.scopes[0].networks[0]
        .configuration
        .eee
        .as_mut()
        .unwrap()
        .overrides[0]
        .participant
        .network = c::NetworkRef::local("other");
    assert!(has_issue(
        &normalize_connectivity(&foreign).unwrap_err(),
        "E_CONN_CONFIGURATION_SCOPE"
    ));
}

fn override_bindings(
    graph: &NormalizedConnectivityGraph,
    override_kind: ObjectKind,
    target_edge_kind: EdgeKind,
) -> BTreeMap<StableObjectId, (StableObjectId, BTreeSet<StableEdgeId>)> {
    graph
        .nodes()
        .iter()
        .filter(|node| node.kind() == override_kind)
        .map(|node| {
            let target_edge = graph
                .edges()
                .iter()
                .find(|edge| {
                    edge.kind() == target_edge_kind
                        && (edge.from() == node.id() || edge.to() == node.id())
                })
                .unwrap();
            let target_id = if target_edge.from() == node.id() {
                target_edge.to().clone()
            } else {
                target_edge.from().clone()
            };
            let edge_ids = graph
                .edges()
                .iter()
                .filter(|edge| edge.from() == node.id() || edge.to() == node.id())
                .map(|edge| edge.id().clone())
                .collect();
            (target_id, (node.id().clone(), edge_ids))
        })
        .collect()
}

#[test]
fn macsec_override_identity_and_edges_are_stable_when_declarations_reverse() {
    let mut document = c::ConnectivityDocument::new(document_identity());
    let mut middle = component("middle");
    middle
        .ports
        .push(port("bus-out", "signal", c::Carrier::Electrical));
    declare_components(
        &mut document.scopes[0],
        [component("left"), middle, component("right")],
    );
    document.scopes[0].networks.push(c::Network {
        name: "secured-path".to_owned(),
        structure: c::NetworkStructure::Chain(c::PathTopology {
            hops: vec![
                topology_hop("left-hop", "left"),
                topology_hop("middle-hop", "middle"),
                topology_hop("right-hop", "right"),
            ],
            legs: vec![
                topology_leg(
                    "secured-path",
                    "left-middle",
                    "left-hop",
                    "left",
                    "middle-hop",
                    "middle-in",
                ),
                topology_leg(
                    "secured-path",
                    "middle-right",
                    "middle-hop",
                    "middle-out",
                    "right-hop",
                    "right",
                ),
            ],
        }),
        description: None,
        selected: network_selection(),
        participants: vec![
            topology_participant("left", "left", "bus"),
            topology_participant("middle-in", "middle", "bus"),
            topology_participant("middle-out", "middle", "bus-out"),
            topology_participant("right", "right", "bus"),
        ],
        configuration: c::NetworkConfiguration {
            macsec: Some(c::MacsecConfiguration {
                policies: vec![c::MacsecPolicyDefinition {
                    name: "secure".to_owned(),
                    enforcement: c::MacsecEnforcement::ShouldSecure,
                    cipher: Some(profile("ieee:gcm-aes-128")),
                    key_agreement: Some(profile("ieee:mka")),
                    confidentiality_offset: Some(30),
                    rekey_interval_ns: Some(1_000),
                    credential_store_ref: Some("keystore:secured-path".to_owned()),
                }],
                default_policy: None,
                overrides: vec![
                    c::MacsecOverride {
                        target: c::TopologySegmentRef::Leg(c::LegRef::local(
                            "secured-path",
                            "left-middle",
                        )),
                        policy: c::MacsecPolicyRef::local("secured-path", "secure"),
                    },
                    c::MacsecOverride {
                        target: c::TopologySegmentRef::Leg(c::LegRef::local(
                            "secured-path",
                            "middle-right",
                        )),
                        policy: c::MacsecPolicyRef::local("secured-path", "secure"),
                    },
                ],
            }),
            ..c::NetworkConfiguration::default()
        },
    });

    let original = normalize_connectivity(&document).unwrap();
    document.scopes[0].networks[0]
        .configuration
        .macsec
        .as_mut()
        .unwrap()
        .overrides
        .reverse();
    let reversed = normalize_connectivity(&document).unwrap();
    let original_bindings = override_bindings(
        &original,
        ObjectKind::MacsecOverride,
        EdgeKind::MacsecOverrideTarget,
    );
    let reversed_bindings = override_bindings(
        &reversed,
        ObjectKind::MacsecOverride,
        EdgeKind::MacsecOverrideTarget,
    );
    assert_eq!(original_bindings.len(), 2);
    assert_eq!(reversed_bindings, original_bindings);
    assert_eq!(reversed, original);
}

#[test]
fn eee_override_identity_and_edges_are_stable_when_declarations_reverse() {
    let mut document = secured_bus_document();
    document.scopes[0].networks[0]
        .configuration
        .eee
        .as_mut()
        .unwrap()
        .overrides
        .push(c::EeeOverride {
            participant: c::ParticipantRef::local("control-bus", "controller"),
            mode: c::EeeMode::Disabled,
        });

    let original = normalize_connectivity(&document).unwrap();
    let configuration = &mut document.scopes[0].networks[0].configuration;
    configuration.plca.as_mut().unwrap().nodes.reverse();
    configuration.macsec.as_mut().unwrap().policies.reverse();
    configuration.eee.as_mut().unwrap().overrides.reverse();
    let reversed = normalize_connectivity(&document).unwrap();
    let original_bindings = override_bindings(
        &original,
        ObjectKind::EeeOverride,
        EdgeKind::EeeOverrideParticipant,
    );
    let reversed_bindings = override_bindings(
        &reversed,
        ObjectKind::EeeOverride,
        EdgeKind::EeeOverrideParticipant,
    );
    assert_eq!(original_bindings.len(), 2);
    assert_eq!(reversed_bindings, original_bindings);
    assert_eq!(reversed, original);
}

#[test]
fn plca_macsec_and_eee_xml_round_trip_with_exact_wrappers() {
    let source = r#"<hcdf name="secured" version="1.0">
      <comp name="controller"><port name="p"/></comp>
      <comp name="sensor"><port name="p"/></comp>
      <bus name="control-bus">
        <selected purpose="communication" carrier="electrical"/>
        <configuration>
          <plca max-node-id="7" to-timer-bit-times="32">
            <node id="0" burst-count="1" burst-timer-bit-times="16"><participant-ref network="control-bus" participant="controller"/></node>
            <node id="1" burst-count="0" burst-timer-bit-times="16"><participant-ref network="control-bus" participant="sensor"/></node>
          </plca>
          <macsec>
            <policy name="secure" enforcement="must-secure" cipher="ieee:gcm-aes-128" key-agreement="ieee:mka" confidentiality-offset="0" rekey-interval-ns="1000000000" credential-store-ref="keystore:control-bus"/>
            <policy name="off" enforcement="disabled"/>
            <default-policy><macsec-policy-ref network="control-bus" policy="secure"/></default-policy>
            <override><target><network-ref network="control-bus"/></target><macsec-policy-ref network="control-bus" policy="off"/></override>
          </macsec>
          <eee default-mode="disabled"><override mode="enabled"><participant-ref network="control-bus" participant="sensor"/></override></eee>
        </configuration>
        <participant name="controller"><endpoint><port-ref component="controller" port="p"/></endpoint></participant>
        <participant name="sensor"><endpoint><port-ref component="sensor" port="p"/></endpoint></participant>
      </bus>
    </hcdf>"#;
    let authored = hcdformat::Hcdf::from_xml_str(source).unwrap();
    let serialized = authored.to_xml_string().unwrap();
    assert_eq!(
        hcdformat::Hcdf::from_xml_str(&serialized).unwrap(),
        authored
    );
    let canonical = authored
        .to_connectivity_document(document_identity())
        .unwrap();
    normalize_connectivity(&canonical).unwrap();
    let configuration = &canonical.scopes[0].networks[0].configuration;
    assert_eq!(configuration.plca.as_ref().unwrap().to_timer_bit_times, 32);
    assert_eq!(
        configuration
            .macsec
            .as_ref()
            .unwrap()
            .policies
            .first()
            .unwrap()
            .cipher
            .as_ref()
            .unwrap()
            .as_str(),
        "ieee:gcm-aes-128"
    );
    assert!(serialized.contains("<default-policy>"));
    assert!(serialized.contains("<target>"));
    assert!(serialized.contains(r#"<network-ref network="control-bus""#));

    let opaque_target = source.replace(
        r#"<target><network-ref network="control-bus"/></target>"#,
        r#"<target segment="control-bus"/>"#,
    );
    assert!(hcdformat::Hcdf::from_xml_str(&opaque_target).is_err());

    let key_material = source.replace(
        r#"credential-store-ref="keystore:control-bus""#,
        r#"credential-store-ref="keystore:control-bus" key="secret""#,
    );
    assert!(hcdformat::Hcdf::from_xml_str(&key_material).is_err());

    let path_source = r#"<hcdf name="secured-path" version="1.0">
      <comp name="left"><port name="p"/></comp>
      <comp name="right"><port name="p"/></comp>
      <chain name="path">
        <selected purpose="communication" carrier="electrical"/>
        <configuration>
          <macsec>
            <policy name="secure" enforcement="should-secure" cipher="ieee:gcm-aes-128" key-agreement="ieee:mka" confidentiality-offset="30" rekey-interval-ns="1000" credential-store-ref="keystore:path"/>
            <override><target><leg-ref network="path" leg="left-to-right"/></target><macsec-policy-ref network="path" policy="secure"/></override>
          </macsec>
        </configuration>
        <participant name="left"><endpoint><port-ref component="left" port="p"/></endpoint></participant>
        <participant name="right"><endpoint><port-ref component="right" port="p"/></endpoint></participant>
        <hop name="left-hop"><owner><component-ref component="left"/></owner></hop>
        <hop name="right-hop"><owner><component-ref component="right"/></owner></hop>
        <leg name="left-to-right">
          <from><hop-ref network="path" hop="left-hop"/><participant-ref network="path" participant="left"/></from>
          <to><hop-ref network="path" hop="right-hop"/><participant-ref network="path" participant="right"/></to>
        </leg>
      </chain>
    </hcdf>"#;
    let path = hcdformat::Hcdf::from_xml_str(path_source).unwrap();
    let canonical = path.to_connectivity_document(document_identity()).unwrap();
    normalize_connectivity(&canonical).unwrap();
    assert!(matches!(
        canonical.scopes[0].networks[0]
            .configuration
            .macsec
            .as_ref()
            .unwrap()
            .overrides[0]
            .target,
        c::TopologySegmentRef::Leg(_)
    ));
}
