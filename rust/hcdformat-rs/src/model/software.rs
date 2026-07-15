//! `<software>` (firmware identity) and `<discovered>` (runtime discovery state): native official
//! fields, NOT extensions. Plus `<urdf-compat>` on a component.
use serde::{Deserialize, Serialize};

/// `<software name=>`: version / hash / firmware-manifest-uri / params (all text children).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "software")]
pub struct Software {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(
        rename = "firmware-manifest-uri",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub firmware_manifest_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<String>,
}

/// `<discovered>`: ip / port / last-seen (text children).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "discovered")]
pub struct Discovered {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(rename = "last-seen", default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

/// `<urdf-compat link-type=>` on a component.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "urdf-compat")]
pub struct UrdfCompatComp {
    #[serde(
        rename = "@link-type",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub link_type: Option<String>,
}
