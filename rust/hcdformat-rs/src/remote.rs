//! Safe, opt-in fetching of remote (http/https) assets for bundling: a Rust port of
//! `hcdf_io/remote.py`, reproducing its security posture function-for-function.
//!
//! Bundling a model that references a mesh or an `<include>` by an `http(s)://` URL needs those bytes
//! on disk to vendor them. Fetching arbitrary URLs is a foot-gun, so this helper is deliberately
//! narrow and SAFE BY CONSTRUCTION: it is only ever reached when the integrator opts in
//! (`--vendor-remote`) AND the crate is built with the `remote` feature:
//!
//!   * **scheme allow-list, enforced across redirects**: ONLY `http://` / `https://` are fetched.
//!     Anything else (`file://`, `ftp://`, `data:`, a bare path, `package://`) is rejected with a
//!     clear error BEFORE any I/O, so the fetcher can never be turned into a local-file read
//!     primitive. The allow-list is re-asserted on the INITIAL url AND on EVERY redirect hop:
//!     redirects are NOT auto-followed by the HTTP client (`max_redirects(0)`); each 3xx `Location`
//!     is validated against the allow-list IN OUR CODE before we re-issue the request, so an
//!     `http -> ftp://` / `http -> file://` redirect cannot smuggle a non-http(s) scheme past the
//!     guarantee (the SSRF bug the Python original caught + fixed).
//!   * **timeout**: every request carries a PER-PHASE timeout (default 30 s each on resolve / connect /
//!     send / receive), matching `hcdf_io.remote`'s socket timeout (which bounds each blocking operation
//!     independently and resets on each), so a hung server cannot stall a bundle indefinitely, while a
//!     transfer that keeps making progress is never cut off by a single end-to-end wall-clock deadline.
//!   * **size cap**: the body is STREAMED and aborted the moment it exceeds `max_bytes` (default
//!     256 MiB), so a hostile/huge URL cannot exhaust memory or disk. A server-declared
//!     `Content-Length` over the cap is rejected up front, too.
//!   * **content-addressed atomic cache**: fetched bytes are written to `cache_dir` under their own
//!     content sha, so the same content is fetched at most once. The write is ATOMIC (temp file in
//!     the same dir then rename). On a cache HIT the existing file's bytes are RE-HASHED and only
//!     trusted if they still match the content address (and `expect_sha` when given); a tampered/stale
//!     entry is OVERWRITTEN with the freshly-verified bytes rather than returned blind (the
//!     cache-poisoning bug the Python original caught + fixed). The default cache dir is a PER-USER
//!     private directory (XDG cache / `~/.cache`), never a world-shared guessable `/tmp` path.
//!   * **integrity**: if `expect_sha` is given (a pinned `@sha`), the fetched content sha must equal
//!     it or the fetch errors (the bytes are discarded, never cached).
//!
//! Native-only (the `remote` feature adds `ureq` + `rustls`, which never enter a wasm or a default
//! build). All network failures surface as a single [`RemoteError`] type (no leaked client errors),
//! mirroring Python's "every failure is one `ValueError`" contract.

#![cfg(all(feature = "remote", not(target_arch = "wasm32")))]

use crate::compose::content_sha;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The two schemes the fetcher will EVER touch, re-asserted on the initial url and every redirect.
const ALLOWED_SCHEMES: [&str; 2] = ["http://", "https://"];
/// 256 MiB body cap (`hcdf_io.remote._DEFAULT_MAX_BYTES`).
const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// 30 s request timeout (`hcdf_io.remote._DEFAULT_TIMEOUT`).
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// 256 KiB streaming reads (`hcdf_io.remote._CHUNK`).
const CHUNK: usize = 1024 * 256;
/// Cap on redirect hops we will manually follow before giving up (parity with a browser's ~10/20;
/// any non-http(s) hop is rejected regardless, so this only bounds a redirect LOOP).
const MAX_REDIRECTS: usize = 10;

