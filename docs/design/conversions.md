# Converting between URDF, SDF, and HCDF

This explains how the converter moves robot descriptions between URDF, SDF, and HCDF, what each
direction preserves, and what it cannot. HCDF is the center. URDF and SDF are peers that both talk
to it through one neutral typed model. There is no direct URDF to SDF logic of our own and no SDF to
URDF path at all. Everything routes through the hub.

```
  URDF  ──urdf_hcdf──┐                                  ┌──urdf_hcdf──►  URDF   (+ profile tiers)
                     │                                  │
   SDF  ──sdf_io─────┼───►   hcdfdom (neutral typed IR) ┼──sdf_io─────►  SDF    (+ gz validate)
                     │                                  │
                     └── hcdf_io: JSON, include flatten ─┘
                          assets: mesh ──► GLB (content addressed)
                          validate: tree and references   ·   frames: pose math
```

Both `urdf_hcdf` and `sdf_io` are spokes on the same in memory model. A conversion is always two
halves: read a format into the model, then write the model out to a format. Round trips are just the
two halves run back to back.

## The hub: hcdfdom

`hcdfdom` is the neutral typed model that every conversion reads into and writes out of. It has four
layers:

1. A typed model generated from the canonical `hcdf.xsd` schema, so it never drifts from the spec.
2. XML parse and serialize that is lossless. Leaf values are kept as their original strings, so
   `0.30` stays `0.30` and never becomes `0.3`.
3. A validator for the things XML schema cannot check on its own: the kinematic tree is acyclic with
   one root, every reference resolves, joint types carry the right fields.
4. A pose and frame normalizer built on numpy.

```python
from hcdfdom import load, dump, loads, dumps
from hcdfdom.validate import validate, is_valid
```

## What converts to what

| From | To | Entry point | Fidelity |
|------|----|-------------|----------|
| URDF | HCDF | `urdf_hcdf.from_urdf` | URDF is absorbed in full. All of its content survives. |
| HCDF | URDF | `urdf_hcdf.to_urdf` | Lossy by nature. HCDF says more than URDF can. Every drop is recorded. |
| SDF  | HCDF | `sdf_io.from_sdf` | Maps the mechanical overlap. The rest is recorded in notes. |
| HCDF | SDF  | `sdf_io.to_sdf` | Cleaner than URDF. Only the cyber layer is lost, and it is recorded. |
| HCDF | JSON | `hcdf_io.to_json` | Value exact. |
| JSON | HCDF | `hcdf_io.from_json` | Value exact. |
| URDF | SDF  | `sdf_io.gz.urdf_to_sdf` | Delegated to libsdformat. This is the canonical gazebo path. |
| SDF  | URDF | none | Does not exist. SDF is wider than URDF and the inverse is not well defined. |

Each writer returns its output plus a loss manifest. Each reader returns the model plus a list of
notes. Nothing is ever dropped in silence.

## Frames

URDF and SDF are both forward left up for body axes and east north up for the world. HCDF may use
forward right down and north east down. Two rules follow from that.

On import from URDF or SDF there is no transform. The document is tagged forward left up and east
north up and the numbers pass through untouched.

On export, a forward right down body is converted to forward left up. Poses and joint axis vectors
are both converted. The change of basis is `diag(1, -1, -1)`. A north east down world is reported as
a loss for now and is not yet converted, while the body conversion is applied. Rotations use roll
pitch yaw as XYZ extrinsic, or a Hamilton quaternion written `x y z w` where the quaternion wins if
both are present.

Example. A joint axis `0 1 0` in a forward right down body becomes `0 -1 0` on export to URDF or SDF.
A quaternion orientation is written back as roll pitch yaw because neither URDF nor SDF origins carry
a quaternion. The rotation is exact, only the spelling changes.

## URDF

### Reading URDF into HCDF

`from_urdf(path or text or bytes)` returns `(doc, notes)`. It reads with lxml and maps the kinematic
core directly.

```
<robot>   becomes  <hcdf>      <link>   becomes  <comp>      <joint>  becomes  <joint>
```

A URDF material maps to and from an HCDF color with no loss:

