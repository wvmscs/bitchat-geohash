// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! Optional TLS (lot 13 bis, D17): a certificate and key given by the user
//! (`mkcert`, say), HTTP/2 or HTTP/1.1 negotiated by ALPN. HTTP/2 carries
//! every SSE stream of a browser on one connection, past the limit of 6
//! connections per host of HTTP/1.1.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

use crate::error::{Error, Result};

/// A certificate chain and its private key, PEM files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// A slow or silent client gives up its handshake after this.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct Tls {
    pub(crate) acceptor: TlsAcceptor,
    /// The server certificate, to check the names requests use.
    pub(crate) leaf: CertificateDer<'static>,
}

fn tls_error(path: &Path, what: impl std::fmt::Display) -> Error {
    Error::Tls(format!("{}: {what}", path.display()))
}

/// Reads the files and checks that the key matches the certificate.
pub(crate) fn load(files: &TlsFiles) -> Result<Tls> {
    crate::install_crypto_provider();
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&files.cert)
        .map_err(|e| tls_error(&files.cert, e))?
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| tls_error(&files.cert, e))?;
    let Some(leaf) = certs.first().cloned() else {
        return Err(tls_error(&files.cert, "no certificate in the file"));
    };
    let key = PrivateKeyDer::from_pem_file(&files.key).map_err(|e| tls_error(&files.key, e))?;
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| {
            Error::Tls(format!(
                "{} and {}: {e}",
                files.cert.display(),
                files.key.display()
            ))
        })?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Tls {
        acceptor: TlsAcceptor::from(Arc::new(config)),
        leaf,
    })
}

/// Whether the certificate is valid for `name` (a host name or an IP
/// address, without port or brackets).
pub(crate) fn covers(leaf: &CertificateDer<'_>, name: &str) -> bool {
    let Ok(server_name) = rustls_pki_types::ServerName::try_from(name) else {
        return false;
    };
    rustls::server::ParsedCertificate::try_from(leaf)
        .and_then(|cert| rustls::client::verify_server_name(&cert, &server_name))
        .is_ok()
}

/// Accepts TLS connections and serves `app` on each, HTTP/2 or HTTP/1.1,
/// until `stop`; then closes them gracefully.
pub(crate) async fn serve(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    app: Router,
    mut stop: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let (tcp, peer) = tokio::select! {
            _ = stop.wait_for(|s| *s) => break,
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(e) => {
                    tracing::debug!(error = %e, "accept failed");
                    continue;
                }
            },
        };
        let (acceptor, app, mut stop) = (acceptor.clone(), app.clone(), stop.clone());
        connections.spawn(async move {
            let tls = match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                Ok(Ok(tls)) => tls,
                Ok(Err(e)) => {
                    tracing::debug!(%peer, error = %e, "TLS handshake failed");
                    return;
                }
                Err(_) => {
                    tracing::debug!(%peer, "TLS handshake timed out");
                    return;
                }
            };
            let alpn = tls
                .get_ref()
                .1
                .alpn_protocol()
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .unwrap_or_default();
            tracing::debug!(%peer, alpn, "TLS connection");
            let builder = Builder::new(TokioExecutor::new());
            let conn = builder
                .serve_connection_with_upgrades(TokioIo::new(tls), TowerToHyperService::new(app));
            tokio::pin!(conn);
            tokio::select! {
                result = conn.as_mut() => {
                    if let Err(e) = result {
                        tracing::debug!(%peer, error = %e, "connection closed");
                    }
                }
                // The guard `wait_for` returns must not live across the
                // `await` below.
                _ = async { let _ = stop.wait_for(|s| *s).await; } => {
                    conn.as_mut().graceful_shutdown();
                    let _ = conn.await;
                }
            }
        });
    }
    while connections.join_next().await.is_some() {}
    Ok(())
}
