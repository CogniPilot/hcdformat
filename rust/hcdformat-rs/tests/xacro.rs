//! XACRO PARITY harness: the FULLY PURE-RUST `xacro-pure`-backed [`hcdformat::expand_xacro`] vs the
//! canonical `xacro` CLI (the authority `urdf_hcdf/xacro.py` shells to).
//!
//! For each corpus xacro this test runs canonical `xacro <file>` (with a throwaway ament index so
//! `$(find)` resolves, exactly like `xacro.py`) AND Rust [`hcdformat::expand_xacro`], parses BOTH URDFs,
//! and compares at the SEMANTIC bar:
//!   * the top-level `<robot>/<link>` set: count + every name (so no link is dropped/fabricated);
//!   * the top-level `<robot>/<joint>` set: count + every (name, type) (topology + joint types);
//!   * every mesh URI (`<mesh filename>`), as a sorted multiset, preserved VERBATIM (not path-resolved);
//!   * every numeric attribute/text value across the whole document, as an order-independent multiset.
//!
//! Unlike the previous (xacro-rs) harness, `xacro-pure` is BYTE-IDENTICAL to canonical, so this could
//! compare bytes, but the semantic, order-independent bar is kept so a benign serializer/float-spelling
//! difference (should one ever appear) does not mask a real topology regression.
//!
//!   * **SO-ARM101 / SO-ARM100** (declarative class) match canonical via PURE-RUST [`hcdformat::expand_xacro`].
//!   * **OpenArm v2.0** (the "programmatic" class: eager self-referential `lazy_eval="false"` properties,
//!     `dict()`/concat/`load_yaml` inside ternaries, recursive macros, dotted `load_yaml`) ALSO matches
//!     canonical via the SAME pure-Rust [`hcdformat::expand_xacro`], NO subprocess fallback. This is the
//!     payoff of the `xacro-pure` port: the class no Rust xacro crate could do now expands in pure Rust.
//!
//! Plus a `convert .xacro -> .hcdf` smoke test through [`hcdformat::from_xacro_path`], and a source-grep
//! assertion that NO `Command::new` / `xacro` subprocess / Python remains anywhere in the xacro path.
//!
//! Skips cleanly (with a logged reason) when canonical `xacro` or the corpus is unavailable; in the dev
//! environment canonical `xacro` is at `/opt/ros/jazzy/bin/xacro`.
//!
//! Only built with the `xacro` feature and never on wasm: the harness shells to the canonical CLI and
//! builds unix-symlink ament prefixes ([`hcdformat::from_xacro_path`] itself runs on every target).

#![cfg(all(feature = "xacro", not(target_arch = "wasm32")))]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A process-wide monotonic counter for collision-free temp paths across parallel test threads.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_tmp(label: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("hcdf_xacro_{}_{n}_{label}", std::process::id()))
}

/// Resolve the canonical `xacro` CLI: `$XACRO_BIN`, `PATH`, then `/opt/ros/*/bin/xacro`. `None` to skip.
fn canonical_xacro() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("XACRO_BIN") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join("xacro");
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    let mut ros: Vec<PathBuf> = std::fs::read_dir("/opt/ros")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("bin/xacro"))
        .filter(|p| p.is_file())
        .collect();
    ros.sort();
    ros.pop()
}

/// A corpus xacro to drive parity over: its file, and the `{pkg: path}` map for `$(find)`.
struct Case {
    label: &'static str,
    file: PathBuf,
    packages: BTreeMap<String, PathBuf>,
}

