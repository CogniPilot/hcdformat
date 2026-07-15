//! Canonical connectivity identity, normalization, and resolution.
//!
//! The authored model remains editable in [`crate::model::connectivity`]. Normalization produces an
//! immutable graph used by validation, visualization, tracing, and editing diagnostics.

mod graph;
mod identity;
mod normalize;
mod profile_registry;
pub mod units;

pub(crate) use graph::{
    ConnectivityGraphExtension, ConnectivityGraphExtensionEdge, StreamGroupReferenceTarget,
};
pub use graph::{
    ConnectivityIssue, ConnectivityNode, ConnectivityNodeData, ConnectivityResolver, EdgeExactness,
    EdgeKind, IssueLevel, NormalizedConnectivityEdge, NormalizedConnectivityGraph, ResolveError,
};
pub use identity::{
    structural_component_identity, structural_frame_identity, structural_visual_identity,
    IdentityPart, ObjectIdentity, ObjectKind, StableEdgeId, StableObjectId,
};
pub use normalize::{
    normalize_connectivity, normalize_connectivity_with_options, NormalizationError,
    NormalizationOptions, ProfileRegistryCompleteness,
};
pub use profile_registry::{
    built_in_profile_registry, CitationKey, HcdfSchemaCompatibility, ProfileCitation,
    ProfileRegistry, ProfileRegistryError, ProfileRuleset, RegistryVersion, ValidatorKind,
};
