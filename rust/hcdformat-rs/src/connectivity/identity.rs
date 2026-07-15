use crate::model::connectivity::{DocumentIdentity, IncludeInstanceId, IncludeSegmentName};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Kind discriminator included in every canonical object identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectKind {
    Scope,
    Component,
    StructuralVisualRoot,
    StructuralFrame,
    Port,
    Channel,
    PhysicalAssembly,
    Connector,
    Position,
    Antenna,
    Binding,
    Mate,
    PhysicalPath,
    Junction,
    Termination,
    Network,
    Profile,
    Group,
    Stream,
    StreamForwarding,
    Participant,
    Hop,
    Leg,
    GptpDomain,
    GptpClock,
    TrafficClass,
    GateSchedule,
    GateControlEntry,
    ScheduleAssignment,
    PlcaConfiguration,
    PlcaNode,
    MacsecConfiguration,
    MacsecPolicy,
    MacsecOverride,
    EeeConfiguration,
    EeeOverride,
    ConnectivityFunction,
    Representation,
}

impl ObjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scope => "scope",
            Self::Component => "component",
            Self::StructuralVisualRoot => "structural-visual-root",
            Self::StructuralFrame => "structural-frame",
            Self::Port => "port",
            Self::Channel => "channel",
            Self::PhysicalAssembly => "physical-assembly",
            Self::Connector => "connector",
            Self::Position => "position",
            Self::Antenna => "antenna",
            Self::Binding => "binding",
            Self::Mate => "mate",
            Self::PhysicalPath => "physical-path",
            Self::Junction => "junction",
            Self::Termination => "termination",
            Self::Network => "network",
            Self::Profile => "profile",
            Self::Group => "group",
            Self::Stream => "stream",
            Self::StreamForwarding => "stream-forwarding",
            Self::Participant => "participant",
            Self::Hop => "hop",
            Self::Leg => "leg",
            Self::GptpDomain => "gptp-domain",
            Self::GptpClock => "gptp-clock",
            Self::TrafficClass => "traffic-class",
            Self::GateSchedule => "gate-schedule",
            Self::GateControlEntry => "gate-control-entry",
            Self::ScheduleAssignment => "schedule-assignment",
            Self::PlcaConfiguration => "plca-configuration",
            Self::PlcaNode => "plca-node",
            Self::MacsecConfiguration => "macsec-configuration",
            Self::MacsecPolicy => "macsec-policy",
            Self::MacsecOverride => "macsec-override",
            Self::EeeConfiguration => "eee-configuration",
            Self::EeeOverride => "eee-override",
            Self::ConnectivityFunction => "connectivity-function",
            Self::Representation => "representation",
        }
    }
}

/// One typed segment in an object's local identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IdentityPart {
    pub field: String,
    pub value: String,
}

impl IdentityPart {
    pub fn new(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
        }
    }
}

/// Full source and include-aware identity of a canonical object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectIdentity {
    document: DocumentIdentity,
    instance: IncludeInstanceId,
    kind: ObjectKind,
    local: Vec<IdentityPart>,
}

impl ObjectIdentity {
    pub fn new(
        document: DocumentIdentity,
        instance: IncludeInstanceId,
        kind: ObjectKind,
        local: Vec<IdentityPart>,
    ) -> Self {
        Self {
            document,
            instance,
            kind,
            local,
        }
    }

    pub fn document(&self) -> &DocumentIdentity {
        &self.document
    }

    pub fn instance(&self) -> &IncludeInstanceId {
        &self.instance
    }

    pub fn kind(&self) -> ObjectKind {
        self.kind
    }

    pub fn local(&self) -> &[IdentityPart] {
        &self.local
    }

    /// Render a diagnostic path. This string is not a machine-consumed reference format.
    pub fn display_path(&self) -> String {
        let local = self
            .local
            .iter()
            .map(|part| format!("{}={:?}", part.field, part.value))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} [{}] {} ({})",
            self.document,
            self.instance.display_path(),
            self.kind.as_str(),
            local
        )
    }

    pub fn stable_id(&self) -> StableObjectId {
        let mut fields = Vec::<Vec<u8>>::new();
        fields.push(self.document.as_str().as_bytes().to_vec());
        for segment in self.instance.segments() {
            match &segment.name {
                IncludeSegmentName::Unnamed => {
                    fields.push(b"include-segment-name:unnamed".to_vec());
                }
                IncludeSegmentName::Named(name) => {
                    fields.push(b"include-segment-name:named".to_vec());
                    fields.push(name.as_bytes().to_vec());
                }
            }
            fields.push(b"include-segment-occurrence".to_vec());
            fields.push(segment.occurrence.to_string().into_bytes());
        }
        fields.push(self.kind.as_str().as_bytes().to_vec());
        for part in &self.local {
            fields.push(part.field.as_bytes().to_vec());
            fields.push(part.value.as_bytes().to_vec());
        }
        StableObjectId(format!(
            "hcdf-object-v1:{}",
            stable_digest(
                "hcdf-connectivity-object-v1",
                fields.iter().map(Vec::as_slice),
            )
        ))
    }
}

/// Build the canonical component identity shared by structural and connectivity projections.
pub fn structural_component_identity(
    document: &DocumentIdentity,
    instance: &IncludeInstanceId,
    component: &str,
) -> ObjectIdentity {
    ObjectIdentity::new(
        document.clone(),
        instance.clone(),
        ObjectKind::Component,
        vec![IdentityPart::new("component", component)],
    )
}

/// Build the canonical identity of one component-local visual root.
pub fn structural_visual_identity(
    document: &DocumentIdentity,
    instance: &IncludeInstanceId,
    component: &str,
    visual: &str,
) -> ObjectIdentity {
    ObjectIdentity::new(
        document.clone(),
        instance.clone(),
        ObjectKind::StructuralVisualRoot,
        vec![
            IdentityPart::new("component", component),
            IdentityPart::new("visual", visual),
        ],
    )
}

/// Build the canonical identity of one component-local named frame.
pub fn structural_frame_identity(
    document: &DocumentIdentity,
    instance: &IncludeInstanceId,
    component: &str,
    frame: &str,
) -> ObjectIdentity {
    ObjectIdentity::new(
        document.clone(),
        instance.clone(),
        ObjectKind::StructuralFrame,
        vec![
            IdentityPart::new("component", component),
            IdentityPart::new("frame", frame),
        ],
    )
}

/// Opaque deterministic identity for one normalized object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct StableObjectId(String);

impl StableObjectId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StableObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Opaque deterministic identity for one normalized graph edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct StableEdgeId(pub(crate) String);

impl StableEdgeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StableEdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for StableObjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if stable_id_has_exact_shape(&value, "hcdf-object-v1:") {
            Ok(Self(value))
        } else {
            Err(serde::de::Error::custom(
                "stable object ID must use hcdf-object-v1: followed by 64 lowercase hexadecimal digits",
            ))
        }
    }
}

impl<'de> Deserialize<'de> for StableEdgeId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if stable_id_has_exact_shape(&value, "hcdf-edge-v1:") {
            Ok(Self(value))
        } else {
            Err(serde::de::Error::custom(
                "stable edge ID must use hcdf-edge-v1: followed by 64 lowercase hexadecimal digits",
            ))
        }
    }
}

fn stable_id_has_exact_shape(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

pub(crate) fn stable_digest<'a>(
    domain: &str,
    fields: impl IntoIterator<Item = &'a [u8]>,
) -> String {
    let mut hasher = Sha256::new();
    write_field(&mut hasher, domain.as_bytes());
    for field in fields {
        write_field(&mut hasher, field);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

fn write_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}
