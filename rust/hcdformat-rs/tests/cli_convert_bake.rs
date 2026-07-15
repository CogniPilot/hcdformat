//! CLI-level guarantees for `hcdf convert --bake DIR` (feature `cli`, native-only), driving the REAL
//! binary: the whole-robot bake walk that closes the documented deferral vs `hcdf_cli.py`:
//!
//!   * a URDF visual mesh bakes to a content-addressed GLB in DIR with the source `scale` (magnitude +
//!     mirror) AND the resolved flat `<material>` colour folded in: the bytes are EXACTLY what
//!     `bake_convert` produces for that (mesh, scale, color), proving the importer's
//!     `VisualAssetHint` side-channel is what feeds the bake;
//!   * a SCALED URDF collision bakes to a lean STL with the scale folded into the vertices and the
//!     now-consumed `@scale` CLEARED from the document;
//!   * an UNRESOLVABLE mesh is left as the source reference with a note (never silently dropped);
//!   * the SDF importer's side-channel (`<mesh><scale>` + `<material><diffuse>`) feeds the bake the
//!     same way: the post-side-channel SDF half of the deferral;
//!   * `@uri` is written relative to the OUTPUT document (`assets/<stem>_<short12>.glb` for a bake dir
//!     next to it), matching Python's `Baker.uri_prefix` contract.
#![cfg(feature = "cli")]

use hcdformat::{bake_convert, bake_textured_box, BakeKind, Hcdf, HintTexture, VisualAppearance};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A small watertight OBJ cube `[0,1]^3` (8 corners, 12 triangles), a guaranteed bakeable source.
const CUBE_OBJ: &str = "\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 3 2\nf 1 4 3\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\n\
f 2 3 7\nf 2 7 6\nf 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";

/// A URDF with a scaled+coloured visual mesh, a scaled collision mesh, and an unresolvable visual.
const ROBOT_URDF: &str = r#"<?xml version="1.0"?>
<robot name="baked">
  <material name="shell"><color rgba="0.8 0.2 0.1 1"/></material>
  <link name="base">
    <visual>
      <geometry><mesh filename="meshes/cube.obj" scale="0.001 0.001 0.001"/></geometry>
      <material name="shell"/>
    </visual>
    <collision>
      <geometry><mesh filename="meshes/cube.obj" scale="2 2 2"/></geometry>
    </collision>
    <visual name="ghost">
      <geometry><mesh filename="meshes/missing.stl"/></geometry>
    </visual>
  </link>
</robot>
"#;

/// An SDF with a scaled visual mesh carrying a flat `<diffuse>`: the SDF side-channel inputs.
const ROBOT_SDF: &str = r#"<?xml version="1.0"?>
<sdf version="1.9">
  <model name="baked_sdf">
    <link name="base">
      <visual name="body">
        <geometry><mesh><uri>meshes/cube.obj</uri><scale>0.001 0.001 0.001</scale></mesh></geometry>
        <material><diffuse>0.1 0.4 0.9 1</diffuse></material>
      </visual>
    </link>
  </model>
</sdf>
"#;

/// A fresh per-test temp dir seeded with `meshes/cube.obj` + the robot file; returns its root.
fn fixture_dir(tag: &str, robot_name: &str, robot: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hcdf_cli_bake_{tag}_{}_{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("meshes")).unwrap();
    std::fs::write(dir.join("meshes/cube.obj"), CUBE_OBJ).unwrap();
    std::fs::write(dir.join(robot_name), robot).unwrap();
    dir
}

/// Run the real `hcdf` binary with `args` and return its output (stderr carries the notes).
fn hcdf(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hcdf"))
        .args(args)
        .output()
        .expect("hcdf binary runs")
}

