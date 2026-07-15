//! `<visual>`: optional `pose` then an `xs:choice` of two arms:
//!   - ARM A `{ model, geometry? }`  (GLB primary + optional primitive fallback)
//!   - ARM B `{ geometry, color? }`  (primitive + optional inline color)
//!
//! Both arms contain `geometry`, so `#[serde(untagged)]` mis-deserializes the common ARM B case.
//! Instead we deserialize into a flat helper `VisualRaw` (all possible children/attrs) and map to
//! the typed [`VisualAppearance`] by presence of `<model>` (present ⇒ ARM A, else ARM B). Serialize
//! reverses through the same helper, emitting valid official form.
use super::common::Color;
use super::enums::NameOrigin;
use super::geometry::{ExcludeSubmesh, Submesh, VisualGeometry};
use super::Pose;
use serde::{Deserialize, Serialize};

/// `model_ref`: `<model uri= sha=>` (a GLB reference; NO scale, NO href in official 1.0), optionally
/// carrying submesh selectors that choose WHICH subtrees of the model this visual draws. Three
/// mutually exclusive modes: no selectors = the whole model; `<submesh>`* = the UNION of named
/// subtrees (include mode); `<exclude-submesh>`* = the whole model MINUS named subtrees (exclude
/// mode). Mixing include + exclude is a validator error: the grammar carries both optional lists,
/// so serde stays permissive and the companion validator enforces exclusivity. Selections keep their
/// model-root-relative node transforms (no recenter); `@center` on a `<submesh>` is legal only on a
/// LONE include (validator-enforced). Both lists are `skip_serializing_if` empty, so a selector-less
/// `<model>` (the whole existing corpus) serializes byte-identically (no submesh token emitted).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelRef {
    #[serde(rename = "@uri", default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(rename = "@sha", default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// `<submesh>`*: include-mode selectors (the union of named subtrees). Reuses the [`Submesh`]
    /// type (name + optional `@center`); `@center` is meaningful only on a lone include.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submesh: Vec<Submesh>,
    /// `<exclude-submesh>`*: exclude-mode selectors (the whole model minus named subtrees). A
    /// dedicated [`ExcludeSubmesh`] (name only, no `@center`, since a subtraction has nothing to
    /// recenter). Appended after `submesh` in the sequence; mixing the two lists is a validator error.
    #[serde(
        rename = "exclude-submesh",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub exclude_submesh: Vec<ExcludeSubmesh>,
}

/// The exclusive appearance choice for a `<visual>`.
#[derive(Debug, Clone, PartialEq)]
pub enum VisualAppearance {
    /// ARM A: GLB model with optional primitive fallback geometry.
    Model {
        model: ModelRef,
        geometry: Option<VisualGeometry>,
    },
    /// ARM B: primitive geometry with optional inline color.
    Primitive {
        geometry: Option<VisualGeometry>,
        color: Option<Color>,
    },
}

impl Default for VisualAppearance {
    fn default() -> Self {
        VisualAppearance::Primitive {
            geometry: None,
            color: None,
        }
    }
}

/// `<visual name= toggle= name-origin=>` + pose + appearance.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Visual {
    pub name: String,
    pub toggle: Option<String>,
    pub name_origin: Option<NameOrigin>,
    pub pose: Option<Pose>,
    pub appearance: VisualAppearance,
}

/// Flat 1:1 helper matching every possible attribute/child of `<visual>` (derive-(de)serialized).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename = "visual")]
struct VisualRaw {
    #[serde(rename = "@name", default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(rename = "@toggle", default, skip_serializing_if = "Option::is_none")]
    toggle: Option<String>,
    #[serde(
        rename = "@name-origin",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    name_origin: Option<NameOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pose: Option<Pose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    geometry: Option<VisualGeometry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<Color>,
}

impl<'de> Deserialize<'de> for Visual {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let r = VisualRaw::deserialize(d)?;
        let appearance = match r.model {
            Some(model) => VisualAppearance::Model {
                model,
                geometry: r.geometry,
            },
            None => VisualAppearance::Primitive {
                geometry: r.geometry,
                color: r.color,
            },
        };
        Ok(Visual {
            name: r.name.unwrap_or_default(),
            toggle: r.toggle,
            name_origin: r.name_origin,
            pose: r.pose,
            appearance,
        })
    }
}

impl Serialize for Visual {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (model, geometry, color) = match &self.appearance {
            VisualAppearance::Model { model, geometry } => {
                (Some(model.clone()), geometry.clone(), None)
            }
            VisualAppearance::Primitive { geometry, color } => {
                (None, geometry.clone(), color.clone())
            }
        };
        let r = VisualRaw {
            name: Some(self.name.clone()),
            toggle: self.toggle.clone(),
            name_origin: self.name_origin,
            pose: self.pose.clone(),
            model,
            geometry,
            color,
        };
        r.serialize(s)
    }
}
