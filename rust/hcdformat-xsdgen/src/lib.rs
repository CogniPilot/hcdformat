mod spec;

pub use spec::{
    component_connectivity_particles, connectivity_schema_spec, root_connectivity_particles,
    stream_profile_schema_spec, AttributeSpec, ComplexTypeSpec, ElementSpec, GroupSpec,
    ParticleSpec, SchemaSpec, SimpleTypeKind, SimpleTypeSpec, StreamProfileSchemaSpec,
};

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

const INTERFACE_BEGIN: &str = "  <!-- BEGIN GENERATED CLEAN CONNECTIVITY INTERFACE RETIREMENT -->";
const INTERFACE_END: &str = "  <!-- END GENERATED CLEAN CONNECTIVITY INTERFACE RETIREMENT -->";
const LEGACY_ENUMS_A_BEGIN: &str =
    "  <!-- BEGIN GENERATED CLEAN CONNECTIVITY LEGACY ENUM RETIREMENT A -->";
const LEGACY_ENUMS_A_END: &str =
    "  <!-- END GENERATED CLEAN CONNECTIVITY LEGACY ENUM RETIREMENT A -->";
const LEGACY_ENUMS_B_BEGIN: &str =
    "  <!-- BEGIN GENERATED CLEAN CONNECTIVITY LEGACY ENUM RETIREMENT B -->";
const LEGACY_ENUMS_B_END: &str =
    "  <!-- END GENERATED CLEAN CONNECTIVITY LEGACY ENUM RETIREMENT B -->";
const LEGACY_ENUMS_C_BEGIN: &str =
    "  <!-- BEGIN GENERATED CLEAN CONNECTIVITY LEGACY ENUM RETIREMENT C -->";
const LEGACY_ENUMS_C_END: &str =
    "  <!-- END GENERATED CLEAN CONNECTIVITY LEGACY ENUM RETIREMENT C -->";
const TYPES_BEGIN: &str = "  <!-- BEGIN GENERATED CLEAN CONNECTIVITY TYPES -->";
const TYPES_END: &str = "  <!-- END GENERATED CLEAN CONNECTIVITY TYPES -->";
const SWITCH_BEGIN: &str = "  <!-- BEGIN GENERATED CLEAN CONNECTIVITY FUNCTION RETIREMENT -->";
const SWITCH_END: &str = "  <!-- END GENERATED CLEAN CONNECTIVITY FUNCTION RETIREMENT -->";
const COMP_BEGIN: &str = "      <!-- BEGIN GENERATED COMPONENT CONNECTIVITY PARTICLES -->";
const COMP_END: &str = "      <!-- END GENERATED COMPONENT CONNECTIVITY PARTICLES -->";
const ROOT_BEGIN: &str = "        <!-- BEGIN GENERATED ROOT CONNECTIVITY PARTICLES -->";
const ROOT_END: &str = "        <!-- END GENERATED ROOT CONNECTIVITY PARTICLES -->";

const LEGACY_ENUMS_A_START: &str = "  <xs:simpleType name=\"PortType\">";
const LEGACY_ENUMS_A_STOP: &str = "  <xs:simpleType name=\"NoiseType\">";
const LEGACY_ENUMS_B_START: &str = "  <xs:simpleType name=\"PhyRole\">";
const LEGACY_ENUMS_B_STOP: &str = "  <xs:simpleType name=\"AxisValue\">";
const LEGACY_ENUMS_C_START: &str = "  <xs:simpleType name=\"TunnelProtocol\">";
const LEGACY_ENUMS_C_STOP: &str = "  <!-- Sensor sub-type enumerations -->";
const LEGACY_INTERFACE_START: &str = "  <!-- ================================================================ -->\n  <!-- PORTS & ANTENNAS";
const LEGACY_INTERFACE_END: &str = "  <!-- ================================================================ -->\n  <!-- DYNAMIC SURFACES";
const LEGACY_TYPES_START: &str = "  <!-- ================================================================ -->\n  <!-- NETWORK: Unified connectivity + configuration";
const LEGACY_TYPES_END: &str = "  <!-- ================================================================ -->\n  <!-- COMP, NETWORK-INTERNAL, SOFTWARE, DISCOVERY, EXTENSION, ROOT";
const LEGACY_SWITCH_START: &str = "  <!-- Switch: a switching fabric that bridges ports together.";
const LEGACY_SWITCH_END: &str = "  <xs:complexType name=\"software\">";
const LEGACY_COMP_START: &str = "      <xs:element name=\"switch\" type=\"switch\"";
const LEGACY_COMP_END: &str = "      <xs:element name=\"sensor\" type=\"sensor\"";
const LEGACY_ROOT_START: &str = "        <xs:element name=\"network\" type=\"network\"";
const LEGACY_ROOT_END: &str = "        <xs:element name=\"transmission\" type=\"transmission\"";

