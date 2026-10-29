// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Request checks and security headers (D13): `Host` (DNS rebinding),
//! `Origin` and `Content-Type` of commands, CSP. With TLS, the names
//! accepted are those of the certificate (D17); behind a proxy, the public
//! name, and only from the proxy's address in HTTPS upstream (D25).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use rustls_pki_types::CertificateDer;

use super::api::ApiError;
use super::proxy::{self, Net, Public};

/// Behind a proxy (D25).
pub(crate) struct Behind {
    pub(crate) public: Public,
    pub(crate) from: Vec<Net>,
    /// Answered at [`proxy::CHECK_PATH`].
    pub(crate) token: String,
}

pub(crate) struct Guard {
    /// Without TLS, the accepted `Host` values; `None` accepts any
    /// (unspecified listen address, `allow_remote` only).
    hosts: Option<Vec<String>>,
    /// With TLS, the server certificate: a `Host` is accepted when the
    /// certificate covers its name and the port is the server's.
    leaf: Option<CertificateDer<'static>>,
    port: u16,
    behind: Option<Behind>,
    csp: HeaderValue,
}

impl Guard {
    pub(crate) fn new(
        addr: SocketAddr,
        tiles: &str,
        leaf: Option<CertificateDer<'static>>,
        behind: Option<Behind>,
    ) -> Self {
        Guard {
            hosts: allowed_hosts(addr),
            leaf,
            port: addr.port(),
            behind,
            csp: HeaderValue::from_str(&csp(tiles))
                .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'")),
        }
    }

    /// The `Origin` of the server's own page for `host`.
    fn origin(&self, host: &str) -> String {
        match (&self.behind, &self.leaf) {
            (Some(b), _) => b.public.origin.clone(),
            (None, Some(_)) => format!("https://{host}"),
            (None, None) => format!("http://{host}"),
        }
    }

    fn host_ok(&self, host: Option<&str>) -> bool {
        let Some(host) = host else { return false };
        if let Some(b) = &self.behind {
            return b.public.host_ok(host);
        }
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

/// Scripts, styles and data from the server only; images from the server,
/// the tile host, and `data:` (Leaflet's blank tile).
fn csp(tiles: &str) -> String {
    format!(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data: {}; \
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
    if let Some(b) = &g.behind {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip());
        if !peer.is_some_and(|ip| b.from.iter().any(|n| n.contains(ip))) {
            return refuse(&req, StatusCode::FORBIDDEN, "not_from_proxy");
        }
        if !proxy::forwarded_https(req.headers()) {
            return refuse(&req, StatusCode::FORBIDDEN, "not_https_upstream");
        }
        let forwarded_host = req
            .headers()
            .get("x-forwarded-host")
            .map(|h| h.to_str().unwrap_or_default());
        if forwarded_host.is_some_and(|h| !b.public.host_ok(h)) {
            return refuse(&req, StatusCode::MISDIRECTED_REQUEST, "wrong_host");
        }
    }
    if !g.host_ok(host.as_deref()) {
        return refuse(&req, StatusCode::MISDIRECTED_REQUEST, "wrong_host");
    }
    if let Some(b) = &g.behind
        && req.method() == Method::GET
        && req.uri().path() == proxy::CHECK_PATH
    {
        return b.token.clone().into_response();
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
        let expected = host.as_deref().map(|h| g.origin(h));
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
