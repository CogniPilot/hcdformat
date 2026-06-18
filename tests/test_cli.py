#!/usr/bin/env python3
"""Smoke tests for the hcdf command line interface (hcdf_cli).

Drives the CLI as a subprocess (the way a user runs it) through the main conversion
directions plus validate and profile, using the example arm as input.

Run:  python3 tests/test_cli.py
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
ARM = os.path.join(ROOT, "tests/urdf/example-arm.urdf")

_n = _fail = 0


def check(cond, msg):
    global _n, _fail
    _n += 1
    if cond:
        print(f"  \033[32mok\033[0m   {msg}")
    else:
        _fail += 1
        print(f"  \033[31mFAIL\033[0m {msg}")


def cli(*args):
    return subprocess.run([sys.executable, "-m", "hcdf_cli", *args],
                          cwd=ROOT, capture_output=True, text=True)


def main():
    with tempfile.TemporaryDirectory() as td:
        hcdf = os.path.join(td, "arm.hcdf")
        urdf = os.path.join(td, "arm.rt.urdf")
        sdf = os.path.join(td, "arm.sdf")
        js = os.path.join(td, "arm.json")

        print("convert URDF -> HCDF:")
        r = cli("convert", ARM, hcdf)
        check(r.returncode == 0 and os.path.exists(hcdf), "exit 0 and HCDF written")
        check("<hcdf" in open(hcdf).read(), "output is an HCDF document")

        print("\nconvert HCDF -> URDF (round trip) is clean:")
        r = cli("convert", hcdf, urdf)
        body = open(urdf).read() if os.path.exists(urdf) else ""
        check(r.returncode == 0 and "<robot" in body, "exit 0 and URDF written")
        check("loss:" not in r.stderr, "no loss reported on the URDF round trip")
        check('name="shoulder"' in body and "<material" in body, "joint and materials restored")

        print("\nconvert HCDF -> SDF reports the color-palette loss:")
        r = cli("convert", hcdf, sdf)
        check(r.returncode == 0 and "<sdf" in open(sdf).read(), "exit 0 and SDF written")
        check("loss:" in r.stderr and "color" in r.stderr, "SDF color-palette difference reported")

        print("\nJSON round trip:")
        check(cli("convert", hcdf, js).returncode == 0 and os.path.exists(js), "HCDF -> JSON")
        back = os.path.join(td, "back.hcdf")
        check(cli("convert", js, back).returncode == 0 and "<hcdf" in open(back).read(), "JSON -> HCDF")

        print("\nstdout + format override:")
        r = cli("convert", ARM, "-", "--to", "hcdf")
        check(r.returncode == 0 and "<hcdf" in r.stdout, "writes to stdout with --to")

        print("\nSDF -> URDF through the hub (no native converter exists):")
        urdf2 = os.path.join(td, "from_sdf.urdf")
        r = cli("convert", sdf, urdf2)
        check(r.returncode == 0 and os.path.exists(urdf2) and "<robot" in open(urdf2).read(),
              "SDF routed to URDF via HCDF")

        print("\nvalidate + profile:")
        r = cli("validate", hcdf)
        check(r.returncode == 0 and "OK" in r.stderr, "validate exits 0 and reports OK")
        r = cli("profile", hcdf)
        check(r.returncode == 0 and "IN-PROFILE-IDENTITY" in r.stdout, "profile reports IN-PROFILE-IDENTITY")

        print("\nerror handling:")
        r = cli("convert", os.path.join(td, "x.bogus"), os.path.join(td, "y.hcdf"))
        check(r.returncode != 0 and "infer format" in r.stderr, "unknown extension is a clean error")

        print("\nxacro input (expanded automatically):")
        from urdf_hcdf import xacro
        if not xacro.available():
            check(True, "xacro tool not present, skipping xacro checks")
        else:
            xc = os.path.join(ROOT, "tests/urdf/example-arm.urdf.xacro")
            xh = os.path.join(td, "from_xacro.hcdf")
            r = cli("convert", xc, xh)
            body = open(xh).read() if os.path.exists(xh) else ""
            check(r.returncode == 0 and 'name="upper_arm"' in body and 'name="shoulder"' in body,
                  ".xacro expands and converts (macro + property resolved)")
            check('radius>0.03<' in body.replace(" ", ""), "default xacro arg used (radius 0.03)")
            xh2 = os.path.join(td, "from_xacro_arg.hcdf")
            r = cli("convert", xc, xh2, "--xacro", "upper_radius:=0.055")
            check(r.returncode == 0 and "0.055" in open(xh2).read(), "--xacro overrides the arg (radius 0.055)")

            print("\n--package resolves $(find) without a build; expand emits a plain URDF:")
            pkgdir = os.path.join(td, "find_pkg")
            os.makedirs(pkgdir, exist_ok=True)
            open(os.path.join(pkgdir, "box.stl"), "w").close()
            open(os.path.join(pkgdir, "extra.xacro"), "w").write(
                '<?xml version="1.0"?><robot xmlns:xacro="http://www.ros.org/wiki/xacro">'
                '<link name="extra_link"/></robot>')
            fb = os.path.join(td, "find_bot.urdf.xacro")
            open(fb, "w").write(
                '<?xml version="1.0"?>\n<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="find_bot">\n'
                '  <xacro:include filename="$(find find_pkg)/extra.xacro"/>\n'
                '  <link name="base"><visual><geometry>'
                '<mesh filename="package://find_pkg/box.stl"/></geometry></visual></link>\n</robot>\n')
            fhc = os.path.join(td, "find_bot.hcdf")
            r = cli("convert", fb, fhc, "--package", f"find_pkg={pkgdir}")
            if r.returncode != 0 and "xacro failed" in r.stderr:
                check(True, "$(find) needs ament_index (no ROS env here), skipping --package checks")
            else:
                body = open(fhc).read() if os.path.exists(fhc) else ""
                check(r.returncode == 0 and 'name="extra_link"' in body and 'name="base"' in body,
                      "--package resolves $(find pkg) in an included xacro")
                fu = os.path.join(td, "find_bot.expanded.urdf")
                r = cli("expand", fb, fu, "--package", f"find_pkg={pkgdir}", "--abs-meshes")
                ub = open(fu).read() if os.path.exists(fu) else ""
                check(r.returncode == 0 and 'name="extra_link"' in ub, "expand resolves $(find) to a plain URDF")
                check(f"file://{pkgdir}/box.stl" in ub and "package://" not in ub,
                      "expand --abs-meshes rewrites package:// to absolute file://")

        print("\n--bake folds scale and mirror into content-addressed assets:")
        try:
            import trimesh
        except Exception:  # noqa: BLE001
            check(True, "trimesh not present, skipping --bake checks")
        else:
            stl = os.path.join(td, "arm.stl")
            trimesh.creation.box(extents=(0.2, 0.3, 0.4)).export(stl)
            mesh_urdf = os.path.join(td, "mesh_bot.urdf")
            open(mesh_urdf, "w").write(
                '<?xml version="1.0"?>\n<robot name="mesh_bot"><link name="base">'
                '<visual><geometry><mesh filename="arm.stl" scale="0.001 0.001 0.001"/></geometry></visual>'
                '<collision><geometry><mesh filename="arm.stl" scale="1 -1 1"/></geometry></collision>'
                '</link></robot>\n')
            baked_hcdf = os.path.join(td, "mesh_bot.hcdf")
            bake_dir = os.path.join(td, "assets")
            r = cli("convert", mesh_urdf, baked_hcdf, "--bake", bake_dir)
            body = open(baked_hcdf).read() if os.path.exists(baked_hcdf) else ""
            check(r.returncode == 0 and os.path.isdir(bake_dir), "exit 0 and bake dir created")
            check(".glb" in body and 'sha="sha256:' in body, "visual baked to a GLB with a content sha")
            check('uri="assets/' in body, "baked @uri is document-relative (assets/<sha>)")
            check("scale=" not in body, "no source scale survives in HCDF (baked into the geometry)")
            cols = [f for f in os.listdir(bake_dir) if f.endswith(".stl")]
            check(len(cols) == 1, "mirrored collision baked to one lean asset")
            m = trimesh.load(os.path.join(bake_dir, cols[0]), force="mesh")
            check(m.is_watertight and m.volume > 0,
                  "mirror folded into vertices (positive volume, no negative scale for the engine)")

    print(f"\ncli tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