/// The corpus xacros (present-only): so_arm101, so_arm100 (the declarative class), openarm_v20 (programmatic).
fn corpus() -> Vec<Case> {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let ex = home.join("git/hcdf-conversion-examples");
    let mut cases = Vec::new();

    let so101_pkg = ex.join("ros2_so_arm/so_arm101_description");
    cases.push(Case {
        label: "so_arm101",
        file: so101_pkg.join("urdf/so_arm101.urdf.xacro"),
        packages: BTreeMap::from([("so_arm101_description".to_string(), so101_pkg)]),
    });

    let so100_pkg = ex.join("ros2_so_arm/so_arm100_description");
    cases.push(Case {
        label: "so_arm100",
        file: so100_pkg.join("urdf/so_arm100.urdf.xacro"),
        packages: BTreeMap::from([("so_arm100_description".to_string(), so100_pkg)]),
    });

    let openarm_pkg = ex.join("openarm_description");
    cases.push(Case {
        label: "openarm_v20",
        file: openarm_pkg.join("assets/robot/openarm_v2.0/urdf/openarm_v20.urdf.xacro"),
        packages: BTreeMap::from([("openarm_description".to_string(), openarm_pkg)]),
    });

    cases.into_iter().filter(|c| c.file.is_file()).collect()
}

/// Build a throwaway ament prefix exposing `{pkg: path}` so canonical `$(find pkg)` resolves, a port of
/// `xacro.py::_ament_prefix`. Returns the prefix dir (removed by the caller).
fn ament_prefix(packages: &BTreeMap<String, PathBuf>) -> PathBuf {
    let prefix = unique_tmp("ament");
    let markers = prefix.join("share/ament_index/resource_index/packages");
    std::fs::create_dir_all(&markers).expect("mkdir ament markers");
    for (name, path) in packages {
        std::fs::write(markers.join(name), b"").expect("write marker");
        let link = prefix.join("share").join(name);
        let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        std::os::unix::fs::symlink(&abs, &link).expect("symlink pkg");
    }
    prefix
}

