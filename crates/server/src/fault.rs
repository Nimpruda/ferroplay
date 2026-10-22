//! `/fault/<spec>/<path>`: serves `web/<path>` as a bad network or CDN would, for the catalog's fault entries.
//! `<spec>` is a comma-separated list of `key=value` (a `/` in a value written `%2F`):
//! - `nth=<n>`: only the nth request under this spec (counted per spec since the server started, all paths
//!   together: 1 is the first playlist) gets the faults below; without it, every request does.
//! - `status=<code>`: that status, and no body.
//! - `flaky=<share>`: that share of requests fail with 503, picked by a fixed hash of the path and how many times
//!   it was asked for, so a run is repeatable.
//! - `delay=<n>ms|<n>s`: waits that long before answering.
//! - `slow=<n>bit|kbit|mbit`: sends the body at that rate.
//! - `truncate=<share>`: sends only that share of the body, with the full Content-Length, and closes.
//! - `type=<content type>`: serves with that Content-Type.
//! - `body=html`: an HTML error page in place of the file, with status 200, as a CDN or a captive portal sends.
//!
//! Playlists list their URIs through the same `/fault/<spec>/`, so the faults reach every child playlist and segment.
//! Nothing is cached: a fault is not to be hidden by a copy from before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderValue, Request, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{StreamExt, stream};
use tower_http::services::ServeDir;

/// The route, serving files from `dir`.
pub fn router(dir: &str) -> Router {
    let state = Arc::new(Faults { files: ServeDir::new(dir), counts: Mutex::default() });
    Router::new().route("/fault/{spec}/{*path}", get(serve)).with_state(state)
}

struct Faults {
    files: ServeDir,
    /// Requests so far, by spec and by spec and path.
    counts: Mutex<HashMap<String, u64>>,
}

impl Faults {
    /// Counts one more request for `key`; its number, from 1.
    fn count(&self, key: &str) -> u64 {
        let mut counts = self.counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let n = counts.entry(key.into()).or_default();
        *n += 1;
        *n
    }
}

#[derive(Debug, Default, PartialEq)]
struct Spec {
    nth: Option<u64>,
    status: Option<StatusCode>,
    flaky: Option<f64>,
    delay: Option<Duration>,
    /// Bytes per second.
    slow: Option<u64>,
    truncate: Option<f64>,
    kind: Option<HeaderValue>,
    html: bool,
}

const HTML: &str = "<!doctype html><title>503 Service Unavailable</title><h1>Service Unavailable</h1>\n";

impl std::str::FromStr for Spec {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let mut spec = Spec::default();
        for item in text.split(',') {
            let (key, value) = item.split_once('=').ok_or_else(|| format!("'{item}' is not key=value"))?;
            let bad = |what: &str| format!("{key}={value}: not {what}");
            match key {
                "nth" => spec.nth = Some(value.parse().ok().filter(|&n| n > 0).ok_or_else(|| bad("a request number from 1"))?),
                "status" => spec.status = Some(value.parse::<u16>().ok().and_then(|code| StatusCode::from_u16(code).ok()).ok_or_else(|| bad("an HTTP status"))?),
                "flaky" => spec.flaky = Some(share(value).ok_or_else(|| bad("a share from 0 to 1"))?),
                "truncate" => spec.truncate = Some(share(value).ok_or_else(|| bad("a share from 0 to 1"))?),
                "delay" => spec.delay = Some(duration(value).ok_or_else(|| bad("a duration like 500ms or 2s"))?),
                "slow" => spec.slow = Some(rate(value).ok_or_else(|| bad("a rate like 300kbit"))?),
                "type" => spec.kind = Some(HeaderValue::from_str(value).map_err(|_| bad("a content type"))?),
                "body" if value == "html" => spec.html = true,
                "body" => return Err(bad("html")),
                _ => return Err(format!("unknown fault '{key}'")),
            }
        }
        Ok(spec)
    }
}

fn share(value: &str) -> Option<f64> {
    value.parse().ok().filter(|share| (0.0..=1.0).contains(share))
}

fn duration(value: &str) -> Option<Duration> {
    scaled(value, &[("ms", 1e-3), ("s", 1.0)]).map(Duration::from_secs_f64)
}