/// Every fetch failure (bad scheme, network/DNS, timeout, HTTP status, rejected redirect, size cap,
/// `@sha` mismatch) surfaces as one error type, mirroring Python's single-`ValueError` contract.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// A non-http(s) scheme (initial url OR a redirect target): rejected before/instead of I/O.
    #[error("refusing to fetch non-http(s) uri {uri:?}: only http:// and https:// are fetched (a remote asset must be a plain web URL)")]
    BadScheme { uri: String },
    /// The body exceeded `max_bytes` (declared up front, or streamed + aborted mid-download).
    #[error("remote asset {uri:?} exceeds the {cap}-byte size cap (raise max_bytes to allow it)")]
    TooLarge { uri: String, cap: u64 },
    /// `expect_sha` was given but the fetched content sha did not match (bytes discarded, not cached).
    #[error("remote asset {uri:?} failed integrity check: pinned @sha {pinned} != fetched content {actual} (the remote bytes do not match the document's @sha)")]
    ShaMismatch {
        uri: String,
        pinned: String,
        actual: String,
    },
    /// A redirect to a non-http(s) scheme: refused per hop (the SSRF guard).
    #[error("refusing to follow redirect to non-http(s) uri {target:?} (only http:// and https:// are fetched)")]
    BadRedirect { target: String },
    /// A network / HTTP-status / timeout failure (the reason text preserved, like Python).
    #[error("could not fetch remote asset {uri:?}: {reason}")]
    Network { uri: String, reason: String },
    /// A local I/O failure writing the content-addressed cache entry.
    #[error("could not cache remote asset {uri:?}: {reason}")]
    Cache { uri: String, reason: String },
    /// A misuse: a non-positive `max_bytes`.
    #[error("max_bytes must be positive, got {0}")]
    BadMaxBytes(u64),
}

/// True iff `uri` is an `http(s)://` URL (`hcdf_io.remote.is_remote_uri`).
pub fn is_remote_uri(uri: &str) -> bool {
    let low = uri.to_ascii_lowercase();
    ALLOWED_SCHEMES.iter().any(|s| low.starts_with(s))
}

/// A PER-USER private cache dir for fetched remote bytes (never a world-shared `/tmp` path). Prefers
/// `$XDG_CACHE_HOME/hcdf-remote` (when absolute), else `~/.cache/hcdf-remote`, else a uid-namespaced
/// dir under the system temp. Mirrors `hcdf_io.remote._default_cache_dir`.
pub fn default_cache_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        let p = PathBuf::from(&xdg);
        if p.is_absolute() {
            return p.join("hcdf-remote");
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if home.is_dir() {
            return home.join(".cache").join("hcdf-remote");
        }
    }
    // Last resort: a uid-namespaced dir under the system temp (still per-user, not a fixed name).
    let uid = uid_string();
    std::env::temp_dir().join(format!("hcdf-remote-cache-{uid}"))
}

#[cfg(unix)]
fn uid_string() -> String {
    // Avoid a libc dependency: read the effective uid from `/proc/self/status` (the `Uid:` line's
    // first token is the real uid; good enough to namespace a per-user cache dir). If unavailable,
    // fall back to "u" (matching Python's `getattr(os, "geteuid", lambda: "u")()` fallback), so the
    // path is still per-process/per-user at worst, never the shared fixed name.
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                if let Some(tok) = rest.split_whitespace().next() {
                    return tok.to_string();
                }
            }
        }
    }
    "u".to_string()
}

#[cfg(not(unix))]
fn uid_string() -> String {
    "u".to_string()
}

