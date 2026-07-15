//! The `hcdf` CLI (feature = `cli`, native-only): a Rust port of `hcdf_cli.py`'s subcommands wired to
//! the existing library functions.
//!
//! Subcommands:
//!   * `convert`: URDF/SDF <-> HCDF (and HCDF<->HCDF passthrough, plus cross pairs like SDF->URDF
//!     through the HCDF hub). SDF input is imported RAW via `from_sdf_str_with_assets`, exactly as the
//!     Python CLI's `convert` routes SDF through `sdf_io.from_sdf` (a tolerant lxml `recover=True` walk
//!     that never shells to gz): a default-omitting / `model://` / unnamed-element SDF still imports.
//!     `--bake DIR` bakes EVERY referenced mesh of a URDF/xacro/SDF import to a canonical
//!     content-addressed asset in DIR: the per-visual scale/mirror/colour from the importers' hint
//!     side-channel (`from_urdf_str_with_assets` / `from_sdf_str_with_assets`) folded into the GLB,
//!     rewriting `@uri`/`@sha` relative to the output document, matching `hcdf_cli.py`'s `assets.Baker`.
//!     Notes + the export loss manifest go to stderr, like the Python CLI.
//!   * `validate`: kinematic/reference validation (plain), or schema-shape validation (`--xsd`), matching
//!     the Python `validate` / `validate --xsd` split (plain validate is kinematic-only, so a missing
//!     required attribute is OK at this layer; it surfaces under `--xsd`). Like Python's lxml path,
//!     `--xsd` validates the RAW input bytes of a `.hcdf` (never a re-serialization, which would launder
//!     constructs the typed parse silently drops, e.g. legacy text-content poses); a load failure
//!     (unsupported `@version`, XML parse error) is reported as a validation FAILURE, not a CLI error.
//!   * `profile`: classify an HCDF document against the HCDF-URDF Profile (markdown or `--json`).
//!   * `bundle` / `bundle-verify`: pack a model + its meshes into a `.hcdfz` / verify a bundle.
//!     `--vendor-remote` fetches+embeds remote assets; `--keep-remote` leaves the live uri.
//!   * `bake`: bake ONE mesh to a canonical content-addressed GLB (visual) / lean STL (collision); a Rust
//!     convenience over the asset baker. (Python exposes baking only via `convert --bake DIR`.)
//!   * `expand`: expand a `.xacro` (or read a `.urdf`) to a plain URDF, resolving `$(find)` via
//!     `--package`; `--abs-meshes` rewrites `package://PKG/...` mesh uris to absolute `file://` paths.
//!     Mirrors `hcdf_cli.py`'s `expand`. A `.xacro` input to `convert` is expanded the same way.
//!   * `regen enums|schema|spec|completions`: regenerate an XSD-derived artifact (dev tool): the typed
//!     Rust enums (`src/model/enums.rs`), the JSON Schema (`hcdf.schema.json`), a spec-browser HTML page
//!     (`spec.html` / `website/spec/*.html`), or the editor completion model (`hcdf.completions.json`).
//!     Pure-Rust ports of the retired `generate_hcdf_rs_enums.py` / `generate_json_schema.py` /
//!     `generate_spec_html.py` / `gen_completions.py`, same `<input.xsd> <output>` contract; the enums /
//!     schema / `spec.html` are byte-identical to a fresh regen (a `cargo test` drift gate).
//!
//! Deliberate deferrals (documented gaps vs `hcdf_cli.py`, NOT bugs):
//!   * `--package` on validate/profile is accepted for CLI parity but inert for a plain `.hcdf`/`.urdf`
//!     input; it resolves `$(find)` only for a `.xacro` input (which `convert`/`expand` accept).
//!
//! Native-only: `required-features = ["cli"]` keeps this bin out of the default build entirely, and the
//! whole binary is additionally `cfg(not(wasm32))`-gated (the bundle/bake/remote wiring it drives is
//! native-only), so even `--features cli --target wasm32` compiles to a trivial no-op rather than
//! pulling clap into a wasm tree.

use crate::bundle::{pack, verify, PackOptions, RemotePolicy};
use crate::import_bake::{bake_document, BakeEnv};
use crate::validate::Level;
use crate::{
    bake_convert, check_profile, from_sdf_str_with_assets, from_urdf_str_with_assets,
    hcdf_xml_to_json, json_to_hcdf_xml, to_sdf, to_urdf, validate_coverage, validate_enums,
    validate_loops, validate_network, validate_semantic, validate_structural, validate_xsd,
    BakeKind, Hcdf, Issue, VisualAssetHint,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "hcdf",
    about = "Convert and check HCDF, URDF, and SDF robot descriptions.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// A supported document format (the `--from`/`--to` choices and extension inference).
#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Format {
    Urdf,
    Sdf,
    Hcdf,
    Json,
}

