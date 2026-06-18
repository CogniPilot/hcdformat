"""URDF -> HCDF import: the mechanical core plus an import quarantine pass.

Mapping spine: ``<robot>`` -> ``<hcdf>``, ``<link>`` -> ``<comp>``, ``<joint>`` -> ``<joint>``,
top-level ``<material>`` -> ``<color>``. URDF is always FLU/ENU, so the imported document is
tagged body-frame="FLU" world-frame="ENU" and needs no pose transform (frame conversion is
an *export*-side concern).

Import quarantine pass: every top-level ``<robot>`` child that is not a typed-core
kind (link / joint / material) is quarantined verbatim into a root ``<extension>`` grouped by
domain (gazebo, transmission, ros2_control, unknown vendor tags), so non-URDF content survives
round-trip without polluting the clean core.

Known increment-1 losses (recorded in the returned notes; a structured loss manifest is step 6):
  * a URDF visual mesh becomes a GLB ``<model>`` placeholder (uri = the source mesh); the
    actual GLB bake + @sha is the asset pipeline (step 5). A ``<material>`` on a mesh visual is
    dropped (appearance bakes into the GLB).
  * ``<texture>`` on a material is dropped (bakes to GLB).
  * xacro is assumed already expanded.
"""
from __future__ import annotations

import copy
import os

from lxml import etree

from hcdfdom import model as M

# URDF top-level children that map to the typed core; everything else is quarantined.
_TYPED_TOPLEVEL = {"link", "joint", "material"}

# URDF joint type -> HCDF JointType value (all others map 1:1).
_JTYPE = {"floating": "free"}

_DOMAIN = {
    "gazebo": "org.gazebosim",
    "transmission": "org.ros.control",
    "ros2_control": "org.ros2.control",
}


def _ln(el):
    return etree.QName(el.tag).localname


def _kids(el):
    return [c for c in el if isinstance(c.tag, str)]


def _find(el, name):
    if el is None:
        return None
    return next((c for c in _kids(el) if _ln(c) == name), None)


def _findall(el, name):
    return [c for c in _kids(el) if _ln(c) == name] if el is not None else []


def _parse(src):
    if isinstance(src, (bytes, bytearray)):
        return etree.fromstring(src)
    if isinstance(src, str) and src.lstrip().startswith("<"):
        return etree.fromstring(src.encode("utf-8"))
    return etree.parse(str(src)).getroot()


# ── poses & geometry ──────────────────────────────────────────────────────────
def _origin_to_pose(org):
    if org is None:
        return None
    p = M.Pose()
    if org.get("xyz") is not None:
        p.xyz = org.get("xyz")
    if org.get("rpy") is not None:
        p.rpy = org.get("rpy")
    return p


def _primitive(geom, vg):
    """Fill a (visual|collision)_geometry dataclass `vg` from a URDF <geometry>'s primitive."""
    b = _find(geom, "box")
    if b is not None:
        box = M.Box()
        box.size = b.get("size")
        vg.box = box
        return True
    c = _find(geom, "cylinder")
    if c is not None:
        cyl = M.Cylinder()
        cyl.radius, cyl.length = c.get("radius"), c.get("length")
        vg.cylinder = cyl
        return True
    s = _find(geom, "sphere")
    if s is not None:
        sph = M.Sphere()
        sph.radius = s.get("radius")
        vg.sphere = sph
        return True
    return False


def _material_to_color(mat):
    col = M.Color()
    col.name = mat.get("name")
    c = _find(mat, "color")
    if c is not None and c.get("rgba") is not None:
        col.rgba = c.get("rgba")
    return col


def _material_rgba(mat, mat_colors):
    """Resolve a visual <material> to an 'r g b a' string: inline <color rgba>, or a named reference
    into the top-level material palette ``mat_colors``. None if it carries no flat colour."""
    if mat is None:
        return None
    c = _find(mat, "color")
    if c is not None and c.get("rgba") is not None:
        return c.get("rgba")
    name = mat.get("name")
    return mat_colors.get(name) if (name and mat_colors) else None


