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

    /// The one line of `kind` (the latest state of one thing, a wished
    /// download): its older lines go. Trims the log as [`Self::log_job`].
    pub fn put_job_line(&self, kind: &str, title: &str, subtitle: &str, error: Option<&str>) -> Result<i64> {
        self.write(|tx| {
            tx.execute("DELETE FROM job_log WHERE kind = ?1", params![kind])?;
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

    /// The lines of `kind` go.
    pub fn drop_job_lines(&self, kind: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute("DELETE FROM job_log WHERE kind = ?1", params![kind])?;
            Ok(())
        })
    }

    /// The latest `limit` entries of one kind (`None` = all), newest first.
    pub fn job_log(&self, kind: Option<&str>, limit: u32) -> Result<Vec<JobLogEntry>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT seq, kind, title, subtitle, error, at FROM job_log
                  WHERE ?1 IS NULL OR kind = ?1 ORDER BY seq DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![kind, limit], entry)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The latest `limit` entries of `kind` and of the kinds starting with
    /// `prefix`, newest first.
    pub fn job_log_with(&self, kind: &str, prefix: &str, limit: u32) -> Result<Vec<JobLogEntry>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT seq, kind, title, subtitle, error, at FROM job_log
                  WHERE kind = ?1 OR substr(kind, 1, length(?2)) = ?2 ORDER BY seq DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![kind, prefix, limit], entry)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

fn entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobLogEntry> {
    Ok(JobLogEntry {
        seq: r.get(0)?,
        kind: r.get(1)?,
        title: r.get(2)?,
        subtitle: r.get(3)?,
        error: r.get(4)?,
        at: r.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wished download has one line, its latest; the imports' lines and
    /// the other wishes' stay.
    #[test]
    fn a_line_of_its_own_kind_replaces_the_older_one() {
        let lib = Library::open_in_memory().unwrap();
        lib.log_job("import", "Импорт «A»", "добавлено 1", None).unwrap();
        lib.put_job_line("wish:1", "Eve — Underdog", "Tidal не отдаёт", None).unwrap();
        lib.put_job_line("wish:2", "Ado — Show", "Qobuz просит подождать", None).unwrap();
        lib.put_job_line("wish:1", "Eve — Underdog", "дальше — Soulseek", None).unwrap();
        lib.log_job("torrent", "B", "Скачано", None).unwrap();
        let shown: Vec<(String, String)> = lib
            .job_log_with("import", "wish:", 10)
            .unwrap()
            .into_iter()
            .map(|e| (e.kind, e.subtitle))
            .collect();
        assert_eq!(
            shown,
            [
                ("wish:1".to_owned(), "дальше — Soulseek".to_owned()),
                ("wish:2".to_owned(), "Qobuz просит подождать".to_owned()),
                ("import".to_owned(), "добавлено 1".to_owned()),
            ]
        );
        lib.drop_job_lines("wish:1").unwrap();
        assert_eq!(lib.job_log_with("import", "wish:", 10).unwrap().len(), 2);
    }
}
