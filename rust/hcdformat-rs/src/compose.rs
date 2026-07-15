//! `<include>` flattening for HCDF model composition: a Rust port of the Python
//! `hcdf_io/include.py` resolver, matched field-for-field.
//!
//! An `<include uri name pose>` imports a reusable sub-assembly. Flattening produces a single
//! self-contained document:
//!   * `@name` is a prefix (`prefix/...`) applied to every definition (comp/joint/group/state/
//!     color/transmission) and rewritten through every reference, so a module can be included many
//!     times without name collisions.
//!   * `@pose` ("x y z r p y", XYZ-extrinsic rpy, relative to the including document's world frame)
//!     rigidly places the sub-assembly: every root comp's own geometry/frames and the origins of
//!     joints leaving a root are pre-multiplied by the offset (the internal tree is relative, so this
//!     moves the whole assembly).
//!
//! Nested includes resolve recursively (relative to each file's own directory); include cycles error.
//!
//! ## WASM safety
//! The core, [`flatten_with`], takes a *loader closure* instead of touching the filesystem, so it
//! compiles and runs on `wasm32-unknown-unknown`. The native conveniences [`flatten`] and
//! [`flatten_path`] (which read files via [`std::fs`]) are gated behind `cfg(not(target_arch =
//! "wasm32"))`.
//!
//! ## Cycle handling
//! Mirroring `include.py` (which `raise`s on a cycle), the core returns `Result<Vec<String>, String>`
//! and aborts with an `Err` describing the cycle path; an *unresolvable* uri (loader error) is instead
//! left in place as a kept `<include>` with a note, exactly like the Python keeps a not-found file.

use crate::comments::{CapturedComment, CommentAnchor};
use crate::model::connectivity_xml;
use crate::model::{Hcdf, Pose, VisualAppearance};
use crate::resource::{visit_resources, ResourceClass, ResourceRewrite};
use crate::resource_path::{normalize_filesystem_path as normalize, reroot_filesystem_asset};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(crate) mod pose_math;
pub use pose_math::{matrix_to_pose, matrix_to_rpy, mul44, pose_to_matrix, rpy_to_matrix, Mat4};

/// The definition-name separator used to build a prefix (`"<name>/"`), matching `include.py::_SEP`.
const SEP: &str = "/";

/// A content address for raw bytes: `"sha256:<hex>"`. The Rust equivalent of `hcdf.assets.content_sha`:
/// the SAME bytes always produce the SAME string in both languages (lowercase hex sha-256). Pure
/// `sha2`, so it is wasm-clean; used both to verify a pinned include and to [`stamp_include_shas`].
pub fn content_sha(data: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(data))
}

/// The CANONICAL content sha of a module document: [`content_sha`] over its canonical serialization
/// ([`Hcdf::to_xml_string`] bytes). Deterministic and wasm-clean (the serializer runs on both targets),
/// and it TRANSITIVELY covers the module's mesh `@sha`s: those are serialized as `model.sha`/`mesh.sha`
/// attributes, so a mesh change flows into this value.
///
/// This is the ONE definition of a module's content identity shared by the keep-live pack `@sha` stamp,
/// the native `BundleVerify`, and dendrite's in-app load check. Its value legitimately DIFFERS between
/// contexts (an in-store module carries authored `/mem/` mesh uris; a packed module carries
/// content-addressed `assets/<sha>` uris), which is fine, because every consumer RE-STAMPS whenever the
/// content or context changes and the verify seam ([`verify_include_shas`]) DUAL-ACCEPTS this canonical
/// sha OR a loader's raw-bytes sha.
pub fn content_sha_of_module(doc: &Hcdf) -> Result<String, String> {
    Ok(content_sha(
        doc.to_xml_string().map_err(|e| e.to_string())?.as_bytes(),
    ))
}

/// One `<include>` whose pinned `@sha` does NOT match the module it currently resolves to: the unit
/// [`verify_include_shas`] reports. `actual = None` means the module was MISSING / unresolvable (the
/// loader errored); `actual = Some(..)` is what the resolved module actually hashes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeShaMismatch {
    /// The offending `<include>`'s `@name` (the instance prefix), if any.
    pub include_name: Option<String>,
    /// The `<include>`'s `@uri` as written.
    pub uri: String,
    /// The pinned `@sha` that failed to match.
    pub expected: String,
    /// What the resolved module actually hashes to (canonical, else the loader's raw sha), or `None` when
    /// the module is missing / unresolvable.
    pub actual: Option<String>,
}

/// The maximum `<include>` nesting depth [`verify_include_shas`] descends. A real module tree is only a
/// few deep; this bounds the recursion so a maliciously/corruptly CYCLIC keep-live bundle (which the packer
/// never emits, but `verify` may be handed from an UNTRUSTED source such as a fetched URL) cannot
/// stack-overflow the verifier. Reaching it is reported as a mismatch rather than silently truncated.
const MAX_INCLUDE_DEPTH: usize = 64;

/// Per-call memo entry for one resolved module key ([`verify_include_shas`]): the parsed module, the
/// loader's raw-bytes sha, and the canonical sha, computed LAZILY (only once some pin fails the cheap
/// raw compare) and then cached, so N includes of one module cost at most ONE canonical serialization
/// and the fs-stamped common case (raw always matches) never pays for a serialize at all.
struct VerifiedModule {
    doc: Hcdf,
    raw: Option<String>,
    canon: Option<String>,
}

impl VerifiedModule {
    /// The dual-accept "matches" rule: `pinned` equals the loader's raw-bytes sha OR (computed lazily on
    /// the first raw miss, then cached) the module's [`content_sha_of_module`].
    fn pin_matches(&mut self, pinned: &str) -> bool {
        if self.raw.as_deref().filter(|s| !s.is_empty()) == Some(pinned) {
            return true;
        }
        if self.canon.is_none() {
            self.canon = content_sha_of_module(&self.doc).ok();
        }
        self.canon.as_deref() == Some(pinned)
    }

