//! Validation (pure-Rust, wasm-safe).
//!
//! Two layers, both returning a stable [`Issue`] (code + level + message) so callers and the
//! golden-file parity harness can compare on CODES, matching the Python oracle (`hcdfdom.validate`):
//!
//!  * **Structural** ([`validate_structural`]): the key required-attribute invariants from the schema
//!    after deserialize: the document and component names, the required `visual/@name`, `frame/@name`,
//!    `extension/@domain`, and joint parent/child component references.
//!  * **Enum-value** ([`validate_enums`]): the RESIDUAL enum check. Now every enumerated
//!    attribute / singular element-text is a typed `Option<EnumTy>` model field, so an out-of-enum
//!    literal is rejected by serde at PARSE (`Hcdf::from_xml_str` fails, matching Python's load-time
//!    `ValueError`) and needs no run-time check. This layer covers only the two slot families the typed
//!    model can't enforce: the nine sensor-category `@type` slots that share one `SensorCategory`
//!    struct, and the repeated `<cipher>` text element (a `Vec` quick-xml can't deserialize as
//!    `Vec<EnumTy>`), for which an out-of-enum literal is an `E_ENUM_VALUE` error. The (element, key)
//!    -> value-set mapping is GENERATED from `hcdf.xsd` (see [`crate::model::enums::ENUM_ATTRS`]).
//!
//! Deep XSD-level constraints (full keyref integrity, choice exclusivity, frustum placement) and the
//! semantic kinematic-tree rules (E_CYCLE, E_MULTI_PARENT, ...) are left to later work / the
//! authoritative libxml2 path.
use crate::error::{Error, Result as CrateResult};
use crate::model::enums::valid_values_for;
use crate::model::Hcdf;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

/// Severity of a validation [`Issue`]. Mirrors the Python oracle's `error`/`warning` levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// A rule violation that makes the document invalid.
    Error,
    /// An advisory that does not, on its own, invalidate the document.
    Warning,
}

impl Level {
    /// The lowercase string form used in serialized issue lines (matches the Python oracle).
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

/// A single validation finding: a stable machine code, a severity, and a human message. Code + level
/// are the parity surface (compared against the Python oracle's `Issue` codes); the message is advisory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// The severity (`error`/`warning`).
    pub level: Level,
    /// A stable machine-readable code (e.g. `E_ENUM_VALUE`).
    pub code: String,
    /// A human-readable explanation.
    pub message: String,
}

impl Issue {
    fn error(code: &str, message: String) -> Self {
        Issue {
            level: Level::Error,
            code: code.to_string(),
            message,
        }
    }

    fn warning(code: &str, message: String) -> Self {
        Issue {
            level: Level::Warning,
            code: code.to_string(),
            message,
        }
    }
}

impl core::fmt::Display for Issue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "[{}] {}: {}",
            self.level.as_str(),
            self.code,
            self.message
        )
    }
}

