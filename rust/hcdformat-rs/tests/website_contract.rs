use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

const HANDWRITTEN_PAGES: &[&str] = &[
    "index.html",
    "about/index.html",
    "design/index.html",
    "examples/index.html",
    "extensions/index.html",
    "extensions/imu-stability.html",
    "spec/index.html",
    "tools/index.html",
];

fn website_dir() -> Option<PathBuf> {
    let website = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("website");
    website.is_dir().then_some(website)
}

fn read_page(website: &Path, relative: &str) -> String {
    let path = website.join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn normalized_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn code_fragments(input: &str) -> Vec<&str> {
    let mut fragments = Vec::new();
    let mut remaining = input;

    while let Some(code_start) = remaining.find("<code") {
        remaining = &remaining[code_start + "<code".len()..];
        let Some(body_start) = remaining.find('>') else {
            break;
        };
        remaining = &remaining[body_start + 1..];
        let Some(body_end) = remaining.find("</code>") else {
            break;
        };
        fragments.push(&remaining[..body_end]);
        remaining = &remaining[body_end + "</code>".len()..];
    }

    fragments
}

fn assert_port_tags_use_current_grammar(relative: &str, source: &str) {
    let source = normalized_whitespace(source);

    for (start_marker, end_marker) in [("&lt;port ", "&gt;"), ("<port ", ">")] {
        let mut remaining = source.as_str();
        while let Some(start) = remaining.find(start_marker) {
            remaining = &remaining[start..];
            let Some(end) = remaining.find(end_marker) else {
                panic!("{relative} contains an unterminated port tag");
            };
            let opening_tag = &remaining[..end];
            for retired_attribute in [" iface=", " type=", " speed="] {
                assert!(
                    !opening_tag.contains(retired_attribute),
                    "{relative} contains retired port attribute {retired_attribute:?}: {opening_tag}"
                );
            }
            remaining = &remaining[end + end_marker.len()..];
        }
    }
}

fn collect_html_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!(
                "failed to read an entry in {}: {error}",
                directory.display()
            )
        });
        let path = entry.path();
        if path.is_dir() {
            collect_html_files(&path, files);
        } else if path.extension() == Some(OsStr::new("html")) {
            files.push(path);
        }
    }
}

