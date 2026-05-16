#!/usr/bin/env python3
"""Round-trip test for the hcdfdom typed DOM.

For every example and valid fixture: parse -> typed model -> serialize, then assert
the result is (1) still schema-VALID and (2) canonically equal to the source. Together
that is a strong value-exact check: (2) catches any content lost or changed by the DOM,
(1) catches any structural/ordering error introduced on serialize.

Run:  python3 tests/test_hcdfdom.py
"""
from __future__ import annotations

import glob
import os
import sys

from lxml import etree

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import hcdfdom  # noqa: E402


def canon(el):
    """Order-insensitive structural form: (tag, sorted-attrs, stripped-text, sorted-kids).

    Order-insensitive because xs:all children may legitimately reorder; the companion
    schema-VALID check (below) is what guards xs:sequence ordering.
    """
    attrs = tuple(sorted(el.attrib.items()))
    text = (el.text or "").strip()
    kids = sorted(canon(c) for c in el if isinstance(c.tag, str))
    return (etree.QName(el.tag).localname, attrs, text, tuple(kids))


def diff_path(a, b, path="hcdf"):
    """Return a human-readable first divergence between two canon() trees, or None."""
    if a[0] != b[0]:
        return f"{path}: tag {a[0]!r} != {b[0]!r}"
    if a[1] != b[1]:
        return f"{path}: attrs {dict(a[1])} != {dict(b[1])}"
    if a[2] != b[2]:
        return f"{path}: text {a[2]!r} != {b[2]!r}"
    if len(a[3]) != len(b[3]):
        ta, tb = [k[0] for k in a[3]], [k[0] for k in b[3]]
        only_a = sorted(set(ta) - set(tb)) or "-"
        only_b = sorted(set(tb) - set(ta)) or "-"
        return f"{path}: child count {len(a[3])} != {len(b[3])} (only-src={only_a}, only-out={only_b})"
    for ca, cb in zip(a[3], b[3]):
        d = diff_path(ca, cb, f"{path}/{ca[0]}")
        if d:
            return d
    return None


def main():
    schema = etree.XMLSchema(etree.parse(os.path.join(ROOT, "hcdf.xsd")))
    files = sorted(glob.glob(os.path.join(ROOT, "examples", "*.hcdf")) +
                   glob.glob(os.path.join(ROOT, "tests", "valid", "*.hcdf")))
    passed, failed = 0, 0
    for f in files:
        rel = os.path.relpath(f, ROOT)
        src = etree.parse(f).getroot()
        try:
            doc = hcdfdom.Hcdf.from_xml(src)
            out = hcdfdom.to_element(doc)
        except Exception as e:  # noqa: BLE001
            print(f"  \033[31mFAIL\033[0m {rel}  (DOM error: {e})")
            failed += 1
            continue
        errs = []
        if not schema.validate(out):
            errs.append("serialized output is NOT schema-valid: " +
                        "; ".join(f"L{e.line}: {e.message}" for e in schema.error_log))
        d = diff_path(canon(src), canon(out))
        if d:
            errs.append("content drift: " + d)
        if errs:
            print(f"  \033[31mFAIL\033[0m {rel}")
            for e in errs:
                print(f"        {e}")
            failed += 1
        else:
            print(f"  \033[32mok\033[0m   {rel}")
            passed += 1
    print(f"\nhcdfdom round-trip: {passed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
