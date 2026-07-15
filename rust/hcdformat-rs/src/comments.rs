//! XML comment preservation through the HCDF round-trip (a side-channel, like `<extension>` bodies).
//!
//! serde skips `Event::Comment`, so a naive parse/serialize drops every `<!-- -->` in a document,
//! including a commented-out `<visual>` block an editor user expects to get back. This module captures
//! comments during the same event pass that extracts `<extension>` bodies (see [`crate::de`]) and
//! re-injects them during the splice pass on write (see [`crate::ser`]). Nothing here touches the XML
//! schema: the carrier fields ([`DocComments`] on `Hcdf`, [`CommentSet`] on `Comp`/`Joint`) are
//! `#[serde(skip)]`, so no new element or attribute is ever serialized.
//!
//! ## Anchoring model
//! A captured comment records the path to its PARENT element plus a [`CommentAnchor`]: either "before
//! the k-th `<name>` element child" or "at the end of the parent's content". Path steps are
//! `(element-name, occurrence-among-same-name-siblings)` rather than raw child indexes: serde
//! re-serializes children in struct-field order, which can permute DIFFERENTLY-named siblings of a
//! hand-authored document, but never reorders same-name siblings (they live in one `Vec`), so a
//! `(name, occurrence)` step survives re-serialization where a raw index would not. A comment anchors
//! to the element that FOLLOWS it (banner comments and commented-out blocks precede what they
//! describe/replaced); trailing comments anchor [`CommentAnchor::AtEnd`].
//!
//! Comments captured under a top-level `<comp>`/`<joint>` subtree are stored ON that struct with
//! comp-/joint-relative paths, so they travel through slot replacement, rename, and include-flatten
//! automatically. Everything else stays on `Hcdf::comments` with an `<hcdf>`-relative path.
//!
//! ## Deliberately NOT preserved
//! 1. Comment POSITION inside mixed/text content (`<description>a<!--c-->b</description>`): the
//!    comment is retained but repositions to the parent's element-content boundary (`AtEnd`), and the
//!    surrounding text parses as one node (`ab`).
//! 2. Comments before/after the root element of a FRAGMENT ([`extract_fragment_comments`] counts them
//!    in [`FragmentComments::outside`] and drops them). Whole-document head/tail comments ARE kept.
//! 3. Whitespace around comments: compact document output has none; indented fragments get standard
//!    two-space placement (the fragment serializer's configured indent step).
//! 4. The exact anchor when the following element vanishes (an unknown element dropped by serde, or an
//!    element deleted by an edit): the comment's content is preserved but flushes to end-of-parent. A
//!    comment INSIDE an element that is dropped/deleted dies with it (its content is gone too).
//! 5. Doc-level anchors can drift one slot when top-level children are added/removed/reordered by
//!    editor operations: degraded per (4), never lost, never an error.
//! 6. Processing instructions, DOCTYPE, and the XML declaration (unchanged behavior: dropped).
//!
//! Comments are also NOT carried across format conversion (`to_urdf`/`to_sdf`/`from_*`), and the Python
//! `hcdfdom` does not preserve them either (Rust is the canonical implementation; this is a deliberate
//! parity divergence). Comments inside `<extension>` bodies are preserved by the existing VERBATIM body
//! channel and never reach the comment collector, so there is no double handling.
//!
//! Include/compose interplay (see [`crate::compose`]): comp-/joint-attached comments ride the struct
//! moves of `merge`; module document comments are rebased onto the merged top-level element occurrences
//! so comments under connectivity assemblies, bindings, mates, and networks remain attached. Comments
//! are part of a module's canonical bytes, so they flow
//! into [`crate::compose::content_sha_of_module`]: a comment edit is a content change and re-pins.
use crate::error::{Error, Result};
use crate::model::Hcdf;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use std::collections::HashMap;
use std::io::Cursor;

/// One `(element-name, occurrence-among-same-name-siblings)` step of a comment's parent path.
pub type PathStep = (String, usize);

/// Where a captured comment re-attaches inside its parent's content.
#[derive(Debug, Clone, PartialEq)]
pub enum CommentAnchor {
    /// Immediately before the `occurrence`-th (0-based) `<name>` ELEMENT child of the parent.
    BeforeElement { name: String, occurrence: usize },
    /// After the last element child (also: the only content of an element with no element children).
    AtEnd,
}

/// One captured `<!-- -->` with enough addressing to re-attach it on write.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedComment {
    /// Path steps from the capture ROOT's content down to the PARENT element holding the comment.
    /// Empty = the comment sits directly in the capture root's content.
    pub path: Vec<PathStep>,
    pub anchor: CommentAnchor,
    /// Verbatim bytes between `<!--` and `-->` (NOT unescaped; re-emitted raw).
    pub text: String,
}

/// Comment set attached to a `Comp`/`Joint`.
///
/// EQUALITY-TRANSPARENT: `PartialEq` always returns `true`, so comments never participate in model
/// equality. They are a preservation side-channel, not modeled content; this keeps every existing
/// `assert_eq!(doc, reparsed)` round-trip gate meaning exactly what it meant, and keeps
/// `parse(serialize(d)) == d` exact even where an anchor legitimately degrades (a dropped anchor
/// re-captures as `AtEnd`: same bytes, different anchor value). Preservation itself is asserted at the
/// BYTE level by the round-trip tests.
#[derive(Debug, Clone, Default)]
pub struct CommentSet(pub Vec<CapturedComment>);

