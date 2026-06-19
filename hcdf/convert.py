#!/usr/bin/env python3
"""HCDF XML <-> JSON bidirectional converter.

Generic tree-walking conversion -- no hardcoded structs.  Automatically
supports any element the XSD defines.

Mapping conventions (matching hcdf-tools Rust converter):
  - XML attributes  -> direct JSON object keys (no @ prefix)
  - XML child elements -> nested objects or arrays
  - Multiple same-named child elements -> JSON arrays
  - Text content    -> "$text" key
  - Pose strings    -> kept as strings (not split into arrays)

When converting JSON -> XML, the converter auto-detects hcdf.xsd in the
same directory (or accepts --xsd <path>) to correctly distinguish XML
attributes from child elements.  Without the XSD, a fallback heuristic
is used.

Usage:
    python3 convert.py input.hcdf output.json     # XML -> JSON
    python3 convert.py input.json  output.hcdf     # JSON -> XML
    python3 convert.py input.json  output.xml      # JSON -> XML
"""

from __future__ import annotations

import json
import os
import sys
from collections import OrderedDict
from typing import Any, Dict, List, Optional, Set

from lxml import etree


# ═══════════════════════════════════════════════════════════════════════════
# XML -> JSON
# ═══════════════════════════════════════════════════════════════════════════

def _count_child_tags(elem: etree._Element) -> Dict[str, int]:
    """Count how many children share each tag name."""
    counts: Dict[str, int] = {}
    for child in elem:
        if isinstance(child.tag, str):  # skip comments / PIs
            tag = etree.QName(child.tag).localname
            counts[tag] = counts.get(tag, 0) + 1
    return counts


def _text_value(text: Optional[str]) -> Optional[str]:
    """Clean up text content (strip whitespace)."""
    if text is None:
        return None
    stripped = text.strip()
    return stripped if stripped else None


def _try_numeric(value: str) -> Any:
    """Try to interpret a string as int or float; return original if not numeric."""
    if value is None:
        return None
    # Don't convert strings that look like space-separated vectors
    if " " in value.strip():
        return value
    # Don't convert hex strings
    if value.startswith("0x") or value.startswith("0X"):
        return value
    # Don't convert version strings like "1.0.3"
    if value.count(".") > 1:
        return value
    # Boolean
    if value == "true":
        return True
    if value == "false":
        return False
    # Integer
    try:
        int_val = int(value)
        # Avoid converting strings that have leading zeros (like "007")
        if str(int_val) == value:
            return int_val
    except ValueError:
        pass
    # Float
    try:
        float_val = float(value)
        if not value.startswith(".") and "e" not in value.lower():
            return float_val
    except ValueError:
        pass
    return value


def xml_element_to_json(elem: etree._Element) -> Any:
    """Recursively convert an XML element to a JSON-compatible dict/value.

    Returns a dict, string, number, or boolean depending on the element
    content.
    """
    # Count children with the same tag to decide array vs object
    tag_counts = _count_child_tags(elem)
    has_children = bool(tag_counts)
    has_attrs = bool(elem.attrib)
    text = _text_value(elem.text)

    # Simple text element: no children, no attributes -> return scalar
    if not has_children and not has_attrs:
        if text is not None:
            return _try_numeric(text)
        return None

    # Build object
    obj: OrderedDict[str, Any] = OrderedDict()

    # Attributes -> direct keys
    for attr_name, attr_value in elem.attrib.items():
        # Strip namespace prefixes from attributes
        local_name = etree.QName(attr_name).localname if "{" in attr_name else attr_name
        obj[local_name] = _try_numeric(attr_value)

    # Mixed content: text + children or text + attributes
    if text is not None:
        obj["$text"] = _try_numeric(text)

    # Child elements
    # Track which tags we've already started as arrays
    array_tags: Dict[str, list] = {}

    for child in elem:
        if not isinstance(child.tag, str):  # skip comments
            continue
        child_tag = etree.QName(child.tag).localname
        child_value = xml_element_to_json(child)

        if tag_counts.get(child_tag, 0) > 1:
            # Multiple elements with this tag -> array
            if child_tag not in array_tags:
                array_tags[child_tag] = []
                obj[child_tag] = array_tags[child_tag]
            array_tags[child_tag].append(child_value)
        else:
            # Single element -> direct value
            obj[child_tag] = child_value

    return obj


