# Stream Profiles

## Purpose

HCDF separates installed connectivity from operational data flow:

- An `.hcdf` document declares components, ports, functions, physical connectivity, networks, and network configuration.
- A `.streams.xml` document declares end-to-end streams over those existing networks.

The hardware description remains stable when an application changes mode. Different profiles can describe normal operation, calibration, degraded operation, or diagnostics without duplicating physical topology.

The HCDF root declares profile resources:

```xml
<hcdf name="robot" version="1.0">
  <!-- Components and connectivity omitted. -->
  <stream-profile uri="profiles/operational.streams.xml"
                  selection-role="default"/>
  <stream-profile uri="profiles/calibration.streams.xml"
                  required="false"/>
</hcdf>
```

Profile URIs resolve relative to the declaring HCDF document. A profile can declare nested profile dependencies. Resource loading applies the same structured include-instance, digest, cycle, and work-limit rules used by the document set.

## Streams

A stream is one unidirectional end-to-end flow. It has exactly one talker and one or more listeners. Multiple listeners describe multicast without creating artificial per-segment streams.

```xml
<stream-profile name="operational" version="1.0">
  <stream name="joint-command"
          vlan-id="10"
          pcp="5"
          max-frame-size-bytes="192"
          interval-ns="1000000"
          max-latency-ns="200000"
          protocol="ieee:1722">
    <path>
      <network-ref network="arm-network"/>
    </path>
    <talker>
      <participant-ref network="arm-network" participant="controller"/>
    </talker>
    <listener>
      <participant-ref network="arm-network" participant="joint-1"/>
    </listener>
    <listener>
      <participant-ref network="arm-network" participant="joint-2"/>
    </listener>
  </stream>
</stream-profile>
```

The path references an existing HCDF network. Its topology can be a link, bus, chain, star, ring, mesh, or tree. The stream profile does not restate that topology.

Stream scalar values use their protocol domains. `vlan-id` is 0 through 4094, with 0 available for priority-tagged frames and 4095 reserved. `pcp` is 0 through 7. `max-frame-size-bytes`, `interval-ns`, and an authored `max-latency-ns` must be greater than zero. Declaring `frer` requires at least two seamless trees because it represents redundant delivery.

## Multi-network paths

A path spanning multiple networks is a directed forwarding graph. It lists every participating network and declares each inter-network forwarding relation explicitly.

```xml
<stream name="camera-multicast"
        max-frame-size-bytes="9000"
        interval-ns="33333333">
  <path>
    <network-ref network="camera-link"/>
    <network-ref network="control-lan"/>
    <network-ref network="logging-lan"/>

    <forwarding>
      <from>
        <participant-ref network="camera-link" participant="gateway-camera"/>
      </from>
      <function-ref component="gateway" function="camera-bridge"/>
      <to>
        <participant-ref network="control-lan" participant="gateway-control"/>
      </to>
    </forwarding>

    <forwarding>
      <from>
        <participant-ref network="camera-link" participant="gateway-camera"/>
      </from>
      <function-ref component="gateway" function="camera-bridge"/>
      <to>
        <participant-ref network="logging-lan" participant="gateway-logging"/>
      </to>
    </forwarding>
  </path>

  <talker>
    <participant-ref network="camera-link" participant="camera"/>
  </talker>
  <listener>
    <participant-ref network="control-lan" participant="controller"/>
  </listener>
  <listener>
    <participant-ref network="logging-lan" participant="recorder"/>
  </listener>
</stream>
```

The declaration order of `network-ref` and `forwarding` elements does not imply connectivity. Each forwarding element supplies the directed relation.

## Exact forwarding relation

A forwarding relation names three canonical objects:

1. A participant on the source network.
2. A connectivity function declared by a component.
3. A participant on the target network.

The HCDF connectivity graph must prove both sides of the relation:

- The source participant's exact port or channel is an input or bidirectional endpoint of the function.
- The target participant's exact port or channel is an output or bidirectional endpoint of the function.

This works for bridges, switches, converters, transceivers, and radios. Matching component names are never treated as proof of forwarding.

For example, the referenced HCDF component can declare:

```xml
<comp name="gateway">
  <port name="camera"/>
  <port name="control"/>
  <port name="logging"/>
  <bridge name="camera-bridge">
    <input><port-ref component="gateway" port="camera"/></input>
    <output><port-ref component="gateway" port="control"/></output>
    <output><port-ref component="gateway" port="logging"/></output>
  </bridge>
</comp>
```

## Route validity

The normalized route graph enforces these invariants:

- Every path network resolves through a structured reference and appears only once.
- The talker's network is the single route root and must be listed in the path.
- Every listener's network is listed and reachable from the talker's network.
- Each forwarding connects two distinct listed networks.
- Each canonical source participant, function, and target participant triple appears only once.
- Every forwarding relation is proven by exact participant and function endpoint edges.
- The forwarding graph is acyclic.
- Every listed network lies on at least one directed route from the talker network to a listener network.

The graph may branch for multicast and reconverge for redundant delivery. IEEE 802.1CB requirements can be attached with `frer` without splitting the end-to-end flow:

```xml
<frer seamless-trees="2" sequence-encoding="r-tag"/>
```

## Structured instance references

Any network, participant, function, group, traffic-class, or schedule reference can target a repeated HCDF include instance. The same structured instance form is used everywhere:

```xml
<network-ref network="arm-network">
  <instance>
    <segment name="arm-module" occurrence="1"/>
  </instance>
</network-ref>
```

String path parsing and slash-delimited component references are not part of the stream-profile vocabulary.

## Groups and network configuration

`stream-group` provides organizational grouping. A stream can reference one group through `group-ref`.

`traffic-class-ref` and `schedule-ref` refer to configuration declared by any network listed in the stream path. They do not create configuration and do not imply a route.

```xml
<traffic-class-ref network="control-lan" traffic-class="scheduled-control"/>
<schedule-ref network="control-lan" schedule="control-cycle"/>
```

## Canonical graph projection

Loaded profiles add immutable profile, group, stream, and stream-forwarding nodes to the normalized connectivity graph. Stable forwarding identity derives from the canonical source participant, function, and target participant references, not declaration position. Exact edges retain:

- Stream membership in every path network.
- Talker and listener participants.
- Stream groups and network configuration references.
- Each forwarding node's source participant, function, and target participant.

If a required profile or any stream route is invalid, structural projection remains available while connectivity projection is invalidated transactionally.

## File organization

```text
robot-project/
|-- robot.hcdf
|-- profiles/
|   |-- operational.streams.xml
|   |-- calibration.streams.xml
|   `-- safe-mode.streams.xml
`-- models/
    `-- robot.glb
```
