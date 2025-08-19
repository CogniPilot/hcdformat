#!/usr/bin/env python3
"""Generate a JSON Schema (draft 2020-12) from the HCDF XSD.

The XSD is the single source of truth. This script parses it and produces
a JSON Schema that mirrors the XSD structure, enabling JSON-based tooling
to validate HCDF documents converted to JSON.

Usage:
    python3 generate_json_schema.py hcdf.xsd hcdf.schema.json
"""

from __future__ import annotations

import json
import sys
from collections import OrderedDict
from typing import Dict, List, Optional, Set

from lxml import etree

XS = "http://www.w3.org/2001/XMLSchema"


def ns(tag):
    """Return fully-qualified XSD tag name."""
    return f"{{{XS}}}{tag}"


# ── XSD simple type → JSON Schema type mapping ──────────────────────────────

BUILTIN_TYPE_MAP = {
    "xs:string": {"type": "string"},
    "xs:double": {"type": "number"},
    "xs:int": {"type": "integer"},
    "xs:integer": {"type": "integer"},
    "xs:unsignedInt": {"type": "integer", "minimum": 0},
    "xs:unsignedLong": {"type": "integer", "minimum": 0},
    "xs:boolean": {"type": "boolean"},
    "xs:anyURI": {"type": "string", "format": "uri"},
}


