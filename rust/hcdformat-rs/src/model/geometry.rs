//! Geometry primitives and the THREE distinct geometry container types from the official schema:
//!
//! - [`VisualGeometry`]: box/cylinder/sphere/capsule/cone/ellipsoid (NO mesh, NO frustum).
//! - [`CollisionGeometry`]: those six + mesh (NO frustum).
//! - [`Geometry`]: general, those six + mesh + frustum (used by sensor / FOV / port / antenna / hmi).
//!
//! Keeping them separate makes an illegal primitive (e.g. frustum in collision) unrepresentable.
//! Primitive dimensions are kept as text strings (e.g. `<size>0.048 0.044 0.012</size>`) so authored
//! values round-trip exactly.
use super::enums::FrustumShape;
use serde::{Deserialize, Serialize};

macro_rules! text_child {
    ($name:ident { $($f:ident => $tag:literal),* $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
        pub struct $name {
            $(
                #[serde(rename = $tag, default, skip_serializing_if = "Option::is_none")]
                pub $f: Option<String>,
            )*
        }
    };
}

text_child!(Box_ { size => "size" });
text_child!(Cylinder { radius => "radius", length => "length" });
text_child!(Sphere { radius => "radius" });
text_child!(Capsule { radius => "radius", length => "length" });
text_child!(Cone { radius => "radius", length => "length" });
text_child!(Ellipsoid { radii => "radii" });

/// `<mesh uri= scale= sha= source-uri=>` with an optional `<submesh>` sub-part selector.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Mesh {
    #[serde(rename = "@uri", default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(rename = "@scale", default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<String>,
    #[serde(rename = "@sha", default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(
        rename = "@source-uri",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub source_uri: Option<String>,
    /// `<submesh name= center=>`: load only a single named sub-part of a multi-part mesh file
    /// (SDF mesh/submesh). Absent =&gt; the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submesh: Option<Submesh>,
}

/// `<submesh name= center=>`: selects a single named sub-part (node/group) within a mesh file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Submesh {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "@center", default, skip_serializing_if = "Option::is_none")]
    pub center: Option<String>,
}

/// `<exclude-submesh name=>`: names a sub-part (node/group) to SUBTRACT from a model's loaded
/// geometry: the whole model minus this subtree, the complement of [`Submesh`]'s include. Carries
/// NO `@center` (a subtraction has nothing to recenter): the exclusion cannot even spell it, so the
/// illegal state is unrepresentable in the grammar rather than validator-caught. Used only by
/// [`crate::model::ModelRef`] in exclude mode.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ExcludeSubmesh {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `<frustum shape=>` with near/far + (fov | hfov+vfov). Used only in general [`Geometry`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Frustum {
    #[serde(rename = "@shape", default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<FrustumShape>,
    #[serde(rename = "near", default, skip_serializing_if = "Option::is_none")]
    pub near: Option<String>,
    #[serde(rename = "far", default, skip_serializing_if = "Option::is_none")]
    pub far: Option<String>,
    #[serde(rename = "fov", default, skip_serializing_if = "Option::is_none")]
    pub fov: Option<String>,
    #[serde(rename = "hfov", default, skip_serializing_if = "Option::is_none")]
    pub hfov: Option<String>,
    #[serde(rename = "vfov", default, skip_serializing_if = "Option::is_none")]
    pub vfov: Option<String>,
}

/// Visual primitives only (no mesh / frustum).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VisualGeometry {
    #[serde(rename = "box", default, skip_serializing_if = "Option::is_none")]
    pub box_: Option<Box_>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cylinder: Option<Cylinder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sphere: Option<Sphere>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule: Option<Capsule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone: Option<Cone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ellipsoid: Option<Ellipsoid>,
}

/// Collision geometry: visual primitives + mesh (no frustum).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CollisionGeometry {
    #[serde(rename = "box", default, skip_serializing_if = "Option::is_none")]
    pub box_: Option<Box_>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cylinder: Option<Cylinder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sphere: Option<Sphere>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule: Option<Capsule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone: Option<Cone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ellipsoid: Option<Ellipsoid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<Mesh>,
}

/// General geometry: primitives + mesh + frustum.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Geometry {
    #[serde(rename = "box", default, skip_serializing_if = "Option::is_none")]
    pub box_: Option<Box_>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cylinder: Option<Cylinder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sphere: Option<Sphere>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule: Option<Capsule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cone: Option<Cone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ellipsoid: Option<Ellipsoid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<Mesh>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frustum: Option<Frustum>,
}
