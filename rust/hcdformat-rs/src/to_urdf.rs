//! HCDF -> URDF export (feature = `urdf`), a faithful port of `urdf_hcdf/to_urdf.py`.
//!
//! The reverse of [`crate::from_urdf`]: walk the typed HCDF DOM and emit URDF as a string (quick-xml
//! writer). HCDF is a superset of URDF, so export is inherently lossy; every dropped or approximated
//! construct is recorded in a structured [`LossManifest`].
//!
//! Frame handling: URDF is always FLU/ENU. If the document declares `body-frame="FRD"`, every
//! body-frame quantity is converted to FLU on the way out: pose translations/rotations (`t'=C·t`,
//! `R'=C·R·Cᵀ`) AND joint axis direction vectors (`a'=C·a`), with `C = diag(1, −1, −1)`. World-frame
//! `NED` is recorded as a loss (world-relative pose conversion is not applied here, matching Python).
//! URDF `<origin>` has no quaternion, so any pose using `quat` is emitted as `rpy` (the rotation is
//! preserved exactly).
//!
//! Reversal of the import quarantine: a quarantined `<extension>` is de-merged back to its top-level
//! URDF elements (the verbatim body is re-emitted), so URDF->HCDF->URDF reproduces them exactly.
use crate::compose::pose_math::{mat3_vec, mul33, transpose, FRD_FLU};
use crate::compose::{matrix_to_pose, pose_to_matrix};
use crate::error::{Error, Result};
use crate::model::enums::{BodyFrame, JointType, OpticalSensorType, TransmissionType, WorldFrame};
use crate::model::{
    CollisionGeometry, Color, Comp, Hcdf, Inertial, Joint, OpticalSensor, Pose, Sensor,
    Transmission, Visual, VisualAppearance, VisualGeometry,
};
use crate::pyrepr::{repr_enum, repr_opt, repr_str};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;
use std::io::Cursor;

/// One entry in the [`LossManifest`]: a `(category, detail)` pair, mirroring the Python tuple list.
pub type LossItem = (String, String);

/// Structured record of what HCDF content could not be represented in URDF.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LossManifest {
    pub items: Vec<LossItem>,
}

