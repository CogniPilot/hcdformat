# Changelog

## Unreleased

Library and tooling additions plus additive HCDF 1.0 schema extensions (backward-compatible: new
optional elements/attributes and enum types only; existing documents still validate).

### Schema (HCDF 1.0, additive)
- **Fluid sensor category** (`<sensor>/<fluid>`): the twelfth sensor category, for gas/liquid pressure
  and flow (distinct from force = solid contact and chemical = composition). New `FluidSensorType`
  (barometer, airspeed, depth, flow, level, differential, gauge, absolute, sealed, vacuum) and
  `ProbeType` (pitot, pitot_static, multihole, hotwire, venturi, orifice) enums. Fully describes a
  barometer (absolute pressure → altitude) and an airspeed/pitot sensor (differential dynamic pressure
  → airspeed) over a shared pressure core.
- **Lidar range** (`lidar_params/<range>`): min/max detectable distance plus radial resolution,
  mapping losslessly to SDF `lidar/<range>`.
- **Camera skew + lens model**: `camera_intrinsics/<s>` completes the 3×3 K matrix; a new `camera_lens`
  (`sensor_fov/<lens>`) names the projection model (`CameraProjectionType`: pinhole, stereographic,
  equidistant, equisolid, orthographic, custom) with cutoff angle and a custom mapping function,
  describing fisheye/wide-angle optics that Brown-Conrady distortion cannot.

### Bundling
- `.hcdfz` bundle: `hcdf bundle` packs a model + all its meshes into a self-contained, USDZ-shaped
  root-first `ZIP_STORED` archive (content-addressed `assets/`, flattened `<include>`s); `--dir` writes
  a loose git-diffable directory; `hcdf bundle-verify` rehashes every blob against its `@sha`.
- See [docs/design/bundling-and-remote-assets.md](docs/design/bundling-and-remote-assets.md).

### Remote (http/https) assets: opt-in, security-hardened by design
- The integrator chooses at bundle time: `--vendor-remote` (fetch + integrity-check + embed → a
  self-contained/offline bundle with no `http(s)` URIs), `--keep-remote` (leave live web URIs), or
  neither (a remote URI is an explicit error so the dependency is never silent).
- **Intentional security posture** (single audited fetch path in the Rust core, `hcdformat-rs::remote`,
  driven by `vendor_assets`): fetching is **opt-in only**; an `http(s)`-only **scheme allow-list enforced
  across redirects** (no SSRF via an `http → ftp://`/`file://` redirect); a **timeout** + streamed
  **size cap** (256 MiB); and a **content-addressed, sha-re-verified, atomically-written cache** so
  tampered/poisoned bytes are never embedded or rendered. No raw HTTP exists outside this helper.

### `<include>` composition
- `<include>` `@sha` is now **verified on resolve** (a mismatch is a *note*, never an error, since modules
  change as they are edited; no `@sha` ⇒ no verification) with content-addressed sha-256 hashing;
  `stamp_include_shas` pins includes for reproducible bundles.
- Gazebo `model://` mesh URIs resolve (via `--package <model>=<dir>` or `GZ_SIM_RESOURCE_PATH`).
- A module's relative `assets/<sha>` mesh URIs reroot to the module's own directory when composed, so a
  sub-assembly in a subdirectory keeps resolvable meshes.

### Validation & editor support
- `hcdf validate --xsd`: Rust schema-shape validation (the crates.io `uppsala` XSD validator, at parity
  with the former lxml path) that catches malformed appearance XML the kinematic validator misses
  (e.g. `<box>w d h</box>` instead of `<box><size>…`).
- `hcdf.completions.json` (+ generator): contextual v2 roots, types, and particles for schema-aware
  editors, preserving repeated local names with distinct content and attributes.

### Rust (`hcdformat-rs`)
- The `<include>` `flatten()` resolver is a wasm-safe loader core + native fs convenience (during
  migration it was matched field-for-field against the since-retired Python reference).
- **xacro expansion is now FULLY pure-Rust on every target (feature `xacro`).** `expand_xacro` /
  `from_xacro_*` are backed by the `xacro-pure` crate, a faithful, byte-identical pure-Rust port
  of canonical ROS `xacro` (rustpython-vm as the `${...}` engine) that expands BOTH the declarative SO-ARM
  class AND the programmatic OpenArm class to match `/opt/ros/*/bin/xacro` exactly. This RETIRES: the
  vendored patched `xacro-rs` + `pyisheval` forks (`vendor/`, deleted), the two `[patch.crates-io]` entries
  (downstream roots enabling `xacro` no longer replicate any patch), and the native canonical-`xacro`
  `std::process::Command` fallback (`expand_xacro_best` / `expand_xacro_canonical`, removed). There is now
  ZERO subprocess and ZERO Python anywhere in the xacro path, so OpenArm-class `.xacro` import works in
  the browser (wasm), which the prior hybrid could not do. `rustpython-vm` is pulled ONLY by the `xacro`
  feature (absent from a bare model build and from hcdviz).

### Rust is canonical: the pure-Python implementation is retired
- **`hcdformat` is now Rust-only.** The pure-Python implementation (parse/serialize, the numpy frame
  math, the lxml XSD path, `urdf_hcdf`/`sdf_io`/`hcdf.assets`/`hcdf_io`) has been **deleted**. The
  canonical core is the `hcdformat-rs` crate; the Python package is a **thin maturin/PyO3 wrapper**:
  shim packages + CLI trampoline + presentation formatting over the native `_hcdformat_rs` binding.
  There is **no pure-Python fallback** and no `HCDF_USE_BINDING` toggle; the native extension is
  required (a build/wheel without it cannot function, and CI self-certifies the binding is present).
