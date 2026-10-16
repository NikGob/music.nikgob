//! "Загрузки → История": finished imports and downloads, kept across restarts
//! (table `job_log`). Not user data: not exported, not synced.

use rusqlite::params;

use crate::db::Library;
use crate::error::Result;
use crate::types::JobLogEntry;
use crate::util::now_ms;

/// Entries kept; older ones are dropped on every append.
pub const JOB_LOG_KEEP: u32 = 200;

impl Library {
    /// Appends a finished job and trims the log to [`JOB_LOG_KEEP`] entries.
    pub fn log_job(&self, kind: &str, title: &str, subtitle: &str, error: Option<&str>) -> Result<i64> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO job_log(kind, title, subtitle, error, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![kind, title, subtitle, error, now_ms()],
            )?;
            let seq = tx.last_insert_rowid();
            tx.execute(
                "DELETE FROM job_log WHERE seq <= ?1 - ?2",
                params![seq, i64::from(JOB_LOG_KEEP)],
            )?;
            Ok(seq)
        })
    }

    /// The latest `limit` entries of one kind (`None` = all), newest first.
    pub fn job_log(&self, kind: Option<&str>, limit: u32) -> Result<Vec<JobLogEntry>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT seq, kind, title, subtitle, error, at FROM job_log
                  WHERE ?1 IS NULL OR kind = ?1 ORDER BY seq DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![kind, limit], |r| {
                Ok(JobLogEntry {
                    seq: r.get(0)?,
                    kind: r.get(1)?,
                    title: r.get(2)?,
                    subtitle: r.get(3)?,
                    error: r.get(4)?,
                    at: r.get(5)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}
