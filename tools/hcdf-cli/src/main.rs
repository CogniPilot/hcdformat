//! HCDF CLI — validate, convert, and inspect Hardware Configuration Descriptive Format files.
//!
//! Generic tree-walking XML<->JSON converter (no hardcoded structs).
//! Works with any valid HCDF XML without knowing the schema types.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Reader;
use serde_json::{Map, Number, Value};

// ═══════════════════════════════════════════════════════════════════════════
// CLI definition
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Parser)]
#[command(
    name = "hcdf",
    version,
    about = "HCDF CLI — validate, convert, and inspect Hardware Configuration Descriptive Format files"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Convert between XML (.hcdf/.xml) and JSON formats
    Convert {
        /// Input file (.hcdf, .xml, or .json)
        #[arg(short, long)]
        input: PathBuf,
        /// Output file (.hcdf, .xml, or .json)
        #[arg(short, long)]
        output: PathBuf,
        /// Path to HCDF XSD schema (auto-detected if not specified)
        #[arg(long)]
        xsd: Option<PathBuf>,
        /// Force output format (xml or json)
        #[arg(long)]
        format: Option<String>,
    },
    /// Validate an HCDF file against the XSD schema
    Validate {
        /// HCDF file to validate
        file: PathBuf,
        /// Path to HCDF XSD schema (auto-detected if not specified)
        #[arg(long)]
        xsd: Option<PathBuf>,
    },
    /// Print summary info about an HCDF file
    Info {
        /// HCDF file to inspect
        file: PathBuf,
    },
    /// Print schema information from the XSD
    Schema {
        /// Path to HCDF XSD schema (auto-detected if not specified)
        #[arg(long)]
        xsd: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Convert {
            input,
            output,
            xsd,
            format,
        } => cmd_convert(&input, &output, xsd.as_deref(), format.as_deref()),
        Commands::Validate { file, xsd } => cmd_validate(&file, xsd.as_deref()),
        Commands::Info { file } => cmd_info(&file),
        Commands::Schema { xsd } => cmd_schema(xsd.as_deref()),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Format detection
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Xml,
    Json,
}

fn detect_format_from_path(path: &Path) -> Option<Format> {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("xml") | Some("hcdf") => Some(Format::Xml),
        Some("json") => Some(Format::Json),
        _ => None,
    }
}

fn detect_format_from_content(content: &str) -> Format {
    let trimmed = content.trim_start();
    if trimmed.starts_with('<') {
        Format::Xml
    } else {
        Format::Json
    }
}

