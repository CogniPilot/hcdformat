"""SDF → HCDF import.

A peer spoke on the hcdfdom IR, parallel to ``urdf_hcdf.from_urdf`` — SDF is read with lxml and the
*overlap* is mapped onto the typed DOM. SDF, like URDF, is FLU/ENU, so NO frame transform is
applied on import (frame conversion is an export-side concern).

Scope (this version maps the mechanical overlap value-exactly):
- ``<model>`` → ``<hcdf>``; ``<link>`` → ``<comp>``; ``<joint>`` → ``<joint>``;
  ``<inertial>`` / ``<collision>+<surface>`` / ``<visual>+<material>`` / ``<geometry>`` /
  ``<axis>+<limit>+<dynamics>`` / ``<mimic>``.
Deferred / loss-reported (no clean home yet — recorded in ``notes``, never silent):
- SDF sensors (the per-device→typed-container re-root needs a shared sensor re-root, not built yet);
- ``relative_to`` / ``expressed_in`` frame-graph attributes and non-identity ``<link>`` poses
  (HCDF flattens frames and a comp has no own pose);
- SDF-only surface (``<world>``, multiple/nested ``<model>``, ``<light>``/``<actor>``/``<frame>``,
  PBR ``<material><pbr>``, ODE-specific surface params).
"""
from __future__ import annotations

from lxml import etree

from hcdfdom import model as M

from .gz import SDF_VERSION

# SDF joint types that map 1:1 to an HCDF joint type. (SDF 'revolute2'/'gearbox' have no HCDF home.)
_JTYPE = {"revolute", "prismatic", "fixed", "continuous", "ball", "universal", "screw"}
_SHAPES = ("box", "cylinder", "sphere", "capsule", "cone", "ellipsoid")


def _txt(el):
    return el.text.strip() if el is not None and el.text else None


def _pose(el, notes, ctx):
    """SDF <pose>x y z r p y</pose> → structured HCDF Pose (xyz + rpy). relative_to/quat → note."""
    if el is None:
        return None
    if el.get("relative_to") or el.get("expressed_in") or el.get("rotation_format") or el.get("degrees"):
        attr = el.get("relative_to") or el.get("expressed_in")
        notes.append(f"{ctx}: <pose> frame qualifier ({attr!r}/format) not represented; "
                     f"pose taken as element-local")
    vals = (el.text or "").split()
    if len(vals) != 6:
        notes.append(f"{ctx}: <pose> has {len(vals)} values (expected 6); dropped")
        return None
    p = M.Pose()
    p.xyz = " ".join(vals[:3])
    p.rpy = " ".join(vals[3:])
    return p


def _geometry(gel, notes, ctx, *, klass, allow_mesh):
    """Map an SDF <geometry> to a (visual|collision)_geometry. Returns (geo, mesh_uri_or_None)."""
    if gel is None:
        return None, None
    geo = klass()
    box, cyl, sph = gel.find("box"), gel.find("cylinder"), gel.find("sphere")
    cap, cone, ell, mesh = gel.find("capsule"), gel.find("cone"), gel.find("ellipsoid"), gel.find("mesh")
    if box is not None:
        geo.box = M.Box(); geo.box.size = _txt(box.find("size"))
    elif cyl is not None:
        geo.cylinder = M.Cylinder()
        geo.cylinder.radius, geo.cylinder.length = _txt(cyl.find("radius")), _txt(cyl.find("length"))
    elif sph is not None:
        geo.sphere = M.Sphere(); geo.sphere.radius = _txt(sph.find("radius"))
    elif cap is not None:
        geo.capsule = M.Capsule()
        geo.capsule.radius, geo.capsule.length = _txt(cap.find("radius")), _txt(cap.find("length"))
    elif cone is not None:
        geo.cone = M.Cone()
        geo.cone.radius, geo.cone.length = _txt(cone.find("radius")), _txt(cone.find("length"))
    elif ell is not None:
        geo.ellipsoid = M.Ellipsoid(); geo.ellipsoid.radii = _txt(ell.find("radii"))
    elif allow_mesh and mesh is not None:
        m = M.Mesh(); m.uri = _txt(mesh.find("uri"))
        scale = _txt(mesh.find("scale"))
        if scale and scale != "1 1 1":
            m.scale = scale
        geo.mesh = m
        return geo, m.uri
    elif mesh is not None:
        return None, _txt(mesh.find("uri"))  # a visual mesh -> ARM A model, handled by caller
    else:
        other = next((c.tag for c in gel if isinstance(c.tag, str)), "empty")
        notes.append(f"{ctx}: <geometry><{other}> has no HCDF mapping; dropped")
        return None, None
    return geo, None


