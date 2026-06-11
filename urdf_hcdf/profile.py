"""HCDF-URDF Profile (1.0) checker — the clean-export contract.

Classifies an hcdfdom document by how cleanly it exports to URDF:

- ``IN-PROFILE-IDENTITY``        — exports to clean valid URDF with a value-exact round-trip;
                                   no frame transform, no asset bake, nothing dropped.
- ``IN-PROFILE-WITH-FRAME-TRANSFORM`` — same fidelity, but a frame conversion (FRD/NED),
                                   a GLB-appearance bake, or an ``<include>`` flatten is needed first.
- ``OUT-OF-PROFILE``            — exporting to URDF drops model-significant content (closed loops,
                                   HCDF-only joint types/primitives, contact ``<surface>``, the cyber
                                   layer, ``group``/``state``, …). The URDF is a lossy projection.

The classification is the worst tier across all findings. Each finding is mapped directly from the
profile contract by inspecting the typed DOM — readable and auditable against the spec text. The report
also carries the authoritative field-level ``LossManifest`` (from ``to_urdf``) and the validator
issues, so "which tier and why" (findings) and "exactly what content is lost" (manifest) are both
available and can be cross-checked.

Deliberate deviations from the strict profile definition, grounded in what ``to_urdf`` actually does:
- **Root ``<extension>``** (gazebo / ros2_control / transmission / vendor passthrough) is NOT
  out-of-profile: ``to_urdf`` de-merges it back to the native top-level URDF elements, so it
  round-trips. Only **comp-level and joint-level** ``<extension>`` are truly dropped → out-of-profile.
- **Authored ``visual``/``collision`` names** are NOT flagged: the converter emits and re-reads them
  (urdfdom URDF carries ``<visual name>``/``<collision name>``), so they round-trip.

    from urdf_hcdf import check_profile
    report = check_profile(doc)
    report.classification        # one of the three tier strings
    report.in_profile            # True unless OUT-OF-PROFILE
    print(report.markdown())     # human report  (report.to_json() for machines)

    python3 -m urdf_hcdf.profile robot.hcdf [--json]
"""
from __future__ import annotations

from dataclasses import dataclass, field

from hcdfdom.validate import ERROR, validate

from .to_urdf import LossManifest, to_urdf

# Tier labels + ordering (worst wins).
IDENTITY = "IN-PROFILE-IDENTITY"
WITH_TRANSFORM = "IN-PROFILE-WITH-FRAME-TRANSFORM"
OUT_OF_PROFILE = "OUT-OF-PROFILE"
_RANK = {IDENTITY: 0, WITH_TRANSFORM: 1, OUT_OF_PROFILE: 2}

_TIER_MEANING = {
    IDENTITY: "Exports to clean valid URDF with a value-exact round-trip; nothing dropped.",
    WITH_TRANSFORM: "Exports losslessly after a frame conversion, GLB-appearance bake, or include flatten.",
    OUT_OF_PROFILE: "Exporting to URDF drops model-significant content; the URDF is a lossy projection.",
}

# URDF-expressible joint types (free maps to URDF 'floating'). Anything else is out-of-profile.
_IN_PROFILE_JOINT_TYPES = {"revolute", "continuous", "prismatic", "fixed", "free", "planar"}
# Primitives HCDF has but URDF core does not.
_NON_URDF_PRIMS = ("capsule", "cone", "ellipsoid")
# Shapes that DO export (a visual mesh is the GLB <model>, so visuals carry no <mesh>).
_URDF_VISUAL_PRIMS = ("box", "cylinder", "sphere")
_URDF_COLLISION_PRIMS = ("box", "cylinder", "sphere", "mesh")

# HCDF-only comp metadata scalars with no URDF home (silently dropped before step 6 hardened this).
_COMP_METADATA = [
    ("struct_type", "P_COMP_STRUCT_TYPE", "struct-type"), ("ip_rating", "P_COMP_IP_RATING", "ip-rating"),
    ("role", "P_COMP_ROLE", "role"), ("hwid", "P_COMP_HWID", "hwid"),
]

# Loss-manifest categories that are pure annotation (doc/comp/joint/color <description>, doc author/
# license/url/version, collision @verbose): recorded by to_urdf for honesty but NOT model content, so
# they never drop a doc below IN-PROFILE-IDENTITY. The consistency contract is keyed off this set.
BENIGN_LOSS_CATEGORIES = frozenset({"annotation"})

