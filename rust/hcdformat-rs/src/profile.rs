//! HCDF-URDF Profile (1.0) checker (feature = `urdf`), a faithful port of `urdf_hcdf/profile.py`.
//!
//! Classifies an HCDF document by how cleanly it exports to URDF:
//!
//! - [`Tier::Identity`]: exports to clean valid URDF with a value-exact round-trip; no frame
//!   transform, no asset bake, nothing dropped.
//! - [`Tier::WithTransform`]: same fidelity, but a frame conversion (FRD/NED), a GLB-appearance bake,
//!   or an `<include>` flatten is needed first.
//! - [`Tier::OutOfProfile`]: exporting to URDF drops model-significant content (closed loops, HCDF-only
//!   joint types/primitives, contact `<surface>`, the cyber layer, `group`/`state`, …). The URDF is a
//!   lossy projection.
//!
//! The classification is the worst tier across all findings, each mapped directly from the profile
//! contract by inspecting the typed DOM (the `P_*` finding codes). The report also carries the
//! authoritative field-level [`LossManifest`] (from [`mod@crate::to_urdf`]) and the validator issues.
//!
//! Deliberate deviations from the strict profile definition, grounded in what `to_urdf` actually does:
//! root `<extension>` (gazebo / ros2_control / transmission / vendor passthrough) is NOT out-of-profile
//! (it is de-merged back); authored visual/collision names are NOT flagged (they round-trip).
use crate::model::enums::{BodyFrame, JointType, WorldFrame};
use crate::model::{CollisionGeometry, Hcdf, Pose, VisualAppearance, VisualGeometry};
use crate::pyrepr::{repr_enum, repr_opt, repr_str};
use crate::to_urdf::{to_urdf, LossManifest};
use crate::validate::{validate_semantic, Issue, Level};

/// The three profile tiers (worst wins). `Identity` < `WithTransform` < `OutOfProfile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Exports to clean valid URDF with a value-exact round-trip; nothing dropped.
    Identity,
    /// Exports losslessly after a frame conversion, GLB-appearance bake, or include flatten.
    WithTransform,
    /// Exporting to URDF drops model-significant content; the URDF is a lossy projection.
    OutOfProfile,
}

impl Tier {
    /// The exact classification string used by the Python checker (`IN-PROFILE-IDENTITY`, etc.).
    pub fn label(self) -> &'static str {
        match self {
            Tier::Identity => "IN-PROFILE-IDENTITY",
            Tier::WithTransform => "IN-PROFILE-WITH-FRAME-TRANSFORM",
            Tier::OutOfProfile => "OUT-OF-PROFILE",
        }
    }
    /// One-line meaning of the tier (matches Python `_TIER_MEANING`).
    pub fn meaning(self) -> &'static str {
        match self {
            Tier::Identity => "Exports to clean valid URDF with a value-exact round-trip; nothing dropped.",
            Tier::WithTransform => "Exports losslessly after a frame conversion, GLB-appearance bake, or include flatten.",
            Tier::OutOfProfile => "Exporting to URDF drops model-significant content; the URDF is a lossy projection.",
        }
    }
}

/// One reason a document is at a given tier (the driver of the classification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub tier: Tier,
    pub code: String,
    pub detail: String,
}

impl Finding {
    fn new(tier: Tier, code: &str, detail: impl Into<String>) -> Self {
        Finding {
            tier,
            code: code.to_string(),
            detail: detail.into(),
        }
    }
}

/// The profile report: classification, the driving findings, the field-level loss manifest, and the
/// validator issues: "which tier and why" + "exactly what content is lost".
#[derive(Debug, Clone)]
pub struct ProfileReport {
    pub classification: Tier,
    pub findings: Vec<Finding>,
    pub loss: LossManifest,
    pub issues: Vec<Issue>,
}

