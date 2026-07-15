//! Transactional loading and projection of an HCDF include-instance tree.
//!
//! This module is synchronous, pure Rust, and independent of platform I/O. Callers provide a
//! DocumentResolver, so the same contract works with native, in-memory, remote, and wasm stores.

use crate::compose::{
    content_sha, flatten_with, mul44, pose_to_matrix, visit_connectivity_instance_ref_slots_mut,
    Mat4,
};
use crate::connectivity::{
    normalize_connectivity, structural_component_identity, structural_frame_identity,
    structural_visual_identity, ConnectivityGraphExtension, ConnectivityGraphExtensionEdge,
    ConnectivityIssue, ConnectivityNode, ConnectivityNodeData, EdgeExactness, EdgeKind,
    IdentityPart, NormalizedConnectivityGraph, ObjectIdentity, ObjectKind, StableObjectId,
    StreamGroupReferenceTarget,
};
use crate::model::connectivity::{
    ComponentRef as CanonicalComponentRef, ConnectivityDocument,
    ConnectivityFunctionRef as CanonicalConnectivityFunctionRef, DocumentIdentity,
    IncludeInstanceId, IncludeSegmentName, NetworkRef as CanonicalNetworkRef,
    ParticipantRef as CanonicalParticipantRef, ReferenceScope, ScheduleRef as CanonicalScheduleRef,
    StreamGroupRef as CanonicalStreamGroupRef, TrafficClassRef as CanonicalTrafficClassRef,
};
use crate::model::connectivity_xml::{
    ConnectivityFunctionRef as XmlConnectivityFunctionRef, InstanceRef, InstanceSegment,
    NetworkRef as XmlNetworkRef, ParticipantRef as XmlParticipantRef,
    ScheduleRef as XmlScheduleRef, TrafficClassRef as XmlTrafficClassRef,
};
use crate::model::{
    ConnectivityConversionError, Hcdf, Pose, StreamGroupRef as XmlStreamGroupRef,
    StreamProfileDocument, StreamProfileSelectionRole,
};
use crate::resource::{visit_resources, ResourceClass, ResourceRewrite, ResourceSite};
use crate::resource_path::{resolve_resource_reference, ResourceReferenceError};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocumentResourceKey(String);

impl DocumentResourceKey {
    pub fn new(value: impl Into<String>) -> Result<Self, ResourceKeyError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ResourceKeyError::EmptyDocument);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DocumentResourceKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetResourceKey(String);

