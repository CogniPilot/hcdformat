# Topology Types

HCDF uses four connectivity elements inside `<network>` to describe how devices communicate. The choice depends on how data moves between devices on the physical medium.

## The Four Types

| Type | Physical | Logical | Latency | Example |
|------|----------|---------|---------|---------|
| **Link** | Point-to-point | Direct connection | Fixed (one hop) | Ethernet, USB, UART direct |
| **Bus** | Shared medium | All devices hear all traffic | Same for all participants | CAN, I2C, SPI, STS servo, 10BASE-T1S |
| **Chain** | Daisy-chain with forwarding | Each hop stores-and-forwards | Accumulates per hop | TSN Ethernet, EtherCAT, PROFINET |
| **Mesh** | Self-organizing wireless | Dynamic multi-hop routing | Variable | Thread, BLE mesh, UWB |

## Link: Point-to-Point

A direct connection between exactly two ports. No forwarding, no sharing. Use for Ethernet (non-daisy-chained), USB, UART, and dedicated connections between two devices.

```xml
<link name="compute_to_sensor">
  <wired standard="100base-t1">
    <port>s32n79/eth0:2</port>
    <port>optical-flow/ETH0</port>
    <vlan id="10" pcp="5"/>
  </wired>
</link>
```

## Bus: Shared Medium

Multiple devices connected to the same electrical medium. Every device sees every message simultaneously. The physical wiring is often daisy-chain, but logically the medium is shared. No device actively forwards data.

Use for CAN/CAN-FD, I2C, SPI, servo buses (STS, Dynamixel), 10BASE-T1S multidrop Ethernet, LIN, RS-485, and power distribution.

```xml
<bus name="main_can" type="CAN" topology="daisy-chain">
  <bitrate>5000000</bitrate>
  <participant device="s32j100" port="can0" role="controller" position="1"/>
  <participant device="wheel-drive" port="can0" position="2" id="0x601"/>
  <participant device="bms" port="can0" position="3" id="0x610" terminator="true"/>
</bus>
```

## Chain: Bridged Daisy-Chain

An ordered sequence of devices where each device has an internal switch that actively forwards frames to the next hop. Unlike a bus, devices do not all see traffic simultaneously. Latency accumulates with each hop.

Two modes:
- **Switched** (`mode="switched"`): Store-and-forward at each hop. Used for TSN Ethernet where each motor control node has a bridging switch.
- **Pass-through** (`mode="pass-through"`): Process-on-the-fly. Used for EtherCAT where each slave's ASIC processes frames in hardware as they pass through (~1 us per hop).

```xml
<chain name="left-arm" type="802.3dm" mode="switched">
  <vlan id="10" pcp="5"/>
  <hop device="s32n79" port="xfi_left" role="root"/>
  <hop device="mcn1-l" ingress="eth0" egress="eth1"/>
  <hop device="mcn2-l" ingress="eth0" egress="eth1"/>
  <hop device="mcn3-l" ingress="eth0" egress="eth1">
    <spur device="cam-l" port="eth0" via="eth2"/>
  </hop>
  <hop device="mcn4-l" ingress="eth0" role="tail"/>
</chain>
```

The hop order defines the physical chain topology. An agent traces the path to compute per-hop latency and validate bandwidth at each segment.

## Mesh: Self-Organizing Wireless

A wireless network where nodes discover paths dynamically and can self-heal when nodes join or leave. Unlike buses (shared medium, no routing) and chains (fixed forwarding order), mesh networks handle topology autonomously at runtime. The HCDF declares membership and protocol; the routing protocol handles the rest.

Use for Thread/802.15.4 sensor meshes, BLE mesh sensor arrays, UWB positioning networks, and WiFi mesh.

```xml
<mesh name="hand-sensors" standard="802.15.4" protocol="Thread">
  <node device="mcn4-l" antenna="wpan0" role="coordinator"/>
  <node device="tactile-thumb" antenna="wpan0"/>
  <node device="tactile-index" antenna="wpan0"/>
  <node device="tactile-palm" antenna="wpan0"/>
</mesh>
```

## Physical Wiring vs. Logical Topology

Many networks are physically daisy-chained but logically a bus or chain. The distinction is whether each device actively forwards:

| Physical wiring | Active forwarding? | HCDF element |
|-----------------|-------------------|-------------|
| Point-to-point | N/A | `<link>` |
| Daisy-chain | No (shared electrical medium) | `<bus>` |
| Daisy-chain | Yes (store-and-forward bridge) | `<chain>` |
| Star | N/A | Multiple `<link>` elements |
| Ring | Yes | `<chain topology="ring">` |
| Self-organizing | Dynamic | `<mesh>` |

## Protocol Mapping

| Protocol | Element | Why |
|----------|---------|-----|
| Ethernet (two devices) | `<link>` | Point-to-point |
| Ethernet (TSN daisy-chain) | `<chain mode="switched">` | Each device bridges |
| EtherCAT | `<chain mode="pass-through">` | Each slave processes in hardware |
| PROFINET line | `<chain mode="switched">` | Each device is a 2-port switch |
| CAN/CAN-FD | `<bus>` | Shared differential pair |
| I2C | `<bus>` | Shared SDA/SCL |
| SPI (shared) | `<bus>` | Shared MISO/MOSI/CLK |
| STS/Dynamixel servo | `<bus>` | Half-duplex UART, shared wire |
| 10BASE-T1S multidrop | `<bus>` | 802.3cg PLCA shared Ethernet |
| Thread/BLE mesh | `<mesh>` | Self-organizing wireless |
| Power distribution | `<bus type="power">` | Shared voltage rail |
