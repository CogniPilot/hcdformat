//! Typed authored model for external HCDF stream-profile documents.
//!
//! Stream definitions live only at the sidecar root. A stream joins an optional named group through
//! `<group-ref>`, and its route is a directed graph over existing HCDF networks. All references use
//! the same structured include-instance shape as the core connectivity model.

use super::connectivity::QualifiedId;
use super::connectivity_xml::{
    ConnectivityFunctionRef, InstanceRef, NetworkRef, ParticipantRef, ScheduleRef, TrafficClassRef,
};
use crate::error::{Error, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// A stream-profile resource declared by the root HCDF document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "stream-profile", deny_unknown_fields)]
pub struct StreamProfileResource {
    #[serde(rename = "@uri")]
    pub uri: String,
    #[serde(rename = "@sha", default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(
        rename = "@required",
        default = "default_true",
        skip_serializing_if = "is_true"
    )]
    pub required: bool,
    #[serde(
        rename = "@selection-role",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub selection_role: Option<StreamProfileSelectionRole>,
}

/// The root document may identify one profile as the default selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamProfileSelectionRole {
    #[serde(rename = "default")]
    Default,
}

/// Root `<stream-profile>` sidecar document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename = "stream-profile", deny_unknown_fields)]
pub struct StreamProfileDocument {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@version")]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency: Vec<StreamProfileDependency>,
    #[serde(
        rename = "stream-group",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub stream_group: Vec<StreamGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream: Vec<StreamDefinition>,
}

impl StreamProfileDocument {
    /// Parse a stream-profile sidecar while rejecting unknown or retired core XML at every depth.
    pub fn from_xml_str(source: &str) -> Result<Self> {
        check_stream_profile_xml(source)?;
        let document: Self =
            quick_xml::de::from_str(source).map_err(|error| Error::Xml(error.to_string()))?;
        document.validate()?;
        Ok(document)
    }

    /// Validate constraints shared by parsed documents and programmatically authored documents.
    pub fn validate(&self) -> Result<()> {
        for stream in &self.stream {
            if stream.listener.is_empty() {
                return Err(Error::Xml(format!(
                    "stream {:?} must declare at least one <listener>",
                    stream.name
                )));
            }
            if stream.path.network.is_empty() {
                return Err(Error::Xml(format!(
                    "stream {:?} path must declare at least one <network-ref>",
                    stream.name
                )));
            }
            if stream.vlan_id.is_some_and(|vlan_id| vlan_id > 4094) {
                return Err(Error::Xml(format!(
                    "stream {:?} vlan-id must be in 0..=4094",
                    stream.name
                )));
            }
            if stream.pcp.is_some_and(|pcp| pcp > 7) {
                return Err(Error::Xml(format!(
                    "stream {:?} pcp must be in 0..=7",
                    stream.name
                )));
            }
            if stream.max_frame_size_bytes == 0 {
                return Err(Error::Xml(format!(
                    "stream {:?} max-frame-size-bytes must be greater than zero",
                    stream.name
                )));
            }
            if stream.interval_ns == 0 {
                return Err(Error::Xml(format!(
                    "stream {:?} interval-ns must be greater than zero",
                    stream.name
                )));
            }
            if stream.max_latency_ns == Some(0) {
                return Err(Error::Xml(format!(
                    "stream {:?} max-latency-ns must be greater than zero when present",
                    stream.name
                )));
            }
            if stream
                .frer
                .as_ref()
                .is_some_and(|frer| frer.seamless_trees < 2)
            {
                return Err(Error::Xml(format!(
                    "stream {:?} frer seamless-trees must be at least 2",
                    stream.name
                )));
            }

            validate_instance_ref(
                stream
                    .group_ref
                    .as_ref()
                    .and_then(|reference| reference.instance.as_ref()),
                &stream.name,
                "group-ref",
            )?;
            for network in &stream.path.network {
                validate_instance_ref(network.instance.as_ref(), &stream.name, "path network-ref")?;
            }
            for forwarding in &stream.path.forwarding {
                validate_instance_ref(
                    forwarding.from.participant.instance.as_ref(),
                    &stream.name,
                    "forwarding from participant-ref",
                )?;
                validate_instance_ref(
                    forwarding.function.instance.as_ref(),
                    &stream.name,
                    "forwarding function-ref",
                )?;
                validate_instance_ref(
                    forwarding.to.participant.instance.as_ref(),
                    &stream.name,
                    "forwarding to participant-ref",
                )?;
            }
            validate_instance_ref(
                stream.talker.participant.instance.as_ref(),
                &stream.name,
                "talker participant-ref",
            )?;
            for listener in &stream.listener {
                validate_instance_ref(
                    listener.participant.instance.as_ref(),
                    &stream.name,
                    "listener participant-ref",
                )?;
            }
            validate_instance_ref(
                stream
                    .traffic_class_ref
                    .as_ref()
                    .and_then(|reference| reference.instance.as_ref()),
                &stream.name,
                "traffic-class-ref",
            )?;
            validate_instance_ref(
                stream
                    .schedule_ref
                    .as_ref()
                    .and_then(|reference| reference.instance.as_ref()),
                &stream.name,
                "schedule-ref",
            )?;
        }
        Ok(())
    }

