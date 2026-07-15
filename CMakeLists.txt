cmake_minimum_required(VERSION 3.16)
project(hcdformat NONE)

# ── ROS 2 / colcon build of the Rust-backed hcdformat package ────────────────────────────────────────
#
# hcdformat is Rust-canonical: the Python API IS the native binding (`import hcdf` loads the extension,
# which ships as `hcdf/__init__.abi3.so`) and the `hcdf` CLI is the RUST binary. So this is an ament_cmake
# package that builds the binding + the CLI FROM SOURCE with a Rust toolchain; it is deliberately NOT an
# ament_python (pure-Python) package any more. Build-machine requirements (developer/CI tools; no
# universal rosdep key across distros, so they are documented here + in the README rather than declared
# as <depend>s in package.xml):
#
#   * a Rust toolchain (cargo + rustc, stable), install via rustup: https://rustup.rs
#   * maturin >= 1.7, < 2, via `pip install 'maturin>=1.7,<2'` (builds the abi3 binding wheel)
#
# What colcon installs:
#   * the abi3 binding wheel (maturin) unpacked into this prefix's python site-packages → `import hcdf`
#   * the native `hcdf` CLI binary into lib/hcdformat → `ros2 run hcdformat hcdf ...`
#   * schema, completion, and connectivity registry data into share/hcdformat

find_package(ament_cmake REQUIRED)
find_package(Python3 REQUIRED COMPONENTS Interpreter)

# The Rust toolchain gate: a clear configure-time error if cargo is missing (see the rustup note above).
find_program(CARGO_EXECUTABLE cargo REQUIRED
  DOC "Rust cargo; install a stable toolchain via rustup (https://rustup.rs)")

set(_repo "${CMAKE_CURRENT_SOURCE_DIR}")
set(_py_manifest "${_repo}/rust/hcdformat-py/Cargo.toml")
set(_rs_manifest "${_repo}/rust/hcdformat-rs/Cargo.toml")
set(_wheel_dir "${CMAKE_CURRENT_BINARY_DIR}/wheel")
set(_cargo_target "${CMAKE_CURRENT_BINARY_DIR}/cargo-target")

# 1) Build the abi3 binding wheel with maturin. maturin compiles the Rust core + PyO3 binding and packs
#    it as `hcdf/__init__.abi3.so` (the extension IS the package) alongside the generated type stubs and
#    the data files, one abi3 wheel with no shipped `.py`.
add_custom_command(
  OUTPUT "${_wheel_dir}/.built"
  COMMAND "${Python3_EXECUTABLE}" -m maturin build --release
          --manifest-path "${_py_manifest}" --out "${_wheel_dir}"
  COMMAND "${CMAKE_COMMAND}" -E touch "${_wheel_dir}/.built"
  COMMENT "hcdformat: building the abi3 Rust binding wheel (maturin)"
  VERBATIM)

# 2) Build the native `hcdf` CLI binary (cargo, feature=cli) so `ros2 run hcdformat hcdf` works. This is
#    the SAME Rust CLI the wheel's console script drives (via the run_cli binding): one implementation.
add_custom_command(
  OUTPUT "${_cargo_target}/release/hcdf"
  COMMAND "${CARGO_EXECUTABLE}" build --release --features cli --bin hcdf
          --manifest-path "${_rs_manifest}" --target-dir "${_cargo_target}"
  COMMENT "hcdformat: building the native hcdf CLI binary (cargo --features cli)"
  VERBATIM)

add_custom_target(hcdformat_build ALL
  DEPENDS "${_wheel_dir}/.built" "${_cargo_target}/release/hcdf")

# 3a) Install the binding: unpack the wheel into this prefix's python site-packages. The wheel has NO
#     runtime deps (the pure-Python that needed lxml/numpy is gone), so --no-deps/--no-index is safe and
#     keeps the install network-free and hermetic.
set(_py_site "lib/python${Python3_VERSION_MAJOR}.${Python3_VERSION_MINOR}/site-packages")
install(CODE "
  file(GLOB _whl \"${_wheel_dir}/*.whl\")
  if(NOT _whl)
    message(FATAL_ERROR \"hcdformat: no wheel found in ${_wheel_dir}; did the maturin build run?\")
  endif()
  list(GET _whl 0 _whl0)
  set(_dest \"\${CMAKE_INSTALL_PREFIX}/${_py_site}\")
  file(MAKE_DIRECTORY \"\${_dest}\")
  message(STATUS \"hcdformat: installing binding wheel \${_whl0} -> \${_dest}\")
  execute_process(
    COMMAND \"${Python3_EXECUTABLE}\" -m pip install --no-deps --no-index --upgrade
            --target \"\${_dest}\" \"\${_whl0}\"
    RESULT_VARIABLE _rc)
  if(NOT _rc EQUAL 0)
    message(FATAL_ERROR \"hcdformat: pip install of the binding wheel failed (rc=\${_rc})\")
  endif()
")

# 3b) Install the native CLI binary and the schema/reference data.
install(PROGRAMS "${_cargo_target}/release/hcdf" DESTINATION lib/${PROJECT_NAME})
install(FILES
  "${_repo}/hcdf.xsd"
  "${_repo}/hcdf-stream-profile.xsd"
  "${_repo}/hcdf.schema.json"
  "${_repo}/hcdf.completions.json"
  DESTINATION share/${PROJECT_NAME})
install(FILES
  "${_repo}/registries/connectivity-profiles.json"
  "${_repo}/registries/connectivity-profile-registry.schema.json"
  DESTINATION share/${PROJECT_NAME}/registries)

ament_package()