const CORE_XSD_COPIES: &[&str] = &[
    "hcdf.xsd",
    "versions/1.0/hcdf.xsd",
    "rust/hcdformat-rs/assets/schema/hcdf/1.0/hcdf.xsd",
    "rust/hcdformat-py/wheel-data/data/share/hcdformat/hcdf.xsd",
];

const STREAM_PROFILE_XSD_COPIES: &[&str] = &[
    "hcdf-stream-profile.xsd",
    "versions/1.0/hcdf-stream-profile.xsd",
    "rust/hcdformat-rs/assets/schema/hcdf/1.0/hcdf-stream-profile.xsd",
    "rust/hcdformat-py/wheel-data/data/share/hcdformat/hcdf-stream-profile.xsd",
];

const HASH_MANIFESTS: &[&str] = &[
    "versions/HASHES",
    "rust/hcdformat-rs/assets/schema/hcdf/1.0/HASHES",
];

pub fn generate_core_schema(source: &str) -> Result<String, String> {
    let mut output = source.to_owned();
    output = replace_block(
        &output,
        LEGACY_ENUMS_A_BEGIN,
        LEGACY_ENUMS_A_END,
        LEGACY_ENUMS_A_START,
        LEGACY_ENUMS_A_STOP,
        "",
    )?;
    output = replace_block(
        &output,
        LEGACY_ENUMS_B_BEGIN,
        LEGACY_ENUMS_B_END,
        LEGACY_ENUMS_B_START,
        LEGACY_ENUMS_B_STOP,
        "",
    )?;
    output = replace_block(
        &output,
        LEGACY_ENUMS_C_BEGIN,
        LEGACY_ENUMS_C_END,
        LEGACY_ENUMS_C_START,
        LEGACY_ENUMS_C_STOP,
        "",
    )?;
    output = replace_block(
        &output,
        INTERFACE_BEGIN,
        INTERFACE_END,
        LEGACY_INTERFACE_START,
        LEGACY_INTERFACE_END,
        "",
    )?;
    output = replace_block(
        &output,
        TYPES_BEGIN,
        TYPES_END,
        LEGACY_TYPES_START,
        LEGACY_TYPES_END,
        &render_connectivity_types(),
    )?;
    output = replace_block(
        &output,
        SWITCH_BEGIN,
        SWITCH_END,
        LEGACY_SWITCH_START,
        LEGACY_SWITCH_END,
        "",
    )?;
    output = replace_block(
        &output,
        COMP_BEGIN,
        COMP_END,
        LEGACY_COMP_START,
        LEGACY_COMP_END,
        &render_particles(&component_connectivity_particles(), 6),
    )?;
    output = replace_block(
        &output,
        ROOT_BEGIN,
        ROOT_END,
        LEGACY_ROOT_START,
        LEGACY_ROOT_END,
        &render_particles(&root_connectivity_particles(), 8),
    )?;
    Ok(output)
}

