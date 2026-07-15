//! Conformance tests for `<include>` flattening (the Rust port of `hcdf_io/include.py`).
//!
//! Mirrors the motor scenario: a reusable module (a `rotor` comp with a mesh visual + a joint) is
//! included TWICE with `name=left`/`name=right` and distinct poses, plus a nested include and a cycle.
//! Asserts definitions get prefixed, references get rewritten against the ORIGINAL name sets, the pose
//! offset lands on root geometry/joint origins (hand-derived), relative mesh uris are rerooted to
//! absolute under the module dir while `package://`-style uris are left alone, a cycle is an `Err`, and
//! an unresolvable include is left in place.
//!
//! These exercise the flatten ALGORITHM on synthetic in-memory modules (prefixing, ref rewriting, pose
//! math); there is no corpus `<include>` graph that resolves on disk to diff against a Python golden
//! (the one corpus include, in `test-minimal`, points at an absent module and is intentionally left
//! unresolved). Corpus-level round-trip + validation parity against the live Python oracle is in
//! `parity.rs`; this file keeps the resolver's unit-level coverage that the oracle harness cannot reach.
//!
//! The in-memory tests compile on every target; the tests of the NATIVE fs conveniences
//! (`flatten_path`, `flatten`, `stamp_include_shas`, the native-only exports) are cfg-gated off wasm32,
//! mirroring the lib's gating, so the wasm `--all-targets` clippy gate still compiles this file.
use hcdformat::model::{connectivity_xml, Comp, JointEndpoint, Mesh, VisualAppearance};
use hcdformat::{
    validate_coverage, validate_loops, validate_network, validate_semantic, Hcdf, Issue, Level,
};
use std::collections::HashMap;
use std::path::Path;

/// A module: comp `rotor` (mesh visual `rotor.glb` + a `package://` collision mesh + an inline color
/// ref) and a joint `spin` whose parent is `hub` (NOT defined here, so it stays unprefixed) and child is
/// `rotor` (defined here, so it gets prefixed). The `spin` joint has an origin pose to offset-test.
fn module_xml() -> &'static str {
    r#"<hcdf name="rotor-module" version="1.0">
         <color name="blade"/>
         <comp name="rotor">
           <visual name="v">
             <pose xyz="0 0 0"/>
             <model uri="meshes/rotor.glb"/>
           </visual>
           <collision name="c">
             <geometry><mesh uri="package://things/rotor_col.stl"/></geometry>
           </collision>
           <frame name="tip"><pose xyz="0 0 1"/></frame>
         </comp>
         <comp name="hub_local"><description>a second root</description></comp>
         <joint name="spin" type="continuous">
           <parent comp="hub"/>
           <child comp="rotor"/>
           <origin xyz="1 0 0"/>
           <mimic joint="spin"/>
         </joint>
       </hcdf>"#
}

/// A parent that nests the module via a relative uri (to exercise the nested-first recursion + the
/// per-file base-relative resolution).
fn nested_parent_xml() -> &'static str {
    r#"<hcdf name="nested" version="1.0">
         <include uri="module.hcdf" name="inner"/>
       </hcdf>"#
}

/// Build an in-memory loader over a name->xml table; the resolved KEY (an absolute path) is matched by
/// its file name so the test does not depend on the process cwd. Returns `(parsed sub-doc, source_sha)`,
/// where the sha is the `content_sha` of the module's XML bytes, so an in-memory loader can ALSO drive the
/// `@sha` verification path (mirroring what the native `fs_loader` reports for an on-disk module).
fn mem_loader(
    files: HashMap<&'static str, &'static str>,
) -> impl FnMut(&str, &Path) -> Result<(Hcdf, Option<String>), String> {
    move |key: &str, _base: &Path| {
        let name = Path::new(key)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(key);
        match files.get(name) {
            Some(xml) => Hcdf::from_xml_str(xml)
                .map(|doc| (doc, Some(hcdformat::content_sha(xml.as_bytes()))))
                .map_err(|e| e.to_string()),
            None => Err(format!("file not found at {key:?}")),
        }
    }
}

fn find_comp<'a>(doc: &'a Hcdf, name: &str) -> Option<&'a Comp> {
    doc.comp.iter().find(|c| c.name == name)
}

fn participant_port_ref(participant: &connectivity_xml::Participant) -> &connectivity_xml::PortRef {
    match &participant.endpoint.endpoint {
        connectivity_xml::FunctionalEndpointChoice::Port(reference) => reference,
        other => panic!("expected port endpoint, got {other:?}"),
    }
}

fn connector_representation<'a>(
    doc: &'a Hcdf,
    component: &str,
    connector: &str,
) -> &'a connectivity_xml::Representation {
    find_comp(doc, component)
        .unwrap_or_else(|| panic!("missing component {component:?}"))
        .connector
        .iter()
        .find(|value| value.name == connector)
        .unwrap_or_else(|| panic!("missing connector {component:?}/{connector:?}"))
        .representation
        .as_ref()
        .unwrap_or_else(|| panic!("missing representation {component:?}/{connector:?}"))
}

fn sphere_placement<'a>(
    doc: &'a Hcdf,
    component: &str,
    connector: &str,
) -> &'a connectivity_xml::Placement {
    match &connector_representation(doc, component, connector).variant {
        connectivity_xml::RepresentationChoice::Sphere(value) => &value.placement,
        other => panic!("expected sphere representation, got {other:?}"),
    }
}

fn assert_vec3_close(actual: [f64; 3], expected: [f64; 3]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }
}

#[test]
fn module_included_twice_prefixes_and_offsets() {
    let mut doc: Hcdf = Hcdf::from_xml_str(
        r#"<hcdf name="airframe" version="1.0">
             <comp name="hub"/>
             <include uri="module.hcdf" name="left" pose="0 0 0 0 0 1.5707963267948966"/>
             <include uri="module.hcdf" name="right" pose="2 0 0"/>
           </hcdf>"#,
    )
    .unwrap();

    let files = HashMap::from([("module.hcdf", module_xml())]);
    let mut load = mem_loader(files);
    let notes = hcdformat::flatten_with(&mut doc, Path::new("/abs/base"), &mut load).unwrap();

    // includes consumed
    assert!(doc.include.is_empty(), "all includes resolved");
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.starts_with("flattened include"))
            .count(),
        2
    );

    // definitions prefixed (left/rotor, right/rotor, left/hub_local, right/hub_local)
    assert!(
        find_comp(&doc, "left/rotor").is_some(),
        "left/rotor present"
    );
    assert!(
        find_comp(&doc, "right/rotor").is_some(),
        "right/rotor present"
    );
    assert!(
        find_comp(&doc, "hub").is_some(),
        "the parent hub is untouched"
    );

    // joint refs: child 'rotor' (defined in module) -> prefixed; parent 'hub' (NOT in module) -> stays.
    let left_spin = doc
        .joint
        .iter()
        .find(|j| j.name.as_deref() == Some("left/spin"))
        .unwrap();
    assert_eq!(
        left_spin.child.as_ref().unwrap().comp.as_deref(),
        Some("left/rotor")
    );
    assert_eq!(
        left_spin.parent.as_ref().unwrap().comp.as_deref(),
        Some("hub"),
        "a ref to a NON-included name stays unprefixed"
    );
    // mimic.joint 'spin' is a module joint -> prefixed
    assert_eq!(
        left_spin.mimic.as_ref().unwrap().joint.as_deref(),
        Some("left/spin")
    );

    // color def prefixed
    assert!(doc
        .color
        .iter()
        .any(|c| c.name.as_deref() == Some("left/blade")));

    // POSE OFFSET on the LEFT module = yaw +90deg about world origin.
    // rotor is a non-loop joint child -> NOT a root -> its own geometry is NOT offset.
    // hub_local IS a root (no joint makes it a child) -> but it has no geometry. The joint 'spin'
    // leaves root 'hub' which is NOT in the module's root set (hub is the parent doc's comp, not in
    // the sub), so spin.origin is offset ONLY if its parent is a MODULE root. Its parent is 'hub'
    // (unprefixed, not a module comp) so spin.origin is NOT offset. Assert that holds:
    let left_origin = left_spin.origin.clone().unwrap_or_default();
    assert_eq!(
        left_origin.xyz_or_zero(),
        [1.0, 0.0, 0.0],
        "spin parent 'hub' is not a module root; origin unchanged"
    );

    // The rotor comp is a child of spin -> not a root -> its visual pose (0,0,0) is unchanged.
    let left_rotor = find_comp(&doc, "left/rotor").unwrap();
    let vpose = left_rotor.visual[0].pose.clone().unwrap_or_default();
    assert_eq!(
        vpose.xyz_or_zero(),
        [0.0, 0.0, 0.0],
        "rotor is not a root; geometry not offset"
    );

    // hub_local IS a module root; on the RIGHT module (pose '2 0 0') its frame... it has none, but its
    // visual/collision/frame lists are empty, so nothing to check there. Instead verify the offset on a
    // ROOT by checking the LEFT module's hub_local picks up no geometry but stays a comp:
    assert!(find_comp(&doc, "left/hub_local").is_some());
}

#[test]
fn root_comp_geometry_is_offset_hand_derived() {
    // A standalone root comp with a visual at the origin, offset by a pure +90deg yaw + translate.
    // Root = a comp that is not a non-loop joint child. With no joints, the single comp is a root.
    let module = r#"<hcdf name="m" version="1.0">
                      <comp name="thing">
                        <visual name="v"><pose xyz="1 0 0"/></visual>
                      </comp>
                    </hcdf>"#;
    let mut doc: Hcdf = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0">
             <include uri="m.hcdf" name="a" pose="0 0 0 0 0 1.5707963267948966"/>
           </hcdf>"#,
    )
    .unwrap();
    let files = HashMap::from([("m.hcdf", module)]);
    let mut load = mem_loader(files);
    hcdformat::flatten_with(&mut doc, Path::new("/b"), &mut load).unwrap();

    // visual.pose xyz (1,0,0) rotated by Rz(90) -> (0,1,0); translation 0. Hand-derived.
    let c = find_comp(&doc, "a/thing").unwrap();
    let v = c.visual[0].pose.clone().unwrap();
    let vxyz = v.xyz_or_zero();
    let vrpy = v.rpy_or_zero();
    assert!((vxyz[0]).abs() < 1e-9, "x ~ 0, got {}", vxyz[0]);
    assert!((vxyz[1] - 1.0).abs() < 1e-9, "y ~ 1, got {}", vxyz[1]);
    assert!((vxyz[2]).abs() < 1e-9);
    // rotation: yaw 90; matrix_to_pose emits rpy and clears quat.
    assert!(
        (vrpy[2] - std::f64::consts::FRAC_PI_2).abs() < 1e-9,
        "yaw ~ pi/2, got {}",
        vrpy[2]
    );
    assert!(v.quat.is_none(), "offset emits rpy, clears quat");
}

