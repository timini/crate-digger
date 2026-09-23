//! SQLite connection setup and versioned migrations.

use std::path::Path;

use rusqlite::Connection;

use crate::Result;

/// Ordered, append-only list of migrations. Never edit a released entry.
pub const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "init", include_str!("../migrations/0001_init.sql")),
    (2, "job_hold", include_str!("../migrations/0002_job_hold.sql")),
    (3, "library", include_str!("../migrations/0003_library.sql")),
    (
        4,
        "rating_order",
        include_str!("../migrations/0004_rating_order.sql"),
    ),
    (5, "identity", include_str!("../migrations/0005_identity.sql")),
    (6, "analysis", include_str!("../migrations/0006_analysis.sql")),
    (7, "matching", include_str!("../migrations/0007_matching.sql")),
];

pub fn latest_version() -> i64 {
    MIGRATIONS.last().map(|m| m.0).unwrap_or(0)
}

/// Open (creating if needed) and migrate the database at `path`.
pub fn open(path: &Path) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

/// Open without migrating. Used by worker threads after the main connection
/// has already migrated the file.
pub fn open_existing(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection> {
    let mut conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // FULL: a rating or job transition is on disk once the call returns.
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> Result<i64> {
    ensure_version_table(conn)?;
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?)
}

pub fn migrate(conn: &mut Connection) -> Result<()> {
    migrate_to(conn, latest_version())
}

/// Apply every migration up to and including `target`, each in its own
/// transaction.
pub fn migrate_to(conn: &mut Connection, target: i64) -> Result<()> {
    let current = schema_version(conn)?;
    if current > latest_version() {
        return Err(crate::Error::Invalid(format!(
            "database schema version {current} is newer than this app supports ({}); \
             update Crate Digger before opening this library",
            latest_version()
        )));
    }
    for (version, name, sql) in MIGRATIONS {
        if *version <= current || *version > target {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        after_migration(&tx, *version)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![version, name, crate::util::now_ms()],
        )?;
        tx.commit()?;
        tracing::info!(version, name, "applied migration");
    }
    Ok(())
}

/// Data changes that need Rust code, run inside the migration's transaction.
fn after_migration(conn: &Connection, version: i64) -> Result<()> {
    if version == 7 {
        let rows: Vec<(String, Option<String>)> = {
            let mut stmt = conn.prepare("SELECT track_id, title FROM track_meta")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        for (track, title) in rows {
            conn.execute(
                "UPDATE track_meta SET title_key = ?2 WHERE track_id = ?1",
                rusqlite::params![track, title.map(|t| crate::identity::normalize::title_key(&t))],
            )?;
        }
        let fps: Vec<(String, Vec<u8>)> = {
            let mut stmt = conn.prepare("SELECT id, data FROM fingerprint")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };
        for (id, data) in fps {
            crate::analysis::store::index_fingerprint(conn, &id, &data)?;
        }
    }
    Ok(())
}

fn ensure_version_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version    INTEGER PRIMARY KEY,
             name       TEXT NOT NULL,
             applied_at INTEGER NOT NULL
         )",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_empty_database_to_latest() {
        let conn = open_in_memory().unwrap();
        assert_eq!(schema_version(&conn).unwrap(), latest_version());
    }

    #[test]
    fn migrations_are_strictly_increasing() {
        let versions: Vec<i64> = MIGRATIONS.iter().map(|m| m.0).collect();
        assert!(versions.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(versions.first(), Some(&1));
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        drop(open(&path).unwrap());
        let conn = open(&path).unwrap();
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, MIGRATIONS.len() as i64);
    }

    /// For every historical version: build a database at that version, put
    /// data in it, then migrate forward and check the data survived.
    #[test]
    fn migrates_existing_database_forwards_from_every_version() {
        for (version, _, _) in MIGRATIONS {
            let mut conn = Connection::open_in_memory().unwrap();
            configure(&conn).unwrap();
            migrate_to(&mut conn, *version).unwrap();
            conn.execute(
                "INSERT INTO track (id, created_at, updated_at) VALUES ('t1', 1, 1)",
                [],
            )
            .unwrap();
            migrate(&mut conn).unwrap();
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM track WHERE id = 't1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "data lost migrating from version {version}");
            assert_eq!(schema_version(&conn).unwrap(), latest_version());
        }
    }

    #[test]
    fn refuses_database_from_newer_app() {
        let mut conn = open_in_memory().unwrap();
        conn.execute("INSERT INTO schema_migrations VALUES (9999, 'future', 0)", [])
            .unwrap();
        assert!(migrate(&mut conn).is_err());
    }

    #[test]
    fn track_table_has_no_path_column() {
        let conn = open_in_memory().unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('track')")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(
            cols.iter().all(|c| !c.contains("path")),
            "track columns: {cols:?}"
        );
    }
}