def xml_to_json(xml_path: str) -> dict:
    """Parse an XML file and convert to JSON dict."""
    tree = etree.parse(xml_path)
    root = tree.getroot()

    root_tag = etree.QName(root.tag).localname
    result = xml_element_to_json(root)

    return {root_tag: result}


# ═══════════════════════════════════════════════════════════════════════════
# XSD type information (for context-aware JSON -> XML)
# ═══════════════════════════════════════════════════════════════════════════

XS_NS = "http://www.w3.org/2001/XMLSchema"


def _xs(tag: str) -> str:
    """Return fully-qualified XSD tag name."""
    return f"{{{XS_NS}}}{tag}"


class XsdInfo:
    """Rich XSD type information for context-aware JSON->XML conversion.

    Mirrors the Rust XsdInfo struct: carries full type context so that
    the same element name in different parent types can resolve to
    different XSD types with different attribute sets.
    """

    def __init__(self):
        # type_name -> set of attribute names
        self.type_attrs: Dict[str, Set[str]] = {}
        # type_name -> {child_elem_name: child_type_name}
        self.type_children: Dict[str, Dict[str, str]] = {}
        # type_name -> base type name (from xs:extension)
        self.type_bases: Dict[str, str] = {}
        # global element_name -> type name
        self.elem_type_map: Dict[str, str] = {}
        # type_name -> has mixed content
        self.type_mixed: Dict[str, bool] = {}

    def child_type(self, parent_type: str, child_tag: str) -> Optional[str]:
        """Look up the type of a child element within a parent type context."""
        children = self.type_children.get(parent_type)
        if children:
            ct = children.get(child_tag)
            if ct is not None:
                return ct
        # Walk base chain
        base = self.type_bases.get(parent_type)
        if base:
            return self.child_type(base, child_tag)
        return None

    def attrs_for_type(self, type_name: str) -> Set[str]:
        """Get the resolved attribute set for a type (including inherited attrs)."""
        attrs = set(self.type_attrs.get(type_name, set()))
        base = self.type_bases.get(type_name)
        if base:
            attrs |= self.attrs_for_type(base)
        return attrs

    def is_attr_only(self, type_name: str) -> bool:
        """Check if a type is attribute-only (has attributes, no children, no mixed text)."""
        attrs = self.attrs_for_type(type_name)
        if not attrs:
            return False
        children = self.type_children.get(type_name, {})
        has_children = bool(children)
        is_mixed = self.type_mixed.get(type_name, False)
        return not has_children and not is_mixed

    def is_mixed(self, type_name: str) -> bool:
        """Check if a type has mixed="true" content."""
        if self.type_mixed.get(type_name, False):
            return True
        base = self.type_bases.get(type_name)
        if base:
            return self.is_mixed(base)
        return False