#[test]
fn relative_mesh_uris_rerooted_scheme_left_alone() {
    let mut doc: Hcdf =
        Hcdf::from_xml_str(r#"<hcdf name="p" version="1.0"><include uri="module.hcdf"/></hcdf>"#)
            .unwrap();
    let files = HashMap::from([("module.hcdf", module_xml())]);
    // Use a loader that puts the module under a KNOWN absolute directory so we can assert the reroot.
    let mut load = move |key: &str, _base: &Path| {
        let name = Path::new(key).file_name().and_then(|n| n.to_str()).unwrap();
        assert_eq!(name, "module.hcdf");
        files
            .get(name)
            .map(|xml| (Hcdf::from_xml_str(xml).unwrap(), None))
            .ok_or_else(|| "missing".to_string())
    };
    // base_dir picks the module's resolved dir; reroot anchors to THAT dir.
    hcdformat::flatten_with(&mut doc, Path::new("/module/dir"), &mut load).unwrap();

    let rotor = find_comp(&doc, "rotor").unwrap();
    // visual model mesh "meshes/rotor.glb" -> absolute under /module/dir
    let VisualAppearance::Model { model, .. } = &rotor.visual[0].appearance else {
        panic!("expected ARM A model visual");
    };
    let uri = model.uri.as_deref().unwrap();
    assert!(uri.starts_with('/'), "rerooted to absolute, got {uri}");
    assert!(
        uri.ends_with("/module/dir/meshes/rotor.glb"),
        "anchored under module dir, got {uri}"
    );

    // collision mesh is package:// -> UNCHANGED
    let col_mesh: &Mesh = rotor.collision[0]
        .geometry
        .as_ref()
        .unwrap()
        .mesh
        .as_ref()
        .unwrap();
    assert_eq!(
        col_mesh.uri.as_deref(),
        Some("package://things/rotor_col.stl")
    );
}

#[test]
fn asset_reroot_preserves_schemes_and_suffix_bytes() {
    let module = r#"<hcdf name="assets" version="1.0">
      <comp name="device">
        <visual name="urn"><model uri="urn:asset:visual"/></visual>
        <visual name="model"><model uri="model://robot/body.glb"/></visual>
        <visual name="https"><model uri="https://example.com/body.glb"/></visual>
        <collision name="file"><geometry><mesh uri="file:/mem/collision.stl"/></geometry></collision>
        <collision name="absolute"><geometry><mesh uri="/absolute/collision.stl"/></geometry></collision>
        <sensor name="custom"><em type="mag"><geometry><mesh uri="custom+v1:sensor"/></geometry></em></sensor>
        <hmi name="data"><geometry><mesh uri="data:model/gltf-binary;base64,AAAA"/></geometry></hmi>
        <connector name="suffix"><representation><model uri="assets/connector.glb?next=/../x#part/../y"><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/></rotation></placement></model></representation></connector>
        <connector name="package"><representation><model uri="package://robot/connector.glb"><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/></rotation></placement></model></representation></connector>
      </comp>
    </hcdf>"#;
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="root" version="1.0"><include uri="module.hcdf"/></hcdf>"#,
    )
    .unwrap();
    let mut load = mem_loader(HashMap::from([("module.hcdf", module)]));
    hcdformat::flatten_with(&mut doc, Path::new("/module/dir"), &mut load).unwrap();

    let component = find_comp(&doc, "device").unwrap();
    let visual_uri = |index: usize| {
        let VisualAppearance::Model { model, .. } = &component.visual[index].appearance else {
            panic!("expected model-backed visual");
        };
        model.uri.as_deref().unwrap()
    };
    assert_eq!(visual_uri(0), "urn:asset:visual");
    assert_eq!(visual_uri(1), "model://robot/body.glb");
    assert_eq!(visual_uri(2), "https://example.com/body.glb");
    assert_eq!(
        component.collision[0]
            .geometry
            .as_ref()
            .unwrap()
            .mesh
            .as_ref()
            .unwrap()
            .uri
            .as_deref(),
        Some("file:/mem/collision.stl")
    );
    assert_eq!(
        component.collision[1]
            .geometry
            .as_ref()
            .unwrap()
            .mesh
            .as_ref()
            .unwrap()
            .uri
            .as_deref(),
        Some("/absolute/collision.stl")
    );
    assert_eq!(
        component.sensor[0].em[0]
            .geometry
            .as_ref()
            .unwrap()
            .mesh
            .as_ref()
            .unwrap()
            .uri
            .as_deref(),
        Some("custom+v1:sensor")
    );
    assert_eq!(
        component.hmi[0]
            .geometry
            .as_ref()
            .unwrap()
            .mesh
            .as_ref()
            .unwrap()
            .uri
            .as_deref(),
        Some("data:model/gltf-binary;base64,AAAA")
    );
    let representation_uri = |index: usize| {
        let connectivity_xml::RepresentationChoice::Model(model) = &component.connector[index]
            .representation
            .as_ref()
            .unwrap()
            .variant
        else {
            panic!("expected model-backed connector representation");
        };
        model.uri.as_str()
    };
    assert_eq!(
        representation_uri(0),
        "/module/dir/assets/connector.glb?next=/../x#part/../y"
    );
    assert_eq!(representation_uri(1), "package://robot/connector.glb");
}

#[test]
fn nested_include_resolves() {
    let mut doc: Hcdf = Hcdf::from_xml_str(
        r#"<hcdf name="top" version="1.0"><include uri="nested.hcdf" name="outer"/></hcdf>"#,
    )
    .unwrap();
    let files = HashMap::from([
        ("nested.hcdf", nested_parent_xml()),
        ("module.hcdf", module_xml()),
    ]);
    let mut load = mem_loader(files);
    hcdformat::flatten_with(&mut doc, Path::new("/x"), &mut load).unwrap();

    // double prefix: nested.hcdf's <include name="inner"> then top's <include name="outer"> ->
    // "outer/inner/rotor". (Nested resolves first, applying "inner/", then "outer/" wraps the merged.)
    assert!(
        find_comp(&doc, "outer/inner/rotor").is_some(),
        "comps: {:?}",
        doc.comp.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}

#[test]
fn nested_include_offsets_only_world_framed_connectivity_geometry() {
    let leaf = r#"<hcdf name="leaf" version="1.0">
      <comp name="device">
        <frame name="mount"><pose xyz="0 0 0"/></frame>
        <connector name="world"><representation>
          <sphere radius="1"><placement xyz="1 0 0"><frame><world/></frame><rotation><quaternion value="0 0 0 1"/></rotation></placement></sphere>
        </representation></connector>
        <connector name="local"><representation>
          <sphere radius="1"><placement xyz="5 0 0"><frame><component-frame component="device" frame="mount"/></frame><rotation><rpy value="0 0 0"/></rotation></placement></sphere>
        </representation></connector>
        <connector name="route"><representation>
          <derived-route>
            <rectangular-section width="0.02" height="0.01"/>
            <waypoint xyz="0 0 0"><frame><world/></frame><rotation><quaternion value="0 0.7071067811865476 0 0.7071067811865476"/></rotation></waypoint>
            <waypoint xyz="2 0 0"><frame><world/></frame><rotation><rpy value="0 1.5707963267948966 0"/></rotation></waypoint>
            <waypoint xyz="7 0 0"><frame><component-origin component="device"/></frame><rotation><rpy value="0 1.5707963267948966 0"/></rotation></waypoint>
            <waypoint xyz="9 0 0"><frame><component-origin component="child"/></frame><rotation><rpy value="0 1.5707963267948966 0"/></rotation></waypoint>
          </derived-route>
        </representation></connector>
      </comp>
      <comp name="child"/>
      <joint name="child-joint"><parent comp="device"/><child comp="child"/></joint>
    </hcdf>"#;
    let nested = r#"<hcdf name="nested" version="1.0">
      <include uri="leaf.hcdf" name="inner" pose="1 0 0 0 0 1.5707963267948966"/>
    </hcdf>"#;
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="top" version="1.0">
          <include uri="nested.hcdf" name="outer" pose="0 2 0 0 0 1.5707963267948966"/>
        </hcdf>"#,
    )
    .unwrap();
    let mut load = mem_loader(HashMap::from([
        ("leaf.hcdf", leaf),
        ("nested.hcdf", nested),
    ]));
    hcdformat::flatten_with(&mut doc, Path::new("/offset"), &mut load).unwrap();

    let component = find_comp(&doc, "outer/inner/device").unwrap();
    let representation = |name: &str| {
        component
            .connector
            .iter()
            .find(|connector| connector.name == name)
            .and_then(|connector| connector.representation.as_ref())
            .unwrap()
    };
    let close = |actual: [f64; 3], expected: [f64; 3]| {
        for index in 0..3 {
            assert!(
                (actual[index] - expected[index]).abs() < 1e-9,
                "coordinate {index}: expected {}, got {}",
                expected[index],
                actual[index]
            );
        }
    };
    let local_z = |waypoint: &hcdformat::model::RoutePoint| {
        let rotation = waypoint.rotation.as_ref().expect("route rotation");
        match &rotation.rotation {
            hcdformat::model::PlacementRotationChoice::Rpy(rotation) => {
                let [roll, pitch, yaw] = rotation.value;
                let (sr, cr) = roll.sin_cos();
                let (sp, cp) = pitch.sin_cos();
                let (sy, cy) = yaw.sin_cos();
                [cy * sp * cr + sy * sr, sy * sp * cr - cy * sr, cp * cr]
            }
            hcdformat::model::PlacementRotationChoice::Quaternion(rotation) => {
                let [x, y, z, w] = rotation.value;
                [
                    2.0 * (x * z + w * y),
                    2.0 * (y * z - w * x),
                    1.0 - 2.0 * (x * x + y * y),
                ]
            }
        }
    };

    let hcdformat::model::RepresentationChoice::Sphere(world) = &representation("world").variant
    else {
        panic!("expected world sphere");
    };
    close(world.placement.xyz, [-1.0, 3.0, 0.0]);
    let hcdformat::model::PlacementRotationChoice::Rpy(rotation) =
        &world.placement.rotation.rotation
    else {
        panic!("include offset must emit exactly one rpy rotation payload");
    };
    assert!((rotation.value[2].abs() - std::f64::consts::PI).abs() < 1e-9);

    let hcdformat::model::RepresentationChoice::Sphere(local) = &representation("local").variant
    else {
        panic!("expected component-frame sphere");
    };
    close(local.placement.xyz, [5.0, 0.0, 0.0]);
    let hcdformat::model::RouteFrameChoice::ComponentFrame(frame) = &local.placement.frame.frame
    else {
        panic!("expected component-frame payload");
    };
    assert_eq!(frame.component, "outer/inner/device");
    assert_eq!(frame.frame, "mount");

    let hcdformat::model::RepresentationChoice::DerivedRoute(route) =
        &representation("route").variant
    else {
        panic!("expected derived route");
    };
    close(route.waypoint[0].xyz, [0.0, 3.0, 0.0]);
    close(route.waypoint[1].xyz, [-2.0, 3.0, 0.0]);
    close(route.waypoint[2].xyz, [-7.0, 3.0, 0.0]);
    close(route.waypoint[3].xyz, [9.0, 0.0, 0.0]);
    close(local_z(&route.waypoint[0]), [-1.0, 0.0, 0.0]);
    close(local_z(&route.waypoint[1]), [-1.0, 0.0, 0.0]);
    close(local_z(&route.waypoint[2]), [-1.0, 0.0, 0.0]);
    close(local_z(&route.waypoint[3]), [1.0, 0.0, 0.0]);
    let hcdformat::model::RouteFrameChoice::ComponentOrigin(frame) = &route.waypoint[2].frame.frame
    else {
        panic!("expected component-origin payload");
    };
    assert_eq!(frame.component, "outer/inner/device");
    let hcdformat::model::RouteFrameChoice::ComponentOrigin(frame) = &route.waypoint[3].frame.frame
    else {
        panic!("expected non-root component-origin payload");
    };
    assert_eq!(frame.component, "outer/inner/child");
}

