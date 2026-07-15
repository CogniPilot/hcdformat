//! Asset baker: a pure-Rust, **wasm-clean** port of `hcdf/assets.py`'s mesh
//! conversion pipeline (`to_glb` / `to_lean` / the conversion half of `bake`).
//!
//! Python's baker is built on `trimesh` (+ `numpy`/`pygltflib`); there is no single Rust equivalent, so
//! this composes three small pure-Rust crates, each validated against real corpora:
//!
//!   * [`mesh_loader`]: load STL / OBJ source into positions + indices (+ normals/materials); its
//!     COLLADA parser is NOT compiled in (the `.dae` path is `dae-parser` under `bake-dae`, see below),
//!   * [`glam`]: the scale / mirror matrix, its determinant, and the winding/normal flip,
//!   * a HAND-ROLLED, fully-deterministic GLB writer (the GLB container + JSON chunk are small and we
//!     control every byte: fixed accessor / bufferView / mesh / node ordering, fixed buffer packing,
//!     a constant generator string, no timestamps, no `HashMap` iteration), so the same input yields
//!     the same bytes (and therefore the same `@sha`) on every run and every machine.
//!
//! ## What it does, per (kind, input-format, scale): mirroring `assets.py`
//!
//! ### Visual ([`to_glb`], the canonical baked appearance is GLB)
//!   * Load the source as a SCENE (every geometry kept separate, like trimesh `force="scene"`).
//!   * Bake `scale` (`[sx,sy,sz]`, possibly NEGATIVE for a mirror) into the VERTICES, not a node
//!     transform. A negative-determinant (mirror) scale is the case engines like gz/ogre2 get wrong if
//!     left as a node transform (the surface renders inside-out): folding it into the vertices and
//!     REVERSING the triangle winding wherever `det < 0` keeps every face outward so every renderer
//!     culls correctly, exactly what `Scene(scene.dump())` does in Python (winding flip on det<0).
//!   * Bake a flat `color` (`[r,g,b,a]` 0..1) as the GLB PBR base colour ONLY onto geometry that
//!     carries no material of its own (a bare STL whose colour lived in the URDF `<material>`); a mesh
//!     that already has a material (a `.dae`, an OBJ `usemtl` group bound to a resolved `.mtl`
//!     material) keeps it, matching Python's `TextureVisuals` guard.
//!   * Carry the source's AUTHORED per-vertex normals into the GLB `NORMAL` attribute (a smooth-shaded
//!     `.dae` / OBJ-with-`vn` / glTF) so smooth shading survives the bake. When a `scale`/mirror is
//!     baked into the positions the normals are baked too, transformed by the NORMAL matrix
//!     ([`normal_matrix`], the inverse-transpose of the scale) and renormalized, so a SCALED or
//!     MIRRORED instance keeps the same authored shading as its un-scaled twin. Only a plain STL
//!     (synthesized normals) stays POSITION-only. This DIVERGES from trimesh, whose `Scene(scene.dump())`
//!     scale/mirror round-trip discards stale normals and leaves a mirrored twin flat-shaded; we keep
//!     and correctly re-orient them instead.
//!   * Emit a deterministic embedded GLB.
//!
//! ### OBJ `.mtl` materials: resolved through a COMPANION RESOLVER (the `.gltf`-packer pattern)
//! An OBJ's appearance lives in a SEPARATE `.mtl` file (`mtllib digit.mtl` + per-group `usemtl`), so
//! baking from bytes alone can only ever produce gray. The OBJ load therefore takes the same
//! resolver contract as [`crate::gltf_pack::pack_gltf_to_glb`]: the caller supplies companion bytes by
//! their OBJ-relative uri (`"digit.mtl"`): the path-based entry points read siblings off disk, the
//! in-RAM importer passes its upload map through [`bake_convert_bytes_with`]. `mesh-loader` already
//! splits the face soup into one mesh per `usemtl` group; each group's material maps to PBR exactly
//! where MTL overlaps the DAE collapse (`baseColor` = `Kd`, `metallic` = 0, `roughness` =
//! `1 - glossiness` with glossiness the same normalized shininess heuristic; see `shader_to_pbr`).
//! `Ke` emissive is NOT carried (the writer emits no emissiveFactor); a `map_Kd` diffuse texture IS
//! embedded when it can be (see "Textures" below). An unresolved `mtllib` or a non-embeddable texture
//! is a NOTE on the result ([`BakedAsset::notes`]): a visible fallback, never an error, and never a
//! silent drop.
//!
//! ### Textures: embedded PNG/JPEG `baseColorTexture`, STRICTLY ADDITIVE
//! A diffuse texture (OBJ `map_Kd`; COLLADA `<texture>` diffuse, resolved through the
//! sampler2D → surface → `library_images` chain) is embedded into the GLB: image bytes passed
//! through VERBATIM (PNG/JPEG only, never re-encoded) as a `bufferView` + `mimeType` image, one
//! default sampler, a texture, and the material's `baseColorTexture`, with the mesh's texture
//! coordinates carried as `TEXCOORD_0` (V flipped, OBJ/COLLADA bottom-left origin → glTF top-left,
//! the standard exporter convention). Identical image bytes embed ONCE (content dedup, first-use
//! order). A textured material's `baseColorFactor` is WHITE (glTF multiplies factor × texture;
//! mirroring what common exporters do, trimesh included, so the texture shows unmodulated). When the
//! embedded PNG carries alpha (colour type 4/6 or a `tRNS` chunk, detected by a cheap header walk,
//! [`png_has_alpha`]) the material declares `alphaMode:"BLEND"`, so a transparent-background logo (the
//! b3rb NXP/CogniPilot decals) bakes TRANSPARENT instead of the old opaque black; the WHITE factor keeps
//! baseColorFactor alpha at 1.0 so the texture's own alpha drives it. An OPAQUE texture declares no
//! `alphaMode` (byte-identical to the pre-alpha writer). BLEND, not MASK: the logos are antialiased, so a
//! hard alphaCutoff would jag their soft edges.
//! Textures attach ONLY when the geometry has BOTH texture coordinates and resolvable PNG/JPEG image
//! bytes; each miss is a distinct note (image not resolved / unsupported format / mesh has no
//! texture coordinates) and the material falls back to its flat diffuse. **A bake with NO texture is
//! BYTE-IDENTICAL to the pre-texture writer**: no TEXCOORD accessor, no image/sampler/texture JSON,
//! and the weld ignores uv, pinned by `no_texture_bake_is_byte_identical_to_pretexture_output` in
//! `tests/bake.rs`.
//!
//! ### Hint textures: the importer's `<pbr><albedo_map>` side-channel
//! The SDF importer captures a visual material's `<pbr><albedo_map>` uri on the
//! [`crate::VisualAssetHint`] side-channel; the CALLER resolves it to image bytes (the same
//! package-root machinery mesh uris use) and hands it in as a [`HintTexture`]. On a MESH visual
//! ([`bake_convert_with_texture`] / [`bake_convert_bytes_with_texture`]) it embeds using the mesh's
//! OWN texture coordinates and reaches only MATERIAL-LESS geometry (a mesh's own materials/textures
//! win, exactly the flat-colour precedence), WHITE factor like every textured material (so a hint
//! texture also beats a hint colour for the baseColor slot). On a PRIMITIVE visual the baker
//! synthesizes a UV-mapped quad/box mesh instead ([`bake_textured_box`]; a plane imports as a
//! zero-thickness box) and the visual becomes an ordinary `<model uri>`; the HCDF document never
//! carries the texture path.
//!
//! ### Collision ([`to_lean`], stays lean: shape, not appearance)
//!   * Bake `scale` into the vertices (mirror-safe: winding reversed on `det < 0` so a physics engine
//!     like DART never sees a non-positive scale) and emit a lean **binary STL**.
//!   * **Deliberate divergence from Python: DOCUMENTED canonicalization.** Python's `bake()` calls
//!     `to_lean(src, scale, file_type=ext)`, KEEPING the collision SOURCE format (an OBJ collision stays
//!     `.obj` text, a DAE stays `.dae`), so Python's converted-collision `@uri`/`@sha`/extension are
//!     source-format-dependent. This Rust conversion path instead CANONICALIZES every scaled collision to
//!     binary STL (the universal lean format every physics engine reads). Rationale: the parity bar is
//!     SEMANTIC, not byte, and even an OBJ→OBJ Rust writer could not reproduce trimesh's exact
//!     OBJ bytes (welding order + `%.8f` formatting), so cross-writer `@sha` parity for a CONVERTED
//!     collision is unattainable in ANY format; each side recomputes `@sha` over its own bytes (exactly
//!     as for the GLB visual, see [`bake_convert`]), never cross-checked; canonicalizing to one format
//!     also makes the importer's collision output format-stable and dedups identical shapes regardless of
//!     source container. **Python↔Rust contract:** the importer driving this baker MUST treat a
//!     converted collision as canonical binary STL on BOTH sides (when the Python oracle is used for an
//!     importer-parity check it must pass `file_type="stl"`, not the source ext). The verbatim passthrough
//!     ([`crate::assets_vendor::bake`], `scale=None`) is UNAFFECTED: it keeps the source extension and
//!     bytes exactly like Python, so an already-baked HCDF document's collision `@uri`/`@sha`/ext stay
//!     byte-identical across tools; only the rare IMPORT-with-scale is normalized.
//!   * A `.dae` collision still requires `bake-dae` (else [`BakeError::DaeUnsupported`]); when enabled it
//!     is loaded, merged, scaled, and written as binary STL like any other lean source.
//!
//! ### Passthrough (no conversion): handled by [`crate::assets_vendor::bake`], NOT here
//!   * An already-GLB visual with no scale and no colour, and a collision mesh with no scale, are a
//!     verbatim byte-copy + hash (so a vendored bundle is byte-identical to Python's). That path lives
//!     in `assets_vendor` and is unchanged by this module; this module is the CONVERSION engine the
//!     URDF/SDF importer drives, and is exercised directly by `tests/bake.rs`.
//!
//! ## Parity bar
//! SEMANTIC, not byte: a Rust-baked GLB is NOT byte-identical to trimesh's (different writer) and that
//! is expected. The two real bars, both proven in `tests/bake.rs`:
//!   1. **self-determinism**: `bake(input)` is bit-identical across runs (same `content_sha` every
//!      time): no nondeterministic ordering anywhere in the writer;
//!   2. **semantic equivalence to Python**: load both GLBs and compare geometry: same vertex SET
//!      (within float epsilon; welding differs but the point cloud and bbox do not), same triangle
//!      count, scale/mirror applied identically (a mirrored mesh has flipped winding so its signed
//!      volume keeps its sign), same bounding box. The lone DELIBERATE exception is the `NORMAL`
//!      attribute on a scaled/mirrored visual: Python drops it, we keep it (re-oriented via
//!      [`normal_matrix`]) so mirrored parts shade like their twins, a shading FIX, geometry unchanged.
//!
//! ## DAE orientation: VERIFIED equivalent to the Python/trimesh authority (no `<up_axis>` rotation)
//! `mesh-loader`'s COLLADA reader does NOT act on the `<up_axis>` element (a `// TODO` in its source),
//! but NEITHER DOES `trimesh`, the loader behind the Python baker (its `exchange/dae.py` has no
//! `<up_axis>` handling at all). BOTH instead bake the COLLADA `<visual_scene>` NODE transforms into the
//! vertices, and that is where a well-formed asset's orientation actually lives: a Blender-exported Y-up
//! `.dae` carries a matching `Rx(+90deg)` node `<matrix>` (`(x,y,z)->(x,-z,y)`), a Z-up `.dae` carries
//! identity nodes. So mesh-loader and trimesh produce the SAME world-space geometry for any `up_axis`,
//! and applying an EXTRA `<up_axis>` rotation here would DOUBLE-rotate the node-oriented files and
//! DIVERGE from the authority, the opposite of correct.
//!
//! This was verified empirically against the `openarm.hcdfz` reference bundle (baked by the Python/
//! trimesh toolchain, renders correctly): [`to_glb`] of the Z-up `body_link0.dae` and the Y-up
//! `link3.dae` reproduces the reference GLBs' world-space AABBs EXACTLY with NO coordinate rotation, and
//! the result is oriented IDENTICALLY to the equivalent collision STL of the same link (a raw STL
//! passthrough). The synthetic-DAE unit tests below also confirm, against `trimesh` 4.10.1, that a Y_UP
//! vs Z_UP `.dae` with identity nodes bake IDENTICALLY (up_axis ignored), while an `Rx(+90deg)` node
//! matrix IS baked (`(1,2,3)->(1,-3,2)`, matching trimesh). See `bake_dae_matches_openarm_reference`
//! (`#[ignore]`) for the reference-AABB parity check.
//!
//! The full-Rust DAE bake therefore stays behind the `bake-dae` sub-feature only as an explicit opt-in;
//! the residual risk is mesh-loader's COLLADA *parser* coverage vs pycollada on unusual files, NOT
//! orientation. Without the feature a `.dae` input returns [`BakeError::DaeUnsupported`], a CLEAR error,
//! never a silently-wrong bake. Enable `bake-dae` on native or wasm32 to accept COLLADA input.

#![cfg(feature = "bake")]

use crate::compose::content_sha;
use glam::{Mat3, Vec3};
use std::path::{Path, PathBuf};

/// A source mesh format the baker can load (drives which `mesh-loader` parser is used and the DAE gate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SrcFormat {
    Stl,
    Obj,
    Dae,
    /// An already-canonical GLB/glTF, never converted by this module (see module docs / passthrough).
    Glb,
}

impl SrcFormat {
    /// Classify by the path's extension (case-insensitive), mirroring Python's `src_path.lower()` tests.
    fn from_path(path: &str) -> Option<Self> {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "stl" => Some(SrcFormat::Stl),
            "obj" => Some(SrcFormat::Obj),
            "dae" => Some(SrcFormat::Dae),
            "glb" | "gltf" => Some(SrcFormat::Glb),
            _ => None,
        }
    }
}

/// What can go wrong while baking. The DAE gate ([`BakeError::DaeUnsupported`]) is intentionally a
/// distinct, loud variant so a build without `bake-dae` reports the missing opt-in rather than shipping
/// a silently-wrong bake.
#[derive(Debug, thiserror::Error)]
pub enum BakeError {
    /// The source extension is not one the baker handles (STL/OBJ/DAE for conversion, GLB for visual).
    #[error("unsupported mesh format for {path:?} (baker handles .stl/.obj/.dae source and .glb/.gltf visual)")]
    UnsupportedFormat { path: String },
    /// A `.dae` input was given but the full-Rust COLLADA bake is not enabled (`bake-dae` off). When the
    /// feature IS on, the DAE bake is ORIENTATION-CORRECT for any `<up_axis>`: mesh-loader (exactly like
    /// trimesh) ignores `<up_axis>` and bakes the COLLADA `<visual_scene>` node transforms, which is where
    /// orientation lives, verified to reproduce the trimesh-baked reference world-space geometry exactly
    /// (see the module docs). The gate is a plain opt-in (a caution around mesh-loader's COLLADA parser
    /// coverage, NOT an orientation-correctness gate). Enable `bake-dae` and retry on native or wasm32.
    /// NEVER returned when `bake-dae` is on.
    #[error(
        "DAE bake for {path:?} is disabled (the full-Rust COLLADA path is opt-in behind the `bake-dae` \
         feature). Build with --features bake-dae to enable the orientation-correct full-Rust DAE bake \
         on native or wasm32."
    )]
    DaeUnsupported { path: String },
    /// A GLB/glTF was handed to a CONVERSION entry point: a visual GLB is a verbatim passthrough
    /// (`assets_vendor::bake`), not a `to_glb` re-export, and a multi-file `.gltf` is PACKED
    /// self-contained by [`crate::gltf_pack::pack_gltf_to_glb`] (which the passthrough drives), never
    /// re-baked here. (`to_lean` rejects GLB too: a GLB is not a lean collision source.)
    #[error("{path:?} is already a GLB/glTF; convert is a no-op (use the verbatim passthrough in vendor/bundle, which packs a .gltf self-contained)")]
    AlreadyGlb { path: String },
    /// `mesh-loader` failed to parse the source bytes.
    #[error("could not load mesh {path:?}: {source}")]
    Load {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The source parsed but carried no triangles (an empty mesh cannot be baked).
    #[error("mesh {path:?} has no triangles")]
    Empty { path: String },
    /// A textured-PRIMITIVE synthesis ([`bake_textured_box`]) could not produce a GLB: degenerate box
    /// extents (fewer than two positive) or a texture image that is not embeddable PNG/JPEG. Loud:
    /// the whole point of the synthesis is the texture, so there is no meaningful flat fallback HERE;
    /// the caller keeps the primitive visual (with its flat colour) and notes why.
    #[error("cannot synthesize a textured primitive: {reason}")]
    PrimitiveSynth { reason: String },
}

/// A side-channel hint texture ALREADY resolved to image bytes by the caller: the
/// [`crate::VisualAssetHint::texture`] uri (e.g. `model://pkg/materials/textures/logo.png`) looked up
/// through the caller's package-root machinery, exactly like a mesh uri (the CLI's `resolve_uri` +
/// `--package` map on disk, the browser upload map in RAM). `name` is the display/extension name (the
/// mime falls back to it after the content magic and it labels the fallback notes); `bytes` pass into
/// the GLB VERBATIM (PNG/JPEG only; anything else is a note + flat-colour fallback on a mesh bake, a
/// [`BakeError::PrimitiveSynth`] on a primitive synthesis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintTexture {
    pub bytes: Vec<u8>,
    pub name: String,
}

/// One embedded texture image: the source PNG/JPEG bytes VERBATIM (passthrough, never re-encoded) plus
/// their glTF-core mime. Shared by `Arc` so a multi-group OBJ whose materials reference one `skin.png`
/// holds the bytes once; the writer additionally dedups by CONTENT, so even two separately-resolved
/// identical images embed once.
struct TextureImage {
    bytes: Vec<u8>,
    mime: &'static str,
    /// Whether the image can render non-opaque pixels: a PNG with an alpha colour type (4/6) or a
    /// `tRNS` chunk (a JPEG never carries alpha). Precomputed once at construction from a cheap header
    /// walk so [`build_json`] can flag the bound material's `alphaMode` (a transparent logo bakes
    /// TRANSPARENT, not with the old black background). Opaque images leave it `false`, so their
    /// material stays byte-identical to the pre-alpha writer.
    has_alpha: bool,
}

/// A shared handle to one [`TextureImage`].
type TexRef = std::sync::Arc<TextureImage>;

/// Build a shared [`TextureImage`] from verbatim source bytes + their sniffed mime, precomputing
/// [`TextureImage::has_alpha`] (a PNG alpha colour type or `tRNS` chunk; a JPEG never has alpha). The
/// single construction point for every embedded texture, so the alpha probe runs exactly once per image.
fn texture_image(bytes: Vec<u8>, mime: &'static str) -> TexRef {
    let has_alpha = mime == "image/png" && png_has_alpha(&bytes);
    TexRef::new(TextureImage {
        bytes,
        mime,
        has_alpha,
    })
}

