//! CORS origin allowlisting.
//!
//! `create_router`/`main` must only grant CORS approval to known frontend
//! origins (web-app, native-app in dev/staging/prod), never reflect
//! `Access-Control-Allow-Origin` for an arbitrary requesting origin — and
//! never fall back to a permissive wildcard just because the configured
//! list happens to be empty (e.g. a misconfigured deployment).

use crate::config::AppConfig;
use tower_http::cors::{Any, CorsLayer};

/// Builds the CORS layer from [`AppConfig::allowed_cors_origins`].
///
/// Only the configured origins ever receive `Access-Control-Allow-Origin`
/// approval. There is intentionally no permissive fallback: an empty list
/// (misconfigured or deliberately locked down) results in a `CorsLayer`
/// that grants no origin any cross-origin access, rather than silently
/// becoming wildcard-permissive.
pub fn build_cors_layer(config: &AppConfig) -> CorsLayer {
    let origins: Vec<_> = config
        .allowed_cors_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

    let layer = CorsLayer::new().allow_methods(Any).allow_headers(Any);

    if origins.is_empty() {
        layer
    } else {
        layer.allow_origin(origins)
    }
}
