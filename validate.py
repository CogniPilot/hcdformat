#!/usr/bin/env python3
"""HCDF XSD validator using lxml (not xmllint)."""
import sys
from lxml import etree

xsd_path = sys.argv[1] if len(sys.argv) > 1 else "hcdf.xsd"
xml_path = sys.argv[2] if len(sys.argv) > 2 else "examples/test-minimal.hcdf"

try:
    schema_doc = etree.parse(xsd_path)
    print(f"XSD parsed OK: {xsd_path}")
except Exception as e:
    print(f"XSD PARSE ERROR: {e}")
    sys.exit(1)

try:
    schema = etree.XMLSchema(schema_doc)
    print("XSD compiled OK")
except Exception as e:
    print(f"XSD COMPILE ERROR: {e}")
    sys.exit(1)

try:
    doc = etree.parse(xml_path)
    print(f"XML parsed OK: {xml_path}")
except Exception as e:
    print(f"XML PARSE ERROR: {e}")
    sys.exit(1)

if schema.validate(doc):
    print("VALID")
else:
    for err in schema.error_log:
        print(f"  Line {err.line}: {err.message}")
    print(f"INVALID ({len(schema.error_log)} errors)")
    sys.exit(1)