    /// The `actual` value a mismatch reports: the canonical sha when computable (always attempted by
    /// [`Self::pin_matches`] before a mismatch is declared), else the loader's raw sha.
    fn actual(&self) -> Option<String> {
        self.canon.clone().or_else(|| {
            self.raw
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
    }
}

/// Verify every pinned `<include> @sha` in `doc` (recursively) against the module it resolves to, using
/// `load`, the SAME loader type [`flatten_with`] takes, so a caller passes its own live resolver and
/// verification matches composition exactly. Returns one [`IncludeShaMismatch`] per include whose
/// NON-EMPTY `@sha` does not match; an unpinned (`None`/`""`) include is skipped.
///
/// DUAL-ACCEPT: a pin is OK iff it equals EITHER the loader's raw-bytes `source_sha` (what "Pin includes"
/// / `fs_loader` / an extracted keep-live module file produce) OR [`content_sha_of_module`] of the parsed
/// module (what the wasm store / the auto-stamp / the keep-live pack embedded bytes produce). This is the
/// single "matches" definition shared by native `BundleVerify` and dendrite's in-app check, so Pin,
/// auto-stamp, and keep-live all read clean while a genuine drift (a changed/tampered/stale module) is
/// caught. Recurses into each resolved module (relative to its own dir) with a cycle guard.
///
/// PER-CALL MEMO: each distinct module (resolved key) is loaded and descended ONCE per call. A repeat
/// include still checks ITS OWN pin (two sites may pin different values) against the memoized shas, but
/// the shared subtree is not re-walked; a nested mismatch inside a module included N times is reported
/// once, not once per include path. The canonical sha is computed lazily ([`VerifiedModule`]), so a doc
/// whose pins all match their raw shas never serializes a module at all.
pub fn verify_include_shas<L>(doc: &Hcdf, base: &Path, load: &mut L) -> Vec<IncludeShaMismatch>
where
    L: FnMut(&str, &Path) -> Result<(Hcdf, Option<String>), String>,
{
    let mut out = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut memo: HashMap<String, Option<VerifiedModule>> = HashMap::new();
    verify_include_shas_rec(doc, base, load, &mut out, &mut stack, &mut memo);
    out
}

/// Recursive worker for [`verify_include_shas`]; `stack` holds the resolved keys currently being verified
/// (cycle guard, mirroring [`resolve`]'s `stack`); `memo` holds the modules already verified this call
/// (`None` = the loader errored, so the module is missing at every site that resolves to that key).
fn verify_include_shas_rec<L>(
    doc: &Hcdf,
    base: &Path,
    load: &mut L,
    out: &mut Vec<IncludeShaMismatch>,
    stack: &mut Vec<String>,
    memo: &mut HashMap<String, Option<VerifiedModule>>,
) where
    L: FnMut(&str, &Path) -> Result<(Hcdf, Option<String>), String>,
{
    for inc in &doc.include {
        let Some(uri) = inc.uri.as_deref() else {
            continue;
        };
        let key = resolve_key(uri, base);
        let key_str = key.to_string_lossy().into_owned();
        let pinned = inc.sha.as_deref().filter(|s| !s.is_empty());

        // Repeat include of an already-verified module: check THIS site's pin against the memoized
        // shas (cheap string compares; the lazy canonical fills in at most once per module) and move
        // on: no re-load, no re-descent into the shared subtree.
        if let Some(cached) = memo.get_mut(&key_str) {
            if let Some(pinned) = pinned {
                match cached {
                    None => out.push(IncludeShaMismatch {
                        include_name: inc.name.clone(),
                        uri: uri.to_string(),
                        expected: pinned.to_string(),
                        actual: None,
                    }),
                    Some(module) => {
                        if !module.pin_matches(pinned) {
                            out.push(IncludeShaMismatch {
                                include_name: inc.name.clone(),
                                uri: uri.to_string(),
                                expected: pinned.to_string(),
                                actual: module.actual(),
                            });
                        }
                    }
                }
            }
            continue;
        }

        let sub_base = key.parent().map(Path::to_path_buf).unwrap_or_default();
        match load(&key_str, base) {
            Err(_) => {
                // Unresolvable module: report it only if the include PINS a sha (an unpinned missing
                // include is a resolution concern for flatten, not a sha-integrity mismatch). Memoized
                // so N sites pinning one missing module report N times off a single loader call.
                if let Some(pinned) = pinned {
                    out.push(IncludeShaMismatch {
                        include_name: inc.name.clone(),
                        uri: uri.to_string(),
                        expected: pinned.to_string(),
                        actual: None,
                    });
                }
                memo.insert(key_str, None);
            }
            Ok((mdoc, source_sha)) => {
                let mut module = VerifiedModule {
                    doc: mdoc,
                    raw: source_sha,
                    canon: None,
                };
                if let Some(pinned) = pinned {
                    if !module.pin_matches(pinned) {
                        out.push(IncludeShaMismatch {
                            include_name: inc.name.clone(),
                            uri: uri.to_string(),
                            expected: pinned.to_string(),
                            actual: module.actual(),
                        });
                    }
                }
                // Recurse into the module (relative to its own dir) with a cycle guard PLUS a hard depth
                // cap: the lexical `resolve_key` can GROW under a rfind-style keep-live loader (it finds the
                // real file however deep the joined key is), so a crafted A->B->A cycle would evade the
                // key-based guard and recurse unbounded, the depth cap (`stack.len()` is the current depth)
                // is the backstop that keeps `verify` from stack-overflowing on an untrusted/corrupt bundle.
                // Only a COMPLETED descent is memoized (a depth-capped or stack-blocked one is not), so a
                // memo hit always means "this subtree was fully verified once this call".
                if stack.len() >= MAX_INCLUDE_DEPTH {
                    out.push(IncludeShaMismatch {
                        include_name: inc.name.clone(),
                        uri: uri.to_string(),
                        expected: pinned.map(str::to_string).unwrap_or_default(),
                        actual: Some(format!(
                            "include nesting exceeds {MAX_INCLUDE_DEPTH} levels (possible module cycle); not verified deeper"
                        )),
                    });
                } else if !stack.iter().any(|k| k == &key_str) {
                    stack.push(key_str.clone());
                    verify_include_shas_rec(&module.doc, &sub_base, load, out, stack, memo);
                    stack.pop();
                    memo.insert(key_str, Some(module));
                }
            }
        }
    }
}

/// A readable short form of a `"sha256:<hex>"` (or bare-hex) sha for notes: the first 12 hex chars,
/// matching `include.py::_short_sha`. Used only in mismatch notes; never compared.
fn short_sha(sha: &str) -> &str {
    let hexpart = sha.split_once(':').map_or(sha, |(_, hex)| hex);
    hexpart.get(..12).unwrap_or(hexpart)
}

/// Resolve every `<include>` in `doc` into a single self-contained document, using `load` to fetch
/// each referenced sub-document. WASM-safe: no filesystem access happens here; `load` owns I/O.
///
/// `base_dir` is the directory `doc`'s own relative uris resolve against. `load(resolved, base)` is
/// called with the resolved key (an absolute path string when the loader joins against `base`) and the
/// base directory; it returns `(parsed sub-Hcdf, source_sha)` or an `Err` (treated as "unresolvable":
/// the include is kept in place with a note, like a not-found file in `include.py`). `source_sha` is
/// the `"sha256:<hex>"` content hash of the RAW bytes the loader read, or `None` when the loader
/// cannot supply it (in-memory loaders, etc.); `None` simply skips verification.
///
/// ## `@sha` verification (parity with `include.py`)
/// When an `<include>` pins an optional `@sha` AND the loader returned `Some(source_sha)` that differs,
/// a NOTE `"include <uri>: sha mismatch (pinned .., actual ..)"` is pushed, NEVER an `Err` (a module
/// legitimately changes as it is edited via live-link; only a true CYCLE is an `Err`). A no-`@sha`
/// include or a `None` loader-sha is not verified and adds no note.
///
/// Returns the accumulated notes on success, or `Err(msg)` on a true include cycle (matching
/// `include.py`, which raises on a cycle).
///
/// The loader is responsible for cycle-relevant identity: `flatten_with` tracks the set of resolved
/// keys (the first element of the `(key, base)` pair it would pass to `load`) on the active resolve
/// stack and refuses to re-enter one.
///
/// Each distinct resolved key is loaded ONCE per flatten call (memoized); a module included N times is
/// CLONED from the first load and still independently recursed/prefixed/offset, so composition and notes
/// are unchanged; only the loader traffic dedupes. A loader must therefore treat a key's content as
/// stable for the duration of one call (the fs/store loaders already do).
pub fn flatten_with<L>(doc: &mut Hcdf, base_dir: &Path, load: &mut L) -> Result<Vec<String>, String>
where
    L: FnMut(&str, &Path) -> Result<(Hcdf, Option<String>), String>,
{
    // Empty initial stack mirrors `include.py::flatten` when handed an `Hcdf` (in-memory): the root
    // doc has no path identity to cycle against. The native `flatten_path` seeds the root path instead
    // (mirroring `flatten(path)` with `_stack=(top,)`).
    flatten_with_stack(doc, base_dir, load, Vec::new())
}

/// Core flattening with an explicit initial resolve `stack` (the chain of resolved keys already being
/// resolved). Used by [`flatten_with`] (empty stack) and the native [`flatten_path`] (root-seeded).
fn flatten_with_stack<L>(
    doc: &mut Hcdf,
    base_dir: &Path,
    load: &mut L,
    initial_stack: Vec<String>,
) -> Result<Vec<String>, String>
where
    L: FnMut(&str, &Path) -> Result<(Hcdf, Option<String>), String>,
{
    let mut notes = Vec::new();
    let mut stack = initial_stack;
    let mut memo: HashMap<String, (Hcdf, Option<String>)> = HashMap::new();
    let mut working = doc.clone();
    let mut origin_adjustments = ComponentOriginAdjustments::new();
    resolve(
        &mut working,
        base_dir,
        load,
        &mut notes,
        &mut stack,
        &mut memo,
        &mut origin_adjustments,
    )?;
    *doc = working;
    Ok(notes)
}

/// Recursive worker for [`flatten_with`]. `stack` holds the resolved keys currently being resolved
/// (for cycle detection); `notes` accumulates human-readable resolution notes; `memo` caches each
/// SUCCESSFUL load by resolved key (the pristine as-loaded module + its source sha) so a module
/// included N times is read+parsed once and cloned thereafter.
fn resolve<L>(
    doc: &mut Hcdf,
    base: &Path,
    load: &mut L,
    notes: &mut Vec<String>,
    stack: &mut Vec<String>,
    memo: &mut HashMap<String, (Hcdf, Option<String>)>,
    origin_adjustments: &mut ComponentOriginAdjustments,
) -> Result<(), String>
where
    L: FnMut(&str, &Path) -> Result<(Hcdf, Option<String>), String>,
{
    let parent_connectivity = ParentConnectivityBoundary::capture(doc);
    reject_nonzero_connectivity_occurrences(doc, parent_connectivity)?;
    validate_include_siblings(&doc.include)?;
    let pending = std::mem::take(&mut doc.include);
    for inc in pending {
        let uri = inc.uri.clone().ok_or_else(|| {
            format!(
                "include {:?} is missing required @uri",
                inc.name.as_deref().unwrap_or("")
            )
        })?;
        // Resolve the key: an absolute uri stands alone; a relative one joins `base`. The key is the
        // identity used for cycle detection and is the value passed to `load`.
        let key = resolve_key(&uri, base);
        let key_str = key.to_string_lossy().into_owned();
        let sub_base = key.parent().map(Path::to_path_buf).unwrap_or_default();

        // A repeat include CLONES the memoized (pristine, as-loaded) module; every instance is still
        // independently recursed/prefixed/offset below, so only the load itself dedupes. Loader errors
        // are NOT memoized: the unresolved path is rare and its note carries the per-call error.
        let (mut sub, source_sha) = if let Some((cached, sha)) = memo.get(&key_str) {
            (cached.clone(), sha.clone())
        } else {
            match load(&key_str, base) {
                Ok(loaded) => {
                    memo.insert(key_str.clone(), loaded.clone());
                    loaded
                }
                Err(why) => {
                    // Unresolvable (e.g. file not found): keep the include in place with a note, exactly
                    // like `include.py` keeps a not-found include (existence check precedes the cycle check
                    // there, so a not-found-and-cycling include is kept, not raised; same here).
                    doc.include.push(inc);
                    notes.push(format!("include {uri:?}: {why}; left unresolved"));
                    continue;
                }
            }
        };
        if stack.iter().any(|k| k == &key_str) {
            let mut chain = stack.clone();
            chain.push(key_str.clone());
            return Err(format!("include cycle detected: {}", chain.join(" -> ")));
        }

        // Optional @sha verification, parity with `include.py::_resolve`. ONLY engages when the include
        // pins a NON-EMPTY sha AND the loader supplied a NON-EMPTY source sha; a mismatch is a NOTE,
        // never an `Err` (the module legitimately changes as it is edited via live-link; only a CYCLE
        // aborts). The hash is over the module's RAW file bytes ("sha256:<hex>"). A no-@sha include, an
        // EMPTY `sha=""` pin (serde keeps it as `Some("")`, but Python guards on truthiness so it is not
        // verified), or a `None` loader-sha adds no note.
        let pinned = inc.sha.as_deref().filter(|s| !s.is_empty());
        let actual = source_sha.as_deref().filter(|s| !s.is_empty());
        if let (Some(pinned), Some(actual)) = (pinned, actual) {
            if pinned != actual {
                notes.push(format!(
                    "include {uri:?}: sha mismatch (pinned {}, actual {})",
                    short_sha(pinned),
                    short_sha(actual)
                ));
            }
        }

        // Nested first: resolve the sub-document's own includes relative to ITS directory.
        stack.push(key_str.clone());
        let mut sub_origin_adjustments = ComponentOriginAdjustments::new();
        let res = resolve(
            &mut sub,
            &sub_base,
            load,
            notes,
            stack,
            memo,
            &mut sub_origin_adjustments,
        );
        stack.pop();
        res?;

        // The sub's opaque asset URIs are relative to ITS OWN dir; anchor them to that dir so a
        // downstream resolve finds them after merging into `doc`.
        reroot_asset_uris(&mut sub, &sub_base);

        if let (Some(d), Some(s)) = (doc.body_frame.as_ref(), sub.body_frame.as_ref()) {
            if d != s {
                notes.push(format!(
                    "include {uri:?}: body-frame {s} differs from including document {d} \
                     (not converted; convention should match)"
                ));
            }
        }

        if let Some(name) = inc.name.as_deref() {
            if !name.is_empty() {
                prefix(&mut sub, name);
                prefix_origin_adjustments(&mut sub_origin_adjustments, name);
            }
        }
        if let Some(pose) = inc.pose.as_deref() {
            if !pose.is_empty() {
                record_component_origin_adjustments(&sub, pose, &mut sub_origin_adjustments);
                offset(&mut sub, pose);
            }
        }
        apply_parent_component_origin_adjustments(
            doc,
            parent_connectivity,
            &sub_origin_adjustments,
        );
        collapse_parent_instance_refs(doc, parent_connectivity, &sub)?;
        merge(doc, sub);
        origin_adjustments.extend(sub_origin_adjustments);

        let mut note = format!("flattened include {uri:?}");
        if let Some(name) = inc.name.as_deref() {
            if !name.is_empty() {
                note.push_str(&format!(" as {name:?}"));
            }
        }
        if let Some(pose) = inc.pose.as_deref() {
            if !pose.is_empty() {
                note.push_str(&format!(" at pose {pose:?}"));
            }
        }
        notes.push(note);
    }
    Ok(())
}

/// Compute the resolved-key path for an include uri against `base`, mirroring
/// `os.path.normpath(os.path.abspath(os.path.join(base, uri)))` for the relative case. We deliberately
/// keep this lexical (no `canonicalize`) so it works without the file existing yet and is identical on
/// wasm; the loader is free to canonicalize internally.
fn resolve_key(uri: &str, base: &Path) -> PathBuf {
    let p = Path::new(uri);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    normalize(&joined)
}

/// Reject sibling include names that cannot be flattened into distinct definition prefixes.
/// Include instance occurrence remains part of canonical identity, but flattened definition names
/// have no occurrence field. Rejecting ambiguous siblings prevents two occurrences from aliasing.
fn validate_include_siblings(includes: &[crate::model::Include]) -> Result<(), String> {
    let mut names = HashSet::new();
    for include in includes {
        if include.uri.is_none() {
            return Err(format!(
                "include {:?} is missing required @uri",
                include.name.as_deref().unwrap_or("")
            ));
        }
        let Some(name) = include.name.as_deref().filter(|name| !name.is_empty()) else {
            continue;
        };
        if !names.insert(name) {
            return Err(format!(
                "duplicate sibling include name {name:?} cannot be flattened without aliasing"
            ));
        }
    }
    Ok(())
}

fn root_element_count(doc: &Hcdf, name: &str) -> usize {
    match name {
        "comp" => doc.comp.len(),
        "joint" => doc.joint.len(),
        "group" => doc.group.len(),
        "state" => doc.state.len(),
        "self-collision-disable" => usize::from(doc.self_collision_disable.is_some()),
        "harness" => doc.harness.len(),
        "cable" => doc.cable.len(),
        "plumbing" => doc.plumbing.len(),
        "umbilical" => doc.umbilical.len(),
        "binding" => doc.binding.len(),
        "mate" => doc.mate.len(),
        "link" => doc.link.len(),
        "bus" => doc.bus.len(),
        "chain" => doc.chain.len(),
        "star" => doc.star.len(),
        "ring" => doc.ring.len(),
        "mesh" => doc.mesh.len(),
        "tree" => doc.tree.len(),
        "transmission" => doc.transmission.len(),
        "color" => doc.color.len(),
        "include" => doc.include.len(),
        "extension" => doc.extension.len(),
        _ => 0,
    }
}

fn first_merged_root_element(doc: &Hcdf) -> Option<&'static str> {
    [
        ("comp", doc.comp.len()),
        ("joint", doc.joint.len()),
        ("group", doc.group.len()),
        ("state", doc.state.len()),
        (
            "self-collision-disable",
            usize::from(doc.self_collision_disable.is_some()),
        ),
        ("harness", doc.harness.len()),
        ("cable", doc.cable.len()),
        ("plumbing", doc.plumbing.len()),
        ("umbilical", doc.umbilical.len()),
        ("binding", doc.binding.len()),
        ("mate", doc.mate.len()),
        ("link", doc.link.len()),
        ("bus", doc.bus.len()),
        ("chain", doc.chain.len()),
        ("star", doc.star.len()),
        ("ring", doc.ring.len()),
        ("mesh", doc.mesh.len()),
        ("tree", doc.tree.len()),
        ("transmission", doc.transmission.len()),
        ("color", doc.color.len()),
        ("extension", doc.extension.len()),
    ]
    .into_iter()
    .find_map(|(name, count)| (count > 0).then_some(name))
}

fn merge_doc_comments(doc: &mut Hcdf, sub: &mut Hcdf) {
    let first = first_merged_root_element(sub).map(|name| CommentAnchor::BeforeElement {
        name: name.to_owned(),
        occurrence: root_element_count(doc, name),
    });
    for text in std::mem::take(&mut sub.comments.head) {
        doc.comments.body.push(CapturedComment {
            path: Vec::new(),
            anchor: first.clone().unwrap_or(CommentAnchor::AtEnd),
            text,
        });
    }
    for mut comment in std::mem::take(&mut sub.comments.body) {
        if let Some((name, occurrence)) = comment.path.first_mut() {
            *occurrence += root_element_count(doc, name);
        } else if let CommentAnchor::BeforeElement { name, occurrence } = &mut comment.anchor {
            *occurrence += root_element_count(doc, name);
        }
        doc.comments.body.push(comment);
    }
    for text in std::mem::take(&mut sub.comments.tail) {
        doc.comments.body.push(CapturedComment {
            path: Vec::new(),
            anchor: CommentAnchor::AtEnd,
            text,
        });
    }
}

/// Append the sub's definition sequences into `doc`. Extends `include.py::_merge` with the composition-
/// safety carry of `self_collision_disable`: the Python resolver merged only the Vec-typed sequences
/// and silently dropped an included module's Option-typed collision-pair block, so a subassembly's
/// self-collision exclusions vanished post-flatten. Here the sub's pairs fold into the parent block
/// (created on first contribution, never fabricated empty), with `prefix()` having already rewritten each
/// pair's `@comp1`/`@comp2` to the instance-qualified comp names.
fn merge(doc: &mut Hcdf, mut sub: Hcdf) {
    merge_doc_comments(doc, &mut sub);
    doc.comp.extend(sub.comp);
    doc.joint.extend(sub.joint);
    doc.group.extend(sub.group);
    doc.state.extend(sub.state);
    doc.color.extend(sub.color);
    doc.harness.extend(sub.harness);
    doc.cable.extend(sub.cable);
    doc.plumbing.extend(sub.plumbing);
    doc.umbilical.extend(sub.umbilical);
    doc.binding.extend(sub.binding);
    doc.mate.extend(sub.mate);
    doc.link.extend(sub.link);
    doc.bus.extend(sub.bus);
    doc.chain.extend(sub.chain);
    doc.star.extend(sub.star);
    doc.ring.extend(sub.ring);
    doc.mesh.extend(sub.mesh);
    doc.tree.extend(sub.tree);
    doc.transmission.extend(sub.transmission);
    doc.extension.extend(sub.extension);
    if let Some(sub_scd) = sub.self_collision_disable {
        doc.self_collision_disable
            .get_or_insert_with(Default::default)
            .pair
            .extend(sub_scd.pair);
    }
}

// ── asset-URI rerooting ───────────────────────────────────────────────────────

/// Anchor a document-relative mesh URI to `sub_base` using native path semantics. Syntactically
/// schemed references, absolute paths, and non-path suffix bytes remain unchanged.
fn reroot_uri(uri: &str, sub_base: &Path) -> String {
    reroot_filesystem_asset(sub_base, uri)
}

/// Rewrite every opaque asset URI in `sub` relative to `sub_base`.
fn reroot_asset_uris(sub: &mut Hcdf, sub_base: &Path) {
    reroot_asset_uris_with(sub, |uri| reroot_uri(uri, sub_base));
}

fn reroot_asset_uris_with(sub: &mut Hcdf, mut reroot: impl FnMut(&str) -> String) {
    let result: Result<(), std::convert::Infallible> = visit_resources(sub, |site, reference| {
        if site.class() != ResourceClass::OpaqueAsset || reference.uri.is_empty() {
            return Ok(ResourceRewrite::Keep);
        }
        Ok(ResourceRewrite::Replace {
            uri: reroot(reference.uri),
            sha: reference.sha.map(str::to_owned),
        })
    });
    match result {
        Ok(()) => {}
        Err(never) => match never {},
    }
}

// ── name prefixing + reference rewriting ──────────────────────────────────────

/// Prefix every definition name with `"<prefix>/"` and rewrite every reference against the original
/// pre-prefix name sets. In addition to kinematic references, connectivity rewriting covers participant
/// endpoint component refs, hop owner component and function refs, topology participant and hop network
/// refs, and physical owner refs. Component-local port, channel, function, connector, participant, hop,
/// and leg names remain local to their owning definition.
///   * self-collision `<pair>` `@comp1`/`@comp2` (bare comp refs), so `merge()`'s carried pairs still
///     name their instance-qualified comps.
///   * a BARE transmission motor ref resolves against the module's OWN comps and is rewritten to the
///     slash-path form (`drive1/dm8009p_case/foc_motor`) when unambiguous, so N instances do not collapse
///     onto one bare name; an intra-module-ambiguous bare name is left bare for the doc-level
///     `E_TRANS_MOTOR_AMBIGUOUS` to catch.
fn prefix(sub: &mut Hcdf, prefix: &str) {
    let p = format!("{prefix}{SEP}");

    // Capture the ORIGINAL name sets BEFORE renaming (references are decided against these).
    let comps: HashSet<String> = sub.comp.iter().map(|c| c.name.clone()).collect();
    let joints: HashSet<String> = sub.joint.iter().filter_map(|j| j.name.clone()).collect();
    let groups: HashSet<String> = sub.group.iter().filter_map(|g| g.name.clone()).collect();
    let colors: HashSet<String> = sub.color.iter().filter_map(|c| c.name.clone()).collect();
    let assemblies: HashSet<String> = sub
        .harness
        .iter()
        .chain(&sub.cable)
        .chain(&sub.plumbing)
        .chain(&sub.umbilical)
        .map(|assembly| assembly.name.clone())
        .collect();
    let networks: HashSet<String> = sub
        .link
        .iter()
        .map(|value| value.name.clone())
        .chain(sub.bus.iter().map(|value| value.name.clone()))
        .chain(sub.chain.iter().map(|value| value.name.clone()))
        .chain(sub.star.iter().map(|value| value.name.clone()))
        .chain(sub.ring.iter().map(|value| value.name.clone()))
        .chain(sub.mesh.iter().map(|value| value.name.clone()))
        .chain(sub.tree.iter().map(|value| value.name.clone()))
        .collect();
    // Transmission motor refs: map each motor NAME to the ORIGINAL comp(s) that carry it (a comp counted
    // ONCE even with two same-named motors, mirroring the validator's comps-per-name tally). A bare
    // transmission ref is rewritten iff its name is owned by exactly one module comp; a name on MORE THAN
    // ONE is left bare on purpose (the module is internally ambiguous: the doc-level
    // E_TRANS_MOTOR_AMBIGUOUS is the backstop).
    let mut motor_owner: HashMap<String, Vec<String>> = HashMap::new();
    for c in &sub.comp {
        for m in &c.motor {
            if let Some(mn) = m.name.as_deref() {
                let owners = motor_owner.entry(mn.to_string()).or_default();
                if !owners.contains(&c.name) {
                    owners.push(c.name.clone());
                }
            }
        }
    }

    let pc = |n: &str| -> String {
        if comps.contains(n) {
            format!("{p}{n}")
        } else {
            n.to_string()
        }
    };
    let pj = |n: &str| -> String {
        if joints.contains(n) {
            format!("{p}{n}")
        } else {
            n.to_string()
        }
    };
    let pg = |n: &str| -> String {
        if groups.contains(n) {
            format!("{p}{n}")
        } else {
            n.to_string()
        }
    };
    // rename definitions
    for c in sub.comp.iter_mut() {
        c.name = format!("{p}{}", c.name);
    }
    rename_opt(sub.joint.iter_mut().map(|j| &mut j.name), &p);
    rename_opt(sub.group.iter_mut().map(|g| &mut g.name), &p);
    rename_opt(sub.state.iter_mut().map(|s| &mut s.name), &p);
    rename_opt(sub.transmission.iter_mut().map(|t| &mut t.name), &p);
    rename_opt(sub.color.iter_mut().map(|c| &mut c.name), &p);

    // rewrite references (decided against the ORIGINAL name sets captured above)
    for j in sub.joint.iter_mut() {
        if let Some(comp) = j.parent.as_mut().and_then(|e| e.comp.as_mut()) {
            *comp = pc(comp);
        }
        if let Some(comp) = j.child.as_mut().and_then(|e| e.comp.as_mut()) {
            *comp = pc(comp);
        }
        if let Some(joint) = j.mimic.as_mut().and_then(|m| m.joint.as_mut()) {
            *joint = pj(joint);
        }
        if let Some(l) = j.loop_.as_mut() {
            if let Some(pred) = l.predecessor.as_mut() {
                *pred = pc(pred);
            }
            if let Some(succ) = l.successor.as_mut() {
                *succ = pc(succ);
            }
        }
    }
    for g in sub.group.iter_mut() {
        for jr in g.joint.iter_mut() {
            if let Some(r) = jr.ref_.as_mut() {
                *r = pj(r);
            }
        }
        for gr in g.group.iter_mut() {
            if let Some(r) = gr.ref_.as_mut() {
                *r = pg(r);
            }
        }
        if let Some(tip) = g.tip_comp.as_mut() {
            *tip = pc(tip); // tip_frame is a comp-local frame name -> unchanged
        }
    }
    for s in sub.state.iter_mut() {
        for jp in s.joint_position.iter_mut() {
            if let Some(joint) = jp.joint.as_mut() {
                *joint = pj(joint);
            }
        }
    }
    for c in sub.comp.iter_mut() {
        let siblings: HashSet<String> = c.frame.iter().map(|f| f.name.clone()).collect();
        for f in c.frame.iter_mut() {
            if let Some(r) = f.relative_to.clone() {
                if siblings.contains(&r) {
                    continue; // sibling frame: comp-local, stays
                }
                if comps.contains(&r) || joints.contains(&r) {
                    f.relative_to = Some(format!("{p}{r}")); // comp / joint anchor -> prefixed
                }
            }
        }
        for v in c.visual.iter_mut() {
            if let VisualAppearance::Primitive {
                color: Some(col), ..
            } = &mut v.appearance
            {
                if col.rgba.is_none() {
                    if let Some(name) = col.name.clone() {
                        if colors.contains(&name) {
                            col.name = Some(format!("{p}{name}"));
                        }
                    }
                }
            }
        }
    }
    for t in sub.transmission.iter_mut() {
        for jr in t.joint.iter_mut().filter_map(|e| e.ref_.as_mut()) {
            if joints.contains(jr.as_str()) {
                *jr = format!("{p}{jr}");
            }
        }
        for mr in t.motor.iter_mut().filter_map(|e| e.ref_.as_mut()) {
            // CLEAN-1.0 slash-path for a transmission motor ref: a "comp/motor" ref prefixes on the comp head (rsplit; the head
            // may itself carry '/' under nested includes); a BARE "motor" ref rewrites to the slash-path
            // form when it is owned by exactly ONE module comp, and is left bare (dangling or intra-module
            // ambiguous) otherwise, for the doc-level validator to adjudicate.
            let rewritten = match mr.rsplit_once('/') {
                Some((comp, _motor)) if comps.contains(comp) => Some(format!("{p}{mr}")),
                Some(_) => None,
                None => match motor_owner.get(mr.as_str()) {
                    Some(owners) if owners.len() == 1 => Some(format!("{p}{}{SEP}{mr}", owners[0])),
                    _ => None,
                },
            };
            if let Some(v) = rewritten {
                *mr = v;
            }
        }
    }
    prefix_connectivity(sub, &p, &comps, &assemblies, &networks);
    // Self-collision pairs: @comp1/@comp2 are bare comp refs, prefixed like any other comp reference
    // so merge()'s carried pairs still name their instance-qualified comps.
    if let Some(scd) = sub.self_collision_disable.as_mut() {
        for pair in scd.pair.iter_mut() {
            if let Some(c) = pair.comp1.as_mut() {
                *c = pc(c);
            }
            if let Some(c) = pair.comp2.as_mut() {
                *c = pc(c);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ConnectivityTargetKind {
    Component,
    Assembly,
    Network,
}

type ComponentOriginAdjustments = HashMap<String, Mat4>;

#[derive(Clone, Copy)]
struct ParentConnectivityBoundary {
    component: usize,
    harness: usize,
    cable: usize,
    plumbing: usize,
    umbilical: usize,
    binding: usize,
    mate: usize,
    link: usize,
    bus: usize,
    chain: usize,
    star: usize,
    ring: usize,
    mesh: usize,
    tree: usize,
}

impl ParentConnectivityBoundary {
    fn capture(doc: &Hcdf) -> Self {
        Self {
            component: doc.comp.len(),
            harness: doc.harness.len(),
            cable: doc.cable.len(),
            plumbing: doc.plumbing.len(),
            umbilical: doc.umbilical.len(),
            binding: doc.binding.len(),
            mate: doc.mate.len(),
            link: doc.link.len(),
            bus: doc.bus.len(),
            chain: doc.chain.len(),
            star: doc.star.len(),
            ring: doc.ring.len(),
            mesh: doc.mesh.len(),
            tree: doc.tree.len(),
        }
    }
}

type ConnectivityTargetVisitor<'a> = dyn FnMut(
        ConnectivityTargetKind,
        &mut String,
        &mut Option<connectivity_xml::InstanceRef>,
    ) -> Result<(), String>
    + 'a;

fn visit_owner_target(
    owner: &mut connectivity_xml::PhysicalOwnerChoice,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    match owner {
        connectivity_xml::PhysicalOwnerChoice::Component(reference) => visit(
            ConnectivityTargetKind::Component,
            &mut reference.component,
            &mut reference.instance,
        )?,
        connectivity_xml::PhysicalOwnerChoice::Assembly(reference) => visit(
            ConnectivityTargetKind::Assembly,
            &mut reference.assembly,
            &mut reference.instance,
        )?,
    }
    Ok(())
}

fn visit_port_target(
    reference: &mut connectivity_xml::PortRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Component,
        &mut reference.component,
        &mut reference.instance,
    )
}

fn visit_functional_target(
    reference: &mut connectivity_xml::FunctionalEndpointRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    match &mut reference.endpoint {
        connectivity_xml::FunctionalEndpointChoice::Port(reference) => {
            visit_port_target(reference, visit)
        }
        connectivity_xml::FunctionalEndpointChoice::Channel(reference) => visit(
            ConnectivityTargetKind::Component,
            &mut reference.component,
            &mut reference.instance,
        ),
    }
}

fn visit_component_ref_target(
    reference: &mut connectivity_xml::ComponentRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Component,
        &mut reference.component,
        &mut reference.instance,
    )
}

fn visit_function_ref_target(
    reference: &mut connectivity_xml::ConnectivityFunctionRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Component,
        &mut reference.component,
        &mut reference.instance,
    )
}

fn visit_participant_ref_target(
    reference: &mut connectivity_xml::ParticipantRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Network,
        &mut reference.network,
        &mut reference.instance,
    )
}