/// Whether a PNG can render non-opaque pixels: an alpha sample in the IHDR colour type (4 = grey+alpha,
/// 6 = RGBA) OR a `tRNS` transparency chunk (keyed transparency on a grey/truecolour image, or a
/// palette alpha table). A cheap, bounded header/chunk walk, no decode, no new dependency. Non-PNG
/// bytes (a JPEG, which never carries alpha) and any malformed/truncated header read as opaque: the
/// safe byte-stable default, since an image that cannot be proven to carry alpha stays OPAQUE exactly
/// as it did before this fix.
fn png_has_alpha(bytes: &[u8]) -> bool {
    const SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    // Need the 8-byte signature + IHDR through its colour-type byte (at offset 25).
    if bytes.len() < 26 || bytes[..8] != SIG {
        return false;
    }
    // IHDR is the mandatory first chunk: sig(8) + len(4) + "IHDR"(4) + width(4) + height(4) +
    // bitDepth(1) + colourType(1)…, the colour-type byte lands at offset 25.
    match bytes[25] {
        4 | 6 => return true,
        _ => {}
    }
    // Types 0/2/3 are opaque unless a `tRNS` chunk supplies transparency: walk the length-prefixed
    // chunk list (`[len:4][type:4][data:len][crc:4]`) from offset 8 until `tRNS`, `IEND`, or exhaustion.
    let mut off = 8usize;
    while off + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
        let ty = &bytes[off + 4..off + 8];
        if ty == b"tRNS" {
            return true;
        }
        if ty == b"IEND" {
            break;
        }
        // Advance past this chunk's data + 4-byte CRC; a corrupt length that would overflow ends the walk.
        off = match off
            .checked_add(12)
            .and_then(|o| o.checked_add(len as usize))
        {
            Some(o) => o,
            None => break,
        };
    }
    false
}

/// One triangle soup: a separate geometry within a baked scene (a visual GLB may hold several, a `.dae`
/// being the common case). Positions are welded (deduplicated within `WELD_EPS`); `indices` are triangle
/// vertex indices into `positions`. `color` is the baked PBR base colour for this geometry, or `None`.
struct Geom {
    /// Welded vertex positions, `[x,y,z]` each, in deterministic (sorted-dedup) order.
    positions: Vec<[f32; 3]>,
    /// Per-vertex normals, parallel to `positions` (same length), or EMPTY when the source authored none
    /// (a plain STL, whose normals the mesh-loader synthesizes). A source that AUTHORED per-vertex
    /// normals (a `.dae`, an OBJ with `vn`, a glTF) keeps them; a baked scale/mirror transforms them by
    /// the NORMAL matrix ([`normal_matrix`]) and renormalizes, so a scaled or mirrored visual keeps its
    /// authored shading (unlike trimesh, which drops normals on any scale; see the module docs).
    normals: Vec<[f32; 3]>,
    /// Texture coordinates (`TEXCOORD_0`), parallel to `positions`, ALREADY in glTF convention (V
    /// flipped from the OBJ/COLLADA bottom-left origin), or EMPTY. Carried ONLY together with `texture`
    /// (both or neither; the loaders attach them as a pair), so a texture-less bake keeps the weld and
    /// the writer byte-identical to the pre-texture output.
    uvs: Vec<[f32; 2]>,
    /// Triangle indices into `positions` (length is a multiple of 3).
    indices: Vec<u32>,
    /// Baked PBR `baseColorFactor` (`[r,g,b,a]`, 0..1), or `None` to leave the geometry material-less.
    /// For a `.dae` this is the per-mesh material colour resolved from the COLLADA shader (diffuse, with
    /// the Phong/Blinn specular fold-in); for an OBJ it is the `usemtl` group's `.mtl` diffuse `Kd`
    /// (companion-resolved, see [`bake_convert_bytes_with`]); for bare geometry it is the flat URDF
    /// colour (or `None`). A TEXTURED geometry carries WHITE (glTF multiplies factor × texture).
    color: Option<[f32; 4]>,
    /// PBR `metallicFactor` (0..1). The COLLADA common profile is a specular/dielectric model, so this is
    /// `0.0` for every resolved DAE material (and for the matte STL/OBJ default).
    metallic: f32,
    /// PBR `roughnessFactor` (0..1). `1.0` (fully matte) for Lambert / bare geometry; for Phong/Blinn it is
    /// `1 - glossiness` where glossiness is the normalized COLLADA shininess (see `shader_to_pbr`).
    roughness: f32,
    /// The embedded diffuse texture (`baseColorTexture`), or `None`. Set ONLY together with non-empty
    /// `uvs` and a `Some(WHITE)` colour (the loaders enforce the pairing; the writer emits texture JSON
    /// only when both halves are present).
    texture: Option<TexRef>,
    /// Whether the bound material renders `doubleSided` (glTF back-faces visible). `false` for every
    /// loaded mesh (STL/OBJ/DAE, whose winding is authored, and this keeps their bake byte-identical);
    /// `true` only for the synthesized textured DECAL PLANE ([`bake_textured_box`] on a zero-thickness
    /// size), a one-sided quad that would otherwise be invisible from behind (a closed synthesized box
    /// never shows its back-faces, so it stays `false`).
    double_sided: bool,
}

/// The `baseColorFactor` of a TEXTURED material: white, so the texture shows unmodulated (glTF
/// multiplies factor × texture, the convention trimesh/obj2gltf-style exporters use).
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// The matte PBR default (`metallic = 0`, `roughness = 1`): a Lambert `.dae` material, a flat-coloured
/// STL/OBJ, and any material-less geometry all use it, matching the GLB the previous writer emitted
/// (which hard-coded `metallicFactor:0, roughnessFactor:1`).
const MATTE: (f32, f32) = (0.0, 1.0);

/// The mid-grey diffuse used when a resolved material omits its diffuse colour (the COLLADA
/// common-profile default; also the `.mtl` fallback for a `Kd`-less material), so a bound material
/// never bakes to black.
const FALLBACK_DIFFUSE: [f32; 4] = [0.8, 0.8, 0.8, 1.0];

/// Normalize a shininess exponent to a 0..1 glossiness: the ONE heuristic, shared by the COLLADA
/// Phong/Blinn collapse and the OBJ `Ns` mapping: a value above 1 is treated as the common 0..128
/// Phong-exponent range (`/128`), an already-0..1 value passes through. Roughness is `1 - glossiness`.
fn shininess_glossiness(shininess: f32) -> f32 {
    if shininess > 1.0 {
        (shininess / 128.0).clamp(0.0, 1.0)
    } else {
        shininess.clamp(0.0, 1.0)
    }
}

/// Epsilon (in source units, ~meters) for welding coincident vertices. Coarse enough to merge the
/// float-printed duplicates an STL/OBJ carries, fine enough never to merge distinct geometry; only
/// affects the GLB's vertex *count* (welded vs unwelded), never the geometry; semantic parity compares
/// the vertex SET, so the choice is free.
const WELD_EPS: f32 = 1e-7;

/// Parse a `"sx sy sz"` (or single-value) scale string to a `[f32;3]`, or `None` if absent / identity,
/// mirroring `hcdf.assets._scale_vec` (a single value broadcasts to all three; an all-~1.0 scale is
/// identity → `None`, so no transform is applied).
fn scale_vec(scale: Option<&str>) -> Option<[f32; 3]> {
    let s = scale?.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<f32> = s
        .split_whitespace()
        .filter_map(|t| t.parse::<f32>().ok())
        .collect();
    let v = match parts.len() {
        1 => [parts[0]; 3],
        3 => [parts[0], parts[1], parts[2]],
        _ => return None,
    };
    if v.iter().all(|x| (x - 1.0).abs() < 1e-12) {
        None
    } else {
        Some(v)
    }
}

/// The NORMAL matrix for a position transform `m`: its inverse-transpose, so per-vertex normals stay
/// perpendicular to the (possibly non-uniformly scaled or mirrored) surface. Callers renormalize the
/// result. For a diagonal axis scale `diag(sx,sy,sz)` this is `diag(1/sx,1/sy,1/sz)`, and for a mirror
/// like `diag(-1,1,1)` it negates the mirrored component: the reflected surface's outward normal.
fn normal_matrix(m: Mat3) -> Mat3 {
    m.inverse().transpose()
}

/// Parse an `"r g b [a]"` (0..1) colour string to a PBR `baseColorFactor` (`[r,g,b,a]`, 0..1), or
/// `None`. Mirrors `hcdf.assets._color_vec` EXACTLY, including its 0..255 round-trip: Python rounds each
/// channel to a 0..255 int (`int(round(c*255))`, clamped) and trimesh stores it back as `byte/255`, so
/// e.g. `0.1 -> round(25.5)=26 -> 26/255`.
///
/// Two things must match Python bit-for-bit, since the result is quantized to an exact byte boundary:
///   1. **f64 parse.** Python parses each channel with `float(x)` (an f64). Parsing as `f32` instead can
///      nudge a value that is exactly `N.5` in f64 off the tie (e.g. the f32 nearest `0.00980392156862745`
///      times 255 is `2.50000008…`, not `2.5`), flipping the rounded byte, so we parse channels as f64.
///   2. **Round-half-to-EVEN.** Python's `round()` is banker's rounding, NOT round-half-away-from-zero,
///      so a channel landing exactly on `N.5` with `N` even rounds DOWN (`2.5 -> 2`, `0.5 -> 0`,
///      `4.5 -> 4`). Rust's `f64::round()` is round-half-AWAY (`2.5 -> 3`), which would diverge by 1/255 on
///      those boundaries, so we use `f64::round_ties_even`.
///
/// (The stored factor is the f32 `byte/255`, since the GLB material is f32; Python+trimesh store the same.)
fn color_vec(color: Option<&str>) -> Option<[f32; 4]> {
    let s = color?.trim();
    if s.is_empty() {
        return None;
    }
    let mut parts: Vec<f64> = s
        .split_whitespace()
        .filter_map(|t| t.parse::<f64>().ok())
        .collect();
    if parts.len() == 3 {
        parts.push(1.0);
    }
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0.0f32; 4];
    for (i, c) in parts.iter().enumerate() {
        // f64 product + round-half-to-even to match Python's `int(round(c*255))`, then clamp to 0..=255.
        let byte = (*c * 255.0).round_ties_even().clamp(0.0, 255.0) as i32;
        out[i] = byte as f32 / 255.0;
    }
    Some(out)
}

/// The shared knobs of one geometry load (how `scale`/`color` fold in, whether geometries stay
/// separate), bundled into one struct so the load functions stay within the 7-argument budget now that
/// the OBJ companion resolver + notes sink joined their signatures.
#[derive(Clone)]
struct LoadOpts {
    /// `[sx,sy,sz]` baked into the vertices (possibly a mirror), or `None` for identity.
    scale: Option<[f32; 3]>,
    /// The flat colour baked onto BARE geometry, or `None`.
    color: Option<[f32; 4]>,
    /// The HINT texture (`(name, image)`) applied to BARE geometry that carries texture coordinates:
    /// the importer's `<pbr><albedo_map>` side-channel, already resolved + mime-sniffed (see
    /// [`hint_texture_image`]), or `None`. Exactly the flat-`color` precedence: a mesh's OWN
    /// materials/textures always win, the hint reaches only material-less geometry; and like every
    /// texture it attaches only alongside texture coordinates (a coordinate-less mesh is a note + the
    /// flat colour). A textured geometry's `baseColorFactor` is WHITE (the module-docs convention), so
    /// when both a hint colour and a hint texture are supplied the TEXTURE wins the baseColor slot.
    texture: Option<(String, TexRef)>,
    /// Keep geometries separate (the visual scene case, trimesh `force="scene"`) vs merge into one
    /// (the lean-collision case, trimesh `force="mesh"`).
    keep_geometries: bool,
    /// Whether the flat `color` may be applied at all (visual only).
    apply_color: bool,
}

/// Resolve a caller-supplied [`HintTexture`] to the `(name, image)` pair [`LoadOpts::texture`] carries:
/// mime by content magic first, extension as the fallback: the same sniff as every companion-resolved
/// texture. A non-PNG/JPEG image is a note + `None` (the flat colour stands), mirroring
/// [`resolve_texture_image`]'s unsupported-format arm.
fn hint_texture_image(
    texture: Option<&HintTexture>,
    notes: &mut Vec<String>,
) -> Option<(String, TexRef)> {
    let t = texture?;
    match crate::gltf_pack::sniff_image_mime(&t.bytes)
        .or_else(|| crate::gltf_pack::mime_from_ext(&t.name))
    {
        Some(mime) => Some((t.name.clone(), texture_image(t.bytes.clone(), mime))),
        None => {
            push_note(
                notes,
                format!(
                    "texture {:?} not embedded (unsupported image format; only PNG/JPEG embed); \
                     flat colour used",
                    t.name
                ),
            );
            None
        }
    }
}

/// Load a source mesh into a list of [`Geom`] (one per source geometry, like trimesh `force="scene"`),
/// applying `opts.scale` (with the mirror winding-flip) and baking `opts.color` onto bare geometries.
///
/// The NATIVE (path-reading) entry: an OBJ's `mtllib`/`map_Kd` companions and a DAE's texture images
/// resolve to sibling files on disk, exactly how `assets_vendor`'s `.gltf` packing resolves its
/// `.bin`/texture companions.
fn load_geoms(
    path: &str,
    fmt: SrcFormat,
    opts: &LoadOpts,
    notes: &mut Vec<String>,
) -> Result<Vec<Geom>, BakeError> {
    let bytes = std::fs::read(path).map_err(|e| BakeError::Load {
        path: path.to_string(),
        source: e,
    })?;
    let parent = Path::new(path)
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf);
    let mut resolve = |rel: &str| std::fs::read(parent.join(rel)).ok();
    load_geoms_from_bytes(&bytes, path, fmt, opts, &mut resolve, notes)
}

/// Bytes-backed core of [`load_geoms`]: identical geometry loading + scale/colour bake, but from an
/// in-memory `bytes` slice instead of reading `name` off disk: the wasm-clean path the in-RAM importer
/// ([`bake_convert_bytes_with`]) drives (NO filesystem access of its own). `name` supplies only the
/// mesh-loader format hint and the error/`@uri` context; `resolve` supplies companion bytes by their
/// source-relative uri (the [`crate::gltf_pack::pack_gltf_to_glb`] resolver contract): an OBJ's
/// `mtllib` libraries and `map_Kd` images, a DAE's `<init_from>` texture images; `notes` collects the
/// non-fatal appearance fallbacks (see [`BakedAsset::notes`]).
fn load_geoms_from_bytes(
    bytes: &[u8],
    name: &str,
    fmt: SrcFormat,
    opts: &LoadOpts,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    notes: &mut Vec<String>,
) -> Result<Vec<Geom>, BakeError> {
    // The DAE path is handled by `dae-parser` (full per-mesh materials + node transforms), NOT
    // mesh-loader; see `load_dae_geoms_from_bytes`. STL/OBJ stay on mesh-loader unchanged. mesh-loader's
    // own COLLADA parser is NOT compiled in (the `collada` feature is off, keeping roxmltree out of the
    // tree), so the DAE match arm below re-asserts `gate_dae`'s verdict instead of reaching a loader.
    // (A `.dae` geometry is self-contained XML; the companion resolver feeds only its texture images.)
    #[cfg(feature = "bake-dae")]
    if fmt == SrcFormat::Dae {
        return load_dae_geoms_from_bytes(bytes, name, opts, resolve, notes);
    }
    // `newmtl` name -> `map_Kd` texture uri, collected from the resolved `.mtl` bytes themselves:
    // mesh-loader's own `Material.texture.diffuse` is gated on a filesystem existence probe (useless for
    // the in-RAM path and CWD-dependent on disk), so the map_Kd linkage is parsed here and the image
    // resolved through the SAME companion resolver as the `.mtl`.
    let mut mtl_textures: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let loader = mesh_loader::Loader::default().merge_meshes(!opts.keep_geometries);
    let scene = match fmt {
        SrcFormat::Stl => loader.load_stl_from_slice(bytes, name),
        SrcFormat::Obj => load_obj_scene(&loader, bytes, name, resolve, notes, &mut mtl_textures),
        // Unreachable in practice (`gate_dae` rejects a `.dae` at every entry point when `bake-dae` is
        // off; the early-return above takes it when on), returning the same gate error keeps that
        // invariant honest without a loader.
        SrcFormat::Dae => {
            return Err(BakeError::DaeUnsupported {
                path: name.to_string(),
            })
        }
        SrcFormat::Glb => {
            return Err(BakeError::AlreadyGlb {
                path: name.to_string(),
            })
        }
    }
    .map_err(|e| BakeError::Load {
        path: name.to_string(),
        source: e,
    })?;

    // The scale matrix and whether it mirrors (negative determinant -> reverse winding).
    let (mat, mirror) = match opts.scale {
        Some(s) => {
            let m = Mat3::from_cols(
                Vec3::new(s[0], 0.0, 0.0),
                Vec3::new(0.0, s[1], 0.0),
                Vec3::new(0.0, 0.0, s[2]),
            );
            (Some(m), m.determinant() < 0.0)
        }
        None => (None, false),
    };

    // Resolved texture images, cached by uri so several `usemtl` groups sharing one `skin.png` resolve
    // (and note a failure) once.
    let mut tex_cache: std::collections::BTreeMap<String, Option<TexRef>> =
        std::collections::BTreeMap::new();
    let mut geoms = Vec::with_capacity(scene.meshes.len());
    for (mesh_i, mesh) in scene.meshes.iter().enumerate() {
        if mesh.faces.is_empty() {
            continue;
        }
        // The mesh's OWN material mapped to PBR (`Scene::materials` is parallel per mesh: for an OBJ,
        // one entry per `usemtl` group; an STL / merged scene carries only defaults), or `None` for a
        // bare mesh, which alone is eligible for the flat colour, Python's `TextureVisuals` guard.
        let material = scene.materials.get(mesh_i);
        let tex_uri = material.and_then(|m| mtl_textures.get(&m.name));
        let pbr = material.and_then(|m| obj_material_pbr(m, tex_uri.is_some()));
        // The group's `map_Kd` texture, attached only when the mesh has texture coordinates AND the
        // image resolves to PNG/JPEG bytes; every miss is a note and the flat `Kd` stands (visuals only:
        // a lean collision merges materials away before this loop sees them).
        let has_uv =
            mesh.texcoords[0].len() == mesh.vertices.len() && !mesh.texcoords[0].is_empty();
        let texture = tex_uri.and_then(|uri| {
            if has_uv {
                resolve_texture_image(uri, &mut tex_cache, resolve, notes)
            } else {
                push_note(
                    notes,
                    format!("texture {uri:?} not embedded (mesh has no texture coordinates); flat colour used"),
                );
                None
            }
        });
        // The HINT texture (importer side-channel) reaches only BARE geometry: the mesh's own
        // materials/textures win, exactly the flat-colour precedence, and, like any texture, needs the
        // mesh's own texture coordinates (a plain STL has none: the standard note + flat colour).
        let texture = texture.or_else(|| hint_texture_for_bare(pbr.is_none(), opts, has_uv, notes));
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(mesh.vertices.len());
        for v in &mesh.vertices {
            let p = match mat {
                Some(m) => {
                    let t = m * Vec3::from_array(*v);
                    t.to_array()
                }
                None => *v,
            };
            positions.push(p);
        }
        // Carry per-vertex normals whenever the source AUTHORED them on the visual (GLB) path: a
        // .dae / OBJ-with-`vn` / glTF, never a plain STL (its mesh-loader normals are synthesized) and
        // never a collision (`keep_geometries` is the visual path; collisions stay POSITION-only). When
        // a scale/mirror is baked into the positions the normals are baked too, by the NORMAL matrix
        // (inverse-transpose of the scale, then renormalized), so smooth shading survives on scaled
        // AND mirrored instances alike. For an axis mirror this negates the mirrored component, which is
        // exactly the reflected surface's outward normal and stays consistent with the winding flip
        // above. This DELIBERATELY diverges from trimesh's `Scene(scene.dump())` (which discards stale
        // normals on any scale, leaving a mirrored twin POSITION-only and flat-shaded); keeping them
        // gives a mirrored link the same authored shading as its un-mirrored twin.
        let authored_normals = opts.keep_geometries
            && fmt != SrcFormat::Stl
            && mesh.normals.len() == mesh.vertices.len();
        let baked_normals: Vec<[f32; 3]> = if !authored_normals {
            Vec::new()
        } else if let Some(m) = mat {
            let nm = normal_matrix(m);
            mesh.normals
                .iter()
                .map(|n| (nm * Vec3::from_array(*n)).normalize_or_zero().to_array())
                .collect()
        } else {
            mesh.normals.clone()
        };
        let src_normals: &[[f32; 3]] = &baked_normals;
        // Texture coordinates ride along ONLY when the texture actually attaches (both or neither, the
        // pairing that keeps a texture-less bake byte-identical). V flips to the glTF top-left origin.
        // A scale/mirror bake keeps them: positions transform, the uv mapping doesn't (unlike normals).
        let src_uvs: Vec<[f32; 2]> = if texture.is_some() {
            mesh.texcoords[0]
                .iter()
                .map(|t| [t[0], 1.0 - t[1]])
                .collect()
        } else {
            Vec::new()
        };
        // Faces; reverse winding on a mirror so every triangle stays outward-facing. (The per-vertex
        // position/normal/uv arrays are untouched by the reorder, indices alone flip, so the uv stream
        // stays in sync with the winding-corrected triangles.)
        let mut indices: Vec<u32> = Vec::with_capacity(mesh.faces.len() * 3);
        for f in &mesh.faces {
            if mirror {
                indices.extend_from_slice(&[f[0], f[2], f[1]]);
            } else {
                indices.extend_from_slice(&[f[0], f[1], f[2]]);
            }
        }
        // Weld coincident vertices deterministically (first-seen order) so the GLB is compact AND stable.
        // When normals/uvs are carried they join the weld key, so a hard edge (shared position, different
        // normal) or a uv seam (shared position, different uv) keeps distinct vertices, preserving the
        // 1:1 mapping the GLB NORMAL/TEXCOORD_0 attributes require.
        let welded = weld(&positions, src_normals, &src_uvs, &indices);
        // A resolved `.mtl` material wins; else material-less + (visual only) the flat URDF colour /
        // hint texture, the same precedence arm as the DAE loader, so importer hints compose
        // identically on every format. A textured group's baseColorFactor is WHITE (factor × texture,
        // see the module docs), so a hint texture beats a hint colour for the baseColor slot.
        let (geom_color, metallic, roughness) = match pbr {
            Some((c, m, r)) => (Some(if texture.is_some() { WHITE } else { c }), m, r),
            None => {
                let flat = if opts.apply_color { opts.color } else { None };
                let base = if texture.is_some() { Some(WHITE) } else { flat };
                (base, MATTE.0, MATTE.1)
            }
        };
        geoms.push(Geom {
            positions: welded.positions,
            normals: welded.normals,
            uvs: welded.uvs,
            indices: welded.indices,
            color: geom_color,
            metallic,
            roughness,
            texture,
            // A loaded mesh keeps the source's authored winding, never forced double-sided (byte-identity).
            double_sided: false,
        });
    }
    if geoms.is_empty() {
        return Err(BakeError::Empty {
            path: name.to_string(),
        });
    }
    Ok(geoms)
}

