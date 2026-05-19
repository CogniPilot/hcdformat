#!/usr/bin/env python3
"""Tests for the hcdfdom companion validator (hcdfdom/validate.py).

Each invalid fixture is XSD-VALID by design — it passes the schema but trips one
semantic rule the schema cannot express — so this proves the validator is what
catches it. The articulated arm must be validator-clean (no errors).

Run:  python3 tests/test_validator.py
"""
from __future__ import annotations

import glob
import os
import sys

from lxml import etree

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import hcdfdom  # noqa: E402
from hcdfdom.validate import validate, ERROR  # noqa: E402

# invalid fixture -> the error code it must produce
EXPECT = {
    "cycle.hcdf": "E_CYCLE",
    "multi-parent.hcdf": "E_MULTI_PARENT",
    "dangling-relative-to.hcdf": "E_FRAME_RELATIVE_TO",
    "continuous-bounds.hcdf": "E_JOINT_CONTINUOUS_BOUNDS",
    "revolute-no-limit.hcdf": "E_JOINT_LIMIT_REQUIRED",
    "fixed-with-axis.hcdf": "E_JOINT_AXIS_FORBIDDEN",
    "dangling-color.hcdf": "E_COLOR_REF",
    "two-default-states.hcdf": "E_MULTI_DEFAULT_STATE",
    "group-bad-tip-frame.hcdf": "E_GROUP_TIP_FRAME",
}

# documents that must be validator-clean (zero errors)
CLEAN = ["tests/valid/articulated-arm.hcdf"]

OK, FAIL = "\033[32mok\033[0m", "\033[31mFAIL\033[0m"


def main():
    schema = etree.XMLSchema(etree.parse(os.path.join(ROOT, "hcdf.xsd")))
    passed = failed = 0

    print("Invalid fixtures (XSD-valid, must trip one validator rule):")
    inv_dir = os.path.join(ROOT, "tests", "validator", "invalid")
    seen = set()
    for path in sorted(glob.glob(os.path.join(inv_dir, "*.hcdf"))):
        fn = os.path.basename(path)
        seen.add(fn)
        want = EXPECT.get(fn)
        if want is None:
            print(f"  {FAIL} {fn}  (no expected code in EXPECT manifest)")
            failed += 1
            continue
        if not schema.validate(etree.parse(path)):
            print(f"  {FAIL} {fn}  (fixture is not XSD-valid; schema would catch it, not the validator)")
            failed += 1
            continue
        codes = {i.code for i in validate(hcdfdom.load(path)) if i.level == ERROR}
        if want in codes:
            print(f"  {OK} {fn}  -> {want}")
            passed += 1
        else:
            print(f"  {FAIL} {fn}  (expected {want}, got errors: {sorted(codes) or 'none'})")
            failed += 1

    missing = set(EXPECT) - seen
    for fn in sorted(missing):
        print(f"  {FAIL} {fn}  (manifest entry has no fixture file)")
        failed += 1

    print("\nClean documents (must have zero validator errors):")
    for rel in CLEAN:
        issues = validate(hcdfdom.load(os.path.join(ROOT, rel)))
        errs = [i for i in issues if i.level == ERROR]
        if errs:
            print(f"  {FAIL} {rel}")
            for e in errs:
                print(f"        {e}")
            failed += 1
        else:
            warns = sum(1 for i in issues if i.level != ERROR)
            print(f"  {OK} {rel}  (0 errors, {warns} warning(s))")
            passed += 1

    print(f"\nvalidator tests: {passed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
