//! HCDF bundling: pack a model and all its meshes into one self-contained artifact. A Rust port of
//! `hcdf_io/bundle.py` (`pack`/`open_bundle`/`verify`), matched function-for-function.
//!
//! A bundle pins a model's geometry to the exact bytes that produced each `@sha`, so it ships,
//! archives, or verifies with no resource-path setup. Two on-disk shapes, both FLAT (no `<include>`)
//! and document-relative (every mesh under `assets/`):
//!
//!   * a `.hcdfz` ZIP: the **root `.hcdf` is the FIRST entry** and every entry is `ZIP_STORED`
//!     (uncompressed, USDZ-style), so a blob's bytes are byte-identical to its `@sha`;
//!   * a loose directory (`as_dir`): a single `<name>.hcdf` beside an `assets/` tree, git-diffable.
//!
//! This step is flatten-only: [`pack`] runs [`crate::compose::flatten_path`] first so the bundle is
//! one document with no `<include>`, then the [`crate::assets_vendor`] step copies every mesh into the
//! bundle, in memory for the zip output (the bytes feed the zip writer directly) and staged on disk for
//! `--dir`.
//!
//! Native-only (`cfg(not(wasm32))`): it walks the local filesystem and uses the hand-rolled
//! [`crate::zip_store`] codec (no `zip`/flate dep, so the wasm/default dependency tree is untouched).
//! The remote-asset path (`vendor_remote=true`) additionally needs the `remote` feature.

#![cfg(not(target_arch = "wasm32"))]

use crate::assets_vendor::{self, VendorEntry, VendorStatus};
use crate::compose;
use crate::model::Hcdf;
use crate::zip_store::{self, ZipEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const ASSETS_DIR: &str = "assets";

/// The three-way handling of a remote asset/include at bundle time (`--vendor-remote` /
/// `--keep-remote` / neither), mirroring the Python `vendor_remote`/`keep_remote` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemotePolicy {
    /// Fetch + embed every remote mesh/include (output has NO `http(s)` uri). Needs the `remote`
    /// feature.
    Vendor,
    /// Leave remote uris live in the bundle (a deliberate choice, not an error).
    Keep,
    /// A remote uri is an explicit error naming it (so a network dependency never bundles by accident).
    Error,
}

impl RemotePolicy {
    fn vendor(self) -> bool {
        matches!(self, RemotePolicy::Vendor)
    }
    fn keep(self) -> bool {
        matches!(self, RemotePolicy::Keep)
    }
}

/// Options for [`pack`] (the analogue of `bundle.pack`'s keyword args).
pub struct PackOptions {
    /// `--package PKG=PATH` map for resolving `package://` / `model://` mesh uris.
    pub package_paths: BTreeMap<String, PathBuf>,
    /// Leave unresolved LOCAL meshes in the manifest instead of erroring (`--allow-partial`).
    pub allow_partial: bool,
    /// Write a loose directory instead of a `.hcdfz` zip (`--dir`).
    pub as_dir: bool,
    /// How to handle remote (`http(s)`) assets/includes.
    pub remote: RemotePolicy,
    /// Where fetched remote bytes are content-addressed (defaults to a per-user cache when `None`).
    pub cache_dir: Option<PathBuf>,
}

impl Default for PackOptions {
    fn default() -> Self {
        PackOptions {
            package_paths: BTreeMap::new(),
            allow_partial: false,
            as_dir: false,
            remote: RemotePolicy::Error,
            cache_dir: None,
        }
    }
}

/// The manifest [`pack`] returns (the Rust analogue of the Python dict).
#[derive(Debug, Clone)]
pub struct PackManifest {
    pub bundle: PathBuf,
    pub format: &'static str,
    pub root: String,
    pub entries: usize,
    pub assets: Vec<VendorEntry>,
    pub unresolved: Vec<VendorEntry>,
    pub kept_remote: Vec<String>,
    pub notes: Vec<String>,
}