fn parse_format_flag(s: &str) -> Result<Format> {
    match s.to_lowercase().as_str() {
        "xml" | "hcdf" => Ok(Format::Xml),
        "json" => Ok(Format::Json),
        _ => bail!("Unknown format '{}'. Use 'xml' or 'json'.", s),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// XML -> JSON (generic tree-walking)
// ═══════════════════════════════════════════════════════════════════════════

/// Try to interpret a string as a numeric/boolean value.
/// Matching conventions from Python convert.py.
fn try_numeric(value: &str) -> Value {
    // Don't convert space-separated vectors (e.g. "0.18 0.18 0.03")
    if value.contains(' ') {
        return Value::String(value.to_string());
    }
    // Don't convert hex strings
    if value.starts_with("0x") || value.starts_with("0X") {
        return Value::String(value.to_string());
    }
    // Don't convert version strings like "1.0.3" (multiple dots)
    if value.chars().filter(|&c| c == '.').count() > 1 {
        return Value::String(value.to_string());
    }
    // Booleans
    if value == "true" {
        return Value::Bool(true);
    }
    if value == "false" {
        return Value::Bool(false);
    }
    // Integer
    if let Ok(i) = value.parse::<i64>() {
        // Avoid converting strings with leading zeros like "007"
        if i.to_string() == value {
            return Value::Number(Number::from(i));
        }
    }
    // Float
    if !value.starts_with('.') && !value.to_lowercase().contains('e') {
        if let Ok(f) = value.parse::<f64>() {
            if let Some(n) = Number::from_f64(f) {
                return Value::Number(n);
            }
        }
    }
    Value::String(value.to_string())
}

/// Extract attributes from a BytesStart event into a Vec of (name, json_value).
fn extract_attributes(start: &BytesStart) -> Result<Vec<(String, Value)>> {
    let mut attrs = Vec::new();
    for attr_result in start.attributes() {
        let attr = attr_result?;
        let key = String::from_utf8(attr.key.local_name().as_ref().to_vec())?;
        let val_str = String::from_utf8(attr.value.to_vec())?;
        attrs.push((key, try_numeric(&val_str)));
    }
    Ok(attrs)
}

/// Parse an XML element and its children from a quick_xml Reader into a JSON Value.
/// `start` is the opening BytesStart event. Returns (local_name, value).
fn parse_element(reader: &mut Reader<&[u8]>, start: &BytesStart) -> Result<(String, Value)> {
    let name = String::from_utf8(start.local_name().as_ref().to_vec())?;
    let attrs = extract_attributes(start)?;

    // Collect child elements and text
    let mut children: Vec<(String, Value)> = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let (child_name, child_value) = parse_element(reader, e)?;
                children.push((child_name, child_value));
            }
            Ok(Event::Empty(ref e)) => {
                let child_name = String::from_utf8(e.local_name().as_ref().to_vec())?;
                let child_attrs = extract_attributes(e)?;
                if child_attrs.is_empty() {
                    children.push((child_name, Value::Object(Map::new())));
                } else {
                    let mut obj = Map::new();
                    for (k, v) in child_attrs {
                        obj.insert(k, v);
                    }
                    children.push((child_name, Value::Object(obj)));
                }
            }
            Ok(Event::Text(ref e)) => {
                let t = e.unescape()?.to_string();
                let trimmed = t.trim().to_string();
                if !trimmed.is_empty() {
                    text_parts.push(trimmed);
                }
            }
            Ok(Event::CData(ref e)) => {
                let t = String::from_utf8_lossy(e.as_ref()).to_string();
                let trimmed = t.trim().to_string();
                if !trimmed.is_empty() {
                    text_parts.push(trimmed);
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Comment(_)) | Ok(Event::PI(_)) | Ok(Event::Decl(_)) => {}
            Ok(Event::DocType(_)) => {}
            Ok(Event::Eof) => bail!("Unexpected EOF inside element <{}>", name),
            Err(e) => bail!("XML parse error in element <{}>: {}", name, e),
        }
        buf.clear();
    }

    let has_attrs = !attrs.is_empty();
    let has_children = !children.is_empty();
    let text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join(" "))
    };

    // Simple text element: no children, no attributes -> scalar
    if !has_children && !has_attrs {
        if let Some(t) = text {
            return Ok((name, try_numeric(&t)));
        }
        return Ok((name, Value::Null));
    }

    // Build object
    let mut obj = Map::new();

    // Attributes -> direct keys
    for (k, v) in attrs {
        obj.insert(k, v);
    }

    // Mixed content: text + children or text + attributes
    if let Some(t) = text {
        obj.insert("$text".to_string(), try_numeric(&t));
    }

    // Count children by tag to decide array vs object
    let mut tag_counts: HashMap<String, usize> = HashMap::new();
    for (child_name, _) in &children {
        *tag_counts.entry(child_name.clone()).or_insert(0) += 1;
    }

    // Track which tags have been started as arrays
    let mut array_tags: HashMap<String, Vec<Value>> = HashMap::new();
    let mut array_inserted: HashSet<String> = HashSet::new();

    for (child_name, child_value) in children {
        let count = tag_counts.get(&child_name).copied().unwrap_or(0);
        if count > 1 {
            // Multiple same-named children -> array
            let arr = array_tags.entry(child_name.clone()).or_default();
            arr.push(child_value);
            if !array_inserted.contains(&child_name) {
                // Insert placeholder for ordering (serde_json preserves insertion order)
                array_inserted.insert(child_name.clone());
                obj.insert(child_name, Value::Null);
            }
        } else {
            obj.insert(child_name, child_value);
        }
    }

    // Replace placeholders with actual arrays
    for (tag, arr) in array_tags {
        obj.insert(tag, Value::Array(arr));
    }

    Ok((name, Value::Object(obj)))
}

fn xml_to_json(xml: &str) -> Result<Value> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let (name, value) = parse_element(&mut reader, e)?;
                let mut root = Map::new();
                root.insert(name, value);
                return Ok(Value::Object(root));
            }
            Ok(Event::Eof) => break,
            Ok(Event::Decl(_)) | Ok(Event::Comment(_)) | Ok(Event::PI(_)) => {}
            Ok(Event::DocType(_)) => {}
            Err(e) => bail!("XML parse error: {}", e),
            _ => {}
        }
        buf.clear();
    }
    bail!("No root element found in XML")
}

// ═══════════════════════════════════════════════════════════════════════════
// XSD type information (for context-aware JSON -> XML)
// ═══════════════════════════════════════════════════════════════════════════

/// Helper to get an attribute value from a BytesStart event.
fn get_xsd_attr(e: &BytesStart, name: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref() == name.as_bytes() {
            return String::from_utf8(a.value.to_vec()).ok();
        }
    }
    None
}

