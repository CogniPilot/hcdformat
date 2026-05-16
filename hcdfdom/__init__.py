"""hcdfdom — a typed HCDF DOM generated from ``hcdf.xsd``.

The neutral in-memory model that the URDF/SDF/HCDF I/O adapters sit on.
Layer 1 (typed model) + Layer 2 (XML parse/serialize) live here; the tree/ref
validator and the pose/frame normalizer are added on top in later modules.

    import hcdfdom
    doc = hcdfdom.load("examples/drone-quadrotor.hcdf")   # -> Hcdf
    doc.name                                               # typed attribute access
    xml = hcdfdom.dumps(doc)                               # ordered, schema-valid XML
"""
from __future__ import annotations

from lxml import etree

from . import model
from .model import *  # noqa: F401,F403  (re-export every generated class/enum)
from .model import Hcdf

__all__ = list(model.__all__) + ["load", "loads", "to_element", "dump", "dumps"]


def load(path) -> Hcdf:
    """Parse an HCDF file into the typed DOM."""
    return Hcdf.from_xml(etree.parse(str(path)).getroot())


def loads(text) -> Hcdf:
    """Parse HCDF from a string or bytes into the typed DOM."""
    if isinstance(text, str):
        text = text.encode("utf-8")
    return Hcdf.from_xml(etree.fromstring(text))


def to_element(doc: Hcdf):
    """Serialize the DOM to an lxml ``<hcdf>`` element (children in schema order)."""
    return doc.to_xml("hcdf")


def dumps(doc: Hcdf, pretty: bool = True) -> str:
    """Serialize the DOM to an XML string."""
    return etree.tostring(to_element(doc), pretty_print=pretty, encoding="unicode")


def dump(doc: Hcdf, path, pretty: bool = True) -> None:
    """Serialize the DOM to an HCDF file."""
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(dumps(doc, pretty))
