//! Lexical resolution for HCDF document keys and opaque asset references.
//!
//! Logical resource resolution is intentionally narrower than general RFC 3986
//! resolution. It normalizes only path segments and preserves query and fragment
//! suffixes byte-for-byte. Filesystem asset rerooting remains a separate adapter
//! because native path roots and remote URL origins have different semantics.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceReferenceError {
    #[error("resource URI must not be empty")]
    EmptyReference,
    #[error(
        "relative resource URI {reference:?} cannot be resolved against opaque parent {parent:?}"
    )]
    OpaqueParent { parent: String, reference: String },
}

/// Resolves a logical resource-path reference against a parent resource key.
///
/// Hierarchical parents may use an authority, a scheme-rooted path, a
/// network-path authority, an absolute path, or a relative path. Resolution does
/// not percent-decode, normalize hosts, inherit authorities, or imply fetch
/// semantics. Absolute and explicitly schemed references are returned byte-for-byte.
pub fn resolve_resource_reference(
    parent: &str,
    reference: &str,
) -> Result<String, ResourceReferenceError> {
    if reference.trim().is_empty() {
        return Err(ResourceReferenceError::EmptyReference);
    }
    if reference.starts_with('/') || has_uri_scheme(reference) {
        return Ok(reference.to_owned());
    }
    let (parent_path, _) = split_suffix(parent);
    let parent_path =
        LexicalPath::parse(parent_path).ok_or_else(|| ResourceReferenceError::OpaqueParent {
            parent: parent.to_owned(),
            reference: reference.to_owned(),
        })?;
    let (reference_path, suffix) = split_suffix(reference);
    if reference_path.is_empty() {
        return Ok(format!("{}{suffix}", parent_path.original));
    }
    let directory = parent_path
        .path
        .rsplit_once('/')
        .map(|(base, _)| base)
        .unwrap_or("");
    let joined = if directory.is_empty() {
        reference_path.to_owned()
    } else {
        format!("{directory}/{reference_path}")
    };
    let normalized = normalize_logical_segments(&joined, parent_path.is_rooted());
    Ok(format!("{}{suffix}", parent_path.render(&normalized)))
}

/// Reroots a relative opaque asset URI against a native filesystem directory.
pub(crate) fn reroot_filesystem_asset(base: &Path, reference: &str) -> String {
    if reference.is_empty() || has_uri_scheme(reference) {
        return reference.to_owned();
    }
    let (path, suffix) = split_suffix(reference);
    if path.is_empty() || Path::new(path).is_absolute() {
        return reference.to_owned();
    }
    let rooted = normalize_filesystem_path(&normalize_filesystem_path(base).join(path));
    format!("{}{suffix}", rooted.to_string_lossy())
}

pub(crate) fn has_uri_scheme(value: &str) -> bool {
    let Some(colon) = value.find(':') else {
        return false;
    };
    let scheme = &value[..colon];
    !scheme.is_empty()
        && scheme.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_alphabetic()
            } else {
                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.')
            }
        })
}

pub(crate) fn split_suffix(value: &str) -> (&str, &str) {
    let suffix_start = value.find(['?', '#']).unwrap_or(value.len());
    value.split_at(suffix_start)
}

pub(crate) fn normalize_filesystem_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LexicalPathRoot {
    Authority(String),
    SchemeRoot(String),
    NetworkAuthority(String),
    Absolute,
    Relative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LexicalPath<'a> {
    original: &'a str,
    root: LexicalPathRoot,
    path: &'a str,
}

