//! `<joint>` and its children: parent/child refs, origin pose, axes, limit, dynamics, calibration,
//! mimic, loop closure, and the optional urdf-compat block. `<extension>` inside a joint is captured
//! verbatim like any other (see [`crate::model::Extension`]).
use super::enums::{Handedness, JointType, PitchConvention};
use super::{Extension, Pose};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "joint")]
pub struct Joint {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<JointType>,
    #[serde(
        rename = "@thread_pitch",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub thread_pitch: Option<String>,
    /// `@pitch_convention`: unit convention of `@thread_pitch` (`m_per_rev` canonical / `rad_per_m`
    /// legacy). Absent => canonical `m_per_rev`. Screw-only (see [`crate::validate`]).
    #[serde(
        rename = "@pitch_convention",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub pitch_convention: Option<PitchConvention>,
    /// `@handedness`: thread chirality of `@thread_pitch` (`right` canonical / `left` legacy). Absent
    /// => canonical `right`. Screw-only.
    #[serde(
        rename = "@handedness",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub handedness: Option<Handedness>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<JointEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<JointEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<Axis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis2: Option<Axis>,
    /// `<limit2>`: limits for the second DOF (axis2) of a universal/revolute2 joint (SDF axis2/limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit2: Option<JointLimit>,
    /// `<dynamics2>`: dynamics for the second DOF (axis2) (SDF axis2/dynamics).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamics2: Option<JointDynamics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<JointLimit>,
    /// `<swing_limit>`: ball-joint swing-cone half-angles (radians; circular or elliptic). Ball-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing_limit: Option<SwingLimit>,
    /// `<twist_limit>`: ball-joint twist bound (rotation about the twist axis, radians). Ball-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twist_limit: Option<JointLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamics: Option<JointDynamics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<JointCalibration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mimic: Option<Mimic>,
    /// `<loop>` (renamed; `loop` is a Rust keyword).
    #[serde(rename = "loop", default, skip_serializing_if = "Option::is_none")]
    pub loop_: Option<LoopClosure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<Extension>,
    #[serde(
        rename = "urdf-compat",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub urdf_compat: Option<UrdfCompatJoint>,

    /// XML comments captured inside this `<joint>` (joint-relative paths; [`crate::comments`]).
    /// NEVER part of the schema/serde shape (equality-transparent; travels with the struct).
    #[serde(skip)]
    pub comments: crate::comments::CommentSet,
}

/// `<parent comp=>` / `<child comp=>` (same shape).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JointEndpoint {
    #[serde(rename = "@comp", default, skip_serializing_if = "Option::is_none")]
    pub comp: Option<String>,
}

/// `<axis xyz=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Axis {
    #[serde(rename = "@xyz", default, skip_serializing_if = "Option::is_none")]
    pub xyz: Option<String>,
}

/// `<limit lower= upper= effort= velocity= acceleration= jerk= deceleration=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JointLimit {
    #[serde(rename = "@lower", default, skip_serializing_if = "Option::is_none")]
    pub lower: Option<String>,
    #[serde(rename = "@upper", default, skip_serializing_if = "Option::is_none")]
    pub upper: Option<String>,
    #[serde(rename = "@effort", default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(rename = "@velocity", default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<String>,
    #[serde(
        rename = "@acceleration",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub acceleration: Option<String>,
    #[serde(rename = "@jerk", default, skip_serializing_if = "Option::is_none")]
    pub jerk: Option<String>,
    #[serde(
        rename = "@deceleration",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub deceleration: Option<String>,
}

/// `<swing_limit swing1= swing2= effort= velocity=>`: a ball joint's swing-cone (half-angles in
/// radians). `swing1 == swing2` (or `swing2` omitted) => circular cone; unequal => elliptic cone.
/// Maps to Bullet `btConeTwistConstraint`, USD spherical cone, MuJoCo scalar cone.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SwingLimit {
    #[serde(rename = "@swing1", default, skip_serializing_if = "Option::is_none")]
    pub swing1: Option<String>,
    #[serde(rename = "@swing2", default, skip_serializing_if = "Option::is_none")]
    pub swing2: Option<String>,
    #[serde(rename = "@effort", default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(rename = "@velocity", default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<String>,
}

/// `<dynamics damping= friction= spring_stiffness= spring_reference=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JointDynamics {
    #[serde(rename = "@damping", default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<String>,
    #[serde(rename = "@friction", default, skip_serializing_if = "Option::is_none")]
    pub friction: Option<String>,
    #[serde(
        rename = "@spring_stiffness",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub spring_stiffness: Option<String>,
    #[serde(
        rename = "@spring_reference",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub spring_reference: Option<String>,
}

/// `<calibration reference_position= rising= falling=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JointCalibration {
    #[serde(
        rename = "@reference_position",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reference_position: Option<String>,
    #[serde(rename = "@rising", default, skip_serializing_if = "Option::is_none")]
    pub rising: Option<String>,
    #[serde(rename = "@falling", default, skip_serializing_if = "Option::is_none")]
    pub falling: Option<String>,
}

/// `<mimic joint= multiplier= offset=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Mimic {
    #[serde(rename = "@joint", default, skip_serializing_if = "Option::is_none")]
    pub joint: Option<String>,
    #[serde(
        rename = "@multiplier",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub multiplier: Option<String>,
    #[serde(rename = "@offset", default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<String>,
}

/// `<loop>`: predecessor/successor/constraint-axes (text element children).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LoopClosure {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predecessor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor: Option<String>,
    #[serde(
        rename = "constraint-axes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub constraint_axes: Option<String>,
}

/// `<urdf-compat>` on a joint: optional `<safety_controller>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UrdfCompatJoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_controller: Option<SafetyController>,
}

/// `<safety_controller soft_lower_limit= soft_upper_limit= k_position= k_velocity=>`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SafetyController {
    #[serde(
        rename = "@soft_lower_limit",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub soft_lower_limit: Option<String>,
    #[serde(
        rename = "@soft_upper_limit",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub soft_upper_limit: Option<String>,
    #[serde(
        rename = "@k_position",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub k_position: Option<String>,
    #[serde(
        rename = "@k_velocity",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub k_velocity: Option<String>,
}
