use super::graph::{
    connector_key, function_key, functional_key, hop_key, leg_key, macsec_policy_key, network_key,
    owner_key, participant_key, physical_key, port_key, position_key, schedule_key,
    structural_frame_key, structural_visual_key, traffic_class_key, ConnectivityIssue,
    ConnectivityNode, ConnectivityNodeData, EdgeExactness, EdgeKind, IssueLevel, OwnerKey,
    ReferenceKey,
};
use super::identity::{
    stable_digest, structural_component_identity, structural_frame_identity,
    structural_visual_identity, IdentityPart, ObjectIdentity, ObjectKind, StableEdgeId,
    StableObjectId,
};
use super::profile_registry::{
    built_in_profile_registry, ProfileRegistry, ProfileRegistryError, ProfileRuleset, ValidatorKind,
};
use super::units::{
    definitely_greater_normalized, definitely_less_normalized, normalize_quantity, Dimension,
    NormalizedQuantity,
};
use super::{NormalizedConnectivityEdge, NormalizedConnectivityGraph};
use crate::model::connectivity as authored;
use crate::model::connectivity::{
    ConnectivityDocument, ConnectivityScope, Connector, Fidelity, FunctionalEndpointRef,
    HopOwnerRef, IncludeInstanceId, Junction, LegEnd, Mate, Network, NetworkSelection,
    NetworkStructure, ParticipantRef, PhysicalEndpointRef, PhysicalPath, Representation,
    StructuralFrameRef, StructuralVisualRef, Termination, Topology,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Blocking failure returned when authored connectivity cannot produce one truthful graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizationError {
    issues: Vec<ConnectivityIssue>,
}

impl NormalizationError {
    pub fn issues(&self) -> &[ConnectivityIssue] {
        &self.issues
    }

    pub fn into_issues(self) -> Vec<ConnectivityIssue> {
        self.issues
    }
}

impl fmt::Display for NormalizationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "connectivity normalization failed with {} issue(s)",
            self.issues.len()
        )
    }
}

impl std::error::Error for NormalizationError {}

/// Treatment of selected profiles that are not present in the trusted built-in registry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProfileRegistryCompleteness {
    /// Preserve the graph and report each unknown selected profile as an advisory.
    #[default]
    Advisory,
    /// Treat every unknown selected profile as a blocking validation error.
    Strict,
}

/// Options controlling canonical connectivity normalization and validation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NormalizationOptions {
    /// Whether a selected profile missing from the trusted built-in registry is advisory or fatal.
    pub profile_registry_completeness: ProfileRegistryCompleteness,
}

/// Normalize authored connectivity into the sole canonical graph consumed by downstream tools.
pub fn normalize_connectivity(
    authored: &ConnectivityDocument,
) -> Result<NormalizedConnectivityGraph, NormalizationError> {
    normalize_connectivity_with_options(authored, NormalizationOptions::default())
}

/// Normalize authored connectivity with an explicit profile-registry completeness policy.
pub fn normalize_connectivity_with_options(
    authored: &ConnectivityDocument,
    options: NormalizationOptions,
) -> Result<NormalizedConnectivityGraph, NormalizationError> {
    let registry =
        built_in_profile_registry().map_err(|error| profile_registry_failure(authored, &error))?;
    normalize_connectivity_with_registry(authored, options, registry)
}

pub(super) fn normalize_connectivity_with_registry(
    authored: &ConnectivityDocument,
    options: NormalizationOptions,
    registry: &ProfileRegistry,
) -> Result<NormalizedConnectivityGraph, NormalizationError> {
    let mut builder = Builder::new(authored, options, registry);
    builder.validate_document_shape();
    for scope in &authored.scopes {
        builder.register_scope(scope);
    }
    for scope in &authored.scopes {
        builder.connect_scope(scope);
    }
    for scope in &authored.scopes {
        builder.connect_networks(scope);
    }
    builder.finalize_connections();
    builder.finish()
}

pub(super) fn profile_registry_failure(
    authored: &ConnectivityDocument,
    error: &ProfileRegistryError,
) -> NormalizationError {
    let subject = ObjectIdentity::new(
        authored.document.clone(),
        IncludeInstanceId::root(),
        ObjectKind::Scope,
        Vec::new(),
    );
    NormalizationError {
        issues: vec![ConnectivityIssue::error(
            "E_CONN_PROFILE_REGISTRY",
            format!("the trusted connectivity profile registry is invalid: {error}"),
            subject,
            Vec::new(),
        )],
    }
}

struct Builder<'a> {
    authored: &'a ConnectivityDocument,
    options: NormalizationOptions,
    profile_registry: &'a ProfileRegistry,
    nodes: BTreeMap<StableObjectId, ConnectivityNode>,
    identities: BTreeMap<ObjectIdentity, StableObjectId>,
    references: BTreeMap<ReferenceKey, StableObjectId>,
    edges: BTreeMap<StableEdgeId, NormalizedConnectivityEdge>,
    issues: Vec<ConnectivityIssue>,
    scopes: BTreeSet<IncludeInstanceId>,
    representations: Vec<PendingRepresentation>,
    pending_profile_validations: Vec<PendingProfileValidation<'a>>,
    graph_connections_complete: bool,
}

#[derive(Clone)]
struct PendingRepresentation {
    owner: ObjectIdentity,
    subject: ObjectIdentity,
    id: StableObjectId,
    representation: Representation,
}

struct PendingProfileValidation<'a> {
    scope: &'a ConnectivityScope,
    network: &'a Network,
    subject: ObjectIdentity,
    network_id: StableObjectId,
    profile: &'a authored::QualifiedId,
}

struct ProfileValidationContext<'a> {
    scope: &'a ConnectivityScope,
    network: &'a Network,
    subject: &'a ObjectIdentity,
    network_id: &'a StableObjectId,
    profile: &'a authored::QualifiedId,
}

impl<'a> Builder<'a> {
    fn new(
        authored: &'a ConnectivityDocument,
        options: NormalizationOptions,
        profile_registry: &'a ProfileRegistry,
    ) -> Self {
        Self {
            authored,
            options,
            profile_registry,
            nodes: BTreeMap::new(),
            identities: BTreeMap::new(),
            references: BTreeMap::new(),
            edges: BTreeMap::new(),
            issues: Vec::new(),
            scopes: BTreeSet::new(),
            representations: Vec::new(),
            pending_profile_validations: Vec::new(),
            graph_connections_complete: false,
        }
    }

