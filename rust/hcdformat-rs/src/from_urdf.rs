//! URDF -> HCDF import (feature = `urdf`), a faithful port of `urdf_hcdf/from_urdf.py`.
//!
//! Mapping spine: `<robot>` -> [`Hcdf`], `<link>` -> [`Comp`], `<joint>` -> [`Joint`], top-level
//! `<material>` -> `<color>`. URDF is always FLU/ENU, so the imported document is tagged
//! `body-frame="FLU" world-frame="ENU"` and needs no pose transform (frame conversion is an
//! *export*-side concern, see [`mod@crate::to_urdf`]).
//!
//! Import quarantine pass: every top-level `<robot>` child that is not a typed-core kind
//! (link / joint / material) is quarantined VERBATIM into a root [`Extension`] grouped by domain
//! (gazebo, transmission, ros2_control, unknown vendor tags), so non-URDF content survives the
//! round-trip without polluting the clean core. `urdf-rs`'s `Robot` struct has NO field for these
//! top-level blocks (it silently drops `<gazebo>`/`<transmission>`/`<ros2_control>`/vendor tags), so
//! they are captured by a thin SIBLING quick-xml pass over the raw URDF.
//!
//! ## Parser strategy
//! `urdf-rs` 0.9 is the URDF parser: pure-Rust / wasm-clean, with exact geometry /
//! topology / mesh / inertia / limit fidelity to the Python oracle. We run its
//! [`urdf_rs::read_from_string`] as a STRUCTURAL VALIDATION GATE (a malformed URDF that urdf-rs's
//! parser rejects is rejected here too), and drive the actual element mapping from a parallel
//! quick-xml element tree (`UrdfEl`). That tree preserves *exact attribute text* (URDF `"0.0"`
//! vs urdf-rs's `f64` round-trip `"0"`) and *element presence* (urdf-rs always materializes
//! `<inertial>`/`<axis>`/`<limit>`/`<origin>` with defaults, erasing whether the source had them),
//! both required for byte-semantic parity with `from_urdf.py`'s lxml `.get()` / drop-when-absent walk.
//! (`UrdfEl` is the internal quick-xml element tree the mapping reads.)
//!
//! ## Lenient pre-pass (real-world tolerance)
//! Three quirks that appear in real published URDFs are hard-rejected by `urdf-rs` but tolerated by the
//! wider toolchain (RViz, Gazebo). A surgical quick-xml pre-pass (`sanitize_urdf`) fixes them in the
//! raw text BEFORE the strict gate, each with a conversion note: a top-level `<joint>` with no `type`
//! attribute gets `type="fixed"` injected (RViz's behaviour); a duplicate `<material>` directly inside
//! a `<visual>` is dropped (an exporter bug; the first one wins); and a nameless inline `<material>`
//! (whose `@name` the URDF XSD makes optional but urdf-rs requires) gets a deterministic synthesized
//! name injected (Unitree h1_2). A quirk-free document passes through zero-copy and byte-identical;
//! when a fix applies, every byte outside the edited spans is untouched (no reformatting, no other
//! rewrites).
//!
//! ## Known losses (recorded in the returned notes; a structured manifest is in profile)
//!   * a URDF visual mesh becomes a GLB `<model>` placeholder (uri = the source mesh); the actual GLB
//!     bake + `@sha` is the asset pipeline ([`crate::bake`]). A `<material>` on a mesh visual never
//!     lands in the HCDF document: its flat colour (inline `rgba`, or a named reference resolved
//!     through the robot-level palette) AND its diffuse `<texture filename>` (inline, or resolved by
//!     name through the robot-level palette) ride the [`VisualAssetHint`] side-channel and bake into
//!     the GLB at the asset step; a material with neither a resolvable flat colour nor a texture is
//!     dropped with a note.
//!   * a `<texture>` on a PRIMITIVE visual's material (no GLB to bake into) is still dropped (noted).
//!   * xacro is assumed already expanded; the separate pure-Rust `xacro` feature provides
//!     filesystem-backed and caller-supplied in-memory expansion entry points.
use crate::asset_hint::VisualAssetHint;
use crate::error::{Error, Result};
use crate::model::enums::{
    BodyFrame, FrustumShape, JointType, NameOrigin, OpticalSensorType, TransmissionType, WorldFrame,
};
use crate::model::{
    Axis, Box_, CameraIntrinsics, Capsule, Collision, CollisionGeometry, Color, Comp, Cylinder,
    Endpoint, Extension, Frustum, Geometry, Hcdf, Inertial, Joint, JointCalibration, JointDynamics,
    JointEndpoint, JointLimit, LidarParams, LidarScanAxis, LidarScanPattern, Mesh, Mimic, ModelRef,
    OpticalSensor, Pose, SafetyController, Sensor, SensorFov, Sphere, Transmission, UrdfCompatComp,
    UrdfCompatJoint, Visual, VisualAppearance, VisualGeometry,
};
use crate::pyrepr::repr_str;
// The ros2_control re-homing (domain + root-element rename) lives in `to_urdf` (the module compiled
// whenever either converter is enabled), so `to_urdf` can apply the inverse without depending on
// `from_urdf` (which the wasm/SDF-only build omits).
use crate::to_urdf::{
    rename_root_element, GAZEBO_RAW_DOMAIN, ROS2_CONTROL_DOMAIN, ROS2_CONTROL_TYPED_ROOT,
    ROS2_CONTROL_URDF_ROOT,
};
use std::collections::BTreeMap;
use std::str::FromStr;

/// URDF top-level children that map to the typed core; everything else is quarantined.
const TYPED_TOPLEVEL: &[&str] = &["link", "joint", "material"];

/// URDF top-level element local-name -> HCDF extension domain (all others -> `urdf:<tag>`).
///
/// `<transmission>` is deliberately absent: it is imported into the TYPED core `<transmission>`
/// ([`extract_transmissions`]), not quarantined. A pathological nameless `<transmission>` that the
/// typed pass cannot adopt falls through to the honest `urdf:transmission` catch-all rather than the
/// former `org.ros.control` blob.
fn domain_for(tag: &str) -> String {
    match tag {
        // A verbatim `<gazebo reference=...>` residual has no typed home (it mixes plugin/material/
        // un-typed-sensor content that does not fit the `<gazebo-sim>` schema), so it is quarantined
        // OPAQUE under `org.gazebosim.raw`, NOT the typed `org.gazebosim` domain, whose body must be a
        // `<gazebo-sim>` root (raw `<gazebo>` bytes cannot validate against
        // hcdf-ext-gazebo.xsd). `to_urdf` re-emits the raw body verbatim, so the URDF round-trip holds.
        "gazebo" => GAZEBO_RAW_DOMAIN.to_string(),
        "ros2_control" => ROS2_CONTROL_DOMAIN.to_string(),
        other => format!("urdf:{other}"),
    }
}

/// URDF `<transmission><type>` text -> HCDF [`TransmissionType`]. Accepts the ros_control class paths
/// (`transmission_interface/SimpleTransmission`, `.../DifferentialTransmission`) by their leaf name and,
/// as a convenience, a bare HCDF enum literal (`simple`, `differential`, …). Anything else (e.g.
/// `transmission_interface/FourBarLinkageTransmission`, which HCDF has no value for) yields `None` so the
/// caller leaves `@type` unset and records a note.
fn map_transmission_type(text: &str) -> Option<TransmissionType> {
    let leaf = text.rsplit('/').next().unwrap_or(text);
    match leaf {
        "SimpleTransmission" => Some(TransmissionType::Simple),
        "DifferentialTransmission" => Some(TransmissionType::Differential),
        _ => TransmissionType::from_str(text).ok(),
    }
}

/// URDF joint type literal -> HCDF [`JointType`] value. `floating` renames to `free` (6-DOF) and
/// `spherical` to `ball` (URDF's name for a 3-DOF spherical joint; HCDF has no `spherical` literal);
/// the remaining URDF types (revolute/continuous/prismatic/fixed/planar) are 1:1.
fn map_joint_type(utype: &str) -> &str {
    match utype {
        "floating" => "free",
        "spherical" => "ball",
        other => other,
    }
}

// ── a minimal exact-text quick-xml DOM (mirrors lxml's element access) ──────────────────────────

/// A parsed XML element preserving attributes as exact text and child order, the read surface the
/// Python importer uses through lxml `.get(...)` / child iteration.
struct UrdfEl {
    tag: String,
    attrs: BTreeMap<String, String>,
    children: Vec<UrdfEl>,
    /// Concatenated character content directly inside this element (lxml `.text`/`.itertext`). URDF
    /// leaves it empty for the attribute-only spine; it carries the value of text leaves like
    /// `<type>` and `<mechanicalReduction>` inside a `<transmission>`.
    text: String,
}

impl UrdfEl {
    fn get(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(|s| s.as_str())
    }
    /// This element's text content, whitespace-trimmed, or `None` when empty (lxml `.text` semantics).
    fn text_trim(&self) -> Option<&str> {
        let t = self.text.trim();
        (!t.is_empty()).then_some(t)
    }
    /// First direct child with the given local name (lxml `_find`).
    fn find(&self, name: &str) -> Option<&UrdfEl> {
        self.children.iter().find(|c| c.tag == name)
    }
    /// All direct children with the given local name (lxml `_findall`).
    fn find_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a UrdfEl> + 'a {
        self.children.iter().filter(move |c| c.tag == name)
    }
}

