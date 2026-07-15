//! Helpers for the official string-typed numeric attributes ("x y z", "ixx ixy ... izz", quat).
//!
//! HCDF encodes vectors/tensors as space-separated doubles inside XML *attributes* (e.g.
//! `<pose xyz="1 2 3" rpy="0 0 1.57"/>`). serde sees the attribute as a string; these (de)serialize
//! helpers convert to/from fixed-size float arrays.
use serde::{Deserialize, Deserializer, Serializer};

fn parse_n<const N: usize>(s: &str) -> Result<[f64; N], String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() != N {
        return Err(format!(
            "expected {N} numbers, got {} in {s:?}",
            parts.len()
        ));
    }
    let mut out = [0.0; N];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p.parse().map_err(|_| format!("not a number: {p:?}"))?;
    }
    Ok(out)
}

fn fmt_n(v: &[f64]) -> String {
    v.iter()
        .map(|x| {
            // compact: integers without trailing ".0", else default float formatting
            if x.fract() == 0.0 && x.is_finite() {
                format!("{}", *x as i64)
            } else {
                format!("{x}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// (de)serialize an OPTIONAL 3-vector attribute ("x y z"), preserving attribute presence: a `None`
/// is omitted on serialize and read from an absent attribute on deserialize. Used by `Pose` so an
/// `<origin xyz="...">` without an `rpy` round-trips without a fabricated `rpy="0 0 0"`.
pub mod opt_vec3 {
    use super::*;
    pub fn serialize<S: Serializer>(v: &Option<[f64; 3]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(a) => s.serialize_str(&fmt_n(a)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[f64; 3]>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => Ok(Some(parse_n::<3>(&s).map_err(serde::de::Error::custom)?)),
            None => Ok(None),
        }
    }
}

/// (de)serialize an optional quaternion attribute ("x y z w").
pub mod opt_quat {
    use super::*;
    pub fn serialize<S: Serializer>(v: &Option<[f64; 4]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(q) => s.serialize_str(&fmt_n(q)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[f64; 4]>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => Ok(Some(parse_n::<4>(&s).map_err(serde::de::Error::custom)?)),
            None => Ok(None),
        }
    }
}
