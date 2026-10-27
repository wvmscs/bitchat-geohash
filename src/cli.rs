//! Command-line options shared by the `bitchat-geohash` binary and the
//! `watch` example (lot 17), so that both read them the same way. No
//! dependency: a hand-written loop over the arguments.
//!
//! [`parse`] reads the monitor options and hands every other option to the
//! caller's `extra` function, which consumes its own (`--listen`,
//! `--refresh`, ...) and says whether it knew it.

use std::time::Duration;

use crate::channels::Level;
use crate::directory::ListSource;
use crate::monitor::{ListSources, MonitorConfig};

/// The monitor options common to the binary and `watch`.
#[derive(Clone)]
pub struct MonitorArgs {
    pub config: MonitorConfig,
    pub sources: ListSources,
    pub db: String,
    pub verbose: bool,
}

/// Usage of the common options, for the callers' usage lines.
pub const MONITOR_USAGE: &str = "<geohash> [--depth N] [--levels 2,4,5,6] [--max-cells N] \
     [--window 15m] [--inactivity 15m] [--relays-csv <url|path>] \
     [--android-csv <url|path> | --no-android] [--no-probe] [--db <path>] [--reinstate-all] \
     [--forget-descents] [--verbose]";

/// The value of the option being read.
pub struct Values<'a> {
    flag: &'a str,
    it: &'a mut dyn Iterator<Item = String>,
}

impl Values<'_> {
    /// The next argument, which the option needs.
    pub fn value(&mut self) -> Result<String, String> {
        self.it
            .next()
            .ok_or_else(|| format!("{} needs a value", self.flag))
    }

    /// The next argument, parsed.
    pub fn parsed<T: std::str::FromStr>(&mut self, what: &str) -> Result<T, String> {
        let v = self.value()?;
        v.parse()
            .map_err(|_| format!("{}: {what} expected, got {v:?}", self.flag))
    }

    /// The next argument, as a duration.
    pub fn duration(&mut self) -> Result<Duration, String> {
        let v = self.value()?;
        duration(&v).ok_or_else(|| {
            format!(
                "{}: duration expected (90, 90s, 15m, 2h), got {v:?}",
                self.flag
            )
        })
    }
}

/// `90`, `90s`, `15m`, `2h`.
pub fn duration(s: &str) -> Option<Duration> {
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = n.parse().ok()?;
    Some(Duration::from_secs(match unit {
        "" | "s" => n,
        "m" => n.checked_mul(60)?,
        "h" => n.checked_mul(3600)?,
        _ => return None,
    }))
}

/// `4,5,6` -> channel levels.
fn levels(s: &str) -> Result<Vec<Level>, String> {
    s.split(',')
        .map(|p| {
            p.trim()
                .parse()
                .ok()
                .and_then(Level::from_precision)
                .ok_or_else(|| format!("--levels: channel levels are 2, 4, 5, 6, 7; got {p:?}"))
        })
        .collect()
}

