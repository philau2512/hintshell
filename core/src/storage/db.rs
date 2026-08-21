use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEntry {
    pub id: Option<i64>,
    pub command: String,
    pub description: Option<String>,
    pub frequency: i64,
    pub last_used: DateTime<Utc>,
    pub directory: Option<String>,
    pub shell: Option<String>,
    pub source: String,
}

#[derive(Deserialize)]
pub struct DefaultCmd {
    pub command: String,
    pub description: Option<String>,
}

pub struct HistoryStore {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct LocalHistoryStats {
    pub frequency: i64,
    pub last_used: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPruneResult {
    pub candidates: i64,
    pub deleted: i64,
}

impl HistoryStore {
    pub fn new(db_path: &PathBuf) -> SqlResult<Self> {
        let conn = Connection::open(db_path)?;
        // Avoid multi-process hang if a stale reader briefly holds the DB.
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;");
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_tables()?;
        Ok(store)
    }

    pub fn in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_tables()?;
        Ok(store)
    }

    fn init_tables(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                command     TEXT NOT NULL,
                description TEXT,
                frequency   INTEGER NOT NULL DEFAULT 1,
                last_used   TEXT NOT NULL,
                directory   TEXT,
                shell       TEXT,
                source      TEXT DEFAULT 'user'
            );

            CREATE INDEX IF NOT EXISTS idx_command ON history(command);
            CREATE INDEX IF NOT EXISTS idx_frequency ON history(frequency DESC);
            CREATE INDEX IF NOT EXISTS idx_last_used ON history(last_used DESC);

            CREATE TABLE IF NOT EXISTS history_context (
                command   TEXT NOT NULL,
                directory TEXT NOT NULL,
                frequency INTEGER NOT NULL DEFAULT 1,
                last_used TEXT NOT NULL,
                PRIMARY KEY (command, directory)
            );

            CREATE INDEX IF NOT EXISTS idx_history_context_directory
                ON history_context(directory, frequency DESC, last_used DESC);

            CREATE TABLE IF NOT EXISTS storage_metadata (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;

        // Migration: Add description column if missing
        let has_description: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('history') WHERE name='description'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_description {
            conn.execute("ALTER TABLE history ADD COLUMN description TEXT", [])?;
        }

        // Migration: Add source column if missing
        let has_source: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('history') WHERE name='source'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0)
            > 0;

        if !has_source {
            conn.execute(
                "ALTER TABLE history ADD COLUMN source TEXT DEFAULT 'user'",
                [],
            )?;
        }

        Ok(())
    }