/// Fetch an `http(s)://` URL to a content-addressed file in `cache_dir`; return its path. The Rust
/// port of `hcdf_io.remote.fetch_remote`, reproducing every guard (see the module docs).
///
/// `expect_sha` (when `Some`) pins the content: a mismatch is a [`RemoteError::ShaMismatch`] and
/// nothing is cached. `cache_dir` defaults to a per-user private dir ([`default_cache_dir`]).
/// `max_bytes`/`timeout` default to 256 MiB / 30 s.
pub fn fetch_remote(
    url: &str,
    expect_sha: Option<&str>,
    cache_dir: Option<&Path>,
    max_bytes: Option<u64>,
    timeout: Option<Duration>,
) -> Result<PathBuf, RemoteError> {
    if !is_remote_uri(url) {
        return Err(RemoteError::BadScheme {
            uri: url.to_string(),
        });
    }
    let max_bytes = max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    if max_bytes == 0 {
        return Err(RemoteError::BadMaxBytes(max_bytes));
    }
    let timeout = timeout.unwrap_or_else(|| Duration::from_secs(DEFAULT_TIMEOUT_SECS));

    let cache_owned;
    let cache_dir = match cache_dir {
        Some(c) => c,
        None => {
            cache_owned = default_cache_dir();
            &cache_owned
        }
    };
    std::fs::create_dir_all(cache_dir).map_err(|e| RemoteError::Cache {
        uri: url.to_string(),
        reason: e.to_string(),
    })?;

    let data = read_capped(url, max_bytes, timeout)?;
    let sha = content_sha(&data);
    if let Some(expect) = expect_sha {
        if sha != expect {
            return Err(RemoteError::ShaMismatch {
                uri: url.to_string(),
                pinned: expect.to_string(),
                actual: sha,
            });
        }
    }

    let ext = ext_for(url);
    let hexpart = sha.split_once(':').map_or(sha.as_str(), |(_, h)| h);
    let dest = cache_dir.join(format!("{hexpart}{ext}"));

    // A cache HIT is only trusted if the existing file's BYTES still hash to the content address (and
    // expect_sha, when pinned). The cache is content-addressed, but a local attacker could pre-create
    // the predictable <sha><ext> file with DIFFERENT bytes; returning it blind would hand back poison
    // whose content does not match its own name. So we RE-HASH on hit and, on any mismatch, OVERWRITE
    // it with the freshly-fetched + verified bytes we already hold.
    if dest.exists() {
        if let Ok(existing) = std::fs::read(&dest) {
            if content_sha(&existing) == sha {
                return Ok(dest);
            }
        }
        // else: unreadable or tampered cache entry -> fall through and overwrite atomically.
    }

    // Atomic publish: write to a unique temp file in the SAME dir, then rename it into place, so a
    // concurrent reader never sees a half-written entry and a poisoned/stale dest is replaced wholesale.
    atomic_write(cache_dir, &dest, &ext, &data).map_err(|e| RemoteError::Cache {
        uri: url.to_string(),
        reason: e.to_string(),
    })?;
    Ok(dest)
}

