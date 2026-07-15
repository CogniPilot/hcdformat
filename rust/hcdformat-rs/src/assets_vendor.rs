//! Asset vendoring: a Rust port of `hcdf/assets.py`'s `resolve_uri` + `vendor_assets` (and the
//! `bake` content-addressing they rely on), matched function-for-function.
//!
//! `vendor_assets` RELOCATES every visual `<model uri>` and collision `<geometry><mesh uri>` in a
//! document into one `assets/` directory and rewrites the DOM to point at the document-relative,
//! content-addressed copy: the self-containment step a bundle needs. It is the dual of include
//! flattening: flatten merges sub-documents, vendor pulls in the geometry.
//!
//! ## Scope vs. Python `bake`
//!
//! In the vendor path Python always calls `bake(path, scale=None, kind, out_dir)` (no scale, no
//! color). With `scale=None`:
//!
//!   * a **visual** `<model>` is GLB-only by the schema grammar (a visual mesh is the GLB `<model>`,
//!     enforced by `visual_geometry` having no `<mesh>`), so the GLB-passthrough branch always runs:
//!     read the source bytes verbatim, keep its extension, content-address, copy. No trimesh.
//!   * a **collision** `<mesh>` stays LEAN and `scale=None` means pass-through too: read verbatim,
//!     keep the source extension, content-address, copy. No trimesh.
//!
//! So vendoring a VALID HCDF document is a pure byte-copy + hash in both languages: identical asset
//! names and `@sha`. The trimesh `to_glb`/`to_lean` conversion branches only ever fire during
//! URDF/SDF IMPORT (the `Baker`), never in `vendor_assets`, so they are intentionally out of scope for
//! this Rust port. If a non-GLB visual source with a needed scale ever reached here, [`bake`] reports
//! a `needs-conversion` status (left unvendored) rather than silently mis-handling it.
//!
//! ## `.gltf` visual sources: PACKED to one GLB, not byte-copied (documented divergence from Python)
//!
//! Whether a visual source is "already a GLB" is decided by CONTENT, the 4-byte `glTF` magic
//! ([`crate::gltf_pack::is_glb_bytes`]), never by extension. Real GLB bytes take the verbatim
//! byte-copy path above (unchanged, byte-identical to Python). A JSON `.gltf`, however, keeps its
//! geometry in EXTERNAL companion files (`buffers[].uri = "CHASSIS.bin"`, texture images), and the
//! content-hash rename severs those RELATIVE links; Python byte-copies it anyway and ships a broken
//! asset. This port instead PACKS it into a self-contained GLB first
//! ([`crate::gltf_pack::pack_gltf_to_glb`], companions resolved from the source's own directory), so
//! the vendored asset is `<stem>_<short12>.glb` over the PACKED bytes. A companion that cannot be
//! resolved/inlined is a LOUD error (never a silently-broken GLB in a bundle).
//!
//! ## Remote URIs
//!
//! A `http(s)://` mesh is the integrator's bundle-time choice, mirroring Python `_vendor_one`:
//! `vendor_remote=false` (default) leaves it UNCHANGED with `status = "remote"` (caller picks
//! keep-live vs error); `vendor_remote=true` fetches it via the feature-gated hardened fetcher
//! (`remote::fetch_remote`, passing the element's `@sha` as `expect_sha`) and then vendors it
//! exactly like a local mesh. A genuinely unresolvable LOCAL uri is `status = "unresolved"` and left
//! unchanged. Building without the `remote` feature, a `vendor_remote=true` request errors (there is no
//! fetcher compiled in), so you cannot accidentally get a silent no-op.

#![cfg(not(target_arch = "wasm32"))]

use crate::compose::content_sha;
use crate::model::{Hcdf, VisualAppearance};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Hex chars of the content hash kept in a vendored filename (`hcdf.assets._SHORT_SHA`).
const SHORT_SHA: usize = 12;
/// Mesh-uri scheme prefixes that resolve identically from any base dir (`hcdf_io.remote` allow-list).
const REMOTE_SCHEMES: [&str; 2] = ["http://", "https://"];

