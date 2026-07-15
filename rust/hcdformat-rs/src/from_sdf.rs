//! SDF -> HCDF import (feature = `sdf`), a faithful port of `sdf_io/from_sdf.py`.
//!
//! Mapping spine: `<model>` -> [`Hcdf`], `<link>` -> [`Comp`], `<joint>` -> [`Joint`],
//! `<inertial>`/`<collision>+<surface>`/`<visual>+<material>`/`<geometry>`/`<axis>+<limit>+<dynamics>`/
//! `<mimic>`. SDF, like URDF, is FLU/ENU, so the imported document is tagged `body-frame="FLU"
//! world-frame="ENU"` and needs NO pose transform (frame conversion is an *export*-side concern, see
//! [`mod@crate::to_sdf`]).
//!
//! ## Parser strategy (mirrors `from_sdf.py`'s tolerant lxml `recover=True` walk)
//! CRITICAL: `from_sdf.py` parses with `etree.XMLParser(recover=True)`, a deliberately TOLERANT parse
//! (unlike `from_urdf.py`, which parses URDF strictly). It tolerates missing `name=` attributes, missing
//! spec-defaulted elements, `model://` uris, and even undeclared-prefix legacy tags (`<sensor:contact>`),
//! mapping the mechanical overlap and dropping what has no HCDF home. So this importer does the same: it
//! drives the mapping straight from a tolerant quick-xml element tree (`SdfEl`) and DOES NOT impose a
//! strict structural-validation gate. (An earlier version gated on `sdformat::from_str::<SdfRoot>`, but
//! that crate is a SEMANTIC validator far stricter than lxml-recover: it rejects a `<scene>` missing
//! `<shadows>`, an unnamed `<collision>`, etc., so it rejected valid SDF the Python authority imports.
//! The `sdformat` crate stays only as the optional `gz`-free pre-resolver dependency surface; it is not
//! a parse gate.) The quick-xml tree preserves *exact text* (so `5.19711e-05` is mapped verbatim, not
//! float round-tripped) and *element presence* (so an absent `<rpy>`/`<dynamics>` stays absent), both
//! required for byte-semantic parity with `from_sdf.py`'s lxml `.find()`/`.text`/drop-when-absent walk.
//! The only divergence forced by the typed HCDF model: a [`Pose`] stores `xyz`/`rpy` as `Option<[f64;3]>`
//! (vs the Python text-Pose's strings), so the SDF `<pose>x y z r p y</pose>` six-tuple is parsed into
//! the two typed triples, numerically identical, re-serialized in the document writer's compact form.
//!
//! ## gz policy (mirrors from_sdf.py: gz is NEVER invoked)
//! `from_sdf.py` parses the SDF directly with lxml and NEVER shells to `gz` (it imports only the
//! `SDF_VERSION` constant from `gz.py`). With `gz` absent, Python still imports `world.sdf`, a `model://`
//! mesh (verbatim), unnamed collisions/visuals, and an SDF carrying `<include>` (the include reference
//! is CAPTURED into the doc-level `<include>` home, never resolved/flattened; the inline `<model>` is
//! mapped). [`from_sdf_str`] and [`from_sdf_path`] mirror this
//! exactly: both map the RAW SDF, never rejecting model://. There is NO `gz` subprocess anywhere in this
//! crate: a caller that *chooses* to resolve `<include>`/`model://`/Fuel or fill spec-defaults first runs
//! `gz sdf --print` externally and feeds the canonical output to [`from_sdf_str`]: an explicit external
//! pre-pass, never an automatic one.
//!
//! ## Frame semantics (SDF 1.7+ pose frame graph)
//! Link, joint, and `<frame>` poses (including their `relative_to` qualifiers) are resolved through a
//! per-model frame graph (the private `FrameGraph`) built on the spec defaults: a link pose is relative to the
//! MODEL frame, a joint pose to its CHILD link, a `<frame>` pose to its `attached_to` frame. Each HCDF
//! joint origin is then emitted as `X(parent comp ← child comp)`, the convention [`mod@crate::to_sdf`]
//! round-trips, so a pose-less fixed joint whose placement lives entirely in the child link's model
//! pose still gets a correct origin, and a `<frame>` maps onto the comp of the link it is attached to.
//! One representational limit (like URDF): HCDF anchors a joint at the child comp origin, so an SDF
//! joint frame OFFSET from its child link origin on a non-fixed joint keeps exact zero-configuration
//! placement but not the rotation anchor (noted). Visual/collision/inertial poses are element-local by
//! spec default (relative to their enclosing link) and are mapped exactly as before; an explicit
//! `relative_to` on those stays noted-and-local.
use crate::asset_hint::VisualAssetHint;
use crate::compose::pose_math::{mat3_vec, matrix_to_rpy, mul33, rpy_to_matrix, transpose};
use crate::error::{Error, Result};
use crate::model::enums::{
    BodyFrame, ForceFrame, FrustumShape, HmiType, InertialSensorType, JointType, MeasureDirection,
    NameOrigin, NoiseType, OpticalSensorType, TransmissionType, WorldFrame,
};
use crate::model::{
    Axis, AxisNoise, BatterySource, Box_, CameraDistortion, CameraIntrinsics, CameraLens,
    CameraLensCustomFunction, CameraMatrix, CameraParams, CameraProjection, Capsule, Collision,
    CollisionGeometry, Color, Comp, Cone, Contact, Cylinder, Ellipsoid, Endpoint, Extension,
    FluidSensor, ForceAxisNoise, Frame, Friction, FrictionDirection, Frustum, Geometry, Hcdf,
    HmiElement, Include, Inertial, InertialSensor, Joint, JointDynamics, JointEndpoint, JointLimit,
    LedIllumination, LidarParams, LidarRange, LidarScanAxis, LidarScanPattern, LightAttenuation,
    MeasuredValue, Mesh, Mimic, ModelRef, Noise, OpticalSensor, Pose, PowerSource, RangeValue,
    Sensor, SensorCategory, SensorFov, SensorParams, Sphere, Submesh, Surface, Transmission,
    Visual, VisualAppearance, VisualGeometry,
};
use crate::pyrepr::repr_str;
// Gazebo extension domain/root constants live in `to_urdf` (compiled whenever either converter is on)
// so the URDF and SDF front ends agree on the domain split without a cross-feature dependency.
use crate::to_urdf::{
    GAZEBO_DOMAIN, GAZEBO_RAW_DOMAIN, GAZEBO_SIM_ROOT, ROS2_DOMAIN, ROS2_TOPIC_MAP_ROOT,
};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

/// Physics-engine literals the typed `<gazebo-sim><physics><engine>` accepts (hcdf-ext-gazebo.xsd
/// `PhysicsEngine`). An SDF `<physics type=...>` outside this set leaves `<engine>` unset (noted).
const PHYSICS_ENGINES: &[&str] = &["ode", "bullet", "dart", "simbody", "tpe"];

/// Pinned SDFormat spec version (libsdformat 16.x emits/consumes 1.12): the version this importer warns
/// against on mismatch and [`mod@crate::to_sdf`] stamps on export. Matches `gz.py`'s `SDF_VERSION` and the
/// SDFormat spec libsdformat 16.x binds to.
pub const SDF_VERSION: &str = "1.12";

/// SDF joint types that map 1:1 to an HCDF joint type. SDF `revolute2` has no 1:1 literal but is
/// approximated as HCDF `universal` (handled separately in [`joint`]); SDF `gearbox` is a gear-coupling
/// constraint (not a kinematic DOF) and is intercepted in [`from_model`] as a typed `<transmission
/// type="gear">` coupling (reaching [`joint`]'s `fixed` downgrade only as the dangling-child edge
/// fallback); any unknown type downgrades to `fixed`.
const JTYPE: &[&str] = &[
    "revolute",
    "prismatic",
    "fixed",
    "continuous",
    "ball",
    "universal",
    "screw",
];

// ── a minimal exact-text quick-xml DOM (mirrors lxml's element access) ──────────────────────────

/// A parsed SDF element preserving attributes as exact text, child order, and the element's own text
/// content, the read surface the Python importer uses through lxml `.find()`/`.get()`/`.text`.
struct SdfEl {
    tag: String,
    attrs: BTreeMap<String, String>,
    text: Option<String>,
    children: Vec<SdfEl>,
}

impl SdfEl {
    /// Attribute value (lxml `.get`).
    fn get(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(|s| s.as_str())
    }
    /// First direct child with the given local name (lxml `.find`).
    fn find(&self, name: &str) -> Option<&SdfEl> {
        self.children.iter().find(|c| c.tag == name)
    }
    /// All direct children with the given local name (lxml `.findall`).
    fn find_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a SdfEl> + 'a {
        self.children.iter().filter(move |c| c.tag == name)
    }
    /// First descendant by a slash path `a/b/c` (lxml `.find("a/b/c")`).
    fn find_path(&self, path: &str) -> Option<&SdfEl> {
        let mut cur = self;
        for seg in path.split('/') {
            cur = cur.find(seg)?;
        }
        Some(cur)
    }
    /// The element's stripped text (lxml `el.text.strip()` via the `_txt` helper). `None` when empty.
    fn txt(&self) -> Option<&str> {
        self.text
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
    /// The stripped text of a named child (the very common `_txt(el.find("tag"))` pattern).
    fn child_txt(&self, name: &str) -> Option<&str> {
        self.find(name).and_then(|c| c.txt())
    }
}

/// Parse the raw SDF into an `SdfEl` tree, preserving attributes/text/child order. Uses the same
/// quick-xml dependency the document reader does; no new dep, wasm-clean.
fn parse_dom(src: &str) -> Result<SdfEl> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(src);
    reader.config_mut().expand_empty_elements = false;
    let mut stack: Vec<SdfEl> = Vec::new();
    let mut root: Option<SdfEl> = None;
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) => {
                stack.push(SdfEl {
                    tag: local_name(&e),
                    attrs: read_attrs(&e)?,
                    text: None,
                    children: Vec::new(),
                });
            }
            Event::Empty(e) => {
                let el = SdfEl {
                    tag: local_name(&e),
                    attrs: read_attrs(&e)?,
                    text: None,
                    children: Vec::new(),
                };
                push_into(&mut stack, &mut root, el);
            }
            Event::Text(t) => {
                let txt = t.unescape().map_err(|e| Error::Xml(e.to_string()))?;
                if let Some(top) = stack.last_mut() {
                    // accumulate text (SDF leaf elements carry a single text run).
                    match &mut top.text {
                        Some(existing) => existing.push_str(&txt),
                        None => top.text = Some(txt.into_owned()),
                    }
                }
            }
            Event::End(_) => {
                if let Some(done) = stack.pop() {
                    push_into(&mut stack, &mut root, done);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    root.ok_or_else(|| Error::Xml("empty SDF document".to_string()))
}

fn push_into(stack: &mut [SdfEl], root: &mut Option<SdfEl>, el: SdfEl) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(el);
    } else {
        *root = Some(el);
    }
}

fn local_name(e: &quick_xml::events::BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn read_attrs(e: &quick_xml::events::BytesStart) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for a in e.attributes() {
        let a = a.map_err(|e| Error::Xml(e.to_string()))?;
        let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
        let val = a
            .unescape_value()
            .map_err(|e| Error::Xml(e.to_string()))?
            .into_owned();
        out.insert(key, val);
    }
    Ok(out)
}

// ── poses ────────────────────────────────────────────────────────────────────────────────────────

/// SDF `<pose>x y z r p y</pose>` -> a typed HCDF [`Pose`] (xyz + rpy), for the poses whose spec
/// default frame IS today's element-local reading (visual/collision/inertial: relative to the enclosing
/// link). A frame/format qualifier on THESE poses stays noted-and-local, mirroring `from_sdf.py::_pose`;
/// link/joint/`<frame>` poses resolve their qualifiers through the [`FrameGraph`] instead.
fn pose(el: Option<&SdfEl>, notes: &mut Vec<String>, ctx: &str) -> Option<Pose> {
    let el = el?;
    note_pose_qualifiers(el, notes, ctx);
    pose_values(el, notes, ctx)
}

/// The qualifier half of [`pose`]: note a `relative_to`/`expressed_in`/`rotation_format`/`degrees`
/// attribute as not represented (the pose is then read element-local).
fn note_pose_qualifiers(el: &SdfEl, notes: &mut Vec<String>, ctx: &str) {
    if el.get("relative_to").is_some()
        || el.get("expressed_in").is_some()
        || el.get("rotation_format").is_some()
        || el.get("degrees").is_some()
    {
        let attr = el.get("relative_to").or_else(|| el.get("expressed_in"));
        notes.push(format!(
            "{ctx}: <pose> frame qualifier ({}/format) not represented; pose taken as element-local",
            repr_opt(attr)
        ));
    }
}

/// The value half of [`pose`]: parse the six-tuple text, noting-and-dropping a non-6-value pose.
fn pose_values(el: &SdfEl, notes: &mut Vec<String>, ctx: &str) -> Option<Pose> {
    let vals: Vec<&str> = el
        .text
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .collect();
    if vals.len() != 6 {
        notes.push(format!(
            "{ctx}: <pose> has {} values (expected 6); dropped",
            vals.len()
        ));
        return None;
    }
    let nums: Vec<f64> = vals.iter().map(|t| t.parse().unwrap_or(0.0)).collect();
    Some(Pose {
        xyz: Some([nums[0], nums[1], nums[2]]),
        rpy: Some([nums[3], nums[4], nums[5]]),
        quat: None,
    })
}

/// [`pose_values`] without diagnostics, for re-reads of graph poses whose malformations were already
/// noted at [`FrameGraph::build`] time.
fn pose_values_silent(el: Option<&SdfEl>) -> Option<Pose> {
    let mut scratch = Vec::new();
    el.and_then(|e| pose_values(e, &mut scratch, ""))
}

/// Python `repr` of an `Option<&str>`: `'value'` or `None`. (Local copy: from_sdf forms `{attr!r}`.)
fn repr_opt(v: Option<&str>) -> String {
    match v {
        Some(s) => repr_str(s),
        None => "None".to_string(),
    }
}

// ── SDF 1.7 pose frame graph ──────────────────────────────────────────────────────────────────────

/// X(model←frame) as translation + rotation. The rotation keeps its authored rpy triple for as long as
/// it is the ONLY non-identity rotation in the composition chain, so an authored `0 1.57 0` reaches the
/// HCDF origin VERBATIM instead of round-tripping through asin/atan2 (which would emit
/// `1.5699999999999876`).
#[derive(Clone)]
struct XForm {
    t: [f64; 3],
    r: RotK,
}

/// The rotation kind of an [`XForm`], tracked so a lone authored rpy survives composition verbatim.
#[derive(Clone)]
enum RotK {
    Ident,
    /// An authored rpy + its cached 3×3 (composition uses the matrix, extraction the triple).
    Rpy {
        rpy: [f64; 3],
        m: [[f64; 3]; 3],
    },
    /// A genuinely composed rotation; rpy recovered via [`matrix_to_rpy`] on extraction.
    Mat([[f64; 3]; 3]),
}

impl XForm {
    fn identity() -> Self {
        XForm {
            t: [0.0; 3],
            r: RotK::Ident,
        }
    }
    fn from_pose(p: &Pose) -> Self {
        let rpy = p.rpy_or_zero();
        let r = if rpy == [0.0; 3] {
            RotK::Ident
        } else {
            RotK::Rpy {
                rpy,
                m: rpy_to_matrix(rpy[0], rpy[1], rpy[2]),
            }
        };
        XForm {
            t: p.xyz_or_zero(),
            r,
        }
    }
    /// `self ∘ other` (apply `other` inside `self`'s frame): `t = t_s + R_s·t_o`, `R = R_s·R_o`.
    fn compose(&self, other: &XForm) -> XForm {
        let rt = match &self.r {
            RotK::Ident => other.t,
            _ => mat3_vec(&self.rot(), other.t),
        };
        let r = match (&self.r, &other.r) {
            (RotK::Ident, x) | (x, RotK::Ident) => x.clone(),
            _ => RotK::Mat(mul33(&self.rot(), &other.rot())),
        };
        XForm {
            t: [self.t[0] + rt[0], self.t[1] + rt[1], self.t[2] + rt[2]],
            r,
        }
    }
    /// The inverse transform: `R' = Rᵀ`, `t' = −R'·t`.
    fn inverse(&self) -> XForm {
        match &self.r {
            RotK::Ident => XForm {
                t: [-self.t[0], -self.t[1], -self.t[2]],
                r: RotK::Ident,
            },
            _ => {
                let rt = transpose(&self.rot());
                let it = mat3_vec(&rt, self.t);
                XForm {
                    t: [-it[0], -it[1], -it[2]],
                    r: RotK::Mat(rt),
                }
            }
        }
    }
    /// The 3×3 rotation block.
    fn rot(&self) -> [[f64; 3]; 3] {
        match &self.r {
            RotK::Ident => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            RotK::Rpy { m, .. } | RotK::Mat(m) => *m,
        }
    }
    /// Whether the transform is the identity within `eps` (per translation component / matrix entry).
    fn is_identity(&self, eps: f64) -> bool {
        self.t.iter().all(|v| v.abs() <= eps) && rot_is_identity(&self.rot(), eps)
    }
    /// Back to a typed [`Pose`]: the authored rpy verbatim when the rotation is a single triple.
    fn to_pose(&self) -> Pose {
        let rpy = match &self.r {
            RotK::Ident => [0.0; 3],
            RotK::Rpy { rpy, .. } => *rpy,
            RotK::Mat(m) => {
                let (roll, pitch, yaw) = matrix_to_rpy(m);
                [roll, pitch, yaw]
            }
        };
        Pose {
            xyz: Some(self.t),
            rpy: Some(rpy),
            quat: None,
        }
    }
}

