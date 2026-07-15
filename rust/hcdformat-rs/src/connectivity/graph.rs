use super::identity::{ObjectIdentity, ObjectKind, StableEdgeId, StableObjectId};
use crate::model::connectivity::{
    AssemblyKind, AssemblyRef, Capabilities, ChannelRef, ConnectivityFunctionRef, ConnectorRef,
    DocumentIdentity, EeeMode, Fidelity, FunctionKind, FunctionalEndpointRef, GptpClockKind,
    GptpPortDefaults, HopOwnerRef, HopRef, IncludeInstanceId, JunctionKind, JunctionRef, LegEnd,
    LegRef, MacsecEnforcement, MacsecPolicyRef, NetworkRef, NetworkSelection, OwnerRef,
    ParticipantRef, PathKind, PhysicalEndpointRef, PortRef, PositionKind, PositionRef, QualifiedId,
    Representation, ScheduleRef, StreamGroupRef, StructuralFrameRef, StructuralVisualRef, Topology,
    TopologySegmentRef, TrafficClassRef, TrafficPreemption,
};
use crate::model::stream_profile::{
    StreamDefinition, StreamForwarding, StreamGroup, StreamProfileDependency,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueLevel {
    Error,
    Warning,
}

/// One normalization or resolution diagnostic with stable source provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectivityIssue {
    level: IssueLevel,
    code: String,
    message: String,
    subject: ObjectIdentity,
    related: Vec<ObjectIdentity>,
}

impl ConnectivityIssue {
    pub(crate) fn error(
        code: &str,
        message: impl Into<String>,
        subject: ObjectIdentity,
        related: Vec<ObjectIdentity>,
    ) -> Self {
        Self {
            level: IssueLevel::Error,
            code: code.to_owned(),
            message: message.into(),
            subject,
            related,
        }
    }

    pub(crate) fn warning(
        code: &str,
        message: impl Into<String>,
        subject: ObjectIdentity,
        related: Vec<ObjectIdentity>,
    ) -> Self {
        Self {
            level: IssueLevel::Warning,
            code: code.to_owned(),
            message: message.into(),
            subject,
            related,
        }
    }

    pub fn level(&self) -> IssueLevel {
        self.level
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn subject(&self) -> &ObjectIdentity {
        &self.subject
    }

    pub fn subject_id(&self) -> StableObjectId {
        self.subject.stable_id()
    }

    pub fn related(&self) -> &[ObjectIdentity] {
        &self.related
    }
}

/// Typed content retained on each canonical graph node.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "data-kind", rename_all = "kebab-case")]
pub enum ConnectivityNodeData {
    Component,
    StructuralVisualRoot {
        model_backed: bool,
    },
    StructuralFrame,
    Port {
        capabilities: Capabilities,
    },
    Channel {
        capabilities: Capabilities,
        role: Option<QualifiedId>,
        local_group: Option<String>,
    },
    PhysicalAssembly {
        kind: AssemblyKind,
    },
    Connector {
        family: Option<QualifiedId>,
    },
    Position {
        kind: PositionKind,
        role: Option<QualifiedId>,
        local_group: Option<String>,
    },
    Antenna,
    Binding {
        fidelity: Fidelity,
    },
    Mate {
        fidelity: Fidelity,
    },
    PhysicalPath {
        kind: PathKind,
        fidelity: Fidelity,
        role: Option<QualifiedId>,
        local_group: Option<String>,
    },
    Junction {
        kind: JunctionKind,
        fidelity: Fidelity,
    },
    Termination {
        kind: QualifiedId,
        mounting: crate::model::connectivity::TerminationMounting,
        quantities: Vec<crate::model::connectivity::NamedQuantity>,
        profile: Option<QualifiedId>,
        fidelity: Fidelity,
    },
    Network {
        topology: Topology,
        description: Option<String>,
        participant_order: Vec<String>,
        hop_order: Vec<String>,
        leg_order: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        gptp_domain_order: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        traffic_class_order: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        gate_schedule_order: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        schedule_assignment_order: Vec<String>,
        coordinator: Option<ParticipantRef>,
        root: Option<HopRef>,
        selected: Box<NetworkSelection>,
    },
    Profile {
        name: String,
        version: String,
        description: Option<String>,
        dependencies: Vec<StreamProfileDependency>,
    },
    Group {
        group: StreamGroup,
    },
    Stream {
        definition: Box<StreamDefinition>,
    },
    StreamForwarding {
        forwarding: Box<StreamForwarding>,
    },
    Participant {
        role: Option<QualifiedId>,
    },
    Hop {
        owner: HopOwnerRef,
        role: Option<QualifiedId>,
        description: Option<String>,
        processing_delay_ns: Option<u64>,
    },
    Leg {
        from: LegEnd,
        to: LegEnd,
    },
    GptpDomain {
        number: u8,
        port_defaults: Option<GptpPortDefaults>,
        clock_order: Vec<String>,
    },
    GptpClock {
        kind: GptpClockKind,
        participant: ParticipantRef,
        gm_capable: bool,
        priority1: Option<u8>,
        priority2: Option<u8>,
        clock_class: Option<u8>,
        clock_accuracy: Option<u8>,
    },
    TrafficClass {
        number: u8,
        description: Option<String>,
        preemption: Option<TrafficPreemption>,
        pcp: BTreeSet<u8>,
    },
    GateSchedule {
        cycle_time_ns: u64,
        entry_order: Vec<u32>,
    },
    GateControlEntry {
        order: u32,
        duration_ns: u64,
        open_order: Vec<TrafficClassRef>,
    },
    ScheduleAssignment {
        schedule: ScheduleRef,
        target_order: Vec<ParticipantRef>,
    },
    PlcaConfiguration {
        max_node_id: u8,
        to_timer_bit_times: u16,
    },
    PlcaNode {
        node_id: u8,
        burst_count: u8,
        burst_timer_bit_times: u16,
        participant: ParticipantRef,
    },
    MacsecConfiguration {
        default_policy: Option<MacsecPolicyRef>,
    },
    MacsecPolicy {
        enforcement: MacsecEnforcement,
        cipher: Option<QualifiedId>,
        key_agreement: Option<QualifiedId>,
        confidentiality_offset: Option<u8>,
        rekey_interval_ns: Option<u64>,
        credential_store_ref: Option<String>,
    },
    MacsecOverride {
        target: TopologySegmentRef,
        policy: MacsecPolicyRef,
    },
    EeeConfiguration {
        default_mode: EeeMode,
    },
    EeeOverride {
        participant: ParticipantRef,
        mode: EeeMode,
    },
    ConnectivityFunction {
        kind: FunctionKind,
    },
    Representation {
        representation: Representation,
    },
}

/// Immutable canonical object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectivityNode {
    id: StableObjectId,
    identity: ObjectIdentity,
    data: ConnectivityNodeData,
}

