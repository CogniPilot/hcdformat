"""Companion validator for the hcdfdom typed DOM.

The XSD already enforces structure and the six root keyrefs (joint parent/child comp,
group joint/group, state joint, mimic joint). This layer adds the rules XSD 1.0 cannot
express:

  * **Kinematic tree integrity** — acyclic; every comp is the child of at most one joint;
    a single kinematic root is expected. Loop-closure joints (those carrying ``<loop>``)
    are constraints, not tree edges, so they are excluded from the tree.
  * **frame/@relative-to** resolves against the UNION of sibling-frame ∪ comp ∪ joint
    names (a heterogeneous namespace no single xs:keyref can target).
  * **Joint-type semantics** — axis / limit / axis2 / thread_pitch presence per type, and
    upper ≥ lower (the rules documented on the ``joint`` complexType).
  * **Visual <color> references** — a name-only ``<color name="x"/>`` (no @rgba) must
    resolve to a document-level ``<color name="x">``.
  * **At most one** ``<state default="true">``.
  * **group @tip-comp / @tip-frame** resolve (tip-frame must be a frame on tip-comp).
  * **@uri media rules** (warnings) — a visual ``<model>`` is GLB/glTF; a collision mesh
    is lean (STL/OBJ/convex), never a GLB.

Usage:
    import hcdfdom
    from hcdfdom.validate import validate, is_valid
    issues = validate(hcdfdom.load("robot.hcdf"))

    $ python3 -m hcdfdom.validate robot.hcdf
"""
from __future__ import annotations

import sys
from dataclasses import dataclass

ERROR = "error"
WARNING = "warning"

_GLB_EXT = (".glb", ".gltf")
_LEAN_EXT = (".stl", ".obj", ".dae", ".ply")


@dataclass
class Issue:
    level: str
    code: str
    message: str

    def __str__(self):
        return f"[{self.level}] {self.code}: {self.message}"


def _val(enum_or_none):
    """The .value of a str-Enum, or None."""
    return enum_or_none.value if enum_or_none is not None else None


def _as_float(s):
    try:
        return float(s)
    except (TypeError, ValueError):
        return None


def validate(doc) -> list[Issue]:
    """Return all semantic issues (errors + warnings) for a parsed Hcdf document."""
    issues: list[Issue] = []
    comps = list(doc.comp or [])
    joints = list(doc.joint or [])
    comp_names = {c.name for c in comps if c.name}
    joint_names = {j.name for j in joints if j.name}
    color_names = {c.name for c in (doc.color or []) if c.name}
    frames_by_comp = {c.name: {f.name for f in (c.frame or []) if f.name} for c in comps}

    _check_tree(joints, issues)
    _check_loops(joints, comp_names, issues)
    _check_joint_semantics(joints, issues)
    _check_frames(comps, comp_names, joint_names, frames_by_comp, issues)
    _check_colors(comps, color_names, issues)
    _check_states(doc, issues)
    _check_groups(doc, comp_names, frames_by_comp, issues)
    _check_media(comps, issues)
    return issues


def is_valid(doc) -> bool:
    """True if the document has no error-level issues (warnings are allowed)."""
    return not any(i.level == ERROR for i in validate(doc))