def _surface(sel, notes, ctx):
    """SDF <surface> → HCDF <surface> (friction + restitution + contact); ODE extras → note."""
    if sel is None:
        return None
    surf = M.Surface()
    fr = sel.find("friction")
    if fr is not None:
        ode = fr.find("ode")
        mu = _txt(ode.find("mu")) if ode is not None else _txt(fr.find("mu"))
        mu2 = _txt(ode.find("mu2")) if ode is not None else _txt(fr.find("mu2"))
        if mu is not None or mu2 is not None:
            surf.friction = M.Friction()
            surf.friction.static = mu
            surf.friction.dynamic = mu2
    bounce = sel.find("bounce")
    if bounce is not None:
        rc = _txt(bounce.find("restitution_coefficient"))
        if rc is not None:
            surf.restitution = rc
    cn = sel.find("contact")
    if cn is not None:
        ode = cn.find("ode")
        kp = _txt(ode.find("kp")) if ode is not None else None
        kd = _txt(ode.find("kd")) if ode is not None else None
        if kp is not None or kd is not None:
            surf.contact = M.Contact()
            surf.contact.stiffness, surf.contact.damping = kp, kd
    notes.append(f"{ctx}: <surface> mapped (friction/restitution/contact); ODE-specific params not "
                 f"represented are dropped")
    return surf if (surf.friction or surf.restitution or surf.contact) else None


def _inertial(iel, notes, ctx):
    ip = M.InertialProperties()
    ip.mass = _txt(iel.find("mass"))
    ip.inertia_origin = _pose(iel.find("pose"), notes, ctx + " inertial")
    inr = iel.find("inertia")
    if inr is not None:
        # SDF inertia: ixx/iyy/izz default 1.0, off-diagonals default 0.0; a partial <inertia> is valid.
        defaults = {"ixx": "1.0", "ixy": "0.0", "ixz": "0.0", "iyy": "1.0", "iyz": "0.0", "izz": "1.0"}
        ip.inertia = " ".join(_txt(inr.find(k)) or defaults[k] for k in defaults)
    if iel.find("auto") is not None or iel.get("auto"):
        notes.append(f"{ctx}: <inertial auto> (auto-computed inertia) not represented")
    return ip


