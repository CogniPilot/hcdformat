"""Locate and drive the local libsdformat ``gz sdf`` CLI.

libsdformat is OPTIONAL: it is a source build (``~/git/gazebo/install/...``), not an apt package,
with no Python bindings — so ``sdf_io`` resolves the real binary (``$GZ_BIN``, the known install
path, or ``PATH``) and shells out, degrading gracefully when it is absent (like ``assets`` without
trimesh).

CRITICAL: ``gz sdf --print`` writes non-fatal ``Error``/``Warning`` lines to
**stderr yet still exits 0** — it can silently empty a model (a URDF link without ``<inertial>`` is
dropped). So we parse stderr for diagnostics and never treat exit 0 alone as "clean": ``GzResult.ok``
is true only when the process succeeded AND no ``Error`` diagnostic was emitted.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess

# Pinned SDFormat spec version (libsdformat 16.x emits/consumes 1.12). Bind by version.
SDF_VERSION = "1.12"

_ANSI = re.compile(r"\x1b\[[0-9;]*m")
_KNOWN = os.path.expanduser("~/git/gazebo/install/gz-tools2/bin/gz")


def gz_bin():
    """Resolve the gz binary: $GZ_BIN, the known source-build install path, then PATH. None if absent."""
    for c in (os.environ.get("GZ_BIN"), _KNOWN, shutil.which("gz")):
        if c and os.path.exists(c):
            return c
    return None


def available():
    return gz_bin() is not None


class GzResult:
    """Outcome of a ``gz sdf`` call. ``ok`` requires exit 0 AND no Error-level diagnostic."""

    def __init__(self, returncode, stdout, stderr):
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr
        self.diagnostics = [d for d in (_ANSI.sub("", ln).strip() for ln in stderr.splitlines())
                            if "Error" in d or "Warning" in d]
        self.errors = [d for d in self.diagnostics if "Error" in d]
        self.ok = returncode == 0 and not self.errors

    def __bool__(self):
        return self.ok


def _run(args):
    b = gz_bin()
    if b is None:
        raise RuntimeError("gz (libsdformat) not found; set GZ_BIN or install gz-tools")
    p = subprocess.run([b, "sdf"] + args, capture_output=True, text=True)
    return GzResult(p.returncode, p.stdout, p.stderr)


def check(path):
    """``gz sdf --check`` — validate an SDFormat file against libsdformat."""
    return _run(["--check", str(path)])


def print_sdf(path, preserve_includes=False):
    """``gz sdf --print`` — canonicalize an SDFormat (or URDF) file. Surfaces stderr diagnostics."""
    return _run((["--print", "-i"] if preserve_includes else ["--print"]) + [str(path)])


def urdf_to_sdf(path):
    """Delegated URDF→SDF via libsdformat — the canonical gazebo path.

    Returns a GzResult; ``.stdout`` is the SDF. Because libsdformat silently drops content on a
    bad URDF (exit 0), callers MUST consult ``.ok`` / ``.diagnostics``, not just the return code.
    """
    return print_sdf(path)
