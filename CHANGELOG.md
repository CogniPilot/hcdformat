# Changelog

## 1.1.0 (2026-04-14)

Schema completeness additions based on deep review.

### Schema
- Root metadata: required name attribute, optional description/author/license/url elements
- Frame conventions: body-frame (FLU, FRD) and world-frame (ENU, NED) attributes on root
- Component: ip-rating attribute (IEC 60529 / NEMA), operating-temp element (range with min/max)
- Solenoid added to MotorType enum for on/off actuators (valves, latches, locks)
- Tactile sensor category (11th): capacitive, resistive, piezoelectric, barometric, optical
- Solar panel and supercapacitor power sources alongside battery, tank, fuel cell
- Cylindrical joint (rotation + translation on same axis) and free joint (6-DOF unconstrained)
- Gripper surface type: mechanical, suction, magnetic, adhesive, jamming
- Spring element in transmission for compliant actuators (SEA, PEA, CPEA, AE-PEA, VSA)
- Removed framework-specific references (PX4, ArduPilot, ROS REP) from annotations
- 147 complex types, 42 enumerations (up from 142/39)

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
- Test suite (43 tests: valid, invalid, roundtrip)

### Examples
- Humanoid mobile base (5 networks, 19 devices)
- Drone quadrotor (ROS 2 + Gazebo extensions)
