#!/usr/bin/env python3
"""Generate ``hcdfdom/model.py`` — a typed Python DOM — from ``hcdf.xsd``.

The XSD is the single source of truth (same principle as ``generate_json_schema.py``
and ``generate_spec_html.py``). The generated model is committed and CI checks it
for drift, so the DOM can never diverge from the schema (prevents the versions/1.0 drift at the source — one schema, one model).

Design (value-exact, lossless round-trip):
  * one ``@dataclass`` per named ``xs:complexType`` (+ the root ``hcdf`` -> ``Hcdf``);
  * one ``str``-valued ``Enum`` per enumerated ``xs:simpleType``;
  * leaf values (attributes, simple-typed element text) stored as raw ``str`` so no
    numeric reformatting can occur on round-trip; enum leaves stored as the Enum
    (``.value`` is the original string);
  * ``mixed`` types get a ``text`` field; the lax ``<extension>`` (``xs:any``) keeps
    its foreign children as raw lxml elements (``any_content``);
  * children are collected in schema-declaration order, so ``xs:sequence`` order is
    reproduced exactly and the serialized output is always schema-valid.

Usage:
    python3 generate_hcdfdom.py hcdf.xsd hcdfdom/model.py
"""
from __future__ import annotations

import keyword
import re
import sys

from lxml import etree

XS = "http://www.w3.org/2001/XMLSchema"


def ns(tag: str) -> str:
    return f"{{{XS}}}{tag}"


def ln(el) -> str:
    return etree.QName(el.tag).localname


def class_name(type_name: str) -> str:
    """snake_case / kebab-case / already-Pascal -> PascalCase."""
    parts = re.split(r"[_\-]", type_name)
    return "".join(p[:1].upper() + p[1:] for p in parts if p)


def field_name(xml_name: str) -> str:
    n = xml_name.replace("-", "_").replace(".", "_")
    if keyword.iskeyword(n) or n in ("text", "any_content"):
        n += "_"
    if n and n[0].isdigit():
        n = "_" + n
    return n


def enum_member(value: str) -> str:
    m = re.sub(r"\W", "_", value)
    if not m or m[0].isdigit():
        m = "V_" + m
    return m


