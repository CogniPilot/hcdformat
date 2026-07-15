//! Type-specific sensor parameter blocks hung off the optical/rf sensor categories and FoVs:
//! `<camera-params>`, `<lidar-params>`, `<radar-params>`, `<gnss-params>`, `<data-output>`,
//! camera `<intrinsics>`, and the IMU `<fifo>` config. Each mirrors the generated Python model
//! (`hcdfdom/model.py`) field-for-field, with numeric *text* content kept as `String` (leaf-as-String
//! convention) and structured value children as [`MeasuredValue`]/[`RangeValue`]. Camera
//! distortion/calibration/lens and the lidar `<range>`/`<scan-pattern>` are typed here too so a
//! populated sensor sub-tree survives parse -> serialize without loss.
use super::common::{MeasuredValue, RangeValue};
use super::enums::CameraDistortionModel;
use super::sensor::Noise;
use serde::{Deserialize, Serialize};

/// `<data-output>`: sensor data-rate characteristics for network/TSN bandwidth planning.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DataOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<MeasuredValue>,
    #[serde(
        rename = "bandwidth-compressed",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bandwidth_compressed: Option<MeasuredValue>,
    #[serde(
        rename = "frame-size",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub frame_size: Option<MeasuredValue>,
}

/// `<intrinsics>`: pinhole camera intrinsics (image dims, pixel format, focal length/principal point,
/// skew). `s` completes the 3x3 K matrix `[[fx, s, cx], [0, fy, cy], [0, 0, 1]]`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraIntrinsics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cy: Option<String>,
    /// Skew coefficient (axis skew) of the pinhole intrinsic matrix. Zero for orthogonal pixel axes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s: Option<String>,
}

/// `<distortion>`: Brown-Conrady lens distortion coefficients (k1-k3 radial, p1-p2 tangential). Holds
/// design-spec nominal values; measured per-unit values live in [`CameraCalibration`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraDistortion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2: Option<String>,
    /// Fourth coefficient: rational_polynomial D[5] / equidistant fisheye D[3].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k4: Option<String>,
    /// Fifth coefficient: rational_polynomial D[6].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k5: Option<String>,
    /// Sixth coefficient: rational_polynomial D[7].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k6: Option<String>,
    /// `@model`: ROS CameraInfo distortion_model (plumb_bob / rational_polynomial / equidistant).
    #[serde(rename = "@model", default, skip_serializing_if = "Option::is_none")]
    pub model: Option<CameraDistortionModel>,
}

/// `<camera-matrix>`: full ROS CameraInfo geometry beyond K (intrinsics) and D (distortion): the
/// rectification matrix R, projection matrix P, pixel binning, and region of interest. Present for a
/// stereo-rectified or cropped/binned camera.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraMatrix {
    /// `<rectification>`: R (3x3, row-major) as nine space-separated doubles. ROS CameraInfo/R.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rectification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<CameraProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binning: Option<CameraBinning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roi: Option<CameraRoi>,
}

/// `<projection>`: structured ROS CameraInfo P (3x4): rectified intrinsics (fx'/fy'/cx'/cy') plus the
/// stereo baseline translation (tx/ty).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
}

/// `<binning x= y=>`: ROS CameraInfo binning_x/binning_y.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraBinning {
    #[serde(rename = "@x", default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(rename = "@y", default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
}

/// `<roi x-offset= y-offset= width= height= rectify=>`: ROS CameraInfo region of interest.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraRoi {
    #[serde(rename = "@x-offset", default, skip_serializing_if = "Option::is_none")]
    pub x_offset: Option<String>,
    #[serde(rename = "@y-offset", default, skip_serializing_if = "Option::is_none")]
    pub y_offset: Option<String>,
    #[serde(rename = "@width", default, skip_serializing_if = "Option::is_none")]
    pub width: Option<String>,
    #[serde(rename = "@height", default, skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
    #[serde(rename = "@rectify", default, skip_serializing_if = "Option::is_none")]
    pub rectify: Option<String>,
}

/// `<lens type=>`: projection model for a non-rectilinear (fisheye/wide-angle) lens that Brown-Conrady
/// distortion cannot describe. `@type` stays `Option<String>`: it is a model-untyped slot enforced by
/// [`crate::validate::validate_enums`] via the `lens` entry in [`crate::model::enums::ENUM_ATTRS`],
/// exactly like the sensor-category `@type`s.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraLens {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(
        rename = "cutoff-angle",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cutoff_angle: Option<MeasuredValue>,
    #[serde(
        rename = "scale-to-hfov",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub scale_to_hfov: Option<String>,
    #[serde(
        rename = "custom-function",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub custom_function: Option<CameraLensCustomFunction>,
}

/// `<custom-function>`: coefficients of a custom radial mapping (SDF `lens/custom_function`), used when
/// `lens/@type="custom"`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraLensCustomFunction {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub f: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fun: Option<String>,
}