    /// Add a command to history. If it already exists, increment frequency and update timestamp.
    pub fn add_command(
        &self,
        command: &str,
        directory: Option<&str>,
        shell: Option<&str>,
    ) -> SqlResult<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();

        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM history WHERE command = ?1",
                params![command],
                |row| row.get(0),
            )
            .ok();

        match existing {
            Some(id) => {
                conn.execute(
                    "UPDATE history SET frequency = frequency + 1, last_used = ?1 WHERE id = ?2",
                    params![now, id],
                )?;
            }
            None => {
                conn.execute(
                    "INSERT INTO history (command, frequency, last_used, directory, shell, source) VALUES (?1, 1, ?2, ?3, ?4, 'user')",
                    params![command, now, directory, shell],
                )?;
            }
        }

        if let Some(directory) = directory.filter(|directory| !directory.trim().is_empty()) {
            conn.execute(
                "INSERT INTO history_context (command, directory, frequency, last_used)
                 VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(command, directory) DO UPDATE SET
                     frequency = history_context.frequency + 1,
                     last_used = excluded.last_used",
                params![command, directory, now],
            )?;
        }
        Ok(())
    }

    pub fn get_local_history(
        &self,
        directory: &str,
    ) -> SqlResult<HashMap<String, LocalHistoryStats>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT command, frequency, last_used
             FROM history_context
             WHERE directory = ?1",
        )?;

        let stats = statement
            .query_map(params![directory], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    LocalHistoryStats {
                        frequency: row.get(1)?,
                        last_used: row
                            .get::<_, String>(2)?
                            .parse::<DateTime<Utc>>()
                            .unwrap_or_else(|_| Utc::now()),
                    },
                ))
            })?
            .collect::<SqlResult<HashMap<_, _>>>()?;
        Ok(stats)
    }

    /// Search commands by prefix match, ordered by ranking score.
    pub fn search_by_prefix(&self, prefix: &str, limit: usize) -> SqlResult<Vec<CommandEntry>> {
        let pattern = format!("{}%", prefix);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, command, frequency, last_used, directory, shell, description, source
             FROM history
             WHERE command LIKE ?1
             ORDER BY frequency DESC, last_used DESC
             LIMIT ?2",
        )?;

        let entries = stmt
            .query_map(params![pattern, limit as i64], |row| {
                Ok(CommandEntry {
                    id: Some(row.get(0)?),
                    command: row.get(1)?,
                    frequency: row.get(2)?,
                    last_used: row
                        .get::<_, String>(3)?
                        .parse::<DateTime<Utc>>()
                        .unwrap_or_else(|_| Utc::now()),
                    directory: row.get(4)?,
                    shell: row.get(5)?,
                    description: row.get(6).unwrap_or(None),
                    source: row.get(7).unwrap_or_else(|_| "user".to_string()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// Get all commands (for fuzzy matching in engine).
    pub fn get_all_commands(&self) -> SqlResult<Vec<CommandEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, command, frequency, last_used, directory, shell, description, source FROM history ORDER BY frequency DESC",
        )?;

        let entries = stmt
            .query_map([], |row| {
                Ok(CommandEntry {
                    id: Some(row.get(0)?),
                    command: row.get(1)?,
                    frequency: row.get(2)?,
                    last_used: row
                        .get::<_, String>(3)?
                        .parse::<DateTime<Utc>>()
                        .unwrap_or_else(|_| Utc::now()),
                    directory: row.get(4)?,
                    shell: row.get(5)?,
                    description: row.get(6).unwrap_or(None),
                    source: row.get(7).unwrap_or_else(|_| "user".to_string()),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// Get the single most recent matching command.
    pub fn get_most_recent_match(&self, input: &str) -> SqlResult<Option<CommandEntry>> {
        let pattern = format!("%{}%", input); // Substring match for more flexibility
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, command, frequency, last_used, directory, shell, description, source
             FROM history
             WHERE command LIKE ?1
             ORDER BY last_used DESC
             LIMIT 1",
        )?;

        let mut entries = stmt.query_map(params![pattern], |row| {
            Ok(CommandEntry {
                id: Some(row.get(0)?),
                command: row.get(1)?,
                frequency: row.get(2)?,
                last_used: row
                    .get::<_, String>(3)?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now()),
                directory: row.get(4)?,
                shell: row.get(5)?,
                description: row.get(6).unwrap_or(None),
                source: row.get(7).unwrap_or_else(|_| "user".to_string()),
            })
        })?;

        if let Some(entry) = entries.next() {
            return Ok(Some(entry?));
        }
        Ok(None)
    }

    pub fn get_total_commands(&self) -> SqlResult<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
    }

    /// Seed the database with default commands from JSON content.
    /// Only inserts commands that don't already exist (idempotent).
    pub fn seed_defaults(&self, json_content: &str) -> Result<usize, String> {
        let categories: HashMap<String, Vec<DefaultCmd>> = serde_json::from_str(json_content)
            .map_err(|e| format!("Failed to parse defaults JSON: {}", e))?;

        let old_time = "2000-01-01T00:00:00Z";
        let conn = self.conn.lock().unwrap();

        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| e.to_string())?;

        let mut count = 0;
        for commands in categories.values() {
            for cmd_obj in commands {
                let trimmed = cmd_obj.command.trim();
                let desc = cmd_obj.description.clone();
                if trimmed.is_empty() {
                    continue;
                }
                let exists: bool = conn
                    .query_row(
                        "SELECT COUNT(*) FROM history WHERE command = ?1",
                        params![trimmed],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|c| c > 0)
                    .unwrap_or(false);

                if !exists {
                    conn.execute(
                        "INSERT INTO history (command, frequency, last_used, directory, shell, description, source) VALUES (?1, 0, ?2, NULL, NULL, ?3, 'default')",
                        params![trimmed, old_time, desc],
                    ).map_err(|e| e.to_string())?;
                    count += 1;
                } else {
                    // Keep commands listed in the default catalog permanently protected from pruning.
                    conn.execute(
                        "UPDATE history
                         SET description = ?1, source = 'default'
                         WHERE command = ?2 AND (source != 'default' OR description IS NULL OR description != ?1)",
                        params![desc, trimmed],
                    ).map_err(|e| e.to_string())?;
                }
            }
        }

        conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;

        Ok(count)
    }

    /// Count and optionally delete stale, low-frequency user history.
    /// Matching context rows are removed in the same transaction as history rows.
    pub fn prune_user_history(
        &self,
        cutoff: DateTime<Utc>,
        dry_run: bool,
    ) -> SqlResult<HistoryPruneResult> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let cutoff = cutoff.to_rfc3339();
        let candidates: i64 = tx.query_row(
            "SELECT COUNT(*) FROM history
             WHERE source = 'user' AND frequency < 3 AND last_used < ?1",
            params![cutoff],
            |row| row.get(0),
        )?;

        if dry_run {
            tx.rollback()?;
            return Ok(HistoryPruneResult {
                candidates,
                deleted: 0,
            });
        }

        tx.execute(
            "DELETE FROM history_context
             WHERE command IN (
                 SELECT command FROM history
                 WHERE source = 'user' AND frequency < 3 AND last_used < ?1
             )",
            params![cutoff],
        )?;
        let deleted = tx.execute(
            "DELETE FROM history
             WHERE source = 'user' AND frequency < 3 AND last_used < ?1",
            params![cutoff],
        )? as i64;
        tx.commit()?;

        Ok(HistoryPruneResult {
            candidates,
            deleted,
        })
    }

    /// Prune stale user history at most once per 24 hours.
    /// Returns `None` when the cadence has not elapsed.
    pub fn prune_user_history_daily(
        &self,
        now: DateTime<Utc>,
        cutoff: DateTime<Utc>,
        dry_run: bool,
    ) -> SqlResult<Option<HistoryPruneResult>> {
        const KEY: &str = "last_history_prune";
        let due = {
            let conn = self.conn.lock().unwrap();
            let tx = conn.unchecked_transaction()?;
            let last: Option<String> = tx
                .query_row(
                    "SELECT value FROM storage_metadata WHERE key = ?1",
                    params![KEY],
                    |row| row.get(0),
                )
                .ok();
            let due = last
                .as_deref()
                .and_then(|value| value.parse::<DateTime<Utc>>().ok())
                .map(|timestamp| now.signed_duration_since(timestamp).num_hours() >= 24)
                .unwrap_or(true);
            tx.rollback()?;
            due
        };

        if !due {
            return Ok(None);
        }

        let result = self.prune_user_history(cutoff, dry_run)?;
        if !dry_run {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO storage_metadata (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![KEY, now.to_rfc3339()],
            )?;
        }
        Ok(Some(result))
    }

    #[cfg(test)]
    pub fn set_last_used_for_test(&self, command: &str, time: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE history SET last_used = ?1 WHERE command = ?2",
            params![time, command],
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_search() {
        let store = HistoryStore::in_memory().unwrap();

        store
            .add_command("git init", None, Some("powershell"))
            .unwrap();
        store
            .add_command("git commit -m \"test\"", None, Some("powershell"))
            .unwrap();
        store
            .add_command("git push", None, Some("powershell"))
            .unwrap();

        let results = store.search_by_prefix("git", 10).unwrap();
        assert_eq!(results.len(), 3);
        assert!(results[0].command.starts_with("git"));
    }

    #[test]
    fn test_local_history_is_tracked_per_directory() {
        let store = HistoryStore::in_memory().unwrap();

        store
            .add_command("cargo test", Some("C:/projects/a"), Some("powershell"))
            .unwrap();
        store
            .add_command("cargo test", Some("C:/projects/a"), Some("powershell"))
            .unwrap();
        store
            .add_command("cargo test", Some("C:/projects/b"), Some("powershell"))
            .unwrap();

        let first = store.get_local_history("C:/projects/a").unwrap();
        let second = store.get_local_history("C:/projects/b").unwrap();
        assert_eq!(first["cargo test"].frequency, 2);
        assert_eq!(second["cargo test"].frequency, 1);
    }

    #[test]
    fn test_frequency_increment() {
        let store = HistoryStore::in_memory().unwrap();

        store.add_command("git status", None, None).unwrap();
        store.add_command("git status", None, None).unwrap();
        store.add_command("git status", None, None).unwrap();

        let results = store.search_by_prefix("git status", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].frequency, 3);
    }

    #[test]
    fn test_ranking_order() {
        let store = HistoryStore::in_memory().unwrap();

        store.add_command("git add .", None, None).unwrap();

        // git commit used 5 times -> should rank higher
        for _ in 0..5 {
            store
                .add_command("git commit -m \"msg\"", None, None)
                .unwrap();
        }

        store.add_command("git push", None, None).unwrap();

        let results = store.search_by_prefix("git", 10).unwrap();
        assert_eq!(results[0].command, "git commit -m \"msg\"");
    }

    #[test]
    fn test_seed_defaults_sets_source() {
        let store = HistoryStore::in_memory().unwrap();
        let json = r#"{ "git": [{ "command": "git pull", "description": "pull changes" }] }"#;
        store.seed_defaults(json).unwrap();

        let results = store.search_by_prefix("git", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "default");
    }

    #[test]
    fn test_prune_user_history_preserves_defaults_and_frequent_rows() {
        let store = HistoryStore::in_memory().unwrap();
        let json = r#"{ "git": [{ "command": "git pull", "description": "pull changes" }] }"#;
        store.seed_defaults(json).unwrap();
        store.add_command("git status", None, None).unwrap();
        for _ in 0..3 {
            store.add_command("git log", None, None).unwrap();
        }
        store.set_last_used_for_test("git status", "2000-01-01T00:00:00Z");
        store.set_last_used_for_test("git log", "2000-01-01T00:00:00Z");

        let result = store
            .prune_user_history("2020-01-01T00:00:00Z".parse().unwrap(), false)
            .unwrap();
        assert_eq!(result.candidates, 1);
        assert_eq!(result.deleted, 1);
        assert_eq!(store.search_by_prefix("git", 10).unwrap().len(), 2);
        assert_eq!(
            store.search_by_prefix("git pull", 10).unwrap()[0].source,
            "default"
        );
    }

    #[test]
    fn test_prune_user_history_cleans_context_and_dry_run_mutates_nothing() {
        let store = HistoryStore::in_memory().unwrap();
        store
            .add_command("old command", Some("C:/old"), None)
            .unwrap();
        store.set_last_used_for_test("old command", "2000-01-01T00:00:00Z");

        let cutoff = "2020-01-01T00:00:00Z".parse().unwrap();
        let preview = store.prune_user_history(cutoff, true).unwrap();
        assert_eq!(
            preview,
            HistoryPruneResult {
                candidates: 1,
                deleted: 0
            }
        );
        assert_eq!(store.get_total_commands().unwrap(), 1);
        assert_eq!(store.get_local_history("C:/old").unwrap().len(), 1);

        let result = store.prune_user_history(cutoff, false).unwrap();
        assert_eq!(result.deleted, 1);
        assert_eq!(store.get_total_commands().unwrap(), 0);
        assert!(store.get_local_history("C:/old").unwrap().is_empty());
    }

    #[test]
    fn test_daily_prune_persists_24_hour_cadence() {
        let store = HistoryStore::in_memory().unwrap();
        let now = "2024-01-02T00:00:00Z".parse().unwrap();
        let cutoff = "2020-01-01T00:00:00Z".parse().unwrap();
        let first = store.prune_user_history_daily(now, cutoff, true).unwrap();
        assert!(first.is_some());
        let second = store.prune_user_history_daily(now, cutoff, true).unwrap();
        assert!(second.is_some());

        let first = store.prune_user_history_daily(now, cutoff, false).unwrap();
        assert!(first.is_some());
        assert!(store
            .prune_user_history_daily(now, cutoff, false)
            .unwrap()
            .is_none());
        assert!(store
            .prune_user_history_daily(now + chrono::Duration::hours(24), cutoff, false)
            .unwrap()
            .is_some());
    }

    #[test]
    fn test_add_command_sets_user_source() {
        let store = HistoryStore::in_memory().unwrap();
        store.add_command("git push", None, None).unwrap();
        let results = store.search_by_prefix("git", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source, "user");
    }
}
