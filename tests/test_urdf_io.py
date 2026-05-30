#!/usr/bin/env python3
"""Tests for urdf_hcdf URDF -> HCDF import.

Asserts the synthetic fixture maps field-by-field and yields a document that is both
schema-VALID and validator-clean. Optionally runs a real PR2 URDF (outside the repo) as a
scale + quarantine integration check when present.

Run:  python3 tests/test_urdf_io.py
"""
from __future__ import annotations

import os
import sys

from lxml import etree

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import hcdfdom  # noqa: E402
from hcdfdom.validate import ERROR, validate  # noqa: E402
from urdf_hcdf import from_urdf  # noqa: E402

_n = _fail = 0


def check(cond, msg):
    global _n, _fail
    _n += 1
    if cond:
        print(f"  \033[32mok\033[0m   {msg}")
    else:
        _fail += 1
        print(f"  \033[31mFAIL\033[0m {msg}")


def main():
    schema = etree.XMLSchema(etree.parse(os.path.join(ROOT, "hcdf.xsd")))

    print("Synthetic fixture (tests/urdf/synth-arm.urdf):")
    doc, notes = from_urdf(os.path.join(ROOT, "tests", "urdf", "synth-arm.urdf"))
    el = doc.to_xml("hcdf")

    check(schema.validate(el), "imported document is schema-VALID")
    if not schema.validate(el):
        for e in list(schema.error_log)[:5]:
            print(f"        L{e.line}: {e.message}")
    errs = [i for i in validate(doc) if i.level == ERROR]
    check(not errs, "imported document is validator-clean (0 errors)")
    for e in errs:
        print(f"        {e}")

    check(doc.name == "synth-arm", "robot name -> hcdf @name")
    check(doc.body_frame == hcdfdom.BodyFrame.FLU and doc.world_frame == hcdfdom.WorldFrame.ENU,
          "URDF conventions tagged FLU/ENU (no pose transform on import)")
    check([c.name for c in doc.comp] == ["base", "arm", "tool", "finger_l", "finger_r"],
          "links -> comps (order preserved)")

    jt = {j.name: j for j in doc.joint}
    check(jt["j1"].type.value == "revolute" and jt["j2"].type.value == "continuous"
          and jt["j3"].type.value == "prismatic", "joint types mapped")
    check(jt["j1"].parent.comp == "base" and jt["j1"].child.comp == "arm", "joint parent/child link -> comp")
    check(jt["j1"].limit.lower == "-1.5" and jt["j1"].limit.effort == "10", "revolute limit mapped")
    check(jt["j2"].limit is None, "continuous joint has no limit (no bounds)")
    check(jt["j1"].dynamics.damping == "0.1", "joint dynamics mapped")
    check(jt["j4"].mimic is not None and jt["j4"].mimic.joint == "j3", "mimic joint mapped")
    check(jt["j1"].urdf_compat is not None
          and jt["j1"].urdf_compat.safety_controller.k_velocity == "1",
          "safety_controller -> <urdf-compat>")

    arm = next(c for c in doc.comp if c.name == "arm")
    check(arm.inertial is not None and arm.inertial.mass == "1.2"
          and arm.inertial.inertia == "0.01 0 0 0.01 0 0.005", "inertial mass + inertia tuple mapped")
    av = arm.visual[0]
    check(av.geometry is not None and av.geometry.cylinder is not None and av.color is not None
          and av.color.name == "grey", "primitive visual -> geometry + color ref (ARM B)")
    tool = next(c for c in doc.comp if c.name == "tool")
    tv = tool.visual[0]
    check(tv.model is not None and tv.model.uri.endswith(".dae") and tv.color is None,
          "mesh visual -> GLB <model> (ARM A), material dropped")
    check(tool.collision[0].geometry.mesh is not None
          and tool.collision[0].geometry.mesh.uri.endswith(".stl"), "collision mesh -> lean <mesh>")

    check([c.name for c in doc.color] == ["grey"], "top-level material -> <color>")
    domains = {e.domain for e in doc.extension}
    check("org.gazebosim" in domains and "org.ros2.control" in domains,
          "gazebo + ros2_control quarantined into <extension> by domain")
    check(any("quarantined" in n for n in notes) and any("safety_controller" in n for n in notes),
          "loss/quarantine notes recorded")

    # optional real-robot integration check (outside the repo; skipped in CI)
    pr2 = os.path.expanduser("~/git/urdf/urdf/test/pr2_desc.urdf")
    if os.path.exists(pr2):
        print("\nReal PR2 integration check (~/git/urdf/.../pr2_desc.urdf):")
        d2, _ = from_urdf(pr2)
        e2 = d2.to_xml("hcdf")
        check(schema.validate(e2), f"PR2 ({len(d2.comp)} links, {len(d2.joint)} joints) imports schema-VALID")
        check(not [i for i in validate(d2) if i.level == ERROR], "PR2 import is validator-clean (0 errors)")
    else:
        print("\n(PR2 fixture not present — skipping real-robot integration check)")

    print(f"\nurdf_io tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
