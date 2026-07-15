//! `<sensor>` and the 12 sensor categories. A `<sensor>` carries `@name`/`@update-rate` and a list
//! of category elements (inertial/em/optical/rf/chemical/force/encoder/temperature/radiation/audio/
//! tactile/fluid). Each category shares a common pose/driver/range/resolution/noise/data-output/
//! geometry base; the category-specific fields are typed so a populated sub-tree round-trips.
use super::common::{MeasuredValue, RangeValue};
use super::enums::{
    AxisValue, EncoderPrinciple, ForceFrame, InertialSensorType, MeasureDirection, NoiseType,
    OpticalSensorType,
};
use super::geometry::Geometry;
use super::sensor_params::{
    CameraCalibration, CameraDistortion, CameraIntrinsics, CameraLens, CameraMatrix, CameraParams,
    DataOutput, FifoConfig, GnssParams, LidarParams, RadarParams,
};
use super::Pose;
use serde::{Deserialize, Serialize};

/// `<sensor name= update-rate=>` with one or more category children.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "sensor")]
pub struct Sensor {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        rename = "@update-rate",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub update_rate: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inertial: Vec<InertialSensor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub em: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optical: Vec<OpticalSensor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rf: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chemical: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub force: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub encoder: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub temperature: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub radiation: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tactile: Vec<SensorCategory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fluid: Vec<FluidSensor>,
}

/// Generic sensor category covering the shared base of em/rf/chemical/force/encoder/temperature/
/// radiation/audio/tactile. Category-specific attributes (`@type`, `@principle`, ...) and rare child
/// detail are leniently ignored; the common modeling surface is typed here. The rf-specific parameter
/// blocks (`data-output`/`radar-params`/`gnss-params`) are typed too; they only appear on `<rf>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SensorCategory {
    /// `@type` stays `Option<String>`, NOT retyped to an enum. This one struct deserializes NINE
    /// different category elements (`<em>`/`<rf>`/`<chemical>`/`<force>`/`<encoder>`/`<temperature>`/
    /// `<radiation>`/`<audio>`/`<tactile>`), each with a DIFFERENT `@type` enum, so no single Rust enum
    /// fits. These slots are the one remaining `crate::model::enums::ENUM_ATTRS` entries and are enforced
    /// by `crate::validate::validate_enums` at validate time rather than at parse. (`<inertial>` and
    /// `<optical>` have their own structs, so their `@type` IS a typed enum.)
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    /// `@principle` only ever appears on `<encoder>`, so it maps cleanly to one enum.
    #[serde(
        rename = "@principle",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub principle: Option<EncoderPrinciple>,
    #[serde(
        rename = "@interface",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub interface: Option<String>,
    /// `@active` only ever appears on `<audio>` (active transducer vs passive receiver).
    #[serde(rename = "@active", default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// `@frame` only ever appears on `<force>` (F/T reporting frame: parent/child/sensor).
    #[serde(rename = "@frame", default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ForceFrame>,
    /// `@measure-direction` only ever appears on `<force>` (wrench sign convention).
    #[serde(
        rename = "@measure-direction",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub measure_direction: Option<MeasureDirection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<SensorDriver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<Noise>,
    #[serde(
        rename = "data-output",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_output: Option<DataOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
    #[serde(
        rename = "radar-params",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub radar_params: Option<RadarParams>,
    #[serde(
        rename = "gnss-params",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub gnss_params: Option<GnssParams>,

    // `<audio>` extras (present only on the audio category; mirror the XSD `audio_sensor` sequence).
    #[serde(
        rename = "frequency-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub frequency_range: Option<RangeValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<String>,
    #[serde(
        rename = "beam-width",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub beam_width: Option<MeasuredValue>,

    // `<tactile>` extras (present only on the tactile category; mirror the XSD `tactile_sensor`
    // sequence). Only one category's extras are ever populated at a time, so this shared struct
    // serializes each category in its own valid schema order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taxels: Option<String>,
    #[serde(
        rename = "spatial-resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub spatial_resolution: Option<MeasuredValue>,
    #[serde(
        rename = "pressure-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub pressure_range: Option<RangeValue>,
    #[serde(
        rename = "sensing-area",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sensing_area: Option<MeasuredValue>,

    // `<force>` extras (present only on the force category; appended per the force_sensor extension
    // sequence, so a contact/F-T sensor serializes in valid schema order after the sensor_base body).
    /// `<collision>`: name of the collision this contact/force sensor monitors (SDF contact/collision).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collision: Option<String>,
    /// `<axis-noise>`: per-axis force/torque noise for a multi-axis F/T sensor (SDF force_torque).
    #[serde(
        rename = "axis-noise",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub axis_noise: Option<ForceAxisNoise>,
}

/// `<inertial type=>` sensor category: pose/driver + `<accel>`/`<gyro>` parameter blocks.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InertialSensor {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<InertialSensorType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<SensorDriver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accel: Option<SensorParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gyro: Option<SensorParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
}

/// `<optical type=>` sensor category: pose/driver + a list of `<fov>` field-of-view definitions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OpticalSensor {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<OpticalSensorType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<SensorDriver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fov: Vec<SensorFov>,
    #[serde(
        rename = "data-output",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_output: Option<DataOutput>,
    #[serde(
        rename = "camera-params",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub camera_params: Option<CameraParams>,
    #[serde(
        rename = "lidar-params",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub lidar_params: Option<LidarParams>,
}

