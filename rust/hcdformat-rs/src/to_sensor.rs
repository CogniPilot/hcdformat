//! Shared HCDF -> SDFormat `<sensor>` emission (compiled with feature `urdf` or `sdf`).
//!
//! The inverse of the typed-sensor re-root in [`crate::from_sdf::map_sensor`] / the URDF-Gazebo front
//! end: turn a typed HCDF [`Sensor`] back into the SDFormat sensor grammar both importers parse. BOTH
//! exporters reuse this: [`crate::to_sdf`] emits the `<sensor>` directly INSIDE its `<link>`, and
//! [`crate::to_urdf`] wraps the per-comp sensors in a `<gazebo reference="link">` block (Gazebo reuses
//! the SDFormat sensor grammar). This closes the HCDF -> SDF/URDF sensor round-trip.
//!
//! Category -> SDF `@type` map (the inverse of the importer dispatch): `inertial` -> `imu`,
//! `optical/camera|thermal|tof` -> `camera|thermal|depth_camera`, `optical/lidar` -> `gpu_lidar` (with
//! `<scan>`/`<range>`), `em/mag` -> `magnetometer`, `rf/gnss` -> `navsat`, `fluid/barometer` ->
//! `air_pressure`, `fluid/airspeed` -> `air_speed`, `force/pressure` -> `contact`, `force/torque` ->
//! `force_torque`. Only the hardware-modeling fields the importer typed have an SDF home; the remaining
//! design-spec sub-fields (camera-params extras, gnss/radar params, calibration, IMU FIFO/ODR, …) have
//! no SDFormat sensor element, so they are recorded in the [`LossManifest`], never silently dropped.
use crate::compose::pose_math::{mat3_vec, mul33, transpose};
use crate::compose::{matrix_to_pose, pose_to_matrix};
use crate::error::{Error, Result};
use crate::model::enums::OpticalSensorType;
use crate::model::{
    AxisNoise, FluidSensor, InertialSensor, Noise, OpticalSensor, Pose, Sensor, SensorCategory,
    SensorParams,
};
use crate::to_urdf::LossManifest;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;
use std::io::Cursor;

type Xw = Writer<Cursor<Vec<u8>>>;

// ── compact float text (mirrors the exporters' private fmt1/fmt3) ────────────────────────────────
fn fmt1(x: f64) -> String {
    if x.fract() == 0.0 && x.is_finite() {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}
fn fmt3(v: [f64; 3]) -> String {
    v.iter().map(|x| fmt1(*x)).collect::<Vec<_>>().join(" ")
}

/// An HCDF [`Pose`] -> an SDFormat `<pose>` six-tuple `x y z r p y` (quat -> rpy, FRD -> FLU when `c`
/// is the change-of-basis). Mirrors [`crate::to_sdf`]'s `pose_text` so a sensor pose converts exactly
/// like a link/joint pose.
fn pose_six_tuple(pose: &Pose, c: Option<&[[f64; 3]; 3]>) -> String {
    if c.is_some() || pose.quat.is_some() {
        let m = pose_to_matrix(pose);
        let r = [
            [m[0][0], m[0][1], m[0][2]],
            [m[1][0], m[1][1], m[1][2]],
            [m[2][0], m[2][1], m[2][2]],
        ];
        let t = [m[0][3], m[1][3], m[2][3]];
        let (rp, tp) = match c {
            Some(c) => {
                let ct = transpose(c);
                (mul33(&mul33(c, &r), &ct), mat3_vec(c, t))
            }
            None => (r, t),
        };
        let mut m2 = [[0.0; 4]; 4];
        for i in 0..3 {
            m2[i][..3].copy_from_slice(&rp[i]);
            m2[i][3] = tp[i];
        }
        m2[3][3] = 1.0;
        let out = matrix_to_pose(&m2);
        format!("{} {}", fmt3(out.xyz_or_zero()), fmt3(out.rpy_or_zero()))
    } else {
        format!(
            "{} {}",
            pose.xyz.map(fmt3).unwrap_or_else(|| "0 0 0".to_string()),
            pose.rpy.map(fmt3).unwrap_or_else(|| "0 0 0".to_string()),
        )
    }
}

// ── XML writer helpers (SDF leaves are element TEXT) ─────────────────────────────────────────────
fn w_start(w: &mut Xw, name: &str, attrs: &[(&str, &str)]) -> Result<()> {
    let mut e = BytesStart::new(name.to_string());
    for (k, v) in attrs {
        e.push_attribute((*k, *v));
    }
    w.write_event(Event::Start(e))
        .map_err(|e| Error::Xml(e.to_string()))
}
fn w_end(w: &mut Xw, name: &str) -> Result<()> {
    w.write_event(Event::End(BytesEnd::new(name.to_string())))
        .map_err(|e| Error::Xml(e.to_string()))
}
/// A value-bearing leaf, emitted ONLY when `value` is `Some` (an absent scalar is omitted, not blanked).
fn w_leaf(w: &mut Xw, name: &str, value: Option<&str>) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    w_start(w, name, &[])?;
    w.write_event(Event::Text(BytesText::new(value)))
        .map_err(|e| Error::Xml(e.to_string()))?;
    w_end(w, name)
}