/// Pack the model at `src` and its meshes into a self-contained bundle at `out_path`. Port of
/// `bundle.pack` (always `flatten=True`; a structure-preserving, non-flattened bundle is not supported,
/// so there is no `flatten=false` here).
///
/// The document is FIRST flattened (so the bundle has no `<include>`), then every visual `<model>` and
/// collision `<mesh>` is vendored into `assets/`, leaving only document-relative `assets/<name>` uris
/// carrying the matching `@sha`. An unresolved LOCAL mesh errors unless `allow_partial`. A remote
/// (`http(s)`) mesh/include obeys [`RemotePolicy`].
pub fn pack(src: &Path, out_path: &Path, opts: &PackOptions) -> Result<PackManifest, String> {
    let src = compose_abspath(src);
    let base_dir = src
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // A remote <include> is fetched + inlined ONLY under Vendor; with Keep (or Error) it is left as a
    // live <include> for flatten, then surfaced below as a remote site for the same keep/error choice.
    let (mut doc, notes) =
        flatten_for_bundle(&src, &base_dir, opts.remote, opts.cache_dir.as_deref())?;

    // A remote <include> the integrator chose NOT to vendor survives flatten as an <include>. Treat it
    // as an unresolved REMOTE site so the keep/error choice applies (rather than letting an <include>
    // silently ride along in a flatten-only bundle).
    let remote_includes: Vec<String> = doc
        .include
        .iter()
        .filter_map(|inc| inc.uri.clone())
        .filter(|u| is_remote(u))
        .collect();

    // Under Vendor, every remote include MUST have been fetched + inlined by flatten(fetch=true). A
    // survivor would mean a live http <include> leaked into a self-contained bundle: a contradiction.
    // Guard it (belt + suspenders).
    if opts.remote.vendor() && !remote_includes.is_empty() {
        return Err(format!(
            "--vendor-remote could not fetch + embed {} remote include(s): {}. A self-contained \
             bundle must inline every remote <include>; a survivor means its fetch failed.",
            remote_includes.len(),
            remote_includes.join(", ")
        ));
    }

    // The zip path vendors IN MEMORY: the bytes feed `write_stored` directly, no staging-dir
    // write + read-back. Only the loose-dir output stages into a fresh temp dir first (write_dir
    // publishes by copy).
    let staging = if opts.as_dir {
        Some(make_staging_dir()?)
    } else {
        None
    };
    let result = (|| -> Result<PackManifest, String> {
        let mut mem_assets: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let vend = match &staging {
            Some(staging) => assets_vendor::vendor_assets(
                &mut doc,
                &base_dir,
                &staging.join(ASSETS_DIR),
                &opts.package_paths,
                ASSETS_DIR,
                opts.remote.vendor(),
                opts.cache_dir.as_deref(),
            )?,
            None => assets_vendor::vendor_assets_mem(
                &mut doc,
                &base_dir,
                &mut mem_assets,
                &opts.package_paths,
                ASSETS_DIR,
                opts.remote.vendor(),
                opts.cache_dir.as_deref(),
            )?,
        };

        // Apply the three-way remote choice to mesh sites flagged Remote + any kept remote include.
        let remote_meshes: Vec<&VendorEntry> = vend
            .iter()
            .filter(|e| e.status == VendorStatus::Remote)
            .collect();
        let mut remote_sites: Vec<String> = remote_meshes
            .iter()
            .map(|e| format!("{} ({})", e.site, e.old_uri))
            .collect();
        remote_sites.extend(remote_includes.iter().map(|u| format!("<include> ({u})")));

        if !remote_sites.is_empty() && opts.remote == RemotePolicy::Error {
            return Err(format!(
                "{} remote (http/https) asset(s) referenced: {}. Choose --vendor-remote to fetch + \
                 embed them (self-contained bundle) or --keep-remote to leave the live web uris in \
                 the bundle.",
                remote_sites.len(),
                remote_sites.join(", ")
            ));
        }

        let unresolved: Vec<VendorEntry> = vend
            .iter()
            .filter(|e| e.status == VendorStatus::Unresolved)
            .cloned()
            .collect();
        if !unresolved.is_empty() && !opts.allow_partial {
            let sites = unresolved
                .iter()
                .map(|e| format!("{} ({})", e.site, e.old_uri))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "{} mesh asset(s) could not be resolved for bundling: {sites}. Pass \
                 allow_partial=true (CLI: --allow-partial) to bundle anyway.",
                unresolved.len()
            ));
        }

        let root_name = root_name(&doc, &src);
        let root_xml = doc
            .to_xml_string()
            .map_err(|e| format!("could not serialize bundle root document: {e}"))?;

        let entries = if let Some(staging) = &staging {
            // The bundle holds exactly the meshes the (deduped) vendor step wrote: list from disk.
            let assets_dir = staging.join(ASSETS_DIR);
            let mut asset_files: Vec<String> = if assets_dir.is_dir() {
                std::fs::read_dir(&assets_dir)
                    .map_err(|e| format!("read assets dir: {e}"))?
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            } else {
                Vec::new()
            };
            asset_files.sort();
            write_dir(out_path, &root_name, &root_xml, &assets_dir, &asset_files)?
        } else {
            write_zip(out_path, &root_name, &root_xml, &mem_assets)?
        };

        Ok(PackManifest {
            bundle: out_path.to_path_buf(),
            format: if opts.as_dir { "dir" } else { "hcdfz" },
            root: root_name,
            entries,
            assets: vend.clone(),
            unresolved,
            kept_remote: if opts.remote.keep() {
                remote_sites
            } else {
                Vec::new()
            },
            notes,
        })
    })();

    // Always clean the staging dir (dir output only), like Python's TemporaryDirectory context.
    if let Some(staging) = &staging {
        let _ = std::fs::remove_dir_all(staging);
    }
    result
}

/// Flatten `src` for the bundle, FETCHING remote `<include>`s only under [`RemotePolicy::Vendor`].
///
/// Under `Vendor` (and when the crate is built with the `remote` feature) this routes through
/// [`compose::flatten_path_fetch`], so a `http(s)://` `<include>` is fetched + inlined in Rust and a
/// self-contained `--vendor-remote` bundle never carries a live `<include>` (a fetch failure aborts the
/// pack, matching `include.py` flatten(fetch=True)). Under `Keep`/`Error` (or when the `remote` feature
/// is off) a remote include SURVIVES flatten as an `<include>` and is surfaced by [`pack`] as a remote
/// site for the keep/error choice. Local includes flatten exactly as Python either way.
fn flatten_for_bundle(
    src: &Path,
    _base_dir: &Path,
    remote: RemotePolicy,
    cache_dir: Option<&Path>,
) -> Result<(Hcdf, Vec<String>), String> {
    #[cfg(feature = "remote")]
    {
        if remote.vendor() {
            return compose::flatten_path_fetch(src, cache_dir);
        }
    }
    #[cfg(not(feature = "remote"))]
    {
        let _ = (remote, cache_dir);
    }
    compose::flatten_path(src)
}

/// True iff `uri` is an `http(s)://` URL (`bundle._is_remote`).
fn is_remote(uri: &str) -> bool {
    let low = uri.to_ascii_lowercase();
    low.starts_with("http://") || low.starts_with("https://")
}

/// The bundle's root `.hcdf` filename: the document name, else the source stem, else `root`, sanitized
/// to a confined single-component name ([`crate::bundle_mem::safe_root_stem`]) so a hostile `doc.name`
/// cannot widen the write path. Port of `bundle._root_name`, hardened on the write side to match the
/// zip-slip confinement the read side ([`safe_join`]) already applies.
fn root_name(doc: &Hcdf, src: &Path) -> String {
    let stem = if !doc.name.is_empty() {
        doc.name.clone()
    } else {
        src.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".to_string())
    };
    format!("{}.hcdf", crate::bundle_mem::safe_root_stem(&stem, "root"))
}

