//! Asset-baker determinism + frozen-output tests (`src/bake.rs`).
//!
//! Two bars:
//!
//!   1. **SELF-DETERMINISM**: baking the same input twice yields BIT-IDENTICAL bytes (hence the same
//!      `content_sha`). Pure-Rust, needs no Python; covers STL / OBJ / DAE inputs. This is the hard gate
//!      that lets the bake `@sha` be a stable content address.
//!   2. **FROZEN BAKE OUTPUT**: because the Rust baker is deterministic + content-addressed, its exact
//!      baked bytes are pinned to a committed content-signature golden (`len` + FNV-1a-64), the frozen
//!      replacement for the removed trimesh geometry-equivalence oracle. These run over the external mesh
//!      corpus and are SKIPPED (printed note) when it is absent, so the suite stays green offline. No
//!      python3 is invoked anywhere in this file. Regenerate goldens with `HCDF_REGEN_GOLDENS=1`.
//!
//! Plus a GLB-passthrough byte-identity check on the vendor path (an already-canonical GLB visual with
//! no scale/colour is a verbatim copy: the only place bake `@sha` IS byte-parity, and the invariant
//! the bundle byte-parity test relies on).

#![cfg(feature = "bake")]

use hcdformat::bake::{self, BakeKind};
use std::path::{Path, PathBuf};

// ── corpus + frozen-golden helpers (graceful skip on absent corpus) ──────────────────────────────

/// The external real-robot corpus (`~/git/hcdf-conversion-examples`), or `None` if not checked out.
fn corpus_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join("git/hcdf-conversion-examples");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// First existing path among `candidates` (relative to the corpus root), or `None` to skip.
fn corpus_file(candidates: &[&str]) -> Option<PathBuf> {
    let root = corpus_root()?;
    candidates
        .iter()
        .map(|c| root.join(c))
        .find(|p| p.is_file())
}

/// The crate-local frozen-golden directory (`<crate>/tests/golden`).
fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Out-of-repo staging directory for ad-hoc REAL-WORLD reference files the suite exercises when
/// present (the COLLADA-1.5 pelvis, the Digit torso, …): `$HCDF_STAGING_DIR`, defaulting to
/// `~/hcdf-test-staging`. Nothing here is vendored; every consumer skips cleanly when its file is
/// absent, so an unstaged machine loses only the real-file spot checks.
fn staging_dir() -> PathBuf {
    std::env::var_os("HCDF_STAGING_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join("hcdf-test-staging")
        })
}

/// Compare `actual` to the committed golden at `tests/golden/<rel>`; when `HCDF_REGEN_GOLDENS` is set,
/// (re)write the golden instead of asserting. The Rust baker is deterministic + content-addressed (the
/// self-determinism gate proves bit-identical bakes), so its output freezes cleanly.
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

/// A stable content signature for baked bytes: the byte length + an FNV-1a-64 digest (dependency-free,
/// deterministic). The Rust baker produces bit-identical bytes for a given input, so this pins the exact
/// baked output: the frozen replacement for the removed trimesh geometry-equivalence oracle.
fn bake_signature(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("len={} fnv1a64={h:016x}\n", data.len())
}

// ── temp dir ──────────────────────────────────────────────────────────────────────────────────────

