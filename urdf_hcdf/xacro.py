"""Expand xacro files to URDF by shelling out to the optional ``xacro`` tool.

xacro is the ROS macro language that most real URDFs are authored in. It is an optional
dependency here: we resolve the ``xacro`` command (``$XACRO_BIN``, then ``PATH``, then a ROS
install under ``/opt/ros/*/bin``) and run it, the same way ``sdf_io`` shells out to ``gz``.

A xacro that uses ``$(find some_package)`` needs that package to be resolvable. Pass ``packages``
({name: path}) and a throwaway ament index is built so ``$(find name)`` resolves to that path with
no colcon build or sourced workspace. (Or source a workspace that provides the package, as usual.)
"""
from __future__ import annotations

import glob
import os
import shutil
import subprocess
import tempfile


def xacro_bin():
    """Resolve the xacro command: $XACRO_BIN, PATH, then a ROS install. None if absent."""
    for c in (os.environ.get("XACRO_BIN"), shutil.which("xacro")):
        if c and os.path.exists(c):
            return c
    for c in sorted(glob.glob("/opt/ros/*/bin/xacro"), reverse=True):
        if os.path.exists(c):
            return c
    return None


def available():
    return xacro_bin() is not None


def _ament_prefix(packages):
    """Build a throwaway ament prefix exposing ``{pkg: path}`` so ``$(find pkg)`` resolves to ``path``.

    Mirrors what an installed/sourced workspace provides: a marker under
    ``share/ament_index/resource_index/packages`` and the package dir at ``share/<pkg>``. Returns the
    prefix directory (the caller adds it to AMENT_PREFIX_PATH and removes it afterwards).
    """
    prefix = tempfile.mkdtemp(prefix="hcdf-ament-")
    markers = os.path.join(prefix, "share", "ament_index", "resource_index", "packages")
    os.makedirs(markers, exist_ok=True)
    for name, path in packages.items():
        open(os.path.join(markers, name), "w").close()
        link = os.path.join(prefix, "share", name)
        if not os.path.lexists(link):
            os.symlink(os.path.abspath(path), link)
    return prefix


def expand(path, mappings=None, packages=None):
    """Expand a xacro file to a URDF string.

    ``mappings`` are xacro args as a ``{name: value}`` dict. ``packages`` ({name: path}) makes
    ``$(find name)`` resolve to ``path`` without a colcon build, via a throwaway ament index.
    """
    b = xacro_bin()
    if b is None:
        raise RuntimeError("xacro not found; install xacro or set XACRO_BIN to convert a .xacro file")
    args = [b, str(path)] + [f"{k}:={v}" for k, v in (mappings or {}).items()]
    env = dict(os.environ)
    prefix = _ament_prefix(packages) if packages else None
    if prefix is not None:
        existing = env.get("AMENT_PREFIX_PATH")
        env["AMENT_PREFIX_PATH"] = prefix + (os.pathsep + existing if existing else "")
    try:
        p = subprocess.run(args, capture_output=True, text=True, env=env)
    finally:
        if prefix is not None:
            shutil.rmtree(prefix, ignore_errors=True)
    if p.returncode != 0:
        tail = p.stderr.strip().splitlines()[-1] if p.stderr.strip() else f"exit {p.returncode}"
        raise RuntimeError(f"xacro failed to expand {path!r}: {tail}")
    return p.stdout
