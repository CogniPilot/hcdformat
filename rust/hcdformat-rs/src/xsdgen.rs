//! XSD-driven source generators (feature `cli`, native-only): the pure-Rust ports of the dev-time
//! Python generators that derived checked-in artifacts from `hcdf.xsd`.
//!
//!   * [`generate_enums_rs`] emits `src/model/enums.rs`: one Rust `enum` per enumerated
//!     `xs:simpleType` (`FromStr`/`Display`/`valid_values` + rustdoc carried from the schema), plus the
//!     `ENUM_ATTRS` residual table the run-time validator walks. The residual is COMPUTED from the model
//!     sources (which enum classes already appear as a typed `Option<..>`/`Vec<..>` field), never
//!     hand-curated, exactly like `generate_hcdf_rs_enums.py`.
//!   * [`generate_json_schema`] emits `hcdf.schema.json` (JSON Schema draft 2020-12): a `$defs` entry
//!     per named type mirroring the XSD structure (enumerations, attribute/element cardinality,
//!     `complexContent` extension inheritance, `xs:choice` → `oneOf`, mixed content → `$text`), a port
//!     of `generate_json_schema.py`.
//!   * [`generate_spec_html`] emits the spec-browser page (`spec.html` / `website/spec/*.html`): an
//!     expandable element/attribute tree per subsystem tab, styled after sdformat.org. The tab layout is
//!     FIXED for the core + stream-profile schemas and AUTO-DISCOVERED for extension schemas; the title
//!     and layout are chosen from the input FILENAME, exactly like `generate_spec_html.py`.
//!   * [`generate_completions_json`] emits `hcdf.completions.json`: a contextual model keyed by
//!     document roots, schema types, and content particles. Repeated local names retain distinct
//!     identities, cardinality, attributes, annotations, and type references.
//!
//! The enums, JSON Schema, and specification generators preserve the bytes of their retired Python
//! originals. The `hcdf.schema.json` path reproduces `json.dump(indent=2, ensure_ascii=False)`
//! (2-space indent, non-ASCII literal, `float.__repr__` for numeric defaults via
//! [`crate::validate::py_float_repr`]); the contextual completions path uses deterministic
//! `json.dump(indent=1, sort_keys=True)` presentation (1-space indent, sorted keys,
//! `ensure_ascii=True`, no trailing newline); the enums
//! path reproduces the generator's line layout, the `~98`-column rustdoc wrapping (counted in `char`s,
//! not bytes), and the emitted header VERBATIM; the spec-HTML path splices the header/footer templates
//! extracted verbatim from the Python around the same tree walk. A `#[cfg(test)]` drift check
//! regenerates the enums / JSON schema / `spec.html` from the frozen embedded schema and asserts
//! byte-equality, so `cargo test` owns drift detection with no Python.
//!
//! XSD parsing is `quick-xml` (an unconditional base dep), so these need no extra feature beyond the
//! `cli` gate that carries the subcommands wiring them (`hcdf regen enums` / `hcdf regen schema`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// A minimal XSD DOM (quick-xml), matching the `lxml` element semantics the Python generators rely on
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// One XSD element: its LOCAL name (namespace prefix stripped, like `lxml`'s `QName.localname`), its
/// attributes in source order (localname + entity-unescaped value), its child ELEMENTS in order, and the
/// leading text (the `lxml` `.text`: text before the first child element, entity-unescaped). Comments /
/// PIs are dropped; no XSD generator input reads them.
#[derive(Debug)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    /// First attribute whose localname equals `key` (`lxml` `Element.get`).
    fn get(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    /// First direct child element with local name `local` (`lxml` `Element.find` of a one-step path).
    fn find(&self, local: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == local)
    }
    /// Every direct child element with local name `local` (`lxml` `Element.findall`).
    fn findall<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == local)
    }
    /// Pre-order DFS over `self` and all descendants (`lxml` `Element.iter`, document order).
    fn descendants<'a>(&'a self, out: &mut Vec<&'a Node>) {
        out.push(self);
        for c in &self.children {
            c.descendants(out);
        }
    }
}

/// Parse an XSD document into the generic [`Node`] tree with `quick-xml`. Only well-formedness matters;
/// attribute values and element text are entity-unescaped so the tree carries the DECODED content,
/// exactly as `lxml` hands it to the Python generators.
fn parse_xsd(xml: &str) -> Result<Node, String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;

    fn attach(stack: &mut [Node], root: &mut Option<Node>, n: Node) {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(n);
        } else {
            *root = Some(n);
        }
    }

    fn start_node(e: &quick_xml::events::BytesStart<'_>) -> Result<Node, String> {
        let name = String::from_utf8_lossy(e.name().local_name().as_ref()).into_owned();
        let mut attrs = Vec::new();
        for a in e.attributes() {
            let a = a.map_err(|e| e.to_string())?;
            let key = String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned();
            let val = a.unescape_value().map_err(|e| e.to_string())?.into_owned();
            attrs.push((key, val));
        }
        Ok(Node {
            name,
            attrs,
            children: Vec::new(),
            text: String::new(),
        })
    }

    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) => stack.push(start_node(&e)?),
            Event::Empty(e) => {
                let n = start_node(&e)?;
                attach(&mut stack, &mut root, n);
            }
            Event::End(_) => {
                let n = stack
                    .pop()
                    .ok_or_else(|| "unbalanced end tag".to_string())?;
                attach(&mut stack, &mut root, n);
            }
            Event::Text(t) => {
                // `lxml` `.text` is only the text BEFORE the first child element; capture while the
                // current element still has no children (accumulate across split text/entity events).
                if let Some(top) = stack.last_mut() {
                    if top.children.is_empty() {
                        top.text.push_str(&t.unescape().map_err(|e| e.to_string())?);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    root.ok_or_else(|| "no root element".to_string())
}

/// The `annotation/documentation` text of `elem`, or `None` when absent. This is the RAW captured text
/// (entity-unescaped); the two generators normalize it differently (see [`enum_doc`] / [`json_doc`]).
fn doc_text(elem: &Node) -> Option<&str> {
    let doc = elem.find("annotation")?.find("documentation")?;
    if doc.text.is_empty() {
        None
    } else {
        Some(&doc.text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Occurs {
    min: u64,
    max: Option<u64>,
}

impl Occurs {
    const ONCE: Self = Self {
        min: 1,
        max: Some(1),
    };

    fn parse(node: &Node) -> Result<Self, String> {
        let min = match node.get("minOccurs") {
            Some(value) => value.parse::<u64>().map_err(|_| {
                format!(
                    "{} has invalid minOccurs value {value:?}",
                    particle_label(node)
                )
            })?,
            None => 1,
        };
        let max = match node.get("maxOccurs") {
            Some("unbounded") => None,
            Some(value) => Some(value.parse::<u64>().map_err(|_| {
                format!(
                    "{} has invalid maxOccurs value {value:?}",
                    particle_label(node)
                )
            })?),
            None => Some(1),
        };
        if max.is_some_and(|maximum| maximum < min) {
            return Err(format!(
                "{} has maxOccurs smaller than minOccurs",
                particle_label(node)
            ));
        }
        Ok(Self { min, max })
    }

    fn is_repeated(self) -> bool {
        self.max.is_none_or(|maximum| maximum > 1)
    }
}

#[derive(Debug, Clone, Copy)]
enum ElementType<'a> {
    Named(&'a str),
    InlineSimple(&'a Node),
    InlineComplex(&'a Node),
    Unspecified,
}

#[derive(Debug, Clone)]
struct ElementParticle<'a> {
    node: &'a Node,
    name: String,
    reference: Option<&'a str>,
    occurs: Occurs,
    annotation: Option<&'a str>,
    type_use: ElementType<'a>,
}

#[derive(Debug, Clone)]
struct GroupParticle<'a> {
    occurs: Occurs,
    annotation: Option<&'a str>,
    children: Vec<Particle<'a>>,
}

#[derive(Debug, Clone)]
struct AnyParticle<'a> {
    occurs: Occurs,
    annotation: Option<&'a str>,
    namespace: Option<&'a str>,
    process_contents: Option<&'a str>,
}

#[derive(Debug, Clone)]
enum Particle<'a> {
    Element(ElementParticle<'a>),
    Sequence(GroupParticle<'a>),
    Choice(GroupParticle<'a>),
    All(GroupParticle<'a>),
    Any(AnyParticle<'a>),
}

impl Particle<'_> {
    fn occurs(&self) -> Occurs {
        match self {
            Self::Element(element) => element.occurs,
            Self::Sequence(group) | Self::Choice(group) | Self::All(group) => group.occurs,
            Self::Any(any) => any.occurs,
        }
    }
}

#[derive(Debug, Clone)]
struct ComplexTypeDef<'a> {
    node: &'a Node,
    particle: Option<Particle<'a>>,
    extension_base: Option<&'a str>,
    attributes: Vec<&'a Node>,
    mixed: bool,
}

#[derive(Debug)]
struct SchemaIndex<'a> {
    simple_order: Vec<(String, &'a Node)>,
    simple: HashMap<String, &'a Node>,
    complex_order: Vec<(String, ComplexTypeDef<'a>)>,
    complex: HashMap<String, ComplexTypeDef<'a>>,
    roots: Vec<ElementParticle<'a>>,
}

impl<'a> SchemaIndex<'a> {
    fn build(root: &'a Node) -> Result<Self, String> {
        if root.name != "schema" {
            return Err(format!(
                "expected an xs:schema document root, found {:?}",
                root.name
            ));
        }

        let mut simple_order = Vec::new();
        let mut simple = HashMap::new();
        let mut complex_order = Vec::new();
        let mut complex = HashMap::new();
        let mut named_kinds: HashMap<String, String> = HashMap::new();
        let mut roots = Vec::new();
        let mut root_names = HashSet::new();

        for child in &root.children {
            match child.name.as_str() {
                "simpleType" | "complexType" => {
                    let name = child
                        .get("name")
                        .ok_or_else(|| format!("global xs:{} is missing @name", child.name))?;
                    if let Some(previous) = named_kinds.insert(name.to_string(), child.name.clone())
                    {
                        return Err(format!(
                            "duplicate named type {name:?}: xs:{previous} and xs:{}",
                            child.name
                        ));
                    }
                    if child.name == "simpleType" {
                        simple_order.push((name.to_string(), child));
                        simple.insert(name.to_string(), child);
                    } else {
                        let definition = parse_complex_type(child)?;
                        complex_order.push((name.to_string(), definition.clone()));
                        complex.insert(name.to_string(), definition);
                    }
                }
                "element" => {
                    let element = parse_element_particle(child)?;
                    if !root_names.insert(element.name.clone()) {
                        return Err(format!("duplicate global document root {:?}", element.name));
                    }
                    roots.push(element);
                }
                "annotation" | "key" | "keyref" | "unique" | "attribute" => {}
                other => {
                    return Err(format!(
                        "unsupported global xs:{other}; exact generation requires an explicit schema model"
                    ));
                }
            }
        }

        if roots.is_empty() {
            return Err("schema has no global document roots".to_string());
        }

        Ok(Self {
            simple_order,
            simple,
            complex_order,
            complex,
            roots,
        })
    }
}

fn particle_label(node: &Node) -> String {
    match node.get("name").or_else(|| node.get("ref")) {
        Some(name) => format!("xs:{} {name:?}", node.name),
        None => format!("xs:{}", node.name),
    }
}

fn parse_element_particle(node: &Node) -> Result<ElementParticle<'_>, String> {
    let name = match (node.get("name"), node.get("ref")) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "{} declares both @name and @ref",
                particle_label(node)
            ));
        }
        (Some(name), None) => name.to_string(),
        (None, Some(reference)) => reference
            .rsplit(':')
            .next()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| format!("{} has an empty @ref", particle_label(node)))?
            .to_string(),
        (None, None) => return Err("xs:element is missing both @name and @ref".to_string()),
    };
    let inline_simple: Vec<&Node> = node.findall("simpleType").collect();
    let inline_complex: Vec<&Node> = node.findall("complexType").collect();
    let inline_count = inline_simple.len() + inline_complex.len();
    if inline_count > 1 {
        return Err(format!(
            "xs:element {name:?} has multiple inline type definitions"
        ));
    }
    if node.get("type").is_some() && inline_count != 0 {
        return Err(format!(
            "xs:element {name:?} has both @type and an inline type"
        ));
    }
    let type_use = if let Some(name) = node.get("type") {
        ElementType::Named(name)
    } else if let Some(simple) = inline_simple.first() {
        ElementType::InlineSimple(simple)
    } else if let Some(complex) = inline_complex.first() {
        ElementType::InlineComplex(complex)
    } else {
        ElementType::Unspecified
    };
    Ok(ElementParticle {
        node,
        name,
        reference: node.get("ref"),
        occurs: Occurs::parse(node)?,
        annotation: doc_text(node),
        type_use,
    })
}

fn parse_particle(node: &Node) -> Result<Particle<'_>, String> {
    match node.name.as_str() {
        "element" => Ok(Particle::Element(parse_element_particle(node)?)),
        "sequence" | "choice" | "all" => {
            let mut children = Vec::new();
            for child in &node.children {
                match child.name.as_str() {
                    "annotation" => {}
                    "element" | "sequence" | "choice" | "all" | "any" => {
                        children.push(parse_particle(child)?);
                    }
                    other => {
                        return Err(format!(
                            "unsupported xs:{other} inside {}; exact generation cannot discard it",
                            particle_label(node)
                        ));
                    }
                }
            }
            if children.is_empty() {
                return Err(format!("{} has no content", particle_label(node)));
            }
            let group = GroupParticle {
                occurs: Occurs::parse(node)?,
                annotation: doc_text(node),
                children,
            };
            Ok(match node.name.as_str() {
                "sequence" => Particle::Sequence(group),
                "choice" => Particle::Choice(group),
                "all" => Particle::All(group),
                _ => unreachable!(),
            })
        }
        "any" => Ok(Particle::Any(AnyParticle {
            occurs: Occurs::parse(node)?,
            annotation: doc_text(node),
            namespace: node.get("namespace"),
            process_contents: node.get("processContents"),
        })),
        other => Err(format!("xs:{other} is not a schema particle")),
    }
}

