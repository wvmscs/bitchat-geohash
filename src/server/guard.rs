//! Request checks and security headers (D13): `Host` (DNS rebinding),
//! `Origin` and `Content-Type` of commands, CSP.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::api::ApiError;

pub(crate) struct Guard {
    /// Accepted `Host` values; `None` accepts any (unspecified listen
    /// address, `allow_remote` only).
    hosts: Option<Vec<String>>,
    csp: HeaderValue,
}

impl Guard {
    pub(crate) fn new(addr: SocketAddr, tiles: &str) -> Self {
        Guard {
            hosts: allowed_hosts(addr),
            csp: HeaderValue::from_str(&csp(tiles))
                .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'")),
        }
    }
}

/// The names a browser may use for the server: its address, and
/// `localhost` when it listens on a loopback address.
fn allowed_hosts(addr: SocketAddr) -> Option<Vec<String>> {
    let ip = addr.ip();
    if ip.is_unspecified() {
        return None;
    }
    let port = addr.port();
    let mut hosts = vec![match addr {
        SocketAddr::V4(_) => format!("{ip}:{port}"),
        SocketAddr::V6(_) => format!("[{ip}]:{port}"),
    }];
    if ip.is_loopback() {
        hosts.push(format!("localhost:{port}"));
    }
    Some(hosts)
}

/// Scripts, styles and data from the server only; images from the server
/// and the tile host.
fn csp(tiles: &str) -> String {
    format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' {}; \
         connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        tile_origin(tiles)
    )
}

/// `https://{s}.tile.example/{z}/{x}/{y}.png` -> `https://*.tile.example`.
fn tile_origin(tiles: &str) -> String {
    let (scheme, rest) = tiles.split_once("://").unwrap_or(("https", tiles));
    let host = rest.split('/').next().unwrap_or_default();
    let host = host.replace("{s}", "*");
    // Anything else in braces would not be a valid source: drop the host.
    if host.is_empty() || host.contains(['{', '}', ' ', ';', '\'']) {
        return String::new();
    }
    format!("{scheme}://{host}")
}

pub(crate) async fn guard(State(g): State<Arc<Guard>>, req: Request, next: Next) -> Response {
    // HTTP/2 carries the host in the URI (`:authority`), not in `Host`.
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_owned)
        .or_else(|| req.uri().authority().map(|a| a.to_string()));
    if let Some(hosts) = &g.hosts
        && !host
            .as_deref()
            .is_some_and(|h| hosts.iter().any(|a| a.eq_ignore_ascii_case(h)))
    {
        return refuse(&req, StatusCode::MISDIRECTED_REQUEST, "wrong_host");
    }
    if req.method() == Method::POST {
        let json = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"));
        if !json {
            return refuse(&req, StatusCode::UNSUPPORTED_MEDIA_TYPE, "not_json");
        }
        // Another page open in the browser cannot command the monitor: its
        // requests carry its own origin.
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok());
        let expected = host.as_deref().map(|h| format!("http://{h}"));
        if origin.is_none() || origin.map(str::to_owned) != expected {
            return refuse(&req, StatusCode::FORBIDDEN, "wrong_origin");
        }
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, g.csp.clone());
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

fn refuse(req: &Request, status: StatusCode, code: &'static str) -> Response {
    tracing::warn!(method = %req.method(), path = req.uri().path(), reason = code, "request refused");
    ApiError::new(status, code, code.replace('_', " ")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_tile_origin() {
        assert_eq!(
            allowed_hosts("127.0.0.1:8080".parse().unwrap()),
            Some(vec!["127.0.0.1:8080".into(), "localhost:8080".into()])
        );
        assert_eq!(
            allowed_hosts("[::1]:80".parse().unwrap()),
            Some(vec!["[::1]:80".into(), "localhost:80".into()])
        );
        assert_eq!(
            allowed_hosts("192.168.1.2:8080".parse().unwrap()),
            Some(vec!["192.168.1.2:8080".into()])
        );
        assert_eq!(allowed_hosts("0.0.0.0:8080".parse().unwrap()), None);
        assert_eq!(
            tile_origin("https://tile.openstreetmap.org/{z}/{x}/{y}.png"),
            "https://tile.openstreetmap.org"
        );
        assert_eq!(
            tile_origin("https://{s}.tile.example/{z}/{x}/{y}.png"),
            "https://*.tile.example"
        );
        assert_eq!(tile_origin("https://{bad}/x"), "");
    }
}
