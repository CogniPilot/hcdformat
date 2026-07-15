//! PyO3 binding exposing the canonical Rust `hcdformat` core to Python. The extension IS the `hcdf`
//! package: it ships as `hcdf/__init__.abi3.so`, so `import hcdf` loads it directly.
//!
//! Rust is canonical. The public `hcdf.*` surface (the `dom`/`urdf`/`sdf`/`io`/`assets` submodules, the
//! report types, and the console `main`) lives in the `surface` module; it hands the write-through DOM
//! handle (`PyHcdf`) to callers and routes behaviour through the core. The module-level `#[pyfunction]`s
//! in this file are the INTERNAL string-boundary entries the surface and the CLI build on: each takes
//! and returns plain data (str / bytes / list / tuple / dict) around the HCDF XML interchange, so the
//! core round-trips every document through [`Hcdf::from_xml_str`] / [`Hcdf::to_xml_string`].

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};
use std::path::Path;

use hcdformat::compose::flatten_path_fetch as rs_flatten_path_fetch;
use hcdformat::flatten_fetch as rs_flatten_fetch;
use hcdformat::resolve_uri as rs_resolve_uri;
use hcdformat::{
    bake, check_profile as rs_check_profile, content_sha as rs_content_sha,
    fetch_remote as rs_fetch_remote, flatten as rs_flatten_doc, flatten_path,
    from_sdf::from_sdf_str, from_sdf::from_sdf_str_with_assets, from_urdf::from_urdf_str,
    from_urdf::from_urdf_str_with_assets, profile::Tier, stamp_include_shas,
    to_sdf::to_sdf as rs_to_sdf, to_urdf::to_urdf as rs_to_urdf, validate::Level,
    validate_coverage, validate_enums as rs_validate_enums, validate_loops, validate_network,
    validate_semantic, validate_structural as rs_validate_structural,
    validate_xsd as rs_validate_xsd, vendor_assets as rs_vendor_assets, Hcdf, VisualAssetHint,
};
use hcdformat::{
    open_bundle as rs_open_bundle, open_bundle_bytes as rs_open_bundle_bytes, pack as rs_pack,
    pack_to_bytes as rs_pack_to_bytes, verify as rs_verify_bundle, verify_bytes as rs_verify_bytes,
    MemBundle, PackOptions, RemotePolicy,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

// Write-through DOM handles (PyHcdf/PyComp/PyVisual/PySensor). They prove out the pyo3 aliasing
// model the `hcdformat-pyderive` macro generalizes. pyo3 lives ONLY in this binding
// crate; `hcdformat-rs` stays wasm-clean.
mod dom;

// The public `hcdf.*` submodule surface (dom / urdf / sdf / io / assets), the report types, and the
// console `main()`. The flat pyfunctions below are the internal extension entries these build on.
mod surface;

/// Map any crate error into a Python `ValueError` with the crate's message.
fn err<E: std::fmt::Display>(e: E) -> PyErr {
    PyValueError::new_err(e.to_string())
}

// ── document round-trip ──────────────────────────────────────────────────────────────────────────

/// Parse an HCDF document and re-serialize it canonically. The shim uses this as the parse/serialize
/// authority (the round-tripped XML is then loaded into the Python passive DOM by the shim's loader).
#[pyfunction]
fn canonicalize_xml(src: &str) -> PyResult<String> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    doc.to_xml_string().map_err(err)
}

/// True if the bytes/str parse as an official-format HCDF document (used by the shim's parse path).
#[pyfunction]
fn parse_ok(src: &str) -> bool {
    Hcdf::from_xml_str(src).is_ok()
}

// ── validation ───────────────────────────────────────────────────────────────────────────────────

fn issues_to_py(py: Python<'_>, issues: &[hcdformat::validate::Issue]) -> PyResult<Py<PyList>> {
    let out = PyList::empty(py);
    for i in issues {
        let level = match i.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        out.append((level, i.code.clone(), i.message.clone()))?;
    }
    Ok(out.into())
}

/// Semantic validator (the companion rules XSD 1.0 cannot express). Returns a list of
/// `(level, code, message)` tuples, one per validator finding.
#[pyfunction]
fn validate_doc(py: Python<'_>, src: &str) -> PyResult<Py<PyList>> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    // Enum-value validation (the 13 model-untyped slots) runs off the raw XML; the typed-load already
    // rejects the 38 retyped enum fields, so a doc that parsed cannot carry a bad value there.
    let mut issues = rs_validate_enums(src).map_err(err)?;
    issues.extend(validate_semantic(&doc));
    // The comms/network validators (referential integrity, uniqueness, port-type/protocol/casing) run
    // in the same pass as the kinematic ones, with no Python oracle, but they are part of `validate`.
    issues.extend(validate_network(&doc));
    // The loop-closure validators (predecessor/successor refs, `<constraint-axes>` mask shape, body
    // agreement) run in the same pass too; likewise no Python oracle, but part of `validate`.
    issues.extend(validate_loops(&doc));
    // The schema-coverage validators (self-collision pair refs, transmission endpoint refs, motor
    // sensor refs, pin duplicate/carrier) run in the same pass too, with no Python oracle, part of
    // `validate`.
    issues.extend(validate_coverage(&doc));
    issues_to_py(py, &issues)
}

/// Pure-Rust XSD schema-SHAPE validator against the embedded frozen `hcdf.xsd`: the in-crate
/// replacement for the lxml `hcdf validate --xsd` shell-out. Returns a list of `(level, code, message)`
/// tuples (empty ⇒ schema-VALID); each message carries the `line:col: message` the CLI prints. Catches
/// the schema-shape errors the typed parse silently swallows (a `<box>` without a `<size>` child, a
/// `<color>` with an `<rgba>` child instead of the `rgba=` attribute). Not-well-formed XML surfaces as a
/// single `E_XSD` issue (a reject, matching lxml).
#[pyfunction]
fn validate_xsd(py: Python<'_>, src: &str) -> PyResult<Py<PyList>> {
    let issues = rs_validate_xsd(src);
    issues_to_py(py, &issues)
}

// ── HCDF XML <-> JSON ──────────────────────────────────────────────────────────────────────────────

/// Convert an HCDF XML document string to its JSON view (a single-key `{"hcdf": ...}` object), reproducing
/// `hcdf/convert.py` byte-for-byte so existing `.json` files stay compatible. The shim's `hcdf_io.to_json`
/// routes through this once Python is retired.
#[pyfunction]
fn hcdf_to_json(src: &str) -> PyResult<String> {
    hcdformat::hcdf_xml_to_json(src).map_err(err)
}

/// Convert an HCDF JSON document string back to canonical HCDF XML, XSD-driven (attribute vs child-element
/// resolution from the embedded frozen schema), the inverse of [`hcdf_to_json`]. The shim's
/// `hcdf_io.from_json` routes through this (then loads the XML into the typed DOM).
#[pyfunction]
fn hcdf_from_json(src: &str) -> PyResult<String> {
    hcdformat::json_to_hcdf_xml(src).map_err(err)
}

// ── URDF <-> HCDF ────────────────────────────────────────────────────────────────────────────────

