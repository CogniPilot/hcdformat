//! The official `pose` complexType: `xyz` / `rpy` / optional `quat` ATTRIBUTES (quat wins).
//!
//! This single type is reused by every pose-bearing element (13 sites in the schema) and is the core
//! fix versus the legacy dendrite dialect, which read `<pose>x y z r p y</pose>` as element text and
//! silently collapsed every pose to identity when fed official HCDF.
//!
//! ## Attribute PRESENCE is preserved
//! Each of `xyz` / `rpy` / `quat` is an [`Option`], so an `<origin xyz="0 0 0.2"/>` (no `rpy`) imports
//! and re-emits WITHOUT a fabricated `rpy="0 0 0"`, byte-faithful to `hcdfdom.model.Pose`, whose
//! string-valued xyz/rpy/quat are each `None` when the source attribute is absent and whose `to_xml`
//! emits an attribute only if it is not `None`. An absent component reads as the identity `[0,0,0]` for
//! all geometric math (matrix construction, frame conversion), so kinematics are unchanged; only the
//! serialized attribute set matches the Python oracle exactly (no spurious identity attributes).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename = "pose")]
pub struct Pose {
    #[serde(
        rename = "@xyz",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::value::opt_vec3"
    )]
    pub xyz: Option<[f64; 3]>,
    #[serde(
        rename = "@rpy",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::value::opt_vec3"
    )]
    pub rpy: Option<[f64; 3]>,
    #[serde(
        rename = "@quat",
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::value::opt_quat"
    )]
    pub quat: Option<[f64; 4]>,
}

impl Pose {
    /// The translation component, defaulting an absent `xyz` to the origin (matches Python's geometric
    /// math, where a `None` xyz/rpy is treated as identity by `frames.pose_to_matrix`).
    pub fn xyz_or_zero(&self) -> [f64; 3] {
        self.xyz.unwrap_or([0.0; 3])
    }
    /// The rpy component, defaulting an absent `rpy` to the identity rotation.
    pub fn rpy_or_zero(&self) -> [f64; 3] {
        self.rpy.unwrap_or([0.0; 3])
    }
}
