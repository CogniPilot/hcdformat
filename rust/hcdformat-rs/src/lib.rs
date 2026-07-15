//! # hcdformat
//!
//! Typed Rust model, parser, and sha-pinned official schema for **HCDF** (Hardware Configuration
//! Descriptive Format). Shared by the `hcdviz` viewer and `dendrite_build` tooling so there is exactly
//! one definition of "official HCDF" in Rust, preventing the dialect drift that the old dendrite
//! parser fell into (text poses, `pose_cg`, comp-level `<model>`).
//!
//! Targets the OFFICIAL frozen-1.0 schema (attribute-form `<pose xyz= rpy=>` etc.) and binds to it by
//! content hash per the hcdformat versioning contract (see [`schema`] + [`version`]).
//!
//! ## What ships
//! The full official type set ([`model`]) with XML round-trip preserving verbatim `<extension>` bodies
//! AND XML comments (an anchored side-channel, [`comments`]; nothing new in the schema),
//! the build-time + runtime schema sha-pin ([`schema::verify_embedded_schema`]), the reader-side version
//! gate ([`version`], wired into [`Hcdf::from_xml_str`]), include flatten + sha-pinning ([`compose`]),
//! and `.hcdfz` bundling (on-disk native-only, in-memory on every target). Each converter/validator layer
//! is opt-in and wasm-clean: pure-Rust XSD validation (`xsd`), the URDF/SDF converters (`urdf`/`sdf`),
//! the xacro expander (`xacro`), and the asset baker (`bake`/`bake-dae`); the hardened remote fetcher
//! (`remote`) and the `hcdf` CLI (`cli`) are native-only. Only official HCDF is read: there is NO
//! legacy-dendrite dialect reader; pre-standard fragments get regenerated as official 1.0, not migrated.
// ── The asset baker (feature = "bake"; pure-Rust and WASM-CLEAN, the in-browser-bake payoff) ──
//
// Mesh -> canonical GLB (visual) / lean binary STL (collision), with scale/mirror baked into the
// vertices. Deps `mesh-loader` + `glam` are pure-Rust (no `*-sys`/cc/native backend) so this compiles
// to wasm32, unlike the native-only bundle/remote layer below. NOT `cfg(not(wasm32))`-gated: a browser
// can bake. The verbatim GLB/STL passthrough (the bundle byte-parity path) stays in `assets_vendor`.
#[cfg(feature = "bake")]
pub mod bake;
// The `hcdf` CLI implementation (feature = "cli", native-only). It lives in the LIBRARY (not just the
// `src/bin/hcdf.rs` entry) so the PyO3 binding's `run_cli` drives the SAME Rust CLI: the Rust binary is
// the sole `hcdf` CLI on BOTH the native/colcon build and the pip/wheel console script. Pulls clap;
// native-only (the bundle/bake/remote wiring it drives is), so it never enters a wasm tree.
#[cfg(all(feature = "cli", not(target_arch = "wasm32")))]
pub mod cli;
pub mod connectivity;
pub mod document_set;
// The XSD-driven source generators (feature = "cli", native-only): the pure-Rust ports of the retired
// `generate_hcdf_rs_enums.py` / `generate_json_schema.py`, wired as the `hcdf regen enums` / `regen
// schema` subcommands. Dev-time only (they read `hcdf.xsd` + the model sources and emit the committed
// `src/model/enums.rs` / `hcdf.schema.json`), so they ride the same native `cli` gate as the CLI itself;
// a `#[cfg(test)]` drift check regenerates each artifact and asserts byte-equality with no Python.
pub mod compose;
pub mod resource;
pub mod resource_path;
#[cfg(all(feature = "cli", not(target_arch = "wasm32")))]
pub mod xsdgen;
// ── URDF <-> HCDF converter + profile (feature = "urdf"; wasm-clean, urdf-rs is pure-Rust) ──
//
// `from_urdf` uses urdf-rs (validated against real URDF corpora) as a structural-validation gate and maps the document
// from a parallel quick-xml tree (exact text/presence parity with the Python oracle). `to_urdf` is the
// reverse with a structured `LossManifest`; `profile` classifies a doc against the HCDF-URDF Profile.
#[cfg(feature = "urdf")]
pub mod from_urdf;
#[cfg(feature = "urdf")]
pub mod profile;
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub(crate) mod pyrepr;
// The per-visual scale/colour asset-bake side-channel ([`asset_hint::VisualAssetHint`]) is shared by the
// URDF and SDF importers (`from_urdf_str_with_assets` / `from_sdf_str_with_assets`), so like `pyrepr` it
// compiles whenever either converter is enabled.
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub mod asset_hint;
// `to_urdf` houses the shared [`to_urdf::LossManifest`] (re-exported by [`to_sdf`]); it is otherwise
// self-contained (compose/error/model/pyrepr only, NO dep on `from_urdf`), so it compiles whenever the
// URDF *or* the SDF converter is enabled, matching the Python authority, where `sdf_io.to_sdf` imports
// `LossManifest` from `urdf_hcdf.to_urdf`.
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub mod to_urdf;
// The shared renderer for the profile report and the URDF-export loss manifest (the flat text, the
// grouped markdown, and the pretty JSON). Lifted out of the CLI so the CLI and the PyO3 binding drive
// ONE renderer; it holds only the small `json.dumps`-parity writer, the render methods themselves hang
// off `LossManifest`/`ProfileReport`. Compiles whenever either converter is enabled (the loss renders
// need only `LossManifest`; the profile renders are further gated on `urdf` at the method site).
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub(crate) mod report;
// Shared HCDF -> SDFormat `<sensor>` emission (the inverse of the typed sensor re-root). Used by BOTH
// exporters: `to_sdf` embeds the `<sensor>` in its `<link>`; `to_urdf` wraps per-comp sensors in a
// `<gazebo reference="link">` block, so it compiles whenever either converter is enabled.
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub(crate) mod to_sensor;
// ── SDF <-> HCDF converter (feature = "sdf"; wasm-clean, the importer is pure quick-xml) ──
//
// `from_sdf` maps the document from a tolerant quick-xml tree, mirroring `from_sdf.py`'s lxml
// `recover=True` walk (exact text/presence parity with the Python oracle); it imposes NO strict gate and
// NEVER shells to gz, so a default-omitting / `model://` / unnamed-element SDF imports just as it does
// under Python. `to_sdf` is the reverse with a `LossManifest`. There is NO `gz` subprocess anywhere in
// the crate: a caller that wants `<include>`/`model://`/Fuel resolution, spec-default fill, or a
// canonicalized export runs `gz sdf --print` externally: an explicit external pre/post-pass, never an
// automatic one, mirroring `from_sdf.py` (which never invokes gz on the import path). NOT default;
// absent from wasm/hcdviz.
#[cfg(feature = "sdf")]
pub mod from_sdf;
#[cfg(feature = "sdf")]
pub mod to_sdf;
// ── xacro -> URDF expander (feature = "xacro"; FULLY pure-Rust + wasm-clean, backed by the crates.io
// `xacro-pure` port of canonical ROS `xacro`). `expand_xacro` expands a .xacro to a URDF string IN-PROCESS
// (the in-browser-xacro payoff) for BOTH the SO-ARM and OpenArm classes (no subprocess, no Python, no
// fork), which then chains into `from_urdf` for .xacro import. `xacro-pure` pulls `rustpython-vm` (pure-
// Rust, wasm-clean) ONLY under this feature. NOT default; absent from a bare model build and from hcdviz
// (`hcdformat = "1"`, no features). dendrite_build enables `xacro` and needs NO `[patch.crates-io]`.
pub mod comments;
pub mod error;
#[cfg(feature = "xacro")]
pub mod xacro;
// ── glTF → GLB packer (wasm-clean, ungated: every vendor/bake path and the browser importer use it) ──
//
// Packs a multi-file `.gltf` (external `.bin` buffers / `.png`/`.jpg` textures / `data:` URIs) into ONE
// self-contained GLB, so the content-hash rename in vendor/bake cannot sever the relative companion
// links. Pure serde_json + a hand-rolled GLB frame; NO filesystem access (the caller passes a resolver
// closure), so dendrite_build's in-browser MemFs importer can drive it directly.
pub mod gltf_pack;
// ── XSD-driven HCDF XML <-> JSON converter (feature = "json"; pure-Rust + wasm-clean) ──
//
// A byte-for-byte port of the Python authority `hcdf/convert.py` (+ `hcdf_io/json_io.py`): it reproduces
// that converter's mapping conventions EXACTLY so existing `.json` files stay compatible (JSON is a
// DERIVED view of the sha-pinned XSD, not a second schema). Uses only `quick-xml` + `serde_json` (already
// base deps) and the embedded frozen schema (reused via `schema`, never re-embedded), so it compiles to
// wasm32. NOT default; the `cli` feature and the PyO3 binding enable it.
#[cfg(feature = "json")]
pub mod json;
pub mod model;
pub mod schema;
#[cfg(test)]
mod schema_build_support;
pub mod validate;
// Pure-Rust XSD validation against the embedded frozen `hcdf.xsd` (feature `xsd`). Backed by `uppsala`
// (zero transitive deps, wasm-clean), this is the in-crate replacement for the lxml `hcdf validate
// --xsd` shell-out: it catches the schema-SHAPE errors the typed parse silently swallows (a `<box>`
// without a `<size>` child, a `<color>` with an `<rgba>` child instead of the `rgba=` attribute). NOT
// default, so a bare/hcdviz build pulls neither uppsala nor this module; dendrite_build enables `xsd`.
pub mod value;
pub mod version;
#[cfg(feature = "xsd")]
pub mod xsd;