impl Format {
    fn infer(path: &str) -> Option<Format> {
        match Path::new(path).extension().and_then(|e| e.to_str()) {
            Some("urdf") | Some("xacro") => Some(Format::Urdf),
            Some("sdf") => Some(Format::Sdf),
            Some("hcdf") => Some(Format::Hcdf),
            Some("json") => Some(Format::Json),
            _ => None,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Convert between URDF/SDF and HCDF (and cross pairs through the hub). A `.xacro` input is expanded to
    /// URDF first (`--xacro` args, `--package` for `$(find)`). `--bake DIR` bakes every mesh during import.
    Convert {
        input: String,
        /// Output path, or `-` for stdout (then `--to` is required).
        output: String,
        #[arg(long = "from", value_enum)]
        from_fmt: Option<Format>,
        #[arg(long = "to", value_enum)]
        to_fmt: Option<Format>,
        /// Write the loss manifest as JSON to FILE.
        #[arg(long)]
        loss: Option<String>,
        /// Bake meshes to canonical assets in DIR (scale and mirror folded in): every visual mesh becomes
        /// a content-addressed GLB (the source scale/mirror/flat colour baked into the geometry), every
        /// SCALED collision a lean STL, with `@uri`/`@sha` rewritten relative to the output document.
        /// URDF/xacro/SDF inputs only; inert for an HCDF input (vendor an existing document's meshes
        /// with `hcdf bundle`), exactly like `hcdf_cli.py`.
        #[arg(long, value_name = "DIR")]
        bake: Option<String>,
        /// A xacro argument for a `.xacro` input (repeatable, `NAME:=VALUE`), matching `hcdf_cli.py`.
        /// Each entry overrides the matching `<xacro:arg>` default; an arg not given keeps its default.
        #[arg(long = "xacro", value_name = "NAME:=VALUE")]
        xacro: Vec<String>,
        /// Locate a ROS package for `$(find PKG)` xacro resolution / `package://PKG/...` uris (repeatable,
        /// `PKG=PATH`). Resolves `$(find)` for a `.xacro` input and `package://`/`model://` mesh uris
        /// under `--bake`; for a plain URDF/HCDF input WITHOUT `--bake`, mesh resolution is a bundle-time
        /// concern (`hcdf bundle --package`), so it is inert there.
        #[arg(long = "package", value_name = "PKG=PATH")]
        package: Vec<String>,
    },
    /// Validate the kinematic tree and references (plain), or the schema shape (`--xsd`).
    Validate {
        input: String,
        /// Validate the document's SCHEMA SHAPE against `hcdf.xsd` (uppsala, at parity with Python's
        /// lxml path) instead of the kinematic tree. A native `.hcdf` is validated as its RAW bytes.
        #[arg(long)]
        xsd: bool,
        #[arg(long = "from", value_enum)]
        from_fmt: Option<Format>,
        /// Locate a ROS package for `$(find PKG)` xacro resolution (repeatable, `PKG=PATH`).
        /// Xacro expansion is not yet wired in here, so this is accepted but currently only relevant once xacro lands.
        #[arg(long = "package", value_name = "PKG=PATH")]
        package: Vec<String>,
    },
    /// Classify a document against the HCDF-URDF profile.
    Profile {
        input: String,
        #[arg(long = "from", value_enum)]
        from_fmt: Option<Format>,
        /// Emit JSON instead of markdown.
        #[arg(long)]
        json: bool,
        /// Locate a ROS package for `$(find PKG)` xacro resolution (repeatable, `PKG=PATH`). Accepted for
        /// CLI parity; relevant once xacro lands.
        #[arg(long = "package", value_name = "PKG=PATH")]
        package: Vec<String>,
    },
    /// Pack a model + all its meshes into a self-contained `.hcdfz` (or `--dir`).
    Bundle {
        input: String,
        output: String,
        /// Locate a package for `package://PKG/...` mesh uris (repeatable, `PKG=PATH`).
        #[arg(long = "package", value_name = "PKG=PATH")]
        package: Vec<String>,
        /// Write a loose directory instead of a `.hcdfz`.
        #[arg(long)]
        dir: bool,
        /// Bundle even if some LOCAL meshes cannot be resolved.
        #[arg(long = "allow-partial")]
        allow_partial: bool,
        /// Fetch + embed remote (http/https) meshes and includes so the bundle is self-contained/offline.
        /// Mutually exclusive with `--keep-remote`. (Network fetch requires the `remote` feature.)
        #[arg(long = "vendor-remote", conflicts_with = "keep_remote")]
        vendor_remote: bool,
        /// Leave remote (http/https) uris LIVE in the bundle (do not fetch).
        #[arg(long = "keep-remote")]
        keep_remote: bool,
        /// Directory to content-address fetched remote bytes into.
        #[arg(long)]
        cache: Option<String>,
    },
    /// Verify a bundle's integrity (mesh blobs match their `@sha`).
    BundleVerify { input: String },
    /// Expand a xacro (or read a URDF) to a plain URDF, resolving `$(find)` via `--package`.
    Expand {
        input: String,
        /// Output path, or `-` for stdout.
        output: String,
        /// A xacro argument for a `.xacro` input (repeatable, `NAME:=VALUE`). Each entry overrides
        /// the matching `<xacro:arg>` default; an arg not given keeps its declared default.
        #[arg(long = "xacro", value_name = "NAME:=VALUE")]
        xacro: Vec<String>,
        /// Locate a ROS package for `$(find PKG)` and `package://` rewrites (repeatable, `PKG=PATH`).
        #[arg(long = "package", value_name = "PKG=PATH")]
        package: Vec<String>,
        /// Rewrite `package://PKG/...` mesh uris to absolute `file://` paths using the `--package` map.
        #[arg(long = "abs-meshes")]
        abs_meshes: bool,
    },
    /// Bake one mesh to a canonical content-addressed asset.
    Bake {
        /// The source mesh (`.stl`/`.obj`/`.dae`).
        input: String,
        /// Output asset path. The extension is canonical (`.glb` visual / `.stl` collision); the actual
        /// `@sha`-named file is written next to it.
        output: String,
        /// Bake as a collision (lean STL) rather than a visual (GLB).
        #[arg(long)]
        collision: bool,
        /// Source scale to fold into the vertices, e.g. `"0.001 0.001 0.001"`.
        #[arg(long)]
        scale: Option<String>,
        /// Flat `"r g b a"` color to bake into a visual GLB.
        #[arg(long)]
        color: Option<String>,
    },
    /// Regenerate an XSD-derived artifact (dev tool): the typed Rust enums, the JSON Schema, a
    /// spec-browser HTML page, or the editor completion model. The committed artifact is byte-identical
    /// to a fresh regen (the enums / schema / `spec.html` have a `cargo test` drift gate).
    Regen {
        #[command(subcommand)]
        target: RegenTarget,
    },
}

/// Which XSD-derived artifact `hcdf regen` rebuilds. Each takes the same `<input.xsd> <output>` contract
/// as its retired Python original (`generate_hcdf_rs_enums.py` / `generate_json_schema.py` /
/// `generate_spec_html.py` / `gen_completions.py`).
#[derive(Subcommand)]
enum RegenTarget {
    /// Regenerate the typed Rust enums (`src/model/enums.rs`). The residual `ENUM_ATTRS` table is
    /// computed from the model structs beside the output file, so `output`'s parent IS the model dir.
    Enums {
        /// The schema to read (`hcdf.xsd`).
        input: String,
        /// The enums file to (over)write (`rust/hcdformat-rs/src/model/enums.rs`).
        output: String,
    },
    /// Regenerate the JSON Schema (`hcdf.schema.json`): a derived view of the sha-pinned XSD.
    Schema {
        /// The schema to read (`hcdf.xsd`).
        input: String,
        /// The JSON Schema file to (over)write (`hcdf.schema.json`).
        output: String,
    },
    /// Regenerate a spec-browser HTML page from any HCDF schema. The page TITLE + tab layout are chosen
    /// from the input filename: `hcdf.xsd` → the core `TABS`, `hcdf-stream-profile.xsd` → the stream
    /// `TABS`, an `extensions/hcdf-ext-*.xsd` → an auto-discovered extension page.
    Spec {
        /// The schema to read (`hcdf.xsd`, `hcdf-stream-profile.xsd`, or an `extensions/*.xsd`).
        input: String,
        /// The HTML page to (over)write (e.g. `spec.html` or `website/spec/core.html`).
        output: String,
    },
    /// Regenerate the contextual editor completion model (`hcdf.completions.json`).
    Completions {
        /// The schema to read (`hcdf.xsd`).
        input: String,
        /// The completion-model JSON to (over)write (`hcdf.completions.json`).
        output: String,
    },
}

/// Run the `hcdf` CLI with `argv` (argv[0] is the program name); returns the process exit code
/// (0 success / 1 runtime error / 2 usage error). Called by BOTH the native `hcdf` bin and the wheel's
/// console script (via the PyO3 `run_cli` binding), so the Rust binary IS the CLI on every install path.
pub fn run(argv: Vec<String>) -> i32 {
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            // clap renders `--help`/`--version` to stdout (exit 0) and a usage error to stderr
            // (exit 2) WITHOUT terminating the process, so this is safe to call in-process from the
            // binding (a bare `Cli::parse()` would `std::process::exit` and kill the Python host).
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };
    match run_cmd(cli.command) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn run_cmd(cmd: Command) -> Result<i32, String> {
    match cmd {
        Command::Convert {
            input,
            output,
            from_fmt,
            to_fmt,
            loss,
            bake,
            xacro,
            package,
        } => cmd_convert(
            &input,
            &output,
            from_fmt,
            to_fmt,
            ConvertOpts {
                loss,
                bake,
                xacro,
                package,
            },
        ),
        Command::Validate {
            input,
            xsd,
            from_fmt,
            package,
        } => cmd_validate(&input, xsd, from_fmt, &package),
        Command::Profile {
            input,
            from_fmt,
            json,
            package,
        } => cmd_profile(&input, from_fmt, json, &package),
        Command::Bundle {
            input,
            output,
            package,
            dir,
            allow_partial,
            vendor_remote,
            keep_remote,
            cache,
        } => {
            // --vendor-remote fetches+embeds remote assets (self-contained bundle), --keep-remote leaves
            // the live web uri, and neither aborts on a remote uri (RemotePolicy::Error) so the choice is
            // conscious, matching the Python `bundle` contract.
            let remote = if vendor_remote {
                RemotePolicy::Vendor
            } else if keep_remote {
                RemotePolicy::Keep
            } else {
                RemotePolicy::Error
            };
            let opts = PackOptions {
                package_paths: parse_pkg_map(&package)?,
                allow_partial,
                as_dir: dir,
                remote,
                cache_dir: cache.map(PathBuf::from),
            };
            cmd_bundle(&input, &output, opts)
        }
        Command::BundleVerify { input } => cmd_bundle_verify(&input),
        Command::Expand {
            input,
            output,
            xacro,
            package,
            abs_meshes,
        } => cmd_expand(&input, &output, &xacro, &package, abs_meshes),
        Command::Bake {
            input,
            output,
            collision,
            scale,
            color,
        } => cmd_bake(
            &input,
            &output,
            collision,
            scale.as_deref(),
            color.as_deref(),
        ),
        Command::Regen { target } => cmd_regen(target),
    }
}

/// `regen enums|schema|spec|completions`: regenerate an XSD-derived artifact and write it (the pure-Rust
/// ports of the retired `generate_hcdf_rs_enums.py` / `generate_json_schema.py` / `generate_spec_html.py`
/// / `gen_completions.py`). Each reads the input XSD as text and writes the generated bytes verbatim:
/// the generators already terminate with each file's canonical trailing newline (or its deliberate LACK,
/// for the spec HTML + completions JSON), so no `write_out` fix-up is applied (it would launder a byte
/// difference).
fn cmd_regen(target: RegenTarget) -> Result<i32, String> {
    match target {
        RegenTarget::Enums { input, output } => {
            let xsd = std::fs::read_to_string(&input).map_err(|e| format!("{input}: {e}"))?;
            // The model structs live beside the output file (`.../src/model/enums.rs`), so the residual
            // `ENUM_ATTRS` slots are computed from that directory, the parent of the output path.
            let model_dir = Path::new(&output)
                .parent()
                .unwrap_or_else(|| Path::new("."));
            let code = crate::xsdgen::generate_enums_rs(&xsd, model_dir)?;
            std::fs::write(&output, &code).map_err(|e| format!("{output}: {e}"))?;
            eprintln!("regen enums: wrote {output}");
            Ok(0)
        }
        RegenTarget::Schema { input, output } => {
            let xsd = std::fs::read_to_string(&input).map_err(|e| format!("{input}: {e}"))?;
            let json = crate::xsdgen::generate_json_schema(&xsd)?;
            std::fs::write(&output, &json).map_err(|e| format!("{output}: {e}"))?;
            eprintln!("regen schema: wrote {output}");
            Ok(0)
        }
        RegenTarget::Spec { input, output } => {
            let xsd = std::fs::read_to_string(&input).map_err(|e| format!("{input}: {e}"))?;
            // The INPUT path (not the output) drives the title + tab layout, mirroring the Python
            // `main`'s filename detection.
            let html = crate::xsdgen::generate_spec_html(&xsd, &input)?;
            std::fs::write(&output, &html).map_err(|e| format!("{output}: {e}"))?;
            eprintln!("regen spec: wrote {output}");
            Ok(0)
        }
        RegenTarget::Completions { input, output } => {
            let xsd = std::fs::read_to_string(&input).map_err(|e| format!("{input}: {e}"))?;
            let json = crate::xsdgen::generate_completions_json(&xsd)?;
            std::fs::write(&output, &json).map_err(|e| format!("{output}: {e}"))?;
            eprintln!("regen completions: wrote {output}");
            Ok(0)
        }
    }
}

/// Resolve a format from `--from`/`--to` or the path extension.
fn resolve_fmt(path: &str, override_: Option<Format>) -> Result<Format, String> {
    override_.or_else(|| Format::infer(path)).ok_or_else(|| {
        format!("cannot infer format from {path:?}; pass --from/--to (known: urdf, sdf, hcdf)")
    })
}

/// Read any supported input into an HCDF document, returning `(doc, import_notes, asset_hints)`. SDF is
/// read RAW through [`from_sdf_str_with_assets`], mirroring the Python CLI's `convert` (which routes SDF
/// through `sdf_io.from_sdf`, a tolerant lxml walk that never shells to gz). A `.xacro` URDF input is
/// expanded to URDF first (the FULLY pure-Rust `xacro-pure` engine, no subprocess), then mapped, exactly
/// like `hcdf_cli.py`'s `_load` -> `from_urdf` -> `xacro.expand`. The [`VisualAssetHint`]s are the
/// importers' per-visual scale/colour side-channel `convert --bake` folds into the baked GLBs (empty for
/// a native `.hcdf`, which carries no source scale/colour to bake).
fn load(
    path: &str,
    fmt: Format,
    xacro_args: &[String],
    package: &[String],
) -> Result<(Hcdf, Vec<String>, Vec<VisualAssetHint>), String> {
    match fmt {
        Format::Sdf => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            from_sdf_str_with_assets(&text).map_err(|e| e.to_string())
        }
        Format::Urdf => {
            let text = if is_xacro(path) {
                let pkg = parse_pkg_str_map(package)?;
                // `--xacro NAME:=VALUE` overrides seed the pure-Rust engine's `$(arg)` table (each entry
                // overriding that arg's `<xacro:arg>` default). Expansion is chained here (rather than
                // `from_xacro_path`) so the import hints survive.
                let mappings: std::collections::HashMap<String, String> =
                    parse_xacro_map(xacro_args)?.into_iter().collect();
                crate::expand_xacro_with_args(Path::new(path), &pkg, &mappings)
                    .map_err(|e| e.to_string())?
            } else {
                std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?
            };
            from_urdf_str_with_assets(&text).map_err(|e| e.to_string())
        }
        Format::Hcdf => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            Hcdf::from_xml_str(&text)
                .map(|d| (d, Vec::new(), Vec::new()))
                .map_err(|e| e.to_string())
        }
        Format::Json => {
            // JSON in -> HCDF XML (XSD-driven, native Rust) -> typed doc, mirroring `hcdf_cli.py`'s
            // `_load` json branch (`from_json`). No import hints; JSON carries a full HCDF document.
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let xml = json_to_hcdf_xml(&text).map_err(|e| e.to_string())?;
            Hcdf::from_xml_str(&xml)
                .map(|d| (d, Vec::new(), Vec::new()))
                .map_err(|e| e.to_string())
        }
    }
}

