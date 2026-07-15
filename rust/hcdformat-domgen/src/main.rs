//! SPEC-SOURCE = CODEGEN. Parse the canonical Rust model AST with `syn` and EMIT the `#[pydom]`
//! mirror and tagged-choice declarations that the `hcdformat-pyderive` attribute macros expand into
//! Python DOM handles. Output is a single checked-in file (`hcdformat-py/src/dom_generated.rs`); a
//! drift check regenerates and diffs it so the bindings cannot silently diverge from the model.
//!
//! The model is a DAG: a shared type (`Pose`, `Color`, geometry primitives, …) is reachable from many
//! parents. This tool walks every `pub struct`'s fields, records each parent→child SITE, and emits one
//! mirror per reachable struct whose `#[pydom]` attribute lists all of that type's sites (one locator
//! enum variant per site). Exactly one selected model root owns each generated DOM. Generic tuple
//! tagged choices receive independent authoring methods, including deterministic draft constructors
//! for struct-valued arms. The irregular named-field `VisualAppearance` choice remains an explicit
//! companion because its public API intentionally differs from the generic tuple-choice surface.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use proc_macro2::TokenTree;
use quote::{format_ident, quote, ToTokens};
use syn::{Item, Type};

/// A `pub struct`'s (or `text_child!`-generated struct's) fields, in declaration order.
struct StructDef {
    name: String,
    fields: Vec<(String, Type)>,
    private_fields: Vec<String>,
    defaultable: bool,
    unit: bool,
}

struct ChoiceDef {
    name: String,
    variants: Vec<ChoiceVariantDef>,
    explicit_companion: Option<String>,
}

struct ChoiceVariantDef {
    name: String,
    label: String,
    payload: ChoicePayloadDef,
}

enum ChoicePayloadDef {
    Unit,
    Struct { ty: Type, name: String },
    String(Type),
    Scalar(Type),
    Enum(Type),
}

struct RawChoiceDef {
    name: String,
    filename: String,
    variants: Vec<RawChoiceVariantDef>,
}

struct RawChoiceVariantDef {
    name: String,
    label: String,
    fields: syn::Fields,
}

struct UnitEnumDef {
    name: String,
    variants: Vec<UnitEnumVariantDef>,
}

struct UnitEnumVariantDef {
    name: String,
    label: String,
}

struct EmitContext<'a> {
    structs: &'a BTreeMap<String, StructDef>,
    aliases: &'a BTreeMap<String, Type>,
    choices: &'a BTreeMap<String, ChoiceDef>,
    unit_enums: &'a BTreeMap<String, UnitEnumDef>,
    is_struct: &'a dyn Fn(&str) -> bool,
    choice_handle: &'a dyn Fn(&str) -> Option<String>,
    is_generic_choice: &'a dyn Fn(&str) -> bool,
    is_enum: &'a dyn Fn(&str) -> bool,
    is_validated_string: &'a dyn Fn(&str) -> bool,
    py_handle: &'a dyn Fn(&str) -> String,
}

/// Cardinality of a parent→child use.
#[derive(Clone, Copy)]
enum Card {
    Vec,
    Opt,
    Req,
}
impl Card {
    fn tag(self) -> &'static str {
        match self {
            Card::Vec => "vec",
            Card::Opt => "opt",
            Card::Req => "req",
        }
    }
}

/// One place a child type is USED: (parent struct, field ident, cardinality).
enum Site {
    Field {
        parent: String,
        field: String,
        card: Card,
    },
    Choice {
        parent: String,
        variant: String,
    },
}

const AUTHORED_MODULES: &[&str] = &[
    "mod",
    "pose",
    "collision",
    "common",
    "connectivity_xml",
    "dynamic_surface",
    "enums",
    "extension",
    "frame",
    "geometry",
    "group",
    "hmi",
    "include",
    "inertial",
    "joint",
    "motor",
    "power",
    "sensor",
    "sensor_params",
    "software",
    "stream_profile",
    "transmission",
    "visual",
];

const AUTHORED_SYMBOL_ALIASES: &[(&str, &str, &str)] =
    &[("connectivity_xml", "Mesh", "ConnectivityMesh")];

const EXTERNAL_VOCABULARY_ENUMS: &[&str] = &[
    "Carrier",
    "EeeMode",
    "Fidelity",
    "GptpClockKind",
    "MacsecEnforcement",
    "Purpose",
    "TerminationMounting",
    "TrafficPreemption",
];

const EXTERNAL_VALIDATED_STRING_TYPES: &[&str] = &["QualifiedId"];

const EXPLICIT_CHOICE_COMPANIONS: &[(&str, &str, &str)] =
    &[("VisualAppearance", "PyVisualAppearance", "Hcdf")];

#[derive(Clone)]
struct DomGenConfig {
    model_dir: PathBuf,
    modules: Vec<String>,
    roots: Vec<String>,
    collision_root: Option<String>,
}

struct ModelIndex {
    structs: BTreeMap<String, StructDef>,
    struct_names: Vec<String>,
    enum_names: BTreeSet<String>,
    unit_enums: BTreeMap<String, UnitEnumDef>,
    choices: BTreeMap<String, ChoiceDef>,
    aliases: BTreeMap<String, Type>,
    module_names: Vec<String>,
}

