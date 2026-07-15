//! Writing official-format HCDF (quick-xml + serde): attribute-form poses, schema element names,
//! verbatim `<extension>` body splicing, and XML comment re-injection.
//!
//! Output is PRETTY-PRINTED (quick-xml's serializer configured with a two-space indent): one element
//! per line, nested children indented, attributes on the element's own line, text-only elements
//! (`<mass>1</mass>`) kept inline, and self-closing empty elements preserved. This is the canonical
//! serialization the whole content-addressing pipeline hashes ([`crate::compose::content_sha_of_module`]
//! over these bytes), so it must be deterministic and byte-idempotent under parse→serialize.
//!
//! serde serializes the model into a skeleton where each `<extension>` is empty (its `body` is
//! `#[serde(skip)]` and `hcdf-ext-id` is `skip_serializing`) and no comment appears (the carriers are
//! `#[serde(skip)]` too). A quick-xml event pass ([`crate::comments::splice_doc`]) then walks the
//! skeleton and, in one pass, injects the stored raw inner XML at each `<extension>` element in
//! document order and re-emits every captured comment at its anchor. The splice is indent-aware (it
//! holds the serializer's whitespace and re-emits each comment on its own correctly-indented line), so
//! the pretty layout survives the injection. Documents with no extensions and no comments return the
//! skeleton directly (the splice pass would be an identity copy). See [`crate::de`] for the inverse
//! capture on read.
use crate::comments;
use crate::de::collect_extensions;
use crate::error::{Error, Result};
use crate::model::Hcdf;
use serde::Serialize;

impl Hcdf {
    /// Serialize to an official-format HCDF XML string (pretty-printed, two-space indent).
    pub fn to_xml_string(&self) -> Result<String> {
        let skeleton = self.to_skeleton_xml()?;
        let extensions = collect_extensions(self);
        // Fast path for the common extension-free, comment-free document: the skeleton contains no
        // `<extension>` element to splice into and there is no comment to re-emit, so the event
        // re-write would be a byte-identical copy (asserted by the splice_is_identity_without_
        // side_channels test), so return the skeleton as-is.
        if extensions.is_empty() && !comments::doc_has_comments(self) {
            return Ok(skeleton);
        }
        let bodies: Vec<&str> = extensions.iter().map(|e| e.body.as_str()).collect();
        comments::splice_doc(&skeleton, &bodies, self)
    }