impl LossManifest {
    /// Append a `(category, detail)` loss.
    pub fn add(&mut self, category: &str, detail: impl Into<String>) {
        self.items.push((category.to_string(), detail.into()));
    }
    /// Total number of loss items.
    pub fn len(&self) -> usize {
        self.items.len()
    }
    /// True when nothing was lost.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Flat text form, one `[category] detail` per line (mirrors Python `LossManifest.text()`).
    pub fn text(&self) -> String {
        self.items
            .iter()
            .map(|(c, d)| format!("[{c}] {d}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
    /// Order-preserving grouping `{category: [detail, ...]}` (mirrors Python `categories()`).
    pub fn categories(&self) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for (c, d) in &self.items {
            if let Some(slot) = out.iter_mut().find(|(k, _)| k == c) {
                slot.1.push(d.clone());
            } else {
                out.push((c.clone(), vec![d.clone()]));
            }
        }
        out
    }

    /// The `{total, categories, items}` JSON value Python `LossManifest.to_dict()` builds: the total
    /// count, the insertion-order per-category grouping, and the flat item list. Backs [`Self::to_json`].
    pub(crate) fn to_json_value(&self) -> crate::report::Json {
        use crate::report::Json;
        let categories = self
            .categories()
            .into_iter()
            .map(|(c, ds)| (c, Json::Arr(ds.into_iter().map(Json::Str).collect())))
            .collect();
        let items = self
            .items
            .iter()
            .map(|(c, d)| {
                Json::Obj(vec![
                    ("category".to_string(), Json::Str(c.clone())),
                    ("detail".to_string(), Json::Str(d.clone())),
                ])
            })
            .collect();
        Json::Obj(vec![
            ("total".to_string(), Json::Uint(self.items.len())),
            ("categories".to_string(), Json::Obj(categories)),
            ("items".to_string(), Json::Arr(items)),
        ])
    }

    /// Structured JSON form (total / categories / items), byte-identical to Python
    /// `LossManifest.to_json()` (`json.dumps(..., indent=2)`, no trailing newline).
    pub fn to_json(&self) -> String {
        crate::report::dump(&self.to_json_value())
    }

    /// Human-readable markdown grouped by category, byte-identical to Python
    /// `LossManifest.markdown(title)`: a one-line summary, then one `## category (n)` section per
    /// category in SORTED category order (the details within a category keep insertion order). Ends
    /// with a single trailing newline. Pass [`DEFAULT_LOSS_TITLE`] for the default heading.
    pub fn markdown(&self, title: &str) -> String {
        let mut lines = vec![format!("# {title}"), String::new()];
        if self.items.is_empty() {
            lines.push(
                "_No losses: the document exports to URDF without dropping content._".to_string(),
            );
            return lines.join("\n") + "\n";
        }
        let mut cats = self.categories();
        let n_cats = cats.len();
        lines.push(format!(
            "**{} loss item(s)** across {n_cats} categor{}.",
            self.items.len(),
            if n_cats == 1 { "y" } else { "ies" }
        ));
        lines.push(String::new());
        cats.sort_by(|a, b| a.0.cmp(&b.0));
        for (c, ds) in &cats {
            lines.push(format!("## {c} ({})", ds.len()));
            lines.extend(ds.iter().map(|d| format!("- {d}")));
            lines.push(String::new());
        }
        lines.join("\n").trim_end().to_string() + "\n"
    }
}

/// The default heading Python `LossManifest.markdown()` uses when no title is passed. The arrow here
/// is a rightwards-arrow character (U+2192), matching the Python renderer's output exactly; it is not
/// an em-dash.
pub const DEFAULT_LOSS_TITLE: &str = "HCDF → URDF loss manifest";

/// HCDF [`JointType`] -> URDF joint-type literal. Returns `None` for HCDF-only types (reported as a
/// loss and exported as `fixed`).
fn jtype_out(jt: JointType) -> Option<&'static str> {
    match jt {
        JointType::Revolute => Some("revolute"),
        JointType::Continuous => Some("continuous"),
        JointType::Prismatic => Some("prismatic"),
        JointType::Fixed => Some("fixed"),
        JointType::Planar => Some("planar"),
        JointType::Free => Some("floating"),
        _ => None,
    }
}

/// Format a pose vector as URDF attribute text (`"x y z"`), compact integers like the document writer.
fn fmt3(v: [f64; 3]) -> String {
    v.iter()
        .map(|x| {
            if x.fract() == 0.0 && x.is_finite() {
                format!("{}", *x as i64)
            } else {
                format!("{x}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// HCDF -> URDF exporter, carrying the change-of-basis and accumulating losses.
struct Exporter<'a> {
    doc: &'a Hcdf,
    loss: LossManifest,
    c: Option<[[f64; 3]; 3]>, // Some(C) when body-frame is FRD; None for FLU.
}

impl<'a> Exporter<'a> {
    fn new(doc: &'a Hcdf) -> Self {
        let c = if doc.body_frame == Some(BodyFrame::FRD) {
            Some(FRD_FLU)
        } else {
            None
        };
        Exporter {
            doc,
            loss: LossManifest::default(),
            c,
        }
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

    /// Convert an HCDF pose to URDF (xyz, rpy) attribute strings, applying frame/quat conversion.
    /// Returns `None` when the pose has neither xyz nor rpy (so no `<origin>` is emitted).
    fn origin_attrs(&self, pose: &Pose) -> Option<(Option<String>, Option<String>)> {
        let needs_transform = self.c.is_some() || pose.quat.is_some();
        let (xyz, rpy) = if needs_transform {
            // t' = C·t, R' = C·R·Cᵀ via the pose matrix, then emit rpy (quat cleared), exactly Python's
            // transform_pose(..., use_quat=False).
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
            // A transformed pose carries explicit xyz + rpy (matrix_to_pose sets both), so emit both,
            // matching Python `transform_pose(..., use_quat=False)`.
            (Some(fmt3(out.xyz_or_zero())), Some(fmt3(out.rpy_or_zero())))
        } else {
            // No transform: echo exactly the attributes the HCDF pose carries (presence-preserving),
            // matching the Python exporter, which emits `xyz`/`rpy` only when the source pose has them
            // (an `<origin xyz="0 0 0.2"/>` round-trips with no fabricated `rpy="0 0 0"`).
            (pose.xyz.map(fmt3), pose.rpy.map(fmt3))
        };
        if xyz.is_none() && rpy.is_none() {
            None
        } else {
            Some((xyz, rpy))
        }
    }
}

// ── XML writer helpers ──────────────────────────────────────────────────────────────────────────

fn start_with(name: &str, attrs: &[(&str, Option<&str>)]) -> BytesStart<'static> {
    let mut e = BytesStart::new(name.to_string());
    for (k, v) in attrs {
        if let Some(v) = v {
            e.push_attribute((*k, *v));
        }
    }
    e
}

fn w_empty(
    w: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    attrs: &[(&str, Option<&str>)],
) -> Result<()> {
    w.write_event(Event::Empty(start_with(name, attrs)))
        .map_err(|e| Error::Xml(e.to_string()))
}

fn w_start(
    w: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    attrs: &[(&str, Option<&str>)],
) -> Result<()> {
    w.write_event(Event::Start(start_with(name, attrs)))
        .map_err(|e| Error::Xml(e.to_string()))
}

fn w_end(w: &mut Writer<Cursor<Vec<u8>>>, name: &str) -> Result<()> {
    w.write_event(Event::End(BytesEnd::new(name.to_string())))
        .map_err(|e| Error::Xml(e.to_string()))
}

/// Write escaped character content (a text leaf like `<type>…</type>` / `<mechanicalReduction>…`).
fn w_text(w: &mut Writer<Cursor<Vec<u8>>>, text: &str) -> Result<()> {
    w.write_event(Event::Text(BytesText::new(text)))
        .map_err(|e| Error::Xml(e.to_string()))
}

/// Write a text-content leaf `<name>value</name>`, or nothing when `value` is `None`.
fn w_leaf_opt(w: &mut Writer<Cursor<Vec<u8>>>, name: &str, value: Option<&str>) -> Result<()> {
    if let Some(v) = value {
        w_start(w, name, &[])?;
        w_text(w, v)?;
        w_end(w, name)?;
    }
    Ok(())
}

fn w_raw(w: &mut Writer<Cursor<Vec<u8>>>, raw: &str) -> Result<()> {
    if raw.is_empty() {
        return Ok(());
    }
    let mut reader = quick_xml::reader::Reader::from_str(raw);
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Eof => break,
            ev => w
                .write_event(ev.into_owned())
                .map_err(|e| Error::Xml(e.to_string()))?,
        }
    }
    Ok(())
}

impl Exporter<'_> {
    fn write_origin(&self, w: &mut Writer<Cursor<Vec<u8>>>, pose: Option<&Pose>) -> Result<()> {
        let Some(pose) = pose else { return Ok(()) };
        let Some((xyz, rpy)) = self.origin_attrs(pose) else {
            return Ok(());
        };
        w_empty(
            w,
            "origin",
            &[("xyz", xyz.as_deref()), ("rpy", rpy.as_deref())],
        )
    }

    /// Emit `<geometry>` for a visual/collision geometry. Returns false if unrepresentable in URDF.
    fn write_geometry_visual(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        geo: &VisualGeometry,
        ctx: &str,
    ) -> Result<bool> {
        if let Some(b) = &geo.box_ {
            w_start(w, "geometry", &[])?;
            w_empty(w, "box", &[("size", b.size.as_deref())])?;
            w_end(w, "geometry")?;
        } else if let Some(c) = &geo.cylinder {
            w_start(w, "geometry", &[])?;
            w_empty(
                w,
                "cylinder",
                &[
                    ("radius", c.radius.as_deref()),
                    ("length", c.length.as_deref()),
                ],
            )?;
            w_end(w, "geometry")?;
        } else if let Some(s) = &geo.sphere {
            w_start(w, "geometry", &[])?;
            w_empty(w, "sphere", &[("radius", s.radius.as_deref())])?;
            w_end(w, "geometry")?;
        } else if let Some(c) = &geo.capsule {
            // `<capsule radius length>` is standard URDF v1.2 (urdf.xsd:101-114); it shares the
            // `radius`/`length` attribute pair with `<cylinder>` and round-trips the import at :478.
            w_start(w, "geometry", &[])?;
            w_empty(
                w,
                "capsule",
                &[
                    ("radius", c.radius.as_deref()),
                    ("length", c.length.as_deref()),
                ],
            )?;
            w_end(w, "geometry")?;
        } else {
            // Only `<cone>`/`<ellipsoid>` genuinely lack a URDF primitive (SDF-only shapes urdf-rs rejects).
            let bad = if geo.cone.is_some() {
                "cone"
            } else if geo.ellipsoid.is_some() {
                "ellipsoid"
            } else {
                "empty"
            };
            self.loss.add(
                "geometry",
                format!("{ctx}: <{bad}> has no URDF primitive; {ctx} dropped"),
            );
            return Ok(false);
        }
        Ok(true)
    }

    fn write_geometry_collision(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        geo: &CollisionGeometry,
        ctx: &str,
    ) -> Result<bool> {
        if let Some(b) = &geo.box_ {
            w_start(w, "geometry", &[])?;
            w_empty(w, "box", &[("size", b.size.as_deref())])?;
            w_end(w, "geometry")?;
        } else if let Some(c) = &geo.cylinder {
            w_start(w, "geometry", &[])?;
            w_empty(
                w,
                "cylinder",
                &[
                    ("radius", c.radius.as_deref()),
                    ("length", c.length.as_deref()),
                ],
            )?;
            w_end(w, "geometry")?;
        } else if let Some(s) = &geo.sphere {
            w_start(w, "geometry", &[])?;
            w_empty(w, "sphere", &[("radius", s.radius.as_deref())])?;
            w_end(w, "geometry")?;
        } else if let Some(c) = &geo.capsule {
            // `<capsule radius length>` is standard URDF v1.2 (urdf.xsd:101-114); it shares the
            // `radius`/`length` attribute pair with `<cylinder>` and round-trips the import at :490.
            w_start(w, "geometry", &[])?;
            w_empty(
                w,
                "capsule",
                &[
                    ("radius", c.radius.as_deref()),
                    ("length", c.length.as_deref()),
                ],
            )?;
            w_end(w, "geometry")?;
        } else if let Some(m) = &geo.mesh {
            w_start(w, "geometry", &[])?;
            let scale = m.scale.as_deref().filter(|s| *s != "1 1 1");
            w_empty(
                w,
                "mesh",
                &[("filename", m.uri.as_deref()), ("scale", scale)],
            )?;
            w_end(w, "geometry")?;
        } else {
            // Only `<cone>`/`<ellipsoid>` genuinely lack a URDF primitive (SDF-only shapes urdf-rs rejects).
            let bad = if geo.cone.is_some() {
                "cone"
            } else if geo.ellipsoid.is_some() {
                "ellipsoid"
            } else {
                "empty"
            };
            self.loss.add(
                "geometry",
                format!("{ctx}: <{bad}> has no URDF primitive; {ctx} dropped"),
            );
            return Ok(false);
        }
        Ok(true)
    }

    fn write_material(&mut self, w: &mut Writer<Cursor<Vec<u8>>>, color: &Color) -> Result<()> {
        if color.description.is_some() {
            self.loss.add(
                "annotation",
                format!(
                    "color {}: description dropped (no URDF field)",
                    repr_opt(color.name.as_deref())
                ),
            );
        }
        // URDF material needs a name or an inline color; emit only if it has one.
        let has_color = color.rgba.is_some();
        if color.name.is_none() && !has_color {
            return Ok(());
        }
        if has_color {
            w_start(w, "material", &[("name", color.name.as_deref())])?;
            w_empty(w, "color", &[("rgba", color.rgba.as_deref())])?;
            w_end(w, "material")?;
        } else {
            w_empty(w, "material", &[("name", color.name.as_deref())])?;
        }
        Ok(())
    }

    fn write_inertial(&mut self, w: &mut Writer<Cursor<Vec<u8>>>, ip: &Inertial) -> Result<()> {
        w_start(w, "inertial", &[])?;
        if let Some(mass) = &ip.mass {
            w_empty(w, "mass", &[("value", Some(mass.as_str()))])?;
        }
        self.write_origin(w, ip.inertia_origin.as_ref())?;
        if let Some(inertia) = &ip.inertia {
            let vals: Vec<&str> = inertia.split_whitespace().collect();
            if vals.len() == 6 {
                let keys = ["ixx", "ixy", "ixz", "iyy", "iyz", "izz"];
                let attrs: Vec<(&str, Option<&str>)> = keys
                    .iter()
                    .zip(&vals)
                    .map(|(k, v)| (*k, Some(*v)))
                    .collect();
                w_empty(w, "inertia", &attrs)?;
            } else {
                self.loss.add(
                    "inertial",
                    format!(
                        "inertia tuple {} is not 6 values; dropped",
                        repr_str(inertia)
                    ),
                );
            }
        }
        w_end(w, "inertial")?;
        Ok(())
    }

    fn write_visual(&mut self, w: &mut Writer<Cursor<Vec<u8>>>, v: &Visual) -> Result<()> {
        // A synthesized (converter-generated) name is stripped on export so the URDF stays byte-clean; an
        // authored name round-trips. An empty name is also omitted.
        let name = if v.name_origin == Some(crate::model::enums::NameOrigin::Synthesized)
            || v.name.is_empty()
        {
            None
        } else {
            Some(v.name.as_str())
        };
        // Build the visual element. We must decide its content before opening the tag (origin first).
        w_start(w, "visual", &[("name", name)])?;
        self.write_origin(w, v.pose.as_ref())?;
        match &v.appearance {
            VisualAppearance::Model { model, .. } => {
                // ARM A: a baked GLB -> URDF mesh visual (appearance is in the asset, no material).
                w_start(w, "geometry", &[])?;
                w_empty(w, "mesh", &[("filename", model.uri.as_deref())])?;
                w_end(w, "geometry")?;
            }
            VisualAppearance::Primitive { geometry, color } => {
                if let Some(geo) = geometry {
                    if !self.write_geometry_visual(
                        w,
                        geo,
                        &format!("visual {}", repr_str(&v.name)),
                    )? {
                        // unrepresentable: close <visual> and return (matches Python early return)
                        w_end(w, "visual")?;
                        return Ok(());
                    }
                    if let Some(col) = color {
                        self.write_material(w, col)?;
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
        // (No color check for the Model arm: the typed model forbids a color alongside a <model>.)
        if v.toggle.is_some() {
            self.loss.add(
                "visual",
                format!(
                    "visual {}: toggle group {} dropped (URDF has no runtime show/hide grouping)",
                    repr_str(&v.name),
                    repr_opt(v.toggle.as_deref())
                ),
            );
        }
        w_end(w, "visual")?;
        Ok(())
    }

    fn write_collision(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        c: &crate::model::Collision,
    ) -> Result<()> {
        let name = if c.name_origin == Some(crate::model::enums::NameOrigin::Synthesized) {
            None
        } else {
            c.name.as_deref().filter(|s| !s.is_empty())
        };
        w_start(w, "collision", &[("name", name)])?;
        self.write_origin(w, c.pose.as_ref())?;
        let cname = repr_opt(c.name.as_deref());
        let ok = match &c.geometry {
            Some(geo) => self.write_geometry_collision(w, geo, &format!("collision {cname}"))?,
            None => false,
        };
        if !ok {
            self.loss.add(
                "collision",
                format!("collision {cname}: no representable geometry; dropped"),
            );
            w_end(w, "collision")?;
            return Ok(());
        }
        // NB: a collision <surface> is NOT dropped here. Gazebo-classic friction/contact is link-wide, so
        // the referenced link's friction round-trips through the `<gazebo reference><mu1>/<mu2>/<kp>/<kd>`
        // idiom emitted once per comp by `write_gazebo_friction` (the exact inverse of the import). Only the
        // surface bits with no Gazebo-classic home (restitution, kinetic @dynamic) are recorded there.
        // HCDF collision/@verbose (bool, hcdf.xsd:917) round-trips into the URDF `<verbose value=>` CHILD
        // element (urdf.xsd:161). URDF has no false form, so we emit only when true (an absent element
        // reads as false); a false/absent @verbose emits nothing and records no loss.
        if matches!(c.verbose.as_deref(), Some("true") | Some("1")) {
            w_empty(w, "verbose", &[("value", Some("true"))])?;
        }
        w_end(w, "collision")?;
        Ok(())
    }

    fn write_link(&mut self, w: &mut Writer<Cursor<Vec<u8>>>, comp: &Comp) -> Result<()> {
        let link_type = comp
            .urdf_compat
            .as_ref()
            .and_then(|u| u.link_type.as_deref());
        w_start(
            w,
            "link",
            &[("name", Some(comp.name.as_str())), ("type", link_type)],
        )?;
        if let Some(ip) = &comp.inertial {
            self.write_inertial(w, ip)?;
        }
        for v in &comp.visual {
            self.write_visual(w, v)?;
        }
        for c in &comp.collision {
            self.write_collision(w, c)?;
        }
        w_end(w, "link")?;
        // HCDF-rich constructs with no URDF home -> recorded in the manifest, never silently dropped.
        let ctx = format!("comp {}", repr_str(&comp.name));
        self.drop_list("comp", &ctx, "frames", comp.frame.len());
        // NB: comp.sensor is NOT dropped here; typed sensors are emitted as a `<gazebo reference>`
        // block by write_gazebo_sensors; only their unmappable sub-fields are recorded as losses.
        self.drop_list("comp", &ctx, "motors", comp.motor.len());
        self.drop_list("comp", &ctx, "HMI elements", comp.hmi.len());
        self.drop_list("comp", &ctx, "dynamic surfaces", comp.dynamic_surface.len());
        self.drop_list("comp", &ctx, "power sources", comp.power_source.len());
        self.drop_list("comp", &ctx, "ports", comp.port.len());
        self.drop_list("comp", &ctx, "antennas", comp.antenna.len());
        self.drop_list("comp", &ctx, "boards", usize::from(comp.board.is_some()));
        self.drop_list(
            "comp",
            &ctx,
            "operating-temp blocks",
            usize::from(comp.operating_temp.is_some()),
        );
        self.drop_list("comp", &ctx, "switches", comp.switch.len());
        self.drop_list(
            "comp",
            &ctx,
            "software blocks",
            usize::from(comp.software.is_some()),
        );
        self.drop_list(
            "comp",
            &ctx,
            "discovered-device blocks",
            usize::from(comp.discovered.is_some()),
        );
        self.drop_list("comp", &ctx, "vendor extensions", comp.extension.len());
        // HCDF-only comp metadata scalars with no URDF home. Python formats `{label}={val!r}`: string
        // values render single-quoted; the `role` enum renders `<CompRole.member: 'value'>` (enum repr).
        if let Some(v) = &comp.struct_type {
            self.loss.add(
                "comp",
                format!(
                    "{ctx}: struct-type={} dropped (HCDF-only; no URDF home)",
                    repr_str(v)
                ),
            );
        }
        if let Some(v) = &comp.ip_rating {
            self.loss.add(
                "comp",
                format!(
                    "{ctx}: ip-rating={} dropped (HCDF-only; no URDF home)",
                    repr_str(v)
                ),
            );
        }
        if let Some(v) = &comp.role {
            self.loss.add(
                "comp",
                format!(
                    "{ctx}: role={} dropped (HCDF-only; no URDF home)",
                    repr_enum("CompRole", &v.to_string())
                ),
            );
        }
        if let Some(v) = &comp.hwid {
            self.loss.add(
                "comp",
                format!(
                    "{ctx}: hwid={} dropped (HCDF-only; no URDF home)",
                    repr_str(v)
                ),
            );
        }
        if comp.description.is_some() {
            self.loss.add(
                "annotation",
                format!("{ctx}: <description> dropped (no URDF field)"),
            );
        }
        Ok(())
    }

    fn drop_list(&mut self, category: &str, ctx: &str, label: &str, n: usize) {
        if n > 0 {
            self.loss.add(
                category,
                format!("{ctx}: {n} {label} dropped (no URDF equivalent)"),
            );
        }
    }

    /// The typed core `<transmission>` -> a URDF `<transmission>` (the inverse of the import map):
    /// `@name` -> `@name`, `@type` -> `<type>transmission_interface/…Transmission</type>`,
    /// `<joint ref>` -> `<joint name=…/>`, `<motor ref>` -> `<actuator name=…>` carrying
    /// `<reduction>` as `<mechanicalReduction>`. HCDF-only fields with no URDF home
    /// (`efficiency`/`backlash`/`encoder`/`visual`/`mesh`/springs) are recorded as losses.
    fn write_transmission(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        tr: &Transmission,
    ) -> Result<()> {
        let Some(name) = tr.name.as_deref() else {
            self.loss.add(
                "transmission",
                "a <transmission> with no name was dropped (URDF requires a transmission name)"
                    .to_string(),
            );
            return Ok(());
        };
        w_start(w, "transmission", &[("name", Some(name))])?;
        // @type -> <type> class path. simple/differential have direct URDF classes; any richer HCDF
        // type (belt/gear/planetary/…) has no ros_control class, so it is generalized to Simple + a loss.
        let type_path = match &tr.type_ {
            Some(TransmissionType::Simple) => Some("transmission_interface/SimpleTransmission"),
            Some(TransmissionType::Differential) => {
                Some("transmission_interface/DifferentialTransmission")
            }
            Some(other) => {
                self.loss.add(
                    "transmission",
                    format!(
                        "transmission {}: type {} has no ros_control class; emitted as SimpleTransmission",
                        repr_str(name),
                        repr_enum("TransmissionType", &other.to_string())
                    ),
                );
                Some("transmission_interface/SimpleTransmission")
            }
            None => None,
        };
        if let Some(path) = type_path {
            w_start(w, "type", &[])?;
            w_text(w, path)?;
            w_end(w, "type")?;
        }
        // URDF ros_control transmissions are single-joint / single-actuator; emit the first endpoint of
        // each. A multi-endpoint HCDF coupling (a gearbox's reference+driven joints, a differential's
        // motor inputs + joint outputs) has no faithful URDF shape, so the extra endpoints are recorded
        // as a loss rather than emitted into an invalid transmission.
        if tr.joint.len() > 1 || tr.motor.len() > 1 {
            self.loss.add(
                "transmission",
                format!(
                    "transmission {}: multi-endpoint coupling ({} joint / {} motor endpoints) flattened to the first of each; URDF ros_control transmissions are single-joint/single-actuator",
                    repr_str(name),
                    tr.joint.len(),
                    tr.motor.len()
                ),
            );
        }
        if let Some(jref) = tr.joint.first().and_then(|e| e.ref_.as_deref()) {
            w_empty(w, "joint", &[("name", Some(jref))])?;
        }
        match (
            tr.motor.first().and_then(|e| e.ref_.as_deref()),
            tr.reduction.as_deref(),
        ) {
            (Some(aref), Some(red)) => {
                w_start(w, "actuator", &[("name", Some(aref))])?;
                w_start(w, "mechanicalReduction", &[])?;
                w_text(w, red)?;
                w_end(w, "mechanicalReduction")?;
                w_end(w, "actuator")?;
            }
            (Some(aref), None) => w_empty(w, "actuator", &[("name", Some(aref))])?,
            (None, Some(red)) => {
                // reduction with no motor endpoint -> the older transmission-level placement.
                w_start(w, "mechanicalReduction", &[])?;
                w_text(w, red)?;
                w_end(w, "mechanicalReduction")?;
            }
            (None, None) => {}
        }
        w_end(w, "transmission")?;
        // NATIVE-ONLY typed fields with no URDF home -> recorded, never silently dropped.
        let ctx = format!("transmission {}", repr_str(name));
        if tr.efficiency.is_some() {
            self.loss.add(
                "transmission",
                format!("{ctx}: efficiency dropped (no URDF field)"),
            );
        }
        if tr.backlash.is_some() {
            self.loss.add(
                "transmission",
                format!("{ctx}: backlash dropped (no URDF field)"),
            );
        }
        if let Some(v) = &tr.encoder {
            self.loss.add(
                "transmission",
                format!("{ctx}: encoder={} dropped (no URDF field)", repr_str(v)),
            );
        }
        if tr.visual.is_some() {
            self.loss.add(
                "transmission",
                format!("{ctx}: visual reference dropped (no URDF field)"),
            );
        }
        if tr.mesh.is_some() {
            self.loss.add(
                "transmission",
                format!("{ctx}: mesh reference dropped (no URDF field)"),
            );
        }
        Ok(())
    }

    /// Emit a comp's typed sensors, split by the prefer-native rule: a typed optical CAMERA or LIDAR
    /// with NO `urdf:sensor` quarantine (SDF-sourced or hand-authored) prefers a NATIVE robot-level URDF
    /// `<sensor>` (the only two sensor kinds with a native URDF grammar); every OTHER category
    /// (imu/gnss/mag/thermal/tof/force/fluid/…) has no native home and stays a `<gazebo reference><sensor>`
    /// block. A sensor already carried verbatim by a `urdf:sensor` quarantine round-trips through THAT (it
    /// is suppressed here), so every sensor is emitted EXACTLY once.
    fn write_comp_sensors(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        comp: &Comp,
        native_quarantined: &std::collections::BTreeSet<String>,
    ) -> Result<()> {
        // A sensor carried verbatim by a native `<extension domain="urdf:sensor">` quarantine round-trips
        // to URDF through that quarantine; re-emitting it here (native OR gazebo) would emit it TWICE.
        // Suppress those (matched by @name) and emit only the SDF-sourced / hand-authored sensors.
        let emit: Vec<&Sensor> = comp
            .sensor
            .iter()
            .filter(|s| {
                !s.name
                    .as_ref()
                    .is_some_and(|n| native_quarantined.contains(n))
            })
            .collect();
        if emit.is_empty() {
            return Ok(());
        }
        // Prefer-native: a typed optical camera/lidar splits off to a native `<sensor>`; the rest -> gazebo.
        let (native, gazebo): (Vec<&Sensor>, Vec<&Sensor>) =
            emit.into_iter().partition(|s| native_optical(s).is_some());
        self.write_gazebo_sensors(w, comp, &gazebo)?;
        self.write_native_sensors(w, comp, &native)?;
        Ok(())
    }

    /// The gazebo-bound subset of a comp's sensors -> a `<gazebo reference="link">` block of SDFormat
    /// `<sensor>`s (Gazebo reuses the SDFormat sensor grammar). This closes the round-trip for every sensor
    /// category WITHOUT a native URDF grammar. Sensors on an unnamed comp cannot attach to a `@reference`,
    /// so they are recorded as a loss instead.
    fn write_gazebo_sensors(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        comp: &Comp,
        emit: &[&Sensor],
    ) -> Result<()> {
        if emit.is_empty() {
            return Ok(());
        }
        if comp.name.is_empty() {
            self.loss.add(
                "comp",
                format!(
                    "{} sensors on an unnamed comp cannot attach to a <gazebo reference>; dropped",
                    emit.len()
                ),
            );
            return Ok(());
        }
        w_start(w, "gazebo", &[("reference", Some(comp.name.as_str()))])?;
        let ctx = format!("comp {}", repr_str(&comp.name));
        for s in emit {
            // URDF/Gazebo has no contact-sensor topic side-channel; pass no topic.
            crate::to_sensor::write_sensor(w, s, &ctx, self.c.as_ref(), &mut self.loss, None)?;
        }
        w_end(w, "gazebo")?;
        Ok(())
    }

    /// Prefer-native: emit each typed optical camera/lidar as a NATIVE robot-level URDF
    /// `<sensor>` (urdf.xsd:401, complexType 327-341), the EXACT inverse of the native `<sensor>` import
    /// ([`crate::from_urdf`]'s `extract_native_sensors`): `@name`/`@update_rate`, the optical pose as
    /// `<origin>`, the `<parent link>` = the comp, then a `<camera><image w/h/format/hfov/near/far>` (from
    /// the typed intrinsics + FoV frustum) or a `<ray><horizontal|vertical>` (from the scan pattern). These
    /// sensors have NO verbatim `urdf:sensor` quarantine (that path is suppressed upstream), so this is the
    /// SOLE emission; no duplication. A native `<sensor>` REQUIRES a `@name` (export correlation) and a
    /// `<parent link>` (the comp); when either is missing the sensor falls back to a `<gazebo>` block so it
    /// is never silently lost.
    fn write_native_sensors(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        comp: &Comp,
        emit: &[&Sensor],
    ) -> Result<()> {
        for s in emit {
            let Some((kind, opt)) = native_optical(s) else {
                continue;
            };
            let sname = s.name.as_deref().filter(|n| !n.is_empty());
            let sctx = match sname {
                Some(n) => format!("sensor {}", repr_str(n)),
                None => "sensor".to_string(),
            };
            if sname.is_none() || comp.name.is_empty() {
                self.loss.add(
                    "sensor",
                    format!("{sctx}: native URDF <sensor> needs a @name and a <parent link>; emitted as <gazebo><sensor> instead"),
                );
                self.write_gazebo_sensors(w, comp, &[*s])?;
                continue;
            }
            w_start(
                w,
                "sensor",
                &[("name", sname), ("update_rate", s.update_rate.as_deref())],
            )?;
            self.write_origin(w, opt.pose.as_ref())?;
            w_empty(w, "parent", &[("link", Some(comp.name.as_str()))])?;
            match kind {
                OpticalSensorType::Lidar => self.write_native_ray(w, opt, &sctx)?,
                _ => self.write_native_camera(w, opt, &sctx)?,
            }
            w_end(w, "sensor")?;
        }
        Ok(())
    }

    /// The typed camera's `<camera><image>` (urdf.xsd:288-307): dims/format from the intrinsics,
    /// hfov/near/far from the FoV frustum, the inverse of `from_urdf::native_camera_optical`. Native URDF
    /// stores ONLY these `<image>` attributes; anything richer the typed camera carries (the derived
    /// pinhole intrinsics fx/fy/cx/cy/s, `<distortion>`, `<lens>`, `<calibration>`, `<camera-matrix>`,
    /// `<camera-params>`, extra `<fov>`s, vertical/diagonal fov) has NO native home and is recorded.
    fn write_native_camera(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        opt: &OpticalSensor,
        sctx: &str,
    ) -> Result<()> {
        let fov = opt.fov.first();
        let intr = fov.and_then(|f| f.intrinsics.as_ref());
        let frustum = fov
            .and_then(|f| f.geometry.as_ref())
            .and_then(|g| g.frustum.as_ref());
        w_start(w, "camera", &[])?;
        let image = [
            ("width", intr.and_then(|i| i.width.as_deref())),
            ("height", intr.and_then(|i| i.height.as_deref())),
            ("format", intr.and_then(|i| i.format.as_deref())),
            ("hfov", frustum.and_then(|f| f.hfov.as_deref())),
            ("near", frustum.and_then(|f| f.near.as_deref())),
            ("far", frustum.and_then(|f| f.far.as_deref())),
        ];
        if image.iter().any(|(_, v)| v.is_some()) {
            w_empty(w, "image", &image)?;
        }
        w_end(w, "camera")?;
        self.note_native_camera_losses(opt, sctx);
        Ok(())
    }

    /// The typed camera fields with no native URDF `<image>` home -> recorded, never silently dropped.
    fn note_native_camera_losses(&mut self, opt: &OpticalSensor, sctx: &str) {
        let fov = opt.fov.first();
        if let Some(i) = fov.and_then(|f| f.intrinsics.as_ref()) {
            if i.fx.is_some() || i.fy.is_some() || i.cx.is_some() || i.cy.is_some() || i.s.is_some()
            {
                self.loss.add("sensor", format!("{sctx}: camera intrinsics fx/fy/cx/cy/s have no native URDF <image> field (URDF derives them from @hfov+@width); dropped"));
            }
        }
        if fov.is_some_and(|f| f.distortion.is_some()) {
            self.loss.add(
                "sensor",
                format!("{sctx}: camera <distortion> has no native URDF field; dropped"),
            );
        }
        if fov.is_some_and(|f| f.lens.is_some()) {
            self.loss.add(
                "sensor",
                format!("{sctx}: camera <lens> projection model has no native URDF field; dropped"),
            );
        }
        if fov.is_some_and(|f| f.calibration.is_some()) {
            self.loss.add(
                "sensor",
                format!("{sctx}: camera <calibration> has no native URDF field; dropped"),
            );
        }
        if fov.is_some_and(|f| f.camera_matrix.is_some()) {
            self.loss.add("sensor", format!("{sctx}: camera <camera-matrix> (rectification/projection/binning/roi) has no native URDF field; dropped"));
        }
        if fov.is_some_and(|f| f.noise.is_some()) {
            self.loss.add(
                "sensor",
                format!("{sctx}: camera <noise> has no native URDF field; dropped"),
            );
        }
        if fov.is_some_and(|f| {
            f.geometry
                .as_ref()
                .and_then(|g| g.frustum.as_ref())
                .is_some_and(|fr| fr.fov.is_some() || fr.vfov.is_some())
        }) {
            self.loss.add("sensor", format!("{sctx}: camera frustum diagonal/vertical fov has no native URDF <image> field (only @hfov); dropped"));
        }
        if opt.fov.len() > 1 {
            self.loss.add(
                "sensor",
                format!(
                    "{sctx}: {} extra <fov>(s) not emitted (native URDF <camera> is single-view)",
                    opt.fov.len() - 1
                ),
            );
        }
        if opt.camera_params.is_some() {
            self.loss.add("sensor", format!("{sctx}: <camera-params> (shutter/hdr/compression/thermal/…) has no native URDF field; dropped"));
        }
        if opt.data_output.is_some() {
            self.loss.add(
                "sensor",
                format!("{sctx}: <data-output> has no native URDF field; dropped"),
            );
        }
        if opt.driver.is_some() {
            self.loss.add(
                "sensor",
                format!("{sctx}: sensor <driver> has no native URDF field; dropped"),
            );
        }
    }

    /// The typed lidar's `<ray>` (urdf.xsd:308-325): the angular `<horizontal>`/`<vertical>` scan grid from
    /// the typed scan-pattern, the inverse of `from_urdf::native_ray_optical`. Native URDF `<ray>` carries
    /// ONLY that angular grid; the along-beam `<range>`, beam `<noise>`, and the design-spec lidar-params
    /// (scan-type/channels/points-per-second/scan-rate/returns/wavelength/fov) have no native home.
    fn write_native_ray(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        opt: &OpticalSensor,
        sctx: &str,
    ) -> Result<()> {
        w_start(w, "ray", &[])?;
        if let Some(sp) = opt
            .lidar_params
            .as_ref()
            .and_then(|lp| lp.scan_pattern.as_ref())
        {
            if let Some(h) = &sp.horizontal {
                write_native_scan_axis(w, "horizontal", h)?;
            }
            if let Some(v) = &sp.vertical {
                write_native_scan_axis(w, "vertical", v)?;
            }
        }
        w_end(w, "ray")?;
        if let Some(lp) = &opt.lidar_params {
            if lp.range.is_some() || lp.noise.is_some() {
                self.loss.add("sensor", format!("{sctx}: lidar <range>/beam <noise> have no native URDF <ray> field; dropped"));
            }
            if lp.scan_type.is_some()
                || lp.channels.is_some()
                || lp.points_per_second.is_some()
                || lp.scan_rate.is_some()
                || lp.returns.is_some()
                || lp.wavelength.is_some()
                || lp.horizontal_fov.is_some()
                || lp.vertical_fov.is_some()
            {
                self.loss.add("sensor", format!("{sctx}: <lidar-params> scan-type/channels/points-per-second/scan-rate/returns/wavelength/fov have no native URDF field; dropped"));
            }
        }
        if opt.data_output.is_some() {
            self.loss.add(
                "sensor",
                format!("{sctx}: <data-output> has no native URDF field; dropped"),
            );
        }
        if opt.driver.is_some() {
            self.loss.add(
                "sensor",
                format!("{sctx}: sensor <driver> has no native URDF field; dropped"),
            );
        }
        Ok(())
    }

    /// A comp's typed collision `<surface>` friction/contact -> the Gazebo-classic
    /// `<gazebo reference="link"><mu1>/<mu2>/<kp>/<kd>/<fdir1>/<slip1>/<slip2>` idiom, the EXACT inverse of
    /// the URDF/Gazebo import (`from_urdf::extract_gazebo` + `from_sdf::gazebo_friction_surface`): mu1 <-
    /// friction @static, mu2 <- friction-direction/@mu2, kp <- contact @stiffness, kd <- contact @damping,
    /// and the anisotropic fdir1/slip1/slip2 <- friction-direction. On import this friction lands on EVERY
    /// collision of the referenced link and is stripped from the `org.gazebosim.raw` quarantine, so URDF
    /// core (which has no `<surface>`) would otherwise lose it; re-emitting it here closes the round-trip.
    ///
    /// Gazebo-classic friction is link-wide, so it is emitted ONCE from the FIRST collision that carries a
    /// surface. Surface bits with no Gazebo-classic home are recorded as losses, never silently dropped:
    /// restitution/`<bounce>`, a genuine kinetic friction `@dynamic`, and any additional collision surface
    /// that differs from the first (a single link-wide block cannot carry per-collision friction).
    fn write_gazebo_friction(
        &mut self,
        w: &mut Writer<Cursor<Vec<u8>>>,
        comp: &Comp,
    ) -> Result<()> {
        let Some(first) = comp.collision.iter().find(|c| c.surface.is_some()) else {
            return Ok(());
        };
        let surf = first.surface.as_ref().unwrap();
        let ctx = format!("comp {}", repr_str(&comp.name));
        if comp.name.is_empty() {
            self.loss.add(
                "collision",
                format!("{ctx}: <surface> friction cannot attach to a <gazebo reference> on an unnamed comp; dropped"),
            );
            return Ok(());
        }
        let fric = surf.friction.as_ref();
        let fd = fric.and_then(|f| f.friction_direction.as_ref());
        let mu1 = fric.and_then(|f| f.static_.as_deref());
        let mu2 = fd.and_then(|d| d.mu2.as_deref());
        let (fdir1, slip1, slip2) = (
            fd.and_then(|d| d.fdir1.as_deref()),
            fd.and_then(|d| d.slip1.as_deref()),
            fd.and_then(|d| d.slip2.as_deref()),
        );
        let contact = surf.contact.as_ref();
        let (kp, kd) = (
            contact.and_then(|c| c.stiffness.as_deref()),
            contact.and_then(|c| c.damping.as_deref()),
        );
        if [mu1, mu2, kp, kd, fdir1, slip1, slip2]
            .iter()
            .any(Option::is_some)
        {
            w_start(w, "gazebo", &[("reference", Some(comp.name.as_str()))])?;
            w_leaf_opt(w, "mu1", mu1)?;
            w_leaf_opt(w, "mu2", mu2)?;
            w_leaf_opt(w, "kp", kp)?;
            w_leaf_opt(w, "kd", kd)?;
            w_leaf_opt(w, "fdir1", fdir1)?;
            w_leaf_opt(w, "slip1", slip1)?;
            w_leaf_opt(w, "slip2", slip2)?;
            w_end(w, "gazebo")?;
        }
        // Surface physics with no Gazebo-classic home -> loss, never silent.
        if surf.restitution.is_some() {
            self.loss.add(
                "collision",
                format!("{ctx}: <surface> restitution dropped (the Gazebo-classic friction idiom has no restitution coefficient)"),
            );
        }
        if fric.and_then(|f| f.dynamic.as_deref()).is_some() {
            self.loss.add(
                "collision",
                format!("{ctx}: <surface> friction @dynamic (kinetic) dropped (the Gazebo-classic friction idiom has no kinetic coefficient)"),
            );
        }
        let differing = comp
            .collision
            .iter()
            .filter_map(|c| c.surface.as_ref())
            .filter(|s| *s != surf)
            .count();
        if differing > 0 {
            self.loss.add(
                "collision",
                format!("{ctx}: {differing} additional collision <surface>(s) differ from the first; Gazebo-classic friction is link-wide and cannot carry per-collision surfaces; dropped"),
            );
        }
        Ok(())
    }

    fn write_joint(&mut self, w: &mut Writer<Cursor<Vec<u8>>>, j: &Joint) -> Result<()> {
        // Python formats `joint {j.name!r}` (single-quoted; `None` when the joint is unnamed).
        let jname = repr_opt(j.name.as_deref());
        if j.loop_.is_some() {
            self.loss.add(
                "loop",
                format!(
                    "joint {jname}: loop-closure dropped; URDF is tree-only and cannot express the closed kinematic loop (a core HCDF capability)"
                ),
            );
            return Ok(());
        }
        let jt = j.type_.unwrap_or(JointType::Fixed);
        let downgraded = jtype_out(jt).is_none();
        let urdf_type = jtype_out(jt).unwrap_or("fixed");
        if downgraded {
            self.loss.add(
                "joint-type",
                format!(
                    "joint {jname}: type {} has no URDF equivalent; exported as 'fixed' to keep the tree connected",
                    repr_str(&jt.to_string())
                ),
            );
            if let Some(ax) = &j.axis {
                self.loss.add(
                    "joint",
                    format!(
                        "joint {jname}: <axis xyz={}> dropped with the {jt}->fixed downgrade",
                        repr_opt(ax.xyz.as_deref())
                    ),
                );
            }
            if j.limit.is_some() {
                self.loss.add(
                    "joint",
                    format!("joint {jname}: <limit> dropped with the {jt}->fixed downgrade"),
                );
            }
        }
        if j.thread_pitch.is_some() {
            self.loss.add(
                "joint",
                format!(
                    "joint {jname}: thread_pitch {} dropped (URDF has no screw joint)",
                    repr_opt(j.thread_pitch.as_deref())
                ),
            );
        }
        w_start(
            w,
            "joint",
            &[("name", j.name.as_deref()), ("type", Some(urdf_type))],
        )?;
        if let Some(p) = &j.parent {
            w_empty(w, "parent", &[("link", p.comp.as_deref())])?;
        }
        if let Some(c) = &j.child {
            w_empty(w, "child", &[("link", c.comp.as_deref())])?;
        }
        self.write_origin(w, j.origin.as_ref())?;
        let no_dof = urdf_type == "fixed" || urdf_type == "floating";
        if let Some(ax) = &j.axis {
            if !no_dof {
                let xyz = ax.xyz.as_deref().map(|x| self.conv_axis(x));
                w_empty(w, "axis", &[("xyz", xyz.as_deref())])?;
            }
        }
        if j.axis2.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname}: <axis2> dropped (URDF has no second axis)"),
            );
        }
        // URDF bounds only ONE DOF per joint (a single <limit>). A <limit2> (the universal axis2
        // bound, the cylindrical rotation bound, or the planar SECOND in-plane range) has no URDF home.
        if j.limit2.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname}: <limit2> (second-DOF bound) dropped (URDF bounds only one DOF per joint)"),
            );
        }
        // Ball swing-cone + twist have no URDF home (URDF spherical carries no limits).
        if j.swing_limit.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname}: <swing_limit> (ball swing-cone) dropped (URDF spherical has no limits)"),
            );
        }
        if j.twist_limit.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname}: <twist_limit> (ball twist bound) dropped (URDF spherical has no limits)"),
            );
        }
        if let Some(lim) = &j.limit {
            if !no_dof {
                let effort = lim.effort.clone().unwrap_or_else(|| "0".to_string());
                let velocity = lim.velocity.clone().unwrap_or_else(|| "0".to_string());
                // acceleration/deceleration/jerk are first-class urdfdom v1.2 limit attributes
                // (urdf.xsd:215-217, mirroring urdfdom_headers JointLimits); emit them so they
                // round-trip; start_with skips the None ones.
                w_empty(
                    w,
                    "limit",
                    &[
                        ("lower", lim.lower.as_deref()),
                        ("upper", lim.upper.as_deref()),
                        ("effort", Some(&effort)),
                        ("velocity", Some(&velocity)),
                        ("acceleration", lim.acceleration.as_deref()),
                        ("deceleration", lim.deceleration.as_deref()),
                        ("jerk", lim.jerk.as_deref()),
                    ],
                )?;
            }
        }
        if let Some(dyn_) = &j.dynamics {
            w_empty(
                w,
                "dynamics",
                &[
                    ("damping", dyn_.damping.as_deref()),
                    ("friction", dyn_.friction.as_deref()),
                ],
            )?;
            for (sp, present) in [
                ("spring_stiffness", dyn_.spring_stiffness.is_some()),
                ("spring_reference", dyn_.spring_reference.is_some()),
            ] {
                if present {
                    self.loss.add(
                        "joint",
                        format!("joint {jname}: dynamics/{sp} dropped (no URDF field)"),
                    );
                }
            }
        }
        if let Some(mim) = &j.mimic {
            w_empty(
                w,
                "mimic",
                &[
                    ("joint", mim.joint.as_deref()),
                    ("multiplier", mim.multiplier.as_deref()),
                    ("offset", mim.offset.as_deref()),
                ],
            )?;
        }
        if let Some(cal) = &j.calibration {
            w_empty(
                w,
                "calibration",
                &[
                    ("reference_position", cal.reference_position.as_deref()),
                    ("rising", cal.rising.as_deref()),
                    ("falling", cal.falling.as_deref()),
                ],
            )?;
        }
        if let Some(uc) = &j.urdf_compat {
            if let Some(s) = &uc.safety_controller {
                w_empty(
                    w,
                    "safety_controller",
                    &[
                        ("soft_lower_limit", s.soft_lower_limit.as_deref()),
                        ("soft_upper_limit", s.soft_upper_limit.as_deref()),
                        ("k_position", s.k_position.as_deref()),
                        ("k_velocity", s.k_velocity.as_deref()),
                    ],
                )?;
            }
        }
        if j.extension.is_some() {
            self.loss.add(
                "joint",
                format!("joint {jname}: <extension> vendor data dropped (no URDF home)"),
            );
        }
        if j.description.is_some() {
            self.loss.add(
                "annotation",
                format!("joint {jname}: <description> dropped (no URDF field)"),
            );
        }
        w_end(w, "joint")?;
        Ok(())
    }

    fn build(&mut self, w: &mut Writer<Cursor<Vec<u8>>>) -> Result<()> {
        let name = if self.doc.name.is_empty() {
            "robot".to_string()
        } else {
            self.doc.name.clone()
        };
        w_start(w, "robot", &[("name", Some(&name))])?;
        // document-level metadata URDF <robot> has no field for (benign, non-model). Python formats
        // `document {label} {val!r}` -> single-quoted values.
        if let Some(v) = &self.doc.description {
            self.loss.add(
                "annotation",
                format!(
                    "document <description> {} dropped (no URDF field)",
                    repr_str(v)
                ),
            );
        }
        if let Some(v) = &self.doc.author {
            self.loss.add(
                "annotation",
                format!("document author {} dropped (no URDF field)", repr_str(v)),
            );
        }
        if let Some(v) = &self.doc.license {
            self.loss.add(
                "annotation",
                format!("document license {} dropped (no URDF field)", repr_str(v)),
            );
        }
        if let Some(v) = &self.doc.url {
            self.loss.add(
                "annotation",
                format!("document url {} dropped (no URDF field)", repr_str(v)),
            );
        }
        // The document schema version has no URDF home (a URDF robot/@version is a distinct concept).
        // Recorded as an annotation loss to match to_urdf.py's loop (which includes ("version","version"));
        // fires only when the doc actually carries a version (an absent `@version` stays empty), matching
        // Python's `version is not None` guard.
        if !self.doc.version.is_empty() {
            self.loss.add(
                "annotation",
                format!(
                    "document version {} dropped (no URDF field)",
                    repr_str(&self.doc.version)
                ),
            );
        }
        if self.doc.world_frame == Some(WorldFrame::NED) {
            self.loss.add(
                "world-frame",
                "document is world-frame NED; world-relative placements are NOT converted to URDF's ENU (body-frame poses ARE). Re-root world poses before consuming the URDF.",
            );
        }
        let colors = self.doc.color.clone();
        for col in &colors {
            self.write_material(w, col)?;
        }
        let comps = self.doc.comp.clone();
        for comp in &comps {
            self.write_link(w, comp)?;
        }
        let joints = self.doc.joint.clone();
        for j in &joints {
            self.write_joint(w, j)?;
        }
        // Typed sensors -> a NATIVE robot-level `<sensor>` (optical camera/lidar, prefer-native) or a
        // `<gazebo reference="link"><sensor>` block (every other category), grouped after the tree.
        // Sensors that a native `urdf:sensor` quarantine already round-trips verbatim are suppressed here
        // so every sensor emits exactly once.
        let native_quarantined = native_quarantined_sensor_names(self.doc);
        for comp in &comps {
            self.write_comp_sensors(w, comp, &native_quarantined)?;
            // Round-trip the link-wide Gazebo-classic friction/contact idiom from the typed collision
            // <surface> (inverse of the URDF/Gazebo import). Emitted as its own <gazebo reference> block.
            self.write_gazebo_friction(w, comp)?;
        }
        // Typed core transmissions -> URDF <transmission> (inverse of the import map).
        let transmissions = self.doc.transmission.clone();
        for tr in &transmissions {
            self.write_transmission(w, tr)?;
        }
        // de-merge quarantined extensions back to their native top-level elements. The ros2_control
        // block was re-homed under the typed extension root <ros2-control> on import (schema
        // hcdf-ext-ros2-control.xsd); rename it back to a URDF <ros2_control> so the round-trip is a
        // faithful URDF element. Every other domain re-emits its verbatim body unchanged.
        for ext in &self.doc.extension {
            if ext.domain == ROS2_CONTROL_DOMAIN {
                let urdf_body = rename_root_element(
                    &ext.body,
                    ROS2_CONTROL_TYPED_ROOT,
                    ROS2_CONTROL_URDF_ROOT,
                )?;
                w_raw(w, &urdf_body)?;
            } else {
                w_raw(w, &ext.body)?;
            }
        }
        // HCDF-only top-level constructs -> recorded, never silently dropped.
        self.drop_list(
            "top-level",
            "document",
            "joint groups",
            self.doc.group.len(),
        );
        self.drop_list(
            "top-level",
            "document",
            "kinematic states",
            self.doc.state.len(),
        );
        self.drop_list(
            "top-level",
            "document",
            "networks",
            self.doc.link.len()
                + self.doc.bus.len()
                + self.doc.chain.len()
                + self.doc.star.len()
                + self.doc.ring.len()
                + self.doc.mesh.len()
                + self.doc.tree.len(),
        );
        self.drop_list("top-level", "document", "includes", self.doc.include.len());
        self.drop_list(
            "top-level",
            "document",
            "self-collision-disable blocks",
            usize::from(self.doc.self_collision_disable.is_some()),
        );
        w_end(w, "robot")?;
        Ok(())
    }
}

