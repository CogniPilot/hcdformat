//! Bundle layer tests: pack/open/verify a flatten-only `.hcdfz` (and `--dir`), plus FROZEN bundle
//! STRUCTURE goldens (the Rust `bundle::pack` output is the source of truth).
//!
//! The structure golden is the ordered entry-name list + the root `.hcdf` doc (which carries the
//! rerooted `@uri`/`@sha` and the flattened comp set). `bundle::pack` is deterministic and
//! content-addressed (asset name = `<stem>_<sha12>.<ext>`), so identical names imply identical bytes and
//! the projection freezes cleanly with NO Python in the test path. Regenerate with `HCDF_REGEN_GOLDENS=1`.

#![cfg(not(target_arch = "wasm32"))]

use hcdformat::bundle::{self, OpenedBundle, PackOptions, RemotePolicy};
use hcdformat::zip_store;
use hcdformat::{open_bundle_bytes, pack_to_bytes, verify_bytes, Hcdf, MemBundle};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A self-contained fixture written to a fresh temp dir: a root `robot.hcdf` that includes a
/// `motors/motor.hcdf` twice, a visual GLB beside the root, and a lean collision STL inside the
/// module's own subdir (so the flatten -> vendor base_dir seam is exercised).
struct Fixture {
    dir: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = unique_dir(&format!("hcdf-bundle-fixture-{tag}-"));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::create_dir_all(dir.join("motors/assets")).unwrap();

        // Deterministic "GLB" + "STL" bytes; the bundle layer treats them as opaque blobs, so real
        // mesh formats are unnecessary for content-addressing parity. The extensions drive the
        // vendored name (`<stem>_<short12>.<ext>`); the bytes drive the @sha. (The GLB fixtures START
        // WITH THE `glTF` MAGIC: the visual passthrough is content-checked, and magic-less visual bytes
        // are treated as a `.gltf` to pack, see tests/gltf_pack.rs.)
        let chassis_glb = b"glTF-CHASSIS-BYTES-deterministic-0001".to_vec();
        let rotor_stl = b"solid box\nfacet ... deterministic collision\nendsolid\n".to_vec();
        std::fs::write(dir.join("assets/chassis.glb"), &chassis_glb).unwrap();
        std::fs::write(dir.join("motors/assets/rotor.stl"), &rotor_stl).unwrap();

        // MIXED-CASE extension fixtures (.GLB visual, .STL collision) that PIN the extension-casing
        // parity: the vendored entry name + rerooted @uri must keep the original case (Chassis_<sha>.GLB,
        // not .glb) exactly like Python, NOT be lowercased. Distinct bytes -> distinct sha so the
        // content-addressed names are unique alongside the lowercase ones.
        let arm_glb = b"glTF-ARM-BYTES-deterministic-uppercase-ext".to_vec();
        let bracket_stl =
            b"solid arm-bracket\nfacet ... uppercase ext collision\nendsolid\n".to_vec();
        std::fs::write(dir.join("assets/Arm.GLB"), &arm_glb).unwrap();
        std::fs::write(dir.join("motors/assets/Bracket.STL"), &bracket_stl).unwrap();

        std::fs::write(
            dir.join("motors/motor.hcdf"),
            r#"<hcdf name="motor" body-frame="FLU" world-frame="ENU">
  <comp name="rotor">
    <collision name="rcol"><geometry><mesh uri="assets/rotor.stl"/></geometry></collision>
    <collision name="bcol"><geometry><mesh uri="assets/Bracket.STL"/></geometry></collision>
  </comp>
</hcdf>
"#,
        )
        .unwrap();

