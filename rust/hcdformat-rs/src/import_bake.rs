//! Import-time asset baking: the whole-document mesh-bake orchestration that turns a freshly imported
//! URDF/SDF document into one whose visual `<model>` / collision `<mesh>` references point at canonical,
//! content-addressed baked assets.
//!
//! This is the Rust home of what `hcdf/assets.py` orchestrates on the Python side: the per-visual
//! asset-hint walk over the imported document ([`bake_document`]), the bake decisions (a GLB visual is
//! adopted verbatim, a non-GLB visual is converted to a GLB, a scaled collision is baked lean, an
//! unscaled one is hashed in place), the readable content-addressed filenames (`<stem>_<short12>.<ext>`),
//! the vendored `@uri`/`@sha` patched back into the document, and the deferral-note filtering keyed on the
//! `(comp, visual)` pair ([`drop_baked_notes`]). The two string entry points ([`from_urdf_str_with_baking`]
//! / [`from_sdf_str_with_baking`]) compose an import with that bake in one call, so a caller need not thread
//! the [`crate::VisualAssetHint`] side-channel through a baker of its own.
//!
//! The lower-level walk ([`bake_document`]) is the same one the `hcdf convert --bake DIR` CLI drives, so
//! the CLI and the string entry points share one orchestration (no second, drifting baker).
//!
//! Native-only (it resolves mesh uris on disk and writes assets into a directory) and gated on the `bake`
//! feature; the mesh conversions themselves live in [`crate::bake`], the verbatim passthrough + resolver
//! in [`crate::assets_vendor`].

#![cfg(all(
    feature = "bake",
    not(target_arch = "wasm32"),
    any(feature = "urdf", feature = "sdf")
))]