impl ProfileReport {
    /// True if URDF-consumable (identity or with-transform); false if out-of-profile.
    pub fn in_profile(&self) -> bool {
        self.classification != Tier::OutOfProfile
    }
    /// Findings at a given tier.
    pub fn by_tier(&self, tier: Tier) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.tier == tier)
    }

    /// The report as the JSON value Python `ProfileReport.to_dict()` builds: classification, meaning,
    /// the `in_profile` flag, the findings, the ERROR-level validator issues as `[level] code: message`
    /// strings, and the embedded loss manifest. Backs [`Self::to_json`].
    fn to_json_value(&self) -> crate::report::Json {
        use crate::report::Json;
        let findings = self
            .findings
            .iter()
            .map(|f| {
                Json::Obj(vec![
                    ("tier".to_string(), Json::Str(f.tier.label().to_string())),
                    ("code".to_string(), Json::Str(f.code.clone())),
                    ("detail".to_string(), Json::Str(f.detail.clone())),
                ])
            })
            .collect();
        let validator_errors = self
            .issues
            .iter()
            .filter(|i| i.level == Level::Error)
            .map(|i| Json::Str(i.to_string()))
            .collect();
        Json::Obj(vec![
            (
                "classification".to_string(),
                Json::Str(self.classification.label().to_string()),
            ),
            (
                "meaning".to_string(),
                Json::Str(self.classification.meaning().to_string()),
            ),
            ("in_profile".to_string(), Json::Bool(self.in_profile())),
            ("findings".to_string(), Json::Arr(findings)),
            ("validator_errors".to_string(), Json::Arr(validator_errors)),
            ("loss_manifest".to_string(), self.loss.to_json_value()),
        ])
    }

    /// Structured JSON report, byte-identical to Python `ProfileReport.to_json()`
    /// (`json.dumps(..., indent=2)`, no trailing newline).
    pub fn to_json(&self) -> String {
        crate::report::dump(&self.to_json_value())
    }

    /// Human-readable markdown report, byte-identical to Python `ProfileReport.markdown()`: the
    /// classification heading and meaning, the out-of-profile and transform sections, the ERROR-level
    /// validator issues, and the field-level loss manifest (its category sections in sorted order).
    /// Ends with a single trailing newline.
    pub fn markdown(&self) -> String {
        let mut lines = vec![
            format!("# HCDF-URDF profile: {}", self.classification.label()),
            String::new(),
            self.classification.meaning().to_string(),
            String::new(),
        ];
        let out: Vec<_> = self.by_tier(Tier::OutOfProfile).collect();
        let xf: Vec<_> = self.by_tier(Tier::WithTransform).collect();
        if !out.is_empty() {
            lines.push(format!("## Out-of-profile drivers ({})", out.len()));
            lines.extend(out.iter().map(|f| format!("- `{}` {}", f.code, f.detail)));
            lines.push(String::new());
        }
        if !xf.is_empty() {
            lines.push(format!("## Transform / bake needed ({})", xf.len()));
            lines.extend(xf.iter().map(|f| format!("- `{}` {}", f.code, f.detail)));
            lines.push(String::new());
        }
        let errs: Vec<_> = self
            .issues
            .iter()
            .filter(|i| i.level == Level::Error)
            .collect();
        if !errs.is_empty() {
            lines.push(format!("## Validator errors ({})", errs.len()));
            lines.extend(errs.iter().map(|i| format!("- {i}")));
            lines.push(String::new());
        }
        lines.push("## Field-level loss manifest".to_string());
        lines.push(String::new());
        if self.loss.is_empty() {
            lines.push("_No field-level losses._".to_string());
        } else {
            // Python embeds the loss manifest's markdown body under the heading "Export drops" with its
            // own title line stripped: `self.loss.markdown(title="Export drops").split("\n", 2)[-1]`.
            lines.push(split_after_two_newlines(
                &self.loss.markdown("Export drops"),
            ));
        }
        lines.join("\n").trim_end().to_string() + "\n"
    }
}

/// Reproduce Python `s.split("\n", 2)[-1]`: the substring after the second newline, or after the only
/// newline / the whole string when there are fewer than two (the last piece of the capped split).
fn split_after_two_newlines(s: &str) -> String {
    let mut nls = s.match_indices('\n');
    match (nls.next(), nls.next()) {
        (_, Some((idx, _))) | (Some((idx, _)), None) => s[idx + 1..].to_string(),
        (None, _) => s.to_string(),
    }
}