/// Load an OBJ scene, resolving its `mtllib` companions through `resolve` (an OBJ-relative uri →
/// bytes, the same contract as [`crate::gltf_pack::pack_gltf_to_glb`]'s resolver). mesh-loader already
/// splits the face soup into one `Mesh` per `usemtl` group (with `Scene::materials` parallel per
/// mesh); the resolver is what lets those groups bind to real `.mtl` materials without filesystem
/// access. An unresolved library is a NOTE, never an error; its groups simply bake material-less
/// (gray, the pre-resolver behaviour, now explicit). Each resolved library's `newmtl` → `map_Kd`
/// pairs land in `mtl_textures` so the caller can embed the diffuse textures (mesh-loader's own
/// texture field is filesystem-probed and unusable in-RAM; see [`load_geoms_from_bytes`]).
fn load_obj_scene(
    loader: &mesh_loader::Loader,
    bytes: &[u8],
    name: &str,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    notes: &mut Vec<String>,
    mtl_textures: &mut std::collections::BTreeMap<String, String>,
) -> std::io::Result<mesh_loader::Scene> {
    // Hand the loader a BARE file name: it joins each `mtllib` entry onto the OBJ's parent directory,
    // and a bare name's parent is empty, so the reader callback sees the entry exactly as authored
    // (OBJ-relative), the uri shape the resolver contract wants. (With a full path the callback would
    // receive absolute paths and the resolver would need to un-join them.)
    let bare = Path::new(name)
        .file_name()
        .map_or_else(|| PathBuf::from(name), PathBuf::from);
    loader.load_obj_from_slice_with_reader(bytes, &bare, |mtl: &Path| {
        let rel = mtl.to_string_lossy();
        match resolve(&rel) {
            Some(data) => {
                collect_mtl_textures(&data, mtl_textures);
                Ok(data)
            }
            None => {
                push_note(
                    notes,
                    format!("material library {rel:?} not resolved; its geometry bakes material-less (gray)"),
                );
                // mesh-loader treats a reader error as "no material library" and parses on; the
                // note above is what keeps the fallback visible.
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "unresolved mtllib companion"))
            }
        }
    })
}

/// Push `msg` onto `notes` unless an identical note is already there (an OBJ may repeat `mtllib`
/// lines, and several materials commonly share one texture; one note per distinct fact).
fn push_note(notes: &mut Vec<String>, msg: String) {
    if !notes.contains(&msg) {
        notes.push(msg);
    }
}

/// Collect every `newmtl <name>` → `map_Kd <uri>` pair from one resolved `.mtl` (later libraries /
/// re-declarations overwrite, matching the usual last-wins MTL semantics). The uri is recorded exactly
/// as authored; it resolves through the SAME companion resolver as the `.mtl` itself.
fn collect_mtl_textures(mtl: &[u8], out: &mut std::collections::BTreeMap<String, String>) {
    let mut current: Option<String> = None;
    for line in String::from_utf8_lossy(mtl).lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("newmtl") {
            if rest.starts_with([' ', '\t']) && !rest.trim().is_empty() {
                current = Some(rest.trim().to_string());
            }
        } else if let Some(rest) = line.strip_prefix("map_Kd") {
            let tex = rest.trim();
            if rest.starts_with([' ', '\t']) && !tex.is_empty() {
                if let Some(name) = &current {
                    out.insert(name.clone(), tex.to_string());
                }
            }
        }
    }
}

/// Resolve one texture uri to an embeddable [`TexRef`] through the companion resolver, cached by uri
/// (one resolution, and at most one note, per distinct image). `None` (with a note saying WHY) when
/// the companion is missing or its bytes are not PNG/JPEG (magic-sniffed first, extension as the
/// fallback for a mime-less container the caller still wants passed through verbatim): the affected
/// material keeps its flat diffuse: a visible fallback, never an error.
fn resolve_texture_image(
    uri: &str,
    cache: &mut std::collections::BTreeMap<String, Option<TexRef>>,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    notes: &mut Vec<String>,
) -> Option<TexRef> {
    if let Some(hit) = cache.get(uri) {
        return hit.clone();
    }
    let resolved = match resolve(uri) {
        None => {
            push_note(
                notes,
                format!("texture {uri:?} not embedded (image not resolved); flat colour used"),
            );
            None
        }
        Some(bytes) => {
            match crate::gltf_pack::sniff_image_mime(&bytes)
                .or_else(|| crate::gltf_pack::mime_from_ext(uri))
            {
                Some(mime) => Some(texture_image(bytes, mime)),
                None => {
                    push_note(
                        notes,
                        format!(
                            "texture {uri:?} not embedded (unsupported image format; only PNG/JPEG \
                             embed); flat colour used"
                        ),
                    );
                    None
                }
            }
        }
    };
    cache.insert(uri.to_string(), resolved.clone());
    resolved
}

/// The hint texture of one BARE (material-less) geometry, or `None`: the load must carry one
/// ([`LoadOpts::texture`]), the geometry must actually BE bare (`bare`; a mesh's own material wins,
/// the flat-colour precedence), and the geometry must have texture coordinates (a coordinate-less
/// mesh, a plain STL) is the standard not-embedded note + the flat colour. Shared by the mesh-loader
/// and DAE paths so the hint composes identically on every format.
fn hint_texture_for_bare(
    bare: bool,
    opts: &LoadOpts,
    has_uv: bool,
    notes: &mut Vec<String>,
) -> Option<TexRef> {
    if !bare {
        return None;
    }
    let (name, img) = opts.texture.as_ref()?;
    if has_uv {
        Some(img.clone())
    } else {
        push_note(
            notes,
            format!(
                "texture {name:?} not embedded (mesh has no texture coordinates); flat colour used"
            ),
        );
        None
    }
}

/// Map one `.mtl` material (as mesh-loader parsed it) to `(baseColorFactor, metallic, roughness)`, or
/// `None` for a BARE mesh (a default `Material`: no `usemtl`, an unresolved `mtllib`, an STL). "Has a
/// material" is any authored colour or diffuse texture, `has_mtl_texture` being the `map_Kd` linkage
/// parsed from the resolved `.mtl` bytes themselves ([`collect_mtl_textures`]; mesh-loader's own
/// `texture.diffuse` is filesystem-probed, so it backs this up only on disk). The mapping mirrors the
/// DAE shader collapse where MTL overlaps it: `baseColor` = `Kd` (diffuse, mid-grey fallback so e.g. a
/// texture-only material keeps its shading), `metallic` = 0 (MTL is a dielectric model), `roughness` =
/// `1 - glossiness` with glossiness the normalized `Ns` ([`shininess_glossiness`]; no `Ns` → fully
/// matte). `Ke` emissive is NOT carried (no emissiveFactor in the writer's schema).
fn obj_material_pbr(
    m: &mesh_loader::Material,
    has_mtl_texture: bool,
) -> Option<([f32; 4], f32, f32)> {
    let has_material = m.color.diffuse.is_some()
        || m.color.ambient.is_some()
        || m.color.specular.is_some()
        || m.color.emissive.is_some()
        || m.texture.diffuse.is_some()
        || has_mtl_texture;
    if !has_material {
        return None;
    }
    let base = m.color.diffuse.unwrap_or(FALLBACK_DIFFUSE);
    let roughness = 1.0 - m.shininess.map_or(0.0, shininess_glossiness);
    Some((base, MATTE.0, roughness))
}

// ── full-Rust COLLADA (.dae) loader: feature `bake-dae`, driven by `dae-parser` ──────────────────────
//
// ## DAE materials: FAITHFUL per-mesh appearance (the reason this path is NOT mesh-loader)
// `mesh-loader` MERGES every COLLADA sub-mesh into one and never populates a per-mesh material on the
// DAE path, so a `mesh-loader`-baked `.dae` came out MATERIAL-LESS (gray). `dae-parser` exposes the full
// binding chain, so each triangle-set keeps its own material:
//
//   triangle-set `material` SYMBOL
//     -> the instancing node's `<bind_material>` `InstanceMaterial` with that symbol
//        -> its `target` `<material>`  -> the material's `<instance_effect>` `<effect>`
//           -> the effect's `profile_COMMON` `<technique>` shader (Phong / Lambert / Blinn / Constant)
//
// The shader is mapped to a glTF PBR material by `shader_to_pbr` (the canonical
// KHR_materials_pbrSpecularGlossiness collapse): `baseColor.rgb = diffuse.rgb * (1 - max(specular.rgb))`,
// `baseColor.a = diffuse.a`, `metallic = 0` (the COLLADA common profile is a specular/dielectric model),
// `roughness = 1 - glossiness`. Glossiness is the normalized COLLADA shininess, the ONE heuristic:
// `glossiness = shininess > 1 ? (shininess/128).clamp(0,1) : shininess.clamp(0,1)` (a Phong exponent is
// commonly 0..128, occasionally already 0..1). LAMBERT (no specular/shininess) is `baseColor = diffuse`,
// `metallic = 0`, `roughness = 1`: fully matte, which is the OpenArm case and is CORRECT. A triangle-set
// with no bound material is left material-less (and the flat URDF colour may apply, as for an STL).
//
// A diffuse that is a `<texture texture="…">` reference resolves through the effect's param chain
// (`sampler2D` → `surface` → `<library_images>` `<init_from>`, tolerating the common exporter shortcut
// where the texture names the image id directly); the image file resolves through the SAME companion
// resolver as an OBJ's `.mtl`, and the triangle-set's TEXCOORD input rides along as `TEXCOORD_0` (see
// the module-level "Textures" section). The textured material's baseColorFactor is WHITE: glTF
// multiplies factor × texture, the convention glTF exporters (trimesh included) use. NOTE: textured
// DAEs are OUTSIDE the pinned Python/trimesh oracle corpus (the OpenArm references are Lambert
// flat-colour files), so this arm mirrors exporter convention rather than a byte-pinned oracle; every
// non-embeddable texture is a note + flat-diffuse fallback, and a texture-less DAE bakes byte-identically
// to the pre-texture writer.
//
// ## DAE orientation: reproduces the mesh-loader / trimesh world geometry (no `<up_axis>` rotation)
// `<up_axis>` is IGNORED (exactly like mesh-loader and trimesh); orientation lives in the
// `<visual_scene>` NODE transforms, which are accumulated from the root and baked into the vertices here
// (see `node_local_matrix`). The URDF mesh `scale` is then folded in (mirror reverses winding), and
// authored normals are carried on the visual path, rotated by the node's linear part, and, when a
// scale/mirror is folded in, additionally by that scale's NORMAL matrix ([`normal_matrix`]) so they stay
// perpendicular to the scaled surface (diverging from trimesh, which drops normals on any scale). This
// makes the baked world-space AABBs identical to the mesh-loader path (`bake_dae_matches_openarm_reference`).
//
// ## COLLADA 1.5: read via a downgrade pre-pass
// `dae-parser` implements ONLY 1.4.1 and hard-rejects any other root `version` ("Unsupported COLLADA
// version"; the namespace is never checked on the read path). Real exporters (e.g. Cinema 4D, the
// Unitree H2 meshes) emit 1.5.0, whose geometry/material/scene element set is unchanged from 1.4, so
// `downgrade_collada_15` splices the root tag's namespace + version down to 1.4.1 before parsing (with a
// conversion note). A 1.5-only construct the 1.4 reader cannot place (none in the real-exporter meshes
// seen so far) surfaces as `dae-parser`'s ordinary load error, never a silently-wrong bake.

/// One loaded COLLADA vertex: position plus optional authored normal and texture coordinate. Built by
/// the `dae-parser` geometry importer (which resolves the VERTEX/NORMAL/TEXCOORD inputs of each
/// triangle-set) via [`VertexLoad`].
#[cfg(feature = "bake-dae")]
#[derive(Clone)]
struct DaeVertex {
    pos: [f32; 3],
    nrm: Option<[f32; 3]>,
    /// Raw COLLADA `(S,T)`, still bottom-left origin; the V flip to glTF convention happens where the
    /// uvs attach to a textured [`Geom`].
    uv: Option<[f32; 2]>,
}

#[cfg(feature = "bake-dae")]
impl<'a> dae_parser::geom::VertexLoad<'a> for DaeVertex {
    fn position(
        _ctx: &(),
        reader: &dae_parser::source::SourceReader<'a, dae_parser::source::XYZ>,
        index: u32,
    ) -> Self {
        DaeVertex {
            pos: reader.get(index as usize),
            nrm: None,
            uv: None,
        }
    }
    fn add_normal(
        &mut self,
        _ctx: &(),
        reader: &dae_parser::source::SourceReader<'a, dae_parser::source::XYZ>,
        index: u32,
    ) {
        self.nrm = Some(reader.get(index as usize));
    }
    fn add_texcoord(
        &mut self,
        _ctx: &(),
        reader: &dae_parser::source::SourceReader<'a, dae_parser::source::ST>,
        index: u32,
        _set: Option<u32>,
    ) {
        // First TEXCOORD input wins (set 0 in every real exporter's ordering); additional uv sets have
        // no glTF slot in this writer (baseColorTexture samples TEXCOORD_0).
        if self.uv.is_none() {
            self.uv = Some(reader.get(index as usize));
        }
    }
}

/// Compose a node's `<transform>` stack into a single 4x4 (in document order, left-to-right). A COLLADA
/// `<matrix>` is ROW-major, so it is loaded column-major and transposed to recover `M` (`world = M * v`);
/// the other transforms map directly. `LookAt`/`Skew` do not occur in robot visual meshes and are treated
/// as identity. Computed with [`glam`] (the crate's `nalgebra` feature is intentionally NOT enabled).
#[cfg(feature = "bake-dae")]
fn node_local_matrix(node: &dae_parser::Node) -> glam::Mat4 {
    use dae_parser::Transform;
    use glam::Mat4;
    let mut m = Mat4::IDENTITY;
    for t in &node.transforms {
        let tm = match t {
            Transform::Matrix(mat) => Mat4::from_cols_array(&mat.0).transpose(),
            Transform::Translate(tr) => Mat4::from_translation(Vec3::from_array(*tr.0)),
            Transform::Scale(sc) => Mat4::from_scale(Vec3::from_array(*sc.0)),
            Transform::Rotate(r) => {
                let axis = Vec3::from_array(*r.axis());
                if axis.length_squared() > 0.0 {
                    Mat4::from_axis_angle(axis.normalize(), r.angle().to_radians())
                } else {
                    Mat4::IDENTITY
                }
            }
            Transform::LookAt(_) | Transform::Skew(_) => Mat4::IDENTITY,
        };
        m *= tm;
    }
    m
}

