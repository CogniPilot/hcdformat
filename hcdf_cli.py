#!/usr/bin/env python3
"""Command line interface for HCDF conversion and checks.

Convert between URDF, SDF, HCDF, and HCDF-JSON. Every conversion routes through the
hcdfdom model, so any pair works, including ones the native tools cannot do (for
example SDF to URDF, which has no direct converter). Formats are taken from the file
extensions and can be overridden with --from / --to.

    hcdf convert arm.urdf arm.hcdf      # URDF -> HCDF
    hcdf convert arm.hcdf arm.urdf      # HCDF -> URDF, loss manifest to stderr
    hcdf convert arm.sdf  arm.urdf      # SDF -> HCDF -> URDF through the hub
    hcdf convert arm.urdf.xacro arm.hcdf --package my_robot=/path/to/my_robot  # resolves $(find)
    hcdf validate arm.hcdf
    hcdf profile  arm.hcdf
    hcdf expand   arm.urdf.xacro arm.urdf --package my_robot=/path --abs-meshes

Installed as the ``hcdf`` command (it also runs as ``python3 -m hcdf_cli ...`` from a checkout).
"""
from __future__ import annotations

import argparse
import json
import os
import sys

_EXT = {".urdf": "urdf", ".sdf": "sdf", ".hcdf": "hcdf", ".json": "json", ".xacro": "urdf"}
_FORMATS = ("urdf", "sdf", "hcdf", "json")


def _xacro_map(items):
    m = {}
    for it in items or []:
        if ":=" not in it:
            raise SystemExit(f"error: --xacro expects NAME:=VALUE, got {it!r}")
        k, v = it.split(":=", 1)
        m[k] = v
    return m


def _pkg_map(items):
    """Parse ``--package PKG=PATH`` items into a {name: path} map (for $(find) and package:// uris)."""
    m = {}
    for it in items or []:
        if "=" not in it:
            raise SystemExit(f"error: --package expects PKG=PATH, got {it!r}")
        k, v = it.split("=", 1)
        m[k] = v
    return m


def _fmt(path, override):
    if override:
        return override
    fmt = _EXT.get(os.path.splitext(path)[1].lower())
    if fmt is None:
        raise SystemExit(f"error: cannot infer format from {path!r}; "
                         f"pass --from/--to (known: {', '.join(_FORMATS)})")
    return fmt


def _load(path, fmt, xacro_args=None, baker=None, packages=None):
    """Read any supported format into an hcdfdom document. Returns (doc, notes)."""
    if fmt == "urdf":
        from urdf_hcdf import from_urdf
        return from_urdf(path, xacro_args, baker, packages)
    if fmt == "sdf":
        from sdf_io import from_sdf
        return from_sdf(path, baker)
    if fmt == "hcdf":
        from hcdfdom import load
        return load(path), []
    if fmt == "json":
        from hcdf_io import from_json
        with open(path) as fh:
            return from_json(json.load(fh)), []
    raise SystemExit(f"error: unsupported input format {fmt!r}")


def _dump(doc, fmt):
    """Serialize an hcdfdom document to a target format. Returns (text, loss_or_None)."""
    if fmt == "urdf":
        from urdf_hcdf import to_urdf
        return to_urdf(doc)
    if fmt == "sdf":
        from sdf_io import to_sdf
        return to_sdf(doc)
    if fmt == "hcdf":
        from hcdfdom import dumps
        return dumps(doc), None
    if fmt == "json":
        from hcdf_io import to_json
        return json.dumps(to_json(doc), indent=2) + "\n", None
    raise SystemExit(f"error: unsupported output format {fmt!r}")


def _write(text, out):
    text = text if text.endswith("\n") else text + "\n"
    if out == "-":
        sys.stdout.write(text)
    else:
        with open(out, "w") as fh:
            fh.write(text)


def cmd_convert(args):
    src = _fmt(args.input, args.from_fmt)
    if args.to_fmt:
        dst = args.to_fmt
    elif args.output != "-":
        dst = _fmt(args.output, None)
    else:
        raise SystemExit("error: writing to stdout requires --to FORMAT")

    pkgs = _pkg_map(args.package)
    baker = None
    if args.bake is not None:
        from hcdf import assets
        # write @uri relative to the output document so a viewer resolves the mesh next to it
        doc_dir = os.getcwd() if args.output == "-" else os.path.dirname(os.path.abspath(args.output))
        prefix = os.path.relpath(os.path.abspath(args.bake), doc_dir).replace(os.sep, "/")
        baker = assets.Baker(out_dir=args.bake, base_dir=os.path.dirname(os.path.abspath(args.input)),
                             package_paths=pkgs, uri_prefix=prefix)

    doc, notes = _load(args.input, src, _xacro_map(args.xacro), baker, pkgs)
    text, loss = _dump(doc, dst)
    _write(text, args.output)

    # nothing is dropped silently: import notes and export losses both go to stderr
    for n in notes:
        print(f"note: {n}", file=sys.stderr)
    if loss is not None and len(loss):
        if args.loss:
            with open(args.loss, "w") as fh:
                fh.write(loss.to_json())
            print(f"loss: {len(loss)} item(s) written to {args.loss}", file=sys.stderr)
        else:
            print(f"loss: {len(loss)} construct(s) not representable in {dst.upper()}:", file=sys.stderr)
            print(loss.text(), file=sys.stderr)
    dest = "stdout" if args.output == "-" else args.output
    print(f"{src.upper()} -> {dst.upper()}: wrote {dest}", file=sys.stderr)
    return 0