/// Whether a visual model or a collision mesh is being vendored (drives the bake/pass-through branch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// A visual `<model>` (GLB-only by schema; passed through unchanged when already a GLB).
    Visual,
    /// A collision `<mesh>` (lean; passed through unchanged with no scale).
    Collision,
}

impl AssetKind {
    fn label(self) -> &'static str {
        match self {
            AssetKind::Visual => "visual-model",
            AssetKind::Collision => "collision-mesh",
        }
    }
}

/// The disposition of one vendored asset (parity with the Python manifest `status` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorStatus {
    /// Resolved + copied into `assets/` and the DOM rewritten (`@uri`/`@sha` updated).
    Vendored,
    /// A live `http(s)://` uri left untouched (caller decides keep-live vs error). `vendor_remote=false`.
    Remote,
    /// A local uri that could not be resolved to an existing file; left untouched.
    Unresolved,
    /// A non-GLB visual source that would need trimesh conversion (never hit by a valid HCDF doc).
    NeedsConversion,
}

impl VendorStatus {
    /// The Python manifest string for this status (`"vendored"|"remote"|"unresolved"`).
    pub fn as_str(self) -> &'static str {
        match self {
            VendorStatus::Vendored => "vendored",
            VendorStatus::Remote => "remote",
            VendorStatus::Unresolved => "unresolved",
            VendorStatus::NeedsConversion => "needs-conversion",
        }
    }
}

/// One row of the vendor manifest, mirroring Python's
/// `{site, kind, old_uri, new_uri, sha, status}`.
#[derive(Debug, Clone, PartialEq)]
pub struct VendorEntry {
    pub site: String,
    pub kind: &'static str,
    pub old_uri: String,
    pub new_uri: String,
    pub sha: Option<String>,
    pub status: VendorStatus,
}

/// True iff `uri` is an `http(s)://` URL: the only schemes the `remote` fetcher fetches.
/// Mirrors `hcdf_io.remote.is_remote_uri`.
pub fn is_remote_uri(uri: &str) -> bool {
    let low = uri.to_ascii_lowercase();
    REMOTE_SCHEMES.iter().any(|s| low.starts_with(s))
}

/// Gazebo model search directories from the standard env vars (GZ first, then legacy). Mirrors
/// `hcdf.assets._gz_resource_dirs`.
fn gz_resource_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for var in [
        "GZ_SIM_RESOURCE_PATH",
        "IGN_GAZEBO_RESOURCE_PATH",
        "GAZEBO_MODEL_PATH",
    ] {
        if let Some(val) = std::env::var_os(var) {
            for p in std::env::split_paths(&val) {
                if !p.as_os_str().is_empty() {
                    dirs.push(p);
                }
            }
        }
    }
    dirs
}

