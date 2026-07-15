# hcdformat-rs

Typed Rust model, parser, and sha-pinned official schema for **HCDF** (Hardware Configuration
Descriptive Format). The single Rust definition of "official HCDF", shared by `hcdviz` (static viewer)
and `dendrite_build` (discovery/assembly), so the dialect drift the old dendrite parser fell into
(text poses, `pose_cg`, comp-level `<model>`) cannot recur.

Targets the OFFICIAL frozen 1.0 schema (`<pose xyz= rpy=>` attributes etc.) and binds to it by content
hash per the hcdformat versioning contract: the frozen `versions/1.0/hcdf.xsd` is vendored under
`assets/schema/`, its sha256 is pinned from a vendored copy of `versions/HASHES` (single source), and
`build.rs` fails the build if the bytes drift. Only official HCDF is read; there is NO legacy-dendrite
dialect reader; pre-standard fragments get regenerated as official 1.0, not migrated.

Ships: the full official type set (`src/model/`) with XML round-trip preserving verbatim `<extension>`
bodies, the reader-side version gate, include flatten + sha-pinning, and `.hcdfz` bundling (on-disk
native-only, in-memory on every target). WASM-clean by default: every feature is opt-in, and each layer
a browser build wants is pure-Rust on wasm32: XSD validation (`xsd`, uppsala), the URDF/SDF converters
(`urdf`/`sdf`), the xacro expander (`xacro`, xacro-pure), and the asset baker (`bake`/`bake-dae`). The
hardened remote fetcher (`remote`) and the `hcdf` CLI (`cli`) are native-only.
