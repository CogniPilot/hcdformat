//! `<collision>`: optional pose + collision geometry + optional surface (friction/restitution/contact).
use super::common::{Contact, Friction};
use super::enums::NameOrigin;
use super::geometry::CollisionGeometry;
use super::Pose;
use serde::{Deserialize, Serialize};

/// `<collision name= verbose= name-origin=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "collision")]
pub struct Collision {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@verbose", default, skip_serializing_if = "Option::is_none")]
    pub verbose: Option<String>,
    #[serde(
        rename = "@name-origin",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub name_origin: Option<NameOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<CollisionGeometry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<Surface>,
}

/// `<surface>`: `<friction>` + `<restitution>` (text) + `<contact>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Surface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friction: Option<Friction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restitution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<Contact>,
}
