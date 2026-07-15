//! Reading official-format HCDF (quick-xml + serde) with verbatim `<extension>` body capture and
//! XML comment capture.
//!
//! quick-xml's serde cannot round-trip arbitrary nested XML (the lax `xs:any` extension body) and
//! skips comments entirely, so we preprocess: ONE quick-xml event pass ([`crate::comments`]) rewrites
//! every `<extension ...>...</extension>` into a skeleton `<extension ... hcdf-ext-id="N"/>` recording
//! the inner XML by id, and strips every `<!-- -->` into an anchored side-channel. serde then parses
//! the skeleton; afterwards the captured bodies are reattached to the matching [`Extension`] structs
//! by id (robust to nesting and ordering) and the comments are distributed onto the model
//! ([`crate::comments::distribute`]). Documents without the `<extension` and `<!--` substrings skip
//! the rewrite entirely (a cheap scan; the pass would be an identity copy). See [`crate::ser`] for
//! the inverse (skeleton serialize + side-channel splice).
use crate::comments;
use crate::error::{Error, Result};
use crate::model::extension::Extension;
use crate::model::Hcdf;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

impl Hcdf {
    /// Parse an official-format HCDF document from a string.
    ///
    /// Enforces the version policy first ([`crate::version::check_readable`]): a root `@version`
    /// that is malformed or of a foreign MAJOR is [`Error::UnsupportedVersion`] before any
    /// structural parse; an absent/empty one reads as 1.0 and same-MAJOR minors load.
    pub fn from_xml_str(s: &str) -> Result<Hcdf> {
        crate::version::check_readable(root_version(s)?.as_deref().unwrap_or(""))?;
        crate::strict::check_core_connectivity(s)?;
        // Fast path for the common extension-free, comment-free document: an `<extension>` element
        // cannot exist without the literal bytes `<extension` (XML admits no whitespace between `<`
        // and the tag name) and a comment cannot exist without `<!--`, so absence of both substrings
        // proves the capture rewrite would be an identity copy. False positives (`<extensions...>`
        // prefix collisions, or either substring inside a comment/CDATA) merely take the capture
        // path spuriously, which stays correct.
        let doc = if !s.contains("<extension") && !s.contains("<!--") {
            quick_xml::de::from_str(s).map_err(|e| Error::Xml(e.to_string()))?
        } else {
            parse_with_capture(s)?
        };
        Ok(doc)
    }
}

/// Capture-path parse: rewrite `<extension>` bodies into a skeleton and strip comments into the
/// side-channel, serde-parse the skeleton, then reattach the captured bodies to the matching
/// [`Extension`] structs by synthetic id and distribute the comments onto the model. `pub(crate)` so
/// the fast-path equivalence test in [`crate::ser`] can run it on documents the fast path would skip.
pub(crate) fn parse_with_capture(s: &str) -> Result<Hcdf> {
    let raw = comments::extract_side_channels(s)?;
    let mut doc: Hcdf =
        quick_xml::de::from_str(&raw.skeleton).map_err(|e| Error::Xml(e.to_string()))?;
    for ext in collect_extensions_mut(&mut doc) {
        if let Some(id) = ext.ext_id.take() {
            ext.body = raw.ext_bodies.get(id as usize).cloned().unwrap_or_default();
        }
    }
    comments::distribute(&mut doc, raw);
    Ok(doc)
}

/// Read the root `<hcdf>` element's `@version` without a full parse (None when absent, or when the
/// root is not `<hcdf>`, serde reports that shape error itself), so the version gate can reject a
/// foreign-MAJOR document even when its body no longer matches the 1.0 model shape.
fn root_version(src: &str) -> Result<Option<String>> {
    let mut reader = Reader::from_str(src);
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) | Event::Empty(e) => {
                if e.name().as_ref() != b"hcdf" {
                    return Ok(None);
                }
                for attr in e.attributes() {
                    let attr = attr.map_err(|e| Error::Xml(e.to_string()))?;
                    if attr.key.as_ref() == b"version" {
                        let val = attr
                            .unescape_value()
                            .map_err(|e| Error::Xml(e.to_string()))?;
                        return Ok(Some(val.into_owned()));
                    }
                }
                return Ok(None);
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
    }
}