impl<'a> LexicalPath<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        if has_uri_scheme(value) {
            let colon = value.find(':')?;
            let scheme = &value[..colon];
            let remainder = &value[colon + 1..];
            if let Some(authority_and_path) = remainder.strip_prefix("//") {
                let (authority, path) = authority_and_path
                    .split_once('/')
                    .unwrap_or((authority_and_path, ""));
                return Some(Self {
                    original: value,
                    root: LexicalPathRoot::Authority(format!("{scheme}://{authority}")),
                    path,
                });
            }
            let path = remainder.strip_prefix('/')?;
            return Some(Self {
                original: value,
                root: LexicalPathRoot::SchemeRoot(format!("{scheme}:")),
                path,
            });
        }
        if let Some(authority_and_path) = value.strip_prefix("//") {
            let (authority, path) = authority_and_path
                .split_once('/')
                .unwrap_or((authority_and_path, ""));
            return Some(Self {
                original: value,
                root: LexicalPathRoot::NetworkAuthority(format!("//{authority}")),
                path,
            });
        }
        if let Some(path) = value.strip_prefix('/') {
            return Some(Self {
                original: value,
                root: LexicalPathRoot::Absolute,
                path,
            });
        }
        Some(Self {
            original: value,
            root: LexicalPathRoot::Relative,
            path: value,
        })
    }

    fn is_rooted(&self) -> bool {
        self.root != LexicalPathRoot::Relative
    }

    fn render(&self, normalized: &str) -> String {
        match (&self.root, normalized.is_empty()) {
            (LexicalPathRoot::Authority(prefix), true)
            | (LexicalPathRoot::NetworkAuthority(prefix), true) => format!("{prefix}/"),
            (LexicalPathRoot::Authority(prefix), false)
            | (LexicalPathRoot::NetworkAuthority(prefix), false) => {
                format!("{prefix}/{normalized}")
            }
            (LexicalPathRoot::SchemeRoot(prefix), true) => format!("{prefix}/"),
            (LexicalPathRoot::SchemeRoot(prefix), false) => format!("{prefix}/{normalized}"),
            (LexicalPathRoot::Absolute, true) => "/".to_owned(),
            (LexicalPathRoot::Absolute, false) => format!("/{normalized}"),
            (LexicalPathRoot::Relative, _) => normalized.to_owned(),
        }
    }
}

