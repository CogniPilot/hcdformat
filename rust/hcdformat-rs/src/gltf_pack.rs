//! glTF → GLB packer: turn a `.gltf` JSON document, whose binary payload lives in EXTERNAL companion
//! files (`buffers[].uri = "CHASSIS.bin"`, `images[].uri = "Textures/….jpg"`) or in `data:` URIs, into
//! ONE self-contained, spec-correct binary GLB.
//!
//! ## Why (the vendored-asset severing problem)
//! An HCDF visual `<model>` is a SINGLE self-contained asset: the vendor/bake step content-addresses it
//! to `<stem>_<short12sha>.<ext>` inside `assets/`. A multi-file `.gltf` cannot survive that step: the
//! rename severs the JSON's RELATIVE `uri` links to its `.bin`/texture companions (which are never
//! vendored at all), leaving a broken asset in the bundle (the Perseverance rover's 12 `.gltf`+`.bin`
//! pairs are the real-world case). Packing to GLB *before* content-addressing makes the asset
//! self-contained, so the rename is harmless, and one GLB is exactly the shape the HCDF visual grammar
//! wants a `<model>` to be.
//!
//! ## What packing does
//!   * every `buffers[i]` is resolved (`data:` URIs base64-decoded; any other uri percent-decoded and
//!     handed to the caller's `resolve` lookup) and its bytes concatenated (each 4-byte aligned) into
//!     the GLB BIN chunk; `bufferViews[*]` are re-pointed at merged buffer `0` with shifted
//!     `byteOffset`s (internal alignment is preserved: every buffer lands on a 4-byte boundary);
//!   * every `images[i].uri` (an external `.png`/`.jpg`, or a `data:` URI) is inlined as a NEW
//!     `bufferView` + `mimeType`, the way the glTF spec stores GLB-internal images. The mime is the
//!     image's own `mimeType` field when present, else the data-uri header, else sniffed from the
//!     `.png`/`.jpg`/`.jpeg` extension;
//!   * `buffers` is rewritten to the single uri-less GLB buffer carrying the merged `byteLength`;
//!   * the JSON chunk is padded with SPACES and the BIN chunk with ZEROS to 4-byte boundaries, framed
//!     with the `glTF` header (magic / version 2 / total length), the layout `bake.rs`'s writer uses.
//!
//! Output is DETERMINISTIC: serde_json's default map is a `BTreeMap` (key-sorted serialization), floats
//! print via ryu shortest-round-trip, and the chunk layout is fixed: the same input bytes + resolver
//! contents always produce the same GLB (and therefore the same content `@sha`).
//!
//! Any reference that CANNOT be made self-contained (a missing companion file, a remote uri, an image
//! type that is neither PNG nor JPEG and declares no mime) is a loud [`GltfPackError::Unresolved`]
//! listing EVERY offender. The packer never emits a silently-broken GLB.
//!
//! ## Callers
//! Wired into the native vendor/bake visual passthrough ([`crate::assets_vendor`]) so a `.gltf` visual
//! packs on every native path (CLI `convert --bake`, `hcdf bundle`, dendrite_build's desktop import,
//! which all route through [`crate::assets_vendor::bake`] / the vendor walk). WASM-CLEAN and
//! dependency-lean (serde_json only, no filesystem access): a browser importer (dendrite_build's
//! `MemFs`) calls [`pack_gltf_to_glb`] directly with a closure over its in-memory upload map, e.g.
//! `pack_gltf_to_glb(&bytes, |uri| self.resolve_mesh(uri, base_dir))`. GLB *detection* is
//! [`is_glb_bytes`]: a magic check on the CONTENT, never extension trust (a JSON-bodied `.glb` packs,
//! a binary-bodied `.gltf` passes through).

use serde_json::{json, Value};