/// Parse the raw URDF into an [`UrdfEl`] tree, recording each element's attributes as exact text and
/// its raw serialization (for verbatim quarantine). Uses the same quick-xml dependency the document
/// reader does; no new dep, wasm-clean.
fn parse_dom(src: &str) -> Result<(UrdfEl, BTreeMap<usize, String>)> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(src);
    reader.config_mut().expand_empty_elements = false;
    let mut stack: Vec<UrdfEl> = Vec::new();
    // Per top-level child of <robot>, capture the verbatim raw XML (start byte .. end byte) for the
    // quarantine pass. We track byte spans of depth-1 (under root) elements.
    let mut roots: Vec<UrdfEl> = Vec::new();
    loop {
        let ev = reader.read_event().map_err(|e| Error::Xml(e.to_string()))?;
        match ev {
            Event::Start(e) => {
                let tag = local_name(&e);
                let attrs = read_attrs(&e)?;
                stack.push(UrdfEl {
                    tag,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                });
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    let txt = t.unescape().map_err(|e| Error::Xml(e.to_string()))?;
                    top.text.push_str(&txt);
                }
            }
            Event::Empty(e) => {
                let tag = local_name(&e);
                let attrs = read_attrs(&e)?;
                let el = UrdfEl {
                    tag,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                };
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(el);
                } else {
                    roots.push(el);
                }
            }
            Event::End(_) => {
                if let Some(done) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(done);
                    } else {
                        roots.push(done);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let root = roots
        .into_iter()
        .next()
        .ok_or_else(|| Error::Xml("empty URDF document".to_string()))?;
    // Capture verbatim raw bodies of each top-level child via a second targeted pass (so the quarantine
    // re-emits gazebo/ros2_control blocks unchanged, like the Python deepcopy).
    let raw = capture_toplevel_raw(src)?;
    Ok((root, raw))
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

/// The namespace prefix of a qualified XML name (`controller:gazebo` -> `Some("controller")`),
/// or `None` for an unprefixed name. The bytes are the raw element/attribute key.
fn prefix_of(qname: &[u8]) -> Option<&[u8]> {
    qname.iter().position(|&b| b == b':').map(|i| &qname[..i])
}

/// The namespace prefix an attribute *uses* (not one it declares): `None` for an unprefixed key or
/// for an `xmlns`/`xmlns:<p>` declaration (those declare a namespace rather than reference one).
fn attr_used_prefix(key: &[u8]) -> Option<&[u8]> {
    if key == b"xmlns" || key.starts_with(b"xmlns:") {
        return None;
    }
    prefix_of(key)
}

/// Inject, onto a top-level start element being quarantined, the in-scope `xmlns:<prefix>`
/// declarations (inherited from the `<robot>` root) for every prefix the element + its subtree
/// USE but do not declare locally. This mirrors lxml `copy.deepcopy(el)` + serialize, which emits
/// exactly the namespace declarations the subtree references (so prefixed content like pr2's
/// `<controller:gazebo_ros_controller_manager>` stays well-formed when reparsed standalone) and
/// nothing more (an unused root `xmlns:` is not copied, keeping byte-parity for prefix-free blocks).
fn with_inherited_ns(
    start: &quick_xml::events::BytesStart<'static>,
    used: &std::collections::BTreeSet<Vec<u8>>,
    root_ns: &BTreeMap<Vec<u8>, Vec<u8>>,
) -> Result<quick_xml::events::BytesStart<'static>> {
    use std::collections::BTreeSet;
    // Prefixes already declared locally on this element (`xmlns:<prefix>=...`) need no re-injection.
    let mut local: BTreeSet<Vec<u8>> = BTreeSet::new();
    for a in start.attributes() {
        let a = a.map_err(|e| Error::Xml(e.to_string()))?;
        if let Some(p) = a.key.as_ref().strip_prefix(b"xmlns:") {
            local.insert(p.to_vec());
        }
    }
    let mut out = start.clone();
    for prefix in used {
        if local.contains(prefix) {
            continue;
        }
        if let Some(uri) = root_ns.get(prefix) {
            let attr = format!("xmlns:{}", String::from_utf8_lossy(prefix));
            out.push_attribute((attr.as_str(), String::from_utf8_lossy(uri).as_ref()));
        }
    }
    Ok(out)
}