fn tmp_dir(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "hcdf-bake-test-{tag}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A small, valid binary-STL cube written to `path` (so the OBJ/STL tests have a guaranteed input even
/// without the external corpus). Returns nothing; the bytes are a unit cube `[0,1]^3`.
fn write_cube_obj(path: &Path) {
    // 8 corners, 12 triangles (2 per face): a watertight unit cube.
    let obj = "\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 3 2\nf 1 4 3\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\n\
f 2 3 7\nf 2 7 6\nf 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";
    std::fs::write(path, obj).unwrap();
}

// ── 1. SELF-DETERMINISM (no Python needed) ─────────────────────────────────────────────────────────

#[test]
fn determinism_obj_glb_and_lean() {
    let d = tmp_dir("det-obj");
    let obj = d.join("cube.obj");
    write_cube_obj(&obj);
    let p = obj.to_string_lossy();

    // GLB bake twice -> identical bytes + sha.
    let a = bake::to_glb(&p, None, None).unwrap();
    let b = bake::to_glb(&p, None, None).unwrap();
    assert_eq!(a, b, "OBJ->GLB must be byte-deterministic across runs");

    // With a mirror scale + colour, still deterministic.
    let a2 = bake::to_glb(&p, Some("1 -1 2"), Some("0.8 0.2 0.1")).unwrap();
    let b2 = bake::to_glb(&p, Some("1 -1 2"), Some("0.8 0.2 0.1")).unwrap();
    assert_eq!(a2, b2, "OBJ->GLB (mirror+color) must be byte-deterministic");

    // Lean STL bake twice -> identical.
    let la = bake::to_lean(&p, Some("2 3 4")).unwrap();
    let lb = bake::to_lean(&p, Some("2 3 4")).unwrap();
    assert_eq!(la, lb, "OBJ->lean STL must be byte-deterministic");

    // The convenience wrapper recomputes the same sha each time.
    let s1 = bake::bake_convert(&p, None, BakeKind::Visual, None)
        .unwrap()
        .sha;
    let s2 = bake::bake_convert(&p, None, BakeKind::Visual, None)
        .unwrap()
        .sha;
    assert_eq!(s1, s2, "bake_convert sha is a stable content address");

    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn determinism_stl_input() {
    // Use a corpus STL when present; else skip (the OBJ test already proves the writer is deterministic).
    let Some(stl) = corpus_file(&[
        "so_arm101/assets/moving_jaw_so101_v1_785a9dded2f4.stl",
        "openarm/assets/link1_5d53fe21d008.stl",
    ]) else {
        eprintln!("(no corpus STL, skipping STL determinism)");
        return;
    };
    let p = stl.to_string_lossy();
    let a = bake::to_glb(&p, None, None).unwrap();
    let b = bake::to_glb(&p, None, None).unwrap();
    assert_eq!(a, b, "STL->GLB byte-deterministic");
    let la = bake::to_lean(&p, Some("1 -1 1")).unwrap();
    let lb = bake::to_lean(&p, Some("1 -1 1")).unwrap();
    assert_eq!(la, lb, "STL->lean(mirror) byte-deterministic");
}

#[test]
#[cfg(feature = "bake-dae")]
fn determinism_dae_input() {
    let Some(dae) = corpus_file(&[
        "openarm_description/assets/robot/openarm_v2.0/meshes/arm/visual/base_link.dae",
        "openarm_description/assets/robot/openarm_v1.0/mesh/arm/visual/link0.dae",
    ]) else {
        eprintln!("(no corpus DAE, skipping DAE determinism)");
        return;
    };
    let p = dae.to_string_lossy();
    let a = bake::to_glb(&p, None, None).unwrap();
    let b = bake::to_glb(&p, None, None).unwrap();
    assert_eq!(a, b, "DAE->GLB byte-deterministic");
}

// ── 2. DAE GATE (no Python needed) ──────────────────────────────────────────────────────────────────

#[test]
#[cfg(not(feature = "bake-dae"))]
fn dae_gated_off_is_a_clear_error_not_a_wrong_bake() {
    // Without `bake-dae`, a .dae must be a LOUD DaeUnsupported error, never a silently-wrong bake.
    let d = tmp_dir("dae-gate");
    let dae = d.join("x.dae");
    std::fs::write(&dae, b"<COLLADA/>").unwrap();
    let err = bake::to_glb(&dae.to_string_lossy(), None, None).unwrap_err();
    assert!(
        matches!(err, bake::BakeError::DaeUnsupported { .. }),
        "DAE without bake-dae must be DaeUnsupported, got {err:?}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn glb_source_is_rejected_by_convert_not_misconverted() {
    // to_glb/to_lean are CONVERSION entry points; an already-GLB visual is a verbatim passthrough
    // (assets_vendor::bake), so handing one here is AlreadyGlb, never a re-export.
    let d = tmp_dir("glb-reject");
    let glb = d.join("already.glb");
    std::fs::write(&glb, b"glTF....").unwrap();
    assert!(matches!(
        bake::to_glb(&glb.to_string_lossy(), None, None).unwrap_err(),
        bake::BakeError::AlreadyGlb { .. }
    ));
    let _ = std::fs::remove_dir_all(&d);
}

// ── 3. GLB PASSTHROUGH BYTE-IDENTITY (the bundle byte-parity invariant) ─────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn glb_visual_passthrough_is_verbatim_copy() {
    // The vendor passthrough (assets_vendor::bake, NOT the baker) copies an already-GLB visual verbatim
    // by reading the source bytes, content-addressing, and writing under <stem>_<sha>.<ext>. This is the
    // byte-parity path the bundle test relies on; assert the written bytes equal the source bytes exactly.
    // (The payload starts with the GLB MAGIC: the passthrough is content-checked, and a magic-less visual is
    // treated as a `.gltf` to pack, see tests/gltf_pack.rs.)
    use hcdformat::assets_vendor::{self, AssetKind};
    let d = tmp_dir("glb-pass");
    let out = d.join("assets");
    let src = d.join("Chassis.GLB");
    let payload = b"glTF-CANONICAL-VISUAL-bytes-passthrough-unchanged-0001";
    std::fs::write(&src, payload).unwrap();
    let baked = assets_vendor::bake(&src, AssetKind::Visual, &out).unwrap();
    let written = std::fs::read(out.join(&baked.name)).unwrap();
    assert_eq!(
        written, payload,
        "GLB visual is a verbatim passthrough (byte-identical to source)"
    );
    assert_eq!(
        baked.sha,
        hcdformat::content_sha(payload),
        "passthrough sha = sha(source bytes)"
    );
    let _ = std::fs::remove_dir_all(&d);
}

// ── 4. FROZEN BAKE-OUTPUT GOLDENS (corpus; skipped when the external corpus is absent) ────────────
//
// The Rust baker is deterministic + content-addressed (the self-determinism gate above proves
// bit-identical bakes), so these pin the EXACT baked bytes via a content signature: the frozen
// replacement for the removed trimesh geometry-equivalence oracle. Gated on the external mesh corpus.

#[test]
fn stl_to_glb_matches_frozen_golden() {
    let Some(stl) = corpus_file(&["so_arm101/assets/moving_jaw_so101_v1_785a9dded2f4.stl"]) else {
        eprintln!("(no corpus STL, skipping STL->GLB bake golden)");
        return;
    };
    let p = stl.to_string_lossy();
    // Plain, non-uniform scale, and MIRROR: each baked GLB's bytes are frozen.
    for (tag, scale) in [("plain", ""), ("scale", "2 3 4"), ("mirror", "1 -1 1")] {
        let data =
            bake::to_glb(&p, if scale.is_empty() { None } else { Some(scale) }, None).unwrap();
        assert_golden(
            &format!("bake/moving-jaw.{tag}.glb.sig"),
            &bake_signature(&data),
        );
    }
}

#[test]
fn stl_color_bare_mesh_matches_frozen_golden() {
    let Some(stl) = corpus_file(&["so_arm101/assets/moving_jaw_so101_v1_785a9dded2f4.stl"]) else {
        eprintln!("(no corpus STL, skipping color-bake golden)");
        return;
    };
    // A bare STL gets the flat colour baked onto the material; the baked GLB bytes are frozen.
    let data = bake::to_glb(&stl.to_string_lossy(), None, Some("0.8 0.2 0.1 1.0")).unwrap();
    assert_golden("bake/moving-jaw.color.glb.sig", &bake_signature(&data));
}

/// Extract the GLB's JSON chunk as a UTF-8 string (the chunk after the 12-byte header + 8-byte chunk
/// header). Pure-Rust, no Python; used by the structural (attribute/material) assertions below.
fn glb_json(data: &[u8]) -> String {
    assert_eq!(&data[0..4], b"glTF", "not a GLB");
    let jlen = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
    String::from_utf8_lossy(&data[20..20 + jlen]).into_owned()
}

#[test]
fn color_basecolorfactor_uses_bankers_rounding() {
    // Regression guard: the channel 2.5/255 lands EXACTLY on a banker's-rounding boundary,
    // round-half-to-even -> 2 (stored 2/255); the OLD round-half-away code produced 3/255. This is a
    // pure-Rust property (no oracle needed): the baked baseColorFactor must quantize to byte 2, not 3.
    let Some(stl) = corpus_file(&["so_arm101/assets/moving_jaw_so101_v1_785a9dded2f4.stl"]) else {
        eprintln!("(no corpus STL, skipping color-factor rounding)");
        return;
    };
    // 2.5/255 = 0.00980392156862745 is an EXACT f64 tie (f64*255 == 2.5); round-half-to-even -> 2.
    let color = "0.00980392156862745 0.5 0.9 1.0";
    let data = bake::to_glb(&stl.to_string_lossy(), None, Some(color)).unwrap();
    let json = glb_json(&data);
    // Parse the first baseColorFactor channel back out and assert it quantizes to byte 2, NOT 3.
    let bcf = json
        .split("\"baseColorFactor\":[")
        .nth(1)
        .expect("a baseColorFactor in the GLB JSON");
    let ch0: f64 = bcf
        .split(',')
        .next()
        .unwrap()
        .parse()
        .expect("first colour channel parses");
    let byte0 = (ch0 * 255.0).round() as i32;
    assert_eq!(
        byte0, 2,
        "baked channel must be 2/255 (banker's rounding of 2.5), got byte {byte0} (={ch0}); \
         round-half-away would have given 3, the bug this guards"
    );
    // And the stored factor is 2/255 (the GLB stores it as an f32, so compare at f32 tolerance, well
    // inside the ~0.0039 gap to the 3/255 the round-half-away bug would have produced).
    assert!(
        (ch0 - 2.0 / 255.0).abs() < 1e-6,
        "baked baseColorFactor[0] must be 2/255 for the 2.5 boundary, got {ch0}"
    );
}

#[test]
fn dae_glb_carries_normal_through_scale_and_mirror() {
    // A smooth-shaded .dae authors per-vertex normals; the baker must carry POSITION+NORMAL so smooth
    // shading survives (else it is silently lost, invisible to the geometry-only comparator). Unlike
    // trimesh (which drops NORMAL on any scale, leaving a mirrored twin flat-shaded), a scaled OR
    // mirrored bake keeps NORMAL, re-oriented by the normal matrix, so the openarm left arm shades
    // like the right instead of shipping POSITION-only. See `normal_matrix_*` for the transform math.
    let Some(dae) = corpus_file(&[
        "openarm_description/assets/robot/openarm_v2.0/meshes/arm/visual/base_link.dae",
        "openarm_description/assets/robot/openarm_v1.0/mesh/arm/visual/link0.dae",
    ]) else {
        eprintln!("(no corpus DAE, skipping NORMAL parity)");
        return;
    };
    if cfg!(not(feature = "bake-dae")) {
        // Without bake-dae the .dae is a clear error (covered elsewhere); nothing to assert here.
        eprintln!("(bake-dae off, DAE NORMAL parity needs --features bake-dae)");
        return;
    }
    let p = dae.to_string_lossy();
    // Every case (no scale, a non-uniform scale, and a mirror) must carry POSITION+NORMAL now.
    for (tag, scale) in [
        ("plain", None),
        ("scale", Some("2 3 4")),
        ("mirror", Some("1 -1 1")),
    ] {
        let data = bake::to_glb(&p, scale, None).unwrap();
        let j = glb_json(&data);
        assert!(
            j.contains("\"POSITION\":"),
            "{tag} DAE GLB must carry POSITION"
        );
        assert!(
            j.contains("\"NORMAL\":"),
            "{tag} DAE GLB must carry NORMAL (smooth shading preserved through scale/mirror)"
        );
    }
}

#[test]
fn obj_collision_is_intentionally_canonicalized_to_stl() {
    // Regression guard: Python keeps a collision's SOURCE format (an OBJ collision stays .obj
    // text); the Rust conversion path DELIBERATELY canonicalizes every scaled collision to binary STL.
    // This test pins that documented decision so a future change to the contract is caught, and proves
    // the two outputs genuinely differ in format (binary STL header vs OBJ text), as the module docs say.
    let d = tmp_dir("obj-coll");
    let obj = d.join("cube.obj");
    write_cube_obj(&obj);
    let p = obj.to_string_lossy();

    // Rust: bake_convert reports ext="stl" and writes binary STL (80-byte header + u32 count, not OBJ text).
    let baked = bake::bake_convert(&p, Some("2 2 2"), BakeKind::Collision, None).unwrap();
    assert_eq!(
        baked.ext, "stl",
        "Rust canonicalizes a converted collision to STL ext"
    );
    assert!(
        !baked.data.starts_with(b"v ")
            && !baked.data.starts_with(b"# ")
            && !baked.data.starts_with(b"o "),
        "Rust collision output is BINARY STL, not OBJ text"
    );
    // Binary STL: header(80) + u32 count + count*50 bytes.
    assert!(
        baked.data.len() >= 84,
        "binary STL has at least the 84-byte preamble"
    );
    let tri_count = u32::from_le_bytes([
        baked.data[80],
        baked.data[81],
        baked.data[82],
        baked.data[83],
    ]);
    assert_eq!(
        baked.data.len(),
        84 + tri_count as usize * 50,
        "binary STL length = 84 + 50*triangles"
    );
    assert_eq!(tri_count, 12, "the unit cube is 12 triangles");
    let _ = std::fs::remove_dir_all(&d);
}

// ── OBJ `.mtl` materials (companion resolver; the DigitRobot all-gray regression) ─────────────────

/// Every material's `baseColorFactor` from a baked GLB's JSON chunk, in document order.
fn glb_base_colors(json: &str) -> Vec<[f32; 4]> {
    let key = "\"baseColorFactor\":[";
    let mut out = Vec::new();
    let mut rest = json;
    while let Some(p) = rest.find(key) {
        rest = &rest[p + key.len()..];
        let end = rest.find(']').unwrap();
        let v: Vec<f32> = rest[..end]
            .split(',')
            .map(|t| t.trim().parse().expect("colour channel"))
            .collect();
        assert_eq!(v.len(), 4, "baseColorFactor is RGBA");
        out.push([v[0], v[1], v[2], v[3]]);
    }
    out
}

/// A unit cube whose 12 faces are split into two `usemtl` groups (6 faces each) bound to `cube.mtl`.
const TWO_MAT_OBJ: &str = "\
mtllib cube.mtl\n\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
usemtl red\n\
f 1 3 2\nf 1 4 3\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\n\
usemtl blue\n\
f 2 3 7\nf 2 7 6\nf 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";

/// Two materials with distinct `Kd`; `blue` also carries `Ns 96` (glossiness 96/128 → roughness 0.25).
const TWO_MAT_MTL: &str = "\
newmtl red\nKd 1 0 0\n\
newmtl blue\nKd 0 0 1\nNs 96\n";

#[test]
fn obj_mtl_materials_apply_per_usemtl_group() {
    // The resolver is asked for the OBJ-relative `mtllib` companion (the pack_gltf_to_glb contract),
    // and each `usemtl` group becomes its own GLB primitive carrying its `.mtl` colour.
    let mut asked: Vec<String> = Vec::new();
    let asset = bake::bake_convert_bytes_with(
        TWO_MAT_OBJ.as_bytes(),
        "cube.obj",
        None,
        BakeKind::Visual,
        None,
        |uri| {
            asked.push(uri.to_string());
            (uri == "cube.mtl").then(|| TWO_MAT_MTL.as_bytes().to_vec())
        },
    )
    .unwrap();
    assert_eq!(
        asked,
        ["cube.mtl"],
        "resolver sees the mtllib entry exactly as authored"
    );
    assert!(
        asset.notes.is_empty(),
        "nothing skipped -> no notes, got {:?}",
        asset.notes
    );

    let json = glb_json(&asset.data);
    assert_eq!(
        json.matches("\"material\":").count(),
        2,
        "two usemtl groups -> two material-bound primitives: {json}"
    );
    assert_eq!(
        glb_base_colors(&json),
        vec![[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]],
        "each group carries its own Kd"
    );
    // Ns maps through the SAME shininess normalization as the DAE path: blue's Ns 96 -> glossiness
    // 96/128 = 0.75 -> roughness 0.25; red has no Ns -> fully matte. Metallic is always 0 (dielectric).
    assert!(
        json.contains("\"roughnessFactor\":0.25"),
        "Ns 96 -> roughness 0.25: {json}"
    );
    assert!(
        json.contains("\"roughnessFactor\":1"),
        "no Ns -> matte: {json}"
    );
    assert!(
        !json.contains("emissive"),
        "writer stays at the DAE-output schema (no emissive)"
    );

    // Same input + same resolver bytes -> same sha (the resolver is part of the deterministic input).
    let again = bake::bake_convert_bytes_with(
        TWO_MAT_OBJ.as_bytes(),
        "cube.obj",
        None,
        BakeKind::Visual,
        None,
        |_| Some(TWO_MAT_MTL.as_bytes().to_vec()),
    )
    .unwrap();
    assert_eq!(
        asset.sha, again.sha,
        "mtl-resolved OBJ bake is deterministic"
    );
}

#[test]
fn obj_missing_mtl_bakes_gray_with_note() {
    // No resolver hit -> the pre-resolver behaviour (material-less, renders gray), now EXPLICIT via a
    // note naming the unresolved library. bake_convert_bytes (no resolver) is the same bake + note.
    let asset = bake::bake_convert_bytes_with(
        TWO_MAT_OBJ.as_bytes(),
        "cube.obj",
        None,
        BakeKind::Visual,
        None,
        |_| None,
    )
    .unwrap();
    let json = glb_json(&asset.data);
    assert!(
        !json.contains("\"materials\""),
        "unresolved mtllib -> material-less GLB (gray): {json}"
    );
    assert_eq!(
        asset.notes.len(),
        1,
        "one unresolved library -> one note: {:?}",
        asset.notes
    );
    assert!(
        asset.notes[0].contains("cube.mtl"),
        "note names the library: {:?}",
        asset.notes
    );
    let plain = bake::bake_convert_bytes(
        TWO_MAT_OBJ.as_bytes(),
        "cube.obj",
        None,
        BakeKind::Visual,
        None,
    )
    .unwrap();
    assert_eq!(
        plain, asset,
        "the resolver-less entry is the |_| None case of _with"
    );
}

#[test]
fn obj_urdf_color_hint_composes_like_dae() {
    // A unit cube with a BARE leading group (faces before any usemtl) then the red/blue groups: the
    // URDF <material> colour hint must colour ONLY the bare group; a group bound to a resolved .mtl
    // material keeps its own colour (Python's TextureVisuals guard, exactly as the DAE path composes).
    const HINT_OBJ: &str = "\
mtllib cube.mtl\n\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 3 2\nf 1 4 3\nf 5 6 7\nf 5 7 8\n\
usemtl red\n\
f 1 2 6\nf 1 6 5\nf 2 3 7\nf 2 7 6\n\
usemtl blue\n\
f 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";
    let hinted = bake::bake_convert_bytes_with(
        HINT_OBJ.as_bytes(),
        "cube.obj",
        None,
        BakeKind::Visual,
        Some("0 1 0"),
        |_| Some(TWO_MAT_MTL.as_bytes().to_vec()),
    )
    .unwrap();
    assert_eq!(
        glb_base_colors(&glb_json(&hinted.data)),
        vec![
            [0.0, 1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0, 1.0]
        ],
        "hint colours the bare group; resolved .mtl materials win over the hint"
    );

    // With the library UNRESOLVED every group is bare, so the hint colours them all (one deduplicated
    // material): the hint flow of a plain mtl-less OBJ, unchanged by the resolver plumbing.
    let unresolved = bake::bake_convert_bytes_with(
        HINT_OBJ.as_bytes(),
        "cube.obj",
        None,
        BakeKind::Visual,
        Some("0 1 0"),
        |_| None,
    )
    .unwrap();
    assert_eq!(
        glb_base_colors(&glb_json(&unresolved.data)),
        vec![[0.0, 1.0, 0.0, 1.0]],
        "unresolved mtl -> every group takes the hint (deduplicated to one material)"
    );
}

#[test]
fn bake_obj_mtl_digit_torso_reference() {
    // The real-world all-gray regression: DigitRobot.jl's torso.obj (8 usemtl groups; its .mtl mixes
    // flat Kd colours with map_Kd textures). Staged OUTSIDE the repo; skipped when absent.
    //
    // NOTE on textures: the DigitRobot OBJ meshes reference `map_Kd skin.png` but author NO `vt`
    // texture coordinates at all (checked against upstream), so the texture CANNOT bind, even with
    // skin.png staged next to the OBJ the bake must keep the flat Kd, note WHY, and emit no
    // TEXCOORD_0 (the graceful no-UV path; the distilled fixtures cover the actual embedding).
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let candidates = [
        staging_dir().join("digit/torso.obj"),
        home.join("git/DigitRobot.jl/urdf/torso.obj"),
    ];
    let Some(obj) = candidates.iter().find(|p| p.is_file()) else {
        eprintln!("(no DigitRobot torso.obj staged, skipping the real-file .mtl check)");
        return;
    };
    let asset = bake::bake_convert(&obj.to_string_lossy(), None, BakeKind::Visual, None).unwrap();
    let json = glb_json(&asset.data);
    let colors = glb_base_colors(&json);
    assert!(
        colors.len() > 1,
        "expected multiple .mtl material groups, got {colors:?}"
    );
    // A clearly non-default .mtl colour must be carried (the torso's teal, Kd 0 0.56 0.58).
    assert!(
        colors
            .iter()
            .any(|c| c[0] < 1e-3 && (c[1] - 0.56).abs() < 1e-3 && (c[2] - 0.58).abs() < 1e-3),
        "the torso's teal Kd was not carried: {colors:?}"
    );
    // The un-embeddable map_Kd is a flat-colour fallback: NOTED, never silent, and never a phantom
    // TEXCOORD_0 (the note names the file; with skin.png staged the reason is the missing vt lines,
    // without it the unresolved image, both name skin.png).
    assert!(
        asset.notes.iter().any(|n| n.contains("skin.png")),
        "skipped texture must be noted: {:?}",
        asset.notes
    );
    assert!(
        !json.contains("TEXCOORD_0") && !json.contains("\"images\""),
        "a vt-less OBJ must not grow texture structures"
    );
    if obj.with_file_name("skin.png").is_file() {
        assert!(
            asset
                .notes
                .iter()
                .any(|n| n.contains("no texture coordinates")),
            "with skin.png resolvable the note must name the real blocker (no vt): {:?}",
            asset.notes
        );
    }
}

// ── embedded textures (baseColorTexture + TEXCOORD_0; strictly additive) ─────────────────────────

/// A deterministic 77-byte 2×2 RGB PNG (generated once with zlib level 9): the "tiny 4-px PNG" the
/// distilled texture fixtures embed. The writer must pass these bytes through VERBATIM.
const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xFD, 0xD4, 0x9A,
    0x73, 0x00, 0x00, 0x00, 0x14, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0xF8, 0xCF, 0xC0, 0xC0,
    0x00, 0xC2, 0x0C, 0xFF, 0xFF, 0xFF, 0x67, 0x00, 0x00, 0x1E, 0xEF, 0x04, 0xFC, 0x73, 0x1C, 0x53,
    0xCC, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// A deterministic 78-byte 2×2 RGBA PNG (colour type 6, one fully-transparent pixel): the alpha twin
/// of [`TINY_PNG`]. Its embedded material must gain `alphaMode:"BLEND"` so a transparent logo bakes
/// TRANSPARENT (the b3rb decals no longer sit on an opaque black background).
const TINY_RGBA_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xB6, 0x0D,
    0x24, 0x00, 0x00, 0x00, 0x15, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x08, 0x41, 0xE0, 0x3F, 0x10, 0x30, 0x34, 0x00, 0x00, 0x3D, 0x55, 0x07, 0x7A, 0xEF, 0xCC,
    0x87, 0xA0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// A unit quad with per-corner `vt` texture coordinates, one `usemtl` group bound to `quad.mtl`.
const VT_QUAD_OBJ: &str = "\
mtllib quad.mtl\n\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
usemtl skinned\n\
f 1/1 2/2 3/3\nf 1/1 3/3 4/4\n";

/// The textured `.mtl` twin: flat `Kd` plus a `map_Kd` the resolver may (or may not) supply.
const VT_QUAD_MTL_TEX: &str = "newmtl skinned\nKd 0.2 0.4 0.6\nmap_Kd quad.png\n";
/// The texture-less `.mtl` twin (same geometry → the byte-identity baseline).
const VT_QUAD_MTL_PLAIN: &str = "newmtl skinned\nKd 0.2 0.4 0.6\n";

/// Parse a GLB into (JSON chunk value, BIN chunk bytes), asserting the container invariants.
fn glb_parts(data: &[u8]) -> (serde_json::Value, Vec<u8>) {
    assert_eq!(&data[..4], b"glTF", "GLB magic");
    let u32_at = |off: usize| u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
    assert_eq!(u32_at(8), data.len(), "declared GLB length == actual");
    let json_len = u32_at(12);
    let json: serde_json::Value =
        serde_json::from_slice(&data[20..20 + json_len]).expect("JSON chunk parses");
    let bin_off = 20 + json_len;
    let bin = if bin_off < data.len() {
        let bin_len = u32_at(bin_off);
        data[bin_off + 8..bin_off + 8 + bin_len].to_vec()
    } else {
        Vec::new()
    };
    (json, bin)
}

/// Structural texture assertions shared by the OBJ and DAE fixtures: exactly one embedded image whose
/// bufferView holds `TINY_PNG` verbatim, one default sampler, one texture, `TEXCOORD_0` parallel to
/// POSITION (a VEC2 float accessor), and the FIRST material textured with a WHITE baseColorFactor.
/// Returns the decoded `TEXCOORD_0` uv pairs of the first primitive.
fn assert_textured_glb(data: &[u8]) -> Vec<[f32; 2]> {
    let (json, bin) = glb_parts(data);
    // images/samplers/textures wiring.
    let images = json["images"].as_array().expect("images array");
    assert_eq!(images.len(), 1, "one deduplicated image: {json}");
    assert_eq!(images[0]["mimeType"], "image/png");
    let img_bv = &json["bufferViews"][images[0]["bufferView"].as_u64().unwrap() as usize];
    let (off, len) = (
        img_bv["byteOffset"].as_u64().unwrap() as usize,
        img_bv["byteLength"].as_u64().unwrap() as usize,
    );
    assert_eq!(
        &bin[off..off + len],
        TINY_PNG,
        "PNG bytes passed through verbatim"
    );
    assert!(
        img_bv.get("target").is_none(),
        "an image bufferView carries no GPU target"
    );
    assert_eq!(
        json["samplers"].as_array().map(Vec::len),
        Some(1),
        "one default sampler"
    );
    assert_eq!(
        json["textures"],
        serde_json::json!([{"sampler": 0, "source": 0}])
    );
    // material: white factor × texture.
    let pbr = &json["materials"][0]["pbrMetallicRoughness"];
    assert_eq!(
        pbr["baseColorTexture"]["index"], 0,
        "material binds the texture: {json}"
    );
    assert_eq!(
        pbr["baseColorFactor"],
        serde_json::json!([1, 1, 1, 1]),
        "textured factor is WHITE"
    );
    // TEXCOORD_0: VEC2 float, parallel to POSITION.
    let attrs = &json["meshes"][0]["primitives"][0]["attributes"];
    let acc_i = attrs["TEXCOORD_0"].as_u64().expect("TEXCOORD_0 attribute") as usize;
    let pos_i = attrs["POSITION"].as_u64().unwrap() as usize;
    let (uv_acc, pos_acc) = (&json["accessors"][acc_i], &json["accessors"][pos_i]);
    assert_eq!(uv_acc["type"], "VEC2");
    assert_eq!(uv_acc["componentType"], 5126);
    assert_eq!(
        uv_acc["count"], pos_acc["count"],
        "TEXCOORD_0 parallel to POSITION"
    );
    // Decode the uv pairs.
    let bv = &json["bufferViews"][uv_acc["bufferView"].as_u64().unwrap() as usize];
    let (off, count) = (
        bv["byteOffset"].as_u64().unwrap() as usize,
        uv_acc["count"].as_u64().unwrap() as usize,
    );
    (0..count)
        .map(|i| {
            let at = |k: usize| {
                let b = off + i * 8 + k * 4;
                f32::from_le_bytes(bin[b..b + 4].try_into().unwrap())
            };
            [at(0), at(1)]
        })
        .collect()
}

/// The uv set (order-free, quantized): welding may reorder vertices, the SET may not change.
fn uv_set(uvs: &[[f32; 2]]) -> std::collections::BTreeSet<(i64, i64)> {
    uvs.iter()
        .map(|uv| ((uv[0] * 1e6) as i64, (uv[1] * 1e6) as i64))
        .collect()
}

#[test]
fn textured_obj_embeds_texcoords_image_and_base_color_texture() {
    let bake_it = || {
        bake::bake_convert_bytes_with(
            VT_QUAD_OBJ.as_bytes(),
            "quad.obj",
            None,
            BakeKind::Visual,
            None,
            |uri| match uri {
                "quad.mtl" => Some(VT_QUAD_MTL_TEX.as_bytes().to_vec()),
                "quad.png" => Some(TINY_PNG.to_vec()),
                _ => None,
            },
        )
        .unwrap()
    };
    let asset = bake_it();
    assert!(
        asset.notes.is_empty(),
        "fully-embedded texture -> no notes: {:?}",
        asset.notes
    );
    let uvs = assert_textured_glb(&asset.data);
    // OBJ `vt` (bottom-left origin) flips V to glTF's top-left: (0,0)(1,0)(1,1)(0,1) -> V' = 1-V.
    assert_eq!(
        uv_set(&uvs),
        uv_set(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]),
        "vt V-flip to the glTF origin"
    );
    // Deterministic: same input + same companions -> bit-identical GLB (stable @sha).
    assert_eq!(
        asset,
        bake_it(),
        "textured bake must stay byte-deterministic"
    );
}

#[test]
fn textured_obj_dedups_shared_image_across_groups() {
    // Two usemtl groups with DIFFERENT materials (Ns differs) sharing ONE map_Kd: the image embeds
    // once (content dedup), one texture, two materials both binding texture 0.
    const OBJ: &str = "\
mtllib quad.mtl\n\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
usemtl matte\nf 1/1 2/2 3/3\n\
usemtl glossy\nf 1/1 3/3 4/4\n";
    const MTL: &str = "\
newmtl matte\nKd 1 0 0\nmap_Kd quad.png\n\
newmtl glossy\nKd 0 0 1\nNs 96\nmap_Kd quad.png\n";
    let asset = bake::bake_convert_bytes_with(
        OBJ.as_bytes(),
        "quad.obj",
        None,
        BakeKind::Visual,
        None,
        |uri| match uri {
            "quad.mtl" => Some(MTL.as_bytes().to_vec()),
            "quad.png" => Some(TINY_PNG.to_vec()),
            _ => None,
        },
    )
    .unwrap();
    assert!(asset.notes.is_empty(), "no notes: {:?}", asset.notes);
    let (json, _) = glb_parts(&asset.data);
    assert_eq!(
        json["images"].as_array().map(Vec::len),
        Some(1),
        "shared image embeds ONCE"
    );
    assert_eq!(
        json["textures"].as_array().map(Vec::len),
        Some(1),
        "one texture per unique image"
    );
    let mats = json["materials"].as_array().unwrap();
    assert_eq!(
        mats.len(),
        2,
        "distinct materials stay distinct (Ns differs)"
    );
    for m in mats {
        assert_eq!(m["pbrMetallicRoughness"]["baseColorTexture"]["index"], 0);
        assert_eq!(
            m["pbrMetallicRoughness"]["baseColorFactor"],
            serde_json::json!([1, 1, 1, 1])
        );
    }
}

#[test]
fn textured_obj_mirror_scale_keeps_uvs_in_sync() {
    // A mirror bake reverses winding by reordering INDICES only; the parallel uv stream must ride
    // along untouched (same uv set), and (trimesh rule) the scale drops NORMAL but never TEXCOORD_0.
    let asset = bake::bake_convert_bytes_with(
        VT_QUAD_OBJ.as_bytes(),
        "quad.obj",
        Some("1 -1 1"),
        BakeKind::Visual,
        None,
        |uri| match uri {
            "quad.mtl" => Some(VT_QUAD_MTL_TEX.as_bytes().to_vec()),
            "quad.png" => Some(TINY_PNG.to_vec()),
            _ => None,
        },
    )
    .unwrap();
    let uvs = assert_textured_glb(&asset.data);
    assert_eq!(
        uv_set(&uvs),
        uv_set(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]),
        "mirror/scale leaves the uv stream untouched"
    );
    let (json, _) = glb_parts(&asset.data);
    assert!(
        json["meshes"][0]["primitives"][0]["attributes"]
            .get("NORMAL")
            .is_none(),
        "a scaled bake drops NORMAL (trimesh rule) while keeping TEXCOORD_0"
    );
}

