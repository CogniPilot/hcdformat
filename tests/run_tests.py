#!/usr/bin/env python3
"""HCDF Schema Test Suite

Validates HCDF and stream-profile XML files against their XSD schemas.
- Valid tests (tests/valid/, examples/): must pass validation
- Stream profile tests (examples/profiles/): must pass against hcdf-stream-profile.xsd
- Invalid tests (tests/invalid/): must FAIL validation
- Optional .expected companion files specify a required error substring

Exit code: 0 if all tests pass, 1 if any fail.
"""

import glob
import os
import sys
import tempfile
from pathlib import Path

from lxml import etree

# ANSI color codes
GREEN = "\033[32m"
RED = "\033[31m"
BOLD = "\033[1m"
RESET = "\033[0m"

# Resolve paths relative to the repository root (parent of tests/)
REPO_ROOT = Path(__file__).resolve().parent.parent
HCDF_XSD = REPO_ROOT / "hcdf.xsd"
STREAM_XSD = REPO_ROOT / "hcdf-stream-profile.xsd"

# Ensure repo root is on sys.path so we can import convert.py
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))


def load_schema(xsd_path: Path) -> etree.XMLSchema:
    """Parse and return an XMLSchema object."""
    schema_doc = etree.parse(str(xsd_path))
    return etree.XMLSchema(schema_doc)


def validate_file(filepath: Path, schema: etree.XMLSchema):
    """Validate an XML file against a schema.

    Returns (success: bool, error_message: str | None).
    Parse errors and schema errors both count as failures.
    """
    try:
        doc = etree.parse(str(filepath))
    except etree.XMLSyntaxError as e:
        return False, str(e)
    is_valid = schema.validate(doc)
    if is_valid:
        return True, None
    # Collect all error messages
    errors = "\n".join(str(e) for e in schema.error_log)
    return False, errors


def run_valid_tests(schema: etree.XMLSchema, label: str, files: list[Path]):
    """Run tests that are expected to pass. Returns (passed, failed) counts."""
    passed = 0
    failed = 0
    if not files:
        return passed, failed
    print(f"\n{BOLD}{label}:{RESET}")
    for f in sorted(files):
        rel = f.relative_to(REPO_ROOT)
        ok, err = validate_file(f, schema)
        if ok:
            print(f"  {GREEN}\u2713{RESET} {rel}")
            passed += 1
        else:
            print(f"  {RED}\u2717{RESET} {rel}")
            # Show first line of error for diagnosis
            first_line = (err or "").split("\n")[0]
            print(f"    Error: {first_line}")
            failed += 1
    return passed, failed


def run_invalid_tests(schema: etree.XMLSchema, files: list[Path]):
    """Run tests that are expected to fail. Returns (passed, failed) counts."""
    passed = 0
    failed = 0
    if not files:
        return passed, failed
    print(f"\n{BOLD}Invalid tests (must fail):{RESET}")
    for f in sorted(files):
        rel = f.relative_to(REPO_ROOT)
        ok, err = validate_file(f, schema)

        # Read optional .expected companion
        expected_path = f.with_suffix(".expected")
        expected_substr = None
        if expected_path.exists():
            expected_substr = expected_path.read_text().strip()

        if ok:
            # File validated when it should have failed
            print(f"  {RED}\u2717{RESET} {rel} — expected failure but file validated OK")
            failed += 1
        else:
            # File failed as expected — check error substring if provided
            if expected_substr and expected_substr not in (err or ""):
                print(
                    f"  {RED}\u2717{RESET} {rel} — failed but error does not contain "
                    f"'{expected_substr}'"
                )
                first_line = (err or "").split("\n")[0]
                print(f"    Actual: {first_line}")
                failed += 1
            else:
                suffix = f" (expected: {expected_substr})" if expected_substr else ""
                print(f"  {GREEN}\u2713{RESET} {rel}{suffix}")
                passed += 1
    return passed, failed


def collect_files(directory: Path, extension: str) -> list[Path]:
    """Recursively collect files with the given extension."""
    return list(directory.rglob(f"*{extension}"))


