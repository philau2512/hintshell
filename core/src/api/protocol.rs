use serde::{Deserialize, Serialize};

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
    },

    /// Record a command that was just executed.
    #[serde(rename = "add")]
    AddCommand {
        command: String,
        directory: Option<String>,
        shell: Option<String>,
    },

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
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionItem {
    pub command: String,
    pub score: f64,
    pub frequency: i64,
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
            error: None,
        }
    }

    pub fn ok_status(status: DaemonStatus) -> Self {
        Self {
            success: true,
            suggestions: None,
            status: Some(status),
            error: None,
        }
    }

    pub fn ok_empty() -> Self {
        Self {
            success: true,
            suggestions: None,
            status: None,
            error: None,
        }
    }

    pub fn err(msg: &str) -> Self {
        Self {
            success: false,
            suggestions: None,
            status: None,
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
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("suggest"));
        assert!(json.contains("git c"));
    }

    #[test]
    fn test_deserialize_suggest_request() {
        let json = r#"{"action":"suggest","input":"git c","limit":5}"#;
        let req: HintShellRequest = serde_json::from_str(json).unwrap();
        match req {
            HintShellRequest::Suggest { input, limit } => {
                assert_eq!(input, "git c");
                assert_eq!(limit, 5);
            }
            _ => panic!("Expected Suggest variant"),
        }
    }

    #[test]
    fn test_serialize_response() {
        let resp = HintShellResponse::ok_suggestions(vec![
            SuggestionItem {
                command: "git commit".to_string(),
                score: 95.0,
                frequency: 10,
            },
        ]);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("git commit"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn test_add_command_request() {
        let json = r#"{"action":"add","command":"git push","directory":"/home/user","shell":"bash"}"#;
        let req: HintShellRequest = serde_json::from_str(json).unwrap();
        match req {
            HintShellRequest::AddCommand { command, directory, shell } => {
                assert_eq!(command, "git push");
                assert_eq!(directory.unwrap(), "/home/user");
                assert_eq!(shell.unwrap(), "bash");
            }
            _ => panic!("Expected AddCommand variant"),
        }
    }
}
