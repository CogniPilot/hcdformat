//! The public `hcdf.*` Python surface: the five submodules (`dom`, `urdf`, `sdf`, `io`, `assets`),
//! the report `#[pyclass]` types, and the console `main()`, all over the canonical Rust core.
//!
//! The flat `#[pyfunction]`s in `lib.rs` are the internal extension entries (CLI-only helpers and the
//! primitives these functions build on); this module is the shape a Python user sees. Every function
//! carries the shim docstring content and either forwards to a core entry point directly or navigates
//! the write-through `Hcdf` handle the DOM exposes. The submodules are registered on the extension at
//! module init and inserted into `sys.modules` so `import <ext>.urdf` and `from <ext>.urdf import ...`
//! both resolve cold.

use pyo3::exceptions::{PyOSError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyDict, PyList, PyModule, PySet};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use hcdformat::compose::flatten_path_fetch as core_flatten_path_fetch;
use hcdformat::flatten_fetch as core_flatten_fetch;
use hcdformat::profile::{ProfileReport as CoreReport, Tier};
use hcdformat::to_urdf::LossManifest as CoreLoss;
use hcdformat::validate::Level;
use hcdformat::{
    bake, bake_document, check_profile as core_check_profile, content_sha as core_content_sha,
    drop_baked_notes as core_drop_baked_notes, flatten as core_flatten_doc, flatten_path,
    from_sdf::from_sdf_str, from_sdf_str_with_baking, from_urdf::from_urdf_str,
    from_urdf_str_with_baking, open_bundle as core_open_bundle, pack as core_pack,
    resolve_uri as core_resolve_uri, stamp_include_shas as core_stamp_shas,
    to_sdf::to_sdf as core_to_sdf, to_urdf::to_urdf as core_to_urdf,
    vendor_assets as core_vendor_assets, verify as core_verify_bundle, BakeEnv, Hcdf, PackOptions,
    RemotePolicy, StreamProfileDocument, VendorEntry, VisualAssetHint,
};

use crate::dom::{self, PyHcdf, PyStreamProfileDocument};

/// The package version the wheel ships as (distinct from the spec `HCDF_VERSION`).
const WHEEL_VERSION: &str = "1.0.0";

/// The one loss category that is pure annotation (doc/comp/joint/color descriptions, document metadata,
/// collision verbosity): recorded for honesty but never model content, so it never drops a document
/// below in-profile. Mirrors the shim `BENIGN_LOSS_CATEGORIES`.
const BENIGN: &[&str] = &["annotation"];

/// Map any crate error into a Python `ValueError` with the crate's message (the shim's parse/convert
/// errors surface as `ValueError`).
fn err<E: std::fmt::Display>(e: E) -> PyErr {
    PyValueError::new_err(e.to_string())
}

/// Map a filesystem error into a Python `OSError` (the shim's file reads/writes raise `OSError`).
fn oserr(e: std::io::Error) -> PyErr {
    PyOSError::new_err(e.to_string())
}

/// One per-visual asset-bake hint as plain data (`comp`, `visual`, `scale`, `color`, `texture`), the
/// tuple the importer side-channel hands `apply_import_baker`.
type HintTuple = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

// ── shared helpers ─────────────────────────────────────────────────────────────────────────────────

/// `str(obj)`: the string form of a path, str, or any object with `__str__` (the shim's `str(src)`).
fn pyany_to_str(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    obj.str()?.extract()
}

/// Lexical `os.path.abspath`: join a relative path onto the cwd and collapse `.`/`..` without touching
/// the filesystem (so a not-yet-created path still normalizes, matching Python's `abspath`).
fn abspath(p: &str) -> String {
    let pb = Path::new(p);
    let joined = if pb.is_absolute() {
        pb.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(pb))
            .unwrap_or_else(|_| pb.to_path_buf())
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out.to_string_lossy().into_owned()
}

/// The `{pkg: /path}` map as the core wants it (`PathBuf` values).
fn package_map(pkgs: &BTreeMap<String, String>) -> BTreeMap<String, PathBuf> {
    pkgs.iter()
        .map(|(k, v)| (k.clone(), PathBuf::from(v)))
        .collect()
}

/// Resolve a mesh `uri` to an existing filesystem path, or `None` (the shim's `resolve_uri` body).
fn resolve_uri_impl(uri: &str, base_dir: &str, pkgs: &BTreeMap<String, String>) -> Option<String> {
    let map = package_map(pkgs);
    core_resolve_uri(uri, Path::new(base_dir), &map).map(|p| p.to_string_lossy().into_owned())
}

/// Parse a `"sx sy sz"` (or single-value) scale to a vector, or `None` when absent or the identity
/// (mirrors the shim `_scale_vec`; a non-numeric token raises `ValueError`).
fn scale_vec(scale: Option<&str>) -> PyResult<Option<Vec<f64>>> {
    let Some(s) = scale else { return Ok(None) };
    if s.is_empty() {
        return Ok(None);
    }
    let mut parts: Vec<f64> = Vec::new();
    for tok in s.split_whitespace() {
        parts.push(tok.parse::<f64>().map_err(|_| {
            PyValueError::new_err(format!("could not parse a float from scale token {tok:?}"))
        })?);
    }
    if parts.is_empty() {
        return Ok(None);
    }
    if parts.len() == 1 {
        let v = parts[0];
        parts = vec![v, v, v];
    }
    if parts.iter().all(|x| (x - 1.0).abs() < 1e-12) {
        Ok(None)
    } else {
        Ok(Some(parts))
    }
}

/// The basename without its extension, sanitized to `[0-9A-Za-z._-]` (mirrors the shim `_safe_stem`;
/// a `""` result becomes `"mesh"`). Used only to make a baked filename readable.
fn safe_stem(path: &str) -> String {
    let base = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = match base.rfind('.') {
        Some(i) if i > 0 => base[..i].to_string(),
        _ => base.clone(),
    };
    let sanitized: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "mesh".to_string()
    } else {
        sanitized
    }
}

/// The source extension with the dot stripped and case preserved (mirrors Python
/// `os.path.splitext(p)[1].lstrip(".")`), or `""` when there is no extension.
fn ext_lstrip_dot(path: &str) -> String {
    let base = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match base.rfind('.') {
        Some(i) if i > 0 => base[i + 1..].to_string(),
        _ => String::new(),
    }
}

/// Export a deterministic embedded GLB for a source mesh; an already-GLB source is a verbatim
/// passthrough (the core converts non-GLB only), so its own appearance is never re-flattened.
fn to_glb_impl(src_path: &str, scale: Option<&str>, color: Option<&str>) -> PyResult<Vec<u8>> {
    let lower = src_path.to_ascii_lowercase();
    if lower.ends_with(".glb") || lower.ends_with(".gltf") {
        return std::fs::read(src_path).map_err(oserr);
    }
    bake::to_glb(src_path, scale, color).map_err(err)
}

/// Bake one mesh to a canonical asset in `out_dir`, returning `(filename, sha)` (the shim `bake` body).
fn bake_impl(
    src_path: &str,
    scale: Option<&str>,
    kind: &str,
    out_dir: &str,
    color: Option<&str>,
) -> PyResult<(String, String)> {
    std::fs::create_dir_all(out_dir).map_err(oserr)?;
    let vec = scale_vec(scale)?;
    let lower = src_path.to_ascii_lowercase();
    let is_glb = lower.ends_with(".glb") || lower.ends_with(".gltf");
    let (data, ext) = if kind == "visual" {
        if is_glb && vec.is_none() && color.is_none() {
            (
                std::fs::read(src_path).map_err(oserr)?,
                ext_lstrip_dot(src_path),
            )
        } else {
            (
                bake::to_glb(src_path, scale, color).map_err(err)?,
                "glb".to_string(),
            )
        }
    } else {
        let e = ext_lstrip_dot(src_path);
        let ext = if e.is_empty() { "stl".to_string() } else { e };
        if vec.is_none() {
            (std::fs::read(src_path).map_err(oserr)?, ext)
        } else {
            (bake::to_lean(src_path, scale).map_err(err)?, ext)
        }
    };
    let sha = core_content_sha(&data);
    let hexpart = sha.split_once(':').map_or(sha.as_str(), |(_, h)| h);
    let short = &hexpart[..12.min(hexpart.len())];
    let stem = safe_stem(src_path);
    let name = format!("{stem}_{short}.{ext}");
    std::fs::write(Path::new(out_dir).join(&name), &data).map_err(oserr)?;
    Ok((name, sha))
}

