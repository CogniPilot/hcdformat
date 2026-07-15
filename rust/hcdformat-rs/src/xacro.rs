//! Expand xacro files to URDF (feature = `xacro`), then chain into [`from_urdf`](crate::from_urdf) for
//! `.xacro` import. A FULLY PURE-RUST port of `urdf_hcdf/xacro.py`, backed by the crates.io
//! [`xacro_pure`] crate: a faithful, byte-identical pure-Rust reimplementation of canonical ROS `xacro`.
//!
//! ## One pure-Rust backend for EVERY xacro class: no fork, no subprocess, no Python
//! [`xacro_pure`] expands BOTH the declarative SO-ARM class AND the programmatic OpenArm class
//! BYTE-IDENTICALLY to `/opt/ros/*/bin/xacro`, in pure Rust, on every target including wasm. It uses
//! `rustpython-vm` (a pure-Rust, wasm-clean CPython-grade interpreter) as the `${...}` expression engine,
//! so it has full Python `eval()` parity: the eager `lazy_eval="false"` self-referential properties,
//! `dict()`/string-concat/`load_yaml` inside ternaries, recursive macros, and dotted `load_yaml` access
//! the OpenArm robot drives its whole topology from. There is therefore NO canonical-`xacro` subprocess
//! fallback, NO `xacro-rs`/`pyisheval` fork, and NO Python anywhere in this path.
//!
//! ## The two-target seam
//! [`xacro_pure`] exposes the same DOM pipeline through two entry points:
//!   * **native** ([`xacro_pure::process_document`], `cfg(not(wasm32))`): a `std::fs`-backed include
//!     reader + a real `std::fs` `load_yaml` reader.
//!   * **any target incl. wasm** ([`xacro_pure::process_document_with`]): a CALLER-SUPPLIED include reader
//!     and `load_yaml` reader. On wasm these are backed by closures that read from whatever virtual FS the
//!     host wires up; here, the file-based [`expand_xacro`] uses `std::fs` closures on all targets so a
//!     `.xacro` laid out on disk expands identically native or wasm-in-a-test, while the string entry
//!     [`expand_xacro_str`] supplies closures that resolve includes/yaml relative to a caller-provided base.
//!
//! ## `$(find pkg)` resolution: the package map, then ament
//! `package_map` (`{pkg: /abs/path}`) makes `$(find pkg)` resolve to `path`. A `MapResolver` consults the
//! map first; for a package NOT in the map it falls back (on native) to [`xacro_pure::AmentPackageResolver`]
//! (walking `AMENT_PREFIX_PATH`, the same data `ament_index_python` indexes) so a `.xacro` in a sourced
//! workspace still resolves `$(find)` with no explicit map. On wasm there is no env/FS, so an unmapped
//! package is a clear error.
//!
//! ## Feature gating (rustpython stays out of non-`xacro` trees)
//! [`xacro_pure`] (and its `rustpython-vm` dependency, ~215 crates) is pulled ONLY by the `xacro` feature.
//! A bare model build, and hcdviz (`hcdformat = "1"`, no features), enable neither and so pull neither
//! xacro-pure nor rustpython. dendrite_build enables `xacro` (for in-browser SO-ARM **and OpenArm** import)
//! and pulls xacro-pure straight from crates.io with no extra Cargo plumbing (local xacro-pure development
//! can patch it to a sibling checkout via the gitignored `.cargo/config.toml`, see `.cargo/config.toml.example`).

use crate::{Error, Result};
use std::collections::HashMap;
use std::path::Path;

use xacro_pure::{
    process_document_with, FnIncludeReader, FnPackageResolver, PackageResolver, ProcessError,
};

/// A [`PackageResolver`] that resolves `$(find pkg)` from a caller-supplied `{pkg: path}` map first, then
/// (on native) falls back to [`xacro_pure::AmentPackageResolver`] (`AMENT_PREFIX_PATH`) for any package not
/// in the map. This preserves the prior behaviour: an explicit map wins, and an EMPTY map still resolves
/// `$(find)` for a `.xacro` laid out in a sourced ROS workspace. On wasm there is no env/FS, so an unmapped
/// package surfaces a clear "not found" error for the caller to route to a desktop conversion.
struct MapResolver {
    /// The explicit `{pkg: /abs/path}` map (the `package_map` argument).
    map: HashMap<String, String>,
}