/// Resolve a mesh URI to an existing filesystem path, or `None`. Port of `hcdf.assets.resolve_uri`:
/// supports `file://`, `package://<pkg>/<rest>` (via `package_paths`), `model://<model>/<rest>`
/// (Gazebo: `package_paths` keyed by model name, then the GZ/Gazebo resource-path env vars), absolute
/// paths, and paths relative to `base_dir`. Returns the path ONLY if it exists (matching Python).
pub fn resolve_uri(
    uri: &str,
    base_dir: &Path,
    package_paths: &BTreeMap<String, PathBuf>,
) -> Option<PathBuf> {
    if uri.is_empty() {
        return None;
    }
    let path: PathBuf = if let Some(rest) = uri.strip_prefix("file://") {
        PathBuf::from(rest)
    } else if let Some(rest) = uri.strip_prefix("package://") {
        let (pkg, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let root = package_paths.get(pkg)?;
        root.join(tail)
    } else if let Some(rest) = uri.strip_prefix("model://") {
        let (model, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if let Some(root) = package_paths.get(model) {
            root.join(tail)
        } else {
            for base in gz_resource_dirs() {
                let cand = normpath(&base.join(model).join(tail));
                if cand.exists() {
                    return Some(cand);
                }
            }
            return None;
        }
    } else if Path::new(uri).is_absolute() {
        PathBuf::from(uri)
    } else {
        base_dir.join(uri)
    };
    let path = normpath(&path);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Lexical `os.path.normpath`: collapse `.`/`..` without touching the filesystem (keeps relative
/// paths relative, unlike `compose::normalize` which absolutizes; `resolve_uri` only needs `.`/`..`
/// folding before the `exists()` check, matching Python `os.path.normpath`).
fn normpath(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// A filesystem-safe stem from a source mesh path: its basename without the extension, with any char
/// outside `[0-9A-Za-z._-]` replaced by `_` (mirrors `hcdf.assets._safe_stem`). Cosmetic only, never
/// feeds the hash.
fn safe_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "mesh".to_string()
    } else {
        safe
    }
}

/// The source file's extension WITHOUT the dot (`""` if none), case PRESERVED, exactly
/// `os.path.splitext(src_path)[1].lstrip(".")`. The baked filename (`<stem>_<short12>.<ext>`) and the
/// rerooted `@uri` carry this verbatim, so a source `Rotor.STL` vendors to `Rotor_<sha>.STL` byte-for-byte
/// like Python (NOT lowercased): lowercasing here would diverge the asset entry name + `@uri` from a
/// Python-packed bundle for any mixed-case extension (.GLB/.STL/.DAE/.Obj, routine in URDF/Gazebo trees
/// and Windows/macOS exports), defeating cross-tool parity. GLB/GLTF *detection* lowercases a COPY at the
/// call site, so the visual passthrough guard stays case-insensitive while the WRITTEN ext keeps its case.
fn ext_no_dot(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Result of baking one mesh into the asset store: the vendored filename and its content sha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baked {
    pub name: String,
    pub sha: String,
}

/// The pure (no-write) half of [`bake`]: the content-addressed `<safe_stem>_<short12sha>.<ext>` name +
/// `@sha` for `data` read from `src_path`. Split out so the zip pack path can address the bytes it
/// already holds in memory and hand them straight to the zip writer (no staging-dir round-trip).
fn baked_name(src_path: &Path, kind: AssetKind, data: &[u8]) -> Baked {
    let ext = match kind {
        AssetKind::Visual => {
            // visual passthrough keeps the source's extension verbatim, case PRESERVED (.GLB stays .GLB,
            // matching Python's `os.path.splitext()[1].lstrip(".")`)
            ext_no_dot(src_path)
        }
        AssetKind::Collision => {
            // lean: keep the source extension (case preserved), or the lowercase literal "stl" when
            // unknown, exactly Python's `os.path.splitext()[1].lstrip(".") or "stl"`
            let e = ext_no_dot(src_path);
            if e.is_empty() {
                "stl".to_string()
            } else {
                e
            }
        }
    };
    let sha = content_sha(data);
    let hexpart = sha.split_once(':').map_or(sha.as_str(), |(_, h)| h);
    let short = &hexpart[..SHORT_SHA.min(hexpart.len())];
    let stem = safe_stem(src_path);
    let name = if ext.is_empty() {
        format!("{stem}_{short}")
    } else {
        format!("{stem}_{short}.{ext}")
    };
    Baked { name, sha }
}

/// Content-address `src_path` into `out_dir` and return `(filename, sha)`. Port of `hcdf.assets.bake`
/// for the VENDOR case (`scale=None`, `color=None`): a visual GLB (magic-checked) and a lean collision
/// mesh are passed through verbatim (read raw bytes, keep the source extension), hashed, and copied to
/// `<safe_stem>_<short12sha>.<ext>`. Identical content always yields the same name + `@sha` (so two
/// sites that share a mesh dedup to one file), byte-for-byte matching what Python writes.
///
/// A VISUAL source WITHOUT the GLB magic is treated as a `.gltf` JSON document and PACKED into a
/// self-contained GLB first (external buffers/images resolved next to `src_path` and inlined; the
/// written name/ext become `<stem>_<short12>.glb` over the packed bytes): the documented divergence
/// from Python's byte-copy, which severs the `.gltf → .bin` links (see the module docs). A magic-less
/// `.gltf` COLLISION is packed the same way ([`canonical_collision_mesh`]); every other lean collision
/// format (STL/DAE/OBJ/existing GLB) still passes through verbatim.
///
/// Returns `Err` if `src_path` cannot be read, or (`ErrorKind::InvalidData`) if a magic-less visual or
/// a `.gltf` collision cannot be packed (unparseable JSON / missing companion): never a silently-broken
/// vendored asset.
/// A visual whose source is not GLB/glTF at all (which a valid HCDF never has; see the module docs)
/// would need trimesh; we do NOT silently misconvert it, the caller maps that to
/// [`VendorStatus::NeedsConversion`].
pub fn bake(src_path: &Path, kind: AssetKind, out_dir: &Path) -> std::io::Result<Baked> {
    let data = std::fs::read(src_path)?;
    let (src_for_name, data) = match kind {
        AssetKind::Visual => canonical_visual_glb(src_path, data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
        AssetKind::Collision => canonical_collision_mesh(src_path, data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
    };
    let baked = baked_name(&src_for_name, kind, &data);
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join(&baked.name), &data)?;
    Ok(baked)
}

/// Canonicalize a VISUAL source to SELF-CONTAINED GLB bytes, deciding by CONTENT (the `glTF` magic),
/// never by extension: real GLB bytes pass through verbatim (the byte-copy parity path, a binary-bodied
/// `.gltf` counts), anything else must be a `.gltf` JSON document and is PACKED into one GLB, every
/// external `buffer`/`image` companion resolved RELATIVE TO THE SOURCE's own directory (exactly how the
/// authoring tool laid them out) and inlined, so the content-hash rename cannot sever the links.
/// Returns the effective source path (extension swapped to `.glb` on a pack, so the vendored name says
/// what the bytes now are) plus the canonical bytes. Errors are loud and name the source + offending
/// references, never a broken GLB.
fn canonical_visual_glb(src_path: &Path, data: Vec<u8>) -> Result<(PathBuf, Vec<u8>), String> {
    if crate::gltf_pack::is_glb_bytes(&data) {
        return Ok((src_path.to_path_buf(), data));
    }
    let base = src_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let packed = crate::gltf_pack::pack_gltf_to_glb(&data, |uri: &str| {
        std::fs::read(normpath(&base.join(uri))).ok()
    })
    .map_err(|e| {
        format!(
            "visual mesh {} is not a GLB (no glTF magic) and could not be packed from .gltf: {e}",
            src_path.display()
        )
    })?;
    Ok((src_path.with_extension("glb"), packed))
}

/// Canonicalize a COLLISION source, packing ONLY the `.gltf` byte-copy case.
///
/// A collision mesh is LEAN and stays whatever lean format it already is: an existing GLB (magic-
/// checked), a binary/ASCII STL, a DAE, an OBJ all pass through byte-for-byte (the Python parity
/// path, unchanged). The ONE broken case is a JSON `.gltf` collision: its geometry lives in EXTERNAL
/// companion files (`buffers[].uri = "hull.bin"`, textures) and the content-hash rename severs those
/// relative links (Python byte-copies it anyway and ships a dangling asset). Exactly as the visual
/// lane does ([`canonical_visual_glb`]), we PACK it into one self-contained GLB first (companions
/// resolved from the source's own directory and inlined) so the vendored asset is
/// `<stem>_<short12>.glb` over the packed bytes and cannot dangle. A companion that cannot be
/// resolved/inlined is a LOUD error (never a silently-broken GLB in a bundle).
///
/// The pack decision is CONTENT-first then extension: real GLB bytes short-circuit to passthrough (a
/// binary-bodied `.gltf` is already self-contained), and only a magic-less source whose extension is
/// `.gltf` (case-insensitive) is packed; an STL/DAE/OBJ is never fed to the glTF packer.
fn canonical_collision_mesh(src_path: &Path, data: Vec<u8>) -> Result<(PathBuf, Vec<u8>), String> {
    if crate::gltf_pack::is_glb_bytes(&data) || !ext_no_dot(src_path).eq_ignore_ascii_case("gltf") {
        return Ok((src_path.to_path_buf(), data));
    }
    let base = src_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let packed = crate::gltf_pack::pack_gltf_to_glb(&data, |uri: &str| {
        std::fs::read(normpath(&base.join(uri))).ok()
    })
    .map_err(|e| {
        format!(
            "collision mesh {} is a .gltf but could not be packed to a self-contained GLB: {e}",
            src_path.display()
        )
    })?;
    Ok((src_path.with_extension("glb"), packed))
}

/// Copy every referenced mesh into `assets_out_dir` and rewrite the DOM to point at it. Port of
/// `hcdf.assets.vendor_assets`.
///
/// For each visual `<model uri>` and collision `<geometry><mesh uri>` in `doc.comp`: a LOCAL uri is
/// [`resolve_uri`]-resolved then [`bake`]-copied into `assets_out_dir`, and the element's `@uri` is
/// rewritten to `"<uri_prefix>/<name>"` with `@sha` set; a collision keeps/seeds its `@source-uri`
/// (but a vendored REMOTE mesh does NOT, so a `--vendor-remote` bundle carries no http uri). A
/// `http(s)://` uri is handled per `vendor_remote` (see module docs). Returns the manifest.
///
/// `vendor_remote=true` requires the `remote` feature (else `Err`). `cache_dir` is the
/// content-addressed cache for fetched remote bytes.
pub fn vendor_assets(
    doc: &mut Hcdf,
    base_dir: &Path,
    assets_out_dir: &Path,
    package_paths: &BTreeMap<String, PathBuf>,
    uri_prefix: &str,
    vendor_remote: bool,
    cache_dir: Option<&Path>,
) -> Result<Vec<VendorEntry>, String> {
    let mut ctx = VendorCtx {
        base_dir,
        sink: VendorSink::Dir(assets_out_dir),
        package_paths,
        prefix: uri_prefix.trim_end_matches('/'),
        vendor_remote,
        cache_dir,
    };
    vendor_all(doc, &mut ctx)
}

/// [`vendor_assets`] with an IN-MEMORY sink: the vendored bytes land in `assets_out` (content-addressed
/// name -> bytes) instead of a directory, so `bundle::pack`'s zip path can feed them straight to the zip
/// writer, with no staging-dir write + read-back. Same resolution, rewrite, and manifest as the dir variant.
pub(crate) fn vendor_assets_mem(
    doc: &mut Hcdf,
    base_dir: &Path,
    assets_out: &mut BTreeMap<String, Vec<u8>>,
    package_paths: &BTreeMap<String, PathBuf>,
    uri_prefix: &str,
    vendor_remote: bool,
    cache_dir: Option<&Path>,
) -> Result<Vec<VendorEntry>, String> {
    let mut ctx = VendorCtx {
        base_dir,
        sink: VendorSink::Mem(assets_out),
        package_paths,
        prefix: uri_prefix.trim_end_matches('/'),
        vendor_remote,
        cache_dir,
    };
    vendor_all(doc, &mut ctx)
}

/// The shared vendor walk behind [`vendor_assets`] / [`vendor_assets_mem`]: every visual `<model>` and
/// collision `<mesh>` site in document order, each vendored through the ctx's sink.
fn vendor_all(doc: &mut Hcdf, ctx: &mut VendorCtx<'_>) -> Result<Vec<VendorEntry>, String> {
    let mut manifest = Vec::new();

    // Collect indices first so the per-element rewrite can re-borrow `doc` mutably without aliasing.
    let comp_count = doc.comp.len();
    for ci in 0..comp_count {
        let comp_name = doc.comp[ci].name.clone();

        // Visual <model> uris.
        let vis_count = doc.comp[ci].visual.len();
        for vi in 0..vis_count {
            let (vname, model_uri, expect_sha) = {
                let v = &doc.comp[ci].visual[vi];
                match &v.appearance {
                    VisualAppearance::Model { model, .. } => {
                        (v.name.clone(), model.uri.clone(), model.sha.clone())
                    }
                    VisualAppearance::Primitive { .. } => (v.name.clone(), None, None),
                }
            };
            if let Some(uri) = model_uri.filter(|u| !u.is_empty()) {
                let site = format!("{comp_name}/{vname}");
                let entry = vendor_one(
                    ctx,
                    AssetKind::Visual,
                    &uri,
                    expect_sha.as_deref(),
                    &site,
                    |doc, new_uri, sha| {
                        if let VisualAppearance::Model { model, .. } =
                            &mut doc.comp[ci].visual[vi].appearance
                        {
                            model.uri = Some(new_uri.to_string());
                            model.sha = Some(sha.to_string());
                        }
                    },
                    doc,
                )?;
                manifest.push(entry);
            }
        }

        // Collision <mesh> uris.
        let col_count = doc.comp[ci].collision.len();
        for col_i in 0..col_count {
            let (cname, mesh_uri, expect_sha) = {
                let c = &doc.comp[ci].collision[col_i];
                let mesh = c.geometry.as_ref().and_then(|g| g.mesh.as_ref());
                (
                    c.name.clone().unwrap_or_default(),
                    mesh.and_then(|m| m.uri.clone()),
                    mesh.and_then(|m| m.sha.clone()),
                )
            };
            if let Some(uri) = mesh_uri.filter(|u| !u.is_empty()) {
                let site = format!("{comp_name}/{cname}");
                let old_uri = uri.clone();
                let entry = vendor_one(
                    ctx,
                    AssetKind::Collision,
                    &uri,
                    expect_sha.as_deref(),
                    &site,
                    |doc, new_uri, sha| {
                        if let Some(mesh) = doc.comp[ci].collision[col_i]
                            .geometry
                            .as_mut()
                            .and_then(|g| g.mesh.as_mut())
                        {
                            // Breadcrumb to the origin for a LOCAL collision (seed @source-uri if
                            // unset); NOT for a vendored remote mesh (a --vendor-remote bundle must
                            // carry no http uri).
                            if !is_remote_uri(&old_uri) && mesh.source_uri.is_none() {
                                mesh.source_uri = Some(old_uri.clone());
                            }
                            mesh.uri = Some(new_uri.to_string());
                            mesh.sha = Some(sha.to_string());
                        }
                    },
                    doc,
                )?;
                manifest.push(entry);
            }
        }
    }
    Ok(manifest)
}

/// Where the vendored bytes land: the `assets/` directory on disk (loose-dir bundles + the `--dir`
/// staging path) or an in-memory name-keyed map (the zip path, where the bytes feed `write_stored` directly).
pub(crate) enum VendorSink<'a> {
    Dir(&'a Path),
    Mem(&'a mut BTreeMap<String, Vec<u8>>),
}

impl VendorSink<'_> {
    /// Land one vendored blob under its content-addressed `name`. Identical content re-lands the same
    /// name (a byte-identical overwrite either way), so dedup falls out order-independently.
    fn write(&mut self, name: &str, data: Vec<u8>) -> std::io::Result<()> {
        match self {
            VendorSink::Dir(dir) => {
                std::fs::create_dir_all(*dir)?;
                std::fs::write(dir.join(name), &data)
            }
            VendorSink::Mem(map) => {
                map.insert(name.to_string(), data);
                Ok(())
            }
        }
    }
}

/// The shared vendor configuration, passed once so each per-asset call takes few arguments (and the
/// `vendor_one` worker needs no `#[allow(clippy::too_many_arguments)]`).
struct VendorCtx<'a> {
    base_dir: &'a Path,
    sink: VendorSink<'a>,
    package_paths: &'a BTreeMap<String, PathBuf>,
    prefix: &'a str,
    vendor_remote: bool,
    cache_dir: Option<&'a Path>,
}

enum ResolveOutcome {
    /// A resolved local OR a fetched-remote source path to vendor.
    Path(PathBuf),
    /// Skip with this manifest entry (remote-not-vendored / unresolved-local).
    Skip(VendorEntry),
}

/// Vendor one mesh: resolve its source (local or fetched-remote), land it content-addressed in the
/// ctx's sink, and call `rewrite(doc, new_uri, sha)` to update the DOM element in place. Mirrors
/// `hcdf.assets._vendor_one`; `rewrite` is the element-specific patch (visual model vs collision mesh).
fn vendor_one<F>(
    ctx: &mut VendorCtx<'_>,
    kind: AssetKind,
    old_uri: &str,
    expect_sha: Option<&str>,
    site: &str,
    rewrite: F,
    doc: &mut Hcdf,
) -> Result<VendorEntry, String>
where
    F: FnOnce(&mut Hcdf, &str, &str),
{
    let path = match resolve_source(ctx, kind, old_uri, expect_sha, site)? {
        ResolveOutcome::Skip(e) => return Ok(e),
        ResolveOutcome::Path(p) => p,
    };
    // A valid HCDF visual is GLB-only; guard the (unreachable for valid docs) non-GLB case rather than
    // misconvert without trimesh. Detection is case-INSENSITIVE (a lowercased COPY), mirroring Python's
    // `src_path.lower().endswith(_GLB_EXT)`, while bake() keeps the source's original-case extension.
    if kind == AssetKind::Visual {
        let ext = ext_no_dot(&path).to_ascii_lowercase();
        if ext != "glb" && ext != "gltf" {
            return Ok(skip_entry(
                kind,
                old_uri,
                site,
                VendorStatus::NeedsConversion,
            ));
        }
    }
    let copy_err = |e: std::io::Error| {
        format!(
            "vendor {} {site}: could not copy {} into assets: {e}",
            kind.label(),
            path.display()
        )
    };
    let data = std::fs::read(&path).map_err(&copy_err)?;
    // A visual is vendored as ONE self-contained GLB; a collision keeps its lean format EXCEPT a `.gltf`
    // collision, which is likewise packed. In both lanes real GLB bytes (magic-checked) pass verbatim and
    // a `.gltf` JSON is packed with its external companions inlined (see [`canonical_visual_glb`] /
    // [`canonical_collision_mesh`]); byte-copying a `.gltf` would sever its relative `.bin`/texture links
    // under the content-hash rename. STL/DAE/OBJ/existing-GLB collisions are byte-copied unchanged.
    let (src_for_name, data) = if kind == AssetKind::Visual {
        canonical_visual_glb(&path, data)
            .map_err(|e| format!("vendor {} {site}: {e}", kind.label()))?
    } else {
        canonical_collision_mesh(&path, data)
            .map_err(|e| format!("vendor {} {site}: {e}", kind.label()))?
    };
    let baked = baked_name(&src_for_name, kind, &data);
    ctx.sink.write(&baked.name, data).map_err(&copy_err)?;
    let new_uri = join_prefix(ctx.prefix, &baked.name);
    rewrite(doc, &new_uri, &baked.sha);
    Ok(VendorEntry {
        site: site.to_string(),
        kind: kind.label(),
        old_uri: old_uri.to_string(),
        new_uri,
        sha: Some(baked.sha),
        status: VendorStatus::Vendored,
    })
}

/// Resolve one asset's source path: a remote uri is fetched (when `vendor_remote`) else flagged
/// `Remote`; a local uri is `resolve_uri`-resolved. `Err` only on a remote fetch failure (loud).
fn resolve_source(
    ctx: &VendorCtx<'_>,
    kind: AssetKind,
    old_uri: &str,
    expect_sha: Option<&str>,
    site: &str,
) -> Result<ResolveOutcome, String> {
    if is_remote_uri(old_uri) {
        if !ctx.vendor_remote {
            return Ok(ResolveOutcome::Skip(skip_entry(
                kind,
                old_uri,
                site,
                VendorStatus::Remote,
            )));
        }
        // Fetch via the hardened fetcher (feature-gated). A failed fetch is a loud Err (the integrator
        // asked to embed it), mirroring `_fetch_remote_mesh`.
        let path = fetch_remote_mesh(old_uri, expect_sha, ctx.cache_dir)?;
        Ok(ResolveOutcome::Path(path))
    } else {
        match resolve_uri(old_uri, ctx.base_dir, ctx.package_paths) {
            Some(p) => Ok(ResolveOutcome::Path(p)),
            None => Ok(ResolveOutcome::Skip(skip_entry(
                kind,
                old_uri,
                site,
                VendorStatus::Unresolved,
            ))),
        }
    }
}

/// A manifest entry for an asset left UNCHANGED (remote-not-vendored / unresolved / needs-conversion).
fn skip_entry(kind: AssetKind, old_uri: &str, site: &str, status: VendorStatus) -> VendorEntry {
    VendorEntry {
        site: site.to_string(),
        kind: kind.label(),
        old_uri: old_uri.to_string(),
        new_uri: old_uri.to_string(),
        sha: None,
        status,
    }
}

/// `"<prefix>/<name>"` when a prefix is set, else just `name` (mirrors the Python join).
fn join_prefix(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// Fetch a remote mesh to a local cache path, verifying `@sha` when present. Mirrors
/// `hcdf.assets._fetch_remote_mesh`. Requires the `remote` feature; without it, a `vendor_remote=true`
/// request is a clear error (no silent no-op).
#[cfg(feature = "remote")]
fn fetch_remote_mesh(
    uri: &str,
    expect_sha: Option<&str>,
    cache_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    crate::remote::fetch_remote(uri, expect_sha, cache_dir, None, None).map_err(|e| e.to_string())
}

#[cfg(not(feature = "remote"))]
fn fetch_remote_mesh(
    uri: &str,
    _expect_sha: Option<&str>,
    _cache_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    Err(format!(
        "cannot vendor remote asset {uri:?}: this build of hcdformat was compiled without the \
         'remote' feature (the http(s) fetcher is not available). Rebuild with --features remote, \
         or choose keep-remote/error for remote assets."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d =
            std::env::temp_dir().join(format!("hcdf-bake-{tag}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The vendored filename + extension must PRESERVE the source extension's case exactly like Python's
    /// `os.path.splitext(src)[1].lstrip(".")`: a `Chassis.GLB` visual bakes to `Chassis_<sha>.GLB`, a
    /// `Rotor.STL` collision to `Rotor_<sha>.STL` (NOT `.glb`/`.stl`). Lowercasing here was the parity
    /// break: a Rust- vs Python-packed bundle for the same input would carry different asset names + @uri.
    /// (The visual fixture bytes START WITH THE GLB MAGIC: the passthrough is magic-checked; a magic-less
    /// visual is treated as a `.gltf` to pack, not blindly byte-copied.)
    #[test]
    fn bake_preserves_source_extension_case() {
        let d = tmp_dir("ext-case");
        let out = d.join("assets");

        let glb = d.join("Chassis.GLB");
        std::fs::write(&glb, b"glTF-CHASSIS-BYTES-deterministic-0001").unwrap();
        let b = bake(&glb, AssetKind::Visual, &out).unwrap();
        assert!(
            b.name.ends_with(".GLB"),
            "visual keeps uppercase ext: {} (Python writes Chassis_<sha>.GLB)",
            b.name
        );
        assert_eq!(
            b.name, "Chassis_69c28c6974dd.GLB",
            "byte-for-byte parity with Python bake"
        );

        let stl = d.join("Rotor.STL");
        std::fs::write(
            &stl,
            b"solid box\nfacet ... deterministic collision\nendsolid\n",
        )
        .unwrap();
        let c = bake(&stl, AssetKind::Collision, &out).unwrap();
        assert!(
            c.name.ends_with(".STL"),
            "collision keeps uppercase ext: {} (Python writes Rotor_<sha>.STL)",
            c.name
        );
        assert_eq!(
            c.name, "Rotor_a71b820512ce.STL",
            "byte-for-byte parity with Python bake"
        );

        // The asset file is actually written under the case-preserved name.
        assert!(out.join(&b.name).is_file());
        assert!(out.join(&c.name).is_file());

        // A mixed-case collision extension (.Obj) is likewise preserved verbatim.
        let obj = d.join("Hull.Obj");
        std::fs::write(&obj, b"# obj\nv 0 0 0\n").unwrap();
        let o = bake(&obj, AssetKind::Collision, &out).unwrap();
        assert!(
            o.name.ends_with(".Obj"),
            "collision keeps mixed-case ext: {}",
            o.name
        );

        // An extension-less collision still falls back to the LOWERCASE literal "stl" (Python `or "stl"`).
        let noext = d.join("blob");
        std::fs::write(&noext, b"raw").unwrap();
        let n = bake(&noext, AssetKind::Collision, &out).unwrap();
        assert!(
            n.name.ends_with(".stl"),
            "extensionless collision -> lowercase .stl fallback: {}",
            n.name
        );

        let _ = std::fs::remove_dir_all(&d);
    }
}