/// Extract a literal RGBA from a COLLADA shader colour parameter (a `<color>`; a texture/param reference
/// yields `None`, since the baker carries flat colours, not textures).
#[cfg(feature = "bake-dae")]
fn color_param_rgba(c: &Option<dae_parser::WithSid<dae_parser::ColorParam>>) -> Option<[f32; 4]> {
    c.as_ref().and_then(|w| w.as_color().copied())
}

/// Map a COLLADA common-profile shader to a glTF PBR `(baseColorFactor, metallicFactor, roughnessFactor)`;
/// see the module-section docs for the Phong->PBR collapse and the shininess normalization.
#[cfg(feature = "bake-dae")]
fn shader_to_pbr(shader: &dae_parser::Shader) -> ([f32; 4], f32, f32) {
    use dae_parser::Shader;
    match shader {
        // Lambert is matte: baseColor = diffuse, metallic 0, roughness 1 (the OpenArm case, correct).
        Shader::Lambert(l) => (
            color_param_rgba(&l.diffuse).unwrap_or(FALLBACK_DIFFUSE),
            MATTE.0,
            MATTE.1,
        ),
        // Constant is unlit: treat emission as the base colour, matte.
        Shader::Constant(c) => (
            color_param_rgba(&c.emission).unwrap_or(FALLBACK_DIFFUSE),
            MATTE.0,
            MATTE.1,
        ),
        Shader::Phong(p) => phong_blinn_to_pbr(&p.diffuse, &p.specular, &p.shininess),
        Shader::Blinn(b) => phong_blinn_to_pbr(&b.diffuse, &b.specular, &b.shininess),
    }
}

/// The Phong/Blinn -> PBR collapse shared by both specular shaders (see the module-section docs).
#[cfg(feature = "bake-dae")]
fn phong_blinn_to_pbr(
    diffuse: &Option<dae_parser::WithSid<dae_parser::ColorParam>>,
    specular: &Option<dae_parser::WithSid<dae_parser::ColorParam>>,
    shininess: &Option<dae_parser::WithSid<dae_parser::FloatParam>>,
) -> ([f32; 4], f32, f32) {
    use dae_parser::FloatParam;
    let d = color_param_rgba(diffuse).unwrap_or(FALLBACK_DIFFUSE);
    let s = color_param_rgba(specular).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    // baseColor.rgb = diffuse.rgb * (1 - max(specular.rgb)); baseColor.a = diffuse.a.
    let smax = s[0].max(s[1]).max(s[2]).clamp(0.0, 1.0);
    let base = [
        d[0] * (1.0 - smax),
        d[1] * (1.0 - smax),
        d[2] * (1.0 - smax),
        d[3],
    ];
    // glossiness from the normalized COLLADA shininess ([`shininess_glossiness`]); roughness = 1 - it.
    let glossiness = match shininess.as_ref().map(|w| &**w) {
        Some(FloatParam::Float(v)) => shininess_glossiness(*v),
        _ => 0.0,
    };
    (base, 0.0, 1.0 - glossiness)
}

/// The diffuse-texture disposition of one resolved DAE material.
#[cfg(feature = "bake-dae")]
enum DaeTex {
    /// The diffuse is a flat colour, no texture referenced.
    None,
    /// The diffuse references a texture that could not be embedded (a note was already pushed); the
    /// flat fallback diffuse stands.
    Unavailable,
    /// The diffuse texture resolved to embeddable image bytes; the `String` is its display name for
    /// the "mesh has no texture coordinates" note the geometry site may still need.
    Image(String, TexRef),
}

/// One DAE material resolved to PBR: the flat collapse (`shader_to_pbr`) plus the diffuse-texture
/// disposition.
#[cfg(feature = "bake-dae")]
struct DaeMat {
    base: [f32; 4],
    metallic: f32,
    roughness: f32,
    texture: DaeTex,
}

/// The shared lookup state of one DAE material resolution: the binding-chain maps, the resolved-image
/// cache, and whether textures embed at all (`false` for a lean collision bake, which never touches
/// the resolver, since a lean shape carries no appearance).
#[cfg(feature = "bake-dae")]
struct DaeMatCtx<'a> {
    material_map: dae_parser::LocalMap<'a, dae_parser::Material>,
    effect_map: dae_parser::LocalMap<'a, dae_parser::Effect>,
    image_map: dae_parser::LocalMap<'a, dae_parser::Image>,
    cache: std::collections::BTreeMap<String, Option<TexRef>>,
    want_textures: bool,
}

/// The `<texture>` reference of a shader's diffuse, if any (Constant has no diffuse slot).
#[cfg(feature = "bake-dae")]
fn shader_diffuse_texture(shader: &dae_parser::Shader) -> Option<&dae_parser::Texture> {
    use dae_parser::Shader;
    let diffuse = match shader {
        Shader::Lambert(l) => &l.diffuse,
        Shader::Phong(p) => &p.diffuse,
        Shader::Blinn(b) => &b.diffuse,
        Shader::Constant(_) => return None,
    };
    diffuse.as_ref().and_then(|w| w.as_texture())
}

/// Find an effect-scope `<newparam>` by sid: technique-level params shadow profile-level, which
/// shadow effect-level (nearest scope wins, the COLLADA parameter-scoping rule).
#[cfg(feature = "bake-dae")]
fn find_effect_param<'a>(
    effect: &'a dae_parser::Effect,
    profile: &'a dae_parser::ProfileCommon,
    sid: &str,
) -> Option<&'a dae_parser::ParamType> {
    use dae_parser::ImageParam;
    let technique = profile
        .technique
        .data
        .image_param
        .iter()
        .filter_map(|ip| match ip {
            ImageParam::NewParam(p) => Some(p),
            ImageParam::Image(_) => None,
        });
    technique
        .chain(profile.new_param.iter())
        .chain(effect.new_param.iter())
        .find(|p| p.sid == sid)
        .map(|p| &p.ty)
}

#[cfg(feature = "bake-dae")]
impl<'a> DaeMatCtx<'a> {
    /// Find a COLLADA `<image>` by id: `<library_images>` first, then the images declared inline in
    /// the technique / profile / effect (all legal 1.4 homes for an image).
    fn find_image(
        &self,
        effect: &'a dae_parser::Effect,
        profile: &'a dae_parser::ProfileCommon,
        id: &str,
    ) -> Option<&'a dae_parser::Image> {
        use dae_parser::ImageParam;
        if let Some(img) = self.image_map.get_str(id) {
            return Some(img);
        }
        let technique = profile
            .technique
            .data
            .image_param
            .iter()
            .filter_map(|ip| match ip {
                ImageParam::Image(i) => Some(i),
                ImageParam::NewParam(_) => None,
            });
        technique
            .chain(profile.image.iter())
            .chain(effect.image.iter())
            .find(|i| i.id.as_deref() == Some(id))
    }

    /// Walk the canonical sampler chain: texture sid → `sampler2D` param → its `surface` param →
    /// `<init_from>` image id → the image element.
    fn sampler_chain_image(
        &self,
        effect: &'a dae_parser::Effect,
        profile: &'a dae_parser::ProfileCommon,
        sid: &str,
    ) -> Option<&'a dae_parser::Image> {
        let sampler = find_effect_param(effect, profile, sid)?.as_sampler2d()?;
        let surface = find_effect_param(effect, profile, &sampler.source)?.as_surface()?;
        let dae_parser::SurfaceInit::From { image, .. } = &surface.init else {
            return None;
        };
        self.find_image(effect, profile, image)
    }

    /// Resolve a shader's diffuse `<texture>` to its disposition: the sampler chain (or the common
    /// exporter shortcut where `texture` names the image id directly) down to embeddable PNG/JPEG
    /// bytes: embedded `<data>` hex sniffed by magic, an `<init_from>` uri through the SAME companion
    /// `resolve` as an OBJ's textures. Every non-embeddable outcome is a note + [`DaeTex::Unavailable`].
    fn resolve_texture(
        &mut self,
        effect: &'a dae_parser::Effect,
        profile: &'a dae_parser::ProfileCommon,
        tex: &dae_parser::Texture,
        resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
        notes: &mut Vec<String>,
    ) -> DaeTex {
        let sid: &str = &tex.texture;
        let image = self
            .sampler_chain_image(effect, profile, sid)
            .or_else(|| self.find_image(effect, profile, sid));
        let Some(image) = image else {
            push_note(
                notes,
                format!(
                    "texture {sid:?} not embedded (unresolved sampler/surface/image chain); \
                     flat colour used"
                ),
            );
            return DaeTex::Unavailable;
        };
        match &image.source {
            // Embedded <data> hex: the bytes are already in the document; magic decides the mime
            // (there is no uri to fall back on). Cached under "#<id>" (a fragment can never collide
            // with a companion uri).
            dae_parser::ImageSource::Data(bytes) => {
                let key = format!("#{}", image.id.as_deref().unwrap_or(sid));
                if let Some(hit) = self.cache.get(&key) {
                    return dae_tex_of(&key, hit.clone());
                }
                let resolved = match crate::gltf_pack::sniff_image_mime(bytes) {
                    Some(mime) => Some(texture_image(bytes.to_vec(), mime)),
                    None => {
                        push_note(
                            notes,
                            format!(
                                "texture {key:?} not embedded (unsupported image format; only \
                                 PNG/JPEG embed); flat colour used"
                            ),
                        );
                        None
                    }
                };
                self.cache.insert(key.clone(), resolved.clone());
                dae_tex_of(&key, resolved)
            }
            dae_parser::ImageSource::InitFrom(url) => {
                let uri = dae_image_uri(url);
                dae_tex_of(
                    &uri,
                    resolve_texture_image(&uri, &mut self.cache, resolve, notes),
                )
            }
        }
    }
}

/// Wrap a resolved-or-not image into its [`DaeTex`] disposition.
#[cfg(feature = "bake-dae")]
fn dae_tex_of(name: &str, resolved: Option<TexRef>) -> DaeTex {
    match resolved {
        Some(img) => DaeTex::Image(name.to_string(), img),
        None => DaeTex::Unavailable,
    }
}

/// The companion-resolver uri of a COLLADA `<init_from>` url: percent-decoded, `file://` and leading
/// `./` stripped: the uri shape an exporter-laid-out sibling texture actually has on disk / in the
/// upload map.
#[cfg(feature = "bake-dae")]
fn dae_image_uri(url: &dae_parser::Url) -> String {
    match url {
        // A fragment as an image SOURCE is malformed; keep it visible as-is (it will not resolve).
        dae_parser::Url::Fragment(s) => format!("#{s}"),
        dae_parser::Url::Other(s) => {
            let s = crate::gltf_pack::percent_decode(s);
            let s = s.strip_prefix("file://").unwrap_or(&s);
            s.strip_prefix("./").unwrap_or(s).to_string()
        }
    }
}

/// Resolve a triangle-set's material symbol through the instancing node's binding to a PBR material
/// (+ diffuse-texture disposition), or `None` if it is unbound / unresolved (leaving the geometry
/// material-less). If the set names no symbol, fall back to the single bound material (the common
/// one-material-mesh case).
#[cfg(feature = "bake-dae")]
fn dae_material_pbr(
    symbol: Option<&str>,
    inst: &dae_parser::Instance<dae_parser::Geometry>,
    ctx: &mut DaeMatCtx<'_>,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    notes: &mut Vec<String>,
) -> Option<DaeMat> {
    let im = match symbol {
        Some(sym) => inst.get_instance_material(sym)?,
        None => inst.instance_materials().first()?,
    };
    let material = ctx.material_map.get(&im.target)?;
    let effect = ctx.effect_map.get(&material.instance_effect.url)?;
    let profile = effect.get_common_profile()?;
    let shader = profile.technique.data.shaders.first()?;
    let (base, metallic, roughness) = shader_to_pbr(shader);
    let texture = match shader_diffuse_texture(shader) {
        Some(tex) if ctx.want_textures => ctx.resolve_texture(effect, profile, tex, resolve, notes),
        _ => DaeTex::None,
    };
    Some(DaeMat {
        base,
        metallic,
        roughness,
        texture,
    })
}

/// Build the per-triangle-set vertex importer for a primitive (resolves its VERTEX/NORMAL `<source>`s).
#[cfg(feature = "bake-dae")]
fn build_importer<'a, T>(
    g: &'a dae_parser::Geom<T>,
    source_map: &dae_parser::LocalMap<'a, dae_parser::Source>,
    vimp: &dae_parser::geom::VertexImporter<'a>,
) -> Result<dae_parser::geom::Importer<'a>, String> {
    g.importer(source_map, vimp.clone())
        .map_err(|()| "unresolved <source> referenced by a mesh primitive".to_string())
}

/// Expand one COLLADA mesh primitive into a flat, already-triangulated vertex soup (every 3 consecutive
/// vertices form a triangle) plus the triangle-set's material symbol. Triangulation is fan-based for
/// polygons / polylists / triangle-fans, and strip-based for triangle-strips; line primitives yield no
/// surface (empty soup). Returns an error only if a primitive's vertex `<source>` cannot be resolved.
#[cfg(feature = "bake-dae")]
fn triangulate_primitive<'a>(
    prim: &'a dae_parser::Primitive,
    vimp: &dae_parser::geom::VertexImporter<'a>,
    source_map: &dae_parser::LocalMap<'a, dae_parser::Source>,
) -> Result<(Vec<DaeVertex>, Option<&'a str>), String> {
    use dae_parser::Primitive;
    let read = |imp: &dae_parser::geom::Importer<'a>, arr: &'a [u32]| -> Vec<DaeVertex> {
        imp.read::<(), DaeVertex>(&(), arr).collect()
    };
    // Triangulate a polygon (fan) given its vertex list, appending to `soup`.
    let fan = |soup: &mut Vec<DaeVertex>, vs: &[DaeVertex]| {
        for i in 1..vs.len().saturating_sub(1) {
            soup.push(vs[0].clone());
            soup.push(vs[i].clone());
            soup.push(vs[i + 1].clone());
        }
    };
    match prim {
        Primitive::Triangles(g) => {
            let imp = build_importer(g, source_map, vimp)?;
            let soup = match &g.data.prim {
                Some(arr) => read(&imp, arr),
                None => Vec::new(),
            };
            Ok((soup, g.material.as_deref()))
        }
        Primitive::PolyList(g) => {
            let imp = build_importer(g, source_map, vimp)?;
            let vs = read(&imp, &g.data.prim);
            let mut soup = Vec::new();
            let mut idx = 0usize;
            for &n in g.data.vcount.iter() {
                let n = n as usize;
                if n >= 3 && idx + n <= vs.len() {
                    fan(&mut soup, &vs[idx..idx + n]);
                }
                idx += n;
            }
            Ok((soup, g.material.as_deref()))
        }
        Primitive::Polygons(g) => {
            let imp = build_importer(g, source_map, vimp)?;
            let mut soup = Vec::new();
            for ph in &g.data.0 {
                let vs = read(&imp, &ph.verts);
                fan(&mut soup, &vs);
            }
            Ok((soup, g.material.as_deref()))
        }
        Primitive::TriFans(g) => {
            let imp = build_importer(g, source_map, vimp)?;
            let mut soup = Vec::new();
            for buf in &g.data.prim {
                let vs = read(&imp, buf);
                fan(&mut soup, &vs);
            }
            Ok((soup, g.material.as_deref()))
        }
        Primitive::TriStrips(g) => {
            let imp = build_importer(g, source_map, vimp)?;
            let mut soup = Vec::new();
            for buf in &g.data.prim {
                let vs = read(&imp, buf);
                for i in 0..vs.len().saturating_sub(2) {
                    // Alternate winding each step so every triangle keeps a consistent orientation.
                    let (a, b, c) = if i % 2 == 0 {
                        (i, i + 1, i + 2)
                    } else {
                        (i + 1, i, i + 2)
                    };
                    soup.push(vs[a].clone());
                    soup.push(vs[b].clone());
                    soup.push(vs[c].clone());
                }
            }
            Ok((soup, g.material.as_deref()))
        }
        // Line primitives carry no surface, nothing to bake.
        Primitive::Lines(g) => Ok((Vec::new(), g.material.as_deref())),
        Primitive::LineStrips(g) => Ok((Vec::new(), g.material.as_deref())),
    }
}

/// The COLLADA 1.5 root namespace. A 1.5 document is downgraded to 1.4.1 before `dae-parser` (which
/// implements only 1.4.1 and hard-rejects on the `version` attribute); see [`downgrade_collada_15`].
#[cfg(feature = "bake-dae")]
const COLLADA_15_NS: &[u8] = b"http://www.collada.org/2008/03/COLLADASchema";
/// The COLLADA 1.4 root namespace `dae-parser` documents (same byte length as [`COLLADA_15_NS`]).
#[cfg(feature = "bake-dae")]
const COLLADA_14_NS: &[u8] = b"http://www.collada.org/2005/11/COLLADASchema";

/// Downgrade pre-pass for COLLADA 1.5 (the lenient-URDF pre-pass pattern): when the root `<COLLADA>`
/// carries the 2008/03 namespace and/or `version="1.5.*"`, splice the root START TAG ONLY: namespace
/// value -> 2005/11, `version` value -> `"1.4.1"`, so `dae-parser` (a strict 1.4.1 parser that keys
/// SOLELY on the `version` attribute) accepts it, and push one conversion note. Every byte outside the
/// edited attribute-value spans is untouched (no reformatting). Sound because the 1.4 element set this
/// loader reads (geometry/effect/material/visual-scene) is unchanged in 1.5; a 1.5-only construct the
/// 1.4 reader cannot place becomes `dae-parser`'s ordinary load error. A 1.4 document (or anything
/// unscannable, which `dae-parser` then rejects with its own error) passes through zero-copy and
/// byte-identical.
#[cfg(feature = "bake-dae")]
fn downgrade_collada_15<'a>(
    bytes: &'a [u8],
    notes: &mut Vec<String>,
) -> std::borrow::Cow<'a, [u8]> {
    use quick_xml::events::Event;
    use std::borrow::Cow;
    let pass = Cow::Borrowed(bytes);
    // Find the root start tag (skipping the decl/comments/doctype). `Reader<&[u8]>` events borrow from
    // `bytes`, so an attribute's raw value slice sits INSIDE `bytes`; its offset is the splice span.
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let root = loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => break e,
            Ok(Event::Eof) | Err(_) => return pass,
            Ok(_) => {}
        }
    };
    if root.local_name().as_ref() != b"COLLADA" {
        return pass;
    }
    // The value span of an attribute borrowed from `bytes` (raw, so always `Cow::Borrowed`), guarded so
    // a non-borrowed value can never splice at a bogus offset.
    let span_in_bytes = |v: &Cow<'_, [u8]>| -> Option<(usize, usize)> {
        let Cow::Borrowed(raw) = v else { return None };
        let off = (raw.as_ptr() as usize).checked_sub(bytes.as_ptr() as usize)?;
        (off + raw.len() <= bytes.len()).then_some((off, off + raw.len()))
    };
    // Schedule the splices: every namespace-declaration value equal to the 2008/03 URI -> 2005/11, and a
    // `version` starting `1.5` -> `1.4.1`. (`xsi:schemaLocation` mentions the URI inside a larger value,
    // so exact equality keeps it untouched.) Spans come from quick-xml's own parse, so arbitrary attribute
    // order/whitespace/quoting is handled, and edits arrive left-to-right in tag order.
    let mut edits: Vec<(usize, usize, &[u8])> = Vec::new();
    for attr in root.attributes().flatten() {
        let replacement: &[u8] = if attr.value.as_ref() == COLLADA_15_NS {
            COLLADA_14_NS
        } else if attr.key.as_ref() == b"version" && attr.value.starts_with(b"1.5") {
            b"1.4.1"
        } else {
            continue;
        };
        let Some((start, end)) = span_in_bytes(&attr.value) else {
            return pass;
        };
        edits.push((start, end, replacement));
    }
    if edits.is_empty() {
        return pass;
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut cursor = 0usize;
    for (start, end, replacement) in edits {
        out.extend_from_slice(&bytes[cursor..start]);
        out.extend_from_slice(replacement);
        cursor = end;
    }
    out.extend_from_slice(&bytes[cursor..]);
    notes.push(
        "COLLADA 1.5 downgraded to 1.4.1 for import; 1.5-only constructs ignored".to_string(),
    );
    Cow::Owned(out)
}