    fn validate_document_shape(&mut self) {
        let declared_instances = self
            .authored
            .scopes
            .iter()
            .map(|scope| scope.instance.clone())
            .collect::<BTreeSet<_>>();
        let root = IncludeInstanceId::root();
        let root_count = self
            .authored
            .scopes
            .iter()
            .filter(|scope| scope.instance.is_root())
            .count();
        let root_identity = ObjectIdentity::new(
            self.authored.document.clone(),
            root,
            ObjectKind::Scope,
            Vec::new(),
        );
        if root_count != 1 {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_SCOPE_ROOT",
                "a connectivity document must contain exactly one root scope",
                root_identity,
                Vec::new(),
            ));
        }
        for scope in &self.authored.scopes {
            let scope_identity = ObjectIdentity::new(
                self.authored.document.clone(),
                scope.instance.clone(),
                ObjectKind::Scope,
                Vec::new(),
            );
            if let Some(parent) = scope.instance.parent() {
                if !declared_instances.contains(&parent) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_SCOPE_PARENT",
                        "every nonroot connectivity scope requires its immediate parent scope",
                        scope_identity.clone(),
                        vec![ObjectIdentity::new(
                            self.authored.document.clone(),
                            parent,
                            ObjectKind::Scope,
                            Vec::new(),
                        )],
                    ));
                }
            }
            if scope_has_empty_instance_reference(scope) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_EMPTY_INSTANCE_SCOPE",
                    "explicit instance reference scopes must contain at least one segment",
                    scope_identity,
                    Vec::new(),
                ));
            }
        }
    }

    fn register_scope(&mut self, scope: &ConnectivityScope) {
        let scope_identity = ObjectIdentity::new(
            self.authored.document.clone(),
            scope.instance.clone(),
            ObjectKind::Scope,
            Vec::new(),
        );
        if scope
            .instance
            .segments()
            .iter()
            .any(|segment| matches!(&segment.name, authored::IncludeSegmentName::Named(name) if name.is_empty()))
        {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_INSTANCE_SEGMENT",
                "named include instance segments must not be empty",
                scope_identity.clone(),
                Vec::new(),
            ));
        }
        if !self.scopes.insert(scope.instance.clone()) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_DUPLICATE_SCOPE",
                "each include instance path may identify only one connectivity scope",
                scope_identity,
                Vec::new(),
            ));
            return;
        }
        for component in &scope.components {
            let component_identity = structural_component_identity(
                &self.authored.document,
                &scope.instance,
                &component.component,
            );
            let component_key = ReferenceKey::Component {
                instance: scope.instance.clone(),
                component: component.component.clone(),
            };
            let component_id = self.register(
                component_identity.clone(),
                ConnectivityNodeData::Component,
                Some(component_key),
            );

            for port in &component.ports {
                let port_identity = self.identity(
                    &scope.instance,
                    ObjectKind::Port,
                    [
                        ("component", component.component.as_str()),
                        ("port", port.name.as_str()),
                    ],
                );
                self.validate_capabilities(&port_identity, &port.capabilities, "port");
                let port_key = ReferenceKey::Port {
                    instance: scope.instance.clone(),
                    component: component.component.clone(),
                    port: port.name.clone(),
                };
                let port_id = self.register(
                    port_identity.clone(),
                    ConnectivityNodeData::Port {
                        capabilities: port.capabilities.clone(),
                    },
                    Some(port_key),
                );
                self.add_edge(
                    EdgeKind::Owns,
                    &component_id,
                    &port_id,
                    &component_id,
                    EdgeExactness::Exact,
                    "port",
                );

                for channel in &port.channels {
                    let channel_identity = self.identity(
                        &scope.instance,
                        ObjectKind::Channel,
                        [
                            ("component", component.component.as_str()),
                            ("port", port.name.as_str()),
                            ("channel", channel.name.as_str()),
                        ],
                    );
                    self.validate_capabilities(&channel_identity, &channel.capabilities, "channel");
                    self.validate_local_group(
                        &channel_identity,
                        channel.local_group.as_deref(),
                        "channel",
                    );
                    let channel_key = ReferenceKey::Channel {
                        instance: scope.instance.clone(),
                        component: component.component.clone(),
                        port: port.name.clone(),
                        channel: channel.name.clone(),
                    };
                    let channel_id = self.register(
                        channel_identity,
                        ConnectivityNodeData::Channel {
                            capabilities: channel.capabilities.clone(),
                            role: channel.role.clone(),
                            local_group: channel.local_group.clone(),
                        },
                        Some(channel_key),
                    );
                    self.add_edge(
                        EdgeKind::Contains,
                        &port_id,
                        &channel_id,
                        &port_id,
                        EdgeExactness::Exact,
                        "channel",
                    );
                }
            }

            let owner_key = OwnerKey::Component {
                instance: scope.instance.clone(),
                component: component.component.clone(),
            };
            for connector in &component.connectors {
                self.register_connector(
                    scope,
                    &component_id,
                    owner_key.clone(),
                    "component",
                    &component.component,
                    connector,
                );
            }

            for antenna in &component.antennas {
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::Antenna,
                    [
                        ("component", component.component.as_str()),
                        ("antenna", antenna.name.as_str()),
                    ],
                );
                let id = self.register(identity.clone(), ConnectivityNodeData::Antenna, None);
                self.add_edge(
                    EdgeKind::Owns,
                    &component_id,
                    &id,
                    &component_id,
                    EdgeExactness::Exact,
                    "antenna",
                );
                self.register_representation(&identity, &id, antenna.representation.as_ref());
            }

            for function in &component.functions {
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::ConnectivityFunction,
                    [
                        ("component", component.component.as_str()),
                        ("function", function.name.as_str()),
                    ],
                );
                let endpoint_count =
                    function.inputs.len() + function.outputs.len() + function.bidirectional.len();
                if endpoint_count < 2 {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_FUNCTION_ARITY",
                        "a connectivity function requires at least two referenced endpoints",
                        identity.clone(),
                        Vec::new(),
                    ));
                }
                let key = ReferenceKey::Function {
                    instance: scope.instance.clone(),
                    component: component.component.clone(),
                    function: function.name.clone(),
                };
                let id = self.register(
                    identity,
                    ConnectivityNodeData::ConnectivityFunction {
                        kind: function.kind,
                    },
                    Some(key),
                );
                self.add_edge(
                    EdgeKind::Owns,
                    &component_id,
                    &id,
                    &component_id,
                    EdgeExactness::Exact,
                    "function",
                );
            }
            self.register_owned_physical(
                scope,
                &component_id,
                owner_key,
                "component",
                &component.component,
                &component.paths,
                &component.junctions,
                &component.terminations,
            );
        }

        let mut structural_components = BTreeSet::new();
        for anchors in &scope.structural_anchors {
            let component_identity = structural_component_identity(
                &self.authored.document,
                &scope.instance,
                &anchors.component,
            );
            if !structural_components.insert(anchors.component.clone()) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_DUPLICATE_STRUCTURAL_CATALOG",
                    "each component may have only one structural anchor catalog entry per scope",
                    component_identity.clone(),
                    Vec::new(),
                ));
            }
            let component_key = ReferenceKey::Component {
                instance: scope.instance.clone(),
                component: anchors.component.clone(),
            };
            let Some(component_id) = self.references.get(&component_key).cloned() else {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_STRUCTURAL_OWNER",
                    "structural anchors require a matching component in the same scope",
                    component_identity,
                    Vec::new(),
                ));
                continue;
            };
            for visual in &anchors.visuals {
                let identity = structural_visual_identity(
                    &self.authored.document,
                    &scope.instance,
                    &anchors.component,
                    &visual.name,
                );
                let key = ReferenceKey::StructuralVisualRoot {
                    instance: scope.instance.clone(),
                    component: anchors.component.clone(),
                    visual: visual.name.clone(),
                };
                let id = self.register(
                    identity,
                    ConnectivityNodeData::StructuralVisualRoot {
                        model_backed: visual.model_backed,
                    },
                    Some(key),
                );
                self.add_edge(
                    EdgeKind::Owns,
                    &component_id,
                    &id,
                    &component_id,
                    EdgeExactness::Exact,
                    "structural-visual",
                );
            }
            for frame in &anchors.frames {
                let identity = structural_frame_identity(
                    &self.authored.document,
                    &scope.instance,
                    &anchors.component,
                    frame,
                );
                let key = ReferenceKey::StructuralFrame {
                    instance: scope.instance.clone(),
                    component: anchors.component.clone(),
                    frame: frame.clone(),
                };
                let id = self.register(identity, ConnectivityNodeData::StructuralFrame, Some(key));
                self.add_edge(
                    EdgeKind::Owns,
                    &component_id,
                    &id,
                    &component_id,
                    EdgeExactness::Exact,
                    "structural-frame",
                );
            }
        }
        for component in &scope.components {
            if !structural_components.contains(&component.component) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_MISSING_STRUCTURAL_CATALOG",
                    "each component requires exactly one structural anchor catalog entry in the same scope",
                    structural_component_identity(
                        &self.authored.document,
                        &scope.instance,
                        &component.component,
                    ),
                    Vec::new(),
                ));
            }
        }
        for assembly in &scope.assemblies {
            let identity = self.identity(
                &scope.instance,
                ObjectKind::PhysicalAssembly,
                [("assembly", assembly.name.as_str())],
            );
            let key = ReferenceKey::Assembly {
                instance: scope.instance.clone(),
                assembly: assembly.name.clone(),
            };
            let id = self.register(
                identity.clone(),
                ConnectivityNodeData::PhysicalAssembly {
                    kind: assembly.kind,
                },
                Some(key),
            );
            self.register_representation(&identity, &id, assembly.representation.as_ref());
            let owner_key = OwnerKey::Assembly {
                instance: scope.instance.clone(),
                assembly: assembly.name.clone(),
            };
            for connector in &assembly.connectors {
                self.register_connector(
                    scope,
                    &id,
                    owner_key.clone(),
                    "assembly",
                    &assembly.name,
                    connector,
                );
            }
            self.register_owned_physical(
                scope,
                &id,
                owner_key,
                "assembly",
                &assembly.name,
                &assembly.paths,
                &assembly.junctions,
                &assembly.terminations,
            );
        }

        for binding in &scope.bindings {
            let identity =
                self.named_identity(scope, ObjectKind::Binding, "binding", &binding.name);
            self.register(
                identity,
                ConnectivityNodeData::Binding {
                    fidelity: binding.fidelity,
                },
                None,
            );
        }
        for mate in &scope.mates {
            let identity = self.named_identity(scope, ObjectKind::Mate, "mate", &mate.name);
            self.register(
                identity,
                ConnectivityNodeData::Mate {
                    fidelity: mate.fidelity,
                },
                None,
            );
        }
        for network in &scope.networks {
            let identity =
                self.named_identity(scope, ObjectKind::Network, "network", &network.name);
            self.validate_selected(&identity, &network.selected, "network");
            let key = ReferenceKey::Network {
                instance: scope.instance.clone(),
                network: network.name.clone(),
            };
            let coordinator = match &network.structure {
                NetworkStructure::Star(topology) => Some(topology.coordinator.clone()),
                _ => None,
            };
            let root = match &network.structure {
                NetworkStructure::Tree(topology) => Some(topology.root.clone()),
                _ => None,
            };
            let network_id = self.register(
                identity,
                ConnectivityNodeData::Network {
                    topology: network.structure.topology(),
                    description: network.description.clone(),
                    participant_order: network
                        .participants
                        .iter()
                        .map(|participant| participant.name.clone())
                        .collect(),
                    hop_order: network
                        .structure
                        .hops()
                        .iter()
                        .map(|hop| hop.name.clone())
                        .collect(),
                    leg_order: network
                        .structure
                        .legs()
                        .iter()
                        .map(|leg| leg.name.clone())
                        .collect(),
                    gptp_domain_order: network
                        .configuration
                        .gptp_domains
                        .iter()
                        .map(|domain| domain.name.clone())
                        .collect(),
                    traffic_class_order: network
                        .configuration
                        .traffic_classes
                        .iter()
                        .map(|class| class.name.clone())
                        .collect(),
                    gate_schedule_order: network
                        .configuration
                        .gate_schedules
                        .iter()
                        .map(|schedule| schedule.name.clone())
                        .collect(),
                    schedule_assignment_order: network
                        .configuration
                        .schedule_assignments
                        .iter()
                        .map(|assignment| assignment.name.clone())
                        .collect(),
                    coordinator,
                    root,
                    selected: Box::new(network.selected.clone()),
                },
                Some(key),
            );

            for participant in &network.participants {
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::Participant,
                    [
                        ("network", network.name.as_str()),
                        ("participant", participant.name.as_str()),
                    ],
                );
                let key = ReferenceKey::Participant {
                    instance: scope.instance.clone(),
                    network: network.name.clone(),
                    participant: participant.name.clone(),
                };
                let participant_id = self.register(
                    identity,
                    ConnectivityNodeData::Participant {
                        role: participant.role.clone(),
                    },
                    Some(key),
                );
                self.add_edge(
                    EdgeKind::NetworkMembership,
                    &participant_id,
                    &network_id,
                    &participant_id,
                    EdgeExactness::Exact,
                    "network",
                );
            }

            for hop in network.structure.hops() {
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::Hop,
                    [
                        ("network", network.name.as_str()),
                        ("hop", hop.name.as_str()),
                    ],
                );
                let key = ReferenceKey::Hop {
                    instance: scope.instance.clone(),
                    network: network.name.clone(),
                    hop: hop.name.clone(),
                };
                let hop_id = self.register(
                    identity,
                    ConnectivityNodeData::Hop {
                        owner: hop.owner.clone(),
                        role: hop.role.clone(),
                        description: hop.description.clone(),
                        processing_delay_ns: hop.processing_delay_ns,
                    },
                    Some(key),
                );
                self.add_edge(
                    EdgeKind::TopologyMembership,
                    &hop_id,
                    &network_id,
                    &hop_id,
                    EdgeExactness::Exact,
                    "network",
                );
            }

            for leg in network.structure.legs() {
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::Leg,
                    [
                        ("network", network.name.as_str()),
                        ("leg", leg.name.as_str()),
                    ],
                );
                let key = ReferenceKey::Leg {
                    instance: scope.instance.clone(),
                    network: network.name.clone(),
                    leg: leg.name.clone(),
                };
                let leg_id = self.register(
                    identity,
                    ConnectivityNodeData::Leg {
                        from: leg.from.clone(),
                        to: leg.to.clone(),
                    },
                    Some(key),
                );
                self.add_edge(
                    EdgeKind::TopologyMembership,
                    &leg_id,
                    &network_id,
                    &leg_id,
                    EdgeExactness::Exact,
                    "network",
                );
            }
            self.register_network_configuration(scope, network, &network_id);
        }
    }

    fn register_network_configuration(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        network_id: &StableObjectId,
    ) {
        for domain in &network.configuration.gptp_domains {
            let identity = self.identity(
                &scope.instance,
                ObjectKind::GptpDomain,
                [
                    ("network", network.name.as_str()),
                    ("gptp-domain", domain.name.as_str()),
                ],
            );
            let domain_id = self.register(
                identity,
                ConnectivityNodeData::GptpDomain {
                    number: domain.number,
                    port_defaults: domain.port_defaults.clone(),
                    clock_order: domain
                        .clocks
                        .iter()
                        .map(|clock| clock.name.clone())
                        .collect(),
                },
                None,
            );
            self.add_edge(
                EdgeKind::Contains,
                network_id,
                &domain_id,
                network_id,
                EdgeExactness::Exact,
                &format!("gptp-domain:{}", domain.name),
            );
            for clock in &domain.clocks {
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::GptpClock,
                    [
                        ("network", network.name.as_str()),
                        ("gptp-domain", domain.name.as_str()),
                        ("clock", clock.name.as_str()),
                    ],
                );
                let clock_id = self.register(
                    identity,
                    ConnectivityNodeData::GptpClock {
                        kind: clock.kind,
                        participant: clock.participant.clone(),
                        gm_capable: clock.gm_capable,
                        priority1: clock.priority1,
                        priority2: clock.priority2,
                        clock_class: clock.clock_class,
                        clock_accuracy: clock.clock_accuracy,
                    },
                    None,
                );
                self.add_edge(
                    EdgeKind::Contains,
                    &domain_id,
                    &clock_id,
                    &domain_id,
                    EdgeExactness::Exact,
                    &format!("clock:{}", clock.name),
                );
            }
        }

        for class in &network.configuration.traffic_classes {
            let identity = self.identity(
                &scope.instance,
                ObjectKind::TrafficClass,
                [
                    ("network", network.name.as_str()),
                    ("traffic-class", class.name.as_str()),
                ],
            );
            let key = ReferenceKey::TrafficClass {
                instance: scope.instance.clone(),
                network: network.name.clone(),
                traffic_class: class.name.clone(),
            };
            let class_id = self.register(
                identity,
                ConnectivityNodeData::TrafficClass {
                    number: class.number,
                    description: class.description.clone(),
                    preemption: class.preemption,
                    pcp: class.pcp.clone(),
                },
                Some(key),
            );
            self.add_edge(
                EdgeKind::Contains,
                network_id,
                &class_id,
                network_id,
                EdgeExactness::Exact,
                &format!("traffic-class:{}", class.name),
            );
        }

        for schedule in &network.configuration.gate_schedules {
            let identity = self.identity(
                &scope.instance,
                ObjectKind::GateSchedule,
                [
                    ("network", network.name.as_str()),
                    ("gate-schedule", schedule.name.as_str()),
                ],
            );
            let key = ReferenceKey::GateSchedule {
                instance: scope.instance.clone(),
                network: network.name.clone(),
                schedule: schedule.name.clone(),
            };
            let entry_order = schedule
                .entries
                .iter()
                .enumerate()
                .filter_map(|(index, _)| u32::try_from(index).ok())
                .collect();
            let schedule_id = self.register(
                identity,
                ConnectivityNodeData::GateSchedule {
                    cycle_time_ns: schedule.cycle_time_ns,
                    entry_order,
                },
                Some(key),
            );
            self.add_edge(
                EdgeKind::Contains,
                network_id,
                &schedule_id,
                network_id,
                EdgeExactness::Exact,
                &format!("gate-schedule:{}", schedule.name),
            );
            for (index, entry) in schedule.entries.iter().enumerate() {
                let Ok(order) = u32::try_from(index) else {
                    let subject = self.named_identity(
                        scope,
                        ObjectKind::GateSchedule,
                        "gate-schedule",
                        &schedule.name,
                    );
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_ORDER_RANGE",
                        "gate schedule entry order exceeds u32",
                        subject,
                        Vec::new(),
                    ));
                    continue;
                };
                let order_text = order.to_string();
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::GateControlEntry,
                    [
                        ("network", network.name.as_str()),
                        ("gate-schedule", schedule.name.as_str()),
                        ("entry", order_text.as_str()),
                    ],
                );
                let entry_id = self.register(
                    identity,
                    ConnectivityNodeData::GateControlEntry {
                        order,
                        duration_ns: entry.duration_ns,
                        open_order: entry.open.iter().cloned().collect(),
                    },
                    None,
                );
                self.add_edge(
                    EdgeKind::Contains,
                    &schedule_id,
                    &entry_id,
                    &schedule_id,
                    EdgeExactness::Exact,
                    &order_text,
                );
            }
        }

        for assignment in &network.configuration.schedule_assignments {
            let identity = self.identity(
                &scope.instance,
                ObjectKind::ScheduleAssignment,
                [
                    ("network", network.name.as_str()),
                    ("schedule-assignment", assignment.name.as_str()),
                ],
            );
            let assignment_id = self.register(
                identity,
                ConnectivityNodeData::ScheduleAssignment {
                    schedule: assignment.schedule.clone(),
                    target_order: assignment.targets.clone(),
                },
                None,
            );
            self.add_edge(
                EdgeKind::Contains,
                network_id,
                &assignment_id,
                network_id,
                EdgeExactness::Exact,
                &format!("schedule-assignment:{}", assignment.name),
            );
        }

        if let Some(plca) = &network.configuration.plca {
            let identity = self.identity(
                &scope.instance,
                ObjectKind::PlcaConfiguration,
                [
                    ("network", network.name.as_str()),
                    ("configuration", "plca"),
                ],
            );
            let configuration_id = self.register(
                identity,
                ConnectivityNodeData::PlcaConfiguration {
                    max_node_id: plca.max_node_id,
                    to_timer_bit_times: plca.to_timer_bit_times,
                },
                None,
            );
            self.add_edge(
                EdgeKind::Contains,
                network_id,
                &configuration_id,
                network_id,
                EdgeExactness::Exact,
                "plca",
            );
            for node in &plca.nodes {
                let node_id = node.node_id.to_string();
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::PlcaNode,
                    [
                        ("network", network.name.as_str()),
                        ("plca-node", node_id.as_str()),
                    ],
                );
                let id = self.register(
                    identity,
                    ConnectivityNodeData::PlcaNode {
                        node_id: node.node_id,
                        burst_count: node.burst_count,
                        burst_timer_bit_times: node.burst_timer_bit_times,
                        participant: node.participant.clone(),
                    },
                    None,
                );
                self.add_edge(
                    EdgeKind::Contains,
                    &configuration_id,
                    &id,
                    &configuration_id,
                    EdgeExactness::Exact,
                    &node_id,
                );
            }
        }

        if let Some(macsec) = &network.configuration.macsec {
            let identity = self.identity(
                &scope.instance,
                ObjectKind::MacsecConfiguration,
                [
                    ("network", network.name.as_str()),
                    ("configuration", "macsec"),
                ],
            );
            let id = self.register(
                identity,
                ConnectivityNodeData::MacsecConfiguration {
                    default_policy: macsec.default_policy.clone(),
                },
                None,
            );
            self.add_edge(
                EdgeKind::Contains,
                network_id,
                &id,
                network_id,
                EdgeExactness::Exact,
                "macsec",
            );
            for policy in &macsec.policies {
                let identity = self.identity(
                    &scope.instance,
                    ObjectKind::MacsecPolicy,
                    [
                        ("network", network.name.as_str()),
                        ("macsec-policy", policy.name.as_str()),
                    ],
                );
                let key = ReferenceKey::MacsecPolicy {
                    instance: scope.instance.clone(),
                    network: network.name.clone(),
                    policy: policy.name.clone(),
                };
                let policy_id = self.register(
                    identity,
                    ConnectivityNodeData::MacsecPolicy {
                        enforcement: policy.enforcement,
                        cipher: policy.cipher.clone(),
                        key_agreement: policy.key_agreement.clone(),
                        confidentiality_offset: policy.confidentiality_offset,
                        rekey_interval_ns: policy.rekey_interval_ns,
                        credential_store_ref: policy.credential_store_ref.clone(),
                    },
                    Some(key),
                );
                self.add_edge(
                    EdgeKind::Contains,
                    &id,
                    &policy_id,
                    &id,
                    EdgeExactness::Exact,
                    &format!("policy:{}", policy.name),
                );
            }
        }

        if let Some(eee) = &network.configuration.eee {
            let identity = self.identity(
                &scope.instance,
                ObjectKind::EeeConfiguration,
                [("network", network.name.as_str()), ("configuration", "eee")],
            );
            let id = self.register(
                identity,
                ConnectivityNodeData::EeeConfiguration {
                    default_mode: eee.default_mode,
                },
                None,
            );
            self.add_edge(
                EdgeKind::Contains,
                network_id,
                &id,
                network_id,
                EdgeExactness::Exact,
                "eee",
            );
        }
    }

    fn register_connector(
        &mut self,
        scope: &ConnectivityScope,
        owner_id: &StableObjectId,
        owner_key: OwnerKey,
        owner_field: &str,
        owner_name: &str,
        connector: &Connector,
    ) {
        let identity = self.identity(
            &scope.instance,
            ObjectKind::Connector,
            [
                (owner_field, owner_name),
                ("connector", connector.name.as_str()),
            ],
        );
        let key = ReferenceKey::Connector {
            owner: owner_key.clone(),
            connector: connector.name.clone(),
        };
        let id = self.register(
            identity.clone(),
            ConnectivityNodeData::Connector {
                family: connector.family.clone(),
            },
            Some(key),
        );
        self.add_edge(
            EdgeKind::Owns,
            owner_id,
            &id,
            owner_id,
            EdgeExactness::Exact,
            "connector",
        );
        self.register_representation(&identity, &id, connector.representation.as_ref());

        for position in &connector.positions {
            let position_identity = self.identity(
                &scope.instance,
                ObjectKind::Position,
                [
                    (owner_field, owner_name),
                    ("connector", connector.name.as_str()),
                    ("position", position.name.as_str()),
                ],
            );
            self.validate_local_group(
                &position_identity,
                position.local_group.as_deref(),
                "position",
            );
            let position_key = ReferenceKey::Position {
                owner: owner_key.clone(),
                connector: connector.name.clone(),
                position: position.name.clone(),
            };
            let position_id = self.register(
                position_identity.clone(),
                ConnectivityNodeData::Position {
                    kind: position.kind,
                    role: position.role.clone(),
                    local_group: position.local_group.clone(),
                },
                Some(position_key),
            );
            self.add_edge(
                EdgeKind::Contains,
                &id,
                &position_id,
                &id,
                EdgeExactness::Exact,
                "position",
            );
            self.register_representation(
                &position_identity,
                &position_id,
                position.representation.as_ref(),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn register_owned_physical(
        &mut self,
        scope: &ConnectivityScope,
        owner_id: &StableObjectId,
        owner_key: OwnerKey,
        owner_field: &str,
        owner_name: &str,
        paths: &[PhysicalPath],
        junctions: &[Junction],
        terminations: &[Termination],
    ) {
        for path in paths {
            let identity = self.owned_identity(
                scope,
                ObjectKind::PhysicalPath,
                owner_field,
                owner_name,
                "path",
                &path.name,
            );
            self.validate_local_group(&identity, path.local_group.as_deref(), "physical path");
            let id = self.register(
                identity.clone(),
                ConnectivityNodeData::PhysicalPath {
                    kind: path.kind,
                    fidelity: path.fidelity,
                    role: path.role.clone(),
                    local_group: path.local_group.clone(),
                },
                None,
            );
            self.add_edge(
                EdgeKind::Owns,
                owner_id,
                &id,
                owner_id,
                EdgeExactness::Exact,
                "path",
            );
            self.register_representation(&identity, &id, path.representation.as_ref());
        }
        for junction in junctions {
            let identity = self.owned_identity(
                scope,
                ObjectKind::Junction,
                owner_field,
                owner_name,
                "junction",
                &junction.name,
            );
            let key = ReferenceKey::Junction {
                owner: owner_key.clone(),
                junction: junction.name.clone(),
            };
            let id = self.register(
                identity.clone(),
                ConnectivityNodeData::Junction {
                    kind: junction.kind,
                    fidelity: junction.fidelity,
                },
                Some(key),
            );
            self.add_edge(
                EdgeKind::Owns,
                owner_id,
                &id,
                owner_id,
                EdgeExactness::Exact,
                "junction",
            );
            self.register_representation(&identity, &id, junction.representation.as_ref());
        }
        for termination in terminations {
            let identity = self.owned_identity(
                scope,
                ObjectKind::Termination,
                owner_field,
                owner_name,
                "termination",
                &termination.name,
            );
            self.validate_termination(&identity, scope, termination);
            let id = self.register(
                identity.clone(),
                ConnectivityNodeData::Termination {
                    kind: termination.kind.clone(),
                    mounting: termination.mounting,
                    quantities: termination.quantities.clone(),
                    profile: termination.profile.clone(),
                    fidelity: termination.fidelity,
                },
                None,
            );
            self.add_edge(
                EdgeKind::Owns,
                owner_id,
                &id,
                owner_id,
                EdgeExactness::Exact,
                "termination",
            );
            self.register_representation(&identity, &id, termination.representation.as_ref());
        }
    }

    fn validate_capabilities(
        &mut self,
        subject: &ObjectIdentity,
        capabilities: &crate::model::connectivity::Capabilities,
        owner: &str,
    ) {
        let limits = &capabilities.limits;
        for (axis, range) in [
            ("rate", limits.rate.as_ref()),
            ("voltage", limits.voltage.as_ref()),
            ("current", limits.current.as_ref()),
            ("power", limits.power.as_ref()),
            ("impedance", limits.impedance.as_ref()),
            ("frequency", limits.frequency.as_ref()),
            ("bandwidth", limits.bandwidth.as_ref()),
            ("pressure", limits.pressure.as_ref()),
            ("flow", limits.flow.as_ref()),
            ("temperature", limits.temperature.as_ref()),
        ] {
            if let Some(range) = range {
                self.validate_quantity_range(
                    subject,
                    range,
                    axis_dimensions(axis),
                    &format!("{owner} {axis}"),
                );
            }
        }
    }

    fn validate_local_group(
        &mut self,
        subject: &ObjectIdentity,
        local_group: Option<&str>,
        owner: &str,
    ) {
        if local_group.is_some_and(|value| value.trim().is_empty()) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_LOCAL_GROUP",
                format!("{owner} local-group must not be empty when present"),
                subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_selected(
        &mut self,
        subject: &ObjectIdentity,
        selected: &NetworkSelection,
        owner: &str,
    ) {
        for (axis, quantity) in [
            ("rate", selected.rate.as_ref()),
            ("voltage", selected.voltage.as_ref()),
            ("current", selected.current.as_ref()),
            ("power", selected.power.as_ref()),
            ("impedance", selected.impedance.as_ref()),
            ("frequency", selected.frequency.as_ref()),
            ("pressure", selected.pressure.as_ref()),
            ("flow", selected.flow.as_ref()),
            ("temperature", selected.temperature.as_ref()),
        ] {
            if let Some(quantity) = quantity {
                self.validate_selection_quantity(
                    subject,
                    quantity,
                    axis_dimensions(axis),
                    &format!("{owner} selected {axis}"),
                );
            }
        }
        if let Some(rf) = &selected.rf {
            if !matches!(
                selected.carrier,
                authored::Carrier::ConductedRf | authored::Carrier::RadiatedRf
            ) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_RF_CARRIER",
                    format!("{owner} selects an RF channel on a non-RF carrier"),
                    subject.clone(),
                    Vec::new(),
                ));
            }
            let center_frequency = rf.channel.center_frequency();
            if selected.frequency.is_some() && center_frequency.is_some() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_RF_CENTER_CONFLICT",
                    format!(
                        "{owner} selects both generic frequency and RF channel center frequency"
                    ),
                    subject.clone(),
                    Vec::new(),
                ));
            }
            if rf.channel.number().is_some()
                && center_frequency.is_none()
                && selected.frequency.is_none()
                && selected.profiles.is_empty()
            {
                self.issues.push(ConnectivityIssue::warning(
                    "W_CONN_SELECTION_UNVERIFIED",
                    format!(
                        "{owner} selects an RF channel number without a center frequency or profile that defines its mapping"
                    ),
                    subject.clone(),
                    Vec::new(),
                ));
            }
            if let Some(center_frequency) = center_frequency {
                self.validate_selection_quantity(
                    subject,
                    center_frequency,
                    &[Dimension::Frequency],
                    &format!("{owner} selected RF channel center frequency"),
                );
            }
            if let Some(bandwidth) = rf.channel.bandwidth() {
                self.validate_selection_quantity(
                    subject,
                    bandwidth,
                    &[Dimension::Frequency],
                    &format!("{owner} selected RF channel bandwidth"),
                );
            }
            if let Some((minimum, maximum)) = normalized_rf_frequency_envelope(selected) {
                if !minimum.value.is_finite() || !maximum.value.is_finite() {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_QUANTITY_VALUE",
                        format!("{owner} selected RF channel has a nonfinite occupied spectrum"),
                        subject.clone(),
                        Vec::new(),
                    ));
                } else {
                    let zero = NormalizedQuantity {
                        dimension: Dimension::Frequency,
                        value: 0.0,
                    };
                    if definitely_less_normalized(minimum, zero) {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_PHYSICAL_RANGE",
                            format!("{owner} selected RF channel occupies frequencies below zero"),
                            subject.clone(),
                            Vec::new(),
                        ));
                    }
                }
            }
        }
    }

    fn queue_selected_profiles(
        &mut self,
        scope: &'a ConnectivityScope,
        network: &'a Network,
        subject: &ObjectIdentity,
        network_id: &StableObjectId,
    ) {
        for profile in &network.selected.profiles {
            self.pending_profile_validations
                .push(PendingProfileValidation {
                    scope,
                    network,
                    subject: subject.clone(),
                    network_id: network_id.clone(),
                    profile,
                });
        }
    }

    fn validate_selected_profiles(&mut self) {
        debug_assert!(self.graph_connections_complete);
        let mut pending = std::mem::take(&mut self.pending_profile_validations);
        pending.sort_by(|first, second| {
            first
                .subject
                .stable_id()
                .cmp(&second.subject.stable_id())
                .then_with(|| first.profile.cmp(second.profile))
        });
        for pending in pending {
            match self.profile_registry.lookup(pending.profile).cloned() {
                Some(ruleset) => self.dispatch_profile_validator(
                    &ruleset,
                    &ProfileValidationContext {
                        scope: pending.scope,
                        network: pending.network,
                        subject: &pending.subject,
                        network_id: &pending.network_id,
                        profile: pending.profile,
                    },
                ),
                None => {
                    let message = format!(
                        "selected profile {} has no ruleset in the trusted built-in registry; only generic connectivity validation was applied",
                        pending.profile
                    );
                    let issue = match self.options.profile_registry_completeness {
                        ProfileRegistryCompleteness::Advisory => ConnectivityIssue::warning(
                            "W_CONN_PROFILE_INCOMPLETE",
                            message,
                            pending.subject.clone(),
                            Vec::new(),
                        ),
                        ProfileRegistryCompleteness::Strict => ConnectivityIssue::error(
                            "E_CONN_PROFILE_INCOMPLETE",
                            message,
                            pending.subject.clone(),
                            Vec::new(),
                        ),
                    };
                    self.issues.push(issue);
                }
            }
        }
    }

    fn dispatch_profile_validator(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        match ruleset.validator() {
            // Generic structural, dimensional, range, selection, and topology checks run
            // unconditionally in their ordinary validation passes. Registry dispatch never gates
            // those checks, and this validator deliberately adds no profile-specific rules.
            ValidatorKind::BidirectionalDshot => {
                self.validate_bidirectional_dshot_profile(ruleset, context)
            }
            ValidatorKind::Can => self.validate_can_profile(ruleset, context),
            ValidatorKind::Dshot => self.validate_dshot_profile(ruleset, context),
            ValidatorKind::FeetechSts => self.validate_feetech_sts_profile(ruleset, context),
            ValidatorKind::Generic => self.validate_generic_profile(ruleset, context),
            ValidatorKind::Pwm => self.validate_pwm_profile(ruleset, context),
            ValidatorKind::Rs485 => self.validate_rs485_profile(ruleset, context),
            ValidatorKind::Uart => self.validate_uart_profile(ruleset, context),
            #[cfg(test)]
            ValidatorKind::CrossNetworkProbe => self.validate_cross_network_probe(context),
        }
    }

    fn validate_generic_profile(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        debug_assert!(self.graph_connections_complete);
        debug_assert_eq!(context.subject.kind(), ObjectKind::Network);
        debug_assert_eq!(context.subject.instance(), &context.scope.instance);
        debug_assert_eq!(&context.subject.stable_id(), context.network_id);
        debug_assert_eq!(ruleset.profile(), context.profile);
        debug_assert!(context.network.selected.profiles.contains(context.profile));
        debug_assert!(matches!(
            self.nodes
                .get(context.network_id)
                .map(ConnectivityNode::data),
            Some(ConnectivityNodeData::Network { .. })
        ));
    }

    fn validate_bidirectional_dshot_profile(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        debug_assert_eq!(ruleset.profile(), context.profile);
        debug_assert_eq!(context.profile.as_str(), "hcdf:bidirectional-dshot");
        debug_assert!(ruleset.citation("same-line-telemetry").is_some());

        if !context
            .network
            .selected
            .profiles
            .iter()
            .any(|profile| profile.as_str() == "hcdf:dshot")
        {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_BIDIRECTIONAL_DSHOT_BASE_PROFILE",
                "the bidirectional DShot profile requires explicit co-selection of the hcdf:dshot base profile",
                context.subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_dshot_profile(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        debug_assert_eq!(ruleset.profile(), context.profile);
        debug_assert_eq!(context.profile.as_str(), "hcdf:dshot");
        debug_assert!(ruleset.citation("digital-motor-link").is_some());
        debug_assert!(ruleset.citation("frame-and-direction").is_some());

        if context.network.selected.purpose != authored::Purpose::Communication {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_DSHOT_PURPOSE",
                "the DShot profile requires communication purpose",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if context.network.selected.carrier != authored::Carrier::Electrical {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_DSHOT_CARRIER",
                "the DShot profile requires electrical carrier",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if context.network.structure.topology() != Topology::Link {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_DSHOT_TOPOLOGY",
                "the DShot profile requires link logical topology",
                context.subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_can_profile(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        debug_assert_eq!(ruleset.profile(), context.profile);
        debug_assert_eq!(context.profile.as_str(), "hcdf:can");
        debug_assert!(ruleset.citation("can-lower-layers").is_some());
        debug_assert!(ruleset.citation("high-speed-pma").is_some());
        debug_assert!(ruleset.citation("network-topology").is_some());

        if context.network.selected.purpose != authored::Purpose::Communication {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_CAN_PURPOSE",
                "the CAN profile requires communication purpose",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if context.network.selected.carrier != authored::Carrier::Electrical {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_CAN_CARRIER",
                "the CAN profile requires electrical carrier",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if context.network.structure.topology() != Topology::Bus {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_CAN_TOPOLOGY",
                "the CAN profile requires bus logical topology",
                context.subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_feetech_sts_profile(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        debug_assert_eq!(ruleset.profile(), context.profile);
        debug_assert_eq!(context.profile.as_str(), "feetech:sts");
        debug_assert!(ruleset.citation("serial-bus-protocol").is_some());
        debug_assert!(ruleset.citation("sts3215-evaluation").is_some());
        debug_assert!(ruleset.citation("sts3215-product").is_some());

        if !context
            .network
            .selected
            .profiles
            .iter()
            .any(|profile| profile.as_str() == "hcdf:uart")
        {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_FEETECH_STS_BASE_PROFILE",
                "the Feetech STS profile requires explicit co-selection of the hcdf:uart base profile",
                context.subject.clone(),
                Vec::new(),
            ));
            return;
        }

        if context.network.structure.topology() == Topology::Link {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_FEETECH_STS_TOPOLOGY",
                "the Feetech STS profile requires bus logical topology",
                context.subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_rs485_profile(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        debug_assert_eq!(ruleset.profile(), context.profile);
        debug_assert_eq!(context.profile.as_str(), "hcdf:rs-485");
        debug_assert!(ruleset.citation("electrical-and-bus").is_some());
        debug_assert!(ruleset.citation("system-configurations").is_some());

        if context.network.selected.purpose != authored::Purpose::Communication {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_RS485_PURPOSE",
                "the RS-485 profile requires communication purpose",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if context.network.selected.carrier != authored::Carrier::Electrical {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_RS485_CARRIER",
                "the RS-485 profile requires electrical carrier",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if !matches!(
            context.network.structure.topology(),
            Topology::Link | Topology::Bus
        ) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_RS485_TOPOLOGY",
                "the RS-485 profile permits only link or bus logical topology",
                context.subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_pwm_profile(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        debug_assert_eq!(ruleset.profile(), context.profile);
        debug_assert_eq!(context.profile.as_str(), "hcdf:pwm");
        debug_assert!(ruleset.citation("point-to-point-control").is_some());
        debug_assert!(ruleset.citation("signal-definition").is_some());

        if context.network.selected.purpose != authored::Purpose::Communication {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_PWM_PURPOSE",
                "the PWM control profile requires communication purpose",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if context.network.selected.carrier != authored::Carrier::Electrical {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_PWM_CARRIER",
                "the PWM control profile requires electrical carrier",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if context.network.structure.topology() != Topology::Link {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_PWM_TOPOLOGY",
                "the PWM control profile requires link logical topology",
                context.subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_uart_profile(
        &mut self,
        ruleset: &ProfileRuleset,
        context: &ProfileValidationContext<'_>,
    ) {
        debug_assert_eq!(ruleset.profile(), context.profile);
        debug_assert_eq!(context.profile.as_str(), "hcdf:uart");
        debug_assert!(ruleset.citation("link-interface").is_some());
        debug_assert!(ruleset.citation("multiprocessor-bus").is_some());

        if context.network.selected.purpose != authored::Purpose::Communication {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_UART_PURPOSE",
                "the UART profile requires communication purpose",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if context.network.selected.carrier != authored::Carrier::Electrical {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_UART_CARRIER",
                "the UART profile requires electrical carrier",
                context.subject.clone(),
                Vec::new(),
            ));
        }

        if !matches!(
            context.network.structure.topology(),
            Topology::Link | Topology::Bus
        ) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_UART_TOPOLOGY",
                "the UART profile permits only link or bus logical topology",
                context.subject.clone(),
                Vec::new(),
            ));
        }
    }

    #[cfg(test)]
    fn validate_cross_network_probe(&mut self, context: &ProfileValidationContext<'_>) {
        let expected = context
            .scope
            .networks
            .iter()
            .map(|network| network.participants.len())
            .sum::<usize>();
        let actual = self
            .edges
            .values()
            .filter(|edge| edge.kind() == EdgeKind::ParticipantEndpoint)
            .filter(|edge| {
                self.nodes
                    .get(edge.to())
                    .is_some_and(|node| node.identity().instance() == &context.scope.instance)
            })
            .count();

        if actual == expected {
            self.issues.push(ConnectivityIssue::warning(
                "W_CONN_TEST_CROSS_NETWORK_PROBE",
                format!("profile validator observed all {actual} participant-endpoint edges"),
                context.subject.clone(),
                Vec::new(),
            ));
        } else {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TEST_CROSS_NETWORK_PROBE",
                format!(
                    "profile validator observed {actual} of {expected} participant-endpoint edges"
                ),
                context.subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_quantity(
        &mut self,
        subject: &ObjectIdentity,
        quantity: &crate::model::connectivity::Quantity,
        dimensions: &[Dimension],
        label: &str,
    ) -> Option<NormalizedQuantity> {
        let normalized = match normalize_quantity(quantity) {
            Ok(normalized) => normalized,
            Err(error) => {
                self.issues.push(ConnectivityIssue::error(
                    error.code(),
                    format!("{label}: {}", error.message()),
                    subject.clone(),
                    Vec::new(),
                ));
                return None;
            }
        };
        if !dimensions.is_empty() && !dimensions.contains(&normalized.dimension) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_DIMENSION",
                format!(
                    "{label} uses {}, which is not valid for this quantity axis",
                    normalized.dimension.name()
                ),
                subject.clone(),
                Vec::new(),
            ));
            return None;
        }
        Some(normalized)
    }

    fn validate_quantity_range(
        &mut self,
        subject: &ObjectIdentity,
        range: &crate::model::connectivity::QuantityRange,
        dimensions: &[Dimension],
        label: &str,
    ) {
        if range.minimum.is_none() && range.nominal.is_none() && range.maximum.is_none() {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_RANGE_EMPTY",
                format!("{label} range must declare a minimum, nominal, or maximum value"),
                subject.clone(),
                Vec::new(),
            ));
            return;
        }
        let minimum = range.minimum.as_ref().and_then(|value| {
            self.validate_quantity(subject, value, dimensions, &format!("{label} minimum"))
        });
        let nominal = range.nominal.as_ref().and_then(|value| {
            self.validate_quantity(subject, value, dimensions, &format!("{label} nominal"))
        });
        let maximum = range.maximum.as_ref().and_then(|value| {
            self.validate_quantity(subject, value, dimensions, &format!("{label} maximum"))
        });
        let normalized = [minimum, nominal, maximum]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if normalized
            .iter()
            .map(|value| value.dimension)
            .collect::<BTreeSet<_>>()
            .len()
            > 1
        {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_DIMENSION",
                format!("{label} range values must use compatible dimensions"),
                subject.clone(),
                Vec::new(),
            ));
            return;
        }
        if minimum
            .zip(maximum)
            .is_some_and(|(minimum, maximum)| definitely_greater_normalized(minimum, maximum))
        {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_RANGE_ORDER",
                format!("{label} range minimum must not exceed its maximum"),
                subject.clone(),
                Vec::new(),
            ));
        }
        if nominal.is_some_and(|nominal| {
            minimum.is_some_and(|minimum| definitely_less_normalized(nominal, minimum))
                || maximum.is_some_and(|maximum| definitely_greater_normalized(nominal, maximum))
        }) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_RANGE_NOMINAL",
                format!("{label} nominal must lie within its declared bounds"),
                subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn validate_selection_quantity(
        &mut self,
        subject: &ObjectIdentity,
        selection: &authored::SelectionQuantity,
        dimensions: &[Dimension],
        label: &str,
    ) {
        match selection {
            authored::SelectionQuantity::Nominal(quantity) => {
                self.validate_quantity(subject, quantity, dimensions, label);
            }
            authored::SelectionQuantity::Range(range) => {
                let minimum = self.validate_quantity(
                    subject,
                    &range.minimum,
                    dimensions,
                    &format!("{label} minimum"),
                );
                let maximum = self.validate_quantity(
                    subject,
                    &range.maximum,
                    dimensions,
                    &format!("{label} maximum"),
                );
                let nominal = range.nominal.as_ref().and_then(|quantity| {
                    self.validate_quantity(
                        subject,
                        quantity,
                        dimensions,
                        &format!("{label} nominal"),
                    )
                });
                let normalized = [minimum, maximum, nominal]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                if normalized
                    .iter()
                    .map(|value| value.dimension)
                    .collect::<BTreeSet<_>>()
                    .len()
                    > 1
                {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_DIMENSION",
                        format!("{label} range values must use compatible dimensions"),
                        subject.clone(),
                        Vec::new(),
                    ));
                    return;
                }
                if minimum.zip(maximum).is_some_and(|(minimum, maximum)| {
                    definitely_greater_normalized(minimum, maximum)
                }) || nominal.is_some_and(|nominal| {
                    minimum.is_some_and(|minimum| definitely_less_normalized(nominal, minimum))
                        || maximum
                            .is_some_and(|maximum| definitely_greater_normalized(nominal, maximum))
                }) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_SELECTION_RANGE",
                        format!("{label} range must be ordered and contain its nominal value"),
                        subject.clone(),
                        Vec::new(),
                    ));
                }
            }
        }
    }

    fn validate_termination(
        &mut self,
        subject: &ObjectIdentity,
        scope: &ConnectivityScope,
        termination: &Termination,
    ) {
        let attachment_count = termination.attachments.len();
        let minimum_attachment_count =
            if termination.mounting == authored::TerminationMounting::Inline {
                2
            } else {
                1
            };
        if attachment_count < minimum_attachment_count {
            let message = if minimum_attachment_count == 2 {
                "an inline termination requires at least two physical attachment sites"
            } else {
                "a termination requires at least one physical attachment"
            };
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TERMINATION_ATTACHMENT_COUNT",
                message,
                subject.clone(),
                Vec::new(),
            ));
        }
        let mut attachments = BTreeSet::new();
        for attachment in &termination.attachments {
            if !attachments.insert(physical_key(attachment, &scope.instance)) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_TERMINATION_DUPLICATE_ATTACHMENT",
                    "a termination may not repeat the same physical attachment",
                    subject.clone(),
                    Vec::new(),
                ));
            }
        }
        let mut properties = BTreeSet::new();
        for value in &termination.quantities {
            if !properties.insert(&value.property) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_TERMINATION_DUPLICATE_PROPERTY",
                    format!("termination repeats property {}", value.property),
                    subject.clone(),
                    Vec::new(),
                ));
            }
            let normalized = self.validate_quantity(
                subject,
                &value.quantity,
                termination_property_dimensions(&value.property),
                &format!("termination property {}", value.property),
            );
            if termination_property_requires_nonnegative(&value.property)
                && normalized.is_some_and(|quantity| quantity.value < 0.0)
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_PHYSICAL_RANGE",
                    format!(
                        "termination property {} requires a nonnegative value",
                        value.property
                    ),
                    subject.clone(),
                    Vec::new(),
                ));
            }
        }
    }

    fn register_representation(
        &mut self,
        owner: &ObjectIdentity,
        owner_id: &StableObjectId,
        representation: Option<&Representation>,
    ) {
        let Some(representation) = representation else {
            return;
        };
        let mut local = owner.local().to_vec();
        local.push(IdentityPart::new("representation", "primary"));
        let identity = ObjectIdentity::new(
            owner.document().clone(),
            owner.instance().clone(),
            ObjectKind::Representation,
            local,
        );
        self.validate_representation(&identity, representation);
        let id = self.register(
            identity.clone(),
            ConnectivityNodeData::Representation {
                representation: representation.clone(),
            },
            None,
        );
        self.representations.push(PendingRepresentation {
            owner: owner.clone(),
            subject: identity,
            id: id.clone(),
            representation: representation.clone(),
        });
        self.add_edge(
            EdgeKind::Representation,
            owner_id,
            &id,
            owner_id,
            EdgeExactness::Exact,
            "primary",
        );
    }

    fn validate_representation(&mut self, subject: &ObjectIdentity, value: &Representation) {
        let placement_is_finite = |placement: &crate::model::connectivity::Placement| {
            placement.xyz.iter().all(|value| value.is_finite())
                && match &placement.rotation {
                    crate::model::connectivity::PlacementRotation::Rpy(values) => {
                        values.iter().all(|value| value.is_finite())
                    }
                    crate::model::connectivity::PlacementRotation::Quaternion(values) => {
                        values.iter().all(|value| value.is_finite())
                    }
                }
        };
        let geometry_is_valid = match value {
            Representation::Primitive { primitive, .. } => match primitive {
                crate::model::connectivity::PrimitiveRepresentation::Box { size } => {
                    size.iter().all(|value| value.is_finite() && *value > 0.0)
                }
                crate::model::connectivity::PrimitiveRepresentation::Cylinder {
                    radius,
                    length,
                } => radius.is_finite() && *radius > 0.0 && length.is_finite() && *length > 0.0,
                crate::model::connectivity::PrimitiveRepresentation::Sphere { radius } => {
                    radius.is_finite() && *radius > 0.0
                }
            },
            _ => true,
        };
        let quaternion_is_valid = match value {
            Representation::Primitive { placement, .. }
            | Representation::Model { placement, .. } => match &placement.rotation {
                crate::model::connectivity::PlacementRotation::Quaternion(values) => {
                    let norm_squared = values.iter().map(|value| value * value).sum::<f64>();
                    norm_squared.is_finite() && (norm_squared - 1.0).abs() <= 1.0e-6
                }
                crate::model::connectivity::PlacementRotation::Rpy(_) => true,
            },
            _ => true,
        };
        if !geometry_is_valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_REPRESENTATION_GEOMETRY",
                "primitive representation dimensions must be finite and greater than zero",
                subject.clone(),
                Vec::new(),
            ));
        }
        let route_section_is_valid = match value {
            Representation::DerivedRoute(route) => match &route.section {
                None => true,
                Some(crate::model::connectivity::RouteSection::Round { diameter }) => {
                    diameter.is_finite() && *diameter > 0.0
                }
                Some(crate::model::connectivity::RouteSection::Rectangular { width, height }) => {
                    width.is_finite() && *width > 0.0 && height.is_finite() && *height > 0.0
                }
            },
            _ => true,
        };
        if !route_section_is_valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_ROUTE_SECTION",
                "route section dimensions must be finite and greater than zero",
                subject.clone(),
                Vec::new(),
            ));
        }
        let route_orientation_shape_is_valid = match value {
            Representation::DerivedRoute(route) => match &route.section {
                Some(crate::model::connectivity::RouteSection::Rectangular { .. }) => route
                    .waypoints
                    .iter()
                    .all(|waypoint| waypoint.rotation.is_some()),
                None | Some(crate::model::connectivity::RouteSection::Round { .. }) => route
                    .waypoints
                    .iter()
                    .all(|waypoint| waypoint.rotation.is_none()),
            },
            _ => true,
        };
        if !route_orientation_shape_is_valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_ROUTE_ORIENTATION",
                "rectangular routes require a rotation at every waypoint; round and sectionless routes must omit waypoint rotations",
                subject.clone(),
                Vec::new(),
            ));
        }
        let route_rotations_are_valid = match value {
            Representation::DerivedRoute(route) => route
                .waypoints
                .iter()
                .filter_map(|waypoint| waypoint.rotation.as_ref())
                .all(route_rotation_is_valid),
            _ => true,
        };
        if !route_rotations_are_valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_REPRESENTATION_ROTATION",
                "waypoint rotations must be finite; quaternions must be unit length within 1e-6",
                subject.clone(),
                Vec::new(),
            ));
        }
        let route_tangents_are_valid = match value {
            Representation::DerivedRoute(route)
                if route_orientation_shape_is_valid
                    && route_rotations_are_valid
                    && matches!(
                        &route.section,
                        Some(crate::model::connectivity::RouteSection::Rectangular { .. })
                    ) =>
            {
                rectangular_route_tangents_are_valid(route)
            }
            _ => true,
        };
        if !route_tangents_are_valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_ROUTE_TANGENT",
                "rectangular waypoint local +Z must follow the route tangent within one degree",
                subject.clone(),
                Vec::new(),
            ));
        }
        let lexical_values_are_valid = match value {
            Representation::Model { model, .. } => {
                !model.uri.trim().is_empty()
                    && model
                        .sha
                        .as_deref()
                        .is_none_or(|value| !value.trim().is_empty())
                    && model
                        .node_path
                        .as_deref()
                        .is_none_or(|value| !value.trim().is_empty())
            }
            Representation::ModelPart(model_part) => {
                !model_part.node_path.trim().is_empty()
                    && model_part
                        .submesh_fallback
                        .as_deref()
                        .is_none_or(|value| !value.trim().is_empty())
            }
            _ => true,
        };
        if !lexical_values_are_valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_REPRESENTATION_LEXICAL",
                "model representation URI, SHA, node path, and submesh fallback values must not be empty when present",
                subject.clone(),
                Vec::new(),
            ));
        }
        if !quaternion_is_valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_REPRESENTATION_ROTATION",
                "placement quaternions must be nonzero and unit length within 1e-6",
                subject.clone(),
                Vec::new(),
            ));
        }
        let coordinates_are_finite = match value {
            Representation::Primitive { placement, .. }
            | Representation::Model { placement, .. } => placement_is_finite(placement),
            Representation::DerivedRoute(route) => route
                .waypoints
                .iter()
                .all(|waypoint| waypoint.xyz.iter().all(|value| value.is_finite())),
            Representation::ModelPart(_) => true,
        };
        if !coordinates_are_finite {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_REPRESENTATION_COORDINATE",
                "placement and route coordinates must be finite",
                subject.clone(),
                Vec::new(),
            ));
        }
        if matches!(value, Representation::DerivedRoute(route) if route.waypoints.len() < 2) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_ROUTE_ARITY",
                "a derived route requires at least two framed waypoints",
                subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn finalize_connections(&mut self) {
        self.connect_representations();
        self.graph_connections_complete = true;
        self.validate_selected_profiles();
    }

    fn connect_representations(&mut self) {
        let assembly_model_roots = self
            .representations
            .iter()
            .filter(|pending| {
                pending.owner.kind() == ObjectKind::PhysicalAssembly
                    && matches!(&pending.representation, Representation::Model { .. })
            })
            .map(|pending| (pending.owner.clone(), pending.id.clone()))
            .collect::<BTreeMap<_, _>>();
        for pending in self.representations.clone() {
            match &pending.representation {
                Representation::Primitive { placement, .. }
                | Representation::Model { placement, .. } => {
                    self.connect_representation_frame(
                        &placement.frame,
                        &pending.subject,
                        &pending.id,
                        EdgeKind::RepresentationFrame,
                        "placement",
                    );
                }
                Representation::ModelPart(model_part) => match &model_part.model_root {
                    crate::model::connectivity::ModelRootRef::ComponentVisual {
                        component,
                        visual,
                    } => {
                        let reference = StructuralVisualRef {
                            component: component.clone(),
                            visual: visual.clone(),
                        };
                        let key = structural_visual_key(&reference, pending.subject.instance());
                        if let Some(root) = self.resolve(&key, &pending.subject) {
                            let root_identity =
                                self.nodes.get(&root).map(|node| node.identity().clone());
                            let model_backed = matches!(
                                self.nodes.get(&root).map(|node| node.data()),
                                Some(ConnectivityNodeData::StructuralVisualRoot {
                                    model_backed: true
                                })
                            );
                            if !model_backed {
                                self.issues.push(ConnectivityIssue::error(
                                    "E_CONN_MODEL_ROOT_TYPE",
                                    "a component-visual selector requires a model-backed structural visual",
                                    pending.subject.clone(),
                                    root_identity.into_iter().collect(),
                                ));
                            } else {
                                self.add_edge(
                                    EdgeKind::RepresentationModelRoot,
                                    &root,
                                    &pending.id,
                                    &pending.id,
                                    EdgeExactness::Exact,
                                    "component-visual",
                                );
                            }
                        }
                    }
                    crate::model::connectivity::ModelRootRef::AssemblyModel { assembly } => {
                        let key = ReferenceKey::Assembly {
                            instance: assembly.scope.resolve(pending.subject.instance()),
                            assembly: assembly.assembly.clone(),
                        };
                        if let Some(assembly_id) = self.resolve(&key, &pending.subject) {
                            let assembly_identity = self
                                .nodes
                                .get(&assembly_id)
                                .map(|node| node.identity().clone())
                                .expect("resolved assembly must have a graph node");
                            if let Some(root) = assembly_model_roots.get(&assembly_identity) {
                                self.add_edge(
                                    EdgeKind::RepresentationModelRoot,
                                    root,
                                    &pending.id,
                                    &pending.id,
                                    EdgeExactness::Exact,
                                    "assembly-model",
                                );
                            } else {
                                self.issues.push(ConnectivityIssue::error(
                                    "E_CONN_MODEL_ROOT_TYPE",
                                    "an assembly-model selector requires an assembly with a standalone model representation",
                                    pending.subject.clone(),
                                    vec![assembly_identity],
                                ));
                            }
                        }
                    }
                },
                Representation::DerivedRoute(route) => {
                    for (index, waypoint) in route.waypoints.iter().enumerate() {
                        self.connect_representation_frame(
                            &waypoint.frame,
                            &pending.subject,
                            &pending.id,
                            EdgeKind::RouteWaypointFrame,
                            &index.to_string(),
                        );
                    }
                }
            }
        }
    }

    fn connect_representation_frame(
        &mut self,
        frame: &crate::model::connectivity::RouteFrameRef,
        subject: &ObjectIdentity,
        representation_id: &StableObjectId,
        kind: EdgeKind,
        discriminator: &str,
    ) {
        let key = match frame {
            crate::model::connectivity::RouteFrameRef::World => return,
            crate::model::connectivity::RouteFrameRef::ComponentOrigin { component } => {
                ReferenceKey::Component {
                    instance: component.scope.resolve(subject.instance()),
                    component: component.component.clone(),
                }
            }
            crate::model::connectivity::RouteFrameRef::ComponentFrame { component, frame } => {
                structural_frame_key(
                    &StructuralFrameRef {
                        component: component.clone(),
                        frame: frame.clone(),
                    },
                    subject.instance(),
                )
            }
        };
        if let Some(frame_owner) = self.resolve(&key, subject) {
            self.add_edge(
                kind,
                &frame_owner,
                representation_id,
                representation_id,
                EdgeExactness::Exact,
                discriminator,
            );
        }
    }
    fn connect_scope(&mut self, scope: &ConnectivityScope) {
        let mut coarse_bindings = BTreeMap::<(ReferenceKey, ReferenceKey), ObjectIdentity>::new();
        let mut exact_bindings = BTreeMap::<(ReferenceKey, ReferenceKey), ObjectIdentity>::new();
        let mut exact_position_channels =
            BTreeMap::<ReferenceKey, (ReferenceKey, ObjectIdentity)>::new();

        for binding in &scope.bindings {
            let subject = self.named_identity(scope, ObjectKind::Binding, "binding", &binding.name);
            let Some(subject_id) = self.id_for_identity(&subject) else {
                continue;
            };
            if !binding_belongs_to_same_component(binding, scope) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_BINDING_OWNER",
                    "a binding must relate a port or channel to physical connectivity on the same component",
                    subject.clone(),
                    Vec::new(),
                ));
                continue;
            }
            let functional_key = functional_key(&binding.functional, &scope.instance);
            let physical_key = physical_key(&binding.physical, &scope.instance);
            let functional = self.resolve(&functional_key, &subject);
            let physical = self.resolve(&physical_key, &subject);
            let exactness = self.binding_exactness(binding, &subject);
            if let (Some(functional), Some(physical), Some(exactness)) =
                (functional, physical, exactness)
            {
                self.add_edge(
                    EdgeKind::Binding,
                    &functional,
                    &physical,
                    &subject_id,
                    exactness,
                    "binding",
                );
                if exactness == EdgeExactness::Exact {
                    if let (FunctionalEndpointRef::Channel(_), PhysicalEndpointRef::Position(_)) =
                        (&binding.functional, &binding.physical)
                    {
                        if let Some((first_channel, first_subject)) =
                            exact_position_channels.get(&physical_key)
                        {
                            if first_channel != &functional_key {
                                self.issues.push(ConnectivityIssue::error(
                                    "E_CONN_POSITION_CHANNEL_CONFLICT",
                                    "one exact physical position cannot be bound to two distinct channels",
                                    subject.clone(),
                                    vec![first_subject.clone()],
                                ));
                            }
                        } else {
                            exact_position_channels.insert(
                                physical_key.clone(),
                                (functional_key.clone(), subject.clone()),
                            );
                        }
                    }
                }
                let port = port_key(&binding.functional.port(), &scope.instance);
                let connector = match &binding.physical {
                    PhysicalEndpointRef::Connector(reference) => {
                        connector_key(reference, &scope.instance)
                    }
                    PhysicalEndpointRef::Position(reference) => {
                        connector_key(&reference.connector_ref(), &scope.instance)
                    }
                    PhysicalEndpointRef::Junction(_) => continue,
                };
                match exactness {
                    EdgeExactness::Coarse => {
                        coarse_bindings.insert((port, connector), subject.clone());
                    }
                    EdgeExactness::Exact => {
                        exact_bindings.insert((port, connector), subject.clone());
                    }
                }
            }
        }

        for (key, coarse_subject) in coarse_bindings {
            if let Some(exact_subject) = exact_bindings.get(&key) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_DUAL_FIDELITY",
                    "whole-port and exact channel-position bindings compete for the same port and connector",
                    coarse_subject,
                    vec![exact_subject.clone()],
                ));
            }
        }

        for mate in &scope.mates {
            self.connect_mate(scope, mate);
        }
        for component in &scope.components {
            for antenna in &component.antennas {
                let subject = self.identity(
                    &scope.instance,
                    ObjectKind::Antenna,
                    [
                        ("component", component.component.as_str()),
                        ("antenna", antenna.name.as_str()),
                    ],
                );
                let Some(antenna_id) = self.id_for_identity(&subject) else {
                    continue;
                };
                let refs_belong = antenna.conducted_port.as_ref().is_none_or(|reference| {
                    port_belongs_to_component(reference, scope, &component.component)
                }) && port_belongs_to_component(
                    &antenna.radiated_port,
                    scope,
                    &component.component,
                );
                if !refs_belong {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_OWNER_SCOPE",
                        "an antenna may reference only ports on its containing component",
                        subject.clone(),
                        Vec::new(),
                    ));
                    continue;
                }
                if let Some(conducted) = &antenna.conducted_port {
                    if let Some(port) =
                        self.resolve(&port_key(conducted, &scope.instance), &subject)
                    {
                        self.add_edge(
                            EdgeKind::AntennaFeed,
                            &port,
                            &antenna_id,
                            &antenna_id,
                            EdgeExactness::Exact,
                            "conducted",
                        );
                    }
                }
                if let Some(port) =
                    self.resolve(&port_key(&antenna.radiated_port, &scope.instance), &subject)
                {
                    self.add_edge(
                        EdgeKind::AntennaRadiation,
                        &antenna_id,
                        &port,
                        &antenna_id,
                        EdgeExactness::Exact,
                        "radiated",
                    );
                }
            }
            self.connect_owned_physical(
                scope,
                "component",
                &component.component,
                &component.paths,
                &component.junctions,
                &component.terminations,
            );
            for function in &component.functions {
                let subject = self.identity(
                    &scope.instance,
                    ObjectKind::ConnectivityFunction,
                    [
                        ("component", component.component.as_str()),
                        ("function", function.name.as_str()),
                    ],
                );
                let Some(subject_id) = self.id_for_identity(&subject) else {
                    continue;
                };
                self.connect_function_endpoints(
                    scope,
                    &subject,
                    &subject_id,
                    &component.component,
                    EdgeKind::FunctionInput,
                    &function.inputs,
                );
                self.connect_function_endpoints(
                    scope,
                    &subject,
                    &subject_id,
                    &component.component,
                    EdgeKind::FunctionOutput,
                    &function.outputs,
                );
                self.connect_function_endpoints(
                    scope,
                    &subject,
                    &subject_id,
                    &component.component,
                    EdgeKind::FunctionBidirectional,
                    &function.bidirectional,
                );
            }
        }

        for assembly in &scope.assemblies {
            self.connect_owned_physical(
                scope,
                "assembly",
                &assembly.name,
                &assembly.paths,
                &assembly.junctions,
                &assembly.terminations,
            );
        }
    }

    fn connect_networks(&mut self, scope: &'a ConnectivityScope) {
        for network in &scope.networks {
            self.connect_network(scope, network);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn connect_owned_physical(
        &mut self,
        scope: &ConnectivityScope,
        owner_field: &str,
        owner_name: &str,
        paths: &[PhysicalPath],
        junctions: &[Junction],
        terminations: &[Termination],
    ) {
        let expected_owner = if owner_field == "component" {
            OwnerKey::Component {
                instance: scope.instance.clone(),
                component: owner_name.to_owned(),
            }
        } else {
            OwnerKey::Assembly {
                instance: scope.instance.clone(),
                assembly: owner_name.to_owned(),
            }
        };
        for path in paths {
            let subject = self.owned_identity(
                scope,
                ObjectKind::PhysicalPath,
                owner_field,
                owner_name,
                "path",
                &path.name,
            );
            let Some(subject_id) = self.id_for_identity(&subject) else {
                continue;
            };
            if !physical_belongs_to_owner(&path.first, scope, &expected_owner)
                || !physical_belongs_to_owner(&path.second, scope, &expected_owner)
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_OWNER_SCOPE",
                    "an owner-scoped physical path may reference only connectors, positions, or junctions owned by its containing component or assembly",
                    subject.clone(),
                    Vec::new(),
                ));
                continue;
            }
            let exactness = self.path_exactness(path.fidelity, &path.first, &path.second, &subject);
            let first = self.resolve(&physical_key(&path.first, &scope.instance), &subject);
            let second = self.resolve(&physical_key(&path.second, &scope.instance), &subject);
            if let (Some(first), Some(second), Some(exactness)) = (first, second, exactness) {
                self.add_edge(
                    EdgeKind::PhysicalPathEndpoint,
                    &first,
                    &subject_id,
                    &subject_id,
                    exactness,
                    "first",
                );
                self.add_edge(
                    EdgeKind::PhysicalPathEndpoint,
                    &subject_id,
                    &second,
                    &subject_id,
                    exactness,
                    "second",
                );
            }
        }

        for junction in junctions {
            let subject = self.owned_identity(
                scope,
                ObjectKind::Junction,
                owner_field,
                owner_name,
                "junction",
                &junction.name,
            );
            let Some(subject_id) = self.id_for_identity(&subject) else {
                continue;
            };
            if junction.attachments.len() < 2 {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_JUNCTION_ARITY",
                    "a passive junction requires at least two attachments",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            for (index, attachment) in junction.attachments.iter().enumerate() {
                if !physical_belongs_to_owner(attachment, scope, &expected_owner) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_OWNER_SCOPE",
                        "a junction attachment may reference only physical objects owned by its containing component or assembly",
                        subject.clone(),
                        Vec::new(),
                    ));
                    continue;
                }
                let Some(exactness) = physical_exactness(
                    junction.fidelity,
                    attachment,
                    &subject,
                    "junction attachments",
                    &mut self.issues,
                ) else {
                    continue;
                };
                if let Some(target) =
                    self.resolve(&physical_key(attachment, &scope.instance), &subject)
                {
                    self.add_edge(
                        EdgeKind::JunctionAttachment,
                        &target,
                        &subject_id,
                        &subject_id,
                        exactness,
                        &index.to_string(),
                    );
                }
            }
        }

        for termination in terminations {
            let subject = self.owned_identity(
                scope,
                ObjectKind::Termination,
                owner_field,
                owner_name,
                "termination",
                &termination.name,
            );
            let Some(subject_id) = self.id_for_identity(&subject) else {
                continue;
            };
            for (index, attachment) in termination.attachments.iter().enumerate() {
                if !physical_belongs_to_owner(attachment, scope, &expected_owner) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_OWNER_SCOPE",
                        "a termination attachment may reference only a physical object owned by its containing component or assembly",
                        subject.clone(),
                        Vec::new(),
                    ));
                    continue;
                }
                let Some(exactness) = physical_exactness(
                    termination.fidelity,
                    attachment,
                    &subject,
                    "termination attachments",
                    &mut self.issues,
                ) else {
                    continue;
                };
                if let Some(target) =
                    self.resolve(&physical_key(attachment, &scope.instance), &subject)
                {
                    self.add_edge(
                        EdgeKind::TerminationAttachment,
                        &subject_id,
                        &target,
                        &subject_id,
                        exactness,
                        &index.to_string(),
                    );
                }
            }
        }
    }

    fn connect_mate(&mut self, scope: &ConnectivityScope, mate: &Mate) {
        let subject = self.named_identity(scope, ObjectKind::Mate, "mate", &mate.name);
        let Some(subject_id) = self.id_for_identity(&subject) else {
            return;
        };
        let exactness = match (mate.fidelity.is_exact(), mate.mappings.is_empty()) {
            (false, true) if mate.fidelity == Fidelity::Presented => Some(EdgeExactness::Coarse),
            (true, false) => Some(EdgeExactness::Exact),
            _ => {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_MATE_FIDELITY",
                    "presented mates have no position mappings; exact or quantified mates require mappings",
                    subject.clone(),
                    Vec::new(),
                ));
                None
            }
        };
        let first_key = connector_key(&mate.first, &scope.instance);
        let second_key = connector_key(&mate.second, &scope.instance);
        let first = self.resolve(&first_key, &subject);
        let second = self.resolve(&second_key, &subject);
        if let (Some(first), Some(second), Some(exactness)) = (first, second, exactness) {
            self.add_edge(
                EdgeKind::Mate,
                &first,
                &second,
                &subject_id,
                exactness,
                "shells",
            );
        }

        for (index, mapping) in mate.mappings.iter().enumerate() {
            let mapping_first_connector =
                connector_key(&mapping.first.connector_ref(), &scope.instance);
            let mapping_second_connector =
                connector_key(&mapping.second.connector_ref(), &scope.instance);
            if mapping_first_connector != first_key || mapping_second_connector != second_key {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_MATE_MAPPING",
                    "position mapping endpoints must belong to the mate's first and second connector respectively",
                    subject.clone(),
                    Vec::new(),
                ));
                continue;
            }
            let first_position =
                self.resolve(&position_key(&mapping.first, &scope.instance), &subject);
            let second_position =
                self.resolve(&position_key(&mapping.second, &scope.instance), &subject);
            if let (Some(first_position), Some(second_position)) = (first_position, second_position)
            {
                self.add_edge(
                    EdgeKind::MappedMate,
                    &first_position,
                    &second_position,
                    &subject_id,
                    EdgeExactness::Exact,
                    &index.to_string(),
                );
            }
        }
    }

    fn connect_function_endpoints(
        &mut self,
        scope: &ConnectivityScope,
        subject: &ObjectIdentity,
        function_id: &StableObjectId,
        component: &str,
        kind: EdgeKind,
        endpoints: &[FunctionalEndpointRef],
    ) {
        for (index, endpoint) in endpoints.iter().enumerate() {
            if !functional_belongs_to_component(endpoint, scope, component) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_OWNER_SCOPE",
                    "a connectivity function may reference only ports or channels on its containing component",
                    subject.clone(),
                    Vec::new(),
                ));
                continue;
            }
            if let Some(endpoint_id) =
                self.resolve(&functional_key(endpoint, &scope.instance), subject)
            {
                let (from, to) = if kind == EdgeKind::FunctionInput {
                    (&endpoint_id, function_id)
                } else {
                    (function_id, &endpoint_id)
                };
                self.add_edge(
                    kind,
                    from,
                    to,
                    function_id,
                    EdgeExactness::Exact,
                    &index.to_string(),
                );
            }
        }
    }

    fn connect_network(&mut self, scope: &'a ConnectivityScope, network: &'a Network) {
        let network_identity =
            self.named_identity(scope, ObjectKind::Network, "network", &network.name);
        let Some(network_id) = self.id_for_identity(&network_identity) else {
            return;
        };
        let participant_count = network.participants.len();
        let valid_arity = match network.structure.topology() {
            Topology::Link => participant_count == 2,
            Topology::Ring => participant_count >= 3,
            Topology::Bus | Topology::Chain | Topology::Star | Topology::Mesh | Topology::Tree => {
                participant_count >= 2
            }
        };
        if !valid_arity {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_ARITY",
                format!(
                    "topology {:?} has invalid participant count {participant_count}",
                    network.structure.topology()
                ),
                network_identity.clone(),
                Vec::new(),
            ));
        }

        let mut endpoints = BTreeMap::<
            ReferenceKey,
            (Option<ObjectIdentity>, BTreeMap<String, ObjectIdentity>),
        >::new();
        for participant in &network.participants {
            let subject = self.identity(
                &scope.instance,
                ObjectKind::Participant,
                [
                    ("network", network.name.as_str()),
                    ("participant", participant.name.as_str()),
                ],
            );
            let port = participant.endpoint.port();
            let base_key = port_key(&port, &scope.instance);
            let entry = endpoints.entry(base_key.clone()).or_default();

            match &participant.endpoint {
                FunctionalEndpointRef::Port(_) => {
                    if let Some(existing) = &entry.0 {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_DUPLICATE_PARTICIPANT_ENDPOINT",
                            "a network cannot repeat the same whole-port participant endpoint",
                            subject.clone(),
                            vec![existing.clone()],
                        ));
                    } else if let Some(existing) = entry.1.values().next() {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_PARTICIPANT_ENDPOINT_OVERLAP",
                            "a whole-port participant cannot coexist with a channel participant on the same port",
                            subject.clone(),
                            vec![existing.clone()],
                        ));
                    }
                    entry.0.get_or_insert_with(|| subject.clone());
                }
                FunctionalEndpointRef::Channel(channel) => {
                    if let Some(existing) = &entry.0 {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_PARTICIPANT_ENDPOINT_OVERLAP",
                            "a channel participant cannot coexist with a whole-port participant on the same port",
                            subject.clone(),
                            vec![existing.clone()],
                        ));
                    }
                    if let Some(existing) = entry.1.insert(channel.channel.clone(), subject.clone())
                    {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_DUPLICATE_PARTICIPANT_ENDPOINT",
                            "a network cannot repeat the same channel participant endpoint",
                            subject.clone(),
                            vec![existing],
                        ));
                    }
                }
            }

            let Some(participant_id) = self.id_for_identity(&subject) else {
                continue;
            };
            let endpoint_key = functional_key(&participant.endpoint, &scope.instance);
            if let Some(endpoint) = self.resolve(&endpoint_key, &subject) {
                self.add_edge(
                    EdgeKind::ParticipantEndpoint,
                    &endpoint,
                    &participant_id,
                    &participant_id,
                    EdgeExactness::Exact,
                    "endpoint",
                );
                if let Some(port_id) = self.resolve(&base_key, &subject) {
                    self.check_selection(&subject, &endpoint, &port_id, &network.selected);
                }
            }
        }

        self.connect_network_configuration(scope, network);

        match &network.structure {
            NetworkStructure::Link | NetworkStructure::Bus | NetworkStructure::Mesh => {}
            NetworkStructure::Star(topology) => {
                self.connect_star(scope, network, &network_identity, &network_id, topology)
            }
            NetworkStructure::Chain(topology) => {
                self.connect_path_topology(scope, network, &network_identity, topology, false)
            }
            NetworkStructure::Ring(topology) => {
                self.connect_path_topology(scope, network, &network_identity, topology, true)
            }
            NetworkStructure::Tree(topology) => {
                self.connect_tree(scope, network, &network_identity, &network_id, topology)
            }
        }

        self.queue_selected_profiles(scope, network, &network_identity, &network_id);
    }

    fn connect_network_configuration(&mut self, scope: &ConnectivityScope, network: &Network) {
        let mut domain_names = BTreeMap::<String, ObjectIdentity>::new();
        let mut domain_numbers = BTreeMap::<u8, ObjectIdentity>::new();
        for domain in &network.configuration.gptp_domains {
            let subject = self.identity(
                &scope.instance,
                ObjectKind::GptpDomain,
                [
                    ("network", network.name.as_str()),
                    ("gptp-domain", domain.name.as_str()),
                ],
            );
            if domain.name.trim().is_empty() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_GPTP_DOMAIN_NAME",
                    "gPTP domain name must not be empty",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            if let Some(existing) = domain_names.insert(domain.name.clone(), subject.clone()) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_GPTP_DUPLICATE_DOMAIN_NAME",
                    format!(
                        "gPTP domain name {:?} is declared more than once",
                        domain.name
                    ),
                    subject.clone(),
                    vec![existing],
                ));
            }
            if let Some(existing) = domain_numbers.insert(domain.number, subject.clone()) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_GPTP_DUPLICATE_DOMAIN",
                    format!("gPTP domain {} is declared more than once", domain.number),
                    subject.clone(),
                    vec![existing],
                ));
            }
            if domain.number > 127 {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_GPTP_DOMAIN_RANGE",
                    "gPTP domain number must be in 0..=127",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            if domain.clocks.is_empty() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_GPTP_CLOCKS",
                    "gPTP domain requires at least one declared clock",
                    subject.clone(),
                    Vec::new(),
                ));
            }

            let mut clock_names = BTreeMap::<String, ObjectIdentity>::new();
            let mut clock_participants = BTreeMap::<ReferenceKey, ObjectIdentity>::new();
            for clock in &domain.clocks {
                let clock_subject = self.identity(
                    &scope.instance,
                    ObjectKind::GptpClock,
                    [
                        ("network", network.name.as_str()),
                        ("gptp-domain", domain.name.as_str()),
                        ("clock", clock.name.as_str()),
                    ],
                );
                if clock.name.trim().is_empty() {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_GPTP_CLOCK_NAME",
                        "gPTP clock name must not be empty",
                        clock_subject.clone(),
                        Vec::new(),
                    ));
                }
                if let Some(existing) =
                    clock_names.insert(clock.name.clone(), clock_subject.clone())
                {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_GPTP_DUPLICATE_CLOCK_NAME",
                        format!(
                            "gPTP clock name {:?} is declared more than once in domain {:?}",
                            clock.name, domain.name
                        ),
                        clock_subject.clone(),
                        vec![existing],
                    ));
                }
                let Some(clock_id) = self.id_for_identity(&clock_subject) else {
                    continue;
                };
                if !network_local_reference_is_nonempty(
                    &clock.participant.network,
                    &clock.participant.participant,
                ) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_EMPTY_REFERENCE",
                        "gPTP clock requires nonempty network and participant names",
                        clock_subject,
                        Vec::new(),
                    ));
                    continue;
                }
                if !reference_targets_network(&clock.participant.network, scope, network) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_CONFIGURATION_SCOPE",
                        "gPTP clock must reference a participant on its containing network",
                        clock_subject,
                        Vec::new(),
                    ));
                    continue;
                }
                let participant_key = participant_key(&clock.participant, &scope.instance);
                if let Some(existing) =
                    clock_participants.insert(participant_key.clone(), clock_subject.clone())
                {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_GPTP_DUPLICATE_CLOCK",
                        "a participant may be declared as only one clock in a gPTP domain",
                        clock_subject.clone(),
                        vec![existing],
                    ));
                }
                if let Some(participant_id) = self.resolve(&participant_key, &clock_subject) {
                    self.add_edge(
                        EdgeKind::GptpClockParticipant,
                        &participant_id,
                        &clock_id,
                        &clock_id,
                        EdgeExactness::Exact,
                        "participant",
                    );
                }
            }
        }

        let mut claimed_class_numbers = BTreeMap::<u8, ObjectIdentity>::new();
        let mut claimed_pcp = BTreeMap::<u8, ObjectIdentity>::new();
        for class in &network.configuration.traffic_classes {
            let subject = self.identity(
                &scope.instance,
                ObjectKind::TrafficClass,
                [
                    ("network", network.name.as_str()),
                    ("traffic-class", class.name.as_str()),
                ],
            );
            if class.name.trim().is_empty() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_TRAFFIC_CLASS_NAME",
                    "traffic class name must not be empty",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            if class.number > 7 {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_TRAFFIC_CLASS_RANGE",
                    format!("traffic class number {} is outside 0..=7", class.number),
                    subject.clone(),
                    Vec::new(),
                ));
            }
            if let Some(existing) = claimed_class_numbers.insert(class.number, subject.clone()) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_TRAFFIC_CLASS_NUMBER",
                    format!(
                        "traffic class number {} is assigned to more than one class",
                        class.number
                    ),
                    subject.clone(),
                    vec![existing],
                ));
            }
            if class.pcp.is_empty() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_TRAFFIC_CLASS_PCP",
                    "traffic class requires at least one PCP value",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            for pcp in &class.pcp {
                if *pcp > 7 {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_PCP_RANGE",
                        format!("PCP {pcp} is outside 0..=7"),
                        subject.clone(),
                        Vec::new(),
                    ));
                }
                if let Some(existing) = claimed_pcp.insert(*pcp, subject.clone()) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_PCP_AMBIGUOUS",
                        format!("PCP {pcp} is assigned to more than one traffic class"),
                        subject.clone(),
                        vec![existing],
                    ));
                }
            }
        }

        for schedule in &network.configuration.gate_schedules {
            let subject = self.identity(
                &scope.instance,
                ObjectKind::GateSchedule,
                [
                    ("network", network.name.as_str()),
                    ("gate-schedule", schedule.name.as_str()),
                ],
            );
            if schedule.cycle_time_ns == 0 {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_GATE_CYCLE",
                    "gate schedule cycle must be positive",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            if schedule.entries.is_empty() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_GATE_ENTRIES",
                    "gate schedule requires at least one entry",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            let mut duration_sum = 0u128;
            for (index, entry) in schedule.entries.iter().enumerate() {
                let Ok(order) = u32::try_from(index) else {
                    continue;
                };
                let order_text = order.to_string();
                let entry_subject = self.identity(
                    &scope.instance,
                    ObjectKind::GateControlEntry,
                    [
                        ("network", network.name.as_str()),
                        ("gate-schedule", schedule.name.as_str()),
                        ("entry", order_text.as_str()),
                    ],
                );
                let Some(entry_id) = self.id_for_identity(&entry_subject) else {
                    continue;
                };
                if entry.duration_ns == 0 {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_GATE_DURATION",
                        "gate control entry duration must be positive",
                        entry_subject.clone(),
                        Vec::new(),
                    ));
                }
                duration_sum += u128::from(entry.duration_ns);
                for (reference_order, reference) in entry.open.iter().enumerate() {
                    if !network_local_reference_is_nonempty(
                        &reference.network,
                        &reference.traffic_class,
                    ) {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_EMPTY_REFERENCE",
                            "gate entry traffic-class references require nonempty network and class names",
                            entry_subject.clone(),
                            Vec::new(),
                        ));
                        continue;
                    }
                    if !reference_targets_network(&reference.network, scope, network) {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_CONFIGURATION_SCOPE",
                            "gate entry may open only traffic classes on its containing network",
                            entry_subject.clone(),
                            Vec::new(),
                        ));
                        continue;
                    }
                    if let Some(class_id) = self.resolve(
                        &traffic_class_key(reference, &scope.instance),
                        &entry_subject,
                    ) {
                        self.add_edge(
                            EdgeKind::GateEntryTrafficClass,
                            &entry_id,
                            &class_id,
                            &entry_id,
                            EdgeExactness::Exact,
                            &reference_order.to_string(),
                        );
                    }
                }
            }
            if duration_sum != u128::from(schedule.cycle_time_ns) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_GATE_SUM",
                    format!(
                        "gate entry durations sum to {duration_sum} ns, expected cycle {} ns",
                        schedule.cycle_time_ns
                    ),
                    subject,
                    Vec::new(),
                ));
            }
        }

        let mut assignment_names = BTreeMap::<String, ObjectIdentity>::new();
        let mut assigned_participants = BTreeMap::<ReferenceKey, ObjectIdentity>::new();
        for assignment in &network.configuration.schedule_assignments {
            let subject = self.identity(
                &scope.instance,
                ObjectKind::ScheduleAssignment,
                [
                    ("network", network.name.as_str()),
                    ("schedule-assignment", assignment.name.as_str()),
                ],
            );
            if assignment.name.trim().is_empty() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_SCHEDULE_ASSIGNMENT_NAME",
                    "schedule assignment name must not be empty",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            if let Some(existing) =
                assignment_names.insert(assignment.name.clone(), subject.clone())
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_DUPLICATE_SCHEDULE_ASSIGNMENT_NAME",
                    format!(
                        "schedule assignment name {:?} is declared more than once",
                        assignment.name
                    ),
                    subject.clone(),
                    vec![existing],
                ));
            }
            let Some(assignment_id) = self.id_for_identity(&subject) else {
                continue;
            };
            if assignment.targets.is_empty() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_SCHEDULE_ASSIGNMENT_TARGETS",
                    "schedule assignment requires at least one participant target",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            let schedule_is_nonempty = network_local_reference_is_nonempty(
                &assignment.schedule.network,
                &assignment.schedule.schedule,
            );
            if !schedule_is_nonempty {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_EMPTY_REFERENCE",
                    "schedule assignment requires a nonempty schedule target",
                    subject.clone(),
                    Vec::new(),
                ));
            }
            let schedule_is_local = schedule_is_nonempty
                && reference_targets_network(&assignment.schedule.network, scope, network);
            if schedule_is_nonempty && !schedule_is_local {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_CONFIGURATION_SCOPE",
                    "schedule assignment schedule must belong to the containing network",
                    subject.clone(),
                    Vec::new(),
                ));
            }

            for (target_order, target) in assignment.targets.iter().enumerate() {
                let target_is_nonempty =
                    network_local_reference_is_nonempty(&target.network, &target.participant);
                if !target_is_nonempty {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_EMPTY_REFERENCE",
                        "schedule assignment participant targets require nonempty network and participant names",
                        subject.clone(),
                        Vec::new(),
                    ));
                    continue;
                }
                let target_is_local = reference_targets_network(&target.network, scope, network);
                if !target_is_local {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_CONFIGURATION_SCOPE",
                        "schedule assignment participants must belong to the containing network",
                        subject.clone(),
                        Vec::new(),
                    ));
                    continue;
                }
                let participant_key = participant_key(target, &scope.instance);
                if let Some(existing) =
                    assigned_participants.insert(participant_key.clone(), subject.clone())
                {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_DUPLICATE_SCHEDULE_ASSIGNMENT",
                        "a participant may have only one gate schedule assignment",
                        subject.clone(),
                        vec![existing],
                    ));
                }
                if let Some(participant_id) = self.resolve(&participant_key, &subject) {
                    self.add_edge(
                        EdgeKind::ScheduleAssignmentParticipant,
                        &participant_id,
                        &assignment_id,
                        &assignment_id,
                        EdgeExactness::Exact,
                        &target_order.to_string(),
                    );
                }
            }
            if schedule_is_local {
                if let Some(schedule_id) = self.resolve(
                    &schedule_key(&assignment.schedule, &scope.instance),
                    &subject,
                ) {
                    self.add_edge(
                        EdgeKind::ScheduleAssignmentSchedule,
                        &assignment_id,
                        &schedule_id,
                        &assignment_id,
                        EdgeExactness::Exact,
                        "schedule",
                    );
                }
            }
        }

        self.connect_plca_configuration(scope, network);
        self.connect_macsec_configuration(scope, network);
        self.connect_eee_configuration(scope, network);
    }

    fn connect_plca_configuration(&mut self, scope: &ConnectivityScope, network: &Network) {
        let Some(plca) = &network.configuration.plca else {
            return;
        };
        let subject = self.identity(
            &scope.instance,
            ObjectKind::PlcaConfiguration,
            [
                ("network", network.name.as_str()),
                ("configuration", "plca"),
            ],
        );
        let configuration_id = self.id_for_identity(&subject);
        if !matches!(network.structure, NetworkStructure::Bus) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_PLCA_TOPOLOGY",
                "PLCA configuration is valid only on a bus topology",
                subject.clone(),
                Vec::new(),
            ));
        }
        if plca.max_node_id == u8::MAX {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_PLCA_MAX_NODE_ID",
                "PLCA max-node-id 255 is reserved and must not be used",
                subject.clone(),
                Vec::new(),
            ));
        }

        let mut claimed_ids = BTreeMap::<u8, ObjectIdentity>::new();
        let mut claimed_participants = BTreeMap::<ReferenceKey, ObjectIdentity>::new();
        let mut configured_participants = BTreeSet::<String>::new();
        let mut coordinator_count = 0usize;
        for node in &plca.nodes {
            let node_id_text = node.node_id.to_string();
            let node_subject = self.identity(
                &scope.instance,
                ObjectKind::PlcaNode,
                [
                    ("network", network.name.as_str()),
                    ("plca-node", node_id_text.as_str()),
                ],
            );
            if node.node_id == u8::MAX {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_PLCA_NODE_ID",
                    "PLCA node ID 255 is reserved and must not be used",
                    node_subject.clone(),
                    Vec::new(),
                ));
            }
            if node.node_id > plca.max_node_id {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_PLCA_NODE_RANGE",
                    format!(
                        "PLCA node ID {} exceeds max-node-id {}",
                        node.node_id, plca.max_node_id
                    ),
                    node_subject.clone(),
                    Vec::new(),
                ));
            }
            if node.node_id == 0 {
                coordinator_count += 1;
            }
            if let Some(existing) = claimed_ids.insert(node.node_id, node_subject.clone()) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_PLCA_DUPLICATE_NODE_ID",
                    format!("PLCA node ID {} is declared more than once", node.node_id),
                    node_subject.clone(),
                    vec![existing],
                ));
            }
            if !network_local_reference_is_nonempty(
                &node.participant.network,
                &node.participant.participant,
            ) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_EMPTY_REFERENCE",
                    "PLCA node requires nonempty network and participant names",
                    node_subject,
                    Vec::new(),
                ));
                continue;
            }
            if !reference_targets_network(&node.participant.network, scope, network) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_CONFIGURATION_SCOPE",
                    "PLCA nodes must reference participants on the containing network",
                    node_subject,
                    Vec::new(),
                ));
                continue;
            }
            configured_participants.insert(node.participant.participant.clone());
            let participant_key = participant_key(&node.participant, &scope.instance);
            if let Some(existing) =
                claimed_participants.insert(participant_key.clone(), node_subject.clone())
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_PLCA_DUPLICATE_PARTICIPANT",
                    "a participant may have only one PLCA node assignment",
                    node_subject.clone(),
                    vec![existing],
                ));
            }
            let Some(node_id) = self.id_for_identity(&node_subject) else {
                continue;
            };
            if let Some(participant_id) = self.resolve(&participant_key, &node_subject) {
                self.add_edge(
                    EdgeKind::PlcaNodeParticipant,
                    &participant_id,
                    &node_id,
                    &node_id,
                    EdgeExactness::Exact,
                    "participant",
                );
                if node.node_id == 0 {
                    if let Some(configuration_id) = &configuration_id {
                        self.add_edge(
                            EdgeKind::PlcaCoordinator,
                            &participant_id,
                            configuration_id,
                            &node_id,
                            EdgeExactness::Exact,
                            "node-0",
                        );
                    }
                }
            }
        }
        if coordinator_count != 1 {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_PLCA_COORDINATOR",
                "PLCA configuration requires exactly one node with ID 0",
                subject.clone(),
                Vec::new(),
            ));
        }
        let declared_participants = network
            .participants
            .iter()
            .map(|participant| participant.name.clone())
            .collect::<BTreeSet<_>>();
        if configured_participants != declared_participants {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_PLCA_PARTICIPANT_COVERAGE",
                "PLCA nodes must assign every participant on the bus exactly once",
                subject,
                Vec::new(),
            ));
        }
    }

    fn connect_macsec_configuration(&mut self, scope: &ConnectivityScope, network: &Network) {
        let Some(macsec) = &network.configuration.macsec else {
            return;
        };
        let subject = self.identity(
            &scope.instance,
            ObjectKind::MacsecConfiguration,
            [
                ("network", network.name.as_str()),
                ("configuration", "macsec"),
            ],
        );
        let configuration_id = self.id_for_identity(&subject);
        if macsec.policies.is_empty() {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_MACSEC_POLICIES",
                "MACsec configuration requires at least one named policy",
                subject.clone(),
                Vec::new(),
            ));
        }

        let mut policy_names = BTreeMap::<String, ObjectIdentity>::new();
        for policy in &macsec.policies {
            let policy_subject = self.identity(
                &scope.instance,
                ObjectKind::MacsecPolicy,
                [
                    ("network", network.name.as_str()),
                    ("macsec-policy", policy.name.as_str()),
                ],
            );
            if policy.name.trim().is_empty() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_MACSEC_POLICY_NAME",
                    "MACsec policy name must not be empty",
                    policy_subject.clone(),
                    Vec::new(),
                ));
            }
            if let Some(existing) = policy_names.insert(policy.name.clone(), policy_subject.clone())
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_MACSEC_DUPLICATE_POLICY",
                    format!("MACsec policy {:?} is declared more than once", policy.name),
                    policy_subject.clone(),
                    vec![existing],
                ));
            }

            let has_security_parameters = policy.cipher.is_some()
                || policy.key_agreement.is_some()
                || policy.confidentiality_offset.is_some()
                || policy.rekey_interval_ns.is_some()
                || policy.credential_store_ref.is_some();
            if policy.enforcement == authored::MacsecEnforcement::Disabled {
                if has_security_parameters {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_MACSEC_DISABLED_CONTRADICTION",
                        "disabled MACsec policy must not declare cipher, key agreement, confidentiality offset, rekey interval, or credential store",
                        policy_subject.clone(),
                        Vec::new(),
                    ));
                }
            } else {
                if policy.cipher.is_none()
                    || policy.key_agreement.is_none()
                    || policy.rekey_interval_ns.is_none()
                    || policy.credential_store_ref.is_none()
                {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_MACSEC_REQUIRED_PARAMETER",
                        "enabled MACsec policy requires cipher, key agreement, rekey interval, and credential store reference",
                        policy_subject.clone(),
                        Vec::new(),
                    ));
                }
                if policy.enforcement == authored::MacsecEnforcement::IntegrityOnly
                    && policy.confidentiality_offset.is_some()
                {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_MACSEC_INTEGRITY_OFFSET",
                        "integrity-only MACsec policy must not declare a confidentiality offset",
                        policy_subject.clone(),
                        Vec::new(),
                    ));
                }
                if policy.cipher.as_ref().is_some_and(macsec_cipher_is_xpn)
                    && policy.confidentiality_offset.is_some()
                {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_MACSEC_XPN_OFFSET",
                        "recognized XPN MACsec ciphers must not declare a confidentiality offset",
                        policy_subject.clone(),
                        Vec::new(),
                    ));
                }
            }
            if policy
                .confidentiality_offset
                .is_some_and(|offset| !matches!(offset, 0 | 30 | 50))
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_MACSEC_CONFIDENTIALITY_OFFSET",
                    "MACsec confidentiality offset must be 0, 30, or 50",
                    policy_subject.clone(),
                    Vec::new(),
                ));
            }
            if policy.rekey_interval_ns == Some(0) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_MACSEC_REKEY_INTERVAL",
                    "MACsec rekey interval must be positive",
                    policy_subject.clone(),
                    Vec::new(),
                ));
            }
            if policy
                .credential_store_ref
                .as_ref()
                .is_some_and(|reference| reference.trim().is_empty())
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_MACSEC_CREDENTIAL_STORE",
                    "MACsec credential store reference must not be blank",
                    policy_subject,
                    Vec::new(),
                ));
            }
        }

        if let Some(default_policy) = &macsec.default_policy {
            if self.validate_macsec_policy_reference(scope, network, &subject, default_policy) {
                if let (Some(configuration_id), Some(policy_id)) = (
                    &configuration_id,
                    self.resolve(
                        &macsec_policy_key(default_policy, &scope.instance),
                        &subject,
                    ),
                ) {
                    self.add_edge(
                        EdgeKind::MacsecDefaultPolicy,
                        configuration_id,
                        &policy_id,
                        configuration_id,
                        EdgeExactness::Exact,
                        "default-policy",
                    );
                }
            }
        }

        let mut override_targets = BTreeMap::<StableObjectId, ObjectIdentity>::new();
        for override_ in &macsec.overrides {
            let provisional_subject = match &override_.target {
                authored::TopologySegmentRef::Network(_) => self.identity(
                    &scope.instance,
                    ObjectKind::MacsecOverride,
                    [("network", network.name.as_str())],
                ),
                authored::TopologySegmentRef::Leg(reference) => self.identity(
                    &scope.instance,
                    ObjectKind::MacsecOverride,
                    [
                        ("network", network.name.as_str()),
                        ("leg", reference.leg.as_str()),
                    ],
                ),
            };
            let target_id = match &override_.target {
                authored::TopologySegmentRef::Network(reference) => {
                    let coherent = matches!(
                        network.structure,
                        NetworkStructure::Link
                            | NetworkStructure::Bus
                            | NetworkStructure::Star(_)
                            | NetworkStructure::Mesh
                    );
                    if !reference_targets_network(reference, scope, network) {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_CONFIGURATION_SCOPE",
                            "MACsec override network target must be its containing network",
                            provisional_subject.clone(),
                            Vec::new(),
                        ));
                        None
                    } else if !coherent {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_MACSEC_TARGET_KIND",
                            "path topologies require a leg target for a MACsec override",
                            provisional_subject.clone(),
                            Vec::new(),
                        ));
                        None
                    } else {
                        self.resolve(
                            &network_key(reference, &scope.instance),
                            &provisional_subject,
                        )
                    }
                }
                authored::TopologySegmentRef::Leg(reference) => {
                    let coherent = matches!(
                        network.structure,
                        NetworkStructure::Chain(_)
                            | NetworkStructure::Ring(_)
                            | NetworkStructure::Tree(_)
                    );
                    if !reference_targets_network(&reference.network, scope, network) {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_CONFIGURATION_SCOPE",
                            "MACsec override leg target must belong to its containing network",
                            provisional_subject.clone(),
                            Vec::new(),
                        ));
                        None
                    } else if !coherent {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_MACSEC_TARGET_KIND",
                            "only chain, ring, and tree topologies may use a MACsec leg target",
                            provisional_subject.clone(),
                            Vec::new(),
                        ));
                        None
                    } else {
                        self.resolve(&leg_key(reference, &scope.instance), &provisional_subject)
                    }
                }
            };
            let override_subject = target_id
                .as_ref()
                .and_then(|target_id| {
                    self.identity_from_resolved_target(target_id, ObjectKind::MacsecOverride)
                })
                .unwrap_or(provisional_subject);
            let duplicate = target_id.as_ref().is_some_and(|target_id| {
                if let Some(existing) = override_targets.get(target_id) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_MACSEC_DUPLICATE_OVERRIDE",
                        "a topology segment may have only one MACsec override",
                        override_subject.clone(),
                        vec![existing.clone()],
                    ));
                    true
                } else {
                    override_targets.insert(target_id.clone(), override_subject.clone());
                    false
                }
            });
            let override_id = if duplicate {
                None
            } else if let Some(target_id) = target_id {
                let override_id = self.register(
                    override_subject.clone(),
                    ConnectivityNodeData::MacsecOverride {
                        target: override_.target.clone(),
                        policy: override_.policy.clone(),
                    },
                    None,
                );
                if let Some(configuration_id) = &configuration_id {
                    self.add_edge(
                        EdgeKind::Contains,
                        configuration_id,
                        &override_id,
                        &override_id,
                        EdgeExactness::Exact,
                        "override",
                    );
                }
                self.add_edge(
                    EdgeKind::MacsecOverrideTarget,
                    &override_id,
                    &target_id,
                    &override_id,
                    EdgeExactness::Exact,
                    "target",
                );
                Some(override_id)
            } else {
                None
            };
            if self.validate_macsec_policy_reference(
                scope,
                network,
                &override_subject,
                &override_.policy,
            ) {
                if let (Some(override_id), Some(policy_id)) = (
                    override_id,
                    self.resolve(
                        &macsec_policy_key(&override_.policy, &scope.instance),
                        &override_subject,
                    ),
                ) {
                    self.add_edge(
                        EdgeKind::MacsecOverridePolicy,
                        &override_id,
                        &policy_id,
                        &override_id,
                        EdgeExactness::Exact,
                        "policy",
                    );
                }
            }
        }
    }

    fn validate_macsec_policy_reference(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        reference: &authored::MacsecPolicyRef,
    ) -> bool {
        if !network_local_reference_is_nonempty(&reference.network, &reference.policy) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_EMPTY_REFERENCE",
                "MACsec policy reference requires nonempty network and policy names",
                subject.clone(),
                Vec::new(),
            ));
            return false;
        }
        if !reference_targets_network(&reference.network, scope, network) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_CONFIGURATION_SCOPE",
                "MACsec policy reference must belong to the containing network",
                subject.clone(),
                Vec::new(),
            ));
            return false;
        }
        true
    }

    fn connect_eee_configuration(&mut self, scope: &ConnectivityScope, network: &Network) {
        let Some(eee) = &network.configuration.eee else {
            return;
        };
        let configuration_subject = self.identity(
            &scope.instance,
            ObjectKind::EeeConfiguration,
            [("network", network.name.as_str()), ("configuration", "eee")],
        );
        let configuration_id = self.id_for_identity(&configuration_subject);
        let mut targets = BTreeMap::<StableObjectId, ObjectIdentity>::new();
        for override_ in &eee.overrides {
            let provisional_subject = self.identity(
                &scope.instance,
                ObjectKind::EeeOverride,
                [
                    ("network", network.name.as_str()),
                    ("participant", override_.participant.participant.as_str()),
                ],
            );
            if !network_local_reference_is_nonempty(
                &override_.participant.network,
                &override_.participant.participant,
            ) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_EMPTY_REFERENCE",
                    "EEE override requires nonempty network and participant names",
                    provisional_subject,
                    Vec::new(),
                ));
                continue;
            }
            if !reference_targets_network(&override_.participant.network, scope, network) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_CONFIGURATION_SCOPE",
                    "EEE override must reference a participant on its containing network",
                    provisional_subject,
                    Vec::new(),
                ));
                continue;
            }
            let participant_key = participant_key(&override_.participant, &scope.instance);
            let Some(participant_id) = self.resolve(&participant_key, &provisional_subject) else {
                continue;
            };
            let subject = self
                .identity_from_resolved_target(&participant_id, ObjectKind::EeeOverride)
                .unwrap_or(provisional_subject);
            if let Some(existing) = targets.get(&participant_id) {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_EEE_DUPLICATE_OVERRIDE",
                    "a participant may have only one EEE override",
                    subject.clone(),
                    vec![existing.clone()],
                ));
                continue;
            }
            targets.insert(participant_id.clone(), subject.clone());
            let override_id = self.register(
                subject,
                ConnectivityNodeData::EeeOverride {
                    participant: override_.participant.clone(),
                    mode: override_.mode,
                },
                None,
            );
            if let Some(configuration_id) = &configuration_id {
                self.add_edge(
                    EdgeKind::Contains,
                    configuration_id,
                    &override_id,
                    &override_id,
                    EdgeExactness::Exact,
                    "override",
                );
            }
            self.add_edge(
                EdgeKind::EeeOverrideParticipant,
                &participant_id,
                &override_id,
                &override_id,
                EdgeExactness::Exact,
                "participant",
            );
        }
    }

    fn connect_star(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        network_id: &StableObjectId,
        topology: &authored::StarTopology,
    ) {
        if let Some(coordinator) =
            self.resolve_topology_participant(scope, network, subject, &topology.coordinator)
        {
            self.add_edge(
                EdgeKind::StarCoordinator,
                &coordinator,
                network_id,
                network_id,
                EdgeExactness::Exact,
                "coordinator",
            );
        }
    }

    fn connect_path_topology(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        topology: &authored::PathTopology,
        closed: bool,
    ) {
        let minimum_hops = if closed { 3 } else { 2 };
        let expected_legs = if closed {
            topology.hops.len()
        } else {
            topology.hops.len().saturating_sub(1)
        };
        if topology.hops.len() < minimum_hops || topology.legs.len() != expected_legs {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_ARITY",
                format!(
                    "{} requires at least {minimum_hops} hops and exactly {expected_legs} legs",
                    if closed { "ring" } else { "chain" }
                ),
                subject.clone(),
                Vec::new(),
            ));
        }

        for hop in &topology.hops {
            self.connect_hop_owner(scope, network, hop);
        }
        let mut expected = BTreeSet::new();
        for adjacent in topology.hops.windows(2) {
            expected.insert((adjacent[0].name.clone(), adjacent[1].name.clone()));
        }
        if closed && topology.hops.len() >= 2 {
            expected.insert((
                topology
                    .hops
                    .last()
                    .expect("nonempty ring hops")
                    .name
                    .clone(),
                topology.hops[0].name.clone(),
            ));
        }

        let mut actual = BTreeMap::<(String, String), ObjectIdentity>::new();
        let mut used_participants = BTreeMap::new();
        for leg in &topology.legs {
            let leg_subject = self.identity(
                &scope.instance,
                ObjectKind::Leg,
                [
                    ("network", network.name.as_str()),
                    ("leg", leg.name.as_str()),
                ],
            );
            if let Some((from_hop, to_hop, from_participant, to_participant)) =
                self.connect_leg(scope, network, &leg_subject, leg)
            {
                if let Some(existing) = actual.insert((from_hop, to_hop), leg_subject.clone()) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_TOPOLOGY_DUPLICATE_LEG",
                        "topology cannot declare duplicate oriented hop adjacency",
                        leg_subject,
                        vec![existing],
                    ));
                }
                *used_participants.entry(from_participant).or_default() += 1;
                *used_participants.entry(to_participant).or_default() += 1;
            }
        }
        if actual.keys().cloned().collect::<BTreeSet<_>>() != expected {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_ADJACENCY",
                "chain and ring legs must exactly follow semantic hop order and ring closure",
                subject.clone(),
                Vec::new(),
            ));
        }
        self.validate_topology_participant_coverage(network, subject, &used_participants);
    }

    fn connect_tree(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        network_id: &StableObjectId,
        topology: &authored::TreeTopology,
    ) {
        if topology.hops.len() < 2 || topology.legs.len() != topology.hops.len().saturating_sub(1) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_ARITY",
                "tree requires at least two hops and exactly one fewer leg than hops",
                subject.clone(),
                Vec::new(),
            ));
        }
        for hop in &topology.hops {
            self.connect_hop_owner(scope, network, hop);
        }
        if let Some(root) = self.resolve_topology_hop(scope, network, subject, &topology.root) {
            self.add_edge(
                EdgeKind::TreeRoot,
                &root,
                network_id,
                network_id,
                EdgeExactness::Exact,
                "root",
            );
        }

        let declared = topology
            .hops
            .iter()
            .map(|hop| hop.name.clone())
            .collect::<BTreeSet<_>>();
        if !declared.contains(&topology.root.hop) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_ROOT",
                "tree root must reference one of the tree's declared hops",
                subject.clone(),
                Vec::new(),
            ));
        }
        let mut incoming = declared
            .iter()
            .map(|hop| (hop.clone(), 0usize))
            .collect::<BTreeMap<_, _>>();
        let mut adjacency = BTreeMap::<String, BTreeSet<String>>::new();
        let mut pairs = BTreeSet::new();
        let mut used_participants = BTreeMap::new();

        for leg in &topology.legs {
            let leg_subject = self.identity(
                &scope.instance,
                ObjectKind::Leg,
                [
                    ("network", network.name.as_str()),
                    ("leg", leg.name.as_str()),
                ],
            );
            if let Some((from_hop, to_hop, from_participant, to_participant)) =
                self.connect_leg(scope, network, &leg_subject, leg)
            {
                if !pairs.insert((from_hop.clone(), to_hop.clone())) {
                    self.issues.push(ConnectivityIssue::error(
                        "E_CONN_TOPOLOGY_DUPLICATE_LEG",
                        "tree cannot repeat an oriented hop adjacency",
                        leg_subject,
                        Vec::new(),
                    ));
                }
                if let Some(count) = incoming.get_mut(&to_hop) {
                    *count += 1;
                }
                adjacency.entry(from_hop).or_default().insert(to_hop);
                *used_participants.entry(from_participant).or_default() += 1;
                *used_participants.entry(to_participant).or_default() += 1;
            }
        }

        let incoming_is_valid = incoming.iter().all(|(hop, count)| {
            if hop == &topology.root.hop {
                *count == 0
            } else {
                *count == 1
            }
        });
        if !incoming_is_valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_INCOMING",
                "tree root must have no incoming leg and every other hop exactly one",
                subject.clone(),
                Vec::new(),
            ));
        }

        let mut reachable = BTreeSet::new();
        let mut frontier = vec![topology.root.hop.clone()];
        while let Some(hop) = frontier.pop() {
            if !reachable.insert(hop.clone()) {
                continue;
            }
            if let Some(children) = adjacency.get(&hop) {
                frontier.extend(children.iter().cloned());
            }
        }
        if reachable != declared {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_REACHABILITY",
                "every tree hop must be reachable from the declared root",
                subject.clone(),
                Vec::new(),
            ));
        }

        let mut remaining_incoming = incoming.clone();
        let mut ready = remaining_incoming
            .iter()
            .filter_map(|(hop, count)| (*count == 0).then_some(hop.clone()))
            .collect::<Vec<_>>();
        let mut visited = 0usize;
        while let Some(hop) = ready.pop() {
            visited += 1;
            if let Some(children) = adjacency.get(&hop) {
                for child in children {
                    let count = remaining_incoming
                        .get_mut(child)
                        .expect("tree adjacency target is declared");
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.push(child.clone());
                    }
                }
            }
        }
        if visited != declared.len() {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_CYCLE",
                "tree topology must be acyclic",
                subject.clone(),
                Vec::new(),
            ));
        }
        self.validate_topology_participant_coverage(network, subject, &used_participants);
    }

    fn connect_hop_owner(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        hop: &authored::Hop,
    ) {
        let hop_subject = self.identity(
            &scope.instance,
            ObjectKind::Hop,
            [
                ("network", network.name.as_str()),
                ("hop", hop.name.as_str()),
            ],
        );
        let Some(hop_id) = self.id_for_identity(&hop_subject) else {
            return;
        };
        if !self.validate_hop_owner_reference(&hop_subject, &hop.owner) {
            return;
        }
        let key = match &hop.owner {
            HopOwnerRef::Component(reference) => ReferenceKey::Component {
                instance: reference.scope.resolve(&scope.instance),
                component: reference.component.clone(),
            },
            HopOwnerRef::Function(reference) => function_key(reference, &scope.instance),
        };
        if let Some(owner_id) = self.resolve(&key, &hop_subject) {
            self.add_edge(
                EdgeKind::HopOwnership,
                &owner_id,
                &hop_id,
                &hop_id,
                EdgeExactness::Exact,
                "owner",
            );
        }
    }

    fn connect_leg(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        leg: &authored::Leg,
    ) -> Option<(String, String, String, String)> {
        let leg_id = self.id_for_identity(subject)?;
        let from = self.resolve_leg_end(scope, network, subject, &leg.from)?;
        let to = self.resolve_leg_end(scope, network, subject, &leg.to)?;
        self.add_edge(
            EdgeKind::LegFromHop,
            &from.0,
            &leg_id,
            &leg_id,
            EdgeExactness::Exact,
            "from-hop",
        );
        self.add_edge(
            EdgeKind::LegFromParticipant,
            &from.1,
            &leg_id,
            &leg_id,
            EdgeExactness::Exact,
            "from-participant",
        );
        self.add_edge(
            EdgeKind::LegToHop,
            &leg_id,
            &to.0,
            &leg_id,
            EdgeExactness::Exact,
            "to-hop",
        );
        self.add_edge(
            EdgeKind::LegToParticipant,
            &leg_id,
            &to.1,
            &leg_id,
            EdgeExactness::Exact,
            "to-participant",
        );
        Some((
            leg.from.hop.hop.clone(),
            leg.to.hop.hop.clone(),
            leg.from.participant.participant.clone(),
            leg.to.participant.participant.clone(),
        ))
    }

    fn resolve_leg_end(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        end: &LegEnd,
    ) -> Option<(StableObjectId, StableObjectId)> {
        let hop_id = self.resolve_topology_hop(scope, network, subject, &end.hop)?;
        let participant_id =
            self.resolve_topology_participant(scope, network, subject, &end.participant)?;
        let hop = network
            .structure
            .hops()
            .iter()
            .find(|hop| hop.name == end.hop.hop)?;
        let participant = network
            .participants
            .iter()
            .find(|participant| participant.name == end.participant.participant)?;
        let endpoint_id = self.resolve(
            &functional_key(&participant.endpoint, &scope.instance),
            subject,
        )?;
        let port_id = self.resolve(
            &port_key(&participant.endpoint.port(), &scope.instance),
            subject,
        )?;
        let owner_key = match &hop.owner {
            HopOwnerRef::Component(reference) => ReferenceKey::Component {
                instance: reference.scope.resolve(&scope.instance),
                component: reference.component.clone(),
            },
            HopOwnerRef::Function(reference) => function_key(reference, &scope.instance),
        };
        let owner_id = self.resolve(&owner_key, subject)?;

        let owns_endpoint = match &hop.owner {
            HopOwnerRef::Component(_) => self.edges.values().any(|edge| {
                edge.kind() == EdgeKind::Owns && edge.from() == &owner_id && edge.to() == &port_id
            }),
            HopOwnerRef::Function(_) => self.edges.values().any(|edge| {
                matches!(
                    edge.kind(),
                    EdgeKind::FunctionInput
                        | EdgeKind::FunctionOutput
                        | EdgeKind::FunctionBidirectional
                ) && ((edge.from() == &endpoint_id && edge.to() == &owner_id)
                    || (edge.from() == &owner_id && edge.to() == &endpoint_id))
            }),
        };
        if !owns_endpoint {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_LEG_ENDPOINT_OWNER",
                "each leg-end participant endpoint must belong to its hop owner; function-owned hops accept any declared input, output, or bidirectional endpoint",
                subject.clone(),
                Vec::new(),
            ));
        }
        Some((hop_id, participant_id))
    }

    fn resolve_topology_hop(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        reference: &authored::HopRef,
    ) -> Option<StableObjectId> {
        if !self.validate_topology_reference(
            scope,
            network,
            subject,
            &reference.network,
            &reference.hop,
            "hop",
        ) {
            return None;
        }
        self.resolve(&hop_key(reference, &scope.instance), subject)
    }

    fn resolve_topology_participant(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        reference: &ParticipantRef,
    ) -> Option<StableObjectId> {
        if !self.validate_topology_reference(
            scope,
            network,
            subject,
            &reference.network,
            &reference.participant,
            "participant",
        ) {
            return None;
        }
        self.resolve(&participant_key(reference, &scope.instance), subject)
    }

    fn validate_topology_reference(
        &mut self,
        scope: &ConnectivityScope,
        network: &Network,
        subject: &ObjectIdentity,
        network_reference: &authored::NetworkRef,
        local_name: &str,
        kind: &str,
    ) -> bool {
        if network_reference.network.trim().is_empty() || local_name.trim().is_empty() {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_EMPTY_REFERENCE",
                format!("topology {kind} references require nonempty network and object names"),
                subject.clone(),
                Vec::new(),
            ));
            return false;
        }
        if network_reference.scope.resolve(&scope.instance) != scope.instance
            || network_reference.network != network.name
        {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_SCOPE",
                format!("topology {kind} references must target the containing network"),
                subject.clone(),
                Vec::new(),
            ));
            return false;
        }
        true
    }

    fn validate_hop_owner_reference(
        &mut self,
        subject: &ObjectIdentity,
        owner: &HopOwnerRef,
    ) -> bool {
        let valid = match owner {
            HopOwnerRef::Component(reference) => !reference.component.trim().is_empty(),
            HopOwnerRef::Function(reference) => {
                !reference.component.component.trim().is_empty()
                    && !reference.function.trim().is_empty()
            }
        };
        if !valid {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_EMPTY_REFERENCE",
                "hop owner references require nonempty component and function names",
                subject.clone(),
                Vec::new(),
            ));
        }
        valid
    }

    fn validate_topology_participant_coverage(
        &mut self,
        network: &Network,
        subject: &ObjectIdentity,
        used: &BTreeMap<String, usize>,
    ) {
        let declared = network
            .participants
            .iter()
            .map(|participant| participant.name.clone())
            .collect::<BTreeSet<_>>();
        if declared != used.keys().cloned().collect::<BTreeSet<_>>()
            || used.values().any(|count| *count != 1)
        {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_TOPOLOGY_COVERAGE",
                "chain, ring, and tree legs must use every declared network participant exactly once and may not reference undeclared participants",
                subject.clone(),
                Vec::new(),
            ));
        }
    }

    fn check_selection(
        &mut self,
        subject: &ObjectIdentity,
        endpoint_id: &StableObjectId,
        port_id: &StableObjectId,
        selected: &NetworkSelection,
    ) {
        let purpose = Some(selected.purpose);
        let carrier = Some(selected.carrier);
        let selection_owner = "network";
        let Some(port_node) = self.nodes.get(port_id) else {
            return;
        };
        let ConnectivityNodeData::Port {
            capabilities: port_capabilities,
        } = port_node.data()
        else {
            return;
        };
        let port_capabilities = port_capabilities.clone();
        let mut related = vec![port_node.identity().clone()];
        let channel_capabilities = if endpoint_id == port_id {
            None
        } else {
            self.nodes.get(endpoint_id).and_then(|node| {
                let ConnectivityNodeData::Channel { capabilities, .. } = node.data() else {
                    return None;
                };
                related.push(node.identity().clone());
                Some(capabilities.clone())
            })
        };
        let layers = std::iter::once(&port_capabilities)
            .chain(channel_capabilities.as_ref())
            .collect::<Vec<_>>();
        if let Some(purpose) = purpose {
            let evidence = layers
                .iter()
                .filter(|capabilities| !capabilities.purposes.is_empty())
                .collect::<Vec<_>>();
            if evidence.is_empty() {
                self.issues.push(ConnectivityIssue::warning(
                    "W_CONN_SELECTION_UNVERIFIED",
                    format!(
                        "{selection_owner} selects purpose {purpose:?}, but the endpoint declares no purpose capability evidence"
                    ),
                    subject.clone(),
                    related.clone(),
                ));
            } else if evidence
                .iter()
                .any(|capabilities| !capabilities.purposes.contains(&purpose))
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_SELECTION_UNSUPPORTED",
                    format!(
                        "{selection_owner} selects purpose {purpose:?}, but a port or channel layer affirmatively lists different supported purposes"
                    ),
                    subject.clone(),
                    related.clone(),
                ));
            }
        }
        if let Some(carrier) = carrier {
            let evidence = layers
                .iter()
                .filter(|capabilities| !capabilities.carriers.is_empty())
                .collect::<Vec<_>>();
            if evidence.is_empty() {
                self.issues.push(ConnectivityIssue::warning(
                    "W_CONN_SELECTION_UNVERIFIED",
                    format!(
                        "{selection_owner} selects carrier {carrier:?}, but the endpoint declares no carrier capability evidence"
                    ),
                    subject.clone(),
                    related.clone(),
                ));
            } else if evidence
                .iter()
                .any(|capabilities| !capabilities.carriers.contains(&carrier))
            {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_SELECTION_UNSUPPORTED",
                    format!(
                        "{selection_owner} selects carrier {carrier:?}, but a port or channel layer affirmatively lists different supported carriers"
                    ),
                    subject.clone(),
                    related.clone(),
                ));
            }
        }
        if !selected.profiles.is_empty() {
            let profiles = layers
                .iter()
                .flat_map(|capabilities| capabilities.profiles.iter())
                .collect::<BTreeSet<_>>();
            if profiles.is_empty() {
                self.issues.push(ConnectivityIssue::warning(
                    "W_CONN_SELECTION_UNVERIFIED",
                    format!(
                        "{selection_owner} selects profile(s), but the endpoint declares no profile capability evidence"
                    ),
                    subject.clone(),
                    related.clone(),
                ));
            } else {
                for profile in &selected.profiles {
                    if !profiles.contains(profile) {
                        self.issues.push(ConnectivityIssue::error(
                            "E_CONN_SELECTION_UNSUPPORTED",
                            format!(
                                "{selection_owner} selects profile {profile}, but neither the port nor channel lists it as supported"
                            ),
                            subject.clone(),
                            related.clone(),
                        ));
                    }
                }
            }
        }
        let selected_frequency = selected
            .rf
            .as_ref()
            .and_then(|rf| rf.channel.center_frequency())
            .or(selected.frequency.as_ref());
        let selected_bandwidth = selected.rf.as_ref().and_then(|rf| rf.channel.bandwidth());
        for (axis, quantity, port_range, channel_range) in [
            (
                "rate",
                selected.rate.as_ref(),
                port_capabilities.limits.rate.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.rate.as_ref()),
            ),
            (
                "voltage",
                selected.voltage.as_ref(),
                port_capabilities.limits.voltage.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.voltage.as_ref()),
            ),
            (
                "current",
                selected.current.as_ref(),
                port_capabilities.limits.current.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.current.as_ref()),
            ),
            (
                "power",
                selected.power.as_ref(),
                port_capabilities.limits.power.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.power.as_ref()),
            ),
            (
                "impedance",
                selected.impedance.as_ref(),
                port_capabilities.limits.impedance.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.impedance.as_ref()),
            ),
            (
                "frequency",
                selected_frequency,
                port_capabilities.limits.frequency.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.frequency.as_ref()),
            ),
            (
                "bandwidth",
                selected_bandwidth,
                port_capabilities.limits.bandwidth.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.bandwidth.as_ref()),
            ),
            (
                "pressure",
                selected.pressure.as_ref(),
                port_capabilities.limits.pressure.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.pressure.as_ref()),
            ),
            (
                "flow",
                selected.flow.as_ref(),
                port_capabilities.limits.flow.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.flow.as_ref()),
            ),
            (
                "temperature",
                selected.temperature.as_ref(),
                port_capabilities.limits.temperature.as_ref(),
                channel_capabilities
                    .as_ref()
                    .and_then(|capabilities| capabilities.limits.temperature.as_ref()),
            ),
        ] {
            let Some(quantity) = quantity else {
                continue;
            };
            let frequency_envelope = if axis == "frequency" {
                normalized_rf_frequency_envelope(selected)
            } else {
                None
            };
            if frequency_envelope
                .as_ref()
                .is_some_and(|(minimum, maximum)| {
                    !minimum.value.is_finite() || !maximum.value.is_finite()
                })
            {
                continue;
            }
            let Some((mut selected_minimum, mut selected_maximum)) =
                normalized_selection_envelope(quantity)
            else {
                continue;
            };
            if let Some((minimum, maximum)) = frequency_envelope {
                selected_minimum = minimum;
                selected_maximum = maximum;
            }
            let ranges = [port_range, channel_range]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            if ranges.is_empty() {
                self.issues.push(ConnectivityIssue::warning(
                    "W_CONN_SELECTION_UNVERIFIED",
                    format!(
                        "{selection_owner} selects {axis}, but the endpoint declares no {axis} capability range"
                    ),
                    subject.clone(),
                    related.clone(),
                ));
                continue;
            }
            let selected_dimension = selected_minimum.dimension;
            let mut dimension_mismatch = false;
            let mut outside = false;
            for range in ranges {
                let Some((range_dimension, minimum, maximum)) =
                    normalized_capability_envelope(range)
                else {
                    continue;
                };
                if range_dimension != selected_dimension {
                    dimension_mismatch = true;
                    continue;
                }
                outside |= minimum
                    .is_some_and(|minimum| definitely_less_normalized(selected_minimum, minimum))
                    || maximum.is_some_and(|maximum| {
                        definitely_greater_normalized(selected_maximum, maximum)
                    });
            }
            if dimension_mismatch {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_DIMENSION",
                    format!(
                        "{selection_owner} selects {axis} with a different dimension than a port or channel capability range"
                    ),
                    subject.clone(),
                    related.clone(),
                ));
            }
            if outside {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_SELECTION_RANGE",
                    format!(
                        "{selection_owner} selects {axis} outside a port or channel capability range"
                    ),
                    subject.clone(),
                    related.clone(),
                ));
            }
        }
    }

    fn binding_exactness(
        &mut self,
        binding: &crate::model::connectivity::Binding,
        subject: &ObjectIdentity,
    ) -> Option<EdgeExactness> {
        match (&binding.functional, &binding.physical, binding.fidelity) {
            (
                FunctionalEndpointRef::Port(_),
                PhysicalEndpointRef::Connector(_),
                Fidelity::Presented,
            ) => Some(EdgeExactness::Coarse),
            (FunctionalEndpointRef::Channel(_), PhysicalEndpointRef::Position(_), fidelity)
                if fidelity.is_exact() =>
            {
                Some(EdgeExactness::Exact)
            }
            _ => {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_BINDING_FIDELITY",
                    "presented bindings pair a whole port with a connector; exact or quantified bindings pair a channel with a position",
                    subject.clone(),
                    Vec::new(),
                ));
                None
            }
        }
    }

    fn path_exactness(
        &mut self,
        fidelity: Fidelity,
        first: &PhysicalEndpointRef,
        second: &PhysicalEndpointRef,
        subject: &ObjectIdentity,
    ) -> Option<EdgeExactness> {
        let both_connectors = matches!(first, PhysicalEndpointRef::Connector(_))
            && matches!(second, PhysicalEndpointRef::Connector(_));
        let both_exact = !matches!(first, PhysicalEndpointRef::Connector(_))
            && !matches!(second, PhysicalEndpointRef::Connector(_));
        if fidelity == Fidelity::Presented && both_connectors {
            Some(EdgeExactness::Coarse)
        } else if fidelity.is_exact() && both_exact {
            Some(EdgeExactness::Exact)
        } else {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_PATH_FIDELITY",
                "presented paths join connector shells; exact or quantified paths join positions or junctions",
                subject.clone(),
                Vec::new(),
            ));
            None
        }
    }

    fn identity<'b>(
        &self,
        instance: &IncludeInstanceId,
        kind: ObjectKind,
        local: impl IntoIterator<Item = (&'b str, &'b str)>,
    ) -> ObjectIdentity {
        ObjectIdentity::new(
            self.authored.document.clone(),
            instance.clone(),
            kind,
            local
                .into_iter()
                .map(|(field, value)| IdentityPart::new(field, value))
                .collect(),
        )
    }

    fn identity_from_resolved_target(
        &self,
        target_id: &StableObjectId,
        kind: ObjectKind,
    ) -> Option<ObjectIdentity> {
        let target = self.nodes.get(target_id)?;
        Some(ObjectIdentity::new(
            target.identity().document().clone(),
            target.identity().instance().clone(),
            kind,
            target.identity().local().to_vec(),
        ))
    }

    fn named_identity(
        &self,
        scope: &ConnectivityScope,
        kind: ObjectKind,
        field: &str,
        name: &str,
    ) -> ObjectIdentity {
        self.identity(&scope.instance, kind, [(field, name)])
    }
    fn owned_identity(
        &self,
        scope: &ConnectivityScope,
        kind: ObjectKind,
        owner_field: &str,
        owner_name: &str,
        object_field: &str,
        object_name: &str,
    ) -> ObjectIdentity {
        self.identity(
            &scope.instance,
            kind,
            [(owner_field, owner_name), (object_field, object_name)],
        )
    }

    fn register(
        &mut self,
        identity: ObjectIdentity,
        data: ConnectivityNodeData,
        reference: Option<ReferenceKey>,
    ) -> StableObjectId {
        if identity
            .local()
            .iter()
            .any(|part| part.value.trim().is_empty())
        {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_EMPTY_IDENTITY",
                "connectivity identity fields must not be empty",
                identity.clone(),
                Vec::new(),
            ));
        }
        let id = identity.stable_id();
        if let Some(existing) = self.identities.get(&identity) {
            self.issues.push(ConnectivityIssue::error(
                "E_CONN_DUPLICATE_IDENTITY",
                "duplicate connectivity object identity in the same include instance",
                identity,
                Vec::new(),
            ));
            return existing.clone();
        }
        if let Some(reference) = reference {
            if self.references.insert(reference, id.clone()).is_some() {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_DUPLICATE_IDENTITY",
                    "duplicate typed reference target in the same include instance",
                    identity.clone(),
                    Vec::new(),
                ));
            }
        }
        self.identities.insert(identity.clone(), id.clone());
        self.nodes
            .insert(id.clone(), ConnectivityNode::new(identity, data));
        id
    }

    fn id_for_identity(&self, identity: &ObjectIdentity) -> Option<StableObjectId> {
        self.identities.get(identity).cloned()
    }

    fn resolve(&mut self, key: &ReferenceKey, subject: &ObjectIdentity) -> Option<StableObjectId> {
        match self.references.get(key) {
            Some(id) => Some(id.clone()),
            None => {
                self.issues.push(ConnectivityIssue::error(
                    "E_CONN_UNRESOLVED_REFERENCE",
                    format!("unresolved typed reference: {}", key.describe()),
                    subject.clone(),
                    Vec::new(),
                ));
                None
            }
        }
    }

    fn add_edge(
        &mut self,
        kind: EdgeKind,
        from: &StableObjectId,
        to: &StableObjectId,
        subject: &StableObjectId,
        exactness: EdgeExactness,
        discriminator: &str,
    ) {
        let fields = [
            kind.as_str().as_bytes(),
            from.as_str().as_bytes(),
            to.as_str().as_bytes(),
            subject.as_str().as_bytes(),
            discriminator.as_bytes(),
        ];
        let id = StableEdgeId(format!(
            "hcdf-edge-v1:{}",
            stable_digest("hcdf-connectivity-edge-v1", fields)
        ));
        self.edges.insert(
            id.clone(),
            NormalizedConnectivityEdge::new(
                id,
                kind,
                from.clone(),
                to.clone(),
                subject.clone(),
                exactness,
            ),
        );
    }

    fn finish(mut self) -> Result<NormalizedConnectivityGraph, NormalizationError> {
        self.issues.sort_by(|first, second| {
            first
                .subject_id()
                .cmp(&second.subject_id())
                .then_with(|| first.code().cmp(second.code()))
                .then_with(|| first.message().cmp(second.message()))
        });
        if self
            .issues
            .iter()
            .any(|issue| issue.level() == IssueLevel::Error)
        {
            return Err(NormalizationError {
                issues: self.issues,
            });
        }
        Ok(NormalizedConnectivityGraph::new(
            self.authored.document.clone(),
            self.nodes.into_values().collect(),
            self.edges.into_values().collect(),
            self.issues,
            self.references,
        ))
    }
}