// GLB container constants (the glTF 2.0 spec values), shared with `bake.rs`'s deterministic writer.
/// `"glTF"` little-endian: the 4-byte magic every GLB starts with.
pub(crate) const GLB_MAGIC: u32 = 0x4654_6C67;
/// GLB container version (2).
pub(crate) const GLB_VERSION: u32 = 2;
/// `"JSON"` chunk type.
pub(crate) const CHUNK_JSON: u32 = 0x4E4F_534A;
/// `"BIN\0"` chunk type.
pub(crate) const CHUNK_BIN: u32 = 0x004E_4942;

/// True iff `bytes` begin with the 4-byte GLB magic `glTF`: the content check every "already a GLB,
/// pass it through" decision must use. Extension trust would misroute a JSON-bodied `.glb` (needs
/// packing) or a binary-bodied `.gltf` (already self-contained).
pub fn is_glb_bytes(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == GLB_MAGIC.to_le_bytes()
}

/// What can go wrong while packing a `.gltf` into a self-contained GLB. Both variants are LOUD by
/// design; the alternative to an error here is a GLB with severed/broken references.
#[derive(Debug, thiserror::Error)]
pub enum GltfPackError {
    /// The bytes are not a packable `.gltf` document: JSON parse failure, a structurally-invalid
    /// `buffer`/`bufferView`/`image` entry, an oversized result, or a buffer-rewriting extension the
    /// packer cannot preserve (`EXT_meshopt_compression`).
    #[error("not a packable .gltf document: {0}")]
    Invalid(String),
    /// External references that could not be made self-contained (missing companion files, remote uris,
    /// unsupported image types): EVERY offender listed so the caller can name exactly what to fix.
    #[error("cannot pack into a self-contained GLB; unresolved external references: {}", .0.join("; "))]
    Unresolved(Vec<String>),
}

