//! User settings stored as JSON values in the `setting` table. Secrets never
//! go here; they belong in the operating system's credential store.

use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::Result;

pub mod keys {
    /// Generate demo candidates so the review flow can be tried.
    pub const DEMO_DISCOVERY: &str = "demo_discovery";
    /// Folder for kept audio.
    pub const ARCHIVE_DIR: &str = "archive_dir";
    /// Folder for temporary downloads.
    pub const STAGING_DIR: &str = "staging_dir";
    pub const LIMITS: &str = "limits";
    pub const ONBOARDED: &str = "onboarded";
    pub const VOLUME: &str = "volume";
}

pub fn get<T: DeserializeOwned>(conn: &Connection, key: &str) -> Result<Option<T>> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM setting WHERE key = ?1", params![key], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(match raw {
        Some(s) => serde_json::from_str(&s).ok(),
        None => None,
    })
}

pub fn get_or<T: DeserializeOwned>(conn: &Connection, key: &str, default: T) -> Result<T> {
    Ok(get(conn, key)?.unwrap_or(default))
}

pub fn set<T: Serialize>(conn: &Connection, key: &str, value: &T) -> Result<()> {
    conn.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, serde_json::to_string(value)?],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults() {
        let conn = crate::db::open_in_memory().unwrap();
        assert!(!get_or(&conn, keys::DEMO_DISCOVERY, false).unwrap());
        set(&conn, keys::DEMO_DISCOVERY, &true).unwrap();
        assert!(get_or(&conn, keys::DEMO_DISCOVERY, false).unwrap());
        set(&conn, keys::ARCHIVE_DIR, &"/music/archive").unwrap();
        assert_eq!(
            get::<String>(&conn, keys::ARCHIVE_DIR).unwrap().as_deref(),
            Some("/music/archive")
        );
    }
}
