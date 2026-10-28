// SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
// SPDX-License-Identifier: GPL-3.0-or-later

//! The `bitchat-geohash` binary: start on a local relay list and a local
//! relay, the map answers; clear messages and exit codes otherwise.
#![cfg(feature = "server")]

mod common;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::*;

const BIN: &str = env!("CARGO_BIN_EXE_bitchat-geohash");

/// A relay list file with only `relay`, removed on drop.
struct ListFile(std::path::PathBuf);

impl ListFile {
    fn new(name: &str, relay: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bitchat-geohash-cli-{name}-{}.csv",
            std::process::id()
        ));
        std::fs::write(
            &path,
            format!("Relay URL,Latitude,Longitude\n{relay},48.8566,2.3522\n"),
        )
        .unwrap();
        ListFile(path)
    }
}

impl Drop for ListFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Runs the binary offline: local list, no Android list, no probe.
fn command(list: &ListFile, db: &TempDb, args: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.args(args)
        .args(["--relays-csv", list.0.to_str().unwrap(), "--no-android"])
        .args(["--no-probe", "--db", db.0.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    c
}

/// Exit code and stderr of a run that must stop by itself.
fn run(list: &ListFile, db: &TempDb, args: &[&str]) -> (i32, String) {
    let out = command(list, db, args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Reads stderr until the `map: <url>` line; returns the URL and the lines.
fn map_url(child: &mut Child) -> (String, Vec<String>) {
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut lines = Vec::new();
    loop {
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("no map URL; stderr: {lines:#?}"));
        if let Some(url) = line.strip_prefix("map: ") {
            return (url.to_owned(), lines);
        }
        lines.push(line);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_binary_serves_the_map() {
    bitchat_geohash::install_crypto_provider();
    let relay = TestRelay::spawn("127.0.0.1:0").await.unwrap();
    let list = ListFile::new("serve", &relay.url());
    let db = TempDb::new("cli-serve");
    let mut child = command(&list, &db, &["u09t", "--listen", "127.0.0.1:0"])
        .spawn()
        .unwrap();
    let (url, lines) = map_url(&mut child);
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    let text = lines.join("\n");
    assert!(
        text.contains("watching #u09t (depth 1, levels 2,4,5,6): 33 channels, 0 retired"),
        "{text}"
    );
    assert!(text.contains("relays: 1 listened, 0 set aside"), "{text}");
    assert!(text.contains(&relay.url()), "{text}");

    let body = reqwest::get(format!("{url}api/state"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let state: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(state["cells"].as_array().unwrap().len(), 33);
    assert_eq!(state["params"]["start"], "u09t");

    // Ctrl-C: a clean stop.
    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
    relay.shutdown().await;
}

#[test]
fn wrong_command_lines_exit_with_2() {
    let list = ListFile::new("usage", "ws://127.0.0.1:9");
    let db = TempDb::new("cli-usage");
    let (code, err) = run(&list, &db, &["u09t", "--nope"]);
    assert_eq!(code, 2);
    assert!(err.contains("unknown option --nope"), "{err}");
    assert!(err.contains("usage: bitchat-geohash <geohash>"), "{err}");
    let (code, err) = run(&list, &db, &["u09t", "--tls-key", "k.pem"]);
    assert_eq!((code, err.contains("go together")), (2, true), "{err}");
}

#[test]
fn start_failures_say_why() {
    let list = ListFile::new("fail", "ws://127.0.0.1:9");
    let db = TempDb::new("cli-fail");
    let (code, err) = run(&list, &db, &["u0a", "--listen", "127.0.0.1:0"]);
    assert_eq!(code, 1);
    assert!(err.contains("invalid geohash \"u0a\""), "{err}");

    let (code, err) = run(
        &list,
        &db,
        &[
            "u09",
            "--depth",
            "2",
            "--max-cells",
            "100",
            "--listen",
            "127.0.0.1:0",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("more than the maximum of 100"), "{err}");
    assert!(err.contains("--max-cells"), "{err}");

    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = busy.local_addr().unwrap().to_string();
    let (code, err) = run(&list, &db, &["u09t", "--listen", &addr]);
    assert_eq!(code, 1);
    assert!(err.contains("already in use"), "{err}");

    let (code, err) = run(&list, &db, &["u09t", "--listen", "192.0.2.1:8080"]);
    assert_eq!(code, 1);
    assert!(err.contains("--allow-remote"), "{err}");

    // A missing certificate is found before the relay list is even read.
    let (code, err) = run(
        &list,
        &db,
        &[
            "u09t",
            "--listen",
            "127.0.0.1:0",
            "--tls-cert",
            "absent.pem",
            "--tls-key",
            "absent-key.pem",
        ],
    );
    assert_eq!(code, 1);
    assert!(err.contains("cannot read absent.pem"), "{err}");
    assert!(err.contains("relative paths start from"), "{err}");
    assert!(!err.contains("watching #"), "{err}");
}

#[test]
fn no_relay_list_is_an_error() {
    let list = ListFile::new("missing", "ws://127.0.0.1:9");
    let missing = ListFile(list.0.with_extension("absent"));
    let db = TempDb::new("cli-nolist");
    let (code, err) = run(&missing, &db, &["u09t", "--listen", "127.0.0.1:0"]);
    assert_eq!(code, 1);
    assert!(err.contains("no relay list"), "{err}");
    assert!(err.contains("--relays-csv"), "{err}");
}
