use chrono::Utc;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use serde::{Deserialize, Serialize};

use crate::engine::context::dynamic::generate_dynamic_candidates;
use crate::engine::context::generator::{generate_filesystem_candidates, ContextCandidate};
use crate::engine::context::path::shell_cwd_to_path;
use crate::engine::context::process::ProcessRunner;
use crate::engine::context::workspace::WorkspaceDetector;
use crate::engine::context::CommandContext;
use crate::storage::db::{CommandEntry, HistoryStore, LocalHistoryStats};

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
    workspace_detector: WorkspaceDetector,
    process_runner: ProcessRunner,
}

impl SuggestionEngine {
    pub fn new(store: HistoryStore) -> Self {
        Self {
            store,
            matcher: SkimMatcherV2::default(),
            workspace_detector: WorkspaceDetector::new(),
            process_runner: ProcessRunner::default(),
        }
    }

    /// Get suggestions for a given input.
    /// Strategy:
    /// 1. Try prefix match first (fast path from SQLite).
    /// 2. If not enough results, fall back to fuzzy match (in-memory).
    /// 3. Rank by combined score: frequency * 0.4 + recency * 0.3 + match_quality * 0.3
    pub fn suggest(&self, input: &str, limit: usize) -> Vec<Suggestion> {
        self.suggest_with_context(input, limit, None, None)
    }

    pub fn suggest_with_context(
        &self,
        input: &str,
        limit: usize,
        cwd: Option<&str>,
        _shell: Option<&str>,
    ) -> Vec<Suggestion> {
        if input.is_empty() {
            return vec![];
        }

        let local_history = cwd
            .filter(|directory| !directory.trim().is_empty())
            .and_then(|directory| {
                self.store
                    .get_local_history(&normalize_directory_key(directory))
                    .ok()
            })
            .unwrap_or_default();
        let all_commands = self.store.get_all_commands().unwrap_or_default();
        let all_matches: Vec<(CommandEntry, i64)> = all_commands
            .into_iter()
            .filter_map(|entry| {
                self.matcher
                    .fuzzy_match(&entry.command, input)
                    .map(|score| (entry, score))
            })
            .collect();

        let command_context = CommandContext::parse(input);
        let contextual: Vec<ContextCandidate> = cwd
            .and_then(|directory| {
                let path = shell_cwd_to_path(directory);
                path.is_dir().then(|| {
                    let filesystem = generate_filesystem_candidates(&command_context, &path);
                    let workspace = self.workspace_detector.detect(&path);
                    let dynamic = generate_dynamic_candidates(
                        &command_context,
                        &path,
                        &workspace,
                        &self.process_runner,
                    );
                    filesystem.into_iter().chain(dynamic).collect()
                })
            })
            .unwrap_or_default();
        let history_limit = limit.saturating_sub(contextual.len()).max(1);
        let history = self.multipass_rank(input, all_matches, history_limit, &local_history);

        merge_suggestions(input, history, contextual, limit)
    }

