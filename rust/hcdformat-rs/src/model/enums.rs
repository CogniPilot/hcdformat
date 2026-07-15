// GENERATED from hcdf.xsd by hcdf regen enums. DO NOT EDIT.
// Regenerate: hcdf regen enums hcdf.xsd rust/hcdformat-rs/src/model/enums.rs
// rustfmt must not reformat this file: it is emitted verbatim by the generator and pinned by a
// byte-exact regeneration check, which reformatting would break.
#![cfg_attr(rustfmt, rustfmt::skip)]
//! Typed HCDF enums, generated from `hcdf.xsd` (the single source of truth).
//!
//! One Rust enum per enumerated `xs:simpleType`, each variant `#[serde(rename = "...")]`d to its exact
//! XSD enumeration literal, with `FromStr`/`Display` to/from that literal, and rustdoc carried from the
//! schema. It is one of the XSD-driven generators (alongside the JSON schema, completions and spec).
//!
//! The hand-written model TYPES most enumerated attributes/element-text as `Option<EnumTy>`
//! (or `Vec<EnumTy>`), so an out-of-enum literal is rejected by serde at PARSE, matching Python's
//! load-time `ValueError`. Residual slots that intentionally retain a string field (see
//! [`ENUM_ATTRS`]), including shared sensor-category `@type`, projection/probe `@type`, and
//! transmission endpoint `@role`, are enforced by `crate::validate` during the document walk. The
//! enums serialize to the exact XSD literals.
use serde::{Deserialize, Serialize};

/// Audio/acoustic sensor sub-type. microphone: audio capture for speech, sound localization, acoustic
/// monitoring. ultrasonic: ultrasonic transducer for proximity sensing and ranging (typically 40kHz).
/// sonar: underwater acoustic ranging and imaging. hydrophone: underwater passive acoustic sensor for
/// listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioSensorType {
    #[serde(rename = "microphone")]
    Microphone,
    #[serde(rename = "ultrasonic")]
    Ultrasonic,
    #[serde(rename = "sonar")]
    Sonar,
    #[serde(rename = "hydrophone")]
    Hydrophone,
}

impl AudioSensorType {
    /// The exact XSD enumeration literals accepted for `AudioSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["microphone", "ultrasonic", "sonar", "hydrophone"]
    }
}

impl core::str::FromStr for AudioSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "microphone" => Ok(AudioSensorType::Microphone),
            "ultrasonic" => Ok(AudioSensorType::Ultrasonic),
            "sonar" => Ok(AudioSensorType::Sonar),
            "hydrophone" => Ok(AudioSensorType::Hydrophone),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for AudioSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            AudioSensorType::Microphone => "microphone",
            AudioSensorType::Ultrasonic => "ultrasonic",
            AudioSensorType::Sonar => "sonar",
            AudioSensorType::Hydrophone => "hydrophone",
        })
    }
}

/// Sensor axis alignment mapping. Maps a sensor's internal axis to the component's body frame axis.
/// Supports positive and negative directions: X, -X, Y, -Y, Z, -Z. Used to correct for sensors
/// mounted in non-standard orientations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisValue {
    #[serde(rename = "X")]
    X,
    #[serde(rename = "-X")]
    NegX,
    #[serde(rename = "Y")]
    Y,
    #[serde(rename = "-Y")]
    NegY,
    #[serde(rename = "Z")]
    Z,
    #[serde(rename = "-Z")]
    NegZ,
}

impl AxisValue {
    /// The exact XSD enumeration literals accepted for `AxisValue`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["X", "-X", "Y", "-Y", "Z", "-Z"]
    }
}

impl core::str::FromStr for AxisValue {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "X" => Ok(AxisValue::X),
            "-X" => Ok(AxisValue::NegX),
            "Y" => Ok(AxisValue::Y),
            "-Y" => Ok(AxisValue::NegY),
            "Z" => Ok(AxisValue::Z),
            "-Z" => Ok(AxisValue::NegZ),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for AxisValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            AxisValue::X => "X",
            AxisValue::NegX => "-X",
            AxisValue::Y => "Y",
            AxisValue::NegY => "-Y",
            AxisValue::Z => "Z",
            AxisValue::NegZ => "-Z",
        })
    }
}

/// Body-frame axis convention describing how the robot's local coordinate axes are oriented relative
/// to its physical body. The three-letter code specifies the direction each axis points: first letter
/// = X, second = Y, third = Z. F=Forward, B=Back, L=Left, R=Right, U=Up, D=Down. FLU (X-forward,
/// Y-left, Z-up): common in ground robotics, humanoids, and wheeled platforms. Positive X faces the
/// direction the robot drives or walks. FRD (X-forward, Y-right, Z-down): common in aerospace,
/// aviation, and flight controllers. Matches the aircraft body-frame convention where Z points toward
/// the ground. Choosing a convention affects how all pose xyz and rpy values in the document are
/// interpreted. Semantic rule for composition: all files in an include chain must use the same
/// body-frame convention. Tooling should validate this at include-resolve time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyFrame {
    #[serde(rename = "FLU")]
    FLU,
    #[serde(rename = "FRD")]
    FRD,
}

impl BodyFrame {
    /// The exact XSD enumeration literals accepted for `BodyFrame`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["FLU", "FRD"]
    }
}

impl core::str::FromStr for BodyFrame {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "FLU" => Ok(BodyFrame::FLU),
            "FRD" => Ok(BodyFrame::FRD),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for BodyFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            BodyFrame::FLU => "FLU",
            BodyFrame::FRD => "FRD",
        })
    }
}

/// Distortion model naming the meaning of the camera_distortion coefficient set, matching ROS
/// CameraInfo/distortion_model. plumb_bob: the classic 5-coefficient Brown-Conrady radial/tangential
/// model (k1 k2 p1 p2 k3). rational_polynomial: the 8-coefficient rational model (k1 k2 p1 p2 k3 k4
/// k5 k6) for wide-angle lenses. equidistant: the 4-coefficient fisheye/Kannala-Brandt model (k1..k4
/// used as fisheye terms). Absent => assume plumb_bob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraDistortionModel {
    #[serde(rename = "plumb_bob")]
    PlumbBob,
    #[serde(rename = "rational_polynomial")]
    RationalPolynomial,
    #[serde(rename = "equidistant")]
    Equidistant,
}

impl CameraDistortionModel {
    /// The exact XSD enumeration literals accepted for `CameraDistortionModel`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["plumb_bob", "rational_polynomial", "equidistant"]
    }
}