def _build_xsd_info(xsd_path: str) -> XsdInfo:
    """Parse an XSD file and build an XsdInfo with full type context.

    Uses lxml XPath for clean traversal (unlike the Rust event-based approach),
    but builds the same data structures.
    """
    info = XsdInfo()
    tree = etree.parse(xsd_path)
    root = tree.getroot()

    # Index all named complexType elements
    named_cts: Dict[str, etree._Element] = {}
    for ct in root.findall(_xs("complexType")):
        name = ct.get("name")
        if name:
            named_cts[name] = ct

    def _collect_type_info(ct_elem: etree._Element, type_name: str) -> None:
        """Extract attributes, children, mixed, and base from a complexType."""
        is_mixed = ct_elem.get("mixed") == "true"
        if is_mixed:
            info.type_mixed[type_name] = True

        info.type_attrs.setdefault(type_name, set())
        info.type_children.setdefault(type_name, {})

        # Direct attributes on this complexType (any depth within it)
        # But we must be careful: attributes nested inside an extension belong
        # to THIS type (via extension), attributes in a nested anonymous
        # complexType belong to that nested type. We handle that correctly by
        # only looking at xs:attribute that are direct children of:
        # - the complexType itself
        # - sequence/all/choice within it
        # - complexContent/extension within it

        def _walk_for_attrs_and_children(node: etree._Element, owner: str) -> None:
            """Walk a node collecting attribute decls and element decls."""
            for child in node:
                if not isinstance(child.tag, str):
                    continue
                local = etree.QName(child.tag).localname

                if local == "attribute":
                    attr_name = child.get("name")
                    if attr_name:
                        info.type_attrs.setdefault(owner, set()).add(attr_name)

                elif local == "element":
                    elem_name = child.get("name")
                    elem_type = child.get("type", "")
                    if elem_name and elem_type:
                        info.type_children.setdefault(owner, {})[elem_name] = elem_type
                    elif elem_name:
                        # Element with inline complexType -- create synthetic type
                        inline_ct = child.find(_xs("complexType"))
                        if inline_ct is not None:
                            synthetic = f"__{elem_name}__"
                            info.type_children.setdefault(owner, {})[elem_name] = synthetic
                            _collect_type_info(inline_ct, synthetic)
                            info.elem_type_map[elem_name] = synthetic
                        else:
                            # Element with inline simpleType or no type -- treat as string
                            info.type_children.setdefault(owner, {})[elem_name] = ""

                elif local in ("sequence", "all", "choice"):
                    _walk_for_attrs_and_children(child, owner)

                elif local == "complexContent":
                    ext = child.find(_xs("extension"))
                    if ext is not None:
                        base = ext.get("base", "")
                        if base:
                            info.type_bases[owner] = base
                        _walk_for_attrs_and_children(ext, owner)

                elif local in ("simpleContent",):
                    ext = child.find(_xs("extension"))
                    if ext is not None:
                        base = ext.get("base", "")
                        if base:
                            info.type_bases[owner] = base
                        _walk_for_attrs_and_children(ext, owner)

        _walk_for_attrs_and_children(ct_elem, type_name)

    # Process all named complex types
    for name, ct in named_cts.items():
        _collect_type_info(ct, name)

    # Process global element declarations
    for el in root.findall(_xs("element")):
        elem_name = el.get("name")
        elem_type = el.get("type")
        if elem_name and elem_type:
            info.elem_type_map[elem_name] = elem_type

        # Inline complex type on global element (e.g., <xs:element name="hcdf">)
        inline_ct = el.find(_xs("complexType"))
        if elem_name and inline_ct is not None:
            synthetic = f"__{elem_name}__"
            info.elem_type_map[elem_name] = synthetic
            _collect_type_info(inline_ct, synthetic)

    # Resolve inheritance: merge base type children into derived types
    all_types = list(info.type_children.keys())
    for _ in range(10):  # max 10 inheritance depth
        changed = False
        for tn in all_types:
            base_name = info.type_bases.get(tn)
            if not base_name:
                continue
            base_children = info.type_children.get(base_name, {})
            entry = info.type_children.setdefault(tn, {})
            for k, v in base_children.items():
                if k not in entry:
                    entry[k] = v
                    changed = True
        if not changed:
            break

    return info


# ═══════════════════════════════════════════════════════════════════════════
# JSON -> XML  (XSD-aware for correct attribute vs child-element handling)
# ═══════════════════════════════════════════════════════════════════════════

