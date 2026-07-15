//! The `hcdf` CLI binary: a thin entry point over the library's [`hcdformat::cli`] module.
//!
//! The whole CLI implementation moved INTO the library (`src/cli.rs`, feature = `cli`, native-only) so
//! the PyO3 binding's `run_cli` pyfunction can drive the SAME Rust CLI: the Rust binary is therefore the
//! sole `hcdf` CLI on BOTH install paths: the native/colcon build (this bin) and the pip/wheel console
//! script (the binding trampoline). This file just forwards `argv` and maps the returned exit code.
//!
//! Native-only + `required-features = ["cli"]`: the bin never enters the default/wasm build, and on wasm
//! it is a trivial no-op (clap + the bundle/bake/remote wiring the CLI drives are native-only).
#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(hcdformat::cli::run(std::env::args().collect()) as u8)
}

#[cfg(target_arch = "wasm32")]
fn main() {}
