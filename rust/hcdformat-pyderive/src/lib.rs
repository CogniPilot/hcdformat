//! Proc-macro that exposes the Rust `hcdformat` typed DOM to Python as typed, MUTABLE,
//! Rust-backed handles for the complete generated model surface.
//!
//! # What it generates
//! Applied to a small *mirror* struct that restates a model type's field shape, `#[pydom(...)]`
//! emits the handle pattern for that type:
//!   * a `#[pyclass(unsendable)]` handle carrying the declared `Py<Owner>` plus a per-type locator,
//!   * one `impl Resolve<OwnerRoot>` per model type that re-walks from the selected owner root,
//!   * a getter+setter per field (one code path per taxonomy category), and
//!   * list sub-handles (with `append`) for every `Vec` field.
//!
//! Applied to a generated tagged-choice declaration, `#[pychoice(...)]` emits an active-arm handle.
//! Scalar arms are selected from their value. Struct arms can be initialized independently from a
//! deterministic Rust draft or explicitly cloned from another live handle.
//!
//! # DAG support: per-type ENUM locators
//! The model is a DAG: a shared type (`Pose`, `Color`, `Geometry`, primitives, …) is reachable from
//! many parents. Rust coherence allows only one `impl Resolve<OwnerRoot> for T`, so a type's locator is
//! an enum
//! with one variant per PARENT SITE (`#[pydom(... site(parent=…, ptype=…, field=…, card=…) …)]`). Each
//! variant boxes its parent locator (uniform size ⇒ no `clippy::large_enum_variant`), and the single
//! `Resolve` impl matches over the variants. The variant name is `⟨ParentType⟩⟨PascalField⟩` (keyed on
//! the parent TYPE, not the `Py` handle, so the variants don't all share a `Py` prefix that would trip
//! `clippy::enum_variant_names`), computed identically where the parent hands out the child handle and
//! where the child declares the site, so the two always agree. List sub-handle CLASSES are
//! parent-qualified (`⟨ParentPy⟩⟨PascalField⟩List`) so a child reachable from two parents never collides.
//!
//! # Why an ATTRIBUTE macro, not `#[derive]`
//! A `#[derive]` cannot delete its input, so the field-only mirror struct would linger as `dead_code`,
//! and the project forbids `#[allow]`. As an attribute macro `#[pydom]` CONSUMES the mirror (it emits
//! only the handle code), so no dead struct is left and the zero-warning bar holds.
//!
//! # pyo3 isolation
//! This is a `proc-macro` crate: it links no pyo3 into any target. The emitted code (which DOES
//! reference pyo3) lands in the CALLER (`hcdformat-py`), so `hcdformat-rs` stays wasm-clean.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, ItemEnum, ItemStruct, LitStr, Meta, Token, Type};

// ── container spec (the struct-level `#[pydom(...)]`) ────────────────────────────────────────────

/// Cardinality of a single parent→child site.
#[derive(Clone, Copy, PartialEq)]
enum Card {
    Vec,
    Opt,
    Req,
}

/// One place a type is USED (one variant of its locator enum).
struct Site {
    parent_py: Ident,
    ptype: Type,
    access: SiteAccess,
}

enum SiteAccess {
    Field { field: Ident, card: Card },
    Choice { variant: Ident },
}

impl Site {
    fn variant(&self) -> Ident {
        match &self.access {
            SiteAccess::Field { field, .. } => variant_ident(&ty_ident(&self.ptype), field),
            SiteAccess::Choice { variant } => choice_variant_ident(&ty_ident(&self.ptype), variant),
        }
    }
    fn parent_loc(&self) -> Ident {
        format_ident!("{}Loc", self.parent_py)
    }
}

/// A locator-enum variant name: `⟨ParentTypeName⟩⟨PascalField⟩`. Keyed on the parent TYPE (not the `Py`
/// handle) so the variants of a shared type's `Loc` enum do NOT all share the `Py` prefix
/// (`clippy::enum_variant_names`). The parent's own getter and the child's site both compute it the same
/// way (from the parent's target type + the field ident), so they always agree.
fn variant_ident(parent_ty: &Ident, field: &Ident) -> Ident {
    format_ident!("{}{}", parent_ty, pascal(&field.to_string()))
}

fn choice_variant_ident(choice_ty: &Ident, variant: &Ident) -> Ident {
    format_ident!("{}{}", choice_ty, variant)
}

fn ty_ident(ty: &Type) -> Ident {
    match ty {
        Type::Path(p) => p.path.segments.last().unwrap().ident.clone(),
        _ => panic!("pydom: expected a path type"),
    }
}

struct Container {
    py: Ident,
    target: Type,
    owner_py: Ident,
    owner_target: Type,
    is_root: bool,
    sites: Vec<Site>,
}

// ── field spec ───────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Default)]
enum NameField {
    #[default]
    None,
    Req,
    Opt,
}

enum FieldKind {
    ReqStr,
    ReqScalar(Ident),
    OptStr,
    OptScalar(Ident),
    ReqArr(usize),
    OptArr(usize),
    EnumScalar {
        opt: bool,
    },
    MappedEnumScalar {
        opt: bool,
    },
    ValidatedString {
        inner: Ident,
        opt: bool,
    },
    StrList,
    ScalarList(Ident),
    EnumList {
        inner: Ident,
        mapped: bool,
    },
    List {
        child: Ident,
        name_field: NameField,
        clone_append: bool,
    },
    Nested {
        child: Ident,
        clone_set: bool,
    },
    RequiredNested {
        child: Ident,
    },
    DataEnum {
        wrapper: Ident,
    },
    Choice {
        wrapper: Ident,
        opt: bool,
    },
}

struct FieldSpec {
    rust: Ident,
    py_name: String,
    kind: FieldKind,
}

enum ChoicePayload {
    Unit,
    Struct { ty: Type, child: Ident, draft: Expr },
    String(Type),
    Scalar(Type),
    Enum(Type),
    MappedEnum(Type),
}

struct ChoiceVariantSpec {
    rust: Ident,
    py_name: String,
    label: String,
    payload: ChoicePayload,
}

// ── entry point ────────────────────────────────────────────────────────────────────────────────

#[proc_macro_attribute]
pub fn pydom(attr: TokenStream, item: TokenStream) -> TokenStream {
    let s: ItemStruct = syn::parse_macro_input!(item as ItemStruct);
    let metas = syn::parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);
    let ctr = match parse_container(metas) {
        Ok(c) => c,
        Err(e) => return e.to_compile_error().into(),
    };
    let fields = match parse_fields(&s) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error().into(),
    };
    if let Err(error) = validate_root_field_compatibility(&ctr, &fields) {
        return error.to_compile_error().into();
    }
    gen_all(&ctr, &fields).into()
}

#[proc_macro_attribute]
pub fn pychoice(attr: TokenStream, item: TokenStream) -> TokenStream {
    let choice: ItemEnum = syn::parse_macro_input!(item as ItemEnum);
    let metas = syn::parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);
    let container = match parse_container(metas) {
        Ok(container) => container,
        Err(error) => return error.to_compile_error().into(),
    };
    if container.is_root {
        return syn::Error::new_spanned(choice, "pychoice cannot be a root handle")
            .to_compile_error()
            .into();
    }
    let variants = match parse_choice_variants(&choice) {
        Ok(variants) => variants,
        Err(error) => return error.to_compile_error().into(),
    };
    gen_choice_all(&container, &variants).into()
}

// ── attribute parsing ──────────────────────────────────────────────────────────────────────────

fn parse_container(metas: Punctuated<Meta, Token![,]>) -> syn::Result<Container> {
    let mut py = None;
    let mut target = None;
    let mut owner = None;
    let mut is_root = false;
    let mut sites = Vec::new();
    for m in metas {
        match m {
            Meta::Path(p) if p.is_ident("root") => is_root = true,
            Meta::NameValue(nv) => {
                let key = nv
                    .path
                    .get_ident()
                    .ok_or_else(|| syn::Error::new_spanned(&nv.path, "expected identifier key"))?
                    .to_string();
                match key.as_str() {
                    "py" => py = Some(expr_ident(&nv.value)?),
                    "target" => target = Some(expr_type(&nv.value)?),
                    _ => return Err(syn::Error::new_spanned(nv.path, "unknown pydom key")),
                }
            }
            Meta::List(l) if l.path.is_ident("owner") => {
                if owner.is_some() {
                    return Err(syn::Error::new_spanned(l, "duplicate owner metadata"));
                }
                owner = Some(parse_owner(&l)?);
            }
            Meta::List(l) if l.path.is_ident("site") => sites.push(parse_site(&l, false)?),
            Meta::List(l) if l.path.is_ident("choice_site") => sites.push(parse_site(&l, true)?),
            other => return Err(syn::Error::new_spanned(other, "unexpected pydom meta")),
        }
    }
    let py = py.ok_or_else(|| err_call("missing `py = ...`"))?;
    let target = target.ok_or_else(|| err_call("missing `target = ...`"))?;
    let (owner_target, owner_py) =
        owner.ok_or_else(|| err_call("missing `owner(target = ..., py = ...)`"))?;
    if is_root {
        if target != owner_target {
            return Err(err_call("root target must match owner target"));
        }
        if py != owner_py {
            return Err(err_call(
                "root Python handle must match owner Python handle",
            ));
        }
        if !sites.is_empty() {
            return Err(err_call("root handle cannot declare parent sites"));
        }
    }
    Ok(Container {
        py,
        target,
        owner_py,
        owner_target,
        is_root,
        sites,
    })
}

fn parse_owner(l: &syn::MetaList) -> syn::Result<(Type, Ident)> {
    let inner: Punctuated<Meta, Token![,]> = l.parse_args_with(Punctuated::parse_terminated)?;
    let mut target = None;
    let mut py = None;
    for meta in inner {
        let Meta::NameValue(value) = meta else {
            return Err(syn::Error::new_spanned(
                meta,
                "owner: expected `key = value`",
            ));
        };
        if value.path.is_ident("target") {
            if target.is_some() {
                return Err(syn::Error::new_spanned(value, "duplicate owner target"));
            }
            target = Some(expr_type(&value.value)?);
        } else if value.path.is_ident("py") {
            if py.is_some() {
                return Err(syn::Error::new_spanned(
                    value,
                    "duplicate owner Python handle",
                ));
            }
            py = Some(expr_ident(&value.value)?);
        } else {
            return Err(syn::Error::new_spanned(value.path, "unknown owner key"));
        }
    }
    Ok((
        target.ok_or_else(|| err_call("owner: missing `target = ...`"))?,
        py.ok_or_else(|| err_call("owner: missing `py = ...`"))?,
    ))
}

fn parse_site(l: &syn::MetaList, is_choice: bool) -> syn::Result<Site> {
    let inner: Punctuated<Meta, Token![,]> = l.parse_args_with(Punctuated::parse_terminated)?;
    let mut parent_py = None;
    let mut ptype = None;
    let mut field = None;
    let mut card = None;
    let mut variant = None;
    for m in inner {
        let nv = match m {
            Meta::NameValue(nv) => nv,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "site: expected `key = value`",
                ))
            }
        };
        let key = nv
            .path
            .get_ident()
            .ok_or_else(|| syn::Error::new_spanned(&nv.path, "site: identifier key"))?
            .to_string();
        match key.as_str() {
            "parent" => parent_py = Some(expr_ident(&nv.value)?),
            "ptype" => ptype = Some(expr_type(&nv.value)?),
            "field" => field = Some(expr_ident(&nv.value)?),
            "variant" => variant = Some(expr_ident(&nv.value)?),
            "card" => {
                let k = expr_ident(&nv.value)?;
                card = Some(match k.to_string().as_str() {
                    "vec" => Card::Vec,
                    "opt" => Card::Opt,
                    "req" => Card::Req,
                    _ => return Err(syn::Error::new_spanned(k, "card = vec|opt|req")),
                });
            }
            _ => return Err(syn::Error::new_spanned(nv.path, "unknown site key")),
        }
    }
    let access = if is_choice {
        if field.is_some() || card.is_some() {
            return Err(err_call("choice_site accepts variant, not field/card"));
        }
        SiteAccess::Choice {
            variant: variant.ok_or_else(|| err_call("choice_site: missing variant"))?,
        }
    } else {
        if variant.is_some() {
            return Err(err_call("site accepts field/card, not variant"));
        }
        SiteAccess::Field {
            field: field.ok_or_else(|| err_call("site: missing field"))?,
            card: card.ok_or_else(|| err_call("site: missing card"))?,
        }
    };
    Ok(Site {
        parent_py: parent_py.ok_or_else(|| err_call("site: missing parent"))?,
        ptype: ptype.ok_or_else(|| err_call("site: missing ptype"))?,
        access,
    })
}