impl PackageResolver for MapResolver {
    fn share_directory(&self, pkg: &str) -> std::result::Result<String, String> {
        if let Some(path) = self.map.get(pkg) {
            return Ok(path.clone());
        }
        self.fallback(pkg)
    }
}

impl MapResolver {
    /// Resolve a package NOT in the explicit map: on native via `AMENT_PREFIX_PATH`, on wasm a clear error
    /// (no env/FS). Split into a `cfg`-gated helper so each target has exactly one tail expression (no
    /// `return` and no dead-code lint on the unused branch).
    #[cfg(not(target_arch = "wasm32"))]
    fn fallback(&self, pkg: &str) -> std::result::Result<String, String> {
        xacro_pure::AmentPackageResolver.share_directory(pkg)
    }

    #[cfg(target_arch = "wasm32")]
    fn fallback(&self, pkg: &str) -> std::result::Result<String, String> {
        Err(format!(
            "package '{pkg}' not in the package map (no AMENT_PREFIX_PATH on wasm); pass it via the package map"
        ))
    }
}

/// Map a [`xacro_pure`] [`ProcessError`] into the crate error, naming xacro-pure so a divergence is
/// unambiguous (an unresolved `$(find)`, an unsupported construct, a parse/eval failure all surface here
/// rather than as silent partial output).
fn map_process_err(e: ProcessError) -> Error {
    Error::Xml(format!("xacro-pure failed to expand: {e}"))
}

/// Read a YAML file from disk for `xacro.load_yaml(path)`. Native + wasm-in-a-test both read through
/// `std::fs` (paths are absolute after `$(find)` expansion; a relative path is taken as-is). On a true wasm
/// host with no filesystem the caller would use [`expand_xacro_str`] / a custom pipeline instead.
fn read_yaml_fs(path: &str) -> std::result::Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

/// Read an included file from disk for `xacro:include`.
fn read_include_fs(path: &str) -> std::result::Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

/// Expand a xacro **file** to a URDF string with the FULLY PURE-RUST, wasm-clean [`xacro_pure`] engine,
/// matching canonical `xacro` byte-for-byte on BOTH the SO-ARM and OpenArm classes.
///
/// `package_map` (`{pkg: /abs/path}`) makes `$(find pkg)` resolve to `path`. With an empty map, `$(find)`
/// still resolves on native through `AMENT_PREFIX_PATH` (a sourced ROS workspace), so a xacro inside a
/// laid-out package expands hermetically. Includes and `load_yaml` are read from disk via `std::fs` (so a
/// `.xacro` on disk expands identically native or in a wasm test harness with a real FS).
pub fn expand_xacro(
    path: impl AsRef<std::path::Path>,
    package_map: &HashMap<String, String>,
) -> Result<String> {
    expand_xacro_with_args(path, package_map, &HashMap::new())
}

/// Expand a xacro **file** to a URDF string exactly as [`expand_xacro`], additionally seeding the
/// `$(arg name)` substitution table from `mappings` (the `name:=value` overrides canonical `xacro` takes
/// on the command line). Each entry overrides the matching `<xacro:arg>`'s declared default; an arg not in
/// `mappings` keeps its default, so an EMPTY `mappings` reproduces [`expand_xacro`] byte-for-byte.
pub fn expand_xacro_with_args(
    path: impl AsRef<std::path::Path>,
    package_map: &HashMap<String, String>,
    mappings: &HashMap<String, String>,
) -> Result<String> {
    let path = path.as_ref();
    let src = std::fs::read_to_string(path)
        .map_err(|e| Error::Xml(format!("{}: {e}", path.display())))?;
    let current = path.to_string_lossy().into_owned();
    expand_source(&src, Some(&current), package_map, mappings)
}

/// Expand a xacro **string** to a URDF string with [`xacro_pure`]. The in-memory entry point: there is no
/// top-level file on disk, so `<xacro:include>` (and `$(dirname)`) resolve relative to `.` and `load_yaml`/
/// includes still hit `std::fs` for any path they reference. Use [`expand_xacro`] when the document has an
/// on-disk location (for correct relative-include resolution).
pub fn expand_xacro_str(content: &str, package_map: &HashMap<String, String>) -> Result<String> {
    expand_source(content, None, package_map, &HashMap::new())
}