```xml
URDF                                       HCDF
<material name="red">                      <color name="red" rgba="1 0 0 1"/>
  <color rgba="1 0 0 1"/>
</material>                                <visual><color name="red"/></visual>
<visual><material name="red"/></visual>
```

Anything that is not a `link`, `joint`, or `material` at the top level is quarantined verbatim into a
root `<extension>` grouped by domain, before the schema sees it. A `<gazebo>` block goes to
`org.gazebosim`, a `<transmission>` to `org.ros.control`, `ros2_control` to `org.ros2.control`. On
export those are restored to their native top level form, so a URDF that round trips reproduces them
exactly.

```xml
<gazebo reference="link1"> ... </gazebo>
   becomes   <extension domain="org.gazebosim"> ... </extension>    (raw, kept intact)
```

Mesh visuals become a GLB model reference. Primitive visuals become a primitive shape plus a flat
color. A visual mesh is the appearance, so HCDF treats it as a model, and a primitive visual is a
shape that can take a color. The two cannot both appear on one visual.

### Writing HCDF out to URDF

`to_urdf(doc)` returns `(urdf_xml, loss)`. It writes URDF at the lxml string level, never through a
lossy exporter. HCDF is a superset, so some content has no URDF home. The loss manifest names every
one. Examples of what gets dropped and recorded:

* A closed loop joint. URDF is a tree only and cannot express it.
* A ball, universal, screw, or cylindrical joint. These downgrade to `fixed` and the axis, limit, and
  thread pitch are recorded as dropped.
* A capsule, cone, or ellipsoid shape. URDF has no such primitive.
* A collision surface, sensors, motors, networks, power sources, named frames, groups, states. None
  of these exist in URDF.

Pure metadata such as a description or a document author is recorded under an `annotation` category
that does not count as model loss.

### The URDF profile

`urdf_hcdf.check_profile(doc)` answers one question: if I export this to URDF, what do I get back.
It returns one of three tiers.

```
IN-PROFILE-IDENTITY              exports to clean URDF and round trips with nothing lost
IN-PROFILE-WITH-FRAME-TRANSFORM  same, after a frame conversion, a GLB bake, or an include flatten
OUT-OF-PROFILE                   exporting drops real model content, so the URDF is a lossy projection
```

The classifier and the loss manifest agree by contract. Any loss that is real model content forces
the out of profile tier, so a document that loses content can never be labelled identity. A PR2
imported from URDF lands at the with transform tier with zero real losses, because its only reason
not to be identity is that its mesh visuals are now GLB models.

```
python3 -m urdf_hcdf.profile robot.hcdf          # prints a readable report, exit 0 if in profile
python3 -m urdf_hcdf.profile robot.hcdf --json    # machine readable
```

## SDF

SDF is a wider peer than URDF. The reader and writer map the mechanical overlap value exactly and
record everything else.

### Reading SDF into HCDF

`from_sdf(path or text or bytes)` returns `(doc, notes)`.

```
<model>  becomes  <hcdf>     <link>  becomes  <comp>     <joint>  becomes  <joint>
```

A collision surface maps cleanly:

```xml
SDF                                            HCDF
<surface>                                      <surface>
  <friction><ode><mu>0.8</mu></ode></friction>   <friction static="0.8"/>
  <bounce><restitution_coefficient>0.2 ...      <restitution>0.2</restitution>
</surface>                                     </surface>
```

The parser recovers from imperfect input. libsdformat output for older models can carry tags with an
undeclared namespace prefix, such as a legacy `sensor:contact`. The reader still imports the
mechanical content and notes the rest. Content that HCDF keeps out of its core on purpose is noted,
not mapped: worlds, several or nested models, lights, actors, pose graphs that use `relative_to` or
`expressed_in`, a link pose that is not identity, and material PBR.

### Writing HCDF out to SDF

`to_sdf(doc)` returns `(sdf_xml, loss)` written at the string level. Because SDF says more than URDF,
this export keeps things the URDF export must drop:

* Closed loop joints are written as ordinary SDF joints.
* Ball, universal, and screw joint types survive.
* Capsule, cone, and ellipsoid shapes survive.
* The collision surface survives.