impl core::str::FromStr for CameraDistortionModel {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "plumb_bob" => Ok(CameraDistortionModel::PlumbBob),
            "rational_polynomial" => Ok(CameraDistortionModel::RationalPolynomial),
            "equidistant" => Ok(CameraDistortionModel::Equidistant),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for CameraDistortionModel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            CameraDistortionModel::PlumbBob => "plumb_bob",
            CameraDistortionModel::RationalPolynomial => "rational_polynomial",
            CameraDistortionModel::Equidistant => "equidistant",
        })
    }
}

/// Lens projection model: how world-ray angles map to image-plane radius, which the Brown-Conrady
/// camera_distortion coefficients CANNOT express for wide-angle/fisheye optics. pinhole: rectilinear
/// (gnomonical) projection r = f*tan(theta); the standard perspective model the intrinsics/distortion
/// already assume. stereographic: r = 2f*tan(theta/2), conformal. equidistant: r = f*theta, constant
/// angular resolution (common fisheye). equisolid: r = 2f*sin(theta/2), equal-area (many circular
/// fisheyes). orthographic: r = f*sin(theta), 180deg hemispherical. custom: mapping given by the
/// custom-function coefficients. theta is the angle from the optical axis; f is focal length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraProjectionType {
    #[serde(rename = "pinhole")]
    Pinhole,
    #[serde(rename = "stereographic")]
    Stereographic,
    #[serde(rename = "equidistant")]
    Equidistant,
    #[serde(rename = "equisolid")]
    Equisolid,
    #[serde(rename = "orthographic")]
    Orthographic,
    #[serde(rename = "custom")]
    Custom,
}

impl CameraProjectionType {
    /// The exact XSD enumeration literals accepted for `CameraProjectionType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["pinhole", "stereographic", "equidistant", "equisolid", "orthographic", "custom"]
    }
}

impl core::str::FromStr for CameraProjectionType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pinhole" => Ok(CameraProjectionType::Pinhole),
            "stereographic" => Ok(CameraProjectionType::Stereographic),
            "equidistant" => Ok(CameraProjectionType::Equidistant),
            "equisolid" => Ok(CameraProjectionType::Equisolid),
            "orthographic" => Ok(CameraProjectionType::Orthographic),
            "custom" => Ok(CameraProjectionType::Custom),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for CameraProjectionType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            CameraProjectionType::Pinhole => "pinhole",
            CameraProjectionType::Stereographic => "stereographic",
            CameraProjectionType::Equidistant => "equidistant",
            CameraProjectionType::Equisolid => "equisolid",
            CameraProjectionType::Orthographic => "orthographic",
            CameraProjectionType::Custom => "custom",
        })
    }
}

/// Chemical/environmental sensor sub-type. gas: gas concentration sensor (CO2, VOC, methane, etc.).
/// ph: pH sensor for liquid acidity measurement. humidity: relative humidity sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChemicalSensorType {
    #[serde(rename = "gas")]
    Gas,
    #[serde(rename = "ph")]
    Ph,
    #[serde(rename = "humidity")]
    Humidity,
}

impl ChemicalSensorType {
    /// The exact XSD enumeration literals accepted for `ChemicalSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["gas", "ph", "humidity"]
    }
}

impl core::str::FromStr for ChemicalSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gas" => Ok(ChemicalSensorType::Gas),
            "ph" => Ok(ChemicalSensorType::Ph),
            "humidity" => Ok(ChemicalSensorType::Humidity),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for ChemicalSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ChemicalSensorType::Gas => "gas",
            ChemicalSensorType::Ph => "ph",
            ChemicalSensorType::Humidity => "humidity",
        })
    }
}

/// Functional role of a component in the robot. sensor: sensing-only device (e.g., camera module, IMU
/// board). compute: processing unit (e.g., SoC, SBC, FPGA). actuator: motor driver or actuator
/// assembly. parent: structural/mechanical parent that groups child components (e.g., chassis, arm
/// segment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompRole {
    #[serde(rename = "sensor")]
    Sensor,
    #[serde(rename = "compute")]
    Compute,
    #[serde(rename = "actuator")]
    Actuator,
    #[serde(rename = "parent")]
    Parent,
}

impl CompRole {
    /// The exact XSD enumeration literals accepted for `CompRole`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["sensor", "compute", "actuator", "parent"]
    }
}

impl core::str::FromStr for CompRole {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sensor" => Ok(CompRole::Sensor),
            "compute" => Ok(CompRole::Compute),
            "actuator" => Ok(CompRole::Actuator),
            "parent" => Ok(CompRole::Parent),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for CompRole {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            CompRole::Sensor => "sensor",
            CompRole::Compute => "compute",
            CompRole::Actuator => "actuator",
            CompRole::Parent => "parent",
        })
    }
}

/// Near-field electromagnetic sensor sub-type. Measures static or quasi-static EM fields, NOT
/// propagating radio waves (those are rf). mag: magnetometer (measures Earth's magnetic field or
/// local fields). metal_detector: inductive proximity sensing. eddy_current: non-destructive testing
/// via induced currents. emf: electromotive force measurement. hall-effect: Hall effect
/// proximity/position sensor. fluxgate: fluxgate magnetometer (high-precision compass).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmSensorType {
    #[serde(rename = "mag")]
    Mag,
    #[serde(rename = "metal_detector")]
    MetalDetector,
    #[serde(rename = "eddy_current")]
    EddyCurrent,
    #[serde(rename = "emf")]
    Emf,
    #[serde(rename = "hall-effect")]
    HallEffect,
    #[serde(rename = "fluxgate")]
    Fluxgate,
}

impl EmSensorType {
    /// The exact XSD enumeration literals accepted for `EmSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["mag", "metal_detector", "eddy_current", "emf", "hall-effect", "fluxgate"]
    }
}

impl core::str::FromStr for EmSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mag" => Ok(EmSensorType::Mag),
            "metal_detector" => Ok(EmSensorType::MetalDetector),
            "eddy_current" => Ok(EmSensorType::EddyCurrent),
            "emf" => Ok(EmSensorType::Emf),
            "hall-effect" => Ok(EmSensorType::HallEffect),
            "fluxgate" => Ok(EmSensorType::Fluxgate),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for EmSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            EmSensorType::Mag => "mag",
            EmSensorType::MetalDetector => "metal_detector",
            EmSensorType::EddyCurrent => "eddy_current",
            EmSensorType::Emf => "emf",
            EmSensorType::HallEffect => "hall-effect",
            EmSensorType::Fluxgate => "fluxgate",
        })
    }
}