// URDF-expressible joint types (`free` maps to URDF `floating`). Anything else is out-of-profile.
fn joint_type_in_profile(jt: JointType) -> bool {
    matches!(
        jt,
        JointType::Revolute
            | JointType::Continuous
            | JointType::Prismatic
            | JointType::Fixed
            | JointType::Free
            | JointType::Planar
    )
}

fn visual_bad_prim(geo: &VisualGeometry) -> Option<&'static str> {
    if geo.capsule.is_some() {
        Some("capsule")
    } else if geo.cone.is_some() {
        Some("cone")
    } else if geo.ellipsoid.is_some() {
        Some("ellipsoid")
    } else {
        None
    }
}

fn collision_bad_prim(geo: &CollisionGeometry) -> Option<&'static str> {
    if geo.capsule.is_some() {
        Some("capsule")
    } else if geo.cone.is_some() {
        Some("cone")
    } else if geo.ellipsoid.is_some() {
        Some("ellipsoid")
    } else {
        None
    }
}

fn visual_has_urdf_prim(geo: &VisualGeometry) -> bool {
    geo.box_.is_some() || geo.cylinder.is_some() || geo.sphere.is_some()
}

fn collision_has_urdf_prim(geo: &CollisionGeometry) -> bool {
    geo.box_.is_some() || geo.cylinder.is_some() || geo.sphere.is_some() || geo.mesh.is_some()
}

fn pose_quat(pose: Option<&Pose>) -> bool {
    pose.map(|p| p.quat.is_some()).unwrap_or(false)
}

