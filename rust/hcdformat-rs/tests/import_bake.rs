//! Import-then-bake orchestration tests (`src/import_bake.rs`): the whole-document mesh-bake walk the
//! `from_urdf_str_with_baking` / `from_sdf_str_with_baking` entry points compose, plus the deferral-note
//! filtering keyed on the `(comp, visual)` pair.
//!
//! Covers each bake decision branch (visual GLB adopt vs convert, scaled-collision lean vs unscaled
//! hash-in-place), the `(comp, visual)` note-pair contract (ported from the Python `test_baked_notes.py`
//! scenario: two comps with same-named visuals, one bakes, the other's note survives), content-addressed
//! filename stability, and the vendored `@uri`/`@sha` patch-back over a real (synthetic) mesh. Pure Rust,
//! no python3 invoked; meshes are written into per-test temp dirs so the suite is self-contained.
#![cfg(all(
    feature = "bake",
    feature = "urdf",
    feature = "sdf",
    not(target_arch = "wasm32")
))]

use hcdformat::{
    content_sha, drop_baked_notes, from_sdf_str_with_baking, from_urdf_str_with_baking, Hcdf,
    VisualAppearance,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

// ── fixtures ─────────────────────────────────────────────────────────────────────────────────────

/// A small watertight OBJ cube `[0,1]^3` (8 corners, 12 triangles), a guaranteed bakeable source (the
/// same fixture `tests/cli_convert_bake.rs` uses).
const CUBE_OBJ: &str = "\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 3 2\nf 1 4 3\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\n\
f 2 3 7\nf 2 7 6\nf 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";

/// Bytes that pass the GLB magic check (`glTF`) so the visual passthrough (adopt) branch treats them as an
/// already-canonical GLB and copies them verbatim, rather than converting.
const FAKE_GLB: &[u8] = b"glTFdeterministic-adopt-me-verbatim-0001";

/// A fresh, unique temp directory for one test.
fn tmp_dir(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "hcdf_import_bake_{tag}_{}_{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The empty package map (no `package://`/`model://` resolution needed for these fixtures).
fn no_pkgs() -> BTreeMap<String, PathBuf> {
    BTreeMap::new()
}

/// The `(uri, sha)` of the named visual's `<model>`, or a panic naming what is missing.
fn visual_model(doc: &Hcdf, comp: &str, visual: &str) -> (Option<String>, Option<String>) {
    let c = doc
        .comp
        .iter()
        .find(|c| c.name == comp)
        .expect("comp exists");
    let v = c
        .visual
        .iter()
        .find(|v| v.name == visual)
        .expect("visual exists");
    match &v.appearance {
        VisualAppearance::Model { model, .. } => (model.uri.clone(), model.sha.clone()),
        other => panic!("visual {visual:?} is not a <model> appearance: {other:?}"),
    }
}

// ── bake decision branches ─────────────────────────────────────────────────────────────────────────

#[test]
fn visual_glb_source_is_adopted_verbatim() {
    // A visual mesh whose source is already a GLB is passed through byte-for-byte (adopt), NOT re-exported:
    // the vendored @uri keeps the .glb extension and the @sha is the content hash of the source bytes.
    let dir = tmp_dir("adopt");
    std::fs::write(dir.join("chassis.glb"), FAKE_GLB).unwrap();
    let urdf = r#"<?xml version="1.0"?>
<robot name="t">
  <link name="base">
    <visual name="shell"><geometry><mesh filename="chassis.glb"/></geometry></visual>
  </link>
</robot>"#;
    let out = dir.join("assets");
    let (doc, _notes) =
        from_urdf_str_with_baking(urdf, &dir, &out, "assets", &no_pkgs()).expect("import+bake");

    let (uri, sha) = visual_model(&doc, "base", "shell");
    let uri = uri.expect("visual has a baked uri");
    assert!(
        uri.starts_with("assets/") && uri.ends_with(".glb"),
        "adopted GLB @uri: {uri}"
    );
    assert_eq!(
        sha.as_deref(),
        Some(content_sha(FAKE_GLB).as_str()),
        "adopt hashes the source GLB bytes verbatim"
    );
    let asset = out.join(uri.strip_prefix("assets/").unwrap());
    assert_eq!(
        std::fs::read(&asset).unwrap(),
        FAKE_GLB,
        "the adopted GLB is a verbatim byte-copy"
    );
}

#[test]
fn visual_non_glb_source_is_converted_to_glb() {
    // A non-GLB visual mesh is CONVERTED to a canonical GLB with the hint scale folded in: the @uri ends
    // .glb (not .obj) and a matching @sha is stamped.
    let dir = tmp_dir("convert");
    std::fs::write(dir.join("cube.obj"), CUBE_OBJ).unwrap();
    let urdf = r#"<?xml version="1.0"?>
<robot name="t">
  <link name="base">
    <visual name="body"><geometry><mesh filename="cube.obj" scale="2 2 2"/></geometry></visual>
  </link>
</robot>"#;
    let out = dir.join("assets");
    let (doc, _notes) =
        from_urdf_str_with_baking(urdf, &dir, &out, "assets", &no_pkgs()).expect("import+bake");

    let (uri, sha) = visual_model(&doc, "base", "body");
    let uri = uri.expect("visual has a baked uri");
    assert!(
        uri.starts_with("assets/cube_") && uri.ends_with(".glb"),
        "a non-GLB visual converts to a GLB: {uri}"
    );
    assert!(
        sha.unwrap().starts_with("sha256:"),
        "converted visual carries a content sha"
    );
    assert!(
        out.join(uri.strip_prefix("assets/").unwrap()).is_file(),
        "the GLB landed in the asset dir"
    );
}

#[test]
fn scaled_collision_bakes_lean_and_clears_scale() {
    // A SCALED collision mesh bakes to a lean STL with the scale folded into the vertices; the now-consumed
    // @scale is CLEARED (it round-tripped onto the HCDF mesh, and the bake consumed it).
    let dir = tmp_dir("coll_scaled");
    std::fs::write(dir.join("cube.obj"), CUBE_OBJ).unwrap();
    let urdf = r#"<?xml version="1.0"?>
<robot name="t">
  <link name="base">
    <collision name="hull"><geometry><mesh filename="cube.obj" scale="2 2 2"/></geometry></collision>
  </link>
</robot>"#;
    let out = dir.join("assets");
    let (doc, _notes) =
        from_urdf_str_with_baking(urdf, &dir, &out, "assets", &no_pkgs()).expect("import+bake");

    let mesh = doc.comp[0].collision[0]
        .geometry
        .as_ref()
        .unwrap()
        .mesh
        .as_ref()
        .unwrap();
    let uri = mesh.uri.clone().expect("collision has a baked uri");
    assert!(
        uri.starts_with("assets/cube_") && uri.ends_with(".stl"),
        "a scaled collision canonicalizes to a lean STL: {uri}"
    );
    assert!(
        mesh.sha.as_deref().unwrap().starts_with("sha256:"),
        "baked collision carries a sha"
    );
    assert_eq!(
        mesh.scale, None,
        "the baked-in collision scale must be cleared"
    );
    assert!(out.join(uri.strip_prefix("assets/").unwrap()).is_file());
}

#[test]
fn unscaled_collision_is_hashed_in_place() {
    // An UNSCALED collision mesh is passed through verbatim (hashed in place): the source bytes + extension
    // are kept and the @sha is their content hash; the (already-absent) scale stays cleared.
    let dir = tmp_dir("coll_plain");
    std::fs::write(dir.join("cube.obj"), CUBE_OBJ).unwrap();
    let urdf = r#"<?xml version="1.0"?>
<robot name="t">
  <link name="base">
    <collision name="hull"><geometry><mesh filename="cube.obj"/></geometry></collision>
  </link>
</robot>"#;
    let out = dir.join("assets");
    let (doc, _notes) =
        from_urdf_str_with_baking(urdf, &dir, &out, "assets", &no_pkgs()).expect("import+bake");

    let mesh = doc.comp[0].collision[0]
        .geometry
        .as_ref()
        .unwrap()
        .mesh
        .as_ref()
        .unwrap();
    let uri = mesh.uri.clone().expect("collision has a baked uri");
    assert!(
        uri.starts_with("assets/cube_") && uri.ends_with(".obj"),
        "an unscaled collision keeps its source format verbatim: {uri}"
    );
    assert_eq!(
        mesh.sha.as_deref(),
        Some(content_sha(CUBE_OBJ.as_bytes()).as_str()),
        "hash-in-place addresses the source bytes"
    );
    assert_eq!(mesh.scale, None);
    let asset = out.join(uri.strip_prefix("assets/").unwrap());
    assert_eq!(
        std::fs::read(&asset).unwrap(),
        CUBE_OBJ.as_bytes(),
        "verbatim byte-copy"
    );
}

// ── (comp, visual) note-pair semantics ──────────────────────────────────────────────────────────────

#[test]
fn note_pair_keeps_same_named_visual_under_another_comp_end_to_end() {
    // Two links each carry a visual named 'mesh'; only linkA's mesh is present (bakes), linkB's is absent
    // (unresolvable). The deferral note the SDF importer writes for linkA is CONSUMED (dropped), while
    // linkB's same-named-visual note SURVIVES: the (comp, visual) pair keyed drop never cross-drops.
    let dir = tmp_dir("note_pair");
    std::fs::write(dir.join("a.obj"), CUBE_OBJ).unwrap();
    // b.obj is deliberately NOT written.
    let sdf = r#"<?xml version="1.0"?>
<sdf version="1.9">
  <model name="m">
    <link name="linkA">
      <visual name="mesh"><geometry><mesh><uri>a.obj</uri><scale>2 2 2</scale></mesh></geometry></visual>
    </link>
    <link name="linkB">
      <visual name="mesh"><geometry><mesh><uri>b.obj</uri><scale>2 2 2</scale></mesh></geometry></visual>
    </link>
  </model>
</sdf>"#;
    let out = dir.join("assets");
    let (doc, notes) =
        from_sdf_str_with_baking(sdf, &dir, &out, "assets", &no_pkgs()).expect("import+bake");

    // linkA baked (its visual became a GLB model uri), linkB left as the source reference.
    let (a_uri, _) = visual_model(&doc, "linkA", "mesh");
    assert!(
        a_uri.as_deref().unwrap().starts_with("assets/"),
        "linkA baked to an asset uri"
    );
    let (b_uri, b_sha) = visual_model(&doc, "linkB", "mesh");
    assert_eq!(
        b_uri.as_deref(),
        Some("b.obj"),
        "linkB's unresolvable mesh keeps its source uri"
    );
    assert_eq!(b_sha, None, "an unbaked mesh gets no @sha");

    let note_a = "link 'linkA' visual 'mesh': mesh <scale> '2 2 2' not applied \
                  (bake the GLB with the meshes present to fold it in)";
    let note_b = "link 'linkB' visual 'mesh': mesh <scale> '2 2 2' not applied \
                  (bake the GLB with the meshes present to fold it in)";
    assert!(
        !notes.iter().any(|n| n == note_a),
        "the baked comp's deferral note is consumed:\n{notes:#?}"
    );
    assert!(
        notes.iter().any(|n| n == note_b),
        "the unbaked comp's same-named visual keeps its note:\n{notes:#?}"
    );
}

#[test]
fn drop_baked_notes_comp_qualified_pair_is_not_cross_dropped() {
    // The SDF wording embeds the comp: only the baked (comp, visual) pair's note drops; a same-named visual
    // under another comp survives (the exact `test_baked_notes.py` scenario, at the unit level).
    let note_a = "link 'linkA' visual 'mesh': mesh <scale> '2 2 2' not applied \
                  (bake the GLB with the meshes present to fold it in)"
        .to_string();
    let note_b = "link 'linkB' visual 'mesh': mesh <scale> '2 2 2' not applied \
                  (bake the GLB with the meshes present to fold it in)"
        .to_string();
    let baked: BTreeSet<(String, String)> = [("linkA".to_string(), "mesh".to_string())]
        .into_iter()
        .collect();
    let kept = drop_baked_notes(vec![note_a.clone(), note_b.clone()], &baked);
    assert!(!kept.contains(&note_a), "baked comp's note consumed");
    assert!(
        kept.contains(&note_b),
        "unbaked comp's same-named visual keeps its note"
    );
}

#[test]
fn drop_baked_notes_comp_less_urdf_note_drops_by_visual_name() {
    // The URDF importer omits the comp from the note; a baked visual's comp-less note is still dropped by
    // visual name alone, preserving that importer's behavior.
    let note = "visual 'wheel': mesh scale '2 2 2' not applied \
                (bake the GLB with the meshes present to fold it in)"
        .to_string();
    let baked: BTreeSet<(String, String)> = [("base".to_string(), "wheel".to_string())]
        .into_iter()
        .collect();
    assert_eq!(drop_baked_notes(vec![note], &baked), Vec::<String>::new());
}

#[test]
fn drop_baked_notes_is_a_noop_when_nothing_baked() {
    let note = "link 'linkB' visual 'mesh': mesh <scale> '2 2 2' not applied \
                (bake the GLB with the meshes present to fold it in)"
        .to_string();
    let baked: BTreeSet<(String, String)> = BTreeSet::new();
    assert_eq!(drop_baked_notes(vec![note.clone()], &baked), vec![note]);
}

// ── filename stability + patch-back ─────────────────────────────────────────────────────────────────

#[test]
fn content_addressed_filename_is_stable_across_runs() {
    // Baking the same mesh with the same inputs is deterministic: the content-addressed filename and @sha
    // are identical across two independent runs (into two separate asset dirs).
    let dir = tmp_dir("stable");
    std::fs::write(dir.join("cube.obj"), CUBE_OBJ).unwrap();
    let urdf = r#"<?xml version="1.0"?>
<robot name="t">
  <link name="base">
    <visual name="body"><geometry><mesh filename="cube.obj" scale="0.5 0.5 0.5"/></geometry></visual>
  </link>
</robot>"#;
    let (doc1, _) =
        from_urdf_str_with_baking(urdf, &dir, &dir.join("out1"), "assets", &no_pkgs()).unwrap();
    let (doc2, _) =
        from_urdf_str_with_baking(urdf, &dir, &dir.join("out2"), "assets", &no_pkgs()).unwrap();
    assert_eq!(
        visual_model(&doc1, "base", "body"),
        visual_model(&doc2, "base", "body"),
        "the content-addressed (uri, sha) is stable across runs"
    );
}

#[test]
fn vendored_uri_and_hash_are_patched_back_and_match_the_written_asset() {
    // The patch-back contract over a real mesh: after import+bake the document's visual and collision
    // references point at the baked assets, each @sha equals the content hash of the bytes actually written
    // to the asset dir, and each asset file exists at its @uri-relative path.
    let dir = tmp_dir("patchback");
    std::fs::write(dir.join("cube.obj"), CUBE_OBJ).unwrap();
    let urdf = r#"<?xml version="1.0"?>
<robot name="t">
  <link name="base">
    <visual name="body"><geometry><mesh filename="cube.obj" scale="2 2 2"/></geometry></visual>
    <collision name="hull"><geometry><mesh filename="cube.obj" scale="2 2 2"/></geometry></collision>
  </link>
</robot>"#;
    let out = dir.join("assets");
    let (doc, _notes) =
        from_urdf_str_with_baking(urdf, &dir, &out, "assets", &no_pkgs()).expect("import+bake");

    // Visual patch-back: @uri rewritten under the prefix, @sha addresses the on-disk asset bytes.
    let (v_uri, v_sha) = visual_model(&doc, "base", "body");
    let v_uri = v_uri.expect("visual uri patched back");
    let v_sha = v_sha.expect("visual sha patched back");
    let v_asset = out.join(
        v_uri
            .strip_prefix("assets/")
            .expect("visual uri carries the prefix"),
    );
    assert!(
        v_asset.is_file(),
        "the visual asset file exists: {}",
        v_asset.display()
    );
    assert_eq!(
        content_sha(&std::fs::read(&v_asset).unwrap()),
        v_sha,
        "the visual @sha addresses the exact bytes written to the asset dir"
    );

    // Collision patch-back: same contract, and the source scale is gone (folded into the lean STL).
    let mesh = doc.comp[0].collision[0]
        .geometry
        .as_ref()
        .unwrap()
        .mesh
        .as_ref()
        .unwrap();
    let c_uri = mesh.uri.clone().expect("collision uri patched back");
    let c_sha = mesh.sha.clone().expect("collision sha patched back");
    let c_asset = out.join(
        c_uri
            .strip_prefix("assets/")
            .expect("collision uri carries the prefix"),
    );
    assert!(
        c_asset.is_file(),
        "the collision asset file exists: {}",
        c_asset.display()
    );
    assert_eq!(
        content_sha(&std::fs::read(&c_asset).unwrap()),
        c_sha,
        "the collision @sha addresses the exact bytes written to the asset dir"
    );
    assert_eq!(
        mesh.scale, None,
        "the folded-in collision scale is cleared on patch-back"
    );
}