# Per-comp HCDF-only content with no URDF home (dropped by to_urdf). 'frame' included: URDF has no
# named auxiliary coordinate frames. 'extension' included: comp-level extensions are NOT de-merged.
_COMP_DROPPED = [
    ("sensor", "sensors"), ("motor", "motors"), ("hmi", "HMI elements"),
    ("dynamic_surface", "dynamic surfaces"), ("power_source", "power sources"),
    ("port", "ports"), ("antenna", "antennas"), ("board", "boards"),
    ("operating_temp", "operating-temp blocks"), ("switch", "switches"),
    ("software", "software blocks"), ("discovered", "discovered-device blocks"),
    ("frame", "coordinate frames"), ("extension", "vendor extensions"),
]
# Top-level HCDF-only content with no URDF home (dropped by to_urdf).
_TOP_DROPPED = [
    ("group", "joint groups"), ("state", "kinematic states"), ("network", "networks"),
    ("transmission", "HCDF transmissions"), ("self_collision_disable", "self-collision-disable blocks"),
]


def _val(enum_or_str):
    return enum_or_str.value if hasattr(enum_or_str, "value") else enum_or_str


def _count(vals):
    if not vals:
        return 0
    return len(vals) if isinstance(vals, (list, tuple)) else 1


def _bad_prim(geo):
    """Return the first HCDF-only primitive present on a geometry, or None."""
    if geo is None:
        return None
    return next((p for p in _NON_URDF_PRIMS if getattr(geo, p, None) is not None), None)


def _has_shape(geo, prims):
    return geo is not None and any(getattr(geo, p, None) is not None for p in prims)


def _pose_quat(pose):
    """True if a pose carries a quaternion orientation (emitted as URDF rpy — a representation change)."""
    return pose is not None and getattr(pose, "quat", None) is not None


@dataclass
class Finding:
    """One reason a document is at a given tier (the driver of the classification)."""
    tier: str
    code: str
    detail: str


@dataclass
class ProfileReport:
    classification: str
    findings: list = field(default_factory=list)
    loss: LossManifest = field(default_factory=LossManifest)
    issues: list = field(default_factory=list)  # validator Issues

    @property
    def in_profile(self):
        """True if URDF-consumable (identity or with-transform); False if out-of-profile."""
        return self.classification != OUT_OF_PROFILE

    @property
    def is_identity(self):
        return self.classification == IDENTITY

    def by_tier(self, tier):
        return [f for f in self.findings if f.tier == tier]

    @property
    def nonbenign_losses(self):
        """Loss items that represent dropped MODEL content (excludes pure annotation/metadata).

        The consistency contract: a doc with ANY non-benign loss must NOT be IN-PROFILE (it must be
        out-of-profile) — every such drop has a matching out-of-profile finding.
        """
        return [(c, d) for c, d in self.loss.items if c not in BENIGN_LOSS_CATEGORIES]

    def to_dict(self):
        return {
            "classification": self.classification,
            "meaning": _TIER_MEANING[self.classification],
            "in_profile": self.in_profile,
            "findings": [{"tier": f.tier, "code": f.code, "detail": f.detail} for f in self.findings],
            "validator_errors": [str(i) for i in self.issues if i.level == ERROR],
            "loss_manifest": self.loss.to_dict(),
        }

    def to_json(self, indent=2):
        import json
        return json.dumps(self.to_dict(), indent=indent)

    def markdown(self):
        lines = [f"# HCDF-URDF profile: {self.classification}", "", _TIER_MEANING[self.classification], ""]
        out = self.by_tier(OUT_OF_PROFILE)
        xf = self.by_tier(WITH_TRANSFORM)
        if out:
            lines.append(f"## Out-of-profile drivers ({len(out)})")
            lines.extend(f"- `{f.code}` {f.detail}" for f in out)
            lines.append("")
        if xf:
            lines.append(f"## Transform / bake needed ({len(xf)})")
            lines.extend(f"- `{f.code}` {f.detail}" for f in xf)
            lines.append("")
        errs = [i for i in self.issues if i.level == ERROR]
        if errs:
            lines.append(f"## Validator errors ({len(errs)})")
            lines.extend(f"- {i}" for i in errs)
            lines.append("")
        lines.append("## Field-level loss manifest")
        lines.append("")
        lines.append(self.loss.markdown(title="Export drops").split("\n", 2)[-1]
                     if self.loss else "_No field-level losses._")
        return "\n".join(lines).rstrip() + "\n"


