//! HCDF version (MAJOR.MINOR) and the compatibility policy from hcdformat docs/design/versioning.md.
//!
//! Same MAJOR is forward/backward compatible: a reader ignores unknown elements/attributes from a
//! newer MINOR. A foreign MAJOR is rejected. Enforcement lives in the parser ([`check_readable`],
//! wired into [`Hcdf::from_xml_str`]); by design the XSD only checks that `@version` is
//! MAJOR.MINOR-shaped, so the schema validator can warn (not reject) on future same-MAJOR documents.
//!
//! [`Hcdf::from_xml_str`]: crate::model::Hcdf::from_xml_str
use crate::error::{Error, Result};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HcdfVersion {
    pub major: u32,
    pub minor: u32,
}

impl HcdfVersion {
    pub const V1_0: HcdfVersion = HcdfVersion { major: 1, minor: 0 };

    /// Parse "MAJOR.MINOR" (e.g. "1.0"). Returns None if malformed.
    pub fn parse(s: &str) -> Option<HcdfVersion> {
        let (a, b) = s.split_once('.')?;
        Some(HcdfVersion {
            major: a.trim().parse().ok()?,
            minor: b.trim().parse().ok()?,
        })
    }

    /// A document of `self` version can be read by tooling supporting `supported` iff same MAJOR.
    pub fn readable_by(&self, supported: HcdfVersion) -> bool {
        self.major == supported.major
    }
}

impl fmt::Display for HcdfVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Versions this crate ships a vendored, sha-pinned schema for.
pub const SUPPORTED_VERSIONS: &[HcdfVersion] = &[HcdfVersion::V1_0];

/// Gate a document's root `@version` against [`SUPPORTED_VERSIONS`]: an absent/empty attribute
/// reads as 1.0 (the schema default), a same-MAJOR newer MINOR loads (forward compatibility), and a
/// malformed value or a MAJOR no supported version can read is [`Error::UnsupportedVersion`] naming
/// both the found and the supported version(s).
pub fn check_readable(version: &str) -> Result<HcdfVersion> {
    if version.is_empty() {
        return Ok(HcdfVersion::V1_0);
    }
    HcdfVersion::parse(version)
        .filter(|v| SUPPORTED_VERSIONS.iter().any(|s| v.readable_by(*s)))
        .ok_or_else(|| Error::UnsupportedVersion {
            found: version.to_string(),
            supported: SUPPORTED_VERSIONS
                .iter()
                .map(HcdfVersion::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_and_compat() {
        assert_eq!(HcdfVersion::parse("1.0"), Some(HcdfVersion::V1_0));
        assert!(HcdfVersion { major: 1, minor: 3 }.readable_by(HcdfVersion::V1_0));
        assert!(!HcdfVersion { major: 2, minor: 0 }.readable_by(HcdfVersion::V1_0));
        assert_eq!(HcdfVersion::parse("nope"), None);
    }

    #[test]
    fn check_readable_policy() {
        // absent/empty defaults to 1.0; same-MAJOR minors load; foreign MAJOR and malformed reject.
        assert_eq!(check_readable("").unwrap(), HcdfVersion::V1_0);
        assert_eq!(check_readable("1.0").unwrap(), HcdfVersion::V1_0);
        assert_eq!(
            check_readable("1.3").unwrap(),
            HcdfVersion { major: 1, minor: 3 }
        );
        for bad in ["2.0", "nope", "1", "1.x"] {
            let err = check_readable(bad).unwrap_err();
            assert!(
                matches!(&err, Error::UnsupportedVersion { found, supported }
                    if found == bad && supported == "1.0"),
                "{bad}: unexpected error {err:?}"
            );
        }
    }
}
