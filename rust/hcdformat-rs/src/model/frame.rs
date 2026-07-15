//! `<frame name= relative-to= type=>`: a named reference frame on a component.
use super::Pose;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "frame")]
pub struct Frame {
    /// `@name` is REQUIRED by the schema.
    #[serde(rename = "@name", default)]
    pub name: String,
    #[serde(
        rename = "@relative-to",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub relative_to: Option<String>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
}