/// Whether `path` is a xacro file (`.xacro` extension), matching `hcdf_cli.py`'s `str(input).endswith`.
fn is_xacro(path: &str) -> bool {
    Path::new(path).extension().and_then(|e| e.to_str()) == Some("xacro")
}

/// Parse `--package PKG=PATH` items into the `{pkg: path}` string map `expand_xacro` wants for `$(find)`.
fn parse_pkg_str_map(
    items: &[String],
) -> Result<std::collections::HashMap<String, String>, String> {
    let mut m = std::collections::HashMap::new();
    for it in items {
        let (k, v) = it
            .split_once('=')
            .ok_or_else(|| format!("--package expects PKG=PATH, got {it:?}"))?;
        m.insert(k.to_string(), v.to_string());
    }
    Ok(m)
}

/// Parse `--xacro NAME:=VALUE` items, matching `hcdf_cli.py`'s `_xacro_map` (`:=` split), into the
/// `{name: value}` override map the pure-Rust engine seeds the `$(arg)` table from.
fn parse_xacro_map(items: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut m = BTreeMap::new();
    for it in items {
        let (k, v) = it
            .split_once(":=")
            .ok_or_else(|| format!("--xacro expects NAME:=VALUE, got {it:?}"))?;
        m.insert(k.to_string(), v.to_string());
    }
    Ok(m)
}