impl PartialEq for CommentSet {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl CommentSet {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Doc-level comments on `Hcdf`. Equality-transparent like [`CommentSet`] (same rationale).
#[derive(Debug, Clone, Default)]
pub struct DocComments {
    /// Before the root element (license banners; after any XML declaration). Verbatim, in order.
    pub head: Vec<String>,
    /// Inside the root element's content, paths relative to the root. Includes comments under
    /// top-level containers other than comp/joint (group/network/…) with their full path.
    pub body: Vec<CapturedComment>,
    /// After the root element's end tag.
    pub tail: Vec<String>,
}

impl PartialEq for DocComments {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl DocComments {
    pub fn is_empty(&self) -> bool {
        self.head.is_empty() && self.body.is_empty() && self.tail.is_empty()
    }
}

/// `true` when the document carries ANY captured comment (doc-level or attached to a comp/joint):
/// the write-side fast-path gate ([`crate::ser`] returns the serde skeleton untouched when this and
/// the extension list are both empty).
pub(crate) fn doc_has_comments(doc: &Hcdf) -> bool {
    !doc.comments.is_empty()
        || doc.comp.iter().any(|c| !c.comments.is_empty())
        || doc.joint.iter().any(|j| !j.comments.is_empty())
}

// ── capture (read side) ─────────────────────────────────────────────────────────────────────────

/// How the capture pass treats `<extension>` elements.
enum ExtChannel {
    /// Whole-document mode: rewrite each `<extension>` into an empty skeleton element carrying a
    /// synthetic `@hcdf-ext-id` and record the inner XML by id (the pre-existing verbatim body
    /// channel). Interior events, INCLUDING comments, go to the body, never to the collector.
    Capture,
    /// Fragment mode: copy the `<extension>` subtree through verbatim (no skeleton rewrite, since
    /// nothing reattaches bodies on a plain fragment parse); interior comments stay in place.
    Verbatim,
}

/// Everything one capture pass extracts: the serde-ready skeleton (comments stripped, extensions
/// rewritten per [`ExtChannel`]) plus the two side-channels.
pub(crate) struct RawCapture {
    pub skeleton: String,
    /// Captured `<extension>` bodies indexed by synthetic id (empty in fragment mode).
    pub ext_bodies: Vec<String>,
    /// Comments before the root element, in order.
    pub head: Vec<String>,
    /// Comments inside the root element, paths relative to the root's content.
    pub inner: Vec<CapturedComment>,
    /// Comments after the root element's end tag, in order.
    pub tail: Vec<String>,
}

/// Comments captured from a single-root FRAGMENT (e.g. one `<comp>` in an XML editor buffer).
#[derive(Debug)]
pub struct FragmentComments {
    /// The fragment with comments removed: feed THIS to serde so mixed-text behavior is identical
    /// to the whole-document path.
    pub stripped: String,
    /// Comments inside the root element, paths relative to its content.
    pub inner: Vec<CapturedComment>,
    /// How many comments sat before/after the root element: the fragment channel deliberately does
    /// NOT preserve those (a caller may surface the count).
    pub outside: usize,
}

/// Whole-document capture: extension bodies (verbatim channel) + comments, one event pass.
pub(crate) fn extract_side_channels(src: &str) -> Result<RawCapture> {
    capture(src, ExtChannel::Capture)
}

/// Capture comments INSIDE a fragment's single root element (paths relative to its content) and
/// return the comment-stripped fragment. `<extension>` subtrees pass through verbatim, so their
/// interior comments stay on the body channel exactly like the whole-document path.
pub fn extract_fragment_comments(src: &str) -> Result<FragmentComments> {
    let raw = capture(src, ExtChannel::Verbatim)?;
    Ok(FragmentComments {
        stripped: raw.skeleton,
        inner: raw.inner,
        outside: raw.head.len() + raw.tail.len(),
    })
}

/// Reattach a whole-document capture's comments to the parsed model: comments under the i-th
/// top-level `<comp>`/`<joint>` move ONTO that struct with the leading step stripped (the occurrence
/// index of a top-level comp/joint equals its `Vec` index; serde fills the `Vec` in document order,
/// correct even for duplicate names); everything else stays doc-level.
pub(crate) fn distribute(doc: &mut Hcdf, raw: RawCapture) {
    for c in raw.inner {
        match c.path.first() {
            Some((name, i)) if name == "comp" && *i < doc.comp.len() => {
                let (idx, rest) = (*i, c.path[1..].to_vec());
                doc.comp[idx]
                    .comments
                    .0
                    .push(CapturedComment { path: rest, ..c });
            }
            Some((name, i)) if name == "joint" && *i < doc.joint.len() => {
                let (idx, rest) = (*i, c.path[1..].to_vec());
                doc.joint[idx]
                    .comments
                    .0
                    .push(CapturedComment { path: rest, ..c });
            }
            _ => doc.comments.body.push(c),
        }
    }
    doc.comments.head = raw.head;
    doc.comments.tail = raw.tail;
}

/// Per-element capture state: per-name child counters (occurrence steps) and comments awaiting an
/// anchor (flushed `BeforeElement` at the next element child, `AtEnd` at the end tag).
#[derive(Default)]
struct Frame {
    counters: HashMap<String, usize>,
    pending: Vec<String>,
}

/// The capture walker. Bundles the writer + frame stack so the per-event handlers stay within the
/// argument budget.
struct Capturer {
    writer: Writer<Cursor<Vec<u8>>>,
    /// One frame per OPEN element (`frames[0]` = the root); empty at document level.
    frames: Vec<Frame>,
    /// Path of the current innermost element relative to the root's content
    /// (`path.len() == frames.len() - 1` while inside the root).
    path: Vec<PathStep>,
    /// Document-level comments awaiting classification (head before the root, tail at EOF).
    doc_pending: Vec<String>,
    ext: ExtChannel,
    out: RawCapture,
}

fn capture(src: &str, ext: ExtChannel) -> Result<RawCapture> {
    let mut reader = Reader::from_str(src);
    let mut cap = Capturer {
        writer: Writer::new(Cursor::new(Vec::new())),
        frames: Vec::new(),
        path: Vec::new(),
        doc_pending: Vec::new(),
        ext,
        out: RawCapture {
            skeleton: String::new(),
            ext_bodies: Vec::new(),
            head: Vec::new(),
            inner: Vec::new(),
            tail: Vec::new(),
        },
    };
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) => cap.on_start(&mut reader, e)?,
            Event::Empty(e) => cap.on_empty(e)?,
            Event::End(e) => cap.on_end(e)?,
            Event::Comment(e) => cap.on_comment(e)?,
            Event::Eof => break,
            ev => cap.write(ev)?,
        }
    }
    cap.out.tail = cap.doc_pending;
    cap.out.skeleton = String::from_utf8(cap.writer.into_inner().into_inner())
        .map_err(|e| Error::Xml(e.to_string()))?;
    Ok(cap.out)
}

