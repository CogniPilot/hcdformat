//! `<inertial>`: mass + center-of-gravity (`inertia_origin`, a [`Pose`]) + inertia tensor text.
use super::Pose;
use serde::{Deserialize, Serialize};

/// `inertial_properties`: `<mass>` (text), `<inertia_origin>` (pose), `<inertia>` (six-tuple text).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "inertial")]
pub struct Inertial {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inertia_origin: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inertia: Option<String>,
}
