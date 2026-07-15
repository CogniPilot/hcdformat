//! SECURITY VECTORS for the hardened remote fetcher ([`hcdformat::remote::fetch_remote`]), mirroring
//! the Python `tests` for `hcdf_io/remote.py`. Each test stands up a tiny in-process HTTP server (raw
//! `std::net::TcpListener` in a thread, no extra deps) that emits a crafted response, then asserts the
//! fetcher's guard fires:
//!
//!   * `http -> ftp://`  redirect is REFUSED (not followed): SSRF guard, per hop.
//!   * `http -> file://` redirect is REFUSED: local-file read primitive denied across redirects.
//!   * a body over the size cap is ABORTED (declared `Content-Length` over cap, AND streamed-over-cap).
//!   * a slow server TIMES OUT (the request carries a bounded timeout).
//!   * `expect_sha` mismatch is REJECTED (bytes discarded, nothing cached).
//!   * a cache HIT whose cached file was TAMPERED is RE-VERIFIED + overwritten (no cache poisoning).
//!   * the scheme allow-list rejects a non-http(s) INITIAL url before any I/O.
//!
//! Only built with the `remote` feature (the fetcher only exists there) and never on wasm.

#![cfg(all(feature = "remote", not(target_arch = "wasm32")))]

use hcdformat::remote::{fetch_remote, RemoteError};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// A one-shot test server: binds 127.0.0.1:0, hands each accepted connection to `handler`, and
/// returns its `http://127.0.0.1:<port>/` base url. The thread serves until the process ends (tests
/// are short-lived); each test uses its own server so there is no cross-talk.
fn serve<F>(handler: F) -> String
where
    F: Fn(TcpStream) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => handler(s),
                Err(_) => break,
            }
        }
    });
    format!("http://127.0.0.1:{}/", addr.port())
}

