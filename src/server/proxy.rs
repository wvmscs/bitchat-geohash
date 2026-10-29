// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Behind a TLS proxy (D25): the server speaks clear HTTP to a proxy
//! (Cloudflare Tunnel, NGINX, Caddy…) that serves the public name in HTTPS.
//! Every request must come from the proxy's address and say it arrived in
//! HTTPS (`X-Forwarded-Proto`, `Forwarded` or Cloudflare's `CF-Visitor`);
//! `Host` and `Origin` are those of the public name. At start, a probe asks
//! the public name for [`CHECK_PATH`] and expects this server's token back:
//! the proxy exists and leads here.

use std::net::IpAddr;
use std::time::Duration;

use axum::http::HeaderMap;

use crate::error::{Error, Result};

/// Answered with the server's token once the request passed the checks.
pub(crate) const CHECK_PATH: &str = "/.well-known/bitchat-proxy-check";

/// How long the start probe waits for the public name.
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// Settings of the mode behind a proxy.
#[derive(Debug, Clone, PartialEq)]
pub struct ProxyConfig {
    /// The public origin, `https://name` or `https://name:port`.
    pub public: String,
    /// The addresses requests may come from (default: the loopback ones,
    /// a proxy on the same machine).
    pub from: Vec<Net>,
    /// Probe the public name at start (default on).
    pub check: bool,
}

impl ProxyConfig {
    /// `public` is checked: `https`, a host, no path.
    pub fn new(public: &str) -> Result<Self> {
        Ok(ProxyConfig {
            public: Public::parse(public)?.origin,
            from: vec![Net::parse("127.0.0.1/8")?, Net::parse("::1")?],
            check: true,
        })
    }
}

/// An address or a network (`10.0.0.0/8`, `fd00::/8`, `192.168.1.2`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Net {
    addr: IpAddr,
    prefix: u8,
}

