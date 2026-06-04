#!/usr/bin/env python3
"""Tests for hcdf_io: <include> flattening + typed-DOM JSON round-trip.

Run:  python3 tests/test_hcdf_io.py
"""
from __future__ import annotations

import os
import sys

from lxml import etree

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import hcdf_io  # noqa: E402
import hcdfdom  # noqa: E402
from hcdfdom.validate import ERROR, validate  # noqa: E402

_n = _fail = 0


def check(cond, msg):
    global _n, _fail
    _n += 1
    if cond:
        print(f"  \033[32mok\033[0m   {msg}")
    else:
        _fail += 1
        print(f"  \033[31mFAIL\033[0m {msg}")


def _norm_text(s):
    """Normalize a possibly-numeric (or space-list) string so JSON's number formatting
    (e.g. '0.30' -> 0.3) doesn't read as a difference. Value-exact, not string-exact."""
    out = []
    for tok in (s or "").split():
        try:
            out.append(repr(float(tok)))
        except ValueError:
            out.append(tok)
    return " ".join(out)


def vcanon(el):
    """Value-normalized canonical form (numbers compared by value, children order-insensitive)."""
    attrs = tuple(sorted((k, _norm_text(v)) for k, v in el.attrib.items()))
    return (etree.QName(el.tag).localname, attrs, _norm_text(el.text),
            tuple(sorted(vcanon(c) for c in el if isinstance(c.tag, str))))


def main():
    schema = etree.XMLSchema(etree.parse(os.path.join(ROOT, "hcdf.xsd")))
    HD = os.path.join(ROOT, "tests", "hcdf")

    print("<include> flattening (composed-robot = sensor-module x2):")
    doc, notes = hcdf_io.flatten(os.path.join(HD, "composed-robot.hcdf"))
    check(schema.validate(doc.to_xml("hcdf")), "flattened document is schema-VALID")
    check(not [i for i in validate(doc) if i.level == ERROR], "flattened document validator-clean (no errors)")
    check(len(doc.include) == 0, "all <include> elements resolved")
    names = [c.name for c in doc.comp]
    check(names == ["torso", "left/base", "left/head", "right/base", "right/head"],
          "comp names prefixed per instance (no collision across two includes)")
    check([c.name for c in doc.color] == ["left/shell", "right/shell"], "color defs prefixed")
    lpan = next(j for j in doc.joint if j.name == "left/pan")
    check(lpan.parent.comp == "left/base" and lpan.child.comp == "left/head",
          "joint parent/child refs rewritten to prefixed comps")
    lb = next(c for c in doc.comp if c.name == "left/base")
    check(lb.visual[0].color.name == "left/shell", "visual color reference rewritten to prefixed color")
    check(lb.visual[0].pose.xyz == "1 0 0", f"left instance placed by @pose (visual xyz='{lb.visual[0].pose.xyz}')")
    check(lpan.origin.xyz == "1 0 0.02", f"root-leaving joint origin offset by @pose ('{lpan.origin.xyz}')")
    rb = next(c for c in doc.comp if c.name == "right/base")
    check(rb.visual[0].pose.xyz == "-1 0 0", f"right instance placed by its own @pose ('{rb.visual[0].pose.xyz}')")
    check(any("flattened include" in n for n in notes), "merge/loss notes recorded")

    print("\nFlattening is idempotent + detects cycles:")
    doc2, n2 = hcdf_io.flatten(doc, base_dir=HD)
    check(len(doc2.include) == 0 and not n2, "re-flattening a flattened document is a no-op")
    try:
        hcdf_io.flatten(os.path.join(HD, "cycle-a.hcdf"))
        raised = False
    except ValueError:
        raised = True
    check(raised, "include cycle (a->b->a) raises ValueError")

    print("\nTyped-DOM JSON round-trip (value-exact; JSON numbers don't preserve formatting):")
    for rel in ("tests/valid/articulated-arm.hcdf", "tests/hcdf/sensor-module.hcdf",
                "tests/valid/minimal-comp.hcdf", "tests/valid/fourbar-loop.hcdf"):
        d = hcdfdom.load(os.path.join(ROOT, rel))
        d2 = hcdf_io.from_json(hcdf_io.to_json(d))
        check(vcanon(d.to_xml("hcdf")) == vcanon(d2.to_xml("hcdf")), f"{rel}: DOM->JSON->DOM value-exact")

    # docs with lax <extension> foreign content: typed core still round-trips; extension is XML-only
    d = hcdfdom.load(os.path.join(ROOT, "examples", "drone-quadrotor.hcdf"))
    try:
        hcdf_io.from_json(hcdf_io.to_json(d))
        ran = True
    except Exception:  # noqa: BLE001
        ran = False
    check(ran, "doc with <extension> survives JSON round-trip (foreign content is XML-only; see note)")

    print(f"\nhcdf_io tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
