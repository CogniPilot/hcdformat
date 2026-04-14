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
- **Model composition** via `<include>` with SHA integrity, per-file metadata (name, author, license), and frame conventions (FLU/FRD, ENU/NED)
- **XSD validation** with full schema documentation on every element and attribute

## Files

| File | Description |
|------|-------------|
| `hcdf.xsd` | HCDF XML Schema Definition (primary schema) |
| `hcdf-stream-profile.xsd` | Stream profile schema for `.streams.xml` files |
| `validate.py` | Python validator using lxml |
| `generate_spec_html.py` | Generates interactive spec browser from XSD |
| `spec.html` | Generated specification browser (open in browser) |
| `examples/humanoid-mobile-base.hcdf` | Full humanoid mobile base example (5 networks) |
| `examples/drone-quadrotor.hcdf` | Drone quadrotor with ROS 2 and Gazebo extensions |
| `convert.py` | Bidirectional XML/JSON converter (Python) |
| `generate_json_schema.py` | Generates JSON Schema from XSD |
| `hcdf.schema.json` | Generated JSON Schema |
| `tools/hcdf-cli/` | Rust CLI: convert, validate, info, schema |
| `examples/test-minimal.hcdf` | Minimal test exercising all major element types |
| `examples/profiles/operational.streams.xml` | Stream profile example |
| `docs/design/` | Design documents and architecture notes |

## Quick Start

Validate an HCDF file against the schema:

```bash
python3 validate.py hcdf.xsd examples/humanoid-mobile-base.hcdf
```

Validate a stream profile:

```bash
python3 validate.py hcdf-stream-profile.xsd examples/profiles/operational.streams.xml
```

Browse the specification interactively:

```bash
python3 generate_spec_html.py hcdf.xsd spec.html
python3 -m http.server 8080
# Open http://localhost:8080/spec.html
```

Convert between XML and JSON:

```bash
# Python
python3 convert.py examples/drone-quadrotor.hcdf drone.json
python3 convert.py drone.json drone-roundtrip.hcdf

# Rust
cd tools/hcdf-cli && cargo run -- convert -i ../../examples/drone-quadrotor.hcdf -o drone.json
```

## Specification Browser

Open [spec.html](spec.html) in a web browser for an interactive, expandable view
of every element, attribute, type, and enumeration in the HCDF schema.

## Version

1.0.0

## License

Apache-2.0. Copyright 2026 CogniPilot Foundation
