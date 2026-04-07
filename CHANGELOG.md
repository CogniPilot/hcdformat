# Changelog

## 1.0.0 (2026-04-07)

Initial release of the Hardware Configuration Descriptive Format.

### Schema
- 142 complex types, 39 enumerations
- 9 top-level elements: comp, joint, group, state, self-collision-disable, network, transmission, material, include
- 10 sensor categories: inertial, optical, electromagnetic, RF, force, chemical, encoder, temperature, radiation, audio
- 8 joint types: revolute, continuous, prismatic, fixed, ball, universal, planar, screw
- 5 topology types: link, bus, chain (switched + pass-through), mesh
- HMI elements, dynamic surfaces, power sources (battery, tank, fuel cell)
- Camera calibration, motor load curves, encoder as sensor

### Extensions
- ROS 2 topic mapping (org.ros2)
- Gazebo simulation parameters (org.gazebosim)
- IEEE 1722 AVTP configuration (org.ieee.1722)

### Tools
- XSD validation (Python lxml)
- Interactive spec browser generator (20 tabs)
- XML/JSON bidirectional converter (Python + Rust)
- JSON Schema generator
- Test suite (23 tests: valid, invalid, roundtrip)

### Examples
- Humanoid mobile base (2297 lines, 5 networks, 19 devices)
- Drone quadrotor (947 lines, ROS 2 + Gazebo extensions)