impl ConnectivityNode {
    pub(crate) fn new(identity: ObjectIdentity, data: ConnectivityNodeData) -> Self {
        let id = identity.stable_id();
        Self { id, identity, data }
    }

    pub fn id(&self) -> &StableObjectId {
        &self.id
    }

    pub fn identity(&self) -> &ObjectIdentity {
        &self.identity
    }

    pub fn kind(&self) -> ObjectKind {
        self.identity.kind()
    }

    pub fn data(&self) -> &ConnectivityNodeData {
        &self.data
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Owns,
    Contains,
    Binding,
    Mate,
    MappedMate,
    PhysicalPathEndpoint,
    JunctionAttachment,
    TerminationAttachment,
    ParticipantEndpoint,
    /// Membership and distinguished topology-role edges point from the member to its containing network.
    NetworkMembership,
    TopologyMembership,
    StarCoordinator,
    TreeRoot,
    HopOwnership,
    LegFromHop,
    LegFromParticipant,
    LegToHop,
    LegToParticipant,
    GptpClockParticipant,
    GateEntryTrafficClass,
    ScheduleAssignmentParticipant,
    ScheduleAssignmentSchedule,
    PlcaNodeParticipant,
    PlcaCoordinator,
    MacsecDefaultPolicy,
    MacsecOverridePolicy,
    MacsecOverrideTarget,
    EeeOverrideParticipant,
    FunctionInput,
    FunctionOutput,
    FunctionBidirectional,
    AntennaFeed,
    AntennaRadiation,
    Representation,
    RepresentationFrame,
    RepresentationModelRoot,
    RouteWaypointFrame,
    ProfileDependency,
    ProfileContainment,
    StreamGroup,
    StreamPath,
    StreamForwarding,
    StreamForwardingFrom,
    StreamForwardingFunction,
    StreamForwardingTo,
    StreamTalker,
    StreamListener,
    StreamTrafficClass,
    StreamSchedule,
}

impl EdgeKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Owns => "owns",
            Self::Contains => "contains",
            Self::Binding => "binding",
            Self::Mate => "mate",
            Self::MappedMate => "mapped-mate",
            Self::PhysicalPathEndpoint => "physical-path-endpoint",
            Self::JunctionAttachment => "junction-attachment",
            Self::TerminationAttachment => "termination-attachment",
            Self::ParticipantEndpoint => "participant-endpoint",
            Self::NetworkMembership => "network-membership",
            Self::TopologyMembership => "topology-membership",
            Self::StarCoordinator => "star-coordinator",
            Self::TreeRoot => "tree-root",
            Self::HopOwnership => "hop-ownership",
            Self::LegFromHop => "leg-from-hop",
            Self::LegFromParticipant => "leg-from-participant",
            Self::LegToHop => "leg-to-hop",
            Self::LegToParticipant => "leg-to-participant",
            Self::GptpClockParticipant => "gptp-clock-participant",
            Self::GateEntryTrafficClass => "gate-entry-traffic-class",
            Self::ScheduleAssignmentParticipant => "schedule-assignment-participant",
            Self::ScheduleAssignmentSchedule => "schedule-assignment-schedule",
            Self::PlcaNodeParticipant => "plca-node-participant",
            Self::PlcaCoordinator => "plca-coordinator",
            Self::MacsecDefaultPolicy => "macsec-default-policy",
            Self::MacsecOverridePolicy => "macsec-override-policy",
            Self::MacsecOverrideTarget => "macsec-override-target",
            Self::EeeOverrideParticipant => "eee-override-participant",
            Self::FunctionInput => "function-input",
            Self::FunctionOutput => "function-output",
            Self::FunctionBidirectional => "function-bidirectional",
            Self::AntennaFeed => "antenna-feed",
            Self::AntennaRadiation => "antenna-radiation",
            Self::Representation => "representation",
            Self::RepresentationFrame => "representation-frame",
            Self::RepresentationModelRoot => "representation-model-root",
            Self::RouteWaypointFrame => "route-waypoint-frame",
            Self::ProfileDependency => "profile-dependency",
            Self::ProfileContainment => "profile-containment",
            Self::StreamGroup => "stream-group",
            Self::StreamPath => "stream-path",
            Self::StreamForwarding => "stream-forwarding",
            Self::StreamForwardingFrom => "stream-forwarding-from",
            Self::StreamForwardingFunction => "stream-forwarding-function",
            Self::StreamForwardingTo => "stream-forwarding-to",
            Self::StreamTalker => "stream-talker",
            Self::StreamListener => "stream-listener",
            Self::StreamTrafficClass => "stream-traffic-class",
            Self::StreamSchedule => "stream-schedule",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeExactness {
    Coarse,
    Exact,
}

/// Immutable canonical graph relationship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NormalizedConnectivityEdge {
    id: StableEdgeId,
    kind: EdgeKind,
    from: StableObjectId,
    to: StableObjectId,
    subject: StableObjectId,
    exactness: EdgeExactness,
}

