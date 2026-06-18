"""HCDF → SDF export.

The reverse of ``from_sdf``: walk the typed hcdfdom DOM and emit SDFormat at the lxml string layer
(never through a lossy path), parallel to ``urdf_hcdf.to_urdf``. SDF is a *wider* peer than URDF, so
this export is cleaner than HCDF→URDF: SDF natively expresses closed kinematic **loops**, the
**ball/universal/screw** joint types, **all** HCDF geometry primitives (capsule/cone/ellipsoid), and
collision **<surface>** physics — none of which survive HCDF→URDF. The HCDF *cyber* layer
(motors/networks/power/TSN/transmissions, accel/jerk limits, sensors) still has no SDF home → it is
recorded in the same structured ``LossManifest`` ``urdf_hcdf`` uses.

Frames: SDF is FLU/ENU. A body-frame="FRD" document has its body poses and joint axes converted to
FLU on export (t'=C·t, R'=C·R·Cᵀ, a'=C·a); world-frame="NED" is recorded as a loss (not yet applied),
exactly as on the URDF path.
"""
from __future__ import annotations

from lxml import etree

from hcdfdom import frames
from hcdfdom.model import BodyFrame, WorldFrame
from urdf_hcdf.to_urdf import LossManifest

from .gz import SDF_VERSION

# HCDF joint type -> SDF joint type. SDF keeps ball/universal/screw (URDF cannot); only 'free' and
# 'cylindrical' have no SDF home.
_JTYPE_OUT = {"revolute": "revolute", "continuous": "continuous", "prismatic": "prismatic",
              "fixed": "fixed", "ball": "ball", "universal": "universal", "screw": "screw"}


def _val(x):
    return x.value if hasattr(x, "value") else x


def _sub(parent, tag, text=None, **attrs):
    """Create a container/attr-bearing element (always emitted; children appended by the caller)."""
    e = etree.SubElement(parent, tag)
    for k, v in attrs.items():
        if v is not None:
            e.set(k, v)
    if text is not None:
        e.text = text
    return e


def _leaf(parent, tag, value):
    """Create a value-bearing leaf ONLY when value is not None — libsdformat rejects empty typed
    elements (e.g. an empty <spring_stiffness/>), so optional scalars must be omitted, not blanked."""
    if value is None:
        return None
    e = etree.SubElement(parent, tag)
    e.text = value
    return e