struct GeneratedDom {
    source: String,
    emitted: Vec<String>,
    unreached: Vec<String>,
}

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut args = std::env::args().skip(1);
    let model_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../hcdformat-rs/src/model"));
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../hcdformat-py/src/dom_generated.rs"));
    let roots = args
        .next()
        .map(|value| split_list(&value))
        .unwrap_or_else(|| vec!["Hcdf".to_string()]);
    let modules = args
        .next()
        .filter(|value| value != "-")
        .map(|value| split_list(&value))
        .unwrap_or_else(|| {
            AUTHORED_MODULES
                .iter()
                .map(|value| (*value).to_string())
                .collect()
        });
    let collision_root = args.next().filter(|value| !value.is_empty());
    if args.next().is_some() {
        eprintln!("domgen: expected [model-dir] [output] [roots] [modules] [collision-root]");
        std::process::exit(2);
    }

    let config = DomGenConfig {
        model_dir,
        modules,
        roots,
        collision_root,
    };
    let generated = match generate_dom(&config) {
        Ok(generated) => generated,
        Err(error) => {
            eprintln!("domgen: {error}");
            std::process::exit(2);
        }
    };
    std::fs::write(&out, generated.source).expect("write generated file");
    let status = Command::new("rustfmt")
        .arg("--edition")
        .arg("2021")
        .arg(&out)
        .status()
        .expect("run rustfmt");
    assert!(status.success(), "rustfmt failed");
    eprintln!(
        "domgen: emitted {} mirrors for owner root {:?} to {}",
        generated.emitted.len(),
        config.roots[0],
        out.display()
    );
    eprintln!(
        "domgen: {} struct(s) reachable only through tagged choices or excluded roots: {:?}",
        generated.unreached.len(),
        generated.unreached
    );
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn generate_dom(config: &DomGenConfig) -> Result<GeneratedDom, String> {
    let owner_root = configured_owner_root(config)?;
    let index = load_model(config)?;
    if !index.structs.contains_key(owner_root) {
        return Err(format!(
            "configured owner root {owner_root:?} is not an authored struct"
        ));
    }
    if let Some(collision_root) = config.collision_root.as_deref() {
        if !index.structs.contains_key(collision_root) {
            return Err(format!(
                "configured collision root {collision_root:?} is not an authored struct"
            ));
        }
        if collision_root == owner_root {
            return Err(format!(
                "configured collision root {collision_root:?} must differ from owner root"
            ));
        }
    }
    let is_struct = |name: &str| index.structs.contains_key(name);
    let is_enum = |name: &str| index.enum_names.contains(name);
    let is_choice = |name: &str| index.choices.contains_key(name);
    let is_generic_choice = |name: &str| {
        index
            .choices
            .get(name)
            .is_some_and(|choice| choice.explicit_companion.is_none())
    };

    let mut edges: Vec<(String, String, Card, String)> = Vec::new();
    let mut choice_edges: Vec<(String, String, Card, String)> = Vec::new();
    let mut classify_err: Vec<String> = Vec::new();
    for sname in index.structs.keys() {
        let sd = &index.structs[sname];
        for (fname, fty) in &sd.fields {
            if let Some((child, card)) =
                child_site(fty, &index.aliases, &is_struct, &is_choice, &is_enum)
            {
                match child {
                    Ok(child) => edges.push((sname.clone(), fname.clone(), card, child)),
                    Err(msg) => classify_err.push(format!("{sname}.{fname}: {msg}")),
                }
            }
            match choice_field(fty, &index.aliases, &is_choice) {
                Ok(Some((choice, card))) => {
                    choice_edges.push((sname.clone(), fname.clone(), card, choice));
                }
                Ok(None) => {}
                Err(error) => classify_err.push(format!("{sname}.{fname}: {error}")),
            }
        }
    }
    if !classify_err.is_empty() {
        return Err(format!(
            "unsupported field shapes:\n{}",
            classify_err.join("\n")
        ));
    }

    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (p, _f, _c, ch) in &edges {
        adj.entry(p.as_str()).or_default().push(ch.as_str());
    }
    for (parent, _field, _card, choice) in &choice_edges {
        adj.entry(parent.as_str())
            .or_default()
            .push(choice.as_str());
    }
    for choice in index.choices.values() {
        if choice.explicit_companion.is_some() {
            continue;
        }
        for variant in &choice.variants {
            if let ChoicePayloadDef::Struct { name, .. } = &variant.payload {
                adj.entry(choice.name.as_str())
                    .or_default()
                    .push(name.as_str());
            }
        }
    }
    let reachable_set = reachable_from(owner_root, &adj);
    let collision_set = config
        .collision_root
        .as_deref()
        .map(|root| reachable_from(root, &adj))
        .unwrap_or_default();
    let owner_namespace = owner_root.strip_suffix("Document").unwrap_or(owner_root);
    let py_handle_for = |name: &str| {
        if collision_set.contains(name) {
            format!("Py{owner_namespace}{}", name.trim_end_matches('_'))
        } else {
            py_handle(name)
        }
    };
    let choice_handle_for = |name: &str| {
        index.choices.get(name).map(|choice| {
            choice
                .explicit_companion
                .clone()
                .unwrap_or_else(|| py_handle_for(name))
        })
    };

    // Sites for each child, keeping ONLY edges whose parent is reachable.
    let mut sites: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    for (p, f, c, ch) in &edges {
        if reachable_set.contains(p) {
            sites.entry(ch.clone()).or_default().push(Site::Field {
                parent: p.clone(),
                field: f.clone(),
                card: *c,
            });
        }
    }
    for choice in index.choices.values() {
        if choice.explicit_companion.is_some() || !reachable_set.contains(&choice.name) {
            continue;
        }
        for variant in &choice.variants {
            if let ChoicePayloadDef::Struct { name, .. } = &variant.payload {
                sites.entry(name.clone()).or_default().push(Site::Choice {
                    parent: choice.name.clone(),
                    variant: variant.name.clone(),
                });
            }
        }
    }

    let mut choice_sites: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    for (parent, field, card, choice_name) in &choice_edges {
        let choice = &index.choices[choice_name];
        if reachable_set.contains(parent) && choice.explicit_companion.is_none() {
            choice_sites
                .entry(choice_name.clone())
                .or_default()
                .push(Site::Field {
                    parent: parent.clone(),
                    field: field.clone(),
                    card: *card,
                });
        }
    }

    let mut all: Vec<String> = index.struct_names.clone();
    all.sort();
    all.dedup();
    let unreached: Vec<String> = all
        .iter()
        .filter(|n| !reachable_set.contains(*n))
        .cloned()
        .collect();
    let emit_structs: Vec<String> = all
        .iter()
        .filter(|n| reachable_set.contains(*n))
        .cloned()
        .collect();
    let emit_choices: Vec<String> = index
        .choices
        .values()
        .filter(|choice| {
            choice.explicit_companion.is_none() && reachable_set.contains(&choice.name)
        })
        .map(|choice| choice.name.clone())
        .collect();
    validate_draft_function_names(&emit_choices, &index.choices)?;
    validate_explicit_companion_owners(owner_root, &reachable_set, &index.choices)?;

    let companions: BTreeSet<String> = index
        .choices
        .values()
        .filter(|choice| reachable_set.contains(&choice.name))
        .filter_map(|choice| choice.explicit_companion.clone())
        .collect();

    let mut o = String::new();
    let shared_enum_impls = config.collision_root.is_some();
    let used_validated_strings = emit_structs
        .iter()
        .flat_map(|name| index.structs[name].fields.iter().map(|(_, ty)| ty))
        .filter_map(|ty| {
            EXTERNAL_VALIDATED_STRING_TYPES
                .iter()
                .find(|name| type_contains_ident(ty, name))
                .copied()
        })
        .collect::<BTreeSet<_>>();
    o.push_str(&header(
        &index.module_names,
        &companions,
        &index.unit_enums,
        shared_enum_impls,
        &used_validated_strings,
        !emit_choices.is_empty(),
    ));
    let emit_context = EmitContext {
        structs: &index.structs,
        aliases: &index.aliases,
        choices: &index.choices,
        unit_enums: &index.unit_enums,
        is_struct: &is_struct,
        choice_handle: &choice_handle_for,
        is_generic_choice: &is_generic_choice,
        is_enum: &is_enum,
        is_validated_string: &|name| EXTERNAL_VALIDATED_STRING_TYPES.contains(&name),
        py_handle: &py_handle_for,
    };
    for name in &emit_structs {
        o.push_str(&emit_mirror(
            &index.structs[name],
            sites.get(name),
            name == owner_root,
            owner_root,
            &emit_context,
        )?);
        o.push('\n');
    }
    for name in &emit_choices {
        o.push_str(&emit_choice(
            &index.choices[name],
            choice_sites.get(name),
            owner_root,
            &emit_context,
        )?);
        o.push('\n');
    }
    let mut emitted = emit_structs;
    emitted.extend(emit_choices);
    let emitted_handles = emitted
        .iter()
        .map(|name| py_handle_for(name))
        .collect::<Vec<_>>();
    o.push_str(&emit_register(&emitted_handles));
    o.push_str(&emit_pyi(&emitted_handles));

    Ok(GeneratedDom {
        source: o,
        emitted,
        unreached,
    })
}

fn configured_owner_root(config: &DomGenConfig) -> Result<&str, String> {
    if config.roots.is_empty() {
        return Err("exactly one owner root is required, but none was configured".to_owned());
    }
    let mut seen = BTreeSet::new();
    for root in &config.roots {
        if !seen.insert(root) {
            return Err(format!("duplicate configured owner root {root:?}"));
        }
    }
    if config.roots.len() != 1 {
        return Err(format!(
            "exactly one owner root is required, but multiple were configured: {:?}",
            config.roots
        ));
    }
    Ok(&config.roots[0])
}

fn reachable_from(root: &str, adjacency: &BTreeMap<&str, Vec<&str>>) -> BTreeSet<String> {
    let mut reachable = BTreeSet::from([root.to_owned()]);
    let mut stack = vec![root];
    while let Some(name) = stack.pop() {
        for child in adjacency.get(name).into_iter().flatten() {
            if reachable.insert((*child).to_owned()) {
                stack.push(child);
            }
        }
    }
    reachable
}

fn validate_draft_function_names(
    emitted_choices: &[String],
    choices: &BTreeMap<String, ChoiceDef>,
) -> Result<(), String> {
    let mut names = BTreeMap::new();
    for choice_name in emitted_choices {
        let choice = &choices[choice_name];
        for variant in &choice.variants {
            if !matches!(variant.payload, ChoicePayloadDef::Struct { .. }) {
                continue;
            }
            let function = format!(
                "__draft_{}_{}",
                snake_rust(choice_name),
                snake_rust(&variant.name)
            );
            let origin = format!("{}::{}", choice.name, variant.name);
            if let Some(previous) = names.insert(function.clone(), origin.clone()) {
                return Err(format!(
                    "generated draft function {function:?} collides for {previous} and {origin}"
                ));
            }
        }
    }
    Ok(())
}

fn validate_explicit_companion_owners(
    owner_root: &str,
    reachable: &BTreeSet<String>,
    choices: &BTreeMap<String, ChoiceDef>,
) -> Result<(), String> {
    for choice in choices.values() {
        let Some(companion) = choice.explicit_companion.as_deref() else {
            continue;
        };
        if !reachable.contains(&choice.name) {
            continue;
        }
        let companion_owner = explicit_choice_companion_owner(&choice.name).ok_or_else(|| {
            format!(
                "choice {:?} declares explicit companion {companion:?} without owner-root metadata",
                choice.name
            )
        })?;
        if owner_root != companion_owner {
            return Err(format!(
                "configured owner root {owner_root:?} reaches choice {:?}, but its explicit data-enum companion {companion:?} is bound to owner root {companion_owner:?}; select owner root {companion_owner:?}",
                choice.name
            ));
        }
    }
    Ok(())
}

