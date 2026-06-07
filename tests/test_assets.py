#!/usr/bin/env python3
"""Tests for the mesh asset pipeline (assets.py).

Generates meshes with trimesh into a temp dir, then exercises GLB conversion determinism,
URI resolution, and DOM stamping. Skips cleanly if trimesh/pygltflib are unavailable.

Run:  python3 tests/test_assets.py
"""
from __future__ import annotations

import os
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

try:
    import trimesh  # noqa: F401
    from pygltflib import GLTF2
except Exception:  # noqa: BLE001
    print("(trimesh/pygltflib not installed — skipping asset pipeline tests)")
    sys.exit(0)

import warnings

import assets  # noqa: E402
from hcdfdom import model as M  # noqa: E402
from hcdfdom.validate import validate  # noqa: E402

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
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        box = trimesh.creation.box(extents=(0.2, 0.3, 0.4))
    with tempfile.TemporaryDirectory() as td:
        stl = os.path.join(td, "box.stl")
        obj = os.path.join(td, "box.obj")
        glb_in = os.path.join(td, "premade.glb")
        box.export(stl)
        box.export(obj)
        with open(glb_in, "wb") as fh:
            fh.write(assets.to_glb(stl))

        print("Conversion + content addressing:")
        g1 = assets.to_glb(stl)
        g2 = assets.to_glb(stl)
        check(g1 == g2, "STL->GLB is byte-deterministic (stable content address)")
        check(assets.content_sha(g1) == assets.content_sha(g2), "content_sha stable for identical bytes")
        check(assets.content_sha(b"a") != assets.content_sha(b"b"), "content_sha distinguishes different bytes")
        gltf = GLTF2.load_from_bytes(g1)
        check(len(gltf.meshes) >= 1 and len(gltf.accessors) >= 1, "produced GLB is valid (pygltflib loads it)")

        print("\nURI resolution:")
        check(assets.resolve_uri("box.stl", base_dir=td) == os.path.normpath(stl), "relative path resolves")
        check(assets.resolve_uri("file://" + stl) == os.path.normpath(stl), "file:// resolves")
        check(assets.resolve_uri("package://pkg/box.stl", package_paths={"pkg": td}) == os.path.normpath(stl),
              "package:// resolves via package_paths")
        check(assets.resolve_uri("package://nope/x.stl", package_paths={}) is None, "unknown package -> None")
        check(assets.resolve_uri("missing.stl", base_dir=td) is None, "missing file -> None")

        print("\nDOM stamping (visual model -> GLB; collision mesh -> hashed lean):")
        doc = M.Hcdf()
        doc.name = "asset-bot"
        c = M.Comp()
        c.name = "body"
        v = M.Visual()
        v.name = "body_vis"
        v.model = M.ModelRef()
        v.model.uri = "box.stl"          # a URDF-imported visual mesh (not yet a GLB)
        c.visual.append(v)
        vg = M.Visual()
        vg.name = "body_glb"
        vg.model = M.ModelRef()
        vg.model.uri = "premade.glb"     # already a GLB
        c.visual.append(vg)
        col = M.Collision()
        col.name = "body_col"
        col.geometry = M.CollisionGeometry()
        col.geometry.mesh = M.Mesh()
        col.geometry.mesh.uri = "box.obj"   # lean collision mesh
        c.collision.append(col)
        doc.comp = [c]

        out = os.path.join(td, "glb")
        manifest = assets.stamp(doc, base_dir=td, out_dir=out, package_paths=None)

        check(v.model.uri.endswith(".glb") and v.model.sha and v.model.sha.startswith("sha256:"),
              f"non-GLB visual model converted -> '{v.model.uri}' + sha set")
        check(os.path.exists(os.path.join(out, v.model.uri)), "content-addressed GLB written to out_dir")
        check(vg.model.uri == "premade.glb" and vg.model.sha and vg.model.sha.startswith("sha256:"),
              "already-GLB visual model hashed in place (uri unchanged)")
        check(col.geometry.mesh.uri == "box.obj" and col.geometry.mesh.sha,
              "collision mesh kept lean (uri unchanged) + sha set")
        check(col.geometry.mesh.source_uri == "box.obj", "collision mesh @source-uri recorded")

        statuses = sorted(e["status"] for e in manifest)
        check(statuses == ["converted-to-glb", "hashed-glb", "hashed-lean"],
              f"manifest reports each action ({statuses})")

        # the stamped visual is now a real GLB ref -> no W_VISUAL_URI warning anymore
        warns = [i for i in validate(doc) if i.code == "W_VISUAL_URI"]
        check(not warns, "after stamping, no W_VISUAL_URI warning (visuals are GLB)")

        print("\nUnresolved + no-out_dir paths are reported, not crashed:")
        doc2 = M.Hcdf(); doc2.name = "x"
        c2 = M.Comp(); c2.name = "b"
        v2 = M.Visual(); v2.name = "v"; v2.model = M.ModelRef(); v2.model.uri = "ghost.dae"
        c2.visual.append(v2); doc2.comp = [c2]
        m2 = assets.stamp(doc2, base_dir=td)
        check(m2[0]["status"] == "unresolved", "unresolved mesh -> 'unresolved' status (no crash)")
        v2.model.uri = "box.stl"
        m3 = assets.stamp(doc2, base_dir=td, out_dir=None)
        check("needs-conversion" in m3[0]["status"], "non-GLB visual without out_dir -> 'needs-conversion'")

    print(f"\nassets tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
