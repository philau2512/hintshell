use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(2);
const MAX_MARKER_DEPTH: usize = 8;
const MARKERS: &[&str] = &[
    ".git",
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "Dockerfile",
    "Makefile",
    "justfile",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceFacts {
    pub root: Option<PathBuf>,
    pub markers: Vec<String>,
}

impl WorkspaceFacts {
    pub fn has(&self, marker: &str) -> bool {
        self.markers.iter().any(|candidate| candidate == marker)
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    facts: WorkspaceFacts,
    expires_at: Instant,
    fingerprint: Vec<Option<SystemTime>>,
}

#[derive(Default)]
pub struct WorkspaceDetector {
    cache: Mutex<HashMap<PathBuf, CacheEntry>>,
    ttl: Duration,
}

impl WorkspaceDetector {
    pub fn new() -> Self {
        Self::with_ttl(DEFAULT_CACHE_TTL)
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub fn detect(&self, cwd: &Path) -> WorkspaceFacts {
        let cwd = normalize_directory(cwd);
        let current_fingerprint = marker_fingerprint(&cwd);
        if let Ok(cache) = self.cache.lock() {
            if let Some(entry) = cache.get(&cwd) {
                if Instant::now() < entry.expires_at && entry.fingerprint == current_fingerprint {
                    return entry.facts.clone();
                }
            }
        }

        let facts = inspect_workspace(&cwd);
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                cwd,
                CacheEntry {
                    facts: facts.clone(),
                    expires_at: Instant::now() + self.ttl,
                    fingerprint: current_fingerprint,
                },
            );
        }
        facts
    }
}

fn inspect_workspace(cwd: &Path) -> WorkspaceFacts {
    let mut current = cwd.to_path_buf();
    for _ in 0..=MAX_MARKER_DEPTH {
        let markers = MARKERS
            .iter()
            .filter(|marker| current.join(marker).exists())
            .map(|marker| (*marker).to_string())
            .collect::<Vec<_>>();
        if !markers.is_empty() {
            return WorkspaceFacts {
                root: Some(current),
                markers,
            };
        }
        if !current.pop() {
            break;
        }
    }
    WorkspaceFacts::default()
}

fn normalize_directory(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn marker_fingerprint(cwd: &Path) -> Vec<Option<SystemTime>> {
    MARKERS
        .iter()
        .map(|marker| {
            std::fs::metadata(cwd.join(marker))
                .and_then(|metadata| metadata.modified())
                .ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_nearest_workspace_marker() {
        let temp = tempdir().unwrap();
        let project = temp.path().join("project");
        let nested = project.join("src").join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(project.join("Cargo.toml"), "[package]").unwrap();

        let facts = WorkspaceDetector::new().detect(&nested);
        assert_eq!(
            facts
                .root
                .as_deref()
                .and_then(|root| root.file_name())
                .and_then(|name| name.to_str()),
            Some("project")
        );
        assert!(facts.has("Cargo.toml"));
    }

    #[test]
    fn invalidates_cached_facts_when_marker_appears() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("isolated");
        std::fs::create_dir(&root).unwrap();
        let detector = WorkspaceDetector::with_ttl(Duration::from_secs(60));
        let before = detector.detect(&root);
        assert_ne!(
            before
                .root
                .as_deref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str()),
            Some("isolated")
        );

        std::fs::write(root.join("package.json"), "{}").unwrap();
        let facts = detector.detect(&root);
        assert_eq!(
            facts
                .root
                .as_deref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str()),
            Some("isolated")
        );
        assert!(facts.has("package.json"));
    }
}