/// Pack a `.gltf` JSON document into a single self-contained GLB (see the module docs for the layout).
///
/// `resolve` supplies the bytes of one external companion by its PERCENT-DECODED, `.gltf`-relative uri
/// (`"CHASSIS.bin"`, `"Textures/M2020_Rover_Texture.jpg"`), returning `None` when it cannot; the
/// caller owns resolution policy (native: read next to the `.gltf` on disk; browser: look up the
/// in-memory upload map). `data:` URIs are decoded internally and never reach `resolve`.
///
/// Fails loudly ([`GltfPackError`]) rather than emitting a broken GLB; on success the returned bytes
/// start with the GLB magic and reference no external resource.
pub fn pack_gltf_to_glb(
    gltf_json: &[u8],
    mut resolve: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, GltfPackError> {
    let mut root: Value = serde_json::from_slice(gltf_json)
        .map_err(|e| GltfPackError::Invalid(format!("invalid JSON: {e}")))?;
    let Some(obj) = root.as_object_mut() else {
        return Err(GltfPackError::Invalid(
            "root is not a JSON object".to_string(),
        ));
    };

    // First pass: resolve EVERY external reference before mutating anything, collecting ALL failures so
    // one error names the complete fix list (not just the first missing file).
    let mut unresolved: Vec<String> = Vec::new();
    let buffer_datas = resolve_buffers(obj, &mut resolve, &mut unresolved)?;
    let image_datas = resolve_images(obj, &mut resolve, &mut unresolved)?;
    if !unresolved.is_empty() {
        return Err(GltfPackError::Unresolved(unresolved));
    }

    // Second pass: build the merged BIN chunk and rewrite the DOM to point into it.
    let mut bin: Vec<u8> = Vec::with_capacity(buffer_datas.iter().map(Vec::len).sum());
    let offsets: Vec<usize> = buffer_datas
        .iter()
        .map(|d| append_aligned(&mut bin, d))
        .collect();
    rewrite_buffer_views(obj, &offsets)?;
    inline_images(obj, &image_datas, &mut bin);
    if bin.is_empty() {
        // A geometry-less .gltf (nothing external, nothing embedded): a GLB may omit the BIN chunk, and
        // a zero-length buffer entry would be spec-invalid, so drop the (empty) array if present.
        obj.remove("buffers");
    } else {
        obj.insert("buffers".to_string(), json!([{ "byteLength": bin.len() }]));
    }

    let json_bytes = serde_json::to_vec(&root).map_err(|e| {
        GltfPackError::Invalid(format!("could not re-serialize the JSON chunk: {e}"))
    })?;
    // The GLB header fields are u32; padded chunk sizes must fit (12B header + 8B/chunk framing).
    let worst = 12 + 8 + json_bytes.len() + 3 + 8 + bin.len() + 3;
    if worst > u32::MAX as usize {
        return Err(GltfPackError::Invalid(format!(
            "packed GLB would be {worst} bytes, larger than the 4 GiB the GLB u32 header can address"
        )));
    }
    Ok(assemble_glb(json_bytes, bin))
}

/// Resolve every `buffers[i]` to its bytes, TRUNCATED to the declared `byteLength` (the spec allows the
/// resource to be longer; consumers read exactly `byteLength`). A structural problem (no uri, no
/// byteLength, short data, bad base64) is an immediate [`GltfPackError::Invalid`]; a merely-missing
/// companion is appended to `unresolved` (with a placeholder entry keeping indices aligned) so every
/// offender is reported at once.
fn resolve_buffers(
    obj: &serde_json::Map<String, Value>,
    resolve: &mut impl FnMut(&str) -> Option<Vec<u8>>,
    unresolved: &mut Vec<String>,
) -> Result<Vec<Vec<u8>>, GltfPackError> {
    let Some(bufs) = obj.get("buffers") else {
        return Ok(Vec::new());
    };
    let arr = bufs
        .as_array()
        .ok_or_else(|| GltfPackError::Invalid("\"buffers\" is not an array".to_string()))?;
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(arr.len());
    for (i, b) in arr.iter().enumerate() {
        let declared = b
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| GltfPackError::Invalid(format!("buffer {i} has no byteLength")))?
            as usize;
        let Some(uri) = b.get("uri").and_then(Value::as_str) else {
            return Err(GltfPackError::Invalid(format!(
                "buffer {i} has no uri (a uri-less buffer is only valid inside a GLB, not a .gltf)"
            )));
        };
        let data = if uri.starts_with("data:") {
            Some(
                decode_data_uri(uri)
                    .map_err(|e| GltfPackError::Invalid(format!("buffer {i}: {e}")))?
                    .1,
            )
        } else {
            resolve(&percent_decode(uri))
        };
        let Some(mut data) = data else {
            unresolved.push(format!("buffer {i}: {uri:?} ({})", why_unresolved(uri)));
            out.push(Vec::new()); // placeholder keeps buffer indices aligned for the offset table
            continue;
        };
        if data.len() < declared {
            return Err(GltfPackError::Invalid(format!(
                "buffer {i} ({uri:?}): resolved {} bytes but byteLength declares {declared}",
                data.len()
            )));
        }
        data.truncate(declared);
        out.push(data);
    }
    Ok(out)
}

/// One image to be inlined into the BIN chunk: its raw bytes + mime type. `None` for an image that is
/// already GLB-internal (stored via `bufferView`) or could not be resolved (reported separately).
type InlineImage = Option<(Vec<u8>, String)>;