impl Capturer {
    fn write(&mut self, ev: Event<'_>) -> Result<()> {
        self.writer
            .write_event(ev)
            .map_err(|e| Error::Xml(e.to_string()))
    }

    /// Count `name` as the next element child of the current frame and flush the frame's pending
    /// comments as `BeforeElement{name, occurrence}` anchors (a comment attaches to what follows it).
    fn step(&mut self, name: &str) -> usize {
        let frame = self
            .frames
            .last_mut()
            .expect("step() is only called inside the root");
        let slot = frame.counters.entry(name.to_string()).or_insert(0);
        let occurrence = *slot;
        *slot += 1;
        let pending = std::mem::take(&mut frame.pending);
        for text in pending {
            self.out.inner.push(CapturedComment {
                path: self.path.clone(),
                anchor: CommentAnchor::BeforeElement {
                    name: name.to_string(),
                    occurrence,
                },
                text,
            });
        }
        occurrence
    }

    fn on_start(&mut self, reader: &mut Reader<&[u8]>, e: BytesStart<'_>) -> Result<()> {
        let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
        if self.frames.is_empty() {
            // Document level: this Start opens the ROOT; pending comments become the head.
            self.out.head.append(&mut self.doc_pending);
            self.frames.push(Frame::default());
            return self.write(Event::Start(e.into_owned()));
        }
        let occurrence = self.step(&name);
        if e.name().as_ref() == b"extension" {
            // The <extension> element itself is an anchor target and counts as an element child on
            // both sides (step() above); its INTERIOR is consumed here, so interior comments go to
            // the verbatim body channel and never reach the collector, so there is no double handling.
            let inner = capture_inner(reader, b"extension")?;
            match self.ext {
                ExtChannel::Capture => {
                    let id = self.out.ext_bodies.len() as u32;
                    self.out.ext_bodies.push(inner);
                    let mut start = e.into_owned();
                    start.push_attribute(("hcdf-ext-id", id.to_string().as_str()));
                    self.write(Event::Start(start))?;
                }
                ExtChannel::Verbatim => {
                    self.write(Event::Start(e.into_owned()))?;
                    write_raw(&mut self.writer, &inner)?;
                }
            }
            return self.write(Event::End(BytesEnd::new("extension")));
        }
        self.frames.push(Frame::default());
        self.path.push((name, occurrence));
        self.write(Event::Start(e.into_owned()))
    }

    fn on_empty(&mut self, e: BytesStart<'_>) -> Result<()> {
        let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
        if self.frames.is_empty() {
            // An empty ROOT element: no content, so pending comments are the head and anything
            // after it becomes the tail.
            self.out.head.append(&mut self.doc_pending);
            return self.write(Event::Empty(e.into_owned()));
        }
        self.step(&name);
        if e.name().as_ref() == b"extension" {
            if let ExtChannel::Capture = self.ext {
                let id = self.out.ext_bodies.len() as u32;
                self.out.ext_bodies.push(String::new());
                let mut start = e.into_owned();
                start.push_attribute(("hcdf-ext-id", id.to_string().as_str()));
                return self.write(Event::Empty(start));
            }
        }
        self.write(Event::Empty(e.into_owned()))
    }

    fn on_end(&mut self, e: BytesEnd<'_>) -> Result<()> {
        let Some(frame) = self.frames.pop() else {
            return self.write(Event::End(e.into_owned()));
        };
        for text in frame.pending {
            self.out.inner.push(CapturedComment {
                path: self.path.clone(),
                anchor: CommentAnchor::AtEnd,
                text,
            });
        }
        self.path.pop();
        self.write(Event::End(e.into_owned()))
    }

    fn on_comment(&mut self, e: BytesText<'_>) -> Result<()> {
        // Verbatim bytes between `<!--` and `-->`: no unescape, no skeleton write (serde would skip
        // it anyway; stripping keeps the skeleton canonical and mixed-text behavior single-pathed).
        let text = String::from_utf8(e.into_inner().into_owned())
            .map_err(|e| Error::Xml(e.to_string()))?;
        match self.frames.last_mut() {
            Some(frame) => frame.pending.push(text),
            None => self.doc_pending.push(text),
        }
        Ok(())
    }
}

