#!/usr/bin/env python3
"""Round-trip + loss-manifest tests for urdf_hcdf.

Fidelity proof: URDF -> HCDF (H1) -> URDF -> HCDF (H2), asserting canon(H1) == canon(H2).
HCDF idempotence after the first import means export loses nothing further — the rigorous
"value-exact modulo documented losses" claim. Also checks the export loss manifest reports
HCDF-only constructs (loops, groups, states, frames) and that frame conversion (FRD->FLU)
is applied on export.

Run:  python3 tests/test_urdf_roundtrip.py
"""
from __future__ import annotations

import os
import sys

from lxml import etree

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import hcdfdom  # noqa: E402
from hcdfdom import frames  # noqa: E402
from hcdfdom.model import (Axis, BodyFrame, Comp, Hcdf, Joint, JointChild, JointLimit, JointParent,  # noqa: E402
                           JointType, Pose, WorldFrame)
from hcdfdom.validate import ERROR, validate  # noqa: E402
from urdf_hcdf import from_urdf, to_urdf  # noqa: E402

_n = _fail = 0


def check(cond, msg):
    global _n, _fail
    _n += 1
    if cond:
        print(f"  \033[32mok\033[0m   {msg}")
    else:
        _fail += 1
        print(f"  \033[31mFAIL\033[0m {msg}")


def canon(el):
    return (etree.QName(el.tag).localname, tuple(sorted(el.attrib.items())),
            (el.text or "").strip(), tuple(sorted(canon(c) for c in el if isinstance(c.tag, str))))


def _ln(e):
    return etree.QName(e.tag).localname


def _kids(e):
    return [c for c in e if isinstance(c.tag, str)]


def main():
    schema = etree.XMLSchema(etree.parse(os.path.join(ROOT, "hcdf.xsd")))

    cases = [("synth-arm", os.path.join(ROOT, "tests", "urdf", "synth-arm.urdf"))]
    pr2 = os.path.expanduser("~/git/urdf/urdf/test/pr2_desc.urdf")
    if os.path.exists(pr2):
        cases.append(("PR2", pr2))

    print("Round-trip idempotence (URDF -> HCDF -> URDF -> HCDF):")
    for name, path in cases:
        h1, _ = from_urdf(path)
        urdf2, loss = to_urdf(h1)
        h2, _ = from_urdf(urdf2)
        check(canon(h1.to_xml("hcdf")) == canon(h2.to_xml("hcdf")),
              f"{name}: HCDF idempotent (export loses nothing further)")
        check(schema.validate(h2.to_xml("hcdf")), f"{name}: re-exported URDF re-imports schema-VALID")
        check(not [i for i in validate(h2) if i.level == ERROR], f"{name}: re-import validator-clean")

    print("\nStructural fidelity (synth-arm URDF vs re-exported URDF):")
    h1, _ = from_urdf(cases[0][1])
    urdf2, _ = to_urdf(h1)
    r0 = etree.parse(cases[0][1]).getroot()
    r2 = etree.fromstring(urdf2.encode())

    def names(root, kind):
        return sorted(e.get("name") for e in _kids(root) if _ln(e) == kind)

    def jtypes(root):
        return {e.get("name"): e.get("type") for e in _kids(root) if _ln(e) == "joint"}

    check(names(r0, "link") == names(r2, "link"), "all links preserved (names)")
    check(jtypes(r0) == jtypes(r2), "all joints preserved (names + types)")
    top2 = {_ln(e) for e in _kids(r2)}
    check("gazebo" in top2 and "ros2_control" in top2,
          "quarantined <gazebo> + <ros2_control> de-merged back to top level")

    print("\nLoss manifest reports HCDF-only constructs:")
    _, loss = to_urdf(hcdfdom.load(os.path.join(ROOT, "tests", "valid", "fourbar-loop.hcdf")))
    check(any(c == "loop" for c, _ in loss.items),
          "four-bar: loop-closure reported as a loss (URDF is tree-only)")
    _, loss = to_urdf(hcdfdom.load(os.path.join(ROOT, "tests", "valid", "articulated-arm.hcdf")))
    cats = {c for c, _ in loss.items}
    check("top-level" in cats, "articulated-arm: groups/states reported as a loss")
    check("comp" in cats, "articulated-arm: frames / dynamic-surfaces reported as a loss")

    print("\nFrame conversion on export (FRD body-frame -> URDF FLU):")
    doc = Hcdf()
    doc.name, doc.body_frame, doc.world_frame = "frd-bot", BodyFrame.FRD, WorldFrame.NED
    doc.comp = [Comp(), Comp()]
    doc.comp[0].name, doc.comp[1].name = "a", "b"
    j = Joint()
    j.name, j.type = "j", JointType.revolute
    j.parent, j.child = JointParent(), JointChild()
    j.parent.comp, j.child.comp = "a", "b"
    j.origin = Pose()
    j.origin.xyz, j.origin.rpy = "0 1 2", "0 0 0"
    j.axis = Axis()
    j.axis.xyz = "0 1 0"
    j.limit = JointLimit()
    j.limit.lower, j.limit.upper = "-1", "1"
    doc.joint = [j]
    urdf, _ = to_urdf(doc)
    r = etree.fromstring(urdf.encode())
    jo = next(e for e in _kids(r) if _ln(e) == "joint")
    org = next(e for e in _kids(jo) if _ln(e) == "origin")
    xyz = [float(v) for v in org.get("xyz").split()]
    check(xyz == [0, -1, -2], f"FRD origin xyz '0 1 2' -> FLU '{org.get('xyz')}' (diag(1,-1,-1))")
    check(org.get("rpy") is not None and "quat" not in org.attrib, "origin emitted as xyz+rpy (no quat in URDF)")
    ax = next(e for e in _kids(jo) if _ln(e) == "axis")
    axyz = [float(v) for v in ax.get("xyz").split()]
    check(axyz == [0, -1, 0], f"FRD axis '0 1 0' -> FLU '{ax.get('xyz')}' (axis vector converted too, not just origin)")

    print(f"\nurdf round-trip tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