/// Build a `{site, kind, old_uri, new_uri, sha, status}` list from vendor-manifest rows.
fn rows_to_dicts<'py>(py: Python<'py>, entries: &[VendorEntry]) -> PyResult<Bound<'py, PyList>> {
    let out = PyList::empty(py);
    for e in entries {
        let d = PyDict::new(py);
        d.set_item("site", &e.site)?;
        d.set_item("kind", e.kind)?;
        d.set_item("old_uri", &e.old_uri)?;
        d.set_item("new_uri", &e.new_uri)?;
        d.set_item("sha", e.sha.clone())?;
        d.set_item("status", e.status.as_str())?;
        out.append(d)?;
    }
    Ok(out)
}

/// Fetch an optional keyword, treating a `None` value as absent.
fn kw_get<'py>(
    kwargs: Option<&Bound<'py, PyDict>>,
    key: &str,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    match kwargs {
        Some(d) => match d.get_item(key)? {
            Some(v) if !v.is_none() => Ok(Some(v)),
            _ => Ok(None),
        },
        None => Ok(None),
    }
}

fn kw_bool(kwargs: Option<&Bound<'_, PyDict>>, key: &str, default: bool) -> PyResult<bool> {
    match kw_get(kwargs, key)? {
        Some(v) => v.extract(),
        None => Ok(default),
    }
}

fn kw_str(kwargs: Option<&Bound<'_, PyDict>>, key: &str) -> PyResult<Option<String>> {
    match kw_get(kwargs, key)? {
        Some(v) => Ok(Some(v.extract()?)),
        None => Ok(None),
    }
}

fn kw_map(
    kwargs: Option<&Bound<'_, PyDict>>,
    key: &str,
) -> PyResult<Option<BTreeMap<String, String>>> {
    match kw_get(kwargs, key)? {
        Some(v) => Ok(Some(v.extract()?)),
        None => Ok(None),
    }
}

/// Reject any keyword the target function does not accept, matching a fixed-signature call.
fn reject_unknown_kwargs(kwargs: Option<&Bound<'_, PyDict>>, allowed: &[&str]) -> PyResult<()> {
    if let Some(d) = kwargs {
        for (k, _) in d.iter() {
            let key: String = k.extract()?;
            if !allowed.contains(&key.as_str()) {
                return Err(PyTypeError::new_err(format!(
                    "unexpected keyword argument {key:?}"
                )));
            }
        }
    }
    Ok(())
}

// ── report types (hcdf.urdf) ─────────────────────────────────────────────────────────────────────────

/// Structured record of what HCDF content could not be represented in URDF; the render methods
/// (`text`/`categories`/`to_dict`/`to_json`/`markdown`) are the canonical core renders, so a Python
/// caller produces the same bytes the `hcdf` CLI prints.
#[pyclass(name = "LossManifest")]
pub struct PyLossManifest {
    inner: CoreLoss,
}

#[pymethods]
impl PyLossManifest {
    #[new]
    fn new() -> Self {
        PyLossManifest {
            inner: CoreLoss::default(),
        }
    }

    /// Record a `(category, detail)` loss.
    fn add(&mut self, category: &str, detail: String) {
        self.inner.add(category, detail);
    }

    fn __bool__(&self) -> bool {
        !self.inner.is_empty()
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    /// Flat text form, one `[category] detail` per line.
    fn text(&self) -> String {
        self.inner.text()
    }

    /// An order-preserving `{category: [detail, ...]}` grouping of the items.
    fn categories<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for (c, ds) in self.inner.categories() {
            d.set_item(c, ds)?;
        }
        Ok(d)
    }

    /// Structured form: total count, per-category grouping, and the flat item list.
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        py.import("json")?
            .call_method1("loads", (self.inner.to_json(),))
    }

    /// The structured form as pretty JSON (`indent` defaults to 2, the CLI's rendering).
    #[pyo3(signature = (indent=2))]
    fn to_json(&self, py: Python<'_>, indent: i64) -> PyResult<String> {
        json_with_indent(py, self.inner.to_json(), indent)
    }

    /// A human-readable report grouped by category (stable category ordering); `title` defaults to the
    /// canonical `HCDF -> URDF loss manifest` heading.
    #[pyo3(signature = (title=None))]
    fn markdown(&self, title: Option<&str>) -> String {
        self.inner
            .markdown(title.unwrap_or(hcdformat::to_urdf::DEFAULT_LOSS_TITLE))
    }

    fn __repr__(&self) -> String {
        format!("<LossManifest {} item(s)>", self.inner.len())
    }
}

/// One reason a document is at a given tier (the driver of the classification).
#[pyclass(frozen, name = "Finding")]
pub struct PyFinding {
    #[pyo3(get)]
    tier: String,
    #[pyo3(get)]
    code: String,
    #[pyo3(get)]
    detail: String,
}

#[pymethods]
impl PyFinding {
    #[new]
    fn new(tier: String, code: String, detail: String) -> Self {
        PyFinding { tier, code, detail }
    }

    fn __repr__(&self) -> String {
        format!(
            "Finding(tier={:?}, code={:?}, detail={:?})",
            self.tier, self.code, self.detail
        )
    }
}

/// A single validator finding (level / code / message).
#[pyclass(frozen, name = "Issue")]
pub struct PyIssue {
    #[pyo3(get)]
    level: String,
    #[pyo3(get)]
    code: String,
    #[pyo3(get)]
    message: String,
}

#[pymethods]
impl PyIssue {
    #[new]
    fn new(level: String, code: String, message: String) -> Self {
        PyIssue {
            level,
            code,
            message,
        }
    }

    fn __str__(&self) -> String {
        format!("[{}] {}: {}", self.level, self.code, self.message)
    }

    fn __repr__(&self) -> String {
        format!(
            "Issue(level={:?}, code={:?}, message={:?})",
            self.level, self.code, self.message
        )
    }
}

fn level_str(level: Level) -> &'static str {
    match level {
        Level::Error => "error",
        Level::Warning => "warning",
    }
}

/// The HCDF-URDF Profile classification: tier plus findings, the field-level loss manifest, and the
/// validator issues; its `to_dict`/`to_json`/`markdown` are the canonical core renders.
#[pyclass(name = "ProfileReport")]
pub struct PyProfileReport {
    inner: CoreReport,
}

#[pymethods]
impl PyProfileReport {
    /// One of the three tier strings.
    #[getter]
    fn classification(&self) -> String {
        self.inner.classification.label().to_string()
    }

    /// True if URDF-consumable (identity or with-transform); False if out-of-profile.
    #[getter]
    fn in_profile(&self) -> bool {
        self.inner.in_profile()
    }

    #[getter]
    fn is_identity(&self) -> bool {
        self.inner.classification == Tier::Identity
    }

    #[getter]
    fn findings(&self) -> Vec<PyFinding> {
        self.inner
            .findings
            .iter()
            .map(|f| PyFinding {
                tier: f.tier.label().to_string(),
                code: f.code.clone(),
                detail: f.detail.clone(),
            })
            .collect()
    }

    #[getter]
    fn loss(&self) -> PyLossManifest {
        PyLossManifest {
            inner: self.inner.loss.clone(),
        }
    }

    #[getter]
    fn issues(&self) -> Vec<PyIssue> {
        self.inner
            .issues
            .iter()
            .map(|i| PyIssue {
                level: level_str(i.level).to_string(),
                code: i.code.clone(),
                message: i.message.clone(),
            })
            .collect()
    }