- **Typed DOM handle.** `hcdfdom.Hcdf` is the PyO3 `PyHcdf` handle over the Rust model: parse,
  navigate, and serialize all go through Rust; there is no Python DOM re-implementation behind it.
- **JSON is Rust-backed.** HCDF ↔ JSON conversion (`hcdf convert … .json`) runs the Rust XSD-driven
  walker, not the former Python `convert.py`. It stays value-exact (JSON cannot keep trailing zeros).
- **`gz` (libsdformat) is retired.** No tool shells out to `gz`/libsdformat. The Rust `from_sdf` maps
  gz-flavored SDF directly, and there is no longer a libsdformat cross-check or `urdf_to_sdf` delegation.
- **The wheel is the binding-bearing maturin abi3 wheel** with **zero runtime dependencies**
  (lxml/numpy/trimesh/pygltflib are gone). The colcon package builds the same binding from source via
  `ament_cmake` + maturin (needs a Rust toolchain), replacing the old `ament_python`/setuptools path.
- **Two behavioural changes carried over from the Rust core** (values are equal; the representation/bytes
  change, pinned by committed goldens so they are never silent):
  - **Export format** (`to_urdf` / `to_sdf`): the canonical Rust serializer emits compact (single-line)
    XML, paired empty tags (`<link></link>`), a double-quoted XML declaration, and re-renders source
    numeric text (`0.10` → `0.1`). Pinned by `tests/golden/_export/`.
  - **Asset content addresses** (`to_glb` / `to_lean` / `bake` / `vendor_assets`): the Rust baker is at
    semantic (**not byte**) parity with the legacy `trimesh`/`pygltflib` path, so baked GLB/STL bytes
    and their `@sha` content addresses differ from the pre-1.0 Python baker. This is a **one-time
    asset-SHA rebake**: externally-built `.hcdfz` bundles must be re-baked. Pinned by
    `tests/golden/_baker/`. No in-repo bundle/HCDF pins a Python-baked SHA, so nothing in-repo breaks.

## 1.0 (2026-04-14)

Initial release of the Hardware Configuration Descriptive Format.

### Schema
- 147 complex types, 42 enumerations
- Root metadata: required name attribute, optional description/author/license/url elements
- Frame conventions: body-frame (FLU, FRD) and world-frame (ENU, NED) attributes on root
- Component: ip-rating attribute (IEC 60529 / NEMA), operating-temp element (range with min/max)
- 10 joint types: revolute, continuous, prismatic, fixed, ball, universal, planar, screw, cylindrical, free
- 11 sensor categories: inertial, optical, electromagnetic, RF, force, chemical, encoder, temperature, radiation, audio, tactile
- 10 motor types: bldc, brushed, stepper, servo, linear, solenoid, hydraulic, pneumatic, thrust, ice
- 5 power sources: battery, tank, fuel cell, solar, supercapacitor
- 7 dynamic surfaces: propeller, aerofoil, hydrofoil, control surface, wheel, track, gripper
- Spring element in transmission for compliant actuators (SEA, PEA, CPEA, AE-PEA, VSA)
- 4 topology types: link, bus, chain (switched + pass-through), mesh
- TSN stack: gPTP, Qbv, CBS, PSFP, FRER, ATS, MACsec
- HMI elements, camera calibration, motor load curves, encoder as sensor
- Framework-agnostic annotations (no PX4, ArduPilot, or ROS REP references)

### Extensions
- ROS 2 topic mapping (org.ros2)
- ros2_control hardware interface (org.ros2.control): typed extension schema
  (`extensions/hcdf-ext-ros2-control.xsd`) for the ros2_control surface: `<ros2-control>` with
  `<hardware>` (plugin + params) and per-joint/sensor/gpio `<command_interface>`/`<state_interface>`
  wiring. The URDF importer re-homes a `<ros2_control>` block under this typed root (schema-typed /
  validatable rather than an opaque blob) and the exporter renames it back, preserving round-trip
  fidelity. Distinct from org.ros2 (topic mapping).
- Gazebo simulation parameters (org.gazebosim): the SDF importer now re-homes a model/world's
  `<plugin>`s and `<physics>` into a TYPED `<gazebo-sim>` extension body (schema
  `extensions/hcdf-ext-gazebo.xsd`) instead of dropping them: `<physics type>`→`<engine>`,
  `max_step_size`/`real_time_factor`→hyphenated, plugins preserved verbatim under the lax `xs:any`.
  Two domains keep typed and opaque content from colliding: `org.gazebosim`
  carries the `<gazebo-sim>` body (validates against the schema), while `org.gazebosim.raw` is the
  opaque passthrough for content with no typed home, holding the URDF importer's verbatim `<gazebo reference>`
  residual and the SDF sensor sim-only fragments (camera `<clip>`, IMU dynamic-bias), which the URDF
  quarantine previously mislabeled `org.gazebosim` (raw `<gazebo>` bytes cannot validate against the
  `<gazebo-sim>`-rooted schema).
- IEEE 1722 AVTP configuration (org.ieee.1722)

### Tools
- XSD validation (Python lxml)
- Interactive spec browser generator (20 tabs)
- XML/JSON bidirectional converter (Python + Rust)
- JSON Schema generator
- Test suite (43 tests: valid, invalid, roundtrip)

### Examples
- Humanoid mobile base (5 networks, 19 devices)
- Drone quadrotor (ROS 2 + Gazebo extensions)