fn write_out(text: &str, out: &str) -> Result<(), String> {
    let text = if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    };
    if out == "-" {
        print!("{text}");
        Ok(())
    } else {
        std::fs::write(out, text).map_err(|e| format!("{out}: {e}"))
    }
}

/// The `convert` options beyond the input/output/format core, bundled into ONE value so `cmd_convert`
/// stays within the clippy argument budget WITHOUT an `#[allow]` (the same shape as `Bundle`'s
/// [`PackOptions`]): the `--loss` JSON path, the `--bake DIR` asset store, and the `--xacro`/`--package`
/// input-resolution maps.
struct ConvertOpts {
    loss: Option<String>,
    bake: Option<String>,
    xacro: Vec<String>,
    package: Vec<String>,
}

fn cmd_convert(
    input: &str,
    output: &str,
    from_fmt: Option<Format>,
    to_fmt: Option<Format>,
    opts: ConvertOpts,
) -> Result<i32, String> {
    // For a `.xacro` input `--package`/`--xacro` drive `$(find)`/arg resolution; under `--bake` the
    // package map ALSO resolves `package://`/`model://` mesh uris. For a plain URDF/HCDF input without
    // `--bake`, `package://` mesh resolution is a bundle-time concern, so they are inert there.
    let src = resolve_fmt(input, from_fmt)?;
    let dst = if let Some(t) = to_fmt {
        t
    } else if output != "-" {
        resolve_fmt(output, None)?
    } else {
        return Err("writing to stdout requires --to FORMAT".to_string());
    };

    let (mut doc, mut notes, hints) = load(input, src, &opts.xacro, &opts.package)?;
    if let Some(bake_dir) = opts.bake.as_deref() {
        if src == Format::Hcdf {
            // Python parity: the baker is an IMPORT hook (`from_urdf`/`from_sdf` receive it), so a native
            // `.hcdf` input takes no baker, but say so rather than silently doing nothing.
            notes.push(
                "--bake is inert for an HCDF input (vendor an existing document's meshes with `hcdf bundle`)"
                    .to_string(),
            );
        } else if src == Format::Json {
            // A JSON input is a full HCDF document (like `.hcdf`); `hcdf_cli.py`'s `from_json` ignores the
            // baker, so `--bake` is silently inert here too (no source scale/colour/uri to fold in).
        } else {
            let package_map = parse_pkg_map(&opts.package)?;
            let env = BakeEnv::new(bake_dir, input, output, &package_map)?;
            let baked = bake_document(&mut doc, &hints, &env, &mut notes);
            // The importers emit their deferred-bake loss notes unconditionally (they never bake, like
            // `from_urdf.py` with `baker=None`); this pass just consumed them, so say so; otherwise a
            // "scale not applied" note above would read as a loss that did NOT happen.
            if !baked.is_empty() {
                notes.push(format!(
                    "bake: {} mesh(es) baked into {bake_dir}: a deferred-bake import note above \
                     (scale \"not applied\" / material \"dropped\") IS folded into the asset for every \
                     mesh that baked",
                    baked.len()
                ));
            }
        }
    }
    let (text, loss) = match dst {
        Format::Hcdf => (doc.to_xml_string().map_err(|e| e.to_string())?, None),
        Format::Json => {
            // HCDF -> JSON, mirroring `hcdf_cli.py`'s `_dump` json branch (`json.dumps(to_json(doc))`):
            // re-serialize the typed doc to canonical XML, then convert that XML to the JSON view. No
            // loss manifest (JSON is the full-fidelity HCDF view).
            let xml = doc.to_xml_string().map_err(|e| e.to_string())?;
            (hcdf_xml_to_json(&xml).map_err(|e| e.to_string())?, None)
        }
        Format::Urdf => {
            let (urdf, loss) = to_urdf(&doc).map_err(|e| e.to_string())?;
            (urdf, Some(loss))
        }
        Format::Sdf => {
            let (sdf, loss) = to_sdf(&doc).map_err(|e| e.to_string())?;
            (sdf, Some(loss))
        }
    };
    write_out(&text, output)?;

    // Nothing is dropped silently: import notes and export losses both go to stderr.
    for n in &notes {
        eprintln!("note: {n}");
    }
    if let Some(loss) = &loss {
        if !loss.is_empty() {
            if let Some(lf) = opts.loss.as_deref() {
                // The pure render carries no trailing newline (json.dumps parity); the CLI terminates
                // the written file with one, as it always has.
                std::fs::write(lf, format!("{}\n", loss.to_json()))
                    .map_err(|e| format!("{lf}: {e}"))?;
                eprintln!("loss: {} item(s) written to {lf}", loss.len());
            } else {
                eprintln!(
                    "loss: {} construct(s) not representable in {}:",
                    loss.len(),
                    fmt_name(dst)
                );
                eprintln!("{}", loss.text());
            }
        }
    }
    let dest = if output == "-" { "stdout" } else { output };
    eprintln!("{} -> {}: wrote {dest}", fmt_name(src), fmt_name(dst));
    Ok(0)
}