/// Export an HCDF [`Hcdf`] model to a URDF XML string. Returns `(urdf_xml, LossManifest)`.
pub fn to_urdf(doc: &Hcdf) -> Result<(String, LossManifest)> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    writer
        .write_event(Event::Text(BytesText::from_escaped("")))
        .map_err(|e| Error::Xml(e.to_string()))?;
    let mut ex = Exporter::new(doc);
    ex.build(&mut writer)?;
    let body = String::from_utf8(writer.into_inner().into_inner())
        .map_err(|e| Error::Xml(e.to_string()))?;
    let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{body}");
    Ok((xml, ex.loss))
}

// ── ros2_control typed-extension re-homing (shared with `from_urdf`) ─────────────────────────────
//
// A URDF `<ros2_control>` block is quarantined under `<extension domain="org.ros2.control">`. Instead of
// an opaque verbatim blob, the importer re-homes it under the typed extension root `<ros2-control>`
// (schema `extensions/hcdf-ext-ros2-control.xsd`) so the body is schema-typed/validatable; the exporter
// renames it back to `<ros2_control>`. The constants + the rename helper live here (the module compiled
// whenever EITHER converter is enabled) so `from_urdf` can reuse them without `to_urdf` depending on
// `from_urdf` (which an SDF-only / wasm build omits).

