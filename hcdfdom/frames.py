"""Pose & frame-convention normalizer for hcdfdom.

Pose conventions (hcdf.xsd ``pose``):
  * ``xyz``  — "x y z" metres.
  * ``rpy``  — "roll pitch yaw" radians, **XYZ extrinsic** (fixed-axis): R = Rz(yaw)·Ry(pitch)·Rx(roll).
  * ``quat`` — "x y z w", **Hamilton, scalar-last**; if present it **wins** over rpy.

Frame conventions (right-handed both ways), as orthogonal change-of-basis matrices C
applied to a pose by  t' = C·t,  R' = C·R·Cᵀ:
  * Body  FRD ↔ FLU : a 180° roll about X,  C = diag(1, −1, −1).
  * World NED ↔ ENU : swap X↔Y, flip Z,     C = [[0,1,0],[1,0,0],[0,0,−1]].
Both are involutions (C = Cᵀ = C⁻¹) and proper rotations (det +1), so converting twice
is the identity — the basis of the round-trip tests.

This is the numeric layer on top of the lossless string DOM: it parses pose strings to
floats, does the math in numpy, and re-emits a ``Pose``. Walking a whole document (which
pose is body- vs world-frame) is the converter's job (urdf_io); this module is the
primitives + single-pose conversion, exhaustively unit-tested.
"""
from __future__ import annotations

import numpy as np

from .model import BodyFrame, Pose, WorldFrame

# ── change-of-basis matrices ──────────────────────────────────────────────────
FRD_FLU = np.diag([1.0, -1.0, -1.0])                     # body: FRD <-> FLU (180° roll about X)
NED_ENU = np.array([[0.0, 1.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 0.0, -1.0]])                   # world: NED <-> ENU (swap X<->Y, flip Z)
_I3 = np.eye(3)


def change_of_basis(src, dst) -> np.ndarray:
    """Orthogonal 3x3 mapping a pose expressed in ``src`` convention into ``dst``.

    ``src``/``dst`` are BodyFrame members (FLU/FRD) or WorldFrame members (ENU/NED).
    """
    if src == dst:
        return _I3.copy()
    pair = {src, dst}
    if pair == {BodyFrame.FRD, BodyFrame.FLU}:
        return FRD_FLU.copy()
    if pair == {WorldFrame.NED, WorldFrame.ENU}:
        return NED_ENU.copy()
    raise ValueError(f"no change-of-basis between {src!r} and {dst!r}")


# ── elementary rotations & rpy <-> matrix (XYZ extrinsic) ─────────────────────
def _rx(a):
    c, s = np.cos(a), np.sin(a)
    return np.array([[1, 0, 0], [0, c, -s], [0, s, c]])


def _ry(a):
    c, s = np.cos(a), np.sin(a)
    return np.array([[c, 0, s], [0, 1, 0], [-s, 0, c]])


def _rz(a):
    c, s = np.cos(a), np.sin(a)
    return np.array([[c, -s, 0], [s, c, 0], [0, 0, 1]])


def rpy_to_matrix(roll, pitch, yaw) -> np.ndarray:
    """XYZ-extrinsic rpy -> rotation matrix:  R = Rz(yaw) @ Ry(pitch) @ Rx(roll)."""
    return _rz(yaw) @ _ry(pitch) @ _rx(roll)


def matrix_to_rpy(R) -> tuple[float, float, float]:
    """Rotation matrix -> (roll, pitch, yaw), XYZ extrinsic. Handles gimbal lock."""
    R = np.asarray(R, dtype=float)
    if abs(R[2, 0]) < 1.0 - 1e-10:
        pitch = np.arcsin(-R[2, 0])
        roll = np.arctan2(R[2, 1], R[2, 2])
        yaw = np.arctan2(R[1, 0], R[0, 0])
    else:  # gimbal lock: pitch = ±90°, fold roll into yaw (set roll = 0)
        pitch = np.pi / 2 if R[2, 0] < 0 else -np.pi / 2
        roll = 0.0
        yaw = np.arctan2(-R[0, 1], R[1, 1])
    return float(roll), float(pitch), float(yaw)


# ── quaternion (Hamilton, scalar-last x y z w) <-> matrix ─────────────────────
def quat_to_matrix(x, y, z, w) -> np.ndarray:
    n = np.sqrt(x * x + y * y + z * z + w * w)
    if n == 0:
        return _I3.copy()
    x, y, z, w = x / n, y / n, z / n, w / n
    return np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - w * z),     2 * (x * z + w * y)],
        [2 * (x * y + w * z),     1 - 2 * (x * x + z * z), 2 * (y * z - w * x)],
        [2 * (x * z - w * y),     2 * (y * z + w * x),     1 - 2 * (x * x + y * y)],
    ])