/// Physical sensing principle of the encoder. optical: light source + photodetector + code disk
/// (e.g., US Digital, Broadcom AEDR). magnetic: Hall effect or magnetoresistive element + rotating
/// magnet (e.g., AS5048A, AS5600). inductive: resolver, electromagnetic induction between coils.
/// capacitive: capacitance change between rotating plates. resistive: potentiometer, resistance
/// proportional to position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncoderPrinciple {
    #[serde(rename = "optical")]
    Optical,
    #[serde(rename = "magnetic")]
    Magnetic,
    #[serde(rename = "inductive")]
    Inductive,
    #[serde(rename = "capacitive")]
    Capacitive,
    #[serde(rename = "resistive")]
    Resistive,
}

impl EncoderPrinciple {
    /// The exact XSD enumeration literals accepted for `EncoderPrinciple`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["optical", "magnetic", "inductive", "capacitive", "resistive"]
    }
}

impl core::str::FromStr for EncoderPrinciple {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "optical" => Ok(EncoderPrinciple::Optical),
            "magnetic" => Ok(EncoderPrinciple::Magnetic),
            "inductive" => Ok(EncoderPrinciple::Inductive),
            "capacitive" => Ok(EncoderPrinciple::Capacitive),
            "resistive" => Ok(EncoderPrinciple::Resistive),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for EncoderPrinciple {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            EncoderPrinciple::Optical => "optical",
            EncoderPrinciple::Magnetic => "magnetic",
            EncoderPrinciple::Inductive => "inductive",
            EncoderPrinciple::Capacitive => "capacitive",
            EncoderPrinciple::Resistive => "resistive",
        })
    }
}

/// Encoder measurement type. incremental: relative position from index/home, requires homing.
/// absolute: absolute position within one revolution (single-turn) or multiple revolutions
/// (multi-turn). linear: linear position encoder (e.g., optical strip, magnetic linear).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncoderSensorType {
    #[serde(rename = "incremental")]
    Incremental,
    #[serde(rename = "absolute")]
    Absolute,
    #[serde(rename = "linear")]
    Linear,
}

impl EncoderSensorType {
    /// The exact XSD enumeration literals accepted for `EncoderSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["incremental", "absolute", "linear"]
    }
}

impl core::str::FromStr for EncoderSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "incremental" => Ok(EncoderSensorType::Incremental),
            "absolute" => Ok(EncoderSensorType::Absolute),
            "linear" => Ok(EncoderSensorType::Linear),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for EncoderSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            EncoderSensorType::Incremental => "incremental",
            EncoderSensorType::Absolute => "absolute",
            EncoderSensorType::Linear => "linear",
        })
    }
}

/// Fluid sensor sub-type: the measurand is fluid (gas or liquid) pressure or flow, as distinct from
/// force (solid/mechanical contact loads) and chemical (composition). Application sub-types:
/// barometer (absolute atmospheric pressure, derives altitude), airspeed (differential/dynamic
/// pressure from a pitot probe, derives airspeed), depth (hydrostatic pressure, derives submersion
/// depth), flow (volumetric or mass flow rate), level (liquid level via hydrostatic head).
/// Reference-mode sub-types (name the pressure reference of a general-purpose transducer):
/// differential (across two ports), gauge (relative to ambient), absolute (relative to vacuum),
/// sealed (relative to a sealed reference, typically 1 atm), vacuum (below-ambient/vacuum
/// measurement). Choose the application sub-type when the device has a defined purpose; choose the
/// reference-mode sub-type for a general-purpose pressure transducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FluidSensorType {
    #[serde(rename = "barometer")]
    Barometer,
    #[serde(rename = "airspeed")]
    Airspeed,
    #[serde(rename = "depth")]
    Depth,
    #[serde(rename = "flow")]
    Flow,
    #[serde(rename = "level")]
    Level,
    #[serde(rename = "differential")]
    Differential,
    #[serde(rename = "gauge")]
    Gauge,
    #[serde(rename = "absolute")]
    Absolute,
    #[serde(rename = "sealed")]
    Sealed,
    #[serde(rename = "vacuum")]
    Vacuum,
}

impl FluidSensorType {
    /// The exact XSD enumeration literals accepted for `FluidSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["barometer", "airspeed", "depth", "flow", "level", "differential", "gauge", "absolute", "sealed", "vacuum"]
    }
}

impl core::str::FromStr for FluidSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "barometer" => Ok(FluidSensorType::Barometer),
            "airspeed" => Ok(FluidSensorType::Airspeed),
            "depth" => Ok(FluidSensorType::Depth),
            "flow" => Ok(FluidSensorType::Flow),
            "level" => Ok(FluidSensorType::Level),
            "differential" => Ok(FluidSensorType::Differential),
            "gauge" => Ok(FluidSensorType::Gauge),
            "absolute" => Ok(FluidSensorType::Absolute),
            "sealed" => Ok(FluidSensorType::Sealed),
            "vacuum" => Ok(FluidSensorType::Vacuum),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for FluidSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            FluidSensorType::Barometer => "barometer",
            FluidSensorType::Airspeed => "airspeed",
            FluidSensorType::Depth => "depth",
            FluidSensorType::Flow => "flow",
            FluidSensorType::Level => "level",
            FluidSensorType::Differential => "differential",
            FluidSensorType::Gauge => "gauge",
            FluidSensorType::Absolute => "absolute",
            FluidSensorType::Sealed => "sealed",
            FluidSensorType::Vacuum => "vacuum",
        })
    }
}

/// Reference frame in which a force/torque sensor reports its wrench, matching SDF
/// force_torque/frame. parent: the joint parent link frame. child: the joint child link frame.
/// sensor: the sensor's own frame. Absent => the simulator/hardware default (SDF defaults to
/// "child").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForceFrame {
    #[serde(rename = "parent")]
    Parent,
    #[serde(rename = "child")]
    Child,
    #[serde(rename = "sensor")]
    Sensor,
}

impl ForceFrame {
    /// The exact XSD enumeration literals accepted for `ForceFrame`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["parent", "child", "sensor"]
    }
}

impl core::str::FromStr for ForceFrame {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "parent" => Ok(ForceFrame::Parent),
            "child" => Ok(ForceFrame::Child),
            "sensor" => Ok(ForceFrame::Sensor),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for ForceFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ForceFrame::Parent => "parent",
            ForceFrame::Child => "child",
            ForceFrame::Sensor => "sensor",
        })
    }
}

