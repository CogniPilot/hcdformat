//! CLI-level guarantees that `--xacro NAME:=VALUE` overrides reach xacro expansion (feature `cli`,
//! native-only), driving the REAL `hcdf` binary. A `.xacro` whose `<xacro:arg>` default names a link
//! provably differs with and without the override on BOTH the `convert` and `expand` paths, while
//! omitting `--xacro` reproduces the default expansion.
#![cfg(feature = "cli")]

use std::path::PathBuf;
use std::process::{Command, Output};

/// A `.xacro` whose one link name is driven by `$(arg side)` (default `left`), so a `side:=right`
/// override changes the emitted name.
const ROBOT_XACRO: &str = r#"<?xml version="1.0"?>
<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="argbot">
  <xacro:arg name="side" default="left"/>
  <link name="$(arg side)_wheel"/>
</robot>
"#;

/// A fresh per-test temp dir seeded with `robot.xacro`; returns its root.
fn fixture_dir(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hcdf_cli_xacro_{tag}_{}_{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("robot.xacro"), ROBOT_XACRO).unwrap();
    dir
}

/// Run the real `hcdf` binary with `args` and return its output.
fn hcdf(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hcdf"))
        .args(args)
        .output()
        .expect("hcdf binary runs")
}

#[test]
fn convert_threads_xacro_override_into_the_imported_model() {
    let dir = fixture_dir("convert");
    let xacro = dir.join("robot.xacro");
    let xacro = xacro.to_str().unwrap();

    // Default (no `--xacro`): the arg keeps its declared `left` default.
    let default_out = dir.join("default.hcdf");
    let out = hcdf(&["convert", xacro, default_out.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "default convert must succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let default_doc = std::fs::read_to_string(&default_out).unwrap();
    assert!(
        default_doc.contains("left_wheel"),
        "default arg not applied:\n{default_doc}"
    );
    assert!(
        !default_doc.contains("right_wheel"),
        "override leaked without a flag:\n{default_doc}"
    );

    // `--xacro side:=right` overrides the default, so the imported comp is `right_wheel`.
    let override_out = dir.join("override.hcdf");
    let out = hcdf(&[
        "convert",
        xacro,
        override_out.to_str().unwrap(),
        "--xacro",
        "side:=right",
    ]);
    assert!(
        out.status.success(),
        "override convert must succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let override_doc = std::fs::read_to_string(&override_out).unwrap();
    assert!(
        override_doc.contains("right_wheel"),
        "override not applied:\n{override_doc}"
    );
    assert!(
        !override_doc.contains("left_wheel"),
        "default leaked past override:\n{override_doc}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn expand_threads_xacro_override_into_the_urdf() {
    let dir = fixture_dir("expand");
    let xacro = dir.join("robot.xacro");
    let xacro = xacro.to_str().unwrap();

    // Default (no `--xacro`): the expanded URDF carries the `left` default.
    let out = hcdf(&["expand", xacro, "-"]);
    assert!(
        out.status.success(),
        "default expand must succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let default_urdf = String::from_utf8_lossy(&out.stdout);
    assert!(
        default_urdf.contains("left_wheel"),
        "default arg not applied:\n{default_urdf}"
    );
    assert!(
        !default_urdf.contains("right_wheel"),
        "override leaked without a flag:\n{default_urdf}"
    );

    // `--xacro side:=right` overrides the default in the expanded URDF.
    let out = hcdf(&["expand", xacro, "-", "--xacro", "side:=right"]);
    assert!(
        out.status.success(),
        "override expand must succeed, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let override_urdf = String::from_utf8_lossy(&out.stdout);
    assert!(
        override_urdf.contains("right_wheel"),
        "override not applied:\n{override_urdf}"
    );
    assert!(
        !override_urdf.contains("left_wheel"),
        "default leaked past override:\n{override_urdf}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
