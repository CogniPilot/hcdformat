//! `<hmi>`: a human-machine interface element (display/speaker/button/...).
use super::enums::HmiType;
use super::geometry::Geometry;
use super::Pose;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "hmi")]
pub struct HmiElement {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<HmiType>,
    #[serde(rename = "@visual", default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<String>,
    #[serde(rename = "@mesh", default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
    /// `<illumination>`: the emitted-light model of a scene-illumination HMI (`@type="led-illumination"`),
    /// a headlight/work-light/IR illuminator. Maps bidirectionally to an SDF `<light>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub illumination: Option<LedIllumination>,
}

/// `<illumination light-type= cast-shadows= intensity=>` with diffuse/specular/direction/attenuation:
/// the photometric parameters of a scene-illumination light (SDF `<light>`). The light's placement is
/// the owning [`HmiElement`]'s `<pose>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LedIllumination {
    #[serde(
        rename = "@light-type",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub light_type: Option<String>,
    #[serde(
        rename = "@cast-shadows",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cast_shadows: Option<String>,
    #[serde(
        rename = "@intensity",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub intensity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diffuse: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specular: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attenuation: Option<LightAttenuation>,
}

/// `<attenuation>`: distance falloff of a light (SDF `light/attenuation`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LightAttenuation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quadratic: Option<String>,
}