/// Whether a 3×3 rotation is the identity within `eps` per entry.
fn rot_is_identity(m: &[[f64; 3]; 3], eps: f64) -> bool {
    m.iter().enumerate().all(|(i, row)| {
        row.iter()
            .enumerate()
            .all(|(j, v)| (v - if i == j { 1.0 } else { 0.0 }).abs() <= eps)
    })
}

/// One node of the SDF 1.7 pose frame graph (a link, joint, `<frame>`, or the model frame itself).
struct FrameNode {
    /// What declared the frame ("link" / "joint" / "frame" / "model"), for diagnostics + attach chains.
    kind: &'static str,
    /// The parsed `<pose>` six-tuple; `None` = identity (absent/empty/malformed pose).
    pose: Option<Pose>,
    /// The frame the pose is expressed in, spec default already applied (`__model__` for links, the
    /// child link for joints, `attached_to` for `<frame>`s).
    relative_to: String,
    /// X(model←node), filled by [`FrameGraph::resolve`].
    resolved: Option<XForm>,
    /// Whether resolution FELL BACK to model-relative flattening (dangling `relative_to` or a cycle);
    /// such a node's local pose is no longer X(relative_to←node), so chain walks must skip it.
    flattened: bool,
}

impl FrameNode {
    /// The node's own pose as an [`XForm`] (identity when absent/dropped).
    fn local(&self) -> XForm {
        self.pose
            .as_ref()
            .map(XForm::from_pose)
            .unwrap_or_else(XForm::identity)
    }
}

/// The SDF 1.7 pose frame graph of one `<model>`: every referable frame resolved to X(model←frame).
struct FrameGraph {
    /// Nodes keyed by frame name; `"__model__"` is pre-resolved to the identity.
    nodes: BTreeMap<String, FrameNode>,
    /// The canonical link (`@canonical_link`, default first `<link>`); the model frame is attached to
    /// it, so it owns `__model__`-attached `<frame>`s.
    canonical: Option<String>,
    /// `<frame name>` -> raw `attached_to`, for resolving frame ownership down to a link.
    frame_attach: BTreeMap<String, String>,
    /// Joint name -> child link name (a joint frame is attached to its child link).
    joint_child: BTreeMap<String, String>,
}

impl FrameGraph {
    /// Build and resolve the frame graph of `model`.
    fn build(model: &SdfEl, notes: &mut Vec<String>) -> FrameGraph {
        let mut g = FrameGraph {
            nodes: BTreeMap::new(),
            canonical: model
                .get("canonical_link")
                .filter(|s| !s.is_empty())
                .or_else(|| model.find("link").and_then(|l| l.get("name")))
                .map(str::to_string),
            frame_attach: BTreeMap::new(),
            joint_child: BTreeMap::new(),
        };
        g.nodes.insert(
            "__model__".to_string(),
            FrameNode {
                kind: "model",
                pose: None,
                relative_to: String::new(),
                resolved: Some(XForm::identity()),
                flattened: false,
            },
        );
        for lel in model.find_all("link") {
            let Some(name) = lel.get("name").filter(|s| !s.is_empty()) else {
                continue; // an unnamed link is unreferable; no frame node
            };
            let ctx = format!("link {}", repr_str(name));
            let (pose, relative_to) = node_pose(lel, "__model__", &ctx, notes);
            g.insert(name, "link", pose, relative_to, notes);
        }
        for jel in model.find_all("joint") {
            let child = jel.child_txt("child");
            let Some(name) = jel.get("name").filter(|s| !s.is_empty()) else {
                // an unnamed joint is unreferable, no node; still surface a malformed <pose>.
                if let Some(pel) = jel.find("pose") {
                    let _ = pose_values(pel, notes, "joint None");
                }
                continue;
            };
            let ctx = format!("joint {}", repr_str(name));
            if let Some(c) = child {
                g.joint_child.insert(name.to_string(), c.to_string());
            }
            let default_rel = match child {
                Some(c) => c,
                None => {
                    notes.push(format!(
                        "{ctx}: no <child>; joint pose taken relative to the model frame"
                    ));
                    "__model__"
                }
            };
            let (pose, relative_to) = node_pose(jel, default_rel, &ctx, notes);
            g.insert(name, "joint", pose, relative_to, notes);
        }
        for fel in model.find_all("frame") {
            let Some(name) = fel.get("name").filter(|s| !s.is_empty()) else {
                continue; // an unnamed <frame> is unreferable; the mapping pass notes the drop
            };
            let ctx = format!("frame {}", repr_str(name));
            let attached = fel
                .get("attached_to")
                .filter(|s| !s.is_empty())
                .unwrap_or("__model__");
            g.frame_attach
                .insert(name.to_string(), attached.to_string());
            let (pose, relative_to) = node_pose(fel, attached, &ctx, notes);
            g.insert(name, "frame", pose, relative_to, notes);
        }
        g.resolve(notes);
        g
    }

    /// Insert a node; the FIRST definition wins on duplicate names (lxml-recover tolerance) + a note.
    fn insert(
        &mut self,
        name: &str,
        kind: &'static str,
        pose: Option<Pose>,
        relative_to: String,
        notes: &mut Vec<String>,
    ) {
        use std::collections::btree_map::Entry;
        match self.nodes.entry(name.to_string()) {
            Entry::Vacant(v) => {
                v.insert(FrameNode {
                    kind,
                    pose,
                    relative_to,
                    resolved: None,
                    flattened: false,
                });
            }
            Entry::Occupied(o) => notes.push(format!(
                "{kind} {}: duplicate frame name (a {} already owns it); the first definition wins for pose resolution",
                repr_str(name),
                o.get().kind
            )),
        }
    }

    /// Resolve every node to X(model←node) by iterative fixpoint: a node resolves once its
    /// `relative_to` base has (O(n²) worst case over ≤ dozens of nodes). Leftovers reference an unknown
    /// frame or form a cycle: invalid SDF, handled tolerantly PER NODE as model-relative (the importer's
    /// lxml-recover ethos, i.e. the pre-frame-graph flattening), with a note.
    fn resolve(&mut self, notes: &mut Vec<String>) {
        loop {
            let mut progress = false;
            let pending: Vec<String> = self
                .nodes
                .iter()
                .filter(|(_, n)| n.resolved.is_none())
                .map(|(k, _)| k.clone())
                .collect();
            for name in pending {
                let rel = self.nodes[&name].relative_to.clone();
                let Some(base) = self.nodes.get(&rel).and_then(|b| b.resolved.clone()) else {
                    continue;
                };
                let node = self.nodes.get_mut(&name).expect("pending name exists");
                node.resolved = Some(base.compose(&node.local()));
                progress = true;
            }
            if !progress {
                break;
            }
        }
        let leftover: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.resolved.is_none())
            .map(|(k, _)| k.clone())
            .collect();
        let mut cyclic: Vec<String> = Vec::new();
        for name in &leftover {
            let node = &self.nodes[name];
            if self.nodes.contains_key(&node.relative_to) {
                cyclic.push(repr_str(name));
            } else {
                notes.push(format!(
                    "{} {}: <pose relative_to={}> references an unknown frame; pose taken as model-relative",
                    node.kind,
                    repr_str(name),
                    repr_str(&node.relative_to)
                ));
            }
        }
        if !cyclic.is_empty() {
            notes.push(format!(
                "<pose relative_to> cycle involving {}; poses taken as model-relative",
                cyclic.join(", ")
            ));
        }
        for name in leftover {
            let node = self.nodes.get_mut(&name).expect("leftover name exists");
            node.flattened = true;
            node.resolved = Some(node.local());
        }
    }

    /// X(model←name), when the frame exists.
    fn xform(&self, name: &str) -> Option<&XForm> {
        self.nodes.get(name).and_then(|n| n.resolved.as_ref())
    }

    /// X(from←to). Fast path: when `to`'s `relative_to` chain reaches `from` through normally-resolved
    /// nodes, the chain's local poses compose DIRECTLY (no inverse round-trip), so the authored pose of
    /// a `to_sdf`-style `<pose relative_to="parent">` joint survives byte-verbatim, even for rotations
    /// adjacent to gimbal lock. Otherwise the general path via the model frame: inv(X(M←from))∘X(M←to).
    fn relative(&self, from: &str, to: &str) -> Option<XForm> {
        if let Some(x) = self.walk_up(from, to) {
            return Some(x);
        }
        Some(self.xform(from)?.inverse().compose(self.xform(to)?))
    }

    /// The chain fast path of [`FrameGraph::relative`]: X(from←to) = local(nₘ₋₁)∘…∘local(to) when
    /// following `relative_to` links up from `to` hits `from` (depth-capped; never through a
    /// fallback-flattened node, whose local pose is no longer X(relative_to←node)).
    fn walk_up(&self, from: &str, to: &str) -> Option<XForm> {
        let mut chain: Vec<&FrameNode> = Vec::new();
        let mut cur = to;
        for _ in 0..=self.nodes.len() {
            if cur == from {
                let mut x = XForm::identity();
                for node in chain.iter().rev() {
                    x = x.compose(&node.local());
                }
                return Some(x);
            }
            let node = self.nodes.get(cur)?;
            if node.flattened {
                return None;
            }
            chain.push(node);
            cur = node.relative_to.as_str();
        }
        None
    }

    /// The link a frame is ultimately attached to (following `<frame>` attach + joint→child chains;
    /// `__model__` → the canonical link). Depth-capped against `attached_to` cycles.
    fn owner_link(&self, frame: &str) -> Option<String> {
        let mut cur = frame;
        for _ in 0..=self.nodes.len() {
            if cur == "__model__" {
                return self.canonical.clone();
            }
            match self.nodes.get(cur)?.kind {
                "link" => return Some(cur.to_string()),
                "joint" => cur = self.joint_child.get(cur)?.as_str(),
                _ => cur = self.frame_attach.get(cur)?.as_str(),
            }
        }
        None
    }
}

/// The `<pose>` of a frame-graph element: the parsed six-tuple (malformed -> note + `None` = identity)
/// and the frame it is expressed in (`@relative_to` when non-empty, else the element's spec default).
fn node_pose(
    el: &SdfEl,
    default_rel: &str,
    ctx: &str,
    notes: &mut Vec<String>,
) -> (Option<Pose>, String) {
    let Some(pel) = el.find("pose") else {
        return (None, default_rel.to_string());
    };
    if pel.get("rotation_format").is_some() || pel.get("degrees").is_some() {
        notes.push(format!(
            "{ctx}: <pose> rotation_format/degrees not represented; rpy read as radians"
        ));
    }
    let rel = pel
        .get("relative_to")
        .filter(|s| !s.is_empty())
        .unwrap_or(default_rel);
    (pose_values(pel, notes, ctx), rel.to_string())
}

// ── geometry ──────────────────────────────────────────────────────────────────────────────────────

/// The matched primitive of an SDF `<geometry>`, plus an optional mesh-uri tail. `allow_mesh` controls
/// whether a `<mesh>` maps into the geometry (collision) or is returned as a uri for the caller to lift
/// into a `<model>` (visual). Mirrors `from_sdf.py::_geometry`.
enum GeoOut<G> {
    /// A primitive (or collision mesh) geometry.
    Geo(G),
    /// A visual mesh: no in-geometry mapping, the uri is lifted to a `<model>` by the caller.
    MeshUri(Option<String>),
    /// No HCDF-representable shape.
    None,
}

/// Common primitive filling shared by visual/collision geometry (all six closed shapes).
fn fill_primitive<G: GeometrySink>(gel: &SdfEl) -> Option<G> {
    if let Some(b) = gel.find("box") {
        let mut g = G::default();
        g.set_box(Box_ {
            size: b.child_txt("size").map(str::to_string),
        });
        return Some(g);
    }
    if let Some(c) = gel.find("cylinder") {
        let mut g = G::default();
        g.set_cylinder(Cylinder {
            radius: c.child_txt("radius").map(str::to_string),
            length: c.child_txt("length").map(str::to_string),
        });
        return Some(g);
    }
    if let Some(s) = gel.find("sphere") {
        let mut g = G::default();
        g.set_sphere(Sphere {
            radius: s.child_txt("radius").map(str::to_string),
        });
        return Some(g);
    }
    if let Some(c) = gel.find("capsule") {
        let mut g = G::default();
        g.set_capsule(Capsule {
            radius: c.child_txt("radius").map(str::to_string),
            length: c.child_txt("length").map(str::to_string),
        });
        return Some(g);
    }
    if let Some(c) = gel.find("cone") {
        let mut g = G::default();
        g.set_cone(Cone {
            radius: c.child_txt("radius").map(str::to_string),
            length: c.child_txt("length").map(str::to_string),
        });
        return Some(g);
    }
    if let Some(e) = gel.find("ellipsoid") {
        let mut g = G::default();
        g.set_ellipsoid(Ellipsoid {
            radii: e.child_txt("radii").map(str::to_string),
        });
        return Some(g);
    }
    None
}

/// A geometry container we can fill with the six closed primitives generically.
trait GeometrySink: Default {
    fn set_box(&mut self, b: Box_);
    fn set_cylinder(&mut self, c: Cylinder);
    fn set_sphere(&mut self, s: Sphere);
    fn set_capsule(&mut self, c: Capsule);
    fn set_cone(&mut self, c: Cone);
    fn set_ellipsoid(&mut self, e: Ellipsoid);
}

impl GeometrySink for VisualGeometry {
    fn set_box(&mut self, b: Box_) {
        self.box_ = Some(b);
    }
    fn set_cylinder(&mut self, c: Cylinder) {
        self.cylinder = Some(c);
    }
    fn set_sphere(&mut self, s: Sphere) {
        self.sphere = Some(s);
    }
    fn set_capsule(&mut self, c: Capsule) {
        self.capsule = Some(c);
    }
    fn set_cone(&mut self, c: Cone) {
        self.cone = Some(c);
    }
    fn set_ellipsoid(&mut self, e: Ellipsoid) {
        self.ellipsoid = Some(e);
    }
}

impl GeometrySink for CollisionGeometry {
    fn set_box(&mut self, b: Box_) {
        self.box_ = Some(b);
    }
    fn set_cylinder(&mut self, c: Cylinder) {
        self.cylinder = Some(c);
    }
    fn set_sphere(&mut self, s: Sphere) {
        self.sphere = Some(s);
    }
    fn set_capsule(&mut self, c: Capsule) {
        self.capsule = Some(c);
    }
    fn set_cone(&mut self, c: Cone) {
        self.cone = Some(c);
    }
    fn set_ellipsoid(&mut self, e: Ellipsoid) {
        self.ellipsoid = Some(e);
    }
}

/// Map a collision `<geometry>` (allow_mesh = true): primitives or a `<mesh uri scale>`.
/// Read an SDF `<mesh><submesh>` (`<name>` + optional `<center>` bool child elements) into the typed
/// HCDF [`Submesh`] (name/center attributes). Returns `None` when the mesh selects no sub-part (whole
/// file) or the submesh carries no `<name>`.
fn mesh_submesh(mesh: &SdfEl) -> Option<Submesh> {
    let sm = mesh.find("submesh")?;
    let name = sm.child_txt("name")?;
    Some(Submesh {
        name: Some(name.to_string()),
        center: sm.child_txt("center").map(str::to_string),
    })
}

fn collision_geometry(
    gel: Option<&SdfEl>,
    notes: &mut Vec<String>,
    ctx: &str,
) -> Option<CollisionGeometry> {
    let gel = gel?;
    if let Some(prim) = fill_primitive::<CollisionGeometry>(gel) {
        return Some(prim);
    }
    if let Some(mesh) = gel.find("mesh") {
        let mut m = Mesh {
            uri: mesh.child_txt("uri").map(str::to_string),
            ..Default::default()
        };
        let scale = mesh.child_txt("scale");
        if let Some(scale) = scale {
            if scale != "1 1 1" {
                m.scale = Some(scale.to_string());
            }
        }
        m.submesh = mesh_submesh(mesh);
        return Some(CollisionGeometry {
            mesh: Some(m),
            ..Default::default()
        });
    }
    note_unmapped_geometry(gel, notes, ctx);
    None
}

/// Map a visual `<geometry>` (allow_mesh = false): primitives, or a mesh uri lifted to the caller
/// (which becomes a `<model>`). Returns a `GeoOut`. `albedo` is the visual's `<pbr><albedo_map>` uri
/// (if any): it unlocks the ONE extra mapping below: a TEXTURED `<plane>` becomes a zero-thickness
/// `<box>` (see [`textured_plane_box`]); an untextured plane keeps its no-HCDF-mapping drop.
fn visual_geometry(
    gel: Option<&SdfEl>,
    notes: &mut Vec<String>,
    ctx: &str,
    albedo: Option<&str>,
) -> GeoOut<VisualGeometry> {
    let Some(gel) = gel else {
        return GeoOut::None;
    };
    if let Some(prim) = fill_primitive::<VisualGeometry>(gel) {
        return GeoOut::Geo(prim);
    }
    if let Some(mesh) = gel.find("mesh") {
        // a visual mesh -> ARM A model, handled by caller (uri lifted to <model uri>).
        return GeoOut::MeshUri(mesh.child_txt("uri").map(str::to_string));
    }
    if albedo.is_some() {
        if let Some(vg) = gel
            .find("plane")
            .and_then(|p| textured_plane_box(p, notes, ctx))
        {
            return GeoOut::Geo(vg);
        }
    }
    note_unmapped_geometry(gel, notes, ctx);
    GeoOut::None
}

