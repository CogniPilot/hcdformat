#!/usr/bin/env python3
"""Tests for the SDFormat <-> HCDF peer spoke (sdf_io).

Covers (1) SDF->HCDF mapping of the overlap, (2) value-exact SDF->HCDF->SDF->HCDF round-trip,
(3) the "SDF is a wider peer than URDF" wins — HCDF->SDF preserves closed loops, ball/universal/screw
joints, and capsule/cone geometry that HCDF->URDF must drop, (4) the HCDF cyber layer -> loss manifest,
(5) the optional libsdformat (`gz`) path: our emitted SDF passes `gz --check`, a clean URDF converts
clean, and PR2 exercises the "exit 0 is NOT clean" diagnostic-surfacing contract + recovering parse.

gz-dependent checks skip cleanly when libsdformat is absent (like the assets tests without trimesh).

Run:  python3 tests/test_sdf_io.py
"""
from __future__ import annotations

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from hcdfdom import model as M  # noqa: E402
from hcdfdom.validate import is_valid  # noqa: E402
from sdf_io import SDF_VERSION, from_sdf, gz, to_sdf  # noqa: E402

_n = _fail = 0


def check(cond, msg):
    global _n, _fail
    _n += 1
    if cond:
        print(f"  \033[32mok\033[0m   {msg}")
    else:
        _fail += 1
        print(f"  \033[31mFAIL\033[0m {msg}")


def two_link_doc():
    """base -> link1 via a revolute joint; box collision+visual; minimal but validator-clean."""
    d = M.Hcdf(); d.name = "bot"; d.body_frame = M.BodyFrame("FLU"); d.world_frame = M.WorldFrame("ENU")
    base = M.Comp(); base.name = "base"
    v = M.Visual(); v.name = "bv"; v.geometry = M.VisualGeometry(); v.geometry.box = M.Box(); v.geometry.box.size = "1 1 1"
    base.visual = [v]
    link = M.Comp(); link.name = "link1"
    d.comp = [base, link]
    j = M.Joint(); j.name = "j1"; j.type = M.JointType("revolute")
    j.parent = M.JointParent(); j.parent.comp = "base"
    j.child = M.JointChild(); j.child.comp = "link1"
    j.axis = M.Axis(); j.axis.xyz = "0 0 1"
    j.limit = M.JointLimit(); j.limit.lower = "-1"; j.limit.upper = "1"; j.limit.effort = "5"; j.limit.velocity = "2"
    d.joint = [j]
    return d


def loss_text(doc):
    _, loss = to_sdf(doc)
    return loss


