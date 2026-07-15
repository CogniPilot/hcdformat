#!/usr/bin/env bash
# Regenerate the checked-in `#[pydom]` DOM mirrors from the canonical Rust model AST.
#
# `hcdformat-domgen` parses `hcdformat-rs/src/model/*.rs` with `syn` and
# emits one generated module for the HCDF owner and one for the stream-profile sidecar owner.
# Shared model types receive sidecar-specific Python handle names so both owners can be registered.
# Struct-valued choice arms receive deterministic draft constructors.
# CI runs this then
# `git diff --exit-code` on the output (the same regenerate-and-diff drift guard the XSD-driven Rust
# enums and the Python model/schema/spec use), so the mirrors can never silently diverge from the model.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cargo run --quiet --manifest-path "$DIR/rust/hcdformat-domgen/Cargo.toml" -- \
  "$DIR/rust/hcdformat-rs/src/model" \
  "$DIR/rust/hcdformat-py/src/dom_generated.rs" \
  "Hcdf"
cargo run --quiet --manifest-path "$DIR/rust/hcdformat-domgen/Cargo.toml" -- \
  "$DIR/rust/hcdformat-rs/src/model" \
  "$DIR/rust/hcdformat-py/src/stream_profile_dom_generated.rs" \
  "StreamProfileDocument" \
  "-" \
  "Hcdf"
echo "regenerated Rust DOM owner modules"
