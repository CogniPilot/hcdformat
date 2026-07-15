//! HCDF -> SDF export (feature = `sdf`), a faithful port of `sdf_io/to_sdf.py`.
//!
//! The reverse of [`crate::from_sdf`]: walk the typed HCDF DOM and emit SDFormat as a string (quick-xml
//! writer). SDF is a *wider* peer than URDF, so this export is cleaner than HCDF->URDF: SDF natively
//! expresses closed kinematic LOOPS, the ball/universal/screw joint types, ALL HCDF geometry primitives
//! (capsule/cone/ellipsoid), and collision `<surface>` physics; none of which survive HCDF->URDF. The
//! HCDF *cyber* layer (motors/networks/power/sensors, accel/jerk limits) still has no SDF home -> it is
//! recorded in the same structured [`LossManifest`] the URDF path uses.
//!
//! Frames: SDF is FLU/ENU. A `body-frame="FRD"` document has its body poses and joint axes converted to
//! FLU on export (t'=C·t, R'=C·R·Cᵀ, a'=C·a); `world-frame="NED"` is recorded as a loss (not applied),
//! exactly as on the URDF path. The frame math reuses the shared [`crate::compose`] pose helpers.
//!
//! Link placement: URDF/HCDF position links THROUGH joints; SDF positions them through link poses. So a
//! child link gets `<pose relative_to="<its joint>">0 0 0 0 0 0</pose>` and the joint carries the
//! parent->child offset (`<pose relative_to="<parent>">`), mirroring `to_sdf.py`.
//!
//! The emitted SDF is final; there is NO `gz` subprocess in this crate. A caller that wants the output
//! spec-canonicalized (defaults filled, floats reformatted) runs `gz sdf --print` on it externally.
use crate::compose::pose_math::{mat3_vec, mul33, transpose, FRD_FLU};
use crate::compose::{matrix_to_pose, pose_to_matrix};
use crate::error::{Error, Result};
use crate::model::enums::{
    BodyFrame, Handedness, HmiType, JointType, PitchConvention, TransmissionType, WorldFrame,
};
use crate::model::{
    Collision, CollisionGeometry, Color, Comp, Hcdf, HmiElement, Joint, Pose, Surface,
    Transmission, Visual, VisualAppearance, VisualGeometry,
};
use crate::pyrepr::{repr_enum, repr_opt, repr_str};
pub use crate::to_urdf::LossManifest;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;
use std::collections::BTreeMap;
use std::io::Cursor;

/// HCDF joint type -> SDF joint type. SDF keeps ball/universal/screw (URDF cannot); only `free`/`planar`
/// and `cylindrical` have no SDF home (mirrors `to_sdf.py::_JTYPE_OUT`). SDFormat has NO `continuous`
/// joint type (libsdformat rejects the model at load); a continuous joint IS an unlimited `revolute`,
/// so it maps to `revolute` here and the caller SUPPRESSES the `<limit>` lower/upper bounds.
fn jtype_out(jt: JointType) -> Option<&'static str> {
    match jt {
        JointType::Revolute => Some("revolute"),
        JointType::Continuous => Some("revolute"),
        JointType::Prismatic => Some("prismatic"),
        JointType::Fixed => Some("fixed"),
        JointType::Ball => Some("ball"),
        JointType::Universal => Some("universal"),
        JointType::Screw => Some("screw"),
        _ => None,
    }
}

/// A gear `<transmission>` that couples two `<joint>` endpoints (a `role="driven"` member geared against
/// a `role="reference"` body) is exactly an SDF `<joint type="gearbox">`; it round-trips there (the
/// reverse of [`crate::from_sdf`]'s gearbox mapping). Other transmissions (simple/differential
/// reductions, motor-driven gears) have no SDF home and are dropped with a note.
fn is_sdf_gearbox(tr: &Transmission) -> bool {
    tr.type_ == Some(TransmissionType::Gear) && tr.joint.len() >= 2
}

/// Format a float compactly: an integer-valued float as a bare integer, else default float text. Matches
/// the document writer / [`crate::to_urdf`]'s `fmt3` so SDF leaf text stays clean (`0` not `0.0`).
fn fmt1(x: f64) -> String {
    if x.fract() == 0.0 && x.is_finite() {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

fn fmt3(v: [f64; 3]) -> String {
    v.iter().map(|x| fmt1(*x)).collect::<Vec<_>>().join(" ")
}

// ── XML writer helpers (SDF leaves are element TEXT, not attributes) ──────────────────────────────

fn start_with(name: &str, attrs: &[(&str, &str)]) -> BytesStart<'static> {
    let mut e = BytesStart::new(name.to_string());
    for (k, v) in attrs {
        e.push_attribute((*k, *v));
    }
    e
}

fn w_start(w: &mut Writer<Cursor<Vec<u8>>>, name: &str, attrs: &[(&str, &str)]) -> Result<()> {
    w.write_event(Event::Start(start_with(name, attrs)))
        .map_err(|e| Error::Xml(e.to_string()))
}

fn w_end(w: &mut Writer<Cursor<Vec<u8>>>, name: &str) -> Result<()> {
    w.write_event(Event::End(BytesEnd::new(name.to_string())))
        .map_err(|e| Error::Xml(e.to_string()))
}

/// A value-bearing leaf, emitted ONLY when `value` is `Some` (libsdformat rejects empty typed elements,
/// so optional scalars are omitted, not blanked); mirrors `to_sdf.py::_leaf`.
fn w_leaf(w: &mut Writer<Cursor<Vec<u8>>>, name: &str, value: Option<&str>) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    w.write_event(Event::Start(BytesStart::new(name.to_string())))
        .map_err(|e| Error::Xml(e.to_string()))?;
    w.write_event(Event::Text(BytesText::new(value)))
        .map_err(|e| Error::Xml(e.to_string()))?;
    w_end(w, name)
}

/// Emit a joint `<dynamics>` block (damping/friction/spring_*). Shared by the primary `<axis>` and the
/// second-DOF `<axis2>` so both round-trip identically.
/// Whether an HMI element is a scene light (`@type="led-illumination"`), the one HMI type with an SDF
/// home (a `<light>`). All other HMI types are exporter losses.
fn is_light_hmi(hmi: &HmiElement) -> bool {
    hmi.type_ == Some(HmiType::LedIllumination)
}

fn write_dynamics(
    w: &mut Writer<Cursor<Vec<u8>>>,
    dyn_: &crate::model::JointDynamics,
) -> Result<()> {
    w_start(w, "dynamics", &[])?;
    w_leaf(w, "damping", dyn_.damping.as_deref())?;
    w_leaf(w, "friction", dyn_.friction.as_deref())?;
    w_leaf(w, "spring_stiffness", dyn_.spring_stiffness.as_deref())?;
    w_leaf(w, "spring_reference", dyn_.spring_reference.as_deref())?;
    w_end(w, "dynamics")
}

/// A `<pose ...>text</pose>` element (always emitted; the caller supplies the text + optional
/// `relative_to`).
fn w_pose_text(
    w: &mut Writer<Cursor<Vec<u8>>>,
    text: &str,
    relative_to: Option<&str>,
) -> Result<()> {
    let attrs: Vec<(&str, &str)> = match relative_to {
        Some(r) => vec![("relative_to", r)],
        None => vec![],
    };
    w.write_event(Event::Start(start_with("pose", &attrs)))
        .map_err(|e| Error::Xml(e.to_string()))?;
    w.write_event(Event::Text(BytesText::new(text)))
        .map_err(|e| Error::Xml(e.to_string()))?;
    w_end(w, "pose")
}

// ── exporter ──────────────────────────────────────────────────────────────────────────────────────

/// Per-link simulation scalars keyed by link (comp) name, then by typed element name
/// (e.g. `gravity`, `self-collide`, `velocity-decay/linear`).
type LinkPhysicsMap = BTreeMap<String, BTreeMap<String, String>>;