/// Full-Rust COLLADA loader (feature `bake-dae`): parse with `dae-parser`, walk the `<visual_scene>` node
/// tree accumulating transforms, and emit one [`Geom`] per (triangle-set, material) with the node
/// transform + URDF `scale` baked into the vertices and the resolved per-mesh PBR material attached. The
/// `dae-parser` counterpart of the mesh-loader branch of [`load_geoms_from_bytes`], preserving all of its
/// behaviour: scale baked into verts, mirror (`det < 0`) winding reversal, authored normals carried on the
/// visual path (transformed by the scale's NORMAL matrix so scaled/mirrored bakes stay smooth-shaded),
/// and the flat URDF colour applied only to material-less geometry.
/// `resolve` supplies texture-image companion bytes by uri (a diffuse `<texture>`'s `<init_from>` file;
/// the geometry itself is self-contained XML and never touches it). A COLLADA 1.5 source is first
/// downgraded to 1.4.1 (`notes` records it; see [`downgrade_collada_15`]).
#[cfg(feature = "bake-dae")]
fn load_dae_geoms_from_bytes(
    bytes: &[u8],
    name: &str,
    opts: &LoadOpts,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    notes: &mut Vec<String>,
) -> Result<Vec<Geom>, BakeError> {
    use dae_parser::{Document, Effect, Geometry, Image, Material, Node, Source, VisualScene};
    use glam::Mat4;
    let LoadOpts {
        scale,
        color,
        keep_geometries,
        apply_color,
        ..
    } = *opts;

    let load_err = |msg: String| BakeError::Load {
        path: name.to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, msg),
    };
    // COLLADA 1.5 downgrade pre-pass (zero-copy for the 1.4 fast path, the pinned byte-identical bakes).
    let bytes = downgrade_collada_15(bytes, notes);
    let doc = Document::try_from(bytes.as_ref()).map_err(|e| load_err(format!("{e:?}")))?;
    // ID -> element maps for the source resolution + the material binding chain (lookup only).
    let source_map = doc
        .local_map::<Source>()
        .map_err(|e| load_err(format!("{e:?}")))?;
    let geometry_map = doc
        .local_map::<Geometry>()
        .map_err(|e| load_err(format!("{e:?}")))?;
    let mut matctx = DaeMatCtx {
        material_map: doc
            .local_map::<Material>()
            .map_err(|e| load_err(format!("{e:?}")))?,
        effect_map: doc
            .local_map::<Effect>()
            .map_err(|e| load_err(format!("{e:?}")))?,
        image_map: doc
            .local_map::<Image>()
            .map_err(|e| load_err(format!("{e:?}")))?,
        cache: std::collections::BTreeMap::new(),
        // A lean collision never embeds appearance, so its bake must not touch the resolver.
        want_textures: keep_geometries,
    };

    // The URDF mesh scale (possibly a mirror) folded into the vertices AFTER the node transform, exactly
    // as the mesh-loader path does. A negative determinant reverses winding so faces stay outward-facing.
    let (scale_mat, mirror) = match scale {
        Some(s) => {
            let m = Mat3::from_cols(
                Vec3::new(s[0], 0.0, 0.0),
                Vec3::new(0.0, s[1], 0.0),
                Vec3::new(0.0, 0.0, s[2]),
            );
            (Some(m), m.determinant() < 0.0)
        }
        None => (None, false),
    };

    // The visual scene whose node tree carries orientation (baked into verts; `<up_axis>` ignored).
    let scene: &VisualScene = doc
        .get_visual_scene()
        .or_else(|| doc.iter::<VisualScene>().next())
        .ok_or_else(|| load_err("COLLADA document has no visual_scene to bake".to_string()))?;

    let mut geoms: Vec<Geom> = Vec::new();
    // Depth-first walk; the stack carries each node with its parent's accumulated transform. Children are
    // pushed in reverse so siblings are visited in document order (deterministic geometry/material order).
    let mut stack: Vec<(&Node, Mat4)> = scene
        .nodes
        .iter()
        .rev()
        .map(|n| (n, Mat4::IDENTITY))
        .collect();
    while let Some((node, parent)) = stack.pop() {
        let accum = parent * node_local_matrix(node);
        let normal_mat = Mat3::from_mat4(accum);
        for inst in &node.instance_geometry {
            let Some(geometry) = geometry_map.get(&inst.url) else {
                continue;
            };
            let Some(mesh) = geometry.element.as_mesh() else {
                continue;
            };
            let Some(vertices) = &mesh.vertices else {
                continue;
            };
            let vimp = vertices.importer(&source_map).map_err(|()| {
                load_err(format!(
                    "unresolved vertex <source> in geometry {:?}",
                    geometry.id
                ))
            })?;
            for prim in &mesh.elements {
                let (soup, symbol) =
                    triangulate_primitive(prim, &vimp, &source_map).map_err(load_err)?;
                if soup.is_empty() {
                    continue;
                }
                let pbr = dae_material_pbr(symbol, inst, &mut matctx, &mut *resolve, notes);
                // World positions: node transform, then URDF scale.
                let mut positions: Vec<[f32; 3]> = Vec::with_capacity(soup.len());
                for v in &soup {
                    let wp = accum.transform_point3(Vec3::from_array(v.pos));
                    let p = match scale_mat {
                        Some(m) => (m * wp).to_array(),
                        None => wp.to_array(),
                    };
                    positions.push(p);
                }
                // Authored normals: visual path, every vertex carries one, rotated by the node's linear
                // part. When a URDF scale/mirror is folded in too, compose its NORMAL matrix
                // (inverse-transpose, [`normal_matrix`]) on top so the normal stays perpendicular to the
                // scaled surface and a mirror's outward direction is preserved; the position transform
                // is `scale · node`, so its normal transform is `normal_matrix(scale) · node_linear`.
                // This keeps a scaled/mirrored DAE smooth-shaded (unlike trimesh, which drops normals on
                // any scale, so the openarm left arm shaded like its right twin, not POSITION-only flat).
                let carry_normals = keep_geometries && soup.iter().all(|v| v.nrm.is_some());
                let nrm_xform = match scale_mat {
                    Some(sm) => normal_matrix(sm) * normal_mat,
                    None => normal_mat,
                };
                let normals: Vec<[f32; 3]> = if carry_normals {
                    soup.iter()
                        .map(|v| {
                            (nrm_xform * Vec3::from_array(v.nrm.unwrap()))
                                .normalize_or_zero()
                                .to_array()
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                // Triangle soup -> sequential indices; reverse each triangle's winding on a mirror. (The
                // soup, and with it the parallel position/normal/uv streams, is untouched by the
                // reorder, so a mirrored textured bake keeps its uvs in sync with the flipped triangles.)
                let ntri = soup.len() / 3;
                let mut indices: Vec<u32> = Vec::with_capacity(ntri * 3);
                for t in 0..ntri {
                    let b = (t * 3) as u32;
                    if mirror {
                        indices.extend_from_slice(&[b, b + 2, b + 1]);
                    } else {
                        indices.extend_from_slice(&[b, b + 1, b + 2]);
                    }
                }
                // A resolved DAE shader colour wins; else material-less + (visual only) the flat URDF
                // colour. The diffuse texture attaches only when the triangle-set also carries a
                // TEXCOORD input on every vertex (else a note + the flat fallback); a textured set's
                // baseColorFactor is WHITE (factor × texture, the module-docs convention), and its raw
                // COLLADA (S,T) flips to the glTF top-left origin. Scale/mirror leave uvs untouched.
                let has_uv = soup.iter().all(|v| v.uv.is_some());
                let (texture, geom_color, metallic, roughness) = match pbr {
                    Some(m) => {
                        let texture = match m.texture {
                            DaeTex::Image(_, img) if has_uv => Some(img),
                            DaeTex::Image(tex_name, _) => {
                                push_note(
                                    notes,
                                    format!(
                                        "texture {tex_name:?} not embedded (mesh has no texture \
                                         coordinates); flat colour used"
                                    ),
                                );
                                None
                            }
                            DaeTex::None | DaeTex::Unavailable => None,
                        };
                        let base = if texture.is_some() { WHITE } else { m.base };
                        (texture, Some(base), m.metallic, m.roughness)
                    }
                    None => {
                        // Material-less triangle-set: the flat hint colour / hint texture apply
                        // (visual only), the same bare-geometry arm as the mesh-loader path.
                        let hint = hint_texture_for_bare(true, opts, has_uv, notes);
                        let flat = if apply_color { color } else { None };
                        let base = if hint.is_some() { Some(WHITE) } else { flat };
                        (hint, base, MATTE.0, MATTE.1)
                    }
                };
                let uvs_soup: Vec<[f32; 2]> = if texture.is_some() {
                    soup.iter()
                        .map(|v| {
                            let [s, t] = v.uv.unwrap();
                            [s, 1.0 - t]
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let welded = weld(&positions, &normals, &uvs_soup, &indices);
                geoms.push(Geom {
                    positions: welded.positions,
                    normals: welded.normals,
                    uvs: welded.uvs,
                    indices: welded.indices,
                    color: geom_color,
                    metallic,
                    roughness,
                    texture,
                    // A loaded DAE keeps its authored winding, never forced double-sided (byte-identity).
                    double_sided: false,
                });
            }
        }
        for child in node.children.iter().rev() {
            stack.push((child, accum));
        }
    }
    if geoms.is_empty() {
        return Err(BakeError::Empty {
            path: name.to_string(),
        });
    }
    Ok(geoms)
}

/// Weld coincident vertices: quantize each position (and, when carried, normal / uv) to a `WELD_EPS`
/// grid, deduplicate, and remap the indices. Deterministic: vertices are emitted in first-seen order
/// over the (already deterministic) index stream, so the same input always yields the same
/// `positions`/`normals`/`uvs`/`indices` and thus the same GLB.
///
/// `normals` and `uvs` are each either empty or parallel to `positions`. When present they join the
/// weld key `(quantized position[, normal][, uv])`, so two vertices at the same point but with
/// different normals (a hard edge) or different uvs (a texture seam) stay distinct, keeping the 1:1
/// pairing the GLB `NORMAL`/`TEXCOORD_0` attributes need. Each returned stream is empty iff its input
/// was empty; with both absent the key, and therefore the weld, is EXACTLY the position-only weld
/// (the pre-texture byte-identity invariant).
fn weld(positions: &[[f32; 3]], normals: &[[f32; 3]], uvs: &[[f32; 2]], indices: &[u32]) -> Welded {
    use std::collections::HashMap;
    let with_normals = !normals.is_empty();
    let with_uvs = !uvs.is_empty();
    debug_assert!(!with_normals || normals.len() == positions.len());
    debug_assert!(!with_uvs || uvs.len() == positions.len());
    // Key by the quantized (position[, normal][, uv]) grid cell; first-seen order is driven by the index
    // stream (deterministic). `[i64; 8]` holds position in [0..3], normal in [3..6], uv in [6..8]
    // (zeros when a stream is absent, indistinguishable from the shorter key, so absent streams cannot
    // change the weld). Reserve for the no-dedup worst case (every vertex unique) so the map never
    // rehashes/regrows mid-weld.
    let mut map: HashMap<[i64; 8], u32> = HashMap::with_capacity(positions.len());
    let mut out_pos: Vec<[f32; 3]> = Vec::with_capacity(positions.len());
    let mut out_nrm: Vec<[f32; 3]> = Vec::with_capacity(normals.len());
    let mut out_uv: Vec<[f32; 2]> = Vec::with_capacity(uvs.len());
    let mut out_idx: Vec<u32> = Vec::with_capacity(indices.len());
    let q = |x: f32| (x as f64 / WELD_EPS as f64).round() as i64;
    for &i in indices {
        let p = positions[i as usize];
        let n = if with_normals {
            normals[i as usize]
        } else {
            [0.0; 3]
        };
        let t = if with_uvs { uvs[i as usize] } else { [0.0; 2] };
        let key = [
            q(p[0]),
            q(p[1]),
            q(p[2]),
            q(n[0]),
            q(n[1]),
            q(n[2]),
            q(t[0]),
            q(t[1]),
        ];
        let next = out_pos.len() as u32;
        let id = *map.entry(key).or_insert_with(|| {
            out_pos.push(p);
            if with_normals {
                out_nrm.push(n);
            }
            if with_uvs {
                out_uv.push(t);
            }
            next
        });
        out_idx.push(id);
    }
    Welded {
        positions: out_pos,
        normals: out_nrm,
        uvs: out_uv,
        indices: out_idx,
    }
}

/// The welded output streams of [`weld`] (`normals`/`uvs` empty iff their inputs were).
struct Welded {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

// ── deterministic GLB writer ────────────────────────────────────────────────────────────────────────
//
// glTF/GLB component & type constants (the spec values; named to keep the writer readable). The GLB
// container constants are shared with the `.gltf` → GLB packer (`crate::gltf_pack`), the one other
// place a GLB frame is written.
use crate::gltf_pack::{CHUNK_BIN, CHUNK_JSON, GLB_MAGIC, GLB_VERSION};
const COMPONENT_U32: u32 = 5125; // UNSIGNED_INT (indices)
const COMPONENT_F32: u32 = 5126; // FLOAT (positions)
const TARGET_ELEMENT_ARRAY: u32 = 34963; // ELEMENT_ARRAY_BUFFER (indices)
const TARGET_ARRAY: u32 = 34962; // ARRAY_BUFFER (vertex attributes)
const MODE_TRIANGLES: u32 = 4;
// The single default sampler (emitted only when a texture is embedded): LINEAR magnification,
// trilinear minification, REPEAT wrap: the values trimesh-style exporters write.
const FILTER_LINEAR: u32 = 9729;
const FILTER_LINEAR_MIPMAP_LINEAR: u32 = 9987;
const WRAP_REPEAT: u32 = 10497;

/// A single accessor's metadata, emitted in a fixed order so the JSON chunk is byte-stable.
struct Accessor {
    buffer_view: usize,
    component_type: u32,
    count: usize,
    type_: &'static str, // "SCALAR" | "VEC2" | "VEC3"
    min: Vec<f64>,
    max: Vec<f64>,
}

/// One primitive's accessor/material wiring (fixed fields, fixed order → byte-stable JSON).
struct Prim {
    indices: usize,
    position: usize,
    normal: Option<usize>,
    texcoord: Option<usize>,
    material: Option<usize>,
}

/// One deduplicated glTF material: the flat PBR factors plus, for a textured geometry, its texture
/// index (== image index: one texture per unique image, all on the single default sampler). The two
/// appearance flags below both take part in the content dedup (a `derive`d `PartialEq`), so materials
/// that differ only in transparency or sidedness stay distinct.
#[derive(PartialEq)]
struct GlbMaterial {
    color: [f32; 4],
    metallic: f32,
    roughness: f32,
    texture: Option<usize>,
    /// Emit `"alphaMode":"BLEND"`: set when the bound texture carries alpha ([`TextureImage::has_alpha`]),
    /// so a transparent-background logo renders TRANSPARENT instead of the old opaque black. `false` for
    /// an opaque texture / flat colour, whose material is then byte-identical to the pre-alpha writer.
    alpha_blend: bool,
    /// Emit `"doubleSided":true`: set only for a synthesized decal-plane primitive (see [`Geom::double_sided`]).
    double_sided: bool,
}

/// Build a deterministic embedded GLB from the baked geometries. Buffer layout, accessor / bufferView /
/// mesh / node / material ordering, and the JSON serialization are all fixed (no `HashMap`, no
/// timestamps, a constant generator) so the bytes, and therefore the `content_sha`, are stable across
/// runs and machines.
fn write_glb(geoms: &[Geom]) -> Vec<u8> {
    // BIN chunk: for each geometry, indices (u32, 4-byte aligned) then positions (f32x3), then optional
    // normals (f32x3) / texcoords (f32x2), all 4-byte multiples, so everything stays aligned with no
    // inter-stream padding; embedded texture images (arbitrary length, each padded to 4) land AFTER
    // every geometry stream, keeping a texture-less BIN chunk byte-identical to the pre-texture writer.
    // The geometry size is computable up front; reserve it exactly (images extend once, below).
    let bin_len: usize = geoms
        .iter()
        .map(|g| g.indices.len() * 4 + (g.positions.len() + g.normals.len()) * 12 + g.uvs.len() * 8)
        .sum();
    let mut bin: Vec<u8> = Vec::with_capacity(bin_len);
    let mut buffer_views: Vec<(usize, usize, Option<u32>)> = Vec::new(); // (byteOffset, byteLength, target)
    let mut accessors: Vec<Accessor> = Vec::new();
    let mut prims: Vec<Prim> = Vec::new();
    // Unique materials / images in first-seen geometry order. Both deduplicated by a linear CONTENT
    // scan (NOT a `HashMap`) so the ordering, and thus the GLB bytes, stay deterministic; two
    // geometries bound to the same COLLADA material (a `.dae` may reuse one) collapse to a single glTF
    // material, exactly like the trimesh/Python bake, and two materials sharing one `skin.png` embed
    // the image once.
    let mut materials: Vec<GlbMaterial> = Vec::new();
    let mut images: Vec<TexRef> = Vec::new();

    for g in geoms {
        // Indices bufferView + accessor.
        let idx_off = bin.len();
        for &i in &g.indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let idx_len = bin.len() - idx_off;
        let idx_bv = buffer_views.len();
        buffer_views.push((idx_off, idx_len, Some(TARGET_ELEMENT_ARRAY)));
        let idx_max = g.indices.iter().copied().max().unwrap_or(0) as f64;
        let idx_acc = accessors.len();
        accessors.push(Accessor {
            buffer_view: idx_bv,
            component_type: COMPONENT_U32,
            count: g.indices.len(),
            type_: "SCALAR",
            min: vec![0.0],
            max: vec![idx_max],
        });

        // Positions bufferView + accessor (with the bbox min/max, which the comparator reads).
        // 4-byte align (already aligned: u32 indices). Positions are f32x3 = 12B, also 4-byte aligned.
        let pos_off = bin.len();
        let mut mn = [f64::INFINITY; 3];
        let mut mx = [f64::NEG_INFINITY; 3];
        for p in &g.positions {
            for k in 0..3 {
                bin.extend_from_slice(&p[k].to_le_bytes());
                mn[k] = mn[k].min(p[k] as f64);
                mx[k] = mx[k].max(p[k] as f64);
            }
        }
        let pos_len = bin.len() - pos_off;
        let pos_bv = buffer_views.len();
        buffer_views.push((pos_off, pos_len, Some(TARGET_ARRAY)));
        let pos_acc = accessors.len();
        accessors.push(Accessor {
            buffer_view: pos_bv,
            component_type: COMPONENT_F32,
            count: g.positions.len(),
            type_: "VEC3",
            min: mn.to_vec(),
            max: mx.to_vec(),
        });

        // NORMAL bufferView + accessor, only when the source authored per-vertex normals (carried by
        // `Geom.normals`, parallel to positions). The glTF spec requires accessor min/max only on
        // POSITION, so normals carry none (matching what trimesh writes).
        let nrm_acc = if g.normals.is_empty() {
            None
        } else {
            let nrm_off = bin.len();
            for n in &g.normals {
                for comp in n {
                    bin.extend_from_slice(&comp.to_le_bytes());
                }
            }
            let nrm_len = bin.len() - nrm_off;
            let nrm_bv = buffer_views.len();
            buffer_views.push((nrm_off, nrm_len, Some(TARGET_ARRAY)));
            let acc = accessors.len();
            accessors.push(Accessor {
                buffer_view: nrm_bv,
                component_type: COMPONENT_F32,
                count: g.normals.len(),
                type_: "VEC3",
                min: Vec::new(),
                max: Vec::new(),
            });
            Some(acc)
        };

        // TEXCOORD_0 bufferView + accessor + the deduplicated image, ONLY for a textured geometry
        // (uvs AND image both present, the loaders' pairing). No texture → none of this exists and the
        // stream layout is exactly the pre-texture writer's.
        let textured = !g.uvs.is_empty() && g.texture.is_some();
        let (uv_acc, tex_idx) = if textured {
            let uv_off = bin.len();
            for uv in &g.uvs {
                for comp in uv {
                    bin.extend_from_slice(&comp.to_le_bytes());
                }
            }
            let uv_len = bin.len() - uv_off;
            let uv_bv = buffer_views.len();
            buffer_views.push((uv_off, uv_len, Some(TARGET_ARRAY)));
            let acc = accessors.len();
            accessors.push(Accessor {
                buffer_view: uv_bv,
                component_type: COMPONENT_F32,
                count: g.uvs.len(),
                type_: "VEC2",
                min: Vec::new(),
                max: Vec::new(),
            });
            // Dedup the image by CONTENT in first-use order (Arc identity as the cheap fast path).
            let img = g.texture.as_ref().expect("textured implies an image");
            let idx = match images.iter().position(|e| {
                TexRef::ptr_eq(e, img) || (e.mime == img.mime && e.bytes == img.bytes)
            }) {
                Some(i) => i,
                None => {
                    images.push(img.clone());
                    images.len() - 1
                }
            };
            (Some(acc), Some(idx))
        } else {
            (None, None)
        };

        // A textured material blends when its image carries alpha (the deduped image shares the source's
        // `has_alpha`, so reading it off `g.texture` is equivalent to reading `images[idx]`). Flat colours
        // never blend here; this fix is scoped to TEXTURE alpha (a flat baseColorFactor alpha stays
        // ignored under the default OPAQUE mode, exactly as before), keeping every opaque bake unchanged.
        let alpha_blend = tex_idx.is_some() && g.texture.as_deref().is_some_and(|t| t.has_alpha);
        let mat_idx = g.color.map(|c| {
            let key = GlbMaterial {
                color: c,
                metallic: g.metallic,
                roughness: g.roughness,
                texture: tex_idx,
                alpha_blend,
                double_sided: g.double_sided,
            };
            // Reuse an identical material (deterministic first-seen dedup), else append a new one.
            match materials.iter().position(|m| *m == key) {
                Some(i) => i,
                None => {
                    materials.push(key);
                    materials.len() - 1
                }
            }
        });
        prims.push(Prim {
            indices: idx_acc,
            position: pos_acc,
            normal: nrm_acc,
            texcoord: uv_acc,
            material: mat_idx,
        });
    }

    // Embedded texture images: verbatim PNG/JPEG bytes appended after every geometry stream, each on a
    // 4-byte boundary, one target-less bufferView per unique image (the glTF-internal image shape, as
    // the `.gltf` packer writes it). `image_views[i]` is image i's bufferView index.
    let image_views: Vec<(usize, &'static str)> = images
        .iter()
        .map(|img| {
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            let off = bin.len();
            bin.extend_from_slice(&img.bytes);
            let bv = buffer_views.len();
            buffer_views.push((off, img.bytes.len(), None));
            (bv, img.mime)
        })
        .collect();

    // Pad the BIN chunk to a 4-byte boundary (the GLB spec requires each chunk 4-byte aligned).
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    let json = build_json(
        &buffer_views,
        &accessors,
        &prims,
        &materials,
        &image_views,
        bin.len(),
    );
    let mut json_bytes = json.into_bytes();
    // Pad the JSON chunk with spaces to a 4-byte boundary (spec: JSON padded with 0x20).
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }

    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    // JSON chunk.
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    // BIN chunk.
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

/// Serialize the glTF JSON chunk by HAND in a fixed field order (no serde map, no `HashMap`) so the
/// bytes are byte-stable across runs/machines. Floats are printed via [`ryu`] (shortest round-trip,
/// deterministic), integers plainly. Texture JSON (`baseColorTexture` + the `textures`/`samplers`/
/// `images` arrays) exists ONLY when `images` is non-empty; a texture-less document serializes to
/// exactly the pre-texture bytes.
fn build_json(
    buffer_views: &[(usize, usize, Option<u32>)],
    accessors: &[Accessor],
    prims: &[Prim],
    materials: &[GlbMaterial],
    images: &[(usize, &'static str)],
    bin_len: usize,
) -> String {
    let mut s = String::new();
    s.push('{');
    // asset: constant generator, no timestamp.
    s.push_str("\"asset\":{\"version\":\"2.0\",\"generator\":\"hcdformat-rs baker\"},");

    // scene + scenes + nodes: a single scene, one root node, one child node per primitive/mesh.
    s.push_str("\"scene\":0,");
    s.push_str("\"scenes\":[{\"nodes\":[0]}],");
    s.push_str("\"nodes\":[");
    // root node 0 -> children [1..=N].
    s.push_str("{\"children\":[");
    for i in 0..prims.len() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&(i + 1).to_string());
    }
    s.push_str("]}");
    for (i, _) in prims.iter().enumerate() {
        s.push_str(",{\"mesh\":");
        s.push_str(&i.to_string());
        s.push('}');
    }
    s.push_str("],");

    // meshes: one mesh per primitive (each its own POSITION/indices accessor pair).
    s.push_str("\"meshes\":[");
    for (i, prim) in prims.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"primitives\":[{\"attributes\":{\"POSITION\":");
        s.push_str(&prim.position.to_string());
        if let Some(n) = prim.normal {
            s.push_str(",\"NORMAL\":");
            s.push_str(&n.to_string());
        }
        if let Some(t) = prim.texcoord {
            s.push_str(",\"TEXCOORD_0\":");
            s.push_str(&t.to_string());
        }
        s.push_str("},\"indices\":");
        s.push_str(&prim.indices.to_string());
        s.push_str(",\"mode\":");
        s.push_str(&MODE_TRIANGLES.to_string());
        if let Some(m) = prim.material {
            s.push_str(",\"material\":");
            s.push_str(&m.to_string());
        }
        s.push_str("}]}");
    }
    s.push_str("],");

    // accessors.
    s.push_str("\"accessors\":[");
    for (i, a) in accessors.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"bufferView\":");
        s.push_str(&a.buffer_view.to_string());
        s.push_str(",\"componentType\":");
        s.push_str(&a.component_type.to_string());
        s.push_str(",\"count\":");
        s.push_str(&a.count.to_string());
        s.push_str(",\"type\":\"");
        s.push_str(a.type_);
        s.push('"');
        // min/max are required on POSITION and emitted for indices; a NORMAL accessor carries none (empty
        // vecs) so its keys are omitted entirely (spec: min/max optional except where the spec mandates).
        if !a.min.is_empty() || !a.max.is_empty() {
            s.push_str(",\"min\":[");
            push_num_list(&mut s, &a.min, a.component_type == COMPONENT_U32);
            s.push_str("],\"max\":[");
            push_num_list(&mut s, &a.max, a.component_type == COMPONENT_U32);
            s.push(']');
        }
        s.push('}');
    }
    s.push_str("],");

    // bufferViews.
    s.push_str("\"bufferViews\":[");
    for (i, (off, len, target)) in buffer_views.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"buffer\":0,\"byteOffset\":");
        s.push_str(&off.to_string());
        s.push_str(",\"byteLength\":");
        s.push_str(&len.to_string());
        if let Some(t) = target {
            s.push_str(",\"target\":");
            s.push_str(&t.to_string());
        }
        s.push('}');
    }
    s.push_str("],");

    // materials (resolved per-mesh DAE/.mtl materials and/or the bare-geometry flat colour,
    // deduplicated in geometry order). Each carries baseColorFactor + (textured only) baseColorTexture
    // + metallicFactor + roughnessFactor (the matte default is 0 / 1; a Phong/Blinn `.dae` gets a
    // roughness < 1 from its normalized shininess; a textured material's factor is WHITE).
    if !materials.is_empty() {
        s.push_str("\"materials\":[");
        for (i, m) in materials.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let c = m.color;
            s.push_str("{\"pbrMetallicRoughness\":{\"baseColorFactor\":[");
            push_num_list(
                &mut s,
                &[c[0] as f64, c[1] as f64, c[2] as f64, c[3] as f64],
                false,
            );
            s.push(']');
            if let Some(t) = m.texture {
                s.push_str(",\"baseColorTexture\":{\"index\":");
                s.push_str(&t.to_string());
                s.push('}');
            }
            s.push_str(",\"metallicFactor\":");
            push_num_list(&mut s, &[m.metallic as f64], false);
            s.push_str(",\"roughnessFactor\":");
            push_num_list(&mut s, &[m.roughness as f64], false);
            s.push('}'); // close pbrMetallicRoughness
                         // Transparency + sidedness are MATERIAL-level (siblings of pbrMetallicRoughness). BLEND (not
                         // MASK) is the safe choice for the antialiased CogniPilot/NXP logos: MASK's hard alphaCutoff
                         // aliases their soft edges, whereas BLEND composites them smoothly; the WHITE baseColorFactor
                         // keeps alpha at 1.0 so the texture's own alpha drives transparency. Both keys are emitted ONLY
                         // when set, so an opaque single-sided material serializes to exactly the pre-alpha bytes (the
                         // trailing `}}` is reproduced by this `}` plus the material-closing `}` below).
            if m.alpha_blend {
                s.push_str(",\"alphaMode\":\"BLEND\"");
            }
            if m.double_sided {
                s.push_str(",\"doubleSided\":true");
            }
            s.push('}'); // close material
        }
        s.push_str("],");
    }

    // textures / samplers / images, only when a texture is embedded (texture i sources image i; ONE
    // default sampler shared by all). Images are GLB-internal: bufferView + mimeType, verbatim bytes.
    if !images.is_empty() {
        s.push_str("\"textures\":[");
        for i in 0..images.len() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"sampler\":0,\"source\":");
            s.push_str(&i.to_string());
            s.push('}');
        }
        s.push_str("],\"samplers\":[{\"magFilter\":");
        s.push_str(&FILTER_LINEAR.to_string());
        s.push_str(",\"minFilter\":");
        s.push_str(&FILTER_LINEAR_MIPMAP_LINEAR.to_string());
        s.push_str(",\"wrapS\":");
        s.push_str(&WRAP_REPEAT.to_string());
        s.push_str(",\"wrapT\":");
        s.push_str(&WRAP_REPEAT.to_string());
        s.push_str("}],\"images\":[");
        for (i, (bv, mime)) in images.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"bufferView\":");
            s.push_str(&bv.to_string());
            s.push_str(",\"mimeType\":\"");
            s.push_str(mime);
            s.push_str("\"}");
        }
        s.push_str("],");
    }

    // buffers (single embedded buffer; the BIN chunk supplies the bytes, no uri).
    s.push_str("\"buffers\":[{\"byteLength\":");
    s.push_str(&bin_len.to_string());
    s.push_str("}]}");
    s
}