/// Write a root-first, all-`ZIP_STORED` `.hcdfz` from the in-memory vendored assets (content-addressed
/// name -> bytes; the `BTreeMap` iterates in the same sorted-filename order the staged path listed).
/// The root `.hcdf` MUST be the first entry. Port of `bundle._write_zip`.
fn write_zip(
    out_path: &Path,
    root_name: &str,
    root_xml: &str,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<usize, String> {
    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir for bundle: {e}"))?;
        }
    }
    // Build the entry list with the root FIRST, then write the STORED archive in that order. The asset
    // bytes are borrowed straight from the vendor step's map, never re-read or copied.
    let named: Vec<(String, &[u8])> = assets
        .iter()
        .map(|(name, data)| (format!("{ASSETS_DIR}/{name}"), data.as_slice()))
        .collect();
    let mut entries: Vec<ZipEntry> = Vec::with_capacity(1 + named.len());
    entries.push(ZipEntry {
        name: root_name,
        data: root_xml.as_bytes(),
    });
    for (name, data) in &named {
        entries.push(ZipEntry { name, data });
    }
    let mut f = std::fs::File::create(out_path).map_err(|e| format!("create bundle: {e}"))?;
    let n = zip_store::write_stored(&mut f, &entries).map_err(|e| format!("write zip: {e}"))?;
    Ok(n)
}

/// Write a loose bundle directory: `out_path/<root_name>` beside `out_path/assets/`. Clears any
/// previously-packed bundle (top-level `*.hcdf` + the `assets/` subtree) first so the directory always
/// reflects exactly ONE bundle. Port of `bundle._write_dir`.
fn write_dir(
    out_path: &Path,
    root_name: &str,
    root_xml: &str,
    assets_dir: &Path,
    asset_files: &[String],
) -> Result<usize, String> {
    std::fs::create_dir_all(out_path).map_err(|e| format!("mkdir bundle dir: {e}"))?;
    if out_path.is_dir() {
        for entry in std::fs::read_dir(out_path).map_err(|e| format!("read bundle dir: {e}"))? {
            let entry = entry.map_err(|e| format!("read bundle dir entry: {e}"))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let full = entry.path();
            if name == ASSETS_DIR && full.is_dir() {
                let _ = std::fs::remove_dir_all(&full);
            } else if name.ends_with(".hcdf") && full.is_file() {
                let _ = std::fs::remove_file(&full);
            }
        }
    }
    std::fs::write(out_path.join(root_name), root_xml.as_bytes())
        .map_err(|e| format!("write root .hcdf: {e}"))?;
    if !asset_files.is_empty() {
        let dest = out_path.join(ASSETS_DIR);
        std::fs::create_dir_all(&dest).map_err(|e| format!("mkdir assets: {e}"))?;
        for fn_ in asset_files {
            std::fs::copy(assets_dir.join(fn_), dest.join(fn_))
                .map_err(|e| format!("copy asset {fn_:?}: {e}"))?;
        }
    }
    Ok(1 + asset_files.len())
}

/// The directory a bundle's mesh uris resolve under, with its ownership tracked so a `.hcdfz` open's
/// extraction tempdir is cleaned up automatically (RAII).
///
/// A `.hcdfz` is extracted to a fresh temp directory this guard OWNS: dropping the guard (when the
/// owning [`OpenedBundle`] drops) removes that directory, so a caller that only reads the doc and its
/// meshes never leaks a per-open tempdir. A loose-directory bundle BORROWS the directory the caller
/// passed in; dropping the guard never touches it. A caller that must RETAIN an extracted tempdir (an
/// editor that keeps the meshes on disk, a shim that hands the path to another runtime) calls
/// [`keep`](BundleDir::keep) / [`into_path`](BundleDir::into_path) to defuse the guard and take the path
/// itself; it is then responsible for removing an owned extraction dir.
pub struct BundleDir {
    path: PathBuf,
    /// True iff `path` is an extraction tempdir this guard removes on drop; false for a borrowed dir.
    owned: bool,
}

impl BundleDir {
    /// The directory mesh uris resolve under.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True iff this is an OWNED extraction tempdir (a `.hcdfz` open) the guard removes on drop; false
    /// for a loose-directory bundle opened in place, which the guard never deletes.
    pub fn is_temp(&self) -> bool {
        self.owned
    }

    /// Defuse the guard and return the directory path, transferring ownership to the caller: the
    /// directory is NOT removed on drop, so a caller that keeps an owned extraction tempdir is
    /// responsible for removing it. For a borrowed (loose-dir) bundle this simply returns the borrowed
    /// path.
    pub fn into_path(mut self) -> PathBuf {
        self.owned = false;
        std::mem::take(&mut self.path)
    }

    /// Defuse the guard and return the directory path, reading as an explicit "keep this directory
    /// alive past the guard" at the call site. Identical to [`into_path`](BundleDir::into_path).
    pub fn keep(self) -> PathBuf {
        self.into_path()
    }
}