/// `<noise>` with `<type>`/`<mean>`/`<stddev>`/`<bias_mean>`/`<bias_stddev>`/`<precision>`: the SDF
/// form [`crate::from_sdf`]'s `sdf_noise` reads back.
fn write_noise(w: &mut Xw, n: &Noise) -> Result<()> {
    w_start(w, "noise", &[])?;
    if let Some(t) = &n.type_ {
        w_leaf(w, "type", Some(&t.to_string()))?;
    }
    w_leaf(w, "mean", n.mean.as_deref())?;
    w_leaf(w, "stddev", n.stddev.as_deref())?;
    w_leaf(w, "bias_mean", n.bias_mean.as_deref())?;
    w_leaf(w, "bias_stddev", n.bias_stddev.as_deref())?;
    w_leaf(w, "precision", n.precision.as_deref())?;
    // The dynamic (Gauss-Markov) bias leaves round-trip back to the SDF noise.
    w_leaf(w, "dynamic_bias_stddev", n.dynamic_bias_stddev.as_deref())?;
    w_leaf(
        w,
        "dynamic_bias_correlation_time",
        n.dynamic_bias_correlation_time.as_deref(),
    )?;
    w_end(w, "noise")
}

/// `<x><noise>…</noise></x>`: a single representative axis carrying a collapsed [`Noise`], the shape
/// the importer's `axis_noise_collapsed` reads (x-axis representative).
fn write_axis_noise(w: &mut Xw, n: &Noise) -> Result<()> {
    w_start(w, "x", &[])?;
    write_noise(w, n)?;
    w_end(w, "x")
}

/// A `<force>`/`<torque>`/`<linear_acceleration>`/`<angular_velocity>` wrapper carrying per-axis
/// `<x|y|z><noise>` from an [`AxisNoise`]: the shape the importer reads back per axis.
fn write_axis_noise_channel(w: &mut Xw, tag: &str, an: &AxisNoise) -> Result<()> {
    w_start(w, tag, &[])?;
    for (axname, n) in [("x", &an.x), ("y", &an.y), ("z", &an.z)] {
        if let Some(n) = n {
            w_start(w, axname, &[])?;
            write_noise(w, n)?;
            w_end(w, axname)?;
        }
    }
    w_end(w, tag)
}

/// Emit an IMU channel (`linear_acceleration`/`angular_velocity`): per-axis `<x|y|z><noise>` when the
/// channel carries anisotropic per-axis noise, else the scalar `<noise>` on the x axis (fallback).
fn write_imu_channel(w: &mut Xw, tag: &str, p: &SensorParams) -> Result<()> {
    if p.noise.is_none() && p.axis_noise.is_none() {
        return Ok(());
    }
    if let Some(an) = &p.axis_noise {
        write_axis_noise_channel(w, tag, an)
    } else if let Some(n) = &p.noise {
        w_start(w, tag, &[])?;
        write_axis_noise(w, n)?;
        w_end(w, tag)
    } else {
        Ok(())
    }
}