/// Capture the inner XML of the currently-open element until its matching end tag (exclusive).
pub(crate) fn capture_inner(reader: &mut Reader<&[u8]>, end_name: &[u8]) -> Result<String> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut depth = 0usize;
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) => {
                depth += 1;
                writer
                    .write_event(Event::Start(e.into_owned()))
                    .map_err(|e| Error::Xml(e.to_string()))?;
            }
            Event::End(e) => {
                if depth == 0 && e.name().as_ref() == end_name {
                    break;
                }
                depth = depth.saturating_sub(1);
                writer
                    .write_event(Event::End(e.into_owned()))
                    .map_err(|e| Error::Xml(e.to_string()))?;
            }
            Event::Eof => break,
            ev => writer
                .write_event(ev.into_owned())
                .map_err(|e| Error::Xml(e.to_string()))?,
        }
    }
    String::from_utf8(writer.into_inner().into_inner()).map_err(|e| Error::Xml(e.to_string()))
}

/// Re-emit a raw XML fragment through the writer (parse its events and forward them unchanged,
/// including comments, which is what keeps extension bodies verbatim).
pub(crate) fn write_raw(writer: &mut Writer<Cursor<Vec<u8>>>, raw: &str) -> Result<()> {
    if raw.is_empty() {
        return Ok(());
    }
    let mut reader = Reader::from_str(raw);
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Eof => break,
            ev => writer
                .write_event(ev.into_owned())
                .map_err(|e| Error::Xml(e.to_string()))?,
        }
    }
    Ok(())
}

// ── splice (write side) ─────────────────────────────────────────────────────────────────────────

/// A comment plus its ABSOLUTE parent path in the document being spliced (comp-/joint-attached
/// comments get their `("comp", i)`/`("joint", i)` prefix restored here).
pub(crate) struct PlacedComment<'a> {
    pub parent: Vec<PathStep>,
    pub comment: &'a CapturedComment,
}

/// Everything one splice pass injects into a serialized skeleton/fragment.
pub(crate) struct SpliceChannels<'a> {
    /// `Some` = whole-document mode: inject each body at the matching `<extension>` element in
    /// document order. `None` = fragment mode: `<extension>` subtrees pass through verbatim (their
    /// interiors invisible to the occurrence counters, mirroring the capture side).
    pub ext_bodies: Option<&'a [&'a str]>,
    pub head: &'a [String],
    pub comments: Vec<PlacedComment<'a>>,
    pub tail: &'a [String],
}

/// Splice a whole document's side-channels (extension bodies + comments) into its serde skeleton.
pub(crate) fn splice_doc(skeleton: &str, ext_bodies: &[&str], doc: &Hcdf) -> Result<String> {
    let mut comments: Vec<PlacedComment<'_>> = Vec::new();
    for c in &doc.comments.body {
        comments.push(PlacedComment {
            parent: c.path.clone(),
            comment: c,
        });
    }
    for (kind, sets) in [
        (
            "comp",
            doc.comp.iter().map(|c| &c.comments).collect::<Vec<_>>(),
        ),
        (
            "joint",
            doc.joint.iter().map(|j| &j.comments).collect::<Vec<_>>(),
        ),
    ] {
        for (i, set) in sets.into_iter().enumerate() {
            for c in &set.0 {
                let mut parent = Vec::with_capacity(c.path.len() + 1);
                parent.push((kind.to_string(), i));
                parent.extend(c.path.iter().cloned());
                comments.push(PlacedComment { parent, comment: c });
            }
        }
    }
    splice(
        skeleton,
        SpliceChannels {
            ext_bodies: Some(ext_bodies),
            head: &doc.comments.head,
            comments,
            tail: &doc.comments.tail,
        },
    )
}

/// Splice `comments` (paths relative to the fragment root's content) into a serialized single-root
/// fragment. Indent-aware: in an indented fragment each comment lands on its own line at the anchor's
/// indent (two-space step for end-of-content placement); in a compact fragment it is emitted flush at
/// the boundary.
pub fn splice_fragment_comments(
    fragment_xml: &str,
    comments: &[CapturedComment],
) -> Result<String> {
    let placed = comments
        .iter()
        .map(|c| PlacedComment {
            parent: c.path.clone(),
            comment: c,
        })
        .collect();
    splice(
        fragment_xml,
        SpliceChannels {
            ext_bodies: None,
            head: &[],
            comments: placed,
            tail: &[],
        },
    )
}

/// One comment awaiting splice under a fixed parent path, in stored (document) order.
struct Entry<'a> {
    anchor: &'a CommentAnchor,
    text: &'a str,
    used: bool,
}

/// The indent step appended to a held sibling indent when a comment is placed at end-of-content in
/// an INDENTED fragment (the fragment serializer's configured two-space step). Compact output holds
/// no whitespace, so this never appears there.
const INDENT_UNIT: &str = "  ";

/// The splice walker: the same frame-stack/per-name counters as the capture side, walked over the
/// serialized skeleton, plus a hold-back slot for whitespace-only text so injected comments land on
/// their own correctly-indented line in indented fragments (one mode-agnostic code path).
struct Splicer<'a> {
    writer: Writer<Cursor<Vec<u8>>>,
    /// Per-open-element per-name child counters (`frames[0]` = the root).
    frames: Vec<HashMap<String, usize>>,
    /// Path of the current innermost element relative to the root's content.
    path: Vec<PathStep>,
    by_parent: HashMap<Vec<PathStep>, Vec<Entry<'a>>>,
    /// A held whitespace-only text event (the serializer's `\n` + indent), written just before the
    /// next structural event, with comment copies of it interleaved when splicing.
    held_ws: Option<String>,
    ext_bodies: Option<&'a [&'a str]>,
    ext_idx: usize,
    head: &'a [String],
    tail: &'a [String],
}