/// Import URDF -> (canonical HCDF XML, notes). The shim loads the XML into the Python DOM.
#[pyfunction]
fn from_urdf(src: &str) -> PyResult<(String, Vec<String>)> {
    let (doc, notes) = from_urdf_str(src).map_err(err)?;
    Ok((doc.to_xml_string().map_err(err)?, notes))
}

/// One per-visual asset-bake hint as plain data the shim turns into a baker call: `(comp, visual,
/// scale, color, texture)`, the already-resolved mesh scale + flat colour + diffuse texture the
/// [`VisualAssetHint`] side-channel carries (an HCDF visual `<model>` stores none of these; they bake
/// into the GLB). Returning native tuples keeps the `Python` token off the arg list.
type AssetHintRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn hints_to_rows(hints: Vec<VisualAssetHint>) -> Vec<AssetHintRow> {
    hints
        .into_iter()
        .map(|h| (h.comp, h.visual, h.scale, h.color, h.texture))
        .collect()
}

/// Import URDF -> (canonical HCDF XML, notes, per-visual asset-bake hints). The DOM-coupled import-time
/// baker path: the shim loads the XML, then folds each hint's `scale`/`color` into the visual's GLB via
/// its `assets.Baker` (and bakes collision meshes off the DOM's own `<mesh scale>`), so HCDF never
/// carries a source scale. `from_urdf` (no hints) is the fast path when no baker is requested.
#[pyfunction]
fn from_urdf_with_assets(src: &str) -> PyResult<(String, Vec<String>, Vec<AssetHintRow>)> {
    let (doc, notes, hints) = from_urdf_str_with_assets(src).map_err(err)?;
    Ok((
        doc.to_xml_string().map_err(err)?,
        notes,
        hints_to_rows(hints),
    ))
}

/// Export HCDF XML -> (URDF XML string, loss items). loss items: list of `(category, detail)`.
#[pyfunction]
fn to_urdf(src: &str) -> PyResult<(String, Vec<(String, String)>)> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    let (xml, loss) = rs_to_urdf(&doc).map_err(err)?;
    Ok((xml, loss.items.clone()))
}

// ── SDF <-> HCDF ─────────────────────────────────────────────────────────────────────────────────

/// Import SDF -> (canonical HCDF XML, notes).
#[pyfunction]
fn from_sdf(src: &str) -> PyResult<(String, Vec<String>)> {
    let (doc, notes) = from_sdf_str(src).map_err(err)?;
    Ok((doc.to_xml_string().map_err(err)?, notes))
}

/// Import SDF -> (canonical HCDF XML, notes, per-visual asset-bake hints). The import-time baker path
/// (see [`from_urdf_with_assets`]); `from_sdf` (no hints) is the fast path when no baker is requested.
#[pyfunction]
fn from_sdf_with_assets(src: &str) -> PyResult<(String, Vec<String>, Vec<AssetHintRow>)> {
    let (doc, notes, hints) = from_sdf_str_with_assets(src).map_err(err)?;
    Ok((
        doc.to_xml_string().map_err(err)?,
        notes,
        hints_to_rows(hints),
    ))
}

/// Export HCDF XML -> (SDF XML string, loss items).
#[pyfunction]
fn to_sdf(src: &str) -> PyResult<(String, Vec<(String, String)>)> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    let (xml, loss) = rs_to_sdf(&doc).map_err(err)?;
    Ok((xml, loss.items.clone()))
}

// ── xacro -> URDF ────────────────────────────────────────────────────────────────────────────────

/// Expand a .xacro file path -> a URDF string with the FULLY pure-Rust engine (no subprocess).
/// `packages` maps package names to dirs (for `$(find pkg)`). `mappings` seeds the `$(arg name)`
/// substitution table (the `name:=value` overrides canonical `xacro` takes on the command line):
/// each entry overrides that arg's `<xacro:arg>` default, and an empty or absent mapping expands with
/// the document's own arg defaults.
#[pyfunction]
#[pyo3(signature = (path, mappings=None, packages=None))]
#[cfg(not(target_arch = "wasm32"))]
fn expand_xacro_path(
    path: &str,
    mappings: Option<std::collections::BTreeMap<String, String>>,
    packages: Option<std::collections::HashMap<String, String>>,
) -> PyResult<String> {
    let pkg = packages.unwrap_or_default();
    let args: std::collections::HashMap<String, String> =
        mappings.unwrap_or_default().into_iter().collect();
    hcdformat::expand_xacro_with_args(Path::new(path), &pkg, &args).map_err(err)
}

/// Always true: the pure-Rust engine ships inside the binding, so xacro expansion needs no external
/// tool (the Python `xacro.available()` contract, which used to probe for a `xacro` subprocess).
#[pyfunction]
#[cfg(not(target_arch = "wasm32"))]
fn xacro_available() -> bool {
    true
}

// ── profile ──────────────────────────────────────────────────────────────────────────────────────

/// Classify HCDF XML against the HCDF-URDF Profile 1.0. Returns a dict the shim turns into a
/// `urdf_hcdf.profile.ProfileReport` (keeping the Python tier labels + markdown/to_json formatting):
///   classification: str, findings: [(tier, code, detail)], loss: [(category, detail)],
///   issues: [(level, code, message)]
#[pyfunction]
fn check_profile(py: Python<'_>, src: &str) -> PyResult<Py<pyo3::types::PyDict>> {
    use pyo3::types::PyDict;
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    let rep = rs_check_profile(&doc);
    let tier_label = |t: Tier| t.label().to_string();
    let d = PyDict::new(py);
    d.set_item("classification", tier_label(rep.classification))?;
    let findings = PyList::empty(py);
    for f in &rep.findings {
        findings.append((tier_label(f.tier), f.code.clone(), f.detail.clone()))?;
    }
    d.set_item("findings", findings)?;
    let loss = PyList::empty(py);
    for (cat, detail) in &rep.loss.items {
        loss.append((cat.clone(), detail.clone()))?;
    }
    d.set_item("loss", loss)?;
    d.set_item("issues", issues_to_py(py, &rep.issues)?)?;
    Ok(d.into())
}

// ── report renders ─────────────────────────────────────────────────────────────────────────────────
//
// The canonical renders of the profile report and the URDF-export loss manifest (flat text, grouped
// markdown, pretty JSON) live in the core; these expose them so a Python caller renders the SAME bytes
// the `hcdf` CLI prints, instead of formatting the structured `check_profile`/`to_urdf` data itself.

/// Render the HCDF-URDF profile report for a document as markdown (Python `ProfileReport.markdown()`).
#[pyfunction]
fn profile_markdown(src: &str) -> PyResult<String> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    Ok(rs_check_profile(&doc).markdown())
}

/// Render the HCDF-URDF profile report for a document as JSON (Python `ProfileReport.to_json()`).
#[pyfunction]
fn profile_json(src: &str) -> PyResult<String> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    Ok(rs_check_profile(&doc).to_json())
}

/// Render a document's URDF-export loss manifest as flat text (Python `LossManifest.text()`).
#[pyfunction]
fn loss_text(src: &str) -> PyResult<String> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    Ok(rs_to_urdf(&doc).map_err(err)?.1.text())
}