fn parse_complex_type(node: &Node) -> Result<ComplexTypeDef<'_>, String> {
    let mut content_nodes: Vec<&Node> = node
        .children
        .iter()
        .filter(|child| {
            matches!(
                child.name.as_str(),
                "element" | "sequence" | "choice" | "all" | "any"
            )
        })
        .collect();
    let mut extension_base = None;
    let mut attributes: Vec<&Node> = node.findall("attribute").collect();

    if let Some(complex_content) = node.find("complexContent") {
        if !content_nodes.is_empty() {
            return Err("complexType mixes direct particles with complexContent".to_string());
        }
        let extensions: Vec<&Node> = complex_content.findall("extension").collect();
        if extensions.len() != 1 {
            return Err(format!(
                "complexContent requires exactly one extension, found {}",
                extensions.len()
            ));
        }
        let extension = extensions[0];
        extension_base = Some(
            extension
                .get("base")
                .ok_or_else(|| "complexContent extension is missing @base".to_string())?,
        );
        content_nodes = extension
            .children
            .iter()
            .filter(|child| {
                matches!(
                    child.name.as_str(),
                    "element" | "sequence" | "choice" | "all" | "any"
                )
            })
            .collect();
        attributes.extend(extension.findall("attribute"));
    }

    if content_nodes.len() > 1 {
        return Err(format!(
            "complexType contains {} sibling content particles; use an explicit sequence, choice, or all",
            content_nodes.len()
        ));
    }
    let particle = content_nodes
        .first()
        .map(|node| parse_particle(node))
        .transpose()?;
    Ok(ComplexTypeDef {
        node,
        particle,
        extension_base,
        attributes,
        mixed: node.get("mixed") == Some("true"),
    })
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// enums.rs generator (port of generate_hcdf_rs_enums.py)
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// The emitted header of `enums.rs`, naming the `hcdf regen enums` command that regenerates it. The
/// committed `enums.rs` carries this header verbatim, so the drift gate can regenerate it byte-for-byte.
/// The file-level `rustfmt::skip` keeps rustfmt from reformatting this verbatim output, which would
/// break that byte-exact gate.
const ENUMS_HEADER: &str = r#"// GENERATED from hcdf.xsd by hcdf regen enums. DO NOT EDIT.
// Regenerate: hcdf regen enums hcdf.xsd rust/hcdformat-rs/src/model/enums.rs
// rustfmt must not reformat this file: it is emitted verbatim by the generator and pinned by a
// byte-exact regeneration check, which reformatting would break.
#![cfg_attr(rustfmt, rustfmt::skip)]
//! Typed HCDF enums, generated from `hcdf.xsd` (the single source of truth).
//!
//! One Rust enum per enumerated `xs:simpleType`, each variant `#[serde(rename = "...")]`d to its exact
//! XSD enumeration literal, with `FromStr`/`Display` to/from that literal, and rustdoc carried from the
//! schema. It is one of the XSD-driven generators (alongside the JSON schema, completions and spec).
//!
//! The hand-written model TYPES most enumerated attributes/element-text as `Option<EnumTy>`
//! (or `Vec<EnumTy>`), so an out-of-enum literal is rejected by serde at PARSE, matching Python's
//! load-time `ValueError`. Residual slots that intentionally retain a string field (see
//! [`ENUM_ATTRS`]), including shared sensor-category `@type`, projection/probe `@type`, and
//! transmission endpoint `@role`, are enforced by `crate::validate` during the document walk. The
//! enums serialize to the exact XSD literals.
use serde::{Deserialize, Serialize};"#;

/// Enumerated XSD vocabularies with authoritative handwritten Rust definitions outside
/// `model/enums.rs`. They still participate in typed-slot ownership, but the generated enum file must
/// not duplicate them.
const EXTERNALLY_AUTHORED_ENUMS: &[&str] = &[
    "Carrier",
    "EeeMode",
    "Fidelity",
    "GptpClockKind",
    "MacsecEnforcement",
    "Purpose",
    "StreamProfileSelectionRole",
    "TerminationMounting",
    "TrafficPreemption",
];

const RUST_OWNED_ENUMS: &[&str] = &[
    "AxisValue",
    "BodyFrame",
    "CameraDistortionModel",
    "Carrier",
    "CompRole",
    "EeeMode",
    "EncoderPrinciple",
    "ForceFrame",
    "FrustumShape",
    "Fidelity",
    "GptpClockKind",
    "Handedness",
    "HmiType",
    "InertialSensorType",
    "JointType",
    "MacsecEnforcement",
    "MeasureDirection",
    "MotorType",
    "NameOrigin",
    "NoiseType",
    "OpticalSensorType",
    "PitchConvention",
    "Purpose",
    "StreamProfileSelectionRole",
    "TerminationMounting",
    "TrafficPreemption",
    "TransmissionType",
    "WorldFrame",
];

/// Rust enum classes whose authored model fields enforce the XSD vocabulary during deserialization.
/// This explicit ownership manifest is also available to parity tests and downstream generators.
pub const fn rust_owned_enum_names() -> &'static [&'static str] {
    RUST_OWNED_ENUMS
}

/// A snake/kebab/already-Pascal XSD simpleType name → PascalCase Rust enum identifier.
fn enum_name(type_name: &str) -> String {
    type_name
        .split(['_', '-'])
        .filter(|p| !p.is_empty())
        .map(cap)
        .collect()
}

/// An enumeration literal → a valid PascalCase Rust variant identifier. Deterministic and
/// collision-resistant within an enum: a leading `-` (a negative axis like `-X`) becomes a `Neg` prefix
/// (so `X` and `-X` do not collide); non-alphanumerics are word separators; a leading digit is `V`-prefixed.
/// The caller de-dups defensively. Mirrors `variant_name` in the Python original.
fn variant_name(value: &str) -> String {
    let neg = value.starts_with('-');
    let body = if neg { &value[1..] } else { value };
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in body.chars() {
        if c.is_ascii_alphanumeric() {
            cur.push(c);
        } else if !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    let mut name: String = parts.iter().map(|p| cap(p)).collect();
    if neg {
        name = format!("Neg{name}");
    }
    if name.is_empty() {
        name = "Empty".to_string();
    }
    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        name = format!("V{name}");
    }
    name
}

