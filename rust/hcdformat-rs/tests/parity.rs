//! Frozen corpus and compatibility contract harness.
//!
//! Each corpus document has committed artifacts under `<repo>/tests/golden/<name>/`:
//! `canonical.xml` (the canonical schema-ordered Rust serialization), `model.json` (the Rust JSON
//! projection), `issues.txt` (sorted semantic validation findings, one
//! `level<TAB>code<TAB>message` per line), and `load.txt` (the accepted-document contract).
//!
//! The `_enum_oracle` and `_invalid_oracle` directories retain frozen compatibility cases originally
//! captured from the retired Python implementation. No Python executes in this harness. Those cases
//! preserve useful rejection decisions and error details while the current Rust model and validator
//! remain the authority for generated corpus artifacts.
//!
//! Every enumerated attribute or element-text slot is typed where the XML model permits it. A bad
//! typed value is rejected during `Hcdf::from_xml_str`; the ten shared sensor-category `@type` slots
//! remain strings and are checked by `validate_enums`. The reject channel dispatches on
//! `valid_values_for(element, attr)` so both representation paths remain covered.
//!
//! For each corpus file this test asserts:
//!   (a) Parsing and reserializing the canonical golden preserves its full element-content multiset.
//!       Attribute order, whitespace, float formatting, and defaulted-attribute presence are outside
//!       this comparison, while any silently dropped or fabricated element fails.
//!   (b) Every accepted corpus document parses without an enum finding, and every frozen enum-reject
//!       fixture is rejected at parse or by `validate_enums`, according to its modeled slot.
//!   (c) Rust semantic findings exactly match the committed sorted `issues.txt` contract.
//!
//! Byte-identical JSON output is asserted separately by `tests/json_convert.rs`; this harness checks
//! that the companion corpus artifact is present.
use hcdformat::model::enums::valid_values_for;
use hcdformat::Hcdf;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `<repo>/tests/golden` (the crate sits at `<repo>/rust/hcdformat-rs`).
fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden")
}