/// Open `<sensor name= type=>`, then its `<pose>` (when present) and `<update_rate>`. The caller emits
/// the typed payload and closes with `</sensor>`.
fn open_sensor(
    w: &mut Xw,
    name: Option<&str>,
    stype: &str,
    pose: Option<&Pose>,
    update_rate: Option<&str>,
    c: Option<&[[f64; 3]; 3]>,
) -> Result<()> {
    let mut attrs: Vec<(&str, &str)> = Vec::new();
    if let Some(n) = name.filter(|s| !s.is_empty()) {
        attrs.push(("name", n));
    }
    attrs.push(("type", stype));
    w_start(w, "sensor", &attrs)?;
    if let Some(p) = pose {
        w_leaf(w, "pose", Some(&pose_six_tuple(p, c)))?;
    }
    w_leaf(w, "update_rate", update_rate)?;
    Ok(())
}

/// Emit ONE typed HCDF [`Sensor`] as an SDFormat `<sensor>` element. The first populated category wins
/// (an import-produced sensor has exactly one); extra categories/elements are recorded as a loss.
pub(crate) fn write_sensor(
    w: &mut Xw,
    sensor: &Sensor,
    ctx: &str,
    c: Option<&[[f64; 3]; 3]>,
    loss: &mut LossManifest,
    topic: Option<&str>,
) -> Result<()> {
    let sctx = match sensor.name.as_deref() {
        Some(n) if !n.is_empty() => format!("{ctx} sensor '{n}'"),
        _ => format!("{ctx} sensor"),
    };
    note_extra_categories(sensor, &sctx, loss);
    if let Some(imu) = sensor.inertial.first() {
        write_imu(w, sensor, imu, &sctx, c, loss)?;
    } else if let Some(opt) = sensor.optical.first() {
        write_optical(w, sensor, opt, &sctx, c, loss)?;
    } else if let Some(em) = sensor.em.first() {
        write_mag(w, sensor, em, &sctx, c, loss)?;
    } else if let Some(rf) = sensor.rf.first() {
        write_gnss(w, sensor, rf, &sctx, c, loss)?;
    } else if let Some(fl) = sensor.fluid.first() {
        write_fluid(w, sensor, fl, &sctx, c, loss)?;
    } else if let Some(fo) = sensor.force.first() {
        write_force(w, sensor, fo, &sctx, c, loss, topic)?;
    } else {
        loss.add(
            "sensor",
            format!("{sctx}: no SDFormat-mappable sensor category; dropped"),
        );
    }
    Ok(())
}

/// Record the categories/elements NOT emitted (only the first populated category maps to one SDF
/// `<sensor>`), plus the sensor-driver, so nothing beyond the emitted one goes silently.
fn note_extra_categories(sensor: &Sensor, sctx: &str, loss: &mut LossManifest) {
    let total = sensor.inertial.len()
        + sensor.optical.len()
        + sensor.em.len()
        + sensor.rf.len()
        + sensor.fluid.len()
        + sensor.force.len();
    let unmappable = sensor.chemical.len()
        + sensor.encoder.len()
        + sensor.temperature.len()
        + sensor.radiation.len()
        + sensor.audio.len()
        + sensor.tactile.len();
    if total > 1 {
        loss.add(
            "sensor",
            format!("{sctx}: {} extra sensor categor(y/ies) not emitted (one <sensor> carries one type)", total - 1),
        );
    }
    if unmappable > 0 {
        loss.add(
            "sensor",
            format!("{sctx}: {unmappable} chemical/encoder/temperature/radiation/audio/tactile categor(y/ies) have no SDF sensor type; dropped"),
        );
    }
}