/// `<fluid type=>` sensor category (the twelfth): fluid pressure/flow, from barometers, airspeed/pitot,
/// depth, flow, level, and general pressure transducers. Extends the shared sensor base (pose/driver/
/// range/resolution/noise/data-output/geometry) with a pressure core, reference values, per-subtype
/// derived outputs (altitude/airspeed/depth/level/flow), and a flow probe. `@type` stays
/// `Option<String>`: a model-untyped slot enforced by [`crate::validate::validate_enums`] via the
/// `fluid` entry in [`crate::model::enums::ENUM_ATTRS`], mirroring the `em`/`force`/etc. categories.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FluidSensor {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    // sensor_base
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<SensorDriver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<Noise>,
    #[serde(
        rename = "data-output",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_output: Option<DataOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
    // fluid-specific pressure core + references + derived outputs + probe
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<String>,
    #[serde(
        rename = "pressure-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub pressure_range: Option<RangeValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<MeasuredValue>,
    #[serde(
        rename = "temperature-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub temperature_range: Option<RangeValue>,
    #[serde(
        rename = "temperature-coefficient",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub temperature_coefficient: Option<MeasuredValue>,
    #[serde(
        rename = "reference-pressure",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reference_pressure: Option<MeasuredValue>,
    #[serde(
        rename = "reference-density",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reference_density: Option<MeasuredValue>,
    #[serde(
        rename = "altitude-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub altitude_range: Option<RangeValue>,
    #[serde(
        rename = "altitude-resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub altitude_resolution: Option<MeasuredValue>,
    #[serde(
        rename = "airspeed-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub airspeed_range: Option<RangeValue>,
    #[serde(
        rename = "airspeed-resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub airspeed_resolution: Option<MeasuredValue>,
    #[serde(
        rename = "airspeed-output",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub airspeed_output: Vec<String>,
    #[serde(
        rename = "depth-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub depth_range: Option<RangeValue>,
    #[serde(
        rename = "level-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub level_range: Option<RangeValue>,
    #[serde(
        rename = "flow-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub flow_range: Option<RangeValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<FluidProbe>,
}

/// `<probe type= static-port= total-port= ports=>`: flow/velocity probe geometry and ports for the
/// airspeed/flow fluid sub-types. `@type` stays `Option<String>`: a model-untyped slot enforced by
/// [`crate::validate::validate_enums`] via the `probe` entry in [`crate::model::enums::ENUM_ATTRS`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FluidProbe {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(
        rename = "@static-port",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub static_port: Option<String>,
    #[serde(
        rename = "@total-port",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub total_port: Option<String>,
    #[serde(rename = "@ports", default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<String>,
}

/// `<fov name= color=>`: pose + geometry (incl. frustum) + the design-spec camera intrinsics,
/// distortion, lens projection model, measured calibration, and per-FoV noise, all typed so a
/// populated FoV round-trips without loss.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SensorFov {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@color", default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intrinsics: Option<CameraIntrinsics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distortion: Option<CameraDistortion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<CameraLens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<CameraCalibration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<Noise>,
    /// Full ROS CameraInfo geometry (rectification R, projection P, binning, ROI) beyond K/D.
    #[serde(
        rename = "camera-matrix",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub camera_matrix: Option<CameraMatrix>,
}

/// `<driver name=>` with optional `<axis-align>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SensorDriver {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        rename = "axis-align",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub axis_align: Option<AxisAlign>,
}

/// `<axis-align x= y= z=>` (e.g. `x="Y" y="-X" z="Z"`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AxisAlign {
    #[serde(rename = "@x", default, skip_serializing_if = "Option::is_none")]
    pub x: Option<AxisValue>,
    #[serde(rename = "@y", default, skip_serializing_if = "Option::is_none")]
    pub y: Option<AxisValue>,
    #[serde(rename = "@z", default, skip_serializing_if = "Option::is_none")]
    pub z: Option<AxisValue>,
}

/// `<accel>`/`<gyro>` parameter block: range/resolution/odr/bandwidth/fifo/noise.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SensorParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub odr: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fifo: Option<FifoConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<Noise>,
    /// Optional per-axis noise (x/y/z), each a full `<noise>`; the scalar `noise` above is the
    /// isotropic fallback. SDF gz-sim per-axis accel/gyro `<noise>` blocks.
    #[serde(
        rename = "axis-noise",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub axis_noise: Option<AxisNoise>,
}

/// `<noise type=>`: gaussian/uniform/none with mean/stddev/bias text children.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Noise {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<NoiseType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stddev: Option<String>,
    #[serde(rename = "bias-mean", default, skip_serializing_if = "Option::is_none")]
    pub bias_mean: Option<String>,
    #[serde(
        rename = "bias-stddev",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bias_stddev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<String>,
    /// Dynamic (time-varying Gauss-Markov) bias stddev: SDF `noise/dynamic_bias_stddev`.
    #[serde(
        rename = "dynamic-bias-stddev",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dynamic_bias_stddev: Option<String>,
    /// Correlation time (s) of the dynamic bias process: SDF `noise/dynamic_bias_correlation_time`.
    #[serde(
        rename = "dynamic-bias-correlation-time",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dynamic_bias_correlation_time: Option<String>,
}

/// `<axis-noise>`: an independent [`Noise`] model on each of the x/y/z channels of a 3-axis
/// measurement (per-axis IMU accel/gyro noise, or one force/torque channel triplet). Complements the
/// scalar `<noise>` fallback.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AxisNoise {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Noise>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Noise>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<Noise>,
}

/// `<axis-noise>` on a multi-axis force/torque sensor: per-axis [`AxisNoise`] on the force channels
/// and the torque channels (SDF force_torque per-axis force/torque `<noise>`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ForceAxisNoise {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<AxisNoise>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torque: Option<AxisNoise>,
}
