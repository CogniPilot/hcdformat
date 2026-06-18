"""HCDF -> URDF export.

The reverse of ``from_urdf``: walk the typed hcdfdom DOM and emit URDF at the lxml string
layer (never through urdfdom's lossy exporter). HCDF is a superset of URDF, so export is
inherently lossy; every dropped or approximated construct is recorded in a structured
LossManifest.

Frame handling: URDF is always FLU/ENU. If the HCDF document declares body-frame="FRD", every
body-frame quantity is converted to FLU on the way out: pose translations/rotations (t'=C·t,
R'=C·R·Cᵀ) AND joint axis direction vectors (a'=C·a). World-frame="NED" is recorded as a loss
(world-relative pose conversion is not applied here). URDF ``<origin>`` has no
quaternion, so any pose using ``quat`` is emitted as ``rpy`` (the rotation is preserved exactly).

Reversals of the import quarantine: a quarantined ``<extension domain="org.gazebosim">`` (etc.)
is de-merged back to top-level ``<gazebo>`` / ``<transmission>`` / ``<ros2_control>`` / vendor
elements, so URDF->HCDF->URDF reproduces them exactly.
"""
from __future__ import annotations

import copy
from dataclasses import dataclass, field

from lxml import etree

from hcdfdom import frames
from hcdfdom.model import BodyFrame, WorldFrame

# URDF-expressible joint types (HCDF -> URDF). HCDF-only types are reported as losses.
_JTYPE_OUT = {"revolute": "revolute", "continuous": "continuous", "prismatic": "prismatic",
              "fixed": "fixed", "planar": "planar", "free": "floating"}
_URDF_PRIMS = {"box", "cylinder", "sphere"}        # URDF geometry primitives (HCDF adds capsule/cone/ellipsoid)


@dataclass
class LossManifest:
    """Structured record of what HCDF content could not be represented in URDF."""
    items: list = field(default_factory=list)

    def add(self, category, detail):
        self.items.append((category, detail))

    def __bool__(self):
        return bool(self.items)

    def __len__(self):
        return len(self.items)

    def text(self):
        return "\n".join(f"[{c}] {d}" for c, d in self.items)

    def categories(self):
        """Return an order-preserving {category: [detail, ...]} grouping of the items."""
        cats = {}
        for c, d in self.items:
            cats.setdefault(c, []).append(d)
        return cats

    def to_dict(self):
        """Structured form: total count, per-category grouping, and the flat item list."""
        return {
            "total": len(self.items),
            "categories": self.categories(),
            "items": [{"category": c, "detail": d} for c, d in self.items],
        }

    def to_json(self, indent=2):
        import json
        return json.dumps(self.to_dict(), indent=indent)

    def markdown(self, title="HCDF → URDF loss manifest"):
        """A human-readable report grouped by category (stable category ordering)."""
        lines = [f"# {title}", ""]
        if not self.items:
            lines.append("_No losses — the document exports to URDF without dropping content._")
            return "\n".join(lines) + "\n"
        cats = self.categories()
        lines.append(f"**{len(self.items)} loss item(s)** across {len(cats)} categor"
                     f"{'y' if len(cats) == 1 else 'ies'}.")
        lines.append("")
        for c in sorted(cats):
            lines.append(f"## {c} ({len(cats[c])})")
            lines.extend(f"- {d}" for d in cats[c])
            lines.append("")
        return "\n".join(lines).rstrip() + "\n"


def _sub(parent, tag, **attrs):
    e = etree.SubElement(parent, tag)
    for k, v in attrs.items():
        if v is not None:
            e.set(k, v)
    return e


def _val(enum_or_str):
    return enum_or_str.value if hasattr(enum_or_str, "value") else enum_or_str