impl Drop for BundleDir {
    fn drop(&mut self) {
        // Only an owned extraction tempdir is removed; a borrowed loose-dir bundle is never deleted.
        if self.owned {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// An opened bundle: the root document and the [`BundleDir`] its mesh uris resolve under.
pub struct OpenedBundle {
    pub doc: Hcdf,
    /// The directory mesh uris resolve under, ownership-tracked. For a `.hcdfz` this is an OWNED
    /// extraction tempdir (removed when this [`OpenedBundle`] drops, unless the caller takes it via
    /// [`BundleDir::keep`] / [`BundleDir::into_path`]); for a loose directory it is the borrowed
    /// directory itself. Read the path with [`BundleDir::path`], and whether it is a tempdir with
    /// [`BundleDir::is_temp`].
    pub root_dir: BundleDir,
    /// True iff this is a KEEP-LIVE bundle (its `<include>`s are PRESERVED, with the referenced modules
    /// embedded under `modules/` and extracted alongside the root so they resolve LIVE off `root_dir`). For
    /// the status line only; resolution needs nothing extra (the tempdir extraction already placed the
    /// module files). A flat bundle is `false`. Detected from the presence of any `modules/` archive entry
    /// (or the `_keeplive` marker) written by [`crate::bundle_mem::pack_to_bytes_keep_live`].
    pub keep_live: bool,
}

/// Open a bundle. Returns the root [`Hcdf`] and the `root_dir` mesh uris resolve under. A `.hcdfz`
/// (any STORED zip) is extracted to a fresh tempdir (CALLER-OWNED, remove it when done); a loose
/// directory is used as-is. The root document is the FIRST zip entry (by the pack contract) or the
/// single top-level `.hcdf` in a directory. Port of `bundle.open_bundle`.
pub fn open_bundle(path: &Path) -> Result<OpenedBundle, String> {
    if path.is_dir() {
        let root_name = dir_root_name(path)?;
        let xml = std::fs::read_to_string(path.join(&root_name))
            .map_err(|e| format!("read root .hcdf: {e}"))?;
        let doc = Hcdf::from_xml_str(&xml).map_err(|e| format!("parse root .hcdf: {e}"))?;
        // A loose keep-live bundle would carry a `modules/` subtree beside the root; detect it for status.
        let keep_live = path.join("modules").is_dir();
        return Ok(OpenedBundle {
            doc,
            root_dir: BundleDir {
                path: path.to_path_buf(),
                owned: false,
            },
            keep_live,
        });
    }
    let bytes = zip_store::read_file_bytes(path).map_err(|e| format!("read bundle: {e}"))?;
    if !zip_store::is_zip(&bytes) {
        return Err(format!(
            "not an HCDF bundle (neither a directory nor a zip): {path:?}"
        ));
    }
    let entries = zip_store::read_stored(&bytes).map_err(|e| format!("read bundle zip: {e}"))?;
    let root_name = zip_root_name(&entries)?;
    // Keep-live iff any `modules/` entry (or the `_keeplive` marker) is present: the includes survive and
    // the module files are extracted alongside the root, so they resolve live off the tempdir below.
    let keep_live = entries
        .iter()
        .any(|e| e.name == "_keeplive" || e.name.starts_with("modules/"));
    let root_dir = make_extract_dir()?;
    let extract = (|| -> Result<Hcdf, String> {
        for e in &entries {
            // Confine extraction to the dir: a hand-crafted bundle could carry an absolute/`..` name.
            let dest = safe_join(&root_dir, &e.name)?;
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|x| format!("mkdir on extract: {x}"))?;
            }
            std::fs::write(&dest, &e.data).map_err(|x| format!("write on extract: {x}"))?;
        }
        let xml = std::fs::read_to_string(root_dir.join(&root_name))
            .map_err(|e| format!("read root .hcdf: {e}"))?;
        Hcdf::from_xml_str(&xml).map_err(|e| format!("parse root .hcdf: {e}"))
    })();
    match extract {
        Ok(doc) => Ok(OpenedBundle {
            doc,
            root_dir: BundleDir {
                path: root_dir,
                owned: true,
            },
            keep_live,
        }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&root_dir);
            Err(e)
        }
    }
}

/// The first zip entry's name, required to be a `.hcdf` (the pack contract). Port of
/// `bundle._zip_root_name`.
fn zip_root_name(entries: &[zip_store::ReadEntry]) -> Result<String, String> {
    let first = entries
        .first()
        .ok_or_else(|| "empty bundle zip (no entries)".to_string())?;
    if !first.name.ends_with(".hcdf") {
        return Err(format!(
            "bundle root is not the first zip entry (first entry is {:?})",
            first.name
        ));
    }
    Ok(first.name.clone())
}

/// The single top-level `.hcdf` in `root_dir` (error if none / multiple). Port of
/// `bundle._dir_root_name`.
fn dir_root_name(root_dir: &Path) -> Result<String, String> {
    let mut roots: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(root_dir).map_err(|e| format!("read bundle dir: {e}"))? {
        let entry = entry.map_err(|e| format!("read dir entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".hcdf") && entry.path().is_file() {
            roots.push(name);
        }
    }
    roots.sort();
    match roots.len() {
        0 => Err(format!(
            "no root .hcdf found in bundle directory {root_dir:?}"
        )),
        1 => Ok(roots.remove(0)),
        _ => Err(format!(
            "ambiguous bundle directory {root_dir:?}: multiple top-level .hcdf: {roots:?}"
        )),
    }
}

/// The result of [`verify`]: `ok` is true iff there are no integrity issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub ok: bool,
    pub issues: Vec<String>,
}

/// Verify a bundle's integrity. Port of `bundle.verify`: checks the root `.hcdf` is FIRST (zip) /
/// present (dir), the bundle is flatten-only (a surviving NON-remote `<include>` is flagged; a live
/// `--keep-remote` http(s) include is the one allowed survivor), and every visual `<model>` /
/// collision `<mesh>` blob's `content_sha` equals its `@sha` and the filename short-sha matches.
///
/// A `.hcdfz` is verified entirely IN MEMORY from ONE read of the archive (doc uris map straight to
/// entry names; blobs hash in place): no tempdir extraction, no per-blob re-read. A loose directory is
/// verified off disk as before.
pub fn verify(path: &Path) -> VerifyReport {
    if path.is_dir() {
        verify_dir(path)
    } else {
        verify_zip(path)
    }
}