/// Uppercase the first char (Unicode, matching Python `s[:1].upper()`), keep the rest verbatim.
fn cap(p: &str) -> String {
    let mut chars = p.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The documentation of an enumerated simpleType, whitespace-collapsed to a single line (`_doc`:
/// `" ".join(text.split())`); `None` when absent or all-whitespace (both falsy in the Python emit guard).
fn enum_doc(st: &Node) -> Option<String> {
    let raw = doc_text(st)?;
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

/// Greedy-wrap a documentation string to `~98` columns for tidy rustdoc lines, counting CHARS (not
/// bytes) exactly as Python `len()` does: the descriptions carry multi-byte `∪`/`°`. Port of `_rustdoc`.
fn rustdoc_wrap(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > 98 {
            out.push(std::mem::take(&mut cur));
            cur.push_str(w);
        } else if cur.is_empty() {
            cur.push_str(w);
        } else {
            cur.push(' ');
            cur.push_str(w);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The indexed view of an XSD the enums generator walks: the top-level simpleTypes (for docs), the
/// enumerated ones (value sets + PascalCase class names), the top-level complexTypes (document order,
/// for the usage walk), and the root element.
struct EnumGen<'a> {
    root: &'a Node,
    /// Top-level named simpleType nodes, by name (for `_doc` lookup).
    sts: HashMap<String, &'a Node>,
    /// Enumerated simpleType name → literal value set, in schema order.
    enums: HashMap<String, Vec<String>>,
    /// Enum simpleType name → PascalCase Rust enum class name.
    enum_cls: HashMap<String, String>,
    /// Membership set of enumerated simpleType names (fast `type in enums` test).
    enum_names: HashSet<String>,
    /// Top-level named complexTypes in DOCUMENT order (last-write-wins on a repeated usage key needs it).
    cts_order: Vec<(String, &'a Node)>,
    /// Membership set of top-level complexType names.
    cts_names: HashSet<String>,
}

impl<'a> EnumGen<'a> {
    fn index(root: &'a Node) -> Self {
        let mut sts: HashMap<String, &Node> = HashMap::new();
        let mut sts_order: Vec<(String, &Node)> = Vec::new();
        let mut cts_order: Vec<(String, &Node)> = Vec::new();
        let mut cts_names: HashSet<String> = HashSet::new();
        for c in &root.children {
            let nm = match c.get("name") {
                Some(n) => n.to_string(),
                None => continue,
            };
            match c.name.as_str() {
                "simpleType" => {
                    sts.insert(nm.clone(), c);
                    sts_order.push((nm, c));
                }
                "complexType" => {
                    cts_names.insert(nm.clone());
                    cts_order.push((nm, c));
                }
                _ => {}
            }
        }
        let mut enums: HashMap<String, Vec<String>> = HashMap::new();
        let mut enum_cls: HashMap<String, String> = HashMap::new();
        let mut enum_names: HashSet<String> = HashSet::new();
        for (nm, st) in &sts_order {
            let vals = enum_values(st);
            if !vals.is_empty() {
                enum_cls.insert(nm.clone(), enum_name(nm));
                enum_names.insert(nm.clone());
                enums.insert(nm.clone(), vals);
            }
        }
        EnumGen {
            root,
            sts,
            enums,
            enum_cls,
            enum_names,
            cts_order,
            cts_names,
        }
    }

    /// complexType name → element local names declared with `type=` that complexType (a set; `lxml`
    /// `root.iter(element)` over every descendant element). Mirrors `_element_names_for_ct`.
    fn element_names_for_ct(&self) -> HashMap<String, Vec<String>> {
        let mut descs = Vec::new();
        self.root.descendants(&mut descs);
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for el in descs {
            if el.name != "element" {
                continue;
            }
            if let (Some(tp), Some(en)) = (el.get("type"), el.get("name")) {
                if self.cts_names.contains(tp) {
                    let v = out.entry(tp.to_string()).or_default();
                    if !v.iter().any(|x| x == en) {
                        v.push(en.to_string());
                    }
                }
            }
        }
        out
    }

    /// The `(element-local-name, key)` → enum type name map driving the residual table. `key` is
    /// `@<attr>` for an enumerated attribute or `#text` for an enum-typed child element; keyed by the
    /// DOCUMENT element name (what the validator sees). Mirrors `_usages`: a HashMap gives last-write-
    /// wins over the document-ordered complexType walk (then the root element's inline type last).
    fn usages(&self) -> HashMap<(String, String), String> {
        let ct_elems = self.element_names_for_ct();
        let mut usages: HashMap<(String, String), String> = HashMap::new();
        for (nm, ct) in &self.cts_order {
            let names = ct_elems.get(nm).cloned().unwrap_or_default();
            self.walk(ct, &names, &mut usages);
        }
        if let Some(root_el) = self.root.find("element") {
            if let Some(root_ct) = root_el.find("complexType") {
                self.walk(root_ct, &["hcdf".to_string()], &mut usages);
            }
        }
        usages
    }

    /// One complexType's contribution to [`usages`]: over `ct` and every descendant (`lxml` `.iter()`),
    /// record enum-typed attributes (once per element name that uses `ct`) and enum-typed child elements.
    fn walk(
        &self,
        ct: &Node,
        elem_names: &[String],
        usages: &mut HashMap<(String, String), String>,
    ) {
        let mut descs = Vec::new();
        ct.descendants(&mut descs);
        for ch in descs {
            match ch.name.as_str() {
                "attribute" => {
                    if let (Some(ty), Some(name)) = (ch.get("type"), ch.get("name")) {
                        if self.enum_names.contains(ty) {
                            for en in elem_names {
                                usages.insert((en.clone(), format!("@{name}")), ty.to_string());
                            }
                        }
                    }
                }
                "element" => {
                    if let (Some(ty), Some(name)) = (ch.get("type"), ch.get("name")) {
                        if self.enum_names.contains(ty) {
                            usages.insert((name.to_string(), "#text".to_string()), ty.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn typed_enum_classes(&self) -> Result<HashSet<String>, String> {
        let mut typed = HashSet::new();
        let known: HashSet<&str> = self.enum_cls.values().map(String::as_str).collect();
        for class in RUST_OWNED_ENUMS {
            if !typed.insert((*class).to_string()) {
                return Err(format!("duplicate Rust-owned enum class {class:?}"));
            }
            if !known.contains(class) {
                return Err(format!(
                    "Rust-owned enum class {class:?} has no enumerated XSD simpleType"
                ));
            }
        }
        Ok(typed)
    }

    /// The residual `(element, key)` slots the typed model does NOT reject at parse (so they belong in
    /// `ENUM_ATTRS`): every enumerated slot whose enum class has no typed field.
    /// The inverse of "assumed typed": a slot is kept unless a struct field demonstrably retyped it.
    fn model_untyped_slots(
        &self,
        _model_dir: &Path,
        usages: &HashMap<(String, String), String>,
    ) -> Result<HashSet<(String, String)>, String> {
        let typed = self.typed_enum_classes()?;
        let mut residual = HashSet::new();
        for (k, type_name) in usages {
            let cls = &self.enum_cls[type_name];
            if !typed.contains(cls) {
                residual.insert(k.clone());
            }
        }
        Ok(residual)
    }

    /// One enum block (rustdoc + `enum` + `valid_values` + `FromStr` + `Display`), joined with `\n` and
    /// NO trailing newline, mirroring `_emit_enum` line-for-line so the blocks byte-match the committed file.
    fn emit_enum(&self, cls: &str, type_name: &str, values: &[String]) -> String {
        let mut lines: Vec<String> = Vec::new();
        if let Some(doc) = enum_doc(self.sts[type_name]) {
            for dl in rustdoc_wrap(&doc) {
                lines.push(format!("/// {dl}"));
            }
        }
        lines.push(
            "#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]".to_string(),
        );
        lines.push(format!("pub enum {cls} {{"));
        let mut used: HashMap<String, usize> = HashMap::new();
        let mut variants: Vec<(String, String)> = Vec::new();
        for v in values {
            let base = variant_name(v);
            let mem = if let Some(c) = used.get_mut(&base) {
                *c += 1;
                format!("{base}{}", *c)
            } else {
                used.insert(base.clone(), 0);
                base
            };
            lines.push(format!("    #[serde(rename = \"{v}\")]"));
            lines.push(format!("    {mem},"));
            variants.push((mem, v.clone()));
        }
        lines.push("}".to_string());
        lines.push(String::new());
        lines.push(format!("impl {cls} {{"));
        lines.push(format!(
            "    /// The exact XSD enumeration literals accepted for `{type_name}`, in schema order."
        ));
        lines.push("    pub const fn valid_values() -> &'static [&'static str] {".to_string());
        let joined = values
            .iter()
            .map(|v| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("        &[{joined}]"));
        lines.push("    }".to_string());
        lines.push("}".to_string());
        lines.push(String::new());
        lines.push(format!("impl core::str::FromStr for {cls} {{"));
        lines.push("    type Err = ();".to_string());
        lines.push("    fn from_str(s: &str) -> Result<Self, Self::Err> {".to_string());
        lines.push("        match s {".to_string());
        for (mem, v) in &variants {
            lines.push(format!("            \"{v}\" => Ok({cls}::{mem}),"));
        }
        lines.push("            _ => Err(()),".to_string());
        lines.push("        }".to_string());
        lines.push("    }".to_string());
        lines.push("}".to_string());
        lines.push(String::new());
        lines.push(format!("impl core::fmt::Display for {cls} {{"));
        lines.push(
            "    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {"
                .to_string(),
        );
        lines.push("        f.write_str(match self {".to_string());
        for (mem, v) in &variants {
            lines.push(format!("            {cls}::{mem} => \"{v}\","));
        }
        lines.push("        })".to_string());
        lines.push("    }".to_string());
        lines.push("}".to_string());
        lines.join("\n")
    }

    fn generate(&self, model_dir: &Path) -> Result<String, String> {
        let mut out: Vec<String> = vec![ENUMS_HEADER.to_string()];
        let mut names: Vec<&String> = self.enums.keys().collect();
        names.sort();
        for nm in &names {
            let class = &self.enum_cls[*nm];
            if !EXTERNALLY_AUTHORED_ENUMS.contains(&class.as_str()) {
                out.push(self.emit_enum(class, nm, &self.enums[*nm]));
            }
        }

        let usages = self.usages();
        let untyped = self.model_untyped_slots(model_dir, &usages)?;
        let mut keys: Vec<&(String, String)> = usages.keys().collect();
        keys.sort();
        let mut rows: Vec<String> = Vec::new();
        for k in &keys {
            if !untyped.contains(*k) {
                continue;
            }
            let cls = &self.enum_cls[&usages[*k]];
            rows.push(format!(
                "    (\"{}\", \"{}\", {cls}::valid_values()),",
                k.0, k.1
            ));
        }

        let mut table: Vec<String> = TABLE_DOC.iter().map(|s| s.to_string()).collect();
        table.push("pub static ENUM_ATTRS: &[(&str, &str, &[&str])] = &[".to_string());
        table.extend(rows);
        table.push("];".to_string());
        table.push(String::new());
        table.push(
            "/// The accepted literal set for `(element, key)`, or `None` if it is not a model-untyped slot."
                .to_string(),
        );
        table.push(
            "pub fn valid_values_for(element: &str, key: &str) -> Option<&'static [&'static str]> {"
                .to_string(),
        );
        table.push(
            "    ENUM_ATTRS.iter().find(|(e, k, _)| *e == element && *k == key).map(|(_, _, v)| *v)"
                .to_string(),
        );
        table.push("}".to_string());
        out.push(table.join("\n"));

        Ok(out.join("\n\n") + "\n")
    }
}

/// The fixed rustdoc header of the `ENUM_ATTRS` table (VERBATIM from `generate_hcdf_rs_enums.py`).
const TABLE_DOC: &[&str] = &[
    "/// The enumerated `(element-local-name, key)` slots the TYPED model cannot enforce at parse,",
    "/// paired with the exact literal set the schema accepts. This is the RESIDUAL: every",
    "/// enumerated XSD slot whose enum has NO typed model field (so serde cannot reject it at",
    "/// parse), computed from the model sources rather than hand-curated: a slot is kept ONLY",
    "/// when no struct field retyped it, so any future enumerated slot lacking a typed field is",
    "/// retained automatically. Three families remain: (1) sensor-category `@type` slots that",
    "/// share [`super::sensor::SensorCategory`]; (2) camera projection and fluid probe `@type`;",
    "/// and (3) transmission endpoint `@role`. These fields intentionally stay string-typed, and",
    "/// [`crate::validate::validate_enums`] enforces their exact vocabularies by document walk.",
];

/// The enumeration literal values of a simpleType, in schema order (empty when not an enum restriction).
fn enum_values(st: &Node) -> Vec<String> {
    match st.find("restriction") {
        Some(r) => r
            .findall("enumeration")
            .filter_map(|e| e.get("value").map(str::to_string))
            .collect(),
        None => Vec::new(),
    }
}

/// Generate `src/model/enums.rs` from `xsd`. `model_dir` is where the hand-written model structs live
/// (the residual `ENUM_ATTRS` slots are the enum classes NOT already retyped there); the `hcdf regen
/// enums` subcommand passes the OUTPUT file's parent directory, which IS that model directory.
pub fn generate_enums_rs(xsd: &str, model_dir: &Path) -> Result<String, String> {
    let root = parse_xsd(xsd)?;
    EnumGen::index(&root).generate(model_dir)
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// hcdf.schema.json generator (port of generate_json_schema.py)
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

/// An ordered JSON value that PRESERVES object key insertion order and the Python `int`/`float`
/// distinction, both load-bearing for byte-parity with `json.dump(..., indent=2, ensure_ascii=False)`.
enum J {
    S(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    fn s(x: &str) -> J {
        J::S(x.to_string())
    }
}

/// Upsert `key` in an ordered object: replace the value in place if the key exists (keeping its
/// position), else append, mirroring Python `dict[key] = value`.
fn oset(map: &mut Vec<(String, J)>, key: &str, val: J) {
    if let Some(slot) = map.iter_mut().find(|(k, _)| k == key) {
        slot.1 = val;
    } else {
        map.push((key.to_string(), val));
    }
}

/// First value for `key` in an ordered object (`dict.get`).
fn oget<'a>(map: &'a [(String, J)], key: &str) -> Option<&'a J> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Unwrap an object's pairs (every `convert_*` returns an object; the wrap arm is defensive).
fn as_obj(j: J) -> Vec<(String, J)> {
    match j {
        J::Obj(o) => o,
        other => vec![("value".to_string(), other)],
    }
}

/// The indexed view of an XSD the JSON-schema generator walks: the top-level named simple/complex types
/// (`$defs`) and the root. Only top-level named types are indexed (inline types resolve through elements).
struct JsonGen<'a> {
    simple: &'a HashMap<String, &'a Node>,
    complex: &'a HashMap<String, ComplexTypeDef<'a>>,
    roots: &'a [ElementParticle<'a>],
}

impl<'a> JsonGen<'a> {
    fn index(schema: &'a SchemaIndex<'a>) -> Self {
        Self {
            simple: &schema.simple,
            complex: &schema.complex,
            roots: &schema.roots,
        }
    }

    fn generate(&self) -> J {
        let mut schema: Vec<(String, J)> = vec![
            (
                "$schema".to_string(),
                J::s("https://json-schema.org/draft/2020-12/schema"),
            ),
            (
                "$id".to_string(),
                J::s("https://hcdf.org/schema/hcdf.schema.json"),
            ),
            (
                "title".to_string(),
                J::s("HCDF (Hardware Configuration Descriptive Format)"),
            ),
            (
                "description".to_string(),
                J::s(
                    "JSON Schema for HCDF documents, auto-generated from hcdf.xsd. \
                     The XSD is the source of truth.",
                ),
            ),
        ];

        let mut simple_names: Vec<&String> = self.simple.keys().collect();
        simple_names.sort();
        let mut complex_names: Vec<&String> = self.complex.keys().collect();
        complex_names.sort();
        let mut defs: Vec<(String, J)> = Vec::new();
        for n in &simple_names {
            oset(&mut defs, n, self.convert_simple_type(self.simple[*n]));
        }
        for n in &complex_names {
            oset(
                &mut defs,
                n,
                self.convert_complex_type(self.complex[*n].node),
            );
        }
        schema.push(("$defs".to_string(), J::Obj(defs)));

        if self.roots.len() == 1 {
            let root_schema = as_obj(self.convert_root_element(self.roots[0].node));
            let ty = match oget(&root_schema, "type") {
                Some(J::S(s)) => J::s(s),
                _ => J::s("object"),
            };
            schema.push(("type".to_string(), ty));
            for key in ["properties", "required", "additionalProperties"] {
                if let Some(v) = oget(&root_schema, key) {
                    schema.push((key.to_string(), clone_j(v)));
                }
            }
        } else if !self.roots.is_empty() {
            schema.push((
                "oneOf".to_string(),
                J::Arr(
                    self.roots
                        .iter()
                        .map(|root| self.convert_root_element(root.node))
                        .collect(),
                ),
            ));
        }
        J::Obj(schema)
    }

    /// A named `xs:simpleType` (typically an enumeration).
    fn convert_simple_type(&self, elem: &Node) -> J {
        match elem.find("restriction") {
            Some(r) => self.convert_restriction(r),
            None => J::Obj(vec![("type".to_string(), J::s("string"))]),
        }
    }

    /// `xs:restriction`: an enumeration becomes `{type:string, enum:[...]}`, else the base builtin.
    fn convert_restriction(&self, restriction: &Node) -> J {
        let base = restriction.get("base").unwrap_or("xs:string");
        let enums: Vec<J> = restriction
            .findall("enumeration")
            .filter_map(|e| e.get("value").map(J::s))
            .collect();
        if enums.is_empty() {
            resolve_builtin(base)
        } else {
            J::Obj(vec![
                ("type".to_string(), J::s("string")),
                ("enum".to_string(), J::Arr(enums)),
            ])
        }
    }

    /// A named `xs:complexType`: `complexContent` extension inheritance, else object with children +
    /// attributes (+ `$text` for mixed content). Key order mirrors the Python dict insertion order.
    fn convert_complex_type(&self, elem: &Node) -> J {
        if let Some(cc) = elem.find("complexContent") {
            if let Some(ext) = cc.find("extension") {
                return self.convert_extension(ext, elem);
            }
        }
        let is_mixed = elem.get("mixed") == Some("true");
        let mut result: Vec<(String, J)> = vec![("type".to_string(), J::s("object"))];
        let mut props: Vec<(String, J)> = Vec::new();
        let mut required: Vec<String> = Vec::new();
        self.collect_children(elem, &mut props, &mut required, &mut result);
        self.collect_attributes(elem, &mut props, &mut required);
        if is_mixed {
            oset(
                &mut props,
                "$text",
                J::Obj(vec![
                    (
                        "description".to_string(),
                        J::s("Text content of the element"),
                    ),
                    (
                        "type".to_string(),
                        J::Arr(vec![J::s("string"), J::s("number")]),
                    ),
                ]),
            );
        }
        if !props.is_empty() {
            oset(&mut result, "properties", J::Obj(props));
        }
        if !required.is_empty() {
            required.sort();
            oset(
                &mut result,
                "required",
                J::Arr(required.into_iter().map(J::S).collect()),
            );
        }
        if let Some(doc) = json_doc(elem) {
            oset(&mut result, "description", J::S(doc));
        }
        J::Obj(result)
    }

    /// `xs:extension` inheritance: merge the base type's properties/required, then the extension's own
    /// children/attributes; documentation is taken from the enclosing complexType (`grandparent`).
    /// `required` is deduplicated (a set) here, unlike the plain complexType path.
    fn convert_extension(&self, ext: &Node, grandparent: &Node) -> J {
        let base_name = ext.get("base").unwrap_or("");
        let base = if self.complex.contains_key(base_name) {
            as_obj(self.convert_complex_type(self.complex[base_name].node))
        } else if self.simple.contains_key(base_name) {
            as_obj(self.convert_simple_type(self.simple[base_name]))
        } else {
            Vec::new()
        };
        let mut result: Vec<(String, J)> = vec![("type".to_string(), J::s("object"))];
        let mut props: Vec<(String, J)> = Vec::new();
        let mut required: Vec<String> = Vec::new();
        if let Some(J::Obj(bp)) = oget(&base, "properties") {
            for (k, v) in bp {
                oset(&mut props, k, clone_j(v));
            }
        }
        if let Some(J::Arr(br)) = oget(&base, "required") {
            for item in br {
                if let J::S(s) = item {
                    required.push(s.clone());
                }
            }
        }
        self.collect_children(ext, &mut props, &mut required, &mut result);
        self.collect_attributes(ext, &mut props, &mut required);
        if !props.is_empty() {
            oset(&mut result, "properties", J::Obj(props));
        }
        if !required.is_empty() {
            let uniq: BTreeSet<String> = required.into_iter().collect();
            oset(
                &mut result,
                "required",
                J::Arr(uniq.into_iter().map(J::S).collect()),
            );
        }
        if let Some(doc) = json_doc(grandparent) {
            oset(&mut result, "description", J::S(doc));
        }
        J::Obj(result)
    }

    /// Collect child element definitions from a complexType/extension's `sequence`/`all`/`choice` (and
    /// direct `element`) children.
    fn collect_children(
        &self,
        parent: &Node,
        props: &mut Vec<(String, J)>,
        required: &mut Vec<String>,
        result: &mut Vec<(String, J)>,
    ) {
        for child in &parent.children {
            match child.name.as_str() {
                "sequence" => self.collect_sequence(child, props, required),
                "all" => self.collect_all(child, props, required),
                "choice" => self.convert_choice(child, result),
                "element" => self.add_element_property(child, props, required, false),
                _ => {}
            }
        }
    }

    fn collect_sequence(
        &self,
        seq: &Node,
        props: &mut Vec<(String, J)>,
        required: &mut Vec<String>,
    ) {
        for child in &seq.children {
            match child.name.as_str() {
                "element" => self.add_element_property(child, props, required, false),
                "choice" => {
                    // An inline choice in a sequence: each option is an optional property.
                    for cc in &child.children {
                        if cc.name == "element" {
                            self.add_element_property(cc, props, required, true);
                        }
                    }
                }
                "any" => {
                    oset(
                        props,
                        "_extensions",
                        J::Obj(vec![
                            (
                                "description".to_string(),
                                J::s("Extension content (xs:any)"),
                            ),
                            ("type".to_string(), J::s("object")),
                            ("additionalProperties".to_string(), J::Bool(true)),
                        ]),
                    );
                }
                _ => {}
            }
        }
    }

    fn collect_all(
        &self,
        all_elem: &Node,
        props: &mut Vec<(String, J)>,
        required: &mut Vec<String>,
    ) {
        for child in &all_elem.children {
            if child.name == "element" {
                self.add_element_property(child, props, required, false);
            }
        }
    }

    /// `xs:choice` → `oneOf` (one single-property object per option).
    fn convert_choice(&self, choice: &Node, result: &mut Vec<(String, J)>) {
        let mut one_of: Vec<J> = Vec::new();
        for child in &choice.children {
            if child.name == "element" {
                let elem_name = child.get("name").unwrap_or("");
                let elem_schema = self.resolve_element_type(child);
                one_of.push(J::Obj(vec![
                    ("type".to_string(), J::s("object")),
                    (
                        "properties".to_string(),
                        J::Obj(vec![(elem_name.to_string(), elem_schema)]),
                    ),
                    ("required".to_string(), J::Arr(vec![J::s(elem_name)])),
                ]));
            }
        }
        if !one_of.is_empty() {
            oset(result, "oneOf", J::Arr(one_of));
        }
    }

    /// Add a single `xs:element` as a JSON property (an array when `maxOccurs > 1`/`unbounded`).
    fn add_element_property(
        &self,
        elem: &Node,
        props: &mut Vec<(String, J)>,
        required: &mut Vec<String>,
        force_optional: bool,
    ) {
        let name = match elem.get("name") {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => return,
        };
        let min_occurs = elem
            .get("minOccurs")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(1);
        let max_occurs: Option<i64> = match elem.get("maxOccurs") {
            Some("unbounded") => None,
            Some(s) => Some(s.parse::<i64>().unwrap_or(1)),
            None => Some(1),
        };
        let doc = json_doc(elem);
        let mut elem_schema = as_obj(self.resolve_element_type(elem));
        if let Some(d) = &doc {
            if oget(&elem_schema, "description").is_none() {
                elem_schema.push(("description".to_string(), J::S(d.clone())));
            }
        }
        let is_array = match max_occurs {
            None => true,
            Some(m) => m > 1,
        };
        if is_array {
            let mut prop: Vec<(String, J)> = vec![
                ("type".to_string(), J::s("array")),
                ("items".to_string(), J::Obj(elem_schema)),
            ];
            if min_occurs > 0 {
                prop.push(("minItems".to_string(), J::Int(min_occurs)));
            }
            if let Some(m) = max_occurs {
                prop.push(("maxItems".to_string(), J::Int(m)));
            }
            if let Some(d) = &doc {
                prop.push(("description".to_string(), J::S(d.clone())));
            }
            oset(props, &name, J::Obj(prop));
        } else {
            oset(props, &name, J::Obj(elem_schema));
        }
        if min_occurs >= 1 && !force_optional {
            required.push(name);
        }
    }

    /// Resolve an `xs:element`'s type to a JSON fragment (inline complex/simple type, named ref, or the
    /// string default).
    fn resolve_element_type(&self, elem: &Node) -> J {
        if let Some(ct) = elem.find("complexType") {
            return self.convert_complex_type(ct);
        }
        if let Some(st) = elem.find("simpleType") {
            return self.convert_simple_type(st);
        }
        match elem.get("type") {
            Some(t) if !t.is_empty() => self.resolve_type_ref(t),
            _ => J::Obj(vec![("type".to_string(), J::s("string"))]),
        }
    }

    /// Resolve a type reference: a builtin `xs:*`, a `$ref` to a named type, else the string fallback.
    fn resolve_type_ref(&self, type_name: &str) -> J {
        if type_name.starts_with("xs:") {
            return resolve_builtin(type_name);
        }
        if self.simple.contains_key(type_name) || self.complex.contains_key(type_name) {
            return J::Obj(vec![(
                "$ref".to_string(),
                J::S(format!("#/$defs/{type_name}")),
            )]);
        }
        J::Obj(vec![("type".to_string(), J::s("string"))])
    }

    /// Collect `xs:attribute` definitions as properties (with `description`/`default`, coerced to the JSON type).
    fn collect_attributes(
        &self,
        parent: &Node,
        props: &mut Vec<(String, J)>,
        required: &mut Vec<String>,
    ) {
        for child in &parent.children {
            if child.name != "attribute" {
                continue;
            }
            let attr_name = match child.get("name") {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => continue,
            };
            let attr_type = child.get("type").unwrap_or("xs:string");
            let use_ = child.get("use").unwrap_or("optional");
            let default = child.get("default");
            let mut schema = as_obj(self.resolve_type_ref(attr_type));
            if let Some(d) = json_doc(child) {
                schema.push(("description".to_string(), J::S(d)));
            }
            if let Some(dv) = default {
                let coerced = coerce_default(dv, &schema);
                schema.push(("default".to_string(), coerced));
            }
            oset(props, &attr_name, J::Obj(schema));
            if use_ == "required" {
                required.push(attr_name);
            }
        }
    }

    /// The root `xs:element` (its inline complexType), else a bare object.
    fn convert_root_element(&self, elem: &Node) -> J {
        match elem.find("complexType") {
            Some(ct) => self.convert_complex_type(ct),
            None => J::Obj(vec![("type".to_string(), J::s("object"))]),
        }
    }
}

/// Map an XSD builtin to its JSON Schema fragment (unknown → string), key order preserved.
fn resolve_builtin(type_name: &str) -> J {
    let pairs: Vec<(String, J)> = match type_name {
        "xs:string" => vec![("type".to_string(), J::s("string"))],
        "xs:double" => vec![("type".to_string(), J::s("number"))],
        "xs:int" | "xs:integer" => vec![("type".to_string(), J::s("integer"))],
        "xs:unsignedInt" | "xs:unsignedLong" => {
            vec![
                ("type".to_string(), J::s("integer")),
                ("minimum".to_string(), J::Int(0)),
            ]
        }
        "xs:boolean" => vec![("type".to_string(), J::s("boolean"))],
        "xs:anyURI" => {
            vec![
                ("type".to_string(), J::s("string")),
                ("format".to_string(), J::s("uri")),
            ]
        }
        _ => vec![("type".to_string(), J::s("string"))],
    };
    J::Obj(pairs)
}

/// Coerce an XSD `default=` string to the JSON type of `schema` (`json.dump` renders a `number` through
/// `float.__repr__`; a non-parsing value stays a string), mirroring `_coerce_default`.
fn coerce_default(value: &str, schema: &[(String, J)]) -> J {
    let json_type = match oget(schema, "type") {
        Some(J::S(s)) => s.as_str(),
        _ => "string",
    };
    match json_type {
        "integer" => match value.parse::<i64>() {
            Ok(i) => J::Int(i),
            Err(_) => J::S(value.to_string()),
        },
        "number" => match value.parse::<f64>() {
            Ok(f) => J::Float(f),
            Err(_) => J::S(value.to_string()),
        },
        "boolean" => {
            let lv = value.to_lowercase();
            J::Bool(lv == "true" || lv == "1")
        }
        _ => J::S(value.to_string()),
    }
}

/// The `annotation/documentation` of `elem`, whitespace-STRIPPED (not collapsed): the JSON-schema
/// convention (`_get_documentation`: `text.strip()`); `None` when absent or all-whitespace.
fn json_doc(elem: &Node) -> Option<String> {
    let raw = doc_text(elem)?;
    let stripped = raw.trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

/// Deep-clone a [`J`] value (only ever needed for base-type property copies + the root passthrough).
fn clone_j(v: &J) -> J {
    match v {
        J::S(s) => J::S(s.clone()),
        J::Int(i) => J::Int(*i),
        J::Float(f) => J::Float(*f),
        J::Bool(b) => J::Bool(*b),
        J::Arr(a) => J::Arr(a.iter().map(clone_j).collect()),
        J::Obj(o) => J::Obj(o.iter().map(|(k, v)| (k.clone(), clone_j(v))).collect()),
    }
}

/// Serialize a JSON value exactly as CPython `json.dump(v, indent=2, ensure_ascii=False)` followed by a
/// trailing newline: 2-space indent, `": "` key / `,` item separators, non-ASCII kept literal.
fn dump_json(v: &J) -> String {
    let mut s = String::new();
    write_json(v, 0, &mut s);
    s.push('\n');
    s
}

fn write_json(v: &J, depth: usize, out: &mut String) {
    match v {
        J::S(s) => esc(s, out),
        J::Int(i) => out.push_str(&i.to_string()),
        J::Float(f) => out.push_str(&crate::validate::py_float_repr(*f)),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            let inner = "  ".repeat(depth + 1);
            for (i, it) in items.iter().enumerate() {
                out.push_str(&inner);
                write_json(it, depth + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&"  ".repeat(depth));
            out.push(']');
        }
        J::Obj(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let inner = "  ".repeat(depth + 1);
            for (i, (k, val)) in pairs.iter().enumerate() {
                out.push_str(&inner);
                esc(k, out);
                out.push_str(": ");
                write_json(val, depth + 1, out);
                out.push_str(if i + 1 < pairs.len() { ",\n" } else { "\n" });
            }
            out.push_str(&"  ".repeat(depth));
            out.push('}');
        }
    }
}

/// Escape a string as a JSON literal with `ensure_ascii=False`: only `"`, `\`, and control chars
/// (`< 0x20`) are escaped (short escapes for `\b\t\n\f\r`, else `\u00xx`); everything else (including
/// multi-byte UTF-8) is emitted verbatim, matching CPython's `json` encoder.
fn esc(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
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

/// Generate `hcdf.schema.json` (JSON Schema draft 2020-12) from `xsd`.
pub fn generate_json_schema(xsd: &str) -> Result<String, String> {
    let root = parse_xsd(xsd)?;
    let schema = SchemaIndex::build(&root)?;
    Ok(dump_json(&JsonGen::index(&schema).generate()))
}

struct ExactJsonGen<'a> {
    schema: &'a SchemaIndex<'a>,
}

impl<'a> ExactJsonGen<'a> {
    fn generate(&self) -> Result<J, String> {
        let mut definitions = Vec::new();
        let mut simple_names: Vec<&String> = self.schema.simple.keys().collect();
        simple_names.sort();
        for name in simple_names {
            definitions.push((name.clone(), self.simple_schema(self.schema.simple[name])?));
        }
        let mut complex_names: Vec<&String> = self.schema.complex.keys().collect();
        complex_names.sort();
        for name in complex_names {
            definitions.push((
                name.clone(),
                self.complex_schema(&self.schema.complex[name])?,
            ));
        }

        let root_schemas = self
            .schema
            .roots
            .iter()
            .map(|root| self.root_schema(root))
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = vec![
            (
                "$schema".to_string(),
                J::S("https://json-schema.org/draft/2020-12/schema".to_string()),
            ),
            (
                "$id".to_string(),
                J::S("https://hcdf.org/schema/hcdf.schema.json".to_string()),
            ),
            ("$defs".to_string(), J::Obj(definitions)),
        ];
        if root_schemas.len() == 1 {
            if let J::Obj(fields) = root_schemas.into_iter().next().unwrap() {
                result.extend(fields);
            }
        } else {
            result.push(("oneOf".to_string(), J::Arr(root_schemas)));
        }
        Ok(J::Obj(result))
    }

    fn root_schema(&self, root: &ElementParticle<'a>) -> Result<J, String> {
        match root.reference {
            Some(reference) => Err(format!(
                "global document root {:?} cannot be an element reference {reference:?}",
                root.name
            )),
            None => self.element_value_schema(root),
        }
    }

    fn simple_schema(&self, node: &Node) -> Result<J, String> {
        if let Some(restriction) = node.find("restriction") {
            let base = restriction.get("base").unwrap_or("xs:string");
            let mut schema = as_obj(self.type_schema(base)?);
            let values: Vec<J> = restriction
                .findall("enumeration")
                .filter_map(|value| value.get("value").map(J::s))
                .collect();
            if !values.is_empty() {
                oset(&mut schema, "enum", J::Arr(values));
            }
            if let Some(pattern) = restriction
                .find("pattern")
                .and_then(|value| value.get("value"))
            {
                oset(&mut schema, "pattern", J::S(pattern.to_string()));
            }
            if let Some(length) = restriction
                .find("length")
                .and_then(|value| value.get("value"))
            {
                let length = length
                    .parse::<i64>()
                    .map_err(|_| format!("simpleType has invalid xs:length value {length:?}"))?;
                oset(&mut schema, "minLength", J::Int(length));
                oset(&mut schema, "maxLength", J::Int(length));
            }
            return Ok(J::Obj(schema));
        }
        if node.find("list").is_some() || node.find("union").is_some() {
            return Err(
                "exact JSON Schema generation does not yet represent xs:list or xs:union"
                    .to_string(),
            );
        }
        Ok(J::Obj(vec![(
            "type".to_string(),
            J::S("string".to_string()),
        )]))
    }

    fn complex_schema(&self, definition: &ComplexTypeDef<'a>) -> Result<J, String> {
        let mut result = vec![("type".to_string(), J::S("object".to_string()))];
        let mut properties = Vec::new();
        let mut required = Vec::new();
        for attribute in &definition.attributes {
            let name = attribute
                .get("name")
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "xs:attribute is missing @name".to_string())?;
            let schema = if let Some(type_name) = attribute.get("type") {
                self.type_schema(type_name)?
            } else if let Some(simple) = attribute.find("simpleType") {
                self.simple_schema(simple)?
            } else {
                resolve_builtin("xs:string")
            };
            properties.push((name.to_string(), schema));
            if attribute.get("use") == Some("required") {
                required.push(J::S(name.to_string()));
            }
        }
        if definition.mixed {
            properties.push((
                "$text".to_string(),
                J::Obj(vec![(
                    "type".to_string(),
                    J::Arr(vec![J::S("string".to_string()), J::S("number".to_string())]),
                )]),
            ));
        }
        if !properties.is_empty() {
            result.push(("properties".to_string(), J::Obj(properties)));
        }
        if !required.is_empty() {
            result.push(("required".to_string(), J::Arr(required)));
        }

        let mut constraints = Vec::new();
        if let Some(base) = definition.extension_base {
            if !self.schema.complex.contains_key(base) {
                return Err(format!(
                    "complexType extends unknown or non-complex type {base:?}"
                ));
            }
            constraints.push(J::Obj(vec![(
                "$ref".to_string(),
                J::S(format!("#/$defs/{base}")),
            )]));
        }
        if let Some(particle) = &definition.particle {
            constraints.push(self.particle_schema(particle)?);
        }
        if !constraints.is_empty() {
            result.push(("allOf".to_string(), J::Arr(constraints)));
        }
        result.push(("unevaluatedProperties".to_string(), J::Bool(false)));
        Ok(J::Obj(result))
    }

    fn particle_schema(&self, particle: &Particle<'a>) -> Result<J, String> {
        match particle {
            Particle::Element(element) => self.element_property_schema(element),
            Particle::Sequence(group) | Particle::All(group) => {
                self.require_single_group(particle)?;
                let schemas = group
                    .children
                    .iter()
                    .map(|child| self.particle_schema(child))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(combine_all_of(schemas))
            }
            Particle::Choice(group) => {
                self.require_single_group(particle)?;
                let mut arm_names = Vec::new();
                for child in &group.children {
                    arm_names.push(particle_element_names(child)?);
                }
                let mut signatures = HashSet::new();
                for names in &arm_names {
                    let signature = names.iter().cloned().collect::<Vec<_>>().join("\u{1f}");
                    if !signatures.insert(signature) {
                        return Err(
                            "choice has contextually ambiguous arms with identical element names"
                                .to_string(),
                        );
                    }
                }
                let union: BTreeSet<String> = arm_names.iter().flatten().cloned().collect();
                let mut choices = Vec::new();
                for (child, names) in group.children.iter().zip(&arm_names) {
                    let mut constraints = vec![self.particle_schema(child)?];
                    let excluded: Vec<String> = union.difference(names).cloned().collect();
                    if !excluded.is_empty() {
                        constraints.push(excluded_properties_schema(&excluded));
                    }
                    choices.push(combine_all_of(constraints));
                }
                if group.occurs.min == 0 {
                    choices.push(excluded_properties_schema(
                        &union.into_iter().collect::<Vec<_>>(),
                    ));
                }
                Ok(J::Obj(vec![("oneOf".to_string(), J::Arr(choices))]))
            }
            Particle::Any(_) => Err(
                "exact JSON Schema generation cannot represent xs:any in the keyed XML-to-JSON view"
                    .to_string(),
            ),
        }
    }

    fn require_single_group(&self, particle: &Particle<'a>) -> Result<(), String> {
        if particle.occurs() != Occurs::ONCE
            && !matches!(particle, Particle::Choice(group) if group.occurs.min == 0 && group.occurs.max == Some(1))
        {
            return Err(format!(
                "exact JSON Schema generation cannot represent repeated {} groups",
                particle_kind(particle)
            ));
        }
        Ok(())
    }

    fn element_property_schema(&self, element: &ElementParticle<'a>) -> Result<J, String> {
        let value_schema = self.element_value_schema(element)?;
        let property_schema =
            if element.occurs.max == Some(0) {
                J::Bool(false)
            } else if element.occurs.is_repeated() {
                let mut array = vec![
                    ("type".to_string(), J::S("array".to_string())),
                    ("items".to_string(), value_schema),
                ];
                if element.occurs.min > 0 {
                    array.push((
                        "minItems".to_string(),
                        J::Int(i64::try_from(element.occurs.min).map_err(|_| {
                            "element minOccurs exceeds generator range".to_string()
                        })?),
                    ));
                }
                if let Some(maximum) = element.occurs.max {
                    array.push((
                        "maxItems".to_string(),
                        J::Int(i64::try_from(maximum).map_err(|_| {
                            "element maxOccurs exceeds generator range".to_string()
                        })?),
                    ));
                }
                J::Obj(array)
            } else {
                value_schema
            };
        let mut result = vec![(
            "properties".to_string(),
            J::Obj(vec![(element.name.clone(), property_schema)]),
        )];
        if element.occurs.min > 0 {
            result.push((
                "required".to_string(),
                J::Arr(vec![J::S(element.name.clone())]),
            ));
        }
        Ok(J::Obj(result))
    }

    fn element_value_schema(&self, element: &ElementParticle<'a>) -> Result<J, String> {
        if let Some(reference) = element.reference {
            return Err(format!(
                "exact JSON Schema generation does not inline element reference {reference:?}"
            ));
        }
        match element.type_use {
            ElementType::Named(name) => self.type_schema(name),
            ElementType::InlineSimple(simple) => self.simple_schema(simple),
            ElementType::InlineComplex(complex) => {
                self.complex_schema(&parse_complex_type(complex)?)
            }
            ElementType::Unspecified => Ok(resolve_builtin("xs:string")),
        }
    }

    fn type_schema(&self, type_name: &str) -> Result<J, String> {
        if type_name.starts_with("xs:") {
            return Ok(resolve_builtin(type_name));
        }
        if self.schema.simple.contains_key(type_name) || self.schema.complex.contains_key(type_name)
        {
            return Ok(J::Obj(vec![(
                "$ref".to_string(),
                J::S(format!("#/$defs/{type_name}")),
            )]));
        }
        Err(format!("reference to unknown named type {type_name:?}"))
    }
}

fn particle_kind(particle: &Particle<'_>) -> &'static str {
    match particle {
        Particle::Element(_) => "element",
        Particle::Sequence(_) => "sequence",
        Particle::Choice(_) => "choice",
        Particle::All(_) => "all",
        Particle::Any(_) => "any",
    }
}

fn particle_element_names(particle: &Particle<'_>) -> Result<BTreeSet<String>, String> {
    match particle {
        Particle::Element(element) => Ok(BTreeSet::from([element.name.clone()])),
        Particle::Sequence(group) | Particle::Choice(group) | Particle::All(group) => {
            let mut names = BTreeSet::new();
            for child in &group.children {
                names.extend(particle_element_names(child)?);
            }
            Ok(names)
        }
        Particle::Any(_) => {
            Err("choice containing xs:any has no finite contextual element-name set".to_string())
        }
    }
}

fn combine_all_of(mut schemas: Vec<J>) -> J {
    match schemas.len() {
        0 => J::Obj(Vec::new()),
        1 => schemas.pop().unwrap(),
        _ => J::Obj(vec![("allOf".to_string(), J::Arr(schemas))]),
    }
}

fn excluded_properties_schema(names: &[String]) -> J {
    let forbidden = names
        .iter()
        .map(|name| {
            J::Obj(vec![(
                "required".to_string(),
                J::Arr(vec![J::S(name.clone())]),
            )])
        })
        .collect();
    J::Obj(vec![(
        "not".to_string(),
        J::Obj(vec![("anyOf".to_string(), J::Arr(forbidden))]),
    )])
}

/// Generate the exact recursive JSON Schema projection used for the next atomic schema cut.
/// The legacy generator remains the byte-compatible production renderer until that cut lands.
pub fn generate_json_schema_exact(xsd: &str) -> Result<String, String> {
    let root = parse_xsd(xsd)?;
    let schema = SchemaIndex::build(&root)?;
    Ok(dump_json(&ExactJsonGen { schema: &schema }.generate()?))
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// spec-browser HTML generator (port of generate_spec_html.py)
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

// The page header/footer templates, extracted VERBATIM from `generate_spec_html.py` (`_header` split at
// its two `{title}` slots, `_footer` whole) so the emitted markup byte-matches the committed pages. The
// CSS carries literal `{`/`}`, so these are raw strings spliced by concatenation, never `format!`-ed.
const SPEC_HEAD_0: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>"##;
const SPEC_HEAD_1: &str = r##" Specification Browser</title>
<link rel="stylesheet" href="https://maxcdn.bootstrapcdn.com/bootstrap/3.3.7/css/bootstrap.min.css">
<link rel="stylesheet" href="/assets/style.css"/>
<style>
/* Spec browser overrides for dark theme */
.spec-container { max-width:1200px; margin:0 auto; padding:20px; }
.tree { background-color:#161b22; border-radius:6px; min-height:20px; padding:19px; overflow-y:auto; border:1px solid #30363d; }
.tree ul { margin:0; padding-left:25px; }
.tree li { list-style-type:none; margin:0 !important; margin-bottom:0 !important; padding:2px 0 0 2px; position:relative; }
.tree li::before, .tree li::after { display:none; }
.tree>ul>li::before, .tree>ul>li::after { display:none; }
.tree li.parent_li > span { cursor:pointer; transition:background-color 0.15s; }
.tree li.parent_li > span.tree-element:hover { background-color:#244a6f; border-color:#58a6ff; }
.tree li.parent_li > span.tree-attribute:hover { background-color:#2a5a3a; border-color:#3fb950; }
.tree-element { word-wrap:break-word; border:1px solid #2d5a8a; border-radius:5px; display:inline-block;
  line-height:14px; padding:4px 8px; background-color:#1a3a5c; color:#79c0ff; margin:2px 0; }
.tree-attribute { word-wrap:break-word; border:1px solid #2a6a3a; border-radius:5px; display:inline-block;
  line-height:14px; padding:4px 8px; background-color:#1a3a2a; color:#56d364; margin:2px 0; }
.tree-element h5, .tree-attribute h5 { display:inline; margin:0; padding:0; font-size:13px; color:inherit; font-weight:600;
  background:none; border:none; line-height:inherit; }
.tree-element h5 small, .tree-attribute h5 small { color:#8b949e; font-size:10px; font-weight:normal; }
.tree-element div, .tree-attribute div { margin-top:2px; }
.type-info { font-size:11px; color:#8b949e; padding:2px 4px; }
.desc { font-size:11px; color:#8b949e; padding:2px 4px; font-style:italic; }
.req { color:#f85149; font-weight:bold; }
.mult { color:#8b949e; }
.tree-collapse { margin-right:4px; cursor:pointer; font-size:10px; color:#8b949e; background:none !important;
  border:none !important; box-shadow:none !important; padding:0 !important; }
h1 { color:#e6edf3; }
h1 small { font-size:14px; color:#8b949e; }
.key-box { background:#161b22; padding:12px; border-radius:6px; margin-bottom:15px; border:1px solid #30363d; color:#e6edf3; }
.key-box span { margin-right:12px; }
.nav-tabs { border-bottom-color:#30363d !important; }
.nav-tabs > li > a { color:#8b949e !important; background:transparent !important; border-color:transparent !important; }
.nav-tabs > li > a:hover { color:#e6edf3 !important; background:#161b22 !important; border-color:#30363d #30363d transparent !important; }
.nav-tabs > li.active > a, .nav-tabs > li.active > a:focus, .nav-tabs > li.active > a:hover {
  color:#58a6ff !important; background:#161b22 !important; border-color:#30363d #30363d #161b22 !important; }
.tab-content { padding-top:15px; }
@media (max-width:768px) { .nav-tabs > li > a { font-size:10px; padding:6px 6px; } }
</style>
</head>
<body>

<nav>
  <a href="/" class="brand">HCDF</a>
  <button class="hamburger" aria-label="Menu">
    <span></span><span></span><span></span>
  </button>
  <div class="nav-links">
    <div class="dropdown">
      <a>Specification &#9662;</a>
      <div class="dropdown-content">
        <a href="/spec/">Overview</a>
        <a href="/spec/core.html">Core Schema</a>
        <a href="/spec/stream-profile.html">Stream Profile</a>
        <a href="/spec/extensions/ros2.html">ROS 2 Extension</a>
        <a href="/spec/extensions/ros2-control.html">ros2_control Extension</a>
        <a href="/spec/extensions/gazebo.html">Gazebo Extension</a>
        <a href="/spec/extensions/1722.html">IEEE 1722 Extension</a>
      </div>
    </div>
    <div class="dropdown">
      <a>Examples &#9662;</a>
      <div class="dropdown-content">
        <a href="/examples/">All Examples</a>
        <a href="/examples/#humanoid">Humanoid Mobile Base</a>
      </div>
    </div>
    <a href="/design/">Design</a>
    <div class="dropdown">
      <a>Extensions &#9662;</a>
      <div class="dropdown-content">
        <a href="/extensions/">Guide</a>
        <a href="/extensions/imu-stability.html">IMU Stability</a>
      </div>
    </div>
    <a href="/tools/">Tools</a>
    <a href="/about/">About</a>
    <a href="https://github.com/CogniPilot/hcdformat">GitHub</a>
  </div>
</nav>

<div class="spec-container">
<h1>"##;
const SPEC_HEAD_2: &str = r##" <small>Specification Browser</small></h1>
<div class="key-box">
  <span class="tree-collapse glyphicon glyphicon-stop" style="color:#1a3a5c"></span> Element
  <span class="tree-collapse glyphicon glyphicon-stop" style="color:#1a3a2a"></span> Attribute
  <span class="tree-collapse glyphicon glyphicon-plus"></span> Has children (click to expand)
  <span class="tree-collapse glyphicon glyphicon-minus"></span> Leaf node
  <span class="tree-collapse glyphicon glyphicon-chevron-right"></span> Defined in another tab (click to switch)
</div>
"##;
const SPEC_FOOTER: &str = r##"
</div>

<footer>
  <p>HCDF is a project of the <a href="https://cognipilot.org">CogniPilot Foundation</a>. Licensed under <a href="https://www.apache.org/licenses/LICENSE-2.0">Apache 2.0</a>.</p>
</footer>

<script src="https://code.jquery.com/jquery-1.12.4.min.js"></script>
<script src="https://maxcdn.bootstrapcdn.com/bootstrap/3.3.7/js/bootstrap.min.js"></script>
<script src="/assets/nav.js"></script>
<script>
function switchTab(tabId) {
    $('#spec-tabs a[href="#tab-' + tabId + '"]').tab('show');
}
$(function () {
    // Mark parent nodes
    $('.tree li:has(ul)').addClass('parent_li')
        .find(' > span.tree-element, > span.tree-attribute').attr('title', 'Collapse this branch');
    // Start collapsed
    $('.tree li.parent_li > ul > li').hide();
    // Toggle on click (only on tree-element/tree-attribute spans, not chevrons)
    $('.tree li.parent_li > span.tree-element, .tree li.parent_li > span.tree-attribute').on('click', function (e) {
        var children = $(this).parent('li.parent_li').find(' > ul > li');
        if (children.is(":visible")) {
            children.hide('fast');
        } else {
            children.show('fast');
        }
        e.stopPropagation();
    });
    // Cross-tab chevron clicks
    $('.tree .glyphicon-chevron-right[onclick]').on('click', function(e) {
        e.stopPropagation();
    });
});
</script>
</body>
</html>"##;

/// A spec-browser tab: `(tab_id, display_name, root_type_name)`. `root_type_name` is `None` for a tab
/// that renders the schema's ROOT element inline (via [`SpecGen::render_root`]); `Some(ty)` renders the
/// named complexType `ty`. Owned strings so the fixed [`CORE_TABS`]/[`STREAM_TABS`] and the
/// auto-discovered extension tabs flow through one code path. Mirrors the Python `TABS` tuples.
type Tab = (String, String, Option<String>);

/// The FIXED core-schema tab layout (`generate_spec_html.py`'s `TABS`): the `<hcdf>` root first (rendered
/// inline), then one tab per top-level subsystem complexType. The display names + tab ids are frozen so
/// the emitted markup byte-matches the committed pages.
const CORE_TABS: &[(&str, &str, Option<&str>)] = &[
    ("hcdf", "HCDF", None),
    ("comp", "Comp", Some("comp")),
    ("joint", "Joint", Some("joint")),
    ("group", "Group", Some("joint_group")),
    ("state", "State", Some("kinematic_state")),
    (
        "component-function",
        "Component Function",
        Some("connectivity_function"),
    ),
    ("sensor", "Sensor", Some("sensor")),
    ("motor", "Motor", Some("motor")),
    ("hmi", "HMI", Some("hmi_element")),
    (
        "dynamic-surface",
        "Dynamic Surface",
        Some("dynamic_surface"),
    ),
    ("power", "Power", Some("power_source")),
    ("port", "Port", Some("connectivity_port")),
    ("channel", "Channel", Some("connectivity_channel")),
    ("connector", "Connector", Some("connectivity_connector")),
    ("antenna", "Antenna", Some("connectivity_antenna")),
    ("assembly", "Assembly", Some("connectivity_assembly")),
    ("binding", "Binding", Some("connectivity_binding")),
    ("mate", "Mate", Some("connectivity_mate")),
    ("link", "Link", Some("connectivity_link")),
    ("bus", "Bus", Some("connectivity_bus")),
    ("chain", "Chain", Some("connectivity_chain")),
    ("star", "Star", Some("connectivity_star")),
    ("ring", "Ring", Some("connectivity_ring")),
    ("mesh", "Mesh", Some("connectivity_mesh")),
    ("tree", "Tree", Some("connectivity_tree")),
    (
        "network-configuration",
        "Network Configuration",
        Some("connectivity_network_configuration"),
    ),
    ("transmission", "Transmission", Some("transmission")),
    ("color", "Color", Some("color")),
    ("geometry", "Geometry", Some("geometry")),
];

/// The FIXED stream-profile tab layout (`generate_spec_html.py`'s `STREAM_TABS`).
const STREAM_TABS: &[(&str, &str, Option<&str>)] = &[
    ("stream-profile", "Stream Profile", None),
    ("stream", "Stream", Some("stream_profile_stream")),
    ("stream-group", "Stream Group", Some("stream_profile_group")),
    ("frer", "FRER", Some("stream_profile_frer")),
];

/// Promote a fixed `&'static` tab table to owned [`Tab`]s (so fixed + auto-discovered tabs share one type).
fn const_tabs(t: &[(&str, &str, Option<&str>)]) -> Vec<Tab> {
    t.iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.map(|s| s.to_string())))
        .collect()
}

/// HTML-escape a string exactly as the Python `_esc`: `&`→`&amp;`, `<`→`&lt;`, `>`→`&gt;`, `"`→`&quot;`,
/// applied IN THAT ORDER (so a literal `&` becomes `&amp;` and is not re-escaped). `'` is left as-is.
fn spec_esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Python `str.title()` for the ASCII identifier fragments the extension/auto-discover paths title-case:
/// the FIRST cased (alphabetic) char of each run is upper-cased, the rest lower-cased, and any non-cased
/// char (digit, space) resets the run. Reproduces e.g. `topic_map`→`Topic Map`, `control`→`Control`.
fn py_title(s: &str) -> String {
    let mut out = String::new();
    let mut prev_cased = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_cased = true;
        } else {
            out.push(c);
            prev_cased = false;
        }
    }
    out
}

/// The extension-title token overrides (`_detect_schema_kind`'s `known_upper`): tokens whose canonical
/// casing is not a plain title-case (`ros2`→`ROS 2`, `1722`→`IEEE 1722`, …). Any other token falls back
/// to [`py_title`].
fn known_upper(w: &str) -> Option<&'static str> {
    match w {
        "ros2" => Some("ROS 2"),
        "ros" => Some("ROS"),
        "gazebo" => Some("Gazebo"),
        "isaac" => Some("Isaac"),
        "nav2" => Some("Nav2"),
        "moveit" => Some("MoveIt"),
        "1722" => Some("IEEE 1722"),
        _ => None,
    }
}

/// Decide the page title + tab layout from the INPUT FILENAME, exactly as `_detect_schema_kind`: a
/// `stream` (non-`ext`) path is the stream profile, a non-`ext`/non-`extension` path is the core schema
/// (both with a FIXED tab table), and anything else is an extension: its title is derived from the
/// `hcdf-ext-<name>` stem and its tabs are auto-discovered (`None` return ⇒ [`auto_discover_tabs`]).
fn detect_schema_kind(xsd_path: &str) -> (String, Option<Vec<Tab>>) {
    let lower = xsd_path.to_lowercase();
    if lower.contains("stream") && !lower.contains("ext") {
        return (
            "HCDF Stream Profile 1.0".to_string(),
            Some(const_tabs(STREAM_TABS)),
        );
    }
    if !lower.contains("ext") && !lower.contains("extension") {
        return ("HCDF 1.0".to_string(), Some(const_tabs(CORE_TABS)));
    }
    // Extension XSD: derive the title from the file stem, e.g. `hcdf-ext-ros2` → `HCDF ROS 2 Extension`.
    let base = Path::new(xsd_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut label = base.as_str();
    for prefix in ["hcdf-ext-", "hcdf-extension-"] {
        if base.to_ascii_lowercase().starts_with(prefix) {
            label = &base[prefix.len()..];
            break;
        }
    }
    let spaced = label.replace(['-', '_'], " ");
    let titled: Vec<String> = spaced
        .split_whitespace()
        .map(|w| match known_upper(&w.to_lowercase()) {
            Some(m) => m.to_string(),
            None => py_title(w),
        })
        .collect();
    (format!("HCDF {} Extension", titled.join(" ")), None)
}

/// Build the tab layout for an extension (or any schema without a fixed table), mirroring
/// `_auto_discover_tabs`: a first tab for the ROOT element (rendering its referenced named type directly
/// when it has one, else its inline type via `render_root`), then one tab per remaining named
/// complexType in DOCUMENT order (skipping the one the root already renders).
fn auto_discover_tabs(parser: &SpecParser) -> Vec<Tab> {
    let mut tabs: Vec<Tab> = Vec::new();
    let mut root_type_refs = HashSet::new();
    for root in parser.roots {
        let root_el = root.node;
        let root_name = root_el.get("name").unwrap_or("root").to_string();
        let display = py_title(&root_name.replace(['-', '_'], " "));
        let root_type_ref = root_el.get("type").map(|s| s.to_string());
        match &root_type_ref {
            Some(rtr) if !rtr.is_empty() && parser.types.contains_key(rtr) => {
                root_type_refs.insert(rtr.clone());
                tabs.push((root_name, display, Some(rtr.clone())));
            }
            _ => tabs.push((root_name, display, None)),
        }
    }
    for (type_name, _) in &parser.types_order {
        if root_type_refs.contains(type_name) {
            continue;
        }
        let display = py_title(&type_name.replace(['_', '-'], " "));
        tabs.push((type_name.clone(), display, Some(type_name.clone())));
    }
    tabs
}

/// One attribute as the Python `_parse_attribute` dict: its name, type ref, required flag, `default=`,
/// the enumeration value set (empty when the type is not an enum), and the collapsed documentation.
struct SpecAttr {
    name: String,
    type_name: String,
    required: bool,
    default: Option<String>,
    enum_vals: Vec<String>,
    doc: Option<String>,
}

/// One element as the Python `_parse_element` dict: name, type ref, required/multiple cardinality,
/// whether the type has children (is a named complexType), whether it is an enum (+ its values), and the
/// documentation.
struct SpecElem {
    name: String,
    type_name: String,
    required: bool,
    multiple: bool,
    has_children: bool,
    is_enum: bool,
    enum_vals: Vec<String>,
    doc: Option<String>,
}

/// The indexed XSD view the spec generator walks (`XsdParser`): the top-level named complexTypes (in
/// document order for auto-discover + a set for the `type in types` test) and the enumerated
/// simpleTypes (name → value set). Only top-level named types are indexed, exactly like the Python.
struct SpecParser<'a> {
    roots: &'a [ElementParticle<'a>],
    /// Top-level named complexTypes in DOCUMENT order (auto-discover iterates these).
    types_order: Vec<(String, &'a Node)>,
    /// Membership + lookup for the top-level named complexTypes.
    types: HashMap<String, &'a Node>,
    /// Enumerated simpleType name → literal value set (schema order).
    enums: HashMap<String, Vec<String>>,
}

impl<'a> SpecParser<'a> {
    fn index(schema: &'a SchemaIndex<'a>) -> Self {
        let types_order: Vec<(String, &Node)> = schema
            .complex_order
            .iter()
            .map(|(name, definition)| (name.clone(), definition.node))
            .collect();
        let types: HashMap<String, &Node> = schema
            .complex
            .iter()
            .map(|(name, definition)| (name.clone(), definition.node))
            .collect();
        let mut enums = HashMap::new();
        for (name, simple) in &schema.simple_order {
            let vals = enum_values(simple);
            if !vals.is_empty() {
                enums.insert(name.clone(), vals);
            }
        }
        SpecParser {
            roots: &schema.roots,
            types_order,
            types,
            enums,
        }
    }

    /// `annotation/documentation` text, whitespace-STRIPPED (the Python `get_annotation`); `None` when
    /// absent or all-whitespace (both render as "no doc" in the emit guards, so the distinction is inert).
    fn annotation(&self, elem: &Node) -> Option<String> {
        let doc = elem.find("annotation")?.find("documentation")?;
        if doc.text.is_empty() {
            return None;
        }
        let t = doc.text.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }

    /// Port of `_parse_attribute`.
    fn parse_attribute(&self, attr: &Node) -> SpecAttr {
        let name = attr.get("name").unwrap_or("?").to_string();
        let type_name = attr.get("type").unwrap_or("xs:string").to_string();
        SpecAttr {
            required: attr.get("use") == Some("required"),
            default: attr.get("default").map(|s| s.to_string()),
            enum_vals: self.enums.get(&type_name).cloned().unwrap_or_default(),
            doc: self.annotation(attr),
            name,
            type_name,
        }
    }

    /// Port of `_parse_element`. `required` is false inside a `choice` (the compositor), and `multiple`
    /// tracks `maxOccurs="unbounded"`.
    fn parse_element(&self, elem: &Node, compositor: &str) -> SpecElem {
        let name = elem.get("name").unwrap_or("?").to_string();
        let type_name = elem.get("type").unwrap_or("").to_string();
        let required = elem.get("minOccurs").unwrap_or("1") != "0" && compositor != "choice";
        let multiple = elem.get("maxOccurs").unwrap_or("1") == "unbounded";
        SpecElem {
            has_children: self.types.contains_key(&type_name),
            is_enum: self.enums.contains_key(&type_name),
            enum_vals: self.enums.get(&type_name).cloned().unwrap_or_default(),
            doc: self.annotation(elem),
            required,
            multiple,
            name,
            type_name,
        }
    }

    /// Every child element + attribute of a named complexType (`get_type_children`), following a single
    /// `complexContent/extension` to prepend the base type's children first. The non-extension path also
    /// picks up a `choice` NESTED directly in a `sequence` (elements marked as the `choice` compositor).
    fn type_children(&self, type_name: &str) -> (Vec<SpecElem>, Vec<SpecAttr>) {
        let Some(t) = self.types.get(type_name).copied() else {
            return (Vec::new(), Vec::new());
        };
        let mut elements = Vec::new();
        let mut attributes = Vec::new();

        if let Some(ext) = t.find("complexContent").and_then(|cc| cc.find("extension")) {
            if let Some(base) = ext.get("base").filter(|b| !b.is_empty()) {
                let (be, ba) = self.type_children(base);
                elements.extend(be);
                attributes.extend(ba);
            }
            for attr in ext.findall("attribute") {
                attributes.push(self.parse_attribute(attr));
            }
            for ctag in ["sequence", "all", "choice"] {
                if let Some(container) = ext.find(ctag) {
                    for child in container.findall("element") {
                        elements.push(self.parse_element(child, ctag));
                    }
                }
            }
            return (elements, attributes);
        }

        for attr in t.findall("attribute") {
            attributes.push(self.parse_attribute(attr));
        }
        for ctag in ["sequence", "all", "choice"] {
            if let Some(container) = t.find(ctag) {
                for child in container.findall("element") {
                    elements.push(self.parse_element(child, ctag));
                }
                if ctag == "sequence" {
                    for inner_choice in container.findall("choice") {
                        for child in inner_choice.findall("element") {
                            elements.push(self.parse_element(child, "choice"));
                        }
                    }
                }
            }
        }
        (elements, attributes)
    }
}

/// The spec-browser HTML emitter (`HtmlGenerator`): a parser view + the resolved title + tab layout. The
/// tree is expanded to a fixed depth (6), and cross-type references become a "switch tab" chevron.
struct SpecGen<'a> {
    parser: &'a SpecParser<'a>,
    title: String,
    tabs: Vec<Tab>,
}

impl SpecGen<'_> {
    /// The tree recursion depth cap (`HtmlGenerator.depth_limit`): a `render_type` past this yields ""
    /// and an element at this depth is treated as a leaf (no expand).
    const DEPTH_LIMIT: usize = 6;

    /// Assemble the whole page: header, tab bar, one `tab-pane` per tab (root-inline or named-type tree),
    /// footer, joined with `\n`, exactly like `HtmlGenerator.generate`.
    fn generate(&self) -> String {
        let first_tab_id = &self.tabs[0].0;
        let mut parts: Vec<String> = vec![
            self.header(),
            self.tabs_html(),
            r#"<div class="tab-content">"#.to_string(),
        ];
        for (tab_id, _tab_name, type_name) in &self.tabs {
            let active = if tab_id == first_tab_id { "active" } else { "" };
            parts.push(format!(
                r#"<div class="tab-pane {active}" id="tab-{tab_id}">"#
            ));
            parts.push(r#"<div class="tree"><ul>"#.to_string());
            match type_name {
                None => parts.push(self.render_root(tab_id)),
                Some(tn) if !tn.is_empty() => parts.push(self.render_type(tn, 0)),
                Some(_) => {}
            }
            parts.push(r#"</ul></div></div>"#.to_string());
        }
        parts.push("</div>".to_string());
        parts.push(SPEC_FOOTER.to_string());
        parts.join("\n")
    }

    /// Render the root `<hcdf>` (or extension root) element's attributes + child elements inline
    /// (`_render_root`): the root's inline complexType is walked for attributes then sequence/all/choice.
    fn render_root(&self, root_name: &str) -> String {
        let Some(root_el) = self
            .parser
            .roots
            .iter()
            .find(|root| root.name == root_name)
            .or_else(|| self.parser.roots.first())
            .map(|root| root.node)
        else {
            return "<li>No root element found</li>".to_string();
        };
        let Some(ct) = root_el.find("complexType") else {
            return "<li>No complex type</li>".to_string();
        };
        let mut lines: Vec<String> = Vec::new();
        for attr in ct.findall("attribute") {
            lines.push(self.attr_li(&self.parser.parse_attribute(attr)));
        }
        for ctag in ["sequence", "all", "choice"] {
            if let Some(container) = ct.find(ctag) {
                for child in container.findall("element") {
                    lines.push(self.element_li(&self.parser.parse_element(child, ctag), 0));
                }
            }
        }
        lines.join("\n")
    }

    /// Render all children of a named type as `<li>`s (`_render_type`): attributes first, then elements.
    /// Past [`DEPTH_LIMIT`] the recursion yields "".
    fn render_type(&self, type_name: &str, depth: usize) -> String {
        if depth > Self::DEPTH_LIMIT {
            return String::new();
        }
        let (elements, attributes) = self.parser.type_children(type_name);
        let mut lines: Vec<String> = Vec::new();
        for a in &attributes {
            lines.push(self.attr_li(a));
        }
        for e in &elements {
            lines.push(self.element_li(e, depth));
        }
        lines.join("\n")
    }

    /// Render one element `<li>` (`_element_li`): the icon is a cross-tab chevron when the element's type
    /// is another tab's root, else a +/- expand/leaf glyph; a same-tab child type recurses inline.
    fn element_li(&self, e: &SpecElem, depth: usize) -> String {
        let name_esc = spec_esc(&e.name);
        let has_children = e.has_children && depth < Self::DEPTH_LIMIT;
        let tab_ref = self
            .tabs
            .iter()
            .find(|(_, _, tab_type)| tab_type.as_deref() == Some(e.type_name.as_str()))
            .map(|(tab_id, _, _)| tab_id.as_str());
        let (icon, elem_extra) = if let Some(tr) = tab_ref {
            let tab_link = format!(r#" onclick="switchTab('{tr}')""#);
            (
                format!(
                    r#"<span class="tree-collapse glyphicon glyphicon-chevron-right" style="cursor:pointer"{tab_link}></span>"#
                ),
                format!(r#" style="cursor:pointer"{tab_link}"#),
            )
        } else if has_children {
            (
                r#"<span class="tree-collapse glyphicon glyphicon-plus"></span>"#.to_string(),
                String::new(),
            )
        } else {
            (
                r#"<span class="tree-collapse glyphicon glyphicon-minus"></span>"#.to_string(),
                String::new(),
            )
        };
        let req = if e.required {
            r#" <small class="req">(required)</small>"#
        } else {
            ""
        };
        let mult = if e.multiple {
            r#" <small class="mult">[0..*]</small>"#
        } else {
            ""
        };
        let type_str = if e.is_enum {
            let vals = e.enum_vals.join(" | ");
            format!(r#"<div class="type-info"><b>Values:</b> {vals}</div>"#)
        } else if !e.type_name.is_empty() && !self.parser.types.contains_key(&e.type_name) {
            format!(
                r#"<div class="type-info"><b>Type:</b> {}</div>"#,
                spec_esc(&e.type_name)
            )
        } else {
            String::new()
        };
        let doc = match &e.doc {
            Some(d) => format!(r#"<div class="desc">{}</div>"#, spec_esc(d)),
            None => String::new(),
        };
        let children_html = if has_children && tab_ref.is_none() {
            let inner = self.render_type(&e.type_name, depth + 1);
            if inner.is_empty() {
                String::new()
            } else {
                format!("<ul>{inner}</ul>")
            }
        } else {
            String::new()
        };
        format!(
            r#"<li>{icon}<span class="tree-element"{elem_extra}><h5>&lt;{name_esc}&gt; <small>Element</small></h5>{req}{mult}{type_str}{doc}</span>{children_html}</li>"#
        )
    }

    /// Render one attribute `<li>` (`_attr_li`): the enum value set replaces the type when present, and
    /// the `default=`/`(required)`/`: doc` suffixes follow the Python truthiness rules.
    fn attr_li(&self, a: &SpecAttr) -> String {
        let name_esc = spec_esc(&a.name);
        let req = if a.required { " (required)" } else { "" };
        let default = match a.default.as_deref().filter(|d| !d.is_empty()) {
            Some(d) => format!(r#", default="{}""#, spec_esc(d)),
            None => String::new(),
        };
        let type_str = if a.enum_vals.is_empty() {
            spec_esc(&a.type_name)
        } else {
            a.enum_vals.join(" | ")
        };
        let doc = match &a.doc {
            Some(d) => format!(": {}", spec_esc(d)),
            None => String::new(),
        };
        format!(
            r#"<li><span class="tree-attribute"><h5>{name_esc} <small>Attribute</small></h5><div class="type-info"><b>Type:</b> {type_str}{req}{default}{doc}</div></span></li>"#
        )
    }

    /// The tab bar (`_tabs_html`): the first tab carries `class="active"`; the display name is NOT escaped
    /// (matching the Python, which trusts the fixed/derived tab names).
    fn tabs_html(&self) -> String {
        let mut lines =
            vec![r#"<ul class="nav nav-tabs" role="tablist" id="spec-tabs">"#.to_string()];
        for (i, (tab_id, tab_name, _)) in self.tabs.iter().enumerate() {
            let active = if i == 0 { r#" class="active""# } else { "" };
            lines.push(format!(
                r##"<li{active}><a href="#tab-{tab_id}" data-toggle="tab">{tab_name}</a></li>"##
            ));
        }
        lines.push("</ul>".to_string());
        lines.join("\n")
    }

    /// The page header (`_header`): the three frozen template chunks with the escaped title spliced into
    /// its two `{title}` slots (`<title>` + `<h1>`). Concatenated (not `format!`d) so the literal CSS
    /// braces in the template need no escaping.
    fn header(&self) -> String {
        let title = spec_esc(&self.title);
        let mut s = String::with_capacity(
            SPEC_HEAD_0.len() + SPEC_HEAD_1.len() + SPEC_HEAD_2.len() + 2 * title.len(),
        );
        s.push_str(SPEC_HEAD_0);
        s.push_str(&title);
        s.push_str(SPEC_HEAD_1);
        s.push_str(&title);
        s.push_str(SPEC_HEAD_2);
        s
    }
}

/// Generate the spec-browser HTML for `xsd`, dispatching title/tabs from `xsd_path` (the Python
/// `main`'s filename detection). `xsd_path` need not exist (only its basename drives the schema-kind
/// decision), so a caller can pass the canonical name (`hcdf.xsd`) while feeding embedded bytes.
pub fn generate_spec_html(xsd: &str, xsd_path: &str) -> Result<String, String> {
    let root = parse_xsd(xsd)?;
    let schema = SchemaIndex::build(&root)?;
    let parser = SpecParser::index(&schema);
    let (title, tabs) = detect_schema_kind(xsd_path);
    let tabs = tabs.unwrap_or_else(|| auto_discover_tabs(&parser));
    for (tab_id, _, type_name) in &tabs {
        if let Some(type_name) = type_name {
            if !parser.types.contains_key(type_name) {
                return Err(format!(
                    "spec tab {tab_id:?} references unknown complex type {type_name:?}"
                ));
            }
        }
    }
    Ok(SpecGen {
        parser: &parser,
        title,
        tabs,
    }
    .generate())
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════════
// hcdf.completions.json contextual generator
// ═══════════════════════════════════════════════════════════════════════════════════════════════════

struct ContextCompletionGen<'a> {
    schema: &'a SchemaIndex<'a>,
    types: HashMap<String, J>,
    particles: HashMap<String, J>,
}

impl<'a> ContextCompletionGen<'a> {
    fn new(schema: &'a SchemaIndex<'a>) -> Self {
        Self {
            schema,
            types: HashMap::new(),
            particles: HashMap::new(),
        }
    }

    fn generate(mut self) -> Result<J, String> {
        for (name, simple) in self.schema.simple_order.clone() {
            self.register_simple_type(&format!("named:{name}"), Some(&name), simple)?;
        }
        for (name, complex) in self.schema.complex_order.clone() {
            self.register_complex_type(&format!("named:{name}"), Some(&name), &complex)?;
        }

        let mut roots = Vec::new();
        for index in 0..self.schema.roots.len() {
            let root = self.schema.roots[index].clone();
            let particle_id = format!("root:{}", root.name);
            self.register_particle(&particle_id, &Particle::Element(root.clone()))?;
            roots.push((
                root.name,
                J::Obj(vec![("particle".to_string(), J::S(particle_id))]),
            ));
        }

        Ok(J::Obj(vec![
            (
                "format".to_string(),
                J::S("hcdf-contextual-completions-v2".to_string()),
            ),
            ("roots".to_string(), J::Obj(roots)),
            (
                "types".to_string(),
                J::Obj(self.types.into_iter().collect()),
            ),
            (
                "particles".to_string(),
                J::Obj(self.particles.into_iter().collect()),
            ),
        ]))
    }

    fn register_simple_type(
        &mut self,
        id: &str,
        name: Option<&str>,
        node: &'a Node,
    ) -> Result<(), String> {
        if self.types.contains_key(id) {
            return Err(format!("ambiguous contextual type definition {id:?}"));
        }
        let restriction = node.find("restriction");
        let base = restriction
            .and_then(|value| value.get("base"))
            .unwrap_or("xs:string");
        let mut entry = vec![
            ("kind".to_string(), J::S("simple".to_string())),
            ("base".to_string(), J::S(base.to_string())),
        ];
        if let Some(name) = name {
            entry.push(("name".to_string(), J::S(name.to_string())));
        }
        if let Some(annotation) = normalized_annotation(doc_text(node)) {
            entry.push(("annotation".to_string(), J::S(annotation)));
        }
        let values = enum_values(node);
        if !values.is_empty() {
            entry.push((
                "enum".to_string(),
                J::Arr(values.into_iter().map(J::S).collect()),
            ));
        }
        self.types.insert(id.to_string(), J::Obj(entry));
        Ok(())
    }

    fn register_complex_type(
        &mut self,
        id: &str,
        name: Option<&str>,
        definition: &ComplexTypeDef<'a>,
    ) -> Result<(), String> {
        if self.types.contains_key(id) {
            return Err(format!("ambiguous contextual type definition {id:?}"));
        }
        self.types.insert(id.to_string(), J::Obj(Vec::new()));

        let mut entry = vec![
            ("kind".to_string(), J::S("complex".to_string())),
            ("mixed".to_string(), J::Bool(definition.mixed)),
        ];
        if let Some(name) = name {
            entry.push(("name".to_string(), J::S(name.to_string())));
        }
        if let Some(annotation) = normalized_annotation(doc_text(definition.node)) {
            entry.push(("annotation".to_string(), J::S(annotation)));
        }
        if let Some(base) = definition.extension_base {
            let target = self.named_type_id(base)?;
            entry.push(("base".to_string(), J::S(target)));
        }
        let attributes = definition
            .attributes
            .iter()
            .map(|attribute| self.attribute_entry(attribute))
            .collect::<Result<Vec<_>, _>>()?;
        entry.push(("attributes".to_string(), J::Arr(attributes)));
        if let Some(particle) = &definition.particle {
            let particle_id = format!("{id}/content");
            self.register_particle(&particle_id, particle)?;
            entry.push(("content".to_string(), J::S(particle_id)));
        }
        self.types.insert(id.to_string(), J::Obj(entry));
        Ok(())
    }

    fn attribute_entry(&self, attribute: &Node) -> Result<J, String> {
        let name = attribute
            .get("name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "xs:attribute is missing @name".to_string())?;
        let mut entry = vec![
            ("name".to_string(), J::S(name.to_string())),
            (
                "required".to_string(),
                J::Bool(attribute.get("use") == Some("required")),
            ),
        ];
        if let Some(type_name) = attribute.get("type") {
            entry.push(("type".to_string(), J::S(self.type_reference(type_name)?)));
        } else if let Some(simple) = attribute.find("simpleType") {
            let values = enum_values(simple);
            entry.push(("type".to_string(), J::S("inline-simple".to_string())));
            if !values.is_empty() {
                entry.push((
                    "enum".to_string(),
                    J::Arr(values.into_iter().map(J::S).collect()),
                ));
            }
        } else {
            entry.push(("type".to_string(), J::S("builtin:xs:string".to_string())));
        }
        if let Some(default) = attribute.get("default") {
            entry.push(("default".to_string(), J::S(default.to_string())));
        }
        if let Some(annotation) = normalized_annotation(doc_text(attribute)) {
            entry.push(("annotation".to_string(), J::S(annotation)));
        }
        Ok(J::Obj(entry))
    }

    fn register_particle(&mut self, id: &str, particle: &Particle<'a>) -> Result<(), String> {
        if self.particles.contains_key(id) {
            return Err(format!("ambiguous contextual particle definition {id:?}"));
        }
        self.particles.insert(id.to_string(), J::Obj(Vec::new()));

        let mut entry = occurs_entry(particle.occurs())?;
        match particle {
            Particle::Element(element) => {
                entry.insert(0, ("kind".to_string(), J::S("element".to_string())));
                entry.push(("name".to_string(), J::S(element.name.clone())));
                if let Some(reference) = element.reference {
                    let local = reference.rsplit(':').next().unwrap_or(reference);
                    if !self.schema.roots.iter().any(|root| root.name == local) {
                        return Err(format!(
                            "xs:element {:?} references unknown global element {reference:?}",
                            element.name
                        ));
                    }
                    entry.push(("elementRef".to_string(), J::S(format!("root:{local}"))));
                } else {
                    let type_id = match element.type_use {
                        ElementType::Named(name) => self.type_reference(name)?,
                        ElementType::InlineSimple(simple) => {
                            let type_id = format!("inline:{id}");
                            self.register_simple_type(&type_id, None, simple)?;
                            type_id
                        }
                        ElementType::InlineComplex(complex) => {
                            let definition = parse_complex_type(complex)?;
                            let type_id = format!("inline:{id}");
                            self.register_complex_type(&type_id, None, &definition)?;
                            type_id
                        }
                        ElementType::Unspecified => "builtin:xs:string".to_string(),
                    };
                    entry.push(("type".to_string(), J::S(type_id)));
                }
                if let Some(annotation) = normalized_annotation(element.annotation) {
                    entry.push(("annotation".to_string(), J::S(annotation)));
                }
            }
            Particle::Sequence(group) | Particle::Choice(group) | Particle::All(group) => {
                let kind = match particle {
                    Particle::Sequence(_) => "sequence",
                    Particle::Choice(_) => "choice",
                    Particle::All(_) => "all",
                    _ => unreachable!(),
                };
                entry.insert(0, ("kind".to_string(), J::S(kind.to_string())));
                let mut children = Vec::new();
                for (index, child) in group.children.iter().enumerate() {
                    let child_id = format!("{id}/{index}");
                    self.register_particle(&child_id, child)?;
                    children.push(J::S(child_id));
                }
                entry.push(("children".to_string(), J::Arr(children)));
                if let Some(annotation) = normalized_annotation(group.annotation) {
                    entry.push(("annotation".to_string(), J::S(annotation)));
                }
            }
            Particle::Any(any) => {
                entry.insert(0, ("kind".to_string(), J::S("any".to_string())));
                if let Some(namespace) = any.namespace {
                    entry.push(("namespace".to_string(), J::S(namespace.to_string())));
                }
                if let Some(process_contents) = any.process_contents {
                    entry.push((
                        "processContents".to_string(),
                        J::S(process_contents.to_string()),
                    ));
                }
                if let Some(annotation) = normalized_annotation(any.annotation) {
                    entry.push(("annotation".to_string(), J::S(annotation)));
                }
            }
        }
        self.particles.insert(id.to_string(), J::Obj(entry));
        Ok(())
    }

    fn named_type_id(&self, name: &str) -> Result<String, String> {
        if self.schema.simple.contains_key(name) || self.schema.complex.contains_key(name) {
            Ok(format!("named:{name}"))
        } else {
            Err(format!("reference to unknown named type {name:?}"))
        }
    }

    fn type_reference(&self, name: &str) -> Result<String, String> {
        if name.starts_with("xs:") {
            Ok(format!("builtin:{name}"))
        } else {
            self.named_type_id(name)
        }
    }
}

fn normalized_annotation(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn occurs_entry(occurs: Occurs) -> Result<Vec<(String, J)>, String> {
    let min = i64::try_from(occurs.min)
        .map_err(|_| format!("minOccurs {} exceeds generator range", occurs.min))?;
    let max = match occurs.max {
        Some(value) => J::Int(
            i64::try_from(value)
                .map_err(|_| format!("maxOccurs {value} exceeds generator range"))?,
        ),
        None => J::S("unbounded".to_string()),
    };
    Ok(vec![
        ("minOccurs".to_string(), J::Int(min)),
        ("maxOccurs".to_string(), max),
    ])
}

/// Generate the canonical contextual completion model. Repeated local names in distinct particles
/// retain separate stable identities, so consumers never need a lossy name-only fallback.
pub fn generate_completions_json(xsd: &str) -> Result<String, String> {
    let root = parse_xsd(xsd)?;
    let schema = SchemaIndex::build(&root)?;
    let model = ContextCompletionGen::new(&schema).generate()?;
    Ok(dump_completions(&model))
}

/// Serialize a [`J`] value exactly as CPython `json.dump(v, indent=1, sort_keys=True)` (the default
/// `ensure_ascii=True`) with NO trailing newline: 1-space indent per level, `": "`/`,` separators,
/// object keys sorted by code point, non-ASCII `\u`-escaped.
fn dump_completions(v: &J) -> String {
    let mut s = String::new();
    write_completions_json(v, 0, &mut s);
    s
}

fn write_completions_json(v: &J, depth: usize, out: &mut String) {
    match v {
        J::S(s) => esc_ascii(s, out),
        J::Int(i) => out.push_str(&i.to_string()),
        J::Float(f) => out.push_str(&crate::validate::py_float_repr(*f)),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            let inner = " ".repeat(depth + 1);
            for (i, it) in items.iter().enumerate() {
                out.push_str(&inner);
                write_completions_json(it, depth + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&" ".repeat(depth));
            out.push(']');
        }
        J::Obj(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut sorted: Vec<&(String, J)> = pairs.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            out.push_str("{\n");
            let inner = " ".repeat(depth + 1);
            for (i, (k, val)) in sorted.iter().enumerate() {
                out.push_str(&inner);
                esc_ascii(k, out);
                out.push_str(": ");
                write_completions_json(val, depth + 1, out);
                out.push_str(if i + 1 < sorted.len() { ",\n" } else { "\n" });
            }
            out.push_str(&" ".repeat(depth));
            out.push('}');
        }
    }
}

/// Escape a string as a JSON literal with `ensure_ascii=True` (CPython default): `"`, `\`, and control
/// chars get their short/`\u00xx` escape, printable ASCII (`0x20`-`0x7e`) is verbatim, and every char
/// `>= 0x7f` is `\uXXXX`-escaped (a surrogate pair above the BMP), matching CPython's `c_encode_basestring_ascii`.
fn esc_ascii(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) <= 0x7e => out.push(c),
            c => {
                let cp = c as u32;
                if cp <= 0xffff {
                    out.push_str(&format!("\\u{cp:04x}"));
                } else {
                    let v = cp - 0x10000;
                    out.push_str(&format!(
                        "\\u{:04x}\\u{:04x}",
                        0xd800 + (v >> 10),
                        0xdc00 + (v & 0x3ff)
                    ));
                }
            }
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ADVERSARIAL_XSD: &str = r#"
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:complexType name="Envelope">
    <xs:sequence>
      <xs:element name="prefix" type="xs:string"/>
      <xs:choice>
        <xs:sequence>
          <xs:element name="alpha" type="xs:string"/>
          <xs:element name="value" type="xs:int" minOccurs="1" maxOccurs="2"/>
        </xs:sequence>
        <xs:sequence>
          <xs:element name="beta" type="xs:string"/>
          <xs:element name="value" type="xs:boolean" minOccurs="0"/>
        </xs:sequence>
      </xs:choice>
    </xs:sequence>
  </xs:complexType>
  <xs:element name="first" type="Envelope"/>
  <xs:element name="second" type="Envelope"/>
</xs:schema>
"#;

    /// The frozen, sha-pinned schema as UTF-8: the same bytes the committed artifacts derive from.
    fn frozen_xsd() -> &'static str {
        std::str::from_utf8(crate::schema::HCDF_XSD_1_0).expect("embedded hcdf.xsd is UTF-8")
    }

    fn frozen_stream_profile_xsd() -> &'static str {
        std::str::from_utf8(crate::schema::HCDF_STREAM_PROFILE_XSD_1_0)
            .expect("embedded hcdf-stream-profile.xsd is UTF-8")
    }

    /// Drift gate: regenerating `enums.rs` from the frozen schema must byte-match the committed file, so
    /// `cargo test` alone catches XSD/model drift (no Python). The model directory is this crate's own
    /// `src/model`, exactly what `hcdf regen enums <xsd> src/model/enums.rs` would pass.
    #[test]
    fn enums_rs_regen_is_a_no_op() {
        let model_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src/model"));
        let generated = generate_enums_rs(frozen_xsd(), &model_dir).expect("generate enums.rs");
        let committed = include_str!("model/enums.rs");
        assert_eq!(
            generated, committed,
            "src/model/enums.rs is out of sync with hcdf.xsd"
        );
    }

    #[test]
    fn externally_authored_connectivity_enums_are_not_duplicated() {
        let model_dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src/model"));
        let generated = generate_enums_rs(frozen_xsd(), &model_dir).expect("generate enums.rs");
        for class in EXTERNALLY_AUTHORED_ENUMS {
            assert!(
                !generated.contains(&format!("pub enum {class}")),
                "{class} must remain authored outside model/enums.rs"
            );
        }
    }

    /// Drift gate for `hcdf.schema.json` (the repo-root committed artifact): regenerating from the frozen
    /// schema must byte-match it. Read at test time via a manifest-relative path (repo-only, not packaged).
    #[test]
    fn json_schema_regen_is_a_no_op() {
        let generated = generate_json_schema(frozen_xsd()).expect("generate hcdf.schema.json");
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../hcdf.schema.json");
        let committed = std::fs::read_to_string(path).expect("read committed hcdf.schema.json");
        assert_eq!(
            generated, committed,
            "hcdf.schema.json is out of sync with hcdf.xsd"
        );
    }

    /// Drift gate for the checked-in core and stream-profile specification pages.
    #[test]
    fn spec_html_copies_regenerate_to_the_same_bytes() {
        for (schema, schema_name, artifacts) in [
            (
                frozen_xsd(),
                "hcdf.xsd",
                ["spec.html", "website/spec/core.html"],
            ),
            (
                frozen_stream_profile_xsd(),
                "hcdf-stream-profile.xsd",
                [
                    "stream-profile-spec.html",
                    "website/spec/stream-profile.html",
                ],
            ),
        ] {
            let generated =
                generate_spec_html(schema, schema_name).expect("generate specification HTML");
            for artifact in artifacts {
                let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(artifact);
                let committed = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                assert_eq!(
                    generated, committed,
                    "{artifact} is out of sync with {schema_name}"
                );
            }
        }
    }

    /// Drift gate for `hcdf.completions.json` (the repo-root editor completion model, PACKAGED into
    /// the Python wheel and the CMake install): regenerating from the frozen schema must byte-match
    /// the committed file. This was the ONE generated artifact with no drift gate in any CI era; it
    /// shipped stale (missing the comms/CAN-FD/submesh-era vocabulary) until this gate pinned it.
    #[test]
    fn completions_json_regen_is_a_no_op() {
        let generated =
            generate_completions_json(frozen_xsd()).expect("generate hcdf.completions.json");
        let document: serde_json::Value =
            serde_json::from_str(&generated).expect("parse generated completions");
        assert_eq!(document["format"], "hcdf-contextual-completions-v2");
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../hcdf.completions.json");
        let committed =
            std::fs::read_to_string(path).expect("read committed hcdf.completions.json");
        assert_eq!(
            generated, committed,
            "hcdf.completions.json is out of sync with hcdf.xsd"
        );
    }

    #[test]
    fn contextual_completions_keep_repeated_names_in_distinct_particles() {
        let generated =
            generate_completions_json(ADVERSARIAL_XSD).expect("generate contextual completions");
        let document: serde_json::Value =
            serde_json::from_str(&generated).expect("parse generated JSON");
        let roots = document["roots"].as_object().expect("roots object");
        assert!(roots.contains_key("first"));
        assert!(roots.contains_key("second"));

        let particles = document["particles"].as_object().expect("particles object");
        let first_value = &particles["named:Envelope/content/1/0/1"];
        let second_value = &particles["named:Envelope/content/1/1/1"];
        assert_eq!(first_value["name"], "value");
        assert_eq!(first_value["maxOccurs"], 2);
        assert_eq!(first_value["type"], "builtin:xs:int");
        assert_eq!(second_value["name"], "value");
        assert_eq!(second_value["type"], "builtin:xs:boolean");
        assert_eq!(particles["named:Envelope/content/1"]["kind"], "choice");
        assert_eq!(particles["named:Envelope/content/1/0"]["kind"], "sequence");
    }

    #[test]
    fn canonical_completions_distinguish_structural_and_connectivity_boxes() {
        let generated =
            generate_completions_json(frozen_xsd()).expect("generate contextual completions");
        let document: serde_json::Value =
            serde_json::from_str(&generated).expect("parse generated JSON");
        let particles = &document["particles"];

        assert_eq!(
            particles["named:visual_geometry/content/0"]["type"],
            "named:box"
        );
        assert_eq!(particles["named:box/content/0"]["name"], "size");

        assert_eq!(
            particles["named:connectivity_connector/content/7"]["type"],
            "named:connectivity_representation"
        );
        assert_eq!(
            particles["named:connectivity_representation/content/0"]["type"],
            "named:connectivity_primitive_box"
        );
        assert_eq!(
            particles["named:connectivity_primitive_box/content/0"]["name"],
            "placement"
        );
    }

    #[test]
    fn exact_json_keeps_nested_choice_arms_and_element_cardinality() {
        let generated =
            generate_json_schema_exact(ADVERSARIAL_XSD).expect("generate exact JSON Schema");
        let document: serde_json::Value =
            serde_json::from_str(&generated).expect("parse generated JSON Schema");
        assert_eq!(document["oneOf"].as_array().map(Vec::len), Some(2));
        let choice = &document["$defs"]["Envelope"]["allOf"][0]["allOf"][1]["oneOf"];
        assert_eq!(choice.as_array().map(Vec::len), Some(2));
        let first_arm_sequence = &choice[0]["allOf"][0]["allOf"];
        assert_eq!(first_arm_sequence[0]["required"][0], "alpha");
        assert_eq!(first_arm_sequence[1]["required"][0], "value");
        assert_eq!(
            first_arm_sequence[1]["properties"]["value"]["type"],
            "array"
        );
        assert_eq!(first_arm_sequence[1]["properties"]["value"]["maxItems"], 2);
    }

    #[test]
    fn spec_auto_discovery_includes_every_document_root() {
        let generated = generate_spec_html(ADVERSARIAL_XSD, "hcdf-ext-fixture.xsd")
            .expect("generate spec HTML");
        assert!(generated.contains("id=\"tab-first\""));
        assert!(generated.contains("id=\"tab-second\""));
    }

    #[test]
    fn schema_index_rejects_duplicate_named_types_and_roots() {
        let duplicate_type = r#"
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:simpleType name="Thing"><xs:restriction base="xs:string"/></xs:simpleType>
  <xs:complexType name="Thing"/>
  <xs:element name="root" type="Thing"/>
</xs:schema>"#;
        let error = generate_completions_json(duplicate_type).unwrap_err();
        assert!(error.contains("duplicate named type"), "{error}");

        let duplicate_root = r#"
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="root" type="xs:string"/>
  <xs:element name="root" type="xs:int"/>
</xs:schema>"#;
        let error = generate_completions_json(duplicate_root).unwrap_err();
        assert!(error.contains("duplicate global document root"), "{error}");
    }

    #[test]
    fn exact_projection_rejects_unrepresentable_or_ambiguous_particles() {
        let repeated_group = r#"
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:complexType name="Root"><xs:sequence maxOccurs="2">
    <xs:element name="value" type="xs:string"/>
  </xs:sequence></xs:complexType>
  <xs:element name="root" type="Root"/>
</xs:schema>"#;
        let error = generate_json_schema_exact(repeated_group).unwrap_err();
        assert!(
            error.contains("cannot represent repeated sequence groups"),
            "{error}"
        );

        let ambiguous_choice = r#"
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:complexType name="Root"><xs:choice>
    <xs:element name="value" type="xs:string"/>
    <xs:element name="value" type="xs:int"/>
  </xs:choice></xs:complexType>
  <xs:element name="root" type="Root"/>
</xs:schema>"#;
        let error = generate_json_schema_exact(ambiguous_choice).unwrap_err();
        assert!(error.contains("contextually ambiguous arms"), "{error}");
    }
}