/// The zero-thickness `<box>` a TEXTURED SDF `<plane>` maps to. HCDF has no plane primitive, so a plane
/// visual normally drops (noted), but when its material carries a `<pbr><albedo_map>` the plane maps to
/// `<box size="sx sy 0">` so the texture has a geometry to bake onto: the asset step synthesizes a
/// UV-mapped quad GLB from the box + the texture hint (the b3rb logo-decal pattern), and a NO-bake
/// conversion keeps the box + these notes. `None` (keeping the plain drop) when `<size>` is missing or
/// not two numbers. A non-`+Z` `<normal>` is noted; the synthesized quad always faces `+Z`, which is
/// both the SDF default and what decal planes author (the visual `<pose>` carries the orientation).
fn textured_plane_box(p: &SdfEl, notes: &mut Vec<String>, ctx: &str) -> Option<VisualGeometry> {
    let size = p.child_txt("size")?;
    let mut it = size.split_whitespace();
    let (sx, sy) = (it.next()?, it.next()?);
    if it.next().is_some() || sx.parse::<f64>().is_err() || sy.parse::<f64>().is_err() {
        return None;
    }
    if let Some(n) = p.child_txt("normal") {
        let non_plus_z =
            parse3(n).is_none_or(|v| v[0].abs() > 1e-9 || v[1].abs() > 1e-9 || v[2] <= 0.0);
        if non_plus_z {
            notes.push(format!(
                "{ctx}: <plane> normal {} not represented; the textured quad bakes facing +Z",
                repr_str(n)
            ));
        }
    }
    notes.push(format!(
        "{ctx}: textured <plane> mapped to a zero-thickness <box> ({sx} {sy} 0); the albedo map bakes it to a quad GLB model at the asset step"
    ));
    Some(VisualGeometry {
        box_: Some(Box_ {
            size: Some(format!("{sx} {sy} 0")),
        }),
        ..Default::default()
    })
}

/// The `<material><pbr><metal|specular><albedo_map>` texture uri of a visual, if any (the metal
/// workflow first, the common gz authoring, then specular). The uri is captured VERBATIM
/// (`model://…` included): like a mesh uri it is a bake-time reference, resolved by the baker through
/// the same package-root machinery, never by this importer.
fn pbr_albedo_map(mat: Option<&SdfEl>) -> Option<&str> {
    let pbr = mat?.find("pbr")?;
    pbr.find("metal")
        .and_then(|w| w.child_txt("albedo_map"))
        .or_else(|| pbr.find("specular").and_then(|w| w.child_txt("albedo_map")))
}

fn note_unmapped_geometry(gel: &SdfEl, notes: &mut Vec<String>, ctx: &str) {
    let other = gel
        .children
        .first()
        .map(|c| c.tag.as_str())
        .unwrap_or("empty");
    notes.push(format!(
        "{ctx}: <geometry><{other}> has no HCDF mapping; dropped"
    ));
}

// ── surface / inertial ────────────────────────────────────────────────────────────────────────────

/// SDF `<surface>` -> HCDF `<surface>` (friction + restitution + contact). ODE extras -> note.
/// Mirrors `from_sdf.py::_surface`. Returns `None` if nothing mapped.
fn surface(sel: Option<&SdfEl>, notes: &mut Vec<String>, ctx: &str) -> Option<Surface> {
    let sel = sel?;
    let mut surf = Surface::default();
    if let Some(fr) = sel.find("friction") {
        let ode = fr.find("ode");
        // The Bullet friction block is an alternate engine home for the same coefficients; read it
        // as a fallback so a Bullet-authored SDF no longer loses all friction. bullet/friction ->
        // @static (like ode/mu), bullet/friction2 -> @dynamic (like ode/mu2), bullet/fdir1 -> the
        // anisotropy axis. ODE is preferred when both are present.
        let bullet = fr.find("bullet");
        let mu = ode
            .and_then(|o| o.child_txt("mu"))
            .or_else(|| fr.child_txt("mu"))
            .or_else(|| bullet.and_then(|b| b.child_txt("friction")));
        let mu2 = ode
            .and_then(|o| o.child_txt("mu2"))
            .or_else(|| fr.child_txt("mu2"))
            .or_else(|| bullet.and_then(|b| b.child_txt("friction2")));
        // The SDF/Bullet second-friction-direction coefficient (mu2/friction2) is NOT
        // kinetic friction; it is the coefficient ALONG the anisotropic <friction-direction>, so it is
        // captured there (friction-direction/@mu2), never relabeled to @dynamic. fdir1 + per-direction
        // slip compliance (slip1/slip2) come from the same ODE/Bullet friction block.
        let fdir1 = ode
            .and_then(|o| o.child_txt("fdir1"))
            .or_else(|| bullet.and_then(|b| b.child_txt("fdir1")));
        let slip1 = ode.and_then(|o| o.child_txt("slip1"));
        let slip2 = ode.and_then(|o| o.child_txt("slip2"));
        // bullet/rolling_friction (torsional rolling resistance) has no HCDF surface home; note it
        // rather than drop it silently.
        if let Some(rf) = bullet.and_then(|b| b.child_txt("rolling_friction")) {
            notes.push(format!(
                "{ctx}: <surface><friction><bullet><rolling_friction>{} has no HCDF home (no rolling-resistance field); dropped",
                repr_str(rf)
            ));
        }
        let friction_direction =
            (mu2.is_some() || fdir1.is_some() || slip1.is_some() || slip2.is_some()).then(|| {
                FrictionDirection {
                    fdir1: fdir1.map(str::to_string),
                    mu2: mu2.map(str::to_string),
                    slip1: slip1.map(str::to_string),
                    slip2: slip2.map(str::to_string),
                }
            });
        if mu.is_some() || friction_direction.is_some() {
            surf.friction = Some(Friction {
                static_: mu.map(str::to_string),
                // SDF <ode> has no kinetic-friction leaf, so @dynamic stays unset on the SDF
                // import path (mu2 is now carried by friction-direction/@mu2, above).
                dynamic: None,
                friction_direction,
            });
        }
    }
    if let Some(bounce) = sel.find("bounce") {
        if let Some(rc) = bounce.child_txt("restitution_coefficient") {
            surf.restitution = Some(rc.to_string());
        }
    }
    if let Some(cn) = sel.find("contact") {
        let ode = cn.find("ode");
        // Read the Bullet contact block as a fallback (bullet/kp -> @stiffness, bullet/kd ->
        // @damping) so a Bullet-authored SDF no longer loses contact stiffness/damping. ODE preferred.
        let bullet = cn.find("bullet");
        let kp = ode
            .and_then(|o| o.child_txt("kp"))
            .or_else(|| bullet.and_then(|b| b.child_txt("kp")));
        let kd = ode
            .and_then(|o| o.child_txt("kd"))
            .or_else(|| bullet.and_then(|b| b.child_txt("kd")));
        if kp.is_some() || kd.is_some() {
            surf.contact = Some(Contact {
                stiffness: kp.map(str::to_string),
                damping: kd.map(str::to_string),
            });
        }
        // The Bullet solver knobs (soft_cfm/soft_erp) have no HCDF contact home; note, never drop
        // silently.
        for knob in ["soft_cfm", "soft_erp"] {
            if let Some(v) = bullet.and_then(|b| b.child_txt(knob)) {
                notes.push(format!(
                    "{ctx}: <surface><contact><bullet><{knob}>{} has no HCDF home (sim solver knob); dropped",
                    repr_str(v)
                ));
            }
        }
    }
    notes.push(format!(
        "{ctx}: <surface> mapped (friction/restitution/contact); ODE-specific params not represented are dropped"
    ));
    if surf.friction.is_some() || surf.restitution.is_some() || surf.contact.is_some() {
        Some(surf)
    } else {
        None
    }
}

/// SDF `<inertial>` -> HCDF inertial (mass + inertia tensor with SDF defaults + cog pose).
/// Mirrors `from_sdf.py::_inertial`.
fn inertial(iel: &SdfEl, notes: &mut Vec<String>, ctx: &str) -> Inertial {
    let mut ip = Inertial {
        mass: iel.child_txt("mass").map(str::to_string),
        inertia_origin: pose(iel.find("pose"), notes, &format!("{ctx} inertial")),
        ..Default::default()
    };
    if let Some(inr) = iel.find("inertia") {
        // SDF inertia: ixx/iyy/izz default 1.0, off-diagonals default 0.0; a partial <inertia> is valid.
        const KEYS: [(&str, &str); 6] = [
            ("ixx", "1.0"),
            ("ixy", "0.0"),
            ("ixz", "0.0"),
            ("iyy", "1.0"),
            ("iyz", "0.0"),
            ("izz", "1.0"),
        ];
        let parts: Vec<String> = KEYS
            .iter()
            .map(|(k, default)| inr.child_txt(k).unwrap_or(default).to_string())
            .collect();
        ip.inertia = Some(parts.join(" "));
    }
    if iel.find("auto").is_some() || iel.get("auto").is_some() {
        notes.push(format!(
            "{ctx}: <inertial auto> (auto-computed inertia) not represented"
        ));
    }
    ip
}

// ── links ─────────────────────────────────────────────────────────────────────────────────────────

/// `<link>` -> [`Comp`]. The link's own `<pose>` is NOT read here: it lives in the [`FrameGraph`] and
/// is represented through the origins of the joints that place the comp (a posed ROOT link is the one
/// unrepresentable case, noted by the caller).
fn link(lel: &SdfEl, notes: &mut Vec<String>, hints: &mut Vec<VisualAssetHint>) -> Comp {
    let name = lel.get("name").unwrap_or("").to_string();
    let ctx = format!("link {}", repr_str(&name));
    let mut comp = Comp {
        name: name.clone(),
        ..Default::default()
    };
    if let Some(iel) = lel.find("inertial") {
        comp.inertial = Some(inertial(iel, notes, &ctx));
    }
    for cel in lel.find_all("collision") {
        let authored = cel.get("name").map(str::to_string);
        // Synthesized name mirrors `from_sdf.py` EXACTLY: `{comp.name}_collision` with NO index
        // (two unnamed collisions on link 'l' both become 'l_collision', matching the authority; the
        // HCDF document is then identical, name references and round-trip preserved).
        let cname = authored
            .clone()
            .unwrap_or_else(|| format!("{name}_collision"));
        let cctx = format!("{ctx} collision {}", repr_str(&cname));
        let geo = collision_geometry(cel.find("geometry"), notes, &cctx);
        if let Some(geo) = geo {
            comp.collision.push(Collision {
                name: Some(cname),
                verbose: None,
                name_origin: if authored.is_none() {
                    Some(NameOrigin::Synthesized)
                } else {
                    None
                },
                pose: pose(cel.find("pose"), notes, &cctx),
                geometry: Some(geo),
                surface: surface(cel.find("surface"), notes, &cctx),
            });
        }
    }
    for vel in lel.find_all("visual") {
        let authored = vel.get("name").map(str::to_string);
        // Synthesized name mirrors `from_sdf.py` EXACTLY: `{comp.name}_visual` with NO index.
        let vname = authored.clone().unwrap_or_else(|| format!("{name}_visual"));
        let vctx = format!("{ctx} visual {}", repr_str(&vname));
        let vpose = pose(vel.find("pose"), notes, &vctx);
        let mat = vel.find("material");
        // The `<pbr><albedo_map>` texture uri (metal or specular workflow). CARRIED; it rides the
        // asset side-channel below on BOTH visual arms (mesh: embeds via the mesh's own UVs; primitive:
        // synthesizes a UV-mapped GLB) instead of the old blanket not-represented drop.
        let albedo = pbr_albedo_map(mat);
        let appearance = match visual_geometry(vel.find("geometry"), notes, &vctx, albedo) {
            GeoOut::MeshUri(uri) => {
                // a visual mesh carries its appearance in the GLB model (bake/@sha deferred to assets).
                notes.push(format!(
                    "{vctx}: mesh -> <model uri> (GLB bake/@sha deferred to assets)"
                ));
                let scale = vel.find_path("geometry/mesh/scale").and_then(|s| s.txt());
                if let Some(scale) = scale {
                    if scale != "1 1 1" {
                        notes.push(format!(
                            "{vctx}: mesh <scale> {} not applied (bake the GLB with the meshes present to fold it in)",
                            repr_str(scale)
                        ));
                    }
                }
                // The note distinguishes what is CARRIED (a flat <diffuse> and/or a <pbr> albedo map,
                // both ride the hint and bake into the GLB) from a DROPPED material (neither present:
                // ambient/specular-only, a script, …), mirroring the URDF importer's wording split.
                let diffuse = mat.and_then(|m| m.child_txt("diffuse"));
                if mat.is_some() {
                    if let Some(rgba) = diffuse {
                        notes.push(format!(
                            "{vctx}: <material> diffuse {} carried on the asset side-channel (bakes into the GLB at the asset step, not into the HCDF document)",
                            repr_str(rgba)
                        ));
                    }
                    if let Some(tex) = albedo {
                        notes.push(format!(
                            "{vctx}: <material> albedo map {} carried on the asset side-channel (embeds into the GLB at the asset step, not into the HCDF document)",
                            repr_str(tex)
                        ));
                    }
                    if diffuse.is_none() && albedo.is_none() {
                        notes.push(format!(
                            "{vctx}: <material> carries no flat <diffuse> or <pbr> albedo map; dropped (the GLB keeps the mesh's own appearance)"
                        ));
                    }
                }
                // SIDE-CHANNEL: emit ONE hint per mesh visual carrying the raw `<mesh><scale>` text, the
                // flat `<material><diffuse>` colour, exactly the pair `from_sdf.py` hands its import-time
                // `baker(mesh_uri, scale, "visual", color)` hook, plus the `<pbr><albedo_map>` uri, so a
                // baker can fold the magnitude + mirror scale AND the colour/texture into the GLB. The HCDF
                // model stays clean: a visual <model> is still just a GLB uri+sha (no scale/colour/texture
                // fields), so this is purely additive.
                hints.push(VisualAssetHint {
                    comp: name.clone(),
                    visual: vname.clone(),
                    scale: scale.map(str::to_string),
                    color: diffuse.map(str::to_string),
                    texture: albedo.map(str::to_string),
                });
                Some(VisualAppearance::Model {
                    model: ModelRef {
                        uri,
                        sha: None,
                        ..Default::default()
                    },
                    geometry: None,
                })
            }
            GeoOut::Geo(vg) => {
                let mut color = None;
                if let Some(mat) = mat {
                    if let Some(diffuse) = mat.child_txt("diffuse") {
                        color = Some(Color {
                            rgba: Some(diffuse.to_string()),
                            ..Default::default()
                        });
                    }
                    if let Some(tex) = albedo {
                        // SIDE-CHANNEL: a textured PRIMITIVE visual gets a hint too (texture only; the
                        // flat diffuse is already represented in the typed <color>, and a primitive has
                        // no mesh scale). At the asset step the primitive synthesizes a UV-mapped GLB
                        // with the texture embedded and the visual becomes an ordinary <model uri>; a
                        // no-bake conversion keeps the primitive + this note.
                        hints.push(VisualAssetHint {
                            comp: name.clone(),
                            visual: vname.clone(),
                            scale: None,
                            color: None,
                            texture: Some(tex.to_string()),
                        });
                        notes.push(format!(
                            "{vctx}: <material><pbr> albedo map {} carried on the asset side-channel (a textured primitive bakes to a GLB model at the asset step)",
                            repr_str(tex)
                        ));
                    }
                    // Rich-appearance drop note. With NO carried albedo the wording (and firing
                    // condition) is byte-identical to the pre-texture importer; with the albedo carried,
                    // only what REMAINS unrepresented is noted.
                    let pbr_other = mat.find("pbr").is_some_and(|p| {
                        p.children
                            .iter()
                            .any(|w| w.children.iter().any(|c| c.tag != "albedo_map"))
                    });
                    if albedo.is_none() {
                        if mat.find("pbr").is_some()
                            || mat.find("ambient").is_some()
                            || mat.find("specular").is_some()
                        {
                            notes.push(format!(
                                "{vctx}: <material> ambient/specular/PBR not represented (rich appearance is baked into the GLB model)"
                            ));
                        }
                    } else if pbr_other
                        || mat.find("ambient").is_some()
                        || mat.find("specular").is_some()
                    {
                        notes.push(format!(
                            "{vctx}: <material> ambient/specular/PBR beyond the carried albedo map not represented (rich appearance is baked into the GLB model)"
                        ));
                    }
                }
                Some(VisualAppearance::Primitive {
                    geometry: Some(vg),
                    color,
                })
            }
            GeoOut::None => None,
        };
        if let Some(appearance) = appearance {
            comp.visual.push(Visual {
                name: vname,
                toggle: None,
                name_origin: if authored.is_none() {
                    Some(NameOrigin::Synthesized)
                } else {
                    None
                },
                pose: vpose,
                appearance,
            });
        }
    }
    // SDF 1.7+ `<link><battery name><voltage>`: the open-circuit init voltage is the one battery leaf
    // with a clean HCDF home, so it maps to a typed `<power-source><battery><nominal-voltage>`.
    // The pack model (capacity/resistance/chemistry) lives in the gz
    // `LinearBatteryPlugin` `<plugin>`, which has no typed home; those battery_source leaves stay
    // native-authored and are not synthesized here.
    for bel in lel.find_all("battery") {
        comp.power_source
            .push(battery_power_source(bel, &name, notes, &ctx));
    }
    // Typed sensor re-root: each SDF `<link><sensor>` becomes a typed `comp.sensor[*].<category>`
    // (the mapping spine `link->comp` already accepts unbounded `<sensor>`). Sim-only sub-fields
    // routed to Gazebo (camera `<clip>`, IMU dynamic-bias) are collected here and emitted
    // as ONE comp-level `<extension domain="org.gazebosim.raw">`, the OPAQUE passthrough domain: these
    // `<sensor>`-wrapped fragments are not a typed `<gazebo-sim>` document, so they belong in the raw
    // domain, not the typed `org.gazebosim`.
    let mut frags: Vec<String> = Vec::new();
    for sel in lel.find_all("sensor") {
        if let Some(s) = map_sensor(sel, notes, &ctx, &mut frags) {
            comp.sensor.push(s);
        }
    }
    if !frags.is_empty() {
        comp.extension.push(Extension {
            domain: GAZEBO_RAW_DOMAIN.to_string(),
            body: frags.concat(),
            ..Default::default()
        });
    }
    // A link-scope <light> (headlight / work light / IR illuminator) is a scene-illumination HMI;
    // its typed home is a comp.hmi element of type led-illumination. Previously every <light> was
    // dropped (model-scope noted, link-scope silently).
    for light in lel.find_all("light") {
        comp.hmi.push(sdf_light_hmi(light, notes, &ctx));
    }
    comp
}