fn fmt_name(f: Format) -> &'static str {
    match f {
        Format::Urdf => "URDF",
        Format::Sdf => "SDF",
        Format::Hcdf => "HCDF",
        Format::Json => "JSON",
    }
}

fn cmd_validate(
    input: &str,
    xsd: bool,
    from_fmt: Option<Format>,
    package: &[String],
) -> Result<i32, String> {
    // `--package` matters only for a `.xacro` input's `$(find)` resolution; a plain .hcdf/.urdf input
    // needs no package map.
    let fmt = resolve_fmt(input, from_fmt)?;
    if xsd {
        return cmd_validate_xsd(input, fmt, package);
    }
    // Plain validate runs ONLY the kinematic/reference validator (`validate_semantic`), matching the
    // Python `hcdf validate` authority: the required-attribute / enum-literal SHAPE checks live behind
    // `--xsd` (Python's lxml hcdf.xsd path), NOT in plain validate. So a doc missing a required @name
    // is OK here (exit 0), exactly like Python. A doc the reader REJECTS (unsupported `@version`, XML
    // parse error) is a validation failure of its own, reported as `E_LOAD` + INVALID (exit 1), the
    // same UX as `--xsd`, never the generic CLI error path.
    let (doc, _, _) = match load(input, fmt, &[], package) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("[error] E_LOAD: {e}");
            eprintln!("INVALID: 1 issue(s), 1 error(s)");
            return Ok(1);
        }
    };
    let mut issues = validate_semantic(&doc);
    // The comms/network validators (referential integrity, uniqueness, port-type/protocol
    // compatibility, canonical casing) have no Python oracle but run in the same pass so `hcdf validate`
    // reports them too. Appended after the kinematic issues; both fold into the exit-code decision.
    issues.extend(validate_network(&doc));
    // The loop-closure validators (predecessor/successor refs, `<constraint-axes>` mask shape, body
    // agreement) likewise have no Python oracle but belong to the same pass; appended so a four-bar /
    // Stewart model's closure faults fold into the exit-code decision too.
    issues.extend(validate_loops(&doc));
    // The schema-coverage validators (self-collision pair refs, transmission endpoint refs, motor
    // sensor refs, pin duplicate/carrier) run in the same pass too; no Python oracle, but part
    // of `validate`, so their errors fold into the exit-code decision like the network/loop ones.
    issues.extend(validate_coverage(&doc));
    for i in &issues {
        // `[level] code: message`, byte-identical to Python `Issue.__str__`.
        eprintln!("{i}");
    }
    let errors = issues.iter().filter(|i| i.level == Level::Error).count();
    eprintln!(
        "{}: {} issue(s), {errors} error(s)",
        if errors == 0 { "OK" } else { "INVALID" },
        issues.len()
    );
    Ok(if errors == 0 { 0 } else { 1 })
}