// ── Bundling + remote-asset vendoring (native-only; wasm/default build never compiles these) ──
//
// The bundle + vendor layer walks the local filesystem, so it is gated `cfg(not(wasm32))`. It adds NO
// new dependencies (the ZIP codec is the hand-rolled, pure-Rust [`zip_store`]). The hardened http(s)
// fetcher [`remote`] is the ONLY thing that pulls heavy native deps (ureq + rustls), so it is behind
// the OPT-IN `remote` feature; a default/wasm build pulls neither.
#[cfg(not(target_arch = "wasm32"))]
pub mod assets_vendor;
#[cfg(not(target_arch = "wasm32"))]
pub mod bundle;
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub mod remote;
// ── Import-time asset baking (native-only; the whole-document mesh-bake orchestration) ──
//
// The walk that turns a freshly imported URDF/SDF document into one whose visual `<model>` / collision
// `<mesh>` references point at canonical, content-addressed baked assets, plus the two string
// import-then-bake entry points (`from_urdf_str_with_baking` / `from_sdf_str_with_baking`). This is the
// Rust home of what `hcdf/assets.py` orchestrates; the CLI's `convert --bake` and the string entry points
// share this one walk. Native-only (it resolves mesh uris on disk and writes an asset directory) and gated
// on `bake` + a converter; the mesh conversions live in [`bake`], the passthrough/resolver in
// [`assets_vendor`].
#[cfg(all(
    feature = "bake",
    not(target_arch = "wasm32"),
    any(feature = "urdf", feature = "sdf")
))]
pub mod import_bake;
// The `ZIP_STORED` codec is pure `std::io` (its one `std::fs` item, `read_file_bytes`, is itself
// native-gated), so it compiles to wasm; the in-memory bundle writer below builds on it. NOT gated.
pub mod zip_store;