/// Render a document's URDF-export loss manifest as JSON (Python `LossManifest.to_json()`).
#[pyfunction]
fn loss_json(src: &str) -> PyResult<String> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    Ok(rs_to_urdf(&doc).map_err(err)?.1.to_json())
}

/// Render a document's URDF-export loss manifest as markdown (Python `LossManifest.markdown(title)`);
/// `title` defaults to the Python default heading when omitted.
#[pyfunction]
#[pyo3(signature = (src, title=None))]
fn loss_markdown(src: &str, title: Option<&str>) -> PyResult<String> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    let loss = rs_to_urdf(&doc).map_err(err)?.1;
    Ok(loss.markdown(title.unwrap_or(hcdformat::to_urdf::DEFAULT_LOSS_TITLE)))
}

// ── compose: flatten / stamp / content_sha ────────────────────────────────────────────────────────

/// Resolve `<include>` composition for a file on disk -> (flattened HCDF XML, notes).
#[pyfunction]
fn flatten_file(path: &str) -> PyResult<(String, Vec<String>)> {
    let (doc, notes) = flatten_path(Path::new(path)).map_err(err)?;
    Ok((doc.to_xml_string().map_err(err)?, notes))
}

/// Resolve `<include>` composition for an IN-MEMORY document -> (flattened HCDF XML, notes). Takes the
/// document XML STRING and resolves LOCAL `<include>`s off `base_dir` on disk (no temp file for the root
/// document): the string entry point for `hcdf_io.include.flatten` when it holds an `Hcdf` rather than a
/// path. Releases the GIL for the include-file reads.
#[pyfunction]
fn flatten_doc(
    py: Python<'_>,
    doc_xml: String,
    base_dir: String,
) -> PyResult<(String, Vec<String>)> {
    py.allow_threads(|| -> Result<(String, Vec<String>), String> {
        let mut doc = Hcdf::from_xml_str(&doc_xml).map_err(|e| e.to_string())?;
        let notes = rs_flatten_doc(&mut doc, Path::new(&base_dir))?;
        Ok((doc.to_xml_string().map_err(|e| e.to_string())?, notes))
    })
    .map_err(err)
}

/// Pin every `<include>`'s @sha against its module's current bytes -> (stamped HCDF XML, notes).
#[pyfunction]
fn stamp_shas(src: &str, base_dir: &str) -> PyResult<(String, Vec<String>)> {
    let mut doc = Hcdf::from_xml_str(src).map_err(err)?;
    let notes = stamp_include_shas(&mut doc, Path::new(base_dir)).map_err(err)?;
    Ok((doc.to_xml_string().map_err(err)?, notes))
}

/// `"sha256:<hex>"` over the given bytes (the stable content address).
#[pyfunction]
fn content_sha(data: &[u8]) -> String {
    rs_content_sha(data)
}

/// Resolve a mesh `uri` to an existing filesystem path, or `None`. Supports `file://`,
/// `package://<pkg>/<rest>` (via `package_paths`), `model://<model>/<rest>` (`package_paths` then the
/// GZ/Gazebo resource-path env vars), absolute paths, and paths relative to `base_dir`: the canonical
/// port of `hcdf.assets.resolve_uri`. The shim delegates to this so mesh resolution has one authority.
#[pyfunction]
#[pyo3(signature = (uri, base_dir=".", package_paths=None))]
fn resolve_uri(
    uri: &str,
    base_dir: &str,
    package_paths: Option<BTreeMap<String, String>>,
) -> Option<String> {
    let pkgs: BTreeMap<String, PathBuf> = package_paths
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, PathBuf::from(v)))
        .collect();
    rs_resolve_uri(uri, Path::new(base_dir), &pkgs).map(|p| p.to_string_lossy().into_owned())
}

/// Structural required-attribute validation over the typed model (the `@name`/`@domain`-required rules).
/// Returns the list of structural issue messages; EMPTY ⇒ structurally valid. The shim uses this so the
/// required-attribute gate has a binding authority that survives the pure-Python retirement.
#[pyfunction]
fn validate_structural(src: &str) -> PyResult<Vec<String>> {
    let doc = Hcdf::from_xml_str(src).map_err(err)?;
    Ok(rs_validate_structural(&doc).err().unwrap_or_default())
}

// ── asset baker (CONVERSION half; pure-Rust GLB/lean writer) ──────────────────────────────────────

/// Bake a visual mesh -> canonical GLB bytes (scale/color baked into the vertices).
#[pyfunction]
#[pyo3(signature = (src_path, scale=None, color=None))]
fn bake_to_glb(
    py: Python<'_>,
    src_path: &str,
    scale: Option<&str>,
    color: Option<&str>,
) -> PyResult<Py<PyBytes>> {
    let data = bake::to_glb(src_path, scale, color).map_err(err)?;
    Ok(PyBytes::new(py, &data).into())
}

/// Bake a collision mesh -> canonical lean binary STL bytes (scale baked into the vertices).
#[pyfunction]
#[pyo3(signature = (src_path, scale=None))]
fn bake_to_lean(py: Python<'_>, src_path: &str, scale: Option<&str>) -> PyResult<Py<PyBytes>> {
    let data = bake::to_lean(src_path, scale).map_err(err)?;
    Ok(PyBytes::new(py, &data).into())
}

// ── import-then-bake (the whole-document mesh-bake orchestration reaches Python) ───────────────────

/// Import URDF text and bake every referenced mesh in ONE call -> `(baked HCDF XML, notes)`. The canonical
/// Rust port of the Python `urdf_hcdf.from_urdf(..., baker=...)` composition, done end to end in the core:
/// resolve + bake each visual/collision mesh off the importer's asset-hint side-channel (scale/mirror/colour/
/// texture folded into a canonical GLB or lean STL) into `assets_out_dir`, rewrite each `@uri`/`@sha` to
/// `"<uri_prefix>/<name>"`, and return the rewritten document plus the notes (the per-visual deferral notes
/// for the visuals that actually baked are dropped, keyed on the `(comp, visual)` pair; the walk's own
/// resolution/appearance fallbacks are kept). `base_dir` resolves relative/`file://`/`package://` mesh uris;
/// `uri_prefix` (e.g. `"assets"`) makes each rewritten `@uri` relative to the output document; `package_paths`
/// maps `package://`/`model://` roots. Releases the GIL for the filesystem I/O.
#[pyfunction]
#[pyo3(signature = (src, base_dir, assets_out_dir, uri_prefix, package_paths=None))]
fn from_urdf_with_baking(
    py: Python<'_>,
    src: &str,
    base_dir: &str,
    assets_out_dir: &str,
    uri_prefix: &str,
    package_paths: Option<BTreeMap<String, String>>,
) -> PyResult<(String, Vec<String>)> {
    import_with_baking(
        py,
        ImportFormat::Urdf,
        src,
        base_dir,
        assets_out_dir,
        uri_prefix,
        package_paths,
    )
}