/// `inertial` -> `<sensor type="imu">`: the accel/gyro collapsed `<noise>` back onto the SDF IMU
/// channels. FIFO/ODR/bandwidth/range/resolution have no SDF `<imu>` home (recorded).
fn write_imu(
    w: &mut Xw,
    sensor: &Sensor,
    imu: &InertialSensor,
    sctx: &str,
    c: Option<&[[f64; 3]; 3]>,
    loss: &mut LossManifest,
) -> Result<()> {
    open_sensor(
        w,
        sensor.name.as_deref(),
        "imu",
        imu.pose.as_ref(),
        sensor.update_rate.as_deref(),
        c,
    )?;
    let has_noise =
        |p: Option<&SensorParams>| p.is_some_and(|p| p.noise.is_some() || p.axis_noise.is_some());
    if has_noise(imu.accel.as_ref()) || has_noise(imu.gyro.as_ref()) {
        w_start(w, "imu", &[])?;
        if let Some(p) = &imu.accel {
            write_imu_channel(w, "linear_acceleration", p)?;
        }
        if let Some(p) = &imu.gyro {
            write_imu_channel(w, "angular_velocity", p)?;
        }
        w_end(w, "imu")?;
    }
    for (params, label) in [(imu.accel.as_ref(), "accel"), (imu.gyro.as_ref(), "gyro")] {
        if let Some(p) = params {
            if p.range.is_some()
                || p.resolution.is_some()
                || p.odr.is_some()
                || p.bandwidth.is_some()
                || p.fifo.is_some()
            {
                loss.add(
                    "sensor",
                    format!("{sctx}: IMU <{label}> range/resolution/odr/bandwidth/fifo have no SDF <imu> field; dropped"),
                );
            }
        }
    }
    w_end(w, "sensor")
}

/// `optical` -> `<sensor type="camera|thermal|depth_camera|gpu_lidar">` (lidar routes to its own body).
fn write_optical(
    w: &mut Xw,
    sensor: &Sensor,
    opt: &OpticalSensor,
    sctx: &str,
    c: Option<&[[f64; 3]; 3]>,
    loss: &mut LossManifest,
) -> Result<()> {
    if opt.type_ == Some(OpticalSensorType::Lidar) {
        return write_lidar(w, sensor, opt, sctx, c, loss);
    }
    let stype = match opt.type_ {
        Some(OpticalSensorType::Thermal) => "thermal",
        Some(OpticalSensorType::Tof) => "depth_camera",
        Some(OpticalSensorType::OpticalFlow) => {
            loss.add(
                "sensor",
                format!("{sctx}: optical_flow has no SDF sensor type; exported as <camera>"),
            );
            "camera"
        }
        _ => "camera",
    };
    open_sensor(
        w,
        sensor.name.as_deref(),
        stype,
        opt.pose.as_ref(),
        sensor.update_rate.as_deref(),
        c,
    )?;
    write_camera_body(w, opt, sctx, loss)?;
    w_end(w, "sensor")
}