def main():
    print(f"{BOLD}HCDF Schema Test Suite{RESET}")
    print("=" * 22)

    # Load schemas
    if not HCDF_XSD.exists():
        print(f"{RED}ERROR: {HCDF_XSD} not found{RESET}", file=sys.stderr)
        sys.exit(2)
    hcdf_schema = load_schema(HCDF_XSD)

    stream_schema = None
    if STREAM_XSD.exists():
        stream_schema = load_schema(STREAM_XSD)

    total_passed = 0
    total_failed = 0

    # --- Valid HCDF tests ---
    valid_hcdf = []
    valid_dirs = [REPO_ROOT / "tests" / "valid", REPO_ROOT / "examples"]
    for d in valid_dirs:
        if d.exists():
            valid_hcdf.extend(collect_files(d, ".hcdf"))
    p, f = run_valid_tests(hcdf_schema, "Valid tests (must pass)", valid_hcdf)
    total_passed += p
    total_failed += f

    # --- Valid stream profile tests ---
    if stream_schema:
        stream_files = []
        stream_dirs = [
            REPO_ROOT / "tests" / "valid",
            REPO_ROOT / "examples" / "profiles",
        ]
        for d in stream_dirs:
            if d.exists():
                stream_files.extend(collect_files(d, ".streams.xml"))
        if stream_files:
            p, f = run_valid_tests(
                stream_schema, "Stream profile tests (must pass)", stream_files
            )
            total_passed += p
            total_failed += f

    # --- Invalid HCDF tests ---
    invalid_dir = REPO_ROOT / "tests" / "invalid"
    invalid_hcdf = []
    if invalid_dir.exists():
        invalid_hcdf = collect_files(invalid_dir, ".hcdf")
    p, f = run_invalid_tests(hcdf_schema, invalid_hcdf)
    total_passed += p
    total_failed += f

    # --- Roundtrip tests (XML -> JSON -> XML must validate) ---
    try:
        from convert import xml_to_json_file, json_to_xml_file

        print(f"\n{BOLD}Roundtrip tests (XML -> JSON -> XML):{RESET}")
        roundtrip_files = (
            sorted(glob.glob(str(REPO_ROOT / "examples" / "*.hcdf")))
            + sorted(glob.glob(str(REPO_ROOT / "tests" / "valid" / "*.hcdf")))
        )
        for hcdf_file in roundtrip_files:
            json_tmp = tempfile.NamedTemporaryFile(suffix=".json", delete=False)
            xml_tmp = tempfile.NamedTemporaryFile(suffix=".hcdf", delete=False)
            json_tmp.close()
            xml_tmp.close()
            try:
                # XML -> JSON
                xml_to_json_file(hcdf_file, json_tmp.name)
                # JSON -> XML
                json_to_xml_file(json_tmp.name, xml_tmp.name)
                # Validate roundtrip
                doc = etree.parse(xml_tmp.name)
                if hcdf_schema.validate(doc):
                    rel = Path(hcdf_file).relative_to(REPO_ROOT)
                    print(f"  {GREEN}\u2713{RESET} {rel}")
                    total_passed += 1
                else:
                    rel = Path(hcdf_file).relative_to(REPO_ROOT)
                    print(f"  {RED}\u2717{RESET} {rel}")
                    for err in list(hcdf_schema.error_log)[:3]:
                        print(f"    Line {err.line}: {err.message}")
                    total_failed += 1
            except Exception as e:
                rel = Path(hcdf_file).relative_to(REPO_ROOT)
                print(f"  {RED}\u2717{RESET} {rel} (error: {e})")
                total_failed += 1
            finally:
                os.unlink(json_tmp.name)
                os.unlink(xml_tmp.name)
    except ImportError:
        print(f"\n{BOLD}Roundtrip tests (XML -> JSON -> XML):{RESET}")
        print(f"  {RED}SKIPPED{RESET} — could not import convert.py")

    # --- Summary ---
    print()
    color = GREEN if total_failed == 0 else RED
    print(
        f"{BOLD}Summary:{RESET} {color}{total_passed} passed{RESET}, "
        f"{color}{total_failed} failed{RESET}"
    )

    sys.exit(0 if total_failed == 0 else 1)


if __name__ == "__main__":
    main()
