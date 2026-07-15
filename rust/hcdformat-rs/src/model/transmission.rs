//! `<transmission>`: couples a motor to a joint with a reduction/efficiency and optional springs.
use super::common::{MeasuredValue, RangeValue};
use super::enums::TransmissionType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "transmission")]
pub struct Transmission {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<TransmissionType>,
    #[serde(rename = "@encoder", default, skip_serializing_if = "Option::is_none")]
    pub encoder: Option<String>,
    #[serde(rename = "@visual", default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<String>,
    #[serde(rename = "@mesh", default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub motor: Vec<Endpoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joint: Vec<Endpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backlash: Option<MeasuredValue>,
    /// `<spring>` (maxOccurs unbounded): compliant element(s): zero = rigid, one series = SEA, one
    /// parallel = PEA, one of each = SEA+PEA (hcdf.xsd:2928).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spring: Vec<TransmissionSpring>,
}

/// `<spring>` (`transmission_spring`, hcdf.xsd:2865): compliant drivetrain element (SEA/PEA/CPEA/
/// AE-PEA/VSA). Seven physical-quantity leaves plus six attributes selecting the compliance
/// architecture. Attribute `@placement` is required by the XSD; the rest are optional.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TransmissionSpring {
    #[serde(
        rename = "@placement",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub placement: Option<String>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(
        rename = "@torque-sensing",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub torque_sensing: Option<String>,
    #[serde(rename = "@clutch", default, skip_serializing_if = "Option::is_none")]
    pub clutch: Option<String>,
    #[serde(
        rename = "@equilibrium-actuator",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub equilibrium_actuator: Option<String>,
    #[serde(
        rename = "@stiffness-actuator",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub stiffness_actuator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stiffness: Option<RangeValue>,
    #[serde(
        rename = "max-deflection",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_deflection: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equilibrium: Option<MeasuredValue>,
    #[serde(
        rename = "equilibrium-range",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub equilibrium_range: Option<RangeValue>,
    #[serde(
        rename = "clutch-power",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub clutch_power: Option<MeasuredValue>,
    #[serde(
        rename = "engage-time",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub engage_time: Option<MeasuredValue>,
}

/// `<motor ref= role=>` / `<joint ref= role=>` transmission endpoint. In a multi-endpoint coupling
/// (a gearbox's reference/driven joints, a differential's input motors + output joints) the optional
/// `@role` disambiguates this endpoint's part; a simple single-motor/single-joint reduction omits it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Endpoint {
    #[serde(rename = "@ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
    /// `@role` (`RoleType`: reference/driven/input/output). Kept `String`-typed and presence-preserving;
    /// enforced by [`crate::validate::validate_enums`] via `ENUM_ATTRS`, like the other model-untyped slots.
    #[serde(rename = "@role", default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}
