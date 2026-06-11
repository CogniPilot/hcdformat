"""urdf_hcdf — the URDF <-> HCDF conversion adapter on top of hcdfdom.

A front/back-end on the neutral hcdfdom IR: URDF is read with lxml (no urdfdom/yourdfpy
dependency) and mapped into the typed DOM; the reverse (HCDF -> URDF) is emitted at the lxml
string layer, with a structured loss manifest and an HCDF-URDF Profile classifier.

    from urdf_hcdf import from_urdf, to_urdf, check_profile
    doc, notes = from_urdf("robot.urdf")   # doc: hcdfdom.Hcdf, notes: loss/quarantine log
    urdf_xml, loss = to_urdf(doc)          # loss: LossManifest (.text()/.markdown()/.to_json())
    report = check_profile(doc)            # HCDF-URDF Profile 1.0 classification
"""
from __future__ import annotations

from .from_urdf import from_urdf
from .profile import (BENIGN_LOSS_CATEGORIES, IDENTITY, OUT_OF_PROFILE,
                      WITH_TRANSFORM, Finding, ProfileReport, check_profile)
from .to_urdf import LossManifest, to_urdf

__all__ = ["from_urdf", "to_urdf", "LossManifest", "check_profile", "ProfileReport",
           "Finding", "IDENTITY", "WITH_TRANSFORM", "OUT_OF_PROFILE", "BENIGN_LOSS_CATEGORIES"]