/// The extension domain the ros2_control hardware-interface surface is quarantined under.
pub(crate) const ROS2_CONTROL_DOMAIN: &str = "org.ros2.control";
/// The URDF element name of a ros2_control block.
pub(crate) const ROS2_CONTROL_URDF_ROOT: &str = "ros2_control";
/// The typed extension root the ros2_control block is re-homed under (the `hcdf-ext-ros2-control.xsd`
/// document element), so the quarantined body is schema-typed/validatable rather than an opaque blob.
pub(crate) const ROS2_CONTROL_TYPED_ROOT: &str = "ros2-control";

// ── Gazebo extension domains (shared by `from_urdf` and the SDF converters) ───────────────────────
//
// The Gazebo / gz-sim extension surface uses TWO domains so typed content and opaque passthrough do
// not collide. The frozen `extensions/hcdf-ext-gazebo.xsd` declares a single
// global element (the `<gazebo-sim>` root), so an extension body validates against it ONLY when it is
// a `<gazebo-sim>` document; a raw `<gazebo reference=...>` block has "no matching global declaration"
// and cannot validate. The two domains keep those two shapes apart:
//
//   * `org.gazebosim`:       the TYPED domain. Body = one `<gazebo-sim>` root carrying typed
//     `<physics>` (≤1) and `<plugin>` (≥0). `from_sdf` emits this for a model/world's `<plugin>`s and
//     `<physics>`. Content here VALIDATES against hcdf-ext-gazebo.xsd.
//   * `org.gazebosim.raw`:   the OPAQUE passthrough domain (no schema), for Gazebo content that has no
//     typed home: `from_urdf`'s verbatim `<gazebo reference=...>` residual and `from_sdf`'s sensor
//     sim-only fragments (camera `<clip>`, IMU dynamic-bias). Byte-faithful, like `urdf:<tag>` /
//     `org.urdf.material`. Both exporters re-emit its body verbatim, so a URDF round-trip is preserved.
//
// Splitting the domains IS the fix: `from_urdf` previously wrote raw `<gazebo>` bytes under
// `org.gazebosim`, which the `<gazebo-sim>`-rooted schema rejects.