impl NormalizedConnectivityEdge {
    pub(crate) fn new(
        id: StableEdgeId,
        kind: EdgeKind,
        from: StableObjectId,
        to: StableObjectId,
        subject: StableObjectId,
        exactness: EdgeExactness,
    ) -> Self {
        Self {
            id,
            kind,
            from,
            to,
            subject,
            exactness,
        }
    }

    pub(crate) fn canonical(
        kind: EdgeKind,
        from: StableObjectId,
        to: StableObjectId,
        subject: StableObjectId,
        exactness: EdgeExactness,
        discriminator: &str,
    ) -> Self {
        let fields = [
            kind.as_str().as_bytes(),
            from.as_str().as_bytes(),
            to.as_str().as_bytes(),
            subject.as_str().as_bytes(),
            discriminator.as_bytes(),
        ];
        let id = StableEdgeId(format!(
            "hcdf-edge-v1:{}",
            super::identity::stable_digest("hcdf-connectivity-edge-v1", fields)
        ));
        Self::new(id, kind, from, to, subject, exactness)
    }

    pub fn id(&self) -> &StableEdgeId {
        &self.id
    }

    pub fn kind(&self) -> EdgeKind {
        self.kind
    }

    pub fn from(&self) -> &StableObjectId {
        &self.from
    }

    pub fn to(&self) -> &StableObjectId {
        &self.to
    }

    pub fn subject(&self) -> &StableObjectId {
        &self.subject
    }

    pub fn exactness(&self) -> EdgeExactness {
        self.exactness
    }
}

/// One canonical edge requested by a graph extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectivityGraphExtensionEdge {
    kind: EdgeKind,
    from: StableObjectId,
    to: StableObjectId,
    subject: StableObjectId,
    exactness: EdgeExactness,
    discriminator: String,
}

impl ConnectivityGraphExtensionEdge {
    pub(crate) fn new(
        kind: EdgeKind,
        from: StableObjectId,
        to: StableObjectId,
        subject: StableObjectId,
        exactness: EdgeExactness,
        discriminator: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            from,
            to,
            subject,
            exactness,
            discriminator: discriminator.into(),
        }
    }

    fn into_edge(self) -> NormalizedConnectivityEdge {
        NormalizedConnectivityEdge::canonical(
            self.kind,
            self.from,
            self.to,
            self.subject,
            self.exactness,
            &self.discriminator,
        )
    }
}

/// Additional canonical nodes and relationships produced from loaded document dependencies.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConnectivityGraphExtension {
    nodes: Vec<ConnectivityNode>,
    edges: Vec<ConnectivityGraphExtensionEdge>,
    stream_group_targets: Vec<StreamGroupReferenceTarget>,
}

impl ConnectivityGraphExtension {
    pub(crate) fn new(
        nodes: Vec<ConnectivityNode>,
        edges: Vec<ConnectivityGraphExtensionEdge>,
    ) -> Self {
        Self {
            nodes,
            edges,
            stream_group_targets: Vec::new(),
        }
    }

    pub(crate) fn with_stream_group_targets(
        mut self,
        targets: Vec<StreamGroupReferenceTarget>,
    ) -> Self {
        self.stream_group_targets.extend(targets);
        self
    }
}

/// One exact stream-group reference key and its canonical graph target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamGroupReferenceTarget {
    key: ReferenceKey,
    target: StableObjectId,
}

impl StreamGroupReferenceTarget {
    pub(crate) fn new(
        instance: IncludeInstanceId,
        group: impl Into<String>,
        target: StableObjectId,
    ) -> Self {
        Self {
            key: ReferenceKey::StreamGroup {
                instance,
                group: group.into(),
            },
            target,
        }
    }
}

