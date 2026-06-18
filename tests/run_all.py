#!/usr/bin/env python3
"""Run the entire HCDF + converter test gate in one command (mirrors .github/workflows/validate.yml).

A green run here == a green CI run. Use it to verify the tree after resuming work.

    python3 tests/run_all.py
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PY = sys.executable
GREEN, RED, DIM, OFF = "\033[32m", "\033[31m", "\033[2m", "\033[0m"


def run(label, argv):
    p = subprocess.run(argv, cwd=ROOT, capture_output=True, text=True)
    tail = (p.stdout.strip().splitlines() or [""])[-1]
    ok = p.returncode == 0
    print(f"  {GREEN + 'PASS' + OFF if ok else RED + 'FAIL' + OFF}  {label:<34} {DIM}{tail}{OFF}")
    if not ok and p.stderr.strip():
        print(DIM + "        " + p.stderr.strip().splitlines()[-1] + OFF)
    return ok


def drift(label, gen_script, artifact):
    """Regenerate `artifact` from hcdf.xsd into a temp file and diff (no-drift check)."""
    tmp = tempfile.NamedTemporaryFile(delete=False, suffix=os.path.splitext(artifact)[1]).name
    try:
        g = subprocess.run([PY, gen_script, "hcdf.xsd", tmp], cwd=ROOT, capture_output=True, text=True)
        if g.returncode != 0:
            print(f"  {RED}FAIL{OFF}  {label:<34} {DIM}generator error{OFF}")
            return False
        d = subprocess.run(["diff", "-q", artifact, tmp], cwd=ROOT, capture_output=True, text=True)
        ok = d.returncode == 0
        print(f"  {GREEN + 'PASS' + OFF if ok else RED + 'FAIL' + OFF}  {label:<34} "
              f"{DIM}{'no drift' if ok else artifact + ' is stale — regenerate'}{OFF}")
        return ok
    finally:
        os.path.exists(tmp) and os.unlink(tmp)


def main():
    print("HCDF full gate:")
    results = []
    results.append(run("schema test suite", [PY, "tests/run_tests.py"]))
    results.append(drift("spec.html drift", "generate_spec_html.py", "spec.html"))
    results.append(drift("hcdf.schema.json drift", "generate_json_schema.py", "hcdf.schema.json"))
    results.append(drift("hcdfdom model drift", "generate_hcdfdom.py", "hcdfdom/model.py"))
    for label, script in (
        ("hcdfdom round-trip", "tests/test_hcdfdom.py"),
        ("companion validator", "tests/test_validator.py"),
        ("pose/frame normalizer", "tests/test_frames.py"),
        ("urdf_hcdf import", "tests/test_urdf_io.py"),
        ("urdf_hcdf round-trip", "tests/test_urdf_roundtrip.py"),
        ("urdf_hcdf export-loss completeness", "tests/test_urdf_export_losses.py"),
        ("loss manifest + profile checker", "tests/test_profile.py"),
        ("hcdf_io flatten + JSON", "tests/test_hcdf_io.py"),
        ("assets mesh -> GLB", "tests/test_assets.py"),
        ("sdf_io SDF <-> HCDF spoke", "tests/test_sdf_io.py"),
        ("hcdf CLI", "tests/test_cli.py"),
    ):
        results.append(run(label, [PY, script]))
    results.append(run("version-hash archive guard", [PY, "scripts/check_version_hashes.py"]))

    n, passed = len(results), sum(results)
    print(f"\n{'all green' if passed == n else RED + 'FAILURES' + OFF}: {passed}/{n} steps passed")
    return 0 if passed == n else 1


if __name__ == "__main__":
    sys.exit(main())
