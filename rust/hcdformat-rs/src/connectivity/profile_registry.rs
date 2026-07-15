//! Versioned, non-executable connectivity profile registry metadata.
//!
//! Registry data identifies an exact qualified profile, selects one closed Rust validator, and
//! records source citations. Electrical limits, topology rules, formulas, aliases, and all other
//! executable validation behavior remain in Rust.

use crate::model::connectivity::QualifiedId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

const REGISTRY_FORMAT: &str = "hcdf-connectivity-profile-registry";
const REGISTRY_FORMAT_VERSION: u32 = 1;
const HCDF_SCHEMA_MAJOR: u32 = 1;
const HCDF_SCHEMA_MINOR: u32 = 0;

/// A canonical three-part numeric registry or ruleset version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RegistryVersion(String);

impl RegistryVersion {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RegistryVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RegistryVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if valid_version(&value) {
            Ok(Self(value))
        } else {
            Err(serde::de::Error::custom(format!(
                "registry version {value:?} must be canonical MAJOR.MINOR.PATCH"
            )))
        }
    }
}

fn valid_version(value: &str) -> bool {
    let mut parts = value.split('.');
    let valid_part = |part: &str| {
        part == "0"
            || (part
                .as_bytes()
                .first()
                .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
                && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    (0..3).all(|_| parts.next().is_some_and(valid_part)) && parts.next().is_none()
}

/// HCDF schema releases accepted by a registry data release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct HcdfSchemaCompatibility {
    pub major: u32,
    pub minimum_minor: u32,
}

/// Stable citation key used by Rust validators to select publication metadata.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CitationKey(String);

impl CitationKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CitationKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CitationKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let mut bytes = value.bytes();
        let valid = bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(serde::de::Error::custom(format!(
                "citation key {value:?} must be lower-kebab-case"
            )))
        }
    }
}

/// Source provenance for a profile ruleset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCitation {
    pub key: CitationKey,
    pub organization: String,
    pub document: String,
    pub locator: String,
}

/// Closed Rust validator dispatch selected by trusted built-in registry data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidatorKind {
    BidirectionalDshot,
    Can,
    Dshot,
    FeetechSts,
    Generic,
    Pwm,
    Rs485,
    Uart,
    #[cfg(test)]
    CrossNetworkProbe,
}

impl ValidatorKind {
    pub const fn id(self) -> &'static str {
        match self {
            Self::BidirectionalDshot => "hcdf:validator/bidirectional-dshot",
            Self::Can => "hcdf:validator/can",
            Self::Dshot => "hcdf:validator/dshot",
            Self::FeetechSts => "hcdf:validator/feetech-sts",
            Self::Generic => "hcdf:validator/generic",
            Self::Pwm => "hcdf:validator/pwm",
            Self::Rs485 => "hcdf:validator/rs-485",
            Self::Uart => "hcdf:validator/uart",
            #[cfg(test)]
            Self::CrossNetworkProbe => "hcdf:validator/cross-network-probe",
        }
    }

    fn from_id(value: &QualifiedId) -> Option<Self> {
        match value.as_str() {
            "hcdf:validator/bidirectional-dshot" => Some(Self::BidirectionalDshot),
            "hcdf:validator/can" => Some(Self::Can),
            "hcdf:validator/dshot" => Some(Self::Dshot),
            "hcdf:validator/feetech-sts" => Some(Self::FeetechSts),
            "hcdf:validator/generic" => Some(Self::Generic),
            "hcdf:validator/pwm" => Some(Self::Pwm),
            "hcdf:validator/rs-485" => Some(Self::Rs485),
            "hcdf:validator/uart" => Some(Self::Uart),
            #[cfg(test)]
            "hcdf:validator/cross-network-probe" => Some(Self::CrossNetworkProbe),
            _ => None,
        }
    }
}

/// One validated active ruleset for one exact profile identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRuleset {
    profile: QualifiedId,
    ruleset_version: RegistryVersion,
    validator: ValidatorKind,
    citations: Vec<ProfileCitation>,
}

impl ProfileRuleset {
    pub fn profile(&self) -> &QualifiedId {
        &self.profile
    }

    pub fn ruleset_version(&self) -> &RegistryVersion {
        &self.ruleset_version
    }

    pub fn validator(&self) -> ValidatorKind {
        self.validator
    }

    pub fn citations(&self) -> &[ProfileCitation] {
        &self.citations
    }