/// [`verify`] for a loose bundle directory: open in place, then check includes + blobs off disk.
fn verify_dir(path: &Path) -> VerifyReport {
    let mut issues: Vec<String> = Vec::new();
    if let Err(e) = dir_root_name(path) {
        return VerifyReport {
            ok: false,
            issues: vec![e],
        };
    }
    let opened = match open_bundle(path) {
        Ok(o) => o,
        Err(e) => {
            issues.push(format!("root document could not be parsed: {e}"));
            return VerifyReport { ok: false, issues };
        }
    };
    // A directory opens in place, so `root_dir` is a BORROWED path the guard never deletes on drop.
    let OpenedBundle {
        doc,
        root_dir,
        keep_live,
    } = opened;

    if keep_live {
        // KEEP-LIVE bundle: the `<include>`s SURVIVE by design (repointed at the embedded `modules/…`
        // entries). Instead of flagging them, verify each include's `@sha` against the embedded module it
        // points at via the SHARED seam (the ONE "matches" definition), and check the module MESH blobs.
        issues.extend(verify_keep_live_includes(&doc, root_dir.path()));
        issues.extend(check_module_mesh_blobs(root_dir.path()));
    } else {
        issues.extend(flat_include_issues(&doc));
    }
    issues.extend(check_doc_mesh_blobs(&doc, "", &mut |uri, sha, site| {
        check_blob(root_dir.path(), uri, sha, site)
    }));

    VerifyReport {
        ok: issues.is_empty(),
        issues,
    }
}

/// [`verify`] for a `.hcdfz` on disk: read the archive bytes, then delegate to [`verify_bytes`], which
/// runs every check against the in-memory entries. A file that cannot be read or is not a ZIP_STORED
/// archive is the "not an HCDF bundle" issue (naming the path); everything else is exactly what
/// [`verify_bytes`] reports on the same bytes, so a disk verify and a byte verify of one bundle agree.
fn verify_zip(path: &Path) -> VerifyReport {
    match zip_store::read_file_bytes(path) {
        Ok(bytes) if zip_store::is_zip(&bytes) => verify_bytes(&bytes),
        _ => VerifyReport {
            ok: false,
            issues: vec![format!("not an HCDF bundle: {path:?}")],
        },
    }
}

/// Verify a `.hcdfz` bundle's integrity from its RAW ARCHIVE BYTES, entirely in memory (no filesystem):
/// the SAME checks [`verify`] runs on a `.hcdfz` (root `.hcdf` FIRST, flatten-only unless keep-live, every
/// visual `<model>` / collision `<mesh>` blob's `content_sha` equals its `@sha` + the filename short-sha),
/// resolved against the archive entries instead of an extraction tempdir. [`verify`] on a zip path reads
/// the file and calls this, so the two agree entry-for-entry on identical bytes. A loose-directory bundle
/// has no single byte image; verify it through [`verify`] with its path. `ok` is true iff `issues` empty.
pub fn verify_bytes(bytes: &[u8]) -> VerifyReport {
    let mut issues: Vec<String> = Vec::new();
    if !zip_store::is_zip(bytes) {
        return VerifyReport {
            ok: false,
            issues: vec!["not an HCDF bundle (bytes are not a ZIP_STORED archive)".to_string()],
        };
    }
    // read_stored already rejects non-STORED entries; surface a parse/STORED failure here. A non-STORED
    // (compressed) entry is exactly the "not ZIP_STORED" issue (and the second line is what opening the
    // same archive reported).
    let entries = match zip_store::read_stored(bytes) {
        Ok(entries) => entries,
        Err(e) => {
            issues.push(format!("bundle zip is not all-STORED / unreadable: {e}"));
            issues.push(format!(
                "root document could not be parsed: read bundle zip: {e}"
            ));
            return VerifyReport { ok: false, issues };
        }
    };

    // Root-first contract (the pack contract `zip_root_name` enforces on open).
    if entries.first().map(|e| e.name.ends_with(".hcdf")) != Some(true) {
        let names: Vec<&str> = entries.iter().take(3).map(|e| e.name.as_str()).collect();
        issues.push(format!(
            "root .hcdf is not the first zip entry (entries: {names:?})"
        ));
        issues.push(match entries.first() {
            None => "root document could not be parsed: empty bundle zip (no entries)".to_string(),
            Some(first) => format!(
                "root document could not be parsed: bundle root is not the first zip entry \
                 (first entry is {:?})",
                first.name
            ),
        });
        return VerifyReport { ok: false, issues };
    }

    // The extractor's zip-slip guard, minus the extraction: an absolute or `..`-escaping entry NAME is
    // rejected exactly like [`safe_join`] would on extract (same message, first offender wins).
    for e in &entries {
        if let Err(msg) = guard_entry_name(&e.name) {
            issues.push(format!("root document could not be parsed: {msg}"));
            return VerifyReport { ok: false, issues };
        }
    }

    // Parse the root doc straight from the first entry (no write-out + read-back).
    let doc = match std::str::from_utf8(&entries[0].data) {
        Err(_) => {
            // The disk path surfaced invalid UTF-8 via read_to_string; keep its exact message.
            issues.push(
                "root document could not be parsed: read root .hcdf: stream did not contain valid \
                 UTF-8"
                    .to_string(),
            );
            return VerifyReport { ok: false, issues };
        }
        Ok(xml) => match Hcdf::from_xml_str(xml) {
            Err(e) => {
                issues.push(format!(
                    "root document could not be parsed: parse root .hcdf: {e}"
                ));
                return VerifyReport { ok: false, issues };
            }
            Ok(d) => d,
        },
    };

    // Keep-live detection, exactly as `open_bundle` flags it.
    let keep_live = entries
        .iter()
        .any(|e| e.name == "_keeplive" || e.name.starts_with("modules/"));
    // Entry name -> bytes (a later duplicate wins, matching extraction's overwrite order).
    let by_name: BTreeMap<&str, &[u8]> = entries
        .iter()
        .map(|e| (e.name.as_str(), e.data.as_slice()))
        .collect();

    if keep_live {
        // Same keep-live checks as the dir path, resolved against the entry map instead of a tempdir.
        issues.extend(verify_keep_live_includes_mem(&doc, &by_name));
        issues.extend(check_module_mesh_blobs_mem(&entries, &by_name));
    } else {
        issues.extend(flat_include_issues(&doc));
    }
    issues.extend(check_doc_mesh_blobs(&doc, "", &mut |uri, sha, site| {
        check_blob_mem(&by_name, "", uri, sha, site)
    }));

    VerifyReport {
        ok: issues.is_empty(),
        issues,
    }
}

