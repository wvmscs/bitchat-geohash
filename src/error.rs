//! Library error type.

/// Errors returned by this crate. Variants are added as the lots land
/// (see `PLAN_ET1.md`).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid geohash {0:?}")]
    InvalidGeohash(String),
    #[error("{count} channels requested, more than the maximum of {max}")]
    TooManyChannels { count: u64, max: usize },
    #[error("channel {geohash:?} is outside the watched area {start:?}")]
    OutsideArea { geohash: String, start: String },
    #[error("no channel level below {0:?}")]
    NoFinerLevel(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema version {found} is newer than this code (up to {supported})")]
    SchemaTooNew { found: u32, supported: u32 },
    #[error("migration {name} failed: {source}")]
    Migration {
        name: String,
        source: rusqlite::Error,
    },
    #[error("the store writer thread has stopped")]
    StoreClosed,
    #[error("listening on {0} (not a loopback address) needs allow_remote")]
    RemoteListen(String),
    #[error("listening on {0} without TLS needs insecure_remote")]
    RemoteWithoutTls(String),
    #[error("TLS: {0}")]
    Tls(String),
    #[error("the monitor has stopped")]
    MonitorStopped,
    #[error("the watched area cannot change here (fixed relay lists, no database to start again)")]
    AreaFixed,
    #[error("relay list {0} has no usable relay")]
    EmptyRelayList(String),
    #[error("host {0:?} is listed at two places: iOS rejects this relay list")]
    RelayListConflict(String),
    #[error("no relay list available (download failed and none in the database)")]
    NoRelayList,
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    /// Boxed: it carries an HTTP response and would bloat every `Result`.
    #[error("WebSocket error: {0}")]
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::WebSocket(Box::new(e))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
