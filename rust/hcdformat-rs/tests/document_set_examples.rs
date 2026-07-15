use hcdformat::connectivity::{ConnectivityNodeData, EdgeKind, ObjectKind, StableObjectId};
use hcdformat::document_set::{
    load_projected_document_set_from_bytes, ConnectivityProjection, DocumentResourceKey,
    DocumentSetOptions, MemoryDocumentResolver, ProjectedConnectivityDocumentSet,
};
use hcdformat::model::connectivity::DocumentIdentity;
use std::collections::{BTreeMap, BTreeSet};

const HUMANOID: &[u8] = include_bytes!("../../../examples/humanoid-mobile-base.hcdf");
const OPERATIONAL: &[u8] = include_bytes!("../../../examples/profiles/operational.streams.xml");

fn project_operational_profile() -> ProjectedConnectivityDocumentSet {
    let mut resolver = MemoryDocumentResolver::new();
    resolver
        .insert_document("/examples/profiles/operational.streams.xml", OPERATIONAL)
        .unwrap();
    load_projected_document_set_from_bytes(
        HUMANOID.to_vec(),
        DocumentIdentity::new("humanoid-mobile-base").unwrap(),
        DocumentResourceKey::new("/examples/humanoid-mobile-base.hcdf").unwrap(),
        &mut resolver,
        DocumentSetOptions::default(),
    )
    .unwrap()
}

fn stream_ids(projected: &ProjectedConnectivityDocumentSet) -> BTreeMap<String, StableObjectId> {
    projected
        .connectivity()
        .graph()
        .unwrap()
        .nodes()
        .iter()
        .filter_map(|node| match node.data() {
            ConnectivityNodeData::Stream { definition } => {
                Some((definition.name.clone(), node.id().clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn operational_profile_projects_through_the_migrated_humanoid_document() {
    let projected = project_operational_profile();
    let ConnectivityProjection::Valid { graph, .. } = projected.connectivity() else {
        panic!("the shipped humanoid document and operational profile must project as valid");
    };

    assert_eq!(projected.sources().len(), 2);
    assert_eq!(projected.stream_profile_instances().len(), 1);
    assert_eq!(projected.stream_profile_sites().len(), 1);
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::Profile)
            .count(),
        1
    );
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::Group)
            .count(),
        3
    );
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::Stream)
            .count(),
        10
    );
    assert_eq!(
        graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::StreamForwarding)
            .count(),
        6
    );
    assert_eq!(
        graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::StreamPath)
            .count(),
        16
    );
    for kind in [
        EdgeKind::StreamForwarding,
        EdgeKind::StreamForwardingFrom,
        EdgeKind::StreamForwardingFunction,
        EdgeKind::StreamForwardingTo,
    ] {
        assert_eq!(
            graph
                .edges()
                .iter()
                .filter(|edge| edge.kind() == kind)
                .count(),
            6,
            "unexpected forwarding edge count for {kind:?}"
        );
    }

    let stream_names = stream_ids(&projected).into_keys().collect::<BTreeSet<_>>();
    assert_eq!(
        stream_names,
        BTreeSet::from([
            "base-command".to_owned(),
            "base-status".to_owned(),
            "front-lidar-ingest".to_owned(),
            "head-telemetry".to_owned(),
            "left-arm-command".to_owned(),
            "left-camera-ingest".to_owned(),
            "radar-ingest".to_owned(),
            "rear-lidar-ingest".to_owned(),
            "right-arm-command".to_owned(),
            "right-camera-ingest".to_owned(),
        ])
    );

    let forwarding = graph
        .nodes()
        .iter()
        .filter_map(|node| match node.data() {
            ConnectivityNodeData::StreamForwarding { forwarding } => Some((
                forwarding.from.participant.network.clone(),
                forwarding.from.participant.participant.clone(),
                forwarding.function.component.clone(),
                forwarding.function.function.clone(),
                forwarding.to.participant.network.clone(),
                forwarding.to.participant.participant.clone(),
            )),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        forwarding,
        BTreeSet::from([
            (
                "j100_to_head".to_owned(),
                "s32j100".to_owned(),
                "s32j100".to_owned(),
                "s32j100-integrated".to_owned(),
                "n79_to_j100_p0".to_owned(),
                "s32j100".to_owned(),
            ),
            (
                "j100_to_lidar_f".to_owned(),
                "s32j100".to_owned(),
                "s32j100".to_owned(),
                "s32j100-integrated".to_owned(),
                "n79_to_j100_p0".to_owned(),
                "s32j100".to_owned(),
            ),
            (
                "j100_to_lidar_r".to_owned(),
                "s32j100".to_owned(),
                "s32j100".to_owned(),
                "s32j100-integrated".to_owned(),
                "n79_to_j100_p0".to_owned(),
                "s32j100".to_owned(),
            ),
            (
                "j100_to_radar".to_owned(),
                "s32j100".to_owned(),
                "s32j100".to_owned(),
                "s32j100-integrated".to_owned(),
                "n79_to_j100_p0".to_owned(),
                "s32j100".to_owned(),
            ),
            (
                "left-wrist-camera".to_owned(),
                "mcn3-l".to_owned(),
                "mcn3-l".to_owned(),
                "integrated_switch".to_owned(),
                "left-arm-chain".to_owned(),
                "mcn3-l-in".to_owned(),
            ),
            (
                "right-wrist-camera".to_owned(),
                "mcn3-r".to_owned(),
                "mcn3-r".to_owned(),
                "integrated_switch".to_owned(),
                "right-arm-chain".to_owned(),
                "mcn3-r-in".to_owned(),
            ),
        ])
    );

    assert_eq!(
        stream_ids(&projected),
        stream_ids(&project_operational_profile()),
        "stable stream identities must not depend on a repeated load"
    );
}