def _to_str(value: Any) -> str:
    """Convert a JSON value to a string for XML text/attribute content."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return ""
    return str(value)


class ContextAwareJsonToXml:
    """JSON->XML converter that uses XSD type context to distinguish
    attributes from child elements, producing correct roundtrip output.

    Port of the Rust JsonToXml with XsdInfo from hcdf-cli/src/main.rs.
    """

    def __init__(self, xsd_path: Optional[str] = None):
        self.xsd: Optional[XsdInfo] = None
        if xsd_path:
            self.xsd = _build_xsd_info(xsd_path)

    def resolve_type(self, tag: str, parent_type: Optional[str]) -> Optional[str]:
        """Resolve the XSD type for an element given parent type context."""
        if not self.xsd:
            return None
        # First try parent context
        if parent_type:
            ct = self.xsd.child_type(parent_type, tag)
            if ct is not None:
                return ct
        # Fall back to global element map
        return self.xsd.elem_type_map.get(tag)

    def is_attribute_xsd(self, my_type: Optional[str], key: str) -> bool:
        """Check if key is an attribute on the given type."""
        if self.xsd and my_type:
            attrs = self.xsd.attrs_for_type(my_type)
            return key in attrs
        return False

    def write_element(
        self,
        parent: Optional[etree._Element],
        tag: str,
        value: Any,
        parent_type: Optional[str] = None,
    ) -> etree._Element:
        """Convert a JSON value to an XML element with context-aware type resolution."""
        # 1. Resolve my type from parent context
        my_type = self.resolve_type(tag, parent_type)

        # 2. Handle attribute-only types (scalar JSON -> <tag attr="value"/>)
        if self.xsd and my_type and self.xsd.is_attr_only(my_type):
            if isinstance(value, (str, int, float, bool)):
                attrs = self.xsd.attrs_for_type(my_type)
                if attrs:
                    attr_name = next(iter(attrs))
                    if parent is None:
                        elem = etree.Element(tag)
                    else:
                        elem = etree.SubElement(parent, tag)
                    elem.set(attr_name, _to_str(value))
                    return elem

        if isinstance(value, dict):
            has_xsd = self.xsd is not None
            my_attrs = self.xsd.attrs_for_type(my_type) if (self.xsd and my_type) else set()

            # Split keys into attrs, children, text
            xml_attrs: List[tuple] = []
            xml_children: List[tuple] = []
            text_content: Optional[str] = None

            for key, val in value.items():
                if key == "$text":
                    text_content = _to_str(val)
                elif isinstance(val, (list, dict)):
                    xml_children.append((key, val))
                elif has_xsd:
                    if key in my_attrs:
                        xml_attrs.append((key, _to_str(val)))
                    else:
                        xml_children.append((key, val))
                else:
                    # No XSD -- heuristic: scalars are attrs
                    xml_attrs.append((key, _to_str(val)))

            if parent is None:
                elem = etree.Element(tag)
            else:
                elem = etree.SubElement(parent, tag)

            for k, v in xml_attrs:
                elem.set(k, v)

            if text_content is not None:
                elem.text = text_content

            for child_tag, child_val in xml_children:
                if isinstance(child_val, list):
                    for item in child_val:
                        self.write_element(elem, child_tag, item, my_type)
                elif isinstance(child_val, dict):
                    self.write_element(elem, child_tag, child_val, my_type)
                else:
                    # Scalar child element
                    child = etree.SubElement(elem, child_tag)
                    if child_val is not None:
                        child.text = _to_str(child_val)

            return elem

        elif isinstance(value, list):
            # Shouldn't happen at top level, but handle gracefully
            last = None
            for item in value:
                last = self.write_element(parent, tag, item, parent_type)
            return last

        else:
            # Scalar value -> text element
            if parent is None:
                elem = etree.Element(tag)
            else:
                elem = etree.SubElement(parent, tag)
            if value is not None:
                elem.text = _to_str(value)
            return elem


def json_to_xml(json_path: str, xsd_path: Optional[str] = None) -> etree._Element:
    """Parse a JSON file and convert to XML element tree."""
    with open(json_path, "r") as f:
        data = json.load(f)

    if not isinstance(data, dict) or len(data) != 1:
        raise ValueError(
            "JSON root must be an object with exactly one key (the root element tag)"
        )

    root_tag = next(iter(data))
    root_value = data[root_tag]

    converter = ContextAwareJsonToXml(xsd_path)
    root = converter.write_element(None, root_tag, root_value)
    return root


# ═══════════════════════════════════════════════════════════════════════════
# Public API for imports
# ═══════════════════════════════════════════════════════════════════════════

def _find_xsd(near_path: str) -> Optional[str]:
    """Auto-detect hcdf.xsd: check same dir as input, cwd, parent dir."""
    candidates = []
    # Same directory as the input file
    input_dir = os.path.dirname(os.path.abspath(near_path))
    candidates.append(os.path.join(input_dir, "hcdf.xsd"))
    # Current working directory
    candidates.append(os.path.join(os.getcwd(), "hcdf.xsd"))
    # Parent of input directory
    candidates.append(os.path.join(os.path.dirname(input_dir), "hcdf.xsd"))
    # Parent of cwd
    candidates.append(os.path.join(os.path.dirname(os.getcwd()), "hcdf.xsd"))

    for c in candidates:
        if os.path.exists(c):
            return c
    return None


def xml_to_json_file(input_path: str, output_path: str) -> None:
    """Convert an XML file to JSON and write to output_path."""
    result = xml_to_json(input_path)
    with open(output_path, "w") as f:
        json.dump(result, f, indent=2, ensure_ascii=False)
        f.write("\n")


def json_to_xml_file(
    input_path: str,
    output_path: str,
    xsd_path: Optional[str] = None,
) -> None:
    """Convert a JSON file to XML and write to output_path.

    If xsd_path is None, auto-detects hcdf.xsd near the input file.
    """
    if xsd_path is None:
        xsd_path = _find_xsd(input_path)
    root = json_to_xml(input_path, xsd_path)
    tree = etree.ElementTree(root)
    etree.indent(tree, space="  ")
    xml_bytes = etree.tostring(
        tree,
        pretty_print=True,
        xml_declaration=True,
        encoding="UTF-8",
    )
    with open(output_path, "wb") as f:
        f.write(xml_bytes)


# ═══════════════════════════════════════════════════════════════════════════
# CLI
# ═══════════════════════════════════════════════════════════════════════════

def detect_format(path: str) -> str:
    """Detect file format from extension."""
    lower = path.lower()
    if lower.endswith(".json"):
        return "json"
    if lower.endswith((".xml", ".hcdf")):
        return "xml"
    raise ValueError(f"Cannot detect format from extension: {path}")


def main() -> None:
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <input> <output> [--xsd <path>]")
        print("  Formats auto-detected from extension: .hcdf/.xml = XML, .json = JSON")
        sys.exit(1)

    input_path = sys.argv[1]
    output_path = sys.argv[2]

    # Optional XSD path for JSON->XML (improves roundtrip fidelity)
    xsd_path: Optional[str] = None
    if "--xsd" in sys.argv:
        idx = sys.argv.index("--xsd")
        if idx + 1 < len(sys.argv):
            xsd_path = sys.argv[idx + 1]
    else:
        xsd_path = _find_xsd(input_path)

    input_fmt = detect_format(input_path)
    output_fmt = detect_format(output_path)

    if input_fmt == "xml" and output_fmt == "json":
        xml_to_json_file(input_path, output_path)
        print(f"Converted XML -> JSON: {input_path} -> {output_path}")

    elif input_fmt == "json" and output_fmt == "xml":
        json_to_xml_file(input_path, output_path, xsd_path)
        print(f"Converted JSON -> XML: {input_path} -> {output_path}")

    elif input_fmt == output_fmt:
        print(f"Error: input and output are both {input_fmt}. Nothing to convert.")
        sys.exit(1)

    else:
        print(f"Error: unsupported conversion {input_fmt} -> {output_fmt}")
        sys.exit(1)


if __name__ == "__main__":
    main()