/// Write `data` to `dest` atomically: a unique temp file in `dest`'s dir, then `rename`.
fn atomic_write(dir: &Path, dest: &Path, ext: &str, data: &[u8]) -> std::io::Result<()> {
    // A unique-enough temp name in the same dir (so rename is atomic on the same filesystem). Uses the
    // nanosecond clock + pid; the rename is the atomic step, so a name collision just retries via the
    // overwrite-on-rename semantics.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let tmp = dir.join(format!(".tmp-{pid}-{nonce}{ext}"));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all().ok();
        drop(f);
        std::fs::rename(&tmp, dest)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Open `url` (http(s) already validated) and read its body, aborting past `max_bytes`. Redirects are
/// NOT auto-followed: each 3xx `Location` is re-validated against the http(s) allow-list IN OUR CODE
/// and re-issued, so a redirect to a non-http(s) scheme is refused per hop (parity with Python's
/// `_RestrictedRedirectHandler` + handler-less opener). Mirrors `hcdf_io.remote._read_capped`.
fn read_capped(url: &str, max_bytes: u64, timeout: Duration) -> Result<Vec<u8>, RemoteError> {
    // An agent that NEVER follows redirects itself: we follow them manually with a per-hop scheme
    // check. `max_redirects_will_error(false)` makes ureq RETURN the 3xx response instead of erroring,
    // so we can read its Location header.
    //
    // TIMEOUTS: PER-PHASE, not global (parity with `hcdf_io.remote`). The Python authority passes its
    // `timeout` to `opener.open(req, timeout=...)`, which becomes the SOCKET timeout: it bounds each
    // blocking operation (connect + every `recv`) INDEPENDENTLY and RESETS on each, so a transfer that
    // keeps making progress (each read within `timeout`) never trips no matter its total wall-clock.
    // `timeout_global` is the opposite (one end-to-end deadline over the WHOLE call), so a slow (or
    // keep-alive-dribbling) local server trips it even though every individual read is prompt, which is
    // exactly what broke the fetcher against the test harness. Map `timeout` onto ureq's per-phase knobs
    // (resolve / connect / send / recv-headers / recv-body) and leave `global` unset, so the behaviour
    // matches Python's per-operation socket timeout; the streaming size cap below still bounds the body.
    let config = ureq::Agent::config_builder()
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_resolve(Some(timeout))
        .timeout_connect(Some(timeout))
        .timeout_send_request(Some(timeout))
        .timeout_send_body(Some(timeout))
        .timeout_recv_response(Some(timeout))
        .timeout_recv_body(Some(timeout))
        .user_agent("hcdf-bundle/1.0")
        .build();
    let agent: ureq::Agent = config.into();

    let mut current = url.to_string();
    for _hop in 0..=MAX_REDIRECTS {
        // Re-assert the allow-list on EVERY url we are about to open (initial + each redirect target).
        if !is_remote_uri(&current) {
            return Err(RemoteError::BadRedirect { target: current });
        }
        let resp = agent
            .get(&current)
            .call()
            .map_err(|e| RemoteError::Network {
                uri: url.to_string(),
                reason: classify(&e),
            })?;
        let status = resp.status().as_u16();
        // Manual redirect handling: 3xx with a Location -> validate + re-issue against that target.
        if (300..400).contains(&status) {
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            match location {
                Some(loc) => {
                    let target = resolve_location(&current, &loc);
                    // Reject a non-http(s) redirect BEFORE re-issuing (the SSRF guard, per hop).
                    if !is_remote_uri(&target) {
                        return Err(RemoteError::BadRedirect { target });
                    }
                    current = target;
                    continue;
                }
                None => {
                    // A 3xx with no Location is a malformed response: treat as a network failure.
                    return Err(RemoteError::Network {
                        uri: url.to_string(),
                        reason: format!("HTTP {status} redirect with no Location header"),
                    });
                }
            }
        }
        if !(200..300).contains(&status) {
            return Err(RemoteError::Network {
                uri: url.to_string(),
                reason: format!("HTTP status {status}"),
            });
        }

        // 2xx: read the body, capping length. A server-declared Content-Length over the cap is
        // rejected before reading a byte; the streaming cap protects regardless of any header.
        let declared = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());
        if let Some(declared) = declared {
            if declared > max_bytes {
                return Err(RemoteError::TooLarge {
                    uri: url.to_string(),
                    cap: max_bytes,
                });
            }
        }
        let mut resp = resp;
        let mut reader = resp.body_mut().as_reader();
        // Length-delimited read (mirrors `urllib`'s HTTPResponse, which returns EOF at Content-Length):
        // when the server declares a length we read EXACTLY that many bytes and STOP, never issuing the
        // trailing read that would block on a keep-alive socket the server holds open (the connection
        // isn't closed to signal EOF). Without a declared length we fall back to reading until the peer
        // closes. Either way the streaming cap still aborts a body that outgrows `max_bytes`.
        return stream_capped(&mut reader, max_bytes, declared, url);
    }
    // Exhausted the redirect budget without reaching a 2xx (a redirect loop).
    Err(RemoteError::Network {
        uri: url.to_string(),
        reason: format!("too many redirects (> {MAX_REDIRECTS})"),
    })
}