/// Rust's `validate_semantic` output serialized as the frozen `issues.txt` contract:
/// one `level<TAB>code<TAB>message` line per issue, sorted.
fn rust_issue_lines(doc: &Hcdf) -> String {
    let mut lines: Vec<String> = hcdformat::validate_semantic(doc)
        .iter()
        .map(|i| format!("{}\t{}\t{}", i.level.as_str(), i.code, i.message))
        .collect();
    lines.sort();
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// The sorted `level:code` set of Rust's `validate_semantic` output: the parity surface the invalid
/// corpus's `accept\t<level:code,...>` oracle line records (matching `issues.txt`'s code projection).
fn rust_semantic_codes(doc: &Hcdf) -> String {
    let mut codes: Vec<String> = hcdformat::validate_semantic(doc)
        .iter()
        .map(|i| format!("{}:{}", i.level.as_str(), i.code))
        .collect();
    codes.sort();
    codes.join(",")
}

/// The `(parent-localname, child-localname)` pairs the hand-written Rust model does NOT capture, so
/// serde would silently drop them on parse (the model has no `deny_unknown_fields`; see `model/mod.rs`).
/// Listing them here makes any loss EXPLICIT and reviewable: assertion (a) fails on any drop NOT in this
/// set (a real regression of a currently-modeled element), and fails if an entry here is never actually
/// dropped across the corpus (a stale entry, so the list shrinks honestly as the model completes).
///
/// This list is currently EMPTY: the hand-written model now captures every leaf subsystem the corpus
/// exercises (radar/lidar/gnss/camera params, camera intrinsics, data-output, IMU fifo, battery, prop,
/// motor thrust, switch/port TSN + MACsec + EEE capabilities, antenna bands, bus voltage, participant
/// protocols, chain-hop spurs). The whole golden corpus, including `humanoid-mobile-base`, which
/// previously dropped 320 elements, now round-trips with ZERO loss. The schema is frozen 1.0, so this
/// hand-written completeness is drift-proof. Any future drop will surface as an `unexpected_drops`
/// failure rather than being silently masked.
///
/// Granularity is `(parent, child)` (not bare child name) on purpose: `("spur", "description")` would be
/// a distinct network leaf from the modeled `("comp", "description")`; a bare `description` allowlist
/// would mask a future regression that drops a modeled `<comp><description>`.
const KNOWN_UNMODELED_ELEMENTS: &[(&str, &str)] = &[];

/// The `(parent-localname, child-localname)` -> count multiset of every element in `xml`. The document
/// root has the sentinel parent `(root)`. This is the oracle comparison surface for assertion (a):
/// element presence + parentage, deliberately ignoring attributes/text so attribute-order, whitespace,
/// float-reformat, and defaulted-attribute noise (the legitimate semantic-bar noise) cannot fail it.
fn element_multiset(xml: &str) -> BTreeMap<(String, String), i64> {
    let mut reader = Reader::from_str(xml);
    let mut counts: BTreeMap<(String, String), i64> = BTreeMap::new();
    let mut stack: Vec<String> = Vec::new();
    loop {
        match reader.read_event().expect("well-formed XML") {
            Event::Start(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let parent = stack
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "(root)".to_string());
                *counts.entry((parent, local.clone())).or_insert(0) += 1;
                stack.push(local);
            }
            Event::Empty(e) => {
                let local = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let parent = stack
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "(root)".to_string());
                *counts.entry((parent, local)).or_insert(0) += 1;
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    counts
}

/// Assert Rust's `reserialized` output reproduces the `golden`'s element content, allowing only the
/// committed [`KNOWN_UNMODELED_ELEMENTS`] drops. Returns the set of allowlist entries this file
/// actually exercised, so the caller can detect stale (never-dropped) entries across the corpus.
fn assert_element_parity_against_golden(
    name: &str,
    golden: &str,
    reserialized: &str,
    used_allow: &mut std::collections::BTreeSet<(String, String)>,
) {
    let g = element_multiset(golden);
    let r = element_multiset(reserialized);
    let mut unexpected_drops: Vec<String> = Vec::new();
    let mut rust_gained: Vec<String> = Vec::new();
    for (key, gcount) in &g {
        let rcount = r.get(key).copied().unwrap_or(0);
        if *gcount > rcount {
            let pair = (key.0.clone(), key.1.clone());
            if KNOWN_UNMODELED_ELEMENTS.contains(&(key.0.as_str(), key.1.as_str())) {
                used_allow.insert(pair);
            } else {
                unexpected_drops.push(format!(
                    "  <{}> under <{}>: oracle={gcount} rust={rcount}",
                    key.1, key.0
                ));
            }
        }
    }
    // Rust must never emit an element the oracle does not (a fabricated element is also a parity bug).
    for (key, rcount) in &r {
        let gcount = g.get(key).copied().unwrap_or(0);
        if *rcount > gcount {
            rust_gained.push(format!(
                "  <{}> under <{}>: rust={rcount} oracle={gcount}",
                key.1, key.0
            ));
        }
    }
    assert!(
        unexpected_drops.is_empty(),
        "{name}: Rust DROPPED modeled element(s) the canonical golden has (not in the \
         KNOWN_UNMODELED_ELEMENTS allowlist, a round-trip regression):\n{}",
        unexpected_drops.join("\n")
    );
    assert!(
        rust_gained.is_empty(),
        "{name}: Rust FABRICATED element(s) absent from the canonical golden:\n{}",
        rust_gained.join("\n")
    );
}

#[test]
fn corpus_matches_frozen_goldens() {
    let root = golden_root();
    if !root.is_dir() {
        eprintln!("skipping: tests/golden not reachable (packaged crate)");
        return;
    }
    let mut checked = 0usize;
    let mut used_allow: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(&root).expect("read golden dir") {
        let dir = entry.expect("dir entry").path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with('_') {
            continue; // `_enum_oracle` / `_invalid_oracle`: oracle channels, not corpus-document goldens
        }
        let canonical = std::fs::read_to_string(dir.join("canonical.xml"))
            .unwrap_or_else(|e| panic!("{name}: missing golden canonical.xml: {e}"));
        // The JSON projection is compared byte-for-byte in `json_convert.rs`.
        assert!(
            dir.join("model.json").is_file(),
            "{name}: missing golden model.json"
        );

        // (a) XML round-trip parity against the committed canonical contract.
        let doc = Hcdf::from_xml_str(&canonical)
            .unwrap_or_else(|e| panic!("{name}: Rust failed to parse the canonical golden: {e}"));
        let reserialized = doc
            .to_xml_string()
            .unwrap_or_else(|e| panic!("{name}: Rust serialize failed: {e}"));
        // Self-consistency (idempotency) check, NOT parity: proves Rust serialize/parse is a fixpoint.
        let doc2 = Hcdf::from_xml_str(&reserialized)
            .unwrap_or_else(|e| panic!("{name}: Rust re-parse of its own output failed: {e}"));
        assert_eq!(doc, doc2, "{name}: Rust serialize/parse is not idempotent");
        // Rust's serialization must reproduce the golden's element content.
        assert_element_parity_against_golden(&name, &canonical, &reserialized, &mut used_allow);

        // (c) Semantic validation must reproduce the complete committed issue-line contract.
        let golden_issues = std::fs::read_to_string(dir.join("issues.txt"))
            .unwrap_or_else(|e| panic!("{name}: missing golden issues.txt: {e}"));
        let rust_issues = rust_issue_lines(&doc);
        assert_eq!(
            rust_issues, golden_issues,
            "{name}: Rust validate_semantic output does not match the frozen issues.txt"
        );

        checked += 1;
    }
    assert!(
        checked >= 3,
        "expected to exercise the golden corpus, checked {checked}"
    );
    // Keep the allowlist honest: every entry must be a drop something in the corpus actually exercises.
    // As model-completion proceeds, drops disappear and stale entries here MUST be removed.
    let stale: Vec<String> = KNOWN_UNMODELED_ELEMENTS
        .iter()
        .filter(|pair| !used_allow.contains(&(pair.0.to_string(), pair.1.to_string())))
        .map(|(p, c)| format!("  (\"{p}\", \"{c}\")"))
        .collect();
    assert!(
        stale.is_empty(),
        "KNOWN_UNMODELED_ELEMENTS has stale entries not dropped by any corpus file (remove them; the \
         model may now capture these):\n{}",
        stale.join("\n")
    );
}

/// Enum parity against the frozen acceptance and rejection contract (assertion (b)).
///
/// accept channel: every accepted corpus doc (`load.txt == accept`) must parse in Rust
/// (`from_xml_str` succeeds because every typed enum value is valid) AND yield ZERO `E_ENUM_VALUE` from
/// `validate_enums` (no false positives over the model-untyped sensor-category `@type` values either).
/// reject channel: each `_enum_oracle/<fixture>.xml` must be rejected by the typed model: at parse for
/// a typed slot, or via `validate_enums` (exactly that `(element, attr)` flagged `E_ENUM_VALUE`) for the
/// model-untyped sensor-category `@type` slots.
#[test]
fn enum_validation_matches_frozen_contract() {
    let root = golden_root();
    if !root.is_dir() {
        eprintln!("skipping: tests/golden not reachable (packaged crate)");
        return;
    }

    // Accept channel: committed corpus documents must produce no enum finding.
    let mut accepted = 0usize;
    for entry in std::fs::read_dir(&root).expect("read golden dir") {
        let dir = entry.expect("dir entry").path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with('_') {
            continue; // oracle channels (`_enum_oracle` / `_invalid_oracle`), not corpus docs
        }
        let load = std::fs::read_to_string(dir.join("load.txt")).unwrap_or_else(|e| {
            panic!("{name}: missing golden load.txt (acceptance decision): {e}")
        });
        assert_eq!(
            load.trim(),
            "accept",
            "{name}: golden load decision is not 'accept' (fixture/corpus mismatch)"
        );
        let canonical = std::fs::read_to_string(dir.join("canonical.xml"))
            .unwrap_or_else(|e| panic!("{name}: missing golden canonical.xml: {e}"));
        // The typed model must PARSE every accepted corpus doc, i.e. every enum literal it carries is a
        // valid variant (a parse failure here would be a false-positive reject by the typed model).
        Hcdf::from_xml_str(&canonical).unwrap_or_else(|e| {
            panic!("{name}: accepted golden failed to parse (enum false reject): {e}")
        });
        let enum_issues: Vec<_> = hcdformat::validate_enums(&canonical)
            .expect("enum validation runs")
            .into_iter()
            .filter(|i| i.code == "E_ENUM_VALUE")
            .collect();
        assert!(
            enum_issues.is_empty(),
            "{name}: accepted golden produced enum value finding(s) (false positive): {:?}",
            enum_issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        accepted += 1;
    }
    assert!(
        accepted >= 3,
        "expected to exercise the accept corpus, checked {accepted}"
    );

    // Reject channel: frozen corrupted fixtures must still be rejected.
    let enum_dir = root.join("_enum_oracle");
    assert!(
        enum_dir.is_dir(),
        "missing tests/golden/_enum_oracle frozen contract"
    );
    let mut rejected = 0usize;
    for entry in std::fs::read_dir(&enum_dir).expect("read _enum_oracle dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("oracle") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let oracle =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{stem}: read oracle: {e}"));
        // `decision<TAB>element<TAB>attr<TAB>bad_value<TAB>python_error`
        let fields: Vec<&str> = oracle.trim_end().splitn(5, '\t').collect();
        assert_eq!(fields.len(), 5, "{stem}: malformed oracle line: {oracle:?}");
        let (decision, element, attr, bad_value) = (fields[0], fields[1], fields[2], fields[3]);
        assert_eq!(
            decision, "reject",
            "{stem}: every enum fixture must be a reject oracle"
        );

        let xml = std::fs::read_to_string(enum_dir.join(format!("{stem}.xml")))
            .unwrap_or_else(|e| panic!("{stem}: missing fixture xml: {e}"));

        // Dispatch on whether this slot is one the typed model CANNOT enforce (the sensor-category
        // `@type` slots still in `ENUM_ATTRS`) vs a typed `Option<EnumTy>` field rejected at parse.
        let key = if attr == "#text" {
            "#text".to_string()
        } else {
            format!("@{attr}")
        };
        if valid_values_for(element, &key).is_some() {
            // Model-untyped slot: `from_xml_str` parses (String accepts anything), but `validate_enums`
            // must flag exactly this slot, matching the oracle's load-time reject.
            let issues = hcdformat::validate_enums(&xml).expect("enum validation runs");
            let enum_issues: Vec<_> = issues.iter().filter(|i| i.code == "E_ENUM_VALUE").collect();
            assert_eq!(
                enum_issues.len(),
                1,
                "{stem}: oracle REJECTED on {element}/{attr}={bad_value:?}; Rust E_ENUM_VALUE issues: {:?}",
                enum_issues.iter().map(|i| &i.message).collect::<Vec<_>>()
            );
            let msg = &enum_issues[0].message;
            let slot_ok = if attr == "#text" {
                msg.contains(&format!("{element} text"))
            } else {
                msg.contains(&format!("{element}/@{attr}"))
            };
            assert!(
                slot_ok && msg.contains(bad_value),
                "{stem}: Rust enum message does not name the oracle's rejected slot \
                 ({element}/{attr}={bad_value:?}): {msg}"
            );
        } else {
            // Typed slot: serde cannot deserialize the bad variant, so `from_xml_str` FAILS at parse,
            // exactly Python's load-time `ValueError`. (And the error mentions the bad value.)
            let err = Hcdf::from_xml_str(&xml).expect_err(&format!(
                "{stem}: oracle REJECTED {element}/{attr}={bad_value:?} at load, but Rust PARSED it"
            ));
            assert!(
                err.to_string().contains(bad_value),
                "{stem}: parse error should mention the bad enum value {bad_value:?}: {err}"
            );
        }
        rejected += 1;
    }
    assert!(
        rejected >= 4,
        "expected the enum reject corpus, checked {rejected}"
    );
}

/// A bad value in a typed enum slot (`comp/@role`, `joint/@type`) makes `from_xml_str` fail at parse.
/// The typed model is the single source of truth, so the bad value is unrepresentable rather than
/// merely flagged later.
#[test]
fn out_of_enum_attribute_is_rejected_at_parse() {
    let bad_role = r#"<hcdf name="x" version="1.0"><comp name="c" role="typo"/></hcdf>"#;
    let err = Hcdf::from_xml_str(bad_role).expect_err("bad comp/@role must fail to parse");
    assert!(err.to_string().contains("typo"), "role parse error: {err}");

    let bad_type = r#"<hcdf name="x" version="1.0"><comp name="a"/><comp name="b"/>
        <joint name="j" type="bogus"><parent comp="a"/><child comp="b"/></joint></hcdf>"#;
    let err = Hcdf::from_xml_str(bad_type).expect_err("bad joint/@type must fail to parse");
    assert!(err.to_string().contains("bogus"), "type parse error: {err}");

    // valid replacements parse cleanly (no false reject).
    let good_role = bad_role.replace("typo", "sensor");
    assert!(Hcdf::from_xml_str(&good_role).is_ok());
    let good_type = bad_type.replace("bogus", "revolute");
    assert!(Hcdf::from_xml_str(&good_type).is_ok());
}

/// The one family the typed model can't reject at parse (the ten sensor-category `@type` slots that
/// share `SensorCategory`) is still rejected by `validate_enums`.
/// `enum_validation_matches_frozen_contract` covers the committed fixture set.
#[test]
fn out_of_enum_sensor_category_type_is_rejected_by_validate() {
    let bad = r#"<hcdf name="x" version="1.0"><comp name="c"><sensor name="s">
        <em type="x-ray-vision"/></sensor></comp></hcdf>"#;
    // The model parses it (the slot is `Option<String>`), but `validate_enums` flags exactly it.
    assert!(
        Hcdf::from_xml_str(bad).is_ok(),
        "model-untyped slot parses as String"
    );
    let issues = hcdformat::validate_enums(bad).unwrap();
    let codes: Vec<&str> = issues.iter().map(|i| i.code.as_str()).collect();
    assert_eq!(
        codes,
        ["E_ENUM_VALUE"],
        "the bad sensor @type must be flagged"
    );
    assert!(
        issues[0].message.contains("em/@type") && issues[0].message.contains("x-ray-vision"),
        "em type message: {}",
        issues[0].message
    );
    // a valid replacement yields no enum issue (no false positive)
    let good = bad.replace("x-ray-vision", "mag");
    assert!(hcdformat::validate_enums(&good).unwrap().is_empty());
}

/// `<repo>` root (the crate sits at `<repo>/rust/hcdformat-rs`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Invalid-corpus parity against the frozen rejection and semantic-code contract.
///
/// Each `_invalid_oracle/<corpus>__<fixture>.oracle` line covers a fixture in
/// `tests/validator/invalid/*` or `tests/invalid/*`:
///   * `load-reject\t<detail>` requires `Hcdf::from_xml_str` to fail.
///   * `accept\t<level:code,...>` requires a successful parse and exactly that sorted semantic-code
///     set. The set is empty for schema-only faults outside `validate_semantic`.
#[test]
fn invalid_corpus_matches_frozen_contract() {
    let oracle_dir = golden_root().join("_invalid_oracle");
    if !oracle_dir.is_dir() {
        eprintln!("skipping: tests/golden/_invalid_oracle not reachable");
        return;
    }
    let repo = repo_root();
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&oracle_dir).expect("read _invalid_oracle dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("oracle") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        // `<corpus>__<fixture>`: map the corpus tag back to its source directory.
        let (corpus, fixture) = stem
            .split_once("__")
            .unwrap_or_else(|| panic!("{stem}: oracle name must be <corpus>__<fixture>"));
        let src_dir = match corpus {
            "validator-invalid" => repo.join("tests/validator/invalid"),
            "xsd-invalid" => repo.join("tests/invalid"),
            other => panic!("{stem}: unknown invalid corpus {other:?}"),
        };
        let src = src_dir.join(format!("{fixture}.hcdf"));
        let xml = std::fs::read_to_string(&src)
            .unwrap_or_else(|e| panic!("{stem}: missing source fixture {}: {e}", src.display()));
        let line =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{stem}: read oracle: {e}"));
        let line = line.trim_end_matches('\n');
        let (decision, detail) = line.split_once('\t').unwrap_or((line, ""));

        match decision {
            "load-reject" => {
                // The oracle rejected at LOAD; Rust's typed parser must FAIL too (the bad enum is
                // unrepresentable, malformed XML is a parse error). Decision parity is the bar.
                let err = Hcdf::from_xml_str(&xml).err().unwrap_or_else(|| {
                    panic!("{stem}: oracle load-rejected ({detail:?}) but Rust PARSED it")
                });
                // For an enum reject the Python error names the bad value (`'X' is not a valid ...`);
                // assert the Rust parse error mentions that same token so it's the SAME reject, not a
                // coincidental different failure. (Malformed-XML rejects carry no such token.)
                if let Some(tok) = detail.split('\'').nth(1) {
                    if detail.contains("is not a valid") {
                        assert!(
                            err.to_string().contains(tok),
                            "{stem}: Rust parse error should name the oracle's bad value {tok:?}: {err}"
                        );
                    }
                }
            }
            "accept" => {
                // Accepted fixtures must parse and reproduce the frozen semantic code set.
                let doc = Hcdf::from_xml_str(&xml)
                    .unwrap_or_else(|e| panic!("{stem}: accepted fixture failed to parse: {e}"));
                let rust = rust_semantic_codes(&doc);
                assert_eq!(
                    rust, detail,
                    "{stem}: Rust validate_semantic code set differs from the frozen contract"
                );
            }
            other => panic!("{stem}: unknown oracle decision {other:?} in line {line:?}"),
        }
        checked += 1;
    }
    // Both corpora present (10 validator-invalid + 10 xsd-invalid = 20); guard against an empty/partial
    // oracle dir silently passing.
    assert!(
        checked >= 15,
        "expected the invalid corpora oracle set, checked {checked}"
    );
}