fn visit_traffic_class_ref_target(
    reference: &mut connectivity_xml::TrafficClassRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Network,
        &mut reference.network,
        &mut reference.instance,
    )
}

fn visit_schedule_ref_target(
    reference: &mut connectivity_xml::ScheduleRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Network,
        &mut reference.network,
        &mut reference.instance,
    )
}

fn visit_network_ref_target(
    reference: &mut connectivity_xml::NetworkRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Network,
        &mut reference.network,
        &mut reference.instance,
    )
}

fn visit_leg_ref_target(
    reference: &mut connectivity_xml::LegRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Network,
        &mut reference.network,
        &mut reference.instance,
    )
}

fn visit_macsec_policy_ref_target(
    reference: &mut connectivity_xml::MacsecPolicyRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Network,
        &mut reference.network,
        &mut reference.instance,
    )
}

fn visit_topology_segment_target(
    reference: &mut connectivity_xml::TopologySegmentRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    match &mut reference.segment {
        connectivity_xml::TopologySegmentChoice::Network(reference) => {
            visit_network_ref_target(reference, visit)
        }
        connectivity_xml::TopologySegmentChoice::Leg(reference) => {
            visit_leg_ref_target(reference, visit)
        }
    }
}

fn visit_network_configuration_targets(
    configuration: &mut connectivity_xml::NetworkConfiguration,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    for domain in &mut configuration.gptp_domain {
        for clock in &mut domain.clock {
            visit_participant_ref_target(&mut clock.participant, visit)?;
        }
    }
    for schedule in &mut configuration.gate_schedule {
        for gate in &mut schedule.gate {
            for reference in &mut gate.open.traffic_class {
                visit_traffic_class_ref_target(reference, visit)?;
            }
        }
    }
    for assignment in &mut configuration.schedule_assignment {
        for target in &mut assignment.target {
            visit_participant_ref_target(&mut target.participant, visit)?;
        }
        visit_schedule_ref_target(&mut assignment.schedule, visit)?;
    }
    if let Some(plca) = &mut configuration.plca {
        for node in &mut plca.node {
            visit_participant_ref_target(&mut node.participant, visit)?;
        }
    }
    if let Some(macsec) = &mut configuration.macsec {
        if let Some(default_policy) = &mut macsec.default_policy {
            visit_macsec_policy_ref_target(&mut default_policy.policy, visit)?;
        }
        for override_ in &mut macsec.override_ {
            visit_topology_segment_target(&mut override_.target, visit)?;
            visit_macsec_policy_ref_target(&mut override_.policy, visit)?;
        }
    }
    if let Some(eee) = &mut configuration.eee {
        for override_ in &mut eee.override_ {
            visit_participant_ref_target(&mut override_.participant, visit)?;
        }
    }
    Ok(())
}

fn visit_hop_ref_target(
    reference: &mut connectivity_xml::HopRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit(
        ConnectivityTargetKind::Network,
        &mut reference.network,
        &mut reference.instance,
    )
}

fn visit_hop_owner_target(
    owner: &mut connectivity_xml::HopOwnerRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    match &mut owner.owner {
        connectivity_xml::HopOwnerChoice::Component(reference) => {
            visit_component_ref_target(reference, visit)
        }
        connectivity_xml::HopOwnerChoice::Function(reference) => {
            visit_function_ref_target(reference, visit)
        }
    }
}

fn visit_participant_target(
    participant: &mut connectivity_xml::Participant,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit_functional_target(&mut participant.endpoint, visit)
}

fn visit_hop_target(
    hop: &mut connectivity_xml::Hop,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit_hop_owner_target(&mut hop.owner, visit)
}

fn visit_leg_end_targets(
    end: &mut connectivity_xml::LegEnd,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit_hop_ref_target(&mut end.hop, visit)?;
    visit_participant_ref_target(&mut end.participant, visit)
}

fn visit_leg_targets(
    leg: &mut connectivity_xml::Leg,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit_leg_end_targets(&mut leg.from, visit)?;
    visit_leg_end_targets(&mut leg.to, visit)
}

fn visit_connector_target(
    reference: &mut connectivity_xml::ConnectorRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit_owner_target(&mut reference.owner, visit)
}

fn visit_position_target(
    reference: &mut connectivity_xml::PositionRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit_owner_target(&mut reference.owner, visit)
}

fn visit_physical_target(
    reference: &mut connectivity_xml::PhysicalEndpointRef,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    match &mut reference.endpoint {
        connectivity_xml::PhysicalEndpointChoice::Connector(reference) => {
            visit_connector_target(reference, visit)
        }
        connectivity_xml::PhysicalEndpointChoice::Position(reference) => {
            visit_position_target(reference, visit)
        }
        connectivity_xml::PhysicalEndpointChoice::Junction(reference) => {
            visit_owner_target(&mut reference.owner, visit)
        }
    }
}

fn visit_route_frame_target(
    frame: &mut connectivity_xml::RouteFrame,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    match &mut frame.frame {
        connectivity_xml::RouteFrameChoice::World(_) => Ok(()),
        connectivity_xml::RouteFrameChoice::ComponentOrigin(frame) => visit(
            ConnectivityTargetKind::Component,
            &mut frame.component,
            &mut frame.instance,
        ),
        connectivity_xml::RouteFrameChoice::ComponentFrame(frame) => visit(
            ConnectivityTargetKind::Component,
            &mut frame.component,
            &mut frame.instance,
        ),
    }
}