    /// The findings at a given tier (matched on the tier label string).
    fn by_tier(&self, tier: &str) -> Vec<PyFinding> {
        self.inner
            .findings
            .iter()
            .filter(|f| f.tier.label() == tier)
            .map(|f| PyFinding {
                tier: f.tier.label().to_string(),
                code: f.code.clone(),
                detail: f.detail.clone(),
            })
            .collect()
    }

    /// Loss items that represent dropped MODEL content (excludes pure annotation/metadata).
    #[getter]
    fn nonbenign_losses(&self) -> Vec<(String, String)> {
        self.inner
            .loss
            .items
            .iter()
            .filter(|(c, _)| !BENIGN.contains(&c.as_str()))
            .cloned()
            .collect()
    }

    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        py.import("json")?
            .call_method1("loads", (self.inner.to_json(),))
    }

    #[pyo3(signature = (indent=2))]
    fn to_json(&self, py: Python<'_>, indent: i64) -> PyResult<String> {
        json_with_indent(py, self.inner.to_json(), indent)
    }

    fn markdown(&self) -> String {
        self.inner.markdown()
    }

    fn __repr__(&self) -> String {
        format!("<ProfileReport {}>", self.inner.classification.label())
    }
}

/// Return `indent==2` JSON verbatim from the core render; for any other indent, re-dump the parsed
/// value at that indent (the shim's `json.dumps(self.to_dict(), indent=indent)` contract).
fn json_with_indent(py: Python<'_>, core_json: String, indent: i64) -> PyResult<String> {
    if indent == 2 {
        return Ok(core_json);
    }
    let json = py.import("json")?;
    let value = json.call_method1("loads", (core_json,))?;
    let kwargs = PyDict::new(py);
    kwargs.set_item("indent", indent)?;
    json.call_method("dumps", (value,), Some(&kwargs))?
        .extract()
}

// ── the asset baker (hcdf.assets.Baker) ──────────────────────────────────────────────────────────────

/// Resolve and bake meshes during conversion, content-addressing into `out_dir`. Returns `(uri, sha)`
/// for a baked asset, or `None` if the mesh cannot be resolved.
#[pyclass(name = "Baker")]
pub struct PyBaker {
    out_dir: String,
    base_dir: String,
    package_paths: BTreeMap<String, String>,
    uri_prefix: String,
}

#[pymethods]
impl PyBaker {
    #[new]
    #[pyo3(signature = (out_dir, base_dir=None, package_paths=None, uri_prefix=None))]
    fn new(
        out_dir: String,
        base_dir: Option<String>,
        package_paths: Option<BTreeMap<String, String>>,
        uri_prefix: Option<String>,
    ) -> Self {
        PyBaker {
            out_dir,
            base_dir: base_dir.unwrap_or_else(|| ".".to_string()),
            package_paths: package_paths.unwrap_or_default(),
            uri_prefix: uri_prefix.unwrap_or_default(),
        }
    }

    #[pyo3(signature = (uri, scale, kind, color=None))]
    fn __call__(
        &self,
        uri: &str,
        scale: Option<&str>,
        kind: &str,
        color: Option<&str>,
    ) -> PyResult<Option<(String, String)>> {
        let Some(path) = resolve_uri_impl(uri, &self.base_dir, &self.package_paths) else {
            return Ok(None);
        };
        let (mut name, sha) = bake_impl(&path, scale, kind, &self.out_dir, color)?;
        if !self.uri_prefix.is_empty() && self.uri_prefix != "." {
            name = format!("{}/{}", self.uri_prefix.trim_end_matches('/'), name);
        }
        Ok(Some((name, sha)))
    }
}

// ── hcdf.dom ─────────────────────────────────────────────────────────────────────────────────────────

/// Parse an HCDF file into the typed DOM.
#[pyfunction]
#[pyo3(name = "load")]
fn dom_load(path: &str) -> PyResult<PyHcdf> {
    let s = std::fs::read_to_string(path).map_err(err)?;
    let doc = Hcdf::from_xml_str(&s).map_err(err)?;
    Ok(dom::new_handle(doc))
}

/// Parse HCDF from a string or bytes into the typed DOM.
#[pyfunction]
#[pyo3(name = "loads")]
fn dom_loads(text: &Bound<'_, PyAny>) -> PyResult<PyHcdf> {
    let s: String = match text.extract::<String>() {
        Ok(s) => s,
        Err(_) => {
            let b: Vec<u8> = text.extract()?;
            String::from_utf8(b).map_err(err)?
        }
    };
    let doc = Hcdf::from_xml_str(&s).map_err(err)?;
    Ok(dom::new_handle(doc))
}

/// Serialize the DOM to a canonical, schema-ordered XML string.
#[pyfunction]
#[pyo3(name = "dumps")]
fn dom_dumps(doc: PyRef<'_, PyHcdf>) -> PyResult<String> {
    dom::handle_xml(&doc)
}

/// Serialize the DOM to an HCDF file (canonical form).
#[pyfunction]
#[pyo3(name = "dump")]
fn dom_dump(doc: PyRef<'_, PyHcdf>, path: &str) -> PyResult<()> {
    let s = dom::handle_xml(&doc)?;
    std::fs::write(path, s).map_err(err)
}

#[pyfunction]
fn load_stream_profile(path: &str) -> PyResult<PyStreamProfileDocument> {
    let source = std::fs::read_to_string(path).map_err(err)?;
    let document = StreamProfileDocument::from_xml_str(&source).map_err(err)?;
    Ok(dom::new_stream_profile_handle(document))
}

#[pyfunction]
fn loads_stream_profile(text: &Bound<'_, PyAny>) -> PyResult<PyStreamProfileDocument> {
    let source: String = match text.extract::<String>() {
        Ok(source) => source,
        Err(_) => {
            let bytes: Vec<u8> = text.extract()?;
            String::from_utf8(bytes).map_err(err)?
        }
    };
    let document = StreamProfileDocument::from_xml_str(&source).map_err(err)?;
    Ok(dom::new_stream_profile_handle(document))
}

#[pyfunction]
fn dumps_stream_profile(document: PyRef<'_, PyStreamProfileDocument>) -> PyResult<String> {
    dom::stream_profile_handle_xml(&document)
}

#[pyfunction]
fn dump_stream_profile(document: PyRef<'_, PyStreamProfileDocument>, path: &str) -> PyResult<()> {
    let source = dom::stream_profile_handle_xml(&document)?;
    std::fs::write(path, source).map_err(err)
}

// ── hcdf.urdf ────────────────────────────────────────────────────────────────────────────────────────

/// Read a URDF source (path, string, or bytes) to text, expanding a `.xacro` path first via the core's
/// bundled xacro engine. `mappings` seeds the xacro `$(arg name)` table (each entry overriding the
/// matching `<xacro:arg>` default, the `name := value` overrides canonical `xacro` takes on the command
/// line); it applies ONLY to a `.xacro` path, since a literal URDF string or a plain `.urdf` file has no
/// `$(arg)` to substitute. `packages` resolves `$(find name)`.
fn read_urdf_src(
    src: &Bound<'_, PyAny>,
    mappings: Option<BTreeMap<String, String>>,
    packages: Option<BTreeMap<String, String>>,
) -> PyResult<String> {
    if let Ok(pb) = src.downcast::<PyBytes>() {
        return String::from_utf8(pb.as_bytes().to_vec()).map_err(err);
    }
    if let Ok(ba) = src.downcast::<PyByteArray>() {
        return String::from_utf8(ba.to_vec()).map_err(err);
    }
    if let Ok(s) = src.extract::<String>() {
        if s.trim_start().starts_with('<') {
            return Ok(s);
        }
        return read_or_xacro(&s, mappings, packages);
    }
    let s = pyany_to_str(src)?;
    read_or_xacro(&s, mappings, packages)
}

