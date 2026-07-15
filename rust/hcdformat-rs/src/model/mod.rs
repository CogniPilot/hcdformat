//! Typed model aligned to the official `hcdf.xsd` (frozen 1.0).
//!
//! One module per schema region. Element/attribute names, optional-vs-list cardinality, and hyphenated
//! names mirror the generated Python model (`hcdfdom/model.py`) and the XSD. Numeric *text* content
//! (mass, inertia, primitive dimensions, limits) is kept as `String` so authored values round-trip
//! exactly without float reformatting; structured numeric *attributes* (pose xyz/rpy/quat) are typed.
//!
//! Core connectivity uses an explicit shape gate, so misspelled or retired connectivity elements and
//! attributes fail instead of disappearing during serde deserialization. Other modeled regions retain
//! their existing forward-compatible behavior. Two side-channels ride outside the serde shape:
//! `<extension>` bodies are preserved verbatim (see [`extension`] and the `de`/`ser` modules) and XML
//! comments are preserved with structural anchors (see [`crate::comments`]).
mod pose;
pub use pose::Pose;

pub mod collision;
pub mod common;
pub mod connectivity;
mod connectivity_convert;
pub mod connectivity_xml;
pub mod dynamic_surface;
pub mod enums;
pub mod extension;
pub mod frame;
pub mod geometry;
pub mod group;
pub mod hmi;
pub mod include;
pub mod inertial;
pub mod joint;
pub mod motor;
pub mod power;
pub mod sensor;
pub mod sensor_params;
pub mod software;
pub mod stream_profile;
pub mod transmission;
pub mod visual;

pub use collision::{Collision, Surface};
pub use common::{
    Color, Contact, Friction, FrictionDirection, MeasuredValue, RangeValue, RatedValue,
};
pub use connectivity_convert::{ConnectivityConversionError, HcdfConnectivityError};
pub use connectivity_xml::{
    Antenna, Assembly, AssemblyModelRoot, AssemblyRef, Binding, Bus, Capabilities,
    CarrierCapability, Chain, Channel, ChannelRef, ComponentNamedFrame, ComponentOriginFrame,
    ComponentRef, ComponentVisualRoot, ConnectivityFunctionRef, Connector, ConnectorRef,
    DerivedRouteRepresentation, EeeConfiguration, EeeOverride, Function, FunctionalEndpointChoice,
    FunctionalEndpointRef, GateControlEntry, GateSchedule, GptpClock, GptpDomain, GptpPortDefaults,
    Hop, HopOwnerChoice, HopOwnerRef, HopRef, HopReference, InstanceRef, InstanceSegment, Junction,
    JunctionRef, Leg, LegEnd, LegRef, Link, MacsecConfiguration, MacsecDefaultPolicy,
    MacsecOverride, MacsecPolicyDefinition, MacsecPolicyRef, Mate, Mesh as ConnectivityMesh,
    ModelPartRepresentation as ConnectivityModelPartRepresentation,
    ModelRepresentation as ConnectivityModelRepresentation, ModelRoot, ModelRootChoice,
    NetworkConfiguration, NetworkRef, NetworkSelection, OpenTrafficClasses, Participant,
    ParticipantRef, ParticipantReference, Path, PcpValue, PhysicalEndpointChoice,
    PhysicalEndpointRef, PhysicalOwnerChoice, Placement, PlacementRotation,
    PlacementRotationChoice, PlcaConfiguration, PlcaNode, Port, PortRef, Position, PositionMapping,
    PositionRef, PrimitiveBox, PrimitiveCylinder, PrimitiveSphere, ProfileCapability,
    PurposeCapability, Quantity as ConnectivityQuantity,
    QuantityRange as ConnectivityQuantityRange, QuaternionRotation, RectangularRouteSection,
    Representation, RepresentationChoice, RfChannelSelection, RfChannelSelectionChoice,
    RfFrequencyDefinedChannel, RfNumberedChannel, RfSelection, Ring, RoundRouteSection, RouteFrame,
    RouteFrameChoice, RoutePoint, RouteSectionChoice, RpyRotation, ScheduleAssignment, ScheduleRef,
    ScheduleTarget, SelectionQuantity as ConnectivitySelectionQuantity,
    SelectionQuantityChoice as ConnectivitySelectionQuantityChoice,
    SelectionRange as ConnectivitySelectionRange, Star, Termination, TopologySegmentChoice,
    TopologySegmentRef, TrafficClass, TrafficClassRef, Tree, WorldRouteFrame,
};
pub use dynamic_surface::{
    AerofoilSurface, ControlSurface, DynamicSurface, GripperSurface, HydrofoilSurface, PropSurface,
    TrackSurface, WheelSurface,
};
pub use enums::{BodyFrame, CompRole, WorldFrame};
pub use extension::Extension;
pub use frame::Frame;
pub use geometry::{
    Box_, Capsule, CollisionGeometry, Cone, Cylinder, Ellipsoid, ExcludeSubmesh, Frustum, Geometry,
    Mesh, Sphere, Submesh, VisualGeometry,
};
pub use group::{
    CollisionPair, JointGroup, JointPosition, KinematicState, Ref, SelfCollisionDisable,
};
pub use hmi::{HmiElement, LedIllumination, LightAttenuation};
pub use include::Include;
pub use inertial::Inertial;
pub use joint::{
    Axis, Joint, JointCalibration, JointDynamics, JointEndpoint, JointLimit, LoopClosure, Mimic,
    SafetyController, SwingLimit, UrdfCompatJoint,
};
pub use motor::{ControlModes, Motor};
pub use power::{
    BatterySource, FlowCharacterization, FuelCell, PowerSource, SolarSource, SupercapacitorSource,
    TankSource,
};
pub use sensor::{
    AxisAlign, AxisNoise, FluidProbe, FluidSensor, ForceAxisNoise, InertialSensor, Noise,
    OpticalSensor, Sensor, SensorCategory, SensorDriver, SensorFov, SensorParams,
};
pub use sensor_params::{
    CameraBinning, CameraCalibration, CameraDistortion, CameraIntrinsics, CameraLens,
    CameraLensCustomFunction, CameraMatrix, CameraParams, CameraProjection, CameraRoi, DataOutput,
    FifoConfig, GnssParams, LidarParams, LidarRange, LidarScanAxis, LidarScanPattern, RadarParams,
};
pub use software::{Discovered, Software, UrdfCompatComp};
pub use stream_profile::{
    Frer, FrerSequenceEncoding, StreamDefinition, StreamForwarding, StreamForwardingEnd,
    StreamGroup, StreamGroupRef, StreamListener, StreamPath, StreamProfileDependency,
    StreamProfileDocument, StreamProfileResource, StreamProfileSelectionRole, StreamTalker,
};
pub use transmission::{Endpoint, Transmission, TransmissionSpring};
pub use visual::{ModelRef, Visual, VisualAppearance};

