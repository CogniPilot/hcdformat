//! glTF → GLB packer tests (`src/gltf_pack.rs`) and the vendor/bake wiring that drives it
//! (`src/assets_vendor.rs`).
//!
//! The packer's contract, each proven here:
//!   * a `.gltf` whose geometry lives in an EXTERNAL `.bin` packs into ONE spec-correct GLB: magic,
//!     version-2 header, 4-byte-aligned space-padded JSON chunk, zero-padded BIN chunk holding the
//!     buffer bytes, `buffers` rewritten to the single uri-less GLB buffer, `bufferViews` re-pointed;
//!   * packing is DETERMINISTIC (same input ⇒ bit-identical GLB ⇒ same content `@sha`);
//!   * `data:` URIs decode without touching the resolver; multiple buffers merge 4-byte aligned with
//!     every `bufferView.byteOffset` shifted correctly;
//!   * external `.png`/`.jpg` images inline as new `bufferView`s (+ `mimeType`); anything that cannot
//!     be made self-contained errors LOUDLY, listing every offender, never a broken GLB;
//!   * the vendor/bake passthrough decides by CONTENT (GLB magic), not extension: real GLB bytes vendor
//!     byte-identically (the Python byte-parity path), a `.gltf` vendors as the packed `<stem>_<sha>.glb`.

use hcdformat::{is_glb_bytes, pack_gltf_to_glb, GltfPackError};
use serde_json::Value;