/// The `<camera>` body from the first `<fov>`: `<horizontal_fov>`/`<image>`/`<clip>` (frustum),
/// `<distortion>`, and `<lens>` (projection model + pinhole `<intrinsics>`).
fn write_camera_body(
    w: &mut Xw,
    opt: &OpticalSensor,
    sctx: &str,
    loss: &mut LossManifest,
) -> Result<()> {
    let Some(fov) = opt.fov.first() else {
        return Ok(());
    };
    if opt.fov.len() > 1 {
        loss.add(
            "sensor",
            format!(
                "{sctx}: {} extra <fov>(s) not emitted (SDF <camera> is single-view)",
                opt.fov.len() - 1
            ),
        );
    }
    w_start(w, "camera", &[])?;
    let frustum = fov.geometry.as_ref().and_then(|g| g.frustum.as_ref());
    if let Some(fr) = frustum {
        w_leaf(w, "horizontal_fov", fr.hfov.as_deref())?;
    }
    if let Some(intr) = &fov.intrinsics {
        if intr.width.is_some() || intr.height.is_some() || intr.format.is_some() {
            w_start(w, "image", &[])?;
            w_leaf(w, "width", intr.width.as_deref())?;
            w_leaf(w, "height", intr.height.as_deref())?;
            w_leaf(w, "format", intr.format.as_deref())?;
            w_end(w, "image")?;
        }
    }
    if let Some(fr) = frustum {
        if fr.near.is_some() || fr.far.is_some() {
            w_start(w, "clip", &[])?;
            w_leaf(w, "near", fr.near.as_deref())?;
            w_leaf(w, "far", fr.far.as_deref())?;
            w_end(w, "clip")?;
        }
    }
    if let Some(d) = &fov.distortion {
        w_start(w, "distortion", &[])?;
        w_leaf(w, "k1", d.k1.as_deref())?;
        w_leaf(w, "k2", d.k2.as_deref())?;
        w_leaf(w, "k3", d.k3.as_deref())?;
        w_leaf(w, "p1", d.p1.as_deref())?;
        w_leaf(w, "p2", d.p2.as_deref())?;
        w_end(w, "distortion")?;
    }
    write_camera_lens(w, fov)?;
    if opt.camera_params.is_some() {
        loss.add("sensor", format!("{sctx}: <camera-params> (shutter/hdr/compression/thermal/…) has no SDF <camera> field; dropped"));
    }
    // The projection matrix P rides <lens><projection>; the rest of the ROS CameraInfo geometry has
    // no SDFormat <camera> home, so it is recorded rather than silently dropped.
    if fov
        .camera_matrix
        .as_ref()
        .is_some_and(|cm| cm.rectification.is_some() || cm.binning.is_some() || cm.roi.is_some())
    {
        loss.add("sensor", format!("{sctx}: <camera-matrix> rectification/binning/roi have no SDF <camera> field; dropped"));
    }
    if fov
        .distortion
        .as_ref()
        .is_some_and(|d| d.k4.is_some() || d.k5.is_some() || d.k6.is_some() || d.model.is_some())
    {
        loss.add("sensor", format!("{sctx}: distortion k4/k5/k6/@model (ROS rational_polynomial/equidistant) have no SDF <distortion> field; dropped"));
    }
    w_end(w, "camera")
}

/// `<lens>` combining the lens projection model ([`crate::model::CameraLens`]), the pinhole
/// `<intrinsics>` (fx/fy/cx/cy/s), and the ROS CameraInfo projection matrix P (`<projection>`),
/// emitted only when at least one carries data.
fn write_camera_lens(w: &mut Xw, fov: &crate::model::SensorFov) -> Result<()> {
    let lens = fov.lens.as_ref();
    let intr = fov.intrinsics.as_ref();
    let has_intr = intr.is_some_and(|i| {
        i.fx.is_some() || i.fy.is_some() || i.cx.is_some() || i.cy.is_some() || i.s.is_some()
    });
    let has_lens = lens.is_some_and(|l| {
        l.type_.is_some()
            || l.cutoff_angle.is_some()
            || l.scale_to_hfov.is_some()
            || l.custom_function.is_some()
    });
    let proj = fov
        .camera_matrix
        .as_ref()
        .and_then(|cm| cm.projection.as_ref());
    let has_proj = proj.is_some_and(|p| {
        p.fx.is_some()
            || p.fy.is_some()
            || p.cx.is_some()
            || p.cy.is_some()
            || p.tx.is_some()
            || p.ty.is_some()
    });
    if !has_intr && !has_lens && !has_proj {
        return Ok(());
    }
    w_start(w, "lens", &[])?;
    if let Some(l) = lens.filter(|_| has_lens) {
        w_leaf(w, "type", l.type_.as_deref())?;
        w_leaf(w, "scale_to_hfov", l.scale_to_hfov.as_deref())?;
        if let Some(ca) = &l.cutoff_angle {
            w_leaf(w, "cutoff_angle", ca.value.as_deref())?;
        }
        if let Some(cf) = &l.custom_function {
            w_start(w, "custom_function", &[])?;
            w_leaf(w, "c1", cf.c1.as_deref())?;
            w_leaf(w, "c2", cf.c2.as_deref())?;
            w_leaf(w, "c3", cf.c3.as_deref())?;
            w_leaf(w, "f", cf.f.as_deref())?;
            w_leaf(w, "fun", cf.fun.as_deref())?;
            w_end(w, "custom_function")?;
        }
    }
    if let Some(i) = intr.filter(|_| has_intr) {
        w_start(w, "intrinsics", &[])?;
        w_leaf(w, "fx", i.fx.as_deref())?;
        w_leaf(w, "fy", i.fy.as_deref())?;
        w_leaf(w, "cx", i.cx.as_deref())?;
        w_leaf(w, "cy", i.cy.as_deref())?;
        w_leaf(w, "s", i.s.as_deref())?;
        w_end(w, "intrinsics")?;
    }
    // The projection matrix P (SDF `p_fx/p_fy/p_cx/p_cy/tx/ty`).
    if let Some(p) = proj.filter(|_| has_proj) {
        w_start(w, "projection", &[])?;
        w_leaf(w, "p_fx", p.fx.as_deref())?;
        w_leaf(w, "p_fy", p.fy.as_deref())?;
        w_leaf(w, "p_cx", p.cx.as_deref())?;
        w_leaf(w, "p_cy", p.cy.as_deref())?;
        w_leaf(w, "tx", p.tx.as_deref())?;
        w_leaf(w, "ty", p.ty.as_deref())?;
        w_end(w, "projection")?;
    }
    w_end(w, "lens")
}