fn visit_representation_targets(
    representation: &mut connectivity_xml::Representation,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    match &mut representation.variant {
        connectivity_xml::RepresentationChoice::Box(value) => {
            visit_route_frame_target(&mut value.placement.frame, visit)
        }
        connectivity_xml::RepresentationChoice::Cylinder(value) => {
            visit_route_frame_target(&mut value.placement.frame, visit)
        }
        connectivity_xml::RepresentationChoice::Sphere(value) => {
            visit_route_frame_target(&mut value.placement.frame, visit)
        }
        connectivity_xml::RepresentationChoice::Model(value) => {
            visit_route_frame_target(&mut value.placement.frame, visit)
        }
        connectivity_xml::RepresentationChoice::ModelPart(model_part) => {
            match &mut model_part.model_root.root {
                connectivity_xml::ModelRootChoice::ComponentVisual(root) => visit(
                    ConnectivityTargetKind::Component,
                    &mut root.component,
                    &mut root.instance,
                ),
                connectivity_xml::ModelRootChoice::AssemblyModel(root) => visit(
                    ConnectivityTargetKind::Assembly,
                    &mut root.assembly,
                    &mut root.instance,
                ),
            }
        }
        connectivity_xml::RepresentationChoice::DerivedRoute(route) => {
            for waypoint in &mut route.waypoint {
                visit_route_frame_target(&mut waypoint.frame, visit)?;
            }
            Ok(())
        }
    }
}

fn visit_connector_targets(
    connector: &mut connectivity_xml::Connector,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    if let Some(representation) = &mut connector.representation {
        visit_representation_targets(representation, visit)?;
    }
    for position in connector
        .pin
        .iter_mut()
        .chain(&mut connector.socket)
        .chain(&mut connector.contact)
        .chain(&mut connector.fiber)
        .chain(&mut connector.passage)
        .chain(&mut connector.feed)
        .chain(&mut connector.waveguide_opening)
    {
        if let Some(representation) = &mut position.representation {
            visit_representation_targets(representation, visit)?;
        }
    }
    Ok(())
}

fn visit_path_targets(
    path: &mut connectivity_xml::Path,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    visit_physical_target(&mut path.first, visit)?;
    visit_physical_target(&mut path.second, visit)?;
    if let Some(representation) = &mut path.representation {
        visit_representation_targets(representation, visit)?;
    }
    Ok(())
}

fn visit_junction_targets(
    junction: &mut connectivity_xml::Junction,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    for attachment in &mut junction.attachment {
        visit_physical_target(attachment, visit)?;
    }
    if let Some(representation) = &mut junction.representation {
        visit_representation_targets(representation, visit)?;
    }
    Ok(())
}

fn visit_termination_targets(
    termination: &mut connectivity_xml::Termination,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    for attachment in &mut termination.attachment {
        visit_physical_target(attachment, visit)?;
    }
    if let Some(representation) = &mut termination.representation {
        visit_representation_targets(representation, visit)?;
    }
    Ok(())
}

fn visit_assembly_targets(
    assembly: &mut connectivity_xml::Assembly,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    if let Some(representation) = &mut assembly.representation {
        visit_representation_targets(representation, visit)?;
    }
    for connector in &mut assembly.connector {
        visit_connector_targets(connector, visit)?;
    }
    for path in assembly
        .wire
        .iter_mut()
        .chain(&mut assembly.conductor)
        .chain(&mut assembly.cable_member)
        .chain(&mut assembly.fiber)
        .chain(&mut assembly.coax)
        .chain(&mut assembly.waveguide)
        .chain(&mut assembly.feed)
        .chain(&mut assembly.hose)
        .chain(&mut assembly.pipe)
        .chain(&mut assembly.passage)
    {
        visit_path_targets(path, visit)?;
    }
    for junction in assembly
        .splice
        .iter_mut()
        .chain(&mut assembly.tee)
        .chain(&mut assembly.manifold)
        .chain(&mut assembly.busbar)
        .chain(&mut assembly.optical_splitter)
    {
        visit_junction_targets(junction, visit)?;
    }
    for termination in &mut assembly.termination {
        visit_termination_targets(termination, visit)?;
    }
    Ok(())
}

fn visit_parent_connectivity_targets(
    doc: &mut Hcdf,
    boundary: ParentConnectivityBoundary,
    visit: &mut ConnectivityTargetVisitor<'_>,
) -> Result<(), String> {
    for component in doc.comp.iter_mut().take(boundary.component) {
        for connector in &mut component.connector {
            visit_connector_targets(connector, visit)?;
        }
        for antenna in &mut component.antenna {
            if let Some(port) = &mut antenna.conducted_port {
                visit_port_target(port, visit)?;
            }
            visit_port_target(&mut antenna.radiated_port, visit)?;
            if let Some(representation) = &mut antenna.representation {
                visit_representation_targets(representation, visit)?;
            }
        }
        for function in component
            .switch
            .iter_mut()
            .chain(&mut component.bridge)
            .chain(&mut component.converter)
            .chain(&mut component.transceiver)
            .chain(&mut component.radio)
        {
            for endpoint in function
                .input
                .iter_mut()
                .chain(&mut function.output)
                .chain(&mut function.bidirectional)
            {
                visit_functional_target(endpoint, visit)?;
            }
        }
        for path in component
            .wire
            .iter_mut()
            .chain(&mut component.conductor)
            .chain(&mut component.cable_member)
            .chain(&mut component.fiber)
            .chain(&mut component.coax)
            .chain(&mut component.waveguide)
            .chain(&mut component.feed)
            .chain(&mut component.hose)
            .chain(&mut component.pipe)
            .chain(&mut component.passage)
        {
            visit_path_targets(path, visit)?;
        }
        for junction in component
            .splice
            .iter_mut()
            .chain(&mut component.tee)
            .chain(&mut component.manifold)
            .chain(&mut component.busbar)
            .chain(&mut component.optical_splitter)
        {
            visit_junction_targets(junction, visit)?;
        }
        for termination in &mut component.termination {
            visit_termination_targets(termination, visit)?;
        }
    }

    for assembly in doc.harness.iter_mut().take(boundary.harness) {
        visit_assembly_targets(assembly, visit)?;
    }
    for assembly in doc.cable.iter_mut().take(boundary.cable) {
        visit_assembly_targets(assembly, visit)?;
    }
    for assembly in doc.plumbing.iter_mut().take(boundary.plumbing) {
        visit_assembly_targets(assembly, visit)?;
    }
    for assembly in doc.umbilical.iter_mut().take(boundary.umbilical) {
        visit_assembly_targets(assembly, visit)?;
    }

    for binding in doc.binding.iter_mut().take(boundary.binding) {
        visit_functional_target(&mut binding.functional, visit)?;
        visit_physical_target(&mut binding.physical, visit)?;
    }
    for mate in doc.mate.iter_mut().take(boundary.mate) {
        visit_connector_target(&mut mate.first, visit)?;
        visit_connector_target(&mut mate.second, visit)?;
        for mapping in &mut mate.position_mapping {
            visit_position_target(&mut mapping.first, visit)?;
            visit_position_target(&mut mapping.second, visit)?;
        }
    }

    macro_rules! visit_network_participants {
        ($networks:expr, $limit:expr) => {
            for network in $networks.iter_mut().take($limit) {
                for participant in &mut network.participant {
                    visit_participant_target(participant, visit)?;
                }
                if let Some(configuration) = &mut network.configuration {
                    visit_network_configuration_targets(configuration, visit)?;
                }
            }
        };
    }
    visit_network_participants!(doc.link, boundary.link);
    visit_network_participants!(doc.bus, boundary.bus);
    visit_network_participants!(doc.chain, boundary.chain);
    visit_network_participants!(doc.star, boundary.star);
    visit_network_participants!(doc.ring, boundary.ring);
    visit_network_participants!(doc.mesh, boundary.mesh);
    visit_network_participants!(doc.tree, boundary.tree);

    for network in doc.chain.iter_mut().take(boundary.chain) {
        for hop in &mut network.hop {
            visit_hop_target(hop, visit)?;
        }
        for leg in &mut network.leg {
            visit_leg_targets(leg, visit)?;
        }
    }
    for network in doc.ring.iter_mut().take(boundary.ring) {
        for hop in &mut network.hop {
            visit_hop_target(hop, visit)?;
        }
        for leg in &mut network.leg {
            visit_leg_targets(leg, visit)?;
        }
    }
    for network in doc.star.iter_mut().take(boundary.star) {
        visit_participant_ref_target(&mut network.coordinator.participant, visit)?;
    }
    for network in doc.tree.iter_mut().take(boundary.tree) {
        visit_hop_ref_target(&mut network.root.hop, visit)?;
        for hop in &mut network.hop {
            visit_hop_target(hop, visit)?;
        }
        for leg in &mut network.leg {
            visit_leg_targets(leg, visit)?;
        }
    }
    Ok(())
}

pub(crate) fn visit_connectivity_instance_ref_slots_mut(
    doc: &mut Hcdf,
    visit: &mut impl FnMut(&mut Option<connectivity_xml::InstanceRef>) -> Result<(), String>,
) -> Result<(), String> {
    let boundary = ParentConnectivityBoundary::capture(doc);
    visit_parent_connectivity_targets(doc, boundary, &mut |_kind, _target, instance| {
        visit(instance)
    })
}

fn reject_nonzero_connectivity_occurrences(
    doc: &mut Hcdf,
    boundary: ParentConnectivityBoundary,
) -> Result<(), String> {
    visit_parent_connectivity_targets(doc, boundary, &mut |_kind, _target, instance| {
        if let Some(segment) = instance.as_ref().and_then(|reference| {
            reference
                .segment
                .iter()
                .find(|segment| segment.occurrence != 0)
        }) {
            return Err(format!(
                "connectivity instance segment {:?} uses unsupported occurrence {}",
                segment.name, segment.occurrence
            ));
        }
        Ok(())
    })
}

fn collapse_parent_instance_refs(
    doc: &mut Hcdf,
    boundary: ParentConnectivityBoundary,
    sub: &Hcdf,
) -> Result<(), String> {
    let components: HashSet<String> = sub.comp.iter().map(|value| value.name.clone()).collect();
    let assemblies: HashSet<String> = sub
        .harness
        .iter()
        .chain(&sub.cable)
        .chain(&sub.plumbing)
        .chain(&sub.umbilical)
        .map(|value| value.name.clone())
        .collect();
    let networks: HashSet<String> = sub
        .link
        .iter()
        .map(|value| value.name.clone())
        .chain(sub.bus.iter().map(|value| value.name.clone()))
        .chain(sub.chain.iter().map(|value| value.name.clone()))
        .chain(sub.star.iter().map(|value| value.name.clone()))
        .chain(sub.ring.iter().map(|value| value.name.clone()))
        .chain(sub.mesh.iter().map(|value| value.name.clone()))
        .chain(sub.tree.iter().map(|value| value.name.clone()))
        .collect();
    visit_parent_connectivity_targets(doc, boundary, &mut |kind, target, instance| {
        let Some(reference) = instance.as_ref() else {
            return Ok(());
        };
        let Some(instance_prefix) = instance_prefix(reference) else {
            return Ok(());
        };
        let candidate = join_instance_target(&instance_prefix, target);
        let definitions = match kind {
            ConnectivityTargetKind::Component => &components,
            ConnectivityTargetKind::Assembly => &assemblies,
            ConnectivityTargetKind::Network => &networks,
        };
        if definitions.contains(&candidate) {
            *target = candidate;
            *instance = None;
        }
        Ok(())
    })
}

fn instance_prefix(instance: &connectivity_xml::InstanceRef) -> Option<String> {
    if instance.segment.is_empty() {
        return None;
    }
    let mut named = Vec::new();
    for segment in &instance.segment {
        if segment.occurrence != 0 {
            return None;
        }
        match segment.name.as_deref() {
            None => {}
            Some("") => return None,
            Some(name) => named.push(name),
        }
    }
    Some(named.join(SEP))
}

fn join_instance_target(instance_prefix: &str, target: &str) -> String {
    if instance_prefix.is_empty() {
        target.to_owned()
    } else {
        format!("{instance_prefix}{SEP}{target}")
    }
}

fn rewrite_target(
    target: &mut String,
    instance: &mut Option<connectivity_xml::InstanceRef>,
    definitions: &HashSet<String>,
    prefix: &str,
) {
    match instance.as_ref() {
        Some(reference) => {
            let Some(instance_prefix) = instance_prefix(reference) else {
                return;
            };
            let candidate = join_instance_target(&instance_prefix, target);
            if definitions.contains(&candidate) {
                *target = format!("{prefix}{candidate}");
                *instance = None;
            }
        }
        None => {
            if definitions.contains(target) {
                *target = format!("{prefix}{target}");
            }
        }
    }
}

fn rewrite_owner(
    owner: &mut connectivity_xml::PhysicalOwnerChoice,
    components: &HashSet<String>,
    assemblies: &HashSet<String>,
    prefix: &str,
) {
    match owner {
        connectivity_xml::PhysicalOwnerChoice::Component(reference) => rewrite_target(
            &mut reference.component,
            &mut reference.instance,
            components,
            prefix,
        ),
        connectivity_xml::PhysicalOwnerChoice::Assembly(reference) => rewrite_target(
            &mut reference.assembly,
            &mut reference.instance,
            assemblies,
            prefix,
        ),
    }
}

fn rewrite_port_ref(
    reference: &mut connectivity_xml::PortRef,
    components: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.component,
        &mut reference.instance,
        components,
        prefix,
    );
}

fn rewrite_channel_ref(
    reference: &mut connectivity_xml::ChannelRef,
    components: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.component,
        &mut reference.instance,
        components,
        prefix,
    );
}

fn rewrite_functional_ref(
    reference: &mut connectivity_xml::FunctionalEndpointRef,
    components: &HashSet<String>,
    prefix: &str,
) {
    match &mut reference.endpoint {
        connectivity_xml::FunctionalEndpointChoice::Port(port) => {
            rewrite_port_ref(port, components, prefix);
        }
        connectivity_xml::FunctionalEndpointChoice::Channel(channel) => {
            rewrite_channel_ref(channel, components, prefix);
        }
    }
}

fn rewrite_component_ref(
    reference: &mut connectivity_xml::ComponentRef,
    components: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.component,
        &mut reference.instance,
        components,
        prefix,
    );
}

fn rewrite_function_ref(
    reference: &mut connectivity_xml::ConnectivityFunctionRef,
    components: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.component,
        &mut reference.instance,
        components,
        prefix,
    );
}

fn rewrite_participant_ref(
    reference: &mut connectivity_xml::ParticipantRef,
    networks: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.network,
        &mut reference.instance,
        networks,
        prefix,
    );
}

fn rewrite_traffic_class_ref(
    reference: &mut connectivity_xml::TrafficClassRef,
    networks: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.network,
        &mut reference.instance,
        networks,
        prefix,
    );
}