/// Failure to merge additional canonical objects into an already normalized graph.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ConnectivityGraphExtensionError {
    #[error("graph extension contains duplicate node identity: {0}")]
    DuplicateNodeIdentity(String),
    #[error("graph extension contains duplicate stable node ID: {0}")]
    DuplicateNodeId(StableObjectId),
    #[error("graph extension contains duplicate stable edge ID: {0}")]
    DuplicateEdgeId(StableEdgeId),
    #[error("{kind:?} edge has no {role} node: {id}")]
    MissingEdgeEndpoint {
        kind: EdgeKind,
        role: &'static str,
        id: StableObjectId,
    },
    #[error(
        "graph extension duplicates reference key {key:?}: {existing} conflicts with {target}"
    )]
    DuplicateReferenceKey {
        key: Box<ReferenceKey>,
        existing: StableObjectId,
        target: StableObjectId,
    },
    #[error("graph extension reference key {key:?} targets missing node {target}")]
    MissingReferenceTarget {
        key: Box<ReferenceKey>,
        target: StableObjectId,
    },
    #[error(
        "graph extension reference key {key:?} expects {expected:?}, but target {target} is {actual:?}"
    )]
    ReferenceTargetKindMismatch {
        key: Box<ReferenceKey>,
        target: StableObjectId,
        expected: ObjectKind,
        actual: ObjectKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum OwnerKey {
    Component {
        instance: IncludeInstanceId,
        component: String,
    },
    Assembly {
        instance: IncludeInstanceId,
        assembly: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ReferenceKey {
    Component {
        instance: IncludeInstanceId,
        component: String,
    },
    Function {
        instance: IncludeInstanceId,
        component: String,
        function: String,
    },
    StructuralVisualRoot {
        instance: IncludeInstanceId,
        component: String,
        visual: String,
    },
    StructuralFrame {
        instance: IncludeInstanceId,
        component: String,
        frame: String,
    },
    Port {
        instance: IncludeInstanceId,
        component: String,
        port: String,
    },
    Channel {
        instance: IncludeInstanceId,
        component: String,
        port: String,
        channel: String,
    },
    Assembly {
        instance: IncludeInstanceId,
        assembly: String,
    },
    Connector {
        owner: OwnerKey,
        connector: String,
    },
    Position {
        owner: OwnerKey,
        connector: String,
        position: String,
    },
    Junction {
        owner: OwnerKey,
        junction: String,
    },
    Network {
        instance: IncludeInstanceId,
        network: String,
    },
    StreamGroup {
        instance: IncludeInstanceId,
        group: String,
    },
    Participant {
        instance: IncludeInstanceId,
        network: String,
        participant: String,
    },
    Hop {
        instance: IncludeInstanceId,
        network: String,
        hop: String,
    },
    Leg {
        instance: IncludeInstanceId,
        network: String,
        leg: String,
    },
    TrafficClass {
        instance: IncludeInstanceId,
        network: String,
        traffic_class: String,
    },
    GateSchedule {
        instance: IncludeInstanceId,
        network: String,
        schedule: String,
    },
    MacsecPolicy {
        instance: IncludeInstanceId,
        network: String,
        policy: String,
    },
}

impl ReferenceKey {
    pub(crate) fn describe(&self) -> String {
        format!("{self:?}")
    }
}

/// Read-only canonical connectivity graph.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NormalizedConnectivityGraph {
    document: DocumentIdentity,
    nodes: Vec<ConnectivityNode>,
    edges: Vec<NormalizedConnectivityEdge>,
    warnings: Vec<ConnectivityIssue>,
    #[serde(skip)]
    node_index: BTreeMap<StableObjectId, usize>,
    #[serde(skip)]
    reference_index: BTreeMap<ReferenceKey, StableObjectId>,
}

impl NormalizedConnectivityGraph {
    pub(crate) fn new(
        document: DocumentIdentity,
        nodes: Vec<ConnectivityNode>,
        edges: Vec<NormalizedConnectivityEdge>,
        warnings: Vec<ConnectivityIssue>,
        reference_index: BTreeMap<ReferenceKey, StableObjectId>,
    ) -> Self {
        let node_index = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.clone(), index))
            .collect();
        Self {
            document,
            nodes,
            edges,
            warnings,
            node_index,
            reference_index,
        }
    }

    pub fn document(&self) -> &DocumentIdentity {
        &self.document
    }

    pub fn nodes(&self) -> &[ConnectivityNode] {
        &self.nodes
    }

    pub fn edges(&self) -> &[NormalizedConnectivityEdge] {
        &self.edges
    }

    pub fn warnings(&self) -> &[ConnectivityIssue] {
        &self.warnings
    }

    pub fn node(&self, id: &StableObjectId) -> Option<&ConnectivityNode> {
        self.node_index.get(id).map(|index| &self.nodes[*index])
    }

    pub fn resolver(&self, source_instance: IncludeInstanceId) -> ConnectivityResolver<'_> {
        ConnectivityResolver {
            graph: self,
            source_instance,
        }
    }

    pub fn to_canonical_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub(crate) fn with_extension(
        self,
        extension: ConnectivityGraphExtension,
    ) -> Result<Self, ConnectivityGraphExtensionError> {
        let Self {
            document,
            nodes,
            edges,
            mut warnings,
            node_index: _,
            mut reference_index,
        } = self;
        let ConnectivityGraphExtension {
            nodes: mut extension_nodes,
            edges: extension_edges,
            stream_group_targets,
        } = extension;

        let mut identities = nodes
            .iter()
            .map(|node| node.identity.clone())
            .collect::<BTreeSet<_>>();
        let mut node_map = nodes
            .into_iter()
            .map(|node| (node.id.clone(), node))
            .collect::<BTreeMap<_, _>>();
        extension_nodes.sort_by(|first, second| {
            first
                .identity
                .cmp(&second.identity)
                .then_with(|| first.id.cmp(&second.id))
        });
        for node in extension_nodes {
            if !identities.insert(node.identity.clone()) {
                return Err(ConnectivityGraphExtensionError::DuplicateNodeIdentity(
                    node.identity.display_path(),
                ));
            }
            let id = node.id.clone();
            if node_map.insert(id.clone(), node).is_some() {
                return Err(ConnectivityGraphExtensionError::DuplicateNodeId(id));
            }
        }

        let mut stream_group_targets = stream_group_targets;
        stream_group_targets.sort_by(|first, second| {
            first
                .key
                .cmp(&second.key)
                .then_with(|| first.target.cmp(&second.target))
        });
        for StreamGroupReferenceTarget { key, target } in stream_group_targets {
            if let Some(existing) = reference_index.get(&key) {
                return Err(ConnectivityGraphExtensionError::DuplicateReferenceKey {
                    key: Box::new(key),
                    existing: existing.clone(),
                    target,
                });
            }
            let Some(target_node) = node_map.get(&target) else {
                return Err(ConnectivityGraphExtensionError::MissingReferenceTarget {
                    key: Box::new(key),
                    target,
                });
            };
            if target_node.kind() != ObjectKind::Group {
                return Err(
                    ConnectivityGraphExtensionError::ReferenceTargetKindMismatch {
                        key: Box::new(key),
                        target,
                        expected: ObjectKind::Group,
                        actual: target_node.kind(),
                    },
                );
            }
            reference_index.insert(key, target);
        }

        let mut edge_map = edges
            .into_iter()
            .map(|edge| (edge.id.clone(), edge))
            .collect::<BTreeMap<_, _>>();
        let mut extension_edges = extension_edges
            .into_iter()
            .map(ConnectivityGraphExtensionEdge::into_edge)
            .collect::<Vec<_>>();
        extension_edges.sort_by(|first, second| first.id.cmp(&second.id));
        for edge in extension_edges {
            for (role, endpoint) in [
                ("from", &edge.from),
                ("to", &edge.to),
                ("subject", &edge.subject),
            ] {
                if !node_map.contains_key(endpoint) {
                    return Err(ConnectivityGraphExtensionError::MissingEdgeEndpoint {
                        kind: edge.kind,
                        role,
                        id: endpoint.clone(),
                    });
                }
            }
            let id = edge.id.clone();
            if edge_map.insert(id.clone(), edge).is_some() {
                return Err(ConnectivityGraphExtensionError::DuplicateEdgeId(id));
            }
        }

        warnings.sort_by(|first, second| {
            first
                .subject_id()
                .cmp(&second.subject_id())
                .then_with(|| first.code().cmp(second.code()))
                .then_with(|| first.message().cmp(second.message()))
        });
        Ok(Self::new(
            document,
            node_map.into_values().collect(),
            edge_map.into_values().collect(),
            warnings,
            reference_index,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unresolved {expected:?} reference: {target}")]
pub struct ResolveError {
    expected: ObjectKind,
    target: String,
}

impl ResolveError {
    pub fn expected(&self) -> ObjectKind {
        self.expected
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}

/// Exact typed reference resolver over an immutable graph.
pub struct ConnectivityResolver<'a> {
    graph: &'a NormalizedConnectivityGraph,
    source_instance: IncludeInstanceId,
}

impl<'a> ConnectivityResolver<'a> {
    pub fn component(
        &self,
        reference: &crate::model::connectivity::ComponentRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            ReferenceKey::Component {
                instance: reference.scope.resolve(&self.source_instance),
                component: reference.component.clone(),
            },
            ObjectKind::Component,
        )
    }
    pub fn function(
        &self,
        reference: &ConnectivityFunctionRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            function_key(reference, &self.source_instance),
            ObjectKind::ConnectivityFunction,
        )
    }

    pub fn assembly(&self, reference: &AssemblyRef) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            ReferenceKey::Assembly {
                instance: reference.scope.resolve(&self.source_instance),
                assembly: reference.assembly.clone(),
            },
            ObjectKind::PhysicalAssembly,
        )
    }

    pub fn visual_root(
        &self,
        reference: &StructuralVisualRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            structural_visual_key(reference, &self.source_instance),
            ObjectKind::StructuralVisualRoot,
        )
    }

    pub fn frame(
        &self,
        reference: &StructuralFrameRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            structural_frame_key(reference, &self.source_instance),
            ObjectKind::StructuralFrame,
        )
    }

    pub fn port(&self, reference: &PortRef) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(port_key(reference, &self.source_instance), ObjectKind::Port)
    }

    pub fn channel(&self, reference: &ChannelRef) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            channel_key(reference, &self.source_instance),
            ObjectKind::Channel,
        )
    }

    pub fn connector(
        &self,
        reference: &ConnectorRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            connector_key(reference, &self.source_instance),
            ObjectKind::Connector,
        )
    }

    pub fn position(&self, reference: &PositionRef) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            position_key(reference, &self.source_instance),
            ObjectKind::Position,
        )
    }

    pub fn junction(&self, reference: &JunctionRef) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            junction_key(reference, &self.source_instance),
            ObjectKind::Junction,
        )
    }

    pub fn network(&self, reference: &NetworkRef) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            network_key(reference, &self.source_instance),
            ObjectKind::Network,
        )
    }

    pub fn stream_group(
        &self,
        reference: &StreamGroupRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            stream_group_key(reference, &self.source_instance),
            ObjectKind::Group,
        )
    }
    pub fn participant(
        &self,
        reference: &ParticipantRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            participant_key(reference, &self.source_instance),
            ObjectKind::Participant,
        )
    }

    pub fn hop(&self, reference: &HopRef) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(hop_key(reference, &self.source_instance), ObjectKind::Hop)
    }

    pub fn leg(&self, reference: &LegRef) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(leg_key(reference, &self.source_instance), ObjectKind::Leg)
    }

    pub fn traffic_class(
        &self,
        reference: &TrafficClassRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            traffic_class_key(reference, &self.source_instance),
            ObjectKind::TrafficClass,
        )
    }

    pub fn gate_schedule(
        &self,
        reference: &ScheduleRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            schedule_key(reference, &self.source_instance),
            ObjectKind::GateSchedule,
        )
    }

    pub fn macsec_policy(
        &self,
        reference: &MacsecPolicyRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        self.resolve(
            macsec_policy_key(reference, &self.source_instance),
            ObjectKind::MacsecPolicy,
        )
    }

    pub fn hop_owner(&self, reference: &HopOwnerRef) -> Result<&'a ConnectivityNode, ResolveError> {
        match reference {
            HopOwnerRef::Component(reference) => self.component(reference),
            HopOwnerRef::Function(reference) => self.function(reference),
        }
    }

    pub fn functional(
        &self,
        reference: &FunctionalEndpointRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        match reference {
            FunctionalEndpointRef::Port(reference) => self.port(reference),
            FunctionalEndpointRef::Channel(reference) => self.channel(reference),
        }
    }

    pub fn physical(
        &self,
        reference: &PhysicalEndpointRef,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        match reference {
            PhysicalEndpointRef::Connector(reference) => self.connector(reference),
            PhysicalEndpointRef::Position(reference) => self.position(reference),
            PhysicalEndpointRef::Junction(reference) => self.junction(reference),
        }
    }

    fn resolve(
        &self,
        key: ReferenceKey,
        expected: ObjectKind,
    ) -> Result<&'a ConnectivityNode, ResolveError> {
        let target = key.describe();
        self.graph
            .reference_index
            .get(&key)
            .and_then(|id| self.graph.node(id))
            .ok_or(ResolveError { expected, target })
    }
}