struct Exporter<'a> {
    doc: &'a Hcdf,
    loss: LossManifest,
    c: Option<[[f64; 3]; 3]>, // Some(C) when body-frame is FRD; None for FLU.
    /// which tree joint places each comp (loop closures do not position links).
    child_joint: BTreeMap<String, String>,
    /// Sensor-name -> runtime topic, harvested from the org.ros2 topic-map extension, so a contact
    /// sensor re-emits its `<topic>` (contact.xsd requires it) on export.
    sensor_topics: BTreeMap<String, String>,
    /// Model-scope sim scalars (typed element name -> value) from the org.gazebosim <model-physics>.
    model_physics: BTreeMap<String, String>,
    /// Per-link sim scalars (link name -> typed element name -> value) from <link-physics>.
    link_physics: LinkPhysicsMap,
}

impl<'a> Exporter<'a> {
    fn new(doc: &'a Hcdf) -> Self {
        let c = if doc.body_frame == Some(BodyFrame::FRD) {
            Some(FRD_FLU)
        } else {
            None
        };
        let mut child_joint = BTreeMap::new();
        for j in &doc.joint {
            if j.loop_.is_none() {
                if let (Some(child), Some(name)) = (&j.child, &j.name) {
                    if let Some(comp) = &child.comp {
                        if !comp.is_empty() && !name.is_empty() {
                            child_joint.insert(comp.clone(), name.clone());
                        }
                    }
                }
            }
        }
        let (model_physics, link_physics) = Self::parse_gazebo_physics(doc);
        Exporter {
            doc,
            loss: LossManifest::default(),
            c,
            child_joint,
            sensor_topics: Self::parse_sensor_topics(doc),
            model_physics,
            link_physics,
        }
    }