fn parse_fields(s: &ItemStruct) -> syn::Result<Vec<FieldSpec>> {
    let named = match &s.fields {
        syn::Fields::Named(n) => n,
        _ => {
            return Err(syn::Error::new_spanned(
                s,
                "pydom needs a named-field struct",
            ))
        }
    };
    let mut out = Vec::new();
    for f in &named.named {
        let rust = f.ident.clone().unwrap();
        let hints = parse_field_hints(f)?;
        if hints.skip {
            continue;
        }
        let py_name = hints
            .name
            .clone()
            .unwrap_or_else(|| rust.to_string().trim_end_matches('_').to_string());
        let kind = classify_field(&rust, &f.ty, &hints)?;
        out.push(FieldSpec {
            rust,
            py_name,
            kind,
        });
    }
    Ok(out)
}

fn validate_root_field_compatibility(c: &Container, fields: &[FieldSpec]) -> syn::Result<()> {
    if !c.is_root {
        return Ok(());
    }
    let Some(field) = fields
        .iter()
        .find(|field| matches!(field.kind, FieldKind::DataEnum { .. }))
    else {
        return Ok(());
    };
    Err(syn::Error::new(
        field.rust.span(),
        format!(
            "root field {:?} uses an owner-bound data-enum companion; owner-bound data-enum fields must remain below their companion's compatible owner root",
            field.py_name
        ),
    ))
}

#[derive(Default)]
struct FieldHints {
    skip: bool,
    enumstr: bool,
    mapped_enum: bool,
    validated_string: bool,
    strlist: bool,
    scalarlist: bool,
    enumlist: bool,
    mapped_enumlist: bool,
    clone_list: bool,
    clone_nested: bool,
    required_scalar: bool,
    name: Option<String>,
    list: Option<Ident>,
    nested: Option<Ident>,
    required_nested: Option<Ident>,
    data_enum: Option<Ident>,
    choice: Option<Ident>,
    arr: Option<usize>,
    name_field: NameField,
}

fn parse_field_hints(f: &syn::Field) -> syn::Result<FieldHints> {
    let mut h = FieldHints::default();
    for a in &f.attrs {
        if !a.path().is_ident("pydom") {
            continue;
        }
        a.parse_nested_meta(|m| {
            let key = m
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            match key.as_str() {
                "skip" => h.skip = true,
                "enumstr" => h.enumstr = true,
                "mapped_enum" => h.mapped_enum = true,
                "validated_string" => h.validated_string = true,
                "strlist" => h.strlist = true,
                "scalarlist" => h.scalarlist = true,
                "enumlist" => h.enumlist = true,
                "mapped_enumlist" => h.mapped_enumlist = true,
                "clone_list" => h.clone_list = true,
                "clone_nested" => h.clone_nested = true,
                "required_scalar" => h.required_scalar = true,
                "name" => h.name = Some(m.value()?.parse::<LitStr>()?.value()),
                "list" => h.list = Some(m.value()?.parse::<Ident>()?),
                "nested" => h.nested = Some(m.value()?.parse::<Ident>()?),
                "required_nested" => h.required_nested = Some(m.value()?.parse::<Ident>()?),
                "data_enum" => h.data_enum = Some(m.value()?.parse::<Ident>()?),
                "choice" => h.choice = Some(m.value()?.parse::<Ident>()?),
                "arr" => h.arr = Some(m.value()?.parse::<syn::LitInt>()?.base10_parse()?),
                "name_field" => {
                    let v = m.value()?.parse::<Ident>()?;
                    h.name_field = match v.to_string().as_str() {
                        "req" => NameField::Req,
                        "opt" => NameField::Opt,
                        "none" => NameField::None,
                        _ => return Err(m.error("name_field = req|opt|none")),
                    };
                }
                _ => return Err(m.error("unknown pydom field hint")),
            }
            Ok(())
        })?;
    }
    Ok(h)
}

fn classify_field(rust: &Ident, ty: &Type, h: &FieldHints) -> syn::Result<FieldKind> {
    if let Some(child) = &h.list {
        return Ok(FieldKind::List {
            child: child.clone(),
            name_field: h.name_field,
            clone_append: h.clone_list,
        });
    }
    if let Some(child) = &h.nested {
        return Ok(FieldKind::Nested {
            child: child.clone(),
            clone_set: h.clone_nested,
        });
    }
    if let Some(child) = &h.required_nested {
        return Ok(FieldKind::RequiredNested {
            child: child.clone(),
        });
    }
    if let Some(w) = &h.data_enum {
        return Ok(FieldKind::DataEnum { wrapper: w.clone() });
    }
    if let Some(wrapper) = &h.choice {
        return Ok(FieldKind::Choice {
            wrapper: wrapper.clone(),
            opt: outer_ident(ty).as_deref() == Some("Option"),
        });
    }
    if let Some(n) = h.arr {
        return Ok(if outer_ident(ty).as_deref() == Some("Option") {
            FieldKind::OptArr(n)
        } else {
            FieldKind::ReqArr(n)
        });
    }
    if h.enumstr {
        return Ok(FieldKind::EnumScalar {
            opt: outer_ident(ty).as_deref() == Some("Option"),
        });
    }
    if h.mapped_enum {
        return Ok(FieldKind::MappedEnumScalar {
            opt: outer_ident(ty).as_deref() == Some("Option"),
        });
    }
    if h.validated_string {
        let opt = outer_ident(ty).as_deref() == Some("Option");
        let value_type = if opt {
            inner_of(ty, "Option").ok_or_else(|| {
                syn::Error::new_spanned(ty, "validated_string requires T or Option<T>")
            })?
        } else {
            ty.clone()
        };
        let inner = outer_ident(&value_type).ok_or_else(|| {
            syn::Error::new_spanned(ty, "validated_string requires a named value type")
        })?;
        return Ok(FieldKind::ValidatedString {
            inner: Ident::new(&inner, Span::call_site()),
            opt,
        });
    }
    if h.strlist || is_vec_of(ty, "String") {
        return Ok(FieldKind::StrList);
    }
    if h.scalarlist {
        let inner = inner_of(ty, "Vec")
            .and_then(|inner| outer_ident(&inner))
            .ok_or_else(|| syn::Error::new_spanned(ty, "scalarlist requires Vec<scalar>"))?;
        return Ok(FieldKind::ScalarList(Ident::new(&inner, Span::call_site())));
    }
    if h.enumlist || h.mapped_enumlist {
        let inner = inner_of(ty, "Vec")
            .and_then(|inner| outer_ident(&inner))
            .ok_or_else(|| syn::Error::new_spanned(ty, "enumlist requires Vec<enum>"))?;
        return Ok(FieldKind::EnumList {
            inner: Ident::new(&inner, Span::call_site()),
            mapped: h.mapped_enumlist,
        });
    }
    if h.required_scalar {
        let scalar = outer_ident(ty)
            .ok_or_else(|| syn::Error::new_spanned(ty, "required_scalar requires a scalar type"))?;
        return Ok(FieldKind::ReqScalar(Ident::new(&scalar, Span::call_site())));
    }
    // Fall back to shape inference.
    match outer_ident(ty).as_deref() {
        Some("String") => Ok(FieldKind::ReqStr),
        Some("Option") => {
            let leaf = inner_leaf(ty).unwrap_or_default();
            match leaf.as_str() {
                "String" => Ok(FieldKind::OptStr),
                "f64" | "f32" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16"
                | "u32" | "u64" | "u128" | "usize" | "bool" => {
                    Ok(FieldKind::OptScalar(Ident::new(&leaf, Span::call_site())))
                }
                _ => Err(syn::Error::new_spanned(
                    ty,
                    format!("field `{rust}`: Option<{leaf}> needs a hint (nested/enumstr/arr)"),
                )),
            }
        }
        _ => Err(syn::Error::new_spanned(
            ty,
            format!("field `{rust}`: unsupported shape; add a pydom hint"),
        )),
    }
}

fn parse_choice_variants(choice: &ItemEnum) -> syn::Result<Vec<ChoiceVariantSpec>> {
    let mut variants = Vec::new();
    for variant in &choice.variants {
        let mut label = None;
        let mut payload_handle = None;
        let mut draft = None;
        let mut enumstr = false;
        let mut mapped_enum = false;
        for attribute in &variant.attrs {
            if !attribute.path().is_ident("pychoice") {
                continue;
            }
            attribute.parse_nested_meta(|meta| {
                let key = meta
                    .path
                    .get_ident()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                match key.as_str() {
                    "name" => label = Some(meta.value()?.parse::<LitStr>()?.value()),
                    "payload" => payload_handle = Some(meta.value()?.parse::<Ident>()?),
                    "draft" => {
                        if draft.is_some() {
                            return Err(meta.error("duplicate draft constructor"));
                        }
                        draft = Some(meta.value()?.parse::<Expr>()?);
                    }
                    "enumstr" => enumstr = true,
                    "mapped_enum" => mapped_enum = true,
                    _ => return Err(meta.error("unknown pychoice variant hint")),
                }
                Ok(())
            })?;
        }

        let py_name = snake(&variant.ident);
        let label = label.unwrap_or_else(|| py_name.clone());
        let payload = match &variant.fields {
            syn::Fields::Unit => {
                if payload_handle.is_some() || draft.is_some() || enumstr || mapped_enum {
                    return Err(syn::Error::new_spanned(
                        variant,
                        "unit choice arms cannot have payload, draft, or enum hints",
                    ));
                }
                ChoicePayload::Unit
            }
            syn::Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let ty = fields.unnamed.first().unwrap().ty.clone();
                if let Some(child) = payload_handle {
                    if enumstr || mapped_enum {
                        return Err(syn::Error::new_spanned(
                            variant,
                            "a choice payload cannot combine a struct handle with an enum hint",
                        ));
                    }
                    let draft = draft.ok_or_else(|| {
                        syn::Error::new_spanned(
                            variant,
                            "struct choice payload needs a `draft = constructor` hint",
                        )
                    })?;
                    ChoicePayload::Struct { ty, child, draft }
                } else if enumstr && mapped_enum {
                    return Err(syn::Error::new_spanned(
                        variant,
                        "a choice payload cannot be both enumstr and mapped_enum",
                    ));
                } else if enumstr {
                    if draft.is_some() {
                        return Err(syn::Error::new_spanned(
                            variant,
                            "enum choice payload cannot have a draft constructor",
                        ));
                    }
                    ChoicePayload::Enum(ty)
                } else if mapped_enum {
                    if draft.is_some() {
                        return Err(syn::Error::new_spanned(
                            variant,
                            "mapped enum choice payload cannot have a draft constructor",
                        ));
                    }
                    ChoicePayload::MappedEnum(ty)
                } else {
                    if draft.is_some() {
                        return Err(syn::Error::new_spanned(
                            variant,
                            "draft constructor requires a struct payload handle",
                        ));
                    }
                    match outer_ident(&ty).as_deref() {
                        Some("String") => ChoicePayload::String(ty),
                        Some(
                            "f64" | "f32" | "i8" | "i16" | "i32" | "i64" | "i128"
                            | "isize" | "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
                            | "bool",
                        ) => ChoicePayload::Scalar(ty),
                        _ => {
                            return Err(syn::Error::new_spanned(
                                variant,
                                "tuple choice payload needs `payload = PyType`, `enumstr`, or `mapped_enum`",
                            ))
                        }
                    }
                }
            }
            syn::Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "choice tuple arms must contain exactly one payload",
                ))
            }
            syn::Fields::Named(_) => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "named-field choice arms require an explicit companion",
                ))
            }
        };
        variants.push(ChoiceVariantSpec {
            rust: variant.ident.clone(),
            py_name,
            label,
            payload,
        });
    }
    if variants.is_empty() {
        return Err(syn::Error::new_spanned(
            choice,
            "pychoice needs at least one arm",
        ));
    }
    validate_choice_public_namespace(&variants)?;
    Ok(variants)
}

