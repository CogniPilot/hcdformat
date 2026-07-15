//! XSD-driven, bidirectional HCDF XML <-> JSON conversion (feature `json`).
//!
//! A byte-for-byte Rust port of the pure-Python authority `hcdf/convert.py` (+ `hcdf_io/json_io.py`):
//! it reproduces that converter's mapping conventions EXACTLY so existing `.json` files stay compatible
//! (JSON is a DERIVED view of the sha-pinned XSD, never a second schema). It CLOSES the JSON Rust-gap
//! while `convert.py` stays in place as the differential oracle (removed once byte-parity is proven).
//!
//! Mapping conventions (the spec, from `convert.py`):
//!   * XML attributes  -> direct JSON object keys (NO `@` prefix).
//!   * child elements   -> nested objects; multiple same-named children -> a JSON ARRAY (the array is
//!     decided by the RUNTIME child multiplicity, exactly as `convert.py::_count_child_tags` does, NOT
//!     from `maxOccurs`; that is only the docstring's intent, the code counts occurrences).
//!   * text content     -> `"$text"` key; a leaf text/attribute is passed through [`try_numeric`]
//!     (`convert.py::_try_numeric`): space-separated vectors and pose strings stay STRINGS, `true`/`false`
//!     become booleans, plain integers/decimals become numbers, hex / dotted-version / `.`-leading /
//!     `e`-notation strings stay STRINGS.
//!   * JSON -> XML is XSD-DRIVEN: the frozen embedded `hcdf.xsd` (reused via [`crate::schema`], NOT
//!     re-embedded) is introspected once into an [`XsdInfo`] so the same element name under different
//!     parent types resolves to its correct type, and a key is emitted as an ATTRIBUTE vs a CHILD element
//!     from that type: the port of `convert.py::ContextAwareJsonToXml`.
//!
//! Pure-Rust and wasm-clean: only `quick-xml` + `serde_json` (both already base deps) and the embedded
//! schema bytes. `from_json(to_json(x))` round-trips the same document.

use crate::error::{Error, Result};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::OnceLock;

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// Ordered JSON value model
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// A JSON value that PRESERVES object key insertion order (a `Vec` of pairs, not a map) and keeps the
/// Python `int` vs `float` distinction: both are load-bearing for byte-parity with `convert.py`'s
/// `json.dump(..., indent=2, ensure_ascii=False)` output. Built from XML on the read path and parsed
/// (order-preservingly, via the manual [`Deserialize`] below) from JSON on the write path.
#[derive(Debug, Clone, PartialEq)]
enum JVal {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<JVal>),
    Obj(Vec<(String, JVal)>),
}