/// Import SDF text and bake every referenced mesh in ONE call -> `(baked HCDF XML, notes)`, the SDF twin of
/// [`from_urdf_with_baking`] (see it for the argument/note semantics). The SDF side-channel additionally
/// carries a textured primitive's albedo map, so a textured `<plane>`/`<box>` synthesizes a UV-mapped GLB and
/// its visual becomes an ordinary `<model>`. Releases the GIL for the filesystem I/O.
#[pyfunction]
#[pyo3(signature = (src, base_dir, assets_out_dir, uri_prefix, package_paths=None))]
fn from_sdf_with_baking(
    py: Python<'_>,
    src: &str,
    base_dir: &str,
    assets_out_dir: &str,
    uri_prefix: &str,
    package_paths: Option<BTreeMap<String, String>>,
) -> PyResult<(String, Vec<String>)> {
    import_with_baking(
        py,
        ImportFormat::Sdf,
        src,
        base_dir,
        assets_out_dir,
        uri_prefix,
        package_paths,
    )
}

/// Which importer an [`import_with_baking`] call drives.
enum ImportFormat {
    Urdf,
    Sdf,
}

/// Shared body of `from_urdf_with_baking` / `from_sdf_with_baking`: own every input before releasing the GIL
/// (the closure must not touch Python-borrowed data), run the core import-then-bake, and serialize the result.
fn import_with_baking(
    py: Python<'_>,
    fmt: ImportFormat,
    src: &str,
    base_dir: &str,
    assets_out_dir: &str,
    uri_prefix: &str,
    package_paths: Option<BTreeMap<String, String>>,
) -> PyResult<(String, Vec<String>)> {
    let pkgs: BTreeMap<String, PathBuf> = package_paths
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, PathBuf::from(v)))
        .collect();
    let src = src.to_string();
    let base = PathBuf::from(base_dir);
    let out = PathBuf::from(assets_out_dir);
    let prefix = uri_prefix.to_string();
    let (xml, notes) = py
        .allow_threads(|| -> Result<(String, Vec<String>), String> {
            let (doc, notes) = match fmt {
                ImportFormat::Urdf => {
                    hcdformat::from_urdf_str_with_baking(&src, &base, &out, &prefix, &pkgs)
                }
                ImportFormat::Sdf => {
                    hcdformat::from_sdf_str_with_baking(&src, &base, &out, &prefix, &pkgs)
                }
            }
            .map_err(|e| e.to_string())?;
            let xml = doc.to_xml_string().map_err(|e| e.to_string())?;
            Ok((xml, notes))
        })
        .map_err(err)?;
    Ok((xml, notes))
}

// ── asset vendoring (bundling half; the collision-safe .gltf PACKER reaches Python) ────────────────

/// One vendor-manifest row as plain data the shim turns into its `{site, kind, old_uri, new_uri, sha,
/// status}` dict: `(site, kind, old_uri, new_uri, sha, status)`. Returning native tuples (PyO3
/// auto-converts) keeps the `Python` token off the arg list, so the signature stays within the default
/// argument budget; the shim rebuilds the dict shape `hcdf.assets.vendor_assets` promises.
type VendorRow = (String, String, String, String, Option<String>, String);

/// Vendor every visual `<model>` and collision `<mesh>` in an HCDF document into `assets_out_dir`,
/// rewriting each `@uri` to the document-relative `"<uri_prefix>/<name>"` with a matching `@sha`, and
/// return `(rewritten HCDF XML, manifest rows)`. Each row is `(site, kind, old_uri, new_uri, sha,
/// status)` with `status` one of `"vendored"|"remote"|"unresolved"|"needs-conversion"`; the shim maps
/// it back to the `hcdf.assets.vendor_assets` manifest dicts.
///
/// This is the canonical Rust vendor path, so a `.gltf` visual OR collision is PACKED into one
/// self-contained GLB (external `.bin`/texture companions inlined) rather than byte-copied, closing
/// the Python collision-packer gap where the content-hash rename severed a `.gltf`'s relative links.
/// `vendor_remote=true` fetches `http(s)` meshes via the hardened fetcher (the crate is built with the
/// `remote` feature); `package_paths` resolves `package://`/`model://` uris; `cache_dir` is the
/// content-addressed cache for fetched remote bytes.
/// The vendor OUTPUT POLICY, bundled into one tuple arg so the pyfunction stays within the default
/// argument budget once `py` is added: `(uri_prefix, vendor_remote, cache_dir)`. The shim passes it as
/// a plain tuple (PyO3 auto-extracts).
type VendorPolicy = (String, bool, Option<String>);

#[pyfunction]
#[pyo3(signature = (doc_xml, base_dir, assets_out_dir, package_paths, policy))]
fn vendor_assets(
    py: Python<'_>,
    doc_xml: &str,
    base_dir: &str,
    assets_out_dir: &str,
    package_paths: Option<BTreeMap<String, String>>,
    policy: VendorPolicy,
) -> PyResult<(String, Vec<VendorRow>)> {
    let mut doc = Hcdf::from_xml_str(doc_xml).map_err(err)?;
    let pkgs: BTreeMap<String, PathBuf> = package_paths
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, PathBuf::from(v)))
        .collect();
    // Own everything the vendor call needs BEFORE releasing the GIL; the closure must not touch any
    // Python-borrowed data (`doc_xml`/`base_dir`/… are borrows of Python strings).
    let (uri_prefix, vendor_remote, cache_dir) = policy;
    let base = PathBuf::from(base_dir);
    let out = PathBuf::from(assets_out_dir);
    // RELEASE THE GIL for the whole vendor step. It walks the filesystem and, under `vendor_remote`,
    // does BLOCKING network I/O (the hardened fetcher). Holding the GIL across that blocking I/O would
    // starve any Python thread the caller runs alongside it; the remote test harness serves the fetched
    // bytes from an in-process `http.server` THREAD, so a GIL-holding fetch deadlocks against its own
    // mock server until the timeout fires. `allow_threads` lets that server thread run, so remote
    // vendoring completes promptly. The closure owns all its inputs (no Python data crosses the seam).
    let manifest = py
        .allow_threads(|| {
            rs_vendor_assets(
                &mut doc,
                &base,
                &out,
                &pkgs,
                &uri_prefix,
                vendor_remote,
                cache_dir.as_deref().map(Path::new),
            )
        })
        .map_err(err)?;
    let out_xml = doc.to_xml_string().map_err(err)?;
    let rows: Vec<VendorRow> = manifest
        .iter()
        .map(|e| {
            (
                e.site.clone(),
                e.kind.to_string(),
                e.old_uri.clone(),
                e.new_uri.clone(),
                e.sha.clone(),
                e.status.as_str().to_string(),
            )
        })
        .collect();
    Ok((out_xml, rows))
}

// ── bundle open / verify (the .hcdfz read side reaches Python) ──────────────────────────────────────

