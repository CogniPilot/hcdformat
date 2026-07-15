# HCDF: Hardware Configuration Descriptive Format

HCDF is an XML schema for describing cyber-physical robotic systems. It captures
both the physical robot (kinematics, dynamics, sensors, actuators, geometry) and
the cyber infrastructure (networking topology, deterministic scheduling, link security,
firmware identity, power distribution, device discovery). No other format covers
both domains in a single validated schema.

## Key Features

- **Kinematic tree** with 10 joint types (revolute, continuous, prismatic, fixed, ball, universal, planar, screw, cylindrical, free)
- **Sensors** across 11 categories with noise models, driver identification, and axis alignment (inertial, optical, EM, RF, force, chemical, encoder, temperature, radiation, audio, tactile)
- **Motors** with full electrical/mechanical specs (voltage, current, torque constant, velocity constant, load curve, solenoids)
- **Compliant actuators** with spring elements supporting SEA, PEA, CPEA, AE-PEA, and VSA architectures
- **Networking** with topology types (links, buses, chains, meshes), deterministic scheduling, clock synchronization, and link security
- **Connectivity** across point-to-point links, shared buses (CAN, I2C, SPI), bridged daisy-chains, and wireless meshes
- **Power budget** with battery, fuel cell, tank, solar panel, and supercapacitor sources
- **Dynamic surfaces** including aerofoils, propellers, wheels, tracks, and gripper contact surfaces
- **Geometry** with box, cylinder, sphere, capsule, cone, ellipsoid, mesh (GLB), and frustum (sensor FOV)
- **Model composition** via `<include>` with SHA integrity (verified on resolve), per-file metadata (name, author, license), and frame conventions (FLU/FRD, ENU/NED)
- **Bundling** into a portable, self-contained `.hcdfz` (USDZ-shaped), with an explicit, security-hardened choice for web-hosted assets (vendor-and-embed for offline use, or keep live). See [docs/design/bundling-and-remote-assets.md](docs/design/bundling-and-remote-assets.md)
- **XSD validation** with full schema documentation on every element and attribute

## Files

| File | Description |
|------|-------------|
| `hcdf.xsd` | HCDF XML Schema Definition (primary schema) |
| `hcdf-stream-profile.xsd` | Stream profile schema for `.streams.xml` files |
| `rust/hcdformat-rs/` | The canonical Rust core: parse/validate/convert/bundle/bake (the `hcdf` CLI lives here, feature `cli`) |
| `rust/hcdformat-py/` | The maturin/PyO3 wheel project: the compiled `hcdf` extension with its typed `.pyi` stubs (no Python source) |
| `spec.html` | Generated specification browser (regenerate: `hcdf regen spec hcdf.xsd spec.html`) |
| `examples/cps-comms-showcase.hcdf` | Communication and connectivity showcase across wired, wireless, and chained media |
| `examples/dm-j8009p-openarm-j1.hcdf` | OpenArm J1 actuator with exact GLB connector and pin representations |
| `examples/humanoid-mobile-base.hcdf` | Full humanoid mobile base with structural and connectivity examples |
| `examples/four-bar.hcdf` | Closed-loop four-bar linkage example |
| `examples/openarm-thor.hcdf` | OpenArm and Jetson Thor CAN-FD and power-network example |
| `examples/so-arm101-buses.hcdf` | Feetech STS and SO-ARM101 connectivity example |
| `hcdf.schema.json` | Generated JSON Schema (regenerate: `hcdf regen schema hcdf.xsd hcdf.schema.json`) |
| `examples/test-minimal.hcdf` | Minimal test exercising all major element types |
| `examples/profiles/operational.streams.xml` | Stream profile example |
| `docs/design/` | Design documents and architecture notes |

## Quick Start

The `hcdf` command (the Rust CLI, shipped by the wheel and the colcon build; see [Installing](#installing))
is the single entry point. Validate an HCDF file's schema shape:

```bash
hcdf validate --xsd examples/humanoid-mobile-base.hcdf
```

Browse the specification interactively:

```bash
hcdf regen spec hcdf.xsd spec.html
python3 -m http.server 8080
# Open http://localhost:8080/spec.html
```

Convert between XML and JSON (the JSON walker is Rust-backed):

```bash
hcdf convert examples/openarm-thor.hcdf openarm-thor.json
hcdf convert openarm-thor.json openarm-thor-roundtrip.hcdf
```

Pack a model and all its meshes into one portable file, then verify it:

```bash
hcdf bundle examples/humanoid-mobile-base.hcdf robot.hcdfz   # self-contained .hcdfz
hcdf bundle-verify robot.hcdfz                               # rehash blobs vs @sha

# web-hosted meshes/includes are an explicit choice (see the design doc):
hcdf bundle robot.hcdf robot.hcdfz --vendor-remote           # fetch + embed (offline-ready)
hcdf bundle robot.hcdf robot.hcdfz --keep-remote             # leave live web URIs
```

Validate the raw XML against the schema shape (catches malformed appearance markup that the
kinematic validator can't see):

```bash
hcdf validate --xsd robot.hcdf
```

## Installing

hcdformat is **Rust-canonical**: the Python API is a compiled native extension and the `hcdf`
command is the Rust CLI. There is no pure-Python fallback; the native extension is required.

### pip (the prebuilt wheel)

```bash
pip install hcdformat        # abi3 wheel: the compiled hcdf extension, no runtime deps
hcdf --help                  # the Rust CLI
python3 -c "import hcdf"      # the extension
```

The published artifact is the **maturin** abi3 wheel (one wheel covers CPython 3.10+). The `hcdf`
package is the native extension itself, shipped with the type stubs (`py.typed` + `.pyi`) and the
schema data and no Python source, and it pulls **no** runtime dependencies.

### ROS 2 / colcon (build from source)

The colcon package (`package.xml` → `ament_cmake`, driven by `CMakeLists.txt`) builds the binding **and**
the native `hcdf` CLI from source, so the build machine needs a **Rust toolchain**:

```bash
# one-time toolchain setup
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # cargo + rustc (https://rustup.rs)
pip install 'maturin>=1.7,<2'                                    # abi3 wheel builder

# then, in your workspace
colcon build --packages-select hcdformat
ros2 run hcdformat hcdf --help     # the native Rust CLI
```

colcon installs the binding into the overlay's Python path (`import hcdf`), the native `hcdf` binary
into `lib/hcdformat`, and the schema/reference data into `share/hcdformat`.

## Specification Browser

Open [spec.html](spec.html) in a web browser for an interactive, expandable view
of every element, attribute, type, and enumeration in the HCDF schema.

## Version

1.0

## License

Apache-2.0. Copyright 2026 CogniPilot Foundation
