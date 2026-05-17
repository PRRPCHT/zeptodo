use axum::http::HeaderName;
use axum::http::HeaderValue;
use axum::http::header;
use tower_http::set_header::SetResponseHeaderLayer;

/// Content-Security-Policy applied to every response. Inline
/// styles are allowed because Alpine sets the `style` attribute at runtime
/// for `x-show`, `x-transition`, and `x-cloak`; banning them would break the
/// menu and theme toggle.
const CSP: &str = "default-src 'self'; \
script-src 'self'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data:; \
font-src 'self'; \
connect-src 'self'; \
form-action 'self'; \
base-uri 'self'; \
frame-ancestors 'none'; \
object-src 'none'";

/// Build the `Content-Security-Policy` response-header layer.
///
/// ### Returns
/// - The layer that sets `Content-Security-Policy` on every response.
pub fn csp_layer() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    )
}

/// Build the `X-Content-Type-Options: nosniff` response-header layer.
///
/// ### Returns
/// - The layer that sets `X-Content-Type-Options: nosniff`.
pub fn nosniff_layer() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    )
}

/// Build the `X-Frame-Options: DENY` response-header layer.
///
/// ### Returns
/// - The layer that sets `X-Frame-Options: DENY`.
pub fn frame_options_layer() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    )
}

/// Build the `Referrer-Policy: same-origin` response-header layer.
///
/// ### Returns
/// - The layer that sets `Referrer-Policy: same-origin`.
pub fn referrer_policy_layer() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    )
}

/// Build the `Permissions-Policy: ()` response-header layer.
///
/// ### Returns
/// - The layer that sets `Permissions-Policy` to the empty allowlist.
pub fn permissions_policy_layer() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::if_not_present(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static(
            "accelerometer=(), camera=(), geolocation=(), gyroscope=(), \
             magnetometer=(), microphone=(), payment=(), usb=(), interest-cohort=()",
        ),
    )
}

/// Build the `Strict-Transport-Security` response-header layer, when applicable.
///
/// ### Arguments
/// - `cookies_secure`: Whether the deployment is served over HTTPS.
///
/// ### Returns
/// - `Some(layer)`: HSTS for 1 year with `includeSubDomains`, when on HTTPS.
/// - `None`: HSTS is omitted on plain HTTP origins.
pub fn hsts_layer(cookies_secure: bool) -> Option<SetResponseHeaderLayer<HeaderValue>> {
    if cookies_secure {
        Some(SetResponseHeaderLayer::if_not_present(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn router_with_layers(cookies_secure: bool) -> Router {
        let mut router = Router::new().route("/", get(ok_handler));
        router = router
            .layer(csp_layer())
            .layer(nosniff_layer())
            .layer(frame_options_layer())
            .layer(referrer_policy_layer())
            .layer(permissions_policy_layer());
        if let Some(layer) = hsts_layer(cookies_secure) {
            router = router.layer(layer);
        }
        router
    }

    #[tokio::test]
    async fn applies_baseline_headers() {
        let response = router_with_layers(false)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        let csp = headers
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("CSP header present");
        let csp = csp.to_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("script-src 'self'"));
        assert!(!csp.contains("'unsafe-eval'"));
        assert!(!csp.split(';').any(|d| {
            let d = d.trim();
            d.starts_with("script-src") && d.contains("'unsafe-inline'")
        }));
        assert_eq!(
            headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
        assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "same-origin");
        assert!(headers.contains_key("permissions-policy"));
        assert!(!headers.contains_key(header::STRICT_TRANSPORT_SECURITY));
    }

    #[tokio::test]
    async fn hsts_only_on_https() {
        let response = router_with_layers(true)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let hsts = response
            .headers()
            .get(header::STRICT_TRANSPORT_SECURITY)
            .expect("HSTS present on HTTPS deployments");
        assert!(hsts.to_str().unwrap().contains("max-age="));
    }
}