/// The single top-level `.hcdf` file in `root_dir`: the bundle root (modules live under `modules/`, so
/// there is exactly one top-level `.hcdf`). Reads its RAW bytes so the shim loads a byte-identical DOM.
pub(crate) fn read_root_hcdf(root_dir: &Path) -> Result<String, String> {
    let mut root: Option<PathBuf> = None;
    for entry in std::fs::read_dir(root_dir).map_err(|e| format!("read bundle dir: {e}"))? {
        let entry = entry.map_err(|e| format!("read dir entry: {e}"))?;
        let p = entry.path();
        if p.is_file() && p.extension().is_some_and(|x| x == "hcdf") {
            if root.is_some() {
                return Err(format!(
                    "ambiguous bundle root in {root_dir:?} (multiple top-level .hcdf)"
                ));
            }
            root = Some(p);
        }
    }
    let root =
        root.ok_or_else(|| format!("no root .hcdf found in bundle directory {root_dir:?}"))?;
    std::fs::read_to_string(&root).map_err(|e| format!("read root .hcdf: {e}"))
}

/// Open a bundle (`.hcdfz` zip or loose dir) -> `(root_hcdf_xml, root_dir, is_temp)`. The Rust port of
/// `hcdf_io.bundle.open_bundle`: a zip is extracted to a fresh tempdir (`is_temp=True`; the CALLER owns
/// it and must remove it), a loose dir is used in place (`is_temp=False`). The shim loads the returned
/// RAW root XML into the typed DOM and hands back `(doc, root_dir)`. Releases the GIL for the archive I/O.
#[pyfunction]
fn open_bundle(py: Python<'_>, path: &str) -> PyResult<(String, String, bool)> {
    let path = path.to_string();
    let (root_xml, root_dir, is_temp) = py
        .allow_threads(|| -> Result<(String, String, bool), String> {
            let opened = rs_open_bundle(Path::new(&path))?;
            let is_temp = opened.root_dir.is_temp();
            // Read the root doc while the guard still owns the extraction dir. If this fails we return
            // the error and the guard removes the tempdir as `opened` drops, so a zip open never
            // orphans its extraction dir.
            let root_xml = read_root_hcdf(opened.root_dir.path())?;
            // Success: hand the directory to Python, which owns it and removes it when `is_temp` is
            // true. Defuse the guard (into_path) so it does not delete the dir Python is about to use.
            let root_dir = opened.root_dir.into_path();
            Ok((root_xml, root_dir.to_string_lossy().into_owned(), is_temp))
        })
        .map_err(err)?;
    Ok((root_xml, root_dir, is_temp))
}

/// Verify a bundle's integrity -> `(ok, issues)`. The Rust port of `hcdf_io.bundle.verify`: checks the
/// root `.hcdf` is FIRST (zip) / present (dir), the bundle is flatten-only (a live `--keep-remote`
/// http(s) `<include>` is the one allowed survivor), and every visual `<model>` / collision `<mesh>`
/// blob's `content_sha` equals its `@sha` (+ the filename short-sha). `ok` is true iff `issues` is empty.
/// Read-only; releases the GIL for the archive I/O.
#[pyfunction]
fn verify_bundle(py: Python<'_>, path: &str) -> (bool, Vec<String>) {
    let path = path.to_string();
    let report = py.allow_threads(|| rs_verify_bundle(Path::new(&path)));
    (report.ok, report.issues)
}

// ── compose: fetch-aware flatten + full bundle pack (the remote-include path reaches Python) ─────────

/// Resolve `<include>` composition for a file on disk, FETCHING remote (`http(s)://`) includes ->
/// `(flattened HCDF XML, notes)`. The canonical Rust port of `hcdf_io.include.flatten(fetch=True)`: a
/// remote `<include>` (or a relative include of a remote module) is fetched via the hardened fetcher,
/// composed exactly like a local include, and a fetch failure is FATAL (never a silently-surviving live
/// `<include>`). `cache_dir` is where fetched bytes are content-addressed (`None` => the per-user default).
/// RELEASES THE GIL for the blocking network I/O so a caller's in-process `http.server` test harness keeps
/// serving instead of deadlocking against it.
#[pyfunction]
#[pyo3(signature = (path, cache_dir=None))]
fn flatten_file_fetch(
    py: Python<'_>,
    path: String,
    cache_dir: Option<String>,
) -> PyResult<(String, Vec<String>)> {
    let (doc, notes) = py
        .allow_threads(|| {
            rs_flatten_path_fetch(Path::new(&path), cache_dir.as_deref().map(Path::new))
        })
        .map_err(err)?;
    Ok((doc.to_xml_string().map_err(err)?, notes))
}

/// Resolve `<include>` composition for an IN-MEMORY document, FETCHING remote (`http(s)://`) includes ->
/// (flattened HCDF XML, notes). The string twin of [`flatten_file_fetch`] (see it for the fetch / fatal
/// semantics): takes the document XML STRING and resolves LOCAL includes off `base_dir` (no temp file for
/// the root document), the entry point for `hcdf_io.include.flatten(fetch=True)` when it holds an `Hcdf`.
/// RELEASES THE GIL for the blocking network + filesystem I/O.
#[pyfunction]
#[pyo3(signature = (doc_xml, base_dir, cache_dir=None))]
fn flatten_doc_fetch(
    py: Python<'_>,
    doc_xml: String,
    base_dir: String,
    cache_dir: Option<String>,
) -> PyResult<(String, Vec<String>)> {
    py.allow_threads(|| -> Result<(String, Vec<String>), String> {
        let mut doc = Hcdf::from_xml_str(&doc_xml).map_err(|e| e.to_string())?;
        let notes = rs_flatten_fetch(
            &mut doc,
            Path::new(&base_dir),
            cache_dir.as_deref().map(Path::new),
        )?;
        Ok((doc.to_xml_string().map_err(|e| e.to_string())?, notes))
    })
    .map_err(err)
}

/// The pack OUTPUT POLICY, bundled into one tuple so the pyfunction stays within the default argument
/// budget once `py` is added: `(allow_partial, as_dir, remote, cache_dir)`, where `remote` is one of
/// `"vendor" | "keep" | "error"` (the three-way `--vendor-remote` / `--keep-remote` / neither choice).
type PackPolicy = (bool, bool, String, Option<String>);

/// One packed manifest, as plain data the shim rebuilds `hcdf_io.bundle.pack`'s dict from:
/// `(bundle, format, root, entries, assets_rows, unresolved_rows, kept_remote, notes)`; the two
/// `*_rows` are `VendorRow`s the shim zips into `{site, kind, old_uri, new_uri, sha, status}` dicts.
type PackResult = (
    String,
    String,
    String,
    usize,
    Vec<VendorRow>,
    Vec<VendorRow>,
    Vec<String>,
    Vec<String>,
);

