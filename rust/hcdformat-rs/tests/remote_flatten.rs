//! End-to-end coverage for the remote-`<include>`-fetching flatten ([`hcdformat::compose::flatten_path_fetch`])
//! and the `--vendor-remote` bundle path that builds on it, mirroring the Python
//! `include.flatten(fetch=True)` + `bundle.pack(vendor_remote=True)` tests.
//!
//! Each test stands up a tiny in-process HTTP server (a raw `std::net::TcpListener` in a thread, no extra
//! deps, the same harness shape `remote_security.rs` uses), serves an HCDF sub-module by path, authors a
//! parent doc on disk with a remote `<include uri="http://…">`, and asserts one of:
//!
//! - flatten(fetch) inlines the remote module (prefix / merge), leaving NO live `<include>`;
//! - a remote module's relative mesh uri is rerooted onto the module URL (remote base propagation);
//! - `bundle::pack(Vendor)` flattens the remote include in Rust and the bundle verifies clean;
//! - a DEAD remote uri is FATAL (an `Err`, never a silently-dropped include) for BOTH flatten + pack.
//!
//! Only built with the `remote` feature (the fetcher only exists there) and never on wasm.
#![cfg(all(feature = "remote", not(target_arch = "wasm32")))]

use hcdformat::bundle::{pack, PackOptions, RemotePolicy};
use hcdformat::compose::{flatten_fetch, flatten_path_fetch};
use hcdformat::{open_bundle, verify, Hcdf, VisualAppearance};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

/// A path-routed one-shot test server: binds 127.0.0.1:0, serves each request's path from `routes`
/// (200 with the body, else 404), and returns its `http://127.0.0.1:<port>/` base url. Serves until the
/// process ends (tests are short-lived); each test uses its own server so there is no cross-talk.
fn serve(routes: HashMap<String, Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let routes = Arc::new(routes);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let routes = Arc::clone(&routes);
            handle(stream, &routes);
        }
    });
    format!("http://127.0.0.1:{port}/")
}

/// Read the request line + headers, extract the request-target path, and write the routed response.
fn handle(mut stream: TcpStream, routes: &HashMap<String, Vec<u8>>) {
    let path = read_request_path(&mut stream);
    match path.and_then(|p| routes.get(&p)) {
        Some(body) => {
            let hdr = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = stream.write_all(hdr.as_bytes());
            let _ = stream.write_all(body);
        }
        None => {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }
    }
}

/// Parse the request-target path (e.g. `/sub.hcdf`) from the first request line, draining headers.
fn read_request_path(stream: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 1];
    let mut data: Vec<u8> = Vec::new();
    let mut last4 = [0u8; 4];
    while data.len() < 64 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                data.push(buf[0]);
                last4 = [last4[1], last4[2], last4[3], buf[0]];
                if &last4 == b"\r\n\r\n" {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&data);
    let line = text.lines().next()?; // "GET /sub.hcdf HTTP/1.1"
    let mut parts = line.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    Some(target.to_string())
}

