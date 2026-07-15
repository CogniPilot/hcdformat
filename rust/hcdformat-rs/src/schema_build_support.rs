//! Validation and identifier helpers shared by the schema pin build script and its tests.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

/// Validate a schema filename from `HASHES` as a portable relative path.
pub(crate) fn safe_relative_schema_path(raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() {
        return Err("schema path is empty".to_owned());
    }
    if raw.starts_with('/') {
        return Err(format!("schema path must be relative: {raw:?}"));
    }
    if raw.contains('\\') {
        return Err(format!(
            "schema path must use portable forward-slash separators: {raw:?}"
        ));
    }
    if raw.as_bytes().get(1) == Some(&b':') && raw.as_bytes()[0].is_ascii_alphabetic() {
        return Err(format!(
            "schema path must not contain a drive prefix: {raw:?}"
        ));
    }
    if raw.chars().any(char::is_control) {
        return Err(format!("schema path contains a control character: {raw:?}"));
    }
    if raw
        .split('/')
        .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(format!("schema path contains an unsafe component: {raw:?}"));
    }

    let path = Path::new(raw);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "schema path must be relative and normalized: {raw:?}"
        ));
    }
    Ok(path.to_owned())
}

/// Build a Rust constant identifier from the schema path and schema version.
pub(crate) fn schema_const_ident(file: &str, version: &str) -> Result<String, String> {
    if file.is_empty() || version.is_empty() {
        return Err("schema filename and version must not be empty".to_owned());
    }

    let raw = format!("{file}_SHA256_{version}");
    let mut ident = String::with_capacity(raw.len() + 1);
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            ident.push(character.to_ascii_uppercase());
        } else {
            ident.push('_');
        }
    }
    if ident.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        ident.insert(0, '_');
    }
    Ok(ident)
}

/// Reserve an emitted constant name and reject sanitized identifier collisions.
pub(crate) fn reserve_schema_const(
    seen: &mut BTreeMap<String, (String, String)>,
    file: &str,
    version: &str,
) -> Result<String, String> {
    let ident = schema_const_ident(file, version)?;
    let origin = (file.to_owned(), version.to_owned());
    if let Some(previous) = seen.insert(ident.clone(), origin.clone()) {
        seen.insert(ident.clone(), previous.clone());
        return Err(format!(
            "schema constant {ident} collides for {} v{} and {} v{}",
            previous.0, previous.1, origin.0, origin.1
        ));
    }
    Ok(ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normalized_nested_relative_paths() {
        assert_eq!(
            safe_relative_schema_path("profiles/common/hcdf-core.xsd").unwrap(),
            PathBuf::from("profiles/common/hcdf-core.xsd")
        );
    }

    #[test]
    fn rejects_unsafe_or_nonportable_paths() {
        for path in [
            "",
            "/absolute.xsd",
            "../escape.xsd",
            "schemas/../escape.xsd",
            "./schema.xsd",
            "schemas//schema.xsd",
            "schemas\\schema.xsd",
            "C:/schema.xsd",
            "schema\0.xsd",
        ] {
            assert!(
                safe_relative_schema_path(path).is_err(),
                "unexpectedly accepted {path:?}"
            );
        }
    }

    #[test]
    fn sanitizes_every_non_alphanumeric_character() {
        assert_eq!(
            schema_const_ident("nested/hcdf.profile-v1+xsd", "1.0-rc+2").unwrap(),
            "NESTED_HCDF_PROFILE_V1_XSD_SHA256_1_0_RC_2"
        );
        assert_eq!(
            schema_const_ident("1schema.xsd", "1.0").unwrap(),
            "_1SCHEMA_XSD_SHA256_1_0"
        );
    }

    #[test]
    fn rejects_sanitized_identifier_collisions() {
        let mut seen = BTreeMap::new();
        assert_eq!(
            reserve_schema_const(&mut seen, "a-b.xsd", "1.0").unwrap(),
            "A_B_XSD_SHA256_1_0"
        );
        let error = reserve_schema_const(&mut seen, "a_b.xsd", "1.0").unwrap_err();
        assert!(error.contains("collides"));
        assert!(error.contains("a-b.xsd"));
        assert!(error.contains("a_b.xsd"));
    }
}
