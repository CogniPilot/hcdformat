"""The public `hcdf.*` submodule surface a Python user sees.

The extension ships as `hcdf/__init__.abi3.so`, so `import hcdf` loads the binding as the package: the
five submodules (`dom` / `urdf` / `sdf` / `io` / `assets`) are registered on it at init and inserted into
`sys.modules`, so both `import hcdf.urdf` and `from hcdf.urdf import ...` resolve cold in a fresh
interpreter. This suite proves that shape: the submodule registration, the `#[pyclass]` report types
whose render methods reproduce the CLI-backed core renders, one representative call per mapped module,
the `main()` console entry point, and the cold-import guarantees.

Run against the extracted wheel:  PYTHONPATH=<extracted-wheel-dir> pytest
"""
import os
import subprocess
import sys

import pytest

import hcdf as ext

REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
EXAMPLE = os.path.join(REPO_ROOT, "tests", "valid", "articulated-arm.hcdf")

MIN_URDF = '<robot name="r"><link name="base_link"/></robot>'
MIN_SDF = '<sdf version="1.6"><model name="m"><link name="l"/></model></sdf>'

SUBMODULES = ("dom", "urdf", "sdf", "io", "assets")


def _example_doc():
    if not os.path.isfile(EXAMPLE):
        pytest.skip(f"corpus not present: {EXAMPLE}")
    return ext.dom.load(EXAMPLE)


def _imported_doc():
    """A document from importing the minimal URDF: guaranteed to carry no `<include>` and no meshes."""
    doc, _notes = ext.urdf.from_urdf(MIN_URDF)
    return doc


# ── submodule registration ───────────────────────────────────────────────────────────────────────────
def test_submodules_present_as_attributes():
    for name in SUBMODULES:
        assert hasattr(ext, name), name
        assert getattr(ext, name).__name__ == f"{ext.__name__}.{name}"


def test_submodules_in_sys_modules():
    for name in SUBMODULES:
        full = f"{ext.__name__}.{name}"
        assert full in sys.modules, full
        assert sys.modules[full] is getattr(ext, name)


def test_top_level_entries():
    assert ext.version == "1.0.0"
    assert ext.__version__ == "1.0.0"
    assert ext.HCDF_VERSION == "1.0"
    assert callable(ext.main)


# ── hcdf.dom ───────────────────────────────────────────────────────────────────────────────────────────
def test_dom_load_dumps_loads_roundtrip():
    doc = _example_doc()
    xml = ext.dom.dumps(doc)
    assert "hcdf" in xml
    again = ext.dom.loads(xml)
    assert ext.dom.dumps(again) == xml
    # loads accepts bytes as well as str
    from_bytes = ext.dom.loads(xml.encode("utf-8"))
    assert ext.dom.dumps(from_bytes) == xml


def test_dom_hcdf_alias():
    assert isinstance(ext.dom.Hcdf, type)
    assert hasattr(ext.dom.Hcdf, "loads")
    assert hasattr(ext.dom.Hcdf, "load")


# ── hcdf.urdf ──────────────────────────────────────────────────────────────────────────────────────────
def test_urdf_from_to_profile():
    doc, notes = ext.urdf.from_urdf(MIN_URDF)
    assert isinstance(notes, list)
    urdf_xml, loss = ext.urdf.to_urdf(doc)
    assert "<robot" in urdf_xml
    assert isinstance(loss, ext.urdf.LossManifest)
    rep = ext.urdf.check_profile(doc)
    assert isinstance(rep, ext.urdf.ProfileReport)
    assert rep.classification in (
        ext.urdf.IDENTITY,
        ext.urdf.WITH_TRANSFORM,
        ext.urdf.OUT_OF_PROFILE,
    )


def test_urdf_from_bytes_source():
    doc, _notes = ext.urdf.from_urdf(MIN_URDF.encode("utf-8"))
    assert "<comp" in ext.dom.dumps(doc) or "hcdf" in ext.dom.dumps(doc)


def test_urdf_from_xacro_mappings_change_output(tmp_path):
    # A .xacro `$(arg)` default names a link; a `mappings` override changes it, so the imported model
    # provably differs with and without the override, while omitting mappings (or an empty map) reproduces
    # the default expansion.
    xacro = tmp_path / "robot.xacro"
    xacro.write_text(
        '<?xml version="1.0"?>\n'
        '<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="t">\n'
        '  <xacro:arg name="side" default="left"/>\n'
        '  <link name="$(arg side)_wheel"/>\n'
        "</robot>\n"
    )
    default_doc, _ = ext.urdf.from_urdf(str(xacro))
    default_xml = ext.dom.dumps(default_doc)
    assert "left_wheel" in default_xml

    override_doc, _ = ext.urdf.from_urdf(str(xacro), mappings={"side": "right"})
    override_xml = ext.dom.dumps(override_doc)
    assert "right_wheel" in override_xml
    assert "left_wheel" not in override_xml
    assert override_xml != default_xml

    # Omitting mappings and an explicit empty map both reproduce the default expansion.
    empty_doc, _ = ext.urdf.from_urdf(str(xacro), mappings={})
    assert ext.dom.dumps(empty_doc) == default_xml