/// Append a comma-separated number list. Integers (`as_int`) print plainly; floats via [`ryu`] (shortest
/// round-trip, locale-free, deterministic), but a value that is integral prints without a fraction so
/// `0`/`1` colour channels and integer bbox bounds stay compact and stable.
fn push_num_list(s: &mut String, vals: &[f64], as_int: bool) {
    let mut buf = ryu::Buffer::new();
    for (i, v) in vals.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        if as_int || v.fract() == 0.0 {
            s.push_str(&(*v as i64).to_string());
        } else {
            s.push_str(buf.format(*v));
        }
    }
}

// ── public conversion API (mirrors hcdf.assets.to_glb / to_lean / the conversion half of bake) ───────

/// Classify `src_name` by extension and apply the shared entry gates: an unknown extension is
/// [`BakeError::UnsupportedFormat`], a GLB/glTF is [`BakeError::AlreadyGlb`] (a visual GLB is a
/// verbatim passthrough / `.gltf` pack, never re-baked here), and a `.dae` without `bake-dae` is
/// [`BakeError::DaeUnsupported`]: the identical preamble of every conversion entry point.
fn gated_src_format(src_name: &str) -> Result<SrcFormat, BakeError> {
    let fmt = SrcFormat::from_path(src_name).ok_or_else(|| BakeError::UnsupportedFormat {
        path: src_name.to_string(),
    })?;
    if fmt == SrcFormat::Glb {
        return Err(BakeError::AlreadyGlb {
            path: src_name.to_string(),
        });
    }
    gate_dae(fmt, src_name)?;
    Ok(fmt)
}

/// The [`LoadOpts`] of a VISUAL bake (scene kept per-geometry, flat colour + hint texture applied to
/// bare geometry). A non-embeddable hint texture degrades to `None` with a note (never an error).
fn visual_opts(
    scale: Option<&str>,
    color: Option<&str>,
    texture: Option<&HintTexture>,
    notes: &mut Vec<String>,
) -> LoadOpts {
    LoadOpts {
        scale: scale_vec(scale),
        color: color_vec(color),
        texture: hint_texture_image(texture, notes),
        keep_geometries: true,
        apply_color: true,
    }
}

/// The [`LoadOpts`] of a COLLISION bake: one merged geometry (trimesh `force="mesh"`), no appearance.
fn lean_opts(scale: Option<&str>) -> LoadOpts {
    LoadOpts {
        scale: scale_vec(scale),
        color: None,
        texture: None,
        keep_geometries: false,
        apply_color: false,
    }
}

/// Load a source mesh (STL/OBJ/DAE) and export a deterministic embedded GLB with `scale` (possibly a
/// negative-component mirror) and `color` baked in. Port of `hcdf.assets.to_glb`.
///
/// `scale` is a `"sx sy sz"` (or single-value) string; `color` an `"r g b [a]"` (0..1) string. A GLB/glTF
/// source is rejected with [`BakeError::AlreadyGlb`] (it is a verbatim passthrough, not a re-export); a
/// `.dae` source needs the `bake-dae` feature else [`BakeError::DaeUnsupported`]. Companions resolve
/// from the file's own directory: an OBJ's `mtllib` (so its `usemtl` groups keep their `.mtl`
/// colours) and the diffuse texture images (OBJ `map_Kd` / DAE `<init_from>`, embedded per the module
/// "Textures" docs); the appearance-fallback notes that load may accumulate are dropped here, since
/// [`bake_convert`] is the path-based entry that carries them ([`BakedAsset::notes`]).
pub fn to_glb(
    src_path: &str,
    scale: Option<&str>,
    color: Option<&str>,
) -> Result<Vec<u8>, BakeError> {
    Ok(to_glb_notes(src_path, scale, color, None)?.0)
}

/// Notes-carrying core of [`to_glb`] (and the visual arm of [`bake_convert_with_texture`]): the same
/// bake, returning `(glb_bytes, notes)` so the caller can surface the non-fatal appearance fallbacks.
/// `texture` is the importer's already-resolved hint texture (see [`LoadOpts::texture`]), or `None`.
fn to_glb_notes(
    src_path: &str,
    scale: Option<&str>,
    color: Option<&str>,
    texture: Option<&HintTexture>,
) -> Result<(Vec<u8>, Vec<String>), BakeError> {
    let fmt = gated_src_format(src_path)?;
    let mut notes = Vec::new();
    let opts = visual_opts(scale, color, texture, &mut notes);
    let geoms = load_geoms(src_path, fmt, &opts, &mut notes)?;
    Ok((write_glb(&geoms), notes))
}

/// In-RAM counterpart of [`to_glb`]: bake an in-memory mesh `bytes` (a STL/OBJ/DAE source identified by
/// `src_name`'s extension) into the same deterministic embedded GLB, with NO filesystem access: the
/// wasm-clean path the browser folder-importer drives so a non-GLB visual still RENDERS through the glTF
/// loader. `src_name` supplies ONLY the format hint + error context (no file is read). A GLB/glTF input is
/// [`BakeError::AlreadyGlb`] (it is a verbatim passthrough, not a re-export); a `.dae` needs `bake-dae`.
///
/// NO companion resolver here, so an OBJ's `mtllib` cannot be honoured (its groups bake material-less
/// gray) and no texture image can embed; each drop is a note. A caller holding companion bytes (the
/// browser upload map) uses [`bake_convert_bytes_with`] instead.
pub fn to_glb_bytes(
    bytes: &[u8],
    src_name: &str,
    scale: Option<&str>,
    color: Option<&str>,
) -> Result<Vec<u8>, BakeError> {
    to_glb_bytes_notes(
        bytes,
        src_name,
        scale,
        color,
        None,
        &mut |_| None,
        &mut Vec::new(),
    )
}