/// Force/torque sensor sub-type. strain: strain gauge for deformation measurement. pressure: pressure
/// transducer (barometric or contact). torque: rotary torque sensor. load_cell: load cell for
/// weight/force measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForceSensorType {
    #[serde(rename = "strain")]
    Strain,
    #[serde(rename = "pressure")]
    Pressure,
    #[serde(rename = "torque")]
    Torque,
    #[serde(rename = "load_cell")]
    LoadCell,
}

impl ForceSensorType {
    /// The exact XSD enumeration literals accepted for `ForceSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["strain", "pressure", "torque", "load_cell"]
    }
}

impl core::str::FromStr for ForceSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "strain" => Ok(ForceSensorType::Strain),
            "pressure" => Ok(ForceSensorType::Pressure),
            "torque" => Ok(ForceSensorType::Torque),
            "load_cell" => Ok(ForceSensorType::LoadCell),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for ForceSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ForceSensorType::Strain => "strain",
            ForceSensorType::Pressure => "pressure",
            ForceSensorType::Torque => "torque",
            ForceSensorType::LoadCell => "load_cell",
        })
    }
}

/// Cross-section shape of a frustum used for sensor field-of-view visualization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrustumShape {
    #[serde(rename = "conical")]
    Conical,
    #[serde(rename = "pyramidal")]
    Pyramidal,
}

impl FrustumShape {
    /// The exact XSD enumeration literals accepted for `FrustumShape`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["conical", "pyramidal"]
    }
}

impl core::str::FromStr for FrustumShape {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "conical" => Ok(FrustumShape::Conical),
            "pyramidal" => Ok(FrustumShape::Pyramidal),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for FrustumShape {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            FrustumShape::Conical => "conical",
            FrustumShape::Pyramidal => "pyramidal",
        })
    }
}

/// Human-Machine Interface element type. display: visual output (LCD, OLED, LED matrix, e-ink).
/// speaker: audio output (speaker, buzzer, piezo). led-status: status indicator LED(s) (on/off,
/// color). led-illumination: scene illumination (headlights, work lights, IR illumination for night
/// vision cameras). button: physical pushbutton or switch input. touchscreen: combined display +
/// touch input. indicator: generic visual/audio indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HmiType {
    #[serde(rename = "display")]
    Display,
    #[serde(rename = "speaker")]
    Speaker,
    #[serde(rename = "buzzer")]
    Buzzer,
    #[serde(rename = "led-status")]
    LedStatus,
    #[serde(rename = "led-illumination")]
    LedIllumination,
    #[serde(rename = "button")]
    Button,
    #[serde(rename = "touchscreen")]
    Touchscreen,
    #[serde(rename = "indicator")]
    Indicator,
}

impl HmiType {
    /// The exact XSD enumeration literals accepted for `HmiType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["display", "speaker", "buzzer", "led-status", "led-illumination", "button", "touchscreen", "indicator"]
    }
}

impl core::str::FromStr for HmiType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "display" => Ok(HmiType::Display),
            "speaker" => Ok(HmiType::Speaker),
            "buzzer" => Ok(HmiType::Buzzer),
            "led-status" => Ok(HmiType::LedStatus),
            "led-illumination" => Ok(HmiType::LedIllumination),
            "button" => Ok(HmiType::Button),
            "touchscreen" => Ok(HmiType::Touchscreen),
            "indicator" => Ok(HmiType::Indicator),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for HmiType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            HmiType::Display => "display",
            HmiType::Speaker => "speaker",
            HmiType::Buzzer => "buzzer",
            HmiType::LedStatus => "led-status",
            HmiType::LedIllumination => "led-illumination",
            HmiType::Button => "button",
            HmiType::Touchscreen => "touchscreen",
            HmiType::Indicator => "indicator",
        })
    }
}

/// Inertial sensor sub-type. accel: accelerometer only (3-axis linear acceleration). gyro: gyroscope
/// only (3-axis angular rate). accel_gyro: combined IMU with both accelerometer and gyroscope
/// (6-axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InertialSensorType {
    #[serde(rename = "accel")]
    Accel,
    #[serde(rename = "gyro")]
    Gyro,
    #[serde(rename = "accel_gyro")]
    AccelGyro,
}

impl InertialSensorType {
    /// The exact XSD enumeration literals accepted for `InertialSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["accel", "gyro", "accel_gyro"]
    }
}

impl core::str::FromStr for InertialSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "accel" => Ok(InertialSensorType::Accel),
            "gyro" => Ok(InertialSensorType::Gyro),
            "accel_gyro" => Ok(InertialSensorType::AccelGyro),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for InertialSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            InertialSensorType::Accel => "accel",
            InertialSensorType::Gyro => "gyro",
            InertialSensorType::AccelGyro => "accel_gyro",
        })
    }
}

/// Enumeration of kinematic joint types. revolute: bounded single-axis rotation (requires axis +
/// limit; radians/Nm). continuous: unbounded single-axis rotation (requires axis; limit is optional
/// and carries effort/velocity ONLY; lower/upper are forbidden). prismatic: bounded linear
/// translation (sliding; requires axis + limit; meters/N). fixed: rigid (zero DOF; axis and limit
/// forbidden). ball: 3-DOF spherical (swing-cone + independent twist; axis, axis2, limit, limit2, and
/// thread_pitch are all forbidden). universal: 2-DOF (requires axis + axis2; limit bounds the primary
/// axis and limit2 bounds axis2, both radians). planar: 2-DOF translation on a plane (requires axis =
/// the plane normal; limit and limit2 are the two in-plane translation ranges, meters). screw:
/// helical, 1-DOF (requires axis + thread_pitch; limit bounds the ROTATIONAL DOF in radians and the
/// coupled axial translation is derived via thread_pitch: canonical pitch is meters-per-revolution,
/// right-handed). cylindrical: 2-DOF rotation + translation on the same axis (like a piston that can
/// also rotate; requires axis + limit; limit is the translation range in meters and limit2, when
/// present, is the rotation range in radians; the rotation bound is optional). free: 6-DOF
/// unconstrained (all translation and rotation allowed, used for the root body of mobile robots,
/// free-flying spacecraft, and any body not kinematically constrained to a parent; axis and limit are
/// forbidden).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JointType {
    #[serde(rename = "revolute")]
    Revolute,
    #[serde(rename = "continuous")]
    Continuous,
    #[serde(rename = "prismatic")]
    Prismatic,
    #[serde(rename = "fixed")]
    Fixed,
    #[serde(rename = "ball")]
    Ball,
    #[serde(rename = "universal")]
    Universal,
    #[serde(rename = "planar")]
    Planar,
    #[serde(rename = "screw")]
    Screw,
    #[serde(rename = "cylindrical")]
    Cylindrical,
    #[serde(rename = "free")]
    Free,
}