pub fn generate_stream_profile_schema() -> String {
    let spec = stream_profile_schema_spec();
    let mut output = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- HCDF stream-profile schema, generated from the typed Rust artifact specification. -->
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
           version="1.0"
           elementFormDefault="unqualified">

"#,
    );
    for simple_type in &spec.schema.simple_types {
        render_simple_type(simple_type, &mut output);
    }
    for complex_type in &spec.schema.complex_types {
        render_complex_type(complex_type, &mut output);
    }
    output.push_str("  <xs:element name=\"stream-profile\">\n");
    output.push_str("    <xs:complexType>\n");
    if let Some(content) = &spec.root.content {
        render_particle(content, 6, &mut output);
    }
    render_attributes(&spec.root.attributes, 6, &mut output);
    output.push_str("    </xs:complexType>\n");
    output.push_str("  </xs:element>\n\n");
    output.push_str("</xs:schema>\n");
    output
}

pub fn check_repository(root: &Path) -> Result<(), String> {
    let canonical_path = root.join("hcdf.xsd");
    let canonical = read(&canonical_path)?;
    let generated = generate_core_schema(&canonical)?;
    if generated != canonical {
        return Err(format!(
            "{} is not generated from the typed connectivity specification",
            canonical_path.display()
        ));
    }
    for relative in &CORE_XSD_COPIES[1..] {
        let path = root.join(relative);
        if read(&path)? != canonical {
            return Err(format!(
                "{} differs from the canonical generated hcdf.xsd",
                path.display()
            ));
        }
    }
    let stream_profile = generate_stream_profile_schema();
    for relative in STREAM_PROFILE_XSD_COPIES {
        let path = root.join(relative);
        if read(&path)? != stream_profile {
            return Err(format!(
                "{} differs from the generated stream-profile schema",
                path.display()
            ));
        }
    }
    let hash = sha256(canonical.as_bytes());
    let stream_profile_hash = sha256(stream_profile.as_bytes());
    for relative in HASH_MANIFESTS {
        let path = root.join(relative);
        let manifest = read(&path)?;
        let expected = update_hash_line(&manifest, "hcdf.xsd", &hash)?;
        let expected =
            update_hash_line(&expected, "hcdf-stream-profile.xsd", &stream_profile_hash)?;
        if expected != manifest {
            return Err(format!(
                "{} does not pin the generated hcdf.xsd",
                path.display()
            ));
        }
    }
    Ok(())
}

pub fn write_repository(root: &Path) -> Result<(), String> {
    let canonical_path = root.join("hcdf.xsd");
    let source = read(&canonical_path)?;
    let generated = generate_core_schema(&source)?;
    for relative in CORE_XSD_COPIES {
        write(&root.join(relative), generated.as_bytes())?;
    }
    let stream_profile = generate_stream_profile_schema();
    for relative in STREAM_PROFILE_XSD_COPIES {
        write(&root.join(relative), stream_profile.as_bytes())?;
    }
    let hash = sha256(generated.as_bytes());
    let stream_profile_hash = sha256(stream_profile.as_bytes());
    for relative in HASH_MANIFESTS {
        let path = root.join(relative);
        let manifest = read(&path)?;
        let updated = update_hash_line(&manifest, "hcdf.xsd", &hash)?;
        let updated = update_hash_line(&updated, "hcdf-stream-profile.xsd", &stream_profile_hash)?;
        write(&path, updated.as_bytes())?;
    }
    Ok(())
}

