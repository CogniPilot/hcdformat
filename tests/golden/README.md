# Golden artifacts: the frozen parity fixtures for the Rust core

These committed golden files are the **frozen parity contract** for HCDF conversion/validation. The
canonical producer is now the Rust core (`hcdformat-rs`); the Rust crate's `tests/parity.rs` reads these
fixtures and asserts the core reproduces them. The pure-Python implementation that originally emitted
them (the original Python oracle) has been **retired**, so these files are frozen snapshots, not a
regenerable Python output; there is no longer a `regen_golden.py`.

## Layout

For each corpus document there is a directory `tests/golden/<name>/` holding four artifacts:

| file | canonical producer (Rust core) | what it is |
|---|---|---|
| `canonical.xml` | `Hcdf::from_xml_str(...).to_xml_string()` | the canonical, schema-ordered XML serialization |
| `model.json` | the Rust XSD-driven JSON walker (`hcdf_xml_to_json`) | the JSON projection (`hcdf.schema.json` shape) |
| `issues.txt` | `validate_semantic(...)` | one `level<TAB>code<TAB>message` per issue, sorted |
| `load.txt` | the load/accept decision | `accept`: the doc loaded cleanly, so every enum literal it carries is valid (the enum-false-POSITIVE fixture) |

Plus a corpus-independent directory `tests/golden/_enum_oracle/`, the enum **reject** fixtures. An
out-of-enum literal is a **load-time** rejection (it is not surfaced through `validate_semantic`), so
`issues.txt` has no enum-reject channel; this directory pins that channel separately. Each fixture is a
deliberately-corrupted document with exactly one out-of-enum value:

| file | what it is |
|---|---|
| `<fixture>.xml` | the corrupted input the Rust harness feeds to `validate_enums` |
| `<fixture>.oracle` | `reject<TAB>element<TAB>attr<TAB>bad_value<TAB>error`: that the literal must be rejected, at which slot, with which error message (frozen from the original oracle) |

The corpus is the eight first-party `examples/*.hcdf` plus two representative real-robot conversions
(`so_arm101`, `openarm-model`) from the sibling `hcdf-conversion-examples` repo. The converted-robot
goldens regenerate only when that repo is checked out; the in-repo example goldens always regenerate.
The enum-reject fixtures are corpus-independent and always regenerate.

## Checking and changing a golden

```sh
cargo test -p hcdformat --test parity   # assert the Rust core reproduces every frozen golden
```

There is no regeneration script: the pure-Python oracle (and its `regen_golden.py`) is retired, and
these fixtures are frozen. A golden changes only by a **deliberate, reviewed commit**; you verify the
Rust core's new output is the intended value and hand-update the file. An unexpected `parity` failure
means the Rust core drifted, not that a golden is stale; fix the core.

## The fixtures are the contract; a diff is an intentional change

These files ARE the source of truth the Rust side is held to. The Rust harness
(`rust/hcdformat-rs/tests/parity.rs`) reads them and asserts:

- **XML round-trip parity against the golden** (semantic, not byte): the Rust model parses
  `canonical.xml`, re-serializes, and the re-serialization must reproduce the golden's **element
  content**, the `(parent-localname, child-localname)` multiset. Attribute-order, whitespace,
  float-reformat, and defaulted-attribute noise (`rpy="0 0 0"`) are normalized away by comparing
  element presence/parentage only. Any element the golden has that Rust **drops** fails the test,
  unless that `(parent, child)` is in the committed `KNOWN_UNMODELED_ELEMENTS` allowlist in
  `parity.rs`: the pre-existing hand-written-model gaps (sensor params, battery, the TSN stack,
  propellers, …) that model completion will close. `so_arm101`/`openarm-model` use **zero**
  allowlist entries; they round-trip exactly. A stale allowlist entry (one no corpus file actually
  drops) also fails, so the list shrinks honestly as the model completes. (The earlier harness only
  asserted Rust serialize/parse was a fixpoint, `doc == doc2`, which is blind to silent data loss:
  every dropped element is absent from *both* sides, so the equality always held. That idempotency
  check is kept, but clearly labeled as self-consistency, not parity.)
- **Enum-validation parity against the golden**: two real channels, not an `assert_eq!(empty, empty)`.
  *accept*: every corpus doc the golden marks accepted (`load.txt == accept`) must yield zero
  `E_ENUM_VALUE` from Rust (no false positives over every authored enum value). *reject*: each
  `_enum_oracle/` fixture pinned as rejected with a named enum must be flagged by Rust's
  `validate_enums` at exactly that `(element, attr)` and nowhere else. Additional semantic codes (e.g.
  `E_MULTI_PARENT`) in `issues.txt` are not yet asserted by Rust; that comparison will land later.

The Rust core now has a JSON output path (`hcdf_xml_to_json`, the XSD-driven walker), byte-checked
against its own frozen goldens in `rust/hcdformat-rs/tests/json_convert.rs`. `parity.rs` still only
verifies each corpus `model.json` **exists** (it does not diff this projection).
