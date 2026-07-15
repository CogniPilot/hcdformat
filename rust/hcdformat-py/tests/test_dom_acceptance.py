"""Acceptance gate: the `#[pydom]` DOM handle over the WHOLE ~190-type model, proven against SIX
diverse real corpora, as an in-repo, standard-pytest test.

For each corpus this suite:
  * navigates DEEP from `comp` into every subhandle family (sensor / visual / joint / power /
    transmission / dynamic_surface) and reads a representative field of each TYPE category:
      - data-enum      : `VisualAppearance.variant`   (the sole payload-bearing enum, via the companion)
      - C-like enum     : `joint.type`                 (reached as a string)
      - Vec             : `hcdf.joint`, `hcdf.transmission`, `sensor.<category>`
      - nested Option   : `joint.parent`, `power_source.battery`/…
    Every required category must be exercised by at least one corpus (asserted in aggregate).
  * mutates one field, dumps, and asserts the edit PERSISTS in `dumps()` and the doc still parses +
    re-parses stably.
  * asserts an UNEDITED `loads(src).dumps()` is BYTE-LOSSLESS versus the canonical CLI form
    (`hcdf convert --from hcdf --to hcdf`), normalizing only the single trailing-newline delta that the
    CLI's file writer adds. `HCDF_CLI` must identify the CLI built from the same checkout. CI always
    supplies it; local runs without it skip only this parity comparison.

Also gates the VisualGeometry exposure: the primitive `<geometry>` that lives solely inside the
untagged `VisualAppearance` choice is now Python-navigable via the companion (`geometry_kind` /
`set_geometry` / `geometry_size`/`_radius`/`_length`/`_radii`) and round-trips through an edit.

Run (standard invocation):  PYTHONPATH="" PYTEST_DISABLE_PLUGIN_AUTOLOAD=1 pytest
"""
import os
import subprocess

import pytest

import hcdf
from hcdf.dom import Hcdf

REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))

# Six diverse corpora spanning the schema regions this suite navigates.
CORPORA = {
    "openarm-thor": "examples/openarm-thor.hcdf",
    "humanoid-mobile-base": "examples/humanoid-mobile-base.hcdf",
    "articulated-arm": "tests/valid/articulated-arm.hcdf",
    "power-solar-supercap": "tests/valid/power-solar-supercap.hcdf",
    "transmission-spring": "tests/valid/transmission-spring.hcdf",
    "surface-gripper": "tests/valid/surface-gripper.hcdf",
}


def _path(rel):
    return os.path.join(REPO_ROOT, rel)


def _read(rel):
    with open(_path(rel), "r", encoding="utf-8") as fh:
        return fh.read()


def _find_cli():
    """Return the explicitly selected current-checkout CLI, if supplied."""
    env = os.environ.get("HCDF_CLI")
    if env is None:
        return None
    if not os.path.isfile(env):
        raise AssertionError(f"HCDF_CLI does not identify a file: {env}")
    return env


def _cli_convert(cli, src_rel):
    """Canonical `hcdf convert --from hcdf --to hcdf` output for a corpus."""
    import tempfile

    with tempfile.TemporaryDirectory() as td:
        out = os.path.join(td, "out.hcdf")
        subprocess.run(
            [cli, "convert", _path(src_rel), out, "--from", "hcdf", "--to", "hcdf"],
            check=True,
            capture_output=True,
        )
        with open(out, "r", encoding="utf-8") as fh:
            return fh.read()


def _first_with(doc, attr, listlike=True):
    """First (comp, value) where comp.<attr> is a non-empty list / a present Option."""
    for c in doc.comp:
        v = getattr(c, attr)
        if listlike:
            if len(v) > 0:
                return c, v[0]
        elif v is not None:
            return c, v
    return None, None


# ── byte-losslessness: unedited load->dumps == canonical CLI convert (modulo trailing newline) ───────


@pytest.mark.parametrize("name", list(CORPORA))
def test_unedited_round_trip_is_byte_lossless_vs_cli(name):
    cli = _find_cli()
    if cli is None:
        pytest.skip("set HCDF_CLI to an hcdf binary built from the current checkout")
    rel = CORPORA[name]
    py = Hcdf.loads(_read(rel)).dumps()
    canon = _cli_convert(cli, rel)
    # The ONLY permitted delta is the single trailing newline the CLI's file writer appends.
    assert py.rstrip("\n") == canon.rstrip("\n"), f"{name}: DOM dumps() diverges from canonical CLI form"
    assert canon == py or canon == py + "\n", f"{name}: delta is more than one trailing newline"


# ── a mutation through the DOM handle persists in dumps() and re-parses ──────────────────────────────


@pytest.mark.parametrize("name", list(CORPORA))
def test_mutation_persists_and_reparses(name):
    rel = CORPORA[name]
    doc = Hcdf.loads(_read(rel))
    orig = doc.comp[0].name
    doc.comp[0].name = orig + "_R2C"
    out = doc.dumps()
    assert doc.comp[0].name == orig + "_R2C"
    assert (orig + "_R2C") in out
    assert hcdf.parse_ok(out), f"{name}: mutated doc no longer parses"
    assert hcdf.parse_ok(Hcdf.loads(out).dumps()), f"{name}: reparse unstable"


# ── deep navigation reaching a representative field of every type-category (asserted in aggregate) ───