/// Reads the arguments (without the program name). An option the monitor
/// does not know goes to `extra(flag, values)`, which returns `Ok(true)` if
/// it took it. The message of an error is for the user.
pub fn parse(
    args: impl IntoIterator<Item = String>,
    mut extra: impl FnMut(&str, &mut Values) -> Result<bool, String>,
) -> Result<MonitorArgs, String> {
    let mut it = args.into_iter();
    let mut start = None;
    let mut config = MonitorConfig::new("");
    let mut sources = ListSources::official();
    let mut db = "bitchat-geohash.sqlite".to_owned();
    let mut verbose = false;
    while let Some(arg) = it.next() {
        let mut v = Values {
            flag: &arg,
            it: &mut it,
        };
        match arg.as_str() {
            "--depth" => config.depth = v.parsed("number of levels")?,
            "--levels" => config.levels = levels(&v.value()?)?,
            "--max-cells" => config.max_channels = v.parsed("number of channels")?,
            "--window" => config.aggregator.window_secs = v.duration()?.as_secs() as i64,
            "--inactivity" => config.aggregator.inactivity_secs = v.duration()?.as_secs() as i64,
            "--relays-csv" => sources.ios = Some(ListSource::parse(&v.value()?)),
            "--android-csv" => sources.android = Some(ListSource::parse(&v.value()?)),
            "--no-android" => sources.android = None,
            "--no-probe" => config.probe = false,
            "--db" => db = v.value()?,
            "--reinstate-all" => config.reinstate_all = true,
            "--forget-descents" => config.forget_descents = true,
            "--verbose" => verbose = true,
            flag if flag.starts_with('-') => {
                if !extra(flag, &mut v)? {
                    return Err(format!("unknown option {flag}"));
                }
            }
            _ if start.is_none() => start = Some(arg),
            _ => return Err(format!("one geohash only, got {arg:?} too")),
        }
    }
    config.start = start.ok_or("the geohash to watch is missing")?;
    Ok(MonitorArgs {
        config,
        sources,
        db,
        verbose,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    fn none(_: &str, _: &mut Values) -> Result<bool, String> {
        Ok(false)
    }

    #[test]
    fn monitor_options() {
        let a = parse(
            args(
                "u09 --depth 2 --levels 4,5 --max-cells 100 --window 5m --inactivity 1h \
                 --relays-csv own.csv --no-android --no-probe --db x.sqlite --reinstate-all \
                 --forget-descents --verbose",
            ),
            none,
        )
        .unwrap();
        let c = &a.config;
        assert_eq!((c.start.as_str(), c.depth, c.max_channels), ("u09", 2, 100));
        assert_eq!(c.levels, [Level::Province, Level::City]);
        assert_eq!(
            (c.aggregator.window_secs, c.aggregator.inactivity_secs),
            (300, 3600)
        );
        assert!(!c.probe && c.reinstate_all && c.forget_descents && a.verbose);
        assert_eq!(a.db, "x.sqlite");
        assert_eq!(a.sources.ios.unwrap().describe(), "own.csv");
        assert!(a.sources.android.is_none());
    }

    #[test]
    fn defaults() {
        let a = parse(args("u09tv"), none).unwrap();
        assert_eq!(
            (a.config.depth, a.db.as_str()),
            (1, "bitchat-geohash.sqlite")
        );
        assert!(a.config.probe && a.sources.ios.is_some() && a.sources.android.is_some());
    }

    #[test]
    fn other_options_go_to_the_caller() {
        let mut listen = None;
        let mut open = false;
        let a = parse(args("--listen 127.0.0.1:9 u09 --open"), |flag, v| {
            match flag {
                "--listen" => listen = Some(v.value()?),
                "--open" => open = true,
                _ => return Ok(false),
            }
            Ok(true)
        })
        .unwrap();
        assert_eq!(a.config.start, "u09");
        assert_eq!((listen.as_deref(), open), (Some("127.0.0.1:9"), true));
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let err = |s: &str| parse(args(s), none).err().unwrap();
        assert_eq!(err("--depth 2"), "the geohash to watch is missing");
        assert_eq!(err("u09 --depth"), "--depth needs a value");
        assert_eq!(
            err("u09 --depth two"),
            "--depth: number of levels expected, got \"two\""
        );
        assert!(err("u09 --levels 3").contains("2, 4, 5, 6, 7"));
        assert!(err("u09 --window 5d").starts_with("--window: duration expected"));
        assert_eq!(err("u09 --nope"), "unknown option --nope");
        assert_eq!(err("u09 u0b"), "one geohash only, got \"u0b\" too");
    }

    #[test]
    fn durations() {
        assert_eq!(duration("90"), Some(Duration::from_secs(90)));
        assert_eq!(duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(duration("15m"), Some(Duration::from_secs(900)));
        assert_eq!(duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(duration("m"), None);
        assert_eq!(duration("1d"), None);
    }
}
