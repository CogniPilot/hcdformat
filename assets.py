"""Mesh asset pipeline for HCDF.

Turns source meshes into content-addressed GLB and stamps the integrity hashes the rest of
the toolchain relies on (the @sha that urdf_hcdf import deliberately left empty):

  * a **visual** is GLB-only (baked appearance), so a non-GLB visual ``<model>`` (e.g. the .dae
    a URDF import produced) is converted to an embedded GLB via trimesh, written content-addressed
    (``<sha>.glb``), the ``@uri`` rewritten to it, and ``@sha`` set;
  * a **collision** ``<mesh>`` stays lean (STL/OBJ/convex — collision needs shape, not appearance),
    so it is only hashed in place (``@sha`` set, ``@source-uri`` recorded), never converted.

GLB conversion is byte-deterministic (verified), so the sha is a stable content address. Requires
``trimesh`` (+ ``pygltflib`` for inspection); import is lazy so the rest of hcdf works without them.

    import assets
    manifest = assets.stamp(doc, base_dir="…", out_dir="…/glb", package_paths={"pr2_description": "…"})
"""
from __future__ import annotations

import hashlib
import os

_GLB_EXT = (".glb", ".gltf")


# ── content addressing ────────────────────────────────────────────────────────
def content_sha(data: bytes) -> str:
    """A content address for asset bytes: ``"sha256:<hex>"``."""
    return "sha256:" + hashlib.sha256(data).hexdigest()


def file_sha(path: str) -> str:
    with open(path, "rb") as fh:
        return content_sha(fh.read())


# ── URI resolution ────────────────────────────────────────────────────────────
def resolve_uri(uri, base_dir=".", package_paths=None):
    """Resolve a mesh URI to an existing filesystem path, or None.

    Supports file://, package://<pkg>/<rest> (via ``package_paths`` map), absolute, and
    paths relative to ``base_dir``.
    """
    if not uri:
        return None
    if uri.startswith("file://"):
        path = uri[len("file://"):]
    elif uri.startswith("package://"):
        pkg, _, tail = uri[len("package://"):].partition("/")
        root = (package_paths or {}).get(pkg)
        if root is None:
            return None
        path = os.path.join(root, tail)
    elif os.path.isabs(uri):
        path = uri
    else:
        path = os.path.join(base_dir, uri)
    path = os.path.normpath(path)
    return path if os.path.exists(path) else None


# ── mesh -> GLB ───────────────────────────────────────────────────────────────
def to_glb(src_path) -> bytes:
    """Load a source mesh (STL/OBJ/DAE/PLY/glTF/…) and export a deterministic embedded GLB."""
    import warnings

    import trimesh
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        scene = trimesh.load(src_path, force="scene")
        return scene.export(file_type="glb")


def convert_to_glb(src_path, out_dir=None):
    """Convert a mesh to GLB; return (glb_bytes, sha). If out_dir, write ``<hex>.glb`` there."""
    data = to_glb(src_path)
    sha = content_sha(data)
    if out_dir is not None:
        os.makedirs(out_dir, exist_ok=True)
        with open(os.path.join(out_dir, sha.split(":")[1] + ".glb"), "wb") as fh:
            fh.write(data)
    return data, sha


# ── DOM stamping ──────────────────────────────────────────────────────────────
def stamp(doc, base_dir=".", out_dir=None, package_paths=None):
    """Content-address every mesh asset in the DOM, mutating it in place.

    Visual ``<model>``: a non-GLB source is converted to GLB (requires ``out_dir`` to write it),
    its ``@uri`` rewritten to the content-addressed ``<hex>.glb`` and ``@sha`` set; an existing GLB
    is hashed in place. Collision ``<mesh>``: hashed in place (kept lean), ``@source-uri`` recorded.

    Returns a manifest: list of dicts ``{site, kind, status, ...}``.
    """
    manifest = []
    for comp in (doc.comp or []):
        for v in (comp.visual or []):
            if v.model is not None and v.model.uri:
                manifest.append(_stamp_model(v.model, base_dir, out_dir, package_paths,
                                             f"{comp.name}/{v.name}"))
        for c in (comp.collision or []):
            mesh = getattr(c.geometry, "mesh", None) if c.geometry is not None else None
            if mesh is not None and mesh.uri:
                manifest.append(_stamp_collision(mesh, base_dir, package_paths, f"{comp.name}/{c.name}"))
    return manifest


def _stamp_model(model, base_dir, out_dir, package_paths, site):
    path = resolve_uri(model.uri, base_dir, package_paths)
    if path is None:
        return {"site": site, "kind": "visual-model", "uri": model.uri, "status": "unresolved"}
    if path.lower().endswith(_GLB_EXT):
        model.sha = file_sha(path)
        return {"site": site, "kind": "visual-model", "uri": model.uri, "sha": model.sha,
                "status": "hashed-glb"}
    if out_dir is None:
        return {"site": site, "kind": "visual-model", "uri": model.uri,
                "status": "needs-conversion (pass out_dir to bake the GLB)"}
    src = model.uri
    try:
        _data, sha = convert_to_glb(path, out_dir)
    except Exception as e:  # noqa: BLE001
        return {"site": site, "kind": "visual-model", "uri": model.uri,
                "status": f"conversion-failed: {type(e).__name__}: {e}"}
    model.sha = sha
    model.uri = sha.split(":")[1] + ".glb"   # content-addressed, relative to out_dir
    return {"site": site, "kind": "visual-model", "src": src, "uri": model.uri, "sha": sha,
            "status": "converted-to-glb"}


def _stamp_collision(mesh, base_dir, package_paths, site):
    path = resolve_uri(mesh.uri, base_dir, package_paths)
    if path is None:
        return {"site": site, "kind": "collision-mesh", "uri": mesh.uri, "status": "unresolved"}
    mesh.sha = file_sha(path)
    if mesh.source_uri is None:
        mesh.source_uri = mesh.uri
    return {"site": site, "kind": "collision-mesh", "uri": mesh.uri, "sha": mesh.sha,
            "status": "hashed-lean"}