class _Exporter:
    def __init__(self, doc):
        self.doc = doc
        self.loss = LossManifest()
        self.body = doc.body_frame
        self.world = doc.world_frame
        self._C = (frames.change_of_basis(self.body, BodyFrame.FLU)
                   if self.body == BodyFrame.FRD else None)

    def _conv_axis(self, xyz):
        """Apply the body-frame change-of-basis to a joint axis direction vector."""
        if self._C is None or xyz is None:
            return xyz
        return frames._fmt(self._C @ frames._floats(xyz))

    def _drop(self, category, ctx, attr, label, obj):
        """Record an HCDF construct that has no URDF home (handles list-or-single fields)."""
        v = getattr(obj, attr, None)
        if not v:
            return
        n = len(v) if isinstance(v, list) else 1
        self.loss.add(category, f"{ctx}: {n} {label} dropped (no URDF equivalent)")

    # ── poses ────────────────────────────────────────────────────────────────
    def _origin(self, parent, pose):
        """Emit a URDF <origin xyz rpy> from an HCDF pose (converting frame/quat as needed)."""
        if pose is None:
            return
        if self._C is not None or pose.quat:
            # quat->rpy and FRD->FLU are exact representation conversions (the rotation is preserved),
            # so neither is a content loss — the profile checker surfaces them as with-transform findings.
            out = frames.transform_pose(pose, self._C if self._C is not None else frames._I3, use_quat=False)
            xyz, rpy = out.xyz, out.rpy
        else:
            xyz, rpy = pose.xyz, pose.rpy
        if xyz is None and rpy is None:
            return
        _sub(parent, "origin", xyz=xyz, rpy=rpy)

    # ── geometry ───────────────────────────────────────────────────────────────
    def _geometry(self, parent, geo, *, allow_mesh, ctx):
        """Emit <geometry> for a (visual|collision)_geometry. Returns False if unrepresentable."""
        g = etree.Element("geometry")
        if geo.box is not None:
            _sub(g, "box", size=geo.box.size)
        elif geo.cylinder is not None:
            _sub(g, "cylinder", radius=geo.cylinder.radius, length=geo.cylinder.length)
        elif geo.sphere is not None:
            _sub(g, "sphere", radius=geo.sphere.radius)
        elif allow_mesh and getattr(geo, "mesh", None) is not None:
            m = _sub(g, "mesh", filename=geo.mesh.uri)
            if geo.mesh.scale and geo.mesh.scale != "1 1 1":
                m.set("scale", geo.mesh.scale)
        else:
            bad = next((p for p in ("capsule", "cone", "ellipsoid") if getattr(geo, p, None) is not None), "empty")
            self.loss.add("geometry", f"{ctx}: <{bad}> has no URDF primitive; {ctx} dropped")
            return False
        parent.append(g)
        return True

    def _material(self, parent, color):
        m = etree.Element("material")
        if color.name is not None:
            m.set("name", color.name)
        if color.rgba is not None:
            _sub(m, "color", rgba=color.rgba)
        if color.description is not None:
            self.loss.add("annotation", f"color {color.name!r}: description dropped (no URDF field)")
        # URDF material needs a name or an inline color; emit only if it has one
        if m.get("name") is not None or len(m):
            parent.append(m)

    # ── link ──────────────────────────────────────────────────────────────────
    def _link(self, robot, comp):
        link = _sub(robot, "link", name=comp.name)
        # URDF-compat: the PR2-era link/@type round-trips via the comp urdf-compat annex
        if comp.urdf_compat is not None and comp.urdf_compat.link_type is not None:
            link.set("type", comp.urdf_compat.link_type)
        if comp.inertial is not None:
            self._inertial(link, comp.inertial)
        for v in (comp.visual or []):
            self._visual(link, v, comp.name)
        for c in (comp.collision or []):
            self._collision(link, c, comp.name)
        # HCDF-rich constructs with no URDF home -> recorded in the manifest, never silently dropped
        ctx = f"comp {comp.name!r}"
        for attr, label in (("frame", "frames"), ("sensor", "sensors"), ("motor", "motors"),
                            ("hmi", "HMI elements"), ("dynamic_surface", "dynamic surfaces"),
                            ("power_source", "power sources"), ("port", "ports"), ("antenna", "antennas"),
                            ("board", "boards"), ("operating_temp", "operating-temp blocks"),
                            ("switch", "switches"), ("software", "software blocks"),
                            ("discovered", "discovered-device blocks"), ("extension", "vendor extensions")):
            self._drop("comp", ctx, attr, label, comp)
        # HCDF-only comp metadata scalars with no URDF home (were silently dropped)
        for attr, label in (("struct_type", "struct-type"), ("ip_rating", "ip-rating"),
                            ("role", "role"), ("hwid", "hwid")):
            val = getattr(comp, attr, None)
            if val is not None:
                self.loss.add("comp", f"{ctx}: {label}={val!r} dropped (HCDF-only; no URDF home)")
        if comp.description is not None:
            self.loss.add("annotation", f"{ctx}: <description> dropped (no URDF field)")

    def _inertial(self, link, ip):
        ine = _sub(link, "inertial")
        if ip.mass is not None:
            _sub(ine, "mass", value=ip.mass)
        self._origin(ine, ip.inertia_origin)
        if ip.inertia is not None:
            vals = ip.inertia.split()
            if len(vals) == 6:
                keys = ("ixx", "ixy", "ixz", "iyy", "iyz", "izz")
                _sub(ine, "inertia", **dict(zip(keys, vals)))
            else:
                self.loss.add("inertial", f"inertia tuple {ip.inertia!r} is not 6 values; dropped")

    def _visual(self, link, v, comp_name):
        vis = etree.Element("visual")
        if v.name is not None and _val(v.name_origin) != "synthesized":
            vis.set("name", v.name)
        self._origin(vis, v.pose)
        if v.model is not None:
            # ARM A: a baked GLB -> URDF mesh visual (appearance is in the asset, no material)
            g = _sub(vis, "geometry")
            _sub(g, "mesh", filename=v.model.uri)
            if v.color is not None:
                self.loss.add("visual", f"visual {v.name!r}: <color> alongside a GLB <model> dropped "
                                        f"(appearance is baked into the model)")
        elif v.geometry is not None:
            if not self._geometry(vis, v.geometry, allow_mesh=False, ctx=f"visual {v.name!r}"):
                return
            if v.color is not None:
                self._material(vis, v.color)
        else:
            self.loss.add("visual", f"visual {v.name!r}: no geometry; dropped")
            return
        if v.toggle is not None:
            self.loss.add("visual", f"visual {v.name!r}: toggle group {v.toggle!r} dropped "
                                    f"(URDF has no runtime show/hide grouping)")
        link.append(vis)

    def _collision(self, link, c, comp_name):
        col = etree.Element("collision")
        if c.name is not None and _val(c.name_origin) != "synthesized":
            col.set("name", c.name)
        self._origin(col, c.pose)
        if c.geometry is None or not self._geometry(col, c.geometry, allow_mesh=True, ctx=f"collision {c.name!r}"):
            self.loss.add("collision", f"collision {c.name!r}: no representable geometry; dropped")
            return
        if c.surface is not None:
            self.loss.add("collision", f"collision {c.name!r}: <surface> contact physics dropped "
                                       f"(URDF core has none; lives in <gazebo> at sim time)")
        if c.verbose is not None:
            # urdf-compat per-collision verbose flag; standard urdf.xsd has no such attribute and it
            # carries zero CPS meaning, so it is a benign (non-model) drop.
            self.loss.add("annotation", f"collision {c.name!r}: @verbose {c.verbose!r} dropped "
                                        f"(no standard URDF field; zero CPS meaning)")
        link.append(col)

    # ── joint ────────────────────────────────────────────────────────────────
    def _joint(self, robot, j):
        jtype = _val(j.type)
        if j.loop is not None:
            self.loss.add("loop", f"joint {j.name!r}: loop-closure dropped — URDF is tree-only and cannot "
                                  f"express the closed kinematic loop (a core HCDF capability)")
            return
        downgraded = jtype not in _JTYPE_OUT
        urdf_type = _JTYPE_OUT.get(jtype, "fixed")
        if downgraded:
            self.loss.add("joint-type", f"joint {j.name!r}: type {jtype!r} has no URDF equivalent; "
                                        f"exported as 'fixed' to keep the tree connected")
            # itemize the kinematic data lost in the downgrade to fixed
            if j.axis is not None:
                self.loss.add("joint", f"joint {j.name!r}: <axis xyz={j.axis.xyz!r}> dropped with the "
                                       f"{jtype}->fixed downgrade")
            if j.limit is not None:
                self.loss.add("joint", f"joint {j.name!r}: <limit> dropped with the {jtype}->fixed downgrade")
        # thread_pitch never has a URDF home (no screw joint), regardless of the exported type
        if j.thread_pitch is not None:
            self.loss.add("joint", f"joint {j.name!r}: thread_pitch {j.thread_pitch!r} dropped "
                                   f"(URDF has no screw joint)")
        jt = _sub(robot, "joint", name=j.name, type=urdf_type)
        if j.parent is not None:
            _sub(jt, "parent", link=j.parent.comp)
        if j.child is not None:
            _sub(jt, "child", link=j.child.comp)
        self._origin(jt, j.origin)
        if j.axis is not None and urdf_type not in ("fixed", "floating"):
            _sub(jt, "axis", xyz=self._conv_axis(j.axis.xyz))   # FRD->FLU converts the axis vector too
        if j.axis2 is not None:
            self.loss.add("joint", f"joint {j.name!r}: <axis2> dropped (URDF has no second axis)")
        if j.limit is not None and urdf_type not in ("fixed", "floating"):
            lim = _sub(jt, "limit", lower=j.limit.lower, upper=j.limit.upper,
                       effort=j.limit.effort, velocity=j.limit.velocity)
            if lim.get("effort") is None:
                lim.set("effort", "0")
            if lim.get("velocity") is None:
                lim.set("velocity", "0")
            for extra in ("acceleration", "jerk", "deceleration"):
                if getattr(j.limit, extra, None) is not None:
                    self.loss.add("joint", f"joint {j.name!r}: limit/{extra} dropped (no URDF field)")
        if j.dynamics is not None:
            _sub(jt, "dynamics", damping=j.dynamics.damping, friction=j.dynamics.friction)
            for sp in ("spring_stiffness", "spring_reference"):
                if getattr(j.dynamics, sp, None) is not None:
                    self.loss.add("joint", f"joint {j.name!r}: dynamics/{sp} dropped (no URDF field)")
        if j.mimic is not None:
            _sub(jt, "mimic", joint=j.mimic.joint, multiplier=j.mimic.multiplier, offset=j.mimic.offset)
        if j.calibration is not None:
            _sub(jt, "calibration", reference_position=j.calibration.reference_position,
                 rising=j.calibration.rising, falling=j.calibration.falling)
        if j.urdf_compat is not None and j.urdf_compat.safety_controller is not None:
            s = j.urdf_compat.safety_controller
            _sub(jt, "safety_controller", soft_lower_limit=s.soft_lower_limit,
                 soft_upper_limit=s.soft_upper_limit, k_position=s.k_position, k_velocity=s.k_velocity)
        if j.extension is not None:
            self.loss.add("joint", f"joint {j.name!r}: <extension> vendor data dropped (no URDF home)")
        if j.description is not None:
            self.loss.add("annotation", f"joint {j.name!r}: <description> dropped (no URDF field)")

    # ── top-level ──────────────────────────────────────────────────────────────
    def build(self):
        robot = etree.Element("robot")
        robot.set("name", self.doc.name or "robot")
        # document-level metadata URDF <robot> has no field for (benign, non-model)
        for attr, label in (("description", "<description>"), ("author", "author"),
                            ("license", "license"), ("url", "url"), ("version", "version")):
            val = getattr(self.doc, attr, None)
            if val is not None:
                self.loss.add("annotation", f"document {label} {val!r} dropped (no URDF field)")
        if self.world == WorldFrame.NED:
            self.loss.add("world-frame", "document is world-frame NED; world-relative placements are NOT "
                                         "converted to URDF's ENU (body-frame poses ARE). Re-root world poses "
                                         "before consuming the URDF.")
        # top-level colors -> URDF materials
        for col in (self.doc.color or []):
            self._material(robot, col)
        for comp in (self.doc.comp or []):
            self._link(robot, comp)
        for j in (self.doc.joint or []):
            self._joint(robot, j)
        # de-merge quarantined extensions back to their native top-level elements
        for ext in (self.doc.extension or []):
            for raw in (ext.any_content or []):
                robot.append(copy.deepcopy(raw))
        # HCDF-only top-level constructs -> recorded, never silently dropped
        for attr, label in (("group", "joint groups"), ("state", "kinematic states"),
                            ("network", "networks"), ("transmission", "HCDF transmissions"),
                            ("include", "includes"), ("self_collision_disable", "self-collision-disable blocks")):
            self._drop("top-level", "document", attr, label, self.doc)
        return robot


def to_urdf(doc):
    """Export an hcdfdom Hcdf model to URDF. Returns ``(urdf_xml_string, LossManifest)``."""
    ex = _Exporter(doc)
    robot = ex.build()
    # De-merged <extension> content keeps its original whitespace, which would suppress lxml's
    # pretty printing for the whole document. Re-parse with blank text removed so the output is
    # uniformly indented regardless of any restored gazebo/ros2_control blocks.
    parser = etree.XMLParser(remove_blank_text=True)
    robot = etree.fromstring(etree.tostring(robot), parser)
    xml = etree.tostring(robot, pretty_print=True, xml_declaration=True, encoding="UTF-8").decode("utf-8")
    return xml, ex.loss