/// A minimal one-triangle `.gltf` whose geometry lives in an external `tri.bin`: u16 indices `[0,1,2]`
/// (6 bytes, then 2 alignment bytes) followed by 3 × f32×3 positions (36 bytes), 44 bytes total.
fn triangle_gltf() -> (&'static str, Vec<u8>) {
    let gltf = r#"{
      "asset": {"version": "2.0"},
      "scene": 0,
      "scenes": [{"nodes": [0]}],
      "nodes": [{"mesh": 0}],
      "meshes": [{"primitives": [{"attributes": {"POSITION": 1}, "indices": 0, "mode": 4}]}],
      "accessors": [
        {"bufferView": 0, "componentType": 5123, "count": 3, "type": "SCALAR", "min": [0], "max": [2]},
        {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3",
         "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]}
      ],
      "bufferViews": [
        {"buffer": 0, "byteOffset": 0, "byteLength": 6, "target": 34963},
        {"buffer": 0, "byteOffset": 8, "byteLength": 36, "target": 34962}
      ],
      "buffers": [{"uri": "tri.bin", "byteLength": 44}]
    }"#;
    let mut bin = Vec::new();
    for i in [0u16, 1, 2] {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    bin.extend_from_slice(&[0, 0]); // align positions to 4
    for v in [[0f32, 0., 0.], [1., 0., 0.], [1., 1., 0.]] {
        for c in v {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    assert_eq!(bin.len(), 44);
    (gltf, bin)
}

/// Parse a GLB, asserting the container invariants along the way (magic, version 2, declared total ==
/// actual length, 4-byte-aligned chunks, chunk type ids). Returns the JSON chunk parsed + the BIN chunk
/// bytes (`None` when the GLB has no BIN chunk).
fn parse_glb(glb: &[u8]) -> (Value, Option<Vec<u8>>) {
    let u32_at = |off: usize| u32::from_le_bytes(glb[off..off + 4].try_into().unwrap());
    assert!(glb.len() >= 20, "GLB too short: {} bytes", glb.len());
    assert_eq!(&glb[..4], b"glTF", "GLB magic");
    assert_eq!(u32_at(4), 2, "GLB version");
    assert_eq!(
        u32_at(8) as usize,
        glb.len(),
        "declared total length == actual"
    );
    let json_len = u32_at(12) as usize;
    assert_eq!(u32_at(16), 0x4E4F_534A, "first chunk is JSON");
    assert_eq!(json_len % 4, 0, "JSON chunk 4-byte aligned");
    let json_chunk = &glb[20..20 + json_len];
    // Spec: the JSON chunk is padded with trailing SPACES (0x20), which serde_json tolerates.
    let json: Value = serde_json::from_slice(json_chunk).expect("JSON chunk parses");
    let mut off = 20 + json_len;
    let bin = if off < glb.len() {
        let bin_len = u32_at(off) as usize;
        assert_eq!(u32_at(off + 4), 0x004E_4942, "second chunk is BIN\\0");
        assert_eq!(bin_len % 4, 0, "BIN chunk 4-byte aligned");
        let chunk = glb[off + 8..off + 8 + bin_len].to_vec();
        off += 8 + bin_len;
        assert_eq!(off, glb.len(), "no trailing bytes after the BIN chunk");
        Some(chunk)
    } else {
        None
    };
    (json, bin)
}

#[test]
fn packs_external_bin_triangle_into_self_contained_glb() {
    let (gltf, bin) = triangle_gltf();
    let packed = pack_gltf_to_glb(gltf.as_bytes(), |uri| {
        (uri == "tri.bin").then(|| bin.clone())
    })
    .expect("pack succeeds");
    assert!(is_glb_bytes(&packed), "output starts with the GLB magic");

    let (json, bin_chunk) = parse_glb(&packed);
    // The single GLB buffer: uri-less, byteLength == the merged data.
    let buffers = json["buffers"].as_array().unwrap();
    assert_eq!(buffers.len(), 1);
    assert_eq!(buffers[0]["byteLength"], 44);
    assert!(
        buffers[0].get("uri").is_none(),
        "GLB buffer must carry no uri"
    );
    // bufferViews re-pointed at buffer 0 with their offsets intact (single source buffer at offset 0).
    let views = json["bufferViews"].as_array().unwrap();
    assert_eq!(views[0]["buffer"], 0);
    assert_eq!(views[0]["byteOffset"], 0);
    assert_eq!(views[1]["buffer"], 0);
    assert_eq!(views[1]["byteOffset"], 8);
    // Accessors survive untouched.
    assert_eq!(json["accessors"][0]["componentType"], 5123);
    assert_eq!(json["accessors"][1]["type"], "VEC3");
    // The BIN chunk holds exactly the companion bytes (chunk itself already 44 = 4-aligned).
    let bin_chunk = bin_chunk.expect("has a BIN chunk");
    assert_eq!(&bin_chunk[..44], &bin[..], "buffer bytes present verbatim");
}

#[test]
fn packing_is_deterministic() {
    let (gltf, bin) = triangle_gltf();
    let mut resolve = |uri: &str| (uri == "tri.bin").then(|| bin.clone());
    let a = pack_gltf_to_glb(gltf.as_bytes(), &mut resolve).unwrap();
    let b = pack_gltf_to_glb(gltf.as_bytes(), &mut resolve).unwrap();
    assert_eq!(
        a, b,
        "same input must pack to bit-identical GLB (stable content @sha)"
    );
}

#[test]
fn data_uri_buffer_decodes_without_the_resolver() {
    // 8 bytes 0..=7, base64 "AAECAwQFBgc=".
    let gltf = br#"{
      "asset": {"version": "2.0"},
      "bufferViews": [{"buffer": 0, "byteLength": 8}],
      "buffers": [{"uri": "data:application/octet-stream;base64,AAECAwQFBgc=", "byteLength": 8}]
    }"#;
    let packed = pack_gltf_to_glb(gltf, |uri| -> Option<Vec<u8>> {
        panic!("resolver must not be called for a data: uri (asked for {uri:?})")
    })
    .expect("data uri packs");
    let (json, bin) = parse_glb(&packed);
    assert_eq!(json["buffers"][0]["byteLength"], 8);
    assert!(json["buffers"][0].get("uri").is_none());
    assert_eq!(&bin.unwrap()[..8], &[0, 1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn multiple_buffers_merge_aligned_with_offsets_rewritten() {
    // Buffer 0 is 5 bytes, so buffer 1 must land at offset 8 (4-byte alignment) and every view on
    // buffer 1 shifts by 8.
    let gltf = br#"{
      "asset": {"version": "2.0"},
      "bufferViews": [
        {"buffer": 0, "byteOffset": 0, "byteLength": 5},
        {"buffer": 1, "byteOffset": 0, "byteLength": 4}
      ],
      "buffers": [
        {"uri": "a.bin", "byteLength": 5},
        {"uri": "b.bin", "byteLength": 4}
      ]
    }"#;
    let packed = pack_gltf_to_glb(gltf, |uri| match uri {
        "a.bin" => Some(vec![0xAA; 5]),
        "b.bin" => Some(vec![0xBB; 4]),
        _ => None,
    })
    .unwrap();
    let (json, bin) = parse_glb(&packed);
    let bin = bin.unwrap();
    assert_eq!(
        json["buffers"].as_array().unwrap().len(),
        1,
        "merged to ONE GLB buffer"
    );
    assert_eq!(json["buffers"][0]["byteLength"], 12);
    assert_eq!(json["bufferViews"][0]["byteOffset"], 0);
    assert_eq!(json["bufferViews"][1]["buffer"], 0);
    assert_eq!(
        json["bufferViews"][1]["byteOffset"], 8,
        "buffer 1 lands 4-byte aligned"
    );
    assert_eq!(&bin[..5], &[0xAA; 5]);
    assert_eq!(&bin[5..8], &[0, 0, 0], "inter-buffer padding is zeros");
    assert_eq!(&bin[8..12], &[0xBB; 4]);
}

#[test]
fn external_images_inline_as_buffer_views() {
    // One png (mime sniffed from the extension), one jpg with a DECLARED mimeType (must win): the
    // Perseverance CHASSIS.gltf shape. No pre-existing bufferViews: the image views are created.
    let gltf = br#"{
      "asset": {"version": "2.0"},
      "images": [
        {"uri": "tex.png"},
        {"mimeType": "image/jpeg", "uri": "Textures/Rover.jpg"}
      ]
    }"#;
    let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
    let jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 9, 9];
    let packed = pack_gltf_to_glb(gltf, |uri| match uri {
        "tex.png" => Some(png.clone()),
        "Textures/Rover.jpg" => Some(jpg.clone()),
        _ => None,
    })
    .unwrap();
    let (json, bin) = parse_glb(&packed);
    let bin = bin.unwrap();
    let images = json["images"].as_array().unwrap();
    for (i, mime) in [(0, "image/png"), (1, "image/jpeg")] {
        assert!(images[i].get("uri").is_none(), "image {i} uri dropped");
        assert_eq!(images[i]["mimeType"], mime);
    }
    let views = json["bufferViews"].as_array().unwrap();
    let v0 = (
        views[0]["byteOffset"].as_u64().unwrap() as usize,
        views[0]["byteLength"].as_u64().unwrap() as usize,
    );
    let v1 = (
        views[1]["byteOffset"].as_u64().unwrap() as usize,
        views[1]["byteLength"].as_u64().unwrap() as usize,
    );
    assert_eq!(images[0]["bufferView"], 0);
    assert_eq!(images[1]["bufferView"], 1);
    assert_eq!(&bin[v0.0..v0.0 + v0.1], &png[..], "png bytes inlined");
    assert_eq!(&bin[v1.0..v1.0 + v1.1], &jpg[..], "jpg bytes inlined");
    assert_eq!(
        json["buffers"][0]["byteLength"].as_u64().unwrap() as usize,
        v1.0 + v1.1
    );
}

#[test]
fn identical_image_bytes_inline_once() {
    // Two image entries resolving to the SAME bytes share one bufferView (content dedup, first-use
    // order); a third distinct image gets its own. Every entry keeps its own mimeType.
    let gltf = br#"{
      "asset": {"version": "2.0"},
      "images": [{"uri": "a.png"}, {"uri": "copy-of-a.png"}, {"uri": "b.png"}]
    }"#;
    let png_a = vec![0x89, b'P', b'N', b'G', 0xAA, 0xAA];
    let png_b = vec![0x89, b'P', b'N', b'G', 0xBB];
    let packed = pack_gltf_to_glb(gltf, |uri| match uri {
        "a.png" | "copy-of-a.png" => Some(png_a.clone()),
        "b.png" => Some(png_b.clone()),
        _ => None,
    })
    .unwrap();
    let (json, bin) = parse_glb(&packed);
    let images = json["images"].as_array().unwrap();
    assert_eq!(
        images[0]["bufferView"], images[1]["bufferView"],
        "identical bytes share a view"
    );
    assert_ne!(
        images[0]["bufferView"], images[2]["bufferView"],
        "distinct bytes get their own"
    );
    let views = json["bufferViews"].as_array().unwrap();
    assert_eq!(views.len(), 2, "TWO views for three images: {json}");
    let bin = bin.unwrap();
    let v = |i: usize| {
        let (o, l) = (
            views[i]["byteOffset"].as_u64().unwrap() as usize,
            views[i]["byteLength"].as_u64().unwrap() as usize,
        );
        &bin[o..o + l]
    };
    assert_eq!(v(0), &png_a[..]);
    assert_eq!(v(1), &png_b[..]);
}

#[test]
fn extensionless_image_mime_sniffed_from_magic() {
    // No mimeType, no useful extension, so the resolved bytes' PNG magic decides. Junk bytes with the
    // same shape stay a LOUD unresolved error (never a mystery-mime image in a GLB).
    let gltf = br#"{"asset": {"version": "2.0"}, "images": [{"uri": "textures/skin"}]}"#;
    let png = b"\x89PNG\r\n\x1a\n-tiny".to_vec();
    let packed = pack_gltf_to_glb(gltf, |_| Some(png.clone())).expect("magic-sniffed image packs");
    let (json, _) = parse_glb(&packed);
    assert_eq!(
        json["images"][0]["mimeType"], "image/png",
        "mime sniffed from content: {json}"
    );

    let err = pack_gltf_to_glb(gltf, |_| Some(b"not an image at all".to_vec()))
        .expect_err("undeterminable mime must not pack");
    assert!(
        err.to_string().contains("textures/skin"),
        "error names the offending image: {err}"
    );
}

/// Real-file check (skipped when absent): the Perseverance CHASSIS.gltf, with an external CHASSIS.bin
/// buffer and a Textures/M2020_Rover_Texture.jpg image, packs into one self-contained deterministic
/// GLB with the image inlined as a bufferView.
#[test]
fn packs_real_chassis_gltf_with_external_texture() {
    // Out-of-repo staging dir ($HCDF_STAGING_DIR, default ~/hcdf-test-staging), skipped when the
    // Perseverance trio is not staged there.
    let base = std::env::var_os("HCDF_STAGING_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default()
                .join("hcdf-test-staging")
        });
    let gltf_path = base.join("CHASSIS.gltf");
    if !gltf_path.is_file()
        || !base.join("CHASSIS.bin").is_file()
        || !base.join("Textures/M2020_Rover_Texture.jpg").is_file()
    {
        eprintln!("(no staged CHASSIS.gltf trio; skipping the real-file pack check)");
        return;
    }
    let gltf = std::fs::read(&gltf_path).unwrap();
    let mut resolve = |uri: &str| std::fs::read(base.join(uri)).ok();
    let packed = pack_gltf_to_glb(&gltf, &mut resolve).expect("CHASSIS.gltf packs");
    assert!(is_glb_bytes(&packed));
    let (json, bin) = parse_glb(&packed);
    let img = &json["images"][0];
    assert!(
        img.get("uri").is_none(),
        "image uri rewritten to a bufferView: {img}"
    );
    assert_eq!(img["mimeType"], "image/jpeg");
    let bv = &json["bufferViews"][img["bufferView"].as_u64().unwrap() as usize];
    let (off, len) = (
        bv["byteOffset"].as_u64().unwrap() as usize,
        bv["byteLength"].as_u64().unwrap() as usize,
    );
    let jpg = std::fs::read(base.join("Textures/M2020_Rover_Texture.jpg")).unwrap();
    assert_eq!(
        &bin.as_ref().unwrap()[off..off + len],
        &jpg[..],
        "JPEG bytes inlined verbatim"
    );
    // Deterministic: bit-identical on a re-pack (stable content @sha for the vendored asset).
    assert_eq!(packed, pack_gltf_to_glb(&gltf, &mut resolve).unwrap());
}