impl AssetResourceKey {
    pub fn new(value: impl Into<String>) -> Result<Self, ResourceKeyError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ResourceKeyError::EmptyAsset);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AssetResourceKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceKeyError {
    #[error("document resource key must not be empty")]
    EmptyDocument,
    #[error("asset resource key must not be empty")]
    EmptyAsset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolverFailureKind {
    NotFound,
    Denied,
    InvalidReference,
    Unsupported,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct ResolverFailure {
    pub kind: ResolverFailureKind,
    pub message: String,
}

impl ResolverFailure {
    pub fn new(kind: ResolverFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ResolverFailureKind::NotFound, message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DocumentDependencyKind {
    HcdfInclude,
    StreamProfile,
    StreamProfileDependency,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDocumentResource {
    pub key: DocumentResourceKey,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAssetResource {
    pub key: AssetResourceKey,
    pub bytes: Vec<u8>,
}

pub trait DocumentResolver {
    fn resolve_document(
        &mut self,
        parent: &DocumentResourceKey,
        uri: &str,
        kind: DocumentDependencyKind,
    ) -> Result<ResolvedDocumentResource, ResolverFailure>;

    fn resolve_asset(
        &mut self,
        parent: &DocumentResourceKey,
        uri: &str,
    ) -> Result<ResolvedAssetResource, ResolverFailure> {
        let _ = (parent, uri);
        Err(ResolverFailure::new(
            ResolverFailureKind::Unsupported,
            "opaque asset resolution is not configured",
        ))
    }
}

pub struct CallbackDocumentResolver<D, A> {
    document: D,
    asset: A,
}

impl<D, A> CallbackDocumentResolver<D, A> {
    pub fn new(document: D, asset: A) -> Self {
        Self { document, asset }
    }
}

impl<D, A> DocumentResolver for CallbackDocumentResolver<D, A>
where
    D: FnMut(
        &DocumentResourceKey,
        &str,
        DocumentDependencyKind,
    ) -> Result<ResolvedDocumentResource, ResolverFailure>,
    A: FnMut(&DocumentResourceKey, &str) -> Result<ResolvedAssetResource, ResolverFailure>,
{
    fn resolve_document(
        &mut self,
        parent: &DocumentResourceKey,
        uri: &str,
        kind: DocumentDependencyKind,
    ) -> Result<ResolvedDocumentResource, ResolverFailure> {
        (self.document)(parent, uri, kind)
    }

    fn resolve_asset(
        &mut self,
        parent: &DocumentResourceKey,
        uri: &str,
    ) -> Result<ResolvedAssetResource, ResolverFailure> {
        (self.asset)(parent, uri)
    }
}

#[derive(Debug, Clone, Default)]
pub struct MemoryDocumentResolver {
    documents: BTreeMap<DocumentResourceKey, Vec<u8>>,
    assets: BTreeMap<AssetResourceKey, Vec<u8>>,
}

impl MemoryDocumentResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_document(
        &mut self,
        key: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<DocumentResourceKey, ResourceKeyError> {
        let key = DocumentResourceKey::new(key)?;
        self.documents.insert(key.clone(), bytes.into());
        Ok(key)
    }

    pub fn insert_asset(
        &mut self,
        key: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<AssetResourceKey, ResourceKeyError> {
        let key = AssetResourceKey::new(key)?;
        self.assets.insert(key.clone(), bytes.into());
        Ok(key)
    }
}

impl DocumentResolver for MemoryDocumentResolver {
    fn resolve_document(
        &mut self,
        parent: &DocumentResourceKey,
        uri: &str,
        _kind: DocumentDependencyKind,
    ) -> Result<ResolvedDocumentResource, ResolverFailure> {
        let key = DocumentResourceKey::new(
            resolve_resource_reference(parent.as_str(), uri).map_err(invalid_reference)?,
        )
        .map_err(invalid_key)?;
        let bytes = self.documents.get(&key).cloned().ok_or_else(|| {
            ResolverFailure::not_found(format!("document {key:?} is not in memory"))
        })?;
        Ok(ResolvedDocumentResource { key, bytes })
    }

    fn resolve_asset(
        &mut self,
        parent: &DocumentResourceKey,
        uri: &str,
    ) -> Result<ResolvedAssetResource, ResolverFailure> {
        let key = AssetResourceKey::new(
            resolve_resource_reference(parent.as_str(), uri).map_err(invalid_reference)?,
        )
        .map_err(invalid_key)?;
        let bytes =
            self.assets.get(&key).cloned().ok_or_else(|| {
                ResolverFailure::not_found(format!("asset {key:?} is not in memory"))
            })?;
        Ok(ResolvedAssetResource { key, bytes })
    }
}

fn invalid_key(error: ResourceKeyError) -> ResolverFailure {
    ResolverFailure::new(ResolverFailureKind::InvalidReference, error.to_string())
}

fn invalid_reference(error: ResourceReferenceError) -> ResolverFailure {
    ResolverFailure::new(ResolverFailureKind::InvalidReference, error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentSetLimits {
    pub max_depth: usize,
    pub max_unique_documents: usize,
    pub max_instances: usize,
    pub max_document_bytes: usize,
    pub max_aggregate_unique_bytes: usize,
    pub max_asset_sites: usize,
    pub max_dependency_sites: usize,
    pub max_resolver_calls: usize,
    pub max_resolver_bytes: usize,
    pub max_expanded_source_bytes: usize,
    pub max_expanded_xml_elements: usize,
}

impl Default for DocumentSetLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_unique_documents: 1024,
            max_instances: 4096,
            max_document_bytes: 16 * 1024 * 1024,
            max_aggregate_unique_bytes: 64 * 1024 * 1024,
            max_asset_sites: 65_536,
            max_dependency_sites: 65_536,
            max_resolver_calls: 16_384,
            max_resolver_bytes: 256 * 1024 * 1024,
            max_expanded_source_bytes: 256 * 1024 * 1024,
            max_expanded_xml_elements: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetPolicy {
    AllowMissing,
    RequireAllAssets,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentLoadPolicy {
    EditorCompatible,
    Strict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentSetOptions {
    pub limits: DocumentSetLimits,
    pub asset_policy: AssetPolicy,
    pub document_policy: DocumentLoadPolicy,
}

impl Default for DocumentSetOptions {
    fn default() -> Self {
        Self {
            limits: DocumentSetLimits::default(),
            asset_policy: AssetPolicy::AllowMissing,
            document_policy: DocumentLoadPolicy::EditorCompatible,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentSetLimitKind {
    Depth,
    UniqueDocuments,
    Instances,
    DocumentBytes,
    AggregateUniqueBytes,
    AssetSites,
    DependencySites,
    ResolverCalls,
    ResolverBytes,
    ExpandedSourceBytes,
    ExpandedXmlElements,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DocumentSetError {
    #[error("failed to serialize root document: {message}")]
    RootSerialization { message: String },
    #[error("{kind:?} limit exceeded: limit {limit}, actual {actual}")]
    LimitExceeded {
        kind: DocumentSetLimitKind,
        limit: usize,
        actual: usize,
        resource: Option<String>,
    },
    #[error("include {include_index} in {instance:?} has no nonempty URI")]
    MissingIncludeUri {
        instance: IncludeInstanceId,
        include_index: usize,
    },
    #[error(
        "duplicate sibling include name {name:?} at include {duplicate_include_index} in {instance:?}; first used at include {first_include_index}"
    )]
    DuplicateIncludeName {
        instance: IncludeInstanceId,
        first_include_index: usize,
        duplicate_include_index: usize,
        name: String,
    },
    #[error("failed to resolve include {uri:?} in {instance:?}: {failure}")]
    DocumentResolution {
        instance: IncludeInstanceId,
        include_index: usize,
        uri: String,
        failure: ResolverFailure,
    },
    #[error("include {uri:?} in {instance:?} expected {expected}, got {actual}")]
    DocumentDigestMismatch {
        instance: IncludeInstanceId,
        include_index: usize,
        uri: String,
        expected: String,
        actual: String,
    },
    #[error("document {key} is not UTF-8: {message}")]
    DocumentEncoding {
        key: DocumentResourceKey,
        message: String,
    },
    #[error("document {key} failed HCDF parsing: {message}")]
    DocumentParse {
        key: DocumentResourceKey,
        message: String,
    },
    #[error("document {key} has content kind {actual}, expected {expected}")]
    DocumentContentKindMismatch {
        key: DocumentResourceKey,
        expected: DocumentContentKind,
        actual: DocumentContentKind,
    },
    #[error("document resolver returned different bytes for existing key {key}")]
    SourceConflict { key: DocumentResourceKey },
    #[error("stream-profile dependency {uri:?} at {id:?} could not be loaded: {diagnostic:?}")]
    StreamProfileDependency {
        id: Box<StreamProfileInstanceId>,
        uri: String,
        required: bool,
        diagnostic: Box<StreamProfileDependencyDiagnostic>,
    },
    #[error("failed to resolve asset {uri:?} at {site:?} in {instance:?}: {failure}")]
    AssetResolution {
        instance: IncludeInstanceId,
        site: Box<ResourceSite>,
        uri: String,
        failure: ResolverFailure,
    },
    #[error("asset {uri:?} at {site:?} in {instance:?} expected {expected}, got {actual}")]
    AssetDigestMismatch {
        instance: IncludeInstanceId,
        site: Box<ResourceSite>,
        uri: String,
        expected: String,
        actual: String,
    },
    #[error("asset resolver returned different bytes for existing key {key}")]
    AssetSourceConflict { key: AssetResourceKey },
    #[error("include cycle detected through {chain:?}")]
    IncludeCycle { chain: Vec<DocumentResourceKey> },
    #[error("document-set structural flattening failed: {message}")]
    StructuralFlatten { message: String },
    #[error(
        "structural projection for {kind:?} {local_name:?} in {instance:?} is inconsistent: {message}"
    )]
    StructuralProjection {
        instance: IncludeInstanceId,
        kind: ObjectKind,
        local_name: String,
        message: String,
    },
}

impl DocumentSetError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::RootSerialization { .. } => "E_DOC_RESOURCE_ROOT_SERIALIZE",
            Self::LimitExceeded { .. } => "E_DOC_RESOURCE_LIMIT",
            Self::MissingIncludeUri { .. } => "E_DOC_RESOURCE_INCLUDE_URI",
            Self::DuplicateIncludeName { .. } => "E_DOC_RESOURCE_DUPLICATE_INCLUDE_NAME",
            Self::DocumentResolution { .. } => "E_DOC_RESOURCE_DOCUMENT_RESOLVE",
            Self::DocumentDigestMismatch { .. } => "E_DOC_RESOURCE_DOCUMENT_DIGEST",
            Self::DocumentEncoding { .. } => "E_DOC_RESOURCE_DOCUMENT_ENCODING",
            Self::DocumentParse { .. } => "E_DOC_RESOURCE_DOCUMENT_PARSE",
            Self::DocumentContentKindMismatch { .. } => "E_DOC_RESOURCE_DOCUMENT_KIND",
            Self::SourceConflict { .. } => "E_DOC_RESOURCE_SOURCE_CONFLICT",
            Self::StreamProfileDependency { required: true, .. } => {
                "E_DOC_RESOURCE_PROFILE_REQUIRED"
            }
            Self::StreamProfileDependency {
                required: false,
                diagnostic,
                ..
            } => diagnostic.code(),
            Self::AssetResolution { .. } => "E_DOC_RESOURCE_ASSET_RESOLVE",
            Self::AssetDigestMismatch { .. } => "E_DOC_RESOURCE_ASSET_DIGEST",
            Self::AssetSourceConflict { .. } => "E_DOC_RESOURCE_ASSET_SOURCE_CONFLICT",
            Self::IncludeCycle { .. } => "E_DOC_RESOURCE_INCLUDE_CYCLE",
            Self::StructuralFlatten { .. } => "E_DOC_RESOURCE_STRUCTURAL_FLATTEN",
            Self::StructuralProjection { .. } => "E_DOC_RESOURCE_STRUCTURAL_PROJECTION",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentSourceProvenance {
    RawRootBytes,
    ResolvedDependencyBytes,
    CanonicalModelSerialization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DocumentContentKind {
    Hcdf,
    StreamProfile,
}

impl DocumentContentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hcdf => "hcdf",
            Self::StreamProfile => "stream-profile",
        }
    }
}

impl std::fmt::Display for DocumentContentKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentContentFailure {
    Encoding { message: String },
    Parse { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DocumentSourceContent {
    Hcdf(Box<Hcdf>),
    StreamProfile(Box<StreamProfileDocument>),
    Unusable {
        detected_kind: Option<DocumentContentKind>,
        failure: DocumentContentFailure,
    },
}

impl DocumentSourceContent {
    pub fn kind(&self) -> Option<DocumentContentKind> {
        match self {
            Self::Hcdf(_) => Some(DocumentContentKind::Hcdf),
            Self::StreamProfile(_) => Some(DocumentContentKind::StreamProfile),
            Self::Unusable { detected_kind, .. } => *detected_kind,
        }
    }

    pub fn hcdf(&self) -> Option<&Hcdf> {
        match self {
            Self::Hcdf(document) => Some(document.as_ref()),
            Self::StreamProfile(_) | Self::Unusable { .. } => None,
        }
    }

    pub fn stream_profile(&self) -> Option<&StreamProfileDocument> {
        match self {
            Self::StreamProfile(document) => Some(document.as_ref()),
            Self::Hcdf(_) | Self::Unusable { .. } => None,
        }
    }

    pub fn failure(&self) -> Option<&DocumentContentFailure> {
        match self {
            Self::Hcdf(_) | Self::StreamProfile(_) => None,
            Self::Unusable { failure, .. } => Some(failure),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedDocumentSource {
    pub key: DocumentResourceKey,
    pub content: DocumentSourceContent,
    pub bytes: Vec<u8>,
    pub source_sha: String,
    pub provenance: DocumentSourceProvenance,
    pub xml_element_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UntrustedPoseReason {
    WrongValueCount { actual: usize },
    InvalidNumber { token_index: usize, token: String },
    NonFiniteNumber { token_index: usize, token: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementProjectionCause {
    UntrustedPose(UntrustedPoseReason),
    PlacementFrameReference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnprojectablePlacementStep {
    pub instance: IncludeInstanceId,
    pub authored_pose: Option<String>,
    pub placement_frame: Option<String>,
    pub causes: Vec<PlacementProjectionCause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPlacement {
    pub local_to_parent: Mat4,
    pub local_to_root: Mat4,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlacementState {
    Resolved(Box<ResolvedPlacement>),
    Unprojectable {
        ancestry: Vec<UnprojectablePlacementStep>,
    },
}

impl PlacementState {
    pub fn is_projected(&self) -> bool {
        matches!(self, Self::Resolved(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComponentOriginAdjustment {
    pub component: String,
    pub authored_pose: Option<String>,
    pub placement: PlacementState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentDependencyDiagnostic {
    ResolutionFailure {
        failure: ResolverFailure,
    },
    ContentFailure {
        key: DocumentResourceKey,
        detected_kind: Option<DocumentContentKind>,
        failure: DocumentContentFailure,
    },
    ContentKindMismatch {
        key: DocumentResourceKey,
        expected: DocumentContentKind,
        actual: DocumentContentKind,
    },
    DigestMismatch {
        expected: String,
        actual: String,
    },
}

impl DocumentDependencyDiagnostic {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ResolutionFailure { .. } => "E_DOC_RESOURCE_DOCUMENT_RESOLVE",
            Self::ContentFailure {
                failure: DocumentContentFailure::Encoding { .. },
                ..
            } => "E_DOC_RESOURCE_DOCUMENT_ENCODING",
            Self::ContentFailure {
                failure: DocumentContentFailure::Parse { .. },
                ..
            } => "E_DOC_RESOURCE_DOCUMENT_PARSE",
            Self::ContentKindMismatch { .. } => "E_DOC_RESOURCE_DOCUMENT_KIND",
            Self::DigestMismatch { .. } => "E_DOC_RESOURCE_DOCUMENT_DIGEST",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentIncludeSiteOutcome {
    Resolved { child: IncludeInstanceId },
    RetainedUnresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentIncludeSite {
    pub include_index: usize,
    pub dependency_kind: DocumentDependencyKind,
    pub authored_uri: String,
    pub authored_sha: Option<String>,
    pub outcome: DocumentIncludeSiteOutcome,
    pub diagnostics: Vec<DocumentDependencyDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamProfileInstanceId {
    owner: IncludeInstanceId,
    declaration_index: usize,
    dependency_path: Vec<usize>,
}

impl StreamProfileInstanceId {
    fn direct(owner: IncludeInstanceId, declaration_index: usize) -> Self {
        Self {
            owner,
            declaration_index,
            dependency_path: Vec::new(),
        }
    }

    fn dependency(&self, dependency_index: usize) -> Self {
        let mut dependency_path = self.dependency_path.clone();
        dependency_path.push(dependency_index);
        Self {
            owner: self.owner.clone(),
            declaration_index: self.declaration_index,
            dependency_path,
        }
    }

    pub fn owner(&self) -> &IncludeInstanceId {
        &self.owner
    }

    pub fn declaration_index(&self) -> usize {
        self.declaration_index
    }

    pub fn dependency_path(&self) -> &[usize] {
        &self.dependency_path
    }

    pub fn display_path(&self) -> String {
        let suffix = self
            .dependency_path
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join("/");
        if suffix.is_empty() {
            format!(
                "{} / stream-profile#{}",
                self.owner.display_path(),
                self.declaration_index
            )
        } else {
            format!(
                "{} / stream-profile#{} / dependency/{suffix}",
                self.owner.display_path(),
                self.declaration_index
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamProfileDependencyDiagnostic {
    ResolutionFailure {
        failure: ResolverFailure,
    },
    ContentFailure {
        key: DocumentResourceKey,
        detected_kind: Option<DocumentContentKind>,
        failure: DocumentContentFailure,
    },
    ContentKindMismatch {
        key: DocumentResourceKey,
        expected: DocumentContentKind,
        actual: DocumentContentKind,
    },
    DigestMismatch {
        expected: String,
        actual: String,
    },
    Cycle {
        chain: Vec<DocumentResourceKey>,
    },
}

impl StreamProfileDependencyDiagnostic {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ResolutionFailure { .. } => "E_DOC_RESOURCE_PROFILE_RESOLVE",
            Self::ContentFailure {
                failure: DocumentContentFailure::Encoding { .. },
                ..
            } => "E_DOC_RESOURCE_PROFILE_ENCODING",
            Self::ContentFailure {
                failure: DocumentContentFailure::Parse { .. },
                ..
            } => "E_DOC_RESOURCE_PROFILE_PARSE",
            Self::ContentKindMismatch { .. } => "E_DOC_RESOURCE_PROFILE_KIND",
            Self::DigestMismatch { .. } => "E_DOC_RESOURCE_PROFILE_DIGEST",
            Self::Cycle { .. } => "E_DOC_RESOURCE_PROFILE_CYCLE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamProfileSiteOutcome {
    Resolved { profile: StreamProfileInstanceId },
    RetainedUnresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamProfileDependencySite {
    pub id: StreamProfileInstanceId,
    pub declaring_source: DocumentResourceKey,
    pub owner: IncludeInstanceId,
    pub parent_profile: Option<StreamProfileInstanceId>,
    pub dependency_kind: DocumentDependencyKind,
    pub dependency_index: usize,
    pub authored_uri: String,
    pub authored_sha: Option<String>,
    pub required: bool,
    pub selection_role: Option<StreamProfileSelectionRole>,
    pub outcome: StreamProfileSiteOutcome,
    pub diagnostics: Vec<StreamProfileDependencyDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedStreamProfileInstance {
    pub id: StreamProfileInstanceId,
    pub source: DocumentResourceKey,
    pub owner: IncludeInstanceId,
    pub parent: Option<StreamProfileInstanceId>,
    pub required: bool,
    pub selection_role: Option<StreamProfileSelectionRole>,
    pub children: Vec<StreamProfileInstanceId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocumentInstance {
    pub id: IncludeInstanceId,
    pub source: DocumentResourceKey,
    pub parent: Option<IncludeInstanceId>,
    pub include_site_index: Option<usize>,
    pub authored_name: Option<String>,
    pub authored_uri: Option<String>,
    pub placement: PlacementState,
    pub component_origin_adjustments: Vec<ComponentOriginAdjustment>,
    pub include_sites: Vec<DocumentIncludeSite>,
    pub children: Vec<IncludeInstanceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlattenPrefixMap {
    prefixes: BTreeMap<IncludeInstanceId, String>,
    segments: BTreeMap<IncludeInstanceId, Option<String>>,
}

impl FlattenPrefixMap {
    pub fn prefix(&self, instance: &IncludeInstanceId) -> Option<&str> {
        self.prefixes.get(instance).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&IncludeInstanceId, &str)> {
        self.prefixes
            .iter()
            .map(|(instance, prefix)| (instance, prefix.as_str()))
    }

    pub fn segment(&self, instance: &IncludeInstanceId) -> Option<Option<&str>> {
        self.segments
            .get(instance)
            .map(|segment| segment.as_deref())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocumentSetWork {
    pub dependency_sites: usize,
    pub asset_sites: usize,
    pub resolver_calls: usize,
    pub resolver_bytes: usize,
    pub expanded_source_bytes: usize,
    pub expanded_xml_elements: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedDocumentTree {
    document: DocumentIdentity,
    root: DocumentResourceKey,
    sources: BTreeMap<DocumentResourceKey, LoadedDocumentSource>,
    instances: BTreeMap<IncludeInstanceId, DocumentInstance>,
    stream_profile_instances: BTreeMap<StreamProfileInstanceId, LoadedStreamProfileInstance>,
    stream_profile_sites: Vec<StreamProfileDependencySite>,
    flatten_prefixes: FlattenPrefixMap,
    options: DocumentSetOptions,
    work: DocumentSetWork,
}

impl LoadedDocumentTree {
    pub fn document(&self) -> &DocumentIdentity {
        &self.document
    }

    pub fn root(&self) -> &DocumentResourceKey {
        &self.root
    }

    pub fn sources(&self) -> &BTreeMap<DocumentResourceKey, LoadedDocumentSource> {
        &self.sources
    }

    pub fn instances(&self) -> &BTreeMap<IncludeInstanceId, DocumentInstance> {
        &self.instances
    }

    pub fn stream_profile_instances(
        &self,
    ) -> &BTreeMap<StreamProfileInstanceId, LoadedStreamProfileInstance> {
        &self.stream_profile_instances
    }

    pub fn stream_profile_sites(&self) -> &[StreamProfileDependencySite] {
        &self.stream_profile_sites
    }

    pub fn flatten_prefixes(&self) -> &FlattenPrefixMap {
        &self.flatten_prefixes
    }

    pub fn options(&self) -> &DocumentSetOptions {
        &self.options
    }

    pub fn work(&self) -> &DocumentSetWork {
        &self.work
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructuralComponentProjection {
    pub identity: ObjectIdentity,
    pub id: StableObjectId,
    pub source: DocumentResourceKey,
    pub instance: IncludeInstanceId,
    pub local_name: String,
    pub flattened_name: String,
    pub flattened_index: usize,
    pub placement: PlacementState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralVisualProjection {
    pub identity: ObjectIdentity,
    pub id: StableObjectId,
    pub component_id: StableObjectId,
    pub source: DocumentResourceKey,
    pub instance: IncludeInstanceId,
    pub local_component: String,
    pub local_name: String,
    pub flattened_component: String,
    pub flattened_component_index: usize,
    pub flattened_visual_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralFrameProjection {
    pub identity: ObjectIdentity,
    pub id: StableObjectId,
    pub component_id: StableObjectId,
    pub source: DocumentResourceKey,
    pub instance: IncludeInstanceId,
    pub local_component: String,
    pub local_name: String,
    pub flattened_component: String,
    pub flattened_component_index: usize,
    pub flattened_frame_index: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructuralSceneProjection {
    pub flatten_prefixes: FlattenPrefixMap,
    pub placement_projection_complete: bool,
    pub components: Vec<StructuralComponentProjection>,
    pub visuals: Vec<StructuralVisualProjection>,
    pub frames: Vec<StructuralFrameProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectedConnectivityIssue {
    DocumentSet(DocumentSetError),
    Dependency {
        instance: IncludeInstanceId,
        include_index: usize,
        diagnostic: DocumentDependencyDiagnostic,
    },
    StreamProfileDependency {
        id: StreamProfileInstanceId,
        diagnostic: StreamProfileDependencyDiagnostic,
    },
    Conversion {
        instance: IncludeInstanceId,
        error: ConnectivityConversionError,
    },
    Normalization(ConnectivityIssue),
}

impl ProjectedConnectivityIssue {
    pub fn code(&self) -> &str {
        match self {
            Self::DocumentSet(error) => error.code(),
            Self::Dependency { diagnostic, .. } => diagnostic.code(),
            Self::StreamProfileDependency { diagnostic, .. } => diagnostic.code(),
            Self::Conversion { error, .. } => error.code(),
            Self::Normalization(issue) => issue.code(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectivityProjection {
    Valid {
        authored: ConnectivityDocument,
        graph: NormalizedConnectivityGraph,
    },
    Invalid {
        issues: Vec<ProjectedConnectivityIssue>,
    },
}

impl ConnectivityProjection {
    pub fn from_document_set_result(
        result: Result<ProjectedConnectivityDocumentSet, DocumentSetError>,
    ) -> Self {
        match result {
            Ok(projected) => projected.into_connectivity(),
            Err(error) => error.into(),
        }
    }

    pub fn graph(&self) -> Option<&NormalizedConnectivityGraph> {
        match self {
            Self::Valid { graph, .. } => Some(graph),
            Self::Invalid { .. } => None,
        }
    }
}

impl From<DocumentSetError> for ConnectivityProjection {
    fn from(error: DocumentSetError) -> Self {
        Self::Invalid {
            issues: vec![ProjectedConnectivityIssue::DocumentSet(error)],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAsset {
    pub key: AssetResourceKey,
    pub bytes: Vec<u8>,
    pub content_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetResolutionFailure {
    Resolver(ResolverFailure),
    DigestMismatch { expected: String, actual: String },
}

impl AssetResolutionFailure {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Resolver(_) => "E_DOC_RESOURCE_ASSET_RESOLVE",
            Self::DigestMismatch { .. } => "E_DOC_RESOURCE_ASSET_DIGEST",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetSiteResolution {
    Resolved { key: AssetResourceKey },
    Unresolved { failure: AssetResolutionFailure },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetResourceSite {
    pub source: DocumentResourceKey,
    pub instance: IncludeInstanceId,
    pub site: ResourceSite,
    pub authored_uri: String,
    pub authored_sha: Option<String>,
    pub resolution: AssetSiteResolution,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedAssetInventory {
    resources: BTreeMap<AssetResourceKey, ResolvedAsset>,
    sites: Vec<AssetResourceSite>,
}

impl ResolvedAssetInventory {
    pub fn resources(&self) -> &BTreeMap<AssetResourceKey, ResolvedAsset> {
        &self.resources
    }

    pub fn sites(&self) -> &[AssetResourceSite] {
        &self.sites
    }

    pub fn resource(&self, key: &AssetResourceKey) -> Option<&ResolvedAsset> {
        self.resources.get(key)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectedConnectivityDocumentSet {
    sources: BTreeMap<DocumentResourceKey, LoadedDocumentSource>,
    instances: BTreeMap<IncludeInstanceId, DocumentInstance>,
    stream_profile_instances: BTreeMap<StreamProfileInstanceId, LoadedStreamProfileInstance>,
    stream_profile_sites: Vec<StreamProfileDependencySite>,
    flattened: Hcdf,
    structural_projection: StructuralSceneProjection,
    connectivity: ConnectivityProjection,
    assets: ResolvedAssetInventory,
    work: DocumentSetWork,
}

impl ProjectedConnectivityDocumentSet {
    pub fn sources(&self) -> &BTreeMap<DocumentResourceKey, LoadedDocumentSource> {
        &self.sources
    }

    pub fn instances(&self) -> &BTreeMap<IncludeInstanceId, DocumentInstance> {
        &self.instances
    }

    pub fn stream_profile_instances(
        &self,
    ) -> &BTreeMap<StreamProfileInstanceId, LoadedStreamProfileInstance> {
        &self.stream_profile_instances
    }

    pub fn stream_profile_sites(&self) -> &[StreamProfileDependencySite] {
        &self.stream_profile_sites
    }

    pub fn flattened(&self) -> &Hcdf {
        &self.flattened
    }

    pub fn structural_projection(&self) -> &StructuralSceneProjection {
        &self.structural_projection
    }

    pub fn connectivity(&self) -> &ConnectivityProjection {
        &self.connectivity
    }

    pub fn into_connectivity(self) -> ConnectivityProjection {
        self.connectivity
    }

    pub fn assets(&self) -> &ResolvedAssetInventory {
        &self.assets
    }

    pub fn work(&self) -> &DocumentSetWork {
        &self.work
    }
}

pub fn load_document_tree_from_bytes<R: DocumentResolver>(
    root_bytes: Vec<u8>,
    document: DocumentIdentity,
    root_key: DocumentResourceKey,
    resolver: &mut R,
    options: DocumentSetOptions,
) -> Result<LoadedDocumentTree, DocumentSetError> {
    check_document_size(&root_key, root_bytes.len(), &options.limits)?;
    let (content, xml_element_count) = parse_document_source_content(&root_bytes);
    let root = match content {
        DocumentSourceContent::Hcdf(root) => root,
        DocumentSourceContent::StreamProfile(_) => {
            return Err(DocumentSetError::DocumentContentKindMismatch {
                key: root_key,
                expected: DocumentContentKind::Hcdf,
                actual: DocumentContentKind::StreamProfile,
            });
        }
        DocumentSourceContent::Unusable {
            detected_kind: Some(DocumentContentKind::StreamProfile),
            ..
        } => {
            return Err(DocumentSetError::DocumentContentKindMismatch {
                key: root_key,
                expected: DocumentContentKind::Hcdf,
                actual: DocumentContentKind::StreamProfile,
            });
        }
        DocumentSourceContent::Unusable { failure, .. } => {
            return Err(document_content_error(root_key, failure));
        }
    };
    let root_source = LoadedDocumentSource {
        key: root_key.clone(),
        content: DocumentSourceContent::Hcdf(root),
        source_sha: content_sha(&root_bytes),
        xml_element_count,
        bytes: root_bytes,
        provenance: DocumentSourceProvenance::RawRootBytes,
    };
    load_document_tree_from_root_source(document, root_key, root_source, resolver, options)
}

pub fn load_document_tree_from_model<R: DocumentResolver>(
    root: Hcdf,
    document: DocumentIdentity,
    root_key: DocumentResourceKey,
    resolver: &mut R,
    options: DocumentSetOptions,
) -> Result<LoadedDocumentTree, DocumentSetError> {
    let root_bytes = root
        .to_xml_string()
        .map_err(|error| DocumentSetError::RootSerialization {
            message: error.to_string(),
        })?
        .into_bytes();
    check_document_size(&root_key, root_bytes.len(), &options.limits)?;
    let root_source = LoadedDocumentSource {
        key: root_key.clone(),
        content: DocumentSourceContent::Hcdf(Box::new(root)),
        source_sha: content_sha(&root_bytes),
        xml_element_count: count_xml_elements(&root_bytes),
        bytes: root_bytes,
        provenance: DocumentSourceProvenance::CanonicalModelSerialization,
    };
    load_document_tree_from_root_source(document, root_key, root_source, resolver, options)
}

pub fn load_projected_document_set_from_bytes<R: DocumentResolver>(
    root_bytes: Vec<u8>,
    document: DocumentIdentity,
    root_key: DocumentResourceKey,
    resolver: &mut R,
    options: DocumentSetOptions,
) -> Result<ProjectedConnectivityDocumentSet, DocumentSetError> {
    let tree = load_document_tree_from_bytes(root_bytes, document, root_key, resolver, options)?;
    project_document_tree(tree, resolver)
}

pub fn load_projected_document_set_from_model<R: DocumentResolver>(
    root: Hcdf,
    document: DocumentIdentity,
    root_key: DocumentResourceKey,
    resolver: &mut R,
    options: DocumentSetOptions,
) -> Result<ProjectedConnectivityDocumentSet, DocumentSetError> {
    let tree = load_document_tree_from_model(root, document, root_key, resolver, options)?;
    project_document_tree(tree, resolver)
}

fn load_document_tree_from_root_source<R: DocumentResolver>(
    document: DocumentIdentity,
    root_key: DocumentResourceKey,
    root_source: LoadedDocumentSource,
    resolver: &mut R,
    options: DocumentSetOptions,
) -> Result<LoadedDocumentTree, DocumentSetError> {
    check_limit(
        DocumentSetLimitKind::UniqueDocuments,
        options.limits.max_unique_documents,
        1,
        Some(root_key.as_str().to_owned()),
    )?;
    check_limit(
        DocumentSetLimitKind::Instances,
        options.limits.max_instances,
        1,
        Some(root_key.as_str().to_owned()),
    )?;
    check_limit(
        DocumentSetLimitKind::AggregateUniqueBytes,
        options.limits.max_aggregate_unique_bytes,
        root_source.bytes.len(),
        Some(root_key.as_str().to_owned()),
    )?;
    check_limit(
        DocumentSetLimitKind::ExpandedSourceBytes,
        options.limits.max_expanded_source_bytes,
        root_source.bytes.len(),
        Some(root_key.as_str().to_owned()),
    )?;
    check_limit(
        DocumentSetLimitKind::ExpandedXmlElements,
        options.limits.max_expanded_xml_elements,
        root_source.xml_element_count,
        Some(root_key.as_str().to_owned()),
    )?;

    let root_id = IncludeInstanceId::root();
    let root_instance = DocumentInstance {
        id: root_id.clone(),
        source: root_key.clone(),
        parent: None,
        include_site_index: None,
        authored_name: None,
        authored_uri: None,
        placement: PlacementState::Resolved(Box::new(ResolvedPlacement {
            local_to_parent: identity_matrix(),
            local_to_root: identity_matrix(),
        })),
        component_origin_adjustments: Vec::new(),
        include_sites: Vec::new(),
        children: Vec::new(),
    };
    let aggregate_unique_bytes = root_source.bytes.len();
    let work = DocumentSetWork {
        expanded_source_bytes: root_source.bytes.len(),
        expanded_xml_elements: root_source.xml_element_count,
        ..DocumentSetWork::default()
    };
    let mut loader = TreeLoader {
        resolver,
        options,
        sources: BTreeMap::from([(root_key.clone(), root_source)]),
        instances: BTreeMap::from([(root_id.clone(), root_instance)]),
        stream_profile_instances: BTreeMap::new(),
        stream_profile_sites: Vec::new(),
        resolution_cache: BTreeMap::new(),
        aggregate_unique_bytes,
        work,
    };
    loader.descend(root_id, vec![root_key.clone()], 0)?;
    let flatten_prefixes = build_flatten_prefix_map(&loader.instances);

    Ok(LoadedDocumentTree {
        document,
        root: root_key,
        sources: loader.sources,
        instances: loader.instances,
        stream_profile_instances: loader.stream_profile_instances,
        stream_profile_sites: loader.stream_profile_sites,
        flatten_prefixes,
        options,
        work: loader.work,
    })
}

pub fn project_document_tree<R: DocumentResolver>(
    tree: LoadedDocumentTree,
    resolver: &mut R,
) -> Result<ProjectedConnectivityDocumentSet, DocumentSetError> {
    let flattened = flatten_document_tree(&tree)?;
    let structural_projection = build_structural_projection(&tree, &flattened)?;
    let connectivity = build_connectivity_projection(&tree);
    let mut work = tree.work;
    let asset_sites = collect_asset_sites(&tree, &mut work)?;
    let assets = resolve_asset_sites(asset_sites, resolver, tree.options, &mut work)?;

    Ok(ProjectedConnectivityDocumentSet {
        sources: tree.sources,
        instances: tree.instances,
        stream_profile_instances: tree.stream_profile_instances,
        stream_profile_sites: tree.stream_profile_sites,
        flattened,
        structural_projection,
        connectivity,
        assets,
        work,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssetReferenceTemplate {
    site: ResourceSite,
    authored_uri: String,
    authored_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingAssetSite {
    source: DocumentResourceKey,
    instance: IncludeInstanceId,
    reference: AssetReferenceTemplate,
}

fn collect_asset_sites(
    tree: &LoadedDocumentTree,
    work: &mut DocumentSetWork,
) -> Result<Vec<PendingAssetSite>, DocumentSetError> {
    let source_keys = tree
        .instances
        .values()
        .map(|instance| instance.source.clone())
        .collect::<BTreeSet<_>>();
    let mut templates = BTreeMap::<DocumentResourceKey, Vec<AssetReferenceTemplate>>::new();
    for source_key in source_keys {
        let source = &tree.sources[&source_key];
        let mut document = source
            .content
            .hcdf()
            .expect("every instantiated source is parsed")
            .clone();
        let mut references = Vec::new();
        let result: Result<(), std::convert::Infallible> =
            visit_resources(&mut document, |site, reference| {
                if site.class() == ResourceClass::OpaqueAsset {
                    references.push(AssetReferenceTemplate {
                        site: site.clone(),
                        authored_uri: reference.uri.to_owned(),
                        authored_sha: reference.sha.map(str::to_owned),
                    });
                }
                Ok(ResourceRewrite::Keep)
            });
        match result {
            Ok(()) => {}
            Err(never) => match never {},
        }
        templates.insert(source_key, references);
    }

    let mut sites = Vec::new();
    for (instance_id, instance) in &tree.instances {
        for reference in &templates[&instance.source] {
            work.asset_sites = checked_limit_add(
                DocumentSetLimitKind::AssetSites,
                work.asset_sites,
                1,
                tree.options.limits.max_asset_sites,
                Some(reference.authored_uri.clone()),
            )?;
            if reference.authored_uri.trim().is_empty() {
                return Err(DocumentSetError::AssetResolution {
                    instance: instance_id.clone(),
                    site: Box::new(reference.site.clone()),
                    uri: reference.authored_uri.clone(),
                    failure: invalid_reference(ResourceReferenceError::EmptyReference),
                });
            }
            sites.push(PendingAssetSite {
                source: instance.source.clone(),
                instance: instance_id.clone(),
                reference: reference.clone(),
            });
        }
    }
    Ok(sites)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AssetResolutionCacheKey {
    parent: DocumentResourceKey,
    authored_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CachedAssetResolution {
    Resolved { key: AssetResourceKey },
    Failed { failure: ResolverFailure },
}

struct AssetLoader<'a, R> {
    resolver: &'a mut R,
    options: DocumentSetOptions,
    cache: BTreeMap<AssetResolutionCacheKey, CachedAssetResolution>,
    observed: BTreeMap<AssetResourceKey, ResolvedAsset>,
    used: BTreeSet<AssetResourceKey>,
    sites: Vec<AssetResourceSite>,
    work: &'a mut DocumentSetWork,
}

fn resolve_asset_sites<R: DocumentResolver>(
    sites: Vec<PendingAssetSite>,
    resolver: &mut R,
    options: DocumentSetOptions,
    work: &mut DocumentSetWork,
) -> Result<ResolvedAssetInventory, DocumentSetError> {
    let mut loader = AssetLoader {
        resolver,
        options,
        cache: BTreeMap::new(),
        observed: BTreeMap::new(),
        used: BTreeSet::new(),
        sites: Vec::with_capacity(sites.len()),
        work,
    };
    for site in sites {
        loader.resolve_site(site)?;
    }
    loader.observed.retain(|key, _| loader.used.contains(key));
    Ok(ResolvedAssetInventory {
        resources: loader.observed,
        sites: loader.sites,
    })
}

impl<R: DocumentResolver> AssetLoader<'_, R> {
    fn resolve_site(&mut self, pending: PendingAssetSite) -> Result<(), DocumentSetError> {
        let resolution =
            self.resolve_cached(&pending.source, pending.reference.authored_uri.as_str())?;
        let outcome = match resolution {
            CachedAssetResolution::Failed { failure }
                if retain_asset_failure(self.options.asset_policy, failure.kind) =>
            {
                AssetSiteResolution::Unresolved {
                    failure: AssetResolutionFailure::Resolver(failure),
                }
            }
            CachedAssetResolution::Failed { failure } => {
                return Err(DocumentSetError::AssetResolution {
                    instance: pending.instance,
                    site: Box::new(pending.reference.site),
                    uri: pending.reference.authored_uri,
                    failure,
                });
            }
            CachedAssetResolution::Resolved { key } => {
                let actual = self.observed[&key].content_sha.clone();
                if let Some(expected) = pending
                    .reference
                    .authored_sha
                    .as_deref()
                    .filter(|expected| !expected.is_empty())
                {
                    if expected != actual {
                        if self.options.asset_policy == AssetPolicy::RequireAllAssets {
                            return Err(DocumentSetError::AssetDigestMismatch {
                                instance: pending.instance,
                                site: Box::new(pending.reference.site),
                                uri: pending.reference.authored_uri,
                                expected: expected.to_owned(),
                                actual,
                            });
                        }
                        AssetSiteResolution::Unresolved {
                            failure: AssetResolutionFailure::DigestMismatch {
                                expected: expected.to_owned(),
                                actual,
                            },
                        }
                    } else {
                        self.used.insert(key.clone());
                        AssetSiteResolution::Resolved { key }
                    }
                } else {
                    self.used.insert(key.clone());
                    AssetSiteResolution::Resolved { key }
                }
            }
        };
        self.sites.push(AssetResourceSite {
            source: pending.source,
            instance: pending.instance,
            site: pending.reference.site,
            authored_uri: pending.reference.authored_uri,
            authored_sha: pending.reference.authored_sha,
            resolution: outcome,
        });
        Ok(())
    }

    fn resolve_cached(
        &mut self,
        parent: &DocumentResourceKey,
        authored_uri: &str,
    ) -> Result<CachedAssetResolution, DocumentSetError> {
        let cache_key = AssetResolutionCacheKey {
            parent: parent.clone(),
            authored_uri: authored_uri.to_owned(),
        };
        if let Some(cached) = self.cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        self.work.resolver_calls = checked_limit_add(
            DocumentSetLimitKind::ResolverCalls,
            self.work.resolver_calls,
            1,
            self.options.limits.max_resolver_calls,
            Some(authored_uri.to_owned()),
        )?;
        let resolution = match self.resolver.resolve_asset(parent, authored_uri) {
            Ok(resolved) => {
                self.work.resolver_bytes = checked_limit_add(
                    DocumentSetLimitKind::ResolverBytes,
                    self.work.resolver_bytes,
                    resolved.bytes.len(),
                    self.options.limits.max_resolver_bytes,
                    Some(resolved.key.as_str().to_owned()),
                )?;
                let asset = ResolvedAsset {
                    key: resolved.key.clone(),
                    content_sha: content_sha(&resolved.bytes),
                    bytes: resolved.bytes,
                };
                if let Some(existing) = self.observed.get(&asset.key) {
                    if existing.bytes != asset.bytes {
                        return Err(DocumentSetError::AssetSourceConflict { key: asset.key });
                    }
                } else {
                    self.observed.insert(asset.key.clone(), asset);
                }
                CachedAssetResolution::Resolved { key: resolved.key }
            }
            Err(failure) => CachedAssetResolution::Failed { failure },
        };
        self.cache.insert(cache_key, resolution.clone());
        Ok(resolution)
    }
}

fn retain_asset_failure(policy: AssetPolicy, kind: ResolverFailureKind) -> bool {
    policy == AssetPolicy::AllowMissing
        && matches!(
            kind,
            ResolverFailureKind::NotFound
                | ResolverFailureKind::Unsupported
                | ResolverFailureKind::Other
        )
}

struct ProjectionGuardNamespaceMaterial {
    canonical_text: Vec<String>,
    seed: String,
}

impl ProjectionGuardNamespaceMaterial {
    fn new(tree: &LoadedDocumentTree) -> Result<Self, DocumentSetError> {
        let mut canonical_text = Vec::new();
        let mut seed_material = String::new();
        for source in tree.sources.values() {
            let canonical_sha = if let Some(document) = source.content.hcdf() {
                let text = document.to_xml_string().map_err(|error| {
                    DocumentSetError::StructuralFlatten {
                        message: format!(
                            "failed to serialize source {} while reserving projection guards: {error}",
                            source.key
                        ),
                    }
                })?;
                let sha = content_sha(text.as_bytes());
                canonical_text.push(text);
                sha
            } else {
                source.source_sha.clone()
            };
            seed_material.push_str(&format!(
                "{}:{}:{}:{};",
                source.key.as_str().len(),
                source.key,
                source.source_sha,
                canonical_sha
            ));
        }
        let seed = content_sha(seed_material.as_bytes())
            .strip_prefix("sha256:")
            .unwrap_or_default()
            .to_owned();
        Ok(Self {
            canonical_text,
            seed,
        })
    }

    fn reserve(
        &self,
        tree: &LoadedDocumentTree,
        prefix: &str,
        exhausted_message: &str,
    ) -> Result<String, DocumentSetError> {
        let base = format!("{prefix}{}_", self.seed);
        let mut occupied_nonces = BTreeSet::new();
        for source in tree.sources.values() {
            collect_guard_namespace_nonces(
                source.key.as_str().as_bytes(),
                base.as_bytes(),
                &mut occupied_nonces,
            );
            collect_guard_namespace_nonces(&source.bytes, base.as_bytes(), &mut occupied_nonces);
        }
        for text in &self.canonical_text {
            collect_guard_namespace_nonces(text.as_bytes(), base.as_bytes(), &mut occupied_nonces);
        }
        let mut nonce = 0_u64;
        while occupied_nonces.contains(&nonce) {
            nonce = nonce
                .checked_add(1)
                .ok_or_else(|| DocumentSetError::StructuralFlatten {
                    message: exhausted_message.to_owned(),
                })?;
        }
        Ok(format!("{base}{nonce:016x}_"))
    }
}

struct GuardedUnresolvedInstanceReferences {
    namespace: String,
    next_index: u64,
    originals: BTreeMap<String, InstanceRef>,
}

impl GuardedUnresolvedInstanceReferences {
    fn new(namespace: String) -> Self {
        Self {
            namespace,
            next_index: 0,
            originals: BTreeMap::new(),
        }
    }

    fn replace(&mut self, reference: &mut Option<InstanceRef>) -> Result<(), String> {
        let original = reference
            .take()
            .ok_or_else(|| "unresolved connectivity reference has no instance path".to_owned())?;
        let index = self.next_index;
        self.next_index = self
            .next_index
            .checked_add(1)
            .ok_or_else(|| "unresolved connectivity reference guard space exhausted".to_owned())?;
        let guard = format!("{}{index:016x}", self.namespace);
        if self.originals.insert(guard.clone(), original).is_some() {
            return Err(format!(
                "unresolved connectivity reference guard {guard:?} was allocated more than once"
            ));
        }
        *reference = Some(InstanceRef {
            segment: vec![InstanceSegment {
                name: Some(guard),
                occurrence: 0,
            }],
        });
        Ok(())
    }

    fn restore(self, document: &mut Hcdf) -> Result<(), String> {
        let mut restored = BTreeSet::new();
        visit_connectivity_instance_ref_slots_mut(document, &mut |reference| {
            let guard = reference
                .as_ref()
                .and_then(|value| match value.segment.as_slice() {
                    [segment] if segment.occurrence == 0 => segment.name.as_deref(),
                    _ => None,
                })
                .map(str::to_owned);
            let Some(guard) = guard else {
                return Ok(());
            };
            let Some(original) = self.originals.get(&guard).cloned() else {
                return Ok(());
            };
            if !restored.insert(guard.clone()) {
                return Err(format!(
                    "unresolved connectivity reference guard {guard:?} appeared more than once"
                ));
            }
            *reference = Some(original);
            Ok(())
        })?;
        if restored.len() != self.originals.len() {
            return Err(format!(
                "restored {} of {} unresolved connectivity references",
                restored.len(),
                self.originals.len()
            ));
        }
        Ok(())
    }
}

struct GuardedResolvedAssetUris {
    namespace: String,
    next_index: u64,
    originals: BTreeMap<String, GuardedResolvedAssetReference>,
}

struct GuardedResolvedAssetReference {
    uri: String,
    sha: Option<String>,
}

impl GuardedResolvedAssetUris {
    fn new(namespace: String) -> Self {
        Self {
            namespace,
            next_index: 0,
            originals: BTreeMap::new(),
        }
    }

    fn replace(&mut self, uri: String, sha: Option<String>) -> Result<String, String> {
        let index = self.next_index;
        self.next_index = self
            .next_index
            .checked_add(1)
            .ok_or_else(|| "resolved asset URI guard space exhausted".to_owned())?;
        let guard = format!("{}{index:016x}", self.namespace);
        if self
            .originals
            .insert(guard.clone(), GuardedResolvedAssetReference { uri, sha })
            .is_some()
        {
            return Err(format!(
                "resolved asset URI guard {guard:?} was allocated more than once"
            ));
        }
        Ok(guard)
    }

    fn restore(self, document: &mut Hcdf) -> Result<(), String> {
        let mut restored_document = document.clone();
        let mut restored = BTreeSet::new();
        visit_resources(&mut restored_document, |site, reference| {
            if site.class() != ResourceClass::OpaqueAsset {
                return Ok(ResourceRewrite::Keep);
            }
            let Some(original) = self.originals.get(reference.uri) else {
                return Ok(ResourceRewrite::Keep);
            };
            if !restored.insert(reference.uri.to_owned()) {
                return Err(format!(
                    "resolved asset URI guard {:?} appeared more than once",
                    reference.uri
                ));
            }
            Ok(ResourceRewrite::Replace {
                uri: original.uri.clone(),
                sha: original.sha.clone(),
            })
        })?;
        if restored.len() != self.originals.len() {
            return Err(format!(
                "restored {} of {} resolved asset URIs",
                restored.len(),
                self.originals.len()
            ));
        }
        *document = restored_document;
        Ok(())
    }
}

fn collect_guard_namespace_nonces(text: &[u8], base: &[u8], nonces: &mut BTreeSet<u64>) {
    let suffix_len = 17;
    if text.len() < base.len() + suffix_len {
        return;
    }
    for start in 0..=text.len() - base.len() - suffix_len {
        if &text[start..start + base.len()] != base {
            continue;
        }
        let nonce_start = start + base.len();
        let nonce_end = nonce_start + 16;
        if text[nonce_end] != b'_' {
            continue;
        }
        if let Some(nonce) = parse_lower_hex_u64(&text[nonce_start..nonce_end]) {
            nonces.insert(nonce);
        }
    }
}

fn parse_lower_hex_u64(text: &[u8]) -> Option<u64> {
    if text.len() != 16 {
        return None;
    }
    text.iter().try_fold(0_u64, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            b'a'..=b'f' => u64::from(byte - b'a' + 10),
            _ => return None,
        };
        Some((value << 4) | digit)
    })
}

fn flatten_document_tree(tree: &LoadedDocumentTree) -> Result<Hcdf, DocumentSetError> {
    let guard_material = ProjectionGuardNamespaceMaterial::new(tree)?;
    let unresolved_namespace = guard_material.reserve(
        tree,
        "__hcdf_unresolved_reference_",
        "unresolved connectivity reference namespace space exhausted",
    )?;
    let asset_namespace = guard_material.reserve(
        tree,
        "hcdf-asset-guard:",
        "resolved asset URI namespace space exhausted",
    )?;
    let mut unresolved_references = GuardedUnresolvedInstanceReferences::new(unresolved_namespace);
    let mut resolved_asset_uris = GuardedResolvedAssetUris::new(asset_namespace);
    let synthetic_keys = tree
        .instances
        .keys()
        .filter(|instance| !instance.is_root())
        .enumerate()
        .map(|(index, instance)| {
            (
                instance.clone(),
                format!("/__hcdf_document_set/{index}.hcdf"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut retained_sentinels = BTreeMap::new();
    let mut retained_authored_uris = BTreeMap::new();
    let mut retained_index = 0;
    for (instance_id, instance) in &tree.instances {
        for site in &instance.include_sites {
            if site.outcome == DocumentIncludeSiteOutcome::RetainedUnresolved {
                let sentinel = format!("/__hcdf_document_set_retained/{retained_index}.hcdf");
                retained_index += 1;
                retained_sentinels
                    .insert((instance_id.clone(), site.include_index), sentinel.clone());
                retained_authored_uris.insert(sentinel, site.authored_uri.clone());
            }
        }
    }
    let mut prepared = BTreeMap::<String, Hcdf>::new();
    let mut root = None;
    for instance_id in tree.instances.keys() {
        let document = prepare_instance_document(
            tree,
            instance_id,
            &synthetic_keys,
            &retained_sentinels,
            &mut unresolved_references,
            &mut resolved_asset_uris,
        )?;
        if instance_id.is_root() {
            root = Some(document);
        } else {
            prepared.insert(synthetic_keys[instance_id].clone(), document);
        }
    }
    let mut flattened = root.expect("every loaded document tree has a root instance");
    let mut loader = |key: &str, _base: &Path| {
        prepared
            .get(key)
            .cloned()
            .map(|document| (document, None))
            .ok_or_else(|| format!("synthetic document {key:?} is not present"))
    };
    flatten_with(&mut flattened, Path::new("/"), &mut loader)
        .map_err(|message| DocumentSetError::StructuralFlatten { message })?;
    resolved_asset_uris
        .restore(&mut flattened)
        .map_err(|message| DocumentSetError::StructuralFlatten { message })?;
    unresolved_references
        .restore(&mut flattened)
        .map_err(|message| DocumentSetError::StructuralFlatten { message })?;

    let mut restored = BTreeSet::new();
    for include in &mut flattened.include {
        let Some(uri) = include.uri.as_deref() else {
            continue;
        };
        if let Some(authored_uri) = retained_authored_uris.get(uri) {
            restored.insert(uri.to_owned());
            include.uri = Some(authored_uri.clone());
        }
    }
    if restored.len() != retained_authored_uris.len() {
        return Err(DocumentSetError::StructuralFlatten {
            message: format!(
                "restored {} of {} retained include URIs",
                restored.len(),
                retained_authored_uris.len()
            ),
        });
    }
    Ok(flattened)
}

fn prepare_instance_document(
    tree: &LoadedDocumentTree,
    instance_id: &IncludeInstanceId,
    synthetic_keys: &BTreeMap<IncludeInstanceId, String>,
    retained_sentinels: &BTreeMap<(IncludeInstanceId, usize), String>,
    unresolved_references: &mut GuardedUnresolvedInstanceReferences,
    resolved_asset_uris: &mut GuardedResolvedAssetUris,
) -> Result<Hcdf, DocumentSetError> {
    let instance = &tree.instances[instance_id];
    let source = &tree.sources[&instance.source];
    let mut document = source
        .content
        .hcdf()
        .expect("every instantiated source is parsed")
        .clone();

    if !instance_id.is_root() {
        visit_resources(&mut document, |site, reference| {
            if site.class() != ResourceClass::OpaqueAsset || reference.uri.is_empty() {
                return Ok::<_, ResolverFailure>(ResourceRewrite::Keep);
            }
            let uri = if reference.uri.trim().is_empty() {
                reference.uri.to_owned()
            } else {
                resolve_resource_reference(source.key.as_str(), reference.uri)
                    .map_err(invalid_reference)?
            };
            let uri = resolved_asset_uris
                .replace(uri, reference.sha.map(str::to_owned))
                .map_err(|message| ResolverFailure::new(ResolverFailureKind::Other, message))?;
            Ok(ResourceRewrite::Replace {
                uri,
                sha: reference.sha.map(str::to_owned),
            })
        })
        .map_err(|failure| DocumentSetError::StructuralFlatten {
            message: format!(
                "failed to establish asset base for source {}: {failure}",
                source.key
            ),
        })?;
    }

    visit_connectivity_instance_ref_slots_mut(&mut document, &mut |reference| {
        let unresolved = remap_flatten_instance_reference(reference, instance_id, tree);
        if unresolved {
            unresolved_references.replace(reference)?;
        }
        Ok(())
    })
    .map_err(|message| DocumentSetError::StructuralFlatten { message })?;

    if document.include.len() != instance.include_sites.len() {
        return Err(DocumentSetError::StructuralFlatten {
            message: format!(
                "instance {:?} has {} authored includes but {} recorded include sites",
                instance_id,
                document.include.len(),
                instance.include_sites.len()
            ),
        });
    }
    for (include_index, (include, site)) in document
        .include
        .iter_mut()
        .zip(&instance.include_sites)
        .enumerate()
    {
        if site.include_index != include_index {
            return Err(DocumentSetError::StructuralFlatten {
                message: format!(
                    "instance {:?} include site {} records index {}",
                    instance_id, include_index, site.include_index
                ),
            });
        }
        if include.uri.as_deref() != Some(site.authored_uri.as_str())
            || include.sha.as_deref() != site.authored_sha.as_deref()
        {
            return Err(DocumentSetError::StructuralFlatten {
                message: format!(
                    "instance {:?} include site {} does not match its authored URI or digest",
                    instance_id, include_index
                ),
            });
        }
        match &site.outcome {
            DocumentIncludeSiteOutcome::Resolved { child } => {
                include.uri = Some(synthetic_keys[child].clone());
                include.sha = None;
            }
            DocumentIncludeSiteOutcome::RetainedUnresolved => {
                include.uri =
                    Some(retained_sentinels[&(instance_id.clone(), include_index)].clone());
            }
        }
    }
    Ok(document)
}

fn remap_flatten_instance_reference(
    reference: &mut Option<InstanceRef>,
    current: &IncludeInstanceId,
    tree: &LoadedDocumentTree,
) -> bool {
    let Some(mut instance_reference) = reference.take() else {
        return false;
    };
    if instance_reference.segment.is_empty() {
        *reference = Some(instance_reference);
        return false;
    }

    let authored = std::mem::take(&mut instance_reference.segment);
    let mut cursor = current.clone();
    let mut unresolved = false;
    let mut mapped = Vec::with_capacity(authored.len());

    for segment in authored {
        let authored_name = match &segment.name {
            None => IncludeSegmentName::Unnamed,
            Some(name) => IncludeSegmentName::Named(name.clone()),
        };
        let child = (!unresolved)
            .then(|| {
                tree.instances[&cursor]
                    .children
                    .iter()
                    .find(|child| {
                        child.segments().last().is_some_and(|candidate| {
                            candidate.name.eq(&authored_name)
                                && candidate.occurrence == segment.occurrence
                        })
                    })
                    .cloned()
            })
            .flatten();
        if let Some(child) = child {
            if let Some(actual_segment) = tree
                .flatten_prefixes
                .segment(&child)
                .expect("every nonroot instance has an explicit flatten segment state")
            {
                mapped.push(InstanceSegment {
                    name: Some(actual_segment.to_owned()),
                    occurrence: 0,
                });
            }
            cursor = child;
        } else {
            unresolved = true;
            mapped.push(segment);
        }
    }

    if !mapped.is_empty() {
        instance_reference.segment = mapped;
        *reference = Some(instance_reference);
    }
    unresolved
}

fn build_structural_projection(
    tree: &LoadedDocumentTree,
    flattened: &Hcdf,
) -> Result<StructuralSceneProjection, DocumentSetError> {
    let mut flattened_components = BTreeMap::<String, usize>::new();
    for (index, component) in flattened.comp.iter().enumerate() {
        if flattened_components
            .insert(component.name.clone(), index)
            .is_some()
        {
            return Err(DocumentSetError::StructuralProjection {
                instance: IncludeInstanceId::root(),
                kind: ObjectKind::Component,
                local_name: component.name.clone(),
                message: "flattened component name is not unique".to_owned(),
            });
        }
    }

    let mut components = Vec::new();
    let mut visuals = Vec::new();
    let mut frames = Vec::new();
    for (instance_id, instance) in &tree.instances {
        let source = &tree.sources[&instance.source];
        let prefix = tree.flatten_prefixes.prefix(instance_id).unwrap();
        let source_document = source
            .content
            .hcdf()
            .expect("every instantiated source is parsed");
        for component in &source_document.comp {
            let flattened_name = join_flatten_name(prefix, &component.name);
            let flattened_index = flattened_components
                .get(&flattened_name)
                .copied()
                .ok_or_else(|| DocumentSetError::StructuralProjection {
                    instance: instance_id.clone(),
                    kind: ObjectKind::Component,
                    local_name: component.name.clone(),
                    message: format!("flattened component {flattened_name:?} is missing"),
                })?;
            let flattened_component = &flattened.comp[flattened_index];
            if flattened_component.visual.len() != component.visual.len()
                || flattened_component.frame.len() != component.frame.len()
            {
                return Err(DocumentSetError::StructuralProjection {
                    instance: instance_id.clone(),
                    kind: ObjectKind::Component,
                    local_name: component.name.clone(),
                    message: "flattened visual or frame counts differ from the source".to_owned(),
                });
            }

            let component_identity =
                structural_component_identity(&tree.document, instance_id, &component.name);
            let component_id = component_identity.stable_id();
            components.push(StructuralComponentProjection {
                identity: component_identity,
                id: component_id.clone(),
                source: source.key.clone(),
                instance: instance_id.clone(),
                local_name: component.name.clone(),
                flattened_name: flattened_name.clone(),
                flattened_index,
                placement: instance.placement.clone(),
            });

            for (visual_index, visual) in component.visual.iter().enumerate() {
                if flattened_component.visual[visual_index].name != visual.name {
                    return Err(DocumentSetError::StructuralProjection {
                        instance: instance_id.clone(),
                        kind: ObjectKind::StructuralVisualRoot,
                        local_name: visual.name.clone(),
                        message: "flattened visual index does not retain the source name"
                            .to_owned(),
                    });
                }
                let identity = structural_visual_identity(
                    &tree.document,
                    instance_id,
                    &component.name,
                    &visual.name,
                );
                visuals.push(StructuralVisualProjection {
                    id: identity.stable_id(),
                    identity,
                    component_id: component_id.clone(),
                    source: source.key.clone(),
                    instance: instance_id.clone(),
                    local_component: component.name.clone(),
                    local_name: visual.name.clone(),
                    flattened_component: flattened_name.clone(),
                    flattened_component_index: flattened_index,
                    flattened_visual_index: visual_index,
                });
            }

            for (frame_index, frame) in component.frame.iter().enumerate() {
                if flattened_component.frame[frame_index].name != frame.name {
                    return Err(DocumentSetError::StructuralProjection {
                        instance: instance_id.clone(),
                        kind: ObjectKind::StructuralFrame,
                        local_name: frame.name.clone(),
                        message: "flattened frame index does not retain the source name".to_owned(),
                    });
                }
                let identity = structural_frame_identity(
                    &tree.document,
                    instance_id,
                    &component.name,
                    &frame.name,
                );
                frames.push(StructuralFrameProjection {
                    id: identity.stable_id(),
                    identity,
                    component_id: component_id.clone(),
                    source: source.key.clone(),
                    instance: instance_id.clone(),
                    local_component: component.name.clone(),
                    local_name: frame.name.clone(),
                    flattened_component: flattened_name.clone(),
                    flattened_component_index: flattened_index,
                    flattened_frame_index: frame_index,
                });
            }
        }
    }

    if components.len() != flattened.comp.len() {
        return Err(DocumentSetError::StructuralProjection {
            instance: IncludeInstanceId::root(),
            kind: ObjectKind::Component,
            local_name: String::new(),
            message: format!(
                "{} projected components do not cover {} flattened components",
                components.len(),
                flattened.comp.len()
            ),
        });
    }
    components.sort_by_key(|component| component.flattened_index);
    visuals.sort_by_key(|visual| {
        (
            visual.flattened_component_index,
            visual.flattened_visual_index,
        )
    });
    frames.sort_by_key(|frame| (frame.flattened_component_index, frame.flattened_frame_index));

    Ok(StructuralSceneProjection {
        flatten_prefixes: tree.flatten_prefixes.clone(),
        placement_projection_complete: tree
            .instances
            .values()
            .all(|instance| instance.placement.is_projected()),
        components,
        visuals,
        frames,
    })
}

fn build_connectivity_projection(tree: &LoadedDocumentTree) -> ConnectivityProjection {
    let mut issues = Vec::new();
    for (instance_id, instance) in &tree.instances {
        for site in &instance.include_sites {
            for diagnostic in &site.diagnostics {
                issues.push(ProjectedConnectivityIssue::Dependency {
                    instance: instance_id.clone(),
                    include_index: site.include_index,
                    diagnostic: diagnostic.clone(),
                });
            }
        }
    }
    for site in &tree.stream_profile_sites {
        for diagnostic in &site.diagnostics {
            if stream_profile_diagnostic_blocks_connectivity(site.required, diagnostic) {
                issues.push(ProjectedConnectivityIssue::StreamProfileDependency {
                    id: site.id.clone(),
                    diagnostic: diagnostic.clone(),
                });
            }
        }
    }
    if !issues.is_empty() {
        return ConnectivityProjection::Invalid { issues };
    }

    let mut scopes = Vec::with_capacity(tree.instances.len());
    for (instance_id, instance) in &tree.instances {
        let source = tree.sources[&instance.source]
            .content
            .hcdf()
            .expect("every instantiated source is parsed");
        match source.to_connectivity_scope(instance_id.clone()) {
            Ok(scope) => scopes.push(scope),
            Err(error) => {
                issues.push(ProjectedConnectivityIssue::Conversion {
                    instance: instance_id.clone(),
                    error,
                });
            }
        }
    }

    if !issues.is_empty() {
        return ConnectivityProjection::Invalid { issues };
    }
    let authored = ConnectivityDocument {
        document: tree.document.clone(),
        scopes,
    };
    let graph = match normalize_connectivity(&authored) {
        Ok(graph) => graph,
        Err(error) => {
            return ConnectivityProjection::Invalid {
                issues: error
                    .into_issues()
                    .into_iter()
                    .map(ProjectedConnectivityIssue::Normalization)
                    .collect(),
            };
        }
    };
    match extend_connectivity_with_stream_profiles(tree, graph) {
        Ok(graph) => ConnectivityProjection::Valid { authored, graph },
        Err(issues) => ConnectivityProjection::Invalid {
            issues: issues
                .into_iter()
                .map(ProjectedConnectivityIssue::Normalization)
                .collect(),
        },
    }
}

fn stream_profile_diagnostic_blocks_connectivity(
    required: bool,
    diagnostic: &StreamProfileDependencyDiagnostic,
) -> bool {
    required
        || !matches!(
            diagnostic,
            StreamProfileDependencyDiagnostic::ResolutionFailure { failure }
                if is_editor_retainable_failure(failure.kind)
        )
}

type ProfileNodeIndex = BTreeMap<StreamProfileInstanceId, (StableObjectId, ObjectIdentity)>;

fn extend_connectivity_with_stream_profiles(
    tree: &LoadedDocumentTree,
    graph: NormalizedConnectivityGraph,
) -> Result<NormalizedConnectivityGraph, Vec<ConnectivityIssue>> {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut issues = Vec::new();
    let mut profiles = ProfileNodeIndex::new();
    let mut group_identities = BTreeMap::<(IncludeInstanceId, String), ObjectIdentity>::new();
    let mut group_targets = Vec::new();
    let mut stream_nodes = BTreeMap::<(StreamProfileInstanceId, usize), StableObjectId>::new();
    let mut selected_defaults = BTreeMap::<IncludeInstanceId, ObjectIdentity>::new();

    for site in &tree.stream_profile_sites {
        if site.parent_profile.is_some()
            || site.selection_role != Some(StreamProfileSelectionRole::Default)
        {
            continue;
        }
        let identity = stream_profile_site_identity(tree, site);
        if let Some(first) = selected_defaults.insert(site.owner.clone(), identity.clone()) {
            issues.push(ConnectivityIssue::error(
                "E_CONN_PROFILE_DEFAULT_SELECTION",
                "an HCDF include instance may select at most one default stream profile",
                identity,
                vec![first],
            ));
        }
    }

    for (instance_id, instance) in &tree.stream_profile_instances {
        let profile = tree.sources[&instance.source]
            .content
            .stream_profile()
            .expect("every loaded profile instance source is parsed");
        let profile_identity = stream_profile_object_identity(&tree.document, instance_id, profile);
        validate_profile_metadata(profile, &profile_identity, &mut issues);
        let profile_node = ConnectivityNode::new(
            profile_identity.clone(),
            ConnectivityNodeData::Profile {
                name: profile.name.clone(),
                version: profile.version.clone(),
                description: profile.description.clone(),
                dependencies: profile.dependency.clone(),
            },
        );
        let profile_id = profile_node.id().clone();
        profiles.insert(
            instance_id.clone(),
            (profile_id.clone(), profile_identity.clone()),
        );
        nodes.push(profile_node);

        let mut local_groups = BTreeMap::<String, ObjectIdentity>::new();
        for group in &profile.stream_group {
            let identity = stream_group_object_identity(&tree.document, instance_id, &group.name);
            if group.name.trim().is_empty() {
                issues.push(ConnectivityIssue::error(
                    "E_CONN_PROFILE_GROUP_NAME",
                    "stream group names must not be empty",
                    identity.clone(),
                    Vec::new(),
                ));
            }
            if let Some(first) = local_groups.insert(group.name.clone(), identity.clone()) {
                issues.push(ConnectivityIssue::error(
                    "E_CONN_PROFILE_DUPLICATE_GROUP",
                    "stream group names must be unique within a profile",
                    identity,
                    vec![first],
                ));
                continue;
            }
            let node = ConnectivityNode::new(
                identity.clone(),
                ConnectivityNodeData::Group {
                    group: group.clone(),
                },
            );
            let id = node.id().clone();
            nodes.push(node);
            edges.push(ConnectivityGraphExtensionEdge::new(
                EdgeKind::ProfileContainment,
                profile_id.clone(),
                id.clone(),
                profile_id.clone(),
                EdgeExactness::Exact,
                "group",
            ));
            let key = (instance.owner.clone(), group.name.clone());
            if let Some(first) = group_identities.insert(key, identity.clone()) {
                issues.push(ConnectivityIssue::error(
                    "E_CONN_PROFILE_AMBIGUOUS_GROUP",
                    "stream group names must identify one target per HCDF include instance",
                    identity.clone(),
                    vec![first],
                ));
            }
            group_targets.push(StreamGroupReferenceTarget::new(
                instance.owner.clone(),
                group.name.clone(),
                id,
            ));
        }

        let mut local_streams = BTreeMap::<String, ObjectIdentity>::new();
        for (stream_index, stream) in profile.stream.iter().enumerate() {
            let identity = stream_object_identity(&tree.document, instance_id, &stream.name);
            if stream.name.trim().is_empty() {
                issues.push(ConnectivityIssue::error(
                    "E_CONN_PROFILE_STREAM_NAME",
                    "stream names must not be empty",
                    identity.clone(),
                    Vec::new(),
                ));
            }
            if stream.listener.is_empty() {
                issues.push(ConnectivityIssue::error(
                    "E_CONN_PROFILE_STREAM_LISTENER",
                    "a stream requires at least one listener",
                    identity.clone(),
                    Vec::new(),
                ));
            }
            if let Some(first) = local_streams.insert(stream.name.clone(), identity.clone()) {
                issues.push(ConnectivityIssue::error(
                    "E_CONN_PROFILE_DUPLICATE_STREAM",
                    "stream names must be unique within a profile",
                    identity,
                    vec![first],
                ));
                continue;
            }
            let node = ConnectivityNode::new(
                identity,
                ConnectivityNodeData::Stream {
                    definition: Box::new(stream.clone()),
                },
            );
            let id = node.id().clone();
            nodes.push(node);
            stream_nodes.insert((instance_id.clone(), stream_index), id.clone());
            edges.push(ConnectivityGraphExtensionEdge::new(
                EdgeKind::ProfileContainment,
                profile_id.clone(),
                id,
                profile_id.clone(),
                EdgeExactness::Exact,
                "stream",
            ));
        }
    }

    for (instance_id, instance) in &tree.stream_profile_instances {
        let (profile_id, profile_identity) = &profiles[instance_id];
        if let Some(parent) = &instance.parent {
            if let Some((parent_id, _)) = profiles.get(parent) {
                edges.push(ConnectivityGraphExtensionEdge::new(
                    EdgeKind::ProfileDependency,
                    parent_id.clone(),
                    profile_id.clone(),
                    parent_id.clone(),
                    EdgeExactness::Exact,
                    instance_id.display_path(),
                ));
            } else {
                issues.push(ConnectivityIssue::error(
                    "E_CONN_PROFILE_DEPENDENCY_PARENT",
                    "a loaded nested profile requires its loaded parent profile",
                    profile_identity.clone(),
                    Vec::new(),
                ));
            }
        }
    }

    sort_connectivity_issues(&mut issues);
    if !issues.is_empty() {
        return Err(issues);
    }
    let graph = graph
        .with_extension(
            ConnectivityGraphExtension::new(nodes, edges).with_stream_group_targets(group_targets),
        )
        .map_err(|error| profile_graph_extension_issue(tree, error))?;

    let mut forwarding_nodes = Vec::new();
    let mut stream_edges = Vec::new();
    for (instance_id, instance) in &tree.stream_profile_instances {
        let profile = tree.sources[&instance.source]
            .content
            .stream_profile()
            .expect("every loaded profile instance source is parsed");
        for (stream_index, stream) in profile.stream.iter().enumerate() {
            let Some(stream_id) = stream_nodes.get(&(instance_id.clone(), stream_index)) else {
                continue;
            };
            let stream_identity = stream_object_identity(&tree.document, instance_id, &stream.name);
            connect_stream_group(
                &graph,
                stream.group_ref.as_ref(),
                instance,
                stream_id,
                &stream_identity,
                &mut stream_edges,
                &mut issues,
            );
            let path = connect_stream_path(
                &graph,
                stream,
                instance,
                stream_id,
                &stream_identity,
                &mut stream_edges,
                &mut issues,
            );
            let context = StreamConnectionContext {
                graph: &graph,
                profile_instance: instance,
                stream_id,
                subject: &stream_identity,
                path_networks: &path.networks,
            };
            let endpoints =
                connect_stream_participants(&context, stream, &mut stream_edges, &mut issues);
            connect_stream_forwarding(
                &context,
                stream,
                &path,
                &endpoints,
                instance_id,
                &tree.document,
                &mut forwarding_nodes,
                &mut stream_edges,
                &mut issues,
            );
            connect_stream_traffic_class(
                &context,
                stream.traffic_class_ref.as_ref(),
                &mut stream_edges,
                &mut issues,
            );
            connect_stream_schedule(
                &context,
                stream.schedule_ref.as_ref(),
                &mut stream_edges,
                &mut issues,
            );
        }
    }
    sort_connectivity_issues(&mut issues);
    if !issues.is_empty() {
        return Err(issues);
    }
    graph
        .with_extension(ConnectivityGraphExtension::new(
            forwarding_nodes,
            stream_edges,
        ))
        .map_err(|error| profile_graph_extension_issue(tree, error))
}

fn profile_graph_extension_issue(
    tree: &LoadedDocumentTree,
    error: impl std::fmt::Display,
) -> Vec<ConnectivityIssue> {
    vec![ConnectivityIssue::error(
        "E_CONN_PROFILE_GRAPH_EXTENSION",
        error.to_string(),
        ObjectIdentity::new(
            tree.document.clone(),
            IncludeInstanceId::root(),
            ObjectKind::Scope,
            Vec::new(),
        ),
        Vec::new(),
    )]
}

fn validate_profile_metadata(
    profile: &StreamProfileDocument,
    identity: &ObjectIdentity,
    issues: &mut Vec<ConnectivityIssue>,
) {
    if profile.name.trim().is_empty() {
        issues.push(ConnectivityIssue::error(
            "E_CONN_PROFILE_NAME",
            "stream profile names must not be empty",
            identity.clone(),
            Vec::new(),
        ));
    }
    if profile.version.trim().is_empty() {
        issues.push(ConnectivityIssue::error(
            "E_CONN_PROFILE_VERSION",
            "stream profile versions must not be empty",
            identity.clone(),
            Vec::new(),
        ));
    }
}

fn stream_profile_identity_parts(instance: &StreamProfileInstanceId) -> Vec<IdentityPart> {
    let mut local = vec![IdentityPart::new(
        "profile-declaration",
        instance.declaration_index.to_string(),
    )];
    for dependency_index in &instance.dependency_path {
        local.push(IdentityPart::new(
            "profile-dependency",
            dependency_index.to_string(),
        ));
    }
    local
}

fn stream_profile_object_identity(
    document: &DocumentIdentity,
    instance: &StreamProfileInstanceId,
    profile: &StreamProfileDocument,
) -> ObjectIdentity {
    let mut local = stream_profile_identity_parts(instance);
    local.push(IdentityPart::new("profile", profile.name.clone()));
    local.push(IdentityPart::new("version", profile.version.clone()));
    ObjectIdentity::new(
        document.clone(),
        instance.owner.clone(),
        ObjectKind::Profile,
        local,
    )
}

fn stream_profile_site_identity(
    tree: &LoadedDocumentTree,
    site: &StreamProfileDependencySite,
) -> ObjectIdentity {
    ObjectIdentity::new(
        tree.document.clone(),
        site.owner.clone(),
        ObjectKind::Profile,
        stream_profile_identity_parts(&site.id),
    )
}

fn stream_group_object_identity(
    document: &DocumentIdentity,
    instance: &StreamProfileInstanceId,
    group: &str,
) -> ObjectIdentity {
    let mut local = stream_profile_identity_parts(instance);
    local.push(IdentityPart::new("group", group));
    ObjectIdentity::new(
        document.clone(),
        instance.owner.clone(),
        ObjectKind::Group,
        local,
    )
}

fn stream_object_identity(
    document: &DocumentIdentity,
    instance: &StreamProfileInstanceId,
    stream: &str,
) -> ObjectIdentity {
    let mut local = stream_profile_identity_parts(instance);
    local.push(IdentityPart::new("stream", stream));
    ObjectIdentity::new(
        document.clone(),
        instance.owner.clone(),
        ObjectKind::Stream,
        local,
    )
}

fn connect_stream_group(
    graph: &NormalizedConnectivityGraph,
    reference: Option<&XmlStreamGroupRef>,
    profile_instance: &LoadedStreamProfileInstance,
    stream_id: &StableObjectId,
    subject: &ObjectIdentity,
    edges: &mut Vec<ConnectivityGraphExtensionEdge>,
    issues: &mut Vec<ConnectivityIssue>,
) {
    let Some(reference) = reference else {
        return;
    };
    let canonical = match canonical_stream_group_reference(reference, "group-ref") {
        Ok(reference) => reference,
        Err(message) => {
            push_profile_reference_issue(issues, subject, "group-ref", message);
            return;
        }
    };
    match graph
        .resolver(profile_instance.owner.clone())
        .stream_group(&canonical)
    {
        Ok(target) => edges.push(ConnectivityGraphExtensionEdge::new(
            EdgeKind::StreamGroup,
            stream_id.clone(),
            target.id().clone(),
            stream_id.clone(),
            EdgeExactness::Exact,
            "group",
        )),
        Err(error) => push_profile_reference_issue(issues, subject, "group-ref", error.to_string()),
    }
}

#[derive(Default)]
struct ResolvedStreamPath {
    networks: BTreeSet<StableObjectId>,
}

fn connect_stream_path(
    graph: &NormalizedConnectivityGraph,
    stream: &crate::model::StreamDefinition,
    profile_instance: &LoadedStreamProfileInstance,
    stream_id: &StableObjectId,
    subject: &ObjectIdentity,
    edges: &mut Vec<ConnectivityGraphExtensionEdge>,
    issues: &mut Vec<ConnectivityIssue>,
) -> ResolvedStreamPath {
    let mut path = ResolvedStreamPath::default();
    for (index, reference) in stream.path.network.iter().enumerate() {
        let reference_path = format!("path/network-ref[{index}]");
        let canonical = match canonical_network_reference(reference, &reference_path) {
            Ok(reference) => reference,
            Err(message) => {
                push_profile_reference_issue(issues, subject, &reference_path, message);
                continue;
            }
        };
        match graph
            .resolver(profile_instance.owner.clone())
            .network(&canonical)
        {
            Ok(target) => {
                if !path.networks.insert(target.id().clone()) {
                    issues.push(ConnectivityIssue::error(
                        "E_CONN_PROFILE_DUPLICATE_PATH_NETWORK",
                        "a stream path may list each canonical network only once",
                        subject.clone(),
                        vec![target.identity().clone()],
                    ));
                    continue;
                }
                edges.push(ConnectivityGraphExtensionEdge::new(
                    EdgeKind::StreamPath,
                    stream_id.clone(),
                    target.id().clone(),
                    stream_id.clone(),
                    EdgeExactness::Exact,
                    target.id().as_str(),
                ));
            }
            Err(error) => {
                push_profile_reference_issue(issues, subject, &reference_path, error.to_string())
            }
        }
    }
    path
}

struct StreamConnectionContext<'a> {
    graph: &'a NormalizedConnectivityGraph,
    profile_instance: &'a LoadedStreamProfileInstance,
    stream_id: &'a StableObjectId,
    subject: &'a ObjectIdentity,
    path_networks: &'a BTreeSet<StableObjectId>,
}

struct ResolvedStreamParticipant {
    network: StableObjectId,
}

#[derive(Default)]
struct ResolvedStreamEndpoints {
    talker_network: Option<StableObjectId>,
    listener_networks: BTreeSet<StableObjectId>,
}

fn connect_stream_participants(
    context: &StreamConnectionContext<'_>,
    stream: &crate::model::StreamDefinition,
    edges: &mut Vec<ConnectivityGraphExtensionEdge>,
    issues: &mut Vec<ConnectivityIssue>,
) -> ResolvedStreamEndpoints {
    let talker_network = connect_stream_participant(
        context,
        &stream.talker.participant,
        EdgeKind::StreamTalker,
        "talker".to_owned(),
        None,
        edges,
        issues,
    )
    .map(|resolved| resolved.network);
    let mut listener_targets = BTreeSet::new();
    let mut listener_networks = BTreeSet::new();
    for listener in &stream.listener {
        if let Some(resolved) = connect_stream_participant(
            context,
            &listener.participant,
            EdgeKind::StreamListener,
            "listener".to_owned(),
            Some(&mut listener_targets),
            edges,
            issues,
        ) {
            listener_networks.insert(resolved.network);
        }
    }
    ResolvedStreamEndpoints {
        talker_network,
        listener_networks,
    }
}

fn connect_stream_participant(
    context: &StreamConnectionContext<'_>,
    reference: &XmlParticipantRef,
    edge_kind: EdgeKind,
    discriminator: String,
    listener_targets: Option<&mut BTreeSet<StableObjectId>>,
    edges: &mut Vec<ConnectivityGraphExtensionEdge>,
    issues: &mut Vec<ConnectivityIssue>,
) -> Option<ResolvedStreamParticipant> {
    let canonical = match canonical_participant_reference(reference, &discriminator) {
        Ok(reference) => reference,
        Err(message) => {
            push_profile_reference_issue(issues, context.subject, &discriminator, message);
            return None;
        }
    };
    match context
        .graph
        .resolver(context.profile_instance.owner.clone())
        .participant(&canonical)
    {
        Ok(target) => {
            let network = stream_reference_matches_path(
                context.graph,
                &context.profile_instance.owner,
                &canonical.network,
                context.path_networks,
                context.subject,
                &discriminator,
                issues,
            )?;
            if let Some(listener_targets) = listener_targets {
                if !listener_targets.insert(target.id().clone()) {
                    issues.push(ConnectivityIssue::error(
                        "E_CONN_PROFILE_DUPLICATE_LISTENER",
                        "a stream listener participant may be declared only once",
                        context.subject.clone(),
                        vec![target.identity().clone()],
                    ));
                    return None;
                }
            }
            edges.push(ConnectivityGraphExtensionEdge::new(
                edge_kind,
                context.stream_id.clone(),
                target.id().clone(),
                context.stream_id.clone(),
                EdgeExactness::Exact,
                discriminator,
            ));
            Some(ResolvedStreamParticipant { network })
        }
        Err(error) => {
            push_profile_reference_issue(
                issues,
                context.subject,
                &discriminator,
                error.to_string(),
            );
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn connect_stream_forwarding(
    context: &StreamConnectionContext<'_>,
    stream: &crate::model::StreamDefinition,
    path: &ResolvedStreamPath,
    endpoints: &ResolvedStreamEndpoints,
    profile_instance_id: &StreamProfileInstanceId,
    document: &DocumentIdentity,
    nodes: &mut Vec<ConnectivityNode>,
    edges: &mut Vec<ConnectivityGraphExtensionEdge>,
    issues: &mut Vec<ConnectivityIssue>,
) {
    let mut triples = BTreeSet::new();
    let mut adjacency = BTreeMap::<StableObjectId, BTreeSet<StableObjectId>>::new();

    for (index, forwarding) in stream.path.forwarding.iter().enumerate() {
        let forwarding_path = format!("path/forwarding[{index}]");
        let from_reference = match canonical_participant_reference(
            &forwarding.from.participant,
            &format!("{forwarding_path}/from"),
        ) {
            Ok(reference) => reference,
            Err(message) => {
                push_profile_reference_issue(
                    issues,
                    context.subject,
                    &format!("{forwarding_path}/from"),
                    message,
                );
                continue;
            }
        };
        let function_reference = match canonical_connectivity_function_reference(
            &forwarding.function,
            &format!("{forwarding_path}/function-ref"),
        ) {
            Ok(reference) => reference,
            Err(message) => {
                push_profile_reference_issue(
                    issues,
                    context.subject,
                    &format!("{forwarding_path}/function-ref"),
                    message,
                );
                continue;
            }
        };
        let to_reference = match canonical_participant_reference(
            &forwarding.to.participant,
            &format!("{forwarding_path}/to"),
        ) {
            Ok(reference) => reference,
            Err(message) => {
                push_profile_reference_issue(
                    issues,
                    context.subject,
                    &format!("{forwarding_path}/to"),
                    message,
                );
                continue;
            }
        };

        let resolver = context
            .graph
            .resolver(context.profile_instance.owner.clone());
        let from_participant = match resolver.participant(&from_reference) {
            Ok(target) => target,
            Err(error) => {
                push_profile_reference_issue(
                    issues,
                    context.subject,
                    &format!("{forwarding_path}/from"),
                    error.to_string(),
                );
                continue;
            }
        };
        let function = match resolver.function(&function_reference) {
            Ok(target) => target,
            Err(error) => {
                push_profile_reference_issue(
                    issues,
                    context.subject,
                    &format!("{forwarding_path}/function-ref"),
                    error.to_string(),
                );
                continue;
            }
        };
        let to_participant = match resolver.participant(&to_reference) {
            Ok(target) => target,
            Err(error) => {
                push_profile_reference_issue(
                    issues,
                    context.subject,
                    &format!("{forwarding_path}/to"),
                    error.to_string(),
                );
                continue;
            }
        };

        let Some(from_network) = stream_reference_matches_path(
            context.graph,
            &context.profile_instance.owner,
            &from_reference.network,
            &path.networks,
            context.subject,
            &format!("{forwarding_path}/from"),
            issues,
        ) else {
            continue;
        };
        let Some(to_network) = stream_reference_matches_path(
            context.graph,
            &context.profile_instance.owner,
            &to_reference.network,
            &path.networks,
            context.subject,
            &format!("{forwarding_path}/to"),
            issues,
        ) else {
            continue;
        };
        if from_network == to_network {
            issues.push(ConnectivityIssue::error(
                "E_CONN_PROFILE_SELF_FORWARDING",
                "stream forwarding must connect two distinct path networks",
                context.subject.clone(),
                vec![
                    from_participant.identity().clone(),
                    to_participant.identity().clone(),
                ],
            ));
            continue;
        }

        let triple = (
            from_participant.id().clone(),
            function.id().clone(),
            to_participant.id().clone(),
        );
        if !triples.insert(triple.clone()) {
            issues.push(ConnectivityIssue::error(
                "E_CONN_PROFILE_DUPLICATE_FORWARDING",
                "a stream path may declare each canonical forwarding triple only once",
                context.subject.clone(),
                vec![
                    from_participant.identity().clone(),
                    function.identity().clone(),
                    to_participant.identity().clone(),
                ],
            ));
            continue;
        }

        let Some(from_endpoint) = participant_endpoint(context.graph, from_participant.id()) else {
            issues.push(ConnectivityIssue::error(
                "E_CONN_PROFILE_FORWARDING_FROM_ENDPOINT",
                "a forwarding source participant requires one exact functional endpoint",
                context.subject.clone(),
                vec![from_participant.identity().clone()],
            ));
            continue;
        };
        let Some(to_endpoint) = participant_endpoint(context.graph, to_participant.id()) else {
            issues.push(ConnectivityIssue::error(
                "E_CONN_PROFILE_FORWARDING_TO_ENDPOINT",
                "a forwarding target participant requires one exact functional endpoint",
                context.subject.clone(),
                vec![to_participant.identity().clone()],
            ));
            continue;
        };
        if !function_accepts_endpoint(context.graph, function.id(), &from_endpoint) {
            issues.push(ConnectivityIssue::error(
                "E_CONN_PROFILE_FORWARDING_INPUT",
                "the forwarding source participant must reach the declared function through an input or bidirectional endpoint",
                context.subject.clone(),
                vec![from_participant.identity().clone(), function.identity().clone()],
            ));
            continue;
        }
        if !function_emits_to_endpoint(context.graph, function.id(), &to_endpoint) {
            issues.push(ConnectivityIssue::error(
                "E_CONN_PROFILE_FORWARDING_OUTPUT",
                "the declared function must reach the forwarding target participant through an output or bidirectional endpoint",
                context.subject.clone(),
                vec![function.identity().clone(), to_participant.identity().clone()],
            ));
            continue;
        }

        let identity = stream_forwarding_object_identity(
            document,
            profile_instance_id,
            &stream.name,
            &triple.0,
            &triple.1,
            &triple.2,
        );
        let node = ConnectivityNode::new(
            identity,
            ConnectivityNodeData::StreamForwarding {
                forwarding: Box::new(forwarding.clone()),
            },
        );
        let forwarding_id = node.id().clone();
        nodes.push(node);
        edges.push(ConnectivityGraphExtensionEdge::new(
            EdgeKind::StreamForwarding,
            context.stream_id.clone(),
            forwarding_id.clone(),
            context.stream_id.clone(),
            EdgeExactness::Exact,
            forwarding_id.as_str(),
        ));
        edges.push(ConnectivityGraphExtensionEdge::new(
            EdgeKind::StreamForwardingFrom,
            from_participant.id().clone(),
            forwarding_id.clone(),
            forwarding_id.clone(),
            EdgeExactness::Exact,
            "from",
        ));
        edges.push(ConnectivityGraphExtensionEdge::new(
            EdgeKind::StreamForwardingFunction,
            forwarding_id.clone(),
            function.id().clone(),
            forwarding_id.clone(),
            EdgeExactness::Exact,
            "function",
        ));
        edges.push(ConnectivityGraphExtensionEdge::new(
            EdgeKind::StreamForwardingTo,
            forwarding_id.clone(),
            to_participant.id().clone(),
            forwarding_id,
            EdgeExactness::Exact,
            "to",
        ));
        adjacency
            .entry(from_network)
            .or_default()
            .insert(to_network);
    }

    validate_stream_route_graph(
        context.graph,
        context.subject,
        &path.networks,
        &adjacency,
        endpoints.talker_network.as_ref(),
        &endpoints.listener_networks,
        issues,
    );
}

fn participant_endpoint(
    graph: &NormalizedConnectivityGraph,
    participant: &StableObjectId,
) -> Option<StableObjectId> {
    graph
        .edges()
        .iter()
        .find(|edge| {
            edge.kind() == EdgeKind::ParticipantEndpoint
                && edge.to() == participant
                && edge.exactness() == EdgeExactness::Exact
        })
        .map(|edge| edge.from().clone())
}

fn function_accepts_endpoint(
    graph: &NormalizedConnectivityGraph,
    function: &StableObjectId,
    endpoint: &StableObjectId,
) -> bool {
    graph.edges().iter().any(|edge| match edge.kind() {
        EdgeKind::FunctionInput => edge.from() == endpoint && edge.to() == function,
        EdgeKind::FunctionBidirectional => edge.from() == function && edge.to() == endpoint,
        _ => false,
    })
}

fn function_emits_to_endpoint(
    graph: &NormalizedConnectivityGraph,
    function: &StableObjectId,
    endpoint: &StableObjectId,
) -> bool {
    graph.edges().iter().any(|edge| match edge.kind() {
        EdgeKind::FunctionOutput | EdgeKind::FunctionBidirectional => {
            edge.from() == function && edge.to() == endpoint
        }
        _ => false,
    })
}

fn stream_forwarding_object_identity(
    document: &DocumentIdentity,
    instance: &StreamProfileInstanceId,
    stream: &str,
    from: &StableObjectId,
    function: &StableObjectId,
    to: &StableObjectId,
) -> ObjectIdentity {
    let mut local = stream_profile_identity_parts(instance);
    local.push(IdentityPart::new("stream", stream));
    local.push(IdentityPart::new("from-participant", from.as_str()));
    local.push(IdentityPart::new("function", function.as_str()));
    local.push(IdentityPart::new("to-participant", to.as_str()));
    ObjectIdentity::new(
        document.clone(),
        instance.owner.clone(),
        ObjectKind::StreamForwarding,
        local,
    )
}

fn validate_stream_route_graph(
    graph: &NormalizedConnectivityGraph,
    subject: &ObjectIdentity,
    networks: &BTreeSet<StableObjectId>,
    adjacency: &BTreeMap<StableObjectId, BTreeSet<StableObjectId>>,
    talker_network: Option<&StableObjectId>,
    listener_networks: &BTreeSet<StableObjectId>,
    issues: &mut Vec<ConnectivityIssue>,
) {
    let mut indegree = networks
        .iter()
        .cloned()
        .map(|network| (network, 0usize))
        .collect::<BTreeMap<_, _>>();
    for targets in adjacency.values() {
        for target in targets {
            if let Some(count) = indegree.get_mut(target) {
                *count += 1;
            }
        }
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(network, degree)| (*degree == 0).then_some(network.clone()))
        .collect::<BTreeSet<_>>();
    let mut visited = 0usize;
    while let Some(network) = ready.pop_first() {
        visited += 1;
        if let Some(targets) = adjacency.get(&network) {
            for target in targets {
                let Some(degree) = indegree.get_mut(target) else {
                    continue;
                };
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(target.clone());
                }
            }
        }
    }
    if visited != networks.len() {
        let related = indegree
            .iter()
            .filter(|(_, degree)| **degree > 0)
            .filter_map(|(network, _)| graph.node(network))
            .map(|node| node.identity().clone())
            .collect();
        issues.push(ConnectivityIssue::error(
            "E_CONN_PROFILE_FORWARDING_CYCLE",
            "a stream forwarding graph must be acyclic",
            subject.clone(),
            related,
        ));
    }

    let (Some(talker_network), false) = (talker_network, listener_networks.is_empty()) else {
        return;
    };
    let forward = reachable_networks(talker_network, adjacency);
    let mut reverse = BTreeMap::<StableObjectId, BTreeSet<StableObjectId>>::new();
    for (from, targets) in adjacency {
        for to in targets {
            reverse.entry(to.clone()).or_default().insert(from.clone());
        }
    }
    let mut toward_listener = BTreeSet::new();
    for listener in listener_networks {
        toward_listener.extend(reachable_networks(listener, &reverse));
    }
    for network in networks {
        if forward.contains(network) && toward_listener.contains(network) {
            continue;
        }
        let related = graph
            .node(network)
            .map(|node| vec![node.identity().clone()])
            .unwrap_or_default();
        issues.push(ConnectivityIssue::error(
            "E_CONN_PROFILE_ROUTE_COVERAGE",
            "every listed path network must lie on a directed route from the talker network to at least one listener network",
            subject.clone(),
            related,
        ));
    }
}

fn reachable_networks(
    root: &StableObjectId,
    adjacency: &BTreeMap<StableObjectId, BTreeSet<StableObjectId>>,
) -> BTreeSet<StableObjectId> {
    let mut reachable = BTreeSet::from([root.clone()]);
    let mut pending = vec![root.clone()];
    while let Some(network) = pending.pop() {
        if let Some(targets) = adjacency.get(&network) {
            for target in targets {
                if reachable.insert(target.clone()) {
                    pending.push(target.clone());
                }
            }
        }
    }
    reachable
}

fn connect_stream_traffic_class(
    context: &StreamConnectionContext<'_>,
    reference: Option<&XmlTrafficClassRef>,
    edges: &mut Vec<ConnectivityGraphExtensionEdge>,
    issues: &mut Vec<ConnectivityIssue>,
) {
    let Some(reference) = reference else {
        return;
    };
    let canonical = match canonical_traffic_class_reference(reference, "traffic-class-ref") {
        Ok(reference) => reference,
        Err(message) => {
            push_profile_reference_issue(issues, context.subject, "traffic-class-ref", message);
            return;
        }
    };
    match context
        .graph
        .resolver(context.profile_instance.owner.clone())
        .traffic_class(&canonical)
    {
        Ok(target) => {
            if stream_reference_matches_path(
                context.graph,
                &context.profile_instance.owner,
                &canonical.network,
                context.path_networks,
                context.subject,
                "traffic-class-ref",
                issues,
            )
            .is_none()
            {
                return;
            }
            edges.push(ConnectivityGraphExtensionEdge::new(
                EdgeKind::StreamTrafficClass,
                context.stream_id.clone(),
                target.id().clone(),
                context.stream_id.clone(),
                EdgeExactness::Exact,
                "traffic-class",
            ));
        }
        Err(error) => push_profile_reference_issue(
            issues,
            context.subject,
            "traffic-class-ref",
            error.to_string(),
        ),
    }
}

fn connect_stream_schedule(
    context: &StreamConnectionContext<'_>,
    reference: Option<&XmlScheduleRef>,
    edges: &mut Vec<ConnectivityGraphExtensionEdge>,
    issues: &mut Vec<ConnectivityIssue>,
) {
    let Some(reference) = reference else {
        return;
    };
    let canonical = match canonical_schedule_reference(reference, "schedule-ref") {
        Ok(reference) => reference,
        Err(message) => {
            push_profile_reference_issue(issues, context.subject, "schedule-ref", message);
            return;
        }
    };
    match context
        .graph
        .resolver(context.profile_instance.owner.clone())
        .gate_schedule(&canonical)
    {
        Ok(target) => {
            if stream_reference_matches_path(
                context.graph,
                &context.profile_instance.owner,
                &canonical.network,
                context.path_networks,
                context.subject,
                "schedule-ref",
                issues,
            )
            .is_none()
            {
                return;
            }
            edges.push(ConnectivityGraphExtensionEdge::new(
                EdgeKind::StreamSchedule,
                context.stream_id.clone(),
                target.id().clone(),
                context.stream_id.clone(),
                EdgeExactness::Exact,
                "schedule",
            ));
        }
        Err(error) => {
            push_profile_reference_issue(issues, context.subject, "schedule-ref", error.to_string())
        }
    }
}

fn stream_reference_matches_path(
    graph: &NormalizedConnectivityGraph,
    owner: &IncludeInstanceId,
    reference: &CanonicalNetworkRef,
    path_networks: &BTreeSet<StableObjectId>,
    subject: &ObjectIdentity,
    path: &str,
    issues: &mut Vec<ConnectivityIssue>,
) -> Option<StableObjectId> {
    let Ok(reference_network) = graph.resolver(owner.clone()).network(reference) else {
        return None;
    };
    if path_networks.contains(reference_network.id()) {
        return Some(reference_network.id().clone());
    }
    let related = vec![reference_network.identity().clone()];
    issues.push(ConnectivityIssue::error(
        "E_CONN_PROFILE_STREAM_NETWORK",
        format!("{path} must target an object on a listed stream path network"),
        subject.clone(),
        related,
    ));
    None
}

fn canonical_network_reference(
    reference: &XmlNetworkRef,
    path: &str,
) -> Result<CanonicalNetworkRef, String> {
    if reference.network.trim().is_empty() {
        return Err(format!("{path} must name a network"));
    }
    Ok(CanonicalNetworkRef {
        scope: profile_reference_scope(reference.instance.as_ref(), path)?,
        network: reference.network.clone(),
    })
}

fn canonical_connectivity_function_reference(
    reference: &XmlConnectivityFunctionRef,
    path: &str,
) -> Result<CanonicalConnectivityFunctionRef, String> {
    if reference.component.trim().is_empty() {
        return Err(format!("{path} must name a component"));
    }
    if reference.function.trim().is_empty() {
        return Err(format!("{path} must name a connectivity function"));
    }
    Ok(CanonicalConnectivityFunctionRef {
        component: CanonicalComponentRef {
            scope: profile_reference_scope(reference.instance.as_ref(), path)?,
            component: reference.component.clone(),
        },
        function: reference.function.clone(),
    })
}

fn canonical_stream_group_reference(
    reference: &XmlStreamGroupRef,
    path: &str,
) -> Result<CanonicalStreamGroupRef, String> {
    if reference.group.trim().is_empty() {
        return Err(format!("{path} must name a stream group"));
    }
    Ok(CanonicalStreamGroupRef {
        scope: profile_reference_scope(reference.instance.as_ref(), path)?,
        group: reference.group.clone(),
    })
}

fn canonical_participant_reference(
    reference: &XmlParticipantRef,
    path: &str,
) -> Result<CanonicalParticipantRef, String> {
    if reference.participant.trim().is_empty() {
        return Err(format!("{path} must name a participant"));
    }
    Ok(CanonicalParticipantRef {
        network: canonical_network_reference(
            &XmlNetworkRef {
                network: reference.network.clone(),
                instance: reference.instance.clone(),
            },
            path,
        )?,
        participant: reference.participant.clone(),
    })
}

fn canonical_traffic_class_reference(
    reference: &XmlTrafficClassRef,
    path: &str,
) -> Result<CanonicalTrafficClassRef, String> {
    if reference.traffic_class.trim().is_empty() {
        return Err(format!("{path} must name a traffic class"));
    }
    Ok(CanonicalTrafficClassRef {
        network: canonical_network_reference(
            &XmlNetworkRef {
                network: reference.network.clone(),
                instance: reference.instance.clone(),
            },
            path,
        )?,
        traffic_class: reference.traffic_class.clone(),
    })
}

fn canonical_schedule_reference(
    reference: &XmlScheduleRef,
    path: &str,
) -> Result<CanonicalScheduleRef, String> {
    if reference.schedule.trim().is_empty() {
        return Err(format!("{path} must name a schedule"));
    }
    Ok(CanonicalScheduleRef {
        network: canonical_network_reference(
            &XmlNetworkRef {
                network: reference.network.clone(),
                instance: reference.instance.clone(),
            },
            path,
        )?,
        schedule: reference.schedule.clone(),
    })
}

fn profile_reference_scope(
    reference: Option<&InstanceRef>,
    path: &str,
) -> Result<ReferenceScope, String> {
    let Some(reference) = reference else {
        return Ok(ReferenceScope::Local);
    };
    if reference.segment.is_empty() {
        return Err(format!(
            "{path} has an explicit instance reference with no segments"
        ));
    }
    let mut relative = IncludeInstanceId::root();
    for segment in &reference.segment {
        relative = match segment.name.as_deref() {
            None => relative.unnamed_child(segment.occurrence),
            Some("") => {
                return Err(format!("{path} has an instance segment with an empty name"));
            }
            Some(name) => relative.named_child(name, segment.occurrence),
        };
    }
    Ok(ReferenceScope::Instance(relative))
}

fn push_profile_reference_issue(
    issues: &mut Vec<ConnectivityIssue>,
    subject: &ObjectIdentity,
    path: &str,
    message: impl std::fmt::Display,
) {
    issues.push(ConnectivityIssue::error(
        "E_CONN_UNRESOLVED_REFERENCE",
        format!("{path}: {message}"),
        subject.clone(),
        Vec::new(),
    ));
}

fn sort_connectivity_issues(issues: &mut [ConnectivityIssue]) {
    issues.sort_by(|first, second| {
        first
            .subject_id()
            .cmp(&second.subject_id())
            .then_with(|| first.code().cmp(second.code()))
            .then_with(|| first.message().cmp(second.message()))
    });
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DocumentResolutionCacheKey {
    parent: DocumentResourceKey,
    authored_uri: String,
    dependency_kind: DocumentDependencyKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CachedDocumentResolution {
    Resolved { key: DocumentResourceKey },
    Failed { failure: ResolverFailure },
}

#[derive(Debug, Clone)]
struct PendingStreamProfileDependency {
    id: StreamProfileInstanceId,
    declaring_source: DocumentResourceKey,
    owner: IncludeInstanceId,
    parent_profile: Option<StreamProfileInstanceId>,
    dependency_kind: DocumentDependencyKind,
    dependency_index: usize,
    authored_uri: String,
    authored_sha: Option<String>,
    required: bool,
    selection_role: Option<StreamProfileSelectionRole>,
    stack: Vec<DocumentResourceKey>,
    depth: usize,
}

struct TreeLoader<'a, R> {
    resolver: &'a mut R,
    options: DocumentSetOptions,
    sources: BTreeMap<DocumentResourceKey, LoadedDocumentSource>,
    instances: BTreeMap<IncludeInstanceId, DocumentInstance>,
    stream_profile_instances: BTreeMap<StreamProfileInstanceId, LoadedStreamProfileInstance>,
    stream_profile_sites: Vec<StreamProfileDependencySite>,
    resolution_cache: BTreeMap<DocumentResolutionCacheKey, CachedDocumentResolution>,
    aggregate_unique_bytes: usize,
    work: DocumentSetWork,
}

fn validate_include_siblings_for_load(
    instance: &IncludeInstanceId,
    includes: &[crate::model::Include],
) -> Result<(), DocumentSetError> {
    let mut named = BTreeMap::<&str, usize>::new();
    for (include_index, include) in includes.iter().enumerate() {
        if include
            .uri
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(DocumentSetError::MissingIncludeUri {
                instance: instance.clone(),
                include_index,
            });
        }
        let Some(name) = include.name.as_deref().filter(|name| !name.is_empty()) else {
            continue;
        };
        if let Some(first_include_index) = named.insert(name, include_index) {
            return Err(DocumentSetError::DuplicateIncludeName {
                instance: instance.clone(),
                first_include_index,
                duplicate_include_index: include_index,
                name: name.to_owned(),
            });
        }
    }
    Ok(())
}

impl<R: DocumentResolver> TreeLoader<'_, R> {
    fn descend(
        &mut self,
        instance_id: IncludeInstanceId,
        stack: Vec<DocumentResourceKey>,
        depth: usize,
    ) -> Result<(), DocumentSetError> {
        let source_key = self.instances[&instance_id].source.clone();
        let document = self.sources[&source_key]
            .content
            .hcdf()
            .expect("every instantiated source is parsed");
        let profiles = document.stream_profile.clone();
        let includes = document.include.clone();
        validate_include_siblings_for_load(&instance_id, &includes)?;
        for (declaration_index, profile) in profiles.into_iter().enumerate() {
            let profile_depth =
                depth
                    .checked_add(1)
                    .ok_or_else(|| DocumentSetError::LimitExceeded {
                        kind: DocumentSetLimitKind::Depth,
                        limit: self.options.limits.max_depth,
                        actual: usize::MAX,
                        resource: Some(profile.uri.clone()),
                    })?;
            self.load_stream_profile_dependency(PendingStreamProfileDependency {
                id: StreamProfileInstanceId::direct(instance_id.clone(), declaration_index),
                declaring_source: source_key.clone(),
                owner: instance_id.clone(),
                parent_profile: None,
                dependency_kind: DocumentDependencyKind::StreamProfile,
                dependency_index: declaration_index,
                authored_uri: profile.uri,
                authored_sha: profile.sha,
                required: profile.required,
                selection_role: profile.selection_role,
                stack: Vec::new(),
                depth: profile_depth,
            })?;
        }
        let mut occurrences = BTreeMap::<IncludeSegmentName, u32>::new();

        for (include_index, include) in includes.into_iter().enumerate() {
            let uri = include
                .uri
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| DocumentSetError::MissingIncludeUri {
                    instance: instance_id.clone(),
                    include_index,
                })?
                .to_owned();
            self.work.dependency_sites = checked_limit_add(
                DocumentSetLimitKind::DependencySites,
                self.work.dependency_sites,
                1,
                self.options.limits.max_dependency_sites,
                Some(uri.clone()),
            )?;
            let authored_name = include.name.clone();
            let segment = match include.name.as_deref() {
                None | Some("") => IncludeSegmentName::Unnamed,
                Some(name) => IncludeSegmentName::Named(name.to_owned()),
            };
            let occurrence = occurrences.entry(segment.clone()).or_insert(0);
            let child_id = match segment {
                IncludeSegmentName::Unnamed => instance_id.unnamed_child(*occurrence),
                IncludeSegmentName::Named(name) => instance_id.named_child(name, *occurrence),
            };
            *occurrence =
                occurrence
                    .checked_add(1)
                    .ok_or_else(|| DocumentSetError::LimitExceeded {
                        kind: DocumentSetLimitKind::DependencySites,
                        limit: self.options.limits.max_dependency_sites,
                        actual: usize::MAX,
                        resource: Some(uri.clone()),
                    })?;

            let pose_projection = project_include_pose(include.pose.as_deref());

            let child_depth =
                depth
                    .checked_add(1)
                    .ok_or_else(|| DocumentSetError::LimitExceeded {
                        kind: DocumentSetLimitKind::Depth,
                        limit: self.options.limits.max_depth,
                        actual: usize::MAX,
                        resource: Some(uri.clone()),
                    })?;
            let resolution = self.resolve_document_cached(
                &source_key,
                &uri,
                DocumentDependencyKind::HcdfInclude,
            )?;
            let mut diagnostics = Vec::new();
            let child_source_key = match resolution {
                CachedDocumentResolution::Resolved { key } => key,
                CachedDocumentResolution::Failed { failure }
                    if self.options.document_policy == DocumentLoadPolicy::EditorCompatible
                        && is_editor_retainable_failure(failure.kind) =>
                {
                    diagnostics.push(DocumentDependencyDiagnostic::ResolutionFailure { failure });
                    self.record_include_site(
                        &instance_id,
                        include_index,
                        &include,
                        uri,
                        DocumentIncludeSiteOutcome::RetainedUnresolved,
                        diagnostics,
                    );
                    continue;
                }
                CachedDocumentResolution::Failed { failure } => {
                    return Err(DocumentSetError::DocumentResolution {
                        instance: instance_id.clone(),
                        include_index,
                        uri,
                        failure,
                    });
                }
            };

            let actual = self.sources[&child_source_key].source_sha.clone();
            if let Some(expected) = include.sha.as_deref().filter(|value| !value.is_empty()) {
                if expected != actual {
                    if self.options.document_policy == DocumentLoadPolicy::Strict {
                        return Err(DocumentSetError::DocumentDigestMismatch {
                            instance: instance_id.clone(),
                            include_index,
                            uri,
                            expected: expected.to_owned(),
                            actual,
                        });
                    }
                    diagnostics.push(DocumentDependencyDiagnostic::DigestMismatch {
                        expected: expected.to_owned(),
                        actual,
                    });
                }
            }
            if stack.contains(&child_source_key) {
                let mut chain = stack.clone();
                chain.push(child_source_key);
                return Err(DocumentSetError::IncludeCycle { chain });
            }

            if let Some(actual_kind) = self.sources[&child_source_key].content.kind() {
                if actual_kind != DocumentContentKind::Hcdf {
                    if self.options.document_policy == DocumentLoadPolicy::Strict {
                        return Err(DocumentSetError::DocumentContentKindMismatch {
                            key: child_source_key,
                            expected: DocumentContentKind::Hcdf,
                            actual: actual_kind,
                        });
                    }
                    diagnostics.push(DocumentDependencyDiagnostic::ContentKindMismatch {
                        key: child_source_key.clone(),
                        expected: DocumentContentKind::Hcdf,
                        actual: actual_kind,
                    });
                    self.record_include_site(
                        &instance_id,
                        include_index,
                        &include,
                        uri,
                        DocumentIncludeSiteOutcome::RetainedUnresolved,
                        diagnostics,
                    );
                    continue;
                }
            }

            if let Some(failure) = self.sources[&child_source_key].content.failure().cloned() {
                if self.options.document_policy == DocumentLoadPolicy::Strict {
                    return Err(document_content_error(child_source_key, failure));
                }
                let detected_kind = self.sources[&child_source_key].content.kind();
                diagnostics.push(DocumentDependencyDiagnostic::ContentFailure {
                    key: child_source_key,
                    detected_kind,
                    failure,
                });
                self.record_include_site(
                    &instance_id,
                    include_index,
                    &include,
                    uri,
                    DocumentIncludeSiteOutcome::RetainedUnresolved,
                    diagnostics,
                );
                continue;
            }

            check_limit(
                DocumentSetLimitKind::Depth,
                self.options.limits.max_depth,
                child_depth,
                Some(uri.clone()),
            )?;
            self.check_instance_work(&child_id, &child_source_key)?;
            let parent_placement = self.instances[&instance_id].placement.clone();
            let placement = placement_state(
                &parent_placement,
                &child_id,
                include.pose.clone(),
                include.placement_frame.clone(),
                pose_projection,
            );
            let adjustments = component_origin_adjustments(
                self.sources[&child_source_key]
                    .content
                    .hcdf()
                    .expect("a resolved instance source is parsed"),
                include.pose.clone(),
                &placement,
            );
            self.instances.insert(
                child_id.clone(),
                DocumentInstance {
                    id: child_id.clone(),
                    source: child_source_key.clone(),
                    parent: Some(instance_id.clone()),
                    include_site_index: Some(include_index),
                    authored_name,
                    authored_uri: Some(uri.clone()),
                    placement,
                    component_origin_adjustments: adjustments,
                    include_sites: Vec::new(),
                    children: Vec::new(),
                },
            );
            let parent = self
                .instances
                .get_mut(&instance_id)
                .expect("parent instance exists");
            parent.include_sites.push(DocumentIncludeSite {
                include_index,
                dependency_kind: DocumentDependencyKind::HcdfInclude,
                authored_uri: uri,
                authored_sha: include.sha.clone(),
                outcome: DocumentIncludeSiteOutcome::Resolved {
                    child: child_id.clone(),
                },
                diagnostics,
            });
            parent.children.push(child_id.clone());
            let mut child_stack = stack.clone();
            child_stack.push(child_source_key);
            self.descend(child_id, child_stack, child_depth)?;
        }
        Ok(())
    }

    fn load_stream_profile_dependency(
        &mut self,
        pending: PendingStreamProfileDependency,
    ) -> Result<(), DocumentSetError> {
        self.work.dependency_sites = checked_limit_add(
            DocumentSetLimitKind::DependencySites,
            self.work.dependency_sites,
            1,
            self.options.limits.max_dependency_sites,
            Some(pending.authored_uri.clone()),
        )?;
        if pending.authored_uri.trim().is_empty() {
            return self.finish_stream_profile_failure(
                pending,
                StreamProfileDependencyDiagnostic::ResolutionFailure {
                    failure: ResolverFailure::new(
                        ResolverFailureKind::InvalidReference,
                        "stream-profile dependency URI must not be empty",
                    ),
                },
            );
        }

        let resolution = self.resolve_document_cached(
            &pending.declaring_source,
            &pending.authored_uri,
            pending.dependency_kind,
        )?;
        let source_key = match resolution {
            CachedDocumentResolution::Resolved { key } => key,
            CachedDocumentResolution::Failed { failure } => {
                return self.finish_stream_profile_failure(
                    pending,
                    StreamProfileDependencyDiagnostic::ResolutionFailure { failure },
                );
            }
        };

        let actual_sha = self.sources[&source_key].source_sha.clone();
        if let Some(expected) = pending
            .authored_sha
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
        {
            if expected != actual_sha {
                return self.finish_stream_profile_failure(
                    pending,
                    StreamProfileDependencyDiagnostic::DigestMismatch {
                        expected,
                        actual: actual_sha,
                    },
                );
            }
        }

        if let Some(actual_kind) = self.sources[&source_key].content.kind() {
            if actual_kind != DocumentContentKind::StreamProfile {
                return self.finish_stream_profile_failure(
                    pending,
                    StreamProfileDependencyDiagnostic::ContentKindMismatch {
                        key: source_key,
                        expected: DocumentContentKind::StreamProfile,
                        actual: actual_kind,
                    },
                );
            }
        }
        if let Some(failure) = self.sources[&source_key].content.failure().cloned() {
            let detected_kind = self.sources[&source_key].content.kind();
            return self.finish_stream_profile_failure(
                pending,
                StreamProfileDependencyDiagnostic::ContentFailure {
                    key: source_key,
                    detected_kind,
                    failure,
                },
            );
        }
        if pending.stack.contains(&source_key) {
            let mut chain = pending.stack.clone();
            chain.push(source_key);
            return self.finish_stream_profile_failure(
                pending,
                StreamProfileDependencyDiagnostic::Cycle { chain },
            );
        }

        check_limit(
            DocumentSetLimitKind::Depth,
            self.options.limits.max_depth,
            pending.depth,
            Some(pending.authored_uri.clone()),
        )?;
        self.check_stream_profile_instance_work(&pending.id, &source_key)?;
        let dependencies = self.sources[&source_key]
            .content
            .stream_profile()
            .expect("a resolved profile instance source is parsed")
            .dependency
            .clone();
        let instance = LoadedStreamProfileInstance {
            id: pending.id.clone(),
            source: source_key.clone(),
            owner: pending.owner.clone(),
            parent: pending.parent_profile.clone(),
            required: pending.required,
            selection_role: pending.selection_role,
            children: Vec::new(),
        };
        self.stream_profile_instances
            .insert(pending.id.clone(), instance);
        if let Some(parent_id) = &pending.parent_profile {
            self.stream_profile_instances
                .get_mut(parent_id)
                .expect("a nested profile dependency has a loaded parent")
                .children
                .push(pending.id.clone());
        }
        self.stream_profile_sites.push(StreamProfileDependencySite {
            id: pending.id.clone(),
            declaring_source: pending.declaring_source,
            owner: pending.owner.clone(),
            parent_profile: pending.parent_profile,
            dependency_kind: pending.dependency_kind,
            dependency_index: pending.dependency_index,
            authored_uri: pending.authored_uri,
            authored_sha: pending.authored_sha,
            required: pending.required,
            selection_role: pending.selection_role,
            outcome: StreamProfileSiteOutcome::Resolved {
                profile: pending.id.clone(),
            },
            diagnostics: Vec::new(),
        });

        let mut stack = pending.stack;
        stack.push(source_key.clone());
        for (dependency_index, dependency) in dependencies.into_iter().enumerate() {
            let child_depth =
                pending
                    .depth
                    .checked_add(1)
                    .ok_or_else(|| DocumentSetError::LimitExceeded {
                        kind: DocumentSetLimitKind::Depth,
                        limit: self.options.limits.max_depth,
                        actual: usize::MAX,
                        resource: Some(dependency.uri.clone()),
                    })?;
            self.load_stream_profile_dependency(PendingStreamProfileDependency {
                id: pending.id.dependency(dependency_index),
                declaring_source: source_key.clone(),
                owner: pending.owner.clone(),
                parent_profile: Some(pending.id.clone()),
                dependency_kind: DocumentDependencyKind::StreamProfileDependency,
                dependency_index,
                authored_uri: dependency.uri,
                authored_sha: dependency.sha,
                required: dependency.required,
                selection_role: None,
                stack: stack.clone(),
                depth: child_depth,
            })?;
        }
        Ok(())
    }

    fn finish_stream_profile_failure(
        &mut self,
        pending: PendingStreamProfileDependency,
        diagnostic: StreamProfileDependencyDiagnostic,
    ) -> Result<(), DocumentSetError> {
        let unconditionally_fatal = matches!(
            &diagnostic,
            StreamProfileDependencyDiagnostic::ResolutionFailure {
                failure: ResolverFailure {
                    kind: ResolverFailureKind::Denied | ResolverFailureKind::InvalidReference,
                    ..
                }
            }
        );
        if self.options.document_policy == DocumentLoadPolicy::Strict || unconditionally_fatal {
            return Err(DocumentSetError::StreamProfileDependency {
                id: Box::new(pending.id),
                uri: pending.authored_uri,
                required: pending.required,
                diagnostic: Box::new(diagnostic),
            });
        }
        self.stream_profile_sites.push(StreamProfileDependencySite {
            id: pending.id,
            declaring_source: pending.declaring_source,
            owner: pending.owner,
            parent_profile: pending.parent_profile,
            dependency_kind: pending.dependency_kind,
            dependency_index: pending.dependency_index,
            authored_uri: pending.authored_uri,
            authored_sha: pending.authored_sha,
            required: pending.required,
            selection_role: pending.selection_role,
            outcome: StreamProfileSiteOutcome::RetainedUnresolved,
            diagnostics: vec![diagnostic],
        });
        Ok(())
    }

    fn record_include_site(
        &mut self,
        instance_id: &IncludeInstanceId,
        include_index: usize,
        include: &crate::model::Include,
        authored_uri: String,
        outcome: DocumentIncludeSiteOutcome,
        diagnostics: Vec<DocumentDependencyDiagnostic>,
    ) {
        self.instances
            .get_mut(instance_id)
            .expect("parent instance exists")
            .include_sites
            .push(DocumentIncludeSite {
                include_index,
                dependency_kind: DocumentDependencyKind::HcdfInclude,
                authored_uri,
                authored_sha: include.sha.clone(),
                outcome,
                diagnostics,
            });
    }

    fn resolve_document_cached(
        &mut self,
        parent: &DocumentResourceKey,
        authored_uri: &str,
        dependency_kind: DocumentDependencyKind,
    ) -> Result<CachedDocumentResolution, DocumentSetError> {
        let cache_key = DocumentResolutionCacheKey {
            parent: parent.clone(),
            authored_uri: authored_uri.to_owned(),
            dependency_kind,
        };
        if let Some(cached) = self.resolution_cache.get(&cache_key) {
            return Ok(cached.clone());
        }

        self.work.resolver_calls = checked_limit_add(
            DocumentSetLimitKind::ResolverCalls,
            self.work.resolver_calls,
            1,
            self.options.limits.max_resolver_calls,
            Some(authored_uri.to_owned()),
        )?;
        let resolution = match self
            .resolver
            .resolve_document(parent, authored_uri, dependency_kind)
        {
            Ok(resolved) => {
                self.work.resolver_bytes = checked_limit_add(
                    DocumentSetLimitKind::ResolverBytes,
                    self.work.resolver_bytes,
                    resolved.bytes.len(),
                    self.options.limits.max_resolver_bytes,
                    Some(resolved.key.as_str().to_owned()),
                )?;
                check_document_size(&resolved.key, resolved.bytes.len(), &self.options.limits)?;
                let key = self.insert_source(resolved)?;
                CachedDocumentResolution::Resolved { key }
            }
            Err(failure) => CachedDocumentResolution::Failed { failure },
        };
        self.resolution_cache.insert(cache_key, resolution.clone());
        Ok(resolution)
    }

    fn insert_source(
        &mut self,
        resolved: ResolvedDocumentResource,
    ) -> Result<DocumentResourceKey, DocumentSetError> {
        if let Some(existing) = self.sources.get(&resolved.key) {
            if existing.bytes != resolved.bytes {
                return Err(DocumentSetError::SourceConflict { key: resolved.key });
            }
            return Ok(existing.key.clone());
        }
        checked_limit_add(
            DocumentSetLimitKind::UniqueDocuments,
            self.sources.len(),
            1,
            self.options.limits.max_unique_documents,
            Some(resolved.key.as_str().to_owned()),
        )?;
        let aggregate = checked_limit_add(
            DocumentSetLimitKind::AggregateUniqueBytes,
            self.aggregate_unique_bytes,
            resolved.bytes.len(),
            self.options.limits.max_aggregate_unique_bytes,
            Some(resolved.key.as_str().to_owned()),
        )?;
        let (content, xml_element_count) = parse_document_source_content(&resolved.bytes);
        let key = resolved.key.clone();
        let source = LoadedDocumentSource {
            key: resolved.key.clone(),
            content,
            source_sha: content_sha(&resolved.bytes),
            bytes: resolved.bytes,
            provenance: DocumentSourceProvenance::ResolvedDependencyBytes,
            xml_element_count,
        };
        self.aggregate_unique_bytes = aggregate;
        self.sources.insert(resolved.key, source);
        Ok(key)
    }

    fn check_instance_work(
        &mut self,
        instance_id: &IncludeInstanceId,
        source_key: &DocumentResourceKey,
    ) -> Result<(), DocumentSetError> {
        checked_limit_add(
            DocumentSetLimitKind::Instances,
            self.instances.len() + self.stream_profile_instances.len(),
            1,
            self.options.limits.max_instances,
            Some(instance_id.display_path()),
        )?;
        let source = &self.sources[source_key];
        let expanded_source_bytes = checked_limit_add(
            DocumentSetLimitKind::ExpandedSourceBytes,
            self.work.expanded_source_bytes,
            source.bytes.len(),
            self.options.limits.max_expanded_source_bytes,
            Some(instance_id.display_path()),
        )?;
        let expanded_xml_elements = checked_limit_add(
            DocumentSetLimitKind::ExpandedXmlElements,
            self.work.expanded_xml_elements,
            source.xml_element_count,
            self.options.limits.max_expanded_xml_elements,
            Some(instance_id.display_path()),
        )?;
        self.work.expanded_source_bytes = expanded_source_bytes;
        self.work.expanded_xml_elements = expanded_xml_elements;
        Ok(())
    }

    fn check_stream_profile_instance_work(
        &mut self,
        instance_id: &StreamProfileInstanceId,
        source_key: &DocumentResourceKey,
    ) -> Result<(), DocumentSetError> {
        checked_limit_add(
            DocumentSetLimitKind::Instances,
            self.instances.len() + self.stream_profile_instances.len(),
            1,
            self.options.limits.max_instances,
            Some(instance_id.display_path()),
        )?;
        let source = &self.sources[source_key];
        self.work.expanded_source_bytes = checked_limit_add(
            DocumentSetLimitKind::ExpandedSourceBytes,
            self.work.expanded_source_bytes,
            source.bytes.len(),
            self.options.limits.max_expanded_source_bytes,
            Some(instance_id.display_path()),
        )?;
        self.work.expanded_xml_elements = checked_limit_add(
            DocumentSetLimitKind::ExpandedXmlElements,
            self.work.expanded_xml_elements,
            source.xml_element_count,
            self.options.limits.max_expanded_xml_elements,
            Some(instance_id.display_path()),
        )?;
        Ok(())
    }
}

fn is_editor_retainable_failure(kind: ResolverFailureKind) -> bool {
    matches!(
        kind,
        ResolverFailureKind::NotFound
            | ResolverFailureKind::Unsupported
            | ResolverFailureKind::Other
    )
}

fn document_content_error(
    key: DocumentResourceKey,
    failure: DocumentContentFailure,
) -> DocumentSetError {
    match failure {
        DocumentContentFailure::Encoding { message } => {
            DocumentSetError::DocumentEncoding { key, message }
        }
        DocumentContentFailure::Parse { message } => {
            DocumentSetError::DocumentParse { key, message }
        }
    }
}

fn parse_document_source_content(bytes: &[u8]) -> (DocumentSourceContent, usize) {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            return (
                DocumentSourceContent::Unusable {
                    detected_kind: None,
                    failure: DocumentContentFailure::Encoding {
                        message: error.to_string(),
                    },
                },
                0,
            );
        }
    };
    let detected_kind = match detect_document_content_kind(text) {
        Ok(kind) => kind,
        Err(message) => {
            return (
                DocumentSourceContent::Unusable {
                    detected_kind: None,
                    failure: DocumentContentFailure::Parse { message },
                },
                0,
            );
        }
    };
    let xml_element_count = count_xml_elements(bytes);
    match detected_kind {
        DocumentContentKind::Hcdf => match Hcdf::from_xml_str(text) {
            Ok(document) => (
                DocumentSourceContent::Hcdf(Box::new(document)),
                xml_element_count,
            ),
            Err(error) => (
                DocumentSourceContent::Unusable {
                    detected_kind: Some(detected_kind),
                    failure: DocumentContentFailure::Parse {
                        message: error.to_string(),
                    },
                },
                0,
            ),
        },
        DocumentContentKind::StreamProfile => match StreamProfileDocument::from_xml_str(text) {
            Ok(document) => (
                DocumentSourceContent::StreamProfile(Box::new(document)),
                xml_element_count,
            ),
            Err(error) => (
                DocumentSourceContent::Unusable {
                    detected_kind: Some(detected_kind),
                    failure: DocumentContentFailure::Parse {
                        message: error.to_string(),
                    },
                },
                0,
            ),
        },
    }
}

fn detect_document_content_kind(text: &str) -> Result<DocumentContentKind, String> {
    let mut reader = quick_xml::Reader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(element))
            | Ok(quick_xml::events::Event::Empty(element)) => {
                return match element.local_name().as_ref() {
                    b"hcdf" => Ok(DocumentContentKind::Hcdf),
                    b"stream-profile" => Ok(DocumentContentKind::StreamProfile),
                    name => Err(format!(
                        "unsupported document root <{}>",
                        String::from_utf8_lossy(name)
                    )),
                };
            }
            Ok(quick_xml::events::Event::Eof) => {
                return Err("document does not contain a root element".to_owned());
            }
            Err(error) => return Err(error.to_string()),
            _ => {}
        }
    }
}

fn build_flatten_prefix_map(
    instances: &BTreeMap<IncludeInstanceId, DocumentInstance>,
) -> FlattenPrefixMap {
    let root = IncludeInstanceId::root();
    let mut prefixes = BTreeMap::from([(root.clone(), String::new())]);
    let mut segments = BTreeMap::new();
    record_flatten_prefixes(&root, "", instances, &mut segments, &mut prefixes);
    FlattenPrefixMap { prefixes, segments }
}

fn join_flatten_name(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

fn record_flatten_prefixes(
    instance_id: &IncludeInstanceId,
    parent_prefix: &str,
    instances: &BTreeMap<IncludeInstanceId, DocumentInstance>,
    segments: &mut BTreeMap<IncludeInstanceId, Option<String>>,
    prefixes: &mut BTreeMap<IncludeInstanceId, String>,
) {
    for child_id in &instances[instance_id].children {
        let actual_segment = instances[child_id]
            .authored_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        let prefix = actual_segment.as_deref().map_or_else(
            || parent_prefix.to_owned(),
            |segment| join_flatten_name(parent_prefix, segment),
        );
        segments.insert(child_id.clone(), actual_segment);
        prefixes.insert(child_id.clone(), prefix.clone());
        record_flatten_prefixes(child_id, &prefix, instances, segments, prefixes);
    }
}

fn count_xml_elements(bytes: &[u8]) -> usize {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut count = 0;
    loop {
        match reader.read_event() {
            Ok(Event::Start(_) | Event::Empty(_)) => count += 1,
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    count
}

fn check_document_size(
    key: &DocumentResourceKey,
    actual: usize,
    limits: &DocumentSetLimits,
) -> Result<(), DocumentSetError> {
    check_limit(
        DocumentSetLimitKind::DocumentBytes,
        limits.max_document_bytes,
        actual,
        Some(key.as_str().to_owned()),
    )
}

fn checked_limit_add(
    kind: DocumentSetLimitKind,
    current: usize,
    increment: usize,
    limit: usize,
    resource: Option<String>,
) -> Result<usize, DocumentSetError> {
    let actual = current
        .checked_add(increment)
        .ok_or_else(|| DocumentSetError::LimitExceeded {
            kind,
            limit,
            actual: usize::MAX,
            resource: resource.clone(),
        })?;
    check_limit(kind, limit, actual, resource)?;
    Ok(actual)
}

fn check_limit(
    kind: DocumentSetLimitKind,
    limit: usize,
    actual: usize,
    resource: Option<String>,
) -> Result<(), DocumentSetError> {
    if actual > limit {
        Err(DocumentSetError::LimitExceeded {
            kind,
            limit,
            actual,
            resource,
        })
    } else {
        Ok(())
    }
}

fn identity_matrix() -> Mat4 {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

#[derive(Debug, Clone, PartialEq)]
enum IncludePoseProjection {
    Trusted(Mat4),
    Untrusted(UntrustedPoseReason),
}

fn placement_state(
    parent: &PlacementState,
    instance: &IncludeInstanceId,
    authored_pose: Option<String>,
    placement_frame: Option<String>,
    pose_projection: IncludePoseProjection,
) -> PlacementState {
    let mut causes = Vec::new();
    let local_to_parent = match pose_projection {
        IncludePoseProjection::Trusted(matrix) => Some(matrix),
        IncludePoseProjection::Untrusted(reason) => {
            causes.push(PlacementProjectionCause::UntrustedPose(reason));
            None
        }
    };
    if placement_frame
        .as_deref()
        .is_some_and(|frame| !frame.is_empty())
    {
        causes.push(PlacementProjectionCause::PlacementFrameReference);
    }

    let step = UnprojectablePlacementStep {
        instance: instance.clone(),
        authored_pose,
        placement_frame,
        causes,
    };
    match parent {
        PlacementState::Resolved(resolved) if step.causes.is_empty() => {
            let local_to_parent =
                local_to_parent.expect("a cause-free placement has a trusted pose");
            PlacementState::Resolved(Box::new(ResolvedPlacement {
                local_to_parent,
                local_to_root: mul44(&resolved.local_to_root, &local_to_parent),
            }))
        }
        PlacementState::Resolved(_) => PlacementState::Unprojectable {
            ancestry: vec![step],
        },
        PlacementState::Unprojectable { ancestry } => {
            let mut ancestry = ancestry.clone();
            ancestry.push(step);
            PlacementState::Unprojectable { ancestry }
        }
    }
}

fn project_include_pose(text: Option<&str>) -> IncludePoseProjection {
    let Some(text) = text else {
        return IncludePoseProjection::Trusted(identity_matrix());
    };
    if text.is_empty() {
        return IncludePoseProjection::Trusted(identity_matrix());
    }

    let tokens = text.split_whitespace().collect::<Vec<_>>();
    if !matches!(tokens.len(), 3 | 6) {
        return IncludePoseProjection::Untrusted(UntrustedPoseReason::WrongValueCount {
            actual: tokens.len(),
        });
    }
    let values = match tokens
        .iter()
        .enumerate()
        .map(|(token_index, token)| {
            token
                .parse::<f64>()
                .map_err(|_| UntrustedPoseReason::InvalidNumber {
                    token_index,
                    token: (*token).to_owned(),
                })
        })
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(values) => values,
        Err(reason) => return IncludePoseProjection::Untrusted(reason),
    };
    if let Some(token_index) = values.iter().position(|value| !value.is_finite()) {
        return IncludePoseProjection::Untrusted(UntrustedPoseReason::NonFiniteNumber {
            token_index,
            token: tokens[token_index].to_owned(),
        });
    }

    let mut xyz = [0.0; 3];
    xyz.copy_from_slice(&values[..3]);
    let mut rpy = [0.0; 3];
    if values.len() == 6 {
        rpy.copy_from_slice(&values[3..]);
    }
    IncludePoseProjection::Trusted(pose_to_matrix(&Pose {
        xyz: Some(xyz),
        rpy: Some(rpy),
        quat: None,
    }))
}

fn component_origin_adjustments(
    source: &Hcdf,
    authored_pose: Option<String>,
    placement: &PlacementState,
) -> Vec<ComponentOriginAdjustment> {
    if authored_pose.as_deref().is_none_or(str::is_empty) {
        return Vec::new();
    }
    let children = source
        .joint
        .iter()
        .filter(|joint| joint.loop_.is_none())
        .filter_map(|joint| joint.child.as_ref()?.comp.as_deref())
        .collect::<BTreeSet<_>>();
    source
        .comp
        .iter()
        .filter(|component| !children.contains(component.name.as_str()))
        .map(|component| ComponentOriginAdjustment {
            component: component.name.clone(),
            authored_pose: authored_pose.clone(),
            placement: placement.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::connectivity::Topology;
    use crate::model::Include;

    fn root_with(includes: Vec<Include>) -> Hcdf {
        Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            include: includes,
            ..Default::default()
        }
    }

    fn include(uri: &str, name: Option<&str>) -> Include {
        Include {
            uri: Some(uri.to_owned()),
            name: name.map(str::to_owned),
            ..Default::default()
        }
    }

    fn strict_options() -> DocumentSetOptions {
        DocumentSetOptions {
            document_policy: DocumentLoadPolicy::Strict,
            ..DocumentSetOptions::default()
        }
    }

    fn assert_limit_kind(error: DocumentSetError, expected: DocumentSetLimitKind) {
        assert!(
            matches!(
                error,
                DocumentSetError::LimitExceeded { kind, .. } if kind == expected
            ),
            "expected {expected:?} limit, got {error:?}"
        );
    }

    fn module(name: &str, includes: &[(&str, Option<&str>)]) -> Vec<u8> {
        let mut document = Hcdf {
            name: name.to_owned(),
            version: "1.0".to_owned(),
            ..Default::default()
        };
        document.include = includes
            .iter()
            .map(|(uri, name)| include(uri, *name))
            .collect();
        document.to_xml_string().unwrap().into_bytes()
    }

    fn profile_resource(uri: &str, required: bool) -> crate::model::StreamProfileResource {
        crate::model::StreamProfileResource {
            uri: uri.to_owned(),
            sha: None,
            required,
            selection_role: None,
        }
    }

    fn module_with_profile(name: &str, uri: &str, required: bool) -> Vec<u8> {
        Hcdf {
            name: name.to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource(uri, required)],
            ..Default::default()
        }
        .to_xml_string()
        .unwrap()
        .into_bytes()
    }

    fn stream_profile(name: &str, dependencies: &[(&str, bool)]) -> Vec<u8> {
        let dependencies = dependencies
            .iter()
            .map(|(uri, required)| format!(r#"<dependency uri="{uri}" required="{required}"/>"#))
            .collect::<String>();
        format!(r#"<stream-profile name="{name}" version="1.0">{dependencies}</stream-profile>"#)
            .into_bytes()
    }

    fn asset_document(name: &str) -> Hcdf {
        Hcdf::from_xml_str(&format!(
            r#"<hcdf name="{name}" version="1.0">
              <comp name="body">
                <visual name="shell"><model uri="assets/visual.glb" sha="sha256:visual"/></visual>
                <collision name="solid"><geometry><mesh uri="assets/collision.stl"/></geometry></collision>
                <sensor name="field"><em type="mag"><geometry><mesh uri="assets/sensor.stl"/></geometry></em></sensor>
                <hmi name="panel"><geometry><mesh uri="assets/hmi.stl"/></geometry></hmi>
                <connector name="J1"><representation><model uri="assets/connector.glb"><placement xyz="0 0 0"><frame><world/></frame><rotation><rpy value="0 0 0"/></rotation></placement></model></representation></connector>
              </comp>
            </hcdf>"#
        ))
        .unwrap()
    }

    fn single_asset_document(name: &str, uri: &str, sha: Option<&str>) -> Hcdf {
        let mut document = asset_document(name);
        document.comp[0].collision.clear();
        document.comp[0].sensor.clear();
        document.comp[0].hmi.clear();
        document.comp[0].connector.clear();
        let crate::model::VisualAppearance::Model { model, .. } =
            &mut document.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        model.uri = Some(uri.to_owned());
        model.sha = sha.map(str::to_owned);
        document
    }

    fn push_visual_asset(document: &mut Hcdf, name: &str, uri: &str, sha: Option<&str>) {
        let mut visual = document.comp[0].visual[0].clone();
        visual.name = name.to_owned();
        let crate::model::VisualAppearance::Model { model, .. } = &mut visual.appearance else {
            panic!("expected model-backed visual");
        };
        model.uri = Some(uri.to_owned());
        model.sha = sha.map(str::to_owned);
        document.comp[0].visual.push(visual);
    }

    fn component_asset_uris(component: &crate::model::Comp) -> [&str; 5] {
        let crate::model::VisualAppearance::Model { model, .. } = &component.visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        let crate::model::RepresentationChoice::Model(representation) = &component.connector[0]
            .representation
            .as_ref()
            .unwrap()
            .variant
        else {
            panic!("expected model-backed connector representation");
        };
        [
            model.uri.as_deref().unwrap(),
            component.collision[0]
                .geometry
                .as_ref()
                .unwrap()
                .mesh
                .as_ref()
                .unwrap()
                .uri
                .as_deref()
                .unwrap(),
            component.sensor[0].em[0]
                .geometry
                .as_ref()
                .unwrap()
                .mesh
                .as_ref()
                .unwrap()
                .uri
                .as_deref()
                .unwrap(),
            component.hmi[0]
                .geometry
                .as_ref()
                .unwrap()
                .mesh
                .as_ref()
                .unwrap()
                .uri
                .as_deref()
                .unwrap(),
            representation.uri.as_str(),
        ]
    }

    fn reserve_fixed_asset_guard_namespace(tree: &LoadedDocumentTree) -> String {
        let mut material = ProjectionGuardNamespaceMaterial::new(tree).unwrap();
        material.seed = "fixed".to_owned();
        material
            .reserve(
                tree,
                "hcdf-asset-guard:",
                "resolved asset URI namespace space exhausted",
            )
            .unwrap()
    }

    #[test]
    fn repeated_sources_produce_distinct_instances_and_one_source() {
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/module.hcdf", module("module", &[]))
            .unwrap();
        let tree = load_document_tree_from_model(
            root_with(vec![
                include("/mem/module.hcdf", Some("left")),
                include("/mem/module.hcdf", Some("right")),
            ]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        assert_eq!(tree.sources.len(), 2);
        assert_eq!(tree.instances.len(), 3);
        assert!(tree
            .instances
            .contains_key(&IncludeInstanceId::root().child("left", 0)));
        assert!(tree
            .instances
            .contains_key(&IncludeInstanceId::root().child("right", 0)));
    }

    #[test]
    fn repeated_hcdf_instances_expand_profile_instances_and_share_exact_resolution_cache() {
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/module.hcdf",
                module_with_profile("module", "profiles/main.xml", true),
            )
            .unwrap();
        resolver
            .insert_document("/mem/profiles/main.xml", stream_profile("motion", &[]))
            .unwrap();
        let tree = load_document_tree_from_model(
            root_with(vec![
                include("/mem/module.hcdf", Some("left")),
                include("/mem/module.hcdf", Some("right")),
            ]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let left = IncludeInstanceId::root().named_child("left", 0);
        let right = IncludeInstanceId::root().named_child("right", 0);
        assert_eq!(tree.stream_profile_instances().len(), 2);
        assert!(tree
            .stream_profile_instances()
            .contains_key(&StreamProfileInstanceId::direct(left, 0)));
        assert!(tree
            .stream_profile_instances()
            .contains_key(&StreamProfileInstanceId::direct(right, 0)));
        assert_eq!(tree.stream_profile_sites().len(), 2);
        assert_eq!(tree.work().resolver_calls, 2);
        let profile_key = DocumentResourceKey::new("/mem/profiles/main.xml").unwrap();
        assert_eq!(
            tree.sources()[&profile_key]
                .content
                .stream_profile()
                .unwrap()
                .name,
            "motion"
        );
    }

    #[test]
    fn nested_profile_dependencies_resolve_relative_to_the_profile_source() {
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/profiles/main.xml",
                stream_profile("main", &[("nested/child.xml", true)]),
            )
            .unwrap();
        resolver
            .insert_document(
                "/mem/profiles/nested/child.xml",
                stream_profile("child", &[]),
            )
            .unwrap();
        let root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource("profiles/main.xml", true)],
            ..Default::default()
        };
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let direct = StreamProfileInstanceId::direct(IncludeInstanceId::root(), 0);
        let nested = direct.dependency(0);
        assert_eq!(
            tree.stream_profile_instances()[&direct].children,
            vec![nested.clone()]
        );
        assert_eq!(
            tree.stream_profile_instances()[&nested].source.as_str(),
            "/mem/profiles/nested/child.xml"
        );
        assert_eq!(
            tree.stream_profile_sites()[1].declaring_source.as_str(),
            "/mem/profiles/main.xml"
        );
        assert_eq!(tree.work().dependency_sites, 2);
    }

    #[test]
    fn editor_preserves_structure_but_invalidates_connectivity_for_missing_required_profile() {
        let root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource("missing.xml", true)],
            ..Default::default()
        };
        let mut resolver = MemoryDocumentResolver::new();
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        assert_eq!(projected.flattened().name, "root");
        assert!(projected.stream_profile_instances().is_empty());
        assert!(matches!(
            projected.connectivity(),
            ConnectivityProjection::Invalid { issues }
                if matches!(issues.as_slice(), [ProjectedConnectivityIssue::StreamProfileDependency { diagnostic: StreamProfileDependencyDiagnostic::ResolutionFailure { .. }, .. }])
        ));
    }

    #[test]
    fn optional_missing_profile_is_nonblocking_in_editor_and_strict_is_fatal() {
        let root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource("missing.xml", false)],
            ..Default::default()
        };
        let mut resolver = MemoryDocumentResolver::new();
        let editor = load_projected_document_set_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert!(matches!(
            editor.connectivity(),
            ConnectivityProjection::Valid { .. }
        ));
        assert_eq!(editor.stream_profile_sites().len(), 1);

        let error = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            strict_options(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "E_DOC_RESOURCE_PROFILE_RESOLVE");
        assert!(matches!(
            error,
            DocumentSetError::StreamProfileDependency {
                required: false,
                ..
            }
        ));

        let required_error = load_document_tree_from_model(
            Hcdf {
                name: "root".to_owned(),
                version: "1.0".to_owned(),
                stream_profile: vec![profile_resource("missing.xml", true)],
                ..Default::default()
            },
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            strict_options(),
        )
        .unwrap_err();
        assert_eq!(required_error.code(), "E_DOC_RESOURCE_PROFILE_REQUIRED");
        assert!(matches!(
            required_error,
            DocumentSetError::StreamProfileDependency {
                required: true,
                diagnostic,
                ..
            } if diagnostic.code() == "E_DOC_RESOURCE_PROFILE_RESOLVE"
        ));
    }

    #[test]
    fn duplicate_default_selection_includes_unresolved_optional_profiles() {
        let mut first = profile_resource("missing.xml", false);
        first.selection_role = Some(StreamProfileSelectionRole::Default);
        let mut second = profile_resource("loaded.xml", false);
        second.selection_role = Some(StreamProfileSelectionRole::Default);
        let root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![first, second],
            ..Default::default()
        };

        let selection_issue = |projected: &ProjectedConnectivityDocumentSet| {
            let ConnectivityProjection::Invalid { issues } = projected.connectivity() else {
                panic!("duplicate default selection must invalidate connectivity");
            };
            let matching = issues
                .iter()
                .filter_map(|issue| match issue {
                    ProjectedConnectivityIssue::Normalization(issue)
                        if issue.code() == "E_CONN_PROFILE_DEFAULT_SELECTION" =>
                    {
                        Some(issue)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(matching.len(), 1);
            assert_eq!(matching[0].related().len(), 1);
            (
                matching[0].subject_id(),
                matching[0].related()[0].stable_id(),
            )
        };

        let mut missing_resolver = MemoryDocumentResolver::new();
        missing_resolver
            .insert_document("/mem/loaded.xml", stream_profile("loaded", &[]))
            .unwrap();
        let missing = load_projected_document_set_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut missing_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert!(missing.stream_profile_sites()[0]
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "E_DOC_RESOURCE_PROFILE_RESOLVE"));
        let unresolved_ids = selection_issue(&missing);

        let mut loaded_resolver = MemoryDocumentResolver::new();
        loaded_resolver
            .insert_document("/mem/missing.xml", stream_profile("first", &[]))
            .unwrap();
        loaded_resolver
            .insert_document("/mem/loaded.xml", stream_profile("loaded", &[]))
            .unwrap();
        let loaded = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut loaded_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(selection_issue(&loaded), unresolved_ids);
    }

    #[test]
    fn profile_failure_cache_is_exact_and_denied_or_invalid_references_are_always_fatal() {
        let root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![
                profile_resource("missing.xml", false),
                profile_resource("missing.xml", false),
            ],
            ..Default::default()
        };
        let mut calls = 0;
        let mut resolver = CallbackDocumentResolver::new(
            |_parent: &DocumentResourceKey, _uri: &str, kind| {
                calls += 1;
                assert_eq!(kind, DocumentDependencyKind::StreamProfile);
                Err(ResolverFailure::not_found("missing profile"))
            },
            |_parent: &DocumentResourceKey, _uri: &str| Err(ResolverFailure::not_found("unused")),
        );
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(tree.stream_profile_sites().len(), 2);
        assert_eq!(tree.work().resolver_calls, 1);

        for kind in [
            ResolverFailureKind::Denied,
            ResolverFailureKind::InvalidReference,
        ] {
            let mut resolver = CallbackDocumentResolver::new(
                move |_parent: &DocumentResourceKey, _uri: &str, _kind| {
                    Err(ResolverFailure::new(kind, "blocked"))
                },
                |_parent: &DocumentResourceKey, _uri: &str| {
                    Err(ResolverFailure::not_found("unused"))
                },
            );
            let error = load_document_tree_from_model(
                Hcdf {
                    name: "root".to_owned(),
                    version: "1.0".to_owned(),
                    stream_profile: vec![profile_resource("blocked.xml", false)],
                    ..Default::default()
                },
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver,
                DocumentSetOptions::default(),
            )
            .unwrap_err();
            assert_eq!(error.code(), "E_DOC_RESOURCE_PROFILE_RESOLVE");
            assert!(matches!(
                error,
                DocumentSetError::StreamProfileDependency {
                    required: false,
                    diagnostic,
                    ..
                } if matches!(
                    *diagnostic,
                    StreamProfileDependencyDiagnostic::ResolutionFailure {
                        failure: ResolverFailure { kind: actual, .. }
                    } if actual == kind
                )
            ));
        }
    }

    #[test]
    fn optional_profile_integrity_and_content_failures_still_invalidate_connectivity() {
        let assert_issue = |projected: &ProjectedConnectivityDocumentSet, expected: &str| {
            let ConnectivityProjection::Invalid { issues } = projected.connectivity() else {
                panic!("optional profile failure {expected} must invalidate connectivity");
            };
            assert!(issues.iter().any(|issue| {
                matches!(
                    issue,
                    ProjectedConnectivityIssue::StreamProfileDependency { diagnostic, .. }
                        if diagnostic.code() == expected
                )
            }));
        };

        let optional_root = |uri: &str| Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource(uri, false)],
            ..Default::default()
        };

        let mut kind_resolver = MemoryDocumentResolver::new();
        kind_resolver
            .insert_document("/mem/wrong.hcdf", module("wrong", &[]))
            .unwrap();
        let projected = load_projected_document_set_from_model(
            optional_root("wrong.hcdf"),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut kind_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_issue(&projected, "E_DOC_RESOURCE_PROFILE_KIND");

        let mut parse_resolver = MemoryDocumentResolver::new();
        parse_resolver
            .insert_document(
                "/mem/malformed.xml",
                br#"<stream-profile name="malformed" version="1.0"><unknown/></stream-profile>"#
                    .to_vec(),
            )
            .unwrap();
        let projected = load_projected_document_set_from_model(
            optional_root("malformed.xml"),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut parse_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_issue(&projected, "E_DOC_RESOURCE_PROFILE_PARSE");

        let mut digest_root = optional_root("digest.xml");
        digest_root.stream_profile[0].sha = Some("sha256:wrong".to_owned());
        let mut digest_resolver = MemoryDocumentResolver::new();
        digest_resolver
            .insert_document("/mem/digest.xml", stream_profile("digest", &[]))
            .unwrap();
        let projected = load_projected_document_set_from_model(
            digest_root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut digest_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_issue(&projected, "E_DOC_RESOURCE_PROFILE_DIGEST");

        let mut cycle_resolver = MemoryDocumentResolver::new();
        cycle_resolver
            .insert_document("/mem/a.xml", stream_profile("a", &[("b.xml", false)]))
            .unwrap();
        cycle_resolver
            .insert_document("/mem/b.xml", stream_profile("b", &[("a.xml", false)]))
            .unwrap();
        let projected = load_projected_document_set_from_model(
            optional_root("a.xml"),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut cycle_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_issue(&projected, "E_DOC_RESOURCE_PROFILE_CYCLE");
    }

    #[test]
    fn profile_kind_digest_cycle_and_cross_kind_conflict_are_typed() {
        let mut kind_resolver = MemoryDocumentResolver::new();
        kind_resolver
            .insert_document("/mem/not-profile.hcdf", module("wrong", &[]))
            .unwrap();
        let kind_root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource("not-profile.hcdf", true)],
            ..Default::default()
        };
        let kind_tree = load_document_tree_from_model(
            kind_root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut kind_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(
            kind_tree.stream_profile_sites()[0].diagnostics[0].code(),
            "E_DOC_RESOURCE_PROFILE_KIND"
        );

        let profile_bytes = stream_profile("digest", &[]);
        let mut digest_resolver = MemoryDocumentResolver::new();
        digest_resolver
            .insert_document("/mem/profile.xml", profile_bytes)
            .unwrap();
        let mut digest_root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource("profile.xml", true)],
            ..Default::default()
        };
        digest_root.stream_profile[0].sha = Some("sha256:wrong".to_owned());
        let digest_tree = load_document_tree_from_model(
            digest_root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut digest_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(
            digest_tree.stream_profile_sites()[0].diagnostics[0].code(),
            "E_DOC_RESOURCE_PROFILE_DIGEST"
        );

        let mut cycle_resolver = MemoryDocumentResolver::new();
        cycle_resolver
            .insert_document("/mem/a.xml", stream_profile("a", &[("b.xml", true)]))
            .unwrap();
        cycle_resolver
            .insert_document("/mem/b.xml", stream_profile("b", &[("a.xml", true)]))
            .unwrap();
        let cycle_root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource("a.xml", true)],
            ..Default::default()
        };
        let cycle_tree = load_document_tree_from_model(
            cycle_root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut cycle_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(
            cycle_tree.stream_profile_sites()[2].diagnostics[0].code(),
            "E_DOC_RESOURCE_PROFILE_CYCLE"
        );

        let profile = stream_profile("shared", &[]);
        let hcdf = module("shared", &[]);
        let mut resolver = CallbackDocumentResolver::new(
            move |_parent: &DocumentResourceKey, uri: &str, _kind| {
                Ok(ResolvedDocumentResource {
                    key: DocumentResourceKey::new("/mem/shared.xml").unwrap(),
                    bytes: if uri == "profile.xml" {
                        profile.clone()
                    } else {
                        hcdf.clone()
                    },
                })
            },
            |_parent: &DocumentResourceKey, _uri: &str| Err(ResolverFailure::not_found("unused")),
        );
        let conflict_root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource("profile.xml", true)],
            include: vec![include("module.hcdf", Some("module"))],
            ..Default::default()
        };
        let error = load_document_tree_from_model(
            conflict_root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(error, DocumentSetError::SourceConflict { .. }));
    }

    #[test]
    fn stream_profile_root_is_rejected_as_an_hcdf_entry_document() {
        for bytes in [
            stream_profile("wrong-root", &[]),
            br#"<stream-profile name="malformed" version="1.0"><unknown/></stream-profile>"#
                .to_vec(),
        ] {
            let error = load_document_tree_from_bytes(
                bytes,
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.xml").unwrap(),
                &mut MemoryDocumentResolver::new(),
                DocumentSetOptions::default(),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                DocumentSetError::DocumentContentKindMismatch {
                    expected: DocumentContentKind::Hcdf,
                    actual: DocumentContentKind::StreamProfile,
                    ..
                }
            ));
        }
    }

    #[test]
    fn projected_profiles_add_typed_nodes_and_resolve_structured_stream_edges() {
        let module = Hcdf::from_xml_str(
            r#"<hcdf name="module" version="1.0">
              <comp name="left"><port name="p"/></comp>
              <comp name="right"><port name="p"/></comp>
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <configuration>
                  <traffic-class name="control" number="5"><pcp value="5"/></traffic-class>
                  <gate-schedule name="main" cycle-time-ns="1000000">
                    <gate duration-ns="1000000"><open><traffic-class-ref network="network" traffic-class="control"/></open></gate>
                  </gate-schedule>
                </configuration>
                <participant name="left"><endpoint><port-ref component="left" port="p"/></endpoint></participant>
                <participant name="right"><endpoint><port-ref component="right" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#,
        )
        .unwrap();
        let mut root = root_with(vec![include("module.hcdf", Some("module"))]);
        root.stream_profile
            .push(profile_resource("profile.xml", true));
        let instance = r#"<instance><segment name="module" occurrence="0"/></instance>"#;
        let profile = format!(
            r#"<stream-profile name="operations" version="1.0">
              <stream-group name="control"><description>Control traffic</description></stream-group>
              <stream name="command" vlan-id="42" pcp="5" max-frame-size-bytes="256" interval-ns="1000000">
                <description>Joint command stream</description>
                <group-ref group="control"/>
                <path><network-ref network="network">{instance}</network-ref></path>
                <talker><participant-ref network="network" participant="left">{instance}</participant-ref></talker>
                <listener><participant-ref network="network" participant="right">{instance}</participant-ref></listener>
                <traffic-class-ref network="network" traffic-class="control">{instance}</traffic-class-ref>
                <schedule-ref network="network" schedule="main">{instance}</schedule-ref>
              </stream>
            </stream-profile>"#
        );
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/module.hcdf",
                module.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        resolver
            .insert_document("/mem/profile.xml", profile.into_bytes())
            .unwrap();

        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let ConnectivityProjection::Valid { graph, .. } = projected.connectivity() else {
            panic!("expected a valid profile-extended connectivity graph");
        };
        assert_eq!(
            graph
                .nodes()
                .iter()
                .filter(|node| node.kind() == ObjectKind::Profile)
                .count(),
            1
        );
        assert_eq!(
            graph
                .nodes()
                .iter()
                .filter(|node| node.kind() == ObjectKind::Group)
                .count(),
            1
        );
        let group = graph
            .resolver(IncludeInstanceId::root())
            .stream_group(&CanonicalStreamGroupRef::local("control"))
            .unwrap();
        assert_eq!(group.kind(), ObjectKind::Group);
        let stream = graph
            .nodes()
            .iter()
            .find(|node| node.kind() == ObjectKind::Stream)
            .unwrap();
        let ConnectivityNodeData::Stream { definition } = stream.data() else {
            panic!("expected typed stream content");
        };
        assert_eq!(definition.name, "command");
        assert_eq!(definition.vlan_id, Some(42));
        assert_eq!(definition.listener.len(), 1);
        for kind in [
            EdgeKind::ProfileContainment,
            EdgeKind::StreamGroup,
            EdgeKind::StreamPath,
            EdgeKind::StreamTalker,
            EdgeKind::StreamListener,
            EdgeKind::StreamTrafficClass,
            EdgeKind::StreamSchedule,
        ] {
            assert!(graph.edges().iter().any(|edge| edge.kind() == kind));
        }
        let path = graph
            .edges()
            .iter()
            .find(|edge| edge.kind() == EdgeKind::StreamPath)
            .unwrap();
        assert_eq!(path.from(), stream.id());
        let network = graph.node(path.to()).unwrap();
        assert_eq!(network.kind(), ObjectKind::Network);
        assert_eq!(
            network.identity().instance(),
            &IncludeInstanceId::root().named_child("module", 0)
        );
    }

    #[test]
    fn nested_profiles_are_nodes_connected_by_exact_dependency_edges() {
        let mut root = root_with(Vec::new());
        root.stream_profile
            .push(profile_resource("profiles/main.xml", true));
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/profiles/main.xml",
                stream_profile("main", &[("nested/child.xml", true)]),
            )
            .unwrap();
        resolver
            .insert_document(
                "/mem/profiles/nested/child.xml",
                stream_profile("child", &[]),
            )
            .unwrap();
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let ConnectivityProjection::Valid { graph, .. } = projected.connectivity() else {
            panic!("expected nested profiles to extend the graph");
        };
        assert_eq!(
            graph
                .nodes()
                .iter()
                .filter(|node| node.kind() == ObjectKind::Profile)
                .count(),
            2
        );
        let dependency_edges = graph
            .edges()
            .iter()
            .filter(|edge| edge.kind() == EdgeKind::ProfileDependency)
            .collect::<Vec<_>>();
        assert_eq!(dependency_edges.len(), 1);
        assert_eq!(dependency_edges[0].exactness(), EdgeExactness::Exact);
        assert_eq!(
            graph.node(dependency_edges[0].from()).unwrap().kind(),
            ObjectKind::Profile
        );
        assert_eq!(
            graph.node(dependency_edges[0].to()).unwrap().kind(),
            ObjectKind::Profile
        );
    }

    #[test]
    fn profile_group_declaration_order_does_not_change_the_canonical_graph() {
        let root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            stream_profile: vec![profile_resource("profile.xml", true)],
            ..Default::default()
        };
        let first = br#"<stream-profile name="ordered" version="1.0">
          <stream-group name="alpha"/><stream-group name="beta"/>
        </stream-profile>"#;
        let second = br#"<stream-profile name="ordered" version="1.0">
          <stream-group name="beta"/><stream-group name="alpha"/>
        </stream-profile>"#;
        let mut canonical = Vec::new();
        for profile in [first.as_slice(), second.as_slice()] {
            let mut resolver = MemoryDocumentResolver::new();
            resolver
                .insert_document("/mem/profile.xml", profile.to_vec())
                .unwrap();
            let projected = load_projected_document_set_from_model(
                root.clone(),
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver,
                DocumentSetOptions::default(),
            )
            .unwrap();
            canonical.push(
                projected
                    .connectivity()
                    .graph()
                    .unwrap()
                    .to_canonical_json()
                    .unwrap(),
            );
        }
        assert_eq!(canonical[0], canonical[1]);
    }

    fn forwarding_fixture_root() -> Hcdf {
        Hcdf::from_xml_str(
            r#"<hcdf name="route-fixture" version="1.0">
              <comp name="source"><port name="p"/></comp>
              <comp name="sink-final"><port name="p"/></comp>
              <comp name="sink-a"><port name="p"/></comp>
              <comp name="sink-b"><port name="p"/></comp>
              <comp name="gateway">
                <port name="p0"/><port name="p1"/><port name="p2"/><port name="p3"/>
                <bridge name="fanout">
                  <input><port-ref component="gateway" port="p0"/></input>
                  <output><port-ref component="gateway" port="p1"/></output>
                  <output><port-ref component="gateway" port="p2"/></output>
                </bridge>
                <bridge name="merge">
                  <input><port-ref component="gateway" port="p1"/></input>
                  <input><port-ref component="gateway" port="p2"/></input>
                  <output><port-ref component="gateway" port="p3"/></output>
                </bridge>
                <bridge name="return">
                  <input><port-ref component="gateway" port="p1"/></input>
                  <output><port-ref component="gateway" port="p0"/></output>
                </bridge>
              </comp>
              <link name="ingress">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="source"><endpoint><port-ref component="source" port="p"/></endpoint></participant>
                <participant name="gateway-0"><endpoint><port-ref component="gateway" port="p0"/></endpoint></participant>
              </link>
              <link name="branch-a">
                <selected purpose="communication" carrier="electrical"/>
                <configuration>
                  <traffic-class name="control" number="5"><pcp value="5"/></traffic-class>
                </configuration>
                <participant name="gateway-1"><endpoint><port-ref component="gateway" port="p1"/></endpoint></participant>
                <participant name="sink-a"><endpoint><port-ref component="sink-a" port="p"/></endpoint></participant>
              </link>
              <link name="branch-b">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="gateway-2"><endpoint><port-ref component="gateway" port="p2"/></endpoint></participant>
                <participant name="sink-b"><endpoint><port-ref component="sink-b" port="p"/></endpoint></participant>
              </link>
              <link name="egress">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="gateway-3"><endpoint><port-ref component="gateway" port="p3"/></endpoint></participant>
                <participant name="sink-final"><endpoint><port-ref component="sink-final" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#,
        )
        .unwrap()
    }

    fn project_forwarding_profile(profile: &str) -> ProjectedConnectivityDocumentSet {
        let mut root = forwarding_fixture_root();
        root.stream_profile
            .push(profile_resource("profile.xml", true));
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/profile.xml", profile.as_bytes().to_vec())
            .unwrap();
        load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("route-fixture").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap()
    }

    fn forwarding(
        from_network: &str,
        from_participant: &str,
        function: &str,
        to_network: &str,
        to_participant: &str,
    ) -> String {
        format!(
            r#"<forwarding>
              <from><participant-ref network="{from_network}" participant="{from_participant}"/></from>
              <function-ref component="gateway" function="{function}"/>
              <to><participant-ref network="{to_network}" participant="{to_participant}"/></to>
            </forwarding>"#
        )
    }

    #[test]
    fn multicast_route_graph_projects_exact_stable_forwarding_nodes() {
        let first_forwarding =
            forwarding("ingress", "gateway-0", "fanout", "branch-a", "gateway-1");
        let second_forwarding =
            forwarding("ingress", "gateway-0", "fanout", "branch-b", "gateway-2");
        let profile = format!(
            r#"<stream-profile name="multicast" version="1.0">
              <stream name="fanout" max-frame-size-bytes="256" interval-ns="1000000">
                <path>
                  <network-ref network="ingress"/>
                  <network-ref network="branch-a"/>
                  <network-ref network="branch-b"/>
                  {first_forwarding}
                  {second_forwarding}
                </path>
                <talker><participant-ref network="ingress" participant="source"/></talker>
                <listener><participant-ref network="branch-a" participant="sink-a"/></listener>
                <listener><participant-ref network="branch-b" participant="sink-b"/></listener>
                <traffic-class-ref network="branch-a" traffic-class="control"/>
              </stream>
            </stream-profile>"#
        );
        let projected = project_forwarding_profile(&profile);
        let ConnectivityProjection::Valid { graph, .. } = projected.connectivity() else {
            panic!("explicit multicast forwarding should produce a valid graph");
        };
        assert_eq!(
            graph
                .nodes()
                .iter()
                .filter(|node| node.kind() == ObjectKind::StreamForwarding)
                .count(),
            2
        );
        assert_eq!(
            graph
                .edges()
                .iter()
                .filter(|edge| edge.kind() == EdgeKind::StreamPath)
                .count(),
            3
        );
        for kind in [
            EdgeKind::StreamForwarding,
            EdgeKind::StreamForwardingFrom,
            EdgeKind::StreamForwardingFunction,
            EdgeKind::StreamForwardingTo,
        ] {
            assert_eq!(
                graph
                    .edges()
                    .iter()
                    .filter(|edge| edge.kind() == kind)
                    .count(),
                2
            );
        }
        for node in graph
            .nodes()
            .iter()
            .filter(|node| node.kind() == ObjectKind::StreamForwarding)
        {
            let ConnectivityNodeData::StreamForwarding { forwarding } = node.data() else {
                panic!("stream-forwarding identity must retain typed authored content");
            };
            assert_eq!(forwarding.function.function, "fanout");
        }

        let reordered = format!(
            r#"<stream-profile name="multicast" version="1.0">
              <stream name="fanout" max-frame-size-bytes="256" interval-ns="1000000">
                <path>
                  <network-ref network="branch-b"/>
                  <network-ref network="ingress"/>
                  <network-ref network="branch-a"/>
                  {second_forwarding}
                  {first_forwarding}
                </path>
                <talker><participant-ref network="ingress" participant="source"/></talker>
                <listener><participant-ref network="branch-a" participant="sink-a"/></listener>
                <listener><participant-ref network="branch-b" participant="sink-b"/></listener>
                <traffic-class-ref network="branch-a" traffic-class="control"/>
              </stream>
            </stream-profile>"#
        );
        let reordered = project_forwarding_profile(&reordered);
        let forwarding_ids = |projection: &ProjectedConnectivityDocumentSet| {
            projection
                .connectivity()
                .graph()
                .unwrap()
                .nodes()
                .iter()
                .filter(|node| node.kind() == ObjectKind::StreamForwarding)
                .map(|node| node.id().clone())
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(forwarding_ids(&projected), forwarding_ids(&reordered));
    }

    #[test]
    fn route_graph_allows_reconverging_redundant_paths_without_segment_streams() {
        let profile = format!(
            r#"<stream-profile name="redundant" version="1.0">
              <stream name="reconverged" max-frame-size-bytes="256" interval-ns="1000000">
                <path>
                  <network-ref network="ingress"/>
                  <network-ref network="branch-a"/>
                  <network-ref network="branch-b"/>
                  <network-ref network="egress"/>
                  {}
                  {}
                  {}
                  {}
                </path>
                <talker><participant-ref network="ingress" participant="source"/></talker>
                <listener><participant-ref network="egress" participant="sink-final"/></listener>
                <frer seamless-trees="2" sequence-encoding="r-tag"/>
              </stream>
            </stream-profile>"#,
            forwarding("ingress", "gateway-0", "fanout", "branch-a", "gateway-1",),
            forwarding("ingress", "gateway-0", "fanout", "branch-b", "gateway-2",),
            forwarding("branch-a", "gateway-1", "merge", "egress", "gateway-3",),
            forwarding("branch-b", "gateway-2", "merge", "egress", "gateway-3",),
        );
        let projected = project_forwarding_profile(&profile);
        let graph = projected
            .connectivity()
            .graph()
            .expect("reconverging forwarding DAG should be valid");
        assert_eq!(
            graph
                .nodes()
                .iter()
                .filter(|node| node.kind() == ObjectKind::Stream)
                .count(),
            1,
            "one end-to-end stream must not be split into per-segment streams"
        );
        assert_eq!(
            graph
                .nodes()
                .iter()
                .filter(|node| node.kind() == ObjectKind::StreamForwarding)
                .count(),
            4
        );
    }

    fn route_profile(networks: &str, forwardings: &str, listeners: &str) -> String {
        format!(
            r#"<stream-profile name="route-check" version="1.0">
              <stream name="checked" max-frame-size-bytes="64" interval-ns="1000">
                <path>{networks}{forwardings}</path>
                <talker><participant-ref network="ingress" participant="source"/></talker>
                {listeners}
              </stream>
            </stream-profile>"#
        )
    }

    #[test]
    fn route_graph_rejects_ambiguous_or_unproven_forwarding() {
        let ingress = r#"<network-ref network="ingress"/>"#;
        let branch_a = r#"<network-ref network="branch-a"/>"#;
        let branch_b = r#"<network-ref network="branch-b"/>"#;
        let listener_ingress =
            r#"<listener><participant-ref network="ingress" participant="gateway-0"/></listener>"#;
        let listener_a =
            r#"<listener><participant-ref network="branch-a" participant="sink-a"/></listener>"#;
        let listener_b =
            r#"<listener><participant-ref network="branch-b" participant="sink-b"/></listener>"#;
        let ingress_to_a = forwarding("ingress", "gateway-0", "fanout", "branch-a", "gateway-1");

        let cases = vec![
            (
                "duplicate path network",
                route_profile(&format!("{ingress}{ingress}"), "", listener_ingress),
                "E_CONN_PROFILE_DUPLICATE_PATH_NETWORK",
            ),
            (
                "duplicate forwarding triple",
                route_profile(
                    &format!("{ingress}{branch_a}"),
                    &format!("{ingress_to_a}{ingress_to_a}"),
                    listener_a,
                ),
                "E_CONN_PROFILE_DUPLICATE_FORWARDING",
            ),
            (
                "self forwarding",
                route_profile(
                    ingress,
                    &forwarding("ingress", "gateway-0", "fanout", "ingress", "source"),
                    listener_ingress,
                ),
                "E_CONN_PROFILE_SELF_FORWARDING",
            ),
            (
                "unproven input relation",
                route_profile(
                    &format!("{ingress}{branch_a}"),
                    &forwarding("ingress", "source", "fanout", "branch-a", "gateway-1"),
                    listener_a,
                ),
                "E_CONN_PROFILE_FORWARDING_INPUT",
            ),
            (
                "unproven output relation",
                route_profile(
                    &format!("{ingress}{branch_a}"),
                    &forwarding("ingress", "gateway-0", "fanout", "branch-a", "sink-a"),
                    listener_a,
                ),
                "E_CONN_PROFILE_FORWARDING_OUTPUT",
            ),
            (
                "forwarding cycle",
                route_profile(
                    &format!("{ingress}{branch_a}"),
                    &format!(
                        "{}{}",
                        ingress_to_a,
                        forwarding("branch-a", "gateway-1", "return", "ingress", "gateway-0",)
                    ),
                    listener_a,
                ),
                "E_CONN_PROFILE_FORWARDING_CYCLE",
            ),
            (
                "orphan listed network",
                route_profile(
                    &format!("{ingress}{branch_a}{branch_b}"),
                    &ingress_to_a,
                    listener_a,
                ),
                "E_CONN_PROFILE_ROUTE_COVERAGE",
            ),
            (
                "unreachable listener network",
                route_profile(
                    &format!("{ingress}{branch_a}{branch_b}"),
                    &ingress_to_a,
                    listener_b,
                ),
                "E_CONN_PROFILE_ROUTE_COVERAGE",
            ),
        ];

        for (name, profile, expected) in cases {
            let projected = project_forwarding_profile(&profile);
            let ConnectivityProjection::Invalid { issues } = projected.connectivity() else {
                panic!("{name} must invalidate the canonical connectivity graph");
            };
            assert!(
                issues.iter().any(|issue| {
                    matches!(
                        issue,
                        ProjectedConnectivityIssue::Normalization(issue)
                            if issue.code() == expected
                    )
                }),
                "{name} did not produce {expected}: {issues:?}"
            );
        }
    }

    #[test]
    fn stream_paths_reference_existing_networks_without_fake_chain_types() {
        let mut root = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="left"><port name="p"/></comp>
              <comp name="right"><port name="p"/></comp>
              <chain name="path">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="left"><endpoint><port-ref component="left" port="p"/></endpoint></participant>
                <participant name="right"><endpoint><port-ref component="right" port="p"/></endpoint></participant>
                <hop name="left-hop"><owner><component-ref component="left"/></owner></hop>
                <hop name="right-hop"><owner><component-ref component="right"/></owner></hop>
                <leg name="left-to-right">
                  <from><hop-ref network="path" hop="left-hop"/><participant-ref network="path" participant="left"/></from>
                  <to><hop-ref network="path" hop="right-hop"/><participant-ref network="path" participant="right"/></to>
                </leg>
              </chain>
            </hcdf>"#,
        )
        .unwrap();
        root.stream_profile
            .push(profile_resource("profile.xml", true));
        let profile = br#"<stream-profile name="chain-profile" version="1.0">
          <stream name="chain-stream" max-frame-size-bytes="64" interval-ns="1000">
            <path><network-ref network="path"/></path>
            <talker><participant-ref network="path" participant="left"/></talker>
            <listener><participant-ref network="path" participant="right"/></listener>
          </stream>
        </stream-profile>"#;
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/profile.xml", profile.to_vec())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let ConnectivityProjection::Valid { graph, .. } = projected.connectivity() else {
            panic!("network-ref should resolve the existing chain topology");
        };
        let path = graph
            .edges()
            .iter()
            .find(|edge| edge.kind() == EdgeKind::StreamPath)
            .unwrap();
        assert!(matches!(
            graph.node(path.to()).unwrap().data(),
            ConnectivityNodeData::Network {
                topology: Topology::Chain,
                ..
            }
        ));
    }

    #[test]
    fn unresolved_stream_references_clear_only_the_connectivity_graph() {
        let mut root = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="left"><port name="p"/></comp>
              <comp name="right"><port name="p"/></comp>
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="left"><endpoint><port-ref component="left" port="p"/></endpoint></participant>
                <participant name="right"><endpoint><port-ref component="right" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#,
        )
        .unwrap();
        root.stream_profile
            .push(profile_resource("profile.xml", true));
        let profile = br#"<stream-profile name="invalid" version="1.0">
          <stream name="broken" max-frame-size-bytes="64" interval-ns="1000">
            <path><network-ref network="network"/></path>
            <talker><participant-ref network="network" participant="missing"/></talker>
            <listener><participant-ref network="network" participant="right"/></listener>
          </stream>
        </stream-profile>"#;
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/profile.xml", profile.to_vec())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        assert_eq!(projected.structural_projection().components.len(), 2);
        assert!(projected.connectivity().graph().is_none());
        let ConnectivityProjection::Invalid { issues } = projected.connectivity() else {
            panic!("unresolved stream references must invalidate connectivity");
        };
        assert!(issues.iter().any(|issue| {
            matches!(
                issue,
                ProjectedConnectivityIssue::Normalization(issue)
                    if issue.code() == "E_CONN_UNRESOLVED_REFERENCE"
            )
        }));
    }

    #[test]
    fn stream_endpoints_must_belong_to_the_selected_path_network() {
        let mut root = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="left"><port name="p"/></comp>
              <comp name="right"><port name="p"/></comp>
              <link name="primary">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="left"><endpoint><port-ref component="left" port="p"/></endpoint></participant>
                <participant name="right"><endpoint><port-ref component="right" port="p"/></endpoint></participant>
              </link>
              <link name="other">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="left"><endpoint><port-ref component="left" port="p"/></endpoint></participant>
                <participant name="right"><endpoint><port-ref component="right" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#,
        )
        .unwrap();
        root.stream_profile
            .push(profile_resource("profile.xml", true));
        let profile = br#"<stream-profile name="invalid-scope" version="1.0">
          <stream name="crossed" max-frame-size-bytes="64" interval-ns="1000">
            <path><network-ref network="primary"/></path>
            <talker><participant-ref network="other" participant="left"/></talker>
            <listener><participant-ref network="primary" participant="right"/></listener>
          </stream>
        </stream-profile>"#;
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/profile.xml", profile.to_vec())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let ConnectivityProjection::Invalid { issues } = projected.connectivity() else {
            panic!("cross-network stream endpoints must invalidate connectivity");
        };
        assert!(issues.iter().any(|issue| {
            matches!(
                issue,
                ProjectedConnectivityIssue::Normalization(issue)
                    if issue.code() == "E_CONN_PROFILE_STREAM_NETWORK"
            )
        }));
    }

    #[test]
    fn duplicate_listener_targets_are_rejected_with_a_domain_error() {
        let mut root = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="left"><port name="p"/></comp>
              <comp name="right"><port name="p"/></comp>
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="left"><endpoint><port-ref component="left" port="p"/></endpoint></participant>
                <participant name="right"><endpoint><port-ref component="right" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#,
        )
        .unwrap();
        root.stream_profile
            .push(profile_resource("profile.xml", true));
        let profile = br#"<stream-profile name="duplicates" version="1.0">
          <stream name="command" max-frame-size-bytes="64" interval-ns="1000">
            <path><network-ref network="network"/></path>
            <talker><participant-ref network="network" participant="left"/></talker>
            <listener><participant-ref network="network" participant="right"/></listener>
            <listener><participant-ref network="network" participant="right"/></listener>
          </stream>
        </stream-profile>"#;
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/profile.xml", profile.to_vec())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let ConnectivityProjection::Invalid { issues } = projected.connectivity() else {
            panic!("duplicate listener targets must invalidate connectivity");
        };
        assert!(issues.iter().any(|issue| {
            matches!(
                issue,
                ProjectedConnectivityIssue::Normalization(issue)
                    if issue.code() == "E_CONN_PROFILE_DUPLICATE_LISTENER"
            )
        }));
        assert!(!issues.iter().any(|issue| {
            matches!(
                issue,
                ProjectedConnectivityIssue::Normalization(issue)
                    if issue.code() == "E_CONN_PROFILE_GRAPH_EXTENSION"
            )
        }));
    }

    #[test]
    fn unnamed_siblings_use_occurrences_and_named_duplicates_are_transactional() {
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/module.hcdf", module("module", &[]))
            .unwrap();
        let tree = load_document_tree_from_model(
            root_with(vec![
                include("/mem/module.hcdf", None),
                include("/mem/module.hcdf", Some("")),
                include("/mem/module.hcdf", Some("wheel")),
                include("/mem/module.hcdf", Some("   ")),
                include("/mem/module.hcdf", Some("$include")),
            ]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let root = IncludeInstanceId::root();
        let unnamed = root.unnamed_child(0);
        let explicit_empty = root.unnamed_child(1);
        for id in [
            unnamed.clone(),
            explicit_empty.clone(),
            root.named_child("wheel", 0),
            root.named_child("   ", 0),
            root.named_child("$include", 0),
        ] {
            assert!(tree.instances.contains_key(&id), "{id:?}");
        }
        assert_eq!(tree.instances[&unnamed].authored_name, None);
        assert_eq!(
            tree.instances[&explicit_empty].authored_name.as_deref(),
            Some("")
        );
        assert_eq!(tree.flatten_prefixes.segment(&unnamed), Some(None));
        assert_eq!(tree.flatten_prefixes.segment(&explicit_empty), Some(None));

        let mut document_calls = 0;
        {
            let mut resolver = CallbackDocumentResolver::new(
                |_: &DocumentResourceKey,
                 _: &str,
                 _: DocumentDependencyKind|
                 -> Result<ResolvedDocumentResource, ResolverFailure> {
                    document_calls += 1;
                    Err(ResolverFailure::not_found("must not resolve"))
                },
                |_: &DocumentResourceKey,
                 _: &str|
                 -> Result<ResolvedAssetResource, ResolverFailure> {
                    unreachable!("duplicate-name validation does not resolve assets")
                },
            );
            let mut invalid = root_with(vec![
                include("/mem/first.hcdf", Some("wheel")),
                include("/mem/second.hcdf", Some("wheel")),
            ]);
            invalid
                .stream_profile
                .push(profile_resource("must-not-resolve.xml", true));
            let error = load_document_tree_from_model(
                invalid,
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver,
                DocumentSetOptions::default(),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                DocumentSetError::DuplicateIncludeName {
                    first_include_index: 0,
                    duplicate_include_index: 1,
                    ref name,
                    ..
                } if name == "wheel"
            ));
        }
        assert_eq!(document_calls, 0);
    }

    #[test]
    fn placement_frame_is_retained_as_explicit_unprojectable_state() {
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/module.hcdf", module("module", &[]))
            .unwrap();
        let mut placed = include("/mem/module.hcdf", Some("sensor"));
        placed.pose = Some("1 2 3 0 0 0".to_owned());
        placed.placement_frame = Some("mount".to_owned());
        let tree = load_document_tree_from_model(
            root_with(vec![placed]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        assert_eq!(
            tree.instances[&IncludeInstanceId::root().child("sensor", 0)].placement,
            PlacementState::Unprojectable {
                ancestry: vec![UnprojectablePlacementStep {
                    instance: IncludeInstanceId::root().child("sensor", 0),
                    authored_pose: Some("1 2 3 0 0 0".to_owned()),
                    placement_frame: Some("mount".to_owned()),
                    causes: vec![PlacementProjectionCause::PlacementFrameReference],
                }],
            }
        );
    }

    #[test]
    fn memory_resolver_maps_resource_reference_errors() {
        let error = MemoryDocumentResolver::new()
            .resolve_document(
                &DocumentResourceKey::new("urn:opaque-parent").unwrap(),
                "child.hcdf",
                DocumentDependencyKind::HcdfInclude,
            )
            .unwrap_err();
        assert_eq!(error.kind, ResolverFailureKind::InvalidReference);
        assert!(error.message.contains("opaque"), "{}", error.message);
    }

    #[test]
    fn projected_facades_resolve_root_and_included_assets_with_exact_provenance() {
        let module = single_asset_document("module", "module.glb", None);
        let module_bytes = module.to_xml_string().unwrap().into_bytes();
        let mut root = single_asset_document("root", "root.glb", None);
        root.include = vec![include("module.hcdf", Some("module"))];
        let root_key = DocumentResourceKey::new("/mem/root.hcdf").unwrap();
        let module_key = DocumentResourceKey::new("/mem/module.hcdf").unwrap();

        let mut model_resolver = MemoryDocumentResolver::new();
        model_resolver
            .insert_document(module_key.as_str(), module_bytes.clone())
            .unwrap();
        model_resolver
            .insert_asset("/mem/root.glb", b"root".to_vec())
            .unwrap();
        model_resolver
            .insert_asset("/mem/module.glb", b"module".to_vec())
            .unwrap();
        let from_model = load_projected_document_set_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            root_key.clone(),
            &mut model_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(
            from_model.sources()[&root_key].provenance,
            DocumentSourceProvenance::CanonicalModelSerialization
        );
        assert_eq!(
            from_model.sources()[&module_key].provenance,
            DocumentSourceProvenance::ResolvedDependencyBytes
        );
        assert_eq!(from_model.assets().resources().len(), 2);
        assert_eq!(from_model.assets().sites().len(), 2);
        for site in from_model.assets().sites() {
            let AssetSiteResolution::Resolved { key } = &site.resolution else {
                panic!("expected resolved asset site");
            };
            assert!(from_model.assets().resource(key).is_some());
        }

        let raw_root = root.to_xml_string().unwrap().into_bytes();
        let mut bytes_resolver = MemoryDocumentResolver::new();
        bytes_resolver
            .insert_document(module_key.as_str(), module_bytes)
            .unwrap();
        bytes_resolver
            .insert_asset("/mem/root.glb", b"root".to_vec())
            .unwrap();
        bytes_resolver
            .insert_asset("/mem/module.glb", b"module".to_vec())
            .unwrap();
        let from_bytes = load_projected_document_set_from_bytes(
            raw_root,
            DocumentIdentity::new("robot").unwrap(),
            root_key.clone(),
            &mut bytes_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(
            from_bytes.sources()[&root_key].provenance,
            DocumentSourceProvenance::RawRootBytes
        );
        assert_eq!(from_bytes.assets().resources().len(), 2);
        assert_eq!(from_bytes.assets().sites().len(), 2);
    }

    #[test]
    fn repeated_asset_instances_expand_sites_but_share_exact_request_cache() {
        let module = single_asset_document("module", "shared.glb", None);
        let module_bytes = module.to_xml_string().unwrap().into_bytes();
        let root = root_with(vec![
            include("module.hcdf", Some("left")),
            include("module.hcdf", Some("right")),
        ]);
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/module.hcdf", module_bytes.clone())
            .unwrap();
        resolver
            .insert_asset("/mem/shared.glb", b"asset".to_vec())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        assert_eq!(projected.assets().sites().len(), 2);
        assert_eq!(projected.assets().resources().len(), 1);
        assert_eq!(projected.work().dependency_sites, 2);
        assert_eq!(projected.work().asset_sites, 2);
        assert_eq!(projected.work().resolver_calls, 2);
        assert_eq!(
            projected.work().resolver_bytes,
            module_bytes.len() + b"asset".len()
        );
        assert_eq!(
            projected
                .assets()
                .sites()
                .iter()
                .map(|site| site.instance.clone())
                .collect::<Vec<_>>(),
            vec![
                IncludeInstanceId::root().child("left", 0),
                IncludeInstanceId::root().child("right", 0),
            ]
        );
    }

    #[test]
    fn asset_cache_identity_is_exact_parent_and_authored_uri() {
        let mut distinct = single_asset_document("root", "asset.glb", None);
        push_visual_asset(&mut distinct, "alternate", "./asset.glb", None);
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_asset("/mem/asset.glb", b"same".to_vec())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            distinct,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(projected.work().resolver_calls, 2);
        assert_eq!(projected.assets().resources().len(), 1);

        let mut repeated_failure = single_asset_document("root", "missing.glb", None);
        push_visual_asset(&mut repeated_failure, "again", "missing.glb", None);
        let projected = load_projected_document_set_from_model(
            repeated_failure,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(projected.work().resolver_calls, 1);
        assert_eq!(projected.assets().sites().len(), 2);
        assert!(projected.assets().sites().iter().all(|site| matches!(
            site.resolution,
            AssetSiteResolution::Unresolved {
                failure: AssetResolutionFailure::Resolver(_)
            }
        )));

        let left = single_asset_document("left", "asset.glb", None);
        let right = single_asset_document("right", "asset.glb", None);
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/left/module.hcdf",
                left.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        resolver
            .insert_document(
                "/mem/right/module.hcdf",
                right.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        resolver
            .insert_asset("/mem/left/asset.glb", b"left".to_vec())
            .unwrap();
        resolver
            .insert_asset("/mem/right/asset.glb", b"right".to_vec())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            root_with(vec![
                include("left/module.hcdf", Some("left")),
                include("right/module.hcdf", Some("right")),
            ]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(projected.work().resolver_calls, 4);
        assert_eq!(projected.assets().resources().len(), 2);
        assert_eq!(projected.assets().sites()[0].authored_uri, "asset.glb");
        assert_eq!(projected.assets().sites()[1].authored_uri, "asset.glb");
        assert_ne!(
            projected.assets().sites()[0].source,
            projected.assets().sites()[1].source
        );
    }

    #[test]
    fn asset_digests_are_adjudicated_per_site_and_filter_public_resources() {
        let bytes = b"asset-bytes".to_vec();
        let correct_sha = content_sha(&bytes);
        let mut mixed = single_asset_document("root", "asset.bin", Some(&correct_sha));
        push_visual_asset(&mut mixed, "mismatch", "asset.bin", Some("sha256:wrong"));
        push_visual_asset(&mut mixed, "unpinned", "asset.bin", Some(""));
        let mut resolver = MemoryDocumentResolver::new();
        let key = resolver
            .insert_asset("/mem/asset.bin", bytes.clone())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            mixed,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(projected.work().resolver_calls, 1);
        assert_eq!(projected.work().resolver_bytes, bytes.len());
        assert_eq!(projected.assets().resources().len(), 1);
        assert!(projected.assets().resource(&key).is_some());
        assert!(matches!(
            projected.assets().sites()[0].resolution,
            AssetSiteResolution::Resolved { .. }
        ));
        let AssetSiteResolution::Unresolved { failure } = &projected.assets().sites()[1].resolution
        else {
            panic!("expected per-site digest mismatch");
        };
        assert_eq!(failure.code(), "E_DOC_RESOURCE_ASSET_DIGEST");
        assert_eq!(
            failure,
            &AssetResolutionFailure::DigestMismatch {
                expected: "sha256:wrong".to_owned(),
                actual: correct_sha,
            }
        );
        assert!(matches!(
            projected.assets().sites()[2].resolution,
            AssetSiteResolution::Resolved { .. }
        ));

        let mismatch_only = single_asset_document("root", "asset.bin", Some("sha256:wrong"));
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_asset("/mem/asset.bin", bytes.clone())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            mismatch_only,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert!(projected.assets().resources().is_empty());
        assert_eq!(projected.work().resolver_bytes, bytes.len());
    }

    #[test]
    fn asset_resolver_failure_policy_is_explicit_for_every_kind() {
        for policy in [AssetPolicy::AllowMissing, AssetPolicy::RequireAllAssets] {
            for kind in [
                ResolverFailureKind::NotFound,
                ResolverFailureKind::Denied,
                ResolverFailureKind::InvalidReference,
                ResolverFailureKind::Unsupported,
                ResolverFailureKind::Other,
            ] {
                let options = DocumentSetOptions {
                    asset_policy: policy,
                    ..DocumentSetOptions::default()
                };
                let tree = load_document_tree_from_model(
                    single_asset_document("root", "asset.bin", None),
                    DocumentIdentity::new("robot").unwrap(),
                    DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                    &mut MemoryDocumentResolver::new(),
                    options,
                )
                .unwrap();
                let mut calls = 0;
                let outcome = {
                    let mut resolver = CallbackDocumentResolver::new(
                        |_: &DocumentResourceKey,
                         _: &str,
                         _: DocumentDependencyKind|
                         -> Result<ResolvedDocumentResource, ResolverFailure> {
                            unreachable!("projection must not reload documents")
                        },
                        |_: &DocumentResourceKey,
                         _: &str|
                         -> Result<ResolvedAssetResource, ResolverFailure> {
                            calls += 1;
                            Err(ResolverFailure::new(kind, format!("{kind:?}")))
                        },
                    );
                    project_document_tree(tree, &mut resolver)
                };
                assert_eq!(calls, 1);
                let retained = policy == AssetPolicy::AllowMissing
                    && matches!(
                        kind,
                        ResolverFailureKind::NotFound
                            | ResolverFailureKind::Unsupported
                            | ResolverFailureKind::Other
                    );
                if retained {
                    let projected = outcome.unwrap();
                    let AssetSiteResolution::Unresolved { failure } =
                        &projected.assets().sites()[0].resolution
                    else {
                        panic!("expected retained resolver failure");
                    };
                    assert_eq!(failure.code(), "E_DOC_RESOURCE_ASSET_RESOLVE");
                    assert!(matches!(
                        failure,
                        AssetResolutionFailure::Resolver(ResolverFailure {
                            kind: actual,
                            ..
                        }) if *actual == kind
                    ));
                } else {
                    let error = outcome.unwrap_err();
                    assert_eq!(error.code(), "E_DOC_RESOURCE_ASSET_RESOLVE");
                    assert!(matches!(
                        error,
                        DocumentSetError::AssetResolution {
                            failure: ResolverFailure {
                                kind: actual,
                                ..
                            },
                            ..
                        } if actual == kind
                    ));
                }
            }
        }
    }

    #[test]
    fn blank_asset_uris_are_fatal_without_resolver_calls_under_both_policies() {
        for policy in [AssetPolicy::AllowMissing, AssetPolicy::RequireAllAssets] {
            for uri in ["", " \t "] {
                let options = DocumentSetOptions {
                    asset_policy: policy,
                    ..DocumentSetOptions::default()
                };
                let tree = load_document_tree_from_model(
                    single_asset_document("root", uri, None),
                    DocumentIdentity::new("robot").unwrap(),
                    DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                    &mut MemoryDocumentResolver::new(),
                    options,
                )
                .unwrap();
                let mut calls = 0;
                let error = {
                    let mut resolver = CallbackDocumentResolver::new(
                        |_: &DocumentResourceKey,
                         _: &str,
                         _: DocumentDependencyKind|
                         -> Result<ResolvedDocumentResource, ResolverFailure> {
                            unreachable!("projection must not reload documents")
                        },
                        |_: &DocumentResourceKey,
                         _: &str|
                         -> Result<ResolvedAssetResource, ResolverFailure> {
                            calls += 1;
                            unreachable!("blank references must fail before resolution")
                        },
                    );
                    project_document_tree(tree, &mut resolver).unwrap_err()
                };
                assert_eq!(calls, 0);
                assert_eq!(error.code(), "E_DOC_RESOURCE_ASSET_RESOLVE");
                assert!(matches!(
                    error,
                    DocumentSetError::AssetResolution {
                        failure: ResolverFailure {
                            kind: ResolverFailureKind::InvalidReference,
                            ..
                        },
                        ..
                    }
                ));
            }
        }

        let mut absent = single_asset_document("root", "unused", None);
        let crate::model::VisualAppearance::Model { model, .. } =
            &mut absent.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        model.uri = None;
        let projected = load_projected_document_set_from_model(
            absent,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(projected.work().asset_sites, 0);
        assert!(projected.assets().sites().is_empty());
    }

    #[test]
    fn asset_source_conflicts_track_all_observed_bytes_under_both_policies() {
        let mut equal = single_asset_document("root", "first.bin", None);
        push_visual_asset(&mut equal, "second", "second.bin", None);
        let mut calls = 0;
        let projected = {
            let mut resolver = CallbackDocumentResolver::new(
                |_: &DocumentResourceKey,
                 _: &str,
                 _: DocumentDependencyKind|
                 -> Result<ResolvedDocumentResource, ResolverFailure> {
                    unreachable!("projection must not reload documents")
                },
                |_: &DocumentResourceKey,
                 _: &str|
                 -> Result<ResolvedAssetResource, ResolverFailure> {
                    calls += 1;
                    Ok(ResolvedAssetResource {
                        key: AssetResourceKey::new("/mem/shared.bin").unwrap(),
                        bytes: b"same".to_vec(),
                    })
                },
            );
            let tree = load_document_tree_from_model(
                equal,
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut MemoryDocumentResolver::new(),
                DocumentSetOptions::default(),
            )
            .unwrap();
            project_document_tree(tree, &mut resolver).unwrap()
        };
        assert_eq!(calls, 2);
        assert_eq!(projected.assets().resources().len(), 1);

        for policy in [AssetPolicy::AllowMissing, AssetPolicy::RequireAllAssets] {
            let mut conflict = single_asset_document("root", "first.bin", None);
            push_visual_asset(&mut conflict, "second", "second.bin", None);
            let options = DocumentSetOptions {
                asset_policy: policy,
                ..DocumentSetOptions::default()
            };
            let tree = load_document_tree_from_model(
                conflict,
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut MemoryDocumentResolver::new(),
                options,
            )
            .unwrap();
            let error = {
                let mut resolver = CallbackDocumentResolver::new(
                    |_: &DocumentResourceKey,
                     _: &str,
                     _: DocumentDependencyKind|
                     -> Result<ResolvedDocumentResource, ResolverFailure> {
                        unreachable!("projection must not reload documents")
                    },
                    |_: &DocumentResourceKey,
                     uri: &str|
                     -> Result<ResolvedAssetResource, ResolverFailure> {
                        Ok(ResolvedAssetResource {
                            key: AssetResourceKey::new("/mem/shared.bin").unwrap(),
                            bytes: uri.as_bytes().to_vec(),
                        })
                    },
                );
                project_document_tree(tree, &mut resolver).unwrap_err()
            };
            assert_eq!(error.code(), "E_DOC_RESOURCE_ASSET_SOURCE_CONFLICT");
            assert!(matches!(
                error,
                DocumentSetError::AssetSourceConflict { key }
                    if key == AssetResourceKey::new("/mem/shared.bin").unwrap()
            ));
        }

        let mut mismatch_then_conflict =
            single_asset_document("root", "first.bin", Some("sha256:wrong"));
        push_visual_asset(&mut mismatch_then_conflict, "second", "second.bin", None);
        let tree = load_document_tree_from_model(
            mismatch_then_conflict,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        let error = {
            let mut resolver = CallbackDocumentResolver::new(
                |_: &DocumentResourceKey,
                 _: &str,
                 _: DocumentDependencyKind|
                 -> Result<ResolvedDocumentResource, ResolverFailure> {
                    unreachable!("projection must not reload documents")
                },
                |_: &DocumentResourceKey,
                 uri: &str|
                 -> Result<ResolvedAssetResource, ResolverFailure> {
                    Ok(ResolvedAssetResource {
                        key: AssetResourceKey::new("/mem/shared.bin").unwrap(),
                        bytes: uri.as_bytes().to_vec(),
                    })
                },
            );
            project_document_tree(tree, &mut resolver).unwrap_err()
        };
        assert_eq!(error.code(), "E_DOC_RESOURCE_ASSET_SOURCE_CONFLICT");
    }

    #[test]
    fn required_asset_digest_mismatch_is_a_stable_fatal_error() {
        let bytes = b"asset".to_vec();
        let actual = content_sha(&bytes);
        let mut resolver = MemoryDocumentResolver::new();
        resolver.insert_asset("/mem/asset.bin", bytes).unwrap();
        let options = DocumentSetOptions {
            asset_policy: AssetPolicy::RequireAllAssets,
            ..DocumentSetOptions::default()
        };
        let error = load_projected_document_set_from_model(
            single_asset_document("root", "asset.bin", Some("sha256:expected")),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            options,
        )
        .unwrap_err();
        assert_eq!(error.code(), "E_DOC_RESOURCE_ASSET_DIGEST");
        assert!(matches!(
            error,
            DocumentSetError::AssetDigestMismatch {
                uri,
                expected,
                actual: observed,
                ..
            } if uri == "asset.bin"
                && expected == "sha256:expected"
                && observed == actual
        ));
    }

    #[test]
    fn asset_site_and_shared_resolver_limits_use_expanded_cache_aware_work() {
        let module = single_asset_document("module", "asset.bin", None);
        let module_bytes = module.to_xml_string().unwrap().into_bytes();
        let asset_bytes = b"asset".to_vec();
        let root = root_with(vec![
            include("module.hcdf", Some("one")),
            include("module.hcdf", Some("two")),
            include("module.hcdf", Some("three")),
        ]);
        let exact_limits = DocumentSetLimits {
            max_asset_sites: 3,
            max_resolver_calls: 2,
            max_resolver_bytes: module_bytes.len() + asset_bytes.len(),
            ..DocumentSetLimits::default()
        };
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/module.hcdf", module_bytes.clone())
            .unwrap();
        resolver
            .insert_asset("/mem/asset.bin", asset_bytes.clone())
            .unwrap();
        let projected = load_projected_document_set_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: exact_limits,
                ..DocumentSetOptions::default()
            },
        )
        .unwrap();
        assert_eq!(projected.work().dependency_sites, 3);
        assert_eq!(projected.work().asset_sites, 3);
        assert_eq!(projected.work().resolver_calls, 2);
        assert_eq!(
            projected.work().resolver_bytes,
            module_bytes.len() + asset_bytes.len()
        );

        let document_calls = std::cell::Cell::new(0);
        let asset_calls = std::cell::Cell::new(0);
        let error = {
            let module_bytes = module_bytes.clone();
            let mut resolver = CallbackDocumentResolver::new(
                |_: &DocumentResourceKey,
                 _: &str,
                 _: DocumentDependencyKind|
                 -> Result<ResolvedDocumentResource, ResolverFailure> {
                    document_calls.set(document_calls.get() + 1);
                    Ok(ResolvedDocumentResource {
                        key: DocumentResourceKey::new("/mem/module.hcdf").unwrap(),
                        bytes: module_bytes.clone(),
                    })
                },
                |_: &DocumentResourceKey,
                 _: &str|
                 -> Result<ResolvedAssetResource, ResolverFailure> {
                    asset_calls.set(asset_calls.get() + 1);
                    Ok(ResolvedAssetResource {
                        key: AssetResourceKey::new("/mem/asset.bin").unwrap(),
                        bytes: asset_bytes.clone(),
                    })
                },
            );
            load_projected_document_set_from_model(
                root.clone(),
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver,
                DocumentSetOptions {
                    limits: DocumentSetLimits {
                        max_asset_sites: 2,
                        ..DocumentSetLimits::default()
                    },
                    ..DocumentSetOptions::default()
                },
            )
            .unwrap_err()
        };
        assert_limit_kind(error, DocumentSetLimitKind::AssetSites);
        assert_eq!(document_calls.get(), 1);
        assert_eq!(asset_calls.get(), 0);

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/module.hcdf", module_bytes.clone())
            .unwrap();
        resolver
            .insert_asset("/mem/asset.bin", asset_bytes.clone())
            .unwrap();
        let error = load_projected_document_set_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_resolver_calls: 1,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::ResolverCalls);

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/module.hcdf", module_bytes.clone())
            .unwrap();
        resolver
            .insert_asset("/mem/asset.bin", asset_bytes.clone())
            .unwrap();
        let error = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_resolver_bytes: module_bytes.len() + asset_bytes.len() - 1,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::ResolverBytes);

        let mut cached_failure = single_asset_document("root", "missing.bin", None);
        push_visual_asset(&mut cached_failure, "again", "missing.bin", None);
        let projected = load_projected_document_set_from_model(
            cached_failure,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_resolver_calls: 1,
                    max_resolver_bytes: 0,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap();
        assert_eq!(projected.work().asset_sites, 2);
        assert_eq!(projected.work().resolver_calls, 1);
        assert_eq!(projected.work().resolver_bytes, 0);
    }

    #[test]
    fn asset_discovery_covers_every_opaque_family_and_preserves_resolver_inputs() {
        let module = single_asset_document("module", "module.glb", None);
        let module_bytes = module.to_xml_string().unwrap().into_bytes();
        let mut root = asset_document("root");
        root.include = vec![include("module.hcdf", Some("module"))];
        let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let asset_requests = std::rc::Rc::clone(&requests);
        let mut resolver = CallbackDocumentResolver::new(
            move |parent: &DocumentResourceKey,
                  uri: &str,
                  kind: DocumentDependencyKind|
                  -> Result<ResolvedDocumentResource, ResolverFailure> {
                assert_eq!(parent.as_str(), "/mem/root.hcdf");
                assert_eq!(uri, "module.hcdf");
                assert_eq!(kind, DocumentDependencyKind::HcdfInclude);
                Ok(ResolvedDocumentResource {
                    key: DocumentResourceKey::new("/mem/module.hcdf").unwrap(),
                    bytes: module_bytes.clone(),
                })
            },
            move |parent: &DocumentResourceKey,
                  uri: &str|
                  -> Result<ResolvedAssetResource, ResolverFailure> {
                asset_requests
                    .borrow_mut()
                    .push((parent.clone(), uri.to_owned()));
                Err(ResolverFailure::not_found("not present"))
            },
        );
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let requests = requests.borrow();
        assert_eq!(requests.len(), 6);
        assert_eq!(
            requests
                .iter()
                .map(|(parent, uri)| (parent.as_str(), uri.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("/mem/root.hcdf", "assets/visual.glb"),
                ("/mem/root.hcdf", "assets/collision.stl"),
                ("/mem/root.hcdf", "assets/sensor.stl"),
                ("/mem/root.hcdf", "assets/hmi.stl"),
                ("/mem/root.hcdf", "assets/connector.glb"),
                ("/mem/module.hcdf", "module.glb"),
            ]
        );
        assert!(requests.iter().all(|(_, uri)| uri != "module.hcdf"));
        assert_eq!(projected.work().dependency_sites, 1);
        assert_eq!(projected.work().asset_sites, 6);
        assert!(projected.assets().resources().is_empty());
        assert!(matches!(
            projected.assets().sites()[0].site,
            ResourceSite::ComponentVisualModel { .. }
        ));
        assert!(matches!(
            projected.assets().sites()[1].site,
            ResourceSite::ComponentCollisionMesh { .. }
        ));
        assert!(matches!(
            projected.assets().sites()[2].site,
            ResourceSite::SensorGeometryMesh { .. }
        ));
        assert!(matches!(
            projected.assets().sites()[3].site,
            ResourceSite::HmiGeometryMesh { .. }
        ));
        assert!(matches!(
            projected.assets().sites()[4].site,
            ResourceSite::ConnectivityModel { .. }
        ));
    }

    #[test]
    fn structural_failure_precedes_asset_validation_and_resolution() {
        let root_key = DocumentResourceKey::new("/mem/root.hcdf").unwrap();
        let mut tree = load_document_tree_from_model(
            single_asset_document("root", " \t ", None),
            DocumentIdentity::new("robot").unwrap(),
            root_key.clone(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        let DocumentSourceContent::Hcdf(document) =
            &mut tree.sources.get_mut(&root_key).unwrap().content
        else {
            panic!("expected parsed root source");
        };
        document.include.push(include("late.hcdf", Some("late")));
        let calls = std::cell::Cell::new(0);
        let error = {
            let mut resolver = CallbackDocumentResolver::new(
                |_: &DocumentResourceKey,
                 _: &str,
                 _: DocumentDependencyKind|
                 -> Result<ResolvedDocumentResource, ResolverFailure> {
                    unreachable!("projection must not reload documents")
                },
                |_: &DocumentResourceKey,
                 _: &str|
                 -> Result<ResolvedAssetResource, ResolverFailure> {
                    calls.set(calls.get() + 1);
                    unreachable!("structural failure must precede asset resolution")
                },
            );
            project_document_tree(tree, &mut resolver).unwrap_err()
        };
        assert_eq!(calls.get(), 0);
        assert_eq!(error.code(), "E_DOC_RESOURCE_STRUCTURAL_FLATTEN");
        assert!(matches!(error, DocumentSetError::StructuralFlatten { .. }));
    }

    #[test]
    fn root_asset_uris_match_direct_flatten_and_remain_separate_from_inventory() {
        let mut root = asset_document("root");
        let crate::model::VisualAppearance::Model { model, .. } =
            &mut root.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        model.sha = None;
        let mut direct = root.clone();
        flatten_with(&mut direct, Path::new("/mem"), &mut |key, _| {
            Err(format!("unexpected document load for {key:?}"))
        })
        .unwrap();

        let mut resolver = MemoryDocumentResolver::new();
        let resolved_key = resolver
            .insert_asset("/mem/assets/visual.glb", b"visual".to_vec())
            .unwrap();
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut resolver).unwrap();

        assert_eq!(projected.flattened, direct);
        assert_eq!(
            component_asset_uris(&projected.flattened.comp[0]),
            [
                "assets/visual.glb",
                "assets/collision.stl",
                "assets/sensor.stl",
                "assets/hmi.stl",
                "assets/connector.glb",
            ]
        );
        assert_eq!(projected.assets().resources().len(), 1);
        assert_eq!(
            projected.assets().resource(&resolved_key).unwrap().bytes,
            b"visual"
        );
        assert_eq!(projected.assets().sites().len(), 5);
    }

    #[test]
    fn included_asset_families_reroot_with_direct_flatten_parity() {
        let module = asset_document("module");
        let root = root_with(vec![include("/mem/modules/module.hcdf", Some("module"))]);
        let mut direct = root.clone();
        let module_for_direct = module.clone();
        flatten_with(&mut direct, Path::new("/"), &mut |key, _| {
            if key == "/mem/modules/module.hcdf" {
                Ok((module_for_direct.clone(), None))
            } else {
                Err(format!("unexpected document load for {key:?}"))
            }
        })
        .unwrap();

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/modules/module.hcdf",
                module.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();

        assert_eq!(projected.flattened, direct);
        assert_eq!(
            component_asset_uris(&projected.flattened.comp[0]),
            [
                "/mem/modules/assets/visual.glb",
                "/mem/modules/assets/collision.stl",
                "/mem/modules/assets/sensor.stl",
                "/mem/modules/assets/hmi.stl",
                "/mem/modules/assets/connector.glb",
            ]
        );
    }

    #[test]
    fn relative_document_keys_restore_asset_uris_without_synthetic_rerooting() {
        let module = asset_document("module");
        let root = root_with(vec![
            include("modules/module.hcdf", Some("left")),
            include("modules/module.hcdf", Some("right")),
        ]);
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "modules/module.hcdf",
                module.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();

        for component_name in ["left/body", "right/body"] {
            let component = projected
                .flattened
                .comp
                .iter()
                .find(|component| component.name == component_name)
                .unwrap();
            assert_eq!(
                component_asset_uris(component),
                [
                    "modules/assets/visual.glb",
                    "modules/assets/collision.stl",
                    "modules/assets/sensor.stl",
                    "modules/assets/hmi.stl",
                    "modules/assets/connector.glb",
                ]
            );
            let crate::model::VisualAppearance::Model { model, .. } =
                &component.visual[0].appearance
            else {
                panic!("expected model-backed visual");
            };
            assert_eq!(model.sha.as_deref(), Some("sha256:visual"));
        }
        let xml = projected.flattened.to_xml_string().unwrap();
        assert!(!xml.contains("__hcdf_document_set"));
        assert!(!xml.contains("hcdf-asset-guard:"));
    }

    #[test]
    fn asset_guard_namespace_collision_advances_and_restores_exactly() {
        let candidate_namespace = "hcdf-asset-guard:fixed_0000000000000000_";
        let authored_candidate = format!("{candidate_namespace}0000000000000000");
        let mut document = asset_document("root");
        let crate::model::VisualAppearance::Model { model, .. } =
            &mut document.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        model.uri = Some(authored_candidate.clone());

        let tree = load_document_tree_from_model(
            document.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        let material = ProjectionGuardNamespaceMaterial {
            canonical_text: Vec::new(),
            seed: "fixed".to_owned(),
        };
        let namespace = material
            .reserve(
                &tree,
                "hcdf-asset-guard:",
                "resolved asset URI namespace space exhausted",
            )
            .unwrap();
        assert_eq!(namespace, "hcdf-asset-guard:fixed_0000000000000001_");

        let mut guards = GuardedResolvedAssetUris::new(namespace);
        let protected = guards
            .replace(
                "modules/assets/collision.stl".to_owned(),
                Some("sha256:collision".to_owned()),
            )
            .unwrap();
        let collision = document.comp[0].collision[0]
            .geometry
            .as_mut()
            .unwrap()
            .mesh
            .as_mut()
            .unwrap();
        collision.uri = Some(protected);
        collision.sha = None;
        guards.restore(&mut document).unwrap();

        assert_eq!(
            component_asset_uris(&document.comp[0])[0],
            authored_candidate
        );
        assert_eq!(
            component_asset_uris(&document.comp[0])[1],
            "modules/assets/collision.stl"
        );
        assert_eq!(
            document.comp[0].collision[0]
                .geometry
                .as_ref()
                .unwrap()
                .mesh
                .as_ref()
                .unwrap()
                .sha
                .as_deref(),
            Some("sha256:collision")
        );
    }

    #[test]
    fn asset_guard_namespace_reservation_scans_every_source_representation() {
        let candidate_namespace = "hcdf-asset-guard:fixed_0000000000000000_";
        let expected_namespace = "hcdf-asset-guard:fixed_0000000000000001_";

        let root_key = DocumentResourceKey::new("root.hcdf").unwrap();
        let mut raw_tree = load_document_tree_from_model(
            root_with(Vec::new()),
            DocumentIdentity::new("robot").unwrap(),
            root_key.clone(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        raw_tree
            .sources
            .get_mut(&root_key)
            .unwrap()
            .bytes
            .extend_from_slice(candidate_namespace.as_bytes());
        let raw_material = ProjectionGuardNamespaceMaterial::new(&raw_tree).unwrap();
        assert!(raw_material
            .canonical_text
            .iter()
            .all(|text| !text.contains(candidate_namespace)));
        assert_eq!(
            reserve_fixed_asset_guard_namespace(&raw_tree),
            expected_namespace
        );

        let authored_guard = format!("{candidate_namespace}0000000000000000");
        let encoded_guard = authored_guard.replacen(':', "&#58;", 1);
        let canonical_bytes = asset_document("root")
            .to_xml_string()
            .unwrap()
            .replacen("assets/visual.glb", &encoded_guard, 1)
            .into_bytes();
        assert!(!std::str::from_utf8(&canonical_bytes)
            .unwrap()
            .contains(candidate_namespace));
        let canonical_tree = load_document_tree_from_bytes(
            canonical_bytes,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        let canonical_material = ProjectionGuardNamespaceMaterial::new(&canonical_tree).unwrap();
        assert!(canonical_material
            .canonical_text
            .iter()
            .any(|text| text.contains(candidate_namespace)));
        assert_eq!(
            reserve_fixed_asset_guard_namespace(&canonical_tree),
            expected_namespace
        );

        let key = format!("{candidate_namespace}root.hcdf");
        let key_tree = load_document_tree_from_model(
            asset_document("root"),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new(key).unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert!(key_tree.sources.values().all(|source| !source
            .bytes
            .windows(candidate_namespace.len())
            .any(|window| { window == candidate_namespace.as_bytes() })));
        assert!(ProjectionGuardNamespaceMaterial::new(&key_tree)
            .unwrap()
            .canonical_text
            .iter()
            .all(|text| !text.contains(candidate_namespace)));
        assert_eq!(
            reserve_fixed_asset_guard_namespace(&key_tree),
            expected_namespace
        );
    }

    #[test]
    fn asset_guard_restore_failures_are_transactional() {
        let namespace = "hcdf-asset-guard:fixed_0000000000000000_".to_owned();

        let mut missing_document = asset_document("root");
        let missing_before = missing_document.clone();
        let mut missing_guards = GuardedResolvedAssetUris::new(namespace.clone());
        missing_guards
            .replace("modules/assets/missing.glb".to_owned(), None)
            .unwrap();
        let error = missing_guards.restore(&mut missing_document).unwrap_err();
        assert!(error.contains("restored 0 of 1 resolved asset URIs"));
        assert_eq!(missing_document, missing_before);

        let mut duplicate_document = asset_document("root");
        let mut duplicate_guards = GuardedResolvedAssetUris::new(namespace);
        let duplicate_guard = duplicate_guards
            .replace(
                "modules/assets/shared.glb".to_owned(),
                Some("sha256:shared".to_owned()),
            )
            .unwrap();
        let crate::model::VisualAppearance::Model { model, .. } =
            &mut duplicate_document.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        model.uri = Some(duplicate_guard.clone());
        let collision = duplicate_document.comp[0].collision[0]
            .geometry
            .as_mut()
            .unwrap()
            .mesh
            .as_mut()
            .unwrap();
        collision.uri = Some(duplicate_guard);
        let duplicate_before = duplicate_document.clone();
        let error = duplicate_guards
            .restore(&mut duplicate_document)
            .unwrap_err();
        assert!(error.contains("appeared more than once"));
        assert_eq!(duplicate_document, duplicate_before);
    }

    #[test]
    fn root_guard_looking_asset_uri_remains_byte_exact() {
        let authored_uri =
            "hcdf-asset-guard:authored_0000000000000000_0000000000000000?x=/../y#z/../w";
        let authored_sha = "sha256:authored";
        let mut root = asset_document("root");
        let crate::model::VisualAppearance::Model { model, .. } =
            &mut root.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        model.uri = Some(authored_uri.to_owned());
        model.sha = Some(authored_sha.to_owned());
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        let crate::model::VisualAppearance::Model { model, .. } =
            &projected.flattened.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        assert_eq!(model.uri.as_deref(), Some(authored_uri));
        assert_eq!(model.sha.as_deref(), Some(authored_sha));
    }

    #[test]
    fn included_whitespace_only_asset_uri_fails_without_synthetic_leakage() {
        let mut module = asset_document("module");
        let crate::model::VisualAppearance::Model { model, .. } =
            &mut module.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        model.uri = Some(" \t ".to_owned());
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "modules/module.hcdf",
                module.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let tree = load_document_tree_from_model(
            root_with(vec![include("modules/module.hcdf", Some("module"))]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let retained_tree = tree.clone();
        let error = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("resource URI must not be empty"),
            "{message}"
        );
        assert!(!message.contains("__hcdf_document_set"), "{message}");
        let source =
            &retained_tree.sources[&DocumentResourceKey::new("modules/module.hcdf").unwrap()];
        let source_document = source.content.hcdf().unwrap();
        let crate::model::VisualAppearance::Model { model, .. } =
            &source_document.comp[0].visual[0].appearance
        else {
            panic!("expected model-backed visual");
        };
        assert_eq!(model.uri.as_deref(), Some(" \t "));
    }

    #[test]
    fn parse_digest_cycle_and_limit_failures_are_typed() {
        let mut parse_resolver = MemoryDocumentResolver::new();
        parse_resolver
            .insert_document("/mem/bad.hcdf", b"not xml".to_vec())
            .unwrap();
        let error = load_document_tree_from_model(
            root_with(vec![include("/mem/bad.hcdf", None)]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut parse_resolver,
            strict_options(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "E_DOC_RESOURCE_DOCUMENT_PARSE");

        let mut digest_resolver = MemoryDocumentResolver::new();
        digest_resolver
            .insert_document("/mem/module.hcdf", module("module", &[]))
            .unwrap();
        let mut pinned = include("/mem/module.hcdf", None);
        pinned.sha = Some("sha256:bad".to_owned());
        let error = load_document_tree_from_model(
            root_with(vec![pinned]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut digest_resolver,
            strict_options(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "E_DOC_RESOURCE_DOCUMENT_DIGEST");

        let mut cycle_resolver = MemoryDocumentResolver::new();
        cycle_resolver
            .insert_document(
                "/mem/a.hcdf",
                module("a", &[("/mem/root.hcdf", Some("back"))]),
            )
            .unwrap();
        let root = root_with(vec![include("/mem/a.hcdf", Some("a"))]);
        cycle_resolver
            .insert_document("/mem/root.hcdf", root.to_xml_string().unwrap().into_bytes())
            .unwrap();
        let error = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut cycle_resolver,
            DocumentSetOptions::default(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "E_DOC_RESOURCE_INCLUDE_CYCLE");

        let mut limited = MemoryDocumentResolver::new();
        limited
            .insert_document("/mem/module.hcdf", module("module", &[]))
            .unwrap();
        let options = DocumentSetOptions {
            limits: DocumentSetLimits {
                max_instances: 1,
                ..DocumentSetLimits::default()
            },
            ..DocumentSetOptions::default()
        };
        let error = load_document_tree_from_model(
            root_with(vec![include("/mem/module.hcdf", None)]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut limited,
            options,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DocumentSetError::LimitExceeded {
                kind: DocumentSetLimitKind::Instances,
                ..
            }
        ));
    }

    #[test]
    fn raw_root_bytes_and_model_entry_report_exact_provenance() {
        assert_eq!(
            DocumentSetOptions::default().document_policy,
            DocumentLoadPolicy::EditorCompatible
        );

        let raw = br#"<?xml version="1.0"?>
<hcdf  name="raw-root" version="1.0">

</hcdf>
"#
        .to_vec();
        let expected_sha = content_sha(&raw);
        let mut resolver = MemoryDocumentResolver::new();
        let tree = load_document_tree_from_bytes(
            raw.clone(),
            DocumentIdentity::new("raw").unwrap(),
            DocumentResourceKey::new("/mem/raw.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let source = &tree.sources[&tree.root];
        assert_eq!(source.bytes, raw);
        assert_eq!(source.source_sha, expected_sha);
        assert_eq!(source.provenance, DocumentSourceProvenance::RawRootBytes);
        assert_eq!(source.content.hcdf().unwrap().name, "raw-root");

        let model = root_with(Vec::new());
        let canonical = model.to_xml_string().unwrap().into_bytes();
        let tree = load_document_tree_from_model(
            model,
            DocumentIdentity::new("model").unwrap(),
            DocumentResourceKey::new("/mem/model.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let source = &tree.sources[&tree.root];
        assert_eq!(source.bytes, canonical);
        assert_eq!(
            source.provenance,
            DocumentSourceProvenance::CanonicalModelSerialization
        );
    }

    #[test]
    fn editor_and_strict_resolution_dispositions_are_explicit() {
        for kind in [
            ResolverFailureKind::NotFound,
            ResolverFailureKind::Unsupported,
            ResolverFailureKind::Other,
        ] {
            let mut resolver = CallbackDocumentResolver::new(
                move |_: &DocumentResourceKey,
                      _: &str,
                      _: DocumentDependencyKind|
                      -> Result<ResolvedDocumentResource, ResolverFailure> {
                    Err(ResolverFailure::new(kind, "expected editor failure"))
                },
                |_: &DocumentResourceKey,
                 _: &str|
                 -> Result<ResolvedAssetResource, ResolverFailure> {
                    unreachable!("document loading does not resolve assets")
                },
            );
            let tree = load_document_tree_from_model(
                root_with(vec![include("missing.hcdf", None)]),
                DocumentIdentity::new("editor").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver,
                DocumentSetOptions::default(),
            )
            .unwrap();
            let site = &tree.instances[&IncludeInstanceId::root()].include_sites[0];
            assert_eq!(site.outcome, DocumentIncludeSiteOutcome::RetainedUnresolved);
            assert!(matches!(
                site.diagnostics.as_slice(),
                [DocumentDependencyDiagnostic::ResolutionFailure { failure }]
                    if failure.kind == kind
            ));
        }

        let mut missing = MemoryDocumentResolver::new();
        let error = load_document_tree_from_model(
            root_with(vec![include("missing.hcdf", None)]),
            DocumentIdentity::new("strict").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut missing,
            strict_options(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DocumentSetError::DocumentResolution {
                failure: ResolverFailure {
                    kind: ResolverFailureKind::NotFound,
                    ..
                },
                ..
            }
        ));

        for kind in [
            ResolverFailureKind::Denied,
            ResolverFailureKind::InvalidReference,
        ] {
            let mut resolver = CallbackDocumentResolver::new(
                move |_: &DocumentResourceKey,
                      _: &str,
                      _: DocumentDependencyKind|
                      -> Result<ResolvedDocumentResource, ResolverFailure> {
                    Err(ResolverFailure::new(kind, "hard failure"))
                },
                |_: &DocumentResourceKey,
                 _: &str|
                 -> Result<ResolvedAssetResource, ResolverFailure> {
                    unreachable!("document loading does not resolve assets")
                },
            );
            let error = load_document_tree_from_model(
                root_with(vec![include("blocked.hcdf", None)]),
                DocumentIdentity::new("hard").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver,
                DocumentSetOptions::default(),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                DocumentSetError::DocumentResolution { failure, .. }
                    if failure.kind == kind
            ));
        }

        let calls = std::cell::Cell::new(0);
        let mut resolver = CallbackDocumentResolver::new(
            |_: &DocumentResourceKey,
             _: &str,
             _: DocumentDependencyKind|
             -> Result<ResolvedDocumentResource, ResolverFailure> {
                calls.set(calls.get() + 1);
                Err(ResolverFailure::not_found("must not be called"))
            },
            |_: &DocumentResourceKey, _: &str| -> Result<ResolvedAssetResource, ResolverFailure> {
                unreachable!("document loading does not resolve assets")
            },
        );
        let error = load_document_tree_from_model(
            root_with(vec![include("   ", None)]),
            DocumentIdentity::new("blank").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(error, DocumentSetError::MissingIncludeUri { .. }));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn unresolved_editor_sites_do_not_consume_tree_depth() {
        let depth_zero = DocumentSetOptions {
            limits: DocumentSetLimits {
                max_depth: 0,
                ..DocumentSetLimits::default()
            },
            ..DocumentSetOptions::default()
        };
        let mut missing = MemoryDocumentResolver::new();
        let tree = load_document_tree_from_model(
            root_with(vec![include("missing.hcdf", None)]),
            DocumentIdentity::new("editor").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut missing,
            depth_zero,
        )
        .unwrap();
        assert_eq!(tree.instances.len(), 1);
        assert_eq!(
            tree.instances[&IncludeInstanceId::root()].include_sites[0].outcome,
            DocumentIncludeSiteOutcome::RetainedUnresolved
        );

        let mut resolved = MemoryDocumentResolver::new();
        resolved
            .insert_document("/mem/child.hcdf", module("child", &[]))
            .unwrap();
        let error = load_document_tree_from_model(
            root_with(vec![include("/mem/child.hcdf", None)]),
            DocumentIdentity::new("resolved").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolved,
            depth_zero,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DocumentSetError::LimitExceeded {
                kind: DocumentSetLimitKind::Depth,
                ..
            }
        ));
    }

    #[test]
    fn editor_retains_malformed_children_while_strict_rejects_them() {
        for (bytes, encoding_failure) in [(vec![0xff], true), (b"not xml".to_vec(), false)] {
            let mut resolver = MemoryDocumentResolver::new();
            resolver.insert_document("/mem/bad.hcdf", bytes).unwrap();
            let mut editor_resolver = resolver.clone();
            let tree = load_document_tree_from_model(
                root_with(vec![include("/mem/bad.hcdf", None)]),
                DocumentIdentity::new("editor").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut editor_resolver,
                DocumentSetOptions::default(),
            )
            .unwrap();
            assert_eq!(tree.instances.len(), 1);
            let site = &tree.instances[&IncludeInstanceId::root()].include_sites[0];
            assert_eq!(site.outcome, DocumentIncludeSiteOutcome::RetainedUnresolved);
            assert_eq!(site.diagnostics.len(), 1);
            if encoding_failure {
                assert!(matches!(
                    site.diagnostics[0],
                    DocumentDependencyDiagnostic::ContentFailure {
                        failure: DocumentContentFailure::Encoding { .. },
                        ..
                    }
                ));
            } else {
                assert!(matches!(
                    site.diagnostics[0],
                    DocumentDependencyDiagnostic::ContentFailure {
                        failure: DocumentContentFailure::Parse { .. },
                        ..
                    }
                ));
            }

            let error = load_document_tree_from_model(
                root_with(vec![include("/mem/bad.hcdf", None)]),
                DocumentIdentity::new("strict").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver,
                strict_options(),
            )
            .unwrap_err();
            if encoding_failure {
                assert!(matches!(error, DocumentSetError::DocumentEncoding { .. }));
            } else {
                assert!(matches!(error, DocumentSetError::DocumentParse { .. }));
            }
        }
    }

    #[test]
    fn editor_digest_mismatch_loads_child_but_invalidates_connectivity() {
        let child_bytes = module("child", &[]);
        let actual_sha = content_sha(&child_bytes);
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/child.hcdf", child_bytes.clone())
            .unwrap();
        let mut pinned = include("/mem/child.hcdf", Some("child"));
        pinned.sha = Some("sha256:wrong".to_owned());
        let root = root_with(vec![pinned.clone()]);
        let tree = load_document_tree_from_model(
            root.clone(),
            DocumentIdentity::new("editor").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let child_id = IncludeInstanceId::root().named_child("child", 0);
        let site = &tree.instances[&IncludeInstanceId::root()].include_sites[0];
        assert_eq!(
            site.outcome,
            DocumentIncludeSiteOutcome::Resolved {
                child: child_id.clone()
            }
        );
        assert!(tree.instances.contains_key(&child_id));
        assert!(matches!(
            site.diagnostics.as_slice(),
            [DocumentDependencyDiagnostic::DigestMismatch { expected, actual }]
                if expected == "sha256:wrong" && actual == &actual_sha
        ));
        let flattened = flatten_document_tree(&tree).unwrap();
        assert!(flattened.include.is_empty());

        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        assert!(matches!(
            projected.connectivity,
            ConnectivityProjection::Invalid { ref issues }
                if matches!(
                    issues.as_slice(),
                    [ProjectedConnectivityIssue::Dependency {
                        diagnostic: DocumentDependencyDiagnostic::DigestMismatch { .. },
                        ..
                    }]
                )
        ));

        let mut strict_resolver = MemoryDocumentResolver::new();
        strict_resolver
            .insert_document("/mem/child.hcdf", child_bytes)
            .unwrap();
        let error = load_document_tree_from_model(
            root,
            DocumentIdentity::new("strict").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut strict_resolver,
            strict_options(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            DocumentSetError::DocumentDigestMismatch { .. }
        ));
    }

    #[test]
    fn mixed_outcomes_match_editor_flatten_and_cannot_collide_with_synthetic_keys() {
        let child_bytes = module("child", &[]);
        let child = Hcdf::from_xml_str(std::str::from_utf8(&child_bytes).unwrap()).unwrap();

        let resolved = include("/mem/child.hcdf", Some("loaded"));
        let mut collision = include("/__hcdf_document_set/0.hcdf", Some("collision"));
        collision.sha = Some("sha256:keep-exact".to_owned());
        collision.pose = Some("1 2 3 0 0 0".to_owned());
        collision.static_ = Some("true".to_owned());
        collision.placement_frame = Some("mount".to_owned());
        let relative_missing = include("missing.hcdf", Some("relative"));
        let mut root = root_with(vec![resolved, collision.clone(), relative_missing.clone()]);
        let reference_fixture = Hcdf::from_xml_str(
            r#"<hcdf name="references" version="1.0">
              <comp name="local"><port name="p"/></comp>
              <link name="retained-reference">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="remote"><endpoint><port-ref component="remote-device" port="p">
                  <instance><segment name="collision" occurrence="0"/></instance>
                </port-ref></endpoint></participant>
                <participant name="local"><endpoint><port-ref component="local" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#,
        )
        .unwrap();
        root.comp = reference_fixture.comp;
        root.link = reference_fixture.link;

        let mut expected = root.clone();
        let mut direct_loader = |key: &str, _base: &Path| {
            if key == "/mem/child.hcdf" {
                Ok((child.clone(), None))
            } else {
                Err(format!("{key} is missing"))
            }
        };
        flatten_with(&mut expected, Path::new("/"), &mut direct_loader).unwrap();

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/child.hcdf", child_bytes)
            .unwrap();
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("mixed").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let sites = &tree.instances[&IncludeInstanceId::root()].include_sites;
        assert_eq!(sites.len(), 3);
        assert!(matches!(
            sites[0].outcome,
            DocumentIncludeSiteOutcome::Resolved { .. }
        ));
        assert_eq!(
            sites[1].outcome,
            DocumentIncludeSiteOutcome::RetainedUnresolved
        );
        assert_eq!(
            sites[2].outcome,
            DocumentIncludeSiteOutcome::RetainedUnresolved
        );

        let flattened = flatten_document_tree(&tree).unwrap();
        assert_eq!(flattened, expected);
        assert_eq!(flattened.include, vec![collision, relative_missing]);

        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        assert!(matches!(
            projected.connectivity,
            ConnectivityProjection::Invalid { ref issues } if issues.len() == 2
        ));
        assert!(projected.connectivity.graph().is_none());
    }

    #[test]
    fn exact_resolution_cache_deduplicates_successes_and_failures_per_request() {
        let child_bytes = module("child", &[]);
        let child_sha = content_sha(&child_bytes);
        let calls = std::cell::Cell::new(0);
        let mut resolver = CallbackDocumentResolver::new(
            |_: &DocumentResourceKey,
             _: &str,
             _: DocumentDependencyKind|
             -> Result<ResolvedDocumentResource, ResolverFailure> {
                calls.set(calls.get() + 1);
                Ok(ResolvedDocumentResource {
                    key: DocumentResourceKey::new("/mem/child.hcdf").unwrap(),
                    bytes: child_bytes.clone(),
                })
            },
            |_: &DocumentResourceKey, _: &str| -> Result<ResolvedAssetResource, ResolverFailure> {
                unreachable!("document loading does not resolve assets")
            },
        );
        let mut first = include("child.hcdf", None);
        first.sha = Some(child_sha);
        let mut second = include("child.hcdf", None);
        second.sha = Some("sha256:wrong".to_owned());
        let tree = load_document_tree_from_model(
            root_with(vec![first, second]),
            DocumentIdentity::new("cached").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(tree.work.dependency_sites, 2);
        assert_eq!(tree.work.resolver_calls, 1);
        assert_eq!(tree.work.resolver_bytes, child_bytes.len());
        assert_eq!(tree.sources.len(), 2);
        assert_eq!(tree.instances.len(), 3);
        let sites = &tree.instances[&IncludeInstanceId::root()].include_sites;
        assert!(sites[0].diagnostics.is_empty());
        assert!(matches!(
            sites[1].diagnostics.as_slice(),
            [DocumentDependencyDiagnostic::DigestMismatch { .. }]
        ));

        let failed_calls = std::cell::Cell::new(0);
        let mut failed = CallbackDocumentResolver::new(
            |_: &DocumentResourceKey,
             _: &str,
             _: DocumentDependencyKind|
             -> Result<ResolvedDocumentResource, ResolverFailure> {
                failed_calls.set(failed_calls.get() + 1);
                Err(ResolverFailure::not_found("cached missing document"))
            },
            |_: &DocumentResourceKey, _: &str| -> Result<ResolvedAssetResource, ResolverFailure> {
                unreachable!("document loading does not resolve assets")
            },
        );
        let tree = load_document_tree_from_model(
            root_with(vec![
                include("missing.hcdf", None),
                include("missing.hcdf", None),
            ]),
            DocumentIdentity::new("cached-failure").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut failed,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(failed_calls.get(), 1);
        assert_eq!(tree.work.resolver_calls, 1);
        assert_eq!(tree.work.resolver_bytes, 0);
        assert_eq!(tree.instances.len(), 1);
        assert!(tree.instances[&IncludeInstanceId::root()]
            .include_sites
            .iter()
            .all(|site| {
                site.outcome == DocumentIncludeSiteOutcome::RetainedUnresolved
                    && matches!(
                        site.diagnostics.as_slice(),
                        [DocumentDependencyDiagnostic::ResolutionFailure { .. }]
                    )
            }));
    }

    #[test]
    fn cache_identity_preserves_exact_authored_uri_and_detects_source_conflicts() {
        let child_bytes = module("child", &[]);
        let calls = std::cell::Cell::new(0);
        let mut resolver = CallbackDocumentResolver::new(
            |_: &DocumentResourceKey,
             _: &str,
             _: DocumentDependencyKind|
             -> Result<ResolvedDocumentResource, ResolverFailure> {
                calls.set(calls.get() + 1);
                Ok(ResolvedDocumentResource {
                    key: DocumentResourceKey::new("/mem/child.hcdf").unwrap(),
                    bytes: child_bytes.clone(),
                })
            },
            |_: &DocumentResourceKey, _: &str| -> Result<ResolvedAssetResource, ResolverFailure> {
                unreachable!("document loading does not resolve assets")
            },
        );
        let tree = load_document_tree_from_model(
            root_with(vec![
                include("child.hcdf", Some("plain")),
                include("./child.hcdf", Some("dotted")),
            ]),
            DocumentIdentity::new("exact").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(tree.work.resolver_calls, 2);
        assert_eq!(tree.work.resolver_bytes, child_bytes.len() * 2);
        assert_eq!(tree.sources.len(), 2);

        let conflict_calls = std::cell::Cell::new(0);
        let first_bytes = module("first", &[]);
        let second_bytes = module("second", &[]);
        let mut conflict = CallbackDocumentResolver::new(
            |_: &DocumentResourceKey,
             uri: &str,
             _: DocumentDependencyKind|
             -> Result<ResolvedDocumentResource, ResolverFailure> {
                conflict_calls.set(conflict_calls.get() + 1);
                Ok(ResolvedDocumentResource {
                    key: DocumentResourceKey::new("/mem/shared.hcdf").unwrap(),
                    bytes: if uri == "first.hcdf" {
                        first_bytes.clone()
                    } else {
                        second_bytes.clone()
                    },
                })
            },
            |_: &DocumentResourceKey, _: &str| -> Result<ResolvedAssetResource, ResolverFailure> {
                unreachable!("document loading does not resolve assets")
            },
        );
        let error = load_document_tree_from_model(
            root_with(vec![
                include("first.hcdf", Some("first")),
                include("second.hcdf", Some("second")),
            ]),
            DocumentIdentity::new("conflict").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut conflict,
            DocumentSetOptions::default(),
        )
        .unwrap_err();
        assert_eq!(conflict_calls.get(), 2);
        assert!(matches!(error, DocumentSetError::SourceConflict { .. }));
    }

    #[test]
    fn document_work_limits_cover_unique_resolution_and_expansion_dimensions() {
        let child_bytes = module("child", &[]);
        let one_child_root = root_with(vec![include("/mem/child.hcdf", None)]);
        let one_child_root_bytes = one_child_root.to_xml_string().unwrap().into_bytes();

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/child.hcdf", child_bytes.clone())
            .unwrap();
        let error = load_document_tree_from_model(
            one_child_root.clone(),
            DocumentIdentity::new("unique").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_unique_documents: 1,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::UniqueDocuments);

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/child.hcdf", child_bytes.clone())
            .unwrap();
        let error = load_document_tree_from_model(
            one_child_root.clone(),
            DocumentIdentity::new("instances").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_instances: 1,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::Instances);

        let empty_root = root_with(Vec::new());
        let empty_root_len = empty_root.to_xml_string().unwrap().len();
        let mut resolver = MemoryDocumentResolver::new();
        let error = load_document_tree_from_model(
            empty_root,
            DocumentIdentity::new("document-bytes").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_document_bytes: empty_root_len - 1,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::DocumentBytes);

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/child.hcdf", child_bytes.clone())
            .unwrap();
        let error = load_document_tree_from_model(
            one_child_root.clone(),
            DocumentIdentity::new("aggregate").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_aggregate_unique_bytes: one_child_root_bytes.len(),
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::AggregateUniqueBytes);

        let mut resolver = MemoryDocumentResolver::new();
        let error = load_document_tree_from_model(
            root_with(vec![
                include("missing.hcdf", None),
                include("missing.hcdf", None),
            ]),
            DocumentIdentity::new("dependency-sites").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_dependency_sites: 1,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::DependencySites);

        let calls = std::cell::Cell::new(0);
        let mut resolver = CallbackDocumentResolver::new(
            |_: &DocumentResourceKey,
             _: &str,
             _: DocumentDependencyKind|
             -> Result<ResolvedDocumentResource, ResolverFailure> {
                calls.set(calls.get() + 1);
                Err(ResolverFailure::not_found("missing"))
            },
            |_: &DocumentResourceKey, _: &str| -> Result<ResolvedAssetResource, ResolverFailure> {
                unreachable!("document loading does not resolve assets")
            },
        );
        let error = load_document_tree_from_model(
            root_with(vec![
                include("first.hcdf", None),
                include("second.hcdf", None),
            ]),
            DocumentIdentity::new("resolver-calls").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_resolver_calls: 1,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(calls.get(), 1);
        assert_limit_kind(error, DocumentSetLimitKind::ResolverCalls);

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/child.hcdf", child_bytes.clone())
            .unwrap();
        let error = load_document_tree_from_model(
            one_child_root,
            DocumentIdentity::new("resolver-bytes").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_resolver_bytes: child_bytes.len() - 1,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::ResolverBytes);

        let repeated_root = root_with(vec![
            include("/mem/child.hcdf", None),
            include("/mem/child.hcdf", None),
        ]);
        let repeated_root_bytes = repeated_root.to_xml_string().unwrap().into_bytes();
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/child.hcdf", child_bytes.clone())
            .unwrap();
        let error = load_document_tree_from_model(
            repeated_root.clone(),
            DocumentIdentity::new("expanded-bytes").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_expanded_source_bytes: repeated_root_bytes.len() + child_bytes.len(),
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::ExpandedSourceBytes);

        let root_elements = count_xml_elements(&repeated_root_bytes);
        let child_elements = count_xml_elements(&child_bytes);
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/child.hcdf", child_bytes)
            .unwrap();
        let error = load_document_tree_from_model(
            repeated_root,
            DocumentIdentity::new("expanded-elements").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions {
                limits: DocumentSetLimits {
                    max_expanded_xml_elements: root_elements + child_elements,
                    ..DocumentSetLimits::default()
                },
                ..DocumentSetOptions::default()
            },
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::ExpandedXmlElements);

        let error = checked_limit_add(
            DocumentSetLimitKind::ResolverBytes,
            usize::MAX,
            1,
            usize::MAX,
            Some("overflow".to_owned()),
        )
        .unwrap_err();
        assert_limit_kind(error, DocumentSetLimitKind::ResolverBytes);
    }

    #[test]
    fn include_names_and_poses_use_exact_authored_boundaries() {
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/module.hcdf", module("module", &[]))
            .unwrap();

        for name in ["   ", "$include"] {
            let tree = load_document_tree_from_model(
                root_with(vec![include("/mem/module.hcdf", Some(name))]),
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver.clone(),
                DocumentSetOptions::default(),
            )
            .unwrap();
            let instance = IncludeInstanceId::root().named_child(name, 0);
            assert_eq!(
                tree.instances[&instance].authored_name.as_deref(),
                Some(name)
            );
            assert_eq!(tree.flatten_prefixes.segment(&instance), Some(Some(name)));
        }

        let mut placed = include("/mem/module.hcdf", Some("placed"));
        for pose in ["", "1\t2\n3", "1 2 3 0 0 1", "1\u{2003}2\u{2009}3"] {
            placed.pose = Some(pose.to_owned());
            let tree = load_document_tree_from_model(
                root_with(vec![placed.clone()]),
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver.clone(),
                DocumentSetOptions::default(),
            )
            .unwrap();
            let instance = IncludeInstanceId::root().named_child("placed", 0);
            assert!(tree.instances[&instance].placement.is_projected());
            assert_eq!(
                tree.sources[&tree.root].content.hcdf().unwrap().include[0]
                    .pose
                    .as_deref(),
                Some(pose)
            );
        }
    }

    #[test]
    fn legacy_pose_forms_keep_compose_parity_and_precise_projection_state() {
        let module_doc = Hcdf::from_xml_str(
            r#"<hcdf name="module" version="1.0">
              <comp name="body"><frame name="origin"/></comp>
            </hcdf>"#,
        )
        .unwrap();
        let cases = vec![
            (
                "1 2",
                Some(UntrustedPoseReason::WrongValueCount { actual: 2 }),
            ),
            (
                "1 nope 3",
                Some(UntrustedPoseReason::InvalidNumber {
                    token_index: 1,
                    token: "nope".to_owned(),
                }),
            ),
            (
                "1 2 3 4 5 6 7",
                Some(UntrustedPoseReason::WrongValueCount { actual: 7 }),
            ),
            (
                "1 2 NaN",
                Some(UntrustedPoseReason::NonFiniteNumber {
                    token_index: 2,
                    token: "NaN".to_owned(),
                }),
            ),
            (
                "1 2 inf",
                Some(UntrustedPoseReason::NonFiniteNumber {
                    token_index: 2,
                    token: "inf".to_owned(),
                }),
            ),
            (
                "   ",
                Some(UntrustedPoseReason::WrongValueCount { actual: 0 }),
            ),
            ("", None),
            ("1\u{2003}2\u{2009}3", None),
        ];

        for (pose, expected_reason) in cases {
            let mut placed = include("/mem/module.hcdf", Some("placed"));
            placed.pose = Some(pose.to_owned());
            let root = root_with(vec![placed]);
            let mut resolver = MemoryDocumentResolver::new();
            resolver
                .insert_document(
                    "/mem/module.hcdf",
                    module_doc.to_xml_string().unwrap().into_bytes(),
                )
                .unwrap();
            let tree = load_document_tree_from_model(
                root.clone(),
                DocumentIdentity::new("robot").unwrap(),
                DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
                &mut resolver,
                DocumentSetOptions::default(),
            )
            .unwrap();
            let instance = IncludeInstanceId::root().named_child("placed", 0);
            assert_eq!(
                tree.sources[&tree.root].content.hcdf().unwrap().include[0]
                    .pose
                    .as_deref(),
                Some(pose)
            );

            let projection_complete = expected_reason.is_none();
            if let Some(reason) = expected_reason {
                assert_eq!(
                    tree.instances[&instance].placement,
                    PlacementState::Unprojectable {
                        ancestry: vec![UnprojectablePlacementStep {
                            instance: instance.clone(),
                            authored_pose: Some(pose.to_owned()),
                            placement_frame: None,
                            causes: vec![PlacementProjectionCause::UntrustedPose(reason)],
                        }],
                    },
                    "pose {pose:?}"
                );
            } else {
                assert!(
                    tree.instances[&instance].placement.is_projected(),
                    "pose {pose:?}"
                );
            }

            let document_set_flattened = flatten_document_tree(&tree).unwrap();
            let mut compose_flattened = root;
            let module_for_compose = module_doc.clone();
            let mut compose_loader = move |key: &str, _: &Path| {
                assert_eq!(key, "/mem/module.hcdf");
                Ok((module_for_compose.clone(), None))
            };
            flatten_with(
                &mut compose_flattened,
                Path::new("/mem"),
                &mut compose_loader,
            )
            .unwrap();
            let compose_xml = compose_flattened.to_xml_string().unwrap();
            assert_eq!(
                document_set_flattened.to_xml_string().unwrap(),
                compose_xml,
                "pose {pose:?}"
            );

            let projected =
                project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
            assert_eq!(
                projected
                    .structural_projection
                    .placement_projection_complete,
                projection_complete,
                "pose {pose:?}"
            );
            assert_eq!(
                projected.structural_projection.components[0]
                    .placement
                    .is_projected(),
                projection_complete,
                "pose {pose:?}"
            );
            assert_eq!(
                projected.flattened.to_xml_string().unwrap(),
                compose_xml,
                "pose {pose:?}"
            );
        }
    }

    #[test]
    fn untrusted_pose_ancestry_propagates_without_changing_nested_compose_output() {
        let leaf = Hcdf::from_xml_str(
            r#"<hcdf name="leaf" version="1.0">
              <comp name="leaf-body"><frame name="origin"/></comp>
            </hcdf>"#,
        )
        .unwrap();
        let middle = Hcdf::from_xml_str(
            r#"<hcdf name="middle" version="1.0">
              <comp name="middle-body"><frame name="origin"/></comp>
              <include uri="/mem/leaf.hcdf" name="leaf" pose="0 2 0"/>
            </hcdf>"#,
        )
        .unwrap();
        let mut outer = include("/mem/middle.hcdf", Some("middle"));
        outer.pose = Some("1 nope 3".to_owned());
        let root = root_with(vec![outer]);

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/middle.hcdf",
                middle.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        resolver
            .insert_document("/mem/leaf.hcdf", leaf.to_xml_string().unwrap().into_bytes())
            .unwrap();
        let tree = load_document_tree_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let middle_id = IncludeInstanceId::root().named_child("middle", 0);
        let leaf_id = middle_id.named_child("leaf", 0);
        let PlacementState::Unprojectable { ancestry } = &tree.instances[&leaf_id].placement else {
            panic!("untrusted ancestor must keep descendant placement unprojectable");
        };
        assert_eq!(ancestry.len(), 2);
        assert_eq!(ancestry[0].instance, middle_id);
        assert_eq!(ancestry[0].authored_pose.as_deref(), Some("1 nope 3"));
        assert_eq!(
            ancestry[0].causes,
            vec![PlacementProjectionCause::UntrustedPose(
                UntrustedPoseReason::InvalidNumber {
                    token_index: 1,
                    token: "nope".to_owned(),
                }
            )]
        );
        assert_eq!(ancestry[1].instance, leaf_id);
        assert_eq!(ancestry[1].authored_pose.as_deref(), Some("0 2 0"));
        assert!(ancestry[1].causes.is_empty());

        let document_set_flattened = flatten_document_tree(&tree).unwrap();
        let mut compose_flattened = root;
        let middle_for_compose = middle.clone();
        let leaf_for_compose = leaf.clone();
        let mut compose_loader = move |key: &str, _: &Path| match key {
            "/mem/middle.hcdf" => Ok((middle_for_compose.clone(), None)),
            "/mem/leaf.hcdf" => Ok((leaf_for_compose.clone(), None)),
            other => Err(format!("unexpected key {other:?}")),
        };
        flatten_with(
            &mut compose_flattened,
            Path::new("/mem"),
            &mut compose_loader,
        )
        .unwrap();
        let compose_xml = compose_flattened.to_xml_string().unwrap();
        assert_eq!(document_set_flattened.to_xml_string().unwrap(), compose_xml);

        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        assert!(
            !projected
                .structural_projection
                .placement_projection_complete
        );
        assert!(projected
            .structural_projection
            .components
            .iter()
            .all(|component| !component.placement.is_projected()));
        assert_eq!(projected.flattened.to_xml_string().unwrap(), compose_xml);
    }

    #[test]
    fn placement_frame_ancestry_preserves_nested_compose_output() {
        let b = Hcdf::from_xml_str(
            r#"<hcdf name="b" version="1.0">
              <comp name="b-body"><frame name="origin"/></comp>
            </hcdf>"#,
        )
        .unwrap();
        let a = Hcdf::from_xml_str(
            r#"<hcdf name="a" version="1.0">
              <comp name="a-body"><frame name="origin"/></comp>
              <include uri="/mem/b.hcdf" name="b" pose="0 2 0"/>
            </hcdf>"#,
        )
        .unwrap();
        let mut first = include("/mem/a.hcdf", Some("a"));
        first.pose = Some("1 0 0".to_owned());
        first.placement_frame = Some("mount".to_owned());
        let root = root_with(vec![first]);

        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/a.hcdf", a.to_xml_string().unwrap().into_bytes())
            .unwrap();
        resolver
            .insert_document("/mem/b.hcdf", b.to_xml_string().unwrap().into_bytes())
            .unwrap();
        let tree = load_document_tree_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let id = IncludeInstanceId::root().child("a", 0).child("b", 0);
        let PlacementState::Unprojectable { ancestry } = &tree.instances[&id].placement else {
            panic!("nested placement must remain unprojectable");
        };
        assert_eq!(ancestry.len(), 2);
        assert_eq!(ancestry[0].placement_frame.as_deref(), Some("mount"));
        assert_eq!(
            ancestry[0].causes,
            vec![PlacementProjectionCause::PlacementFrameReference]
        );
        assert_eq!(ancestry[1].authored_pose.as_deref(), Some("0 2 0"));
        assert!(ancestry[1].causes.is_empty());

        let document_set_flattened = flatten_document_tree(&tree).unwrap();
        let mut compose_flattened = root;
        let a_for_compose = a.clone();
        let b_for_compose = b.clone();
        let mut compose_loader = move |key: &str, _: &Path| match key {
            "/mem/a.hcdf" => Ok((a_for_compose.clone(), None)),
            "/mem/b.hcdf" => Ok((b_for_compose.clone(), None)),
            other => Err(format!("unexpected key {other:?}")),
        };
        flatten_with(
            &mut compose_flattened,
            Path::new("/mem"),
            &mut compose_loader,
        )
        .unwrap();
        let compose_xml = compose_flattened.to_xml_string().unwrap();
        assert_eq!(document_set_flattened.to_xml_string().unwrap(), compose_xml);

        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        assert!(
            !projected
                .structural_projection
                .placement_projection_complete
        );
        assert!(projected
            .structural_projection
            .components
            .iter()
            .all(|component| !component.placement.is_projected()));
        assert_eq!(projected.flattened.to_xml_string().unwrap(), compose_xml);
    }

    #[test]
    fn flatten_prefixes_preserve_authored_names_without_allocation() {
        let module_doc = Hcdf {
            name: "module".to_owned(),
            version: "1.0".to_owned(),
            comp: vec![crate::model::Comp {
                name: "body".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/module.hcdf",
                module_doc.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let root = root_with(vec![
            include("/mem/module.hcdf", None),
            include("/mem/module.hcdf", Some("wheel")),
            include("/mem/module.hcdf", Some("")),
            include("/mem/module.hcdf", Some("   ")),
            include("/mem/module.hcdf", Some("$include")),
        ]);
        let tree = load_document_tree_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver.clone(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        let document_set_flattened = flatten_document_tree(&tree).unwrap();

        let mut compose_flattened = root;
        let mut compose_loader = |key: &str, _base: &Path| {
            assert_eq!(key, "/mem/module.hcdf");
            Ok((module_doc.clone(), None))
        };
        flatten_with(
            &mut compose_flattened,
            Path::new("/mem"),
            &mut compose_loader,
        )
        .unwrap();
        assert_eq!(document_set_flattened, compose_flattened);
        assert_eq!(
            document_set_flattened
                .comp
                .iter()
                .map(|component| component.name.as_str())
                .collect::<Vec<_>>(),
            vec!["body", "wheel/body", "body", "   /body", "$include/body"]
        );

        let root_id = IncludeInstanceId::root();
        let unnamed = root_id.unnamed_child(0);
        assert_eq!(tree.flatten_prefixes.segment(&unnamed), Some(None));
        assert_eq!(tree.flatten_prefixes.prefix(&unnamed), Some(""));
        let explicit_empty = root_id.unnamed_child(1);
        assert_eq!(tree.flatten_prefixes.segment(&explicit_empty), Some(None));
        assert_eq!(tree.flatten_prefixes.prefix(&explicit_empty), Some(""));

        for name in ["wheel", "   ", "$include"] {
            let instance = root_id.named_child(name, 0);
            assert_eq!(tree.flatten_prefixes.segment(&instance), Some(Some(name)));
            assert_eq!(tree.flatten_prefixes.prefix(&instance), Some(name));
        }

        let collision_root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            comp: vec![crate::model::Comp {
                name: "wheel/body".to_owned(),
                ..Default::default()
            }],
            include: vec![include("/mem/module.hcdf", Some("wheel"))],
            ..Default::default()
        };
        let collision_tree = load_document_tree_from_model(
            collision_root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let collision_document_set = flatten_document_tree(&collision_tree).unwrap();
        let mut collision_compose = collision_root;
        let mut compose_loader = |_: &str, _: &Path| Ok((module_doc.clone(), None));
        flatten_with(
            &mut collision_compose,
            Path::new("/mem"),
            &mut compose_loader,
        )
        .unwrap();
        assert_eq!(collision_document_set, collision_compose);
        assert_eq!(
            collision_document_set
                .comp
                .iter()
                .filter(|component| component.name == "wheel/body")
                .count(),
            2
        );
    }

    #[test]
    fn projection_preserves_repeated_sources_structural_alignment_and_graph_identity() {
        let module = Hcdf::from_xml_str(
            r#"<hcdf name="wheel-module" version="1.0">
              <comp name="device">
                <visual name="body"><model uri="assets/body.glb"/></visual>
                <frame name="mount"/>
                <port name="p"/>
              </comp>
            </hcdf>"#,
        )
        .unwrap();
        let root = Hcdf::from_xml_str(
            r#"<hcdf name="robot" version="1.0">
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="left"><endpoint><port-ref component="device" port="p">
                  <instance><segment name="left" occurrence="0"/></instance>
                </port-ref></endpoint></participant>
                <participant name="right"><endpoint><port-ref component="device" port="p">
                  <instance><segment name="right" occurrence="0"/></instance>
                </port-ref></endpoint></participant>
              </link>
              <include uri="/mem/module.hcdf" name="left"/>
              <include uri="/mem/module.hcdf" name="right"/>
            </hcdf>"#,
        )
        .unwrap();
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/module.hcdf",
                module.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();

        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();

        assert_eq!(projected.sources.len(), 2);
        assert_eq!(projected.instances.len(), 3);
        assert!(projected.flattened.include.is_empty());
        assert_eq!(
            projected
                .flattened
                .comp
                .iter()
                .map(|component| component.name.as_str())
                .collect::<Vec<_>>(),
            vec!["left/device", "right/device"]
        );
        assert!(
            projected
                .structural_projection
                .placement_projection_complete
        );
        assert_eq!(projected.structural_projection.components.len(), 2);
        assert_eq!(projected.structural_projection.visuals.len(), 2);
        assert_eq!(projected.structural_projection.frames.len(), 2);

        for component in &projected.flattened.comp {
            let crate::model::VisualAppearance::Model { model, .. } =
                &component.visual[0].appearance
            else {
                panic!("expected model-backed visual");
            };
            assert_eq!(model.uri.as_deref(), Some("/mem/assets/body.glb"));
        }

        let ConnectivityProjection::Valid { authored, graph } = &projected.connectivity else {
            panic!("expected a valid multi-scope connectivity graph");
        };
        assert_eq!(authored.scopes.len(), 3);
        for component in &projected.structural_projection.components {
            assert!(graph.node(&component.id).is_some());
        }
        for visual in &projected.structural_projection.visuals {
            assert!(graph.node(&visual.id).is_some());
        }
        for frame in &projected.structural_projection.frames {
            assert!(graph.node(&frame.id).is_some());
        }
    }

    #[test]
    fn normalization_failure_keeps_structural_truth_and_clears_the_graph() {
        let root = Hcdf::from_xml_str(
            r#"<hcdf name="robot" version="1.0">
              <comp name="controller"><port name="p"/></comp>
              <link name="invalid">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="only"><endpoint><port-ref component="controller" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#,
        )
        .unwrap();
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();

        assert_eq!(projected.flattened.comp.len(), 1);
        assert_eq!(projected.structural_projection.components.len(), 1);
        assert!(projected.connectivity.graph().is_none());
        let ConnectivityProjection::Invalid { issues } = &projected.connectivity else {
            panic!("invalid normalization must not retain a graph");
        };
        assert!(issues
            .iter()
            .any(|issue| matches!(issue, ProjectedConnectivityIssue::Normalization(_))));
    }

    #[test]
    fn unprojectable_placement_marks_projection_incomplete_without_dropping_structure() {
        let mut resolver = MemoryDocumentResolver::new();
        let module = Hcdf {
            name: "sensor".to_owned(),
            version: "1.0".to_owned(),
            comp: vec![crate::model::Comp {
                name: "body".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        resolver
            .insert_document(
                "/mem/sensor.hcdf",
                module.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let mut placed = include("/mem/sensor.hcdf", Some("sensor"));
        placed.pose = Some("1 2 3".to_owned());
        placed.placement_frame = Some("mount".to_owned());
        let tree = load_document_tree_from_model(
            root_with(vec![placed]),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();

        assert_eq!(projected.flattened.comp.len(), 1);
        assert!(
            !projected
                .structural_projection
                .placement_projection_complete
        );
        assert!(matches!(
            projected.structural_projection.components[0].placement,
            PlacementState::Unprojectable { .. }
        ));
    }

    #[test]
    fn nested_named_and_unnamed_segments_match_compose_structural_references() {
        let leaf = Hcdf {
            name: "leaf".to_owned(),
            version: "1.0".to_owned(),
            comp: vec![crate::model::Comp {
                name: "body".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let middle = Hcdf::from_xml_str(
            r#"<hcdf name="module" version="1.0">
              <comp name="bridge"/>
              <joint name="attach" type="fixed">
                <parent comp="bridge"/>
                <child comp="body"/>
              </joint>
              <include uri="/mem/leaf.hcdf"/>
            </hcdf>"#,
        )
        .unwrap();
        let root = root_with(vec![include("/mem/module.hcdf", Some("module"))]);
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/leaf.hcdf", leaf.to_xml_string().unwrap().into_bytes())
            .unwrap();
        resolver
            .insert_document(
                "/mem/module.hcdf",
                middle.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let tree = load_document_tree_from_model(
            root.clone(),
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let document_set_flattened = flatten_document_tree(&tree).unwrap();

        let mut compose_flattened = root;
        let leaf_for_load = leaf.clone();
        let middle_for_load = middle.clone();
        let mut compose_loader = move |key: &str, _: &Path| match key {
            "/mem/module.hcdf" => Ok((middle_for_load.clone(), None)),
            "/mem/leaf.hcdf" => Ok((leaf_for_load.clone(), None)),
            other => Err(format!("unexpected key {other:?}")),
        };
        flatten_with(
            &mut compose_flattened,
            Path::new("/mem"),
            &mut compose_loader,
        )
        .unwrap();
        assert_eq!(document_set_flattened, compose_flattened);

        let module_id = IncludeInstanceId::root().named_child("module", 0);
        let leaf_id = module_id.unnamed_child(0);
        assert_eq!(
            tree.flatten_prefixes.segment(&module_id),
            Some(Some("module"))
        );
        assert_eq!(tree.flatten_prefixes.segment(&leaf_id), Some(None));
        assert_eq!(tree.flatten_prefixes.prefix(&module_id), Some("module"));
        assert_eq!(tree.flatten_prefixes.prefix(&leaf_id), Some("module"));
        assert_eq!(
            document_set_flattened
                .comp
                .iter()
                .map(|component| component.name.as_str())
                .collect::<Vec<_>>(),
            vec!["module/bridge", "module/body"]
        );
        let joint = &document_set_flattened.joint[0];
        assert_eq!(
            joint.parent.as_ref().and_then(|end| end.comp.as_deref()),
            Some("module/bridge")
        );
        assert_eq!(
            joint.child.as_ref().and_then(|end| end.comp.as_deref()),
            Some("module/body")
        );
    }

    #[test]
    fn consumed_unnamed_instance_paths_clear_slots_but_explicit_empty_refs_remain_invalid() {
        let module = Hcdf::from_xml_str(
            r#"<hcdf name="module" version="1.0">
              <comp name="device"><port name="p"/></comp>
            </hcdf>"#,
        )
        .unwrap();
        let root = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="peer"><port name="p"/></comp>
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="included"><endpoint><port-ref component="device" port="p">
                  <instance><segment occurrence="0"/></instance>
                </port-ref></endpoint></participant>
                <participant name="peer"><endpoint><port-ref component="peer" port="p"/></endpoint></participant>
              </link>
              <include uri="/mem/module.hcdf"/>
            </hcdf>"#,
        )
        .unwrap();
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/module.hcdf",
                module.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        let crate::model::connectivity_xml::FunctionalEndpointChoice::Port(included) =
            &projected.flattened.link[0].participant[0].endpoint.endpoint
        else {
            panic!("expected port endpoint");
        };
        assert!(included.instance.is_none());
        assert!(matches!(
            projected.connectivity,
            ConnectivityProjection::Valid { .. }
        ));

        let explicit_empty = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="first"><port name="p"/></comp>
              <comp name="second"><port name="p"/></comp>
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="first"><endpoint><port-ref component="first" port="p">
                  <instance/>
                </port-ref></endpoint></participant>
                <participant name="second"><endpoint><port-ref component="second" port="p"/></endpoint></participant>
              </link>
            </hcdf>"#,
        )
        .unwrap();
        let tree = load_document_tree_from_model(
            explicit_empty,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        let crate::model::connectivity_xml::FunctionalEndpointChoice::Port(first) =
            &projected.flattened.link[0].participant[0].endpoint.endpoint
        else {
            panic!("expected port endpoint");
        };
        assert!(first
            .instance
            .as_ref()
            .is_some_and(|value| value.segment.is_empty()));
        let ConnectivityProjection::Invalid { issues } = projected.connectivity else {
            panic!("explicit empty instance references must remain invalid");
        };
        assert!(issues
            .iter()
            .any(|issue| matches!(issue, ProjectedConnectivityIssue::Conversion { .. })));
    }

    #[test]
    fn retained_unnamed_occurrence_reference_does_not_block_structural_projection() {
        let root = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="local"><port name="p"/></comp>
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="remote"><endpoint><port-ref component="remote" port="p">
                  <instance><segment occurrence="1"/></instance>
                </port-ref></endpoint></participant>
                <participant name="local"><endpoint><port-ref component="local" port="p"/></endpoint></participant>
              </link>
              <include uri="/mem/missing-first.hcdf"/>
              <include uri="/mem/missing-second.hcdf"/>
            </hcdf>"#,
        )
        .unwrap();
        let mut root = root;
        let remote = root.link[0].participant[0].clone();
        let local = root.link[0].participant.pop().unwrap();
        for index in 0..256 {
            let mut repeated = remote.clone();
            repeated.name = format!("remote-{index}");
            root.link[0].participant.push(repeated);
        }
        root.link[0].participant.push(local);
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut MemoryDocumentResolver::new(),
            DocumentSetOptions::default(),
        )
        .unwrap();

        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        assert_eq!(projected.flattened.comp.len(), 1);
        assert_eq!(projected.flattened.include.len(), 2);
        for participant in &projected.flattened.link[0].participant[..257] {
            let crate::model::connectivity_xml::FunctionalEndpointChoice::Port(remote) =
                &participant.endpoint.endpoint
            else {
                panic!("expected port endpoint");
            };
            assert_eq!(remote.component, "remote");
            let instance = remote.instance.as_ref().unwrap();
            assert_eq!(instance.segment.len(), 1);
            assert_eq!(instance.segment[0].name, None);
            assert_eq!(instance.segment[0].occurrence, 1);
        }
        assert!(matches!(
            projected.connectivity,
            ConnectivityProjection::Invalid { ref issues } if issues.len() == 2
        ));
    }

    #[test]
    fn retained_named_reference_cannot_collapse_into_an_unrelated_definition() {
        let decoy = Hcdf::from_xml_str(
            r#"<hcdf name="decoy" version="1.0">
              <comp name="missing/remote"><port name="p"/></comp>
            </hcdf>"#,
        )
        .unwrap();
        let root = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="local"><port name="p"/></comp>
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="remote"><endpoint><port-ref component="remote" port="p">
                  <instance><segment name="missing" occurrence="0"/></instance>
                </port-ref></endpoint></participant>
                <participant name="local"><endpoint><port-ref component="local" port="p"/></endpoint></participant>
              </link>
              <include uri="/mem/missing.hcdf" name="missing"/>
              <include uri="/mem/decoy.hcdf"/>
            </hcdf>"#,
        )
        .unwrap();
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/decoy.hcdf",
                decoy.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        assert_eq!(projected.flattened.include.len(), 1);
        assert_eq!(projected.flattened.comp.len(), 2);
        let crate::model::connectivity_xml::FunctionalEndpointChoice::Port(remote) =
            &projected.flattened.link[0].participant[0].endpoint.endpoint
        else {
            panic!("expected port endpoint");
        };
        assert_eq!(remote.component, "remote");
        let instance = remote.instance.as_ref().unwrap();
        assert_eq!(instance.segment.len(), 1);
        assert_eq!(instance.segment[0].name.as_deref(), Some("missing"));
        assert_eq!(instance.segment[0].occurrence, 0);
        assert!(matches!(
            projected.connectivity,
            ConnectivityProjection::Invalid { ref issues } if issues.len() == 1
        ));
    }

    #[test]
    fn nonexistent_resolved_occurrence_invalidates_only_connectivity_projection() {
        let module = Hcdf::from_xml_str(
            r#"<hcdf name="wheel" version="1.0">
              <comp name="device"><port name="p"/></comp>
            </hcdf>"#,
        )
        .unwrap();
        let root = Hcdf::from_xml_str(
            r#"<hcdf name="root" version="1.0">
              <comp name="local"><port name="p"/></comp>
              <link name="network">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="remote"><endpoint><port-ref component="device" port="p">
                  <instance><segment name="wheel" occurrence="1"/></instance>
                </port-ref></endpoint></participant>
                <participant name="local"><endpoint><port-ref component="local" port="p"/></endpoint></participant>
              </link>
              <include uri="/mem/wheel.hcdf" name="wheel"/>
            </hcdf>"#,
        )
        .unwrap();
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document(
                "/mem/wheel.hcdf",
                module.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();
        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();

        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        assert_eq!(projected.flattened.include.len(), 0);
        assert_eq!(projected.flattened.comp.len(), 2);
        let crate::model::connectivity_xml::FunctionalEndpointChoice::Port(remote) =
            &projected.flattened.link[0].participant[0].endpoint.endpoint
        else {
            panic!("expected port endpoint");
        };
        assert_eq!(remote.component, "device");
        let instance = remote.instance.as_ref().unwrap();
        assert_eq!(instance.segment.len(), 1);
        assert_eq!(instance.segment[0].name.as_deref(), Some("wheel"));
        assert_eq!(instance.segment[0].occurrence, 1);
        assert!(matches!(
            projected.connectivity,
            ConnectivityProjection::Invalid { ref issues } if !issues.is_empty()
        ));
    }

    #[test]
    fn included_scope_keeps_local_and_nested_references_relative() {
        let leaf = Hcdf::from_xml_str(
            r#"<hcdf name="leaf" version="1.0">
              <comp name="leaf-device"><port name="p"/></comp>
            </hcdf>"#,
        )
        .unwrap();
        let middle = Hcdf::from_xml_str(
            r#"<hcdf name="module" version="1.0">
              <comp name="bridge"><port name="local"/></comp>
              <link name="inside">
                <selected purpose="communication" carrier="electrical"/>
                <participant name="local"><endpoint><port-ref component="bridge" port="local"/></endpoint></participant>
                <participant name="nested"><endpoint><port-ref component="leaf-device" port="p">
                  <instance><segment name="leaf" occurrence="0"/></instance>
                </port-ref></endpoint></participant>
              </link>
              <include uri="/mem/leaf.hcdf" name="leaf"/>
            </hcdf>"#,
        )
        .unwrap();
        let root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            include: vec![include("/mem/module.hcdf", Some("module"))],
            ..Default::default()
        };
        let mut resolver = MemoryDocumentResolver::new();
        resolver
            .insert_document("/mem/leaf.hcdf", leaf.to_xml_string().unwrap().into_bytes())
            .unwrap();
        resolver
            .insert_document(
                "/mem/module.hcdf",
                middle.to_xml_string().unwrap().into_bytes(),
            )
            .unwrap();

        let tree = load_document_tree_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("/mem/root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let projected = project_document_tree(tree, &mut MemoryDocumentResolver::new()).unwrap();
        let ConnectivityProjection::Valid { authored, graph } = &projected.connectivity else {
            panic!("local and nested relative references must normalize");
        };

        let module_id = IncludeInstanceId::root().child("module", 0);
        let leaf_id = module_id.child("leaf", 0);
        let module_scope = authored
            .scopes
            .iter()
            .find(|scope| scope.instance == module_id)
            .unwrap();
        let local = module_scope.networks[0].participants[0].endpoint.port();
        let nested = module_scope.networks[0].participants[1].endpoint.port();
        assert_eq!(
            local.scope,
            crate::model::connectivity::ReferenceScope::Local
        );
        assert_eq!(
            nested.scope,
            crate::model::connectivity::ReferenceScope::Instance(
                IncludeInstanceId::root().child("leaf", 0)
            )
        );

        let bridge = structural_component_identity(graph.document(), &module_id, "bridge");
        let leaf_device = structural_component_identity(graph.document(), &leaf_id, "leaf-device");
        assert!(graph.node(&bridge.stable_id()).is_some());
        assert!(graph.node(&leaf_device.stable_id()).is_some());
    }

    #[test]
    fn document_set_error_becomes_a_typed_connectivity_issue() {
        let error = DocumentSetError::LimitExceeded {
            kind: DocumentSetLimitKind::Depth,
            limit: 1,
            actual: 2,
            resource: Some("memory://nested.hcdf".to_owned()),
        };

        let projection = ConnectivityProjection::from_document_set_result(Err(error.clone()));
        let ConnectivityProjection::Invalid { issues } = projection else {
            panic!("document-set errors must invalidate connectivity");
        };
        assert_eq!(issues, vec![ProjectedConnectivityIssue::DocumentSet(error)]);
        assert_eq!(issues[0].code(), "E_DOC_RESOURCE_LIMIT");
    }

    #[test]
    fn projected_connectivity_can_be_moved_without_reconstruction() {
        let root = Hcdf {
            name: "root".to_owned(),
            version: "1.0".to_owned(),
            ..Default::default()
        };
        let mut resolver = MemoryDocumentResolver::new();
        let projected = load_projected_document_set_from_model(
            root,
            DocumentIdentity::new("robot").unwrap(),
            DocumentResourceKey::new("memory://root.hcdf").unwrap(),
            &mut resolver,
            DocumentSetOptions::default(),
        )
        .unwrap();
        let expected = projected.connectivity().clone();

        assert_eq!(projected.into_connectivity(), expected);
    }
}
