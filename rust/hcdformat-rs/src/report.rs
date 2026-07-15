//! Rendering of the URDF profile report and the URDF-export loss manifest (feature = `urdf`/`sdf`).
//!
//! The profile classifier ([`mod@crate::profile`]) and the URDF exporter ([`mod@crate::to_urdf`])
//! produce structured results; the human- and machine-readable renders of those results (the flat
//! text form, the grouped markdown, and the pretty JSON) live here so the CLI and the PyO3 binding
//! share ONE renderer instead of each carrying its own. The render methods hang off
//! [`crate::to_urdf::LossManifest`] and [`crate::profile::ProfileReport`]; this module holds only the
//! small JSON writer they build on.
//!
//! The JSON writer reproduces CPython's `json.dumps(value, indent=2)` byte-for-byte, including the
//! `ensure_ascii=True` default (every non-ASCII scalar becomes one or two `\uXXXX` units), so a
//! document's rendered report is identical whether it came from the CLI or from a Python caller.

/// A tiny JSON value model, ordered so object keys serialize in insertion order (matching a Python
/// dict, which `serde_json`'s key-sorted `BTreeMap` maps can NOT reproduce). Only the shapes the two
/// reports need are modelled: strings, booleans, non-negative counts, arrays, and ordered objects.
pub(crate) enum Json {
    Str(String),
    Bool(bool),
    Uint(usize),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// Serialize a [`Json`] value exactly like CPython `json.dumps(value, indent=2)`: two-space indent per
/// level, `": "` after keys, a `,` between siblings, and empty containers collapsed to `{}` / `[]`.
/// There is no trailing newline (matching `json.dumps`); the caller adds one when it prints.
pub(crate) fn dump(v: &Json) -> String {
    let mut out = String::new();
    write_value(v, 0, &mut out);
    out
}

fn write_value(v: &Json, level: usize, out: &mut String) {
    match v {
        Json::Str(s) => write_string(s, out),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Uint(n) => out.push_str(&n.to_string()),
        Json::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                indent(level + 1, out);
                write_value(item, level + 1, out);
            }
            out.push('\n');
            indent(level, out);
            out.push(']');
        }
        Json::Obj(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (key, val)) in entries.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                indent(level + 1, out);
                write_string(key, out);
                out.push_str(": ");
                write_value(val, level + 1, out);
            }
            out.push('\n');
            indent(level, out);
            out.push('}');
        }
    }
}

fn indent(level: usize, out: &mut String) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

/// Write `s` as a quoted JSON string with CPython `json.dumps` default escaping (`ensure_ascii=True`):
/// the two mandatory escapes, the short control escapes, any other C0 control as `\u00xx`, and every
/// non-ASCII scalar as one `\uXXXX` unit (or a UTF-16 surrogate pair above U+FFFF), all lowercase hex.
fn write_string(s: &str, out: &mut String) {
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
            c if (c as u32) < 0x20 => push_u(c as u32, out),
            c if c.is_ascii() => out.push(c),
            c => {
                let cp = c as u32;
                if cp <= 0xFFFF {
                    push_u(cp, out);
                } else {
                    let v = cp - 0x10000;
                    push_u(0xD800 + (v >> 10), out);
                    push_u(0xDC00 + (v & 0x3FF), out);
                }
            }
        }
    }
    out.push('"');
}

fn push_u(code: u32, out: &mut String) {
    out.push_str(&format!("\\u{code:04x}"));
}
