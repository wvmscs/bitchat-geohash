// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! `bitchat-geohash`: live map of BitChat location-channel activity.
//!
//! Watches the channels inside a geohash and serves the live map (lot 17,
//! see `PLAN_ET2.md`). Options shared with the `watch` example come from
//! `bitchat_geohash::cli`; the server ones are read here.
//!
//!   cargo run -- <geohash> [--depth 1] [--levels 2,4,5,6] [--max-cells 2048] \
//!       [--window 15m] [--inactivity 15m] [--relays-csv <url|path>] \
//!       [--android-csv <url|path> | --no-android] [--no-probe] [--db bitchat-geohash.sqlite] \
//!       [--reinstate-all] [--forget-descents] [--listen 127.0.0.1:8080] [--allow-remote] \
//!       [--tls-cert cert.pem --tls-key key.pem | --insecure-remote | \
//!        --behind-proxy https://name [--proxy-from 127.0.0.1/8,::1] [--no-proxy-check]] \
//!       [--tiles <url> --tiles-attribution <html>] [--open] [--verbose]
//!
//! Exit code 2 for a wrong command line, 1 when the monitor or the server
//! cannot start; the message on stderr says why.

use std::net::SocketAddr;
use std::path::PathBuf;

use bitchat_geohash::Error;
use bitchat_geohash::cli::{self, MONITOR_USAGE, MonitorArgs};
use bitchat_geohash::monitor::{Monitor, MonitorConfig, MonitorStatus, RelayInfo};
use bitchat_geohash::server::{Net, ProxyConfig, Relaunch, ServerConfig, TlsFiles, serve_with};
use bitchat_geohash::store::Store;

const SERVER_USAGE: &str = "[--listen 127.0.0.1:8080] [--allow-remote] \
     [--tls-cert <pem> --tls-key <pem> | --insecure-remote | \
     --behind-proxy https://<name> [--proxy-from <addr|net>,...] [--no-proxy-check]] \
     [--tiles <url> --tiles-attribution <html>] [--open]";

