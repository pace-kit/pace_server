use axum::http::{header, HeaderValue, Method};
use tower_http::cors::CorsLayer;

/// Explicit browser origin allowlist. Authentication remains mandatory on requests.
pub fn cors(origins: &str) -> Result<CorsLayer, &'static str> {
    let mut allowed = Vec::new();
    for value in origins.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let url = url::Url::parse(value).map_err(|_| "Invalid PACE_ALLOWED_ORIGINS origin")?;
        if !matches!(url.scheme(), "http" | "https") || url.origin().ascii_serialization() != value
        {
            return Err(
                "PACE_ALLOWED_ORIGINS requires exact http(s) origins without paths or wildcards",
            );
        }
        allowed.push(HeaderValue::from_str(value).map_err(|_| "Invalid origin header")?);
    }
    Ok(CorsLayer::new()
        .allow_origin(allowed)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::IF_MATCH,
        ])
        .expose_headers([header::ETAG, header::LOCATION]))
}
