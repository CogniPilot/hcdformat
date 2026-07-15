# Bundling & remote assets

How an HCDF model and all the geometry it references are packed into one portable artifact, and how
remote (web-hosted) assets are handled. The behaviour here is **deliberate**: in particular the
network/security posture is a designed-in choice, not an accident of the implementation, and this
document is the canonical place that records *why*.

## The `.hcdfz` bundle

An `.hcdf` document references its meshes (and, via `<include>`, its sub-assemblies) by URI. To ship,
archive, or reproduce a model you need those bytes alongside it. A **bundle** pins a model to the exact
bytes that produced each `@sha`:

```bash
hcdf bundle robot.hcdf robot.hcdfz          # one self-contained file
hcdf bundle robot.hcdf out/ --dir           # a loose, git-diffable directory
hcdf bundle-verify robot.hcdfz              # rehash every blob against its @sha
```

A `.hcdfz` is a **USDZ-shaped ZIP**: the root `.hcdf` is the **first entry**, every entry is stored
**uncompressed (`ZIP_STORED`)** so a blob's bytes are byte-identical to its `@sha` (and mmap-friendly),
and all meshes live under `assets/` as **content-addressed** `<name>_<sha>.<ext>` with document-relative
`@uri`s. `<include>`s are **flattened** into the single document, so a bundle opens with no resource-path
setup, no ROS environment, and no package map. This is a container *around* the frozen 1.0 schema (it
only writes values into existing `@uri`/`@sha` fields); the schema itself is never changed.

## `<include>` SHA integrity

`<include uri sha …>` carries an **optional** `@sha` (the sha-256 of the included file's bytes,
`sha256:<hex>`). When present it is **verified on resolve** by the Rust resolver
(`hcdformat-rs::compose`), the single implementation, reached from the CLI and from the
`hcdf.io.flatten` Python entry point alike.

Two deliberate rules make this useful without being obstructive:

- **A mismatch is a *note*, never an error.** A module legitimately changes as it is edited (e.g. the
  live-link authoring flow in `dendrite_build`), so a drifted `@sha` surfaces a warning ("module changed
  since pinned") and composition still proceeds. It is the *caller's* choice to treat a mismatch as
  fatal (a reproducible build can).
- **No `@sha` ⇒ no verification.** Hand-authored includes without a pin behave exactly as before. Pins
  are added intentionally (`stamp_include_shas`, or `dendrite_build`'s "Pin includes" action), typically
  before producing a reproducible bundle.

## Remote (`http(s)://`) assets: the integrator's choice

A model may reference a mesh or an `<include>` by an `http(s)://` URL. What happens to it at bundle time
is **the integrator's explicit, conscious choice**, never a silent default:

| Flag | Behaviour |
|------|-----------|
| `--vendor-remote` | **Fetch + embed.** Every remote mesh/include is downloaded, integrity-checked, and stored in the bundle. The output has **zero `http(s)` URIs**, fully self-contained and offline. |
| `--keep-remote`   | **Leave live.** Remote URIs stay in the bundle as live web references (a genuinely-missing *local* asset still errors unless `--allow-partial`). |
| *(neither)*       | A remote URI is an **explicit error** naming it and telling you to pick one, so a model that depends on the network can never bundle by accident. |

`dendrite_build` surfaces the same three-way choice on its **Bundle** button, and an opt-in **"Load
remote assets"** checkbox (off by default) that renders web assets live by vendoring them to a cache
through the same path described below, first telling you *what* would download.

## Security posture (intentional, by design)

Fetching arbitrary URLs named inside a document is a foot-gun (SSRF, supply-chain, resource exhaustion).
The design treats this as a first-class threat and is **safe by construction**. These are deliberate
decisions, recorded here so they are not "simplified away" later:

- **Opt-in only.** No tool ever fetches a URL unless the integrator explicitly asks (`--vendor-remote`,
  or the off-by-default editor checkbox). A default bundle, a default render, and every offline workflow
  touch the network **zero** times.
- **One audited fetch path.** All network I/O lives in a single hardened Rust helper
  (`hcdformat-rs::remote`, driven by `vendor_assets`). Nothing else performs raw HTTP; `dendrite_build`
  and `hcdviz` reach the network only through it (via the CLI / vendor path), so they inherit every
  guard below for free.
- **Scheme allow-list, enforced across redirects.** Only `http://` / `https://` are fetched. `file://`,
  `ftp://`, `data:`, bare paths, etc. are rejected *before* any I/O, so the fetcher can't be turned into
  a local-file read primitive, and it **re-validates every 3xx redirect target's scheme**, so an
  `http → ftp://`/`file://` redirect cannot smuggle a disallowed scheme past the guarantee.
- **Bounded.** A **timeout** (default 30 s) and a streamed **size cap** (default 256 MiB, with a
  `Content-Length` over the cap rejected up front) prevent a hung or hostile URL from stalling a build or
  exhausting memory/disk.
- **Integrity-pinned.** When an element carries `@sha`, fetched bytes are verified against it; a mismatch
  is a hard error and the bytes are discarded: **tampered content is never embedded or rendered**. The
  cache is **content-addressed** and **re-verified on every hit** (no cache poisoning), written
  atomically (temp + rename), in a per-user private directory.

The net guarantees: a `--vendor-remote` bundle is self-contained and verifiable; a tampered remote asset
fails closed; and a viewer that opts into live remote rendering falls back to a local render on any
failure rather than displaying unverified bytes.

## See also

- `hcdformat-rs::remote`: the fetch helper (per-function rationale); exposed to Python as `hcdf.assets`.
- `hcdformat-rs::bundle`: `pack` / `open_bundle` / `verify` (Python: `hcdf.io.pack` / `hcdf.io.open_bundle` / `hcdf.io.verify`).
- `hcdformat-rs::compose`: the `<include>` resolver + `@sha` matching (Python: `hcdf.io.flatten`).
- `hcdf bundle --help`, `hcdf bundle-verify --help`.
