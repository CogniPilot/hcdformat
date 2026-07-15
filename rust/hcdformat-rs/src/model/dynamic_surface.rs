//! `<dynamic-surface>`: a force-producing / contact surface (wheel/track/prop/aerofoil/...). The
//! container plus the wheel surface (present in fixtures) are typed; other surface kinds are modeled
//! loosely via lenient ignore but their presence still parses.
use super::common::{Friction, MeasuredValue, RangeValue};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "dynamic-surface")]
pub struct DynamicSurface {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@visual", default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<String>,
    #[serde(rename = "@mesh", default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prop: Option<PropSurface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aerofoil: Option<AerofoilSurface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hydrofoil: Option<HydrofoilSurface>,
    #[serde(
        rename = "control-surface",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub control_surface: Option<ControlSurface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wheel: Option<WheelSurface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackSurface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gripper: Option<GripperSurface>,
}

/// `<aerofoil>` (`aerofoil_surface`, hcdf.xsd:1381): aerodynamic lifting surface: wing, stabilizer,
/// canard. `profile` is an airfoil designation text (e.g. "NACA2412"); the rest are [`MeasuredValue`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AerofoilSurface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chord: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dihedral: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<MeasuredValue>,
}

/// `<hydrofoil>` (`hydrofoil_surface`, hcdf.xsd:1405): hydrodynamic lifting surface, an underwater foil.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HydrofoilSurface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chord: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<MeasuredValue>,
}

/// `<control-surface type=>` (`control_surface_def`, hcdf.xsd:1423): hinged aerodynamic control
/// surface (aileron/elevator/rudder/elevon/flap). `chord-ratio`/`span-fraction` are `xs:double` kept
/// as `String` (text-preserving); `deflection` is a [`RangeValue`]. `@type` is required by the XSD.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ControlSurface {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(
        rename = "chord-ratio",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub chord_ratio: Option<String>,
    #[serde(
        rename = "span-fraction",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub span_fraction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deflection: Option<RangeValue>,
}

/// `<track>` (`track_surface`, hcdf.xsd:1459): tracked-vehicle surface (tank treads / rubber tracks).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TrackSurface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<MeasuredValue>,
    #[serde(
        rename = "ground-pressure",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ground_pressure: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friction: Option<Friction>,
}

/// `<gripper type=>` (`gripper_surface`, hcdf.xsd:1477): end-effector contact surface: mechanical
/// pads, suction cups, magnetic, adhesive, or granular jamming. Physical-quantity leaves are
/// [`MeasuredValue`]; `shore-hardness` is a scale+value text (e.g. "A30"). `@type` is required by the XSD.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GripperSurface {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(
        rename = "grip-force",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub grip_force: Option<MeasuredValue>,
    #[serde(
        rename = "contact-area",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub contact_area: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<MeasuredValue>,
    #[serde(
        rename = "shore-hardness",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub shore_hardness: Option<String>,
    #[serde(
        rename = "vacuum-level",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub vacuum_level: Option<MeasuredValue>,
    #[serde(
        rename = "magnetic-force",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub magnetic_force: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friction: Option<Friction>,
}

/// `<prop>`: propeller surface, generating thrust from rotation (multirotor/fixed-wing/marine).
/// `diameter`/`pitch` are physical-quantity [`MeasuredValue`]; `blades`/`direction` are text leaves.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PropSurface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diameter: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blades: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
}

/// `<wheel type=>`: radius/width/friction.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WheelSurface {
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<MeasuredValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friction: Option<Friction>,
}