class Generator:
    def __init__(self, xsd_path: str):
        self.root = etree.parse(xsd_path).getroot()
        self.cts: dict[str, etree._Element] = {}
        self.sts: dict[str, etree._Element] = {}
        for c in self.root:
            if not isinstance(c.tag, str):
                continue
            t, nm = ln(c), c.get("name")
            if not nm:
                continue
            if t == "complexType":
                self.cts[nm] = c
            elif t == "simpleType":
                self.sts[nm] = c
        self.enums: dict[str, list[str]] = {}
        for nm, st in self.sts.items():
            vals = self._enum_values(st)
            if vals:
                self.enums[nm] = vals
        # Injective python class names. XSD's simpleType and complexType namespaces are
        # separate, but Python's class namespace is one -- e.g. enum "MacsecPolicy" and
        # complexType "macsec_policy" both want "MacsecPolicy". Reserve enum names first
        # (author-given, stable), then suffix any colliding complexType/root name.
        self.enum_cls: dict[str, str] = {}
        self.ct_cls: dict[str, str] = {}
        self._used: set[str] = set()
        for nm in sorted(self.enums):
            self.enum_cls[nm] = self._unique(class_name(nm))
        for nm in sorted(self.cts):
            self.ct_cls[nm] = self._unique(class_name(nm))
        self.root_cls = self._unique("Hcdf")
        # synthesized classes for inline (anonymous) complexTypes: cls_name -> element
        self.inline: dict[str, etree._Element] = {}
        self._inline_by_key: dict[tuple, str] = {}

    def _unique(self, name):
        while name in self._used:
            name += "_"
        self._used.add(name)
        return name

    # ── helpers ──────────────────────────────────────────────────────────
    def _enum_values(self, st):
        r = st.find(ns("restriction"))
        if r is None:
            return []
        return [e.get("value") for e in r.findall(ns("enumeration"))]

    def _doc(self, el):
        ann = el.find(ns("annotation"))
        if ann is not None:
            d = ann.find(ns("documentation"))
            if d is not None and d.text:
                return " ".join(d.text.split())
        return None

    # ── field model ──────────────────────────────────────────────────────
    def _elem_field(self, el, owner):
        name = el.get("name") or el.get("ref")
        if not name:
            return None
        maxo = el.get("maxOccurs", "1")
        is_list = maxo == "unbounded" or (maxo.isdigit() and int(maxo) > 1)
        f = {"kind": "elem", "xml": name, "list": is_list,
             "cls": None, "enum": None, "leaf": True}
        inline_ct = el.find(ns("complexType"))
        tattr = el.get("type")
        if inline_ct is not None:
            key = (owner, name)
            syn = self._inline_by_key.get(key)
            if syn is None:
                syn = self._unique(class_name(owner) + class_name(name))
                self._inline_by_key[key] = syn
                self.inline[syn] = inline_ct
            f.update(cls=syn, leaf=False)
        elif tattr in self.cts:
            f.update(cls=self.ct_cls[tattr], leaf=False)
        elif tattr in self.enums:
            f.update(enum=self.enum_cls[tattr])
        # else: builtin or non-enum simpleType (Quat4, restrictions) -> str leaf
        return f

    def _attr_field(self, el):
        name = el.get("name")
        if not name:
            return None
        tattr = el.get("type")
        return {"kind": "attr", "xml": name, "enum": self.enum_cls.get(tattr)}

    def _collect(self, node, fields, owner):
        """Walk sequence/all/choice/extension, appending element & attribute fields."""
        for ch in node:
            if not isinstance(ch.tag, str):
                continue
            t = ln(ch)
            if t in ("sequence", "all", "choice"):
                self._collect(ch, fields, owner)
            elif t == "element":
                fp = self._elem_field(ch, owner)
                if fp:
                    fields.append(fp)
            elif t == "attribute":
                fp = self._attr_field(ch)
                if fp:
                    fields.append(fp)
            elif t == "any":
                fields.append({"kind": "any", "xml": None})
            elif t in ("complexContent", "simpleContent"):
                ext = ch.find(ns("extension"))
                if ext is not None:
                    base = ext.get("base")
                    if base in self.cts:
                        self._collect(self.cts[base], fields, owner)
                    self._collect(ext, fields, owner)

    def _fields(self, ct, owner):
        fields: list[dict] = []
        self._collect(ct, fields, owner)
        mixed = ct.get("mixed") == "true"
        # an extension may set mixed too
        for cc in ct.findall(ns("complexContent")) + ct.findall(ns("simpleContent")):
            if cc.get("mixed") == "true":
                mixed = True
            ext = cc.find(ns("extension"))
            if ext is not None and ext.get("mixed") == "true":
                mixed = True
        # assign unique python names, dropping exact duplicates (base/extension overlap)
        seen_xml: set = set()
        out = []
        used_py: set = set()
        for f in fields:
            key = (f["kind"], f.get("xml"))
            if key in seen_xml:
                continue
            seen_xml.add(key)
            if f["kind"] == "any":
                f["py"] = "any_content"
            else:
                py = field_name(f["xml"])
                while py in used_py:
                    py += "_"
                f["py"] = py
                used_py.add(py)
            out.append(f)
        return out, mixed

    # ── code emission ──────────────────────────────────────────────────────
    def _emit_class(self, cls, ct, owner_type_name):
        fields, mixed = self._fields(ct, owner_type_name)
        lines = ["@dataclass", f"class {cls}:"]
        doc = self._doc(ct)
        if doc:
            lines.append(f'    """{self._safe_doc(doc)}"""')
        if mixed:
            lines.append("    text: Optional[str] = None")
        for f in fields:
            if f["kind"] == "any":
                lines.append(f"    {f['py']}: List[Any] = field(default_factory=list)")
            elif f["kind"] == "elem" and f["list"]:
                lines.append(f"    {f['py']}: List[Any] = field(default_factory=list)")
            else:
                lines.append(f"    {f['py']}: Optional[Any] = None")
        if not fields and not mixed:
            lines.append("    pass")

        # from_xml
        lines += ["", "    @classmethod", "    def from_xml(cls, el):", "        self = cls()"]
        if mixed:
            lines.append("        self.text = el.text")
        body = []
        for f in fields:
            body += self._from_xml_lines(f)
        lines += ["        " + b for b in body] if body else (["        pass"] if not mixed else [])
        lines.append("        return self")

        # to_xml
        lines += ["", "    def to_xml(self, tag):", "        el = etree.Element(tag)"]
        # attributes first
        for f in fields:
            if f["kind"] == "attr":
                lines += ["        " + b for b in self._to_xml_attr(f)]
        if mixed:
            lines += ["        if self.text is not None:", "            el.text = self.text"]
        for f in fields:
            if f["kind"] == "elem":
                lines += ["        " + b for b in self._to_xml_elem(f)]
            elif f["kind"] == "any":
                lines += ["        " + b for b in self._to_xml_any(f)]
        lines.append("        return el")
        return "\n".join(lines)

    def _from_xml_lines(self, f):
        py, xml = f["py"], f.get("xml")
        if f["kind"] == "attr":
            if f["enum"]:
                return [f'_v = el.get("{xml}")',
                        f'self.{py} = {f["enum"]}(_v) if _v is not None else None']
            return [f'self.{py} = el.get("{xml}")']
        if f["kind"] == "any":
            return [f"self.{py} = [copy.deepcopy(_c) for _c in _children(el)]"]
        # element
        if f["list"]:
            if f["leaf"]:
                if f["enum"]:
                    return [f'self.{py} = [{f["enum"]}(_c.text) for _c in _children(el) if _ln(_c) == "{xml}"]']
                return [f'self.{py} = [_c.text for _c in _children(el) if _ln(_c) == "{xml}"]']
            return [f'self.{py} = [{f["cls"]}.from_xml(_c) for _c in _children(el) if _ln(_c) == "{xml}"]']
        # single element
        lines = [f'_c = next((_c for _c in _children(el) if _ln(_c) == "{xml}"), None)']
        if f["leaf"]:
            if f["enum"]:
                lines.append(f'self.{py} = {f["enum"]}(_c.text) if _c is not None else None')
            else:
                lines.append(f"self.{py} = _c.text if _c is not None else None")
        else:
            lines.append(f'self.{py} = {f["cls"]}.from_xml(_c) if _c is not None else None')
        return lines

    def _to_xml_attr(self, f):
        py, xml = f["py"], f["xml"]
        if f["enum"]:
            return [f"if self.{py} is not None:", f'    el.set("{xml}", self.{py}.value)']
        return [f"if self.{py} is not None:", f'    el.set("{xml}", self.{py})']

    def _to_xml_elem(self, f):
        py, xml = f["py"], f["xml"]
        if f["list"]:
            if f["leaf"]:
                if f["enum"]:
                    return [f"for _i in self.{py}:",
                            f'    _e = etree.SubElement(el, "{xml}"); _e.text = _i.value']
                return [f"for _i in self.{py}:",
                        f'    _e = etree.SubElement(el, "{xml}")',
                        f"    if _i is not None: _e.text = _i"]
            return [f"for _i in self.{py}:", f'    el.append(_i.to_xml("{xml}"))']
        if f["leaf"]:
            if f["enum"]:
                return [f"if self.{py} is not None:",
                        f'    _e = etree.SubElement(el, "{xml}"); _e.text = self.{py}.value']
            return [f"if self.{py} is not None:",
                    f'    _e = etree.SubElement(el, "{xml}"); _e.text = self.{py}']
        return [f"if self.{py} is not None:", f'    el.append(self.{py}.to_xml("{xml}"))']

    def _to_xml_any(self, f):
        return [f"for _a in self.{f['py']}:", "    el.append(copy.deepcopy(_a))"]

    def _safe_doc(self, doc):
        return doc.replace("\\", "\\\\").replace('"""', "'''")[:500]

    # ── top-level ──────────────────────────────────────────────────────────
    def generate(self):
        out = [HEADER]

        # enums
        for nm in sorted(self.enums):
            cls = self.enum_cls[nm]
            out.append(f"class {cls}(str, Enum):")
            d = self._doc(self.sts[nm])
            if d:
                out.append(f'    """{self._safe_doc(d)}"""')
            used = {}
            for v in self.enums[nm]:
                mem = enum_member(v)
                while mem in used:
                    mem += "_"
                used[mem] = v
                out.append(f'    {mem} = "{v}"')
            out.append("")

        # root <hcdf> element -> Hcdf (inline complexType)
        root_el = self.root.find(ns("element"))
        root_ct = root_el.find(ns("complexType")) if root_el is not None else None

        # named complexTypes
        emitted = []
        for nm in sorted(self.cts):
            emitted.append((self.ct_cls[nm], self.cts[nm], nm))
        if root_ct is not None:
            emitted.append((self.root_cls, root_ct, "hcdf"))

        # emit named + root, then drain synthesized inline classes
        rendered = []
        done = set()
        i = 0
        worklist = list(emitted)
        while i < len(worklist):
            cls, ct, owner = worklist[i]
            i += 1
            if cls in done:
                continue
            done.add(cls)
            rendered.append(self._emit_class(cls, ct, owner))
            # newly discovered inline classes get queued
            for syn, syn_ct in list(self.inline.items()):
                if syn not in done and all(syn != w[0] for w in worklist):
                    worklist.append((syn, syn_ct, syn))

        out.extend(rendered)

        # __all__
        names = sorted(set(self.ct_cls.values()) | set(self.enum_cls.values()) | set(self.inline) | {self.root_cls})
        out.append("")
        out.append("__all__ = [")
        for n in names:
            out.append(f'    "{n}",')
        out.append("]")
        return "\n\n".join(out) + "\n"


HEADER = '''# GENERATED by generate_hcdfdom.py from hcdf.xsd -- DO NOT EDIT.
# Regenerate: python3 generate_hcdfdom.py hcdf.xsd hcdfdom/model.py
"""Typed HCDF DOM, generated from hcdf.xsd (the single source of truth)."""
from __future__ import annotations

import copy
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, List, Optional

from lxml import etree


def _children(el):
    return [c for c in el if isinstance(c.tag, str)]


def _ln(c):
    return etree.QName(c.tag).localname'''


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <input.xsd> <output.py>")
        sys.exit(1)
    gen = Generator(sys.argv[1])
    code = gen.generate()
    with open(sys.argv[2], "w", encoding="utf-8") as fh:
        fh.write(code)
    n_ct = len(gen.cts) + 1
    print(f"Generated {sys.argv[2]}: {n_ct} dataclasses, "
          f"{len(gen.enums)} enums, {len(gen.inline)} inline classes")


if __name__ == "__main__":
    main()
