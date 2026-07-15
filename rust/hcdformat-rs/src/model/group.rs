//! Document-level kinematic grouping: `<group>` (joint groups / kinematic chains), `<state>` (named
//! kinematic states), and `<self-collision-disable>` (collision pair exclusions).
use serde::{Deserialize, Serialize};

/// `<group name= type= tip-comp= tip-frame=>`: an ordered set of joint and sub-group references.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "group")]
pub struct JointGroup {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(rename = "@tip-comp", default, skip_serializing_if = "Option::is_none")]
    pub tip_comp: Option<String>,
    #[serde(
        rename = "@tip-frame",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub tip_frame: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joint: Vec<Ref>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group: Vec<Ref>,
}

/// A `<joint ref=>` or `<group ref=>` reference inside a group.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Ref {
    #[serde(rename = "@ref", default, skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
}

/// `<state name= default=>`: a named set of joint positions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "state")]
pub struct KinematicState {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@default", default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        rename = "joint-position",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub joint_position: Vec<JointPosition>,
}

/// `<joint-position joint= value=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JointPosition {
    #[serde(rename = "@joint", default, skip_serializing_if = "Option::is_none")]
    pub joint: Option<String>,
    #[serde(rename = "@value", default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// `<self-collision-disable>`: a set of `<pair>` exclusions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "self-collision-disable")]
pub struct SelfCollisionDisable {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pair: Vec<CollisionPair>,
}

/// `<pair comp1= comp2= reason=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CollisionPair {
    #[serde(rename = "@comp1", default, skip_serializing_if = "Option::is_none")]
    pub comp1: Option<String>,
    #[serde(rename = "@comp2", default, skip_serializing_if = "Option::is_none")]
    pub comp2: Option<String>,
    #[serde(rename = "@reason", default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
