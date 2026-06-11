#!/usr/bin/env python3
"""Tests for the loss-manifest export + HCDF-URDF profile checker.

Covers (1) LossManifest.to_dict/to_json/markdown, (2) the three-tier profile classification
(identity / with-transform / out-of-profile) on built minimal docs + the real fixtures, and
(3) the consistency contract between the classifier and the field-level loss manifest:
    IDENTITY / WITH-TRANSFORM-via-GLB  =>  zero field-level losses
    OUT-OF-PROFILE                     =>  at least one field-level loss
plus a smoke test of the `python3 -m urdf_hcdf.profile` CLI.

Run:  python3 tests/test_profile.py
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

from hcdfdom import dump, load  # noqa: E402
from hcdfdom import model as M  # noqa: E402
from hcdfdom.validate import is_valid  # noqa: E402
from urdf_hcdf import (BENIGN_LOSS_CATEGORIES, IDENTITY, OUT_OF_PROFILE,  # noqa: E402
                       WITH_TRANSFORM, check_profile, from_urdf)
from urdf_hcdf.to_urdf import LossManifest, to_urdf  # noqa: E402

_n = _fail = 0


def check(cond, msg):
    global _n, _fail
    _n += 1
    if cond:
        print(f"  \033[32mok\033[0m   {msg}")
    else:
        _fail += 1
        print(f"  \033[31mFAIL\033[0m {msg}")


def identity_doc():
    """A minimal doc that is URDF-identity: FLU/ENU, primitive geometry, flat color, clean tree."""
    d = M.Hcdf()
    d.name = "id-bot"
    d.body_frame = M.BodyFrame.FLU
    d.world_frame = M.WorldFrame.ENU
    col = M.Color(); col.name = "red"; col.rgba = "1 0 0 1"
    d.color = [col]
    base = M.Comp(); base.name = "base"
    v = M.Visual(); v.name = "base_vis"
    v.geometry = M.VisualGeometry(); v.geometry.box = M.Box(); v.geometry.box.size = "1 1 1"
    v.color = M.Color(); v.color.name = "red"        # name-only ref to the top-level color
    base.visual = [v]
    link = M.Comp(); link.name = "link1"
    v2 = M.Visual(); v2.name = "l1_vis"
    v2.geometry = M.VisualGeometry(); v2.geometry.cylinder = M.Cylinder()
    v2.geometry.cylinder.radius = "0.1"; v2.geometry.cylinder.length = "0.5"
    link.visual = [v2]
    d.comp = [base, link]
    j = M.Joint(); j.name = "j1"; j.type = M.JointType.revolute
    j.parent = M.JointParent(); j.parent.comp = "base"
    j.child = M.JointChild(); j.child.comp = "link1"
    j.axis = M.Axis(); j.axis.xyz = "0 0 1"
    j.limit = M.JointLimit(); j.limit.lower = "-1"; j.limit.upper = "1"
    j.limit.effort = "10"; j.limit.velocity = "2"
    d.joint = [j]
    return d


def codes(report):
    return sorted({f.code for f in report.findings})


def main():
    print("LossManifest structured export:")
    m = LossManifest()
    check(m.to_dict()["total"] == 0, "empty manifest -> total 0")
    check("No losses" in m.markdown(), "empty manifest markdown says 'No losses'")
    m.add("joint", "axis2 dropped"); m.add("joint", "thread_pitch dropped"); m.add("visual", "capsule dropped")
    d = m.to_dict()
    check(d["total"] == 3, "3 items counted")
    check(set(d["categories"]) == {"joint", "visual"} and len(d["categories"]["joint"]) == 2,
          "items grouped by category (joint x2, visual x1)")
    check(json.loads(m.to_json())["total"] == 3, "to_json is valid JSON with the same total")
    md = m.markdown()
    check("## joint (2)" in md and "## visual (1)" in md, "markdown has per-category sections with counts")

    print("\nProfile: IN-PROFILE-IDENTITY")
    doc = identity_doc()
    check(is_valid(doc), "identity doc is validator-clean")
    r = check_profile(doc)
    check(r.classification == IDENTITY, f"classified IDENTITY (got {r.classification})")
    check(r.is_identity and r.in_profile, "is_identity and in_profile")
    check(len(r.loss) == 0, f"identity => zero field-level losses (got {len(r.loss)})")
    check(r.findings == [], "identity => no profile findings")

    print("\nProfile: IN-PROFILE-WITH-FRAME-TRANSFORM")
    # (a) body-frame FRD
    d_frd = identity_doc(); d_frd.body_frame = M.BodyFrame.FRD
    r = check_profile(d_frd)
    check(r.classification == WITH_TRANSFORM and "P_BODY_FRD" in codes(r), "FRD body -> WITH_TRANSFORM (P_BODY_FRD)")
    check(len(r.loss) == 0, "FRD body conversion is applied -> zero field-level losses")
    # (b) GLB visual model
    d_glb = identity_doc()
    g = d_glb.comp[0].visual[0]; g.geometry = None; g.color = None
    g.model = M.ModelRef(); g.model.uri = "base.glb"
    r = check_profile(d_glb)
    check(r.classification == WITH_TRANSFORM and "P_GLB_MODEL" in codes(r), "GLB model -> WITH_TRANSFORM (P_GLB_MODEL)")
    check(len(r.loss) == 0, "GLB model exports as a URDF mesh ref -> zero field-level losses")
    # (c) imported synth-arm (mesh visuals -> GLB) is the natural with-transform fixture
    arm, _ = from_urdf(os.path.join(ROOT, "tests/urdf/synth-arm.urdf"))
    r = check_profile(arm)
    check(r.classification == WITH_TRANSFORM, "imported synth-arm.urdf -> WITH_TRANSFORM")
    check(r.in_profile and not r.is_identity, "synth-arm in_profile but not identity")

    print("\nProfile: OUT-OF-PROFILE")
    # screw joint (HCDF-only type + thread_pitch)
    d_screw = identity_doc(); j = d_screw.joint[0]
    j.type = M.JointType.screw; j.limit = None; j.thread_pitch = "0.01"
    r = check_profile(d_screw)
    check(r.classification == OUT_OF_PROFILE and {"P_JOINT_TYPE", "P_THREAD_PITCH"} <= set(codes(r)),
          "screw joint -> OUT (P_JOINT_TYPE + P_THREAD_PITCH)")
    check(len(r.loss) > 0, "out-of-profile screw => at least one field-level loss")
    # HCDF-only primitive (capsule)
    d_cap = identity_doc(); vg = d_cap.comp[1].visual[0]
    vg.geometry = M.VisualGeometry(); vg.geometry.capsule = M.Capsule()
    vg.geometry.capsule.radius = "0.1"; vg.geometry.capsule.length = "0.5"
    r = check_profile(d_cap)
    check(r.classification == OUT_OF_PROFILE and "P_GEOMETRY" in codes(r), "capsule visual -> OUT (P_GEOMETRY)")
    check(len(r.loss) > 0, "out-of-profile capsule => at least one field-level loss")
    # cyber layer (a comp sensor)
    d_sensor = identity_doc(); s = M.Sensor(); s.name = "imu"; d_sensor.comp[0].sensor = [s]
    r = check_profile(d_sensor)
    check(r.classification == OUT_OF_PROFILE and "P_COMP_SENSOR" in codes(r), "comp sensor -> OUT (P_COMP_SENSOR)")
    check(not r.in_profile, "out-of-profile => not in_profile")

    print("\nProfile: real fixtures")
    r = check_profile(load(os.path.join(ROOT, "tests/valid/fourbar-loop.hcdf")))
    check(r.classification == OUT_OF_PROFILE and "P_LOOP" in codes(r), "fourbar-loop -> OUT (P_LOOP)")
    check(len(r.loss) > 0, "fourbar OUT => losses present")
    r = check_profile(load(os.path.join(ROOT, "tests/valid/articulated-arm.hcdf")))
    check(r.classification == OUT_OF_PROFILE and {"P_GROUP", "P_STATE", "P_COMP_FRAME"} <= set(codes(r)),
          "articulated-arm -> OUT (P_GROUP + P_STATE + P_COMP_FRAME)")

    print("\nHardening regressions: silent drops + crashes")

    def setquat(d):
        d.comp[0].visual[0].pose = M.Pose(); d.comp[0].visual[0].pose.quat = "0.7071068 0 0 0.7071068"

    def set_inertia(d, tup):
        ip = M.InertialProperties(); ip.mass = "1"; ip.inertia = tup; d.comp[0].inertial = ip

    def glb_color(d):
        v = d.comp[0].visual[0]; v.geometry = None
        v.model = M.ModelRef(); v.model.uri = "x.glb"; v.color = M.Color(); v.color.rgba = "1 0 0 1"

    def empty_visual(d):
        v = d.comp[1].visual[0]; v.geometry = None; v.model = None

    def empty_collision(d):
        col = M.Collision(); col.name = "c"; col.geometry = M.CollisionGeometry(); d.comp[0].collision = [col]

    def add_verbose(d):
        col = M.Collision(); col.name = "c"; col.geometry = M.CollisionGeometry()
        col.geometry.box = M.Box(); col.geometry.box.size = "1 1 1"; col.verbose = "true"
        d.comp[0].collision = [col]

    # (label, mutate, expected_tier, expected_code|None)  — every row is a confirmed past defect.
    REG = [
        ("comp struct-type -> OUT", lambda d: setattr(d.comp[0], "struct_type", "aluminum-6061"), OUT_OF_PROFILE, "P_COMP_STRUCT_TYPE"),
        ("comp ip-rating -> OUT", lambda d: setattr(d.comp[0], "ip_rating", "IP67"), OUT_OF_PROFILE, "P_COMP_IP_RATING"),
        ("comp role -> OUT", lambda d: setattr(d.comp[0], "role", "sensor-head"), OUT_OF_PROFILE, "P_COMP_ROLE"),
        ("comp hwid -> OUT", lambda d: setattr(d.comp[0], "hwid", "HW-99"), OUT_OF_PROFILE, "P_COMP_HWID"),
        ("visual toggle -> OUT", lambda d: setattr(d.comp[0].visual[0], "toggle", "grp"), OUT_OF_PROFILE, "P_VISUAL_TOGGLE"),
        ("thread_pitch on revolute -> OUT", lambda d: setattr(d.joint[0], "thread_pitch", "0.01"), OUT_OF_PROFILE, "P_THREAD_PITCH"),
        ("malformed inertia tuple -> OUT", lambda d: set_inertia(d, "0.1 0.2 0.3"), OUT_OF_PROFILE, "P_INERTIA_ARITY"),
        ("GLB model + flat color -> OUT", glb_color, OUT_OF_PROFILE, "P_GLB_COLOR"),
        ("empty visual -> OUT", empty_visual, OUT_OF_PROFILE, "P_VISUAL_NO_GEOMETRY"),
        ("empty collision -> OUT", empty_collision, OUT_OF_PROFILE, "P_COLLISION_NO_GEOMETRY"),
        ("quat pose -> WITH_TRANSFORM", setquat, WITH_TRANSFORM, "P_POSE_QUAT"),
        ("comp description -> IDENTITY (benign)", lambda d: setattr(d.comp[0], "description", "x"), IDENTITY, None),
        ("joint description -> IDENTITY (benign)", lambda d: setattr(d.joint[0], "description", "x"), IDENTITY, None),
        ("color description -> IDENTITY (benign)", lambda d: setattr(d.color[0], "description", "x"), IDENTITY, None),
        ("doc author -> IDENTITY (benign)", lambda d: setattr(d, "author", "me"), IDENTITY, None),
        ("doc version -> IDENTITY (benign)", lambda d: setattr(d, "version", "1.0"), IDENTITY, None),
        ("collision @verbose -> IDENTITY (benign)", add_verbose, IDENTITY, None),
    ]
    for label, mutate, exp, code in REG:
        d = identity_doc(); mutate(d)
        r = check_profile(d)
        ok_cls = r.classification == exp
        ok_code = code is None or code in codes(r)
        # the consistency contract: anything still IN-PROFILE must carry no non-benign losses
        ok_contract = (not r.in_profile) or (r.nonbenign_losses == [])
        check(ok_cls and ok_code and ok_contract, label)

    print("\nConsistency contract over the construct corpus:")
    # every confirmed-IN-PROFILE doc above already asserted nonbenign_losses==[]. Now the converse
    # mechanism: a doc loaded with ONLY benign metadata is IDENTITY yet records (non-empty, all-benign) losses.
    d = identity_doc()
    d.description = "doc"; d.author = "a"; d.license = "MIT"; d.url = "u"; d.version = "1.0"
    d.comp[0].description = "c"; d.joint[0].description = "j"; d.color[0].description = "col"
    r = check_profile(d); _, loss = to_urdf(d)
    check(r.is_identity and r.nonbenign_losses == [] and len(loss) > 0,
          f"all-benign-metadata doc: IDENTITY, recorded {len(loss)} benign loss(es), zero non-benign")
    check(all(c in BENIGN_LOSS_CATEGORIES for c, _ in loss.items), "every recorded loss is in a benign category")

    print("\nRobustness: degenerate input must not crash")
    d = identity_doc(); d.joint[0].type = "revolute"  # raw string instead of the JointType enum
    check(check_profile(d).classification in (IDENTITY, WITH_TRANSFORM, OUT_OF_PROFILE),
          "raw-string joint type: classifies without crashing")
    dn = M.Hcdf(); dn.name = "t"
    a = M.Comp(); a.name = None; b = M.Comp(); b.name = None; dn.comp = [a, b]
    check(check_profile(dn).classification == OUT_OF_PROFILE, "multi-root with None names: OUT, no crash")
    check(check_profile(M.Hcdf()).classification == IDENTITY, "empty doc: IDENTITY, no crash")

    print("\nReport serialization:")
    r = check_profile(identity_doc())
    j = json.loads(r.to_json())
    check(j["classification"] == IDENTITY and j["in_profile"] is True, "to_json round-trips classification + in_profile")
    check("loss_manifest" in j and j["loss_manifest"]["total"] == 0, "to_json embeds the loss manifest")
    check(IDENTITY in r.markdown(), "markdown names the classification")
    r2 = check_profile(load(os.path.join(ROOT, "tests/valid/fourbar-loop.hcdf")))
    check("Out-of-profile drivers" in r2.markdown(), "out-of-profile markdown lists the drivers")

    print("\nCLI smoke test (python3 -m urdf_hcdf.profile):")
    with tempfile.TemporaryDirectory() as td:
        idf = os.path.join(td, "id.hcdf")
        dump(identity_doc(), idf)
        p_id = subprocess.run([sys.executable, "-m", "urdf_hcdf.profile", idf],
                              cwd=ROOT, capture_output=True, text=True)
        check(p_id.returncode == 0 and IDENTITY in p_id.stdout, "CLI on identity doc -> exit 0 + IDENTITY")
        p_out = subprocess.run([sys.executable, "-m", "urdf_hcdf.profile",
                                os.path.join(ROOT, "tests/valid/fourbar-loop.hcdf"), "--json"],
                               cwd=ROOT, capture_output=True, text=True)
        ok_json = p_out.returncode == 1 and json.loads(p_out.stdout)["classification"] == OUT_OF_PROFILE
        check(ok_json, "CLI --json on out-of-profile doc -> exit 1 + valid JSON")

    print(f"\nprofile tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
