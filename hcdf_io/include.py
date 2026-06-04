"""<include> flattening for HCDF model composition.

An ``<include uri name pose>`` imports a reusable sub-assembly. Flattening produces a single
self-contained document:
  * ``@name`` is a prefix (``prefix/...``) applied to every definition (comp/joint/group/state/
    color/transmission) and rewritten through every reference, so a module can be included many
    times without name collisions.
  * ``@pose`` ("x y z r p y", relative to the including document's world frame) rigidly places
    the sub-assembly: every root comp's own geometry/frames and the origins of joints leaving a
    root are pre-multiplied by the offset (the internal tree is relative, so this moves the whole
    assembly).
Nested includes resolve recursively (relative to each file's own directory); include cycles raise.
"""
from __future__ import annotations

import os

import hcdfdom
from hcdfdom import frames
from hcdfdom import model as M

_SEP = "/"


def flatten(src, base_dir=None):
    """Resolve all ``<include>`` elements into a single document.

    ``src`` is a path or an ``Hcdf``. Returns ``(doc, notes)``; ``doc`` is the same object
    when an Hcdf is passed (mutated in place) or a freshly loaded one for a path.
    """
    notes: list[str] = []
    if isinstance(src, M.Hcdf):
        doc, base, top = src, (base_dir or "."), None
    else:
        path = os.path.abspath(str(src))
        doc, base, top = hcdfdom.load(path), os.path.dirname(path), path
    _resolve(doc, base, notes, _stack=((top,) if top else ()))
    return doc, notes


def _resolve(doc, base, notes, _stack):
    pending = list(doc.include or [])
    doc.include = []
    for inc in pending:
        uri = inc.uri
        if uri is None:
            continue
        sub_path = uri if os.path.isabs(uri) else os.path.join(base, uri)
        sub_path = os.path.normpath(os.path.abspath(sub_path))
        if not os.path.exists(sub_path):
            doc.include.append(inc)   # keep it; cannot resolve
            notes.append(f"include {uri!r}: file not found at {sub_path!r}; left unresolved")
            continue
        if sub_path in _stack:
            raise ValueError(f"include cycle detected: {' -> '.join(_stack + (sub_path,))}")
        sub = hcdfdom.load(sub_path)
        _resolve(sub, os.path.dirname(sub_path), notes, _stack + (sub_path,))  # nested first
        if doc.body_frame is not None and sub.body_frame is not None and doc.body_frame != sub.body_frame:
            notes.append(f"include {uri!r}: body-frame {sub.body_frame.value} differs from including "
                         f"document {doc.body_frame.value} (not converted; convention should match)")
        if inc.name:
            _prefix(sub, inc.name)
        if inc.pose:
            _offset(sub, inc.pose)
        _merge(doc, sub)
        notes.append(f"flattened include {uri!r}"
                     + (f" as {inc.name!r}" if inc.name else "")
                     + (f" at pose {inc.pose!r}" if inc.pose else ""))


def _merge(doc, sub):
    for attr in ("comp", "joint", "group", "state", "color", "network", "transmission", "extension"):
        getattr(doc, attr).extend(getattr(sub, attr) or [])


# ── name prefixing + reference rewriting ──────────────────────────────────────
def _prefix(sub, prefix):
    p = prefix + _SEP
    comps = {c.name for c in sub.comp if c.name}
    joints = {j.name for j in sub.joint if j.name}
    groups = {g.name for g in sub.group if g.name}
    colors = {c.name for c in sub.color if c.name}

    def pc(n):
        return p + n if n in comps else n

    def pj(n):
        return p + n if n in joints else n

    def pg(n):
        return p + n if n in groups else n

    # rename definitions
    for seq in (sub.comp, sub.joint, sub.group, sub.state, sub.transmission):
        for e in (seq or []):
            if e.name:
                e.name = p + e.name
    for col in sub.color:
        if col.name:
            col.name = p + col.name

    # rewrite references (decided against the ORIGINAL name sets above)
    for j in sub.joint:
        if j.parent and j.parent.comp:
            j.parent.comp = pc(j.parent.comp)
        if j.child and j.child.comp:
            j.child.comp = pc(j.child.comp)
        if j.mimic and j.mimic.joint:
            j.mimic.joint = pj(j.mimic.joint)
        if j.loop:
            if j.loop.predecessor:
                j.loop.predecessor = pc(j.loop.predecessor)
            if j.loop.successor:
                j.loop.successor = pc(j.loop.successor)
    for g in sub.group:
        for jr in (g.joint or []):
            if jr.ref:
                jr.ref = pj(jr.ref)
        for gr in (g.group or []):
            if gr.ref:
                gr.ref = pg(gr.ref)
        if g.tip_comp:
            g.tip_comp = pc(g.tip_comp)        # tip_frame is a comp-local frame name -> unchanged
    for s in sub.state:
        for jp in (s.joint_position or []):
            if jp.joint:
                jp.joint = pj(jp.joint)
    for c in sub.comp:
        siblings = {f.name for f in (c.frame or []) if f.name}
        for f in (c.frame or []):
            r = f.relative_to
            if r is None or r in siblings:     # sibling frame: comp-local, stays
                continue
            if r in comps or r in joints:      # comp / joint anchor -> prefixed
                f.relative_to = p + r
        for v in (c.visual or []):
            if v.color and v.color.name in colors and v.color.rgba is None:
                v.color.name = p + v.color.name   # reference to a (now-prefixed) document color
    for t in sub.transmission:
        if t.joint and t.joint.ref in joints:
            t.joint.ref = p + t.joint.ref
        if t.motor and t.motor.ref:
            head, _, tail = t.motor.ref.partition(".")
            if head in comps:
                t.motor.ref = p + head + (("." + tail) if tail else "")


# ── pose offset (rigid placement) ─────────────────────────────────────────────
def _offset(sub, pose_str):
    vals = pose_str.split()
    off = M.Pose()
    off.xyz = " ".join(vals[:3]) if len(vals) >= 3 else "0 0 0"
    if len(vals) >= 6:
        off.rpy = " ".join(vals[3:6])
    Moff = frames.pose_to_matrix(off)

    child = {j.child.comp for j in sub.joint if j.loop is None and j.child}
    roots = {c.name for c in sub.comp if c.name not in child}

    def comp(pose):
        if pose is None:
            return frames.matrix_to_pose(Moff, use_quat=False)
        return frames.matrix_to_pose(Moff @ frames.pose_to_matrix(pose), use_quat=False)

    for c in sub.comp:
        if c.name not in roots:
            continue
        for v in (c.visual or []):
            v.pose = comp(v.pose)
        for col in (c.collision or []):
            col.pose = comp(col.pose)
        if c.inertial is not None:
            c.inertial.inertia_origin = comp(c.inertial.inertia_origin)
        for f in (c.frame or []):
            f.pose = comp(f.pose)
    for j in sub.joint:
        if j.loop is None and j.parent and j.parent.comp in roots:
            j.origin = comp(j.origin)