/// Structural required-attribute validation over the typed model.
///
/// Returns `Ok` or the list of structural issue messages. Kept returning `Vec<String>` (not `Issue`)
/// for the existing call sites in the example/round-trip tests; the message wording is unchanged.
pub fn validate_structural(doc: &Hcdf) -> std::result::Result<(), Vec<String>> {
    let mut issues = Vec::new();
    if doc.name.trim().is_empty() {
        issues.push("hcdf/@name is required".to_string());
    }
    for (i, c) in doc.comp.iter().enumerate() {
        if c.name.trim().is_empty() {
            issues.push(format!("comp[{i}]/@name is required"));
        }
        for (j, v) in c.visual.iter().enumerate() {
            if v.name.trim().is_empty() {
                issues.push(format!("comp[{i}]/visual[{j}]/@name is required"));
            }
        }
        for (j, f) in c.frame.iter().enumerate() {
            if f.name.trim().is_empty() {
                issues.push(format!("comp[{i}]/frame[{j}]/@name is required"));
            }
        }
        for (j, e) in c.extension.iter().enumerate() {
            if e.domain.trim().is_empty() {
                issues.push(format!("comp[{i}]/extension[{j}]/@domain is required"));
            }
        }
    }
    for (i, e) in doc.extension.iter().enumerate() {
        if e.domain.trim().is_empty() {
            issues.push(format!("extension[{i}]/@domain is required"));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

/// Enum-value validation over the document XML.
///
/// For every `(element, attr)` enumerated slot the schema defines (the generated
/// [`crate::model::enums::ENUM_ATTRS`] table), any attribute value or enum-typed element text not in the
/// accepted set is reported as `E_ENUM_VALUE`. Operating on the XML (not the typed model) means every
/// enumerated slot is covered uniformly with no per-field hand-wiring and no model field retyping. A
/// valid value (every value the corpus carries) yields no issue. Returns the issues in document order.
pub fn validate_enums(xml: &str) -> CrateResult<Vec<Issue>> {
    let mut reader = Reader::from_str(xml);
    let mut issues: Vec<Issue> = Vec::new();
    // The most-recently-opened start element whose text is an enum-typed leaf, awaiting its text event.
    let mut pending_text: Option<(String, &'static [&'static str])> = None;
    loop {
        let event = reader.read_event().map_err(|e| Error::Xml(e.to_string()))?;
        match event {
            Event::Start(e) => {
                let elem = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                check_attributes(&elem, &e, &reader, &mut issues)?;
                // an enum-typed element-text slot opens on a Start; capture its text on the next event.
                pending_text = valid_values_for(&elem, "#text").map(|v| (elem, v));
            }
            Event::Empty(e) => {
                let elem = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                check_attributes(&elem, &e, &reader, &mut issues)?;
                // an empty element carries no text, so no `#text` capture; clear any stale pending one.
                pending_text = None;
            }
            Event::Text(t) => {
                if let Some((elem, valid)) = pending_text.take() {
                    let val = t
                        .unescape()
                        .map_err(|err| Error::Xml(err.to_string()))?
                        .trim()
                        .to_string();
                    if !val.is_empty() && !valid.contains(&val.as_str()) {
                        issues.push(enum_issue(&elem, "#text", &val, valid));
                    }
                }
            }
            Event::End(_) => pending_text = None,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(issues)
}

/// Check each enumerated attribute on an element's start tag against its accepted value-set.
fn check_attributes(
    elem: &str,
    e: &quick_xml::events::BytesStart<'_>,
    reader: &Reader<&[u8]>,
    issues: &mut Vec<Issue>,
) -> CrateResult<()> {
    for attr in e.attributes().with_checks(false) {
        let attr = attr.map_err(|err| Error::Xml(err.to_string()))?;
        let key_local = String::from_utf8_lossy(attr.key.local_name().as_ref()).into_owned();
        let key = format!("@{key_local}");
        if let Some(valid) = valid_values_for(elem, &key) {
            let val = attr
                .decode_and_unescape_value(reader.decoder())
                .map_err(|err| Error::Xml(err.to_string()))?
                .into_owned();
            if !valid.contains(&val.as_str()) {
                issues.push(enum_issue(elem, &key, &val, valid));
            }
        }
    }
    Ok(())
}

fn enum_issue(elem: &str, key: &str, val: &str, valid: &[&str]) -> Issue {
    let slot = if key == "#text" {
        format!("{elem} text")
    } else {
        format!("{elem}/{key}")
    };
    Issue::error(
        "E_ENUM_VALUE",
        format!("{slot}: {val:?} is not one of [{}]", valid.join(", ")),
    )
}

// ── semantic / kinematic validation ───────────────────────────────────────────
//
// Port of the Python companion validator (`hcdfdom/validate.py`). The XSD already enforces structure
// and the six root keyrefs; this layer adds the rules XSD 1.0 cannot express, producing the SAME
// `(code, level)` issues as the Python oracle. The model fields are typed (`JointType`, etc.), so the
// per-joint-type axis/limit matrix matches on enum variants. Messages reproduce the oracle's exact
// wording (including Python's `repr()` single-quoting and `float()` formatting) so the committed
// `issues.txt` goldens compare byte-for-byte, not just on codes.

use crate::model::enums::JointType;
use std::collections::{BTreeMap, BTreeSet};

const GLB_EXT: &[&str] = &[".glb", ".gltf"];

/// Python's `repr()` for a plain string: single-quoted unless the value contains a `'` but no `"`
/// (then double-quoted), matching CPython. Used so issue messages reproduce the oracle's `{x!r}`.
fn py_repr(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

/// Format an `f64` exactly as CPython 3's `repr(float)` / `str(float)`.
///
/// CPython uses David Gay's `dtoa` mode 0 (shortest decimal that round-trips, ties to even) for the
/// digits, then `repr`-style presentation: fixed notation when the decimal exponent of the leading
/// significant digit is in `-4..=15`, else scientific with a sign and a >= 2-digit exponent
/// (`1e+20`, `1e-05`). The `ryu` crate produces the SAME shortest digits as Gay's algorithm (verified
/// byte-for-byte over ~400k random + boundary doubles), so we take ryu's digits + decimal exponent and
/// only re-apply Python's presentation rules, avoiding a from-scratch dtoa while staying exact. Rust's
/// own `{}`/`{:e}` are NOT used for the digits: their shortest-digit tie-breaking differs from Python's
/// (e.g. `...910.1562` vs `...910.1563`), which would break byte-for-byte parity.
///
/// `pub(crate)` because the XSD-driven JSON-schema generator ([`crate::xsdgen`]) serializes an XSD
/// `default=` on an `xs:double` attribute as a JSON number, and `json.dump` renders it through
/// `float.__repr__`, the very presentation this reproduces, so both live off one dtoa-parity helper.
pub(crate) fn py_float_repr(v: f64) -> String {
    if v == 0.0 {
        // ryu renders signed zero as "0.0"/"-0.0" too, but short-circuit to be explicit.
        return if v.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    if v.is_nan() {
        // Python str(float('nan')) == 'nan'; Rust's Display is 'NaN', so emit Python's spelling.
        return "nan".into();
    }
    if v.is_infinite() {
        // Python str(±inf) == 'inf' / '-inf'; Rust Display matches.
        return if v < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let neg = v < 0.0;
    let mut buf = ryu::Buffer::new();
    // ryu forms for a finite non-zero magnitude: "ddd.ddd" or "d.ddde[-]N" (exponent unsigned or '-N').
    let r = buf.format_finite(v.abs());
    let (mant, exp): (&str, i32) = match r.split_once('e') {
        Some((m, e)) => (m, e.parse().expect("ryu exponent is an integer")),
        None => (r, 0),
    };
    let (int_part, frac_part) = mant.split_once('.').unwrap_or((mant, ""));
    // Decimal exponent of the leading significant digit (the "scientific" exponent).
    let int_digits = int_part.trim_start_matches('0');
    let lead_exp = if int_digits.is_empty() {
        // |value| < 1: leading digit lives in the fraction; count its leading zeros.
        let lz = frac_part.len() - frac_part.trim_start_matches('0').len();
        exp - 1 - lz as i32
    } else {
        exp + int_digits.len() as i32 - 1
    };
    // The significant digit run (no point, no structural leading/trailing zeros).
    let all = format!("{int_part}{frac_part}");
    let digits = all.trim_start_matches('0').trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };

    let body = if !(-4..16).contains(&lead_exp) {
        // scientific (decimal exp < -4 or >= 16): d[.ddd]eSNN  (S = '+'/'-', exp zero-padded >= 2 digits)
        let m = if digits.len() == 1 {
            digits.to_string()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        let sign = if lead_exp < 0 { '-' } else { '+' };
        format!("{m}e{sign}{:02}", lead_exp.abs())
    } else if lead_exp >= 0 {
        // fixed, point after lead_exp+1 digits; pad zeros + ".0" when it lands past the digits.
        let point = (lead_exp + 1) as usize;
        if point >= digits.len() {
            format!("{digits}{}.0", "0".repeat(point - digits.len()))
        } else {
            format!("{}.{}", &digits[..point], &digits[point..])
        }
    } else {
        // 0.0ddd (lead_exp in -4..=-1)
        format!("0.{}{digits}", "0".repeat((-lead_exp - 1) as usize))
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// Full semantic validation of a parsed [`Hcdf`] document: the Rust port of `hcdfdom.validate`.
///
/// Returns every semantic issue (errors + warnings) the Python oracle would, with matching codes,
/// levels, and message text. The checks, in oracle order: kinematic-tree integrity
/// (`E_SELF_JOINT`, `E_MULTI_PARENT`, `E_CYCLE`, `W_MULTI_ROOT`); loop-closure refs (`E_LOOP_REF`);
/// per-joint-type axis/limit semantics (`E_JOINT_*`, `E_LIMIT_RANGE`); `frame/@relative-to` resolution
/// over the sibling-frame ∪ comp ∪ joint union (`E_FRAME_RELATIVE_TO`); visual color refs
/// (`E_COLOR_REF`); at-most-one default state (`E_MULTI_DEFAULT_STATE`); group tip-comp/tip-frame
/// resolution (`E_GROUP_TIP_COMP`, `E_GROUP_TIP_FRAME`); and `@uri` media advisories
/// (`W_VISUAL_URI`, `W_COLLISION_URI`).
pub fn validate_semantic(doc: &Hcdf) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let comp_names: BTreeSet<&str> = doc.comp.iter().map(|c| c.name.as_str()).collect();
    let joint_names: BTreeSet<&str> = doc.joint.iter().filter_map(|j| j.name.as_deref()).collect();
    let color_names: BTreeSet<&str> = doc.color.iter().filter_map(|c| c.name.as_deref()).collect();
    // comp name -> set of frame names on that comp (frame/@name is REQUIRED, so always present).
    let mut frames_by_comp: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for c in &doc.comp {
        let set = frames_by_comp.entry(c.name.as_str()).or_default();
        for f in &c.frame {
            if !f.name.is_empty() {
                set.insert(f.name.as_str());
            }
        }
    }

    check_tree(doc, &mut issues);
    check_loops(doc, &comp_names, &mut issues);
    check_joint_semantics(doc, &mut issues);
    check_frames(doc, &comp_names, &joint_names, &frames_by_comp, &mut issues);
    check_colors(doc, &color_names, &mut issues);
    check_states(doc, &mut issues);
    check_groups(doc, &comp_names, &frames_by_comp, &mut issues);
    check_media(doc, &mut issues);
    issues
}

/// Kinematic-tree integrity over PRIMARY edges (non-loop joints): self-joint, multi-parent, cycle, and
/// single-root. Loop-closure joints (those carrying `<loop>`) are constraints, not tree edges, so they
/// are excluded here; a four-bar/Stewart loop is allowed and does not trip `E_CYCLE`.
fn check_tree(doc: &Hcdf, issues: &mut Vec<Issue>) {
    // Edges in document order: (parent, child, joint-name). Joint names default to "" like Python's
    // None-name (XSD guarantees presence in practice); kept as &str for stable ordering.
    let mut edges: Vec<(&str, &str, &str)> = Vec::new();
    // child -> joint names that parent it (insertion order preserved for the document-order DFS).
    let mut parents_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut parents_order: Vec<&str> = Vec::new();
    for j in &doc.joint {
        if j.loop_.is_some() {
            continue; // loop joints are constraints, not tree edges
        }
        let p = j.parent.as_ref().and_then(|e| e.comp.as_deref());
        let c = j.child.as_ref().and_then(|e| e.comp.as_deref());
        let (Some(p), Some(c)) = (p, c) else {
            continue; // XSD guarantees presence; defensive (mirror Python's skip)
        };
        let jname = j.name.as_deref().unwrap_or("");
        edges.push((p, c, jname));
        if !parents_of.contains_key(c) {
            parents_order.push(c);
        }
        parents_of.entry(c).or_default().push(jname);
        if p == c {
            issues.push(Issue::error(
                "E_SELF_JOINT",
                format!(
                    "joint {} has the same parent and child comp {}",
                    py_repr(jname),
                    py_repr(p)
                ),
            ));
        }
    }

    // Multi-parent: a comp that is the child of >1 non-loop joint. Joint names sorted (Python sorts).
    for child in &parents_order {
        let js = &parents_of[child];
        if js.len() > 1 {
            let mut sorted: Vec<&str> = js.clone();
            sorted.sort_unstable();
            issues.push(Issue::error(
                "E_MULTI_PARENT",
                format!(
                    "comp {} is the child of {} joints ({}); a kinematic tree allows one",
                    py_repr(child),
                    js.len(),
                    sorted.join(", ")
                ),
            ));
        }
    }

    // Cycle detection over parent -> children adjacency (insertion-ordered), reporting the back-edge
    // path exactly as Python's DFS does (`stack + [n, m]` joined by " -> ").
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut adj_order: Vec<&str> = Vec::new();
    for (p, c, _) in &edges {
        if !adj.contains_key(p) {
            adj_order.push(p);
        }
        adj.entry(p).or_default().push(c);
    }
    // node set = adj keys ∪ all children, visited in: adj-key insertion order, then any remaining
    // children. Python iterates a `set`, but the recorded cycle PATH is deterministic regardless of
    // start order, so we use a stable order and de-dup the reported paths to match the oracle's output.
    let mut node_order: Vec<&str> = adj_order.clone();
    for (_, c, _) in &edges {
        if !node_order.contains(c) {
            node_order.push(c);
        }
    }
    let mut color: BTreeMap<&str, Mark> = BTreeMap::new();
    let mut cycles: Vec<Vec<&str>> = Vec::new();
    for start in &node_order {
        if *color.get(start).unwrap_or(&Mark::White) == Mark::White {
            dfs_cycle(start, &[], &adj, &mut color, &mut cycles);
        }
    }
    // Canonicalize each discovered path (drop any non-cycle tail, rotate to the lexicographically-min
    // node) so the reported string is independent of the DFS start node; Python's DFS starts from a
    // `set` (hash-seed order), so without this the same cycle renders rotated differently across runs.
    // De-dup + sort so a cycle reachable from several starts is reported once, in seed-independent
    // order; this MATCHES `hcdfdom.validate._canon_cycle` byte-for-byte (see that function).
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for path in &cycles {
        seen.insert(canon_cycle(path));
    }
    let cycle_found = !seen.is_empty();
    for path in &seen {
        issues.push(Issue::error(
            "E_CYCLE",
            format!("kinematic cycle through comps: {path}"),
        ));
    }

    // Single-root: only when there are edges and NO cycle (a cyclic graph has no well-defined root).
    if !edges.is_empty() && !cycle_found {
        let children: BTreeSet<&str> = edges.iter().map(|(_, c, _)| *c).collect();
        let mut jointed: BTreeSet<&str> = edges.iter().map(|(p, _, _)| *p).collect();
        jointed.extend(children.iter().copied());
        let roots: Vec<&str> = jointed.difference(&children).copied().collect(); // BTreeSet => sorted
        if roots.len() > 1 {
            issues.push(Issue::warning(
                "W_MULTI_ROOT",
                format!(
                    "{} kinematic roots ({}); a single robot model usually has one",
                    roots.len(),
                    roots.join(", ")
                ),
            ));
        }
    }
}

/// DFS back-edge detection mirroring Python's `dfs(n, stack)`: a GREY successor records the cycle path
/// `stack + [n, m]`; a WHITE successor recurses. `adj` lists are in insertion (document) order. The
/// recorded path is the raw node list (`[start, ..., m, ..., n, m]`); [`canon_cycle`] normalizes it.
fn dfs_cycle<'a>(
    n: &'a str,
    stack: &[&'a str],
    adj: &BTreeMap<&'a str, Vec<&'a str>>,
    color: &mut BTreeMap<&'a str, Mark>,
    cycles: &mut Vec<Vec<&'a str>>,
) {
    color.insert(n, Mark::Grey);
    if let Some(succs) = adj.get(n) {
        for m in succs {
            match color.get(m).copied().unwrap_or(Mark::White) {
                Mark::Grey => {
                    let mut path: Vec<&str> = stack.to_vec();
                    path.push(n);
                    path.push(m);
                    cycles.push(path);
                }
                Mark::White => {
                    let mut next: Vec<&str> = stack.to_vec();
                    next.push(n);
                    dfs_cycle(m, &next, adj, color, cycles);
                }
                Mark::Black => {}
            }
        }
    }
    color.insert(n, Mark::Black);
}

/// Canonicalize a DFS-discovered cycle path to a deterministic ` -> `-joined string, identically to
/// `hcdfdom.validate._canon_cycle`. The path is `[start, ..., m, ..., n, m]`: the back edge `n -> m`
/// closes onto an ancestor `m`, possibly behind a non-cycle tail. We drop the tail (slice from the
/// first occurrence of the closing node `m` = `path.last()`), then rotate the cycle so its
/// lexicographically-smallest node leads, and re-append that node to close the loop. This makes the
/// `E_CYCLE` message independent of the DFS start node (Python iterates a hash-seeded `set`), so the
/// two validators agree byte-for-byte regardless of `PYTHONHASHSEED`.
fn canon_cycle(path: &[&str]) -> String {
    let target = *path.last().expect("cycle path is non-empty");
    let i = path.iter().position(|&x| x == target).unwrap_or(0);
    let cyc = &path[i..path.len() - 1]; // cycle nodes without the closing duplicate
    let k = cyc
        .iter()
        .enumerate()
        .min_by_key(|(_, &node)| node)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let mut rot: Vec<&str> = Vec::with_capacity(cyc.len() + 1);
    rot.extend_from_slice(&cyc[k..]);
    rot.extend_from_slice(&cyc[..k]);
    rot.push(cyc[k]);
    rot.join(" -> ")
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    White,
    Grey,
    Black,
}

/// Loop-closure body references: a `<loop>` joint's predecessor/successor comps, when set, must exist.
fn check_loops(doc: &Hcdf, comp_names: &BTreeSet<&str>, issues: &mut Vec<Issue>) {
    for j in &doc.joint {
        let Some(lp) = &j.loop_ else { continue };
        let jname = j.name.as_deref().unwrap_or("");
        for (role, name) in [
            ("predecessor", lp.predecessor.as_deref()),
            ("successor", lp.successor.as_deref()),
        ] {
            if let Some(name) = name {
                if !comp_names.contains(name) {
                    issues.push(Issue::error(
                        "E_LOOP_REF",
                        format!(
                            "joint {}: loop {role} {} is not a comp",
                            py_repr(jname),
                            py_repr(name)
                        ),
                    ));
                }
            }
        }
    }
}

/// Per-joint-type axis / axis2 / limit / limit2 / thread_pitch presence rules + `upper >= lower`.
/// Matches on the typed [`JointType`] variant. Enforces the approved per-type semantics (see hcdf.xsd
/// joint annotations): required fields per type, forbidden fields per type, and the cross-cutting
/// stray-field forbids (axis2 only on universal; limit2 only on universal/cylindrical/planar;
/// thread_pitch only on screw). A joint with no `@type` (None) stays permissive; only the limit-range
/// check applies.
fn check_joint_semantics(doc: &Hcdf, issues: &mut Vec<Issue>) {
    for j in &doc.joint {
        let jname = j.name.as_deref().unwrap_or("");
        let tname = j.type_.map(|t| t.to_string());
        let tdisp = tname.as_deref().unwrap_or("None");
        let has_axis = j.axis.is_some();
        let has_axis2 = j.axis2.is_some();
        let has_limit2 = j.limit2.is_some();
        let has_thread_pitch = j.thread_pitch.is_some();
        let has_swing = j.swing_limit.is_some() || j.twist_limit.is_some();
        let has_pitch_attr = j.pitch_convention.is_some() || j.handedness.is_some();
        let lim = j.limit.as_ref();
        let has_bounds = lim.is_some_and(|l| l.lower.is_some() || l.upper.is_some());

        let mut err = |code: &str, msg: &str| {
            issues.push(Issue::error(
                code,
                format!("joint {} ({tdisp}): {msg}", py_repr(jname)),
            ));
        };

        // Required / forbidden axis + primary <limit> per type.
        match j.type_ {
            Some(JointType::Revolute | JointType::Prismatic) => {
                if !has_axis {
                    err("E_JOINT_AXIS_REQUIRED", "requires an <axis>");
                }
                if !has_bounds {
                    err(
                        "E_JOINT_LIMIT_REQUIRED",
                        "requires a <limit> with lower and upper",
                    );
                }
            }
            Some(JointType::Cylindrical) => {
                // 2-DOF: axis + translation <limit> required; rotation <limit2> optional (radians).
                if !has_axis {
                    err("E_JOINT_AXIS_REQUIRED", "requires an <axis>");
                }
                if !has_bounds {
                    err(
                        "E_JOINT_LIMIT_REQUIRED",
                        "requires a <limit> with lower and upper (translation range, meters)",
                    );
                }
            }
            Some(JointType::Continuous) => {
                if !has_axis {
                    err("E_JOINT_AXIS_REQUIRED", "requires an <axis>");
                }
                if has_bounds {
                    err(
                        "E_JOINT_CONTINUOUS_BOUNDS",
                        "is continuous and must omit limit lower/upper (unbounded rotation)",
                    );
                }
            }
            Some(JointType::Universal) => {
                if !has_axis || !has_axis2 {
                    err("E_JOINT_AXIS2_REQUIRED", "requires both <axis> and <axis2>");
                }
            }
            Some(JointType::Screw) => {
                // 1-DOF helical: axis + thread_pitch required. <limit> (rotational DOF, radians) is
                // allowed but optional; the coupled axial translation derives via thread_pitch.
                if !has_axis {
                    err("E_JOINT_AXIS_REQUIRED", "requires an <axis>");
                }
                if !has_thread_pitch {
                    err("E_JOINT_THREADPITCH_REQUIRED", "requires @thread_pitch");
                }
            }
            Some(JointType::Planar) => {
                // 2-DOF in-plane translation: axis (the plane normal) required. <limit>/<limit2> are the
                // two in-plane ranges (meters), both optional.
                if !has_axis {
                    err(
                        "E_JOINT_AXIS_REQUIRED",
                        "requires an <axis> (the plane normal)",
                    );
                }
            }
            Some(JointType::Ball) => {
                // 3-DOF spherical: no single axis and no scalar <limit> (its range is a swing-cone +
                // twist). axis/axis2/limit/limit2/thread_pitch are all forbidden (axis2/limit2/
                // thread_pitch by the stray-field forbids below).
                if has_axis {
                    err(
                        "E_JOINT_AXIS_FORBIDDEN",
                        "is 3-DOF spherical and must not have an <axis>",
                    );
                }
                if lim.is_some() {
                    err(
                        "E_JOINT_LIMIT_FORBIDDEN",
                        "is 3-DOF spherical and must not have a <limit>",
                    );
                }
            }
            Some(JointType::Fixed | JointType::Free) => {
                if has_axis {
                    err("E_JOINT_AXIS_FORBIDDEN", "must not have an <axis>");
                }
                if lim.is_some() {
                    err("E_JOINT_LIMIT_FORBIDDEN", "must not have a <limit>");
                }
            }
            None => {}
        }

        // Cross-cutting stray-field forbids: axis2 only on universal; limit2 only on universal/
        // cylindrical/planar; thread_pitch (+ its @pitch_convention/@handedness qualifiers) only on
        // screw; swing_limit/twist_limit only on ball. Unknown/unset type stays permissive.
        let (allow_axis2, allow_limit2, allow_thread_pitch, allow_swing) = match j.type_ {
            Some(JointType::Universal) => (true, true, false, false),
            Some(JointType::Cylindrical | JointType::Planar) => (false, true, false, false),
            Some(JointType::Screw) => (false, false, true, false),
            Some(JointType::Ball) => (false, false, false, true),
            Some(_) => (false, false, false, false),
            None => (true, true, true, true),
        };
        if has_axis2 && !allow_axis2 {
            err(
                "E_JOINT_AXIS2_FORBIDDEN",
                "must not have an <axis2> (only universal joints have a second axis)",
            );
        }
        if has_limit2 && !allow_limit2 {
            err(
                "E_JOINT_LIMIT2_FORBIDDEN",
                "must not have a <limit2> (only universal, cylindrical, and planar joints bound a second DOF)",
            );
        }
        if has_thread_pitch && !allow_thread_pitch {
            err(
                "E_JOINT_THREADPITCH_FORBIDDEN",
                "must not have @thread_pitch (only screw joints)",
            );
        }
        if has_pitch_attr && !allow_thread_pitch {
            err(
                "E_JOINT_PITCH_ATTR_FORBIDDEN",
                "must not have @pitch_convention/@handedness (only screw joints)",
            );
        }
        if has_swing && !allow_swing {
            err(
                "E_JOINT_SWING_FORBIDDEN",
                "must not have <swing_limit>/<twist_limit> (only ball joints have a swing-cone + twist)",
            );
        }

        // upper >= lower on each present limit (primary <limit> and second-DOF <limit2>). Only when BOTH
        // numeric bounds parse. Python compares and formats the PARSED floats (`float(text)`), so e.g.
        // "1e400" -> inf on both the comparison and the message; we likewise format `lof`/`hif`.
        for (which, l) in [
            ("limit", lim),
            ("limit2", j.limit2.as_ref()),
            ("twist_limit", j.twist_limit.as_ref()),
        ] {
            let Some(l) = l else { continue };
            if let (Some(lo), Some(hi)) = (l.lower.as_deref(), l.upper.as_deref()) {
                if let (Some(lof), Some(hif)) =
                    (lo.trim().parse::<f64>().ok(), hi.trim().parse::<f64>().ok())
                {
                    if hif < lof {
                        err(
                            "E_LIMIT_RANGE",
                            &format!(
                                "{which} upper ({}) < lower ({})",
                                py_float_repr(hif),
                                py_float_repr(lof)
                            ),
                        );
                    }
                }
            }
        }
    }
}

/// `frame/@relative-to` must resolve against the UNION of sibling-frame ∪ comp ∪ joint names: a
/// heterogeneous namespace no single xs:keyref can target. An unresolved ref is `E_FRAME_RELATIVE_TO`.
fn check_frames(
    doc: &Hcdf,
    comp_names: &BTreeSet<&str>,
    joint_names: &BTreeSet<&str>,
    frames_by_comp: &BTreeMap<&str, BTreeSet<&str>>,
    issues: &mut Vec<Issue>,
) {
    let empty = BTreeSet::new();
    for c in &doc.comp {
        let siblings = frames_by_comp.get(c.name.as_str()).unwrap_or(&empty);
        for f in &c.frame {
            let Some(rel) = f.relative_to.as_deref() else {
                continue;
            };
            if siblings.contains(rel) || comp_names.contains(rel) || joint_names.contains(rel) {
                continue;
            }
            issues.push(Issue::error(
                "E_FRAME_RELATIVE_TO",
                format!(
                    "frame {} on comp {}: relative-to {} matches no sibling frame, comp, or joint",
                    py_repr(&f.name),
                    py_repr(&c.name),
                    py_repr(rel)
                ),
            ));
        }
    }
}

/// A name-only visual `<color name="x"/>` (no `@rgba`) is a REFERENCE; it must resolve to a
/// document-level `<color name="x">`. An inline color (`@rgba` present) is self-contained and skipped.
fn check_colors(doc: &Hcdf, color_names: &BTreeSet<&str>, issues: &mut Vec<Issue>) {
    for c in &doc.comp {
        for v in &c.visual {
            let col = match &v.appearance {
                crate::model::VisualAppearance::Primitive { color, .. } => color.as_ref(),
                crate::model::VisualAppearance::Model { .. } => None,
            };
            if let Some(col) = col {
                if let Some(cname) = col.name.as_deref() {
                    if col.rgba.is_none() && !color_names.contains(cname) {
                        issues.push(Issue::error(
                            "E_COLOR_REF",
                            format!(
                                "visual {} on comp {} references color {}, which is not a \
                                 document-level <color>",
                                py_repr(&v.name),
                                py_repr(&c.name),
                                py_repr(cname)
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// At most one `<state default="true">`. The `@default` attribute is compared case-insensitively to
/// `"true"`, matching Python's `str(s.default).lower() == "true"`.
fn check_states(doc: &Hcdf, issues: &mut Vec<Issue>) {
    let defaults: Vec<&str> = doc
        .state
        .iter()
        .filter(|s| {
            s.default
                .as_deref()
                .map(|d| d.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        })
        .map(|s| s.name.as_deref().unwrap_or(""))
        .collect();
    if defaults.len() > 1 {
        issues.push(Issue::error(
            "E_MULTI_DEFAULT_STATE",
            format!(
                "{} states marked default=true ({}); at most one allowed",
                defaults.len(),
                defaults.join(", ")
            ),
        ));
    }
}

/// `group/@tip-comp` must be a comp; `group/@tip-frame`, when set, requires a tip-comp and must name a
/// frame ON that tip-comp.
fn check_groups(
    doc: &Hcdf,
    comp_names: &BTreeSet<&str>,
    frames_by_comp: &BTreeMap<&str, BTreeSet<&str>>,
    issues: &mut Vec<Issue>,
) {
    let empty = BTreeSet::new();
    for g in &doc.group {
        let gname = g.name.as_deref().unwrap_or("");
        if let Some(tip_comp) = g.tip_comp.as_deref() {
            if !comp_names.contains(tip_comp) {
                issues.push(Issue::error(
                    "E_GROUP_TIP_COMP",
                    format!(
                        "group {}: tip-comp {} is not a comp",
                        py_repr(gname),
                        py_repr(tip_comp)
                    ),
                ));
            }
        }
        if let Some(tip_frame) = g.tip_frame.as_deref() {
            match g.tip_comp.as_deref() {
                None => issues.push(Issue::error(
                    "E_GROUP_TIP_FRAME",
                    format!(
                        "group {}: tip-frame {} set without a tip-comp",
                        py_repr(gname),
                        py_repr(tip_frame)
                    ),
                )),
                Some(tip_comp) => {
                    let frames = frames_by_comp.get(tip_comp).unwrap_or(&empty);
                    if !frames.contains(tip_frame) {
                        issues.push(Issue::error(
                            "E_GROUP_TIP_FRAME",
                            format!(
                                "group {}: tip-frame {} is not a frame on tip-comp {}",
                                py_repr(gname),
                                py_repr(tip_frame),
                                py_repr(tip_comp)
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// `@uri` media advisories (warnings): a visual `<model>` uri should be GLB/glTF (`W_VISUAL_URI` if
/// not); a collision mesh uri should be lean, never a GLB (`W_COLLISION_URI` if it is).
fn check_media(doc: &Hcdf, issues: &mut Vec<Issue>) {
    for c in &doc.comp {
        for v in &c.visual {
            let uri = match &v.appearance {
                crate::model::VisualAppearance::Model { model, .. } => model.uri.as_deref(),
                crate::model::VisualAppearance::Primitive { .. } => None,
            };
            if let Some(uri) = uri {
                if !uri.is_empty() && !ends_with_any(uri, GLB_EXT) {
                    issues.push(Issue::warning(
                        "W_VISUAL_URI",
                        format!(
                            "visual {} on comp {}: <model> uri {} is not GLB/glTF (rich visual \
                             appearance should be baked GLB)",
                            py_repr(&v.name),
                            py_repr(&c.name),
                            py_repr(uri)
                        ),
                    ));
                }
            }
        }
        for col in &c.collision {
            let uri = col
                .geometry
                .as_ref()
                .and_then(|g| g.mesh.as_ref())
                .and_then(|m| m.uri.as_deref());
            if let Some(uri) = uri {
                if !uri.is_empty() && ends_with_any(uri, GLB_EXT) {
                    issues.push(Issue::warning(
                        "W_COLLISION_URI",
                        format!(
                            "collision {} on comp {}: mesh uri {} is a GLB; collision meshes \
                             should be lean (STL/OBJ/convex)",
                            py_repr(col.name.as_deref().unwrap_or("")),
                            py_repr(&c.name),
                            py_repr(uri)
                        ),
                    ));
                }
            }
        }
    }
}

/// Case-insensitive suffix test (Python's `uri.lower().endswith(exts)`).
fn ends_with_any(uri: &str, exts: &[&str]) -> bool {
    let lower = uri.to_lowercase();
    exts.iter().any(|e| lower.ends_with(e))
}

// ── loop-closure validation ───────────────────────────────────────────────────
//
// A Rust-only layer over the `<loop>` marker that turns a joint from a spanning-tree edge into a
// CLOSURE CONSTRAINT (a four-bar / delta / Stewart platform, a cycle URDF cannot express). Like the
// network layer below, these codes have NO Python oracle (the pure-Python validator was retired before
// loop closure was actioned), so they are a fresh contract frozen by the `tests/golden/_loop_oracle`
// code-set goldens. All codes are `E_LOOP_*` / `W_LOOP_*`.
//
// This layer COMPLEMENTS (it does not replace) `validate_semantic`'s `E_LOOP_REF`. That kinematic-layer
// code (Python-oracle parity) still fires when a predecessor/successor names no comp, treating the two
// endpoints as one class. The codes here add the machine-facing detail the constraint solver actually
// needs and could not otherwise recover from a single combined code:
//   * `E_LOOP_PREDECESSOR_REF` / `E_LOOP_SUCCESSOR_REF`: WHICH endpoint dangles, split so a tool can
//     point the author at the exact bad reference (errors: a solver cannot form a residual for a body
//     that does not exist).
//   * `E_LOOP_AXES_MALFORMED`: `<constraint-axes>` is present but is not exactly six space-separated
//     `0`/`1` tokens. It is MACHINE-CONSUMED: the residual projector reads it as the mask
//     "tx ty tz rx ry rz" (1 = constrained, 0 = free) in the JOINT frame, so a wrong token count or a
//     non-0/1 token cannot be projected and is a hard error, not an advisory.
//   * `W_LOOP_BODY_MISMATCH`: a predecessor/successor that resolves to a REAL comp but is not the
//     joint's own parent/child comp. The closure joint's parent/child already name the two bodies it
//     pins; a `<loop>` that names different (existing) bodies is almost always an authoring slip. It is
//     only a warning because the schema permits it and a reader can still solve the joint's own edge.
//
// A predecessor/successor that names no comp is reported ONLY as the ref error (not also as a mismatch):
// a dangling reference is a strictly worse fault than disagreeing with an existing body, and double
// reporting would muddy the one-code-per-fault fixture discipline.

/// Full loop-closure validation of a parsed [`Hcdf`] document: for every joint carrying a `<loop>`, the
/// predecessor/successor comp references resolve, the `<constraint-axes>` mask (when present) is exactly
/// six `0`/`1` tokens, and the named bodies agree with the joint's own parent/child comps. Emits
/// `E_LOOP_*` errors and `W_LOOP_BODY_MISMATCH` warnings; complements [`validate_semantic`] (whose
/// `E_LOOP_REF` covers the coarse dangling-reference case) and carries no Python oracle. Returns the
/// issues in document (joint) order.
pub fn validate_loops(doc: &Hcdf) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let comp_names: BTreeSet<&str> = doc.comp.iter().map(|c| c.name.as_str()).collect();
    for j in &doc.joint {
        let Some(lp) = &j.loop_ else { continue };
        let jname = j.name.as_deref().unwrap_or("");
        // The joint's OWN tree endpoints: the two bodies the closure joint physically pins. A `<loop>`
        // predecessor/successor is expected to restate these; naming a different (existing) body warns.
        let parent_comp = j.parent.as_ref().and_then(|e| e.comp.as_deref());
        let child_comp = j.child.as_ref().and_then(|e| e.comp.as_deref());
        for (role, endpoint, own, own_label, ref_code) in [
            (
                "predecessor",
                lp.predecessor.as_deref(),
                parent_comp,
                "parent",
                "E_LOOP_PREDECESSOR_REF",
            ),
            (
                "successor",
                lp.successor.as_deref(),
                child_comp,
                "child",
                "E_LOOP_SUCCESSOR_REF",
            ),
        ] {
            let Some(name) = endpoint else { continue };
            if !comp_names.contains(name) {
                issues.push(Issue::error(
                    ref_code,
                    format!(
                        "joint {}: loop {role} {} is not a comp",
                        py_repr(jname),
                        py_repr(name)
                    ),
                ));
            } else if let Some(own) = own {
                if own != name {
                    issues.push(Issue::warning(
                        "W_LOOP_BODY_MISMATCH",
                        format!(
                            "joint {}: loop {role} {} is not the joint's {own_label} comp {}",
                            py_repr(jname),
                            py_repr(name),
                            py_repr(own)
                        ),
                    ));
                }
            }
        }
        // `<constraint-axes>`: exactly six space-separated 0/1 tokens (the projector's mask). Absent is
        // fine; the solver then derives the mask from the joint TYPE. Present-but-malformed is
        // a hard error: whitespace-split (folding any run of spaces), then require six literal 0/1 tokens.
        if let Some(axes) = lp.constraint_axes.as_deref() {
            let toks: Vec<&str> = axes.split_whitespace().collect();
            let well_formed = toks.len() == 6 && toks.iter().all(|t| *t == "0" || *t == "1");
            if !well_formed {
                issues.push(Issue::error(
                    "E_LOOP_AXES_MALFORMED",
                    format!(
                        "joint {}: <constraint-axes> must be exactly six space-separated 0/1 tokens \
                         \"tx ty tz rx ry rz\", got {}",
                        py_repr(jname),
                        py_repr(axes)
                    ),
                ));
            }
        }
    }
    issues
}

/// Validate the public connectivity model through the canonical normalized graph.
pub fn validate_network(doc: &Hcdf) -> Vec<Issue> {
    validate_network_with_options(doc, crate::connectivity::NormalizationOptions::default())
}

/// Validate the public connectivity model with explicit connectivity normalization options.
pub fn validate_network_with_options(
    doc: &Hcdf,
    options: crate::connectivity::NormalizationOptions,
) -> Vec<Issue> {
    let document_name = if doc.name.trim().is_empty() {
        "<unnamed>"
    } else {
        doc.name.as_str()
    };
    let document = crate::model::connectivity::DocumentIdentity::new(document_name)
        .expect("non-empty document identity");
    let authored = match doc.to_connectivity_document(document) {
        Ok(authored) => authored,
        Err(error) => {
            return vec![Issue::error(error.code(), error.to_string())];
        }
    };
    match crate::connectivity::normalize_connectivity_with_options(&authored, options) {
        Ok(graph) => graph
            .warnings()
            .iter()
            .cloned()
            .map(connectivity_issue)
            .collect(),
        Err(error) => error
            .into_issues()
            .into_iter()
            .map(connectivity_issue)
            .collect(),
    }
}

fn connectivity_issue(issue: crate::connectivity::ConnectivityIssue) -> Issue {
    let message = format!("{}: {}", issue.subject().display_path(), issue.message());
    match issue.level() {
        crate::connectivity::IssueLevel::Error => Issue::error(issue.code(), message),
        crate::connectivity::IssueLevel::Warning => Issue::warning(issue.code(), message),
    }
}

fn sep_hint(token: &str) -> &'static str {
    if token.contains('.') {
        " ('.' is not a reference separator - use '/')"
    } else {
        ""
    }
}

// ── schema-coverage validation ─────────────────────────────────────────────────
//
// Rust-only validators for the referential/uniqueness invariants the schema and the
// long-standing self-collision / transmission / motor surfaces leave to "the companion validator" but
// which no XSD keyref can express (each targets a UNION namespace, or keys on an optional attribute a
// keyref cannot scope). Like the network + loop layers above they have NO Python oracle (the pure-
// Python validator was retired before these grew), so they are a fresh contract frozen by the
// `tests/golden/_coverage_oracle` code-set goldens. Codes are `E_NAME_SEPARATOR`,
// `E_MODEL_SUBMESH_*`, `E_PAIR_*`/`W_PAIR_*`, `E_TRANS_*` (including
// `E_TRANS_MOTOR_AMBIGUOUS`), and `E_MOTOR_SENSOR_REF`.
//
// Message style follows the NETWORK layer (Rust `{:?}` quoting), NOT the kinematic/loop layers'
// Python-`repr()` (`py_repr`): these codes are the closest analogue of the net referential/uniqueness/
// type-compat checks, share no oracle, and the frozen goldens compare on CODES only, so the simpler
// `{:?}` is used throughout these checks.
//
// Error-vs-warning follows the rest of the family:
//   * A DANGLING reference is an ERROR everywhere in this validator (E_LOOP_REF, E_NET_DEVICE_REF,
//     E_COLOR_REF, E_FRAME_RELATIVE_TO, E_GROUP_TIP_*), so the four ref checks here (E_PAIR_REF,
//     E_TRANS_MOTOR_REF, E_TRANS_JOINT_REF, E_MOTOR_SENSOR_REF) are all errors.
//   * A duplicate that changes no meaning is a warning when the construct is symmetric/idempotent
//     (W_PAIR_SELF a no-op self-pair, W_PAIR_DUP the same unordered pair twice).
use crate::model::VisualAppearance;

/// Full schema-coverage validation of a parsed [`Hcdf`] document: the referential/uniqueness batch plus the
/// separator-unification guards. Emits the leaf-name separator (`E_NAME_SEPARATOR`), visual submesh
/// selection (`E_MODEL_SUBMESH_MIX`/`E_MODEL_SUBMESH_CENTER`), self-collision pair
/// (`E_PAIR_REF`/`W_PAIR_SELF`/`W_PAIR_DUP`), transmission
/// endpoint (`E_TRANS_MOTOR_REF`/`E_TRANS_MOTOR_AMBIGUOUS`/`E_TRANS_JOINT_REF`), motor sensor
/// (`E_MOTOR_SENSOR_REF`) issues. Complements [`validate_semantic`] (kinematic),
/// [`validate_network`] (comms), and [`validate_loops`] (closure); carries no Python oracle. Returns the
/// issues in document order.
pub fn validate_coverage(doc: &Hcdf) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let comp_names: BTreeSet<&str> = doc.comp.iter().map(|c| c.name.as_str()).collect();
    let joint_names: BTreeSet<&str> = doc.joint.iter().filter_map(|j| j.name.as_deref()).collect();
    // Motor/sensor names are scoped to their comp. `motor_names` is the flattened set (every motor name
    // on any comp) for a BARE transmission ref; `motors_by_comp` keeps the per-comp scoping for a
    // slash-path "comp/motor" ref; `sensors_by_comp` scopes the motor sensor refs to the SAME comp. Both
    // @names are `Option` in the model (XSD-required in practice); an unnamed motor/sensor cannot target.
    let mut motors_by_comp: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    // How many DISTINCT comps carry each motor name; drives the bare-ref disambiguation: a bare motor
    // ref resolves iff its name appears on exactly one comp; a name on >1 comp is `E_TRANS_MOTOR_AMBIGUOUS`.
    let mut motor_name_comp_count: BTreeMap<&str, usize> = BTreeMap::new();
    let mut sensors_by_comp: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for c in &doc.comp {
        let ms = motors_by_comp.entry(c.name.as_str()).or_default();
        for m in &c.motor {
            if let Some(n) = m.name.as_deref() {
                // `insert` is true only on this comp's FIRST motor of that name, so the counter tallies
                // comps-per-name (not motors-per-name): two same-named motors on one comp count once.
                if ms.insert(n) {
                    *motor_name_comp_count.entry(n).or_default() += 1;
                }
            }
        }
        let ss = sensors_by_comp.entry(c.name.as_str()).or_default();
        for s in &c.sensor {
            if let Some(n) = s.name.as_deref() {
                ss.insert(n);
            }
        }
    }

    check_name_separators(doc, &mut issues);
    check_model_submeshes(doc, &mut issues);
    check_self_collision_pairs(doc, &comp_names, &mut issues);
    check_transmissions(
        doc,
        &joint_names,
        &motor_name_comp_count,
        &motors_by_comp,
        &mut issues,
    );
    check_motor_sensors(doc, &sensors_by_comp, &mut issues);
    issues
}

/// A motor name containing a separator character (`/` or `.`) breaks legacy transmission reference
/// resolution: a combined ref splits on the last slash to recover the leaf ([`split_ref`]), so a leaf
/// whose own name carried a slash (or the retired dot) would be un-addressable once an include prefix
/// is prepended. Comp names are EXEMPT: an include prefix legitimately builds `"drive1/dm8009p_case"`,
/// and the resolver strips exactly that prefix off the front. Only the LEAF a ref finally names must be
/// separator-free, so `E_NAME_SEPARATOR` guards motor names and keeps rsplit
/// resolution sound under arbitrarily nested includes.
fn check_name_separators(doc: &Hcdf, issues: &mut Vec<Issue>) {
    for c in &doc.comp {
        let mut flag = |kind: &str, name: Option<&str>| {
            if let Some(n) = name {
                if n.contains('/') || n.contains('.') {
                    issues.push(Issue::error(
                        "E_NAME_SEPARATOR",
                        format!(
                            "comp {:?}: {kind} name {n:?} may not contain a separator ('/' or '.')",
                            c.name
                        ),
                    ));
                }
            }
        };
        for m in &c.motor {
            flag("motor", m.name.as_deref());
        }
    }
}

/// Visual submesh selection: the two exclusivity/placement invariants the grammar cannot express
/// (both selector lists are optional/repeated, so the XSD sequence admits states the validator must
/// forbid). `E_MODEL_SUBMESH_MIX`: a `<model>` carries BOTH `<submesh>` (include) and `<exclude-submesh>`
/// children: the two modes are mutually exclusive (a union-OF named subtrees vs the whole model MINUS
/// them), and mixing them in one model is undefined in v1. `E_MODEL_SUBMESH_CENTER`: `@center="true"`
/// sits on anything but a LONE include `<submesh>`: recentering (SDF parity) only means something for a
/// single named subtree drawn on its own; a multi-include union, or an include paired with an exclude,
/// has no single subtree to recenter, so the flag is meaningless there. (`<exclude-submesh>` cannot even
/// spell `@center` (the grammar drops it), so only include submeshes are inspected.) Both are errors: an
/// ambiguous selection would render differently across viewers (which the validator, not the GLB, guards).
fn check_model_submeshes(doc: &Hcdf, issues: &mut Vec<Issue>) {
    for c in &doc.comp {
        for v in &c.visual {
            let VisualAppearance::Model { model, .. } = &v.appearance else {
                continue;
            };
            let label = format!("comp {:?} visual {:?}", c.name, v.name);
            if !model.submesh.is_empty() && !model.exclude_submesh.is_empty() {
                issues.push(Issue::error(
                    "E_MODEL_SUBMESH_MIX",
                    format!(
                        "{label}: a model mixes <submesh> (include) and <exclude-submesh>; the two \
                         selector modes are mutually exclusive"
                    ),
                ));
            }
            // A lone include (exactly one <submesh>, no <exclude-submesh>) is the ONLY place @center
            // means anything; anywhere else a center="true" is meaningless. @center is xs:boolean, so a
            // lexical true is "true" or "1"; "false"/"0" (a no-op) is legal anywhere.
            let lone_include = model.submesh.len() == 1 && model.exclude_submesh.is_empty();
            if !lone_include {
                for s in &model.submesh {
                    if matches!(s.center.as_deref(), Some("true") | Some("1")) {
                        issues.push(Issue::error(
                            "E_MODEL_SUBMESH_CENTER",
                            format!(
                                "{label}: submesh {:?} sets @center outside a lone include (center is \
                                 legal only on a single <submesh> with no <exclude-submesh>)",
                                s.name.as_deref().unwrap_or("")
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// `<self-collision-disable>` `<pair>` integrity: each `@comp1`/`@comp2` must name an existing comp
/// (`E_PAIR_REF`); a pair naming one comp on both sides is a no-op (`W_PAIR_SELF`); and the same
/// UNORDERED pair listed twice is redundant (`W_PAIR_DUP`); collision disabling is symmetric, so
/// {a, b} and {b, a} are the same exclusion. Duplicate detection is independent of ref resolution (a
/// repeat is a repeat whether or not the comps exist).
fn check_self_collision_pairs(doc: &Hcdf, comp_names: &BTreeSet<&str>, issues: &mut Vec<Issue>) {
    let Some(scd) = &doc.self_collision_disable else {
        return;
    };
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut dup: BTreeSet<(&str, &str)> = BTreeSet::new();
    for pair in &scd.pair {
        // XSD requires both @comp1 and @comp2; a partial pair cannot form a reference or a key, so skip it.
        let (Some(a), Some(b)) = (pair.comp1.as_deref(), pair.comp2.as_deref()) else {
            continue;
        };
        for name in [a, b] {
            if !comp_names.contains(name) {
                issues.push(Issue::error(
                    "E_PAIR_REF",
                    format!(
                        "self-collision-disable pair references comp {name:?}, which is not a comp{}",
                        sep_hint(name)
                    ),
                ));
            }
        }
        if a == b {
            issues.push(Issue::warning(
                "W_PAIR_SELF",
                format!("self-collision-disable pair names comp {a:?} on both sides (a no-op)"),
            ));
        }
        let key = if a <= b { (a, b) } else { (b, a) };
        if !seen.insert(key) {
            dup.insert(key);
        }
    }
    for (a, b) in dup {
        issues.push(Issue::warning(
            "W_PAIR_DUP",
            format!(
                "self-collision-disable lists the unordered pair ({a:?}, {b:?}) more than once"
            ),
        ));
    }
}

/// `<transmission>` endpoint `@ref` resolution. A `<motor>` endpoint resolves against the comp-
/// scoped motor names in EITHER form the schema/corpus uses: a slash-path `"comp/motor"` (split on the
/// LAST slash, scoped to that comp) or a BARE motor name (matched against every comp's motor scope); an
/// unresolved ref is `E_TRANS_MOTOR_REF`. A dot is not a separator, so a dotted ref splits to nothing and
/// dangles (with the [`sep_hint`] nudge), and a bare ref that matches motors on MORE THAN ONE comp is the
/// uninterpretable `E_TRANS_MOTOR_AMBIGUOUS` (the doc-level backstop for the include-composition case
/// where N instances of one motorized module each carry a `foc_motor`, so the bare ref must be qualified
/// to `"comp/foc_motor"`). A `<joint>` endpoint resolves against the DOC-level joint names (joints are
/// document-scoped, so the ref is a bare joint name); an unresolved ref is `E_TRANS_JOINT_REF`. The
/// optional `@role` (reference/driven/input/output) only disambiguates an endpoint's PART in a multi-
/// endpoint coupling (gearbox, differential); it never changes WHICH namespace `@ref` resolves against
/// (the element name motor/joint does), so it plays no part here; its value is enum-checked by
/// [`validate_enums`] via `ENUM_ATTRS`.
fn check_transmissions(
    doc: &Hcdf,
    joint_names: &BTreeSet<&str>,
    motor_name_comp_count: &BTreeMap<&str, usize>,
    motors_by_comp: &BTreeMap<&str, BTreeSet<&str>>,
    issues: &mut Vec<Issue>,
) {
    for t in &doc.transmission {
        let tname = t.name.as_deref().unwrap_or("");
        for m in &t.motor {
            // XSD requires @ref; a ref-less endpoint has nothing to resolve, so skip (defensive).
            let Some(r) = m.ref_.as_deref() else {
                continue;
            };
            let dangling = |issues: &mut Vec<Issue>| {
                issues.push(Issue::error(
                    "E_TRANS_MOTOR_REF",
                    format!(
                        "transmission {tname:?}: motor endpoint {r:?} is not a motor{}",
                        sep_hint(r)
                    ),
                ));
            };
            match r.rsplit_once('/') {
                // Slash-path "comp/motor": scoped to the named comp, unambiguous by construction.
                Some((comp, motor)) => {
                    if !motors_by_comp.get(comp).is_some_and(|s| s.contains(motor)) {
                        dangling(issues);
                    }
                }
                // Bare ref: resolves iff the name is on exactly one comp; zero comps dangles, and more
                // than one is ambiguous (needs the "comp/" qualifier).
                None => match motor_name_comp_count.get(r).copied().unwrap_or(0) {
                    0 => dangling(issues),
                    1 => {}
                    _ => issues.push(Issue::error(
                        "E_TRANS_MOTOR_AMBIGUOUS",
                        format!(
                            "transmission {tname:?}: bare motor endpoint {r:?} matches motors on more \
                             than one comp; qualify it as \"comp/{r}\""
                        ),
                    )),
                },
            }
        }
        for j in &t.joint {
            let Some(r) = j.ref_.as_deref() else {
                continue;
            };
            if !joint_names.contains(r) {
                issues.push(Issue::error(
                    "E_TRANS_JOINT_REF",
                    format!(
                        "transmission {tname:?}: joint endpoint {r:?} is not a joint{}",
                        sep_hint(r)
                    ),
                ));
            }
        }
    }
}

/// `<motor>` `@encoder`/`@hall`/`@thermistor` sensor references. Each, when present, must name a
/// `<sensor>` on the SAME comp as the motor (the XSD documents them as "a sensor on this component").
/// An unresolved ref is `E_MOTOR_SENSOR_REF`, an ERROR, matching every other dangling-reference check
/// in the validator family (E_LOOP_REF, E_TRANS_*, E_PAIR_REF, E_COLOR_REF, E_FRAME_RELATIVE_TO,
/// E_GROUP_TIP_*); a motor pointing at a non-existent feedback device is a real integrity fault, not an
/// advisory.
fn check_motor_sensors(
    doc: &Hcdf,
    sensors_by_comp: &BTreeMap<&str, BTreeSet<&str>>,
    issues: &mut Vec<Issue>,
) {
    let empty = BTreeSet::new();
    for c in &doc.comp {
        let sensors = sensors_by_comp.get(c.name.as_str()).unwrap_or(&empty);
        for m in &c.motor {
            let mname = m.name.as_deref().unwrap_or("");
            for (kind, r) in [
                ("encoder", m.encoder.as_deref()),
                ("hall", m.hall.as_deref()),
                ("thermistor", m.thermistor.as_deref()),
            ] {
                let Some(r) = r else {
                    continue;
                };
                if !sensors.contains(r) {
                    issues.push(Issue::error(
                        "E_MOTOR_SENSOR_REF",
                        format!(
                            "comp {:?} motor {mname:?}: {kind} {r:?} is not a sensor on this comp{}",
                            c.name,
                            sep_hint(r)
                        ),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{canon_cycle, py_float_repr};

    /// `py_float_repr` reproduces CPython's `str(float(...))` byte-for-byte, including the
    /// scientific-notation thresholds (decimal exp `< -4` or `>= 16`), signed >= 2-digit exponents,
    /// the integral `.0` suffix, the `1e16` boundary, signed zero, and non-finite values. The
    /// expected strings were produced by CPython `str(float(...))`. Random-corpus parity (~400k
    /// doubles) is verified out-of-band; these pin the boundaries the docstring claims.
    #[test]
    fn py_float_repr_matches_cpython_str_float() {
        let cases: &[(f64, &str)] = &[
            (1.0, "1.0"),
            (100.0, "100.0"),
            (4.25, "4.25"),
            (0.1, "0.1"),
            // fixed/scientific boundary at 1e16 (1e15 stays fixed, 1e16 flips to sci).
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.2345678901234568e16, "1.2345678901234568e+16"),
            (1e20, "1e+20"),
            (1e100, "1e+100"),
            (-1e20, "-1e+20"),
            // small-magnitude boundary at 1e-4 (1e-4 stays fixed, 1e-5 flips to sci with -05).
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1.5e-10, "1.5e-10"),
            (5e-324, "5e-324"), // smallest positive subnormal
            // signed zero
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            // overflow/underflow of a parsed literal -> the parsed f64 (inf / 0.0) is what Python prints
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            ("1e400".parse().unwrap(), "inf"),
            ("1e-400".parse().unwrap(), "0.0"),
        ];
        for (v, want) in cases {
            assert_eq!(
                &py_float_repr(*v),
                want,
                "py_float_repr({v:?}) should equal CPython str(float)"
            );
        }
        assert_eq!(py_float_repr(f64::NAN), "nan");
    }

    /// `canon_cycle` collapses every DFS rotation (and any non-cycle tail prefix) of the SAME cycle to
    /// one canonical string: the property that makes `E_CYCLE` seed-independent and matches Python's
    /// `_canon_cycle`. The canonical form rotates the cycle to its lexicographically-min node.
    #[test]
    fn canon_cycle_is_rotation_invariant() {
        // a 3-cycle a->b->c->a, however the DFS happened to enter it.
        for raw in [
            vec!["a", "b", "c", "a"],
            vec!["c", "a", "b", "c"],
            vec!["b", "c", "a", "b"],
            vec!["x", "a", "b", "c", "a"], // with a non-cycle tail prefix x -> ...
        ] {
            assert_eq!(canon_cycle(&raw), "a -> b -> c -> a", "raw={raw:?}");
        }
        // a 2-cycle.
        assert_eq!(canon_cycle(&["a", "b", "a"]), "a -> b -> a");
        assert_eq!(canon_cycle(&["b", "a", "b"]), "a -> b -> a");
        // a self-loop (parent==child non-loop joint is E_SELF_JOINT, but a 1-node back edge canonizes).
        assert_eq!(canon_cycle(&["s", "s"]), "s -> s");
    }
}