/// Resolve every uri-carrying `images[i]` to `(bytes, mime)`; an image already stored via `bufferView`
/// (or with no uri) stays `None`. The mime is the image's own declared `mimeType`, else the
/// `.png`/`.jpg` extension, else the resolved bytes' magic ([`sniff_image_mime`]), so an
/// extension-less or oddly-named companion still inlines when its bytes ARE PNG/JPEG. A missing
/// companion or an undeterminable mime type goes to `unresolved` (all offenders reported together); a
/// malformed `data:` uri is an immediate error.
fn resolve_images(
    obj: &serde_json::Map<String, Value>,
    resolve: &mut impl FnMut(&str) -> Option<Vec<u8>>,
    unresolved: &mut Vec<String>,
) -> Result<Vec<InlineImage>, GltfPackError> {
    let Some(images) = obj.get("images") else {
        return Ok(Vec::new());
    };
    let arr = images
        .as_array()
        .ok_or_else(|| GltfPackError::Invalid("\"images\" is not an array".to_string()))?;
    let mut out: Vec<Option<(Vec<u8>, String)>> = Vec::with_capacity(arr.len());
    for (i, img) in arr.iter().enumerate() {
        let Some(uri) = img.get("uri").and_then(Value::as_str) else {
            out.push(None); // already GLB-internal (bufferView), nothing to inline
            continue;
        };
        // The image's own declared mimeType wins; a data-uri header, a .png/.jpg extension, or (once
        // the bytes are in hand) the content magic back it up.
        let declared_mime = img
            .get("mimeType")
            .and_then(Value::as_str)
            .map(str::to_string);
        if uri.starts_with("data:") {
            let (uri_mime, data) = decode_data_uri(uri)
                .map_err(|e| GltfPackError::Invalid(format!("image {i}: {e}")))?;
            out.push(Some((data, declared_mime.unwrap_or(uri_mime))));
            continue;
        }
        let Some(data) = resolve(&percent_decode(uri)) else {
            unresolved.push(format!("image {i}: {uri:?} ({})", why_unresolved(uri)));
            out.push(None);
            continue;
        };
        let mime = declared_mime
            .or_else(|| mime_from_ext(uri).map(str::to_string))
            .or_else(|| sniff_image_mime(&data).map(str::to_string));
        let Some(mime) = mime else {
            unresolved.push(format!(
                "image {i}: {uri:?} (cannot determine its mime type; only PNG/JPEG external \
                 images are inlined; add a mimeType or convert the texture)"
            ));
            out.push(None);
            continue;
        };
        out.push(Some((data, mime)));
    }
    Ok(out)
}

/// Re-point every `bufferViews[i]` at the single merged GLB buffer: `buffer` → `0`, `byteOffset` shifted
/// by where its old buffer landed in the BIN chunk. Rejects `EXT_meshopt_compression` (that extension
/// carries its OWN buffer reference the packer cannot rewrite; passing it through would corrupt).
fn rewrite_buffer_views(
    obj: &mut serde_json::Map<String, Value>,
    offsets: &[usize],
) -> Result<(), GltfPackError> {
    let Some(views) = obj.get_mut("bufferViews") else {
        return Ok(());
    };
    let arr = views
        .as_array_mut()
        .ok_or_else(|| GltfPackError::Invalid("\"bufferViews\" is not an array".to_string()))?;
    for (i, view) in arr.iter_mut().enumerate() {
        let Some(vo) = view.as_object_mut() else {
            return Err(GltfPackError::Invalid(format!(
                "bufferView {i} is not an object"
            )));
        };
        if vo
            .get("extensions")
            .is_some_and(|e| e.get("EXT_meshopt_compression").is_some())
        {
            return Err(GltfPackError::Invalid(format!(
                "bufferView {i} uses EXT_meshopt_compression, whose internal buffer reference the \
                 packer cannot rewrite"
            )));
        }
        let b =
            vo.get("buffer").and_then(Value::as_u64).ok_or_else(|| {
                GltfPackError::Invalid(format!("bufferView {i} has no buffer index"))
            })? as usize;
        let base = *offsets.get(b).ok_or_else(|| {
            GltfPackError::Invalid(format!(
                "bufferView {i} references buffer {b}, but only {} buffer(s) exist",
                offsets.len()
            ))
        })?;
        let byte_offset = vo.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        vo.insert("buffer".to_string(), Value::from(0u64));
        vo.insert("byteOffset".to_string(), Value::from(base + byte_offset));
    }
    Ok(())
}

