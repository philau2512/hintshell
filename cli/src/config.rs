use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HintShellConfig {
    #[serde(default = "default_border_color")]
    pub border_color: String,

    #[serde(default = "default_max_visible")]
    pub max_visible: usize,

    #[serde(default = "default_ghost_text")]
    pub ghost_text: bool,
}

fn default_border_color() -> String {
    "purple".to_string()
}

fn default_max_visible() -> usize {
    6
}

fn default_ghost_text() -> bool {
    true
}

impl Default for HintShellConfig {
    fn default() -> Self {
        Self {
            border_color: default_border_color(),
            max_visible: default_max_visible(),
            ghost_text: default_ghost_text(),
        }
    }
}

impl HintShellConfig {
    pub fn config_path() -> PathBuf {
        crate::shell::hintshell_home().join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(config) = toml::from_str::<HintShellConfig>(&content) {
                return config;
            }
        }
        // Save default config if not existing
        let default_config = Self::default();
        if !path.exists() {
            if let Ok(content) = toml::to_string_pretty(&default_config) {
                let _ = fs::write(path, content);
            }
        }
        default_config
    }

    pub fn is_rainbow(&self) -> bool {
        matches!(
            self.border_color.to_lowercase().as_str(),
            "rainbow" | "gemini" | "gradient" | "aurora"
        )
    }

    pub fn is_apple(&self) -> bool {
        matches!(
            self.border_color.to_lowercase().as_str(),
            "apple" | "siri" | "apple-intelligence" | "glow" | "neon"
        )
    }

    pub fn border_ansi_code(&self) -> &'static str {
        match self.border_color.to_lowercase().as_str() {
            "blue" => "\x1b[38;5;75m",          // Deep sky / vibrant blue
            "cyan" => "\x1b[38;5;51m",          // Bright cyan / aqua
            "green" => "\x1b[38;5;48m",         // Neon green
            "yellow" | "gold" => "\x1b[38;5;220m", // Bright yellow
            "orange" => "\x1b[38;5;208m",       // Warm vibrant orange
            "pink" | "rose" => "\x1b[38;5;212m",// Cute bright pastel pink
            "magenta" => "\x1b[38;5;201m",      // Cyberpunk magenta / hot pink
            "minimal" | "gray" | "grey" => "\x1b[38;5;244m",
            "red" => "\x1b[38;5;203m",
            "apple" | "siri" | "apple-intelligence" | "glow" | "neon" => "\x1b[38;2;0;245;212m", // Apple Intelligence Neon Cyan
            "rainbow" | "gemini" | "gradient" | "aurora" => "\x1b[38;5;141m",
            _ => "\x1b[38;5;141m",              // Purple default
        }
    }
}