        let root = dir.join("robot.hcdf");
        std::fs::write(
            &root,
            r#"<hcdf name="robot" body-frame="FLU" world-frame="ENU">
  <comp name="chassis">
    <visual name="chassis_vis"><model uri="assets/chassis.glb"/></visual>
    <visual name="arm_vis"><model uri="assets/Arm.GLB"/></visual>
  </comp>
  <include uri="motors/motor.hcdf" name="left"/>
  <include uri="motors/motor.hcdf" name="right"/>
</hcdf>
"#,
        )
        .unwrap();
        Fixture { dir, root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn unique_dir(prefix: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!("{prefix}{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Map a zip's contents to (name -> bytes) in archive order, plus the ordered name list.
fn zip_contents(path: &Path) -> (Vec<String>, BTreeMap<String, Vec<u8>>) {
    let bytes = zip_store::read_file_bytes(path).unwrap();
    let entries = zip_store::read_stored(&bytes).unwrap();
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let map: BTreeMap<String, Vec<u8>> = entries.into_iter().map(|e| (e.name, e.data)).collect();
    (names, map)
}

#[test]
fn pack_zip_is_root_first_all_stored_with_vendored_assets() {
    let fx = Fixture::new("zip");
    let out = fx.dir.join("robot.hcdfz");
    let opts = PackOptions::default();
    let m = bundle::pack(&fx.root, &out, &opts).expect("pack");

    assert_eq!(m.format, "hcdfz");
    assert!(out.is_file());

    let (names, map) = zip_contents(&out);
    // root .hcdf is the FIRST entry, by contract
    assert!(
        names[0].ends_with(".hcdf"),
        "first entry is the root .hcdf: {names:?}"
    );
    assert_eq!(names[0], "robot.hcdf");
    // every other entry is under assets/
    assert!(
        names[1..].iter().all(|n| n.starts_with("assets/")),
        "non-root entries under assets/: {names:?}"
    );
    // the visual GLBs + the (deduped, two includes share each mesh) collision STLs are vendored in.
    // Two visuals (chassis.glb + Arm.GLB) -> 2 GLBs; two collisions per include (rotor.stl + Bracket.STL),
    // each shared by both includes -> deduped to 2 STLs. Counting is case-INSENSITIVE on the extension so a
    // case regression (.GLB lowercased to .glb) would still be counted here; the parity test pins the case.
    let glb = names
        .iter()
        .filter(|n| n.to_lowercase().ends_with(".glb"))
        .count();
    let stl = names
        .iter()
        .filter(|n| n.to_lowercase().ends_with(".stl"))
        .count();
    assert_eq!(
        glb, 2,
        "two distinct vendored GLBs (lowercase + uppercase ext)"
    );
    assert_eq!(
        stl, 2,
        "two distinct collision STLs, each deduped across the two includes"
    );

    // The mixed-case-extension assets keep their ORIGINAL-case extension in the vendored entry name
    // (Arm_<sha>.GLB / Bracket_<sha>.STL), matching Python, NOT lowercased to .glb/.stl.
    assert!(
        names
            .iter()
            .any(|n| n.ends_with(".GLB") && n.starts_with("assets/Arm_")),
        "uppercase-ext visual keeps its case: {names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n.ends_with(".STL") && n.starts_with("assets/Bracket_")),
        "uppercase-ext collision keeps its case: {names:?}"
    );

    let root_xml = String::from_utf8(map[&names[0]].clone()).unwrap();
    assert!(
        !root_xml.contains("<include"),
        "flatten-only: no <include> survives"
    );
    assert!(
        root_xml.contains("uri=\"assets/"),
        "document-relative asset uris"
    );
    assert!(
        root_xml.contains("sha=\"sha256:"),
        "@sha stamped on vendored meshes"
    );
    // The rerooted @uri keeps the original-case extension too (not just the entry name).
    assert!(
        root_xml.contains(".GLB\""),
        "rerooted @uri keeps uppercase .GLB: {root_xml}"
    );
    assert!(
        root_xml.contains(".STL\""),
        "rerooted @uri keeps uppercase .STL: {root_xml}"
    );
    // the prefixed sub-assembly comps from both includes are present
    assert!(root_xml.contains("left/rotor"));
    assert!(root_xml.contains("right/rotor"));

    // verify() passes on the clean bundle
    let rep = bundle::verify(&out);
    assert!(rep.ok, "verify clean bundle: {:?}", rep.issues);
}

#[test]
fn pack_zip_is_byte_deterministic() {
    // Packing the same fixture twice yields BYTE-IDENTICAL .hcdfz output (stored entries, fixed order,
    // no timestamps): the determinism the sha-pinned ecosystem relies on, and the guard that the
    // in-memory vendor path emits exactly the bytes the staged path did.
    let fx = Fixture::new("determinism");
    let out1 = fx.dir.join("a.hcdfz");
    let out2 = fx.dir.join("b.hcdfz");
    bundle::pack(&fx.root, &out1, &PackOptions::default()).unwrap();
    bundle::pack(&fx.root, &out2, &PackOptions::default()).unwrap();
    let a = std::fs::read(&out1).unwrap();
    let b = std::fs::read(&out2).unwrap();
    assert_eq!(a, b, "same input must pack to byte-identical bundles");
}

#[test]
fn open_bundle_resolves_every_mesh_under_root_dir() {
    let fx = Fixture::new("open");
    let out = fx.dir.join("robot.hcdfz");
    bundle::pack(&fx.root, &out, &PackOptions::default()).unwrap();

    let OpenedBundle {
        doc,
        root_dir,
        keep_live,
    } = bundle::open_bundle(&out).unwrap();
    assert!(root_dir.is_temp(), "a zip extracts to an owned tempdir");
    assert!(!keep_live, "a flat bundle is not keep-live");
    let mut uris = Vec::new();
    for c in &doc.comp {
        for v in &c.visual {
            if let hcdformat::model::VisualAppearance::Model { model, .. } = &v.appearance {
                if let Some(u) = &model.uri {
                    uris.push(u.clone());
                }
            }
        }
        for col in &c.collision {
            if let Some(m) = col.geometry.as_ref().and_then(|g| g.mesh.as_ref()) {
                if let Some(u) = &m.uri {
                    uris.push(u.clone());
                }
            }
        }
    }
    assert!(!uris.is_empty());
    assert!(uris.iter().all(|u| u.starts_with("assets/")));
    assert!(
        uris.iter().all(|u| root_dir.path().join(u).is_file()),
        "every mesh @uri resolves to a real file under root_dir"
    );
    // `root_dir` (an owned tempdir guard) removes the extraction dir when it drops at scope end.
}

#[test]
fn opened_bundle_guard_removes_the_owned_extraction_dir_on_drop() {
    // A .hcdfz opens into an OWNED extraction tempdir; dropping the OpenedBundle (its BundleDir guard)
    // removes that directory, so a read-only open never leaks a tempdir.
    let fx = Fixture::new("guard-drop");
    let out = fx.dir.join("robot.hcdfz");
    bundle::pack(&fx.root, &out, &PackOptions::default()).unwrap();

    let opened = bundle::open_bundle(&out).unwrap();
    assert!(
        opened.root_dir.is_temp(),
        "a zip open owns its extraction dir"
    );
    let extracted = opened.root_dir.path().to_path_buf();
    assert!(extracted.is_dir(), "the extraction dir exists while opened");
    drop(opened);
    assert!(
        !extracted.exists(),
        "the guard removed the owned dir on drop"
    );
}

#[test]
fn opened_bundle_keep_preserves_the_extraction_dir() {
    // keep()/into_path() defuses the guard so a caller can retain the extracted meshes on disk (the
    // dendrite DocDir case): the directory then outlives the OpenedBundle.
    let fx = Fixture::new("guard-keep");
    let out = fx.dir.join("robot.hcdfz");
    bundle::pack(&fx.root, &out, &PackOptions::default()).unwrap();

    let opened = bundle::open_bundle(&out).unwrap();
    let kept = opened.root_dir.keep(); // defuse: the caller now owns the dir
    assert!(
        kept.is_dir(),
        "keep() preserved the extraction dir past the guard"
    );
    // The caller is responsible for cleanup now that the guard is defused.
    let _ = std::fs::remove_dir_all(&kept);
}

#[test]
fn dir_bundle_is_loose_root_and_assets() {
    let fx = Fixture::new("dir");
    let out = fx.dir.join("robot_bundle");
    let opts = PackOptions {
        as_dir: true,
        ..Default::default()
    };
    let m = bundle::pack(&fx.root, &out, &opts).unwrap();
    assert_eq!(m.format, "dir");
    assert!(out.join("robot.hcdf").is_file(), "loose root .hcdf");
    assert!(out.join("assets").is_dir(), "loose assets/ dir");
    assert!(bundle::verify(&out).ok, "verify loose dir bundle");

    // open in place (no tempdir)
    let opened = bundle::open_bundle(&out).unwrap();
    assert!(!opened.root_dir.is_temp());
    assert_eq!(opened.root_dir.path(), out.as_path());
}

#[test]
fn open_bundle_rejects_a_byte_flipped_zip_as_corrupted() {
    // A .hcdfz whose stored bytes were flipped after packing is rejected at open (CRC-32 mismatch),
    // surfacing corruption with a precise message instead of a downstream parse surprise.
    use hcdformat::zip_store::{self, ZipEntry};
    let mut bytes = Vec::new();
    zip_store::write_stored(
        &mut bytes,
        &[ZipEntry {
            name: "root.hcdf",
            data: b"<hcdf name=\"x\"/>",
        }],
    )
    .unwrap();
    // Flip the first data byte (after the 30-byte local header + the 9-byte name) to corrupt the blob.
    let data_off = 30 + "root.hcdf".len();
    bytes[data_off] ^= 0xFF;

    let dir = unique_dir("hcdf-corrupt-");
    let out = dir.join("root.hcdfz");
    std::fs::write(&out, &bytes).unwrap();
    let err = match bundle::open_bundle(&out) {
        Ok(_) => panic!("a byte-flipped bundle must not open"),
        Err(e) => e,
    };
    assert!(
        err.contains("corrupted"),
        "corruption surfaced at open: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_catches_tampered_blob_and_flipped_sha() {
    let fx = Fixture::new("tamper");
    let out = fx.dir.join("robot.hcdfz");
    bundle::pack(&fx.root, &out, &PackOptions::default()).unwrap();

    // tamper a blob: append garbage to the STL entry, re-pack a STORED zip with the same names.
    let (names, mut map) = zip_contents(&out);
    let stl_name = names.iter().find(|n| n.ends_with(".stl")).unwrap().clone();
    map.get_mut(&stl_name)
        .unwrap()
        .extend_from_slice(b"\x00garbage");
    let tampered = fx.dir.join("tampered.hcdfz");
    rewrite_zip(&tampered, &names, &map);
    let rep = bundle::verify(&tampered);
    assert!(
        !rep.ok && rep.issues.iter().any(|i| i.to_lowercase().contains("sha")),
        "tampered blob -> a sha issue: {:?}",
        rep.issues
    );

    // flip an @sha in the root doc (keep the blob) -> @sha no longer matches the content.
    let (names2, mut map2) = zip_contents(&out);
    let root = &names2[0];
    let root_xml = String::from_utf8(map2[root].clone()).unwrap();
    let flipped_xml = root_xml.replacen("sha=\"sha256:", "sha=\"sha256:dead", 1);
    map2.insert(root.clone(), flipped_xml.into_bytes());
    let flipped = fx.dir.join("flipped.hcdfz");
    rewrite_zip(&flipped, &names2, &map2);
    let rep = bundle::verify(&flipped);
    assert!(
        !rep.ok && !rep.issues.is_empty(),
        "flipped @sha -> issue: {:?}",
        rep.issues
    );
}

#[test]
fn remote_uri_errors_by_default_and_kept_with_keep_policy() {
    // A doc with a remote visual mesh: default policy (Error) must refuse; Keep must leave it live.
    let dir = unique_dir("hcdf-bundle-remote-");
    let root = dir.join("r.hcdf");
    std::fs::write(
        &root,
        r#"<hcdf name="r" body-frame="FLU">
  <comp name="c"><visual name="v"><model uri="https://example.com/x.glb"/></visual></comp>
</hcdf>
"#,
    )
    .unwrap();

    // default (Error): explicit error naming the remote site
    let err = bundle::pack(&root, &dir.join("r.hcdfz"), &PackOptions::default()).unwrap_err();
    assert!(
        err.contains("remote") && err.contains("--vendor-remote"),
        "error names the choice: {err}"
    );

    // Keep: leaves the live uri; verify skips it (no local blob to hash)
    let keep = PackOptions {
        remote: RemotePolicy::Keep,
        ..Default::default()
    };
    let out = dir.join("r_keep.hcdfz");
    let m = bundle::pack(&root, &out, &keep).unwrap();
    assert!(!m.kept_remote.is_empty(), "kept_remote populated");
    let (names, map) = zip_contents(&out);
    let root_xml = String::from_utf8(map[&names[0]].clone()).unwrap();
    assert!(
        root_xml.contains("https://example.com/x.glb"),
        "live uri kept in the bundle"
    );
    assert!(bundle::verify(&out).ok, "verify skips a kept-remote mesh");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "remote")]
#[test]
fn vendor_remote_fetches_embeds_and_strips_http_uris() {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread;

    // A tiny server that serves a deterministic "GLB" body for the remote visual mesh.
    let payload = b"glTF-REMOTE-GLB-BYTES-deterministic".to_vec();
    let payload2 = payload.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            // drain request headers
            let mut buf = [0u8; 1];
            let mut last4 = [0u8; 4];
            let mut n = 0;
            while n < 65536 {
                match std::io::Read::read(&mut s, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        last4 = [last4[1], last4[2], last4[3], buf[0]];
                        n += 1;
                        if &last4 == b"\r\n\r\n" {
                            break;
                        }
                    }
                }
            }
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                payload2.len()
            );
            let _ = s.write_all(hdr.as_bytes());
            let _ = s.write_all(&payload2);
        }
    });

    let dir = unique_dir("hcdf-vendor-remote-");
    let url = format!("http://127.0.0.1:{port}/rotor.glb");
    let root = dir.join("r.hcdf");
    std::fs::write(
        &root,
        format!(
            r#"<hcdf name="r" body-frame="FLU">
  <comp name="c"><visual name="v"><model uri="{url}"/></visual></comp>
</hcdf>
"#
        ),
    )
    .unwrap();

    let cache = dir.join("cache");
    let opts = PackOptions {
        remote: RemotePolicy::Vendor,
        cache_dir: Some(cache),
        ..Default::default()
    };
    let out = dir.join("r.hcdfz");
    let m = bundle::pack(&root, &out, &opts).expect("vendor-remote pack");
    assert!(
        m.kept_remote.is_empty(),
        "a vendor-remote bundle keeps NO live uri"
    );

    let (names, map) = zip_contents(&out);
    let root_xml = String::from_utf8(map[&names[0]].clone()).unwrap();
    // No http(s) uri survives: self-contained/offline bundle.
    assert!(
        !root_xml.contains("http://"),
        "vendor-remote bundle has no http uri: {root_xml}"
    );
    assert!(
        root_xml.contains("uri=\"assets/"),
        "remote mesh vendored to assets/"
    );
    // The vendored blob's bytes are exactly what the server served (content-addressed).
    let glb = names
        .iter()
        .find(|n| n.ends_with(".glb"))
        .expect("a vendored glb entry");
    assert_eq!(
        map[glb], payload,
        "embedded bytes are the fetched remote content"
    );
    assert!(
        bundle::verify(&out).ok,
        "vendor-remote bundle verifies clean"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Helper: write a STORED zip from an ordered name list + a content map.
fn rewrite_zip(path: &Path, names: &[String], map: &BTreeMap<String, Vec<u8>>) {
    let entries: Vec<zip_store::ZipEntry> = names
        .iter()
        .map(|n| zip_store::ZipEntry {
            name: n,
            data: &map[n],
        })
        .collect();
    let mut f = std::fs::File::create(path).unwrap();
    zip_store::write_stored(&mut f, &entries).unwrap();
}

// ── BUNDLE STRUCTURE GOLDENS (frozen; the Rust bundle::pack output is the source of truth) ────────

/// The crate-local frozen-golden directory (`<crate>/tests/golden`).
fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Compare `actual` to the committed golden at `tests/golden/<rel>`; when `HCDF_REGEN_GOLDENS` is set,
/// (re)write the golden instead of asserting. `bundle::pack` is deterministic + content-addressed, so
/// its structure freezes cleanly.
fn assert_golden(rel: &str, actual: &str) {
    let path = golden_dir().join(rel);
    if std::env::var_os("HCDF_REGEN_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden {} ({e}); regenerate with HCDF_REGEN_GOLDENS=1",
            path.display()
        )
    });
    assert_eq!(actual, expected, "golden mismatch for {}", path.display());
}