def matrix_to_quat(R) -> tuple[float, float, float, float]:
    """Rotation matrix -> (x, y, z, w), Hamilton scalar-last, canonical sign (w >= 0)."""
    R = np.asarray(R, dtype=float)
    t = np.trace(R)
    if t > 0:
        s = np.sqrt(t + 1.0) * 2
        w = 0.25 * s
        x = (R[2, 1] - R[1, 2]) / s
        y = (R[0, 2] - R[2, 0]) / s
        z = (R[1, 0] - R[0, 1]) / s
    elif R[0, 0] > R[1, 1] and R[0, 0] > R[2, 2]:
        s = np.sqrt(1.0 + R[0, 0] - R[1, 1] - R[2, 2]) * 2
        w = (R[2, 1] - R[1, 2]) / s
        x = 0.25 * s
        y = (R[0, 1] + R[1, 0]) / s
        z = (R[0, 2] + R[2, 0]) / s
    elif R[1, 1] > R[2, 2]:
        s = np.sqrt(1.0 + R[1, 1] - R[0, 0] - R[2, 2]) * 2
        w = (R[0, 2] - R[2, 0]) / s
        x = (R[0, 1] + R[1, 0]) / s
        y = 0.25 * s
        z = (R[1, 2] + R[2, 1]) / s
    else:
        s = np.sqrt(1.0 + R[2, 2] - R[0, 0] - R[1, 1]) * 2
        w = (R[1, 0] - R[0, 1]) / s
        x = (R[0, 2] + R[2, 0]) / s
        y = (R[1, 2] + R[2, 1]) / s
        z = 0.25 * s
    q = np.array([x, y, z, w])
    q /= np.linalg.norm(q)
    if q[3] < 0:           # canonical sign: w >= 0 (q and -q are the same rotation)
        q = -q
    return tuple(float(v) for v in q)


# ── parsing & formatting ──────────────────────────────────────────────────────
def _floats(s):
    return [float(v) for v in s.split()] if s else None


def _fmt(vals):
    out = []
    for v in vals:
        if v == 0:           # collapse -0.0
            v = 0.0
        out.append("%.12g" % v)
    return " ".join(out)


# ── Pose <-> numeric ──────────────────────────────────────────────────────────
def pose_translation(pose: Pose) -> np.ndarray:
    v = _floats(pose.xyz) if pose is not None else None
    return np.array(v, dtype=float) if v else np.zeros(3)


def pose_rotation(pose: Pose) -> np.ndarray:
    """Rotation matrix of a pose, applying quat-wins precedence."""
    if pose is None:
        return _I3.copy()
    if pose.quat:
        return quat_to_matrix(*_floats(pose.quat))
    if pose.rpy:
        return rpy_to_matrix(*_floats(pose.rpy))
    return _I3.copy()


def pose_to_matrix(pose: Pose) -> np.ndarray:
    """Pose -> 4x4 homogeneous transform (quat-wins)."""
    M = np.eye(4)
    M[:3, :3] = pose_rotation(pose)
    M[:3, 3] = pose_translation(pose)
    return M


def matrix_to_pose(M, use_quat: bool = True) -> Pose:
    """4x4 (or 3x3) -> Pose. Emits xyz + quat by default (lossless rotation); use_quat=False emits rpy."""
    M = np.asarray(M, dtype=float)
    R = M[:3, :3]
    t = M[:3, 3] if M.shape == (4, 4) else np.zeros(3)
    p = Pose()
    p.xyz = _fmt(t)
    if use_quat:
        p.quat = _fmt(matrix_to_quat(R))
    else:
        p.rpy = _fmt(matrix_to_rpy(R))
    return p


# ── single-pose frame conversion ──────────────────────────────────────────────
def transform_pose(pose: Pose, C, use_quat: bool = True) -> Pose:
    """Apply an orthogonal change-of-basis C to a pose:  t' = C·t,  R' = C·R·Cᵀ."""
    C = np.asarray(C, dtype=float)
    R = pose_rotation(pose)
    t = pose_translation(pose)
    M = np.eye(4)
    M[:3, :3] = C @ R @ C.T
    M[:3, 3] = C @ t
    return matrix_to_pose(M, use_quat=use_quat)


def convert_body(pose: Pose, src, dst, use_quat: bool = True) -> Pose:
    """Convert a body-frame pose between BodyFrame conventions (FLU/FRD)."""
    return transform_pose(pose, change_of_basis(src, dst), use_quat=use_quat)


def convert_world(pose: Pose, src, dst, use_quat: bool = True) -> Pose:
    """Convert a world-frame pose between WorldFrame conventions (ENU/NED)."""
    return transform_pose(pose, change_of_basis(src, dst), use_quat=use_quat)
