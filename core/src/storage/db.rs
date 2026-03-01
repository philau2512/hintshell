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
    pub frequency: i64,
    pub last_used: DateTime<Utc>,
    pub directory: Option<String>,
    pub shell: Option<String>,
}

pub struct HistoryStore {
    conn: Mutex<Connection>,
}

impl HistoryStore {
    pub fn new(db_path: &PathBuf) -> SqlResult<Self> {
        let conn = Connection::open(db_path)?;
        let store = Self { conn: Mutex::new(conn) };
        store.init_tables()?;
        Ok(store)
    }

    pub fn in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn: Mutex::new(conn) };
        store.init_tables()?;
        Ok(store)
    }

    fn init_tables(&self) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                command     TEXT NOT NULL,
                frequency   INTEGER NOT NULL DEFAULT 1,
                last_used   TEXT NOT NULL,
                directory   TEXT,
                shell       TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_command ON history(command);
            CREATE INDEX IF NOT EXISTS idx_frequency ON history(frequency DESC);
            CREATE INDEX IF NOT EXISTS idx_last_used ON history(last_used DESC);
            ",
        )?;
        Ok(())
    }

    /// Add a command to history. If it already exists, increment frequency and update timestamp.
    pub fn add_command(&self, command: &str, directory: Option<&str>, shell: Option<&str>) -> SqlResult<()> {
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
                    "INSERT INTO history (command, frequency, last_used, directory, shell) VALUES (?1, 1, ?2, ?3, ?4)",
                    params![command, now, directory, shell],
                )?;
            }
        }
        Ok(())
    }

    /// Search commands by prefix match, ordered by ranking score.
    pub fn search_by_prefix(&self, prefix: &str, limit: usize) -> SqlResult<Vec<CommandEntry>> {
        let pattern = format!("{}%", prefix);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, command, frequency, last_used, directory, shell
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
                    last_used: row.get::<_, String>(3)?
                        .parse::<DateTime<Utc>>()
                        .unwrap_or_else(|_| Utc::now()),
                    directory: row.get(4)?,
                    shell: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    /// Get all commands (for fuzzy matching in engine).
    pub fn get_all_commands(&self) -> SqlResult<Vec<CommandEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, command, frequency, last_used, directory, shell FROM history ORDER BY frequency DESC",
        )?;

        let entries = stmt
            .query_map([], |row| {
                Ok(CommandEntry {
                    id: Some(row.get(0)?),
                    command: row.get(1)?,
                    frequency: row.get(2)?,
                    last_used: row.get::<_, String>(3)?
                        .parse::<DateTime<Utc>>()
                        .unwrap_or_else(|_| Utc::now()),
                    directory: row.get(4)?,
                    shell: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    pub fn get_total_commands(&self) -> SqlResult<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
    }

    /// Seed the database with default commands from JSON content.
    /// Only inserts commands that don't already exist (idempotent).
    pub fn seed_defaults(&self, json_content: &str) -> Result<usize, String> {
        let categories: HashMap<String, Vec<String>> = serde_json::from_str(json_content)
            .map_err(|e| format!("Failed to parse defaults JSON: {}", e))?;

        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();

        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| e.to_string())?;

        let mut count = 0;
        for (_category, commands) in &categories {
            for cmd in commands {
                let trimmed = cmd.trim();
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
                        "INSERT INTO history (command, frequency, last_used, directory, shell) VALUES (?1, 1, ?2, NULL, NULL)",
                        params![trimmed, now],
                    ).map_err(|e| e.to_string())?;
                    count += 1;
                }
            }
        }

        conn.execute_batch("COMMIT")
            .map_err(|e| e.to_string())?;

        Ok(count)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_search() {
        let store = HistoryStore::in_memory().unwrap();

        store.add_command("git init", None, Some("powershell")).unwrap();
        store.add_command("git commit -m \"test\"", None, Some("powershell")).unwrap();
        store.add_command("git push", None, Some("powershell")).unwrap();

        let results = store.search_by_prefix("git", 10).unwrap();
        assert_eq!(results.len(), 3);
        assert!(results[0].command.starts_with("git"));
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
            store.add_command("git commit -m \"msg\"", None, None).unwrap();
        }

        store.add_command("git push", None, None).unwrap();

        let results = store.search_by_prefix("git", 10).unwrap();
        assert_eq!(results[0].command, "git commit -m \"msg\"");
    }
}