/// A URDF path: a `.xacro` expands via the core engine (seeding `$(arg)` from `mappings`), any other path
/// is read verbatim (a plain URDF file has no substitution, so `mappings` does not apply).
fn read_or_xacro(
    path: &str,
    mappings: Option<BTreeMap<String, String>>,
    packages: Option<BTreeMap<String, String>>,
) -> PyResult<String> {
    if path.ends_with(".xacro") {
        let pkg: HashMap<String, String> = packages.unwrap_or_default().into_iter().collect();
        let args: HashMap<String, String> = mappings.unwrap_or_default().into_iter().collect();
        hcdformat::expand_xacro_with_args(Path::new(path), &pkg, &args).map_err(err)
    } else {
        std::fs::read_to_string(path).map_err(err)
    }
}

/// Import URDF (path, string, bytes) into an `hcdf.dom.Hcdf` model. Returns `(doc, notes)`. `mappings`
/// supplies `$(arg name)` values when `src` is a `.xacro` path (each overriding that arg's `<xacro:arg>`
/// default); it has no effect on a literal URDF string or a plain `.urdf` file. With a `baker` (an
/// `assets.Baker`), each visual/collision mesh is resolved and baked to a canonical asset there
/// (scale/colour folded in), so HCDF never carries a source scale.
#[pyfunction]
#[pyo3(signature = (src, mappings=None, baker=None, packages=None), name = "from_urdf")]
fn urdf_from_urdf(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    mappings: Option<BTreeMap<String, String>>,
    baker: Option<&Bound<'_, PyBaker>>,
    packages: Option<BTreeMap<String, String>>,
) -> PyResult<(PyHcdf, Vec<String>)> {
    let urdf_text = read_urdf_src(src, mappings, packages)?;
    match baker {
        None => {
            let (doc, notes) = from_urdf_str(&urdf_text).map_err(err)?;
            Ok((dom::new_handle(doc), notes))
        }
        Some(b) => {
            let b = b.borrow();
            let (doc, notes) = import_with_baking(py, ImportKind::Urdf, &urdf_text, &b)?;
            Ok((dom::new_handle(doc), notes))
        }
    }
}

/// Export an `hcdf.dom.Hcdf` model to URDF. Returns `(urdf_xml, LossManifest)`.
#[pyfunction]
#[pyo3(name = "to_urdf")]
fn urdf_to_urdf(doc: PyRef<'_, PyHcdf>) -> PyResult<(String, PyLossManifest)> {
    let g = doc.doc.borrow();
    let (xml, loss) = core_to_urdf(&g).map_err(err)?;
    Ok((xml, PyLossManifest { inner: loss }))
}

/// Classify the document against the HCDF-URDF Profile 1.0.
#[pyfunction]
#[pyo3(name = "check_profile")]
fn urdf_check_profile(doc: PyRef<'_, PyHcdf>) -> PyResult<PyProfileReport> {
    let g = doc.doc.borrow();
    Ok(PyProfileReport {
        inner: core_check_profile(&g),
    })
}

// ── hcdf.sdf ─────────────────────────────────────────────────────────────────────────────────────────

/// Read an SDF source (path, string, or bytes) to text.
fn read_sdf_src(src: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(pb) = src.downcast::<PyBytes>() {
        return String::from_utf8(pb.as_bytes().to_vec()).map_err(err);
    }
    if let Ok(ba) = src.downcast::<PyByteArray>() {
        return String::from_utf8(ba.to_vec()).map_err(err);
    }
    if let Ok(s) = src.extract::<String>() {
        if s.contains('<') {
            return Ok(s);
        }
        return std::fs::read_to_string(&s).map_err(err);
    }
    let s = pyany_to_str(src)?;
    std::fs::read_to_string(&s).map_err(err)
}

/// Import SDF (path, str, bytes) into an `hcdf.dom.Hcdf` model. Returns `(doc, notes)`. With a `baker`
/// (an `assets.Baker`), each visual/collision mesh is resolved and baked to a canonical asset there.
#[pyfunction]
#[pyo3(signature = (src, baker=None), name = "from_sdf")]
fn sdf_from_sdf(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    baker: Option<&Bound<'_, PyBaker>>,
) -> PyResult<(PyHcdf, Vec<String>)> {
    let sdf_text = read_sdf_src(src)?;
    match baker {
        None => {
            let (doc, notes) = from_sdf_str(&sdf_text).map_err(err)?;
            Ok((dom::new_handle(doc), notes))
        }
        Some(b) => {
            let b = b.borrow();
            let (doc, notes) = import_with_baking(py, ImportKind::Sdf, &sdf_text, &b)?;
            Ok((dom::new_handle(doc), notes))
        }
    }
}

/// Export an `hcdf.dom.Hcdf` model to SDFormat. Returns `(sdf_xml, LossManifest)`.
#[pyfunction]
#[pyo3(name = "to_sdf")]
fn sdf_to_sdf(doc: PyRef<'_, PyHcdf>) -> PyResult<(String, PyLossManifest)> {
    let g = doc.doc.borrow();
    let (xml, loss) = core_to_sdf(&g).map_err(err)?;
    Ok((xml, PyLossManifest { inner: loss }))
}

/// Which importer an `import_with_baking` call drives.
enum ImportKind {
    Urdf,
    Sdf,
}

/// Run the whole-document import-then-bake in the core, off a `Baker`'s resolution roots. Owns every
/// input before releasing the GIL for the filesystem I/O.
fn import_with_baking(
    py: Python<'_>,
    kind: ImportKind,
    src: &str,
    baker: &PyBaker,
) -> PyResult<(Hcdf, Vec<String>)> {
    let pkgs = package_map(&baker.package_paths);
    let src = src.to_string();
    let base = PathBuf::from(&baker.base_dir);
    let out = PathBuf::from(&baker.out_dir);
    let prefix = baker.uri_prefix.clone();
    py.allow_threads(move || match kind {
        ImportKind::Urdf => from_urdf_str_with_baking(&src, &base, &out, &prefix, &pkgs),
        ImportKind::Sdf => from_sdf_str_with_baking(&src, &base, &out, &prefix, &pkgs),
    })
    .map_err(err)
}

// ── hcdf.io ──────────────────────────────────────────────────────────────────────────────────────────

/// Resolve all `<include>` elements into a single document. Returns `(doc, notes)`. `src` is a path,
/// an HCDF XML string, or an `Hcdf` handle; with `fetch` on, remote `http(s)://` includes are fetched
/// and inlined (a fetch failure aborts).
#[pyfunction]
#[pyo3(signature = (src, base_dir=None, fetch=false, cache_dir=None), name = "flatten")]
fn io_flatten(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    base_dir: Option<String>,
    fetch: bool,
    cache_dir: Option<String>,
) -> PyResult<(PyHcdf, Vec<String>)> {
    if let Ok(h) = src.downcast::<PyHcdf>() {
        let xml = dom::handle_xml(&h.borrow())?;
        let base = base_dir.unwrap_or_else(|| ".".to_string());
        let (doc, notes) = flatten_in_memory(py, xml, base, fetch, cache_dir)?;
        return Ok((dom::new_handle(doc), notes));
    }
    let s = pyany_to_str(src)?;
    if s.trim_start().starts_with('<') {
        let base = base_dir.unwrap_or_else(|| ".".to_string());
        let (doc, notes) = flatten_in_memory(py, s, base, fetch, cache_dir)?;
        return Ok((dom::new_handle(doc), notes));
    }
    let path = abspath(&s);
    let (doc, notes) = if fetch {
        py.allow_threads(|| {
            core_flatten_path_fetch(Path::new(&path), cache_dir.as_deref().map(Path::new))
        })
        .map_err(err)?
    } else {
        py.allow_threads(|| flatten_path(Path::new(&path)))
            .map_err(err)?
    };
    Ok((dom::new_handle(doc), notes))
}