pub(crate) fn owner_key(owner: &OwnerRef, source: &IncludeInstanceId) -> OwnerKey {
    match owner {
        OwnerRef::Component(reference) => OwnerKey::Component {
            instance: reference.scope.resolve(source),
            component: reference.component.clone(),
        },
        OwnerRef::Assembly(reference) => OwnerKey::Assembly {
            instance: reference.scope.resolve(source),
            assembly: reference.assembly.clone(),
        },
    }
}

pub(crate) fn structural_visual_key(
    reference: &StructuralVisualRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    ReferenceKey::StructuralVisualRoot {
        instance: reference.component.scope.resolve(source),
        component: reference.component.component.clone(),
        visual: reference.visual.clone(),
    }
}

pub(crate) fn structural_frame_key(
    reference: &StructuralFrameRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    ReferenceKey::StructuralFrame {
        instance: reference.component.scope.resolve(source),
        component: reference.component.component.clone(),
        frame: reference.frame.clone(),
    }
}

pub(crate) fn function_key(
    reference: &ConnectivityFunctionRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    ReferenceKey::Function {
        instance: reference.component.scope.resolve(source),
        component: reference.component.component.clone(),
        function: reference.function.clone(),
    }
}

pub(crate) fn port_key(reference: &PortRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::Port {
        instance: reference.scope.resolve(source),
        component: reference.component.clone(),
        port: reference.port.clone(),
    }
}