class XsdToJsonSchema:
    """Converts an XSD document to a JSON Schema."""

    def __init__(self, xsd_path: str):
        self.tree = etree.parse(xsd_path)
        self.root = self.tree.getroot()
        # Collect named types
        self.simple_types: dict[str, etree._Element] = {}
        self.complex_types: dict[str, etree._Element] = {}
        self._index_types()
        # Cache for converted definitions to avoid infinite recursion
        self._defs: dict[str, dict] = {}
        self._in_progress: set[str] = set()

    def _index_types(self):
        """Index all named simpleType and complexType definitions."""
        for child in self.root:
            if not isinstance(child.tag, str):
                continue  # skip comments / PIs
            tag = etree.QName(child.tag).localname
            name = child.get("name")
            if not name:
                continue
            if tag == "simpleType":
                self.simple_types[name] = child
            elif tag == "complexType":
                self.complex_types[name] = child

    # ── Public entry point ───────────────────────────────────────────────

    def generate(self) -> dict:
        """Generate the complete JSON Schema."""
        schema = OrderedDict()
        schema["$schema"] = "https://json-schema.org/draft/2020-12/schema"
        schema["$id"] = "https://hcdf.org/schema/hcdf.schema.json"
        schema["title"] = "HCDF (Hardware Configuration Descriptive Format)"
        schema["description"] = (
            "JSON Schema for HCDF documents, auto-generated from hcdf.xsd. "
            "The XSD is the source of truth."
        )

        # Build $defs for all named types
        defs = OrderedDict()
        for name in sorted(self.simple_types):
            defs[name] = self._convert_simple_type(self.simple_types[name])
        for name in sorted(self.complex_types):
            defs[name] = self._convert_complex_type(self.complex_types[name])
        schema["$defs"] = defs

        # Root element: <hcdf>
        root_elem = self.root.find(ns("element"))
        if root_elem is not None:
            root_schema = self._convert_root_element(root_elem)
            schema["type"] = root_schema.get("type", "object")
            if "properties" in root_schema:
                schema["properties"] = root_schema["properties"]
            if "required" in root_schema:
                schema["required"] = root_schema["required"]
            if "additionalProperties" in root_schema:
                schema["additionalProperties"] = root_schema["additionalProperties"]

        return schema

    # ── Simple types ─────────────────────────────────────────────────────

    def _convert_simple_type(self, elem: etree._Element) -> dict:
        """Convert a named xs:simpleType (typically an enumeration)."""
        restriction = elem.find(ns("restriction"))
        if restriction is not None:
            return self._convert_restriction(restriction)
        return {"type": "string"}

    def _convert_restriction(self, restriction: etree._Element) -> dict:
        """Convert xs:restriction (enumerations, base type constraints)."""
        base = restriction.get("base", "xs:string")
        enums = [e.get("value") for e in restriction.findall(ns("enumeration"))]
        if enums:
            return {"type": "string", "enum": enums}
        return dict(self._resolve_builtin(base))

    # ── Complex types ────────────────────────────────────────────────────

    def _convert_complex_type(self, elem: etree._Element) -> dict:
        """Convert a named xs:complexType."""
        name = elem.get("name", "")

        # Handle extension (inheritance)
        complex_content = elem.find(ns("complexContent"))
        if complex_content is not None:
            extension = complex_content.find(ns("extension"))
            if extension is not None:
                return self._convert_extension(extension, name)

        # Check for mixed content (text + attributes)
        is_mixed = elem.get("mixed") == "true"

        result = {"type": "object"}
        props = OrderedDict()
        required = []

        # Collect child elements from sequence / all / choice
        self._collect_children(elem, props, required, result)

        # Collect attributes
        self._collect_attributes(elem, props, required)

        # Mixed content: add $text property
        if is_mixed:
            props["$text"] = {
                "description": "Text content of the element",
                "type": ["string", "number"],
            }

        if props:
            result["properties"] = props
        if required:
            result["required"] = sorted(required)

        # Add documentation
        doc = self._get_documentation(elem)
        if doc:
            result["description"] = doc

        return result

    def _convert_extension(self, extension: etree._Element, type_name: str) -> dict:
        """Convert xs:extension (type inheritance) by merging base properties."""
        base_name = extension.get("base", "")
        base_schema = {}

        # Resolve base type
        if base_name in self.complex_types:
            base_schema = dict(self._convert_complex_type(self.complex_types[base_name]))
        elif base_name in self.simple_types:
            base_schema = dict(self._convert_simple_type(self.simple_types[base_name]))

        result = {"type": "object"}
        props = OrderedDict()
        required = []

        # Copy base properties
        if "properties" in base_schema:
            props.update(base_schema["properties"])
        if "required" in base_schema:
            required.extend(base_schema["required"])

        # Collect extension's own children
        self._collect_children(extension, props, required, result)

        # Collect extension's own attributes
        self._collect_attributes(extension, props, required)

        if props:
            result["properties"] = props
        if required:
            result["required"] = sorted(set(required))

        # Get documentation from the enclosing complexType
        parent = extension.getparent()
        if parent is not None:
            grandparent = parent.getparent()
            if grandparent is not None:
                doc = self._get_documentation(grandparent)
                if doc:
                    result["description"] = doc

        return result

    def _collect_children(
        self,
        parent: etree._Element,
        props: OrderedDict,
        required: list,
        result: dict,
    ):
        """Collect child element definitions from sequence/all/choice."""
        for child in parent:
            if not isinstance(child.tag, str):
                continue  # skip comments / PIs
            tag = etree.QName(child.tag).localname

            if tag == "sequence":
                self._collect_sequence(child, props, required)
            elif tag == "all":
                self._collect_all(child, props, required)
            elif tag == "choice":
                self._convert_choice(child, result)
            # Also handle direct element children (rare but possible)
            elif tag == "element":
                self._add_element_property(child, props, required)

    def _collect_sequence(
        self,
        seq: etree._Element,
        props: OrderedDict,
        required: list,
    ):
        """Convert xs:sequence children to properties."""
        for child in seq:
            if not isinstance(child.tag, str):
                continue
            tag = etree.QName(child.tag).localname
            if tag == "element":
                self._add_element_property(child, props, required)
            elif tag == "choice":
                # Inline choice within sequence — add all options as optional
                for choice_child in child:
                    if not isinstance(choice_child.tag, str):
                        continue
                    ctag = etree.QName(choice_child.tag).localname
                    if ctag == "element":
                        self._add_element_property(
                            choice_child, props, required, force_optional=True
                        )
            elif tag == "any":
                # xs:any — allow additional properties
                props["_extensions"] = {
                    "description": "Extension content (xs:any)",
                    "type": "object",
                    "additionalProperties": True,
                }

    def _collect_all(
        self,
        all_elem: etree._Element,
        props: OrderedDict,
        required: list,
    ):
        """Convert xs:all children to properties (unordered)."""
        for child in all_elem:
            if not isinstance(child.tag, str):
                continue
            tag = etree.QName(child.tag).localname
            if tag == "element":
                self._add_element_property(child, props, required)

    def _convert_choice(self, choice: etree._Element, result: dict):
        """Convert xs:choice to oneOf."""
        one_of = []
        for child in choice:
            if not isinstance(child.tag, str):
                continue
            tag = etree.QName(child.tag).localname
            if tag == "element":
                elem_name = child.get("name")
                elem_schema = self._resolve_element_type(child)
                one_of.append(
                    {
                        "type": "object",
                        "properties": {elem_name: elem_schema},
                        "required": [elem_name],
                    }
                )
        if one_of:
            result["oneOf"] = one_of

    def _add_element_property(
        self,
        elem: etree._Element,
        props: OrderedDict,
        required: list,
        force_optional: bool = False,
    ):
        """Add a single xs:element as a JSON property."""
        name = elem.get("name")
        if not name:
            return

        min_occurs = int(elem.get("minOccurs", "1"))
        max_occurs_str = elem.get("maxOccurs", "1")
        max_occurs = None if max_occurs_str == "unbounded" else int(max_occurs_str)

        elem_schema = self._resolve_element_type(elem)

        # Add documentation from the element itself
        doc = self._get_documentation(elem)
        if doc and "description" not in elem_schema:
            elem_schema["description"] = doc

        # Handle arrays (maxOccurs > 1 or unbounded)
        if max_occurs is None or max_occurs > 1:
            prop_schema = {
                "type": "array",
                "items": elem_schema,
            }
            if min_occurs > 0:
                prop_schema["minItems"] = min_occurs
            if max_occurs is not None:
                prop_schema["maxItems"] = max_occurs
            if doc:
                prop_schema["description"] = doc
            props[name] = prop_schema
        else:
            props[name] = elem_schema

        # Required if minOccurs >= 1 and not forced optional
        if min_occurs >= 1 and not force_optional:
            required.append(name)

    def _resolve_element_type(self, elem: etree._Element) -> dict:
        """Resolve the type of an xs:element to a JSON Schema fragment."""
        type_attr = elem.get("type", "")

        # Inline complex type
        inline_ct = elem.find(ns("complexType"))
        if inline_ct is not None:
            return self._convert_complex_type(inline_ct)

        # Inline simple type
        inline_st = elem.find(ns("simpleType"))
        if inline_st is not None:
            return self._convert_simple_type(inline_st)

        # Named type reference
        if type_attr:
            return self._resolve_type_ref(type_attr)

        # No type info — default to string
        return {"type": "string"}

    def _resolve_type_ref(self, type_name: str) -> dict:
        """Resolve a type reference to a JSON Schema fragment."""
        # Built-in XSD types
        if type_name.startswith("xs:"):
            return dict(self._resolve_builtin(type_name))

        # Named types — use $ref
        if type_name in self.simple_types or type_name in self.complex_types:
            return {"$ref": f"#/$defs/{type_name}"}

        # Unknown — treat as string
        return {"type": "string"}

    def _resolve_builtin(self, type_name: str) -> dict:
        """Map XSD built-in types to JSON Schema types."""
        if type_name in BUILTIN_TYPE_MAP:
            return dict(BUILTIN_TYPE_MAP[type_name])
        # Fallback
        return {"type": "string"}

    def _collect_attributes(
        self,
        parent: etree._Element,
        props: OrderedDict,
        required: list,
    ):
        """Collect xs:attribute definitions as properties."""
        for child in parent:
            if not isinstance(child.tag, str):
                continue
            tag = etree.QName(child.tag).localname
            if tag != "attribute":
                continue
            attr_name = child.get("name")
            if not attr_name:
                continue

            attr_type = child.get("type", "xs:string")
            use = child.get("use", "optional")
            default = child.get("default")

            attr_schema = self._resolve_type_ref(attr_type)

            # Add documentation
            doc = self._get_documentation(child)
            if doc:
                attr_schema["description"] = doc

            # Default value
            if default is not None:
                attr_schema["default"] = self._coerce_default(default, attr_schema)

            props[attr_name] = attr_schema

            if use == "required":
                required.append(attr_name)

    def _coerce_default(self, value: str, schema: dict) -> object:
        """Coerce a default value string to the appropriate JSON type."""
        json_type = schema.get("type", "string")
        if json_type == "integer":
            try:
                return int(value)
            except ValueError:
                return value
        elif json_type == "number":
            try:
                return float(value)
            except ValueError:
                return value
        elif json_type == "boolean":
            return value.lower() in ("true", "1")
        return value

    # ── Root element ─────────────────────────────────────────────────────

    def _convert_root_element(self, elem: etree._Element) -> dict:
        """Convert the root xs:element (hcdf)."""
        # The root element has an inline complexType
        inline_ct = elem.find(ns("complexType"))
        if inline_ct is not None:
            result = self._convert_complex_type(inline_ct)
            # Add version attribute
            return result
        return {"type": "object"}

    # ── Documentation extraction ─────────────────────────────────────────

    def _get_documentation(self, elem: etree._Element) -> str | None:
        """Extract xs:annotation/xs:documentation text."""
        if elem is None:
            return None
        ann = elem.find(ns("annotation"))
        if ann is not None:
            doc = ann.find(ns("documentation"))
            if doc is not None and doc.text:
                return doc.text.strip()
        return None


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <input.xsd> <output.schema.json>")
        sys.exit(1)

    xsd_path = sys.argv[1]
    output_path = sys.argv[2]

    converter = XsdToJsonSchema(xsd_path)
    schema = converter.generate()

    with open(output_path, "w") as f:
        json.dump(schema, f, indent=2, ensure_ascii=False)
        f.write("\n")

    # Print summary
    num_defs = len(schema.get("$defs", {}))
    num_props = len(schema.get("properties", {}))
    print(f"Generated JSON Schema: {output_path}")
    print(f"  $defs: {num_defs} type definitions")
    print(f"  Root properties: {num_props}")


if __name__ == "__main__":
    main()