/// Rich XSD type information for context-aware JSON->XML conversion.
/// Instead of a flat element_name->attributes map, this carries full type
/// context so that the same element name in different parent types can
/// resolve to different XSD types with different attribute sets.
struct XsdInfo {
    /// type_name -> set of attribute names (including inherited via xs:extension)
    type_attrs: HashMap<String, HashSet<String>>,
    /// type_name -> map of child element name -> child element type name
    type_children: HashMap<String, HashMap<String, String>>,
    /// type_name -> base type name (from xs:complexContent/xs:extension)
    type_bases: HashMap<String, String>,
    /// global element_name -> type name (for root and uncontextualized lookups)
    elem_type_map: HashMap<String, String>,
    /// type_name -> bool (mixed content: has both text and attributes)
    type_mixed: HashMap<String, bool>,
}

impl XsdInfo {
    /// Look up the type of a child element within a parent type context.
    fn child_type(&self, parent_type: &str, child_tag: &str) -> Option<String> {
        // Check this type's children
        if let Some(children) = self.type_children.get(parent_type) {
            if let Some(ct) = children.get(child_tag) {
                return Some(ct.clone());
            }
        }
        // Walk base chain
        if let Some(base) = self.type_bases.get(parent_type) {
            return self.child_type(base, child_tag);
        }
        None
    }

    /// Get the resolved attribute set for a type (including inherited attrs).
    fn attrs_for_type(&self, type_name: &str) -> HashSet<String> {
        let mut attrs = self.type_attrs.get(type_name).cloned().unwrap_or_default();
        if let Some(base) = self.type_bases.get(type_name) {
            attrs.extend(self.attrs_for_type(base));
        }
        attrs
    }

    /// Check if a type is attribute-only (has attributes, no children, no mixed text).
    fn is_attr_only(&self, type_name: &str) -> bool {
        let attrs = self.attrs_for_type(type_name);
        if attrs.is_empty() {
            return false;
        }
        // Check that the type has no child elements
        let has_children = self
            .type_children
            .get(type_name)
            .map(|c| !c.is_empty())
            .unwrap_or(false);
        let is_mixed = self.type_mixed.get(type_name).copied().unwrap_or(false);
        !has_children && !is_mixed
    }

    /// Check if a type has mixed="true" content.
    #[allow(dead_code)]
    fn is_mixed(&self, type_name: &str) -> bool {
        if self.type_mixed.get(type_name).copied().unwrap_or(false) {
            return true;
        }
        // Check base chain
        if let Some(base) = self.type_bases.get(type_name) {
            return self.is_mixed(base);
        }
        false
    }
}