pub(crate) fn channel_key(reference: &ChannelRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::Channel {
        instance: reference.scope.resolve(source),
        component: reference.component.clone(),
        port: reference.port.clone(),
        channel: reference.channel.clone(),
    }
}

pub(crate) fn functional_key(
    reference: &FunctionalEndpointRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    match reference {
        FunctionalEndpointRef::Port(reference) => port_key(reference, source),
        FunctionalEndpointRef::Channel(reference) => channel_key(reference, source),
    }
}

pub(crate) fn connector_key(reference: &ConnectorRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::Connector {
        owner: owner_key(&reference.owner, source),
        connector: reference.connector.clone(),
    }
}

pub(crate) fn position_key(reference: &PositionRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::Position {
        owner: owner_key(&reference.owner, source),
        connector: reference.connector.clone(),
        position: reference.position.clone(),
    }
}

pub(crate) fn junction_key(reference: &JunctionRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::Junction {
        owner: owner_key(&reference.owner, source),
        junction: reference.junction.clone(),
    }
}

pub(crate) fn network_key(reference: &NetworkRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::Network {
        instance: reference.scope.resolve(source),
        network: reference.network.clone(),
    }
}

pub(crate) fn stream_group_key(
    reference: &StreamGroupRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    ReferenceKey::StreamGroup {
        instance: reference.scope.resolve(source),
        group: reference.group.clone(),
    }
}
pub(crate) fn participant_key(
    reference: &ParticipantRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    ReferenceKey::Participant {
        instance: reference.network.scope.resolve(source),
        network: reference.network.network.clone(),
        participant: reference.participant.clone(),
    }
}