#[test]
fn unresolved_references_error_lists_every_offender() {
    // A missing .bin, an unsupported image container, AND a missing png: all three must be named in
    // ONE error (fix-list semantics), and no GLB may be emitted.
    let gltf = br#"{
      "asset": {"version": "2.0"},
      "images": [{"uri": "skin.ktx2"}, {"uri": "missing.png"}],
      "buffers": [{"uri": "gone.bin", "byteLength": 4}]
    }"#;
    let err = pack_gltf_to_glb(gltf, |_| None).expect_err("must not emit a broken GLB");
    let GltfPackError::Unresolved(refs) = &err else {
        panic!("expected Unresolved, got: {err}");
    };
    assert_eq!(refs.len(), 3, "every offender listed once: {refs:?}");
    let joined = err.to_string();
    for needle in ["gone.bin", "skin.ktx2", "missing.png"] {
        assert!(
            joined.contains(needle),
            "error must name {needle}: {joined}"
        );
    }
}

#[test]
fn short_companion_bytes_error_cleanly() {
    let (gltf, _) = triangle_gltf();
    let err = pack_gltf_to_glb(gltf.as_bytes(), |_| Some(vec![0u8; 10]))
        .expect_err("10 bytes cannot satisfy byteLength 44");
    let msg = err.to_string();
    assert!(
        msg.contains("44") && msg.contains("10"),
        "names declared vs actual: {msg}"
    );
}

