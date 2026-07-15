//! URDF <-> HCDF converter + profile FROZEN-GOLDEN harness.
//!
//! Rust is the source of truth (complete and lossless), so each corpus URDF is
//! pinned byte-for-byte to a committed golden produced BY the Rust converter itself, with no live Python
//! oracle. For each in-repo corpus URDF:
//!   (a) [`hcdformat::from_urdf_str`] -> serialized HCDF is frozen at `tests/golden/urdf/<stem>.import.hcdf`.
//!   (b) the imported doc exported with [`hcdformat::to_urdf`] -> URDF + its `LossManifest` are frozen at
//!       `tests/golden/urdf/<stem>.export.urdf` / `.loss.txt`.
//!   (c) [`hcdformat::check_profile`]'s tier label is frozen at `tests/golden/urdf/<stem>.profile.txt`.
//!
//! The corpus is the self-contained in-repo `tests/urdf` fixtures, so the gate runs unconditionally with
//! NO Python. Regenerate the goldens with `HCDF_REGEN_GOLDENS=1 cargo test`.
#![cfg(feature = "urdf")]

use hcdformat::{check_profile, from_urdf_str, to_urdf, Hcdf};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `<repo>` root: the crate sits at `<repo>/rust/hcdformat-rs`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The corpus URDFs the frozen goldens are committed for: the in-repo fixtures only, so the golden gate
/// is fully self-contained and deterministic in any checkout (no external robot corpus required). The
/// synthetic `synth-arm` is deliberately rich; `example-arm` and the pr2-style `prefixed-ns` (the only
/// fixture that exercises the inherited-namespace injection path) round out the mapping coverage.
fn corpus() -> Vec<PathBuf> {
    let repo = repo_root();
    let candidates = [
        repo.join("tests/urdf/synth-arm.urdf"),
        repo.join("tests/urdf/example-arm.urdf"),
        repo.join("tests/urdf/prefixed-ns.urdf"),
    ];
    candidates.into_iter().filter(|p| p.is_file()).collect()
}

/// The crate-local frozen-golden directory (`<crate>/tests/golden`).
fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Compare `actual` to the committed golden at `tests/golden/<rel>`; when `HCDF_REGEN_GOLDENS` is set,
/// (re)write the golden instead of asserting. The Rust converter's output IS the canonical golden now
/// (the Rust converter is complete and lossless), so a byte-for-byte match pins that output.
fn assert_golden(rel: &str, actual: &str) {
    let path = golden_dir().join(rel);
    if std::env::var_os("HCDF_REGEN_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden {} ({e}); regenerate with HCDF_REGEN_GOLDENS=1",
            path.display()
        )
    });
    assert_eq!(actual, expected, "golden mismatch for {}", path.display());
}

/// Serialize a [`hcdformat::LossManifest`]'s items as one `category\tdetail` line each (insertion order
/// preserved), the frozen-golden projection of the exporter's loss list.
fn loss_text(loss: &hcdformat::LossManifest) -> String {
    loss.items
        .iter()
        .map(|(c, d)| format!("{c}\t{d}\n"))
        .collect()
}