def _visual(v, link, idx, notes, baker=None, mat_colors=None):
    vis = M.Visual()
    vis.name = v.get("name") or f"{link}_visual_{idx}"
    if v.get("name") is None:
        vis.name_origin = M.NameOrigin.synthesized
    vis.pose = _origin_to_pose(_find(v, "origin"))
    geom = _find(v, "geometry")
    mesh = _find(geom, "mesh")
    mat = _find(v, "material")
    if mesh is not None:
        # ARM A: a visual mesh is a GLB <model>. Scale, mirror and the material colour all bake in.
        mr = M.ModelRef()
        mr.uri = mesh.get("filename")
        scale = mesh.get("scale")
        color = _material_rgba(mat, mat_colors)
        baked = baker(mr.uri, scale, "visual", color) if baker is not None else None
        if baked is not None:
            mr.uri, mr.sha = baked
            if mat is not None and color is None:
                notes.append(f"visual {vis.name!r}: <material> on a mesh visual carried no flat color; "
                             f"the GLB keeps the mesh's own appearance")
        else:
            if scale is not None and scale != "1 1 1":
                notes.append(f"visual {vis.name!r}: mesh scale {scale!r} not applied "
                             f"(bake the GLB with the meshes present to fold it in)")
            if mat is not None:
                notes.append(f"visual {vis.name!r}: <material> on a mesh visual dropped "
                             f"(appearance bakes into the GLB at the asset step)")
        vis.model = mr
    else:
        # ARM B: a primitive with an optional flat color.
        vg = M.VisualGeometry()
        if not _primitive(geom, vg):
            notes.append(f"visual {vis.name!r}: unsupported/empty <geometry> (no primitive)")
        vis.geometry = vg
        if mat is not None:
            vis.color = _material_to_color(mat)
            if _find(mat, "texture") is not None:
                notes.append(f"visual {vis.name!r}: material <texture> dropped (bakes to GLB)")
    return vis


def _collision(c, link, idx, notes, baker=None):
    col = M.Collision()
    col.name = c.get("name") or f"{link}_collision_{idx}"
    if c.get("name") is None:
        col.name_origin = M.NameOrigin.synthesized
    col.pose = _origin_to_pose(_find(c, "origin"))
    geom = _find(c, "geometry")
    cg = M.CollisionGeometry()
    mesh = _find(geom, "mesh")
    if mesh is not None:
        m = M.Mesh()
        m.uri = mesh.get("filename")
        scale = mesh.get("scale")
        baked = baker(m.uri, scale, "collision") if baker is not None else None
        if baked is not None:
            m.uri, m.sha = baked             # scale baked into the vertices (no negative scale left)
        elif scale is not None:              # not baked -> keep the scale so it round-trips
            m.scale = scale
        cg.mesh = m
    elif not _primitive(geom, cg):
        notes.append(f"collision {col.name!r}: unsupported/empty <geometry>")
    col.geometry = cg
    return col


def _inertial(ine):
    ip = M.InertialProperties()
    mass = _find(ine, "mass")
    if mass is not None:
        ip.mass = mass.get("value")
    org = _find(ine, "origin")
    if org is not None:
        ip.inertia_origin = _origin_to_pose(org)
    inr = _find(ine, "inertia")
    if inr is not None:
        ip.inertia = " ".join(inr.get(k, "0") for k in ("ixx", "ixy", "ixz", "iyy", "iyz", "izz"))
    return ip


def _link_to_comp(link, notes, baker=None, mat_colors=None):
    comp = M.Comp()
    comp.name = link.get("name")
    ine = _find(link, "inertial")
    if ine is not None:
        comp.inertial = _inertial(ine)
    for i, v in enumerate(_findall(link, "visual")):
        comp.visual.append(_visual(v, comp.name, i, notes, baker, mat_colors))
    for i, c in enumerate(_findall(link, "collision")):
        comp.collision.append(_collision(c, comp.name, i, notes, baker))
    return comp


# ── joints ──────────────────────────────────────────────────────────────────
def _joint(j, notes):
    jt = M.Joint()
    jt.name = j.get("name")
    utype = j.get("type")
    jt.type = M.JointType(_JTYPE.get(utype, utype))
    p, c = _find(j, "parent"), _find(j, "child")
    if p is not None:
        jt.parent = M.JointParent()
        jt.parent.comp = p.get("link")
    if c is not None:
        jt.child = M.JointChild()
        jt.child.comp = c.get("link")
    jt.origin = _origin_to_pose(_find(j, "origin"))
    ax = _find(j, "axis")
    if ax is not None:
        if utype not in ("fixed", "floating"):
            a = M.Axis()
            a.xyz = ax.get("xyz")
            jt.axis = a
        else:
            notes.append(f"joint {jt.name!r}: <axis> on a {utype} joint dropped (joint has no DOF)")
    lim = _find(j, "limit")
    if lim is not None:
        if utype not in ("fixed", "floating"):
            jl = M.JointLimit()
            for k in ("lower", "upper", "effort", "velocity"):
                if lim.get(k) is not None:
                    setattr(jl, k, lim.get(k))
            jt.limit = jl
        else:
            notes.append(f"joint {jt.name!r}: <limit> on a {utype} joint dropped (joint has no DOF)")
    dyn = _find(j, "dynamics")
    if dyn is not None:
        jd = M.JointDynamics()
        for k in ("damping", "friction"):
            if dyn.get(k) is not None:
                setattr(jd, k, dyn.get(k))
        jt.dynamics = jd
    mim = _find(j, "mimic")
    if mim is not None:
        mm = M.Mimic()
        mm.joint = mim.get("joint")
        for k in ("multiplier", "offset"):
            if mim.get(k) is not None:
                setattr(mm, k, mim.get(k))
        jt.mimic = mm
    cal = _find(j, "calibration")
    if cal is not None:
        jc = M.JointCalibration()
        for k in ("reference_position", "rising", "falling"):
            if cal.get(k) is not None:
                setattr(jc, k, cal.get(k))
        jt.calibration = jc
    sc = _find(j, "safety_controller")
    if sc is not None:
        s = M.SafetyController()
        for k in ("soft_lower_limit", "soft_upper_limit", "k_position", "k_velocity"):
            if sc.get(k) is not None:
                setattr(s, k, sc.get(k))
        if s.k_velocity is None:        # HCDF requires k_velocity
            s.k_velocity = "0"
            notes.append(f"joint {jt.name!r}: safety_controller k_velocity absent in source; defaulted to 0")
        uc = M.UrdfCompatJoint()
        uc.safety_controller = s
        jt.urdf_compat = uc
        notes.append(f"joint {jt.name!r}: <safety_controller> moved to <urdf-compat> (URDF-compat, zero-CPS)")
    return jt