/// Read and discard the request line + headers (until the blank line) so the client's write completes.
fn drain_request(stream: &mut TcpStream) {
    let mut buf = [0u8; 1];
    let mut last4 = [0u8; 4];
    let mut n = 0usize;
    // Read until "\r\n\r\n"; cap at 64 KiB to avoid hanging on a malformed request.
    while n < 64 * 1024 {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {
                last4 = [last4[1], last4[2], last4[3], buf[0]];
                n += 1;
                if &last4 == b"\r\n\r\n" {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn unique_cache(tag: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "hcdf-remote-test-{tag}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn rejects_non_http_initial_scheme_before_io() {
    // No server needed: the allow-list rejects file:// before any I/O.
    let cache = unique_cache("scheme");
    for bad in [
        "file:///etc/passwd",
        "ftp://host/x",
        "data:text/plain,hi",
        "package://p/m.glb",
    ] {
        let err = fetch_remote(bad, None, Some(&cache), None, None).unwrap_err();
        assert!(
            matches!(err, RemoteError::BadScheme { .. }),
            "{bad} -> {err:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn redirect_to_ftp_is_refused() {
    let base = serve(|mut s| {
        drain_request(&mut s);
        let body =
            "HTTP/1.1 302 Found\r\nLocation: ftp://internal/secret\r\nContent-Length: 0\r\n\r\n";
        let _ = s.write_all(body.as_bytes());
    });
    let cache = unique_cache("ftp");
    let err = fetch_remote(&base, None, Some(&cache), None, None).unwrap_err();
    assert!(
        matches!(err, RemoteError::BadRedirect { .. }),
        "http->ftp redirect must be refused, got {err:?}"
    );
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn redirect_to_file_is_refused() {
    let base = serve(|mut s| {
        drain_request(&mut s);
        // The classic SSRF -> local-file-read attempt across a redirect.
        let body = "HTTP/1.1 301 Moved Permanently\r\nLocation: file:///etc/passwd\r\nContent-Length: 0\r\n\r\n";
        let _ = s.write_all(body.as_bytes());
    });
    let cache = unique_cache("file");
    let err = fetch_remote(&base, None, Some(&cache), None, None).unwrap_err();
    assert!(
        matches!(err, RemoteError::BadRedirect { .. }),
        "http->file:// redirect must be refused, got {err:?}"
    );
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn legitimate_http_to_http_redirect_is_followed() {
    // Prove the manual redirect handling is not a blanket block: an http -> http(s) redirect on the
    // SAME server is followed and the final body fetched. (The guard rejects only NON-http(s) hops.)
    let payload = b"final-redirect-target-bytes".to_vec();
    let payload2 = payload.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            drain_request(&mut s);
            // Peek which path was requested by re-reading is awkward post-drain; instead use a tiny
            // state: first connection -> 302 to /final, second -> 200 with the body. We disambiguate
            // by a shared counter.
            // Simpler: always 302 to an absolute /final unless the (already-drained) request was /final.
            // Since we drained, just alternate via a static AtomicUsize.
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::SeqCst);
            if n.is_multiple_of(2) {
                let loc = format!("http://127.0.0.1:{port}/final");
                let resp =
                    format!("HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\n\r\n");
                let _ = s.write_all(resp.as_bytes());
            } else {
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    payload2.len()
                );
                let _ = s.write_all(hdr.as_bytes());
                let _ = s.write_all(&payload2);
            }
        }
    });
    let cache = unique_cache("redirect-ok");
    let url = format!("http://127.0.0.1:{port}/start");
    let path = fetch_remote(&url, None, Some(&cache), None, None).expect("http->http redirect ok");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        payload,
        "followed redirect to the final body"
    );
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn oversize_declared_content_length_is_rejected_up_front() {
    let base = serve(|mut s| {
        drain_request(&mut s);
        // Declare a huge Content-Length (no body sent): must be rejected before reading a byte.
        let hdr = "HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n";
        let _ = s.write_all(hdr.as_bytes());
    });
    let cache = unique_cache("oversize-declared");
    // cap = 1 KiB
    let err = fetch_remote(&base, None, Some(&cache), Some(1024), None).unwrap_err();
    assert!(
        matches!(err, RemoteError::TooLarge { .. }),
        "declared oversize -> {err:?}"
    );
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn oversize_streamed_body_is_aborted_mid_download() {
    // No (or a small/omitted) Content-Length, but the streamed body exceeds the cap -> abort.
    let base = serve(|mut s| {
        drain_request(&mut s);
        // Chunked-ish: send headers with NO content-length, then a body bigger than the cap.
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
        let chunk = vec![b'A'; 64 * 1024];
        for _ in 0..40 {
            // ~2.5 MiB total
            if s.write_all(&chunk).is_err() {
                break;
            }
        }
    });
    let cache = unique_cache("oversize-stream");
    // cap = 256 KiB; the ~2.5 MiB body must be aborted past the cap.
    let err = fetch_remote(&base, None, Some(&cache), Some(256 * 1024), None).unwrap_err();
    assert!(
        matches!(err, RemoteError::TooLarge { .. }),
        "streamed oversize -> {err:?}"
    );
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn slow_server_times_out() {
    let base = serve(|mut s| {
        drain_request(&mut s);
        // Send headers, then STALL well past the timeout without finishing the body.
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n");
        thread::sleep(Duration::from_secs(5));
        let _ = s.write_all(&[b'x'; 100]);
    });
    let cache = unique_cache("slow");
    let err = fetch_remote(
        &base,
        None,
        Some(&cache),
        None,
        Some(Duration::from_millis(500)),
    )
    .unwrap_err();
    assert!(
        matches!(err, RemoteError::Network { .. }),
        "slow server -> timeout Network, got {err:?}"
    );
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn expect_sha_mismatch_is_rejected_and_nothing_cached() {
    let payload = b"the actual remote bytes".to_vec();
    let payload2 = payload.clone();
    let base = serve(move |mut s| {
        drain_request(&mut s);
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            payload2.len()
        );
        let _ = s.write_all(hdr.as_bytes());
        let _ = s.write_all(&payload2);
    });
    let cache = unique_cache("sha-mismatch");
    let url = format!("{base}rotor.glb");
    let wrong = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let err = fetch_remote(&url, Some(wrong), Some(&cache), None, None).unwrap_err();
    assert!(
        matches!(err, RemoteError::ShaMismatch { .. }),
        "sha mismatch -> {err:?}"
    );
    // Nothing was cached (the bytes were discarded).
    let cached: Vec<_> = std::fs::read_dir(&cache)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(
        cached.is_empty(),
        "no cache entry left after a sha-mismatch: {cached:?}"
    );
    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn good_fetch_caches_and_reverifies_a_tampered_hit() {
    let payload = b"GLB-bytes-content-addressed".to_vec();
    let payload2 = payload.clone();
    let base = serve(move |mut s| {
        drain_request(&mut s);
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            payload2.len()
        );
        let _ = s.write_all(hdr.as_bytes());
        let _ = s.write_all(&payload2);
    });
    let cache = unique_cache("cache-poison");
    let url = format!("{base}rotor.glb");

    // 1) A clean fetch caches the bytes content-addressed.
    let path = fetch_remote(&url, None, Some(&cache), None, None).expect("fetch ok");
    assert!(path.is_file());
    let cached = std::fs::read(&path).unwrap();
    assert_eq!(cached, payload, "cached bytes are the fetched content");

    // 2) POISON the cache file (a local attacker overwrites the predictable <sha>.glb with garbage).
    std::fs::write(&path, b"POISONED-NOT-THE-REAL-BYTES").unwrap();

    // 3) Fetch again. On the cache HIT the fetcher RE-HASHES the file; the tampered bytes no longer
    //    match the content address, so it is OVERWRITTEN with the freshly-fetched, verified bytes,
    //    never returned blind. The returned path must hold the REAL content again.
    let path2 = fetch_remote(&url, None, Some(&cache), None, None).expect("re-fetch ok");
    assert_eq!(path2, path, "same content-addressed path");
    let after = std::fs::read(&path2).unwrap();
    assert_eq!(
        after, payload,
        "poisoned cache entry was re-verified + overwritten (no poison served)"
    );

    let _ = std::fs::remove_dir_all(&cache);
}

#[test]
fn cache_is_content_addressed_one_file_per_content() {
    // Parity with Python: `fetch_remote` always reads the URL (so the server is hit each call), then
    // content-addresses; the cache DEDUPS ON DISK: the same content maps to ONE stable file path,
    // never a second/duplicate entry. (Python's `_read_capped` likewise runs before the cache check;
    // the cache saves disk, not the round-trip, and re-verifies the hit.)
    let payload = b"dedup-me".to_vec();
    let payload2 = payload.clone();
    let base = serve(move |mut s| {
        drain_request(&mut s);
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            payload2.len()
        );
        let _ = s.write_all(hdr.as_bytes());
        let _ = s.write_all(&payload2);
    });
    let cache = unique_cache("dedup");
    let url = format!("{base}m.glb");
    let p1 = fetch_remote(&url, None, Some(&cache), None, None).unwrap();
    let p2 = fetch_remote(&url, None, Some(&cache), None, None).unwrap();
    assert_eq!(p1, p2, "same content-addressed path on a repeat");
    assert_eq!(
        std::fs::read(&p1).unwrap(),
        payload,
        "cached file holds the content"
    );
    // Exactly ONE non-temp cache file exists (deduped on disk), regardless of how many fetches ran.
    let files: Vec<_> = std::fs::read_dir(&cache)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert_eq!(
        files.len(),
        1,
        "one content-addressed cache file (deduped): {files:?}"
    );
    let _ = std::fs::remove_dir_all(&cache);
}
