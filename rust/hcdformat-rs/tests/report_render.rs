//! Frozen render tests for the profile report and the URDF-export loss manifest.
//!
//! The renders (flat text, grouped markdown, pretty JSON) moved out of the CLI into the core so the
//! CLI and the PyO3 binding share ONE renderer. These fixtures pin every render shape byte-for-byte to
//! the output the Python renderers produced (the wheel's current output), the canonical form both
//! surfaces now emit. Every expected string here was captured from CPython so the JSON matches
//! `json.dumps(value, indent=2)` exactly, including the `ensure_ascii=True` `\uXXXX` escaping.
#![cfg(feature = "urdf")]

use hcdformat::to_urdf::DEFAULT_LOSS_TITLE;
use hcdformat::{Finding, Issue, Level, LossManifest, ProfileReport, Tier};

fn loss(items: &[(&str, &str)]) -> LossManifest {
    LossManifest {
        items: items
            .iter()
            .map(|(c, d)| (c.to_string(), d.to_string()))
            .collect(),
    }
}

// ── LossManifest ────────────────────────────────────────────────────────────────────────────────

#[test]
fn loss_empty_shapes() {
    let m = loss(&[]);
    assert_eq!(m.text(), "");
    assert_eq!(
        m.to_json(),
        "{\n  \"total\": 0,\n  \"categories\": {},\n  \"items\": []\n}"
    );
    assert_eq!(
        m.markdown(DEFAULT_LOSS_TITLE),
        "# HCDF → URDF loss manifest\n\n_No losses: the document exports to URDF without dropping content._\n"
    );
}

#[test]
fn loss_multi_category_text() {
    let m = loss(&[
        ("annotation", "doc description dropped"),
        ("joint", "screw joint j1 -> fixed"),
        ("annotation", "license dropped"),
    ]);
    assert_eq!(
        m.text(),
        "[annotation] doc description dropped\n[joint] screw joint j1 -> fixed\n[annotation] license dropped"
    );
}

#[test]
fn loss_multi_category_json_keeps_insertion_order() {
    let m = loss(&[
        ("annotation", "doc description dropped"),
        ("joint", "screw joint j1 -> fixed"),
        ("annotation", "license dropped"),
    ]);
    let expected = r#"{
  "total": 3,
  "categories": {
    "annotation": [
      "doc description dropped",
      "license dropped"
    ],
    "joint": [
      "screw joint j1 -> fixed"
    ]
  },
  "items": [
    {
      "category": "annotation",
      "detail": "doc description dropped"
    },
    {
      "category": "joint",
      "detail": "screw joint j1 -> fixed"
    },
    {
      "category": "annotation",
      "detail": "license dropped"
    }
  ]
}"#;
    assert_eq!(m.to_json(), expected);
}

#[test]
fn loss_multi_category_markdown_sorts_categories() {
    let m = loss(&[
        ("annotation", "doc description dropped"),
        ("joint", "screw joint j1 -> fixed"),
        ("annotation", "license dropped"),
    ]);
    let expected = "# HCDF → URDF loss manifest\n\n**3 loss item(s)** across 2 categories.\n\n## annotation (2)\n- doc description dropped\n- license dropped\n\n## joint (1)\n- screw joint j1 -> fixed\n";
    assert_eq!(m.markdown(DEFAULT_LOSS_TITLE), expected);
}

#[test]
fn loss_single_category_markdown_is_singular() {
    let m = loss(&[("annotation", "only one")]);
    let expected =
        "# HCDF → URDF loss manifest\n\n**1 loss item(s)** across 1 category.\n\n## annotation (1)\n- only one\n";
    assert_eq!(m.markdown(DEFAULT_LOSS_TITLE), expected);
}

#[test]
fn loss_json_escapes_like_python() {
    let m = loss(&[("an\"no", "tab\there \\ back \n newline")]);
    let expected = r#"{
  "total": 1,
  "categories": {
    "an\"no": [
      "tab\there \\ back \n newline"
    ]
  },
  "items": [
    {
      "category": "an\"no",
      "detail": "tab\there \\ back \n newline"
    }
  ]
}"#;
    assert_eq!(m.to_json(), expected);
}

#[test]
fn loss_json_escapes_non_ascii_as_ensure_ascii() {
    // CPython `json.dumps` defaults to `ensure_ascii=True`: every non-ASCII scalar becomes a `\uXXXX`
    // unit (a surrogate pair above U+FFFF), lowercase hex; ASCII controls keep their short escapes.
    let m = loss(&[("moteur", "corriente é ° µ → 😀 tab\tq")]);
    // Each `é` below is the literal six-character escape CPython emits (written with a doubled
    // backslash in this ordinary string literal), and `\t` stays a short escape.
    let expected = "{\n  \"total\": 1,\n  \"categories\": {\n    \"moteur\": [\n      \
        \"corriente \\u00e9 \\u00b0 \\u00b5 \\u2192 \\ud83d\\ude00 tab\\tq\"\n    ]\n  },\n  \
        \"items\": [\n    {\n      \"category\": \"moteur\",\n      \"detail\": \
        \"corriente \\u00e9 \\u00b0 \\u00b5 \\u2192 \\ud83d\\ude00 tab\\tq\"\n    }\n  ]\n}";
    assert_eq!(m.to_json(), expected);
}

