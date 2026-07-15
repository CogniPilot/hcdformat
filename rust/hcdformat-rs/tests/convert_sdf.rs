//! SDF <-> HCDF converter FROZEN-GOLDEN harness (feature = `sdf`).
//!
//! Rust is the source of truth, so each fixture is pinned byte-for-byte to a committed golden produced BY
//! the Rust converter, with no live Python oracle:
//!   (a) [`hcdformat::from_sdf_path`] -> serialized HCDF is frozen at `tests/golden/sdf/<stem>.import.hcdf`.
//!   (b) the imported doc exported with [`hcdformat::to_sdf`] -> SDF is frozen at `<stem>.export.sdf`.
//!   (c) the gz-ABSENT contract: an SDF that omits spec-defaults imports RAW with no gz subprocess (the
//!       crate has none), asserted as a pure-Rust invariant and frozen at `sdf/raw-world.import.hcdf`.
//!   (d) inline mapping-branch fixtures frozen under `tests/golden/sdf/inline/`, plus a round-trip and a
//!       CLI smoke test of the built `hcdf convert`.
//!
//! The corpus is the self-contained in-repo `tests/sdf` fixtures, so the gate runs unconditionally with
//! NO Python. Regenerate the goldens with `HCDF_REGEN_GOLDENS=1 cargo test`.
#![cfg(feature = "sdf")]

use hcdformat::{from_sdf_path, from_sdf_str, from_sdf_str_with_assets, to_sdf, Hcdf};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A collision-free temp path: `<tmpdir>/hcdf_sdf_parity_<pid>_<counter>_<label>.<ext>`.
fn unique_tmp(label: &str, ext: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "hcdf_sdf_parity_{}_{n}_{label}.{ext}",
        std::process::id()
    ))
}

/// `<repo>` root: the crate sits at `<repo>/rust/hcdformat-rs`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The corpus SDFs the frozen goldens are committed for: the in-repo `tests/sdf` fixtures, so the golden
/// gate is fully self-contained and deterministic in any checkout (no external robot corpus required).
fn corpus() -> Vec<PathBuf> {
    let dir = repo_root().join("tests/sdf");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("sdf"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn world_sdf() -> Option<PathBuf> {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let p = home.join("git/hcdf-conversion-examples/world.sdf");
    p.is_file().then_some(p)
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

/// Write an inline fixture to a temp file with the given extension, returning its path (caller removes it).
/// The label is sanitized (non-alphanumeric -> '_') so a label with spaces/slashes still yields a flat,
/// valid temp filename.
fn write_fixture(label: &str, ext: &str, contents: &str) -> PathBuf {
    let safe: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let p = unique_tmp(&safe, ext);
    std::fs::write(&p, contents).expect("write fixture");
    p
}

// ── numeric extraction (shared with the unit-level round-trip checks) ─────────────────────────────

/// Every numeric attribute/text, keyed by a structural path so two same-structure docs align. Only
/// space-separated all-float values are collected (names/uris ignored). Returns `path -> Vec<f64>`.
fn numeric_values(xml: &str) -> BTreeMap<String, Vec<f64>> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(xml);
    let mut out: BTreeMap<String, Vec<f64>> = BTreeMap::new();
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

// ── (a) SDF -> HCDF parity on the corpus arms ────────────────────────────────────────────────────

/// A stable golden key stem for a corpus SDF (the file stem, e.g. `clean-model`).
fn corpus_stem(sdf: &std::path::Path) -> String {
    sdf.file_stem().unwrap().to_string_lossy().into_owned()
}

/// Sanitize a fixture label into a flat golden filename component (non-alphanumeric -> `_`).
fn golden_label(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[test]
fn from_sdf_matches_frozen_golden_on_corpus() {
    let files = corpus();
    assert!(
        !files.is_empty(),
        "no corpus SDFs found (in-repo tests/sdf/*.sdf must exist)"
    );
    let mut checked = 0usize;
    for sdf in &files {
        let name = sdf.file_name().unwrap().to_string_lossy().into_owned();
        let (doc, _notes) =
            from_sdf_path(sdf).unwrap_or_else(|e| panic!("{name}: Rust from_sdf failed: {e}"));
        let rust_hcdf = doc
            .to_xml_string()
            .unwrap_or_else(|e| panic!("{name}: Rust serialize failed: {e}"));
        // Frozen golden = the Rust importer's own HCDF output, byte-for-byte (no Python oracle).
        assert_golden(&format!("sdf/{}.import.hcdf", corpus_stem(sdf)), &rust_hcdf);
        checked += 1;
    }
    assert!(
        checked >= 1,
        "expected to exercise at least one corpus SDF, checked {checked}"
    );
}

// ── (b) HCDF -> SDF parity (freeze the exporter's output) ─────────────────────────────────────────

#[test]
fn to_sdf_matches_frozen_golden_on_corpus() {
    let mut checked = 0usize;
    for sdf in &corpus() {
        let name = sdf.file_name().unwrap().to_string_lossy().into_owned();
        // Import with the Rust importer, then export with the Rust exporter: the natural pipeline now
        // that the Python oracle is gone. The exported SDF is frozen byte-for-byte.
        let (doc, _notes) =
            from_sdf_path(sdf).unwrap_or_else(|e| panic!("{name}: Rust from_sdf failed: {e}"));
        let (rust_sdf, _rust_loss) =
            to_sdf(&doc).unwrap_or_else(|e| panic!("{name}: Rust to_sdf failed: {e}"));
        assert_golden(&format!("sdf/{}.export.sdf", corpus_stem(sdf)), &rust_sdf);
        checked += 1;
    }
    assert!(
        checked >= 1,
        "expected to exercise at least one HCDF->SDF round-trip, checked {checked}"
    );
}

// ── (c) the gz-ABSENT contract: an SDF omitting spec-defaults imports RAW (no gz) ─────────────────
//
// `from_sdf` NEVER shells to gz on the import path (it mirrors lxml `recover=True`). This pins that an
// SDF that omits spec-defaults / required elements imports WITHOUT gz and that `from_sdf_str` and
// `from_sdf_path` agree: a pure-Rust contract needing no oracle. The `world.sdf` fixture, when the
// external corpus is present, additionally exercises the raw-world path.

#[test]
fn sdf_imports_raw_without_gz() {
    // A hand-authored world that omits many spec-defaulted/required elements: must import RAW, gz-free.
    let raw = r#"<sdf version="1.9"><world name="w">
        <model name="m"><link name="l">
          <visual name="v"><geometry><box><size>1 1 1</size></box></geometry></visual>
        </link></model></world></sdf>"#;
    let (doc, notes) =
        from_sdf_str(raw).expect("raw SDF imports with no strict gate (mirrors recover=True)");
    assert!(
        !notes.iter().any(|n| n.contains("gz sdf --print")),
        "from_sdf_str must NOT shell to gz; got {notes:?}"
    );
    // Frozen golden of the raw import (self-contained inline fixture).
    assert_golden(
        "sdf/raw-world.import.hcdf",
        &doc.to_xml_string().expect("serialize"),
    );

    // When the external world.sdf corpus fixture is present, also assert from_sdf_str == from_sdf_path
    // (both gz-free) on it: a pure-Rust invariant that needs no golden.
    if let Some(world) = world_sdf() {
        let text = std::fs::read_to_string(&world).expect("read world.sdf");
        let (d_str, _) = from_sdf_str(&text).expect("world.sdf imports raw");
        let (d_path, _) = from_sdf_path(&world).expect("from_sdf_path raw import");
        assert_eq!(
            d_str.to_xml_string().unwrap(),
            d_path.to_xml_string().unwrap(),
            "from_sdf_str and from_sdf_path must produce the identical raw import"
        );
    }
}

// ── (c3) frozen goldens on the previously-uncovered mapping branches (inline gz-absent fixtures) ───
//
// The corpus is canonical/FLU/fully-named/single-model. These inline fixtures cover mapping branches
// with distinct shapes (synthesized names, model:// meshes, scaled/coloured visuals, multi/nested/
// world-nested/include-bearing models), each frozen to the Rust importer's byte-for-byte HCDF output.

/// Shared driver: run an inline fixture SDF through Rust `from_sdf_str` and freeze its HCDF output.
fn assert_sdf_import_parity(label: &str, sdf: &str) {
    let (doc, _notes) =
        from_sdf_str(sdf).unwrap_or_else(|e| panic!("{label}: Rust from_sdf_str failed: {e}"));
    let rust_hcdf = doc
        .to_xml_string()
        .unwrap_or_else(|e| panic!("{label}: serialize failed: {e}"));
    assert_golden(
        &format!("sdf/inline/{}.hcdf", golden_label(label)),
        &rust_hcdf,
    );
}

#[test]
fn unnamed_collision_visual_synthesized_names_match_python() {
    // Two unnamed collisions + two unnamed visuals on link 'l' -> Python synthesizes 'l_collision'
    // (x2) / 'l_visual' (x2) with NO index and name-origin="synthesized". Rust MUST match exactly (the
    // _{i}-index form would diverge). Both sides import gz-free.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <collision><geometry><box><size>1 1 1</size></box></geometry></collision>
        <collision><geometry><sphere><radius>0.5</radius></sphere></geometry></collision>
        <visual><geometry><box><size>1 1 1</size></box></geometry></visual>
        <visual><geometry><sphere><radius>0.5</radius></sphere></geometry></visual>
        </link></model></sdf>"#;
    // First a direct, oracle-free assertion on the synthesized names (so the contract is pinned even when
    // the Python oracle is unavailable).
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    let l = &doc.comp[0];
    assert_eq!(l.collision.len(), 2);
    assert_eq!(l.visual.len(), 2);
    use hcdformat::model::enums::NameOrigin;
    for c in &l.collision {
        assert_eq!(
            c.name.as_deref(),
            Some("l_collision"),
            "no index, matches Python"
        );
        assert_eq!(c.name_origin, Some(NameOrigin::Synthesized));
    }
    for v in &l.visual {
        assert_eq!(v.name, "l_visual", "no index, matches Python");
        assert_eq!(v.name_origin, Some(NameOrigin::Synthesized));
    }
    assert_sdf_import_parity("unnamed coll/vis [sdf->hcdf]", sdf);
}

#[test]
fn continuous_joint_strips_gazebo_position_sentinels() {
    // Import guard: an SDF/gazebo continuous joint routinely carries ±1e16 sentinel position bounds
    // on <axis><limit>. HCDF continuous FORBIDS lower/upper (E_JOINT_CONTINUOUS_BOUNDS), so a verbatim
    // copy would produce a self-invalid import. from_sdf must drop lower/upper (keeping the meaningful
    // effort/velocity), record a note, and the imported doc must pass semantic validation.
    let sdf = r#"<sdf version="1.9"><model name="m">
        <link name="base"/><link name="wheel"/>
        <joint name="spin" type="continuous">
          <parent>base</parent><child>wheel</child>
          <axis><xyz>0 0 1</xyz><limit>
            <lower>-1e16</lower><upper>1e16</upper><effort>5</effort><velocity>10</velocity>
          </limit></axis>
        </joint></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let j = doc
        .joint
        .iter()
        .find(|j| j.name.as_deref() == Some("spin"))
        .expect("spin joint");
    let lim = j
        .limit
        .as_ref()
        .expect("continuous joint keeps its effort/velocity limit");
    assert_eq!(lim.lower, None, "gazebo lower sentinel stripped");
    assert_eq!(lim.upper, None, "gazebo upper sentinel stripped");
    assert_eq!(lim.effort.as_deref(), Some("5"), "effort kept");
    assert_eq!(lim.velocity.as_deref(), Some("10"), "velocity kept");
    assert!(
        notes
            .iter()
            .any(|n| n.contains("dropped position lower/upper on the continuous joint")),
        "the drop must be recorded as a note; got {notes:?}"
    );
    assert!(
        hcdformat::validate_semantic(&doc).is_empty(),
        "the imported continuous joint must pass semantic validation (no E_JOINT_CONTINUOUS_BOUNDS)"
    );
}

#[test]
fn model_uri_mesh_preserved_verbatim_matches_python() {
    // A model:// mesh uri must be preserved VERBATIM (not resolved by gz to an absolute path). Python
    // keeps `model://my_pkg/meshes/part.stl` as-is, gz-free; Rust must too.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <collision name="c"><geometry><mesh><uri>model://my_pkg/meshes/part.stl</uri></mesh></geometry></collision>
        </link></model></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    let mesh = doc.comp[0].collision[0]
        .geometry
        .as_ref()
        .and_then(|g| g.mesh.as_ref())
        .expect("collision mesh");
    assert_eq!(
        mesh.uri.as_deref(),
        Some("model://my_pkg/meshes/part.stl"),
        "model:// uri preserved verbatim (not gz-resolved)"
    );
    assert_sdf_import_parity("model:// mesh [sdf->hcdf]", sdf);
}

#[test]
fn visual_mesh_scale_and_color_carried_by_asset_hints() {
    // The SDF asset side-channel (mirrors the URDF one): each ARM-A (mesh) visual yields ONE
    // VisualAssetHint keyed by (comp, visual) carrying the raw <mesh><scale> text + the flat
    // <material><diffuse> colour, exactly what from_sdf.py hands its baker(mesh_uri, scale,
    // "visual", color) hook, while the MODEL stays untouched (a visual <model> is a clean GLB
    // uri+sha; scale/colour never enter the HCDF). Primitive visuals keep their typed <color>
    // in the model and get NO hint.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <visual name="scaled"><geometry><mesh><uri>meshes/arm.stl</uri><scale>2 2 -2</scale></mesh></geometry>
          <material><diffuse>0.8 0.2 0.1 1</diffuse></material></visual>
        <visual><geometry><mesh><uri>meshes/plain.stl</uri></mesh></geometry></visual>
        <visual name="prim"><geometry><box><size>1 1 1</size></box></geometry>
          <material><diffuse>0 0 1 1</diffuse></material></visual>
        </link></model></sdf>"#;
    let (doc, notes, hints) = from_sdf_str_with_assets(sdf).expect("from_sdf_str_with_assets");
    assert_eq!(
        hints.len(),
        2,
        "one hint per MESH visual, none for the primitive: {hints:?}"
    );
    let scaled = &hints[0];
    assert_eq!(
        (scaled.comp.as_str(), scaled.visual.as_str()),
        ("l", "scaled")
    );
    assert_eq!(
        scaled.scale.as_deref(),
        Some("2 2 -2"),
        "raw <scale> text, mirror included"
    );
    assert_eq!(
        scaled.color.as_deref(),
        Some("0.8 0.2 0.1 1"),
        "material <diffuse> colour"
    );
    let plain = &hints[1];
    assert_eq!(
        plain.visual, "l_visual",
        "the synthesized visual name keys the hint"
    );
    assert_eq!(
        (plain.scale.as_deref(), plain.color.as_deref()),
        (None, None)
    );
    // the model is IDENTICAL to the discarding wrapper's; the hints are purely additive.
    let (doc2, notes2) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(doc.to_xml_string().unwrap(), doc2.to_xml_string().unwrap());
    assert_eq!(
        notes, notes2,
        "the wrapper only discards hints; the notes are unchanged"
    );
    let xml = doc.to_xml_string().unwrap();
    assert!(
        !xml.contains("2 2 -2") && !xml.contains("0.8 0.2 0.1"),
        "visual scale/colour never enter the HCDF: {xml}"
    );
    // the deferred-bake notes still match from_sdf.py with baker=None.
    assert!(
        notes
            .iter()
            .any(|n| n.contains("mesh <scale> '2 2 -2' not applied")),
        "scale deferral note kept: {notes:?}"
    );
    assert_sdf_import_parity("scaled+coloured mesh visuals [sdf->hcdf]", sdf);
}

#[test]
fn albedo_map_carried_by_texture_hints() {
    // The <material><pbr><metal|specular><albedo_map> side-channel: a MESH visual's albedo uri rides
    // its hint (texture field) next to scale/diffuse; a textured PRIMITIVE gets a texture-only hint;
    // and a textured <plane> (which HCDF cannot represent) maps to a zero-thickness <box> so the
    // asset step can synthesize the quad GLB (the b3rb logo-decal pattern). The uris stay OUT of the
    // HCDF document (model:// resolution is the baker's job). No Python-oracle parity here: the
    // authority still drops all of this, the carry is the Rust importer's documented extension.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <visual name="skinned"><geometry><mesh><uri>meshes/arm.obj</uri></mesh></geometry>
          <material><diffuse>1 1 1</diffuse>
            <pbr><metal><albedo_map>model://m/materials/textures/skin.png</albedo_map></metal></pbr>
          </material></visual>
        <visual name="logo">
          <geometry><plane><normal>0 0 1</normal><size>.075 .075</size></plane></geometry>
          <material><diffuse>1.0 1.0 1.0</diffuse><specular>1.0 1.0 1.0</specular>
            <pbr><metal><albedo_map>model://m/materials/textures/logo.png</albedo_map></metal></pbr>
          </material></visual>
        <visual name="bare_plane"><geometry><plane><size>1 1</size></plane></geometry></visual>
        <visual name="ball"><geometry><sphere><radius>0.1</radius></sphere></geometry>
          <material><pbr><specular><albedo_map>tex/ball.png</albedo_map></specular></pbr></material></visual>
        </link></model></sdf>"#;
    let (doc, notes, hints) = from_sdf_str_with_assets(sdf).expect("from_sdf_str_with_assets");
    assert_eq!(
        hints.len(),
        3,
        "mesh + textured plane + textured sphere: {hints:?}"
    );

    // MESH visual: diffuse + albedo both on the hint.
    let mesh = &hints[0];
    assert_eq!((mesh.comp.as_str(), mesh.visual.as_str()), ("l", "skinned"));
    assert_eq!(mesh.color.as_deref(), Some("1 1 1"));
    assert_eq!(
        mesh.texture.as_deref(),
        Some("model://m/materials/textures/skin.png")
    );

    // TEXTURED PLANE: maps to a zero-thickness box (untextured planes keep the old drop) + a
    // texture-only hint (the diffuse already lives in the typed <color>).
    let logo = &hints[1];
    assert_eq!(logo.visual, "logo");
    assert_eq!((logo.scale.as_deref(), logo.color.as_deref()), (None, None));
    assert_eq!(
        logo.texture.as_deref(),
        Some("model://m/materials/textures/logo.png")
    );
    let l = &doc.comp[0];
    let logo_vis = l
        .visual
        .iter()
        .find(|v| v.name == "logo")
        .expect("plane visual mapped");
    match &logo_vis.appearance {
        hcdformat::VisualAppearance::Primitive { geometry, color } => {
            let b = geometry
                .as_ref()
                .and_then(|g| g.box_.as_ref())
                .expect("a box");
            assert_eq!(
                b.size.as_deref(),
                Some(".075 .075 0"),
                "plane <size> verbatim + zero thickness"
            );
            assert_eq!(
                color.as_ref().and_then(|c| c.rgba.as_deref()),
                Some("1.0 1.0 1.0"),
                "the diffuse stays in the typed <color>"
            );
        }
        other => panic!("textured plane must stay a primitive pre-bake: {other:?}"),
    }
    assert!(
        l.visual.iter().all(|v| v.name != "bare_plane"),
        "an UNTEXTURED plane keeps the old no-HCDF-mapping drop"
    );

    // Textured sphere: hint carried (specular-workflow albedo_map), primitive untouched.
    let ball = &hints[2];
    assert_eq!(ball.visual, "ball");
    assert_eq!(ball.texture.as_deref(), Some("tex/ball.png"));

    // The texture uris never enter the HCDF document.
    let xml = doc.to_xml_string().unwrap();
    assert!(
        !xml.contains(".png"),
        "texture uris are side-channel only: {xml}"
    );

    // Notes: carried wording for both arms, the plane mapping note, and the untextured-plane drop.
    let has = |needle: &str| notes.iter().any(|n| n.contains(needle));
    assert!(
        has("albedo map 'model://m/materials/textures/skin.png' carried"),
        "{notes:?}"
    );
    assert!(
        has("textured <plane> mapped to a zero-thickness <box> (.075 .075 0)"),
        "{notes:?}"
    );
    assert!(
        has("<geometry><plane> has no HCDF mapping; dropped"),
        "{notes:?}"
    );
    assert!(
        has("ambient/specular/PBR beyond the carried albedo map not represented"),
        "the specular next to the carried albedo is still noted: {notes:?}"
    );
    // the discarding wrapper stays in lockstep.
    let (_, notes2) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(notes, notes2);
}

#[test]
fn sensors_typed_reroot_survives_not_dropped() {
    // INVERTED regression lock (was `sensors_dropped_note_and_mapping_match_python`): the typed sensor
    // re-root now maps <link><sensor>s into comp.sensor[*].<category> instead of dropping them. The
    // Rust importer goes AHEAD of the Python spec-reference here (which still drops sensors), exactly as
    // it already does on the SDF frame graph / <frame> mapping, so NO Python-oracle parity is asserted
    // for the sensors; the mechanical visual/surface still map as before.
    let sdf = r#"<sdf version="1.12"><model name="rich"><link name="base">
        <inertial><mass>2.5</mass><inertia><ixx>0.1</ixx><iyy>0.2</iyy><izz>0.3</izz></inertia></inertial>
        <visual name="bv"><geometry><box><size>1 0.5 0.25</size></box></geometry>
          <material><diffuse>0.8 0.2 0.2 1</diffuse><ambient>0.1 0.1 0.1 1</ambient></material></visual>
        <collision name="bc"><geometry><cylinder><radius>0.3</radius><length>0.6</length></cylinder></geometry>
          <surface><friction><ode><mu>0.9</mu><mu2>0.7</mu2></ode></friction>
            <bounce><restitution_coefficient>0.3</restitution_coefficient></bounce>
            <contact><ode><kp>1000</kp><kd>10</kd></ode></contact></surface></collision>
        <sensor name="imu_sensor" type="imu"><always_on>1</always_on></sensor>
        <sensor name="cam" type="camera"><update_rate>30</update_rate></sensor>
        </link></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let base = &doc.comp[0];
    // Sensors SURVIVE (the inverted assertion): imu -> inertial, camera -> optical.
    assert_eq!(
        base.sensor.len(),
        2,
        "both <sensor>s now mapped, not dropped"
    );
    assert_eq!(base.sensor[0].name.as_deref(), Some("imu_sensor"));
    assert!(!base.sensor[0].inertial.is_empty(), "imu -> <inertial>");
    assert_eq!(base.sensor[1].name.as_deref(), Some("cam"));
    assert_eq!(base.sensor[1].update_rate.as_deref(), Some("30"));
    assert!(!base.sensor[1].optical.is_empty(), "camera -> <optical>");
    // The old drop note is GONE.
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("not mapped") && n.contains("deferred")),
        "the typed re-root replaced the deferred-drop note; got {notes:?}"
    );
    // always_on (runtime) is noted, never silent.
    assert!(
        notes
            .iter()
            .any(|n| n.contains("runtime/sim-only") && n.contains("always_on")),
        "runtime field noted: {notes:?}"
    );
    // The mechanical visual/surface still map (regression guard on the rest of link()).
    assert!(
        base.visual.iter().any(|v| v.name == "bv"),
        "visual still maps"
    );
    assert!(
        base.collision[0].surface.is_some(),
        "collision surface still maps"
    );
}

#[test]
fn sdf_imu_sensor_captures_per_axis_noise_and_dynamic_bias() {
    // Dynamic-bias + per-axis IMU noise: the sim-only dynamic-bias leaves now land in the
    // TYPED noise (not org.gazebosim.raw), and the per-axis y/z noise is kept in a full <axis-noise>
    // container instead of being collapsed to the x-axis representative.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <sensor name="imu0" type="imu"><pose>0 0 0.01 0 0 0</pose>
          <update_rate>200</update_rate>
          <imu>
            <linear_acceleration>
              <x><noise type="gaussian"><mean>0</mean><stddev>0.017</stddev>
                <dynamic_bias_stddev>0.001</dynamic_bias_stddev>
                <dynamic_bias_correlation_time>400</dynamic_bias_correlation_time></noise></x>
              <y><noise type="gaussian"><stddev>0.019</stddev></noise></y>
            </linear_acceleration>
            <angular_velocity>
              <x><noise type="gaussian"><stddev>0.0009</stddev></noise></x>
            </angular_velocity>
          </imu>
          <always_on>1</always_on><topic>/imu</topic>
        </sensor></link></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let s = &doc.comp[0].sensor[0];
    assert_eq!(s.name.as_deref(), Some("imu0"));
    assert_eq!(s.update_rate.as_deref(), Some("200"));
    let imu = &s.inertial[0];
    use hcdformat::model::enums::{InertialSensorType, NoiseType};
    assert_eq!(imu.type_, Some(InertialSensorType::AccelGyro));
    assert_eq!(
        imu.pose.as_ref().and_then(|p| p.xyz).map(|x| x[2]),
        Some(0.01)
    );
    // Dynamic-bias captured into the TYPED accel noise (x-axis / scalar fallback), NOT quarantined.
    let accel_params = imu.accel.as_ref().expect("accel params");
    let accel = accel_params.noise.as_ref().expect("accel scalar noise");
    assert_eq!(accel.stddev.as_deref(), Some("0.017"));
    assert_eq!(accel.type_, Some(NoiseType::Gaussian));
    assert_eq!(accel.dynamic_bias_stddev.as_deref(), Some("0.001"));
    assert_eq!(accel.dynamic_bias_correlation_time.as_deref(), Some("400"));
    // The accel channel has genuine per-axis noise -> a full <axis-noise> (x + y, z absent).
    let an = accel_params.axis_noise.as_ref().expect("accel axis-noise");
    assert_eq!(
        an.x.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.017")
    );
    assert_eq!(
        an.y.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.019")
    );
    assert!(an.z.is_none(), "z axis carried no noise");
    // gyro has only an x-axis noise -> just the scalar fallback, no <axis-noise>.
    let gyro_params = imu.gyro.as_ref().expect("gyro params");
    assert_eq!(
        gyro_params.noise.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.0009")
    );
    assert!(
        gyro_params.axis_noise.is_none(),
        "isotropic gyro keeps only the scalar noise"
    );
    // No dynamic-bias quarantine: nothing in org.gazebosim.raw references dynamic_bias.
    assert!(
        !doc.comp[0]
            .extension
            .iter()
            .any(|e| e.domain == "org.gazebosim.raw" && e.body.contains("dynamic_bias")),
        "dynamic-bias is typed now, not quarantined"
    );
    // The stale collapse/quarantine notes are gone; runtime always_on is still noted.
    assert!(
        !notes.iter().any(|n| n.contains("per-axis noise collapsed")),
        "{notes:?}"
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("dynamic-bias") && n.contains("org.gazebosim")),
        "{notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("runtime/sim-only") && n.contains("always_on")),
        "{notes:?}"
    );

    // Round-trip SDF -> HCDF -> SDF: the per-axis noise + dynamic-bias survive back to the SDF grammar.
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains("<linear_acceleration>"),
        "imu channel re-emitted: {out}"
    );
    assert!(
        out.contains("<dynamic_bias_stddev>0.001</dynamic_bias_stddev>"),
        "dynamic-bias re-emitted: {out}"
    );
    assert!(
        out.contains("<dynamic_bias_correlation_time>400</dynamic_bias_correlation_time>"),
        "{out}"
    );
    assert!(out.contains("<y>"), "per-axis y noise re-emitted: {out}");
    // Re-import the emitted SDF: the typed noise/axis-noise/dynamic-bias are stable.
    let (doc2, _) = from_sdf_str(&out).expect("re-import emitted SDF");
    let accel2 = doc2.comp[0].sensor[0].inertial[0]
        .accel
        .as_ref()
        .expect("accel2");
    assert_eq!(
        accel2
            .noise
            .as_ref()
            .and_then(|n| n.dynamic_bias_stddev.as_deref()),
        Some("0.001")
    );
    let an2 = accel2.axis_noise.as_ref().expect("accel2 axis-noise");
    assert_eq!(
        an2.y.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.019")
    );
}