/// Classify `doc` against the HCDF-URDF Profile 1.0.
pub fn check_profile(doc: &Hcdf) -> ProfileReport {
    let mut findings: Vec<Finding> = Vec::new();
    let issues = validate_semantic(doc);

    // ── structure: a single connected acyclic tree with exactly one root ──────────────────────────
    for i in &issues {
        if i.level == Level::Error
            && matches!(
                i.code.as_str(),
                "E_MULTI_PARENT" | "E_CYCLE" | "E_SELF_JOINT" | "E_LOOP_REF"
            )
        {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_NOT_A_TREE",
                format!("{}: {}", i.code, i.message),
            ));
        }
    }
    let mut children: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for j in &doc.joint {
        if j.loop_.is_some() {
            continue;
        }
        if let Some(c) = j.child.as_ref().and_then(|e| e.comp.as_deref()) {
            children.insert(c);
        }
    }
    let roots: Vec<&str> = doc
        .comp
        .iter()
        .map(|c| c.name.as_str())
        .filter(|n| !children.contains(n))
        .collect();
    if !doc.comp.is_empty() && roots.is_empty() {
        findings.push(Finding::new(
            Tier::OutOfProfile,
            "P_NO_ROOT",
            "no root comp (every comp is a joint child; a tree needs one root)",
        ));
    } else if roots.len() > 1 {
        let mut sorted: Vec<&str> = roots.clone();
        sorted.sort();
        let names = sorted
            .iter()
            .map(|r| repr_str(r))
            .collect::<Vec<_>>()
            .join(", ");
        findings.push(Finding::new(
            Tier::OutOfProfile,
            "P_MULTI_ROOT",
            format!(
                "{} roots ({names}); URDF is one robot with one root link (a network-only comp counts as an extra root)",
                roots.len()
            ),
        ));
    }

    // ── frame: URDF is always FLU/ENU ─────────────────────────────────────────────────────────────
    if doc.body_frame == Some(BodyFrame::FRD) {
        findings.push(Finding::new(
            Tier::WithTransform,
            "P_BODY_FRD",
            "body-frame FRD: body poses and joint axes are converted to URDF FLU on export (lossless)",
        ));
    }
    if doc.world_frame == Some(WorldFrame::NED) {
        findings.push(Finding::new(
            Tier::WithTransform,
            "P_WORLD_NED",
            "world-frame NED: URDF is ENU; world-relative placements need conversion (note: to_urdf does not yet apply the world transform; body poses ARE converted)",
        ));
    }

    // ── joints ─────────────────────────────────────────────────────────────────────────────────────
    let mut quat_poses = 0usize;
    for j in &doc.joint {
        // Python formats `joint {j.name!r}` (single-quoted, `None` when unnamed); repr the Option.
        let nm = repr_opt(j.name.as_deref());
        if pose_quat(j.origin.as_ref()) {
            quat_poses += 1;
        }
        if j.loop_.is_some() {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_LOOP",
                format!("joint {nm}: loop-closure (URDF is tree-only, cannot express a closed kinematic loop)"),
            ));
        }
        if let Some(jt) = j.type_ {
            if !joint_type_in_profile(jt) {
                findings.push(Finding::new(
                    Tier::OutOfProfile,
                    "P_JOINT_TYPE",
                    // Python: `type {jt!r}` where jt is the enum VALUE string -> single-quoted.
                    format!(
                        "joint {nm}: type {} has no URDF equivalent",
                        repr_str(&jt.to_string())
                    ),
                ));
            }
        }
        if j.axis2.is_some() {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_AXIS2",
                format!("joint {nm}: <axis2> (URDF has a single axis)"),
            ));
        }
        if j.thread_pitch.is_some() {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_THREAD_PITCH",
                format!("joint {nm}: thread_pitch (URDF has no screw joint)"),
            ));
        }
        if let Some(lim) = &j.limit {
            for (fld, present) in [
                ("acceleration", lim.acceleration.is_some()),
                ("jerk", lim.jerk.is_some()),
                ("deceleration", lim.deceleration.is_some()),
            ] {
                if present {
                    findings.push(Finding::new(
                        Tier::OutOfProfile,
                        "P_LIMIT_FIELD",
                        format!("joint {nm}: limit/{fld} (URDF limit has only lower/upper/effort/velocity)"),
                    ));
                }
            }
        }
        if let Some(d) = &j.dynamics {
            for (sp, present) in [
                ("spring_stiffness", d.spring_stiffness.is_some()),
                ("spring_reference", d.spring_reference.is_some()),
            ] {
                if present {
                    findings.push(Finding::new(
                        Tier::OutOfProfile,
                        "P_DYNAMICS_SPRING",
                        format!(
                            "joint {nm}: dynamics/{sp} (URDF dynamics has only damping/friction)"
                        ),
                    ));
                }
            }
        }
        if j.extension.is_some() {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_JOINT_EXTENSION",
                format!("joint {nm}: <extension> vendor data (no URDF home; joint extensions are not de-merged)"),
            ));
        }
    }

    // ── per-comp: geometry, contact surface, cyber layer, metadata ────────────────────────────────
    for comp in &doc.comp {
        let ctx = format!("comp {}", repr_str(&comp.name));
        if let Some(ip) = &comp.inertial {
            if pose_quat(ip.inertia_origin.as_ref()) {
                quat_poses += 1;
            }
            if let Some(inertia) = &ip.inertia {
                if inertia.split_whitespace().count() != 6 {
                    findings.push(Finding::new(
                        Tier::OutOfProfile,
                        "P_INERTIA_ARITY",
                        format!("{ctx}: inertia tuple {} is not 6 values (the rotational inertia matrix is dropped on URDF export)", repr_str(inertia)),
                    ));
                }
            }
        }
        for v in &comp.visual {
            let vname = repr_str(&v.name);
            if pose_quat(v.pose.as_ref()) {
                quat_poses += 1;
            }
            match &v.appearance {
                VisualAppearance::Model { .. } => {
                    findings.push(Finding::new(
                        Tier::WithTransform,
                        "P_GLB_MODEL",
                        format!("{ctx} visual {vname}: GLB <model>: geometry exports as a URDF mesh ref, but baked PBR/textures are not represented in URDF"),
                    ));
                    // The typed model forbids a color alongside a model, so P_GLB_COLOR cannot arise here.
                }
                VisualAppearance::Primitive { geometry, .. } => {
                    let geo = geometry.as_ref();
                    let bad = geo.and_then(visual_bad_prim);
                    if let Some(b) = bad {
                        findings.push(Finding::new(
                            Tier::OutOfProfile,
                            "P_GEOMETRY",
                            format!("{ctx} visual {vname}: <{b}> has no URDF primitive"),
                        ));
                    } else if !geo.map(visual_has_urdf_prim).unwrap_or(false) {
                        findings.push(Finding::new(
                            Tier::OutOfProfile,
                            "P_VISUAL_NO_GEOMETRY",
                            format!("{ctx} visual {vname}: neither a GLB <model> nor a URDF primitive, dropped on export"),
                        ));
                    }
                }
            }
            if v.toggle.is_some() {
                findings.push(Finding::new(
                    Tier::OutOfProfile,
                    "P_VISUAL_TOGGLE",
                    format!("{ctx} visual {vname}: toggle group {} (HCDF runtime show/hide grouping; no URDF home)", repr_opt(v.toggle.as_deref())),
                ));
            }
        }
        for c in &comp.collision {
            let cname = repr_opt(c.name.as_deref());
            if pose_quat(c.pose.as_ref()) {
                quat_poses += 1;
            }
            let bad = c.geometry.as_ref().and_then(collision_bad_prim);
            if let Some(b) = bad {
                findings.push(Finding::new(
                    Tier::OutOfProfile,
                    "P_GEOMETRY",
                    format!("{ctx} collision {cname}: <{b}> has no URDF primitive"),
                ));
            } else if !c
                .geometry
                .as_ref()
                .map(collision_has_urdf_prim)
                .unwrap_or(false)
            {
                findings.push(Finding::new(
                    Tier::OutOfProfile,
                    "P_COLLISION_NO_GEOMETRY",
                    format!("{ctx} collision {cname}: no URDF-representable geometry, dropped on export"),
                ));
            }
            if c.surface.is_some() {
                findings.push(Finding::new(
                    Tier::OutOfProfile,
                    "P_SURFACE",
                    format!("{ctx} collision {cname}: <surface> contact physics (no URDF core home; lives in <gazebo> at sim time)"),
                ));
            }
        }
        // Per-comp HCDF-only content with no URDF home (`P_COMP_<FIELD>`).
        comp_dropped(&ctx, "sensor", comp.sensor.len(), &mut findings);
        comp_dropped(&ctx, "motor", comp.motor.len(), &mut findings);
        comp_dropped(&ctx, "hmi", comp.hmi.len(), &mut findings);
        comp_dropped(
            &ctx,
            "dynamic_surface",
            comp.dynamic_surface.len(),
            &mut findings,
        );
        comp_dropped(&ctx, "power_source", comp.power_source.len(), &mut findings);
        comp_dropped(&ctx, "port", comp.port.len(), &mut findings);
        comp_dropped(&ctx, "antenna", comp.antenna.len(), &mut findings);
        comp_dropped(&ctx, "frame", comp.frame.len(), &mut findings);
        comp_dropped(&ctx, "extension", comp.extension.len(), &mut findings);
        // HCDF-only comp metadata scalars. Python formats `{label}={val!r}`: string values render
        // single-quoted, and the `role` enum renders `<CompRole.member: 'value'>` (enum repr).
        if let Some(v) = &comp.struct_type {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_COMP_STRUCT_TYPE",
                format!(
                    "{ctx}: struct-type={} (HCDF-only metadata; no URDF home)",
                    repr_str(v)
                ),
            ));
        }
        if let Some(v) = &comp.ip_rating {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_COMP_IP_RATING",
                format!(
                    "{ctx}: ip-rating={} (HCDF-only metadata; no URDF home)",
                    repr_str(v)
                ),
            ));
        }
        if let Some(v) = &comp.role {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_COMP_ROLE",
                format!(
                    "{ctx}: role={} (HCDF-only metadata; no URDF home)",
                    repr_enum("CompRole", &v.to_string())
                ),
            ));
        }
        if let Some(v) = &comp.hwid {
            findings.push(Finding::new(
                Tier::OutOfProfile,
                "P_COMP_HWID",
                format!(
                    "{ctx}: hwid={} (HCDF-only metadata; no URDF home)",
                    repr_str(v)
                ),
            ));
        }
    }

    // ── top-level HCDF-only content ───────────────────────────────────────────────────────────────
    top_dropped("group", doc.group.len(), &mut findings);
    top_dropped("state", doc.state.len(), &mut findings);
    top_dropped(
        "network",
        doc.link.len()
            + doc.bus.len()
            + doc.chain.len()
            + doc.star.len()
            + doc.ring.len()
            + doc.mesh.len()
            + doc.tree.len(),
        &mut findings,
    );
    // NB: <transmission> is NOT listed here; it now exports to a URDF <transmission>. Any
    // field-level loss (efficiency/backlash/spring, NATIVE-ONLY) is recorded in the `loss`
    // manifest produced by the to_urdf pass below, not as a top-level "no URDF home" drop.
    top_dropped(
        "self_collision_disable",
        usize::from(doc.self_collision_disable.is_some()),
        &mut findings,
    );
    if !doc.include.is_empty() {
        findings.push(Finding::new(
            Tier::WithTransform,
            "P_INCLUDE",
            format!(
                "document: {} <include>(s); flatten with hcdf.io.flatten() before URDF export",
                doc.include.len()
            ),
        ));
    }
    if quat_poses > 0 {
        findings.push(Finding::new(
            Tier::WithTransform,
            "P_POSE_QUAT",
            format!("{quat_poses} pose(s) use quat orientation; emitted as URDF rpy (rotation preserved exactly; URDF <origin> has no quaternion)"),
        ));
    }

    // worst tier wins
    let cls = findings
        .iter()
        .map(|f| f.tier)
        .max()
        .unwrap_or(Tier::Identity);

    let loss = match to_urdf(doc) {
        Ok((_, loss)) => loss,
        Err(e) => {
            let mut l = LossManifest::default();
            l.add("export-error", format!("to_urdf failed: {e}"));
            l
        }
    };

    ProfileReport {
        classification: cls,
        findings,
        loss,
        issues,
    }
}