/// All `<extension>` elements in the document, in serialization (document) order. Used to reattach
/// captured bodies on read and to look up bodies on write. The order MUST match the serde field order
/// of [`Hcdf`]/[`crate::model::Comp`]/[`crate::model::Joint`] so it aligns with the serialized skeleton.
pub(crate) fn collect_extensions_mut(doc: &mut Hcdf) -> Vec<&mut Extension> {
    let mut out: Vec<&mut Extension> = Vec::new();
    for comp in doc.comp.iter_mut() {
        for ext in comp.extension.iter_mut() {
            out.push(ext);
        }
    }
    for joint in doc.joint.iter_mut() {
        if let Some(ext) = joint.extension.as_mut() {
            out.push(ext);
        }
    }
    for ext in doc.extension.iter_mut() {
        out.push(ext);
    }
    out
}

/// Read-only twin of [`collect_extensions_mut`], same order.
pub(crate) fn collect_extensions(doc: &Hcdf) -> Vec<&Extension> {
    let mut out: Vec<&Extension> = Vec::new();
    for comp in doc.comp.iter() {
        for ext in comp.extension.iter() {
            out.push(ext);
        }
    }
    for joint in doc.joint.iter() {
        if let Some(ext) = joint.extension.as_ref() {
            out.push(ext);
        }
    }
    for ext in doc.extension.iter() {
        out.push(ext);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_path_matches_capture_path() {
        // no `<extension` and no `<!--` substring -> from_xml_str takes the direct-serde fast path;
        // it must produce the same model as the capture rewrite it skipped.
        let src = r#"<hcdf name="demo" version="1.0"><comp name="base"/></hcdf>"#;
        assert!(!src.contains("<extension") && !src.contains("<!--"));
        assert_eq!(
            Hcdf::from_xml_str(src).unwrap(),
            parse_with_capture(src).unwrap()
        );
    }

    #[test]
    fn scan_false_positive_in_comment_is_harmless() {
        // the `<extension` substring inside a comment sends the doc down the capture path; the
        // parsed model must equal the comment-free twin that takes the fast path (comment carriers
        // are equality-transparent), and the comment itself now survives to the output.
        let with_comment =
            r#"<hcdf name="demo" version="1.0"><!-- <extension> --><comp name="base"/></hcdf>"#;
        let without = r#"<hcdf name="demo" version="1.0"><comp name="base"/></hcdf>"#;
        assert!(with_comment.contains("<extension") && !without.contains("<extension"));
        let doc = Hcdf::from_xml_str(with_comment).unwrap();
        assert_eq!(doc, Hcdf::from_xml_str(without).unwrap());
        assert!(doc.extension.is_empty() && doc.comp[0].extension.is_empty());
        assert!(
            doc.to_xml_string()
                .unwrap()
                .contains("<!-- <extension> -->"),
            "the comment must round-trip"
        );
    }

    #[test]
    fn retired_connectivity_shapes_fail_instead_of_disappearing() {
        let cases = [
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p" type="ethernet"/></comp></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"><pin name="1"/></port></comp></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><comp name="a"><switch name="s"><port name="p"/></switch></comp></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><network name="n"><link/></network></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><link name="n"><wired/></link></hcdf>"#,
            concat!(
                r#"<hcdf name="d" version="1.0"><link name="n"><selected purpose="communication" carrier="electrical"/>"#,
                r#"<participant name="p" component="a" port="p"><selected/></participant>"#,
                r#"</link></hcdf>"#,
            ),
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"><capabilities><rate value="1" unit="bit/s"/></capabilities></port></comp></hcdf>"#,
            concat!(
                r#"<hcdf name="d" version="1.0"><link name="n">"#,
                r#"<selected purpose="communication" carrier="electrical"><rate min="1" unit="bit/s"/></selected>"#,
                r#"</link></hcdf>"#,
            ),
            concat!(
                r#"<hcdf name="d" version="1.0"><comp name="a"><wire name="w" fidelity="exact">"#,
                r#"<first><instance/></first></wire></comp></hcdf>"#,
            ),
            concat!(
                r#"<hcdf name="d" version="1.0"><mate name="m" fidelity="exact">"#,
                r#"<first connector="J"><component-ref component="a"/>"#,
                r#"<connector-ref connector="J"><component-ref component="a"/></connector-ref>"#,
                r#"</first></mate></hcdf>"#,
            ),
        ];
        for src in cases {
            let error = Hcdf::from_xml_str(src).unwrap_err().to_string();
            assert!(
                error.contains("unknown HCDF core"),
                "unexpected strict error for {src}: {error}"
            );
        }
    }

    #[test]
    fn connectivity_character_content_is_rejected_except_for_declared_text_fields() {
        let rejected = [
            (
                r#"<hcdf name="d" version="1.0"><link name="n"><selected purpose="communication" carrier="electrical"><voltage><nominal value="24" unit="V">text</nominal></voltage></selected></link></hcdf>"#,
                "nominal",
            ),
            (
                r#"<hcdf name="d" version="1.0"><link name="n"><selected purpose="communication" carrier="electrical"><voltage><range min="20" max="28" unit="V">text</range></voltage></selected></link></hcdf>"#,
                "range",
            ),
            (
                r#"<hcdf name="d" version="1.0"><comp name="a"><termination name="R" kind="hcdf:resistor" mounting="endpoint" fidelity="exact"><quantity property="hcdf:resistance" value="120" unit="ohm">text</quantity></termination></comp></hcdf>"#,
                "quantity",
            ),
            (
                r#"<hcdf name="d" version="1.0"><link name="n"><selected purpose="communication" carrier="electrical">text<profile id="hcdf:test"/></selected></link></hcdf>"#,
                "selected",
            ),
            (
                r#"<hcdf name="d" version="1.0"><link name="n"><selected purpose="communication" carrier="electrical"><voltage><nominal value="24" unit="V"><![CDATA[text]]></nominal></voltage></selected></link></hcdf>"#,
                "nominal",
            ),
        ];
        for (source, element) in rejected {
            let error = Hcdf::from_xml_str(source).unwrap_err().to_string();
            assert!(
                error.contains("unexpected character content")
                    && error.contains(&format!("<{element}>")),
                "unexpected strict-content error for <{element}>: {error}"
            );
        }

        let description = Hcdf::from_xml_str(
            r#"<hcdf name="d" version="1.0"><link name="n"><description>allowed &amp; retained</description></link></hcdf>"#,
        )
        .unwrap();
        assert_eq!(
            description.link[0].description.as_deref(),
            Some("allowed & retained")
        );

        Hcdf::from_xml_str(
            r#"<hcdf name="d" version="1.0"><link name="n"><selected purpose="communication" carrier="electrical">
                <voltage><nominal value="24" unit="V">
                </nominal></voltage>
            </selected></link></hcdf>"#,
        )
        .unwrap();
    }

    #[test]
    fn prefixed_elements_cannot_impersonate_hcdf_core() {
        let src = concat!(
            r#"<hcdf name="d" version="1.0" xmlns:foreign="urn:foreign">"#,
            r#"<foreign:link name="n"/></hcdf>"#,
        );
        let error = Hcdf::from_xml_str(src).unwrap_err().to_string();
        assert!(error.contains("namespaced HCDF core element <link>"));
    }

    #[test]
    fn default_namespace_cannot_impersonate_hcdf_core() {
        let cases = [
            r#"<hcdf xmlns="urn:foreign" name="d" version="1.0"/>"#,
            r#"<hcdf name="d" version="1.0"><bus xmlns="urn:foreign" name="n"/></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><extension xmlns="urn:foreign" domain="x"/></hcdf>"#,
        ];
        for src in cases {
            let error = Hcdf::from_xml_str(src).unwrap_err().to_string();
            assert!(
                error.contains("namespaced HCDF core element"),
                "unexpected namespace error for {src}: {error}"
            );
        }
    }

    #[test]
    fn extension_namespace_remains_opaque() {
        let src = concat!(
            r#"<hcdf name="d" version="1.0"><extension domain="example" xmlns:foreign="urn:foreign">"#,
            r#"<foreign:link old-attribute="accepted"><foreign:wired/></foreign:link>"#,
            r#"</extension></hcdf>"#,
        );
        let doc = Hcdf::from_xml_str(src).unwrap();
        assert!(doc.extension[0].body.contains("<foreign:link"));
        assert!(doc.extension[0].body.contains("<foreign:wired/>"));
    }

    #[test]
    fn joint_extension_accepts_prefixed_and_default_namespaced_bodies() {
        let src = concat!(
            r#"<hcdf name="d" version="1.0"><joint name="hinge" type="fixed">"#,
            r#"<extension domain="example" xmlns:foreign="urn:prefixed">"#,
            r#"<foreign:bus foreign:mode="legacy"/>"#,
            r#"<payload xmlns="urn:default"><mesh name="opaque"/></payload>"#,
            r#"</extension></joint></hcdf>"#,
        );
        let doc = Hcdf::from_xml_str(src).unwrap();
        let body = &doc.joint[0].extension.as_ref().unwrap().body;
        assert!(body.contains("<foreign:bus foreign:mode=\"legacy\"/>"));
        assert!(body.contains("<payload xmlns=\"urn:default\">"));
        assert!(body.contains("<mesh name=\"opaque\"/>"));
        let out = doc.to_xml_string().unwrap();
        assert!(out.contains("<foreign:bus foreign:mode=\"legacy\"/>"));
        assert!(out.contains("<payload xmlns=\"urn:default\">"));
    }

    #[test]
    fn strong_topology_record_round_trips_description_and_selection() {
        let src = concat!(
            r#"<hcdf name="d" version="1.0"><comp name="a"><port name="p"><capabilities><rate min="1" max="2" unit="bit/s"/></capabilities></port></comp>"#,
            r#"<bus name="control"><description>actuator bus</description>"#,
            r#"<selected purpose="communication" carrier="electrical"><profile id="hcdf:rs-485"/><rate><nominal value="1" unit="bit/s"/></rate></selected>"#,
            r#"<participant name="drive" role="hcdf:node"><endpoint><port-ref component="a" port="p"/></endpoint></participant>"#,
            r#"</bus></hcdf>"#,
        );
        let doc = Hcdf::from_xml_str(src).unwrap();
        assert_eq!(doc.bus[0].description.as_deref(), Some("actuator bus"));
        assert_eq!(
            doc.bus[0].selected.as_ref().unwrap().profile[0].id,
            "hcdf:rs-485"
        );
        let out = doc.to_xml_string().unwrap();
        assert!(out.contains("<bus name=\"control\">"));
        assert!(out.contains("<description>actuator bus</description>"));
        assert!(out.contains("<profile id=\"hcdf:rs-485\"/>"));
    }

    #[test]
    fn prefix_collision_element_still_captured_verbatim() {
        // an `<extensionParams>` child shares the `<extension` prefix: the scan must send the doc
        // down the capture path and the exact end-tag match must keep the body verbatim.
        let src = concat!(
            r#"<hcdf name="demo" version="1.0"><comp name="base"><extension domain="x">"#,
            r#"<extensionParams a="1"><v>2</v></extensionParams></extension></comp></hcdf>"#
        );
        let doc = Hcdf::from_xml_str(src).unwrap();
        assert_eq!(
            doc.comp[0].extension[0].body,
            r#"<extensionParams a="1"><v>2</v></extensionParams>"#
        );
    }

    #[test]
    fn structured_tree_xml_round_trips_and_normalizes() {
        let source = concat!(
            r#"<hcdf name="d" version="1.0">"#,
            r#"<comp name="a"><port name="p"><channel name="signal" role="hcdf:data" local-group="pair-a"/></port>"#,
            r#"<connector name="J1"><pin name="1" role="hcdf:positive" local-group="pair-a"/></connector>"#,
            r#"<connector name="J2"><pin name="1"/></connector>"#,
            r#"<wire name="w" fidelity="exact" role="hcdf:differential-pair" local-group="pair-a">"#,
            r#"<first><position-ref connector="J1" position="1"><component-ref component="a"/></position-ref></first>"#,
            r#"<second><position-ref connector="J2" position="1"><component-ref component="a"/></position-ref></second></wire></comp>"#,
            r#"<comp name="b"><port name="p"><channel name="signal"/></port></comp>"#,
            r#"<tree name="control"><description>rooted path</description>"#,
            r#"<selected purpose="communication" carrier="electrical"/>"#,
            r#"<root><hop-ref network="control" hop="root-hop"/></root>"#,
            r#"<participant name="root-end" role="hcdf:controller"><endpoint><channel-ref component="a" port="p" channel="signal"/></endpoint></participant>"#,
            r#"<participant name="leaf-end"><endpoint><channel-ref component="b" port="p" channel="signal"/></endpoint></participant>"#,
            r#"<hop name="root-hop" role="hcdf:forwarder" processing-delay-ns="25"><description>root stage</description><owner><component-ref component="a"/></owner></hop>"#,
            r#"<hop name="leaf-hop"><owner><component-ref component="b"/></owner></hop>"#,
            r#"<leg name="root-leaf"><from><hop-ref network="control" hop="root-hop"/><participant-ref network="control" participant="root-end"/></from>"#,
            r#"<to><hop-ref network="control" hop="leaf-hop"/><participant-ref network="control" participant="leaf-end"/></to></leg>"#,
            r#"</tree></hcdf>"#,
        );
        let doc = Hcdf::from_xml_str(source).unwrap();
        assert_eq!(doc.tree.len(), 1);
        assert_eq!(doc.tree[0].hop.len(), 2);
        assert_eq!(doc.tree[0].leg.len(), 1);

        let output = doc.to_xml_string().unwrap();
        assert!(output.contains("<tree name=\"control\">"));
        assert!(output.contains("<root>"));
        assert!(output.contains("<hop-ref network=\"control\" hop=\"root-hop\"/>"));
        assert!(output.contains("role=\"hcdf:data\" local-group=\"pair-a\""));
        assert!(output.contains("processing-delay-ns=\"25\""));
        assert_eq!(Hcdf::from_xml_str(&output).unwrap(), doc);

        let canonical = doc
            .to_connectivity_document(
                crate::model::connectivity::DocumentIdentity::new("memory://tree.hcdf").unwrap(),
            )
            .unwrap();
        crate::connectivity::normalize_connectivity(&canonical).unwrap();
    }

    #[test]
    fn every_exact_topology_wrapper_round_trips() {
        let participants = concat!(
            r#"<participant name="a"><endpoint><port-ref component="a" port="p"/></endpoint></participant>"#,
            r#"<participant name="b"><endpoint><port-ref component="b" port="p"/></endpoint></participant>"#,
        );
        let hops = concat!(
            r#"<hop name="h0"><owner><function-ref component="a" function="bridge"/></owner></hop>"#,
            r#"<hop name="h1"><owner><component-ref component="b"/></owner></hop>"#,
        );
        let forward = concat!(
            r#"<leg name="forward"><from><hop-ref network="n" hop="h0"/><participant-ref network="n" participant="a"/></from>"#,
            r#"<to><hop-ref network="n" hop="h1"/><participant-ref network="n" participant="b"/></to></leg>"#,
        );
        let reverse = concat!(
            r#"<leg name="reverse"><from><hop-ref network="n" hop="h1"/><participant-ref network="n" participant="b"/></from>"#,
            r#"<to><hop-ref network="n" hop="h0"/><participant-ref network="n" participant="a"/></to></leg>"#,
        );
        let cases = [
            (
                "link",
                format!(
                    r#"<hcdf name="d" version="1.0"><link name="n">{participants}</link></hcdf>"#
                ),
            ),
            (
                "bus",
                format!(
                    r#"<hcdf name="d" version="1.0"><bus name="n">{participants}</bus></hcdf>"#
                ),
            ),
            (
                "mesh",
                format!(
                    r#"<hcdf name="d" version="1.0"><mesh name="n">{participants}</mesh></hcdf>"#
                ),
            ),
            (
                "star",
                format!(
                    r#"<hcdf name="d" version="1.0"><star name="n"><coordinator><participant-ref network="n" participant="a"/></coordinator>{participants}</star></hcdf>"#
                ),
            ),
            (
                "chain",
                format!(
                    r#"<hcdf name="d" version="1.0"><chain name="n">{participants}{hops}{forward}</chain></hcdf>"#
                ),
            ),
            (
                "ring",
                format!(
                    r#"<hcdf name="d" version="1.0"><ring name="n">{participants}{hops}{forward}{reverse}</ring></hcdf>"#
                ),
            ),
            (
                "tree",
                format!(
                    r#"<hcdf name="d" version="1.0"><tree name="n"><root><hop-ref network="n" hop="h0"/></root>{participants}{hops}{forward}</tree></hcdf>"#
                ),
            ),
        ];

        for (tag, source) in cases {
            let doc = Hcdf::from_xml_str(&source).unwrap();
            let output = doc.to_xml_string().unwrap();
            assert!(output.contains(&format!("<{tag} name=\"n\">")), "{output}");
            assert_eq!(Hcdf::from_xml_str(&output).unwrap(), doc);
            if tag == "star" {
                assert!(output.contains("<coordinator>"));
                assert!(output.contains("participant=\"a\""));
            }
            if matches!(tag, "chain" | "ring" | "tree") {
                assert!(output.contains("<function-ref component=\"a\" function=\"bridge\"/>"));
                assert!(output.contains("<from>"));
                assert!(output.contains("<to>"));
            }
        }
    }

    #[test]
    fn malformed_structured_topology_xml_is_rejected() {
        let cases = [
            r#"<hcdf name="d" version="1.0"><star name="n"/></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><tree name="n"/></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><chain name="n"><hop name="h"/></chain></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><chain name="n"><leg name="l"><to><hop-ref network="n" hop="h1"/><participant-ref network="n" participant="b"/></to></leg></chain></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><chain name="n"><leg name="l"><from><hop-ref network="n" hop="h0"/><participant-ref network="n" participant="a"/></from></leg></chain></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><chain name="n"><leg name="l"><from><component-ref component="a"/><participant-ref network="n" participant="a"/></from><to><hop-ref network="n" hop="h1"/><participant-ref network="n" participant="b"/></to></leg></chain></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><link name="n"><participant name="p" component="a" port="p"/></link></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><tree name="n"><branch/></tree></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><tree name="n" topology="tree"/></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><link name="n"><participant name="p"/></link></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><link name="n"><participant name="p"><endpoint><port-ref component="a" port="p"/><channel-ref component="a" port="p" channel="c"/></endpoint></participant></link></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><chain name="n"><hop name="h"><owner><component-ref component="a"/><function-ref component="a" function="f"/></owner></hop></chain></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><chain name="n"><hop name="h"><owner><participant-ref network="n" participant="p"/></owner></hop></chain></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><chain name="n"><hop name="h"><owner>text<component-ref component="a"/></owner></hop></chain></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><chain name="n"><hop name="h"><owner><![CDATA[text]]><component-ref component="a"/></owner></hop></chain></hcdf>"#,
            r#"<hcdf name="d" version="1.0"><tree name="n"><root><participant-ref network="n" participant="p"/></root></tree></hcdf>"#,
        ];
        for source in cases {
            assert!(
                Hcdf::from_xml_str(source).is_err(),
                "malformed structured connectivity was accepted: {source}"
            );
        }

        let retired = Hcdf::from_xml_str(
            r#"<hcdf name="d" version="1.0"><link name="n"><participant name="p" component="a" port="p"/></link></hcdf>"#,
        )
        .unwrap_err()
        .to_string();
        assert!(retired.contains("@component") && retired.contains("<participant>"));

        let unknown_tree = Hcdf::from_xml_str(
            r#"<hcdf name="d" version="1.0"><tree name="n"><branch/></tree></hcdf>"#,
        )
        .unwrap_err()
        .to_string();
        assert!(unknown_tree.contains("<branch>") && unknown_tree.contains("<tree>"));
    }
}