fn check_attributes(element: &BytesStart<'_>) -> Result<(), String> {
    for attribute in element.attributes() {
        attribute.map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn check_xml_attributes(element: &BytesStart<'_>) -> Result<(), String> {
    for attribute in element.attributes() {
        attribute
            .map_err(|error| error.to_string())?
            .unescape_value()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn is_xml_code_block(element: &BytesStart<'_>) -> Result<bool, String> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| error.to_string())?;
        if attribute.key.as_ref() == b"class" {
            let value = attribute
                .unescape_value()
                .map_err(|error| error.to_string())?;
            return Ok(value
                .split_ascii_whitespace()
                .any(|class| class == "language-xml"));
        }
    }
    Ok(false)
}

fn extract_xml_snippets(relative: &str, source: &str) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(source);
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut root_count = 0usize;
    let mut in_xml_pre = false;
    let mut in_xml_code = false;
    let mut snippet = String::new();
    let mut snippets = Vec::new();

    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("{relative}: {error}"))?;
        match event {
            Event::Start(element) => {
                check_attributes(&element).map_err(|error| format!("{relative}: {error}"))?;
                let name = element.name().as_ref().to_vec();
                if stack.is_empty() {
                    root_count += 1;
                }
                if in_xml_code {
                    return Err(format!(
                        "{relative}: XML sample contains unescaped <{}>",
                        String::from_utf8_lossy(&name)
                    ));
                }
                if name == b"pre" {
                    in_xml_pre = is_xml_code_block(&element)
                        .map_err(|error| format!("{relative}: {error}"))?;
                } else if in_xml_pre && name == b"code" {
                    in_xml_code = true;
                    snippet.clear();
                }
                stack.push(name);
            }
            Event::Empty(element) => {
                check_attributes(&element).map_err(|error| format!("{relative}: {error}"))?;
                if stack.is_empty() {
                    root_count += 1;
                }
                if in_xml_code {
                    return Err(format!(
                        "{relative}: XML sample contains an unescaped empty element"
                    ));
                }
            }
            Event::End(element) => {
                let name = element.name().as_ref().to_vec();
                let Some(open) = stack.pop() else {
                    return Err(format!(
                        "{relative}: unexpected closing tag </{}>",
                        String::from_utf8_lossy(&name)
                    ));
                };
                if open != name {
                    return Err(format!(
                        "{relative}: closing tag </{}> does not match <{}>",
                        String::from_utf8_lossy(&name),
                        String::from_utf8_lossy(&open)
                    ));
                }
                if in_xml_code && name == b"code" {
                    snippets.push(std::mem::take(&mut snippet));
                    in_xml_code = false;
                } else if in_xml_code {
                    return Err(format!(
                        "{relative}: XML sample contains unescaped closing markup"
                    ));
                }
                if in_xml_pre && name == b"pre" {
                    in_xml_pre = false;
                }
            }
            Event::Text(text) if in_xml_code => {
                snippet.push_str(
                    std::str::from_utf8(text.as_ref())
                        .map_err(|error| format!("{relative}: {error}"))?,
                );
            }
            Event::CData(text) if in_xml_code => {
                snippet.push_str(
                    std::str::from_utf8(text.as_ref())
                        .map_err(|error| format!("{relative}: {error}"))?,
                );
            }
            Event::Comment(_) if in_xml_code => {
                return Err(format!(
                    "{relative}: XML sample contains an unescaped comment"
                ));
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if let Some(open) = stack.last() {
        return Err(format!(
            "{relative}: unclosed <{}> element",
            String::from_utf8_lossy(open)
        ));
    }
    if root_count != 1 {
        return Err(format!(
            "{relative}: expected one HTML root element, found {root_count}"
        ));
    }
    if in_xml_code || in_xml_pre {
        return Err(format!("{relative}: unclosed XML code block"));
    }

    Ok(snippets)
}

fn decode_five_xml_entities(source: &str) -> String {
    source
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn assert_well_formed_xml(label: &str, source: &str) -> Result<(), String> {
    let mut reader = Reader::from_str(source);
    let mut stack: Vec<Vec<u8>> = Vec::new();

    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("{label}: {error}"))?;
        match event {
            Event::Start(element) => {
                check_xml_attributes(&element).map_err(|error| format!("{label}: {error}"))?;
                stack.push(element.name().as_ref().to_vec());
            }
            Event::Empty(element) => {
                check_xml_attributes(&element).map_err(|error| format!("{label}: {error}"))?;
            }
            Event::End(element) => {
                let name = element.name().as_ref().to_vec();
                let Some(open) = stack.pop() else {
                    return Err(format!(
                        "{label}: unexpected closing tag </{}>",
                        String::from_utf8_lossy(&name)
                    ));
                };
                if open != name {
                    return Err(format!(
                        "{label}: closing tag </{}> does not match <{}>",
                        String::from_utf8_lossy(&name),
                        String::from_utf8_lossy(&open)
                    ));
                }
            }
            Event::Text(text) => {
                text.unescape()
                    .map_err(|error| format!("{label}: {error}"))?;
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if let Some(open) = stack.last() {
        return Err(format!(
            "{label}: unclosed <{}> element",
            String::from_utf8_lossy(open)
        ));
    }
    Ok(())
}

#[test]
fn handwritten_website_rejects_retired_schema_and_cli_contracts() {
    let Some(website) = website_dir() else {
        return;
    };

    let retired_text = [
        "&lt;network&gt;",
        "&lt;network ",
        "<network>",
        "<network ",
        "&lt;material&gt;",
        "&lt;material ",
        "<material>",
        "<material ",
        "147 types",
        "147 schema types",
        "42 enums",
        "42 enumerations",
        "every file is sha256-verified",
        "every file is sha-256-verified",
        "every referenced artifact carries a sha256 hash",
        "every referenced artifact carries a sha-256 hash",
        "all referenced artifacts must carry a sha",
        "verified distribution of all referenced artifacts",
    ];

    for relative in HANDWRITTEN_PAGES {
        let source = read_page(&website, relative);
        let lowercase = normalized_whitespace(&source).to_ascii_lowercase();
        for retired in retired_text {
            assert!(
                !lowercase.contains(retired),
                "{relative} contains retired website contract {retired:?}"
            );
        }

        for code in code_fragments(&source) {
            let code = normalized_whitespace(code);
            if code.contains("hcdf convert") {
                let has_retired_flag = code
                    .split_whitespace()
                    .any(|token| token == "-i" || token == "-o");
                assert!(
                    !has_retired_flag,
                    "{relative} documents retired convert flags: {code}"
                );
            }
            assert!(
                !code.contains("hcdf info"),
                "{relative} documents the retired hcdf info command: {code}"
            );
            assert!(
                !code.contains("hcdf schema"),
                "{relative} documents the retired hcdf schema command: {code}"
            );
        }

        assert_port_tags_use_current_grammar(relative, &source);
    }
}

#[test]
fn design_and_examples_document_the_connectivity_contract() {
    let Some(website) = website_dir() else {
        return;
    };

    let design = read_page(&website, "design/index.html");
    let examples = read_page(&website, "examples/index.html");
    let combined = format!("{design}\n{examples}");

    for vocabulary in [
        "communication",
        "power-delivery",
        "material-transfer",
        "electrical",
        "guided-optical",
        "conducted-rf",
        "radiated-rf",
        "liquid",
        "gas",
    ] {
        assert!(
            combined.contains(vocabulary),
            "design and examples omit connectivity vocabulary {vocabulary:?}"
        );
    }

    for topology in ["link", "bus", "chain", "star", "ring", "mesh", "tree"] {
        let code_term = format!("<code>{topology}</code>");
        let element_term = format!("&lt;{topology} ");
        assert!(
            combined.contains(&code_term) || combined.contains(&element_term),
            "design and examples omit topology {topology:?}"
        );
    }
}

#[test]
fn handwritten_pages_and_xml_samples_are_well_formed() {
    let Some(website) = website_dir() else {
        return;
    };

    for relative in HANDWRITTEN_PAGES {
        let source = read_page(&website, relative);
        let snippets = extract_xml_snippets(relative, &source)
            .unwrap_or_else(|error| panic!("handwritten page is not XHTML-style markup: {error}"));

        for (index, snippet) in snippets.iter().enumerate() {
            let decoded = decode_five_xml_entities(snippet);
            let wrapped = format!("<website-snippet>{decoded}</website-snippet>");
            let label = format!("{relative} XML sample {}", index + 1);
            assert_well_formed_xml(&label, &wrapped)
                .unwrap_or_else(|error| panic!("embedded XML is not well formed: {error}"));
        }
    }
}

#[test]
fn website_html_contains_no_unicode_em_dash() {
    let Some(website) = website_dir() else {
        return;
    };

    let mut files = Vec::new();
    collect_html_files(&website, &mut files);
    files.sort();

    assert!(
        !files.is_empty(),
        "website directory contains no HTML files"
    );
    for path in files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert!(
            !source.contains('\u{2014}'),
            "{} contains U+2014",
            path.display()
        );
    }
}