/// Resolver- and notes-carrying core of [`to_glb_bytes`] / the visual arm of
/// [`bake_convert_bytes_with_texture`]: `resolve` supplies OBJ `mtllib` companion bytes by OBJ-relative
/// uri, `texture` is the importer's already-resolved hint texture (see [`LoadOpts::texture`]), `notes`
/// collects the appearance fallbacks.
fn to_glb_bytes_notes(
    bytes: &[u8],
    src_name: &str,
    scale: Option<&str>,
    color: Option<&str>,
    texture: Option<&HintTexture>,
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    notes: &mut Vec<String>,
) -> Result<Vec<u8>, BakeError> {
    let fmt = gated_src_format(src_name)?;
    let opts = visual_opts(scale, color, texture, notes);
    let geoms = load_geoms_from_bytes(bytes, src_name, fmt, &opts, resolve, notes)?;
    Ok(write_glb(&geoms))
}

/// Bake `scale` into a lean collision mesh (into the vertices, mirror-safe) and return BINARY STL bytes.
///
/// Port of `hcdf.assets.to_lean`, with a DELIBERATE, DOCUMENTED divergence: Python keeps the collision
/// SOURCE format (`to_lean(src, scale, file_type=ext)`, an OBJ stays `.obj`), whereas this CANONICALIZES
/// every scaled collision to binary STL (the universal lean format). See the module-level "Collision"
/// docs for the full rationale and the Python↔Rust contract (a converted collision is canonical
/// STL on both sides; the verbatim passthrough in [`crate::assets_vendor::bake`] is unaffected and still
/// keeps the source format byte-identically). A `.dae` source needs `bake-dae`.
pub fn to_lean(src_path: &str, scale: Option<&str>) -> Result<Vec<u8>, BakeError> {
    let fmt = gated_src_format(src_path)?;
    // Materials are irrelevant to a lean collision shape, so the notes (mtl/texture fallbacks) are
    // discarded, since nothing appearance-related is lost from an output that never carries appearance.
    let geoms = load_geoms(src_path, fmt, &lean_opts(scale), &mut Vec::new())?;
    Ok(write_binary_stl(&geoms))
}

/// In-RAM counterpart of [`to_lean`]: bake an in-memory mesh `bytes` into the same lean binary STL (scale
/// folded into the vertices, mirror-safe), with NO filesystem access. `src_name` supplies only the format
/// hint + error context; the output is canonical binary STL regardless of source format (see [`to_lean`]).
pub fn to_lean_bytes(
    bytes: &[u8],
    src_name: &str,
    scale: Option<&str>,
) -> Result<Vec<u8>, BakeError> {
    let fmt = gated_src_format(src_name)?;
    // No resolver, no notes: a lean shape carries no appearance (see [`to_lean`]).
    let geoms = load_geoms_from_bytes(
        bytes,
        src_name,
        fmt,
        &lean_opts(scale),
        &mut |_| None,
        &mut Vec::new(),
    )?;
    Ok(write_binary_stl(&geoms))
}

/// The result of baking one mesh to a content-addressed canonical asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BakedAsset {
    /// The canonical bytes (a GLB for a visual, binary STL for a collision).
    pub data: Vec<u8>,
    /// `"sha256:<hex>"` over `data`: the stable content address (`@sha`).
    pub sha: String,
    /// The canonical extension (`"glb"` for visual, `"stl"` for a converted collision).
    pub ext: &'static str,
    /// Informational appearance-fallback notes accumulated by a VISUAL bake: an OBJ whose `mtllib`
    /// could not be resolved (its groups baked gray), a diffuse texture that could NOT be embedded
    /// (image not resolved / not PNG-or-JPEG / the mesh has no texture coordinates, so the flat colour
    /// stands), a COLLADA 1.5 downgrade. Never fatal and empty for most sources; carried so a lossy
    /// fallback is VISIBLE instead of silent. Always empty for a collision (a lean shape
    /// has no appearance).
    pub notes: Vec<String>,
}

/// Whether a mesh is being baked for appearance (GLB) or physics shape (lean STL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BakeKind {
    /// A visual `<model>` -> a canonical GLB (`to_glb`).
    Visual,
    /// A collision `<mesh>` -> a lean binary STL (`to_lean`).
    Collision,
}

/// Bake one mesh to a canonical, content-addressed asset (the CONVERSION half of `hcdf.assets.bake`;
/// the verbatim passthrough of an already-GLB visual / unscaled collision lives in
/// [`crate::assets_vendor::bake`], so the bundle byte-parity guarantee is untouched). Drives the
/// URDF/SDF importer where a source scale must be folded out into the geometry.
///
///   * [`BakeKind::Visual`] -> a deterministic GLB with `scale`/`color` baked in;
///   * [`BakeKind::Collision`] -> a lean binary STL with `scale` baked in. NOTE the collision is
///     CANONICALIZED to binary STL regardless of source format (so `ext` is always `"stl"`), a documented
///     divergence from Python's keep-source-format `bake()`; see [`to_lean`] / the module docs for the
///     rationale and the importer contract. The verbatim passthrough ([`crate::assets_vendor::bake`])
///     keeps the source ext and is the path a valid HCDF document's collisions actually take.
///
/// The returned `sha` is `content_sha(data)`, recomputed by this baker (NOT cross-checked against
/// Python's, which legitimately differs because the GLB/lean writers differ; the parity bar is semantic).
pub fn bake_convert(
    src_path: &str,
    scale: Option<&str>,
    kind: BakeKind,
    color: Option<&str>,
) -> Result<BakedAsset, BakeError> {
    bake_convert_with_texture(src_path, scale, kind, color, None)
}

/// [`bake_convert`] with the importer's HINT TEXTURE: the [`crate::VisualAssetHint::texture`] uri
/// already resolved to image bytes by the caller (the same package-root machinery mesh uris resolve
/// through). The texture reaches only MATERIAL-LESS visual geometry that carries texture coordinates
/// (a mesh's own materials/textures win, exactly the flat-`color` precedence), embeds verbatim
/// (PNG/JPEG) with a WHITE `baseColorFactor` (the module-docs factor×texture convention, so it also
/// beats a hint `color` for the baseColor slot), and every miss (unsupported format / no texture
/// coordinates, e.g. an STL) is a [`BakedAsset::notes`] entry + the flat colour, never an error. A
/// collision bake ignores it entirely (a lean shape carries no appearance). `texture: None` is
/// byte-identical to [`bake_convert`].
pub fn bake_convert_with_texture(
    src_path: &str,
    scale: Option<&str>,
    kind: BakeKind,
    color: Option<&str>,
    texture: Option<&HintTexture>,
) -> Result<BakedAsset, BakeError> {
    let (data, ext, notes) = match kind {
        BakeKind::Visual => {
            let (data, notes) = to_glb_notes(src_path, scale, color, texture)?;
            (data, "glb", notes)
        }
        BakeKind::Collision => (to_lean(src_path, scale)?, "stl", Vec::new()),
    };
    let sha = content_sha(&data);
    Ok(BakedAsset {
        data,
        sha,
        ext,
        notes,
    })
}

/// In-RAM counterpart of [`bake_convert`]: bake an in-memory mesh `bytes` (format inferred from
/// `src_name`'s extension) to a content-addressed canonical asset: a GLB for [`BakeKind::Visual`], a lean
/// binary STL for [`BakeKind::Collision`], with NO filesystem access. This is the wasm-clean baker the
/// browser folder-importer drives so a non-GLB visual mesh is canonicalized to GLB and therefore RENDERS
/// through the glTF loader (the desktop importer uses the disk-reading [`bake_convert`]). The returned
/// `sha` is `content_sha(data)` over the baked bytes (recomputed, like [`bake_convert`]).
pub fn bake_convert_bytes(
    bytes: &[u8],
    src_name: &str,
    scale: Option<&str>,
    kind: BakeKind,
    color: Option<&str>,
) -> Result<BakedAsset, BakeError> {
    // No companion resolver: an OBJ visual with a `mtllib` bakes material-less here, with a note
    // saying so; a caller holding companion bytes uses [`bake_convert_bytes_with`].
    bake_convert_bytes_with(bytes, src_name, scale, kind, color, |_| None)
}