/// Pack a model + its meshes into a self-contained bundle -> the manifest tuple ([`PackResult`]). The
/// canonical Rust port of `hcdf_io.bundle.pack`: FLATTEN (fetching remote includes under `remote="vendor"`)
/// then vendor every visual `<model>` / collision `<mesh>` into `assets/`. `remote="vendor"` fetches +
/// embeds remote assets/includes (self-contained), `"keep"` leaves live http(s) uris, `"error"` (default)
/// makes a remote uri an explicit error. RELEASES THE GIL for the blocking filesystem + network I/O.
#[pyfunction]
#[pyo3(signature = (src, out_path, package_paths, policy))]
fn pack_bundle(
    py: Python<'_>,
    src: String,
    out_path: String,
    package_paths: Option<BTreeMap<String, String>>,
    policy: PackPolicy,
) -> PyResult<PackResult> {
    let (allow_partial, as_dir, remote, cache_dir) = policy;
    let remote = match remote.as_str() {
        "vendor" => RemotePolicy::Vendor,
        "keep" => RemotePolicy::Keep,
        "error" => RemotePolicy::Error,
        other => {
            return Err(PyValueError::new_err(format!(
                "unknown remote policy {other:?} (expected \"vendor\", \"keep\", or \"error\")"
            )))
        }
    };
    let package_paths: BTreeMap<String, PathBuf> = package_paths
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| (k, PathBuf::from(v)))
        .collect();
    let opts = PackOptions {
        package_paths,
        allow_partial,
        as_dir,
        remote,
        cache_dir: cache_dir.map(PathBuf::from),
    };
    let manifest = py
        .allow_threads(|| rs_pack(Path::new(&src), Path::new(&out_path), &opts))
        .map_err(err)?;
    let rows = |es: &[hcdformat::VendorEntry]| -> Vec<VendorRow> {
        es.iter()
            .map(|e| {
                (
                    e.site.clone(),
                    e.kind.to_string(),
                    e.old_uri.clone(),
                    e.new_uri.clone(),
                    e.sha.clone(),
                    e.status.as_str().to_string(),
                )
            })
            .collect()
    };
    Ok((
        manifest.bundle.to_string_lossy().into_owned(),
        manifest.format.to_string(),
        manifest.root,
        manifest.entries,
        rows(&manifest.assets),
        rows(&manifest.unresolved),
        manifest.kept_remote,
        manifest.notes,
    ))
}

// ── in-memory bundle pack / open / verify (the byte-based flows: document-in-memory in, bytes out) ───

/// The report [`pack_bundle_bytes`] returns beside the archive bytes: `(root, entries, embedded,
/// unresolved, notes)`, mirroring the filesystem-free fields of the pack manifest. `root` is the bundle's
/// root `.hcdf` name, `entries` the ZIP entry count, `embedded` the `"<comp>/<visual>"` sites whose mesh
/// bytes were embedded, `unresolved` the sites whose uri had no bytes, `notes` the flatten/include notes.
type MemPackResult = (String, usize, Vec<String>, Vec<String>, Vec<String>);

/// Pack an IN-MEMORY document + its supplied asset bytes into a `.hcdfz`, returning `(bundle_bytes,
/// report)` with NO caller-visible temp file. The byte-based counterpart to [`pack_bundle`]: `doc_xml` is
/// the document to bundle, `assets` maps each mesh/`<include>` uri AS WRITTEN in the document to that
/// resource's raw bytes, and the result is the archive image (a `ZIP_STORED` `.hcdfz`) plus a
/// [`MemPackResult`]. Flattens the document (resolving each `<include>` from `assets`), content-addresses
/// every LOCAL visual `<model>` / collision `<mesh>` whose bytes were supplied into `assets/`, and writes
/// the root `.hcdf` FIRST; a mesh uri with no bytes is left unrewritten (and, unless `allow_partial`, is
/// an error so a bundle cannot silently ship without its meshes). Given the same document and the same
/// bytes under each uri, this produces the byte-identical archive [`pack_bundle`] writes to disk.
#[pyfunction]
#[pyo3(signature = (doc_xml, assets, allow_partial=false))]
fn pack_bundle_bytes(
    py: Python<'_>,
    doc_xml: &str,
    assets: MemBundle,
    allow_partial: bool,
) -> PyResult<(Py<PyBytes>, MemPackResult)> {
    let doc = Hcdf::from_xml_str(doc_xml).map_err(err)?;
    let (bytes, report) = rs_pack_to_bytes(&doc, &assets, allow_partial).map_err(err)?;
    Ok((
        PyBytes::new(py, &bytes).into(),
        (
            report.root,
            report.entries,
            report.embedded,
            report.unresolved,
            report.notes,
        ),
    ))
}

/// The `(relative_path, bytes)` asset pairs an opened bundle carries: every non-root, non-module entry
/// (the `assets/<name>` mesh blobs), keyed by the same document-relative uri the root doc carries.
type BundleAssets = Vec<(String, Py<PyBytes>)>;

/// The KEEP-LIVE embedded `<include>` modules an opened bundle carries, each as
/// `(bundle_module_path, module_xml, module_meshes)`; EMPTY for a flat bundle.
type BundleModules = Vec<(String, String, BundleAssets)>;

/// What [`open_bundle_bytes`] returns: `(root_xml, assets, modules)`, the byte-based mirror of
/// [`open_bundle`]'s extraction with no filesystem access.
type OpenedBundle = (String, BundleAssets, BundleModules);

/// Open a `.hcdfz` from its RAW ARCHIVE BYTES in memory -> `(root_xml, assets, modules)`, with NO
/// filesystem access and no extraction tempdir. The byte-based counterpart to [`open_bundle`] (which
/// extracts a zip to a caller-owned tempdir): `root_xml` is the root document, `assets` is every non-root,
/// non-module entry as `(relative_path, bytes)` pairs (the `assets/<name>` mesh blobs, keyed by the same
/// document-relative uri the root doc carries), and `modules` is each KEEP-LIVE embedded `<include>` module
/// as `(bundle_module_path, module_xml, module_meshes)` (EMPTY for a flat bundle). Releases the GIL for
/// the archive decode.
#[pyfunction]
fn open_bundle_bytes(py: Python<'_>, data: &[u8]) -> PyResult<OpenedBundle> {
    let opened = rs_open_bundle_bytes(data).map_err(err)?;
    let root_xml = opened.doc.to_xml_string().map_err(err)?;
    let assets: BundleAssets = opened
        .assets
        .into_iter()
        .map(|(name, b)| (name, PyBytes::new(py, &b).into()))
        .collect();
    let mut modules: BundleModules = Vec::new();
    for (name, mdoc, meshes) in opened.modules {
        let module_xml = mdoc.to_xml_string().map_err(err)?;
        let module_meshes: BundleAssets = meshes
            .into_iter()
            .map(|(rel, b)| (rel, PyBytes::new(py, &b).into()))
            .collect();
        modules.push((name, module_xml, module_meshes));
    }
    Ok((root_xml, assets, modules))
}

/// Verify a `.hcdfz`'s integrity from its RAW ARCHIVE BYTES in memory -> `(ok, issues)`. The byte-based
/// counterpart to [`verify_bundle`]: runs the SAME checks (root `.hcdf` FIRST, flatten-only unless
/// keep-live, every visual `<model>` / collision `<mesh>` blob's `content_sha` matches its `@sha` + the
/// filename short-sha) against the archive entries, so it agrees issue-for-issue with a path verify of the
/// same bytes. `ok` is true iff `issues` is empty. Read-only, in-memory.
#[pyfunction]
fn verify_bundle_bytes(data: &[u8]) -> (bool, Vec<String>) {
    let report = rs_verify_bytes(data);
    (report.ok, report.issues)
}

// ── remote fetch (the hardened http(s) fetcher reaches Python) ──────────────────────────────────────