impl JointType {
    /// The exact XSD enumeration literals accepted for `JointType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["revolute", "continuous", "prismatic", "fixed", "ball", "universal", "planar", "screw", "cylindrical", "free"]
    }
}

impl core::str::FromStr for JointType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "revolute" => Ok(JointType::Revolute),
            "continuous" => Ok(JointType::Continuous),
            "prismatic" => Ok(JointType::Prismatic),
            "fixed" => Ok(JointType::Fixed),
            "ball" => Ok(JointType::Ball),
            "universal" => Ok(JointType::Universal),
            "planar" => Ok(JointType::Planar),
            "screw" => Ok(JointType::Screw),
            "cylindrical" => Ok(JointType::Cylindrical),
            "free" => Ok(JointType::Free),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for JointType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            JointType::Revolute => "revolute",
            JointType::Continuous => "continuous",
            JointType::Prismatic => "prismatic",
            JointType::Fixed => "fixed",
            JointType::Ball => "ball",
            JointType::Universal => "universal",
            JointType::Planar => "planar",
            JointType::Screw => "screw",
            JointType::Cylindrical => "cylindrical",
            JointType::Free => "free",
        })
    }
}

/// Sign convention for the wrench a force/torque sensor reports, matching SDF
/// force_torque/measure_direction. parent_to_child: the wrench the parent applies to the child.
/// child_to_parent: the wrench the child applies to the parent (the negation). Absent => the
/// simulator default (SDF defaults to "child_to_parent"). Flipping this negates the reported force
/// and torque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasureDirection {
    #[serde(rename = "parent_to_child")]
    ParentToChild,
    #[serde(rename = "child_to_parent")]
    ChildToParent,
}

impl MeasureDirection {
    /// The exact XSD enumeration literals accepted for `MeasureDirection`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["parent_to_child", "child_to_parent"]
    }
}

impl core::str::FromStr for MeasureDirection {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "parent_to_child" => Ok(MeasureDirection::ParentToChild),
            "child_to_parent" => Ok(MeasureDirection::ChildToParent),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for MeasureDirection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            MeasureDirection::ParentToChild => "parent_to_child",
            MeasureDirection::ChildToParent => "child_to_parent",
        })
    }
}

/// Motor or actuator technology type. bldc: brushless DC motor. brushed: brushed DC motor. stepper:
/// stepper motor. servo: hobby/industrial servo. linear: linear actuator (proportional control over
/// stroke length). solenoid: electromagnetic on/off linear actuator (binary pull/push action with no
/// proportional position control, used for valves, latches, locks, pin releases, landing gear, and
/// gripper release mechanisms). hydraulic: hydraulic actuator. pneumatic: pneumatic actuator. thrust:
/// thrust motor (propeller, jet, rocket). ice: internal combustion engine (gasoline, diesel; for
/// generators or direct mechanical drive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MotorType {
    #[serde(rename = "bldc")]
    Bldc,
    #[serde(rename = "brushed")]
    Brushed,
    #[serde(rename = "stepper")]
    Stepper,
    #[serde(rename = "servo")]
    Servo,
    #[serde(rename = "linear")]
    Linear,
    #[serde(rename = "solenoid")]
    Solenoid,
    #[serde(rename = "hydraulic")]
    Hydraulic,
    #[serde(rename = "pneumatic")]
    Pneumatic,
    #[serde(rename = "thrust")]
    Thrust,
    #[serde(rename = "ice")]
    Ice,
}

impl MotorType {
    /// The exact XSD enumeration literals accepted for `MotorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["bldc", "brushed", "stepper", "servo", "linear", "solenoid", "hydraulic", "pneumatic", "thrust", "ice"]
    }
}

impl core::str::FromStr for MotorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bldc" => Ok(MotorType::Bldc),
            "brushed" => Ok(MotorType::Brushed),
            "stepper" => Ok(MotorType::Stepper),
            "servo" => Ok(MotorType::Servo),
            "linear" => Ok(MotorType::Linear),
            "solenoid" => Ok(MotorType::Solenoid),
            "hydraulic" => Ok(MotorType::Hydraulic),
            "pneumatic" => Ok(MotorType::Pneumatic),
            "thrust" => Ok(MotorType::Thrust),
            "ice" => Ok(MotorType::Ice),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for MotorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            MotorType::Bldc => "bldc",
            MotorType::Brushed => "brushed",
            MotorType::Stepper => "stepper",
            MotorType::Servo => "servo",
            MotorType::Linear => "linear",
            MotorType::Solenoid => "solenoid",
            MotorType::Hydraulic => "hydraulic",
            MotorType::Pneumatic => "pneumatic",
            MotorType::Thrust => "thrust",
            MotorType::Ice => "ice",
        })
    }
}

/// Provenance of a visual/collision name: "authored" = meaningful, human/source-supplied
/// (round-trips, emitted to URDF where the URDF schema allows it); "synthesized" = generated by a
/// converter to satisfy HCDF's required-name rule (stripped on URDF export so the output stays
/// byte-clean). Absence is interpreted as "authored" by the companion validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NameOrigin {
    #[serde(rename = "authored")]
    Authored,
    #[serde(rename = "synthesized")]
    Synthesized,
}

impl NameOrigin {
    /// The exact XSD enumeration literals accepted for `NameOrigin`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["authored", "synthesized"]
    }
}

impl core::str::FromStr for NameOrigin {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "authored" => Ok(NameOrigin::Authored),
            "synthesized" => Ok(NameOrigin::Synthesized),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for NameOrigin {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            NameOrigin::Authored => "authored",
            NameOrigin::Synthesized => "synthesized",
        })
    }
}

/// Noise distribution model for sensor simulation. gaussian: Gaussian (normal) distribution with mean
/// and stddev. uniform: uniform distribution over a range. none: no noise applied (ideal sensor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseType {
    #[serde(rename = "gaussian")]
    Gaussian,
    #[serde(rename = "uniform")]
    Uniform,
    #[serde(rename = "none")]
    None,
}

impl NoiseType {
    /// The exact XSD enumeration literals accepted for `NoiseType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["gaussian", "uniform", "none"]
    }
}