    pub fn citation(&self, key: &str) -> Option<&ProfileCitation> {
        self.citations
            .binary_search_by(|citation| citation.key.as_str().cmp(key))
            .ok()
            .map(|index| &self.citations[index])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RegistryDocument {
    format: String,
    format_version: u32,
    registry_version: RegistryVersion,
    hcdf_schema: HcdfSchemaCompatibility,
    rulesets: Vec<RulesetDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RulesetDocument {
    profile: QualifiedId,
    ruleset_version: RegistryVersion,
    validator: QualifiedId,
    citations: Vec<ProfileCitation>,
}

/// Parsed registry plus an exact, authority-sensitive profile index.
#[derive(Debug, Clone)]
pub struct ProfileRegistry {
    document: RegistryDocument,
    rulesets: Vec<ProfileRuleset>,
    by_profile: BTreeMap<QualifiedId, usize>,
}

impl ProfileRegistry {
    pub fn registry_version(&self) -> &RegistryVersion {
        &self.document.registry_version
    }

    pub fn hcdf_schema(&self) -> HcdfSchemaCompatibility {
        self.document.hcdf_schema
    }

    pub fn rulesets(&self) -> &[ProfileRuleset] {
        &self.rulesets
    }

    /// Exact lookup of the complete QualifiedId. No case folding, aliases, or local-part matching occur.
    pub fn lookup(&self, profile: &QualifiedId) -> Option<&ProfileRuleset> {
        self.by_profile
            .get(profile)
            .map(|index| &self.rulesets[*index])
    }

    /// Deterministic representation used by the canonical artifact drift gate.
    pub fn to_canonical_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(&self.document)
            .expect("validated profile registry always serializes");
        text.push('\n');
        text
    }
}

/// Strict profile registry parse or contract failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfileRegistryError {
    #[error("invalid connectivity profile registry JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported connectivity profile registry format {0:?}")]
    UnsupportedFormat(String),
    #[error("unsupported connectivity profile registry format version {0}")]
    UnsupportedFormatVersion(u32),
    #[error(
        "connectivity profile registry requires HCDF {required_major}.{required_minor} or newer in that major; this build supports {supported_major}.{supported_minor}"
    )]
    IncompatibleHcdfSchema {
        required_major: u32,
        required_minor: u32,
        supported_major: u32,
        supported_minor: u32,
    },
    #[error("duplicate profile ruleset {0}")]
    DuplicateProfile(QualifiedId),
    #[error("profile rulesets are not sorted: {previous} precedes {current}")]
    UnsortedProfiles {
        previous: QualifiedId,
        current: QualifiedId,
    },
    #[error("profile {profile} selects unsupported validator {validator}")]
    UnsupportedValidator {
        profile: QualifiedId,
        validator: QualifiedId,
    },
    #[error("profile {0} must carry at least one citation")]
    MissingCitations(QualifiedId),
    #[error("profile {profile} repeats citation key {key}")]
    DuplicateCitation {
        profile: QualifiedId,
        key: CitationKey,
    },
    #[error("profile {profile} citation keys are not sorted: {previous} precedes {current}")]
    UnsortedCitations {
        profile: QualifiedId,
        previous: CitationKey,
        current: CitationKey,
    },
    #[error("profile {profile} citation {key} has a blank {field}")]
    BlankCitationField {
        profile: QualifiedId,
        key: CitationKey,
        field: &'static str,
    },
}

/// Parse trusted built-in registry bytes and validate their non-executable metadata contract.
fn parse_profile_registry(bytes: &[u8]) -> Result<ProfileRegistry, ProfileRegistryError> {
    let document: RegistryDocument = serde_json::from_slice(bytes)
        .map_err(|error| ProfileRegistryError::InvalidJson(error.to_string()))?;
    validate_document(document)
}

fn validate_document(document: RegistryDocument) -> Result<ProfileRegistry, ProfileRegistryError> {
    if document.format != REGISTRY_FORMAT {
        return Err(ProfileRegistryError::UnsupportedFormat(
            document.format.clone(),
        ));
    }
    if document.format_version != REGISTRY_FORMAT_VERSION {
        return Err(ProfileRegistryError::UnsupportedFormatVersion(
            document.format_version,
        ));
    }
    if document.hcdf_schema.major != HCDF_SCHEMA_MAJOR
        || document.hcdf_schema.minimum_minor > HCDF_SCHEMA_MINOR
    {
        return Err(ProfileRegistryError::IncompatibleHcdfSchema {
            required_major: document.hcdf_schema.major,
            required_minor: document.hcdf_schema.minimum_minor,
            supported_major: HCDF_SCHEMA_MAJOR,
            supported_minor: HCDF_SCHEMA_MINOR,
        });
    }

    let mut rulesets = Vec::with_capacity(document.rulesets.len());
    let mut by_profile = BTreeMap::new();
    let mut previous_profile: Option<&QualifiedId> = None;
    for raw in &document.rulesets {
        if let Some(previous) = previous_profile {
            match previous.cmp(&raw.profile) {
                std::cmp::Ordering::Equal => {
                    return Err(ProfileRegistryError::DuplicateProfile(raw.profile.clone()));
                }
                std::cmp::Ordering::Greater => {
                    return Err(ProfileRegistryError::UnsortedProfiles {
                        previous: previous.clone(),
                        current: raw.profile.clone(),
                    });
                }
                std::cmp::Ordering::Less => {}
            }
        }
        previous_profile = Some(&raw.profile);

        let validator = ValidatorKind::from_id(&raw.validator).ok_or_else(|| {
            ProfileRegistryError::UnsupportedValidator {
                profile: raw.profile.clone(),
                validator: raw.validator.clone(),
            }
        })?;
        validate_citations(&raw.profile, &raw.citations)?;
        let index = rulesets.len();
        by_profile.insert(raw.profile.clone(), index);
        rulesets.push(ProfileRuleset {
            profile: raw.profile.clone(),
            ruleset_version: raw.ruleset_version.clone(),
            validator,
            citations: raw.citations.clone(),
        });
    }

    Ok(ProfileRegistry {
        document,
        rulesets,
        by_profile,
    })
}