/// Flatten an in-memory document (parsed from `xml`), resolving LOCAL includes off `base` on disk.
fn flatten_in_memory(
    py: Python<'_>,
    xml: String,
    base: String,
    fetch: bool,
    cache_dir: Option<String>,
) -> PyResult<(Hcdf, Vec<String>)> {
    py.allow_threads(move || -> Result<(Hcdf, Vec<String>), String> {
        let mut doc = Hcdf::from_xml_str(&xml).map_err(|e| e.to_string())?;
        let notes = if fetch {
            core_flatten_fetch(
                &mut doc,
                Path::new(&base),
                cache_dir.as_deref().map(Path::new),
            )?
        } else {
            core_flatten_doc(&mut doc, Path::new(&base))?
        };
        Ok((doc, notes))
    })
    .map_err(err)
}

/// Populate/update every `<include>`'s `@sha` to its module's current bytes, without inlining. Returns
/// `(doc, notes)`. `src` is a path or an `Hcdf` handle.
#[pyfunction]
#[pyo3(signature = (src, base_dir=None), name = "stamp_include_shas")]
fn io_stamp_include_shas(
    src: &Bound<'_, PyAny>,
    base_dir: Option<String>,
) -> PyResult<(PyHcdf, Vec<String>)> {
    let (xml, base) = if let Ok(h) = src.downcast::<PyHcdf>() {
        (
            dom::handle_xml(&h.borrow())?,
            base_dir.unwrap_or_else(|| ".".to_string()),
        )
    } else {
        let path = abspath(&pyany_to_str(src)?);
        let xml = std::fs::read_to_string(&path).map_err(err)?;
        let base = Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".to_string());
        (xml, base)
    };
    let mut doc = Hcdf::from_xml_str(&xml).map_err(err)?;
    let notes = core_stamp_shas(&mut doc, Path::new(&base)).map_err(err)?;
    Ok((dom::new_handle(doc), notes))
}

/// Serialize a typed Hcdf DOM to a JSON-compatible dict (`{"hcdf": {...}}`), via the Rust core.
#[pyfunction]
#[pyo3(name = "to_json")]
fn io_to_json(py: Python<'_>, doc: PyRef<'_, PyHcdf>) -> PyResult<Py<PyAny>> {
    let xml = dom::handle_xml(&doc)?;
    let json_str = hcdformat::hcdf_xml_to_json(&xml).map_err(err)?;
    Ok(py
        .import("json")?
        .call_method1("loads", (json_str,))?
        .unbind())
}

/// Parse a JSON dict (or path or str) into a typed Hcdf DOM (via the Rust core).
#[pyfunction]
#[pyo3(name = "from_json")]
fn io_from_json(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<PyHcdf> {
    let src: String = if obj.is_instance_of::<PyDict>() || obj.is_instance_of::<PyList>() {
        py.import("json")?
            .call_method1("dumps", (obj,))?
            .extract()?
    } else if let Ok(s) = obj.extract::<String>() {
        let t = s.trim_start();
        if t.starts_with('{') || t.starts_with('[') {
            s
        } else {
            std::fs::read_to_string(&s).map_err(err)?
        }
    } else {
        std::fs::read_to_string(pyany_to_str(obj)?).map_err(err)?
    };
    let xml = hcdformat::json_to_hcdf_xml(&src).map_err(err)?;
    let doc = Hcdf::from_xml_str(&xml).map_err(err)?;
    Ok(dom::new_handle(doc))
}

/// A unique path in the cwd for materializing an in-memory document (so relative meshes resolve against
/// the cwd, matching the shim's `tempfile.mkstemp(dir=".")`).
fn temp_in_cwd(suffix: &str) -> PyResult<PathBuf> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = format!(".hcdf-pack-{}-{nonce}{suffix}", std::process::id());
    Ok(std::env::current_dir().map_err(err)?.join(name))
}

/// Pack a model and its meshes into a self-contained bundle. Returns a manifest dict. `src` is a path
/// to a `.hcdf` or an `Hcdf` handle. Keywords: `flatten` (defaults True and must stay True; a bundle is
/// always flattened, so flatten=False raises rather than silently flattening), `package_paths`,
/// `allow_partial`, `as_dir`, `vendor_remote`, `keep_remote`, `cache_dir`.
#[pyfunction]
#[pyo3(signature = (src, out_path, **kwargs), name = "pack")]
fn io_pack(
    py: Python<'_>,
    src: &Bound<'_, PyAny>,
    out_path: &str,
    kwargs: Option<&Bound<'_, PyDict>>,
) -> PyResult<Py<PyDict>> {
    reject_unknown_kwargs(
        kwargs,
        &[
            "flatten",
            "package_paths",
            "allow_partial",
            "as_dir",
            "vendor_remote",
            "keep_remote",
            "cache_dir",
        ],
    )?;
    // Only the flattened bundle shape is reachable here: the core's keep-live (structure-preserving)
    // packer is a separate in-memory pipeline that consumes caller-collected `<include>` modules and
    // returns a report shaped nothing like this manifest, so honour the documented contract by rejecting
    // flatten=False outright rather than accepting it and flattening anyway.
    if !kw_bool(kwargs, "flatten", true)? {
        return Err(PyValueError::new_err(
            "pack(flatten=False) is not supported: this bundler always flattens the document. The \
             structure-preserving keep-live pack is a separate pipeline not exposed through pack(); \
             omit flatten or pass flatten=True.",
        ));
    }
    let package_paths = kw_map(kwargs, "package_paths")?.unwrap_or_default();
    let allow_partial = kw_bool(kwargs, "allow_partial", false)?;
    let as_dir = kw_bool(kwargs, "as_dir", false)?;
    let vendor_remote = kw_bool(kwargs, "vendor_remote", false)?;
    let keep_remote = kw_bool(kwargs, "keep_remote", false)?;
    let cache_dir = kw_str(kwargs, "cache_dir")?;
    if vendor_remote && keep_remote {
        return Err(PyValueError::new_err(
            "vendor_remote and keep_remote are mutually exclusive; pick one \
             (CLI: --vendor-remote OR --keep-remote, not both)",
        ));
    }

    // An in-memory document is materialized in the cwd (its base for relative meshes) and packed as a
    // path; a genuine path is packed directly. The core has no in-memory-doc-plus-disk-meshes pack.
    let (src_path, temp) = if let Ok(h) = src.downcast::<PyHcdf>() {
        let xml = dom::handle_xml(&h.borrow())?;
        let tmp = temp_in_cwd(".hcdf")?;
        std::fs::write(&tmp, xml).map_err(err)?;
        (tmp.to_string_lossy().into_owned(), Some(tmp))
    } else {
        (abspath(&pyany_to_str(src)?), None)
    };

    let remote = if vendor_remote {
        RemotePolicy::Vendor
    } else if keep_remote {
        RemotePolicy::Keep
    } else {
        RemotePolicy::Error
    };
    let opts = PackOptions {
        package_paths: package_map(&package_paths),
        allow_partial,
        as_dir,
        remote,
        cache_dir: cache_dir.map(PathBuf::from),
    };
    let src_c = src_path.clone();
    let out_c = out_path.to_string();
    let packed = py.allow_threads(|| core_pack(Path::new(&src_c), Path::new(&out_c), &opts));
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    let manifest = packed.map_err(err)?;

    let d = PyDict::new(py);
    d.set_item("bundle", out_path)?;
    d.set_item("format", manifest.format)?;
    d.set_item("root", manifest.root)?;
    d.set_item("entries", manifest.entries)?;
    d.set_item("assets", rows_to_dicts(py, &manifest.assets)?)?;
    d.set_item("unresolved", rows_to_dicts(py, &manifest.unresolved)?)?;
    d.set_item("kept_remote", manifest.kept_remote)?;
    d.set_item("notes", manifest.notes)?;
    Ok(d.unbind())
}