impl core::str::FromStr for NoiseType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gaussian" => Ok(NoiseType::Gaussian),
            "uniform" => Ok(NoiseType::Uniform),
            "none" => Ok(NoiseType::None),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for NoiseType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            NoiseType::Gaussian => "gaussian",
            NoiseType::Uniform => "uniform",
            NoiseType::None => "none",
        })
    }
}

/// Optical sensor sub-type. camera: visible-spectrum image sensor (RGB, mono, stereo, depth).
/// thermal: infrared thermal camera (LWIR/MWIR). lidar: Light Detection and Ranging (spinning or
/// solid-state). tof: Time-of-Flight depth sensor. optical_flow: optical flow sensor for velocity
/// estimation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpticalSensorType {
    #[serde(rename = "camera")]
    Camera,
    #[serde(rename = "thermal")]
    Thermal,
    #[serde(rename = "lidar")]
    Lidar,
    #[serde(rename = "tof")]
    Tof,
    #[serde(rename = "optical_flow")]
    OpticalFlow,
}

impl OpticalSensorType {
    /// The exact XSD enumeration literals accepted for `OpticalSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["camera", "thermal", "lidar", "tof", "optical_flow"]
    }
}

impl core::str::FromStr for OpticalSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "camera" => Ok(OpticalSensorType::Camera),
            "thermal" => Ok(OpticalSensorType::Thermal),
            "lidar" => Ok(OpticalSensorType::Lidar),
            "tof" => Ok(OpticalSensorType::Tof),
            "optical_flow" => Ok(OpticalSensorType::OpticalFlow),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for OpticalSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            OpticalSensorType::Camera => "camera",
            OpticalSensorType::Thermal => "thermal",
            OpticalSensorType::Lidar => "lidar",
            OpticalSensorType::Tof => "tof",
            OpticalSensorType::OpticalFlow => "optical_flow",
        })
    }
}

/// Flow/velocity probe geometry for a fluid sensor (airspeed or flow sub-types). pitot: single
/// total-pressure tube; static pressure taken from a separate source. pitot_static: combined Prandtl
/// tube carrying both total and static ports. multihole: multi-port probe (e.g. 5-hole/7-hole)
/// resolving flow angle as well as speed. hotwire: thermal anemometer (heated element, convective
/// cooling proportional to speed). venturi: venturi/nozzle differential-pressure flow element.
/// orifice: orifice-plate differential-pressure flow element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeType {
    #[serde(rename = "pitot")]
    Pitot,
    #[serde(rename = "pitot_static")]
    PitotStatic,
    #[serde(rename = "multihole")]
    Multihole,
    #[serde(rename = "hotwire")]
    Hotwire,
    #[serde(rename = "venturi")]
    Venturi,
    #[serde(rename = "orifice")]
    Orifice,
}

impl ProbeType {
    /// The exact XSD enumeration literals accepted for `ProbeType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["pitot", "pitot_static", "multihole", "hotwire", "venturi", "orifice"]
    }
}

impl core::str::FromStr for ProbeType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pitot" => Ok(ProbeType::Pitot),
            "pitot_static" => Ok(ProbeType::PitotStatic),
            "multihole" => Ok(ProbeType::Multihole),
            "hotwire" => Ok(ProbeType::Hotwire),
            "venturi" => Ok(ProbeType::Venturi),
            "orifice" => Ok(ProbeType::Orifice),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for ProbeType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ProbeType::Pitot => "pitot",
            ProbeType::PitotStatic => "pitot_static",
            ProbeType::Multihole => "multihole",
            ProbeType::Hotwire => "hotwire",
            ProbeType::Venturi => "venturi",
            ProbeType::Orifice => "orifice",
        })
    }
}

/// Radiation sensor sub-type for nuclear inspection, environmental monitoring, and space robotics.
/// geiger: Geiger-Muller tube for ionizing radiation counting (alpha, beta, gamma). scintillation:
/// scintillation detector for gamma spectroscopy (higher energy resolution than Geiger). neutron:
/// neutron detector for nuclear material detection. dosimeter: cumulative radiation dose measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RadiationSensorType {
    #[serde(rename = "geiger")]
    Geiger,
    #[serde(rename = "scintillation")]
    Scintillation,
    #[serde(rename = "neutron")]
    Neutron,
    #[serde(rename = "dosimeter")]
    Dosimeter,
}

impl RadiationSensorType {
    /// The exact XSD enumeration literals accepted for `RadiationSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["geiger", "scintillation", "neutron", "dosimeter"]
    }
}

impl core::str::FromStr for RadiationSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "geiger" => Ok(RadiationSensorType::Geiger),
            "scintillation" => Ok(RadiationSensorType::Scintillation),
            "neutron" => Ok(RadiationSensorType::Neutron),
            "dosimeter" => Ok(RadiationSensorType::Dosimeter),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for RadiationSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            RadiationSensorType::Geiger => "geiger",
            RadiationSensorType::Scintillation => "scintillation",
            RadiationSensorType::Neutron => "neutron",
            RadiationSensorType::Dosimeter => "dosimeter",
        })
    }
}

/// Radio-frequency sensor sub-type. Uses propagating radio waves for ranging, positioning, or
/// imaging; distinct from em which measures static/quasi-static fields. gnss: satellite navigation
/// receiver. uwb: Ultra-Wideband ranging. radar: radio detection and ranging (24/77/79 GHz).
/// radio_altimeter: radio altitude measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RfSensorType {
    #[serde(rename = "gnss")]
    Gnss,
    #[serde(rename = "uwb")]
    Uwb,
    #[serde(rename = "radar")]
    Radar,
    #[serde(rename = "radio_altimeter")]
    RadioAltimeter,
}

impl RfSensorType {
    /// The exact XSD enumeration literals accepted for `RfSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["gnss", "uwb", "radar", "radio_altimeter"]
    }
}

impl core::str::FromStr for RfSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "gnss" => Ok(RfSensorType::Gnss),
            "uwb" => Ok(RfSensorType::Uwb),
            "radar" => Ok(RfSensorType::Radar),
            "radio_altimeter" => Ok(RfSensorType::RadioAltimeter),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for RfSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            RfSensorType::Gnss => "gnss",
            RfSensorType::Uwb => "uwb",
            RfSensorType::Radar => "radar",
            RfSensorType::RadioAltimeter => "radio_altimeter",
        })
    }
}