// serde_json feeds map entries to a Visitor in DOCUMENT order regardless of the target type, so a manual
// Visitor collecting into a `Vec<(String, JVal)>` preserves key order WITHOUT the `preserve_order`
// feature (which would change serde_json crate-wide). Integer/float variants are chosen by which visit_*
// serde_json dispatches to, mirroring the Python `int`/`float` a JSON literal parses to.
impl<'de> Deserialize<'de> for JVal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = JVal;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_bool<E>(self, v: bool) -> std::result::Result<JVal, E> {
                Ok(JVal::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> std::result::Result<JVal, E> {
                Ok(JVal::Int(v))
            }
            fn visit_u64<E>(self, v: u64) -> std::result::Result<JVal, E> {
                // Fits i64 -> stay an integer; otherwise widen to float (HCDF never carries such values).
                Ok(if v <= i64::MAX as u64 {
                    JVal::Int(v as i64)
                } else {
                    JVal::Float(v as f64)
                })
            }
            fn visit_f64<E>(self, v: f64) -> std::result::Result<JVal, E> {
                Ok(JVal::Float(v))
            }
            fn visit_str<E>(self, v: &str) -> std::result::Result<JVal, E> {
                Ok(JVal::Str(v.to_string()))
            }
            fn visit_none<E>(self) -> std::result::Result<JVal, E> {
                Ok(JVal::Null)
            }
            fn visit_unit<E>(self) -> std::result::Result<JVal, E> {
                Ok(JVal::Null)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> std::result::Result<JVal, D::Error> {
                d.deserialize_any(V)
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<JVal, A::Error> {
                let mut out = Vec::new();
                while let Some(e) = seq.next_element()? {
                    out.push(e);
                }
                Ok(JVal::Arr(out))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<JVal, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, JVal>()? {
                    out.push((k, v));
                }
                Ok(JVal::Obj(out))
            }
        }
        d.deserialize_any(V)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// A tiny generic XML DOM (shared by the XML->JSON read path and the XSD introspection)
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// One XML element: its LOCAL name (namespace prefix stripped, like `lxml`'s `QName.localname`), its
/// attributes in source order (localname + entity-unescaped value), its child ELEMENTS in order, and the
/// raw leading text (the `lxml` `.text` semantics: text before the first child element; stripped when
/// consumed). Comments/PIs are dropped, matching `convert.py`.
struct Dom {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Dom>,
    text: String,
}

impl Dom {
    /// First attribute whose localname equals `key` (the `lxml` `Element.get` semantics used by the XSD
    /// walk).
    fn get(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    /// First direct child element with local name `local` (the `lxml` `Element.find` used by the XSD
    /// walk).
    fn find(&self, local: &str) -> Option<&Dom> {
        self.children.iter().find(|c| c.name == local)
    }
}

/// Parse an XML string into the generic [`Dom`] tree with `quick-xml`. Only well-formedness matters here
/// (schema validation is a separate layer); attribute values and text are entity-unescaped so the JSON
/// carries the DECODED content, exactly as `lxml` hands it to `convert.py`.
fn parse_dom(xml: &str) -> Result<Dom> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<Dom> = Vec::new();
    let mut root: Option<Dom> = None;

    fn attach(stack: &mut [Dom], root: &mut Option<Dom>, d: Dom) {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(d);
        } else {
            *root = Some(d);
        }
    }

    fn start_dom(e: &quick_xml::events::BytesStart<'_>) -> Result<Dom> {
        let name = String::from_utf8_lossy(e.name().local_name().as_ref()).into_owned();
        let mut attrs = Vec::new();
        for a in e.attributes() {
            let a = a.map_err(|e| Error::Xml(e.to_string()))?;
            let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
            let val = a
                .unescape_value()
                .map_err(|e| Error::Xml(e.to_string()))?
                .into_owned();
            attrs.push((key, val));
        }
        Ok(Dom {
            name,
            attrs,
            children: Vec::new(),
            text: String::new(),
        })
    }

    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) => stack.push(start_dom(&e)?),
            Event::Empty(e) => {
                let d = start_dom(&e)?;
                attach(&mut stack, &mut root, d);
            }
            Event::End(_) => {
                let d = stack
                    .pop()
                    .ok_or_else(|| Error::Xml("unbalanced end tag".into()))?;
                attach(&mut stack, &mut root, d);
            }
            Event::Text(t) => {
                // `lxml` `.text` is only the text BEFORE the first child element, so capture text only
                // while the current element still has no children.
                if let Some(top) = stack.last_mut() {
                    if top.children.is_empty() {
                        let s = t.unescape().map_err(|e| Error::Xml(e.to_string()))?;
                        top.text.push_str(&s);
                    }
                }
            }
            Event::Eof => break,
            _ => {} // Decl / Comment / PI / DocType: dropped, as `convert.py` drops comments/PIs.
        }
    }
    root.ok_or_else(|| Error::Xml("no root element".into()))
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// XML -> JSON
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// Strip surrounding whitespace from element text; empty -> `None` (`convert.py::_text_value`).
fn text_value(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Interpret a leaf string as bool/int/float, else keep it a string: a faithful port of
/// `convert.py::_try_numeric` (the check ORDER is load-bearing for byte-parity):
///   1. a space in the trimmed value (a vector / pose string) -> keep string;
///   2. a `0x`/`0X` prefix (hex) -> keep string;
///   3. more than one `.` (a dotted version like `1.0.3`) -> keep string;
///   4. `true`/`false` -> bool;
///   5. an integer whose canonical form round-trips the original (guards `007`, `+5`) -> int;
///   6. a float that does NOT start with `.` and has no `e`/`E` -> float;
///   7. otherwise the original string.
fn try_numeric(value: &str) -> JVal {
    if value.trim().contains(' ') {
        return JVal::Str(value.to_string());
    }
    if value.starts_with("0x") || value.starts_with("0X") {
        return JVal::Str(value.to_string());
    }
    if value.matches('.').count() > 1 {
        return JVal::Str(value.to_string());
    }
    if value == "true" {
        return JVal::Bool(true);
    }
    if value == "false" {
        return JVal::Bool(false);
    }
    // Integer: accept only if formatting the parsed value reproduces the ORIGINAL text (so `007`, `+5`,
    // `-0` fall through, matching Python's `str(int(value)) == value` guard).
    if let Ok(i) = value.parse::<i64>() {
        if i.to_string() == value {
            return JVal::Int(i);
        }
    }
    // Float: Python floats anything `float()` accepts that does not start with `.` and has no `e`. We
    // additionally require finiteness so pathological `inf`/`nan` stay strings (they would otherwise
    // serialize to invalid JSON); such inputs never occur in HCDF.
    if let Ok(f) = value.parse::<f64>() {
        if f.is_finite() && !value.starts_with('.') && !value.contains(['e', 'E']) {
            return JVal::Float(f);
        }
    }
    JVal::Str(value.to_string())
}

/// Recursively convert one [`Dom`] element to a [`JVal`] (`convert.py::xml_element_to_json`): a leaf with
/// no attributes and no children collapses to its scalar value (or `null`); otherwise an object carrying
/// attributes first, then `$text`, then child elements in document order; a tag seen more than once
/// becoming an array anchored at its FIRST occurrence.
fn element_to_json(el: &Dom) -> JVal {
    let text = text_value(&el.text);
    let has_children = !el.children.is_empty();
    let has_attrs = !el.attrs.is_empty();

    if !has_children && !has_attrs {
        return match text {
            Some(t) => try_numeric(&t),
            None => JVal::Null,
        };
    }

    let mut obj: Vec<(String, JVal)> = Vec::new();
    for (k, v) in &el.attrs {
        obj.push((k.clone(), try_numeric(v)));
    }
    if let Some(t) = &text {
        obj.push(("$text".to_string(), try_numeric(t)));
    }

    // Count child tags to decide array vs scalar (runtime multiplicity, per `_count_child_tags`).
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for c in &el.children {
        *counts.entry(c.name.as_str()).or_insert(0) += 1;
    }
    // Track the obj index where each array-valued tag was first placed, so repeats append in order.
    let mut array_at: HashMap<&str, usize> = HashMap::new();
    for c in &el.children {
        let cv = element_to_json(c);
        if counts.get(c.name.as_str()).copied().unwrap_or(0) > 1 {
            if let Some(&idx) = array_at.get(c.name.as_str()) {
                if let JVal::Arr(a) = &mut obj[idx].1 {
                    a.push(cv);
                }
            } else {
                let idx = obj.len();
                array_at.insert(c.name.as_str(), idx);
                obj.push((c.name.clone(), JVal::Arr(vec![cv])));
            }
        } else {
            obj.push((c.name.clone(), cv));
        }
    }
    JVal::Obj(obj)
}

/// Convert an HCDF XML document string to its JSON view (`{ "<root>": ... }`), formatted byte-for-byte
/// like `convert.py`'s `json.dump(..., indent=2, ensure_ascii=False)` with a trailing newline.
pub fn hcdf_xml_to_json(xml: &str) -> Result<String> {
    let dom = parse_dom(xml)?;
    let root_val = element_to_json(&dom);
    let doc = JVal::Obj(vec![(dom.name.clone(), root_val)]);
    Ok(to_json_string(&doc))
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// JSON serialization (Python `json.dump(indent=2, ensure_ascii=False)` parity)
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// Serialize a [`JVal`] as Python's `json.dump(obj, indent=2, ensure_ascii=False)` would, plus a trailing
/// newline: 2-space indent, `", "`/`": "` separators collapsed to `,\n`/`: ` under indent mode, `{}`/`[]`
/// for empties, non-ASCII passed through literally.
fn to_json_string(v: &JVal) -> String {
    let mut s = String::new();
    write_json(v, 0, &mut s);
    s.push('\n');
    s
}

fn write_json(v: &JVal, indent: usize, out: &mut String) {
    match v {
        JVal::Null => out.push_str("null"),
        JVal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JVal::Int(i) => out.push_str(&i.to_string()),
        JVal::Float(f) => out.push_str(&fmt_float(*f)),
        JVal::Str(s) => write_json_str(s, out),
        JVal::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            let pad = "  ".repeat(indent + 1);
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad);
                write_json(item, indent + 1, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(indent));
            out.push(']');
        }
        JVal::Obj(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let pad = "  ".repeat(indent + 1);
            for (i, (k, val)) in pairs.iter().enumerate() {
                out.push_str(&pad);
                write_json_str(k, out);
                out.push_str(": ");
                write_json(val, indent + 1, out);
                if i + 1 < pairs.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(indent));
            out.push('}');
        }
    }
}

