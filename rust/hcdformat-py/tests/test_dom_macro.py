"""Acceptance: the `#[pydom]`-GENERATED write-through handles over the Rust `Hcdf`.

Extends the baseline handle tests to the new field-shape taxonomy categories the macro must cover on the subset:
enum get/set (+ validation), the VisualAppearance DATA-carrying enum, Vec<String>, keyword-renamed
attributes (`type`/`loop`), Option<NestedStruct> (get / ensure / clear), and comment survival across a
load -> edit -> dumps. All edits write through IN-PLACE to the single owned document.

Run: `maturin develop` then `pytest` (PYTHONPATH="" PYTEST_DISABLE_PLUGIN_AUTOLOAD=1 to skip the ROS
launch_testing plugin).
"""
import os

import pytest

from hcdf.dom import Hcdf

REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CORPUS = os.path.join(REPO_ROOT, "examples", "test-minimal.hcdf")


def _load():
    with open(CORPUS, "r", encoding="utf-8") as fh:
        return Hcdf.loads(fh.read())


# ── cat 6 + cat 9: C-like enum field, exposed under the keyword-renamed Python attr `type` ──────────

def test_enum_get_set_keyword_renamed_type():
    doc = _load()
    j = doc.joint[0]  # <joint name="shoulder_yaw" type="revolute">
    # `type_` (Rust) is exposed as Python `type` (cat 9 keyword rename)
    assert j.type == "revolute"
    j.type = "prismatic"
    assert j.type == "prismatic"
    assert 'type="prismatic"' in doc.dumps()
    # clearing an Option<enum> writes through as an absent attribute
    j.type = None
    assert j.type is None


def test_enum_set_validates():
    doc = _load()
    with pytest.raises(ValueError):
        doc.joint[0].type = "not-a-real-jointtype"


def test_enum_on_motor():
    doc = _load()
    m = doc.comp[0].motor[0]  # <motor type="bldc">
    assert m.type == "bldc"
    m.type = "stepper"
    assert doc.comp[0].motor[0].type == "stepper"


# ── cat 7: the DATA-CARRYING enum (VisualAppearance) via its variant-aware companion ────────────────

def test_data_enum_variant_and_payload():
    doc = _load()
    ap = doc.comp[0].visual[0].appearance  # <model uri= sha=> => the Model arm
    assert ap.variant == "model"
    assert ap.model_uri == "models/test-board.glb"
    assert ap.model_sha == "abc123def456"
    # edit payload in place, write-through
    ap.model_uri = "models/renamed.glb"
    assert "models/renamed.glb" in doc.dumps()
    assert doc.comp[0].visual[0].appearance.model_uri == "models/renamed.glb"


def test_data_enum_switch_variant():
    doc = _load()
    ap = doc.comp[0].visual[0].appearance
    ap.set_primitive()
    assert ap.variant == "primitive"
    # the model-arm accessors now error (wrong active variant)
    with pytest.raises(ValueError):
        _ = ap.model_uri
    ap.set_model()
    assert ap.variant == "model"


# ── cat 4: Vec<String> reached through a nested Option<Struct> (control_modes -> mode) ──────────────

def test_vec_string_control_modes():
    doc = _load()
    cm = doc.comp[0].motor[0].control_modes  # Option<ControlModes> present in corpus
    assert cm is not None
    modes = cm.mode
    assert len(modes) == 3
    assert modes[0] == "velocity"
    assert list(iter(modes)) == ["velocity", "position", "torque"]
    modes.append("current")
    assert len(doc.comp[0].motor[0].control_modes.mode) == 4
    assert "<mode>current</mode>" in doc.dumps()


# ── cat 5: Option<NestedStruct>: get returns handle-or-None, ensure creates, set None clears ─────────

def test_nested_option_ensure_and_clear():
    doc = _load()
    j = doc.joint[0]
    # no <loop> in the corpus joint => None
    assert j.loop is None
    lc = j.ensure_loop()          # create the nested struct, get a handle
    lc.predecessor = "linkA"
    lc.successor = "linkB"
    assert j.loop is not None
    assert j.loop.predecessor == "linkA"
    out = doc.dumps()
    assert "linkA" in out and "linkB" in out
    # clear it via set-None
    j.loop = None
    assert j.loop is None


def test_nested_option_on_optical_sensor():
    doc = _load()
    optical = doc.comp[0].sensor[1].optical[0]  # sensor tof0 -> <optical type="tof">
    assert optical.type == "tof"
    drv = optical.ensure_driver()
    drv.name = "tof_driver"
    assert doc.comp[0].sensor[1].optical[0].driver.name == "tof_driver"


# ── cat 8: serde-skip comments survive load -> edit -> dumps ─────────────────────────────────────

def test_comments_survive_edit():
    doc = _load()
    # a deep write-through edit
    doc.comp[0].motor[0].type = "servo"
    doc.joint[0].type = "continuous"
    out = doc.dumps()
    assert 'type="servo"' in out
    # the XML-comment side-channel (a #[serde(skip)] field) is preserved despite the edits
    assert "Comp 1: board with switch" in out


# ── stale-handle safety (index locator invalidated) surfaces as ValueError, not a panic ────────────

def test_stale_handle_errors():
    doc = _load()
    n = len(doc.comp)
    ghost = doc.comp[n - 1]
    # a handle one past the end never resolves -> clean ValueError
    with pytest.raises(ValueError):
        _ = doc.comp[n]
    # the valid handle still works
    assert isinstance(ghost.name, str)