// ── In-memory `.hcdfz` packer (wasm-clean, the browser-side bundle payoff) ──
//
// `bundle_mem::pack_to_bytes` builds a self-contained `.hcdfz` from an IN-MEMORY [`Hcdf`] + a caller-
// supplied map of asset bytes, using `flatten_with` (loader closure), `content_sha`, and the pure-Rust
// [`zip_store`] codec. NO filesystem access, so it works on native AND wasm32 (the disk `bundle::pack`
// stays native-only and unchanged). NOT feature-gated and adds NO new dependency (sha2 + zip_store are
// already in the base tree).
pub mod bundle_mem;

mod de;
mod ser;
mod strict;

#[cfg(feature = "bake")]
pub use bake::{
    bake_convert, bake_convert_bytes, bake_convert_bytes_with, bake_convert_bytes_with_texture,
    bake_convert_with_texture, bake_textured_box, to_glb, to_glb_bytes, to_lean, to_lean_bytes,
    BakeError, BakeKind, BakedAsset, HintTexture,
};
#[cfg(feature = "urdf")]
pub use from_urdf::{from_urdf_str, from_urdf_str_with_assets};
// Import-then-bake string entry points + the whole-document bake walk / deferral-note filter they compose
// (native-only; see [`import_bake`]). Each is gated on the converter it imports through.
#[cfg(all(feature = "bake", not(target_arch = "wasm32"), feature = "sdf"))]
pub use import_bake::from_sdf_str_with_baking;
#[cfg(all(feature = "bake", not(target_arch = "wasm32"), feature = "urdf"))]
pub use import_bake::from_urdf_str_with_baking;
#[cfg(all(
    feature = "bake",
    not(target_arch = "wasm32"),
    any(feature = "urdf", feature = "sdf")
))]
pub use import_bake::{bake_document, drop_baked_notes, BakeEnv};
// The shared per-visual asset-bake side-channel struct both importers return (see [`asset_hint`]).
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub use asset_hint::VisualAssetHint;
#[cfg(feature = "urdf")]
pub use profile::{check_profile, Finding, ProfileReport, Tier};
#[cfg(feature = "urdf")]
pub use to_urdf::{to_urdf, LossManifest};
// The shared `LossManifest` is re-exported whenever the URDF *or* SDF converter is enabled (the SDF
// exporter reuses it). Guarded so it is not double-exported when both features are on.
#[cfg(feature = "sdf")]
pub use from_sdf::{from_sdf_str, from_sdf_str_with_assets};
#[cfg(all(feature = "sdf", not(feature = "urdf")))]
pub use to_urdf::LossManifest;
// `from_sdf_path` never touches `std::process` (it reads the file and maps the raw SDF, like
// `from_sdf.py`), so it is available on all targets including wasm.
#[cfg(feature = "sdf")]
pub use from_sdf::from_sdf_path;
#[cfg(feature = "sdf")]
pub use to_sdf::to_sdf;
// xacro -> URDF expansion + .xacro import. `expand_xacro` / `expand_xacro_with_args` / `expand_xacro_str`
// / `expand_xacro_str_with` (FULLY pure-Rust, backed by the crates.io `xacro-pure` port of canonical
// `xacro`) and `from_xacro_path` (expand -> from_urdf) are available on all targets including wasm, for
// BOTH the SO-ARM and OpenArm classes, with no subprocess. `expand_xacro_with_args` additionally seeds the
// `$(arg)` table from a caller map (the `name:=value` overrides canonical `xacro` takes on the CLI).
#[cfg(feature = "xacro")]
pub use xacro::{
    expand_xacro, expand_xacro_str, expand_xacro_str_with, expand_xacro_with_args, from_xacro_path,
};
// XML comment preservation: the anchored side-channel types carried by the model plus the fragment
// helpers a single-comp XML editor uses (capture comments from an edited fragment / splice them back
// into a serialized one). Whole-document capture/splice is wired into from_xml_str/to_xml_string.
pub use comments::{
    extract_fragment_comments, splice_fragment_comments, CapturedComment, CommentAnchor,
    CommentSet, DocComments, FragmentComments,
};
pub use error::{Error, Result};
// The glTF → GLB packer + the content-based GLB check (wasm-clean, no filesystem: the resolver closure
// supplies companion bytes, so the browser MemFs importer calls `pack_gltf_to_glb` directly).
pub use gltf_pack::{is_glb_bytes, pack_gltf_to_glb, GltfPackError};
pub use model::{
    Comp, Extension, Hcdf, Joint, Pose, StreamProfileDocument, StreamProfileResource, Visual,
    VisualAppearance,
};
pub use validate::{
    validate_coverage, validate_enums, validate_loops, validate_network,
    validate_network_with_options, validate_semantic, validate_structural, Issue, Level,
};
pub use version::HcdfVersion;
// Pure-Rust XSD schema-shape validation (feature `xsd`, native + wasm). `validate_xsd` returns the
// schema violations as `Issue`s (empty ⇒ valid); `E_XSD` is the stable code they carry.
#[cfg(feature = "xsd")]
pub use xsd::{validate_stream_profile_xsd, validate_xsd, E_XSD};
// XSD-driven HCDF XML <-> JSON conversion (feature `json`, native + wasm). `hcdf_xml_to_json` reproduces
// `convert.py`'s JSON view of a document; `json_to_hcdf_xml` is the XSD-aware inverse.
#[cfg(feature = "json")]
pub use json::{hcdf_xml_to_json, json_to_hcdf_xml};

