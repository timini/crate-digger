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
    /// Closing the window keeps background work running in the tray.
    pub const CLOSE_TO_TRAY: &str = "close_to_tray";
}

/// Resource limits the user controls. Defaults are the product spec's.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UserLimits {
    pub ready_target: u32,
    pub replenish_below: u32,
    pub active_downloads: u32,
    pub analysis_jobs: u32,
    pub temp_budget_gb: f64,
    pub daily_acquisitions: u32,
    pub source_refresh_hours: u32,
}

impl Default for UserLimits {
    fn default() -> Self {
        UserLimits {
            ready_target: 50,
            replenish_below: 30,
            active_downloads: 2,
            analysis_jobs: 1,
            temp_budget_gb: 10.0,
            daily_acquisitions: 100,
            source_refresh_hours: 6,
        }
    }
}

impl UserLimits {
    /// Reject values that would stop the app working or make no sense.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.active_downloads == 0 || self.active_downloads > 20 {
            return Err("Active downloads must be between 1 and 20.".into());
        }
        if self.analysis_jobs == 0 || self.analysis_jobs > 16 {
            return Err("Concurrent analysis jobs must be between 1 and 16.".into());
        }
        if !(0.1..=10_000.0).contains(&self.temp_budget_gb) {
            return Err("The temporary audio budget must be between 0.1 and 10000 GB.".into());
        }
        if self.replenish_below > self.ready_target {
            return Err("The replenishment threshold cannot be above the ready-to-review target.".into());
        }
        if self.ready_target == 0 || self.source_refresh_hours == 0 {
            return Err("The ready target and refresh interval must be at least 1.".into());
        }
        Ok(())
    }

    /// The scheduler's view of these limits.
    pub fn scheduler_limits(&self) -> crate::jobs::scheduler::Limits {
        use crate::jobs::kinds;
        let mut l = crate::jobs::scheduler::Limits::default();
        l.concurrency
            .insert(kinds::ACQUIRE.into(), self.active_downloads as usize);
        l.concurrency
            .insert(kinds::ANALYSE.into(), self.analysis_jobs as usize);
        l.daily.insert(kinds::ACQUIRE.into(), self.daily_acquisitions);
        l.staging_budget_bytes = (self.temp_budget_gb * 1024.0 * 1024.0 * 1024.0) as u64;
        l
    }
}

pub fn limits(conn: &Connection) -> Result<UserLimits> {
    get_or(conn, keys::LIMITS, UserLimits::default())
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

    #[test]
    fn default_limits_match_the_spec_and_map_to_the_scheduler() {
        let conn = crate::db::open_in_memory().unwrap();
        let l = limits(&conn).unwrap();
        assert_eq!(
            (
                l.ready_target,
                l.replenish_below,
                l.active_downloads,
                l.analysis_jobs
            ),
            (50, 30, 2, 1)
        );
        assert_eq!(
            (l.temp_budget_gb, l.daily_acquisitions, l.source_refresh_hours),
            (10.0, 100, 6)
        );
        assert_eq!(l.scheduler_limits(), crate::jobs::scheduler::Limits::default());
    }

    #[test]
    fn invalid_limits_are_rejected() {
        let bad = UserLimits {
            replenish_below: 60,
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        let none = UserLimits {
            active_downloads: 0,
            ..Default::default()
        };
        assert!(none.validate().is_err());
        assert!(UserLimits::default().validate().is_ok());
    }
}
