//! Pure-Rust XSD validation against the embedded frozen `hcdf.xsd` (feature `xsd`).
//!
//! This is the in-crate replacement for the `hcdf validate --xsd` (lxml) shell-out: it catches the
//! schema-SHAPE errors the typed parse silently swallows, notably a `<box>` written with raw text
//! instead of a `<size>` CHILD (`<box>.1 .1 .5</box>`, which deserializes to an empty `<box/>`), and a
//! `<color>` written with an `<rgba>` CHILD instead of the required `rgba=` ATTRIBUTE
//! (`<color><rgba>…</rgba></color>`). The other validation layers ([`crate::validate::validate_enums`]
//! / `validate_structural` / `validate_semantic`) operate on the deserialized model and so never see
//! these, which is exactly why dendrite_build used to shell out to lxml.
//!
//! Backed by [`uppsala`], a pure-Rust XSD validator with ZERO transitive dependencies that builds to
//! wasm32 (no `*-sys`/cc/native backend), so this gate runs unchanged in the browser. It reaches 16/16
//! accept/reject parity with lxml on the example corpus, the `tests/invalid/*` corpus, and the two shape
//! fixtures, with `key`/`keyref`/`unique` identity constraints enforced by default.
//!
//! The compiled validator is built ONCE from [`crate::schema::HCDF_XSD_1_0`] and reused across calls
//! (the schema is frozen + sha-pinned, so a single compile is correct and the per-validate cost is just
//! parsing the instance document).

use crate::validate::{Issue, Level};
use std::sync::OnceLock;
use uppsala::{parse, XsdValidator};

/// A stable machine code for an XSD schema violation reported by [`validate_xsd`]. uppsala does not
/// carry a per-error constraint category, so all schema violations (and the well-formedness/compile
/// failures) share this one code; the line:col + message in [`Issue::message`] carry the detail.
pub const E_XSD: &str = "E_XSD";

/// Compile the embedded `hcdf.xsd` with uppsala exactly once and hand back the shared validator.
///
/// The schema is parsed as XML and built into an [`XsdValidator`] on first use, then cached in a
/// process-wide [`OnceLock`] and reused: we never recompile per [`validate_xsd`] call. `Err` carries a
/// message only if the FROZEN, sha-pinned schema fails to parse/compile (an impossible state in a
/// correctly-built crate), so callers can surface it as a single `E_XSD` issue rather than panicking.
fn validator() -> Result<&'static XsdValidator, &'static str> {
    // The compiled validator borrows from the parsed schema document, so both the document and the
    // validator must outlive every call. We leak both into `'static` (a one-time, bounded cost: the
    // schema is a single frozen blob) so the cached `&'static XsdValidator` is sound to hand out. The
    // error arm is a `&'static str` so the cached `Result` is trivially copyable out of the `OnceLock`.
    static VALIDATOR: OnceLock<Result<&'static XsdValidator, &'static str>> = OnceLock::new();
    *VALIDATOR.get_or_init(|| {
        let schema_src = std::str::from_utf8(crate::schema::HCDF_XSD_1_0)
            .map_err(|_| "embedded hcdf.xsd is not valid UTF-8")?;
        // Leak the parsed schema document so the validator built from it can be `'static`.
        let schema_doc: &'static _ = Box::leak(Box::new(
            parse(schema_src).map_err(|_| "embedded hcdf.xsd failed to parse as XML")?,
        ));
        let validator = XsdValidator::from_schema(schema_doc)
            .map_err(|_| "embedded hcdf.xsd failed to compile as an XSD schema")?;
        Ok(Box::leak(Box::new(validator)))
    })
}