# ── kinematic tree ──────────────────────────────────────────────────────────
def _check_tree(joints, issues):
    tree = [j for j in joints if j.loop is None]
    edges = []
    parents_of = {}  # child comp -> [joint names]
    for j in tree:
        p = getattr(j.parent, "comp", None) if j.parent is not None else None
        c = getattr(j.child, "comp", None) if j.child is not None else None
        if p is None or c is None:
            continue  # XSD guarantees presence; defensive
        edges.append((p, c, j.name))
        parents_of.setdefault(c, []).append(j.name)
        if p == c:
            issues.append(Issue(ERROR, "E_SELF_JOINT",
                                 f"joint {j.name!r} has the same parent and child comp {p!r}"))

    for child, js in parents_of.items():
        if len(js) > 1:
            issues.append(Issue(ERROR, "E_MULTI_PARENT",
                                 f"comp {child!r} is the child of {len(js)} joints "
                                 f"({', '.join(sorted(js))}); a kinematic tree allows one"))

    # cycle detection over parent -> children
    adj = {}
    for p, c, _ in edges:
        adj.setdefault(p, []).append(c)
    WHITE, GREY, BLACK = 0, 1, 2
    color = {}
    cyc = []

    def dfs(n, stack):
        color[n] = GREY
        for m in adj.get(n, ()):
            if color.get(m, WHITE) == GREY:
                cyc.append(" -> ".join(stack + [n, m]))
            elif color.get(m, WHITE) == WHITE:
                dfs(m, stack + [n])
        color[n] = BLACK

    nodes = set(adj) | {c for _, c, _ in edges}
    for n in nodes:
        if color.get(n, WHITE) == WHITE:
            dfs(n, [])
    for path in cyc:
        issues.append(Issue(ERROR, "E_CYCLE", f"kinematic cycle through comps: {path}"))

    if edges and not cyc:
        children = {c for _, c, _ in edges}
        jointed = {p for p, _, _ in edges} | children
        roots = sorted(jointed - children)
        if len(roots) > 1:
            issues.append(Issue(WARNING, "W_MULTI_ROOT",
                                 f"{len(roots)} kinematic roots ({', '.join(roots)}); "
                                 f"a single robot model usually has one"))


# ── loop-closure references ───────────────────────────────────────────────────
def _check_loops(joints, comp_names, issues):
    """A <loop> joint may name predecessor/successor comps; if set they must exist.

    Loop joints close kinematic loops (four-bars, delta/Stewart platforms) that URDF's
    tree-only model cannot express; _check_tree already excludes them from tree edges, so
    a marked loop is *allowed* — this only validates its body references.
    """
    for j in joints:
        lp = j.loop
        if lp is None:
            continue
        for role, name in (("predecessor", lp.predecessor), ("successor", lp.successor)):
            if name is not None and name not in comp_names:
                issues.append(Issue(ERROR, "E_LOOP_REF",
                                    f"joint {j.name!r}: loop {role} {name!r} is not a comp"))


# ── joint-type semantics ──────────────────────────────────────────────────────
def _check_joint_semantics(joints, issues):
    for j in joints:
        t = _val(j.type)
        has_axis = j.axis is not None
        has_axis2 = j.axis2 is not None
        lim = j.limit
        has_bounds = lim is not None and (lim.lower is not None or lim.upper is not None)

        def err(code, msg):
            issues.append(Issue(ERROR, code, f"joint {j.name!r} ({t}): {msg}"))

        if t in ("revolute", "prismatic", "cylindrical"):
            if not has_axis:
                err("E_JOINT_AXIS_REQUIRED", "requires an <axis>")
            if not has_bounds:
                err("E_JOINT_LIMIT_REQUIRED", "requires a <limit> with lower and upper")
        elif t == "continuous":
            if not has_axis:
                err("E_JOINT_AXIS_REQUIRED", "requires an <axis>")
            if has_bounds:
                err("E_JOINT_CONTINUOUS_BOUNDS",
                    "is continuous and must omit limit lower/upper (unbounded rotation)")
        elif t == "universal":
            if not has_axis or not has_axis2:
                err("E_JOINT_AXIS2_REQUIRED", "requires both <axis> and <axis2>")
        elif t == "screw":
            if not has_axis:
                err("E_JOINT_AXIS_REQUIRED", "requires an <axis>")
            if j.thread_pitch is None:
                err("E_JOINT_THREADPITCH_REQUIRED", "requires @thread_pitch")
        elif t in ("fixed", "free"):
            if has_axis:
                err("E_JOINT_AXIS_FORBIDDEN", "must not have an <axis>")
            if lim is not None:
                err("E_JOINT_LIMIT_FORBIDDEN", "must not have a <limit>")

        if lim is not None and lim.lower is not None and lim.upper is not None:
            lo, hi = _as_float(lim.lower), _as_float(lim.upper)
            if lo is not None and hi is not None and hi < lo:
                err("E_LIMIT_RANGE", f"limit upper ({hi}) < lower ({lo})")