#[test]
fn sdf_camera_sensor_maps_intrinsics_distortion_frustum_quarantines_clip() {
    // hfov = pi/2 so the derived fx = (width/2)/tan(pi/4) = 320 exactly.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <sensor name="cam0" type="camera"><pose>0.1 0 0.2 0 0 0</pose>
          <update_rate>30</update_rate>
          <camera>
            <horizontal_fov>1.5707963267948966</horizontal_fov>
            <image><width>640</width><height>480</height><format>R8G8B8</format></image>
            <clip><near>0.05</near><far>300</far></clip>
            <distortion><k1>-0.1</k1><k2>0.02</k2><p1>0.001</p1><p2>-0.001</p2></distortion>
          </camera>
        </sensor></link></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    use hcdformat::model::enums::OpticalSensorType;
    let opt = &doc.comp[0].sensor[0].optical[0];
    assert_eq!(opt.type_, Some(OpticalSensorType::Camera));
    let fov = &opt.fov[0];
    let intr = fov.intrinsics.as_ref().expect("intrinsics");
    assert_eq!(intr.width.as_deref(), Some("640"));
    assert_eq!(intr.height.as_deref(), Some("480"));
    assert_eq!(intr.format.as_deref(), Some("R8G8B8"));
    assert_eq!(
        intr.fx.as_deref(),
        Some("320"),
        "fx derived from hfov+image"
    );
    assert_eq!(intr.cx.as_deref(), Some("320"));
    assert_eq!(intr.cy.as_deref(), Some("240"));
    let dist = fov.distortion.as_ref().expect("distortion");
    assert_eq!(dist.k1.as_deref(), Some("-0.1"));
    assert_eq!(dist.p2.as_deref(), Some("-0.001"));
    let fr = fov
        .geometry
        .as_ref()
        .and_then(|g| g.frustum.as_ref())
        .expect("frustum");
    assert_eq!(fr.near.as_deref(), Some("0.05"));
    assert_eq!(fr.far.as_deref(), Some("300"));
    use hcdformat::model::enums::FrustumShape;
    assert_eq!(fr.shape, Some(FrustumShape::Pyramidal));
    // raw <clip> quarantined to org.gazebosim.raw.
    let ext = doc.comp[0]
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim.raw")
        .expect("org.gazebosim.raw");
    assert!(
        ext.body.contains("<clip>") && ext.body.contains("<near>0.05</near>"),
        "clip preserved: {}",
        ext.body
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("intrinsics") && n.contains("derived")),
        "{notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("<clip>") && n.contains("org.gazebosim")),
        "{notes:?}"
    );
}

#[test]
fn sdf_force_torque_frame_measure_and_per_axis_noise_round_trip() {
    // The reporting @frame/@measure-direction sign convention + per-axis force/torque <noise>
    // survive SDF -> HCDF -> SDF (previously frame/measure_direction were dropped and the 6 axes of
    // noise collapsed to one).
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <sensor name="ft0" type="force_torque"><force_torque>
          <frame>sensor</frame>
          <measure_direction>parent_to_child</measure_direction>
          <force>
            <x><noise type="gaussian"><stddev>0.1</stddev></noise></x>
            <z><noise type="gaussian"><stddev>0.3</stddev></noise></z>
          </force>
          <torque>
            <y><noise type="gaussian"><stddev>0.02</stddev></noise></y>
          </torque>
        </force_torque></sensor></link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    use hcdformat::model::enums::{ForceFrame, MeasureDirection};
    let fo = &doc.comp[0].sensor[0].force[0];
    assert_eq!(fo.frame, Some(ForceFrame::Sensor));
    assert_eq!(fo.measure_direction, Some(MeasureDirection::ParentToChild));
    let an = fo.axis_noise.as_ref().expect("axis-noise");
    let force = an.force.as_ref().expect("force axis-noise");
    assert_eq!(
        force.x.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.1")
    );
    assert!(force.y.is_none(), "force y carried no noise");
    assert_eq!(
        force.z.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.3")
    );
    assert_eq!(
        an.torque
            .as_ref()
            .and_then(|t| t.y.as_ref())
            .and_then(|n| n.stddev.as_deref()),
        Some("0.02")
    );
    // scalar fallback is the x-axis representative (torque had no x, so force/x wins).
    assert_eq!(
        fo.noise.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.1")
    );

    // Round-trip to SDF and re-import.
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains("<frame>sensor</frame>"),
        "frame re-emitted: {out}"
    );
    assert!(
        out.contains("<measure_direction>parent_to_child</measure_direction>"),
        "{out}"
    );
    assert!(
        out.contains("<force>") && out.contains("<z>"),
        "per-axis force noise re-emitted: {out}"
    );
    let (doc2, _) = from_sdf_str(&out).expect("re-import");
    let fo2 = &doc2.comp[0].sensor[0].force[0];
    assert_eq!(fo2.frame, Some(ForceFrame::Sensor));
    assert_eq!(
        fo2.axis_noise
            .as_ref()
            .and_then(|an| an.force.as_ref())
            .and_then(|f| f.z.as_ref())
            .and_then(|n| n.stddev.as_deref()),
        Some("0.3")
    );
}