fn replace_block(
    source: &str,
    begin: &str,
    end: &str,
    legacy_start: &str,
    legacy_end: &str,
    body: &str,
) -> Result<String, String> {
    let replacement = if body.is_empty() {
        format!("{begin}\n{end}\n")
    } else {
        format!("{begin}\n{body}{end}\n")
    };

    if let Some(start) = source.find(begin) {
        let tail = &source[start..];
        let end_offset = tail
            .find(end)
            .ok_or_else(|| format!("found {begin:?} without matching {end:?}"))?;
        let after_marker = start + end_offset + end.len();
        let after = if source[after_marker..].starts_with('\n') {
            after_marker + 1
        } else {
            after_marker
        };
        let mut output = String::with_capacity(source.len() + replacement.len());
        output.push_str(&source[..start]);
        output.push_str(&replacement);
        output.push_str(&source[after..]);
        return Ok(output);
    }

    let start = source.find(legacy_start).ok_or_else(|| {
        format!("could not find generated marker or legacy start {legacy_start:?}")
    })?;
    let end_offset = source[start..]
        .find(legacy_end)
        .ok_or_else(|| format!("could not find legacy end {legacy_end:?}"))?;
    let after = start + end_offset;
    let mut output = String::with_capacity(source.len() + replacement.len());
    output.push_str(&source[..start]);
    output.push_str(&replacement);
    output.push_str(&source[after..]);
    Ok(output)
}

fn render_connectivity_types() -> String {
    let spec = connectivity_schema_spec();
    let mut output = String::new();
    for simple_type in &spec.simple_types {
        render_simple_type(simple_type, &mut output);
    }
    for complex_type in &spec.complex_types {
        render_complex_type(complex_type, &mut output);
    }
    output
}

fn render_simple_type(spec: &SimpleTypeSpec, output: &mut String) {
    output.push_str(&format!("  <xs:simpleType name=\"{}\">\n", spec.name));
    match &spec.kind {
        SimpleTypeKind::Enumeration(values) => {
            output.push_str("    <xs:restriction base=\"xs:string\">\n");
            for value in *values {
                output.push_str(&format!("      <xs:enumeration value=\"{value}\"/>\n"));
            }
            output.push_str("    </xs:restriction>\n");
        }
        SimpleTypeKind::DoubleList { length } => {
            output.push_str("    <xs:restriction>\n");
            output.push_str(
                "      <xs:simpleType><xs:list itemType=\"xs:double\"/></xs:simpleType>\n",
            );
            output.push_str(&format!("      <xs:length value=\"{length}\"/>\n"));
            output.push_str("    </xs:restriction>\n");
        }
        SimpleTypeKind::IntegerRange {
            base,
            min_inclusive,
            max_inclusive,
        } => {
            output.push_str(&format!("    <xs:restriction base=\"{base}\">\n"));
            output.push_str(&format!(
                "      <xs:minInclusive value=\"{min_inclusive}\"/>\n"
            ));
            output.push_str(&format!(
                "      <xs:maxInclusive value=\"{max_inclusive}\"/>\n"
            ));
            output.push_str("    </xs:restriction>\n");
        }
    }
    output.push_str("  </xs:simpleType>\n\n");
}

fn render_complex_type(spec: &ComplexTypeSpec, output: &mut String) {
    output.push_str(&format!("  <xs:complexType name=\"{}\">\n", spec.name));
    if let Some(content) = &spec.content {
        render_particle(content, 4, output);
    }
    render_attributes(&spec.attributes, 4, output);
    output.push_str("  </xs:complexType>\n\n");
}

fn render_attributes(attributes: &[AttributeSpec], indent: usize, output: &mut String) {
    let spaces = " ".repeat(indent);
    for attribute in attributes {
        output.push_str(&format!(
            "{spaces}<xs:attribute name=\"{}\" type=\"{}\"",
            attribute.name, attribute.ty
        ));
        if attribute.required {
            output.push_str(" use=\"required\"");
        }
        if let Some(default) = attribute.default {
            output.push_str(&format!(" default=\"{default}\""));
        }
        output.push_str("/>\n");
    }
}

fn render_particles(particles: &[ParticleSpec], indent: usize) -> String {
    let mut output = String::new();
    for particle in particles {
        render_particle(particle, indent, &mut output);
    }
    output
}