    /// Serialize a stream-profile sidecar with deterministic two-space indentation.
    pub fn to_xml_string(&self) -> Result<String> {
        self.validate()?;
        let mut output = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut output);
        serializer.indent(' ', 2);
        self.serialize(serializer)
            .map_err(|error| Error::Xml(error.to_string()))?;
        Ok(output)
    }
}

fn validate_instance_ref(
    instance: Option<&InstanceRef>,
    stream: &str,
    context: &str,
) -> Result<()> {
    if instance.is_some_and(|instance| instance.segment.is_empty()) {
        return Err(Error::Xml(format!(
            "stream {stream:?} {context} instance must declare at least one <segment>"
        )));
    }
    Ok(())
}

/// A nested stream-profile dependency. Loader recursion and merge behavior are separate concerns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamProfileDependency {
    #[serde(rename = "@uri")]
    pub uri: String,
    #[serde(rename = "@sha", default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(
        rename = "@required",
        default = "default_true",
        skip_serializing_if = "is_true"
    )]
    pub required: bool,
}

/// A named organizational group. Streams reference it from their authoritative top-level definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamGroup {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A structured reference from a stream to a named group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamGroupRef {
    #[serde(rename = "@group")]
    pub group: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceRef>,
}

/// One directed route graph over existing HCDF network topologies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamPath {
    #[serde(rename = "network-ref", default)]
    pub network: Vec<NetworkRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwarding: Vec<StreamForwarding>,
}

/// One endpoint of inter-topology stream forwarding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamForwardingEnd {
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

/// One explicit directed forwarding relation between participant endpoints on two route networks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamForwarding {
    pub from: StreamForwardingEnd,
    #[serde(rename = "function-ref")]
    pub function: ConnectivityFunctionRef,
    pub to: StreamForwardingEnd,
}

/// The unique transmitting participant of a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamTalker {
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

/// One receiving participant of a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamListener {
    #[serde(rename = "participant-ref")]
    pub participant: ParticipantRef,
}

/// IEEE 802.1CB frame replication and elimination requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frer {
    #[serde(rename = "@seamless-trees")]
    pub seamless_trees: u32,
    #[serde(rename = "@sequence-encoding")]
    pub sequence_encoding: FrerSequenceEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrerSequenceEncoding {
    #[serde(rename = "r-tag")]
    RTag,
    #[serde(rename = "hsr")]
    Hsr,
}

/// One authoritative top-level operational stream definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamDefinition {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@vlan-id", default, skip_serializing_if = "Option::is_none")]
    pub vlan_id: Option<u16>,
    #[serde(rename = "@pcp", default, skip_serializing_if = "Option::is_none")]
    pub pcp: Option<u8>,
    #[serde(rename = "@max-frame-size-bytes")]
    pub max_frame_size_bytes: u32,
    #[serde(rename = "@interval-ns")]
    pub interval_ns: u64,
    #[serde(
        rename = "@max-latency-ns",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub max_latency_ns: Option<u64>,
    #[serde(rename = "@protocol", default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<QualifiedId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "group-ref", default, skip_serializing_if = "Option::is_none")]
    pub group_ref: Option<StreamGroupRef>,
    pub path: StreamPath,
    pub talker: StreamTalker,
    #[serde(default)]
    pub listener: Vec<StreamListener>,
    #[serde(
        rename = "traffic-class-ref",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub traffic_class_ref: Option<TrafficClassRef>,
    #[serde(
        rename = "schedule-ref",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub schedule_ref: Option<ScheduleRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frer: Option<Frer>,
}