/// Capture the verbatim raw XML of each top-level child of `<robot>`, keyed by its document order
/// index among ALL top-level children. Mirrors lxml `copy.deepcopy` of the source element, including
/// its emission of the in-scope namespace declarations the subtree references (see [`with_inherited_ns`]).
fn capture_toplevel_raw(src: &str) -> Result<BTreeMap<usize, String>> {
    use quick_xml::events::{BytesEnd, Event};
    use quick_xml::reader::Reader;
    use quick_xml::writer::Writer;
    use std::collections::BTreeSet;
    use std::io::Cursor;
    let mut reader = Reader::from_str(src);
    reader.config_mut().expand_empty_elements = false;
    let mut out: BTreeMap<usize, String> = BTreeMap::new();
    let mut depth = 0usize;
    let mut idx = 0usize;
    // Namespace declarations on the <robot> root (`xmlns:<prefix>` -> uri), inherited by every
    // quarantined top-level child exactly as lxml's deepcopy would carry them down.
    let mut root_ns: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) => {
                if depth == 0 {
                    // the <robot> root: record its xmlns:<prefix> declarations for inheritance.
                    for a in e.attributes() {
                        let a = a.map_err(|e| Error::Xml(e.to_string()))?;
                        if let Some(p) = a.key.as_ref().strip_prefix(b"xmlns:") {
                            root_ns.insert(
                                p.to_vec(),
                                a.unescape_value()
                                    .map_err(|e| Error::Xml(e.to_string()))?
                                    .as_bytes()
                                    .to_vec(),
                            );
                        }
                    }
                    depth += 1;
                } else if depth == 1 {
                    // capture this element + subtree verbatim, tracking which namespace prefixes the
                    // subtree references (element names + attribute keys) so we can re-declare the
                    // inherited ones on the captured top-level element.
                    let name = e.name().as_ref().to_vec();
                    let mut used: BTreeSet<Vec<u8>> = BTreeSet::new();
                    let collect_prefixes = |bs: &quick_xml::events::BytesStart,
                                            used: &mut BTreeSet<Vec<u8>>|
                     -> Result<()> {
                        if let Some(p) = prefix_of(bs.name().as_ref()) {
                            used.insert(p.to_vec());
                        }
                        for a in bs.attributes() {
                            let a = a.map_err(|e| Error::Xml(e.to_string()))?;
                            if let Some(p) = attr_used_prefix(a.key.as_ref()) {
                                used.insert(p.to_vec());
                            }
                        }
                        Ok(())
                    };
                    let top = e.clone().into_owned();
                    collect_prefixes(&top, &mut used)?;
                    // First pass: write the subtree to a buffer while collecting used prefixes.
                    let mut w = Writer::new(Cursor::new(Vec::new()));
                    let mut inner_depth = 0usize;
                    loop {
                        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
                            Event::Start(s) => {
                                inner_depth += 1;
                                collect_prefixes(&s, &mut used)?;
                                w.write_event(Event::Start(s.into_owned()))
                                    .map_err(|e| Error::Xml(e.to_string()))?;
                            }
                            Event::Empty(s) => {
                                collect_prefixes(&s, &mut used)?;
                                w.write_event(Event::Empty(s.into_owned()))
                                    .map_err(|e| Error::Xml(e.to_string()))?;
                            }
                            Event::End(en) => {
                                if inner_depth == 0 && en.name().as_ref() == name.as_slice() {
                                    w.write_event(Event::End(BytesEnd::new(
                                        String::from_utf8_lossy(&name),
                                    )))
                                    .map_err(|e| Error::Xml(e.to_string()))?;
                                    break;
                                }
                                inner_depth = inner_depth.saturating_sub(1);
                                w.write_event(Event::End(en.into_owned()))
                                    .map_err(|e| Error::Xml(e.to_string()))?;
                            }
                            Event::Eof => break,
                            ev => w
                                .write_event(ev.into_owned())
                                .map_err(|e| Error::Xml(e.to_string()))?,
                        }
                    }
                    let subtree = w.into_inner().into_inner();
                    // Second: re-emit the (now ns-augmented) top-level start, then the captured subtree.
                    let top_ns = with_inherited_ns(&top, &used, &root_ns)?;
                    let mut tw = Writer::new(Cursor::new(Vec::new()));
                    tw.write_event(Event::Start(top_ns))
                        .map_err(|e| Error::Xml(e.to_string()))?;
                    let mut body = tw.into_inner().into_inner();
                    body.extend_from_slice(&subtree);
                    let body = String::from_utf8(body).map_err(|e| Error::Xml(e.to_string()))?;
                    out.insert(idx, body);
                    idx += 1;
                } else {
                    depth += 1;
                }
            }
            Event::Empty(e) => {
                if depth == 1 {
                    // a self-closing top-level child: inject inherited ns for any prefix it references.
                    let mut used: BTreeSet<Vec<u8>> = BTreeSet::new();
                    if let Some(p) = prefix_of(e.name().as_ref()) {
                        used.insert(p.to_vec());
                    }
                    for a in e.attributes() {
                        let a = a.map_err(|e| Error::Xml(e.to_string()))?;
                        if let Some(p) = attr_used_prefix(a.key.as_ref()) {
                            used.insert(p.to_vec());
                        }
                    }
                    let owned = e.into_owned();
                    let augmented = with_inherited_ns(&owned, &used, &root_ns)?;
                    let mut w = Writer::new(Cursor::new(Vec::new()));
                    w.write_event(Event::Empty(augmented))
                        .map_err(|e| Error::Xml(e.to_string()))?;
                    let body = String::from_utf8(w.into_inner().into_inner())
                        .map_err(|e| Error::Xml(e.to_string()))?;
                    out.insert(idx, body);
                    idx += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

// ── poses & geometry ────────────────────────────────────────────────────────────────────────────

/// Build an HCDF [`Pose`] from a URDF `<origin xyz= rpy=>`, preserving presence (None if absent).
fn origin_to_pose(org: Option<&UrdfEl>) -> Option<Pose> {
    let org = org?;
    // Preserve attribute presence (matches Python `_origin_to_pose`, which sets `p.xyz`/`p.rpy` only
    // when the source `<origin>` carries that attribute), so an xyz-only origin round-trips without a
    // fabricated `rpy="0 0 0"`.
    // `quat_xyzw` is the official urdfdom v1.2 pose attribute (urdf.xsd:31, default "0 0 0 1"); it maps
    // directly onto HCDF `pose/@quat` (hcdf.xsd:575); both are scalar-last "x y z w". Presence-preserving
    // like xyz/rpy: no quat is fabricated when the source `<origin>` omits the attribute.
    Some(Pose {
        xyz: org.get("xyz").map(parse3),
        rpy: org.get("rpy").map(parse3),
        quat: org.get("quat_xyzw").map(parse4),
    })
}

fn parse3(s: &str) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (i, t) in s.split_whitespace().take(3).enumerate() {
        out[i] = t.parse().unwrap_or(0.0);
    }
    out
}

fn parse4(s: &str) -> [f64; 4] {
    let mut out = [0.0; 4];
    for (i, t) in s.split_whitespace().take(4).enumerate() {
        out[i] = t.parse().unwrap_or(0.0);
    }
    out
}

/// Fill a [`VisualGeometry`] / [`CollisionGeometry`] from a URDF `<geometry>`'s primitive. Returns
/// the matched primitive, or `None` if there is no box/cylinder/sphere/capsule. `<capsule>` uses the
/// same `radius`/`length` attribute pair as `<cylinder>`; urdf-rs 0.9 accepts it, so it passes the
/// structural gate; the typed home (`visual_geometry`/`collision_geometry` `<capsule>`) already exists
/// and the SDF path populates it, so URDF mirrors that mapping. `<cone>`/`<ellipsoid>`
/// stay unmatched here: urdf-rs 0.9 REJECTS them, so they never reach this mapper (SDF-only shapes).
fn primitive_visual(geom: Option<&UrdfEl>) -> Option<VisualGeometry> {
    let geom = geom?;
    if let Some(b) = geom.find("box") {
        return Some(VisualGeometry {
            box_: Some(Box_ {
                size: b.get("size").map(str::to_string),
            }),
            ..Default::default()
        });
    }
    if let Some(c) = geom.find("cylinder") {
        return Some(VisualGeometry {
            cylinder: Some(Cylinder {
                radius: c.get("radius").map(str::to_string),
                length: c.get("length").map(str::to_string),
            }),
            ..Default::default()
        });
    }
    if let Some(s) = geom.find("sphere") {
        return Some(VisualGeometry {
            sphere: Some(Sphere {
                radius: s.get("radius").map(str::to_string),
            }),
            ..Default::default()
        });
    }
    if let Some(c) = geom.find("capsule") {
        return Some(VisualGeometry {
            capsule: Some(Capsule {
                radius: c.get("radius").map(str::to_string),
                length: c.get("length").map(str::to_string),
            }),
            ..Default::default()
        });
    }
    None
}

fn primitive_collision(geom: Option<&UrdfEl>) -> Option<CollisionGeometry> {
    let v = primitive_visual(geom)?;
    Some(CollisionGeometry {
        box_: v.box_,
        cylinder: v.cylinder,
        sphere: v.sphere,
        capsule: v.capsule,
        ..Default::default()
    })
}

fn material_to_color(mat: &UrdfEl) -> Color {
    let mut col = Color {
        name: mat.get("name").map(str::to_string),
        ..Default::default()
    };
    if let Some(c) = mat.find("color") {
        if let Some(rgba) = c.get("rgba") {
            col.rgba = Some(rgba.to_string());
        }
    }
    col
}

/// Resolve a visual `<material>` to an `'r g b a'` string: inline `<color rgba>`, or a named
/// reference into the top-level palette `mat_colors`. None if it carries no flat colour.
fn material_rgba(mat: Option<&UrdfEl>, mat_colors: &BTreeMap<String, String>) -> Option<String> {
    let mat = mat?;
    if let Some(c) = mat.find("color") {
        if let Some(rgba) = c.get("rgba") {
            return Some(rgba.to_string());
        }
    }
    let name = mat.get("name")?;
    mat_colors.get(name).cloned()
}

/// Resolve a visual `<material>` to a diffuse texture uri: an inline `<texture filename>`, or a named
/// reference into the top-level texture palette `mat_textures`. None if it carries no texture. Mirrors
/// [`material_rgba`] so a mesh visual's texture rides the [`VisualAssetHint`] side-channel exactly like
/// its flat colour does (the GLB baker folds it in; the HCDF document stays clean).
fn material_texture(
    mat: Option<&UrdfEl>,
    mat_textures: &BTreeMap<String, String>,
) -> Option<String> {
    let mat = mat?;
    if let Some(f) = mat.find("texture").and_then(|t| t.get("filename")) {
        return Some(f.to_string());
    }
    let name = mat.get("name")?;
    mat_textures.get(name).cloned()
}

fn visual(
    v: &UrdfEl,
    link: &str,
    idx: usize,
    notes: &mut Vec<String>,
    mat_colors: &BTreeMap<String, String>,
    mat_textures: &BTreeMap<String, String>,
    hints: &mut Vec<VisualAssetHint>,
) -> Visual {
    let authored = v.get("name").map(str::to_string);
    let name = authored
        .clone()
        .unwrap_or_else(|| format!("{link}_visual_{idx}"));
    let name_origin = if authored.is_none() {
        Some(NameOrigin::Synthesized)
    } else {
        None
    };
    let pose = origin_to_pose(v.find("origin"));
    let geom = v.find("geometry");
    let mesh = geom.and_then(|g| g.find("mesh"));
    let mat = v.find("material");
    let appearance = if let Some(mesh) = mesh {
        // ARM A: a visual mesh is a GLB <model>. Scale, mirror and material colour all bake in.
        let scale = mesh.get("scale");
        let color = material_rgba(mat, mat_colors);
        let texture = material_texture(mat, mat_textures);
        if let Some(s) = scale {
            if s != "1 1 1" {
                notes.push(format!(
                    "visual {}: mesh scale {} not applied (bake the GLB with the meshes present to fold it in)",
                    repr_str(&name), repr_str(s)
                ));
            }
        }
        // The note distinguishes a CARRIED colour (inline rgba, or a named reference resolved through
        // the robot-level palette; it rides the hint and bakes into the GLB) from a DROPPED material
        // (no flat colour anywhere: texture-only, or a name the palette does not resolve).
        if let Some(m) = mat {
            match color.as_deref() {
                Some(rgba) => notes.push(format!(
                    "visual {}: <material> color {} carried on the asset side-channel (bakes into the GLB at the asset step, not into the HCDF document)",
                    repr_str(&name),
                    repr_str(rgba)
                )),
                None => match texture.as_deref() {
                    // A texture-only material (no flat rgba) is no longer a dropped case: its diffuse
                    // texture rides the asset side-channel just like a colour does, so the baker can
                    // embed it into the GLB.
                    Some(tex) => notes.push(format!(
                        "visual {}: <material> texture {} carried on the asset side-channel (bakes into the GLB at the asset step, not into the HCDF document)",
                        repr_str(&name),
                        repr_str(tex)
                    )),
                    None => {
                        let why = match m.get("name") {
                            Some(n) => format!("references {} which resolves to no flat rgba", repr_str(n)),
                            None => "carries no flat rgba".to_string(),
                        };
                        notes.push(format!(
                            "visual {}: <material> {}; dropped (the GLB keeps the mesh's own appearance)",
                            repr_str(&name),
                            why
                        ));
                    }
                },
            }
        }
        // SIDE-CHANNEL: emit ONE hint per mesh visual carrying the already-resolved scale + flat colour +
        // diffuse texture, so a baker can fold the magnitude + mirror scale AND the appearance (flat rgba
        // and/or a `<texture filename>`) into the GLB. The HCDF model stays clean: a visual <model> is
        // still just a GLB uri+sha (no scale/colour/texture fields), so this is purely additive. The
        // texture is resolved inline off the visual's `<material><texture>` or by NAME through the
        // top-level texture palette, mirroring how the flat colour resolves.
        hints.push(VisualAssetHint {
            comp: link.to_string(),
            visual: name.clone(),
            scale: scale.map(str::to_string),
            color,
            texture,
        });
        VisualAppearance::Model {
            model: ModelRef {
                uri: mesh.get("filename").map(str::to_string),
                sha: None,
                ..Default::default()
            },
            geometry: None,
        }
    } else {
        // ARM B: a primitive with an optional flat color.
        let vg = primitive_visual(geom);
        if vg.is_none() {
            notes.push(format!(
                "visual {}: unsupported/empty <geometry> (no primitive)",
                repr_str(&name)
            ));
        }
        let color = mat.map(|m| {
            if m.find("texture").is_some() {
                notes.push(format!(
                    "visual {}: material <texture> dropped (bakes to GLB)",
                    repr_str(&name)
                ));
            }
            material_to_color(m)
        });
        VisualAppearance::Primitive {
            geometry: vg.or_else(|| Some(VisualGeometry::default())),
            color,
        }
    };
    Visual {
        name,
        toggle: None,
        name_origin,
        pose,
        appearance,
    }
}

fn collision(c: &UrdfEl, link: &str, idx: usize, notes: &mut Vec<String>) -> Collision {
    let authored = c.get("name").map(str::to_string);
    let name = authored
        .clone()
        .unwrap_or_else(|| format!("{link}_collision_{idx}"));
    let name_origin = if authored.is_none() {
        Some(NameOrigin::Synthesized)
    } else {
        None
    };
    let pose = origin_to_pose(c.find("origin"));
    let geom = c.find("geometry");
    let mesh = geom.and_then(|g| g.find("mesh"));
    let geometry = if let Some(mesh) = mesh {
        let mut m = Mesh {
            uri: mesh.get("filename").map(str::to_string),
            ..Default::default()
        };
        if let Some(scale) = mesh.get("scale") {
            m.scale = Some(scale.to_string()); // not baked -> keep the scale so it round-trips
        }
        CollisionGeometry {
            mesh: Some(m),
            ..Default::default()
        }
    } else {
        match primitive_collision(geom) {
            Some(cg) => cg,
            None => {
                notes.push(format!(
                    "collision {}: unsupported/empty <geometry>",
                    repr_str(&name)
                ));
                CollisionGeometry::default()
            }
        }
    };
    // URDF `<verbose value="true"/>` is a CHILD element with a string `value` (urdf.xsd:40-42,161);
    // HCDF collision/@verbose is a boolean attribute (hcdf.xsd:917) with a dedicated per-collision home.
    // Read the child's value, normalize to the canonical "true"/"false" (only when the element is present).
    let verbose = c
        .find("verbose")
        .and_then(|v| v.get("value"))
        .map(|s| matches!(s.trim(), "true" | "1").to_string());
    Collision {
        name: Some(name),
        verbose,
        name_origin,
        pose,
        geometry: Some(geometry),
        surface: None,
    }
}

fn inertial(ine: &UrdfEl) -> Inertial {
    let mut ip = Inertial::default();
    if let Some(mass) = ine.find("mass") {
        ip.mass = mass.get("value").map(str::to_string);
    }
    if let Some(org) = ine.find("origin") {
        ip.inertia_origin = origin_to_pose(Some(org));
    }
    if let Some(inr) = ine.find("inertia") {
        let parts: Vec<String> = ["ixx", "ixy", "ixz", "iyy", "iyz", "izz"]
            .iter()
            .map(|k| inr.get(k).unwrap_or("0").to_string())
            .collect();
        ip.inertia = Some(parts.join(" "));
    }
    ip
}

fn link_to_comp(
    link: &UrdfEl,
    notes: &mut Vec<String>,
    mat_colors: &BTreeMap<String, String>,
    mat_textures: &BTreeMap<String, String>,
    hints: &mut Vec<VisualAssetHint>,
) -> Comp {
    let mut comp = Comp {
        name: link.get("name").unwrap_or("").to_string(),
        ..Default::default()
    };
    if let Some(ine) = link.find("inertial") {
        comp.inertial = Some(inertial(ine));
    }
    for (i, v) in link.find_all("visual").enumerate() {
        comp.visual.push(visual(
            v,
            &comp.name,
            i,
            notes,
            mat_colors,
            mat_textures,
            hints,
        ));
    }
    for (i, c) in link.find_all("collision").enumerate() {
        comp.collision.push(collision(c, &comp.name, i, notes));
    }
    // link/@type (PR2 official form, urdf.xsd:177) -> urdf-compat/@link-type (hcdf.xsd:2732). Export
    // already emits it (to_urdf.rs write_link), so reading it here closes the export-only asymmetry.
    if let Some(link_type) = link.get("type") {
        comp.urdf_compat = Some(UrdfCompatComp {
            link_type: Some(link_type.to_string()),
        });
    }
    comp
}

// ── joints ────────────────────────────────────────────────────────────────────────────────────

fn joint(j: &UrdfEl, notes: &mut Vec<String>) -> Result<Joint> {
    let mut jt = Joint {
        name: j.get("name").map(str::to_string),
        ..Default::default()
    };
    let utype = j.get("type").unwrap_or("");
    let mapped = map_joint_type(utype);
    jt.type_ = Some(JointType::from_str(mapped).map_err(|_| {
        Error::Xml(format!(
            "joint {:?}: unknown joint type {utype:?}",
            jt.name.as_deref().unwrap_or("")
        ))
    })?);
    // Python formats `joint {jt.name!r}` in its notes -> single-quoted (`None` when unnamed).
    let jname = crate::pyrepr::repr_opt(jt.name.as_deref());
    if let Some(p) = j.find("parent") {
        jt.parent = Some(JointEndpoint {
            comp: p.get("link").map(str::to_string),
        });
    }
    if let Some(c) = j.find("child") {
        jt.child = Some(JointEndpoint {
            comp: c.get("link").map(str::to_string),
        });
    }
    jt.origin = origin_to_pose(j.find("origin"));
    let no_dof = utype == "fixed" || utype == "floating";
    if let Some(ax) = j.find("axis") {
        if !no_dof {
            jt.axis = Some(Axis {
                xyz: ax.get("xyz").map(str::to_string),
            });
        } else {
            notes.push(format!(
                "joint {jname}: <axis> on a {utype} joint dropped (joint has no DOF)"
            ));
        }
    }
    if let Some(lim) = j.find("limit") {
        if !no_dof {
            jt.limit = Some(JointLimit {
                lower: lim.get("lower").map(str::to_string),
                upper: lim.get("upper").map(str::to_string),
                effort: lim.get("effort").map(str::to_string),
                velocity: lim.get("velocity").map(str::to_string),
                // urdfdom v1.2 first-class limit attributes (urdf.xsd:215-217), mirroring
                // urdfdom_headers JointLimits, presence-preserving, same numeric string form.
                acceleration: lim.get("acceleration").map(str::to_string),
                deceleration: lim.get("deceleration").map(str::to_string),
                jerk: lim.get("jerk").map(str::to_string),
            });
        } else {
            notes.push(format!(
                "joint {jname}: <limit> on a {utype} joint dropped (joint has no DOF)"
            ));
        }
    }
    // Planar is 2-DOF (two in-plane translation ranges), but URDF gives a single <limit> and no second
    // bound. The single URDF <limit> above lands in HCDF <limit> (the FIRST in-plane range) and <axis>
    // is the plane normal; the SECOND in-plane DOF (limit2) is left UNBOUNDED; we do NOT fabricate a
    // bound (URDF's own under-specification, faithfully preserved). Warn so it is not a silent gap.
    if mapped == "planar" {
        notes.push(format!(
            "joint {jname}: planar joint: URDF's single <limit> mapped to <limit> (first in-plane range); the second in-plane DOF (<limit2>) is left UNBOUNDED (URDF specifies no second bound; not fabricated)"
        ));
    }
    if let Some(dyn_) = j.find("dynamics") {
        jt.dynamics = Some(JointDynamics {
            damping: dyn_.get("damping").map(str::to_string),
            friction: dyn_.get("friction").map(str::to_string),
            ..Default::default()
        });
    }
    if let Some(mim) = j.find("mimic") {
        jt.mimic = Some(Mimic {
            joint: mim.get("joint").map(str::to_string),
            multiplier: mim.get("multiplier").map(str::to_string),
            offset: mim.get("offset").map(str::to_string),
        });
    }
    if let Some(cal) = j.find("calibration") {
        jt.calibration = Some(JointCalibration {
            reference_position: cal.get("reference_position").map(str::to_string),
            rising: cal.get("rising").map(str::to_string),
            falling: cal.get("falling").map(str::to_string),
        });
    }
    if let Some(sc) = j.find("safety_controller") {
        let mut s = SafetyController {
            soft_lower_limit: sc.get("soft_lower_limit").map(str::to_string),
            soft_upper_limit: sc.get("soft_upper_limit").map(str::to_string),
            k_position: sc.get("k_position").map(str::to_string),
            k_velocity: sc.get("k_velocity").map(str::to_string),
        };
        if s.k_velocity.is_none() {
            s.k_velocity = Some("0".to_string()); // HCDF requires k_velocity
            notes.push(format!(
                "joint {jname}: safety_controller k_velocity absent in source; defaulted to 0"
            ));
        }
        jt.urdf_compat = Some(UrdfCompatJoint {
            safety_controller: Some(s),
        });
        notes.push(format!(
            "joint {jname}: <safety_controller> moved to <urdf-compat> (URDF-compat, zero-CPS)"
        ));
    }
    Ok(jt)
}

// ── real-world tolerance: joint endpoints referencing links the document never declares ─────────

/// Synthesize an empty stub [`Comp`] for every `<joint>` `parent`/`child` that references a link the
/// document never declares, so the joint edge resolves instead of dangling.
///
/// `urdf-rs` accepts a document whose joint endpoints point at undeclared link names (it does not
/// cross-check `<joint>` `parent`/`child` against the set of `<link>` names), so a malformed URDF
/// imports "successfully" yet leaves those edges pointing at comps that never exist. A broken
/// find-replace in the source is the canonical way this happens: perseverance.urdf renames exactly one
/// side of each of its four `Frame_STEER_*` link/joint-endpoint pairs, so four joint `<child>`s
/// reference links (`Frame_STEER_LF`, `_LR`, `_RF`, `_RR`) that were never declared while the four
/// matching declared links are orphaned. A dangling child edge collapses its whole subtree onto the
/// origin (the "chaotic" render) and blocks Save downstream. Rather than leave the dangling ref, this
/// mints an EMPTY stub comp (the same name-only shape a massless frame-link imports to, via `link_to_comp`
/// on a `<link>` with no inertial/visual/collision) for each undeclared endpoint, in first-reference
/// document order, and records one note per stub. A well-formed URDF (every endpoint declared)
/// synthesizes nothing and leaves the document untouched.
fn synthesize_missing_endpoint_comps(doc: &mut Hcdf, notes: &mut Vec<String>) {
    use std::collections::BTreeSet;
    let declared: BTreeSet<String> = doc.comp.iter().map(|c| c.name.clone()).collect();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut missing: Vec<(String, &'static str)> = Vec::new();
    for j in &doc.joint {
        for (role, ep) in [("parent", &j.parent), ("child", &j.child)] {
            if let Some(name) = ep.as_ref().and_then(|e| e.comp.as_deref()) {
                if !declared.contains(name) && seen.insert(name.to_string()) {
                    missing.push((name.to_string(), role));
                }
            }
        }
    }
    for (name, role) in missing {
        notes.push(format!(
            "joint {role} references undeclared link {}; synthesized an empty stub comp so the joint edge resolves (the source URDF declares no matching <link>)",
            repr_str(&name)
        ));
        doc.comp.push(Comp {
            name,
            ..Default::default()
        });
    }
}

// ── transmission front end ──────────────────────────────────────────────────────────────────────

/// Import every top-level URDF `<transmission>` into the TYPED core `<transmission>` (doc-level
/// [`Hcdf::transmission`]) instead of quarantining it verbatim into the old `org.ros.control` blob.
///
/// Mapping (ros_control `transmission_interface` grammar):
///   * `@name` -> `@name` (required; a nameless block is left for the verbatim quarantine, since the
///     typed core requires `@name`).
///   * `<type>` text -> `@type` via [`map_transmission_type`] (unknown class -> `@type` unset + note).
///   * `<joint name=…>` -> `<joint ref=…>` transmission endpoint.
///   * `<actuator name=…>` -> `<motor ref=…>` transmission endpoint; its `<mechanicalReduction>` (or a
///     transmission-level `<mechanicalReduction>`, the older PR2 placement) -> `<reduction>`.
///   * `<hardwareInterface>` has no typed-core home -> recorded as a residual note (the electrical/
///     control-mode surface is NATIVE-ONLY; ros2_control carries the interface wiring).
///
/// Returns the set of top-level child indices consumed into typed transmissions, which the caller drops
/// from the verbatim quarantine pass.
fn extract_transmissions(
    root: &UrdfEl,
    doc: &mut Hcdf,
    notes: &mut Vec<String>,
) -> std::collections::BTreeSet<usize> {
    use std::collections::BTreeSet;
    let mut consumed: BTreeSet<usize> = BTreeSet::new();
    for (idx, ch) in root.children.iter().enumerate() {
        if ch.tag != "transmission" {
            continue;
        }
        let Some(name) = ch.get("name") else {
            notes.push(
                "<transmission> without a name left quarantined verbatim (the typed core <transmission> requires @name)".to_string(),
            );
            continue;
        };
        let mut tr = Transmission {
            name: Some(name.to_string()),
            ..Default::default()
        };
        // Modern ros_control uses a `<type>` CHILD element; the official PR2 urdf.xsd:288 form uses a
        // required `@type` ATTRIBUTE. Prefer the child, fall back to the attribute (additive import
        // tolerance, zero downside; export form is unchanged).
        if let Some(ttext) = ch
            .find("type")
            .and_then(UrdfEl::text_trim)
            .or_else(|| ch.get("type"))
        {
            match map_transmission_type(ttext) {
                Some(kind) => tr.type_ = Some(kind),
                None => notes.push(format!(
                    "transmission {}: unknown <type> {} has no HCDF TransmissionType; @type left unset",
                    repr_str(name),
                    repr_str(ttext)
                )),
            }
        }
        if let Some(jname) = ch.find("joint").and_then(|j| j.get("name")) {
            tr.joint.push(Endpoint {
                ref_: Some(jname.to_string()),
                role: None,
            });
        }
        let actuator = ch.find("actuator");
        if let Some(aname) = actuator.and_then(|a| a.get("name")) {
            tr.motor.push(Endpoint {
                ref_: Some(aname.to_string()),
                role: None,
            });
        }
        // <mechanicalReduction> lives inside <actuator> (transmission_interface) or directly under
        // <transmission> (older PR2 form); accept either, actuator-scoped first.
        let reduction = actuator
            .and_then(|a| a.find("mechanicalReduction"))
            .or_else(|| ch.find("mechanicalReduction"))
            .and_then(UrdfEl::text_trim);
        if let Some(red) = reduction {
            tr.reduction = Some(red.to_string());
        }
        // <hardwareInterface> (on the joint and/or actuator) has no typed-core home.
        let hw: Vec<&str> = ["joint", "actuator"]
            .iter()
            .filter_map(|k| ch.find(k))
            .flat_map(|el| el.find_all("hardwareInterface"))
            .filter_map(UrdfEl::text_trim)
            .collect();
        if !hw.is_empty() {
            notes.push(format!(
                "transmission {}: <hardwareInterface> {} has no core-transmission home (dropped; the control-interface surface is carried by ros2_control, not the core <transmission>)",
                repr_str(name),
                repr_str(&hw.join(", "))
            ));
        }
        // The legacy PR2 transmission mechanics (urdf.xsd:265-277) have no typed-core home and are not
        // carried by the verbatim quarantine once the <transmission> is consumed; note them so they are
        // not SILENTLY lost (full support needs model work; this at least flags the drop).
        let pr2_present: Vec<&str> = [
            "leftActuator",
            "rightActuator",
            "flexJoint",
            "rollJoint",
            "gap_joint",
            "passive_joint",
            "use_simulated_gripper_joint",
        ]
        .into_iter()
        .filter(|k| ch.find(k).is_some())
        .collect();
        if !pr2_present.is_empty() {
            notes.push(format!(
                "transmission {}: PR2 mechanics {} have no core-transmission home (dropped)",
                repr_str(name),
                repr_str(&pr2_present.join(", "))
            ));
        }
        doc.transmission.push(tr);
        consumed.insert(idx);
    }
    consumed
}

// ── Gazebo sensor front end ─────────────────────────────────────────────────────────────────────

/// Reach into each top-level `<gazebo reference=...>` block and decompose it into typed HCDF, REUSING the
/// SDF mappers ([`crate::from_sdf::gazebo_block_extract`]); Gazebo shares the SDFormat sensor + surface
/// grammar, so the URDF/Gazebo front end feeds the SAME code the SDF `<link>` re-root uses. Two things are
/// pulled from each block and attached to the comp named by the block's `@reference`:
///   * every mappable `<sensor>` child -> a typed HCDF sensor on the comp;
///   * the Gazebo-classic friction/contact idiom (`<mu1>/<mu2>/<kp>/<kd>`) -> the typed collision
///     `<surface>` on EACH of the comp's collisions; if the comp has no
///     `<collision>`, the friction stays quarantined verbatim rather than being dropped.
///
/// The block's residual (`<plugin>`/material overrides, any un-typed `<sensor>`, un-mapped sim-only
/// sub-fields, and, when the friction could not be attached, the friction idiom itself) stays
/// quarantined OPAQUE in `org.gazebosim.raw` (a verbatim `<gazebo>` block has no typed `<gazebo-sim>`
/// home; see [`domain_for`]).
///
/// Returns a per-top-level-index override for the verbatim quarantine: a decomposed block contributes its
/// RESIDUAL string in place of its raw body; an EMPTY residual meaning the block was fully consumed
/// (dropped from the quarantine entirely). A block that names no comp, carries no `@reference`, or from
/// which nothing was typed is absent from the map and stays quarantined verbatim (never silently dropped).
#[cfg(feature = "sdf")]
fn extract_gazebo(
    root: &UrdfEl,
    raw_toplevel: &BTreeMap<usize, String>,
    doc: &mut Hcdf,
    notes: &mut Vec<String>,
) -> Result<BTreeMap<usize, String>> {
    let mut residual_by_idx: BTreeMap<usize, String> = BTreeMap::new();
    for (idx, ch) in root.children.iter().enumerate() {
        if ch.tag != "gazebo" {
            continue;
        }
        let Some(raw) = raw_toplevel.get(&idx) else {
            continue;
        };
        let Some(extract) = crate::from_sdf::gazebo_block_extract(raw, notes)? else {
            continue; // nothing typed in this block -> leave it quarantined verbatim.
        };
        let n = extract.sensors.len();
        let has_friction = extract.surface.is_some();
        let Some(reference) = extract.reference.clone() else {
            notes.push(format!(
                "gazebo block carries no @reference; {n} typed sensor(s){} left quarantined verbatim",
                if has_friction { " and its friction" } else { "" }
            ));
            continue;
        };
        let Some(comp) = doc.comp.iter_mut().find(|c| c.name == reference) else {
            notes.push(format!(
                "gazebo reference={}: names no <link> to attach {n} typed sensor(s){}; block left quarantined verbatim",
                repr_str(&reference),
                if has_friction { " and its friction" } else { "" }
            ));
            continue;
        };
        comp.sensor.extend(extract.sensors);
        // Route the Gazebo-classic friction idiom to the typed collision <surface>. Gazebo friction is
        // link-wide, so it lands on EVERY collision of the comp (URDF collisions never carry a surface of
        // their own -> nothing is clobbered). A comp with no <collision> has nowhere to put it, so the
        // friction is left in the quarantine (`residual_surface_kept`) instead of being dropped.
        let surface_applied = match extract.surface {
            Some(surf) if !comp.collision.is_empty() => {
                let n_col = comp.collision.len();
                for col in &mut comp.collision {
                    col.surface = Some(surf.clone());
                }
                notes.push(format!(
                    "gazebo reference={}: mapped <mu1>/<mu2>/<kp>/<kd> to the typed collision <surface> on {n_col} collision(s)",
                    repr_str(&reference)
                ));
                true
            }
            Some(_) => {
                notes.push(format!(
                    "gazebo reference={}: <mu1>/<mu2>/<kp>/<kd> friction present but the link has no <collision>; friction left quarantined verbatim",
                    repr_str(&reference)
                ));
                false
            }
            None => false,
        };
        // Apply a recognized Gazebo/<Color> per-link <material> override to the comp's ARM-B (primitive)
        // visuals' inline <color>. ARM-A (GLB model) visuals carry their own baked
        // appearance and have no inline-color slot, so the override cannot land on them. When it lands on at
        // least one primitive visual the `<material>` is consumed from the residual (`material_applied`);
        // otherwise it stays quarantined verbatim (never lost) WITH a note.
        let material_applied = match extract.material {
            Some(color) => {
                let mut applied = 0usize;
                for v in &mut comp.visual {
                    if let VisualAppearance::Primitive { color: c, .. } = &mut v.appearance {
                        *c = Some(color.clone());
                        applied += 1;
                    }
                }
                if applied > 0 {
                    notes.push(format!(
                        "gazebo reference={}: mapped <material> to the inline <color> on {applied} primitive visual(s)",
                        repr_str(&reference)
                    ));
                } else {
                    notes.push(format!(
                        "gazebo reference={}: <material> override recognized but the link has no primitive <visual> to color (GLB visuals keep their baked appearance); material left quarantined verbatim",
                        repr_str(&reference)
                    ));
                }
                applied > 0
            }
            None => false,
        };
        // Insert a residual override only when something was consumed from the block; otherwise leave it
        // quarantined verbatim (byte-identical). An empty residual makes the quarantine pass drop the block.
        if n > 0 || surface_applied || material_applied {
            residual_by_idx.insert(
                idx,
                extract.residual.render(surface_applied, material_applied),
            );
        }
        if n > 0 {
            notes.push(format!(
                "gazebo reference={}: imported {n} <sensor> into the comp as typed sensor(s); residual <gazebo> content stays quarantined in org.gazebosim.raw",
                repr_str(&reference)
            ));
        }
    }
    Ok(residual_by_idx)
}

/// Without the SDF mappers (`feature = "sdf"` off), a `<gazebo>` block stays quarantined whole (the prior
/// behavior): no per-index override, so the verbatim quarantine carries every block.
#[cfg(not(feature = "sdf"))]
fn extract_gazebo(
    _root: &UrdfEl,
    _raw_toplevel: &BTreeMap<usize, String>,
    _doc: &mut Hcdf,
    _notes: &mut Vec<String>,
) -> Result<BTreeMap<usize, String>> {
    Ok(BTreeMap::new())
}

// ── native URDF `<sensor>` front end ────────────────────────────────────────────────────────────

/// Decompose each robot-level `<sensor>` (urdf.xsd:401, complexType 327-341) into a TYPED HCDF sensor
/// on the comp named by its required `<parent link>`, mirroring the SDF/Gazebo path
/// ([`crate::from_sdf`] camera/ray decomposition) but reading the URDF-native `<camera><image>`
/// ATTRIBUTE grammar (urdf.xsd:288-307) and the `<ray>` `LaserRay` grammar (urdf.xsd:308-325).
///
/// Zero-regression contract: this ADDS typed visibility only. The very same `<sensor>` STAYS quarantined
/// verbatim under `<extension domain="urdf:sensor">` (the quarantine pass is untouched; a top-level
/// `<sensor>` is neither typed-core nor consumed here), so the URDF round-trip is byte-identical. On
/// export, `to_urdf` suppresses the synthesized `<gazebo><sensor>` for any sensor carried by that
/// quarantine (matched by `@name`), so the sensor is emitted exactly once (verbatim, via the quarantine).
///
/// A `<sensor>` with no `@name` (which the export correlation needs), no `<parent link>`, or an unknown
/// `<parent>` link is left with only its verbatim quarantine (typed decomposition skipped, never dropped).
fn extract_native_sensors(root: &UrdfEl, doc: &mut Hcdf, notes: &mut Vec<String>) {
    for sel in root.find_all("sensor") {
        let Some(name) = sel.get("name").map(str::to_string) else {
            notes.push(
                "native <sensor> without @name left quarantined verbatim only (no typed decomposition; export correlation needs the name)".to_string(),
            );
            continue;
        };
        let ctx = format!("native sensor {}", repr_str(&name));
        let Some(optical) = native_sensor_optical(sel, notes, &ctx) else {
            // Neither <camera> nor <ray> payload -> nothing typed; the verbatim quarantine still carries it.
            notes.push(format!(
                "{ctx}: no <camera>/<ray> payload to decompose; left quarantined verbatim only"
            ));
            continue;
        };
        let Some(parent) = sel.find("parent").and_then(|p| p.get("link")) else {
            notes.push(format!(
                "{ctx}: no <parent link>; typed decomposition skipped (left quarantined verbatim only)"
            ));
            continue;
        };
        let Some(comp) = doc.comp.iter_mut().find(|c| c.name == parent) else {
            notes.push(format!(
                "{ctx}: <parent link={}> names no <link>; typed decomposition skipped (left quarantined verbatim only)",
                repr_str(parent)
            ));
            continue;
        };
        comp.sensor.push(Sensor {
            name: Some(name),
            update_rate: sel.get("update_rate").map(str::to_string),
            optical: vec![optical],
            ..Default::default()
        });
        notes.push(format!(
            "{ctx}: decomposed into a typed optical sensor on comp {} (also kept quarantined verbatim for the byte-identical URDF round-trip)",
            repr_str(parent)
        ));
    }
}

/// The `<sensor>`'s `<camera>` or `<ray>` payload -> a typed [`OpticalSensor`]. `<origin>` (urdf.xsd:330,
/// pose type) rides along as the optical pose. Returns `None` when neither payload is present.
fn native_sensor_optical(
    sel: &UrdfEl,
    notes: &mut Vec<String>,
    ctx: &str,
) -> Option<OpticalSensor> {
    let pose = origin_to_pose(sel.find("origin"));
    if let Some(cam) = sel.find("camera") {
        Some(native_camera_optical(cam, pose, notes, ctx))
    } else {
        sel.find("ray").map(|ray| native_ray_optical(ray, pose))
    }
}

/// `<camera><image width height format hfov near far>` (urdf.xsd:288-307) -> `<optical type="camera">`
/// with one `<fov>`: image dims/format plus derived square-pixel pinhole intrinsics
/// (fx=fy=(w/2)/tan(hfov/2), cx=w/2, cy=h/2, mirroring [`crate::from_sdf`]) and a pyramidal frustum
/// whose near/far are the URDF
/// image attributes FIRST-CLASS (urdf.xsd:297-298 are REQUIRED attributes, so, unlike SDF's separable
/// `<clip>`, nothing is quarantined; they fold straight into `frustum/near|far`).
fn native_camera_optical(
    cam: &UrdfEl,
    pose: Option<Pose>,
    notes: &mut Vec<String>,
    ctx: &str,
) -> OpticalSensor {
    let image = cam.find("image");
    OpticalSensor {
        type_: Some(OpticalSensorType::Camera),
        pose,
        fov: vec![SensorFov {
            // `<fov name>` is schema-required (hcdf.xsd sensor_fov); a URDF camera is single-view, so
            // name the one FoV "main" (matching the SDF import convention) rather than emitting an
            // anonymous `<fov>` that would fail the document's own XSD.
            name: Some("main".to_string()),
            geometry: native_camera_frustum(image),
            intrinsics: native_camera_intrinsics(image, notes, ctx),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// `<image>` dims/format + derived pinhole intrinsics from `@hfov`+`@width` (square-pixel:
/// fx=fy=(w/2)/tan(hfov/2), cx=w/2, cy=h/2). Presence-preserving: an attribute absent from the source
/// stays `None`.
fn native_camera_intrinsics(
    image: Option<&UrdfEl>,
    notes: &mut Vec<String>,
    ctx: &str,
) -> Option<CameraIntrinsics> {
    let image = image?;
    let width = image.get("width");
    let height = image.get("height");
    let mut intr = CameraIntrinsics {
        width: width.map(str::to_string),
        height: height.map(str::to_string),
        format: image.get("format").map(str::to_string),
        ..Default::default()
    };
    if let (Some(hfov), Some(w)) = (image.get("hfov").and_then(parse_f), width.and_then(parse_f)) {
        let fx = (w / 2.0) / (hfov / 2.0).tan();
        intr.fx = Some(fmt_num(fx));
        intr.fy = Some(fmt_num(fx));
        intr.cx = Some(fmt_num(w / 2.0));
        intr.cy = height.and_then(parse_f).map(|h| fmt_num(h / 2.0));
        notes.push(format!(
            "{ctx}: camera intrinsics fx/fy/cx/cy derived from <image> @hfov+@width (square-pixel pinhole)"
        ));
    }
    (intr != CameraIntrinsics::default()).then_some(intr)
}

/// A pyramidal camera FoV frustum from the URDF `<image>` @near/@far/@hfov (all first-class attributes).
fn native_camera_frustum(image: Option<&UrdfEl>) -> Option<Geometry> {
    let image = image?;
    let near = image.get("near");
    let far = image.get("far");
    let hfov = image.get("hfov");
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

/// `<ray><horizontal|vertical samples resolution min_angle max_angle>` (urdf.xsd:308-325) ->
/// `<optical type="lidar">` + `<lidar-params>` with the angular `<scan-pattern>`, the schema's
/// explicitly lossless target for a converted ray sensor (sensor_params.rs scan-pattern). All LaserRay
/// values are attributes; presence is preserved (an omitted attribute stays `None`).
fn native_ray_optical(ray: &UrdfEl, pose: Option<Pose>) -> OpticalSensor {
    let horizontal = ray.find("horizontal").map(native_scan_axis);
    let vertical = ray.find("vertical").map(native_scan_axis);
    let mut lp = LidarParams::default();
    if horizontal.is_some() || vertical.is_some() {
        lp.scan_pattern = Some(LidarScanPattern {
            horizontal,
            vertical,
        });
    }
    OpticalSensor {
        type_: Some(OpticalSensorType::Lidar),
        pose,
        lidar_params: (lp != LidarParams::default()).then_some(lp),
        ..Default::default()
    }
}

/// One `<horizontal>`/`<vertical>` LaserRay -> [`LidarScanAxis`] (samples/resolution/min_angle/max_angle).
fn native_scan_axis(ax: &UrdfEl) -> LidarScanAxis {
    LidarScanAxis {
        samples: ax.get("samples").map(str::to_string),
        resolution: ax.get("resolution").map(str::to_string),
        min_angle: ax.get("min_angle").map(str::to_string),
        max_angle: ax.get("max_angle").map(str::to_string),
    }
}

/// Parse a single float from trimmed attribute text (for the derived pinhole intrinsics).
fn parse_f(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

/// Compact `f64` -> text: integral values without a fractional part, else six-decimal trimmed (mirrors
/// the SDF importer's `fmt_num`, so a URDF-derived and an SDF-derived intrinsic format identically).
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ── lenient pre-pass: real-world URDF quirks the strict urdf-rs gate hard-rejects ───────────────

/// A byte-span edit against the original URDF text, scheduled by [`scan_urdf_quirks`].
enum QuirkEdit {
    /// Insert ` type="fixed"` at this byte offset (inside a typeless top-level `<joint …>` tag,
    /// just before its closing `>` / `/>`).
    InsertFixedType(usize),
    /// Insert ` name="{name}"` at this byte offset (inside a nameless inline `<visual><material …>`
    /// tag, just before its closing `>` / `/>`). The official URDF XSD makes an inline material's
    /// `@name` OPTIONAL, but urdf-rs types it as a required `String`, so a real published URDF (Unitree
    /// h1_2) with a nameless inline material is hard-rejected; the synthesized, deterministic name lets
    /// the strict gate accept the document.
    InsertMaterialName { at: usize, name: String },
    /// Drop the bytes `start..end` (a duplicate `<material>` element directly inside a `<visual>`).
    Drop { start: usize, end: usize },
}

/// The unescaped value of the attribute with this EXACT key (URDF attributes are unprefixed, so no
/// local-name matching; a prefixed `foo:type` must NOT count as a `type`).
fn attr_exact(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

/// Streaming scan state for [`scan_urdf_quirks`]: tracks the open-element path plus just enough
/// per-`<link>`/`<visual>` context to name the fixed element in each note (the synthesized
/// `{link}_visual_{idx}` naming matches [`link_to_comp`]'s).
#[derive(Default)]
struct QuirkScan {
    edits: Vec<QuirkEdit>,
    notes: Vec<String>,
    /// Local names of the open elements, from `<robot>` down to the current element.
    path: Vec<String>,
    link_name: String,
    visual_idx: usize,
    visual_name: String,
    mat_count: usize,
    dropped_in_visual: usize,
    /// An OPEN duplicate `<material>` being dropped: (span start, path depth to pop back to).
    open_drop: Option<(usize, usize)>,
}

impl QuirkScan {
    fn path_is(&self, want: &[&str]) -> bool {
        self.path.len() == want.len() && self.path.iter().zip(want).all(|(a, b)| a == b)
    }

    /// A Start/Empty tag spanning bytes `pos..end` was read (`is_empty` = self-closing).
    fn on_open(
        &mut self,
        e: &quick_xml::events::BytesStart,
        pos: usize,
        end: usize,
        is_empty: bool,
    ) {
        let tag = local_name(e);
        if tag == "joint" && self.path_is(&["robot"]) && attr_exact(e, b"type").is_none() {
            // quirk 1: a typeless joint: RViz treats it as fixed; inject `type="fixed"` before the
            // tag's closing `>` (Start) / `/>` (Empty).
            let at = if is_empty { end - 2 } else { end - 1 };
            self.edits.push(QuirkEdit::InsertFixedType(at));
            self.notes.push(format!(
                "joint {} has no type; treated as fixed",
                crate::pyrepr::repr_opt(attr_exact(e, b"name").as_deref())
            ));
        } else if tag == "link" && self.path_is(&["robot"]) {
            self.link_name = attr_exact(e, b"name").unwrap_or_default();
            self.visual_idx = 0;
        } else if tag == "visual" && self.path_is(&["robot", "link"]) {
            self.visual_name = attr_exact(e, b"name")
                .unwrap_or_else(|| format!("{}_visual_{}", self.link_name, self.visual_idx));
            self.visual_idx += 1;
            self.mat_count = 0;
            self.dropped_in_visual = 0;
        } else if tag == "material" && self.path_is(&["robot", "link", "visual"]) {
            self.mat_count += 1;
            if self.mat_count > 1 {
                // quirk 2: a duplicate <material> directly inside one <visual> (exporter bug); keep
                // the FIRST, drop this one. A Start element's span closes at its matching End tag.
                if is_empty {
                    self.edits.push(QuirkEdit::Drop { start: pos, end });
                } else {
                    self.open_drop = Some((pos, self.path.len()));
                }
                self.dropped_in_visual += 1;
            } else if attr_exact(e, b"name").is_none() {
                // quirk 3: the KEPT inline <material> has no name; the official URDF XSD makes it
                // optional, but urdf-rs requires it (rejecting e.g. Unitree h1_2). Inject a
                // deterministic, distinctively-prefixed name (never authored on a real material) before
                // the tag's closing `>` (Start) / `/>` (Empty) so the strict gate accepts the document.
                // Only the FIRST material of a visual is ever kept, so the injection never lands inside
                // a to-be-dropped duplicate span.
                let at = if is_empty { end - 2 } else { end - 1 };
                let name = format!("__hcdf_unnamed_material_{}", self.visual_name);
                self.edits.push(QuirkEdit::InsertMaterialName {
                    at,
                    name: name.clone(),
                });
                self.notes.push(format!(
                    "visual {}: inline <material> has no name; synthesized {} so the strict parser accepts it (URDF XSD makes an inline material name optional)",
                    repr_str(&self.visual_name),
                    repr_str(&name)
                ));
            }
        }
        if !is_empty {
            self.path.push(tag);
        }
    }

    /// An End tag was read; `end` = the byte offset just past its `>`.
    fn on_close(&mut self, end: usize) {
        let closed = self.path.pop();
        if let Some((start, depth)) = self.open_drop {
            if self.path.len() == depth {
                self.edits.push(QuirkEdit::Drop { start, end });
                self.open_drop = None;
            }
        }
        if closed.as_deref() == Some("visual")
            && self.path_is(&["robot", "link"])
            && self.dropped_in_visual > 0
        {
            self.notes.push(format!(
                "visual {}: {} duplicate <material> element(s) dropped (first one wins)",
                repr_str(&self.visual_name),
                self.dropped_in_visual
            ));
            self.dropped_in_visual = 0;
        }
    }
}

/// Scan the raw URDF for the three known real-world quirks (see the module doc's lenient pre-pass
/// section) and schedule the byte edits + human notes that fix them. Returns empty vecs when the
/// document is quirk-free, or when it is not scannable at all, so the strict gate sees the ORIGINAL
/// bytes and reports its own (unchanged) error.
fn scan_urdf_quirks(src: &str) -> (Vec<QuirkEdit>, Vec<String>) {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(src);
    reader.config_mut().expand_empty_elements = false;
    let mut scan = QuirkScan::default();
    loop {
        let pos = reader.buffer_position() as usize;
        let Ok(ev) = reader.read_event() else {
            return (Vec::new(), Vec::new());
        };
        let end = reader.buffer_position() as usize;
        match ev {
            Event::Start(e) => scan.on_open(&e, pos, end, false),
            Event::Empty(e) => scan.on_open(&e, pos, end, true),
            Event::End(_) => scan.on_close(end),
            Event::Eof => break,
            _ => {}
        }
    }
    (scan.edits, scan.notes)
}

/// Lenient pre-pass applied to the raw URDF BEFORE the strict `urdf-rs` gate: fixes the three known
/// real-world quirks (typeless top-level `<joint>` -> `type="fixed"`; duplicate `<visual>`
/// `<material>` -> first wins; nameless inline `<material>` -> synthesized name), appending one note
/// per fix to `notes`. A quirk-free document is
/// returned zero-copy and byte-identical; otherwise the scheduled edits are spliced surgically;
/// every byte outside the edited spans is untouched (no reformatting, no other rewrites).
fn sanitize_urdf<'a>(src: &'a str, notes: &mut Vec<String>) -> std::borrow::Cow<'a, str> {
    let (edits, quirk_notes) = scan_urdf_quirks(src);
    if edits.is_empty() {
        return std::borrow::Cow::Borrowed(src);
    }
    // The edits arrive in document order and never overlap (an insert sits inside a top-level
    // <joint> tag; a drop covers a whole <material> element three levels down).
    let mut out = String::with_capacity(src.len() + 16 * edits.len());
    let mut cursor = 0usize;
    for edit in &edits {
        match edit {
            QuirkEdit::InsertFixedType(at) => {
                out.push_str(&src[cursor..*at]);
                out.push_str(" type=\"fixed\"");
                cursor = *at;
            }
            QuirkEdit::InsertMaterialName { at, name } => {
                out.push_str(&src[cursor..*at]);
                out.push_str(" name=\"");
                out.push_str(name);
                out.push('"');
                cursor = *at;
            }
            QuirkEdit::Drop { start, end } => {
                out.push_str(&src[cursor..*start]);
                cursor = *end;
            }
        }
    }
    out.push_str(&src[cursor..]);
    notes.extend(quirk_notes);
    std::borrow::Cow::Owned(out)
}

// ── entry point ─────────────────────────────────────────────────────────────────────────────────

/// Import a URDF document (XML string) into an [`Hcdf`] model, returning `(doc, notes)`.
///
/// `urdf-rs` validates structural well-formedness (the pure-Rust parser); the typed mapping is
/// driven from a parallel quick-xml element tree so attribute text and element presence match the
/// Python oracle exactly. Top-level `<gazebo>`/`<transmission>`/`<ros2_control>`/vendor blocks (which
/// `urdf-rs` drops) are quarantined VERBATIM into root `<extension>` blocks by domain.
///
/// xacro is assumed already expanded; meshes are NOT baked here (they stay as `<model uri>` /
/// `<mesh uri>` references), matching `from_urdf.py` with `baker=None`.
///
/// This is the thin back-compat wrapper around [`from_urdf_str_with_assets`]: it DISCARDS the per-visual
/// [`VisualAssetHint`]s, so its return + behaviour are byte-for-byte unchanged for existing callers.
pub fn from_urdf_str(src: &str) -> Result<(Hcdf, Vec<String>)> {
    let (doc, notes, _hints) = from_urdf_str_with_assets(src)?;
    Ok((doc, notes))
}

/// Like [`from_urdf_str`], but ALSO returns one [`VisualAssetHint`] per ARM-A (mesh) `<visual>`: the
/// already-resolved per-visual mesh `scale` and flat material `color`. The [`Hcdf`] / [`Visual`] /
/// [`ModelRef`] model is IDENTICAL to what [`from_urdf_str`] produces (a visual `<model>` is still a clean
/// GLB `uri`+`sha` with no scale/colour fields); the hints are a SEPARATE side-channel a baker consumes to
/// fold the magnitude + mirror scale and the flat colour into the baked GLB. (`from_urdf` itself never bakes,
/// matching `from_urdf.py` with `baker=None`; the loss notes describing the deferred bake are unchanged.)
pub fn from_urdf_str_with_assets(src: &str) -> Result<(Hcdf, Vec<String>, Vec<VisualAssetHint>)> {
    let mut notes: Vec<String> = Vec::new();
    // Lenient pre-pass (see the module doc): fix the three real-world quirks urdf-rs hard-rejects,
    // noting each fix. A quirk-free document passes through zero-copy and byte-identical.
    let src = sanitize_urdf(src, &mut notes);
    // Structural validation gate: the pure-Rust parser must accept the (sanitized) document.
    urdf_rs::read_from_string(&src)
        .map_err(|e| Error::Xml(format!("urdf-rs rejected the URDF: {e}")))?;

    let (root, raw_toplevel) = parse_dom(&src)?;
    if root.tag != "robot" {
        return Err(Error::Xml(format!(
            "not a URDF: root element is <{}>, expected <robot>",
            root.tag
        )));
    }
    let mut hints: Vec<VisualAssetHint> = Vec::new();

    let mut doc = Hcdf {
        name: root.get("name").unwrap_or("robot").to_string(),
        // Leave the document version UNSET, exactly like Python `from_urdf` (whose `dumps` omits the
        // `@version` attribute). Stamping a "1.0" here would diverge the round-trip: `to_urdf` records a
        // dropped-`version` annotation loss ONLY when a version is present, so a stamped version makes the
        // Rust import->export manifest carry an item the Python manifest does not. (The schema treats an
        // absent `@version` as the implicit current version, so the output stays a valid 1.0 document.)
        version: String::new(),
        body_frame: Some(BodyFrame::FLU),
        world_frame: Some(WorldFrame::ENU),
        ..Default::default()
    };
    if let Some(ver) = root.get("version") {
        notes.push(format!(
            "URDF robot/@version={} not preserved (a URDF document version is distinct from the HCDF schema version)",
            repr_str(ver)
        ));
    }

    // typed core: a top-level material becomes a flat <color> only if it has a usable <color rgba>;
    // a texture-based (or rgba-less) material is quarantined verbatim into <extension>.
    let mut mat_colors: BTreeMap<String, String> = BTreeMap::new();
    // A parallel palette of top-level material textures (name -> `<texture filename>`), so a mesh
    // visual referencing a top-level material BY NAME resolves the diffuse texture onto its bake hint,
    // mirroring how `mat_colors` resolves the flat colour. Captured for BOTH color+texture and
    // texture-only top-level materials (the texture is a document-level drop either way, but the
    // side-channel carries it to the GLB bake).
    let mut mat_textures: BTreeMap<String, String> = BTreeMap::new();
    let mut mat_quarantine_idx: Vec<usize> = Vec::new();
    // walk top-level children in document order, tracking the index that aligns with raw_toplevel.
    for (idx, ch) in root.children.iter().enumerate() {
        if ch.tag != "material" {
            continue;
        }
        let name = match ch.get("name") {
            Some(n) => n.to_string(),
            None => {
                notes.push(
                    "top-level <material> without a name dropped (URDF requires a material name)"
                        .to_string(),
                );
                continue;
            }
        };
        if let Some(tex) = ch.find("texture").and_then(|t| t.get("filename")) {
            mat_textures.insert(name.clone(), tex.to_string());
        }
        let rgba = ch.find("color").and_then(|c| c.get("rgba"));
        if let Some(rgba) = rgba {
            doc.color.push(material_to_color(ch));
            mat_colors.insert(name.clone(), rgba.to_string());
            if ch.find("texture").is_some() {
                notes.push(format!(
                    "material {}: <texture> dropped, flat color kept (bake to GLB)",
                    repr_str(&name)
                ));
            }
        } else {
            mat_quarantine_idx.push(idx);
            notes.push(format!(
                "material {}: texture-based (no flat rgba) -> quarantined to <extension domain='org.urdf.material'> for exact round-trip / GLB bake",
                repr_str(&name)
            ));
        }
    }
    for link in root.find_all("link") {
        doc.comp.push(link_to_comp(
            link,
            &mut notes,
            &mat_colors,
            &mat_textures,
            &mut hints,
        ));
    }
    for j in root.find_all("joint") {
        doc.joint.push(joint(j, &mut notes)?);
    }
    // Real-world tolerance: a malformed URDF can reference a link no `<link>` declares (see
    // `synthesize_missing_endpoint_comps`); mint an empty stub comp for each so the joint edge resolves
    // rather than dangling and breaking the kinematic tree.
    synthesize_missing_endpoint_comps(&mut doc, &mut notes);

    // Transmission front end: import each top-level <transmission> into the TYPED
    // core <transmission> (doc-level) rather than the old org.ros.control verbatim blob. The consumed
    // indices are dropped from the verbatim quarantine below.
    let consumed_transmissions = extract_transmissions(&root, &mut doc, &mut notes);

    // Gazebo front end: reach into each top-level
    // <gazebo reference=...> block, decompose its <sensor> children AND its <mu1>/<mu2>/<kp>/<kd>
    // friction idiom through the shared SDFormat mappers, and attach the typed sensors + collision
    // <surface> to the comp named by @reference. The block's residual (non-sensor/non-friction content +
    // un-mapped bits) is returned per top-level index to override the verbatim quarantine below; an empty
    // residual means the block was fully consumed (dropped from the quarantine).
    let gazebo_residual = extract_gazebo(&root, &raw_toplevel, &mut doc, &mut notes)?;

    // Native URDF <sensor> front end: decompose each robot-level <sensor> (urdf.xsd:401)
    // into a TYPED optical sensor on the comp named by its <parent link>. This is ADD-ONLY; the
    // <sensor> is NOT consumed from the quarantine below (it is neither typed-core nor a decomposed
    // gazebo/transmission index), so it STILL rides the verbatim `urdf:sensor` quarantine and the URDF
    // round-trip stays byte-identical; `to_urdf` suppresses the duplicate `<gazebo>` re-emit by @name.
    extract_native_sensors(&root, &mut doc, &mut notes);

    // quarantine pass: move every non-typed top-level child into <extension> by domain, verbatim.
    let mut quarantine: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, ch) in root.children.iter().enumerate() {
        if TYPED_TOPLEVEL.contains(&ch.tag.as_str()) {
            continue;
        }
        // a <transmission> adopted into the typed core is dropped from the verbatim quarantine.
        if consumed_transmissions.contains(&idx) {
            continue;
        }
        // a fully-consumed <gazebo> block (all <sensor>s typed, no residual) is dropped from the quarantine.
        if gazebo_residual.get(&idx).is_some_and(String::is_empty) {
            continue;
        }
        quarantine.entry(domain_for(&ch.tag)).or_default().push(idx);
    }
    if !mat_quarantine_idx.is_empty() {
        quarantine
            .entry("org.urdf.material".to_string())
            .or_default()
            .extend(&mat_quarantine_idx);
    }
    for (domain, idxs) in &quarantine {
        let mut ext = Extension {
            domain: domain.clone(),
            ..Default::default()
        };
        // splice the verbatim raw bodies, indented to match the document writer's two-space inner level.
        // A <gazebo> block whose <sensor>s were re-rooted contributes its RESIDUAL (sensors removed)
        // instead of its verbatim raw body (empty residuals were already filtered out of the quarantine).
        let mut body = String::new();
        for &i in idxs {
            if let Some(residual) = gazebo_residual.get(&i) {
                body.push_str(residual);
            } else if let Some(raw) = raw_toplevel.get(&i) {
                // ros2_control is re-homed under the typed extension root <ros2-control> (schema
                // hcdf-ext-ros2-control.xsd) so the body is schema-typed/validatable rather than an
                // opaque blob; every other domain keeps its verbatim body. to_urdf renames it back.
                if domain.as_str() == ROS2_CONTROL_DOMAIN {
                    body.push_str(&rename_root_element(
                        raw,
                        ROS2_CONTROL_URDF_ROOT,
                        ROS2_CONTROL_TYPED_ROOT,
                    )?);
                } else {
                    body.push_str(raw);
                }
            }
        }
        ext.body = body;
        doc.extension.push(ext);
        let mut kinds: Vec<&str> = idxs
            .iter()
            .filter_map(|&i| root.children.get(i).map(|c| c.tag.as_str()))
            .collect();
        kinds.sort();
        kinds.dedup();
        notes.push(format!(
            "quarantined {} top-level <{}> element(s) into <extension domain={}>",
            idxs.len(),
            kinds.join("/"),
            repr_str(domain)
        ));
    }

    Ok((doc, notes, hints))
}