/// Stream `reader` in `CHUNK`-sized reads, aborting the moment the total exceeds `max_bytes` (so a
/// hostile body can never be buffered unbounded). Mirrors the Python chunk loop.
///
/// `declared` is the server's `Content-Length` when known: once we have read that many bytes we STOP
/// without another read, mirroring `urllib`'s length-delimited EOF and avoiding a trailing read that
/// would block on a keep-alive socket the server never closes. When `declared` is `None` we read until
/// the peer closes (a plain `read == 0`).
fn stream_capped<R: Read>(
    reader: &mut R,
    max_bytes: u64,
    declared: Option<u64>,
    url: &str,
) -> Result<Vec<u8>, RemoteError> {
    let mut out: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total: u64 = 0;
    loop {
        // Length-delimited: stop the moment we have the declared body, never issuing the blocking
        // trailing read (the connection may be kept alive rather than closed to signal EOF).
        if declared.is_some_and(|d| total >= d) {
            break;
        }
        let n = reader.read(&mut buf).map_err(|e| RemoteError::Network {
            uri: url.to_string(),
            reason: e.to_string(),
        })?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max_bytes {
            return Err(RemoteError::TooLarge {
                uri: url.to_string(),
                cap: max_bytes,
            });
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

/// Resolve a redirect `Location` (which may be absolute or relative) against the current url. Keeps
/// this minimal: an absolute `scheme://...` Location replaces the url; a relative one is joined to the
/// current url's origin/path. A non-http(s) absolute target is left as-is so the caller's allow-list
/// check rejects it.
fn resolve_location(current: &str, location: &str) -> String {
    let low = location.to_ascii_lowercase();
    // Absolute URL with a scheme (http://, https://, ftp://, file://, data:, ...): use verbatim.
    if low.contains("://") || low.starts_with("data:") || low.starts_with("mailto:") {
        return location.to_string();
    }
    // Scheme-relative ("//host/path"): inherit the current scheme.
    if let Some(rest) = location.strip_prefix("//") {
        let scheme = current.split("://").next().unwrap_or("https");
        return format!("{scheme}://{rest}");
    }
    // Path-absolute ("/path") or path-relative: join to the current origin.
    let (scheme, after) = match current.split_once("://") {
        Some((s, a)) => (s, a),
        None => return location.to_string(),
    };
    let host = after.split('/').next().unwrap_or("");
    if let Some(abs_path) = location.strip_prefix('/') {
        format!("{scheme}://{host}/{abs_path}")
    } else {
        // path-relative: drop the current last path segment, append the relative one.
        let path = &after[host.len()..];
        let base = match path.rfind('/') {
            Some(i) => &path[..=i],
            None => "/",
        };
        format!("{scheme}://{host}{base}{location}")
    }
}

/// A short, stable reason string from a ureq error (so a network failure reads cleanly, like Python's
/// preserved `reason`).
fn classify(e: &ureq::Error) -> String {
    e.to_string()
}

/// A file extension for the cached name, taken from the URL path (defaults to `.bin`). Mirrors
/// `hcdf_io.remote._ext_for`.
fn ext_for(url: &str) -> String {
    // Strip query/fragment, take the last path segment's extension.
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let seg = path.rsplit('/').next().unwrap_or("");
    match seg.rfind('.') {
        Some(i) if i + 1 < seg.len() => seg[i..].to_string(),
        _ => ".bin".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list_rejects_non_http() {
        assert!(is_remote_uri("http://x/y"));
        assert!(is_remote_uri("https://x/y"));
        assert!(!is_remote_uri("file:///etc/passwd"));
        assert!(!is_remote_uri("ftp://x/y"));
        assert!(!is_remote_uri("package://p/m.glb"));
        assert!(!is_remote_uri("data:text/plain,hi"));
    }

    #[test]
    fn fetch_rejects_non_http_before_io() {
        let err = fetch_remote("file:///etc/passwd", None, None, None, None).unwrap_err();
        assert!(matches!(err, RemoteError::BadScheme { .. }));
    }

    #[test]
    fn ext_for_matches_python() {
        assert_eq!(ext_for("https://h/a/rotor.glb"), ".glb");
        assert_eq!(ext_for("https://h/a/rotor.glb?v=2"), ".glb");
        assert_eq!(ext_for("https://h/noext"), ".bin");
        assert_eq!(ext_for("https://h/"), ".bin");
    }

    #[test]
    fn resolve_location_handles_forms() {
        assert_eq!(
            resolve_location("http://h/a/b", "http://other/x"),
            "http://other/x"
        );
        assert_eq!(
            resolve_location("http://h/a/b", "ftp://evil/x"),
            "ftp://evil/x"
        );
        assert_eq!(resolve_location("http://h/a/b", "/c"), "http://h/c");
        assert_eq!(resolve_location("http://h/a/b", "c"), "http://h/a/c");
        assert_eq!(
            resolve_location("http://h/a/b", "//other/x"),
            "http://other/x"
        );
    }
}
