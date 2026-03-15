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
    pub source: String,
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

        let all_commands = self.store.get_all_commands().unwrap_or_default();
        let all_matches: Vec<(CommandEntry, i64)> = all_commands
            .into_iter()
            .filter_map(|entry| {
                self.matcher
                    .fuzzy_match(&entry.command, input)
                    .map(|score| (entry, score))
            })
            .collect();

        self.multipass_rank(all_matches, limit)
    }

    fn multipass_rank(&self, matches: Vec<(CommandEntry, i64)>, limit: usize) -> Vec<Suggestion> {
        let now = Utc::now();
        let mut recent = Vec::new();
        let mut defaults = Vec::new();
        let mut popular = Vec::new();
        let mut others = Vec::new();

        for (entry, match_score) in matches {
            let age_seconds = (now - entry.last_used).num_seconds().max(1) as f64;
            let freq_score = (entry.frequency as f64).ln().max(0.0) * 10.0;
            let recency_score = 100.0 / age_seconds.sqrt();
            let sub_score = freq_score * 0.6 + recency_score * 0.15 + (match_score as f64) * 0.25;

            let suggestion = Suggestion {
                command: entry.command.clone(),
                description: entry.description.clone(),
                score: sub_score,
                frequency: entry.frequency,
                source: entry.source.clone(),
            };

            // Categorize
            if entry.source == "user" && age_seconds < 1800.0 {
                recent.push(suggestion);
            } else if entry.source == "default" {
                defaults.push(suggestion);
            } else if entry.frequency >= 3 {
                popular.push(suggestion);
            } else {
                others.push(suggestion);
            }
        }

        // Sort each tier internally by sub_score
        let sort_by_score = |a: &Suggestion, b: &Suggestion| {
            b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
        };
        recent.sort_by(sort_by_score);
        defaults.sort_by(sort_by_score);
        popular.sort_by(sort_by_score);
        others.sort_by(sort_by_score);

        // Merge and dedup
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        let all_tiers = vec![recent, defaults, popular, others];
        for tier in all_tiers {
            for s in tier {
                if !seen.contains(&s.command) {
                    seen.insert(s.command.clone());
                    result.push(s);
                    if result.len() >= limit {
                        return result;
                    }
                }
            }
        }

        result
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

        // Add some default commands
        let defaults = r#"{ "default": [
            { "command": "cargo default", "description": "D1" },
            { "command": "git clone", "description": "D2" }
        ] }"#;
        store.seed_defaults(defaults).unwrap();

        // Make "git init" recent by re-adding it
        store.add_command("git init", None, None).unwrap();

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

    #[test]
    fn test_multipass_ranking_order() {
        let store = HistoryStore::in_memory().unwrap();
        let engine = SuggestionEngine::new(store);
        
        // 1. Others
        engine.add_command("git others", None, None).unwrap();
        
        // 2. Default
        engine.seed_defaults(r#"{ "test": [{ "command": "git default", "description": "" }] }"#).unwrap();
        
        // 3. Most used
        for _ in 0..5 {
            engine.add_command("git most_used", None, None).unwrap();
        }
        
        // We'll simulate `git others` by backdating its last_used so it doesn't count as recent.
        // And backdate `git most_used` too.
        engine.store.set_last_used_for_test("git others", "2000-01-01T00:00:00Z");
        engine.store.set_last_used_for_test("git most_used", "2000-01-01T00:00:00Z");
        
        // 4. Recent
        engine.add_command("git recent", None, None).unwrap();

        let suggestions = engine.suggest("git", 10);
        assert!(suggestions.len() >= 4);

        // Tier 1: Recent ("git recent")
        assert_eq!(suggestions[0].command, "git recent");
        
        // Tier 2: Default ("git default")
        assert_eq!(suggestions[1].command, "git default");
        
        // Tier 3: Most used ("git most_used")
        assert_eq!(suggestions[2].command, "git most_used");
        
        // Tier 4: Others ("git others")
        assert_eq!(suggestions[3].command, "git others");
    }

    #[test]
    fn test_multipass_dedup() {
        let store = HistoryStore::in_memory().unwrap();
        let engine = SuggestionEngine::new(store);
        
        // Command acts as BOTH default and recent
        engine.seed_defaults(r#"{ "test": [{ "command": "git duplicate", "description": "" }] }"#).unwrap();
        engine.add_command("git duplicate", None, None).unwrap();
        
        let suggestions = engine.suggest("git dup", 10);
        
        // Should only appear once
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].command, "git duplicate");
    }
}