/// Validate an HCDF document XML string against the embedded frozen `hcdf.xsd`, returning the schema
/// violations as [`Issue`]s (empty ⇒ schema-VALID). All issues are [`Level::Error`] with code [`E_XSD`].
///
/// This is the schema-SHAPE gate: it rejects the malformed `<box>`/`<color>` forms that deserialize to
/// empty (see the module docs) and enforces required attributes, enum facets, choice exclusivity, and
/// keyref/identity constraints, at PARITY with lxml `hcdf validate --xsd`.
///
/// XML that is not well-formed surfaces as a single `E_XSD` issue (a REJECT, matching lxml). The
/// one-time schema compile is cached; a (theoretically impossible) failure to compile the frozen,
/// sha-pinned schema is itself reported as one `E_XSD` issue rather than a panic, so the caller's Apply
/// gate degrades safely instead of crashing.
///
/// Pure-Rust and wasm-clean (uppsala has zero transitive deps and no native backend).
pub fn validate_xsd(xml: &str) -> Vec<Issue> {
    let validator = match validator() {
        Ok(v) => v,
        Err(e) => {
            return vec![Issue {
                level: Level::Error,
                code: E_XSD.to_string(),
                message: format!("XSD schema unavailable: {e}"),
            }]
        }
    };
    // A well-formedness failure is a REJECT (lxml does the same): map it to one E_XSD issue carrying the
    // parser's line:col + message via its Display.
    let doc = match parse(xml) {
        Ok(d) => d,
        Err(e) => {
            return vec![Issue {
                level: Level::Error,
                code: E_XSD.to_string(),
                message: format!("not well-formed XML: {e}"),
            }]
        }
    };
    validator
        .validate(&doc)
        .into_iter()
        .map(|e| Issue {
            level: Level::Error,
            code: E_XSD.to_string(),
            // `ValidationError`'s Display renders `line:col: message` when a location is known, else just
            // the message, so the issue carries the line/col a caller needs to locate the fault.
            message: e.to_string(),
        })
        .collect()
}

fn stream_profile_validator() -> Result<&'static XsdValidator, &'static str> {
    static VALIDATOR: OnceLock<Result<&'static XsdValidator, &'static str>> = OnceLock::new();
    *VALIDATOR.get_or_init(|| {
        let schema_src = std::str::from_utf8(crate::schema::HCDF_STREAM_PROFILE_XSD_1_0)
            .map_err(|_| "embedded hcdf-stream-profile.xsd is not valid UTF-8")?;
        let schema_doc: &'static _ =
            Box::leak(Box::new(parse(schema_src).map_err(|_| {
                "embedded hcdf-stream-profile.xsd failed to parse as XML"
            })?));
        let validator = XsdValidator::from_schema(schema_doc)
            .map_err(|_| "embedded hcdf-stream-profile.xsd failed to compile as an XSD schema")?;
        Ok(Box::leak(Box::new(validator)))
    })
}

