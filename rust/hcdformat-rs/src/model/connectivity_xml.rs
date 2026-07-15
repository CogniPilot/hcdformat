//! XML-facing connectivity model.
//!
//! The public document vocabulary uses concrete physical and functional nouns. Conversion into
//! [`super::connectivity`] collapses those authored choices into the common graph kinds used by
//! validation, visualization, and editing tools.

use super::connectivity::{
    Carrier, EeeMode, Fidelity, GptpClockKind, MacsecEnforcement, Purpose, TrafficPreemption,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InstanceRef {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segment: Vec<InstanceSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceSegment {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@occurrence")]
    pub occurrence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentRef {
    #[serde(rename = "@component")]
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssemblyRef {
    #[serde(rename = "@assembly")]
    pub assembly: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhysicalOwnerChoice {
    #[serde(rename = "component-ref")]
    Component(ComponentRef),
    #[serde(rename = "assembly-ref")]
    Assembly(AssemblyRef),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRef {
    #[serde(rename = "@network")]
    pub network: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectivityFunctionRef {
    #[serde(rename = "@component")]
    pub component: String,
    #[serde(rename = "@function")]
    pub function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantRef {
    #[serde(rename = "@network")]
    pub network: String,
    #[serde(rename = "@participant")]
    pub participant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HopRef {
    #[serde(rename = "@network")]
    pub network: String,
    #[serde(rename = "@hop")]
    pub hop: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegRef {
    #[serde(rename = "@network")]
    pub network: String,
    #[serde(rename = "@leg")]
    pub leg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficClassRef {
    #[serde(rename = "@network")]
    pub network: String,
    #[serde(rename = "@traffic-class")]
    pub traffic_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRef {
    #[serde(rename = "@network")]
    pub network: String,
    #[serde(rename = "@schedule")]
    pub schedule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacsecPolicyRef {
    #[serde(rename = "@network")]
    pub network: String,
    #[serde(rename = "@policy")]
    pub policy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologySegmentRef {
    #[serde(rename = "$value")]
    pub segment: TopologySegmentChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TopologySegmentChoice {
    #[serde(rename = "network-ref")]
    Network(NetworkRef),
    #[serde(rename = "leg-ref")]
    Leg(LegRef),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortRef {
    #[serde(rename = "@component")]
    pub component: String,
    #[serde(rename = "@port")]
    pub port: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRef {
    #[serde(rename = "@component")]
    pub component: String,
    #[serde(rename = "@port")]
    pub port: String,
    #[serde(rename = "@channel")]
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionalEndpointRef {
    #[serde(rename = "$value")]
    pub endpoint: FunctionalEndpointChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FunctionalEndpointChoice {
    #[serde(rename = "port-ref")]
    Port(PortRef),
    #[serde(rename = "channel-ref")]
    Channel(ChannelRef),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorRef {
    #[serde(rename = "@connector")]
    pub connector: String,
    #[serde(rename = "$value")]
    pub owner: PhysicalOwnerChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionRef {
    #[serde(rename = "@connector")]
    pub connector: String,
    #[serde(rename = "@position")]
    pub position: String,
    #[serde(rename = "$value")]
    pub owner: PhysicalOwnerChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JunctionRef {
    #[serde(rename = "@junction")]
    pub junction: String,
    #[serde(rename = "$value")]
    pub owner: PhysicalOwnerChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalEndpointRef {
    #[serde(rename = "$value")]
    pub endpoint: PhysicalEndpointChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhysicalEndpointChoice {
    #[serde(rename = "connector-ref")]
    Connector(ConnectorRef),
    #[serde(rename = "position-ref")]
    Position(PositionRef),
    #[serde(rename = "junction-ref")]
    Junction(JunctionRef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    #[serde(rename = "@value")]
    pub value: f64,
    #[serde(rename = "@unit")]
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QuantityRange {
    #[serde(rename = "@min", default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    #[serde(rename = "@max", default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    #[serde(rename = "@nominal", default, skip_serializing_if = "Option::is_none")]
    pub nominal: Option<f64>,
    #[serde(rename = "@unit")]
    pub unit: String,
}

/// Exact XML choice for an authored network selection axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionQuantity {
    #[serde(rename = "$value")]
    pub selection: SelectionQuantityChoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SelectionQuantityChoice {
    #[serde(rename = "nominal")]
    Nominal(Quantity),
    #[serde(rename = "range")]
    Range(SelectionRange),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectionRange {
    #[serde(rename = "@min")]
    pub minimum: f64,
    #[serde(rename = "@max")]
    pub maximum: f64,
    #[serde(rename = "@nominal", default, skip_serializing_if = "Option::is_none")]
    pub nominal: Option<f64>,
    #[serde(rename = "@unit")]
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RfSelection {
    pub channel: RfChannelSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RfChannelSelection {
    #[serde(rename = "$value")]
    pub selection: RfChannelSelectionChoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RfChannelSelectionChoice {
    #[serde(rename = "numbered")]
    Numbered(RfNumberedChannel),
    #[serde(rename = "frequency-defined")]
    FrequencyDefined(RfFrequencyDefinedChannel),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RfNumberedChannel {
    #[serde(rename = "@number")]
    pub number: u32,
    #[serde(
        rename = "center-frequency",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub center_frequency: Option<SelectionQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<SelectionQuantity>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RfFrequencyDefinedChannel {
    #[serde(rename = "center-frequency")]
    pub center_frequency: SelectionQuantity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<SelectionQuantity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurposeCapability {
    #[serde(rename = "@value")]
    pub value: Purpose,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarrierCapability {
    #[serde(rename = "@value")]
    pub value: Carrier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileCapability {
    #[serde(rename = "@id")]
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub purpose: Vec<PurposeCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub carrier: Vec<CarrierCapability>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile: Vec<ProfileCapability>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedProfile {
    #[serde(rename = "@id")]
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkSelection {
    #[serde(rename = "@purpose")]
    pub purpose: Purpose,
    #[serde(rename = "@carrier")]
    pub carrier: Carrier,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile: Vec<SelectedProfile>,
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

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Channel {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@role", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(
        rename = "@local-group",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub local_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Capabilities>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Port {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Capabilities>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrimitiveBox {
    #[serde(rename = "@size", with = "vec3_attribute")]
    pub size: [f64; 3],
    pub placement: Placement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrimitiveCylinder {
    #[serde(rename = "@radius")]
    pub radius: f64,
    #[serde(rename = "@length")]
    pub length: f64,
    pub placement: Placement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrimitiveSphere {
    #[serde(rename = "@radius")]
    pub radius: f64,
    pub placement: Placement,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRepresentation {
    #[serde(rename = "@uri")]
    pub uri: String,
    #[serde(rename = "@sha", default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(
        rename = "@node-path",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub node_path: Option<String>,
    pub placement: Placement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentVisualRoot {
    #[serde(rename = "@component")]
    pub component: String,
    #[serde(rename = "@visual")]
    pub visual: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssemblyModelRoot {
    #[serde(rename = "@assembly")]
    pub assembly: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoot {
    #[serde(rename = "$value")]
    pub root: ModelRootChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelRootChoice {
    #[serde(rename = "component-visual")]
    ComponentVisual(ComponentVisualRoot),
    #[serde(rename = "assembly-model")]
    AssemblyModel(AssemblyModelRoot),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPartRepresentation {
    #[serde(rename = "@node-path")]
    pub node_path: String,
    #[serde(
        rename = "@submesh-fallback",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub submesh_fallback: Option<String>,
    #[serde(rename = "model-root")]
    pub model_root: ModelRoot,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorldRouteFrame {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentOriginFrame {
    #[serde(rename = "@component")]
    pub component: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentNamedFrame {
    #[serde(rename = "@component")]
    pub component: String,
    #[serde(rename = "@frame")]
    pub frame: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteFrame {
    #[serde(rename = "$value")]
    pub frame: RouteFrameChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RouteFrameChoice {
    #[serde(rename = "world")]
    World(WorldRouteFrame),
    #[serde(rename = "component-origin")]
    ComponentOrigin(ComponentOriginFrame),
    #[serde(rename = "component-frame")]
    ComponentFrame(ComponentNamedFrame),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpyRotation {
    #[serde(rename = "@value", with = "vec3_attribute")]
    pub value: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuaternionRotation {
    #[serde(rename = "@value", with = "quat_attribute")]
    pub value: [f64; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementRotation {
    #[serde(rename = "$value")]
    pub rotation: PlacementRotationChoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlacementRotationChoice {
    #[serde(rename = "rpy")]
    Rpy(RpyRotation),
    #[serde(rename = "quaternion")]
    Quaternion(QuaternionRotation),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    #[serde(rename = "@xyz", with = "vec3_attribute")]
    pub xyz: [f64; 3],
    pub frame: RouteFrame,
    pub rotation: PlacementRotation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutePoint {
    #[serde(rename = "@xyz", with = "vec3_attribute")]
    pub xyz: [f64; 3],
    pub frame: RouteFrame,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<PlacementRotation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundRouteSection {
    #[serde(rename = "@diameter")]
    pub diameter: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RectangularRouteSection {
    #[serde(rename = "@width")]
    pub width: f64,
    #[serde(rename = "@height")]
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RouteSectionChoice {
    #[serde(rename = "round-section")]
    Round(RoundRouteSection),
    #[serde(rename = "rectangular-section")]
    Rectangular(RectangularRouteSection),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DerivedRouteRepresentation {
    pub section: Option<RouteSectionChoice>,
    pub waypoint: Vec<RoutePoint>,
}

#[derive(Serialize, Deserialize)]
struct DerivedRouteRaw {
    #[serde(
        rename = "round-section",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    round_section: Option<RoundRouteSection>,
    #[serde(
        rename = "rectangular-section",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    rectangular_section: Option<RectangularRouteSection>,
    #[serde(rename = "waypoint", default, skip_serializing_if = "Vec::is_empty")]
    waypoint: Vec<RoutePoint>,
}

impl Serialize for DerivedRouteRepresentation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (round_section, rectangular_section) = match &self.section {
            None => (None, None),
            Some(RouteSectionChoice::Round(section)) => (Some(section.clone()), None),
            Some(RouteSectionChoice::Rectangular(section)) => (None, Some(section.clone())),
        };
        DerivedRouteRaw {
            round_section,
            rectangular_section,
            waypoint: self.waypoint.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DerivedRouteRepresentation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = DerivedRouteRaw::deserialize(deserializer)?;
        let section = match (raw.round_section, raw.rectangular_section) {
            (None, None) => None,
            (Some(section), None) => Some(RouteSectionChoice::Round(section)),
            (None, Some(section)) => Some(RouteSectionChoice::Rectangular(section)),
            (Some(_), Some(_)) => {
                return Err(serde::de::Error::custom(
                    "derived-route may contain at most one round-section or rectangular-section",
                ));
            }
        };
        Ok(Self {
            section,
            waypoint: raw.waypoint,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Representation {
    #[serde(rename = "$value")]
    pub variant: RepresentationChoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RepresentationChoice {
    #[serde(rename = "box")]
    Box(PrimitiveBox),
    #[serde(rename = "cylinder")]
    Cylinder(PrimitiveCylinder),
    #[serde(rename = "sphere")]
    Sphere(PrimitiveSphere),
    #[serde(rename = "model")]
    Model(ModelRepresentation),
    #[serde(rename = "model-part")]
    ModelPart(ModelPartRepresentation),
    #[serde(rename = "derived-route")]
    DerivedRoute(DerivedRouteRepresentation),
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Position {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@role", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(
        rename = "@local-group",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub local_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Connector {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@family", default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pin: Vec<Position>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub socket: Vec<Position>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contact: Vec<Position>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fiber: Vec<Position>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passage: Vec<Position>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feed: Vec<Position>,
    #[serde(
        rename = "waveguide-opening",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub waveguide_opening: Vec<Position>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Antenna {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(
        rename = "conducted-port",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub conducted_port: Option<PortRef>,
    #[serde(rename = "radiated-port")]
    pub radiated_port: PortRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Function {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<FunctionalEndpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<FunctionalEndpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bidirectional: Vec<FunctionalEndpointRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Path {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@fidelity")]
    pub fidelity: Fidelity,
    #[serde(rename = "@role", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(
        rename = "@local-group",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub local_group: Option<String>,
    pub first: PhysicalEndpointRef,
    pub second: PhysicalEndpointRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Junction {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@fidelity")]
    pub fidelity: Fidelity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment: Vec<PhysicalEndpointRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedQuantity {
    #[serde(rename = "@property")]
    pub property: String,
    #[serde(rename = "@value")]
    pub value: f64,
    #[serde(rename = "@unit")]
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Termination {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@kind")]
    pub kind: String,
    #[serde(rename = "@mounting")]
    pub mounting: super::connectivity::TerminationMounting,
    #[serde(rename = "@fidelity")]
    pub fidelity: Fidelity,
    #[serde(rename = "@profile", default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment: Vec<PhysicalEndpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quantity: Vec<NamedQuantity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Assembly {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connector: Vec<Connector>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wire: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conductor: Vec<Path>,
    #[serde(
        rename = "cable-member",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub cable_member: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fiber: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coax: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waveguide: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feed: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hose: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipe: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passage: Vec<Path>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub splice: Vec<Junction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tee: Vec<Junction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manifold: Vec<Junction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub busbar: Vec<Junction>,
    #[serde(
        rename = "optical-splitter",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub optical_splitter: Vec<Junction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub termination: Vec<Termination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representation: Option<Representation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@fidelity")]
    pub fidelity: Fidelity,
    pub functional: FunctionalEndpointRef,
    pub physical: PhysicalEndpointRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionMapping {
    pub first: PositionRef,
    pub second: PositionRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mate {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@fidelity")]
    pub fidelity: Fidelity,
    pub first: ConnectorRef,
    pub second: ConnectorRef,
    #[serde(
        rename = "position-mapping",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub position_mapping: Vec<PositionMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HopOwnerRef {
    #[serde(rename = "$value")]
    pub owner: HopOwnerChoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HopOwnerChoice {
    #[serde(rename = "component-ref")]
    Component(ComponentRef),
    #[serde(rename = "function-ref")]
    Function(ConnectivityFunctionRef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@role", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub endpoint: FunctionalEndpointRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hop {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@role", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(
        rename = "@processing-delay-ns",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub processing_delay_ns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub owner: HopOwnerRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegEnd {
    #[serde(rename = "hop-ref")]
    pub hop: HopRef,
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leg {
    #[serde(rename = "@name")]
    pub name: String,
    pub from: LegEnd,
    pub to: LegEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParticipantReference {
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HopReference {
    #[serde(rename = "hop-ref")]
    pub hop: HopRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GptpClock {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@kind")]
    pub kind: GptpClockKind,
    #[serde(rename = "@gm-capable")]
    pub gm_capable: bool,
    #[serde(
        rename = "@priority1",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub priority1: Option<u8>,
    #[serde(
        rename = "@priority2",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub priority2: Option<u8>,
    #[serde(
        rename = "@clock-class",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub clock_class: Option<u8>,
    #[serde(
        rename = "@clock-accuracy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub clock_accuracy: Option<u8>,
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GptpPortDefaults {
    #[serde(
        rename = "@log-sync-interval",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub log_sync_interval: Option<i8>,
    #[serde(
        rename = "@log-announce-interval",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub log_announce_interval: Option<i8>,
    #[serde(
        rename = "@log-pdelay-req-interval",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub log_pdelay_req_interval: Option<i8>,
    #[serde(
        rename = "@announce-receipt-timeout",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub announce_receipt_timeout: Option<u8>,
    #[serde(
        rename = "@neighbor-prop-delay-threshold-ns",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub neighbor_prop_delay_threshold_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GptpDomain {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@number")]
    pub number: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clock: Vec<GptpClock>,
    #[serde(
        rename = "port-defaults",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub port_defaults: Option<GptpPortDefaults>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PcpValue {
    #[serde(rename = "@value")]
    pub value: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficClass {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@number")]
    pub number: u8,
    #[serde(
        rename = "@preemption",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub preemption: Option<TrafficPreemption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub pcp: Vec<PcpValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTrafficClasses {
    #[serde(
        rename = "traffic-class-ref",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub traffic_class: Vec<TrafficClassRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateControlEntry {
    #[serde(rename = "@duration-ns")]
    pub duration_ns: u64,
    pub open: OpenTrafficClasses,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateSchedule {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@cycle-time-ns")]
    pub cycle_time_ns: u64,
    #[serde(rename = "gate")]
    pub gate: Vec<GateControlEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleTarget {
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleAssignment {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "schedule-ref")]
    pub schedule: ScheduleRef,
    #[serde(rename = "target", default, skip_serializing_if = "Vec::is_empty")]
    pub target: Vec<ScheduleTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlcaNode {
    #[serde(rename = "@id")]
    pub id: u8,
    #[serde(rename = "@burst-count")]
    pub burst_count: u8,
    #[serde(rename = "@burst-timer-bit-times")]
    pub burst_timer_bit_times: u16,
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlcaConfiguration {
    #[serde(rename = "@max-node-id")]
    pub max_node_id: u8,
    #[serde(rename = "@to-timer-bit-times")]
    pub to_timer_bit_times: u16,
    #[serde(rename = "node")]
    pub node: Vec<PlcaNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacsecPolicyDefinition {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@enforcement")]
    pub enforcement: MacsecEnforcement,
    #[serde(rename = "@cipher", default, skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
    #[serde(
        rename = "@key-agreement",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub key_agreement: Option<String>,
    #[serde(
        rename = "@confidentiality-offset",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub confidentiality_offset: Option<u8>,
    #[serde(
        rename = "@rekey-interval-ns",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub rekey_interval_ns: Option<u64>,
    #[serde(
        rename = "@credential-store-ref",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub credential_store_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacsecDefaultPolicy {
    #[serde(rename = "macsec-policy-ref")]
    pub policy: MacsecPolicyRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacsecOverride {
    pub target: TopologySegmentRef,
    #[serde(rename = "macsec-policy-ref")]
    pub policy: MacsecPolicyRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacsecConfiguration {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy: Vec<MacsecPolicyDefinition>,
    #[serde(
        rename = "default-policy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub default_policy: Option<MacsecDefaultPolicy>,
    #[serde(rename = "override", default, skip_serializing_if = "Vec::is_empty")]
    pub override_: Vec<MacsecOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EeeOverride {
    #[serde(rename = "@mode")]
    pub mode: EeeMode,
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EeeConfiguration {
    #[serde(rename = "@default-mode")]
    pub default_mode: EeeMode,
    #[serde(rename = "override", default, skip_serializing_if = "Vec::is_empty")]
    pub override_: Vec<EeeOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NetworkConfiguration {
    #[serde(rename = "gptp-domain", default, skip_serializing_if = "Vec::is_empty")]
    pub gptp_domain: Vec<GptpDomain>,
    #[serde(
        rename = "traffic-class",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub traffic_class: Vec<TrafficClass>,
    #[serde(
        rename = "gate-schedule",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub gate_schedule: Vec<GateSchedule>,
    #[serde(
        rename = "schedule-assignment",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub schedule_assignment: Vec<ScheduleAssignment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plca: Option<PlcaConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macsec: Option<MacsecConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eee: Option<EeeConfiguration>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<NetworkSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<NetworkConfiguration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant: Vec<Participant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<NetworkSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<NetworkConfiguration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant: Vec<Participant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chain {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<NetworkSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<NetworkConfiguration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant: Vec<Participant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hop: Vec<Hop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leg: Vec<Leg>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Star {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<NetworkSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<NetworkConfiguration>,
    pub coordinator: ParticipantReference,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant: Vec<Participant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ring {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<NetworkSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<NetworkConfiguration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant: Vec<Participant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hop: Vec<Hop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leg: Vec<Leg>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<NetworkSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<NetworkConfiguration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant: Vec<Participant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<NetworkSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<NetworkConfiguration>,
    pub root: HopReference,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant: Vec<Participant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hop: Vec<Hop>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leg: Vec<Leg>,
}

pub(crate) mod quat_attribute {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[f64; 4], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(
            &value
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[f64; 4], D::Error> {
        let text = String::deserialize(deserializer)?;
        let values = text
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(serde::de::Error::custom)?;
        values.try_into().map_err(|values: Vec<f64>| {
            serde::de::Error::custom(format!("expected 4 numbers, got {}", values.len()))
        })
    }
}

pub(crate) mod vec3_attribute {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[f64; 3], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(
            &value
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[f64; 3], D::Error> {
        let text = String::deserialize(deserializer)?;
        let values = text
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(serde::de::Error::custom)?;
        values.try_into().map_err(|values: Vec<f64>| {
            serde::de::Error::custom(format!("expected 3 numbers, got {}", values.len()))
        })
    }
}