    /// The serde SKELETON: pretty-printed (two-space indent) XML with each `<extension>` emitted empty
    /// and no comments: the exact input the side-channel splice ([`comments::splice_doc`]) walks. Kept
    /// as a named seam (not an inline `quick_xml::se::to_string`) so the fast-path equivalence tests
    /// compare against the identical serializer configuration `to_xml_string` uses, and the compact
    /// serializer is never a stand-in for it.
    pub(crate) fn to_skeleton_xml(&self) -> Result<String> {
        let mut buf = String::new();
        let mut ser = quick_xml::se::Serializer::new(&mut buf);
        ser.indent(' ', 2);
        self.serialize(ser).map_err(|e| Error::Xml(e.to_string()))?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn output_is_pretty_printed_and_stable() {
        // A multi-comp document with nested subsystems, a text-only leaf, a self-closing empty comp,
        // and a joint: the pretty printer must lay it out one element per line at a two-space step,
        // keep the text leaf inline, preserve the self-closing form, reparse identically, and be a
        // byte fixpoint under a second serialize. The canonical content sha (which the include-@sha
        // stamp/verify pipeline hashes) must be stable across that pretty round-trip.
        let src = concat!(
            r#"<hcdf name="robot" version="1.0">"#,
            r#"<comp name="base" role="parent">"#,
            r#"<inertial><mass>1.5</mass></inertial>"#,
            r#"<visual name="v"><geometry><box size="1 1 1"/></geometry></visual>"#,
            r#"</comp><comp name="two"/>"#,
            r#"<joint name="j" type="fixed"><parent comp="base"/><child comp="two"/></joint>"#,
            r#"</hcdf>"#,
        );
        let doc = Hcdf::from_xml_str(src).unwrap();
        let out = doc.to_xml_string().unwrap();

        // Nested indentation: top-level comps at one step, subsystems at two, geometry at three,
        // each on its own line (a compact serializer would jam them, so this pins the layout).
        assert!(
            out.contains("\n  <comp name=\"base\" role=\"parent\">"),
            "{out}"
        );
        assert!(out.contains("\n    <inertial>"), "{out}");
        assert!(out.contains("\n      <geometry>"), "{out}");
        // Attributes ride the element line; a text-only leaf stays inline; the empty comp keeps its
        // self-closing form (indented, not expanded).
        assert!(
            out.contains("<mass>1.5</mass>"),
            "text leaf must stay inline: {out}"
        );
        assert!(
            out.contains("\n  <comp name=\"two\"/>"),
            "self-closing preserved: {out}"
        );
        // One element per line: line breaks follow tags and no two tags are jammed adjacent.
        assert!(
            out.contains(">\n"),
            "expected line breaks after tags: {out}"
        );
        assert!(!out.contains("><"), "no two tags may share a line: {out}");

        // Reparse is identical and a second serialize is a byte fixpoint (idempotent pretty output).
        let doc2 = Hcdf::from_xml_str(&out).unwrap();
        assert_eq!(doc, doc2, "pretty output did not reparse to an equal model");
        assert_eq!(
            out,
            doc2.to_xml_string().unwrap(),
            "pretty output is not byte-idempotent"
        );

        // The canonical module sha (the value the keep-live pack stamps and BundleVerify checks) is
        // computed over these exact pretty bytes and must survive the parse->serialize round-trip, so
        // a stamp verifies against itself.
        let stamp = crate::compose::content_sha_of_module(&doc).unwrap();
        assert_eq!(stamp, crate::compose::content_sha(out.as_bytes()));
        assert_eq!(stamp, crate::compose::content_sha_of_module(&doc2).unwrap());
    }

    #[test]
    fn splice_is_identity_without_side_channels() {
        // the fast path returns the serde skeleton directly; the splice pass it skips must be a
        // byte-identical copy (this is the invariant the to_xml_string fast path relies on).
        let doc =
            Hcdf::from_xml_str(r#"<hcdf name="demo" version="1.0"><comp name="base"/></hcdf>"#)
                .unwrap();
        let skeleton = doc.to_skeleton_xml().unwrap();
        assert_eq!(doc.to_xml_string().unwrap(), skeleton);
        assert_eq!(
            comments::splice_doc(&skeleton, &[], &doc).unwrap(),
            skeleton
        );
    }

    /// Every first-party `.hcdf` on disk: the canonical `examples/` corpus (the files
    /// `tests/examples.rs` gates on) plus the XSD fixture documents (ALL of which carry comments)
    /// plus one inline comment-free minimal document so the fast paths stay exercised. Disk entries
    /// are empty when run from a packaged crate tarball (both directories sit outside the packaged
    /// sources).
    fn corpus() -> Vec<(String, String)> {
        let mut out = vec![(
            "inline-minimal".to_string(),
            r#"<hcdf name="demo" version="1.0"><comp name="base"/></hcdf>"#.to_string(),
        )];
        for dir in [
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/xsd_fixtures"),
        ] {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.expect("dir entry").path();
                if path.extension().and_then(|e| e.to_str()) == Some("hcdf") {
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    out.push((name, std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        out
    }

    #[test]
    fn fast_paths_match_full_passes_on_corpus() {
        // parse: from_xml_str (fast path when no `<extension` and no `<!--`) must equal the capture
        // rewrite; serialize: to_xml_string (fast path when no extensions and no comments) must be
        // BYTE-identical to the splice pass, and serialize->parse->serialize must be a byte fixpoint.
        // The corpus must keep at least one document on each side of the scan.
        let (mut fast, mut slow) = (0u32, 0u32);
        for (name, xml) in corpus() {
            let doc =
                Hcdf::from_xml_str(&xml).unwrap_or_else(|e| panic!("{name}: parse failed: {e}"));
            let captured = crate::de::parse_with_capture(&xml)
                .unwrap_or_else(|e| panic!("{name}: capture-path parse failed: {e}"));
            assert_eq!(
                doc, captured,
                "{name}: fast-path parse diverges from the capture pass"
            );

            let out = doc.to_xml_string().unwrap();
            let skeleton = doc.to_skeleton_xml().unwrap();
            let bodies: Vec<&str> = collect_extensions(&doc)
                .iter()
                .map(|e| e.body.as_str())
                .collect();
            let spliced = comments::splice_doc(&skeleton, &bodies, &doc).unwrap();
            assert_eq!(
                out, spliced,
                "{name}: fast-path serialize diverges from the splice pass"
            );

            let out2 = Hcdf::from_xml_str(&out)
                .unwrap_or_else(|e| panic!("{name}: re-parse failed: {e}"))
                .to_xml_string()
                .unwrap();
            assert_eq!(
                out, out2,
                "{name}: serialize/parse/serialize is not byte-idempotent"
            );

            if bodies.is_empty() && !comments::doc_has_comments(&doc) {
                fast += 1;
            } else {
                slow += 1;
            }
        }
        if fast + slow > 1 {
            assert!(
                fast >= 1 && slow >= 1,
                "corpus no longer exercises both paths (fast={fast}, slow={slow})"
            );
        } else {
            // only the inline fast-path entry ran (disk corpus not reachable from a packaged crate)
            eprintln!("skipping: disk corpus not reachable (packaged crate)");
        }
    }
}
