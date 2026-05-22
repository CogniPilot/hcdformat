#!/usr/bin/env python3
"""Tests for the hcdfdom pose/frame normalizer (hcdfdom/frames.py).

Run:  python3 tests/test_frames.py
"""
from __future__ import annotations

import os
import sys

import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

import hcdfdom  # noqa: E402
from hcdfdom import frames as F  # noqa: E402
from hcdfdom.model import BodyFrame, Pose, WorldFrame  # noqa: E402

_n = 0
_fail = 0


def check(cond, msg):
    global _n, _fail
    _n += 1
    if cond:
        print(f"  \033[32mok\033[0m   {msg}")
    else:
        _fail += 1
        print(f"  \033[31mFAIL\033[0m {msg}")


def P(xyz=None, rpy=None, quat=None):
    p = Pose()
    p.xyz, p.rpy, p.quat = xyz, rpy, quat
    return p


def main():
    # ── change-of-basis matrix properties ───────────────────────────────────
    for name, C in (("FRD_FLU", F.FRD_FLU), ("NED_ENU", F.NED_ENU)):
        check(np.allclose(C @ C.T, np.eye(3)), f"{name} is orthogonal")
        check(np.isclose(np.linalg.det(C), 1.0), f"{name} is a proper rotation (det +1)")
        check(np.allclose(C @ C, np.eye(3)), f"{name} is an involution (C·C = I)")

    # ── known point transforms ───────────────────────────────────────────────
    check(np.allclose(F.FRD_FLU @ [1, 2, 3], [1, -2, -3]),
          "FRD->FLU maps (1,2,3) -> (1,-2,-3)")
    check(np.allclose(F.NED_ENU @ [1, 2, 3], [2, 1, -3]),
          "NED->ENU maps (1,2,3) -> (2,1,-3)")

    # ── rpy <-> matrix round-trip (XYZ extrinsic) ────────────────────────────
    rng = np.random.default_rng(0)
    ok = True
    for _ in range(2000):
        r, p, y = rng.uniform(-3, 3), rng.uniform(-1.4, 1.4), rng.uniform(-3, 3)  # avoid gimbal
        R = F.rpy_to_matrix(r, p, y)
        r2, p2, y2 = F.matrix_to_rpy(R)
        if not np.allclose(F.rpy_to_matrix(r2, p2, y2), R, atol=1e-9):
            ok = False
            break
    check(ok, "rpy -> matrix -> rpy reproduces the rotation (2000 samples)")

    # yaw 90° sends +X to +Y
    check(np.allclose(F.rpy_to_matrix(0, 0, np.pi / 2) @ [1, 0, 0], [0, 1, 0]),
          "yaw=90° rotates +X to +Y")

    # ── quat <-> matrix round-trip + quat-wins ───────────────────────────────
    ok = True
    for _ in range(2000):
        q = rng.normal(size=4)
        q /= np.linalg.norm(q)
        R = F.quat_to_matrix(*q)
        check_R = F.quat_to_matrix(*F.matrix_to_quat(R))
        if not np.allclose(check_R, R, atol=1e-9):
            ok = False
            break
    check(ok, "quat -> matrix -> quat reproduces the rotation (2000 samples)")
    check(np.allclose(F.quat_to_matrix(*F.matrix_to_quat(np.eye(3))), np.eye(3)),
          "identity matrix -> quat -> matrix is identity")

    # quat-wins: pose carries a yaw=90° rpy AND an identity quat -> rotation must be identity
    qw = P(rpy="0 0 1.5708", quat="0 0 0 1")
    check(np.allclose(F.pose_rotation(qw), np.eye(3)),
          "quat-wins: identity quat overrides a non-trivial rpy")

    # ── transform_pose: t'=C·t, R'=C·R·Cᵀ ────────────────────────────────────
    pose = P(xyz="1 2 3", rpy="0.3 -0.4 1.1")
    C = F.FRD_FLU
    out = F.transform_pose(pose, C)
    check(np.allclose(F.pose_translation(out), C @ F.pose_translation(pose)),
          "transform_pose translation = C·t")
    check(np.allclose(F.pose_rotation(out), C @ F.pose_rotation(pose) @ C.T),
          "transform_pose rotation = C·R·Cᵀ")

    # ── frame-conversion round-trips (involution) on synthetic + fixture poses ─
    synth = [P(xyz="1 2 3"), P(xyz="0 0 0.5", rpy="0.1 0.2 0.3"),
             P(quat="0.0 0.7071068 0.0 0.7071068"), P(xyz="-1 0.5 2", rpy="1.2 -0.6 2.9")]
    doc = hcdfdom.load(os.path.join(ROOT, "tests", "valid", "articulated-arm.hcdf"))
    fixture_poses = [j.origin for j in doc.joint if j.origin is not None]
    check(len(fixture_poses) >= 2, f"articulated-arm contributes {len(fixture_poses)} joint-origin poses")

    body_ok = world_ok = True
    for pose in synth + fixture_poses:
        R0, t0 = F.pose_rotation(pose), F.pose_translation(pose)
        b = F.convert_body(F.convert_body(pose, BodyFrame.FRD, BodyFrame.FLU),
                           BodyFrame.FLU, BodyFrame.FRD)
        if not (np.allclose(F.pose_rotation(b), R0, atol=1e-9) and
                np.allclose(F.pose_translation(b), t0, atol=1e-9)):
            body_ok = False
        w = F.convert_world(F.convert_world(pose, WorldFrame.NED, WorldFrame.ENU),
                            WorldFrame.ENU, WorldFrame.NED)
        if not (np.allclose(F.pose_rotation(w), R0, atol=1e-9) and
                np.allclose(F.pose_translation(w), t0, atol=1e-9)):
            world_ok = False
    check(body_ok, "FRD->FLU->FRD is identity on all synthetic + fixture poses")
    check(world_ok, "NED->ENU->ENU->NED is identity on all synthetic + fixture poses")

    # same-convention conversion is a no-op
    check(np.allclose(F.change_of_basis(BodyFrame.FLU, BodyFrame.FLU), np.eye(3)),
          "change_of_basis(FLU, FLU) is identity")

    # matrix_to_pose emits rpy when asked, and it round-trips
    pr = F.matrix_to_pose(F.rpy_to_matrix(0.2, 0.3, -0.5), use_quat=False)
    check(pr.rpy is not None and pr.quat is None, "matrix_to_pose(use_quat=False) emits rpy")
    check(np.allclose(F.pose_rotation(pr), F.rpy_to_matrix(0.2, 0.3, -0.5), atol=1e-9),
          "matrix_to_pose rpy emission round-trips")

    print(f"\nframes tests: {_n - _fail} passed, {_fail} failed")
    return 1 if _fail else 0


if __name__ == "__main__":
    sys.exit(main())