/// FLAT-bundle include check: a NON-remote `<include>` can never legitimately survive packing (a
/// `--keep-remote` live http(s) include is the one allowed survivor).
fn flat_include_issues(doc: &Hcdf) -> Vec<String> {
    let mut issues = Vec::new();
    for inc in &doc.include {
        let is_kept_remote = inc.uri.as_deref().map(is_remote) == Some(true);
        if !is_kept_remote {
            issues.push(format!(
                "bundle contains an unflattened <include uri={:?}>: a bundle must be flatten-only \
                 (only a --keep-remote live http(s) include may survive)",
                inc.uri
            ));
        }
    }
    issues
}

/// Walk every visual `<model>` / collision `<mesh>` uri in `doc` and run `check(uri, sha, site)` on it;
/// `site_prefix` tags module-embedded docs (`"module <usha>: "`) vs the root (`""`). The one mesh-site
/// walk shared by the dir and in-memory verifiers, for root docs AND keep-live module docs.
fn check_doc_mesh_blobs<F>(doc: &Hcdf, site_prefix: &str, check: &mut F) -> Vec<String>
where
    F: FnMut(&str, Option<&str>, &str) -> Vec<String>,
{
    let mut issues = Vec::new();
    for comp in &doc.comp {
        for v in &comp.visual {
            if let crate::model::VisualAppearance::Model { model, .. } = &v.appearance {
                if let Some(uri) = model.uri.as_deref().filter(|u| !u.is_empty()) {
                    issues.extend(check(
                        uri,
                        model.sha.as_deref(),
                        &format!("{site_prefix}{}/{} (visual model)", comp.name, v.name),
                    ));
                }
            }
        }
        for c in &comp.collision {
            if let Some(mesh) = c.geometry.as_ref().and_then(|g| g.mesh.as_ref()) {
                if let Some(uri) = mesh.uri.as_deref().filter(|u| !u.is_empty()) {
                    let cname = c.name.clone().unwrap_or_default();
                    issues.extend(check(
                        uri,
                        mesh.sha.as_deref(),
                        &format!("{site_prefix}{}/{cname} (collision mesh)", comp.name),
                    ));
                }
            }
        }
    }
    issues
}

/// Verify a KEEP-LIVE bundle's `<include> @sha`s against the embedded module each resolves to, via the
/// shared [`compose::verify_include_shas`] seam (the SAME "matches" definition dendrite's load-check uses).
/// The loader reads the extracted module file: its RAW bytes ARE the packer's embedded bytes, so the
/// pinned sha (= `content_sha` of those bytes) matches under the raw branch of the dual-accept rule. A
/// nested module include uri is ROOT-relative (`modules/<usha>/…`); since every module lives flat under
/// `root_dir/modules/<usha>/`, the loader recovers the right file by rejoining the LAST `modules/` segment.
fn verify_keep_live_includes(doc: &Hcdf, root_dir: &Path) -> Vec<String> {
    let root_abs = compose_abspath(root_dir);
    let mut loader = |key: &str, _base: &Path| -> Result<(Hcdf, Option<String>), String> {
        let mut path = PathBuf::from(key);
        if !path.exists() {
            if let Some(pos) = key.rfind("modules/") {
                path = root_abs.join(&key[pos..]);
            }
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        let sha = compose::content_sha(&bytes);
        let xml = std::str::from_utf8(&bytes).map_err(|e| format!("{path:?}: {e}"))?;
        let mdoc = Hcdf::from_xml_str(xml).map_err(|e| format!("parse {path:?}: {e}"))?;
        Ok((mdoc, Some(sha)))
    };
    keep_live_mismatch_lines(compose::verify_include_shas(doc, root_dir, &mut loader))
}

/// The in-memory counterpart of [`verify_keep_live_includes`]: the loader resolves each include key
/// against the archive's entry map, the same read the disk loader does off the extracted tree, without
/// the disk. `base` is `/` so the lexical resolve keys mirror the extracted-tree shape (a root include
/// `modules/<usha>/…` resolves directly); a NESTED module include key grows lexically and is recovered by
/// rejoining the LAST `modules/` segment, exactly like the disk loader's rfind fallback.
fn verify_keep_live_includes_mem(doc: &Hcdf, by_name: &BTreeMap<&str, &[u8]>) -> Vec<String> {
    let mut loader = |key: &str, _base: &Path| -> Result<(Hcdf, Option<String>), String> {
        let bytes = by_name
            .get(key.trim_start_matches('/'))
            .or_else(|| {
                key.rfind("modules/")
                    .and_then(|pos| by_name.get(&key[pos..]))
            })
            .copied()
            .ok_or_else(|| format!("no bundle entry for {key:?}"))?;
        let sha = compose::content_sha(bytes);
        let xml = std::str::from_utf8(bytes).map_err(|e| format!("{key:?}: {e}"))?;
        let mdoc = Hcdf::from_xml_str(xml).map_err(|e| format!("parse {key:?}: {e}"))?;
        Ok((mdoc, Some(sha)))
    };
    keep_live_mismatch_lines(compose::verify_include_shas(
        doc,
        Path::new("/"),
        &mut loader,
    ))
}

/// Render include-sha mismatches as keep-live verify issues: the shared wording for the disk and
/// in-memory verifiers.
fn keep_live_mismatch_lines(mismatches: Vec<compose::IncludeShaMismatch>) -> Vec<String> {
    mismatches
        .into_iter()
        .map(|m| match m.actual {
            Some(actual) => format!(
                "include {:?}: @sha does not match embedded module (pinned {}, module {})",
                m.uri, m.expected, actual
            ),
            None => format!(
                "include {:?}: @sha pinned but the embedded module is missing/unresolvable",
                m.uri
            ),
        })
        .collect()
}

/// Check every keep-live MODULE's mesh blobs: walk `root_dir/modules/<usha>/` for module `.hcdf`s and
/// `check_blob` each visual `<model>` / collision `<mesh>` uri against that module's sibling `assets/`,
/// so a tampered MODULE mesh (not just a root mesh) is caught. Missing/unparsable module files are
/// skipped here (the include `@sha` seam already reports an unresolvable module).
fn check_module_mesh_blobs(root_dir: &Path) -> Vec<String> {
    let mut issues = Vec::new();
    let modules_root = root_dir.join("modules");
    let Ok(dirs) = std::fs::read_dir(&modules_root) else {
        return issues;
    };
    for entry in dirs.filter_map(|e| e.ok()) {
        let mdir = entry.path();
        if !mdir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&mdir) else {
            continue;
        };
        for f in files.filter_map(|e| e.ok()) {
            let fpath = f.path();
            if fpath.extension().is_none_or(|x| x != "hcdf") {
                continue;
            }
            let Ok(xml) = std::fs::read_to_string(&fpath) else {
                continue;
            };
            let Ok(mdoc) = Hcdf::from_xml_str(&xml) else {
                continue;
            };
            let mtag = mdir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            issues.extend(check_doc_mesh_blobs(
                &mdoc,
                &format!("module {mtag}: "),
                &mut |uri, sha, site| check_blob(&mdir, uri, sha, site),
            ));
        }
    }
    issues
}

