"""Mesh asset pipeline for HCDF.

Turns source meshes into content-addressed GLB and stamps the integrity hashes the rest of
the toolchain relies on (the @sha that urdf_hcdf import deliberately left empty):

  * a **visual** is GLB-only (baked appearance), so a non-GLB visual ``<model>`` (e.g. the .dae
    a URDF import produced) is converted to an embedded GLB via trimesh, written under a readable
    content-addressed name (``<source-name>_<short-sha>.glb``), the ``@uri`` rewritten to it, ``@sha`` set;
  * a **collision** ``<mesh>`` stays lean (STL/OBJ/convex — collision needs shape, not appearance),
    so it is only hashed in place (``@sha`` set, ``@source-uri`` recorded), never converted.

GLB conversion is byte-deterministic (verified), so the sha is a stable content address. Requires
``trimesh`` (+ ``pygltflib`` for inspection); import is lazy so the rest of hcdf works without them.

    from hcdf import assets
    manifest = assets.stamp(doc, base_dir="…", out_dir="…/glb", package_paths={"pr2_description": "…"})
"""
from __future__ import annotations

import hashlib
import os
import re

_GLB_EXT = (".glb", ".gltf")
_SHORT_SHA = 12  # hex chars of the content hash kept in a filename; the full sha lives in @sha


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


# ── scale baking ──────────────────────────────────────────────────────────────
def _scale_vec(scale):
    """Parse a 'sx sy sz' (or single value) scale string to a list, or None if absent/identity."""
    if not scale:
        return None
    parts = [float(x) for x in str(scale).split()]
    if len(parts) == 1:
        parts *= 3
    return None if all(abs(x - 1.0) < 1e-12 for x in parts) else parts


def _safe_stem(path):
    """A filesystem-safe stem from a source mesh path: its basename without the extension.

    Used only to make a baked filename readable; it never feeds the content hash.
    """
    stem = os.path.splitext(os.path.basename(str(path)))[0]
    return re.sub(r"[^0-9A-Za-z._-]", "_", stem) or "mesh"


def _color_vec(color):
    """Parse an 'r g b [a]' (0..1) colour string to an RGBA 0..255 list, or None if absent."""
    if not color:
        return None
    parts = [float(x) for x in str(color).split()]
    if len(parts) == 3:
        parts.append(1.0)
    if len(parts) != 4:
        return None
    return [max(0, min(255, int(round(c * 255)))) for c in parts]


# ── mesh -> GLB ───────────────────────────────────────────────────────────────
def to_glb(src_path, scale=None, color=None) -> bytes:
    """Load a source mesh (STL/OBJ/DAE/PLY/glTF/…) and export a deterministic embedded GLB.

    A ``scale`` ('sx sy sz', possibly negative for a mirror) is baked into the GLB so the result is
    self-contained in meters. The scale is folded into the vertices, not left as a node transform:
    a mirror is a negative-determinant transform, which the glTF spec says to render with reversed
    triangle winding but several engines (gz/ogre2 among them) ignore, leaving the surface inside-out
    and see-through. Folding it into the vertices flips the winding so every renderer culls correctly.

    A flat ``color`` ('r g b a', 0..1) is baked as the GLB base colour, but only onto geometry that
    carries no material of its own (a plain STL whose colour lived in the URDF/SDF ``<material>``). A
    mesh that already has materials (e.g. a .dae or glTF) keeps them, so its richer appearance is never
    flattened by a generic URDF material.
    """
    import warnings

    import numpy as np
    import trimesh
    vec = _scale_vec(scale)
    rgba = _color_vec(color)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        scene = trimesh.load(src_path, force="scene")
        if vec is not None:
            scene.apply_transform(np.diag([vec[0], vec[1], vec[2], 1.0]))
            # bake the node transforms into the geometry so no node carries the scale or the mirror;
            # dump() applies each node transform to its mesh, flipping winding where the determinant
            # is negative, leaving every exported node at identity
            scene = trimesh.Scene(scene.dump(concatenate=False))
        if rgba is not None:
            from trimesh.visual import TextureVisuals
            from trimesh.visual.material import PBRMaterial
            mat = PBRMaterial(baseColorFactor=rgba, metallicFactor=0.0, roughnessFactor=1.0)
            for g in scene.geometry.values():
                if not isinstance(g.visual, TextureVisuals):  # bare mesh (no own material) -> apply colour
                    g.visual = TextureVisuals(material=mat)
        return scene.export(file_type="glb")