/// Write a JSON string literal with Python's `ensure_ascii=False` escaping: `\"`, `\\`, the short
/// escapes for `\b \f \n \r \t`, `\u00XX` for the remaining C0 controls, and every other character
/// (including all non-ASCII) passed through verbatim. `/` is NOT escaped.
fn write_json_str(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Format an `f64` as Python `repr(float)` / `str(float)` does for the plain-decimal range HCDF uses:
/// shortest round-trip digits (Rust's `{}` and CPython's dtoa agree; the shortest decimal is unique),
/// with a trailing `.0` appended to an integral value so it stays a float (`50.0`, not `50`). NOTE:
/// Rust's `{}` never switches to exponential form, so a value Python would render in `e` notation (only
/// possible for magnitudes `>= 1e16` or `< 1e-4`, which `try_numeric` never produces from a plain
/// decimal) would differ, outside the corpus's range.
fn fmt_float(f: f64) -> String {
    let mut s = format!("{f}");
    if !s.contains(['.', 'e', 'E']) {
        s.push_str(".0");
    }
    s
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// XSD introspection (for context-aware JSON -> XML): port of `convert.py::XsdInfo` / `_build_xsd_info`
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// Rich XSD type context: which attributes and child elements each complexType declares, its base type
/// (from `xs:extension`), whether it is `mixed`, and the global element -> type map. Carries full context
/// so the same element name under different parent types resolves to different types with different
/// attribute sets: the port of `convert.py::XsdInfo`.
#[derive(Default)]
struct XsdInfo {
    type_attrs: HashMap<String, HashSet<String>>,
    type_children: HashMap<String, HashMap<String, String>>,
    type_bases: HashMap<String, String>,
    elem_type_map: HashMap<String, String>,
    type_mixed: HashMap<String, bool>,
}

impl XsdInfo {
    /// The type of `child_tag` within `parent_type`, walking the `xs:extension` base chain
    /// (`XsdInfo.child_type`). `Some("")` is a real result (an inline-simpleType / untyped element).
    fn child_type(&self, parent_type: &str, child_tag: &str) -> Option<String> {
        if let Some(children) = self.type_children.get(parent_type) {
            if let Some(ct) = children.get(child_tag) {
                return Some(ct.clone());
            }
        }
        if let Some(base) = self.type_bases.get(parent_type) {
            return self.child_type(base, child_tag);
        }
        None
    }

    /// The resolved attribute set for a type, including inherited attributes (`XsdInfo.attrs_for_type`).
    fn attrs_for_type(&self, type_name: &str) -> HashSet<String> {
        let mut attrs = self.type_attrs.get(type_name).cloned().unwrap_or_default();
        if let Some(base) = self.type_bases.get(type_name) {
            attrs.extend(self.attrs_for_type(base));
        }
        attrs
    }

    /// A type is attribute-only iff it has attributes but no (post-inheritance) child elements and is not
    /// `mixed`: the shape that makes a scalar JSON value serialize to `<tag attr="value"/>`
    /// (`XsdInfo.is_attr_only`; the `mixed`/children checks are on the type DIRECTLY, matching Python).
    fn is_attr_only(&self, type_name: &str) -> bool {
        let attrs = self.attrs_for_type(type_name);
        if attrs.is_empty() {
            return false;
        }
        let has_children = self
            .type_children
            .get(type_name)
            .is_some_and(|c| !c.is_empty());
        let is_mixed = self.type_mixed.get(type_name).copied().unwrap_or(false);
        !has_children && !is_mixed
    }
}

/// Extract attributes / child elements / mixed / base from one `xs:complexType` (or an inline one) into
/// `info` under `type_name`: `convert.py::_build_xsd_info._collect_type_info`.
fn collect_type_info(ct: &Dom, type_name: &str, info: &mut XsdInfo) {
    if ct.get("mixed") == Some("true") {
        info.type_mixed.insert(type_name.to_string(), true);
    }
    info.type_attrs.entry(type_name.to_string()).or_default();
    info.type_children.entry(type_name.to_string()).or_default();
    walk_attrs_children(ct, type_name, info);
}

/// Walk a complexType's content model collecting attribute decls and element decls under `owner`,
/// descending through `sequence`/`all`/`choice` and `complexContent`/`simpleContent` extensions (whose
/// `base` sets `owner`'s base type): `_collect_type_info._walk_for_attrs_and_children`.
fn walk_attrs_children(node: &Dom, owner: &str, info: &mut XsdInfo) {
    for child in &node.children {
        match child.name.as_str() {
            "attribute" => {
                if let Some(an) = child.get("name") {
                    info.type_attrs
                        .entry(owner.to_string())
                        .or_default()
                        .insert(an.to_string());
                }
            }
            "element" => {
                let en = child.get("name").map(str::to_string);
                let et = child.get("type").unwrap_or("");
                match en {
                    Some(en) if !et.is_empty() => {
                        info.type_children
                            .entry(owner.to_string())
                            .or_default()
                            .insert(en, et.to_string());
                    }
                    Some(en) => {
                        if let Some(inline) = child.find("complexType") {
                            let synth = format!("__{en}__");
                            info.type_children
                                .entry(owner.to_string())
                                .or_default()
                                .insert(en.clone(), synth.clone());
                            collect_type_info(inline, &synth, info);
                            info.elem_type_map.insert(en, synth);
                        } else {
                            // Inline simpleType or no type -> the empty-string type (a leaf string).
                            info.type_children
                                .entry(owner.to_string())
                                .or_default()
                                .insert(en, String::new());
                        }
                    }
                    None => {}
                }
            }
            "sequence" | "all" | "choice" => walk_attrs_children(child, owner, info),
            "complexContent" | "simpleContent" => {
                if let Some(ext) = child.find("extension") {
                    if let Some(base) = ext.get("base") {
                        if !base.is_empty() {
                            info.type_bases.insert(owner.to_string(), base.to_string());
                        }
                    }
                    walk_attrs_children(ext, owner, info);
                }
            }
            _ => {}
        }
    }
}

/// Build the full [`XsdInfo`] from a parsed `xs:schema` root (`convert.py::_build_xsd_info`): index named
/// complexTypes, then global elements (with inline complexTypes -> synthetic `__name__` types), then
/// resolve inheritance by merging each base type's children into its derived types to a fixpoint.
fn build_xsd_info(root: &Dom) -> XsdInfo {
    let mut info = XsdInfo::default();

    for ct in root.children.iter().filter(|c| c.name == "complexType") {
        if let Some(name) = ct.get("name").map(str::to_string) {
            collect_type_info(ct, &name, &mut info);
        }
    }

    for el in root.children.iter().filter(|c| c.name == "element") {
        let name = el.get("name").map(str::to_string);
        let typ = el.get("type").map(str::to_string);
        if let (Some(n), Some(t)) = (&name, &typ) {
            info.elem_type_map.insert(n.clone(), t.clone());
        }
        if let Some(n) = name {
            if let Some(inline) = el.find("complexType") {
                let synth = format!("__{n}__");
                info.elem_type_map.insert(n, synth.clone());
                collect_type_info(inline, &synth, &mut info);
            }
        }
    }

    // Merge base type children into derived types (max 10 levels of inheritance depth).
    let all: Vec<String> = info.type_children.keys().cloned().collect();
    for _ in 0..10 {
        let mut changed = false;
        for tn in &all {
            let Some(base) = info.type_bases.get(tn).cloned() else {
                continue;
            };
            let base_children = info.type_children.get(&base).cloned().unwrap_or_default();
            let entry = info.type_children.entry(tn.clone()).or_default();
            for (k, v) in base_children {
                if let std::collections::hash_map::Entry::Vacant(slot) = entry.entry(k) {
                    slot.insert(v);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    info
}

/// The process-wide, once-built introspection of the FROZEN embedded `hcdf.xsd` (reused from
/// [`crate::schema::HCDF_XSD_1_0`], never re-embedded). The schema is sha-pinned and immutable, so a
/// single parse+build is correct and shared across every conversion. Panics only if the frozen schema
/// fails to parse, an impossible state in a correctly-built crate.
fn xsd_info() -> &'static XsdInfo {
    static INFO: OnceLock<XsdInfo> = OnceLock::new();
    INFO.get_or_init(|| {
        let src = std::str::from_utf8(crate::schema::HCDF_XSD_1_0)
            .expect("embedded hcdf.xsd is valid UTF-8");
        let dom = parse_dom(src).expect("embedded hcdf.xsd parses as XML");
        build_xsd_info(&dom)
    })
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// JSON -> XML (XSD-aware): port of `convert.py::ContextAwareJsonToXml`
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// A built XML element ready to serialize (attributes in order, optional text, child elements in order).
struct XmlEl {
    name: String,
    attrs: Vec<(String, String)>,
    text: Option<String>,
    children: Vec<XmlEl>,
}

/// A JSON value as XML text/attribute content (`convert.py::_to_str`): booleans as `true`/`false`, null
/// as empty, integers/floats via their Python-string form, strings verbatim. (`Arr`/`Obj` never reach
/// here in practice.)
fn to_str(v: &JVal) -> String {
    match v {
        JVal::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        JVal::Null => String::new(),
        JVal::Int(i) => i.to_string(),
        JVal::Float(f) => fmt_float(*f),
        JVal::Str(s) => s.clone(),
        JVal::Arr(_) | JVal::Obj(_) => String::new(),
    }
}

/// The scalar-string form of a value that `isinstance(value, (str, int, float, bool))` accepts (i.e. NOT
/// null / array / object): used by the attribute-only branch below.
fn scalar_str(v: &JVal) -> Option<String> {
    match v {
        JVal::Bool(_) | JVal::Int(_) | JVal::Float(_) | JVal::Str(_) => Some(to_str(v)),
        JVal::Null | JVal::Arr(_) | JVal::Obj(_) => None,
    }
}

/// Resolve the XSD type of `tag` given `parent_type` context, else the global element map
/// (`ContextAwareJsonToXml.resolve_type`).
fn resolve_type(xsd: &XsdInfo, tag: &str, parent_type: Option<&str>) -> Option<String> {
    if let Some(pt) = parent_type {
        if let Some(ct) = xsd.child_type(pt, tag) {
            return Some(ct);
        }
    }
    xsd.elem_type_map.get(tag).cloned()
}

/// Convert a JSON value to an [`XmlEl`] with context-aware type resolution: the port of
/// `ContextAwareJsonToXml.write_element` (the XSD is always present here, so the no-XSD fallback branch
/// of the Python original does not apply).
fn write_element(xsd: &XsdInfo, tag: &str, value: &JVal, parent_type: Option<&str>) -> XmlEl {
    let my_type = resolve_type(xsd, tag, parent_type);

    // Attribute-only type receiving a SCALAR -> `<tag attr="value"/>` (its single declared attribute).
    if let Some(mt) = &my_type {
        if xsd.is_attr_only(mt) {
            if let Some(sv) = scalar_str(value) {
                let attrs = xsd.attrs_for_type(mt);
                if let Some(attr_name) = attrs.iter().next() {
                    return XmlEl {
                        name: tag.to_string(),
                        attrs: vec![(attr_name.clone(), sv)],
                        text: None,
                        children: Vec::new(),
                    };
                }
            }
        }
    }

    match value {
        JVal::Obj(pairs) => {
            let my_attrs = my_type
                .as_deref()
                .map(|mt| xsd.attrs_for_type(mt))
                .unwrap_or_default();
            let mut xml_attrs: Vec<(String, String)> = Vec::new();
            let mut xml_children: Vec<(&str, &JVal)> = Vec::new();
            let mut text_content: Option<String> = None;

            for (key, val) in pairs {
                if key == "$text" {
                    text_content = Some(to_str(val));
                } else if matches!(val, JVal::Arr(_) | JVal::Obj(_)) {
                    xml_children.push((key.as_str(), val));
                } else if my_attrs.contains(key) {
                    xml_attrs.push((key.clone(), to_str(val)));
                } else {
                    xml_children.push((key.as_str(), val));
                }
            }

            let mut children: Vec<XmlEl> = Vec::new();
            for (child_tag, child_val) in xml_children {
                match child_val {
                    JVal::Arr(items) => {
                        for item in items {
                            children.push(write_element(xsd, child_tag, item, my_type.as_deref()));
                        }
                    }
                    JVal::Obj(_) => {
                        children.push(write_element(xsd, child_tag, child_val, my_type.as_deref()));
                    }
                    _ => {
                        // Scalar child element -> `<child_tag>text</child_tag>` (or empty on null).
                        let text = match child_val {
                            JVal::Null => None,
                            other => Some(to_str(other)),
                        };
                        children.push(XmlEl {
                            name: child_tag.to_string(),
                            attrs: Vec::new(),
                            text,
                            children: Vec::new(),
                        });
                    }
                }
            }

            XmlEl {
                name: tag.to_string(),
                attrs: xml_attrs,
                text: text_content,
                children,
            }
        }
        JVal::Arr(items) => {
            // A bare array at this position: Python's `write_element` returns the LAST built element.
            // (The root is never an array; nested arrays are consumed by the Obj branch above.)
            match items.last() {
                Some(last) => write_element(xsd, tag, last, parent_type),
                None => XmlEl {
                    name: tag.to_string(),
                    attrs: Vec::new(),
                    text: None,
                    children: Vec::new(),
                },
            }
        }
        _ => {
            let text = match value {
                JVal::Null => None,
                other => Some(to_str(other)),
            };
            XmlEl {
                name: tag.to_string(),
                attrs: Vec::new(),
                text,
                children: Vec::new(),
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// XML serialization for the JSON -> XML path
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

fn esc_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn esc_attr(s: &str) -> String {
    esc_text(s).replace('"', "&quot;")
}

fn write_el(el: &XmlEl, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    out.push_str(&pad);
    out.push('<');
    out.push_str(&el.name);
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&esc_attr(v));
        out.push('"');
    }
    if el.children.is_empty() && el.text.is_none() {
        out.push_str("/>\n");
    } else if el.children.is_empty() {
        out.push('>');
        out.push_str(&esc_text(el.text.as_deref().unwrap_or("")));
        out.push_str("</");
        out.push_str(&el.name);
        out.push_str(">\n");
    } else {
        out.push('>');
        if let Some(t) = &el.text {
            if !t.is_empty() {
                out.push_str(&esc_text(t));
            }
        }
        out.push('\n');
        for c in &el.children {
            write_el(c, indent + 1, out);
        }
        out.push_str(&pad);
        out.push_str("</");
        out.push_str(&el.name);
        out.push_str(">\n");
    }
}

/// Convert an HCDF JSON document string (a single-key object `{ "<root>": ... }`) back to XML, XSD-driven
/// so keys resolve to attributes vs child elements correctly. The output is a well-formed, 2-space
/// pretty-printed document (`<?xml version='1.0' encoding='UTF-8'?>` header, matching lxml's declaration
/// style) that round-trips through [`hcdf_xml_to_json`] to the same JSON.
pub fn json_to_hcdf_xml(json: &str) -> Result<String> {
    let root: JVal = serde_json::from_str(json).map_err(|e| Error::Json(e.to_string()))?;
    let JVal::Obj(pairs) = &root else {
        return Err(Error::Json(
            "JSON root must be an object with exactly one key (the root element tag)".into(),
        ));
    };
    if pairs.len() != 1 {
        return Err(Error::Json(
            "JSON root must be an object with exactly one key (the root element tag)".into(),
        ));
    }
    let (tag, val) = &pairs[0];
    let el = write_element(xsd_info(), tag, val, None);
    let mut out = String::from("<?xml version='1.0' encoding='UTF-8'?>\n");
    write_el(&el, 0, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_numeric_matches_python_rules() {
        assert_eq!(try_numeric("0 0 0"), JVal::Str("0 0 0".into())); // vector -> string
        assert_eq!(try_numeric("0x601"), JVal::Str("0x601".into())); // hex -> string
        assert_eq!(try_numeric("1.0.3"), JVal::Str("1.0.3".into())); // version -> string
        assert_eq!(try_numeric("true"), JVal::Bool(true));
        assert_eq!(try_numeric("false"), JVal::Bool(false));
        assert_eq!(try_numeric("007"), JVal::Float(7.0)); // int guard fails, float branch floats it
        assert_eq!(try_numeric("24"), JVal::Int(24));
        assert_eq!(try_numeric("-3"), JVal::Int(-3));
        assert_eq!(try_numeric("50.0"), JVal::Float(50.0));
        assert_eq!(try_numeric(".5"), JVal::Str(".5".into())); // leading-dot -> string
        assert_eq!(try_numeric("1.2e-5"), JVal::Str("1.2e-5".into())); // e-notation -> string
        assert_eq!(try_numeric("802.3dm"), JVal::Str("802.3dm".into()));
    }

    #[test]
    fn float_formats_like_python_repr() {
        assert_eq!(fmt_float(50.0), "50.0");
        assert_eq!(fmt_float(0.5), "0.5");
        assert_eq!(fmt_float(-3.5), "-3.5");
        assert_eq!(fmt_float(0.0942), "0.0942");
    }

    #[test]
    fn minimal_xml_json_roundtrips_to_a_fixpoint() {
        let xml = r#"<?xml version="1.0"?>
<hcdf version="1.0" name="t">
  <comp name="base" role="actuator">
    <inertial><mass>0.5</mass></inertial>
    <visual name="v"><pose xyz="0 0 0" rpy="0 0 0"/></visual>
  </comp>
  <color name="rubber" rgba="0.1 0.1 0.1 1.0"/>
</hcdf>
"#;
        let json1 = hcdf_xml_to_json(xml).expect("xml->json");
        let xml2 = json_to_hcdf_xml(&json1).expect("json->xml");
        let json2 = hcdf_xml_to_json(&xml2).expect("xml2->json");
        assert_eq!(json1, json2, "xml->json->xml must reach a JSON fixpoint");
        // version -> the float 1.0 (Python `_try_numeric` floats a single-dot decimal).
        assert!(json1.contains("\"version\": 1.0"), "got: {json1}");
        // A pose string stays a string, not split into an array.
        assert!(json1.contains("\"xyz\": \"0 0 0\""), "got: {json1}");
    }
}