/// The TYPED Gazebo extension domain; its body is a `<gazebo-sim>` root (hcdf-ext-gazebo.xsd).
pub(crate) const GAZEBO_DOMAIN: &str = "org.gazebosim";
/// The OPAQUE Gazebo passthrough domain (no schema): verbatim `<gazebo>` residual / sim-only fragments.
pub(crate) const GAZEBO_RAW_DOMAIN: &str = "org.gazebosim.raw";
/// The typed Gazebo extension root element (the hcdf-ext-gazebo.xsd document element).
pub(crate) const GAZEBO_SIM_ROOT: &str = "gazebo-sim";

/// The ROS 2 extension domain (hcdf-ext-ros2.xsd): sensor/motor -> ROS 2 topic mappings.
pub(crate) const ROS2_DOMAIN: &str = "org.ros2";
/// The ROS 2 extension root element (the hcdf-ext-ros2.xsd document element).
pub(crate) const ROS2_TOPIC_MAP_ROOT: &str = "topic-map";

/// The extension domain a NATIVE robot-level URDF `<sensor>` is quarantined under (`from_urdf`'s
/// `domain_for("sensor")` = `urdf:sensor`). The native sensor is BOTH decomposed into a typed comp
/// sensor AND kept quarantined verbatim here for a byte-identical URDF round-trip; the exporter uses
/// this domain to suppress the duplicate `<gazebo><sensor>` re-emit of any sensor it carries.
pub(crate) const URDF_SENSOR_DOMAIN: &str = "urdf:sensor";