use serde::{Deserialize, Serialize};

/// Root `<hcdf>` element: identity metadata + the full official child sequence.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "hcdf")]
pub struct Hcdf {
    #[serde(rename = "@name", default)]
    pub name: String,
    // PRESENCE-preserving like `hcdfdom.model.Hcdf.version` (a `None`-defaulting string): an absent
    // `@version` stays empty and is NOT re-emitted, so a Python-produced HCDF (which omits `@version`)
    // round-trips without a fabricated version, and `to_urdf` reports a version loss only when the doc
    // actually carries one (matching to_urdf.py, whose loss loop guards on `version is not None`).
    #[serde(rename = "@version", default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    #[serde(
        rename = "@body-frame",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub body_frame: Option<BodyFrame>,
    #[serde(
        rename = "@world-frame",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub world_frame: Option<WorldFrame>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comp: Vec<Comp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joint: Vec<Joint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group: Vec<JointGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state: Vec<KinematicState>,
    #[serde(
        rename = "self-collision-disable",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub self_collision_disable: Option<SelfCollisionDisable>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harness: Vec<Assembly>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cable: Vec<Assembly>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plumbing: Vec<Assembly>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub umbilical: Vec<Assembly>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binding: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mate: Vec<Mate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bus: Vec<Bus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<Chain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub star: Vec<Star>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ring: Vec<Ring>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mesh: Vec<ConnectivityMesh>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree: Vec<Tree>,
    #[serde(
        rename = "stream-profile",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub stream_profile: Vec<StreamProfileResource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transmission: Vec<Transmission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub color: Vec<Color>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<Include>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extension: Vec<Extension>,

    /// Doc-level XML comments captured for round-trip preservation ([`crate::comments`]). NEVER part
    /// of the schema/serde shape (equality-transparent; spliced back by [`Hcdf::to_xml_string`]).
    #[serde(skip)]
    pub comments: crate::comments::DocComments,
}

/// `<comp>`: a rigid body / hardware assembly. Full official child sequence + attributes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "comp")]
pub struct Comp {
    #[serde(rename = "@name", default)]
    pub name: String,
    #[serde(rename = "@role", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<CompRole>,
    #[serde(rename = "@hwid", default, skip_serializing_if = "Option::is_none")]
    pub hwid: Option<String>,
    #[serde(
        rename = "@struct-type",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub struct_type: Option<String>,
    #[serde(
        rename = "@ip-rating",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ip_rating: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    #[serde(
        rename = "operating-temp",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub operating_temp: Option<RangeValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inertial: Option<Inertial>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visual: Vec<Visual>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collision: Vec<Collision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frame: Vec<Frame>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port: Vec<Port>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connector: Vec<Connector>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub antenna: Vec<Antenna>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switch: Vec<Function>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bridge: Vec<Function>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub converter: Vec<Function>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transceiver: Vec<Function>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub radio: Vec<Function>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensor: Vec<Sensor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub motor: Vec<Motor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hmi: Vec<HmiElement>,
    #[serde(
        rename = "dynamic-surface",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub dynamic_surface: Vec<DynamicSurface>,
    #[serde(
        rename = "power-source",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub power_source: Vec<PowerSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software: Option<Software>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered: Option<Discovered>,
    #[serde(
        rename = "urdf-compat",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub urdf_compat: Option<UrdfCompatComp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extension: Vec<Extension>,

    /// XML comments captured inside this `<comp>` (comp-relative paths; [`crate::comments`]). NEVER
    /// part of the schema/serde shape (equality-transparent; travels with the struct through slot
    /// replacement and include-flatten).
    #[serde(skip)]
    pub comments: crate::comments::CommentSet,
}