/// `optical/lidar` -> `<sensor type="gpu_lidar"><lidar>` with the angular `<scan>` and along-beam
/// `<range>` (the schema's lossless lidar target).
fn write_lidar(
    w: &mut Xw,
    sensor: &Sensor,
    opt: &OpticalSensor,
    sctx: &str,
    c: Option<&[[f64; 3]; 3]>,
    loss: &mut LossManifest,
) -> Result<()> {
    open_sensor(
        w,
        sensor.name.as_deref(),
        "gpu_lidar",
        opt.pose.as_ref(),
        sensor.update_rate.as_deref(),
        c,
    )?;
    if let Some(lp) = &opt.lidar_params {
        w_start(w, "lidar", &[])?;
        if let Some(sp) = &lp.scan_pattern {
            w_start(w, "scan", &[])?;
            if let Some(h) = &sp.horizontal {
                write_scan_axis(w, "horizontal", h)?;
            }
            if let Some(v) = &sp.vertical {
                write_scan_axis(w, "vertical", v)?;
            }
            w_end(w, "scan")?;
        }
        if let Some(r) = &lp.range {
            w_start(w, "range", &[])?;
            w_leaf(w, "min", r.min.as_ref().and_then(|m| m.value.as_deref()))?;
            w_leaf(w, "max", r.max.as_ref().and_then(|m| m.value.as_deref()))?;
            w_leaf(
                w,
                "resolution",
                r.resolution.as_ref().and_then(|m| m.value.as_deref()),
            )?;
            w_end(w, "range")?;
        }
        // The beam <noise> (SDF lidar order is scan, range, noise).
        if let Some(n) = &lp.noise {
            write_noise(w, n)?;
        }
        w_end(w, "lidar")?;
        let extra = lp.scan_type.is_some()
            || lp.channels.is_some()
            || lp.points_per_second.is_some()
            || lp.scan_rate.is_some()
            || lp.returns.is_some()
            || lp.wavelength.is_some()
            || lp.horizontal_fov.is_some()
            || lp.vertical_fov.is_some();
        if extra {
            loss.add("sensor", format!("{sctx}: <lidar-params> scan-type/channels/points-per-second/scan-rate/returns/wavelength/fov have no SDF <lidar> field; dropped"));
        }
    }
    w_end(w, "sensor")
}

/// One `<horizontal>`/`<vertical>` scan axis: samples/resolution/min_angle/max_angle child leaves.
fn write_scan_axis(w: &mut Xw, name: &str, ax: &crate::model::LidarScanAxis) -> Result<()> {
    w_start(w, name, &[])?;
    w_leaf(w, "samples", ax.samples.as_deref())?;
    w_leaf(w, "resolution", ax.resolution.as_deref())?;
    w_leaf(w, "min_angle", ax.min_angle.as_deref())?;
    w_leaf(w, "max_angle", ax.max_angle.as_deref())?;
    w_end(w, name)
}