/// Every numeric attribute/text in the document, keyed by a structural path so two documents with the
/// same structure align. Only space-separated all-float values are collected (so names/uris are
/// ignored). Returns `path -> Vec<f64>`.
fn numeric_values(xml: &str) -> BTreeMap<String, Vec<f64>> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(xml);
    let mut out: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    // path stack with per-parent child indices for stable alignment
    let mut path: Vec<String> = Vec::new();
    let mut child_idx: Vec<BTreeMap<String, usize>> = vec![BTreeMap::new()];
    fn parse_nums(s: &str) -> Option<Vec<f64>> {
        let toks: Vec<&str> = s.split_whitespace().collect();
        if toks.is_empty() {
            return None;
        }
        let mut v = Vec::new();
        for t in toks {
            v.push(t.parse::<f64>().ok()?);
        }
        Some(v)
    }
    let handle_attrs = |path: &[String],
                        e: &quick_xml::events::BytesStart,
                        out: &mut BTreeMap<String, Vec<f64>>| {
        let cur = path.join("/");
        for a in e.attributes().flatten() {
            let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
            let val = String::from_utf8_lossy(&a.value).into_owned();
            if let Some(nums) = parse_nums(&val) {
                out.insert(format!("{cur}@{key}"), nums);
            }
        }
    };
    loop {
        match reader.read_event().expect("well-formed XML") {
            Event::Start(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let idx = {
                    let slot = child_idx.last_mut().unwrap();
                    let n = slot.entry(local.clone()).or_insert(0);
                    let i = *n;
                    *n += 1;
                    i
                };
                path.push(format!("{local}[{idx}]"));
                child_idx.push(BTreeMap::new());
                handle_attrs(&path, &e, &mut out);
            }
            Event::Empty(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let idx = {
                    let slot = child_idx.last_mut().unwrap();
                    let n = slot.entry(local.clone()).or_insert(0);
                    let i = *n;
                    *n += 1;
                    i
                };
                path.push(format!("{local}[{idx}]"));
                handle_attrs(&path, &e, &mut out);
                path.pop();
            }
            Event::Text(t) => {
                let txt = t.unescape().unwrap_or_default().into_owned();
                if let Some(nums) = parse_nums(&txt) {
                    let cur = path.join("/");
                    out.insert(format!("{cur}#text"), nums);
                }
            }
            Event::End(_) => {
                path.pop();
                child_idx.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out
}

/// A stable golden key stem for a corpus URDF (the file stem, e.g. `synth-arm`).
fn corpus_stem(urdf: &std::path::Path) -> String {
    urdf.file_stem().unwrap().to_string_lossy().into_owned()
}

#[test]
fn from_urdf_matches_frozen_golden_on_corpus() {
    let files = corpus();
    assert!(
        !files.is_empty(),
        "no corpus URDFs found (in-repo tests/urdf/*.urdf must exist)"
    );
    let mut checked = 0usize;
    for urdf in &files {
        let name = urdf.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(urdf).expect("read urdf");
        let (doc, _notes) =
            from_urdf_str(&src).unwrap_or_else(|e| panic!("{name}: Rust from_urdf failed: {e}"));
        let rust_hcdf = doc
            .to_xml_string()
            .unwrap_or_else(|e| panic!("{name}: Rust serialize failed: {e}"));
        // The frozen golden is the Rust importer's own HCDF output, byte-for-byte. It pins the
        // complete, lossless import with NO Python oracle in the path.
        assert_golden(
            &format!("urdf/{}.import.hcdf", corpus_stem(urdf)),
            &rust_hcdf,
        );
        checked += 1;
    }
    assert!(
        checked >= 1,
        "expected to exercise at least one corpus URDF, checked {checked}"
    );
}

#[test]
fn to_urdf_matches_frozen_golden_on_corpus() {
    let mut checked = 0usize;
    for urdf in &corpus() {
        let name = urdf.file_name().unwrap().to_string_lossy().into_owned();
        // Import with the Rust importer, then export with the Rust exporter: the natural pipeline now
        // that the Python oracle is gone. The exported URDF and its LossManifest are both frozen.
        let src = std::fs::read_to_string(urdf).expect("read urdf");
        let (doc, _notes) =
            from_urdf_str(&src).unwrap_or_else(|e| panic!("{name}: Rust from_urdf failed: {e}"));
        let (rust_urdf, rust_loss) =
            to_urdf(&doc).unwrap_or_else(|e| panic!("{name}: Rust to_urdf failed: {e}"));

        let stem = corpus_stem(urdf);
        assert_golden(&format!("urdf/{stem}.export.urdf"), &rust_urdf);
        assert_golden(&format!("urdf/{stem}.loss.txt"), &loss_text(&rust_loss));
        checked += 1;
    }
    assert!(
        checked >= 1,
        "expected to exercise at least one round-trip, checked {checked}"
    );
}

#[test]
fn profile_tier_matches_frozen_golden_on_corpus() {
    let mut checked = 0usize;
    for urdf in &corpus() {
        let name = urdf.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(urdf).expect("read urdf");
        let (doc, _notes) =
            from_urdf_str(&src).unwrap_or_else(|e| panic!("{name}: Rust from_urdf failed: {e}"));
        let rust_tier = check_profile(&doc).classification.label().to_string();
        assert_golden(
            &format!("urdf/{}.profile.txt", corpus_stem(urdf)),
            &rust_tier,
        );
        checked += 1;
    }
    assert!(
        checked >= 1,
        "expected to exercise the profile corpus, checked {checked}"
    );
}

// ── unit-level parity (no Python needed) ─────────────────────────────────────────────────────────

#[test]
fn partial_origin_does_not_fabricate_rpy_in_either_direction() {
    // A URDF <origin xyz=".."/> with NO rpy must import to an HCDF pose carrying xyz only, and re-export
    // to a URDF <origin xyz=".."/> with NO fabricated rpy="0 0 0", byte-faithful to the Python oracle,
    // whose Optional-string Pose preserves attribute absence. (Regression guard for the typed-[f64;3]
    // pose that used to zero-fill both attributes.)
    let urdf = r#"<robot name="p"><link name="a"/><link name="b"/>
        <joint name="j" type="fixed"><parent link="a"/><child link="b"/>
        <origin xyz="0 0 0.2"/></joint></robot>"#;
    let (doc, _) = from_urdf_str(urdf).expect("from_urdf");
    // import: the joint origin carries xyz, NOT rpy.
    let pose = doc.joint[0].origin.as_ref().expect("origin present");
    assert_eq!(pose.xyz, Some([0.0, 0.0, 0.2]), "xyz preserved");
    assert!(
        pose.rpy.is_none(),
        "rpy must stay ABSENT (no fabricated identity rpy)"
    );
    // serialized HCDF: the <origin> has an xyz attribute but no rpy attribute.
    let hcdf = doc.to_xml_string().expect("serialize");
    assert!(hcdf.contains(r#"xyz="0 0 0.2""#), "HCDF keeps xyz");
    assert!(
        !hcdf.contains(r#"rpy="0 0 0""#),
        "HCDF must not fabricate rpy: {hcdf}"
    );
    // export back to URDF: still xyz-only.
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(out.contains(r#"xyz="0 0 0.2""#), "URDF keeps xyz");
    assert!(
        !out.contains(r#"rpy="0 0 0""#),
        "URDF must not fabricate rpy: {out}"
    );
}

#[test]
fn origin_quat_xyzw_imports_into_pose_quat() {
    // The official urdfdom v1.2 pose type carries `quat_xyzw` (urdf.xsd:31, default "0 0 0 1") which maps
    // onto HCDF `pose/@quat` (hcdf.xsd:575): both scalar-last "x y z w". A `<origin quat_xyzw=..>` used
    // to import with `quat: None` (silent identity rotation); it now populates the typed quat.
    let urdf = r#"<robot name="q"><link name="a"/><link name="b"/>
        <joint name="j" type="fixed"><parent link="a"/><child link="b"/>
        <origin xyz="1 0 0" quat_xyzw="0.1 0.2 0.3 0.4"/></joint></robot>"#;
    let (doc, _) = from_urdf_str(urdf).expect("from_urdf");
    let pose = doc.joint[0].origin.as_ref().expect("origin present");
    let quat = pose
        .quat
        .expect("quat must be populated, not None/identity");
    assert_eq!(
        quat,
        [0.1, 0.2, 0.3, 0.4],
        "quat_xyzw x,y,z,w imported scalar-last: {quat:?}"
    );
    assert_eq!(
        pose.xyz,
        Some([1.0, 0.0, 0.0]),
        "translation preserved alongside quat"
    );
    // an <origin> WITHOUT quat_xyzw must NOT fabricate a quat.
    let urdf2 = r#"<robot name="q2"><link name="a"/><link name="b"/>
        <joint name="j" type="fixed"><parent link="a"/><child link="b"/>
        <origin xyz="1 0 0"/></joint></robot>"#;
    let (doc2, _) = from_urdf_str(urdf2).expect("from_urdf");
    assert!(
        doc2.joint[0].origin.as_ref().and_then(|p| p.quat).is_none(),
        "absent quat_xyzw must stay None (no fabricated identity quat)"
    );
}

#[test]
fn link_type_imports_and_round_trips_through_urdf_compat() {
    // link/@type (PR2 official form, urdf.xsd:177) maps to urdf-compat/@link-type (hcdf.xsd:2732). Export
    // already emitted it, so import used to be an export-only asymmetry that dropped @type on round-trip.
    let urdf = r#"<robot name="x"><link name="x" type="laser"/></robot>"#;
    let (doc, _) = from_urdf_str(urdf).expect("from_urdf");
    let link_type = doc.comp[0]
        .urdf_compat
        .as_ref()
        .and_then(|u| u.link_type.as_deref());
    assert_eq!(
        link_type,
        Some("laser"),
        "link @type imported into urdf-compat/@link-type"
    );
    // export re-emits <link type="laser"> and a re-import preserves it -> clean round-trip.
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"type="laser""#),
        "link @type re-emitted on export: {out}"
    );
    let (doc2, _) = from_urdf_str(&out).expect("re-import");
    assert_eq!(
        doc2.comp[0]
            .urdf_compat
            .as_ref()
            .and_then(|u| u.link_type.as_deref()),
        Some("laser"),
        "link @type survives the full round-trip"
    );
}

#[test]
fn collision_verbose_round_trips_through_dedicated_home() {
    // URDF `<verbose value="true"/>` is a collision CHILD element with a string value (urdf.xsd:161);
    // HCDF collision/@verbose is a boolean attr (hcdf.xsd:917) that exists so each collision round-trips
    // its own flag. Import used to hardcode None and export dropped-with-loss; both now honor the flag.
    let urdf = r#"<robot name="v">
        <link name="l">
          <collision><geometry><box size="1 1 1"/></geometry><verbose value="true"/></collision>
        </link></robot>"#;
    let (doc, _) = from_urdf_str(urdf).expect("from_urdf");
    // import: the <verbose value="true"> child becomes @verbose="true".
    assert_eq!(
        doc.comp[0].collision[0].verbose.as_deref(),
        Some("true"),
        "collision <verbose value=true> imported into @verbose"
    );
    // export: emitted as a <verbose value="true"/> child, NOT recorded as a loss.
    let (out, loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"<verbose value="true""#),
        "verbose re-emitted as a child element: {out}"
    );
    assert!(
        !loss.items.iter().any(|(_, d)| d.contains("verbose")),
        "verbose must NOT be recorded as a loss: {:?}",
        loss.items
    );
    // full round-trip: re-import keeps @verbose="true".
    let (doc2, _) = from_urdf_str(&out).expect("re-import");
    assert_eq!(
        doc2.comp[0].collision[0].verbose.as_deref(),
        Some("true"),
        "verbose survives round-trip"
    );

    // a collision WITHOUT <verbose> leaves @verbose absent, and export emits no <verbose> child.
    let urdf2 = r#"<robot name="v2">
        <link name="l"><collision><geometry><box size="1 1 1"/></geometry></collision></link></robot>"#;
    let (doc3, _) = from_urdf_str(urdf2).expect("from_urdf");
    assert!(
        doc3.comp[0].collision[0].verbose.is_none(),
        "absent <verbose> stays None"
    );
    let (out3, _) = to_urdf(&doc3).expect("to_urdf");
    assert!(
        !out3.contains("<verbose"),
        "no <verbose> child emitted when @verbose absent: {out3}"
    );
}

#[test]
fn limit_accel_decel_jerk_round_trip_through_urdf() {
    // acceleration/deceleration/jerk are first-class urdfdom v1.2 limit attributes (urdf.xsd:215-217,
    // mirroring urdfdom_headers JointLimits) with typed HCDF homes (joint.rs:82-94; hcdf.xsd:2760/2766/
    // 2763). Import used to hardcode them None (..Default) and export loss-noted "no URDF field"; both
    // now read + emit the attributes so a MoveIt/ros2_control trajectory limit survives the round-trip.
    let urdf = r#"<robot name="r">
        <link name="base"/><link name="arm"/>
        <joint name="j" type="revolute">
            <parent link="base"/><child link="arm"/>
            <axis xyz="0 0 1"/>
            <limit lower="-1" upper="1" effort="10" velocity="2" acceleration="5" deceleration="7" jerk="100"/>
        </joint></robot>"#;
    let (doc, _n) = from_urdf_str(urdf).expect("from_urdf");
    let lim = doc.joint[0].limit.as_ref().expect("limit imported");
    assert_eq!(
        lim.acceleration.as_deref(),
        Some("5"),
        "acceleration read on import"
    );
    assert_eq!(
        lim.deceleration.as_deref(),
        Some("7"),
        "deceleration read on import"
    );
    assert_eq!(lim.jerk.as_deref(), Some("100"), "jerk read on import");

    // export: the three attributes are emitted on <limit> and NOT recorded as a loss.
    let (out, loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"acceleration="5""#),
        "acceleration emitted: {out}"
    );
    assert!(
        out.contains(r#"deceleration="7""#),
        "deceleration emitted: {out}"
    );
    assert!(out.contains(r#"jerk="100""#), "jerk emitted: {out}");
    assert!(
        !loss.items.iter().any(|(_, d)| d.contains("acceleration")
            || d.contains("deceleration")
            || d.contains("jerk")),
        "accel/decel/jerk must NOT be recorded as a loss: {:?}",
        loss.items
    );

    // full round-trip: re-import preserves all three (and the base four).
    let (doc2, _n2) = from_urdf_str(&out).expect("re-import");
    let lim2 = doc2.joint[0].limit.as_ref().expect("limit survived");
    assert_eq!(lim2.lower.as_deref(), Some("-1"));
    assert_eq!(lim2.upper.as_deref(), Some("1"));
    assert_eq!(lim2.effort.as_deref(), Some("10"));
    assert_eq!(lim2.velocity.as_deref(), Some("2"));
    assert_eq!(
        lim2.acceleration.as_deref(),
        Some("5"),
        "acceleration survives round-trip"
    );
    assert_eq!(
        lim2.deceleration.as_deref(),
        Some("7"),
        "deceleration survives round-trip"
    );
    assert_eq!(
        lim2.jerk.as_deref(),
        Some("100"),
        "jerk survives round-trip"
    );

    // a <limit> without the extras leaves them absent and emits no such attributes.
    let bare = r#"<robot name="r2"><link name="b"/><link name="a"/>
        <joint name="j" type="revolute"><parent link="b"/><child link="a"/><axis xyz="0 0 1"/>
        <limit lower="0" upper="1" effort="1" velocity="1"/></joint></robot>"#;
    let (doc3, _) = from_urdf_str(bare).expect("from_urdf");
    let lim3 = doc3.joint[0].limit.as_ref().expect("limit");
    assert!(lim3.acceleration.is_none() && lim3.deceleration.is_none() && lim3.jerk.is_none());
    let (out3, _) = to_urdf(&doc3).expect("to_urdf");
    assert!(
        !out3.contains("acceleration=")
            && !out3.contains("deceleration=")
            && !out3.contains("jerk="),
        "absent extras emit no attributes: {out3}"
    );
}

#[test]
fn to_urdf_records_version_loss_only_when_version_present() {
    // A doc carrying @version reports a `[annotation] document version '..' dropped` loss (matching
    // to_urdf.py's loss loop); a doc WITHOUT @version reports none (Python's `version is not None` guard).
    let with_ver = r#"<hcdf name="v" version="1.0"><comp name="a"/></hcdf>"#;
    let (_u, loss) = to_urdf(&Hcdf::from_xml_str(with_ver).expect("parse")).expect("to_urdf");
    assert!(
        loss.items.iter().any(
            |(c, d)| c == "annotation" && d == "document version '1.0' dropped (no URDF field)"
        ),
        "version loss item missing: {:?}",
        loss.items
    );
    let no_ver = r#"<hcdf name="v"><comp name="a"/></hcdf>"#;
    let (_u, loss2) = to_urdf(&Hcdf::from_xml_str(no_ver).expect("parse")).expect("to_urdf");
    assert!(
        !loss2
            .items
            .iter()
            .any(|(_, d)| d.contains("document version")),
        "absent @version must NOT produce a version loss: {:?}",
        loss2.items
    );
}

#[test]
fn frd_body_frame_converts_pose_and_axis_on_export() {
    // An FRD document: a body pose + a joint axis must be converted to FLU (C = diag(1,-1,-1)).
    let hcdf = r#"<hcdf name="frd" version="1.0" body-frame="FRD" world-frame="ENU">
        <comp name="a"/><comp name="b"/>
        <joint name="j" type="revolute"><parent comp="a"/><child comp="b"/>
        <origin xyz="1 2 3" rpy="0 0 0"/><axis xyz="0 1 0"/>
        <limit lower="-1" upper="1" effort="1" velocity="1"/></joint></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse FRD hcdf");
    let (urdf, _loss) = to_urdf(&doc).expect("to_urdf");
    // origin xyz (1,2,3) -> C·(1,2,3) = (1,-2,-3); axis (0,1,0) -> (0,-1,0).
    let nums = numeric_values(&urdf);
    let origin = nums
        .iter()
        .find(|(k, _)| k.contains("origin") && k.ends_with("@xyz"))
        .map(|(_, v)| v.clone())
        .expect("origin xyz present");
    assert_eq!(origin, vec![1.0, -2.0, -3.0], "FRD->FLU origin");
    let axis = nums
        .iter()
        .find(|(k, _)| k.contains("axis") && k.ends_with("@xyz"))
        .map(|(_, v)| v.clone())
        .expect("axis xyz present");
    assert_eq!(axis, vec![0.0, -1.0, 0.0], "FRD->FLU axis");
}

#[test]
fn mesh_visual_becomes_glb_model_collision_keeps_mesh() {
    let urdf = r#"<robot name="m">
        <link name="l">
          <visual><geometry><mesh filename="pkg://x/v.dae"/></geometry></visual>
          <collision><geometry><mesh filename="pkg://x/c.stl" scale="2 2 2"/></geometry></collision>
        </link></robot>"#;
    let (doc, _) = from_urdf_str(urdf).expect("from_urdf");
    let comp = &doc.comp[0];
    // visual -> <model>, NOT <mesh>; collision -> <mesh> with the scale kept (unbaked).
    match &comp.visual[0].appearance {
        hcdformat::VisualAppearance::Model { model, .. } => {
            assert_eq!(model.uri.as_deref(), Some("pkg://x/v.dae"));
        }
        _ => panic!("visual mesh should map to a <model>"),
    }
    let mesh = comp.collision[0]
        .geometry
        .as_ref()
        .and_then(|g| g.mesh.as_ref())
        .expect("collision mesh");
    assert_eq!(mesh.uri.as_deref(), Some("pkg://x/c.stl"));
    assert_eq!(mesh.scale.as_deref(), Some("2 2 2"));
}

#[test]
fn fixed_joint_drops_axis_and_limit() {
    // <axis>/<limit> on a fixed joint are dropped (the joint has no DOF), with a note.
    let urdf = r#"<robot name="f"><link name="a"/><link name="b"/>
        <joint name="j" type="fixed"><parent link="a"/><child link="b"/>
        <axis xyz="0 0 1"/><limit lower="-1" upper="1" effort="1" velocity="1"/></joint></robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");
    let j = &doc.joint[0];
    assert!(j.axis.is_none(), "fixed joint axis must be dropped");
    assert!(j.limit.is_none(), "fixed joint limit must be dropped");
    assert!(notes
        .iter()
        .any(|n| n.contains("<axis>") && n.contains("no DOF")));
    assert!(notes
        .iter()
        .any(|n| n.contains("<limit>") && n.contains("no DOF")));
}

#[test]
fn toplevel_gazebo_ros2_control_quarantined_by_domain() {
    let urdf = r#"<robot name="q"><link name="a"/>
        <gazebo reference="a"><mu1>0.8</mu1></gazebo>
        <ros2_control name="c" type="system"><joint name="j"/></ros2_control></robot>"#;
    let (doc, _) = from_urdf_str(urdf).expect("from_urdf");
    let domains: Vec<&str> = doc.extension.iter().map(|e| e.domain.as_str()).collect();
    // A raw `<gazebo>` residual quarantines OPAQUE under `org.gazebosim.raw`, NOT the typed
    // `org.gazebosim` (whose body must be a `<gazebo-sim>` root); this avoids the domain collision.
    assert!(
        domains.contains(&"org.gazebosim.raw"),
        "gazebo residual -> org.gazebosim.raw: {domains:?}"
    );
    assert!(
        !domains.contains(&"org.gazebosim"),
        "raw residual must NOT collide with the typed domain: {domains:?}"
    );
    assert!(
        domains.contains(&"org.ros2.control"),
        "ros2_control -> org.ros2.control: {domains:?}"
    );
    // ros2_control is re-homed under the typed extension root <ros2-control> (schema
    // hcdf-ext-ros2-control.xsd), not left as an opaque <ros2_control> blob.
    let r2c = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.ros2.control")
        .expect("org.ros2.control extension");
    assert!(
        r2c.body.contains("<ros2-control"),
        "typed extension root: {}",
        r2c.body
    );
    assert!(
        !r2c.body.contains("<ros2_control"),
        "no raw URDF root in the typed body: {}",
        r2c.body
    );
    // On export the typed root is renamed back, so it re-emits and round-trips to a URDF <ros2_control>.
    let (urdf_out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        urdf_out.contains("<gazebo"),
        "gazebo de-merged back to top-level"
    );
    assert!(
        urdf_out.contains("<ros2_control"),
        "ros2_control de-merged back to top-level"
    );
    assert!(
        !urdf_out.contains("<ros2-control"),
        "typed root renamed back on export: {urdf_out}"
    );
}

/// A URDF `<ros2_control>` hardware-interface block (`<hardware><plugin>` + `<param>`, and a
/// `<joint>` with `<command_interface>`/`<state_interface>`, each with a per-interface `<param>`)
/// is re-homed under the TYPED extension root `<ros2-control>`
/// (domain `org.ros2.control`, schema `hcdf-ext-ros2-control.xsd`) so the body is schema-typed /
/// validatable rather than an opaque blob, and round-trips back out through `to_urdf` as a URDF
/// `<ros2_control>` that re-imports to the same typed shape.
#[test]
fn ros2_control_imports_into_typed_extension_and_round_trips() {
    let urdf = r#"<robot name="r">
        <link name="base"/>
        <ros2_control name="RArm" type="system">
            <hardware>
                <plugin>mock_components/GenericSystem</plugin>
                <param name="state_following_offset">0.0</param>
            </hardware>
            <joint name="j1">
                <param name="id">1</param>
                <command_interface name="position"/>
                <state_interface name="position">
                    <param name="initial_value">0.0</param>
                </state_interface>
                <state_interface name="velocity"/>
            </joint>
        </ros2_control>
    </robot>"#;

    let (doc, _notes) = from_urdf_str(urdf).expect("from_urdf");
    let ext = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.ros2.control")
        .expect("ros2_control -> org.ros2.control typed extension");
    // Typed root, URDF-native children preserved (hardware/plugin/joint/command_interface/state_interface).
    assert!(
        ext.body.contains("<ros2-control"),
        "typed extension root <ros2-control>: {}",
        ext.body
    );
    assert!(
        !ext.body.contains("<ros2_control"),
        "the raw URDF root is renamed away: {}",
        ext.body
    );
    for tag in [
        "<hardware",
        "<plugin",
        "<joint",
        "<command_interface",
        "<state_interface",
        "<param",
    ] {
        assert!(
            ext.body.contains(tag),
            "typed body keeps {tag}: {}",
            ext.body
        );
    }
    assert!(
        ext.body.contains(r#"name="RArm""#),
        "block @name preserved: {}",
        ext.body
    );
    assert!(
        ext.body.contains(r#"type="system""#),
        "block @type preserved: {}",
        ext.body
    );

    // Export back to URDF and re-import: the typed shape survives the round-trip.
    let (urdf_out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        urdf_out.contains("<ros2_control"),
        "emits a URDF <ros2_control>: {urdf_out}"
    );
    assert!(
        !urdf_out.contains("<ros2-control"),
        "no typed root leaks into the URDF: {urdf_out}"
    );
    assert!(
        urdf_out.contains("<command_interface"),
        "interfaces re-emitted: {urdf_out}"
    );

    let (doc2, _n2) = from_urdf_str(&urdf_out).expect("re-import exported URDF");
    let ext2 = doc2
        .extension
        .iter()
        .find(|e| e.domain == "org.ros2.control")
        .expect("ros2_control survived the round-trip");
    assert!(
        ext2.body.contains("<ros2-control"),
        "typed root re-established on re-import: {}",
        ext2.body
    );
    assert!(
        ext2.body.contains(r#"name="RArm""#),
        "block @name survives the round-trip: {}",
        ext2.body
    );
    assert!(
        ext2.body.contains(r#"name="j1""#),
        "joint survives the round-trip: {}",
        ext2.body
    );
}

/// The URDF/Gazebo sensor front end reaches into a quarantined `<gazebo reference=...>`
/// block, decomposes its `<sensor>` children through the SAME per-`@type` dispatch the SDF re-root uses,
/// attaches the typed sensors to the comp named by `@reference`, and leaves `<plugin>` (and un-mapped
/// sensor sub-fields) as residual `org.gazebosim.raw` extension. Requires the SDF dispatch (`feature = "sdf"`).
#[test]
#[cfg(feature = "sdf")]
fn gazebo_sensors_import_into_typed_sensors_residual_plugin_quarantined() {
    use hcdformat::model::enums::OpticalSensorType;
    let urdf = r#"<robot name="r">
        <link name="cam"/>
        <link name="body"/>
        <gazebo reference="cam">
            <sensor name="cam_sensor" type="camera">
                <update_rate>30</update_rate>
                <camera>
                    <horizontal_fov>1.047</horizontal_fov>
                    <image><width>640</width><height>480</height></image>
                    <clip><near>0.1</near><far>100</far></clip>
                </camera>
            </sensor>
            <plugin name="cam_ctrl" filename="libgazebo_ros_camera.so"><cameraName>c</cameraName></plugin>
        </gazebo>
        <gazebo reference="body">
            <sensor name="imu0" type="imu"/>
            <sensor name="lidar0" type="ray"><ray><range><min>0.1</min><max>30</max></range></ray></sensor>
            <sensor name="gps0" type="gps"/>
        </gazebo>
    </robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");

    // (1) the `cam` comp carries a typed optical/camera sensor, with @name and <update-rate> mapped.
    let cam = doc.comp.iter().find(|c| c.name == "cam").expect("cam comp");
    assert_eq!(cam.sensor.len(), 1, "cam has exactly one typed sensor");
    assert_eq!(cam.sensor[0].name.as_deref(), Some("cam_sensor"));
    assert_eq!(cam.sensor[0].update_rate.as_deref(), Some("30"));
    assert_eq!(
        cam.sensor[0].optical.first().and_then(|o| o.type_),
        Some(OpticalSensorType::Camera),
        "camera -> optical(Camera)"
    );

    // (2) imu/ray/gps on the `body` comp likewise decompose through the shared dispatch.
    let body = doc
        .comp
        .iter()
        .find(|c| c.name == "body")
        .expect("body comp");
    assert_eq!(body.sensor.len(), 3, "imu+ray+gps all typed");
    assert!(
        body.sensor.iter().any(|s| !s.inertial.is_empty()),
        "imu -> inertial"
    );
    assert!(
        body.sensor.iter().any(|s| s
            .optical
            .iter()
            .any(|o| o.type_ == Some(OpticalSensorType::Lidar))),
        "ray -> optical(Lidar)"
    );
    assert!(body.sensor.iter().any(|s| !s.rf.is_empty()), "gps -> rf");

    // (3) the residual gazebo <plugin> is still quarantined OPAQUE in org.gazebosim.raw; the un-mapped
    // camera <clip> rides along in the residual, and the typed <sensor>s are NOT duplicated back into it.
    let gz = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim.raw")
        .expect("org.gazebosim.raw extension");
    assert!(
        gz.body.contains("<plugin"),
        "residual <plugin> stays quarantined: {}",
        gz.body
    );
    assert!(
        gz.body.contains("libgazebo_ros_camera.so"),
        "plugin body preserved verbatim: {}",
        gz.body
    );
    assert!(
        gz.body.contains("<clip"),
        "un-mapped camera <clip> preserved in residual: {}",
        gz.body
    );
    assert!(
        !gz.body.contains("type=\"imu\""),
        "a fully-typed <sensor> is removed from the residual (no duplication): {}",
        gz.body
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("imported") && n.contains("org.gazebosim")),
        "{notes:?}"
    );

    // (4) round-trip: serialize -> reparse; the typed sensors and the residual extension both survive.
    let xml = doc.to_xml_string().expect("serialize");
    let doc2 = Hcdf::from_xml_str(&xml).expect("reparse");
    let cam2 = doc2
        .comp
        .iter()
        .find(|c| c.name == "cam")
        .expect("cam comp (reparse)");
    assert_eq!(
        cam2.sensor.len(),
        1,
        "typed sensor survives the HCDF round-trip"
    );
    assert!(
        doc2.extension
            .iter()
            .any(|e| e.domain == "org.gazebosim.raw" && e.body.contains("<plugin")),
        "residual gazebo plugin survives the HCDF round-trip"
    );
    // (4b) the imported camera's FoV carries the schema-required `<fov name>` (sensor_fov), so the
    // serialized HCDF passes its OWN xsd; an anonymous `<fov>` would fail validate --xsd.
    assert_eq!(
        cam2.sensor[0].optical[0].fov[0].name.as_deref(),
        Some("main"),
        "imported camera <fov> is named (schema-required)"
    );
    #[cfg(feature = "xsd")]
    assert!(
        hcdformat::validate_xsd(&xml).is_empty(),
        "imported-sensor HCDF must satisfy its own hcdf.xsd: {:?}",
        hcdformat::validate_xsd(&xml)
    );

    // (5) export to URDF (prefer-native): a typed optical CAMERA/LIDAR with NO urdf:sensor
    // quarantine (these were gazebo/SDF-sourced) now prefers a NATIVE robot-level `<sensor>`; the
    // remaining categories (imu/gnss) stay a `<gazebo reference><sensor>` block, and the residual gazebo
    // <plugin> de-merges back too.
    let (urdf_out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        urdf_out.contains("libgazebo_ros_camera.so"),
        "residual plugin re-emitted to URDF: {urdf_out}"
    );
    // the SDF-sourced camera -> a NATIVE <sensor><camera><image> (dims from intrinsics, hfov/near/far from
    // the derived frustum), NOT a <gazebo> block.
    assert!(
        urdf_out.contains(r#"<sensor name="cam_sensor" update_rate="30"><parent link="cam"/><camera><image width="640" height="480" hfov="1.047" near="0.1" far="100"/></camera></sensor>"#),
        "SDF-sourced camera exports as a native <sensor><camera>: {urdf_out}"
    );
    // the SDF-sourced ray -> a NATIVE <sensor><ray> (the <range>-only source has no angular scan grid, so
    // the ray is empty and the range is recorded as a loss), NOT a `type="gpu_lidar"` gazebo sensor.
    assert!(
        urdf_out.contains(r#"<sensor name="lidar0"><parent link="body"/><ray>"#),
        "SDF-sourced ray exports as a native <sensor><ray>: {urdf_out}"
    );
    assert!(
        !urdf_out.contains(r#"type="gpu_lidar""#),
        "no synthesized gazebo lidar (now native): {urdf_out}"
    );
    // the imu + gnss (no native URDF home) still ride a `<gazebo reference="body">` block, and the residual
    // gazebo plugin keeps `<gazebo reference="cam">`.
    assert!(
        urdf_out.contains(r#"<gazebo reference="body">"#),
        "imu/gnss re-emitted as a gazebo block: {urdf_out}"
    );
    assert!(
        urdf_out.contains(r#"<gazebo reference="cam">"#),
        "residual gazebo plugin block preserved: {urdf_out}"
    );
    assert!(
        urdf_out.contains(r#"type="imu""#),
        "typed imu re-emitted to URDF gazebo: {urdf_out}"
    );
    assert!(
        urdf_out.contains(r#"type="navsat""#),
        "typed gnss re-emitted to URDF gazebo: {urdf_out}"
    );
}

/// A NATIVE robot-level `<sensor>` (urdf.xsd:401, 327-341) with a
/// `<camera><image .../></camera>` decomposes into a typed `optical/@type="camera"` on the comp named by
/// its `<parent link>`: populated `camera_intrinsics` (dims/format + derived pinhole) and a frustum whose
/// near/far are the URDF `<image>` attributes FIRST-CLASS (no quarantine). The `<sensor>` is ALSO kept
/// quarantined verbatim for the byte-identical URDF round-trip. Pure-URDF path (no `sdf` feature needed).
#[test]
fn native_urdf_camera_sensor_decomposes_to_typed_optical() {
    use hcdformat::model::enums::{FrustumShape, OpticalSensorType};
    let urdf = r#"<robot name="r">
        <link name="base"/>
        <link name="head"/>
        <joint name="j" type="fixed"><parent link="base"/><child link="head"/></joint>
        <sensor name="head_cam" update_rate="30">
            <parent link="head"/>
            <origin xyz="0.1 0 0.2" rpy="0 0 0"/>
            <camera><image width="640" height="480" format="R8G8B8" hfov="1.5708" near="0.05" far="100"/></camera>
        </sensor></robot>"#;
    let (doc, _n) = from_urdf_str(urdf).expect("from_urdf");

    // Anchored to the comp named by <parent link>, NOT the root.
    let head = doc
        .comp
        .iter()
        .find(|c| c.name == "head")
        .expect("head comp");
    assert!(
        doc.comp
            .iter()
            .all(|c| c.name == "head" || c.sensor.is_empty()),
        "the sensor lands ONLY on the <parent link> comp"
    );
    assert_eq!(
        head.sensor.len(),
        1,
        "one typed sensor decomposed onto head"
    );
    let s = &head.sensor[0];
    assert_eq!(s.name.as_deref(), Some("head_cam"));
    assert_eq!(s.update_rate.as_deref(), Some("30"));
    let opt = s.optical.first().expect("optical camera");
    assert_eq!(opt.type_, Some(OpticalSensorType::Camera));
    assert_eq!(
        opt.pose.as_ref().and_then(|p| p.xyz),
        Some([0.1, 0.0, 0.2]),
        "<origin> -> optical pose"
    );

    let fov = opt.fov.first().expect("one fov");
    assert_eq!(
        fov.name.as_deref(),
        Some("main"),
        "schema-required <fov name>"
    );
    let intr = fov.intrinsics.as_ref().expect("camera intrinsics");
    assert_eq!(intr.width.as_deref(), Some("640"));
    assert_eq!(intr.height.as_deref(), Some("480"));
    assert_eq!(intr.format.as_deref(), Some("R8G8B8"));
    assert_eq!(intr.cx.as_deref(), Some("320"), "cx = width/2");
    assert_eq!(intr.cy.as_deref(), Some("240"), "cy = height/2");
    assert!(
        intr.fx.is_some() && intr.fx == intr.fy,
        "square-pixel fx=fy derived from hfov+width"
    );

    // near/far FOLD DIRECTLY into the frustum (first-class URDF attributes, NO quarantine of near/far).
    let fr = fov
        .geometry
        .as_ref()
        .and_then(|g| g.frustum.as_ref())
        .expect("frustum");
    assert_eq!(fr.shape, Some(FrustumShape::Pyramidal));
    assert_eq!(
        fr.near.as_deref(),
        Some("0.05"),
        "image @near -> frustum near (first-class)"
    );
    assert_eq!(
        fr.far.as_deref(),
        Some("100"),
        "image @far -> frustum far (first-class)"
    );
    assert_eq!(fr.hfov.as_deref(), Some("1.5708"));

    // The native <sensor> is ALSO kept quarantined verbatim (source of truth for the URDF round-trip).
    assert!(
        doc.extension
            .iter()
            .any(|e| e.domain == "urdf:sensor" && e.body.contains("head_cam")),
        "native <sensor> kept quarantined verbatim under urdf:sensor: {:?}",
        doc.extension
    );
}

/// The same native-camera URDF must round-trip URDF->HCDF->URDF with the `<sensor>` re-emitted BYTE-FOR-BYTE
/// via its verbatim `urdf:sensor` quarantine, and with NO synthesized `<gazebo><sensor>` duplicate for the
/// typed decomposition (the zero-regression crux: typed visibility added without a double emit).
#[test]
fn native_urdf_camera_sensor_round_trips_via_quarantine_no_duplicate() {
    let sensor_block = r#"<sensor name="head_cam" update_rate="30">
            <parent link="head"/>
            <origin xyz="0.1 0 0.2" rpy="0 0 0"/>
            <camera><image width="640" height="480" format="R8G8B8" hfov="1.5708" near="0.05" far="100"/></camera>
        </sensor>"#;
    let urdf = format!(
        r#"<robot name="r">
        <link name="base"/>
        <link name="head"/>
        <joint name="j" type="fixed"><parent link="base"/><child link="head"/></joint>
        {sensor_block}</robot>"#
    );
    let (doc, _n) = from_urdf_str(&urdf).expect("from_urdf");
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");

    // (1) the native <sensor> re-emits VERBATIM (byte-for-byte) from the quarantine.
    assert!(
        out.contains(sensor_block),
        "native <sensor> must re-emit byte-identical via the quarantine:\n{out}"
    );
    // (2) NO synthesized <gazebo> block for the same sensor (no duplication).
    assert!(
        !out.contains("<gazebo"),
        "no synthesized <gazebo> duplicate for the native sensor:\n{out}"
    );
    assert_eq!(
        out.matches("<sensor ").count(),
        1,
        "the sensor is emitted exactly once:\n{out}"
    );

    // (3) re-import is idempotent: still exactly one typed sensor (the quarantine did not spawn a second).
    let (doc2, _) = from_urdf_str(&out).expect("re-import");
    let head2 = doc2
        .comp
        .iter()
        .find(|c| c.name == "head")
        .expect("head comp (reparse)");
    assert_eq!(
        head2.sensor.len(),
        1,
        "re-import yields exactly one typed sensor (no growth)"
    );
}

/// A native `<sensor><ray><horizontal .../></ray>` (urdf.xsd:308-325) decomposes into a typed
/// `optical/@type="lidar"` with a populated `<lidar-params>/<scan-pattern>` on the `<parent link>` comp,
/// and the `<sensor>` still round-trips verbatim with no synthesized `<gazebo>` duplicate.
#[test]
fn native_urdf_ray_sensor_decomposes_to_typed_lidar() {
    use hcdformat::model::enums::OpticalSensorType;
    let urdf = r#"<robot name="r">
        <link name="base"/>
        <link name="scan"/>
        <joint name="j" type="fixed"><parent link="base"/><child link="scan"/></joint>
        <sensor name="hokuyo" update_rate="40">
            <parent link="scan"/>
            <ray>
                <horizontal samples="720" resolution="1" min_angle="-2.0" max_angle="2.0"/>
            </ray>
        </sensor></robot>"#;
    let (doc, _n) = from_urdf_str(urdf).expect("from_urdf");

    let scan = doc
        .comp
        .iter()
        .find(|c| c.name == "scan")
        .expect("scan comp");
    assert_eq!(scan.sensor.len(), 1, "one typed lidar decomposed onto scan");
    assert_eq!(scan.sensor[0].name.as_deref(), Some("hokuyo"));
    assert_eq!(scan.sensor[0].update_rate.as_deref(), Some("40"));
    let opt = scan.sensor[0].optical.first().expect("optical lidar");
    assert_eq!(opt.type_, Some(OpticalSensorType::Lidar));
    let sp = opt
        .lidar_params
        .as_ref()
        .and_then(|lp| lp.scan_pattern.as_ref())
        .expect("scan-pattern");
    let h = sp.horizontal.as_ref().expect("horizontal scan axis");
    assert_eq!(h.samples.as_deref(), Some("720"));
    assert_eq!(h.resolution.as_deref(), Some("1"));
    assert_eq!(h.min_angle.as_deref(), Some("-2.0"));
    assert_eq!(h.max_angle.as_deref(), Some("2.0"));
    assert!(
        sp.vertical.is_none(),
        "no <vertical> in the source -> None (presence-preserving)"
    );

    // Verbatim quarantine kept; export emits the sensor once (no <gazebo> duplicate).
    let (out, _) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"<sensor name="hokuyo" update_rate="40">"#),
        "verbatim re-emit: {out}"
    );
    assert!(
        !out.contains("<gazebo"),
        "no synthesized <gazebo> duplicate for the native ray: {out}"
    );
}

/// Prefer-native, camera: an SDF-sourced / hand-authored typed optical CAMERA with NO
/// `urdf:sensor` quarantine exports to URDF as a NATIVE robot-level `<sensor><camera><image>`: dims/format
/// from the typed intrinsics, hfov/near/far from the FoV frustum (the inverse of the native-sensor import,
/// urdf.xsd:288-341), NOT a `<gazebo reference><sensor type="camera">` block. Pure-URDF path.
#[test]
fn sdf_sourced_camera_exports_as_native_urdf_sensor() {
    let hcdf = r#"<hcdf name="r" version="1.0"><comp name="cam_link">
        <sensor name="front_cam" update-rate="30">
          <optical type="camera">
            <fov name="main">
              <geometry><frustum shape="pyramidal"><near>0.05</near><far>100</far><hfov>1.5708</hfov></frustum></geometry>
              <intrinsics><width>640</width><height>480</height><format>R8G8B8</format></intrinsics>
            </fov>
          </optical>
        </sensor>
      </comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse hcdf");
    // No native `urdf:sensor` quarantine -> the camera takes the prefer-native path.
    assert!(
        doc.extension.iter().all(|e| e.domain != "urdf:sensor"),
        "the hand-authored camera has no urdf:sensor quarantine"
    );
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"<sensor name="front_cam" update_rate="30"><parent link="cam_link"/><camera><image width="640" height="480" format="R8G8B8" hfov="1.5708" near="0.05" far="100"/></camera></sensor>"#),
        "SDF-sourced camera exports as a native <sensor><camera><image>: {out}"
    );
    assert!(
        !out.contains("<gazebo"),
        "no <gazebo> block for a native camera: {out}"
    );
    assert_eq!(
        out.matches("<sensor ").count(),
        1,
        "the sensor is emitted exactly once: {out}"
    );
}

/// Prefer-native, the lidar half: an SDF-sourced typed optical LIDAR with NO `urdf:sensor`
/// quarantine exports as a NATIVE `<sensor><ray><horizontal|vertical>` (the angular scan grid, urdf.xsd:
/// 308-325), NOT a `type="gpu_lidar"` gazebo sensor.
#[test]
fn sdf_sourced_lidar_exports_as_native_urdf_ray() {
    let hcdf = r#"<hcdf name="r" version="1.0"><comp name="scan_link">
        <sensor name="scan0" update-rate="15">
          <optical type="lidar">
            <lidar-params><scan-pattern>
              <horizontal samples="360" resolution="1" min-angle="-3.14" max-angle="3.14"/>
            </scan-pattern></lidar-params>
          </optical>
        </sensor>
      </comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse hcdf");
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"<sensor name="scan0" update_rate="15"><parent link="scan_link"/><ray><horizontal samples="360" resolution="1" min_angle="-3.14" max_angle="3.14"/></ray></sensor>"#),
        "SDF-sourced lidar exports as a native <sensor><ray>: {out}"
    );
    assert!(
        !out.contains("<gazebo"),
        "no <gazebo> block for a native ray: {out}"
    );
    assert!(
        !out.contains(r#"type="gpu_lidar""#),
        "no synthesized gazebo lidar: {out}"
    );
}

/// A URDF-native `<sensor>` (which IS quarantined verbatim under `urdf:sensor`)
/// still round-trips BYTE-IDENTICALLY via the quarantine; the prefer-native path does NOT double-emit it
/// (no `<gazebo>` duplicate, exactly one `<sensor>`), a no-regression guard for the pre-existing quarantine
/// contract. Requires the SDF-shared mappers only for the friction path, so it is a pure-URDF test here.
#[test]
fn native_urdf_camera_still_round_trips_via_quarantine_no_native_dup() {
    let sensor_block = r#"<sensor name="head_cam" update_rate="30">
            <parent link="head"/>
            <origin xyz="0.1 0 0.2" rpy="0 0 0"/>
            <camera><image width="640" height="480" format="R8G8B8" hfov="1.5708" near="0.05" far="100"/></camera>
        </sensor>"#;
    let urdf = format!(
        r#"<robot name="r"><link name="base"/><link name="head"/>
        <joint name="j" type="fixed"><parent link="base"/><child link="head"/></joint>
        {sensor_block}</robot>"#
    );
    let (doc, _n) = from_urdf_str(&urdf).expect("from_urdf");
    // The native <sensor> is BOTH typed on the comp AND kept quarantined verbatim under urdf:sensor.
    assert!(
        doc.extension
            .iter()
            .any(|e| e.domain == "urdf:sensor" && e.body.contains("head_cam")),
        "native <sensor> kept quarantined verbatim under urdf:sensor"
    );
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(sensor_block),
        "native <sensor> re-emits byte-identical via the quarantine (NOT a synthesized native block):\n{out}"
    );
    assert!(
        !out.contains("<gazebo"),
        "no synthesized <gazebo> duplicate: {out}"
    );
    assert_eq!(
        out.matches("<sensor ").count(),
        1,
        "the sensor is emitted exactly once: {out}"
    );
}

/// An IMU (no native URDF grammar) still exports as a `<gazebo reference><sensor
/// type="imu">` block; the prefer-native split only reroutes optical camera/lidar, never other categories.
#[test]
fn imu_still_exports_as_gazebo_sensor() {
    let hcdf = r#"<hcdf name="r" version="1.0"><comp name="imu_link">
        <sensor name="imu0"><inertial type="accel_gyro"/></sensor>
      </comp></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse hcdf");
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"<gazebo reference="imu_link"><sensor name="imu0" type="imu">"#),
        "IMU exports as a <gazebo reference><sensor type=\"imu\">: {out}"
    );
    assert!(
        !out.contains("<parent link="),
        "an IMU is NOT emitted as a native robot-level <sensor>: {out}"
    );
}

#[test]
fn safety_controller_moves_to_urdf_compat() {
    let urdf = r#"<robot name="s"><link name="a"/><link name="b"/>
        <joint name="j" type="revolute"><parent link="a"/><child link="b"/><axis xyz="0 0 1"/>
        <limit lower="-1" upper="1" effort="1" velocity="1"/>
        <safety_controller soft_lower_limit="-0.9" soft_upper_limit="0.9" k_position="10" k_velocity="1"/>
        </joint></robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");
    let sc = doc.joint[0]
        .urdf_compat
        .as_ref()
        .and_then(|u| u.safety_controller.as_ref())
        .expect("safety_controller under urdf-compat");
    assert_eq!(sc.k_velocity.as_deref(), Some("1"));
    assert!(notes.iter().any(|n| n.contains("urdf-compat")));
}

#[test]
fn import_then_export_is_a_clean_roundtrip_for_a_primitive_arm() {
    let urdf = std::fs::read_to_string(repo_root().join("tests/urdf/example-arm.urdf"));
    let Ok(urdf) = urdf else {
        eprintln!("skipping: example-arm.urdf not present");
        return;
    };
    let (doc, _) = from_urdf_str(&urdf).expect("from_urdf");
    let (out, loss) = to_urdf(&doc).expect("to_urdf");
    // A primitive-only arm has no GLB/mesh, so export is lossless.
    assert!(
        loss.is_empty(),
        "primitive arm should export losslessly, got: {}",
        loss.text()
    );
    // every source link/joint name survives the round-trip.
    for needle in ["base", "upper_arm", "shoulder"] {
        assert!(out.contains(needle), "round-trip URDF missing {needle:?}");
    }
}

/// COVERAGE for the inherited-namespace injection on quarantined top-level URDF children
/// (`capture_toplevel_raw` / `with_inherited_ns` in `src/from_urdf.rs`). The in-repo + out-of-repo
/// corpus carries NO prefixed top-level content, so the differential parity tests never enter this
/// branch; this unit test exercises it directly with a pr2-style document so a regression in the path
/// is caught by `cargo test` on a clean checkout (not silently shipped).
///
/// The contract mirrors lxml `copy.deepcopy(el)` + serialize: a quarantined top-level child gets
/// re-declared exactly the inherited `xmlns:<prefix>` declarations its subtree REFERENCES, and nothing
/// more, so a USED prefix is carried down (keeping the verbatim block well-formed when reparsed
/// standalone) while an UNUSED root declaration is not.
#[test]
fn prefixed_toplevel_child_inherits_only_the_used_namespace() {
    // <robot> declares xmlns:controller (USED by the top-level child + its subtree) and
    // xmlns:unused (declared but referenced by nothing).
    let urdf = r#"<?xml version="1.0"?>
<robot name="pr2" xmlns:controller="http://ros.org/controller" xmlns:unused="http://example.com/unused">
  <link name="base"/>
  <controller:gazebo_ros_controller_manager name="cm">
    <controller:rate>100</controller:rate>
  </controller:gazebo_ros_controller_manager>
</robot>"#;
    let (doc, _notes) = from_urdf_str(urdf).expect("from_urdf");
    let xml = doc.to_xml_string().expect("to_xml_string");
    // the prefixed top-level child is quarantined into an <extension> verbatim block...
    assert!(
        xml.contains("controller:gazebo_ros_controller_manager"),
        "prefixed top-level child must survive as a verbatim extension block: {xml}"
    );
    // ...with the USED prefix re-declared on the captured top element (so it reparses standalone)...
    assert!(
        xml.contains(r#"xmlns:controller="http://ros.org/controller""#),
        "the referenced xmlns:controller must be injected onto the quarantined child: {xml}"
    );
    // ...and the UNUSED root declaration NOT carried down (byte-parity with lxml deepcopy).
    assert!(
        !xml.contains("xmlns:unused"),
        "the unreferenced xmlns:unused must NOT be injected: {xml}"
    );
    // the injected declaration must appear exactly once (not duplicated per subtree reference).
    assert_eq!(
        xml.matches(r#"xmlns:controller="http://ros.org/controller""#)
            .count(),
        1,
        "the inherited namespace must be declared once, on the top-level element: {xml}"
    );
}

/// COVERAGE companion: a SELF-CLOSING prefixed top-level child (the `Event::Empty` arm of
/// `capture_toplevel_raw`) must likewise inherit the namespace it references.
#[test]
fn self_closing_prefixed_toplevel_child_inherits_namespace() {
    let urdf = r#"<?xml version="1.0"?>
<robot name="r" xmlns:ctl="http://ros.org/ctl">
  <link name="base"/>
  <ctl:manager rate="100"/>
</robot>"#;
    let (doc, _notes) = from_urdf_str(urdf).expect("from_urdf");
    let xml = doc.to_xml_string().expect("to_xml_string");
    assert!(
        xml.contains("ctl:manager"),
        "self-closing prefixed child must survive: {xml}"
    );
    assert!(
        xml.contains(r#"xmlns:ctl="http://ros.org/ctl""#),
        "the referenced namespace must be injected onto the self-closing child: {xml}"
    );
}

// ── lenient pre-pass: real-world URDF quirks (sanitize_urdf in src/from_urdf.rs) ────────────────

/// Quirk 1 (ingenuity.urdf pattern): a top-level `<joint>` with NO `type` attribute. `urdf-rs` alone
/// hard-rejects it ("missing field type"); RViz treats it as fixed. The lenient pre-pass injects
/// `type="fixed"` with a note naming the joint, so the import SUCCEEDS as a fixed joint.
#[test]
fn typeless_joint_treated_as_fixed_with_note() {
    let urdf = r#"<robot name="t"><link name="a"/><link name="b"/>
        <joint name="j"><origin xyz="0 0 1"/><parent link="a"/><child link="b"/></joint></robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("typeless joint must convert leniently");
    assert_eq!(
        doc.joint[0].type_,
        Some(hcdformat::model::enums::JointType::Fixed),
        "a typeless joint maps to fixed"
    );
    // the rest of the joint is mapped normally (the pre-pass edits ONLY the tag's type attribute).
    assert_eq!(
        doc.joint[0].origin.as_ref().and_then(|p| p.xyz),
        Some([0.0, 0.0, 1.0])
    );
    assert!(
        notes
            .iter()
            .any(|n| n == "joint 'j' has no type; treated as fixed"),
        "expected the treated-as-fixed note, got: {notes:?}"
    );
}

/// Quirk 2 (H2_dae.urdf pattern): TWO `<material>` elements directly inside one `<visual>` (an
/// exporter bug urdf-rs rejects as "duplicate field material"). The pre-pass keeps the FIRST and
/// drops the rest with a note; the first material's colour is what reaches the asset hint (mesh
/// visual) / the primitive colour.
#[test]
fn duplicate_visual_materials_keep_first_with_note() {
    let urdf = r#"<robot name="d">
        <material name="red"><color rgba="1 0 0 1"/></material>
        <material name="blue"><color rgba="0 0 1 1"/></material>
        <link name="l">
          <visual><geometry><mesh filename="m.stl"/></geometry>
            <material name="red"/>
            <material name="blue"/>
          </visual>
          <visual><geometry><box size="1 1 1"/></geometry>
            <material name="m1"><color rgba="0 1 0 1"/></material>
            <material name="m2"><color rgba="1 1 0 1"/></material>
          </visual>
        </link></robot>"#;
    let (doc, notes, hints) = hcdformat::from_urdf_str_with_assets(urdf)
        .expect("dup-material visual must convert leniently");
    // mesh visual: the FIRST material (red, resolved via the top-level palette) reaches the bake hint.
    assert_eq!(hints.len(), 1, "one hint for the one mesh visual");
    assert_eq!(
        hints[0].color.as_deref(),
        Some("1 0 0 1"),
        "first <material> wins"
    );
    // primitive visual: the FIRST inline material's colour is kept on the typed <color>.
    match &doc.comp[0].visual[1].appearance {
        hcdformat::VisualAppearance::Primitive { color, .. } => {
            let c = color.as_ref().expect("primitive keeps a color");
            assert_eq!(
                c.rgba.as_deref(),
                Some("0 1 0 1"),
                "first inline <material> wins"
            );
        }
        _ => panic!("box visual should stay a primitive"),
    }
    // one note per affected visual, counting the dropped duplicates.
    for visual in ["l_visual_0", "l_visual_1"] {
        assert!(
            notes.iter().any(|n| n
                == &format!(
                    "visual '{visual}': 1 duplicate <material> element(s) dropped (first one wins)"
                )),
            "expected a duplicate-material note for {visual}, got: {notes:?}"
        );
    }
}

/// Robot-level NAMED materials resolve into BOTH colour paths (the H2.urdf pattern:
/// `<material name="white"><color rgba=".."/></material>` at robot level, each visual carrying only
/// `<material name="white"/>`): a mesh visual's resolved rgba lands on the bake hint (so the baker
/// folds it into the GLB, before this the STLs baked gray), and a primitive visual keeps a flat
/// `<color name>` referencing the document palette entry that carries the rgba. Inline rgba still
/// wins over the palette; an unresolvable name keeps a colour-less hint with a sharper dropped-note.
#[test]
fn robot_level_named_materials_resolve_into_both_color_paths() {
    let urdf = r#"<robot name="m">
        <material name="white"><color rgba="0.7 0.7 0.7 1"/></material>
        <link name="a"><visual><geometry><mesh filename="meshes/a.stl"/></geometry>
            <material name="white"/></visual></link>
        <link name="b"><visual><geometry><box size="1 1 1"/></geometry>
            <material name="white"/></visual></link>
        <link name="c"><visual><geometry><mesh filename="meshes/c.stl"/></geometry>
            <material name="ghost"/></visual></link>
        <link name="d"><visual><geometry><mesh filename="meshes/d.stl"/></geometry>
            <material name="white"><color rgba="1 0 0 1"/></material></visual></link>
        </robot>"#;
    let (doc, notes, hints) = hcdformat::from_urdf_str_with_assets(urdf).expect("from_urdf");
    let hint = |comp: &str| {
        hints
            .iter()
            .find(|h| h.comp == comp)
            .unwrap_or_else(|| panic!("hint for {comp}"))
    };
    // ARM A: the named reference resolves through the robot-level palette onto the bake hint.
    assert_eq!(
        hint("a").color.as_deref(),
        Some("0.7 0.7 0.7 1"),
        "named ref resolves onto the hint"
    );
    // Inline rgba wins over the same-named palette entry.
    assert_eq!(
        hint("d").color.as_deref(),
        Some("1 0 0 1"),
        "inline rgba wins over the palette"
    );
    // An unresolvable name carries no colour.
    assert_eq!(
        hint("c").color,
        None,
        "unresolvable name gives a colour-less hint"
    );
    // Notes: a RESOLVED colour is 'carried' (bakes into the GLB), never 'dropped'; an unresolvable
    // name is 'dropped' with the offending name spelled out.
    assert!(
        notes.iter().any(|n| n
            == "visual 'a_visual_0': <material> color '0.7 0.7 0.7 1' carried on the asset \
                side-channel (bakes into the GLB at the asset step, not into the HCDF document)"),
        "expected a carried-color note for a_visual_0, got: {notes:?}"
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.starts_with("visual 'a_visual_0'") && n.contains("dropped")),
        "a resolved color must not note a drop: {notes:?}"
    );
    assert!(
        notes.iter().any(|n| {
            n
            == "visual 'c_visual_0': <material> references 'ghost' which resolves to no flat rgba; \
                dropped (the GLB keeps the mesh's own appearance)"
        }),
        "expected a sharper dropped note for c_visual_0, got: {notes:?}"
    );
    // ARM B: the primitive keeps a flat <color> naming the palette entry; the rgba lives on the
    // document-level palette (which named-only refs resolve through, hcdviz's resolve_color).
    let b = doc.comp.iter().find(|c| c.name == "b").expect("comp b");
    match &b.visual[0].appearance {
        hcdformat::VisualAppearance::Primitive { color, .. } => {
            let c = color.as_ref().expect("primitive keeps a flat color");
            assert_eq!(
                c.name.as_deref(),
                Some("white"),
                "the named ref lands on the flat <color>"
            );
        }
        _ => panic!("box visual should stay a primitive"),
    }
    assert!(
        doc.color
            .iter()
            .any(|c| c.name.as_deref() == Some("white")
                && c.rgba.as_deref() == Some("0.7 0.7 0.7 1")),
        "the document palette carries the named color's rgba"
    );
}

/// The pre-pass is SURGICAL: a typeless `<joint>` that is NOT a top-level kinematic joint (here the
/// `<joint name>` endpoint inside a `<transmission>`, where a type attribute is neither present nor
/// meaningful) must NOT be rewritten to `type="fixed"` and must NOT emit a "has no type" note. The
/// `<transmission>` is adopted into the TYPED core (no longer an
/// `org.ros.control` blob), so the endpoint surfaces as the transmission's typed `<joint ref>`.
#[test]
fn typeless_joint_inside_transmission_is_not_rewritten() {
    use hcdformat::model::enums::TransmissionType;
    let urdf = r#"<robot name="s"><link name="a"/>
        <transmission name="tr"><type>simple</type><joint name="tj"><hardwareInterface>effort</hardwareInterface></joint></transmission>
        </robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");
    // No verbatim quarantine of the transmission any more.
    assert!(
        !doc.extension.iter().any(|e| e.domain == "org.ros.control"),
        "the transmission must be adopted into the typed core, not quarantined to org.ros.control"
    );
    let tr = doc
        .transmission
        .iter()
        .find(|t| t.name.as_deref() == Some("tr"))
        .expect("typed transmission present");
    assert_eq!(
        tr.type_,
        Some(TransmissionType::Simple),
        "<type>simple</type> -> @type=simple"
    );
    assert_eq!(
        tr.joint.first().and_then(|e| e.ref_.as_deref()),
        Some("tj"),
        "the transmission <joint name> endpoint maps to the typed <joint ref>"
    );
    assert!(
        !notes.iter().any(|n| n.contains("treated as fixed")),
        "no treated-as-fixed note for a transmission joint: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("hardwareInterface") && n.contains("no core-transmission home")),
        "the <hardwareInterface> residual is recorded as a note: {notes:?}"
    );
}

/// A full URDF `<transmission>` (`<type>`, `<actuator name>`
/// with `<mechanicalReduction>`, and `<joint name>`) imports into the TYPED core `<transmission>`
/// (name, @type, motor/joint endpoints, the load-bearing reduction) rather than being dead-lettered to
/// `org.ros.control`, and round-trips back out through `to_urdf` as a URDF `<transmission>` that
/// re-imports to the same typed shape.
#[test]
fn transmission_imports_into_typed_core_and_round_trips() {
    use hcdformat::model::enums::TransmissionType;
    let urdf = r#"<robot name="r">
        <link name="base"/>
        <link name="arm"/>
        <joint name="arm_joint" type="revolute">
            <parent link="base"/><child link="arm"/>
            <axis xyz="0 0 1"/>
            <limit lower="-1" upper="1" effort="10" velocity="2"/>
        </joint>
        <transmission name="arm_trans">
            <type>transmission_interface/SimpleTransmission</type>
            <joint name="arm_joint">
                <hardwareInterface>hardware_interface/EffortJointInterface</hardwareInterface>
            </joint>
            <actuator name="arm_motor">
                <mechanicalReduction>100</mechanicalReduction>
                <hardwareInterface>hardware_interface/EffortJointInterface</hardwareInterface>
            </actuator>
        </transmission>
    </robot>"#;

    let (doc, _notes) = from_urdf_str(urdf).expect("from_urdf");
    assert!(
        !doc.extension.iter().any(|e| e.domain == "org.ros.control"),
        "the transmission is no longer quarantined to org.ros.control"
    );
    assert_eq!(doc.transmission.len(), 1, "exactly one typed transmission");
    let tr = &doc.transmission[0];
    assert_eq!(tr.name.as_deref(), Some("arm_trans"));
    assert_eq!(tr.type_, Some(TransmissionType::Simple));
    assert_eq!(
        tr.motor.first().and_then(|e| e.ref_.as_deref()),
        Some("arm_motor")
    );
    assert_eq!(
        tr.joint.first().and_then(|e| e.ref_.as_deref()),
        Some("arm_joint")
    );
    assert_eq!(
        tr.reduction.as_deref(),
        Some("100"),
        "the load-bearing gear ratio"
    );

    // Export back to URDF and re-import: the typed shape survives the round-trip.
    let (urdf_out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        urdf_out.contains(r#"<transmission name="arm_trans">"#),
        "emits a typed <transmission>: {urdf_out}"
    );
    assert!(
        urdf_out.contains("<mechanicalReduction>100</mechanicalReduction>"),
        "reduction re-emitted: {urdf_out}"
    );
    let (doc2, _n2) = from_urdf_str(&urdf_out).expect("re-import exported URDF");
    let tr2 = doc2
        .transmission
        .first()
        .expect("transmission survived the round-trip");
    assert_eq!(tr2.name.as_deref(), Some("arm_trans"));
    assert_eq!(tr2.type_, Some(TransmissionType::Simple));
    assert_eq!(
        tr2.motor.first().and_then(|e| e.ref_.as_deref()),
        Some("arm_motor")
    );
    assert_eq!(
        tr2.joint.first().and_then(|e| e.ref_.as_deref()),
        Some("arm_joint")
    );
    assert_eq!(tr2.reduction.as_deref(), Some("100"));
}

#[test]
fn transmission_type_attribute_form_imports() {
    use hcdformat::model::enums::TransmissionType;
    // The official PR2 urdf.xsd:288 models @type as a REQUIRED ATTRIBUTE (vs the modern ros_control
    // `<type>` child). Import now falls back to the attribute when the child is absent. (`type_` is the
    // enum TransmissionType, so the value must map (a class path or bare literal) rather than an
    // arbitrary string; a class path via the attribute maps by its leaf name just like the child.)
    let urdf = r#"<robot name="t"><transmission name="t" type="transmission_interface/SimpleTransmission"><joint name="j"/></transmission></robot>"#;
    let (doc, _n) = from_urdf_str(urdf).expect("from_urdf");
    let tr = doc
        .transmission
        .first()
        .expect("attribute-form transmission imported");
    assert_eq!(tr.name.as_deref(), Some("t"));
    assert_eq!(
        tr.type_,
        Some(TransmissionType::Simple),
        "@type attribute read on import"
    );
    assert_eq!(tr.joint.first().and_then(|e| e.ref_.as_deref()), Some("j"));

    // A bare HCDF enum literal in the attribute maps too.
    let bare = r#"<robot name="t2"><transmission name="t2" type="differential"><joint name="j"/></transmission></robot>"#;
    let (doc2, _) = from_urdf_str(bare).expect("from_urdf");
    assert_eq!(
        doc2.transmission[0].type_,
        Some(TransmissionType::Differential)
    );

    // The `<type>` CHILD still wins when both are present (modern form takes precedence).
    let both = r#"<robot name="t3"><transmission name="t3" type="differential"><type>transmission_interface/SimpleTransmission</type><joint name="j"/></transmission></robot>"#;
    let (doc3, _) = from_urdf_str(both).expect("from_urdf");
    assert_eq!(
        doc3.transmission[0].type_,
        Some(TransmissionType::Simple),
        "child <type> wins over @type"
    );
}

#[test]
fn transmission_pr2_mechanics_are_loss_noted_not_silently_dropped() {
    // The legacy PR2 sub-elements (urdf.xsd:265-277) have no typed-core home and are dropped from the
    // verbatim quarantine once the <transmission> is consumed; they must at least be flagged.
    let urdf = r#"<robot name="pr2">
        <transmission name="gripper_trans" type="pr2_mechanism_model/PR2GripperTransmission">
            <gap_joint name="l_gripper_joint"/>
            <passive_joint name="l_gripper_l_finger_joint"/>
            <use_simulated_gripper_joint/>
        </transmission></robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");
    assert_eq!(
        doc.transmission.len(),
        1,
        "transmission still consumed into the typed core"
    );
    assert!(
        notes.iter().any(|n| n.contains("gripper_trans")
            && n.contains("PR2 mechanics")
            && n.contains("gap_joint")
            && n.contains("passive_joint")
            && n.contains("use_simulated_gripper_joint")),
        "PR2 mechanics must be loss-noted, not silently dropped: {notes:?}"
    );

    // A transmission WITHOUT any PR2 sub-elements produces no such note.
    let plain = r#"<robot name="p"><transmission name="p"><type>transmission_interface/SimpleTransmission</type><joint name="j"/></transmission></robot>"#;
    let (_d, notes2) = from_urdf_str(plain).expect("from_urdf");
    assert!(
        !notes2.iter().any(|n| n.contains("PR2 mechanics")),
        "no PR2 note when none present: {notes2:?}"
    );
}

/// Export symmetry: URDF/ros_control has NO gearbox and is single-joint/single-actuator, so a GEAR
/// transmission coupling (two `<joint>` endpoints, the SDF-gearbox shape) is loss-noted on `to_urdf`
/// (generalized to a SimpleTransmission, with the extra endpoints flattened), while a plain SIMPLE
/// reduction exports unchanged with no such loss.
#[test]
fn to_urdf_loss_notes_gear_coupling_but_leaves_simple_unchanged() {
    use hcdformat::model::enums::TransmissionType;
    use hcdformat::model::{Endpoint, Transmission};

    let mut doc = Hcdf {
        name: "r".to_string(),
        version: "1.0".to_string(),
        ..Default::default()
    };
    // A gear coupling (the gearbox shape): two joint endpoints, no URDF home.
    doc.transmission.push(Transmission {
        name: Some("knee_gearbox".to_string()),
        type_: Some(TransmissionType::Gear),
        joint: vec![
            Endpoint {
                ref_: Some("out".to_string()),
                role: Some("driven".to_string()),
            },
            Endpoint {
                ref_: Some("ref_body".to_string()),
                role: Some("reference".to_string()),
            },
        ],
        reduction: Some("4".to_string()),
        ..Default::default()
    });
    // A plain simple reduction: one motor + one joint, a faithful URDF transmission.
    doc.transmission.push(Transmission {
        name: Some("arm_trans".to_string()),
        type_: Some(TransmissionType::Simple),
        motor: vec![Endpoint {
            ref_: Some("arm_motor".to_string()),
            role: None,
        }],
        joint: vec![Endpoint {
            ref_: Some("arm_joint".to_string()),
            role: None,
        }],
        reduction: Some("100".to_string()),
        ..Default::default()
    });

    let (urdf_out, loss) = to_urdf(&doc).expect("to_urdf");
    // The gear coupling has no ros_control class (generalized to Simple) AND its two joint endpoints are
    // flattened; both are honest loss notes; URDF has no gearbox.
    assert!(
        loss.items.iter().any(|(_, d)| d.contains("knee_gearbox")
            && d.contains("gear")
            && d.contains("no ros_control class")),
        "gear coupling must be loss-noted (no URDF gearbox); got {:?}",
        loss.items
    );
    assert!(
        loss.items
            .iter()
            .any(|(_, d)| d.contains("knee_gearbox") && d.contains("multi-endpoint")),
        "the gearbox's extra joint endpoint must be loss-noted on flattening; got {:?}",
        loss.items
    );
    // The simple reduction exports unchanged: no gear/multi-endpoint loss mentions it.
    assert!(
        !loss.items.iter().any(|(_, d)| d.contains("arm_trans")
            && (d.contains("no ros_control class") || d.contains("multi-endpoint"))),
        "a simple reduction must export unchanged; got {:?}",
        loss.items
    );
    assert!(
        urdf_out.contains(r#"<transmission name="arm_trans">"#),
        "simple transmission still emits: {urdf_out}"
    );
    assert!(
        urdf_out.contains("<mechanicalReduction>100</mechanicalReduction>"),
        "simple reduction re-emitted: {urdf_out}"
    );
}

/// The out-of-repo real-world quirk corpus (staged ad hoc; the `HCDF_QUIRK_URDF_DIR` env var
/// overrides the default staging location `~/hcdf-test-staging`). Tests skip cleanly when a file
/// is absent, like the other out-of-repo corpora.
fn real_quirk_urdf(name: &str) -> Option<String> {
    let dir = std::env::var_os("HCDF_QUIRK_URDF_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join("hcdf-test-staging")
        });
    std::fs::read_to_string(dir.join(name)).ok()
}

/// REAL FILE: NASA ingenuity.urdf carries a `<joint name="MHS_NAVCAM">` with no type attribute
/// (line 242); strict urdf-rs alone fails with "missing field type". The lenient import must
/// SUCCEED, mapping that joint to fixed with the naming note.
#[test]
fn real_ingenuity_typeless_joint_converts_as_fixed() {
    let Some(src) = real_quirk_urdf("ingenuity.urdf") else {
        eprintln!("skipping: ingenuity.urdf not present");
        return;
    };
    let (doc, notes) = from_urdf_str(&src).expect("ingenuity.urdf must convert leniently");
    let j = doc
        .joint
        .iter()
        .find(|j| j.name.as_deref() == Some("MHS_NAVCAM"))
        .expect("MHS_NAVCAM joint present");
    assert_eq!(j.type_, Some(hcdformat::model::enums::JointType::Fixed));
    assert!(
        notes
            .iter()
            .any(|n| n == "joint 'MHS_NAVCAM' has no type; treated as fixed"),
        "expected the MHS_NAVCAM note, got: {notes:?}"
    );
}

/// REAL FILE: Unitree H2_dae.urdf's visuals carry TWO `<material>` elements each (exporter bug);
/// strict urdf-rs alone fails with "duplicate field material". The lenient import must SUCCEED,
/// keeping the first material of every affected visual and noting each drop.
#[test]
fn real_h2_dae_duplicate_materials_convert_first_wins() {
    let Some(src) = real_quirk_urdf("H2_dae.urdf") else {
        eprintln!("skipping: H2_dae.urdf not present");
        return;
    };
    let (doc, notes) = from_urdf_str(&src).expect("H2_dae.urdf must convert leniently");
    assert!(!doc.comp.is_empty(), "links imported");
    assert!(
        notes
            .iter()
            .any(|n| n.contains("duplicate <material>") && n.contains("first one wins")),
        "expected duplicate-material notes, got: {notes:?}"
    );
}

/// REAL FILE: Unitree H2.urdf defines its whole palette at robot level (`<material name="white">` /
/// `"dark"`, each with an rgba) and every visual references it by NAME only. The named references
/// must resolve onto the bake hints, which is what colours the STL bakes; a colour-less hint bakes
/// a uniform gray GLB.
#[test]
fn real_h2_robot_level_named_materials_reach_the_hints() {
    let Some(src) = real_quirk_urdf("H2.urdf") else {
        eprintln!("skipping: H2.urdf not present");
        return;
    };
    let (_doc, notes, hints) =
        hcdformat::from_urdf_str_with_assets(&src).expect("H2.urdf must convert");
    let color_of = |comp: &str| {
        hints
            .iter()
            .find(|h| h.comp == comp)
            .unwrap_or_else(|| panic!("hint for {comp}"))
            .color
            .clone()
    };
    // pelvis references 'white', the hip pitch links reference 'dark'; both robot-level.
    assert_eq!(
        color_of("pelvis").as_deref(),
        Some("0.7 0.7 0.7 1"),
        "pelvis resolves 'white'"
    );
    for hip in ["left_hip_pitch_link", "right_hip_pitch_link"] {
        assert_eq!(
            color_of(hip).as_deref(),
            Some("0.4 0.4 0.4 1"),
            "{hip} resolves 'dark'"
        );
    }
    // Every H2 mesh visual references a defined material, so EVERY hint carries a resolved colour
    // and no material is noted as dropped.
    assert!(
        hints.iter().all(|h| h.color.is_some()),
        "every H2 hint carries a resolved colour"
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("<material>") && n.contains("dropped")),
        "no dropped-material notes for H2, got: {notes:?}"
    );
}

/// The official URDF XSD makes an inline `<material>`'s `@name` OPTIONAL, but urdf-rs types it as a
/// required `String`, so a real published URDF (Unitree h1_2) with a nameless inline material is
/// hard-rejected by the strict gate: the WHOLE document fails to import. The lenient pre-pass
/// synthesizes a deterministic, distinctively-prefixed name so the document imports; the injected name
/// carries the material's own rgba, a named sibling material keeps its authored name (no collision),
/// and each injection is noted.
#[test]
fn nameless_inline_material_gets_synthesized_name_and_imports() {
    let urdf = r#"<robot name="r">
        <link name="base">
            <visual name="v0">
                <geometry><box size="1 1 1"/></geometry>
                <material><color rgba="1 0 0 1"/></material>
            </visual>
        </link>
        <link name="tip">
            <visual>
                <geometry><sphere radius="0.5"/></geometry>
                <material name="blue"><color rgba="0 0 1 1"/></material>
            </visual>
        </link>
    </robot>"#;
    // Before the fix, urdf-rs's "missing field name" rejection made this `.expect` panic; the whole
    // document was unimportable purely because of the nameless inline material.
    let (doc, notes) = from_urdf_str(urdf).expect("nameless inline material must import leniently");
    // The kept material's rgba survives, tagged with the deterministic synthesized name.
    match &doc
        .comp
        .iter()
        .find(|c| c.name == "base")
        .expect("base comp")
        .visual[0]
        .appearance
    {
        hcdformat::VisualAppearance::Primitive { color, .. } => {
            let c = color.as_ref().expect("nameless material keeps its color");
            assert_eq!(
                c.rgba.as_deref(),
                Some("1 0 0 1"),
                "the nameless material's rgba is captured"
            );
            assert_eq!(
                c.name.as_deref(),
                Some("__hcdf_unnamed_material_v0"),
                "deterministic, distinctively-prefixed synthesized name"
            );
        }
        _ => panic!("box visual should stay a primitive"),
    }
    assert!(
        notes
            .iter()
            .any(|n| n.contains("inline <material> has no name")
                && n.contains("__hcdf_unnamed_material_v0")),
        "expected a synthesized-name note, got: {notes:?}"
    );
    // A named sibling material is untouched; the injection never collides with an authored name.
    match &doc
        .comp
        .iter()
        .find(|c| c.name == "tip")
        .expect("tip comp")
        .visual[0]
        .appearance
    {
        hcdformat::VisualAppearance::Primitive { color, .. } => {
            let c = color.as_ref().expect("named sibling keeps its color");
            assert_eq!(
                c.name.as_deref(),
                Some("blue"),
                "named sibling keeps its authored name"
            );
        }
        _ => panic!("sphere visual should stay a primitive"),
    }
    // Exactly one synthesized name (only the one nameless material), so no over-injection.
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.contains("inline <material> has no name"))
            .count(),
        1,
        "exactly one nameless material was named: {notes:?}"
    );
}

/// A URDF `<material><texture filename=..>` on a MESH visual is no longer dropped: the diffuse texture
/// rides the `VisualAssetHint` side-channel (like scale/colour) so the GLB baker can embed it. Both an
/// INLINE texture and a top-level material referenced BY NAME resolve onto the hint, and the note
/// records the capture instead of a drop.
#[test]
fn material_texture_rides_the_visual_asset_hint() {
    let urdf = r#"<robot name="t">
        <material name="brick"><texture filename="tex/brick.png"/></material>
        <link name="inline">
            <visual name="iv">
                <geometry><mesh filename="a.stl"/></geometry>
                <material name="wood"><texture filename="tex/wood.png"/></material>
            </visual>
        </link>
        <link name="byname">
            <visual name="bv">
                <geometry><mesh filename="b.stl"/></geometry>
                <material name="brick"/>
            </visual>
        </link>
    </robot>"#;
    let (_doc, notes, hints) =
        hcdformat::from_urdf_str_with_assets(urdf).expect("textured materials must import");
    let tex_of = |visual: &str| {
        hints
            .iter()
            .find(|h| h.visual == visual)
            .unwrap_or_else(|| panic!("hint for {visual}"))
            .texture
            .clone()
    };
    // Inline `<texture>` off the visual's own material reaches the hint.
    assert_eq!(
        tex_of("iv").as_deref(),
        Some("tex/wood.png"),
        "inline <texture> reaches the hint"
    );
    // A top-level material referenced by name resolves its texture through the palette onto the hint.
    assert_eq!(
        tex_of("bv").as_deref(),
        Some("tex/brick.png"),
        "top-level material texture resolves by name onto the hint"
    );
    // The note records a CARRY, not a drop.
    assert!(
        notes.iter().any(|n| n.contains("texture 'tex/wood.png'")
            && n.contains("carried on the asset side-channel")),
        "expected an inline texture-carry note, got: {notes:?}"
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("visual 'iv'") && n.contains("dropped")),
        "the textured mesh visual must not be recorded as a drop: {notes:?}"
    );
}

/// The joint parent/child endpoint comp refs that resolve to no `<comp>`: the same invariant
/// dendrite_build's `validate_references` guards before Save, duplicated here so the importer's own
/// tests can assert an imported document is self-consistent without depending on the app crate.
fn dangling_endpoint_refs(doc: &Hcdf) -> Vec<String> {
    let names: std::collections::HashSet<&str> = doc.comp.iter().map(|c| c.name.as_str()).collect();
    let mut out = Vec::new();
    for j in &doc.joint {
        for (role, ep) in [("parent", &j.parent), ("child", &j.child)] {
            if let Some(name) = ep.as_ref().and_then(|e| e.comp.as_deref()) {
                if !names.contains(name) {
                    out.push(format!("{role} -> {name}"));
                }
            }
        }
    }
    out
}

/// A massless FRAME-LINK (a `<link>` with only a name, no inertial/visual/collision) that is a joint
/// child must still become a comp, so its joint edge resolves. This is the common, well-formed case
/// (URDF connector links); it must import with the comp present, ZERO dangling refs, and NO synthesis
/// note (the link is declared, not fabricated).
#[test]
fn empty_frame_link_child_becomes_comp_no_dangling() {
    let urdf = r#"<robot name="r">
        <link name="base"/>
        <link name="frame"/>
        <joint name="j" type="fixed">
            <parent link="base"/>
            <child link="frame"/>
        </joint>
    </robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");
    assert!(
        doc.comp.iter().any(|c| c.name == "frame"),
        "the empty frame-link must become a comp"
    );
    assert!(
        dangling_endpoint_refs(&doc).is_empty(),
        "a declared frame-link child must not dangle: {:?}",
        dangling_endpoint_refs(&doc)
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("synthesized an empty stub comp")),
        "a DECLARED link must not trigger stub synthesis: {notes:?}"
    );
}

/// A malformed URDF whose `<joint>` `<child>` references a link no `<link>` declares (a scrambled
/// find-replace, perseverance.urdf's failure mode) must import with a synthesized empty stub comp for
/// the undeclared endpoint (so the joint edge resolves and Save is not blocked), plus a note. A
/// well-formed sibling joint in the same document must not gain a stub or a note.
#[test]
fn undeclared_joint_child_gets_stub_comp_and_note() {
    let urdf = r#"<robot name="r">
        <link name="base"/>
        <link name="ok_link"/>
        <joint name="good" type="fixed">
            <parent link="base"/>
            <child link="ok_link"/>
        </joint>
        <joint name="broken" type="fixed">
            <parent link="base"/>
            <child link="Frame_STEER_LF"/>
        </joint>
    </robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");
    let stub = doc
        .comp
        .iter()
        .find(|c| c.name == "Frame_STEER_LF")
        .expect("undeclared child must be synthesized as a comp");
    // A stub is name-only, the same shape a massless frame-link imports to.
    assert!(
        stub.inertial.is_none() && stub.visual.is_empty() && stub.collision.is_empty(),
        "the stub comp must be empty (name only)"
    );
    assert!(
        dangling_endpoint_refs(&doc).is_empty(),
        "no endpoint may dangle after synthesis: {:?}",
        dangling_endpoint_refs(&doc)
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("Frame_STEER_LF") && n.contains("synthesized an empty stub comp")),
        "expected a stub-synthesis note for the undeclared link, got: {notes:?}"
    );
    // The well-formed 'ok_link' child must NOT be treated as undeclared.
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.contains("synthesized an empty stub comp"))
            .count(),
        1,
        "exactly one stub synthesized (only the undeclared endpoint): {notes:?}"
    );
}

/// REAL FILE: David Dorf's perseverance.urdf has a scrambled find-replace: four joint `<child>`s
/// reference `Frame_STEER_LF` / `_LR` / `_RF` / `_RR`, none of which is declared as a `<link>` (each
/// pair's other side is), so strict import leaves four dangling child edges (the "chaotic blue mess").
/// The lenient import must synthesize a stub comp for each, so the document imports with ZERO dangling
/// endpoint refs, and its sibling ingenuity.urdf (well-formed here) synthesizes nothing.
#[test]
fn real_perseverance_undeclared_steer_links_get_stubs() {
    let Some(src) = real_quirk_urdf("perseverance.urdf") else {
        eprintln!("skipping: perseverance.urdf not present");
        return;
    };
    let (doc, notes) = from_urdf_str(&src).expect("perseverance.urdf must convert leniently");
    assert!(
        dangling_endpoint_refs(&doc).is_empty(),
        "perseverance.urdf must import with 0 dangling refs, got: {:?}",
        dangling_endpoint_refs(&doc)
    );
    for name in ["Frame_STEER_LF", "_LR", "_RF", "_RR"] {
        assert!(
            doc.comp.iter().any(|c| c.name == name),
            "expected a synthesized stub comp for undeclared link {name:?}"
        );
    }
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.contains("synthesized an empty stub comp"))
            .count(),
        4,
        "exactly the four undeclared STEER endpoints get stubs: {notes:?}"
    );
}

/// URDF `<geometry><capsule radius length>` used to be DROPPED: the primitive
/// matcher returned `None`, so the visual/collision imported as an EMPTY geometry with an
/// "unsupported/empty <geometry>" note. The typed `<capsule>` home already exists (and the SDF path
/// populates it), so URDF now mirrors that mapping on BOTH the visual and collision paths.
#[test]
fn urdf_capsule_imports_as_typed_capsule_geometry() {
    let urdf = r#"<robot name="cap">
        <link name="l">
          <visual><geometry><capsule radius="0.05" length="0.2"/></geometry></visual>
          <collision><geometry><capsule radius="0.06" length="0.3"/></geometry></collision>
        </link></robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");
    let comp = &doc.comp[0];
    // visual -> typed <capsule> primitive (NOT an empty default geometry).
    match &comp.visual[0].appearance {
        hcdformat::VisualAppearance::Primitive { geometry, .. } => {
            let cap = geometry
                .as_ref()
                .and_then(|g| g.capsule.as_ref())
                .expect("visual capsule geometry");
            assert_eq!(cap.radius.as_deref(), Some("0.05"), "visual capsule radius");
            assert_eq!(cap.length.as_deref(), Some("0.2"), "visual capsule length");
        }
        other => panic!("capsule visual should be a Primitive, got {other:?}"),
    }
    // collision -> typed <capsule>.
    let cg = comp.collision[0]
        .geometry
        .as_ref()
        .expect("collision geometry");
    let cap = cg.capsule.as_ref().expect("collision capsule geometry");
    assert_eq!(
        cap.radius.as_deref(),
        Some("0.06"),
        "collision capsule radius"
    );
    assert_eq!(
        cap.length.as_deref(),
        Some("0.3"),
        "collision capsule length"
    );
    // the old "dropped" behavior is gone: no unsupported/empty-geometry note.
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("unsupported/empty <geometry>")),
        "capsule must not be reported as unsupported/empty: {notes:?}"
    );
    // the imported HCDF satisfies its own hcdf.xsd.
    #[cfg(feature = "xsd")]
    {
        let xml = doc.to_xml_string().expect("serialize");
        assert!(
            hcdformat::validate_xsd(&xml).is_empty(),
            "capsule HCDF must satisfy hcdf.xsd: {:?}",
            hcdformat::validate_xsd(&xml)
        );
    }
}

/// Export half: `<capsule>` is standard URDF v1.2 (urdf.xsd:101-114) and used to
/// be DROPPED on export (bucketed with cone/ellipsoid as "has no URDF primitive"), so URDF->HCDF->URDF
/// silently lost every capsule. Both the visual AND the collision writer now emit `<capsule radius
/// length>`, closing the round-trip; only `<cone>`/`<ellipsoid>` remain unrepresentable.
#[test]
fn urdf_capsule_survives_export_round_trip_visual_and_collision() {
    let urdf = r#"<robot name="cap">
        <link name="l">
          <visual><geometry><capsule radius="0.05" length="0.2"/></geometry></visual>
          <collision><geometry><capsule radius="0.06" length="0.3"/></geometry></collision>
        </link></robot>"#;
    let (doc, _notes) = from_urdf_str(urdf).expect("from_urdf");
    let (out, loss) = to_urdf(&doc).expect("to_urdf");
    // BOTH the visual and the collision capsule are emitted with their exact dimensions.
    assert_eq!(
        out.matches("<capsule").count(),
        2,
        "both the visual and collision capsule must be emitted: {out}"
    );
    assert!(
        out.contains(r#"radius="0.05""#) && out.contains(r#"length="0.2""#),
        "visual capsule dims: {out}"
    );
    assert!(
        out.contains(r#"radius="0.06""#) && out.contains(r#"length="0.3""#),
        "collision capsule dims: {out}"
    );
    // no "has no URDF primitive; dropped" loss for the capsules.
    assert!(
        !loss.items.iter().any(|(_, d)| d.contains("capsule")),
        "capsule must NOT be recorded as a loss: {:?}",
        loss.items
    );
    // full round-trip: re-import the exported URDF and confirm the typed capsules survive unchanged.
    let (doc2, _n2) = from_urdf_str(&out).expect("re-import exported URDF");
    let comp = &doc2.comp[0];
    match &comp.visual[0].appearance {
        hcdformat::VisualAppearance::Primitive { geometry, .. } => {
            let cap = geometry
                .as_ref()
                .and_then(|g| g.capsule.as_ref())
                .expect("visual capsule");
            assert_eq!(cap.radius.as_deref(), Some("0.05"));
            assert_eq!(cap.length.as_deref(), Some("0.2"));
        }
        other => panic!("capsule visual should re-import as a Primitive, got {other:?}"),
    }
    let cap = comp.collision[0]
        .geometry
        .as_ref()
        .and_then(|g| g.capsule.as_ref())
        .expect("collision capsule");
    assert_eq!(cap.radius.as_deref(), Some("0.06"));
    assert_eq!(cap.length.as_deref(), Some("0.3"));
}

/// The Gazebo-classic universal friction idiom
/// `<gazebo reference="link"><mu1>/<mu2>/<kp>/<kd>` used to be quarantined verbatim while the collision
/// `<surface>` was hard-coded `None`. It now populates the referenced comp's collision `<surface>`,
/// mapping EXACTLY like the SDF `<surface>` path (mu1 -> friction @static, mu2 -> friction-direction/@mu2,
/// kp -> contact @stiffness, kd -> @damping); the rest of the block (here a `<material>` override) stays quarantined,
/// and the consumed friction leaves are NOT duplicated back into the residual. Requires the SDF mappers.
#[test]
#[cfg(feature = "sdf")]
fn urdf_gazebo_friction_maps_to_typed_collision_surface() {
    let urdf = r#"<robot name="fr">
        <link name="wheel">
          <collision><geometry><cylinder radius="0.1" length="0.05"/></geometry></collision>
        </link>
        <gazebo reference="wheel">
          <mu1>0.8</mu1>
          <mu2>0.9</mu2>
          <kp>100000</kp>
          <kd>10</kd>
          <material>Gazebo/Black</material>
        </gazebo></robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");
    let wheel = doc
        .comp
        .iter()
        .find(|c| c.name == "wheel")
        .expect("wheel comp");
    let surf = wheel.collision[0]
        .surface
        .as_ref()
        .expect("collision <surface> populated from the gazebo friction idiom");
    let fric = surf.friction.as_ref().expect("friction mapped");
    assert_eq!(
        fric.static_.as_deref(),
        Some("0.8"),
        "mu1 -> friction @static"
    );
    assert!(
        fric.dynamic.is_none(),
        "@dynamic unset: gazebo mu2 is not kinetic friction"
    );
    assert_eq!(
        fric.friction_direction
            .as_ref()
            .and_then(|d| d.mu2.as_deref()),
        Some("0.9"),
        "mu2 -> friction-direction/@mu2"
    );
    let contact = surf.contact.as_ref().expect("contact mapped");
    assert_eq!(
        contact.stiffness.as_deref(),
        Some("100000"),
        "kp -> contact @stiffness"
    );
    assert_eq!(
        contact.damping.as_deref(),
        Some("10"),
        "kd -> contact @damping"
    );
    // the residual <material> stays quarantined; the mapped friction leaves do NOT (no duplication).
    let gz = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim.raw")
        .expect("org.gazebosim.raw extension for the residual <material>");
    assert!(
        gz.body.contains("Gazebo/Black"),
        "residual <material> preserved verbatim: {}",
        gz.body
    );
    assert!(
        !gz.body.contains("<mu1"),
        "mapped friction not re-quarantined: {}",
        gz.body
    );
    assert!(
        !gz.body.contains("<mu2"),
        "mapped friction not re-quarantined: {}",
        gz.body
    );
    assert!(
        !gz.body.contains("<kp"),
        "mapped contact not re-quarantined: {}",
        gz.body
    );
    assert!(
        !gz.body.contains("<kd"),
        "mapped contact not re-quarantined: {}",
        gz.body
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("mapped <mu1>") && n.contains("<surface>")),
        "a mapping note is recorded: {notes:?}"
    );
    // round-trip: serialize -> reparse; the typed surface survives, and the HCDF is schema-valid.
    let xml = doc.to_xml_string().expect("serialize");
    let doc2 = Hcdf::from_xml_str(&xml).expect("reparse");
    let wheel2 = doc2
        .comp
        .iter()
        .find(|c| c.name == "wheel")
        .expect("wheel comp (reparse)");
    assert!(
        wheel2.collision[0].surface.is_some(),
        "collision <surface> survives the HCDF round-trip"
    );
    #[cfg(feature = "xsd")]
    assert!(
        hcdformat::validate_xsd(&xml).is_empty(),
        "gazebo-friction HCDF must satisfy hcdf.xsd: {:?}",
        hcdformat::validate_xsd(&xml)
    );
}

/// The Gazebo-classic friction/contact idiom survives a FULL URDF -> HCDF -> URDF
/// round-trip. On import `<gazebo reference><mu1>/<mu2>/<kp>/<kd>/<fdir1>/<slip1>/<slip2>` is mapped into
/// the typed collision `<surface>` and STRIPPED from the org.gazebosim.raw quarantine (the typed surface is
/// the sole carrier); `to_urdf::write_gazebo_friction` now re-emits it as the EXACT inverse block, so it is
/// no longer lost. Requires the SDF mappers (`feature = "sdf"`).
#[test]
#[cfg(feature = "sdf")]
fn urdf_gazebo_friction_round_trips_through_hcdf() {
    let urdf = r#"<robot name="fr">
        <link name="wheel">
          <collision><geometry><cylinder radius="0.1" length="0.05"/></geometry></collision>
        </link>
        <gazebo reference="wheel">
          <mu1>0.8</mu1>
          <mu2>0.9</mu2>
          <kp>100000</kp>
          <kd>10</kd>
          <fdir1>1 0 0</fdir1>
          <slip1>0.01</slip1>
          <slip2>0.02</slip2>
        </gazebo></robot>"#;
    let (doc, _notes) = from_urdf_str(urdf).expect("from_urdf");
    // Sanity: the friction idiom was consumed (not left in the raw quarantine).
    assert!(
        doc.extension
            .iter()
            .all(|e| e.domain != "org.gazebosim.raw" || !e.body.contains("<mu1")),
        "friction idiom must be consumed from the quarantine, not duplicated"
    );

    // Export back to URDF: the friction idiom must reappear, byte-faithfully to the import.
    let (out, loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"<gazebo reference="wheel">"#),
        "gazebo reference block re-emitted: {out}"
    );
    for leaf in [
        "<mu1>0.8</mu1>",
        "<mu2>0.9</mu2>",
        "<kp>100000</kp>",
        "<kd>10</kd>",
        "<fdir1>1 0 0</fdir1>",
        "<slip1>0.01</slip1>",
        "<slip2>0.02</slip2>",
    ] {
        assert!(out.contains(leaf), "friction leaf {leaf} re-emitted: {out}");
    }
    // The previously-recorded "surface contact physics dropped" loss is GONE (friction now round-trips);
    // and no kinetic/restitution residual loss here (there is none in this surface).
    assert!(
        !loss
            .items
            .iter()
            .any(|(c, d)| c == "collision" && d.contains("<surface>")),
        "no surface loss: friction round-trips instead of dropping: {:?}",
        loss.items
    );

    // Full closure: re-import the exported URDF and confirm the typed surface is identical.
    let (doc2, _) = from_urdf_str(&out).expect("re-import exported URDF");
    let s1 = doc
        .comp
        .iter()
        .find(|c| c.name == "wheel")
        .unwrap()
        .collision[0]
        .surface
        .as_ref()
        .unwrap();
    let s2 = doc2
        .comp
        .iter()
        .find(|c| c.name == "wheel")
        .unwrap()
        .collision[0]
        .surface
        .as_ref()
        .unwrap();
    assert_eq!(
        s1, s2,
        "collision <surface> is byte-identical across the URDF round-trip"
    );
}

/// A companion to the urdf-only allowance in `from_urdf_matches_python_oracle_on_corpus`: under
/// `--features urdf` WITHOUT `sdf`, the SDF-shared Gazebo mappers (`from_sdf::gazebo_block_extract`) are
/// cfg'd out, so a `<gazebo reference>` friction idiom is NOT decomposed into a typed `<surface>`, but it
/// is NOT lost either: it stays QUARANTINED VERBATIM in `org.gazebosim.raw` and round-trips byte-faithfully
/// through `to_urdf`. This proves the parity-harness exclusion of that domain is graceful feature
/// degradation, not data loss (i.e. the allowance is not masking a bug).
#[test]
#[cfg(all(feature = "urdf", not(feature = "sdf")))]
fn urdf_gazebo_friction_quarantined_verbatim_without_sdf() {
    let urdf = r#"<robot name="fr">
        <link name="wheel">
          <collision><geometry><cylinder radius="0.1" length="0.05"/></geometry></collision>
        </link>
        <gazebo reference="wheel"><mu1>0.8</mu1><mu2>0.9</mu2><kp>100000</kp><kd>10</kd></gazebo></robot>"#;
    let (doc, _notes) = from_urdf_str(urdf).expect("from_urdf");
    // No typed decomposition without the sdf mappers: the collision carries NO surface.
    let wheel = doc
        .comp
        .iter()
        .find(|c| c.name == "wheel")
        .expect("wheel comp");
    assert!(
        wheel.collision[0].surface.is_none(),
        "without the sdf mappers the friction idiom is NOT typed into a <surface>"
    );
    // ...but it is preserved verbatim in the org.gazebosim.raw quarantine (lossless).
    let gz = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim.raw")
        .expect("friction idiom kept in the org.gazebosim.raw verbatim quarantine");
    let leaves = [
        "<mu1>0.8</mu1>",
        "<mu2>0.9</mu2>",
        "<kp>100000</kp>",
        "<kd>10</kd>",
    ];
    for leaf in leaves {
        assert!(
            gz.body.contains(leaf),
            "friction leaf {leaf} quarantined verbatim: {}",
            gz.body
        );
    }
    // Byte-faithful URDF round-trip: to_urdf re-emits the verbatim quarantine unchanged.
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");
    for leaf in leaves {
        assert!(
            out.contains(leaf),
            "friction leaf {leaf} survives the URDF round-trip: {out}"
        );
    }
}

/// A URDF `<gazebo reference="link"><material>Gazebo/&lt;Color&gt;</material>` per-link
/// appearance override used to be quarantined verbatim and never applied. A recognized `Gazebo/<Color>`
/// preset now decodes to an rgba and lands on the referenced comp's ARM-B (primitive) visuals' inline
/// `<color>`; the consumed `<material>` leaves the residual. An UNRECOGNIZED name (no recoverable flat
/// rgba) stays quarantined verbatim. Requires the SDF mappers (`feature = "sdf"`).
#[test]
#[cfg(feature = "sdf")]
fn urdf_gazebo_material_maps_to_visual_color() {
    use hcdformat::model::VisualAppearance;
    let urdf = r#"<robot name="painted">
        <link name="hull">
          <visual><geometry><box size="1 1 1"/></geometry></visual>
        </link>
        <link name="mast">
          <visual><geometry><cylinder radius="0.05" length="2"/></geometry></visual>
        </link>
        <gazebo reference="hull"><material>Gazebo/Blue</material></gazebo>
        <gazebo reference="mast"><material>Gazebo/NotAPreset</material></gazebo>
    </robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("from_urdf");

    // (1) the recognized Gazebo/Blue preset lands on the hull visual's inline <color> as an rgba.
    let hull = doc
        .comp
        .iter()
        .find(|c| c.name == "hull")
        .expect("hull comp");
    let color = match &hull.visual[0].appearance {
        VisualAppearance::Primitive { color, .. } => color.as_ref().expect("inline color applied"),
        VisualAppearance::Model { .. } => panic!("primitive visual expected"),
    };
    assert_eq!(
        color.rgba.as_deref(),
        Some("0 0 1 1"),
        "Gazebo/Blue -> rgba"
    );
    // The consumed hull <material> is gone from the residual (or the whole block dropped from quarantine).
    if let Some(gz) = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim.raw")
    {
        assert!(
            !gz.body.contains("Gazebo/Blue"),
            "recognized <material> consumed, not re-quarantined: {}",
            gz.body
        );
    }
    assert!(
        notes
            .iter()
            .any(|n| n.contains("mapped <material>") && n.contains("hull")),
        "a hull material mapping note is recorded: {notes:?}"
    );

    // (2) the UNRECOGNIZED Gazebo/NotAPreset stays quarantined verbatim (no flat rgba recoverable) and
    // does NOT color the mast visual.
    let mast = doc
        .comp
        .iter()
        .find(|c| c.name == "mast")
        .expect("mast comp");
    match &mast.visual[0].appearance {
        VisualAppearance::Primitive { color, .. } => {
            assert!(color.is_none(), "an unrecognized preset applies no color")
        }
        VisualAppearance::Model { .. } => panic!("primitive visual expected"),
    }
    let gz = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim.raw")
        .expect("org.gazebosim.raw extension for the unrecognized <material>");
    assert!(
        gz.body.contains("Gazebo/NotAPreset"),
        "unrecognized <material> preserved verbatim: {}",
        gz.body
    );

    // The imported HCDF round-trips and is schema-valid.
    let xml = doc.to_xml_string().expect("serialize");
    let doc2 = Hcdf::from_xml_str(&xml).expect("reparse");
    let hull2 = doc2
        .comp
        .iter()
        .find(|c| c.name == "hull")
        .expect("hull comp (reparse)");
    match &hull2.visual[0].appearance {
        VisualAppearance::Primitive { color, .. } => assert_eq!(
            color.as_ref().and_then(|c| c.rgba.as_deref()),
            Some("0 0 1 1"),
            "the applied color survives the HCDF round-trip"
        ),
        VisualAppearance::Model { .. } => panic!("primitive visual expected"),
    }
    #[cfg(feature = "xsd")]
    assert!(
        hcdformat::validate_xsd(&xml).is_empty(),
        "gazebo-material HCDF must satisfy hcdf.xsd: {:?}",
        hcdformat::validate_xsd(&xml)
    );
}

// ── joint-type coverage: URDF import + HCDF->URDF export symmetry ─────────────────────────────────

/// urdf-rs parses `<joint type="spherical">` (its 3-DOF ball joint), but HCDF has no `spherical`
/// literal, so the importer must map it to `ball` rather than erroring the whole document.
#[test]
fn urdf_spherical_imports_as_ball() {
    let urdf = r#"<robot name="s"><link name="a"/><link name="b"/>
        <joint name="j" type="spherical"><parent link="a"/><child link="b"/></joint></robot>"#;
    let (doc, _notes) = from_urdf_str(urdf).expect("spherical must import (mapped to ball)");
    assert_eq!(
        doc.joint[0]
            .type_
            .as_ref()
            .map(|t| t.to_string())
            .as_deref(),
        Some("ball"),
        "URDF spherical must import as HCDF ball"
    );
}

/// Every other URDF joint type still maps as before (1:1, plus floating->free).
#[test]
fn urdf_all_joint_types_still_map() {
    for (utype, expected) in [
        ("revolute", "revolute"),
        ("continuous", "continuous"),
        ("prismatic", "prismatic"),
        ("fixed", "fixed"),
        ("planar", "planar"),
        ("floating", "free"),
        ("spherical", "ball"),
    ] {
        let urdf = format!(
            r#"<robot name="r"><link name="a"/><link name="b"/>
            <joint name="j" type="{utype}"><parent link="a"/><child link="b"/></joint></robot>"#
        );
        let (doc, _) =
            from_urdf_str(&urdf).unwrap_or_else(|e| panic!("{utype}: import failed: {e}"));
        assert_eq!(
            doc.joint[0]
                .type_
                .as_ref()
                .map(|t| t.to_string())
                .as_deref(),
            Some(expected),
            "URDF {utype} -> HCDF {expected}"
        );
    }
}

/// A URDF planar joint imports with its axis (the plane normal) + the single URDF `<limit>`
/// mapped to the FIRST in-plane range (`<limit>`), and a WARNING that the SECOND in-plane DOF
/// (`<limit2>`) is left UNBOUNDED, never fabricated. It round-trips back out to a URDF `<planar>` with
/// the single limit re-emitted.
#[test]
fn urdf_planar_imports_first_range_and_warns_second_unbounded() {
    let urdf = r#"<robot name="r"><link name="a"/><link name="b"/>
      <joint name="j" type="planar"><parent link="a"/><child link="b"/>
        <axis xyz="0 0 1"/>
        <limit lower="-1.0" upper="1.0" effort="10" velocity="1"/></joint></robot>"#;
    let (doc, notes) = from_urdf_str(urdf).expect("planar imports");
    let j = &doc.joint[0];
    assert_eq!(j.type_.map(|t| t.to_string()).as_deref(), Some("planar"));
    // axis = plane normal, single limit -> first in-plane range.
    assert_eq!(
        j.axis.as_ref().and_then(|a| a.xyz.as_deref()),
        Some("0 0 1")
    );
    let lim = j.limit.as_ref().expect("first in-plane range imported");
    assert_eq!(lim.lower.as_deref(), Some("-1.0"));
    assert_eq!(lim.upper.as_deref(), Some("1.0"));
    // The second in-plane DOF is NOT fabricated.
    assert!(
        j.limit2.is_none(),
        "the second in-plane range must not be fabricated"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("second in-plane DOF") && n.contains("UNBOUNDED")),
        "planar under-import warning expected; got {notes:?}"
    );
    // Round-trip back to URDF: type + the single (box-1) limit re-emitted.
    let (out, _loss) = to_urdf(&doc).expect("to_urdf");
    assert!(
        out.contains(r#"type="planar""#),
        "planar type must round-trip:\n{out}"
    );
    assert!(
        out.contains(r#"lower="-1.0""#),
        "first in-plane range must round-trip:\n{out}"
    );
}

/// Each of the 10 HCDF joint types exports to URDF as its faithful literal, or downgrades to `fixed`
/// with an honest joint-type loss note, never a panic and never an invalid/silent bad type.
#[test]
fn hcdf_all_joint_types_export_to_urdf() {
    // (HCDF type, expected URDF @type, downgraded-with-loss?)
    let cases = [
        ("revolute", "revolute", false),
        ("continuous", "continuous", false),
        ("prismatic", "prismatic", false),
        ("fixed", "fixed", false),
        ("ball", "fixed", true),
        ("universal", "fixed", true),
        ("planar", "planar", false),
        ("screw", "fixed", true),
        ("cylindrical", "fixed", true),
        ("free", "floating", false),
    ];
    for (hcdf_ty, urdf_ty, downgraded) in cases {
        // A rich joint (axis + axis2 + limit + thread_pitch) exercises every drop-note path without
        // relying on XSD validity (from_xml_str is a lenient deserializer).
        let hcdf = format!(
            r#"<hcdf name="jt" version="1.0"><comp name="a"/><comp name="b"/>
            <joint name="j" type="{hcdf_ty}" thread_pitch="0.01">
              <parent comp="a"/><child comp="b"/>
              <axis xyz="0 0 1"/><axis2 xyz="0 1 0"/>
              <limit lower="-1" upper="1" effort="1" velocity="1"/></joint></hcdf>"#
        );
        let doc =
            Hcdf::from_xml_str(&hcdf).unwrap_or_else(|e| panic!("{hcdf_ty}: parse failed: {e}"));
        let (urdf, loss) =
            to_urdf(&doc).unwrap_or_else(|e| panic!("{hcdf_ty}: to_urdf failed: {e}"));
        assert!(
            urdf.contains(&format!(r#"<joint name="j" type="{urdf_ty}""#)),
            "HCDF {hcdf_ty} must export URDF type {urdf_ty}; got:\n{urdf}"
        );
        let has_type_loss = loss
            .text()
            .lines()
            .any(|l| l.contains("joint-type") && l.contains("no URDF equivalent"));
        assert_eq!(
            has_type_loss, downgraded,
            "HCDF {hcdf_ty}: joint-type loss note presence must match the downgrade"
        );
    }
}

#[test]
fn tree_is_counted_by_profile_and_urdf_loss_manifest() {
    let doc = Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><tree name="n"><root><hop-ref network="n" hop="h"/></root><hop name="h"><owner><component-ref component="ghost"/></owner></hop></tree></hcdf>"#,
    )
    .unwrap();

    let report = check_profile(&doc);
    let finding = report
        .findings
        .iter()
        .find(|value| value.code == "P_NETWORK")
        .expect("Tree must make the profile out of profile");
    assert_eq!(
        finding.detail,
        "document: 1 networks (HCDF-only; no URDF home)"
    );

    let (_, loss) = to_urdf(&doc).unwrap();
    assert!(loss.items.iter().any(|(category, detail)| {
        category == "top-level" && detail == "document: 1 networks dropped (no URDF equivalent)"
    }));
}