def cmd_validate(args):
    from hcdfdom.validate import ERROR, validate
    doc, _ = _load(args.input, _fmt(args.input, args.from_fmt), _xacro_map(args.xacro),
                   packages=_pkg_map(args.package))
    issues = validate(doc)
    for i in issues:
        print(str(i), file=sys.stderr)
    errors = [i for i in issues if i.level == ERROR]
    print(f"{'OK' if not errors else 'INVALID'}: {len(issues)} issue(s), {len(errors)} error(s)",
          file=sys.stderr)
    return 0 if not errors else 1


def cmd_profile(args):
    from urdf_hcdf import check_profile
    doc, _ = _load(args.input, _fmt(args.input, args.from_fmt), _xacro_map(args.xacro),
                   packages=_pkg_map(args.package))
    report = check_profile(doc)
    print(report.to_json() if args.json else report.markdown())
    return 0 if report.in_profile else 1


def cmd_expand(args):
    """Expand a xacro (or read a URDF) to a plain URDF, resolving $(find) via --package.

    With --abs-meshes, package://PKG/... mesh uris are rewritten to absolute file:// paths using the
    --package map, so the URDF self-resolves with no resource path (handy for spawning the source
    model next to a converted one).
    """
    from urdf_hcdf import xacro
    pkgs = _pkg_map(args.package)
    if str(args.input).endswith(".xacro"):
        text = xacro.expand(args.input, _xacro_map(args.xacro), pkgs)
    else:
        with open(args.input) as fh:
            text = fh.read()
    if args.abs_meshes:
        for name, path in pkgs.items():
            text = text.replace(f"package://{name}/", "file://" + os.path.abspath(path).rstrip("/") + "/")
    _write(text, args.output)
    print(f"expand: wrote {'stdout' if args.output == '-' else args.output}", file=sys.stderr)
    return 0


def _version():
    try:
        from importlib.metadata import version
        return version("hcdformat")
    except Exception:  # noqa: BLE001  (running from a source checkout, not installed)
        return "1.0.0+local"


def build_parser():
    p = argparse.ArgumentParser(prog="hcdf",
                                description="Convert and check HCDF, URDF, and SDF robot descriptions.")
    p.add_argument("--version", action="version", version=f"hcdf {_version()}")
    sub = p.add_subparsers(dest="command", required=True)

    c = sub.add_parser("convert", help="convert between URDF, SDF, HCDF, and JSON")
    c.add_argument("input")
    c.add_argument("output", help="output path, or - for stdout (then --to is required)")
    c.add_argument("--from", dest="from_fmt", choices=_FORMATS, help="override the input format")
    c.add_argument("--to", dest="to_fmt", choices=_FORMATS, help="override the output format")
    c.add_argument("--loss", metavar="FILE", help="write the loss manifest as JSON to FILE")
    c.add_argument("--xacro", action="append", metavar="NAME:=VALUE",
                   help="xacro argument for a .xacro input (repeatable)")
    c.add_argument("--bake", metavar="DIR",
                   help="bake meshes to canonical assets in DIR (scale and mirror folded in)")
    c.add_argument("--package", action="append", metavar="PKG=PATH",
                   help="locate a ROS package: resolves $(find PKG) in xacro and package://PKG/... "
                        "mesh uris (repeatable)")
    c.set_defaults(func=cmd_convert)

    v = sub.add_parser("validate", help="validate the kinematic tree and references")
    v.add_argument("input")
    v.add_argument("--from", dest="from_fmt", choices=_FORMATS, help="override the input format")
    v.add_argument("--xacro", action="append", metavar="NAME:=VALUE",
                   help="xacro argument for a .xacro input (repeatable)")
    v.add_argument("--package", action="append", metavar="PKG=PATH",
                   help="locate a ROS package for $(find PKG) (repeatable)")
    v.set_defaults(func=cmd_validate)

    pr = sub.add_parser("profile", help="classify a document against the HCDF-URDF profile")
    pr.add_argument("input")
    pr.add_argument("--from", dest="from_fmt", choices=_FORMATS, help="override the input format")
    pr.add_argument("--json", action="store_true", help="emit JSON instead of markdown")
    pr.add_argument("--xacro", action="append", metavar="NAME:=VALUE",
                    help="xacro argument for a .xacro input (repeatable)")
    pr.add_argument("--package", action="append", metavar="PKG=PATH",
                    help="locate a ROS package for $(find PKG) (repeatable)")
    pr.set_defaults(func=cmd_profile)

    e = sub.add_parser("expand", help="expand a xacro (or read a URDF) to a plain URDF")
    e.add_argument("input")
    e.add_argument("output", help="output path, or - for stdout")
    e.add_argument("--xacro", action="append", metavar="NAME:=VALUE",
                   help="xacro argument for a .xacro input (repeatable)")
    e.add_argument("--package", action="append", metavar="PKG=PATH",
                   help="locate a ROS package for $(find PKG) and package:// rewrites (repeatable)")
    e.add_argument("--abs-meshes", action="store_true",
                   help="rewrite package://PKG/... mesh uris to absolute file:// paths")
    e.set_defaults(func=cmd_expand)
    return p


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        return args.func(args)
    except (FileNotFoundError, RuntimeError) as e:
        raise SystemExit(f"error: {e}")


if __name__ == "__main__":
    sys.exit(main())