What still has no SDF home is the HCDF cyber layer: motors, networks, power, transmissions, sensors,
and accel or jerk limits. Each is recorded. Two joint types have no SDF target either, `planar` and
`free`, so they downgrade to `fixed` and are recorded.

### libsdformat and the URDF to SDF path

The optional `gz` command from libsdformat does two jobs: it validates SDF, and it performs the one
canonical URDF to SDF conversion. We never invent that conversion ourselves.

```python
from sdf_io import gz
if gz.available():
    result = gz.urdf_to_sdf("robot.urdf")   # result.stdout is the SDF
    if not result.ok:
        print(result.diagnostics)           # surface what libsdformat complained about
```

One sharp edge is built into the wrapper. `gz sdf --print` writes errors and warnings to standard
error yet still exits zero. On a URDF whose links lack inertia it can return an empty model and still
exit zero. So `result.ok` is true only when the exit code is zero and there is no error in the
diagnostics. Exit zero alone is never treated as clean. The SDF spec version is pinned at `1.12`.

## Includes, JSON, and meshes

Includes are flattened by `hcdf_io.flatten`. Each included file is prefixed by its name, every
reference is rewritten, and the subassembly is placed by its pose offset. Cycles raise an error.

JSON is handled by `hcdf_io.to_json` and `hcdf_io.from_json`. It is value exact rather than text
exact, because JSON numbers cannot keep trailing zeros.

Meshes are baked by `assets.stamp`. A visual mesh is converted to a GLB and addressed by the sha256
of its bytes. Collision meshes stay lean and are hashed in place. The GLB export is deterministic, so
the same input always yields the same hash.

## What survives, side by side

K means kept, L means lost and recorded in the manifest.

| HCDF construct                         | to URDF | to SDF |
|----------------------------------------|:-------:|:------:|
| revolute, prismatic, fixed, continuous | K       | K      |
| free joint                             | K (floating) | L |
| planar joint                           | K       | L      |
| ball, universal, screw                 | L       | K      |
| cylindrical joint                      | L       | L      |
| closed loop joint                      | L       | K      |
| box, cylinder, sphere                  | K       | K      |
| capsule, cone, ellipsoid               | L       | K      |
| mesh visual as a GLB                   | K as mesh ref | K as mesh ref |
| flat color                             | K       | K      |
| collision surface                      | L       | K      |
| sensors                                | L       | L      |
| motors, networks, power, transmissions | L       | L      |
| named frames, groups, states           | L       | L      |
| description, author, version           | L (annotation) | L (annotation) |

The single sentence version: URDF is a narrow tree format that HCDF can fully absorb and round trip
without loss, while SDF is a wider peer that carries most of the mechanical and contact content, and
HCDF holds a cyber layer that neither of them has.

## Guarantees

* URDF to HCDF to URDF to HCDF is value exact. A second import equals the first.
* SDF to HCDF to SDF to HCDF is value exact for the overlap.
* Output is honest. Whatever a writer cannot represent is in the returned manifest, and whatever a
  reader cannot map is in the returned notes.
* The SDF we emit passes `gz --check`.

## Quick reference

```python
from urdf_hcdf import from_urdf, to_urdf, check_profile
from sdf_io import from_sdf, to_sdf, gz
from hcdf_io import flatten, to_json, from_json
import assets

doc, notes   = from_urdf("robot.urdf")     # URDF in
urdf, loss   = to_urdf(doc)                # URDF out, with a loss manifest
report       = check_profile(doc)          # is it URDF clean, and why

doc, notes   = from_sdf("robot.sdf")       # SDF in
sdf, loss    = to_sdf(doc)                 # SDF out, with a loss manifest

doc          = flatten("assembly.hcdf")    # resolve includes
data         = to_json(doc)                # HCDF as JSON
manifest     = assets.stamp(doc, base_dir=".", out_dir="glb")   # bake meshes
```

Two command line tools exist:

```
python3 -m hcdfdom.validate robot.hcdf      # validate the tree and references
python3 -m urdf_hcdf.profile robot.hcdf     # classify against the URDF profile
```