/// Fetch an `http(s)://` URL to a content-addressed file in `cache_dir`; return its path. The canonical
/// Rust port of `hcdf_io.remote.fetch_remote`, reproducing every guard (scheme allow-list across
/// redirects, per-operation timeout, streaming size cap, content-addressed atomic cache, `@sha`
/// integrity; see `hcdformat::remote`). `expect_sha` pins the content; `timeout` is in SECONDS
/// (defaults to 30). RELEASES THE GIL for the blocking network I/O so a caller's Python threads (e.g.
/// the remote test harness's in-process `http.server`) keep running instead of deadlocking against it.
#[pyfunction]
#[pyo3(signature = (url, expect_sha=None, cache_dir=None, max_bytes=None, timeout=None))]
fn fetch_remote(
    py: Python<'_>,
    url: String,
    expect_sha: Option<String>,
    cache_dir: Option<String>,
    max_bytes: Option<u64>,
    timeout: Option<f64>,
) -> PyResult<String> {
    let path = py
        .allow_threads(|| {
            rs_fetch_remote(
                &url,
                expect_sha.as_deref(),
                cache_dir.as_deref().map(Path::new),
                max_bytes,
                timeout.map(Duration::from_secs_f64),
            )
        })
        .map_err(err)?;
    Ok(path.to_string_lossy().into_owned())
}

// ── the Rust `hcdf` CLI (the wheel's console script drives it) ───────────────────────────────────────

/// Run the canonical Rust `hcdf` CLI with `argv` (argv[0] is the program name) and return the process
/// exit code. This is what the wheel's `hcdf` console-script trampoline calls, so a `pip install`
/// user gets the SAME Rust CLI as the native `hcdf` binary; there is no Python argument-routing left.
/// clap renders `--help`/`--version`/usage errors itself (without terminating the host process) and the
/// returned int is the code the trampoline hands to `sys.exit`. Releases the GIL for the CLI's I/O.
#[pyfunction]
fn run_cli(py: Python<'_>, argv: Vec<String>) -> i32 {
    py.allow_threads(|| hcdformat::cli::run(argv))
}

// ── type stubs (.pyi tree) ─────────────────────────────────────────────────────────────────────────
//
// The extension IS the `hcdf` package (`hcdf/__init__.abi3.so`), so the shipped stubs are a PEP 561
// tree next to it: `hcdf/__init__.pyi` (this module's top-level entries), `hcdf/dom.pyi` (the generated
// DOM handles), and one file per public submodule. The `gen_pyi` bin writes the whole tree and the
// `pyi_regen_is_a_no_op` test byte-checks it. The builders here are pyo3-free so both run without a
// Python interpreter.

/// The header shared by every generated stub file.
pub(crate) fn pyi_header() -> &'static str {
    "# AUTO-GENERATED by hcdformat-py: run\n\
     #   cargo run --manifest-path rust/hcdformat-py/Cargo.toml --bin gen_pyi\n\
     # Do not edit by hand.\n"
}

/// The stub for the top-level extension module (`hcdf/__init__.pyi`): the package version entries, the
/// console `main`, the internal module-level pyfunctions the submodule surface and CLI build on, and the
/// public submodule attributes. The generated DOM handles live in `hcdf/dom.pyi`.
pub fn init_pyi_text() -> String {
    format!(
        "{header}from typing import Optional\n\n\
         from . import assets as assets\n\
         from . import dom as dom\n\
         from . import io as io\n\
         from . import sdf as sdf\n\
         from . import urdf as urdf\n\n\
         HCDF_VERSION: str\n\
         version: str\n\
         __version__: str\n\n\
         def main(argv: Optional[list[str]] = ...) -> int: ...\n\
         def canonicalize_xml(src: str) -> str: ...\n\
         def parse_ok(src: str) -> bool: ...\n\
         def validate_doc(src: str) -> list[tuple[str, str, str]]: ...\n\
         def validate_xsd(src: str) -> list[tuple[str, str, str]]: ...\n\
         def hcdf_to_json(src: str) -> str: ...\n\
         def hcdf_from_json(src: str) -> str: ...\n\
         def from_urdf(src: str) -> tuple[str, list[str]]: ...\n\
         def from_urdf_with_assets(src: str) -> tuple[str, list[str], list[tuple[str, str, Optional[str], Optional[str], Optional[str]]]]: ...\n\
         def to_urdf(src: str) -> tuple[str, list[tuple[str, str]]]: ...\n\
         def from_sdf(src: str) -> tuple[str, list[str]]: ...\n\
         def from_sdf_with_assets(src: str) -> tuple[str, list[str], list[tuple[str, str, Optional[str], Optional[str], Optional[str]]]]: ...\n\
         def to_sdf(src: str) -> tuple[str, list[tuple[str, str]]]: ...\n\
         def expand_xacro_path(path: str, mappings: Optional[dict[str, str]] = ..., packages: Optional[dict[str, str]] = ...) -> str: ...\n\
         def xacro_available() -> bool: ...\n\
         def check_profile(src: str) -> dict: ...\n\
         def profile_markdown(src: str) -> str: ...\n\
         def profile_json(src: str) -> str: ...\n\
         def loss_text(src: str) -> str: ...\n\
         def loss_json(src: str) -> str: ...\n\
         def loss_markdown(src: str, title: Optional[str] = ...) -> str: ...\n\
         def flatten_file(path: str) -> tuple[str, list[str]]: ...\n\
         def flatten_doc(doc_xml: str, base_dir: str) -> tuple[str, list[str]]: ...\n\
         def flatten_file_fetch(path: str, cache_dir: Optional[str] = ...) -> tuple[str, list[str]]: ...\n\
         def flatten_doc_fetch(doc_xml: str, base_dir: str, cache_dir: Optional[str] = ...) -> tuple[str, list[str]]: ...\n\
         def pack_bundle(src: str, out_path: str, package_paths: Optional[dict[str, str]], policy: tuple[bool, bool, str, Optional[str]]) -> tuple[str, str, str, int, list[tuple[str, str, str, str, Optional[str], str]], list[tuple[str, str, str, str, Optional[str], str]], list[str], list[str]]: ...\n\
         def stamp_shas(src: str, base_dir: str) -> tuple[str, list[str]]: ...\n\
         def content_sha(data: bytes) -> str: ...\n\
         def resolve_uri(uri: str, base_dir: str = ..., package_paths: Optional[dict[str, str]] = ...) -> Optional[str]: ...\n\
         def validate_structural(src: str) -> list[str]: ...\n\
         def bake_to_glb(src_path: str, scale: Optional[str] = ..., color: Optional[str] = ...) -> bytes: ...\n\
         def bake_to_lean(src_path: str, scale: Optional[str] = ...) -> bytes: ...\n\
         def from_urdf_with_baking(src: str, base_dir: str, assets_out_dir: str, uri_prefix: str, package_paths: Optional[dict[str, str]] = ...) -> tuple[str, list[str]]: ...\n\
         def from_sdf_with_baking(src: str, base_dir: str, assets_out_dir: str, uri_prefix: str, package_paths: Optional[dict[str, str]] = ...) -> tuple[str, list[str]]: ...\n\
         def vendor_assets(doc_xml: str, base_dir: str, assets_out_dir: str, package_paths: Optional[dict[str, str]], policy: tuple[str, bool, Optional[str]]) -> tuple[str, list[tuple[str, str, str, str, Optional[str], str]]]: ...\n\
         def open_bundle(path: str) -> tuple[str, str, bool]: ...\n\
         def verify_bundle(path: str) -> tuple[bool, list[str]]: ...\n\
         def pack_bundle_bytes(doc_xml: str, assets: dict[str, bytes], allow_partial: bool = ...) -> tuple[bytes, tuple[str, int, list[str], list[str], list[str]]]: ...\n\
         def open_bundle_bytes(data: bytes) -> tuple[str, list[tuple[str, bytes]], list[tuple[str, str, list[tuple[str, bytes]]]]]: ...\n\
         def verify_bundle_bytes(data: bytes) -> tuple[bool, list[str]]: ...\n\
         def fetch_remote(url: str, expect_sha: Optional[str] = ..., cache_dir: Optional[str] = ..., max_bytes: Optional[int] = ..., timeout: Optional[float] = ...) -> str: ...\n\
         def run_cli(argv: list[str]) -> int: ...\n\
         def dom_pyi() -> str: ...\n\
         def binding_pyi() -> str: ...\n",
        header = pyi_header(),
    )
}