/// Validate a stream-profile sidecar against the embedded, sha-pinned sidecar schema.
///
/// Compilation is cached once per process, and the implementation is pure Rust and wasm-clean.
pub fn validate_stream_profile_xsd(xml: &str) -> Vec<Issue> {
    let validator = match stream_profile_validator() {
        Ok(validator) => validator,
        Err(error) => {
            return vec![Issue {
                level: Level::Error,
                code: E_XSD.to_string(),
                message: format!("stream-profile XSD schema unavailable: {error}"),
            }]
        }
    };
    let document = match parse(xml) {
        Ok(document) => document,
        Err(error) => {
            return vec![Issue {
                level: Level::Error,
                code: E_XSD.to_string(),
                message: format!("not well-formed XML: {error}"),
            }]
        }
    };
    validator
        .validate(&document)
        .into_iter()
        .map(|error| Issue {
            level: Level::Error,
            code: E_XSD.to_string(),
            message: error.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal schema-VALID HCDF document (one comp, no geometry): the accept baseline.
    const MINIMAL_VALID: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base"/>
</hcdf>
"#;

    #[test]
    fn accepts_a_minimal_valid_doc() {
        assert!(
            validate_xsd(MINIMAL_VALID).is_empty(),
            "a minimal valid HCDF document must pass XSD validation"
        );
    }

    #[test]
    fn rejects_not_well_formed_xml() {
        let issues = validate_xsd("<hcdf not valid <<<");
        assert!(!issues.is_empty(), "non-well-formed XML must be rejected");
        assert!(issues
            .iter()
            .all(|i| i.code == E_XSD && i.level == Level::Error));
    }

    /// The FIRST schema-shape bug: a `<box>` with raw text where a `<size>` CHILD is required. The typed
    /// parse collapses this to an empty `<box/>`, so only XSD validation catches it.
    #[test]
    fn rejects_box_without_size_child() {
        let bad = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base">
    <visual name="v">
      <geometry><box>.1 .1 .5</box></geometry>
    </visual>
  </comp>
</hcdf>
"#;
        assert!(
            !validate_xsd(bad).is_empty(),
            "<box> with raw text instead of a <size> child must be rejected"
        );
    }

    /// The correctly-formed `<box>` (a `<size>` CHILD) must be ACCEPTED: the positive control for the
    /// box-shape rejection above.
    #[test]
    fn accepts_box_with_size_child() {
        let good = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base">
    <visual name="v">
      <geometry><box><size>0.1 0.1 0.5</size></box></geometry>
    </visual>
  </comp>
</hcdf>
"#;
        assert!(
            validate_xsd(good).is_empty(),
            "<box> with a <size> child is valid and must be accepted; got: {:?}",
            validate_xsd(good)
        );
    }

    /// The SECOND schema-shape bug: a top-level `<color>` with an `<rgba>` CHILD where an `rgba=` ATTRIBUTE
    /// is required (the `color` complexType has attributes only, no child elements). The typed parse
    /// collapses this to an empty `<color/>`, so only XSD validation catches it.
    #[test]
    fn rejects_color_with_rgba_child() {
        let bad = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base"/>
  <color name="rubber"><rgba>0.1 0.1 0.1 1.0</rgba></color>
</hcdf>
"#;
        assert!(
            !validate_xsd(bad).is_empty(),
            "<color> with an <rgba> child instead of an rgba= attribute must be rejected"
        );
    }

    /// The correctly-formed `<color>` (an `rgba=` ATTRIBUTE) must be ACCEPTED: the positive control for
    /// the color-shape rejection above.
    #[test]
    fn accepts_color_with_rgba_attribute() {
        let good = r#"<?xml version="1.0" encoding="UTF-8"?>
<hcdf name="t" version="1.0">
  <comp name="base"/>
  <color name="rubber" rgba="0.1 0.1 0.1 1.0"/>
</hcdf>
"#;
        assert!(
            validate_xsd(good).is_empty(),
            "<color rgba=…> is valid and must be accepted; got: {:?}",
            validate_xsd(good)
        );
    }

    #[test]
    fn schema_compiles_once_and_is_reused() {
        // Two calls must both succeed against the cached, once-compiled validator.
        assert!(validate_xsd(MINIMAL_VALID).is_empty());
        assert!(validate_xsd(MINIMAL_VALID).is_empty());
    }

    /// The stream-profile schema gate the retired `tests/run_tests.py` used to own. The fixture is
    /// compiled into the crate, and validation uses the cached embedded sidecar schema through the
    /// same uppsala engine as [`validate_xsd`].
    #[test]
    fn stream_profile_example_is_schema_valid() {
        let instance = include_str!("../tests/fixtures/typed-stream-profile.xml");
        let errors = validate_stream_profile_xsd(instance);
        assert!(
            errors.is_empty(),
            "typed stream-profile fixture must validate against the embedded schema: {errors:?}"
        );
        assert!(validate_stream_profile_xsd(instance).is_empty());
    }

    #[test]
    fn operational_stream_profile_is_schema_valid() {
        let instance = include_str!("../../../examples/profiles/operational.streams.xml");
        let errors = validate_stream_profile_xsd(instance);
        assert!(
            errors.is_empty(),
            "operational stream-profile fixture must validate against the embedded schema: {errors:?}"
        );
    }

    #[test]
    fn stream_profile_validator_rejects_core_resource_shape() {
        assert!(!validate_stream_profile_xsd(
            r#"<stream-profile uri="profiles/operational.streams.xml"/>"#
        )
        .is_empty());
    }
}