/// Bytes per second.
fn rate(value: &str) -> Option<u64> {
    scaled(value, &[("bit", 1.0 / 8.0), ("kbit", 1e3 / 8.0), ("mbit", 1e6 / 8.0)]).map(|bytes| bytes as u64).filter(|&bytes| bytes > 0)
}

/// A number followed by one of `units`, times that unit's scale.
fn scaled(value: &str, units: &[(&str, f64)]) -> Option<f64> {
    let (number, unit) = value.split_at(value.find(|c: char| c.is_ascii_alphabetic())?);
    let number: f64 = number.parse().ok().filter(|n: &f64| n.is_finite() && *n >= 0.0)?;
    units.iter().find(|&&(name, _)| name == unit).map(|&(_, scale)| number * scale)
}

impl Spec {
    /// Whether request number `n` under this spec gets the faults.
    fn hits(&self, n: u64) -> bool {
        self.nth.is_none_or(|nth| nth == n)
    }

    /// Whether the `k`th request for `path` is one of the flaky share that fails.
    fn flakes(&self, path: &str, k: u64) -> bool {
        self.flaky.is_some_and(|share| unit(path, k) < share)
    }
}

/// A number in [0, 1) from `path` and `k`, the same in every run: FNV-1a, then a SplitMix64 finish to spread it.
fn unit(path: &str, k: u64) -> f64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in path.bytes().chain(k.to_le_bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;
    (hash >> 11) as f64 / (1u64 << 53) as f64
}