/// Role of an endpoint within a MULTI-ENDPOINT transmission coupling, disambiguating which motor
/// drives which output and how outputs are geared together. Omit for a simple
/// single-motor/single-joint reduction, where each endpoint's part is unambiguous. reference: the
/// fixed reference endpoint a driven endpoint is geared against (a gearbox's reference joint/body).
/// driven: the output endpoint driven through the coupling relative to the reference (a gearbox's
/// driven joint). input: an input endpoint of the coupling (a differential's motor inputs). output:
/// an output endpoint of the coupling (a differential's joint outputs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoleType {
    #[serde(rename = "reference")]
    Reference,
    #[serde(rename = "driven")]
    Driven,
    #[serde(rename = "input")]
    Input,
    #[serde(rename = "output")]
    Output,
}

impl RoleType {
    /// The exact XSD enumeration literals accepted for `RoleType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["reference", "driven", "input", "output"]
    }
}

impl core::str::FromStr for RoleType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "reference" => Ok(RoleType::Reference),
            "driven" => Ok(RoleType::Driven),
            "input" => Ok(RoleType::Input),
            "output" => Ok(RoleType::Output),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for RoleType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            RoleType::Reference => "reference",
            RoleType::Driven => "driven",
            RoleType::Input => "input",
            RoleType::Output => "output",
        })
    }
}

/// Tactile sensor sub-type. Tactile sensors measure contact, pressure, or deformation distributed
/// across a surface area, as distinct from force sensors which measure at a single point. capacitive:
/// measures pressure via capacitance change between conductive layers; high sensitivity, good spatial
/// resolution, common in robot fingertips and e-skin (e.g., PPS TakkTile, SynTouch BioTac).
/// resistive: force-sensitive resistor (FSR) arrays or piezoresistive films; lower cost, simpler
/// electronics, used in gripper pads and pressure mats (e.g., Interlink FSR, Tekscan FlexiForce).
/// piezoelectric: generates voltage from mechanical deformation; measures dynamic contact events
/// (slip, vibration, texture) rather than static pressure, used for slip detection in dexterous
/// hands. barometric: array of barometric pressure sensors under a deformable membrane; robust to
/// wear, high dynamic range, used in soft grippers and compliant fingers (e.g., XELA uSkin). optical:
/// uses internal cameras and deformable gel to measure contact geometry via image analysis; provides
/// rich 3D contact shape and shear force data (e.g., GelSight, DIGIT, GelSlim).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TactileSensorType {
    #[serde(rename = "capacitive")]
    Capacitive,
    #[serde(rename = "resistive")]
    Resistive,
    #[serde(rename = "piezoelectric")]
    Piezoelectric,
    #[serde(rename = "barometric")]
    Barometric,
    #[serde(rename = "optical")]
    Optical,
}

impl TactileSensorType {
    /// The exact XSD enumeration literals accepted for `TactileSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["capacitive", "resistive", "piezoelectric", "barometric", "optical"]
    }
}

impl core::str::FromStr for TactileSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "capacitive" => Ok(TactileSensorType::Capacitive),
            "resistive" => Ok(TactileSensorType::Resistive),
            "piezoelectric" => Ok(TactileSensorType::Piezoelectric),
            "barometric" => Ok(TactileSensorType::Barometric),
            "optical" => Ok(TactileSensorType::Optical),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for TactileSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            TactileSensorType::Capacitive => "capacitive",
            TactileSensorType::Resistive => "resistive",
            TactileSensorType::Piezoelectric => "piezoelectric",
            TactileSensorType::Barometric => "barometric",
            TactileSensorType::Optical => "optical",
        })
    }
}

/// Temperature sensor type. thermistor: NTC or PTC resistive element (common in motors for winding
/// temp). rtd: Resistance Temperature Detector (PT100/PT1000, high accuracy). thermocouple: K/J/T/E
/// type junction (wide range, industrial). ir: non-contact infrared temperature sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemperatureSensorType {
    #[serde(rename = "thermistor")]
    Thermistor,
    #[serde(rename = "rtd")]
    Rtd,
    #[serde(rename = "thermocouple")]
    Thermocouple,
    #[serde(rename = "ir")]
    Ir,
}

impl TemperatureSensorType {
    /// The exact XSD enumeration literals accepted for `TemperatureSensorType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["thermistor", "rtd", "thermocouple", "ir"]
    }
}

impl core::str::FromStr for TemperatureSensorType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "thermistor" => Ok(TemperatureSensorType::Thermistor),
            "rtd" => Ok(TemperatureSensorType::Rtd),
            "thermocouple" => Ok(TemperatureSensorType::Thermocouple),
            "ir" => Ok(TemperatureSensorType::Ir),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for TemperatureSensorType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            TemperatureSensorType::Thermistor => "thermistor",
            TemperatureSensorType::Rtd => "rtd",
            TemperatureSensorType::Thermocouple => "thermocouple",
            TemperatureSensorType::Ir => "ir",
        })
    }
}

/// Mechanical power transmission mechanism between motor and joint. simple: direct drive or
/// single-stage reduction. differential: differential gear mechanism (couples two motors to two
/// outputs). belt: belt/pulley transmission. gear: spur or helical gear train. planetary: planetary
/// (epicyclic) gear set (compact, high ratio). cycloidal: cycloidal drive (high ratio, zero backlash,
/// high shock load capacity, used in robot joints). harmonic: harmonic drive / strain-wave gearing
/// (very high ratio in compact form, used in robot arms; Harmonic Drive AG). strain-wave: same as
/// harmonic (alternate name). worm: worm gear (high ratio, self-locking, lower efficiency, used where
/// back-driving prevention needed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransmissionType {
    #[serde(rename = "simple")]
    Simple,
    #[serde(rename = "differential")]
    Differential,
    #[serde(rename = "belt")]
    Belt,
    #[serde(rename = "gear")]
    Gear,
    #[serde(rename = "planetary")]
    Planetary,
    #[serde(rename = "cycloidal")]
    Cycloidal,
    #[serde(rename = "harmonic")]
    Harmonic,
    #[serde(rename = "strain-wave")]
    StrainWave,
    #[serde(rename = "worm")]
    Worm,
}

impl TransmissionType {
    /// The exact XSD enumeration literals accepted for `TransmissionType`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["simple", "differential", "belt", "gear", "planetary", "cycloidal", "harmonic", "strain-wave", "worm"]
    }
}