/// Parse an XSD file and build an XsdInfo with full type context.
fn build_xsd_info(xsd_content: &str) -> XsdInfo {
    let mut type_attrs: HashMap<String, HashSet<String>> = HashMap::new();
    let mut type_children: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut type_bases: HashMap<String, String> = HashMap::new();
    let mut elem_type_map: HashMap<String, String> = HashMap::new();
    let mut type_mixed: HashMap<String, bool> = HashMap::new();

    // ── Pass 1: Walk XSD collecting types, children, attributes ──

    let mut reader = Reader::from_str(xsd_content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    // Context stack tracks where we are in the XSD.
    // Each entry is (local_tag_name, optional_type_context)
    // type_context = the complexType name we're currently defining children/attrs for
    #[derive(Clone, Debug)]
    enum Ctx {
        Root,
        NamedComplexType(String),         // complexType name="X"
        InlineComplexType(String),        // anonymous complexType for element "hcdf" etc.; use synthetic name
        Extension(String),                // extension inside type (owning_type)
        Element(String),                  // element name
        Other,
    }

    fn owning_type(stack: &[Ctx]) -> Option<String> {
        for ctx in stack.iter().rev() {
            match ctx {
                Ctx::NamedComplexType(t) => return Some(t.clone()),
                Ctx::InlineComplexType(t) => return Some(t.clone()),
                Ctx::Extension(t) => return Some(t.clone()),
                _ => {}
            }
        }
        None
    }

    fn enclosing_element_name(stack: &[Ctx]) -> Option<String> {
        for ctx in stack.iter().rev() {
            if let Ctx::Element(name) = ctx {
                return Some(name.clone());
            }
        }
        None
    }

    let mut stack: Vec<Ctx> = vec![Ctx::Root];

    // Helper to handle an element declaration (Start or Empty)
    fn handle_element_decl(
        e: &BytesStart,
        stack: &[Ctx],
        type_children: &mut HashMap<String, HashMap<String, String>>,
        elem_type_map: &mut HashMap<String, String>,
        is_global: bool,
    ) {
        let name = match get_xsd_attr(e, "name") {
            Some(n) => n,
            None => return,
        };
        let typ = get_xsd_attr(e, "type").unwrap_or_default();

        // If at schema level (depth 1), record as global element
        if is_global && !name.is_empty() && !typ.is_empty() {
            elem_type_map.insert(name.clone(), typ.clone());
        }

        // Record as child of owning type
        if !typ.is_empty() {
            if let Some(owner) = owning_type(stack) {
                type_children
                    .entry(owner)
                    .or_default()
                    .insert(name.clone(), typ);
            }
        }
    }

    fn handle_attribute_decl(
        e: &BytesStart,
        stack: &[Ctx],
        type_attrs: &mut HashMap<String, HashSet<String>>,
    ) {
        if let Some(attr_name) = get_xsd_attr(e, "name") {
            if let Some(owner) = owning_type(stack) {
                type_attrs.entry(owner).or_default().insert(attr_name);
            }
        }
    }

    // Track schema-level depth to identify global elements
    let mut depth: usize = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                depth += 1;
                let local = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                match local.as_str() {
                    "complexType" => {
                        if let Some(type_name) = get_xsd_attr(e, "name") {
                            let is_mixed =
                                get_xsd_attr(e, "mixed").as_deref() == Some("true");
                            type_attrs.entry(type_name.clone()).or_default();
                            type_children.entry(type_name.clone()).or_default();
                            if is_mixed {
                                type_mixed.insert(type_name.clone(), true);
                            }
                            stack.push(Ctx::NamedComplexType(type_name));
                        } else {
                            // Anonymous inline complexType — synthesize a name from
                            // the enclosing element, e.g. element "hcdf" gets type "__hcdf__"
                            let is_mixed =
                                get_xsd_attr(e, "mixed").as_deref() == Some("true");
                            let synthetic = if let Some(elt) = enclosing_element_name(&stack)
                            {
                                format!("__{}__", elt)
                            } else {
                                "__anon__".to_string()
                            };
                            type_attrs.entry(synthetic.clone()).or_default();
                            type_children.entry(synthetic.clone()).or_default();
                            if is_mixed {
                                type_mixed.insert(synthetic.clone(), true);
                            }
                            // Also register this synthetic type for the enclosing element
                            if let Some(elt) = enclosing_element_name(&stack) {
                                elem_type_map.insert(elt, synthetic.clone());
                            }
                            stack.push(Ctx::InlineComplexType(synthetic));
                        }
                    }
                    "extension" => {
                        if let Some(base) = get_xsd_attr(e, "base") {
                            let owner = owning_type(&stack).unwrap_or_default();
                            if !owner.is_empty() {
                                type_bases.insert(owner.clone(), base.clone());
                            }
                            stack.push(Ctx::Extension(owner));
                        } else {
                            stack.push(Ctx::Other);
                        }
                    }
                    "element" => {
                        let is_global = depth == 2; // xs:schema > xs:element
                        handle_element_decl(
                            e,
                            &stack,
                            &mut type_children,
                            &mut elem_type_map,
                            is_global,
                        );
                        let name = get_xsd_attr(e, "name").unwrap_or_default();
                        stack.push(Ctx::Element(name));
                    }
                    "attribute" => {
                        handle_attribute_decl(e, &stack, &mut type_attrs);
                        stack.push(Ctx::Other);
                    }
                    _ => {
                        stack.push(Ctx::Other);
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                match local.as_str() {
                    "element" => {
                        let is_global = depth == 1; // depth hasn't been incremented for Empty
                        handle_element_decl(
                            e,
                            &stack,
                            &mut type_children,
                            &mut elem_type_map,
                            is_global,
                        );
                    }
                    "attribute" => {
                        handle_attribute_decl(e, &stack, &mut type_attrs);
                    }
                    "extension" => {
                        // Self-closing extension (no children)
                        if let Some(base) = get_xsd_attr(e, "base") {
                            let owner = owning_type(&stack).unwrap_or_default();
                            if !owner.is_empty() {
                                type_bases.insert(owner, base);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(_)) => {
                if depth > 0 {
                    depth -= 1;
                }
                stack.pop();
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    // ── Pass 2: Resolve inheritance for children ──
    // Merge base type children into derived types (iteratively until stable).
    let all_types: Vec<String> = type_children.keys().cloned().collect();
    for _ in 0..10 {
        // max 10 inheritance depth
        let mut changed = false;
        for tn in &all_types {
            if let Some(base_name) = type_bases.get(tn).cloned() {
                let base_children = type_children
                    .get(&base_name)
                    .cloned()
                    .unwrap_or_default();
                let entry = type_children.entry(tn.clone()).or_default();
                for (k, v) in base_children {
                    if !entry.contains_key(&k) {
                        entry.insert(k, v);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    XsdInfo {
        type_attrs,
        type_children,
        type_bases,
        elem_type_map,
        type_mixed,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// JSON -> XML
// ═══════════════════════════════════════════════════════════════════════════

/// Known attribute names as fallback heuristic when XSD is not available.
const HEURISTIC_ATTR_NAMES: &[&str] = &[
    "name",
    "type",
    "role",
    "version",
    "unit",
    "id",
    "ref",
    "domain",
    "iface",
    "chip",
    "sha",
    "uri",
    "default",
    "active",
    "policy",
    "cipher",
    "topology",
    "mode",
    "standard",
    "comp",
    "joint",
    "motor",
    "sensor",
    "device",
    "port",
    "ports",
    "chains",
    "schedule",
    "pcp",
    "min",
    "max",
    "nominal",
    "continuous",
    "peak",
    "supported",
    "struct-type",
    "relative-to",
    "update-rate",
    "x",
    "y",
    "z",
    "color",
    "qos",
    "filename",
    "encoder",
    "hall",
    "thermistor",
    "principle",
    "gm-capable",
    "hw-timestamping",
    "priority1",
    "priority2",
    "log-sync-interval",
    "neighbor-prop-delay-thresh-ns",
    "traffic-classes",
    "hw-queues",
    "max-gcl-entries",
    "min-fragment-size",
    "key-agreement",
    "key-ref",
    "tc",
    "preemption",
    "tc-mask",
    "duration-us",
    "cycle-time-us",
    "count",
    "comp1",
    "comp2",
    "reason",
    "position",
    "terminator",
    "ingress",
    "static",
    "dynamic",
    "shape",
    "lower",
    "upper",
    "effort",
    "velocity",
    "damping",
    "friction",
    "spring_stiffness",
    "spring_reference",
    "xyz",
    "rpy",
    "rgba",
];

struct JsonToXml {
    xsd: Option<XsdInfo>,
}

impl JsonToXml {
    fn new(xsd_content: Option<&str>) -> Self {
        if let Some(xsd) = xsd_content {
            let xsd_info = build_xsd_info(xsd);
            JsonToXml {
                xsd: Some(xsd_info),
            }
        } else {
            JsonToXml { xsd: None }
        }
    }

    /// Resolve the XSD type for an element given parent type context.
    fn resolve_type(&self, tag: &str, parent_type: Option<&str>) -> Option<String> {
        let xsd = self.xsd.as_ref()?;
        // First try parent context
        if let Some(pt) = parent_type {
            if let Some(ct) = xsd.child_type(pt, tag) {
                return Some(ct);
            }
        }
        // Fall back to global element map
        xsd.elem_type_map.get(tag).cloned()
    }

    fn is_attribute_xsd(&self, my_type: Option<&str>, key: &str) -> bool {
        if let (Some(xsd), Some(mt)) = (self.xsd.as_ref(), my_type) {
            let attrs = xsd.attrs_for_type(mt);
            return attrs.contains(key);
        }
        false
    }

    fn is_attribute_heuristic(key: &str) -> bool {
        HEURISTIC_ATTR_NAMES.contains(&key)
    }

    fn value_to_str(value: &Value) -> String {
        match value {
            Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => value.to_string(),
        }
    }

    fn write_element(
        &self,
        writer: &mut quick_xml::Writer<Vec<u8>>,
        tag: &str,
        value: &Value,
        parent_type: Option<&str>,
    ) -> Result<()> {
        // 1. Resolve my type from parent context
        let my_type = self.resolve_type(tag, parent_type);
        let my_type_str = my_type.as_deref();

        // 2. Handle attribute-only types (scalar JSON -> <tag attr="value"/>)
        if let Some(xsd) = &self.xsd {
            if let Some(ref mt) = my_type {
                if xsd.is_attr_only(mt) {
                    if let Value::String(s) = value {
                        let attrs = xsd.attrs_for_type(mt);
                        if let Some(attr_name) = attrs.iter().next() {
                            let mut elem = BytesStart::new(tag);
                            elem.push_attribute((attr_name.as_str(), s.as_str()));
                            writer.write_event(Event::Empty(elem))?;
                            return Ok(());
                        }
                    }
                    // Numeric/bool scalar for attr-only type
                    if value.is_number() || value.is_boolean() {
                        let s = Self::value_to_str(value);
                        let attrs = xsd.attrs_for_type(mt);
                        if let Some(attr_name) = attrs.iter().next() {
                            let mut elem = BytesStart::new(tag);
                            elem.push_attribute((attr_name.as_str(), s.as_str()));
                            writer.write_event(Event::Empty(elem))?;
                            return Ok(());
                        }
                    }
                }
            }
        }

        match value {
            Value::Object(obj) => {
                let has_xsd = self.xsd.is_some();

                let mut xml_attrs: Vec<(&str, String)> = Vec::new();
                let mut xml_children: Vec<(&str, &Value)> = Vec::new();
                let mut text_content: Option<String> = None;

                for (key, val) in obj {
                    if key == "$text" {
                        text_content = Some(Self::value_to_str(val));
                    } else if val.is_array() || val.is_object() {
                        xml_children.push((key.as_str(), val));
                    } else if has_xsd {
                        // Use type-aware attribute detection
                        if self.is_attribute_xsd(my_type_str, key) {
                            xml_attrs.push((key.as_str(), Self::value_to_str(val)));
                        } else {
                            xml_children.push((key.as_str(), val));
                        }
                    } else {
                        // Heuristic fallback when no XSD
                        if Self::is_attribute_heuristic(key) {
                            xml_attrs.push((key.as_str(), Self::value_to_str(val)));
                        } else {
                            xml_children.push((key.as_str(), val));
                        }
                    }
                }

                let has_content = text_content.is_some() || !xml_children.is_empty();

                if !has_content && xml_attrs.is_empty() {
                    // Completely empty element
                    writer.write_event(Event::Empty(BytesStart::new(tag)))?;
                    return Ok(());
                }

                if !has_content {
                    // Self-closing element with only attributes
                    let mut elem = BytesStart::new(tag);
                    for (k, v) in &xml_attrs {
                        elem.push_attribute((*k, v.as_str()));
                    }
                    writer.write_event(Event::Empty(elem))?;
                    return Ok(());
                }

                let mut elem = BytesStart::new(tag);
                for (k, v) in &xml_attrs {
                    elem.push_attribute((*k, v.as_str()));
                }
                writer.write_event(Event::Start(elem))?;

                if let Some(ref text) = text_content {
                    writer.write_event(Event::Text(BytesText::new(text)))?;
                }

                for (child_tag, child_val) in &xml_children {
                    if let Value::Array(arr) = child_val {
                        for item in arr {
                            self.write_element(writer, child_tag, item, my_type_str)?;
                        }
                    } else {
                        self.write_element(writer, child_tag, child_val, my_type_str)?;
                    }
                }

                writer.write_event(Event::End(BytesEnd::new(tag)))?;
            }
            Value::Array(arr) => {
                for item in arr {
                    self.write_element(writer, tag, item, parent_type)?;
                }
            }
            Value::Null => {
                writer.write_event(Event::Empty(BytesStart::new(tag)))?;
            }
            _ => {
                let text = Self::value_to_str(value);
                writer.write_event(Event::Start(BytesStart::new(tag)))?;
                writer.write_event(Event::Text(BytesText::new(&text)))?;
                writer.write_event(Event::End(BytesEnd::new(tag)))?;
            }
        }
        Ok(())
    }
}

fn json_to_xml(json_str: &str, xsd_content: Option<&str>) -> Result<String> {
    let data: Value = serde_json::from_str(json_str).context("Failed to parse JSON")?;

    let obj = data
        .as_object()
        .context("JSON root must be an object with exactly one key (the root element tag)")?;
    if obj.len() != 1 {
        bail!(
            "JSON root must have exactly one key (the root element), found {}",
            obj.len()
        );
    }

    let (root_tag, root_value) = obj.iter().next().unwrap();
    let converter = JsonToXml::new(xsd_content);

    let mut writer = quick_xml::Writer::new_with_indent(Vec::new(), b' ', 2);

    // XML declaration
    writer.write_event(Event::Decl(quick_xml::events::BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        None,
    )))?;

    // Root element type comes from elem_type_map["hcdf"] -> "__hcdf__" (inline type)
    converter.write_element(&mut writer, root_tag, root_value, None)?;

    let xml_bytes = writer.into_inner();
    let xml_string = String::from_utf8(xml_bytes)?;

    Ok(xml_string + "\n")
}

// ═══════════════════════════════════════════════════════════════════════════
// Subcommands
// ═══════════════════════════════════════════════════════════════════════════

fn find_xsd(near: &Path) -> Option<PathBuf> {
    let mut dir = if near.is_file() {
        near.parent().map(|p| p.to_path_buf())
    } else {
        Some(near.to_path_buf())
    };
    for _ in 0..4 {
        if let Some(ref d) = dir {
            let candidate = d.join("hcdf.xsd");
            if candidate.exists() {
                return Some(candidate);
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    None
}

fn cmd_convert(
    input: &Path,
    output: &Path,
    xsd_path: Option<&Path>,
    format_flag: Option<&str>,
) -> Result<()> {
    let content = fs::read_to_string(input)
        .with_context(|| format!("Failed to read input file: {}", input.display()))?;

    let in_fmt =
        detect_format_from_path(input).unwrap_or_else(|| detect_format_from_content(&content));

    let out_fmt = if let Some(f) = format_flag {
        parse_format_flag(f)?
    } else {
        detect_format_from_path(output).unwrap_or(match in_fmt {
            Format::Xml => Format::Json,
            Format::Json => Format::Xml,
        })
    };

    if in_fmt == out_fmt {
        bail!(
            "Input and output are both {:?}. Nothing to convert.",
            in_fmt
        );
    }

    match (in_fmt, out_fmt) {
        (Format::Xml, Format::Json) => {
            let json_value = xml_to_json(&content)?;
            let json_str = serde_json::to_string_pretty(&json_value)?;
            fs::write(output, json_str + "\n")?;
            eprintln!(
                "Converted XML -> JSON: {} -> {}",
                input.display(),
                output.display()
            );
        }
        (Format::Json, Format::Xml) => {
            let xsd_content = if let Some(xsd) = xsd_path {
                Some(
                    fs::read_to_string(xsd)
                        .with_context(|| format!("Failed to read XSD: {}", xsd.display()))?,
                )
            } else if let Some(found) = find_xsd(input) {
                eprintln!("Auto-detected XSD: {}", found.display());
                Some(fs::read_to_string(&found)?)
            } else {
                eprintln!("Warning: No XSD found. Using heuristic for attribute detection.");
                None
            };

            let xml_str = json_to_xml(&content, xsd_content.as_deref())?;
            fs::write(output, xml_str)?;
            eprintln!(
                "Converted JSON -> XML: {} -> {}",
                input.display(),
                output.display()
            );
        }
        _ => unreachable!(),
    }

    Ok(())
}

fn cmd_validate(file: &Path, xsd_path: Option<&Path>) -> Result<()> {
    let xsd = if let Some(p) = xsd_path {
        p.to_path_buf()
    } else if let Some(found) = find_xsd(file) {
        found
    } else {
        bail!("No XSD schema found. Specify with --xsd <path>");
    };

    // Try to find validate.py near the XSD
    let validate_py = xsd.parent().map(|d| d.join("validate.py"));
    if let Some(ref py) = validate_py {
        if py.exists() {
            eprintln!(
                "Running: python3 {} {} {}",
                py.display(),
                xsd.display(),
                file.display()
            );
            let status = Command::new("python3")
                .arg(py)
                .arg(&xsd)
                .arg(file)
                .status()
                .context("Failed to run python3 validate.py")?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            return Ok(());
        }
    }

    // No validate.py found — do basic well-formedness check
    let content = fs::read_to_string(file)?;
    let mut reader = Reader::from_str(&content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut depth: usize = 0;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                if depth == 0 {
                    eprintln!("XML structure error: unexpected closing tag");
                    std::process::exit(1);
                }
                depth -= 1;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                eprintln!(
                    "XML PARSE ERROR at position {}: {}",
                    reader.error_position(),
                    e
                );
                std::process::exit(1);
            }
            _ => {}
        }
        buf.clear();
    }
    if depth != 0 {
        eprintln!(
            "XML structure error: unclosed elements (depth={})",
            depth
        );
        std::process::exit(1);
    }

    eprintln!("XML well-formed: {}", file.display());
    eprintln!(
        "Note: Full XSD validation requires python3 with lxml. \
         Run: python3 validate.py {} {}",
        xsd.display(),
        file.display()
    );
    Ok(())
}

fn cmd_info(file: &Path) -> Result<()> {
    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read: {}", file.display()))?;

    let fmt =
        detect_format_from_path(file).unwrap_or_else(|| detect_format_from_content(&content));

    // Convert to JSON Value for uniform handling
    let json_val = match fmt {
        Format::Xml => xml_to_json(&content)?,
        Format::Json => serde_json::from_str(&content)?,
    };

    let root_obj = json_val
        .as_object()
        .and_then(|m| m.values().next())
        .and_then(|v| v.as_object());

    let root_obj = match root_obj {
        Some(obj) => obj,
        None => {
            bail!("Cannot parse HCDF structure from {}", file.display());
        }
    };

    // Extract version
    let version = root_obj
        .get("version")
        .map(|v| match v {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => v.to_string(),
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("File: {}", file.display());
    println!("Version: {}", version);
    println!();

    let count_items = |key: &str| -> usize {
        match root_obj.get(key) {
            Some(Value::Array(arr)) => arr.len(),
            Some(Value::Object(_)) => 1,
            _ => 0,
        }
    };

    let comps = count_items("comp");
    let joints = count_items("joint");
    let networks = count_items("network");
    let transmissions = count_items("transmission");
    let materials = count_items("material");
    let groups = count_items("group");
    let states = count_items("state");
    let includes = count_items("include");
    let extensions = count_items("extension");

    println!("Components:    {}", comps);
    println!("Joints:        {}", joints);
    println!("Networks:      {}", networks);
    println!("Transmissions: {}", transmissions);
    println!("Materials:     {}", materials);
    println!("Groups:        {}", groups);
    println!("States:        {}", states);
    println!("Includes:      {}", includes);

    // Count sensors, motors, ports, antennas inside components
    let mut sensor_count = 0usize;
    let mut motor_count = 0usize;
    let mut port_count = 0usize;
    let mut antenna_count = 0usize;

    let count_in_comp = |comp: &Value,
                         sensors: &mut usize,
                         motors: &mut usize,
                         ports: &mut usize,
                         antennas: &mut usize| {
        if let Some(obj) = comp.as_object() {
            for (key, val) in obj {
                let n = match val {
                    Value::Array(arr) => arr.len(),
                    Value::Object(_) => 1,
                    _ => 0,
                };
                match key.as_str() {
                    "sensor" => *sensors += n,
                    "motor" => *motors += n,
                    "port" => *ports += n,
                    "antenna" => *antennas += n,
                    _ => {}
                }
            }
        }
    };

    match root_obj.get("comp") {
        Some(Value::Array(arr)) => {
            for comp in arr {
                count_in_comp(
                    comp,
                    &mut sensor_count,
                    &mut motor_count,
                    &mut port_count,
                    &mut antenna_count,
                );
            }
        }
        Some(comp @ Value::Object(_)) => {
            count_in_comp(
                comp,
                &mut sensor_count,
                &mut motor_count,
                &mut port_count,
                &mut antenna_count,
            );
        }
        _ => {}
    }

    println!("Sensors:       {}", sensor_count);
    println!("Motors:        {}", motor_count);
    println!("Ports:         {}", port_count);
    println!("Antennas:      {}", antenna_count);

    if extensions > 0 {
        println!("Extensions:    {}", extensions);
    }

    // List component names
    println!();
    println!("Components:");
    let list_comp = |val: &Value| {
        if let Some(obj) = val.as_object() {
            let name = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("(unnamed)");
            let role = obj
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("(no role)");
            println!("  - {} [{}]", name, role);
        }
    };
    match root_obj.get("comp") {
        Some(Value::Array(arr)) => {
            for comp in arr {
                list_comp(comp);
            }
        }
        Some(comp @ Value::Object(_)) => list_comp(comp),
        _ => println!("  (none)"),
    }

    // List network names
    if networks > 0 {
        println!();
        println!("Networks:");
        let list_net = |val: &Value| {
            if let Some(obj) = val.as_object() {
                let name = obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unnamed)");
                println!("  - {}", name);
            }
        };
        match root_obj.get("network") {
            Some(Value::Array(arr)) => {
                for net in arr {
                    list_net(net);
                }
            }
            Some(net @ Value::Object(_)) => list_net(net),
            _ => {}
        }
    }

    Ok(())
}

fn cmd_schema(xsd_path: Option<&Path>) -> Result<()> {
    let xsd = if let Some(p) = xsd_path {
        p.to_path_buf()
    } else if let Some(found) = find_xsd(Path::new(".")) {
        found
    } else {
        bail!("No XSD schema found. Specify with --xsd <path>");
    };

    let content = fs::read_to_string(&xsd)
        .with_context(|| format!("Failed to read XSD: {}", xsd.display()))?;

    println!("Schema: {}", xsd.display());
    println!();

    let mut reader = Reader::from_str(&content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut simple_types: Vec<String> = Vec::new();
    let mut complex_types: Vec<String> = Vec::new();
    let mut root_elements: Vec<String> = Vec::new();
    let mut enum_count = 0usize;
    let mut attr_count = 0usize;
    let mut depth = 0usize;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).to_string();

                match local.as_str() {
                    "simpleType" => {
                        if let Some(name) = get_xsd_attr(e, "name") {
                            simple_types.push(name);
                        }
                    }
                    "complexType" => {
                        if let Some(name) = get_xsd_attr(e, "name") {
                            complex_types.push(name);
                        }
                    }
                    "element" => {
                        if depth <= 1 {
                            if let Some(name) = get_xsd_attr(e, "name") {
                                root_elements.push(name);
                            }
                        }
                    }
                    "enumeration" => {
                        enum_count += 1;
                    }
                    "attribute" => {
                        attr_count += 1;
                    }
                    _ => {}
                }

                depth += 1;
            }
            Ok(Event::Empty(ref e)) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).to_string();

                match local.as_str() {
                    "simpleType" => {
                        if let Some(name) = get_xsd_attr(e, "name") {
                            simple_types.push(name);
                        }
                    }
                    "complexType" => {
                        if let Some(name) = get_xsd_attr(e, "name") {
                            complex_types.push(name);
                        }
                    }
                    "element" => {
                        if depth <= 1 {
                            if let Some(name) = get_xsd_attr(e, "name") {
                                root_elements.push(name);
                            }
                        }
                    }
                    "enumeration" => {
                        enum_count += 1;
                    }
                    "attribute" => {
                        attr_count += 1;
                    }
                    _ => {}
                }
                // Empty events don't change depth
            }
            Ok(Event::End(_)) => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    println!("Simple types (enums): {}", simple_types.len());
    for t in &simple_types {
        println!("  - {}", t);
    }

    println!();
    println!("Complex types: {}", complex_types.len());
    for t in &complex_types {
        println!("  - {}", t);
    }

    println!();
    println!("Root elements: {}", root_elements.len());
    for e in &root_elements {
        println!("  - {}", e);
    }

    println!();
    println!("Total enum values:   {}", enum_count);
    println!("Total attributes:    {}", attr_count);

    Ok(())
}
