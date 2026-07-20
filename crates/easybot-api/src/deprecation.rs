//! Central API lifecycle policy and RFC-compliant response headers.

use axum::http::{HeaderName, HeaderValue, Method, Request};
use axum::middleware::Next;
use axum::response::Response;

#[derive(Debug)]
pub struct ApiDeprecation {
    pub method: Method,
    pub path: &'static str,
    /// RFC 9745 Structured Field date (Unix seconds).
    pub deprecated_at: i64,
    /// RFC 8594 HTTP-date.
    pub sunset: &'static str,
    pub documentation: &'static str,
    pub successor: Option<&'static str>,
}

/// No v1 operation is deprecated today. Add entries only with an approved
/// migration document and a future sunset date.
pub static API_DEPRECATIONS: &[ApiDeprecation] = &[];

fn apply_deprecation_headers(response: &mut Response, policy: &ApiDeprecation) {
    let mut links = format!("<{}>; rel=\"deprecation\"", policy.documentation);
    if let Some(successor) = policy.successor {
        links.push_str(&format!(", <{successor}>; rel=\"successor-version\""));
    }
    let values = [
        ("deprecation", format!("@{}", policy.deprecated_at)),
        ("sunset", policy.sunset.to_string()),
        ("link", links),
    ];
    for (name, value) in values {
        match (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            (Ok(name), Ok(value)) => {
                response.headers_mut().insert(name, value);
            }
            _ => tracing::error!(%name, "invalid static API deprecation policy header"),
        }
    }
}

pub async fn deprecation_middleware(request: Request<axum::body::Body>, next: Next) -> Response {
    let policy = API_DEPRECATIONS
        .iter()
        .find(|policy| policy.method == request.method() && policy.path == request.uri().path());
    let mut response = next.run(request).await;
    if let Some(policy) = policy {
        apply_deprecation_headers(&mut response, policy);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[test]
    fn emits_standard_deprecation_sunset_and_links() {
        let policy = ApiDeprecation {
            method: Method::GET,
            path: "/api/v1/example",
            deprecated_at: 1_800_000_000,
            sunset: "Tue, 19 Jan 2038 03:14:07 GMT",
            documentation: "https://docs.example.test/deprecations/example",
            successor: Some("/api/v2/example"),
        };
        let mut response = Response::new(Body::empty());
        apply_deprecation_headers(&mut response, &policy);
        assert_eq!(response.headers()["deprecation"], "@1800000000");
        assert_eq!(
            response.headers()["sunset"],
            "Tue, 19 Jan 2038 03:14:07 GMT"
        );
        assert_eq!(
            response.headers()["link"],
            "<https://docs.example.test/deprecations/example>; rel=\"deprecation\", </api/v2/example>; rel=\"successor-version\""
        );
    }
}
