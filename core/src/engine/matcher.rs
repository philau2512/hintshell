use chrono::Utc;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use serde::{Deserialize, Serialize};

use crate::storage::db::{CommandEntry, HistoryStore};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub command: String,
    pub description: Option<String>,
    pub score: f64,
    pub frequency: i64,
}

pub struct SuggestionEngine {
    store: HistoryStore,
    matcher: SkimMatcherV2,
}

impl SuggestionEngine {
    pub fn new(store: HistoryStore) -> Self {
        Self {
            store,
            matcher: SkimMatcherV2::default(),
        }
    }

    /// Get suggestions for a given input.
    /// Strategy:
    /// 1. Try prefix match first (fast path from SQLite).
    /// 2. If not enough results, fall back to fuzzy match (in-memory).
    /// 3. Rank by combined score: frequency * 0.4 + recency * 0.3 + match_quality * 0.3
    pub fn suggest(&self, input: &str, limit: usize) -> Vec<Suggestion> {
        if input.is_empty() {
            return vec![];
        }

        // Fast path: prefix match from SQLite
        let prefix_results = self.store.search_by_prefix(input, limit).unwrap_or_default();

        if prefix_results.len() >= limit {
            return self.rank_entries(prefix_results, input, limit);
        }

        // Slow path: fuzzy match all commands
        let all_commands = self.store.get_all_commands().unwrap_or_default();
        let mut candidates: Vec<(CommandEntry, i64)> = all_commands
            .into_iter()
            .filter_map(|entry| {
                self.matcher
                    .fuzzy_match(&entry.command, input)
                    .map(|score| (entry, score))
            })
            .collect();

        candidates.sort_by(|a, b| b.1.cmp(&a.1));

        let entries: Vec<CommandEntry> = candidates
            .into_iter()
            .map(|(entry, _)| entry)
            .take(limit * 2) // take more for re-ranking
            .collect();

        self.rank_entries(entries, input, limit)
    }

    fn rank_entries(&self, entries: Vec<CommandEntry>, input: &str, limit: usize) -> Vec<Suggestion> {
        let now = Utc::now();

        let mut scored: Vec<Suggestion> = entries
            .into_iter()
            .map(|entry| {
                let freq_score = (entry.frequency as f64).ln().max(0.0) * 10.0;

                let age_seconds = (now - entry.last_used).num_seconds().max(1) as f64;
                let recency_score = 100.0 / age_seconds.sqrt();

                let match_score = self
                    .matcher
                    .fuzzy_match(&entry.command, input)
                    .unwrap_or(0) as f64;

                let total_score =
                    freq_score * 0.4 + recency_score * 0.3 + match_score * 0.3;

                Suggestion {
                    command: entry.command,
                    description: entry.description,
                    score: total_score,
                    frequency: entry.frequency,
                }
            })
            .collect();

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }

    pub fn add_command(&self, command: &str, directory: Option<&str>, shell: Option<&str>) -> Result<(), String> {
        self.store
            .add_command(command, directory, shell)
            .map_err(|e| e.to_string())
    }

    pub fn total_commands(&self) -> i64 {
        self.store.get_total_commands().unwrap_or(0)
    }

    pub fn seed_defaults(&self, json_content: &str) -> Result<usize, String> {
        self.store.seed_defaults(json_content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::HistoryStore;

    fn create_engine_with_data() -> SuggestionEngine {
        let store = HistoryStore::in_memory().unwrap();

        store.add_command("git init", None, None).unwrap();
        store.add_command("git commit -m \"initial\"", None, None).unwrap();
        store.add_command("git push origin main", None, None).unwrap();
        store.add_command("git status", None, None).unwrap();
        store.add_command("git log --oneline", None, None).unwrap();
        store.add_command("cargo build", None, None).unwrap();
        store.add_command("cargo test", None, None).unwrap();
        store.add_command("cargo run", None, None).unwrap();

        // Simulate frequent use
        for _ in 0..10 {
            store.add_command("git status", None, None).unwrap();
        }
        for _ in 0..5 {
            store.add_command("git commit -m \"initial\"", None, None).unwrap();
        }

        SuggestionEngine::new(store)
    }

    #[test]
    fn test_prefix_suggestions() {
        let engine = create_engine_with_data();
        let suggestions = engine.suggest("git", 5);

        assert!(!suggestions.is_empty());
        assert!(suggestions.len() <= 5);

        // "git status" used most -> should be first
        assert_eq!(suggestions[0].command, "git status");
    }

    #[test]
    fn test_fuzzy_suggestions() {
        let engine = create_engine_with_data();

        // "gt st" should fuzzy match "git status"
        let suggestions = engine.suggest("gt st", 3);
        assert!(!suggestions.is_empty());

        let commands: Vec<&str> = suggestions.iter().map(|s| s.command.as_str()).collect();
        assert!(commands.contains(&"git status"));
    }

    #[test]
    fn test_empty_input() {
        let engine = create_engine_with_data();
        let suggestions = engine.suggest("", 5);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_no_match() {
        let engine = create_engine_with_data();
        let suggestions = engine.suggest("zzzzzzzzz", 5);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_cargo_suggestions() {
        let engine = create_engine_with_data();
        let suggestions = engine.suggest("cargo", 5);

        assert!(!suggestions.is_empty());
        for s in &suggestions {
            assert!(s.command.starts_with("cargo"));
        }
    }
}
