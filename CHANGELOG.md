# Changelog

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