fn render_particle(particle: &ParticleSpec, indent: usize, output: &mut String) {
    let spaces = " ".repeat(indent);
    match particle {
        ParticleSpec::Element(element) => {
            output.push_str(&format!(
                "{spaces}<xs:element name=\"{}\" type=\"{}\"",
                element.name, element.ty
            ));
            render_occurs(element.min, element.max, output);
            output.push_str("/>\n");
        }
        ParticleSpec::Sequence(group) => {
            render_group("sequence", group, indent, output);
        }
        ParticleSpec::Choice(group) => {
            render_group("choice", group, indent, output);
        }
    }
}

fn render_group(tag: &str, group: &GroupSpec, indent: usize, output: &mut String) {
    let spaces = " ".repeat(indent);
    output.push_str(&format!("{spaces}<xs:{tag}"));
    render_occurs(group.min, group.max, output);
    output.push_str(">\n");
    for child in &group.children {
        render_particle(child, indent + 2, output);
    }
    output.push_str(&format!("{spaces}</xs:{tag}>\n"));
}

fn render_occurs(min: usize, max: Option<usize>, output: &mut String) {
    if min != 1 {
        output.push_str(&format!(" minOccurs=\"{min}\""));
    }
    match max {
        Some(1) => {}
        Some(maximum) => output.push_str(&format!(" maxOccurs=\"{maximum}\"")),
        None => output.push_str(" maxOccurs=\"unbounded\""),
    }
}