fn route_rotation_is_valid(rotation: &crate::model::connectivity::PlacementRotation) -> bool {
    match rotation {
        crate::model::connectivity::PlacementRotation::Rpy(values) => {
            values.iter().all(|value| value.is_finite())
        }
        crate::model::connectivity::PlacementRotation::Quaternion(values) => {
            let norm_squared = values.iter().map(|value| value * value).sum::<f64>();
            values.iter().all(|value| value.is_finite())
                && norm_squared.is_finite()
                && (norm_squared - 1.0).abs() <= 1.0e-6
        }
    }
}

fn rotation_local_z(rotation: &crate::model::connectivity::PlacementRotation) -> [f64; 3] {
    match rotation {
        crate::model::connectivity::PlacementRotation::Rpy([roll, pitch, yaw]) => {
            let (sr, cr) = roll.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            let (sy, cy) = yaw.sin_cos();
            [cy * sp * cr + sy * sr, sy * sp * cr - cy * sr, cp * cr]
        }
        crate::model::connectivity::PlacementRotation::Quaternion([x, y, z, w]) => [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    }
}

fn rectangular_route_tangents_are_valid(
    route: &crate::model::connectivity::DerivedRouteRepresentation,
) -> bool {
    const MIN_ALIGNMENT: f64 = 0.999_847_695_156_391_3;
    if route.waypoints.len() < 2 {
        return true;
    }
    for (index, waypoint) in route.waypoints.iter().enumerate() {
        let delta = if index == 0 {
            let next = &route.waypoints[1];
            (next.frame == waypoint.frame).then(|| {
                [
                    next.xyz[0] - waypoint.xyz[0],
                    next.xyz[1] - waypoint.xyz[1],
                    next.xyz[2] - waypoint.xyz[2],
                ]
            })
        } else if index + 1 == route.waypoints.len() {
            let previous = &route.waypoints[index - 1];
            (previous.frame == waypoint.frame).then(|| {
                [
                    waypoint.xyz[0] - previous.xyz[0],
                    waypoint.xyz[1] - previous.xyz[1],
                    waypoint.xyz[2] - previous.xyz[2],
                ]
            })
        } else {
            let previous = &route.waypoints[index - 1];
            let next = &route.waypoints[index + 1];
            (previous.frame == waypoint.frame && next.frame == waypoint.frame).then(|| {
                [
                    next.xyz[0] - previous.xyz[0],
                    next.xyz[1] - previous.xyz[1],
                    next.xyz[2] - previous.xyz[2],
                ]
            })
        };
        let Some(delta) = delta else {
            continue;
        };
        let Some(rotation) = waypoint.rotation.as_ref() else {
            return false;
        };
        let local_z = rotation_local_z(rotation);
        let tangent_norm = delta.iter().map(|value| value * value).sum::<f64>().sqrt();
        let z_norm = local_z
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        if tangent_norm <= f64::EPSILON || z_norm <= f64::EPSILON {
            return false;
        }
        let alignment = (delta[0] * local_z[0] + delta[1] * local_z[1] + delta[2] * local_z[2])
            / (tangent_norm * z_norm);
        if !alignment.is_finite() || alignment < MIN_ALIGNMENT {
            return false;
        }
    }
    true
}
fn network_local_reference_is_nonempty(
    reference: &crate::model::connectivity::NetworkRef,
    local_name: &str,
) -> bool {
    !reference.network.trim().is_empty() && !local_name.trim().is_empty()
}

fn reference_targets_network(
    reference: &crate::model::connectivity::NetworkRef,
    scope: &ConnectivityScope,
    network: &Network,
) -> bool {
    reference.scope.resolve(&scope.instance) == scope.instance && reference.network == network.name
}

fn macsec_cipher_is_xpn(cipher: &authored::QualifiedId) -> bool {
    matches!(
        cipher.as_str(),
        "ieee:gcm-aes-xpn-128" | "ieee:gcm-aes-xpn-256"
    )
}

fn port_belongs_to_component(
    reference: &crate::model::connectivity::PortRef,
    scope: &ConnectivityScope,
    component: &str,
) -> bool {
    reference.scope.resolve(&scope.instance) == scope.instance && reference.component == component
}

fn binding_belongs_to_same_component(
    binding: &crate::model::connectivity::Binding,
    scope: &ConnectivityScope,
) -> bool {
    let (functional_scope, functional_component) = match &binding.functional {
        FunctionalEndpointRef::Port(reference) => (&reference.scope, &reference.component),
        FunctionalEndpointRef::Channel(reference) => (&reference.scope, &reference.component),
    };
    let owner = match &binding.physical {
        PhysicalEndpointRef::Connector(reference) => &reference.owner,
        PhysicalEndpointRef::Position(reference) => &reference.owner,
        PhysicalEndpointRef::Junction(reference) => &reference.owner,
    };
    let authored::OwnerRef::Component(physical_component) = owner else {
        return false;
    };
    functional_component == &physical_component.component
        && functional_scope.resolve(&scope.instance)
            == physical_component.scope.resolve(&scope.instance)
}

fn functional_belongs_to_component(
    endpoint: &FunctionalEndpointRef,
    scope: &ConnectivityScope,
    component: &str,
) -> bool {
    match endpoint {
        FunctionalEndpointRef::Port(reference) => {
            port_belongs_to_component(reference, scope, component)
        }
        FunctionalEndpointRef::Channel(reference) => {
            reference.scope.resolve(&scope.instance) == scope.instance
                && reference.component == component
        }
    }
}

fn physical_belongs_to_owner(
    endpoint: &PhysicalEndpointRef,
    scope: &ConnectivityScope,
    expected: &OwnerKey,
) -> bool {
    let actual = match endpoint {
        PhysicalEndpointRef::Connector(reference) => owner_key(&reference.owner, &scope.instance),
        PhysicalEndpointRef::Position(reference) => owner_key(&reference.owner, &scope.instance),
        PhysicalEndpointRef::Junction(reference) => owner_key(&reference.owner, &scope.instance),
    };
    &actual == expected
}

fn physical_exactness(
    fidelity: Fidelity,
    endpoint: &PhysicalEndpointRef,
    subject: &ObjectIdentity,
    label: &str,
    issues: &mut Vec<ConnectivityIssue>,
) -> Option<EdgeExactness> {
    if fidelity == Fidelity::Presented && matches!(endpoint, PhysicalEndpointRef::Connector(_)) {
        Some(EdgeExactness::Coarse)
    } else if fidelity.is_exact()
        && matches!(
            endpoint,
            PhysicalEndpointRef::Position(_) | PhysicalEndpointRef::Junction(_)
        )
    {
        Some(EdgeExactness::Exact)
    } else {
        issues.push(ConnectivityIssue::error(
            "E_CONN_PHYSICAL_FIDELITY",
            format!(
                "presented {label} use connector shells; exact or quantified {label} use positions or junctions"
            ),
            subject.clone(),
            Vec::new(),
        ));
        None
    }
}

fn scope_has_empty_instance_reference(scope: &ConnectivityScope) -> bool {
    scope
        .components
        .iter()
        .any(component_has_empty_instance_reference)
        || scope
            .assemblies
            .iter()
            .any(assembly_has_empty_instance_reference)
        || scope.bindings.iter().any(|binding| {
            functional_has_empty_instance_reference(&binding.functional)
                || physical_has_empty_instance_reference(&binding.physical)
        })
        || scope.mates.iter().any(|mate| {
            connector_has_empty_instance_reference(&mate.first)
                || connector_has_empty_instance_reference(&mate.second)
                || mate.mappings.iter().any(|mapping| {
                    position_has_empty_instance_reference(&mapping.first)
                        || position_has_empty_instance_reference(&mapping.second)
                })
        })
        || scope
            .networks
            .iter()
            .any(network_has_empty_instance_reference)
}

fn network_has_empty_instance_reference(network: &Network) -> bool {
    network
        .participants
        .iter()
        .any(|participant| functional_has_empty_instance_reference(&participant.endpoint))
        || network.structure.hops().iter().any(|hop| match &hop.owner {
            HopOwnerRef::Component(reference) => reference_scope_is_empty(&reference.scope),
            HopOwnerRef::Function(reference) => {
                reference_scope_is_empty(&reference.component.scope)
            }
        })
        || network.structure.legs().iter().any(|leg| {
            leg_end_has_empty_instance_reference(&leg.from)
                || leg_end_has_empty_instance_reference(&leg.to)
        })
        || network.configuration.gptp_domains.iter().any(|domain| {
            domain
                .clocks
                .iter()
                .any(|clock| reference_scope_is_empty(&clock.participant.network.scope))
        })
        || network
            .configuration
            .gate_schedules
            .iter()
            .flat_map(|schedule| &schedule.entries)
            .flat_map(|entry| &entry.open)
            .any(|reference| reference_scope_is_empty(&reference.network.scope))
        || network
            .configuration
            .schedule_assignments
            .iter()
            .any(|assignment| {
                assignment
                    .targets
                    .iter()
                    .any(|target| reference_scope_is_empty(&target.network.scope))
                    || reference_scope_is_empty(&assignment.schedule.network.scope)
            })
        || network.configuration.plca.as_ref().is_some_and(|plca| {
            plca.nodes
                .iter()
                .any(|node| reference_scope_is_empty(&node.participant.network.scope))
        })
        || network.configuration.macsec.as_ref().is_some_and(|macsec| {
            macsec
                .default_policy
                .as_ref()
                .is_some_and(|reference| reference_scope_is_empty(&reference.network.scope))
                || macsec.overrides.iter().any(|override_| {
                    reference_scope_is_empty(&override_.policy.network.scope)
                        || match &override_.target {
                            authored::TopologySegmentRef::Network(reference) => {
                                reference_scope_is_empty(&reference.scope)
                            }
                            authored::TopologySegmentRef::Leg(reference) => {
                                reference_scope_is_empty(&reference.network.scope)
                            }
                        }
                })
        })
        || network.configuration.eee.as_ref().is_some_and(|eee| {
            eee.overrides
                .iter()
                .any(|override_| reference_scope_is_empty(&override_.participant.network.scope))
        })
        || match &network.structure {
            NetworkStructure::Star(topology) => {
                reference_scope_is_empty(&topology.coordinator.network.scope)
            }
            NetworkStructure::Tree(topology) => {
                reference_scope_is_empty(&topology.root.network.scope)
            }
            _ => false,
        }
}

fn leg_end_has_empty_instance_reference(end: &LegEnd) -> bool {
    reference_scope_is_empty(&end.hop.network.scope)
        || reference_scope_is_empty(&end.participant.network.scope)
}

fn component_has_empty_instance_reference(component: &authored::ComponentConnectivity) -> bool {
    component
        .connectors
        .iter()
        .any(connector_declaration_has_empty_instance_reference)
        || component.antennas.iter().any(|antenna| {
            antenna
                .conducted_port
                .as_ref()
                .is_some_and(|port| reference_scope_is_empty(&port.scope))
                || reference_scope_is_empty(&antenna.radiated_port.scope)
                || antenna
                    .representation
                    .as_ref()
                    .is_some_and(representation_has_empty_instance_reference)
        })
        || component.functions.iter().any(|function| {
            function
                .inputs
                .iter()
                .chain(&function.outputs)
                .chain(&function.bidirectional)
                .any(functional_has_empty_instance_reference)
        })
        || owned_physical_has_empty_instance_reference(
            &component.paths,
            &component.junctions,
            &component.terminations,
        )
}

fn assembly_has_empty_instance_reference(assembly: &authored::PhysicalAssembly) -> bool {
    assembly
        .representation
        .as_ref()
        .is_some_and(representation_has_empty_instance_reference)
        || assembly
            .connectors
            .iter()
            .any(connector_declaration_has_empty_instance_reference)
        || owned_physical_has_empty_instance_reference(
            &assembly.paths,
            &assembly.junctions,
            &assembly.terminations,
        )
}

fn connector_declaration_has_empty_instance_reference(connector: &Connector) -> bool {
    connector
        .representation
        .as_ref()
        .is_some_and(representation_has_empty_instance_reference)
        || connector.positions.iter().any(|position| {
            position
                .representation
                .as_ref()
                .is_some_and(representation_has_empty_instance_reference)
        })
}

fn owned_physical_has_empty_instance_reference(
    paths: &[PhysicalPath],
    junctions: &[Junction],
    terminations: &[Termination],
) -> bool {
    paths.iter().any(|path| {
        physical_has_empty_instance_reference(&path.first)
            || physical_has_empty_instance_reference(&path.second)
            || path
                .representation
                .as_ref()
                .is_some_and(representation_has_empty_instance_reference)
    }) || junctions.iter().any(|junction| {
        junction
            .attachments
            .iter()
            .any(physical_has_empty_instance_reference)
            || junction
                .representation
                .as_ref()
                .is_some_and(representation_has_empty_instance_reference)
    }) || terminations.iter().any(|termination| {
        termination
            .attachments
            .iter()
            .any(physical_has_empty_instance_reference)
            || termination
                .representation
                .as_ref()
                .is_some_and(representation_has_empty_instance_reference)
    })
}

fn representation_has_empty_instance_reference(representation: &Representation) -> bool {
    match representation {
        Representation::Primitive { placement, .. } | Representation::Model { placement, .. } => {
            route_frame_has_empty_instance_reference(&placement.frame)
        }
        Representation::ModelPart(model_part) => match &model_part.model_root {
            authored::ModelRootRef::ComponentVisual { component, .. } => {
                reference_scope_is_empty(&component.scope)
            }
            authored::ModelRootRef::AssemblyModel { assembly } => {
                reference_scope_is_empty(&assembly.scope)
            }
        },
        Representation::DerivedRoute(route) => route
            .waypoints
            .iter()
            .any(|waypoint| route_frame_has_empty_instance_reference(&waypoint.frame)),
    }
}

fn route_frame_has_empty_instance_reference(frame: &authored::RouteFrameRef) -> bool {
    match frame {
        authored::RouteFrameRef::World => false,
        authored::RouteFrameRef::ComponentOrigin { component }
        | authored::RouteFrameRef::ComponentFrame { component, .. } => {
            reference_scope_is_empty(&component.scope)
        }
    }
}

fn functional_has_empty_instance_reference(reference: &FunctionalEndpointRef) -> bool {
    match reference {
        FunctionalEndpointRef::Port(reference) => reference_scope_is_empty(&reference.scope),
        FunctionalEndpointRef::Channel(reference) => reference_scope_is_empty(&reference.scope),
    }
}

fn physical_has_empty_instance_reference(reference: &PhysicalEndpointRef) -> bool {
    match reference {
        PhysicalEndpointRef::Connector(reference) => {
            connector_has_empty_instance_reference(reference)
        }
        PhysicalEndpointRef::Position(reference) => {
            position_has_empty_instance_reference(reference)
        }
        PhysicalEndpointRef::Junction(reference) => {
            owner_has_empty_instance_reference(&reference.owner)
        }
    }
}

fn connector_has_empty_instance_reference(reference: &authored::ConnectorRef) -> bool {
    owner_has_empty_instance_reference(&reference.owner)
}

fn position_has_empty_instance_reference(reference: &authored::PositionRef) -> bool {
    owner_has_empty_instance_reference(&reference.owner)
}

fn owner_has_empty_instance_reference(reference: &authored::OwnerRef) -> bool {
    reference_scope_is_empty(reference.scope())
}

fn reference_scope_is_empty(scope: &authored::ReferenceScope) -> bool {
    matches!(
        scope,
        authored::ReferenceScope::Instance(instance) if instance.is_root()
    )
}

fn axis_dimensions(axis: &str) -> &'static [Dimension] {
    use Dimension::*;
    match axis {
        "rate" => &[BitRate, SymbolRate],
        "voltage" => &[Voltage],
        "current" => &[Current],
        "power" => &[Power],
        "impedance" => &[Impedance],
        "frequency" | "bandwidth" => &[Frequency],
        "pressure" => &[Pressure],
        "flow" => &[VolumetricFlow, MassFlow],
        "temperature" => &[Temperature],
        _ => &[],
    }
}