fn unique_dir(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "hcdf-remote-flatten-{tag}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A remote sub-module: one comp `base` carrying a visual with a RELATIVE mesh uri (to exercise the
/// remote-base mesh reroot) and a second comp `link` joined to it.
const SUB_HCDF: &str = r#"<hcdf name="arm-module" version="1.0">
  <comp name="base">
    <port name="data"/>
    <visual name="v"><model uri="assets/base.glb"/></visual>
  </comp>
  <comp name="link"><port name="data"/></comp>
  <joint name="j0" type="fixed"><parent comp="base"/><child comp="link"/></joint>
  <tree name="route">
    <root><hop-ref network="route" hop="h0"/></root>
    <participant name="base"><endpoint><port-ref component="base" port="data"/></endpoint></participant>
    <participant name="link"><endpoint><port-ref component="link" port="data"/></endpoint></participant>
    <hop name="h0"><owner><component-ref component="base"/></owner></hop>
    <hop name="h1"><owner><component-ref component="link"/></owner></hop>
    <leg name="forward"><from><hop-ref network="route" hop="h0"/><participant-ref network="route" participant="base"/></from><to><hop-ref network="route" hop="h1"/><participant-ref network="route" participant="link"/></to></leg>
  </tree>
</hcdf>"#;

/// Author a parent `.hcdf` on disk that includes `sub_url` under `@name`.
fn write_parent(dir: &std::path::Path, sub_url: &str) -> PathBuf {
    let parent = format!(
        "<hcdf name=\"robot\" version=\"1.0\">\n  <include uri=\"{sub_url}\" name=\"arm\"/>\n</hcdf>\n"
    );
    let path = dir.join("robot.hcdf");
    std::fs::write(&path, parent).unwrap();
    path
}

#[test]
fn flatten_fetch_inlines_remote_include_and_reroots_mesh() {
    let mut routes = HashMap::new();
    routes.insert("/sub.hcdf".to_string(), SUB_HCDF.as_bytes().to_vec());
    let base = serve(routes);
    let sub_url = format!("{base}sub.hcdf");

    let dir = unique_dir("flatten");
    let cache = dir.join("cache");
    let parent = write_parent(&dir, &sub_url);

    let (doc, notes) = flatten_path_fetch(&parent, Some(&cache)).expect("remote flatten");

    // The remote module's comps are inlined + prefixed under the include @name; no live <include> remains.
    assert!(
        doc.include.is_empty(),
        "remote include must be inlined, notes: {notes:?}"
    );
    let names: Vec<&str> = doc.comp.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"arm/base"),
        "expected prefixed comp arm/base, got {names:?}"
    );
    assert!(
        names.contains(&"arm/link"),
        "expected prefixed comp arm/link, got {names:?}"
    );
    let tree = doc
        .tree
        .iter()
        .find(|value| value.name == "arm/route")
        .expect("prefixed remote tree");
    assert_eq!(tree.root.hop.network, "arm/route");
    for (participant, component) in tree.participant.iter().zip(["arm/base", "arm/link"]) {
        match &participant.endpoint.endpoint {
            hcdformat::model::connectivity_xml::FunctionalEndpointChoice::Port(reference) => {
                assert_eq!(reference.component, component);
                assert!(reference.instance.is_none());
            }
            other => panic!("expected port endpoint, got {other:?}"),
        }
    }
    for (hop, component) in tree.hop.iter().zip(["arm/base", "arm/link"]) {
        match &hop.owner.owner {
            hcdformat::model::connectivity_xml::HopOwnerChoice::Component(reference) => {
                assert_eq!(reference.component, component);
                assert!(reference.instance.is_none());
            }
            other => panic!("expected component owner, got {other:?}"),
        }
    }
    let leg = &tree.leg[0];
    for network in [
        &leg.from.hop.network,
        &leg.from.participant.network,
        &leg.to.hop.network,
        &leg.to.participant.network,
    ] {
        assert_eq!(network, "arm/route");
    }

    // The module's RELATIVE mesh uri was rerooted onto the REMOTE module URL (base propagation).
    let mesh_uri = doc
        .comp
        .iter()
        .find(|c| c.name == "arm/base")
        .and_then(|c| c.visual.first())
        .and_then(|v| match &v.appearance {
            VisualAppearance::Model { model, .. } => model.uri.clone(),
            _ => None,
        })
        .expect("arm/base visual model uri");
    let expected = format!("{base}assets/base.glb");
    assert_eq!(mesh_uri, expected, "remote-base mesh reroot mismatch");

    assert!(
        notes.iter().any(|n| n.contains("flattened include")),
        "expected a flattened-include note, got {notes:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pack_vendor_remote_include_flattens_in_rust() {
    // The sub here references NO mesh, so the bundle is the flattened root doc alone (no asset baking in
    // this test); the point is that pack(Vendor) FETCHES + INLINES the remote <include> in Rust and the
    // resulting bundle is flatten-only and verifies clean.
    const SUB_NOMESH: &str = r#"<hcdf name="arm-module" version="1.0">
  <comp name="base"/>
  <comp name="link"/>
  <joint name="j0" type="fixed"><parent comp="base"/><child comp="link"/></joint>
</hcdf>"#;
    let mut routes = HashMap::new();
    routes.insert("/sub.hcdf".to_string(), SUB_NOMESH.as_bytes().to_vec());
    let base = serve(routes);
    let sub_url = format!("{base}sub.hcdf");

    let dir = unique_dir("pack");
    let parent = write_parent(&dir, &sub_url);
    let out = dir.join("robot.hcdfz");
    let opts = PackOptions {
        remote: RemotePolicy::Vendor,
        cache_dir: Some(dir.join("cache")),
        ..Default::default()
    };
    let manifest = pack(&parent, &out, &opts).expect("pack --vendor-remote");
    assert!(
        manifest.kept_remote.is_empty(),
        "vendor bundle must keep no remote sites"
    );

    // Open the bundle and confirm the remote include was inlined (no <include>) + comps present.
    let opened = open_bundle(&out).expect("open bundle");
    let doc: &Hcdf = &opened.doc;
    assert!(doc.include.is_empty(), "bundle must be flatten-only");
    let names: Vec<&str> = doc.comp.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"arm/base") && names.contains(&"arm/link"),
        "got {names:?}"
    );
    // The extraction tempdir is removed automatically when `opened` (its BundleDir guard) drops.
    drop(opened);

    // And it verifies clean.
    let report = verify(&out);
    assert!(report.ok, "bundle verify issues: {:?}", report.issues);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dead_remote_uri_is_fatal_for_flatten_and_pack() {
    // A server that 404s every path -> the fetch fails -> flatten + pack must ERROR, never drop the
    // include silently.
    let base = serve(HashMap::new());
    let sub_url = format!("{base}missing.hcdf");

    let dir = unique_dir("dead");
    let parent = write_parent(&dir, &sub_url);
    let cache = dir.join("cache");

    let flat_err = flatten_path_fetch(&parent, Some(&cache)).unwrap_err();
    assert!(
        flat_err.contains("could not be fetched"),
        "dead remote include must be fatal for flatten, got {flat_err:?}"
    );

    let out = dir.join("robot.hcdfz");
    let opts = PackOptions {
        remote: RemotePolicy::Vendor,
        cache_dir: Some(cache),
        ..Default::default()
    };
    let pack_err = pack(&parent, &out, &opts).unwrap_err();
    assert!(
        pack_err.contains("could not be fetched"),
        "dead remote include must be fatal for pack, got {pack_err:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The in-memory fetch-flatten `flatten_fetch(doc, base_dir, cache)` the binding exposes must produce the
/// byte-identical flattened document `flatten_path_fetch(file, cache)` produces on a LOCAL-only tree (no
/// network): the two share the same fetch resolver, so for local includes only the cycle-stack seed
/// differs, which never matters without a self-including cycle. Proves the byte-based fetch flow degrades
/// to the path-based one for the common all-local case.
#[test]
fn flatten_fetch_doc_matches_flatten_path_fetch_local() {
    let dir = unique_dir("fetch-local-parity");
    let leaf = r#"<hcdf name="leaf" version="1.0"><comp name="device">
      <connector name="inside"><representation><sphere radius="1">
        <placement xyz="1 0 0"><frame><component-origin component="device"/></frame>
          <rotation><rpy value="0 0 0"/></rotation></placement>
      </sphere></representation></connector>
    </comp></hcdf>"#;
    let module = r#"<hcdf name="module" version="1.0">
      <include uri="leaf.hcdf" name="inner" pose="1 0 0 0 0 1.5707963267948966"/>
    </hcdf>"#;
    let top = r#"<hcdf name="t" version="1.0"><comp name="host">
      <connector name="outside"><representation><sphere radius="1">
        <placement xyz="1 0 0">
          <frame><component-origin component="device"><instance>
            <segment name="L" occurrence="0"/><segment name="inner" occurrence="0"/>
          </instance></component-origin></frame>
          <rotation><rpy value="0 0 0"/></rotation>
        </placement>
      </sphere></representation></connector>
    </comp><include uri="mod.hcdf" name="L"
      pose="10 0 0 0 0 1.5707963267948966"/></hcdf>"#;
    std::fs::write(dir.join("leaf.hcdf"), leaf).unwrap();
    std::fs::write(dir.join("mod.hcdf"), module).unwrap();
    std::fs::write(dir.join("top.hcdf"), top).unwrap();
    let cache = dir.join("cache");

    let (from_path, path_notes) = flatten_path_fetch(&dir.join("top.hcdf"), Some(&cache)).unwrap();
    let mut from_doc = Hcdf::from_xml_str(top).unwrap();
    let doc_notes = hcdformat::flatten_fetch(&mut from_doc, &dir, Some(&cache)).unwrap();

    assert_eq!(
        from_path.to_xml_string().unwrap(),
        from_doc.to_xml_string().unwrap(),
        "flatten_fetch(doc, dir) equals flatten_path_fetch(file) on a local tree"
    );
    let placement = |doc: &Hcdf, component: &str, connector: &str| {
        let representation = doc
            .comp
            .iter()
            .find(|value| value.name == component)
            .unwrap()
            .connector
            .iter()
            .find(|value| value.name == connector)
            .unwrap()
            .representation
            .as_ref()
            .unwrap();
        match &representation.variant {
            hcdformat::model::connectivity_xml::RepresentationChoice::Sphere(value) => {
                value.placement.clone()
            }
            other => panic!("expected sphere representation, got {other:?}"),
        }
    };
    let child = placement(&from_doc, "L/inner/device", "inside");
    let parent = placement(&from_doc, "host", "outside");
    for (actual, expected) in child.xyz.into_iter().zip([9.0, 1.0, 0.0]) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }
    for (actual, expected) in parent.xyz.into_iter().zip(child.xyz) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }
    assert_eq!(parent.rotation, child.rotation);
    assert_eq!(
        path_notes, doc_notes,
        "the fetch-flatten notes agree for a local tree"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fetch_flatten_rejects_duplicate_sibling_include_names() {
    let mut doc = Hcdf::from_xml_str(
        r#"<hcdf name="p" version="1.0">
          <include uri="first.hcdf" name="module"/>
          <include uri="second.hcdf" name="module"/>
        </hcdf>"#,
    )
    .unwrap();
    let error = flatten_fetch(&mut doc, std::path::Path::new("/p"), None).unwrap_err();
    assert!(error.contains("duplicate sibling include name \"module\""));
    assert_eq!(doc.include.len(), 2, "validation must be transactional");
}

#[test]
fn fetch_flatten_rejects_uri_less_include() {
    let mut doc =
        Hcdf::from_xml_str(r#"<hcdf name="p" version="1.0"><include name="module"/></hcdf>"#)
            .unwrap();
    let original = doc.clone();
    let error = flatten_fetch(&mut doc, std::path::Path::new("/p"), None).unwrap_err();
    assert!(error.contains("missing required @uri"));
    assert_eq!(
        doc, original,
        "failed fetch flatten must not mutate its input"
    );
}