    fn multipass_rank(
        &self,
        input: &str,
        matches: Vec<(CommandEntry, i64)>,
        limit: usize,
        local_history: &std::collections::HashMap<String, LocalHistoryStats>,
    ) -> Vec<Suggestion> {
        let now = Utc::now();

        // 1. Get the single most recent matching command directly from DB
        let mut recent = Vec::new();
        let mut recent_command = None;
        if let Ok(Some(top_entry)) = self.store.get_most_recent_match(input) {
            recent_command = Some(top_entry.command.clone());
            let age_seconds = (now - top_entry.last_used).num_seconds().max(1) as f64;
            // Recency score that decays slowly (by minutes)
            let recency_score = 1000.0 / ((age_seconds / 60.0).sqrt() + 1.0);
            let freq_score = (top_entry.frequency as f64).ln().max(0.0) * 10.0;
            let (local_bonus, is_local) = local_history_bonus(&top_entry.command, local_history);
            recent.push(Suggestion {
                command: top_entry.command,
                description: top_entry.description,
                score: freq_score * 0.35 + recency_score * 0.4 + 25.0 + local_bonus,
                frequency: top_entry.frequency,
                source: if is_local {
                    "history-local".to_string()
                } else {
                    "history-global".to_string()
                },
            });
        }

        // 2. Score and categorize remaining entries
        let all_entries: Vec<(CommandEntry, f64, bool)> = matches
            .into_iter()
            .filter(|(entry, _)| {
                // Skip the one already added to 'recent'
                recent_command.as_ref() != Some(&entry.command)
            })
            .map(|(entry, match_score)| {
                let age_seconds = (now - entry.last_used).num_seconds().max(1) as f64;
                let freq_score = (entry.frequency as f64).ln().max(0.0) * 10.0;
                let recency_score = 1000.0 / ((age_seconds / 60.0).sqrt() + 1.0);
                let (local_bonus, is_local) = local_history_bonus(&entry.command, local_history);
                let sub_score = freq_score * 0.35
                    + recency_score * 0.4
                    + (match_score as f64) * 0.25
                    + local_bonus;
                (entry, sub_score, is_local)
            })
            .collect();

        let mut defaults = Vec::new();
        let mut popular = Vec::new();
        let mut others = Vec::new();

        for (entry, sub_score, is_local) in all_entries {
            let source = if is_local {
                "history-local".to_string()
            } else if entry.source == "default" {
                "default".to_string()
            } else if entry.frequency >= 3 {
                "frequent".to_string()
            } else {
                "history-global".to_string()
            };
            if entry.source == "default" {
                defaults.push(Suggestion {
                    command: entry.command,
                    description: entry.description,
                    score: sub_score,
                    frequency: entry.frequency,
                    source,
                });
            } else if entry.frequency >= 3 {
                popular.push(Suggestion {
                    command: entry.command,
                    description: entry.description,
                    score: sub_score,
                    frequency: entry.frequency,
                    source,
                });
            } else {
                others.push(Suggestion {
                    command: entry.command,
                    description: entry.description,
                    score: sub_score,
                    frequency: entry.frequency,
                    source,
                });
            }
        }

        // Sort by score
        let sort_by_score = |a: &Suggestion, b: &Suggestion| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        };
        defaults.sort_by(sort_by_score);
        popular.sort_by(sort_by_score);
        others.sort_by(sort_by_score);

        // Only keep tag "frequent" on the #1 most used, rest become regular
        for s in popular.iter_mut().skip(1) {
            if s.source != "history-local" {
                s.source = "history-global".to_string();
            }
        }

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

    pub fn add_command(
        &self,
        command: &str,
        directory: Option<&str>,
        shell: Option<&str>,
    ) -> Result<(), String> {
        let directory = directory.map(normalize_directory_key);
        self.store
            .add_command(command, directory.as_deref(), shell)
            .map_err(|e| e.to_string())
    }

    pub fn total_commands(&self) -> i64 {
        self.store.get_total_commands().unwrap_or(0)
    }

    pub fn seed_defaults(&self, json_content: &str) -> Result<usize, String> {
        self.store.seed_defaults(json_content)
    }
}

fn normalize_directory_key(directory: &str) -> String {
    shell_cwd_to_path(directory).to_string_lossy().to_string()
}

fn merge_suggestions(
    input: &str,
    history: Vec<Suggestion>,
    contextual: Vec<ContextCandidate>,
    limit: usize,
) -> Vec<Suggestion> {
    let mut by_command = std::collections::HashMap::<String, Suggestion>::new();

    for suggestion in history {
        merge_prefer_higher_score(&mut by_command, suggestion);
    }
    for candidate in contextual {
        merge_prefer_higher_score(
            &mut by_command,
            Suggestion {
                command: candidate.command,
                description: candidate.description,
                score: candidate.score,
                frequency: 0,
                source: candidate.source,
            },
        );
    }

    let mut suggestions = by_command.into_values().collect::<Vec<_>>();
    suggestions.sort_by(|left, right| {
        match_tier(left, input)
            .cmp(&match_tier(right, input))
            .then_with(|| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.command.cmp(&right.command))
    });
    suggestions.truncate(limit);
    suggestions
}

