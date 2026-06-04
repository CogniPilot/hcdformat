"""Typed-DOM <-> JSON for HCDF, built on the context-aware converter (convert.py).

JSON mirrors ``hcdf.schema.json``; the converter uses the XSD to infer array-vs-scalar shape,
so ``from_json`` reproduces the same typed DOM that ``to_json`` came from.
"""
from __future__ import annotations

import json
import os
import tempfile

import convert  # repo-root module (path set up by the package __init__)
import hcdfdom

_XSD = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "hcdf.xsd")


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