def check_profile(doc) -> ProfileReport:
    """Classify ``doc`` against the HCDF-URDF Profile 1.0."""
    findings: list[Finding] = []
    issues = validate(doc)
    comps = doc.comp or []
    joints = doc.joint or []

    # ── structure: a single connected acyclic tree with exactly one root ────────────
    for i in issues:
        if i.level == ERROR and i.code in ("E_MULTI_PARENT", "E_CYCLE", "E_SELF_JOINT", "E_LOOP_REF"):
            findings.append(Finding(OUT_OF_PROFILE, "P_NOT_A_TREE", f"{i.code}: {i.message}"))
    tree_joints = [j for j in joints if j.loop is None]
    children = set()
    for j in tree_joints:
        c = getattr(j.child, "comp", None) if j.child is not None else None
        if c:
            children.add(c)
    roots = [c.name for c in comps if c.name not in children]
    if comps and len(roots) == 0:
        findings.append(Finding(OUT_OF_PROFILE, "P_NO_ROOT",
                                "no root comp (every comp is a joint child — a tree needs one root)"))
    elif len(roots) > 1:
        names = ", ".join(repr(r) for r in sorted(roots, key=lambda r: (r is None, r or "")))
        findings.append(Finding(OUT_OF_PROFILE, "P_MULTI_ROOT",
                                f"{len(roots)} roots ({names}); URDF is one robot with "
                                f"one root link (a network-only comp counts as an extra root)"))

    # ── frame: URDF is always FLU/ENU ───────────────────────────────────────────────
    if _val(doc.body_frame) == "FRD":
        findings.append(Finding(WITH_TRANSFORM, "P_BODY_FRD",
                                "body-frame FRD: body poses and joint axes are converted to URDF FLU "
                                "on export (lossless)"))
    if _val(doc.world_frame) == "NED":
        findings.append(Finding(WITH_TRANSFORM, "P_WORLD_NED",
                                "world-frame NED: URDF is ENU; world-relative placements need conversion "
                                "(note: to_urdf does not yet apply the world transform — body poses ARE converted)"))

    # ── joints ──────────────────────────────────────────────────────────────────────
    quat_poses = 0  # in-profile poses using quat orientation (emitted as rpy — a representation change)
    for j in joints:
        nm = j.name
        if _pose_quat(j.origin):
            quat_poses += 1
        if j.loop is not None:
            findings.append(Finding(OUT_OF_PROFILE, "P_LOOP",
                                    f"joint {nm!r}: loop-closure (URDF is tree-only — cannot express a "
                                    f"closed kinematic loop)"))
        jt = _val(j.type)
        if jt not in _IN_PROFILE_JOINT_TYPES:
            findings.append(Finding(OUT_OF_PROFILE, "P_JOINT_TYPE",
                                    f"joint {nm!r}: type {jt!r} has no URDF equivalent"))
        if j.axis2 is not None:
            findings.append(Finding(OUT_OF_PROFILE, "P_AXIS2",
                                    f"joint {nm!r}: <axis2> (URDF has a single axis)"))
        if j.thread_pitch is not None:
            findings.append(Finding(OUT_OF_PROFILE, "P_THREAD_PITCH",
                                    f"joint {nm!r}: thread_pitch (URDF has no screw joint)"))
        if j.limit is not None:
            for fld in ("acceleration", "jerk", "deceleration"):
                if getattr(j.limit, fld, None) is not None:
                    findings.append(Finding(OUT_OF_PROFILE, "P_LIMIT_FIELD",
                                            f"joint {nm!r}: limit/{fld} (URDF limit has only "
                                            f"lower/upper/effort/velocity)"))
        if j.dynamics is not None:
            for sp in ("spring_stiffness", "spring_reference"):
                if getattr(j.dynamics, sp, None) is not None:
                    findings.append(Finding(OUT_OF_PROFILE, "P_DYNAMICS_SPRING",
                                            f"joint {nm!r}: dynamics/{sp} (URDF dynamics has only "
                                            f"damping/friction)"))
        if j.extension is not None:
            findings.append(Finding(OUT_OF_PROFILE, "P_JOINT_EXTENSION",
                                    f"joint {nm!r}: <extension> vendor data (no URDF home — joint "
                                    f"extensions are not de-merged)"))

    # ── per-comp: geometry, contact surface, cyber layer, metadata ──────────────────
    for comp in comps:
        ctx = f"comp {comp.name!r}"
        ip = comp.inertial
        if _pose_quat(getattr(ip, "inertia_origin", None)):
            quat_poses += 1
        if ip is not None and ip.inertia is not None and len(ip.inertia.split()) != 6:
            findings.append(Finding(OUT_OF_PROFILE, "P_INERTIA_ARITY",
                                    f"{ctx}: inertia tuple {ip.inertia!r} is not 6 values (the rotational "
                                    f"inertia matrix is dropped on URDF export)"))
        for v in (comp.visual or []):
            if _pose_quat(v.pose):
                quat_poses += 1
            if v.model is not None:
                findings.append(Finding(WITH_TRANSFORM, "P_GLB_MODEL",
                                        f"{ctx} visual {v.name!r}: GLB <model> — geometry exports as a URDF "
                                        f"mesh ref, but baked PBR/textures are not represented in URDF"))
                if v.color is not None:
                    findings.append(Finding(OUT_OF_PROFILE, "P_GLB_COLOR",
                                            f"{ctx} visual {v.name!r}: flat <color> alongside a GLB <model> is "
                                            f"dropped on export (and is schema-forbidden)"))
            else:
                bad = _bad_prim(v.geometry)
                if bad:
                    findings.append(Finding(OUT_OF_PROFILE, "P_GEOMETRY",
                                            f"{ctx} visual {v.name!r}: <{bad}> has no URDF primitive"))
                elif not _has_shape(v.geometry, _URDF_VISUAL_PRIMS):
                    findings.append(Finding(OUT_OF_PROFILE, "P_VISUAL_NO_GEOMETRY",
                                            f"{ctx} visual {v.name!r}: neither a GLB <model> nor a URDF "
                                            f"primitive — dropped on export"))
            if v.toggle is not None:
                findings.append(Finding(OUT_OF_PROFILE, "P_VISUAL_TOGGLE",
                                        f"{ctx} visual {v.name!r}: toggle group {v.toggle!r} (HCDF runtime "
                                        f"show/hide grouping; no URDF home)"))
        for c in (comp.collision or []):
            if _pose_quat(c.pose):
                quat_poses += 1
            bad = _bad_prim(c.geometry)
            if bad:
                findings.append(Finding(OUT_OF_PROFILE, "P_GEOMETRY",
                                        f"{ctx} collision {c.name!r}: <{bad}> has no URDF primitive"))
            elif not _has_shape(c.geometry, _URDF_COLLISION_PRIMS):
                findings.append(Finding(OUT_OF_PROFILE, "P_COLLISION_NO_GEOMETRY",
                                        f"{ctx} collision {c.name!r}: no URDF-representable geometry — "
                                        f"dropped on export"))
            if c.surface is not None:
                findings.append(Finding(OUT_OF_PROFILE, "P_SURFACE",
                                        f"{ctx} collision {c.name!r}: <surface> contact physics (no URDF "
                                        f"core home — lives in <gazebo> at sim time)"))
        for attr, label in _COMP_DROPPED:
            n = _count(getattr(comp, attr, None))
            if n:
                findings.append(Finding(OUT_OF_PROFILE, "P_COMP_" + attr.upper(),
                                        f"{ctx}: {n} {label} (HCDF-only; no URDF home)"))
        for attr, code, label in _COMP_METADATA:
            val = getattr(comp, attr, None)
            if val is not None:
                findings.append(Finding(OUT_OF_PROFILE, code,
                                        f"{ctx}: {label}={val!r} (HCDF-only metadata; no URDF home)"))

    # ── top-level HCDF-only content ─────────────────────────────────────────────────
    for attr, label in _TOP_DROPPED:
        n = _count(getattr(doc, attr, None))
        if n:
            findings.append(Finding(OUT_OF_PROFILE, "P_" + attr.upper(),
                                    f"document: {n} {label} (HCDF-only; no URDF home)"))
    if doc.include:
        findings.append(Finding(WITH_TRANSFORM, "P_INCLUDE",
                                f"document: {_count(doc.include)} <include>(s) — flatten with "
                                f"hcdf_io.flatten() before URDF export"))
    if quat_poses:
        findings.append(Finding(WITH_TRANSFORM, "P_POSE_QUAT",
                                f"{quat_poses} pose(s) use quat orientation; emitted as URDF rpy "
                                f"(rotation preserved exactly — URDF <origin> has no quaternion)"))

    # worst tier wins
    cls = IDENTITY
    for f in findings:
        if _RANK[f.tier] > _RANK[cls]:
            cls = f.tier

    try:
        _, loss = to_urdf(doc)
    except Exception as e:  # noqa: BLE001 — a broken doc should still yield a report
        loss = LossManifest()
        loss.add("export-error", f"to_urdf raised: {e}")

    return ProfileReport(cls, findings, loss, issues)


def _main(argv):
    import sys

    from hcdfdom import load

    as_json = "--json" in argv
    paths = [a for a in argv if not a.startswith("-")]
    if not paths:
        print("usage: python3 -m urdf_hcdf.profile <file.hcdf> [--json]", file=sys.stderr)
        return 2
    report = check_profile(load(paths[0]))
    print(report.to_json() if as_json else report.markdown())
    return 0 if report.in_profile else 1


if __name__ == "__main__":
    import sys

    sys.exit(_main(sys.argv[1:]))
