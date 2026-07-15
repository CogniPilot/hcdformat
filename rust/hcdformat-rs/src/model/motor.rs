//! `<motor>`: an actuator on a component. Electrical/mechanical/thermal specs are typed, including the
//! thermal (thermal-resistance/max-temperature), BLDC (pole-pairs), and ICE/linear (displacement/
//! cylinders/stroke) detail leaves so a populated `<motor>` round-trips without loss.
use super::common::{MeasuredValue, RatedValue};
use super::enums::MotorType;
use super::Pose;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "motor")]
pub struct Motor {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<MotorType>,
    #[serde(
        rename = "@construction",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub construction: Option<String>,
    #[serde(rename = "@visual", default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<String>,
    #[serde(rename = "@mesh", default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    #[serde(rename = "@encoder", default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    #[serde(rename = "@hall", default, skip_serializing_if = "Option::is_none")]
    pub hall: Option<String>,
    #[serde(
        rename = "@thermistor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub thermistor: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<RatedValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<RatedValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resistance: Option<MeasuredValue>,
    /// `<inductance>` (phase/winding, henries): a standard BLDC datasheet param alongside resistance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inductance: Option<MeasuredValue>,
    #[serde(
        rename = "torque-constant",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub torque_constant: Option<MeasuredValue>,
    #[serde(
        rename = "velocity-constant",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub velocity_constant: Option<MeasuredValue>,
    /// `<rotor-inertia>` (kg·m²): rotor J for reflected-inertia math on QDD actuators.
    #[serde(
        rename = "rotor-inertia",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub rotor_inertia: Option<MeasuredValue>,
    #[serde(rename = "max-speed", default, skip_serializing_if = "Option::is_none")]
    pub max_speed: Option<MeasuredValue>,
    #[serde(
        rename = "stall-torque",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub stall_torque: Option<MeasuredValue>,
    #[serde(
        rename = "thermal-resistance",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub thermal_resistance: Option<MeasuredValue>,
    #[serde(
        rename = "max-temperature",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_temperature: Option<MeasuredValue>,
    #[serde(
        rename = "no-load-speed",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub no_load_speed: Option<MeasuredValue>,
    #[serde(
        rename = "no-load-current",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub no_load_current: Option<MeasuredValue>,
    #[serde(
        rename = "max-power-out",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_power_out: Option<MeasuredValue>,
    #[serde(
        rename = "max-efficiency",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_efficiency: Option<MeasuredValue>,
    #[serde(
        rename = "thrust-axis",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub thrust_axis: Option<String>,
    #[serde(
        rename = "max-thrust",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_thrust: Option<MeasuredValue>,
    /// `<pole-pairs>` (`xs:unsignedInt`) kept as text so the authored value round-trips verbatim.
    #[serde(
        rename = "pole-pairs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub pole_pairs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub displacement: Option<MeasuredValue>,
    /// `<cylinders>` (`xs:unsignedInt`) kept as text so the authored value round-trips verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cylinders: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<MeasuredValue>,
    #[serde(
        rename = "control-modes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub control_modes: Option<ControlModes>,
}

/// `<control-modes>` holding one or more `<mode>` text entries.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ControlModes {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mode: Vec<String>,
}