def _link(lel, notes, baker=None):
    comp = M.Comp()
    comp.name = lel.get("name")
    ctx = f"link {comp.name!r}"
    pel = lel.find("pose")
    if pel is not None:
        lp = _pose(pel, notes, ctx)  # pass the real notes list so a relative_to/expressed_in note is kept
        if lp is not None and (pel.text or "").split() != ["0"] * 6:
            notes.append(f"{ctx}: <link><pose> is non-identity; HCDF comps have no own pose "
                         f"(placement comes from joints) — link pose not represented (frame-graph flattening)")
    if lel.find("inertial") is not None:
        comp.inertial = _inertial(lel.find("inertial"), notes, ctx)
    for cel in lel.findall("collision"):
        col = M.Collision()
        col.name = cel.get("name")
        if col.name is None:
            col.name = f"{comp.name}_collision"
            col.name_origin = M.NameOrigin("synthesized")
        col.pose = _pose(cel.find("pose"), notes, ctx + f" collision {col.name!r}")
        geo, _ = _geometry(cel.find("geometry"), notes, ctx + f" collision {col.name!r}",
                           klass=M.CollisionGeometry, allow_mesh=True)
        if geo is not None and getattr(geo, "mesh", None) is not None and geo.mesh.uri and baker is not None:
            baked = baker(geo.mesh.uri, geo.mesh.scale, "collision")
            if baked is not None:
                geo.mesh.uri, geo.mesh.sha = baked   # scale baked into the vertices (no negative scale left)
                geo.mesh.scale = None
        col.geometry = geo
        col.surface = _surface(cel.find("surface"), notes, ctx + f" collision {col.name!r}")
        if geo is not None:
            comp.collision.append(col)
    for vel in lel.findall("visual"):
        vis = M.Visual()
        vis.name = vel.get("name")
        if vis.name is None:
            vis.name = f"{comp.name}_visual"
            vis.name_origin = M.NameOrigin("synthesized")
        vis.pose = _pose(vel.find("pose"), notes, ctx + f" visual {vis.name!r}")
        geo, mesh_uri = _geometry(vel.find("geometry"), notes, ctx + f" visual {vis.name!r}",
                                  klass=M.VisualGeometry, allow_mesh=False)
        if mesh_uri is not None:
            vis.model = M.ModelRef(); vis.model.uri = mesh_uri  # a visual mesh carries its appearance in the GLB model
            scale = _txt(vel.find("geometry/mesh/scale"))
            mat = vel.find("material")
            color = _txt(mat.find("diffuse")) if mat is not None else None
            baked = baker(mesh_uri, scale, "visual", color) if baker is not None else None
            if baked is not None:
                vis.model.uri, vis.model.sha = baked   # scale, mirror and material colour baked into the GLB
                if mat is not None and color is None:
                    notes.append(f"{ctx} visual {vis.name!r}: <material> carried no diffuse colour; "
                                 f"the GLB keeps the mesh's own appearance")
            else:
                notes.append(f"{ctx} visual {vis.name!r}: mesh -> <model uri> (GLB bake/@sha deferred to assets)")
                if scale and scale != "1 1 1":
                    notes.append(f"{ctx} visual {vis.name!r}: mesh <scale> {scale!r} not applied "
                                 f"(bake the GLB with the meshes present to fold it in)")
                if mat is not None:
                    notes.append(f"{ctx} visual {vis.name!r}: <material> on a mesh visual not represented "
                                 f"(its appearance is baked into the GLB model)")
        elif geo is not None:
            vis.geometry = geo
            mat = vel.find("material")
            if mat is not None:
                diffuse = _txt(mat.find("diffuse"))
                if diffuse is not None:
                    vis.color = M.Color(); vis.color.rgba = diffuse
                if mat.find("pbr") is not None or mat.find("ambient") is not None or mat.find("specular") is not None:
                    notes.append(f"{ctx} visual {vis.name!r}: <material> ambient/specular/PBR not "
                                 f"represented (rich appearance is baked into the GLB model)")
        if vis.model is not None or vis.geometry is not None:
            comp.visual.append(vis)
    if lel.findall("sensor"):
        notes.append(f"{ctx}: {len(lel.findall('sensor'))} <sensor>(s) not mapped (typed sensor re-root "
                     f"is deferred); dropped")
    return comp


def _joint(jel, notes):
    j = M.Joint()
    j.name = jel.get("name")
    ctx = f"joint {j.name!r}"
    jt = jel.get("type")
    parent, child = _txt(jel.find("parent")), _txt(jel.find("child"))
    if parent is not None:
        j.parent = M.JointParent(); j.parent.comp = parent
    if child is not None:
        j.child = M.JointChild(); j.child.comp = child
    j.origin = _pose(jel.find("pose"), notes, ctx)
    if jt not in _JTYPE:
        # No HCDF equivalent -> 'fixed'. A fixed joint forbids axis/limit, so we must NOT carry them
        # over (that would produce an invalid HCDF doc) — record the drop instead.
        j.type = M.JointType("fixed")
        notes.append(f"{ctx}: SDF joint type {jt!r} has no HCDF equivalent; imported as 'fixed'")
        for tag in ("axis", "axis2", "thread_pitch", "screw_thread_pitch"):
            if jel.find(tag) is not None:
                notes.append(f"{ctx}: <{tag}> dropped with the {jt}->fixed downgrade")
        return j
    j.type = M.JointType(jt)
    axis = jel.find("axis")
    if axis is not None:
        xel = axis.find("xyz")
        if xel is not None:
            if xel.get("expressed_in"):
                notes.append(f"{ctx}: <axis><xyz expressed_in={xel.get('expressed_in')!r}> frame qualifier "
                             f"not represented; axis taken as element-local")
            if _txt(xel) is not None:
                j.axis = M.Axis(); j.axis.xyz = _txt(xel)
        lim = axis.find("limit")
        if lim is not None:
            jl = M.JointLimit()
            jl.lower, jl.upper = _txt(lim.find("lower")), _txt(lim.find("upper"))
            jl.effort, jl.velocity = _txt(lim.find("effort")), _txt(lim.find("velocity"))
            if any((jl.lower, jl.upper, jl.effort, jl.velocity)):
                j.limit = jl
        dyn = axis.find("dynamics")
        if dyn is not None:
            jd = M.JointDynamics()
            jd.damping, jd.friction = _txt(dyn.find("damping")), _txt(dyn.find("friction"))
            jd.spring_stiffness, jd.spring_reference = _txt(dyn.find("spring_stiffness")), _txt(dyn.find("spring_reference"))
            if any((jd.damping, jd.friction, jd.spring_stiffness, jd.spring_reference)):
                j.dynamics = jd
    axis2 = jel.find("axis2")
    if axis2 is not None:
        if _txt(axis2.find("xyz")) is not None:
            j.axis2 = M.Axis(); j.axis2.xyz = _txt(axis2.find("xyz"))
        if axis2.find("limit") is not None or axis2.find("dynamics") is not None:
            notes.append(f"{ctx}: <axis2> <limit>/<dynamics> not represented "
                         f"(HCDF Joint has a single limit/dynamics slot)")
    tp = _txt(jel.find("thread_pitch")) or _txt(jel.find("screw_thread_pitch"))
    if tp is not None:
        j.thread_pitch = tp
    mim = jel.find("mimic") if jel.find("mimic") is not None else (axis.find("mimic") if axis is not None else None)
    if mim is not None:
        j.mimic = M.Mimic()
        j.mimic.joint = mim.get("joint") or _txt(mim.find("joint"))
        j.mimic.multiplier, j.mimic.offset = _txt(mim.find("multiplier")), _txt(mim.find("offset"))
    return j


