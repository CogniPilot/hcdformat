# Topology Types

HCDF declares connectivity topologies directly at the document root. A topology states how a selected purpose and carrier relate named participants. It does not hide physical wiring, connectors, or forwarding behavior inside strings.

Every participant has a stable name and an exact structured endpoint reference:

```xml
<participant name="controller">
  <endpoint><port-ref component="controller" port="can0"/></endpoint>
</participant>
```

The seven topology elements are `link`, `bus`, `chain`, `star`, `ring`, `mesh`, and `tree`.

| Topology | Meaning | Typical uses |
|---|---|---|
| `link` | Exactly two direct participants | Ethernet, USB, UART, DShot, optical or RF point-to-point |
| `bus` | One shared medium with two or more participants | CAN, RS-485, I2C, STS servos, 10BASE-T1S, power rails |
| `chain` | Ordered forwarding hops with one leg per adjacent hop | Switched Ethernet, EtherCAT, PROFINET line |
| `star` | One declared coordinator and two or more participants | Hub, access point, or centrally distributed service |
| `ring` | Closed ordered forwarding path | MRP or other redundant forwarding rings |
| `mesh` | Shared membership with runtime-selected paths | Thread, BLE mesh, WiFi mesh, UWB peer networks |
| `tree` | Rooted acyclic forwarding path | Branched switched networks and distribution trees |

## Link

A link connects exactly two functional endpoints. The selected block records the operating purpose, carrier, profiles, and quantities. Port capabilities state what each endpoint can support.

```xml
<link name="compute_to_sensor">
  <description>Dedicated 100BASE-T1 sensor connection.</description>
  <selected purpose="communication" carrier="electrical">
    <rate><nominal value="100000000" unit="bit/s"/></rate>
  </selected>
  <participant name="compute">
    <endpoint><port-ref component="compute" port="eth0"/></endpoint>
  </participant>
  <participant name="sensor">
    <endpoint><port-ref component="sensor" port="eth0"/></endpoint>
  </participant>
</link>
```

The same topology works for guided optical, conducted RF, radiated RF, liquid, or gas by selecting the correct carrier and endpoint capabilities.

## Bus

A bus is one shared medium. Physical wiring may be a trunk, daisy chain, backplane, or busbar, but participants do not forward traffic merely to keep the medium connected.

```xml
<bus name="main_can">
  <description>CAN-FD control bus.</description>
  <selected purpose="communication" carrier="electrical">
    <profile id="hcdf:can"/>
    <rate><nominal value="1000000" unit="bit/s"/></rate>
  </selected>
  <participant name="controller">
    <endpoint><port-ref component="controller" port="can0"/></endpoint>
  </participant>
  <participant name="wheel-drive">
    <endpoint><port-ref component="wheel-drive" port="can0"/></endpoint>
  </participant>
  <participant name="bms">
    <endpoint><port-ref component="bms" port="can0"/></endpoint>
  </participant>
</bus>
```

CAN termination is physical hardware, not a bus attribute. Declare endpoint connectors and `termination` objects, then bind the functional ports to those connectors.

Power and material distribution use the same structure with a different selection:

```xml
<bus name="fuel_supply">
  <selected purpose="material-transfer" carrier="liquid">
    <pressure><range min="250000" max="400000" nominal="300000" unit="Pa"/></pressure>
    <flow><nominal value="0.002" unit="m3/s"/></flow>
  </selected>
  <!-- structured participants -->
</bus>
```

## Chain

A chain describes active forwarding. Hops identify components or named forwarding functions. Legs explicitly pair the participant endpoint used at each end of every adjacent hop. This distinguishes an ingress from an egress without path strings.

```xml
<chain name="arm-chain">
  <selected purpose="communication" carrier="electrical"/>

  <participant name="root-out">
    <endpoint><port-ref component="root" port="eth0"/></endpoint>
  </participant>
  <participant name="node-in">
    <endpoint><port-ref component="node" port="eth0"/></endpoint>
  </participant>
  <participant name="node-out">
    <endpoint><port-ref component="node" port="eth1"/></endpoint>
  </participant>
  <participant name="tail-in">
    <endpoint><port-ref component="tail" port="eth0"/></endpoint>
  </participant>

  <hop name="root"><owner><component-ref component="root"/></owner></hop>
  <hop name="node"><owner><function-ref component="node" function="forwarder"/></owner></hop>
  <hop name="tail"><owner><component-ref component="tail"/></owner></hop>

  <leg name="root-to-node">
    <from><hop-ref network="arm-chain" hop="root"/><participant-ref network="arm-chain" participant="root-out"/></from>
    <to><hop-ref network="arm-chain" hop="node"/><participant-ref network="arm-chain" participant="node-in"/></to>
  </leg>
  <leg name="node-to-tail">
    <from><hop-ref network="arm-chain" hop="node"/><participant-ref network="arm-chain" participant="node-out"/></from>
    <to><hop-ref network="arm-chain" hop="tail"/><participant-ref network="arm-chain" participant="tail-in"/></to>
  </leg>
</chain>
```

The component declares the forwarding function separately:

```xml
<switch name="forwarder">
  <input><port-ref component="node" port="eth0"/></input>
  <output><port-ref component="node" port="eth1"/></output>
</switch>
```

A branch is a separate link or a `tree`; it is not embedded as an unstructured spur inside a chain hop.

## Star

A star names one participant as coordinator. This expresses the operational center without pretending all star-shaped wiring is a shared electrical bus.

```xml
<star name="wifi-infrastructure">
  <selected purpose="communication" carrier="radiated-rf">
    <rf><channel><numbered number="36"/></channel></rf>
  </selected>
  <coordinator><participant-ref network="wifi-infrastructure" participant="access-point"/></coordinator>
  <participant name="access-point"><endpoint><port-ref component="ap" port="wifi0"/></endpoint></participant>
  <participant name="robot"><endpoint><port-ref component="robot" port="wifi0"/></endpoint></participant>
</star>
```

The RF ports may be related to `antenna` objects, conducted feeds, connectors, and representations on their owning components.

## Ring and Tree

`ring` uses the same participant, hop, and leg model as `chain`, with a closing leg and at least three hops. `tree` adds an explicit root hop and requires one incoming leg for every other hop. These forms make adjacency and routing constraints machine-checkable.

## Mesh

A mesh declares membership while runtime protocols select paths dynamically.

```xml
<mesh name="hand-sensors">
  <selected purpose="communication" carrier="radiated-rf">
    <frequency><range min="2400000000" max="2483500000" unit="Hz"/></frequency>
  </selected>
  <participant name="controller"><endpoint><port-ref component="controller" port="wpan0"/></endpoint></participant>
  <participant name="thumb"><endpoint><port-ref component="thumb" port="wpan0"/></endpoint></participant>
  <participant name="palm"><endpoint><port-ref component="palm" port="wpan0"/></endpoint></participant>
</mesh>
```

## Physical Wiring and Logical Topology

Logical topology and physical construction are orthogonal:

| Physical construction | Forwarding behavior | Topology |
|---|---|---|
| One direct path | None | `link` |
| Shared trunk or rail | None | `bus` |
| Daisy chain through active devices | Ordered | `chain` |
| Closed active path | Ordered with closure | `ring` |
| Branched active path | Rooted | `tree` |
| Central coordinator | Coordinator-defined | `star` |
| Runtime route selection | Dynamic | `mesh` |

Use `connector`, `binding`, `mate`, physical assemblies, paths, junctions, and terminations to describe how the topology is physically realized. Do not encode physical construction in topology names or endpoint strings.