/// Print a batch of validation issues (`[level] code: message` each, like Python `Issue.__str__`) and
/// fold them into the running `validate --xsd` totals.
fn report(issues: &[Issue], total: &mut usize, errors: &mut usize) {
    for i in issues {
        eprintln!("{i}");
    }
    *total += issues.len();
    *errors += issues.iter().filter(|i| i.level == Level::Error).count();
}

/// `validate --xsd`: the SCHEMA-SHAPE layer: the real `hcdf.xsd` engine (`validate_xsd`, uppsala) plus
/// the typed-model shape checks (`validate_structural` + `validate_enums`), the Rust analogue of
/// Python's lxml `hcdf.xsd` validation. A native `.hcdf` is schema-validated as its RAW bytes, exactly
/// like Python: the typed parse silently DROPS constructs the schema rejects (legacy text-content
/// poses, `<box>` with raw text instead of a `<size>` child), so validating a re-serialization would
/// launder them into "VALID". A converted URDF/SDF input has no raw HCDF, so there the re-serialization
/// IS the document under test.
fn cmd_validate_xsd(input: &str, fmt: Format, package: &[String]) -> Result<i32, String> {
    let mut errors = 0usize;
    let mut total = 0usize;
    let raw = match fmt {
        Format::Hcdf => {
            let text = std::fs::read_to_string(input).map_err(|e| format!("{input}: {e}"))?;
            report(&validate_xsd(&text), &mut total, &mut errors);
            Some(text)
        }
        _ => None,
    };
    // The typed-model layers still run for BOTH input kinds, for a native `.hcdf` on the SAME raw
    // bytes the schema pass saw. A doc the reader REJECTS (unsupported `@version`, XML parse error) is
    // itself a validation failure, reported as `E_LOAD` + INVALID (exit 1), never the generic CLI
    // error path (the raw-bytes schema issues above still print; note the raw XSD pass ACCEPTS a
    // same-shape foreign `@version` by design, so `E_LOAD` is the only gate that catches it).
    let loaded = match &raw {
        Some(text) => Hcdf::from_xml_str(text).map_err(|e| e.to_string()),
        None => load(input, fmt, &[], package).map(|(d, _, _)| d),
    };
    match loaded {
        Ok(doc) => {
            let xml = doc.to_xml_string().map_err(|e| e.to_string())?;
            if raw.is_none() {
                report(&validate_xsd(&xml), &mut total, &mut errors);
            }
            if let Err(structural) = validate_structural(&doc) {
                for s in &structural {
                    eprintln!("[error] E_STRUCTURAL: {s}");
                }
                total += structural.len();
                errors += structural.len();
            }
            report(
                &validate_enums(&xml).map_err(|e| e.to_string())?,
                &mut total,
                &mut errors,
            );
        }
        Err(e) => {
            eprintln!("[error] E_LOAD: {e}");
            total += 1;
            errors += 1;
        }
    }
    eprintln!(
        "{}: {total} issue(s), {errors} error(s)",
        if errors == 0 { "VALID" } else { "INVALID" }
    );
    Ok(if errors == 0 { 0 } else { 1 })
}

