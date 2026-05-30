"""urdf_hcdf — the URDF <-> HCDF conversion adapter on top of hcdfdom.

A front/back-end on the neutral hcdfdom IR: URDF is read with lxml (no urdfdom/yourdfpy
dependency) and mapped into the typed DOM. The reverse (HCDF -> URDF, emitted at the lxml
string layer) lands in a later increment.

This module currently provides the URDF -> HCDF importer:

    from urdf_hcdf import from_urdf
    doc, notes = from_urdf("robot.urdf")   # doc: hcdfdom.Hcdf, notes: loss/quarantine log
"""
from __future__ import annotations

from .from_urdf import from_urdf
from .to_urdf import LossManifest, to_urdf

__all__ = ["from_urdf", "to_urdf", "LossManifest"]
