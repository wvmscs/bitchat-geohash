//! Watches the BitChat location channels inside a geohash, live, in the
//! terminal: plan, relay verdicts, then the active cells, refreshed.
//!
//!   cargo run -p bitchat-geohash --example watch -- u09 --depth 2 \
//!       [--levels 2,4,5,6] [--max-cells 2048] [--window 15m] [--inactivity 15m] \
//!       [--relays-csv <url|path>] [--android-csv <url|path> | --no-android] \
//!       [--no-probe] [--db bitchat-geohash.sqlite] [--reinstate-all] \
//!       [--refresh 5s] [--duration 20m] [--verbose]
//!
//! * Relay lists: the apps' official ones by default (iOS `--relays-csv`,
//!   Android `--android-csv`), reloaded every 24 h, with the last valid one
//!   in the database as fallback.
//! * Read-only: nothing is published, AUTH is never answered.
//! * Channels silent for `--inactivity` are retired (INFO line on stderr)
//!   and stay retired across runs (same `--db`) until reinstated.
//! * Commands on stdin: `reinstate <geohash>...`, `reinstate all`,
//!   `status`, `quit`.
//! * The table shows counters only: no pubkey, no nickname, no content.

use std::time::{Duration, Instant};

use bitchat_geohash::aggregate::CellStats;
use bitchat_geohash::channels::Level;
use bitchat_geohash::directory::ListSource;
use bitchat_geohash::listener::ConnState;
use bitchat_geohash::monitor::{ListSources, Monitor, MonitorConfig, MonitorStatus};
use bitchat_geohash::nostr::unix_now;
use bitchat_geohash::probe::Status;
use bitchat_geohash::store::Store;
use tokio::sync::mpsc;

struct Args {
    config: MonitorConfig,
    sources: ListSources,
    db: String,
    refresh: Duration,
    duration: Option<Duration>,
    verbose: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage: watch <geohash> [--depth N] [--levels 2,4,5,6] [--max-cells N] [--window 15m] \
         [--inactivity 15m] [--relays-csv <url|path>] [--android-csv <url|path> | --no-android] \
         [--no-probe] [--db <path>] [--reinstate-all] [--refresh 5s] [--duration <d>] [--verbose]"
    );
    std::process::exit(2)
}

/// `90`, `90s`, `5m`, `2h`.
fn duration(s: &str) -> Duration {
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = n.parse().unwrap_or_else(|_| usage());
    Duration::from_secs(match unit {
        "" | "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => usage(),
    })
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let mut start = None;
    let mut config = MonitorConfig::new("");
    let mut sources = ListSources::official();
    let mut db = "bitchat-geohash.sqlite".to_owned();
    let (mut refresh, mut duration_limit, mut verbose) = (Duration::from_secs(5), None, false);
    while let Some(arg) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--depth" => config.depth = value().parse().unwrap_or_else(|_| usage()),
            "--levels" => {
                config.levels = value()
                    .split(',')
                    .map(|p| {
                        p.trim()
                            .parse()
                            .ok()
                            .and_then(Level::from_precision)
                            .unwrap_or_else(|| usage())
                    })
                    .collect()
            }
            "--max-cells" => config.max_channels = value().parse().unwrap_or_else(|_| usage()),
            "--window" => config.aggregator.window_secs = duration(&value()).as_secs() as i64,
            "--inactivity" => {
                config.aggregator.inactivity_secs = duration(&value()).as_secs() as i64
            }
            "--relays-csv" => sources.ios = Some(ListSource::parse(&value())),
            "--android-csv" => sources.android = Some(ListSource::parse(&value())),
            "--no-android" => sources.android = None,
            "--no-probe" => config.probe = false,
            "--db" => db = value(),
            "--reinstate-all" => config.reinstate_all = true,
            "--refresh" => refresh = duration(&value()),
            "--duration" => duration_limit = Some(duration(&value())),
            "--verbose" => verbose = true,
            _ if arg.starts_with("--") => usage(),
            _ if start.is_none() => start = Some(arg),
            _ => usage(),
        }
    }
    config.start = start.unwrap_or_else(|| usage());
    Args {
        config,
        sources,
        db,
        refresh,
        duration: duration_limit,
        verbose,
    }
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    tracing_subscriber::fmt()
        .with_max_level(if args.verbose {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        })
        .with_writer(std::io::stderr)
        .init();
    bitchat_geohash::install_crypto_provider();

    let started = Instant::now();
    let store = Store::open(&args.db).unwrap_or_else(|e| {
        eprintln!("cannot open {}: {e}", args.db);
        std::process::exit(1)
    });
    let monitor =
        match Monitor::start_from_sources(args.config.clone(), args.sources.clone(), store).await {
            Ok(m) => m,
            Err(e) => {
                eprintln!("cannot start: {e}");
                std::process::exit(1)
            }
        };
    println!(
        "started in {:.1} s (plan, probes, connections)",
        started.elapsed().as_secs_f64()
    );
    let status = monitor.status().await.expect("monitor");
    let snapshot = monitor.snapshot().await.expect("monitor");
    print_plan(&status, &snapshot);

    // stdin is blocking: read it on a thread.
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if cmd_tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut ticker = tokio::time::interval(args.refresh);
    let deadline = args.duration.map(|d| tokio::time::Instant::now() + d);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let status = monitor.status().await.expect("monitor");
                let snapshot = monitor.snapshot().await.expect("monitor");
                print_table(&status, &snapshot);
            }
            Some(line) = cmd_rx.recv() => {
                let words: Vec<&str> = line.split_whitespace().collect();
                match words.as_slice() {
                    ["quit"] | ["exit"] => break,
                    ["status"] => print_plan(&monitor.status().await.expect("monitor"), &monitor.snapshot().await.expect("monitor")),
                    ["reinstate", "all"] => println!("reinstated: {:?}", monitor.reinstate_all().await.expect("monitor")),
                    ["reinstate", rest @ ..] if !rest.is_empty() => {
                        let back = monitor.reinstate(rest.iter().map(|s| s.to_lowercase()).collect()).await.expect("monitor");
                        println!("reinstated: {back:?}");
                    }
                    [] => {}
                    _ => println!("commands: reinstate <geohash>... | reinstate all | status | quit"),
                }
            }
            _ = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            } => break,
        }
    }
    let status = monitor.status().await.expect("monitor");
    print_summary(&status, started.elapsed());
    if let Err(e) = monitor.shutdown().await {
        eprintln!("shutdown: {e}");
    }
}

