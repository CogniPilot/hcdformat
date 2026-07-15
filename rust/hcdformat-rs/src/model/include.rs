//! `<include uri= sha= name= pose=>`: composes another HCDF file as a sub-assembly.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "include")]
pub struct Include {
    #[serde(rename = "@uri", default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(rename = "@sha", default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@pose", default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<String>,
    /// `@static`: import the sub-assembly as a fixed body (SDF include/static).
    #[serde(rename = "@static", default, skip_serializing_if = "Option::is_none")]
    pub static_: Option<String>,
    /// `@placement-frame`: the frame within the included model that `@pose` places (SDF
    /// include/placement_frame).
    #[serde(
        rename = "@placement-frame",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub placement_frame: Option<String>,
}
