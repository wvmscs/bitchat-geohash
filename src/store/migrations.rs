//! Versioned schema migrations.
//!
//! Append-only: one numbered file per change, never edited or reordered once
//! committed; a fix is a new migration. Each migration runs in its own
//! transaction together with the version bump (`PRAGMA user_version`), so a
//! failed one leaves the database at the previous version.

use rusqlite::Connection;

use crate::error::{Error, Result};

pub(crate) struct Migration {
    pub name: &'static str,
    pub sql: &'static str,
}

/// All migrations, in order; migration `i` brings the schema to version `i + 1`.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "0001_init",
        sql: include_str!("migrations/0001_init.sql"),
    },
    Migration {
        name: "0002_channel_origin",
        sql: include_str!("migrations/0002_channel_origin.sql"),
    },
];

/// Schema version this code writes.
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

pub(crate) fn user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Brings the database to the last version of `migrations`. Refuses a
/// database newer than that. Returns the resulting version.
pub(crate) fn apply(conn: &mut Connection, migrations: &[Migration]) -> Result<u32> {
    let supported = migrations.len() as u32;
    let found = user_version(conn)?;
    if found > supported {
        return Err(Error::SchemaTooNew { found, supported });
    }
    for (i, m) in migrations.iter().enumerate().skip(found as usize) {
        let version = i as u32 + 1;
        let failed = |source| Error::Migration {
            name: m.name.to_owned(),
            source,
        };
        let tx = conn.transaction().map_err(failed)?;
        tx.execute_batch(m.sql).map_err(failed)?;
        tx.pragma_update(None, "user_version", version)
            .map_err(failed)?;
        tx.commit().map_err(failed)?;
        tracing::info!(migration = m.name, version, "schema migrated");
    }
    Ok(supported)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: Migration = Migration {
        name: "0001_a",
        sql: "CREATE TABLE a (x INTEGER);",
    };
    const V2: Migration = Migration {
        name: "0002_b",
        sql: "CREATE TABLE b (y INTEGER); INSERT INTO a VALUES (1);",
    };
    const BROKEN: Migration = Migration {
        name: "0003_broken",
        sql: "CREATE TABLE c (z INTEGER); INSERT INTO nowhere VALUES (1);",
    };

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn empty_database_gets_every_migration() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(apply(&mut conn, &[V1, V2]).unwrap(), 2);
        assert_eq!(user_version(&conn).unwrap(), 2);
        assert_eq!(tables(&conn), ["a", "b"]);
    }

    #[test]
    fn database_at_version_n_gets_the_rest_only() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &[V1]).unwrap();
        assert_eq!(apply(&mut conn, &[V1, V2]).unwrap(), 2);
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM a", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "V2 ran once");
        // Up to date: nothing runs again.
        assert_eq!(apply(&mut conn, &[V1, V2]).unwrap(), 2);
    }

    #[test]
    fn newer_database_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &[V1, V2]).unwrap();
        assert!(matches!(
            apply(&mut conn, &[V1]),
            Err(Error::SchemaTooNew {
                found: 2,
                supported: 1
            })
        ));
    }

    #[test]
    fn failed_migration_rolls_back() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn, &[V1]).unwrap();
        let err = apply(&mut conn, &[V1, V2, BROKEN]).unwrap_err();
        assert!(
            matches!(&err, Error::Migration { name, .. } if name == "0003_broken"),
            "{err}"
        );
        // V2 committed, BROKEN left nothing behind.
        assert_eq!(user_version(&conn).unwrap(), 2);
        assert_eq!(tables(&conn), ["a", "b"]);
    }

    #[test]
    fn real_migrations_apply() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(apply(&mut conn, MIGRATIONS).unwrap(), SCHEMA_VERSION);
    }
}