use crate::assets_vendor::{self, AssetKind};
use crate::model::{Hcdf, ModelRef, VisualAppearance};
use crate::pyrepr::repr_str;
use crate::resolve_uri;
use crate::{
    bake_convert_with_texture, bake_textured_box, BakeKind, BakedAsset, HintTexture,
    VisualAssetHint,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ── string import-then-bake entry points ────────────────────────────────────────────────────────────

/// Import URDF text and bake every referenced mesh in ONE call: resolve + bake each visual/collision mesh
/// off the importer's [`VisualAssetHint`] side-channel (scale/mirror/colour/texture folded into a canonical
/// GLB or lean STL) into `assets_out_dir`, rewrite each `@uri`/`@sha` to `"<uri_prefix>/<name>"`, and return
/// `(baked document, notes)`.
///
/// `base_dir` is where relative / `file://` / `package://` / `model://` mesh uris resolve from (the source's
/// directory); `assets_out_dir` is where the content-addressed baked assets land; `uri_prefix` is prepended
/// to each rewritten `@uri` so a viewer resolves the mesh relative to the output document (e.g. `"assets"`,
/// or `""` for a bare filename); `package_paths` maps `package://`/`model://` roots.
///
/// The notes are the import notes with the per-visual deferral notes for the visuals that actually baked
/// dropped ([`drop_baked_notes`]), plus the walk's own resolution/appearance fallbacks (an unresolvable mesh
/// is left as the source reference with a note, never silently dropped). This is the composition the Python
/// `urdf_hcdf.from_urdf(..., baker=...)` performs, done end to end in the core.
#[cfg(feature = "urdf")]
pub fn from_urdf_str_with_baking(
    src: &str,
    base_dir: &Path,
    assets_out_dir: &Path,
    uri_prefix: &str,
    package_paths: &BTreeMap<String, PathBuf>,
) -> crate::Result<(Hcdf, Vec<String>)> {
    let (doc, notes, hints) = crate::from_urdf::from_urdf_str_with_assets(src)?;
    Ok(import_then_bake(
        doc,
        notes,
        &hints,
        base_dir,
        assets_out_dir,
        uri_prefix,
        package_paths,
    ))
}

/// Import SDF text and bake every referenced mesh in ONE call, the SDF twin of [`from_urdf_str_with_baking`]
/// (see it for the argument + note semantics). The SDF importer's side-channel additionally carries a textured
/// primitive's albedo map, so a textured `<plane>`/`<box>` synthesizes a UV-mapped GLB and its visual becomes
/// an ordinary `<model>` here, exactly as the CLI bake does.
#[cfg(feature = "sdf")]
pub fn from_sdf_str_with_baking(
    src: &str,
    base_dir: &Path,
    assets_out_dir: &Path,
    uri_prefix: &str,
    package_paths: &BTreeMap<String, PathBuf>,
) -> crate::Result<(Hcdf, Vec<String>)> {
    let (doc, notes, hints) = crate::from_sdf::from_sdf_str_with_assets(src)?;
    Ok(import_then_bake(
        doc,
        notes,
        &hints,
        base_dir,
        assets_out_dir,
        uri_prefix,
        package_paths,
    ))
}

/// Shared tail of the two string entry points: run the bake walk over the imported `doc`, drop the deferral
/// notes for the visuals that baked, and append the walk's own resolution/appearance fallbacks.
fn import_then_bake(
    mut doc: Hcdf,
    import_notes: Vec<String>,
    hints: &[VisualAssetHint],
    base_dir: &Path,
    assets_out_dir: &Path,
    uri_prefix: &str,
    package_paths: &BTreeMap<String, PathBuf>,
) -> (Hcdf, Vec<String>) {
    let env = BakeEnv::direct(
        base_dir.to_path_buf(),
        assets_out_dir.to_path_buf(),
        uri_prefix.to_string(),
        package_paths,
    );
    let mut bake_notes = Vec::new();
    let baked = bake_document(&mut doc, hints, &env, &mut bake_notes);
    let mut notes = drop_baked_notes(import_notes, &baked);
    notes.extend(bake_notes);
    (doc, notes)
}

// ── the whole-document bake walk (shared with `hcdf convert --bake DIR`) ─────────────────────────────

/// The disk-resolution context for one bake pass: where source mesh uris resolve from, where the baked
/// assets land, the `package://`/`model://` map, and the `@uri` prefix. Bundles what every [`bake_one`] call
/// needs so the per-mesh signature stays within the clippy argument budget.
pub struct BakeEnv<'a> {
    /// The base for relative / `file://` source mesh uris (the input document's directory).
    base_dir: PathBuf,
    /// The `--package PKG=PATH` map: for `package://PKG/…` / `model://MODEL/…` source uris.
    package_map: &'a BTreeMap<String, PathBuf>,
    /// Where the content-addressed baked assets land.
    out_dir: PathBuf,
    /// Prefix prepended to a baked filename so the written `@uri` is relative to the OUTPUT document (a
    /// viewer then resolves the mesh next to it with no resource-path setup); `""`/`"."` writes the bare
    /// filename.
    uri_prefix: String,
}

impl<'a> BakeEnv<'a> {
    /// Anchor a bake pass from the CLI's input/output file paths: assets land in `bake_dir`, source uris
    /// resolve from the INPUT's directory, and the written `@uri` prefix is `bake_dir` relative to the
    /// OUTPUT document's directory (the cwd for a `-` stdout output).
    pub fn new(
        bake_dir: &str,
        input: &str,
        output: &str,
        package_map: &'a BTreeMap<String, PathBuf>,
    ) -> Result<Self, String> {
        let out_dir = abs_norm(Path::new(bake_dir))?;
        let doc_dir = if output == "-" {
            std::env::current_dir().map_err(|e| format!("cwd: {e}"))?
        } else {
            parent_dir(&abs_norm(Path::new(output))?)
        };
        let base_dir = parent_dir(&abs_norm(Path::new(input))?);
        let uri_prefix = rel_path(&out_dir, &doc_dir);
        Ok(BakeEnv {
            base_dir,
            package_map,
            out_dir,
            uri_prefix,
        })
    }

    /// Anchor a bake pass from explicit resolution roots (the string import-then-bake entry points):
    /// meshes resolve from `base_dir`, baked assets land in `assets_out_dir`, and each rewritten `@uri`
    /// is prefixed with `uri_prefix`.
    pub fn direct(
        base_dir: PathBuf,
        assets_out_dir: PathBuf,
        uri_prefix: String,
        package_map: &'a BTreeMap<String, PathBuf>,
    ) -> Self {
        BakeEnv {
            base_dir,
            package_map,
            out_dir: assets_out_dir,
            uri_prefix,
        }
    }
}