impl core::str::FromStr for TransmissionType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "simple" => Ok(TransmissionType::Simple),
            "differential" => Ok(TransmissionType::Differential),
            "belt" => Ok(TransmissionType::Belt),
            "gear" => Ok(TransmissionType::Gear),
            "planetary" => Ok(TransmissionType::Planetary),
            "cycloidal" => Ok(TransmissionType::Cycloidal),
            "harmonic" => Ok(TransmissionType::Harmonic),
            "strain-wave" => Ok(TransmissionType::StrainWave),
            "worm" => Ok(TransmissionType::Worm),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for TransmissionType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            TransmissionType::Simple => "simple",
            TransmissionType::Differential => "differential",
            TransmissionType::Belt => "belt",
            TransmissionType::Gear => "gear",
            TransmissionType::Planetary => "planetary",
            TransmissionType::Cycloidal => "cycloidal",
            TransmissionType::Harmonic => "harmonic",
            TransmissionType::StrainWave => "strain-wave",
            TransmissionType::Worm => "worm",
        })
    }
}

/// World-frame (navigation frame) axis convention describing how global/map coordinate axes are
/// oriented. ENU (X-east, Y-north, Z-up): common in geographic information systems, ground robotics,
/// and surveying. Z-up aligns with gravity opposition, making vertical reasoning intuitive. NED
/// (X-north, Y-east, Z-down): common in aerospace, aviation, marine navigation, and most inertial
/// navigation systems. Z-down aligns with gravity direction, matching accelerometer sign conventions
/// in flight controllers. The world-frame convention determines how global poses, waypoints, and map
/// coordinates are interpreted. Independent of body-frame: a ground robot may use body-frame=FLU with
/// world-frame=ENU, while a drone may use body-frame=FRD with world-frame=NED. Semantic rule for
/// composition: all files in an include chain must use the same world-frame convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorldFrame {
    #[serde(rename = "ENU")]
    ENU,
    #[serde(rename = "NED")]
    NED,
}

impl WorldFrame {
    /// The exact XSD enumeration literals accepted for `WorldFrame`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["ENU", "NED"]
    }
}

impl core::str::FromStr for WorldFrame {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ENU" => Ok(WorldFrame::ENU),
            "NED" => Ok(WorldFrame::NED),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for WorldFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            WorldFrame::ENU => "ENU",
            WorldFrame::NED => "NED",
        })
    }
}

/// Chirality of a screw thread. right: a right-handed helix (positive @thread_pitch advances along
/// +axis under a right-hand-rule positive rotation), the canonical HCDF storage and the modern gz
/// convention. left: a left-handed helix, the legacy gazebo-classic thread_pitch positive sense.
/// Absent => right (canonical). Only valid on type="screw". Legacy left-handed sources are converted
/// to the canonical right-handed sign on import (a sign flip accompanies the rad_per_m -> m_per_rev
/// unit conversion).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Handedness {
    #[serde(rename = "right")]
    Right,
    #[serde(rename = "left")]
    Left,
}

impl Handedness {
    /// The exact XSD enumeration literals accepted for `handedness`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["right", "left"]
    }
}

impl core::str::FromStr for Handedness {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "right" => Ok(Handedness::Right),
            "left" => Ok(Handedness::Left),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for Handedness {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Handedness::Right => "right",
            Handedness::Left => "left",
        })
    }
}

/// Unit convention the screw @thread_pitch value is expressed in. m_per_rev: meters of axial advance
/// per full (2*pi rad) revolution; the canonical HCDF storage and the modern gz screw_thread_pitch
/// convention. rad_per_m: radians of rotation per meter of axial advance; the legacy gazebo-classic
/// thread_pitch convention. Absent => m_per_rev (canonical). Only valid on type="screw". The SDF
/// importer CONVERTS a legacy rad_per_m source to canonical m_per_rev and stores the converted value
/// with this attribute omitted; the attribute exists so a non-canonical pitch can also be
/// authored/preserved explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PitchConvention {
    #[serde(rename = "m_per_rev")]
    MPerRev,
    #[serde(rename = "rad_per_m")]
    RadPerM,
}

impl PitchConvention {
    /// The exact XSD enumeration literals accepted for `pitch_convention`, in schema order.
    pub const fn valid_values() -> &'static [&'static str] {
        &["m_per_rev", "rad_per_m"]
    }
}

impl core::str::FromStr for PitchConvention {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "m_per_rev" => Ok(PitchConvention::MPerRev),
            "rad_per_m" => Ok(PitchConvention::RadPerM),
            _ => Err(()),
        }
    }
}

impl core::fmt::Display for PitchConvention {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            PitchConvention::MPerRev => "m_per_rev",
            PitchConvention::RadPerM => "rad_per_m",
        })
    }
}

/// The enumerated `(element-local-name, key)` slots the TYPED model cannot enforce at parse,
/// paired with the exact literal set the schema accepts. This is the RESIDUAL: every
/// enumerated XSD slot whose enum has NO typed model field (so serde cannot reject it at
/// parse), computed from the model sources rather than hand-curated: a slot is kept ONLY
/// when no struct field retyped it, so any future enumerated slot lacking a typed field is
/// retained automatically. Three families remain: (1) sensor-category `@type` slots that
/// share [`super::sensor::SensorCategory`]; (2) camera projection and fluid probe `@type`;
/// and (3) transmission endpoint `@role`. These fields intentionally stay string-typed, and
/// [`crate::validate::validate_enums`] enforces their exact vocabularies by document walk.
pub static ENUM_ATTRS: &[(&str, &str, &[&str])] = &[
    ("audio", "@type", AudioSensorType::valid_values()),
    ("chemical", "@type", ChemicalSensorType::valid_values()),
    ("em", "@type", EmSensorType::valid_values()),
    ("encoder", "@type", EncoderSensorType::valid_values()),
    ("fluid", "@type", FluidSensorType::valid_values()),
    ("force", "@type", ForceSensorType::valid_values()),
    ("joint", "@role", RoleType::valid_values()),
    ("lens", "@type", CameraProjectionType::valid_values()),
    ("motor", "@role", RoleType::valid_values()),
    ("probe", "@type", ProbeType::valid_values()),
    ("radiation", "@type", RadiationSensorType::valid_values()),
    ("rf", "@type", RfSensorType::valid_values()),
    ("tactile", "@type", TactileSensorType::valid_values()),
    ("temperature", "@type", TemperatureSensorType::valid_values()),
];

/// The accepted literal set for `(element, key)`, or `None` if it is not a model-untyped slot.
pub fn valid_values_for(element: &str, key: &str) -> Option<&'static [&'static str]> {
    ENUM_ATTRS.iter().find(|(e, k, _)| *e == element && *k == key).map(|(_, _, v)| *v)
}