/// Land each resolved image's bytes in the BIN chunk as a NEW `bufferView` (appended after the existing
/// ones, so no other index moves) and rewrite the image to `bufferView` + `mimeType`, dropping `uri`:
/// the glTF-spec representation of a GLB-internal image. Images with IDENTICAL bytes share ONE
/// `bufferView` (content dedup, first-use order; a `.gltf` that lists the same texture file under two
/// image entries inlines it once); each image entry still keeps its own `mimeType`.
fn inline_images(
    obj: &mut serde_json::Map<String, Value>,
    image_datas: &[InlineImage],
    bin: &mut Vec<u8>,
) {
    if image_datas.iter().all(Option::is_none) {
        return;
    }
    let existing_views = obj
        .get("bufferViews")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let mut new_views: Vec<Value> = Vec::new();
    // Content dedup: (index into `image_datas` of the first use, its assigned bufferView index).
    let mut seen: Vec<(usize, usize)> = Vec::new();
    if let Some(arr) = obj.get_mut("images").and_then(Value::as_array_mut) {
        for (i, (img, data)) in arr.iter_mut().zip(image_datas).enumerate() {
            let (Some((bytes, mime)), Some(io)) = (data, img.as_object_mut()) else {
                continue;
            };
            let view_index = match seen
                .iter()
                .find(|(j, _)| image_datas[*j].as_ref().is_some_and(|(b, _)| b == bytes))
            {
                Some((_, view)) => *view,
                None => {
                    let off = append_aligned(bin, bytes);
                    let view_index = existing_views + new_views.len();
                    new_views
                        .push(json!({ "buffer": 0, "byteOffset": off, "byteLength": bytes.len() }));
                    seen.push((i, view_index));
                    view_index
                }
            };
            io.remove("uri");
            io.insert("bufferView".to_string(), Value::from(view_index));
            io.insert("mimeType".to_string(), Value::from(mime.as_str()));
        }
    }
    if !new_views.is_empty() {
        match obj.get_mut("bufferViews") {
            Some(Value::Array(a)) => a.extend(new_views),
            _ => {
                obj.insert("bufferViews".to_string(), Value::Array(new_views));
            }
        }
    }
}

/// Append `data` to `bin` starting at the next 4-byte boundary (zero inter-segment padding; the GLB
/// alignment every glTF accessor component size divides) and return the offset it landed at.
fn append_aligned(bin: &mut Vec<u8>, data: &[u8]) -> usize {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let off = bin.len();
    bin.extend_from_slice(data);
    off
}

/// Frame the JSON + BIN chunks as a spec-correct GLB: JSON padded with SPACES and BIN with ZEROS to
/// 4-byte boundaries (padding counts toward each chunk's length), 12-byte `glTF` header carrying the
/// total. The BIN chunk is omitted entirely when there is no binary payload.
fn assemble_glb(mut json: Vec<u8>, mut bin: Vec<u8>) -> Vec<u8> {
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(&bin);
    }
    out
}

/// Why a non-`data:` uri could not be resolved: a remote uri is a policy miss (the packer never
/// fetches), a relative one is a missing companion file. Sharpens the [`GltfPackError::Unresolved`] list.
fn why_unresolved(uri: &str) -> &'static str {
    let low = uri.to_ascii_lowercase();
    if low.starts_with("http://") || low.starts_with("https://") {
        "remote uris are not fetched by the packer"
    } else {
        "companion file not found"
    }
}

/// Decode a `data:[<mime>][;base64],<payload>` URI to `(mime, bytes)`. glTF stores binary data URIs
/// base64-encoded; a non-base64 one is an error (never silently mis-decoded). An empty mime falls back
/// to `application/octet-stream`.
fn decode_data_uri(uri: &str) -> Result<(String, Vec<u8>), String> {
    let rest = uri
        .strip_prefix("data:")
        .ok_or_else(|| "not a data: uri".to_string())?;
    let (header, payload) = rest
        .split_once(',')
        .ok_or_else(|| "malformed data: uri (no comma separator)".to_string())?;
    let Some(mime) = header.strip_suffix(";base64") else {
        return Err(format!(
            "data: uri is not base64-encoded (header {header:?})"
        ));
    };
    let mime = if mime.is_empty() {
        "application/octet-stream"
    } else {
        mime
    };
    Ok((mime.to_string(), base64_decode(payload)?))
}

