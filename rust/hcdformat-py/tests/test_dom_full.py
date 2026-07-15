"""Acceptance: the `#[pydom]` DOM handles scaled from the initial subset to the WHOLE ~190-type model.

The subset suite (`test_dom_writethrough.py`, `test_dom_macro.py`) still passes unchanged; this suite proves the
generalization: shared types reached from MANY parents (the DAG, resolved via per-type enum locators),
fixed-array pose attributes, deep write-through chains, lists at every level, the extended data-enum
companion, module-wide `.pyi`, and stale-handle safety at depth.

Run: rebuild the binding (maturin) then `pytest` (PYTHONPATH="" PYTEST_DISABLE_PLUGIN_AUTOLOAD=1).
"""
import os

import pytest

import hcdf
from hcdf.dom import Hcdf

REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CORPUS = os.path.join(REPO_ROOT, "examples", "test-minimal.hcdf")


def _load():
    with open(CORPUS, "r", encoding="utf-8") as fh:
        return Hcdf.loads(fh.read())


# ── the DAG: one shared type (`Pose`) reached from several DISTINCT parent sites ─────────────────────

def test_shared_pose_type_distinct_sites():
    doc = _load()
    c = doc.comp[0]
    # Pose appears under visual, collision, frame, inertial.inertia_origin: each a separate locator
    # variant of the SAME PyPose handle. Every one must resolve + write through independently.
    vp = c.visual[0].pose
    cp = c.collision[0].pose
    fp = c.frame[0].pose
    ip = c.inertial.inertia_origin
    for p in (vp, cp, fp, ip):
        assert p is not None
    vp.xyz = [1.0, 2.0, 3.0]
    cp.xyz = [4.0, 5.0, 6.0]
    fp.xyz = [7.0, 8.0, 9.0]
    ip.xyz = [0.1, 0.2, 0.3]
    # re-read each independently: no cross-talk between the sites
    assert doc.comp[0].visual[0].pose.xyz == [1.0, 2.0, 3.0]
    assert doc.comp[0].collision[0].pose.xyz == [4.0, 5.0, 6.0]
    assert doc.comp[0].frame[0].pose.xyz == [7.0, 8.0, 9.0]
    assert doc.comp[0].inertial.inertia_origin.xyz == [0.1, 0.2, 0.3]
    out = doc.dumps()
    for frag in ('xyz="1 2 3"', 'xyz="4 5 6"', 'xyz="7 8 9"', 'xyz="0.1 0.2 0.3"'):
        assert frag in out, frag


def test_pose_on_joint_site():
    doc = _load()
    j = doc.joint[0]
    op = j.ensure_origin()  # Pose under a Joint: a different parent family than Comp children
    op.rpy = [0.0, 0.0, 1.5708]
    assert doc.joint[0].origin.rpy == [0.0, 0.0, 1.5708]


# ── fixed-size float array attribute (Pose xyz/rpy/quat): the new OptArr category ───────────────────

def test_pose_array_get_set_clear_and_validate():
    doc = _load()
    p = doc.comp[0].visual[0].pose
    assert p.xyz == [0.0, 0.0, 0.0]  # corpus has xyz="0 0 0"
    p.quat = [0.0, 0.0, 0.0, 1.0]   # a 4-vector attribute
    assert doc.comp[0].visual[0].pose.quat == [0.0, 0.0, 0.0, 1.0]
    p.xyz = None                     # clearing writes through as an absent attribute
    assert p.xyz is None
    with pytest.raises(ValueError):  # wrong arity is rejected
        p.xyz = [1.0, 2.0]
    with pytest.raises(ValueError):
        p.quat = [1.0, 2.0, 3.0]


# ── deep write-through: comp -> function -> endpoint choice -> structured port reference ──────

def test_deep_nested_chain_write_through():
    doc = _load()
    function = doc.comp[0].switch[0]
    assert function.name == "test-switch"
    endpoint = function.bidirectional[0].endpoint
    assert endpoint.variant == "port-ref"
    port_ref = endpoint.port
    assert port_ref is not None
    assert port_ref.component == "test-board"
    assert port_ref.port == "eth0"
    port_ref.port = "eth1"
    assert doc.comp[0].switch[0].bidirectional[0].endpoint.port.port == "eth1"
    assert '<port-ref component="test-board" port="eth1"/>' in doc.dumps()


