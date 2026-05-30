#!/usr/bin/env python3
"""Export-only loss-manifest completeness test for urdf_hcdf (closes the B12 coverage gap).

The URDF->HCDF->URDF round-trip test cannot catch silently-dropped HCDF-only constructs,
because the URDF importer never produces them — so they never enter the round-trip loop.
This test builds a native HCDF exercising every construct that has no URDF home and asserts
each one is reported in the export LossManifest (never silently dropped). New URDF-homeless
constructs should be added here so the converter fails closed.

Run:  python3 tests/test_urdf_export_losses.py
"""
from __future__ import annotations

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from hcdfdom import model as M  # noqa: E402
from urdf_hcdf import to_urdf  # noqa: E402

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
    doc = M.Hcdf()
    doc.name = "kitchen-sink"
    doc.body_frame, doc.world_frame = M.BodyFrame.FLU, M.WorldFrame.ENU

    # comp carrying every HCDF-rich construct with no URDF home
    c = M.Comp()
    c.name = "rich"
    c.description = "a richly-specified component"
    for attr in ("board", "operating_temp", "switch", "software", "discovered", "sensor",
                 "motor", "hmi", "dynamic_surface", "power_source", "port", "antenna", "frame",
                 "extension"):
        cur = getattr(c, attr)
        # list fields default to []; single fields default to None
        setattr(c, attr, [object()] if isinstance(cur, list) else object())
    uc = M.UrdfCompatComp()
    uc.link_type = "pr2-style-link"
    c.urdf_compat = uc

    # comp with an HCDF-only capsule visual (no URDF primitive)
    cap = M.Comp()
    cap.name = "capsule-link"
    vis = M.Visual()
    vis.name = "capvis"
    vg = M.VisualGeometry()
    vg.capsule = M.Capsule()
    vis.geometry = vg
    cap.visual.append(vis)

    doc.comp = [c, cap]

    # an HCDF-only joint type with axis + limit + thread_pitch + axis2 + extension + description
    js = M.Joint()
    js.name, js.type, js.thread_pitch = "screw_j", M.JointType("screw"), "0.002"
    js.parent, js.child = M.JointParent(), M.JointChild()
    js.parent.comp, js.child.comp = "rich", "capsule-link"
    js.axis = M.Axis(); js.axis.xyz = "0 0 1"
    js.axis2 = M.Axis(); js.axis2.xyz = "0 1 0"
    js.limit = M.JointLimit(); js.limit.lower, js.limit.upper = "0", "0.1"
    js.extension = object()
    js.description = "a screw joint"

    # a loop-closure joint (URDF is tree-only)
    jl = M.Joint()
    jl.name, jl.type = "loop_j", M.JointType("revolute")
    jl.parent, jl.child = M.JointParent(), M.JointChild()
    jl.parent.comp, jl.child.comp = "rich", "capsule-link"
    jl.loop = M.LoopClosure()
    doc.joint = [js, jl]

    # root-level HCDF-only constructs
    doc.group = [object()]
    doc.state = [object()]
    doc.network = [object()]
    doc.transmission = [object()]
    doc.include = [object()]
    doc.self_collision_disable = object()

    urdf, loss = to_urdf(doc)
    text = loss.text().lower()

    print("Every URDF-homeless construct must appear in the loss manifest:")
    expect = {
        "comp board": "board",
        "comp operating-temp": "operating-temp",
        "comp switch": "switch",
        "comp software": "software",
        "comp discovered": "discovered",
        "comp sensor": "sensor",
        "comp motor": "motor",
        "comp hmi": "hmi",
        "comp dynamic-surface": "dynamic surface",
        "comp power-source": "power source",
        "comp port": "port",
        "comp antenna": "antenna",
        "comp frame": "frame",
        "comp extension": "extension",
        "comp description": "description",
        "capsule geometry": "capsule",
        "joint-type downgrade": "no urdf equivalent",
        "screw thread_pitch": "thread_pitch",
        "downgrade axis": "axis",
        "downgrade limit": "limit",
        "axis2": "axis2",
        "loop closure": "loop",
        "root groups": "joint groups",
        "root states": "kinematic states",
        "root networks": "networks",
        "root transmissions": "transmissions",
        "root includes": "includes",
        "root self-collision-disable": "self-collision-disable",
    }
    for label, needle in expect.items():
        check(needle in text, f"{label} reported (matches {needle!r})")

    print("\nURDF-compat link/@type is EMITTED (lossless), not a loss:")
    check('type="pr2-style-link"' in urdf, "comp urdf-compat link-type -> URDF link @type")

    print(f"\nexport-loss completeness: {_n - _fail} passed, {_fail} failed  ({len(loss)} loss items total)")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