fn cmd_profile(
    input: &str,
    from_fmt: Option<Format>,
    json: bool,
    package: &[String],
) -> Result<i32, String> {
    // `--package` resolves `$(find)` for a `.xacro` input; inert for a plain .hcdf input.
    let fmt = resolve_fmt(input, from_fmt)?;
    let (doc, _, _) = load(input, fmt, &[], package)?;
    let report = check_profile(&doc);
    if json {
        println!("{}", report.to_json());
    } else {
        println!("{}", report.markdown());
    }
    Ok(if report.in_profile() { 0 } else { 1 })
}

fn parse_pkg_map(items: &[String]) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut m = BTreeMap::new();
    for it in items {
        let (k, v) = it
            .split_once('=')
            .ok_or_else(|| format!("--package expects PKG=PATH, got {it:?}"))?;
        m.insert(k.to_string(), PathBuf::from(v));
    }
    Ok(m)
}

fn cmd_bundle(input: &str, output: &str, opts: PackOptions) -> Result<i32, String> {
    let m = pack(Path::new(input), Path::new(output), &opts)?;
    for n in &m.notes {
        eprintln!("note: {n}");
    }
    for e in &m.unresolved {
        eprintln!(
            "unresolved: {} ({}), left as-is (--allow-partial)",
            e.site, e.old_uri
        );
    }
    for site in &m.kept_remote {
        eprintln!("kept-remote: {site}, left as a live web uri (--keep-remote)");
    }
    eprintln!(
        "bundle: wrote {} {output} (root {}, {} entr{})",
        m.format,
        m.root,
        m.entries,
        if m.entries == 1 { "y" } else { "ies" }
    );
    Ok(0)
}