#[test]
fn unsupported_texture_format_is_a_note_plus_flat_kd() {
    // map_Kd resolves, but the bytes are no PNG/JPEG (and the extension says nothing): flat Kd + a
    // note naming the file and the reason, no texture structures in the GLB.
    const MTL: &str = "newmtl skinned\nKd 0.2 0.4 0.6\nmap_Kd skin.ktx2\n";
    let asset = bake::bake_convert_bytes_with(
        VT_QUAD_OBJ.as_bytes(),
        "quad.obj",
        None,
        BakeKind::Visual,
        None,
        |uri| match uri {
            "quad.mtl" => Some(MTL.as_bytes().to_vec()),
            "skin.ktx2" => Some(b"\xabKTX 20\xbb not an embeddable image".to_vec()),
            _ => None,
        },
    )
    .unwrap();
    assert!(
        asset
            .notes
            .iter()
            .any(|n| n.contains("skin.ktx2") && n.contains("unsupported image format")),
        "unsupported container must be noted: {:?}",
        asset.notes
    );
    let json = glb_json(&asset.data);
    assert!(
        !json.contains("TEXCOORD_0") && !json.contains("\"images\""),
        "no texture structures on the fallback path: {json}"
    );
    assert_eq!(
        glb_base_colors(&json),
        vec![[0.2, 0.4, 0.6, 1.0]],
        "the flat Kd stands when the texture cannot embed"
    );
}