def test_deep_navigation_covers_all_categories():
    subhandles = set()  # comp/sensor/visual/joint/power/transmission/surface
    fieldcats = set()   # data-enum / c-like-enum / vec / nested-option
    for name, rel in CORPORA.items():
        doc = Hcdf.loads(_read(rel))
        assert len(doc.comp) > 0, name
        subhandles.add("comp")

        # joint: C-like enum (type), Vec (hcdf.joint), nested Option (parent) + Pose subhandle
        if len(doc.joint) > 0:
            subhandles.add("joint")
            j = doc.joint[0]
            assert isinstance(j.type, str)
            fieldcats.add("c-like-enum(joint.type)")
            fieldcats.add("vec(hcdf.joint)")
            if j.parent is not None:
                fieldcats.add("nested-option(joint.parent)")
            _ = j.origin  # nested Pose Option

        # visual + the data-carrying enum companion
        _, v = _first_with(doc, "visual")
        if v is not None:
            subhandles.add("visual")
            assert v.appearance.variant in ("model", "primitive")
            fieldcats.add("data-enum(VisualAppearance.variant)")

        # sensor: category Vec lists + a nested subtype's field
        _, s = _first_with(doc, "sensor")
        if s is not None:
            subhandles.add("sensor")
            assert isinstance(s.name, str)
            for cat in ("optical", "inertial", "fluid", "temperature", "em", "rf", "force", "tactile"):
                lst = getattr(s, cat)
                if len(lst) > 0:
                    fieldcats.add("vec(sensor.%s)" % cat)
                    break

        # power source: nested Option payload (battery/solar/supercap/…)
        _, ps = _first_with(doc, "power_source")
        if ps is not None:
            subhandles.add("power")
            for k in ("battery", "solar", "supercapacitor", "fuel_cell", "tank"):
                if getattr(ps, k) is not None:
                    fieldcats.add("nested-option(power_source.%s)" % k)
                    break

        # transmission: Vec at the doc level + a scalar
        if len(doc.transmission) > 0:
            subhandles.add("transmission")
            assert isinstance(doc.transmission[0].name, str)
            fieldcats.add("vec(hcdf.transmission)")

        # dynamic_surface subhandle
        _, ds = _first_with(doc, "dynamic_surface")
        if ds is not None:
            subhandles.add("surface")
            _ = ds.name

    for req in ("comp", "sensor", "visual", "joint", "power", "transmission", "surface"):
        assert req in subhandles, f"subhandle family never reached across corpora: {req}"
    # Each of the four TYPE categories must be exercised by at least one corpus.
    assert any(f.startswith("data-enum(") for f in fieldcats), fieldcats
    assert any(f.startswith("c-like-enum(") for f in fieldcats), fieldcats
    assert any(f.startswith("vec(") for f in fieldcats), fieldcats
    assert any(f.startswith("nested-option(") for f in fieldcats), fieldcats


# ── VisualGeometry leaf inside the untagged choice is navigable + round-trips ────────────────────────


def _forearm_appearance(doc):
    """The `forearm_vis` visual (ARM B primitive) that carries a `<cylinder>` geometry."""
    for c in doc.comp:
        for vis in c.visual:
            if vis.appearance.variant == "primitive" and vis.appearance.geometry_kind is not None:
                return vis.appearance
    return None


def test_visual_geometry_is_navigable_and_read_write_through():
    doc = Hcdf.loads(_read("tests/valid/articulated-arm.hcdf"))
    ap = _forearm_appearance(doc)
    assert ap is not None, "articulated-arm should carry a primitive <geometry>"
    # READ the fallback/primitive geometry through the choice.
    assert ap.geometry_kind == "cylinder"
    assert ap.geometry_radius == "0.03"
    assert ap.geometry_length == "0.30"
    # WRITE-THROUGH a dim, dump, and confirm it persists + re-parses + survives a fresh navigation.
    ap.geometry_radius = "0.055"
    out = doc.dumps()
    assert "<radius>0.055</radius>" in out
    assert hcdf.parse_ok(out)
    ap2 = _forearm_appearance(Hcdf.loads(out))
    assert ap2.geometry_radius == "0.055" and ap2.geometry_length == "0.30"
    # A wrong-shape dim access errors (variant-aware, mirroring color_*).
    with pytest.raises(ValueError):
        _ = ap.geometry_radii  # cylinder has no radii


def test_visual_geometry_select_all_primitive_kinds_round_trips():
    """Every primitive kind is selectable + its dims write through + the doc still parses."""
    doc = Hcdf.loads(_read("tests/valid/articulated-arm.hcdf"))
    ap = _forearm_appearance(doc)
    assert ap is not None
    cases = {
        "box": ("geometry_size", "0.1 0.2 0.3"),
        "sphere": ("geometry_radius", "0.4"),
        "capsule": ("geometry_radius", "0.05"),
        "cone": ("geometry_length", "0.6"),
        "ellipsoid": ("geometry_radii", "0.1 0.2 0.3"),
        "cylinder": ("geometry_radius", "0.07"),
    }
    for kind, (attr, val) in cases.items():
        ap.set_geometry(kind)
        assert ap.geometry_kind == kind
        setattr(ap, attr, val)
        assert getattr(ap, attr) == val
        out = doc.dumps()
        assert hcdf.parse_ok(out), f"{kind}: doc no longer parses"
        # re-navigate the reloaded doc: the kind + dim survive the round-trip
        ap_rt = _forearm_appearance(Hcdf.loads(out))
        assert ap_rt.geometry_kind == kind and getattr(ap_rt, attr) == val
    # Clearing the geometry writes through to an absent primitive.
    ap.set_geometry(None)
    assert ap.geometry_kind is None
    assert hcdf.parse_ok(doc.dumps())
    # An unknown kind is rejected.
    with pytest.raises(ValueError):
        ap.set_geometry("torus")