/// [`bake_convert_bytes`] with a COMPANION RESOLVER: the wasm-clean, in-RAM bake the browser
/// folder-importer drives when the upload map holds an OBJ's `.mtl` (and its `map_Kd` texture images)
/// or a DAE's texture images next to it. `resolve` supplies one companion's bytes by its
/// source-relative uri (`"digit.mtl"` for `mtllib digit.mtl`, `"skin.png"` for its `map_Kd`),
/// returning `None` when it cannot, exactly the [`crate::gltf_pack::pack_gltf_to_glb`] resolver
/// contract, e.g. `bake_convert_bytes_with(bytes, name, scale, kind, color, |uri|
/// self.resolve_mesh(uri, base_dir))`. An unresolved companion is NEVER fatal: the affected `usemtl`
/// groups bake material-less / the texture stays a flat colour, and the drop lands in
/// [`BakedAsset::notes`]. A collision bake ignores the resolver entirely (a lean shape carries no
/// appearance).
pub fn bake_convert_bytes_with(
    bytes: &[u8],
    src_name: &str,
    scale: Option<&str>,
    kind: BakeKind,
    color: Option<&str>,
    resolve: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<BakedAsset, BakeError> {
    bake_convert_bytes_with_texture(bytes, src_name, scale, kind, color, None, resolve)
}

/// [`bake_convert_bytes_with`] + the importer's HINT TEXTURE: the in-RAM counterpart of
/// [`bake_convert_with_texture`] (same precedence, WHITE-factor convention, and note-not-error
/// fallbacks; see there). The caller resolves the [`crate::VisualAssetHint::texture`] uri to bytes
/// itself (a `model://…` uri needs the package-root machinery, which the mesh-relative companion
/// `resolve` deliberately does not have). `texture: None` is byte-identical to
/// [`bake_convert_bytes_with`].
pub fn bake_convert_bytes_with_texture(
    bytes: &[u8],
    src_name: &str,
    scale: Option<&str>,
    kind: BakeKind,
    color: Option<&str>,
    texture: Option<&HintTexture>,
    mut resolve: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<BakedAsset, BakeError> {
    let mut notes = Vec::new();
    let (data, ext) = match kind {
        BakeKind::Visual => (
            to_glb_bytes_notes(
                bytes,
                src_name,
                scale,
                color,
                texture,
                &mut resolve,
                &mut notes,
            )?,
            "glb",
        ),
        BakeKind::Collision => (to_lean_bytes(bytes, src_name, scale)?, "stl"),
    };
    let sha = content_sha(&data);
    Ok(BakedAsset {
        data,
        sha,
        ext,
        notes,
    })
}

/// Synthesize the TEXTURED-PRIMITIVE GLB of one `<box>` visual: the bake-side half of the SDF
/// `<pbr><albedo_map>`-on-a-primitive design (the importer captures the texture uri on the
/// [`crate::VisualAssetHint`] side-channel; the baker synthesizes a UV-mapped mesh, bakes this GLB,
/// and the visual becomes an ordinary `<model uri>`; the HCDF document never carries the texture
/// path). `size` is the HCDF box `"sx sy sz"`:
///
///   * exactly one ZERO extent → the single two-triangle QUAD the SDF importer maps a textured
///     `<plane>` to (`"sx sy 0"` → a `+Z`-facing quad centred at the origin, UV 0..1, the b3rb
///     logo-decal case; the other zero-extent axes face `+X`/`+Y` analogously);
///   * all-positive extents → the 24-vertex per-face-UV box (4 vertices per face, each face 0..1).
///
/// The texture embeds VERBATIM (PNG/JPEG, content-magic first, the `name` extension as fallback) under
/// a WHITE `baseColorFactor`: the module-docs factor×texture convention, shared with every textured
/// mesh bake: the primitive's flat diffuse deliberately does NOT tint the albedo map. Deterministic
/// like every writer output (same input → same bytes → same `@sha`). Fails LOUD
/// ([`BakeError::PrimitiveSynth`]) on degenerate extents (fewer than two positive) or a non-embeddable
/// image: the synthesis has no purpose without its texture, so the caller keeps the flat-coloured
/// primitive visual and notes why (other primitive shapes never reach here; the caller notes them as
/// the honest flat-colour tier).
pub fn bake_textured_box(size: &str, texture: &HintTexture) -> Result<BakedAsset, BakeError> {
    let synth_err = |reason: String| BakeError::PrimitiveSynth { reason };
    let toks: Vec<&str> = size.split_whitespace().collect();
    let vals: Vec<f32> = toks.iter().filter_map(|t| t.parse().ok()).collect();
    if toks.len() != 3 || vals.len() != 3 || vals.iter().any(|v| !v.is_finite()) {
        return Err(synth_err(format!(
            "box size {size:?} is not three finite numbers"
        )));
    }
    if vals.iter().any(|&v| v < 0.0) {
        return Err(synth_err(format!(
            "box size {size:?} has a negative extent"
        )));
    }
    if vals.iter().filter(|&&v| v > 0.0).count() < 2 {
        return Err(synth_err(format!(
            "box size {size:?} has no face area (needs at least two positive extents)"
        )));
    }
    let mime = crate::gltf_pack::sniff_image_mime(&texture.bytes)
        .or_else(|| crate::gltf_pack::mime_from_ext(&texture.name))
        .ok_or_else(|| {
            synth_err(format!(
                "texture {:?} is not an embeddable PNG/JPEG image",
                texture.name
            ))
        })?;
    let tex = texture_image(texture.bytes.clone(), mime);

    let h = [vals[0] / 2.0, vals[1] / 2.0, vals[2] / 2.0];
    // Each face: (normal axis, sign, its 4 corners in ±1 unit-cube coords, CCW seen from OUTSIDE,
    // the glTF front face). Fixed order → deterministic bytes.
    const FACES: [(usize, i8, [[i8; 3]; 4]); 6] = [
        (0, 1, [[1, -1, -1], [1, 1, -1], [1, 1, 1], [1, -1, 1]]),
        (0, -1, [[-1, -1, -1], [-1, -1, 1], [-1, 1, 1], [-1, 1, -1]]),
        (1, 1, [[-1, 1, -1], [-1, 1, 1], [1, 1, 1], [1, 1, -1]]),
        (1, -1, [[-1, -1, -1], [1, -1, -1], [1, -1, 1], [-1, -1, 1]]),
        (2, 1, [[-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]]),
        (2, -1, [[-1, -1, -1], [-1, 1, -1], [1, 1, -1], [1, -1, -1]]),
    ];
    // The shared per-corner UV: 0..1 across each face in the glTF top-left convention (on the +Z quad:
    // image (0,0) lands at the (-x,+y) corner, so the texture reads upright looking down the normal
    // with +y up (the gz plane-decal orientation).
    const FACE_UV: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (n_axis, sign, corners) in FACES {
        let (u_axis, v_axis) = match n_axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        // Zero-AREA faces vanish; a zero-extent NORMAL axis collapses the ± faces onto one plane, so
        // keep only the + face (the side the SDF plane's default normal points), yielding the single
        // quad instead of two coincident ones.
        if h[u_axis] == 0.0 || h[v_axis] == 0.0 || (h[n_axis] == 0.0 && sign < 0) {
            continue;
        }
        let base = positions.len() as u32;
        for c in corners {
            positions.push([h[0] * c[0] as f32, h[1] * c[1] as f32, h[2] * c[2] as f32]);
        }
        uvs.extend_from_slice(&FACE_UV);
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    // No NORMAL attribute: the synthesized faces are flat, and the glTF spec mandates computed flat
    // normals when the attribute is absent, same posture as a scaled/STL bake (POSITION-only).
    // A zero-thickness size is a DECAL PLANE: a single one-sided quad that must render double-sided so
    // the logo is not invisible from behind (a gz plane decal is two-sided). A full box (all extents
    // positive) is a closed solid whose back-faces never show, so it stays single-sided.
    let double_sided = vals.contains(&0.0);
    let geom = Geom {
        positions,
        normals: Vec::new(),
        uvs,
        indices,
        color: Some(WHITE),
        metallic: MATTE.0,
        roughness: MATTE.1,
        texture: Some(tex),
        double_sided,
    };
    let data = write_glb(&[geom]);
    let sha = content_sha(&data);
    Ok(BakedAsset {
        data,
        sha,
        ext: "glb",
        notes: Vec::new(),
    })
}

/// Gate the DAE path: without the `bake-dae` feature a `.dae` is [`BakeError::DaeUnsupported`], never a
/// silently-wrong bake. A no-op for non-DAE.
fn gate_dae(fmt: SrcFormat, src_path: &str) -> Result<(), BakeError> {
    if fmt == SrcFormat::Dae && !cfg!(feature = "bake-dae") {
        return Err(BakeError::DaeUnsupported {
            path: src_path.to_string(),
        });
    }
    Ok(())
}

/// Write the baked geometries as a single binary STL (the lean collision format). Deterministic: a
/// fixed 80-byte zero header, the triangle count, then each triangle's facet normal + 3 vertices +
/// attribute-count, in geometry then face order, matching the standard binary-STL layout trimesh
/// emits. The facet normal is recomputed from the (already winding-corrected) vertices so a mirrored
/// mesh stays outward-facing.
fn write_binary_stl(geoms: &[Geom]) -> Vec<u8> {
    let tri_count: usize = geoms.iter().map(|g| g.indices.len() / 3).sum();
    let mut out = Vec::with_capacity(84 + tri_count * 50);
    out.extend_from_slice(&[0u8; 80]); // header (zeros: no generator string, deterministic)
    out.extend_from_slice(&(tri_count as u32).to_le_bytes());
    for g in geoms {
        for tri in g.indices.chunks_exact(3) {
            let a = Vec3::from_array(g.positions[tri[0] as usize]);
            let b = Vec3::from_array(g.positions[tri[1] as usize]);
            let c = Vec3::from_array(g.positions[tri[2] as usize]);
            let n = (b - a).cross(c - a).normalize_or_zero();
            for comp in [n.x, n.y, n.z] {
                out.extend_from_slice(&comp.to_le_bytes());
            }
            for v in [a, b, c] {
                for comp in [v.x, v.y, v.z] {
                    out.extend_from_slice(&comp.to_le_bytes());
                }
            }
            out.extend_from_slice(&0u16.to_le_bytes()); // attribute byte count
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_vec_parses_and_treats_identity_as_none() {
        assert_eq!(scale_vec(None), None);
        assert_eq!(scale_vec(Some("")), None);
        assert_eq!(scale_vec(Some("1 1 1")), None);
        assert_eq!(scale_vec(Some("2")), Some([2.0, 2.0, 2.0]));
        assert_eq!(scale_vec(Some("2 3 4")), Some([2.0, 3.0, 4.0]));
        assert_eq!(scale_vec(Some("1 -1 1")), Some([1.0, -1.0, 1.0]));
    }

    /// Build the diagonal scale matrix the bake loop uses, transform `n` by its normal matrix, and
    /// renormalize, exactly the per-normal step in `load_geoms_impl`.
    fn baked_normal(scale: [f32; 3], n: [f32; 3]) -> Vec3 {
        let m = Mat3::from_cols(
            Vec3::new(scale[0], 0.0, 0.0),
            Vec3::new(0.0, scale[1], 0.0),
            Vec3::new(0.0, 0.0, scale[2]),
        );
        (normal_matrix(m) * Vec3::from_array(n)).normalize_or_zero()
    }

    #[test]
    fn normal_matrix_mirror_reflects_the_mirrored_component() {
        // An axis mirror negates exactly the mirrored component and leaves the others: the reflected
        // surface's outward normal. This is what keeps a mirrored (left-arm) link shaded like its twin.
        let eps = 1e-6;
        assert!(baked_normal([1.0, -1.0, 1.0], [0.0, 1.0, 0.0])
            .abs_diff_eq(Vec3::new(0.0, -1.0, 0.0), eps));
        assert!(baked_normal([1.0, -1.0, 1.0], [1.0, 0.0, 0.0])
            .abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), eps));
        assert!(baked_normal([-1.0, 1.0, 1.0], [1.0, 0.0, 0.0])
            .abs_diff_eq(Vec3::new(-1.0, 0.0, 0.0), eps));
        // A diagonal normal tilts toward the un-mirrored axes but stays unit length.
        let d = baked_normal([1.0, -1.0, 1.0], [1.0, 1.0, 0.0]);
        assert!((d.length() - 1.0).abs() < eps);
        assert!(
            d.x > 0.0 && d.y < 0.0,
            "the mirrored (y) component flips sign: {d:?}"
        );
    }

    #[test]
    fn normal_matrix_nonuniform_scale_uses_inverse_transpose() {
        // Non-uniform scale bends normals by the RECIPROCAL of each axis (inverse-transpose), not the
        // scale itself: an axis-aligned normal is preserved, an off-axis one tilts toward the
        // least-scaled axis. Always unit length after renormalization.
        let eps = 1e-6;
        assert!(baked_normal([2.0, 3.0, 4.0], [0.0, 0.0, 1.0])
            .abs_diff_eq(Vec3::new(0.0, 0.0, 1.0), eps));
        let n = baked_normal([2.0, 3.0, 4.0], [1.0, 1.0, 0.0]);
        assert!(
            (n.length() - 1.0).abs() < eps,
            "renormalized to unit: {n:?}"
        );
        // 1/2 vs 1/3 → the x (less-scaled axis) dominates over y.
        assert!(
            n.x > n.y && n.y > 0.0,
            "tilts toward the least-scaled axis: {n:?}"
        );
    }

    #[test]
    fn png_has_alpha_reads_colour_type_and_trns() {
        // Build a minimal PNG: signature + IHDR (with the given colour type) + any extra chunks + IEND.
        // Only the header bytes and the chunk *types* matter to the probe (it never decodes pixels), so
        // fake CRCs are fine.
        fn chunk(v: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
            v.extend_from_slice(&(data.len() as u32).to_be_bytes());
            v.extend_from_slice(ty);
            v.extend_from_slice(data);
            v.extend_from_slice(&[0, 0, 0, 0]); // CRC placeholder (unused by the probe)
        }
        fn png(colour_type: u8, trns: bool) -> Vec<u8> {
            let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
            let mut ihdr = Vec::new();
            ihdr.extend_from_slice(&1u32.to_be_bytes()); // width
            ihdr.extend_from_slice(&1u32.to_be_bytes()); // height
            ihdr.extend_from_slice(&[8, colour_type, 0, 0, 0]); // bitDepth, colourType, comp, filter, interlace
            chunk(&mut v, b"IHDR", &ihdr);
            if trns {
                chunk(&mut v, b"tRNS", &[0, 0]);
            }
            chunk(&mut v, b"IEND", &[]);
            v
        }
        // Alpha colour types carry alpha outright.
        assert!(png_has_alpha(&png(6, false)), "RGBA (colour type 6)");
        assert!(png_has_alpha(&png(4, false)), "grey+alpha (colour type 4)");
        // Opaque colour types with no tRNS are opaque.
        assert!(!png_has_alpha(&png(2, false)), "RGB (colour type 2)");
        assert!(!png_has_alpha(&png(0, false)), "grey (colour type 0)");
        assert!(
            !png_has_alpha(&png(3, false)),
            "palette (colour type 3), no tRNS"
        );
        // A tRNS chunk makes an otherwise-opaque type transparent.
        assert!(png_has_alpha(&png(3, true)), "palette + tRNS");
        assert!(png_has_alpha(&png(2, true)), "RGB + keyed tRNS");
        // Non-PNG (a JPEG never has alpha) and malformed/truncated bytes read as opaque.
        assert!(
            !png_has_alpha(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]),
            "JPEG magic"
        );
        assert!(!png_has_alpha(b"not a png at all"), "garbage");
        assert!(!png_has_alpha(&[]), "empty");
        assert!(
            !png_has_alpha(&png(6, false)[..20]),
            "truncated before the colour-type byte"
        );
    }

    #[test]
    fn color_vec_matches_python_byte_quantization() {
        // 0.1 -> round(25.5)=26 -> 26/255 (the exact value Python+trimesh produce).
        let c = color_vec(Some("0.8 0.2 0.1 1.0")).unwrap();
        assert!((c[2] - 26.0 / 255.0).abs() < 1e-6, "got {}", c[2]);
        assert_eq!(c[3], 1.0);
        // 3-channel gets alpha 1.0.
        assert_eq!(color_vec(Some("1 0 0")).unwrap()[3], 1.0);
        assert_eq!(color_vec(None), None);
    }

    #[test]
    fn color_vec_uses_banker_rounding_like_python() {
        // Python's `int(round(float(x)*255))` parses each channel as f64 and rounds half-to-EVEN, so a
        // channel landing exactly on N.5 (in f64) with N even rounds DOWN, NOT round-half-away (which
        // would give N+1). These decimal strings are EXACT f64 ties at k.5/255 (e.g. f64("0.00980392…")
        // * 255 == 2.5 precisely); each must match Python's banker's rounding. The nearest output byte b
        // makes `c[0]` == b/255, so b = round(c[0]*255).
        let byte_of = |s: &str| (color_vec(Some(s)).unwrap()[0] * 255.0).round() as i32;
        // 2.5 -> 2 (even; away-from-zero would give 3).
        assert_eq!(
            byte_of("0.00980392156862745 0 0 1"),
            2,
            "2.5 must round to 2 (banker's)"
        );
        // 0.5 -> 0 (even; away gives 1).
        assert_eq!(
            byte_of("0.00196078431372549 0 0 1"),
            0,
            "0.5 must round to 0 (banker's)"
        );
        // 4.5 -> 4 (even; away gives 5).
        assert_eq!(
            byte_of("0.01764705882352941 0 0 1"),
            4,
            "4.5 must round to 4 (banker's)"
        );
        // 3.5 -> 4 (odd tie rounds up under banker's too).
        assert_eq!(
            byte_of("0.013725490196078431 0 0 1"),
            4,
            "3.5 must round to 4 (banker's)"
        );
    }

    #[test]
    fn mirror_determinant_detected() {
        let s = scale_vec(Some("1 -1 1")).unwrap();
        let m = Mat3::from_cols(
            Vec3::new(s[0], 0.0, 0.0),
            Vec3::new(0.0, s[1], 0.0),
            Vec3::new(0.0, 0.0, s[2]),
        );
        assert!(m.determinant() < 0.0, "1 -1 1 is a mirror");
        let s2 = scale_vec(Some("-1 -1 1")).unwrap();
        let m2 = Mat3::from_cols(
            Vec3::new(s2[0], 0.0, 0.0),
            Vec3::new(0.0, s2[1], 0.0),
            Vec3::new(0.0, 0.0, s2[2]),
        );
        assert!(m2.determinant() > 0.0, "two negatives is NOT a mirror");
    }

    // ── DAE orientation: VERIFIED equivalent to the Python/trimesh authority ─────────────────────────
    //
    // GROUND TRUTH (proven 4 ways while building this, all agreeing): the COLLADA `<up_axis>` is IGNORED
    // by BOTH mesh-loader (this baker) AND trimesh (the Python authority that baked ~/openarm.hcdfz); the
    // asset's orientation is instead carried by the `<visual_scene>` node `<matrix>` transforms, which
    // both loaders bake into the vertices. So NO `<up_axis>` rotation is applied here; applying one would
    // double-rotate node-oriented files (e.g. OpenArm's Y_UP `link3.dae`, which ships an `Rx(+90deg)` node
    // matrix) and DIVERGE from the reference. (1) mesh-loader source: `"up_axis" => {}`; (2) trimesh
    // `exchange/dae.py`: no up_axis handling; (3) trimesh 4.10.1 on these synthetic DAEs maps Y_UP/Z_UP
    // identity-node `(1,2,3)->(1,2,3)` and `Rx(+90)`-node `(1,2,3)->(1,-3,2)`; (4) `to_glb` of the real
    // OpenArm `.dae` files reproduces the reference GLB world AABBs exactly (the `#[ignore]` test below).

    /// Build a one-triangle COLLADA with a chosen `<up_axis>` and node `<matrix>` (row-major) over the
    /// fixed verts (1,2,3),(4,5,6),(7,8,9), to isolate up_axis vs node-transform handling.
    #[cfg(feature = "bake-dae")]
    fn synthetic_dae(up_axis: &str, node_matrix: &str) -> Vec<u8> {
        format!(
            r##"<?xml version="1.0"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
 <asset><created>2024-01-01T00:00:00</created><modified>2024-01-01T00:00:00</modified><unit name="meter" meter="1"/><up_axis>{up_axis}</up_axis></asset>
 <library_geometries>
  <geometry id="g" name="g"><mesh>
   <source id="g-pos"><float_array id="g-pos-a" count="9">1 2 3 4 5 6 7 8 9</float_array>
    <technique_common><accessor source="#g-pos-a" count="3" stride="3">
     <param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/>
    </accessor></technique_common></source>
   <vertices id="g-vtx"><input semantic="POSITION" source="#g-pos"/></vertices>
   <triangles count="1"><input semantic="VERTEX" source="#g-vtx" offset="0"/><p>0 1 2</p></triangles>
  </mesh></geometry>
 </library_geometries>
 <library_visual_scenes><visual_scene id="S" name="S">
  <node id="n" name="n" type="NODE">
   <matrix sid="transform">{node_matrix}</matrix>
   <instance_geometry url="#g"/>
  </node>
 </visual_scene></library_visual_scenes>
 <scene><instance_visual_scene url="#S"/></scene>
</COLLADA>
"##
        )
        .into_bytes()
    }

    /// Aggregate the world-space AABB of a baked GLB from every POSITION accessor's `min`/`max`. The
    /// baker writes identity nodes, so the accessor bounds ARE world space; POSITION accessors carry
    /// `min`/`max` while NORMAL accessors (also VEC3) carry none and are skipped.
    #[cfg(feature = "bake-dae")]
    fn glb_vec3_aabb(glb: &[u8]) -> ([f32; 3], [f32; 3]) {
        let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        let mut rest = json;
        while let Some(p) = rest.find("\"type\":\"VEC3\"") {
            let block_end = rest[p..].find('}').map_or(rest.len(), |e| p + e);
            let block = &rest[p..block_end];
            rest = &rest[block_end..];
            let arr = |key: &str| -> Option<[f32; 3]> {
                let i = block.find(key)? + key.len();
                let end = block[i..].find(']')? + i;
                let v: Vec<f32> = block[i..end]
                    .split(',')
                    .filter_map(|t| t.trim().parse().ok())
                    .collect();
                (v.len() == 3).then_some([v[0], v[1], v[2]])
            };
            if let (Some(amn), Some(amx)) = (arr("\"min\":["), arr("\"max\":[")) {
                for k in 0..3 {
                    mn[k] = mn[k].min(amn[k]);
                    mx[k] = mx[k].max(amx[k]);
                }
            }
        }
        (mn, mx)
    }

    /// The `<up_axis>` is IGNORED: two DAEs differing ONLY in `<up_axis>` (identity node) bake to
    /// IDENTICAL geometry: they do NOT differ by an X-rotation. This matches trimesh exactly and refutes
    /// the "Y_UP needs an Rx(+90)" hypothesis (which the openarm.hcdfz reference also refutes).
    #[cfg(feature = "bake-dae")]
    #[test]
    fn dae_up_axis_is_ignored_matching_trimesh() {
        let id = "1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1";
        let yup = to_glb_bytes(&synthetic_dae("Y_UP", id), "y.dae", None, None).unwrap();
        let zup = to_glb_bytes(&synthetic_dae("Z_UP", id), "z.dae", None, None).unwrap();
        let (ymn, ymx) = glb_vec3_aabb(&yup);
        let (zmn, zmx) = glb_vec3_aabb(&zup);
        // Raw float_array frame reproduced unchanged (no up_axis rotation), like trimesh.
        assert_eq!(ymn, [1.0, 2.0, 3.0], "Y_UP min unchanged");
        assert_eq!(ymx, [7.0, 8.0, 9.0], "Y_UP max unchanged");
        // Z_UP is IDENTICAL to Y_UP: up_axis ignored, so they do NOT differ.
        assert_eq!(
            ymn, zmn,
            "Z_UP and Y_UP must be identical (up_axis ignored)"
        );
        assert_eq!(
            ymx, zmx,
            "Z_UP and Y_UP must be identical (up_axis ignored)"
        );
    }

    /// Orientation comes from the node `<matrix>`, NOT `<up_axis>`: an `Rx(+90deg)` node matrix
    /// `(x,y,z)->(x,-z,y)` (what a Blender Y-up export carries, e.g. OpenArm `link3.dae`) IS baked into
    /// the vertices, mapping `(1,2,3)->(1,-3,2)` exactly as trimesh 4.10.1 does.
    #[cfg(feature = "bake-dae")]
    #[test]
    fn dae_node_transform_carries_orientation() {
        let rx90 = "1 0 0 0 0 0 -1 0 0 1 0 0 0 0 0 1"; // row-major COLLADA matrix
        let glb = to_glb_bytes(&synthetic_dae("Y_UP", rx90), "y.dae", None, None).unwrap();
        let (mn, mx) = glb_vec3_aabb(&glb);
        // (1,2,3),(4,5,6),(7,8,9) -> (1,-3,2),(4,-6,5),(7,-9,8): min (1,-9,2), max (7,-3,8).
        assert_eq!(mn, [1.0, -9.0, 2.0], "Rx(+90) node baked into min");
        assert_eq!(mx, [7.0, -3.0, 8.0], "Rx(+90) node baked into max");
    }

    /// Parity vs the canonical trimesh bake: `to_glb` of the source `.dae` reproduces the world-space
    /// AABBs a live `trimesh` (force=scene) bake produces, with NO up_axis rotation, for a Z_UP source
    /// (`body_link0.dae`, with the URDF mesh scale 0.001 folded in) AND a Y_UP source (`link3.dae`,
    /// oriented by its node matrix). The numbers are the TRIMESH-CORRECT AABBs (verified vs trimesh
    /// 4.10.1): body_link0 happens to match the ~/openarm.hcdfz body GLB, but the bundle's ARM-link GLBs
    /// are STALE (an Rx(+90)-rotated older orientation) and deliberately NOT used as the oracle here.
    /// `#[ignore]`: needs ~/git/openarm_description on disk.
    #[cfg(feature = "bake-dae")]
    #[test]
    #[ignore = "needs ~/git/openarm_description on disk; AABBs verified vs live trimesh 4.10.1"]
    fn bake_dae_matches_openarm_reference() {
        struct RefCase {
            rel: &'static str,
            scale: Option<&'static str>,
            mn: [f32; 3],
            mx: [f32; 3],
        }
        let home = std::env::var("HOME").unwrap();
        let base = format!("{home}/git/openarm_description/assets/robot/openarm_v2.0/meshes");
        let cases = [
            RefCase {
                rel: "body/visual/body_link0.dae",
                scale: Some("0.001 0.001 0.001"),
                mn: [-0.155, -0.095, 0.0],
                mx: [0.095, 0.095, 0.773],
            },
            RefCase {
                rel: "arm/visual/link3.dae",
                scale: None,
                mn: [-0.042510, -0.032524, -0.182248],
                mx: [0.034825, 0.041000, 0.000251],
            },
        ];
        for c in &cases {
            let glb = to_glb(&format!("{base}/{}", c.rel), c.scale, None).unwrap();
            let (mn, mx) = glb_vec3_aabb(&glb);
            for k in 0..3 {
                assert!(
                    (mn[k] - c.mn[k]).abs() < 1e-4,
                    "{} min[{k}]: {} vs ref {}",
                    c.rel,
                    mn[k],
                    c.mn[k]
                );
                assert!(
                    (mx[k] - c.mx[k]).abs() < 1e-4,
                    "{} max[{k}]: {} vs ref {}",
                    c.rel,
                    mx[k],
                    c.mx[k]
                );
            }
        }
    }

    /// Collect every material's `baseColorFactor` red channel from a baked GLB's JSON chunk.
    #[cfg(feature = "bake-dae")]
    fn glb_material_reds(glb: &[u8]) -> Vec<f32> {
        let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        let key = "\"baseColorFactor\":[";
        let mut reds = Vec::new();
        let mut rest = json;
        while let Some(p) = rest.find(key) {
            rest = &rest[p + key.len()..];
            let end = rest.find(']').unwrap();
            let first = rest[..end].split(',').next().unwrap().trim();
            reds.push(first.parse().unwrap());
        }
        reds
    }

    /// PROOF that the full-Rust DAE bake now carries FAITHFUL per-mesh materials (the whole point of the
    /// dae-parser rewrite): baking the real OpenArm `body_link0.dae` yields a GLB whose materials' base
    /// colours match the Python reference. The file has 7 triangle-sets bound to 6 distinct Lambert
    /// materials (one is reused), so after the writer's deterministic dedup the GLB carries EXACTLY 6
    /// materials, whose diffuse-red channels are {0.247, 0.627, 0.094, 0.796, 0.957, 0.561}. A
    /// mesh-loader bake produced ZERO materials (gray), so a non-empty, correctly-coloured material set
    /// is the regression guard. `#[ignore]`: needs ~/git/openarm_description on disk.
    #[cfg(feature = "bake-dae")]
    #[test]
    #[ignore = "needs ~/git/openarm_description on disk; proves DAE per-mesh materials are carried"]
    fn bake_dae_carries_body_link0_materials() {
        let home = std::env::var("HOME").unwrap();
        let path = format!(
            "{home}/git/openarm_description/assets/robot/openarm_v2.0/meshes/body/visual/body_link0.dae"
        );
        let glb = to_glb(&path, Some("0.001 0.001 0.001"), None).unwrap();
        let reds = glb_material_reds(&glb);
        assert!(
            !reds.is_empty(),
            "DAE bake is MATERIAL-LESS (gray); materials were not carried"
        );
        assert_eq!(
            reds.len(),
            6,
            "expected 6 deduplicated materials, got {}: {:?}",
            reds.len(),
            reds
        );
        let expected = [0.247_f32, 0.627, 0.094, 0.796, 0.957, 0.561];
        for e in expected {
            assert!(
                reds.iter().any(|&r| (r - e).abs() < 1e-3),
                "expected a material with baseColor red ~{e}, got {reds:?}"
            );
        }
    }
}