fn rewrite_schedule_ref(
    reference: &mut connectivity_xml::ScheduleRef,
    networks: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.network,
        &mut reference.instance,
        networks,
        prefix,
    );
}

fn rewrite_network_ref(
    reference: &mut connectivity_xml::NetworkRef,
    networks: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.network,
        &mut reference.instance,
        networks,
        prefix,
    );
}

fn rewrite_leg_ref(
    reference: &mut connectivity_xml::LegRef,
    networks: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.network,
        &mut reference.instance,
        networks,
        prefix,
    );
}

fn rewrite_macsec_policy_ref(
    reference: &mut connectivity_xml::MacsecPolicyRef,
    networks: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.network,
        &mut reference.instance,
        networks,
        prefix,
    );
}

fn rewrite_topology_segment_ref(
    reference: &mut connectivity_xml::TopologySegmentRef,
    networks: &HashSet<String>,
    prefix: &str,
) {
    match &mut reference.segment {
        connectivity_xml::TopologySegmentChoice::Network(reference) => {
            rewrite_network_ref(reference, networks, prefix);
        }
        connectivity_xml::TopologySegmentChoice::Leg(reference) => {
            rewrite_leg_ref(reference, networks, prefix);
        }
    }
}

fn rewrite_network_configuration(
    configuration: &mut connectivity_xml::NetworkConfiguration,
    networks: &HashSet<String>,
    prefix: &str,
) {
    for domain in &mut configuration.gptp_domain {
        for clock in &mut domain.clock {
            rewrite_participant_ref(&mut clock.participant, networks, prefix);
        }
    }
    for schedule in &mut configuration.gate_schedule {
        for gate in &mut schedule.gate {
            for reference in &mut gate.open.traffic_class {
                rewrite_traffic_class_ref(reference, networks, prefix);
            }
        }
    }
    for assignment in &mut configuration.schedule_assignment {
        for target in &mut assignment.target {
            rewrite_participant_ref(&mut target.participant, networks, prefix);
        }
        rewrite_schedule_ref(&mut assignment.schedule, networks, prefix);
    }
    if let Some(plca) = &mut configuration.plca {
        for node in &mut plca.node {
            rewrite_participant_ref(&mut node.participant, networks, prefix);
        }
    }
    if let Some(macsec) = &mut configuration.macsec {
        if let Some(default_policy) = &mut macsec.default_policy {
            rewrite_macsec_policy_ref(&mut default_policy.policy, networks, prefix);
        }
        for override_ in &mut macsec.override_ {
            rewrite_topology_segment_ref(&mut override_.target, networks, prefix);
            rewrite_macsec_policy_ref(&mut override_.policy, networks, prefix);
        }
    }
    if let Some(eee) = &mut configuration.eee {
        for override_ in &mut eee.override_ {
            rewrite_participant_ref(&mut override_.participant, networks, prefix);
        }
    }
}

fn rewrite_hop_ref(
    reference: &mut connectivity_xml::HopRef,
    networks: &HashSet<String>,
    prefix: &str,
) {
    rewrite_target(
        &mut reference.network,
        &mut reference.instance,
        networks,
        prefix,
    );
}

fn rewrite_hop_owner(
    owner: &mut connectivity_xml::HopOwnerRef,
    components: &HashSet<String>,
    prefix: &str,
) {
    match &mut owner.owner {
        connectivity_xml::HopOwnerChoice::Component(reference) => {
            rewrite_component_ref(reference, components, prefix);
        }
        connectivity_xml::HopOwnerChoice::Function(reference) => {
            rewrite_function_ref(reference, components, prefix);
        }
    }
}

fn rewrite_participant(
    participant: &mut connectivity_xml::Participant,
    components: &HashSet<String>,
    prefix: &str,
) {
    rewrite_functional_ref(&mut participant.endpoint, components, prefix);
}

fn rewrite_hop(hop: &mut connectivity_xml::Hop, components: &HashSet<String>, prefix: &str) {
    rewrite_hop_owner(&mut hop.owner, components, prefix);
}

fn rewrite_leg_end(end: &mut connectivity_xml::LegEnd, networks: &HashSet<String>, prefix: &str) {
    rewrite_hop_ref(&mut end.hop, networks, prefix);
    rewrite_participant_ref(&mut end.participant, networks, prefix);
}

fn rewrite_leg(leg: &mut connectivity_xml::Leg, networks: &HashSet<String>, prefix: &str) {
    rewrite_leg_end(&mut leg.from, networks, prefix);
    rewrite_leg_end(&mut leg.to, networks, prefix);
}

fn rewrite_connector_ref(
    reference: &mut connectivity_xml::ConnectorRef,
    components: &HashSet<String>,
    assemblies: &HashSet<String>,
    prefix: &str,
) {
    rewrite_owner(&mut reference.owner, components, assemblies, prefix);
}

fn rewrite_position_ref(
    reference: &mut connectivity_xml::PositionRef,
    components: &HashSet<String>,
    assemblies: &HashSet<String>,
    prefix: &str,
) {
    rewrite_owner(&mut reference.owner, components, assemblies, prefix);
}

fn rewrite_junction_ref(
    reference: &mut connectivity_xml::JunctionRef,
    components: &HashSet<String>,
    assemblies: &HashSet<String>,
    prefix: &str,
) {
    rewrite_owner(&mut reference.owner, components, assemblies, prefix);
}

fn rewrite_physical_ref(
    reference: &mut connectivity_xml::PhysicalEndpointRef,
    components: &HashSet<String>,
    assemblies: &HashSet<String>,
    prefix: &str,
) {
    match &mut reference.endpoint {
        connectivity_xml::PhysicalEndpointChoice::Connector(connector) => {
            rewrite_connector_ref(connector, components, assemblies, prefix);
        }
        connectivity_xml::PhysicalEndpointChoice::Position(position) => {
            rewrite_position_ref(position, components, assemblies, prefix);
        }
        connectivity_xml::PhysicalEndpointChoice::Junction(junction) => {
            rewrite_junction_ref(junction, components, assemblies, prefix);
        }
    }
}

fn rewrite_route_frame_ref(
    frame: &mut connectivity_xml::RouteFrame,
    components: &HashSet<String>,
    prefix: &str,
) {
    match &mut frame.frame {
        connectivity_xml::RouteFrameChoice::World(_) => {}
        connectivity_xml::RouteFrameChoice::ComponentOrigin(frame) => {
            rewrite_target(
                &mut frame.component,
                &mut frame.instance,
                components,
                prefix,
            );
        }
        connectivity_xml::RouteFrameChoice::ComponentFrame(frame) => {
            rewrite_target(
                &mut frame.component,
                &mut frame.instance,
                components,
                prefix,
            );
        }
    }
}

fn rewrite_placement_ref(
    placement: &mut connectivity_xml::Placement,
    components: &HashSet<String>,
    prefix: &str,
) {
    rewrite_route_frame_ref(&mut placement.frame, components, prefix);
}

fn rewrite_representation_refs(
    representation: &mut connectivity_xml::Representation,
    components: &HashSet<String>,
    assemblies: &HashSet<String>,
    prefix: &str,
) {
    match &mut representation.variant {
        connectivity_xml::RepresentationChoice::Box(value) => {
            rewrite_placement_ref(&mut value.placement, components, prefix);
        }
        connectivity_xml::RepresentationChoice::Cylinder(value) => {
            rewrite_placement_ref(&mut value.placement, components, prefix);
        }
        connectivity_xml::RepresentationChoice::Sphere(value) => {
            rewrite_placement_ref(&mut value.placement, components, prefix);
        }
        connectivity_xml::RepresentationChoice::Model(value) => {
            rewrite_placement_ref(&mut value.placement, components, prefix);
        }
        connectivity_xml::RepresentationChoice::ModelPart(model_part) => {
            match &mut model_part.model_root.root {
                connectivity_xml::ModelRootChoice::ComponentVisual(root) => {
                    rewrite_target(&mut root.component, &mut root.instance, components, prefix);
                }
                connectivity_xml::ModelRootChoice::AssemblyModel(root) => {
                    rewrite_target(&mut root.assembly, &mut root.instance, assemblies, prefix);
                }
            }
        }
        connectivity_xml::RepresentationChoice::DerivedRoute(route) => {
            for waypoint in &mut route.waypoint {
                rewrite_route_frame_ref(&mut waypoint.frame, components, prefix);
            }
        }
    }
}
fn prefix_connectivity(
    sub: &mut Hcdf,
    prefix: &str,
    components: &HashSet<String>,
    assemblies: &HashSet<String>,
    networks: &HashSet<String>,
) {
    for component in &mut sub.comp {
        for antenna in &mut component.antenna {
            if let Some(port) = &mut antenna.conducted_port {
                rewrite_port_ref(port, components, prefix);
            }
            rewrite_port_ref(&mut antenna.radiated_port, components, prefix);
        }
        for function in component
            .switch
            .iter_mut()
            .chain(&mut component.bridge)
            .chain(&mut component.converter)
            .chain(&mut component.transceiver)
            .chain(&mut component.radio)
        {
            for endpoint in function
                .input
                .iter_mut()
                .chain(&mut function.output)
                .chain(&mut function.bidirectional)
            {
                rewrite_functional_ref(endpoint, components, prefix);
            }
        }
        for path in component
            .wire
            .iter_mut()
            .chain(&mut component.conductor)
            .chain(&mut component.cable_member)
            .chain(&mut component.fiber)
            .chain(&mut component.coax)
            .chain(&mut component.waveguide)
            .chain(&mut component.feed)
            .chain(&mut component.hose)
            .chain(&mut component.pipe)
            .chain(&mut component.passage)
        {
            rewrite_physical_ref(&mut path.first, components, assemblies, prefix);
            rewrite_physical_ref(&mut path.second, components, assemblies, prefix);
        }
        for junction in component
            .splice
            .iter_mut()
            .chain(&mut component.tee)
            .chain(&mut component.manifold)
            .chain(&mut component.busbar)
            .chain(&mut component.optical_splitter)
        {
            for attachment in &mut junction.attachment {
                rewrite_physical_ref(attachment, components, assemblies, prefix);
            }
        }
        for termination in &mut component.termination {
            for attachment in &mut termination.attachment {
                rewrite_physical_ref(attachment, components, assemblies, prefix);
            }
        }
    }

    for assembly in sub
        .harness
        .iter_mut()
        .chain(&mut sub.cable)
        .chain(&mut sub.plumbing)
        .chain(&mut sub.umbilical)
    {
        for path in assembly
            .wire
            .iter_mut()
            .chain(&mut assembly.conductor)
            .chain(&mut assembly.cable_member)
            .chain(&mut assembly.fiber)
            .chain(&mut assembly.coax)
            .chain(&mut assembly.waveguide)
            .chain(&mut assembly.feed)
            .chain(&mut assembly.hose)
            .chain(&mut assembly.pipe)
            .chain(&mut assembly.passage)
        {
            rewrite_physical_ref(&mut path.first, components, assemblies, prefix);
            rewrite_physical_ref(&mut path.second, components, assemblies, prefix);
        }
        for junction in assembly
            .splice
            .iter_mut()
            .chain(&mut assembly.tee)
            .chain(&mut assembly.manifold)
            .chain(&mut assembly.busbar)
            .chain(&mut assembly.optical_splitter)
        {
            for attachment in &mut junction.attachment {
                rewrite_physical_ref(attachment, components, assemblies, prefix);
            }
        }
        for termination in &mut assembly.termination {
            for attachment in &mut termination.attachment {
                rewrite_physical_ref(attachment, components, assemblies, prefix);
            }
        }
    }

    for binding in &mut sub.binding {
        rewrite_functional_ref(&mut binding.functional, components, prefix);
        rewrite_physical_ref(&mut binding.physical, components, assemblies, prefix);
        binding.name = format!("{prefix}{}", binding.name);
    }
    for mate in &mut sub.mate {
        rewrite_connector_ref(&mut mate.first, components, assemblies, prefix);
        rewrite_connector_ref(&mut mate.second, components, assemblies, prefix);
        for mapping in &mut mate.position_mapping {
            rewrite_position_ref(&mut mapping.first, components, assemblies, prefix);
            rewrite_position_ref(&mut mapping.second, components, assemblies, prefix);
        }
        mate.name = format!("{prefix}{}", mate.name);
    }

    for network in &mut sub.chain {
        for hop in &mut network.hop {
            rewrite_hop(hop, components, prefix);
        }
        for leg in &mut network.leg {
            rewrite_leg(leg, networks, prefix);
        }
    }
    for network in &mut sub.ring {
        for hop in &mut network.hop {
            rewrite_hop(hop, components, prefix);
        }
        for leg in &mut network.leg {
            rewrite_leg(leg, networks, prefix);
        }
    }
    for network in &mut sub.star {
        rewrite_participant_ref(&mut network.coordinator.participant, networks, prefix);
    }
    for network in &mut sub.tree {
        rewrite_hop_ref(&mut network.root.hop, networks, prefix);
        for hop in &mut network.hop {
            rewrite_hop(hop, components, prefix);
        }
        for leg in &mut network.leg {
            rewrite_leg(leg, networks, prefix);
        }
    }

    macro_rules! rewrite_networks {
        ($networks:expr) => {
            for network in $networks {
                for participant in &mut network.participant {
                    rewrite_participant(participant, components, prefix);
                }
                if let Some(configuration) = &mut network.configuration {
                    rewrite_network_configuration(configuration, networks, prefix);
                }
                network.name = format!("{prefix}{}", network.name);
            }
        };
    }
    rewrite_networks!(&mut sub.link);
    rewrite_networks!(&mut sub.bus);
    rewrite_networks!(&mut sub.chain);
    rewrite_networks!(&mut sub.star);
    rewrite_networks!(&mut sub.ring);
    rewrite_networks!(&mut sub.mesh);
    rewrite_networks!(&mut sub.tree);

    visit_connectivity_representations_mut(sub, &mut |representation| {
        rewrite_representation_refs(representation, components, assemblies, prefix);
    });

    for assembly in sub
        .harness
        .iter_mut()
        .chain(&mut sub.cable)
        .chain(&mut sub.plumbing)
        .chain(&mut sub.umbilical)
    {
        assembly.name = format!("{prefix}{}", assembly.name);
    }
}

fn visit_connector_representations_mut(
    connector: &mut connectivity_xml::Connector,
    visit: &mut impl FnMut(&mut connectivity_xml::Representation),
) {
    if let Some(representation) = &mut connector.representation {
        visit(representation);
    }
    for position in connector
        .pin
        .iter_mut()
        .chain(&mut connector.socket)
        .chain(&mut connector.contact)
        .chain(&mut connector.fiber)
        .chain(&mut connector.passage)
        .chain(&mut connector.feed)
        .chain(&mut connector.waveguide_opening)
    {
        if let Some(representation) = &mut position.representation {
            visit(representation);
        }
    }
}