async fn serve(State(faults): State<Arc<Faults>>, Path((spec, _)): Path<(String, String)>, uri: Uri) -> Response {
    let mut response = answer(&faults, &spec, &uri).await.into_response();
    response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn answer(faults: &Faults, spec: &str, uri: &Uri) -> Result<Response, (StatusCode, String)> {
    let spec: Spec = spec.parse().map_err(|reason| (StatusCode::BAD_REQUEST, reason))?;
    // As requested (still percent-encoded), to be counted, passed on, and put back in front of playlist URIs.
    let (raw_spec, path) = uri.path().trim_start_matches("/fault/").split_once('/').unwrap_or_default();
    let k = faults.count(uri.path());
    // A request the faults miss (nth) is served as if there were none.
    let spec = if spec.hits(faults.count(raw_spec)) { spec } else { Spec::default() };
    if let Some(delay) = spec.delay {
        tokio::time::sleep(delay).await;
    }
    if let Some(status) = spec.status {
        return Ok(status.into_response());
    }
    if spec.flakes(path, k) {
        return Ok(StatusCode::SERVICE_UNAVAILABLE.into_response());
    }

    // The file, as the server serves it elsewhere; without the browser's headers: no range, no 304.
    let file = Request::builder().uri(format!("/{path}")).body(Body::empty()).expect("a path from a URI");
    let served = faults.files.clone().try_call(file).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !served.status().is_success() {
        return Ok(served.map(Body::new));
    }
    let mut kind = crate::hls_type(path).map(HeaderValue::from_static).or_else(|| served.headers().get(CONTENT_TYPE).cloned());
    let mut body = axum::body::to_bytes(Body::new(served.into_body()), usize::MAX)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if hls::playlist::is_playlist(path) {
        let prefix = |uri: &str| if uri.starts_with('/') { format!("/fault/{raw_spec}{uri}") } else { uri.into() };
        body = hls::playlist::rewrite(&String::from_utf8_lossy(&body), &format!("/{path}"), prefix).into();
    }
    if spec.html {
        (body, kind) = (Bytes::from_static(HTML.as_bytes()), Some(HeaderValue::from_static("text/html")));
    }
    if let Some(other) = spec.kind {
        kind = Some(other);
    }

    let length = body.len();
    if let Some(share) = spec.truncate {
        body.truncate((length as f64 * share) as usize);
    }
    // Chunks of a tenth of a second at the slow rate, each after its tenth; or the whole body at once.
    let (chunk, pause) = match spec.slow {
        Some(rate) => ((rate / 10).max(1) as usize, Duration::from_millis(100)),
        None => (length.max(1), Duration::ZERO),
    };
    let chunks: Vec<Bytes> = body.chunks(chunk).map(|part| body.slice_ref(part)).collect();
    let stream = stream::iter(chunks)
        .then(move |chunk| async move {
            if !pause.is_zero() {
                tokio::time::sleep(pause).await;
            }
            Ok(chunk)
        })
        // A truncated body ends in an error, which closes the connection short of the full length it was announced
        // with, as a dropped one does; after a pause, so what was sent is flushed first.
        .chain(stream::iter(spec.truncate).then(|_| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err(std::io::Error::other("truncated on purpose"))
        }));
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(CONTENT_LENGTH, length.into());
    if let Some(kind) = kind {
        response.headers_mut().insert(CONTENT_TYPE, kind);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_fault() {
        let spec: Spec = "nth=3,status=404,flaky=0.3,delay=2s,slow=500kbit,truncate=0.5,type=text/html,body=html".parse().unwrap();
        assert_eq!(spec, Spec {
            nth: Some(3),
            status: Some(StatusCode::NOT_FOUND),
            flaky: Some(0.3),
            delay: Some(Duration::from_secs(2)),
            slow: Some(62_500),
            truncate: Some(0.5),
            kind: Some(HeaderValue::from_static("text/html")),
            html: true,
        });
        assert_eq!("delay=250ms".parse::<Spec>().unwrap().delay, Some(Duration::from_millis(250)));
        assert_eq!("slow=2mbit".parse::<Spec>().unwrap().slow, Some(250_000));
        assert_eq!("slow=800bit".parse::<Spec>().unwrap().slow, Some(100));
    }

    #[test]
    fn refuses_bad_specs_with_the_reason() {
        for (text, reason) in [
            ("", "'' is not key=value"),
            ("status", "'status' is not key=value"),
            ("status=99", "status=99: not an HTTP status"),
            ("status=abc", "status=abc: not an HTTP status"),
            ("nth=0", "nth=0: not a request number from 1"),
            ("flaky=1.5", "flaky=1.5: not a share from 0 to 1"),
            ("truncate=-0.1", "truncate=-0.1: not a share from 0 to 1"),
            ("delay=2", "delay=2: not a duration like 500ms or 2s"),
            ("delay=2h", "delay=2h: not a duration like 500ms or 2s"),
            ("slow=fast", "slow=fast: not a rate like 300kbit"),
            ("slow=300kb", "slow=300kb: not a rate like 300kbit"),
            ("slow=0kbit", "slow=0kbit: not a rate like 300kbit"),
            ("type=a\nb", "type=a\nb: not a content type"),
            ("body=json", "body=json: not html"),
            ("status=404,lag=1s", "unknown fault 'lag'"),
        ] {
            assert_eq!(text.parse::<Spec>(), Err(reason.into()), "{text:?}");
        }
    }

    #[test]
    fn nth_hits_that_request_only() {
        let spec: Spec = "status=404,nth=3".parse().unwrap();
        assert_eq!((1..=5).map(|n| spec.hits(n)).collect::<Vec<_>>(), [false, false, true, false, false]);
        let every: Spec = "status=503".parse().unwrap();
        assert!((1..=5).all(|n| every.hits(n)));
    }

    #[test]
    fn counts_each_key_apart() {
        let faults = Faults { files: ServeDir::new("."), counts: Mutex::default() };
        assert_eq!([faults.count("a"), faults.count("a"), faults.count("b"), faults.count("a")], [1, 2, 1, 3]);
    }

    #[test]
    fn flaky_fails_its_share_the_same_way_every_run() {
        let spec: Spec = "flaky=0.3".parse().unwrap();
        let fails = |path: &str| (1..=1000).filter(|&k| spec.flakes(path, k)).count();
        let (a, b) = (fails("catalog/x/000.ts"), fails("catalog/x/001.ts"));
        assert!((250..350).contains(&a) && (250..350).contains(&b), "{a} and {b} of 1000");
        assert_eq!(a, fails("catalog/x/000.ts"));
        assert!((1..=20).any(|k| spec.flakes("catalog/x/000.ts", k) != spec.flakes("catalog/x/001.ts", k)), "paths fail apart");
        assert!(!"flaky=0".parse::<Spec>().unwrap().flakes("a", 1));
        assert!("flaky=1".parse::<Spec>().unwrap().flakes("a", 1));
    }
}
