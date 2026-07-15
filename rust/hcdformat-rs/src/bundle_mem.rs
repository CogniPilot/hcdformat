//! IN-MEMORY `.hcdfz` packer: the browser-side bundle payoff.
//!
//! [`pack_to_bytes`] builds a self-contained, portable `.hcdfz` from an IN-MEMORY [`Hcdf`] plus a
//! caller-supplied map of asset bytes, returning the bundle bytes for the caller to download (web) or
//! write (native). It is the wasm-capable counterpart to [`crate::bundle::pack`] (which walks the local
//! filesystem and is `cfg(not(wasm32))`-gated): this one does NO filesystem access, so it compiles and
//! runs on native AND wasm32. The native disk packer stays unchanged.
//!
//! ## What it produces (same contract as `bundle::pack`)
//! A `.hcdfz` is a USDZ-shaped `ZIP_STORED` archive (uncompressed, so a blob's bytes are byte-identical
//! to its `@sha`) whose FIRST entry is the root `.hcdf` and whose remaining entries are the meshes under
//! `assets/`. The packed document is FLAT (every `<include>` resolved) and document-relative (every mesh
//! uri rewritten to `assets/<name>` with a matching `@sha`).
//!
//! ## How assets are supplied (no disk)
//! The caller provides a [`MemBundle`]: a map from a mesh uri AS IT APPEARS IN THE DOCUMENT (the
//! visual `<model @uri>` / collision `<mesh @uri>` string, e.g. `"assets/wheel.glb"` or an absolute
//! path the picker produced) to that mesh's raw bytes (the bytes the browser file-picker read). For each
//! LOCAL mesh site whose uri is present in the map, the bytes are content-addressed
//! (`<stem>_<short-sha>.<ext>`), copied into the bundle's `assets/`, and the `@uri`/`@sha` rewritten;
//! identical bytes dedupe to one entry. A site whose uri is NOT in the map is left as-is and reported as
//! unresolved (the browser had no bytes for it); the caller decides whether that is acceptable
//! (`allow_partial`) or an error.
//!
//! ## Includes
//! Flattening uses [`crate::compose::flatten_with`] with a loader that resolves each `<include>` uri from
//! the SAME [`MemBundle`] (its bytes parsed as a sub-`Hcdf`). An include whose uri has no bytes in the
//! map is left in place with a note (a browser has no filesystem to read it from); a true cycle is an
//! error. Most browser docs are self-contained (no includes), so the common path is a pure flatten no-op.

use crate::compose::{content_sha, flatten_with};
use crate::model::{Hcdf, VisualAppearance};
use crate::zip_store::{self, ZipEntry};
use std::collections::BTreeMap;

const ASSETS_DIR: &str = "assets";
/// The bundle sub-directory a KEEP-LIVE `.hcdfz` embeds each referenced `<include>` module under
/// (`modules/<msha>/<stem>.hcdf` beside `modules/<msha>/assets/<name>`). A FLAT bundle never writes here,
/// so the presence of any `modules/` entry marks a keep-live bundle for the opener.
const MODULES_DIR: &str = "modules";
/// A sentinel zip entry a KEEP-LIVE bundle writes so an opener can positively identify the mode even before
/// scanning for `modules/` entries. Ignored (left out of the asset tree) by [`open_bundle_bytes`]; harmless
/// to a flat-only opener (an unreferenced extra entry).
const KEEPLIVE_MARKER: &str = "_keeplive";

/// One restored keep-live module: `(bundle_module_path, module_doc, module_meshes)`, the archive entry
/// name the root's `<include> @uri` points at, the parsed module document, and its
/// `(module_relative_uri, bytes)` mesh pairs. The unit dendrite re-registers so a reloaded include resolves
/// live. (A named alias so the [`OpenedMemBundle::modules`] field stays within the clippy type budget.)
pub type OpenedModule = (String, Hcdf, Vec<(String, Vec<u8>)>);

/// An opened in-memory `.hcdfz` bundle: the parsed root [`Hcdf`] plus every non-root archive entry as a
/// `(relative_path, bytes)` pair (the asset tree, e.g. `("assets/wheel_<sha>.glb", <bytes>)`).
///
/// The wasm counterpart to [`crate::bundle::open_bundle`] (which extracts a `.hcdfz` to a tempdir on
/// disk): this returns the assets IN MEMORY so a browser caller can hand them to a custom Bevy
/// `AssetReader` (keyed by the same `assets/<name>` document-relative uri the root doc carries) without
/// any filesystem. The asset `path`s are exactly the archive entry names AFTER the root, so they match
/// the rewritten `@uri`s in [`Self::doc`] verbatim.
///
/// (`PartialEq` only: [`Hcdf`] carries float fields and is not `Eq`.)
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedMemBundle {
    /// The bundle's root document (parsed from the FIRST archive entry: the `.hcdf`, by the bundle
    /// contract). Its mesh `@uri`s are document-relative `assets/<name>` strings that key [`Self::assets`].
    pub doc: Hcdf,
    /// Every non-root archive entry that is NOT a keep-live module: `(relative_path, bytes)`, in archive
    /// order. For a normal (flat) bundle these are the `assets/<name>` mesh blobs; the path matches the
    /// doc's rewritten `@uri` exactly. A keep-live bundle's `modules/…` entries are surfaced in
    /// [`Self::modules`] instead (and the `_keeplive` marker is dropped entirely).
    pub assets: Vec<(String, Vec<u8>)>,
    /// KEEP-LIVE bundles only: each embedded `<include>` module, as
    /// `(bundle_module_path, module_doc, module_meshes)` where `bundle_module_path` is the archive entry
    /// name (`modules/<msha>/<stem>.hcdf`) the ROOT doc's `<include> @uri` now points at, and
    /// `module_meshes` are that module's `(module_relative_uri, bytes)` pairs (e.g. `("assets/x.glb", …)`).
    /// EMPTY for a flat bundle (backward compatible: an old `.hcdfz` yields no module entries), so a caller
    /// that ignores this field behaves exactly as before.
    pub modules: Vec<OpenedModule>,
}

/// The in-memory asset source for [`pack_to_bytes`]: a map from a uri AS IT APPEARS IN THE DOCUMENT (a
/// mesh `@uri` or an `<include> @uri`) to that resource's raw bytes. Keys match the document's uri
/// strings exactly (the picker/editor records the bytes under the uri it wrote into the doc). A
/// [`BTreeMap`] so iteration/order is deterministic.
pub type MemBundle = BTreeMap<String, Vec<u8>>;

/// A borrowed, zero-copy view of a mesh-byte source (a [`MemBundle`] or a [`KeepLiveModule::meshes`]
/// list): uri -> `&[u8]`. The packers resolve/rewrite through this view so the mesh bytes are BORROWED
/// from the caller's map all the way into the zip writer, no per-site clone, which on a wasm save of a
/// multi-MB bundle used to multiply peak heap 2-3×.
type MemView<'a> = BTreeMap<&'a str, &'a [u8]>;

/// The borrowed [`MemView`] of a caller's [`MemBundle`].
fn mem_view(assets: &MemBundle) -> MemView<'_> {
    assets
        .iter()
        .map(|(uri, bytes)| (uri.as_str(), bytes.as_slice()))
        .collect()
}

/// The deduped `assets/` payload [`rewrite_local_mesh_sites`] collects: content-addressed
/// `<stem>_<short>.<ext>` name -> mesh bytes, the bytes still borrowed from the caller's map.
type BundleAssets<'a> = BTreeMap<String, &'a [u8]>;

/// What [`pack_to_bytes`] did, for the caller's status line: mirrors the meaningful parts of
/// [`crate::bundle::PackManifest`] without the filesystem-only fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemPackReport {
    /// The root `.hcdf` filename inside the bundle (the document name, sanitized).
    pub root: String,
    /// Number of ZIP entries written (1 root + N deduped assets).
    pub entries: usize,
    /// `"<comp>/<visual-or-collision>"` sites whose mesh bytes WERE embedded.
    pub embedded: Vec<String>,
    /// `"<site> (<uri>)"` mesh sites whose uri had NO bytes in the [`MemBundle`]: left unrewritten.
    pub unresolved: Vec<String>,
    /// Human-readable flatten/notes (include resolution, etc.).
    pub notes: Vec<String>,
}

