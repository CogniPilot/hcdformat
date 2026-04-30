#!/usr/bin/env python3
"""Enforce the versions/ frozen-archive + hash-pin invariants (see versions/README.md).

Fails (exit 1) if any of these hold:
  1. A file listed in versions/HASHES is missing, or its frozen copy at
     versions/<version>/<file> does not match the recorded sha256.
  2. The live, canonical root file (hcdf.xsd / hcdf-stream-profile.xsd /
     hcdf.schema.json) does not byte-match the frozen copy for the version it
     declares -- i.e. the live schema has drifted from its released archive.
  3. The version <hcdf @version> default declared in the root hcdf.xsd has no
     matching versions/<version>/ archive.

This makes "same version string, different bytes" a hard, detectable contradiction
instead of a silent drift (the failure that produced the old versions/1.0 divergence).

Run from the repo root:  python3 scripts/check_version_hashes.py
"""
import hashlib
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HASHES = os.path.join(ROOT, "versions", "HASHES")
# Root canonical schema files that must equal their released archive copy.
# (Derived artifacts -- hcdf.schema.json, spec.html -- are not archived here;
# their consistency is enforced by CI regenerate-and-diff.)
CANONICAL = ("hcdf.xsd", "hcdf-stream-profile.xsd")


def sha256(path):
    with open(path, "rb") as fh:
        return hashlib.sha256(fh.read()).hexdigest()


def parse_hashes():
    """-> list of (version, filename, sha)."""
    rows = []
    with open(HASHES, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split()
            if len(parts) != 3:
                raise SystemExit("malformed HASHES line: %r" % line)
            rows.append(tuple(parts))
    return rows


def declared_version():
    text = open(os.path.join(ROOT, "hcdf.xsd"), encoding="utf-8").read()
    m = re.search(r'<xs:attribute name="version"[^>]*default="([0-9.]+)"', text)
    if not m:
        raise SystemExit("could not read root hcdf @version default")
    return m.group(1)


def main():
    errors = []
    rows = parse_hashes()

    # (1) frozen-archive integrity
    for version, fname, expected in rows:
        frozen = os.path.join(ROOT, "versions", version, fname)
        if not os.path.exists(frozen):
            errors.append("missing frozen archive: versions/%s/%s" % (version, fname))
            continue
        actual = sha256(frozen)
        if actual != expected:
            errors.append(
                "frozen archive changed (versions/%s/%s): HASHES=%s actual=%s"
                % (version, fname, expected[:12], actual[:12])
            )

    # (3) the root-declared version must have an archive
    ver = declared_version()
    versions_present = {v for (v, _f, _s) in rows}
    if ver not in versions_present:
        errors.append('root hcdf @version="%s" has no versions/%s/ archive in HASHES' % (ver, ver))

    # (2) live canonical files must equal the archive for the declared version
    by_ver = {(v, f): s for (v, f, s) in rows}
    for fname in CANONICAL:
        live = os.path.join(ROOT, fname)
        if not os.path.exists(live):
            continue
        expected = by_ver.get((ver, fname))
        if expected is None:
            errors.append("no HASHES entry for live %s at version %s" % (fname, ver))
            continue
        actual = sha256(live)
        if actual != expected:
            errors.append(
                "live %s drifted from its %s archive: live=%s HASHES=%s "
                "(update versions/%s/%s + versions/HASHES if this is an intentional release)"
                % (fname, ver, actual[:12], expected[:12], ver, fname)
            )

    if errors:
        print("version-hash check FAILED:")
        for e in errors:
            print("  - " + e)
        return 1
    print("version-hash check OK (declared version %s; %d archive entries verified)" % (ver, len(rows)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