fn termination_property_requires_nonnegative(property: &authored::QualifiedId) -> bool {
    matches!(
        property.as_str(),
        "hcdf:resistance" | "hcdf:impedance" | "hcdf:capacitance" | "hcdf:inductance"
    )
}

fn termination_property_dimensions(property: &authored::QualifiedId) -> &'static [Dimension] {
    use Dimension::*;
    match property.as_str() {
        "hcdf:rate" => &[BitRate, SymbolRate],
        "hcdf:bit-rate" => &[BitRate],
        "hcdf:symbol-rate" => &[SymbolRate],
        "hcdf:frequency" | "hcdf:bandwidth" => &[Frequency],
        "hcdf:voltage" => &[Voltage],
        "hcdf:current" => &[Current],
        "hcdf:power" => &[Power],
        "hcdf:resistance" | "hcdf:impedance" => &[Impedance],
        "hcdf:capacitance" => &[Capacitance],
        "hcdf:inductance" => &[Inductance],
        "hcdf:pressure" => &[Pressure],
        "hcdf:flow" => &[VolumetricFlow, MassFlow],
        "hcdf:volumetric-flow" => &[VolumetricFlow],
        "hcdf:mass-flow" => &[MassFlow],
        "hcdf:temperature" => &[Temperature],
        "hcdf:length" | "hcdf:wavelength" => &[Length],
        "hcdf:duration" => &[Duration],
        "hcdf:angle" => &[Angle],
        "hcdf:ratio" => &[Ratio],
        "hcdf:loss" => &[Loss],
        "hcdf:isotropic-gain" => &[IsotropicGain],
        _ => &[],
    }
}