/// Map a link-scope SDF `<light>` to a typed `led-illumination` [`HmiElement`] (its emitted-light home).
/// The light's `<pose>` becomes the HMI pose; type/colour/attenuation/direction/intensity/cast_shadows
/// land in `<illumination>`. Spot-cone params and the on/off/visualize flags have no HCDF field and are
/// noted, never silently dropped.
fn sdf_light_hmi(light: &SdfEl, notes: &mut Vec<String>, ctx: &str) -> HmiElement {
    let name = light.get("name").unwrap_or("light").to_string();
    let lctx = format!("{ctx} light {}", repr_str(&name));
    let attenuation = light.find("attenuation").map(|a| LightAttenuation {
        range: a.child_txt("range").map(str::to_string),
        linear: a.child_txt("linear").map(str::to_string),
        constant: a.child_txt("constant").map(str::to_string),
        quadratic: a.child_txt("quadratic").map(str::to_string),
    });
    for extra in ["spot", "light_on", "visualize"] {
        if light.find(extra).is_some() {
            notes.push(format!(
                "{lctx}: <{extra}> not represented (no led-illumination field); dropped"
            ));
        }
    }
    let illumination = LedIllumination {
        light_type: light.get("type").map(str::to_string),
        cast_shadows: light.child_txt("cast_shadows").map(str::to_string),
        intensity: light.child_txt("intensity").map(str::to_string),
        diffuse: light.child_txt("diffuse").map(str::to_string),
        specular: light.child_txt("specular").map(str::to_string),
        direction: light.child_txt("direction").map(str::to_string),
        attenuation,
    };
    HmiElement {
        name: Some(name),
        type_: Some(HmiType::LedIllumination),
        pose: pose(light.find("pose"), notes, &lctx),
        illumination: Some(illumination),
        ..Default::default()
    }
}

/// Map an SDF `<battery name><voltage>` to a typed `<power-source><battery>` carrying only the nominal
/// voltage, the sole battery leaf with a clean SDF source (unit stamped `"V"`, SDFormat's implicit
/// battery unit). A nameless `<battery>` gets a synthesized `{link}_battery` power-source name (the
/// schema requires `<power-source>@name`). Every `<battery>` child other than `<voltage>` is noted,
/// never silently dropped.
fn battery_power_source(
    bel: &SdfEl,
    link: &str,
    notes: &mut Vec<String>,
    ctx: &str,
) -> PowerSource {
    let name = bel
        .get("name")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{link}_battery"));
    let voltage = bel.child_txt("voltage").map(|v| MeasuredValue {
        unit: Some("V".to_string()),
        value: Some(v.to_string()),
    });
    let extras: Vec<&str> = bel
        .children
        .iter()
        .map(|c| c.tag.as_str())
        .filter(|t| *t != "voltage")
        .collect();
    if !extras.is_empty() {
        notes.push(format!(
            "{ctx} battery {}: sub-element(s) {} not represented (only <voltage> maps to nominal-voltage; the pack model lives in the LinearBatteryPlugin); dropped",
            repr_str(&name),
            extras.join(", ")
        ));
    }
    if voltage.is_some() {
        notes.push(format!(
            "{ctx} battery {}: <voltage> mapped to <power-source><battery><nominal-voltage>",
            repr_str(&name)
        ));
    } else {
        notes.push(format!(
            "{ctx} battery {}: no <voltage>; imported as a named <power-source> with no nominal-voltage",
            repr_str(&name)
        ));
    }
    PowerSource {
        name: Some(name),
        battery: Some(BatterySource {
            nominal_voltage: voltage,
            ..Default::default()
        }),
        ..Default::default()
    }
}

// ── sensors (the typed re-root) ─────────────────────────────────────────────────────────────────────

/// Per-sensor context bundled to keep the per-type mappers within the argument budget: the note-context
/// prefix, the sensor `@name`/`@type` (for the quarantine envelope), and the accumulator for the
/// comp-level `org.gazebosim.raw` extension fragments.
struct SensorCtx<'a> {
    ctx: &'a str,
    name: &'a str,
    stype: &'a str,
    frags: &'a mut Vec<String>,
}

impl SensorCtx<'_> {
    /// Wrap a raw XML `body` (a verbatim source sub-element) in a `<sensor name= type=>` envelope and
    /// stash it for the comp-level `org.gazebosim.raw` extension so it round-trips verbatim.
    fn quarantine(&mut self, body: &str) {
        self.frags.push(format!(
            "<sensor name={} type={}>{body}</sensor>",
            xml_attr(self.name),
            xml_attr(self.stype)
        ));
    }
}

/// Map one SDF/Gazebo `<sensor>` to a typed HCDF [`Sensor`]. The dispatch is keyed purely on `@type` and
/// the `SdfEl` read surface, so the URDF/Gazebo front end (whose `<gazebo reference><sensor>` shares the
/// SDFormat grammar) can feed this SAME mapper by handing it the equivalent element. `@name`/`<update_rate>`
/// -> the sensor group; `<pose>` -> the typed child (element-local, like visual/collision); every
/// runtime/sim-only or unmapped field is noted, never silently dropped (an explicit non-goal).
fn map_sensor(
    sel: &SdfEl,
    notes: &mut Vec<String>,
    lctx: &str,
    frags: &mut Vec<String>,
) -> Option<Sensor> {
    let sname = sel.get("name").unwrap_or("").to_string();
    let stype = sel.get("type").unwrap_or("").to_string();
    let ctx = format!("{lctx} sensor {}", repr_str(&sname));
    let pose = pose(sel.find("pose"), notes, &ctx);
    note_runtime_fields(sel, notes, &ctx);
    let mut sensor = Sensor {
        name: (!sname.is_empty()).then(|| sname.clone()),
        update_rate: sel.child_txt("update_rate").map(str::to_string),
        ..Default::default()
    };
    let mut sc = SensorCtx {
        ctx: &ctx,
        name: &sname,
        stype: &stype,
        frags,
    };
    match stype.as_str() {
        "imu" => sensor.inertial.push(imu_sensor(sel, pose, notes, &mut sc)),
        "camera" => {
            sensor.optical.push(camera_optical(
                sel,
                pose,
                OpticalSensorType::Camera,
                notes,
                &mut sc,
            ));
        }
        "depth_camera" | "depth" => {
            sensor.optical.push(camera_optical(
                sel,
                pose,
                OpticalSensorType::Tof,
                notes,
                &mut sc,
            ));
        }
        "rgbd_camera" | "rgbd" => {
            notes.push(format!(
                "{ctx}: rgbd depth channel folded into the camera <camera-params> depth-range; a separate depth FoV is not synthesized"
            ));
            sensor.optical.push(camera_optical(
                sel,
                pose,
                OpticalSensorType::Camera,
                notes,
                &mut sc,
            ));
        }
        "thermal" => {
            sensor.optical.push(camera_optical(
                sel,
                pose,
                OpticalSensorType::Thermal,
                notes,
                &mut sc,
            ));
        }
        "lidar" | "gpu_lidar" | "ray" | "gpu_ray" => {
            sensor.optical.push(lidar_optical(sel, pose, notes, &ctx));
        }
        "magnetometer" => sensor.em.push(mag_em(sel, pose, notes, &ctx)),
        "gps" | "navsat" => sensor.rf.push(gnss_rf(sel, pose, notes, &ctx)),
        "air_pressure" | "altimeter" => {
            sensor
                .fluid
                .push(barometer_fluid(sel, pose, notes, &ctx, &stype));
        }
        "contact" => sensor.force.push(contact_force(sel, pose, notes, &ctx)),
        "force_torque" => sensor
            .force
            .push(force_torque_force(sel, pose, notes, &ctx)),
        other => {
            notes.push(format!(
                "{ctx}: <sensor type={}> has no typed HCDF mapping; dropped",
                repr_str(other)
            ));
            return None;
        }
    }
    Some(sensor)
}

// ── URDF/Gazebo sensor front end ────────────────────────────────────────────────────────────────────

/// What [`gazebo_block_extract`] pulls out of one quarantined top-level `<gazebo>` block: typed sensors
/// AND the Gazebo-classic friction/contact idiom (`<mu1>/<mu2>/<kp>/<kd>`).
pub(crate) struct GazeboExtract {
    /// The block's `@reference`, the link/comp name the sensors + friction attach to; `None` for a
    /// model-scoped block (no `reference`, so the caller cannot place them and keeps the block verbatim).
    pub reference: Option<String>,
    /// The typed sensors decomposed from the block's `<sensor>` children.
    pub sensors: Vec<Sensor>,
    /// The typed collision `<surface>` parsed from the block's `<mu1>/<mu2>/<kp>/<kd>` friction/contact
    /// idiom (Gazebo-classic universal friction), or `None` if the block carries none. The caller attaches
    /// it to the referenced comp's collision(s); mapping matches the SDF `<surface>` path exactly (mu1 ->
    /// friction @static, mu2 -> friction-direction/@mu2, kp -> contact @stiffness, kd -> contact @damping).
    pub surface: Option<Surface>,
    /// The visual `<color>` decoded from the block's Gazebo-classic per-link `<material>Gazebo/&lt;Color&gt;
    /// </material>` preset, or `None` if the block carries no material or one whose
    /// name is not a recognized preset (that stays quarantined verbatim). The caller applies it to the
    /// referenced comp's primitive visuals; when it CAN (at least one primitive visual takes the color) it
    /// passes `drop_material=true` to [`GazeboResidual::render`] so the `<material>` leaves the residual,
    /// otherwise the leaf stays quarantined verbatim rather than being lost (e.g. a GLB-only visual).
    pub material: Option<Color>,
    /// The residual `<gazebo>` quarantine body, rendered by the caller with the friction/material fragments
    /// dropped exactly when they were mapped to typed HCDF (see [`GazeboResidual::render`]).
    pub residual: GazeboResidual,
}

/// One serialized residual `<gazebo>` child, tagged by which optional consumption removes it: `friction`
/// for a `<mu1>/<mu2>/<kp>/<kd>` idiom leaf (dropped once the typed `<surface>` was applied), `material`
/// for the recognized `Gazebo/<Color>` preset (dropped once the typed `<color>` was applied).
struct ResidualChild {
    friction: bool,
    material: bool,
    xml: String,
}

/// The residual of a decomposed `<gazebo>` block: its opening attributes, the serialized children NOT
/// consumed as typed sensors (in document order), and the typed sensors' un-mapped sim-only sub-field
/// envelopes (`frags`). [`render`](Self::render) reconstructs the verbatim `<gazebo>` quarantine body,
/// omitting the friction and/or material fragments the caller mapped to typed HCDF.
pub(crate) struct GazeboResidual {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<ResidualChild>,
    frags: Vec<String>,
}

impl GazeboResidual {
    /// Reconstruct the residual `<gazebo>` quarantine body, dropping the friction idiom (`drop_friction`)
    /// and/or the recognized material preset (`drop_material`) the caller consumed into typed HCDF. Empty
    /// when nothing residual remains (the caller then drops the block from the quarantine entirely).
    pub(crate) fn render(&self, drop_friction: bool, drop_material: bool) -> String {
        let kept =
            |c: &ResidualChild| !(drop_friction && c.friction || drop_material && c.material);
        if !self.children.iter().any(&kept) && self.frags.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        out.push('<');
        out.push_str(&self.tag);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            xml_escape_into(v, true, &mut out);
            out.push('"');
        }
        out.push('>');
        for c in &self.children {
            if kept(c) {
                out.push_str(&c.xml);
            }
        }
        for f in &self.frags {
            out.push_str(f);
        }
        out.push_str("</");
        out.push_str(&self.tag);
        out.push('>');
        out
    }
}

/// URDF/Gazebo front end. Reach INTO one quarantined top-level
/// `<gazebo>` block (handed verbatim as `raw` by the URDF importer) and decompose two things through the
/// SAME mappers the SDF re-root uses: its `<sensor>` children (via [`map_sensor`]; Gazebo reuses the
/// SDFormat sensor grammar), its Gazebo-classic friction/contact idiom (`<mu1>/<mu2>/<kp>/<kd>`, mapped
/// exactly like the SDF `<surface>` path), and its per-link `<material>Gazebo/<Color></material>` preset
/// (decoded to a visual `<color>`). The typed `<sensor>`s leave the block; the
/// friction idiom and a recognized material stay in the [`GazeboResidual`] tagged so the caller drops them
/// only once mapped to typed HCDF. Everything else (`<plugin>`, un-typed `<sensor>`, unrecognized material,
/// the typed sensors' un-mapped sim-only sub-fields) is re-emitted for verbatim re-quarantine. Returns
/// `None` when nothing was typed so the caller keeps quarantining the block byte-for-byte.
pub(crate) fn gazebo_block_extract(
    raw: &str,
    notes: &mut Vec<String>,
) -> Result<Option<GazeboExtract>> {
    let gel = parse_dom(raw)?;
    if gel.tag != "gazebo" {
        return Ok(None);
    }
    let reference = gel.get("reference").map(str::to_string);
    let ctx = match &reference {
        Some(r) => format!("gazebo reference={}", repr_str(r)),
        None => "gazebo".to_string(),
    };
    let mut sensors: Vec<Sensor> = Vec::new();
    let mut frags: Vec<String> = Vec::new();
    // Every child NOT consumed as a typed sensor is serialized into the residual in document order, each
    // tagged by whether the friction idiom (`drop_friction`) or the material preset (`drop_material`) will
    // remove it when [`GazeboResidual::render`] runs. The friction leaves and a recognized `<material>` are
    // KEPT here so they survive verbatim whenever the caller could not attach them (no collision / no
    // primitive visual); the caller drops them only once mapped.
    let mut children: Vec<ResidualChild> = Vec::new();
    let mut material: Option<Color> = None;
    let (mut mu1, mut mu2, mut kp, mut kd) = (None, None, None, None);
    // Gazebo-classic anisotropic friction leaves (direct <gazebo reference> children).
    let (mut fdir1, mut slip1, mut slip2) = (None, None, None);
    for ch in &gel.children {
        match ch.tag.as_str() {
            "sensor" => match map_sensor(ch, notes, &ctx, &mut frags) {
                // typed -> consumed; the raw <sensor> does not stay in the residual.
                Some(s) => sensors.push(s),
                // unmapped type -> keep the <sensor> verbatim in the residual (never silently dropped).
                None => children.push(residual_child(ch, false, false)),
            },
            // Gazebo-classic per-link material override (`<material>Gazebo/Blue</material>`, an Ogre
            // script name). A recognized preset decodes to an rgba; it stays in the residual tagged
            // `material` so render drops it iff the caller applied the color. An unrecognized name has no
            // recoverable flat rgba, so it stays quarantined verbatim (untagged) + noted.
            "material" => {
                let script = ch.txt();
                match script.and_then(gazebo_material_rgba) {
                    Some(rgba) => {
                        material = Some(Color {
                            rgba: Some(rgba),
                            ..Default::default()
                        });
                        children.push(residual_child(ch, false, true));
                    }
                    None => {
                        notes.push(format!(
                            "{ctx}: <material> {} is not a recognized Gazebo/<Color> preset; kept quarantined verbatim (no flat rgba recoverable)",
                            repr_opt(script)
                        ));
                        children.push(residual_child(ch, false, false));
                    }
                }
            }
            // Gazebo-classic universal friction/contact idiom (direct <gazebo reference> children).
            "mu1" => {
                mu1 = ch.txt().map(str::to_string);
                children.push(residual_child(ch, true, false));
            }
            "mu2" => {
                mu2 = ch.txt().map(str::to_string);
                children.push(residual_child(ch, true, false));
            }
            "kp" => {
                kp = ch.txt().map(str::to_string);
                children.push(residual_child(ch, true, false));
            }
            "kd" => {
                kd = ch.txt().map(str::to_string);
                children.push(residual_child(ch, true, false));
            }
            "fdir1" => {
                fdir1 = ch.txt().map(str::to_string);
                children.push(residual_child(ch, true, false));
            }
            "slip1" => {
                slip1 = ch.txt().map(str::to_string);
                children.push(residual_child(ch, true, false));
            }
            "slip2" => {
                slip2 = ch.txt().map(str::to_string);
                children.push(residual_child(ch, true, false));
            }
            _ => children.push(residual_child(ch, false, false)),
        }
    }
    let friction_direction =
        (fdir1.is_some() || slip1.is_some() || slip2.is_some()).then_some(FrictionDirection {
            fdir1,
            slip1,
            slip2,
            ..Default::default()
        });
    let surface = gazebo_friction_surface(mu1, mu2, kp, kd, friction_direction);
    if sensors.is_empty() && surface.is_none() && material.is_none() {
        // nothing was typed: let the caller keep the block verbatim (no residual reconstruction).
        return Ok(None);
    }
    let residual = GazeboResidual {
        tag: gel.tag.clone(),
        attrs: gel
            .attrs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        children,
        frags,
    };
    Ok(Some(GazeboExtract {
        reference,
        sensors,
        surface,
        material,
        residual,
    }))
}

/// Serialize one residual `<gazebo>` child to verbatim XML, tagged by which typed consumption removes it.
fn residual_child(ch: &SdfEl, friction: bool, material: bool) -> ResidualChild {
    let mut xml = String::new();
    el_to_xml(ch, &mut xml);
    ResidualChild {
        friction,
        material,
        xml,
    }
}