fn match_tier(suggestion: &Suggestion, input: &str) -> u8 {
    if suggestion.command.starts_with(input) {
        0
    } else if matches!(
        suggestion.source.as_str(),
        "path" | "git" | "npm" | "docker" | "ssh" | "zoxide"
    ) {
        1
    } else {
        2
    }
}

fn merge_prefer_higher_score(
    by_command: &mut std::collections::HashMap<String, Suggestion>,
    candidate: Suggestion,
) {
    let key = normalize_command(&candidate.command);
    match by_command.get(&key) {
        Some(existing) if existing.score >= candidate.score => {}
        _ => {
            by_command.insert(key, candidate);
        }
    }
}

fn normalize_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn local_history_bonus(
    command: &str,
    local_history: &std::collections::HashMap<String, LocalHistoryStats>,
) -> (f64, bool) {
    let Some(stats) = local_history.get(command) else {
        return (0.0, false);
    };

    let age_seconds = (Utc::now() - stats.last_used).num_seconds().max(1) as f64;
    let frequency = (stats.frequency as f64).ln_1p() * 7.0;
    let recency = 18.0 / ((age_seconds / 60.0).sqrt() + 1.0);
    ((frequency + recency).min(30.0), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::HistoryStore;

    fn create_engine_with_data() -> SuggestionEngine {
        let store = HistoryStore::in_memory().unwrap();

        store.add_command("git init", None, None).unwrap();
        store
            .add_command("git commit -m \"initial\"", None, None)
            .unwrap();
        store
            .add_command("git push origin main", None, None)
            .unwrap();
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
            store
                .add_command("git commit -m \"initial\"", None, None)
                .unwrap();
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
    fn context_candidates_merge_once_and_do_not_displace_stronger_history() {
        let history = vec![Suggestion {
            command: "npm run dev".to_string(),
            description: None,
            score: 70.0,
            frequency: 4,
            source: "history-local".to_string(),
        }];
        let contextual = vec![
            ContextCandidate {
                command: "npm   run dev".to_string(),
                description: Some("package script".to_string()),
                source: "npm".to_string(),
                score: 20.0,
            },
            ContextCandidate {
                command: "npm run test".to_string(),
                description: Some("package script".to_string()),
                source: "npm".to_string(),
                score: 20.0,
            },
        ];

        let suggestions = merge_suggestions("npm run ", history, contextual, 5);
        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].command, "npm run dev");
        assert_eq!(suggestions[0].source, "history-local");
        assert_eq!(suggestions[1].command, "npm run test");
    }

    #[test]
    fn prefix_history_ranks_before_contextual_candidate() {
        let history = vec![Suggestion {
            command: "cd source".to_string(),
            description: None,
            score: 50.0,
            frequency: 1,
            source: "history-global".to_string(),
        }];
        let contextual = vec![ContextCandidate {
            command: "cd src/".to_string(),
            description: Some("directory".to_string()),
            source: "path".to_string(),
            score: 12.0,
        }];

        let suggestions = merge_suggestions("cd s", history, contextual, 5);
        assert_eq!(suggestions[0].command, "cd source");
        assert_eq!(suggestions[1].command, "cd src/");
    }

    #[test]
    fn contextual_candidate_ranks_before_unrelated_fuzzy_history() {
        let history = vec![Suggestion {
            command: "git add src/main.rs".to_string(),
            description: None,
            score: 85.0,
            frequency: 1,
            source: "history-global".to_string(),
        }];
        let contextual = vec![ContextCandidate {
            command: "cd core/src/engine/".to_string(),
            description: Some("directory".to_string()),
            source: "path".to_string(),
            score: 12.0,
        }];

        let suggestions = merge_suggestions("cd core/src/eng", history, contextual, 5);
        assert_eq!(suggestions[0].command, "cd core/src/engine/");
    }

    #[test]
    fn reserves_result_slots_for_directory_candidates() {
        let store = HistoryStore::in_memory().unwrap();
        let engine = SuggestionEngine::new(store);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("cli")).unwrap();
        std::fs::create_dir(temp.path().join("core")).unwrap();
        std::fs::create_dir(temp.path().join("target")).unwrap();

        for index in 0..10 {
            engine
                .add_command(&format!("cd historical-{index}"), None, None)
                .unwrap();
        }

        let direct = generate_filesystem_candidates(&CommandContext::parse("cd "), temp.path());
        assert!(direct
            .iter()
            .any(|candidate| candidate.command == "cd cli/"));

        let suggestions = engine.suggest_with_context("cd ", 5, temp.path().to_str(), Some("bash"));
        assert!(
            suggestions
                .iter()
                .any(|suggestion| suggestion.command == "cd cli/" && suggestion.source == "path"),
            "suggestions: {suggestions:?}"
        );
    }

    #[test]
    fn test_prefix_suggestions() {
        let engine = create_engine_with_data();
        let suggestions = engine.suggest("git", 5);

        assert!(!suggestions.is_empty());
        assert!(suggestions.len() <= 5);

        // "git init" was most recently added -> should be first (recent tier)
        assert_eq!(suggestions[0].command, "git init");
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
    fn test_contextual_local_history_boosts_matching_command() {
        let store = HistoryStore::in_memory().unwrap();
        let engine = SuggestionEngine::new(store);

        for _ in 0..3 {
            engine
                .add_command(
                    "cargo check",
                    Some("C:/projects/hintshell"),
                    Some("powershell"),
                )
                .unwrap();
        }
        engine
            .add_command(
                "cargo clippy",
                Some("C:/projects/other"),
                Some("powershell"),
            )
            .unwrap();

        let suggestions = engine.suggest_with_context(
            "cargo c",
            5,
            Some("C:/projects/hintshell"),
            Some("powershell"),
        );
        let local = suggestions
            .iter()
            .find(|suggestion| suggestion.command == "cargo check")
            .unwrap();

        assert_eq!(local.source, "history-local");
        assert!(suggestions
            .iter()
            .any(|suggestion| suggestion.command == "cargo clippy"));
    }

    #[test]
    fn test_missing_context_preserves_global_history_source() {
        let store = HistoryStore::in_memory().unwrap();
        let engine = SuggestionEngine::new(store);
        engine
            .add_command("cargo check", Some("C:/projects/hintshell"), None)
            .unwrap();

        let suggestion = engine.suggest("cargo c", 1).pop().unwrap();
        assert_eq!(suggestion.source, "history-global");
    }

    #[test]
    fn test_multipass_ranking_order() {
        let store = HistoryStore::in_memory().unwrap();
        let engine = SuggestionEngine::new(store);

        // 1. Others
        engine.add_command("git others", None, None).unwrap();

        // 2. Default
        engine
            .seed_defaults(r#"{ "test": [{ "command": "git default", "description": "" }] }"#)
            .unwrap();

        // 3. Most used
        for _ in 0..5 {
            engine.add_command("git most_used", None, None).unwrap();
        }

        // We'll simulate `git others` by backdating its last_used so it doesn't count as recent.
        // And backdate `git most_used` too.
        engine
            .store
            .set_last_used_for_test("git others", "2000-01-01T00:00:00Z");
        engine
            .store
            .set_last_used_for_test("git most_used", "2000-01-01T00:00:00Z");

        // 4. Recent
        engine.add_command("git recent", None, None).unwrap();

        let suggestions = engine.suggest("git", 10);
        assert!(suggestions.len() >= 4);

        let commands = suggestions
            .iter()
            .map(|suggestion| suggestion.command.as_str())
            .collect::<Vec<_>>();
        assert_eq!(suggestions[0].command, "git recent");
        assert!(commands.contains(&"git default"));
        assert!(commands.contains(&"git most_used"));
        assert!(commands.contains(&"git others"));
    }

    #[test]
    fn test_multipass_dedup() {
        let store = HistoryStore::in_memory().unwrap();
        let engine = SuggestionEngine::new(store);

        // Command acts as BOTH default and recent
        engine
            .seed_defaults(r#"{ "test": [{ "command": "git duplicate", "description": "" }] }"#)
            .unwrap();
        engine.add_command("git duplicate", None, None).unwrap();

        let suggestions = engine.suggest("git dup", 10);

        // Should only appear once
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].command, "git duplicate");
    }
}
