//! Serves the page and player from `web/` (`ServeDir` handles the Range requests seeking needs),
//! plus `/proxy/<url>` so any link plays, even from sites without CORS headers.
//! Usage: cargo run -p server -- [dir] (default `web`, port from $PORT or 8080, address from $HOST or
//! 127.0.0.1). Local use only: the proxy fetches any URL, so only containers set HOST=0.0.0.0.

use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{ACCEPT_RANGES, CACHE_CONTROL, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::services::ServeDir;

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
    for name in [CONTENT_TYPE, CONTENT_RANGE, ACCEPT_RANGES] {
        if let Some(value) = upstream.headers().get(&name) {
            out.insert(name, value.clone());
        }
    }
    let is_playlist = hls::playlist::is_playlist(&base)
        || out.get(CONTENT_TYPE).and_then(|t| t.to_str().ok()).is_some_and(|t| t.contains("mpegurl"));
    let body = match upstream.bytes().await {
        Ok(body) => body,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    if is_playlist && status.is_success() {
        let text = hls::playlist::rewrite(&String::from_utf8_lossy(&body), &base, |uri| format!("/proxy/{uri}"));
        return (status, out, text).into_response();
    }
    (status, out, body).into_response()
}

/// The content types the HLS spec asks for (the defaults are `audio/x-mpegurl` and a DLNA type). Playlists
/// are never cached: a live playlist changes every few seconds, and a cached one lists segments already gone.
async fn hls_types(request: Request, next: Next) -> Response {
    let (kind, playlist) = match request.uri().path().rsplit('.').next() {
        Some("m3u8") => (Some("application/vnd.apple.mpegurl"), true),
        Some("ts") => (Some("video/mp2t"), false),
        _ => (None, false),
    };
    let mut response = next.run(request).await;
    if let Some(kind) = kind
        && response.status().is_success()
    {
        response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(kind));
        if playlist {
            response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        }
    }
    response
}
