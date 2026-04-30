#!/usr/bin/env python3
"""Migrate pre-reset HCDF 1.0 files to the clean HCDF 1.0 schema.

The clean 1.0 reset replaces the
ad-hoc bare-string pose/com encodings with the single structured `pose` type:

  * bare-string  <pose>x y z r p y</pose>  -> <pose xyz="x y z" rpy="r p y"/>
        (6 values -> xyz+rpy ; 7 values -> xyz+quat ; 3 values -> xyz only)
  * bare-string  <com>x y z</com>          -> <inertia_origin xyz="x y z"/>

Both `<pose>` and `<com>` are schema-guaranteed text-only elements (former type
xs:string), so a surgical text substitution is exact and namespace-agnostic, and
leaves every other byte of the document untouched (minimal diff).

Idempotent: structured poses (which carry attributes, e.g. `<pose xyz=...>`) and
already-migrated `<inertia_origin>` are not matched and pass through unchanged.

Usage:
    python3 scripts/migrate_to_clean_1_0.py FILE.hcdf [FILE ...]
    python3 scripts/migrate_to_clean_1_0.py --check FILE.hcdf   # report only, no write
"""
import re
import sys

# A bare element body is text with no child markup, so [^<]* is exact and safe.
_POSE_RE = re.compile(r"<pose>([^<]*)</pose>")
_COM_RE = re.compile(r"<com>([^<]*)</com>")


def _pose_attrs(text):
    """Map a "x y z [r p y | qx qy qz qw]" string to structured pose attributes."""
    v = text.split()
    if len(v) == 0:
        return ""  # empty -> identity, schema defaults apply
    if len(v) == 3:
        return 'xyz="%s"' % " ".join(v)
    if len(v) == 6:
        return 'xyz="%s" rpy="%s"' % (" ".join(v[0:3]), " ".join(v[3:6]))
    if len(v) == 7:
        return 'xyz="%s" quat="%s"' % (" ".join(v[0:3]), " ".join(v[3:7]))
    raise ValueError("unexpected pose arity %d: %r" % (len(v), text))


def migrate_text(s):
    def pose_sub(m):
        attrs = _pose_attrs(m.group(1).strip())
        return ("<pose %s/>" % attrs) if attrs else "<pose/>"

    def com_sub(m):
        t = m.group(1).strip()
        return ('<inertia_origin xyz="%s"/>' % t) if t else "<inertia_origin/>"

    s = _POSE_RE.sub(pose_sub, s)
    s = _COM_RE.sub(com_sub, s)
    return s


def main(argv):
    check = False
    paths = []
    for a in argv:
        if a in ("--check", "-n"):
            check = True
        else:
            paths.append(a)
    if not paths:
        print(__doc__.strip())
        return 2

    rc = 0
    for p in paths:
        with open(p, encoding="utf-8") as fh:
            src = fh.read()
        out = migrate_text(src)
        if out == src:
            print("unchanged  %s" % p)
            continue
        n_pose = len(_POSE_RE.findall(src))
        n_com = len(_COM_RE.findall(src))
        if check:
            print("would migrate  %s  (%d pose, %d com)" % (p, n_pose, n_com))
            rc = 1
        else:
            with open(p, "w", encoding="utf-8") as fh:
                fh.write(out)
            print("migrated  %s  (%d pose, %d com)" % (p, n_pose, n_com))
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