const COMP_DROPPED_LABELS: &[(&str, &str)] = &[
    ("sensor", "sensors"),
    ("motor", "motors"),
    ("hmi", "HMI elements"),
    ("dynamic_surface", "dynamic surfaces"),
    ("power_source", "power sources"),
    ("port", "ports"),
    ("antenna", "antennas"),
    ("frame", "coordinate frames"),
    ("extension", "vendor extensions"),
];

fn comp_dropped(ctx: &str, attr: &str, n: usize, findings: &mut Vec<Finding>) {
    if n == 0 {
        return;
    }
    let label = COMP_DROPPED_LABELS
        .iter()
        .find(|(a, _)| *a == attr)
        .map(|(_, l)| *l)
        .unwrap_or(attr);
    let code = format!("P_COMP_{}", attr.to_uppercase());
    findings.push(Finding::new(
        Tier::OutOfProfile,
        &code,
        format!("{ctx}: {n} {label} (HCDF-only; no URDF home)"),
    ));
}

const TOP_DROPPED_LABELS: &[(&str, &str)] = &[
    ("group", "joint groups"),
    ("state", "kinematic states"),
    ("network", "networks"),
    ("self_collision_disable", "self-collision-disable blocks"),
];

fn top_dropped(attr: &str, n: usize, findings: &mut Vec<Finding>) {
    if n == 0 {
        return;
    }
    let label = TOP_DROPPED_LABELS
        .iter()
        .find(|(a, _)| *a == attr)
        .map(|(_, l)| *l)
        .unwrap_or(attr);
    let code = format!("P_{}", attr.to_uppercase());
    findings.push(Finding::new(
        Tier::OutOfProfile,
        &code,
        format!("document: {n} {label} (HCDF-only; no URDF home)"),
    ));
}