/// Expand a xacro **string** to a URDF string with CALLER-SUPPLIED readers, so an IN-MEMORY / virtual
/// filesystem (a browser folder-upload, a test map) can back `xacro:include`, `xacro.load_yaml`, and
/// `$(find pkg)`. FULLY PURE and wasm-clean: it touches NO `std::fs` and NO environment.
///
/// Unlike [`expand_xacro_str`] (whose readers are `std::fs`-backed and whose resolver falls back to
/// `AMENT_PREFIX_PATH`), this entry point is HERMETIC: `$(find pkg)` is resolved ONLY from `package_map`
/// (`{pkg: dir}`); an unmapped package is a clear error, not a host-dependent ament lookup, and includes
/// / yaml are read ONLY through the supplied closures. This makes a folder-upload import deterministic on
/// every target.
///
///   * `current_file`: the source document's (virtual) path (e.g. `my_robot/urdf/robot.xacro`). xacro-pure
///     resolves RELATIVE `<xacro:include filename="arm.xacro"/>` and `$(dirname)` against its directory, so
///     a sibling include resolves to the sibling key. Pass `None` for a basedir of `.`.
///   * `read_include`: called with the RESOLVED (already joined-against-the-basedir-or-`$(find)`) include
///     path; returns the included file's source, or an `Err` (treated as I/O, swallowed by `optional=true`).
///   * `read_yaml`: the `xacro.load_yaml(path)` reader (same contract). It is `Clone + 'static` because the
///     YAML seam is registered into the property environment; back it with an owned/`Arc`-captured map.
///
/// Globbing is intentionally NOT supported here (each spec resolves to itself): a virtual upload enumerates
/// concrete files, never directory globs. The resulting URDF string chains into
/// [`from_urdf_str`](crate::from_urdf::from_urdf_str) exactly as [`from_xacro_path`] does for a file.
pub fn expand_xacro_str_with<RI, RY>(
    content: &str,
    current_file: Option<&str>,
    package_map: &HashMap<String, String>,
    read_include: RI,
    read_yaml: RY,
) -> Result<String>
where
    RI: Fn(&str) -> std::result::Result<String, String>,
    RY: Fn(&str) -> std::result::Result<String, String> + Clone + 'static,
{
    let map = package_map.clone();
    let resolver: Box<dyn PackageResolver> = Box::new(FnPackageResolver(move |pkg: &str| {
        map.get(pkg).cloned().ok_or_else(|| {
            format!(
                "package '{pkg}' is not in the in-memory package map (declare it via a package.xml \
                 <name> or a matching folder name in the upload)"
            )
        })
    }));
    let reader = FnIncludeReader {
        read_fn: read_include,
        // No globbing on the virtual path: a folder-upload enumerates concrete files, so a spec is itself.
        glob_fn: |spec: &str| vec![spec.to_owned()],
    };
    process_document_with(
        content,
        current_file,
        HashMap::new(),
        resolver,
        read_yaml,
        &reader,
    )
    .map_err(map_process_err)
}

/// Shared core: expand `src` (with optional on-disk `current_file` for relative includes + `$(dirname)`)
/// through [`xacro_pure::process_document_with`], wiring the [`MapResolver`] for `$(find)`, `std::fs`
/// readers for includes + `load_yaml`, and `mappings` as the `$(arg)` override table (an empty map leaves
/// every `<xacro:arg>` at its declared default).
fn expand_source(
    src: &str,
    current_file: Option<&str>,
    package_map: &HashMap<String, String>,
    mappings: &HashMap<String, String>,
) -> Result<String> {
    let resolver: Box<dyn PackageResolver> = Box::new(MapResolver {
        map: package_map.clone(),
    });
    let reader = FnIncludeReader {
        read_fn: read_include_fs,
        glob_fn: glob_fs,
    };
    process_document_with(
        src,
        current_file,
        mappings.clone(),
        resolver,
        read_yaml_fs,
        &reader,
    )
    .map_err(map_process_err)
}

