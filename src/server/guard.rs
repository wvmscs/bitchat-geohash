//! Request checks and security headers (D13): `Host` (DNS rebinding),
//! `Origin` and `Content-Type` of commands, CSP. With TLS, the names
//! accepted are those of the certificate (D17).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use rustls_pki_types::CertificateDer;

use super::api::ApiError;

pub(crate) struct Guard {
    /// Without TLS, the accepted `Host` values; `None` accepts any
    /// (unspecified listen address, `allow_remote` only).
    hosts: Option<Vec<String>>,
    /// With TLS, the server certificate: a `Host` is accepted when the
    /// certificate covers its name and the port is the server's.
    leaf: Option<CertificateDer<'static>>,
    port: u16,
    csp: HeaderValue,
}

impl Guard {
    pub(crate) fn new(
        addr: SocketAddr,
        tiles: &str,
        leaf: Option<CertificateDer<'static>>,
    ) -> Self {
        Guard {
            hosts: allowed_hosts(addr),
            leaf,
            port: addr.port(),
            csp: HeaderValue::from_str(&csp(tiles))
                .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'")),
        }
    }

    fn scheme(&self) -> &'static str {
        if self.leaf.is_some() { "https" } else { "http" }
    }

    fn host_ok(&self, host: Option<&str>) -> bool {
        let Some(host) = host else { return false };
        match &self.leaf {
            Some(leaf) => {
                let (name, port) = split_host(host);
                port.unwrap_or(443) == self.port && super::tls::covers(leaf, name)
            }
            None => self
                .hosts
                .as_ref()
                .is_none_or(|hosts| hosts.iter().any(|a| a.eq_ignore_ascii_case(host))),
        }
    }
}

/// `name:port`, `[v6]:port` or `name` -> (name without brackets, port).
fn split_host(host: &str) -> (&str, Option<u16>) {
    if let Some(rest) = host.strip_prefix('[') {
        let (name, after) = rest.split_once(']').unwrap_or((rest, ""));
        return (name, after.strip_prefix(':').and_then(|p| p.parse().ok()));
    }
    match host.rsplit_once(':') {
        Some((name, port)) => (name, port.parse().ok()),
        None => (host, None),
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
    if !g.host_ok(host.as_deref()) {
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
        let expected = host.as_deref().map(|h| format!("{}://{h}", g.scheme()));
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
        assert_eq!(split_host("localhost:8443"), ("localhost", Some(8443)));
        assert_eq!(split_host("[::1]:8443"), ("::1", Some(8443)));
        assert_eq!(split_host("[::1]"), ("::1", None));
        assert_eq!(split_host("example.org"), ("example.org", None));
    }
}
