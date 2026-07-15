# `versions/`: write-once frozen artifact archive

This directory is an **immutable** archive of released HCDF schema versions. Each
`versions/<MAJOR.MINOR>/` holds a **byte-exact** copy of the canonical schemas and
versioned registry source artifacts released under that version.

**Invariant: never edit a file here after release.** The live, mutable artifacts are
*only* the repo-root schemas and `registries/` sources. When a new version is cut, add a
new `versions/<v>/` snapshot and a `HASHES` entry; do not modify an existing one.

## Why this exists

`versions/1.0/hcdf.xsd` had previously **drifted**: an older copy stamped `"1.0"` was
missing `tactile_sensor`, `supercapacitor_source`, `solar_source`, `gripper_surface`,
`transmission_spring` and the `BodyFrame`/`WorldFrame`/`TactileSensorType` types, yet still
claimed to be version 1.0. The 1.0 archive here has been **re-synced** to the clean in-place
1.0 reset and frozen.

## Bind by hash, not by string

Tooling (the converter, validators, CI) must bind to a versioned artifact **by its SHA-256
content hash** (see [`HASHES`](HASHES)), never by filename or by the `version="1.0"`
string. Two files claiming the same version with different hashes is then a **detectable
contradiction**, not a silent drift. CI enforces that every canonical artifact hash matches
its `HASHES` entry and that frozen archives are byte-unchanged.