/// Native glob for `xacro:include` specs (the same `*`/`?`/`[...]` single-directory-level walk the corpus
/// uses), so a globbed include resolves identically to canonical. Kept here (rather than relying on the
/// crate's default no-op glob) so the file-based entry points glob against the real filesystem.
fn glob_fs(spec: &str) -> Vec<String> {
    let path = Path::new(spec);
    let (dir, pattern) = match (path.parent(), path.file_name()) {
        (Some(d), Some(f)) => (d.to_path_buf(), f.to_string_lossy().into_owned()),
        _ => return vec![spec.to_owned()],
    };
    if !(pattern.contains('*') || pattern.contains('?') || pattern.contains('[')) {
        return vec![spec.to_owned()];
    }
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir.as_path()
    };
    let mut out: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if fnmatch(&pattern, &name) {
                out.push(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    out.sort();
    out
}

/// Minimal `fnmatch` for `*` (any run), `?` (one char), `[...]` (a char class), sufficient for the
/// single-level `xacro:include` globs the corpus uses.
fn fnmatch(pattern: &str, name: &str) -> bool {
    fn rec(p: &[char], n: &[char]) -> bool {
        if p.is_empty() {
            return n.is_empty();
        }
        match p[0] {
            '*' => rec(&p[1..], n) || (!n.is_empty() && rec(p, &n[1..])),
            '?' => !n.is_empty() && rec(&p[1..], &n[1..]),
            '[' => {
                if n.is_empty() {
                    return false;
                }
                if let Some(close) = p.iter().position(|&c| c == ']') {
                    let class = &p[1..close];
                    char_in_class(class, n[0]) && rec(&p[close + 1..], &n[1..])
                } else {
                    !n.is_empty() && p[0] == n[0] && rec(&p[1..], &n[1..])
                }
            }
            c => !n.is_empty() && c == n[0] && rec(&p[1..], &n[1..]),
        }
    }
    let pc: Vec<char> = pattern.chars().collect();
    let nc: Vec<char> = name.chars().collect();
    rec(&pc, &nc)
}

/// Whether `c` is in the bracket class `class` (supporting `a-z` ranges and a leading `!`/`^` negation).
fn char_in_class(class: &[char], c: char) -> bool {
    let (negate, class) = match class.first() {
        Some('!') | Some('^') => (true, &class[1..]),
        _ => (false, class),
    };
    let mut i = 0;
    let mut found = false;
    while i < class.len() {
        if i + 2 < class.len() && class[i + 1] == '-' {
            if class[i] <= c && c <= class[i + 2] {
                found = true;
            }
            i += 3;
        } else {
            if class[i] == c {
                found = true;
            }
            i += 1;
        }
    }
    found != negate
}

// ── .xacro import chain: expand -> from_urdf ─────────────────────────────────────────────────────────

/// Import a xacro **file** into an [`Hcdf`](crate::Hcdf) model on any target (including wasm): expand with
/// the pure-Rust [`xacro_pure`] engine, then chain into [`from_urdf_str`](crate::from_urdf::from_urdf_str).
pub fn from_xacro_path(
    path: impl AsRef<std::path::Path>,
    package_map: &HashMap<String, String>,
) -> Result<(crate::Hcdf, Vec<String>)> {
    let urdf = expand_xacro(path, package_map)?;
    crate::from_urdf::from_urdf_str(&urdf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnmatch_basic() {
        assert!(fnmatch("*.xacro", "a.xacro"));
        assert!(fnmatch("a?b", "axb"));
        assert!(!fnmatch("a?b", "ab"));
        assert!(fnmatch("a[0-9]b", "a5b"));
        assert!(!fnmatch("a[0-9]b", "axb"));
        assert!(!fnmatch("*.xacro", "a.urdf"));
    }

    /// A self-contained xacro (no `$(find)`, no include, no load_yaml) expands in pure Rust through the
    /// string entry point, exercising the seam with zero external dependencies.
    #[test]
    fn expand_str_minimal() {
        let src = r#"<?xml version="1.0"?>
<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="t">
  <xacro:property name="r" value="2"/>
  <link name="L_${r}"/>
</robot>
"#;
        let urdf = expand_xacro_str(src, &HashMap::new()).expect("expand minimal xacro");
        assert!(urdf.contains("L_2"), "property not expanded: {urdf}");
        assert!(!urdf.contains("xacro:"), "xacro markup survived: {urdf}");
    }

    /// A caller `mappings` entry overrides a `<xacro:arg>`'s declared default, so the same file expands to
    /// DIFFERENT bytes with and without the override, while an empty mapping reproduces the default: the
    /// file-based args entry point threads the substitution table all the way through to expansion.
    #[test]
    fn expand_file_with_args_overrides_default() {
        use std::io::Write;
        let src = r#"<?xml version="1.0"?>
<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="t">
  <xacro:arg name="side" default="left"/>
  <link name="$(arg side)_wheel"/>
</robot>
"#;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("hcdf-xacro-args-{nonce}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("robot.xacro");
        std::fs::File::create(&path)
            .and_then(|mut f| f.write_all(src.as_bytes()))
            .expect("write xacro fixture");

        let default = expand_xacro(&path, &HashMap::new()).expect("expand with defaults");
        assert!(
            default.contains("left_wheel"),
            "default arg not applied: {default}"
        );

        let mut args = HashMap::new();
        args.insert("side".to_string(), "right".to_string());
        let overridden =
            expand_xacro_with_args(&path, &HashMap::new(), &args).expect("expand with override");
        assert!(
            overridden.contains("right_wheel"),
            "override not applied: {overridden}"
        );
        assert!(
            !overridden.contains("left_wheel"),
            "default leaked past override: {overridden}"
        );

        // An empty mapping reproduces the no-args expansion byte-for-byte.
        let empty =
            expand_xacro_with_args(&path, &HashMap::new(), &HashMap::new()).expect("expand empty");
        assert_eq!(
            empty, default,
            "empty mapping changed the default expansion"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// [`expand_xacro_str_with`] backs `xacro:include` AND `$(find pkg)` from a CALLER map (no `std::fs`,
    /// no env): the browser folder-upload seam. A relative include resolves against `current_file`'s dir;
    /// `$(find my_robot)` resolves ONLY from the package map (an unmapped package is an error, not ament).
    ///
    /// NOTE the synthetic-ABSOLUTE-root convention (leading `/`): xacro-pure prepends the current file's
    /// dir to any RELATIVE include spec (canonical behaviour). A `$(find pkg)` that resolved to a relative
    /// virtual dir would therefore be re-joined and double-nested. Mapping the package to an ABSOLUTE
    /// virtual path (`/my_robot`), and keying the in-memory FS under that same absolute root, keeps the
    /// resolved include path absolute, so it is read verbatim. The dendrite folder-importer uses the same
    /// `/`-rooted convention for exactly this reason.
    #[test]
    fn expand_str_with_in_memory_readers() {
        let mut files: HashMap<String, String> = HashMap::new();
        // The package's arm fragment, referenced via $(find my_robot).
        files.insert(
            "/my_robot/urdf/arm.xacro".to_string(),
            r#"<robot xmlns:xacro="http://www.ros.org/wiki/xacro"><link name="arm"/></robot>"#
                .to_string(),
        );
        // A sibling fragment, referenced relative to the root file's dir.
        files.insert(
            "/my_robot/urdf/base.xacro".to_string(),
            r#"<robot xmlns:xacro="http://www.ros.org/wiki/xacro"><link name="base"/></robot>"#
                .to_string(),
        );
        let root = r#"<?xml version="1.0"?>
<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="t">
  <xacro:include filename="base.xacro"/>
  <xacro:include filename="$(find my_robot)/urdf/arm.xacro"/>
</robot>
"#;
        let mut package_map = HashMap::new();
        package_map.insert("my_robot".to_string(), "/my_robot".to_string());

        let reader_files = files.clone();
        let urdf = expand_xacro_str_with(
            root,
            Some("/my_robot/urdf/robot.xacro"),
            &package_map,
            |path: &str| {
                reader_files
                    .get(path)
                    .cloned()
                    .ok_or_else(|| format!("no such in-memory file: {path}"))
            },
            |_p: &str| Err("no yaml in this test".to_string()),
        )
        .expect("expand with in-memory readers");

        assert!(
            urdf.contains(r#"name="base""#),
            "relative include not pulled: {urdf}"
        );
        assert!(
            urdf.contains(r#"name="arm""#),
            "$(find) include not pulled: {urdf}"
        );

        // An unmapped package is a hermetic error (no ament fallback on this path).
        let err = expand_xacro_str_with(
            r#"<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="t"><xacro:include filename="$(find nope)/x.xacro"/></robot>"#,
            None,
            &HashMap::new(),
            |_p: &str| Err("unused".to_string()),
            |_p: &str| Err("unused".to_string()),
        )
        .expect_err("an unmapped $(find) package must error");
        assert!(
            format!("{err}").contains("nope"),
            "error names the package: {err}"
        );
    }
}