fn splice(target: &str, ch: SpliceChannels<'_>) -> Result<String> {
    let mut by_parent: HashMap<Vec<PathStep>, Vec<Entry<'_>>> = HashMap::new();
    for p in ch.comments {
        by_parent.entry(p.parent).or_default().push(Entry {
            anchor: &p.comment.anchor,
            text: &p.comment.text,
            used: false,
        });
    }
    let mut reader = Reader::from_str(target);
    let mut sp = Splicer {
        writer: Writer::new(Cursor::new(Vec::new())),
        frames: Vec::new(),
        path: Vec::new(),
        by_parent,
        held_ws: None,
        ext_bodies: ch.ext_bodies,
        ext_idx: 0,
        head: ch.head,
        tail: ch.tail,
    };
    loop {
        match reader.read_event().map_err(|e| Error::Xml(e.to_string()))? {
            Event::Start(e) => sp.on_start(&mut reader, e)?,
            Event::Empty(e) => sp.on_empty(e)?,
            Event::End(e) => sp.on_end(e)?,
            Event::Text(t) => sp.on_text(t)?,
            Event::Eof => {
                sp.flush_ws()?;
                let tail = sp.tail;
                for text in tail {
                    sp.emit_comment(text)?;
                }
                break;
            }
            ev => {
                sp.flush_ws()?;
                sp.write(ev)?;
            }
        }
    }
    String::from_utf8(sp.writer.into_inner().into_inner()).map_err(|e| Error::Xml(e.to_string()))
}

impl<'a> Splicer<'a> {
    fn write(&mut self, ev: Event<'_>) -> Result<()> {
        self.writer
            .write_event(ev)
            .map_err(|e| Error::Xml(e.to_string()))
    }

    fn write_text(&mut self, s: String) -> Result<()> {
        self.write(Event::Text(BytesText::from_escaped(s)))
    }

    fn emit_comment(&mut self, text: &str) -> Result<()> {
        // Verbatim re-emission of the captured bytes: `from_escaped` writes them unchanged.
        self.write(Event::Comment(BytesText::from_escaped(text.to_string())))
    }

    fn flush_ws(&mut self) -> Result<()> {
        if let Some(ws) = self.held_ws.take() {
            self.write_text(ws)?;
        }
        Ok(())
    }

    /// Count `name` as the next element child of the current frame (write-side twin of the capture
    /// `step`; the two agree because extension interiors are invisible to both).
    fn step(&mut self, name: &str) -> usize {
        let frame = self
            .frames
            .last_mut()
            .expect("step() is only called inside the root");
        let slot = frame.entry(name.to_string()).or_insert(0);
        let occurrence = *slot;
        *slot += 1;
        occurrence
    }

