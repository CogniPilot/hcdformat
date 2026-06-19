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

from hcdf import assets  # noqa: E402
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

        print("\nbake() applies scale and mirror, and passes through canonical GLBs:")
        bdir = os.path.join(td, "baked")
        # non-GLB visual + scale -> GLB with the scale baked into the geometry
        n, _ = assets.bake(stl, "0.001 0.001 0.001", "visual", bdir)
        g = trimesh.load(os.path.join(bdir, n), force="mesh")
        check(n.endswith(".glb") and abs(g.extents[0] - 0.0002) < 1e-6, "visual non-GLB + scale -> scaled GLB")
        # already a GLB + no scale -> passed through unchanged (not re-exported)
        np_, _ = assets.bake(glb_in, None, "visual", bdir)
        check(np_.endswith(".glb") and open(glb_in, "rb").read() == open(os.path.join(bdir, np_), "rb").read(),
              "already-GLB + no scale -> passthrough (identical bytes)")
        # already a GLB + a scale -> re-baked
        ns, _ = assets.bake(glb_in, "2 2 2", "visual", bdir)
        gs = trimesh.load(os.path.join(bdir, ns), force="mesh")
        check(abs(gs.extents[0] - 0.4) < 1e-6, "already-GLB + scale -> re-baked to the scale")
        # collision: no scale passes through; a mirror bakes to a positive-volume lean mesh
        nc, _ = assets.bake(stl, None, "collision", bdir)
        check(nc.endswith(".stl") and open(stl, "rb").read() == open(os.path.join(bdir, nc), "rb").read(),
              "collision + no scale -> passthrough lean mesh")
        nm, _ = assets.bake(obj, "1 -1 1", "collision", bdir)
        mm = trimesh.load(os.path.join(bdir, nm), force="mesh")
        check(mm.is_watertight and mm.volume > 0, "collision mirror -> watertight, positive volume (no negative scale)")
        check(assets.bake(stl, "0.001 0.001 0.001", "visual", bdir)[1]
              == assets.bake(stl, "0.001 0.001 0.001", "visual", bdir)[1], "bake is deterministic")

        print("\nmirror is baked into the vertices, not left as a negative-determinant node:")
        # a '1 -1 1' mirror stored as a node transform renders inside-out in engines that ignore the
        # glTF winding-reversal rule (e.g. gz/ogre2); baking it into the vertices keeps every node at
        # positive determinant so the surface culls correctly.
        import numpy as np
        glb = assets.to_glb(stl, "1 -1 1")
        s = trimesh.load(trimesh.util.wrap_as_stream(glb), file_type="glb", force="scene")
        dets = [np.linalg.det(s.graph[n][0][:3, :3]) for n in s.graph.nodes_geometry]
        check(all(d > 0 for d in dets), f"mirrored visual GLB has no negative-determinant node ({[round(d,2) for d in dets]})")

        print("\nbaked filename is readable but the sha is taken over the bytes, not the name:")
        import shutil
        a = os.path.join(td, "alpha.stl"); b = os.path.join(td, "beta.stl")
        shutil.copy(stl, a); shutil.copy(stl, b)
        na, sa = assets.bake(a, None, "collision", bdir)
        nb, sb = assets.bake(b, None, "collision", bdir)
        check(na.startswith("alpha_") and nb.startswith("beta_"), f"filename keeps the source name ({na}, {nb})")
        check(na != nb and sa == sb, "different names, identical bytes -> same sha (name does not feed the hash)")
        short = na.rsplit("_", 1)[1].split(".")[0]
        check(sa.split(":")[1].startswith(short), "the short sha in the name is a prefix of the full @sha")

        print("\nflat material colour bakes into the GLB base colour (for STL visuals etc.):")
        # a plain STL has no material; a URDF <material> colour bakes into the GLB so it renders right
        glb_col = assets.to_glb(stl, None, "0.2 0.7 0.85 1.0")
        sc = trimesh.load(trimesh.util.wrap_as_stream(glb_col), file_type="glb", force="scene")
        mats = [getattr(g.visual, "material", None) for g in sc.geometry.values()]
        bcf = [tuple(m.baseColorFactor[:3]) for m in mats if m is not None]
        check(bool(bcf) and all(abs(c[0] - 51) <= 1 and abs(c[1] - 178) <= 1 for c in bcf),
              f"GLB base color matches the requested rgba ({bcf})")
        check(assets.to_glb(stl, None, None) != glb_col, "colour actually changes the exported bytes")

        # a mesh that already carries its own material (e.g. a .dae) must NOT be flattened by a colour
        from trimesh.visual import TextureVisuals
        from trimesh.visual.material import PBRMaterial
        red = trimesh.creation.box(extents=(0.2, 0.2, 0.2))
        red.visual = TextureVisuals(material=PBRMaterial(baseColorFactor=[200, 0, 0, 255]))
        red_glb = os.path.join(td, "red.glb")
        with open(red_glb, "wb") as fh:
            fh.write(red.export(file_type="glb"))
        out = assets.to_glb(red_glb, None, "0 1 0 1")   # ask for green; should be ignored
        sr = trimesh.load(trimesh.util.wrap_as_stream(out), file_type="glb", force="scene")
        kept = [tuple(int(x) for x in g.visual.material.baseColorFactor[:3]) for g in sr.geometry.values()]
        check(bool(kept) and all(c[0] > 150 and c[1] < 60 for c in kept),
              f"mesh with its own material keeps it, passed colour ignored ({kept})")

    print(f"\nassets tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
