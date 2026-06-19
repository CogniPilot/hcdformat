"""Typed-DOM <-> JSON for HCDF, built on the context-aware converter (convert.py).

JSON mirrors ``hcdf.schema.json``; the converter uses the XSD to infer array-vs-scalar shape,
so ``from_json`` reproduces the same typed DOM that ``to_json`` came from.
"""
from __future__ import annotations

import json
import os
import sys
import tempfile

import hcdfdom

from hcdf import convert


def _locate_xsd():
    """Find hcdf.xsd across a source checkout, a pip install, and a ROS (ament) install.

    Checkout: it sits at the repo root (one level up from this package). pip install: under
    ``<sys.prefix>/share/hcdformat`` (see pyproject data-files). ROS install: under the package's
    ament share directory, found via ament_index when that is available.
    """
    name = "hcdf.xsd"
    here = os.path.dirname(os.path.abspath(__file__))
    candidates = [os.path.join(os.path.dirname(here), name),            # source checkout: repo root
                  os.path.join(sys.prefix, "share", "hcdformat", name),  # pip install prefix
                  os.path.join(here, name)]                              # beside the module
    try:  # ROS / ament install: console script runs under system python, data is in the overlay
        from ament_index_python.packages import get_package_share_directory
        candidates.insert(1, os.path.join(get_package_share_directory("hcdformat"), name))
    except Exception:  # noqa: BLE001  (not a ROS environment)
        pass
    for cand in candidates:
        if os.path.exists(cand):
            return cand
    return candidates[0]  # let the eventual open() report it clearly


_XSD = _locate_xsd()


def to_json(doc) -> dict:
    """Serialize a typed Hcdf DOM to a JSON-compatible dict ({"hcdf": {...}})."""
    return {"hcdf": convert.xml_element_to_json(doc.to_xml("hcdf"))}


def from_json(obj) -> "hcdfdom.Hcdf":
    """Parse a JSON dict (or path/str) into a typed Hcdf DOM (XSD-aware array inference)."""
    if isinstance(obj, (dict, list)):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
            json.dump(obj, fh)
            tmp = fh.name
        try:
            el = convert.json_to_xml(tmp, _XSD)
        finally:
            os.unlink(tmp)
    else:
        el = convert.json_to_xml(str(obj), _XSD)
    return hcdfdom.Hcdf.from_xml(el)
