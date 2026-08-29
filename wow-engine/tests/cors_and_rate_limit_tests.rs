use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt; // for `oneshot`
use wow_engine::anchor::sep38::Sep38Client;
use wow_engine::api::cors::build_cors_layer;
use wow_engine::api::{create_router, create_router_with_cache, RouterDeps};
use wow_engine::cache_sync::ClusterCache;
use wow_engine::config::AppConfig;

fn health_request() -> Request<Body> {
    Request::builder()
        .uri("/api/v1/health")
        .body(Body::empty())
        .unwrap()
}

fn quote_request() -> Request<Body> {
    quote_request_with_xff(None)
}

fn quote_request_with_xff(xff: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/quote")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(value) = xff {
        builder = builder.header("x-forwarded-for", value);
    }
    builder
        .body(Body::from(
            r#"{"source_chain":"Solana","dest_chain":"Ethereum","source_asset":"USDC","dest_asset":"USDC","amount_in":1}"#,
        ))
        .unwrap()
}

fn router_with_config(config: AppConfig) -> axum::Router {
    create_router_with_cache(
        None,
        Duration::from_secs(30),
        RouterDeps {
            db: None,
            tracker: None,
            cache: ClusterCache::local_only(),
            config: Arc::new(config),
            sep38_client: Arc::new(Sep38Client::new()),
            mempool_risk_registry: Arc::new(wow_engine::mempool::PoolRiskRegistry::new()),
        },
    )
}

#[tokio::test]
async fn test_disallowed_origin_does_not_receive_cors_approval() {
    let config = AppConfig {
        allowed_cors_origins: vec!["https://app.example.com".to_string()],
        ..AppConfig::default()
    };
    let app = create_router(None, None).layer(build_cors_layer(&config));

    let mut request = health_request();
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.example.com".parse().unwrap());

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "a disallowed Origin must not receive Access-Control-Allow-Origin"
    );
}

#[tokio::test]
async fn test_allowed_origin_receives_cors_approval() {
    let config = AppConfig {
        allowed_cors_origins: vec!["https://app.example.com".to_string()],
        ..AppConfig::default()
    };
    let app = create_router(None, None).layer(build_cors_layer(&config));

    let mut request = health_request();
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://app.example.com".parse().unwrap());

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "https://app.example.com"
    );
}

#[tokio::test]
async fn test_empty_allowlist_denies_all_origins_by_default() {
    // A safe default: a misconfigured/explicitly-empty allowlist must never
    // silently fall back to permissive CORS.
    let config = AppConfig {
        allowed_cors_origins: vec![],
        ..AppConfig::default()
    };
    let app = create_router(None, None).layer(build_cors_layer(&config));

    let mut request = health_request();
    request.headers_mut().insert(
        header::ORIGIN,
        "https://anything.example.com".parse().unwrap(),
    );

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "an empty allowlist must deny CORS approval, not fall back to permissive"
    );
}

#[tokio::test]
async fn test_default_config_only_allows_the_web_app_dev_origin() {
    let app = create_router(None, None).layer(build_cors_layer(&AppConfig::default()));

    let mut request = health_request();
    request
        .headers_mut()
        .insert(header::ORIGIN, "http://localhost:5173".parse().unwrap());

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap(),
        "http://localhost:5173"
    );
}

#[tokio::test]
async fn test_quote_endpoint_returns_429_with_retry_after_once_over_budget() {
    let config = AppConfig {
        rate_limit_quote_per_minute: 2,
        rate_limit_global_per_minute: 1_000,
        ..AppConfig::default()
    };
    let app = router_with_config(config);

    for _ in 0..2 {
        let response = app.clone().oneshot(quote_request()).await.unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    let response = app.oneshot(quote_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        response.headers().get(header::RETRY_AFTER).is_some(),
        "a 429 must carry a Retry-After header"
    );
}

#[tokio::test]
async fn test_global_rate_limit_covers_routes_without_their_own_budget() {
    let config = AppConfig {
        rate_limit_global_per_minute: 2,
        rate_limit_quote_per_minute: 1_000,
        ..AppConfig::default()
    };
    let app = router_with_config(config);

    for _ in 0..2 {
        let response = app.clone().oneshot(health_request()).await.unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    let response = app.oneshot(health_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_requests_under_budget_are_never_rate_limited() {
    let config = AppConfig {
        rate_limit_quote_per_minute: 5,
        rate_limit_global_per_minute: 1_000,
        ..AppConfig::default()
    };
    let app = router_with_config(config);

    for _ in 0..5 {
        let response = app.clone().oneshot(quote_request()).await.unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}

#[tokio::test]
async fn test_spoofed_x_forwarded_for_does_not_bypass_rate_limit_by_default() {
    // trust_proxy_headers defaults to false: an attacker rotating
    // X-Forwarded-For on every request must NOT get a fresh bucket each
    // time — without a real proxy in front of us, that header is
    // attacker-controlled and must be ignored.
    let config = AppConfig {
        rate_limit_quote_per_minute: 2,
        rate_limit_global_per_minute: 1_000,
        trust_proxy_headers: false,
        ..AppConfig::default()
    };
    let app = router_with_config(config);

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(quote_request_with_xff(Some("1.2.3.4")))
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // A different spoofed IP on the very next request still hits the same
    // shared bucket, since the header isn't trusted.
    let response = app
        .oneshot(quote_request_with_xff(Some("9.9.9.9")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_x_forwarded_for_is_honored_only_when_trust_proxy_headers_is_set() {
    let config = AppConfig {
        rate_limit_quote_per_minute: 1,
        rate_limit_global_per_minute: 1_000,
        trust_proxy_headers: true,
        ..AppConfig::default()
    };
    let app = router_with_config(config);

    // Each distinct (trusted) client IP gets its own budget.
    let response_a = app
        .clone()
        .oneshot(quote_request_with_xff(Some("1.1.1.1")))
        .await
        .unwrap();
    assert_ne!(response_a.status(), StatusCode::TOO_MANY_REQUESTS);

    let response_b = app
        .clone()
        .oneshot(quote_request_with_xff(Some("2.2.2.2")))
        .await
        .unwrap();
    assert_ne!(response_b.status(), StatusCode::TOO_MANY_REQUESTS);

    // But a repeat from the same trusted IP is still limited.
    let response_a_again = app
        .oneshot(quote_request_with_xff(Some("1.1.1.1")))
        .await
        .unwrap();
    assert_eq!(response_a_again.status(), StatusCode::TOO_MANY_REQUESTS);
}