/// The in-memory counterpart of [`check_module_mesh_blobs`]: walk the archive's
/// `modules/<usha>/<stem>.hcdf` entries (archive order) and check each module doc's mesh blobs against
/// its sibling `assets/` entries. Missing/unparsable module entries are skipped here exactly like the
/// disk walk (the include `@sha` seam already reports an unresolvable module).
fn check_module_mesh_blobs_mem(
    entries: &[zip_store::ReadEntry],
    by_name: &BTreeMap<&str, &[u8]>,
) -> Vec<String> {
    let mut issues = Vec::new();
    for e in entries {
        // A module doc lives EXACTLY at `modules/<usha>/<stem>.hcdf`; deeper entries are its meshes
        // (the disk walk likewise reads only the `.hcdf` files directly under each module dir).
        let parts: Vec<&str> = e.name.split('/').collect();
        if parts.len() != 3 || parts[0] != "modules" || !e.name.ends_with(".hcdf") {
            continue;
        }
        let Ok(xml) = std::str::from_utf8(&e.data) else {
            continue;
        };
        let Ok(mdoc) = Hcdf::from_xml_str(xml) else {
            continue;
        };
        let mtag = parts[1];
        let mdir = format!("modules/{mtag}");
        issues.extend(check_doc_mesh_blobs(
            &mdoc,
            &format!("module {mtag}: "),
            &mut |uri, sha, site| check_blob_mem(by_name, &mdir, uri, sha, site),
        ));
    }
    issues
}

/// Check one mesh blob in the bundle: a KEPT remote uri is skipped (its integrity is the web server's
/// concern); otherwise the file must exist inside the bundle, hash to its `@sha`, and the filename's
/// short sha must match. Port of `bundle._check_blob`, including the `..`/absolute escape guard.
fn check_blob(root_dir: &Path, uri: &str, sha: Option<&str>, site: &str) -> Vec<String> {
    if is_remote(uri) {
        return Vec::new();
    }
    let root_abs = compose_abspath(root_dir);
    // Confine reads to the bundle: a hostile bundle could carry an absolute or `..`-escaping @uri.
    if Path::new(uri).is_absolute() {
        return vec![format!(
            "{site}: asset {uri:?} escapes the bundle (must be a document-relative path)"
        )];
    }
    let blob_path = normpath_abs(&root_abs.join(uri));
    if !blob_path.starts_with(&root_abs) {
        return vec![format!(
            "{site}: asset {uri:?} escapes the bundle (must be a document-relative path)"
        )];
    }
    if !blob_path.is_file() {
        return vec![format!("{site}: asset {uri:?} missing from bundle")];
    }
    let data = match std::fs::read(&blob_path) {
        Ok(d) => d,
        Err(e) => return vec![format!("{site}: asset {uri:?} unreadable: {e}")],
    };
    blob_sha_issues(uri, sha, site, &data)
}

/// The in-memory counterpart of [`check_blob`]: resolve `uri` under `prefix` (`""` for the root doc,
/// `modules/<usha>` for an embedded module) against the archive's entry map and hash the bytes in
/// place. Same confinement guard, same messages: no extraction, no re-read.
fn check_blob_mem(
    by_name: &BTreeMap<&str, &[u8]>,
    prefix: &str,
    uri: &str,
    sha: Option<&str>,
    site: &str,
) -> Vec<String> {
    if is_remote(uri) {
        return Vec::new();
    }
    if Path::new(uri).is_absolute() {
        return vec![format!(
            "{site}: asset {uri:?} escapes the bundle (must be a document-relative path)"
        )];
    }
    let Some(name) = lex_resolve(prefix, uri) else {
        return vec![format!(
            "{site}: asset {uri:?} escapes the bundle (must be a document-relative path)"
        )];
    };
    let Some(data) = by_name.get(name.as_str()) else {
        return vec![format!("{site}: asset {uri:?} missing from bundle")];
    };
    blob_sha_issues(uri, sha, site, data)
}