fn validate_choice_public_namespace(variants: &[ChoiceVariantSpec]) -> syn::Result<()> {
    let mut members = std::collections::BTreeMap::from([
        (
            "variant".to_owned(),
            "the active-variant property".to_owned(),
        ),
        (
            "__repr__".to_owned(),
            "the representation method".to_owned(),
        ),
    ]);
    let mut labels = std::collections::BTreeMap::new();

    for variant in variants {
        let arm = format!("choice arm `{}`", variant.rust);
        let member_origins = [
            (variant.py_name.clone(), format!("{arm} property")),
            (
                format!("select_{}", variant.py_name),
                format!("{arm} selector"),
            ),
        ];
        for (name, origin) in member_origins {
            if let Some(previous) = members.insert(name.clone(), origin.clone()) {
                return Err(syn::Error::new(
                    variant.rust.span(),
                    format!(
                        "public Python choice member {name:?} from {origin} conflicts with {previous}"
                    ),
                ));
            }
        }
        if matches!(variant.payload, ChoicePayload::Struct { .. }) {
            let name = format!("set_{}_from", variant.py_name);
            let origin = format!("{arm} clone setter");
            if let Some(previous) = members.insert(name.clone(), origin.clone()) {
                return Err(syn::Error::new(
                    variant.rust.span(),
                    format!(
                        "public Python choice member {name:?} from {origin} conflicts with {previous}"
                    ),
                ));
            }
        }

        let label_origin = format!("{arm} public label");
        if let Some(previous) = labels.insert(variant.label.clone(), label_origin.clone()) {
            return Err(syn::Error::new(
                variant.rust.span(),
                format!(
                    "public Python choice label {:?} from {label_origin} conflicts with {previous}",
                    variant.label
                ),
            ));
        }
    }

    Ok(())
}

// ── code generation ──────────────────────────────────────────────────────────────────────────────

fn gen_choice_all(c: &Container, variants: &[ChoiceVariantSpec]) -> proc_macro2::TokenStream {
    let loc = format_ident!("{}Loc", c.py);
    let loc_def = gen_loc(c, &loc);
    let resolve = gen_resolve(c, &loc);
    let handle = gen_handle(c);
    let withs = gen_with(c);
    let replace = gen_choice_replace(c);
    let converter = gen_choice_converter(c, variants);
    let methods = variants
        .iter()
        .enumerate()
        .map(|(index, variant)| gen_choice_arm(c, variants, index, variant));
    let variant_getter = gen_choice_variant_getter(c, variants);
    let py = &c.py;
    let py_lit = LitStr::new(&py.to_string(), Span::call_site());
    let register = gen_choice_register(c);
    let pyi = gen_choice_pyi(c, variants);

    quote! {
        #loc_def
        #resolve
        #handle
        #withs
        #replace
        #converter

        #[pyo3::pymethods]
        impl #py {
            #variant_getter
            #(#methods)*
            fn __repr__(&self) -> String {
                format!("<{}>", #py_lit)
            }
        }

        #register
        #pyi
    }
}

