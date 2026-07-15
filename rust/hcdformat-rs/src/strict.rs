//! Strict shape checks for HCDF-core connectivity XML.

use crate::error::{Error, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

#[derive(Clone)]
struct ElementContext {
    name: String,
    connectivity: bool,
    extension: bool,
    default_namespace: Option<String>,
}

pub(crate) fn check_core_connectivity(src: &str) -> Result<()> {
    let mut reader = Reader::from_str(src);
    let mut stack = Vec::<ElementContext>::new();
    loop {
        match reader
            .read_event()
            .map_err(|error| Error::Xml(error.to_string()))?
        {
            Event::Start(element) => {
                let local = local_name(&element);
                let default_namespace = effective_default_namespace(&element, stack.last())?;
                let context = classify(
                    &stack,
                    &local,
                    has_prefix(&element) || default_namespace.is_some(),
                    default_namespace,
                    reader.buffer_position(),
                )?;
                if context.connectivity {
                    check_attributes(
                        &element,
                        &local,
                        stack.last().map(|item| item.name.as_str()),
                    )?;
                }
                stack.push(context);
            }
            Event::Empty(element) => {
                let local = local_name(&element);
                let default_namespace = effective_default_namespace(&element, stack.last())?;
                let context = classify(
                    &stack,
                    &local,
                    has_prefix(&element) || default_namespace.is_some(),
                    default_namespace,
                    reader.buffer_position(),
                )?;
                if context.connectivity {
                    check_attributes(
                        &element,
                        &local,
                        stack.last().map(|item| item.name.as_str()),
                    )?;
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Text(text) => {
                let content = text
                    .unescape()
                    .map_err(|error| Error::Xml(error.to_string()))?;
                check_character_content(&stack, &content, reader.buffer_position())?;
            }
            Event::CData(text) => {
                let content = text
                    .decode()
                    .map_err(|error| Error::Xml(error.to_string()))?;
                check_character_content(&stack, &content, reader.buffer_position())?;
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

fn classify(
    stack: &[ElementContext],
    child: &str,
    namespaced: bool,
    default_namespace: Option<String>,
    position: u64,
) -> Result<ElementContext> {
    let Some(parent) = stack.last() else {
        reject_namespaced_core(child, namespaced, position)?;
        if child != "hcdf" {
            return Ok(ElementContext {
                name: child.to_owned(),
                connectivity: false,
                extension: false,
                default_namespace,
            });
        }
        return Ok(ElementContext {
            name: child.to_owned(),
            connectivity: false,
            extension: false,
            default_namespace,
        });
    };
    if parent.extension {
        return Ok(ElementContext {
            name: child.to_owned(),
            connectivity: false,
            extension: true,
            default_namespace,
        });
    }
    reject_namespaced_core(child, namespaced, position)?;
    if child == "extension" && matches!(parent.name.as_str(), "hcdf" | "comp" | "joint") {
        return Ok(ElementContext {
            name: child.to_owned(),
            connectivity: false,
            extension: true,
            default_namespace,
        });
    }
    let connectivity = if parent.connectivity {
        if !allowed_connectivity_child(&parent.name, child, grandparent(stack)) {
            return Err(unknown_element(child, &parent.name, position));
        }
        true
    } else if parent.name == "hcdf" {
        if root_connectivity_child(child) {
            true
        } else if root_structural_child(child) {
            false
        } else {
            return Err(unknown_element(child, "hcdf", position));
        }
    } else if parent.name == "comp" {
        if component_connectivity_child(child) {
            true
        } else if component_structural_child(child) {
            false
        } else {
            return Err(unknown_element(child, "comp", position));
        }
    } else {
        false
    };
    Ok(ElementContext {
        name: child.to_owned(),
        connectivity,
        extension: false,
        default_namespace,
    })
}

fn grandparent(stack: &[ElementContext]) -> Option<&str> {
    stack
        .len()
        .checked_sub(2)
        .and_then(|index| stack.get(index))
        .map(|context| context.name.as_str())
}

fn root_structural_child(name: &str) -> bool {
    matches!(
        name,
        "description"
            | "author"
            | "license"
            | "url"
            | "comp"
            | "joint"
            | "group"
            | "state"
            | "self-collision-disable"
            | "transmission"
            | "color"
            | "include"
            | "extension"
    )
}

fn root_connectivity_child(name: &str) -> bool {
    matches!(
        name,
        "harness"
            | "cable"
            | "plumbing"
            | "umbilical"
            | "binding"
            | "mate"
            | "link"
            | "bus"
            | "chain"
            | "star"
            | "ring"
            | "mesh"
            | "tree"
            | "stream-profile"
    )
}

fn component_structural_child(name: &str) -> bool {
    matches!(
        name,
        "description"
            | "board"
            | "operating-temp"
            | "inertial"
            | "visual"
            | "collision"
            | "frame"
            | "sensor"
            | "motor"
            | "hmi"
            | "dynamic-surface"
            | "power-source"
            | "software"
            | "discovered"
            | "urdf-compat"
            | "extension"
    )
}

fn component_connectivity_child(name: &str) -> bool {
    matches!(
        name,
        "port"
            | "connector"
            | "antenna"
            | "switch"
            | "bridge"
            | "converter"
            | "transceiver"
            | "radio"
            | "wire"
            | "conductor"
            | "cable-member"
            | "fiber"
            | "coax"
            | "waveguide"
            | "feed"
            | "hose"
            | "pipe"
            | "passage"
            | "splice"
            | "tee"
            | "manifold"
            | "busbar"
            | "optical-splitter"
            | "termination"
    )
}

fn allowed_connectivity_child(parent: &str, child: &str, grandparent: Option<&str>) -> bool {
    match parent {
        "harness" | "cable" | "plumbing" | "umbilical" => {
            matches!(
                child,
                "connector"
                    | "wire"
                    | "conductor"
                    | "cable-member"
                    | "fiber"
                    | "coax"
                    | "waveguide"
                    | "feed"
                    | "hose"
                    | "pipe"
                    | "passage"
                    | "splice"
                    | "tee"
                    | "manifold"
                    | "busbar"
                    | "optical-splitter"
                    | "termination"
                    | "representation"
            )
        }
        "port" => matches!(child, "capabilities" | "channel"),
        "channel" if grandparent == Some("rf") => {
            matches!(child, "numbered" | "frequency-defined")
        }
        "channel" => child == "capabilities",
        "capabilities" => matches!(
            child,
            "purpose"
                | "carrier"
                | "profile"
                | "rate"
                | "voltage"
                | "current"
                | "power"
                | "impedance"
                | "frequency"
                | "bandwidth"
                | "pressure"
                | "flow"
                | "temperature"
        ),
        "connector" => matches!(
            child,
            "pin"
                | "socket"
                | "contact"
                | "fiber"
                | "passage"
                | "feed"
                | "waveguide-opening"
                | "representation"
        ),
        "pin" | "socket" | "contact" | "waveguide-opening" => child == "representation",
        "fiber" | "passage" | "feed" if grandparent == Some("connector") => {
            child == "representation"
        }
        "antenna" => matches!(child, "conducted-port" | "radiated-port" | "representation"),
        "switch" | "bridge" | "converter" | "transceiver" | "radio" => {
            matches!(child, "input" | "output" | "bidirectional")
        }
        "input" | "output" | "bidirectional" | "functional" | "endpoint" => {
            matches!(child, "port-ref" | "channel-ref")
        }
        "wire" | "conductor" | "cable-member" | "coax" | "waveguide" | "hose" | "pipe"
            if grandparent != Some("connector") =>
        {
            matches!(child, "first" | "second" | "representation")
        }
        "fiber" | "passage" | "feed" if grandparent != Some("connector") => {
            matches!(child, "first" | "second" | "representation")
        }
        "splice" | "tee" | "manifold" | "busbar" | "optical-splitter" => {
            matches!(child, "attachment" | "representation")
        }
        "termination" => matches!(child, "attachment" | "quantity" | "representation"),
        "binding" => matches!(child, "functional" | "physical"),
        "mate" => matches!(child, "first" | "second" | "position-mapping"),
        "position-mapping" => matches!(child, "first" | "second"),
        "first" | "second" if matches!(grandparent, Some("mate" | "position-mapping")) => {
            matches!(child, "component-ref" | "assembly-ref")
        }
        "first" | "second" => matches!(child, "connector-ref" | "position-ref" | "junction-ref"),
        "physical" | "attachment" => {
            matches!(child, "connector-ref" | "position-ref" | "junction-ref")
        }
        "connector-ref" | "position-ref" | "junction-ref" => {
            matches!(child, "component-ref" | "assembly-ref")
        }
        "participant" => child == "endpoint",
        "hop" => matches!(child, "description" | "owner"),
        "leg" => matches!(child, "from" | "to"),
        "owner" => matches!(child, "component-ref" | "function-ref"),
        "coordinator" => child == "participant-ref",
        "root" => child == "hop-ref",
        "configuration" => matches!(
            child,
            "gptp-domain"
                | "traffic-class"
                | "gate-schedule"
                | "schedule-assignment"
                | "plca"
                | "macsec"
                | "eee"
        ),
        "gptp-domain" => matches!(child, "clock" | "port-defaults"),
        "clock" => child == "participant-ref",
        "traffic-class" => matches!(child, "description" | "pcp"),
        "gate-schedule" => child == "gate",
        "gate" => child == "open",
        "open" => child == "traffic-class-ref",
        "schedule-assignment" => matches!(child, "schedule-ref" | "target"),
        "target" if grandparent == Some("override") => {
            matches!(child, "network-ref" | "leg-ref")
        }
        "target" => child == "participant-ref",
        "plca" => child == "node",
        "node" if grandparent == Some("plca") => child == "participant-ref",
        "macsec" => matches!(child, "policy" | "default-policy" | "override"),
        "default-policy" => child == "macsec-policy-ref",
        "override" if grandparent == Some("macsec") => {
            matches!(child, "target" | "macsec-policy-ref")
        }
        "override" if grandparent == Some("eee") => child == "participant-ref",
        "eee" => child == "override",
        "from" | "to" if grandparent == Some("leg") => {
            matches!(child, "hop-ref" | "participant-ref")
        }
        "port-ref" | "channel-ref" | "conducted-port" | "radiated-port" | "component-ref"
        | "assembly-ref" | "function-ref" | "participant-ref" | "hop-ref" | "leg-ref"
        | "traffic-class-ref" | "schedule-ref" | "component-visual" | "assembly-model"
        | "component-origin" | "component-frame" | "network-ref" | "macsec-policy-ref" => {
            child == "instance"
        }
        "instance" => child == "segment",
        "representation" => matches!(
            child,
            "box" | "cylinder" | "sphere" | "model" | "model-part" | "derived-route"
        ),
        "box" | "cylinder" | "sphere" | "model" => child == "placement",
        "model-part" => child == "model-root",
        "model-root" => matches!(child, "component-visual" | "assembly-model"),
        "placement" => matches!(child, "frame" | "rotation"),
        "waypoint" => matches!(child, "frame" | "rotation"),
        "frame" => matches!(child, "world" | "component-origin" | "component-frame"),
        "rotation" => matches!(child, "rpy" | "quaternion"),
        "derived-route" => matches!(child, "round-section" | "rectangular-section" | "waypoint"),
        "link" | "bus" | "mesh" if grandparent == Some("hcdf") => {
            matches!(
                child,
                "description" | "selected" | "configuration" | "participant"
            )
        }
        "star" if grandparent == Some("hcdf") => {
            matches!(
                child,
                "description" | "selected" | "configuration" | "coordinator" | "participant"
            )
        }
        "chain" | "ring" if grandparent == Some("hcdf") => {
            matches!(
                child,
                "description" | "selected" | "configuration" | "participant" | "hop" | "leg"
            )
        }
        "tree" if grandparent == Some("hcdf") => {
            matches!(
                child,
                "description"
                    | "selected"
                    | "configuration"
                    | "root"
                    | "participant"
                    | "hop"
                    | "leg"
            )
        }
        "stream-profile" if grandparent == Some("hcdf") => false,
        "selected" => matches!(
            child,
            "profile"
                | "rate"
                | "voltage"
                | "current"
                | "power"
                | "impedance"
                | "frequency"
                | "pressure"
                | "flow"
                | "temperature"
                | "rf"
        ),
        "rf" => child == "channel",
        "numbered" | "frequency-defined" if grandparent == Some("channel") => {
            matches!(child, "center-frequency" | "bandwidth")
        }
        "center-frequency" | "bandwidth"
            if matches!(grandparent, Some("numbered" | "frequency-defined")) =>
        {
            matches!(child, "nominal" | "range")
        }
        "rate" | "voltage" | "current" | "power" | "impedance" | "frequency" | "pressure"
        | "flow" | "temperature"
            if grandparent == Some("selected") =>
        {
            matches!(child, "nominal" | "range")
        }
        _ => false,
    }
}

fn check_character_content(stack: &[ElementContext], content: &str, position: u64) -> Result<()> {
    let Some(context) = stack.last() else {
        return Ok(());
    };
    if !context.connectivity
        || context.extension
        || context.name == "description"
        || content.trim().is_empty()
    {
        return Ok(());
    }
    Err(Error::Xml(format!(
        "unexpected character content in HCDF core connectivity element <{}> near byte {position}",
        context.name
    )))
}

fn check_attributes(element: &BytesStart<'_>, name: &str, parent: Option<&str>) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Xml(error.to_string()))?;
        let key = String::from_utf8_lossy(attribute.key.as_ref());
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        if !allowed_attribute(name, &key, parent) {
            return Err(Error::Xml(format!(
                "unknown HCDF core connectivity attribute @{key} on <{name}>"
            )));
        }
    }
    Ok(())
}

fn allowed_attribute(element: &str, attribute: &str, parent: Option<&str>) -> bool {
    match element {
        "channel" if parent == Some("rf") => false,
        "numbered" => attribute == "number",
        "harness" | "cable" | "plumbing" | "umbilical" | "antenna" | "switch" | "bridge"
        | "converter" | "transceiver" | "radio" | "link" | "bus" | "chain" | "star" | "ring"
        | "mesh" | "tree" => attribute == "name",
        "stream-profile" if parent == Some("hcdf") => {
            matches!(attribute, "uri" | "sha" | "required" | "selection-role")
        }
        "port" => attribute == "name",
        "channel" => matches!(attribute, "name" | "role" | "local-group"),
        "connector" => matches!(attribute, "name" | "family"),
        "splice" | "tee" | "manifold" | "busbar" | "optical-splitter" => {
            matches!(attribute, "name" | "fidelity")
        }
        "wire" | "conductor" | "cable-member" | "fiber" | "coax" | "waveguide" | "feed"
        | "hose" | "pipe" | "passage"
            if parent != Some("connector") =>
        {
            matches!(attribute, "name" | "fidelity" | "role" | "local-group")
        }
        "pin" | "socket" | "contact" | "fiber" | "passage" | "feed" | "waveguide-opening"
            if parent == Some("connector") =>
        {
            matches!(attribute, "name" | "role" | "local-group")
        }
        "termination" => matches!(
            attribute,
            "name" | "kind" | "mounting" | "profile" | "fidelity"
        ),
        "binding" | "mate" => matches!(attribute, "name" | "fidelity"),
        "participant" => matches!(attribute, "name" | "role"),
        "hop" => matches!(attribute, "name" | "role" | "processing-delay-ns"),
        "leg" => attribute == "name",
        "gptp-domain" => matches!(attribute, "name" | "number"),
        "clock" => matches!(
            attribute,
            "name"
                | "kind"
                | "gm-capable"
                | "priority1"
                | "priority2"
                | "clock-class"
                | "clock-accuracy"
        ),
        "port-defaults" => matches!(
            attribute,
            "log-sync-interval"
                | "log-announce-interval"
                | "log-pdelay-req-interval"
                | "announce-receipt-timeout"
                | "neighbor-prop-delay-threshold-ns"
        ),
        "traffic-class" => matches!(attribute, "name" | "number" | "preemption"),
        "pcp" => attribute == "value",
        "gate-schedule" => matches!(attribute, "name" | "cycle-time-ns"),
        "gate" => attribute == "duration-ns",
        "schedule-assignment" => attribute == "name",
        "plca" => matches!(attribute, "max-node-id" | "to-timer-bit-times"),
        "node" if parent == Some("plca") => {
            matches!(attribute, "id" | "burst-count" | "burst-timer-bit-times")
        }
        "policy" if parent == Some("macsec") => matches!(
            attribute,
            "name"
                | "enforcement"
                | "cipher"
                | "key-agreement"
                | "confidentiality-offset"
                | "rekey-interval-ns"
                | "credential-store-ref"
        ),
        "eee" => attribute == "default-mode",
        "override" if parent == Some("eee") => attribute == "mode",
        "selected" => matches!(attribute, "purpose" | "carrier"),
        "purpose" | "carrier" => attribute == "value",
        "profile" => attribute == "id",
        "rate" | "voltage" | "current" | "power" | "impedance" | "frequency" | "bandwidth"
        | "pressure" | "flow" | "temperature"
            if parent == Some("capabilities") =>
        {
            matches!(attribute, "min" | "max" | "nominal" | "unit")
        }
        "rate" | "voltage" | "current" | "power" | "impedance" | "frequency" | "pressure"
        | "flow" | "temperature"
            if parent == Some("selected") =>
        {
            false
        }
        "nominal" => matches!(attribute, "value" | "unit"),
        "range" => matches!(attribute, "min" | "max" | "nominal" | "unit"),
        "quantity" => matches!(attribute, "property" | "value" | "unit"),
        "port-ref" | "conducted-port" | "radiated-port" => {
            matches!(attribute, "component" | "port")
        }
        "channel-ref" => matches!(attribute, "component" | "port" | "channel"),
        "component-ref" => attribute == "component",
        "assembly-ref" => attribute == "assembly",
        "function-ref" => matches!(attribute, "component" | "function"),
        "participant-ref" => matches!(attribute, "network" | "participant"),
        "hop-ref" => matches!(attribute, "network" | "hop"),
        "leg-ref" => matches!(attribute, "network" | "leg"),
        "traffic-class-ref" => matches!(attribute, "network" | "traffic-class"),
        "schedule-ref" => matches!(attribute, "network" | "schedule"),
        "network-ref" => attribute == "network",
        "macsec-policy-ref" => matches!(attribute, "network" | "policy"),
        "connector-ref" => attribute == "connector",
        "position-ref" => matches!(attribute, "connector" | "position"),
        "junction-ref" => attribute == "junction",
        "first" | "second" if parent == Some("mate") => attribute == "connector",
        "first" | "second" if parent == Some("position-mapping") => {
            matches!(attribute, "connector" | "position")
        }
        "segment" => matches!(attribute, "name" | "occurrence"),
        "box" => attribute == "size",
        "cylinder" => matches!(attribute, "radius" | "length"),
        "sphere" => attribute == "radius",
        "model" => matches!(attribute, "uri" | "sha" | "node-path"),
        "model-part" => matches!(attribute, "node-path" | "submesh-fallback"),
        "component-visual" => matches!(attribute, "component" | "visual"),
        "assembly-model" => attribute == "assembly",
        "component-origin" => attribute == "component",
        "component-frame" => matches!(attribute, "component" | "frame"),
        "rpy" | "quaternion" => attribute == "value",
        "round-section" => attribute == "diameter",
        "rectangular-section" => matches!(attribute, "width" | "height"),
        "derived-route" => false,
        "placement" | "waypoint" => attribute == "xyz",
        "world" => false,
        "capabilities" | "representation" | "input" | "output" | "bidirectional" | "functional"
        | "endpoint" | "physical" | "attachment" | "position-mapping" | "instance"
        | "model-root" | "frame" | "rotation" | "frequency-defined" | "rf" | "center-frequency"
        | "bandwidth" | "configuration" | "open" | "target" | "macsec" | "default-policy"
        | "override" => false,
        _ => false,
    }
}

fn local_name(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(element.local_name().as_ref()).into_owned()
}

fn has_prefix(element: &BytesStart<'_>) -> bool {
    element.name().as_ref().contains(&b':')
}

fn effective_default_namespace(
    element: &BytesStart<'_>,
    parent: Option<&ElementContext>,
) -> Result<Option<String>> {
    let inherited = parent.and_then(|context| context.default_namespace.clone());
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Xml(error.to_string()))?;
        if attribute.key.as_ref() == b"xmlns" {
            let value = attribute
                .unescape_value()
                .map_err(|error| Error::Xml(error.to_string()))?;
            return if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value.into_owned()))
            };
        }
    }
    Ok(inherited)
}

fn reject_namespaced_core(child: &str, namespaced: bool, position: u64) -> Result<()> {
    if namespaced {
        return Err(Error::Xml(format!(
            "namespaced HCDF core element <{child}> is not allowed outside <extension> near byte {position}"
        )));
    }
    Ok(())
}

fn unknown_element(child: &str, parent: &str, position: u64) -> Error {
    Error::Xml(format!(
        "unknown HCDF core element <{child}> inside <{parent}> near byte {position}"
    ))
}