/// True iff `uri` is an `http(s)://` URL (a remote mesh/include the in-memory packer never fetches).
fn is_remote(uri: &str) -> bool {
    let low = uri.to_ascii_lowercase();
    low.starts_with("http://") || low.starts_with("https://")
}

/// Sanitize a document name into a confined, single-component bundle filename STEM: keep only the
/// portable filename charset (`0-9 A-Z a-z . _ -`) and map every other byte, path separators `/` and
/// `\` included, to `_`. A hostile `doc.name` such as `..\..\evil` (or `../evil`) can then never widen
/// into a subpath or a `..` traversal at a write site, whether the stem becomes a loose file name or a
/// zip entry name a foreign extractor honors. A stem that reduces to empty or to only dots falls back
/// to `fallback`, so the result is always a single non-dot component. Every safe-charset document name
/// (the whole current corpus) is left byte-for-byte unchanged.
pub(crate) fn safe_root_stem(stem: &str, fallback: &str) -> String {
    let mapped: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if mapped.is_empty() || mapped.chars().all(|c| c == '.') {
        fallback.to_string()
    } else {
        mapped
    }
}

/// The bundle's root `.hcdf` filename: the document name, else `root`, sanitized to a confined
/// single-component name (mirrors `bundle::root_name`, but with no source path to fall back on in
/// memory).
fn root_name(doc: &Hcdf) -> String {
    let stem = if doc.name.is_empty() {
        "root".to_string()
    } else {
        doc.name.clone()
    };
    format!("{}.hcdf", safe_root_stem(&stem, "root"))
}

/// The lowercased file extension of a uri (after the last `.` in the last path segment), or `"bin"` when
/// there is none: the suffix the content-addressed `assets/<stem>_<short>.<ext>` name uses.
fn uri_ext(uri: &str) -> String {
    let last = uri.rsplit(['/', '\\']).next().unwrap_or(uri);
    match last.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => "bin".to_string(),
    }
}

/// The file stem of a uri (last path segment, before the final `.`), or `"mesh"` when there is none.
fn uri_stem(uri: &str) -> String {
    let last = uri.rsplit(['/', '\\']).next().unwrap_or(uri);
    let stem = match last.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => last,
    };
    if stem.is_empty() {
        "mesh".to_string()
    } else {
        stem.to_string()
    }
}

/// The content-addressed `assets/<stem>_<short>.<ext>` name + `"sha256:<hex>"` for `bytes`, derived from
/// the source `uri`'s stem/extension and the bytes' sha256 (first 12 hex chars short). Identical bytes
/// from the same-named source always yield the same name (so the bundle dedupes).
fn addressed(uri: &str, bytes: &[u8]) -> (String, String) {
    let sha = content_sha(bytes); // "sha256:<hex>"
    let hex = sha.split_once(':').map_or(sha.as_str(), |(_, h)| h);
    let short = hex.get(..12).unwrap_or(hex);
    let name = format!("{}_{short}.{}", uri_stem(uri), uri_ext(uri));
    (name, sha)
}

/// Rewrite every LOCAL visual `<model>` / collision `<mesh>` site in `doc` whose bytes are present in
/// `assets`: content-address the bytes, collect them into a name-keyed `assets/` map (deduped by name,
/// bytes BORROWED from the view, never cloned), and rewrite the site's `@uri` to `assets/<name>` with
/// the matching `@sha`. Returns `(bundle_assets, embedded_sites, unresolved_sites,
/// unresolved_local_count)`. A remote (`http(s)`) site is reported (never embedded); a LOCAL site with no
/// bytes is left unrewritten and counted in the last field. Shared by [`pack_to_bytes`] (on the flattened
/// doc) and [`pack_to_bytes_keep_live`] (on the root AND on each embedded module doc, so a module's own
/// meshes are content-addressed the same way).
fn rewrite_local_mesh_sites<'a>(
    doc: &mut Hcdf,
    assets: &MemView<'a>,
) -> (BundleAssets<'a>, Vec<String>, Vec<String>, usize) {
    let mut bundle_assets: BundleAssets<'a> = BTreeMap::new();
    let mut embedded: Vec<String> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut unresolved_local = 0usize;

    let comp_count = doc.comp.len();
    for ci in 0..comp_count {
        let comp_name = doc.comp[ci].name.clone();

        // Visual <model @uri> sites.
        let vis_count = doc.comp[ci].visual.len();
        for vi in 0..vis_count {
            let (vname, uri) = {
                let v = &doc.comp[ci].visual[vi];
                let uri = match &v.appearance {
                    VisualAppearance::Model { model, .. } => model.uri.clone(),
                    VisualAppearance::Primitive { .. } => None,
                };
                (v.name.clone(), uri)
            };
            let Some(uri) = uri.filter(|u| !u.is_empty()) else {
                continue;
            };
            let site = format!("{comp_name}/{vname}");
            match resolve_site(&uri, assets) {
                Resolved::Embed(bytes) => {
                    let (name, sha) = addressed(&uri, bytes);
                    bundle_assets.entry(name.clone()).or_insert(bytes);
                    if let VisualAppearance::Model { model, .. } =
                        &mut doc.comp[ci].visual[vi].appearance
                    {
                        model.uri = Some(format!("{ASSETS_DIR}/{name}"));
                        model.sha = Some(sha);
                    }
                    embedded.push(site);
                }
                Resolved::Remote => unresolved.push(format!("{site} ({uri}) [remote]")),
                Resolved::Missing => {
                    unresolved.push(format!("{site} ({uri})"));
                    unresolved_local += 1;
                }
            }
        }

        // Collision <geometry><mesh @uri> sites.
        let col_count = doc.comp[ci].collision.len();
        for col_i in 0..col_count {
            let (cname, uri) = {
                let c = &doc.comp[ci].collision[col_i];
                let mesh = c.geometry.as_ref().and_then(|g| g.mesh.as_ref());
                (
                    c.name.clone().unwrap_or_default(),
                    mesh.and_then(|m| m.uri.clone()),
                )
            };
            let Some(uri) = uri.filter(|u| !u.is_empty()) else {
                continue;
            };
            let site = format!("{comp_name}/{cname}");
            match resolve_site(&uri, assets) {
                Resolved::Embed(bytes) => {
                    let (name, sha) = addressed(&uri, bytes);
                    bundle_assets.entry(name.clone()).or_insert(bytes);
                    if let Some(mesh) = doc.comp[ci].collision[col_i]
                        .geometry
                        .as_mut()
                        .and_then(|g| g.mesh.as_mut())
                    {
                        if !is_remote(&uri) && mesh.source_uri.is_none() {
                            mesh.source_uri = Some(uri.clone());
                        }
                        mesh.uri = Some(format!("{ASSETS_DIR}/{name}"));
                        mesh.sha = Some(sha);
                    }
                    embedded.push(site);
                }
                Resolved::Remote => unresolved.push(format!("{site} ({uri}) [remote]")),
                Resolved::Missing => {
                    unresolved.push(format!("{site} ({uri})"));
                    unresolved_local += 1;
                }
            }
        }
    }

    (bundle_assets, embedded, unresolved, unresolved_local)
}