    /// Parse the org.gazebosim `<gazebo-sim>` body into (model-physics scalars, per-link-physics
    /// scalars). The inverse of [`crate::from_sdf`]'s `gazebo_model_physics_body`/`gazebo_link_physics_body`.
    /// Keys are the typed (kebab-case) element names; velocity-decay nests as `velocity-decay/linear`
    /// and `velocity-decay/angular`.
    fn parse_gazebo_physics(doc: &Hcdf) -> (BTreeMap<String, String>, LinkPhysicsMap) {
        use quick_xml::reader::Reader;
        let mut model_physics = BTreeMap::new();
        let mut link_physics: LinkPhysicsMap = BTreeMap::new();
        for ext in &doc.extension {
            if ext.domain != crate::to_urdf::GAZEBO_DOMAIN {
                continue;
            }
            let mut reader = Reader::from_str(&ext.body);
            reader.config_mut().expand_empty_elements = false;
            let mut path: Vec<String> = Vec::new();
            let mut cur_link: Option<String> = None;
            while let Ok(ev) = reader.read_event() {
                match ev {
                    Event::Start(e) => {
                        let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                        if name == "link-physics" {
                            cur_link = e
                                .attributes()
                                .flatten()
                                .find(|a| a.key.as_ref() == b"name")
                                .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()));
                        }
                        path.push(name);
                    }
                    Event::Text(t) => {
                        let text = t.unescape().map(|v| v.into_owned()).unwrap_or_default();
                        if text.trim().is_empty() {
                            continue;
                        }
                        let p: Vec<&str> = path.iter().map(String::as_str).collect();
                        match p.as_slice() {
                            ["gazebo-sim", "model-physics", leaf] => {
                                model_physics.insert((*leaf).to_string(), text);
                            }
                            ["gazebo-sim", "link-physics", leaf] if *leaf != "velocity-decay" => {
                                if let Some(l) = &cur_link {
                                    link_physics
                                        .entry(l.clone())
                                        .or_default()
                                        .insert((*leaf).to_string(), text);
                                }
                            }
                            ["gazebo-sim", "link-physics", "velocity-decay", leaf] => {
                                if let Some(l) = &cur_link {
                                    link_physics
                                        .entry(l.clone())
                                        .or_default()
                                        .insert(format!("velocity-decay/{leaf}"), text);
                                }
                            }
                            _ => {}
                        }
                    }
                    Event::End(_) => {
                        if path.last().map(String::as_str) == Some("link-physics") {
                            cur_link = None;
                        }
                        path.pop();
                    }
                    Event::Eof => break,
                    _ => {}
                }
            }
        }
        (model_physics, link_physics)
    }

    /// Parse the org.ros2 topic-map extension body into a sensor-name -> topic lookup. Mirrors the
    /// inverse of [`crate::from_sdf`]'s `ros2_topic_extension` (which serializes `<topic sensor= name=>`
    /// entries). A `<topic>` bound to a `motor=` (not `sensor=`) is ignored here.
    fn parse_sensor_topics(doc: &Hcdf) -> BTreeMap<String, String> {
        use quick_xml::reader::Reader;
        let mut map = BTreeMap::new();
        for ext in &doc.extension {
            if ext.domain != crate::to_urdf::ROS2_DOMAIN {
                continue;
            }
            let mut reader = Reader::from_str(&ext.body);
            reader.config_mut().expand_empty_elements = false;
            while let Ok(ev) = reader.read_event() {
                let e = match &ev {
                    Event::Start(e) | Event::Empty(e) => e,
                    Event::Eof => break,
                    _ => continue,
                };
                if e.name().as_ref() != b"topic" {
                    continue;
                }
                let mut sensor = None;
                let mut name = None;
                for attr in e.attributes().flatten() {
                    match attr.key.as_ref() {
                        b"sensor" => {
                            sensor = attr.unescape_value().ok().map(|v| v.into_owned());
                        }
                        b"name" => name = attr.unescape_value().ok().map(|v| v.into_owned()),
                        _ => {}
                    }
                }
                if let (Some(s), Some(n)) = (sensor, name) {
                    map.insert(s, n);
                }
            }
        }
        map
    }

    /// Apply the body change-of-basis to a joint axis direction vector (`a' = C·a`).
    fn conv_axis(&self, xyz: &str) -> String {
        match self.c {
            None => xyz.to_string(),
            Some(c) => {
                let mut v = [0.0; 3];
                for (i, t) in xyz.split_whitespace().take(3).enumerate() {
                    v[i] = t.parse().unwrap_or(0.0);
                }
                fmt3(mat3_vec(&c, v))
            }
        }
    }

    /// An HCDF pose -> SDF `<pose>` six-tuple text `x y z r p y` (quat->rpy, FRD->FLU). `None` only when
    /// the pose is absent (the caller decides whether to force `"0 0 0 0 0 0"`). Mirrors
    /// `to_sdf.py::_pose_text`.
    fn pose_text(&self, pose: Option<&Pose>) -> Option<String> {
        let pose = pose?;
        let needs_transform = self.c.is_some() || pose.quat.is_some();
        let (xyz, rpy) = if needs_transform {
            let m = pose_to_matrix(pose);
            let r = [
                [m[0][0], m[0][1], m[0][2]],
                [m[1][0], m[1][1], m[1][2]],
                [m[2][0], m[2][1], m[2][2]],
            ];
            let t = [m[0][3], m[1][3], m[2][3]];
            let (rp, tp) = match self.c {
                Some(c) => {
                    let ct = transpose(&c);
                    (mul33(&mul33(&c, &r), &ct), mat3_vec(&c, t))
                }
                None => (r, t),
            };
            let mut m2 = [[0.0; 4]; 4];
            for i in 0..3 {
                m2[i][..3].copy_from_slice(&rp[i]);
                m2[i][3] = tp[i];
            }
            m2[3][3] = 1.0;
            let out = matrix_to_pose(&m2);
            (fmt3(out.xyz_or_zero()), fmt3(out.rpy_or_zero()))
        } else {
            // No transform: echo the pose's xyz/rpy, defaulting an absent component to "0 0 0" (SDF
            // <pose> is always a full six-tuple, unlike URDF's presence-preserving <origin>).
            (
                pose.xyz.map(fmt3).unwrap_or_else(|| "0 0 0".to_string()),
                pose.rpy.map(fmt3).unwrap_or_else(|| "0 0 0".to_string()),
            )
        };
        Some(format!("{xyz} {rpy}"))
    }

    /// Emit a `<pose>` for `pose`, optionally `relative_to`. With `force`, an absent pose still emits the
    /// identity `0 0 0 0 0 0` (used for joint placement, which always needs a relative_to anchor).
    fn write_pose(
        &self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        pose: Option<&Pose>,
        relative_to: Option<&str>,
        force: bool,
    ) -> Result<()> {
        match self.pose_text(pose) {
            Some(t) => w_pose_text(w, &t, relative_to),
            None if force => w_pose_text(w, "0 0 0 0 0 0", relative_to),
            None => Ok(()),
        }
    }

    /// Emit a collision/visual `<geometry>` for any HCDF geometry. Returns false (and records a loss) if
    /// nothing representable. `allow_mesh` permits a collision `<mesh uri scale>`.
    fn write_geometry(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        geo: &GeoView,
        allow_mesh: bool,
        ctx: &str,
    ) -> Result<bool> {
        w_start(w, "geometry", &[])?;
        let ok = if let Some(b) = geo.box_ {
            w_start(w, "box", &[])?;
            w_leaf(w, "size", b)?;
            w_end(w, "box")?;
            true
        } else if let Some((radius, length)) = geo.cylinder {
            w_start(w, "cylinder", &[])?;
            w_leaf(w, "radius", radius)?;
            w_leaf(w, "length", length)?;
            w_end(w, "cylinder")?;
            true
        } else if let Some(r) = geo.sphere {
            w_start(w, "sphere", &[])?;
            w_leaf(w, "radius", r)?;
            w_end(w, "sphere")?;
            true
        } else if let Some((radius, length)) = geo.capsule {
            w_start(w, "capsule", &[])?;
            w_leaf(w, "radius", radius)?;
            w_leaf(w, "length", length)?;
            w_end(w, "capsule")?;
            true
        } else if let Some((radius, length)) = geo.cone {
            w_start(w, "cone", &[])?;
            w_leaf(w, "radius", radius)?;
            w_leaf(w, "length", length)?;
            w_end(w, "cone")?;
            true
        } else if let Some(radii) = geo.ellipsoid {
            w_start(w, "ellipsoid", &[])?;
            w_leaf(w, "radii", radii)?;
            w_end(w, "ellipsoid")?;
            true
        } else if let Some((uri, scale)) = geo.mesh.filter(|_| allow_mesh) {
            w_start(w, "mesh", &[])?;
            w_leaf(w, "uri", uri)?;
            // `<submesh>`: select a single named sub-part of a multi-part mesh file (SDF mesh/submesh).
            if let Some((name, center)) = geo.submesh {
                w_start(w, "submesh", &[])?;
                w_leaf(w, "name", name)?;
                w_leaf(w, "center", center)?;
                w_end(w, "submesh")?;
            }
            if let Some(scale) = scale {
                if !scale.is_empty() {
                    w_leaf(w, "scale", Some(scale))?;
                }
            }
            w_end(w, "mesh")?;
            true
        } else {
            false
        };
        w_end(w, "geometry")?;
        if !ok {
            self.loss.add(
                "geometry",
                format!("{ctx}: no SDF-representable shape; dropped"),
            );
        }
        Ok(ok)
    }

    /// Emit a collision `<surface>` (friction/restitution/contact) when it carries any physics. Mirrors
    /// `to_sdf.py::_surface`.
    fn write_surface(&self, w: &mut Writer<Cursor<Vec<u8>>>, surf: &Surface) -> Result<()> {
        let has_friction = surf
            .friction
            .as_ref()
            .map(|f| f.static_.is_some() || f.dynamic.is_some() || f.friction_direction.is_some())
            .unwrap_or(false);
        let has_contact = surf
            .contact
            .as_ref()
            .map(|c| c.stiffness.is_some() || c.damping.is_some())
            .unwrap_or(false);
        if !has_friction && surf.restitution.is_none() && !has_contact {
            return Ok(());
        }
        w_start(w, "surface", &[])?;
        if has_friction {
            let f = surf.friction.as_ref().unwrap();
            w_start(w, "friction", &[])?;
            w_start(w, "ode", &[])?;
            w_leaf(w, "mu", f.static_.as_deref())?;
            // <mu2> is the SECOND-friction-direction coefficient, emitted FROM
            // friction-direction/@mu2 (its sole source; no duplicate). A genuine kinetic @dynamic has no
            // SDF <ode> leaf, so it is not emitted here. fdir1 + per-direction slip accompany mu2.
            if let Some(fd) = &f.friction_direction {
                w_leaf(w, "mu2", fd.mu2.as_deref())?;
                w_leaf(w, "fdir1", fd.fdir1.as_deref())?;
                w_leaf(w, "slip1", fd.slip1.as_deref())?;
                w_leaf(w, "slip2", fd.slip2.as_deref())?;
            }
            w_end(w, "ode")?;
            w_end(w, "friction")?;
        }
        if let Some(r) = &surf.restitution {
            w_start(w, "bounce", &[])?;
            w_leaf(w, "restitution_coefficient", Some(r))?;
            w_end(w, "bounce")?;
        }
        if has_contact {
            let c = surf.contact.as_ref().unwrap();
            w_start(w, "contact", &[])?;
            w_start(w, "ode", &[])?;
            w_leaf(w, "kp", c.stiffness.as_deref())?;
            w_leaf(w, "kd", c.damping.as_deref())?;
            w_end(w, "ode")?;
            w_end(w, "contact")?;
        }
        w_end(w, "surface")
    }

    /// A visual `<color>` -> SDF `<material><diffuse>`. A name-only ref is resolved to its rgba from the
    /// top-level palette (the shared name is not preserved). Mirrors `to_sdf.py::_color`.
    fn write_color(&mut self, w: &mut Writer<Cursor<Vec<u8>>>, color: &Color) -> Result<()> {
        let mut rgba = color.rgba.clone();
        if rgba.is_none() {
            if let Some(name) = &color.name {
                if let Some(palette) = self
                    .doc
                    .color
                    .iter()
                    .find(|c| c.name.as_deref() == Some(name) && c.rgba.is_some())
                {
                    rgba = palette.rgba.clone();
                    self.loss.add(
                        "material",
                        format!(
                            "color {}: SDF has no color palette; inlined rgba (the shared name is not preserved)",
                            repr_str(name)
                        ),
                    );
                }
            }
        }
        if let Some(rgba) = rgba {
            w_start(w, "material", &[])?;
            w_leaf(w, "diffuse", Some(&rgba))?;
            w_end(w, "material")?;
        }
        Ok(())
    }

    fn write_visual(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        comp: &Comp,
        v: &Visual,
    ) -> Result<()> {
        let vname = if v.name.is_empty() {
            format!("{}_visual", comp.name)
        } else {
            v.name.clone()
        };
        w_start(w, "visual", &[("name", &vname)])?;
        self.write_pose(w, v.pose.as_ref(), None, false)?;
        match &v.appearance {
            VisualAppearance::Model { model, .. } => {
                w_start(w, "geometry", &[])?;
                w_start(w, "mesh", &[])?;
                w_leaf(w, "uri", model.uri.as_deref())?;
                w_end(w, "mesh")?;
                w_end(w, "geometry")?;
                self.loss.add(
                    "visual",
                    format!(
                        "visual {}: GLB <model> -> SDF mesh ref; baked PBR/textures are not represented in SDF <material>",
                        repr_str(&v.name)
                    ),
                );
            }
            VisualAppearance::Primitive { geometry, color } => {
                if let Some(geo) = geometry {
                    if !self.write_geometry(
                        w,
                        &GeoView::visual(geo),
                        false,
                        &format!("visual {}", repr_str(&v.name)),
                    )? {
                        // geometry already recorded the loss; close the <visual> and bail this one.
                        w_end(w, "visual")?;
                        return Ok(());
                    }
                    if let Some(color) = color {
                        self.write_color(w, color)?;
                    }
                } else {
                    self.loss.add(
                        "visual",
                        format!("visual {}: no geometry; dropped", repr_str(&v.name)),
                    );
                    w_end(w, "visual")?;
                    return Ok(());
                }
            }
        }
        w_end(w, "visual")
    }

    fn write_collision(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        comp: &Comp,
        c: &Collision,
    ) -> Result<()> {
        let cname = c
            .name
            .clone()
            .unwrap_or_else(|| format!("{}_collision", comp.name));
        w_start(w, "collision", &[("name", &cname)])?;
        self.write_pose(w, c.pose.as_ref(), None, false)?;
        let geo_ok = match &c.geometry {
            Some(geo) => self.write_geometry(
                w,
                &GeoView::collision(geo),
                true,
                &format!("collision {}", repr_opt(c.name.as_deref())),
            )?,
            None => false,
        };
        if !geo_ok {
            self.loss.add(
                "collision",
                format!(
                    "collision {}: no representable geometry; dropped",
                    repr_opt(c.name.as_deref())
                ),
            );
            w_end(w, "collision")?;
            return Ok(());
        }
        if let Some(surf) = &c.surface {
            self.write_surface(w, surf)?;
        }
        w_end(w, "collision")
    }

    /// Emit a link's Gazebo simulation scalars (`<gravity>`/`<self_collide>`/`<kinematic>`/
    /// `<must_be_base_link>` + `<velocity_decay><linear>/<angular>`) from the parsed `<link-physics>`.
    fn write_link_physics(&self, w: &mut Writer<Cursor<Vec<u8>>>, name: &str) -> Result<()> {
        let Some(flags) = self.link_physics.get(name) else {
            return Ok(());
        };
        for (typed, sdf) in [
            ("gravity", "gravity"),
            ("self-collide", "self_collide"),
            ("kinematic", "kinematic"),
            ("must-be-base-link", "must_be_base_link"),
        ] {
            if let Some(v) = flags.get(typed) {
                w_leaf(w, sdf, Some(v))?;
            }
        }
        let linear = flags.get("velocity-decay/linear");
        let angular = flags.get("velocity-decay/angular");
        if linear.is_some() || angular.is_some() {
            w_start(w, "velocity_decay", &[])?;
            if let Some(l) = linear {
                w_leaf(w, "linear", Some(l))?;
            }
            if let Some(a) = angular {
                w_leaf(w, "angular", Some(a))?;
            }
            w_end(w, "velocity_decay")?;
        }
        Ok(())
    }

    fn write_link(&mut self, w: &mut Writer<Cursor<Vec<u8>>>, comp: &Comp) -> Result<()> {
        if comp.name == "world" {
            // SDF reserves 'world' as the implicit world frame; a model cannot declare a 'world' link.
            if comp.inertial.is_some() || !comp.visual.is_empty() || !comp.collision.is_empty() {
                self.loss.add(
                    "comp",
                    "comp 'world' carried geometry or inertia; dropped because SDF reserves 'world' as the implicit world frame",
                );
            }
            return Ok(());
        }
        let name = if comp.name.is_empty() {
            self.loss.add(
                "comp",
                "a comp has no name; emitted SDF <link> name synthesized ('unnamed_link') to keep the SDF valid",
            );
            "unnamed_link".to_string()
        } else {
            comp.name.clone()
        };
        w_start(w, "link", &[("name", &name)])?;
        // Place the link at its parent joint (the joint carries the offset; the child link is identity
        // relative to it).
        if let Some(jn) = self.child_joint.get(&comp.name) {
            w_pose_text(w, "0 0 0 0 0 0", Some(jn))?;
        }
        // Per-link sim scalars (gravity/self_collide/kinematic/must_be_base_link/velocity_decay)
        // from the org.gazebosim <link-physics> extension; the inverse of from_sdf's harvest.
        self.write_link_physics(w, &comp.name)?;
        if let Some(ip) = &comp.inertial {
            w_start(w, "inertial", &[])?;
            self.write_pose(w, ip.inertia_origin.as_ref(), None, false)?;
            w_leaf(w, "mass", ip.mass.as_deref())?;
            if let Some(inertia) = &ip.inertia {
                let vals: Vec<&str> = inertia.split_whitespace().collect();
                if vals.len() == 6 {
                    w_start(w, "inertia", &[])?;
                    for (k, v) in ["ixx", "ixy", "ixz", "iyy", "iyz", "izz"].iter().zip(&vals) {
                        w_leaf(w, k, Some(v))?;
                    }
                    w_end(w, "inertia")?;
                } else {
                    self.loss.add(
                        "inertial",
                        format!(
                            "comp {}: inertia {} is not 6 values; dropped",
                            repr_str(&comp.name),
                            repr_str(inertia)
                        ),
                    );
                }
            }
            w_end(w, "inertial")?;
        }
        for v in &comp.visual {
            self.write_visual(w, comp, v)?;
        }
        for c in &comp.collision {
            self.write_collision(w, comp, c)?;
        }
        // Typed sensors -> SDF `<link><sensor type=...>` (the inverse of the from_sdf re-root). Emitted
        // in-place; only the design-spec sub-fields with no SDFormat home stay in the loss manifest.
        for s in &comp.sensor {
            let sctx = format!("comp {}", repr_str(&comp.name));
            let topic = s
                .name
                .as_deref()
                .and_then(|n| self.sensor_topics.get(n))
                .map(String::as_str);
            crate::to_sensor::write_sensor(w, s, &sctx, self.c.as_ref(), &mut self.loss, topic)?;
        }
        // A led-illumination HMI is a scene light -> emit as a link-scope SDF <light> (the inverse of
        // from_sdf's light->comp.hmi mapping). Other HMI types have no SDF home and stay losses below.
        for hmi in &comp.hmi {
            self.write_hmi_light(w, comp, hmi)?;
        }
        self.record_comp_losses(comp);
        w_end(w, "link")
    }

    /// Emit a `led-illumination` [`HmiElement`] as a link-scope SDF `<light>` (type/pose/diffuse/specular/
    /// attenuation/direction/cast_shadows/intensity); the inverse of from_sdf's `sdf_light_hmi`. A
    /// non-illumination HMI is a no-op here (it stays a loss in `record_comp_losses`). The HMI
    /// `<geometry>` has no SDF `<light>` home and is noted.
    fn write_hmi_light(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        comp: &Comp,
        hmi: &HmiElement,
    ) -> Result<()> {
        if !is_light_hmi(hmi) {
            return Ok(());
        }
        let name = hmi.name.as_deref().unwrap_or("light");
        let illum = hmi.illumination.as_ref();
        let ltype = illum
            .and_then(|i| i.light_type.as_deref())
            .unwrap_or("point");
        w_start(w, "light", &[("name", name), ("type", ltype)])?;
        self.write_pose(w, hmi.pose.as_ref(), None, false)?;
        if let Some(i) = illum {
            w_leaf(w, "cast_shadows", i.cast_shadows.as_deref())?;
            w_leaf(w, "intensity", i.intensity.as_deref())?;
            w_leaf(w, "diffuse", i.diffuse.as_deref())?;
            w_leaf(w, "specular", i.specular.as_deref())?;
            if let Some(a) = &i.attenuation {
                w_start(w, "attenuation", &[])?;
                w_leaf(w, "range", a.range.as_deref())?;
                w_leaf(w, "linear", a.linear.as_deref())?;
                w_leaf(w, "constant", a.constant.as_deref())?;
                w_leaf(w, "quadratic", a.quadratic.as_deref())?;
                w_end(w, "attenuation")?;
            }
            w_leaf(w, "direction", i.direction.as_deref())?;
        }
        w_end(w, "light")?;
        if hmi.geometry.is_some() {
            self.loss.add(
                "comp",
                format!(
                    "comp {}: led-illumination HMI {} <geometry> dropped (no SDF <light> home)",
                    repr_str(&comp.name),
                    repr_str(hmi.name.as_deref().unwrap_or(""))
                ),
            );
        }
        Ok(())
    }

    /// HCDF-rich comp content with no SDF home (mirrors `to_sdf.py::_link`'s loss loops). Nothing is
    /// silently dropped; every cyber/metadata field is recorded.
    fn record_comp_losses(&mut self, comp: &Comp) {
        let lists: &[(usize, &str)] = &[
            // NB: comp.sensor is NOT here; typed sensors are emitted as SDF `<link><sensor>` by
            // write_link; only their unmappable sub-fields (recorded inside write_sensor) are losses.
            (comp.motor.len(), "motors"),
            // NB: comp.frame is NOT here; named frames are emitted as model-level SDF <frame> by
            // write_frames (SDF >=1.7 has a first-class <frame>); only @type/<description> are losses.
            // led-illumination HMIs are emitted as <light> by write_hmi_light; only the OTHER HMI
            // types (display/speaker/button/…) have no SDF home and remain losses here.
            (
                comp.hmi.iter().filter(|h| !is_light_hmi(h)).count(),
                "HMI elements",
            ),
            (comp.dynamic_surface.len(), "dynamic surfaces"),
            (comp.power_source.len(), "power sources"),
            (comp.port.len(), "ports"),
            (comp.antenna.len(), "antennas"),
            (comp.board.is_some() as usize, "boards"),
            (
                comp.operating_temp.is_some() as usize,
                "operating-temp blocks",
            ),
            (comp.switch.len(), "switches"),
            (comp.software.is_some() as usize, "software blocks"),
            (
                comp.discovered.is_some() as usize,
                "discovered-device blocks",
            ),
            (comp.extension.len(), "vendor extensions"),
        ];
        for (n, label) in lists {
            if *n > 0 {
                self.loss.add(
                    "comp",
                    format!(
                        "comp {}: {n} {label} dropped (no SDF home)",
                        repr_str(&comp.name)
                    ),
                );
            }
        }
        let scalars: &[(bool, &str)] = &[
            (comp.struct_type.is_some(), "struct-type"),
            (comp.ip_rating.is_some(), "ip-rating"),
            (comp.role.is_some(), "role"),
            (comp.hwid.is_some(), "hwid"),
        ];
        for (present, label) in scalars {
            if *present {
                self.loss.add(
                    "comp",
                    format!(
                        "comp {}: {label} dropped (no SDF home)",
                        repr_str(&comp.name)
                    ),
                );
            }
        }
        if comp.description.is_some() {
            self.loss.add(
                "annotation",
                format!(
                    "comp {}: <description> dropped (no SDF field)",
                    repr_str(&comp.name)
                ),
            );
        }
        if comp.urdf_compat.is_some() {
            self.loss.add(
                "comp",
                format!(
                    "comp {}: <urdf-compat> dropped (no SDF home)",
                    repr_str(&comp.name)
                ),
            );
        }
    }

    /// Re-emit each doc-level HCDF `<include>` as a model-scope SDF `<include>` (uri/name/pose/static/
    /// placement_frame child elements); the inverse of from_sdf's `find_all("include")` capture. The
    /// reference is carried, NOT resolved. `@sha` is an HCDF-only integrity hash with no SDF home (noted).
    fn write_includes(&mut self, w: &mut Writer<Cursor<Vec<u8>>>) -> Result<()> {
        for inc in &self.doc.include {
            w_start(w, "include", &[])?;
            w_leaf(w, "uri", inc.uri.as_deref())?;
            w_leaf(w, "name", inc.name.as_deref())?;
            w_leaf(w, "pose", inc.pose.as_deref())?;
            w_leaf(w, "static", inc.static_.as_deref())?;
            w_leaf(w, "placement_frame", inc.placement_frame.as_deref())?;
            w_end(w, "include")?;
            if inc.sha.is_some() {
                self.loss.add(
                    "include",
                    format!(
                        "<include {}>: @sha dropped (no SDF field; integrity hash is HCDF-only)",
                        repr_str(inc.uri.as_deref().unwrap_or(""))
                    ),
                );
            }
        }
        Ok(())
    }

    /// Re-emit the typed `org.gazebosim` `<gazebo-sim>` extension back into the SDF `<model>`; the exact
    /// inverse of [`crate::from_sdf`]'s `gazebo_sim_extension` harvest (which absorbs a model/world's
    /// `<plugin>`s into `<gazebo-sim>`). Each carried `<plugin name= filename=>…</plugin>` is forwarded
    /// VERBATIM as a model-scope `<plugin>` (model.xsd:103); since `from_sdf` reads
    /// `model.find_all("plugin")`, a plugin round-trips SDF->HCDF->SDF. The typed `<physics>` the
    /// extension may also carry is WORLD-scope in SDFormat (physics.xsd, `<world>`-only), and this
    /// exporter emits a bare `<model>` with no `<world>`, so `<physics>` has no valid model child home;
    /// it is recorded as a loss, NEVER emitted as a schema-invalid `<model><physics>`.
    fn write_gazebo_extension(&mut self, w: &mut Writer<Cursor<Vec<u8>>>) -> Result<()> {
        use quick_xml::reader::Reader;
        for ext in &self.doc.extension {
            if ext.domain != crate::to_urdf::GAZEBO_DOMAIN {
                continue;
            }
            let mut reader = Reader::from_str(&ext.body);
            reader.config_mut().expand_empty_elements = false;
            // Depth within the `<gazebo-sim>` body; its <plugin>/<physics> children sit at depth 1. A
            // <plugin> subtree is forwarded verbatim; a <physics> is only counted (world-scope, no home).
            let mut depth = 0usize;
            let mut in_plugin = false;
            let mut has_physics = false;
            loop {
                match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
                    Event::Start(e) => {
                        if depth == 1 && !in_plugin {
                            match e.name().as_ref() {
                                b"plugin" => in_plugin = true,
                                b"physics" => has_physics = true,
                                _ => {}
                            }
                        }
                        if in_plugin {
                            w.write_event(Event::Start(e.into_owned()))
                                .map_err(|e| Error::Xml(e.to_string()))?;
                        }
                        depth += 1;
                    }
                    Event::Empty(e) => {
                        if depth == 1 && !in_plugin {
                            match e.name().as_ref() {
                                b"plugin" => w
                                    .write_event(Event::Empty(e.into_owned()))
                                    .map_err(|e| Error::Xml(e.to_string()))?,
                                b"physics" => has_physics = true,
                                _ => {}
                            }
                        } else if in_plugin {
                            w.write_event(Event::Empty(e.into_owned()))
                                .map_err(|e| Error::Xml(e.to_string()))?;
                        }
                    }
                    Event::End(e) => {
                        depth = depth.saturating_sub(1);
                        if in_plugin {
                            w.write_event(Event::End(e.into_owned()))
                                .map_err(|e| Error::Xml(e.to_string()))?;
                            if depth == 1 {
                                in_plugin = false;
                            }
                        }
                    }
                    Event::Text(t) => {
                        if in_plugin {
                            w.write_event(Event::Text(t.into_owned()))
                                .map_err(|e| Error::Xml(e.to_string()))?;
                        }
                    }
                    Event::CData(c) => {
                        if in_plugin {
                            w.write_event(Event::CData(c.into_owned()))
                                .map_err(|e| Error::Xml(e.to_string()))?;
                        }
                    }
                    Event::Eof => break,
                    _ => {}
                }
            }
            if has_physics {
                self.loss.add(
                    "extension",
                    "org.gazebosim <physics> not re-emitted: SDFormat <physics> is world-scope and this exporter emits a bare <model> (no <world>)",
                );
            }
        }
        Ok(())
    }

    /// Emit each comp's named kinematic `<frame>` as a model-level SDF `<frame name attached_to>` with an
    /// optional comp-local `<pose>`, the exact inverse of [`crate::from_sdf`]'s `map_model_frame` (a
    /// model-level SDF `<frame>` attaches to its owner link's comp, posed comp-local). SDFormat >=1.7 has
    /// a first-class `<frame>` (hcdf.xsd:3704 is its HCDF home), so a named frame is NOT a loss; the old
    /// "no SDF home" drop was factually wrong. `attached_to` is the HCDF `@relative-to` target when set,
    /// else the owning comp (link); the HCDF frame pose is expressed in that same frame, which is SDF's
    /// default pose `relative_to == attached_to`, so no explicit pose `relative_to` is emitted. SDF
    /// `<frame>` carries only name/attached_to/pose, so the HCDF `@type` / `<description>` are the only
    /// residual losses.
    fn write_frames(&mut self, w: &mut Writer<Cursor<Vec<u8>>>) -> Result<()> {
        for comp in &self.doc.comp {
            for f in &comp.frame {
                let attached_to = f.relative_to.as_deref().unwrap_or(comp.name.as_str());
                if attached_to.is_empty() {
                    self.loss.add(
                        "comp",
                        format!(
                            "comp {}: <frame {}> dropped; no attachment name (comp is unnamed and the frame has no @relative-to)",
                            repr_str(&comp.name),
                            repr_str(&f.name)
                        ),
                    );
                    continue;
                }
                w_start(
                    w,
                    "frame",
                    &[("name", f.name.as_str()), ("attached_to", attached_to)],
                )?;
                self.write_pose(w, f.pose.as_ref(), None, false)?;
                w_end(w, "frame")?;
                if f.type_.is_some() {
                    self.loss.add(
                        "comp",
                        format!(
                            "comp {}: <frame {}> @type {} dropped (SDF <frame> has no type)",
                            repr_str(&comp.name),
                            repr_str(&f.name),
                            repr_opt(f.type_.as_deref())
                        ),
                    );
                }
                if f.description.is_some() {
                    self.loss.add(
                        "annotation",
                        format!(
                            "comp {}: <frame {}> <description> dropped (no SDF field)",
                            repr_str(&comp.name),
                            repr_str(&f.name)
                        ),
                    );
                }
            }
        }
        Ok(())
    }

    fn write_joint(&mut self, w: &mut Writer<Cursor<Vec<u8>>>, j: &Joint) -> Result<()> {
        let jname_repr = repr_opt(j.name.as_deref());
        // HCDF-only joint metadata with no SDF home, recorded for ALL joint types (incl. fixed).
        if j.description.is_some() {
            self.loss.add(
                "annotation",
                format!("joint {jname_repr}: <description> dropped (no SDF field)"),
            );
        }
        if j.calibration.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname_repr}: <calibration> dropped (no SDF home)"),
            );
        }
        if j.extension.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname_repr}: <extension> dropped (no SDF home)"),
            );
        }
        if j.urdf_compat.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname_repr}: <urdf-compat> dropped (no SDF home)"),
            );
        }
        // Ball swing-cone + twist have no SDF home; classic SDF ball carries no limits, and the modern
        // gz per-DoF ball limits are not emitted here. Recorded as a loss on the (valid) SDF ball.
        if j.swing_limit.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname_repr}: <swing_limit> (ball swing-cone) dropped (no SDF home for ball joint limits)"),
            );
        }
        if j.twist_limit.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname_repr}: <twist_limit> (ball twist bound) dropped (no SDF home for ball joint limits)"),
            );
        }

        let jtype = j.type_.unwrap_or(JointType::Fixed);
        // A continuous joint exports as an unlimited `revolute` (SDF has no `continuous` type); its
        // lower/upper bounds, if any, are dropped below since an unlimited revolute has none.
        let is_continuous = jtype == JointType::Continuous;
        let sdf_type = match jtype_out(jtype) {
            Some(t) => t,
            None => {
                self.loss.add(
                    "joint-type",
                    format!(
                        "joint {jname_repr}: type {} has no SDF equivalent; exported as 'fixed'",
                        repr_enum("JointType", &jtype.to_string())
                    ),
                );
                for (present, what) in [
                    (j.axis.is_some(), "<axis>"),
                    (j.limit.is_some(), "<limit>"),
                    // cylindrical carries its rotation bound in <limit2>; without an SDF home it is lost
                    // alongside <limit> in the ->fixed downgrade (named explicitly, not silently).
                    (j.limit2.is_some(), "<limit2>"),
                    (j.thread_pitch.is_some(), "thread_pitch"),
                ] {
                    if present {
                        self.loss.add(
                            "joint",
                            format!(
                                "joint {jname_repr}: {what} dropped with the {}->fixed downgrade",
                                jtype
                            ),
                        );
                    }
                }
                "fixed"
            }
        };
        let jname = match &j.name {
            Some(n) if !n.is_empty() => n.clone(),
            _ => {
                self.loss.add(
                    "joint",
                    "a joint has no name; emitted SDF <joint> name synthesized ('unnamed_joint') to keep the SDF valid",
                );
                "unnamed_joint".to_string()
            }
        };
        w_start(w, "joint", &[("name", &jname), ("type", sdf_type)])?;
        let parent_comp = j.parent.as_ref().and_then(|p| p.comp.clone());
        if let Some(parent) = &parent_comp {
            w_leaf(w, "parent", Some(parent))?;
        }
        if let Some(child) = j.child.as_ref().and_then(|c| c.comp.as_deref()) {
            w_leaf(w, "child", Some(child))?;
        }
        // The joint pose is the parent->child offset, relative to the parent frame. A joint anchored to
        // 'world' is offset relative to the model frame (SDF reserves 'world' as an implicit frame).
        let pframe = match parent_comp.as_deref() {
            Some("world") => Some("__model__".to_string()),
            Some(p) => Some(p.to_string()),
            None => None,
        };
        match &pframe {
            Some(p) => self.write_pose(w, j.origin.as_ref(), Some(p), true)?,
            None => self.write_pose(w, j.origin.as_ref(), None, false)?,
        }
        if sdf_type == "fixed" {
            return w_end(w, "joint");
        }
        // <axis> aggregates xyz + limit + dynamics + mimic (created lazily, then closed once at the end).
        let mut axis_open = false;
        let ensure_axis = |w: &mut Writer<Cursor<Vec<u8>>>, axis_open: &mut bool| -> Result<()> {
            if !*axis_open {
                w_start(w, "axis", &[])?;
                *axis_open = true;
            }
            Ok(())
        };
        if let Some(axis) = &j.axis {
            ensure_axis(w, &mut axis_open)?;
            w_leaf(
                w,
                "xyz",
                axis.xyz.as_deref().map(|x| self.conv_axis(x)).as_deref(),
            )?;
        }
        if let Some(lim) = &j.limit {
            ensure_axis(w, &mut axis_open)?;
            w_start(w, "limit", &[])?;
            if is_continuous {
                // Unlimited revolute: no lower/upper. A bound on a continuous joint is contradictory
                // (there shouldn't be one), drop it with a note rather than emit a bounded revolute.
                for (present, bound) in [
                    (lim.lower.is_some(), "lower"),
                    (lim.upper.is_some(), "upper"),
                ] {
                    if present {
                        self.loss.add(
                            "joint",
                            format!(
                                "joint {jname_repr}: continuous joint carried limit/{bound}; dropped (a continuous joint is an unlimited revolute)"
                            ),
                        );
                    }
                }
            } else {
                w_leaf(w, "lower", lim.lower.as_deref())?;
                w_leaf(w, "upper", lim.upper.as_deref())?;
            }
            w_leaf(w, "effort", lim.effort.as_deref())?;
            w_leaf(w, "velocity", lim.velocity.as_deref())?;
            w_end(w, "limit")?;
            for (present, extra) in [
                (lim.acceleration.is_some(), "acceleration"),
                (lim.jerk.is_some(), "jerk"),
                (lim.deceleration.is_some(), "deceleration"),
            ] {
                if present {
                    self.loss.add(
                        "joint",
                        format!("joint {jname_repr}: limit/{extra} dropped (no SDF field)"),
                    );
                }
            }
        }
        if let Some(dyn_) = &j.dynamics {
            ensure_axis(w, &mut axis_open)?;
            write_dynamics(w, dyn_)?;
        }
        if let Some(mim) = &j.mimic {
            // SDF 1.12 places <mimic> under <axis> and requires a <reference> child; HCDF Mimic has no
            // reference field, so default it to 0.
            ensure_axis(w, &mut axis_open)?;
            let attrs: Vec<(&str, &str)> = match mim.joint.as_deref() {
                Some(jt) => vec![("joint", jt)],
                None => vec![],
            };
            w_start(w, "mimic", &attrs)?;
            w_leaf(w, "reference", Some("0"))?;
            w_leaf(w, "multiplier", mim.multiplier.as_deref())?;
            w_leaf(w, "offset", mim.offset.as_deref())?;
            w_end(w, "mimic")?;
        }
        if axis_open {
            w_end(w, "axis")?;
        }
        // The second DOF. <axis2> aggregates xyz + its OWN limit2/dynamics2 (universal/revolute2).
        if j.axis2.is_some() || j.limit2.is_some() || j.dynamics2.is_some() {
            w_start(w, "axis2", &[])?;
            if let Some(axis2) = &j.axis2 {
                w_leaf(
                    w,
                    "xyz",
                    axis2.xyz.as_deref().map(|x| self.conv_axis(x)).as_deref(),
                )?;
            }
            if let Some(lim) = &j.limit2 {
                w_start(w, "limit", &[])?;
                w_leaf(w, "lower", lim.lower.as_deref())?;
                w_leaf(w, "upper", lim.upper.as_deref())?;
                w_leaf(w, "effort", lim.effort.as_deref())?;
                w_leaf(w, "velocity", lim.velocity.as_deref())?;
                w_end(w, "limit")?;
                for (present, extra) in [
                    (lim.acceleration.is_some(), "acceleration"),
                    (lim.jerk.is_some(), "jerk"),
                    (lim.deceleration.is_some(), "deceleration"),
                ] {
                    if present {
                        self.loss.add(
                            "joint",
                            format!("joint {jname_repr}: limit2/{extra} dropped (no SDF field)"),
                        );
                    }
                }
            }
            if let Some(dyn_) = &j.dynamics2 {
                write_dynamics(w, dyn_)?;
            }
            w_end(w, "axis2")?;
        }
        if sdf_type == "screw" {
            if let Some(tp) = &j.thread_pitch {
                // HCDF canonical pitch is m/rev, right-handed -> emit the MODERN gz <screw_thread_pitch>
                // (m/rev, +ve right-handed). If the stored value was authored in a non-canonical
                // convention (rad_per_m and/or left-handed), convert it to canonical first so the SDF is
                // always the modern reading. The SDF importer stores canonical (attrs omitted), so the
                // common path emits the value verbatim.
                let rad_per_m = j.pitch_convention == Some(PitchConvention::RadPerM);
                let left = j.handedness == Some(Handedness::Left);
                let emitted = match (rad_per_m, tp.trim().parse::<f64>()) {
                    // rad/m -> m/rev = -2π/value (the sign flip also carries the handedness convention
                    // flip; an explicit left handedness would double-flip back, so fold both here).
                    (true, Ok(v)) if v != 0.0 => {
                        let mag = -std::f64::consts::TAU / v;
                        fmt1(if left { -mag } else { mag })
                    }
                    // m/rev but left-handed -> negate for the right-handed canonical sign.
                    (false, Ok(v)) if left => fmt1(-v),
                    // canonical (or unparseable/degenerate) -> verbatim.
                    _ => tp.clone(),
                };
                if rad_per_m || left {
                    self.loss.add(
                        "joint",
                        format!(
                            "joint {jname_repr}: non-canonical @thread_pitch ({}{}) converted to canonical m/rev right-handed and emitted as <screw_thread_pitch>{emitted}</screw_thread_pitch>",
                            if rad_per_m { "rad_per_m" } else { "m_per_rev" },
                            if left { ", left-handed" } else { "" },
                        ),
                    );
                }
                w_leaf(w, "screw_thread_pitch", Some(&emitted))?;
            }
        }
        w_end(w, "joint")
    }

    /// Emit a gear `<transmission>` coupling back to an SDF `<joint type="gearbox">`, the reverse of
    /// [`crate::from_sdf`]'s gearbox mapping (`<gearbox_ratio>` <- `<reduction>`,
    /// `<gearbox_reference_body>` <- the `role="reference"` endpoint, `<child>` <- the `role="driven"`
    /// endpoint). The gearbox joint's driving-side `<parent>` was not carried on the 2-endpoint gear
    /// transmission, so it is omitted from the emitted `<joint>` and recorded as a loss.
    fn write_gearbox_transmission(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        tr: &Transmission,
    ) -> Result<()> {
        let name = match tr.name.as_deref() {
            Some(n) if !n.is_empty() => n,
            _ => {
                self.loss.add(
                    "transmission",
                    "a gear <transmission> has no name; the emitted SDF gearbox <joint> name is synthesized ('gearbox_joint')",
                );
                "gearbox_joint"
            }
        };
        // Endpoints are role-tagged (reference/driven) on import; fall back positionally (first driven,
        // second reference) for a hand-authored coupling that omitted the roles.
        let driven = tr
            .joint
            .iter()
            .find(|e| e.role.as_deref() == Some("driven"))
            .or_else(|| tr.joint.first());
        let reference = tr
            .joint
            .iter()
            .find(|e| e.role.as_deref() == Some("reference"))
            .or_else(|| tr.joint.get(1));
        w_start(w, "joint", &[("name", name), ("type", "gearbox")])?;
        if let Some(child) = driven.and_then(|e| e.ref_.as_deref()) {
            w_leaf(w, "child", Some(child))?;
        }
        w_leaf(w, "gearbox_ratio", tr.reduction.as_deref())?;
        if let Some(body) = reference.and_then(|e| e.ref_.as_deref()) {
            w_leaf(w, "gearbox_reference_body", Some(body))?;
        }
        w_end(w, "joint")?;
        self.loss.add(
            "transmission",
            format!(
                "transmission {}: emitted as SDF <joint type=\"gearbox\">; the coupling's driving (parent) side is not represented by the gear transmission and is omitted from the <joint>",
                repr_str(name)
            ),
        );
        Ok(())
    }

    fn build(&mut self, w: &mut Writer<Cursor<Vec<u8>>>) -> Result<()> {
        w_start(w, "sdf", &[("version", crate::from_sdf::SDF_VERSION)])?;
        let model_name = if self.doc.name.is_empty() {
            "model".to_string()
        } else {
            self.doc.name.clone()
        };
        // <model @canonical_link> is a MODEL ATTRIBUTE, so it must be set at model-open.
        let mut model_attrs: Vec<(&str, &str)> = vec![("name", &model_name)];
        if let Some(cl) = self.model_physics.get("canonical-link") {
            model_attrs.push(("canonical_link", cl.as_str()));
        }
        w_start(w, "model", &model_attrs)?;
        // Model-scope sim scalars (static/self_collide/allow_auto_disable/enable_wind) as <model>
        // children, the inverse of from_sdf's <model-physics> harvest.
        for (typed, sdf) in [
            ("static", "static"),
            ("self-collide", "self_collide"),
            ("allow-auto-disable", "allow_auto_disable"),
            ("enable-wind", "enable_wind"),
        ] {
            if let Some(v) = self.model_physics.get(typed) {
                w_leaf(w, sdf, Some(v))?;
            }
        }
        if self.doc.world_frame == Some(WorldFrame::NED) {
            self.loss.add(
                "world-frame",
                "document is world-frame NED; world-relative placements are NOT converted to SDF's ENU (body-frame poses ARE).",
            );
        }
        // document-level metadata SDF <model> has no field for.
        for (val, label) in [
            (self.doc.description.as_deref(), "<description>"),
            (self.doc.author.as_deref(), "author"),
            (self.doc.license.as_deref(), "license"),
            (self.doc.url.as_deref(), "url"),
            (
                if self.doc.version.is_empty() {
                    None
                } else {
                    Some(self.doc.version.as_str())
                },
                "version",
            ),
        ] {
            if let Some(val) = val {
                self.loss.add(
                    "annotation",
                    format!("document {label} {} dropped (no SDF field)", repr_str(val)),
                );
            }
        }
        if self.doc.comp.is_empty() {
            self.loss.add(
                "model",
                "document has no comps; an SDF <model> requires >=1 <link> (the emitted SDF will be rejected by libsdformat)",
            );
        }
        for comp in &self.doc.comp {
            self.write_link(w, comp)?;
        }
        // Named kinematic <frame>s (attached to their owning link) -> model-level SDF <frame> (SDF >=1.7
        // has a first-class <frame>, hcdf.xsd:3704 <-> from_sdf's map_model_frame; NOT a loss).
        self.write_frames(w)?;
        for j in &self.doc.joint {
            self.write_joint(w, j)?;
        }
        // A gear <transmission> coupling round-trips to an SDF <joint type="gearbox"> (the reverse of
        // from_sdf's gearbox mapping); emit those inside the <model>. Other transmissions have no SDF
        // home and are reported dropped below.
        for tr in &self.doc.transmission {
            if is_sdf_gearbox(tr) {
                self.write_gearbox_transmission(w, tr)?;
            }
        }
        // Doc-level <include> references re-emitted as model-scope SDF <include> (the inverse of
        // from_sdf's find_all("include") capture); the reference is carried, not resolved.
        self.write_includes(w)?;
        // Typed org.gazebosim plugins re-emitted as model-scope <plugin> (the inverse of from_sdf's
        // <gazebo-sim> harvest); its world-scope <physics> is loss-noted (no valid <model> home).
        self.write_gazebo_extension(w)?;
        w_end(w, "model")?;
        w_end(w, "sdf")?;
        // HCDF-only top-level content with no SDF-model home. Gear transmissions that were emitted as
        // gearbox <joint>s above are NOT counted here, only the residual (non-gearbox) transmissions.
        let dropped_transmissions = self
            .doc
            .transmission
            .iter()
            .filter(|t| !is_sdf_gearbox(t))
            .count();
        // The typed org.gazebosim extension is re-emitted (its <plugin>s become model-scope <plugin>s;
        // see write_gazebo_extension), so it is NOT a dropped extension; only OTHER domains are.
        let dropped_extensions = self
            .doc
            .extension
            .iter()
            .filter(|e| e.domain != crate::to_urdf::GAZEBO_DOMAIN)
            .count();
        let toplevel: &[(usize, &str)] = &[
            (self.doc.group.len(), "joint groups"),
            (self.doc.state.len(), "kinematic states"),
            (
                self.doc.link.len()
                    + self.doc.bus.len()
                    + self.doc.chain.len()
                    + self.doc.star.len()
                    + self.doc.ring.len()
                    + self.doc.mesh.len()
                    + self.doc.tree.len(),
                "networks",
            ),
            (dropped_transmissions, "HCDF transmissions"),
            (
                self.doc.self_collision_disable.is_some() as usize,
                "self-collision-disable blocks",
            ),
            (dropped_extensions, "extensions"),
        ];
        for (n, label) in toplevel {
            if *n > 0 {
                self.loss.add(
                    "top-level",
                    format!("document: {n} {label} dropped (no SDF-model home)"),
                );
            }
        }
        if !self.doc.color.is_empty() {
            self.loss.add(
                "top-level",
                format!(
                    "document: {} top-level <color>(s): SDF has no color palette; referenced colors are inlined per-visual",
                    self.doc.color.len()
                ),
            );
        }
        Ok(())
    }
}