pub(crate) fn hop_key(reference: &HopRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::Hop {
        instance: reference.network.scope.resolve(source),
        network: reference.network.network.clone(),
        hop: reference.hop.clone(),
    }
}

pub(crate) fn leg_key(reference: &LegRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::Leg {
        instance: reference.network.scope.resolve(source),
        network: reference.network.network.clone(),
        leg: reference.leg.clone(),
    }
}

pub(crate) fn traffic_class_key(
    reference: &TrafficClassRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    ReferenceKey::TrafficClass {
        instance: reference.network.scope.resolve(source),
        network: reference.network.network.clone(),
        traffic_class: reference.traffic_class.clone(),
    }
}

pub(crate) fn schedule_key(reference: &ScheduleRef, source: &IncludeInstanceId) -> ReferenceKey {
    ReferenceKey::GateSchedule {
        instance: reference.network.scope.resolve(source),
        network: reference.network.network.clone(),
        schedule: reference.schedule.clone(),
    }
}

pub(crate) fn macsec_policy_key(
    reference: &MacsecPolicyRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    ReferenceKey::MacsecPolicy {
        instance: reference.network.scope.resolve(source),
        network: reference.network.network.clone(),
        policy: reference.policy.clone(),
    }
}

pub(crate) fn physical_key(
    reference: &PhysicalEndpointRef,
    source: &IncludeInstanceId,
) -> ReferenceKey {
    match reference {
        PhysicalEndpointRef::Connector(reference) => connector_key(reference, source),
        PhysicalEndpointRef::Position(reference) => position_key(reference, source),
        PhysicalEndpointRef::Junction(reference) => junction_key(reference, source),
    }
}