def test_expand_xacro_path_mappings_change_output(tmp_path):
    # The `expand_xacro_path` binding threads `mappings` into `$(arg)` expansion: the returned URDF
    # string names `left_wheel` by default and `right_wheel` under the override, while omitting mappings
    # (or an empty map) reproduces the default expansion byte-for-byte.
    xacro = tmp_path / "robot.xacro"
    xacro.write_text(
        '<?xml version="1.0"?>\n'
        '<robot xmlns:xacro="http://www.ros.org/wiki/xacro" name="t">\n'
        '  <xacro:arg name="side" default="left"/>\n'
        '  <link name="$(arg side)_wheel"/>\n'
        "</robot>\n"
    )
    default_urdf = ext.expand_xacro_path(str(xacro))
    assert "left_wheel" in default_urdf
    assert "right_wheel" not in default_urdf

    override_urdf = ext.expand_xacro_path(str(xacro), mappings={"side": "right"})
    assert "right_wheel" in override_urdf
    assert "left_wheel" not in override_urdf
    assert override_urdf != default_urdf

    # Omitting mappings and an explicit empty map both reproduce the default expansion.
    assert ext.expand_xacro_path(str(xacro), mappings={}) == default_urdf


def test_loss_manifest_render_methods():
    doc = _example_doc()
    _urdf_xml, loss = ext.urdf.to_urdf(doc)
    assert isinstance(loss.text(), str)
    assert isinstance(loss.categories(), dict)
    assert isinstance(loss.to_dict(), dict)
    assert isinstance(loss.markdown(), str)
    assert bool(loss) == (len(loss) > 0)
    # The pyclass render is byte-identical to the flat CLI-backed entry point on the same document.
    assert loss.to_json() == ext.loss_json(ext.dom.dumps(doc))
    assert loss.text() == ext.loss_text(ext.dom.dumps(doc))
    assert loss.markdown() == ext.loss_markdown(ext.dom.dumps(doc))


def test_loss_manifest_construct_and_add():
    man = ext.urdf.LossManifest()
    assert not man and len(man) == 0
    man.add("annotation", "a detail")
    assert man and len(man) == 1
    assert man.text() == "[annotation] a detail"
    assert man.categories() == {"annotation": ["a detail"]}


def test_profile_report_render_methods():
    doc = _example_doc()
    rep = ext.urdf.check_profile(doc)
    d = rep.to_dict()
    assert d["classification"] == rep.classification
    assert isinstance(rep.in_profile, bool)
    assert isinstance(rep.is_identity, bool)
    assert isinstance(rep.findings, list)
    assert isinstance(rep.issues, list)
    assert isinstance(rep.loss, ext.urdf.LossManifest)
    assert isinstance(rep.by_tier(ext.urdf.OUT_OF_PROFILE), list)
    assert isinstance(rep.nonbenign_losses, list)
    # Byte-identical to the flat CLI-backed renders on the same document.
    xml = ext.dom.dumps(doc)
    assert rep.to_json() == ext.profile_json(xml)
    assert rep.markdown() == ext.profile_markdown(xml)


def test_urdf_constants():
    assert ext.urdf.IDENTITY == "IN-PROFILE-IDENTITY"
    assert ext.urdf.WITH_TRANSFORM == "IN-PROFILE-WITH-FRAME-TRANSFORM"
    assert ext.urdf.OUT_OF_PROFILE == "OUT-OF-PROFILE"
    assert ext.urdf.ERROR == "error"
    assert ext.urdf.WARNING == "warning"
    assert ext.urdf.BENIGN_LOSS_CATEGORIES == frozenset({"annotation"})


def test_finding_and_issue_types():
    f = ext.urdf.Finding("OUT-OF-PROFILE", "P_X", "why")
    assert (f.tier, f.code, f.detail) == ("OUT-OF-PROFILE", "P_X", "why")
    i = ext.urdf.Issue("error", "E_X", "boom")
    assert str(i) == "[error] E_X: boom"