/// Prefer-native: the sensor's WINNING category (the one [`crate::to_sensor::write_sensor`]
/// would emit; inertial takes priority over optical) is an optical CAMERA or LIDAR, the only two sensor
/// kinds with a native URDF grammar (`<camera>` / `<ray>`, urdf.xsd:327-341). Returns the kind + optical
/// payload so the exporter can emit a native `<sensor>`; `None` (imu/gnss/mag/thermal/tof/force/fluid/…)
/// stays a `<gazebo><sensor>`. Mirrors the dispatch in [`crate::to_sensor::write_sensor`]: an inertial
/// category present means the sensor emits as `imu` (gazebo), so it is NOT native even if optical is set.
fn native_optical(sensor: &Sensor) -> Option<(OpticalSensorType, &OpticalSensor)> {
    if !sensor.inertial.is_empty() {
        return None;
    }
    let opt = sensor.optical.first()?;
    match opt.type_ {
        Some(k @ (OpticalSensorType::Camera | OpticalSensorType::Lidar)) => Some((k, opt)),
        _ => None,
    }
}

/// One `<horizontal>`/`<vertical>` native URDF `LaserRay` (urdf.xsd:308-325): samples/resolution/min_angle/
/// max_angle ATTRIBUTES (underscored, unlike the hyphenated HCDF `@min-angle`), the inverse of
/// `from_urdf::native_scan_axis`. Presence-preserving: an absent field is omitted.
fn write_native_scan_axis(
    w: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    ax: &crate::model::LidarScanAxis,
) -> Result<()> {
    w_empty(
        w,
        name,
        &[
            ("samples", ax.samples.as_deref()),
            ("resolution", ax.resolution.as_deref()),
            ("min_angle", ax.min_angle.as_deref()),
            ("max_angle", ax.max_angle.as_deref()),
        ],
    )
}