/// Pack `doc` and the meshes supplied in `assets` into a self-contained `.hcdfz`, returning the bundle
/// bytes and a [`MemPackReport`]. Does NO filesystem access (wasm-capable; the disk counterpart is
/// [`crate::bundle::pack`]).
///
/// Steps: (1) FLATTEN `doc` via [`flatten_with`], resolving each `<include>` from `assets` (an include
/// with no bytes is left in place with a note; a cycle is an `Err`); (2) for each visual `<model @uri>`
/// and collision `<mesh @uri>` that is LOCAL (not `http(s)`) and present in `assets`, content-address the
/// bytes, copy them into the bundle's `assets/`, and rewrite the `@uri` to `assets/<name>` with the
/// matching `@sha`; (3) write a `ZIP_STORED` archive with the root `.hcdf` FIRST. A mesh uri NOT in
/// `assets` is left unrewritten and reported in [`MemPackReport::unresolved`]; when `allow_partial` is
/// false, any such site (LOCAL; remote sites are reported but never block) makes this return `Err` so a
/// caller cannot ship a bundle missing meshes by accident.
pub fn pack_to_bytes(
    doc: &Hcdf,
    assets: &MemBundle,
    allow_partial: bool,
) -> Result<(Vec<u8>, MemPackReport), String> {
    // 1) Flatten in memory, resolving includes from the same asset map. `base_dir` is irrelevant here
    //    (the loader matches uris verbatim against `assets`), so use an empty path.
    let mut doc = doc.clone();
    let mut loader =
        |key: &str, _base: &std::path::Path| -> Result<(Hcdf, Option<String>), String> {
            // `flatten_with` passes the resolved key (the include uri joined against base). Browser docs use
            // verbatim uris, so try the key first, then the bare uri form the doc carried.
            let bytes = assets
                .get(key)
                .or_else(|| {
                    // Fall back to the last path segment / a stripped form in case the key was lexically
                    // joined; an absolute-vs-relative mismatch falls through to "unresolved" with a note.
                    assets.get(key.trim_start_matches("./"))
                })
                .ok_or_else(|| "no bytes supplied for include".to_string())?;
            let text = std::str::from_utf8(bytes)
                .map_err(|e| format!("include bytes are not utf-8: {e}"))?;
            let sub = Hcdf::from_xml_str(text).map_err(|e| format!("include parse failed: {e}"))?;
            Ok((sub, Some(content_sha(bytes))))
        };
    let mut notes = flatten_with(&mut doc, std::path::Path::new(""), &mut loader)?;

    // 2) Rewrite every local mesh site whose bytes we have (shared with the keep-live packer). The
    //    rewrite works on a borrowed view so the mesh bytes stay the caller's, no clone per site.
    let (bundle_assets, embedded, unresolved, unresolved_local) =
        rewrite_local_mesh_sites(&mut doc, &mem_view(assets));

    if unresolved_local > 0 && !allow_partial {
        return Err(format!(
            "{unresolved_local} mesh asset(s) had no bytes supplied for the in-memory bundle: {}. \
             Pick/provide their bytes, or pass allow_partial=true to bundle anyway.",
            unresolved
                .iter()
                .filter(|s| !s.ends_with("[remote]"))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // 3) Serialize the root and write the STORED archive (root FIRST), exactly like `bundle::write_zip`.
    let root_name = root_name(&doc);
    let root_xml = doc
        .to_xml_string()
        .map_err(|e| format!("could not serialize bundle root document: {e}"))?;
    let root_bytes = root_xml.into_bytes();

    let mut entries: Vec<ZipEntry> = Vec::with_capacity(1 + bundle_assets.len());
    entries.push(ZipEntry {
        name: &root_name,
        data: &root_bytes,
    });
    // BTreeMap iteration is sorted/deterministic; prebuild the prefixed names so the borrows live. The
    // data slices still point into the caller's `assets` map: the zip writer reads them in place.
    let asset_named: Vec<(String, &[u8])> = bundle_assets
        .into_iter()
        .map(|(name, data)| (format!("{ASSETS_DIR}/{name}"), data))
        .collect();
    for (name, data) in &asset_named {
        entries.push(ZipEntry { name, data });
    }

    let mut out: Vec<u8> = Vec::new();
    let n = zip_store::write_stored(&mut out, &entries)
        .map_err(|e| format!("write in-memory bundle zip: {e}"))?;

    notes.sort();
    notes.dedup();
    Ok((
        out,
        MemPackReport {
            root: root_name,
            entries: n,
            embedded,
            unresolved,
            notes,
        },
    ))
}

/// One `<include>` module to EMBED in a keep-live bundle: the include `@uri` AS IT APPEARS in the parent
/// doc, the parsed module [`Hcdf`], and the module's LOCAL mesh bytes keyed by their MODULE-RELATIVE uri
/// (e.g. `("assets/wheel.glb", <bytes>)`). Produced by dendrite's `collect_modules` (which walks the doc's
/// includes recursively), consumed by [`pack_to_bytes_keep_live`].
pub struct KeepLiveModule {
    /// The include uri as written in the parent's `<include @uri>` (the key the rewrite map is built on).
    pub include_uri: String,
    /// The parsed module document (its own `<include>`s, if any, are rewritten through the same map).
    pub doc: Hcdf,
    /// The module's local meshes: `(module_relative_uri, bytes)`, normally `("assets/<name>", …)`.
    pub meshes: Vec<(String, Vec<u8>)>,
}

/// A confined, single-component module `.hcdf` stem from a document name: the trimmed name routed through
/// [`safe_root_stem`] (mirrors `bundle_mem::root_name`, without the extension), so only the portable
/// filename charset survives and a hostile module name cannot become a `modules/<usha>/<stem>.hcdf` zip
/// entry that carries a separator or a `..` traversal a foreign extractor would honor. Empty or all-dot
/// names fall back to `module`, so the result is always one non-dot component. Every safe-charset module
/// name (the whole current corpus) is left byte-for-byte unchanged.
fn module_file_stem(name: &str) -> String {
    safe_root_stem(name.trim(), "module")
}

/// The 12-hex-char short sha256 of `bytes`: the disambiguator a keep-live module directory
/// (`modules/<short>/…`) is named by. Derived from the full `sha256:<hex>` content sha.
fn short_sha(bytes: &[u8]) -> String {
    let sha = content_sha(bytes);
    let hex = sha.split_once(':').map_or(sha.as_str(), |(_, h)| h);
    hex.get(..12).unwrap_or(hex).to_string()
}

/// Pack `root` and the supplied `modules` into a NON-FLATTENING keep-live `.hcdfz`: the root document keeps
/// its `<include>`s INTACT (only their `@uri`s are rewritten to bundle-relative module paths), each
/// referenced module is EMBEDDED at `modules/<msha>/<stem>.hcdf` beside its meshes at
/// `modules/<msha>/assets/<name>`, and a `_keeplive` marker is written so an opener recognises the mode.
/// The counterpart to [`pack_to_bytes`] (which flattens); [`open_bundle_bytes`] restores the modules.
///
/// `root_meshes` supplies the ROOT doc's own local mesh bytes (keyed by the doc uri, exactly like
/// [`pack_to_bytes`]'s `assets`); each `modules[i].meshes` supplies that module's local mesh bytes keyed by
/// their module-relative uri. Every LOCAL mesh site (root + per module) is content-addressed into that
/// document's sibling `assets/`. Identical modules (same serialized bytes) dedupe to ONE `modules/<msha>`
/// entry. A LOCAL mesh with no bytes is left unrewritten and, when `allow_partial` is false, is an `Err`
/// (same contract as [`pack_to_bytes`]); a remote (`http(s)`) uri is reported but never blocks. The
/// returned [`MemPackReport::notes`] records how many modules were embedded.
pub fn pack_to_bytes_keep_live(
    root: &Hcdf,
    root_meshes: &MemBundle,
    modules: &[KeepLiveModule],
    allow_partial: bool,
) -> Result<(Vec<u8>, MemPackReport), String> {
    // 1) Give each DISTINCT include uri its OWN bundle-relative module path `modules/<usha>/<stem>.hcdf`,
    //    where `usha` is the short sha of the include URI (its module-store IDENTITY), NOT of the module's
    //    content. Two distinct live modules can be BYTE-IDENTICAL (e.g. an unedited fork and its parent share
    //    the same name + relative mesh uris) yet MUST stay INDEPENDENT across reload: content-keying would
    //    collapse them into one embedded entry, so a post-reload edit of one would silently mutate the other.
    //    Repeated references to the SAME include uri (already deduped upstream by dendrite's `collect_modules`)
    //    still share one entry, which is correct: they ARE the same live module.
    let mut uri_to_path: BTreeMap<String, String> = BTreeMap::new();
    // The module indices to emit, one per DISTINCT include uri, in `modules` order (deterministic).
    let mut ordered: Vec<usize> = Vec::new();
    for (i, m) in modules.iter().enumerate() {
        if uri_to_path.contains_key(&m.include_uri) {
            continue; // same live module referenced again, one embedded entry suffices
        }
        let usha = short_sha(m.include_uri.as_bytes());
        let stem = module_file_stem(&m.doc.name);
        uri_to_path.insert(
            m.include_uri.clone(),
            format!("{MODULES_DIR}/{usha}/{stem}.hcdf"),
        );
        ordered.push(i);
    }

    // 2) Rewrite the ROOT doc: keep <include>s, only repoint their @uris; content-address its own meshes.
    let mut root = root.clone();
    for inc in &mut root.include {
        if let Some(u) = inc.uri.as_deref() {
            if let Some(path) = uri_to_path.get(u) {
                inc.uri = Some(path.clone());
            }
        }
    }
    let (root_assets, mut embedded, mut unresolved, mut unresolved_local) =
        rewrite_local_mesh_sites(&mut root, &mem_view(root_meshes));

    // 3) First pass: REWRITE each unique module (repoint its <include> uris + content-address its meshes) but
    //    DO NOT serialize yet: the module's own nested-include `@sha`s must be stamped over the CHILDREN's
    //    final embedded bytes first (post-order), so serialization is deferred to the second pass.
    let mut rewritten: Vec<Hcdf> = Vec::with_capacity(ordered.len());
    let mut module_path: Vec<String> = Vec::with_capacity(ordered.len());
    let mut module_usha: Vec<String> = Vec::with_capacity(ordered.len());
    let mut massets: Vec<BundleAssets<'_>> = Vec::with_capacity(ordered.len());
    // bundle module path -> slot (index into `rewritten`), so a nested include resolves to its child slot.
    let mut path_to_slot: BTreeMap<String, usize> = BTreeMap::new();
    for (slot, &idx) in ordered.iter().enumerate() {
        let m = &modules[idx];
        let usha = short_sha(m.include_uri.as_bytes());
        let stem = module_file_stem(&m.doc.name);
        let mpath = format!("{MODULES_DIR}/{usha}/{stem}.hcdf");

        // Per-module mesh view keyed by the module-relative uri (bytes BORROWED from `m.meshes`),
        // reusing the shared site rewriter so the module file and its assets/ stay siblings
        // (`assets/<name>` uri ↔ `modules/<msha>/assets/<name>`).
        let mview: MemView<'_> = m
            .meshes
            .iter()
            .map(|(rel, bytes)| (rel.as_str(), bytes.as_slice()))
            .collect();
        let mut mdoc = m.doc.clone();
        // Rewrite the module's OWN <include>s (nested modules resolve to each other's bundle paths).
        for inc in &mut mdoc.include {
            if let Some(u) = inc.uri.as_deref() {
                if let Some(path) = uri_to_path.get(u) {
                    inc.uri = Some(path.clone());
                }
            }
        }
        let (masset, memb, munres, munres_local) = rewrite_local_mesh_sites(&mut mdoc, &mview);
        embedded.extend(memb.into_iter().map(|s| format!("{usha}/{s}")));
        unresolved.extend(munres);
        unresolved_local += munres_local;

        path_to_slot.insert(mpath.clone(), slot);
        rewritten.push(mdoc);
        module_path.push(mpath);
        module_usha.push(usha);
        massets.push(masset);
    }

    // Second pass: MEMOIZED POST-ORDER sha. Stamp each module's nested-include `@sha`s to their children's
    // final embedded sha, THEN serialize + hash the module. `memo[slot]` = the module's content sha,
    // `bytes_out[slot]` = the exact bytes written under `modules/<usha>/…` (so verify recomputes the SAME
    // bytes). `stamped_includes` counts the include `@sha`s set (module + root, for the report).
    let mut memo: Vec<Option<String>> = vec![None; ordered.len()];
    let mut bytes_out: Vec<Vec<u8>> = vec![Vec::new(); ordered.len()];
    let mut stamped_includes = 0usize;
    for slot in 0..ordered.len() {
        embedded_module_sha(
            slot,
            &rewritten,
            &path_to_slot,
            &mut memo,
            &mut bytes_out,
            &mut stamped_includes,
            &mut Vec::new(),
        )?;
    }

    if unresolved_local > 0 && !allow_partial {
        return Err(format!(
            "{unresolved_local} mesh asset(s) had no bytes supplied for the keep-live bundle: {}. \
             Pick/provide their bytes, or pass allow_partial=true to bundle anyway.",
            unresolved
                .iter()
                .filter(|s| !s.ends_with("[remote]"))
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // Stamp the ROOT's own <include> `@sha`s to the embedded sha of the module each points at (from the
    // memo), BEFORE serializing the root, so a keep-live root pins exactly the module entry it carries.
    for inc in &mut root.include {
        if let Some(u) = inc.uri.as_deref() {
            if let Some(&slot) = path_to_slot.get(u) {
                if let Some(sha) = memo[slot].clone() {
                    inc.sha = Some(sha);
                    stamped_includes += 1;
                }
            }
        }
    }

    // Emit the module entries: the stamped bytes (borrowed from `bytes_out`) + each module's
    // content-addressed meshes (still borrowed from the caller's `modules`, no clone on the way out).
    let mut module_entries: Vec<(String, &[u8])> = Vec::new(); // (entry_name, bytes)
    for slot in 0..ordered.len() {
        module_entries.push((module_path[slot].clone(), bytes_out[slot].as_slice()));
        let usha = &module_usha[slot];
        for (name, bytes) in &massets[slot] {
            module_entries.push((format!("{MODULES_DIR}/{usha}/{ASSETS_DIR}/{name}"), bytes));
        }
    }
    let embedded_modules = ordered.len();

    // 4) Write the STORED archive: root .hcdf FIRST (contract), then the keep-live marker, then the root's
    //    own assets/, then every module doc + module assets.
    let root_name = root_name(&root);
    let root_xml = root
        .to_xml_string()
        .map_err(|e| format!("could not serialize keep-live bundle root document: {e}"))?;
    let root_bytes = root_xml.into_bytes();

    let root_asset_named: Vec<(String, &[u8])> = root_assets
        .into_iter()
        .map(|(name, data)| (format!("{ASSETS_DIR}/{name}"), data))
        .collect();

    let mut entries: Vec<ZipEntry> =
        Vec::with_capacity(2 + root_asset_named.len() + module_entries.len());
    entries.push(ZipEntry {
        name: &root_name,
        data: &root_bytes,
    });
    entries.push(ZipEntry {
        name: KEEPLIVE_MARKER,
        data: b"1",
    });
    for (name, data) in &root_asset_named {
        entries.push(ZipEntry { name, data });
    }
    for (name, data) in &module_entries {
        entries.push(ZipEntry { name, data });
    }

    let mut out: Vec<u8> = Vec::new();
    let n = zip_store::write_stored(&mut out, &entries)
        .map_err(|e| format!("write keep-live bundle zip: {e}"))?;

    let mut notes = vec![
        format!("keep-live: {embedded_modules} module(s) embedded"),
        format!("keep-live: stamped {stamped_includes} include @sha(s)"),
    ];
    notes.sort();
    Ok((
        out,
        MemPackReport {
            root: root_name,
            entries: n,
            embedded,
            unresolved,
            notes,
        },
    ))
}

/// Post-order, memoized content sha of a keep-live module `slot` (index into `rewritten`): recursively
/// stamp the module's nested `<include> @sha`s to their CHILDREN's embedded sha first, THEN serialize +
/// hash this module. Writes the serialized bytes into `bytes_out[slot]` (the exact bytes the archive
/// entry carries) and caches the sha in `memo[slot]`; `stamped` counts each nested include `@sha` set.
/// A module cycle (a module that transitively includes itself) is an `Err`, mirroring [`flatten_with`].
fn embedded_module_sha(
    slot: usize,
    rewritten: &[Hcdf],
    path_to_slot: &BTreeMap<String, usize>,
    memo: &mut [Option<String>],
    bytes_out: &mut [Vec<u8>],
    stamped: &mut usize,
    cycle_stack: &mut Vec<usize>,
) -> Result<String, String> {
    if let Some(sha) = &memo[slot] {
        return Ok(sha.clone());
    }
    if cycle_stack.contains(&slot) {
        let mut chain = cycle_stack.clone();
        chain.push(slot);
        return Err(format!(
            "keep-live module include cycle detected: {chain:?}"
        ));
    }
    cycle_stack.push(slot);
    let mut mdoc = rewritten[slot].clone();
    for inc in &mut mdoc.include {
        if let Some(u) = inc.uri.as_deref() {
            if let Some(&child) = path_to_slot.get(u) {
                let child_sha = embedded_module_sha(
                    child,
                    rewritten,
                    path_to_slot,
                    memo,
                    bytes_out,
                    stamped,
                    cycle_stack,
                )?;
                inc.sha = Some(child_sha);
                *stamped += 1;
            }
        }
    }
    cycle_stack.pop();
    let bytes = mdoc
        .to_xml_string()
        .map_err(|e| format!("could not serialize keep-live module: {e}"))?
        .into_bytes();
    let sha = content_sha(&bytes);
    memo[slot] = Some(sha.clone());
    bytes_out[slot] = bytes;
    Ok(sha)
}

/// Open a self-contained `.hcdfz` bundle from its raw `bytes` IN MEMORY: parse the root [`Hcdf`] (the
/// FIRST archive entry, by the bundle contract) and return the remaining entries as the asset tree.
/// The wasm-capable counterpart to [`crate::bundle::open_bundle`]: it does NO filesystem access (uses
/// the wasm-clean [`zip_store`] reader), so it compiles + runs on native AND wasm32.
///
/// The returned [`OpenedMemBundle::assets`] are `(relative_path, bytes)` pairs whose `relative_path` is
/// the archive entry name verbatim (e.g. `assets/wheel_<sha>.glb`): exactly the document-relative `@uri`
/// the root doc carries, so a caller can populate a uri-keyed asset store with no path rewriting.
///
/// Errors when `bytes` is not a STORED zip ([`zip_store::is_zip`] is false), when a compressed entry is
/// present (the reader rejects non-STORED), when the archive has no entries, when the first entry is not a
/// `.hcdf` (the bundle root contract), or when the root `.hcdf` fails to parse.
pub fn open_bundle_bytes(bytes: &[u8]) -> Result<OpenedMemBundle, String> {
    if !zip_store::is_zip(bytes) {
        return Err("not an HCDF bundle (bytes are not a ZIP_STORED archive)".to_string());
    }
    let entries = zip_store::read_stored(bytes).map_err(|e| format!("read bundle zip: {e}"))?;
    let root = entries
        .first()
        .ok_or_else(|| "empty bundle zip (no entries)".to_string())?;
    if !root.name.ends_with(".hcdf") {
        return Err(format!(
            "bundle root is not the first zip entry (first entry is {:?})",
            root.name
        ));
    }
    let xml =
        std::str::from_utf8(&root.data).map_err(|e| format!("root .hcdf is not utf-8: {e}"))?;
    let doc = Hcdf::from_xml_str(xml).map_err(|e| format!("parse root .hcdf: {e}"))?;

    // Partition every entry AFTER the root. A FLAT bundle has only `assets/<name>` entries → they all land
    // in `assets` and `modules` stays empty (byte-identical to the pre-keep-live behaviour). A KEEP-LIVE
    // bundle additionally carries `modules/<msha>/<stem>.hcdf` (module docs) + `modules/<msha>/assets/<name>`
    // (module meshes) + a `_keeplive` marker; the module entries are grouped by their `modules/<msha>` dir
    // and surfaced in `modules`, the marker is dropped, and the root's own `assets/` tree stays in `assets`.
    let mut assets: Vec<(String, Vec<u8>)> = Vec::new();
    let mut module_docs: Vec<(String, String, Hcdf)> = Vec::new(); // (name, module_dir, doc)
                                                                   // Module meshes keyed by their `modules/<msha>` dir, MOVED in as the entries drain (archive order
                                                                   // within each module), so grouping below is a per-module map take, not an O(modules×meshes)
                                                                   // clone-scan.
    let mut module_meshes: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    for e in entries.into_iter().skip(1) {
        if e.name == KEEPLIVE_MARKER {
            continue; // sentinel, carries no content
        }
        let is_module_entry = e.name.starts_with(&format!("{MODULES_DIR}/"));
        if is_module_entry {
            let module_dir = module_dir_of(&e.name);
            if e.name.ends_with(".hcdf") {
                let mxml = std::str::from_utf8(&e.data)
                    .map_err(|x| format!("module {:?} is not utf-8: {x}", e.name))?;
                let mdoc = Hcdf::from_xml_str(mxml)
                    .map_err(|x| format!("parse module {:?}: {x}", e.name))?;
                module_docs.push((e.name, module_dir, mdoc));
            } else {
                // A module mesh: its module-relative uri strips the `modules/<msha>/` prefix.
                let rel = e
                    .name
                    .strip_prefix(&format!("{module_dir}/"))
                    .unwrap_or(&e.name)
                    .to_string();
                module_meshes
                    .entry(module_dir)
                    .or_default()
                    .push((rel, e.data));
            }
        } else {
            assets.push((e.name, e.data));
        }
    }
    // Hand each module doc the meshes grouped under its `modules/<msha>` dir (a map remove; moves).
    let modules = module_docs
        .into_iter()
        .map(|(name, dir, mdoc)| {
            let meshes = module_meshes.remove(&dir).unwrap_or_default();
            (name, mdoc, meshes)
        })
        .collect();
    Ok(OpenedMemBundle {
        doc,
        assets,
        modules,
    })
}

/// The `modules/<msha>` directory of a keep-live module entry name: its FIRST TWO path segments
/// (`modules/<msha>/<stem>.hcdf` and `modules/<msha>/assets/<name>` both yield `modules/<msha>`), so a
/// module doc and its meshes group under the same key. Falls back to the whole name if it has < 2 segments.
fn module_dir_of(name: &str) -> String {
    let mut it = name.split('/');
    match (it.next(), it.next()) {
        (Some(a), Some(b)) => format!("{a}/{b}"),
        _ => name.to_string(),
    }
}

/// The classification of a mesh site's uri against the caller's asset map (via its [`MemView`]).
enum Resolved<'a> {
    /// LOCAL uri with bytes supplied: embed them (borrowed straight from the caller's map).
    Embed(&'a [u8]),
    /// `http(s)://`: never fetched here (reported, never embedded).
    Remote,
    /// LOCAL uri with NO bytes supplied: left unrewritten.
    Missing,
}

/// Classify a mesh uri: remote (`http(s)`) ⇒ [`Resolved::Remote`]; else look up the bytes in `assets`
/// under the verbatim uri (then a `./`-stripped form) ⇒ [`Resolved::Embed`] / [`Resolved::Missing`].
fn resolve_site<'a>(uri: &str, assets: &MemView<'a>) -> Resolved<'a> {
    if is_remote(uri) {
        return Resolved::Remote;
    }
    match assets
        .get(uri)
        .or_else(|| assets.get(uri.trim_start_matches("./")))
    {
        Some(b) => Resolved::Embed(b),
        None => Resolved::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Comp, ModelRef, Visual, VisualAppearance};
    use crate::zip_store::read_stored;

    fn doc_with_visual_mesh(uri: &str) -> Hcdf {
        let mut h = Hcdf {
            name: "robot".to_string(),
            ..Default::default()
        };
        let mut comp = Comp {
            name: "wheel".to_string(),
            ..Default::default()
        };
        comp.visual.push(Visual {
            name: "v0".to_string(),
            appearance: VisualAppearance::Model {
                model: ModelRef {
                    uri: Some(uri.to_string()),
                    sha: None,
                    ..Default::default()
                },
                geometry: None,
            },
            ..Default::default()
        });
        h.comp.push(comp);
        h
    }

    #[test]
    fn root_name_sanitizes_hostile_doc_names() {
        // The in-memory packer's root entry name must be a confined single component too: a hostile
        // doc.name cannot become a zip entry name carrying a separator or a `..` traversal that a
        // foreign extractor would honor.
        let doc = Hcdf {
            name: "..\\..\\evil".to_string(),
            ..Default::default()
        };
        let name = root_name(&doc);
        assert_eq!(name, ".._.._evil.hcdf");
        assert!(!name.contains('/') && !name.contains('\\'));

        let doc = Hcdf {
            name: "../evil".to_string(),
            ..Default::default()
        };
        assert_eq!(root_name(&doc), ".._evil.hcdf");

        // A name that reduces to only dots falls back to the safe default.
        let doc = Hcdf {
            name: "..".to_string(),
            ..Default::default()
        };
        assert_eq!(root_name(&doc), "root.hcdf");

        // Safe-charset names are byte-stable (zero golden churn).
        let doc = Hcdf {
            name: "cogni-quadrotor".to_string(),
            ..Default::default()
        };
        assert_eq!(root_name(&doc), "cogni-quadrotor.hcdf");
    }

    #[test]
    fn module_file_stem_confines_hostile_names() {
        // A module stem is the tail of a `modules/<usha>/<stem>.hcdf` zip entry name, so a hostile
        // module doc.name must collapse to one safe filename component: no separator (`/` or `\`) may
        // survive, and a name reducing to only dots falls back rather than staying a `..` traversal.
        for hostile in ["..\\..\\evil", "../evil", "a/b\\c", "..", "   ", ""] {
            let stem = module_file_stem(hostile);
            assert!(!stem.contains('/'), "no forward slash in {stem:?}");
            assert!(!stem.contains('\\'), "no backslash in {stem:?}");
            assert!(
                !stem.chars().all(|c| c == '.'),
                "not an all-dot stem: {stem:?}"
            );
            assert_eq!(
                std::path::Path::new(&stem).components().count(),
                1,
                "single path component: {stem:?}"
            );
        }
        assert_eq!(module_file_stem("..\\..\\evil"), ".._.._evil");
        assert_eq!(module_file_stem("../evil"), ".._evil");
        assert_eq!(module_file_stem(".."), "module");
        assert_eq!(module_file_stem(""), "module");

        // Safe-charset names (the whole current corpus) stay byte-stable, trim included.
        assert_eq!(module_file_stem("wheelmod"), "wheelmod");
        assert_eq!(module_file_stem("  cogni-arm_v2  "), "cogni-arm_v2");
    }

    #[test]
    fn packs_root_first_and_embeds_supplied_mesh() {
        let doc = doc_with_visual_mesh("assets/wheel.glb");
        let mut assets = MemBundle::new();
        assets.insert("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec());

        let (bytes, report) = pack_to_bytes(&doc, &assets, false).expect("packs");
        assert!(zip_store::is_zip(&bytes));
        let entries = read_stored(&bytes).unwrap();
        // Root .hcdf is the FIRST entry (bundle contract).
        assert_eq!(entries[0].name, "robot.hcdf");
        // Exactly one asset entry, under assets/, content-addressed.
        assert_eq!(entries.len(), 2);
        assert!(entries[1].name.starts_with("assets/wheel_"));
        assert!(entries[1].name.ends_with(".glb"));
        assert_eq!(entries[1].data, b"GLB-BYTES");
        assert_eq!(report.entries, 2);
        assert_eq!(report.embedded, vec!["wheel/v0".to_string()]);
        assert!(report.unresolved.is_empty());

        // The root doc's uri was rewritten to the content-addressed assets path, with a matching sha.
        let root_xml = String::from_utf8(entries[0].data.clone()).unwrap();
        assert!(root_xml.contains("assets/wheel_"));
        assert!(root_xml.contains(&content_sha(b"GLB-BYTES")));
    }

    #[test]
    fn missing_local_mesh_errors_unless_allow_partial() {
        let doc = doc_with_visual_mesh("assets/missing.glb");
        let assets = MemBundle::new(); // no bytes supplied
        let err = pack_to_bytes(&doc, &assets, false).expect_err("missing mesh must error");
        assert!(err.contains("no bytes supplied"), "got: {err}");

        // allow_partial: bundles anyway, leaving the uri unrewritten and reporting it.
        let (_bytes, report) = pack_to_bytes(&doc, &assets, true).expect("allow_partial bundles");
        assert_eq!(report.unresolved.len(), 1);
        assert!(report.embedded.is_empty());
    }

    #[test]
    fn remote_mesh_is_reported_never_blocks() {
        let doc = doc_with_visual_mesh("https://example.com/wheel.glb");
        let assets = MemBundle::new();
        // A remote uri does not count as a local unresolved site, so it never blocks (allow_partial=false).
        let (_bytes, report) = pack_to_bytes(&doc, &assets, false).expect("remote never blocks");
        assert_eq!(report.unresolved.len(), 1);
        assert!(report.unresolved[0].ends_with("[remote]"));
    }

    #[test]
    fn pack_then_open_bundle_bytes_round_trips_doc_and_assets() {
        // Build a fixture with the in-memory packer, then open it back and assert the doc + assets match.
        let doc = doc_with_visual_mesh("assets/wheel.glb");
        let mut assets = MemBundle::new();
        assets.insert("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec());

        let (bytes, pack_report) = pack_to_bytes(&doc, &assets, false).expect("packs");
        let opened = open_bundle_bytes(&bytes).expect("opens");

        // The root doc round-trips: same name + comp count, and its uri is the content-addressed path.
        assert_eq!(opened.doc.name, "robot");
        assert_eq!(opened.doc.comp.len(), 1);
        let uri = match &opened.doc.comp[0].visual[0].appearance {
            VisualAppearance::Model { model, .. } => model.uri.clone().unwrap(),
            _ => panic!("expected a model visual"),
        };
        assert!(uri.starts_with("assets/wheel_"));
        assert!(uri.ends_with(".glb"));

        // The asset tree round-trips: exactly one asset, keyed by the SAME uri the doc carries, bytes intact.
        assert_eq!(opened.assets.len(), 1);
        assert_eq!(
            opened.assets[0].0, uri,
            "asset path must match the doc's rewritten @uri"
        );
        assert_eq!(opened.assets[0].1, b"GLB-BYTES");
        // Total entries reported by the packer = 1 root + 1 asset; open returns 1 asset (root is `doc`).
        assert_eq!(pack_report.entries, 1 + opened.assets.len());
    }

    #[test]
    fn open_bundle_bytes_rejects_non_zip_and_missing_root() {
        // Not a zip at all.
        let err = open_bundle_bytes(b"not a zip").expect_err("must reject non-zip");
        assert!(err.contains("not an HCDF bundle"), "got: {err}");

        // A STORED zip whose first entry is NOT a .hcdf is rejected (bundle root contract).
        let mut buf = Vec::new();
        zip_store::write_stored(
            &mut buf,
            &[zip_store::ZipEntry {
                name: "assets/x.glb",
                data: b"bytes",
            }],
        )
        .unwrap();
        let err = open_bundle_bytes(&buf).expect_err("must reject non-.hcdf first entry");
        assert!(
            err.contains("bundle root is not the first zip entry"),
            "got: {err}"
        );
    }

    /// A minimal module doc: one comp with one visual mesh at the given (module-relative) uri.
    fn module_doc(name: &str, mesh_uri: &str) -> Hcdf {
        let mut h = Hcdf {
            name: name.to_string(),
            ..Default::default()
        };
        let mut comp = Comp {
            name: "rotor".to_string(),
            ..Default::default()
        };
        comp.visual.push(Visual {
            name: "v0".to_string(),
            appearance: VisualAppearance::Model {
                model: ModelRef {
                    uri: Some(mesh_uri.to_string()),
                    sha: None,
                    ..Default::default()
                },
                geometry: None,
            },
            ..Default::default()
        });
        h.comp.push(comp);
        h
    }

    #[test]
    fn flat_pack_resolves_a_browser_include_from_the_augmented_map() {
        // A wasm-shaped augmented MemBundle: the root has an <include> at an ABSOLUTE synthetic uri, and the
        // map carries BOTH the module doc (keyed by that exact uri) AND the module's mesh keyed by its
        // rerooted absolute key `<module_dir>/assets/<name>`. pack_to_bytes must flatten it self-contained.
        let mut root = Hcdf {
            name: "assembly".to_string(),
            ..Default::default()
        };
        root.comp.push(Comp {
            name: "base".to_string(),
            ..Default::default()
        });
        root.include.push(crate::model::Include {
            uri: Some("/mem/0/m.hcdf".to_string()),
            name: Some("left".to_string()),
            sha: None,
            pose: None,
            ..Default::default()
        });

        let module = module_doc("wheelmod", "assets/wheel.glb");
        let mut assets = MemBundle::new();
        assets.insert(
            "/mem/0/m.hcdf".to_string(),
            module.to_xml_string().unwrap().into_bytes(),
        );
        assets.insert("/mem/0/assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec());

        let (bytes, _report) = pack_to_bytes(&root, &assets, true).expect("packs");
        let opened = open_bundle_bytes(&bytes).expect("opens");

        // FLAT: the include is gone, the module comp is present (prefixed), and its mesh is a bundle asset.
        assert!(
            opened.doc.include.is_empty(),
            "flat bundle must have no <include>"
        );
        assert!(opened.modules.is_empty(), "flat bundle exposes no modules");
        assert!(
            opened.doc.comp.iter().any(|c| c.name == "left/rotor"),
            "module comp should be flattened in with the instance prefix"
        );
        let uri = opened
            .doc
            .comp
            .iter()
            .find(|c| c.name == "left/rotor")
            .and_then(|c| match &c.visual[0].appearance {
                VisualAppearance::Model { model, .. } => model.uri.clone(),
                _ => None,
            })
            .expect("module comp has a model uri");
        assert!(
            uri.starts_with("assets/wheel_"),
            "module mesh rewritten to a bundle asset: {uri}"
        );
        let asset = opened
            .assets
            .iter()
            .find(|(p, _)| *p == uri)
            .expect("module mesh present");
        assert_eq!(asset.1, b"GLB-BYTES");
    }

    #[test]
    fn keep_live_pack_preserves_includes_and_embeds_modules() {
        let mut root = Hcdf {
            name: "assembly".to_string(),
            ..Default::default()
        };
        root.comp.push(Comp {
            name: "base".to_string(),
            ..Default::default()
        });
        root.include.push(crate::model::Include {
            uri: Some("/mem/0/m.hcdf".to_string()),
            name: Some("left".to_string()),
            sha: None,
            pose: None,
            ..Default::default()
        });

        let module = module_doc("wheelmod", "assets/wheel.glb");
        let km = KeepLiveModule {
            include_uri: "/mem/0/m.hcdf".to_string(),
            doc: module,
            meshes: vec![("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec())],
        };
        let root_meshes = MemBundle::new(); // root has no local meshes of its own
        let (bytes, report) =
            pack_to_bytes_keep_live(&root, &root_meshes, &[km], false).expect("keep-live packs");
        assert!(report
            .notes
            .iter()
            .any(|n| n.contains("1 module(s) embedded")));

        let opened = open_bundle_bytes(&bytes).expect("opens");
        // The root's <include> SURVIVES, repointed at the bundle-relative module path.
        assert_eq!(opened.doc.include.len(), 1, "keep-live keeps the <include>");
        let inc_uri = opened.doc.include[0].uri.clone().unwrap();
        assert!(
            inc_uri.starts_with("modules/") && inc_uri.ends_with(".hcdf"),
            "got {inc_uri}"
        );
        // The module is surfaced with its doc + meshes, keyed by that same bundle path.
        assert_eq!(opened.modules.len(), 1);
        let (muri, mdoc, mmeshes) = &opened.modules[0];
        assert_eq!(
            *muri, inc_uri,
            "root include points at the embedded module entry"
        );
        assert!(mdoc.comp.iter().any(|c| c.name == "rotor"));
        assert_eq!(mmeshes.len(), 1);
        assert!(
            mmeshes[0].0.starts_with("assets/wheel_"),
            "module mesh is module-relative: {}",
            mmeshes[0].0
        );
        assert_eq!(mmeshes[0].1, b"GLB-BYTES");

        // BACKWARD-COMPAT: a flat-only reader that ignores `.modules` still parses the root (root is FIRST).
        let raw = read_stored(&bytes).unwrap();
        assert!(raw[0].name.ends_with(".hcdf"), "root is the first entry");
        let root_again = Hcdf::from_xml_str(std::str::from_utf8(&raw[0].data).unwrap()).unwrap();
        assert_eq!(root_again.name, "assembly");
    }

    #[test]
    fn keep_live_unedited_fork_stays_independent_not_collapsed() {
        // Regression: an UNEDITED fork clones a module into a fresh store dir but leaves its XML
        // BYTE-IDENTICAL to the parent (same name, same relative mesh uris) until it is edited.
        // collect_modules yields TWO CollectedModules with DISTINCT include uris. The keep-live packer must
        // emit TWO independent embedded module entries (keyed by include-uri identity, not content sha), so
        // on reload the two instances restore to TWO distinct live modules; editing one must not touch the
        // other. Content-keying would collapse them into ONE entry (the bug).
        let mut root = Hcdf {
            name: "assembly".to_string(),
            ..Default::default()
        };
        root.include.push(crate::model::Include {
            uri: Some("/mem/0/m.hcdf".to_string()),
            name: Some("left".to_string()),
            sha: None,
            pose: None,
            ..Default::default()
        });
        root.include.push(crate::model::Include {
            uri: Some("/mem/1/m.hcdf".to_string()),
            name: Some("right".to_string()),
            sha: None,
            pose: None,
            ..Default::default()
        });

        // Two byte-identical modules (unedited fork) under DIFFERENT include uris.
        let make = |uri: &str| KeepLiveModule {
            include_uri: uri.to_string(),
            doc: module_doc("wheelmod", "assets/wheel.glb"),
            meshes: vec![("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec())],
        };
        let kl = vec![make("/mem/0/m.hcdf"), make("/mem/1/m.hcdf")];

        let root_meshes = MemBundle::new();
        let (bytes, report) =
            pack_to_bytes_keep_live(&root, &root_meshes, &kl, false).expect("keep-live packs");
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("2 module(s) embedded")),
            "both distinct forks must embed as separate modules: {:?}",
            report.notes
        );

        let opened = open_bundle_bytes(&bytes).expect("opens");
        // BOTH includes survive, each repointed at its OWN distinct bundle module path.
        assert_eq!(opened.doc.include.len(), 2);
        let p0 = opened.doc.include[0].uri.clone().unwrap();
        let p1 = opened.doc.include[1].uri.clone().unwrap();
        assert_ne!(
            p0, p1,
            "unedited fork must NOT collapse to a shared module path"
        );
        // Two independent restored module entries, one per include.
        assert_eq!(
            opened.modules.len(),
            2,
            "two independent live modules must be restored"
        );
        let m0 = &opened.modules[0].0;
        let m1 = &opened.modules[1].0;
        assert_ne!(m0, m1);
        assert!([p0.clone(), p1.clone()].contains(m0));
        assert!([p0, p1].contains(m1));
    }

    #[test]
    fn keep_live_pack_stamps_include_sha_to_embedded_module_content() {
        // The keep-live packer must SET the root `<include> @sha` to content_sha of the exact module bytes
        // it embeds, so verify recomputes the same value.
        let mut root = Hcdf {
            name: "assembly".to_string(),
            ..Default::default()
        };
        root.include.push(crate::model::Include {
            uri: Some("/mem/0/m.hcdf".to_string()),
            name: Some("left".to_string()),
            sha: None,
            pose: None,
            ..Default::default()
        });
        let km = KeepLiveModule {
            include_uri: "/mem/0/m.hcdf".to_string(),
            doc: module_doc("wheelmod", "assets/wheel.glb"),
            meshes: vec![("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec())],
        };
        let (bytes, report) =
            pack_to_bytes_keep_live(&root, &MemBundle::new(), &[km], false).expect("packs");
        assert!(
            report.notes.iter().any(|n| n.contains("stamped 1 include")),
            "{:?}",
            report.notes
        );

        let raw = read_stored(&bytes).unwrap();
        // The embedded module entry bytes.
        let module_entry = raw
            .iter()
            .find(|e| e.name.ends_with(".hcdf") && e.name.starts_with("modules/"))
            .unwrap();
        let expected = content_sha(&module_entry.data);
        let opened = open_bundle_bytes(&bytes).expect("opens");
        let inc_sha = opened.doc.include[0]
            .sha
            .clone()
            .expect("root include is stamped");
        assert_eq!(
            inc_sha, expected,
            "root @sha pins the embedded module's content sha"
        );
    }

    #[test]
    fn keep_live_pack_stamps_nested_include_recursively() {
        // root -> A -> B: A's embedded include @sha == content_sha(B's embedded bytes), and root's include
        // @sha == content_sha(A's embedded bytes). Transitive: a change to B flips both.
        let mut root = Hcdf {
            name: "assembly".to_string(),
            ..Default::default()
        };
        root.include.push(crate::model::Include {
            uri: Some("/mem/A/a.hcdf".to_string()),
            name: Some("a".to_string()),
            sha: None,
            pose: None,
            ..Default::default()
        });
        // Module A includes module B.
        let mut a = Hcdf {
            name: "modA".to_string(),
            ..Default::default()
        };
        a.comp.push(Comp {
            name: "acomp".to_string(),
            ..Default::default()
        });
        a.include.push(crate::model::Include {
            uri: Some("/mem/B/b.hcdf".to_string()),
            name: Some("b".to_string()),
            sha: None,
            pose: None,
            ..Default::default()
        });
        let b = module_doc("modB", "assets/bb.glb");
        let kl = vec![
            KeepLiveModule {
                include_uri: "/mem/A/a.hcdf".to_string(),
                doc: a,
                meshes: vec![],
            },
            KeepLiveModule {
                include_uri: "/mem/B/b.hcdf".to_string(),
                doc: b,
                meshes: vec![("assets/bb.glb".to_string(), b"BB".to_vec())],
            },
        ];
        let (bytes, _r) =
            pack_to_bytes_keep_live(&root, &MemBundle::new(), &kl, false).expect("packs");
        let raw = read_stored(&bytes).unwrap();
        let a_bytes = &raw
            .iter()
            .find(|e| e.name.contains("modA") && e.name.ends_with(".hcdf"))
            .unwrap()
            .data;
        let b_bytes = &raw
            .iter()
            .find(|e| e.name.contains("modB") && e.name.ends_with(".hcdf"))
            .unwrap()
            .data;
        let a_sha = content_sha(a_bytes);
        let b_sha = content_sha(b_bytes);

        // A's embedded include (to B) is stamped with B's content sha.
        let a_doc = Hcdf::from_xml_str(std::str::from_utf8(a_bytes).unwrap()).unwrap();
        assert_eq!(
            a_doc.include[0].sha.as_deref(),
            Some(b_sha.as_str()),
            "A pins B's embedded sha"
        );
        // Root's include (to A) is stamped with A's content sha.
        let opened = open_bundle_bytes(&bytes).expect("opens");
        assert_eq!(
            opened.doc.include[0].sha.as_deref(),
            Some(a_sha.as_str()),
            "root pins A's embedded sha"
        );
    }

    #[test]
    fn verify_include_shas_dual_accepts_and_catches_drift() {
        use crate::compose::{content_sha_of_module, verify_include_shas};
        let module = module_doc("m", "assets/w.glb");
        let canon = content_sha_of_module(&module).unwrap();

        let mut doc = Hcdf {
            name: "root".to_string(),
            ..Default::default()
        };
        doc.include.push(crate::model::Include {
            uri: Some("m.hcdf".to_string()),
            name: Some("L".to_string()),
            sha: Some(canon.clone()),
            pose: None,
            ..Default::default()
        });
        // A loader that returns the module with NO raw sha (wasm-store shape) → canonical branch matches.
        let mut load = |_k: &str, _b: &std::path::Path| Ok((module.clone(), None));
        assert!(
            verify_include_shas(&doc, std::path::Path::new(""), &mut load).is_empty(),
            "canonical pin is clean"
        );

        // A drifted pin is reported with actual = the canonical sha.
        doc.include[0].sha = Some(format!("sha256:{}", "0".repeat(64)));
        let mm = verify_include_shas(&doc, std::path::Path::new(""), &mut load);
        assert_eq!(mm.len(), 1);
        assert_eq!(mm[0].actual.as_deref(), Some(canon.as_str()));

        // A missing module (loader Err) with a pinned include → actual None.
        let mut miss = |_k: &str, _b: &std::path::Path| {
            Err::<(Hcdf, Option<String>), String>("nope".to_string())
        };
        let mm = verify_include_shas(&doc, std::path::Path::new(""), &mut miss);
        assert_eq!(mm.len(), 1);
        assert!(mm[0].actual.is_none(), "missing module => actual None");
    }

    #[test]
    fn flat_bundle_opens_with_no_modules_backward_compat() {
        // An OLD-shape flat bundle (produced by pack_to_bytes) must open with an EMPTY modules list.
        let doc = doc_with_visual_mesh("assets/wheel.glb");
        let mut assets = MemBundle::new();
        assets.insert("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec());
        let (bytes, _) = pack_to_bytes(&doc, &assets, false).expect("packs");
        let opened = open_bundle_bytes(&bytes).expect("opens");
        assert!(
            opened.modules.is_empty(),
            "a flat bundle exposes no modules"
        );
        assert_eq!(opened.assets.len(), 1);
    }

    #[test]
    fn pack_to_bytes_is_byte_deterministic() {
        // Both packers must yield BYTE-IDENTICAL output for the same input (BTreeMap ordering, stored
        // entries, no timestamps): the determinism the sha-pinned ecosystem relies on.
        let doc = doc_with_visual_mesh("assets/wheel.glb");
        let mut assets = MemBundle::new();
        assets.insert("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec());
        let (a, _) = pack_to_bytes(&doc, &assets, false).expect("packs");
        let (b, _) = pack_to_bytes(&doc, &assets, false).expect("packs again");
        assert_eq!(a, b, "flat pack must be byte-deterministic");

        let mut root = Hcdf {
            name: "assembly".to_string(),
            ..Default::default()
        };
        root.include.push(crate::model::Include {
            uri: Some("/mem/0/m.hcdf".to_string()),
            name: Some("left".to_string()),
            sha: None,
            pose: None,
            ..Default::default()
        });
        let make = || KeepLiveModule {
            include_uri: "/mem/0/m.hcdf".to_string(),
            doc: module_doc("wheelmod", "assets/wheel.glb"),
            meshes: vec![("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec())],
        };
        let (ka, _) =
            pack_to_bytes_keep_live(&root, &MemBundle::new(), &[make()], false).expect("packs");
        let (kb, _) = pack_to_bytes_keep_live(&root, &MemBundle::new(), &[make()], false)
            .expect("packs again");
        assert_eq!(ka, kb, "keep-live pack must be byte-deterministic");
    }

    #[test]
    fn identical_bytes_dedupe_to_one_entry() {
        let mut doc = doc_with_visual_mesh("assets/a.glb");
        // Second comp referencing different uri but identical bytes content-addresses to the same name.
        let mut comp2 = Comp {
            name: "wheel2".to_string(),
            ..Default::default()
        };
        comp2.visual.push(Visual {
            name: "v0".to_string(),
            appearance: VisualAppearance::Model {
                model: ModelRef {
                    uri: Some("assets/a.glb".to_string()),
                    sha: None,
                    ..Default::default()
                },
                geometry: None,
            },
            ..Default::default()
        });
        doc.comp.push(comp2);

        let mut assets = MemBundle::new();
        assets.insert("assets/a.glb".to_string(), b"SAME".to_vec());
        let (bytes, report) = pack_to_bytes(&doc, &assets, false).expect("packs");
        let entries = read_stored(&bytes).unwrap();
        // 1 root + 1 deduped asset (two sites, identical bytes).
        assert_eq!(entries.len(), 2);
        assert_eq!(report.embedded.len(), 2);
    }
}