/// Remove the non-deterministic `source-uri="..."` attribute (the ORIGINAL absolute path of a vendored
/// asset, which lives under a per-run temp dir) so the projection is path-independent and freezes cleanly.
fn strip_source_uri(doc: &str) -> String {
    const KEY: &str = " source-uri=\"";
    let mut out = String::with_capacity(doc.len());
    let mut rest = doc;
    while let Some(p) = rest.find(KEY) {
        out.push_str(&rest[..p]);
        rest = &rest[p + KEY.len()..];
        match rest.find('"') {
            Some(end) => rest = &rest[end + 1..],
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// The frozen text projection of a bundle: the ordered entry-name list, then the root `.hcdf` doc (with
/// the volatile `source-uri` stripped). The content-addressed asset names encode their bytes (name =
/// `<stem>_<sha12>.<ext>`), so identical names imply identical bytes; the root doc carries the rerooted
/// `@uri`/`@sha` and the flattened comp set.
fn bundle_projection(names: &[String], root_doc: &str) -> String {
    format!(
        "{}\n===ROOT===\n{}",
        names.join("\n"),
        strip_source_uri(root_doc)
    )
}

#[test]
fn bundle_zip_matches_frozen_golden() {
    let fx = Fixture::new("parity-zip");
    let rs_out = fx.dir.join("rs.hcdfz");
    bundle::pack(&fx.root, &rs_out, &PackOptions::default()).unwrap();

    let (names, map) = zip_contents(&rs_out);
    assert!(
        names[0].ends_with(".hcdf"),
        "root .hcdf is the first entry: {names:?}"
    );
    let root = String::from_utf8(map[&names[0]].clone()).unwrap();
    assert!(
        !root.contains("<include"),
        "flatten-only bundle keeps no <include>"
    );
    assert_golden("bundle/parity-zip.txt", &bundle_projection(&names, &root));
    assert!(bundle::verify(&rs_out).ok, "bundle verifies clean");
}

#[test]
fn bundle_dir_matches_frozen_golden() {
    let fx = Fixture::new("parity-dir");
    let rs_out = fx.dir.join("rs_dir");
    let opts = PackOptions {
        as_dir: true,
        ..Default::default()
    };
    bundle::pack(&fx.root, &rs_out, &opts).unwrap();

    let files = list_rel(&rs_out);
    let root_rel = files
        .iter()
        .find(|r| r.ends_with(".hcdf") && !r.contains('/'))
        .expect("a top-level root .hcdf in the loose bundle");
    let root = std::fs::read_to_string(rs_out.join(root_rel)).unwrap();
    assert_golden("bundle/parity-dir.txt", &bundle_projection(&files, &root));
    assert!(bundle::verify(&rs_out).ok, "dir bundle verifies clean");
}

/// Sorted relative file list under a directory (recursive).
fn list_rel(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(base: &Path, cur: &Path, out: &mut Vec<String>) {
        if let Ok(rd) = std::fs::read_dir(cur) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(base, &p, out);
                } else {
                    out.push(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
                }
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

// ── KEEP-LIVE (non-flattening) bundle: native pack + open + live off-disk flatten ────────────────

/// A keep-live `.hcdfz` (produced by the shared in-memory `pack_to_bytes_keep_live`) written to disk and
/// opened via the native `open_bundle` must (a) be flagged `keep_live`, (b) extract its `modules/<sha>/…`
/// entries alongside the root, and (c) resolve its SURVIVING `<include>`s LIVE off the tempdir when
/// flattened, proving a keep-live bundle round-trips editable on native.
#[test]
fn native_keep_live_round_trip_resolves_includes_off_disk() {
    use hcdformat::model::{Comp, Hcdf, Include, ModelRef, Visual, VisualAppearance};
    use hcdformat::{pack_to_bytes_keep_live, KeepLiveModule, MemBundle};

    // Root: one comp + a live <include> of a module (no root-local meshes).
    let mut root = Hcdf {
        name: "assembly".to_string(),
        ..Default::default()
    };
    root.comp.push(Comp {
        name: "base".to_string(),
        ..Default::default()
    });
    root.include.push(Include {
        uri: Some("/mem/0/m.hcdf".to_string()),
        name: Some("left".to_string()),
        sha: None,
        pose: None,
        ..Default::default()
    });

    // Module: one comp with a visual mesh (module-relative uri).
    let mut module = Hcdf {
        name: "wheelmod".to_string(),
        ..Default::default()
    };
    let mut comp = Comp {
        name: "rotor".to_string(),
        ..Default::default()
    };
    comp.visual.push(Visual {
        name: "v0".to_string(),
        appearance: VisualAppearance::Model {
            model: ModelRef {
                uri: Some("assets/wheel.glb".to_string()),
                sha: None,
                ..Default::default()
            },
            geometry: None,
        },
        ..Default::default()
    });
    module.comp.push(comp);

    let km = KeepLiveModule {
        include_uri: "/mem/0/m.hcdf".to_string(),
        doc: module,
        meshes: vec![("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec())],
    };
    let (bytes, _report) =
        pack_to_bytes_keep_live(&root, &MemBundle::new(), &[km], false).expect("keep-live packs");

    let dir = unique_dir("hcdf-keeplive-");
    let out = dir.join("assembly.hcdfz");
    std::fs::write(&out, &bytes).unwrap();

    let opened = bundle::open_bundle(&out).expect("opens");
    assert!(opened.keep_live, "detected as keep-live");
    assert_eq!(
        opened.doc.include.len(),
        1,
        "the <include> survives the round-trip"
    );

    // The module file + its assets landed under modules/<sha>/ in the tempdir.
    let rels = list_rel(opened.root_dir.path());
    assert!(
        rels.iter()
            .any(|r| r.starts_with("modules/") && r.ends_with(".hcdf")),
        "module doc extracted: {rels:?}"
    );

    // Flatten the extracted root LIVE off disk: the include resolves and the module comp merges in.
    let mut doc = opened.doc.clone();
    let notes =
        hcdformat::flatten(&mut doc, opened.root_dir.path()).expect("flattens live off disk");
    assert!(doc.include.is_empty(), "include resolved live: {notes:?}");
    assert!(
        doc.comp.iter().any(|c| c.name == "left/rotor"),
        "module comp resolved live off disk"
    );

    // The extraction tempdir is removed when `opened` (its BundleDir guard) drops.
    drop(opened);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── BYTE-BASED (in-memory) parity: the `pack_to_bytes` / `open_bundle_bytes` / `verify_bytes` flows the
//    binding exposes must agree with the path-based `bundle::pack` / `open_bundle` / `verify` ────────────

/// A minimal FLAT parity fixture: `robot.hcdf` (name "robot") with one comp carrying a visual GLB and a
/// collision STL, both LOCAL relative uris with LOWERCASE extensions whose last path segment IS the file
/// basename. Under these (corpus-typical) conditions the disk vendor's source-path naming and the
/// in-memory uri naming coincide, so `bundle::pack` and `pack_to_bytes` content-address to the SAME
/// `assets/<stem>_<sha12>.<ext>` names and the archives are byte-identical. The GLB carries the `glTF`
/// magic so both lanes pass it through verbatim (no `.gltf` packing divergence); the STL passes through too.
struct FlatFixture {
    dir: PathBuf,
    root: PathBuf,
    chassis_glb: Vec<u8>,
    col_stl: Vec<u8>,
}

impl FlatFixture {
    fn new(tag: &str) -> Self {
        let dir = unique_dir(&format!("hcdf-bytes-parity-{tag}-"));
        std::fs::create_dir_all(dir.join("mesh")).unwrap();
        let chassis_glb = b"glTF-CHASSIS-deterministic-passthrough-0001".to_vec();
        let col_stl = b"solid col\nfacet ... deterministic collision\nendsolid\n".to_vec();
        std::fs::write(dir.join("mesh/chassis.glb"), &chassis_glb).unwrap();
        std::fs::write(dir.join("mesh/col.stl"), &col_stl).unwrap();
        let root = dir.join("robot.hcdf");
        std::fs::write(
            &root,
            r#"<hcdf name="robot" body-frame="FLU" world-frame="ENU">
  <comp name="chassis">
    <visual name="v"><model uri="mesh/chassis.glb"/></visual>
    <collision name="c"><geometry><mesh uri="mesh/col.stl"/></geometry></collision>
  </comp>
</hcdf>
"#,
        )
        .unwrap();
        FlatFixture {
            dir,
            root,
            chassis_glb,
            col_stl,
        }
    }

    /// The in-memory asset map (uri AS WRITTEN in the doc -> its bytes) mirroring the on-disk meshes.
    fn mem_assets(&self) -> MemBundle {
        let mut m = MemBundle::new();
        m.insert("mesh/chassis.glb".to_string(), self.chassis_glb.clone());
        m.insert("mesh/col.stl".to_string(), self.col_stl.clone());
        m
    }
}

impl Drop for FlatFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn pack_to_bytes_matches_disk_pack_byte_for_byte() {
    let fx = FlatFixture::new("pack");
    // Path-based pack -> a .hcdfz on disk.
    let out = fx.dir.join("robot.hcdfz");
    bundle::pack(&fx.root, &out, &PackOptions::default()).expect("disk pack");
    let disk_bytes = std::fs::read(&out).unwrap();

    // Byte-based pack -> the SAME document (parsed from the same file) + the SAME mesh bytes keyed by uri.
    let doc = Hcdf::from_xml_str(&std::fs::read_to_string(&fx.root).unwrap()).unwrap();
    let (mem_bytes, _report) = pack_to_bytes(&doc, &fx.mem_assets(), false).expect("mem pack");

    assert_eq!(
        disk_bytes, mem_bytes,
        "in-memory pack_to_bytes must produce the byte-identical .hcdfz the path-based pack writes"
    );
    // The archive both produced verifies clean under both verifiers.
    assert!(bundle::verify(&out).ok, "disk verify clean");
    assert!(verify_bytes(&mem_bytes).ok, "byte verify clean");
}

#[test]
fn open_bundle_bytes_matches_disk_open() {
    let fx = Fixture::new("bytes-open");
    let out = fx.dir.join("robot.hcdfz");
    bundle::pack(&fx.root, &out, &PackOptions::default()).unwrap();
    let bytes = std::fs::read(&out).unwrap();

    // Path-based open extracts to a tempdir; byte-based open returns the tree in memory.
    let OpenedBundle {
        doc: disk_doc,
        root_dir,
        ..
    } = bundle::open_bundle(&out).unwrap();
    let mem = open_bundle_bytes(&bytes).unwrap();

    // Same root document (compare serialized, since Hcdf carries floats and is not Eq).
    assert_eq!(
        disk_doc.to_xml_string().unwrap(),
        mem.doc.to_xml_string().unwrap(),
        "byte-open root doc equals the path-open root doc"
    );
    // A flat bundle surfaces no keep-live modules; its in-memory asset tree is exactly the files the
    // path-open extracted under root_dir, same relative names and same bytes.
    assert!(
        mem.modules.is_empty(),
        "a flat bundle surfaces no keep-live modules"
    );
    assert!(
        !mem.assets.is_empty(),
        "the flat bundle carries vendored assets"
    );
    for (name, data) in &mem.assets {
        let on_disk = std::fs::read(root_dir.path().join(name)).unwrap();
        assert_eq!(
            &on_disk, data,
            "asset {name} bytes match the extracted file"
        );
    }
    // Every extracted non-root file is present in the byte-open asset tree (and vice versa).
    let mem_names: std::collections::BTreeSet<&str> =
        mem.assets.iter().map(|(n, _)| n.as_str()).collect();
    for rel in list_rel(root_dir.path()) {
        if rel.ends_with(".hcdf") {
            continue; // the root doc, surfaced as mem.doc
        }
        assert!(
            mem_names.contains(rel.as_str()),
            "extracted {rel} present in the byte-open tree"
        );
    }
}

#[test]
fn verify_bytes_matches_disk_verify() {
    let fx = Fixture::new("bytes-verify");
    let out = fx.dir.join("robot.hcdfz");
    bundle::pack(&fx.root, &out, &PackOptions::default()).unwrap();
    let bytes = std::fs::read(&out).unwrap();

    // Clean bundle: both verifiers agree it is OK, with identical issue lists.
    let disk = bundle::verify(&out);
    let mem = verify_bytes(&bytes);
    assert!(
        disk.ok && mem.ok,
        "clean bundle verifies: {:?} / {:?}",
        disk.issues,
        mem.issues
    );
    assert_eq!(disk.issues, mem.issues, "clean verify issue lists agree");

    // Tampered blob: append garbage to the STL entry, re-pack a STORED zip with the same names; both
    // verifiers must catch it with the SAME issue list.
    let (names, mut map) = zip_contents(&out);
    let stl = names.iter().find(|n| n.ends_with(".stl")).unwrap().clone();
    map.get_mut(&stl).unwrap().extend_from_slice(b"\x00garbage");
    let tampered_path = fx.dir.join("tampered.hcdfz");
    rewrite_zip(&tampered_path, &names, &map);
    let tampered_bytes = std::fs::read(&tampered_path).unwrap();

    let disk_t = bundle::verify(&tampered_path);
    let mem_t = verify_bytes(&tampered_bytes);
    assert!(!disk_t.ok && !mem_t.ok, "tamper caught by both verifiers");
    assert_eq!(
        disk_t.issues, mem_t.issues,
        "tampered verify issue lists agree"
    );
}

/// A freshly-packed keep-live bundle must `verify` CLEAN (its stamped include `@sha`s match the embedded
/// module bytes; the module meshes hash to their `@sha`), and mutating an embedded module `.hcdf` byte
/// must be CAUGHT as an include-sha mismatch: the load-bearing "the pin pins the module" guarantee.
#[test]
fn keep_live_verify_clean_and_catches_tampered_module() {
    use hcdformat::model::{Comp, Hcdf, Include, ModelRef, Visual, VisualAppearance};
    use hcdformat::{pack_to_bytes_keep_live, KeepLiveModule, MemBundle};

    let mut root = Hcdf {
        name: "assembly".to_string(),
        ..Default::default()
    };
    root.comp.push(Comp {
        name: "base".to_string(),
        ..Default::default()
    });
    root.include.push(Include {
        uri: Some("/mem/0/m.hcdf".to_string()),
        name: Some("left".to_string()),
        sha: None,
        pose: None,
        ..Default::default()
    });
    let mut module = Hcdf {
        name: "wheelmod".to_string(),
        ..Default::default()
    };
    let mut comp = Comp {
        name: "rotor".to_string(),
        ..Default::default()
    };
    comp.visual.push(Visual {
        name: "v0".to_string(),
        appearance: VisualAppearance::Model {
            model: ModelRef {
                uri: Some("assets/wheel.glb".to_string()),
                sha: None,
                ..Default::default()
            },
            geometry: None,
        },
        ..Default::default()
    });
    module.comp.push(comp);
    let km = KeepLiveModule {
        include_uri: "/mem/0/m.hcdf".to_string(),
        doc: module,
        meshes: vec![("assets/wheel.glb".to_string(), b"GLB-BYTES".to_vec())],
    };
    let (bytes, _r) =
        pack_to_bytes_keep_live(&root, &MemBundle::new(), &[km], false).expect("keep-live packs");

    let dir = unique_dir("hcdf-keeplive-verify-");
    let out = dir.join("assembly.hcdfz");
    std::fs::write(&out, &bytes).unwrap();

    // Clean bundle verifies OK.
    let rep = bundle::verify(&out);
    assert!(
        rep.ok,
        "fresh keep-live bundle must verify clean: {:?}",
        rep.issues
    );

    // TAMPER: extract, mutate the embedded module doc, repack, and verify; the include @sha must catch it.
    let entries = zip_store::read_stored(&bytes).unwrap();
    let mut tampered: Vec<(String, Vec<u8>)> = Vec::new();
    for e in &entries {
        let mut data = e.data.clone();
        if e.name.starts_with("modules/") && e.name.ends_with(".hcdf") {
            // Rename the module's comp inside the XML → different content sha, same file position.
            let xml = String::from_utf8(data).unwrap().replace("rotor", "rotorX");
            data = xml.into_bytes();
        }
        tampered.push((e.name.clone(), data));
    }
    let refs: Vec<zip_store::ZipEntry> = tampered
        .iter()
        .map(|(n, d)| zip_store::ZipEntry { name: n, data: d })
        .collect();
    let mut tbuf = Vec::new();
    zip_store::write_stored(&mut tbuf, &refs).unwrap();
    let tout = dir.join("tampered.hcdfz");
    std::fs::write(&tout, &tbuf).unwrap();
    let rep = bundle::verify(&tout);
    assert!(!rep.ok, "a tampered embedded module must fail verify");
    assert!(
        rep.issues
            .iter()
            .any(|i| i.contains("@sha does not match embedded module")),
        "the include-sha mismatch must be reported: {:?}",
        rep.issues
    );

    let _ = std::fs::remove_dir_all(&dir);
}