fn validate_citations(
    profile: &QualifiedId,
    citations: &[ProfileCitation],
) -> Result<(), ProfileRegistryError> {
    if citations.is_empty() {
        return Err(ProfileRegistryError::MissingCitations(profile.clone()));
    }
    let mut previous: Option<&CitationKey> = None;
    for citation in citations {
        if let Some(previous) = previous {
            match previous.cmp(&citation.key) {
                std::cmp::Ordering::Equal => {
                    return Err(ProfileRegistryError::DuplicateCitation {
                        profile: profile.clone(),
                        key: citation.key.clone(),
                    });
                }
                std::cmp::Ordering::Greater => {
                    return Err(ProfileRegistryError::UnsortedCitations {
                        profile: profile.clone(),
                        previous: previous.clone(),
                        current: citation.key.clone(),
                    });
                }
                std::cmp::Ordering::Less => {}
            }
        }
        previous = Some(&citation.key);
        for (field, value) in [
            ("organization", citation.organization.as_str()),
            ("document", citation.document.as_str()),
            ("locator", citation.locator.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ProfileRegistryError::BlankCitationField {
                    profile: profile.clone(),
                    key: citation.key.clone(),
                    field,
                });
            }
        }
    }
    Ok(())
}

/// Parse the sha-pinned built-in registry once for native and WASM consumers.
pub fn built_in_profile_registry() -> Result<&'static ProfileRegistry, ProfileRegistryError> {
    static REGISTRY: OnceLock<Result<ProfileRegistry, ProfileRegistryError>> = OnceLock::new();
    match REGISTRY
        .get_or_init(|| parse_profile_registry(crate::schema::CONNECTIVITY_PROFILES_JSON_1_0))
    {
        Ok(registry) => Ok(registry),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::graph::IssueLevel;
    use super::super::identity::ObjectKind;
    use super::super::normalize::{
        normalize_connectivity, normalize_connectivity_with_registry, profile_registry_failure,
        NormalizationOptions, ProfileRegistryCompleteness,
    };
    use super::*;
    use crate::model::connectivity as authored;
    use std::collections::BTreeSet;

    fn registry_json(rulesets: &str) -> Vec<u8> {
        format!(
            r#"{{
  "format": "hcdf-connectivity-profile-registry",
  "format-version": 1,
  "registry-version": "1.0.0",
  "hcdf-schema": {{"major": 1, "minimum-minor": 0}},
  "rulesets": {rulesets}
}}"#
        )
        .into_bytes()
    }

    fn citation(key: &str) -> String {
        format!(
            r#"{{"key":"{key}","organization":"Standards Body","document":"STD-1","locator":"Section 1"}}"#
        )
    }

    fn ruleset(profile: &str, validator: &str, citations: &str) -> String {
        format!(
            r#"{{"profile":"{profile}","ruleset-version":"1.0.0","validator":"{validator}","citations":{citations}}}"#
        )
    }

    fn fixture_registry() -> ProfileRegistry {
        let ruleset = ruleset(
            "hcdf:test-link",
            "hcdf:validator/generic",
            &format!("[{}]", citation("source")),
        );
        parse_profile_registry(&registry_json(&format!("[{ruleset}]"))).unwrap()
    }

    fn cross_network_probe_registry() -> ProfileRegistry {
        let ruleset = ruleset(
            "hcdf:test-link",
            "hcdf:validator/cross-network-probe",
            &format!("[{}]", citation("source")),
        );
        parse_profile_registry(&registry_json(&format!("[{ruleset}]"))).unwrap()
    }

    fn profiled_document(
        profile: &str,
        network_names: &[&str],
        invalid_voltage_dimension: bool,
    ) -> authored::ConnectivityDocument {
        let profile = QualifiedId::new(profile).unwrap();
        let mut document = authored::ConnectivityDocument::new(
            authored::DocumentIdentity::new("memory://profile-dispatch/root.hcdf").unwrap(),
        );
        let scope = &mut document.scopes[0];
        for component in ["a", "b"] {
            let capabilities = authored::Capabilities {
                purposes: BTreeSet::from([authored::Purpose::Communication]),
                carriers: BTreeSet::from([authored::Carrier::Electrical]),
                profiles: BTreeSet::from([profile.clone()]),
                limits: authored::CapabilityLimits::default(),
            };
            scope.structural_anchors.push(authored::StructuralAnchors {
                component: component.to_owned(),
                visuals: Vec::new(),
                frames: Vec::new(),
            });
            scope.components.push(authored::ComponentConnectivity {
                component: component.to_owned(),
                ports: vec![authored::Port {
                    name: "p".to_owned(),
                    capabilities,
                    channels: Vec::new(),
                }],
                connectors: Vec::new(),
                antennas: Vec::new(),
                functions: Vec::new(),
                paths: Vec::new(),
                junctions: Vec::new(),
                terminations: Vec::new(),
            });
        }
        for name in network_names {
            scope.networks.push(authored::Network {
                name: (*name).to_owned(),
                structure: authored::NetworkStructure::Link,
                description: None,
                selected: authored::NetworkSelection {
                    purpose: authored::Purpose::Communication,
                    carrier: authored::Carrier::Electrical,
                    profiles: BTreeSet::from([profile.clone()]),
                    rate: None,
                    voltage: invalid_voltage_dimension.then(|| {
                        authored::SelectionQuantity::Nominal(authored::Quantity {
                            value: 1.0,
                            unit: "A".to_owned(),
                        })
                    }),
                    current: None,
                    power: None,
                    impedance: None,
                    frequency: None,
                    rf: None,
                    pressure: None,
                    flow: None,
                    temperature: None,
                },
                participants: ["a", "b"]
                    .into_iter()
                    .map(|component| authored::Participant {
                        name: component.to_owned(),
                        endpoint: authored::FunctionalEndpointRef::Port(authored::PortRef::local(
                            component, "p",
                        )),
                        role: None,
                    })
                    .collect(),
                configuration: authored::NetworkConfiguration::default(),
            });
        }
        document
    }

    fn rs485_document(
        profile: &str,
        structure: authored::NetworkStructure,
        purpose: authored::Purpose,
        carrier: authored::Carrier,
    ) -> authored::ConnectivityDocument {
        let mut document = profiled_document(profile, &["n"], false);
        {
            let scope = &mut document.scopes[0];
            let network = &mut scope.networks[0];
            network.structure = structure;
            network.selected.purpose = purpose;
            network.selected.carrier = carrier;
            for component in &mut scope.components {
                let capabilities = &mut component.ports[0].capabilities;
                capabilities.purposes.insert(purpose);
                capabilities.carriers.insert(carrier);
            }
        }
        document
    }

    fn selected_profiles_document(
        profiles: &[&str],
        structure: authored::NetworkStructure,
        purpose: authored::Purpose,
        carrier: authored::Carrier,
    ) -> authored::ConnectivityDocument {
        let (first, additional) = profiles.split_first().expect("at least one profile");
        let mut document = rs485_document(first, structure, purpose, carrier);
        let scope = &mut document.scopes[0];
        for profile in additional {
            let profile = QualifiedId::new(*profile).unwrap();
            scope.networks[0].selected.profiles.insert(profile.clone());
            for component in &mut scope.components {
                component.ports[0]
                    .capabilities
                    .profiles
                    .insert(profile.clone());
            }
        }
        document
    }

    #[test]
    fn exact_qualified_lookup_and_citation_lookup() {
        let one = ruleset(
            "hcdf:rs-485",
            "hcdf:validator/rs-485",
            &format!("[{}]", citation("physical-layer")),
        );
        let registry = parse_profile_registry(&registry_json(&format!("[{one}]"))).unwrap();
        let exact = QualifiedId::new("hcdf:rs-485").unwrap();
        let ruleset = registry.lookup(&exact).expect("exact profile exists");
        assert_eq!(ruleset.validator(), ValidatorKind::Rs485);
        assert_eq!(ruleset.validator().id(), "hcdf:validator/rs-485");
        assert_eq!(
            ruleset
                .citation("physical-layer")
                .expect("citation exists")
                .document,
            "STD-1"
        );
        for different in ["vendor:rs-485", "HCDF:rs-485", "hcdf:RS-485"] {
            assert!(registry
                .lookup(&QualifiedId::new(different).unwrap())
                .is_none());
        }
    }

    #[test]
    fn can_accepts_communication_over_an_electrical_bus() {
        let document = selected_profiles_document(
            &["hcdf:can"],
            authored::NetworkStructure::Bus,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let graph = normalize_connectivity(&document).unwrap();
        assert!(graph.warnings().is_empty());
    }

    #[test]
    fn can_rejects_incompatible_selected_axes_and_topology_once() {
        for (structure, purpose, carrier, expected_code) in [
            (
                authored::NetworkStructure::Bus,
                authored::Purpose::PowerDelivery,
                authored::Carrier::Electrical,
                "E_CONN_CAN_PURPOSE",
            ),
            (
                authored::NetworkStructure::Bus,
                authored::Purpose::Communication,
                authored::Carrier::GuidedOptical,
                "E_CONN_CAN_CARRIER",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
                "E_CONN_CAN_TOPOLOGY",
            ),
        ] {
            let document = selected_profiles_document(&["hcdf:can"], structure, purpose, carrier);
            let error = normalize_connectivity(&document).unwrap_err();
            assert_eq!(error.issues().len(), 1);
            assert_eq!(error.issues()[0].code(), expected_code);
        }
    }

    #[test]
    fn can_dispatch_uses_only_the_exact_profile_identity() {
        let exact = selected_profiles_document(
            &["hcdf:can"],
            authored::NetworkStructure::Link,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&exact).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_CAN_TOPOLOGY");

        for different in ["vendor:can", "HCDF:can", "hcdf:CAN", "hcdf:can-fd"] {
            let document = selected_profiles_document(
                &[different],
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let graph = normalize_connectivity(&document).unwrap();
            assert_eq!(
                graph.warnings().len(),
                1,
                "unexpected result for {different}"
            );
            let issue = &graph.warnings()[0];
            assert_eq!(issue.code(), "W_CONN_PROFILE_INCOMPLETE");
            assert!(issue.message().contains(different));
        }
    }

    #[test]
    fn can_owns_bus_refinement_without_duplicating_rs485_topology() {
        let document = selected_profiles_document(
            &["hcdf:can", "hcdf:rs-485"],
            authored::NetworkStructure::Link,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&document).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_CAN_TOPOLOGY");
    }

    #[test]
    fn rs485_accepts_link_and_bus_topologies() {
        for structure in [
            authored::NetworkStructure::Link,
            authored::NetworkStructure::Bus,
        ] {
            let document = rs485_document(
                "hcdf:rs-485",
                structure,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let graph = normalize_connectivity(&document).unwrap();
            assert!(graph.warnings().is_empty());
        }
    }

    #[test]
    fn rs485_rejects_incompatible_selected_axes_and_topology() {
        for (structure, purpose, carrier, expected_code) in [
            (
                authored::NetworkStructure::Link,
                authored::Purpose::PowerDelivery,
                authored::Carrier::Electrical,
                "E_CONN_RS485_PURPOSE",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::GuidedOptical,
                "E_CONN_RS485_CARRIER",
            ),
            (
                authored::NetworkStructure::Mesh,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
                "E_CONN_RS485_TOPOLOGY",
            ),
        ] {
            let document = rs485_document("hcdf:rs-485", structure, purpose, carrier);
            let error = normalize_connectivity(&document).unwrap_err();
            assert_eq!(error.issues().len(), 1);
            assert_eq!(error.issues()[0].code(), expected_code);
        }
    }

    #[test]
    fn rs485_dispatch_uses_only_the_exact_profile_identity() {
        let exact = rs485_document(
            "hcdf:rs-485",
            authored::NetworkStructure::Mesh,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&exact).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_RS485_TOPOLOGY");

        for different in ["vendor:rs-485", "HCDF:rs-485", "hcdf:RS-485"] {
            let document = rs485_document(
                different,
                authored::NetworkStructure::Mesh,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let graph = normalize_connectivity(&document).unwrap();
            assert_eq!(
                graph.warnings().len(),
                1,
                "unexpected result for {different}"
            );
            let issue = &graph.warnings()[0];
            assert_eq!(issue.code(), "W_CONN_PROFILE_INCOMPLETE");
            assert!(issue.message().contains(different));
        }
    }

    #[test]
    fn uart_accepts_link_and_bus_topologies() {
        for structure in [
            authored::NetworkStructure::Link,
            authored::NetworkStructure::Bus,
        ] {
            let document = selected_profiles_document(
                &["hcdf:uart"],
                structure,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let graph = normalize_connectivity(&document).unwrap();
            assert!(graph.warnings().is_empty());
        }
    }

    #[test]
    fn uart_rejects_incompatible_selected_axes_and_topology() {
        for (structure, purpose, carrier, expected_code) in [
            (
                authored::NetworkStructure::Link,
                authored::Purpose::PowerDelivery,
                authored::Carrier::Electrical,
                "E_CONN_UART_PURPOSE",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::GuidedOptical,
                "E_CONN_UART_CARRIER",
            ),
            (
                authored::NetworkStructure::Mesh,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
                "E_CONN_UART_TOPOLOGY",
            ),
        ] {
            let document = selected_profiles_document(&["hcdf:uart"], structure, purpose, carrier);
            let error = normalize_connectivity(&document).unwrap_err();
            assert_eq!(error.issues().len(), 1);
            assert_eq!(error.issues()[0].code(), expected_code);
        }
    }

    #[test]
    fn uart_dispatch_uses_only_the_exact_profile_identity() {
        let exact = selected_profiles_document(
            &["hcdf:uart"],
            authored::NetworkStructure::Mesh,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&exact).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_UART_TOPOLOGY");

        for different in ["vendor:uart", "HCDF:uart", "hcdf:UART"] {
            let document = selected_profiles_document(
                &[different],
                authored::NetworkStructure::Mesh,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let graph = normalize_connectivity(&document).unwrap();
            assert_eq!(
                graph.warnings().len(),
                1,
                "unexpected result for {different}"
            );
            assert_eq!(graph.warnings()[0].code(), "W_CONN_PROFILE_INCOMPLETE");
        }
    }

    #[test]
    fn dshot_accepts_only_communication_electrical_links() {
        let valid = selected_profiles_document(
            &["hcdf:dshot"],
            authored::NetworkStructure::Link,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let graph = normalize_connectivity(&valid).unwrap();
        assert!(graph.warnings().is_empty());

        for (structure, purpose, carrier, expected_code) in [
            (
                authored::NetworkStructure::Bus,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
                "E_CONN_DSHOT_TOPOLOGY",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::PowerDelivery,
                authored::Carrier::Electrical,
                "E_CONN_DSHOT_PURPOSE",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::GuidedOptical,
                "E_CONN_DSHOT_CARRIER",
            ),
        ] {
            let document = selected_profiles_document(&["hcdf:dshot"], structure, purpose, carrier);
            let error = normalize_connectivity(&document).unwrap_err();
            assert_eq!(error.issues().len(), 1);
            assert_eq!(error.issues()[0].code(), expected_code);
        }
    }

    #[test]
    fn dshot_dispatch_uses_only_the_exact_profile_identity() {
        let exact = selected_profiles_document(
            &["hcdf:dshot"],
            authored::NetworkStructure::Bus,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&exact).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_DSHOT_TOPOLOGY");

        for different in ["vendor:dshot", "HCDF:dshot", "hcdf:DSHOT"] {
            let document = selected_profiles_document(
                &[different],
                authored::NetworkStructure::Bus,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let graph = normalize_connectivity(&document).unwrap();
            assert_eq!(graph.warnings().len(), 1);
            assert_eq!(graph.warnings()[0].code(), "W_CONN_PROFILE_INCOMPLETE");
        }
    }

    #[test]
    fn bidirectional_dshot_requires_the_exact_dshot_base_profile() {
        for profiles in [
            vec!["hcdf:bidirectional-dshot"],
            vec!["hcdf:bidirectional-dshot", "HCDF:dshot"],
            vec!["hcdf:bidirectional-dshot", "hcdf:DSHOT"],
            vec!["hcdf:bidirectional-dshot", "vendor:dshot"],
        ] {
            let document = selected_profiles_document(
                &profiles,
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let error = normalize_connectivity(&document).unwrap_err();
            assert!(error
                .issues()
                .iter()
                .any(|issue| issue.code() == "E_CONN_BIDIRECTIONAL_DSHOT_BASE_PROFILE"));
        }
    }

    #[test]
    fn dshot_owns_bidirectional_dshot_axis_and_topology_diagnostics() {
        let valid = selected_profiles_document(
            &["hcdf:bidirectional-dshot", "hcdf:dshot"],
            authored::NetworkStructure::Link,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let graph = normalize_connectivity(&valid).unwrap();
        assert!(graph.warnings().is_empty());

        for (structure, purpose, carrier, expected_code) in [
            (
                authored::NetworkStructure::Bus,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
                "E_CONN_DSHOT_TOPOLOGY",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::PowerDelivery,
                authored::Carrier::Electrical,
                "E_CONN_DSHOT_PURPOSE",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::GuidedOptical,
                "E_CONN_DSHOT_CARRIER",
            ),
        ] {
            let document = selected_profiles_document(
                &["hcdf:bidirectional-dshot", "hcdf:dshot"],
                structure,
                purpose,
                carrier,
            );
            let error = normalize_connectivity(&document).unwrap_err();
            assert_eq!(error.issues().len(), 1);
            assert_eq!(error.issues()[0].code(), expected_code);
        }
    }

    #[test]
    fn pwm_accepts_only_communication_electrical_links() {
        let valid = selected_profiles_document(
            &["hcdf:pwm"],
            authored::NetworkStructure::Link,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let graph = normalize_connectivity(&valid).unwrap();
        assert!(graph.warnings().is_empty());

        for (structure, purpose, carrier, expected_code) in [
            (
                authored::NetworkStructure::Bus,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
                "E_CONN_PWM_TOPOLOGY",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::PowerDelivery,
                authored::Carrier::Electrical,
                "E_CONN_PWM_PURPOSE",
            ),
            (
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::GuidedOptical,
                "E_CONN_PWM_CARRIER",
            ),
        ] {
            let document = selected_profiles_document(&["hcdf:pwm"], structure, purpose, carrier);
            let error = normalize_connectivity(&document).unwrap_err();
            assert_eq!(error.issues().len(), 1);
            assert_eq!(error.issues()[0].code(), expected_code);
        }
    }

    #[test]
    fn pwm_dispatch_uses_only_the_exact_profile_identity() {
        let exact = selected_profiles_document(
            &["hcdf:pwm"],
            authored::NetworkStructure::Bus,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&exact).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_PWM_TOPOLOGY");

        for different in ["vendor:pwm", "HCDF:pwm", "hcdf:PWM"] {
            let document = selected_profiles_document(
                &[different],
                authored::NetworkStructure::Bus,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let graph = normalize_connectivity(&document).unwrap();
            assert_eq!(graph.warnings().len(), 1);
            assert_eq!(graph.warnings()[0].code(), "W_CONN_PROFILE_INCOMPLETE");
        }
    }

    #[test]
    fn feetech_sts_requires_explicit_uart_base_before_topology_refinement() {
        for structure in [
            authored::NetworkStructure::Link,
            authored::NetworkStructure::Mesh,
        ] {
            let document = selected_profiles_document(
                &["feetech:sts"],
                structure,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let error = normalize_connectivity(&document).unwrap_err();
            assert_eq!(error.issues().len(), 1);
            assert_eq!(error.issues()[0].code(), "E_CONN_FEETECH_STS_BASE_PROFILE");
        }
    }

    #[test]
    fn feetech_sts_does_not_accept_nearby_uart_identities_as_its_base() {
        for different in ["HCDF:uart", "hcdf:UART", "vendor:uart"] {
            let document = selected_profiles_document(
                &["feetech:sts", different],
                authored::NetworkStructure::Link,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let error = normalize_connectivity(&document).unwrap_err();
            let codes = error
                .issues()
                .iter()
                .map(|issue| issue.code())
                .collect::<Vec<_>>();
            assert!(
                codes.contains(&"E_CONN_FEETECH_STS_BASE_PROFILE"),
                "missing base diagnostic for {different}: {codes:?}"
            );
            assert!(
                !codes.contains(&"E_CONN_FEETECH_STS_TOPOLOGY"),
                "topology refinement ran without the exact base for {different}: {codes:?}"
            );
        }
    }

    #[test]
    fn feetech_sts_and_uart_split_topology_diagnostic_ownership() {
        let bus = selected_profiles_document(
            &["feetech:sts", "hcdf:uart"],
            authored::NetworkStructure::Bus,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let graph = normalize_connectivity(&bus).unwrap();
        assert!(graph.warnings().is_empty());

        let link = selected_profiles_document(
            &["feetech:sts", "hcdf:uart"],
            authored::NetworkStructure::Link,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&link).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_FEETECH_STS_TOPOLOGY");

        let mesh = selected_profiles_document(
            &["feetech:sts", "hcdf:uart"],
            authored::NetworkStructure::Mesh,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&mesh).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_UART_TOPOLOGY");
    }

    #[test]
    fn uart_owns_feetech_sts_purpose_and_carrier_diagnostics() {
        for (purpose, carrier, expected_code) in [
            (
                authored::Purpose::PowerDelivery,
                authored::Carrier::Electrical,
                "E_CONN_UART_PURPOSE",
            ),
            (
                authored::Purpose::Communication,
                authored::Carrier::GuidedOptical,
                "E_CONN_UART_CARRIER",
            ),
        ] {
            let document = selected_profiles_document(
                &["feetech:sts", "hcdf:uart"],
                authored::NetworkStructure::Bus,
                purpose,
                carrier,
            );
            let error = normalize_connectivity(&document).unwrap_err();
            assert_eq!(error.issues().len(), 1);
            assert_eq!(error.issues()[0].code(), expected_code);
        }
    }

    #[test]
    fn feetech_sts_dispatch_uses_only_the_exact_profile_identity() {
        let exact = selected_profiles_document(
            &["feetech:sts"],
            authored::NetworkStructure::Bus,
            authored::Purpose::Communication,
            authored::Carrier::Electrical,
        );
        let error = normalize_connectivity(&exact).unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_FEETECH_STS_BASE_PROFILE");

        for different in ["vendor:sts", "FEETECH:sts", "feetech:STS"] {
            let document = selected_profiles_document(
                &[different],
                authored::NetworkStructure::Bus,
                authored::Purpose::Communication,
                authored::Carrier::Electrical,
            );
            let graph = normalize_connectivity(&document).unwrap();
            assert_eq!(
                graph.warnings().len(),
                1,
                "unexpected result for {different}"
            );
            assert_eq!(graph.warnings()[0].code(), "W_CONN_PROFILE_INCOMPLETE");
        }
    }

    #[test]
    fn exact_selected_profile_dispatches_and_nearby_identities_remain_incomplete() {
        let registry = fixture_registry();
        let exact = profiled_document("hcdf:test-link", &["n"], false);
        let exact = normalize_connectivity_with_registry(
            &exact,
            NormalizationOptions::default(),
            &registry,
        )
        .unwrap();
        assert!(exact.warnings().is_empty());

        for different in [
            "vendor:test-link",
            "HCDF:test-link",
            "hcdf:TEST-LINK",
            "vendor:profiles/test-link",
        ] {
            let document = profiled_document(different, &["n"], false);
            let graph = normalize_connectivity_with_registry(
                &document,
                NormalizationOptions::default(),
                &registry,
            )
            .unwrap();
            assert_eq!(
                graph.warnings().len(),
                1,
                "unexpected result for {different}"
            );
            let issue = &graph.warnings()[0];
            assert_eq!(issue.code(), "W_CONN_PROFILE_INCOMPLETE");
            assert_eq!(issue.level(), IssueLevel::Warning);
            assert_eq!(issue.subject().kind(), ObjectKind::Network);
            assert_eq!(issue.subject().local()[0].value, "n");
            assert!(issue.message().contains(different));
        }
    }

    #[test]
    fn strict_completeness_elevates_only_unknown_profile_diagnostics() {
        let registry = fixture_registry();
        let document = profiled_document("vendor:external-link", &["n"], false);
        let error = normalize_connectivity_with_registry(
            &document,
            NormalizationOptions {
                profile_registry_completeness: ProfileRegistryCompleteness::Strict,
            },
            &registry,
        )
        .unwrap_err();
        assert_eq!(error.issues().len(), 1);
        assert_eq!(error.issues()[0].code(), "E_CONN_PROFILE_INCOMPLETE");
        assert_eq!(error.issues()[0].level(), IssueLevel::Error);
        assert_eq!(error.issues()[0].subject().kind(), ObjectKind::Network);
    }

    #[test]
    fn repeated_unknown_profile_occurrences_keep_distinct_stable_subjects() {
        let registry = fixture_registry();
        let document = profiled_document("vendor:external-link", &["first", "second"], false);
        let first = normalize_connectivity_with_registry(
            &document,
            NormalizationOptions::default(),
            &registry,
        )
        .unwrap();
        let second = normalize_connectivity_with_registry(
            &document,
            NormalizationOptions::default(),
            &registry,
        )
        .unwrap();
        assert_eq!(first.warnings(), second.warnings());
        let issues = first
            .warnings()
            .iter()
            .filter(|issue| issue.code() == "W_CONN_PROFILE_INCOMPLETE")
            .collect::<Vec<_>>();
        assert_eq!(issues.len(), 2);
        assert_ne!(issues[0].subject_id(), issues[1].subject_id());
        assert_eq!(
            issues
                .iter()
                .map(|issue| issue.subject().local()[0].value.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["first", "second"])
        );
    }

    #[test]
    fn changing_profile_identity_cannot_suppress_generic_validation() {
        let registry = fixture_registry();
        for profile in ["hcdf:test-link", "vendor:external-link"] {
            let document = profiled_document(profile, &["n"], true);
            let error = normalize_connectivity_with_registry(
                &document,
                NormalizationOptions::default(),
                &registry,
            )
            .unwrap_err();
            assert!(error
                .issues()
                .iter()
                .any(|issue| issue.code() == "E_CONN_DIMENSION"));
            assert_eq!(
                error
                    .issues()
                    .iter()
                    .filter(|issue| issue.code() == "W_CONN_PROFILE_INCOMPLETE")
                    .count(),
                usize::from(profile != "hcdf:test-link")
            );
        }
    }

    #[test]
    fn cross_related_network_dispatch_is_independent_of_declaration_order() {
        let registry = cross_network_probe_registry();
        // Both networks use the same two ports. Each probe must therefore observe all four
        // participant-endpoint edges, including those belonging to the other network.
        let forward = profiled_document("hcdf:test-link", &["power", "data"], false);
        let mut reversed = forward.clone();
        reversed.scopes[0].networks.reverse();

        let forward = normalize_connectivity_with_registry(
            &forward,
            NormalizationOptions::default(),
            &registry,
        )
        .unwrap();
        let reversed = normalize_connectivity_with_registry(
            &reversed,
            NormalizationOptions::default(),
            &registry,
        )
        .unwrap();

        assert_eq!(forward.warnings(), reversed.warnings());
        assert_eq!(
            forward
                .warnings()
                .iter()
                .filter(|issue| issue.code() == "W_CONN_TEST_CROSS_NETWORK_PROBE")
                .count(),
            2
        );
        assert!(forward
            .warnings()
            .iter()
            .all(|issue| issue.message().contains("all 4 participant-endpoint edges")));
    }

    #[test]
    fn trusted_registry_failure_has_a_deterministic_root_diagnostic() {
        let document = profiled_document("hcdf:test-link", &["n"], false);
        let error = profile_registry_failure(
            &document,
            &ProfileRegistryError::UnsupportedFormatVersion(9),
        );
        assert_eq!(error.issues().len(), 1);
        let issue = &error.issues()[0];
        assert_eq!(issue.code(), "E_CONN_PROFILE_REGISTRY");
        assert_eq!(issue.level(), IssueLevel::Error);
        assert_eq!(issue.subject().kind(), ObjectKind::Scope);
        assert!(issue.subject().instance().is_root());
        assert!(issue.message().contains("format version 9"));
    }

    #[test]
    fn rejects_unknown_fields_and_noncanonical_versions() {
        let unknown = br#"{
          "format":"hcdf-connectivity-profile-registry",
          "format-version":1,
          "registry-version":"1.0.0",
          "hcdf-schema":{"major":1,"minimum-minor":0},
          "rulesets":[],
          "rules":[]
        }"#;
        assert!(matches!(
            parse_profile_registry(unknown),
            Err(ProfileRegistryError::InvalidJson(_))
        ));
        for version in ["1", "1.0", "01.0.0", "1.00.0", "1.0.0-beta"] {
            let bytes = registry_json("[]");
            let source = String::from_utf8(bytes).unwrap().replace("1.0.0", version);
            assert!(matches!(
                parse_profile_registry(source.as_bytes()),
                Err(ProfileRegistryError::InvalidJson(_))
            ));
        }
    }

    #[test]
    fn rejects_format_and_schema_incompatibility() {
        let wrong_format = String::from_utf8(registry_json("[]")).unwrap().replace(
            "hcdf-connectivity-profile-registry",
            "other-profile-registry",
        );
        assert!(matches!(
            parse_profile_registry(wrong_format.as_bytes()),
            Err(ProfileRegistryError::UnsupportedFormat(_))
        ));
        let wrong_format_version = String::from_utf8(registry_json("[]"))
            .unwrap()
            .replace("\"format-version\": 1", "\"format-version\": 2");
        assert!(matches!(
            parse_profile_registry(wrong_format_version.as_bytes()),
            Err(ProfileRegistryError::UnsupportedFormatVersion(2))
        ));
        for replacement in [
            "\"major\": 2, \"minimum-minor\": 0",
            "\"major\": 1, \"minimum-minor\": 1",
        ] {
            let incompatible = String::from_utf8(registry_json("[]"))
                .unwrap()
                .replace("\"major\": 1, \"minimum-minor\": 0", replacement);
            assert!(matches!(
                parse_profile_registry(incompatible.as_bytes()),
                Err(ProfileRegistryError::IncompatibleHcdfSchema { .. })
            ));
        }

        for replacement in [
            "\"major\": 4294967296, \"minimum-minor\": 0",
            "\"major\": 1, \"minimum-minor\": 4294967296",
        ] {
            let out_of_range = String::from_utf8(registry_json("[]"))
                .unwrap()
                .replace("\"major\": 1, \"minimum-minor\": 0", replacement);
            assert!(matches!(
                parse_profile_registry(out_of_range.as_bytes()),
                Err(ProfileRegistryError::InvalidJson(_))
            ));
        }
    }

    #[test]
    fn rejects_duplicate_and_unsorted_profiles() {
        let citation = format!("[{}]", citation("source"));
        let a = ruleset("hcdf:a", "hcdf:validator/generic", &citation);
        let b = ruleset("hcdf:b", "hcdf:validator/generic", &citation);
        assert!(matches!(
            parse_profile_registry(&registry_json(&format!("[{a},{a}]"))),
            Err(ProfileRegistryError::DuplicateProfile(_))
        ));
        assert!(matches!(
            parse_profile_registry(&registry_json(&format!("[{b},{a}]"))),
            Err(ProfileRegistryError::UnsortedProfiles { .. })
        ));
    }

    #[test]
    fn rejects_invalid_validator_and_citations() {
        let valid_citation = citation("source");
        let unsupported = ruleset(
            "hcdf:a",
            "vendor:validator/generic",
            &format!("[{valid_citation}]"),
        );
        assert!(matches!(
            parse_profile_registry(&registry_json(&format!("[{unsupported}]"))),
            Err(ProfileRegistryError::UnsupportedValidator { .. })
        ));

        let missing = ruleset("hcdf:a", "hcdf:validator/generic", "[]");
        assert!(matches!(
            parse_profile_registry(&registry_json(&format!("[{missing}]"))),
            Err(ProfileRegistryError::MissingCitations(_))
        ));

        let duplicate = ruleset(
            "hcdf:a",
            "hcdf:validator/generic",
            &format!("[{valid_citation},{valid_citation}]"),
        );
        assert!(matches!(
            parse_profile_registry(&registry_json(&format!("[{duplicate}]"))),
            Err(ProfileRegistryError::DuplicateCitation { .. })
        ));

        let z = citation("z-source");
        let a = citation("a-source");
        let unsorted = ruleset("hcdf:a", "hcdf:validator/generic", &format!("[{z},{a}]"));
        assert!(matches!(
            parse_profile_registry(&registry_json(&format!("[{unsorted}]"))),
            Err(ProfileRegistryError::UnsortedCitations { .. })
        ));

        let blank = citation("source").replace("Standards Body", "   ");
        let blank = ruleset("hcdf:a", "hcdf:validator/generic", &format!("[{blank}]"));
        assert!(matches!(
            parse_profile_registry(&registry_json(&format!("[{blank}]"))),
            Err(ProfileRegistryError::BlankCitationField { .. })
        ));
    }
}