fn load_model(config: &DomGenConfig) -> Result<ModelIndex, String> {
    let mut structs = BTreeMap::new();
    let mut struct_names = Vec::new();
    let include_external_vocabulary = config
        .modules
        .iter()
        .any(|module| module == "connectivity_xml");
    let mut enum_names: BTreeSet<String> = if include_external_vocabulary {
        EXTERNAL_VOCABULARY_ENUMS
            .iter()
            .map(|name| (*name).to_string())
            .collect()
    } else {
        BTreeSet::new()
    };
    let mut unit_enums = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    let mut raw_choices = Vec::new();
    let mut origins: BTreeMap<String, String> = BTreeMap::new();
    let mut module_names = Vec::new();
    let mut seen_modules = BTreeSet::new();

    for module in &config.modules {
        if !is_safe_module_name(module) {
            return Err(format!("invalid authored module name {module:?}"));
        }
        if !seen_modules.insert(module.clone()) {
            return Err(format!("duplicate authored module {module:?}"));
        }
        let filename = if module == "mod" {
            "mod.rs".to_string()
        } else {
            format!("{module}.rs")
        };
        let path = config.model_dir.join(&filename);
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let file =
            syn::parse_file(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        if module != "mod" && module != "enums" && module != "pose" {
            module_names.push(module.clone());
        }
        for item in &file.items {
            match item {
                Item::Struct(item) if is_pub(&item.vis) => {
                    let name = exposed_symbol_name(module, &item.ident.to_string());
                    insert_symbol(&mut origins, &name, &filename, "struct")?;
                    let (fields, unit) = match &item.fields {
                        syn::Fields::Named(named) => (
                            named
                                .named
                                .iter()
                                .filter(|field| is_pub(&field.vis))
                                .map(|field| {
                                    (field.ident.as_ref().unwrap().to_string(), field.ty.clone())
                                })
                                .collect(),
                            false,
                        ),
                        syn::Fields::Unit => (Vec::new(), true),
                        _ => {
                            return Err(format!(
                                "{filename}: public struct {name:?} must use named or unit fields"
                            ));
                        }
                    };
                    struct_names.push(name.clone());
                    let private_fields = match &item.fields {
                        syn::Fields::Named(named) => named
                            .named
                            .iter()
                            .filter(|field| !is_pub(&field.vis))
                            .map(|field| {
                                field
                                    .ident
                                    .as_ref()
                                    .map(ToString::to_string)
                                    .unwrap_or_else(|| "<unnamed>".to_owned())
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    structs.insert(
                        name.clone(),
                        StructDef {
                            name,
                            fields,
                            private_fields,
                            defaultable: derives_default(&item.attrs)?,
                            unit,
                        },
                    );
                }
                Item::Enum(item) if is_pub(&item.vis) => {
                    let name = exposed_symbol_name(module, &item.ident.to_string());
                    insert_symbol(&mut origins, &name, &filename, "enum")?;
                    let has_data = item
                        .variants
                        .iter()
                        .any(|variant| !matches!(variant.fields, syn::Fields::Unit));
                    if has_data {
                        let rename_all = serde_rename_all(&item.attrs)?;
                        let variants = item
                            .variants
                            .iter()
                            .map(|variant| {
                                Ok(RawChoiceVariantDef {
                                    name: variant.ident.to_string(),
                                    label: serde_variant_label(variant, rename_all.as_deref())?,
                                    fields: variant.fields.clone(),
                                })
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        raw_choices.push(RawChoiceDef {
                            name: name.clone(),
                            filename: filename.clone(),
                            variants,
                        });
                    } else {
                        unit_enums.insert(name.clone(), unit_enum_def(&name, item)?);
                    }
                    enum_names.insert(name);
                }
                Item::Type(item) if is_pub(&item.vis) => {
                    let name = exposed_symbol_name(module, &item.ident.to_string());
                    insert_symbol(&mut origins, &name, &filename, "type alias")?;
                    aliases.insert(name, (*item.ty).clone());
                }
                Item::Macro(item) if item.mac.path.is_ident("text_child") => {
                    let (raw_name, fields) = parse_text_child(item.mac.tokens.clone())?;
                    let name = exposed_symbol_name(module, &raw_name);
                    insert_symbol(&mut origins, &name, &filename, "text_child struct")?;
                    struct_names.push(name.clone());
                    structs.insert(
                        name.clone(),
                        StructDef {
                            name,
                            fields,
                            private_fields: Vec::new(),
                            defaultable: true,
                            unit: false,
                        },
                    );
                }
                _ => {}
            }
        }
    }
    for (name, target) in &aliases {
        expand_alias_type(target, &aliases, &mut BTreeSet::from([name.clone()]))?;
    }
    if include_external_vocabulary {
        for definition in load_external_unit_enums(&config.model_dir)? {
            if unit_enums
                .insert(definition.name.clone(), definition)
                .is_some()
            {
                return Err("external unit enum duplicates an authored enum".to_owned());
            }
        }
    }
    let choice_names: BTreeSet<String> = raw_choices
        .iter()
        .map(|choice| choice.name.clone())
        .collect();
    let mut choices = BTreeMap::new();
    for raw in raw_choices {
        let explicit_companion = explicit_choice_companion(&raw.name).map(str::to_owned);
        if explicit_companion.is_some() {
            choices.insert(
                raw.name.clone(),
                ChoiceDef {
                    name: raw.name,
                    variants: Vec::new(),
                    explicit_companion,
                },
            );
            continue;
        }

        let mut variants = Vec::new();
        for variant in raw.variants {
            let payload = match variant.fields {
                syn::Fields::Unit => ChoicePayloadDef::Unit,
                syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                    let expanded = expand_alias_type(
                        &fields.unnamed.first().unwrap().ty,
                        &aliases,
                        &mut BTreeSet::new(),
                    )?;
                    let Some(payload_name) = type_ident(&expanded) else {
                        return Err(format!(
                            "{}: choice {} arm {} has an unsupported payload shape; configure an explicit companion",
                            raw.filename, raw.name, variant.name
                        ));
                    };
                    if structs.contains_key(&payload_name) {
                        ChoicePayloadDef::Struct {
                            ty: expanded,
                            name: payload_name,
                        }
                    } else if payload_name == "String" {
                        ChoicePayloadDef::String(expanded)
                    } else if is_scalar(&payload_name) {
                        ChoicePayloadDef::Scalar(expanded)
                    } else if choice_names.contains(&payload_name) {
                        return Err(format!(
                            "{}: choice {} arm {} nests payload choice {}; configure an explicit companion",
                            raw.filename, raw.name, variant.name, payload_name
                        ));
                    } else if enum_names.contains(&payload_name) {
                        ChoicePayloadDef::Enum(expanded)
                    } else {
                        return Err(format!(
                            "{}: choice {} arm {} references unsupported payload type {payload_name:?}; configure an explicit companion",
                            raw.filename, raw.name, variant.name
                        ));
                    }
                }
                syn::Fields::Unnamed(fields) => {
                    return Err(format!(
                        "{}: choice {} arm {} has {} tuple payloads; configure an explicit companion",
                        raw.filename,
                        raw.name,
                        variant.name,
                        fields.unnamed.len()
                    ));
                }
                syn::Fields::Named(_) => {
                    return Err(format!(
                        "{}: choice {} arm {} has named fields; configure an explicit companion",
                        raw.filename, raw.name, variant.name
                    ));
                }
            };
            variants.push(ChoiceVariantDef {
                name: variant.name,
                label: variant.label,
                payload,
            });
        }
        choices.insert(
            raw.name.clone(),
            ChoiceDef {
                name: raw.name,
                variants,
                explicit_companion: None,
            },
        );
    }
    Ok(ModelIndex {
        structs,
        struct_names,
        enum_names,
        unit_enums,
        choices,
        aliases,
        module_names,
    })
}

fn explicit_choice_companion(name: &str) -> Option<&'static str> {
    EXPLICIT_CHOICE_COMPANIONS
        .iter()
        .find(|(choice, _, _)| *choice == name)
        .map(|(_, companion, _)| *companion)
}

fn explicit_choice_companion_owner(name: &str) -> Option<&'static str> {
    EXPLICIT_CHOICE_COMPANIONS
        .iter()
        .find(|(choice, _, _)| *choice == name)
        .map(|(_, _, owner_root)| *owner_root)
}

fn unit_enum_def(name: &str, item: &syn::ItemEnum) -> Result<UnitEnumDef, String> {
    let rename_all = serde_rename_all(&item.attrs)?;
    let mut labels = BTreeSet::new();
    let mut variants = Vec::new();
    for variant in &item.variants {
        if !matches!(variant.fields, syn::Fields::Unit) {
            return Err(format!("unit enum {name} contains a payload arm"));
        }
        let label = serde_variant_label(variant, rename_all.as_deref())?;
        if !labels.insert(label.clone()) {
            return Err(format!("unit enum {name} has duplicate value {label:?}"));
        }
        variants.push(UnitEnumVariantDef {
            name: variant.ident.to_string(),
            label,
        });
    }
    Ok(UnitEnumDef {
        name: name.to_owned(),
        variants,
    })
}

fn load_external_unit_enums(model_dir: &std::path::Path) -> Result<Vec<UnitEnumDef>, String> {
    let path = model_dir.join("connectivity.rs");
    let source =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let file = syn::parse_file(&source).map_err(|error| format!("{}: {error}", path.display()))?;
    let expected: BTreeSet<&str> = EXTERNAL_VOCABULARY_ENUMS.iter().copied().collect();
    let mut definitions = Vec::new();
    for item in &file.items {
        let Item::Enum(item) = item else {
            continue;
        };
        let name = item.ident.to_string();
        if expected.contains(name.as_str()) {
            definitions.push(unit_enum_def(&name, item)?);
        }
    }
    let found: BTreeSet<&str> = definitions
        .iter()
        .map(|definition| definition.name.as_str())
        .collect();
    if found != expected {
        return Err(format!(
            "external vocabulary enum mismatch: expected {expected:?}, found {found:?}"
        ));
    }
    Ok(definitions)
}

fn serde_rename_all(attributes: &[syn::Attribute]) -> Result<Option<String>, String> {
    serde_name_value(attributes, "rename_all")
}

fn serde_name_value(
    attributes: &[syn::Attribute],
    requested: &str,
) -> Result<Option<String>, String> {
    for attribute in attributes {
        if !attribute.path().is_ident("serde") {
            continue;
        }
        let metadata = attribute
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .map_err(|error| format!("invalid serde attribute: {error}"))?;
        for meta in metadata {
            let syn::Meta::NameValue(name_value) = meta else {
                continue;
            };
            if !name_value.path.is_ident(requested) {
                continue;
            }
            let syn::Expr::Lit(expression) = name_value.value else {
                return Err(format!("serde {requested} must be a string literal"));
            };
            let syn::Lit::Str(value) = expression.lit else {
                return Err(format!("serde {requested} must be a string literal"));
            };
            return Ok(Some(value.value()));
        }
    }
    Ok(None)
}

fn serde_variant_label(variant: &syn::Variant, rename_all: Option<&str>) -> Result<String, String> {
    if let Some(rename) = serde_name_value(&variant.attrs, "rename")? {
        return Ok(rename);
    }
    let snake = snake_rust(&variant.ident.to_string());
    let Some(rule) = rename_all else {
        return Ok(snake);
    };
    match rule {
        "kebab-case" => Ok(snake.replace('_', "-")),
        "snake_case" => Ok(snake),
        "lowercase" => Ok(snake.replace('_', "")),
        "SCREAMING_SNAKE_CASE" => Ok(snake.to_ascii_uppercase()),
        "SCREAMING-KEBAB-CASE" => Ok(snake.replace('_', "-").to_ascii_uppercase()),
        other => Err(format!("unsupported serde rename_all rule {other:?}")),
    }
}

fn insert_symbol(
    origins: &mut BTreeMap<String, String>,
    name: &str,
    module: &str,
    kind: &str,
) -> Result<(), String> {
    let origin = format!("{module} ({kind})");
    if let Some(previous) = origins.insert(name.to_string(), origin.clone()) {
        return Err(format!(
            "symbol collision for {name:?}: {previous} and {origin}"
        ));
    }
    Ok(())
}

fn is_safe_module_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn exposed_symbol_name(module: &str, name: &str) -> String {
    AUTHORED_SYMBOL_ALIASES
        .iter()
        .find(|(owner, original, _)| *owner == module && *original == name)
        .map(|(_, _, exposed)| (*exposed).to_string())
        .unwrap_or_else(|| name.to_string())
}

fn is_pub(v: &syn::Visibility) -> bool {
    matches!(v, syn::Visibility::Public(_))
}

fn derives_default(attributes: &[syn::Attribute]) -> Result<bool, String> {
    for attribute in attributes {
        if !attribute.path().is_ident("derive") {
            continue;
        }
        let derives = attribute
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            )
            .map_err(|error| format!("invalid derive attribute: {error}"))?;
        if derives.iter().any(|path| path.is_ident("Default")) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The Rust/Python HANDLE ident for a model type. Trailing underscores (only `Box_`, kept to dodge the
/// `box` keyword) are dropped so the generated `Py⟨Name⟩` stays camel-case (`non_camel_case_types`).
fn py_handle(name: &str) -> String {
    format!("Py{}", name.trim_end_matches('_'))
}

/// Parse a `text_child!(Name { field => "tag", ... })` invocation into a StructDef of `Option<String>`
/// fields (the macro generates exactly that shape).
fn parse_text_child(
    tokens: proc_macro2::TokenStream,
) -> Result<(String, Vec<(String, Type)>), String> {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let name = match trees.first() {
        Some(TokenTree::Ident(i)) => i.to_string(),
        _ => return Err("text_child: expected struct name identifier".to_string()),
    };
    let inner: Vec<TokenTree> = trees
        .iter()
        .find_map(|t| match t {
            TokenTree::Group(g) => Some(g.stream().into_iter().collect()),
            _ => None,
        })
        .ok_or_else(|| "text_child: expected brace group".to_string())?;
    let opt_string: Type = syn::parse_str("Option<String>").map_err(|error| error.to_string())?;
    let mut fields = Vec::new();
    for i in 0..inner.len() {
        if let TokenTree::Ident(id) = &inner[i] {
            if let Some(TokenTree::Punct(p)) = inner.get(i + 1) {
                if p.as_char() == '=' {
                    fields.push((id.to_string(), opt_string.clone()));
                }
            }
        }
    }
    Ok((name, fields))
}

/// If `ty` references a child STRUCT (as `Vec<T>`, `Option<T>`, or bare `T`), return `(Ok(child), card)`.
/// A field that references a struct in an UNSUPPORTED shape returns `(Err(msg), _)`. Non-struct fields
/// (strings, scalars, enums, comments, the data enum) return `None`.
fn child_site(
    ty: &Type,
    aliases: &BTreeMap<String, Type>,
    is_struct: &dyn Fn(&str) -> bool,
    is_choice: &dyn Fn(&str) -> bool,
    _is_enum: &dyn Fn(&str) -> bool,
) -> Option<(Result<String, String>, Card)> {
    let expanded = match expand_alias_type(ty, aliases, &mut BTreeSet::new()) {
        Ok(expanded) => expanded,
        Err(error) => return Some((Err(error), Card::Req)),
    };
    if let Some(inner) = inner1(&expanded, "Vec") {
        let inner = match expand_alias_type(&inner, aliases, &mut BTreeSet::new()) {
            Ok(inner) => inner,
            Err(error) => return Some((Err(error), Card::Vec)),
        };
        if let Some(id) = type_ident(&inner) {
            if is_struct(&id) {
                return Some((Ok(id), Card::Vec));
            }
        }
        if let Some(found) = contained_struct(&inner, aliases, is_struct) {
            return Some((
                Err(format!("Vec element contains nested struct {found:?}")),
                Card::Vec,
            ));
        }
        return None;
    }
    if let Some(inner) = inner1(&expanded, "Option") {
        let inner = match expand_alias_type(&inner, aliases, &mut BTreeSet::new()) {
            Ok(inner) => inner,
            Err(error) => return Some((Err(error), Card::Opt)),
        };
        if let Some(id) = type_ident(&inner) {
            if is_struct(&id) {
                return Some((Ok(id), Card::Opt));
            }
        }
        if let Some(found) = contained_struct(&inner, aliases, is_struct) {
            return Some((
                Err(format!("Option element contains nested struct {found:?}")),
                Card::Opt,
            ));
        }
        return None;
    }
    if let Some(id) = type_ident(&expanded) {
        if is_choice(&id) {
            return None;
        }
        if is_struct(&id) {
            return Some((Ok(id), Card::Req));
        }
    }
    if let Some(found) = contained_struct(&expanded, aliases, is_struct) {
        return Some((
            Err(format!("unsupported wrapper around struct {found:?}")),
            Card::Req,
        ));
    }
    None
}

fn choice_field(
    ty: &Type,
    aliases: &BTreeMap<String, Type>,
    is_choice: &dyn Fn(&str) -> bool,
) -> Result<Option<(String, Card)>, String> {
    let expanded = expand_alias_type(ty, aliases, &mut BTreeSet::new())?;
    if let Some(name) = type_ident(&expanded).filter(|name| is_choice(name)) {
        return Ok(Some((name, Card::Req)));
    }
    if let Some(inner) = inner1(&expanded, "Option") {
        let inner = expand_alias_type(&inner, aliases, &mut BTreeSet::new())?;
        if let Some(name) = type_ident(&inner).filter(|name| is_choice(name)) {
            return Ok(Some((name, Card::Opt)));
        }
    }
    if let Some(inner) = inner1(&expanded, "Vec") {
        let inner = expand_alias_type(&inner, aliases, &mut BTreeSet::new())?;
        if let Some(name) = type_ident(&inner).filter(|name| is_choice(name)) {
            return Err(format!(
                "vectors of choice {name:?} require an explicit choice-list wrapper"
            ));
        }
    }
    Ok(None)
}

fn emit_mirror(
    sd: &StructDef,
    sites: Option<&Vec<Site>>,
    is_root: bool,
    owner_root: &str,
    context: &EmitContext<'_>,
) -> Result<String, String> {
    let s = &sd.name;
    let py = (context.py_handle)(s);
    let owner_py = (context.py_handle)(owner_root);
    let mut attr =
        format!("#[pydom(py = {py}, target = {s}, owner(target = {owner_root}, py = {owner_py})");
    if is_root {
        attr.push_str(", root)]\n");
    } else {
        let mut ss: Vec<&Site> = sites.map(|v| v.iter().collect()).unwrap_or_default();
        ss.sort_by_key(|site| site_sort_key(site));
        attr.push_str(",\n");
        for site in ss {
            attr.push_str(&emit_site(site, context.py_handle));
        }
        attr.push_str(")]\n");
    }
    let mut body = format!("struct {s}Dom {{\n");
    for (fname, fty) in &sd.fields {
        body.push_str(&emit_field(fname, fty, context)?);
    }
    body.push_str("}\n");
    Ok(format!("{attr}{body}"))
}

fn site_sort_key(site: &Site) -> (&str, &str, u8) {
    match site {
        Site::Field { parent, field, .. } => (parent, field, 0),
        Site::Choice { parent, variant } => (parent, variant, 1),
    }
}

fn emit_site(site: &Site, py_handle_for: &dyn Fn(&str) -> String) -> String {
    match site {
        Site::Field {
            parent,
            field,
            card,
        } => format!(
            "    site(parent = {pp}, ptype = {parent}, field = {field}, card = {card}),\n",
            pp = py_handle_for(parent),
            card = card.tag(),
        ),
        Site::Choice { parent, variant } => format!(
            "    choice_site(parent = {pp}, ptype = {parent}, variant = {variant}),\n",
            pp = py_handle_for(parent),
        ),
    }
}

fn emit_choice(
    choice: &ChoiceDef,
    sites: Option<&Vec<Site>>,
    owner_root: &str,
    context: &EmitContext<'_>,
) -> Result<String, String> {
    if choice.explicit_companion.is_some() {
        return Err(format!(
            "choice {} has an explicit companion and must not be generated",
            choice.name
        ));
    }
    let py = (context.py_handle)(&choice.name);
    let owner_py = (context.py_handle)(owner_root);
    let mut attr = format!(
        "#[pychoice(py = {py}, target = {}, owner(target = {owner_root}, py = {owner_py}),\n",
        choice.name
    );
    let mut sorted_sites: Vec<&Site> = sites
        .map(|value| value.iter().collect())
        .unwrap_or_default();
    sorted_sites.sort_by_key(|site| site_sort_key(site));
    if sorted_sites.is_empty() {
        return Err(format!(
            "reachable choice {} has no reachable parent field",
            choice.name
        ));
    }
    for site in sorted_sites {
        match site {
            Site::Field { .. } => attr.push_str(&emit_site(site, context.py_handle)),
            Site::Choice { .. } => {
                return Err(format!(
                    "choice {} cannot be nested as a choice payload",
                    choice.name
                ));
            }
        }
    }
    attr.push_str(")]\n");

    let mut drafts = String::new();
    let mut body = format!("enum {}Dom {{\n", choice.name);
    for variant in &choice.variants {
        let name = &variant.name;
        let label = format!("{:?}", variant.label);
        match &variant.payload {
            ChoicePayloadDef::Unit => {
                body.push_str(&format!("    #[pychoice(name = {label})]\n    {name},\n"));
            }
            ChoicePayloadDef::Struct { ty, name: child } => {
                let draft_name =
                    format!("__draft_{}_{}", snake_rust(&choice.name), snake_rust(name));
                let draft = draft_expr(
                    ty,
                    context,
                    &mut Vec::new(),
                    &format!("choice {} arm {}", choice.name, name),
                )?;
                drafts.push_str(&format!(
                    "fn {draft_name}() -> {} {{ {} }}\n",
                    ty.to_token_stream(),
                    draft
                ));
                body.push_str(&format!(
                    "    #[pychoice(name = {label}, payload = {}, draft = {draft_name})]\n    {name}({}),\n",
                    (context.py_handle)(child),
                    ty.to_token_stream()
                ));
            }
            ChoicePayloadDef::String(ty) | ChoicePayloadDef::Scalar(ty) => {
                body.push_str(&format!(
                    "    #[pychoice(name = {label})]\n    {name}({}),\n",
                    ty.to_token_stream()
                ));
            }
            ChoicePayloadDef::Enum(ty) => {
                body.push_str(&format!(
                    "    #[pychoice(name = {label}, mapped_enum)]\n    {name}({}),\n",
                    ty.to_token_stream()
                ));
            }
        }
    }
    body.push_str("}\n");
    Ok(format!("{drafts}{attr}{body}"))
}

fn draft_expr(
    ty: &Type,
    context: &EmitContext<'_>,
    stack: &mut Vec<String>,
    path: &str,
) -> Result<proc_macro2::TokenStream, String> {
    let expanded = expand_alias_type(ty, context.aliases, &mut BTreeSet::new())?;
    if inner1(&expanded, "Option").is_some() {
        return Ok(quote! { None });
    }
    if inner1(&expanded, "Vec").is_some() {
        return Ok(quote! { Vec::new() });
    }
    if let Some((element, length)) = array_parts(&expanded) {
        let value = draft_expr(element, context, stack, &format!("{path}[array-element]"))?;
        return Ok(quote! { std::array::from_fn::<_, #length, _>(|_| #value) });
    }

    let Some(name) = type_ident(&expanded) else {
        return Err(format!(
            "{path}: cannot construct draft for unsupported type {expanded:?}"
        ));
    };
    match name.as_str() {
        "String" => return Ok(quote! { String::new() }),
        "f64" | "f32" => return Ok(quote! { 0.0 }),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => return Ok(quote! { 0 }),
        "bool" => return Ok(quote! { false }),
        _ => {}
    }

    if let Some(definition) = context.structs.get(&name) {
        if definition.defaultable {
            return Ok(quote! { <#expanded as Default>::default() });
        }
        if !definition.private_fields.is_empty() {
            return Err(format!(
                "{path}: non-Default struct {name} has inaccessible fields {:?}; generated draft construction cannot use an out-of-module literal",
                definition.private_fields
            ));
        }
        if let Some(position) = stack.iter().position(|item| item == &name) {
            let mut cycle = stack[position..].to_vec();
            cycle.push(name);
            return Err(format!(
                "{path}: recursive draft construction cycle: {}",
                cycle.join(" -> ")
            ));
        }
        stack.push(name.clone());
        let result = if definition.unit {
            Ok(quote! { #expanded })
        } else {
            let mut fields = Vec::new();
            for (field, field_type) in &definition.fields {
                let field_ident = format_ident!("{field}");
                let value = draft_expr(field_type, context, stack, &format!("{path}.{field}"))?;
                fields.push(quote! { #field_ident: #value });
            }
            Ok(quote! { #expanded { #(#fields),* } })
        };
        stack.pop();
        return result;
    }

    if let Some(choice) = context.choices.get(&name) {
        if choice.explicit_companion.is_some() {
            return Err(format!(
                "{path}: choice {name} uses an explicit companion and has no generated draft contract"
            ));
        }
        if let Some(position) = stack.iter().position(|item| item == &name) {
            let mut cycle = stack[position..].to_vec();
            cycle.push(name);
            return Err(format!(
                "{path}: recursive draft construction cycle: {}",
                cycle.join(" -> ")
            ));
        }
        stack.push(name.clone());
        let result = (|| {
            let variant = choice.variants.first().ok_or_else(|| {
                format!("{path}: choice {name} has no arms for draft construction")
            })?;
            let choice_type = &expanded;
            let variant_ident = format_ident!("{}", variant.name);
            match &variant.payload {
                ChoicePayloadDef::Unit => Ok(quote! { #choice_type::#variant_ident }),
                ChoicePayloadDef::Struct { ty, .. }
                | ChoicePayloadDef::String(ty)
                | ChoicePayloadDef::Scalar(ty)
                | ChoicePayloadDef::Enum(ty) => {
                    let payload =
                        draft_expr(ty, context, stack, &format!("{path}.{}", variant.name))?;
                    Ok(quote! { #choice_type::#variant_ident(#payload) })
                }
            }
        })();
        stack.pop();
        return result;
    }

    if let Some(definition) = context.unit_enums.get(&name) {
        let variant = definition
            .variants
            .first()
            .ok_or_else(|| format!("{path}: enum {name} has no value for draft construction"))?;
        let enum_type = &expanded;
        let variant_ident = format_ident!("{}", variant.name);
        return Ok(quote! { #enum_type::#variant_ident });
    }

    Err(format!(
        "{path}: cannot construct draft for unsupported type {expanded:?}"
    ))
}

/// Emit one mirror field: its `#[pydom]` hint (if any) plus a `name: shape,` line the macro classifies.
fn emit_field(fname: &str, fty: &Type, context: &EmitContext<'_>) -> Result<String, String> {
    let mut emitted = emit_field_inner(fname, fty, context)?;
    if is_python_keyword(fname) {
        let python_name = format!("{fname}_");
        emitted.insert_str(0, &format!("    #[pydom(name = {python_name:?})]\n"));
    }
    Ok(emitted)
}

fn emit_field_inner(fname: &str, fty: &Type, context: &EmitContext<'_>) -> Result<String, String> {
    // serde-skip side-channels (XML comments) are not schema data, so elide.
    if is_comments(fty) {
        return Ok(format!("    #[pydom(skip)]\n    {fname}: (),\n"));
    }
    let expanded = expand_alias_type(fty, context.aliases, &mut BTreeSet::new())?;
    // Vec<...>
    if let Some(inner) = inner1(&expanded, "Vec") {
        let inner = expand_alias_type(&inner, context.aliases, &mut BTreeSet::new())?;
        if let Some(id) = type_ident(&inner) {
            if id == "String" {
                return Ok(format!(
                    "    #[pydom(strlist)]\n    {fname}: Vec<String>,\n"
                ));
            }
            if is_scalar(&id) {
                return Ok(format!(
                    "    #[pydom(scalarlist)]\n    {fname}: Vec<{id}>,\n"
                ));
            }
            if (context.is_struct)(&id) {
                let nf = name_field_of(&id, context.structs, context.aliases);
                let ph = (context.py_handle)(&id);
                let clone_hint = if context.structs[&id].defaultable {
                    ""
                } else {
                    ", clone_list"
                };
                return Ok(format!(
                    "    #[pydom(list = {ph}, name_field = {nf}{clone_hint})]\n    {fname}: Vec<()>,\n"
                ));
            }
            if (context.choice_handle)(&id).is_some() {
                return Err(format!(
                    "field {fname:?}: vectors of tagged payload enum {id:?} need an explicit wrapper list"
                ));
            }
            if (context.is_enum)(&id) {
                return Ok(format!(
                    "    #[pydom(mapped_enumlist)]\n    {fname}: Vec<{id}>,\n"
                ));
            }
        }
        return Err(format!(
            "field {fname:?}: unsupported Vec element {inner:?}"
        ));
    }
    // Option<...>
    if let Some(inner) = inner1(&expanded, "Option") {
        let inner = expand_alias_type(&inner, context.aliases, &mut BTreeSet::new())?;
        if let Some(id) = type_ident(&inner) {
            match id.as_str() {
                "String" => return Ok(format!("    {fname}: Option<String>,\n")),
                "f64" | "f32" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16"
                | "u32" | "u64" | "u128" | "usize" | "bool" => {
                    return Ok(format!("    {fname}: Option<{id}>,\n"));
                }
                _ => {}
            }
            if (context.is_validated_string)(&id) {
                return Ok(format!(
                    "    #[pydom(validated_string)]\n    {fname}: Option<{id}>,\n"
                ));
            }
            if (context.is_struct)(&id) {
                let ph = (context.py_handle)(&id);
                let clone_hint = if context.structs[&id].defaultable {
                    ""
                } else {
                    ", clone_nested"
                };
                return Ok(format!(
                    "    #[pydom(nested = {ph}{clone_hint})]\n    {fname}: Option<()>,\n"
                ));
            }
            if let Some(ph) = (context.choice_handle)(&id) {
                if (context.is_generic_choice)(&id) {
                    return Ok(format!(
                        "    #[pydom(choice = {ph})]\n    {fname}: Option<()>,\n"
                    ));
                }
                return Err(format!(
                    "field {fname:?}: optional choice {id:?} needs explicit companion field support"
                ));
            }
            if (context.is_enum)(&id) {
                return Ok(format!(
                    "    #[pydom(mapped_enum)]\n    {fname}: Option<()>,\n"
                ));
            }
        }
        // fixed-size float array attribute (pose xyz/rpy/quat)
        if let Some(n) = array_len(&inner) {
            return Ok(format!(
                "    #[pydom(arr = {n})]\n    {fname}: Option<()>,\n"
            ));
        }
        return Err(format!(
            "field {fname:?}: unsupported Option element {inner:?}"
        ));
    }
    // bare
    if let Some(n) = array_len(&expanded) {
        return Ok(format!(
            "    #[pydom(arr = {n})]\n    {fname}: [f64; {n}],\n"
        ));
    }
    if let Some(id) = type_ident(&expanded) {
        if id == "String" {
            return Ok(format!("    {fname}: String,\n"));
        }
        if is_scalar(&id) {
            return Ok(format!(
                "    #[pydom(required_scalar)]\n    {fname}: {id},\n"
            ));
        }
        if (context.is_validated_string)(&id) {
            return Ok(format!(
                "    #[pydom(validated_string)]\n    {fname}: {id},\n"
            ));
        }
        if (context.is_struct)(&id) {
            let ph = (context.py_handle)(&id);
            return Ok(format!(
                "    #[pydom(required_nested = {ph})]\n    {fname}: (),\n"
            ));
        }
        if let Some(ph) = (context.choice_handle)(&id) {
            let hint = if (context.is_generic_choice)(&id) {
                "choice"
            } else {
                "data_enum"
            };
            return Ok(format!("    #[pydom({hint} = {ph})]\n    {fname}: (),\n"));
        }
        if (context.is_enum)(&id) {
            return Ok(format!("    #[pydom(mapped_enum)]\n    {fname}: (),\n"));
        }
    }
    Err(format!("field {fname:?}: unsupported shape {expanded:?}"))
}

fn is_python_keyword(value: &str) -> bool {
    matches!(
        value,
        "False"
            | "None"
            | "True"
            | "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    )
}

fn name_field_of(
    child: &str,
    structs: &BTreeMap<String, StructDef>,
    aliases: &BTreeMap<String, Type>,
) -> &'static str {
    let Some(sd) = structs.get(child) else {
        return "none";
    };
    for (fname, fty) in &sd.fields {
        if fname == "name" {
            let expanded = expand_alias_type(fty, aliases, &mut BTreeSet::new())
                .unwrap_or_else(|_| fty.clone());
            if type_ident(&expanded).as_deref() == Some("String") {
                return "req";
            }
            if inner1(&expanded, "Option")
                .and_then(|t| type_ident(&t))
                .as_deref()
                == Some("String")
            {
                return "opt";
            }
        }
    }
    "none"
}

fn emit_register(names: &[String]) -> String {
    let mut s = String::from(
        "/// Register every generated DOM pyclass on the `hcdf` extension module.\n\
         pub fn register_generated(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {\n",
    );
    for n in names {
        s.push_str(&format!("    __register_{}(m)?;\n", snake_rust(n)));
    }
    s.push_str("    Ok(())\n}\n\n");
    s
}

fn emit_pyi(names: &[String]) -> String {
    let mut s = String::from(
        "/// The concatenated `.pyi` stub for every generated handle + its list sub-handles.\n\
         pub fn pyi_generated() -> String {\n    let mut s = String::new();\n",
    );
    for n in names {
        s.push_str(&format!(
            "    s.push_str(__pyi_{}());\n    s.push('\\n');\n",
            snake_rust(n)
        ));
    }
    s.push_str("    s\n}\n");
    s
}

fn header(
    _modules: &[String],
    companions: &BTreeSet<String>,
    unit_enums: &BTreeMap<String, UnitEnumDef>,
    shared_enum_impls: bool,
    used_validated_strings: &BTreeSet<&str>,
    has_choices: bool,
) -> String {
    let mut super_imports = vec![
        "err".to_owned(),
        "resolve_index".to_owned(),
        "stale".to_owned(),
        "Resolve".to_owned(),
    ];
    super_imports.extend(companions.iter().cloned());
    let mut output = format!(
        "// @generated by hcdformat-domgen. DO NOT EDIT.\n\
         // Regenerate with: scripts/generate_dom_mirrors.sh (drift-checked in CI).\n\
         //\n\
         // One mirror per reachable model struct or choice; the `hcdformat-pyderive` attribute macros\n\
         // expand each into a write-through Python DOM handle. Each mirror's site list is every\n\
         // parent that references the type (the locator-enum variants); shared types (Pose, Color,\n\
         // geometry, …) therefore carry several sites. See hcdformat-pyderive/src/lib.rs.\n\n\
         use super::{{{}}};\n\
         use pyo3::prelude::*;\n\
         use hcdformat::model::*;\n",
        super_imports.join(", ")
    );
    if shared_enum_impls {
        output.push_str("use super::generated::DomEnum;\n");
        if !used_validated_strings.is_empty() {
            output.push_str(&format!(
                "use hcdformat::model::connectivity::{{{}}};\n",
                used_validated_strings
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else {
        output.push_str("use hcdformat::model::enums::*;\n");
        output.push_str(&format!(
            "use hcdformat::model::connectivity::{{{}}};\n",
            EXTERNAL_VOCABULARY_ENUMS.join(", ")
        ));
        output.push_str(
            "use hcdformat::model::connectivity_xml::{\n\
                 ModelPartRepresentation, ModelRepresentation, NamedQuantity, Quantity, QuantityRange,\n\
                 SelectedProfile, SelectionQuantity, SelectionQuantityChoice, SelectionRange,\n\
             };\n",
        );
    }
    if has_choices {
        output.push_str("use hcdformat_pyderive::{pydom, pychoice};\n\n");
    } else {
        output.push_str("use hcdformat_pyderive::pydom;\n\n");
    }
    if shared_enum_impls {
        return output;
    }
    output.push_str(
        "pub(super) trait DomEnum: Sized {\n\
             fn dom_value(&self) -> &'static str;\n\
             fn from_dom_value(value: &str) -> Option<Self>;\n\
         }\n\n",
    );
    for definition in unit_enums.values() {
        output.push_str(&format!("impl DomEnum for {} {{\n", definition.name));
        output.push_str("    fn dom_value(&self) -> &'static str {\n        match self {\n");
        for variant in &definition.variants {
            output.push_str(&format!(
                "            Self::{} => {:?},\n",
                variant.name, variant.label
            ));
        }
        output.push_str("        }\n    }\n");
        output.push_str(
            "    fn from_dom_value(value: &str) -> Option<Self> {\n        match value {\n",
        );
        for variant in &definition.variants {
            output.push_str(&format!(
                "            {:?} => Some(Self::{}),\n",
                variant.label, variant.name
            ));
        }
        output.push_str("            _ => None,\n        }\n    }\n}\n\n");
    }
    output
}

// ── type helpers ─────────────────────────────────────────────────────────────────────────────────

fn type_ident(ty: &Type) -> Option<String> {
    if let Type::Path(p) = ty {
        return p.path.segments.last().map(|s| s.ident.to_string());
    }
    None
}

fn type_contains_ident(ty: &Type, requested: &str) -> bool {
    match ty {
        Type::Path(path) => path.path.segments.iter().any(|segment| {
            segment.ident == requested
                || match &segment.arguments {
                    syn::PathArguments::AngleBracketed(arguments) => arguments.args.iter().any(
                        |argument| {
                            matches!(argument, syn::GenericArgument::Type(inner) if type_contains_ident(inner, requested))
                        },
                    ),
                    _ => false,
                }
        }),
        Type::Array(array) => type_contains_ident(&array.elem, requested),
        Type::Group(group) => type_contains_ident(&group.elem, requested),
        Type::Paren(paren) => type_contains_ident(&paren.elem, requested),
        Type::Reference(reference) => type_contains_ident(&reference.elem, requested),
        Type::Slice(slice) => type_contains_ident(&slice.elem, requested),
        Type::Tuple(tuple) => tuple
            .elems
            .iter()
            .any(|inner| type_contains_ident(inner, requested)),
        _ => false,
    }
}

fn is_scalar(name: &str) -> bool {
    matches!(
        name,
        "f64"
            | "f32"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "bool"
    )
}

fn expand_alias_type(
    ty: &Type,
    aliases: &BTreeMap<String, Type>,
    visiting: &mut BTreeSet<String>,
) -> Result<Type, String> {
    let Some(name) = type_ident(ty) else {
        return Ok(ty.clone());
    };
    let Some(target) = aliases.get(&name) else {
        return Ok(ty.clone());
    };
    if !visiting.insert(name.clone()) {
        let mut cycle: Vec<String> = visiting.iter().cloned().collect();
        cycle.push(name);
        return Err(format!("type alias cycle: {}", cycle.join(" -> ")));
    }
    let expanded = expand_alias_type(target, aliases, visiting);
    visiting.remove(&name);
    expanded
}

fn contained_struct(
    ty: &Type,
    aliases: &BTreeMap<String, Type>,
    is_struct: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let expanded = expand_alias_type(ty, aliases, &mut BTreeSet::new()).ok()?;
    if let Some(name) = type_ident(&expanded) {
        if is_struct(&name) {
            return Some(name);
        }
    }
    match expanded {
        Type::Path(path) => path.path.segments.iter().find_map(|segment| {
            let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                return None;
            };
            arguments.args.iter().find_map(|argument| match argument {
                syn::GenericArgument::Type(inner) => contained_struct(inner, aliases, is_struct),
                _ => None,
            })
        }),
        Type::Array(array) => contained_struct(&array.elem, aliases, is_struct),
        Type::Reference(reference) => contained_struct(&reference.elem, aliases, is_struct),
        Type::Tuple(tuple) => tuple
            .elems
            .iter()
            .find_map(|inner| contained_struct(inner, aliases, is_struct)),
        _ => None,
    }
}

fn inner_ty<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    if let Type::Path(p) = ty {
        let seg = p.path.segments.last()?;
        if seg.ident != wrapper {
            return None;
        }
        if let syn::PathArguments::AngleBracketed(a) = &seg.arguments {
            if let Some(syn::GenericArgument::Type(t)) = a.args.first() {
                return Some(t);
            }
        }
    }
    None
}

fn inner1(ty: &Type, wrapper: &str) -> Option<Type> {
    inner_ty(ty, wrapper).cloned()
}

fn array_len(ty: &Type) -> Option<usize> {
    let (element, length) = array_parts(ty)?;
    (type_ident(element).as_deref() == Some("f64")).then_some(length)
}

fn array_parts(ty: &Type) -> Option<(&Type, usize)> {
    let Type::Array(array) = ty else {
        return None;
    };
    let syn::Expr::Lit(length) = &array.len else {
        return None;
    };
    let syn::Lit::Int(length) = &length.lit else {
        return None;
    };
    Some((&array.elem, length.base10_parse().ok()?))
}

fn is_comments(ty: &Type) -> bool {
    if let Type::Path(p) = ty {
        return p.path.segments.iter().any(|s| s.ident == "comments");
    }
    false
}

fn snake_rust(name: &str) -> String {
    let mut output = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index != 0 {
                output.push('_');
            }
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct FixtureDir(PathBuf);

    impl FixtureDir {
        fn new() -> Self {
            let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "hcdformat-domgen-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("create fixture directory");
            Self(path)
        }

        fn write(&self, module: &str, source: &str) {
            std::fs::write(self.0.join(format!("{module}.rs")), source)
                .expect("write fixture module");
        }

        fn config(&self, modules: &[&str], roots: &[&str]) -> DomGenConfig {
            DomGenConfig {
                model_dir: self.0.clone(),
                modules: modules.iter().map(|value| (*value).to_string()).collect(),
                roots: roots.iter().map(|value| (*value).to_string()).collect(),
                collision_root: None,
            }
        }
    }

    impl Drop for FixtureDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_modules_support_one_owner_root_and_exact_field_wrappers() {
        let fixture = FixtureDir::new();
        fixture.write(
            "root",
            r#"
pub struct Root {
    pub required_child: Child,
    pub count: Count,
    pub samples: Vec<u32>,
    pub modes: Vec<Mode>,
    pub choice: Choice,
    pub optional_choice: Option<Choice>,
}
pub struct Second { pub label: String }
"#,
        );
        fixture.write(
            "types",
            r#"
pub type Count = u32;
pub enum Mode { A, B }
pub enum Choice { Payload(Child), Scalar(u32), Empty }
pub struct Child { pub name: String }
pub struct Number { pub value: u32 }
"#,
        );
        fixture.write("dead", "pub struct Root { pub ignored: String }");

        let generated = generate_dom(&fixture.config(&["root", "types"], &["Root"]))
            .expect("generate fixture DOM");
        assert!(generated
            .source
            .contains("target = Root, owner(target = Root, py = PyRoot), root"));
        assert!(!generated.source.contains("target = Second"));
        assert!(generated.unreached.contains(&"Second".to_string()));
        assert!(generated
            .source
            .contains("#[pydom(required_nested = PyChild)]"));
        assert!(generated.source.contains("count: u32"));
        assert!(generated.source.contains("#[pydom(scalarlist)]"));
        assert!(generated.source.contains("samples: Vec<u32>"));
        assert!(generated.source.contains("#[pydom(mapped_enumlist)]"));
        assert!(generated.source.contains("modes: Vec<Mode>"));
        assert!(generated.source.contains("#[pydom(choice = PyChoice)]"));
        assert!(generated
            .source
            .contains("#[pydom(choice = PyChoice)]\n    optional_choice: Option<()>"));
        assert!(generated.source.contains("#[pychoice(py = PyChoice"));
        assert!(generated
            .source
            .contains("fn __draft_choice_payload() -> Child"));
        assert!(generated
            .source
            .contains("payload = PyChild, draft = __draft_choice_payload"));
        assert!(generated
            .source
            .contains("choice_site(parent = PyChoice, ptype = Choice, variant = Payload)"));
        assert!(generated
            .source
            .contains("#[pychoice(name = \"scalar\")]\n    Scalar(u32)"));
        assert!(generated
            .source
            .contains("#[pychoice(name = \"empty\")]\n    Empty"));
        assert!(generated.unreached.contains(&"Number".to_string()));

        let second = generate_dom(&fixture.config(&["root", "types"], &["Second"]))
            .expect("generate alternate owner-root DOM");
        assert!(second
            .source
            .contains("target = Second, owner(target = Second, py = PySecond), root"));
        assert!(!second.source.contains("target = Root"));
        assert!(!second.source.contains("use hcdformat::Hcdf;"));
    }

    #[test]
    fn alternate_owner_reaching_real_visual_companion_is_rejected_before_emission() {
        let model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hcdformat-rs/src/model");
        let config = DomGenConfig {
            model_dir,
            modules: AUTHORED_MODULES
                .iter()
                .map(|module| (*module).to_owned())
                .collect(),
            roots: vec!["Comp".to_owned()],
            collision_root: None,
        };
        let error = generate_dom(&config)
            .err()
            .expect("owner-bound VisualAppearance companion must reject Comp owner root");
        assert!(error.contains("configured owner root \"Comp\""), "{error}");
        assert!(error.contains("choice \"VisualAppearance\""), "{error}");
        assert!(
            error.contains("companion \"PyVisualAppearance\""),
            "{error}"
        );
        assert!(error.contains("bound to owner root \"Hcdf\""), "{error}");
        assert!(error.contains("select owner root \"Hcdf\""), "{error}");
    }

    #[test]
    fn real_stream_profile_resource_is_authored_and_reachable() {
        assert!(
            AUTHORED_MODULES.contains(&"stream_profile"),
            "the canonical module list must expose authored stream-profile resources"
        );

        let model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hcdformat-rs/src/model");
        let generated = generate_dom(&DomGenConfig {
            model_dir,
            modules: AUTHORED_MODULES
                .iter()
                .map(|module| (*module).to_owned())
                .collect(),
            roots: vec!["Hcdf".to_owned()],
            collision_root: None,
        })
        .expect("generate the real authored DOM");

        assert!(
            generated
                .emitted
                .contains(&"StreamProfileResource".to_owned()),
            "the Hcdf stream-profile resource must be reachable"
        );
        assert!(
            generated.source.contains("target = StreamProfileResource"),
            "the generated DOM must expose the stream-profile resource"
        );
    }

    #[test]
    fn real_stream_profile_document_uses_collision_safe_secondary_handles() {
        let model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hcdformat-rs/src/model");
        let generated = generate_dom(&DomGenConfig {
            model_dir,
            modules: AUTHORED_MODULES
                .iter()
                .map(|module| (*module).to_owned())
                .collect(),
            roots: vec!["StreamProfileDocument".to_owned()],
            collision_root: Some("Hcdf".to_owned()),
        })
        .expect("generate the real stream-profile DOM");

        for name in [
            "StreamProfileDocument",
            "StreamProfileDependency",
            "StreamGroup",
            "StreamDefinition",
            "StreamPath",
            "StreamForwarding",
            "StreamTalker",
            "StreamListener",
            "Frer",
        ] {
            assert!(generated.emitted.contains(&name.to_owned()), "{name}");
        }
        assert!(generated
            .source
            .contains("py = PyStreamProfileDocument, target = StreamProfileDocument"));
        assert!(generated
            .source
            .contains("py = PyStreamProfileInstanceRef, target = InstanceRef"));
        assert!(generated
            .source
            .contains("#[pydom(validated_string)]\n    protocol: Option<QualifiedId>"));
        assert!(generated.source.contains(
            "#[pydom(name = \"from_\")]\n    #[pydom(required_nested = PyStreamForwardingEnd)]"
        ));
        assert!(generated.source.contains("use super::generated::DomEnum;"));
        assert!(!generated.source.contains("impl DomEnum for"));
        assert!(!generated.source.contains("ChainRef"));
    }

    #[test]
    fn real_connectivity_vocabulary_drives_classification_and_imports() {
        let model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hcdformat-rs/src/model");
        let generated = generate_dom(&DomGenConfig {
            model_dir,
            modules: AUTHORED_MODULES
                .iter()
                .map(|module| (*module).to_owned())
                .collect(),
            roots: vec!["Hcdf".to_owned()],
            collision_root: None,
        })
        .expect("generate the real authored DOM");

        let import = format!(
            "use hcdformat::model::connectivity::{{{}}};",
            EXTERNAL_VOCABULARY_ENUMS.join(", ")
        );
        assert!(generated.source.contains(&import), "{import}");
        for name in EXTERNAL_VOCABULARY_ENUMS {
            assert!(
                generated
                    .source
                    .contains(&format!("impl DomEnum for {name}")),
                "missing DomEnum implementation for {name}"
            );
        }
        for field in ["default_mode", "kind", "enforcement", "preemption"] {
            assert!(
                generated
                    .source
                    .contains(&format!("#[pydom(mapped_enum)]\n    {field}:")),
                "missing mapped enum field {field}"
            );
        }
    }

    #[test]
    fn owner_root_configuration_is_exactly_one_authored_struct() {
        let fixture = FixtureDir::new();
        fixture.write(
            "root",
            "pub struct Root { pub nested: Nested } pub struct Nested { pub name: String }",
        );

        let zero = generate_dom(&fixture.config(&["root"], &[]))
            .err()
            .expect("zero root error");
        assert!(zero.contains("exactly one owner root"), "{zero}");
        assert!(zero.contains("none was configured"), "{zero}");

        let duplicate = generate_dom(&fixture.config(&["root"], &["Root", "Root"]))
            .err()
            .expect("duplicate root error");
        assert!(
            duplicate.contains("duplicate configured owner root \"Root\""),
            "{duplicate}"
        );

        let multiple = generate_dom(&fixture.config(&["root"], &["Root", "Nested"]))
            .err()
            .expect("multiple root error");
        assert!(multiple.contains("exactly one owner root"), "{multiple}");
        assert!(multiple.contains("multiple were configured"), "{multiple}");

        let missing = generate_dom(&fixture.config(&["root"], &["Missing"]))
            .err()
            .expect("missing root error");
        assert!(
            missing.contains("owner root \"Missing\" is not an authored struct"),
            "{missing}"
        );

        let generated =
            generate_dom(&fixture.config(&["root"], &["Root"])).expect("single root generation");
        assert_eq!(generated.source.matches(", root)]").count(), 1);
        assert!(generated
            .source
            .contains("target = Nested, owner(target = Root, py = PyRoot)"));
        assert!(!generated
            .source
            .contains("target = Nested, owner(target = Nested"));
    }

    #[test]
    fn duplicate_symbols_and_alias_cycles_are_hard_errors() {
        let fixture = FixtureDir::new();
        fixture.write("one", "pub struct Duplicate { pub name: String }");
        fixture.write("two", "pub struct Duplicate { pub value: String }");
        let error = generate_dom(&fixture.config(&["one", "two"], &["Duplicate"]))
            .err()
            .expect("duplicate symbol error");
        assert!(error.contains("symbol collision"), "{error}");

        fixture.write(
            "cycle",
            "pub type Left = Right; pub type Right = Left; pub struct Root { pub value: Left }",
        );
        let error = generate_dom(&fixture.config(&["cycle"], &["Root"]))
            .err()
            .expect("alias cycle error");
        assert!(error.contains("type alias cycle"), "{error}");
    }

    #[test]
    fn unsafe_or_duplicate_module_configuration_is_rejected() {
        let fixture = FixtureDir::new();
        fixture.write("root", "pub struct Root { pub name: String }");
        let error = generate_dom(&fixture.config(&["../root"], &["Root"]))
            .err()
            .expect("unsafe module error");
        assert!(error.contains("invalid authored module name"), "{error}");
        let error = generate_dom(&fixture.config(&["root", "root"], &["Root"]))
            .err()
            .expect("duplicate module error");
        assert!(error.contains("duplicate authored module"), "{error}");
    }

    #[test]
    fn unsupported_choice_shapes_require_an_explicit_companion() {
        let fixture = FixtureDir::new();
        fixture.write(
            "root",
            "pub struct Root { pub choice: Unsupported } \
             pub enum Unsupported { Named { value: u32 }, Empty }",
        );
        let error = generate_dom(&fixture.config(&["root"], &["Root"]))
            .err()
            .expect("unsupported choice error");
        assert!(error.contains("named fields"), "{error}");
        assert!(error.contains("explicit companion"), "{error}");
    }

    #[test]
    fn recursive_drafts_and_helper_name_collisions_are_hard_errors() {
        let recursive = FixtureDir::new();
        recursive.write(
            "root",
            "pub struct Root { pub choice: LoopChoice } \
             pub enum LoopChoice { Payload(LoopPayload) } \
             pub struct LoopPayload { pub choice: LoopChoice }",
        );
        let error = generate_dom(&recursive.config(&["root"], &["Root"]))
            .err()
            .expect("recursive draft error");
        assert!(
            error.contains("recursive draft construction cycle"),
            "{error}"
        );
        assert!(error.contains("LoopPayload"), "{error}");
        assert!(error.contains("LoopChoice"), "{error}");

        let collision = FixtureDir::new();
        collision.write(
            "root",
            "pub struct Root { pub first: Ab, pub second: ab } \
             pub struct Payload { pub name: String } \
             pub enum Ab { Payload(Payload) } \
             pub enum ab { Payload(Payload) }",
        );
        let error = generate_dom(&collision.config(&["root"], &["Root"]))
            .err()
            .expect("draft helper collision error");
        assert!(error.contains("generated draft function"), "{error}");
        assert!(error.contains("collides"), "{error}");
        assert!(error.contains("Ab::Payload"), "{error}");
        assert!(error.contains("ab::Payload"), "{error}");

        let inaccessible = FixtureDir::new();
        inaccessible.write(
            "root",
            "pub struct Root { pub choice: Choice } \
             pub enum Choice { Payload(Payload) } \
             pub struct Payload { pub name: String, hidden: String }",
        );
        let error = generate_dom(&inaccessible.config(&["root"], &["Root"]))
            .err()
            .expect("inaccessible draft field error");
        assert!(error.contains("non-Default struct Payload"), "{error}");
        assert!(error.contains("inaccessible fields"), "{error}");
        assert!(error.contains("hidden"), "{error}");
    }
}