/// The base64 value of one alphabet byte (standard alphabet), or `None` for anything else.
fn b64_val(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some(u32::from(c - b'A')),
        b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Standard-alphabet base64 decode (padding optional, whitespace tolerated; some exporters wrap long
/// data URIs). Kept hand-rolled so the base crate stays dependency-lean (~20 lines vs a new crate).
fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut nbits = 0u32;
    for c in s.bytes() {
        if c.is_ascii_whitespace() || c == b'=' {
            continue;
        }
        let v = b64_val(c).ok_or_else(|| format!("invalid base64 character {:?}", c as char))?;
        acc = (acc << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((acc >> nbits) as u8);
        }
    }
    Ok(out)
}

/// Percent-decode a uri (`%20` → space, …). An invalid escape is kept literally; bytes that do not form
/// UTF-8 fall back to the raw input (the resolver then sees the uri exactly as authored). Shared with
/// the baker's COLLADA `<init_from>` texture-uri handling (`crate::bake`).
pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// The glTF-core image mime for a uri's extension (`.png`/`.jpg`/`.jpeg`, case-insensitive), or `None`
/// for anything else (KTX2/WebP/DDS live behind glTF extensions the packer does not rewrite). Shared
/// with the baker's texture embedding (`crate::bake`), which follows the same PNG/JPEG-only rule.
pub(crate) fn mime_from_ext(uri: &str) -> Option<&'static str> {
    let (_, ext) = uri.rsplit_once('.')?;
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        _ => None,
    }
}

/// The glTF-core image mime sniffed from the CONTENT's magic bytes: PNG (`\x89PNG\r\n\x1a\n`) or JPEG
/// (`\xFF\xD8\xFF`), or `None` for anything else. The content check backs up a missing/false extension
/// (an extension-less texture uri, a COLLADA `<init_from>` naming a bare id) exactly like
/// [`is_glb_bytes`] backs up the `.glb` extension. Shared with the baker (`crate::bake`).
pub(crate) fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glb_magic_is_content_not_extension() {
        assert!(is_glb_bytes(b"glTF\x02\x00\x00\x00rest"));
        assert!(!is_glb_bytes(b"{\"asset\":{}}"));
        assert!(!is_glb_bytes(b"glT")); // too short
    }

    #[test]
    fn base64_decodes_standard_padding_and_whitespace() {
        assert_eq!(base64_decode("AAEC").unwrap(), vec![0, 1, 2]);
        assert_eq!(base64_decode("AA==").unwrap(), vec![0]);
        assert_eq!(base64_decode("AAE=").unwrap(), vec![0, 1]);
        assert_eq!(base64_decode("AA\nEC").unwrap(), vec![0, 1, 2]);
        assert!(base64_decode("A*==").is_err());
    }

    #[test]
    fn data_uri_decodes_mime_and_payload() {
        let (mime, data) = decode_data_uri("data:application/octet-stream;base64,AAEC").unwrap();
        assert_eq!(mime, "application/octet-stream");
        assert_eq!(data, vec![0, 1, 2]);
        // Empty mime falls back; non-base64 is a loud error.
        let (mime, _) = decode_data_uri("data:;base64,AA==").unwrap();
        assert_eq!(mime, "application/octet-stream");
        assert!(decode_data_uri("data:text/plain,hello").is_err());
    }

    #[test]
    fn percent_decoding_and_mime_sniff() {
        assert_eq!(percent_decode("CHASSIS%20v2.bin"), "CHASSIS v2.bin");
        assert_eq!(percent_decode("plain.bin"), "plain.bin");
        assert_eq!(percent_decode("bad%zz.bin"), "bad%zz.bin"); // invalid escape kept literal
        assert_eq!(mime_from_ext("Textures/T.JPG"), Some("image/jpeg"));
        assert_eq!(mime_from_ext("t.png"), Some("image/png"));
        assert_eq!(mime_from_ext("t.ktx2"), None);
        assert_eq!(mime_from_ext("noext"), None);
    }

    #[test]
    fn image_magic_sniff_is_content_not_extension() {
        assert_eq!(
            sniff_image_mime(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(
            sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00]),
            Some("image/jpeg")
        );
        assert_eq!(sniff_image_mime(b"\xabKTX 20\xbb"), None);
        assert_eq!(sniff_image_mime(b""), None);
    }
}