#[test]
fn cycle_is_err() {
    // a.hcdf includes b.hcdf includes a.hcdf -> cycle on re-entering a's resolved key.
    let a = r#"<hcdf name="a" version="1.0"><include uri="b.hcdf"/></hcdf>"#;
    let b = r#"<hcdf name="b" version="1.0"><include uri="a.hcdf"/></hcdf>"#;
    let mut doc: Hcdf =
        Hcdf::from_xml_str(r#"<hcdf name="root" version="1.0"><include uri="a.hcdf"/></hcdf>"#)
            .unwrap();
    let original = doc.clone();
    let files = HashMap::from([("a.hcdf", a), ("b.hcdf", b)]);
    let mut load = mem_loader(files);
    let err = hcdformat::flatten_with(&mut doc, Path::new("/cyc"), &mut load).unwrap_err();
    assert!(err.contains("cycle"), "expected cycle error, got {err}");
    assert_eq!(doc, original, "nested failure must not mutate its input");
}

#[test]
fn unresolvable_include_left_in_place() {
    let mut doc: Hcdf = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0"><include uri="missing.hcdf" name="m"/></hcdf>"#,
    )
    .unwrap();
    let files: HashMap<&'static str, &'static str> = HashMap::new();
    let mut load = mem_loader(files);
    let notes = hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap();
    assert_eq!(doc.include.len(), 1, "unresolvable include kept");
    assert_eq!(doc.include[0].uri.as_deref(), Some("missing.hcdf"));
    assert!(
        notes.iter().any(|n| n.contains("left unresolved")),
        "notes: {notes:?}"
    );
}

#[test]
fn duplicate_sibling_include_names_are_rejected_before_flattening() {
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0">
             <include uri="first.hcdf" name="module"/>
             <include uri="second.hcdf" name="module"/>
           </hcdf>"#,
    )
    .unwrap();
    let mut load = mem_loader(HashMap::new());
    let error = hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap_err();
    assert!(error.contains("duplicate sibling include name \"module\""));
    assert_eq!(doc.include.len(), 2, "validation must be transactional");
}

#[test]
fn uri_less_include_is_an_error_and_is_not_dropped() {
    let mut doc =
        Hcdf::from_xml_str(r#"<hcdf name="p" version="1.0"><include name="module"/></hcdf>"#)
            .unwrap();
    let original = doc.clone();
    let mut load = mem_loader(HashMap::new());
    let error = hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap_err();
    assert!(error.contains("missing required @uri"));
    assert_eq!(doc, original, "failed flatten must not mutate its input");
}

#[test]
fn nonzero_instance_occurrence_is_rejected_transactionally() {
    let module = r#"<hcdf name="m" version="1.0">
      <comp name="device"><port name="p"/></comp>
      <link name="inside">
        <selected purpose="communication" carrier="electrical"/>
        <participant name="first"><endpoint><port-ref component="device" port="p">
          <instance><segment name="child" occurrence="1"/></instance>
        </port-ref></endpoint></participant>
        <participant name="second"><endpoint><port-ref component="device" port="p"/></endpoint></participant>
      </link>
    </hcdf>"#;
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0"><include uri="m.hcdf" name="outer"/></hcdf>"#,
    )
    .unwrap();
    let original = doc.clone();
    let mut load = mem_loader(HashMap::from([("m.hcdf", module)]));
    let error = hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap_err();

    assert!(error.contains("unsupported occurrence 1"), "{error}");
    assert_eq!(doc, original, "failed flatten must not mutate its input");
}

#[test]
fn empty_explicit_instance_is_preserved_for_conversion_to_reject() {
    let module = r#"<hcdf name="m" version="1.0">
      <comp name="device"><port name="p"/></comp>
      <link name="inside">
        <selected purpose="communication" carrier="electrical"/>
        <participant name="first"><endpoint><port-ref component="device" port="p"><instance/></port-ref></endpoint></participant>
        <participant name="second"><endpoint><port-ref component="device" port="p"/></endpoint></participant>
      </link>
    </hcdf>"#;
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0"><include uri="m.hcdf" name="outer"/></hcdf>"#,
    )
    .unwrap();
    let mut load = mem_loader(HashMap::from([("m.hcdf", module)]));
    hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap();

    let participant = &doc.link[0].participant[0];
    assert_eq!(participant_port_ref(participant).component, "device");
    assert!(participant_port_ref(participant)
        .instance
        .as_ref()
        .unwrap()
        .segment
        .is_empty());
}

#[test]
fn included_connectivity_comments_rebase_to_their_prefixed_objects() {
    let module = r#"<!-- module head --><hcdf name="m" version="1.0">
      <harness name="h"><!-- harness comment --></harness>
      <binding name="b" fidelity="presented">
        <functional><port-ref component="a" port="p"/></functional>
        <!-- binding comment -->
        <physical><connector-ref connector="J"><component-ref component="a"/></connector-ref></physical>
      </binding>
      <mate name="m" fidelity="presented">
        <first connector="J"><component-ref component="a"/></first>
        <!-- mate comment -->
        <second connector="J"><component-ref component="b"/></second>
      </mate>
      <bus name="n">
        <!-- network comment -->
        <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
        <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
      </bus>
    </hcdf><!-- module tail -->"#;
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0">
          <harness name="existing"/>
          <binding name="existing" fidelity="presented">
            <functional><port-ref component="a" port="p"/></functional>
            <physical><connector-ref connector="J"><component-ref component="a"/></connector-ref></physical>
          </binding>
          <mate name="existing" fidelity="presented">
            <first connector="J"><component-ref component="a"/></first>
            <second connector="J"><component-ref component="b"/></second>
          </mate>
          <bus name="existing">
            <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
          </bus>
          <include uri="m.hcdf" name="outer"/>
        </hcdf>"#,
    )
    .unwrap();
    let mut load = mem_loader(HashMap::from([("m.hcdf", module)]));
    hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap();
    let output = doc.to_xml_string().unwrap();

    for (tag, name, text) in [
        ("harness", "outer/h", "harness comment"),
        ("binding", "outer/b", "binding comment"),
        ("mate", "outer/m", "mate comment"),
        ("bus", "outer/n", "network comment"),
    ] {
        let start = output
            .find(&format!("<{tag} name=\"{name}\""))
            .unwrap_or_else(|| panic!("missing {tag} {name}: {output}"));
        let end = output[start..]
            .find(&format!("</{tag}>"))
            .map(|offset| start + offset)
            .unwrap();
        let comment = output.find(text).unwrap();
        assert!(
            start < comment && comment < end,
            "{text:?} was not rebased into {tag} {name:?}: {output}"
        );
    }
    assert!(output.contains("module head"));
    assert!(output.contains("module tail"));
}

#[test]
fn structured_topologies_prefix_across_repeated_includes_and_keep_tree_comments() {
    let module = r#"<hcdf name="m" version="1.0">
          <comp name="a"><port name="in"/><port name="out"/><switch name="bridge"><input><port-ref component="a" port="in"/></input><output><port-ref component="a" port="out"/></output></switch></comp>
          <comp name="b"><port name="p"/></comp>
          <star name="hub">
            <selected purpose="communication" carrier="electrical"/>
            <configuration>
              <gptp-domain name="default" number="0">
                <clock name="a-clock" kind="ordinary" gm-capable="true"><participant-ref network="hub" participant="a"/></clock>
                <clock name="b-clock" kind="ordinary" gm-capable="false"><participant-ref network="hub" participant="b"/></clock>
                <port-defaults log-sync-interval="-3" neighbor-prop-delay-threshold-ns="800"/>
              </gptp-domain>
              <traffic-class name="control" number="7" preemption="express"><pcp value="6"/></traffic-class>
              <gate-schedule name="main" cycle-time-ns="100">
                <gate duration-ns="100"><open><traffic-class-ref network="hub" traffic-class="control"/></open></gate>
              </gate-schedule>
              <schedule-assignment name="main-ports"><schedule-ref network="hub" schedule="main"/><target><participant-ref network="hub" participant="a"/></target><target><participant-ref network="hub" participant="b"/></target></schedule-assignment>
            </configuration>
            <coordinator><participant-ref network="hub" participant="a"/></coordinator>
            <participant name="a"><endpoint><port-ref component="a" port="out"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
          </star>
          <tree name="route">
            <selected purpose="communication" carrier="electrical"/>
            <!-- tree topology comment -->
            <root><hop-ref network="route" hop="h0"/></root>
            <participant name="a"><endpoint><port-ref component="a" port="out"/></endpoint></participant>
            <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
            <hop name="h0"><owner><function-ref component="a" function="bridge"/></owner></hop>
            <hop name="h1"><owner><component-ref component="b"/></owner></hop>
            <leg name="forward"><from><hop-ref network="route" hop="h0"/><participant-ref network="route" participant="a"/></from><to><hop-ref network="route" hop="h1"/><participant-ref network="route" participant="b"/></to></leg>
          </tree>
        </hcdf>"#;
    let mut doc = Hcdf::from_xml_str(
            r#"<hcdf name="p" version="1.0"><include uri="m.hcdf" name="left"/><include uri="m.hcdf" name="right"/></hcdf>"#,
        )
        .unwrap();
    let mut load = mem_loader(HashMap::from([("m.hcdf", module)]));
    hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap();

    assert!(doc.include.is_empty());
    let output = doc.to_xml_string().unwrap();
    assert_eq!(output.matches("tree topology comment").count(), 2);
    for prefix in ["left", "right"] {
        let tree_name = format!("{prefix}/route");
        let tree = doc
            .tree
            .iter()
            .find(|value| value.name == tree_name)
            .unwrap();
        assert_eq!(tree.root.hop.network, tree_name);
        assert!(tree.root.hop.instance.is_none());
        assert_eq!(
            participant_port_ref(&tree.participant[0]).component,
            format!("{prefix}/a")
        );
        assert_eq!(
            participant_port_ref(&tree.participant[1]).component,
            format!("{prefix}/b")
        );
        match &tree.hop[0].owner.owner {
            connectivity_xml::HopOwnerChoice::Function(reference) => {
                assert_eq!(reference.component, format!("{prefix}/a"));
                assert_eq!(reference.function, "bridge");
                assert!(reference.instance.is_none());
            }
            other => panic!("expected function owner, got {other:?}"),
        }
        match &tree.hop[1].owner.owner {
            connectivity_xml::HopOwnerChoice::Component(reference) => {
                assert_eq!(reference.component, format!("{prefix}/b"));
                assert!(reference.instance.is_none());
            }
            other => panic!("expected component owner, got {other:?}"),
        }
        let leg = &tree.leg[0];
        for network in [
            &leg.from.hop.network,
            &leg.from.participant.network,
            &leg.to.hop.network,
            &leg.to.participant.network,
        ] {
            assert_eq!(network, &tree_name);
        }
        let start = output
            .find(&format!("<tree name=\"{tree_name}\">"))
            .unwrap();
        let end = start + output[start..].find("</tree>").unwrap();
        assert!(output[start..end].contains("tree topology comment"));

        let star_name = format!("{prefix}/hub");
        let star = doc
            .star
            .iter()
            .find(|value| value.name == star_name)
            .unwrap();
        assert_eq!(star.coordinator.participant.network, star_name);
        assert_eq!(star.coordinator.participant.participant, "a");
        assert!(star.coordinator.participant.instance.is_none());
        let configuration = star.configuration.as_ref().unwrap();
        let domain = &configuration.gptp_domain[0];
        assert_eq!(domain.name, "default");
        assert_eq!(
            domain.port_defaults.as_ref().unwrap().log_sync_interval,
            Some(-3)
        );
        for clock in &domain.clock {
            assert_eq!(clock.participant.network, star_name);
            assert!(clock.participant.instance.is_none());
        }
        let open_class = &configuration.gate_schedule[0].gate[0].open.traffic_class[0];
        assert_eq!(open_class.network, star_name);
        assert!(open_class.instance.is_none());
        let assignment = &configuration.schedule_assignment[0];
        assert_eq!(assignment.schedule.network, star_name);
        assert!(assignment.schedule.instance.is_none());
        assert_eq!(assignment.target.len(), 2);
        for target in &assignment.target {
            assert_eq!(target.participant.network, star_name);
            assert!(target.participant.instance.is_none());
        }
    }
    let canonical = doc
        .to_connectivity_document(
            hcdformat::model::connectivity::DocumentIdentity::new("memory://flattened.hcdf")
                .unwrap(),
        )
        .unwrap();
    hcdformat::connectivity::normalize_connectivity(&canonical).unwrap();
}

