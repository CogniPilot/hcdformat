"""sdf_io — the SDFormat <-> HCDF conversion adapter on top of hcdfdom.

A first-class *peer* spoke of ``urdf_hcdf`` on the neutral hcdfdom IR — NOT an intermediary on the
URDF path (there is no SDF→URDF, by design). SDF is read/written with lxml and the *overlap* is
adapter-mapped (SDF is only adapter-mapped, never absorbed into the HCDF core like URDF is). The
optional ``gz``/libsdformat CLI provides validation/canonicalization and the canonical, delegated
URDF→SDF conversion (the clean gazebo path).

    from sdf_io import from_sdf, to_sdf
    doc, notes = from_sdf("robot.sdf")     # doc: hcdfdom.Hcdf, notes: loss/deferral log
    sdf_xml, loss = to_sdf(doc)            # loss: urdf_hcdf.LossManifest (shared)

    from sdf_io import gz
    if gz.available():
        res = gz.urdf_to_sdf("robot.urdf")  # delegated URDF->SDF; consult res.ok / res.diagnostics
"""
from __future__ import annotations

from . import gz
from .from_sdf import from_sdf
from .gz import SDF_VERSION
from .to_sdf import to_sdf

__all__ = ["from_sdf", "to_sdf", "gz", "SDF_VERSION"]