/// Collect the `@name`s of every native `<sensor>` carried verbatim in an `<extension
/// domain="urdf:sensor">` body. These sensors round-trip to URDF through the verbatim quarantine, so the
/// exporter must NOT also synthesize a `<gazebo><sensor>` for their typed decomposition (that would emit
/// the sensor twice). Best-effort: a name is collected from each depth-0 `<sensor>` in the fragment.
fn native_quarantined_sensor_names(doc: &Hcdf) -> std::collections::BTreeSet<String> {
    use quick_xml::reader::Reader;
    let mut names = std::collections::BTreeSet::new();
    // Pull the `name` attribute of one `<sensor …>` start/empty tag into the set.
    let collect = |e: &BytesStart, names: &mut std::collections::BTreeSet<String>| {
        if e.name().as_ref() != b"sensor" {
            return;
        }
        for a in e.attributes().flatten() {
            if a.key.as_ref() == b"name" {
                if let Ok(v) = a.unescape_value() {
                    names.insert(v.into_owned());
                }
            }
        }
    };
    for ext in &doc.extension {
        if ext.domain != URDF_SENSOR_DOMAIN {
            continue;
        }
        let mut reader = Reader::from_str(&ext.body);
        reader.config_mut().expand_empty_elements = false;
        // Depth of the current element within the fragment; only depth-0 `<sensor>`s (the quarantined
        // native sensors themselves, never a nested tag) contribute a name.
        let mut depth = 0usize;
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    if depth == 0 {
                        collect(&e, &mut names);
                    }
                    depth += 1;
                }
                Ok(Event::Empty(e)) => {
                    if depth == 0 {
                        collect(&e, &mut names);
                    }
                }
                Ok(Event::End(_)) => depth = depth.saturating_sub(1),
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
    }
    names
}