fn normalize_logical_segments(path: &str, rooted: bool) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|part| *part != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            value => parts.push(value),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_and_schemed_references_are_byte_exact() {
        for reference in [
            "/mem/0/assets/rotor.stl",
            "//host/assets/rotor.stl",
            "https://example.com/rotor.stl",
            "file:/C:/models/rotor.stl",
            "urn:example:rotor",
            "custom+v1:rotor",
            "data:model/gltf-binary;base64,AAAA",
        ] {
            assert_eq!(
                resolve_resource_reference("urn:opaque-parent", reference).unwrap(),
                reference
            );
        }
    }

    #[test]
    fn hierarchical_roots_and_parent_segments_are_lexical() {
        for (parent, reference, expected) in [
            (
                "https://example.com",
                "child.hcdf",
                "https://example.com/child.hcdf",
            ),
            (
                "https://example.com/root.hcdf",
                "../child.hcdf",
                "https://example.com/child.hcdf",
            ),
            (
                "https://example.com/a/root.hcdf",
                "../../child.hcdf",
                "https://example.com/child.hcdf",
            ),
            (
                "https://example.com?old=1#root",
                "child.hcdf?new=2#child",
                "https://example.com/child.hcdf?new=2#child",
            ),
            ("/mem/a/root.hcdf", "../../child.hcdf", "/child.hcdf"),
            ("/mem/a/root.hcdf", "../child.hcdf", "/mem/child.hcdf"),
            ("/root.hcdf", "child.hcdf", "/child.hcdf"),
            ("a/root.hcdf", "../../child.hcdf", "../child.hcdf"),
            ("a/b/root.hcdf", "../../child.hcdf", "child.hcdf"),
            (
                "file:///mem/root.hcdf",
                "../child.hcdf",
                "file:///child.hcdf",
            ),
            (
                "file:///mem/root.hcdf?old=1#root",
                "../child.hcdf?new=2#child",
                "file:///child.hcdf?new=2#child",
            ),
            (
                "file:///C:/models/root.hcdf",
                "../child.hcdf",
                "file:///C:/child.hcdf",
            ),
            (
                "file:/C:/models/root.hcdf",
                "../child.hcdf",
                "file:/C:/child.hcdf",
            ),
            ("C:/dir/root.hcdf", "../child.hcdf", "C:/child.hcdf"),
        ] {
            assert_eq!(
                resolve_resource_reference(parent, reference).unwrap(),
                expected,
                "parent {parent:?}, reference {reference:?}"
            );
        }
    }

    #[test]
    fn only_the_child_path_is_normalized() {
        for (parent, reference, expected) in [
            (
                "https://example.com/a/root.hcdf?old=1#old",
                "child.hcdf?new=2#node",
                "https://example.com/a/child.hcdf?new=2#node",
            ),
            (
                "https://example.com/a/root.hcdf?old=1#old",
                "../child.hcdf?next=/../asset.glb#part/../leaf",
                "https://example.com/child.hcdf?next=/../asset.glb#part/../leaf",
            ),
            (
                "https://example.com/a/root.hcdf?old=1#old",
                "?new=2",
                "https://example.com/a/root.hcdf?new=2",
            ),
            (
                "https://example.com/a/root.hcdf?old=1#old",
                "#node",
                "https://example.com/a/root.hcdf#node",
            ),
            (
                "/mem/a/root.hcdf?old=1",
                "?new=2#node",
                "/mem/a/root.hcdf?new=2#node",
            ),
            (
                "/mem/a/root.hcdf",
                "../child.hcdf#node",
                "/mem/child.hcdf#node",
            ),
        ] {
            assert_eq!(
                resolve_resource_reference(parent, reference).unwrap(),
                expected,
                "parent {parent:?}, reference {reference:?}"
            );
        }
    }

    #[test]
    fn scheme_rooted_paths_without_authority_are_supported() {
        for (parent, reference, expected) in [
            ("file:/mem/root.hcdf", "child.hcdf", "file:/mem/child.hcdf"),
            (
                "file:/mem/a/root.hcdf",
                "../../child.hcdf",
                "file:/child.hcdf",
            ),
            ("file:/root.hcdf", "../child.hcdf", "file:/child.hcdf"),
            (
                "file:/mem/root.hcdf?old=1",
                "child.hcdf?new=2#n",
                "file:/mem/child.hcdf?new=2#n",
            ),
        ] {
            assert_eq!(
                resolve_resource_reference(parent, reference).unwrap(),
                expected,
                "parent {parent:?}, reference {reference:?}"
            );
        }
    }

    #[test]
    fn network_authorities_and_authority_roots_are_preserved() {
        assert_eq!(
            resolve_resource_reference("//host/a/root.hcdf", "../child.hcdf").unwrap(),
            "//host/child.hcdf"
        );
        for (parent, expected) in [
            ("https://host/root.hcdf", "https://host/"),
            ("file:///root.hcdf", "file:///"),
            ("//host/root.hcdf", "//host/"),
        ] {
            assert_eq!(resolve_resource_reference(parent, "..").unwrap(), expected);
        }
    }

    #[test]
    fn opaque_parents_and_empty_references_are_typed_errors() {
        for reference in ["child.hcdf", "?query", "#fragment"] {
            assert!(matches!(
                resolve_resource_reference("urn:opaque-parent", reference),
                Err(ResourceReferenceError::OpaqueParent { .. })
            ));
        }
        assert_eq!(
            resolve_resource_reference("/mem/root.hcdf", "  ").unwrap_err(),
            ResourceReferenceError::EmptyReference
        );
    }

    #[test]
    fn filesystem_reroot_preserves_schemes_and_suffix_bytes() {
        let base = Path::new("/module/dir");
        for reference in [
            "urn:asset:visual",
            "file:/C:/models/collision.stl",
            "custom+v1:sensor",
            "data:model/gltf-binary;base64,AAAA",
            "package://robot/connector.glb",
            "model://robot/body.glb",
            "https://example.com/body.glb",
        ] {
            assert_eq!(reroot_filesystem_asset(base, reference), reference);
        }
        for reference in ["?variant=1", "#node", "?variant=1#node"] {
            assert_eq!(reroot_filesystem_asset(base, reference), reference);
        }
        let reference = "assets/model.glb?next=/../x#part/../y";
        let rerooted = reroot_filesystem_asset(base, reference);
        let (path, suffix) = split_suffix(&rerooted);
        assert_eq!(suffix, "?next=/../x#part/../y");
        assert_eq!(
            Path::new(path),
            normalize_filesystem_path(&base.join("assets/model.glb"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn filesystem_reroot_preserves_windows_absolute_and_unc_paths() {
        for reference in [
            r"C:\models\body.glb?variant=1#node",
            r"\\server\share\body.glb#node",
        ] {
            assert_eq!(
                reroot_filesystem_asset(Path::new(r"C:\module\dir"), reference),
                reference
            );
        }
    }
}