/// A geometry view over either [`VisualGeometry`] or [`CollisionGeometry`] (so one writer serves both).
struct GeoView<'a> {
    box_: Option<Option<&'a str>>,
    cylinder: Option<(Option<&'a str>, Option<&'a str>)>,
    sphere: Option<Option<&'a str>>,
    capsule: Option<(Option<&'a str>, Option<&'a str>)>,
    cone: Option<(Option<&'a str>, Option<&'a str>)>,
    ellipsoid: Option<Option<&'a str>>,
    mesh: Option<(Option<&'a str>, Option<&'a str>)>,
    /// `<mesh><submesh>` selection: `(name, center)`, emitted as SDF `<submesh><name>/<center>`.
    submesh: Option<(Option<&'a str>, Option<&'a str>)>,
}

impl<'a> GeoView<'a> {
    fn visual(g: &'a VisualGeometry) -> Self {
        GeoView {
            box_: g.box_.as_ref().map(|b| b.size.as_deref()),
            cylinder: g
                .cylinder
                .as_ref()
                .map(|c| (c.radius.as_deref(), c.length.as_deref())),
            sphere: g.sphere.as_ref().map(|s| s.radius.as_deref()),
            capsule: g
                .capsule
                .as_ref()
                .map(|c| (c.radius.as_deref(), c.length.as_deref())),
            cone: g
                .cone
                .as_ref()
                .map(|c| (c.radius.as_deref(), c.length.as_deref())),
            ellipsoid: g.ellipsoid.as_ref().map(|e| e.radii.as_deref()),
            mesh: None,
            submesh: None,
        }
    }
    fn collision(g: &'a CollisionGeometry) -> Self {
        GeoView {
            box_: g.box_.as_ref().map(|b| b.size.as_deref()),
            cylinder: g
                .cylinder
                .as_ref()
                .map(|c| (c.radius.as_deref(), c.length.as_deref())),
            sphere: g.sphere.as_ref().map(|s| s.radius.as_deref()),
            capsule: g
                .capsule
                .as_ref()
                .map(|c| (c.radius.as_deref(), c.length.as_deref())),
            cone: g
                .cone
                .as_ref()
                .map(|c| (c.radius.as_deref(), c.length.as_deref())),
            ellipsoid: g.ellipsoid.as_ref().map(|e| e.radii.as_deref()),
            mesh: g
                .mesh
                .as_ref()
                .map(|m| (m.uri.as_deref(), m.scale.as_deref())),
            submesh: g
                .mesh
                .as_ref()
                .and_then(|m| m.submesh.as_ref())
                .map(|s| (s.name.as_deref(), s.center.as_deref())),
        }
    }
}

/// Export an HCDF model to SDFormat. Returns `(sdf_xml_string, LossManifest)`. Mirrors `to_sdf.py::to_sdf`.
pub fn to_sdf(doc: &Hcdf) -> Result<(String, LossManifest)> {
    let mut w = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    let mut ex = Exporter::new(doc);
    ex.build(&mut w)?;
    let xml =
        String::from_utf8(w.into_inner().into_inner()).map_err(|e| Error::Xml(e.to_string()))?;
    Ok((format!("{xml}\n"), ex.loss))
}