#[test]
fn sdf_model_link_sim_scalars_round_trip_via_gazebo_ext() {
    // Model/link simulation scalars (static, self_collide, canonical_link, gravity, kinematic,
    // velocity_decay) have no cyber-physical-core home; they round-trip SDF -> HCDF -> SDF through the
    // typed org.gazebosim <model-physics>/<link-physics> extension.
    let sdf = r#"<sdf version="1.12"><model name="m" canonical_link="base">
        <static>false</static>
        <self_collide>true</self_collide>
        <link name="base">
          <gravity>true</gravity>
          <kinematic>false</kinematic>
          <velocity_decay><linear>0.01</linear><angular>0.02</angular></velocity_decay>
        </link>
        <link name="tip"><self_collide>true</self_collide></link>
        </model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let gz = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim")
        .expect("org.gazebosim extension");
    assert!(
        gz.body.contains("<model-physics>"),
        "model-physics captured: {}",
        gz.body
    );
    assert!(
        gz.body.contains("<canonical-link>base</canonical-link>"),
        "{}",
        gz.body
    );
    assert!(
        gz.body.contains(r#"<link-physics name="base">"#),
        "base link-physics: {}",
        gz.body
    );
    assert!(
        gz.body.contains("<velocity-decay><linear>0.01</linear>"),
        "velocity-decay: {}",
        gz.body
    );

    // Export re-emits the scalars onto <model>/<link>.
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains(r#"canonical_link="base""#),
        "canonical_link attr re-emitted: {out}"
    );
    assert!(
        out.contains("<self_collide>true</self_collide>"),
        "self_collide re-emitted: {out}"
    );
    assert!(
        out.contains("<gravity>true</gravity>"),
        "link gravity re-emitted: {out}"
    );
    // (the writer pretty-prints, so match the child leaves + the wrapper independently)
    assert!(
        out.contains("<velocity_decay>"),
        "velocity_decay wrapper re-emitted: {out}"
    );
    assert!(
        out.contains("<linear>0.01</linear>"),
        "velocity_decay linear re-emitted: {out}"
    );
    assert!(
        out.contains("<angular>0.02</angular>"),
        "velocity_decay angular re-emitted: {out}"
    );

    // Full round-trip: re-import the exported SDF and confirm the scalars persist.
    let (doc2, _) = from_sdf_str(&out).expect("re-import");
    let gz2 = doc2
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim")
        .expect("gz ext");
    assert!(
        gz2.body.contains(r#"<link-physics name="tip">"#),
        "tip link-physics survived: {}",
        gz2.body
    );
    assert!(
        gz2.body.contains("<canonical-link>base</canonical-link>"),
        "{}",
        gz2.body
    );
}

#[test]
fn sdf_friction_direction_fdir1_slip_round_trip() {
    // SDF surface/friction/ode mu2 + fdir1 + slip1/slip2 (anisotropic friction) survive
    // SDF -> HCDF -> SDF. The importer relocates mu2 from friction @dynamic to friction-direction/@mu2 (its
    // true meaning: the SECOND-friction-direction coefficient, not kinetic friction).
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <collision name="c"><geometry><box><size>1 1 1</size></box></geometry>
        <surface><friction><ode>
          <mu>0.9</mu>
          <mu2>0.7</mu2>
          <fdir1>1 0 0</fdir1>
          <slip1>0.01</slip1>
          <slip2>0.02</slip2>
        </ode></friction></surface></collision></link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let fr = doc.comp[0].collision[0]
        .surface
        .as_ref()
        .and_then(|s| s.friction.as_ref())
        .expect("friction");
    // @static keeps mu (mu1); @dynamic is UNSET (SDF has no kinetic-friction leaf); mu2 lands
    // in friction-direction/@mu2.
    assert_eq!(fr.static_.as_deref(), Some("0.9"));
    assert!(
        fr.dynamic.is_none(),
        "@dynamic unset: SDF mu2 is not kinetic friction"
    );
    let fd = fr.friction_direction.as_ref().expect("friction-direction");
    assert_eq!(
        fd.mu2.as_deref(),
        Some("0.7"),
        "mu2 -> friction-direction/@mu2"
    );
    assert_eq!(fd.fdir1.as_deref(), Some("1 0 0"));
    assert_eq!(fd.slip1.as_deref(), Some("0.01"));
    assert_eq!(fd.slip2.as_deref(), Some("0.02"));

    // Round-trip to SDF and re-import.
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains("<fdir1>1 0 0</fdir1>"),
        "fdir1 re-emitted: {out}"
    );
    assert!(
        out.contains("<slip1>0.01</slip1>"),
        "slip1 re-emitted: {out}"
    );
    assert!(
        out.contains("<slip2>0.02</slip2>"),
        "slip2 re-emitted: {out}"
    );
    // exactly one <mu2> leaf (no duplicate from friction-direction).
    assert_eq!(out.matches("<mu2>").count(), 1, "single mu2 leaf: {out}");
    let (doc2, _) = from_sdf_str(&out).expect("re-import");
    let fd2 = doc2.comp[0].collision[0]
        .surface
        .as_ref()
        .and_then(|s| s.friction.as_ref())
        .and_then(|f| f.friction_direction.as_ref())
        .expect("friction-direction re-imported");
    assert_eq!(
        fd2.mu2.as_deref(),
        Some("0.7"),
        "mu2 survives the SDF round-trip in friction-direction"
    );
    assert_eq!(fd2.fdir1.as_deref(), Some("1 0 0"));
    assert_eq!(fd2.slip2.as_deref(), Some("0.02"));
}

#[test]
fn sdf_bullet_friction_contact_captured_into_typed_surface() {
    // A Bullet-authored <surface> (friction/friction2/fdir1 + contact kp/kd) is read into the
    // engine-neutral typed HCDF surface fields (previously the importer descended only friction/ode +
    // top-level mu, so a Bullet SDF lost ALL friction/contact). rolling_friction + soft_cfm/soft_erp
    // have no HCDF home and are NOTED, not silently dropped.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <collision name="c"><geometry><box><size>1 1 1</size></box></geometry>
        <surface>
          <friction><bullet>
            <friction>1.1</friction>
            <friction2>0.8</friction2>
            <fdir1>0 1 0</fdir1>
            <rolling_friction>0.05</rolling_friction>
          </bullet></friction>
          <contact><bullet>
            <kp>1e6</kp>
            <kd>120</kd>
            <soft_cfm>0.001</soft_cfm>
          </bullet></contact>
        </surface></collision></link></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let surf = doc.comp[0].collision[0].surface.as_ref().expect("surface");
    let fr = surf.friction.as_ref().expect("friction");
    assert_eq!(
        fr.static_.as_deref(),
        Some("1.1"),
        "bullet friction -> @static"
    );
    assert!(
        fr.dynamic.is_none(),
        "@dynamic unset: bullet friction2 is not kinetic friction"
    );
    let fd = fr.friction_direction.as_ref().expect("friction-direction");
    assert_eq!(
        fd.mu2.as_deref(),
        Some("0.8"),
        "bullet friction2 -> friction-direction/@mu2"
    );
    assert_eq!(
        fd.fdir1.as_deref(),
        Some("0 1 0"),
        "bullet fdir1 -> friction-direction"
    );
    let ct = surf.contact.as_ref().expect("contact");
    assert_eq!(
        ct.stiffness.as_deref(),
        Some("1e6"),
        "bullet kp -> @stiffness"
    );
    assert_eq!(ct.damping.as_deref(), Some("120"), "bullet kd -> @damping");
    // no-home bullet knobs are noted (never silently dropped).
    assert!(
        notes.iter().any(|n| n.contains("rolling_friction")),
        "rolling_friction noted: {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("soft_cfm")),
        "soft_cfm noted: {notes:?}"
    );

    // Export re-emits the captured values under the engine-neutral <ode> block.
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(out.contains("<mu>1.1</mu>"), "friction re-emitted: {out}");
    assert!(
        out.contains("<mu2>0.8</mu2>"),
        "friction2 re-emitted: {out}"
    );
    assert!(out.contains("<kp>1e6</kp>"), "kp re-emitted: {out}");
    assert!(out.contains("<kd>120</kd>"), "kd re-emitted: {out}");
}

#[test]
fn sdf_camera_projection_matrix_round_trips_sdf_hcdf_sdf() {
    // SDF <lens><projection> (p_fx/p_fy/p_cx/p_cy/tx/ty) -> the ROS CameraInfo P matrix inside
    // <camera-matrix>, and back to <lens><projection> on export.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <sensor name="cam0" type="camera"><camera>
          <image><width>640</width><height>480</height></image>
          <lens>
            <intrinsics><fx>320</fx><fy>320</fy><cx>320</cx><cy>240</cy><s>0</s></intrinsics>
            <projection><p_fx>321</p_fx><p_fy>322</p_fy><p_cx>323</p_cx><p_cy>241</p_cy><tx>-40</tx><ty>0</ty></projection>
          </lens>
        </camera></sensor></link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let fov = &doc.comp[0].sensor[0].optical[0].fov[0];
    let proj = fov
        .camera_matrix
        .as_ref()
        .and_then(|cm| cm.projection.as_ref())
        .expect("projection P");
    assert_eq!(proj.fx.as_deref(), Some("321"));
    assert_eq!(proj.cx.as_deref(), Some("323"));
    assert_eq!(proj.tx.as_deref(), Some("-40"));
    // Round-trip: <projection> re-appears and re-imports identically.
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(out.contains("<projection>"), "projection re-emitted: {out}");
    assert!(out.contains("<p_fx>321</p_fx>"), "{out}");
    assert!(out.contains("<tx>-40</tx>"), "{out}");
    let (doc2, _) = from_sdf_str(&out).expect("re-import");
    let proj2 = doc2.comp[0].sensor[0].optical[0].fov[0]
        .camera_matrix
        .as_ref()
        .and_then(|cm| cm.projection.as_ref())
        .expect("projection2");
    assert_eq!(proj2.fx.as_deref(), Some("321"));
    assert_eq!(proj2.ty.as_deref(), Some("0"));
}

/// Validate a `<gazebo-sim>` extension body against the frozen `extensions/hcdf-ext-gazebo.xsd` with
/// xmllint. Skips cleanly when xmllint or the schema is unreachable (packaged crate / minimal image).
fn assert_gazebo_ext_body_valid(body: &str) {
    let schema =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extensions/hcdf-ext-gazebo.xsd");
    if !schema.is_file() {
        eprintln!("skipping xmllint: gazebo extension schema not reachable");
        return;
    }
    let have_xmllint = Command::new("xmllint")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !have_xmllint {
        eprintln!("skipping xmllint: not installed");
        return;
    }
    let f = write_fixture("gazebo_ext_body", "xml", body);
    let out = Command::new("xmllint")
        .args(["--noout", "--schema"])
        .arg(&schema)
        .arg(&f)
        .output()
        .expect("run xmllint");
    let _ = std::fs::remove_file(&f);
    assert!(
        out.status.success(),
        "gazebo extension body must validate against hcdf-ext-gazebo.xsd:\n{}\nbody: {body}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A model `<plugin>` and a world `<physics>` are re-homed into ONE typed
/// `<extension domain="org.gazebosim">` whose body is a `<gazebo-sim>` root (schema
/// `extensions/hcdf-ext-gazebo.xsd`): schema-typed, not dropped and not opaque bytes, and that body
/// VALIDATES against the gazebo extension schema (proved with xmllint when present).
#[test]
fn sdf_plugin_and_physics_import_into_typed_gazebo_extension() {
    let sdf = r#"<sdf version="1.12">
      <world name="w">
        <physics type="ode"><max_step_size>0.001</max_step_size><real_time_factor>1.0</real_time_factor><max_contacts>20</max_contacts></physics>
        <plugin name="scene" filename="libSceneBroadcaster.so"/>
      </world>
      <model name="m">
        <link name="l"><inertial><mass>1</mass></inertial></link>
        <plugin name="motor_model" filename="libMotorModelPlugin.so"><joint>l</joint><turning_direction>ccw</turning_direction></plugin>
      </model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let ext = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim")
        .expect("typed org.gazebosim extension");
    // TYPED root, NOT bare <plugin>/<physics> directly under <extension> (which the schema rejects).
    assert!(
        ext.body.starts_with("<gazebo-sim>"),
        "typed <gazebo-sim> root: {}",
        ext.body
    );
    // physics translated: SDF @type -> <engine>, SDF underscores -> schema hyphens.
    assert!(
        ext.body.contains("<engine>ode</engine>"),
        "@type -> <engine>: {}",
        ext.body
    );
    assert!(
        ext.body.contains("<max-step-size>0.001</max-step-size>"),
        "max_step_size -> max-step-size: {}",
        ext.body
    );
    assert!(
        ext.body
            .contains("<real-time-factor>1.0</real-time-factor>"),
        "real_time_factor -> real-time-factor: {}",
        ext.body
    );
    // BOTH the world-scope and the model-scope plugin land here, name/filename + subtree preserved.
    assert!(
        ext.body.contains(r#"name="motor_model""#) && ext.body.contains("libMotorModelPlugin.so"),
        "model plugin: {}",
        ext.body
    );
    assert!(
        ext.body
            .contains("<turning_direction>ccw</turning_direction>"),
        "plugin subtree preserved: {}",
        ext.body
    );
    assert!(
        ext.body.contains(r#"name="scene""#) && ext.body.contains("libSceneBroadcaster.so"),
        "world plugin: {}",
        ext.body
    );
    // the untyped physics child is noted, never silently dropped.
    assert!(
        notes.iter().any(|n| n.contains("max_contacts")),
        "unmapped physics child noted: {notes:?}"
    );
    // pure plugin/physics never touches the raw passthrough domain.
    assert!(
        !doc.extension
            .iter()
            .any(|e| e.domain == "org.gazebosim.raw"),
        "no raw domain for pure plugin/physics: {:?}",
        doc.extension
            .iter()
            .map(|e| e.domain.as_str())
            .collect::<Vec<_>>()
    );
    // the lax xs:any body round-trips through the HCDF reader/writer byte-stable.
    let xml = doc.to_xml_string().expect("serialize");
    let doc2 = hcdformat::Hcdf::from_xml_str(&xml).expect("reparse");
    let body2 = &doc2
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim")
        .expect("ext round-trip")
        .body;
    assert_eq!(
        &ext.body, body2,
        "gazebo-sim body byte-stable across the HCDF round-trip"
    );
    // and the body VALIDATES against the frozen gazebo extension schema.
    assert_gazebo_ext_body_valid(&ext.body);
}

#[test]
fn model_plugin_round_trips_sdf_hcdf_sdf() {
    // A model `<plugin filename= name=>…</plugin>` imports into the typed org.gazebosim extension and,
    // on EXPORT, is re-emitted as a model-scope `<plugin>` (the exact inverse of the from_sdf harvest),
    // so a controller/ros2_control/motor plugin no longer vanishes on the HCDF round-trip.
    let sdf = r#"<sdf version="1.12"><model name="m">
        <link name="l"><inertial><mass>1</mass></inertial></link>
        <plugin name="motor_model" filename="libMotorModelPlugin.so"><joint>l</joint><turning_direction>ccw</turning_direction></plugin>
      </model></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    assert!(
        doc.extension.iter().any(|e| e.domain == "org.gazebosim"),
        "plugin imported into the typed extension"
    );
    // EXPORT: the plugin must reappear as a model-scope <plugin>, name/filename + subtree intact
    // (attribute ORDER is source-order-preserving via el_to_xml, so assert each independently).
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains("<plugin "),
        "a <plugin> element is re-emitted:\n{out}"
    );
    assert!(
        out.contains(r#"name="motor_model""#),
        "plugin name re-emitted:\n{out}"
    );
    assert!(
        out.contains(r#"filename="libMotorModelPlugin.so""#),
        "plugin filename re-emitted:\n{out}"
    );
    assert!(
        out.contains("<joint>l</joint>"),
        "plugin subtree re-emitted:\n{out}"
    );
    assert!(
        out.contains("<turning_direction>ccw</turning_direction>"),
        "plugin params re-emitted:\n{out}"
    );
    // and it re-imports cleanly, landing back in the typed extension (full round-trip).
    let (doc2, _) = from_sdf_str(&out).expect("re-import emitted SDF");
    let ext2 = doc2
        .extension
        .iter()
        .find(|e| e.domain == "org.gazebosim")
        .expect("plugin survives SDF->HCDF->SDF->HCDF");
    assert!(
        ext2.body.contains(r#"name="motor_model""#),
        "plugin name survives the round-trip"
    );
    assert!(
        ext2.body.contains("libMotorModelPlugin.so"),
        "plugin filename survives the round-trip"
    );
}

#[test]
fn named_frame_round_trips_sdf_hcdf_sdf() {
    // A named kinematic <frame> attached to a link imports onto that link's comp and, on EXPORT, is
    // re-emitted as a model-level SDF <frame name attached_to><pose>; SDFormat >=1.7 has a first-class
    // <frame>, so the old "no SDF home" drop was factually wrong. The frame survives SDF->HCDF->SDF.
    let sdf = r#"<sdf version="1.12"><model name="m">
        <link name="base"><inertial><mass>1</mass></inertial></link>
        <frame name="tool" attached_to="base"><pose>0.1 0 0.2 0 0 0</pose></frame>
      </model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let base = doc
        .comp
        .iter()
        .find(|c| c.name == "base")
        .expect("base comp");
    assert_eq!(
        base.frame.len(),
        1,
        "named frame imports onto the owner link's comp: {notes:?}"
    );
    assert_eq!(base.frame[0].name, "tool");

    let (out, loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains(r#"<frame name="tool" attached_to="base">"#),
        "named frame re-emitted as a model-level SDF <frame>:\n{out}"
    );
    assert!(
        out.contains("<pose>0.1 0 0.2 0 0 0</pose>"),
        "frame pose re-emitted:\n{out}"
    );
    // the false "N frames dropped (no SDF home)" loss must be GONE.
    assert!(
        !loss.text().contains("frames dropped"),
        "no false frame-drop loss may remain: {}",
        loss.text()
    );

    let (doc2, _) = from_sdf_str(&out).expect("re-import emitted SDF");
    let base2 = doc2
        .comp
        .iter()
        .find(|c| c.name == "base")
        .expect("base comp2");
    assert_eq!(base2.frame.len(), 1, "frame survives SDF->HCDF->SDF->HCDF");
    assert_eq!(base2.frame[0].name, "tool");
    assert!(
        base2.frame[0].pose.is_some(),
        "frame pose survives the round-trip"
    );
}

#[test]
fn gazebo_physics_export_noted_not_emitted_as_invalid_model_child() {
    // SDFormat <physics> is WORLD-scope; this exporter emits a bare <model>. The physics carried in the
    // org.gazebosim extension must therefore be LOSS-NOTED, never emitted as a schema-invalid
    // <model><physics> (which libsdformat rejects). The model-scope plugin still round-trips.
    let sdf = r#"<sdf version="1.12">
      <world name="w"><physics type="ode"><max_step_size>0.001</max_step_size></physics></world>
      <model name="m"><link name="l"><inertial><mass>1</mass></inertial></link>
        <plugin name="p" filename="libP.so"/></model></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    let (out, loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        !out.contains("<physics"),
        "no <physics> emitted into <model>:\n{out}"
    );
    assert!(
        out.contains("<plugin ")
            && out.contains(r#"name="p""#)
            && out.contains(r#"filename="libP.so""#),
        "plugin still emitted:\n{out}"
    );
    assert!(
        loss.text()
            .contains("org.gazebosim <physics> not re-emitted"),
        "world-scope physics drop must be noted: {}",
        loss.text()
    );
}

#[test]
fn sdf_lidar_sensor_maps_range_and_scan_pattern() {
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <sensor name="lidar0" type="gpu_lidar"><lidar>
          <scan>
            <horizontal><samples>360</samples><resolution>1</resolution>
              <min_angle>-3.14159</min_angle><max_angle>3.14159</max_angle></horizontal>
            <vertical><samples>16</samples><resolution>1</resolution>
              <min_angle>-0.26</min_angle><max_angle>0.26</max_angle></vertical>
          </scan>
          <range><min>0.08</min><max>30</max><resolution>0.01</resolution></range>
        </lidar></sensor></link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    use hcdformat::model::enums::OpticalSensorType;
    let opt = &doc.comp[0].sensor[0].optical[0];
    assert_eq!(opt.type_, Some(OpticalSensorType::Lidar));
    let lp = opt.lidar_params.as_ref().expect("lidar-params");
    let rng = lp.range.as_ref().expect("range");
    assert_eq!(
        rng.min.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.08")
    );
    assert_eq!(
        rng.max.as_ref().and_then(|v| v.value.as_deref()),
        Some("30")
    );
    assert_eq!(
        rng.resolution.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.01")
    );
    let sp = lp.scan_pattern.as_ref().expect("scan-pattern");
    let h = sp.horizontal.as_ref().expect("horizontal");
    assert_eq!(h.samples.as_deref(), Some("360"));
    assert_eq!(h.min_angle.as_deref(), Some("-3.14159"));
    assert_eq!(h.max_angle.as_deref(), Some("3.14159"));
    assert_eq!(
        sp.vertical.as_ref().and_then(|v| v.samples.as_deref()),
        Some("16")
    );
}

#[test]
fn sdf_lidar_noise_round_trips_sdf_hcdf_sdf() {
    // The lidar beam <noise> maps into <lidar-params><noise> and re-emits inside <lidar>.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <sensor name="lidar0" type="gpu_lidar"><lidar>
          <range><min>0.08</min><max>30</max><resolution>0.01</resolution></range>
          <noise type="gaussian"><mean>0</mean><stddev>0.008</stddev></noise>
        </lidar></sensor></link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let lp = doc.comp[0].sensor[0].optical[0]
        .lidar_params
        .as_ref()
        .expect("lidar-params");
    let n = lp.noise.as_ref().expect("lidar noise");
    use hcdformat::model::enums::NoiseType;
    assert_eq!(n.type_, Some(NoiseType::Gaussian));
    assert_eq!(n.stddev.as_deref(), Some("0.008"));
    // Round-trip: the <noise> re-appears in the emitted SDF and re-imports identically.
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(out.contains("<lidar>"), "lidar body re-emitted: {out}");
    assert!(
        out.contains("<stddev>0.008</stddev>"),
        "beam noise re-emitted: {out}"
    );
    let (doc2, _) = from_sdf_str(&out).expect("re-import");
    assert_eq!(
        doc2.comp[0].sensor[0].optical[0]
            .lidar_params
            .as_ref()
            .and_then(|lp| lp.noise.as_ref())
            .and_then(|n| n.stddev.as_deref()),
        Some("0.008")
    );
}

#[test]
fn sdf_gpu_ray_sensor_maps_to_typed_lidar() {
    // The historical GPU-lidar type name `gpu_ray` must dispatch to the same lidar path
    // as `gpu_lidar`/`ray`, not be dropped-with-note.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <sensor name="lidar0" type="gpu_ray"><ray>
          <range><min>0.08</min><max>30</max><resolution>0.01</resolution></range>
        </ray></sensor></link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    use hcdformat::model::enums::OpticalSensorType;
    let opt = &doc.comp[0].sensor[0].optical[0];
    assert_eq!(opt.type_, Some(OpticalSensorType::Lidar));
    let rng = opt
        .lidar_params
        .as_ref()
        .and_then(|lp| lp.range.as_ref())
        .expect("lidar range");
    assert_eq!(
        rng.min.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.08")
    );
    assert_eq!(
        rng.max.as_ref().and_then(|v| v.value.as_deref()),
        Some("30")
    );
}

#[test]
fn sdf_fluid_mag_gnss_contact_sensors_map() {
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <sensor name="baro0" type="air_pressure"><air_pressure>
          <reference_altitude>0</reference_altitude>
          <pressure><noise type="gaussian"><mean>0</mean><stddev>0.5</stddev></noise></pressure>
        </air_pressure></sensor>
        <sensor name="mag0" type="magnetometer"><magnetometer>
          <x><noise type="gaussian"><stddev>0.0001</stddev></noise></x>
          <y><noise type="gaussian"><stddev>0.0001</stddev></noise></y>
        </magnetometer></sensor>
        <sensor name="gps0" type="navsat"><navsat>
          <position_sensing><horizontal><noise type="gaussian"><stddev>1.5</stddev></noise></horizontal></position_sensing>
        </navsat></sensor>
        <sensor name="bump0" type="contact"><contact><collision>l_collision</collision><topic>/bumper</topic></contact></sensor>
        </link></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let l = &doc.comp[0];
    // barometer (new fluid category)
    let baro = l
        .sensor
        .iter()
        .find(|s| s.name.as_deref() == Some("baro0"))
        .expect("baro0");
    let f = &baro.fluid[0];
    assert_eq!(f.type_.as_deref(), Some("barometer"));
    assert_eq!(
        f.noise.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.5")
    );
    assert!(
        notes.iter().any(|n| n.contains("reference_altitude")),
        "baro reference noted: {notes:?}"
    );
    // magnetometer -> em type=mag
    let mag = l
        .sensor
        .iter()
        .find(|s| s.name.as_deref() == Some("mag0"))
        .expect("mag0");
    assert_eq!(mag.em[0].type_.as_deref(), Some("mag"));
    assert_eq!(
        mag.em[0].noise.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("0.0001")
    );
    // navsat -> rf type=gnss
    let gps = l
        .sensor
        .iter()
        .find(|s| s.name.as_deref() == Some("gps0"))
        .expect("gps0");
    assert_eq!(gps.rf[0].type_.as_deref(), Some("gnss"));
    assert_eq!(
        gps.rf[0].noise.as_ref().and_then(|n| n.stddev.as_deref()),
        Some("1.5")
    );
    // contact -> force type=pressure. The monitored <collision> is now captured on
    // the typed sensor (no longer note-dropped) and the <topic> is preserved in the org.ros2 extension.
    let bump = l
        .sensor
        .iter()
        .find(|s| s.name.as_deref() == Some("bump0"))
        .expect("bump0");
    assert_eq!(bump.force[0].type_.as_deref(), Some("pressure"));
    assert_eq!(
        bump.force[0].collision.as_deref(),
        Some("l_collision"),
        "collision-ref captured"
    );
    let _ = notes; // collision/topic are consumed (no longer note-dropped)
    let ros2 = doc
        .extension
        .iter()
        .find(|e| e.domain == "org.ros2")
        .expect("org.ros2 topic-map extension");
    assert!(
        ros2.body.contains("sensor=\"bump0\""),
        "topic-map names the sensor: {}",
        ros2.body
    );
    assert!(
        ros2.body.contains("name=\"/bumper\""),
        "topic-map carries the SDF topic: {}",
        ros2.body
    );
}

#[test]
fn sdf_contact_sensor_round_trips_to_valid_sdf() {
    // A contact sensor imports its monitored <collision> (core) + <topic> (org.ros2 extension), and
    // EXPORTS a VALID <sensor type="contact"> WITH the required <contact><collision>+<topic> body: the
    // fix for the dead-sensor bug (the old exporter emitted a bare <sensor type="contact"/>).
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
        <collision name="l_collision"><geometry><box><size>1 1 1</size></box></geometry></collision>
        <sensor name="bump0" type="contact"><update_rate>50</update_rate>
          <contact><collision>l_collision</collision><topic>/bumper</topic></contact>
        </sensor></link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(
        doc.comp[0].sensor[0].force[0].collision.as_deref(),
        Some("l_collision")
    );

    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains(r#"<sensor name="bump0" type="contact">"#),
        "contact sensor opened: {out}"
    );
    assert!(
        out.contains("<contact>"),
        "contact body emitted (not bare): {out}"
    );
    assert!(
        out.contains("<collision>l_collision</collision>"),
        "collision-ref re-emitted: {out}"
    );
    assert!(
        out.contains("<topic>/bumper</topic>"),
        "topic re-emitted: {out}"
    );

    // Re-import the exported SDF: collision + topic survive the full round-trip.
    let (doc2, _) = from_sdf_str(&out).expect("re-import");
    assert_eq!(
        doc2.comp[0].sensor[0].force[0].collision.as_deref(),
        Some("l_collision")
    );
    let ros2 = doc2
        .extension
        .iter()
        .find(|e| e.domain == "org.ros2")
        .expect("org.ros2 ext");
    assert!(
        ros2.body.contains("name=\"/bumper\""),
        "topic survived round-trip: {}",
        ros2.body
    );
}

#[test]
fn b3rb_model_sdf_camera_and_lidar_sensors_import() {
    // Skipped-if-absent real-robot check: b3rb's camera_link carries an optical/camera sensor with a
    // FoV + intrinsics, and lidar_link an optical/lidar sensor with a <range>.
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let path =
        home.join("cognipilot/cranium/src/b3rb_simulator/b3rb_gz_resource/models/b3rb/model.sdf");
    if !path.is_file() {
        eprintln!("skipping b3rb sensor test: {} absent", path.display());
        return;
    }
    let (doc, _notes) = from_sdf_path(&path).expect("from_sdf_path b3rb");
    use hcdformat::model::enums::OpticalSensorType;
    // camera_link -> optical camera with intrinsics (width 320) + frustum from <clip>.
    let cam_comp = doc
        .comp
        .iter()
        .find(|c| c.name == "camera_link")
        .expect("camera_link comp");
    let cam = cam_comp
        .sensor
        .iter()
        .find(|s| !s.optical.is_empty())
        .expect("camera_link sensor");
    let opt = &cam.optical[0];
    assert_eq!(opt.type_, Some(OpticalSensorType::Camera));
    let intr = opt.fov[0].intrinsics.as_ref().expect("camera intrinsics");
    assert_eq!(intr.width.as_deref(), Some("320"));
    assert_eq!(intr.height.as_deref(), Some("240"));
    let fr = opt.fov[0]
        .geometry
        .as_ref()
        .and_then(|g| g.frustum.as_ref())
        .expect("frustum");
    assert_eq!(fr.near.as_deref(), Some("0.05"));
    // lidar_link -> optical lidar with range (min 0.03).
    let lidar_comp = doc
        .comp
        .iter()
        .find(|c| c.name == "lidar_link")
        .expect("lidar_link comp");
    let lidar = lidar_comp
        .sensor
        .iter()
        .find(|s| !s.optical.is_empty())
        .expect("lidar_link sensor");
    let lopt = &lidar.optical[0];
    assert_eq!(lopt.type_, Some(OpticalSensorType::Lidar));
    let rng = lopt
        .lidar_params
        .as_ref()
        .and_then(|lp| lp.range.as_ref())
        .expect("lidar range");
    assert_eq!(
        rng.min.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.03")
    );
    assert_eq!(
        rng.max.as_ref().and_then(|v| v.value.as_deref()),
        Some("20")
    );
    // the b3rb IMU/mag/altimeter/navsat live on the 'sensors' link.
    let sensors_link = doc
        .comp
        .iter()
        .find(|c| c.name == "sensors")
        .expect("sensors comp");
    assert!(
        sensors_link.sensor.iter().any(|s| !s.inertial.is_empty()),
        "imu mapped on the sensors link"
    );
    assert!(
        sensors_link.sensor.iter().any(|s| !s.fluid.is_empty()),
        "altimeter -> fluid barometer on the sensors link"
    );
}

#[test]
fn multi_model_imports_first_only_matches_python() {
    // Two top-level <model>s + a nested <model> in the first: Python imports ONLY the first top model and
    // drops the nested one (both noted). Rust must select identically.
    let sdf = r#"<sdf version="1.12">
        <model name="first">
          <link name="a"><inertial><mass>1</mass></inertial></link>
          <model name="nested_in_first"><link name="ninner"/></model>
        </model>
        <model name="second"><link name="b"/></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(doc.name, "first", "first top-level model selected");
    assert_eq!(
        doc.comp.len(),
        1,
        "only link 'a' (nested model + second model dropped)"
    );
    assert!(
        notes.iter().any(|n| n.contains("only the first")),
        "multi-model note: {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("nested")),
        "nested-model note: {notes:?}"
    );
    assert_sdf_import_parity("multi+nested model [sdf->hcdf]", sdf);
}

#[test]
fn world_nested_model_selected_when_no_top_model_matches_python() {
    // No top-level <model>; the model lives in a <world>. Python falls back to the world-nested model.
    let sdf = r#"<sdf version="1.12"><world name="w">
        <model name="inworld"><link name="wl"><inertial><mass>3</mass></inertial></link></model>
        </world></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(
        doc.name, "inworld",
        "world-nested model selected when no top-level model"
    );
    assert_sdf_import_parity("world-nested model [sdf->hcdf]", sdf);
}

#[test]
fn include_bearing_sdf_maps_inline_model_without_gz_matches_python() {
    // An SDF carrying an UNRESOLVED <include> (model:// uri) plus an inline <model>: with gz absent,
    // Python maps the inline model and simply ignores the include (it never shells to gz). Rust must do
    // the same: no gz, no error, the inline model imported. (The previous needs_resolve heuristic forced
    // a gz path here and rejected gz-absent; this pins the corrected behavior.)
    let sdf = r#"<sdf version="1.12"><world name="w">
        <include><uri>model://ground_plane</uri></include>
        <model name="m"><link name="l"><inertial><mass>1</mass></inertial></link></model>
        </world></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("include-bearing SDF imports raw, no gz");
    assert_eq!(
        doc.name, "m",
        "the inline model is imported (include ignored, not gz-resolved)"
    );
    assert_eq!(doc.comp.len(), 1);
    assert_sdf_import_parity("include-bearing [sdf->hcdf raw]", sdf);
}

#[test]
fn frd_nontrivial_rotation_export_matches_frozen_golden() {
    // An FRD HCDF with a NON-trivial joint rotation (rpy != 0) exported to SDF: the full FRD->FLU pose
    // conversion (R' = C·R·Cᵀ, t' = C·t through matrix_to_pose), not just the axis vector, is frozen
    // byte-for-byte to the Rust exporter's output (a self-contained inline fixture, no Python oracle).
    let hcdf = r#"<hcdf name="frd" body-frame="FRD" world-frame="ENU">
        <comp name="a"><inertial><mass>1</mass></inertial></comp>
        <comp name="b"/>
        <joint name="j" type="revolute"><parent comp="a"/><child comp="b"/>
          <origin xyz="0.1 0.2 0.3" rpy="0.5 -0.3 0.7"/>
          <axis xyz="0 1 0"/><limit lower="-1" upper="1" effort="1" velocity="1"/></joint></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse FRD hcdf");
    let (rust_sdf, _) = to_sdf(&doc).expect("to_sdf");
    assert_golden("sdf/frd-nontrivial-rotation.export.sdf", &rust_sdf);
}

// ── (d) round-trip + CLI smoke (no Python needed for the round-trip itself) ───────────────────────

#[test]
fn sdf_round_trips_through_hcdf() {
    // A minimal SDF exercising the overlap: two links, a revolute joint with axis+limit+dynamics, a box
    // visual with a diffuse color, and a collision surface. SDF -> HCDF -> SDF -> HCDF must be stable.
    let sdf = r#"<sdf version="1.12"><model name="bot">
        <link name="base">
          <inertial><mass>2.0</mass><inertia><ixx>0.1</ixx><iyy>0.1</iyy><izz>0.1</izz></inertia></inertial>
          <visual name="bv"><geometry><box><size>1 1 1</size></box></geometry>
            <material><diffuse>0.8 0.2 0.2 1</diffuse></material></visual>
          <collision name="bc"><geometry><box><size>1 1 1</size></box></geometry>
            <surface><friction><ode><mu>0.8</mu><mu2>0.6</mu2></ode></friction>
              <bounce><restitution_coefficient>0.2</restitution_coefficient></bounce></surface></collision>
        </link>
        <link name="link1"/>
        <joint name="j1" type="revolute"><parent>base</parent><child>link1</child>
          <axis><xyz>0 0 1</xyz><limit><lower>-1.5</lower><upper>1.5</upper></limit>
            <dynamics><damping>0.1</damping><friction>0.05</friction></dynamics></axis></joint>
      </model></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(doc.name, "bot");
    assert_eq!(doc.comp.len(), 2, "two links -> two comps");
    assert_eq!(doc.joint.len(), 1);
    // surface mapped
    let base = &doc.comp[0];
    let surf = base.collision[0]
        .surface
        .as_ref()
        .expect("collision surface");
    let fric = surf.friction.as_ref().expect("friction");
    assert_eq!(fric.static_.as_deref(), Some("0.8"));
    // mu2 -> friction-direction/@mu2 (not @dynamic, since SDF ode has no kinetic-friction leaf).
    assert!(
        fric.dynamic.is_none(),
        "@dynamic unset: SDF mu2 is not kinetic friction"
    );
    assert_eq!(
        fric.friction_direction
            .as_ref()
            .and_then(|d| d.mu2.as_deref()),
        Some("0.6"),
        "mu2 -> friction-direction/@mu2"
    );
    assert_eq!(surf.restitution.as_deref(), Some("0.2"));
    // joint axis/limit/dynamics
    let j = &doc.joint[0];
    assert_eq!(
        j.axis.as_ref().and_then(|a| a.xyz.as_deref()),
        Some("0 0 1")
    );
    let lim = j.limit.as_ref().expect("limit");
    assert_eq!(
        (lim.lower.as_deref(), lim.upper.as_deref()),
        (Some("-1.5"), Some("1.5"))
    );

    // export -> SDF -> re-import: the overlap must survive value-exact.
    let (sdf2, _loss) = to_sdf(&doc).expect("to_sdf");
    let (doc2, _) = from_sdf_str(&sdf2).expect("re-import emitted SDF");
    assert_eq!(doc.name, doc2.name);
    assert_eq!(doc.comp.len(), doc2.comp.len());
    assert_eq!(doc.joint.len(), doc2.joint.len());
    let surf2 = doc2.comp[0].collision[0]
        .surface
        .as_ref()
        .expect("re-imported surface");
    assert_eq!(
        surf2.friction.as_ref().and_then(|f| f.static_.as_deref()),
        Some("0.8")
    );
    assert_eq!(
        surf2
            .friction
            .as_ref()
            .and_then(|f| f.friction_direction.as_ref())
            .and_then(|d| d.mu2.as_deref()),
        Some("0.6"),
        "mu2 survives SDF export+re-import in friction-direction",
    );
    let j2 = &doc2.joint[0];
    assert_eq!(
        j2.axis.as_ref().and_then(|a| a.xyz.as_deref()),
        Some("0 0 1")
    );
}

#[test]
fn continuous_joint_exports_as_unlimited_revolute() {
    // SDFormat has NO `continuous` joint type; libsdformat rejects `<joint type="continuous">` at load.
    // A continuous joint IS an unlimited revolute, so it must export as `type="revolute"` with NO
    // lower/upper bound (an absent limit lower/upper = unlimited). effort/velocity, if present, stay.
    let hcdf = r#"<hcdf name="wheelbot">
        <comp name="chassis"><inertial><mass>1</mass></inertial></comp>
        <comp name="wheel"/>
        <joint name="drive" type="continuous">
          <parent comp="chassis"/><child comp="wheel"/>
          <axis xyz="0 1 0"/>
          <limit effort="5" velocity="20"/>
        </joint></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse continuous-joint hcdf");
    let (sdf, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        sdf.contains(r#"<joint name="drive" type="revolute">"#),
        "continuous joint must export as type=\"revolute\", got:\n{sdf}"
    );
    assert!(
        !sdf.contains(r#"type="continuous""#),
        "no `continuous` SDF joint type may be emitted (libsdformat rejects it):\n{sdf}"
    );
    assert!(
        !sdf.contains("<lower>"),
        "unlimited revolute must have no <lower>:\n{sdf}"
    );
    assert!(
        !sdf.contains("<upper>"),
        "unlimited revolute must have no <upper>:\n{sdf}"
    );
    // effort/velocity are still valid on an unlimited revolute and must survive.
    assert!(
        sdf.contains("<effort>5</effort>"),
        "effort must survive:\n{sdf}"
    );
    assert!(
        sdf.contains("<velocity>20</velocity>"),
        "velocity must survive:\n{sdf}"
    );
    // The emitted SDF must re-import as a valid model (the joint survives as a revolute DOF).
    let (doc2, _) = from_sdf_str(&sdf).expect("re-import of continuous->revolute SDF");
    assert_eq!(doc2.joint.len(), 1, "the joint survives the round-trip");
    assert_eq!(
        doc2.joint[0].type_,
        Some(hcdformat::model::enums::JointType::Revolute)
    );
}

#[test]
fn continuous_joint_drops_contradictory_bounds_with_note() {
    // A continuous joint carrying lower/upper bounds is contradictory (an unlimited revolute has none):
    // the bounds are dropped WITH a loss note, never emitted as a bounded revolute.
    let hcdf = r#"<hcdf name="m">
        <comp name="a"><inertial><mass>1</mass></inertial></comp>
        <comp name="b"/>
        <joint name="j" type="continuous"><parent comp="a"/><child comp="b"/>
          <axis xyz="1 0 0"/><limit lower="-3" upper="3" effort="2" velocity="4"/></joint></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse hcdf");
    let (sdf, loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        !sdf.contains("<lower>") && !sdf.contains("<upper>"),
        "bounds suppressed:\n{sdf}"
    );
    let msgs = loss.text();
    assert!(
        msgs.contains("continuous joint carried limit/lower")
            && msgs.contains("continuous joint carried limit/upper"),
        "dropped continuous bounds must be noted, got: {msgs}"
    );
}

#[test]
fn cli_smoke_convert_sdf_to_hcdf() {
    // `hcdf convert <sdf> <hcdf>` via the built binary (skips if the corpus / binary is absent).
    let files = corpus();
    let Some(sdf) = files.first() else {
        eprintln!("skipping: no corpus SDF for the CLI smoke test");
        return;
    };
    let bin = env!("CARGO_BIN_EXE_hcdf");
    let out = unique_tmp("cli_smoke", "hcdf");
    let mut cmd = Command::new(bin);
    cmd.arg("convert").arg(sdf).arg(&out);
    let status = cmd.output().expect("run hcdf convert");
    let ok = status.status.success();
    let exists = out.exists()
        && std::fs::metadata(&out)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
    let _ = std::fs::remove_file(&out);
    assert!(
        ok,
        "hcdf convert exited non-zero: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(exists, "hcdf convert produced no output file");
}

// ── unit-level parity (no Python / gz needed) ────────────────────────────────────────────────────

#[test]
fn non_sdf_root_is_rejected() {
    assert!(
        from_sdf_str("<notsdf/>").is_err(),
        "non-SDF root must error"
    );
}

#[test]
fn sdf_without_model_is_rejected() {
    // No <model> anywhere -> error (matches from_sdf.py). Use a model-less but otherwise valid fragment;
    // an empty <sdf> fails the structural gate, which is also an error; either way `is_err`.
    assert!(
        from_sdf_str("<sdf version='1.12'></sdf>").is_err(),
        "SDF without a <model> must error"
    );
}

#[test]
fn unmapped_joint_type_downgrades_to_fixed_dropping_axis_limit() {
    // A gearbox is a gear-COUPLING constraint (mapped to a <transmission type="gear">, not a kinematic
    // DOF). Here the gearbox is the ONLY thing parenting child link `b`, so the dangling-child guard also
    // emits a FIXED edge to keep `b` in the tree, and on that fixed edge axis/limit MUST be dropped (a
    // fixed joint forbids them; carrying them would be an invalid HCDF doc). (revolute2 -> universal.)
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="a"/><link name="b"/>
        <joint name="j" type="gearbox"><parent>a</parent><child>b</child>
        <axis><xyz>0 0 1</xyz><limit><lower>-1</lower><upper>1</upper></limit></axis></joint></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    use hcdformat::model::enums::JointType;
    let j = &doc.joint[0];
    assert_eq!(
        j.type_,
        Some(JointType::Fixed),
        "orphaned gearbox child -> fixed edge fallback"
    );
    assert!(j.axis.is_none(), "axis dropped on the fixed downgrade");
    assert!(j.limit.is_none(), "limit dropped on the fixed downgrade");
    assert!(
        notes.iter().any(|n| n.contains("downgrade")),
        "the drop is noted: {notes:?}"
    );
    // The gear coupling itself is preserved as a typed transmission (not lost with the fixed downgrade).
    assert_eq!(
        doc.transmission.len(),
        1,
        "gearbox coupling -> one <transmission>"
    );
}

#[test]
fn partial_inertia_maps_with_sdf_defaults() {
    // A partial <inertia> (off-diagonals omitted) is valid SDF -> must map with SDF defaults, not drop.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="b"><inertial><mass>1</mass>
        <inertia><ixx>0.5</ixx><iyy>0.5</iyy><izz>0.5</izz></inertia></inertial></link></model></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(
        doc.comp[0]
            .inertial
            .as_ref()
            .and_then(|i| i.inertia.as_deref()),
        Some("0.5 0.0 0.0 0.5 0.0 0.5"),
        "partial <inertia> fills SDF defaults (ixx/iyy/izz=1.0, off-diag=0.0)"
    );
}

#[test]
fn capsule_cone_ellipsoid_collisions_survive_export() {
    // SDF is a wider peer than URDF: capsule/cone/ellipsoid collisions must round-trip (URDF drops them).
    let sdf = r#"<sdf version="1.12"><model name="m">
        <link name="l">
          <collision name="cap"><geometry><capsule><radius>0.1</radius><length>0.3</length></capsule></geometry></collision>
          <collision name="con"><geometry><cone><radius>0.1</radius><length>0.3</length></cone></geometry></collision>
          <collision name="ell"><geometry><ellipsoid><radii>0.1 0.2 0.3</radii></ellipsoid></geometry></collision>
        </link></model></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(
        doc.comp[0].collision.len(),
        3,
        "all three closed shapes import"
    );
    let (out, loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains("<capsule>") && out.contains("<cone>") && out.contains("<ellipsoid>"),
        "all three shapes re-emit: {out}"
    );
    assert!(
        !loss.items.iter().any(|(c, _)| c == "geometry"),
        "no geometry loss for the closed shapes: {:?}",
        loss.items
    );
}

#[test]
fn mimic_emitted_under_axis_with_reference() {
    // HCDF->SDF: <mimic> must sit under <axis> with a required <reference> child (SDF 1.12), not under
    // <joint>. Build via an SDF that has a mimic, then re-export.
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="a"/><link name="b"/>
        <joint name="j" type="revolute"><parent>a</parent><child>b</child>
        <axis><xyz>0 0 1</xyz><mimic joint="j0"><multiplier>2</multiplier><offset>0.1</offset></mimic></axis>
        </joint></model></sdf>"#;
    let (doc, _) = from_sdf_str(sdf).expect("from_sdf_str");
    assert!(doc.joint[0].mimic.is_some(), "mimic imported");
    let (out, _) = to_sdf(&doc).expect("to_sdf");
    let axis_pos = out.find("<axis>").expect("axis emitted");
    let mimic_pos = out.find("<mimic").expect("mimic emitted");
    assert!(axis_pos < mimic_pos, "mimic must be inside <axis>");
    assert!(
        out.contains("<reference>"),
        "mimic carries the required <reference>"
    );
}

#[test]
fn frd_body_frame_converts_axis_on_export() {
    // An FRD document: a joint axis must be converted to FLU (a' = C·a, C = diag(1,-1,-1)) on SDF export.
    let hcdf = r#"<hcdf name="frd" body-frame="FRD" world-frame="ENU">
        <comp name="a"/><comp name="b"/>
        <joint name="j" type="revolute"><parent comp="a"/><child comp="b"/>
        <origin xyz="1 2 3" rpy="0 0 0"/><axis xyz="0 1 0"/>
        <limit lower="-1" upper="1" effort="1" velocity="1"/></joint></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse FRD hcdf");
    let (sdf, _) = to_sdf(&doc).expect("to_sdf");
    // axis (0,1,0) -> (0,-1,0). The SDF axis is an element-text leaf `<xyz>0 -1 0</xyz>` under <axis>.
    let nums = numeric_values(&sdf);
    let axis = nums
        .iter()
        .find(|(k, _)| k.contains("axis[") && k.contains("xyz[") && k.ends_with("#text"))
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| {
            panic!(
                "axis xyz text present; keys: {:?}",
                nums.keys().collect::<Vec<_>>()
            )
        });
    assert_eq!(axis, vec![0.0, -1.0, 0.0], "FRD->FLU axis on SDF export");
}

// ── SDF 1.7 pose frame-graph resolution (oracle-FREE: pure-Python from_sdf still flattens; Rust is
// now canonical and intentionally leads on frame semantics) ────────────────────────────────────────

/// The joint with the given name (all frame-graph fixtures use named joints).
fn joint_named<'a>(doc: &'a Hcdf, name: &str) -> &'a hcdformat::model::Joint {
    doc.joint
        .iter()
        .find(|j| j.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("joint '{name}' present"))
}

/// The comp with the given name.
fn comp_named<'a>(doc: &'a Hcdf, name: &str) -> &'a hcdformat::model::Comp {
    doc.comp
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("comp '{name}' present"))
}

/// xyz + rpy of a pose within 1e-9 (the frame-graph numeric bar).
fn assert_pose(what: &str, p: Option<&hcdformat::model::Pose>, xyz: [f64; 3], rpy: [f64; 3]) {
    let p = p.unwrap_or_else(|| panic!("{what}: expected a pose, got None"));
    let (axyz, arpy) = (p.xyz_or_zero(), p.rpy_or_zero());
    for i in 0..3 {
        assert!(
            (axyz[i] - xyz[i]).abs() < 1e-9,
            "{what}: xyz[{i}] = {} != {}",
            axyz[i],
            xyz[i]
        );
        assert!(
            (arpy[i] - rpy[i]).abs() < 1e-9,
            "{what}: rpy[{i}] = {} != {}",
            arpy[i],
            rpy[i]
        );
    }
}

/// The joint's parsed `<axis>` vector.
fn parsed_axis(j: &hcdformat::model::Joint) -> [f64; 3] {
    let s = j
        .axis
        .as_ref()
        .and_then(|a| a.xyz.as_deref())
        .expect("axis xyz present");
    let v: Vec<f64> = s.split_whitespace().map(|t| t.parse().unwrap()).collect();
    assert_eq!(v.len(), 3, "axis '{s}' is a 3-vector");
    [v[0], v[1], v[2]]
}

#[test]
fn sdf_frame_graph_distilled_semantics() {
    // Every frame-graph semantic in ~40 lines: link poses default MODEL-relative; a joint pose defaults
    // CHILD-relative (root_joint: sign flip vs element-local); a pose-less joint derives its origin from
    // the link poses (probe_joint, tool_joint); relative_to may name a joint (knuckle/wheel) or a
    // <frame> (tool); the joint origin is X(parent<-child) throughout.
    let sdf = r#"<sdf version="1.10"><model name="fg">
        <link name="root"/>
        <link name="base"><pose>0 0 0.04 0 0 0</pose></link>
        <link name="probe"><pose>-0.168 0.05 0.115 0 1.57 0</pose></link>
        <link name="knuckle"><pose relative_to="steer_joint">0 0 0 0 0 0</pose></link>
        <link name="wheel"><pose relative_to="wheel_joint">0 0 0 0 0 0</pose></link>
        <frame name="mount" attached_to="base"><pose>0.01 0 0.02 0 0 0</pose></frame>
        <link name="tool"><pose relative_to="mount">0 0 0.005 0 0 0</pose></link>
        <joint name="root_joint" type="fixed"><parent>root</parent><child>base</child><pose>0 0 -0.04 0 0 0</pose></joint>
        <joint name="probe_joint" type="fixed"><parent>base</parent><child>probe</child></joint>
        <joint name="steer_joint" type="revolute"><parent>base</parent><child>knuckle</child>
          <pose relative_to="base">0.112 -0.10 0 0 0 0</pose>
          <axis><xyz>0 0 1</xyz><limit><lower>-0.6</lower><upper>0.6</upper></limit></axis></joint>
        <joint name="wheel_joint" type="revolute"><parent>knuckle</parent><child>wheel</child>
          <pose relative_to="steer_joint">0 0 0 0 0 0</pose><axis><xyz>0 1 0</xyz></axis></joint>
        <joint name="tool_joint" type="fixed"><parent>base</parent><child>tool</child></joint>
        </model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");

    // Joint <pose> is CHILD-relative: base sits 0.04 ABOVE root, so origin z = +0.04 (the element-local
    // reading would get the sign wrong: -0.04).
    assert_pose(
        "root_joint",
        joint_named(&doc, "root_joint").origin.as_ref(),
        [0.0, 0.0, 0.04],
        [0.0, 0.0, 0.0],
    );
    // Pose-less fixed joint: origin derived purely from the link poses; the authored 1.57 rpy survives
    // VERBATIM (the RotK::Rpy path; a matrix->rpy round-trip would emit 1.5699999999999876).
    let probe = joint_named(&doc, "probe_joint");
    assert_pose(
        "probe_joint",
        probe.origin.as_ref(),
        [-0.168, 0.05, 0.075],
        [0.0, 1.57, 0.0],
    );
    assert!(
        (probe.origin.as_ref().unwrap().rpy_or_zero()[1] - 1.57).abs() < 1e-12,
        "authored rpy passes verbatim"
    );
    // relative_to = the PARENT frame with the child at the joint identity: numerically identical to the
    // old element-local accident, so wheels stay put, and the axis text passes verbatim.
    let steer = joint_named(&doc, "steer_joint");
    assert_pose(
        "steer_joint",
        steer.origin.as_ref(),
        [0.112, -0.10, 0.0],
        [0.0, 0.0, 0.0],
    );
    assert_eq!(
        steer.axis.as_ref().and_then(|a| a.xyz.as_deref()),
        Some("0 0 1"),
        "axis text verbatim (the joint frame IS represented now, no re-expression needed)"
    );
    let wheel = joint_named(&doc, "wheel_joint");
    assert_pose(
        "wheel_joint (identity origin stays Some: it HAD a <pose> element)",
        wheel.origin.as_ref(),
        [0.0; 3],
        [0.0; 3],
    );
    assert_eq!(
        wheel.axis.as_ref().and_then(|a| a.xyz.as_deref()),
        Some("0 1 0")
    );
    // relative_to names a <frame>: X(base<-tool) = mount pose ∘ tool pose.
    assert_pose(
        "tool_joint",
        joint_named(&doc, "tool_joint").origin.as_ref(),
        [0.01, 0.0, 0.025],
        [0.0, 0.0, 0.0],
    );
    // The <frame> itself maps onto its attached-to comp, posed comp-local.
    let base = comp_named(&doc, "base");
    assert_eq!(base.frame.len(), 1, "frame 'mount' mapped onto comp 'base'");
    assert_eq!(base.frame[0].name, "mount");
    assert_pose(
        "frame 'mount'",
        base.frame[0].pose.as_ref(),
        [0.01, 0.0, 0.02],
        [0.0, 0.0, 0.0],
    );
    // The flattening-era notes are gone: link poses ARE represented (through joint origins), <frame>s map.
    assert!(
        !notes.iter().any(|n| n.contains("flattening")),
        "no frame-graph-flattening note: {notes:?}"
    );
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("not represented (SDF-only")),
        "<frame> is mapped, not dropped: {notes:?}"
    );
}

#[test]
fn sdf_axis_expressed_in_reexpressed_into_child_frame() {
    // j1's joint frame is yawed pi/2 against the model, so <xyz expressed_in="__model__">1 0 0</xyz>
    // re-expresses into the child frame as (0, -1, 0) (Rz(pi/2)^T·x) + a note. j2's frames align:
    // the text passes verbatim with NO note.
    let sdf = r#"<sdf version="1.10"><model name="ax">
        <link name="a"/>
        <link name="b"><pose relative_to="j1">0 0 0 0 0 0</pose></link>
        <link name="c"><pose relative_to="j2">0 0 0 0 0 0</pose></link>
        <joint name="j1" type="revolute"><parent>a</parent><child>b</child>
          <pose relative_to="a">0 0 0 0 0 1.5707963267948966</pose>
          <axis><xyz expressed_in="__model__">1 0 0</xyz></axis></joint>
        <joint name="j2" type="revolute"><parent>a</parent><child>c</child>
          <pose relative_to="a">0.1 0 0 0 0 0</pose>
          <axis><xyz expressed_in="__model__">1 0 0</xyz></axis></joint>
        </model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let v = parsed_axis(joint_named(&doc, "j1"));
    let expect = [0.0, -1.0, 0.0];
    for i in 0..3 {
        assert!(
            (v[i] - expect[i]).abs() < 1e-9,
            "axis[{i}] = {} != {}",
            v[i],
            expect[i]
        );
    }
    assert_eq!(
        notes.iter().filter(|n| n.contains("re-expressed")).count(),
        1,
        "exactly j1's axis is re-expressed: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("<axis><xyz> re-expressed from '__model__' into the child frame")),
        "re-expression note names the source frame: {notes:?}"
    );
    assert_eq!(
        joint_named(&doc, "j2")
            .axis
            .as_ref()
            .and_then(|a| a.xyz.as_deref()),
        Some("1 0 0"),
        "aligned expressed_in frame -> verbatim text"
    );
}

#[test]
fn sdf_joint_anchor_offset_noted() {
    // A revolute joint frame OFFSET from the child link origin (pose -0.1 relative to the child): the
    // zero-configuration origin is still exact X(parent<-child) = (0.5, 0, 0), but the rotation anchor
    // is not representable in HCDF -> note. The axis still passes verbatim (frames align in rotation).
    let sdf = r#"<sdf version="1.10"><model name="anchor">
        <link name="p"/>
        <link name="c"><pose>0.5 0 0 0 0 0</pose></link>
        <joint name="jr" type="revolute"><parent>p</parent><child>c</child>
          <pose>-0.1 0 0 0 0 0</pose>
          <axis><xyz>0 0 1</xyz></axis></joint>
        </model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let jr = joint_named(&doc, "jr");
    assert_pose("jr", jr.origin.as_ref(), [0.5, 0.0, 0.0], [0.0, 0.0, 0.0]);
    assert_eq!(
        jr.axis.as_ref().and_then(|a| a.xyz.as_deref()),
        Some("0 0 1")
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("offset from the child link origin")),
        "anchor-offset note: {notes:?}"
    );
}

#[test]
fn sdf_frame_graph_cycle_and_dangling_fall_back_with_note() {
    // Cyclic relative_to (a <-> b) and a dangling reference ('nope'): invalid SDF, imported tolerantly;
    // every comp still emitted, the affected poses fall back to model-relative (the old flattening), and
    // both defects are noted. jx pins the model-relative fallback through its derived origin.
    let sdf = r#"<sdf version="1.10"><model name="cy">
        <link name="r"/>
        <link name="a"><pose relative_to="b">0 0 1 0 0 0</pose></link>
        <link name="b"><pose relative_to="a">0 1 0 0 0 0</pose></link>
        <link name="x"><pose relative_to="nope">1 2 3 0 0 0</pose></link>
        <joint name="jx" type="fixed"><parent>r</parent><child>x</child></joint>
        </model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(
        doc.comp.len(),
        4,
        "all comps emitted despite the invalid graph"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("cycle") && n.contains("'a'") && n.contains("'b'")),
        "cycle note names the members: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("references an unknown frame")),
        "dangling-reference note: {notes:?}"
    );
    assert_pose(
        "jx (x's pose taken model-relative)",
        joint_named(&doc, "jx").origin.as_ref(),
        [1.0, 2.0, 3.0],
        [0.0, 0.0, 0.0],
    );
}

#[test]
fn sdf_root_link_model_pose_still_dropped_with_note() {
    // A posed ROOT link is the one link pose no joint origin can carry (HCDF's root has no pose): the
    // kinematics stay consistent (the child's origin is relative to the root), the model-frame offset
    // itself is dropped + noted.
    let sdf = r#"<sdf version="1.10"><model name="rooty">
        <link name="root"><pose>1 2 3 0 0 0</pose></link>
        <link name="kid"/>
        <joint name="rk" type="fixed"><parent>root</parent><child>kid</child></joint>
        </model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_pose(
        "rk (kid relative to the posed root)",
        joint_named(&doc, "rk").origin.as_ref(),
        [-1.0, -2.0, -3.0],
        [0.0, 0.0, 0.0],
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("root-comp <pose>") && n.contains("not represented")),
        "root-pose drop note: {notes:?}"
    );
}

/// The real b3rb rover SDF (lives outside the repo; skip when absent, mirroring `corpus()`).
fn b3rb_sdf() -> Option<PathBuf> {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let p =
        home.join("cognipilot/cranium/src/b3rb_simulator/b3rb_gz_resource/models/b3rb/model.sdf");
    p.is_file().then_some(p)
}

#[test]
fn b3rb_real_model_frame_resolution() {
    // The motivating real model (SDF 1.10, 23 links / 22 joints): leds/camera/lidar/sensors place via
    // link poses + pose-less or child-relative joints (ALL bunched at their parents' origins under the
    // old flattening import), while the wheels place via relative_to-parent joint poses (accidentally
    // correct before) and MUST stay byte-stable.
    let Some(path) = b3rb_sdf() else {
        eprintln!("skipping: b3rb model.sdf not present");
        return;
    };
    let (doc, notes) = from_sdf_path(&path).expect("import b3rb");
    assert_eq!(doc.comp.len(), 23, "23 links -> 23 comps");
    assert_eq!(doc.joint.len(), 22, "22 joints");

    // Led joints carry <pose>0 0 0</pose> (child-relative identity): origin = X(base_link<-led_N),
    // with the authored ±1.57 pitch VERBATIM (1e-12) through the RotK::Rpy path.
    let led0 = joint_named(&doc, "LedJoint0");
    assert_pose(
        "LedJoint0",
        led0.origin.as_ref(),
        [-0.168, 0.05, 0.075],
        [0.0, 1.57, 0.0],
    );
    assert!((led0.origin.as_ref().unwrap().rpy_or_zero()[1] - 1.57).abs() < 1e-12);
    let led6 = joint_named(&doc, "LedJoint6");
    assert_pose(
        "LedJoint6",
        led6.origin.as_ref(),
        [0.17, -0.05, 0.075],
        [0.0, -1.57, 0.0],
    );
    assert!((led6.origin.as_ref().unwrap().rpy_or_zero()[1] + 1.57).abs() < 1e-12);
    // The bunching-regression guard: EVERY led joint places away from base_link's origin.
    for i in 0..12 {
        let j = joint_named(&doc, &format!("LedJoint{i}"));
        let xyz = j
            .origin
            .as_ref()
            .map(|p| p.xyz_or_zero())
            .unwrap_or_default();
        assert!(
            xyz.iter().any(|v| v.abs() > 1e-6),
            "LedJoint{i} must not bunch at the parent origin: {xyz:?}"
        );
    }
    // Pose-less fixed joints: placement lives entirely in the child link pose (base_link is 0.04 up).
    assert_pose(
        "camera_joint",
        joint_named(&doc, "camera_joint").origin.as_ref(),
        [0.173, 0.0, 0.11],
        [0.0; 3],
    );
    assert_pose(
        "lidar_joint",
        joint_named(&doc, "lidar_joint").origin.as_ref(),
        [0.0, 0.0, 0.155],
        [0.0; 3],
    );
    // sensors sits at the MODEL origin -> 0.04 BELOW base_link (was 0,0,0 under flattening).
    assert_pose(
        "SensorsJoint",
        joint_named(&doc, "SensorsJoint").origin.as_ref(),
        [0.0, 0.0, -0.04],
        [0.0; 3],
    );
    // base_joint's <pose>0 0 -0.04</pose> is CHILD-relative: origin = X(footprint<-base) = +0.04 (the
    // element-local reading got the SIGN wrong).
    assert_pose(
        "base_joint",
        joint_named(&doc, "base_joint").origin.as_ref(),
        [0.0, 0.0, 0.04],
        [0.0; 3],
    );
    // Wheel sentinels: the relative_to="base_link" steering poses and the joint-frame wheel mounts are
    // UNCHANGED versus the old import (they only worked by accident, but they worked).
    for (name, xyz) in [
        ("FrontRightwheelSteeringJoint", [0.112, -0.10, 0.0]),
        ("FrontLeftwheelSteeringJoint", [0.112, 0.10, 0.0]),
        ("RearRightwheelJoint", [-0.1135, -0.10, 0.0]),
        ("RearLeftwheelJoint", [-0.1135, 0.10, 0.0]),
        ("FrontRightwheelJoint", [0.0; 3]),
        ("FrontLeftwheelJoint", [0.0; 3]),
    ] {
        assert_pose(name, joint_named(&doc, name).origin.as_ref(), xyz, [0.0; 3]);
    }
    for name in [
        "FrontRightwheelSteeringJoint",
        "FrontLeftwheelSteeringJoint",
    ] {
        assert_eq!(
            joint_named(&doc, name)
                .axis
                .as_ref()
                .and_then(|a| a.xyz.as_deref()),
            Some("0 0 1"),
            "{name}: steering axis verbatim"
        );
    }
    for name in [
        "FrontRightwheelJoint",
        "FrontLeftwheelJoint",
        "RearRightwheelJoint",
        "RearLeftwheelJoint",
    ] {
        assert_eq!(
            joint_named(&doc, name)
                .axis
                .as_ref()
                .and_then(|a| a.xyz.as_deref()),
            Some("0 1 0"),
            "{name}: wheel axis verbatim"
        );
    }
    assert!(
        !notes.iter().any(|n| n.contains("flattening")),
        "every b3rb link pose is represented: {notes:?}"
    );
}

#[test]
fn world_comp_maps_to_reserved_world_frame() {
    // A comp named 'world' must NOT be emitted as a <link> (SDF reserves it); a joint anchored to it keeps
    // <parent>world</parent>, fixing the model to the world. Mirrors to_sdf.py.
    let hcdf = r#"<hcdf name="bot" body-frame="FLU" world-frame="ENU">
        <comp name="world"/><comp name="base"/>
        <joint name="anchor" type="fixed"><parent comp="world"/><child comp="base"/></joint></hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse hcdf");
    let (sdf, _) = to_sdf(&doc).expect("to_sdf");
    assert!(
        !sdf.contains(r#"name="world""#),
        "no <link name=\"world\"> emitted"
    );
    assert!(
        sdf.contains("<parent>world</parent>"),
        "anchor keeps <parent>world</parent>"
    );
    assert!(
        sdf.contains(r#"name="base""#),
        "the real link is still emitted"
    );
}

// ── export symmetry: the typed sensor re-root round-trips back through to_sdf ─────────────────────

#[test]
fn typed_sensors_survive_hcdf_to_sdf_reparse() {
    // A hand-authored HCDF carrying a fluid barometer + an optical lidar (with <range>) + an inertial
    // IMU is exported to SDF, then re-imported: every sensor must survive the HCDF -> SDF -> HCDF round
    // trip (the export half of the typed sensor re-root). This is a pure-HCDF authored doc, NOT one that
    // came from SDF, so it isolates the exporter.
    let hcdf = r#"<hcdf name="s" body-frame="FLU" world-frame="ENU">
        <comp name="base">
          <sensor name="imu0" update-rate="200">
            <inertial type="accel_gyro">
              <accel><noise type="gaussian"><stddev>0.017</stddev></noise></accel>
              <gyro><noise type="gaussian"><stddev>0.0009</stddev></noise></gyro>
            </inertial>
          </sensor>
          <sensor name="baro0" update-rate="50">
            <fluid type="barometer"><noise type="gaussian"><stddev>0.5</stddev></noise></fluid>
          </sensor>
          <sensor name="lidar0" update-rate="10">
            <optical type="lidar">
              <lidar-params>
                <range><min>0.08</min><max>30</max><resolution>0.01</resolution></range>
                <scan-pattern>
                  <horizontal samples="360" resolution="1" min-angle="-3.14159" max-angle="3.14159"/>
                </scan-pattern>
              </lidar-params>
            </optical>
          </sensor>
        </comp>
      </hcdf>"#;
    let doc = Hcdf::from_xml_str(hcdf).expect("parse authored hcdf");
    // pre-flight: the authored HCDF actually carries the three typed sensors (guards the fixture).
    assert_eq!(doc.comp[0].sensor.len(), 3, "authored HCDF has 3 sensors");

    let (sdf, _loss) = to_sdf(&doc).expect("to_sdf emits the sensors");
    // the exporter produced the SDFormat sensor types (inverse map).
    assert!(sdf.contains(r#"type="imu""#), "imu sensor emitted: {sdf}");
    assert!(
        sdf.contains(r#"type="air_pressure""#),
        "barometer -> air_pressure: {sdf}"
    );
    assert!(
        sdf.contains(r#"type="gpu_lidar""#),
        "lidar -> gpu_lidar: {sdf}"
    );
    assert!(
        sdf.contains("<range>") && sdf.contains("<min>0.08</min>"),
        "lidar range emitted: {sdf}"
    );

    // reparse the exported SDF: all three sensors survive with their key typed fields.
    let (doc2, _notes) = from_sdf_str(&sdf).expect("reparse exported sdf");
    let base = doc2
        .comp
        .iter()
        .find(|c| c.name == "base")
        .expect("base comp survives");
    assert_eq!(
        base.sensor.len(),
        3,
        "all three sensors survive the round-trip"
    );

    use hcdformat::model::enums::{InertialSensorType, OpticalSensorType};
    let imu = base
        .sensor
        .iter()
        .find(|s| !s.inertial.is_empty())
        .expect("imu survives");
    assert_eq!(imu.name.as_deref(), Some("imu0"));
    assert_eq!(imu.update_rate.as_deref(), Some("200"));
    assert_eq!(imu.inertial[0].type_, Some(InertialSensorType::AccelGyro));
    assert_eq!(
        imu.inertial[0]
            .accel
            .as_ref()
            .and_then(|a| a.noise.as_ref())
            .and_then(|n| n.stddev.as_deref()),
        Some("0.017"),
        "accel noise survives"
    );
    assert_eq!(
        imu.inertial[0]
            .gyro
            .as_ref()
            .and_then(|g| g.noise.as_ref())
            .and_then(|n| n.stddev.as_deref()),
        Some("0.0009"),
        "gyro noise survives"
    );

    let baro = base
        .sensor
        .iter()
        .find(|s| !s.fluid.is_empty())
        .expect("barometer survives");
    assert_eq!(baro.fluid[0].type_.as_deref(), Some("barometer"));
    assert_eq!(
        baro.fluid[0]
            .noise
            .as_ref()
            .and_then(|n| n.stddev.as_deref()),
        Some("0.5"),
        "barometer noise survives"
    );

    let lidar = base
        .sensor
        .iter()
        .find(|s| !s.optical.is_empty())
        .expect("lidar survives");
    assert_eq!(lidar.optical[0].type_, Some(OpticalSensorType::Lidar));
    let lp = lidar.optical[0]
        .lidar_params
        .as_ref()
        .expect("lidar-params survive");
    let rng = lp.range.as_ref().expect("range survives");
    assert_eq!(
        rng.min.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.08")
    );
    assert_eq!(
        rng.max.as_ref().and_then(|v| v.value.as_deref()),
        Some("30")
    );
    assert_eq!(
        rng.resolution.as_ref().and_then(|v| v.value.as_deref()),
        Some("0.01")
    );
    let sp = lp
        .scan_pattern
        .as_ref()
        .and_then(|s| s.horizontal.as_ref())
        .expect("scan horizontal survives");
    assert_eq!(sp.samples.as_deref(), Some("360"));
    assert_eq!(sp.min_angle.as_deref(), Some("-3.14159"));
}

#[test]
fn b3rb_full_sensor_roundtrip_sdf_hcdf_sdf() {
    // FULL round-trip on the real robot: SDF -> HCDF -> SDF. b3rb's camera (ov5645) and lidar must be
    // present BOTH as typed HCDF sensors AND in the re-exported SDF (and survive a second import).
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let path =
        home.join("cognipilot/cranium/src/b3rb_simulator/b3rb_gz_resource/models/b3rb/model.sdf");
    if !path.is_file() {
        eprintln!("skipping b3rb sensor round-trip: {} absent", path.display());
        return;
    }
    use hcdformat::model::enums::OpticalSensorType;

    // (1) SDF -> HCDF: camera + lidar are typed.
    let (doc, _n1) = from_sdf_path(&path).expect("from_sdf_path b3rb");
    let cam_comp = doc
        .comp
        .iter()
        .find(|c| c.name == "camera_link")
        .expect("camera_link");
    assert!(cam_comp.sensor.iter().any(|s| s
        .optical
        .iter()
        .any(|o| o.type_ == Some(OpticalSensorType::Camera))));
    let lidar_comp = doc
        .comp
        .iter()
        .find(|c| c.name == "lidar_link")
        .expect("lidar_link");
    assert!(lidar_comp.sensor.iter().any(|s| s
        .optical
        .iter()
        .any(|o| o.type_ == Some(OpticalSensorType::Lidar))));

    // (2) HCDF -> SDF: the exported SDF carries the camera + lidar sensor grammar back.
    let (sdf, _loss) = to_sdf(&doc).expect("to_sdf b3rb");
    assert!(sdf.contains(r#"type="camera""#), "camera re-emitted to SDF");
    assert!(
        sdf.contains(r#"type="gpu_lidar""#),
        "lidar re-emitted to SDF"
    );
    assert!(
        sdf.contains("<width>320</width>"),
        "camera image width round-trips into SDF"
    );
    assert!(
        sdf.contains("<min>0.03</min>") && sdf.contains("<max>20</max>"),
        "lidar range round-trips into SDF"
    );

    // (3) SDF -> HCDF again: camera + lidar survive the second import (present BOTH ways).
    let (doc2, _n2) = from_sdf_str(&sdf).expect("reparse exported b3rb sdf");
    let cam2 = doc2
        .comp
        .iter()
        .find(|c| c.name == "camera_link")
        .expect("camera_link (reparse)");
    let cam = cam2
        .sensor
        .iter()
        .find(|s| !s.optical.is_empty())
        .expect("camera survives round-trip");
    assert_eq!(cam.optical[0].type_, Some(OpticalSensorType::Camera));
    assert_eq!(
        cam.optical[0]
            .fov
            .first()
            .and_then(|f| f.intrinsics.as_ref())
            .and_then(|i| i.width.as_deref()),
        Some("320"),
        "camera intrinsics width survives the full round-trip"
    );
    let lidar2 = doc2
        .comp
        .iter()
        .find(|c| c.name == "lidar_link")
        .expect("lidar_link (reparse)");
    let lidar = lidar2
        .sensor
        .iter()
        .find(|s| !s.optical.is_empty())
        .expect("lidar survives round-trip");
    assert_eq!(lidar.optical[0].type_, Some(OpticalSensorType::Lidar));
    assert_eq!(
        lidar.optical[0]
            .lidar_params
            .as_ref()
            .and_then(|lp| lp.range.as_ref())
            .and_then(|r| r.min.as_ref())
            .and_then(|m| m.value.as_deref()),
        Some("0.03"),
        "lidar range min survives the full round-trip"
    );
}

/// An SDF 1.7+ `<link><battery name><voltage>` used to be silently dropped
/// (`from_sdf` had no battery handling). Its open-circuit init `<voltage>` (the one battery leaf with a
/// clean SDF source) now maps to a typed `<power-source><battery><nominal-voltage>` (unit "V"); the rest
/// of the pack model lives in the gz `LinearBatteryPlugin` `<plugin>` and stays native-authored.
#[test]
fn sdf_link_battery_voltage_maps_to_battery_source_nominal_voltage() {
    let sdf = r#"<sdf version="1.7"><model name="rover"><link name="base">
        <battery name="main_battery"><voltage>12.592</voltage></battery>
    </link></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let base = doc
        .comp
        .iter()
        .find(|c| c.name == "base")
        .expect("base comp");
    assert_eq!(
        base.power_source.len(),
        1,
        "the SDF <battery> maps to one <power-source>"
    );
    let ps = &base.power_source[0];
    assert_eq!(
        ps.name.as_deref(),
        Some("main_battery"),
        "battery @name -> power-source @name"
    );
    let nv = ps
        .battery
        .as_ref()
        .and_then(|b| b.nominal_voltage.as_ref())
        .expect("battery_source nominal-voltage");
    assert_eq!(
        nv.value.as_deref(),
        Some("12.592"),
        "<voltage> -> nominal-voltage value"
    );
    assert_eq!(
        nv.unit.as_deref(),
        Some("V"),
        "nominal-voltage stamped SDFormat unit V"
    );
    assert!(
        notes.iter().any(|n| n.contains("nominal-voltage")),
        "a battery mapping note is recorded: {notes:?}"
    );
    // The imported HCDF is schema-valid and the battery survives an HCDF round-trip.
    let xml = doc.to_xml_string().expect("serialize");
    let doc2 = Hcdf::from_xml_str(&xml).expect("reparse");
    assert_eq!(
        doc2.comp
            .iter()
            .find(|c| c.name == "base")
            .and_then(|c| c.power_source.first())
            .and_then(|ps| ps.battery.as_ref())
            .and_then(|b| b.nominal_voltage.as_ref())
            .and_then(|v| v.value.as_deref()),
        Some("12.592"),
        "nominal-voltage survives the HCDF round-trip"
    );
    #[cfg(feature = "xsd")]
    assert!(
        hcdformat::validate_xsd(&xml).is_empty(),
        "battery HCDF must satisfy hcdf.xsd: {:?}",
        hcdformat::validate_xsd(&xml)
    );
}

// ── joint-type coverage: SDF import + HCDF->SDF export symmetry ───────────────────────────────────

fn sdf_joint(jtype: &str, extra: &str) -> String {
    format!(
        r#"<sdf version="1.12"><model name="m">
        <link name="a"/><link name="b"/>
        <joint name="j" type="{jtype}"><parent>a</parent><child>b</child>{extra}</joint>
        </model></sdf>"#
    )
}

/// SDF `revolute2` (2-DOF, two independent rotation axes) has no 1:1 HCDF literal; it imports as the
/// closest kinematic match `universal`, with a note documenting the intersecting-axes approximation.
#[test]
fn sdf_revolute2_imports_as_universal_with_note() {
    let sdf = sdf_joint(
        "revolute2",
        "<axis><xyz>0 0 1</xyz></axis><axis2><xyz>0 1 0</xyz></axis2>",
    );
    let (doc, notes) = from_sdf_str(&sdf).expect("revolute2 must import (mapped to universal)");
    assert_eq!(
        doc.joint[0]
            .type_
            .as_ref()
            .map(|t| t.to_string())
            .as_deref(),
        Some("universal"),
        "SDF revolute2 must import as HCDF universal"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("revolute2") && n.contains("universal")),
        "revolute2->universal approximation must be noted; got {notes:?}"
    );
    // The two axes carry over (universal is 2-DOF, requires axis + axis2).
    assert!(
        doc.joint[0].axis.is_some(),
        "revolute2 axis must carry to universal axis"
    );
    assert!(
        doc.joint[0].axis2.is_some(),
        "revolute2 axis2 must carry to universal axis2"
    );
}

/// SDF `gearbox` is a gear-COUPLING constraint (not a kinematic DOF). It maps to a typed
/// `<transmission type="gear">`: the driven child geared against the reference body by `gearbox_ratio`,
/// with two role-tagged `<joint>` endpoints (reference/driven), NOT a fixed joint. Here child `b` is
/// already parented by a real revolute joint, so NO fixed edge fallback is emitted (only the coupling).
/// The transmission round-trips SDF->HCDF->SDF back to a `<joint type="gearbox">` with the ratio.
#[test]
fn sdf_gearbox_imports_as_gear_transmission_and_round_trips() {
    use hcdformat::model::enums::TransmissionType;
    // `drive` (a) --revolute--> `out` (b); a gearbox gears `out` against reference body `ref_body` by 4.
    let sdf = r#"<sdf version="1.12"><model name="m">
        <link name="drive"/><link name="out"/><link name="ref_body"/>
        <joint name="spin" type="revolute"><parent>drive</parent><child>out</child>
          <axis><xyz>0 0 1</xyz></axis></joint>
        <joint name="gbx" type="gearbox"><parent>drive</parent><child>out</child>
          <gearbox_ratio>4</gearbox_ratio><gearbox_reference_body>ref_body</gearbox_reference_body></joint>
      </model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("gearbox must import as a gear transmission");

    // No fixed edge fallback: `out` already has the revolute `spin` as its kinematic parent.
    assert_eq!(
        doc.joint.len(),
        1,
        "only the revolute joint is kinematic; the gearbox is a transmission"
    );
    assert_eq!(doc.joint[0].name.as_deref(), Some("spin"));

    // The gearbox coupling is a typed gear transmission with the ratio + 2 role-tagged joint endpoints.
    assert_eq!(doc.transmission.len(), 1, "gearbox -> one <transmission>");
    let tr = &doc.transmission[0];
    assert_eq!(
        tr.name.as_deref(),
        Some("gbx"),
        "transmission takes the gearbox joint's name"
    );
    assert_eq!(tr.type_, Some(TransmissionType::Gear));
    assert_eq!(
        tr.reduction.as_deref(),
        Some("4"),
        "gearbox_ratio -> reduction"
    );
    assert!(
        tr.motor.is_empty(),
        "a gearbox couples joints, no motor endpoint"
    );
    assert_eq!(tr.joint.len(), 2, "driven + reference endpoints");
    let driven = tr
        .joint
        .iter()
        .find(|e| e.role.as_deref() == Some("driven"))
        .expect("driven endpoint");
    let reference = tr
        .joint
        .iter()
        .find(|e| e.role.as_deref() == Some("reference"))
        .expect("reference endpoint");
    assert_eq!(
        driven.ref_.as_deref(),
        Some("out"),
        "driven endpoint = the gearbox joint's child"
    );
    assert_eq!(
        reference.ref_.as_deref(),
        Some("ref_body"),
        "reference endpoint = gearbox_reference_body"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("gearbox") && n.contains("gear")),
        "the gearbox -> gear-transmission mapping must be noted; got {notes:?}"
    );

    // Round-trip: HCDF -> SDF re-emits a <joint type="gearbox"> with the ratio + reference body.
    let (sdf2, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        sdf2.contains(r#"type="gearbox""#),
        "gear transmission must export back to an SDF gearbox joint; got:\n{sdf2}"
    );
    assert!(
        sdf2.contains("<gearbox_ratio>4</gearbox_ratio>"),
        "the ratio round-trips: {sdf2}"
    );
    assert!(
        sdf2.contains("<gearbox_reference_body>ref_body</gearbox_reference_body>"),
        "the reference body round-trips: {sdf2}"
    );

    // And SDF -> HCDF -> SDF -> HCDF is stable: the coupling is still a gear transmission with the ratio.
    let (doc2, _) = from_sdf_str(&sdf2).expect("re-import emitted SDF");
    let tr2 = doc2
        .transmission
        .first()
        .expect("transmission survives the round-trip");
    assert_eq!(tr2.type_, Some(TransmissionType::Gear));
    assert_eq!(tr2.reduction.as_deref(), Some("4"));
}

/// The dangling-child guard: when the gearbox joint is the ONLY thing parenting its child link, mapping
/// it to a transmission alone would orphan that link, so a FIXED joint is emitted for the kinematic edge
/// too, ALONGSIDE the gear transmission, with a note.
#[test]
fn sdf_gearbox_dangling_child_emits_fixed_edge_plus_transmission() {
    use hcdformat::model::enums::{JointType, TransmissionType};
    // Only a gearbox parents `b`; no other joint gives it a kinematic parent.
    let sdf = sdf_joint(
        "gearbox",
        "<gearbox_ratio>2</gearbox_ratio><gearbox_reference_body>a</gearbox_reference_body>",
    );
    let (doc, notes) = from_sdf_str(&sdf).expect("gearbox must import");

    // Fixed edge fallback for the otherwise-orphaned child link `b`.
    assert_eq!(
        doc.joint.len(),
        1,
        "a fixed edge holds the otherwise-orphaned child"
    );
    assert_eq!(
        doc.joint[0].type_,
        Some(JointType::Fixed),
        "the fallback edge is fixed"
    );
    assert_eq!(
        doc.joint[0].name.as_deref(),
        Some("j_fixed"),
        "the fixed edge is renamed so it does not collide with the same-named gear <transmission> on export"
    );

    // PLUS the gear-coupling transmission.
    assert_eq!(
        doc.transmission.len(),
        1,
        "the gear coupling is still a transmission"
    );
    let tr = &doc.transmission[0];
    assert_eq!(
        tr.name.as_deref(),
        Some("j"),
        "transmission keeps the gearbox joint's name"
    );
    assert_eq!(tr.type_, Some(TransmissionType::Gear));
    assert_eq!(tr.reduction.as_deref(), Some("2"));

    assert!(
        notes
            .iter()
            .any(|n| n.contains("no other parent joint") && n.contains("fixed")),
        "the dangling-child fixed-edge fallback must be noted; got {notes:?}"
    );
}

/// Every 1:1 SDF joint type still maps unchanged.
#[test]
fn sdf_all_1to1_joint_types_still_map() {
    for jtype in [
        "revolute",
        "prismatic",
        "fixed",
        "continuous",
        "ball",
        "universal",
        "screw",
    ] {
        let sdf = sdf_joint(jtype, "");
        let (doc, _) = from_sdf_str(&sdf).unwrap_or_else(|e| panic!("{jtype}: import failed: {e}"));
        assert_eq!(
            doc.joint[0]
                .type_
                .as_ref()
                .map(|t| t.to_string())
                .as_deref(),
            Some(jtype),
            "SDF {jtype} must map 1:1 to HCDF {jtype}"
        );
    }
}

/// Each of the 10 HCDF joint types exports to SDF as its faithful literal, or downgrades to `fixed`
/// with an honest joint-type loss note, never a panic and never an invalid/silent bad type.
#[test]
fn hcdf_all_joint_types_export_to_sdf() {
    // (HCDF type, expected SDF @type, downgraded-with-loss?)
    let cases = [
        ("revolute", "revolute", false),
        // SDFormat has NO `continuous` type; a continuous joint is an unlimited `revolute`.
        ("continuous", "revolute", false),
        ("prismatic", "prismatic", false),
        ("fixed", "fixed", false),
        ("ball", "ball", false),
        ("universal", "universal", false),
        ("planar", "fixed", true),
        ("screw", "screw", false),
        ("cylindrical", "fixed", true),
        ("free", "fixed", true),
    ];
    for (hcdf_ty, sdf_ty, downgraded) in cases {
        let hcdf = format!(
            r#"<hcdf name="jt" version="1.0"><comp name="a"/><comp name="b"/>
            <joint name="j" type="{hcdf_ty}" thread_pitch="0.01">
              <parent comp="a"/><child comp="b"/>
              <axis xyz="0 0 1"/>
              <limit lower="-1" upper="1" effort="1" velocity="1"/></joint></hcdf>"#
        );
        let doc =
            Hcdf::from_xml_str(&hcdf).unwrap_or_else(|e| panic!("{hcdf_ty}: parse failed: {e}"));
        let (sdf, loss) = to_sdf(&doc).unwrap_or_else(|e| panic!("{hcdf_ty}: to_sdf failed: {e}"));
        assert!(
            sdf.contains(&format!(r#"<joint name="j" type="{sdf_ty}""#)),
            "HCDF {hcdf_ty} must export SDF type {sdf_ty}; got:\n{sdf}"
        );
        let has_type_loss = loss
            .text()
            .lines()
            .any(|l| l.contains("joint-type") && l.contains("no SDF equivalent"));
        assert_eq!(
            has_type_loss, downgraded,
            "HCDF {hcdf_ty}: joint-type loss note presence must match the downgrade"
        );
    }
}

/// An SDF screw with an axis `<limit>` and a LEGACY gazebo-classic `<thread_pitch>` (rad/m,
/// left-handed). The limit imports as the ROTATIONAL bound (radians, verbatim) and the pitch is
/// CONVERTED to canonical m/rev right-handed (screw_thread_pitch = -2*pi / thread_pitch), both noted.
/// On export the canonical pitch emits as the MODERN `<screw_thread_pitch>` (never legacy
/// `<thread_pitch>`), and the rotational bound round-trips through the axis `<limit>`.
#[test]
fn sdf_screw_classic_pitch_converts_and_round_trips() {
    // 314.159265 rad/m -> canonical = -2*pi/314.159265 ~= -0.02 m/rev.
    let sdf = sdf_joint(
        "screw",
        "<axis><xyz>0 0 1</xyz><limit><lower>-6.28</lower><upper>6.28</upper></limit></axis>\
         <thread_pitch>314.159265</thread_pitch>",
    );
    let (doc, notes) = from_sdf_str(&sdf).expect("screw imports");
    let j = joint_named(&doc, "j");
    assert_eq!(j.type_.map(|t| t.to_string()).as_deref(), Some("screw"));
    // Axis <limit> imported verbatim as the rotational bound (radians).
    let lim = j.limit.as_ref().expect("screw axis limit imported");
    assert_eq!(lim.lower.as_deref(), Some("-6.28"));
    assert_eq!(lim.upper.as_deref(), Some("6.28"));
    // Legacy pitch converted to canonical (~-0.02 m/rev), attrs left omitted (stored value IS canonical).
    let tp: f64 = j
        .thread_pitch
        .as_deref()
        .expect("thread_pitch set")
        .parse()
        .expect("numeric");
    assert!(
        (tp + 0.02).abs() < 1e-4,
        "canonical pitch ~= -0.02 m/rev, got {tp}"
    );
    assert!(j.pitch_convention.is_none() && j.handedness.is_none());
    assert!(
        notes
            .iter()
            .any(|n| n.contains("converted legacy gazebo-classic thread_pitch")),
        "pitch-conversion note expected; got {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("screw axis <limit> imported as the ROTATIONAL bound")),
        "rotational-limit note expected; got {notes:?}"
    );
    // Export: modern <screw_thread_pitch>, never legacy <thread_pitch>; rotational bound survives.
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains("<screw_thread_pitch>"),
        "modern pitch tag expected:\n{out}"
    );
    assert!(
        !out.contains("<thread_pitch>"),
        "legacy pitch tag must NOT be emitted:\n{out}"
    );
    assert!(
        out.contains("<lower>-6.28</lower>") && out.contains("<upper>6.28</upper>"),
        "screw rotational bound must round-trip on the axis limit:\n{out}"
    );
}

// ── mesh <submesh> selection round-trips ─────────────────────────────────────────────────────────

/// A collision `<mesh>` that selects a single named `<submesh>` (name + center) round-trips
/// SDF -> HCDF -> SDF: the sub-part selection is captured on import (previously dropped) and re-emitted.
#[test]
fn sdf_collision_mesh_submesh_round_trips() {
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
      <collision name="c"><geometry><mesh>
        <uri>parts.glb</uri>
        <submesh><name>gripper_pad</name><center>true</center></submesh>
      </mesh></geometry></collision>
    </link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let sm = doc.comp[0].collision[0]
        .geometry
        .as_ref()
        .and_then(|g| g.mesh.as_ref())
        .and_then(|m| m.submesh.as_ref())
        .expect("the <submesh> selection must be captured on import");
    assert_eq!(sm.name.as_deref(), Some("gripper_pad"));
    assert_eq!(sm.center.as_deref(), Some("true"));

    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains("<submesh>"),
        "export must re-emit <submesh>; got:\n{out}"
    );
    assert!(
        out.contains("<name>gripper_pad</name>"),
        "submesh name lost on export:\n{out}"
    );
    assert!(
        out.contains("<center>true</center>"),
        "submesh center lost on export:\n{out}"
    );
}

/// A mesh with NO `<submesh>` stays whole-file (no fabricated `<submesh>` on either side).
#[test]
fn sdf_collision_mesh_without_submesh_stays_whole() {
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="l">
      <collision name="c"><geometry><mesh><uri>whole.glb</uri></mesh></geometry></collision>
    </link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    assert!(
        doc.comp[0].collision[0]
            .geometry
            .as_ref()
            .and_then(|g| g.mesh.as_ref())
            .and_then(|m| m.submesh.as_ref())
            .is_none(),
        "a mesh with no <submesh> must not fabricate one"
    );
    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        !out.contains("<submesh>"),
        "no <submesh> must be emitted:\n{out}"
    );
}

// ── axis2 second-DOF <limit>/<dynamics> round-trip ───────────────────────────────────────────────

/// A universal joint whose SECOND axis carries its own <limit> and <dynamics> round-trips
/// SDF -> HCDF -> SDF: axis2's bounds/damping are captured in the typed limit2/dynamics2 slots
/// (previously dropped with a "single slot" note) and re-emitted under <axis2>.
#[test]
fn sdf_universal_axis2_limit_dynamics_round_trips() {
    let sdf = r#"<sdf version="1.12"><model name="m">
      <link name="base"/><link name="tool"/>
      <joint name="wrist" type="universal">
        <parent>base</parent><child>tool</child>
        <axis><xyz>1 0 0</xyz>
          <limit><lower>-1</lower><upper>1</upper><effort>10</effort><velocity>2</velocity></limit>
          <dynamics><damping>0.1</damping><friction>0.2</friction></dynamics>
        </axis>
        <axis2><xyz>0 1 0</xyz>
          <limit><lower>-2</lower><upper>2</upper><effort>20</effort><velocity>4</velocity></limit>
          <dynamics><damping>0.5</damping><friction>0.6</friction></dynamics>
        </axis2>
      </joint></model></sdf>"#;
    let (doc, notes) = from_sdf_str(sdf).expect("from_sdf_str");
    assert!(
        !notes
            .iter()
            .any(|n| n.contains("single limit/dynamics slot")),
        "axis2 limit/dynamics must no longer be dropped as unrepresentable; got {notes:?}"
    );
    let j = &doc.joint[0];
    let l2 = j
        .limit2
        .as_ref()
        .expect("axis2 <limit> must land in limit2");
    assert_eq!(l2.lower.as_deref(), Some("-2"));
    assert_eq!(l2.upper.as_deref(), Some("2"));
    assert_eq!(l2.effort.as_deref(), Some("20"));
    assert_eq!(l2.velocity.as_deref(), Some("4"));
    let d2 = j
        .dynamics2
        .as_ref()
        .expect("axis2 <dynamics> must land in dynamics2");
    assert_eq!(d2.damping.as_deref(), Some("0.5"));
    assert_eq!(d2.friction.as_deref(), Some("0.6"));
    // the primary axis is unchanged.
    assert_eq!(j.limit.as_ref().and_then(|l| l.upper.as_deref()), Some("1"));

    let (out, _loss) = to_sdf(&doc).expect("to_sdf");
    // axis2 re-emits its own limit/dynamics (assert the second-DOF values survive the export).
    assert!(out.contains("<axis2>"), "export must emit <axis2>:\n{out}");
    for needle in [
        "<lower>-2</lower>",
        "<effort>20</effort>",
        "<damping>0.5</damping>",
    ] {
        assert!(out.contains(needle), "axis2 export lost {needle}:\n{out}");
    }
    // and it still re-imports to the same second-DOF limit2/dynamics2.
    let (doc2, _n2) = from_sdf_str(&out).expect("re-import");
    assert_eq!(
        doc2.joint[0]
            .limit2
            .as_ref()
            .and_then(|l| l.effort.as_deref()),
        Some("20")
    );
    assert_eq!(
        doc2.joint[0]
            .dynamics2
            .as_ref()
            .and_then(|d| d.damping.as_deref()),
        Some("0.5")
    );
}

// ── <include> composition reference round-trips both directions ───────────────────────────────────

/// A model-scope SDF <include> (uri/name/pose/static/placement_frame) round-trips SDF -> HCDF -> SDF:
/// the sub-assembly reference is captured (previously vanished, no find_all("include") anywhere) and
/// re-emitted, and is no longer reported as a dropped top-level element.
#[test]
fn sdf_model_include_round_trips() {
    let sdf = r#"<sdf version="1.12"><model name="m">
      <link name="base"/>
      <include>
        <uri>model://arm</uri>
        <name>left_arm</name>
        <pose>1 0 0 0 0 0</pose>
        <static>true</static>
        <placement_frame>mount</placement_frame>
      </include>
    </model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    assert_eq!(
        doc.include.len(),
        1,
        "the <include> reference must be captured"
    );
    let inc = &doc.include[0];
    assert_eq!(inc.uri.as_deref(), Some("model://arm"));
    assert_eq!(inc.name.as_deref(), Some("left_arm"));
    assert_eq!(inc.pose.as_deref(), Some("1 0 0 0 0 0"));
    assert_eq!(inc.static_.as_deref(), Some("true"));
    assert_eq!(inc.placement_frame.as_deref(), Some("mount"));

    let (out, loss) = to_sdf(&doc).expect("to_sdf");
    for needle in [
        "<include>",
        "<uri>model://arm</uri>",
        "<name>left_arm</name>",
        "<static>true</static>",
        "<placement_frame>mount</placement_frame>",
    ] {
        assert!(out.contains(needle), "include export lost {needle}:\n{out}");
    }
    assert!(
        !loss.text().lines().any(|l| l.contains("includes dropped")),
        "includes must no longer be a top-level drop:\n{}",
        loss.text()
    );
    // re-import is stable.
    let (doc2, _n2) = from_sdf_str(&out).expect("re-import");
    assert_eq!(doc2.include.len(), 1);
    assert_eq!(doc2.include[0].static_.as_deref(), Some("true"));
    assert_eq!(doc2.include[0].placement_frame.as_deref(), Some("mount"));
}

// ── link-scope <light> <-> led-illumination HMI round-trip ────────────────────────────────────────

/// A link-scope SDF <light> (a headlight/work-light) round-trips SDF -> HCDF -> SDF: it maps to a typed
/// led-illumination HMI whose <illumination> carries type/colour/attenuation/direction/intensity/
/// cast_shadows (previously every <light> was dropped), and re-exports to a <light> under the link.
#[test]
fn sdf_link_light_round_trips_as_led_illumination_hmi() {
    let sdf = r#"<sdf version="1.12"><model name="m"><link name="head">
      <light name="headlight" type="spot">
        <pose>0.1 0 0.2 0 0 0</pose>
        <cast_shadows>true</cast_shadows>
        <intensity>1.5</intensity>
        <diffuse>1 1 0.9 1</diffuse>
        <specular>0.2 0.2 0.2 1</specular>
        <attenuation><range>20</range><linear>0.1</linear><constant>0.2</constant><quadratic>0.01</quadratic></attenuation>
        <direction>0 0 -1</direction>
      </light>
    </link></model></sdf>"#;
    let (doc, _notes) = from_sdf_str(sdf).expect("from_sdf_str");
    let hmi = &doc.comp[0].hmi[0];
    assert_eq!(hmi.name.as_deref(), Some("headlight"));
    assert!(
        format!("{:?}", hmi.type_).contains("LedIllumination"),
        "a <light> must map to a led-illumination HMI; got {:?}",
        hmi.type_
    );
    let il = hmi
        .illumination
        .as_ref()
        .expect("<illumination> must be populated");
    assert_eq!(il.light_type.as_deref(), Some("spot"));
    assert_eq!(il.cast_shadows.as_deref(), Some("true"));
    assert_eq!(il.intensity.as_deref(), Some("1.5"));
    assert_eq!(il.diffuse.as_deref(), Some("1 1 0.9 1"));
    assert_eq!(il.specular.as_deref(), Some("0.2 0.2 0.2 1"));
    assert_eq!(il.direction.as_deref(), Some("0 0 -1"));
    let at = il.attenuation.as_ref().expect("attenuation");
    assert_eq!(at.range.as_deref(), Some("20"));
    assert_eq!(at.linear.as_deref(), Some("0.1"));
    assert_eq!(at.quadratic.as_deref(), Some("0.01"));

    let (out, loss) = to_sdf(&doc).expect("to_sdf");
    assert!(
        out.contains(r#"<light name="headlight" type="spot">"#),
        "light not re-emitted:\n{out}"
    );
    for needle in [
        "<cast_shadows>true</cast_shadows>",
        "<intensity>1.5</intensity>",
        "<diffuse>1 1 0.9 1</diffuse>",
        "<range>20</range>",
        "<direction>0 0 -1</direction>",
    ] {
        assert!(out.contains(needle), "light export lost {needle}:\n{out}");
    }
    assert!(
        !loss
            .text()
            .lines()
            .any(|l| l.contains("HMI elements dropped")),
        "a led-illumination HMI must round-trip, not be dropped:\n{}",
        loss.text()
    );
    // re-import is stable.
    let (doc2, _n2) = from_sdf_str(&out).expect("re-import");
    let il2 = doc2.comp[0].hmi[0]
        .illumination
        .as_ref()
        .expect("re-imported <illumination>");
    assert_eq!(il2.intensity.as_deref(), Some("1.5"));
    assert_eq!(
        il2.attenuation.as_ref().and_then(|a| a.range.as_deref()),
        Some("20")
    );
}

#[test]
fn tree_is_counted_by_sdf_loss_manifest() {
    let doc = Hcdf::from_xml_str(
        r#"<hcdf name="d" version="1.0"><tree name="n"><root><hop-ref network="n" hop="h"/></root><hop name="h"><owner><component-ref component="ghost"/></owner></hop></tree></hcdf>"#,
    )
    .unwrap();

    let (_, loss) = to_sdf(&doc).unwrap();
    assert!(loss.items.iter().any(|(category, detail)| {
        category == "top-level" && detail == "document: 1 networks dropped (no SDF-model home)"
    }));
}