/// A one-triangle COLLADA 1.4.1 with a TEXCOORD input and a Lambert whose diffuse is `diffuse`: the
/// textured twin passes a `<texture>` sampler chain (sampler2D → surface → library_images init_from),
/// the flat twin a plain `<color>`.
#[cfg(feature = "bake-dae")]
fn tex_tri_dae(diffuse: &str) -> Vec<u8> {
    format!(
        r##"<?xml version="1.0"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
 <asset><created>1970-01-01T00:00:00Z</created><modified>1970-01-01T00:00:00Z</modified></asset>
 <library_images>
  <image id="skin-img"><init_from>tri.png</init_from></image>
 </library_images>
 <library_effects>
  <effect id="fx">
   <profile_COMMON>
    <newparam sid="skin-surf"><surface type="2D"><init_from>skin-img</init_from></surface></newparam>
    <newparam sid="skin-samp"><sampler2D><source>skin-surf</source></sampler2D></newparam>
    <technique sid="common">
     <lambert>
      <diffuse>{diffuse}</diffuse>
     </lambert>
    </technique>
   </profile_COMMON>
  </effect>
 </library_effects>
 <library_materials>
  <material id="mat"><instance_effect url="#fx"/></material>
 </library_materials>
 <library_geometries>
  <geometry id="g"><mesh>
   <source id="g-pos"><float_array id="g-pos-a" count="9">0 0 0 1 0 0 0 1 0</float_array>
    <technique_common><accessor source="#g-pos-a" count="3" stride="3">
     <param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/>
    </accessor></technique_common></source>
   <source id="g-uv"><float_array id="g-uv-a" count="6">0 0 1 0 0 1</float_array>
    <technique_common><accessor source="#g-uv-a" count="3" stride="2">
     <param name="S" type="float"/><param name="T" type="float"/>
    </accessor></technique_common></source>
   <vertices id="g-vtx"><input semantic="POSITION" source="#g-pos"/></vertices>
   <triangles count="1" material="mat0">
    <input semantic="VERTEX" source="#g-vtx" offset="0"/>
    <input semantic="TEXCOORD" source="#g-uv" offset="0" set="0"/>
    <p>0 1 2</p>
   </triangles>
  </mesh></geometry>
 </library_geometries>
 <library_visual_scenes><visual_scene id="S">
  <node id="n">
   <instance_geometry url="#g">
    <bind_material><technique_common>
     <instance_material symbol="mat0" target="#mat"/>
    </technique_common></bind_material>
   </instance_geometry>
  </node>
 </visual_scene></library_visual_scenes>
 <scene><instance_visual_scene url="#S"/></scene>
</COLLADA>
"##
    )
    .into_bytes()
}

