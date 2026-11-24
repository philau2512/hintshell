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

/// How much damage a wrong acceptance can do. Surfaced by every shell integration so that
/// destructive commands are never presented with the same visual weight as read-only ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Normal,
    Destructive,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Destructive => "destructive",
        }
    }
}

impl Default for Risk {
    fn default() -> Self {
        Self::Normal
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub command: String,
    pub description: Option<String>,
    pub score: f64,
    pub frequency: i64,
    pub source: String,
    /// Match quality tier from [`match_tier`]: 0 prefix, 1 contextual, 2 substring,
    /// 3 typo-tolerant, 4 fuzzy. Integrations use it to explain ranking.
    #[serde(default)]
    pub tier: u8,
    #[serde(default)]
    pub risk: Risk,
}

impl Default for Suggestion {
    fn default() -> Self {
        Self {
            command: String::new(),
            description: None,
            score: 0.0,
            frequency: 0,
            source: String::new(),
            tier: 4,
            risk: Risk::default(),
        }
    }
}

/// Commands whose acceptance can destroy work the user cannot easily recover.
/// Deliberately narrow: flagging too much makes the marker meaningless.
const DESTRUCTIVE_PREFIXES: &[&str] = &[
    "rm -rf",
    "rm -fr",
    "rm -r ",
    "sudo rm",
    "git reset --hard",
    "git clean -f",
    "git checkout --",
    "git push --force",
    "git push -f",
    "git branch -D",
    "git stash drop",
    "git filter-branch",
    "git gc --prune",
    "docker system prune",
    "docker rm -f",
    "docker volume rm",
    "docker rmi $(docker",
    "docker rm $(docker",
    "terraform destroy",
    "kubectl delete",
    "helm uninstall",
    "dd if=",
    "mkfs",
    "chmod -R 777",
    "chown -R",
    "kill -9",
    "killall",
    "pkill",
    "remove-item -recurse -force",
    "clear-history",
    "set-executionpolicy",
];

/// Flags whose case carries meaning, matched against the command as typed:
/// `git branch -d` is the safe delete, `git branch -D` discards unmerged work.
const DESTRUCTIVE_CASE_SENSITIVE: &[&str] = &["git branch -D"];

/// Destructive intent that only shows up later in the line (pipes, suffixes, second command).
const DESTRUCTIVE_SUBSTRINGS: &[&str] = &[
    " -delete",
    "| xargs rm",
    "&& rm -rf",
    "; rm -rf",
    "prune -af",
    "prune -a ",
    "--force-with-lease",
];

pub fn risk_for_command(command: &str) -> Risk {
    let trimmed = command.trim();
    if DESTRUCTIVE_CASE_SENSITIVE
        .iter()
        .any(|needle| trimmed.starts_with(needle))
    {
        return Risk::Destructive;
    }
    let compact = trimmed.to_ascii_lowercase().replace('\t', " ");
    if DESTRUCTIVE_PREFIXES
        .iter()
        .any(|prefix| compact.starts_with(prefix))
        || DESTRUCTIVE_SUBSTRINGS.iter().any(|needle| compact.contains(needle))
    {
        return Risk::Destructive;
    }
    Risk::Normal
}

/// Short, user-facing name for where a suggestion came from. Surfaces must not print the
/// internal source keys ("history-local", "zoxide", …) because they teach nothing.
pub fn source_label(source: &str) -> &'static str {
    match source {
        "history-local" => "here",
        "history-global" | "history" => "history",
        "frequent" => "most used",
        "recent" => "recent",
        "default" => "starter",
        "path" => "path",
        "git" => "git",
        "npm" | "pnpm" | "yarn" | "bun" => "scripts",
        "docker" => "docker",
        "ssh" => "ssh hosts",
        "zoxide" => "folders",
        "cargo" => "cargo",
        _ => "history",
    }
}

/// Match-quality tag shown next to the label: "" for a prefix match (what the user typed
/// actually starts the command), "~" for substring/fuzzy, "≈" for typo tolerance.
pub fn tier_tag(tier: u8) -> &'static str {
    match tier {
        0 | 1 => "",
        3 => "≈",
        _ => "~",
    }
}