fn normalized_selection_envelope(
    selection: &authored::SelectionQuantity,
) -> Option<(NormalizedQuantity, NormalizedQuantity)> {
    match selection {
        authored::SelectionQuantity::Nominal(quantity) => {
            let quantity = normalize_quantity(quantity).ok()?;
            Some((quantity, quantity))
        }
        authored::SelectionQuantity::Range(range) => {
            let minimum = normalize_quantity(&range.minimum).ok()?;
            let maximum = normalize_quantity(&range.maximum).ok()?;
            (minimum.dimension == maximum.dimension).then_some((minimum, maximum))
        }
    }
}

fn normalized_rf_frequency_envelope(
    selected: &NetworkSelection,
) -> Option<(NormalizedQuantity, NormalizedQuantity)> {
    let center = selected
        .rf
        .as_ref()
        .and_then(|rf| rf.channel.center_frequency())
        .or(selected.frequency.as_ref())?;
    let (mut minimum, mut maximum) = normalized_selection_envelope(center)?;
    if minimum.dimension != Dimension::Frequency {
        return None;
    }
    if let Some(bandwidth) = selected.rf.as_ref().and_then(|rf| rf.channel.bandwidth()) {
        let (_, bandwidth_maximum) = normalized_selection_envelope(bandwidth)?;
        if bandwidth_maximum.dimension != Dimension::Frequency {
            return None;
        }
        minimum.value -= bandwidth_maximum.value / 2.0;
        maximum.value += bandwidth_maximum.value / 2.0;
    }
    Some((minimum, maximum))
}

fn normalized_capability_envelope(
    range: &authored::QuantityRange,
) -> Option<(
    Dimension,
    Option<NormalizedQuantity>,
    Option<NormalizedQuantity>,
)> {
    let minimum = range
        .minimum
        .as_ref()
        .map(normalize_quantity)
        .transpose()
        .ok()?;
    let nominal = range
        .nominal
        .as_ref()
        .map(normalize_quantity)
        .transpose()
        .ok()?;
    let maximum = range
        .maximum
        .as_ref()
        .map(normalize_quantity)
        .transpose()
        .ok()?;
    let dimension = minimum
        .as_ref()
        .or(nominal.as_ref())
        .or(maximum.as_ref())?
        .dimension;
    if [minimum.as_ref(), nominal.as_ref(), maximum.as_ref()]
        .into_iter()
        .flatten()
        .any(|value| value.dimension != dimension)
    {
        return None;
    }
    let (minimum, maximum) = match (minimum, maximum) {
        (None, None) => {
            let exact = nominal?;
            (Some(exact), Some(exact))
        }
        values => values,
    };
    Some((dimension, minimum, maximum))
}