fn visit_component_representations_mut(
    component: &mut crate::model::Comp,
    visit: &mut impl FnMut(&mut connectivity_xml::Representation),
) {
    for connector in &mut component.connector {
        visit_connector_representations_mut(connector, visit);
    }
    for antenna in &mut component.antenna {
        if let Some(representation) = &mut antenna.representation {
            visit(representation);
        }
    }
    for path in component
        .wire
        .iter_mut()
        .chain(&mut component.conductor)
        .chain(&mut component.cable_member)
        .chain(&mut component.fiber)
        .chain(&mut component.coax)
        .chain(&mut component.waveguide)
        .chain(&mut component.feed)
        .chain(&mut component.hose)
        .chain(&mut component.pipe)
        .chain(&mut component.passage)
    {
        if let Some(representation) = &mut path.representation {
            visit(representation);
        }
    }
    for junction in component
        .splice
        .iter_mut()
        .chain(&mut component.tee)
        .chain(&mut component.manifold)
        .chain(&mut component.busbar)
        .chain(&mut component.optical_splitter)
    {
        if let Some(representation) = &mut junction.representation {
            visit(representation);
        }
    }
    for termination in &mut component.termination {
        if let Some(representation) = &mut termination.representation {
            visit(representation);
        }
    }
}

fn visit_assembly_representations_mut(
    assembly: &mut connectivity_xml::Assembly,
    visit: &mut impl FnMut(&mut connectivity_xml::Representation),
) {
    if let Some(representation) = &mut assembly.representation {
        visit(representation);
    }
    for connector in &mut assembly.connector {
        visit_connector_representations_mut(connector, visit);
    }
    for path in assembly
        .wire
        .iter_mut()
        .chain(&mut assembly.conductor)
        .chain(&mut assembly.cable_member)
        .chain(&mut assembly.fiber)
        .chain(&mut assembly.coax)
        .chain(&mut assembly.waveguide)
        .chain(&mut assembly.feed)
        .chain(&mut assembly.hose)
        .chain(&mut assembly.pipe)
        .chain(&mut assembly.passage)
    {
        if let Some(representation) = &mut path.representation {
            visit(representation);
        }
    }
    for junction in assembly
        .splice
        .iter_mut()
        .chain(&mut assembly.tee)
        .chain(&mut assembly.manifold)
        .chain(&mut assembly.busbar)
        .chain(&mut assembly.optical_splitter)
    {
        if let Some(representation) = &mut junction.representation {
            visit(representation);
        }
    }
    for termination in &mut assembly.termination {
        if let Some(representation) = &mut termination.representation {
            visit(representation);
        }
    }
}

fn visit_parent_connectivity_representations_mut(
    doc: &mut Hcdf,
    boundary: ParentConnectivityBoundary,
    visit: &mut impl FnMut(&mut connectivity_xml::Representation),
) {
    for component in doc.comp.iter_mut().take(boundary.component) {
        visit_component_representations_mut(component, visit);
    }
    for assembly in doc.harness.iter_mut().take(boundary.harness) {
        visit_assembly_representations_mut(assembly, visit);
    }
    for assembly in doc.cable.iter_mut().take(boundary.cable) {
        visit_assembly_representations_mut(assembly, visit);
    }
    for assembly in doc.plumbing.iter_mut().take(boundary.plumbing) {
        visit_assembly_representations_mut(assembly, visit);
    }
    for assembly in doc.umbilical.iter_mut().take(boundary.umbilical) {
        visit_assembly_representations_mut(assembly, visit);
    }
}

fn visit_connectivity_representations_mut(
    doc: &mut Hcdf,
    visit: &mut impl FnMut(&mut connectivity_xml::Representation),
) {
    let boundary = ParentConnectivityBoundary::capture(doc);
    visit_parent_connectivity_representations_mut(doc, boundary, visit);
}

/// Prefix every `Some(name)` produced by `it` with `p` (definitions only; `None` names untouched).
fn rename_opt<'a, I>(it: I, p: &str)
where
    I: Iterator<Item = &'a mut Option<String>>,
{
    for name in it.flatten() {
        *name = format!("{p}{name}");
    }
}

// ── pose offset (rigid placement) ─────────────────────────────────────────────

fn prefix_origin_adjustments(adjustments: &mut ComponentOriginAdjustments, prefix: &str) {
    let prefix = format!("{prefix}{SEP}");
    let current = std::mem::take(adjustments);
    adjustments.extend(
        current
            .into_iter()
            .map(|(component, transform)| (format!("{prefix}{component}"), transform)),
    );
}

fn root_component_names(doc: &Hcdf) -> HashSet<String> {
    let children: HashSet<String> = doc
        .joint
        .iter()
        .filter(|joint| joint.loop_.is_none())
        .filter_map(|joint| {
            joint
                .child
                .as_ref()
                .and_then(|endpoint| endpoint.comp.clone())
        })
        .collect();
    doc.comp
        .iter()
        .map(|component| component.name.clone())
        .filter(|name| !children.contains(name))
        .collect()
}

fn record_component_origin_adjustments(
    sub: &Hcdf,
    pose: &str,
    adjustments: &mut ComponentOriginAdjustments,
) {
    let transform = offset_matrix(pose);
    for component in root_component_names(sub) {
        let cumulative = adjustments
            .get(&component)
            .map(|inner| mul44(&transform, inner))
            .unwrap_or(transform);
        adjustments.insert(component, cumulative);
    }
}

fn component_origin_adjustment(
    frame: &connectivity_xml::RouteFrame,
    adjustments: &ComponentOriginAdjustments,
) -> Option<Mat4> {
    let connectivity_xml::RouteFrameChoice::ComponentOrigin(frame) = &frame.frame else {
        return None;
    };
    let instance = frame.instance.as_ref()?;
    let instance_prefix = instance_prefix(instance)?;
    adjustments
        .get(&format!("{instance_prefix}{SEP}{}", frame.component))
        .copied()
}

fn transform_connectivity_placement(placement: &mut connectivity_xml::Placement, transform: &Mat4) {
    let (rpy, quat) = match &placement.rotation.rotation {
        connectivity_xml::PlacementRotationChoice::Rpy(rotation) => (Some(rotation.value), None),
        connectivity_xml::PlacementRotationChoice::Quaternion(rotation) => {
            (None, Some(rotation.value))
        }
    };
    let pose = Pose {
        xyz: Some(placement.xyz),
        rpy,
        quat,
    };
    let transformed = matrix_to_pose(&mul44(transform, &pose_to_matrix(&pose)));
    placement.xyz = transformed.xyz_or_zero();
    placement.rotation = connectivity_xml::PlacementRotation {
        rotation: connectivity_xml::PlacementRotationChoice::Rpy(connectivity_xml::RpyRotation {
            value: transformed.rpy_or_zero(),
        }),
    };
}

fn transform_route_point(point: &mut connectivity_xml::RoutePoint, transform: &Mat4) {
    let Some(rotation) = point.rotation.as_ref() else {
        point.xyz = transform_point(transform, point.xyz);
        return;
    };
    let (rpy, quat) = match &rotation.rotation {
        connectivity_xml::PlacementRotationChoice::Rpy(rotation) => (Some(rotation.value), None),
        connectivity_xml::PlacementRotationChoice::Quaternion(rotation) => {
            (None, Some(rotation.value))
        }
    };
    let pose = Pose {
        xyz: Some(point.xyz),
        rpy,
        quat,
    };
    let transformed = matrix_to_pose(&mul44(transform, &pose_to_matrix(&pose)));
    point.xyz = transformed.xyz_or_zero();
    point.rotation = Some(connectivity_xml::PlacementRotation {
        rotation: connectivity_xml::PlacementRotationChoice::Rpy(connectivity_xml::RpyRotation {
            value: transformed.rpy_or_zero(),
        }),
    });
}

fn apply_parent_component_origin_adjustments(
    doc: &mut Hcdf,
    boundary: ParentConnectivityBoundary,
    adjustments: &ComponentOriginAdjustments,
) {
    visit_parent_connectivity_representations_mut(doc, boundary, &mut |representation| {
        match &mut representation.variant {
            connectivity_xml::RepresentationChoice::Box(value) => {
                if let Some(transform) =
                    component_origin_adjustment(&value.placement.frame, adjustments)
                {
                    transform_connectivity_placement(&mut value.placement, &transform);
                }
            }
            connectivity_xml::RepresentationChoice::Cylinder(value) => {
                if let Some(transform) =
                    component_origin_adjustment(&value.placement.frame, adjustments)
                {
                    transform_connectivity_placement(&mut value.placement, &transform);
                }
            }
            connectivity_xml::RepresentationChoice::Sphere(value) => {
                if let Some(transform) =
                    component_origin_adjustment(&value.placement.frame, adjustments)
                {
                    transform_connectivity_placement(&mut value.placement, &transform);
                }
            }
            connectivity_xml::RepresentationChoice::Model(value) => {
                if let Some(transform) =
                    component_origin_adjustment(&value.placement.frame, adjustments)
                {
                    transform_connectivity_placement(&mut value.placement, &transform);
                }
            }
            connectivity_xml::RepresentationChoice::ModelPart(_) => {}
            connectivity_xml::RepresentationChoice::DerivedRoute(route) => {
                for waypoint in &mut route.waypoint {
                    if let Some(transform) =
                        component_origin_adjustment(&waypoint.frame, adjustments)
                    {
                        transform_route_point(waypoint, &transform);
                    }
                }
            }
        }
    });
}

fn transform_point(transform: &Mat4, point: [f64; 3]) -> [f64; 3] {
    [
        transform[0][0] * point[0]
            + transform[0][1] * point[1]
            + transform[0][2] * point[2]
            + transform[0][3],
        transform[1][0] * point[0]
            + transform[1][1] * point[1]
            + transform[1][2] * point[2]
            + transform[1][3],
        transform[2][0] * point[0]
            + transform[2][1] * point[1]
            + transform[2][2] * point[2]
            + transform[2][3],
    ]
}

fn offset_connectivity_representation(
    representation: &mut connectivity_xml::Representation,
    transform: &Mat4,
    roots: &HashSet<String>,
) {
    let offset_placement = |placement: &mut connectivity_xml::Placement| {
        let moves_with_include = match &placement.frame.frame {
            connectivity_xml::RouteFrameChoice::World(_) => true,
            connectivity_xml::RouteFrameChoice::ComponentOrigin(frame) => {
                frame.instance.is_none() && roots.contains(&frame.component)
            }
            connectivity_xml::RouteFrameChoice::ComponentFrame(_) => false,
        };
        if !moves_with_include {
            return;
        }
        let (rpy, quat) = match &placement.rotation.rotation {
            connectivity_xml::PlacementRotationChoice::Rpy(rotation) => {
                (Some(rotation.value), None)
            }
            connectivity_xml::PlacementRotationChoice::Quaternion(rotation) => {
                (None, Some(rotation.value))
            }
        };
        let pose = Pose {
            xyz: Some(placement.xyz),
            rpy,
            quat,
        };
        let transformed = matrix_to_pose(&mul44(transform, &pose_to_matrix(&pose)));
        placement.xyz = transformed.xyz_or_zero();
        placement.rotation = connectivity_xml::PlacementRotation {
            rotation: connectivity_xml::PlacementRotationChoice::Rpy(
                connectivity_xml::RpyRotation {
                    value: transformed.rpy_or_zero(),
                },
            ),
        };
    };

    match &mut representation.variant {
        connectivity_xml::RepresentationChoice::Box(value) => {
            offset_placement(&mut value.placement);
        }
        connectivity_xml::RepresentationChoice::Cylinder(value) => {
            offset_placement(&mut value.placement);
        }
        connectivity_xml::RepresentationChoice::Sphere(value) => {
            offset_placement(&mut value.placement);
        }
        connectivity_xml::RepresentationChoice::Model(value) => {
            offset_placement(&mut value.placement);
        }
        connectivity_xml::RepresentationChoice::ModelPart(_) => {}
        connectivity_xml::RepresentationChoice::DerivedRoute(route) => {
            for waypoint in &mut route.waypoint {
                let moves_with_include = match &waypoint.frame.frame {
                    connectivity_xml::RouteFrameChoice::World(_) => true,
                    connectivity_xml::RouteFrameChoice::ComponentOrigin(frame) => {
                        frame.instance.is_none() && roots.contains(&frame.component)
                    }
                    connectivity_xml::RouteFrameChoice::ComponentFrame(_) => false,
                };
                if moves_with_include {
                    if let Some(rotation) = waypoint.rotation.as_ref() {
                        let (rpy, quat) = match &rotation.rotation {
                            connectivity_xml::PlacementRotationChoice::Rpy(rotation) => {
                                (Some(rotation.value), None)
                            }
                            connectivity_xml::PlacementRotationChoice::Quaternion(rotation) => {
                                (None, Some(rotation.value))
                            }
                        };
                        let pose = Pose {
                            xyz: Some(waypoint.xyz),
                            rpy,
                            quat,
                        };
                        let transformed = matrix_to_pose(&mul44(transform, &pose_to_matrix(&pose)));
                        waypoint.xyz = transformed.xyz_or_zero();
                        waypoint.rotation = Some(connectivity_xml::PlacementRotation {
                            rotation: connectivity_xml::PlacementRotationChoice::Rpy(
                                connectivity_xml::RpyRotation {
                                    value: transformed.rpy_or_zero(),
                                },
                            ),
                        });
                    } else {
                        waypoint.xyz = transform_point(transform, waypoint.xyz);
                    }
                }
            }
        }
    }
}

/// Rigidly offset the sub-assembly by `pose_str` ("x y z r p y", XYZ-extrinsic rpy). Matches
/// `include.py::_offset`: premultiply `M_off` onto every ROOT comp's geometry/frames and the origins
/// of non-loop joints leaving a root, emitting rpy (clearing quat).
fn offset(sub: &mut Hcdf, pose_str: &str) {
    let m_off = offset_matrix(pose_str);

    // roots = comps that are NOT a non-loop joint child.
    let children: HashSet<String> = sub
        .joint
        .iter()
        .filter(|j| j.loop_.is_none())
        .filter_map(|j| j.child.as_ref().and_then(|e| e.comp.clone()))
        .collect();
    let roots: HashSet<String> = sub
        .comp
        .iter()
        .map(|c| c.name.clone())
        .filter(|n| !children.contains(n))
        .collect();

    // comp(pose): None -> the offset itself; Some -> M_off @ pose, both emitted as rpy.
    let apply = |pose: &Option<Pose>| -> Option<Pose> {
        let m = match pose {
            None => m_off,
            Some(pose) => mul44(&m_off, &pose_to_matrix(pose)),
        };
        Some(matrix_to_pose(&m))
    };

    for c in sub.comp.iter_mut() {
        if !roots.contains(&c.name) {
            continue;
        }
        for v in c.visual.iter_mut() {
            v.pose = apply(&v.pose);
        }
        for col in c.collision.iter_mut() {
            col.pose = apply(&col.pose);
        }
        if let Some(inertial) = c.inertial.as_mut() {
            inertial.inertia_origin = apply(&inertial.inertia_origin);
        }
        for f in c.frame.iter_mut() {
            f.pose = apply(&f.pose);
        }
    }
    for j in sub.joint.iter_mut() {
        let parent_is_root = j.loop_.is_none()
            && j.parent
                .as_ref()
                .and_then(|e| e.comp.as_deref())
                .is_some_and(|c| roots.contains(c));
        if parent_is_root {
            j.origin = apply(&j.origin);
        }
    }

    visit_connectivity_representations_mut(sub, &mut |representation| {
        offset_connectivity_representation(representation, &m_off, &roots);
    });
}

