//! Regenerate the shipped `.pyi` stub tree for the `hcdf` extension package.
//!
//! The extension ships as `hcdf/__init__.abi3.so`, so the type stubs are a PEP 561 tree beside it:
//! `hcdf/__init__.pyi` (the top-level entries) plus one file per public submodule. This bin writes them
//! all from the pyo3-free text builders in the binding crate, so the stubs never drift from the surface.
//! The `pyi_regen_is_a_no_op` test byte-checks the committed files against the same builders.
//!
//! Run:  cargo run --manifest-path rust/hcdformat-py/Cargo.toml --bin gen_pyi

use std::path::Path;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("hcdf");
    std::fs::create_dir_all(&dir).expect("create the hcdf/ stub directory");
    for (name, text) in hcdf::pyi_files() {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        println!("wrote {}", path.display());
    }
}