/// `<calibration>`: measured camera calibration results for a specific unit (calibrated intrinsics,
/// distortion, and calibration metadata) that override the design-spec values at runtime.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraCalibration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cx: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2: Option<String>,
    #[serde(
        rename = "reprojection-error",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reprojection_error: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<String>,
}

/// `<camera-params>`: camera-specific perception/bandwidth/environment parameters (camera/thermal).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shutter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr: Option<String>,
    #[serde(
        rename = "dynamic-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dynamic_range: Option<MeasuredValue>,
    #[serde(
        rename = "depth-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub depth_range: Option<RangeValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
    #[serde(rename = "ir-cut", default, skip_serializing_if = "Option::is_none")]
    pub ir_cut: Option<String>,
    #[serde(
        rename = "spectral-band",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub spectral_band: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub netd: Option<MeasuredValue>,
    #[serde(
        rename = "temperature-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub temperature_range: Option<RangeValue>,
}

/// `<lidar-params>`: lidar scan geometry, point-cloud density, and environmental robustness. `<range>`
/// (along-beam distance) and the per-axis `<scan-pattern>` are typed so a converted ray/lidar sensor
/// round-trips losslessly.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LidarParams {
    #[serde(rename = "scan-type", default, skip_serializing_if = "Option::is_none")]
    pub scan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<String>,
    #[serde(
        rename = "points-per-second",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub points_per_second: Option<String>,
    #[serde(rename = "scan-rate", default, skip_serializing_if = "Option::is_none")]
    pub scan_rate: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wavelength: Option<MeasuredValue>,
    #[serde(
        rename = "horizontal-fov",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub horizontal_fov: Option<MeasuredValue>,
    #[serde(
        rename = "vertical-fov",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub vertical_fov: Option<RangeValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LidarRange>,
    #[serde(
        rename = "scan-pattern",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub scan_pattern: Option<LidarScanPattern>,
    /// Range/beam noise model (SDF lidar/ray `<noise>`). Absent =&gt; ideal ranging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise: Option<Noise>,
}

/// `<range>` under `<lidar-params>`: along-beam distance measurement range (min/max) and radial
/// resolution. Maps losslessly to SDF `lidar/<range>`; distinct from the angular `<scan-pattern>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LidarRange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<MeasuredValue>,
}

/// `<scan-pattern>`: per-axis angular scan (`<horizontal>`/`<vertical>`), the lossless target for a
/// converted URDF ray/LaserScan sensor.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LidarScanPattern {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizontal: Option<LidarScanAxis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical: Option<LidarScanAxis>,
}

/// `<horizontal>`/`<vertical>`: one axis of a lidar/ray angular scan pattern: samples, angular
/// resolution multiplier, and min/max sweep angle (radians), matching URDF ray semantics. All are
/// attributes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LidarScanAxis {
    #[serde(rename = "@samples", default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<String>,
    #[serde(
        rename = "@resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub resolution: Option<String>,
    #[serde(
        rename = "@min-angle",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub min_angle: Option<String>,
    #[serde(
        rename = "@max-angle",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_angle: Option<String>,
}

/// `<radar-params>`: 4D imaging-radar / distance-velocity parameters (rf sensors, `type="radar"`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RadarParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulation: Option<String>,
    #[serde(
        rename = "azimuth-fov",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub azimuth_fov: Option<MeasuredValue>,
    #[serde(
        rename = "elevation-fov",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub elevation_fov: Option<MeasuredValue>,
    #[serde(
        rename = "azimuth-resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub azimuth_resolution: Option<MeasuredValue>,
    #[serde(
        rename = "elevation-resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub elevation_resolution: Option<MeasuredValue>,
    #[serde(
        rename = "range-resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub range_resolution: Option<MeasuredValue>,
    #[serde(
        rename = "velocity-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub velocity_range: Option<RangeValue>,
    #[serde(
        rename = "velocity-resolution",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub velocity_resolution: Option<MeasuredValue>,
    #[serde(
        rename = "max-detections",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_detections: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mimo: Option<String>,
}

/// `<gnss-params>`: GNSS constellation support, correction capabilities, and positioning accuracy.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GnssParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constellation: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frequency: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtk: Option<String>,
    #[serde(
        rename = "accuracy-standalone",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub accuracy_standalone: Option<MeasuredValue>,
    #[serde(
        rename = "accuracy-rtk",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub accuracy_rtk: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pps: Option<String>,
    #[serde(
        rename = "dead-reckoning",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dead_reckoning: Option<String>,
    #[serde(
        rename = "anti-spoofing",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub anti_spoofing: Option<String>,
}

/// `<fifo depth= watermark=>`: hardware FIFO buffer config for IMU accel/gyro channels.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FifoConfig {
    #[serde(rename = "@depth", default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<String>,
    #[serde(
        rename = "@watermark",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub watermark: Option<String>,
}