/// The rgba (`"r g b a"`, alpha 1) for a Gazebo-classic `Gazebo/<Color>` named material script: the
/// common presets from Gazebo's `media/materials/scripts/gazebo.material` (diffuse values). Returns
/// `None` for any unrecognized name (including textured / non-preset scripts), whose flat colour is not
/// recoverable, so the caller keeps that `<material>` quarantined verbatim.
fn gazebo_material_rgba(script: &str) -> Option<String> {
    let rgb = match script.trim() {
        "Gazebo/Grey" => "0.7 0.7 0.7",
        "Gazebo/DarkGrey" => "0.175 0.175 0.175",
        "Gazebo/White" => "1 1 1",
        "Gazebo/FlatBlack" => "0.1 0.1 0.1",
        "Gazebo/Black" => "0 0 0",
        "Gazebo/Red" => "1 0 0",
        "Gazebo/RedBright" => "0.87 0.29 0.19",
        "Gazebo/Green" => "0 1 0",
        "Gazebo/Blue" => "0 0 1",
        "Gazebo/SkyBlue" => "0.13 0.44 0.7",
        "Gazebo/Yellow" => "1 1 0",
        "Gazebo/DarkYellow" => "0.7 0.7 0",
        "Gazebo/Purple" => "1 0 1",
        "Gazebo/Turquoise" => "0 1 1",
        "Gazebo/Orange" => "1 0.5088 0.0468",
        _ => return None,
    };
    Some(format!("{rgb} 1"))
}

/// Build a collision [`Surface`] from the Gazebo-classic friction/contact idiom leaves, matching the SDF
/// `<surface>` field mapping exactly: `<mu1>` -> friction `@static`, `<mu2>` -> friction-direction `@mu2`,
/// `<kp>` -> contact `@stiffness`, `<kd>` -> contact `@damping`, and the anisotropic leaves
/// `<fdir1>/<slip1>/<slip2>` -> `<friction-direction>`. Returns `None` when no leaf is present.
fn gazebo_friction_surface(
    mu1: Option<String>,
    mu2: Option<String>,
    kp: Option<String>,
    kd: Option<String>,
    friction_direction: Option<FrictionDirection>,
) -> Option<Surface> {
    // Gazebo-classic <mu2> is the SECOND-friction-direction coefficient, not kinetic friction,
    // so it folds into <friction-direction>/@mu2 (creating the block when only <mu2> was present), never
    // into @dynamic. @static (mu1) stays the primary coefficient.
    let friction_direction = match mu2 {
        Some(mu2) => {
            let mut fd = friction_direction.unwrap_or_default();
            fd.mu2 = Some(mu2);
            Some(fd)
        }
        None => friction_direction,
    };
    let friction = (mu1.is_some() || friction_direction.is_some()).then_some(Friction {
        static_: mu1,
        dynamic: None,
        friction_direction,
    });
    let contact = (kp.is_some() || kd.is_some()).then_some(Contact {
        stiffness: kp,
        damping: kd,
    });
    (friction.is_some() || contact.is_some()).then_some(Surface {
        friction,
        restitution: None,
        contact,
    })
}

/// SDF sensor-level runtime/sim-only fields (`always_on`, `visualize`, `topic`, `plugin`, ...): no
/// hardware meaning, so they are dropped WITH a note (never silent). `<pose>`/`<update_rate>` are mapped
/// and excluded.
fn note_runtime_fields(sel: &SdfEl, notes: &mut Vec<String>, ctx: &str) {
    const RUNTIME: &[&str] = &[
        "always_on",
        "alwaysOn",
        "visualize",
        "topic",
        "enable_metrics",
        "plugin",
        "save",
        "gz_frame_id",
        "ignition_frame_id",
    ];
    let present: Vec<&str> = RUNTIME
        .iter()
        .copied()
        .filter(|t| sel.find(t).is_some())
        .collect();
    if !present.is_empty() {
        notes.push(format!(
            "{ctx}: runtime/sim-only field(s) {} not represented (HCDF is a hardware description); dropped",
            present.join(", ")
        ));
    }
}

/// Note the payload sub-elements a per-type mapper did NOT consume: the payload-internal half of the
/// no-silent-drop invariant.
fn note_residual(
    payload: &SdfEl,
    consumed: &[&str],
    notes: &mut Vec<String>,
    ctx: &str,
    what: &str,
) {
    let mut left: Vec<&str> = payload
        .children
        .iter()
        .map(|c| c.tag.as_str())
        .filter(|t| !consumed.contains(t))
        .collect();
    left.sort_unstable();
    left.dedup();
    if !left.is_empty() {
        notes.push(format!(
            "{ctx}: {what} sub-element(s) {} not represented; dropped",
            left.join(", ")
        ));
    }
}

/// SDF `<noise>` -> HCDF [`Noise`]. SDFormat carries the type as a `<type>` child (attribute-form
/// tolerated). `gaussian_quantized` collapses to `gaussian` (quantization noted); an unknown type is
/// noted and left untyped. The sim-only `dynamic_bias_stddev`/`dynamic_bias_correlation_time` leaves are
/// captured into the typed noise. Returns `None` for an all-empty noise element.
fn sdf_noise(nel: &SdfEl, notes: &mut Vec<String>, ctx: &str) -> Option<Noise> {
    let raw_type = nel.child_txt("type").or_else(|| nel.get("type"));
    let type_ = raw_type.and_then(|t| match t {
        "gaussian_quantized" => {
            notes.push(format!(
                "{ctx}: <noise> type 'gaussian_quantized' mapped to 'gaussian' (quantization not represented)"
            ));
            Some(NoiseType::Gaussian)
        }
        other => match NoiseType::from_str(other) {
            Ok(n) => Some(n),
            Err(()) => {
                notes.push(format!(
                    "{ctx}: <noise> type {} not represented",
                    repr_str(other)
                ));
                None
            }
        },
    });
    let noise = Noise {
        type_,
        mean: nel.child_txt("mean").map(str::to_string),
        stddev: nel.child_txt("stddev").map(str::to_string),
        bias_mean: nel.child_txt("bias_mean").map(str::to_string),
        bias_stddev: nel.child_txt("bias_stddev").map(str::to_string),
        precision: nel.child_txt("precision").map(str::to_string),
        // The sim-only dynamic (Gauss-Markov) bias leaves now land in the typed noise instead of
        // being quarantined to org.gazebosim.raw.
        dynamic_bias_stddev: nel.child_txt("dynamic_bias_stddev").map(str::to_string),
        dynamic_bias_correlation_time: nel
            .child_txt("dynamic_bias_correlation_time")
            .map(str::to_string),
    };
    (noise != Noise::default()).then_some(noise)
}

/// `<x>/<y>/<z>` per-axis noise (IMU channel / magnetometer body) collapses to ONE HCDF `<noise>`
/// (x-axis representative). y/z per-axis noise has no schema home; its presence is noted (an accepted
/// loss). The SDF sim-only `dynamic_bias_*` leaves are quarantined verbatim by the caller.
fn axis_noise_collapsed(
    payload: &SdfEl,
    notes: &mut Vec<String>,
    ctx: &str,
    what: &str,
) -> Option<Noise> {
    let noise = payload
        .find("x")
        .and_then(|a| a.find("noise"))
        .and_then(|n| sdf_noise(n, notes, ctx));
    let has_yz = ["y", "z"]
        .iter()
        .any(|ax| payload.find(ax).and_then(|a| a.find("noise")).is_some());
    if has_yz {
        notes.push(format!(
            "{ctx}: {what} per-axis noise collapsed to a single <noise> (x-axis); y/z-axis noise not represented"
        ));
    }
    noise
}

/// `imu` -> `<inertial type="accel_gyro">`. Per-axis Gaussian `<noise>` (incl. the sim-only dynamic-bias
/// leaves) -> a full per-axis [`AxisNoise`] on each accel/gyro channel, keeping the x-axis
/// `<noise>` as the scalar isotropic fallback.
fn imu_sensor(
    sel: &SdfEl,
    pose: Option<Pose>,
    notes: &mut Vec<String>,
    sc: &mut SensorCtx,
) -> InertialSensor {
    let mut out = InertialSensor {
        type_: Some(InertialSensorType::AccelGyro),
        pose,
        ..Default::default()
    };
    let Some(imu) = sel.find("imu") else {
        return out;
    };
    out.accel = channel_params(imu, "linear_acceleration", notes, sc.ctx);
    out.gyro = channel_params(imu, "angular_velocity", notes, sc.ctx);
    note_residual(
        imu,
        &["linear_acceleration", "angular_velocity"],
        notes,
        sc.ctx,
        "<imu>",
    );
    out
}

/// One IMU channel (`linear_acceleration`/`angular_velocity`) -> a `<accel>`/`<gyro>` [`SensorParams`]:
/// the x-axis `<noise>` stays the scalar isotropic fallback, and a per-axis [`AxisNoise`] (x/y/z) is
/// populated whenever the channel carries genuinely anisotropic (y/z) noise.
fn channel_params(
    imu: &SdfEl,
    channel: &str,
    notes: &mut Vec<String>,
    ctx: &str,
) -> Option<SensorParams> {
    let ch = imu.find(channel)?;
    let axis = |ax: &str, notes: &mut Vec<String>| {
        ch.find(ax)
            .and_then(|a| a.find("noise"))
            .and_then(|n| sdf_noise(n, notes, ctx))
    };
    let x = axis("x", notes);
    let y = axis("y", notes);
    let z = axis("z", notes);
    if x.is_none() && y.is_none() && z.is_none() {
        return None;
    }
    // The scalar `<noise>` fallback stays the x-axis representative; the full per-axis triplet is added
    // only when a y/z axis actually carries its own noise (isotropic channels keep just the scalar).
    let axis_noise = (y.is_some() || z.is_some()).then(|| AxisNoise { x: x.clone(), y, z });
    Some(SensorParams {
        noise: x,
        axis_noise,
        ..Default::default()
    })
}

/// `camera`/`depth_camera`/`rgbd_camera`/`thermal` -> `<optical>` + one `<fov>` (intrinsics/distortion/
/// lens/frustum). Depth types route `<clip>` to the typed `camera-params` depth-range; a plain camera
/// derives the frustum near/far AND quarantines the raw `<clip>`.
fn camera_optical(
    sel: &SdfEl,
    pose: Option<Pose>,
    otype: OpticalSensorType,
    notes: &mut Vec<String>,
    sc: &mut SensorCtx,
) -> OpticalSensor {
    let mut out = OpticalSensor {
        type_: Some(otype),
        pose,
        ..Default::default()
    };
    let Some(cam) = sel.find("camera") else {
        notes.push(format!(
            "{}: <camera> payload absent; only the group mapped",
            sc.ctx
        ));
        return out;
    };
    let is_depth = matches!(otype, OpticalSensorType::Tof);
    out.fov.push(camera_fov(cam, notes, sc.ctx));
    if is_depth {
        if let Some(dr) = depth_range_from_clip(cam) {
            out.camera_params = Some(CameraParams {
                depth_range: Some(dr),
                ..Default::default()
            });
        }
    } else if let Some(clip) = cam.find("clip") {
        let mut body = String::new();
        el_to_xml(clip, &mut body);
        sc.quarantine(&body);
        notes.push(format!(
            "{}: camera <clip> near/far derived into the FoV frustum; raw <clip> quarantined to <extension domain=\"org.gazebosim.raw\">",
            sc.ctx
        ));
    }
    note_residual(
        cam,
        &["horizontal_fov", "image", "clip", "distortion", "lens"],
        notes,
        sc.ctx,
        "<camera>",
    );
    out
}

/// The `<camera>` -> a single [`SensorFov`]: intrinsics (image dims/format + fx/fy/cx/cy from an explicit
/// `<lens><intrinsics>` or derived from `<horizontal_fov>`+image), Brown-Conrady `<distortion>`, a
/// non-rectilinear `<lens>` projection model, and a pyramidal frustum (near/far/hfov) for visualization.
fn camera_fov(cam: &SdfEl, notes: &mut Vec<String>, ctx: &str) -> SensorFov {
    SensorFov {
        // `<fov name>` is schema-required (hcdf.xsd sensor_fov). SDFormat's `<camera>` is single-view,
        // so the import yields exactly one FoV; name it "main" (matching the test convention) rather
        // than emitting an anonymous `<fov>` that would fail the document's own XSD.
        name: Some("main".to_string()),
        geometry: camera_frustum(cam),
        intrinsics: camera_intrinsics(cam, notes, ctx),
        distortion: camera_distortion(cam),
        lens: camera_lens(cam.find("lens")),
        camera_matrix: camera_matrix(cam),
        ..Default::default()
    }
}

/// `<lens><projection>` (SDF `p_fx/p_fy/p_cx/p_cy/tx/ty`) -> the ROS CameraInfo projection matrix P
/// inside `<camera-matrix>`. SDFormat's `<camera>` carries no rectification R / binning / ROI, so
/// those `<camera-matrix>` children stay absent on an SDF import.
fn camera_matrix(cam: &SdfEl) -> Option<CameraMatrix> {
    let proj = cam.find("lens").and_then(|l| l.find("projection"))?;
    let p = CameraProjection {
        fx: proj.child_txt("p_fx").map(str::to_string),
        fy: proj.child_txt("p_fy").map(str::to_string),
        cx: proj.child_txt("p_cx").map(str::to_string),
        cy: proj.child_txt("p_cy").map(str::to_string),
        tx: proj.child_txt("tx").map(str::to_string),
        ty: proj.child_txt("ty").map(str::to_string),
    };
    (p != CameraProjection::default()).then(|| CameraMatrix {
        projection: Some(p),
        ..Default::default()
    })
}

/// `<image>` dims/format + intrinsics from an explicit `<lens><intrinsics>` (fx/fy/cx/cy/s) or derived
/// from `<horizontal_fov>` + image width (square-pixel pinhole: fx = (w/2)/tan(hfov/2), c = image centre).
fn camera_intrinsics(cam: &SdfEl, notes: &mut Vec<String>, ctx: &str) -> Option<CameraIntrinsics> {
    let image = cam.find("image");
    let width = image.and_then(|i| i.child_txt("width"));
    let height = image.and_then(|i| i.child_txt("height"));
    let mut intr = CameraIntrinsics {
        width: width.map(str::to_string),
        height: height.map(str::to_string),
        format: image
            .and_then(|i| i.child_txt("format"))
            .map(str::to_string),
        ..Default::default()
    };
    if let Some(li) = cam.find("lens").and_then(|l| l.find("intrinsics")) {
        intr.fx = li.child_txt("fx").map(str::to_string);
        intr.fy = li.child_txt("fy").map(str::to_string);
        intr.cx = li.child_txt("cx").map(str::to_string);
        intr.cy = li.child_txt("cy").map(str::to_string);
        intr.s = li.child_txt("s").map(str::to_string);
    } else if let (Some(hfov), Some(w)) = (
        cam.child_txt("horizontal_fov").and_then(parse_f),
        width.and_then(parse_f),
    ) {
        let fx = (w / 2.0) / (hfov / 2.0).tan();
        intr.fx = Some(fmt_num(fx));
        intr.fy = Some(fmt_num(fx));
        intr.cx = Some(fmt_num(w / 2.0));
        intr.cy = height.and_then(parse_f).map(|h| fmt_num(h / 2.0));
        notes.push(format!(
            "{ctx}: camera intrinsics fx/fy/cx/cy derived from <horizontal_fov>+<image> (square-pixel pinhole)"
        ));
    }
    (intr != CameraIntrinsics::default()).then_some(intr)
}

/// `<distortion>` (Brown-Conrady k1-k3/p1-p2) -> [`CameraDistortion`]. `<center>` has no schema home.
fn camera_distortion(cam: &SdfEl) -> Option<CameraDistortion> {
    let d = cam.find("distortion")?;
    let out = CameraDistortion {
        k1: d.child_txt("k1").map(str::to_string),
        k2: d.child_txt("k2").map(str::to_string),
        k3: d.child_txt("k3").map(str::to_string),
        p1: d.child_txt("p1").map(str::to_string),
        p2: d.child_txt("p2").map(str::to_string),
        ..Default::default()
    };
    (out != CameraDistortion::default()).then_some(out)
}

/// `<lens>` -> [`CameraLens`] ONLY for a non-rectilinear (fisheye/wide-angle) lens Brown-Conrady cannot
/// describe. A default rectilinear/`gnomonical` pinhole lens carries no extra data, so it
/// yields `None` (the intrinsics already model it).
fn camera_lens(lens: Option<&SdfEl>) -> Option<CameraLens> {
    let lens = lens?;
    let raw = lens.child_txt("type");
    let non_rectilinear = raw.is_some_and(|t| t != "gnomonical" && t != "pinhole");
    let has_extras =
        lens.find("custom_function").is_some() || lens.child_txt("cutoff_angle").is_some();
    if !non_rectilinear && !has_extras {
        return None;
    }
    let custom = lens
        .find("custom_function")
        .map(|c| CameraLensCustomFunction {
            c1: c.child_txt("c1").map(str::to_string),
            c2: c.child_txt("c2").map(str::to_string),
            c3: c.child_txt("c3").map(str::to_string),
            f: c.child_txt("f").map(str::to_string),
            fun: c.child_txt("fun").map(str::to_string),
        });
    Some(CameraLens {
        type_: raw.map(|t| map_lens_type(t).to_string()),
        cutoff_angle: lens.child_txt("cutoff_angle").map(|v| MeasuredValue {
            unit: Some("rad".to_string()),
            value: Some(v.to_string()),
        }),
        scale_to_hfov: lens.child_txt("scale_to_hfov").map(str::to_string),
        custom_function: custom,
    })
}

/// SDFormat `<lens><type>` vocabulary -> the HCDF `camera_projection_type` literals (validated at
/// validate time). Unknown values pass through verbatim (a validate-time error, never a silent change).
fn map_lens_type(t: &str) -> &str {
    match t {
        "gnomonical" => "pinhole",
        "equisolid_angle" => "equisolid",
        other => other,
    }
}