#[test]
fn document_set_never_projects_out_of_contract_sidecars_as_valid() {
    const ROOT: &[u8] = br#"<hcdf name="root" version="1.0">
  <comp name="talker"><port name="p"/></comp>
  <comp name="listener"><port name="p"/></comp>
  <link name="n">
    <selected purpose="communication" carrier="electrical"/>
    <participant name="t"><endpoint><port-ref component="talker" port="p"/></endpoint></participant>
    <participant name="l"><endpoint><port-ref component="listener" port="p"/></endpoint></participant>
  </link>
  <stream-profile uri="bad.streams.xml"/>
</hcdf>"#;
    let cases = [
        br#"<stream-profile name="p" version="1.0">
  <stream name="s" pcp="8" max-frame-size-bytes="1" interval-ns="1">
    <path><network-ref network="n"/></path>
    <talker><participant-ref network="n" participant="t"/></talker>
    <listener><participant-ref network="n" participant="l"/></listener>
  </stream>
</stream-profile>"#
            .as_slice(),
        br#"<stream-profile name="p" version="1.0">
  <stream name="s" max-frame-size-bytes="1" interval-ns="1">
    <talker><participant-ref network="n" participant="t"/></talker>
    <path><network-ref network="n"/></path>
    <listener><participant-ref network="n" participant="l"/></listener>
  </stream>
</stream-profile>"#
            .as_slice(),
        br#"<stream-profile name="p" version="1.0">
  <stream name="s" max-frame-size-bytes="1" interval-ns="1">
    <path><network-ref network="n"><instance/></network-ref></path>
    <talker><participant-ref network="n" participant="t"/></talker>
    <listener><participant-ref network="n" participant="l"/></listener>
  </stream>
</stream-profile>"#
            .as_slice(),
    ];

    for profile in cases {
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/examples/bad.streams.xml", profile)
            .unwrap();
        let projected = load_projected_document_set_from_bytes(
            ROOT.to_vec(),
            DocumentIdentity::new("root").unwrap(),
            DocumentResourceKey::new("/examples/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let ConnectivityProjection::Invalid { issues } = projected.connectivity() else {
            panic!("an out-of-contract required sidecar must invalidate connectivity projection");
        };
        assert!(!issues.is_empty());
        assert!(projected.connectivity().graph().is_none());
    }
}