// ── ProfileReport ───────────────────────────────────────────────────────────────────────────────

fn finding(tier: Tier, code: &str, detail: &str) -> Finding {
    Finding {
        tier,
        code: code.to_string(),
        detail: detail.to_string(),
    }
}

fn issue(level: Level, code: &str, message: &str) -> Issue {
    Issue {
        level,
        code: code.to_string(),
        message: message.to_string(),
    }
}

#[test]
fn profile_identity_empty_shapes() {
    let r = ProfileReport {
        classification: Tier::Identity,
        findings: vec![],
        loss: loss(&[]),
        issues: vec![],
    };
    let json = r#"{
  "classification": "IN-PROFILE-IDENTITY",
  "meaning": "Exports to clean valid URDF with a value-exact round-trip; nothing dropped.",
  "in_profile": true,
  "findings": [],
  "validator_errors": [],
  "loss_manifest": {
    "total": 0,
    "categories": {},
    "items": []
  }
}"#;
    assert_eq!(r.to_json(), json);
    assert_eq!(
        r.markdown(),
        "# HCDF-URDF profile: IN-PROFILE-IDENTITY\n\nExports to clean valid URDF with a value-exact round-trip; nothing dropped.\n\n## Field-level loss manifest\n\n_No field-level losses._\n"
    );
}

fn full_report() -> ProfileReport {
    ProfileReport {
        classification: Tier::OutOfProfile,
        findings: vec![
            finding(
                Tier::OutOfProfile,
                "P_LOOP",
                "joint 'j1': loop-closure (URDF is tree-only)",
            ),
            finding(Tier::WithTransform, "P_BODY_FRD", "body-frame FRD"),
            finding(
                Tier::OutOfProfile,
                "P_SURFACE",
                "comp 'c1' collision 'x': <surface> contact physics",
            ),
        ],
        loss: loss(&[
            ("joint", "screw"),
            ("annotation", "desc"),
            ("joint", "backlash"),
        ]),
        issues: vec![
            issue(Level::Error, "E_CYCLE", "cycle detected"),
            issue(Level::Warning, "W_X", "ignored warning"),
        ],
    }
}

#[test]
fn profile_full_json_matches_python() {
    let expected = r#"{
  "classification": "OUT-OF-PROFILE",
  "meaning": "Exporting to URDF drops model-significant content; the URDF is a lossy projection.",
  "in_profile": false,
  "findings": [
    {
      "tier": "OUT-OF-PROFILE",
      "code": "P_LOOP",
      "detail": "joint 'j1': loop-closure (URDF is tree-only)"
    },
    {
      "tier": "IN-PROFILE-WITH-FRAME-TRANSFORM",
      "code": "P_BODY_FRD",
      "detail": "body-frame FRD"
    },
    {
      "tier": "OUT-OF-PROFILE",
      "code": "P_SURFACE",
      "detail": "comp 'c1' collision 'x': <surface> contact physics"
    }
  ],
  "validator_errors": [
    "[error] E_CYCLE: cycle detected"
  ],
  "loss_manifest": {
    "total": 3,
    "categories": {
      "joint": [
        "screw",
        "backlash"
      ],
      "annotation": [
        "desc"
      ]
    },
    "items": [
      {
        "category": "joint",
        "detail": "screw"
      },
      {
        "category": "annotation",
        "detail": "desc"
      },
      {
        "category": "joint",
        "detail": "backlash"
      }
    ]
  }
}"#;
    assert_eq!(full_report().to_json(), expected);
}

#[test]
fn profile_full_markdown_matches_python() {
    let expected = "# HCDF-URDF profile: OUT-OF-PROFILE\n\nExporting to URDF drops model-significant content; the URDF is a lossy projection.\n\n## Out-of-profile drivers (2)\n- `P_LOOP` joint 'j1': loop-closure (URDF is tree-only)\n- `P_SURFACE` comp 'c1' collision 'x': <surface> contact physics\n\n## Transform / bake needed (1)\n- `P_BODY_FRD` body-frame FRD\n\n## Validator errors (1)\n- [error] E_CYCLE: cycle detected\n\n## Field-level loss manifest\n\n**3 loss item(s)** across 2 categories.\n\n## annotation (1)\n- desc\n\n## joint (2)\n- screw\n- backlash\n";
    assert_eq!(full_report().markdown(), expected);
}