/// A camera FoV frustum (pyramidal) from `<clip>` near/far + `<horizontal_fov>` for visualization.
fn camera_frustum(cam: &SdfEl) -> Option<Geometry> {
    let clip = cam.find("clip");
    let near = clip.and_then(|c| c.child_txt("near"));
    let far = clip.and_then(|c| c.child_txt("far"));
    let hfov = cam.child_txt("horizontal_fov");
    if near.is_none() && far.is_none() && hfov.is_none() {
        return None;
    }
    Some(Geometry {
        frustum: Some(Frustum {
            shape: Some(FrustumShape::Pyramidal),
            near: near.map(str::to_string),
            far: far.map(str::to_string),
            hfov: hfov.map(str::to_string),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// A depth/tof camera `<clip>` -> a `camera-params` depth-range (near=min, far=max).
fn depth_range_from_clip(cam: &SdfEl) -> Option<RangeValue> {
    let clip = cam.find("clip")?;
    let near = clip.child_txt("near");
    let far = clip.child_txt("far");
    (near.is_some() || far.is_some()).then(|| RangeValue {
        min: near.map(str::to_string),
        max: far.map(str::to_string),
        ..Default::default()
    })
}

/// `lidar`/`gpu_lidar`/`ray` -> `<optical type="lidar">` + `<lidar-params>`: `<range>` (min/max/
/// resolution), the angular `<scan-pattern>` (horizontal/vertical samples/resolution/
/// min-angle/max-angle), and the beam `<noise>`, the schema's explicitly lossless target for a
/// converted ray sensor.
fn lidar_optical(
    sel: &SdfEl,
    pose: Option<Pose>,
    notes: &mut Vec<String>,
    ctx: &str,
) -> OpticalSensor {
    let mut out = OpticalSensor {
        type_: Some(OpticalSensorType::Lidar),
        pose,
        ..Default::default()
    };
    let Some(l) = sel.find("lidar").or_else(|| sel.find("ray")) else {
        notes.push(format!(
            "{ctx}: <lidar>/<ray> payload absent; only the group mapped"
        ));
        return out;
    };
    let mut lp = LidarParams::default();
    if let Some(r) = l.find("range") {
        lp.range = Some(LidarRange {
            min: r.child_txt("min").map(bare_measure),
            max: r.child_txt("max").map(bare_measure),
            resolution: r.child_txt("resolution").map(bare_measure),
        });
    }
    if let Some(scan) = l.find("scan") {
        let horizontal = scan.find("horizontal").map(scan_axis);
        let vertical = scan.find("vertical").map(scan_axis);
        if horizontal.is_some() || vertical.is_some() {
            lp.scan_pattern = Some(LidarScanPattern {
                horizontal,
                vertical,
            });
        }
    }
    // The along-beam range/beam <noise> model (SDF lidar/ray <noise>).
    lp.noise = l.find("noise").and_then(|n| sdf_noise(n, notes, ctx));
    if lp != LidarParams::default() {
        out.lidar_params = Some(lp);
    }
    note_residual(l, &["range", "scan", "noise"], notes, ctx, "<lidar>");
    out
}

/// One `<horizontal>`/`<vertical>` scan axis -> [`LidarScanAxis`] (all attributes, verbatim text).
fn scan_axis(ax: &SdfEl) -> LidarScanAxis {
    LidarScanAxis {
        samples: ax.child_txt("samples").map(str::to_string),
        resolution: ax.child_txt("resolution").map(str::to_string),
        min_angle: ax.child_txt("min_angle").map(str::to_string),
        max_angle: ax.child_txt("max_angle").map(str::to_string),
    }
}

/// `magnetometer` -> `<em type="mag">` with collapsed x/y/z field noise.
fn mag_em(sel: &SdfEl, pose: Option<Pose>, notes: &mut Vec<String>, ctx: &str) -> SensorCategory {
    let mut cat = SensorCategory {
        type_: Some("mag".to_string()),
        pose,
        ..Default::default()
    };
    if let Some(m) = sel.find("magnetometer") {
        cat.noise = axis_noise_collapsed(m, notes, ctx, "<magnetometer>");
        note_residual(m, &["x", "y", "z"], notes, ctx, "<magnetometer>");
    }
    cat
}

/// `gps`/`navsat` -> `<rf type="gnss">`; position-sensing noise collapses onto the sensor-base `<noise>`.
fn gnss_rf(sel: &SdfEl, pose: Option<Pose>, notes: &mut Vec<String>, ctx: &str) -> SensorCategory {
    let mut cat = SensorCategory {
        type_: Some("gnss".to_string()),
        pose,
        ..Default::default()
    };
    let Some(p) = sel.find("navsat").or_else(|| sel.find("gps")) else {
        return cat;
    };
    cat.noise = p
        .find_path("position_sensing/horizontal/noise")
        .or_else(|| p.find_path("position_sensing/vertical/noise"))
        .and_then(|n| sdf_noise(n, notes, ctx));
    if p.find("velocity_sensing").is_some() {
        notes.push(format!(
            "{ctx}: <velocity_sensing> noise not represented (sensor-base carries one position <noise>)"
        ));
    }
    cat
}

/// `air_pressure`/`altimeter` -> `<fluid type="barometer">`: absolute-mode barometer, its
/// pressure/vertical-position Gaussian noise mapped; the SDF-only reference/rate fields are noted.
fn barometer_fluid(
    sel: &SdfEl,
    pose: Option<Pose>,
    notes: &mut Vec<String>,
    ctx: &str,
    stype: &str,
) -> FluidSensor {
    let mut f = FluidSensor {
        type_: Some("barometer".to_string()),
        pose,
        ..Default::default()
    };
    if stype == "air_pressure" {
        if let Some(ap) = sel.find("air_pressure") {
            f.noise = ap
                .find_path("pressure/noise")
                .and_then(|n| sdf_noise(n, notes, ctx));
            if let Some(ra) = ap.child_txt("reference_altitude") {
                notes.push(format!(
                    "{ctx}: <air_pressure><reference_altitude> {} not represented (barometer models pressure via <reference-pressure>)",
                    repr_str(ra)
                ));
            }
            note_residual(
                ap,
                &["pressure", "reference_altitude"],
                notes,
                ctx,
                "<air_pressure>",
            );
        }
    } else if let Some(al) = sel.find("altimeter") {
        f.noise = al
            .find_path("vertical_position/noise")
            .and_then(|n| sdf_noise(n, notes, ctx));
        if al.find("vertical_velocity").is_some() {
            notes.push(format!(
                "{ctx}: <altimeter><vertical_velocity> not represented (barometer models pressure/altitude, not vertical rate)"
            ));
        }
        note_residual(
            al,
            &["vertical_position", "vertical_velocity"],
            notes,
            ctx,
            "<altimeter>",
        );
    }
    f
}

/// `contact` -> `<force type="pressure">` (solid contact pressure). The monitored
/// `<contact><collision>` is captured into the typed core collision-ref so the sensor names the surface
/// it watches (and so export can emit a VALID `<sensor type="contact">`); the `<topic>` is captured
/// separately into the org.ros2 topic-map extension (see [`ros2_topic_extension`]).
fn contact_force(
    sel: &SdfEl,
    pose: Option<Pose>,
    notes: &mut Vec<String>,
    ctx: &str,
) -> SensorCategory {
    let mut cat = SensorCategory {
        type_: Some("pressure".to_string()),
        pose,
        ..Default::default()
    };
    if let Some(c) = sel.find("contact") {
        // Collision-ref -> core; topic captured by ros2_topic_extension. Both are now consumed
        // (no longer note-dropped); any OTHER <contact> child is still noted, never silently dropped.
        cat.collision = c.child_txt("collision").map(str::to_string);
        note_residual(c, &["collision", "topic"], notes, ctx, "<contact>");
    }
    cat
}

/// `force_torque` -> `<force type="torque">`: the reporting `@frame`/`@measure-direction` sign
/// convention is captured, and the per-axis force/torque `<noise>` populates the typed `<axis-noise>`
/// (force/{x,y,z} + torque/{x,y,z}), keeping the x-axis representative as the scalar fallback.
fn force_torque_force(
    sel: &SdfEl,
    pose: Option<Pose>,
    notes: &mut Vec<String>,
    ctx: &str,
) -> SensorCategory {
    let mut cat = SensorCategory {
        type_: Some("torque".to_string()),
        pose,
        ..Default::default()
    };
    if let Some(ft) = sel.find("force_torque") {
        if let Some(fr) = ft.child_txt("frame") {
            match ForceFrame::from_str(fr) {
                Ok(f) => cat.frame = Some(f),
                Err(()) => notes.push(format!(
                    "{ctx}: <force_torque><frame> {} not a recognized reporting frame",
                    repr_str(fr)
                )),
            }
        }
        if let Some(md) = ft.child_txt("measure_direction") {
            match MeasureDirection::from_str(md) {
                Ok(m) => cat.measure_direction = Some(m),
                Err(()) => notes.push(format!(
                    "{ctx}: <force_torque><measure_direction> {} not a recognized sign convention",
                    repr_str(md)
                )),
            }
        }
        let force = ft.find("force").and_then(|f| ft_axis_noise(f, notes, ctx));
        let torque = ft.find("torque").and_then(|t| ft_axis_noise(t, notes, ctx));
        // Scalar fallback: the x-axis representative (torque preferred over force).
        cat.noise = torque
            .as_ref()
            .and_then(|t| t.x.clone())
            .or_else(|| force.as_ref().and_then(|f| f.x.clone()));
        if force.is_some() || torque.is_some() {
            cat.axis_noise = Some(ForceAxisNoise { force, torque });
        }
        note_residual(
            ft,
            &["frame", "measure_direction", "force", "torque"],
            notes,
            ctx,
            "<force_torque>",
        );
    }
    cat
}

/// A force_torque `<force>`/`<torque>` payload -> a per-axis [`AxisNoise`] (its `<x|y|z>/<noise>`).
/// Returns `None` when no axis carries a noise model.
fn ft_axis_noise(payload: &SdfEl, notes: &mut Vec<String>, ctx: &str) -> Option<AxisNoise> {
    let ax = |a: &str, notes: &mut Vec<String>| {
        payload
            .find(a)
            .and_then(|e| e.find("noise"))
            .and_then(|n| sdf_noise(n, notes, ctx))
    };
    let x = ax("x", notes);
    let y = ax("y", notes);
    let z = ax("z", notes);
    (x.is_some() || y.is_some() || z.is_some()).then_some(AxisNoise { x, y, z })
}

// ── quarantine XML helpers ──────────────────────────────────────────────────────────────────────────

/// Parse a single float from trimmed text (for derived intrinsics).
fn parse_f(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

/// Compact `f64` -> text: integral values without a fractional part, else six-decimal trimmed.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.6}");
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    trimmed.to_string()
}

/// A unit-less [`MeasuredValue`] carrying just the source text (lidar range min/max/resolution).
fn bare_measure(text: &str) -> MeasuredValue {
    MeasuredValue {
        unit: None,
        value: Some(text.to_string()),
    }
}

/// Serialize an [`SdfEl`] sub-tree back to compact XML for verbatim quarantine into an `<extension>`
/// body. Sensor payload leaves are simple (text OR children, never mixed), so leaf text and child
/// elements are emitted in that order.
fn el_to_xml(el: &SdfEl, out: &mut String) {
    out.push('<');
    out.push_str(&el.tag);
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        xml_escape_into(v, true, out);
        out.push('"');
    }
    let text = el.txt();
    if el.children.is_empty() && text.is_none() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(t) = text {
        xml_escape_into(t, false, out);
    }
    for c in &el.children {
        el_to_xml(c, out);
    }
    out.push_str("</");
    out.push_str(&el.tag);
    out.push('>');
}

/// A double-quoted, escaped XML attribute value.
fn xml_attr(v: &str) -> String {
    let mut s = String::with_capacity(v.len() + 2);
    s.push('"');
    xml_escape_into(v, true, &mut s);
    s.push('"');
    s
}

/// Escape XML text (or, with `attr`, an attribute value) into `out`.
fn xml_escape_into(v: &str, attr: bool, out: &mut String) {
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

// ── joints ────────────────────────────────────────────────────────────────────────────────────────

/// The per-joint frame context `<axis>`/`<axis2>` re-expression needs (bundled per the arg budget).
struct JointFrameCtx<'a> {
    graph: &'a FrameGraph,
    /// The `<child>` link name, the frame HCDF expresses joint axes in.
    child: Option<&'a str>,
    /// X(child←joint): the joint frame (SDF's DEFAULT `expressed_in`) located in the child frame.
    x_child_joint: Option<&'a XForm>,
}

/// SDF `<axis>`/`<axis2>` `<limit>` -> HCDF [`JointLimit`] (lower/upper/effort/velocity). `None` when the
/// axis carries no `<limit>` or every mapped bound is absent. Shared by the primary axis and axis2.
fn axis_limit(axis: &SdfEl) -> Option<JointLimit> {
    let lim = axis.find("limit")?;
    let jl = JointLimit {
        lower: lim.child_txt("lower").map(str::to_string),
        upper: lim.child_txt("upper").map(str::to_string),
        effort: lim.child_txt("effort").map(str::to_string),
        velocity: lim.child_txt("velocity").map(str::to_string),
        ..Default::default()
    };
    (jl.lower.is_some() || jl.upper.is_some() || jl.effort.is_some() || jl.velocity.is_some())
        .then_some(jl)
}

/// SDF `<axis>`/`<axis2>` `<dynamics>` -> HCDF [`JointDynamics`] (damping/friction/spring_*). `None` when
/// the axis carries no `<dynamics>` or every mapped field is absent. Shared by the primary axis and axis2.
fn axis_dynamics(axis: &SdfEl) -> Option<JointDynamics> {
    let dyn_ = axis.find("dynamics")?;
    let jd = JointDynamics {
        damping: dyn_.child_txt("damping").map(str::to_string),
        friction: dyn_.child_txt("friction").map(str::to_string),
        spring_stiffness: dyn_.child_txt("spring_stiffness").map(str::to_string),
        spring_reference: dyn_.child_txt("spring_reference").map(str::to_string),
    };
    (jd.damping.is_some()
        || jd.friction.is_some()
        || jd.spring_stiffness.is_some()
        || jd.spring_reference.is_some())
    .then_some(jd)
}

/// `<axis>/<xyz>` (or `<axis2>`) -> HCDF [`Axis`]. HCDF expresses the axis in the joint frame == child
/// comp frame (the [`mod@crate::to_sdf`] round-trip convention); SDF's default `expressed_in` is the
/// joint frame too, so the text passes VERBATIM whenever R(child←expressed_in) is the identity and is
/// re-expressed (`v' = R·v`, norm-preserving) otherwise. An unresolvable frame keeps the old
/// element-local reading + not-represented note.
fn axis_in_child_frame(
    xel: &SdfEl,
    tag: &str,
    fctx: &JointFrameCtx<'_>,
    ctx: &str,
    notes: &mut Vec<String>,
) -> Option<Axis> {
    let raw = xel.txt()?;
    let ein = xel.get("expressed_in").filter(|s| !s.is_empty());
    // R(child←expressed_in): default = the joint frame, via X(child←joint) (no name lookup needed, so
    // it also covers unnamed joints); a named frame goes through the graph.
    let rot = match ein {
        None => fctx.x_child_joint.map(XForm::rot),
        Some(e) => fctx
            .child
            .and_then(|c| fctx.graph.relative(c, e))
            .map(|x| x.rot()),
    };
    match (rot, parse3(raw)) {
        (Some(r), _) if rot_is_identity(&r, 1e-9) => Some(Axis {
            xyz: Some(raw.to_string()),
        }),
        (Some(r), Some(v)) => {
            let from = ein.map_or_else(|| "the joint frame".to_string(), repr_str);
            notes.push(format!(
                "{ctx}: <{tag}><xyz> re-expressed from {from} into the child frame"
            ));
            Some(Axis {
                xyz: Some(fmt3(mat3_vec(&r, v))),
            })
        }
        _ => {
            if let Some(e) = ein {
                notes.push(format!(
                    "{ctx}: <{tag}><xyz expressed_in={}> frame qualifier not represented; axis taken as element-local",
                    repr_str(e)
                ));
            }
            Some(Axis {
                xyz: Some(raw.to_string()),
            })
        }
    }
}

/// Exactly three whitespace-separated floats, or `None`.
fn parse3(s: &str) -> Option<[f64; 3]> {
    let mut it = s.split_whitespace();
    let v = [
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    ];
    it.next().is_none().then_some(v)
}

/// Compact 3-vector text for a re-expressed axis (integer-valued components as bare integers, the
/// same leaf style as the exporters' private `fmt3`).
fn fmt3(v: [f64; 3]) -> String {
    let f1 = |x: f64| {
        if x.fract() == 0.0 && x.is_finite() {
            format!("{}", x as i64)
        } else {
            format!("{x}")
        }
    };
    format!("{} {} {}", f1(v[0]), f1(v[1]), f1(v[2]))
}

fn joint(jel: &SdfEl, notes: &mut Vec<String>, graph: &FrameGraph) -> Result<Joint> {
    let name = jel.get("name").map(str::to_string);
    let ctx = format!("joint {}", repr_opt(name.as_deref()));
    let mut j = Joint {
        name: name.clone(),
        ..Default::default()
    };
    let jt = jel.get("type");
    let parent = jel.child_txt("parent");
    let child = jel.child_txt("child");
    if let Some(parent) = parent {
        j.parent = Some(JointEndpoint {
            comp: Some(parent.to_string()),
        });
    }
    if let Some(child) = child {
        j.child = Some(JointEndpoint {
            comp: Some(child.to_string()),
        });
    }

    // Joint origin through the resolved frame graph: the HCDF convention (see [`mod@crate::to_sdf`]) is
    // `origin = X(parent comp ← child comp)` with the joint anchored at the child comp frame; NOT the
    // raw `<pose>` text, which SDF expresses relative to the CHILD link (or `@relative_to`). This also
    // derives origins for pose-less joints whose placement lives entirely in the child link's pose.
    let pel = jel.find("pose");
    let jpose = pose_values_silent(pel); // a malformed pose was noted at graph-build time
                                         // X(child←joint): the joint's own pose hung off its relative_to base, re-expressed in the child
                                         // frame, the default <axis> frame and the anchor-offset diagnostic (works for unnamed joints too,
                                         // since the joint's own node is never looked up).
    let x_child_joint = child.and_then(|c| {
        let rel = pel
            .and_then(|p| p.get("relative_to"))
            .filter(|s| !s.is_empty())
            .unwrap_or(c);
        let local = jpose
            .as_ref()
            .map(XForm::from_pose)
            .unwrap_or_else(XForm::identity);
        let x_mj = graph.xform(rel)?.compose(&local);
        Some(graph.xform(c)?.inverse().compose(&x_mj))
    });
    // SDF reserves 'world' (not referable inside <model> EXCEPT as //joint/parent): that side anchors
    // at the model frame.
    let parent_frame = match parent {
        Some("world") => Some("__model__"),
        p => p,
    };
    let resolved = match (parent_frame, child) {
        (Some(p), Some(c)) => graph.relative(p, c),
        _ => None,
    };
    match resolved {
        Some(x) => {
            if parent == Some("world") {
                notes.push(format!(
                    "{ctx}: parent 'world' has no frame in the model; origin expressed in the model frame"
                ));
            }
            // Presence rule: a joint with no <pose> element and an identity transform keeps its origin
            // ABSENT (byte-faithful to the pre-frame-graph import of unposed models).
            if pel.is_some() || !x.is_identity(1e-12) {
                j.origin = Some(x.to_pose());
            }
        }
        None => {
            // An endpoint is missing or names no known frame, so the graph cannot place this joint; keep
            // the pre-frame-graph element-local reading (+ the not-represented qualifier note).
            if let Some(p) = pel {
                note_pose_qualifiers(p, notes, &ctx);
            }
            j.origin = jpose;
        }
    }

    // Map the SDF joint type onto an HCDF JointType. Most SDF types are 1:1 (JTYPE). SDF `revolute2`
    // is 2-DOF with two independent rotation axes -> HCDF `universal`, the closest kinematic match
    // (see the recorded note for the geometric caveat). `gearbox` is intercepted upstream in
    // [`from_model`] as a typed `<transmission type="gear">` (reaching this fn only as the dangling-child
    // fixed-edge fallback). Any other unknown type has no kinematic-DOF equivalent and downgrades to
    // `fixed`.
    let hcdf_type: Option<&str> = match jt {
        Some(t) if JTYPE.contains(&t) => Some(t),
        Some("revolute2") => {
            notes.push(format!(
                "{ctx}: SDF 'revolute2' imported as HCDF 'universal' (both 2-DOF rotational); SDF \
                 revolute2 axes need not intersect whereas HCDF universal implies intersecting \
                 perpendicular axes, a geometric approximation"
            ));
            Some("universal")
        }
        _ => None,
    };
    let Some(hcdf_type) = hcdf_type else {
        // No kinematic HCDF equivalent -> 'fixed'. A fixed joint forbids axis/limit, so we must NOT
        // carry them over (that would produce an invalid HCDF doc); record the drop instead.
        j.type_ = Some(JointType::Fixed);
        // A `gearbox` reaches this fixed downgrade ONLY as the dangling-child fixed-EDGE fallback that
        // [`from_model`] falls back to (its gear coupling is mapped to a typed `<transmission>`, and its
        // narrative is recorded there), so it gets NO "no HCDF equivalent" note here. Any OTHER unknown
        // type is an honest no-kinematic-equivalent downgrade.
        if jt != Some("gearbox") {
            notes.push(format!(
                "{ctx}: SDF joint type {} has no HCDF equivalent; imported as 'fixed'",
                repr_opt(jt)
            ));
        }
        for tag in ["axis", "axis2", "thread_pitch", "screw_thread_pitch"] {
            if jel.find(tag).is_some() {
                notes.push(format!(
                    "{ctx}: <{tag}> dropped with the {}->fixed downgrade",
                    repr_opt(jt).trim_matches('\'')
                ));
            }
        }
        return Ok(j);
    };
    j.type_ = Some(JointType::from_str(hcdf_type).map_err(|_| {
        Error::Xml(format!(
            "{ctx}: unknown SDF joint type {}",
            repr_str(hcdf_type)
        ))
    })?);

    // HCDF (like URDF) anchors a joint at the child comp origin. If the SDF joint frame sits OFF the
    // child link origin on a NON-fixed joint, the zero-configuration placement is still exact but the
    // rotation anchor is not representable; record it. (Fixed joints never rotate: anchor irrelevant.)
    if hcdf_type != "fixed" {
        let off = x_child_joint.as_ref().map_or([0.0; 3], |x| x.t);
        if (off[0] * off[0] + off[1] * off[1] + off[2] * off[2]).sqrt() > 1e-9 {
            notes.push(format!(
                "{ctx}: SDF joint frame is offset from the child link origin by ({}, {}, {}); HCDF anchors the joint at the child comp origin; placement is exact at zero configuration, the rotation anchor is not represented",
                off[0], off[1], off[2]
            ));
        }
    }

    let fctx = JointFrameCtx {
        graph,
        child,
        x_child_joint: x_child_joint.as_ref(),
    };
    if let Some(axis) = jel.find("axis") {
        if let Some(xel) = axis.find("xyz") {
            j.axis = axis_in_child_frame(xel, "axis", &fctx, &ctx, notes);
        }
        j.limit = axis_limit(axis);
        j.dynamics = axis_dynamics(axis);
    }
    // A continuous joint is unbounded: HCDF forbids position lower/upper on it (E_JOINT_CONTINUOUS_BOUNDS).
    // SDF/gazebo continuous joints routinely carry ±1e16 sentinel position bounds on <axis><limit>; copy
    // them in verbatim and the imported doc would fail HCDF's own validation. Strip lower/upper (keep the
    // meaningful effort/velocity) and record the drop; if nothing else remains, drop the empty <limit>.
    if hcdf_type == "continuous" {
        if let Some(lim) = j.limit.as_mut() {
            let had_lower = lim.lower.take().is_some();
            let had_upper = lim.upper.take().is_some();
            if had_lower || had_upper {
                notes.push(format!(
                    "{ctx}: dropped position lower/upper on the continuous joint (unbounded rotation; effort/velocity kept)"
                ));
            }
            if lim.effort.is_none()
                && lim.velocity.is_none()
                && lim.acceleration.is_none()
                && lim.jerk.is_none()
                && lim.deceleration.is_none()
            {
                j.limit = None;
            }
        }
    }
    // The SECOND DOF (universal/revolute2). Its own <limit>/<dynamics> now land in the typed
    // limit2/dynamics2 slots (previously dropped with a "single slot" note).
    if let Some(axis2) = jel.find("axis2") {
        if let Some(xel) = axis2.find("xyz") {
            j.axis2 = axis_in_child_frame(xel, "axis2", &fctx, &ctx, notes);
        }
        j.limit2 = axis_limit(axis2);
        j.dynamics2 = axis_dynamics(axis2);
    }
    // Screw pitch: canonical HCDF storage is meters-per-revolution, RIGHT-handed (the modern gz
    // `<screw_thread_pitch>`), stored with @pitch_convention/@handedness omitted (they default to the
    // canonical reading). A modern `<screw_thread_pitch>` is already canonical -> keep verbatim. A
    // legacy gazebo-classic `<thread_pitch>` is radians-per-meter, +ve LEFT-handed -> CONVERT it to
    // canonical (screw_thread_pitch = -2*pi / thread_pitch, which flips the sign as the handedness
    // convention flips) so every consumer reads one convention, and record the conversion.
    if let Some(tp) = jel.child_txt("screw_thread_pitch") {
        j.thread_pitch = Some(tp.to_string());
    } else if let Some(tp) = jel.child_txt("thread_pitch") {
        match parse_f(tp) {
            Some(classic) if classic != 0.0 => {
                let canonical = -std::f64::consts::TAU / classic;
                j.thread_pitch = Some(fmt_num(canonical));
                notes.push(format!(
                    "{ctx}: converted legacy gazebo-classic thread_pitch={classic} (rad/m, left-handed) to canonical @thread_pitch={} (m/rev, right-handed)",
                    fmt_num(canonical)
                ));
            }
            _ => {
                // Unparseable or zero (degenerate); keep verbatim rather than divide by zero.
                j.thread_pitch = Some(tp.to_string());
                notes.push(format!(
                    "{ctx}: legacy thread_pitch {} kept verbatim (not a nonzero number; no rad/m->m/rev conversion applied)",
                    repr_str(tp)
                ));
            }
        }
    }
    // A screw's SDF axis <limit> bounds the ROTATIONAL DOF (radians); the coupled axial translation
    // derives via thread_pitch. HCDF reads that <limit> as radians (correct by convention); note it.
    if hcdf_type == "screw"
        && j.limit
            .as_ref()
            .is_some_and(|l| l.lower.is_some() || l.upper.is_some())
    {
        notes.push(format!(
            "{ctx}: screw axis <limit> imported as the ROTATIONAL bound (radians); the coupled axial translation derives via thread_pitch"
        ));
    }
    // <mimic> may sit under <joint> (lxml first) or under <axis>.
    let mim = jel
        .find("mimic")
        .or_else(|| jel.find("axis").and_then(|a| a.find("mimic")));
    if let Some(mim) = mim {
        j.mimic = Some(Mimic {
            joint: mim
                .get("joint")
                .map(str::to_string)
                .or_else(|| mim.child_txt("joint").map(str::to_string)),
            multiplier: mim.child_txt("multiplier").map(str::to_string),
            offset: mim.child_txt("offset").map(str::to_string),
        });
    }
    Ok(j)
}

/// Map an SDF `<joint type="gearbox">` to a typed HCDF `<transmission type="gear">` coupling.
///
/// An SDF gearbox is a GEAR-COUPLING CONSTRAINT, not a kinematic DOF: it gears its driven side against a
/// `<gearbox_reference_body>` by `<gearbox_ratio>`. That is exactly the HCDF gear transmission: two
/// `<joint>` endpoints (`role="driven"` + `role="reference"`) geared by `<reduction>`, so the coupling
/// is preserved TYPED here rather than downgraded to a `fixed` joint (this SUPERSEDES the interim
/// gearbox->fixed mapping). The mapping:
///   * `<gearbox_ratio>`            -> `<reduction>`;
///   * the joint's own `<child>`    -> the `role="driven"` endpoint (the geared member);
///   * `<gearbox_reference_body>`   -> the `role="reference"` endpoint (what the driven member is geared
///     against);
///   * the transmission `@name`     -> the gearbox joint's name (so it round-trips to the `<joint>` name).
///
/// The gearbox joint's `<parent>` link is the coupling's implicit driving side; a 2-endpoint gear
/// transmission has no slot for it, so it is recorded as a loss (the driven child + reference body +
/// ratio + name are what round-trip through [`crate::to_sdf`] back to an SDF gearbox joint).
fn gearbox_transmission(jel: &SdfEl, notes: &mut Vec<String>) -> Transmission {
    let name = jel.get("name").map(str::to_string);
    let ctx = format!("joint {}", repr_opt(name.as_deref()));
    let child = jel.child_txt("child");
    let parent = jel.child_txt("parent");
    let ratio = jel.child_txt("gearbox_ratio");
    let reference = jel.child_txt("gearbox_reference_body");

    let mut tr = Transmission {
        // A `<transmission>` @name is required by the schema; synthesize one for the (unusual) nameless
        // gearbox joint so the emitted HCDF stays valid.
        name: Some(name.clone().unwrap_or_else(|| {
            notes.push(
                "an SDF gearbox <joint> has no name; the mapped <transmission> name is synthesized ('gearbox')".to_string(),
            );
            "gearbox".to_string()
        })),
        type_: Some(TransmissionType::Gear),
        reduction: ratio.map(str::to_string),
        ..Default::default()
    };
    // driven endpoint: the gearbox joint's own child link, the member driven through the gear.
    if let Some(c) = child {
        tr.joint.push(Endpoint {
            ref_: Some(c.to_string()),
            role: Some("driven".to_string()),
        });
    }
    // reference endpoint: the SDF <gearbox_reference_body> the driven member is geared against.
    if let Some(b) = reference {
        tr.joint.push(Endpoint {
            ref_: Some(b.to_string()),
            role: Some("reference".to_string()),
        });
    }
    notes.push(format!(
        "{ctx}: SDF 'gearbox' mapped to a <transmission type=\"gear\"> coupling{}: driven child {} geared against reference body {}; a gearbox is a gear-coupling constraint, not a kinematic DOF",
        ratio.map(|r| format!(" (reduction={r})")).unwrap_or_default(),
        repr_opt(child),
        repr_opt(reference),
    ));
    if let Some(p) = parent {
        notes.push(format!(
            "{ctx}: gearbox parent link {} is the coupling's implicit driving side; a 2-endpoint gear <transmission> carries the driven child + reference body + ratio, so the parent link is not separately represented",
            repr_str(p)
        ));
    }
    tr
}

// ── entry points ──────────────────────────────────────────────────────────────────────────────────

/// Import a RAW SDF document (XML string) into an [`Hcdf`] model, returning `(doc, notes)`.
///
/// Mirrors `from_sdf.py`'s tolerant lxml `recover=True` walk: the mapping is driven from a quick-xml
/// element tree with NO strict structural gate, so an SDF that omits spec-defaults / `name=` attributes /
/// carries `model://` uris imports just as it does under Python (the gz subprocess is never invoked). The
/// quick-xml tree preserves attribute/text and element presence so values match the Python oracle exactly.
/// (The only hard error here is a non-well-formed XML document or a missing `<model>`, both of which
/// Python also errors on.)
///
/// This is the thin back-compat wrapper around [`from_sdf_str_with_assets`]: it DISCARDS the per-visual
/// [`VisualAssetHint`]s, so its return + behaviour are byte-for-byte unchanged for existing callers.
pub fn from_sdf_str(src: &str) -> Result<(Hcdf, Vec<String>)> {
    let (doc, notes, _hints) = from_sdf_str_with_assets(src)?;
    Ok((doc, notes))
}

/// Like [`from_sdf_str`], but ALSO returns one [`VisualAssetHint`] per ARM-A (mesh) `<visual>`: the
/// already-resolved per-visual mesh `scale` (the `<mesh><scale>` text), flat material `color` (the
/// `<material><diffuse>` text) and albedo `texture` uri (the `<material><pbr><albedo_map>`), mirroring
/// the URDF importer's `from_urdf_str_with_assets`, plus one TEXTURE-only hint per PRIMITIVE `<visual>`
/// whose material carries an `<albedo_map>` (a textured `<plane>` maps to a zero-thickness `<box>` so
/// the texture has a geometry to bake onto; the asset step synthesizes the UV-mapped GLB). The [`Hcdf`] /
/// [`Visual`] / [`ModelRef`] model is IDENTICAL to what [`from_sdf_str`] produces for every untextured
/// visual (a visual `<model>` is still a clean GLB `uri`+`sha` with no scale/colour/texture fields); the
/// hints are a SEPARATE side-channel a baker consumes to fold the magnitude + mirror scale, the flat
/// colour and the texture into the baked GLB; the scale/colour pair being exactly the constructs
/// `from_sdf.py` feeds its import-time `baker(mesh_uri, scale, "visual", color)` hook. (`from_sdf`
/// itself never bakes, matching `from_sdf.py` with `baker=None`; the loss notes describing the deferred
/// bake are unchanged.)
pub fn from_sdf_str_with_assets(src: &str) -> Result<(Hcdf, Vec<String>, Vec<VisualAssetHint>)> {
    let root = parse_dom(src)?;
    if root.tag != "sdf" {
        return Err(Error::Xml(format!(
            "not an SDF document (root <{}>)",
            root.tag
        )));
    }
    let mut notes: Vec<String> = Vec::new();
    let mut hints: Vec<VisualAssetHint> = Vec::new();

    if let Some(ver) = root.get("version") {
        if ver != SDF_VERSION {
            notes.push(format!(
                "SDF version {} != pinned {}; mapping may be imperfect",
                repr_str(ver),
                repr_str(SDF_VERSION)
            ));
        }
    }
    let worlds: Vec<&SdfEl> = root.find_all("world").collect();
    if !worlds.is_empty() {
        notes.push(format!(
            "document has {} <world>(s); world/sim composition is out of HCDF-core scope; only the model is imported",
            worlds.len()
        ));
    }
    // top-level <model>s, else models nested in <world>s (mirrors from_sdf.py's `or` chain).
    let mut models: Vec<&SdfEl> = root.find_all("model").collect();
    if models.is_empty() {
        models = worlds.iter().flat_map(|w| w.find_all("model")).collect();
    }
    let model = *models
        .first()
        .ok_or_else(|| Error::Xml("SDF document has no <model>".to_string()))?;
    if models.len() > 1 {
        notes.push(format!(
            "{} top-level <model>s; only the first ({}) is imported (multi-model is out of HCDF-core scope)",
            models.len(),
            repr_opt(models[0].get("name"))
        ));
    }

    let doc_name = model.get("name").map(str::to_string);
    let mut doc = Hcdf {
        name: doc_name.clone().unwrap_or_default(),
        // Leave @version UNSET like the URDF path / Python `from_sdf` (which never stamps a version on
        // the imported doc); the schema treats an absent @version as the implicit current version.
        version: String::new(),
        body_frame: Some(BodyFrame::FLU), // SDF is FLU/ENU; no transform
        world_frame: Some(WorldFrame::ENU),
        ..Default::default()
    };

    let dname_repr = repr_opt(doc_name.as_deref());
    for sub in model.find_all("model") {
        notes.push(format!(
            "model {dname_repr}: nested <model {}> not represented (nested models are out of HCDF-core scope)",
            repr_opt(sub.get("name"))
        ));
    }
    // `<plugin>` is NO LONGER dropped here: model-scope (and world-scope) plugins are re-homed into the
    // typed `org.gazebosim` extension below. `<static>`/`<self_collide>` are NO
    // LONGER dropped either: they round-trip through the gazebo <model-physics> extension.
    for tag in ["light", "gripper"] {
        let n = model.find_all(tag).count();
        if n > 0 {
            notes.push(format!(
                "model {dname_repr}: {n} <{tag}> not represented (SDF-only / out of HCDF-core scope)"
            ));
        }
    }

    // Resolve the SDF 1.7 pose frame graph (every link/joint/<frame> pose + relative_to becomes
    // X(model←frame)), then emit comps + joints against it: joint origins become parent-relative child
    // transforms, and comps stay pose-free per the HCDF schema (placement lives on joints).
    let graph = FrameGraph::build(model, &mut notes);
    let child_set: BTreeSet<&str> = model
        .find_all("joint")
        .filter_map(|jel| jel.child_txt("child"))
        .collect();
    for lel in model.find_all("link") {
        let comp = link(lel, &mut notes, &mut hints);
        // A ROOT (or disconnected) comp with a non-identity model-frame pose is the one link pose no
        // joint origin can carry (HCDF's root has no pose), so that offset alone is dropped + noted.
        if !child_set.contains(comp.name.as_str())
            && graph
                .xform(&comp.name)
                .is_some_and(|x| !x.is_identity(1e-12))
        {
            notes.push(format!(
                "link {}: root-comp <pose> in the model frame not represented (HCDF root has no pose); dropped",
                repr_str(&comp.name)
            ));
        }
        doc.comp.push(comp);
    }
    // Child links that a NON-gearbox joint already parents; a gearbox that is the ONLY thing parenting
    // its child would otherwise leave that link unparented once it becomes a (kinematic-DOF-less)
    // transmission (the dangling-child guard below).
    let kinematic_parents: BTreeSet<&str> = model
        .find_all("joint")
        .filter(|jel| jel.get("type") != Some("gearbox"))
        .filter_map(|jel| jel.child_txt("child"))
        .collect();
    for jel in model.find_all("joint") {
        if jel.get("type") == Some("gearbox") {
            // SDF gearbox is a gear-COUPLING constraint, not a kinematic DOF: map it to a typed
            // <transmission type="gear"> (superseding the interim gearbox->fixed downgrade).
            doc.transmission.push(gearbox_transmission(jel, &mut notes));
            // Dangling-child guard: if NOTHING else parents this gearbox's child link, a transmission
            // alone would orphan the link (a transmission carries no kinematic edge). Emit a fixed joint
            // for that edge too, renamed so it does not collide with the same-named gear <transmission>
            // when [`crate::to_sdf`] round-trips the transmission back to a gearbox <joint>.
            let orphaned = jel
                .child_txt("child")
                .is_some_and(|c| !kinematic_parents.contains(c));
            if orphaned {
                let mut edge = joint(jel, &mut notes, &graph)?; // gearbox -> fixed edge here
                let base = edge.name.clone();
                edge.name = Some(match &base {
                    Some(n) => format!("{n}_fixed"),
                    None => "gearbox_fixed".to_string(),
                });
                notes.push(format!(
                    "joint {}: gearbox child link {} has no other parent joint; a fixed joint {} carries the kinematic edge alongside the gear <transmission>",
                    repr_opt(base.as_deref()),
                    repr_opt(jel.child_txt("child")),
                    repr_opt(edge.name.as_deref()),
                ));
                doc.joint.push(edge);
            }
            continue;
        }
        doc.joint.push(joint(jel, &mut notes, &graph)?);
    }
    for fel in model.find_all("frame") {
        map_model_frame(fel, &graph, &mut doc.comp, &mut notes, &dname_repr);
    }
    // Preserve the model/world Gazebo simulation config (plugins + physics) in ONE typed
    // `<extension domain="org.gazebosim"><gazebo-sim>…` rather than dropping it.
    if let Some(ext) = gazebo_sim_extension(&root, model, &mut notes) {
        doc.extension.push(ext);
    }
    // Capture each contact sensor's <topic> into the org.ros2 topic-map extension (middleware/
    // runtime home; the monitored <collision> is already on the typed sensor). This lets a contact
    // sensor round-trip to VALID SDF (the exporter re-emits <contact><collision>+<topic>).
    if let Some(ext) = ros2_topic_extension(model, &mut notes) {
        doc.extension.push(ext);
    }
    // Capture each model-scope <include> (a nested sub-assembly reference) into the typed
    // doc-level include home. The reference is CARRIED verbatim (uri/name/pose/static/placement_frame),
    // NOT resolved/flattened; a caller that wants composition runs `gz sdf --print` first (mirrors the
    // module contract). Previously every <include> silently vanished (no find_all("include") anywhere).
    for iel in model.find_all("include") {
        doc.include.push(sdf_include(iel, &mut notes, &dname_repr));
    }
    Ok((doc, notes, hints))
}

/// Map an SDF model-scope `<include>` (uri/name/pose/static/placement_frame child elements) to the typed
/// HCDF [`Include`] reference. The `@merge` attribute and any nested `<model>` override are SDF
/// composition directives with no HCDF include field, noted, never silently dropped. `@sha` stays unset
/// (HCDF-only integrity hash; SDF has no source for it).
fn sdf_include(iel: &SdfEl, notes: &mut Vec<String>, dname_repr: &str) -> Include {
    let uri = iel.child_txt("uri");
    if iel.get("merge") == Some("true") {
        notes.push(format!(
            "model {dname_repr}: <include {}> @merge=true not represented (merge-composition is an SDF-only directive)",
            repr_opt(uri)
        ));
    }
    Include {
        uri: uri.map(str::to_string),
        name: iel.child_txt("name").map(str::to_string),
        pose: iel.child_txt("pose").map(str::to_string),
        static_: iel.child_txt("static").map(str::to_string),
        placement_frame: iel.child_txt("placement_frame").map(str::to_string),
        ..Default::default()
    }
}

/// Collect the `<topic>` of every `<link><sensor type="contact">` into ONE typed org.ros2 topic-map
/// extension body (`<topic-map><topic sensor= name=/>…`), so the contact sensor's runtime topic
/// round-trips (its monitored `<collision>` is already on the typed sensor). Returns `None` when no
/// contact sensor carries a topic. The message `type` is intentionally omitted; this is a raw SDF
/// topic with no ROS 2 message binding yet (the `topic/@type` relaxation in hcdf-ext-ros2.xsd).
fn ros2_topic_extension(model: &SdfEl, notes: &mut Vec<String>) -> Option<Extension> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for lel in model.find_all("link") {
        for sel in lel.find_all("sensor") {
            if sel.get("type") != Some("contact") {
                continue;
            }
            let Some(topic) = sel.find("contact").and_then(|c| c.child_txt("topic")) else {
                continue;
            };
            let name = sel.get("name").unwrap_or("").to_string();
            entries.push((name, topic.to_string()));
        }
    }
    if entries.is_empty() {
        return None;
    }
    let mut body = format!("<{ROS2_TOPIC_MAP_ROOT}>");
    for (name, topic) in &entries {
        body.push_str("<topic sensor=\"");
        xml_escape_into(name, true, &mut body);
        body.push_str("\" name=\"");
        xml_escape_into(topic, true, &mut body);
        body.push_str("\"/>");
    }
    body.push_str(&format!("</{ROS2_TOPIC_MAP_ROOT}>"));
    notes.push(format!(
        "{} contact-sensor <topic>(s) preserved in <extension domain=\"{ROS2_DOMAIN}\">",
        entries.len()
    ));
    Some(Extension {
        domain: ROS2_DOMAIN.to_string(),
        body,
        ..Default::default()
    })
}

/// Collect a model/world's Gazebo simulation config into ONE typed `<gazebo-sim>` extension body
/// (schema `extensions/hcdf-ext-gazebo.xsd`, domain [`GAZEBO_DOMAIN`]) so sim plugins and world physics
/// survive import instead of being dropped:
///   * the world `<physics>` (first one; the schema permits at most one) -> a typed `<physics>`:
///     `@type` -> `<engine>`, `<max_step_size>` -> `<max-step-size>`, `<real_time_factor>` ->
///     `<real-time-factor>` (see [`gazebo_physics_body`]);
///   * every world-scope and model-scope `<plugin>` -> a typed `<plugin name= filename=>`, its subtree
///     preserved verbatim under the schema's lax `xs:any`.
///
/// Deeper-scoped plugins (link/sensor/joint) keep their existing handling. Returns `None` when the model
/// and its worlds carry neither a `<plugin>` nor a `<physics>` (no extension is emitted).
fn gazebo_sim_extension(root: &SdfEl, model: &SdfEl, notes: &mut Vec<String>) -> Option<Extension> {
    let worlds: Vec<&SdfEl> = root.find_all("world").collect();
    // physics: the first <physics> across worlds (the typed schema allows at most one per block).
    let physics = worlds.iter().find_map(|w| w.find("physics"));
    // plugins: world-scope first, then model-scope, in document order.
    let mut plugins: Vec<&SdfEl> = worlds.iter().flat_map(|w| w.find_all("plugin")).collect();
    plugins.extend(model.find_all("plugin"));
    // Model/link simulation scalars (static, self_collide, gravity, kinematic, velocity_decay, …)
    // have no cyber-physical-core home; round-trip them through the typed gazebo extension.
    let mut model_physics = String::new();
    let has_model_physics = gazebo_model_physics_body(model, &mut model_physics);
    let mut link_physics = String::new();
    let mut n_link_physics = 0usize;
    for lel in model.find_all("link") {
        if gazebo_link_physics_body(lel, &mut link_physics) {
            n_link_physics += 1;
        }
    }
    if physics.is_none() && plugins.is_empty() && !has_model_physics && n_link_physics == 0 {
        return None;
    }
    let mut body = format!("<{GAZEBO_SIM_ROOT}>");
    if let Some(p) = physics {
        gazebo_physics_body(p, notes, &mut body);
    }
    for pl in &plugins {
        el_to_xml(pl, &mut body);
    }
    // Schema order: physics, plugin*, model-physics?, link-physics*.
    body.push_str(&model_physics);
    body.push_str(&link_physics);
    body.push_str(&format!("</{GAZEBO_SIM_ROOT}>"));
    notes.push(format!(
        "Gazebo sim config preserved in <extension domain=\"{GAZEBO_DOMAIN}\">: {} <plugin>(s){}{}{}",
        plugins.len(),
        if physics.is_some() { " + <physics>" } else { "" },
        if has_model_physics { " + <model-physics>" } else { "" },
        if n_link_physics > 0 {
            format!(" + {n_link_physics} <link-physics>")
        } else {
            String::new()
        },
    ));
    Some(Extension {
        domain: GAZEBO_DOMAIN.to_string(),
        body,
        ..Default::default()
    })
}

/// Harvest a model's simulation scalars into a `<model-physics>` body appended to `out`. Model
/// CHILD elements `<static>/<self_collide>/<allow_auto_disable>/<enable_wind>` and the `@canonical_link`
/// ATTRIBUTE map to the typed (kebab-case) element names. Returns whether anything was appended.
fn gazebo_model_physics_body(model: &SdfEl, out: &mut String) -> bool {
    // (SDF child element, typed element name)
    const FLAGS: [(&str, &str); 4] = [
        ("static", "static"),
        ("self_collide", "self-collide"),
        ("allow_auto_disable", "allow-auto-disable"),
        ("enable_wind", "enable-wind"),
    ];
    let mut inner = String::new();
    for (sdf, typed) in FLAGS {
        if let Some(v) = model.child_txt(sdf) {
            inner.push_str(&format!("<{typed}>"));
            xml_escape_into(v, false, &mut inner);
            inner.push_str(&format!("</{typed}>"));
        }
    }
    if let Some(cl) = model.get("canonical_link").filter(|s| !s.is_empty()) {
        inner.push_str("<canonical-link>");
        xml_escape_into(cl, false, &mut inner);
        inner.push_str("</canonical-link>");
    }
    if inner.is_empty() {
        return false;
    }
    out.push_str("<model-physics>");
    out.push_str(&inner);
    out.push_str("</model-physics>");
    true
}

/// Harvest one link's simulation scalars into a `<link-physics name=...>` body appended to `out`.
/// The link CHILD flags `<gravity>/<self_collide>/<kinematic>/<must_be_base_link>` and the
/// `<velocity_decay><linear>/<angular>` block map to the typed element names. Returns whether anything
/// was appended (an entry is emitted only when the link carries at least one sim scalar).
fn gazebo_link_physics_body(lel: &SdfEl, out: &mut String) -> bool {
    const FLAGS: [(&str, &str); 4] = [
        ("gravity", "gravity"),
        ("self_collide", "self-collide"),
        ("kinematic", "kinematic"),
        ("must_be_base_link", "must-be-base-link"),
    ];
    let mut inner = String::new();
    for (sdf, typed) in FLAGS {
        if let Some(v) = lel.child_txt(sdf) {
            inner.push_str(&format!("<{typed}>"));
            xml_escape_into(v, false, &mut inner);
            inner.push_str(&format!("</{typed}>"));
        }
    }
    if let Some(vd) = lel.find("velocity_decay") {
        let linear = vd.child_txt("linear");
        let angular = vd.child_txt("angular");
        if linear.is_some() || angular.is_some() {
            inner.push_str("<velocity-decay>");
            if let Some(l) = linear {
                inner.push_str("<linear>");
                xml_escape_into(l, false, &mut inner);
                inner.push_str("</linear>");
            }
            if let Some(a) = angular {
                inner.push_str("<angular>");
                xml_escape_into(a, false, &mut inner);
                inner.push_str("</angular>");
            }
            inner.push_str("</velocity-decay>");
        }
    }
    if inner.is_empty() {
        return false;
    }
    out.push_str("<link-physics name=\"");
    xml_escape_into(lel.get("name").unwrap_or(""), true, out);
    out.push_str("\">");
    out.push_str(&inner);
    out.push_str("</link-physics>");
    true
}

/// Translate an SDF `<physics type=...>` into the typed `<physics>` body appended to `out`: `@type` ->
/// `<engine>` (only for a value the schema's `PhysicsEngine` enumerates), `<max_step_size>` ->
/// `<max-step-size>`, `<real_time_factor>` -> `<real-time-factor>`. Other `<physics>` children (solver
/// configs, `max_contacts`, …) have no typed home and are noted, never silently dropped.
fn gazebo_physics_body(p: &SdfEl, notes: &mut Vec<String>, out: &mut String) {
    out.push_str("<physics>");
    if let Some(engine) = p.get("type") {
        if PHYSICS_ENGINES.contains(&engine) {
            out.push_str("<engine>");
            xml_escape_into(engine, false, out);
            out.push_str("</engine>");
        } else {
            notes.push(format!(
                "<physics type={}> is not a typed engine (ode/bullet/dart/simbody/tpe); <engine> left unset",
                repr_str(engine)
            ));
        }
    }
    if let Some(mss) = p.child_txt("max_step_size") {
        out.push_str("<max-step-size>");
        xml_escape_into(mss, false, out);
        out.push_str("</max-step-size>");
    }
    if let Some(rtf) = p.child_txt("real_time_factor") {
        out.push_str("<real-time-factor>");
        xml_escape_into(rtf, false, out);
        out.push_str("</real-time-factor>");
    }
    out.push_str("</physics>");
    note_residual(
        p,
        &["max_step_size", "real_time_factor"],
        notes,
        "<physics>",
        "physics",
    );
}

/// Map one model-level SDF `<frame>` onto the `<frame>` list of the comp owning the link its
/// `attached_to` chain bottoms out at, posed comp-local (X(comp←frame), the HCDF `<frame>` default
/// frame). The pose is emitted when non-identity or when the SDF frame carried a `<pose>` element
/// (presence-preserving). An unattachable frame (dangling/cyclic chain) keeps a drop note.
fn map_model_frame(
    fel: &SdfEl,
    graph: &FrameGraph,
    comps: &mut [Comp],
    notes: &mut Vec<String>,
    dname_repr: &str,
) {
    let fname = fel.get("name").unwrap_or("");
    let placed = graph.owner_link(fname).and_then(|owner| {
        let rel = graph.relative(&owner, fname)?;
        let comp = comps.iter_mut().find(|c| c.name == owner)?;
        comp.frame.push(Frame {
            name: fname.to_string(),
            relative_to: None,
            type_: None,
            description: fel.child_txt("description").map(str::to_string),
            pose: (fel.find("pose").is_some() || !rel.is_identity(1e-12)).then(|| rel.to_pose()),
        });
        Some(())
    });
    if placed.is_none() {
        notes.push(format!(
            "model {dname_repr}: <frame {}> not attached to a link (dangling or cyclic attached_to); dropped",
            repr_opt(fel.get("name"))
        ));
    }
}

/// Import an SDF FILE into an [`Hcdf`] model, returning `(doc, notes)`.
///
/// Mirrors `from_sdf.py` exactly: read the file and map the RAW SDF; `gz` is NEVER invoked, `model://`
/// is preserved verbatim, an unresolved `<include>` is left unresolved (its inline `<model>` is mapped),
/// and a default-omitting / unnamed-element SDF still imports. A caller that wants `<include>`/`model://`/
/// Fuel resolved or spec-defaults filled FIRST runs `gz sdf --print` externally and feeds the canonical
/// output to [`from_sdf_str`]: an explicit external pre-pass, never an automatic one. (This fn is
/// available on all targets, since it never touches `std::process`.)
pub fn from_sdf_path(path: &std::path::Path) -> Result<(Hcdf, Vec<String>)> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| Error::Xml(format!("{}: {e}", path.display())))?;
    from_sdf_str(&src)
}