/// The sha-integrity checks for one blob's `data`: the `@sha` compare + the filename short-sha check.
/// Shared by the on-disk [`check_blob`] and the in-memory [`check_blob_mem`], so both verifiers report
/// the exact same messages.
fn blob_sha_issues(uri: &str, sha: Option<&str>, site: &str, data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let actual = compose::content_sha(data);
    match sha {
        None => out.push(format!(
            "{site}: asset {uri:?} has no @sha to check against (actual {actual})"
        )),
        Some(s) if s != actual => out.push(format!(
            "{site}: asset {uri:?} sha mismatch (@sha {s} != content {actual})"
        )),
        _ => {}
    }
    // The short sha baked into the filename must also match the content.
    if let Some(name) = Path::new(uri).file_name().and_then(|n| n.to_str()) {
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        if let Some((_, short)) = stem.rsplit_once('_') {
            let actual_hex = actual.split_once(':').map_or(actual.as_str(), |(_, h)| h);
            if !actual_hex.starts_with(short) {
                out.push(format!(
                    "{site}: filename sha {short:?} does not match content {actual}"
                ));
            }
        }
    }
    out
}

// ── small path helpers (lexical, no canonicalize, to match Python os.path semantics) ──

/// `os.path.abspath`: absolutize against the cwd (when relative) and fold `.`/`..` lexically.
fn compose_abspath(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(p)
    };
    normpath_abs(&abs)
}

/// Lexically normalize an ABSOLUTE path (collapse `.`/`..`), like `os.path.normpath`.
fn normpath_abs(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The in-memory analogue of [`safe_join`]'s zip-slip guard: reject an absolute entry name or one whose
/// `..` segments climb out of the bundle root: same messages, so the in-memory verifier reports exactly
/// what extraction would have rejected.
fn guard_entry_name(name: &str) -> Result<(), String> {
    if Path::new(name).is_absolute() {
        return Err(format!(
            "bundle entry {name:?} is an absolute path (rejected)"
        ));
    }
    if lex_resolve("", name).is_none() {
        return Err(format!(
            "bundle entry {name:?} escapes the bundle root (rejected)"
        ));
    }
    Ok(())
}

/// Lexically resolve `rel` under `prefix` (`""` = the bundle root) into an archive entry name: collapse
/// `.`/empty segments, pop on `..`. Returns `None` when the result climbs out of the bundle root or does
/// not stay under `prefix`: the in-memory analogue of the `normpath_abs` + `starts_with` confinement
/// the disk checks apply to the extracted tree.
fn lex_resolve(prefix: &str, rel: &str) -> Option<String> {
    let base: Vec<&str> = prefix.split('/').filter(|s| !s.is_empty()).collect();
    let mut segs = base.clone();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                // Popping above the bundle root can never resolve back inside it (the extraction dir's
                // name is unpredictable), so it is an escape outright.
                segs.pop()?;
            }
            other => segs.push(other),
        }
    }
    // The resolved name must still live under `prefix` (a `..` may dip out of a module dir and come
    // back, exactly as the lexical disk normpath allows; only the final location matters).
    if segs.len() < base.len() || segs[..base.len()] != base[..] {
        return None;
    }
    Some(segs.join("/"))
}

/// Join `name` under `root`, refusing an absolute name or one that escapes via `..` (zip-slip guard
/// on extract). Returns the confined destination path.
fn safe_join(root: &Path, name: &str) -> Result<PathBuf, String> {
    if Path::new(name).is_absolute() {
        return Err(format!(
            "bundle entry {name:?} is an absolute path (rejected)"
        ));
    }
    let root_abs = compose_abspath(root);
    let dest = normpath_abs(&root_abs.join(name));
    if !dest.starts_with(&root_abs) {
        return Err(format!(
            "bundle entry {name:?} escapes the bundle root (rejected)"
        ));
    }
    Ok(dest)
}

/// A fresh, unique staging directory under the system temp.
fn make_staging_dir() -> Result<PathBuf, String> {
    make_unique_dir("hcdf-bundle-stage-")
}

/// A fresh, unique extraction directory under the system temp (caller-owned).
fn make_extract_dir() -> Result<PathBuf, String> {
    make_unique_dir("hcdf-bundle-")
}

fn make_unique_dir(prefix: &str) -> Result<PathBuf, String> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("{prefix}{pid}-{nonce}"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir temp {dir:?}: {e}"))?;
    Ok(dir)
}

#[cfg(test)]
mod root_name_tests {
    use super::*;

    #[test]
    fn hostile_doc_name_yields_a_confined_single_component_filename() {
        // A Windows backslash traversal and a POSIX slash traversal both collapse to one safe filename
        // component: no separator survives, so `out_path.join(root_name)` can never climb out of the
        // bundle directory (the write-side twin of the read-side `safe_join` confinement).
        for hostile in ["..\\..\\evil", "../evil", "a/b\\c", "..", ""] {
            let doc = Hcdf {
                name: hostile.to_string(),
                ..Default::default()
            };
            let name = root_name(&doc, Path::new("/tmp/src.hcdf"));
            assert!(name.ends_with(".hcdf"), "{name:?}");
            assert!(!name.contains('/'), "no forward slash in {name:?}");
            assert!(!name.contains('\\'), "no backslash in {name:?}");
            assert_eq!(
                Path::new(&name).components().count(),
                1,
                "single path component: {name:?}"
            );
        }
    }

    #[test]
    fn safe_charset_doc_name_is_byte_stable() {
        // Every existing corpus name is already safe-charset, so sanitization is a no-op: zero golden
        // churn is expected, and asserted here.
        for safe in [
            "robot",
            "cogni-humanoid-mobile-base",
            "so_arm101",
            "my.robot-v2",
        ] {
            let doc = Hcdf {
                name: safe.to_string(),
                ..Default::default()
            };
            assert_eq!(
                root_name(&doc, Path::new("/tmp/x.hcdf")),
                format!("{safe}.hcdf")
            );
        }
    }
}
