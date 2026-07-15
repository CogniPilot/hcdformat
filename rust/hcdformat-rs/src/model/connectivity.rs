//! Authored connectivity contract for the clean-slate HCDF networking model.
//!
//! These types deliberately separate functional interfaces, physical presentation, physical
//! continuity, and logical network membership. The resolver and normalized graph consume this typed
//! form without parsing display paths or inferring omitted physical detail.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// A stable caller-supplied identity for the root source document.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DocumentIdentity(String);

impl DocumentIdentity {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityValueError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(IdentityValueError::EmptyDocumentIdentity);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DocumentIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DocumentIdentity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// A qualified profile or specification identifier, such as `hcdf:rs-485`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct QualifiedId(String);

impl QualifiedId {
    pub fn new(value: impl Into<String>) -> Result<Self, IdentityValueError> {
        let value = value.into();
        let Some((authority, local)) = value.split_once(':') else {
            return Err(IdentityValueError::UnqualifiedIdentifier(value));
        };
        let valid_authority = authority
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && authority
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        let valid_local = local
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && local.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b'+')
            });
        if !valid_authority || !valid_local {
            return Err(IdentityValueError::UnqualifiedIdentifier(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for QualifiedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for QualifiedId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityValueError {
    #[error("document identity must not be empty")]
    EmptyDocumentIdentity,
    #[error("qualified identifier {0:?} must use a safe ASCII authority:local token")]
    UnqualifiedIdentifier(String),
}

/// The authored name state of one include-instance path segment.
///
/// Unnamed include sites are distinct from every authored string, including `$include` and
/// whitespace-only strings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum IncludeSegmentName {
    #[serde(rename = "unnamed")]
    Unnamed,
    #[serde(rename = "named")]
    Named(String),
}

impl IncludeSegmentName {
    pub fn named(value: impl Into<String>) -> Self {
        Self::Named(value.into())
    }

    pub const fn unnamed() -> Self {
        Self::Unnamed
    }

    pub fn as_named(&self) -> Option<&str> {
        match self {
            Self::Unnamed => None,
            Self::Named(value) => Some(value),
        }
    }
}

impl fmt::Display for IncludeSegmentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unnamed => f.write_str("unnamed"),
            Self::Named(value) => write!(f, "named({value:?})"),
        }
    }
}

/// One deterministic include site in an include-instance path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IncludeInstanceSegment {
    pub name: IncludeSegmentName,
    pub occurrence: u32,
}

impl IncludeInstanceSegment {
    pub fn named(name: impl Into<String>, occurrence: u32) -> Self {
        Self {
            name: IncludeSegmentName::named(name),
            occurrence,
        }
    }

    pub const fn unnamed(occurrence: u32) -> Self {
        Self {
            name: IncludeSegmentName::unnamed(),
            occurrence,
        }
    }
}

/// Structured provenance for one root or included module instance.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IncludeInstanceId(Vec<IncludeInstanceSegment>);

impl IncludeInstanceId {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn child(&self, name: impl Into<String>, occurrence: u32) -> Self {
        self.named_child(name, occurrence)
    }

    pub fn named_child(&self, name: impl Into<String>, occurrence: u32) -> Self {
        let mut segments = self.0.clone();
        segments.push(IncludeInstanceSegment::named(name, occurrence));
        Self(segments)
    }

    pub fn unnamed_child(&self, occurrence: u32) -> Self {
        let mut segments = self.0.clone();
        segments.push(IncludeInstanceSegment::unnamed(occurrence));
        Self(segments)
    }

    pub fn segment_child(&self, segment: IncludeInstanceSegment) -> Self {
        let mut segments = self.0.clone();
        segments.push(segment);
        Self(segments)
    }