def from_sdf(src, baker=None):
    """Import SDF (path / str / bytes) → (hcdfdom.Hcdf, notes). ``notes`` records every loss/deferral.

    If ``baker`` (an ``assets.Baker``) is given, each visual and collision mesh is resolved and baked
    to a canonical asset there, with the SDF scale applied, so HCDF never carries a source scale.

    Parses with ``recover=True``: libsdformat's URDF→SDF output for legacy models can contain
    undeclared-prefix tags (e.g. old-style ``<sensor:contact>``) that strict XML rejects; recovery
    lets the mechanical overlap still import (the malformed legacy bits are dropped, not mapped).
    """
    parser = etree.XMLParser(recover=True)
    if isinstance(src, (bytes, bytearray)):
        root = etree.fromstring(bytes(src), parser)
    elif isinstance(src, str) and "<" in src:
        root = etree.fromstring(src.encode(), parser)
    else:
        root = etree.parse(str(src), parser).getroot()
    if root is None:
        raise ValueError("could not parse SDF document")
    if etree.QName(root).localname != "sdf":
        raise ValueError(f"not an SDF document (root <{etree.QName(root).localname}>)")

    notes = []
    ver = root.get("version")
    if ver and ver != SDF_VERSION:
        notes.append(f"SDF version {ver!r} != pinned {SDF_VERSION!r}; mapping may be imperfect")
    if root.findall("world"):
        notes.append(f"document has {len(root.findall('world'))} <world>(s); world/sim composition is "
                     f"out of HCDF-core scope — only the model is imported")
    models = root.findall("model") or [m for w in root.findall("world") for m in w.findall("model")]
    if not models:
        raise ValueError("SDF document has no <model>")
    if len(models) > 1:
        notes.append(f"{len(models)} top-level <model>s; only the first ({models[0].get('name')!r}) is "
                     f"imported (multi-model is out of HCDF-core scope)")
    model = models[0]

    doc = M.Hcdf()
    doc.name = model.get("name")
    doc.body_frame, doc.world_frame = M.BodyFrame("FLU"), M.WorldFrame("ENU")  # SDF is FLU/ENU; no transform
    for sub in model.findall("model"):
        notes.append(f"model {doc.name!r}: nested <model {sub.get('name')!r}> not represented "
                     f"(nested models are out of HCDF-core scope)")
    for tag in ("frame", "light", "static", "self_collide", "plugin", "gripper"):
        if model.findall(tag):
            notes.append(f"model {doc.name!r}: {len(model.findall(tag))} <{tag}> not represented "
                         f"(SDF-only / out of HCDF-core scope)")
    doc.comp = [_link(l, notes, baker) for l in model.findall("link")]
    doc.joint = [_joint(j, notes) for j in model.findall("joint")]
    return doc, notes