// ── the native vendor/bake wiring (assets_vendor is `cfg(not(wasm32))`) ─────────────────────────────
#[cfg(not(target_arch = "wasm32"))]
mod vendor_wiring {
    use super::*;
    use hcdformat::assets_vendor::{bake, AssetKind};
    use std::path::PathBuf;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "hcdf-gltfpack-{tag}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A REAL GLB (magic-checked) vendors byte-identically with its name/ext untouched: the Python
    /// byte-parity passthrough is unchanged by the packer.
    #[test]
    fn real_glb_passes_through_byte_identical() {
        let (gltf, bin) = triangle_gltf();
        let glb = pack_gltf_to_glb(gltf.as_bytes(), |_| Some(bin.clone())).unwrap();

        let d = tmp_dir("glb-pass");
        let src = d.join("tri.glb");
        std::fs::write(&src, &glb).unwrap();
        let out = d.join("assets");
        let baked = bake(&src, AssetKind::Visual, &out).unwrap();
        assert!(
            baked.name.ends_with(".glb"),
            "keeps its extension: {}",
            baked.name
        );
        let vendored = std::fs::read(out.join(&baked.name)).unwrap();
        assert_eq!(vendored, glb, "GLB passthrough must be byte-identical");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A `.gltf` visual vendors as the PACKED self-contained `<stem>_<short12>.glb` (its `.bin`
    /// companion resolved next to it and inlined), deterministically.
    #[test]
    fn gltf_visual_bakes_to_packed_self_contained_glb() {
        let (gltf, bin) = triangle_gltf();
        let d = tmp_dir("gltf-pack");
        std::fs::write(d.join("tri.gltf"), gltf).unwrap();
        std::fs::write(d.join("tri.bin"), &bin).unwrap();
        let out = d.join("assets");

        let baked = bake(&d.join("tri.gltf"), AssetKind::Visual, &out).unwrap();
        assert!(
            baked.name.starts_with("tri_") && baked.name.ends_with(".glb"),
            "content-addressed .glb name: {}",
            baked.name
        );
        let vendored = std::fs::read(out.join(&baked.name)).unwrap();
        assert!(is_glb_bytes(&vendored));
        let (json, bin_chunk) = parse_glb(&vendored);
        assert!(
            json["buffers"][0].get("uri").is_none(),
            "self-contained: no external buffer"
        );
        assert_eq!(&bin_chunk.unwrap()[..44], &bin[..]);

        // Deterministic: re-baking yields the same content-addressed name (same bytes, same @sha).
        let again = bake(&d.join("tri.gltf"), AssetKind::Visual, &out).unwrap();
        assert_eq!(again.name, baked.name);
        assert_eq!(again.sha, baked.sha);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A `.gltf` COLLISION mesh vendors as the PACKED self-contained `<stem>_<short12>.glb` (its
    /// `.bin` companion resolved next to it and inlined), exactly like a visual; byte-copying it would
    /// sever the relative `.bin` link under the content-hash rename. Mirrors
    /// [`gltf_visual_bakes_to_packed_self_contained_glb`] for the collision lane.
    #[test]
    fn gltf_collision_bakes_to_packed_self_contained_glb() {
        let (gltf, bin) = triangle_gltf();
        let d = tmp_dir("gltf-coll-pack");
        std::fs::write(d.join("hull.gltf"), gltf).unwrap();
        std::fs::write(d.join("tri.bin"), &bin).unwrap();
        let out = d.join("assets");

        let baked = bake(&d.join("hull.gltf"), AssetKind::Collision, &out).unwrap();
        assert!(
            baked.name.starts_with("hull_") && baked.name.ends_with(".glb"),
            "content-addressed .glb name (packed, not byte-copied .gltf): {}",
            baked.name
        );
        let vendored = std::fs::read(out.join(&baked.name)).unwrap();
        assert!(
            is_glb_bytes(&vendored),
            "collision packed to a valid self-contained GLB (glTF magic)"
        );
        let (json, bin_chunk) = parse_glb(&vendored);
        assert!(
            json["buffers"][0].get("uri").is_none(),
            "self-contained: no external buffer"
        );
        assert_eq!(
            &bin_chunk.unwrap()[..44],
            &bin[..],
            "external .bin inlined into the GLB buffer"
        );

        // Deterministic: re-baking yields the same content-addressed name (same bytes, same @sha).
        let again = bake(&d.join("hull.gltf"), AssetKind::Collision, &out).unwrap();
        assert_eq!(again.name, baked.name);
        assert_eq!(again.sha, baked.sha);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A NON-`.gltf` collision (a lean binary STL here) is UNCHANGED by the fix: byte-copied verbatim
    /// with its extension preserved, exactly the Python parity path. Only the `.gltf` byte-copy case
    /// changed.
    #[test]
    fn non_gltf_collision_passes_through_byte_identical() {
        let d = tmp_dir("stl-coll-pass");
        let stl = b"solid box\nfacet deterministic collision\nendsolid\n".to_vec();
        let src = d.join("hull.stl");
        std::fs::write(&src, &stl).unwrap();
        let out = d.join("assets");
        let baked = bake(&src, AssetKind::Collision, &out).unwrap();
        assert!(
            baked.name.ends_with(".stl"),
            "STL keeps its extension: {}",
            baked.name
        );
        assert_eq!(
            std::fs::read(out.join(&baked.name)).unwrap(),
            stl,
            "STL collision byte-identical"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The passthrough-vs-pack decision is the CONTENT's GLB magic, never the extension: a `.gltf`
    /// file whose bytes ARE a GLB passes through verbatim, and a `.glb` file whose bytes are `.gltf`
    /// JSON gets packed.
    #[test]
    fn magic_check_beats_extension_trust() {
        let (gltf, bin) = triangle_gltf();
        let glb = pack_gltf_to_glb(gltf.as_bytes(), |_| Some(bin.clone())).unwrap();
        let d = tmp_dir("magic");
        let out = d.join("assets");

        // GLB bytes named ".gltf" → verbatim passthrough (no JSON parse, no companions needed).
        let misnamed_glb = d.join("actually_binary.gltf");
        std::fs::write(&misnamed_glb, &glb).unwrap();
        let baked = bake(&misnamed_glb, AssetKind::Visual, &out).unwrap();
        assert_eq!(
            std::fs::read(out.join(&baked.name)).unwrap(),
            glb,
            "binary .gltf passes through"
        );

        // JSON bytes named ".glb" → packed (companion resolved next to the source).
        let misnamed_gltf = d.join("actually_json.glb");
        std::fs::write(&misnamed_gltf, gltf).unwrap();
        std::fs::write(d.join("tri.bin"), &bin).unwrap();
        let baked = bake(&misnamed_gltf, AssetKind::Visual, &out).unwrap();
        let vendored = std::fs::read(out.join(&baked.name)).unwrap();
        assert!(
            is_glb_bytes(&vendored),
            "JSON .glb was packed to a real GLB"
        );
        assert_ne!(vendored, gltf.as_bytes(), "not a byte-copy");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A `.gltf` whose companion is MISSING errors loudly (naming the file and the reference) instead
    /// of vendoring a broken asset.
    #[test]
    fn missing_companion_is_a_loud_error() {
        let (gltf, _) = triangle_gltf();
        let d = tmp_dir("gltf-missing");
        std::fs::write(d.join("tri.gltf"), gltf).unwrap(); // NO tri.bin next to it
        let err = bake(&d.join("tri.gltf"), AssetKind::Visual, &d.join("assets"))
            .expect_err("missing .bin must not vendor");
        let msg = err.to_string();
        assert!(
            msg.contains("tri.bin"),
            "error names the missing companion: {msg}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