#[test]
#[cfg(feature = "bake-dae")]
fn textured_dae_embeds_sampler_chain_texture() {
    let dae = tex_tri_dae(r#"<texture texture="skin-samp" texcoord="UVSET0"/>"#);
    let bake_it = || {
        bake::bake_convert_bytes_with(&dae, "tri.dae", None, BakeKind::Visual, None, |uri| {
            (uri == "tri.png").then(|| TINY_PNG.to_vec())
        })
        .unwrap()
    };
    let asset = bake_it();
    assert!(
        asset.notes.is_empty(),
        "fully-embedded texture -> no notes: {:?}",
        asset.notes
    );
    let uvs = assert_textured_glb(&asset.data);
    // COLLADA (S,T) bottom-left origin flips to glTF: (0,0)(1,0)(0,1) -> (0,1)(1,1)(0,0).
    assert_eq!(
        uv_set(&uvs),
        uv_set(&[[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]]),
        "ST V-flip"
    );
    assert_eq!(
        asset,
        bake_it(),
        "textured DAE bake must stay byte-deterministic"
    );

    // The unresolved-image twin: same document, resolver has nothing, so flat fallback + note, and NO
    // texture structures (never a phantom TEXCOORD_0).
    let missing = bake::bake_convert_bytes(&dae, "tri.dae", None, BakeKind::Visual, None).unwrap();
    assert!(
        missing
            .notes
            .iter()
            .any(|n| n.contains("tri.png") && n.contains("not resolved")),
        "missing image must be noted: {:?}",
        missing.notes
    );
    let json = glb_json(&missing.data);
    assert!(
        !json.contains("TEXCOORD_0") && !json.contains("\"images\""),
        "no phantom texture: {json}"
    );
    // The texture-diffuse Lambert falls back to the mid-grey COLLADA default (0.8), not black.
    assert_eq!(glb_base_colors(&json), vec![[0.8, 0.8, 0.8, 1.0]]);
}

#[test]
#[cfg(feature = "bake-dae")]
fn dae_texcoords_without_texture_change_nothing() {
    // A TEXCOORD input with a FLAT diffuse: uvs must not leak into the GLB (no TEXCOORD_0, no weld
    // change; the strictly-additive contract), and no notes.
    let dae = tex_tri_dae("<color>0.9 0.1 0.1 1</color>");
    let asset = bake::bake_convert_bytes(&dae, "tri.dae", None, BakeKind::Visual, None).unwrap();
    assert!(asset.notes.is_empty(), "{:?}", asset.notes);
    let json = glb_json(&asset.data);
    assert!(
        !json.contains("TEXCOORD_0") && !json.contains("\"images\""),
        "uvs must not leak: {json}"
    );
    assert_eq!(glb_base_colors(&json), vec![[0.9, 0.1, 0.1, 1.0]]);
}

// ── hint textures (the importer's <pbr><albedo_map> side-channel) + textured-primitive synthesis ──

/// A quad OBJ with per-corner `vt` but NO material anywhere (no `mtllib`/`usemtl`): the bare-geometry
/// canvas a hint texture is allowed to reach.
const BARE_VT_QUAD_OBJ: &str = "\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
f 1/1 2/2 3/3\nf 1/1 3/3 4/4\n";

/// A minimal one-triangle BINARY STL (no texture coordinates by construction).
fn tiny_stl() -> Vec<u8> {
    let mut b = vec![0u8; 84];
    b[80..84].copy_from_slice(&1u32.to_le_bytes());
    // normal (0,0,1) + verts (0,0,0)(1,0,0)(0,1,0) + attribute count.
    for v in [
        [0.0f32, 0.0, 1.0],
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
    ] {
        for c in v {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    b.extend_from_slice(&0u16.to_le_bytes());
    b
}

/// The [`bake::HintTexture`] the fixtures hand in: [`TINY_PNG`] under a `.png` name.
fn tiny_hint() -> bake::HintTexture {
    bake::HintTexture {
        bytes: TINY_PNG.to_vec(),
        name: "logo.png".to_string(),
    }
}

/// The POSITION stream of the first (and only) primitive of a baked GLB.
fn glb_positions(data: &[u8]) -> Vec<[f32; 3]> {
    let (json, bin) = glb_parts(data);
    let pos_i = json["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
        .as_u64()
        .unwrap() as usize;
    let acc = &json["accessors"][pos_i];
    let bv = &json["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
    let off = bv["byteOffset"].as_u64().unwrap() as usize;
    let count = acc["count"].as_u64().unwrap() as usize;
    (0..count)
        .map(|i| {
            let at = |k: usize| {
                let b = off + i * 12 + k * 4;
                f32::from_le_bytes(bin[b..b + 4].try_into().unwrap())
            };
            [at(0), at(1), at(2)]
        })
        .collect()
}

#[test]
fn textured_plane_box_synthesizes_uv_quad() {
    // The SDF importer maps a textured <plane size=".075 .075"> to a zero-thickness box; the synthesis
    // must produce the single +Z two-triangle quad with UV 0..1 and the PNG embedded (the b3rb decal).
    let asset = bake::bake_textured_box("0.075 0.075 0", &tiny_hint()).unwrap();
    assert!(
        asset.notes.is_empty(),
        "clean synthesis has no notes: {:?}",
        asset.notes
    );
    assert_eq!(asset.ext, "glb");
    let uvs = assert_textured_glb(&asset.data);
    assert_eq!(
        uv_set(&uvs),
        uv_set(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]),
        "one full 0..1 uv tile"
    );
    let pos = glb_positions(&asset.data);
    assert_eq!(
        pos.len(),
        4,
        "a single quad (no coincident backside face): {pos:?}"
    );
    let h = 0.075f32 / 2.0;
    for p in &pos {
        assert_eq!(p[2], 0.0, "the quad lies in z=0: {pos:?}");
        assert!(
            (p[0].abs() - h).abs() < 1e-7 && (p[1].abs() - h).abs() < 1e-7,
            "quad extents ±{h}: {pos:?}"
        );
    }
    let (json, _) = glb_parts(&asset.data);
    let idx_i = json["meshes"][0]["primitives"][0]["indices"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(json["accessors"][idx_i]["count"], 6, "two triangles");
    // Deterministic: same input -> same bytes -> same @sha.
    assert_eq!(
        asset,
        bake::bake_textured_box("0.075 0.075 0", &tiny_hint()).unwrap()
    );
}

#[test]
fn textured_box_synthesizes_per_face_uvs() {
    // A full box: 24 vertices (4 per face, the per-face 0..1 uv atlas forbids corner sharing), 12
    // triangles, extents ± half the size on every axis.
    let asset = bake::bake_textured_box("1 2 3", &tiny_hint()).unwrap();
    let uvs = assert_textured_glb(&asset.data);
    assert_eq!(uvs.len(), 24, "4 verts × 6 faces");
    let pos = glb_positions(&asset.data);
    assert_eq!(pos.len(), 24);
    let mut mx = [f32::NEG_INFINITY; 3];
    let mut mn = [f32::INFINITY; 3];
    for p in &pos {
        for k in 0..3 {
            mx[k] = mx[k].max(p[k]);
            mn[k] = mn[k].min(p[k]);
        }
    }
    assert_eq!(
        (mn, mx),
        ([-0.5, -1.0, -1.5], [0.5, 1.0, 1.5]),
        "box centred at the origin"
    );
    let (json, _) = glb_parts(&asset.data);
    let idx_i = json["meshes"][0]["primitives"][0]["indices"]
        .as_u64()
        .unwrap() as usize;
    assert_eq!(json["accessors"][idx_i]["count"], 36, "12 triangles");
}

#[test]
fn textured_box_synth_fails_loud_on_degenerate_input() {
    // Fewer than two positive extents, malformed sizes, and a non-image texture are ERRORS (the caller
    // keeps the flat primitive + notes why), never a silently-wrong GLB.
    for bad in ["0 0 1", "1 1", "a b c", "-1 1 1", "1 1 1 1"] {
        let err = bake::bake_textured_box(bad, &tiny_hint()).unwrap_err();
        assert!(
            matches!(err, bake::BakeError::PrimitiveSynth { .. }),
            "size {bad:?} must fail as PrimitiveSynth, got {err}"
        );
    }
    let not_an_image = bake::HintTexture {
        bytes: b"not an image".to_vec(),
        name: "logo.tga".to_string(),
    };
    let err = bake::bake_textured_box("1 1 1", &not_an_image).unwrap_err();
    assert!(
        matches!(err, bake::BakeError::PrimitiveSynth { .. }),
        "non-PNG/JPEG must fail: {err}"
    );
}

/// The [`bake::HintTexture`] carrying the ALPHA PNG ([`TINY_RGBA_PNG`]) under a `.png` name.
fn rgba_hint() -> bake::HintTexture {
    bake::HintTexture {
        bytes: TINY_RGBA_PNG.to_vec(),
        name: "logo.png".to_string(),
    }
}

/// The single material of a baked GLB (as a JSON value), asserting there is exactly one.
fn sole_material(data: &[u8]) -> serde_json::Value {
    let (json, _) = glb_parts(data);
    let mats = json["materials"].as_array().expect("materials array");
    assert_eq!(mats.len(), 1, "one material: {json}");
    mats[0].clone()
}

#[test]
fn textured_alpha_bake_sets_alpha_mode_blend() {
    // THE fix: an RGBA logo baked onto a decal plane gains alphaMode BLEND (so the transparent
    // background stays transparent, not the old opaque black), keeps baseColorFactor alpha at 1.0 (the
    // texture's own alpha drives it), and passes the PNG bytes through verbatim.
    let asset = bake::bake_textured_box("0.075 0.075 0", &rgba_hint()).unwrap();
    let (json, bin) = glb_parts(&asset.data);
    let mat = &json["materials"][0];
    assert_eq!(
        mat["alphaMode"], "BLEND",
        "alpha texture must blend: {json}"
    );
    assert_eq!(
        mat["pbrMetallicRoughness"]["baseColorFactor"],
        serde_json::json!([1, 1, 1, 1]),
        "baseColorFactor alpha stays 1.0 so the texture's alpha drives transparency"
    );
    // The alpha PNG embeds VERBATIM (never re-encoded).
    let img_bv = &json["bufferViews"][json["images"][0]["bufferView"].as_u64().unwrap() as usize];
    let off = img_bv["byteOffset"].as_u64().unwrap() as usize;
    let len = img_bv["byteLength"].as_u64().unwrap() as usize;
    assert_eq!(
        &bin[off..off + len],
        TINY_RGBA_PNG,
        "RGBA PNG passed through verbatim"
    );
    // Deterministic (a stable @sha).
    assert_eq!(
        asset,
        bake::bake_textured_box("0.075 0.075 0", &rgba_hint()).unwrap()
    );
}

#[test]
fn opaque_texture_bake_has_no_alpha_mode() {
    // The byte-identity contract: an OPAQUE texture (RGB, no tRNS) leaves alphaMode ABSENT, so its
    // material serializes to exactly the pre-alpha bytes. Both the decal plane and a full box.
    for size in ["0.075 0.075 0", "1 2 3"] {
        let asset = bake::bake_textured_box(size, &tiny_hint()).unwrap();
        let mat = sole_material(&asset.data);
        assert!(
            mat.get("alphaMode").is_none(),
            "opaque {size:?} must NOT declare alphaMode: {mat}"
        );
    }
}

#[test]
fn decal_plane_is_double_sided_full_box_is_not() {
    // A zero-thickness decal plane is a one-sided quad → doubleSided true (visible from behind, as gz
    // renders a plane decal). A closed box never shows its back-faces → doubleSided absent (default false).
    let plane = sole_material(
        &bake::bake_textured_box("0.075 0.075 0", &tiny_hint())
            .unwrap()
            .data,
    );
    assert_eq!(
        plane["doubleSided"], true,
        "a decal plane must be double-sided: {plane}"
    );
    let boxed = sole_material(&bake::bake_textured_box("1 2 3", &tiny_hint()).unwrap().data);
    assert!(
        boxed.get("doubleSided").is_none(),
        "a closed box stays single-sided: {boxed}"
    );
}

/// The real b3rb decal logo (an RGBA PNG), resolved from a cranium workspace checkout under `~`.
fn b3rb_nxp_png() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(
        "cognipilot/cranium/src/b3rb_simulator/b3rb_gz_resource/models/b3rb/materials/textures/NXP.png",
    )
}

#[test]
fn real_b3rb_logo_bakes_with_alpha_mode() {
    // Skipped-if-absent: bake the ACTUAL NXP decal (an RGBA logo) and prove its material blends,
    // the end-to-end confirmation the black-background bug is fixed on the real asset.
    let png = b3rb_nxp_png();
    let Ok(bytes) = std::fs::read(&png) else {
        eprintln!(
            "({} not on disk, skipping the real-logo alpha check)",
            png.display()
        );
        return;
    };
    let hint = bake::HintTexture {
        bytes,
        name: "NXP.png".to_string(),
    };
    let asset = bake::bake_textured_box("0.075 0.075 0", &hint).unwrap();
    let mat = sole_material(&asset.data);
    assert_eq!(
        mat["alphaMode"], "BLEND",
        "the real NXP RGBA logo must bake with alphaMode BLEND: {mat}"
    );
}

#[test]
fn hint_texture_embeds_via_mesh_own_uvs() {
    // A material-less OBJ WITH texture coordinates + a hint texture: the texture embeds through the
    // mesh's own uvs (V flipped to the glTF origin), factor WHITE, and beats a hint colour for the
    // baseColor slot (the documented texture-wins convention).
    let asset = bake::bake_convert_bytes_with_texture(
        BARE_VT_QUAD_OBJ.as_bytes(),
        "quad.obj",
        None,
        BakeKind::Visual,
        Some("0.8 0.2 0.1 1"),
        Some(&tiny_hint()),
        |_| None,
    )
    .unwrap();
    assert!(
        asset.notes.is_empty(),
        "clean embed has no notes: {:?}",
        asset.notes
    );
    let uvs = assert_textured_glb(&asset.data);
    assert_eq!(
        uv_set(&uvs),
        uv_set(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]),
        "vt V-flip to the glTF origin"
    );
}

#[test]
fn hint_texture_never_overrides_a_mesh_own_material() {
    // The same quad bound to a RESOLVED flat .mtl material: the mesh's own material wins (no image,
    // no TEXCOORD_0, the flat Kd kept), exactly the hint-colour precedence (silently, like the colour).
    let asset = bake::bake_convert_bytes_with_texture(
        VT_QUAD_OBJ.as_bytes(),
        "quad.obj",
        None,
        BakeKind::Visual,
        None,
        Some(&tiny_hint()),
        |uri| (uri == "quad.mtl").then(|| VT_QUAD_MTL_PLAIN.as_bytes().to_vec()),
    )
    .unwrap();
    assert!(asset.notes.is_empty(), "{:?}", asset.notes);
    let json = glb_json(&asset.data);
    assert!(
        !json.contains("TEXCOORD_0") && !json.contains("\"images\""),
        "own material wins, the hint texture must not attach: {json}"
    );
    assert_eq!(
        glb_base_colors(&json),
        vec![[0.2, 0.4, 0.6, 1.0]],
        "flat Kd kept"
    );
}

#[test]
fn hint_texture_on_uv_less_stl_is_a_note_plus_flat_colour() {
    // An STL authors no texture coordinates, so the hint texture cannot attach: the standard
    // no-texture-coordinates note + the flat hint colour, and NO texture structures in the GLB.
    let asset = bake::bake_convert_bytes_with_texture(
        &tiny_stl(),
        "tri.stl",
        None,
        BakeKind::Visual,
        Some("0.8 0.2 0.1 1"),
        Some(&tiny_hint()),
        |_| None,
    )
    .unwrap();
    assert!(
        asset
            .notes
            .iter()
            .any(|n| n.contains("logo.png") && n.contains("no texture coordinates")),
        "the miss must be noted: {:?}",
        asset.notes
    );
    let json = glb_json(&asset.data);
    assert!(
        !json.contains("TEXCOORD_0") && !json.contains("\"images\""),
        "no texture structures: {json}"
    );
    let colors = glb_base_colors(&json);
    assert_eq!(
        colors.len(),
        1,
        "the flat hint colour still applies: {json}"
    );
}

#[cfg(feature = "bake-dae")]
#[test]
fn hint_texture_reaches_bare_dae_geometry() {
    // A DAE triangle WITH a TEXCOORD input but NO material binding: bare geometry, so the hint texture
    // embeds through the authored uvs: the same shared bare-geometry arm as the OBJ path.
    const BARE_UV_TRI_DAE: &str = r##"<?xml version="1.0"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
 <asset><created>1970-01-01T00:00:00Z</created><modified>1970-01-01T00:00:00Z</modified></asset>
 <library_geometries>
  <geometry id="g"><mesh>
   <source id="g-pos"><float_array id="g-pos-a" count="9">0 0 0 1 0 0 0 1 0</float_array>
    <technique_common><accessor source="#g-pos-a" count="3" stride="3">
     <param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/>
    </accessor></technique_common></source>
   <source id="g-uv"><float_array id="g-uv-a" count="6">0 0 1 0 0 1</float_array>
    <technique_common><accessor source="#g-uv-a" count="3" stride="2">
     <param name="S" type="float"/><param name="T" type="float"/>
    </accessor></technique_common></source>
   <vertices id="g-vtx"><input semantic="POSITION" source="#g-pos"/></vertices>
   <triangles count="1">
    <input semantic="VERTEX" source="#g-vtx" offset="0"/>
    <input semantic="TEXCOORD" source="#g-uv" offset="0" set="0"/>
    <p>0 1 2</p>
   </triangles>
  </mesh></geometry>
 </library_geometries>
 <library_visual_scenes><visual_scene id="S">
  <node id="n"><instance_geometry url="#g"/></node>
 </visual_scene></library_visual_scenes>
 <scene><instance_visual_scene url="#S"/></scene>
</COLLADA>
"##;
    let asset = bake::bake_convert_bytes_with_texture(
        BARE_UV_TRI_DAE.as_bytes(),
        "tri.dae",
        None,
        BakeKind::Visual,
        None,
        Some(&tiny_hint()),
        |_| None,
    )
    .unwrap();
    assert!(asset.notes.is_empty(), "{:?}", asset.notes);
    let uvs = assert_textured_glb(&asset.data);
    assert_eq!(
        uv_set(&uvs),
        uv_set(&[[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]]),
        "COLLADA (S,T) V-flipped to the glTF origin"
    );
}

#[test]
fn hint_texture_none_is_byte_identical_to_plain_bake() {
    // The delegation contract: texture: None routes through the exact pre-hint code path.
    let plain = bake::bake_convert_bytes(
        BARE_VT_QUAD_OBJ.as_bytes(),
        "quad.obj",
        None,
        BakeKind::Visual,
        None,
    )
    .unwrap();
    let hinted = bake::bake_convert_bytes_with_texture(
        BARE_VT_QUAD_OBJ.as_bytes(),
        "quad.obj",
        None,
        BakeKind::Visual,
        None,
        None,
        |_| None,
    )
    .unwrap();
    assert_eq!(
        plain, hinted,
        "texture: None must not perturb a single byte"
    );
}

#[test]
fn no_texture_bake_is_byte_identical_to_pretexture_output() {
    // THE cheap proof of the additive contract: these shas were captured from the writer at the commit
    // BEFORE texture support (HEAD = 48090af) over fixtures that exercise every "almost textured"
    // corner: materials with no texture, `vt`/TEXCOORD present in the SOURCE but no texture bound,
    // and a `map_Kd` whose image cannot resolve. Any drift in the texture-less output changes a
    // content address (@sha) somewhere in the wild, so these must never change (the one exception,
    // called out inline, is the SCALED OpenArm body link, re-captured when scaled/mirrored bakes began
    // carrying NORMAL).
    let sha_of = |data: &[u8]| hcdformat::content_sha(data);

    // OBJ with materials, no textures anywhere.
    let two_mat = bake::bake_convert_bytes_with(
        TWO_MAT_OBJ.as_bytes(),
        "cube.obj",
        None,
        BakeKind::Visual,
        None,
        |_| Some(TWO_MAT_MTL.as_bytes().to_vec()),
    )
    .unwrap();
    assert_eq!(
        sha_of(&two_mat.data),
        "sha256:1cb8c4c4a0150ca82ef4d4ad41d5747861aa16e1919b75cc20158eaea55ce016",
        "two-material OBJ must bake byte-identically to the pre-texture writer"
    );

    // `vt` lines in the source but NO texture in the .mtl: the uvs must not perturb weld or JSON.
    let vt_plain = bake::bake_convert_bytes_with(
        VT_QUAD_OBJ.as_bytes(),
        "quad.obj",
        None,
        BakeKind::Visual,
        None,
        |uri| (uri == "quad.mtl").then(|| VT_QUAD_MTL_PLAIN.as_bytes().to_vec()),
    )
    .unwrap();
    let vt_quad_sha = "sha256:49dba54ec7fafc2e12ec8dbffe958969e96f50cfe124a296ad907700816d1462";
    assert_eq!(
        sha_of(&vt_plain.data),
        vt_quad_sha,
        "vt-without-texture is byte-identical"
    );

    // `map_Kd` present but the image does NOT resolve: byte-identical output + the visible note.
    let vt_missing = bake::bake_convert_bytes_with(
        VT_QUAD_OBJ.as_bytes(),
        "quad.obj",
        None,
        BakeKind::Visual,
        None,
        |uri| (uri == "quad.mtl").then(|| VT_QUAD_MTL_TEX.as_bytes().to_vec()),
    )
    .unwrap();
    assert_eq!(
        sha_of(&vt_missing.data),
        vt_quad_sha,
        "unresolvable texture leaves the bytes alone"
    );
    assert!(
        vt_missing.notes.iter().any(|n| n.contains("quad.png")),
        "…but is noted: {:?}",
        vt_missing.notes
    );

    // DAE (no texcoords, no textures): the COLLADA-1.5 test's 1.4 baseline document.
    #[cfg(feature = "bake-dae")]
    {
        let dae = bake::bake_convert_bytes(
            TRI_DAE_14.as_bytes(),
            "tri.dae",
            None,
            BakeKind::Visual,
            None,
        )
        .unwrap();
        assert_eq!(
            sha_of(&dae.data),
            "sha256:c8a3347f8058aa91229f2919aad6a832c52af680b137cc2adac758adb227ad1b",
            "texture-less DAE must bake byte-identically to the pre-texture writer"
        );
    }

    // Real files (skipped when absent): the Unitree pelvis (COLLADA 1.5 WITH TEXCOORD inputs but no
    // textures), the Digit torso (map_Kd but no vt), and the OpenArm body link (the reference DAE).
    let real: &[(&str, &str, Option<&str>, bool)] = &[
        (
            "~/hcdf-test-staging/pelvis15.dae",
            "sha256:74ebb3b6e93c092208ffcdf1a6b4dfe9528b4053ac521e2677fc6c9dc1557d7e",
            None,
            true,
        ),
        (
            "~/hcdf-test-staging/digit/torso.obj",
            "sha256:de2d02c2475a9e09ffdab3247c06f9517d602f666ee48c042e22bbd9d81f0d72",
            None,
            false,
        ),
        (
            // A SCALED bake: its sha was intentionally re-captured (was 7a7c26ea…) when scaled/mirrored
            // visuals began carrying NORMAL (transformed by the scale's normal matrix) instead of shipping
            // POSITION-only; the no-scale pins above are still the original pre-texture (48090af) values.
            "~/git/openarm_description/assets/robot/openarm_v2.0/meshes/body/visual/body_link0.dae",
            "sha256:25836dd34dbfa1ed811fd0fd5724e40677ff47cb88feb0696380a95c32c633c8",
            Some("0.001 0.001 0.001"),
            true,
        ),
    ];
    let home = std::env::var("HOME").unwrap_or_default();
    for (path, expect, scale, needs_dae) in real {
        if *needs_dae && cfg!(not(feature = "bake-dae")) {
            continue;
        }
        let path = path.replace('~', &home);
        if !Path::new(&path).is_file() {
            eprintln!("({path} not staged, skipping its byte-identity pin)");
            continue;
        }
        let baked = bake::bake_convert(&path, *scale, BakeKind::Visual, None).unwrap();
        assert_eq!(
            &baked.sha, expect,
            "{path} must bake byte-identically to the pre-texture writer"
        );
    }
}

#[test]
fn lean_collision_stl_matches_frozen_golden() {
    let Some(stl) = corpus_file(&["so_arm101/assets/moving_jaw_so101_v1_785a9dded2f4.stl"]) else {
        eprintln!("(no corpus STL, skipping lean-collision bake golden)");
        return;
    };
    // A mirror scale on a lean collision: winding flipped so signed volume keeps sign. Bytes are frozen.
    let data = bake::to_lean(&stl.to_string_lossy(), Some("2 -1 1")).unwrap();
    assert_golden(
        "bake/moving-jaw.lean-mirror.stl.sig",
        &bake_signature(&data),
    );
}

#[test]
#[cfg(feature = "bake-dae")]
fn dae_to_glb_corpus_matches_frozen_golden() {
    let Some(root) = corpus_root() else {
        eprintln!("(no corpus, skipping DAE->GLB bake golden)");
        return;
    };
    // Cover a Y_UP, a Z_UP, and a multi-geom corpus DAE (the up-axis being the documented risk): the
    // baked GLB bytes are frozen per source (each keyed by a stable golden name).
    let daes = [
        (
            "base-link-yup",
            "openarm_description/assets/robot/openarm_v2.0/meshes/arm/visual/base_link.dae",
        ),
        (
            "link0-zup",
            "openarm_description/assets/robot/openarm_v1.0/mesh/arm/visual/link0.dae",
        ),
        (
            "hand-multigeom",
            "openarm_description/assets/end_effector/parallel_link/meshes/visual/hand.dae",
        ),
    ];
    let mut ran = 0;
    for (tag, rel) in daes {
        let src = root.join(rel);
        if !src.is_file() {
            continue;
        }
        let data = bake::to_glb(&src.to_string_lossy(), None, None).unwrap();
        assert_golden(&format!("bake/{tag}.glb.sig"), &bake_signature(&data));
        ran += 1;
    }
    if ran == 0 {
        eprintln!("(no corpus DAE files, skipping DAE->GLB bake golden)");
    }
}

// ── COLLADA 1.5 downgrade pre-pass (bake-dae) ────────────────────────────────────────────────────

/// A distilled 1.4.1 COLLADA document (one triangle, one visual-scene node): the byte-identical
/// baseline its 1.5 twin (namespace/version swapped in the root tag only) must bake down to.
#[cfg(feature = "bake-dae")]
const TRI_DAE_14: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
  <asset>
    <created>1970-01-01T00:00:00Z</created>
    <modified>1970-01-01T00:00:00Z</modified>
  </asset>
  <library_geometries>
    <geometry id="tri-mesh" name="tri">
      <mesh>
        <source id="tri-pos">
          <float_array id="tri-pos-array" count="9">0 0 0 1 0 0 0 1 0</float_array>
          <technique_common>
            <accessor source="#tri-pos-array" count="3" stride="3">
              <param name="X" type="float"/>
              <param name="Y" type="float"/>
              <param name="Z" type="float"/>
            </accessor>
          </technique_common>
        </source>
        <vertices id="tri-verts">
          <input semantic="POSITION" source="#tri-pos"/>
        </vertices>
        <triangles count="1">
          <input semantic="VERTEX" source="#tri-verts" offset="0"/>
          <p>0 1 2</p>
        </triangles>
      </mesh>
    </geometry>
  </library_geometries>
  <library_visual_scenes>
    <visual_scene id="Scene">
      <node id="tri-node">
        <instance_geometry url="#tri-mesh"/>
      </node>
    </visual_scene>
  </library_visual_scenes>
  <scene>
    <instance_visual_scene url="#Scene"/>
  </scene>
</COLLADA>
"##;

/// Total triangle count across every mesh primitive of a GLB (indices accessor counts / 3).
#[cfg(feature = "bake-dae")]
fn glb_tri_count(data: &[u8]) -> usize {
    let j: serde_json::Value = serde_json::from_str(&glb_json(data)).unwrap();
    let accessors = j["accessors"].as_array().unwrap();
    j["meshes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["primitives"].as_array().unwrap())
        .map(|p| {
            accessors[p["indices"].as_u64().unwrap() as usize]["count"]
                .as_u64()
                .unwrap() as usize
                / 3
        })
        .sum()
}

#[test]
#[cfg(feature = "bake-dae")]
fn dae_collada_15_downgrades_and_bakes_like_its_14_twin() {
    // The 1.4 baseline: bakes with NO notes (the pre-pass fast path, untouched bytes), pinning that
    // texture-less/1.4 output stays exactly what it is today.
    let base = bake::bake_convert_bytes(
        TRI_DAE_14.as_bytes(),
        "tri.dae",
        None,
        BakeKind::Visual,
        None,
    )
    .unwrap();
    assert!(
        base.notes.is_empty(),
        "a 1.4 document must not be downgraded: {:?}",
        base.notes
    );
    assert_eq!(
        glb_tri_count(&base.data),
        1,
        "the distilled fixture is one triangle"
    );

    // The 1.5 twin differs ONLY in the root tag (namespace + version): the Cinema 4D / Unitree H2
    // export shape. It must bake BYTE-IDENTICALLY to the 1.4 baseline, with the one conversion note.
    let dae15 = TRI_DAE_14
        .replace(
            "http://www.collada.org/2005/11/COLLADASchema",
            "http://www.collada.org/2008/03/COLLADASchema",
        )
        .replace("version=\"1.4.1\"", "version=\"1.5.0\"");
    let baked15 =
        bake::bake_convert_bytes(dae15.as_bytes(), "tri.dae", None, BakeKind::Visual, None)
            .unwrap();
    assert_eq!(
        baked15.data, base.data,
        "1.5 downgrade must bake byte-identically to its 1.4 twin"
    );
    assert_eq!(
        baked15.notes,
        vec!["COLLADA 1.5 downgraded to 1.4.1 for import; 1.5-only constructs ignored".to_string()],
        "exactly one downgrade note"
    );

    // Attribute order / quoting / whitespace inside the root tag must not matter (the splice spans come
    // from quick-xml's parse, not a substring match): same bake, same note.
    let hostile = dae15.replace(
        "<COLLADA xmlns=\"http://www.collada.org/2008/03/COLLADASchema\" version=\"1.5.0\">",
        "<COLLADA version = '1.5.0'  xmlns='http://www.collada.org/2008/03/COLLADASchema'>",
    );
    assert_ne!(hostile, dae15, "the hostile root-tag rewrite must apply");
    let baked_hostile =
        bake::bake_convert_bytes(hostile.as_bytes(), "tri.dae", None, BakeKind::Visual, None)
            .unwrap();
    assert_eq!(
        baked_hostile.data, base.data,
        "root-tag formatting must not change the bake"
    );
    assert_eq!(baked_hostile.notes, baked15.notes);
}

#[test]
#[cfg(feature = "bake-dae")]
fn bake_dae_collada_15_pelvis_reference() {
    // The real-world rejection: Unitree H2's pelvis mesh (Cinema 4D COLLADA 1.5.0 export, the 2008/03
    // namespace) used to fail with dae-parser's "Unsupported COLLADA version". Staged OUTSIDE the repo,
    // skipped when absent.
    let scratch = staging_dir().join("pelvis15.dae");
    if !scratch.is_file() {
        eprintln!("(no COLLADA 1.5 pelvis.dae staged, skipping the real-file downgrade check)");
        return;
    }
    let asset =
        bake::bake_convert(&scratch.to_string_lossy(), None, BakeKind::Visual, None).unwrap();
    assert!(
        asset
            .notes
            .iter()
            .any(|n| n.contains("COLLADA 1.5 downgraded to 1.4.1")),
        "the downgrade must be noted: {:?}",
        asset.notes
    );
    // Geometry sanity: a plausible mesh, not a degenerate parse.
    let tris = glb_tri_count(&asset.data);
    assert!(
        tris > 10_000,
        "the pelvis mesh should carry >10k triangles, got {tris}"
    );
    // Its STL twin (same geometry, exported separately) pins the exact triangle count when staged.
    let stl_twin = scratch.with_file_name("pelvis.stl");
    if stl_twin.is_file() {
        let stl =
            bake::bake_convert(&stl_twin.to_string_lossy(), None, BakeKind::Visual, None).unwrap();
        assert_eq!(
            glb_tri_count(&stl.data),
            tris,
            "DAE and STL twins must agree on triangle count"
        );
    }
}
