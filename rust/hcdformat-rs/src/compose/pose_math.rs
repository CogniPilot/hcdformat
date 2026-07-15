//! Self-contained f64 pose math for include flattening: a direct port of `hcdfdom/frames.py`'s
//! primitives (NO external deps). A pose's rotation is XYZ-extrinsic rpy `R = Rz(yaw)·Ry(pitch)·Rx(roll)`,
//! or a Hamilton scalar-last (`x y z w`) quaternion which WINS over rpy when present.
//!
//! Transforms are represented as a 4×4 row-major homogeneous matrix [`Mat4`] (`[[f64;4];4]`). Only the
//! operations flattening needs are provided: pose→matrix (quat-wins), matrix→pose (rpy out, quat
//! cleared, matching `matrix_to_pose(use_quat=False)`), and 4×4 multiply.

use crate::model::Pose;

/// A 4×4 row-major homogeneous transform.
pub type Mat4 = [[f64; 4]; 4];

/// The 4×4 identity.
fn identity() -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

/// Rotation about X by `a` radians (3×3, as the rotation block).
fn rx(a: f64) -> [[f64; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

/// Rotation about Y by `a` radians.
fn ry(a: f64) -> [[f64; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// Rotation about Z by `a` radians.
fn rz(a: f64) -> [[f64; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// 3×3 multiply.
pub(crate) fn mul33(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (i, orow) in out.iter_mut().enumerate() {
        for (j, ocell) in orow.iter_mut().enumerate() {
            *ocell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

/// The FRD↔FLU body change-of-basis `C = diag(1, −1, −1)` (an involution, `C = Cᵀ = C⁻¹`). Shared by the
/// URDF/SDF converters ([`crate::to_urdf`]/[`crate::to_sdf`]) for the frame change-of-basis, hence gated
/// to those features (without them it is dead code: include flattening itself never needs the basis flip).
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub(crate) const FRD_FLU: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];

/// 3×3 matrix × 3-vector. Only the URDF/SDF converters need it (the change-of-basis), so it is gated with
/// [`FRD_FLU`]; flattening's 4×4 [`mul44`] path does not use it.
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub(crate) fn mat3_vec(a: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    for (i, ocell) in out.iter_mut().enumerate() {
        *ocell = (0..3).map(|k| a[i][k] * v[k]).sum();
    }
    out
}

/// 3×3 transpose. Used only by the URDF/SDF converters (inverse change-of-basis `Cᵀ`), so gated with
/// [`FRD_FLU`].
#[cfg(any(feature = "urdf", feature = "sdf"))]
pub(crate) fn transpose(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in m.iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            out[j][i] = v;
        }
    }
    out
}

/// 4×4 multiply: `a @ b`.
pub fn mul44(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for (i, orow) in out.iter_mut().enumerate() {
        for (j, ocell) in orow.iter_mut().enumerate() {
            *ocell = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

/// XYZ-extrinsic rpy → 3×3 rotation: `R = Rz(yaw)·Ry(pitch)·Rx(roll)`. Matches `frames.rpy_to_matrix`.
pub fn rpy_to_matrix(roll: f64, pitch: f64, yaw: f64) -> [[f64; 3]; 3] {
    mul33(&mul33(&rz(yaw), &ry(pitch)), &rx(roll))
}

/// 3×3 rotation → (roll, pitch, yaw), XYZ-extrinsic, with gimbal-lock handling. Matches
/// `frames.matrix_to_rpy` exactly (including the `1 - 1e-10` threshold and the lock branch).
pub fn matrix_to_rpy(r: &[[f64; 3]; 3]) -> (f64, f64, f64) {
    if r[2][0].abs() < 1.0 - 1e-10 {
        let pitch = (-r[2][0]).asin();
        let roll = r[2][1].atan2(r[2][2]);
        let yaw = r[1][0].atan2(r[0][0]);
        (roll, pitch, yaw)
    } else {
        // gimbal lock: pitch = ±90°, fold roll into yaw (roll = 0)
        let pitch = if r[2][0] < 0.0 {
            std::f64::consts::FRAC_PI_2
        } else {
            -std::f64::consts::FRAC_PI_2
        };
        let roll = 0.0;
        let yaw = (-r[0][1]).atan2(r[1][1]);
        (roll, pitch, yaw)
    }
}

/// Normalize a Hamilton scalar-last quaternion `(x, y, z, w)` to a 3×3 rotation. Matches
/// `frames.quat_to_matrix` (a zero quaternion maps to identity).
fn quat_to_matrix(q: [f64; 4]) -> [[f64; 3]; 3] {
    let [mut x, mut y, mut z, mut w] = q;
    let n = (x * x + y * y + z * z + w * w).sqrt();
    if n == 0.0 {
        let mut m = [[0.0; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        return m;
    }
    x /= n;
    y /= n;
    z /= n;
    w /= n;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// The rotation block of a pose, applying quat-wins precedence (quat → rpy → identity). Matches
/// `frames.pose_rotation`.
fn pose_rotation(pose: &Pose) -> [[f64; 3]; 3] {
    if let Some(q) = pose.quat {
        return quat_to_matrix(q);
    }
    let rpy = pose.rpy_or_zero();
    rpy_to_matrix(rpy[0], rpy[1], rpy[2])
}

/// Pose → 4×4 homogeneous transform (quat-wins for rotation, xyz for translation). Matches
/// `frames.pose_to_matrix`.
pub fn pose_to_matrix(pose: &Pose) -> Mat4 {
    let r = pose_rotation(pose);
    let xyz = pose.xyz_or_zero();
    let mut m = identity();
    for (i, rrow) in r.iter().enumerate() {
        m[i][..3].copy_from_slice(rrow);
        m[i][3] = xyz[i];
    }
    m
}

/// 4×4 transform → [`Pose`], emitting xyz + rpy and clearing quat, i.e. `matrix_to_pose(use_quat=False)`.
pub fn matrix_to_pose(m: &Mat4) -> Pose {
    let r = [
        [m[0][0], m[0][1], m[0][2]],
        [m[1][0], m[1][1], m[1][2]],
        [m[2][0], m[2][1], m[2][2]],
    ];
    let (roll, pitch, yaw) = matrix_to_rpy(&r);
    // A transformed/offset pose carries explicit xyz + rpy (quat cleared), matching Python
    // `matrix_to_pose(use_quat=False)`, which always sets both string attributes.
    Pose {
        xyz: Some([m[0][3], m[1][3], m[2][3]]),
        rpy: Some([roll, pitch, yaw]),
        quat: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn close3(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| close(*x, y))
    }

    #[test]
    fn yaw_half_pi_maps_x_to_y() {
        // Rz(pi/2) should send +x -> +y.
        let r = rpy_to_matrix(0.0, 0.0, PI / 2.0);
        let v = [r[0][0], r[1][0], r[2][0]]; // R @ [1,0,0]
        assert!(close3(v, [0.0, 1.0, 0.0]), "got {v:?}");
    }

    #[test]
    fn rpy_roundtrips_through_matrix() {
        for &(r, p, y) in &[
            (0.1, 0.2, 0.3),
            (-0.5, 0.4, 1.2),
            (PI / 6.0, -PI / 4.0, PI / 3.0),
        ] {
            let m = rpy_to_matrix(r, p, y);
            let (r2, p2, y2) = matrix_to_rpy(&m);
            assert!(
                close3([r, p, y], [r2, p2, y2]),
                "{r},{p},{y} != {r2},{p2},{y2}"
            );
        }
    }

    #[test]
    fn gimbal_lock_pitch_plus_half_pi() {
        // R[2,0] = -sin(pitch); pitch = +pi/2 => R[2,0] = -1 => the (R[2,0] < 0) branch.
        let m = rpy_to_matrix(0.0, PI / 2.0, 0.0);
        let (roll, pitch, _yaw) = matrix_to_rpy(&m);
        assert!(close(pitch, PI / 2.0), "pitch {pitch}");
        assert!(close(roll, 0.0), "roll folded into yaw, roll={roll}");
    }

    #[test]
    fn gimbal_lock_pitch_minus_half_pi() {
        let m = rpy_to_matrix(0.0, -PI / 2.0, 0.0);
        let (roll, pitch, _yaw) = matrix_to_rpy(&m);
        assert!(close(pitch, -PI / 2.0), "pitch {pitch}");
        assert!(close(roll, 0.0));
    }

    #[test]
    fn quat_wins_over_rpy() {
        // Identity quat (0 0 0 1) must beat a nonzero rpy.
        let pose = Pose {
            xyz: Some([1.0, 2.0, 3.0]),
            rpy: Some([0.5, 0.5, 0.5]),
            quat: Some([0.0, 0.0, 0.0, 1.0]),
        };
        let m = pose_to_matrix(&pose);
        // rotation block is identity
        for (i, row) in m.iter().enumerate().take(3) {
            for (j, &cell) in row.iter().enumerate().take(3) {
                assert!(close(cell, if i == j { 1.0 } else { 0.0 }));
            }
        }
        assert!(close3([m[0][3], m[1][3], m[2][3]], [1.0, 2.0, 3.0]));
    }

    #[test]
    fn offset_composes_translation_then_rotation() {
        // M_off = translate (1,0,0) + yaw 90deg; pose = translate (0,1,0), identity rot.
        // M_off @ pose: the pose's origin (0,1,0) is rotated by Rz(90) -> (-1,0,0) then translated by
        // (1,0,0) -> (0,0,0). Hand-derived.
        let off = Pose {
            xyz: Some([1.0, 0.0, 0.0]),
            rpy: Some([0.0, 0.0, PI / 2.0]),
            quat: None,
        };
        let pose = Pose {
            xyz: Some([0.0, 1.0, 0.0]),
            rpy: Some([0.0, 0.0, 0.0]),
            quat: None,
        };
        let m = mul44(&pose_to_matrix(&off), &pose_to_matrix(&pose));
        let out = matrix_to_pose(&m);
        assert!(
            close3(out.xyz_or_zero(), [0.0, 0.0, 0.0]),
            "xyz {:?}",
            out.xyz
        );
        // resulting rotation is still yaw 90.
        assert!(close(out.rpy_or_zero()[2], PI / 2.0), "yaw {:?}", out.rpy);
        assert!(out.quat.is_none(), "matrix_to_pose must clear quat");
    }

    #[test]
    fn none_pose_offset_is_the_offset_itself() {
        // The _offset comp(None) path returns the offset matrix as a pose.
        let off = Pose {
            xyz: Some([2.0, 3.0, 4.0]),
            rpy: Some([0.0, 0.0, PI / 2.0]),
            quat: None,
        };
        let m = pose_to_matrix(&off);
        let out = matrix_to_pose(&m);
        assert!(close3(out.xyz_or_zero(), [2.0, 3.0, 4.0]));
        assert!(close(out.rpy_or_zero()[2], PI / 2.0));
    }
}