/// Open a bundle. Returns `(doc, root_dir)` where mesh uris resolve under `root_dir`; the CALLER owns
/// `root_dir`'s lifetime (for a zip it is a tempdir that is NOT auto-cleaned).
#[pyfunction]
#[pyo3(name = "open_bundle")]
fn io_open_bundle(py: Python<'_>, path: &str) -> PyResult<(PyHcdf, String)> {
    let path = path.to_string();
    let (root_xml, root_dir) = py
        .allow_threads(|| -> Result<(String, String), String> {
            let opened = core_open_bundle(Path::new(&path))?;
            let root_xml = crate::read_root_hcdf(opened.root_dir.path())?;
            let root_dir = opened.root_dir.into_path();
            Ok((root_xml, root_dir.to_string_lossy().into_owned()))
        })
        .map_err(err)?;
    let doc = Hcdf::from_xml_str(&root_xml).map_err(err)?;
    Ok((dom::new_handle(doc), root_dir))
}

/// Verify a bundle's integrity. Returns `{"ok": bool, "issues": [str, ...]}`.
#[pyfunction]
#[pyo3(name = "verify")]
fn io_verify(py: Python<'_>, path: &str) -> PyResult<Py<PyDict>> {
    let path = path.to_string();
    let report = py.allow_threads(|| core_verify_bundle(Path::new(&path)));
    let d = PyDict::new(py);
    d.set_item("ok", report.ok)?;
    d.set_item("issues", report.issues)?;
    Ok(d.unbind())
}

// ── hcdf.assets ──────────────────────────────────────────────────────────────────────────────────────

/// A content address for asset bytes: `"sha256:<hex>"`.
#[pyfunction]
#[pyo3(name = "content_sha")]
fn assets_content_sha(data: &[u8]) -> String {
    core_content_sha(data)
}

/// The content address of a file's bytes.
#[pyfunction]
#[pyo3(name = "file_sha")]
fn assets_file_sha(path: &str) -> PyResult<String> {
    let data = std::fs::read(path).map_err(oserr)?;
    Ok(core_content_sha(&data))
}

/// Resolve a mesh URI to an existing filesystem path, or None.
#[pyfunction]
#[pyo3(signature = (uri, base_dir=".", package_paths=None), name = "resolve_uri")]
fn assets_resolve_uri(
    uri: &str,
    base_dir: &str,
    package_paths: Option<BTreeMap<String, String>>,
) -> Option<String> {
    if uri.is_empty() {
        return None;
    }
    resolve_uri_impl(uri, base_dir, &package_paths.unwrap_or_default())
}

/// Export a deterministic embedded GLB for a source mesh.
#[pyfunction]
#[pyo3(signature = (src_path, scale=None, color=None), name = "to_glb")]
fn assets_to_glb(
    py: Python<'_>,
    src_path: &str,
    scale: Option<&str>,
    color: Option<&str>,
) -> PyResult<Py<PyBytes>> {
    let data = to_glb_impl(src_path, scale, color)?;
    Ok(PyBytes::new(py, &data).unbind())
}

/// Bake `scale` into a lean collision mesh (folded into the vertices), mirror-safe. Returns bytes.
#[pyfunction]
#[pyo3(signature = (src_path, scale=None), name = "to_lean")]
fn assets_to_lean(py: Python<'_>, src_path: &str, scale: Option<&str>) -> PyResult<Py<PyBytes>> {
    let data = bake::to_lean(src_path, scale).map_err(err)?;
    Ok(PyBytes::new(py, &data).unbind())
}

/// Bake one mesh to a canonical asset in `out_dir`. Returns `(filename, sha)`.
#[pyfunction]
#[pyo3(signature = (src_path, scale, kind, out_dir, color=None), name = "bake")]
fn assets_bake(
    src_path: &str,
    scale: Option<&str>,
    kind: &str,
    out_dir: &str,
    color: Option<&str>,
) -> PyResult<(String, String)> {
    bake_impl(src_path, scale, kind, out_dir, color)
}

/// Fold an import-time `baker` into a Rust-imported `doc` using its per-visual asset `hints`. Returns
/// the set of baked `(comp, visual)` name pairs, so the caller can drop the now-consumed deferral notes.
#[pyfunction]
#[pyo3(name = "apply_import_baker")]
fn assets_apply_import_baker(
    py: Python<'_>,
    doc: &Bound<'_, PyHcdf>,
    hints: Vec<HintTuple>,
    baker: &Bound<'_, PyBaker>,
) -> PyResult<Py<PySet>> {
    let b = baker.borrow();
    let pkgs = package_map(&b.package_paths);
    let env = BakeEnv::direct(
        PathBuf::from(&b.base_dir),
        PathBuf::from(&b.out_dir),
        b.uri_prefix.clone(),
        &pkgs,
    );
    let hint_vec: Vec<VisualAssetHint> = hints
        .into_iter()
        .map(|(comp, visual, scale, color, texture)| VisualAssetHint {
            comp,
            visual,
            scale,
            color,
            texture,
        })
        .collect();
    let mut notes = Vec::new();
    let baked = {
        let handle = doc.borrow();
        let mut hcdf = handle.doc.borrow_mut();
        bake_document(&mut hcdf, &hint_vec, &env, &mut notes)
    };
    let set = PySet::empty(py)?;
    for pair in baked {
        set.add(pair)?;
    }
    Ok(set.unbind())
}

/// Drop the per-visual deferral notes for visuals that were actually baked. `baked_visuals` is an
/// iterable of `(comp, visual)` name pairs; a note is dropped iff it carries a deferral marker AND names
/// one of those baked pairs.
#[pyfunction]
#[pyo3(name = "drop_baked_notes")]
fn assets_drop_baked_notes(
    notes: Vec<String>,
    baked_visuals: &Bound<'_, PyAny>,
) -> PyResult<Vec<String>> {
    let mut baked = std::collections::BTreeSet::new();
    for item in baked_visuals.try_iter()? {
        let (comp, visual): (String, String) = item?.extract()?;
        baked.insert((comp, visual));
    }
    Ok(core_drop_baked_notes(notes, &baked))
}

/// Copy every referenced mesh into `assets_out_dir` and rewrite the DOM to point at it (in place).
/// Returns a manifest of `{site, kind, old_uri, new_uri, sha, status}` dicts. Keywords: `base_dir`,
/// `assets_out_dir` (required), `package_paths`, `uri_prefix`, `vendor_remote`, `cache_dir`.
#[pyfunction]
#[pyo3(signature = (doc, **kwargs), name = "vendor_assets")]
fn assets_vendor_assets(
    py: Python<'_>,
    doc: &Bound<'_, PyHcdf>,
    kwargs: Option<&Bound<'_, PyDict>>,
) -> PyResult<Py<PyList>> {
    reject_unknown_kwargs(
        kwargs,
        &[
            "base_dir",
            "assets_out_dir",
            "package_paths",
            "uri_prefix",
            "vendor_remote",
            "cache_dir",
        ],
    )?;
    let base_dir = kw_str(kwargs, "base_dir")?.unwrap_or_else(|| ".".to_string());
    let assets_out_dir = kw_str(kwargs, "assets_out_dir")?.ok_or_else(|| {
        PyValueError::new_err(
            "vendor_assets requires assets_out_dir (the directory to copy meshes into)",
        )
    })?;
    let package_paths = kw_map(kwargs, "package_paths")?.unwrap_or_default();
    let uri_prefix = kw_str(kwargs, "uri_prefix")?.unwrap_or_else(|| "assets".to_string());
    let vendor_remote = kw_bool(kwargs, "vendor_remote", false)?;
    let cache_dir = kw_str(kwargs, "cache_dir")?;

    let pkgs = package_map(&package_paths);
    let manifest = {
        let handle = doc.borrow();
        let mut hcdf = handle.doc.borrow_mut();
        core_vendor_assets(
            &mut hcdf,
            Path::new(&base_dir),
            Path::new(&assets_out_dir),
            &pkgs,
            &uri_prefix,
            vendor_remote,
            cache_dir.as_deref().map(Path::new),
        )
        .map_err(err)?
    };
    Ok(rows_to_dicts(py, &manifest)?.unbind())
}