/// E_CYCLE and E_LIMIT_RANGE FULL MESSAGE parity: the two cases the broader corpus/invalid-corpus
/// oracles only compare on `(level, code)`.
///
/// The invalid corpus records E_CYCLE as `accept\terror:E_CYCLE` (code only) because Python's DFS once
/// started from a hash-seeded `set`, so its raw cycle-path string was nondeterministic across
/// `PYTHONHASHSEED` (e.g. the 3-cycle rendered `a -> b -> c -> a` under seed 1 but `c -> a -> b -> c`
/// otherwise). Both `hcdfdom.validate._canon_cycle` and `validate.rs::canon_cycle` now canonicalize the
/// path (rotate to the lexicographically-min node), so the message is deterministic AND identical on
/// both sides, which this pins. The expected strings were taken from CPython `hcdfdom.validate` after
/// the canonicalization and are stable across hash seeds.
///
/// E_LIMIT_RANGE pins the scientific-notation float formatting: `py_float_repr` reproduces CPython
/// `str(float())` (including `1e+20`, `1e-05`, `inf` for an overflowed literal), which Rust's native
/// `{}`/`{:e}` do not.
#[test]
fn cycle_and_limit_messages_match_python_byte_for_byte() {
    fn message_for(xml: &str, code: &str) -> Vec<String> {
        let doc = Hcdf::from_xml_str(xml).expect("fixture parses");
        let mut msgs: Vec<String> = hcdformat::validate_semantic(&doc)
            .into_iter()
            .filter(|i| i.code == code)
            .map(|i| i.message)
            .collect();
        msgs.sort();
        msgs
    }

    // The committed cycle fixture: 2-cycle a <-> b.
    let cycle_src = repo_root().join("tests/validator/invalid/cycle.hcdf");
    let cycle_xml = std::fs::read_to_string(&cycle_src).expect("read cycle.hcdf");
    assert_eq!(
        message_for(&cycle_xml, "E_CYCLE"),
        vec!["kinematic cycle through comps: a -> b -> a".to_string()],
    );

    // A 3-cycle and a doc with two independent cycles, canonicalized + de-duped + sorted, so the
    // discovery order (Python's `set` start) cannot change the result.
    let three = r#"<hcdf version="1.0" name="c3"><comp name="a"/><comp name="b"/><comp name="c"/>
        <joint name="j1" type="fixed"><parent comp="a"/><child comp="b"/></joint>
        <joint name="j2" type="fixed"><parent comp="b"/><child comp="c"/></joint>
        <joint name="j3" type="fixed"><parent comp="c"/><child comp="a"/></joint></hcdf>"#;
    assert_eq!(
        message_for(three, "E_CYCLE"),
        vec!["kinematic cycle through comps: a -> b -> c -> a".to_string()],
    );
    let two = r#"<hcdf version="1.0" name="c2"><comp name="a"/><comp name="b"/><comp name="c"/>
        <comp name="x"/><comp name="y"/>
        <joint name="j1" type="fixed"><parent comp="a"/><child comp="b"/></joint>
        <joint name="j2" type="fixed"><parent comp="b"/><child comp="c"/></joint>
        <joint name="j3" type="fixed"><parent comp="c"/><child comp="a"/></joint>
        <joint name="j4" type="fixed"><parent comp="x"/><child comp="y"/></joint>
        <joint name="j5" type="fixed"><parent comp="y"/><child comp="x"/></joint></hcdf>"#;
    assert_eq!(
        message_for(two, "E_CYCLE"),
        vec![
            "kinematic cycle through comps: a -> b -> c -> a".to_string(),
            "kinematic cycle through comps: x -> y -> x".to_string(),
        ],
    );

    // E_LIMIT_RANGE: scientific-notation + overflow limits format as CPython str(float()).
    let lim = |lo: &str, hi: &str| {
        format!(
            r#"<hcdf version="1.0" name="l"><comp name="a"/><comp name="b"/>
            <joint name="j" type="revolute"><parent comp="a"/><child comp="b"/>
            <axis xyz="1 0 0"/><limit lower="{lo}" upper="{hi}"/></joint></hcdf>"#
        )
    };
    assert_eq!(
        message_for(&lim("1e20", "1"), "E_LIMIT_RANGE"),
        vec!["joint 'j' (revolute): limit upper (1.0) < lower (1e+20)".to_string()],
    );
    assert_eq!(
        message_for(&lim("1e-5", "-1"), "E_LIMIT_RANGE"),
        vec!["joint 'j' (revolute): limit upper (-1.0) < lower (1e-05)".to_string()],
    );
    assert_eq!(
        message_for(&lim("1e400", "1"), "E_LIMIT_RANGE"),
        vec!["joint 'j' (revolute): limit upper (1.0) < lower (inf)".to_string()],
    );
}