def main():
    print("SDF -> HCDF mapping (clean-model.sdf):")
    doc, notes = from_sdf(os.path.join(ROOT, "tests/sdf/clean-model.sdf"))
    check(doc.name == "clean-bot", "model name -> hcdf name")
    check([c.name for c in doc.comp] == ["base", "link1"], "links -> comps")
    check(doc.body_frame == M.BodyFrame("FLU") and doc.world_frame == M.WorldFrame("ENU"), "SDF tagged FLU/ENU (no transform)")
    base = doc.comp[0]
    check(base.inertial.mass == "2.0" and base.inertial.inertia == "0.1 0 0 0.1 0 0.1", "inertial mass + inertia tuple")
    check(base.inertial.inertia_origin.xyz == "0 0 0.1", "inertial <pose> -> inertia_origin")
    check(base.collision[0].surface.friction.static == "0.8" and base.collision[0].surface.friction.dynamic == "0.6",
          "collision <surface><friction><ode><mu/mu2> -> friction static/dynamic")
    check(base.collision[0].surface.restitution == "0.2", "<bounce><restitution_coefficient> -> restitution")
    check(base.visual[0].color.rgba == "0.8 0.2 0.2 1", "<material><diffuse> -> color rgba")
    j = doc.joint[0]
    check(j.type.value == "revolute" and j.parent.comp == "base" and j.child.comp == "link1", "joint type + parent/child links")
    check(j.axis.xyz == "0 0 1" and (j.limit.lower, j.limit.upper) == ("-1.5", "1.5"), "axis xyz + limit")
    check((j.dynamics.damping, j.dynamics.friction) == ("0.1", "0.05"), "axis <dynamics> -> damping/friction")
    check(is_valid(doc), "imported clean-model.sdf is validator-clean")

    print("\nValue-exact round-trip (SDF -> HCDF -> SDF -> HCDF):")
    xml, loss = to_sdf(doc)
    doc2, _ = from_sdf(xml)

    def snap(d):
        return (d.name,
                tuple((c.name, c.inertial.mass if c.inertial else None,
                       tuple((v.name, getattr(v.geometry.box, "size", None) if (v.geometry and v.geometry.box) else
                              (v.geometry.cylinder.radius if (v.geometry and v.geometry.cylinder) else None),
                              v.color.rgba if v.color else None) for v in c.visual),
                       tuple((col.name, col.surface.friction.static if (col.surface and col.surface.friction) else None,
                              col.surface.restitution if col.surface else None) for col in c.collision)) for c in d.comp),
                tuple((j.name, j.type.value, j.parent.comp, j.child.comp, j.axis.xyz if j.axis else None,
                       (j.limit.lower, j.limit.upper) if j.limit else None,
                       (j.dynamics.damping, j.dynamics.friction) if j.dynamics else None) for j in d.joint))
    check(snap(doc) == snap(doc2), "overlap is value-exact across the round-trip")
    check(is_valid(doc2), "re-imported emitted SDF is validator-clean")
    check(len(loss) == 0, f"clean overlap doc -> SDF has zero losses (got {len(loss)})")

    print("\nSDF is a wider peer than URDF (HCDF->SDF preserves what HCDF->URDF drops):")
    # closed loop -> emitted as an ordinary SDF joint, no loss
    d = two_link_doc(); d.joint[0].loop = M.LoopClosure()
    xml, loss = to_sdf(d)
    check("<joint" in xml and not any(c == "loop" for c, _ in loss.items), "loop-closure joint emitted (no loss) — SDF expresses loops")
    # ball / universal / screw joint types survive
    for jt, needle in (("ball", 'type="ball"'), ("universal", 'type="universal"'), ("screw", 'type="screw"')):
        d = two_link_doc(); d.joint[0].type = M.JointType(jt)
        if jt == "screw":
            d.joint[0].thread_pitch = "0.01"
        if jt == "universal":
            d.joint[0].axis2 = M.Axis(); d.joint[0].axis2.xyz = "0 1 0"
        xml, loss = to_sdf(d)
        check(needle in xml and not any(c == "joint-type" for c, _ in loss.items), f"{jt} joint preserved (no downgrade loss)")
    # capsule + cone geometry survive (URDF would drop them)
    for prim, sub in (("capsule", M.Capsule()), ("cone", M.Cone())):
        d = two_link_doc()
        col = M.Collision(); col.name = "c"; col.geometry = M.CollisionGeometry()
        sub.radius = "0.1"; sub.length = "0.3"; setattr(col.geometry, prim, sub)
        d.comp[0].collision = [col]
        xml, loss = to_sdf(d)
        check(f"<{prim}>" in xml and not any(c == "geometry" for c, _ in loss.items), f"{prim} geometry preserved")
    # collision surface survives
    d = two_link_doc()
    col = M.Collision(); col.name = "c"; col.geometry = M.CollisionGeometry(); col.geometry.box = M.Box(); col.geometry.box.size = "1 1 1"
    col.surface = M.Surface(); col.surface.friction = M.Friction(); col.surface.friction.static = "0.9"
    d.comp[0].collision = [col]
    xml, loss = to_sdf(d)
    check("<surface>" in xml and "<mu>0.9</mu>" in xml, "collision <surface> contact physics preserved")

    print("\nHCDF cyber layer has no SDF home (-> loss manifest):")
    d = two_link_doc()
    d.comp[0].motor = [object()]; d.comp[0].sensor = [object()]; d.comp[0].struct_type = "aluminum"
    d.network = [object()]; d.transmission = [object()]; d.group = [object()]
    _, loss = to_sdf(d)
    txt = loss.text().lower()
    for needle in ("motor", "sensor", "struct-type", "network", "transmission", "joint group"):
        check(needle in txt, f"cyber construct reported: {needle!r}")

    print("\nfrom_sdf robustness:")
    try:
        from_sdf("<notsdf/>"); check(False, "non-SDF root should raise")
    except ValueError:
        check(True, "non-SDF root raises ValueError")
    try:
        from_sdf("<sdf version='1.12'></sdf>"); check(False, "no <model> should raise")
    except ValueError:
        check(True, "SDF without <model> raises ValueError")
    _, n = from_sdf("<sdf version='1.12'><world name='w'><model name='m'><link name='l'/></model></world></sdf>")
    check(any("world" in x for x in n), "world content -> note (out of HCDF-core scope)")

    print("\nHardening regressions:")
    # cone/ellipsoid are in-scope overlap shapes -> must round-trip (were dropped on import)
    d = two_link_doc()
    col1 = M.Collision(); col1.name = "cc"; col1.geometry = M.CollisionGeometry()
    col1.geometry.cone = M.Cone(); col1.geometry.cone.radius = "0.1"; col1.geometry.cone.length = "0.3"
    col2 = M.Collision(); col2.name = "ce"; col2.geometry = M.CollisionGeometry()
    col2.geometry.ellipsoid = M.Ellipsoid(); col2.geometry.ellipsoid.radii = "0.1 0.2 0.3"
    d.comp[0].collision = [col1, col2]
    xml, loss = to_sdf(d); d2, _ = from_sdf(xml)
    check(len(d2.comp[0].collision) == 2 and len(loss) == 0, "cone + ellipsoid collisions round-trip (no silent drop)")
    # mimic must be emitted under <axis> with a <reference> child (SDF 1.12), not under <joint>
    d = two_link_doc(); j = d.joint[0]
    j.mimic = M.Mimic(); j.mimic.joint = "j0"; j.mimic.multiplier = "2"; j.mimic.offset = "0.1"
    xml, _ = to_sdf(d)
    check("<mimic" in xml and xml.index("<axis>") < xml.index("<mimic") and "<reference>" in xml,
          "<mimic> emitted under <axis> with required <reference>")
    # SDF type with no HCDF equiv -> 'fixed' must NOT keep axis/limit (would be an invalid HCDF doc)
    d3, n3 = from_sdf("<sdf version='1.12'><model name='m'><link name='a'/><link name='b'/>"
                      "<joint name='j' type='revolute2'><parent>a</parent><child>b</child>"
                      "<axis><xyz>0 0 1</xyz><limit><lower>-1</lower></limit></axis></joint></model></sdf>")
    check(is_valid(d3) and d3.joint[0].axis is None and d3.joint[0].limit is None,
          "revolute2 -> 'fixed' drops axis/limit (validator-clean) + notes the drop")
    check(any("downgrade" in x for x in n3), "downgrade drop is noted")
    # partial SDF <inertia> (off-diagonals omitted) is valid SDF -> must map with defaults, not drop
    dp, _ = from_sdf("<sdf version='1.12'><model name='m'><link name='b'><inertial><mass>1</mass>"
                     "<inertia><ixx>0.5</ixx><iyy>0.5</iyy><izz>0.5</izz></inertia></inertial></link></model></sdf>")
    check(dp.comp[0].inertial.inertia == "0.5 0.0 0.0 0.5 0.0 0.5", "partial <inertia> maps with SDF defaults")
    # frame-qualifier / scale notes must reach the caller (not be swallowed)
    _, nq = from_sdf("<sdf version='1.12'><model name='m'><link name='a'/><link name='b'/>"
                     "<joint name='j' type='revolute'><parent>a</parent><child>b</child>"
                     "<axis><xyz expressed_in='b'>0 0 1</xyz></axis></joint></model></sdf>")
    check(any("expressed_in" in x for x in nq), "<axis><xyz expressed_in> qualifier is noted")
    # HCDF->SDF silent-drop coverage for metadata that has no SDF home
    d = two_link_doc()
    d.description = "doc"; d.author = "me"; d.comp[0].description = "cd"; d.comp[0].urdf_compat = M.UrdfCompatComp()
    d.joint[0].description = "jd"; d.joint[0].calibration = M.JointCalibration()
    _, loss = to_sdf(d); t = loss.text().lower()
    for needle in ("document author", "comp 'base': <description>", "urdf-compat", "joint 'j1': <description>", "calibration"):
        check(needle in t, f"silent drop now reported: {needle!r}")
    # degenerate docs must not crash or emit invalid SDF
    _, le = to_sdf(M.Hcdf())
    check("requires" in le.text(), "empty doc (no comps) -> recorded that SDF needs >=1 link")

    print(f"\nlibsdformat (gz) path  [SDF_VERSION pinned = {SDF_VERSION}]:")
    if not gz.available():
        check(True, "gz/libsdformat not present — skipping gz-delegated checks")
    else:
        import tempfile
        xml, _ = to_sdf(doc)
        with tempfile.NamedTemporaryFile("w", suffix=".sdf", delete=False) as fh:
            fh.write(xml); p = fh.name
        res = gz.check(p); os.unlink(p)
        check(res.ok and not res.errors, "our emitted SDF passes `gz --check` (libsdformat accepts it)")

        clean = gz.urdf_to_sdf(os.path.join(ROOT, "tests/sdf/clean.urdf"))
        check(clean.ok and not clean.diagnostics, "clean URDF -> SDF via gz is clean (ok, no diagnostics)")
        dc, _ = from_sdf(clean.stdout.encode())
        check(is_valid(dc) and len(dc.comp) == 2, "gz-converted clean URDF imports to a clean 2-link HCDF")

        pr2 = os.path.expanduser("~/git/urdf/urdf/test/pr2_desc.urdf")
        if os.path.exists(pr2):
            r = gz.urdf_to_sdf(pr2)
            check(r.returncode == 0 and not r.ok and r.diagnostics,
                  "PR2 URDF->SDF: exit 0 but ok=False with diagnostics surfaced ('exit 0 is not clean')")
            dp, _ = from_sdf(r.stdout.encode())  # legacy <sensor:...> tags -> recovering parse, no crash
            check(is_valid(dp) and len(dp.comp) > 0, f"PR2 SDF recovering-imports to a clean HCDF ({len(dp.comp)} links)")
        else:
            check(True, "PR2 fixture absent — skipping the diagnostic-surfacing integration check")

    print(f"\nsdf_io tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