    pub fn segments(&self) -> &[IncludeInstanceSegment] {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn parent(&self) -> Option<Self> {
        (!self.is_root()).then(|| Self(self.0[..self.0.len() - 1].to_vec()))
    }

    /// Render a human-readable path. The result is display-only and is never parsed as a reference.
    pub fn display_path(&self) -> String {
        if self.is_root() {
            return "<root>".to_owned();
        }
        self.0
            .iter()
            .map(|segment| format!("{}#{}", segment.name, segment.occurrence))
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// Scope selection carried by every authored cross-object reference.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "scope", content = "instance", rename_all = "kebab-case")]
pub enum ReferenceScope {
    #[default]
    Local,
    Instance(IncludeInstanceId),
}

impl<'de> Deserialize<'de> for ReferenceScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "scope", content = "instance", rename_all = "kebab-case")]
        enum AuthoredReferenceScope {
            Local,
            Instance(IncludeInstanceId),
        }

        match AuthoredReferenceScope::deserialize(deserializer)? {
            AuthoredReferenceScope::Local => Ok(Self::Local),
            AuthoredReferenceScope::Instance(instance) if instance.is_root() => {
                Err(serde::de::Error::custom(
                    "an explicit instance scope must contain at least one segment",
                ))
            }
            AuthoredReferenceScope::Instance(instance) => Ok(Self::Instance(instance)),
        }
    }
}