/// `em/mag` -> `<sensor type="magnetometer">`: the collapsed field `<noise>` onto the x axis.
fn write_mag(
    w: &mut Xw,
    sensor: &Sensor,
    em: &SensorCategory,
    sctx: &str,
    c: Option<&[[f64; 3]; 3]>,
    loss: &mut LossManifest,
) -> Result<()> {
    if em.type_.as_deref() != Some("mag") {
        loss.add(
            "sensor",
            format!(
                "{sctx}: <em type='{}'> has no SDF sensor type; dropped",
                em.type_.as_deref().unwrap_or("")
            ),
        );
        return Ok(());
    }
    open_sensor(
        w,
        sensor.name.as_deref(),
        "magnetometer",
        em.pose.as_ref(),
        sensor.update_rate.as_deref(),
        c,
    )?;
    if let Some(n) = &em.noise {
        w_start(w, "magnetometer", &[])?;
        write_axis_noise(w, n)?;
        w_end(w, "magnetometer")?;
    }
    w_end(w, "sensor")
}

/// `rf/gnss` -> `<sensor type="navsat">`: the position `<noise>` onto position_sensing/horizontal.
fn write_gnss(
    w: &mut Xw,
    sensor: &Sensor,
    rf: &SensorCategory,
    sctx: &str,
    c: Option<&[[f64; 3]; 3]>,
    loss: &mut LossManifest,
) -> Result<()> {
    if rf.type_.as_deref() != Some("gnss") {
        loss.add(
            "sensor",
            format!(
                "{sctx}: <rf type='{}'> has no SDF sensor type; dropped",
                rf.type_.as_deref().unwrap_or("")
            ),
        );
        return Ok(());
    }
    open_sensor(
        w,
        sensor.name.as_deref(),
        "navsat",
        rf.pose.as_ref(),
        sensor.update_rate.as_deref(),
        c,
    )?;
    if let Some(n) = &rf.noise {
        w_start(w, "navsat", &[])?;
        w_start(w, "position_sensing", &[])?;
        w_start(w, "horizontal", &[])?;
        write_noise(w, n)?;
        w_end(w, "horizontal")?;
        w_end(w, "position_sensing")?;
        w_end(w, "navsat")?;
    }
    if rf.gnss_params.is_some() {
        loss.add("sensor", format!("{sctx}: <gnss-params> (constellation/rtk/accuracy/…) has no SDF <navsat> field; dropped"));
    }
    w_end(w, "sensor")
}

/// `fluid/barometer` -> `<sensor type="air_pressure">`, `fluid/airspeed` -> `<sensor type="air_speed">`;
/// the pressure `<noise>` rides `<…><pressure><noise>`. Other fluid sub-types have no SDF sensor.
fn write_fluid(
    w: &mut Xw,
    sensor: &Sensor,
    fl: &FluidSensor,
    sctx: &str,
    c: Option<&[[f64; 3]; 3]>,
    loss: &mut LossManifest,
) -> Result<()> {
    let (stype, payload) = match fl.type_.as_deref() {
        Some("barometer") | Some("altimeter") => ("air_pressure", "air_pressure"),
        Some("airspeed") | Some("pitot") => ("air_speed", "air_speed"),
        other => {
            loss.add(
                "sensor",
                format!(
                    "{sctx}: <fluid type='{}'> has no SDF sensor type; dropped",
                    other.unwrap_or("")
                ),
            );
            return Ok(());
        }
    };
    open_sensor(
        w,
        sensor.name.as_deref(),
        stype,
        fl.pose.as_ref(),
        sensor.update_rate.as_deref(),
        c,
    )?;
    if let Some(n) = &fl.noise {
        w_start(w, payload, &[])?;
        w_start(w, "pressure", &[])?;
        write_noise(w, n)?;
        w_end(w, "pressure")?;
        w_end(w, payload)?;
    }
    if fl.pressure_range.is_some() || fl.reference_pressure.is_some() || fl.probe.is_some() {
        loss.add("sensor", format!("{sctx}: <fluid> pressure-range/reference/probe design-spec fields have no SDF sensor field; dropped"));
    }
    w_end(w, "sensor")
}