/// Build the offset transform from a pose string ("x y z" or "x y z r p y"), mirroring
/// `include.py::_offset`'s `M.Pose()` construction (xyz then rpy; quat unused for the offset).
fn offset_matrix(pose_str: &str) -> Mat4 {
    let vals: Vec<f64> = pose_str
        .split_whitespace()
        .filter_map(|v| v.parse::<f64>().ok())
        .collect();
    let mut xyz = [0.0; 3];
    if vals.len() >= 3 {
        xyz.copy_from_slice(&vals[0..3]);
    }
    let mut rpy = [0.0; 3];
    if vals.len() >= 6 {
        rpy.copy_from_slice(&vals[3..6]);
    }
    let pose = Pose {
        xyz: Some(xyz),
        rpy: Some(rpy),
        quat: None,
    };
    pose_to_matrix(&pose)
}

// ── native filesystem conveniences ────────────────────────────────────────────

/// Flatten `doc` by reading each include from `<base>/<uri>` via [`std::fs`], recursing relative to
/// each file's own directory. Native-only convenience over [`flatten_with`].
///
/// Returns the accumulated notes, or `Err` on a true include cycle. A not-found / unparsable include
/// is kept in place with a note (matching `include.py`).
#[cfg(not(target_arch = "wasm32"))]
pub fn flatten(doc: &mut Hcdf, base_dir: &Path) -> Result<Vec<String>, String> {
    flatten_with_stack(doc, base_dir, &mut fs_loader, Vec::new())
}

