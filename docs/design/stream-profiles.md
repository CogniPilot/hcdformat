# Stream Profiles

## Why Streams Are Separate

HCDF separates hardware description from application-level data flows. The `.hcdf` file describes what the system *is* (topology, capabilities, devices). A `.streams.xml` file describes what the system *does* (which data flows where, at what rate, with what latency requirements).

This separation exists because:

1. **Same hardware, different applications.** A robot's physical wiring doesn't change when you switch from operational mode to calibration mode. But the streams do: calibration may disable motor commands and enable only sensor streams.

2. **Streams are an application concern.** A system integrator defines the hardware topology. A controls engineer defines the data flows. These are different people making different decisions at different times.

3. **Multiple profiles coexist.** A single robot can have `operational.streams.xml`, `calibration.streams.xml`, `safe-mode.streams.xml`, and `demo.streams.xml`. The HCDF references all of them; one is active at a time.

4. **Agent workflow.** An agent reads the HCDF for topology and capabilities, reads the active stream profile for QoS requirements, and computes per-device network configuration (schedules, shapers, filters) from the combination.

## How It Works

The HCDF references stream profiles inside a `<network>` element:

```xml
<network name="left-arm">
  <chain name="left-arm-chain" type="802.3dm" mode="switched">...</chain>
  <gptp domain="0">...</gptp>
  <schedule name="arm-1ms" cycle-time-us="1000">...</schedule>
  <stream-profile uri="profiles/operational.streams.xml" active="true"/>
  <stream-profile uri="profiles/calibration.streams.xml" active="false"/>
</network>
```

The stream profile file is a separate XML document with its own XSD (`hcdf-stream-profile.xsd`):

```xml
<?xml version="1.0"?>
<stream-profile name="operational" version="1.0" hcdf-ref="humanoid-mobile-base.hcdf">
  <stream-group name="left-arm-control" chain="left-arm-chain">
    <stream name="cmd_mcn1l" talker="s32n79/xfi_left" listener="mcn1-l/sgmii0"
            vlan="10" pcp="5" max-frame-size="192" interval-us="1000" max-latency-us="200"/>
    <stream name="status_mcn1l" talker="mcn1-l/sgmii0" listener="s32n79/xfi_left"
            vlan="10" pcp="5" max-frame-size="384" interval-us="1000" max-latency-us="200"/>
  </stream-group>
</stream-profile>
```

## Stream Elements

Each stream defines a unidirectional data flow:

| Attribute | Purpose |
|-----------|---------|
| `name` | Unique identifier for this stream |
| `talker` | Source port (device/port format) |
| `listener` | Destination port |
| `vlan` | VLAN ID for traffic isolation |
| `pcp` | Priority Code Point (maps to traffic class) |
| `max-frame-size` | Maximum Ethernet frame size in bytes |
| `interval-us` | Transmission interval in microseconds |
| `max-latency-us` | Required maximum end-to-end latency |
| `protocol` | Transport protocol if not raw Ethernet (e.g., `ieee-1722-acf-can`) |

## Stream Groups

Streams can be organized into groups that correspond to functional subsystems. A group can reference an HCDF chain, which tells the agent the path these streams traverse without having to trace each one individually.

## Agent Computation

Given the HCDF topology and an active stream profile, an agent:

1. Traces each stream's path through the network (using chain hop order)
2. Computes per-hop serialization delay + switch processing delay
3. Validates cumulative latency against `max-latency-us`
4. Validates per-link bandwidth (sum of stream bandwidths per hop)
5. Generates per-port gate control lists, traffic shaping, and policing rules
6. Pushes configuration to devices via YANG or device-specific protocols

## File Organization

```
robot-project/
├── robot.hcdf                        # Hardware description
├── profiles/
│   ├── operational.streams.xml       # Normal operation
│   ├── calibration.streams.xml       # Sensor-only mode
│   └── safe-mode.streams.xml         # Safety heartbeat only
└── models/
    └── *.glb                         # 3D model files
```
