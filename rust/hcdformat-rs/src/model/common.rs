//! Small shared value types used across the model: measured/range/rated values, friction, contact,
//! color, and the string-typed numeric helpers. Numeric *text* content (e.g. `<mass>0.5</mass>`,
//! `<radius>0.05</radius>`) is kept as `String` so exact authored text round-trips without float
//! reformatting; structured numeric *attributes* (pose xyz/rpy/quat) are typed in [`crate::Pose`].
use serde::{Deserialize, Serialize};

/// `<color name= rgba= description=>`: a named palette entry (root) or inline visual color (ARM B).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "color")]
pub struct Color {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@rgba", default, skip_serializing_if = "Option::is_none")]
    pub rgba: Option<String>,
    #[serde(
        rename = "@description",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
}

/// `measured_value`: text magnitude + `@unit` (e.g. `<gain unit="dBi">3.5</gain>`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MeasuredValue {
    #[serde(rename = "@unit", default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(rename = "$text", default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// `range_value`: text + `@unit @min @max` (e.g. `<operating-temp unit="C" min="-40" max="85"/>`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RangeValue {
    #[serde(rename = "@unit", default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(rename = "@min", default, skip_serializing_if = "Option::is_none")]
    pub min: Option<String>,
    #[serde(rename = "@max", default, skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
    #[serde(rename = "$text", default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// `current_capability`/`power_capability`: mixed text + `@unit @max` (no `@min`): a port's rated
/// current/power capability (hcdf.xsd `current_capability`/`power_capability`). Distinct from
/// [`RangeValue`] (which also carries `@min`) so a bare `<current unit="A">2</current>` round-trips
/// without inventing an absent `@min`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MaxValue {
    #[serde(rename = "@unit", default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(rename = "@min", default, skip_serializing_if = "Option::is_none")]
    pub min: Option<String>,
    #[serde(rename = "@max", default, skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
    #[serde(rename = "$text", default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// `rated_value`: text + `@unit @nominal @continuous @peak @max` (motor voltage/current).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RatedValue {
    #[serde(rename = "@unit", default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(rename = "@nominal", default, skip_serializing_if = "Option::is_none")]
    pub nominal: Option<String>,
    #[serde(
        rename = "@continuous",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub continuous: Option<String>,
    #[serde(rename = "@peak", default, skip_serializing_if = "Option::is_none")]
    pub peak: Option<String>,
    #[serde(rename = "@max", default, skip_serializing_if = "Option::is_none")]
    pub max: Option<String>,
    #[serde(rename = "$text", default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// `<friction static= dynamic=>` for collision surfaces and dynamic surfaces, with an optional
/// anisotropic `<friction-direction>` child.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Friction {
    #[serde(rename = "@static", default, skip_serializing_if = "Option::is_none")]
    pub static_: Option<String>,
    #[serde(rename = "@dynamic", default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<String>,
    /// `<friction-direction>`: anisotropic (direction-dependent) friction: fdir1 + mu2 + slip1/slip2
    /// (SDF surface/friction/ode). Absent =&gt; isotropic friction from `@static`/`@dynamic`.
    #[serde(
        rename = "friction-direction",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub friction_direction: Option<FrictionDirection>,
}

/// `<friction-direction mu2= slip1= slip2=>` with an optional `<fdir1>` unit-vector child. The first
/// friction direction (fdir1), the second-direction coefficient (mu2), and per-direction slip
/// compliance. SDF surface/friction/ode fdir1 + mu2 + slip1/slip2.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FrictionDirection {
    /// `<fdir1>`: first friction direction as a unit vector "x y z".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fdir1: Option<String>,
    #[serde(rename = "@mu2", default, skip_serializing_if = "Option::is_none")]
    pub mu2: Option<String>,
    #[serde(rename = "@slip1", default, skip_serializing_if = "Option::is_none")]
    pub slip1: Option<String>,
    #[serde(rename = "@slip2", default, skip_serializing_if = "Option::is_none")]
    pub slip2: Option<String>,
}

/// `<contact stiffness= damping=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Contact {
    #[serde(
        rename = "@stiffness",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub stiffness: Option<String>,
    #[serde(rename = "@damping", default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<String>,
}