/// Frequency column text; contextual candidates have no usage count, so they show nothing
/// instead of the invented "1x".
pub fn frequency_label(frequency: i64) -> String {
    if frequency > 0 {
        format!("{frequency}x")
    } else {
        String::new()
    }
}

/// Shared fallback for the description line so every surface answers the same way.
pub const NO_DESCRIPTION: &str = "no description";

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
                if let Some(score) = self.matcher.fuzzy_match(&entry.command, input) {
                    Some((entry, score))
                } else if typo_tolerance_match(&entry.command, input) {
                    Some((entry, 10))
                } else {
                    None
                }
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
                ..Default::default()
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
                    ..Default::default()
                });
            } else if entry.frequency >= 3 {
                popular.push(Suggestion {
                    command: entry.command,
                    description: entry.description,
                    score: sub_score,
                    frequency: entry.frequency,
                    source,
                    ..Default::default()
                });
            } else {
                others.push(Suggestion {
                    command: entry.command,
                    description: entry.description,
                    score: sub_score,
                    frequency: entry.frequency,
                    source,
                    ..Default::default()
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

        let all_tiers = vec![recent, popular, others, defaults];
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

    pub fn prune_user_history(
        &self,
        retention_days: u64,
        dry_run: bool,
    ) -> Result<crate::storage::db::HistoryPruneResult, String> {
        let cutoff = Utc::now()
            - chrono::Duration::days(
                i64::try_from(retention_days)
                    .map_err(|_| "retention period is too large".to_string())?,
            );
        self.store
            .prune_user_history(cutoff, dry_run)
            .map_err(|error| error.to_string())
    }

    pub fn prune_user_history_daily(
        &self,
        retention_days: u64,
    ) -> Result<Option<crate::storage::db::HistoryPruneResult>, String> {
        let now = Utc::now();
        let cutoff = now
            - chrono::Duration::days(
                i64::try_from(retention_days)
                    .map_err(|_| "retention period is too large".to_string())?,
            );
        self.store
            .prune_user_history_daily(now, cutoff, false)
            .map_err(|error| error.to_string())
    }

    pub fn total_commands(&self) -> i64 {
        self.store.get_total_commands().unwrap_or(0)
    }

    pub fn seed_defaults(&self, json_content: &str) -> Result<usize, String> {
        self.store.seed_defaults(json_content)
    }
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_len = a.chars().count();
    let b_len = b.chars().count();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row: Vec<usize> = vec![0; b_len + 1];

    for (i, ca) in a.chars().enumerate() {
        curr_row[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr_row[j + 1] = (curr_row[j] + 1)
                .min(prev_row[j + 1] + 1)
                .min(prev_row[j] + cost);
        }
        prev_row.copy_from_slice(&curr_row);
    }

    prev_row[b_len]
}

fn typo_tolerance_match(command: &str, input: &str) -> bool {
    let input = input.trim();
    if input.len() < 3 {
        return false;
    }
    let input_first = input.split_whitespace().next().unwrap_or_default();
    let cmd_first = command.split_whitespace().next().unwrap_or_default();
    if input_first.is_empty() || cmd_first.is_empty() {
        return false;
    }

    // If typing single word command with typo (e.g. gti -> git, dcoker -> docker)
    if !input.contains(' ') {
        levenshtein_distance(input_first, cmd_first) <= 1
    } else {
        // If input has multiple words: check first word typo AND matching rest
        if levenshtein_distance(input_first, cmd_first) <= 1 {
            let input_rest = input[input_first.len()..].trim_start();
            let cmd_rest = command[cmd_first.len()..].trim_start();
            cmd_rest.starts_with(input_rest) || cmd_rest.contains(input_rest)
        } else {
            false
        }
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
                ..Default::default()
            },
        );
    }

    let mut suggestions = by_command.into_values().collect::<Vec<_>>();
    // Publish why each item ranks where it does, and how much damage it can do, so every
    // surface can explain itself instead of showing an unlabelled list.
    for suggestion in &mut suggestions {
        suggestion.tier = match_tier(suggestion, input);
        suggestion.risk = risk_for_command(&suggestion.command);
    }
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
    let cmd_lower = suggestion.command.to_ascii_lowercase();
    let input_lower = input.to_ascii_lowercase();
    if cmd_lower.starts_with(&input_lower) {
        0
    } else if matches!(
        suggestion.source.as_str(),
        "path" | "git" | "npm" | "docker" | "ssh" | "zoxide" | "cargo" | "pnpm" | "bun" | "yarn"
    ) {
        1
    } else if cmd_lower.contains(&input_lower) {
        2
    } else if typo_tolerance_match(&cmd_lower, &input_lower) {
        3
    } else {
        4
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
    fn merge_publishes_match_tier_and_risk_for_integrations() {
        let history = vec![
            Suggestion {
                command: "git status".to_string(),
                description: Some("Show the working tree status".to_string()),
                score: 90.0,
                frequency: 12,
                source: "history-global".to_string(),
                ..Default::default()
            },
            Suggestion {
                command: "git commit -m \"initial\"".to_string(),
                description: None,
                score: 40.0,
                frequency: 2,
                source: "history-global".to_string(),
                ..Default::default()
            },
            Suggestion {
                command: "rm -rf build".to_string(),
                description: None,
                score: 10.0,
                frequency: 1,
                source: "history-global".to_string(),
                ..Default::default()
            },
        ];

        let suggestions = merge_suggestions("git s", history.clone(), Vec::new(), 10);
        let status = suggestions
            .iter()
            .find(|suggestion| suggestion.command == "git status")
            .unwrap();
        assert_eq!(status.tier, 0, "prefix match must be advertised as tier 0");
        assert_eq!(status.risk, Risk::Normal);

        let commit = suggestions
            .iter()
            .find(|suggestion| suggestion.command.starts_with("git commit"))
            .unwrap();
        assert_eq!(commit.tier, 4, "fuzzy-only match must not look like a prefix match");

        // A destructive candidate is ranked by relevance but still flagged for the surface.
        let suggestions = merge_suggestions("rm -", history, Vec::new(), 10);
        let rm = suggestions
            .iter()
            .find(|suggestion| suggestion.command == "rm -rf build")
            .unwrap();
        assert_eq!(rm.risk, Risk::Destructive);
    }

    #[test]
    fn source_labels_hide_internal_keys() {
        assert_eq!(source_label("history-local"), "here");
        assert_eq!(source_label("zoxide"), "folders");
        assert_eq!(source_label("something-unknown"), "history");
        assert_eq!(tier_tag(0), "");
        assert_eq!(tier_tag(3), "≈");
        assert_eq!(tier_tag(4), "~");
        assert_eq!(frequency_label(0), "");
        assert_eq!(frequency_label(12), "12x");
    }

    #[test]
    fn risk_classification_keeps_case_sensitive_and_suffix_signals() {
        assert_eq!(risk_for_command("git branch -D leftover"), Risk::Destructive);
        assert_eq!(risk_for_command("git branch -d merged"), Risk::Normal);
        assert_eq!(risk_for_command("docker system prune -af --volumes"), Risk::Destructive);
        assert_eq!(
            risk_for_command("find . -type f -name \"*.log\" -delete"),
            Risk::Destructive
        );
        assert_eq!(risk_for_command("git reset --hard HEAD"), Risk::Destructive);
        assert_eq!(risk_for_command("git status"), Risk::Normal);
        assert_eq!(risk_for_command("docker ps -a"), Risk::Normal);
    }

    #[test]
    fn context_candidates_merge_once_and_do_not_displace_stronger_history() {
        let history = vec![Suggestion {
            command: "npm run dev".to_string(),
            description: None,
            score: 70.0,
            frequency: 4,
            source: "history-local".to_string(),
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
        // Đảm bảo 1 lệnh lịch sử gần nhất có điểm cao nhất đứng đầu, và các thư mục path nằm ngay bên dưới
        assert_eq!(suggestions[0].command, "cd historical-9");
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