# ── lists at multiple levels + append materializing a defaulted element ─────────────────────────────

def test_list_append_deep_and_named():
    doc = _load()
    functions = doc.comp[0].switch
    n0 = len(functions)
    functions.append("spare-switch")
    assert len(doc.comp[0].switch) == n0 + 1
    assert doc.comp[0].switch[n0].name == "spare-switch"

    pins = doc.comp[0].connector[0].pin
    m0 = len(pins)
    pins.append("signal-1")
    assert len(doc.comp[0].connector[0].pin) == m0 + 1
    assert doc.comp[0].connector[0].pin[m0].name == "signal-1"


# ── the ONE data-carrying enum companion, extended to the arm-B inline Color ────────────────────────

def test_data_enum_companion_color_arm():
    doc = _load()
    ap = doc.comp[0].visual[0].appearance
    assert ap.variant == "model"
    ap.set_primitive()
    # color_* now editable (ARM B); reaches the inline Color that lives only inside the choice
    ap.color_rgba = "0.2 0.4 0.6 1"
    ap.color_name = "slate"
    assert doc.comp[0].visual[0].appearance.color_rgba == "0.2 0.4 0.6 1"
    assert doc.comp[0].visual[0].appearance.color_name == "slate"
    out = doc.dumps()
    assert "0.2 0.4 0.6 1" in out
    # color_* errors while the model arm is active
    ap.set_model()
    with pytest.raises(ValueError):
        _ = ap.color_rgba


# ── round-trip: an edited doc re-parses, and the comment side-channel survives deep edits ───────────

def test_deep_edit_round_trips_and_keeps_comments():
    doc = _load()
    function = doc.comp[0].switch[0]
    function.name = "forwarding-switch"
    owner = doc.chain[0].hop[1].owner.owner
    assert owner.variant == "function-ref"
    owner.function.function = "forwarding-switch"
    doc.comp[0].visual[0].pose.xyz = [1.0, 1.0, 1.0]
    out = doc.dumps()
    assert hcdf.parse_ok(out)
    assert 'function="forwarding-switch"' in out
    assert "board with switch" in out  # a serde-skip XML comment survived the deep edits
    # reparse is stable
    assert hcdf.parse_ok(Hcdf.loads(out).dumps())


# ── module-wide coverage: the big handle set is registered + stubbed ────────────────────────────────

def test_broad_class_registration_and_stub():
    # A spread of handles across the schema regions must all be registered on the extension.
    for name in [
        "PyHcdf", "PyComp", "PyVisual", "PyPose", "PyCollision", "PySurface", "PyFriction",
        "PyFunction", "PyPort", "PyCapabilities", "PyConnector", "PyPosition", "PyBinding",
        "PyFunctionalEndpointRef", "PyPhysicalEndpointRef", "PyLink", "PyBus", "PyChain",
        "PyParticipant", "PyHop", "PyLeg", "PyNetworkSelection", "PyNetworkConfiguration",
        "PyMotor", "PyTransmission", "PySensor", "PyInertial", "PyColor", "PyMesh",
        "PyVisualAppearance",
    ]:
        assert hasattr(hcdf, name), name
    # The top-level stub (hcdf/__init__.pyi) carries the module-level pyfunctions.
    init_stub = hcdf.binding_pyi()
    assert "def from_urdf(" in init_stub and "def bake_to_glb(" in init_stub
    # The DOM stub carries canonical function, connector, topology, and structured-reference handles.
    dom_stub = hcdf.dom_pyi()
    assert "class PyFunction:" in dom_stub
    assert "class PyConnector:" in dom_stub
    assert "class PyChain:" in dom_stub
    assert "class PyFunctionalEndpointChoice:" in dom_stub
    assert 'def select_port(self) -> "PyPortRef": ...' in dom_stub
    assert "def append(self, name: Optional[str] = ...) -> None: ..." in dom_stub


# ── stale-handle safety survives at depth (index locator invalidated) ───────────────────────────────

def test_stale_handle_deep():
    doc = _load()
    endpoints = doc.comp[0].switch[0].bidirectional
    n = len(endpoints)
    with pytest.raises(ValueError):
        _ = endpoints[n]  # one past the end never resolves cleanly
    ghost = endpoints[n - 1]
    assert ghost.endpoint.variant == "port-ref"
    assert isinstance(ghost.endpoint.port.port, str)