/// The stub text for `hcdf/__init__.pyi` (this module's top-level entries), returned so a Python caller
/// can introspect the surface it presents; the whole shipped tree is written by the `gen_pyi` bin.
#[pyfunction]
fn binding_pyi() -> String {
    init_pyi_text()
}

/// The (filename, content) pairs the `gen_pyi` bin writes under `hcdf/` and the drift test byte-checks:
/// the top-level stub, the generated DOM stub, and the five public submodule stubs.
pub fn pyi_files() -> Vec<(&'static str, String)> {
    vec![
        ("__init__.pyi", init_pyi_text()),
        ("dom.pyi", dom::dom_pyi_text()),
        ("urdf.pyi", surface::urdf_pyi_text()),
        ("sdf.pyi", surface::sdf_pyi_text()),
        ("io.pyi", surface::io_pyi_text()),
        ("assets.pyi", surface::assets_pyi_text()),
    ]
}

// ── module ───────────────────────────────────────────────────────────────────────────────────────

/// The native extension module Python imports as `hcdf`. It ships as `hcdf/__init__.abi3.so`, so the
/// extension IS the package: `import hcdf` loads it, and the public `hcdf.*` submodules registered below
/// are inserted into `sys.modules` at init so they resolve cold.
#[pymodule]
fn hcdf(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add(
        "__doc__",
        "Canonical Rust hcdformat core exposed to Python.",
    )?;
    let v = hcdformat::version::HcdfVersion::V1_0;
    m.add("HCDF_VERSION", format!("{}.{}", v.major, v.minor))?;
    m.add_function(wrap_pyfunction!(canonicalize_xml, m)?)?;
    m.add_function(wrap_pyfunction!(parse_ok, m)?)?;
    m.add_function(wrap_pyfunction!(validate_doc, m)?)?;
    m.add_function(wrap_pyfunction!(validate_xsd, m)?)?;
    m.add_function(wrap_pyfunction!(hcdf_to_json, m)?)?;
    m.add_function(wrap_pyfunction!(hcdf_from_json, m)?)?;
    m.add_function(wrap_pyfunction!(from_urdf, m)?)?;
    m.add_function(wrap_pyfunction!(from_urdf_with_assets, m)?)?;
    m.add_function(wrap_pyfunction!(to_urdf, m)?)?;
    m.add_function(wrap_pyfunction!(from_sdf, m)?)?;
    m.add_function(wrap_pyfunction!(from_sdf_with_assets, m)?)?;
    m.add_function(wrap_pyfunction!(to_sdf, m)?)?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        m.add_function(wrap_pyfunction!(expand_xacro_path, m)?)?;
        m.add_function(wrap_pyfunction!(xacro_available, m)?)?;
    }
    m.add_function(wrap_pyfunction!(check_profile, m)?)?;
    m.add_function(wrap_pyfunction!(profile_markdown, m)?)?;
    m.add_function(wrap_pyfunction!(profile_json, m)?)?;
    m.add_function(wrap_pyfunction!(loss_text, m)?)?;
    m.add_function(wrap_pyfunction!(loss_json, m)?)?;
    m.add_function(wrap_pyfunction!(loss_markdown, m)?)?;
    m.add_function(wrap_pyfunction!(flatten_file, m)?)?;
    m.add_function(wrap_pyfunction!(flatten_doc, m)?)?;
    m.add_function(wrap_pyfunction!(stamp_shas, m)?)?;
    m.add_function(wrap_pyfunction!(content_sha, m)?)?;
    m.add_function(wrap_pyfunction!(resolve_uri, m)?)?;
    m.add_function(wrap_pyfunction!(validate_structural, m)?)?;
    m.add_function(wrap_pyfunction!(bake_to_glb, m)?)?;
    m.add_function(wrap_pyfunction!(bake_to_lean, m)?)?;
    m.add_function(wrap_pyfunction!(from_urdf_with_baking, m)?)?;
    m.add_function(wrap_pyfunction!(from_sdf_with_baking, m)?)?;
    m.add_function(wrap_pyfunction!(vendor_assets, m)?)?;
    m.add_function(wrap_pyfunction!(flatten_file_fetch, m)?)?;
    m.add_function(wrap_pyfunction!(flatten_doc_fetch, m)?)?;
    m.add_function(wrap_pyfunction!(pack_bundle, m)?)?;
    m.add_function(wrap_pyfunction!(open_bundle, m)?)?;
    m.add_function(wrap_pyfunction!(verify_bundle, m)?)?;
    m.add_function(wrap_pyfunction!(pack_bundle_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(open_bundle_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(verify_bundle_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(fetch_remote, m)?)?;
    m.add_function(wrap_pyfunction!(run_cli, m)?)?;
    // Generated DOM handles (+ the data-enum companion + `dom_pyi`).
    dom::register(m)?;
    m.add_function(wrap_pyfunction!(binding_pyi, m)?)?;
    // The public `hcdf.*` submodule surface (dom / urdf / sdf / io / assets), the report types, and
    // the console `main()`, registered on this extension and inserted into `sys.modules`.
    surface::register(m.py(), m)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Drift gate: regenerating the `.pyi` tree from the pyo3-free builders must byte-match the committed
    /// `hcdf/*.pyi` files, so `cargo test` alone catches a stub that diverged from the surface (run
    /// `cargo run --bin gen_pyi` to refresh). Matches the `*_regen_is_a_no_op` gates on the other
    /// generated artifacts. The text is pure Rust, so this needs no Python interpreter.
    #[test]
    fn pyi_regen_is_a_no_op() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("hcdf");
        for (name, text) in crate::pyi_files() {
            let path = dir.join(name);
            let committed = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read committed {}: {e}", path.display()));
            assert_eq!(
                text, committed,
                "{name} is out of sync with the surface; run `cargo run --bin gen_pyi`"
            );
        }
    }
}