# ── frame relative-to (union of sibling-frame / comp / joint) ─────────────────
def _check_frames(comps, comp_names, joint_names, frames_by_comp, issues):
    for c in comps:
        siblings = frames_by_comp.get(c.name, set())
        for f in (c.frame or []):
            rel = f.relative_to
            if rel is None:
                continue
            if rel in siblings or rel in comp_names or rel in joint_names:
                continue
            issues.append(Issue(ERROR, "E_FRAME_RELATIVE_TO",
                                 f"frame {f.name!r} on comp {c.name!r}: relative-to "
                                 f"{rel!r} matches no sibling frame, comp, or joint"))


# ── visual <color> references ─────────────────────────────────────────────────
def _check_colors(comps, color_names, issues):
    for c in comps:
        for v in (c.visual or []):
            col = v.color
            # a pure reference = @name set, @rgba absent; inline = @rgba present
            if col is not None and col.name is not None and col.rgba is None:
                if col.name not in color_names:
                    issues.append(Issue(ERROR, "E_COLOR_REF",
                                        f"visual {v.name!r} on comp {c.name!r} references "
                                        f"color {col.name!r}, which is not a document-level <color>"))


# ── at most one default state ─────────────────────────────────────────────────
def _check_states(doc, issues):
    defaults = [s.name for s in (doc.state or []) if str(s.default).lower() == "true"]
    if len(defaults) > 1:
        issues.append(Issue(ERROR, "E_MULTI_DEFAULT_STATE",
                             f"{len(defaults)} states marked default=true "
                             f"({', '.join(defaults)}); at most one allowed"))


# ── group tip-comp / tip-frame ────────────────────────────────────────────────
def _check_groups(doc, comp_names, frames_by_comp, issues):
    for g in (doc.group or []):
        if g.tip_comp is not None and g.tip_comp not in comp_names:
            issues.append(Issue(ERROR, "E_GROUP_TIP_COMP",
                                 f"group {g.name!r}: tip-comp {g.tip_comp!r} is not a comp"))
        if g.tip_frame is not None:
            if g.tip_comp is None:
                issues.append(Issue(ERROR, "E_GROUP_TIP_FRAME",
                                     f"group {g.name!r}: tip-frame {g.tip_frame!r} set without a tip-comp"))
            elif g.tip_frame not in frames_by_comp.get(g.tip_comp, set()):
                issues.append(Issue(ERROR, "E_GROUP_TIP_FRAME",
                                     f"group {g.name!r}: tip-frame {g.tip_frame!r} is not a frame "
                                     f"on tip-comp {g.tip_comp!r}"))


# ── @uri media rules (warnings) ───────────────────────────────────────────────
def _check_media(comps, issues):
    for c in comps:
        for v in (c.visual or []):
            uri = getattr(v.model, "uri", None) if v.model is not None else None
            if uri and not uri.lower().endswith(_GLB_EXT):
                issues.append(Issue(WARNING, "W_VISUAL_URI",
                                    f"visual {v.name!r} on comp {c.name!r}: <model> uri {uri!r} "
                                    f"is not GLB/glTF (rich visual appearance should be baked GLB)"))
        for col in (c.collision or []):
            mesh = getattr(col.geometry, "mesh", None) if col.geometry is not None else None
            uri = getattr(mesh, "uri", None) if mesh is not None else None
            if uri and uri.lower().endswith(_GLB_EXT):
                issues.append(Issue(WARNING, "W_COLLISION_URI",
                                    f"collision {col.name!r} on comp {c.name!r}: mesh uri {uri!r} "
                                    f"is a GLB; collision meshes should be lean (STL/OBJ/convex)"))


def main(argv=None):
    import hcdfdom
    argv = argv if argv is not None else sys.argv[1:]
    if not argv:
        print("Usage: python3 -m hcdfdom.validate <file.hcdf> [...]")
        return 2
    rc = 0
    for path in argv:
        issues = validate(hcdfdom.load(path))
        errs = [i for i in issues if i.level == ERROR]
        warns = [i for i in issues if i.level == WARNING]
        if not issues:
            print(f"{path}: OK")
            continue
        print(f"{path}: {len(errs)} error(s), {len(warns)} warning(s)")
        for i in issues:
            print(f"  {i}")
        if errs:
            rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