// ── the console script (hcdf.main) ───────────────────────────────────────────────────────────────────

/// Forward argv to the Rust `hcdf` CLI and return its exit code. When `argv` is omitted, the process
/// `sys.argv` is used (argv[0] is the program name; clap ignores it).
#[pyfunction]
#[pyo3(signature = (argv=None), name = "main")]
fn cli_main(py: Python<'_>, argv: Option<Vec<String>>) -> i32 {
    let mut args = match argv {
        Some(a) => a,
        None => py
            .import("sys")
            .and_then(|s| s.getattr("argv"))
            .and_then(|a| a.extract::<Vec<String>>())
            .unwrap_or_default(),
    };
    if args.is_empty() {
        args.push("hcdf".to_string());
    }
    py.allow_threads(|| hcdformat::cli::run(args))
}

// ── type stubs for the public submodules ───────────────────────────────────────────────────────────────
//
// One `.pyi` per public submodule, written by the `gen_pyi` bin next to the extension and byte-checked by
// the `pyi_regen_is_a_no_op` drift test. These builders are pyo3-free (plain string assembly) so both the
// bin and the test run without a Python interpreter.

/// `hcdf/urdf.pyi`: the report types, the profile constants, and the URDF conversion functions.
pub fn urdf_pyi_text() -> String {
    let mut s = String::from(crate::pyi_header());
    s.push_str(
        "from typing import Optional\n\n\
         from .assets import Baker\n\
         from .dom import Hcdf\n\n\
         IDENTITY: str\n\
         WITH_TRANSFORM: str\n\
         OUT_OF_PROFILE: str\n\
         ERROR: str\n\
         WARNING: str\n\
         BENIGN_LOSS_CATEGORIES: frozenset[str]\n\n\
         class LossManifest:\n\
         \x20   def __init__(self) -> None: ...\n\
         \x20   def add(self, category: str, detail: str) -> None: ...\n\
         \x20   def __bool__(self) -> bool: ...\n\
         \x20   def __len__(self) -> int: ...\n\
         \x20   def text(self) -> str: ...\n\
         \x20   def categories(self) -> dict[str, list[str]]: ...\n\
         \x20   def to_dict(self) -> dict: ...\n\
         \x20   def to_json(self, indent: int = ...) -> str: ...\n\
         \x20   def markdown(self, title: Optional[str] = ...) -> str: ...\n\n\
         class Finding:\n\
         \x20   tier: str\n\
         \x20   code: str\n\
         \x20   detail: str\n\
         \x20   def __init__(self, tier: str, code: str, detail: str) -> None: ...\n\n\
         class Issue:\n\
         \x20   level: str\n\
         \x20   code: str\n\
         \x20   message: str\n\
         \x20   def __init__(self, level: str, code: str, message: str) -> None: ...\n\
         \x20   def __str__(self) -> str: ...\n\n\
         class ProfileReport:\n\
         \x20   classification: str\n\
         \x20   in_profile: bool\n\
         \x20   is_identity: bool\n\
         \x20   findings: list[Finding]\n\
         \x20   loss: LossManifest\n\
         \x20   issues: list[Issue]\n\
         \x20   nonbenign_losses: list[tuple[str, str]]\n\
         \x20   def by_tier(self, tier: str) -> list[Finding]: ...\n\
         \x20   def to_dict(self) -> dict: ...\n\
         \x20   def to_json(self, indent: int = ...) -> str: ...\n\
         \x20   def markdown(self) -> str: ...\n\n\
         def from_urdf(src: object, mappings: Optional[dict[str, str]] = ..., baker: Optional[Baker] = ..., packages: Optional[dict[str, str]] = ...) -> tuple[Hcdf, list[str]]: ...\n\
         def to_urdf(doc: Hcdf) -> tuple[str, LossManifest]: ...\n\
         def check_profile(doc: Hcdf) -> ProfileReport: ...\n",
    );
    s
}

/// `hcdf/sdf.pyi`: the SDF conversion functions (the loss type is shared with `hcdf.urdf`).
pub fn sdf_pyi_text() -> String {
    let mut s = String::from(crate::pyi_header());
    s.push_str(
        "from typing import Optional\n\n\
         from .assets import Baker\n\
         from .dom import Hcdf\n\
         from .urdf import LossManifest\n\n\
         def from_sdf(src: object, baker: Optional[Baker] = ...) -> tuple[Hcdf, list[str]]: ...\n\
         def to_sdf(doc: Hcdf) -> tuple[str, LossManifest]: ...\n",
    );
    s
}

/// `hcdf/io.pyi`: flatten/stamp, JSON, and bundle services, plus the DOM load/dump re-exports.
pub fn io_pyi_text() -> String {
    let mut s = String::from(crate::pyi_header());
    s.push_str(
        "from typing import Optional\n\n\
         from .dom import Hcdf\n\
         from .dom import dump as dump\n\
         from .dom import dumps as dumps\n\
         from .dom import load as load\n\n\
         def flatten(src: object, base_dir: Optional[str] = ..., fetch: bool = ..., cache_dir: Optional[str] = ...) -> tuple[Hcdf, list[str]]: ...\n\
         def stamp_include_shas(src: object, base_dir: Optional[str] = ...) -> tuple[Hcdf, list[str]]: ...\n\
         def to_json(doc: Hcdf) -> dict: ...\n\
         def from_json(obj: object) -> Hcdf: ...\n\
         def pack(src: object, out_path: str, **kwargs: object) -> dict: ...\n\
         def open_bundle(path: str) -> tuple[Hcdf, str]: ...\n\
         def verify(path: str) -> dict: ...\n",
    );
    s
}

/// `hcdf/assets.pyi`: the mesh baker class and the resolve/bake/vendor functions.
pub fn assets_pyi_text() -> String {
    let mut s = String::from(crate::pyi_header());
    s.push_str(
        "from typing import Optional\n\n\
         class Baker:\n\
         \x20   def __init__(self, out_dir: str, base_dir: Optional[str] = ..., package_paths: Optional[dict[str, str]] = ..., uri_prefix: Optional[str] = ...) -> None: ...\n\
         \x20   def __call__(self, uri: str, scale: Optional[str], kind: str, color: Optional[str] = ...) -> Optional[tuple[str, str]]: ...\n\n\
         def content_sha(data: bytes) -> str: ...\n\
         def file_sha(path: str) -> str: ...\n\
         def resolve_uri(uri: str, base_dir: str = ..., package_paths: Optional[dict[str, str]] = ...) -> Optional[str]: ...\n\
         def to_glb(src_path: str, scale: Optional[str] = ..., color: Optional[str] = ...) -> bytes: ...\n\
         def to_lean(src_path: str, scale: Optional[str] = ...) -> bytes: ...\n\
         def bake(src_path: str, scale: Optional[str], kind: str, out_dir: str, color: Optional[str] = ...) -> tuple[str, str]: ...\n\
         def apply_import_baker(doc: object, hints: list, baker: Baker) -> set: ...\n\
         def drop_baked_notes(notes: list[str], baked_visuals: object) -> list[str]: ...\n\
         def vendor_assets(doc: object, **kwargs: object) -> list[dict]: ...\n",
    );
    s
}

// ── registration ─────────────────────────────────────────────────────────────────────────────────────

/// Register a submodule on `parent`, name it under the parent's dotted path, and insert it into
/// `sys.modules` so `import <parent>.<short>` and `from <parent>.<short> import ...` both resolve cold.
fn install_submodule(
    py: Python<'_>,
    parent: &Bound<'_, PyModule>,
    child: &Bound<'_, PyModule>,
    parent_name: &str,
    short: &str,
) -> PyResult<()> {
    let full = format!("{parent_name}.{short}");
    child.setattr("__name__", &full)?;
    // Bind the child under its SHORT name on the parent (`<ext>.dom`). `add_submodule` keys the
    // attribute off the child's `__name__`, which is now the full dotted path, so it is not used here.
    parent.add(short, child)?;
    py.import("sys")?
        .getattr("modules")?
        .set_item(&full, child)?;
    Ok(())
}

