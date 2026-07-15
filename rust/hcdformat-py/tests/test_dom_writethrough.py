"""Acceptance test: in-place write-through DOM handles over the Rust `Hcdf`.

Proves the pyo3 aliasing model: edits to sub-objects several levels deep write through to the ONE owned
Rust document, and the comments side-channel survives a round-trip. Run via `maturin develop` then
`pytest`.
"""
import os

from hcdf.dom import Hcdf

REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CORPUS = os.path.join(REPO_ROOT, "examples", "test-minimal.hcdf")


def _load():
    with open(CORPUS, "r", encoding="utf-8") as fh:
        return Hcdf.loads(fh.read())


def test_shallow_scalar_writethrough():
    doc = _load()
    assert len(doc.comp) >= 1
    doc.comp[0].name = "spiked"
    out = doc.dumps()
    assert 'name="spiked"' in out
    # the fresh handle re-reads the SAME owned doc, not a copy
    assert doc.comp[0].name == "spiked"


def test_nested_scalar_writethrough():
    doc = _load()
    assert len(doc.comp[0].visual) >= 1
    doc.comp[0].visual[0].name = "x"
    out = doc.dumps()
    assert 'name="x"' in out
    assert doc.comp[0].visual[0].name == "x"


def test_append_child():
    doc = _load()
    before = len(doc.comp[0].visual)
    doc.comp[0].visual.append("appended_visual")
    assert len(doc.comp[0].visual) == before + 1
    out = doc.dumps()
    assert 'name="appended_visual"' in out
    # locators to pre-existing elements stay valid after an append (index-stable grow)
    assert doc.comp[0].visual[before].name == "appended_visual"


def test_nested_option_scalar_on_sensor():
    doc = _load()
    doc.comp[0].sensor.append("temp0")
    idx = len(doc.comp[0].sensor) - 1
    assert doc.comp[0].sensor[idx].name == "temp0"
    doc.comp[0].sensor[idx].name = "temp_renamed"
    out = doc.dumps()
    assert 'name="temp_renamed"' in out
    # clearing an Option<String> scalar writes through as an absent attribute
    doc.comp[0].sensor[idx].name = None
    assert doc.comp[0].sensor[idx].name is None


def test_roundtrip_preserves_comment_sidechannel():
    with open(CORPUS, "r", encoding="utf-8") as fh:
        src = fh.read()
    assert "<!--" in src  # corpus carries an XML comment
    once = Hcdf.loads(src).dumps()
    twice = Hcdf.loads(once).dumps()
    # unedited round-trip is a fixpoint AND the comment side-channel survives
    assert once == twice
    assert "Comp 1: board with switch" in once
