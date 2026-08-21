use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPruneSummary {
    pub candidate_count: i64,
    pub deleted_count: i64,
}

/// Request sent from Shell integration to HintShell Daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum HintShellRequest {
    /// Get suggestions for a partial command input.
    #[serde(rename = "suggest")]
    Suggest {
        input: String,
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
    },

    /// Record a command that was just executed.
    #[serde(rename = "add")]
    AddCommand {
        command: String,
        directory: Option<String>,
        shell: Option<String>,
    },

    /// Remove stale, low-frequency user commands from history.
    #[serde(rename = "prune_history")]
    PruneHistory { days: u64, dry_run: bool },

    /// Get daemon status info.
    #[serde(rename = "status")]
    Status,

    /// Gracefully stop the daemon.
    #[serde(rename = "shutdown")]
    Shutdown,
}

fn default_limit() -> usize {
    5
}

/// Response sent from HintShell Daemon back to Shell integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HintShellResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<SuggestionItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<DaemonStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_prune: Option<HistoryPruneSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionItem {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub score: f64,
    pub frequency: i64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub total_commands: i64,
    pub uptime_seconds: u64,
}

impl HintShellResponse {
    pub fn ok_suggestions(items: Vec<SuggestionItem>) -> Self {
        Self {
            success: true,
            suggestions: Some(items),
            status: None,
            history_prune: None,
            error: None,
        }
    }

    pub fn ok_status(status: DaemonStatus) -> Self {
        Self {
            success: true,
            suggestions: None,
            status: Some(status),
            history_prune: None,
            error: None,
        }
    }

    pub fn ok_history_prune(history_prune: HistoryPruneSummary) -> Self {
        Self {
            success: true,
            suggestions: None,
            status: None,
            history_prune: Some(history_prune),
            error: None,
        }
    }

    pub fn ok_empty() -> Self {
        Self {
            success: true,
            suggestions: None,
            status: None,
            history_prune: None,
            error: None,
        }
    }

    pub fn err(msg: &str) -> Self {
        Self {
            success: false,
            suggestions: None,
            status: None,
            history_prune: None,
            error: Some(msg.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_suggest_request() {
        let req = HintShellRequest::Suggest {
            input: "git c".to_string(),
            limit: 5,
            cwd: Some("/workspace/hintshell".to_string()),
            shell: Some("bash".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("suggest"));
        assert!(json.contains("git c"));
        assert!(json.contains("/workspace/hintshell"));
        assert!(json.contains("bash"));
    }

    #[test]
    fn test_serialize_prune_history_request() {
        let request = HintShellRequest::PruneHistory {
            days: 30,
            dry_run: true,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("prune_history"));
        assert!(json.contains("\"days\":30"));
        assert!(json.contains("\"dry_run\":true"));
    }

    #[test]
    fn test_deserialize_suggest_request() {
        let json = r#"{"action":"suggest","input":"git c","limit":5}"#;
        let req: HintShellRequest = serde_json::from_str(json).unwrap();
        match req {
            HintShellRequest::Suggest {
                input,
                limit,
                cwd,
                shell,
            } => {
                assert_eq!(input, "git c");
                assert_eq!(limit, 5);
                assert!(cwd.is_none());
                assert!(shell.is_none());
            }
            _ => panic!("Expected Suggest variant"),
        }
    }

    #[test]
    fn test_deserialize_legacy_suggest_request_without_context() {
        let json = r#"{"action":"suggest","input":"git c","limit":5}"#;
        let req: HintShellRequest = serde_json::from_str(json).unwrap();
        match req {
            HintShellRequest::Suggest { cwd, shell, .. } => {
                assert!(cwd.is_none());
                assert!(shell.is_none());
            }
            _ => panic!("Expected Suggest variant"),
        }
    }

    #[test]
    fn test_serialize_response() {
        let resp = HintShellResponse::ok_suggestions(vec![SuggestionItem {
            command: "git commit".to_string(),
            description: None,
            score: 95.0,
            frequency: 10,
            source: "user".to_string(),
        }]);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("git commit"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_add_command_request() {
        let json =
            r#"{"action":"add","command":"git push","directory":"/home/user","shell":"bash"}"#;
        let req: HintShellRequest = serde_json::from_str(json).unwrap();
        match req {
            HintShellRequest::AddCommand {
                command,
                directory,
                shell,
            } => {
                assert_eq!(command, "git push");
                assert_eq!(directory.unwrap(), "/home/user");
                assert_eq!(shell.unwrap(), "bash");
            }
            _ => panic!("Expected AddCommand variant"),
        }
    }
}
