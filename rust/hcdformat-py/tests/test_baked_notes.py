"""Import-time baked-note behavior, through the core-backed `hcdf.assets` surface.

``drop_baked_notes`` removes the per-visual "deferred to the asset step" notes for exactly the visuals
that baked, keyed on the ``(comp, visual)`` pair so a same-named visual under a DIFFERENT comp keeps its
own note. It is now a core function reached through the binding, so this exercises it directly on the
pair contract, and also end to end through a real with-baking URDF import that resolves and bakes a mesh.

Run against the extracted wheel:  PYTHONPATH=<extracted-wheel-dir> pytest
"""
import os
import tempfile

import hcdf
from hcdf import assets

REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
ASSETS_DIR = os.path.join(REPO_ROOT, "tests", "assets")  # holds cube.stl


# ── the (comp, visual) pair contract of drop_baked_notes ─────────────────────────────────────────────
def test_pair_aware_drop_keeps_a_same_named_visual_under_another_comp():
    # Two comps each carry a visual named 'mesh'; only linkA's mesh baked.
    baked = {("linkA", "mesh")}
    note_a = (
        "link 'linkA' visual 'mesh': mesh <scale> '2 2 2' not applied "
        "(bake the GLB with the meshes present to fold it in)"
    )
    note_b = (
        "link 'linkB' visual 'mesh': mesh <scale> '2 2 2' not applied "
        "(bake the GLB with the meshes present to fold it in)"
    )
    kept = assets.drop_baked_notes([note_a, note_b], baked)
    assert note_a not in kept, "the baked comp's deferral note is consumed"
    assert note_b in kept, "the unbaked comp's same-named visual keeps its note"


def test_comp_less_urdf_note_still_drops_by_visual_name():
    # The URDF importer omits the comp from the note; a baked visual's comp-less note still drops.
    baked = {("base", "wheel")}
    note = (
        "visual 'wheel': mesh scale '2 2 2' not applied "
        "(bake the GLB with the meshes present to fold it in)"
    )
    assert assets.drop_baked_notes([note], baked) == []


def test_no_bakes_is_a_noop():
    note = (
        "link 'linkB' visual 'mesh': mesh <scale> '2 2 2' not applied "
        "(bake the GLB with the meshes present to fold it in)"
    )
    # With nothing baked, every note is returned unchanged.
    assert assets.drop_baked_notes([note], set()) == [note]


# ── end to end: a with-baking URDF import bakes the mesh and consumes its deferral note ───────────────
def test_with_baking_import_bakes_mesh_and_drops_its_note():
    if not os.path.isfile(os.path.join(ASSETS_DIR, "cube.stl")):
        import pytest

        pytest.skip("tests/assets/cube.stl not present")
    urdf = (
        '<robot name="r">'
        '  <link name="base_link">'
        '    <visual><geometry><mesh filename="cube.stl" scale="2 2 2"/></geometry></visual>'
        "  </link>"
        "</robot>"
    )
    with tempfile.TemporaryDirectory() as out:
        baker = assets.Baker(out, base_dir=ASSETS_DIR, uri_prefix="assets")
        doc, notes = hcdf.urdf.from_urdf(urdf, baker=baker)
        ap = doc.comp[0].visual[0].appearance
        # The visual is a baked GLB model under the output prefix, and the file was written.
        assert ap.variant == "model"
        assert ap.model_uri.startswith("assets/") and ap.model_uri.endswith(".glb")
        assert os.path.isfile(os.path.join(out, os.path.basename(ap.model_uri)))
        # The "scale not applied" deferral note for that visual was consumed by the bake.
        assert not any("not applied (bake the GLB" in n for n in notes), notes
