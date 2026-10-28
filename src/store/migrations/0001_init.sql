-- SPDX-FileCopyrightText: 2026 William VITAL, FR (wvmscs)
-- SPDX-License-Identifier: GPL-3.0-or-later

-- Initial schema. Times are Unix seconds (UTC).
-- No pubkey, nickname or message content anywhere (spec §5).

-- Each relay list load (24 h reload, D5).
CREATE TABLE relay_list_load (
    id        INTEGER PRIMARY KEY,
    platform  TEXT    NOT NULL CHECK (platform IN ('ios', 'android')),
    source    TEXT    NOT NULL,
    loaded_at INTEGER NOT NULL,
    sha256    TEXT    NOT NULL REFERENCES relay_list_csv (sha256),
    relays    INTEGER NOT NULL,
    rejected  INTEGER NOT NULL
);
CREATE INDEX relay_list_load_latest ON relay_list_load (platform, loaded_at);

-- List contents, once per distinct file: the fallback when a download fails.
CREATE TABLE relay_list_csv (
    sha256 TEXT PRIMARY KEY,
    csv    TEXT NOT NULL
) WITHOUT ROWID;

-- Relay probe results.
CREATE TABLE probe_result (
    id        INTEGER PRIMARY KEY,
    relay     TEXT    NOT NULL,
    probed_at INTEGER NOT NULL,
    status    TEXT    NOT NULL
        CHECK (status IN ('ok', 'auth', 'refused', 'down', 'dead', 'unprobed')),
    detail    TEXT    NOT NULL
);
CREATE INDEX probe_result_latest ON probe_result (relay, probed_at);

-- Watched or retired state of each channel (survives restarts, D6).
CREATE TABLE channel (
    geohash    TEXT    PRIMARY KEY,
    state      TEXT    NOT NULL CHECK (state IN ('watched', 'retired')),
    changed_at INTEGER NOT NULL,
    reason     TEXT
) WITHOUT ROWID;

-- Per-cell counters, one row per channel and per minute.
CREATE TABLE cell_bucket (
    geohash      TEXT    NOT NULL,
    bucket_start INTEGER NOT NULL,
    messages     INTEGER NOT NULL,
    presences    INTEGER NOT NULL,
    participants INTEGER NOT NULL,
    PRIMARY KEY (geohash, bucket_start)
) WITHOUT ROWID;