# ── hcdf.sdf ───────────────────────────────────────────────────────────────────────────────────────────
def test_sdf_from_to():
    doc, notes = ext.sdf.from_sdf(MIN_SDF)
    assert isinstance(notes, list)
    sdf_xml, loss = ext.sdf.to_sdf(doc)
    assert "<sdf" in sdf_xml
    # The SDF lane shares the URDF LossManifest type.
    assert isinstance(loss, ext.urdf.LossManifest)


# ── hcdf.io ────────────────────────────────────────────────────────────────────────────────────────────
def test_io_json_roundtrip():
    doc = _example_doc()
    obj = ext.io.to_json(doc)
    assert isinstance(obj, dict) and "hcdf" in obj
    # JSON is the lossless boundary for the model (XML comments live outside it), so the invariant is
    # JSON-level: re-serializing the document parsed back from JSON reproduces the same JSON.
    back = ext.io.from_json(obj)
    assert ext.io.to_json(back) == obj


def test_io_flatten_no_includes_is_identity():
    doc = _imported_doc()
    flat, notes = ext.io.flatten(doc)
    assert isinstance(notes, list)
    assert ext.dom.dumps(flat) == ext.dom.dumps(doc)


def test_io_reexports_are_the_dom_objects():
    assert ext.io.load is ext.dom.load
    assert ext.io.dump is ext.dom.dump
    assert ext.io.dumps is ext.dom.dumps


def test_io_pack_rejects_flatten_false(tmp_path):
    # pack(flatten=False) is an explicit error, not a silent flatten: the structure-preserving keep-live
    # pack is a separate pipeline this entry point does not expose. flatten=True and the default succeed.
    doc = _imported_doc()  # minimal URDF import: no <include>, no meshes
    out = tmp_path / "bundle.hcdfz"

    with pytest.raises(ValueError):
        ext.io.pack(doc, str(out), flatten=False)
    assert not out.exists()

    manifest = ext.io.pack(doc, str(out))
    assert manifest["bundle"] == str(out)
    assert out.is_file()

    out.unlink()
    manifest_true = ext.io.pack(doc, str(out), flatten=True)
    assert manifest_true["bundle"] == str(out)
    assert out.is_file()


# ── hcdf.assets ────────────────────────────────────────────────────────────────────────────────────────
def test_assets_content_and_resolve():
    sha = ext.assets.content_sha(b"hello")
    assert sha.startswith("sha256:")
    assert ext.assets.resolve_uri("") is None
    assert ext.assets.resolve_uri("package://nope/mesh.stl", ".") is None


def test_assets_baker_constructs():
    baker = ext.assets.Baker("out", base_dir=".", uri_prefix="assets")
    # An unresolvable uri returns None rather than raising.
    assert baker("package://nope/x.stl", None, "visual") is None


def test_assets_drop_baked_notes_passthrough():
    # With no baked pairs, every note is kept.
    notes = ["visual 'a' scale not applied", "unrelated note"]
    assert ext.assets.drop_baked_notes(notes, set()) == notes


# ── console script ─────────────────────────────────────────────────────────────────────────────────────
def test_main_help_and_version():
    assert ext.main(["hcdf", "--help"]) == 0
    assert ext.main(["hcdf", "--version"]) == 0


def test_main_reads_sys_argv():
    # A fresh interpreter running exactly what the `hcdf = hcdf:main` console script does:
    # `import hcdf; sys.exit(hcdf.main())`, with the args on sys.argv (main() reads them when argv=None).
    code = "import hcdf, sys; sys.argv = ['hcdf', '--help']; sys.exit(hcdf.main())"
    result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert "Usage:" in result.stdout


# ── cold submodule import in a fresh interpreter (the submodule is the FIRST import) ─────────────────────
# One representative public name per submodule, imported cold to prove `from hcdf.<sub> import <name>`
# triggers the parent-package extension init that registers the submodule.
_COLD_NAMES = {
    "dom": "load",
    "urdf": "check_profile",
    "sdf": "from_sdf",
    "io": "flatten",
    "assets": "content_sha",
}


@pytest.mark.parametrize("sub", SUBMODULES)
def test_cold_import_submodule_first(sub):
    # `import hcdf.<sub>` as the VERY FIRST import (no prior `import hcdf`): importing the parent must
    # run the extension init that inserts `hcdf.<sub>` into sys.modules.
    code = f"import hcdf.{sub} as m\nprint(m.__name__)\n"
    result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == f"hcdf.{sub}"


@pytest.mark.parametrize("sub", SUBMODULES)
def test_cold_from_import_submodule_first(sub):
    # `from hcdf.<sub> import <name>` as the VERY FIRST import in a fresh interpreter.
    name = _COLD_NAMES[sub]
    code = f"from hcdf.{sub} import {name}\nassert {name} is not None\nprint('ok')\n"
    result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "ok"