/// Rewrite ONLY the outermost element's name in one well-formed XML fragment (`from` -> `to`),
/// preserving its attributes and its entire inner subtree verbatim. Re-homes a URDF `<ros2_control>`
/// block under the typed extension root `<ros2-control>` on import, and applies the inverse on export:
/// the outer tag is renamed while every child (`<hardware>`/`<joint>`/`<command_interface>`/
/// `<state_interface>`/`<param>`/...) is left untouched.
pub(crate) fn rename_root_element(fragment: &str, from: &str, to: &str) -> Result<String> {
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(fragment);
    reader.config_mut().expand_empty_elements = false;
    let mut w = Writer::new(Cursor::new(Vec::new()));
    // Depth of the current element under the fragment root (the root itself is at depth 0). Only the
    // depth-0 element carries the name being rewritten; a hypothetical nested `<from>` is left alone.
    let mut depth = 0usize;
    let rename_start = |e: &BytesStart| -> Result<BytesStart<'static>> {
        let mut renamed = BytesStart::new(to.to_string());
        for a in e.attributes() {
            renamed.push_attribute(a.map_err(|e| Error::Xml(e.to_string()))?);
        }
        Ok(renamed)
    };
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) => {
                let ev = if depth == 0 && e.name().as_ref() == from.as_bytes() {
                    Event::Start(rename_start(&e)?)
                } else {
                    Event::Start(e.into_owned())
                };
                w.write_event(ev).map_err(|e| Error::Xml(e.to_string()))?;
                depth += 1;
            }
            Event::Empty(e) => {
                let ev = if depth == 0 && e.name().as_ref() == from.as_bytes() {
                    Event::Empty(rename_start(&e)?)
                } else {
                    Event::Empty(e.into_owned())
                };
                w.write_event(ev).map_err(|e| Error::Xml(e.to_string()))?;
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                let ev = if depth == 0 && e.name().as_ref() == from.as_bytes() {
                    Event::End(BytesEnd::new(to.to_string()))
                } else {
                    Event::End(e.into_owned())
                };
                w.write_event(ev).map_err(|e| Error::Xml(e.to_string()))?;
            }
            Event::Eof => break,
            ev => w
                .write_event(ev.into_owned())
                .map_err(|e| Error::Xml(e.to_string()))?,
        }
    }
    String::from_utf8(w.into_inner().into_inner()).map_err(|e| Error::Xml(e.to_string()))
}