#[test]
fn ref_to_non_included_name_stays() {
    // Sanity: a transmission motor ref whose text matches nothing in the module (neither a
    // module motor nor a module comp's slash-path) must pass through UNTOUCHED: the doc-level
    // dangling/ambiguity validators own it after flatten, not prefix().
    let module = r#"<hcdf name="m" version="1.0">
                      <comp name="rotor"/>
                      <joint name="spin"><child comp="rotor"/></joint>
                      <transmission name="t">
                        <motor ref="external/m0"/>
                        <joint ref="spin"/>
                      </transmission>
                    </hcdf>"#;
    let mut doc: Hcdf = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0"><include uri="m.hcdf" name="k"/></hcdf>"#,
    )
    .unwrap();
    let files = HashMap::from([("m.hcdf", module)]);
    let mut load = mem_loader(files);
    hcdformat::flatten_with(&mut doc, Path::new("/q"), &mut load).unwrap();

    let t = doc
        .transmission
        .iter()
        .find(|t| t.name.as_deref() == Some("k/t"))
        .unwrap();
    // motor.ref head 'external' is NOT a module comp -> stays; joint.ref 'spin' IS -> prefixed.
    assert_eq!(t.motor[0].ref_.as_deref(), Some("external/m0"));
    assert_eq!(t.joint[0].ref_.as_deref(), Some("k/spin"));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_flatten_reads_files() {
    // The native convenience reads real files relative to each file's own dir.
    let dir = std::env::temp_dir().join(format!("hcdf_flatten_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mod.hcdf"), module_xml()).unwrap();
    std::fs::write(
        dir.join("top.hcdf"),
        r#"<hcdf name="t" version="1.0"><include uri="mod.hcdf" name="L"/></hcdf>"#,
    )
    .unwrap();

    let (doc, _notes) = hcdformat::flatten_path(&dir.join("top.hcdf")).unwrap();
    assert!(find_comp(&doc, "L/rotor").is_some());
    // the relative mesh uri got rerooted under the module's real dir
    let rotor = find_comp(&doc, "L/rotor").unwrap();
    let VisualAppearance::Model { model, .. } = &rotor.visual[0].appearance else {
        panic!();
    };
    let uri = model.uri.as_deref().unwrap();
    assert!(
        uri.contains("meshes/rotor.glb") && Path::new(uri).is_absolute(),
        "uri {uri}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn flatten_path_seeds_root_so_back_reference_is_a_cycle() {
    // root.hcdf includes child.hcdf, which includes root.hcdf back. flatten_path seeds the root path on
    // the cycle stack (mirroring include.py flatten(path)), so this is a cycle Err, not infinite.
    let dir = std::env::temp_dir().join(format!("hcdf_cycle_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("root.hcdf"),
        r#"<hcdf name="root" version="1.0"><include uri="child.hcdf"/></hcdf>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("child.hcdf"),
        r#"<hcdf name="child" version="1.0"><include uri="root.hcdf"/></hcdf>"#,
    )
    .unwrap();

    let err = hcdformat::flatten_path(&dir.join("root.hcdf")).unwrap_err();
    assert!(err.contains("cycle"), "expected cycle, got {err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn flatten_doc_matches_flatten_path() {
    // The in-memory `flatten(doc, base_dir)` the binding exposes must produce the byte-identical flattened
    // document `flatten_path(file)` produces for the same tree. Both read the includes off `dir` and reroot
    // relative mesh uris under the module's real dir; with no self-including cycle, the empty-vs-seeded
    // cycle stack never diverges, so the flattened documents (and their notes) are identical.
    let dir = std::env::temp_dir().join(format!("hcdf_flatten_parity_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mod.hcdf"), module_xml()).unwrap();
    let top = r#"<hcdf name="t" version="1.0"><include uri="mod.hcdf" name="L"/></hcdf>"#;
    std::fs::write(dir.join("top.hcdf"), top).unwrap();

    let (from_path, path_notes) = hcdformat::flatten_path(&dir.join("top.hcdf")).unwrap();
    let mut from_doc = Hcdf::from_xml_str(top).unwrap();
    let doc_notes = hcdformat::flatten(&mut from_doc, &dir).unwrap();

    assert_eq!(
        from_path.to_xml_string().unwrap(),
        from_doc.to_xml_string().unwrap(),
        "flatten(doc, dir) equals flatten_path(file) on the same tree"
    );
    assert_eq!(
        path_notes, doc_notes,
        "the flatten notes agree for a non-cyclic tree"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A small explicit check that an unused endpoint helper import compiles (keeps the import meaningful).
#[test]
fn endpoint_type_is_constructible() {
    let e = JointEndpoint {
        comp: Some("x".into()),
    };
    assert_eq!(e.comp.as_deref(), Some("x"));
}

// ── <include> @sha verification (parity with include.py) ───────────────────────────────────────────

/// The content sha the mem_loader reports for the module: the value a CORRECT pin must carry.
fn module_sha() -> String {
    hcdformat::content_sha(module_xml().as_bytes())
}

#[test]
fn correct_sha_adds_no_mismatch_note() {
    // A pin that MATCHES the loader's source sha verifies silently: no note, include still resolves.
    let parent = format!(
        r#"<hcdf name="p" version="1.0"><include uri="module.hcdf" name="k" sha="{}"/></hcdf>"#,
        module_sha()
    );
    let mut doc = Hcdf::from_xml_str(&parent).unwrap();
    let files = HashMap::from([("module.hcdf", module_xml())]);
    let mut load = mem_loader(files);
    let notes = hcdformat::flatten_with(&mut doc, Path::new("/b"), &mut load).unwrap();

    assert!(
        doc.include.is_empty(),
        "correct @sha: include still resolves"
    );
    assert!(
        !notes.iter().any(|n| n.contains("sha mismatch")),
        "correct @sha must add NO mismatch note: {notes:?}"
    );
    assert!(find_comp(&doc, "k/rotor").is_some(), "module composed");
}

#[test]
fn wrong_sha_adds_mismatch_note_but_still_resolves() {
    // A pin that DIFFERS from the loader's source sha is a NOTE (never an Err) and flatten STILL composes.
    let wrong = format!("sha256:{}", "0".repeat(64));
    let parent = format!(
        r#"<hcdf name="p" version="1.0"><include uri="module.hcdf" name="k" sha="{wrong}"/></hcdf>"#
    );
    let mut doc = Hcdf::from_xml_str(&parent).unwrap();
    let files = HashMap::from([("module.hcdf", module_xml())]);
    let mut load = mem_loader(files);
    let notes = hcdformat::flatten_with(&mut doc, Path::new("/b"), &mut load).unwrap();

    let mismatches: Vec<&String> = notes
        .iter()
        .filter(|n| n.contains("sha mismatch"))
        .collect();
    assert_eq!(mismatches.len(), 1, "exactly one mismatch note: {notes:?}");
    assert!(
        mismatches[0].contains("pinned 000000000000") && mismatches[0].contains("actual"),
        "mismatch note names pinned + actual short shas: {}",
        mismatches[0]
    );
    assert!(
        doc.include.is_empty(),
        "wrong @sha: flatten STILL composes (note, not error)"
    );
    assert!(
        find_comp(&doc, "k/rotor").is_some(),
        "module composed despite mismatch"
    );
}

#[test]
fn no_sha_is_not_verified() {
    // No @sha → no verification at all, even though the loader supplies a source sha.
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0"><include uri="module.hcdf" name="k"/></hcdf>"#,
    )
    .unwrap();
    let files = HashMap::from([("module.hcdf", module_xml())]);
    let mut load = mem_loader(files);
    let notes = hcdformat::flatten_with(&mut doc, Path::new("/b"), &mut load).unwrap();
    assert!(
        !notes.iter().any(|n| n.contains("sha mismatch")),
        "a no-@sha include must never be verified: {notes:?}"
    );
}

#[test]
fn none_loader_sha_skips_verification() {
    // A loader that returns None for the source sha (cannot supply it) skips verification even when the
    // include pins a (here deliberately wrong) sha: None means "unknown", not "mismatch".
    let wrong = format!("sha256:{}", "0".repeat(64));
    let parent = format!(
        r#"<hcdf name="p" version="1.0"><include uri="module.hcdf" sha="{wrong}"/></hcdf>"#
    );
    let mut doc = Hcdf::from_xml_str(&parent).unwrap();
    let files = HashMap::from([("module.hcdf", module_xml())]);
    // A loader that drops the sha (returns None).
    let mut load = move |key: &str, _base: &Path| {
        let name = Path::new(key).file_name().and_then(|n| n.to_str()).unwrap();
        files
            .get(name)
            .map(|xml| (Hcdf::from_xml_str(xml).unwrap(), None))
            .ok_or_else(|| "missing".to_string())
    };
    let notes = hcdformat::flatten_with(&mut doc, Path::new("/b"), &mut load).unwrap();
    assert!(
        !notes.iter().any(|n| n.contains("sha mismatch")),
        "a None loader-sha must skip verification: {notes:?}"
    );
}

#[test]
fn empty_sha_pin_is_not_verified() {
    // An EMPTY `sha=""` pin (serde keeps it as `Some("")`) must NOT verify, matching Python's
    // truthiness guard `if inc.sha:`. Without the emptiness filter, Rust would push a malformed
    // "sha mismatch (pinned , actual ..)" note for what is effectively no pin.
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0"><include uri="module.hcdf" name="k" sha=""/></hcdf>"#,
    )
    .unwrap();
    let files = HashMap::from([("module.hcdf", module_xml())]);
    let mut load = mem_loader(files);
    let notes = hcdformat::flatten_with(&mut doc, Path::new("/b"), &mut load).unwrap();
    assert!(
        !notes.iter().any(|n| n.contains("sha mismatch")),
        "an empty sha=\"\" pin must add NO mismatch note: {notes:?}"
    );
    assert!(doc.include.is_empty(), "empty-sha include still composes");
    assert!(find_comp(&doc, "k/rotor").is_some(), "module composed");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn stamp_self_including_module_is_a_cycle_err_not_a_stack_overflow() {
    // A module that includes ITSELF must be a clean cycle `Err` from stamp_include_shas (matching
    // flatten), NOT an unbounded recursion / stack-overflow process abort. This is the failure mode the
    // dendrite "Pin includes" button relies on catching (an Err it reports, vs a SIGABRT it cannot).
    let dir = std::env::temp_dir().join(format!("hcdf_stamp_cycle_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("cycle.hcdf"),
        r#"<hcdf name="cycle" version="1.0"><include uri="cycle.hcdf"/></hcdf>"#,
    )
    .unwrap();
    let xml = std::fs::read_to_string(dir.join("cycle.hcdf")).unwrap();
    let mut doc = Hcdf::from_xml_str(&xml).unwrap();
    let err = hcdformat::stamp_include_shas(&mut doc, &dir).unwrap_err();
    assert!(err.contains("cycle"), "expected a cycle Err, got {err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn stamp_mutually_including_modules_is_a_cycle_err() {
    // a.hcdf includes b.hcdf includes a.hcdf: a MUTUAL cycle through stamp must also be a clean Err.
    let dir = std::env::temp_dir().join(format!("hcdf_stamp_mutual_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("a.hcdf"),
        r#"<hcdf name="a" version="1.0"><include uri="b.hcdf"/></hcdf>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("b.hcdf"),
        r#"<hcdf name="b" version="1.0"><include uri="a.hcdf"/></hcdf>"#,
    )
    .unwrap();
    let xml = std::fs::read_to_string(dir.join("a.hcdf")).unwrap();
    let mut doc = Hcdf::from_xml_str(&xml).unwrap();
    let err = hcdformat::stamp_include_shas(&mut doc, &dir).unwrap_err();
    assert!(err.contains("cycle"), "expected a cycle Err, got {err}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── native stamp_include_shas (parity with include.py::stamp_include_shas) ──────────────────────────

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn stamp_include_shas_populates_module_content_sha() {
    // Stamp an UNPINNED include against the real on-disk module; the @sha must equal the content sha of
    // the module's RAW bytes: the SAME value Python `file_sha` produces on the same file (sha256 hex).
    let dir = std::env::temp_dir().join(format!("hcdf_stamp_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mod.hcdf"), module_xml()).unwrap();
    std::fs::write(
        dir.join("top.hcdf"),
        r#"<hcdf name="t" version="1.0"><include uri="mod.hcdf" name="L"/></hcdf>"#,
    )
    .unwrap();

    let xml = std::fs::read_to_string(dir.join("top.hcdf")).unwrap();
    let mut doc = Hcdf::from_xml_str(&xml).unwrap();
    assert!(doc.include[0].sha.is_none(), "starts unpinned");

    let notes = hcdformat::stamp_include_shas(&mut doc, &dir).unwrap();
    // The stamped sha equals the content sha of the on-disk module bytes (== Python file_sha).
    let expected = hcdformat::content_sha(&std::fs::read(dir.join("mod.hcdf")).unwrap());
    assert_eq!(
        doc.include[0].sha.as_deref(),
        Some(expected.as_str()),
        "stamp pins the module content sha"
    );
    assert!(
        notes.iter().any(|n| n.starts_with("stamped include")),
        "a stamp note is recorded: {notes:?}"
    );

    // A freshly-stamped doc flattens with NO mismatch (the pin now matches the bytes).
    let flat_notes = hcdformat::flatten(&mut doc.clone(), &dir).unwrap();
    assert!(
        !flat_notes.iter().any(|n| n.contains("sha mismatch")),
        "a freshly-stamped sha must verify clean: {flat_notes:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn stamp_include_shas_updates_stale_pin() {
    // A STALE @sha is overwritten with the module's CURRENT content sha.
    let dir = std::env::temp_dir().join(format!("hcdf_stamp_stale_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mod.hcdf"), module_xml()).unwrap();
    let stale = format!("sha256:{}", "0".repeat(64));
    std::fs::write(
        dir.join("top.hcdf"),
        format!(r#"<hcdf name="t" version="1.0"><include uri="mod.hcdf" name="L" sha="{stale}"/></hcdf>"#),
    )
    .unwrap();

    let xml = std::fs::read_to_string(dir.join("top.hcdf")).unwrap();
    let mut doc = Hcdf::from_xml_str(&xml).unwrap();
    assert_eq!(
        doc.include[0].sha.as_deref(),
        Some(stale.as_str()),
        "starts with the stale pin"
    );

    hcdformat::stamp_include_shas(&mut doc, &dir).unwrap();
    let expected = hcdformat::content_sha(&std::fs::read(dir.join("mod.hcdf")).unwrap());
    assert_eq!(
        doc.include[0].sha.as_deref(),
        Some(expected.as_str()),
        "stale pin updated to current sha"
    );
    assert_ne!(
        doc.include[0].sha.as_deref(),
        Some(stale.as_str()),
        "the stale value is gone"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── per-call module memoization (flatten / verify / stamp load each key once) ───────────────────────

/// A standalone-root module (no joints, so `thing` IS a root and the include pose offsets its visual);
/// lets the memo tests observe that every instance is still INDEPENDENTLY offset off one shared load.
fn root_module_xml() -> &'static str {
    r#"<hcdf name="m" version="1.0">
         <comp name="thing">
           <visual name="v"><pose xyz="1 0 0"/></visual>
         </comp>
       </hcdf>"#
}

#[test]
fn module_included_four_times_loads_once_with_unchanged_output() {
    // Four instances of ONE module: the loader must be invoked exactly once (per-flatten memo) while
    // every instance still composes independently (own prefix, own pose offset). The flattened doc must
    // be BYTE-IDENTICAL to flattening the same structure through four DISTINCT uris of identical
    // content (which never hits the memo), proving memoization changes loader traffic only.
    let quad = |uris: [&str; 4]| {
        format!(
            r#"<hcdf name="quad" version="1.0">
             <comp name="hub"/>
             <include uri="{}" name="fl" pose="1 1 0"/>
             <include uri="{}" name="fr" pose="1 -1 0"/>
             <include uri="{}" name="rl" pose="-1 1 0"/>
             <include uri="{}" name="rr" pose="-1 -1 0"/>
           </hcdf>"#,
            uris[0], uris[1], uris[2], uris[3]
        )
    };
    let calls = std::cell::Cell::new(0usize);
    let mut load = |_key: &str, _base: &Path| {
        calls.set(calls.get() + 1);
        Hcdf::from_xml_str(root_module_xml())
            .map(|doc| {
                (
                    doc,
                    Some(hcdformat::content_sha(root_module_xml().as_bytes())),
                )
            })
            .map_err(|e| e.to_string())
    };

    // Same uri x4: one load, four independent instances.
    let mut doc = Hcdf::from_xml_str(&quad(["m.hcdf"; 4])).unwrap();
    let notes = hcdformat::flatten_with(&mut doc, Path::new("/q"), &mut load).unwrap();
    assert_eq!(
        calls.get(),
        1,
        "four instances of one module must load ONCE"
    );
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.starts_with("flattened include"))
            .count(),
        4
    );
    for (p, xyz) in [
        ("fl", [2.0, 1.0, 0.0]),
        ("fr", [2.0, -1.0, 0.0]),
        ("rl", [0.0, 1.0, 0.0]),
        ("rr", [0.0, -1.0, 0.0]),
    ] {
        let c = find_comp(&doc, &format!("{p}/thing")).expect(p);
        let v = c.visual[0].pose.clone().unwrap();
        assert_eq!(v.xyz_or_zero(), xyz, "instance {p} independently offset");
    }

    // Distinct uris x4: four loads, and (uris aside) the SAME composition.
    calls.set(0);
    let mut plain =
        Hcdf::from_xml_str(&quad(["m1.hcdf", "m2.hcdf", "m3.hcdf", "m4.hcdf"])).unwrap();
    hcdformat::flatten_with(&mut plain, Path::new("/q"), &mut load).unwrap();
    assert_eq!(calls.get(), 4, "distinct uris never hit the memo");
    assert_eq!(
        doc.to_xml_string().unwrap(),
        plain.to_xml_string().unwrap(),
        "memoized flatten output must match the unmemoized composition"
    );
}

#[test]
fn verify_checks_every_pin_but_walks_a_shared_module_once() {
    // Two sites include ONE module with DIFFERENT pins: the module loads once, yet BOTH pins are still
    // checked per site (only the wrong one reports). A stale pin NESTED inside a module included twice
    // is reported ONCE, not once per include path (the accepted dedup of the per-call memo).
    let wrong = format!("sha256:{}", "0".repeat(64));
    let stale = format!("sha256:{}", "1".repeat(64));
    let mid = format!(
        r#"<hcdf name="mid" version="1.0"><include uri="module.hcdf" sha="{stale}"/></hcdf>"#
    );
    // Site `a` pins mid's raw sha (as the counting loader below reports it), which is correct; site `b` doesn't.
    let good_mid = hcdformat::content_sha(mid.as_bytes());
    let doc = Hcdf::from_xml_str(&format!(
        r#"<hcdf name="p" version="1.0">
             <include uri="mid.hcdf" name="a" sha="{good_mid}"/>
             <include uri="mid.hcdf" name="b" sha="{wrong}"/>
           </hcdf>"#,
    ))
    .unwrap();
    let calls = std::cell::Cell::new(0usize);
    let mut load = |key: &str, _base: &Path| {
        calls.set(calls.get() + 1);
        let name = Path::new(key)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(key);
        let xml = match name {
            "mid.hcdf" => mid.as_str(),
            "module.hcdf" => module_xml(),
            _ => return Err(format!("file not found at {key:?}")),
        };
        Ok((
            Hcdf::from_xml_str(xml).unwrap(),
            Some(hcdformat::content_sha(xml.as_bytes())),
        ))
    };
    let out = hcdformat::verify_include_shas(&doc, Path::new("/v"), &mut load);
    assert_eq!(
        calls.get(),
        2,
        "mid + module each load once despite two include paths"
    );
    // site b's wrong DIRECT pin reports; site a's correct pin does not; the stale NESTED pin (inside
    // mid, reached through both a and b) reports once.
    assert_eq!(
        out.len(),
        2,
        "one direct + one deduped nested mismatch: {out:?}"
    );
    assert!(
        out.iter()
            .any(|m| m.include_name.as_deref() == Some("b") && m.expected == wrong),
        "the repeat site's own wrong pin is still checked: {out:?}"
    );
    assert_eq!(
        out.iter().filter(|m| m.expected == stale).count(),
        1,
        "the nested stale pin dedupes to ONE report: {out:?}"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn stamp_repeat_include_replays_subtree_notes() {
    // top includes mid TWICE; mid includes leaf. The memo stamps mid once but must REPLAY its subtree
    // note at the second site, so the note stream (which drives dendrite's "pinned N include sha(s)"
    // count) is identical to an unmemoized walk: 2 direct mid stamps + 2 leaf stamps.
    let dir = std::env::temp_dir().join(format!("hcdf_stamp_memo_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("leaf.hcdf"), module_xml()).unwrap();
    std::fs::write(
        dir.join("mid.hcdf"),
        r#"<hcdf name="mid" version="1.0"><include uri="leaf.hcdf" name="i"/></hcdf>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("top.hcdf"),
        r#"<hcdf name="t" version="1.0">
             <include uri="mid.hcdf" name="L"/>
             <include uri="mid.hcdf" name="R"/>
           </hcdf>"#,
    )
    .unwrap();

    let xml = std::fs::read_to_string(dir.join("top.hcdf")).unwrap();
    let mut doc = Hcdf::from_xml_str(&xml).unwrap();
    let notes = hcdformat::stamp_include_shas(&mut doc, &dir).unwrap();

    let mid_sha = hcdformat::content_sha(&std::fs::read(dir.join("mid.hcdf")).unwrap());
    assert_eq!(
        doc.include[0].sha.as_deref(),
        Some(mid_sha.as_str()),
        "first instance pinned"
    );
    assert_eq!(
        doc.include[1].sha.as_deref(),
        Some(mid_sha.as_str()),
        "repeat instance pinned from the memo"
    );
    let stamped: Vec<&String> = notes
        .iter()
        .filter(|n| n.starts_with("stamped include"))
        .collect();
    assert_eq!(
        stamped.len(),
        4,
        "2 direct + 2 (replayed) nested stamp notes: {notes:?}"
    );
    assert_eq!(
        notes.iter().filter(|n| n.contains("\"leaf.hcdf\"")).count(),
        2,
        "the leaf subtree note is replayed at the repeat site: {notes:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_flatten_correct_pin_no_note_wrong_pin_notes() {
    // End-to-end through the NATIVE fs_loader: stamp pins the module, then flatten verifies clean; a
    // tampered pin yields a mismatch note (proving fs_loader's source sha drives verification on disk).
    let dir = std::env::temp_dir().join(format!("hcdf_native_pin_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mod.hcdf"), module_xml()).unwrap();
    let good = hcdformat::content_sha(&std::fs::read(dir.join("mod.hcdf")).unwrap());

    // Correct pin → flatten_path verifies with no mismatch note.
    std::fs::write(
        dir.join("ok.hcdf"),
        format!(
            r#"<hcdf name="t" version="1.0"><include uri="mod.hcdf" name="L" sha="{good}"/></hcdf>"#
        ),
    )
    .unwrap();
    let (_doc, notes) = hcdformat::flatten_path(&dir.join("ok.hcdf")).unwrap();
    assert!(
        !notes.iter().any(|n| n.contains("sha mismatch")),
        "correct on-disk pin verifies clean: {notes:?}"
    );

    // Tampered pin → a mismatch note, but flatten still composes.
    let bad = format!("sha256:{}", "1".repeat(64));
    std::fs::write(
        dir.join("bad.hcdf"),
        format!(
            r#"<hcdf name="t" version="1.0"><include uri="mod.hcdf" name="L" sha="{bad}"/></hcdf>"#
        ),
    )
    .unwrap();
    let (doc, notes) = hcdformat::flatten_path(&dir.join("bad.hcdf")).unwrap();
    assert!(
        notes.iter().any(|n| n.contains("sha mismatch")),
        "tampered on-disk pin notes a mismatch: {notes:?}"
    );
    assert!(
        find_comp(&doc, "L/rotor").is_some(),
        "module still composed despite mismatch"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── Include-composition safety batch (the DM8009×N fixture) ─────────────────────────────────────────
//
// A parent that includes ONE motorized module N=3 times plus a SECOND module that connects its comps
// through top-level topology records. Post-flatten the whole document must validate with zero errors across
// every family, every transmission motor ref must resolve UNIQUELY, the self-collision pairs must
// survive PREFIXED, and the second module's internal network refs must resolve PREFIXED.

/// Module A, the DM8009-class motorized module: a `dm8009p_case` comp (CAN and power ports plus the
/// `foc_motor`) and a `dm8009p_flange` comp joined by a revolute joint, a
/// BARE-ref transmission (motor `foc_motor`, document-unique WITHIN the module), and a self-collision
/// pair. Included N times, each instance's bare motor ref must resolve to its OWN case (unique motor
/// resolution) and its pair must survive prefixed.
fn dm8009_module_xml() -> &'static str {
    r#"<hcdf name="dm8009-module" version="1.0">
         <comp name="dm8009p_case">
           <port name="can0"/>
           <port name="pwr"/>
           <motor name="foc_motor" type="bldc">
             <torque-constant unit="Nm/A">0.29</torque-constant>
           </motor>
         </comp>
         <comp name="dm8009p_flange"/>
         <joint name="axis" type="revolute">
           <parent comp="dm8009p_case"/>
           <child comp="dm8009p_flange"/>
           <origin xyz="0 0 0.03"/>
           <axis xyz="0 0 1"/>
           <limit lower="-3.14" upper="3.14" effort="20" velocity="30"/>
         </joint>
         <transmission name="drive" type="planetary">
           <motor ref="foc_motor"/>
           <joint ref="axis"/>
           <reduction>9</reduction>
         </transmission>
         <self-collision-disable>
           <pair comp1="dm8009p_case" comp2="dm8009p_flange" reason="adjacent"/>
         </self-collision-disable>
       </hcdf>"#
}

/// Module B, a two-component module with three internal network topologies. Exercises structured
/// participant component references across link, bus, and star records during include prefixing.
fn comms_module_xml() -> &'static str {
    r#"<hcdf name="comms-module" version="1.0">
         <comp name="mcu_a">
           <port name="eth0"/>
           <port name="can0"/>
           <port name="wifi0"/>
         </comp>
         <comp name="mcu_b">
           <port name="eth0"/>
           <port name="can0"/>
           <port name="wifi0"/>
         </comp>
         <link name="uplink">
           <selected purpose="communication" carrier="electrical"/>
           <participant name="a"><endpoint><port-ref component="mcu_a" port="eth0"/></endpoint></participant>
           <participant name="b"><endpoint><port-ref component="mcu_b" port="eth0"/></endpoint></participant>
         </link>
         <bus name="canbus">
           <selected purpose="communication" carrier="electrical"/>
           <participant name="a"><endpoint><port-ref component="mcu_a" port="can0"/></endpoint></participant>
           <participant name="b"><endpoint><port-ref component="mcu_b" port="can0"/></endpoint></participant>
         </bus>
         <star name="wifi">
           <coordinator><participant-ref network="wifi" participant="a"/></coordinator>
           <selected purpose="communication" carrier="radiated-rf"/>
           <participant name="a"><endpoint><port-ref component="mcu_a" port="wifi0"/></endpoint></participant>
           <participant name="b"><endpoint><port-ref component="mcu_b" port="wifi0"/></endpoint></participant>
         </star>
       </hcdf>"#
}

/// The errors (level == Error) across EVERY validator family, `code: message` for a readable panic.
fn all_family_errors(doc: &Hcdf) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let families: [Vec<Issue>; 4] = [
        validate_semantic(doc),
        validate_network(doc),
        validate_loops(doc),
        validate_coverage(doc),
    ];
    for issues in families {
        for i in issues {
            if i.level == Level::Error {
                out.push(format!("{}: {}", i.code, i.message));
            }
        }
    }
    out
}

#[test]
fn dm8009_times_n_flattens_to_a_zero_error_document() {
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="humanoid-arm" version="1.0">
             <comp name="base"/>
             <include uri="dm8009.hcdf" name="drive1" pose="0 0 0.10"/>
             <include uri="dm8009.hcdf" name="drive2" pose="0 0 0.20"/>
             <include uri="dm8009.hcdf" name="drive3" pose="0 0 0.30"/>
             <include uri="comms.hcdf" name="commsX"/>
             <bus name="joint_can">
               <selected purpose="communication" carrier="electrical"/>
               <participant name="drive1"><endpoint><port-ref component="drive1/dm8009p_case" port="can0"/></endpoint></participant>
               <participant name="drive2"><endpoint><port-ref component="drive2/dm8009p_case" port="can0"/></endpoint></participant>
               <participant name="drive3"><endpoint><port-ref component="drive3/dm8009p_case" port="can0"/></endpoint></participant>
             </bus>
             <bus name="power">
               <selected purpose="power-delivery" carrier="electrical"/>
               <participant name="drive1"><endpoint><port-ref component="drive1/dm8009p_case" port="pwr"/></endpoint></participant>
               <participant name="drive2"><endpoint><port-ref component="drive2/dm8009p_case" port="pwr"/></endpoint></participant>
               <participant name="drive3"><endpoint><port-ref component="drive3/dm8009p_case" port="pwr"/></endpoint></participant>
             </bus>
           </hcdf>"#,
    )
    .unwrap();
    let files = HashMap::from([
        ("dm8009.hcdf", dm8009_module_xml()),
        ("comms.hcdf", comms_module_xml()),
    ]);
    let mut load = mem_loader(files);
    hcdformat::flatten_with(&mut doc, Path::new("/abs/base"), &mut load).unwrap();
    assert!(doc.include.is_empty(), "all includes resolved");

    // (0) EVERY validator family is error-free on the flattened document.
    let errors = all_family_errors(&doc);
    assert!(
        errors.is_empty(),
        "flattened document has validation errors: {errors:#?}"
    );

    // Every transmission's bare motor ref is rewritten to the instance-qualified slash-path, so the
    // three same-named `foc_motor`s resolve UNIQUELY (no bare, no ambiguity). Joint refs prefixed too.
    let mut motor_refs: Vec<&str> = doc
        .transmission
        .iter()
        .flat_map(|t| t.motor.iter())
        .filter_map(|m| m.ref_.as_deref())
        .collect();
    motor_refs.sort_unstable();
    assert_eq!(
        motor_refs,
        [
            "drive1/dm8009p_case/foc_motor",
            "drive2/dm8009p_case/foc_motor",
            "drive3/dm8009p_case/foc_motor",
        ],
        "each bare motor ref resolved to its own instance's case"
    );
    for t in &doc.transmission {
        assert_eq!(t.joint.len(), 1);
        let jr = t.joint[0].ref_.as_deref().unwrap();
        assert!(
            jr.ends_with("/axis") && jr.starts_with("drive"),
            "joint ref prefixed: {jr}"
        );
    }

    // All three self-collision pairs survived the merge, each prefixed to its OWN instance's comps.
    let scd = doc
        .self_collision_disable
        .as_ref()
        .expect("the merged block carries the included pairs");
    let mut pairs: Vec<(&str, &str)> = scd
        .pair
        .iter()
        .map(|p| (p.comp1.as_deref().unwrap(), p.comp2.as_deref().unwrap()))
        .collect();
    pairs.sort_unstable();
    assert_eq!(
        pairs,
        [
            ("drive1/dm8009p_case", "drive1/dm8009p_flange"),
            ("drive2/dm8009p_case", "drive2/dm8009p_flange"),
            ("drive3/dm8009p_case", "drive3/dm8009p_flange"),
        ],
        "each pair prefixed to its own instance"
    );

    // The comms module's structured participant refs were rewritten to the prefixed components.
    let link = doc
        .link
        .iter()
        .find(|network| network.name == "commsX/uplink")
        .expect("the module's internal link merged");
    let link_components = link
        .participant
        .iter()
        .map(|participant| participant_port_ref(participant).component.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        link_components,
        ["commsX/mcu_a", "commsX/mcu_b"],
        "link participant components prefixed"
    );
    let bus = doc
        .bus
        .iter()
        .find(|network| network.name == "commsX/canbus")
        .expect("the module's internal bus merged");
    let star = doc
        .star
        .iter()
        .find(|network| network.name == "commsX/wifi")
        .expect("the module's internal star merged");
    for participants in [&bus.participant, &star.participant] {
        assert_eq!(
            participant_port_ref(&participants[0]).component,
            "commsX/mcu_a"
        );
        assert_eq!(
            participant_port_ref(&participants[1]).component,
            "commsX/mcu_b"
        );
    }
}

/// Unique-resolution negative branch: a module that is INTERNALLY ambiguous (two comps each carry a
/// `foc_motor`, with a bare transmission ref) must NOT be rewritten by prefix(); the bare name stays bare so the doc-level
/// `E_TRANS_MOTOR_AMBIGUOUS` remains the single, correct backstop after flattening.
#[test]
fn intra_module_ambiguous_bare_motor_ref_stays_bare_for_the_doc_validator() {
    let module = r#"<hcdf name="ambi" version="1.0">
                      <comp name="a"><motor name="foc_motor" type="bldc"/></comp>
                      <comp name="b"><motor name="foc_motor" type="bldc"/></comp>
                      <joint name="j" type="fixed"><parent comp="a"/><child comp="b"/></joint>
                      <transmission name="t"><motor ref="foc_motor"/><joint ref="j"/></transmission>
                    </hcdf>"#;
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0"><include uri="ambi.hcdf" name="k"/></hcdf>"#,
    )
    .unwrap();
    let files = HashMap::from([("ambi.hcdf", module)]);
    let mut load = mem_loader(files);
    hcdformat::flatten_with(&mut doc, Path::new("/x"), &mut load).unwrap();

    // The bare ref is left bare (ambiguous within the module, so prefix() cannot pick a comp).
    let t = doc
        .transmission
        .iter()
        .find(|t| t.name.as_deref() == Some("k/t"))
        .unwrap();
    assert_eq!(
        t.motor[0].ref_.as_deref(),
        Some("foc_motor"),
        "ambiguous bare ref untouched"
    );
    // …and the doc-level validator flags exactly that.
    assert!(
        validate_coverage(&doc)
            .iter()
            .any(|i| i.code == "E_TRANS_MOTOR_AMBIGUOUS"),
        "the doc-level backstop fires on the ambiguous bare ref"
    );
}

#[test]
fn parent_authored_hop_owners_collapse_after_nested_flatten() {
    let leaf = r#"<hcdf name="leaf" version="1.0"><comp name="device">
      <port name="in"/><port name="out"/>
      <switch name="bridge"><input><port-ref component="device" port="in"/></input><output><port-ref component="device" port="out"/></output></switch>
    </comp></hcdf>"#;
    let middle =
        r#"<hcdf name="module" version="1.0"><include uri="leaf.hcdf" name="inner"/></hcdf>"#;
    let scope = r#"<instance><segment name="outer" occurrence="0"/><segment name="inner" occurrence="0"/></instance>"#;
    let source = format!(
        r#"<hcdf name="top" version="1.0"><chain name="route">
          <hop name="function"><owner><function-ref component="device" function="bridge">{scope}</function-ref></owner></hop>
          <hop name="component"><owner><component-ref component="device">{scope}</component-ref></owner></hop>
        </chain><include uri="module.hcdf" name="outer"/></hcdf>"#
    );
    let mut doc = Hcdf::from_xml_str(&source).unwrap();
    let mut load = mem_loader(HashMap::from([
        ("module.hcdf", middle),
        ("leaf.hcdf", leaf),
    ]));
    hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap();

    let hops = &doc.chain[0].hop;
    match &hops[0].owner.owner {
        connectivity_xml::HopOwnerChoice::Function(reference) => {
            assert_eq!(reference.component, "outer/inner/device");
            assert_eq!(reference.function, "bridge");
            assert!(reference.instance.is_none());
        }
        other => panic!("expected function owner, got {other:?}"),
    }
    match &hops[1].owner.owner {
        connectivity_xml::HopOwnerChoice::Component(reference) => {
            assert_eq!(reference.component, "outer/inner/device");
            assert!(reference.instance.is_none());
        }
        other => panic!("expected component owner, got {other:?}"),
    }
}

#[test]
fn parent_authored_descendant_connectivity_refs_collapse_after_nested_flatten() {
    let leaf = r#"<hcdf name="leaf" version="1.0">
      <comp name="device">
        <visual name="shell"><model uri="package://device/shell.glb"/></visual>
        <frame name="mount"/>
        <port name="data"/>
        <connector name="J"><pin name="1"/></connector>
        <connector name="inside"><representation><sphere radius="1">
          <placement xyz="1 0 0">
            <frame><component-origin component="device"/></frame>
            <rotation><rpy value="0 0 0"/></rotation>
          </placement>
        </sphere></representation></connector>
      </comp>
      <harness name="loom">
        <connector name="H"/>
        <splice name="S" fidelity="presented"/>
        <representation>
          <model uri="package://device/loom.glb">
            <placement xyz="0 0 0">
              <frame><world/></frame>
              <rotation><rpy value="0 0 0"/></rotation>
            </placement>
          </model>
        </representation>
      </harness>
    </hcdf>"#;
    let middle = r#"<hcdf name="module" version="1.0">
      <include uri="leaf.hcdf" name="inner" pose="1 0 0 0 0 1.5707963267948966"/>
    </hcdf>"#;
    let scope = r#"<instance>
      <segment name="outer" occurrence="0"/>
      <segment name="inner" occurrence="0"/>
    </instance>"#;
    let xml = format!(
        r#"<hcdf name="top" version="1.0">
          <comp name="host">
            <port name="data"/>
            <connector name="origin"><representation><sphere radius="1">
              <placement xyz="1 0 0">
                <frame><component-origin component="device">{scope}</component-origin></frame>
                <rotation><rpy value="0 0 0"/></rotation>
              </placement>
            </sphere></representation></connector>
            <connector name="visual-part">
              <representation>
                <model-part node-path="pins">
                  <model-root>
                    <component-visual component="device" visual="shell">{scope}</component-visual>
                  </model-root>
                </model-part>
              </representation>
            </connector>
            <connector name="assembly-part">
              <representation>
                <model-part node-path="body">
                  <model-root>
                    <assembly-model assembly="loom">{scope}</assembly-model>
                  </model-root>
                </model-part>
              </representation>
            </connector>
            <connector name="placed">
              <representation>
                <sphere radius="0.01">
                  <placement xyz="0 0 0">
                    <frame>
                      <component-frame component="device" frame="mount">{scope}</component-frame>
                    </frame>
                    <rotation><rpy value="0 0 0"/></rotation>
                  </placement>
                </sphere>
              </representation>
            </connector>
            <connector name="route">
              <representation>
                <derived-route>
                  <waypoint xyz="0 0 0">
                    <frame><component-origin component="device">{scope}</component-origin></frame>
                  </waypoint>
                  <waypoint xyz="1 0 0">
                    <frame>
                      <component-frame component="device" frame="mount">{scope}</component-frame>
                    </frame>
                  </waypoint>
                </derived-route>
              </representation>
            </connector>
          </comp>
          <binding name="parent-binding" fidelity="exact">
            <functional><port-ref component="device" port="data">{scope}</port-ref></functional>
            <physical>
              <position-ref connector="J" position="1">
                <component-ref component="device">{scope}</component-ref>
              </position-ref>
            </physical>
          </binding>
          <binding name="parent-junction" fidelity="presented">
            <functional><port-ref component="device" port="data">{scope}</port-ref></functional>
            <physical>
              <junction-ref junction="S"><assembly-ref assembly="loom">{scope}</assembly-ref></junction-ref>
            </physical>
          </binding>
          <mate name="parent-mate" fidelity="exact">
            <first connector="J"><component-ref component="device">{scope}</component-ref></first>
            <second connector="H"><assembly-ref assembly="loom">{scope}</assembly-ref></second>
          </mate>
          <link name="parent-link">
            <selected purpose="communication" carrier="electrical"/>
            <participant name="local"><endpoint><port-ref component="host" port="data"/></endpoint></participant>
            <participant name="descendant"><endpoint><port-ref component="device" port="data">{scope}</port-ref></endpoint></participant>
          </link>
          <include uri="module.hcdf" name="outer" pose="10 0 0 0 0 1.5707963267948966"/>
        </hcdf>"#
    );
    let mut doc = Hcdf::from_xml_str(&xml).unwrap();
    let mut load = mem_loader(HashMap::from([
        ("module.hcdf", middle),
        ("leaf.hcdf", leaf),
    ]));

    hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap();

    assert!(doc.include.is_empty());
    assert!(doc
        .comp
        .iter()
        .any(|component| component.name == "outer/inner/device"));
    assert!(doc
        .harness
        .iter()
        .any(|assembly| assembly.name == "outer/inner/loom"));
    let participants = &doc.link[0].participant;
    assert_eq!(participant_port_ref(&participants[0]).component, "host");
    assert!(participant_port_ref(&participants[0]).instance.is_none());
    assert_eq!(
        participant_port_ref(&participants[1]).component,
        "outer/inner/device"
    );
    assert!(participant_port_ref(&participants[1]).instance.is_none());
    let binding = doc
        .binding
        .iter()
        .find(|binding| binding.name == "parent-binding")
        .unwrap();
    let connectivity_xml::PhysicalEndpointChoice::Position(position) = &binding.physical.endpoint
    else {
        panic!("expected flattened position reference");
    };
    let connectivity_xml::PhysicalOwnerChoice::Component(owner) = &position.owner else {
        panic!("expected flattened component owner");
    };
    assert_eq!(owner.component, "outer/inner/device");
    assert!(owner.instance.is_none());
    let junction_binding = doc
        .binding
        .iter()
        .find(|binding| binding.name == "parent-junction")
        .unwrap();
    let connectivity_xml::PhysicalEndpointChoice::Junction(junction) =
        &junction_binding.physical.endpoint
    else {
        panic!("expected flattened junction reference");
    };
    let connectivity_xml::PhysicalOwnerChoice::Assembly(junction_owner) = &junction.owner else {
        panic!("expected flattened junction assembly owner");
    };
    assert_eq!(junction_owner.assembly, "outer/inner/loom");
    assert!(junction_owner.instance.is_none());
    let mate = doc
        .mate
        .iter()
        .find(|mate| mate.name == "parent-mate")
        .unwrap();
    let connectivity_xml::PhysicalOwnerChoice::Component(first_owner) = &mate.first.owner else {
        panic!("expected flattened component mate owner");
    };
    assert_eq!(first_owner.component, "outer/inner/device");
    assert!(first_owner.instance.is_none());
    let connectivity_xml::PhysicalOwnerChoice::Assembly(second_owner) = &mate.second.owner else {
        panic!("expected flattened assembly mate owner");
    };
    assert_eq!(second_owner.assembly, "outer/inner/loom");
    assert!(second_owner.instance.is_none());
    let child_origin = sphere_placement(&doc, "outer/inner/device", "inside");
    let parent_origin = sphere_placement(&doc, "host", "origin");
    assert_vec3_close(parent_origin.xyz, child_origin.xyz);
    assert_eq!(parent_origin.rotation, child_origin.rotation);
    let route = match &connector_representation(&doc, "host", "route").variant {
        connectivity_xml::RepresentationChoice::DerivedRoute(value) => value,
        other => panic!("expected route representation, got {other:?}"),
    };
    assert_vec3_close(route.waypoint[0].xyz, [10.0, 1.0, 0.0]);
    assert_vec3_close(route.waypoint[1].xyz, [1.0, 0.0, 0.0]);
    let flattened = doc.to_xml_string().unwrap();
    assert!(!flattened.contains("<instance>"), "{flattened}");
    assert!(flattened.contains("component=\"outer/inner/device\""));
    assert!(flattened.contains("assembly=\"outer/inner/loom\""));
}

#[test]
fn parent_component_origins_match_child_geometry_through_include_pose() {
    let module = r#"<hcdf name="module" version="1.0">
      <comp name="device">
        <connector name="inside"><representation><sphere radius="1">
          <placement xyz="1 0 0">
            <frame><component-origin component="device"/></frame>
            <rotation><rpy value="0 0 0"/></rotation>
          </placement>
        </sphere></representation></connector>
      </comp>
      <comp name="arm">
        <connector name="inside"><representation><sphere radius="1">
          <placement xyz="1 0 0">
            <frame><component-origin component="arm"/></frame>
            <rotation><rpy value="0 0 0"/></rotation>
          </placement>
        </sphere></representation></connector>
      </comp>
      <joint name="arm-joint" type="fixed">
        <parent comp="device"/><child comp="arm"/>
      </joint>
    </hcdf>"#;
    let scope = r#"<instance><segment name="child" occurrence="0"/></instance>"#;
    let xml = format!(
        r#"<hcdf name="parent" version="1.0"><comp name="host">
          <connector name="outside"><representation><sphere radius="1">
            <placement xyz="1 0 0">
              <frame><component-origin component="device">{scope}</component-origin></frame>
              <rotation><rpy value="0 0 0"/></rotation>
            </placement>
          </sphere></representation></connector>
          <connector name="model"><representation>
            <model uri="package://device/body.glb"><placement xyz="1 0 0">
              <frame><component-origin component="device">{scope}</component-origin></frame>
              <rotation><rpy value="0 0 0"/></rotation>
            </placement></model>
          </representation></connector>
          <connector name="route"><representation><derived-route>
            <waypoint xyz="1 0 0">
              <frame><component-origin component="device">{scope}</component-origin></frame>
              <rotation><rpy value="0 0 0"/></rotation>
            </waypoint>
            <waypoint xyz="2 0 0">
              <frame><component-origin component="device">{scope}</component-origin></frame>
            </waypoint>
          </derived-route></representation></connector>
          <connector name="arm-outside"><representation><sphere radius="1">
            <placement xyz="1 0 0">
              <frame><component-origin component="arm">{scope}</component-origin></frame>
              <rotation><rpy value="0 0 0"/></rotation>
            </placement>
          </sphere></representation></connector>
        </comp>
        <include uri="module.hcdf" name="child"
                 pose="10 0 0 0 0 1.5707963267948966"/>
        </hcdf>"#
    );
    let mut doc = Hcdf::from_xml_str(&xml).unwrap();
    let original = doc.clone();
    let mut load = mem_loader(HashMap::from([("module.hcdf", module)]));
    hcdformat::flatten_with(&mut doc, Path::new("/p"), &mut load).unwrap();

    let child = sphere_placement(&doc, "child/device", "inside");
    let parent = sphere_placement(&doc, "host", "outside");
    assert_vec3_close(child.xyz, [10.0, 1.0, 0.0]);
    assert_vec3_close(parent.xyz, child.xyz);
    assert_eq!(parent.rotation, child.rotation);

    let model = match &connector_representation(&doc, "host", "model").variant {
        connectivity_xml::RepresentationChoice::Model(value) => &value.placement,
        other => panic!("expected model representation, got {other:?}"),
    };
    assert_vec3_close(model.xyz, child.xyz);
    assert_eq!(model.rotation, child.rotation);

    let route = match &connector_representation(&doc, "host", "route").variant {
        connectivity_xml::RepresentationChoice::DerivedRoute(value) => value,
        other => panic!("expected route representation, got {other:?}"),
    };
    assert_vec3_close(route.waypoint[0].xyz, child.xyz);
    assert_eq!(route.waypoint[0].rotation.as_ref(), Some(&child.rotation));
    assert_vec3_close(route.waypoint[1].xyz, [10.0, 2.0, 0.0]);
    assert!(route.waypoint[1].rotation.is_none());

    let child_arm = sphere_placement(&doc, "child/arm", "inside");
    let parent_arm = sphere_placement(&doc, "host", "arm-outside");
    assert_vec3_close(child_arm.xyz, [1.0, 0.0, 0.0]);
    assert_vec3_close(parent_arm.xyz, child_arm.xyz);
    assert_eq!(parent_arm.rotation, child_arm.rotation);
    assert_ne!(doc, original);
}

#[test]
fn plca_macsec_and_eee_references_prefix_across_repeated_includes() {
    let module = r#"<hcdf name="secured-module" version="1.0">
      <comp name="controller"><port name="p"/></comp>
      <comp name="sensor"><port name="p"/></comp>
      <bus name="control-bus">
        <selected purpose="communication" carrier="electrical"/>
        <configuration>
          <plca max-node-id="7" to-timer-bit-times="32">
            <node id="0" burst-count="1" burst-timer-bit-times="16"><participant-ref network="control-bus" participant="controller"/></node>
            <node id="1" burst-count="0" burst-timer-bit-times="16"><participant-ref network="control-bus" participant="sensor"/></node>
          </plca>
          <macsec>
            <policy name="secure" enforcement="must-secure" cipher="ieee:gcm-aes-128" key-agreement="ieee:mka" confidentiality-offset="0" rekey-interval-ns="1000" credential-store-ref="keystore:control-bus"/>
            <default-policy><macsec-policy-ref network="control-bus" policy="secure"/></default-policy>
            <override><target><network-ref network="control-bus"/></target><macsec-policy-ref network="control-bus" policy="secure"/></override>
          </macsec>
          <eee default-mode="disabled"><override mode="enabled"><participant-ref network="control-bus" participant="sensor"/></override></eee>
        </configuration>
        <participant name="controller"><endpoint><port-ref component="controller" port="p"/></endpoint></participant>
        <participant name="sensor"><endpoint><port-ref component="sensor" port="p"/></endpoint></participant>
      </bus>
    </hcdf>"#;
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="parent" version="1.0">
          <include uri="module.hcdf" name="left"/>
          <include uri="module.hcdf" name="right"/>
        </hcdf>"#,
    )
    .unwrap();
    let mut load = mem_loader(HashMap::from([("module.hcdf", module)]));
    hcdformat::flatten_with(&mut doc, Path::new("/parent"), &mut load).unwrap();

    for prefix in ["left", "right"] {
        let network_name = format!("{prefix}/control-bus");
        let bus = doc.bus.iter().find(|bus| bus.name == network_name).unwrap();
        let configuration = bus.configuration.as_ref().unwrap();
        let plca = configuration.plca.as_ref().unwrap();
        assert!(plca
            .node
            .iter()
            .all(|node| node.participant.network == network_name));
        let macsec = configuration.macsec.as_ref().unwrap();
        assert_eq!(
            macsec.default_policy.as_ref().unwrap().policy.network,
            network_name
        );
        let override_ = &macsec.override_[0];
        assert_eq!(override_.policy.network, network_name);
        match &override_.target.segment {
            connectivity_xml::TopologySegmentChoice::Network(reference) => {
                assert_eq!(reference.network, network_name);
                assert!(reference.instance.is_none());
            }
            other => panic!("expected whole-network MACsec target, got {other:?}"),
        }
        assert_eq!(
            configuration.eee.as_ref().unwrap().override_[0]
                .participant
                .network,
            network_name
        );
    }
    assert!(all_family_errors(&doc).is_empty());
}