#[cfg(not(target_arch = "wasm32"))]
pub use compose::{flatten, flatten_path, stamp_include_shas};
// Remote-include-fetching flatten (parity with include.py flatten(fetch=True)). Behind the `remote`
// feature so it never enters the default/wasm dependency tree (it needs the hardened fetcher).
pub use compose::{
    content_sha, content_sha_of_module, flatten_with, verify_include_shas, IncludeShaMismatch,
};
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub use compose::{flatten_fetch, flatten_path_fetch};
pub use resource_path::{resolve_resource_reference, ResourceReferenceError};

// The in-memory `.hcdfz` packer + opener, available on ALL targets (native + wasm). Lets a browser editor
// build a portable bundle from its in-memory doc + picked asset bytes and download it (`pack_to_bytes`),
// and open a `.hcdfz` back into its root doc + in-memory asset tree (`open_bundle_bytes`) with no
// filesystem: the browser-render payoff (the assets feed a custom in-memory Bevy AssetReader).
pub use bundle_mem::{
    open_bundle_bytes, pack_to_bytes, pack_to_bytes_keep_live, KeepLiveModule, MemBundle,
    MemPackReport, OpenedMemBundle, OpenedModule,
};

#[cfg(not(target_arch = "wasm32"))]
pub use assets_vendor::{resolve_uri, vendor_assets, AssetKind, VendorEntry, VendorStatus};
#[cfg(not(target_arch = "wasm32"))]
pub use bundle::{
    open_bundle, pack, verify, verify_bytes, BundleDir, OpenedBundle, PackManifest, PackOptions,
    RemotePolicy, VerifyReport,
};
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub use remote::{fetch_remote, RemoteError};
