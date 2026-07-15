//! The per-visual asset-bake SIDE-CHANNEL shared by the URDF and SDF importers.
//!
//! Compiled whenever the `urdf` *or* the `sdf` feature is enabled (like the internal `pyrepr` module and
//! the shared `LossManifest` in [`mod@crate::to_urdf`]), so `from_sdf_str_with_assets` works in an
//! `sdf`-only build and both importers hand a baker the SAME hint type.

/// A side-channel hint for the asset baker: the ALREADY-resolved per-visual mesh `scale` and flat material
/// `color` of one ARM-A (mesh) `<visual>`. The HCDF model deliberately carries NEITHER on a visual `<model>`
/// (a model is a clean GLB `uri`+`sha`; its scale, mirror and appearance BAKE INTO the GLB), so these are
/// returned ALONGSIDE the [`Hcdf`](crate::Hcdf) by `from_urdf_str_with_assets` / `from_sdf_str_with_assets`
/// rather than stored on the model. A baker looks each visual up by `(comp, visual)` and folds `scale`
/// (magnitude + mirror) and `color` into the GLB.
///
///   * `comp`: the owning link/comp name (matches [`Comp::name`](crate::Comp::name)).
///   * `visual`: the visual's name (the SAME name the importer assigns to the [`Visual`](crate::Visual)).
///   * `scale`: the source mesh scale text (`"sx sy sz"`: the URDF `<mesh scale>` attribute / the SDF
///     `<mesh><scale>` element), or `None` when absent (== `"1 1 1"`).
///   * `color`: the resolved flat material colour (`"r g b [a]"`, 0..1: the URDF inline/named `<material>`
///     rgba / the SDF `<material><diffuse>`), or `None` when the visual carries no flat-colour material.
///   * `texture`: the diffuse/albedo texture uri of the visual's material (today only the SDF
///     `<material><pbr><metal|specular><albedo_map>`; the URDF importer always leaves it `None`), verbatim
///     (`model://…` uris included; the BAKER resolves it through the same package-root machinery as mesh
///     uris). Unlike `scale`/`color` it may also ride a PRIMITIVE visual's hint: a textured primitive is
///     synthesized into a UV-mapped GLB at the asset step (see the CLI `--bake` walk), while on a mesh
///     visual the texture embeds using the mesh's OWN texture coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualAssetHint {
    pub comp: String,
    pub visual: String,
    pub scale: Option<String>,
    pub color: Option<String>,
    pub texture: Option<String>,
}