/// `force/pressure` -> `<sensor type="contact">`, `force/torque` -> `<sensor type="force_torque">`
/// (the reporting frame/measure-direction + per-axis force/torque noise, else the scalar on torque/x).
/// Other force sub-types have no SDF sensor.
fn write_force(
    w: &mut Xw,
    sensor: &Sensor,
    fo: &SensorCategory,
    sctx: &str,
    c: Option<&[[f64; 3]; 3]>,
    loss: &mut LossManifest,
    topic: Option<&str>,
) -> Result<()> {
    match fo.type_.as_deref() {
        Some("pressure") => {
            open_sensor(
                w,
                sensor.name.as_deref(),
                "contact",
                fo.pose.as_ref(),
                sensor.update_rate.as_deref(),
                c,
            )?;
            // An SDF contact sensor REQUIRES a <contact><collision>+<topic> body (contact.xsd
            // minOccurs=1); emitting a bare <sensor type="contact"> is invalid SDF (the old dead-sensor
            // bug). Emit the body when the sensor names the collision it monitors. The topic comes from
            // the org.ros2 topic-map extension; if absent, synthesize a default from the sensor name so
            // the SDF stays valid, and record it.
            if let Some(collision) = fo.collision.as_deref().filter(|s| !s.is_empty()) {
                let topic = topic.map(str::to_string).unwrap_or_else(|| {
                    let t = format!("/{}/contact", sensor.name.as_deref().unwrap_or("contact"));
                    loss.add(
                        "sensor",
                        format!("{sctx}: contact sensor had no org.ros2 topic; synthesized {t} to satisfy contact.xsd"),
                    );
                    t
                });
                w_start(w, "contact", &[])?;
                w_leaf(w, "collision", Some(collision))?;
                w_leaf(w, "topic", Some(&topic))?;
                w_end(w, "contact")?;
            } else {
                loss.add(
                    "sensor",
                    format!("{sctx}: <force type='pressure'> has no monitored <collision>; emitted <sensor type='contact'> lacks the required <contact> body"),
                );
            }
            w_end(w, "sensor")
        }
        Some("torque") | Some("force_torque") => {
            open_sensor(
                w,
                sensor.name.as_deref(),
                "force_torque",
                fo.pose.as_ref(),
                sensor.update_rate.as_deref(),
                c,
            )?;
            // The reporting @frame/@measure-direction + per-axis force/torque noise (else the scalar
            // <noise> on the torque x-axis representative).
            let has_body = fo.frame.is_some()
                || fo.measure_direction.is_some()
                || fo.axis_noise.is_some()
                || fo.noise.is_some();
            if has_body {
                w_start(w, "force_torque", &[])?;
                if let Some(fr) = &fo.frame {
                    w_leaf(w, "frame", Some(&fr.to_string()))?;
                }
                if let Some(md) = &fo.measure_direction {
                    w_leaf(w, "measure_direction", Some(&md.to_string()))?;
                }
                if let Some(an) = &fo.axis_noise {
                    if let Some(f) = &an.force {
                        write_axis_noise_channel(w, "force", f)?;
                    }
                    if let Some(t) = &an.torque {
                        write_axis_noise_channel(w, "torque", t)?;
                    }
                } else if let Some(n) = &fo.noise {
                    w_start(w, "torque", &[])?;
                    write_axis_noise(w, n)?;
                    w_end(w, "torque")?;
                }
                w_end(w, "force_torque")?;
            }
            w_end(w, "sensor")
        }
        other => {
            loss.add(
                "sensor",
                format!(
                    "{sctx}: <force type='{}'> has no SDF sensor type; dropped",
                    other.unwrap_or("")
                ),
            );
            Ok(())
        }
    }
}