/// The native filesystem loader: the resolved key is an absolute path; read + parse it, and report the
/// source bytes' `"sha256:<hex>"` so the resolver can verify a pinned include `@sha`. A missing file is
/// an `Err` (kept-in-place by the resolver, like `include.py`'s existence check). The sha is taken over
/// the RAW file bytes (matching `hcdf.assets.file_sha`), read independently of the UTF-8 parse so it is
/// the byte-exact content hash, identical to what Python `file_sha` produces on the same file.
#[cfg(not(target_arch = "wasm32"))]
fn fs_loader(key: &str, _base: &Path) -> Result<(Hcdf, Option<String>), String> {
    let path = Path::new(key);
    if !path.exists() {
        return Err(format!("file not found at {key:?}"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("read {key:?}: {e}"))?;
    let sha = content_sha(&bytes);
    let xml = std::str::from_utf8(&bytes).map_err(|e| format!("read {key:?}: {e}"))?;
    let doc = Hcdf::from_xml_str(xml).map_err(|e| format!("parse {key:?}: {e}"))?;
    Ok((doc, Some(sha)))
}

/// Load the HCDF at `path`, flatten its includes (relative to its own directory), and return the
/// flattened document with notes. Native-only convenience.
///
/// Mirrors `include.py::flatten(path)`: the root file's own path seeds the cycle stack, so a sub-module
/// that includes the root file back is detected as a cycle.
#[cfg(not(target_arch = "wasm32"))]
pub fn flatten_path(path: &Path) -> Result<(Hcdf, Vec<String>), String> {
    let path = normalize(path);
    let xml = std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?;
    let mut doc = Hcdf::from_xml_str(&xml).map_err(|e| format!("parse {path:?}: {e}"))?;
    let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let root_key = path.to_string_lossy().into_owned();
    let notes = flatten_with_stack(&mut doc, &base, &mut fs_loader, vec![root_key])?;
    Ok((doc, notes))
}

/// Populate/update every `<include>`'s `@sha` in `doc` to its module's CURRENT content sha, in place:
/// the structure-PRESERVING dual of [`flatten`] (the includes stay; nothing is inlined). Parity with
/// `include.py::stamp_include_shas`: each include's module is read relative to ITS OWN directory and
/// hashed over the RAW bytes (so the stamped value equals what Python `file_sha` produces on the same
/// file), then the resolver recurses into the module relative to its own dir. Native-only (it reads the
/// filesystem); used to PIN a doc before a reproducible bundle.
///
/// Returns the per-include notes (one `"stamped include …"` per pin, a `"file not found …"` note for a
/// missing module, which is NOT stamped), or `Err` only on an I/O error reading an EXISTING module.
///
/// Repeat includes of one module (same resolved key) are stamped from a per-call memo: the module is
/// read, hashed, and descended ONCE, and later sites reuse the sha and REPLAY the first descent's notes;
/// a subtree's notes depend only on the module file and its directory (both captured by the key), so
/// the note stream (which drives dendrite's "pinned N include sha(s)" count) is identical to an
/// unmemoized walk.
#[cfg(not(target_arch = "wasm32"))]
pub fn stamp_include_shas(doc: &mut Hcdf, base_dir: &Path) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    // Empty initial stack mirrors `include.py::stamp_include_shas` when handed an `Hcdf` (in-memory):
    // the root doc has no path identity, so a self-including module is caught on RE-ENTRY of its own
    // resolved key, exactly as `flatten_with` does for the in-memory flatten.
    let mut stack: Vec<String> = Vec::new();
    let mut memo: HashMap<String, StampedModule> = HashMap::new();
    stamp(doc, base_dir, &mut notes, &mut stack, &mut memo)?;
    Ok(notes)
}

/// Per-call memo entry for one resolved module key ([`stamp`]): the module's content sha plus the notes
/// its subtree descent produced on FIRST visit, replayed verbatim at every later site so the reported
/// note stream matches an unmemoized walk exactly.
#[cfg(not(target_arch = "wasm32"))]
struct StampedModule {
    sha: String,
    subtree_notes: Vec<String>,
}

/// Recursive worker for [`stamp_include_shas`]: stamp each `<include>` in `doc` against the module
/// resolved relative to `base`, then recurse into the (freshly read) module relative to its own dir.
///
/// Carries the same cycle `stack` (resolved keys currently being stamped) as [`resolve`], so a
/// self-/mutually-including module returns a clean cycle `Err`, keeping stamp's failure semantics
/// identical to flatten's ("a cycle is the only Err") instead of overflowing the stack. The dendrite
/// "Pin includes" button maps this `Err` to a reported message rather than aborting the process.
#[cfg(not(target_arch = "wasm32"))]
fn stamp(
    doc: &mut Hcdf,
    base: &Path,
    notes: &mut Vec<String>,
    stack: &mut Vec<String>,
    memo: &mut HashMap<String, StampedModule>,
) -> Result<(), String> {
    for inc in doc.include.iter_mut() {
        let Some(uri) = inc.uri.clone() else { continue };
        let key = resolve_key(&uri, base);
        let key_str = key.to_string_lossy().into_owned();
        // Repeat include of an already-stamped module: pin from the memo and replay its subtree notes,
        // no re-read, re-hash, or re-descent. (A memoized key can never be on the cycle stack: its
        // entry is only inserted after its own descent completed.)
        if let Some(cached) = memo.get(&key_str) {
            notes.push(format!(
                "stamped include {uri:?}: sha {}",
                short_sha(&cached.sha)
            ));
            notes.extend(cached.subtree_notes.iter().cloned());
            inc.sha = Some(cached.sha.clone());
            continue;
        }
        if !key.exists() {
            notes.push(format!(
                "include {uri:?}: file not found at {key:?}; sha not stamped"
            ));
            continue;
        }
        if stack.iter().any(|k| k == &key_str) {
            let mut chain = stack.clone();
            chain.push(key_str);
            return Err(format!("include cycle detected: {}", chain.join(" -> ")));
        }
        let bytes = std::fs::read(&key).map_err(|e| format!("read {key:?}: {e}"))?;
        let sha = content_sha(&bytes);
        notes.push(format!("stamped include {uri:?}: sha {}", short_sha(&sha)));
        inc.sha = Some(sha.clone());
        // Recurse relative to the MODULE's own dir, mirroring `flatten`'s per-file base resolution. The
        // nested stamps land on a throwaway sub-doc (the module file is not rewritten here); like the
        // Python helper, only the parent doc's direct includes persist; the recursion surfaces notes,
        // captured separately here so a later repeat include can replay them.
        let mut subtree_notes = Vec::new();
        if let Ok(xml) = std::str::from_utf8(&bytes) {
            if let Ok(mut sub) = Hcdf::from_xml_str(xml) {
                let sub_base = key.parent().map(Path::to_path_buf).unwrap_or_default();
                stack.push(key_str.clone());
                let res = stamp(&mut sub, &sub_base, &mut subtree_notes, stack, memo);
                stack.pop();
                notes.extend(subtree_notes.iter().cloned());
                res?;
            }
        }
        memo.insert(key_str, StampedModule { sha, subtree_notes });
    }
    Ok(())
}

// ── remote-include-fetching flatten (feature = "remote", native-only) ─────────────────────────────
//
// The filesystem [`flatten`]/[`flatten_path`] above resolve LOCAL `<include>`s only. This block ports
// `include.py`'s `flatten(fetch=True)` path: a `http(s)://` `<include uri>` (or a RELATIVE include of a
// module that was itself loaded from a remote URL) is FETCHED via the hardened [`crate::remote::fetch_remote`]
// (allow-list / per-op timeout / size cap / content-addressed cache / `@sha` integrity), parsed, and
// composed (prefix / pose / merge) exactly like a local include. The reference semantics reproduced
// field-for-field:
//   * REMOTE BASE PROPAGATION: a remote module's own base is its REMOTE URL, so a nested RELATIVE include
//     or a relative asset URI is resolved by `urljoin`-ing it onto that URL (a browser's relative-ref join),
//     not against a filesystem dir; an absolute-or-remote sub-uri is followed as-is.
//   * FATAL-ON-FETCH-FAILURE: the integrator EXPLICITLY asked to embed the include (`--vendor-remote` =>
//     fetch), so ANY fetch failure (404 / network / size cap / `@sha` mismatch) is an `Err` that aborts the
//     flatten; it NEVER degrades to a surviving live `<include>` (which would let a "self-contained" bundle
//     phone home or carry an inert `@sha` pin). A LOCAL missing include still soft-notes + survives, as before.
//   * REROOT / PREFIX: identical to the local path (the same [`prefix`]/[`offset`]/[`merge`] helpers), with a
//     remote-aware asset reroot (`urljoin` onto the module URL) so a remote module's relative asset stays
//     fetchable by a later remote-aware vendor step.
//
// Gated `remote` + not-wasm so it never enters the default/wasm dependency tree (it needs `crate::remote`,
// itself behind the same gate). The wasm/default `flatten_with` core is untouched.
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub use remote_flatten::{flatten_fetch, flatten_path_fetch};

#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
mod remote_flatten {
    use super::{
        apply_parent_component_origin_adjustments, collapse_parent_instance_refs, content_sha,
        merge, normalize, offset, prefix, prefix_origin_adjustments,
        record_component_origin_adjustments, reject_nonzero_connectivity_occurrences,
        reroot_asset_uris_with, reroot_uri, short_sha, validate_include_siblings,
        ComponentOriginAdjustments, Hcdf, ParentConnectivityBoundary,
    };
    use crate::resource_path::{has_uri_scheme, split_suffix};
    use std::path::{Path, PathBuf};

    /// The base a document's relative uris resolve against: a filesystem DIR (a local file's directory) or
    /// a remote module URL. Mirrors `include.py`, where `base` is a string that is either a directory path
    /// or an `http(s)://` module URL and `_is_remote_base` chooses filesystem-join vs `urljoin`.
    enum FlattenBase {
        Dir(PathBuf),
        Url(String),
    }

    /// Flatten a `.hcdf` FILE on disk, FETCHING remote `<include>`s (parity with `include.py`
    /// `flatten(fetch=True)`). Local includes resolve off the filesystem exactly like [`super::flatten_path`];
    /// a `http(s)://` include (or a relative include of a remote module) is fetched + composed. `cache_dir`
    /// is where fetched bytes are content-addressed (`None` => the per-user default). Returns the flattened
    /// document + notes, or `Err` on a true cycle OR a FATAL remote-fetch failure.
    pub fn flatten_path_fetch(
        path: &Path,
        cache_dir: Option<&Path>,
    ) -> Result<(Hcdf, Vec<String>), String> {
        let path = normalize(path);
        let xml = std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?;
        let mut doc = Hcdf::from_xml_str(&xml).map_err(|e| format!("parse {path:?}: {e}"))?;
        let base = FlattenBase::Dir(path.parent().map(Path::to_path_buf).unwrap_or_default());
        // The root file's own path seeds the cycle stack (mirroring `flatten(path)` with `_stack=(top,)`),
        // so a sub-module that includes the root back is detected as a cycle.
        let mut stack = vec![path.to_string_lossy().into_owned()];
        let mut notes = Vec::new();
        let mut origin_adjustments = ComponentOriginAdjustments::new();
        resolve_fetch(
            &mut doc,
            &base,
            cache_dir,
            &mut notes,
            &mut stack,
            &mut origin_adjustments,
        )?;
        Ok((doc, notes))
    }

    /// Flatten an IN-MEMORY `doc`, FETCHING remote `<include>`s, resolving LOCAL includes off `base_dir`.
    /// The in-memory counterpart to [`flatten_path_fetch`] (see it for the fetch / reroot / fatal-on-fetch
    /// semantics): the document has no path identity, so the cycle stack starts EMPTY (a self-including
    /// module is caught on re-entry of its own resolved key), matching `include.py::flatten(Hcdf, fetch=True)`.
    /// Flattens `doc` in place and returns the notes, or `Err` on a true cycle or a fatal remote-fetch failure.
    pub fn flatten_fetch(
        doc: &mut Hcdf,
        base_dir: &Path,
        cache_dir: Option<&Path>,
    ) -> Result<Vec<String>, String> {
        let base = FlattenBase::Dir(base_dir.to_path_buf());
        let mut stack: Vec<String> = Vec::new();
        let mut notes = Vec::new();
        let mut working = doc.clone();
        let mut origin_adjustments = ComponentOriginAdjustments::new();
        resolve_fetch(
            &mut working,
            &base,
            cache_dir,
            &mut notes,
            &mut stack,
            &mut origin_adjustments,
        )?;
        *doc = working;
        Ok(notes)
    }

    /// Recursive worker mirroring `include.py::_resolve` (fetch arm). `stack` holds the resolved keys
    /// (abspaths for local, resolved URLs for remote) currently being resolved, for cycle detection.
    fn resolve_fetch(
        doc: &mut Hcdf,
        base: &FlattenBase,
        cache_dir: Option<&Path>,
        notes: &mut Vec<String>,
        stack: &mut Vec<String>,
        origin_adjustments: &mut ComponentOriginAdjustments,
    ) -> Result<(), String> {
        let parent_connectivity = ParentConnectivityBoundary::capture(doc);
        reject_nonzero_connectivity_occurrences(doc, parent_connectivity)?;
        validate_include_siblings(&doc.include)?;
        let pending = std::mem::take(&mut doc.include);
        for inc in pending {
            let uri = inc.uri.clone().ok_or_else(|| {
                format!(
                    "include {:?} is missing required @uri",
                    inc.name.as_deref().unwrap_or("")
                )
            })?;
            match load_include_fetch(&uri, base, &inc, cache_dir, notes, stack)? {
                None => {
                    // Unresolvable LOCAL include (missing file / left-unresolved remote note already
                    // recorded): keep it in place, exactly like `include.py`.
                    doc.include.push(inc);
                    continue;
                }
                Some((mut sub, sub_base, key)) => {
                    // Nested first: resolve the sub-document's own includes relative to ITS base.
                    stack.push(key);
                    let mut sub_origin_adjustments = ComponentOriginAdjustments::new();
                    let res = resolve_fetch(
                        &mut sub,
                        &sub_base,
                        cache_dir,
                        notes,
                        stack,
                        &mut sub_origin_adjustments,
                    );
                    stack.pop();
                    res?;
                    reroot_asset_uris_base(&mut sub, &sub_base);
                    if let (Some(d), Some(s)) = (doc.body_frame.as_ref(), sub.body_frame.as_ref()) {
                        if d != s {
                            notes.push(format!(
                                "include {uri:?}: body-frame {s} differs from including document {d} \
                                 (not converted; convention should match)"
                            ));
                        }
                    }
                    if let Some(name) = inc.name.as_deref() {
                        if !name.is_empty() {
                            prefix(&mut sub, name);
                            prefix_origin_adjustments(&mut sub_origin_adjustments, name);
                        }
                    }
                    if let Some(pose) = inc.pose.as_deref() {
                        if !pose.is_empty() {
                            record_component_origin_adjustments(
                                &sub,
                                pose,
                                &mut sub_origin_adjustments,
                            );
                            offset(&mut sub, pose);
                        }
                    }
                    apply_parent_component_origin_adjustments(
                        doc,
                        parent_connectivity,
                        &sub_origin_adjustments,
                    );
                    collapse_parent_instance_refs(doc, parent_connectivity, &sub)?;
                    merge(doc, sub);
                    origin_adjustments.extend(sub_origin_adjustments);
                    let mut note = format!("flattened include {uri:?}");
                    if let Some(name) = inc.name.as_deref() {
                        if !name.is_empty() {
                            note.push_str(&format!(" as {name:?}"));
                        }
                    }
                    if let Some(pose) = inc.pose.as_deref() {
                        if !pose.is_empty() {
                            note.push_str(&format!(" at pose {pose:?}"));
                        }
                    }
                    notes.push(note);
                }
            }
        }
        Ok(())
    }

    /// Load one `<include>` to `(sub_doc, sub_base, cycle_key)`, or `None` when it is left unresolved
    /// (a missing local file). Mirrors `include.py::_load_include` under `fetch=True`: a remote target is
    /// fetched (FATAL on failure); a local target is read + leniently `@sha`-checked (a mismatch is a NOTE).
    fn load_include_fetch(
        uri: &str,
        base: &FlattenBase,
        inc: &crate::model::Include,
        cache_dir: Option<&Path>,
        notes: &mut Vec<String>,
        stack: &[String],
    ) -> Result<Option<(Hcdf, FlattenBase, String)>, String> {
        use crate::remote::{fetch_remote, is_remote_uri};
        let uri_is_abs = Path::new(uri).is_absolute();
        let base_is_remote = matches!(base, FlattenBase::Url(_));
        // remote_target: an http(s) uri, OR a RELATIVE uri under a remote module base.
        let remote_target = is_remote_uri(uri) || (base_is_remote && !uri_is_abs);
        if remote_target {
            let sub_url = if is_remote_uri(uri) {
                uri.to_string()
            } else if let FlattenBase::Url(b) = base {
                urljoin(b, uri)
            } else {
                uri.to_string() // unreachable: remote_target with a non-remote base implies is_remote_uri
            };
            if stack.iter().any(|k| k == &sub_url) {
                let mut chain = stack.to_vec();
                chain.push(sub_url.clone());
                return Err(format!("include cycle detected: {}", chain.join(" -> ")));
            }
            // FATAL on any fetch failure (the integrator asked to embed it). `fetch_remote` verifies the
            // pinned `@sha` internally (expect_sha) and discards mismatched bytes.
            let pinned = inc.sha.as_deref().filter(|s| !s.is_empty());
            let sub_path = fetch_remote(&sub_url, pinned, cache_dir, None, None).map_err(|e| {
                format!(
                    "remote include {uri:?} (resolved {sub_url:?}) could not be fetched for embedding: {e}"
                )
            })?;
            if let Some(pinned) = pinned {
                notes.push(format!(
                    "include {uri:?}: remote @sha verified ({})",
                    short_sha(pinned)
                ));
            }
            let bytes = std::fs::read(&sub_path).map_err(|e| format!("read {sub_path:?}: {e}"))?;
            let xml = std::str::from_utf8(&bytes).map_err(|e| format!("{sub_path:?}: {e}"))?;
            let sub = Hcdf::from_xml_str(xml).map_err(|e| format!("parse {sub_path:?}: {e}"))?;
            // sub_base is the REMOTE URL so nested relative uris re-fetch against it.
            return Ok(Some((sub, FlattenBase::Url(sub_url.clone()), sub_url)));
        }

        // Local include: resolve against the filesystem dir (an absolute uri stands alone).
        let sub_path = if uri_is_abs {
            normalize(Path::new(uri))
        } else if let FlattenBase::Dir(dir) = base {
            normalize(&dir.join(uri))
        } else {
            normalize(Path::new(uri))
        };
        let key = sub_path.to_string_lossy().into_owned();
        if !sub_path.exists() {
            notes.push(format!(
                "include {uri:?}: file not found at {key:?}; left unresolved"
            ));
            return Ok(None);
        }
        if stack.iter().any(|k| k == &key) {
            let mut chain = stack.to_vec();
            chain.push(key.clone());
            return Err(format!("include cycle detected: {}", chain.join(" -> ")));
        }
        let bytes = std::fs::read(&sub_path).map_err(|e| format!("read {key:?}: {e}"))?;
        // Optional @sha verification (lenient: a mismatch is a NOTE, matching live-link editing).
        if let Some(pinned) = inc.sha.as_deref().filter(|s| !s.is_empty()) {
            let actual = content_sha(&bytes);
            if pinned != actual {
                notes.push(format!(
                    "include {uri:?}: sha mismatch (pinned {}, actual {})",
                    short_sha(pinned),
                    short_sha(&actual)
                ));
            }
        }
        let xml = std::str::from_utf8(&bytes).map_err(|e| format!("{key:?}: {e}"))?;
        let sub = Hcdf::from_xml_str(xml).map_err(|e| format!("parse {key:?}: {e}"))?;
        let sub_base = sub_path.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok(Some((sub, FlattenBase::Dir(sub_base), key)))
    }

    /// Rewrite every opaque asset URI in `sub` relative to `sub_base`. The local dir case reuses
    /// [`reroot_uri`]; the remote case joins a relative asset onto the module URL. A
    /// syntactically schemed URI resolves the same from any base and is left unchanged.
    fn reroot_asset_uris_base(sub: &mut Hcdf, sub_base: &FlattenBase) {
        reroot_asset_uris_with(sub, |uri| reroot_one(uri, sub_base));
    }

    /// One asset URI reroot dispatched by base kind (see [`reroot_asset_uris_base`]).
    fn reroot_one(uri: &str, sub_base: &FlattenBase) -> String {
        match sub_base {
            FlattenBase::Dir(dir) => reroot_uri(uri, dir),
            FlattenBase::Url(url) => {
                // Schemed URIs resolve the same from any base and remain unchanged. A plain relative
                // or absolute-local URI is anchored onto the module URL so it stays fetchable.
                if has_uri_scheme(uri) {
                    uri.to_string()
                } else {
                    urljoin(url, uri)
                }
            }
        }
    }

    /// Join a relative reference `rel` onto an absolute `http(s)://` `base`, reproducing
    /// `urllib.parse.urljoin` for the shapes HCDF modules produce (RFC 3986 §5.3 relative resolution with
    /// dot-segment removal). An absolute-scheme `rel` is returned unchanged.
    fn urljoin(base: &str, rel: &str) -> String {
        if has_uri_scheme(rel) {
            return rel.to_string();
        }
        if rel.is_empty() {
            return base.to_owned();
        }
        if rel.starts_with('#') {
            let base_without_fragment = base.split_once('#').map(|(head, _)| head).unwrap_or(base);
            return format!("{base_without_fragment}{rel}");
        }
        if rel.starts_with('?') {
            let (base_without_suffix, _) = split_suffix(base);
            return format!("{base_without_suffix}{rel}");
        }
        let Some((scheme, rest)) = base.split_once("://") else {
            return rel.to_string();
        };
        // Scheme-relative reference ("//host/path"): inherit the base scheme.
        if let Some(after) = rel.strip_prefix("//") {
            return format!("{scheme}://{after}");
        }
        let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..auth_end];
        let (base_path, _) = split_suffix(&rest[auth_end..]);
        let (rel_path, rel_tail) = split_suffix(rel);
        let merged = if rel_path.starts_with('/') {
            rel_path.to_string()
        } else if rel_path.is_empty() {
            base_path.to_string()
        } else if base_path.is_empty() {
            format!("/{rel_path}")
        } else {
            let cut = base_path.rfind('/').map_or("/", |i| &base_path[..=i]);
            format!("{cut}{rel_path}")
        };
        let resolved = remove_dot_segments(&merged);
        format!("{scheme}://{authority}{resolved}{rel_tail}")
    }

    /// RFC 3986 §5.2.4 dot-segment removal on a path (collapsing `.`/`..`), preserving a trailing slash.
    fn remove_dot_segments(path: &str) -> String {
        let absolute = path.starts_with('/');
        let mut out: Vec<&str> = Vec::new();
        for seg in path.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    out.pop();
                }
                s => out.push(s),
            }
        }
        let mut res = String::new();
        if absolute {
            res.push('/');
        }
        res.push_str(&out.join("/"));
        if (path.ends_with('/') || path.ends_with("/.") || path.ends_with("/.."))
            && !res.ends_with('/')
        {
            res.push('/');
        }
        res
    }

    #[cfg(test)]
    mod tests {
        use super::{remove_dot_segments, reroot_one, urljoin, FlattenBase};

        #[test]
        fn urljoin_matches_urllib() {
            assert_eq!(
                urljoin("http://h:8000/sub.hcdf", "assets/x.glb"),
                "http://h:8000/assets/x.glb"
            );
            assert_eq!(
                urljoin("http://h:8000/dir/sub.hcdf", "nested.hcdf"),
                "http://h:8000/dir/nested.hcdf"
            );
            assert_eq!(
                urljoin("http://h:8000/dir/sub.hcdf", "../other.hcdf"),
                "http://h:8000/other.hcdf"
            );
            assert_eq!(
                urljoin("http://h:8000/dir/sub.hcdf", "/abs.hcdf"),
                "http://h:8000/abs.hcdf"
            );
            assert_eq!(
                urljoin("https://h/dir/sub.hcdf", "//other/x"),
                "https://other/x"
            );
            assert_eq!(
                urljoin("https://h/dir/sub.hcdf?old=1#old", "?new=2#node"),
                "https://h/dir/sub.hcdf?new=2#node"
            );
            assert_eq!(
                urljoin("https://h/dir/sub.hcdf?old=1#old", "?new=2"),
                "https://h/dir/sub.hcdf?new=2"
            );
            assert_eq!(
                urljoin("https://h/dir/sub.hcdf?old=1#old", "#node"),
                "https://h/dir/sub.hcdf?old=1#node"
            );
            assert_eq!(
                urljoin("https://h/dir/sub.hcdf?old=1#old", ""),
                "https://h/dir/sub.hcdf?old=1#old"
            );
            assert_eq!(
                urljoin(
                    "https://h/dir/sub.hcdf",
                    "assets/model.glb?next=/../x#part/../y"
                ),
                "https://h/dir/assets/model.glb?next=/../x#part/../y"
            );
            assert_eq!(
                urljoin("http://h:8000/dir/sub.hcdf", "https://other/x"),
                "https://other/x"
            );
            assert_eq!(urljoin("http://h:8000/a/b/", ".."), "http://h:8000/a/");
        }

        #[test]
        fn remove_dot_segments_collapses() {
            assert_eq!(remove_dot_segments("/a/b/../c"), "/a/c");
            assert_eq!(remove_dot_segments("/a/./b"), "/a/b");
            assert_eq!(remove_dot_segments("/a/b/.."), "/a/");
        }

        #[test]
        fn remote_asset_reroot_preserves_schemes_and_suffix_bytes() {
            let base = FlattenBase::Url("https://host/dir/module.hcdf".to_owned());
            for uri in [
                "urn:asset:visual",
                "file:/C:/models/collision.stl",
                "custom+v1:sensor",
                "data:model/gltf-binary;base64,AAAA",
                "package://robot/connector.glb",
                "model://robot/body.glb",
                "https://example.com/body.glb",
            ] {
                assert_eq!(reroot_one(uri, &base), uri);
            }
            assert_eq!(
                reroot_one("assets/model.glb?next=/../x#part/../y", &base),
                "https://host/dir/assets/model.glb?next=/../x#part/../y"
            );
        }
    }
}

#[cfg(test)]
mod cycle_guard_tests {
    use super::*;

    /// A crafted keep-live-style cycle (every include resolves to a module that itself pins another
    /// include, under a growing lexical key so the key-based cycle guard never fires) must TERMINATE via
    /// the depth cap and REPORT the anomaly, never stack-overflow the verifier.
    #[test]
    fn verify_include_shas_depth_caps_a_module_cycle() {
        // Each resolved module carries one pinned include pointing "deeper"; its raw sha matches the pin,
        // so no per-level sha mismatch is produced; only the depth cap should fire.
        let sha = "sha256:deadbeef";
        let mut load = |_key: &str, _base: &Path| -> Result<(Hcdf, Option<String>), String> {
            let doc = Hcdf::from_xml_str(&format!(
                "<hcdf version=\"1.0\" name=\"m\"><include uri=\"deeper/m.hcdf\" sha=\"{sha}\"/></hcdf>"
            ))
            .unwrap();
            Ok((doc, Some(sha.to_string())))
        };
        let root = Hcdf::from_xml_str(&format!(
            "<hcdf version=\"1.0\" name=\"root\"><include uri=\"a/m.hcdf\" sha=\"{sha}\"/></hcdf>"
        ))
        .unwrap();

        // If the cap were missing this would recurse until it overflowed the stack.
        let out = verify_include_shas(&root, Path::new(""), &mut load);
        assert!(
            out.iter().any(|m| m
                .actual
                .as_deref()
                .is_some_and(|a| a.contains("nesting exceeds"))),
            "expected the depth cap to fire + be reported, got {out:?}"
        );
    }
}