class _Exporter:
    def __init__(self, doc):
        self.doc = doc
        self.loss = LossManifest()
        self.body = doc.body_frame
        self.world = doc.world_frame
        self._C = (frames.change_of_basis(self.body, BodyFrame.FLU)
                   if self.body == BodyFrame.FRD else None)

    def _conv_axis(self, xyz):
        if self._C is None or xyz is None:
            return xyz
        return frames._fmt(self._C @ frames._floats(xyz))

    def _pose_text(self, pose):
        """An HCDF pose -> SDF '<pose>x y z r p y' text (quat->rpy, FRD->FLU)."""
        if pose is None:
            return None
        if self._C is not None or pose.quat:
            out = frames.transform_pose(pose, self._C if self._C is not None else frames._I3, use_quat=False)
            xyz, rpy = out.xyz, out.rpy
        else:
            xyz, rpy = pose.xyz, pose.rpy
        xyz = xyz or "0 0 0"
        rpy = rpy or "0 0 0"
        return f"{xyz} {rpy}"

    def _emit_pose(self, parent, pose, relative_to=None, force=False):
        t = self._pose_text(pose)
        if t is None and not force:
            return
        e = _sub(parent, "pose", text=(t or "0 0 0 0 0 0"))
        if relative_to is not None:
            e.set("relative_to", relative_to)

    def _geometry(self, parent, geo, *, allow_mesh, ctx):
        g = etree.Element("geometry")
        if geo.box is not None:
            _leaf(_sub(g, "box"), "size", geo.box.size)
        elif geo.cylinder is not None:
            c = _sub(g, "cylinder"); _leaf(c, "radius", geo.cylinder.radius); _leaf(c, "length", geo.cylinder.length)
        elif geo.sphere is not None:
            _leaf(_sub(g, "sphere"), "radius", geo.sphere.radius)
        elif getattr(geo, "capsule", None) is not None:
            c = _sub(g, "capsule"); _leaf(c, "radius", geo.capsule.radius); _leaf(c, "length", geo.capsule.length)
        elif getattr(geo, "cone", None) is not None:
            c = _sub(g, "cone"); _leaf(c, "radius", geo.cone.radius); _leaf(c, "length", geo.cone.length)
        elif getattr(geo, "ellipsoid", None) is not None:
            _leaf(_sub(g, "ellipsoid"), "radii", getattr(geo.ellipsoid, "radii", None))
        elif allow_mesh and getattr(geo, "mesh", None) is not None:
            m = _sub(g, "mesh"); _leaf(m, "uri", geo.mesh.uri)
            if geo.mesh.scale:
                _leaf(m, "scale", geo.mesh.scale)
        else:
            self.loss.add("geometry", f"{ctx}: no SDF-representable shape; dropped")
            return False
        parent.append(g)
        return True

    def _surface(self, parent, surf):
        s = etree.Element("surface")
        if surf.friction is not None and (surf.friction.static or surf.friction.dynamic):
            ode = _sub(_sub(s, "friction"), "ode")
            _leaf(ode, "mu", surf.friction.static)
            _leaf(ode, "mu2", surf.friction.dynamic)
        if surf.restitution is not None:
            _leaf(_sub(s, "bounce"), "restitution_coefficient", _val(surf.restitution))
        if surf.contact is not None and (surf.contact.stiffness or surf.contact.damping):
            ode = _sub(_sub(s, "contact"), "ode")
            _leaf(ode, "kp", surf.contact.stiffness)
            _leaf(ode, "kd", surf.contact.damping)
        if len(s):
            parent.append(s)

    def _color(self, parent, color):
        """A visual <color> -> SDF <material><diffuse>. A name-only ref is resolved to its rgba."""
        rgba = color.rgba
        if rgba is None and color.name is not None:
            ref = next((c for c in (self.doc.color or []) if c.name == color.name and c.rgba), None)
            if ref is not None:
                rgba = ref.rgba
                self.loss.add("material", f"color {color.name!r}: SDF has no color palette; inlined rgba "
                                          f"(the shared name is not preserved)")
        if rgba is not None:
            _leaf(_sub(parent, "material"), "diffuse", rgba)

    def _link(self, model, comp):
        if comp.name == "world":
            # SDF reserves 'world' as the implicit world frame, so a model cannot declare a link
            # named 'world'. Skip it and let any joint anchored to it reference the world frame
            # directly (the joint keeps <parent>world</parent>), which fixes the model to the world.
            if comp.inertial is not None or comp.visual or comp.collision:
                self.loss.add("comp", "comp 'world' carried geometry or inertia; dropped because SDF "
                                      "reserves 'world' as the implicit world frame")
            return
        name = comp.name
        if name is None:
            name = "unnamed_link"
            self.loss.add("comp", "a comp has no name; emitted SDF <link> name synthesized "
                                  f"({name!r}) to keep the SDF valid")
        link = _sub(model, "link", name=name)
        # Place the link at its parent joint (URDF/HCDF position links through joints, SDF through
        # link poses). The joint carries the offset, so the child link is identity relative to it.
        jn = self._child_joint.get(comp.name)
        if jn is not None:
            pe = etree.SubElement(link, "pose")
            pe.set("relative_to", jn)
            pe.text = "0 0 0 0 0 0"
        if comp.inertial is not None:
            ip = comp.inertial
            ine = _sub(link, "inertial")
            self._emit_pose(ine, ip.inertia_origin)
            _leaf(ine, "mass", ip.mass)
            if ip.inertia is not None:
                vals = ip.inertia.split()
                if len(vals) == 6:
                    inr = _sub(ine, "inertia")
                    for k, v in zip(("ixx", "ixy", "ixz", "iyy", "iyz", "izz"), vals):
                        _leaf(inr, k, v)
                else:
                    self.loss.add("inertial", f"comp {comp.name!r}: inertia {ip.inertia!r} is not 6 values; dropped")
        for v in (comp.visual or []):
            vis = etree.Element("visual")
            vis.set("name", v.name or f"{comp.name}_visual")
            self._emit_pose(vis, v.pose)
            if v.model is not None:
                mesh = _sub(_sub(vis, "geometry"), "mesh")
                _leaf(mesh, "uri", v.model.uri)
                self.loss.add("visual", f"visual {v.name!r}: GLB <model> -> SDF mesh ref; baked PBR/textures "
                                        f"are not represented in SDF <material>")
            elif v.geometry is not None:
                if not self._geometry(vis, v.geometry, allow_mesh=False, ctx=f"visual {v.name!r}"):
                    continue
                if v.color is not None:
                    self._color(vis, v.color)
            else:
                self.loss.add("visual", f"visual {v.name!r}: no geometry; dropped")
                continue
            link.append(vis)
        for c in (comp.collision or []):
            col = etree.Element("collision")
            col.set("name", c.name or f"{comp.name}_collision")
            self._emit_pose(col, c.pose)
            if c.geometry is None or not self._geometry(col, c.geometry, allow_mesh=True, ctx=f"collision {c.name!r}"):
                self.loss.add("collision", f"collision {c.name!r}: no representable geometry; dropped")
                continue
            if c.surface is not None:
                self._surface(col, c.surface)
            link.append(col)
        # HCDF-rich comp content with no SDF home
        for attr, label in (("sensor", "sensors"), ("motor", "motors"), ("frame", "frames"),
                            ("hmi", "HMI elements"), ("dynamic_surface", "dynamic surfaces"),
                            ("power_source", "power sources"), ("port", "ports"), ("antenna", "antennas"),
                            ("board", "boards"), ("operating_temp", "operating-temp blocks"),
                            ("switch", "switches"), ("software", "software blocks"),
                            ("discovered", "discovered-device blocks"), ("extension", "vendor extensions")):
            v = getattr(comp, attr, None)
            if v:
                n = len(v) if isinstance(v, list) else 1
                self.loss.add("comp", f"comp {comp.name!r}: {n} {label} dropped (no SDF home)")
        for attr, label in (("struct_type", "struct-type"), ("ip_rating", "ip-rating"),
                            ("role", "role"), ("hwid", "hwid")):
            if getattr(comp, attr, None) is not None:
                self.loss.add("comp", f"comp {comp.name!r}: {label} dropped (no SDF home)")
        if comp.description is not None:
            self.loss.add("annotation", f"comp {comp.name!r}: <description> dropped (no SDF field)")
        if comp.urdf_compat is not None:
            self.loss.add("comp", f"comp {comp.name!r}: <urdf-compat> dropped (no SDF home)")

    def _joint(self, model, j):
        jtype = _val(j.type)
        # HCDF-only joint metadata with no SDF home — recorded for ALL joint types (incl. the 'fixed'
        # early-return path below), so nothing is silently dropped.
        if j.description is not None:
            self.loss.add("annotation", f"joint {j.name!r}: <description> dropped (no SDF field)")
        for attr, label in (("calibration", "<calibration>"), ("extension", "<extension>"),
                            ("urdf_compat", "<urdf-compat>")):
            if getattr(j, attr, None) is not None:
                self.loss.add("joint", f"joint {j.name!r}: {label} dropped (no SDF home)")
        sdf_type = _JTYPE_OUT.get(jtype)
        if sdf_type is None:
            sdf_type = "fixed"
            self.loss.add("joint-type", f"joint {j.name!r}: type {jtype!r} has no SDF equivalent; "
                                        f"exported as 'fixed'")
            for attr, what in (("axis", "<axis>"), ("limit", "<limit>"), ("thread_pitch", "thread_pitch")):
                if getattr(j, attr, None) is not None:
                    self.loss.add("joint", f"joint {j.name!r}: {what} dropped with the {jtype}->fixed downgrade")
        jname = j.name
        if jname is None:
            jname = "unnamed_joint"
            self.loss.add("joint", "a joint has no name; emitted SDF <joint> name synthesized "
                                   f"({jname!r}) to keep the SDF valid")
        jt = _sub(model, "joint", name=jname, type=sdf_type)
        if j.parent is not None:
            _leaf(jt, "parent", j.parent.comp)
        if j.child is not None:
            _leaf(jt, "child", j.child.comp)
        # The joint pose is the parent->child offset, relative to the parent frame. A joint anchored
        # to 'world' is offset relative to the model frame (SDF reserves 'world' as an implicit frame).
        pframe = j.parent.comp if j.parent is not None else None
        if pframe == "world":
            pframe = "__model__"
        if pframe is not None:
            self._emit_pose(jt, j.origin, relative_to=pframe, force=True)
        else:
            self._emit_pose(jt, j.origin)
        # NB: a <loop> closure joint is emitted as an ordinary SDF joint — SDF natively expresses
        # closed kinematic loops (URDF cannot), so no loss is recorded for it here.
        if sdf_type == "fixed":
            return
        axis = None
        if j.axis is not None:
            axis = _sub(jt, "axis")
            _leaf(axis, "xyz", self._conv_axis(j.axis.xyz))
        if j.limit is not None:
            ax = axis if axis is not None else _sub(jt, "axis")
            lim = _sub(ax, "limit")
            _leaf(lim, "lower", j.limit.lower); _leaf(lim, "upper", j.limit.upper)
            _leaf(lim, "effort", j.limit.effort); _leaf(lim, "velocity", j.limit.velocity)
            for extra in ("acceleration", "jerk", "deceleration"):
                if getattr(j.limit, extra, None) is not None:
                    self.loss.add("joint", f"joint {j.name!r}: limit/{extra} dropped (no SDF field)")
            axis = ax
        if j.dynamics is not None:
            ax = axis if axis is not None else _sub(jt, "axis")
            dyn = _sub(ax, "dynamics")
            _leaf(dyn, "damping", j.dynamics.damping); _leaf(dyn, "friction", j.dynamics.friction)
            _leaf(dyn, "spring_stiffness", j.dynamics.spring_stiffness)
            _leaf(dyn, "spring_reference", j.dynamics.spring_reference)
        if j.axis2 is not None:
            a2 = _sub(jt, "axis2"); _leaf(a2, "xyz", self._conv_axis(j.axis2.xyz))
        if j.thread_pitch is not None and sdf_type == "screw":
            _leaf(jt, "thread_pitch", j.thread_pitch)
        if j.mimic is not None:
            # SDF 1.12 places <mimic> under <axis> (not <joint>) and requires a <reference> child;
            # HCDF Mimic has no reference field, so default it to 0.
            ax = axis if axis is not None else _sub(jt, "axis")
            m = _sub(ax, "mimic", joint=j.mimic.joint)
            _leaf(m, "reference", "0")
            _leaf(m, "multiplier", j.mimic.multiplier); _leaf(m, "offset", j.mimic.offset)

    def build(self):
        sdf = etree.Element("sdf")
        sdf.set("version", SDF_VERSION)
        model = _sub(sdf, "model", name=self.doc.name or "model")
        # which joint places each comp (tree joints only; loop closures do not position links)
        self._child_joint = {j.child.comp: j.name for j in (self.doc.joint or [])
                             if j.loop is None and j.child is not None and j.child.comp and j.name}
        if self.world == WorldFrame.NED:
            self.loss.add("world-frame", "document is world-frame NED; world-relative placements are NOT "
                                         "converted to SDF's ENU (body-frame poses ARE).")
        # document-level metadata SDF <model> has no field for (benign, non-model)
        for attr, label in (("description", "<description>"), ("author", "author"),
                            ("license", "license"), ("url", "url"), ("version", "version")):
            val = getattr(self.doc, attr, None)
            if val is not None:
                self.loss.add("annotation", f"document {label} {val!r} dropped (no SDF field)")
        if not self.doc.comp:
            self.loss.add("model", "document has no comps; an SDF <model> requires >=1 <link> "
                                   "(the emitted SDF will be rejected by libsdformat)")
        for comp in (self.doc.comp or []):
            self._link(model, comp)
        for j in (self.doc.joint or []):
            self._joint(model, j)
        # HCDF-only top-level content with no SDF-model home
        for attr, label in (("group", "joint groups"), ("state", "kinematic states"),
                            ("network", "networks"), ("transmission", "HCDF transmissions"),
                            ("self_collision_disable", "self-collision-disable blocks"),
                            ("include", "includes"), ("extension", "extensions")):
            v = getattr(self.doc, attr, None)
            if v:
                n = len(v) if isinstance(v, list) else 1
                self.loss.add("top-level", f"document: {n} {label} dropped (no SDF-model home)")
        if self.doc.color:
            self.loss.add("top-level", f"document: {len(self.doc.color)} top-level <color>(s) — SDF has no "
                                       f"color palette; referenced colors are inlined per-visual")
        return sdf


def to_sdf(doc):
    """Export an hcdfdom Hcdf model to SDFormat. Returns ``(sdf_xml_string, LossManifest)``."""
    ex = _Exporter(doc)
    sdf = ex.build()
    xml = etree.tostring(sdf, pretty_print=True, xml_declaration=False, encoding="unicode")
    return xml, ex.loss
