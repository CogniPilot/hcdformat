//! Conversion from the public HCDF XML shape into the canonical connectivity contract.

use super::connectivity as canonical;
use super::connectivity_xml as xml;
use super::Hcdf;
use crate::connectivity::{
    normalize_connectivity, normalize_connectivity_with_options, NormalizationError,
    NormalizationOptions, NormalizedConnectivityGraph,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("[{code}] {path}: {reason}")]
pub struct ConnectivityConversionError {
    code: &'static str,
    path: String,
    reason: String,
}

impl ConnectivityConversionError {
    fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::coded("E_CONN_CONVERSION", path, reason)
    }

    fn coded(code: &'static str, path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HcdfConnectivityError {
    #[error(transparent)]
    Conversion(#[from] ConnectivityConversionError),
    #[error(transparent)]
    Normalization(#[from] NormalizationError),
}

impl Hcdf {
    /// Convert this parsed HCDF document into the authored canonical connectivity contract.
    pub fn to_connectivity_document(
        &self,
        document: canonical::DocumentIdentity,
    ) -> Result<canonical::ConnectivityDocument, ConnectivityConversionError> {
        if !self.include.is_empty() {
            return Err(ConnectivityConversionError::coded(
                "E_CONN_DOCUMENT_SET_REQUIRED",
                "hcdf/include",
                "connectivity conversion requires a flattened document or a document-set loader; unresolved includes cannot be omitted from a complete graph",
            ));
        }
        let scope = self.to_connectivity_scope(canonical::IncludeInstanceId::root())?;
        Ok(canonical::ConnectivityDocument {
            document,
            scopes: vec![scope],
        })
    }

    /// Convert the connectivity authored by one loaded source into its include-instance scope.
    /// Local references remain local, and explicit instance references remain relative to this scope.
    pub fn to_connectivity_scope(
        &self,
        instance: canonical::IncludeInstanceId,
    ) -> Result<canonical::ConnectivityScope, ConnectivityConversionError> {
        let mut scope = canonical::ConnectivityScope::new(instance);
        for component in &self.comp {
            scope.structural_anchors.push(canonical::StructuralAnchors {
                component: component.name.clone(),
                visuals: component
                    .visual
                    .iter()
                    .map(|visual| canonical::StructuralVisual {
                        name: visual.name.clone(),
                        model_backed: matches!(
                            &visual.appearance,
                            super::VisualAppearance::Model { .. }
                        ),
                    })
                    .collect(),
                frames: component
                    .frame
                    .iter()
                    .map(|frame| frame.name.clone())
                    .collect(),
            });
            scope.components.push(convert_component(
                component,
                &format!("comp {:?}", component.name),
            )?);
        }
        append_assemblies(
            &mut scope.assemblies,
            &self.harness,
            canonical::AssemblyKind::Harness,
            "harness",
        )?;
        append_assemblies(
            &mut scope.assemblies,
            &self.cable,
            canonical::AssemblyKind::Cable,
            "cable",
        )?;
        append_assemblies(
            &mut scope.assemblies,
            &self.plumbing,
            canonical::AssemblyKind::Plumbing,
            "plumbing",
        )?;
        append_assemblies(
            &mut scope.assemblies,
            &self.umbilical,
            canonical::AssemblyKind::Umbilical,
            "umbilical",
        )?;
        for binding in &self.binding {
            let path = format!("binding {:?}", binding.name);
            scope.bindings.push(canonical::Binding {
                name: binding.name.clone(),
                functional: functional_endpoint(
                    &binding.functional,
                    &format!("{path}/functional"),
                )?,
                physical: physical_endpoint(&binding.physical, &format!("{path}/physical"))?,
                fidelity: binding.fidelity,
            });
        }
        for mate in &self.mate {
            let path = format!("mate {:?}", mate.name);
            let mut mappings = Vec::with_capacity(mate.position_mapping.len());
            for (index, mapping) in mate.position_mapping.iter().enumerate() {
                mappings.push(canonical::PositionMapping {
                    first: position_ref(
                        &mapping.first,
                        &format!("{path}/position-mapping[{index}]/first"),
                    )?,
                    second: position_ref(
                        &mapping.second,
                        &format!("{path}/position-mapping[{index}]/second"),
                    )?,
                });
            }
            scope.mates.push(canonical::Mate {
                name: mate.name.clone(),
                first: connector_ref(&mate.first, &format!("{path}/first"))?,
                second: connector_ref(&mate.second, &format!("{path}/second"))?,
                mappings,
                fidelity: mate.fidelity,
            });
        }
        for network in &self.link {
            scope.networks.push(convert_network(
                &network.name,
                network.description.as_deref(),
                network.selected.as_ref(),
                network.configuration.as_ref(),
                &network.participant,
                canonical::NetworkStructure::Link,
                "link",
            )?);
        }
        for network in &self.bus {
            scope.networks.push(convert_network(
                &network.name,
                network.description.as_deref(),
                network.selected.as_ref(),
                network.configuration.as_ref(),
                &network.participant,
                canonical::NetworkStructure::Bus,
                "bus",
            )?);
        }

        for network in &self.chain {
            let path = format!("chain {:?}", network.name);
            scope.networks.push(convert_network(
                &network.name,
                network.description.as_deref(),
                network.selected.as_ref(),
                network.configuration.as_ref(),
                &network.participant,
                canonical::NetworkStructure::Chain(path_topology(
                    &network.hop,
                    &network.leg,
                    &path,
                )?),
                "chain",
            )?);
        }
        for network in &self.star {
            let path = format!("star {:?}", network.name);
            scope.networks.push(convert_network(
                &network.name,
                network.description.as_deref(),
                network.selected.as_ref(),
                network.configuration.as_ref(),
                &network.participant,
                canonical::NetworkStructure::Star(canonical::StarTopology {
                    coordinator: participant_ref(
                        &network.coordinator.participant,
                        &format!("{path}/coordinator/participant-ref"),
                    )?,
                }),
                "star",
            )?);
        }

        for network in &self.ring {
            let path = format!("ring {:?}", network.name);
            scope.networks.push(convert_network(
                &network.name,
                network.description.as_deref(),
                network.selected.as_ref(),
                network.configuration.as_ref(),
                &network.participant,
                canonical::NetworkStructure::Ring(path_topology(
                    &network.hop,
                    &network.leg,
                    &path,
                )?),
                "ring",
            )?);
        }
        for network in &self.mesh {
            scope.networks.push(convert_network(
                &network.name,
                network.description.as_deref(),
                network.selected.as_ref(),
                network.configuration.as_ref(),
                &network.participant,
                canonical::NetworkStructure::Mesh,
                "mesh",
            )?);
        }

        for network in &self.tree {
            let path = format!("tree {:?}", network.name);
            let topology = path_topology(&network.hop, &network.leg, &path)?;
            scope.networks.push(convert_network(
                &network.name,
                network.description.as_deref(),
                network.selected.as_ref(),
                network.configuration.as_ref(),
                &network.participant,
                canonical::NetworkStructure::Tree(canonical::TreeTopology {
                    root: hop_ref(&network.root.hop, &format!("{path}/root/hop-ref"))?,
                    hops: topology.hops,
                    legs: topology.legs,
                }),
                "tree",
            )?);
        }
        validate_representation_selectors(self, &scope)?;
        Ok(scope)
    }

    /// Convert and normalize this document through the hcdformat-owned canonical graph pipeline.
    pub fn to_normalized_connectivity(
        &self,
        document: canonical::DocumentIdentity,
    ) -> Result<NormalizedConnectivityGraph, HcdfConnectivityError> {
        let authored = self.to_connectivity_document(document)?;
        Ok(normalize_connectivity(&authored)?)
    }

    /// Convert and normalize this document with explicit connectivity validation options.
    pub fn to_normalized_connectivity_with_options(
        &self,
        document: canonical::DocumentIdentity,
        options: NormalizationOptions,
    ) -> Result<NormalizedConnectivityGraph, HcdfConnectivityError> {
        let authored = self.to_connectivity_document(document)?;
        Ok(normalize_connectivity_with_options(&authored, options)?)
    }
}

fn convert_component(
    component: &super::Comp,
    path: &str,
) -> Result<canonical::ComponentConnectivity, ConnectivityConversionError> {
    let mut ports = Vec::with_capacity(component.port.len());
    for port in &component.port {
        let port_path = format!("{path}/port {:?}", port.name);
        let mut channels = Vec::with_capacity(port.channel.len());
        for channel in &port.channel {
            let channel_path = format!("{port_path}/channel {:?}", channel.name);
            channels.push(canonical::Channel {
                name: channel.name.clone(),
                role: channel
                    .role
                    .as_deref()
                    .map(|role| qualified(role, &format!("{channel_path}/@role")))
                    .transpose()?,
                local_group: channel.local_group.clone(),
                capabilities: capabilities(channel.capabilities.as_ref(), &channel_path)?,
            });
        }
        ports.push(canonical::Port {
            name: port.name.clone(),
            capabilities: capabilities(port.capabilities.as_ref(), &port_path)?,
            channels,
        });
    }

    let mut connectors = Vec::with_capacity(component.connector.len());
    for connector in &component.connector {
        connectors.push(convert_connector(
            connector,
            &format!("{path}/connector {:?}", connector.name),
        )?);
    }

    let mut antennas = Vec::with_capacity(component.antenna.len());
    for antenna in &component.antenna {
        let antenna_path = format!("{path}/antenna {:?}", antenna.name);
        antennas.push(canonical::Antenna {
            name: antenna.name.clone(),
            conducted_port: antenna
                .conducted_port
                .as_ref()
                .map(|reference| port_ref(reference, &format!("{antenna_path}/conducted-port")))
                .transpose()?,
            radiated_port: port_ref(
                &antenna.radiated_port,
                &format!("{antenna_path}/radiated-port"),
            )?,
            representation: optional_representation(
                antenna.representation.as_ref(),
                &antenna_path,
            )?,
        });
    }

    let mut functions = Vec::new();
    append_functions(
        &mut functions,
        &component.switch,
        canonical::FunctionKind::Switch,
        path,
        "switch",
    )?;
    append_functions(
        &mut functions,
        &component.bridge,
        canonical::FunctionKind::Bridge,
        path,
        "bridge",
    )?;
    append_functions(
        &mut functions,
        &component.converter,
        canonical::FunctionKind::Converter,
        path,
        "converter",
    )?;
    append_functions(
        &mut functions,
        &component.transceiver,
        canonical::FunctionKind::Transceiver,
        path,
        "transceiver",
    )?;
    append_functions(
        &mut functions,
        &component.radio,
        canonical::FunctionKind::Radio,
        path,
        "radio",
    )?;

    let mut paths = Vec::new();
    append_paths(
        &mut paths,
        &component.wire,
        canonical::PathKind::Wire,
        path,
        "wire",
    )?;
    append_paths(
        &mut paths,
        &component.conductor,
        canonical::PathKind::Conductor,
        path,
        "conductor",
    )?;
    append_paths(
        &mut paths,
        &component.cable_member,
        canonical::PathKind::CableMember,
        path,
        "cable-member",
    )?;
    append_paths(
        &mut paths,
        &component.fiber,
        canonical::PathKind::Fiber,
        path,
        "fiber",
    )?;
    append_paths(
        &mut paths,
        &component.coax,
        canonical::PathKind::Coax,
        path,
        "coax",
    )?;
    append_paths(
        &mut paths,
        &component.waveguide,
        canonical::PathKind::Waveguide,
        path,
        "waveguide",
    )?;
    append_paths(
        &mut paths,
        &component.feed,
        canonical::PathKind::Feed,
        path,
        "feed",
    )?;
    append_paths(
        &mut paths,
        &component.hose,
        canonical::PathKind::Hose,
        path,
        "hose",
    )?;
    append_paths(
        &mut paths,
        &component.pipe,
        canonical::PathKind::Pipe,
        path,
        "pipe",
    )?;
    append_paths(
        &mut paths,
        &component.passage,
        canonical::PathKind::Passage,
        path,
        "passage",
    )?;

    let mut junctions = Vec::new();
    append_junctions(
        &mut junctions,
        &component.splice,
        canonical::JunctionKind::Splice,
        path,
        "splice",
    )?;
    append_junctions(
        &mut junctions,
        &component.tee,
        canonical::JunctionKind::Tee,
        path,
        "tee",
    )?;
    append_junctions(
        &mut junctions,
        &component.manifold,
        canonical::JunctionKind::Manifold,
        path,
        "manifold",
    )?;
    append_junctions(
        &mut junctions,
        &component.busbar,
        canonical::JunctionKind::Busbar,
        path,
        "busbar",
    )?;
    append_junctions(
        &mut junctions,
        &component.optical_splitter,
        canonical::JunctionKind::OpticalSplitter,
        path,
        "optical-splitter",
    )?;

    let mut terminations = Vec::with_capacity(component.termination.len());
    for termination in &component.termination {
        terminations.push(convert_termination(
            termination,
            &format!("{path}/termination {:?}", termination.name),
        )?);
    }

    Ok(canonical::ComponentConnectivity {
        component: component.name.clone(),
        ports,
        connectors,
        antennas,
        functions,
        paths,
        junctions,
        terminations,
    })
}

fn append_assemblies(
    output: &mut Vec<canonical::PhysicalAssembly>,
    assemblies: &[xml::Assembly],
    kind: canonical::AssemblyKind,
    tag: &str,
) -> Result<(), ConnectivityConversionError> {
    for assembly in assemblies {
        let path = format!("{tag} {:?}", assembly.name);
        let mut connectors = Vec::with_capacity(assembly.connector.len());
        for connector in &assembly.connector {
            connectors.push(convert_connector(
                connector,
                &format!("{path}/connector {:?}", connector.name),
            )?);
        }
        let mut paths = Vec::new();
        append_paths(
            &mut paths,
            &assembly.wire,
            canonical::PathKind::Wire,
            &path,
            "wire",
        )?;
        append_paths(
            &mut paths,
            &assembly.conductor,
            canonical::PathKind::Conductor,
            &path,
            "conductor",
        )?;
        append_paths(
            &mut paths,
            &assembly.cable_member,
            canonical::PathKind::CableMember,
            &path,
            "cable-member",
        )?;
        append_paths(
            &mut paths,
            &assembly.fiber,
            canonical::PathKind::Fiber,
            &path,
            "fiber",
        )?;
        append_paths(
            &mut paths,
            &assembly.coax,
            canonical::PathKind::Coax,
            &path,
            "coax",
        )?;
        append_paths(
            &mut paths,
            &assembly.waveguide,
            canonical::PathKind::Waveguide,
            &path,
            "waveguide",
        )?;
        append_paths(
            &mut paths,
            &assembly.feed,
            canonical::PathKind::Feed,
            &path,
            "feed",
        )?;
        append_paths(
            &mut paths,
            &assembly.hose,
            canonical::PathKind::Hose,
            &path,
            "hose",
        )?;
        append_paths(
            &mut paths,
            &assembly.pipe,
            canonical::PathKind::Pipe,
            &path,
            "pipe",
        )?;
        append_paths(
            &mut paths,
            &assembly.passage,
            canonical::PathKind::Passage,
            &path,
            "passage",
        )?;
        let mut junctions = Vec::new();
        append_junctions(
            &mut junctions,
            &assembly.splice,
            canonical::JunctionKind::Splice,
            &path,
            "splice",
        )?;
        append_junctions(
            &mut junctions,
            &assembly.tee,
            canonical::JunctionKind::Tee,
            &path,
            "tee",
        )?;
        append_junctions(
            &mut junctions,
            &assembly.manifold,
            canonical::JunctionKind::Manifold,
            &path,
            "manifold",
        )?;
        append_junctions(
            &mut junctions,
            &assembly.busbar,
            canonical::JunctionKind::Busbar,
            &path,
            "busbar",
        )?;
        append_junctions(
            &mut junctions,
            &assembly.optical_splitter,
            canonical::JunctionKind::OpticalSplitter,
            &path,
            "optical-splitter",
        )?;
        let mut terminations = Vec::with_capacity(assembly.termination.len());
        for termination in &assembly.termination {
            terminations.push(convert_termination(
                termination,
                &format!("{path}/termination {:?}", termination.name),
            )?);
        }
        output.push(canonical::PhysicalAssembly {
            name: assembly.name.clone(),
            connectors,
            kind,
            representation: optional_representation(assembly.representation.as_ref(), &path)?,
            paths,
            junctions,
            terminations,
        });
    }
    Ok(())
}

fn convert_connector(
    connector: &xml::Connector,
    path: &str,
) -> Result<canonical::Connector, ConnectivityConversionError> {
    let mut positions = Vec::new();
    append_positions(
        &mut positions,
        &connector.pin,
        canonical::PositionKind::Pin,
        path,
        "pin",
    )?;
    append_positions(
        &mut positions,
        &connector.socket,
        canonical::PositionKind::Socket,
        path,
        "socket",
    )?;
    append_positions(
        &mut positions,
        &connector.contact,
        canonical::PositionKind::Contact,
        path,
        "contact",
    )?;
    append_positions(
        &mut positions,
        &connector.fiber,
        canonical::PositionKind::Fiber,
        path,
        "fiber",
    )?;
    append_positions(
        &mut positions,
        &connector.passage,
        canonical::PositionKind::Passage,
        path,
        "passage",
    )?;
    append_positions(
        &mut positions,
        &connector.feed,
        canonical::PositionKind::Feed,
        path,
        "feed",
    )?;
    append_positions(
        &mut positions,
        &connector.waveguide_opening,
        canonical::PositionKind::WaveguideOpening,
        path,
        "waveguide-opening",
    )?;
    Ok(canonical::Connector {
        name: connector.name.clone(),
        family: connector
            .family
            .as_deref()
            .map(|id| qualified(id, &format!("{path}/@family")))
            .transpose()?,
        positions,
        representation: optional_representation(connector.representation.as_ref(), path)?,
    })
}

fn append_positions(
    output: &mut Vec<canonical::Position>,
    positions: &[xml::Position],
    kind: canonical::PositionKind,
    owner_path: &str,
    tag: &str,
) -> Result<(), ConnectivityConversionError> {
    for position in positions {
        let path = format!("{owner_path}/{tag} {:?}", position.name);
        output.push(canonical::Position {
            name: position.name.clone(),
            kind,
            role: position
                .role
                .as_deref()
                .map(|role| qualified(role, &format!("{path}/@role")))
                .transpose()?,
            local_group: position.local_group.clone(),
            representation: optional_representation(position.representation.as_ref(), &path)?,
        });
    }
    Ok(())
}

fn append_functions(
    output: &mut Vec<canonical::ConnectivityFunction>,
    functions: &[xml::Function],
    kind: canonical::FunctionKind,
    owner_path: &str,
    tag: &str,
) -> Result<(), ConnectivityConversionError> {
    for function in functions {
        let path = format!("{owner_path}/{tag} {:?}", function.name);
        output.push(canonical::ConnectivityFunction {
            name: function.name.clone(),
            kind,
            inputs: endpoint_list(&function.input, &path, "input")?,
            outputs: endpoint_list(&function.output, &path, "output")?,
            bidirectional: endpoint_list(&function.bidirectional, &path, "bidirectional")?,
        });
    }
    Ok(())
}

fn endpoint_list(
    input: &[xml::FunctionalEndpointRef],
    owner_path: &str,
    tag: &str,
) -> Result<Vec<canonical::FunctionalEndpointRef>, ConnectivityConversionError> {
    input
        .iter()
        .enumerate()
        .map(|(index, endpoint)| {
            functional_endpoint(endpoint, &format!("{owner_path}/{tag}[{index}]"))
        })
        .collect()
}

fn append_paths(
    output: &mut Vec<canonical::PhysicalPath>,
    paths: &[xml::Path],
    kind: canonical::PathKind,
    owner_path: &str,
    tag: &str,
) -> Result<(), ConnectivityConversionError> {
    for path_value in paths {
        let path = format!("{owner_path}/{tag} {:?}", path_value.name);
        output.push(canonical::PhysicalPath {
            name: path_value.name.clone(),
            kind,
            first: physical_endpoint(&path_value.first, &format!("{path}/first"))?,
            second: physical_endpoint(&path_value.second, &format!("{path}/second"))?,
            fidelity: path_value.fidelity,
            role: path_value
                .role
                .as_deref()
                .map(|role| qualified(role, &format!("{path}/@role")))
                .transpose()?,
            local_group: path_value.local_group.clone(),
            representation: optional_representation(path_value.representation.as_ref(), &path)?,
        });
    }
    Ok(())
}

fn append_junctions(
    output: &mut Vec<canonical::Junction>,
    junctions: &[xml::Junction],
    kind: canonical::JunctionKind,
    owner_path: &str,
    tag: &str,
) -> Result<(), ConnectivityConversionError> {
    for junction in junctions {
        let path = format!("{owner_path}/{tag} {:?}", junction.name);
        let attachments = junction
            .attachment
            .iter()
            .enumerate()
            .map(|(index, endpoint)| {
                physical_endpoint(endpoint, &format!("{path}/attachment[{index}]"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        output.push(canonical::Junction {
            name: junction.name.clone(),
            kind,
            fidelity: junction.fidelity,
            attachments,
            representation: optional_representation(junction.representation.as_ref(), &path)?,
        });
    }
    Ok(())
}

fn convert_termination(
    termination: &xml::Termination,
    path: &str,
) -> Result<canonical::Termination, ConnectivityConversionError> {
    let attachments = termination
        .attachment
        .iter()
        .enumerate()
        .map(|(index, attachment)| {
            physical_endpoint(attachment, &format!("{path}/attachment[{index}]"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let quantities = termination
        .quantity
        .iter()
        .enumerate()
        .map(|(index, value)| {
            Ok(canonical::NamedQuantity {
                property: qualified(
                    &value.property,
                    &format!("{path}/quantity[{index}]/@property"),
                )?,
                quantity: canonical::Quantity {
                    value: value.value,
                    unit: value.unit.clone(),
                },
            })
        })
        .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
    Ok(canonical::Termination {
        name: termination.name.clone(),
        kind: qualified(&termination.kind, &format!("{path}/@kind"))?,
        mounting: termination.mounting,
        attachments,
        quantities,
        fidelity: termination.fidelity,
        profile: termination
            .profile
            .as_deref()
            .map(|id| qualified(id, &format!("{path}/@profile")))
            .transpose()?,
        representation: optional_representation(termination.representation.as_ref(), path)?,
    })
}

fn convert_network(
    name: &str,
    description: Option<&str>,
    selected_configuration: Option<&xml::NetworkSelection>,
    configuration: Option<&xml::NetworkConfiguration>,
    participant: &[xml::Participant],
    structure: canonical::NetworkStructure,
    tag: &str,
) -> Result<canonical::Network, ConnectivityConversionError> {
    let path = format!("{tag} {name:?}");
    let participants = participant
        .iter()
        .map(|participant| {
            let participant_path = format!("{path}/participant {:?}", participant.name);
            Ok(canonical::Participant {
                name: participant.name.clone(),
                endpoint: functional_endpoint(
                    &participant.endpoint,
                    &format!("{participant_path}/endpoint"),
                )?,
                role: participant
                    .role
                    .as_deref()
                    .map(|role| qualified(role, &format!("{participant_path}/@role")))
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
    let Some(selected_configuration) = selected_configuration else {
        return Err(ConnectivityConversionError::coded(
            "E_CONN_SELECTED_REQUIRED",
            format!("{path}/selected"),
            "network requires an explicit selected configuration",
        ));
    };
    Ok(canonical::Network {
        name: name.to_owned(),
        structure,
        description: description.map(str::to_owned),
        selected: network_selection(selected_configuration, &path)?,
        participants,
        configuration: network_configuration(configuration, &path)?,
    })
}

fn network_configuration(
    value: Option<&xml::NetworkConfiguration>,
    path: &str,
) -> Result<canonical::NetworkConfiguration, ConnectivityConversionError> {
    let Some(value) = value else {
        return Ok(canonical::NetworkConfiguration::default());
    };

    let gptp_domains = value
        .gptp_domain
        .iter()
        .map(|domain| {
            let domain_path = format!("{path}/configuration/gptp-domain {:?}", domain.name);
            let clocks = domain
                .clock
                .iter()
                .map(|clock| {
                    let clock_path = format!("{domain_path}/clock {:?}", clock.name);
                    Ok(canonical::GptpClock {
                        name: clock.name.clone(),
                        participant: participant_ref(
                            &clock.participant,
                            &format!("{clock_path}/participant-ref"),
                        )?,
                        kind: clock.kind,
                        gm_capable: clock.gm_capable,
                        priority1: clock.priority1,
                        priority2: clock.priority2,
                        clock_class: clock.clock_class,
                        clock_accuracy: clock.clock_accuracy,
                    })
                })
                .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
            Ok(canonical::GptpDomain {
                name: domain.name.clone(),
                number: domain.number,
                port_defaults: domain.port_defaults.as_ref().map(|defaults| {
                    canonical::GptpPortDefaults {
                        log_sync_interval: defaults.log_sync_interval,
                        log_announce_interval: defaults.log_announce_interval,
                        log_pdelay_req_interval: defaults.log_pdelay_req_interval,
                        announce_receipt_timeout: defaults.announce_receipt_timeout,
                        neighbor_prop_delay_threshold_ns: defaults.neighbor_prop_delay_threshold_ns,
                    }
                }),
                clocks,
            })
        })
        .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;

    let traffic_classes = value
        .traffic_class
        .iter()
        .map(|class| {
            let class_path = format!("{path}/configuration/traffic-class {:?}", class.name);
            let mut pcp = BTreeSet::new();
            for (index, value) in class.pcp.iter().enumerate() {
                if !pcp.insert(value.value) {
                    return Err(ConnectivityConversionError::coded(
                        "E_CONN_DUPLICATE_PCP",
                        format!("{class_path}/pcp[{index}]"),
                        format!("duplicate PCP {}", value.value),
                    ));
                }
            }
            Ok(canonical::TrafficClass {
                name: class.name.clone(),
                number: class.number,
                description: class.description.clone(),
                preemption: class.preemption,
                pcp,
            })
        })
        .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;

    let gate_schedules = value
        .gate_schedule
        .iter()
        .map(|schedule| {
            let schedule_path = format!("{path}/configuration/gate-schedule {:?}", schedule.name);
            let entries = schedule
                .gate
                .iter()
                .enumerate()
                .map(|(gate_index, gate)| {
                    let gate_path = format!("{schedule_path}/gate[{gate_index}]");
                    let mut open = BTreeSet::new();
                    for (reference_index, reference) in gate.open.traffic_class.iter().enumerate() {
                        let reference = traffic_class_ref(
                            reference,
                            &format!("{gate_path}/open/traffic-class-ref[{reference_index}]"),
                        )?;
                        if !open.insert(reference.clone()) {
                            return Err(ConnectivityConversionError::coded(
                                "E_CONN_DUPLICATE_TRAFFIC_CLASS_REF",
                                gate_path.clone(),
                                format!(
                                    "duplicate open traffic-class reference {:?}",
                                    reference.traffic_class
                                ),
                            ));
                        }
                    }
                    Ok(canonical::GateControlEntry {
                        duration_ns: gate.duration_ns,
                        open,
                    })
                })
                .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
            Ok(canonical::GateSchedule {
                name: schedule.name.clone(),
                cycle_time_ns: schedule.cycle_time_ns,
                entries,
            })
        })
        .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;

    let schedule_assignments = value
        .schedule_assignment
        .iter()
        .map(|assignment| {
            let assignment_path = format!(
                "{path}/configuration/schedule-assignment {:?}",
                assignment.name
            );
            let targets = assignment
                .target
                .iter()
                .enumerate()
                .map(|(index, target)| {
                    participant_ref(
                        &target.participant,
                        &format!("{assignment_path}/target[{index}]/participant-ref"),
                    )
                })
                .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
            Ok(canonical::ScheduleAssignment {
                name: assignment.name.clone(),
                schedule: schedule_ref(
                    &assignment.schedule,
                    &format!("{assignment_path}/schedule-ref"),
                )?,
                targets,
            })
        })
        .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;

    let plca = value
        .plca
        .as_ref()
        .map(|plca| {
            let nodes = plca
                .node
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    Ok(canonical::PlcaNode {
                        node_id: node.id,
                        burst_count: node.burst_count,
                        burst_timer_bit_times: node.burst_timer_bit_times,
                        participant: participant_ref(
                            &node.participant,
                            &format!("{path}/configuration/plca/node[{index}]/participant-ref"),
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
            Ok(canonical::PlcaConfiguration {
                max_node_id: plca.max_node_id,
                to_timer_bit_times: plca.to_timer_bit_times,
                nodes,
            })
        })
        .transpose()?;

    let macsec = value
        .macsec
        .as_ref()
        .map(|macsec| {
            let policies = macsec
                .policy
                .iter()
                .enumerate()
                .map(|(index, policy)| {
                    let policy_path = format!("{path}/configuration/macsec/policy[{index}]");
                    Ok(canonical::MacsecPolicyDefinition {
                        name: policy.name.clone(),
                        enforcement: policy.enforcement,
                        cipher: policy
                            .cipher
                            .as_deref()
                            .map(|value| qualified(value, &format!("{policy_path}/@cipher")))
                            .transpose()?,
                        key_agreement: policy
                            .key_agreement
                            .as_deref()
                            .map(|value| qualified(value, &format!("{policy_path}/@key-agreement")))
                            .transpose()?,
                        confidentiality_offset: policy.confidentiality_offset,
                        rekey_interval_ns: policy.rekey_interval_ns,
                        credential_store_ref: policy.credential_store_ref.clone(),
                    })
                })
                .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
            let overrides = macsec
                .override_
                .iter()
                .enumerate()
                .map(|(index, override_)| {
                    let override_path = format!("{path}/configuration/macsec/override[{index}]");
                    let target = match &override_.target.segment {
                        xml::TopologySegmentChoice::Network(reference) => {
                            canonical::TopologySegmentRef::Network(network_ref(
                                reference,
                                &format!("{override_path}/target/network-ref"),
                            )?)
                        }
                        xml::TopologySegmentChoice::Leg(reference) => {
                            canonical::TopologySegmentRef::Leg(leg_ref(
                                reference,
                                &format!("{override_path}/target/leg-ref"),
                            )?)
                        }
                    };
                    Ok(canonical::MacsecOverride {
                        target,
                        policy: macsec_policy_ref(
                            &override_.policy,
                            &format!("{override_path}/macsec-policy-ref"),
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
            Ok(canonical::MacsecConfiguration {
                policies,
                default_policy: macsec
                    .default_policy
                    .as_ref()
                    .map(|default| {
                        macsec_policy_ref(
                            &default.policy,
                            &format!(
                                "{path}/configuration/macsec/default-policy/macsec-policy-ref"
                            ),
                        )
                    })
                    .transpose()?,
                overrides,
            })
        })
        .transpose()?;

    let eee = value
        .eee
        .as_ref()
        .map(|eee| {
            let overrides = eee
                .override_
                .iter()
                .enumerate()
                .map(|(index, override_)| {
                    Ok(canonical::EeeOverride {
                        participant: participant_ref(
                            &override_.participant,
                            &format!("{path}/configuration/eee/override[{index}]/participant-ref"),
                        )?,
                        mode: override_.mode,
                    })
                })
                .collect::<Result<Vec<_>, ConnectivityConversionError>>()?;
            Ok(canonical::EeeConfiguration {
                default_mode: eee.default_mode,
                overrides,
            })
        })
        .transpose()?;

    Ok(canonical::NetworkConfiguration {
        gptp_domains,
        traffic_classes,
        gate_schedules,
        schedule_assignments,
        plca,
        macsec,
        eee,
    })
}

fn path_topology(
    hops: &[xml::Hop],
    legs: &[xml::Leg],
    path: &str,
) -> Result<canonical::PathTopology, ConnectivityConversionError> {
    let hops = hops
        .iter()
        .map(|hop| convert_hop(hop, &format!("{path}/hop {:?}", hop.name)))
        .collect::<Result<Vec<_>, _>>()?;
    let legs = legs
        .iter()
        .map(|leg| convert_leg(leg, &format!("{path}/leg {:?}", leg.name)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(canonical::PathTopology { hops, legs })
}

fn convert_hop(hop: &xml::Hop, path: &str) -> Result<canonical::Hop, ConnectivityConversionError> {
    let owner = match &hop.owner.owner {
        xml::HopOwnerChoice::Component(reference) => canonical::HopOwnerRef::Component(
            component_ref(reference, &format!("{path}/owner/component-ref"))?,
        ),
        xml::HopOwnerChoice::Function(reference) => canonical::HopOwnerRef::Function(
            connectivity_function_ref(reference, &format!("{path}/owner/function-ref"))?,
        ),
    };
    Ok(canonical::Hop {
        name: hop.name.clone(),
        owner,
        role: hop
            .role
            .as_deref()
            .map(|role| qualified(role, &format!("{path}/@role")))
            .transpose()?,
        description: hop.description.clone(),
        processing_delay_ns: hop.processing_delay_ns,
    })
}

fn convert_leg(leg: &xml::Leg, path: &str) -> Result<canonical::Leg, ConnectivityConversionError> {
    Ok(canonical::Leg {
        name: leg.name.clone(),
        from: leg_end(&leg.from, &format!("{path}/from"))?,
        to: leg_end(&leg.to, &format!("{path}/to"))?,
    })
}

fn leg_end(
    end: &xml::LegEnd,
    path: &str,
) -> Result<canonical::LegEnd, ConnectivityConversionError> {
    Ok(canonical::LegEnd {
        hop: hop_ref(&end.hop, &format!("{path}/hop-ref"))?,
        participant: participant_ref(&end.participant, &format!("{path}/participant-ref"))?,
    })
}

fn capabilities(
    value: Option<&xml::Capabilities>,
    path: &str,
) -> Result<canonical::Capabilities, ConnectivityConversionError> {
    let Some(value) = value else {
        return Ok(canonical::Capabilities::default());
    };
    let profiles = qualified_set(
        value.profile.iter().map(|profile| profile.id.as_str()),
        &format!("{path}/capabilities/profile"),
    )?;
    Ok(canonical::Capabilities {
        purposes: value.purpose.iter().map(|item| item.value).collect(),
        carriers: value.carrier.iter().map(|item| item.value).collect(),
        profiles,
        limits: canonical::CapabilityLimits {
            rate: quantity_range(value.rate.as_ref()),
            voltage: quantity_range(value.voltage.as_ref()),
            current: quantity_range(value.current.as_ref()),
            power: quantity_range(value.power.as_ref()),
            impedance: quantity_range(value.impedance.as_ref()),
            frequency: quantity_range(value.frequency.as_ref()),
            bandwidth: quantity_range(value.bandwidth.as_ref()),
            pressure: quantity_range(value.pressure.as_ref()),
            flow: quantity_range(value.flow.as_ref()),
            temperature: quantity_range(value.temperature.as_ref()),
        },
    })
}

fn network_selection(
    value: &xml::NetworkSelection,
    path: &str,
) -> Result<canonical::NetworkSelection, ConnectivityConversionError> {
    Ok(canonical::NetworkSelection {
        purpose: value.purpose,
        carrier: value.carrier,
        profiles: qualified_set(
            value.profile.iter().map(|profile| profile.id.as_str()),
            &format!("{path}/selected/profile"),
        )?,
        rate: value.rate.as_ref().map(selection_quantity),
        voltage: value.voltage.as_ref().map(selection_quantity),
        current: value.current.as_ref().map(selection_quantity),
        power: value.power.as_ref().map(selection_quantity),
        impedance: value.impedance.as_ref().map(selection_quantity),
        frequency: value.frequency.as_ref().map(selection_quantity),
        rf: value.rf.as_ref().map(rf_selection),
        pressure: value.pressure.as_ref().map(selection_quantity),
        flow: value.flow.as_ref().map(selection_quantity),
        temperature: value.temperature.as_ref().map(selection_quantity),
    })
}

fn rf_selection(value: &xml::RfSelection) -> canonical::RfSelection {
    let channel = match &value.channel.selection {
        xml::RfChannelSelectionChoice::Numbered(channel) => {
            canonical::RfChannelSelection::Numbered {
                number: channel.number,
                center_frequency: channel.center_frequency.as_ref().map(selection_quantity),
                bandwidth: channel.bandwidth.as_ref().map(selection_quantity),
            }
        }
        xml::RfChannelSelectionChoice::FrequencyDefined(channel) => {
            canonical::RfChannelSelection::FrequencyDefined {
                center_frequency: selection_quantity(&channel.center_frequency),
                bandwidth: channel.bandwidth.as_ref().map(selection_quantity),
            }
        }
    };
    canonical::RfSelection { channel }
}

fn selection_quantity(value: &xml::SelectionQuantity) -> canonical::SelectionQuantity {
    match &value.selection {
        xml::SelectionQuantityChoice::Nominal(value) => {
            canonical::SelectionQuantity::Nominal(quantity(value))
        }
        xml::SelectionQuantityChoice::Range(value) => {
            canonical::SelectionQuantity::Range(canonical::SelectionRange {
                minimum: canonical::Quantity {
                    value: value.minimum,
                    unit: value.unit.clone(),
                },
                maximum: canonical::Quantity {
                    value: value.maximum,
                    unit: value.unit.clone(),
                },
                nominal: value.nominal.map(|number| canonical::Quantity {
                    value: number,
                    unit: value.unit.clone(),
                }),
            })
        }
    }
}

fn quantity(value: &xml::Quantity) -> canonical::Quantity {
    canonical::Quantity {
        value: value.value,
        unit: value.unit.clone(),
    }
}

fn quantity_range(value: Option<&xml::QuantityRange>) -> Option<canonical::QuantityRange> {
    value.map(|value| canonical::QuantityRange {
        minimum: value.minimum.map(|number| canonical::Quantity {
            value: number,
            unit: value.unit.clone(),
        }),
        maximum: value.maximum.map(|number| canonical::Quantity {
            value: number,
            unit: value.unit.clone(),
        }),
        nominal: value.nominal.map(|number| canonical::Quantity {
            value: number,
            unit: value.unit.clone(),
        }),
    })
}

fn qualified_set<'a>(
    values: impl IntoIterator<Item = &'a str>,
    path: &str,
) -> Result<BTreeSet<canonical::QualifiedId>, ConnectivityConversionError> {
    let mut result = BTreeSet::new();
    for (index, value) in values.into_iter().enumerate() {
        let item_path = format!("{path}[{index}]/@id");
        let value = qualified(value, &item_path)?;
        if !result.insert(value.clone()) {
            return Err(ConnectivityConversionError::coded(
                "E_CONN_DUPLICATE_PROFILE",
                item_path,
                format!("duplicate profile declaration {value}"),
            ));
        }
    }
    Ok(result)
}

fn qualified(
    value: &str,
    path: &str,
) -> Result<canonical::QualifiedId, ConnectivityConversionError> {
    canonical::QualifiedId::new(value).map_err(|error| {
        ConnectivityConversionError::coded(
            "E_CONN_QUALIFIED_ID",
            path,
            format!("invalid qualified identifier: {error}"),
        )
    })
}

fn scope(
    value: Option<&xml::InstanceRef>,
    path: &str,
) -> Result<canonical::ReferenceScope, ConnectivityConversionError> {
    let Some(value) = value else {
        return Ok(canonical::ReferenceScope::Local);
    };
    if value.segment.is_empty() {
        return Err(ConnectivityConversionError::new(
            path,
            "an explicit instance reference must contain at least one segment",
        ));
    }
    let mut instance = canonical::IncludeInstanceId::root();
    for segment in &value.segment {
        match segment.name.as_deref() {
            None => {
                instance = instance.unnamed_child(segment.occurrence);
            }
            Some("") => {
                return Err(ConnectivityConversionError::new(
                    path,
                    "named instance segment names must not be empty",
                ));
            }
            Some(name) => {
                instance = instance.named_child(name, segment.occurrence);
            }
        }
    }
    Ok(canonical::ReferenceScope::Instance(instance))
}

fn owner(
    owner: &xml::PhysicalOwnerChoice,
    path: &str,
) -> Result<canonical::OwnerRef, ConnectivityConversionError> {
    match owner {
        xml::PhysicalOwnerChoice::Component(reference) => {
            let reference_path = format!("{path}/component-ref");
            nonempty_reference(
                &reference.component,
                &format!("{reference_path}/@component"),
                "physical owner component",
            )?;
            Ok(canonical::OwnerRef::Component(canonical::ComponentRef {
                scope: scope(reference.instance.as_ref(), &reference_path)?,
                component: reference.component.clone(),
            }))
        }
        xml::PhysicalOwnerChoice::Assembly(reference) => {
            let reference_path = format!("{path}/assembly-ref");
            nonempty_reference(
                &reference.assembly,
                &format!("{reference_path}/@assembly"),
                "physical owner assembly",
            )?;
            Ok(canonical::OwnerRef::Assembly(canonical::AssemblyRef {
                scope: scope(reference.instance.as_ref(), &reference_path)?,
                assembly: reference.assembly.clone(),
            }))
        }
    }
}

fn component_ref(
    value: &xml::ComponentRef,
    path: &str,
) -> Result<canonical::ComponentRef, ConnectivityConversionError> {
    Ok(canonical::ComponentRef {
        scope: scope(value.instance.as_ref(), path)?,
        component: value.component.clone(),
    })
}

fn connectivity_function_ref(
    value: &xml::ConnectivityFunctionRef,
    path: &str,
) -> Result<canonical::ConnectivityFunctionRef, ConnectivityConversionError> {
    Ok(canonical::ConnectivityFunctionRef {
        component: canonical::ComponentRef {
            scope: scope(value.instance.as_ref(), path)?,
            component: value.component.clone(),
        },
        function: value.function.clone(),
    })
}

fn participant_ref(
    value: &xml::ParticipantRef,
    path: &str,
) -> Result<canonical::ParticipantRef, ConnectivityConversionError> {
    Ok(canonical::ParticipantRef {
        network: canonical::NetworkRef {
            scope: scope(value.instance.as_ref(), path)?,
            network: value.network.clone(),
        },
        participant: value.participant.clone(),
    })
}

fn network_ref(
    value: &xml::NetworkRef,
    path: &str,
) -> Result<canonical::NetworkRef, ConnectivityConversionError> {
    Ok(canonical::NetworkRef {
        scope: scope(value.instance.as_ref(), path)?,
        network: value.network.clone(),
    })
}

fn leg_ref(
    value: &xml::LegRef,
    path: &str,
) -> Result<canonical::LegRef, ConnectivityConversionError> {
    Ok(canonical::LegRef {
        network: canonical::NetworkRef {
            scope: scope(value.instance.as_ref(), path)?,
            network: value.network.clone(),
        },
        leg: value.leg.clone(),
    })
}

fn macsec_policy_ref(
    value: &xml::MacsecPolicyRef,
    path: &str,
) -> Result<canonical::MacsecPolicyRef, ConnectivityConversionError> {
    Ok(canonical::MacsecPolicyRef {
        network: canonical::NetworkRef {
            scope: scope(value.instance.as_ref(), path)?,
            network: value.network.clone(),
        },
        policy: value.policy.clone(),
    })
}

fn traffic_class_ref(
    value: &xml::TrafficClassRef,
    path: &str,
) -> Result<canonical::TrafficClassRef, ConnectivityConversionError> {
    Ok(canonical::TrafficClassRef {
        network: canonical::NetworkRef {
            scope: scope(value.instance.as_ref(), path)?,
            network: value.network.clone(),
        },
        traffic_class: value.traffic_class.clone(),
    })
}

fn schedule_ref(
    value: &xml::ScheduleRef,
    path: &str,
) -> Result<canonical::ScheduleRef, ConnectivityConversionError> {
    Ok(canonical::ScheduleRef {
        network: canonical::NetworkRef {
            scope: scope(value.instance.as_ref(), path)?,
            network: value.network.clone(),
        },
        schedule: value.schedule.clone(),
    })
}

fn hop_ref(
    value: &xml::HopRef,
    path: &str,
) -> Result<canonical::HopRef, ConnectivityConversionError> {
    Ok(canonical::HopRef {
        network: canonical::NetworkRef {
            scope: scope(value.instance.as_ref(), path)?,
            network: value.network.clone(),
        },
        hop: value.hop.clone(),
    })
}

fn port_ref(
    value: &xml::PortRef,
    path: &str,
) -> Result<canonical::PortRef, ConnectivityConversionError> {
    Ok(canonical::PortRef {
        scope: scope(value.instance.as_ref(), path)?,
        component: value.component.clone(),
        port: value.port.clone(),
    })
}

fn channel_ref(
    value: &xml::ChannelRef,
    path: &str,
) -> Result<canonical::ChannelRef, ConnectivityConversionError> {
    Ok(canonical::ChannelRef {
        scope: scope(value.instance.as_ref(), path)?,
        component: value.component.clone(),
        port: value.port.clone(),
        channel: value.channel.clone(),
    })
}

fn functional_endpoint(
    value: &xml::FunctionalEndpointRef,
    path: &str,
) -> Result<canonical::FunctionalEndpointRef, ConnectivityConversionError> {
    match &value.endpoint {
        xml::FunctionalEndpointChoice::Port(reference) => {
            Ok(canonical::FunctionalEndpointRef::Port(port_ref(
                reference,
                &format!("{path}/port-ref"),
            )?))
        }
        xml::FunctionalEndpointChoice::Channel(reference) => {
            Ok(canonical::FunctionalEndpointRef::Channel(channel_ref(
                reference,
                &format!("{path}/channel-ref"),
            )?))
        }
    }
}

fn connector_ref(
    value: &xml::ConnectorRef,
    path: &str,
) -> Result<canonical::ConnectorRef, ConnectivityConversionError> {
    nonempty_reference(
        &value.connector,
        &format!("{path}/@connector"),
        "connector reference",
    )?;
    Ok(canonical::ConnectorRef {
        owner: owner(&value.owner, path)?,
        connector: value.connector.clone(),
    })
}

fn position_ref(
    value: &xml::PositionRef,
    path: &str,
) -> Result<canonical::PositionRef, ConnectivityConversionError> {
    nonempty_reference(
        &value.connector,
        &format!("{path}/@connector"),
        "position connector reference",
    )?;
    nonempty_reference(
        &value.position,
        &format!("{path}/@position"),
        "position reference",
    )?;
    Ok(canonical::PositionRef {
        owner: owner(&value.owner, path)?,
        connector: value.connector.clone(),
        position: value.position.clone(),
    })
}

fn junction_ref(
    value: &xml::JunctionRef,
    path: &str,
) -> Result<canonical::JunctionRef, ConnectivityConversionError> {
    nonempty_reference(
        &value.junction,
        &format!("{path}/@junction"),
        "junction reference",
    )?;
    Ok(canonical::JunctionRef {
        owner: owner(&value.owner, path)?,
        junction: value.junction.clone(),
    })
}

fn physical_endpoint(
    value: &xml::PhysicalEndpointRef,
    path: &str,
) -> Result<canonical::PhysicalEndpointRef, ConnectivityConversionError> {
    match &value.endpoint {
        xml::PhysicalEndpointChoice::Connector(reference) => {
            Ok(canonical::PhysicalEndpointRef::Connector(connector_ref(
                reference,
                &format!("{path}/connector-ref"),
            )?))
        }
        xml::PhysicalEndpointChoice::Position(reference) => {
            Ok(canonical::PhysicalEndpointRef::Position(position_ref(
                reference,
                &format!("{path}/position-ref"),
            )?))
        }
        xml::PhysicalEndpointChoice::Junction(reference) => {
            Ok(canonical::PhysicalEndpointRef::Junction(junction_ref(
                reference,
                &format!("{path}/junction-ref"),
            )?))
        }
    }
}

fn optional_representation(
    value: Option<&xml::Representation>,
    path: &str,
) -> Result<Option<canonical::Representation>, ConnectivityConversionError> {
    value
        .map(|value| representation(value, &format!("{path}/representation")))
        .transpose()
}

fn representation(
    value: &xml::Representation,
    path: &str,
) -> Result<canonical::Representation, ConnectivityConversionError> {
    match &value.variant {
        xml::RepresentationChoice::Box(value) => Ok(canonical::Representation::Primitive {
            primitive: canonical::PrimitiveRepresentation::Box { size: value.size },
            placement: placement(&value.placement, &format!("{path}/box/placement"))?,
        }),
        xml::RepresentationChoice::Cylinder(value) => Ok(canonical::Representation::Primitive {
            primitive: canonical::PrimitiveRepresentation::Cylinder {
                radius: value.radius,
                length: value.length,
            },
            placement: placement(&value.placement, &format!("{path}/cylinder/placement"))?,
        }),
        xml::RepresentationChoice::Sphere(value) => Ok(canonical::Representation::Primitive {
            primitive: canonical::PrimitiveRepresentation::Sphere {
                radius: value.radius,
            },
            placement: placement(&value.placement, &format!("{path}/sphere/placement"))?,
        }),
        xml::RepresentationChoice::Model(value) => {
            nonempty(
                &value.uri,
                &format!("{path}/model/@uri"),
                "standalone model @uri",
            )?;
            if let Some(sha) = &value.sha {
                nonempty(sha, &format!("{path}/model/@sha"), "standalone model @sha")?;
            }
            if let Some(node_path) = &value.node_path {
                nonempty(
                    node_path,
                    &format!("{path}/model/@node-path"),
                    "standalone model @node-path",
                )?;
            }
            Ok(canonical::Representation::Model {
                model: canonical::ModelRepresentation {
                    uri: value.uri.clone(),
                    sha: value.sha.clone(),
                    node_path: value.node_path.clone(),
                },
                placement: placement(&value.placement, &format!("{path}/model/placement"))?,
            })
        }
        xml::RepresentationChoice::ModelPart(value) => {
            nonempty(
                &value.node_path,
                &format!("{path}/model-part/@node-path"),
                "model-part @node-path",
            )?;
            if let Some(submesh) = &value.submesh_fallback {
                nonempty(
                    submesh,
                    &format!("{path}/model-part/@submesh-fallback"),
                    "model-part @submesh-fallback",
                )?;
            }
            let model_root = match &value.model_root.root {
                xml::ModelRootChoice::ComponentVisual(root) => {
                    canonical::ModelRootRef::ComponentVisual {
                        component: canonical::ComponentRef {
                            scope: scope(
                                root.instance.as_ref(),
                                &format!("{path}/model-part/model-root/component-visual"),
                            )?,
                            component: root.component.clone(),
                        },
                        visual: root.visual.clone(),
                    }
                }
                xml::ModelRootChoice::AssemblyModel(root) => {
                    canonical::ModelRootRef::AssemblyModel {
                        assembly: canonical::AssemblyRef {
                            scope: scope(
                                root.instance.as_ref(),
                                &format!("{path}/model-part/model-root/assembly-model"),
                            )?,
                            assembly: root.assembly.clone(),
                        },
                    }
                }
            };
            Ok(canonical::Representation::ModelPart(
                canonical::ModelPartRepresentation {
                    model_root,
                    node_path: value.node_path.clone(),
                    submesh_fallback: value.submesh_fallback.clone(),
                },
            ))
        }
        xml::RepresentationChoice::DerivedRoute(value) => {
            let section = route_section(value, path)?;
            if value.waypoint.len() < 2 {
                return Err(ConnectivityConversionError::coded(
                    "E_CONN_REPRESENTATION_SHAPE",
                    format!("{path}/derived-route"),
                    "a derived route requires at least two framed waypoints",
                ));
            }
            let mut waypoints = Vec::with_capacity(value.waypoint.len());
            for (index, waypoint) in value.waypoint.iter().enumerate() {
                let waypoint_path = format!("{path}/derived-route/waypoint[{index}]");
                let frame = route_frame(&waypoint.frame, &waypoint_path, "waypoint")?;
                let rotation =
                    waypoint
                        .rotation
                        .as_ref()
                        .map(|rotation| match &rotation.rotation {
                            xml::PlacementRotationChoice::Rpy(rotation) => {
                                canonical::PlacementRotation::Rpy(rotation.value)
                            }
                            xml::PlacementRotationChoice::Quaternion(rotation) => {
                                canonical::PlacementRotation::Quaternion(rotation.value)
                            }
                        });
                waypoints.push(canonical::RoutePoint {
                    frame,
                    xyz: waypoint.xyz,
                    rotation,
                });
            }
            Ok(canonical::Representation::DerivedRoute(
                canonical::DerivedRouteRepresentation { waypoints, section },
            ))
        }
    }
}

fn nonempty(value: &str, path: &str, label: &str) -> Result<(), ConnectivityConversionError> {
    if value.trim().is_empty() {
        return Err(ConnectivityConversionError::coded(
            "E_CONN_REPRESENTATION_SHAPE",
            path,
            format!("{label} must not be empty"),
        ));
    }
    Ok(())
}

fn nonempty_reference(
    value: &str,
    path: &str,
    label: &str,
) -> Result<(), ConnectivityConversionError> {
    if value.trim().is_empty() {
        return Err(ConnectivityConversionError::coded(
            "E_CONN_EMPTY_REFERENCE",
            path,
            format!("{label} must not be empty"),
        ));
    }
    Ok(())
}

fn nonempty_frame(value: &str, path: &str, label: &str) -> Result<(), ConnectivityConversionError> {
    if value.trim().is_empty() {
        return Err(ConnectivityConversionError::coded(
            "E_CONN_FRAME_REFERENCE",
            path,
            format!("{label} must not be empty"),
        ));
    }
    Ok(())
}

fn route_section(
    value: &xml::DerivedRouteRepresentation,
    path: &str,
) -> Result<Option<canonical::RouteSection>, ConnectivityConversionError> {
    let section = match &value.section {
        None => None,
        Some(xml::RouteSectionChoice::Round(section)) => Some(canonical::RouteSection::Round {
            diameter: section.diameter,
        }),
        Some(xml::RouteSectionChoice::Rectangular(section)) => {
            Some(canonical::RouteSection::Rectangular {
                width: section.width,
                height: section.height,
            })
        }
    };
    let valid = match &section {
        None => true,
        Some(canonical::RouteSection::Round { diameter }) => {
            diameter.is_finite() && *diameter > 0.0
        }
        Some(canonical::RouteSection::Rectangular { width, height }) => {
            width.is_finite() && *width > 0.0 && height.is_finite() && *height > 0.0
        }
    };
    if !valid {
        return Err(ConnectivityConversionError::coded(
            "E_CONN_REPRESENTATION_SHAPE",
            format!("{path}/derived-route"),
            "route section dimensions must be finite and greater than zero",
        ));
    }
    Ok(section)
}

fn route_frame(
    value: &xml::RouteFrame,
    path: &str,
    label: &str,
) -> Result<canonical::RouteFrameRef, ConnectivityConversionError> {
    match &value.frame {
        xml::RouteFrameChoice::World(_) => Ok(canonical::RouteFrameRef::World),
        xml::RouteFrameChoice::ComponentOrigin(frame) => {
            nonempty_frame(&frame.component, path, &format!("{label} component"))?;
            Ok(canonical::RouteFrameRef::ComponentOrigin {
                component: canonical::ComponentRef {
                    scope: scope(frame.instance.as_ref(), path)?,
                    component: frame.component.clone(),
                },
            })
        }
        xml::RouteFrameChoice::ComponentFrame(frame) => {
            nonempty_frame(&frame.component, path, &format!("{label} component"))?;
            nonempty_frame(&frame.frame, path, &format!("{label} frame"))?;
            Ok(canonical::RouteFrameRef::ComponentFrame {
                component: canonical::ComponentRef {
                    scope: scope(frame.instance.as_ref(), path)?,
                    component: frame.component.clone(),
                },
                frame: frame.frame.clone(),
            })
        }
    }
}

fn placement(
    value: &xml::Placement,
    path: &str,
) -> Result<canonical::Placement, ConnectivityConversionError> {
    let frame = route_frame(&value.frame, path, "placement")?;
    let rotation = match &value.rotation.rotation {
        xml::PlacementRotationChoice::Rpy(rotation) => {
            canonical::PlacementRotation::Rpy(rotation.value)
        }
        xml::PlacementRotationChoice::Quaternion(rotation) => {
            canonical::PlacementRotation::Quaternion(rotation.value)
        }
    };
    Ok(canonical::Placement {
        frame,
        xyz: value.xyz,
        rotation,
    })
}
fn validate_representation_selectors(
    document: &Hcdf,
    scope: &canonical::ConnectivityScope,
) -> Result<(), ConnectivityConversionError> {
    for component in &scope.components {
        let base = format!("comp {:?}", component.component);
        for connector in &component.connectors {
            validate_optional_representation(
                document,
                scope,
                connector.representation.as_ref(),
                &format!("{base}/connector {:?}", connector.name),
            )?;
            for position in &connector.positions {
                validate_optional_representation(
                    document,
                    scope,
                    position.representation.as_ref(),
                    &format!(
                        "{base}/connector {:?}/position {:?}",
                        connector.name, position.name
                    ),
                )?;
            }
        }
        for antenna in &component.antennas {
            validate_optional_representation(
                document,
                scope,
                antenna.representation.as_ref(),
                &format!("{base}/antenna {:?}", antenna.name),
            )?;
        }
        for path in &component.paths {
            validate_optional_representation(
                document,
                scope,
                path.representation.as_ref(),
                &format!("{base}/path {:?}", path.name),
            )?;
        }
        for junction in &component.junctions {
            validate_optional_representation(
                document,
                scope,
                junction.representation.as_ref(),
                &format!("{base}/junction {:?}", junction.name),
            )?;
        }
        for termination in &component.terminations {
            validate_optional_representation(
                document,
                scope,
                termination.representation.as_ref(),
                &format!("{base}/termination {:?}", termination.name),
            )?;
        }
    }
    for assembly in &scope.assemblies {
        let base = format!("assembly {:?}", assembly.name);
        validate_optional_representation(document, scope, assembly.representation.as_ref(), &base)?;
        for connector in &assembly.connectors {
            validate_optional_representation(
                document,
                scope,
                connector.representation.as_ref(),
                &format!("{base}/connector {:?}", connector.name),
            )?;
            for position in &connector.positions {
                validate_optional_representation(
                    document,
                    scope,
                    position.representation.as_ref(),
                    &format!(
                        "{base}/connector {:?}/position {:?}",
                        connector.name, position.name
                    ),
                )?;
            }
        }
        for path in &assembly.paths {
            validate_optional_representation(
                document,
                scope,
                path.representation.as_ref(),
                &format!("{base}/path {:?}", path.name),
            )?;
        }
        for junction in &assembly.junctions {
            validate_optional_representation(
                document,
                scope,
                junction.representation.as_ref(),
                &format!("{base}/junction {:?}", junction.name),
            )?;
        }
        for termination in &assembly.terminations {
            validate_optional_representation(
                document,
                scope,
                termination.representation.as_ref(),
                &format!("{base}/termination {:?}", termination.name),
            )?;
        }
    }
    Ok(())
}

fn validate_optional_representation(
    document: &Hcdf,
    scope: &canonical::ConnectivityScope,
    representation: Option<&canonical::Representation>,
    path: &str,
) -> Result<(), ConnectivityConversionError> {
    let Some(representation) = representation else {
        return Ok(());
    };
    match representation {
        canonical::Representation::Primitive { placement, .. }
        | canonical::Representation::Model { placement, .. } => {
            validate_structural_frame(document, scope, &placement.frame, path)?;
        }
        canonical::Representation::ModelPart(model_part) => {
            if model_part.node_path.trim().is_empty() {
                return Err(ConnectivityConversionError::coded(
                    "E_CONN_REPRESENTATION_SHAPE",
                    path,
                    "model-part @node-path must not be empty",
                ));
            }
            match &model_part.model_root {
                canonical::ModelRootRef::ComponentVisual { component, visual } => {
                    if component.scope != canonical::ReferenceScope::Local {
                        return Ok(());
                    }
                    let matching_components = document
                        .comp
                        .iter()
                        .filter(|candidate| candidate.name == component.component)
                        .collect::<Vec<_>>();
                    let [component] = matching_components.as_slice() else {
                        return Err(ConnectivityConversionError::coded(
                            "E_CONN_UNRESOLVED_REFERENCE",
                            path,
                            format!(
                                "component-visual selector requires exactly one component {:?}",
                                component.component
                            ),
                        ));
                    };
                    let matching_visuals = component
                        .visual
                        .iter()
                        .filter(|candidate| candidate.name == *visual)
                        .collect::<Vec<_>>();
                    let [visual] = matching_visuals.as_slice() else {
                        return Err(ConnectivityConversionError::coded(
                            "E_CONN_UNRESOLVED_REFERENCE",
                            path,
                            format!(
                                "component-visual selector requires exactly one visual {:?}",
                                visual
                            ),
                        ));
                    };
                    if !matches!(visual.appearance, super::VisualAppearance::Model { .. }) {
                        return Err(ConnectivityConversionError::coded(
                            "E_CONN_MODEL_ROOT_TYPE",
                            path,
                            "component-visual selector requires a visual backed by a model",
                        ));
                    }
                }
                canonical::ModelRootRef::AssemblyModel { assembly } => {
                    if assembly.scope != canonical::ReferenceScope::Local {
                        return Ok(());
                    }
                    let matching = scope
                        .assemblies
                        .iter()
                        .filter(|candidate| candidate.name == assembly.assembly)
                        .collect::<Vec<_>>();
                    let [assembly] = matching.as_slice() else {
                        return Err(ConnectivityConversionError::coded(
                            "E_CONN_UNRESOLVED_REFERENCE",
                            path,
                            format!(
                                "assembly-model selector requires exactly one assembly {:?}",
                                assembly.assembly
                            ),
                        ));
                    };
                    if !matches!(
                        assembly.representation,
                        Some(canonical::Representation::Model { .. })
                    ) {
                        return Err(ConnectivityConversionError::coded(
                            "E_CONN_MODEL_ROOT_TYPE",
                            path,
                            "assembly-model selector requires an assembly with a standalone model representation",
                        ));
                    }
                }
            }
        }
        canonical::Representation::DerivedRoute(route) => {
            for (index, waypoint) in route.waypoints.iter().enumerate() {
                validate_structural_frame(
                    document,
                    scope,
                    &waypoint.frame,
                    &format!("{path}/waypoint[{index}]"),
                )?;
            }
        }
    }
    Ok(())
}

fn validate_structural_frame(
    document: &Hcdf,
    _scope: &canonical::ConnectivityScope,
    frame: &canonical::RouteFrameRef,
    path: &str,
) -> Result<(), ConnectivityConversionError> {
    let (component, named_frame) = match frame {
        canonical::RouteFrameRef::World => return Ok(()),
        canonical::RouteFrameRef::ComponentOrigin { component } => (component, None),
        canonical::RouteFrameRef::ComponentFrame { component, frame } => {
            (component, Some(frame.as_str()))
        }
    };
    if component.scope != canonical::ReferenceScope::Local {
        return Ok(());
    }
    let matching = document
        .comp
        .iter()
        .filter(|candidate| candidate.name == component.component)
        .collect::<Vec<_>>();
    let [component] = matching.as_slice() else {
        return Err(ConnectivityConversionError::coded(
            "E_CONN_FRAME_REFERENCE",
            path,
            format!(
                "component frame requires exactly one component {:?}",
                component.component
            ),
        ));
    };
    if let Some(frame) = named_frame {
        let count = component
            .frame
            .iter()
            .filter(|candidate| candidate.name == frame)
            .count();
        if count != 1 {
            return Err(ConnectivityConversionError::coded(
                "E_CONN_FRAME_REFERENCE",
                path,
                format!(
                    "component frame requires exactly one frame {frame:?} on component {:?}",
                    component.name
                ),
            ));
        }
    }
    Ok(())
}