# ── entry point ───────────────────────────────────────────────────────────────
def from_urdf(src, xacro_args=None, baker=None, packages=None):
    """Import URDF (path / string / bytes) into an hcdfdom Hcdf model.

    A ``.xacro`` path is expanded to URDF first via the optional ``xacro`` tool; ``xacro_args`` is
    a ``{name: value}`` dict of xacro arguments and ``packages`` ({name: path}) resolves
    ``$(find name)`` with no colcon build. If ``baker`` (an ``assets.Baker``) is given, each visual
    and collision mesh is resolved and baked to a canonical asset there, with the URDF scale applied,
    so HCDF never carries a source scale. Returns ``(doc, notes)``.
    """
    if isinstance(src, (str, os.PathLike)) and str(src).endswith(".xacro"):
        from . import xacro
        src = xacro.expand(src, xacro_args, packages)
    root = _parse(src)
    if _ln(root) != "robot":
        raise ValueError(f"not a URDF: root element is <{_ln(root)}>, expected <robot>")
    notes: list[str] = []

    doc = M.Hcdf()
    doc.name = root.get("name") or "robot"
    doc.body_frame = M.BodyFrame.FLU
    doc.world_frame = M.WorldFrame.ENU
    if root.get("version") is not None:
        notes.append(f"URDF robot/@version={root.get('version')!r} not preserved (a URDF document version is "
                     f"distinct from the HCDF schema version)")

    # typed core. A top-level material becomes a flat <color> only if it has a usable
    # <color rgba>; a texture-based (or rgba-less) material is NOT a flat color, so it is
    # quarantined verbatim (round-trips exactly; the asset pipeline bakes it to a GLB later).
    mat_quarantine = []
    mat_colors = {}   # name -> 'r g b a', so a visual that references a material by name bakes its colour
    for mat in _findall(root, "material"):
        if mat.get("name") is None:
            notes.append("top-level <material> without a name dropped (URDF requires a material name)")
            continue
        c = _find(mat, "color")
        if c is not None and c.get("rgba") is not None:
            doc.color.append(_material_to_color(mat))
            mat_colors[mat.get("name")] = c.get("rgba")
            if _find(mat, "texture") is not None:
                notes.append(f"material {mat.get('name')!r}: <texture> dropped, flat color kept (bake to GLB)")
        else:
            mat_quarantine.append(mat)
            notes.append(f"material {mat.get('name')!r}: texture-based (no flat rgba) -> quarantined to "
                         f"<extension domain='org.urdf.material'> for exact round-trip / GLB bake")
    for link in _findall(root, "link"):
        doc.comp.append(_link_to_comp(link, notes, baker, mat_colors))
    for j in _findall(root, "joint"):
        doc.joint.append(_joint(j, notes))

    # quarantine pass: move every non-typed top-level child into <extension> by domain
    quarantine: dict[str, list] = {}
    for ch in _kids(root):
        t = _ln(ch)
        if t in _TYPED_TOPLEVEL:
            continue
        quarantine.setdefault(_DOMAIN.get(t, f"urdf:{t}"), []).append(ch)
    if mat_quarantine:
        quarantine.setdefault("org.urdf.material", []).extend(mat_quarantine)
    for domain, els in sorted(quarantine.items()):
        ext = M.Extension()
        ext.domain = domain
        ext.any_content = [copy.deepcopy(e) for e in els]
        doc.extension.append(ext)
        kinds = sorted({_ln(e) for e in els})
        notes.append(f"quarantined {len(els)} top-level <{'/'.join(kinds)}> element(s) "
                     f"into <extension domain={domain!r}>")

    return doc, notes