fn status_name(s: Option<Status>) -> &'static str {
    s.map_or("unprobed", Status::as_str)
}

fn print_plan(status: &MonitorStatus, snapshot: &[CellStats]) {
    let mut levels = std::collections::BTreeMap::<u8, usize>::new();
    for c in snapshot {
        *levels.entry(c.level).or_default() += 1;
    }
    println!(
        "\nplan: {} channels {:?} (by level), {} watched, {} retired",
        snapshot.len(),
        levels,
        status.watched.len(),
        status.retired.len()
    );
    // Relay -> (channels, iOS?, Android?, probe status).
    let mut relays = std::collections::BTreeMap::<&str, (usize, bool, bool, Option<Status>)>::new();
    for c in snapshot {
        for r in &c.relays {
            let e = relays.entry(&r.url).or_insert((0, false, false, r.status));
            e.0 += 1;
            e.1 |= r.ios.is_some();
            e.2 |= r.android.is_some();
        }
    }
    println!(
        "{:<46} {:>8}  {:<11} {:<9} listened",
        "relay", "channels", "apps", "probe"
    );
    for (url, (n, ios, android, st)) in &relays {
        let apps = match (ios, android) {
            (true, true) => "iOS+Android",
            (true, false) => "iOS",
            _ => "Android",
        };
        let listened = if status.relays.iter().any(|r| r == url) {
            "yes"
        } else {
            "no"
        };
        println!(
            "{url:<46} {n:>8}  {apps:<11} {:<9} {listened}",
            status_name(*st)
        );
    }
    let divergent = snapshot.iter().filter(|c| !c.divergence.is_empty()).count();
    println!(
        "{} relays, {} listened, {} set aside; {} of {} channels with relays chosen by one app only\n",
        relays.len(),
        status.relays.len(),
        status.dropped_relays.len(),
        divergent,
        snapshot.len()
    );
}

fn connections(status: &MonitorStatus) -> (usize, usize) {
    let up = status
        .connections
        .iter()
        .filter(|c| c.state == ConnState::Connected)
        .count();
    (up, status.connections.len())
}

fn print_table(status: &MonitorStatus, snapshot: &[CellStats]) {
    let now = unix_now();
    let (up, total) = connections(status);
    let rejected: u64 = status.rejected.values().sum();
    println!(
        "--- {} | watched {} retired {} | connections {up}/{total} | rejected {rejected}",
        hms(now),
        status.watched.len(),
        status.retired.len()
    );
    let active: Vec<&CellStats> = snapshot
        .iter()
        .filter(|c| c.messages + c.presences > 0)
        .collect();
    if active.is_empty() {
        return;
    }
    println!(
        "{:<8} {:>3} {:>5} {:>5} {:>6} {:>8} {:>9} {:>6}",
        "cell", "lvl", "msgs", "pres", "people", "last", "relays", "diverg"
    );
    for c in active {
        let people = c.participants.map_or("low".to_owned(), |p| p.to_string());
        let last = c
            .last_seen
            .map_or("-".to_owned(), |t| format!("{}s ago", (now - t).max(0)));
        let delivered = c.relays.iter().filter(|r| r.delivered).count();
        println!(
            "{:<8} {:>3} {:>5} {:>5} {:>6} {:>8} {:>4}/{:<4} {:>6}",
            c.geohash,
            c.level,
            c.messages,
            c.presences,
            people,
            last,
            delivered,
            c.relays.len(),
            c.divergence.len()
        );
    }
}

fn print_summary(status: &MonitorStatus, elapsed: Duration) {
    let (up, total) = connections(status);
    println!(
        "\nafter {} s: watched {}, retired {}, relays listened {}, set aside {}, connections {up}/{total}, rejected {:?}",
        elapsed.as_secs(),
        status.watched.len(),
        status.retired.len(),
        status.relays.len(),
        status.dropped_relays.len(),
        status.rejected
    );
}

fn hms(t: i64) -> String {
    let s = t.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02} UTC", s / 3600, s % 3600 / 60, s % 60)
}