fn gen_choice_replace(c: &Container) -> proc_macro2::TokenStream {
    let py = &c.py;
    let target = &c.target;
    let owner_target = &c.owner_target;
    let loc = format_ident!("{}Loc", c.py);
    let py_name = LitStr::new(&py.to_string(), Span::call_site());
    let arms = c.sites.iter().map(|site| {
        let variant = site.variant();
        let parent = &site.ptype;
        match &site.access {
            SiteAccess::Field {
                field,
                card: Card::Req,
            } => quote! {
                #loc::#variant(parent_loc) => {
                    let parent = <#parent as Resolve<#owner_target>>::resolve_mut(&mut doc, &**parent_loc)
                        .ok_or_else(|| stale(#py_name))?;
                    parent.#field = selected;
                }
            },
            SiteAccess::Field {
                field,
                card: Card::Opt,
            } => quote! {
                #loc::#variant(parent_loc) => {
                    let parent = <#parent as Resolve<#owner_target>>::resolve_mut(&mut doc, &**parent_loc)
                        .ok_or_else(|| stale(#py_name))?;
                    parent.#field = Some(selected);
                }
            },
            SiteAccess::Field {
                field,
                card: Card::Vec,
            } => quote! {
                #loc::#variant(parent_loc, index) => {
                    let parent = <#parent as Resolve<#owner_target>>::resolve_mut(&mut doc, &**parent_loc)
                        .ok_or_else(|| stale(#py_name))?;
                    let slot = parent.#field.get_mut(*index).ok_or_else(|| stale(#py_name))?;
                    *slot = selected;
                }
            },
            SiteAccess::Choice {
                variant: parent_variant,
            } => quote! {
                #loc::#variant(parent_loc) => {
                    let parent = <#parent as Resolve<#owner_target>>::resolve_mut(&mut doc, &**parent_loc)
                        .ok_or_else(|| stale(#py_name))?;
                    match parent {
                        #parent::#parent_variant(payload) => *payload = selected,
                        _ => return Err(stale(#py_name)),
                    }
                }
            },
        }
    });
    quote! {
        impl #py {
            fn replace(&self, py: pyo3::Python<'_>, selected: #target) -> pyo3::PyResult<()> {
                let owner = self.owner.borrow(py);
                let mut doc = owner.doc.borrow_mut();
                match &self.loc {
                    #loc::Nowhere => return Err(stale(#py_name)),
                    #(#arms)*
                }
                Ok(())
            }
        }
    }
}

fn gen_choice_variant_getter(
    c: &Container,
    variants: &[ChoiceVariantSpec],
) -> proc_macro2::TokenStream {
    let target = &c.target;
    let arms = variants.iter().map(|variant| {
        let rust = &variant.rust;
        let label = LitStr::new(&variant.label, Span::call_site());
        match variant.payload {
            ChoicePayload::Unit => quote! { #target::#rust => #label },
            _ => quote! { #target::#rust(..) => #label },
        }
    });
    quote! {
        #[getter]
        fn variant(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<String> {
            self.with(py, |choice| Ok(match choice { #(#arms),* }.to_owned()))
        }
    }
}

fn gen_choice_converter(c: &Container, variants: &[ChoiceVariantSpec]) -> proc_macro2::TokenStream {
    let py = &c.py;
    let target = &c.target;
    let target_name = LitStr::new(&ty_ident(target).to_string(), Span::call_site());
    let params = variants.iter().map(|variant| {
        let arg = choice_arg_ident(variant);
        match &variant.payload {
            ChoicePayload::Unit => quote! { #arg: bool },
            ChoicePayload::Struct { ty, .. }
            | ChoicePayload::String(ty)
            | ChoicePayload::Scalar(ty)
            | ChoicePayload::Enum(ty)
            | ChoicePayload::MappedEnum(ty) => quote! { #arg: Option<#ty> },
        }
    });
    let counts = variants.iter().map(|variant| {
        let arg = choice_arg_ident(variant);
        match variant.payload {
            ChoicePayload::Unit => quote! { usize::from(#arg) },
            _ => quote! { usize::from(#arg.is_some()) },
        }
    });
    let selections = variants.iter().map(|variant| {
        let rust = &variant.rust;
        let arg = choice_arg_ident(variant);
        match variant.payload {
            ChoicePayload::Unit => quote! {
                if #arg { return Ok(#target::#rust); }
            },
            _ => quote! {
                if let Some(payload) = #arg { return Ok(#target::#rust(payload)); }
            },
        }
    });
    quote! {
        impl #py {
            #[doc(hidden)]
            pub(crate) fn __from_arms(#(#params),*) -> Result<#target, String> {
                let selected = 0usize #(+ #counts)*;
                if selected != 1 {
                    return Err(format!(
                        "{} requires exactly one selected arm, got {}",
                        #target_name,
                        selected
                    ));
                }
                #(#selections)*
                Err(format!("{} choice selection was inconsistent", #target_name))
            }
        }
    }
}

fn gen_choice_arm(
    c: &Container,
    variants: &[ChoiceVariantSpec],
    selected_index: usize,
    variant: &ChoiceVariantSpec,
) -> proc_macro2::TokenStream {
    let py = &c.py;
    let target = &c.target;
    let rust = &variant.rust;
    let getter = format_ident!("get_{}", variant.py_name);
    let selector = format_ident!("select_{}", variant.py_name);
    let py_name = LitStr::new(&variant.py_name, Span::call_site());

    match &variant.payload {
        ChoicePayload::Unit => {
            let args = choice_converter_call_args(variants, selected_index, quote! { true });
            quote! {
                #[getter] #[pyo3(name = #py_name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<bool> {
                    self.with(py, |choice| Ok(matches!(choice, #target::#rust)))
                }
                fn #selector(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<()> {
                    let selected = #py::__from_arms(#(#args),*)
                        .map_err(pyo3::exceptions::PyValueError::new_err)?;
                    self.replace(py, selected)
                }
            }
        }
        ChoicePayload::Struct { child, draft, .. } => {
            let child_loc = format_ident!("{}Loc", child);
            let loc_variant = choice_variant_ident(&ty_ident(target), rust);
            let args = choice_converter_call_args(variants, selected_index, quote! { payload });
            let set_from = format_ident!("set_{}_from", variant.py_name);
            let active_handle = quote! {
                #child {
                    owner: self.owner.clone_ref(py),
                    loc: #child_loc::#loc_variant(std::boxed::Box::new(self.loc.clone())),
                }
            };
            quote! {
                #[getter] #[pyo3(name = #py_name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#child>> {
                    let active = self.with(py, |choice| Ok(matches!(choice, #target::#rust(..))))?;
                    if !active { return Ok(None); }
                    Ok(Some(#active_handle))
                }
                fn #selector(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<#child> {
                    let payload = (#draft)();
                    let selected = #py::__from_arms(#(#args),*)
                        .map_err(pyo3::exceptions::PyValueError::new_err)?;
                    self.replace(py, selected)?;
                    Ok(#active_handle)
                }
                fn #set_from(
                    &self,
                    py: pyo3::Python<'_>,
                    value: pyo3::PyRef<'_, #child>,
                ) -> pyo3::PyResult<#child> {
                    let payload = value.with(py, |payload| Ok(payload.clone()))?;
                    let selected = #py::__from_arms(#(#args),*)
                        .map_err(pyo3::exceptions::PyValueError::new_err)?;
                    self.replace(py, selected)?;
                    Ok(#active_handle)
                }
            }
        }
        ChoicePayload::String(_) => {
            let args = choice_converter_call_args(variants, selected_index, quote! { value });
            quote! {
                #[getter] #[pyo3(name = #py_name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<String>> {
                    self.with(py, |choice| Ok(match choice {
                        #target::#rust(value) => Some(value.clone()),
                        _ => None,
                    }))
                }
                fn #selector(&self, py: pyo3::Python<'_>, value: String) -> pyo3::PyResult<()> {
                    let selected = #py::__from_arms(#(#args),*)
                        .map_err(pyo3::exceptions::PyValueError::new_err)?;
                    self.replace(py, selected)
                }
            }
        }
        ChoicePayload::Scalar(ty) => {
            let args = choice_converter_call_args(variants, selected_index, quote! { value });
            quote! {
                #[getter] #[pyo3(name = #py_name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#ty>> {
                    self.with(py, |choice| Ok(match choice {
                        #target::#rust(value) => Some(*value),
                        _ => None,
                    }))
                }
                fn #selector(&self, py: pyo3::Python<'_>, value: #ty) -> pyo3::PyResult<()> {
                    let selected = #py::__from_arms(#(#args),*)
                        .map_err(pyo3::exceptions::PyValueError::new_err)?;
                    self.replace(py, selected)
                }
            }
        }
        ChoicePayload::Enum(ty) => {
            let args = choice_converter_call_args(variants, selected_index, quote! { parsed });
            quote! {
                #[getter] #[pyo3(name = #py_name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<String>> {
                    self.with(py, |choice| Ok(match choice {
                        #target::#rust(value) => Some(value.to_string()),
                        _ => None,
                    }))
                }
                fn #selector(&self, py: pyo3::Python<'_>, value: String) -> pyo3::PyResult<()> {
                    let parsed: #ty = value.parse().map_err(|_| {
                        pyo3::exceptions::PyValueError::new_err(format!(
                            "invalid enum choice value: {:?}", value
                        ))
                    })?;
                    let selected = #py::__from_arms(#(#args),*)
                        .map_err(pyo3::exceptions::PyValueError::new_err)?;
                    self.replace(py, selected)
                }
            }
        }
        ChoicePayload::MappedEnum(ty) => {
            let args = choice_converter_call_args(variants, selected_index, quote! { parsed });
            quote! {
                #[getter] #[pyo3(name = #py_name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<String>> {
                    self.with(py, |choice| Ok(match choice {
                        #target::#rust(value) => Some(value.dom_value().to_owned()),
                        _ => None,
                    }))
                }
                fn #selector(&self, py: pyo3::Python<'_>, value: String) -> pyo3::PyResult<()> {
                    let parsed: #ty = <#ty as DomEnum>::from_dom_value(&value).ok_or_else(|| {
                        pyo3::exceptions::PyValueError::new_err(format!(
                            "invalid enum choice value: {:?}", value
                        ))
                    })?;
                    let selected = #py::__from_arms(#(#args),*)
                        .map_err(pyo3::exceptions::PyValueError::new_err)?;
                    self.replace(py, selected)
                }
            }
        }
    }
}

fn choice_arg_ident(variant: &ChoiceVariantSpec) -> Ident {
    format_ident!("arm_{}", variant.py_name)
}

fn choice_converter_call_args(
    variants: &[ChoiceVariantSpec],
    selected_index: usize,
    selected: proc_macro2::TokenStream,
) -> Vec<proc_macro2::TokenStream> {
    variants
        .iter()
        .enumerate()
        .map(|(index, variant)| {
            if index == selected_index {
                match variant.payload {
                    ChoicePayload::Unit => selected.clone(),
                    _ => quote! { Some(#selected) },
                }
            } else {
                match variant.payload {
                    ChoicePayload::Unit => quote! { false },
                    _ => quote! { None },
                }
            }
        })
        .collect()
}

fn gen_choice_register(c: &Container) -> proc_macro2::TokenStream {
    let py = &c.py;
    let register = format_ident!("__register_{}", snake(py));
    quote! {
        pub fn #register(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
            m.add_class::<#py>()?;
            Ok(())
        }
    }
}

fn gen_choice_pyi(c: &Container, variants: &[ChoiceVariantSpec]) -> proc_macro2::TokenStream {
    let py = &c.py;
    let pyi_fn = format_ident!("__pyi_{}", snake(py));
    let mut lines = format!("class {py}:\n    variant: str\n");
    for variant in variants {
        let name = &variant.py_name;
        let annotation = match &variant.payload {
            ChoicePayload::Unit => "bool".to_owned(),
            ChoicePayload::Struct { child, .. } => format!("Optional[\"{child}\"]"),
            ChoicePayload::String(_) | ChoicePayload::Enum(_) | ChoicePayload::MappedEnum(_) => {
                "Optional[str]".to_owned()
            }
            ChoicePayload::Scalar(ty) => format!("Optional[{}]", python_scalar(ty)),
        };
        lines.push_str(&format!("    {name}: {annotation}\n"));
        let selector_arg = match &variant.payload {
            ChoicePayload::Unit => None,
            ChoicePayload::Struct { .. } => None,
            ChoicePayload::String(_) | ChoicePayload::Enum(_) | ChoicePayload::MappedEnum(_) => {
                Some("value: str".to_owned())
            }
            ChoicePayload::Scalar(ty) => Some(format!("value: {}", python_scalar(ty))),
        };
        match selector_arg {
            Some(argument) => {
                lines.push_str(&format!(
                    "    def select_{name}(self, {argument}) -> None: ...\n"
                ));
            }
            None => match &variant.payload {
                ChoicePayload::Struct { child, .. } => {
                    lines.push_str(&format!(
                        "    def select_{name}(self) -> \"{child}\": ...\n"
                    ));
                    lines.push_str(&format!(
                        "    def set_{name}_from(self, value: \"{child}\") -> \"{child}\": ...\n"
                    ));
                }
                _ => lines.push_str(&format!("    def select_{name}(self) -> None: ...\n")),
            },
        }
    }
    lines.push('\n');
    let literal = LitStr::new(&lines, Span::call_site());
    quote! {
        pub fn #pyi_fn() -> &'static str { #literal }
    }
}

fn python_scalar(ty: &Type) -> &'static str {
    match outer_ident(ty).as_deref() {
        Some("bool") => "bool",
        Some("f64" | "f32") => "float",
        _ => "int",
    }
}

fn gen_all(c: &Container, fields: &[FieldSpec]) -> proc_macro2::TokenStream {
    let loc = format_ident!("{}Loc", c.py);
    let loc_def = gen_loc(c, &loc);
    let resolve = gen_resolve(c, &loc);
    let handle = gen_handle(c);
    let withs = gen_with(c);

    let mut methods = Vec::new();
    let mut side_classes = Vec::new();
    let mut list_class_idents = Vec::new();
    for f in fields {
        let (m, side, list_ids) = gen_field(c, f);
        methods.push(m);
        side_classes.push(side);
        list_class_idents.extend(list_ids);
    }
    let root_methods = gen_root_methods(c);
    let py_name = &c.py;
    let py_lit = LitStr::new(&c.py.to_string(), Span::call_site());

    let register = gen_register(c, &list_class_idents);
    let pyi = gen_pyi(c, fields);

    quote! {
        #loc_def
        #resolve
        #handle
        #withs

        #[pyo3::pymethods]
        impl #py_name {
            #root_methods
            #(#methods)*
            fn __repr__(&self) -> String {
                format!("<{}>", #py_lit)
            }
        }

        #(#side_classes)*
        #register
        #pyi
    }
}

fn gen_loc(c: &Container, loc: &Ident) -> proc_macro2::TokenStream {
    if c.is_root {
        return quote! {
            #[derive(Clone, Default)]
            pub struct #loc;
        };
    }
    let variants = c.sites.iter().map(|s| {
        let v = s.variant();
        let pl = s.parent_loc();
        match &s.access {
            SiteAccess::Field {
                card: Card::Vec, ..
            } => quote! { #v(std::boxed::Box<#pl>, usize) },
            SiteAccess::Field {
                card: Card::Opt | Card::Req,
                ..
            }
            | SiteAccess::Choice { .. } => quote! { #v(std::boxed::Box<#pl>) },
        }
    });
    // A fixed sentinel variant so a locator enum's variants never ALL share a first/last camel word
    // (`clippy::enum_variant_names`, which would otherwise fire whenever every parent shares a family
    // prefix like `Sensor`, or every site uses the same field name like `pose`/`geometry`). It resolves
    // to `None` (a locator into nothing). It is the `#[default]`, so the derived `Default` constructs it,
    // keeping the variant off the `dead_code` (never-constructed) radar.
    quote! {
        #[derive(Clone, Default)]
        pub enum #loc {
            #[default]
            Nowhere,
            #(#variants),*
        }
    }
}

fn gen_resolve(c: &Container, loc: &Ident) -> proc_macro2::TokenStream {
    let target = &c.target;
    let owner_target = &c.owner_target;
    if c.is_root {
        return quote! {
            impl Resolve<#owner_target> for #target {
                type Loc = #loc;
                fn resolve<'a>(root: &'a #owner_target, _loc: &#loc) -> Option<&'a Self> { Some(root) }
                fn resolve_mut<'a>(root: &'a mut #owner_target, _loc: &#loc) -> Option<&'a mut Self> { Some(root) }
            }
        };
    }
    let arms = c.sites.iter().map(|s| {
        let v = s.variant();
        let ptype = &s.ptype;
        match &s.access {
            SiteAccess::Field {
                field,
                card: Card::Vec,
            } => quote! {
                #loc::#v(p, i) => <#ptype as Resolve<#owner_target>>::resolve(root, &**p).and_then(|x| x.#field.get(*i)),
            },
            SiteAccess::Field {
                field,
                card: Card::Opt,
            } => quote! {
                #loc::#v(p) => <#ptype as Resolve<#owner_target>>::resolve(root, &**p).and_then(|x| x.#field.as_ref()),
            },
            SiteAccess::Field {
                field,
                card: Card::Req,
            } => quote! {
                #loc::#v(p) => <#ptype as Resolve<#owner_target>>::resolve(root, &**p).map(|x| &x.#field),
            },
            SiteAccess::Choice { variant } => quote! {
                #loc::#v(p) => <#ptype as Resolve<#owner_target>>::resolve(root, &**p).and_then(|x| match x {
                    #ptype::#variant(payload) => Some(payload),
                    _ => None,
                }),
            },
        }
    });
    let arms_mut = c.sites.iter().map(|s| {
        let v = s.variant();
        let ptype = &s.ptype;
        match &s.access {
            SiteAccess::Field {
                field,
                card: Card::Vec,
            } => quote! {
                #loc::#v(p, i) => <#ptype as Resolve<#owner_target>>::resolve_mut(root, &**p).and_then(|x| x.#field.get_mut(*i)),
            },
            SiteAccess::Field {
                field,
                card: Card::Opt,
            } => quote! {
                #loc::#v(p) => <#ptype as Resolve<#owner_target>>::resolve_mut(root, &**p).and_then(|x| x.#field.as_mut()),
            },
            SiteAccess::Field {
                field,
                card: Card::Req,
            } => quote! {
                #loc::#v(p) => <#ptype as Resolve<#owner_target>>::resolve_mut(root, &**p).map(|x| &mut x.#field),
            },
            SiteAccess::Choice { variant } => quote! {
                #loc::#v(p) => <#ptype as Resolve<#owner_target>>::resolve_mut(root, &**p).and_then(|x| match x {
                    #ptype::#variant(payload) => Some(payload),
                    _ => None,
                }),
            },
        }
    });
    quote! {
        impl Resolve<#owner_target> for #target {
            type Loc = #loc;
            fn resolve<'a>(root: &'a #owner_target, loc: &#loc) -> Option<&'a Self> {
                match loc { #loc::Nowhere => None, #(#arms)* }
            }
            fn resolve_mut<'a>(root: &'a mut #owner_target, loc: &#loc) -> Option<&'a mut Self> {
                match loc { #loc::Nowhere => None, #(#arms_mut)* }
            }
        }
    }
}

fn gen_handle(c: &Container) -> proc_macro2::TokenStream {
    let py = &c.py;
    let owner_py = &c.owner_py;
    let owner_target = &c.owner_target;
    let loc = format_ident!("{}Loc", c.py);
    let py_lit = LitStr::new(&py.to_string(), Span::call_site());
    if c.is_root {
        quote! {
            #[pyo3::pyclass(unsendable, name = #py_lit)]
            pub struct #py { pub(crate) doc: std::rc::Rc<std::cell::RefCell<#owner_target>> }
        }
    } else {
        quote! {
            #[pyo3::pyclass(unsendable, name = #py_lit)]
            pub struct #py { pub(crate) owner: pyo3::Py<#owner_py>, pub(crate) loc: #loc }
        }
    }
}

fn gen_with(c: &Container) -> proc_macro2::TokenStream {
    let py = &c.py;
    let target = &c.target;
    let owner_target = &c.owner_target;
    let py_lit = LitStr::new(&py.to_string(), Span::call_site());
    if c.is_root {
        quote! {
            impl #py {
                pub(crate) fn with<R>(&self, _py: pyo3::Python<'_>, f: impl FnOnce(&#owner_target) -> pyo3::PyResult<R>) -> pyo3::PyResult<R> {
                    f(&self.doc.borrow())
                }
                pub(crate) fn with_mut<R>(&self, _py: pyo3::Python<'_>, f: impl FnOnce(&mut #owner_target) -> pyo3::PyResult<R>) -> pyo3::PyResult<R> {
                    f(&mut self.doc.borrow_mut())
                }
            }
        }
    } else {
        quote! {
            impl #py {
                pub(crate) fn with<R>(&self, py: pyo3::Python<'_>, f: impl FnOnce(&#target) -> pyo3::PyResult<R>) -> pyo3::PyResult<R> {
                    let owner = self.owner.borrow(py);
                    let doc = owner.doc.borrow();
                    let t = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#py_lit))?;
                    f(t)
                }
                pub(crate) fn with_mut<R>(&self, py: pyo3::Python<'_>, f: impl FnOnce(&mut #target) -> pyo3::PyResult<R>) -> pyo3::PyResult<R> {
                    let owner = self.owner.borrow(py);
                    let mut doc = owner.doc.borrow_mut();
                    let t = <#target as Resolve<#owner_target>>::resolve_mut(&mut doc, &self.loc).ok_or_else(|| stale(#py_lit))?;
                    f(t)
                }
            }
        }
    }
}

fn gen_root_methods(c: &Container) -> proc_macro2::TokenStream {
    if !c.is_root {
        return quote! {};
    }
    let target = &c.target;
    quote! {
        #[staticmethod]
        fn loads(xml: &str) -> pyo3::PyResult<Self> {
            let doc = #target::from_xml_str(xml).map_err(err)?;
            Ok(Self { doc: std::rc::Rc::new(std::cell::RefCell::new(doc)) })
        }
        #[staticmethod]
        fn load(path: &str) -> pyo3::PyResult<Self> {
            let s = std::fs::read_to_string(path).map_err(err)?;
            Self::loads(&s)
        }
        fn dumps(&self) -> pyo3::PyResult<String> {
            self.doc.borrow().to_xml_string().map_err(err)
        }
        fn dump(&self, path: &str) -> pyo3::PyResult<()> {
            let s = self.dumps()?;
            std::fs::write(path, s).map_err(err)
        }
    }
}

/// The parent-locator VALUE for constructing a child handle from `c`.
fn parent_loc_value(c: &Container) -> proc_macro2::TokenStream {
    if c.is_root {
        let owner_loc = format_ident!("{}Loc", c.owner_py);
        quote! { #owner_loc }
    } else {
        quote! { self.loc.clone() }
    }
}

/// Emit one field's accessors, its side classes (list/strlist), and the list-class idents to register.
fn gen_field(
    c: &Container,
    f: &FieldSpec,
) -> (
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    Vec<Ident>,
) {
    let rust = &f.rust;
    let name = LitStr::new(&f.py_name, Span::call_site());
    let getter = format_ident!("get_{}", rust);
    let setter = format_ident!("set_{}", rust);
    match &f.kind {
        FieldKind::ReqStr => (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<String> {
                    self.with(py, |t| Ok(t.#rust.clone()))
                }
                #[setter] #[pyo3(name = #name)]
                fn #setter(&self, py: pyo3::Python<'_>, val: String) -> pyo3::PyResult<()> {
                    self.with_mut(py, |t| { t.#rust = val; Ok(()) })
                }
            },
            quote! {},
            vec![],
        ),
        FieldKind::ReqScalar(inner) => (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<#inner> {
                    self.with(py, |t| Ok(t.#rust))
                }
                #[setter] #[pyo3(name = #name)]
                fn #setter(&self, py: pyo3::Python<'_>, val: #inner) -> pyo3::PyResult<()> {
                    self.with_mut(py, |t| { t.#rust = val; Ok(()) })
                }
            },
            quote! {},
            vec![],
        ),
        FieldKind::OptStr => (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<String>> {
                    self.with(py, |t| Ok(t.#rust.clone()))
                }
                #[setter] #[pyo3(name = #name)]
                fn #setter(&self, py: pyo3::Python<'_>, val: Option<String>) -> pyo3::PyResult<()> {
                    self.with_mut(py, |t| { t.#rust = val; Ok(()) })
                }
            },
            quote! {},
            vec![],
        ),
        FieldKind::OptScalar(inner) => (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#inner>> {
                    self.with(py, |t| Ok(t.#rust))
                }
                #[setter] #[pyo3(name = #name)]
                fn #setter(&self, py: pyo3::Python<'_>, val: Option<#inner>) -> pyo3::PyResult<()> {
                    self.with_mut(py, |t| { t.#rust = val; Ok(()) })
                }
            },
            quote! {},
            vec![],
        ),
        FieldKind::ReqArr(n) => {
            let n = *n;
            (
                quote! {
                    #[getter] #[pyo3(name = #name)]
                    fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Vec<f64>> {
                        self.with(py, |target| Ok(target.#rust.to_vec()))
                    }
                    #[setter] #[pyo3(name = #name)]
                    fn #setter(&self, py: pyo3::Python<'_>, value: Vec<f64>) -> pyo3::PyResult<()> {
                        let array: [f64; #n] = value.try_into().map_err(|value: Vec<f64>| {
                            pyo3::exceptions::PyValueError::new_err(format!(
                                "{} expects {} floats, got {}", #name, #n, value.len()
                            ))
                        })?;
                        self.with_mut(py, |target| { target.#rust = array; Ok(()) })
                    }
                },
                quote! {},
                vec![],
            )
        }
        FieldKind::OptArr(n) => {
            let n = *n;
            (
                quote! {
                    #[getter] #[pyo3(name = #name)]
                    fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<Vec<f64>>> {
                        self.with(py, |t| Ok(t.#rust.map(|a| a.to_vec())))
                    }
                    #[setter] #[pyo3(name = #name)]
                    fn #setter(&self, py: pyo3::Python<'_>, val: Option<Vec<f64>>) -> pyo3::PyResult<()> {
                        let arr = match val {
                            None => None,
                            Some(v) => {
                                if v.len() != #n {
                                    return Err(pyo3::exceptions::PyValueError::new_err(
                                        format!("{} expects {} floats, got {}", #name, #n, v.len())));
                                }
                                let mut a = [0.0f64; #n];
                                a.copy_from_slice(&v);
                                Some(a)
                            }
                        };
                        self.with_mut(py, |t| { t.#rust = arr; Ok(()) })
                    }
                },
                quote! {},
                vec![],
            )
        }
        FieldKind::EnumScalar { opt } => {
            if *opt {
                (
                    quote! {
                        #[getter] #[pyo3(name = #name)]
                        fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<String>> {
                            self.with(py, |t| Ok(t.#rust.as_ref().map(|v| v.to_string())))
                        }
                        #[setter] #[pyo3(name = #name)]
                        fn #setter(&self, py: pyo3::Python<'_>, val: Option<String>) -> pyo3::PyResult<()> {
                            self.with_mut(py, |t| {
                                t.#rust = match val {
                                    Some(s) => Some(s.parse().map_err(|_| {
                                        pyo3::exceptions::PyValueError::new_err(
                                            format!("invalid enum value for {}: {:?}", #name, s))
                                    })?),
                                    None => None,
                                };
                                Ok(())
                            })
                        }
                    },
                    quote! {},
                    vec![],
                )
            } else {
                (
                    quote! {
                        #[getter] #[pyo3(name = #name)]
                        fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<String> {
                            self.with(py, |t| Ok(t.#rust.to_string()))
                        }
                        #[setter] #[pyo3(name = #name)]
                        fn #setter(&self, py: pyo3::Python<'_>, val: String) -> pyo3::PyResult<()> {
                            self.with_mut(py, |t| {
                                t.#rust = val.parse().map_err(|_| {
                                    pyo3::exceptions::PyValueError::new_err(
                                        format!("invalid enum value for {}: {:?}", #name, val))
                                })?;
                                Ok(())
                            })
                        }
                    },
                    quote! {},
                    vec![],
                )
            }
        }
        FieldKind::MappedEnumScalar { opt } => {
            if *opt {
                (
                    quote! {
                        #[getter] #[pyo3(name = #name)]
                        fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<String>> {
                            self.with(py, |target| {
                                Ok(target.#rust.as_ref().map(|value| value.dom_value().to_owned()))
                            })
                        }
                        #[setter] #[pyo3(name = #name)]
                        fn #setter(&self, py: pyo3::Python<'_>, value: Option<String>) -> pyo3::PyResult<()> {
                            self.with_mut(py, |target| {
                                target.#rust = match value {
                                    Some(value) => Some(DomEnum::from_dom_value(&value).ok_or_else(|| {
                                        pyo3::exceptions::PyValueError::new_err(format!(
                                            "invalid enum value for {}: {:?}", #name, value
                                        ))
                                    })?),
                                    None => None,
                                };
                                Ok(())
                            })
                        }
                    },
                    quote! {},
                    vec![],
                )
            } else {
                (
                    quote! {
                        #[getter] #[pyo3(name = #name)]
                        fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<String> {
                            self.with(py, |target| Ok(target.#rust.dom_value().to_owned()))
                        }
                        #[setter] #[pyo3(name = #name)]
                        fn #setter(&self, py: pyo3::Python<'_>, value: String) -> pyo3::PyResult<()> {
                            self.with_mut(py, |target| {
                                target.#rust = DomEnum::from_dom_value(&value).ok_or_else(|| {
                                    pyo3::exceptions::PyValueError::new_err(format!(
                                        "invalid enum value for {}: {:?}", #name, value
                                    ))
                                })?;
                                Ok(())
                            })
                        }
                    },
                    quote! {},
                    vec![],
                )
            }
        }
        FieldKind::ValidatedString { inner, opt } => {
            if *opt {
                (
                    quote! {
                        #[getter] #[pyo3(name = #name)]
                        fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<String>> {
                            self.with(py, |target| {
                                Ok(target.#rust.as_ref().map(ToString::to_string))
                            })
                        }
                        #[setter] #[pyo3(name = #name)]
                        fn #setter(&self, py: pyo3::Python<'_>, value: Option<String>) -> pyo3::PyResult<()> {
                            let parsed = value.map(<#inner>::new).transpose().map_err(err)?;
                            self.with_mut(py, |target| {
                                target.#rust = parsed;
                                Ok(())
                            })
                        }
                    },
                    quote! {},
                    vec![],
                )
            } else {
                (
                    quote! {
                        #[getter] #[pyo3(name = #name)]
                        fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<String> {
                            self.with(py, |target| Ok(target.#rust.to_string()))
                        }
                        #[setter] #[pyo3(name = #name)]
                        fn #setter(&self, py: pyo3::Python<'_>, value: String) -> pyo3::PyResult<()> {
                            let parsed = <#inner>::new(value).map_err(err)?;
                            self.with_mut(py, |target| {
                                target.#rust = parsed;
                                Ok(())
                            })
                        }
                    },
                    quote! {},
                    vec![],
                )
            }
        }
        FieldKind::StrList => {
            let cls = format_ident!("{}{}StrList", c.py, pascal(&rust.to_string()));
            let side = gen_strlist_class(c, rust, &cls);
            let get = gen_list_getter(c, &getter, &name, &cls);
            (get, side, vec![cls])
        }
        FieldKind::ScalarList(inner) => {
            let cls = format_ident!("{}{}ScalarList", c.py, pascal(&rust.to_string()));
            let side = gen_scalarlist_class(c, rust, inner, &cls);
            let get = gen_list_getter(c, &getter, &name, &cls);
            (get, side, vec![cls])
        }
        FieldKind::EnumList { inner, mapped } => {
            let cls = format_ident!("{}{}EnumList", c.py, pascal(&rust.to_string()));
            let side = gen_enumlist_class(c, rust, inner, *mapped, &cls);
            let get = gen_list_getter(c, &getter, &name, &cls);
            (get, side, vec![cls])
        }
        FieldKind::List {
            child,
            name_field,
            clone_append,
        } => {
            let cls = format_ident!("{}{}List", c.py, pascal(&rust.to_string()));
            let side = gen_list_class(c, rust, child, *name_field, *clone_append, &cls);
            let get = gen_list_getter(c, &getter, &name, &cls);
            (get, side, vec![cls])
        }
        FieldKind::Nested { child, clone_set } => gen_nested(c, f, child, *clone_set),
        FieldKind::RequiredNested { child } => gen_required_nested(c, f, child),
        FieldKind::DataEnum { wrapper } => {
            let construct = data_enum_construct(wrapper);
            (
                quote! {
                    #[getter] #[pyo3(name = #name)]
                    fn #getter(&self, py: pyo3::Python<'_>) -> #wrapper {
                        #construct
                    }
                },
                quote! {},
                vec![],
            )
        }
        FieldKind::Choice { wrapper, opt } => gen_choice_field(c, f, wrapper, *opt),
    }
}

/// A nested `Option<Struct>` field: getter (handle-or-None), setter (None clears), and an `ensure_*`
/// that materializes the default and returns a handle. Root and child parents differ in how they get
/// a `Py<PyHcdf>` owner (root receives `slf: Py<Self>`; a child clones its own `owner`).
fn gen_nested(
    c: &Container,
    f: &FieldSpec,
    child: &Ident,
    clone_set: bool,
) -> (
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    Vec<Ident>,
) {
    let rust = &f.rust;
    let name = LitStr::new(&f.py_name, Span::call_site());
    let getter = format_ident!("get_{}", rust);
    let setter = format_ident!("set_{}", rust);
    let ensure = format_ident!("ensure_{}", rust);
    let ensure_py = format!("ensure_{}", f.py_name);
    let ensure_lit = LitStr::new(&ensure_py, Span::call_site());
    let variant = variant_ident(&ty_ident(&c.target), rust);
    let child_loc = format_ident!("{}Loc", child);
    let ploc = parent_loc_value(c);
    let build = quote! { #child { owner: __owner, loc: #child_loc::#variant(std::boxed::Box::new(#ploc)) } };

    if clone_set {
        let set_from = format_ident!("set_{}_from", rust);
        let set_from_name = LitStr::new(&format!("set_{}_from", f.py_name), Span::call_site());
        if c.is_root {
            return (
                quote! {
                    #[getter] #[pyo3(name = #name)]
                    fn #getter(slf: pyo3::Py<Self>, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#child>> {
                        let present = slf.borrow(py).with(py, |target| Ok(target.#rust.is_some()))?;
                        if !present { return Ok(None); }
                        let __owner = slf.clone_ref(py);
                        Ok(Some(#build))
                    }
                    #[setter] #[pyo3(name = #name)]
                    fn #setter(&self, py: pyo3::Python<'_>, value: Option<pyo3::PyObject>) -> pyo3::PyResult<()> {
                        match value {
                            None => self.with_mut(py, |target| { target.#rust = None; Ok(()) }),
                            Some(_) => Err(pyo3::exceptions::PyValueError::new_err(
                                format!("assign {} with {}(value)", #name, #set_from_name)
                            )),
                        }
                    }
                    #[pyo3(name = #set_from_name)]
                    fn #set_from(
                        &self,
                        py: pyo3::Python<'_>,
                        value: pyo3::PyRef<'_, #child>,
                    ) -> pyo3::PyResult<()> {
                        let payload = value.with(py, |payload| Ok(payload.clone()))?;
                        self.with_mut(py, |target| { target.#rust = Some(payload); Ok(()) })
                    }
                },
                quote! {},
                vec![],
            );
        }
        return (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#child>> {
                    let present = self.with(py, |target| Ok(target.#rust.is_some()))?;
                    if !present { return Ok(None); }
                    let __owner = self.owner.clone_ref(py);
                    Ok(Some(#build))
                }
                #[setter] #[pyo3(name = #name)]
                fn #setter(&self, py: pyo3::Python<'_>, value: Option<pyo3::PyObject>) -> pyo3::PyResult<()> {
                    match value {
                        None => self.with_mut(py, |target| { target.#rust = None; Ok(()) }),
                        Some(_) => Err(pyo3::exceptions::PyValueError::new_err(
                            format!("assign {} with {}(value)", #name, #set_from_name)
                        )),
                    }
                }
                #[pyo3(name = #set_from_name)]
                fn #set_from(
                    &self,
                    py: pyo3::Python<'_>,
                    value: pyo3::PyRef<'_, #child>,
                ) -> pyo3::PyResult<()> {
                    let payload = value.with(py, |payload| Ok(payload.clone()))?;
                    self.with_mut(py, |target| { target.#rust = Some(payload); Ok(()) })
                }
            },
            quote! {},
            vec![],
        );
    }

    if c.is_root {
        (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(slf: pyo3::Py<Self>, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#child>> {
                    let present = slf.borrow(py).with(py, |t| Ok(t.#rust.is_some()))?;
                    if !present { return Ok(None); }
                    let __owner = slf.clone_ref(py);
                    Ok(Some(#build))
                }
                #[setter] #[pyo3(name = #name)]
                fn #setter(&self, py: pyo3::Python<'_>, val: Option<pyo3::PyObject>) -> pyo3::PyResult<()> {
                    match val {
                        None => self.with_mut(py, |t| { t.#rust = None; Ok(()) }),
                        Some(_) => Err(pyo3::exceptions::PyValueError::new_err(
                            format!("assign {} via {}() then edit the returned handle", #name, #ensure_lit))),
                    }
                }
                #[pyo3(name = #ensure_lit)]
                fn #ensure(slf: pyo3::Py<Self>, py: pyo3::Python<'_>) -> pyo3::PyResult<#child> {
                    slf.borrow(py).with_mut(py, |t| {
                        if t.#rust.is_none() { t.#rust = Some(Default::default()); }
                        Ok(())
                    })?;
                    let __owner = slf.clone_ref(py);
                    Ok(#build)
                }
            },
            quote! {},
            vec![],
        )
    } else {
        (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#child>> {
                    let present = self.with(py, |t| Ok(t.#rust.is_some()))?;
                    if !present { return Ok(None); }
                    let __owner = self.owner.clone_ref(py);
                    Ok(Some(#build))
                }
                #[setter] #[pyo3(name = #name)]
                fn #setter(&self, py: pyo3::Python<'_>, val: Option<pyo3::PyObject>) -> pyo3::PyResult<()> {
                    match val {
                        None => self.with_mut(py, |t| { t.#rust = None; Ok(()) }),
                        Some(_) => Err(pyo3::exceptions::PyValueError::new_err(
                            format!("assign {} via {}() then edit the returned handle", #name, #ensure_lit))),
                    }
                }
                #[pyo3(name = #ensure_lit)]
                fn #ensure(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<#child> {
                    self.with_mut(py, |t| {
                        if t.#rust.is_none() { t.#rust = Some(Default::default()); }
                        Ok(())
                    })?;
                    let __owner = self.owner.clone_ref(py);
                    Ok(#build)
                }
            },
            quote! {},
            vec![],
        )
    }
}

fn gen_required_nested(
    c: &Container,
    f: &FieldSpec,
    child: &Ident,
) -> (
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    Vec<Ident>,
) {
    let rust = &f.rust;
    let name = LitStr::new(&f.py_name, Span::call_site());
    let getter = format_ident!("get_{}", rust);
    let variant = variant_ident(&ty_ident(&c.target), rust);
    let child_loc = format_ident!("{}Loc", child);
    let ploc = parent_loc_value(c);
    let build = quote! { #child { owner: __owner, loc: #child_loc::#variant(std::boxed::Box::new(#ploc)) } };
    let accessors = if c.is_root {
        quote! {
            #[getter] #[pyo3(name = #name)]
            fn #getter(slf: pyo3::Py<Self>, py: pyo3::Python<'_>) -> #child {
                let __owner = slf.clone_ref(py);
                #build
            }
        }
    } else {
        quote! {
            #[getter] #[pyo3(name = #name)]
            fn #getter(&self, py: pyo3::Python<'_>) -> #child {
                let __owner = self.owner.clone_ref(py);
                #build
            }
        }
    };
    (accessors, quote! {}, vec![])
}

/// Construct a data-enum wrapper handle from `self` (wrapper stores the container's own loc).
fn data_enum_construct(wrapper: &Ident) -> proc_macro2::TokenStream {
    quote! { #wrapper { owner: self.owner.clone_ref(py), loc: self.loc.clone() } }
}

fn gen_choice_field(
    c: &Container,
    f: &FieldSpec,
    wrapper: &Ident,
    opt: bool,
) -> (
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    Vec<Ident>,
) {
    let rust = &f.rust;
    let name = LitStr::new(&f.py_name, Span::call_site());
    let getter = format_ident!("get_{}", rust);
    let editor = format_ident!("edit_{}", rust);
    let editor_name = LitStr::new(&format!("edit_{}", f.py_name), Span::call_site());
    let setter = format_ident!("set_{}", rust);
    let choice_loc = format_ident!("{}Loc", wrapper);
    let loc_variant = variant_ident(&ty_ident(&c.target), rust);
    let parent_loc = parent_loc_value(c);

    if c.is_root {
        let construct = quote! {
            #wrapper {
                owner: slf.clone_ref(py),
                loc: #choice_loc::#loc_variant(std::boxed::Box::new(#parent_loc)),
            }
        };
        if opt {
            return (
                quote! {
                    #[getter] #[pyo3(name = #name)]
                    fn #getter(slf: pyo3::Py<Self>, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#wrapper>> {
                        let present = slf.borrow(py).with(py, |target| Ok(target.#rust.is_some()))?;
                        if !present { return Ok(None); }
                        Ok(Some(#construct))
                    }
                    #[pyo3(name = #editor_name)]
                    fn #editor(slf: pyo3::Py<Self>, py: pyo3::Python<'_>) -> #wrapper {
                        #construct
                    }
                    #[setter] #[pyo3(name = #name)]
                    fn #setter(&self, py: pyo3::Python<'_>, value: Option<pyo3::PyObject>) -> pyo3::PyResult<()> {
                        match value {
                            None => self.with_mut(py, |target| { target.#rust = None; Ok(()) }),
                            Some(_) => Err(pyo3::exceptions::PyValueError::new_err(
                                format!("assign {} through {}() and an exact arm setter", #name, #editor_name)
                            )),
                        }
                    }
                },
                quote! {},
                vec![],
            );
        }
        return (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(slf: pyo3::Py<Self>, py: pyo3::Python<'_>) -> #wrapper {
                    #construct
                }
            },
            quote! {},
            vec![],
        );
    }

    let construct = quote! {
        #wrapper {
            owner: self.owner.clone_ref(py),
            loc: #choice_loc::#loc_variant(std::boxed::Box::new(#parent_loc)),
        }
    };
    if opt {
        (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<Option<#wrapper>> {
                    let present = self.with(py, |target| Ok(target.#rust.is_some()))?;
                    if !present { return Ok(None); }
                    Ok(Some(#construct))
                }
                #[pyo3(name = #editor_name)]
                fn #editor(&self, py: pyo3::Python<'_>) -> #wrapper {
                    #construct
                }
                #[setter] #[pyo3(name = #name)]
                fn #setter(&self, py: pyo3::Python<'_>, value: Option<pyo3::PyObject>) -> pyo3::PyResult<()> {
                    match value {
                        None => self.with_mut(py, |target| { target.#rust = None; Ok(()) }),
                        Some(_) => Err(pyo3::exceptions::PyValueError::new_err(
                            format!("assign {} through {}() and an exact arm setter", #name, #editor_name)
                        )),
                    }
                }
            },
            quote! {},
            vec![],
        )
    } else {
        (
            quote! {
                #[getter] #[pyo3(name = #name)]
                fn #getter(&self, py: pyo3::Python<'_>) -> #wrapper {
                    #construct
                }
            },
            quote! {},
            vec![],
        )
    }
}

/// The container getter that returns a list/strlist sub-handle.
fn gen_list_getter(
    c: &Container,
    getter: &Ident,
    name: &LitStr,
    cls: &Ident,
) -> proc_macro2::TokenStream {
    if c.is_root {
        let owner_loc = format_ident!("{}Loc", c.owner_py);
        quote! {
            #[getter] #[pyo3(name = #name)]
            fn #getter(slf: pyo3::Py<Self>) -> #cls {
                #cls { owner: slf, loc: #owner_loc }
            }
        }
    } else {
        quote! {
            #[getter] #[pyo3(name = #name)]
            fn #getter(&self, py: pyo3::Python<'_>) -> #cls {
                #cls { owner: self.owner.clone_ref(py), loc: self.loc.clone() }
            }
        }
    }
}

/// A list-of-handles sub-handle class for a `Vec<Struct>` field.
fn gen_list_class(
    c: &Container,
    field: &Ident,
    child: &Ident,
    name_field: NameField,
    clone_append: bool,
    cls: &Ident,
) -> proc_macro2::TokenStream {
    let target = &c.target;
    let owner_py = &c.owner_py;
    let owner_target = &c.owner_target;
    let loc = format_ident!("{}Loc", c.py);
    let cls_lit = LitStr::new(&cls.to_string(), Span::call_site());
    let cpy_lit = LitStr::new(&c.py.to_string(), Span::call_site());
    let child_loc = format_ident!("{}Loc", child);
    let variant = variant_ident(&ty_ident(&c.target), field);
    let append_method = if clone_append {
        quote! {
            fn append(
                &self,
                py: pyo3::Python<'_>,
                value: pyo3::PyRef<'_, #child>,
            ) -> pyo3::PyResult<()> {
                let payload = value.with(py, |payload| Ok(payload.clone()))?;
                let owner = self.owner.borrow(py);
                let mut doc = owner.doc.borrow_mut();
                let container = <#target as Resolve<#owner_target>>::resolve_mut(&mut doc, &self.loc)
                    .ok_or_else(|| stale(#cpy_lit))?;
                container.#field.push(payload);
                Ok(())
            }
        }
    } else {
        // Push a `Default` first, then stamp its name. Setting `element.name` before the push would
        // leave the element type unavailable for inference.
        let append_body = match name_field {
            NameField::None => quote! {
                let _ = name;
                container.#field.push(Default::default());
            },
            NameField::Req => quote! {
                container.#field.push(Default::default());
                if let (Some(name), Some(element)) = (name, container.#field.last_mut()) {
                    element.name = name;
                }
            },
            NameField::Opt => quote! {
                container.#field.push(Default::default());
                if let Some(element) = container.#field.last_mut() { element.name = name; }
            },
        };
        quote! {
            #[pyo3(signature = (name=None))]
            fn append(&self, py: pyo3::Python<'_>, name: Option<String>) -> pyo3::PyResult<()> {
                let owner = self.owner.borrow(py);
                let mut doc = owner.doc.borrow_mut();
                let container = <#target as Resolve<#owner_target>>::resolve_mut(&mut doc, &self.loc)
                    .ok_or_else(|| stale(#cpy_lit))?;
                #append_body
                Ok(())
            }
        }
    };
    quote! {
        #[pyo3::pyclass(unsendable, name = #cls_lit)]
        pub struct #cls { pub(crate) owner: pyo3::Py<#owner_py>, pub(crate) loc: #loc }
        #[pyo3::pymethods]
        impl #cls {
            fn __len__(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<usize> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                Ok(c.#field.len())
            }
            fn __getitem__(&self, py: pyo3::Python<'_>, idx: isize) -> pyo3::PyResult<#child> {
                let len = self.__len__(py)?;
                let i = resolve_index(idx, len)?;
                Ok(#child { owner: self.owner.clone_ref(py), loc: #child_loc::#variant(std::boxed::Box::new(self.loc.clone()), i) })
            }
            fn __iter__(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
                let len = self.__len__(py)?;
                let items = pyo3::types::PyList::empty(py);
                for i in 0..len {
                    let h = #child { owner: self.owner.clone_ref(py), loc: #child_loc::#variant(std::boxed::Box::new(self.loc.clone()), i) };
                    items.append(pyo3::Py::new(py, h)?)?;
                }
                Ok(items.call_method0("__iter__")?.unbind())
            }
            #append_method
        }
    }
}

/// A `list[str]` sub-handle class for a `Vec<String>` field.
fn gen_strlist_class(c: &Container, field: &Ident, cls: &Ident) -> proc_macro2::TokenStream {
    let target = &c.target;
    let owner_py = &c.owner_py;
    let owner_target = &c.owner_target;
    let loc = format_ident!("{}Loc", c.py);
    let cls_lit = LitStr::new(&cls.to_string(), Span::call_site());
    let cpy_lit = LitStr::new(&c.py.to_string(), Span::call_site());
    quote! {
        #[pyo3::pyclass(unsendable, name = #cls_lit)]
        pub struct #cls { pub(crate) owner: pyo3::Py<#owner_py>, pub(crate) loc: #loc }
        #[pyo3::pymethods]
        impl #cls {
            fn __len__(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<usize> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                Ok(c.#field.len())
            }
            fn __getitem__(&self, py: pyo3::Python<'_>, idx: isize) -> pyo3::PyResult<String> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                let i = resolve_index(idx, c.#field.len())?;
                Ok(c.#field[i].clone())
            }
            fn __iter__(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                let items = pyo3::types::PyList::new(py, c.#field.iter().cloned())?;
                Ok(items.call_method0("__iter__")?.unbind())
            }
            fn append(&self, py: pyo3::Python<'_>, val: String) -> pyo3::PyResult<()> {
                let owner = self.owner.borrow(py);
                let mut doc = owner.doc.borrow_mut();
                let c = <#target as Resolve<#owner_target>>::resolve_mut(&mut doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                c.#field.push(val);
                Ok(())
            }
        }
    }
}

fn gen_scalarlist_class(
    c: &Container,
    field: &Ident,
    inner: &Ident,
    cls: &Ident,
) -> proc_macro2::TokenStream {
    let target = &c.target;
    let owner_py = &c.owner_py;
    let owner_target = &c.owner_target;
    let loc = format_ident!("{}Loc", c.py);
    let cls_lit = LitStr::new(&cls.to_string(), Span::call_site());
    let cpy_lit = LitStr::new(&c.py.to_string(), Span::call_site());
    quote! {
        #[pyo3::pyclass(unsendable, name = #cls_lit)]
        pub struct #cls { pub(crate) owner: pyo3::Py<#owner_py>, pub(crate) loc: #loc }
        #[pyo3::pymethods]
        impl #cls {
            fn __len__(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<usize> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                Ok(c.#field.len())
            }
            fn __getitem__(&self, py: pyo3::Python<'_>, idx: isize) -> pyo3::PyResult<#inner> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                let i = resolve_index(idx, c.#field.len())?;
                Ok(c.#field[i])
            }
            fn __iter__(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                let items = pyo3::types::PyList::new(py, c.#field.iter().copied())?;
                Ok(items.call_method0("__iter__")?.unbind())
            }
            fn append(&self, py: pyo3::Python<'_>, val: #inner) -> pyo3::PyResult<()> {
                let owner = self.owner.borrow(py);
                let mut doc = owner.doc.borrow_mut();
                let c = <#target as Resolve<#owner_target>>::resolve_mut(&mut doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                c.#field.push(val);
                Ok(())
            }
        }
    }
}

fn gen_enumlist_class(
    c: &Container,
    field: &Ident,
    inner: &Ident,
    mapped: bool,
    cls: &Ident,
) -> proc_macro2::TokenStream {
    let target = &c.target;
    let owner_py = &c.owner_py;
    let owner_target = &c.owner_target;
    let loc = format_ident!("{}Loc", c.py);
    let cls_lit = LitStr::new(&cls.to_string(), Span::call_site());
    let cpy_lit = LitStr::new(&c.py.to_string(), Span::call_site());
    let get_value = if mapped {
        quote! { c.#field[i].dom_value().to_owned() }
    } else {
        quote! { c.#field[i].to_string() }
    };
    let collect_values = if mapped {
        quote! {
            c.#field
                .iter()
                .map(|value| value.dom_value().to_owned())
                .collect()
        }
    } else {
        quote! { c.#field.iter().map(ToString::to_string).collect() }
    };
    let parse_value = if mapped {
        quote! {
            DomEnum::from_dom_value(&val).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "invalid enum list value: {:?}", val
                ))
            })?
        }
    } else {
        quote! {
            val.parse().map_err(|_| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "invalid enum list value: {:?}", val
                ))
            })?
        }
    };
    quote! {
        #[pyo3::pyclass(unsendable, name = #cls_lit)]
        pub struct #cls { pub(crate) owner: pyo3::Py<#owner_py>, pub(crate) loc: #loc }
        #[pyo3::pymethods]
        impl #cls {
            fn __len__(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<usize> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                Ok(c.#field.len())
            }
            fn __getitem__(&self, py: pyo3::Python<'_>, idx: isize) -> pyo3::PyResult<String> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                let i = resolve_index(idx, c.#field.len())?;
                Ok(#get_value)
            }
            fn __iter__(&self, py: pyo3::Python<'_>) -> pyo3::PyResult<pyo3::Py<pyo3::PyAny>> {
                let owner = self.owner.borrow(py);
                let doc = owner.doc.borrow();
                let c = <#target as Resolve<#owner_target>>::resolve(&doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                let values: Vec<String> = #collect_values;
                let items = pyo3::types::PyList::new(py, values)?;
                Ok(items.call_method0("__iter__")?.unbind())
            }
            fn append(&self, py: pyo3::Python<'_>, val: String) -> pyo3::PyResult<()> {
                let parsed: #inner = #parse_value;
                let owner = self.owner.borrow(py);
                let mut doc = owner.doc.borrow_mut();
                let c = <#target as Resolve<#owner_target>>::resolve_mut(&mut doc, &self.loc).ok_or_else(|| stale(#cpy_lit))?;
                c.#field.push(parsed);
                Ok(())
            }
        }
    }
}

fn gen_register(c: &Container, list_idents: &[Ident]) -> proc_macro2::TokenStream {
    let py = &c.py;
    let reg = format_ident!("__register_{}", snake(py));
    let adds = list_idents
        .iter()
        .map(|id| quote! { m.add_class::<#id>()?; });
    quote! {
        pub fn #reg(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
            m.add_class::<#py>()?;
            #(#adds)*
            Ok(())
        }
    }
}

fn gen_pyi(c: &Container, fields: &[FieldSpec]) -> proc_macro2::TokenStream {
    let py = &c.py;
    let pyi_fn = format_ident!("__pyi_{}", snake(py));
    let cls = c.py.to_string();
    let mut lines = format!("class {cls}:\n");
    if c.is_root {
        lines.push_str(&format!(
            "    @staticmethod\n    def loads(xml: str) -> \"{cls}\": ...\n"
        ));
        lines.push_str(&format!(
            "    @staticmethod\n    def load(path: str) -> \"{cls}\": ...\n"
        ));
        lines.push_str("    def dumps(self) -> str: ...\n");
        lines.push_str("    def dump(self, path: str) -> None: ...\n");
    }
    if fields.is_empty() && !c.is_root {
        lines.push_str("    pass\n");
    }
    let mut side = String::new();
    for f in fields {
        let n = &f.py_name;
        let ann = match &f.kind {
            FieldKind::ReqStr => "str".to_string(),
            FieldKind::ReqScalar(i) => match i.to_string().as_str() {
                "bool" => "bool".to_string(),
                "f64" | "f32" => "float".to_string(),
                _ => "int".to_string(),
            },
            FieldKind::OptStr => "Optional[str]".to_string(),
            FieldKind::OptScalar(i) => match i.to_string().as_str() {
                "bool" => "Optional[bool]".to_string(),
                "f64" | "f32" => "Optional[float]".to_string(),
                _ => "Optional[int]".to_string(),
            },
            FieldKind::ReqArr(_) => "list[float]".to_string(),
            FieldKind::OptArr(_) => "Optional[list[float]]".to_string(),
            FieldKind::EnumScalar { opt } => {
                if *opt {
                    "Optional[str]".to_string()
                } else {
                    "str".to_string()
                }
            }
            FieldKind::MappedEnumScalar { opt } => {
                if *opt {
                    "Optional[str]".to_string()
                } else {
                    "str".to_string()
                }
            }
            FieldKind::ValidatedString { opt, .. } => {
                if *opt {
                    "Optional[str]".to_string()
                } else {
                    "str".to_string()
                }
            }
            FieldKind::StrList => {
                let scls = format!("{}{}StrList", py, pascal(&f.rust.to_string()));
                side.push_str(&format!(
                    "class {scls}:\n    def __len__(self) -> int: ...\n    def __getitem__(self, i: int) -> str: ...\n    def __iter__(self) -> Iterator[str]: ...\n    def append(self, val: str) -> None: ...\n\n"
                ));
                format!("\"{scls}\"")
            }
            FieldKind::ScalarList(inner) => {
                let scls = format!("{}{}ScalarList", py, pascal(&f.rust.to_string()));
                let item = match inner.to_string().as_str() {
                    "bool" => "bool",
                    "f64" | "f32" => "float",
                    _ => "int",
                };
                side.push_str(&format!(
                    "class {scls}:\n    def __len__(self) -> int: ...\n    def __getitem__(self, i: int) -> {item}: ...\n    def __iter__(self) -> Iterator[{item}]: ...\n    def append(self, val: {item}) -> None: ...\n\n"
                ));
                format!("\"{scls}\"")
            }
            FieldKind::EnumList { .. } => {
                let scls = format!("{}{}EnumList", py, pascal(&f.rust.to_string()));
                side.push_str(&format!(
                    "class {scls}:\n    def __len__(self) -> int: ...\n    def __getitem__(self, i: int) -> str: ...\n    def __iter__(self) -> Iterator[str]: ...\n    def append(self, val: str) -> None: ...\n\n"
                ));
                format!("\"{scls}\"")
            }
            FieldKind::List {
                child,
                clone_append,
                ..
            } => {
                let lcls = format!("{}{}List", py, pascal(&f.rust.to_string()));
                let append = if *clone_append {
                    format!("    def append(self, value: \"{child}\") -> None: ...\n")
                } else {
                    "    def append(self, name: Optional[str] = ...) -> None: ...\n".to_owned()
                };
                side.push_str(&format!(
                    "class {lcls}:\n    def __len__(self) -> int: ...\n    def __getitem__(self, i: int) -> \"{child}\": ...\n    def __iter__(self) -> Iterator[\"{child}\"]: ...\n{append}\n"
                ));
                format!("\"{lcls}\"")
            }
            FieldKind::Nested { child, .. } => format!("Optional[\"{child}\"]"),
            FieldKind::RequiredNested { child } => format!("\"{child}\""),
            FieldKind::DataEnum { wrapper } => format!("\"{wrapper}\""),
            FieldKind::Choice { wrapper, opt } => {
                if *opt {
                    format!("Optional[\"{wrapper}\"]")
                } else {
                    format!("\"{wrapper}\"")
                }
            }
        };
        lines.push_str(&format!("    {n}: {ann}\n"));
        if let FieldKind::Nested { child, clone_set } = &f.kind {
            if *clone_set {
                lines.push_str(&format!(
                    "    def set_{}_from(self, value: \"{child}\") -> None: ...\n",
                    f.py_name
                ));
            } else {
                lines.push_str(&format!(
                    "    def ensure_{}(self) -> \"{child}\": ...\n",
                    f.py_name
                ));
            }
        }
        if let FieldKind::Choice { wrapper, opt: true } = &f.kind {
            lines.push_str(&format!(
                "    def edit_{}(self) -> \"{wrapper}\": ...\n",
                f.py_name
            ));
        }
    }
    lines.push('\n');
    lines.push_str(&side);
    let lit = LitStr::new(&lines, Span::call_site());
    quote! {
        pub fn #pyi_fn() -> &'static str { #lit }
    }
}

// ── type helpers ─────────────────────────────────────────────────────────────────────────────────

fn outer_ident(ty: &Type) -> Option<String> {
    if let Type::Path(p) = ty {
        return p.path.segments.last().map(|s| s.ident.to_string());
    }
    None
}

fn inner_of(ty: &Type, wrapper: &str) -> Option<Type> {
    if let Type::Path(p) = ty {
        let seg = p.path.segments.last()?;
        if seg.ident != wrapper {
            return None;
        }
        if let syn::PathArguments::AngleBracketed(a) = &seg.arguments {
            if let Some(syn::GenericArgument::Type(t)) = a.args.first() {
                return Some(t.clone());
            }
        }
    }
    None
}

fn inner_leaf(ty: &Type) -> Option<String> {
    inner_of(ty, "Option").as_ref().and_then(outer_ident)
}

fn is_vec_of(ty: &Type, leaf: &str) -> bool {
    inner_of(ty, "Vec")
        .as_ref()
        .and_then(outer_ident)
        .as_deref()
        == Some(leaf)
}

fn expr_ident(e: &Expr) -> syn::Result<Ident> {
    if let Expr::Path(p) = e {
        if let Some(id) = p.path.get_ident() {
            return Ok(id.clone());
        }
    }
    Err(syn::Error::new_spanned(e, "expected an identifier"))
}

fn expr_type(e: &Expr) -> syn::Result<Type> {
    if let Expr::Path(p) = e {
        return Ok(Type::Path(syn::TypePath {
            qself: p.qself.clone(),
            path: p.path.clone(),
        }));
    }
    Err(syn::Error::new_spanned(e, "expected a type path"))
}

fn err_call(msg: &str) -> syn::Error {
    syn::Error::new(Span::call_site(), msg)
}

/// `power_source` / `type_` -> `PowerSource` / `Type` (PascalCase for generated class/variant idents).
fn pascal(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `PyOpticalSensor` -> `py_optical_sensor` (for snake-case helper fn names).
fn snake(id: &Ident) -> String {
    let s = id.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_choice(source: &str) -> syn::Result<Vec<ChoiceVariantSpec>> {
        let choice: ItemEnum = syn::parse_str(source).expect("parse choice fixture");
        parse_choice_variants(&choice)
    }

    #[test]
    fn root_data_enum_is_rejected_before_code_generation() {
        let mirror: ItemStruct =
            syn::parse_str("struct RootDom { #[pydom(data_enum = PyAppearance)] appearance: () }")
                .expect("parse root mirror");
        let fields = parse_fields(&mirror).expect("parse root fields");
        let root = Container {
            py: syn::parse_quote!(PyRoot),
            target: syn::parse_quote!(Root),
            owner_py: syn::parse_quote!(PyRoot),
            owner_target: syn::parse_quote!(Root),
            is_root: true,
            sites: Vec::new(),
        };
        let error = validate_root_field_compatibility(&root, &fields)
            .expect_err("root data-enum rejection")
            .to_string();
        assert!(error.contains("root field \"appearance\""), "{error}");
        assert!(error.contains("owner-bound data-enum companion"), "{error}");
        assert!(error.contains("compatible owner root"), "{error}");

        let child = Container {
            is_root: false,
            ..root
        };
        validate_root_field_compatibility(&child, &fields)
            .expect("child data-enum stays below its compatible owner root");
    }

    #[test]
    fn choice_public_namespace_rejects_derived_member_collisions() {
        let selector = parse_choice("enum Choice { Foo, SelectFoo }")
            .err()
            .expect("selector collision");
        let selector = selector.to_string();
        assert!(selector.contains("public Python choice member \"select_foo\""));
        assert!(selector.contains("choice arm `SelectFoo` property"));
        assert!(selector.contains("choice arm `Foo` selector"));

        let clone_setter = parse_choice(
            "enum Choice {\n\
             #[pychoice(payload = PyPayload, draft = draft_payload)] Foo(Payload),\n\
             SetFooFrom,\n\
             }",
        )
        .err()
        .expect("clone setter collision");
        let clone_setter = clone_setter.to_string();
        assert!(clone_setter.contains("public Python choice member \"set_foo_from\""));
        assert!(clone_setter.contains("choice arm `SetFooFrom` property"));
        assert!(clone_setter.contains("choice arm `Foo` clone setter"));
    }

    #[test]
    fn choice_public_namespace_reserves_builtin_members() {
        let variant = parse_choice("enum Choice { Variant }")
            .err()
            .expect("variant property collision")
            .to_string();
        assert!(variant.contains("public Python choice member \"variant\""));
        assert!(variant.contains("choice arm `Variant` property"));
        assert!(variant.contains("the active-variant property"));

        let repr = parse_choice("enum Choice { __repr__ }")
            .err()
            .expect("representation method collision")
            .to_string();
        assert!(repr.contains("public Python choice member \"__repr__\""));
        assert!(repr.contains("choice arm `__repr__` property"));
        assert!(repr.contains("the representation method"));
    }

    #[test]
    fn choice_public_labels_are_unique_and_origin_aware() {
        let error = parse_choice(
            "enum Choice {\n\
             #[pychoice(name = \"shared\")] First,\n\
             #[pychoice(name = \"shared\")] Second,\n\
             }",
        )
        .err()
        .expect("duplicate public label")
        .to_string();
        assert!(error.contains("public Python choice label \"shared\""));
        assert!(error.contains("choice arm `Second` public label"));
        assert!(error.contains("choice arm `First` public label"));
    }

    #[test]
    fn distinct_choice_public_members_and_labels_are_accepted() {
        let variants = parse_choice(
            "enum Choice {\n\
             #[pychoice(name = \"payload-label\", payload = PyPayload, draft = draft_payload)] Payload(Payload),\n\
             #[pychoice(name = \"text-label\")] Text(String),\n\
             #[pychoice(name = \"empty-label\")] Empty,\n\
             }",
        )
        .expect("non-colliding choice");
        assert_eq!(variants.len(), 3);
    }
}
