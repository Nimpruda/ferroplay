//! Serves the page and player from `web/` (`ServeDir` handles the Range requests seeking needs),
//! plus `/proxy/<url>` so any link plays, even from sites without CORS headers. The proxy streams
//! bodies through as they arrive, except playlists, which it reads whole to rewrite.
//! Usage: cargo run -p server -- [dir] (default `web`, port from $PORT or 8080, address from $HOST or
//! 127.0.0.1). Local use only: the proxy fetches any URL, so only containers set HOST=0.0.0.0.

use axum::Router;
use axum::extract::{Request, State};
use axum::body::Body;
use axum::http::header::{ACCEPT_RANGES, CACHE_CONTROL, CONTENT_RANGE, CONTENT_TYPE, ETAG, LAST_MODIFIED, RANGE};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::services::ServeDir;

const PLAYLIST: &str = "application/vnd.apple.mpegurl";

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "web".into());
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
    println!("serving {dir} on http://{}", listener.local_addr()?);
    let client = reqwest::Client::builder().user_agent("player-proxy").build().expect("http client");
    let app = Router::new()
        .route("/proxy/{*url}", get(proxy))
        .fallback_service(ServeDir::new(dir))
        .layer(middleware::from_fn(hls_types))
        .with_state(client);
    axum::serve(listener, app).await
}

/// Fetches `<url>` for the page. Playlists come back with every URI pointing through here too.
async fn proxy(State(client): State<reqwest::Client>, uri: Uri, headers: HeaderMap) -> Response {
    // The raw path and query, not the decoded wildcard: the URL must reach the upstream as given.
    let url = uri.path_and_query().map_or("", |p| p.as_str()).trim_start_matches("/proxy/");
    let mut request = client.get(url);
    if let Some(range) = headers.get(RANGE) {
        request = request.header(RANGE, range);
    }
    let upstream = match request.send().await {
        Ok(upstream) => upstream,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    let (status, base) = (upstream.status(), upstream.url().to_string()); // after redirects
    let mut out = HeaderMap::new();
    for name in [CONTENT_TYPE, CONTENT_RANGE, ACCEPT_RANGES, CACHE_CONTROL, ETAG, LAST_MODIFIED] {
        if let Some(value) = upstream.headers().get(&name) {
            out.insert(name, value.clone());
        }
    }
    // Sites also send `application/x-mpegURL`, `audio/mpegurl`...
    let rewrite = status.is_success()
        && (hls::playlist::is_playlist(&base) || out.get(CONTENT_TYPE).and_then(|t| t.to_str().ok()).is_some_and(|t| t.to_ascii_lowercase().contains("mpegurl")));
    if !rewrite {
        // Segments never change, so the upstream's caching headers hold for our copy too. The body goes
        // through as it arrives: the page gets its first byte without waiting for the last from upstream.
        return (status, out, Body::new(reqwest::Body::from(upstream))).into_response();
    }
    let body = match upstream.bytes().await {
        Ok(body) => body,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    // The HLS type makes hls_types mark it never cached, whatever URL it came from. The upstream's
    // validators were for the text before the rewrite.
    out.insert(CONTENT_TYPE, HeaderValue::from_static(PLAYLIST));
    out.remove(ETAG);
    out.remove(LAST_MODIFIED);
    let text = hls::playlist::rewrite(&String::from_utf8_lossy(&body), &base, |uri| format!("/proxy/{uri}"));
    (status, out, text).into_response()
}

/// The content types the HLS spec asks for (the defaults are `audio/x-mpegurl` and a DLNA type). Playlists,
/// known by that type so the proxy's are too, are never cached: a live playlist changes every few seconds,
/// and a cached one lists segments already gone.
async fn hls_types(request: Request, next: Next) -> Response {
    // Any case, like the proxy's is_playlist: a file named X.M3U8 is a playlist all the same.
    let kind = match request.uri().path().rsplit('.').next().unwrap_or_default() {
        ext if ext.eq_ignore_ascii_case("m3u8") => Some(PLAYLIST),
        ext if ext.eq_ignore_ascii_case("ts") => Some("video/mp2t"),
        _ => None,
    };
    let mut response = next.run(request).await;
    if response.status().is_success() {
        let headers = response.headers_mut();
        if let Some(kind) = kind {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(kind));
        }
        if headers.get(CONTENT_TYPE).is_some_and(|t| t == PLAYLIST) {
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        }
    }
    response
}
