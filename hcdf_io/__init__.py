"""hcdf_io — HCDF document services on the hcdfdom IR.

hcdfdom already does XML <-> typed-DOM (load/dump). This package adds the two things the
converter pipeline needs on top of that:

  * ``flatten(src)``  — resolve ``<include>`` model composition into a single self-contained
    document: each included sub-assembly is name-prefixed (``@name`` -> ``prefix/...``, so the
    same module can be included many times without collision), rigidly placed by ``@pose``, and
    merged. All internal references (joint parent/child, mimic, group/state refs, frame
    relative-to, visual color refs, transmission endpoints, loop bodies) are rewritten to the
    prefixed names. Nested includes resolve recursively; include cycles are detected.
  * ``to_json`` / ``from_json`` — typed-DOM <-> JSON, matching ``hcdf.schema.json`` (closes the
    "JSON in" half of hcdfdom Layer 2), built on the existing context-aware ``convert.py``.

    import hcdf_io
    doc, notes = hcdf_io.flatten("robot.hcdf")     # -> (hcdfdom.Hcdf, [loss/merge notes])
    obj = hcdf_io.to_json(doc); doc2 = hcdf_io.from_json(obj)
"""
from __future__ import annotations

from hcdfdom import dump, dumps, load  # noqa: F401  (convenience re-exports)

from .include import flatten  # noqa: E402
from .json_io import from_json, to_json  # noqa: E402

__all__ = ["flatten", "to_json", "from_json", "load", "dump", "dumps"]