impl ReferenceScope {
    pub fn resolve(&self, local: &IncludeInstanceId) -> IncludeInstanceId {
        match self {
            Self::Local => local.clone(),
            Self::Instance(relative) => {
                let mut resolved = local.clone();
                for segment in relative.segments() {
                    resolved = resolved.segment_child(segment.clone());
                }
                resolved
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ComponentRef {
    #[serde(default)]
    pub scope: ReferenceScope,
    pub component: String,
}

impl ComponentRef {
    pub fn local(component: impl Into<String>) -> Self {
        Self {
            scope: ReferenceScope::Local,
            component: component.into(),
        }
    }

    pub fn in_instance(instance: IncludeInstanceId, component: impl Into<String>) -> Self {
        Self {
            scope: ReferenceScope::Instance(instance),
            component: component.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AssemblyRef {
    #[serde(default)]
    pub scope: ReferenceScope,
    pub assembly: String,
}

impl AssemblyRef {
    pub fn local(assembly: impl Into<String>) -> Self {
        Self {
            scope: ReferenceScope::Local,
            assembly: assembly.into(),
        }
    }

    pub fn in_instance(instance: IncludeInstanceId, assembly: impl Into<String>) -> Self {
        Self {
            scope: ReferenceScope::Instance(instance),
            assembly: assembly.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StructuralVisualRef {
    pub component: ComponentRef,
    pub visual: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StructuralFrameRef {
    pub component: ComponentRef,
    pub frame: String,
}

impl StructuralVisualRef {
    pub fn local(component: impl Into<String>, visual: impl Into<String>) -> Self {
        Self {
            component: ComponentRef::local(component),
            visual: visual.into(),
        }
    }

    pub fn in_instance(
        instance: IncludeInstanceId,
        component: impl Into<String>,
        visual: impl Into<String>,
    ) -> Self {
        Self {
            component: ComponentRef::in_instance(instance, component),
            visual: visual.into(),
        }
    }
}

impl StructuralFrameRef {
    pub fn local(component: impl Into<String>, frame: impl Into<String>) -> Self {
        Self {
            component: ComponentRef::local(component),
            frame: frame.into(),
        }
    }

    pub fn in_instance(
        instance: IncludeInstanceId,
        component: impl Into<String>,
        frame: impl Into<String>,
    ) -> Self {
        Self {
            component: ComponentRef::in_instance(instance, component),
            frame: frame.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "owner-kind", rename_all = "kebab-case")]
pub enum OwnerRef {
    Component(ComponentRef),
    Assembly(AssemblyRef),
}

impl OwnerRef {
    pub fn local_component(component: impl Into<String>) -> Self {
        Self::Component(ComponentRef::local(component))
    }

    pub fn local_assembly(assembly: impl Into<String>) -> Self {
        Self::Assembly(AssemblyRef::local(assembly))
    }

    pub fn scope(&self) -> &ReferenceScope {
        match self {
            Self::Component(reference) => &reference.scope,
            Self::Assembly(reference) => &reference.scope,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PortRef {
    #[serde(default)]
    pub scope: ReferenceScope,
    pub component: String,
    pub port: String,
}

impl PortRef {
    pub fn local(component: impl Into<String>, port: impl Into<String>) -> Self {
        Self {
            scope: ReferenceScope::Local,
            component: component.into(),
            port: port.into(),
        }
    }

    pub fn in_instance(
        instance: IncludeInstanceId,
        component: impl Into<String>,
        port: impl Into<String>,
    ) -> Self {
        Self {
            scope: ReferenceScope::Instance(instance),
            component: component.into(),
            port: port.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChannelRef {
    #[serde(default)]
    pub scope: ReferenceScope,
    pub component: String,
    pub port: String,
    pub channel: String,
}

impl ChannelRef {
    pub fn local(
        component: impl Into<String>,
        port: impl Into<String>,
        channel: impl Into<String>,
    ) -> Self {
        Self {
            scope: ReferenceScope::Local,
            component: component.into(),
            port: port.into(),
            channel: channel.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "endpoint-kind", rename_all = "kebab-case")]
pub enum FunctionalEndpointRef {
    Port(PortRef),
    Channel(ChannelRef),
}

impl FunctionalEndpointRef {
    pub fn port(&self) -> PortRef {
        match self {
            Self::Port(reference) => reference.clone(),
            Self::Channel(reference) => PortRef {
                scope: reference.scope.clone(),
                component: reference.component.clone(),
                port: reference.port.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConnectorRef {
    pub owner: OwnerRef,
    pub connector: String,
}

impl ConnectorRef {
    pub fn local_component(component: impl Into<String>, connector: impl Into<String>) -> Self {
        Self {
            owner: OwnerRef::local_component(component),
            connector: connector.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PositionRef {
    pub owner: OwnerRef,
    pub connector: String,
    pub position: String,
}

impl PositionRef {
    pub fn local_component(
        component: impl Into<String>,
        connector: impl Into<String>,
        position: impl Into<String>,
    ) -> Self {
        Self {
            owner: OwnerRef::local_component(component),
            connector: connector.into(),
            position: position.into(),
        }
    }

    pub fn connector_ref(&self) -> ConnectorRef {
        ConnectorRef {
            owner: self.owner.clone(),
            connector: self.connector.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct JunctionRef {
    pub owner: OwnerRef,
    pub junction: String,
}

impl JunctionRef {
    pub fn local_component(component: impl Into<String>, junction: impl Into<String>) -> Self {
        Self {
            owner: OwnerRef::local_component(component),
            junction: junction.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "physical-endpoint-kind", rename_all = "kebab-case")]
pub enum PhysicalEndpointRef {
    Connector(ConnectorRef),
    Position(PositionRef),
    Junction(JunctionRef),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NetworkRef {
    #[serde(default)]
    pub scope: ReferenceScope,
    pub network: String,
}

impl NetworkRef {
    pub fn local(network: impl Into<String>) -> Self {
        Self {
            scope: ReferenceScope::Local,
            network: network.into(),
        }
    }
}

/// A structured reference to one canonical stream group in an HCDF include-instance scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StreamGroupRef {
    #[serde(default)]
    pub scope: ReferenceScope,
    pub group: String,
}

impl StreamGroupRef {
    pub fn local(group: impl Into<String>) -> Self {
        Self {
            scope: ReferenceScope::Local,
            group: group.into(),
        }
    }

    pub fn in_instance(instance: IncludeInstanceId, group: impl Into<String>) -> Self {
        Self {
            scope: ReferenceScope::Instance(instance),
            group: group.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConnectivityFunctionRef {
    pub component: ComponentRef,
    pub function: String,
}

impl ConnectivityFunctionRef {
    pub fn local(component: impl Into<String>, function: impl Into<String>) -> Self {
        Self {
            component: ComponentRef::local(component),
            function: function.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ParticipantRef {
    pub network: NetworkRef,
    pub participant: String,
}

impl ParticipantRef {
    pub fn local(network: impl Into<String>, participant: impl Into<String>) -> Self {
        Self {
            network: NetworkRef::local(network),
            participant: participant.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HopRef {
    pub network: NetworkRef,
    pub hop: String,
}

impl HopRef {
    pub fn local(network: impl Into<String>, hop: impl Into<String>) -> Self {
        Self {
            network: NetworkRef::local(network),
            hop: hop.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LegRef {
    pub network: NetworkRef,
    pub leg: String,
}

impl LegRef {
    pub fn local(network: impl Into<String>, leg: impl Into<String>) -> Self {
        Self {
            network: NetworkRef::local(network),
            leg: leg.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TrafficClassRef {
    pub network: NetworkRef,
    pub traffic_class: String,
}

impl TrafficClassRef {
    pub fn local(network: impl Into<String>, traffic_class: impl Into<String>) -> Self {
        Self {
            network: NetworkRef::local(network),
            traffic_class: traffic_class.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScheduleRef {
    pub network: NetworkRef,
    pub schedule: String,
}

impl ScheduleRef {
    pub fn local(network: impl Into<String>, schedule: impl Into<String>) -> Self {
        Self {
            network: NetworkRef::local(network),
            schedule: schedule.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MacsecPolicyRef {
    pub network: NetworkRef,
    pub policy: String,
}

impl MacsecPolicyRef {
    pub fn local(network: impl Into<String>, policy: impl Into<String>) -> Self {
        Self {
            network: NetworkRef::local(network),
            policy: policy.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "topology-segment-kind", rename_all = "kebab-case")]
pub enum TopologySegmentRef {
    Network(NetworkRef),
    Leg(LegRef),
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "kebab-case")]
pub enum Fidelity {
    #[default]
    Functional,
    Presented,
    Exact,
    Quantified,
}

impl Fidelity {
    pub fn is_exact(self) -> bool {
        matches!(self, Self::Exact | Self::Quantified)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    Communication,
    PowerDelivery,
    MaterialTransfer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Carrier {
    Electrical,
    GuidedOptical,
    ConductedRf,
    RadiatedRf,
    Liquid,
    Gas,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Topology {
    Link,
    Bus,
    Chain,
    Star,
    Ring,
    Mesh,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PositionKind {
    Pin,
    Socket,
    Contact,
    Fiber,
    Passage,
    Feed,
    WaveguideOpening,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PathKind {
    Wire,
    Conductor,
    CableMember,
    Fiber,
    Coax,
    Waveguide,
    Feed,
    Hose,
    Pipe,
    Passage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JunctionKind {
    Splice,
    Tee,
    Manifold,
    Busbar,
    OpticalSplitter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FunctionKind {
    Switch,
    Bridge,
    Converter,
    Transceiver,
    Radio,
}

/// Strong classification for a physical connectivity assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssemblyKind {
    Harness,
    Cable,
    Plumbing,
    Umbilical,
}

/// One typed physical or operational quantity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    pub value: f64,
    pub unit: String,
}

/// Supported minimum, maximum, and nominal value for one quantity axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QuantityRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<Quantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nominal: Option<Quantity>,
}

/// An exact selected value or a fully bounded selected operating range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum SelectionQuantity {
    Nominal(Quantity),
    Range(SelectionRange),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionRange {
    pub minimum: Quantity,
    pub maximum: Quantity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nominal: Option<Quantity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RfSelection {
    pub channel: RfChannelSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RfChannelSelection {
    Numbered {
        number: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        center_frequency: Option<SelectionQuantity>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bandwidth: Option<SelectionQuantity>,
    },
    FrequencyDefined {
        center_frequency: SelectionQuantity,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bandwidth: Option<SelectionQuantity>,
    },
}

impl RfChannelSelection {
    pub fn number(&self) -> Option<u32> {
        match self {
            Self::Numbered { number, .. } => Some(*number),
            Self::FrequencyDefined { .. } => None,
        }
    }

    pub fn center_frequency(&self) -> Option<&SelectionQuantity> {
        match self {
            Self::Numbered {
                center_frequency, ..
            } => center_frequency.as_ref(),
            Self::FrequencyDefined {
                center_frequency, ..
            } => Some(center_frequency),
        }
    }

    pub fn bandwidth(&self) -> Option<&SelectionQuantity> {
        match self {
            Self::Numbered { bandwidth, .. } | Self::FrequencyDefined { bandwidth, .. } => {
                bandwidth.as_ref()
            }
        }
    }
}

/// Typed capability limits shared across connectivity domains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CapabilityLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<QuantityRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<QuantityRange>,
}

/// Supported possibility declared by a port or channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Capabilities {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub purposes: BTreeSet<Purpose>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub carriers: BTreeSet<Carrier>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub profiles: BTreeSet<QualifiedId>,
    #[serde(default)]
    pub limits: CapabilityLimits,
}

/// Configuration selected for a logical network, including its required active axes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkSelection {
    pub purpose: Purpose,
    pub carrier: Carrier,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub profiles: BTreeSet<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rf: Option<RfSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<SelectionQuantity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_group: Option<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PrimitiveRepresentation {
    Box {
        size: [f64; 3],
    },
    /// Cylinder length extends along local +Z.
    Cylinder {
        radius: f64,
        length: f64,
    },
    Sphere {
        radius: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRepresentation {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "model-root-kind", rename_all = "kebab-case")]
pub enum ModelRootRef {
    ComponentVisual {
        component: ComponentRef,
        visual: String,
    },
    AssemblyModel {
        assembly: AssemblyRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelPartRepresentation {
    pub model_root: ModelRootRef,
    pub node_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submesh_fallback: Option<String>,
}

/// Explicit frame for a placement or route waypoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame-kind", rename_all = "kebab-case")]
pub enum RouteFrameRef {
    World,
    ComponentOrigin {
        component: ComponentRef,
    },
    ComponentFrame {
        component: ComponentRef,
        frame: String,
    },
}
/// RPY is fixed-axis XYZ in radians. Quaternion values are x, y, z, w with scalar last.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rotation-kind", content = "value", rename_all = "kebab-case")]
pub enum PlacementRotation {
    Rpy([f64; 3]),
    Quaternion([f64; 4]),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub frame: RouteFrameRef,
    pub xyz: [f64; 3],
    pub rotation: PlacementRotation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutePoint {
    pub frame: RouteFrameRef,
    pub xyz: [f64; 3],
    /// Cross-section orientation in the waypoint frame. Local +Z follows the route tangent;
    /// local +X spans rectangular width and local +Y spans rectangular height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<PlacementRotation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "section-kind", rename_all = "kebab-case")]
pub enum RouteSection {
    Round { diameter: f64 },
    Rectangular { width: f64, height: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivedRouteRepresentation {
    pub waypoints: Vec<RoutePoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<RouteSection>,
}

/// Exactly one primary physical presentation form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "representation-kind", rename_all = "kebab-case")]
pub enum Representation {
    Primitive {
        primitive: PrimitiveRepresentation,
        placement: Placement,
    },
    Model {
        model: ModelRepresentation,
        placement: Placement,
    },
    ModelPart(ModelPartRepresentation),
    DerivedRoute(DerivedRouteRepresentation),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub name: String,
    pub kind: PositionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connector {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positions: Vec<Position>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Antenna {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conducted_port: Option<PortRef>,
    pub radiated_port: PortRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

/// One named structural visual retained as a model-root candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralVisual {
    pub name: String,
    pub model_backed: bool,
}

/// Structural anchors retained from one component without changing the XML structural model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralAnchors {
    pub component: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visuals: Vec<StructuralVisual>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<String>,
}

/// Connectivity declarations owned by an existing structural component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentConnectivity {
    pub component: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<Port>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connectors: Vec<Connector>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub antennas: Vec<Antenna>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<ConnectivityFunction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PhysicalPath>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub junctions: Vec<Junction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminations: Vec<Termination>,
}

/// A harness, cable, plumbing assembly, or hybrid umbilical that owns physical objects but is not a
/// structural component or network participant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysicalAssembly {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connectors: Vec<Connector>,
    pub kind: AssemblyKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PhysicalPath>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub junctions: Vec<Junction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminations: Vec<Termination>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub name: String,
    pub functional: FunctionalEndpointRef,
    pub physical: PhysicalEndpointRef,
    pub fidelity: Fidelity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionMapping {
    pub first: PositionRef,
    pub second: PositionRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mate {
    pub name: String,
    pub first: ConnectorRef,
    pub second: ConnectorRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mappings: Vec<PositionMapping>,
    pub fidelity: Fidelity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysicalPath {
    pub name: String,
    pub kind: PathKind,
    pub first: PhysicalEndpointRef,
    pub second: PhysicalEndpointRef,
    pub fidelity: Fidelity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Junction {
    pub name: String,
    pub kind: JunctionKind,
    pub fidelity: Fidelity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PhysicalEndpointRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminationMounting {
    Endpoint,
    Inline,
    Branch,
    Closure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedQuantity {
    pub property: QualifiedId,
    pub quantity: Quantity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Termination {
    pub name: String,
    pub kind: QualifiedId,
    pub mounting: TerminationMounting,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PhysicalEndpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quantities: Vec<NamedQuantity>,
    pub fidelity: Fidelity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "owner-kind", rename_all = "kebab-case")]
pub enum HopOwnerRef {
    Component(ComponentRef),
    Function(ConnectivityFunctionRef),
}

impl HopOwnerRef {
    pub fn component(&self) -> &ComponentRef {
        match self {
            Self::Component(reference) => reference,
            Self::Function(reference) => &reference.component,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    pub name: String,
    pub owner: HopOwnerRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing_delay_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegEnd {
    pub hop: HopRef,
    pub participant: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leg {
    pub name: String,
    pub from: LegEnd,
    pub to: LegEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathTopology {
    pub hops: Vec<Hop>,
    pub legs: Vec<Leg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StarTopology {
    pub coordinator: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeTopology {
    pub root: HopRef,
    pub hops: Vec<Hop>,
    pub legs: Vec<Leg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "topology", content = "details", rename_all = "kebab-case")]
pub enum NetworkStructure {
    Link,
    Bus,
    Chain(PathTopology),
    Star(StarTopology),
    Ring(PathTopology),
    Mesh,
    Tree(TreeTopology),
}

impl NetworkStructure {
    pub fn topology(&self) -> Topology {
        match self {
            Self::Link => Topology::Link,
            Self::Bus => Topology::Bus,
            Self::Chain(_) => Topology::Chain,
            Self::Star(_) => Topology::Star,
            Self::Ring(_) => Topology::Ring,
            Self::Mesh => Topology::Mesh,
            Self::Tree(_) => Topology::Tree,
        }
    }

    pub fn hops(&self) -> &[Hop] {
        match self {
            Self::Chain(topology) | Self::Ring(topology) => &topology.hops,
            Self::Tree(topology) => &topology.hops,
            Self::Link | Self::Bus | Self::Star(_) | Self::Mesh => &[],
        }
    }

    pub fn legs(&self) -> &[Leg] {
        match self {
            Self::Chain(topology) | Self::Ring(topology) => &topology.legs,
            Self::Tree(topology) => &topology.legs,
            Self::Link | Self::Bus | Self::Star(_) | Self::Mesh => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GptpClockKind {
    Ordinary,
    Boundary,
    Transparent,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GptpPortDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_sync_interval: Option<i8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_announce_interval: Option<i8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_pdelay_req_interval: Option<i8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announce_receipt_timeout: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub neighbor_prop_delay_threshold_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GptpClock {
    pub name: String,
    pub participant: ParticipantRef,
    pub kind: GptpClockKind,
    pub gm_capable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority1: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority2: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_class: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_accuracy: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GptpDomain {
    pub name: String,
    pub number: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_defaults: Option<GptpPortDefaults>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clocks: Vec<GptpClock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrafficPreemption {
    Express,
    Preemptable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficClass {
    pub name: String,
    pub number: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preemption: Option<TrafficPreemption>,
    pub pcp: BTreeSet<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateControlEntry {
    pub duration_ns: u64,
    pub open: BTreeSet<TrafficClassRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateSchedule {
    pub name: String,
    pub cycle_time_ns: u64,
    pub entries: Vec<GateControlEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleAssignment {
    pub name: String,
    pub schedule: ScheduleRef,
    pub targets: Vec<ParticipantRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlcaNode {
    pub node_id: u8,
    pub burst_count: u8,
    pub burst_timer_bit_times: u16,
    pub participant: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlcaConfiguration {
    pub max_node_id: u8,
    pub to_timer_bit_times: u16,
    pub nodes: Vec<PlcaNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MacsecEnforcement {
    MustSecure,
    ShouldSecure,
    IntegrityOnly,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacsecPolicyDefinition {
    pub name: String,
    pub enforcement: MacsecEnforcement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cipher: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_agreement: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidentiality_offset: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rekey_interval_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_store_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacsecOverride {
    pub target: TopologySegmentRef,
    pub policy: MacsecPolicyRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacsecConfiguration {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<MacsecPolicyDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_policy: Option<MacsecPolicyRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<MacsecOverride>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EeeMode {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EeeOverride {
    pub participant: ParticipantRef,
    pub mode: EeeMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EeeConfiguration {
    pub default_mode: EeeMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<EeeOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NetworkConfiguration {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gptp_domains: Vec<GptpDomain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traffic_classes: Vec<TrafficClass>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gate_schedules: Vec<GateSchedule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schedule_assignments: Vec<ScheduleAssignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plca: Option<PlcaConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macsec: Option<MacsecConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eee: Option<EeeConfiguration>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    pub name: String,
    pub endpoint: FunctionalEndpointRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<QualifiedId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Network {
    pub name: String,
    pub structure: NetworkStructure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub selected: NetworkSelection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<Participant>,
    #[serde(default, skip_serializing_if = "NetworkConfiguration::is_empty")]
    pub configuration: NetworkConfiguration,
}

impl NetworkConfiguration {
    pub fn is_empty(&self) -> bool {
        self.gptp_domains.is_empty()
            && self.traffic_classes.is_empty()
            && self.gate_schedules.is_empty()
            && self.schedule_assignments.is_empty()
            && self.plca.is_none()
            && self.macsec.is_none()
            && self.eee.is_none()
    }
}

/// Active or directional behavior over ordinary component ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectivityFunction {
    pub name: String,
    pub kind: FunctionKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FunctionalEndpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FunctionalEndpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bidirectional: Vec<FunctionalEndpointRef>,
}

/// All connectivity declarations authored in one root or include instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectivityScope {
    pub instance: IncludeInstanceId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub structural_anchors: Vec<StructuralAnchors>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentConnectivity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assemblies: Vec<PhysicalAssembly>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mates: Vec<Mate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<Network>,
}

impl ConnectivityScope {
    pub fn root() -> Self {
        Self::new(IncludeInstanceId::root())
    }

    pub fn new(instance: IncludeInstanceId) -> Self {
        Self {
            instance,
            structural_anchors: Vec::new(),
            components: Vec::new(),
            assemblies: Vec::new(),
            bindings: Vec::new(),
            mates: Vec::new(),
            networks: Vec::new(),
        }
    }
}

/// Connectivity content for a root document and all loaded include instances.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectivityDocument {
    pub document: DocumentIdentity,
    pub scopes: Vec<ConnectivityScope>,
}

impl ConnectivityDocument {
    pub fn new(document: DocumentIdentity) -> Self {
        Self {
            document,
            scopes: vec![ConnectivityScope::root()],
        }
    }
}