fn update_hash_line(manifest: &str, filename: &str, hash: &str) -> Result<String, String> {
    let mut found = false;
    let mut output = String::with_capacity(manifest.len());
    for line in manifest.lines() {
        if line.split_whitespace().nth(1) == Some(filename) {
            let version = line
                .split_whitespace()
                .next()
                .ok_or_else(|| format!("malformed hash line for {filename}"))?;
            output.push_str(&format!("{version:<11}{filename:<25}{hash}\n"));
            found = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if !found {
        return Err(format!("hash manifest has no entry for {filename}"));
    }
    Ok(output)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uppsala::{parse, XsdValidator};

    fn repository_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn validate(schema: &str, xml: &str) -> Vec<uppsala::ValidationError> {
        let schema_document = parse(schema).expect("schema parses as XML");
        let validator =
            XsdValidator::from_schema(&schema_document).expect("schema compiles as XSD");
        let document = parse(xml).expect("fixture parses as XML");
        validator.validate(&document)
    }

    #[test]
    fn generation_is_deterministic() {
        let source = read(&repository_root().join("hcdf.xsd")).unwrap();
        let first = generate_core_schema(&source).unwrap();
        let second = generate_core_schema(&first).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn canonical_schema_is_generated() {
        let source = read(&repository_root().join("hcdf.xsd")).unwrap();
        let generated = generate_core_schema(&source).unwrap();
        assert_eq!(
            generated, source,
            "canonical hcdf.xsd is not generated from the typed connectivity specification"
        );
    }

    #[test]
    fn canonical_stream_profile_schema_is_generated() {
        let source = read(&repository_root().join("hcdf-stream-profile.xsd")).unwrap();
        assert_eq!(
            generate_stream_profile_schema(),
            source,
            "canonical hcdf-stream-profile.xsd is not generated from the typed sidecar specification"
        );
    }

    #[test]
    fn generated_core_schema_compiles_and_accepts_minimal_hcdf() {
        let source = read(&repository_root().join("hcdf.xsd")).unwrap();
        let schema = generate_core_schema(&source).unwrap();
        let errors = validate(
            &schema,
            r#"<hcdf name="minimal" version="1.0"><comp name="base"/></hcdf>"#,
        );
        assert!(
            errors.is_empty(),
            "minimal HCDF must validate against generated schema: {errors:?}"
        );
    }

    #[test]
    fn generated_schemas_accept_typed_core_and_sidecar_stream_profile_forms() {
        let core = read(&repository_root().join("hcdf.xsd")).unwrap();
        let core_fixture = r#"<hcdf name="typed" version="1.0">
  <comp name="a">
    <port name="p"/>
    <connector name="J"><pin name="1"/></connector>
    <antenna name="ant"><radiated-port component="a" port="p"/></antenna>
  </comp>
  <comp name="b"><port name="p"/></comp>
  <link name="radio">
    <selected purpose="communication" carrier="radiated-rf"/>
    <participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>
    <participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>
  </link>
  <stream-profile uri="profiles/operational.streams.xml" selection-role="default"/>
</hcdf>"#;
        let core_errors = validate(&core, core_fixture);
        assert!(
            core_errors.is_empty(),
            "typed core fixture must validate: {core_errors:?}"
        );

        let sidecar = generate_stream_profile_schema();
        let sidecar_fixture = read(
            &repository_root().join("rust/hcdformat-rs/tests/fixtures/typed-stream-profile.xml"),
        )
        .unwrap();
        let sidecar_errors = validate(&sidecar, &sidecar_fixture);
        assert!(
            sidecar_errors.is_empty(),
            "typed sidecar fixture must validate: {sidecar_errors:?}"
        );
    }

    #[test]
    fn generated_core_schema_rejects_retired_and_ambiguous_connectivity_shapes() {
        let schema = read(&repository_root().join("hcdf.xsd")).unwrap();
        for retired_type in [
            "PortType",
            "BusType",
            "ChainTopology",
            "PhyRole",
            "CanTransceiver",
            "SerdesMedium",
            "SafetySil",
        ] {
            assert!(
                !schema.contains(&format!("<xs:simpleType name=\"{retired_type}\"")),
                "retired networking type must not leak into generated artifacts: {retired_type}"
            );
        }
        let rejected = [
            r#"<hcdf name="d" version="1.0"><network name="old"/></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p" type="CAN"/></comp></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"><pin name="1"/></port></comp></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"/><switch name="s"><input><port-ref component="a" port="p"/><channel-ref component="a" port="p" channel="c"/></input></switch></comp></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"/></comp><comp name="b"><port name="p"/></comp><link name="n"><participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant><participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant></link></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"/></comp><comp name="b"><port name="p"/></comp><link name="n"><selected purpose="communication" carrier="electrical"/><participant name="a"><endpoint><port-ref component="a" port="p"><instance/></port-ref></endpoint></participant><participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant></link></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><comp name="a"><connector name="J"><representation><derived-route><waypoint xyz="0 0 0"><frame><world/></frame></waypoint></derived-route></representation></connector></comp></hcdf>"#,
        ];
        for fixture in rejected {
            assert!(
                !validate(&schema, fixture).is_empty(),
                "retired or ambiguous connectivity fixture must be rejected: {fixture}"
            );
        }
    }

    #[test]
    fn generated_sidecar_schema_rejects_wrong_root_missing_listener_and_empty_path() {
        let schema = generate_stream_profile_schema();
        let rejected = [
            r#"<stream-profile uri="profiles/operational.streams.xml"/>"#,
            r#"<hcdf name="not-a-sidecar" version="1.0"/>"#,
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="64" interval-ns="1000"><path><network-ref network="n"/></path><talker><participant-ref network="n" participant="a"/></talker></stream></stream-profile>"#,
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="64" interval-ns="1000"><path/><talker><participant-ref network="n" participant="a"/></talker><listener><participant-ref network="n" participant="b"/></listener></stream></stream-profile>"#,
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="64" interval-ns="1000"><path><chain-ref chain="n"/></path><talker><participant-ref network="n" participant="a"/></talker><listener><participant-ref network="n" participant="b"/></listener></stream></stream-profile>"#,
            r#"<stream-profile name="p" version="1.0"><stream name="s" max-frame-size-bytes="64" interval-ns="1000"><path><network-ref network="a"/><network-ref network="b"/><forwarding><from><participant-ref network="a" participant="gateway-a"/></from><to><participant-ref network="b" participant="gateway-b"/></to></forwarding></path><talker><participant-ref network="a" participant="talker"/></talker><listener><participant-ref network="b" participant="listener"/></listener></stream></stream-profile>"#,
        ];
        for fixture in rejected {
            assert!(
                !validate(&schema, fixture).is_empty(),
                "invalid sidecar fixture must be rejected: {fixture}"
            );
        }
    }

    #[test]
    fn generated_sidecar_schema_enforces_stream_scalar_boundaries() {
        let schema = generate_stream_profile_schema();
        let lower = r#"<stream-profile name="p" version="1.0"><stream name="s" vlan-id="0" pcp="0" max-frame-size-bytes="1" interval-ns="1" max-latency-ns="1"><path><network-ref network="n"/></path><talker><participant-ref network="n" participant="a"/></talker><listener><participant-ref network="n" participant="b"/></listener><frer seamless-trees="2" sequence-encoding="r-tag"/></stream></stream-profile>"#;
        let upper = lower
            .replace("vlan-id=\"0\"", "vlan-id=\"4094\"")
            .replace("pcp=\"0\"", "pcp=\"7\"")
            .replace(
                "max-frame-size-bytes=\"1\"",
                "max-frame-size-bytes=\"4294967295\"",
            )
            .replace("interval-ns=\"1\"", "interval-ns=\"18446744073709551615\"")
            .replace(
                "max-latency-ns=\"1\"",
                "max-latency-ns=\"18446744073709551615\"",
            )
            .replace("seamless-trees=\"2\"", "seamless-trees=\"4294967295\"");
        let without_max_latency = lower.replace(" max-latency-ns=\"1\"", "");
        for (case, fixture) in [
            ("lower bounds", lower.to_owned()),
            ("upper bounds", upper),
            ("optional max latency omitted", without_max_latency),
        ] {
            let errors = validate(&schema, &fixture);
            assert!(
                errors.is_empty(),
                "stream scalar {case} must validate: {errors:?}"
            );
        }

        let rejected = [
            ("pcp below zero", lower.replace("pcp=\"0\"", "pcp=\"-1\"")),
            ("pcp above seven", lower.replace("pcp=\"0\"", "pcp=\"8\"")),
            (
                "vlan id below zero",
                lower.replace("vlan-id=\"0\"", "vlan-id=\"-1\""),
            ),
            (
                "vlan id above 4094",
                lower.replace("vlan-id=\"0\"", "vlan-id=\"4095\""),
            ),
            (
                "zero frame size",
                lower.replace("max-frame-size-bytes=\"1\"", "max-frame-size-bytes=\"0\""),
            ),
            (
                "frame size above u32",
                lower.replace(
                    "max-frame-size-bytes=\"1\"",
                    "max-frame-size-bytes=\"4294967296\"",
                ),
            ),
            (
                "zero interval",
                lower.replace("interval-ns=\"1\"", "interval-ns=\"0\""),
            ),
            (
                "interval above u64",
                lower.replace("interval-ns=\"1\"", "interval-ns=\"18446744073709551616\""),
            ),
            (
                "zero max latency",
                lower.replace("max-latency-ns=\"1\"", "max-latency-ns=\"0\""),
            ),
            (
                "max latency above u64",
                lower.replace(
                    "max-latency-ns=\"1\"",
                    "max-latency-ns=\"18446744073709551616\"",
                ),
            ),
            (
                "one seamless tree",
                lower.replace("seamless-trees=\"2\"", "seamless-trees=\"1\""),
            ),
            (
                "seamless trees above u32",
                lower.replace("seamless-trees=\"2\"", "seamless-trees=\"4294967296\""),
            ),
        ];
        for (case, fixture) in rejected {
            assert!(
                !validate(&schema, &fixture).is_empty(),
                "stream scalar {case} must be rejected"
            );
        }
    }
}