/// The directory containing `p` (which is absolute), or the filesystem root for a rootless edge case,
/// mirroring `os.path.dirname(os.path.abspath(p))`.
fn parent_dir(p: &Path) -> PathBuf {
    p.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Lexical `os.path.abspath`: join onto the cwd and collapse `.`/`..` WITHOUT touching the filesystem
/// (`fs::canonicalize` would fail on the not-yet-created bake/output dirs and resolve symlinks, which
/// Python's `abspath` does not).
fn abs_norm(p: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("cwd: {e}"))?
            .join(p)
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                // `pop` on the root is a no-op, so a leading `/..` collapses to `/` like Python normpath.
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

/// `os.path.relpath(target, base)` for two ABSOLUTE, already-[`abs_norm`]alized paths, `/`-joined like
/// Python's `.replace(os.sep, "/")`: strip the common component prefix, then one `..` per remaining
/// `base` component. `"."` when they coincide (which [`bake_one`] treats as "no prefix", like Python).
fn rel_path(target: &Path, base: &Path) -> String {
    let t: Vec<_> = target.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".to_string(); b.len() - common];
    parts.extend(
        t[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Which appearance arm a mesh being baked belongs to; drives the GLB-visual / lean-collision branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeshSite {
    Visual,
    Collision,
}

/// The per-mesh bake spec bundled into ONE value so [`bake_one`] stays within the clippy argument budget:
/// the scale to fold into the geometry, the flat material colour / albedo texture uri to bake onto
/// material-less visual geometry (visual only; `None` for collisions), and which appearance arm this is.
/// `scale`/`color`/`texture` come from the importer's [`VisualAssetHint`] side-channel for visuals and from
/// the round-tripped `mesh.scale` for collisions.
struct MeshBakeSpec<'a> {
    scale: Option<&'a str>,
    color: Option<&'a str>,
    /// The `<pbr><albedo_map>` uri (resolved to bytes at bake time via [`resolve_uri`], like the mesh
    /// itself), embedded onto material-less geometry through the mesh's own texture coordinates.
    texture: Option<&'a str>,
    site_kind: MeshSite,
}

/// Bake EVERY comp visual `<model uri>` / collision `<mesh uri>` in `doc` into `env.out_dir` and rewrite its
/// `@uri` (+ `@sha`) to the output-relative baked asset: the walk over the [`VisualAssetHint`] side-channel
/// (the same one dendrite_build's import bake runs). Returns the set of `(comp, visual)` name pairs whose
/// VISUAL baked, so [`drop_baked_notes`] can remove exactly those deferral notes; collisions bake too but do
/// not carry a deferral note (their scale round-trips onto the HCDF mesh), so they are not in the set.
///
/// Per mesh, mirroring `hcdf.assets.bake`:
///   * visual: an already-GLB/glTF source is passed through verbatim (kept byte-for-byte, same `@sha`; a
///     scale/colour hint on one cannot fold in and is NOTED, never silently dropped); any other source
///     (STL/OBJ/DAE) is converted to a canonical GLB with the hint's scale/mirror/colour baked in.
///   * collision: passed through verbatim when there is no scale; a SCALED collision is baked to a lean STL
///     with the scale folded into the vertices (and the now-consumed `@scale` cleared; an unbaked one keeps
///     it so the source scale round-trips, exactly like Python).
///
/// NEVER drops a mesh silently: an UNRESOLVABLE uri (a `package://` with no mapping, a missing file) is LEFT
/// as the source reference with a note, and a bake FAILURE (unsupported/corrupt source, a `.dae` without
/// `bake-dae`) likewise notes WHY, so a partial-mesh robot still converts, failures visible. A REMOTE
/// (http/https) mesh is intentionally not baked (matching Python, whose `resolve_uri` never matches one): it
/// stays a live uri, and `hcdf bundle --vendor-remote` is the fetch path.
pub fn bake_document(
    doc: &mut Hcdf,
    hints: &[VisualAssetHint],
    env: &BakeEnv,
    notes: &mut Vec<String>,
) -> BTreeSet<(String, String)> {
    // Index the hints by (comp, visual) for O(log n) lookup during the walk.
    let hint_map: BTreeMap<(&str, &str), &VisualAssetHint> = hints
        .iter()
        .map(|h| ((h.comp.as_str(), h.visual.as_str()), h))
        .collect();
    let mut baked: BTreeSet<(String, String)> = BTreeSet::new();
    for comp in &mut doc.comp {
        let comp_name = comp.name.clone();
        for v in &mut comp.visual {
            let vname = v.name.clone();
            let site = format!("visual {comp_name}/{vname}");
            let hint = hint_map.get(&(comp_name.as_str(), vname.as_str())).copied();
            match &mut v.appearance {
                VisualAppearance::Model { model, .. } => {
                    // Fold this visual's source mesh scale (magnitude + mirror), flat material colour
                    // and albedo texture into the baked GLB via the importer's side-channel hint (the
                    // HCDF model carries none of them on a <model>). Absent hint (no scale/material
                    // authored) ⇒ all-None ⇒ identity.
                    let spec = MeshBakeSpec {
                        scale: hint.and_then(|h| h.scale.as_deref()),
                        color: hint.and_then(|h| h.color.as_deref()),
                        texture: hint.and_then(|h| h.texture.as_deref()),
                        site_kind: MeshSite::Visual,
                    };
                    if bake_one(&mut model.uri, &mut model.sha, spec, env, &site, notes) {
                        baked.insert((comp_name.clone(), vname.clone()));
                    }
                }
                VisualAppearance::Primitive { geometry, .. } => {
                    // A TEXTURED PRIMITIVE (the SDF <pbr><albedo_map> side-channel; a textured <plane>
                    // imported as a zero-thickness <box>) synthesizes a UV-mapped GLB and the visual
                    // becomes an ordinary <model uri>; the no-bake conversion kept the primitive, the
                    // bake upgrades it. Only a <box> synthesizes (plane included); other shapes keep
                    // their flat colour with a note (the honest tier). Any miss leaves the primitive.
                    let Some(tex_uri) = hint.and_then(|h| h.texture.as_deref()) else {
                        continue;
                    };
                    let box_size = geometry
                        .as_ref()
                        .and_then(|g| g.box_.as_ref())
                        .and_then(|b| b.size.clone());
                    if let Some((uri, sha)) =
                        bake_textured_primitive(box_size.as_deref(), tex_uri, env, &site, notes)
                    {
                        v.appearance = VisualAppearance::Model {
                            model: ModelRef {
                                uri: Some(uri),
                                sha: Some(sha),
                                ..Default::default()
                            },
                            geometry: None,
                        };
                        baked.insert((comp_name.clone(), vname.clone()));
                    }
                }
            }
        }
        for c in &mut comp.collision {
            let coll_name = c.name.clone().unwrap_or_else(|| "<unnamed>".to_string());
            if let Some(mesh) = c.geometry.as_mut().and_then(|g| g.mesh.as_mut()) {
                let site = format!("collision {comp_name}/{coll_name}");
                // Collisions carry their own round-tripped `mesh.scale` on the HCDF model (no side-channel)
                // and NO baked colour (a lean STL has no material). The scale is cleared ONLY on a
                // successful bake (it is then folded into the vertices); an unbaked mesh keeps it.
                let scale = mesh.scale.clone();
                let spec = MeshBakeSpec {
                    scale: scale.as_deref(),
                    color: None,
                    texture: None,
                    site_kind: MeshSite::Collision,
                };
                if bake_one(&mut mesh.uri, &mut mesh.sha, spec, env, &site, notes) {
                    mesh.scale = None;
                }
            }
        }
    }
    baked
}

/// Resolve + bake ONE mesh uri in place (the per-mesh core of [`bake_document`]); `true` iff `uri` was
/// rewritten to the baked asset (+ `sha` set). Leaves `uri` unchanged when it is empty, REMOTE (kept
/// live for `bundle --vendor-remote`, silently, since it is not a loss), UNRESOLVABLE (note), or the bake
/// fails (note).
fn bake_one(
    uri: &mut Option<String>,
    sha: &mut Option<String>,
    spec: MeshBakeSpec,
    env: &BakeEnv,
    site: &str,
    notes: &mut Vec<String>,
) -> bool {
    let Some(src_uri) = uri.clone() else {
        return false; // no uri, nothing to bake (a primitive-only collision, etc.)
    };
    if src_uri.trim().is_empty() {
        return false;
    }
    if assets_vendor::is_remote_uri(&src_uri) {
        return false;
    }
    let Some(path) = resolve_uri(&src_uri, &env.base_dir, env.package_map) else {
        notes.push(format!(
            "{site}: mesh {src_uri:?} could not be resolved (pass --package PKG=PATH / check the path); left as the source reference"
        ));
        return false;
    };
    match bake_resolved(&path, &spec, env, site, notes) {
        Ok((name, baked_sha)) => {
            *uri = Some(prefixed_uri(env, name));
            *sha = Some(baked_sha);
            true
        }
        Err(e) => {
            notes.push(format!(
                "{site}: could not bake mesh {src_uri:?}: {e}; left as the source reference"
            ));
            false
        }
    }
}

/// The output-document-relative `@uri` of a baked asset `name` (the [`BakeEnv::uri_prefix`] rule: bare
/// filename when the prefix is empty or `"."`, else `"<prefix>/<name>"`).
fn prefixed_uri(env: &BakeEnv, name: String) -> String {
    if env.uri_prefix.is_empty() || env.uri_prefix == "." {
        name
    } else {
        format!("{}/{name}", env.uri_prefix.trim_end_matches('/'))
    }
}

/// Synthesize + write the textured-primitive GLB of ONE visual (the per-visual core of the
/// [`bake_document`] Primitive arm): resolve the hint's albedo `tex_uri` (the same [`resolve_uri`] +
/// `--package` machinery as a mesh uri), synthesize the `<box>`-shaped quad/box mesh with the texture
/// embedded ([`bake_textured_box`]), and write it content-addressed into the bake dir. Returns the
/// `(uri, sha)` the visual's new `<model>` carries, or `None` with a note naming the miss (no `<box>`
/// geometry / unresolvable or unreadable texture / degenerate extents); the primitive then stays,
/// keeping its flat colour, never silently dropped.
fn bake_textured_primitive(
    box_size: Option<&str>,
    tex_uri: &str,
    env: &BakeEnv,
    site: &str,
    notes: &mut Vec<String>,
) -> Option<(String, String)> {
    let Some(size) = box_size else {
        notes.push(format!(
            "{site}: albedo map {tex_uri:?} not applied (textured-primitive synthesis handles <box> \
             (and the <plane> imported as one) only); the primitive keeps its flat colour"
        ));
        return None;
    };
    let Some(path) = resolve_uri(tex_uri, &env.base_dir, env.package_map) else {
        notes.push(format!(
            "{site}: texture {tex_uri:?} could not be resolved (pass --package PKG=PATH / check the \
             path); the primitive keeps its flat colour"
        ));
        return None;
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            notes.push(format!(
                "{site}: texture {tex_uri:?} could not be read ({e}); the primitive keeps its flat colour"
            ));
            return None;
        }
    };
    let texture = HintTexture {
        bytes,
        name: path.to_string_lossy().into_owned(),
    };
    let asset = match bake_textured_box(size, &texture) {
        Ok(asset) => asset,
        Err(e) => {
            notes.push(format!(
                "{site}: could not bake textured primitive with {tex_uri:?}: {e}; the primitive keeps \
                 its flat colour"
            ));
            return None;
        }
    };
    match write_converted(&safe_stem(&path), &asset, env) {
        Ok((name, sha)) => Some((prefixed_uri(env, name), sha)),
        Err(e) => {
            notes.push(format!(
                "{site}: could not write the textured-primitive GLB for {tex_uri:?}: {e}; the \
                 primitive keeps its flat colour"
            ));
            None
        }
    }
}

/// Bake ONE resolved-on-disk mesh into `env.out_dir`, returning `(filename, "sha256:<hex>")`. Mirrors
/// `hcdf.assets.bake`: a visual GLB (magic-checked) and an unscaled collision are a verbatim passthrough
/// ([`crate::assets_vendor::bake`], byte-identical to the source, same `@sha`), while a `.gltf`
/// visual is PACKED by that same call into ONE self-contained GLB (external `.bin` buffers + textures
/// inlined; byte-copying would sever the relative companion links under the content-hash rename); any
/// other source is CONVERTED ([`bake_convert_with_texture`]: STL/OBJ/DAE -> GLB for a visual, scaled
/// collision -> lean STL) and written under the content-addressed `<stem>_<short12>.<ext>` name.
///
/// Appearance fallbacks the conversion reports (an OBJ's unresolved `.mtl`, a skipped `map_Kd` texture)
/// are PUSHED onto `notes` (site-prefixed) so an import that degrades a visual says so instead of shipping
/// an unexplained gray mesh.
fn bake_resolved(
    path: &Path,
    spec: &MeshBakeSpec,
    env: &BakeEnv,
    site: &str,
    notes: &mut Vec<String>,
) -> Result<(String, String), String> {
    let ext_lower = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let is_glb = ext_lower == "glb" || ext_lower == "gltf";
    let scale_is_identity = scale_is_none_or_identity(spec.scale);

    // Passthrough cases: an already-canonical GLB/glTF visual (a real GLB is a verbatim copy; a `.gltf`
    // is packed self-contained by `assets_vendor::bake`), and any collision with no scale. A GLB/glTF
    // visual NEVER takes the conversion arm (the bake refuses GLB input, AlreadyGlb), so a
    // scale/colour/texture hint on one cannot be folded in; never leave that silent (dendrite_build's
    // vendor walk notes the same case).
    let passthrough = match spec.site_kind {
        MeshSite::Visual => is_glb,
        MeshSite::Collision => scale_is_identity,
    };
    if passthrough {
        if spec.site_kind == MeshSite::Visual
            && (!scale_is_identity || spec.color.is_some() || spec.texture.is_some())
        {
            notes.push(format!(
                "{site}: GLB/glTF visual passed through (a .gltf packs to a self-contained GLB); its \
                 source mesh scale/colour/texture cannot be folded into an already-glTF source \
                 (re-author as a non-GLB mesh to bake them in)"
            ));
        }
        let kind = match spec.site_kind {
            MeshSite::Visual => AssetKind::Visual,
            MeshSite::Collision => AssetKind::Collision,
        };
        let baked = assets_vendor::bake(path, kind, &env.out_dir)
            .map_err(|e| format!("copy failed: {e}"))?;
        return Ok((baked.name, baked.sha));
    }

    // The hint's albedo texture, resolved to bytes like the mesh itself was (resolve_uri +
    // `--package`); an unresolvable/unreadable one degrades to a note + the flat colour, the same
    // note-not-error posture as every other appearance fallback.
    let texture = spec.texture.and_then(|tex_uri| {
        let resolved = resolve_uri(tex_uri, &env.base_dir, env.package_map)
            .ok_or_else(|| {
                "could not be resolved (pass --package PKG=PATH / check the path)".to_string()
            })
            .and_then(|p| std::fs::read(&p).map_err(|e| format!("could not be read ({e})")));
        match resolved {
            Ok(bytes) => Some(HintTexture {
                bytes,
                name: tex_uri.to_string(),
            }),
            Err(why) => {
                notes.push(format!(
                    "{site}: texture {tex_uri:?} {why}; flat colour used"
                ));
                None
            }
        }
    });

    // Conversion case (scale/mirror/colour/texture baked into the geometry, or a non-GLB visual
    // canonicalized to GLB).
    let bake_kind = match spec.site_kind {
        MeshSite::Visual => BakeKind::Visual,
        MeshSite::Collision => BakeKind::Collision,
    };
    let asset = bake_convert_with_texture(
        &path.to_string_lossy(),
        spec.scale,
        bake_kind,
        spec.color,
        texture.as_ref(),
    )
    .map_err(|e| e.to_string())?;
    // Surface appearance fallbacks (unresolved .mtl / skipped texture) as returnable notes so an import
    // that degrades a visual says so, instead of shipping an unexplained gray mesh.
    for note in &asset.notes {
        notes.push(format!("{site}: {note}"));
    }
    write_converted(&safe_stem(path), &asset, env)
}

/// Write one CONVERTED asset into the bake dir under the content-addressed `<stem>_<short12>.<ext>`
/// name (the naming half [`bake_resolved`] and the textured-primitive synthesis share), returning
/// `(filename, sha)`.
fn write_converted(
    stem: &str,
    asset: &BakedAsset,
    env: &BakeEnv,
) -> Result<(String, String), String> {
    let short = asset
        .sha
        .split_once(':')
        .map_or(asset.sha.as_str(), |(_, h)| h);
    let short = &short[..short.len().min(12)];
    let name = format!("{stem}_{short}.{}", asset.ext);
    std::fs::create_dir_all(&env.out_dir).map_err(|e| format!("could not create bake dir: {e}"))?;
    std::fs::write(env.out_dir.join(&name), &asset.data)
        .map_err(|e| format!("could not write baked mesh: {e}"))?;
    Ok((name, asset.sha.clone()))
}

/// `true` when a scale string is absent, blank, or the identity `1 1 1` (so no conversion is needed),
/// mirroring `hcdf.assets._scale_vec`'s identity check used to pick the passthrough branch.
fn scale_is_none_or_identity(scale: Option<&str>) -> bool {
    match scale.map(str::trim) {
        None | Some("") => true,
        Some(s) => {
            let parts: Vec<f64> = s
                .split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect();
            !parts.is_empty() && parts.iter().all(|v| (v - 1.0).abs() < 1e-12)
        }
    }
}

/// A filesystem-safe stem from a source mesh path (basename without extension, non-`[0-9A-Za-z._-]` ->
/// `_`, empty -> `"mesh"`): the readable half of the content-addressed `<stem>_<short>.<ext>` baked
/// name, matching `hcdf.assets._safe_stem`. Cosmetic only (the `@sha` is over the bytes).
fn safe_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
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

// ── deferral-note filtering keyed on the (comp, visual) pair ─────────────────────────────────────────

/// The markers on the per-visual "deferred to the asset step" notes the importers emit for a mesh visual
/// whose scale/colour/texture rode the [`VisualAssetHint`] side-channel. Once [`bake_document`] bakes that
/// visual (folding scale + colour + texture into the GLB) the notes are CONSUMED, so they are dropped for
/// the baked visual (`hcdf.assets._BAKED_NOTE_MARKERS`).
const BAKED_NOTE_MARKERS: [&str; 2] = [
    "not applied (bake the GLB",
    "carried on the asset side-channel",
];

/// Drop the per-visual "deferred to the asset step" notes for the visuals that actually baked.
///
/// `baked` is the set of `(comp, visual)` name pairs [`bake_document`] returns. A note is dropped iff it
/// carries a deferral marker AND names one of those baked pairs. The match uses BOTH the comp and the visual
/// where the importer wrote the comp, so a same-named visual under a DIFFERENT comp keeps its own note: the
/// SDF importer writes the comp into the note (`... 'comp' visual 'name' ...`), matched on both. An importer
/// that omits the comp (the URDF wording opens the note with `visual 'name'`) carries nothing to key the comp
/// on, so it is matched by visual name alone. Notes for an UNBAKED (unresolvable) visual are kept, as are all
/// other notes. This is the exact contract of `hcdf.assets.drop_baked_notes`.
pub fn drop_baked_notes(notes: Vec<String>, baked: &BTreeSet<(String, String)>) -> Vec<String> {
    if baked.is_empty() {
        return notes;
    }
    notes
        .into_iter()
        .filter(|n| !note_is_consumed(n, baked))
        .collect()
}

/// Whether one note is a deferral note consumed by a baked visual (the predicate behind [`drop_baked_notes`]).
fn note_is_consumed(note: &str, baked: &BTreeSet<(String, String)>) -> bool {
    if !BAKED_NOTE_MARKERS.iter().any(|m| note.contains(m)) {
        return false;
    }
    // Comp-qualified wording (SDF: "... 'comp' visual 'name' ..."): match on BOTH names so a same-named
    // visual under another comp is never cross-dropped.
    if baked
        .iter()
        .any(|(comp, name)| note.contains(&format!("{} visual {}", repr_str(comp), repr_str(name))))
    {
        return true;
    }
    // Comp-less wording (URDF: the note opens with "visual 'name'"): no comp to key on, so match by visual
    // name alone, preserving that importer's behavior.
    let stripped = note.trim_start();
    if stripped.starts_with("visual ") {
        return baked
            .iter()
            .any(|(_comp, name)| stripped.starts_with(&format!("visual {}", repr_str(name))));
    }
    false
}
