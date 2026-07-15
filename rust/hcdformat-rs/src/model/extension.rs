//! `<extension domain= version=>` with an `xs:any processContents="lax"` body.
//!
//! The body (e.g. ros2_control, gazebo blocks) is captured VERBATIM as raw inner XML and re-emitted
//! unchanged. serde cannot round-trip arbitrary nested XML through `$value`, so the body is NOT a
//! serde child: it is extracted/spliced by the document reader/writer (see the `de`/`ser` modules).
//!
//! During parse the reader rewrites each `<extension>` into a skeleton element carrying a synthetic
//! `@hcdf-ext-id` attribute and records the inner XML by id; after serde parses the skeleton, the
//! bodies are reattached by id. `@hcdf-ext-id` is `skip_serializing`, so it never appears in output.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "extension")]
pub struct Extension {
    /// `@domain` is REQUIRED by the schema.
    #[serde(rename = "@domain", default)]
    pub domain: String,
    #[serde(rename = "@version", default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Internal: links a parsed skeleton element to its captured body during read. Never serialized.
    #[serde(rename = "@hcdf-ext-id", default, skip_serializing)]
    pub(crate) ext_id: Option<u32>,

    /// Verbatim inner XML of the extension body. Populated after parse; emitted raw on write.
    #[serde(skip)]
    pub body: String,
}