fn cmd_bundle_verify(input: &str) -> Result<i32, String> {
    let rep = verify(Path::new(input));
    for issue in &rep.issues {
        eprintln!("issue: {issue}");
    }
    eprintln!(
        "{}: {} issue(s)",
        if rep.ok { "OK" } else { "INVALID" },
        rep.issues.len()
    );
    Ok(if rep.ok { 0 } else { 1 })
}

/// `expand`: expand a `.xacro` to a plain URDF (FULLY pure-Rust `xacro-pure` engine, SO-ARM AND OpenArm,
/// no subprocess), or read a `.urdf` through verbatim; optionally rewrite `package://PKG/...` mesh uris to
/// absolute `file://` paths. A port of `hcdf_cli.py`'s `cmd_expand`.
fn cmd_expand(
    input: &str,
    output: &str,
    xacro: &[String],
    package: &[String],
    abs_meshes: bool,
) -> Result<i32, String> {
    let pkg = parse_pkg_str_map(package)?;
    let mut text = if is_xacro(input) {
        // `--xacro NAME:=VALUE` overrides seed the pure-Rust engine's `$(arg)` table (each entry
        // overriding that arg's `<xacro:arg>` default).
        let mappings: std::collections::HashMap<String, String> =
            parse_xacro_map(xacro)?.into_iter().collect();
        crate::expand_xacro_with_args(Path::new(input), &pkg, &mappings)
            .map_err(|e| e.to_string())?
    } else {
        std::fs::read_to_string(input).map_err(|e| format!("{input}: {e}"))?
    };
    if abs_meshes {
        for (name, path) in &pkg {
            let abs = std::fs::canonicalize(path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.clone());
            let abs = abs.trim_end_matches('/');
            text = text.replace(&format!("package://{name}/"), &format!("file://{abs}/"));
        }
    }
    write_out(&text, output)?;
    eprintln!(
        "expand: wrote {}",
        if output == "-" { "stdout" } else { output }
    );
    Ok(0)
}

fn cmd_bake(
    input: &str,
    output: &str,
    collision: bool,
    scale: Option<&str>,
    color: Option<&str>,
) -> Result<i32, String> {
    let kind = if collision {
        BakeKind::Collision
    } else {
        BakeKind::Visual
    };
    let asset = bake_convert(input, scale, kind, color).map_err(|e| e.to_string())?;
    // Appearance fallbacks (an OBJ's unresolved .mtl, a skipped map_Kd texture) are non-fatal but must
    // be VISIBLE; the alternative is a user staring at an unexplained gray mesh.
    for note in &asset.notes {
        eprintln!("bake: note: {note}");
    }
    // write the @sha-named canonical file next to the requested output (the bundle convention).
    let out = Path::new(output);
    let parent = out.parent().unwrap_or_else(|| Path::new("."));
    let short = asset
        .sha
        .strip_prefix("sha256:")
        .unwrap_or(&asset.sha)
        .get(..12)
        .unwrap_or("");
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("asset");
    let fname = format!("{stem}_{short}.{}", asset.ext);
    let path = parent.join(&fname);
    std::fs::write(&path, &asset.data).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!(
        "bake: wrote {} ({} bytes, {})",
        path.display(),
        asset.data.len(),
        asset.sha
    );
    Ok(0)
}