def to_lean(src_path, scale=None, file_type="stl") -> bytes:
    """Bake ``scale`` into a lean collision mesh (into the vertices), mirror-safe. Returns bytes.

    Collision meshes stay lean (no appearance), but the scale is baked in so a physics engine never
    sees a non-positive (mirror) scale, which engines like DART reject.
    """
    import warnings

    import numpy as np
    import trimesh
    vec = _scale_vec(scale)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        mesh = trimesh.load(src_path, force="mesh")
        if vec is not None:
            mesh.apply_transform(np.diag([vec[0], vec[1], vec[2], 1.0]))
        data = mesh.export(file_type=file_type)
    return data if isinstance(data, bytes) else data.encode()


def convert_to_glb(src_path, out_dir=None, scale=None):
    """Convert a mesh to GLB; return (glb_bytes, sha). If out_dir, write ``<hex>.glb`` there."""
    data = to_glb(src_path, scale)
    sha = content_sha(data)
    if out_dir is not None:
        os.makedirs(out_dir, exist_ok=True)
        with open(os.path.join(out_dir, sha.split(":")[1] + ".glb"), "wb") as fh:
            fh.write(data)
    return data, sha


# ── inline bake (used by the converters so HCDF never has to carry a source scale) ──
def bake(src_path, scale, kind, out_dir, color=None):
    """Bake one mesh to a canonical asset in ``out_dir``. Returns (filename, sha).

    kind 'visual'  -> a GLB in meters with ``scale`` baked in, and ``color`` (an 'r g b a' from the
                      source ``<material>``) baked as the base colour. An input that is already a GLB
                      with no applied scale or colour is passed through unchanged, not re-exported.
    kind 'collision' -> a lean mesh with ``scale`` baked into the vertices (kept in its source format,
                      or STL if unknown). No scale means the source is passed through unchanged.

    The filename is ``<source-name>_<short-sha>.<ext>`` so the asset store stays readable and a minor
    mesh change lands beside the original. The name is cosmetic: the hash is taken over the file bytes,
    so identical content always yields the same ``@sha`` (and dedups to one file) whatever it is named.
    """
    os.makedirs(out_dir, exist_ok=True)
    vec = _scale_vec(scale)
    is_glb = src_path.lower().endswith(_GLB_EXT)
    if kind == "visual":
        if is_glb and vec is None and color is None:
            data = open(src_path, "rb").read()          # already canonical, just adopt it
            ext = os.path.splitext(src_path)[1].lstrip(".")
        else:
            data, ext = to_glb(src_path, scale, color), "glb"
    else:  # collision
        ext = os.path.splitext(src_path)[1].lstrip(".") or "stl"
        data = open(src_path, "rb").read() if vec is None else to_lean(src_path, scale, ext)
    sha = content_sha(data)
    name = f"{_safe_stem(src_path)}_{sha.split(':')[1][:_SHORT_SHA]}.{ext}"
    with open(os.path.join(out_dir, name), "wb") as fh:
        fh.write(data)
    return name, sha


class Baker:
    """Resolve and bake meshes during conversion, content-addressing into ``out_dir``.

    The converters call this per mesh so the source scale is consumed where it is read (the URDF/SDF),
    and HCDF never has to carry it. Returns (uri, sha) for a baked asset, or None if the mesh cannot
    be resolved (the caller then falls back to keeping the source reference).

    ``uri_prefix`` is prepended to the content-addressed filename so the written ``@uri`` is relative
    to the output document (e.g. ``assets/base_link_a48d8a3ed998.glb``). That lets a viewer resolve the
    mesh next to the document with no resource-path setup, including when a tool spawns it from its file.
    """

    def __init__(self, out_dir, base_dir=".", package_paths=None, uri_prefix=""):
        self.out_dir = out_dir
        self.base_dir = base_dir
        self.package_paths = package_paths or {}
        self.uri_prefix = uri_prefix

    def __call__(self, uri, scale, kind, color=None):
        path = resolve_uri(uri, self.base_dir, self.package_paths)
        if path is None:
            return None
        name, sha = bake(path, scale, kind, self.out_dir, color)
        if self.uri_prefix and self.uri_prefix != ".":
            name = self.uri_prefix.rstrip("/") + "/" + name
        return name, sha


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
