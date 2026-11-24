use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Border colour names the overlay understands; anything else silently falls back to the
/// default, so `hs config` reports it instead.
pub const KNOWN_BORDER_COLORS: &[&str] = &[
    "apple",
    "siri",
    "apple-intelligence",
    "glow",
    "neon",
    "rainbow",
    "gemini",
    "gradient",
    "aurora",
    "purple",
    "blue",
    "cyan",
    "green",
    "yellow",
    "gold",
    "orange",
    "pink",
    "rose",
    "magenta",
    "minimal",
    "gray",
    "grey",
    "red",
];

/// Range the overlay actually honours (it clamps outside of it).
pub const MAX_VISIBLE_RANGE: std::ops::RangeInclusive<usize> = 3..=10;

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

/// Diagnostic commands that print the warnings themselves set this, so one run does not
/// report the same problem twice (stderr from the loader, then stdout from the report).
static CONFIG_WARNINGS_SUPPRESSED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn suppress_startup_warnings() {
    CONFIG_WARNINGS_SUPPRESSED.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn startup_warnings_suppressed() -> bool {
    CONFIG_WARNINGS_SUPPRESSED.load(std::sync::atomic::Ordering::Relaxed)
}

impl HintShellConfig {
    pub fn config_path() -> PathBuf {
        crate::shell::hintshell_home().join("config.toml")
    }

    /// Cached: the overlay renders on every keystroke and must not hit the disk each time.
    pub fn load() -> Self {
        static CACHED: std::sync::OnceLock<HintShellConfig> = std::sync::OnceLock::new();
        CACHED.get_or_init(Self::read).clone()
    }

    fn read() -> Self {
        let path = Self::config_path();
        if let Ok(content) = fs::read_to_string(&path) {
            match toml::from_str::<HintShellConfig>(&content) {
                Ok(config) => {
                    if !startup_warnings_suppressed() {
                        for warning in config.warnings() {
                            eprintln!("HintShell config: {warning}");
                        }
                    }
                    return config;
                }
                Err(error) => {
                    eprintln!(
                        "HintShell config: {} is invalid ({error}); using defaults.",
                        path.display()
                    );
                    return Self::default();
                }
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

    /// Tell the user what will not behave the way they wrote it, instead of silently
    /// substituting a different value.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        let border = self.border_color.to_lowercase();
        if !KNOWN_BORDER_COLORS.contains(&border.as_str()) {
            warnings.push(format!(
                "unknown border_color {:?}; using the default purple frame",
                self.border_color
            ));
        }
        if !MAX_VISIBLE_RANGE.contains(&self.max_visible) {
            warnings.push(format!(
                "max_visible {} is outside {}..={}; the overlay shows {} rows",
                self.max_visible,
                MAX_VISIBLE_RANGE.start(),
                MAX_VISIBLE_RANGE.end(),
                self.max_visible.clamp(
                    *MAX_VISIBLE_RANGE.start(),
                    *MAX_VISIBLE_RANGE.end()
                )
            ));
        }
        warnings
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
            // Gradient presets paint with 24-bit escapes; the frame colour here is only the
            // fallback used when a terminal cannot show them (see `supports_truecolor`).
            "apple" | "siri" | "apple-intelligence" | "glow" | "neon" => "\x1b[38;5;51m", // Neon cyan fallback
            "rainbow" | "gemini" | "gradient" | "aurora" => "\x1b[38;5;141m",
            _ => "\x1b[38;5;141m",              // Purple default
        }
    }
}

/// Whether the terminal advertises 24-bit colour. Gradient presets must degrade to the
/// closest 256-colour code otherwise Conhost and `TERM=xterm-256color` show a broken frame.
pub fn supports_truecolor() -> bool {
    static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("COLORTERM")
            .map(|value| {
                let value = value.to_lowercase();
                value.contains("truecolor") || value.contains("24bit")
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truecolor_degrades_for_non_gradient_palettes() {
        // Every plain palette name already uses a 256-colour escape, so it cannot break.
        for name in ["purple", "blue", "red", "minimal", "apple", "rainbow"] {
            let config = HintShellConfig {
                border_color: name.to_string(),
                ..Default::default()
            };
            assert!(
                config.border_ansi_code().contains("38;5;"),
                "{name} must not require 24-bit colour for the frame"
            );
        }
    }

    #[test]
    fn every_documented_palette_name_resolves_to_a_code() {
        for name in KNOWN_BORDER_COLORS {
            let config = HintShellConfig {
                border_color: name.to_string(),
                ..Default::default()
            };
            assert!(
                config.border_ansi_code().starts_with('\x1b'),
                "palette entry {name} produced no escape code"
            );
            let warnings = config.warnings();
            assert!(warnings.is_empty(), "palette entry {name} warned: {warnings:?}");
        }
    }

    #[test]
    fn unknown_values_are_reported_instead_of_swallowed() {
        let config = HintShellConfig {
            border_color: "hotdog".to_string(),
            max_visible: 99,
            ghost_text: true,
        };
        let warnings = config.warnings();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("hotdog"));
        assert!(warnings[1].contains("99"));

        let gradient = HintShellConfig {
            border_color: "Apple-Intelligence".to_string(),
            ..Default::default()
        };
        assert!(gradient.is_apple());
        assert!(gradient.warnings().is_empty(), "aliases must be accepted");
    }
}