/// Run canonical `xacro <file>` with a throwaway ament index for `$(find)`. Returns the URDF, or `None`.
fn run_canonical(bin: &Path, case: &Case) -> Option<String> {
    let prefix = ament_prefix(&case.packages);
    let existing = std::env::var_os("AMENT_PREFIX_PATH");
    let mut ament = prefix.clone().into_os_string();
    if let Some(e) = &existing {
        ament.push(":");
        ament.push(e);
    }
    let out = std::process::Command::new(bin)
        .arg(&case.file)
        .env("AMENT_PREFIX_PATH", ament)
        .output();
    let _ = std::fs::remove_dir_all(&prefix);
    let out = out.ok()?;
    if !out.status.success() {
        eprintln!(
            "[skip] canonical xacro failed on {}: {}",
            case.label,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ── parsing helpers (semantic bar; order-independent) ────────────────────────────────────────────────

/// The top-level `<robot>/<link name>` set (count via the returned Vec length; names sorted).
fn top_level_links(xml: &str) -> Vec<String> {
    children_of_robot(xml, "link", |e| attr(e, "name"))
}

/// The top-level `<robot>/<joint>` set as sorted `(name, type)` pairs.
fn top_level_joints(xml: &str) -> Vec<(String, String)> {
    children_of_robot(xml, "joint", |e| {
        attr(e, "name").map(|n| (n, attr(e, "type").unwrap_or_default()))
    })
}

/// Collect a projection `f` over every DIRECT `<robot>` child named `tag` (depth-1 only, so `<joint>`s
/// nested inside `<ros2_control>`/`<transmission>` extension blocks are excluded). Results sorted.
fn children_of_robot<T: Ord, F: Fn(&quick_xml::events::BytesStart) -> Option<T>>(
    xml: &str,
    tag: &str,
    f: F,
) -> Vec<T> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(xml);
    let mut depth = 0i32;
    let mut out = Vec::new();
    loop {
        match reader.read_event().expect("well-formed URDF") {
            Event::Start(e) => {
                let local = local_name(&e);
                if depth == 1 && local == tag {
                    if let Some(v) = f(&e) {
                        out.push(v);
                    }
                }
                depth += 1;
            }
            Event::Empty(e) => {
                let local = local_name(&e);
                if depth == 1 && local == tag {
                    if let Some(v) = f(&e) {
                        out.push(v);
                    }
                }
            }
            Event::End(_) => depth -= 1,
            Event::Eof => break,
            _ => {}
        }
    }
    out.sort();
    out
}

/// Every mesh URI (`<mesh filename=...>` attribute) across the document, as a sorted multiset. Preserved
/// verbatim (`package://...` NOT path-resolved).
fn mesh_uris(xml: &str) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    loop {
        match reader.read_event().expect("well-formed URDF") {
            Event::Start(e) | Event::Empty(e) => {
                if local_name(&e) == "mesh" {
                    if let Some(uri) = attr(&e, "filename") {
                        out.push(uri);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out.sort();
    out
}

/// Every numeric value (whitespace-separated float tokens) in any attribute or text node, flattened and
/// sorted into one multiset. Comparing the SORTED multiset absorbs benign float-reformat spelling
/// (`5e-05` vs `0.00005`): checked for "same set of numbers", not positional alignment.
fn numeric_multiset(xml: &str) -> Vec<f64> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(xml);
    let mut out: Vec<f64> = Vec::new();
    let push_nums = |s: &str, out: &mut Vec<f64>| {
        let toks: Vec<&str> = s.split_whitespace().collect();
        if toks.is_empty() {
            return;
        }
        let parsed: Option<Vec<f64>> = toks.iter().map(|t| t.parse::<f64>().ok()).collect();
        if let Some(nums) = parsed {
            out.extend(nums);
        }
    };
    loop {
        match reader.read_event().expect("well-formed URDF") {
            Event::Start(e) | Event::Empty(e) => {
                for a in e.attributes().flatten() {
                    let val = String::from_utf8_lossy(&a.value).into_owned();
                    push_nums(&val, &mut out);
                }
            }
            Event::Text(t) => {
                let txt = t.unescape().unwrap_or_default().into_owned();
                push_nums(&txt, &mut out);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn local_name(e: &quick_xml::events::BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        if a.key.local_name().as_ref() == key.as_bytes() {
            Some(String::from_utf8_lossy(&a.value).into_owned())
        } else {
            None
        }
    })
}

/// Assert two float multisets are numerically equal (count + value, within a tiny tolerance for the
/// reformat round-trip). Reports the first divergence with context.
fn assert_numeric_parity(label: &str, canon: &[f64], rust: &[f64]) {
    assert_eq!(
        canon.len(),
        rust.len(),
        "[{label}] numeric field COUNT differs: canonical has {}, rust has {}",
        canon.len(),
        rust.len()
    );
    for (i, (c, r)) in canon.iter().zip(rust.iter()).enumerate() {
        let tol = 1e-9 * c.abs().max(1.0);
        assert!(
            (c - r).abs() <= tol,
            "[{label}] numeric value #{i} differs: canonical {c} vs rust {r}"
        );
    }
}

/// The `{pkg: /abs/path}` map the Rust expander wants for `$(find)`.
fn pkg_str_map(case: &Case) -> HashMap<String, String> {
    case.packages
        .iter()
        .map(|(k, v)| (k.clone(), v.to_string_lossy().into_owned()))
        .collect()
}

/// Assert `rust` URDF reproduces `canon` URDF at the full semantic bar (links, joints, mesh uris, numeric
/// fields). Returns the counts for the success log.
fn assert_semantic_parity(label: &str, canon: &str, rust: &str) -> (usize, usize, usize, usize) {
    let cl = top_level_links(canon);
    let rl = top_level_links(rust);
    assert_eq!(cl, rl, "[{label}] top-level <link> set differs");
    assert!(!cl.is_empty(), "[{label}] expected at least one link");

    let cj = top_level_joints(canon);
    let rj = top_level_joints(rust);
    assert_eq!(
        cj, rj,
        "[{label}] top-level <joint> set (name,type) differs"
    );

    let cm = mesh_uris(canon);
    let rm = mesh_uris(rust);
    assert_eq!(cm, rm, "[{label}] mesh URI set differs");

    let cn = numeric_multiset(canon);
    assert_numeric_parity(label, &cn, &numeric_multiset(rust));
    (cl.len(), cj.len(), cm.len(), cn.len())
}

// ── tests ────────────────────────────────────────────────────────────────────────────────────────────

/// The headline parity test: the declarative class (SO-ARM101 / SO-ARM100) expands via the PURE-RUST
/// `xacro-pure`-backed [`hcdformat::expand_xacro`] and matches canonical `xacro` at the full semantic bar.
/// OpenArm is covered by [`openarm_matches_canonical_pure_rust`].
#[test]
fn expand_xacro_matches_canonical_on_simple_corpus() {
    let Some(bin) = canonical_xacro() else {
        eprintln!("[skip] canonical xacro not found (set XACRO_BIN); cannot run parity");
        return;
    };
    let cases = corpus();
    if cases.is_empty() {
        eprintln!("[skip] no corpus xacros found under ~/git/hcdf-conversion-examples");
        return;
    }
    let mut exercised = 0;
    for case in cases.iter().filter(|c| c.label != "openarm_v20") {
        let Some(canon) = run_canonical(&bin, case) else {
            continue;
        };
        let rust = hcdformat::expand_xacro(&case.file, &pkg_str_map(case))
            .unwrap_or_else(|e| panic!("[{}] pure-Rust expand_xacro FAILED: {e}", case.label));
        let (l, j, m, n) = assert_semantic_parity(case.label, &canon, &rust);
        exercised += 1;
        eprintln!("[ok] {} (pure-Rust): {l} links, {j} joints, {m} mesh uris, {n} numeric fields match canonical", case.label);
    }
    assert!(
        exercised > 0,
        "no declarative-class corpus xacro was exercised"
    );
}

/// THE DECISIVE GATE: OpenArm v2.0 (the "programmatic" class) matches canonical `xacro` via the SAME
/// FULLY PURE-RUST [`hcdformat::expand_xacro`], NO subprocess fallback. This is the class every prior Rust
/// xacro crate (xacro-rs, xurdf) failed on: it drives its whole topology from data, with eager self-referential
/// `lazy_eval="false"` properties, `dict()`/string-concat/`load_yaml` inside ternaries, recursive macros,
/// dotted `load_yaml`. `xacro-pure` (rustpython-backed `${...}`) expands it in pure Rust, matching canonical.
#[test]
fn openarm_matches_canonical_pure_rust() {
    let Some(bin) = canonical_xacro() else {
        eprintln!("[skip] canonical xacro not found; cannot run OpenArm parity");
        return;
    };
    let cases = corpus();
    let Some(case) = cases.iter().find(|c| c.label == "openarm_v20") else {
        eprintln!("[skip] openarm_v20 xacro not in corpus");
        return;
    };
    let Some(canon) = run_canonical(&bin, case) else {
        return;
    };
    // PURE-RUST only, no fallback. OpenArm must expand AND match canonical entirely in Rust.
    let rust = hcdformat::expand_xacro(&case.file, &pkg_str_map(case))
        .unwrap_or_else(|e| panic!("[openarm_v20] pure-Rust expand_xacro FAILED: {e}"));
    let (l, j, m, n) = assert_semantic_parity(case.label, &canon, &rust);
    assert!(
        l >= 20,
        "[openarm_v20] expansion looks truncated: {l} links"
    );
    eprintln!("[ok] openarm_v20 (PURE-RUST): {l} links, {j} joints, {m} mesh uris, {n} numeric fields match canonical");
}

/// BYTE parity (the strongest bar): `xacro-pure` is a byte-identical port, so on BOTH classes the pure-Rust
/// expansion equals canonical `xacro` output verbatim. Gated on the canonical CLI; reports the first
/// differing line if they ever diverge.
#[test]
fn expand_xacro_byte_identical_to_canonical() {
    let Some(bin) = canonical_xacro() else {
        eprintln!("[skip] canonical xacro not found; cannot run byte parity");
        return;
    };
    let cases = corpus();
    if cases.is_empty() {
        eprintln!("[skip] no corpus xacros found");
        return;
    }
    let mut exercised = 0;
    for case in &cases {
        let Some(canon) = run_canonical(&bin, case) else {
            continue;
        };
        let rust = hcdformat::expand_xacro(&case.file, &pkg_str_map(case))
            .unwrap_or_else(|e| panic!("[{}] pure-Rust expand_xacro FAILED: {e}", case.label));
        if rust != canon {
            let cl: Vec<&str> = canon.lines().collect();
            let rl: Vec<&str> = rust.lines().collect();
            let first = cl
                .iter()
                .zip(rl.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(cl.len().min(rl.len()));
            panic!(
                "[{}] NOT byte-identical to canonical: canonical {} lines, rust {} lines; first diff at line {}:\ncanonical: {:?}\nrust:      {:?}",
                case.label,
                cl.len(),
                rl.len(),
                first + 1,
                cl.get(first),
                rl.get(first),
            );
        }
        exercised += 1;
        eprintln!(
            "[ok] {} (pure-Rust): BYTE-IDENTICAL to canonical xacro",
            case.label
        );
    }
    assert!(exercised > 0, "no corpus xacro was byte-compared");
}

/// A self-contained programmatic-class regression that does NOT need the canonical CLI: it drives the EXACT
/// OpenArm-style constructs (string concat, `dict(kwargs)`, `%`-format, slicing, NEGATIVE indexing, dotted
/// `load_yaml`, eager `lazy_eval="false"` self-reference, ternary-gated `load_yaml`) through the PURE-RUST
/// [`hcdformat::expand_xacro`] and asserts each result equals the CPython/canonical value. It locks the
/// expression-engine semantics even where the canonical CLI is absent (a bare CI, the wasm host runner).
#[test]
fn programmatic_constructs_match_cpython_pure_rust() {
    let dir = unique_tmp("programmatic");
    std::fs::create_dir_all(&dir).expect("mkdir programmatic dir");
    struct RmDir(PathBuf);
    impl Drop for RmDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = RmDir(dir.clone());

    // load_yaml target with a dotted-access shape (the YamlDictWrapper dot-attr OpenArm uses).
    let cfg_path = dir.join("cfg.yaml");
    std::fs::write(
        &cfg_path,
        "geometry:\n  mesh: arm.dae\njoint2:\n  multiplier: -1.0\n",
    )
    .expect("write cfg.yaml");
    let cfg_abs = cfg_path.to_str().expect("temp path is utf-8");
    // An ABSENT yaml planted in an UNTAKEN ternary branch: lazy semantics must never read it.
    let absent_abs = dir.join("definitely_absent.yaml");
    let absent_abs = absent_abs.to_str().expect("utf-8 temp path");

    let xacro = format!(
        r#"<?xml version="1.0"?>
<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="programmatic">
  <!-- string concat + concat chain (order load-bearing) -->
  <xacro:property name="root" value="/abs/openarm_v2.0"/>
  <xacro:property name="cfg_root" value="${{root + '/config'}}"/>
  <xacro:property name="preset" value="default_bimanual"/>
  <xacro:property name="preset_path" value="${{cfg_root + '/robot_presets/' + preset + '.yaml'}}"/>
  <link name="P1_${{cfg_root}}"/>
  <link name="P1chain_${{preset_path}}"/>

  <!-- dict(kwargs) + empty dict() -->
  <xacro:property name="d" value="${{dict(x=0.0, y=1.0, z=2.5)}}"/>
  <link name="P2_dict">
    <inertial><origin xyz="${{d['x']}} ${{d['y']}} ${{d['z']}}"/></inertial>
  </link>

  <!-- dotted load_yaml access -->
  <xacro:property name="ycfg" value="${{xacro.load_yaml('{cfg_abs}')}}"/>
  <link name="P3_${{ycfg.geometry.mesh}}"/>

  <!-- %-format: bare arg + multi-element tuple -->
  <link name="P4one_${{'attaches `%s`' % preset}}"/>
  <link name="P4two_${{'`%s` of %d' % (preset, 7)}}"/>

  <!-- slicing + NEGATIVE indexing -->
  <xacro:property name="comps" value="${{['c0','c1','c2','c3']}}"/>
  <link name="P5to_${{comps[:2]}}"/>
  <link name="NEG_${{comps[-1]}}"/>

  <!-- eager lazy_eval=false self-referential redefinition (OpenArm's resolved_mount_origin shape) -->
  <xacro:property name="origin" value="${{dict(x=1.0, y=2.0)}}" lazy_eval="false"/>
  <xacro:property name="origin" value="${{dict(x=origin['x'] + 10.0, y=origin['y'])}}" lazy_eval="false"/>
  <link name="EAGER">
    <inertial><origin xyz="${{origin['x']}} ${{origin['y']}} 0"/></inertial>
  </link>

  <!-- ternary-gated load_yaml: cond FALSE so the absent file is NEVER read (lazy) -->
  <xacro:property name="component_type" value="body"/>
  <xacro:property name="lz" value="${{xacro.load_yaml('{absent}') if component_type != 'body' else dict()}}" lazy_eval="false"/>
  <link name="LAZY_${{lz}}"/>
</robot>
"#,
        cfg_abs = cfg_abs,
        absent = absent_abs,
    );
    let file = dir.join("programmatic.urdf.xacro");
    std::fs::write(&file, &xacro).expect("write programmatic.xacro");

    // PURE-RUST only, NO fallback. Any failure is a real engine regression.
    let urdf = hcdformat::expand_xacro(&file, &HashMap::new())
        .expect("pure-Rust expand_xacro on the programmatic xacro must succeed");

    let links: Vec<String> = top_level_links(&urdf)
        .into_iter()
        .map(|l| l.replace("&apos;", "'"))
        .collect();
    let has = |name: &str| {
        assert!(
            links.iter().any(|l| l == name),
            "expected <link name=\"{name}\">, got: {links:?}"
        );
    };

    // string concat + chain.
    has("P1_/abs/openarm_v2.0/config");
    has("P1chain_/abs/openarm_v2.0/config/robot_presets/default_bimanual.yaml");
    // dict(kwargs) subscripts.
    let xyz = link_child_attr(&urdf, "P2_dict", "origin", "xyz").expect("P2_dict origin/@xyz");
    assert_eq!(xyz, "0.0 1.0 2.5", "dict(kwargs) subscripts mis-evaluated");
    // dotted load_yaml.
    has("P3_arm.dae");
    // %-format.
    has("P4one_attaches `default_bimanual`");
    has("P4two_`default_bimanual` of 7");
    // slicing + negative index.
    has("P5to_['c0', 'c1']");
    has("NEG_c3");
    // eager self-referential redefinition: x = 1.0 + 10.0 = 11.0 (against the OLD value), y = 2.0.
    let exyz = link_child_attr(&urdf, "EAGER", "origin", "xyz").expect("EAGER origin/@xyz");
    let nums: Vec<f64> = exyz
        .split_whitespace()
        .map(|t| t.parse::<f64>().expect("EAGER xyz numeric"))
        .collect();
    assert_eq!(
        nums,
        vec![11.0, 2.0, 0.0],
        "eager self-referential redefinition mis-evaluated"
    );
    // ternary-gated load_yaml: the untaken branch resolved to empty dict (repr `{}`) and the absent file
    // was NEVER read (no error).
    has("LAZY_{}");

    eprintln!("[ok] OpenArm-style programmatic constructs match CPython via the PURE-RUST path");
}

/// Find, inside the `<link name=link>` block, the first `<tag …>` and return its `attr` value (unescaped).
fn link_child_attr(xml: &str, link: &str, tag: &str, attr_key: &str) -> Option<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(xml);
    let mut in_link = false;
    let mut link_depth = 0i32;
    let mut depth = 0i32;
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => {
                if !in_link && local_name(&e) == "link" && attr(&e, "name").as_deref() == Some(link)
                {
                    in_link = true;
                    link_depth = depth;
                }
                if in_link && local_name(&e) == tag {
                    if let Some(v) = attr(&e, attr_key) {
                        return Some(v);
                    }
                }
                depth += 1;
            }
            Event::Empty(e) => {
                if in_link && local_name(&e) == tag {
                    if let Some(v) = attr(&e, attr_key) {
                        return Some(v);
                    }
                }
            }
            Event::End(_) => {
                depth -= 1;
                if in_link && depth <= link_depth {
                    in_link = false;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    None
}

/// Smoke test: the full `.xacro -> URDF -> HCDF` import chain through [`hcdformat::from_xacro_path`]
/// yields a non-empty, valid-shape HCDF document (the importer's end-to-end path), for EVERY corpus class
/// including OpenArm, all pure-Rust.
#[test]
fn convert_xacro_to_hcdf_smoke() {
    let cases = corpus();
    if cases.is_empty() {
        eprintln!("[skip] no corpus xacros found");
        return;
    }
    for case in &cases {
        let pkg_map = pkg_str_map(case);
        let (doc, _notes) = hcdformat::from_xacro_path(&case.file, &pkg_map)
            .unwrap_or_else(|e| panic!("[{}] from_xacro_path failed: {e}", case.label));
        let xml = doc.to_xml_string().expect("serialize HCDF");
        assert!(
            xml.contains("<comp") || xml.contains("<hcdf"),
            "[{}] HCDF output looks empty",
            case.label
        );
        if let Err(errs) = hcdformat::validate_structural(&doc) {
            panic!(
                "[{}] imported HCDF is structurally invalid: {errs:?}",
                case.label
            );
        }
        eprintln!(
            "[ok] {}: .xacro -> HCDF import + structural-validate clean",
            case.label
        );
    }
}

/// PROOF the xacro path is subprocess-free and Python-free: scan the EXECUTABLE source of `src/xacro.rs`
/// (doc/line comments stripped, so the module's prose that legitimately *describes* the retired subprocess
/// is not a false positive) for any `Command::new` / `std::process` (subprocess spawn) or `xacro`-CLI
/// discovery (`xacro_bin`, `/opt/ros`). The pure-Rust `xacro-pure` port replaced the former canonical-
/// `xacro` subprocess fallback entirely, so none of these may reappear in the actual code path.
#[test]
fn no_subprocess_or_python_in_xacro_path() {
    let code = strip_line_comments(include_str!("../src/xacro.rs"), "//");
    for needle in ["Command::new", "std::process", "/opt/ros", "xacro_bin"] {
        assert!(
            !code.contains(needle),
            "src/xacro.rs executable code must be subprocess-free, but contains {needle:?}"
        );
    }
    // No Python-runtime invocation: a literal `"python"` / `"python3"` program name would only appear in a
    // subprocess spawn (which is already banned above), but assert directly that no such program string is
    // present in the code either.
    for needle in ["python3", "\"python\""] {
        assert!(
            !code.contains(needle),
            "src/xacro.rs executable code must be Python-runtime free, but contains {needle:?}"
        );
    }
    // The crate must not DECLARE the retired xacro-rs / pyisheval forks nor any `[patch.crates-io]`. Strip
    // TOML `#` comments first so the historical note that mentions them in prose does not false-positive;
    // assert against the active manifest directives.
    let manifest = strip_line_comments(include_str!("../Cargo.toml"), "#");
    for needle in ["xacro-rs", "pyisheval", "[patch.crates-io]"] {
        assert!(
            !manifest.contains(needle),
            "Cargo.toml active directives must not reference the retired {needle:?}"
        );
    }
    // And it MUST depend on the pure-Rust port.
    assert!(
        manifest.contains("xacro-pure"),
        "Cargo.toml must declare the xacro-pure dependency"
    );
    eprintln!("[ok] xacro path is subprocess-free, Python-free, fork-free, and patch-free");
}

/// Strip whole-line comments (lines whose first non-whitespace is `marker`) AND trailing `marker` comments
/// from each line, so a code/directive scan ignores prose. Conservative: a `marker` inside a string literal
/// would also be cut, which is fine here (we only assert ABSENCE of needles, and none of the banned needles
/// legitimately live after a `//`/`#` on a code line in these files).
fn strip_line_comments(text: &str, marker: &str) -> String {
    text.lines()
        .map(|line| match line.find(marker) {
            Some(idx) => &line[..idx],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