fn check_stream_profile_xml(source: &str) -> Result<()> {
    let mut reader = Reader::from_str(source);
    let mut stack = Vec::<ElementFrame>::new();
    let mut saw_root = false;
    loop {
        match reader
            .read_event()
            .map_err(|error| Error::Xml(error.to_string()))?
        {
            Event::Start(element) => {
                let frame =
                    check_element(&element, &mut stack, saw_root, reader.buffer_position())?;
                if stack.is_empty() {
                    saw_root = true;
                }
                stack.push(frame);
            }
            Event::Empty(element) => {
                let frame =
                    check_element(&element, &mut stack, saw_root, reader.buffer_position())?;
                if stack.is_empty() {
                    saw_root = true;
                }
                frame.finish(reader.buffer_position())?;
            }
            Event::End(_) => {
                let frame = stack.pop().ok_or_else(|| {
                    Error::Xml("unexpected stream-profile closing element".to_owned())
                })?;
                frame.finish(reader.buffer_position())?;
            }
            Event::Text(text) => {
                let content = text
                    .unescape()
                    .map_err(|error| Error::Xml(error.to_string()))?;
                check_text(&stack, &content, reader.buffer_position())?;
            }
            Event::CData(text) => {
                let content = text
                    .decode()
                    .map_err(|error| Error::Xml(error.to_string()))?;
                check_text(&stack, &content, reader.buffer_position())?;
            }
            Event::Eof => {
                if !saw_root {
                    return Err(Error::Xml(
                        "expected root <stream-profile> element".to_owned(),
                    ));
                }
                return Ok(());
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ChildSpec {
    name: &'static str,
    min: usize,
    max: Option<usize>,
}

const fn child(name: &'static str, min: usize, max: Option<usize>) -> ChildSpec {
    ChildSpec { name, min, max }
}

const ROOT_CHILDREN: [ChildSpec; 4] = [
    child("description", 0, Some(1)),
    child("dependency", 0, None),
    child("stream-group", 0, None),
    child("stream", 0, None),
];
const GROUP_CHILDREN: [ChildSpec; 1] = [child("description", 0, Some(1))];
const STREAM_CHILDREN: [ChildSpec; 8] = [
    child("description", 0, Some(1)),
    child("group-ref", 0, Some(1)),
    child("path", 1, Some(1)),
    child("talker", 1, Some(1)),
    child("listener", 1, None),
    child("traffic-class-ref", 0, Some(1)),
    child("schedule-ref", 0, Some(1)),
    child("frer", 0, Some(1)),
];
const PATH_CHILDREN: [ChildSpec; 2] = [child("network-ref", 1, None), child("forwarding", 0, None)];
const FORWARDING_CHILDREN: [ChildSpec; 3] = [
    child("from", 1, Some(1)),
    child("function-ref", 1, Some(1)),
    child("to", 1, Some(1)),
];
const PARTICIPANT_CHILDREN: [ChildSpec; 1] = [child("participant-ref", 1, Some(1))];
const REFERENCE_CHILDREN: [ChildSpec; 1] = [child("instance", 0, Some(1))];
const INSTANCE_CHILDREN: [ChildSpec; 1] = [child("segment", 1, None)];

fn child_grammar(element: &str) -> &'static [ChildSpec] {
    match element {
        "stream-profile" => &ROOT_CHILDREN,
        "stream-group" => &GROUP_CHILDREN,
        "stream" => &STREAM_CHILDREN,
        "path" => &PATH_CHILDREN,
        "forwarding" => &FORWARDING_CHILDREN,
        "from" | "to" | "talker" | "listener" => &PARTICIPANT_CHILDREN,
        "group-ref" | "network-ref" | "function-ref" | "participant-ref" | "traffic-class-ref"
        | "schedule-ref" => &REFERENCE_CHILDREN,
        "instance" => &INSTANCE_CHILDREN,
        _ => &[],
    }
}

struct ElementFrame {
    name: String,
    child_counts: Vec<usize>,
    last_child_index: Option<usize>,
}

impl ElementFrame {
    fn new(name: String) -> Self {
        Self {
            child_counts: vec![0; child_grammar(&name).len()],
            name,
            last_child_index: None,
        }
    }

    fn accept_child(&mut self, child_name: &str, position: u64) -> Result<()> {
        let grammar = child_grammar(&self.name);
        let Some(index) = grammar.iter().position(|spec| spec.name == child_name) else {
            return Err(Error::Xml(format!(
                "unknown stream-profile core element <{child_name}> inside <{}> near byte {position}",
                self.name
            )));
        };
        if self.last_child_index.is_some_and(|last| index < last) {
            return Err(Error::Xml(format!(
                "stream-profile element <{child_name}> is out of order inside <{}> near byte {position}",
                self.name
            )));
        }
        self.child_counts[index] += 1;
        if grammar[index]
            .max
            .is_some_and(|maximum| self.child_counts[index] > maximum)
        {
            return Err(Error::Xml(format!(
                "stream-profile element <{child_name}> occurs too many times inside <{}> near byte {position}",
                self.name
            )));
        }
        self.last_child_index = Some(index);
        Ok(())
    }

    fn finish(self, position: u64) -> Result<()> {
        for (spec, count) in child_grammar(&self.name).iter().zip(self.child_counts) {
            if count < spec.min {
                if spec.min == 1 {
                    return Err(Error::Xml(format!(
                        "stream-profile element <{}> must declare at least one <{}> child element near byte {position}",
                        self.name, spec.name
                    )));
                }
                return Err(Error::Xml(format!(
                    "stream-profile element <{}> requires at least {} <{}> child element(s) near byte {position}",
                    self.name, spec.min, spec.name
                )));
            }
        }
        Ok(())
    }
}

fn check_element(
    element: &BytesStart<'_>,
    stack: &mut [ElementFrame],
    saw_root: bool,
    position: u64,
) -> Result<ElementFrame> {
    let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
    if element.name().as_ref().contains(&b':') || has_nonempty_namespace(element)? {
        return Err(Error::Xml(format!(
            "namespaced stream-profile core element <{name}> is not allowed near byte {position}"
        )));
    }

    match stack.last_mut() {
        None if saw_root || name != "stream-profile" => {
            return Err(Error::Xml(format!(
                "expected root <stream-profile>, found <{name}> near byte {position}"
            )));
        }
        Some(parent) => {
            parent.accept_child(&name, position)?;
        }
        _ => {}
    }

    check_attributes(element, &name)?;
    Ok(ElementFrame::new(name))
}

fn check_attributes(element: &BytesStart<'_>, name: &str) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Xml(error.to_string()))?;
        let key = String::from_utf8_lossy(attribute.key.as_ref());
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        if !allowed_attribute(name, &key) {
            return Err(Error::Xml(format!(
                "unknown stream-profile core attribute @{key} on <{name}>"
            )));
        }
    }
    Ok(())
}

fn allowed_attribute(element: &str, attribute: &str) -> bool {
    match element {
        "stream-profile" => matches!(attribute, "name" | "version"),
        "dependency" => matches!(attribute, "uri" | "sha" | "required"),
        "stream-group" => attribute == "name",
        "stream" => matches!(
            attribute,
            "name"
                | "vlan-id"
                | "pcp"
                | "max-frame-size-bytes"
                | "interval-ns"
                | "max-latency-ns"
                | "protocol"
        ),
        "group-ref" => attribute == "group",
        "network-ref" => attribute == "network",
        "function-ref" => matches!(attribute, "component" | "function"),
        "participant-ref" => matches!(attribute, "network" | "participant"),
        "traffic-class-ref" => matches!(attribute, "network" | "traffic-class"),
        "schedule-ref" => matches!(attribute, "network" | "schedule"),
        "segment" => matches!(attribute, "name" | "occurrence"),
        "frer" => matches!(attribute, "seamless-trees" | "sequence-encoding"),
        "description" | "path" | "forwarding" | "from" | "to" | "talker" | "listener"
        | "instance" => false,
        _ => false,
    }
}

fn has_nonempty_namespace(element: &BytesStart<'_>) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Xml(error.to_string()))?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            let value = attribute
                .unescape_value()
                .map_err(|error| Error::Xml(error.to_string()))?;
            if !value.is_empty() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn check_text(stack: &[ElementFrame], content: &str, position: u64) -> Result<()> {
    let Some(element) = stack.last() else {
        return Ok(());
    };
    if element.name == "description" || content.trim().is_empty() {
        return Ok(());
    }
    Err(Error::Xml(format!(
        "unexpected character content in stream-profile core element <{}> near byte {position}",
        element.name
    )))
}
