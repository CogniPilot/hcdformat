//! Python-`repr`-compatible formatting for converter detail strings (feature = `urdf`).
//!
//! The `LossManifest` details, profile `Finding` details, and import notes are part of the converter's
//! observable contract (`hcdf convert --loss` JSON, `hcdf profile --json`). The Python authority builds
//! them with `{!r}` (`repr`) and enum `repr`, so a byte-identical Rust port must reproduce Python's
//! quoting exactly:
//!   * a string renders single-quoted: `'tool'` (NOT Rust `{:?}`'s double-quoted `"tool"`);
//!   * an enum renders `<EnumName.member: 'value'>` (Python `enum.Enum.__repr__`).
//!
//! Rust `format!("{:?}", s)` double-quotes and would diverge; [`repr_str`] matches CPython's
//! `str.__repr__` for the cases the converter emits (the corpus values are simple identifiers/uris with
//! no embedded quotes or control chars, but the escaping below is faithful for the general case too).

/// Python `repr()` of a string: single-quoted, switching to double quotes only when the string contains
/// a `'` but no `"` (CPython's quote-selection rule), with `\\`, the active quote, and the standard
/// control characters escaped. Matches `repr(s)` for the values the converter formats.
pub fn repr_str(s: &str) -> String {
    // CPython uses single quotes, unless the string has a ' and no ", in which case it uses double.
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr()` of an `Option<&str>`: `'value'` when present, `None` when absent, the same text
/// Python prints for an optional string attribute formatted with `{!r}`.
pub fn repr_opt(v: Option<&str>) -> String {
    match v {
        Some(s) => repr_str(s),
        None => "None".to_string(),
    }
}

/// Python `enum.Enum.__repr__` form `<EnumName.member: 'value'>`. For the HCDF enums the member name
/// equals the serialized value (e.g. `CompRole.actuator` has value `'actuator'`), so the caller passes
/// the enum's type name and its serialized value and both slots are filled identically, matching
/// `repr(CompRole.actuator)` == `<CompRole.actuator: 'actuator'>`.
pub fn repr_enum(enum_name: &str, value: &str) -> String {
    format!("<{enum_name}.{value}: {}>", repr_str(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_cpython_repr() {
        // CPython: repr('tool') == "'tool'"; repr("a'b") switches to double quotes; repr('a"b') stays single.
        assert_eq!(repr_str("tool"), "'tool'");
        assert_eq!(repr_str("a'b"), "\"a'b\"");
        assert_eq!(repr_str("a\"b"), "'a\"b'");
        assert_eq!(repr_str("a\\b"), "'a\\\\b'");
        assert_eq!(repr_opt(None), "None");
        assert_eq!(repr_opt(Some("x")), "'x'");
        // enum repr form `<CompRole.actuator: 'actuator'>` (member name == serialized value).
        assert_eq!(
            repr_enum("CompRole", "actuator"),
            "<CompRole.actuator: 'actuator'>"
        );
    }
}