impl fmt::Debug for ConnectivityResolver<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectivityResolver")
            .field("document", self.graph.document())
            .field("source_instance", &self.source_instance)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::identity::IdentityPart;

    fn document() -> DocumentIdentity {
        DocumentIdentity::new("graph-extension-test").expect("valid document identity")
    }

    fn identity(kind: ObjectKind, field: &str, value: &str) -> ObjectIdentity {
        ObjectIdentity::new(
            document(),
            IncludeInstanceId::root(),
            kind,
            vec![IdentityPart::new(field, value)],
        )
    }

    fn profile_node(name: &str) -> ConnectivityNode {
        ConnectivityNode::new(
            identity(ObjectKind::Profile, "profile", name),
            ConnectivityNodeData::Profile {
                name: name.to_owned(),
                version: "1".to_owned(),
                description: None,
                dependencies: Vec::new(),
            },
        )
    }

    fn group_node(name: &str) -> ConnectivityNode {
        ConnectivityNode::new(
            identity(ObjectKind::Group, "group", name),
            ConnectivityNodeData::Group {
                group: StreamGroup {
                    name: name.to_owned(),
                    description: None,
                },
            },
        )
    }

    fn base_graph() -> NormalizedConnectivityGraph {
        let component = ConnectivityNode::new(
            identity(ObjectKind::Component, "component", "controller"),
            ConnectivityNodeData::Component,
        );
        let mut references = BTreeMap::new();
        references.insert(
            ReferenceKey::Component {
                instance: IncludeInstanceId::root(),
                component: "controller".to_owned(),
            },
            component.id().clone(),
        );
        NormalizedConnectivityGraph::new(
            document(),
            vec![component],
            Vec::new(),
            Vec::new(),
            references,
        )
    }

    #[test]
    fn extension_is_canonical_and_preserves_reference_index() {
        let profile = profile_node("factory");
        let group = group_node("motion");
        let edge = ConnectivityGraphExtensionEdge::new(
            EdgeKind::ProfileContainment,
            profile.id().clone(),
            group.id().clone(),
            profile.id().clone(),
            EdgeExactness::Exact,
            "group:0",
        );
        let first = base_graph()
            .with_extension(ConnectivityGraphExtension::new(
                vec![group.clone(), profile.clone()],
                vec![edge.clone()],
            ))
            .expect("extension is valid");
        let second = base_graph()
            .with_extension(ConnectivityGraphExtension::new(
                vec![profile, group],
                vec![edge],
            ))
            .expect("extension is valid");

        assert_eq!(first, second);
        assert!(first
            .nodes()
            .windows(2)
            .all(|pair| pair[0].id() < pair[1].id()));
        assert_eq!(first.edges()[0].kind(), EdgeKind::ProfileContainment);
        assert!(first.edges()[0].id().as_str().starts_with("hcdf-edge-v1:"));
        assert_eq!(first.reference_index, base_graph().reference_index);
    }

    #[test]
    fn extension_rejects_duplicate_node_identity() {
        let profile = profile_node("factory");
        let error = base_graph()
            .with_extension(ConnectivityGraphExtension::new(
                vec![profile.clone(), profile],
                Vec::new(),
            ))
            .expect_err("duplicate identities must fail");
        assert!(matches!(
            error,
            ConnectivityGraphExtensionError::DuplicateNodeIdentity(_)
        ));
    }

    #[test]
    fn stream_group_target_resolves_and_preserves_existing_references() {
        let group = group_node("motion");
        let group_id = group.id().clone();
        let graph = base_graph()
            .with_extension(
                ConnectivityGraphExtension::new(vec![group], Vec::new()).with_stream_group_targets(
                    vec![StreamGroupReferenceTarget::new(
                        IncludeInstanceId::root(),
                        "motion",
                        group_id.clone(),
                    )],
                ),
            )
            .expect("stream group target is valid");
        let resolver = graph.resolver(IncludeInstanceId::root());

        assert_eq!(
            resolver
                .stream_group(&StreamGroupRef::local("motion"))
                .expect("stream group resolves")
                .id(),
            &group_id
        );
        assert_eq!(
            resolver
                .component(&crate::model::connectivity::ComponentRef::local(
                    "controller",
                ))
                .expect("existing component reference remains resolvable")
                .kind(),
            ObjectKind::Component
        );
    }

    #[test]
    fn extension_rejects_duplicate_stream_group_keys() {
        let group = group_node("motion");
        let target = StreamGroupReferenceTarget::new(
            IncludeInstanceId::root(),
            "motion",
            group.id().clone(),
        );
        let error = base_graph()
            .with_extension(
                ConnectivityGraphExtension::new(vec![group], Vec::new())
                    .with_stream_group_targets(vec![target.clone(), target]),
            )
            .expect_err("duplicate extension keys must fail");
        assert!(matches!(
            error,
            ConnectivityGraphExtensionError::DuplicateReferenceKey {
                key,
                ..
            } if matches!(*key, ReferenceKey::StreamGroup { .. })
        ));

        let group = group_node("preserved");
        let group_id = group.id().clone();
        let graph = base_graph()
            .with_extension(
                ConnectivityGraphExtension::new(vec![group], Vec::new()).with_stream_group_targets(
                    vec![StreamGroupReferenceTarget::new(
                        IncludeInstanceId::root(),
                        "preserved",
                        group_id.clone(),
                    )],
                ),
            )
            .expect("first key is valid");
        let error = graph
            .with_extension(
                ConnectivityGraphExtension::new(Vec::new(), Vec::new()).with_stream_group_targets(
                    vec![StreamGroupReferenceTarget::new(
                        IncludeInstanceId::root(),
                        "preserved",
                        group_id,
                    )],
                ),
            )
            .expect_err("duplicate existing keys must fail");
        assert!(matches!(
            error,
            ConnectivityGraphExtensionError::DuplicateReferenceKey {
                key,
                ..
            } if matches!(*key, ReferenceKey::StreamGroup { .. })
        ));
    }

    #[test]
    fn extension_rejects_missing_stream_group_target() {
        let absent = group_node("absent");
        let error = base_graph()
            .with_extension(
                ConnectivityGraphExtension::new(Vec::new(), Vec::new()).with_stream_group_targets(
                    vec![StreamGroupReferenceTarget::new(
                        IncludeInstanceId::root(),
                        "absent",
                        absent.id().clone(),
                    )],
                ),
            )
            .expect_err("missing reference targets must fail");
        assert!(matches!(
            error,
            ConnectivityGraphExtensionError::MissingReferenceTarget {
                key,
                ..
            } if matches!(*key, ReferenceKey::StreamGroup { .. })
        ));
    }

    #[test]
    fn extension_rejects_missing_edge_endpoint() {
        let profile = profile_node("factory");
        let absent = group_node("absent");
        let error = base_graph()
            .with_extension(ConnectivityGraphExtension::new(
                vec![profile.clone()],
                vec![ConnectivityGraphExtensionEdge::new(
                    EdgeKind::ProfileContainment,
                    profile.id().clone(),
                    absent.id().clone(),
                    profile.id().clone(),
                    EdgeExactness::Exact,
                    "group:0",
                )],
            ))
            .expect_err("missing endpoints must fail");
        assert!(matches!(
            error,
            ConnectivityGraphExtensionError::MissingEdgeEndpoint { role: "to", .. }
        ));
    }

    #[test]
    fn profile_node_serialization_retains_dependency_metadata() {
        let node = ConnectivityNode::new(
            identity(ObjectKind::Profile, "profile", "factory"),
            ConnectivityNodeData::Profile {
                name: "factory".to_owned(),
                version: "2026.07".to_owned(),
                description: Some("Factory traffic".to_owned()),
                dependencies: vec![StreamProfileDependency {
                    uri: "base.stream-profile.xml".to_owned(),
                    sha: Some("sha256:0123".to_owned()),
                    required: false,
                }],
            },
        );
        let json = serde_json::to_value(node).expect("profile node serializes");
        assert_eq!(json["data"]["data-kind"], "profile");
        assert_eq!(
            json["data"]["dependencies"][0]["@uri"],
            "base.stream-profile.xml"
        );
        assert_eq!(json["data"]["dependencies"][0]["@required"], false);
    }
}