/// Build the five `hcdf.*` submodules, register the report types and console script, and set the
/// top-level version attributes on the extension module.
pub fn register(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    let parent: String = m.name()?.extract()?;

    // hcdf.dom
    let dom_mod = PyModule::new(py, "dom")?;
    dom_mod.add(
        "__doc__",
        "The typed HCDF DOM: the neutral in-memory model the URDF/SDF/HCDF I/O adapters sit on.",
    )?;
    dom_mod.add("Hcdf", py.get_type::<PyHcdf>())?;
    dom_mod.add(
        "StreamProfileDocument",
        py.get_type::<PyStreamProfileDocument>(),
    )?;
    dom_mod.add_function(wrap_pyfunction!(dom_load, &dom_mod)?)?;
    dom_mod.add_function(wrap_pyfunction!(dom_loads, &dom_mod)?)?;
    dom_mod.add_function(wrap_pyfunction!(dom_dumps, &dom_mod)?)?;
    dom_mod.add_function(wrap_pyfunction!(dom_dump, &dom_mod)?)?;
    dom_mod.add_function(wrap_pyfunction!(load_stream_profile, &dom_mod)?)?;
    dom_mod.add_function(wrap_pyfunction!(loads_stream_profile, &dom_mod)?)?;
    dom_mod.add_function(wrap_pyfunction!(dumps_stream_profile, &dom_mod)?)?;
    dom_mod.add_function(wrap_pyfunction!(dump_stream_profile, &dom_mod)?)?;
    dom_mod.add(
        "__all__",
        vec![
            "Hcdf",
            "StreamProfileDocument",
            "load",
            "loads",
            "dumps",
            "dump",
            "load_stream_profile",
            "loads_stream_profile",
            "dumps_stream_profile",
            "dump_stream_profile",
        ],
    )?;
    install_submodule(py, m, &dom_mod, &parent, "dom")?;

    // hcdf.urdf
    let urdf_mod = PyModule::new(py, "urdf")?;
    urdf_mod.add("__doc__", "URDF <-> HCDF conversion on the typed DOM.")?;
    urdf_mod.add_class::<PyLossManifest>()?;
    urdf_mod.add_class::<PyProfileReport>()?;
    urdf_mod.add_class::<PyFinding>()?;
    urdf_mod.add_class::<PyIssue>()?;
    urdf_mod.add_function(wrap_pyfunction!(urdf_from_urdf, &urdf_mod)?)?;
    urdf_mod.add_function(wrap_pyfunction!(urdf_to_urdf, &urdf_mod)?)?;
    urdf_mod.add_function(wrap_pyfunction!(urdf_check_profile, &urdf_mod)?)?;
    urdf_mod.add("IDENTITY", Tier::Identity.label())?;
    urdf_mod.add("WITH_TRANSFORM", Tier::WithTransform.label())?;
    urdf_mod.add("OUT_OF_PROFILE", Tier::OutOfProfile.label())?;
    urdf_mod.add("ERROR", level_str(Level::Error))?;
    urdf_mod.add("WARNING", level_str(Level::Warning))?;
    let benign = py
        .import("builtins")?
        .getattr("frozenset")?
        .call1((BENIGN.to_vec(),))?;
    urdf_mod.add("BENIGN_LOSS_CATEGORIES", benign)?;
    urdf_mod.add(
        "__all__",
        vec![
            "from_urdf",
            "to_urdf",
            "check_profile",
            "LossManifest",
            "ProfileReport",
            "Finding",
            "Issue",
            "IDENTITY",
            "WITH_TRANSFORM",
            "OUT_OF_PROFILE",
            "BENIGN_LOSS_CATEGORIES",
            "ERROR",
            "WARNING",
        ],
    )?;
    install_submodule(py, m, &urdf_mod, &parent, "urdf")?;

    // hcdf.sdf
    let sdf_mod = PyModule::new(py, "sdf")?;
    sdf_mod.add(
        "__doc__",
        "SDFormat <-> HCDF conversion on the typed DOM (a first-class peer of hcdf.urdf).",
    )?;
    sdf_mod.add_function(wrap_pyfunction!(sdf_from_sdf, &sdf_mod)?)?;
    sdf_mod.add_function(wrap_pyfunction!(sdf_to_sdf, &sdf_mod)?)?;
    sdf_mod.add("__all__", vec!["from_sdf", "to_sdf"])?;
    install_submodule(py, m, &sdf_mod, &parent, "sdf")?;

    // hcdf.io
    let io_mod = PyModule::new(py, "io")?;
    io_mod.add("__doc__", "HCDF document services: flatten, bundle, JSON.")?;
    io_mod.add_function(wrap_pyfunction!(io_flatten, &io_mod)?)?;
    io_mod.add_function(wrap_pyfunction!(io_stamp_include_shas, &io_mod)?)?;
    io_mod.add_function(wrap_pyfunction!(io_to_json, &io_mod)?)?;
    io_mod.add_function(wrap_pyfunction!(io_from_json, &io_mod)?)?;
    io_mod.add_function(wrap_pyfunction!(io_pack, &io_mod)?)?;
    io_mod.add_function(wrap_pyfunction!(io_open_bundle, &io_mod)?)?;
    io_mod.add_function(wrap_pyfunction!(io_verify, &io_mod)?)?;
    // Convenience re-exports of the DOM load/dump/dumps (their authority is hcdf.dom).
    io_mod.add("load", dom_mod.getattr("load")?)?;
    io_mod.add("dump", dom_mod.getattr("dump")?)?;
    io_mod.add("dumps", dom_mod.getattr("dumps")?)?;
    io_mod.add(
        "__all__",
        vec![
            "flatten",
            "stamp_include_shas",
            "to_json",
            "from_json",
            "load",
            "dump",
            "dumps",
            "pack",
            "open_bundle",
            "verify",
        ],
    )?;
    install_submodule(py, m, &io_mod, &parent, "io")?;

    // hcdf.assets
    let assets_mod = PyModule::new(py, "assets")?;
    assets_mod.add(
        "__doc__",
        "Mesh asset pipeline: resolve, bake, and vendor meshes.",
    )?;
    assets_mod.add_class::<PyBaker>()?;
    assets_mod.add_function(wrap_pyfunction!(assets_content_sha, &assets_mod)?)?;
    assets_mod.add_function(wrap_pyfunction!(assets_file_sha, &assets_mod)?)?;
    assets_mod.add_function(wrap_pyfunction!(assets_resolve_uri, &assets_mod)?)?;
    assets_mod.add_function(wrap_pyfunction!(assets_to_glb, &assets_mod)?)?;
    assets_mod.add_function(wrap_pyfunction!(assets_to_lean, &assets_mod)?)?;
    assets_mod.add_function(wrap_pyfunction!(assets_bake, &assets_mod)?)?;
    assets_mod.add_function(wrap_pyfunction!(assets_apply_import_baker, &assets_mod)?)?;
    assets_mod.add_function(wrap_pyfunction!(assets_drop_baked_notes, &assets_mod)?)?;
    assets_mod.add_function(wrap_pyfunction!(assets_vendor_assets, &assets_mod)?)?;
    assets_mod.add(
        "__all__",
        vec![
            "content_sha",
            "file_sha",
            "resolve_uri",
            "to_glb",
            "to_lean",
            "bake",
            "Baker",
            "apply_import_baker",
            "drop_baked_notes",
            "vendor_assets",
        ],
    )?;
    install_submodule(py, m, &assets_mod, &parent, "assets")?;

    // Top-level entries on the extension.
    m.add_function(wrap_pyfunction!(cli_main, m)?)?;
    m.add("version", WHEEL_VERSION)?;
    m.add("__version__", WHEEL_VERSION)?;

    Ok(())
}