/// The expected output-relative `@uri` + full `@sha` for baking `meshes/cube.obj` with these inputs,
/// recomputed through the SAME library baker the CLI drives, so the assertion pins the exact bytes
/// (scale/mirror/colour folded in) without hard-coding a hash.
fn expected_bake(
    dir: &Path,
    scale: Option<&str>,
    kind: BakeKind,
    color: Option<&str>,
) -> (String, String) {
    let src = dir.join("meshes/cube.obj");
    let asset = bake_convert(&src.to_string_lossy(), scale, kind, color).unwrap();
    let short = asset
        .sha
        .split_once(':')
        .map_or(asset.sha.as_str(), |(_, h)| h);
    (
        format!("assets/cube_{}.{}", &short[..12], asset.ext),
        asset.sha,
    )
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

#[test]
fn urdf_convert_bake_folds_hint_scale_and_color_into_assets() {
    let dir = fixture_dir("urdf", "robot.urdf", ROBOT_URDF);
    let out_doc = dir.join("out/robot.hcdf");
    let bake_dir = dir.join("out/assets");
    std::fs::create_dir_all(out_doc.parent().unwrap()).unwrap();

    let out = hcdf(&[
        "convert",
        dir.join("robot.urdf").to_str().unwrap(),
        out_doc.to_str().unwrap(),
        "--bake",
        bake_dir.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "convert --bake must succeed, got:\n{stderr}"
    );

    let doc = Hcdf::from_xml_str(&std::fs::read_to_string(&out_doc).unwrap()).unwrap();

    // Visual: the GLB in DIR is EXACTLY the library bake of (cube.obj, scale 0.001, shell colour);
    // the hint side-channel fed both, and the doc points at it relative to the output document.
    let (want_uri, want_sha) = expected_bake(
        &dir,
        Some("0.001 0.001 0.001"),
        BakeKind::Visual,
        Some("0.8 0.2 0.1 1"),
    );
    let (uri, sha) = visual_model(&doc, "base", "base_visual_0");
    assert_eq!(
        uri.as_deref(),
        Some(want_uri.as_str()),
        "visual @uri is the baked GLB"
    );
    assert_eq!(
        sha.as_deref(),
        Some(want_sha.as_str()),
        "visual @sha is the baked GLB's sha"
    );
    let glb = bake_dir.join(want_uri.strip_prefix("assets/").unwrap());
    assert!(
        glb.is_file(),
        "baked GLB {} must land in --bake DIR",
        glb.display()
    );
    assert_eq!(
        hcdformat::content_sha(&std::fs::read(&glb).unwrap()),
        want_sha,
        "the GLB on disk carries the bytes the @sha addresses"
    );

    // Collision: scaled -> lean STL with the scale folded in and the consumed @scale CLEARED.
    let (want_uri, want_sha) = expected_bake(&dir, Some("2 2 2"), BakeKind::Collision, None);
    let comp = doc.comp.iter().find(|c| c.name == "base").unwrap();
    let mesh = comp.collision[0]
        .geometry
        .as_ref()
        .unwrap()
        .mesh
        .as_ref()
        .unwrap();
    assert_eq!(
        mesh.uri.as_deref(),
        Some(want_uri.as_str()),
        "collision @uri is the baked STL"
    );
    assert_eq!(
        mesh.sha.as_deref(),
        Some(want_sha.as_str()),
        "collision @sha is the baked STL's sha"
    );
    assert_eq!(
        mesh.scale, None,
        "the baked-in collision scale must be cleared"
    );
    assert!(bake_dir
        .join(want_uri.strip_prefix("assets/").unwrap())
        .is_file());

    // The unresolvable mesh is LEFT as the source reference, with a note on stderr.
    let (uri, sha) = visual_model(&doc, "base", "ghost");
    assert_eq!(
        uri.as_deref(),
        Some("meshes/missing.stl"),
        "unresolvable mesh keeps its source uri"
    );
    assert_eq!(sha, None, "an unbaked mesh gets no @sha");
    assert!(
        stderr.contains("could not be resolved"),
        "expected an unresolved-mesh note, got:\n{stderr}"
    );
}

/// A deterministic 77-byte 2×2 RGB PNG (the same fixture bytes tests/bake.rs embeds).
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xFD, 0xD4, 0x9A,
    0x73, 0x00, 0x00, 0x00, 0x14, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0xF8, 0xCF, 0xC0, 0xC0,
    0x00, 0xC2, 0x0C, 0xFF, 0xFF, 0xFF, 0x67, 0x00, 0x00, 0x1E, 0xEF, 0x04, 0xFC, 0x73, 0x1C, 0x53,
    0xCC, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// An SDF whose only visual is a TEXTURED `<plane>` (the b3rb logo-decal pattern, distilled): the
/// albedo map is a `model://` uri that resolves through `--package`, like a mesh uri would.
const PLANE_SDF: &str = r#"<?xml version="1.0"?>
<sdf version="1.12">
  <model name="decal_bot">
    <link name="base">
      <visual name="logo">
        <geometry><plane><normal>0 0 1</normal><size>0.075 0.075</size></plane></geometry>
        <material>
          <diffuse>1.0 1.0 1.0</diffuse>
          <pbr><metal><albedo_map>model://decal_pkg/textures/logo.png</albedo_map></metal></pbr>
        </material>
      </visual>
    </link>
  </model>
</sdf>
"#;

#[test]
fn sdf_convert_bake_synthesizes_textured_plane_quad() {
    // End-to-end over the REAL binary: the textured <plane> imports as a zero-thickness box + texture
    // hint, and `--bake` synthesizes the UV quad GLB (TEXCOORD_0 + the PNG embedded; the bytes are
    // EXACTLY bake_textured_box's, pinning the whole side-channel) and rewrites the visual to an
    // ordinary <model uri>. The model:// texture uri resolves through --package like a mesh uri.
    let dir = fixture_dir("sdf_plane", "robot.sdf", PLANE_SDF);
    std::fs::create_dir_all(dir.join("pkg/textures")).unwrap();
    std::fs::write(dir.join("pkg/textures/logo.png"), TINY_PNG).unwrap();
    let out_doc = dir.join("out/robot.hcdf");
    let bake_dir = dir.join("out/assets");
    std::fs::create_dir_all(out_doc.parent().unwrap()).unwrap();

    let out = hcdf(&[
        "convert",
        dir.join("robot.sdf").to_str().unwrap(),
        out_doc.to_str().unwrap(),
        "--bake",
        bake_dir.to_str().unwrap(),
        "--package",
        &format!("decal_pkg={}", dir.join("pkg").display()),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "convert --bake must succeed, got:\n{stderr}"
    );

    let doc = Hcdf::from_xml_str(&std::fs::read_to_string(&out_doc).unwrap()).unwrap();
    let expected = bake_textured_box(
        "0.075 0.075 0",
        &HintTexture {
            bytes: TINY_PNG.to_vec(),
            name: "logo.png".to_string(),
        },
    )
    .unwrap();
    let short = expected
        .sha
        .split_once(':')
        .map_or(expected.sha.as_str(), |(_, h)| h);
    let want_uri = format!("assets/logo_{}.glb", &short[..12]);
    let (uri, sha) = visual_model(&doc, "base", "logo");
    assert_eq!(
        uri.as_deref(),
        Some(want_uri.as_str()),
        "the plane visual became the quad GLB model"
    );
    assert_eq!(sha.as_deref(), Some(expected.sha.as_str()));
    let glb =
        std::fs::read(bake_dir.join(format!("logo_{}.glb", &short[..12]))).expect("GLB written");
    assert_eq!(
        glb, expected.data,
        "the on-disk GLB is byte-for-byte the library synthesis"
    );
}

#[test]
fn sdf_convert_bake_notes_texture_on_unsupported_primitive() {
    // A textured SPHERE cannot synthesize: the primitive is KEPT with its flat colour and the drop is
    // a visible note (the honest tier), never a silent loss or a failure.
    const SPHERE_SDF: &str = r#"<sdf version="1.12"><model name="m"><link name="base">
        <visual name="ball"><geometry><sphere><radius>0.1</radius></sphere></geometry>
          <material><diffuse>0.2 0.3 0.4 1</diffuse>
            <pbr><metal><albedo_map>tex/ball.png</albedo_map></metal></pbr></material></visual>
        </link></model></sdf>"#;
    let dir = fixture_dir("sdf_sphere", "robot.sdf", SPHERE_SDF);
    let out_doc = dir.join("out/robot.hcdf");
    std::fs::create_dir_all(out_doc.parent().unwrap()).unwrap();
    let out = hcdf(&[
        "convert",
        dir.join("robot.sdf").to_str().unwrap(),
        out_doc.to_str().unwrap(),
        "--bake",
        dir.join("out/assets").to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "convert --bake must succeed, got:\n{stderr}"
    );
    let doc = Hcdf::from_xml_str(&std::fs::read_to_string(&out_doc).unwrap()).unwrap();
    let v = doc.comp[0]
        .visual
        .iter()
        .find(|v| v.name == "ball")
        .expect("visual kept");
    assert!(
        matches!(&v.appearance, VisualAppearance::Primitive { geometry: Some(g), .. } if g.sphere.is_some()),
        "the sphere primitive stays: {:?}",
        v.appearance
    );
    assert!(
        stderr.contains("not applied (textured-primitive synthesis handles <box>"),
        "the unsupported-shape tier must be noted:\n{stderr}"
    );
}

#[test]
fn sdf_convert_bake_bakes_b3rb_logo_planes() {
    // The REAL target: the b3rb model's NXP_LOGO / CogniPilot_LOGO <plane> visuals (with model://
    // albedo maps) bake to textured quad GLBs embedding NXP.png / CogniPilot.png. Skipped when the
    // cranium checkout is absent.
    let home = std::env::var("HOME").unwrap_or_default();
    let b3rb = PathBuf::from(format!(
        "{home}/cognipilot/cranium/src/b3rb_simulator/b3rb_gz_resource/models/b3rb"
    ));
    if !b3rb.join("model.sdf").is_file() {
        eprintln!(
            "({} not present, skipping the b3rb reference bake)",
            b3rb.display()
        );
        return;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("hcdf_cli_bake_b3rb_{}_{nonce}", std::process::id()));
    let out_doc = dir.join("out/b3rb.hcdf");
    let bake_dir = dir.join("out/assets");
    std::fs::create_dir_all(out_doc.parent().unwrap()).unwrap();

    let out = hcdf(&[
        "convert",
        b3rb.join("model.sdf").to_str().unwrap(),
        out_doc.to_str().unwrap(),
        "--bake",
        bake_dir.to_str().unwrap(),
        "--package",
        &format!("b3rb={}", b3rb.display()),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "b3rb convert --bake must succeed, got:\n{stderr}"
    );

    let doc = Hcdf::from_xml_str(&std::fs::read_to_string(&out_doc).unwrap()).unwrap();
    // (visual, plane <size> as the importer's zero-thickness box, texture file) per logo decal.
    let cases = [
        ("NXP_LOGO", ".075 .075 0", "NXP.png"),
        ("CogniPilot_LOGO", "0.08652 0.04584 0", "CogniPilot.png"),
    ];
    for (visual, box_size, png) in cases {
        let png_path = b3rb.join("materials/textures").join(png);
        let expected = bake_textured_box(
            box_size,
            &HintTexture {
                bytes: std::fs::read(&png_path).unwrap(),
                name: png.to_string(),
            },
        )
        .unwrap();
        let (uri, sha) = visual_model(&doc, "base_link", visual);
        assert_eq!(
            sha.as_deref(),
            Some(expected.sha.as_str()),
            "{visual} must bake to the {png}-textured quad GLB"
        );
        let uri = uri.expect("baked visual has a uri");
        let name = uri.strip_prefix("assets/").unwrap_or(&uri);
        let glb = std::fs::read(bake_dir.join(name)).expect("logo GLB written");
        assert_eq!(
            glb, expected.data,
            "{visual}: on-disk GLB is the library synthesis (texture embedded)"
        );
    }
}

#[test]
fn sdf_convert_bake_folds_the_sdf_side_channel() {
    let dir = fixture_dir("sdf", "robot.sdf", ROBOT_SDF);
    let out_doc = dir.join("out/robot.hcdf");
    let bake_dir = dir.join("out/assets");
    std::fs::create_dir_all(out_doc.parent().unwrap()).unwrap();

    let out = hcdf(&[
        "convert",
        dir.join("robot.sdf").to_str().unwrap(),
        out_doc.to_str().unwrap(),
        "--bake",
        bake_dir.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "SDF convert --bake must succeed, got:\n{stderr}"
    );

    // The GLB is the library bake of (cube.obj, <scale> 0.001, <diffuse> colour); the SDF hint
    // side-channel (`from_sdf_str_with_assets`) is what carried both to the baker.
    let doc = Hcdf::from_xml_str(&std::fs::read_to_string(&out_doc).unwrap()).unwrap();
    let (want_uri, want_sha) = expected_bake(
        &dir,
        Some("0.001 0.001 0.001"),
        BakeKind::Visual,
        Some("0.1 0.4 0.9 1"),
    );
    let (uri, sha) = visual_model(&doc, "base", "body");
    assert_eq!(
        uri.as_deref(),
        Some(want_uri.as_str()),
        "SDF visual @uri is the baked GLB"
    );
    assert_eq!(
        sha.as_deref(),
        Some(want_sha.as_str()),
        "SDF visual @sha is the baked GLB's sha"
    );
    assert!(bake_dir
        .join(want_uri.strip_prefix("assets/").unwrap())
        .is_file());
}