    /// Unused comments anchored `BeforeElement{name, occurrence}` under the current path, in stored
    /// order; marks them used.
    fn take_before(&mut self, name: &str, occurrence: usize) -> Vec<&'a str> {
        let mut out = Vec::new();
        if let Some(entries) = self.by_parent.get_mut(self.path.as_slice()) {
            for en in entries.iter_mut().filter(|en| !en.used) {
                if let CommentAnchor::BeforeElement {
                    name: n,
                    occurrence: o,
                } = en.anchor
                {
                    if n == name && *o == occurrence {
                        en.used = true;
                        out.push(en.text);
                    }
                }
            }
        }
        out
    }

    /// ALL unused comments under `path` (both `AtEnd` and any `BeforeElement` whose anchor element
    /// vanished, e.g. serde dropped an unknown element, or an edit deleted the anchor), in stored
    /// order; marks them used. Content is preserved; position degrades to end-of-parent.
    fn take_all_unused(&mut self, path: &[PathStep]) -> Vec<&'a str> {
        let mut out = Vec::new();
        if let Some(entries) = self.by_parent.get_mut(path) {
            for en in entries.iter_mut().filter(|en| !en.used) {
                en.used = true;
                out.push(en.text);
            }
        }
        out
    }

    /// Emit comments before an element start: each on its own copy of the held sibling indent (the
    /// element's own write then flushes the held whitespace, so every comment lands on its own line
    /// at the sibling's indent). Compact target: emitted flush at the boundary.
    fn emit_before(&mut self, texts: &[&str]) -> Result<()> {
        let ws = self.held_ws.clone();
        for text in texts {
            if let Some(w) = &ws {
                self.write_text(w.clone())?;
            }
            self.emit_comment(text)?;
        }
        Ok(())
    }

    /// Emit end-of-content comments (before an end tag or inside an expanded empty element): one
    /// indent step DEEPER than the held closing indent, which is the children's depth.
    fn emit_at_end(&mut self, texts: &[&str], ws: &Option<String>) -> Result<()> {
        for text in texts {
            if let Some(w) = ws {
                self.write_text(format!("{w}{INDENT_UNIT}"))?;
            }
            self.emit_comment(text)?;
        }
        Ok(())
    }

    fn on_start(&mut self, reader: &mut Reader<&[u8]>, e: BytesStart<'_>) -> Result<()> {
        let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
        if self.frames.is_empty() {
            let head = self.head;
            for text in head {
                self.emit_comment(text)?;
            }
            self.flush_ws()?;
            self.frames.push(HashMap::new());
            return self.write(Event::Start(e.into_owned()));
        }
        let occurrence = self.step(&name);
        let before = self.take_before(&name, occurrence);
        self.emit_before(&before)?;
        if e.name().as_ref() == b"extension" {
            self.flush_ws()?;
            match self.ext_bodies {
                Some(bodies) => {
                    // Skeleton form `<extension ...></extension>`: inject the body after the start
                    // tag; the skeleton's own End event closes it (popping the frame pushed here).
                    let body = bodies.get(self.ext_idx).copied().unwrap_or("");
                    self.ext_idx += 1;
                    self.write(Event::Start(e.into_owned()))?;
                    write_raw(&mut self.writer, body)?;
                    self.frames.push(HashMap::new());
                    self.path.push((name, occurrence));
                    return Ok(());
                }
                None => {
                    // Fragment mode: the subtree passes through verbatim, invisible to the counters.
                    let inner = capture_inner(reader, b"extension")?;
                    self.write(Event::Start(e.into_owned()))?;
                    write_raw(&mut self.writer, &inner)?;
                    return self.write(Event::End(BytesEnd::new("extension")));
                }
            }
        }
        self.frames.push(HashMap::new());
        self.path.push((name, occurrence));
        self.flush_ws()?;
        self.write(Event::Start(e.into_owned()))
    }

    fn on_empty(&mut self, e: BytesStart<'_>) -> Result<()> {
        let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
        if self.frames.is_empty() {
            // An empty ROOT element: expand it when root-content comments exist so they are never
            // lost (a captured doc gets here only when every child element was dropped/absent).
            let head = self.head;
            for text in head {
                self.emit_comment(text)?;
            }
            let inside = self.take_all_unused(&[]);
            self.flush_ws()?;
            if inside.is_empty() {
                return self.write(Event::Empty(e.into_owned()));
            }
            self.write(Event::Start(e.clone().into_owned()))?;
            self.emit_at_end(&inside, &None)?;
            return self.write(Event::End(BytesEnd::new(name)));
        }
        let occurrence = self.step(&name);
        let before = self.take_before(&name, occurrence);
        self.emit_before(&before)?;
        if e.name().as_ref() == b"extension" {
            if let Some(bodies) = self.ext_bodies {
                // Empty skeleton form `<extension .../>`: expand to Start + body + End.
                let body = bodies.get(self.ext_idx).copied().unwrap_or("");
                self.ext_idx += 1;
                self.flush_ws()?;
                self.write(Event::Start(e.into_owned()))?;
                write_raw(&mut self.writer, body)?;
                return self.write(Event::End(BytesEnd::new("extension")));
            }
        }
        let mut child_path = self.path.clone();
        child_path.push((name.clone(), occurrence));
        let inside = self.take_all_unused(&child_path);
        if inside.is_empty() {
            self.flush_ws()?;
            return self.write(Event::Empty(e.into_owned()));
        }
        // The empty element holds comments: expand `<x/>` to `<x>…</x>` (same trick as the
        // extension arm above) so they have content to live in.
        let ws = self.held_ws.clone();
        self.flush_ws()?;
        self.write(Event::Start(e.clone().into_owned()))?;
        self.emit_at_end(&inside, &ws)?;
        if let Some(w) = &ws {
            self.write_text(w.clone())?;
        }
        self.write(Event::End(BytesEnd::new(name)))
    }

    fn on_end(&mut self, e: BytesEnd<'_>) -> Result<()> {
        if !self.frames.is_empty() {
            let path = self.path.clone();
            let unused = self.take_all_unused(&path);
            let ws = self.held_ws.clone();
            self.emit_at_end(&unused, &ws)?;
            self.frames.pop();
            self.path.pop();
        }
        self.flush_ws()?;
        self.write(Event::End(e.into_owned()))
    }

    fn on_text(&mut self, t: BytesText<'_>) -> Result<()> {
        let bytes: &[u8] = t.as_ref();
        let is_ws = !bytes.is_empty() && bytes.iter().all(|b| b" \t\r\n".contains(b));
        self.flush_ws()?;
        if is_ws {
            self.held_ws =
                Some(String::from_utf8(bytes.to_vec()).map_err(|e| Error::Xml(e.to_string()))?);
            Ok(())
        } else {
            self.write(Event::Text(t.into_owned()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Comments in every supported position: head (before the root, after the declaration), between
    /// top-level children, first/last inside a comp, inside a nested visual, inside a joint, inside
    /// an extension body (the verbatim channel, NOT this one), before the root's end tag, and after
    /// the root (tail).
    const FIXTURE: &str = r#"<?xml version="1.0"?><!--H1--><!--H2-->
<hcdf name="c" version="1.0"><!--A-->
  <description>d</description><!--B1--><!--B2-->
  <comp name="one"><!--C-->
    <inertial><mass>1</mass></inertial>
    <visual name="v"><geometry><box size="1 1 1"/></geometry><!--D--></visual><!--E-->
    <collision name="k"><geometry><box size="1 1 1"/></geometry></collision><!--F-->
  </comp>
  <comp name="two"/>
  <joint name="j" type="fixed"><parent comp="one"/><child comp="two"/><!--G--></joint>
  <extension domain="x"><blob><!--X--></blob></extension><!--Z-->
</hcdf><!--T-->"#;

    fn before(name: &str, occurrence: usize) -> CommentAnchor {
        CommentAnchor::BeforeElement {
            name: name.to_string(),
            occurrence,
        }
    }

    fn idx(haystack: &str, needle: &str) -> usize {
        haystack
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in output"))
    }

    #[test]
    fn round_trip_every_position() {
        let doc = Hcdf::from_xml_str(FIXTURE).unwrap();

        assert_eq!(doc.comments.head, ["H1", "H2"]);
        assert_eq!(doc.comments.tail, ["T"]);
        let body: Vec<(&[PathStep], &CommentAnchor, &str)> = doc
            .comments
            .body
            .iter()
            .map(|c| (c.path.as_slice(), &c.anchor, c.text.as_str()))
            .collect();
        assert_eq!(
            body,
            [
                (&[][..], &before("description", 0), "A"),
                (&[][..], &before("comp", 0), "B1"),
                (&[][..], &before("comp", 0), "B2"),
                (&[][..], &CommentAnchor::AtEnd, "Z"),
            ]
        );

        let one: Vec<(&[PathStep], &CommentAnchor, &str)> = doc.comp[0]
            .comments
            .0
            .iter()
            .map(|c| (c.path.as_slice(), &c.anchor, c.text.as_str()))
            .collect();
        let visual0: &[PathStep] = &[("visual".to_string(), 0)];
        assert_eq!(
            one,
            [
                (&[][..], &before("inertial", 0), "C"),
                (visual0, &CommentAnchor::AtEnd, "D"),
                (&[][..], &before("collision", 0), "E"),
                (&[][..], &CommentAnchor::AtEnd, "F"),
            ]
        );
        assert!(doc.comp[1].comments.is_empty());
        assert_eq!(doc.joint[0].comments.0.len(), 1);
        assert_eq!(doc.joint[0].comments.0[0].anchor, CommentAnchor::AtEnd);
        assert_eq!(doc.joint[0].comments.0[0].text, "G");
        // X belongs to the verbatim extension body channel, not the comment side-channel.
        assert_eq!(doc.extension[0].body, "<blob><!--X--></blob>");

        // Pretty-printed output places each comment on its own indented line, so ordering (not raw
        // adjacency) is the fidelity bar; the two head comments stay adjacent (emitted back-to-back
        // before the root) and the tail comment follows the root's end tag.
        let out = doc.to_xml_string().unwrap();
        assert_eq!(
            out.matches("<!--").count(),
            13,
            "12 captured + X in the body: {out}"
        );
        assert!(idx(&out, "<!--H1--><!--H2-->") < idx(&out, "<hcdf"));
        assert!(idx(&out, "<!--A-->") < idx(&out, "<description"));
        assert!(idx(&out, "<!--B1-->") < idx(&out, "<!--B2-->"));
        assert!(idx(&out, "<!--B2-->") < idx(&out, r#"<comp name="one""#));
        assert!(idx(&out, "<!--C-->") < idx(&out, "<inertial"));
        // D anchors AtEnd inside the visual: after its <geometry>, before the visual's end tag.
        assert!(
            idx(&out, "</geometry>") < idx(&out, "<!--D-->"),
            "D after geometry: {out}"
        );
        assert!(
            idx(&out, "<!--D-->") < idx(&out, "</visual>"),
            "D stays inside the visual: {out}"
        );
        assert!(idx(&out, "</visual>") < idx(&out, "<!--E-->"));
        assert!(idx(&out, "<!--E-->") < idx(&out, "<collision"));
        assert!(
            idx(&out, "<!--F-->") < idx(&out, "</comp>"),
            "F closes comp one's content"
        );
        // G anchors AtEnd inside the joint: after its <child>, before the joint's end tag.
        assert!(
            idx(&out, r#"<child comp="two"/>"#) < idx(&out, "<!--G-->"),
            "G after child: {out}"
        );
        assert!(
            idx(&out, "<!--G-->") < idx(&out, "</joint>"),
            "G stays inside the joint: {out}"
        );
        assert!(idx(&out, "<!--Z-->") < idx(&out, "</hcdf>"));
        assert!(idx(&out, "</hcdf>") < idx(&out, "<!--T-->"));
    }

    #[test]
    fn idempotent() {
        // serialize(parse(...)) is a byte fixpoint: every anchor that resolved re-captures exactly,
        // and degraded anchors re-capture as AtEnd at the position they were emitted.
        let s1 = Hcdf::from_xml_str(FIXTURE)
            .unwrap()
            .to_xml_string()
            .unwrap();
        let s2 = Hcdf::from_xml_str(&s1).unwrap().to_xml_string().unwrap();
        assert_eq!(s1, s2);
    }

    #[test]
    fn no_double_handling_in_extension() {
        let doc = Hcdf::from_xml_str(FIXTURE).unwrap();
        assert!(doc.extension[0].body.contains("<!--X-->"));
        let mut all_sets = doc
            .comments
            .body
            .iter()
            .chain(doc.comp.iter().flat_map(|c| c.comments.0.iter()))
            .chain(doc.joint.iter().flat_map(|j| j.comments.0.iter()));
        assert!(
            all_sets.all(|c| c.text != "X"),
            "X must not be double-captured"
        );
        assert!(!doc
            .comments
            .head
            .iter()
            .chain(&doc.comments.tail)
            .any(|t| t.as_str() == "X"));
        let out = doc.to_xml_string().unwrap();
        assert_eq!(out.matches("<!--X-->").count(), 1);
    }

    #[test]
    fn mixed_text_degrades() {
        // Deliberate degradation (module docs #1): the comment survives but repositions to the
        // element-content boundary, and the split text parses as ONE node.
        let src = r#"<hcdf name="c" version="1.0"><description>a<!--c-->b</description></hcdf>"#;
        let doc = Hcdf::from_xml_str(src).unwrap();
        assert_eq!(doc.description.as_deref(), Some("ab"));
        let out = doc.to_xml_string().unwrap();
        assert!(
            out.contains("<description>ab<!--c--></description>"),
            "{out}"
        );
    }

    #[test]
    fn dropped_anchor_flushes() {
        // An editor can remove an element after comments have been anchored. The comment survives,
        // with its position degrading to end-of-parent (module docs #4).
        let src = r#"<hcdf name="c" version="1.0"><!--c--><comp name="x"/></hcdf>"#;
        let mut doc = Hcdf::from_xml_str(src).unwrap();
        doc.comp.clear();
        let out = doc.to_xml_string().unwrap();
        assert!(!out.contains("<comp"), "{out}");
        assert!(
            idx(&out, "<!--c-->") < idx(&out, "</hcdf>"),
            "c flushes to end of root: {out}"
        );
    }

    #[test]
    fn empty_element_comments_expand() {
        // A comment whose parent serializes as an EMPTY element re-opens it (`<x/>` -> `<x>…</x>`),
        // both for a comp and for the root itself.
        let src = r#"<hcdf name="c" version="1.0"><comp name="x"><!--only--></comp></hcdf>"#;
        let out = Hcdf::from_xml_str(src).unwrap().to_xml_string().unwrap();
        // The comp serializes empty; holding a comment re-opens it (`<x/>` -> `<x>…</x>`) so the
        // comment has content to live in, here on its own indented line between the tags.
        assert!(
            idx(&out, r#"<comp name="x">"#) < idx(&out, "<!--only-->"),
            "{out}"
        );
        assert!(idx(&out, "<!--only-->") < idx(&out, "</comp>"), "{out}");

        // The root itself expands the same way; with no held sibling indent the comment sits flush
        // between the (attribute-bearing) start tag and the end tag.
        let root_only = r#"<hcdf name="c" version="1.0"><!--solo--></hcdf>"#;
        let out = Hcdf::from_xml_str(root_only)
            .unwrap()
            .to_xml_string()
            .unwrap();
        assert!(
            out.contains(r#"<hcdf name="c" version="1.0"><!--solo--></hcdf>"#),
            "{out}"
        );
    }

    #[test]
    fn sha_covers_comments() {
        // A comment edit is a content change: it must flow into the module's canonical sha.
        let plain =
            Hcdf::from_xml_str(r#"<hcdf name="m" version="1.0"><comp name="base"/></hcdf>"#)
                .unwrap();
        let commented = Hcdf::from_xml_str(
            r#"<hcdf name="m" version="1.0"><!--note--><comp name="base"/></hcdf>"#,
        )
        .unwrap();
        assert_ne!(
            crate::compose::content_sha_of_module(&plain).unwrap(),
            crate::compose::content_sha_of_module(&commented).unwrap()
        );
    }

    #[test]
    fn fragment_splice_indents() {
        // Indented editor fragments get each comment on its own line: at the anchor sibling's
        // indent for BeforeElement, one step deeper than the closing tag for AtEnd.
        let frag = "<comp name=\"one\">\n  <inertial>\n    <mass>1</mass>\n  </inertial>\n  \
                    <collision name=\"k\">\n    <geometry>\n      <box size=\"1 1 1\"/>\n    \
                    </geometry>\n  </collision>\n</comp>";
        let comments = vec![
            CapturedComment {
                path: vec![],
                anchor: before("collision", 0),
                text: "c".into(),
            },
            CapturedComment {
                path: vec![],
                anchor: CommentAnchor::AtEnd,
                text: "z".into(),
            },
        ];
        let out = splice_fragment_comments(frag, &comments).unwrap();
        assert!(out.contains("\n  <!--c-->\n  <collision"), "{out}");
        assert!(out.contains("\n  <!--z-->\n</comp>"), "{out}");
        // Re-extraction reproduces the same anchors (fragment-level idempotency).
        let fc = extract_fragment_comments(&out).unwrap();
        assert_eq!(fc.outside, 0);
        assert_eq!(fc.inner, comments);
        assert!(!fc.stripped.contains("<!--"));
    }

    #[test]
    fn fragment_extract_counts_outside_and_keeps_extension_bodies() {
        // Comments outside the fragment root are counted, not preserved (module docs #2).
        let src = r#"<!--out--><comp name="x"><!--in--><inertial><mass>1</mass></inertial></comp>"#;
        let fc = extract_fragment_comments(src).unwrap();
        assert_eq!(fc.outside, 1);
        assert_eq!(fc.inner.len(), 1);
        assert_eq!(fc.inner[0].path, []);
        assert_eq!(fc.inner[0].anchor, before("inertial", 0));
        assert_eq!(fc.inner[0].text, "in");
        assert!(!fc.stripped.contains("<!--"));

        // A comment inside an <extension> subtree stays in place in the stripped fragment (the
        // verbatim body channel owns it) and never reaches the comment collector.
        let src =
            r#"<comp name="x"><extension domain="d"><blob><!--X--></blob></extension></comp>"#;
        let fc = extract_fragment_comments(src).unwrap();
        assert_eq!((fc.inner.len(), fc.outside), (0, 0));
        assert!(fc.stripped.contains("<blob><!--X--></blob>"));
    }
}