impl Net {
    pub fn parse(s: &str) -> Result<Self> {
        let bad = || Error::Proxy(format!("{s:?} is not an address or a network (a.b.c.d/n)"));
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p.parse::<u8>().map_err(|_| bad())?)),
            None => (s, None),
        };
        let addr: IpAddr = addr.parse().map_err(|_| bad())?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        if prefix > max {
            return Err(bad());
        }
        Ok(Net { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        // A listener on `[::]` sees IPv4 peers as `::ffff:a.b.c.d`.
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(n), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(n) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(n) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// The public name, as the browser sees it.
#[derive(Debug, Clone)]
pub(crate) struct Public {
    /// `https://name[:port]`, lowercase, without the default port.
    pub(crate) origin: String,
    /// `name[:port]`: the accepted `Host`.
    authority: String,
}

impl Public {
    pub(crate) fn parse(s: &str) -> Result<Self> {
        let bad =
            |why: &str| Error::Proxy(format!("{s:?}: {why} (https://name or https://name:port)"));
        let rest = s
            .strip_prefix("https://")
            .ok_or_else(|| bad("the public address is in HTTPS"))?;
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.is_empty() || rest.contains(['/', '?', '#', '@', ' ']) {
            return Err(bad("a name only, no path"));
        }
        let lower = rest.to_ascii_lowercase();
        let authority = match lower.strip_suffix(":443") {
            Some(name) => name.to_owned(),
            None => lower,
        };
        if let Some((_, port)) = authority.rsplit_once(':')
            && !authority.ends_with(']')
            && port.parse::<u16>().is_err()
        {
            return Err(bad("wrong port"));
        }
        Ok(Public {
            origin: format!("https://{authority}"),
            authority,
        })
    }

    /// `Host` (and `X-Forwarded-Host` when the proxy sets it) is the public
    /// name; `name:443` is `name`.
    pub(crate) fn host_ok(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let host = host.strip_suffix(":443").unwrap_or(&host);
        host == self.authority
    }
}

/// The request reached the proxy in HTTPS: at least one of the headers says
/// so, and none says otherwise (each hop of a chain of proxies).
pub(crate) fn forwarded_https(headers: &HeaderMap) -> bool {
    let mut schemes = Vec::new();
    for v in headers.get_all("x-forwarded-proto") {
        let v = v.to_str().unwrap_or("?");
        schemes.extend(v.split(',').map(|s| s.trim().to_ascii_lowercase()));
    }
    for v in headers.get_all("forwarded") {
        let v = v.to_str().unwrap_or("proto=?");
        for part in v.split([',', ';']) {
            if let Some((k, val)) = part.trim().split_once('=')
                && k.trim().eq_ignore_ascii_case("proto")
            {
                schemes.push(val.trim().trim_matches('"').to_ascii_lowercase());
            }
        }
    }
    for v in headers.get_all("cf-visitor") {
        let scheme = v
            .to_str()
            .ok()
            .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
            .and_then(|j| j.get("scheme")?.as_str().map(str::to_ascii_lowercase));
        schemes.push(scheme.unwrap_or_else(|| "?".into()));
    }
    !schemes.is_empty() && schemes.iter().all(|s| s == "https")
}

/// A token for the start probe: different at each start.
pub(crate) fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for i in 0..2u8 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u8(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Asks the public name for [`CHECK_PATH`]: the answer must be `token`, so
/// the request went through the proxy, in HTTPS, to this server.
pub(crate) async fn probe(origin: &str, token: &str) -> Result<()> {
    let url = format!("{origin}{CHECK_PATH}");
    let fail = |why: String| {
        Error::Proxy(format!(
            "{url}: {why}; is the proxy running and leading here? (--no-proxy-check to start anyway)"
        ))
    };
    let client = reqwest::Client::builder()
        .timeout(CHECK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| fail(e.to_string()))?;
    let resp = client.get(&url).send().await.map_err(|e| {
        let mut why = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            why = format!("{why}: {s}");
            src = s.source();
        }
        fail(why)
    })?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(fail(format!("answered {status}")));
    }
    if body.trim() != token {
        return Err(fail(
            "answered, but not this server (another one behind the proxy?)".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn public_names() {
        let p = Public::parse("https://Map.Example.org/").unwrap();
        assert_eq!(p.origin, "https://map.example.org");
        assert!(p.host_ok("map.example.org") && p.host_ok("MAP.example.org:443"));
        assert!(!p.host_ok("map.example.org:8443") && !p.host_ok("127.0.0.1:8080"));
        let p = Public::parse("https://map.example.org:8443").unwrap();
        assert_eq!(p.origin, "https://map.example.org:8443");
        assert!(p.host_ok("map.example.org:8443") && !p.host_ok("map.example.org"));
        assert!(
            Public::parse("https://map.example.org:443")
                .unwrap()
                .host_ok("map.example.org")
        );
        for bad in [
            "http://map.example.org",
            "https://",
            "https://x/y",
            "https://x:port",
            "map",
        ] {
            assert!(Public::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn networks() {
        let lo = Net::parse("127.0.0.1/8").unwrap();
        assert!(lo.contains("127.1.2.3".parse().unwrap()));
        assert!(lo.contains("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!lo.contains("10.0.0.1".parse().unwrap()));
        assert!(!lo.contains("::1".parse().unwrap()));
        let one = Net::parse("192.168.1.2").unwrap();
        assert!(one.contains("192.168.1.2".parse().unwrap()));
        assert!(!one.contains("192.168.1.3".parse().unwrap()));
        let v6 = Net::parse("fd00::/8").unwrap();
        assert!(v6.contains("fd12::1".parse().unwrap()));
        assert!(
            Net::parse("0.0.0.0/0")
                .unwrap()
                .contains("8.8.8.8".parse().unwrap())
        );
        for bad in ["10.0.0.0/33", "::/129", "x", "10.0.0.0/"] {
            assert!(Net::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn https_upstream() {
        assert!(forwarded_https(&headers(&[("x-forwarded-proto", "https")])));
        assert!(forwarded_https(&headers(&[(
            "forwarded",
            "for=1.2.3.4;proto=https;host=x"
        )])));
        assert!(forwarded_https(&headers(&[(
            "cf-visitor",
            r#"{"scheme":"https"}"#
        )])));
        assert!(forwarded_https(&headers(&[
            ("x-forwarded-proto", "https"),
            ("cf-visitor", r#"{"scheme":"https"}"#)
        ])));
        assert!(!forwarded_https(&headers(&[])));
        assert!(!forwarded_https(&headers(&[("x-forwarded-proto", "http")])));
        assert!(!forwarded_https(&headers(&[(
            "x-forwarded-proto",
            "https, http"
        )])));
        assert!(!forwarded_https(&headers(&[
            ("x-forwarded-proto", "https"),
            ("cf-visitor", r#"{"scheme":"http"}"#)
        ])));
        assert!(!forwarded_https(&headers(&[("forwarded", "for=1.2.3.4")])));
        assert!(!forwarded_https(&headers(&[("cf-visitor", "nonsense")])));
    }

    #[test]
    fn tokens_differ() {
        let (a, b) = (token(), token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