struct Args {
    monitor: MonitorArgs,
    server: ServerConfig,
    open: bool,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut server = ServerConfig::default();
    let (mut cert, mut key) = (None::<PathBuf>, None::<PathBuf>);
    let (mut tiles, mut attribution) = (None, None);
    let mut open = false;
    let (mut public, mut from, mut no_check) = (None::<String>, None::<Vec<Net>>, false);
    let monitor = cli::parse(args, |flag, v| {
        match flag {
            "--listen" => server.listen = v.parsed::<SocketAddr>("address:port")?,
            "--allow-remote" => server.allow_remote = true,
            "--insecure-remote" => server.insecure_remote = true,
            "--tls-cert" => cert = Some(v.value()?.into()),
            "--tls-key" => key = Some(v.value()?.into()),
            "--behind-proxy" => public = Some(v.value()?),
            "--proxy-from" => {
                let list = v.value()?;
                let nets = list
                    .split(',')
                    .map(|n| Net::parse(n.trim()).map_err(|e| format!("--proxy-from: {e}")))
                    .collect::<Result<Vec<_>, _>>()?;
                from.get_or_insert_with(Vec::new).extend(nets);
            }
            "--no-proxy-check" => no_check = true,
            "--tiles" => tiles = Some(v.value()?),
            "--tiles-attribution" => attribution = Some(v.value()?),
            "--open" => open = true,
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    server.tls = match (cert, key) {
        (Some(cert), Some(key)) => Some(TlsFiles { cert, key }),
        (None, None) => None,
        _ => return Err("--tls-cert and --tls-key go together".into()),
    };
    server.proxy = match public {
        Some(public) => {
            if server.tls.is_some() || server.insecure_remote {
                return Err("--behind-proxy: the proxy does the TLS (no --tls-cert, \
                     --tls-key nor --insecure-remote)"
                    .into());
            }
            let mut p = ProxyConfig::new(&public).map_err(|e| format!("--behind-proxy: {e}"))?;
            if let Some(from) = from {
                p.from = from;
            }
            p.check = !no_check;
            Some(p)
        }
        None if from.is_some() || no_check => {
            return Err("--proxy-from and --no-proxy-check go with --behind-proxy".into());
        }
        None => None,
    };
    // The default attribution is OpenStreetMap's: other tiles need theirs.
    match (tiles, attribution) {
        (Some(t), Some(a)) => (server.tiles, server.attribution) = (t, a),
        (None, None) => {}
        _ => {
            return Err(
                "--tiles and --tiles-attribution go together (the tile provider's credit)".into(),
            );
        }
    }
    Ok(Args {
        monitor,
        server,
        open,
    })
}

/// The error in the command line's words.
fn explain(e: &Error, listen: SocketAddr) -> String {
    match e {
        Error::Io(io) if io.kind() == std::io::ErrorKind::AddrInUse => {
            format!("{listen} is already in use: choose another port with --listen")
        }
        Error::RemoteListen(_) => format!(
            "{listen} is not a loopback address: add --allow-remote to serve the map to other machines"
        ),
        Error::RemoteWithoutTls(_) => format!(
            "serving {listen} to other machines needs TLS (--tls-cert and --tls-key), \
             a TLS proxy in front (--behind-proxy), or --insecure-remote to accept clear text"
        ),
        Error::TooManyChannels { count, max } => format!(
            "{count} channels to watch, more than the maximum of {max}: lower --depth or \
             --levels, or raise --max-cells"
        ),
        Error::NoRelayList => "no relay list: the download failed and the database has none \
             (check the network, or give one with --relays-csv)"
            .into(),
        other => other.to_string(),
    }
}

fn fail(what: &str, e: impl std::fmt::Display) -> ! {
    eprintln!("bitchat-geohash: {what}: {e}");
    std::process::exit(1)
}

/// What is watched and through which relays, on stderr before the URL.
fn summary(config: &MonitorConfig, status: &MonitorStatus, relays: &[RelayInfo]) {
    let levels: Vec<String> = config
        .levels
        .iter()
        .map(|l| l.precision().to_string())
        .collect();
    eprintln!(
        "watching #{} (depth {}, levels {}): {} channels, {} retired",
        config.start,
        config.depth,
        levels.join(","),
        status.watched.len(),
        status.retired.len()
    );
    eprintln!(
        "relays: {} listened, {} set aside",
        status.relays.len(),
        status.dropped_relays.len()
    );
    for r in relays {
        let apps = match (r.ios, r.android) {
            (true, true) => "iOS+Android",
            (true, false) => "iOS",
            _ => "Android",
        };
        let status = r.status.map_or("unprobed", |s| s.as_str());
        let note = if r.listened {
            ""
        } else {
            "  (not listened to)"
        };
        eprintln!(
            "  {status:<9} {apps:<11} {:>5} channels  {}{note}",
            r.channels, r.url
        );
    }
}

/// Opens the map in the default browser.
fn open_browser(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(url).spawn() {
        tracing::warn!(error = %e, "cannot open the browser ({program})");
    }
}

#[tokio::main]
async fn main() {
    let args = parse_args(std::env::args().skip(1)).unwrap_or_else(|e| {
        eprintln!("bitchat-geohash: {e}\nusage: bitchat-geohash {MONITOR_USAGE} {SERVER_USAGE}");
        std::process::exit(2)
    });
    tracing_subscriber::fmt()
        .with_max_level(if args.monitor.verbose {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        })
        .with_writer(std::io::stderr)
        // No colour codes in a log file.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();
    bitchat_geohash::install_crypto_provider();
    let MonitorArgs {
        config,
        sources,
        db,
        ..
    } = args.monitor;
    let listen = args.server.listen;

    // A busy port is found now, not after the relay probes. Other bind
    // errors (a remote address without --allow-remote...) come from `serve`.
    if listen.port() != 0
        && let Err(e) = std::net::TcpListener::bind(listen)
        && e.kind() == std::io::ErrorKind::AddrInUse
    {
        fail("cannot serve the map", explain(&Error::Io(e), listen))
    }
    // Likewise a missing certificate or key: `serve` checks the contents.
    if let Some(tls) = &args.server.tls {
        for path in [&tls.cert, &tls.key] {
            if let Err(e) = std::fs::File::open(path) {
                let cwd = std::env::current_dir().unwrap_or_default();
                fail(
                    "cannot serve the map",
                    format!(
                        "cannot read {} ({e}); relative paths start from {}",
                        path.display(),
                        cwd.display()
                    ),
                )
            }
        }
    }
    let store = Store::open(&db).unwrap_or_else(|e| fail(&format!("cannot open {db}"), e));
    let monitor = Monitor::start_from_sources(config.clone(), sources.clone(), store)
        .await
        .unwrap_or_else(|e| fail("cannot start", explain(&e, listen)));
    match (monitor.status().await, monitor.relays().await) {
        (Ok(status), Ok(relays)) => summary(&config, &status, &relays),
        (Err(e), _) | (_, Err(e)) => fail("cannot start", e),
    }
    // The page can change the watched area (lot 19).
    let relaunch = Relaunch::Sources {
        sources,
        db: db.clone(),
    };
    let server = serve_with(monitor, &config, Some(relaunch), args.server)
        .await
        .unwrap_or_else(|e| fail("cannot serve the map", explain(&e, listen)));
    let url = format!("{}/", server.url());
    eprintln!("map: {url}");
    // Behind a proxy the browser uses the public name: the local URL is
    // refused there (wrong host, not from the proxy).
    let url = match server.public_url() {
        Some(public) => {
            eprintln!("public: {public}/");
            format!("{public}/")
        }
        None => url,
    };
    if args.open {
        open_browser(&url);
    }

    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("stopping");
    match server.shutdown().await {
        Ok(monitor) => {
            if let Err(e) = monitor.shutdown().await {
                fail("shutdown", e)
            }
        }
        Err(e) => fail("shutdown", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        parse_args(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn server_options() {
        let a = parse(
            "u09 --listen 0.0.0.0:9000 --allow-remote --tls-cert c.pem --tls-key k.pem \
             --tiles https://t/{z}/{x}/{y}.png --tiles-attribution T --open --depth 2",
        )
        .unwrap();
        assert_eq!(a.server.listen, "0.0.0.0:9000".parse().unwrap());
        assert!(a.server.allow_remote && a.open && !a.server.insecure_remote);
        let tls = a.server.tls.unwrap();
        assert_eq!((tls.cert, tls.key), ("c.pem".into(), "k.pem".into()));
        assert_eq!(
            (a.server.tiles.as_str(), a.server.attribution.as_str()),
            ("https://t/{z}/{x}/{y}.png", "T")
        );
        assert_eq!(a.monitor.config.depth, 2);
    }

    #[test]
    fn options_that_go_together() {
        let err = |s: &str| parse(s).err().unwrap();
        assert_eq!(
            err("u09 --tls-cert c.pem"),
            "--tls-cert and --tls-key go together"
        );
        assert!(err("u09 --tiles x").starts_with("--tiles and --tiles-attribution"));
        assert_eq!(
            err("u09 --listen localhost"),
            "--listen: address:port expected, got \"localhost\""
        );
        assert_eq!(err("u09 --refresh 5s"), "unknown option --refresh");
    }

    #[test]
    fn behind_a_proxy() {
        let a = parse("u09 --behind-proxy https://Map.Example.org").unwrap();
        let p = a.server.proxy.unwrap();
        assert_eq!(p.public, "https://map.example.org");
        assert_eq!(p.from, ProxyConfig::new("https://x").unwrap().from);
        assert!(p.check);
        let a = parse(
            "u09 --behind-proxy https://m.example.org:8443 --listen 0.0.0.0:8080 --allow-remote \
             --proxy-from 172.17.0.0/16,fd00::/8 --proxy-from 10.0.0.2 --no-proxy-check",
        )
        .unwrap();
        let p = a.server.proxy.unwrap();
        assert_eq!(p.public, "https://m.example.org:8443");
        assert_eq!(p.from.len(), 3);
        assert!(p.from[2].contains("10.0.0.2".parse().unwrap()));
        assert!(!p.check);

        let err = |s: &str| parse(s).err().unwrap();
        assert!(err("u09 --behind-proxy http://m.example.org").contains("HTTPS"));
        assert!(
            err("u09 --behind-proxy https://m --insecure-remote")
                .contains("the proxy does the TLS")
        );
        assert!(
            err("u09 --behind-proxy https://m --tls-cert c --tls-key k")
                .contains("the proxy does the TLS")
        );
        assert!(
            err("u09 --behind-proxy https://m --proxy-from 10.0.0.0/40").contains("--proxy-from")
        );
        assert!(err("u09 --no-proxy-check").contains("go with --behind-proxy"));
        assert!(err("u09 --proxy-from ::1").contains("go with --behind-proxy"));
    }

    #[test]
    fn errors_in_the_command_line_words() {
        let listen: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let busy = Error::Io(std::io::Error::from(std::io::ErrorKind::AddrInUse));
        assert!(explain(&busy, listen).contains("--listen"));
        let cap = Error::TooManyChannels {
            count: 4000,
            max: 2048,
        };
        assert!(explain(&cap, listen).contains("--max-cells"));
        assert!(explain(&Error::NoRelayList, listen).contains("--relays-csv"));
        assert!(explain(&Error::RemoteListen("x".into()), listen).contains("--allow-remote"));
        assert!(explain(&Error::InvalidGeohash("u0a".into()), listen).contains("invalid geohash"));
    }
}
