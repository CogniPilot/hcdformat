# Versioning Policy

HCDF uses semantic versioning (MAJOR.MINOR) for the schema.

## Version Rules

| Change | Version bump | Backward compatible? |
|--------|-------------|---------------------|
| New optional element or attribute | Minor (1.0 to 1.1) | Yes |
| New enumeration value | Minor | Yes |
| New optional top-level element | Minor | Yes |
| Change required to optional | Minor | Yes |
| New extension XSD | None (extensions are independent) | Yes |
| Change optional to required | **Major** (1.x to 2.0) | No |
| Remove element or attribute | **Major** | No |
| Rename element or attribute | **Major** | No |
| Change element type | **Major** | No |
| Reorder required sequence elements | **Major** | No |

## Compatibility Rules

**Forward compatibility:** Parsers should ignore unknown elements and attributes. An HCDF 1.0 parser reading a 1.3 file should work. It will not understand elements added in 1.1, 1.2, or 1.3, but it should not reject the file. This is how XML naturally behaves when `processContents="lax"` or when parsers skip unknown elements.

**Backward compatibility:** Minor versions are backward compatible. A valid 1.0 file is also a valid 1.3 file. Major versions may break backward compatibility.

**Version attribute:** The `<hcdf version="1.0">` attribute on the root element declares which schema version the file was authored against. Parsers use this to select the appropriate schema for validation and to enable version-specific behavior.

## Schema Files

Each version has its own XSD archived in the `versions/` directory:

```
versions/
  1.0/
    hcdf.xsd                    # Archived schema
    hcdf-stream-profile.xsd     # Archived stream profile schema
hcdf.xsd                        # Always the latest version (symlink or copy)
```

When a new version is released:
1. Archive the current XSD into `versions/MAJOR.MINOR/`
2. Make schema changes to `hcdf.xsd`
3. Update the version number in the XSD header and default attribute
4. Generate versioned spec pages: `website/spec/MAJOR.MINOR/core.html`
5. Update the spec browser version dropdown
6. Update CHANGELOG.md

## JSON Schema Versioning

The JSON Schema (`hcdf.schema.json`) is generated from the XSD and carries the same version number in its `$id` field. When the XSD version changes, regenerate the JSON Schema:

```bash
python3 generate_json_schema.py hcdf.xsd hcdf.schema.json
```

## Stream Profile Versioning

Stream profile files have their own `version` attribute independent of the main schema version. A stream profile's version increments when streams are added, removed, or their QoS requirements change. The main schema version governs the structure of the stream profile XML; the profile version governs the content.

## Extension Versioning

Extensions are independently versioned via the `version` attribute on `<extension domain="..." version="...">`. Extension schemas in `extensions/` have their own version numbers. Changes to an extension do not affect the core schema version.

## Changelog

Maintain CHANGELOG.md at the repo root with entries for each version:

```markdown
## 1.1 (planned)
- Added: <new-element> for describing X
- Added: "new-value" to SomeType enum

## 1.0 (2026-04-14)
- Initial release
- 147 complex types, 42 enumerations
```
