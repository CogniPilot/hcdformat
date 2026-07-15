//! Parity between the `hcdf profile` subcommand and the core render functions (feature `cli`).
//!
//! The renders live in the core now; the CLI and the PyO3 binding both call them, so there is a single
//! renderer. This drives the REAL `hcdf` binary over the in-repo corpus and asserts its stdout equals
//! the core `ProfileReport::markdown()` / `to_json()` output verbatim (the CLI adds exactly the one
//! trailing newline `println!` writes). The corpus deliberately spans documents with out-of-profile
//! drivers, transform findings, validator errors, and field-level losses, so every branch of the
//! renderer is exercised by at least one case.
#![cfg(feature = "cli")]

use hcdformat::{check_profile, Hcdf};
use std::path::{Path, PathBuf};
use std::process::Command;

/// `<repo>` root: the crate sits at `<repo>/rust/hcdformat-rs`.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The loadable corpus: every `.hcdf` under `examples/` and `tests/valid/` (plus the composed/sensor
/// fixtures), which between them cover loops, surfaces, transmissions, sensors, motors, metadata, and
/// clean identity docs. Invalid fixtures are excluded because `hcdf profile` (and the core loader)
/// reject them, which is a different code path than the render under test.
fn corpus() -> Vec<PathBuf> {
    let repo = repo_root();
    let mut out = Vec::new();
    for dir in ["examples", "tests/valid", "tests/hcdf"] {
        let d = repo.join(dir);
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("hcdf") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Run `hcdf profile [--json] <path>` and return its stdout.
fn run_profile(path: &Path, json: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hcdf"));
    cmd.arg("profile");
    if json {
        cmd.arg("--json");
    }
    cmd.arg(path);
    let out = cmd.output().expect("spawn hcdf");
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

#[test]
fn profile_subcommand_output_equals_core_render() {
    let corpus = corpus();
    assert!(
        corpus.len() >= 5,
        "expected a real corpus, found {}",
        corpus.len()
    );
    let mut checked = 0usize;
    for path in &corpus {
        let src = std::fs::read_to_string(path).unwrap();
        // Only compare docs the core loads; a doc it rejects has no report to render (the CLI reports
        // that as a load error on stderr, out of scope here).
        let Ok(doc) = Hcdf::from_xml_str(&src) else {
            continue;
        };
        let report = check_profile(&doc);

        // The CLI prints the render with `println!`, i.e. the core string plus one trailing newline.
        let md_cli = run_profile(path, false);
        assert_eq!(
            md_cli,
            format!("{}\n", report.markdown()),
            "markdown parity mismatch for {}",
            path.display()
        );

        let json_cli = run_profile(path, true);
        assert_eq!(
            json_cli,
            format!("{}\n", report.to_json()),
            "json parity mismatch for {}",
            path.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 5,
        "expected to compare a real corpus, compared {checked}"
    );
}
