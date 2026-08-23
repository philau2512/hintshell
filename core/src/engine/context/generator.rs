use std::path::Path;

use super::path::list_path_candidates;
use super::CommandContext;

const MAX_PATH_CANDIDATES: usize = 24;

#[derive(Debug, Clone, PartialEq)]
pub struct ContextCandidate {
    pub command: String,
    pub description: Option<String>,
    pub source: String,
    pub score: f64,
}

pub fn generate_filesystem_candidates(
    context: &CommandContext,
    cwd: &Path,
) -> Vec<ContextCandidate> {
    let Some(command) = context.command.as_deref() else {
        return Vec::new();
    };

    let specification = filesystem_specification(command, &context.arguments);
    let Some(specification) = specification else {
        return Vec::new();
    };

    let values = if context.current_token.starts_with('-') && command != "cd" {
        Vec::new()
    } else {
        list_path_candidates(
            cwd,
            &context.current_token,
            specification.directories_only,
            specification.suffixes,
        )
    };

    let mut candidates: Vec<ContextCandidate> = values
        .into_iter()
        .take(MAX_PATH_CANDIDATES)
        .map(|candidate| ContextCandidate {
            command: format_command(context, &candidate.value),
            description: Some(if candidate.is_directory {
                "directory".to_string()
            } else {
                "file".to_string()
            }),
            source: "path".to_string(),
            score: 12.0,
        })
        .collect();

    // Hỗ trợ bổ sung cho lệnh `cd`: nếu đang gõ cd / cd <token>, gợi ý thêm `cd ..` và `cd -`
    if command == "cd" {
        let token = &context.current_token;
        if token.is_empty() || "..".starts_with(token) {
            candidates.push(ContextCandidate {
                command: "cd ..".to_string(),
                description: Some("parent directory".to_string()),
                source: "path".to_string(),
                score: 11.5,
            });
        }
        if token.is_empty() || "-".starts_with(token) {
            candidates.push(ContextCandidate {
                command: "cd -".to_string(),
                description: Some("previous directory".to_string()),
                source: "path".to_string(),
                score: 11.0,
            });
        }
    }

    candidates
}

struct FilesystemSpecification {
    directories_only: bool,
    suffixes: &'static [&'static str],
}

fn filesystem_specification(
    command: &str,
    arguments: &[String],
) -> Option<FilesystemSpecification> {
    const DIRECTORIES: &[&str] = &["cd", "pushd", "mkdir", "rmdir"];
    const FILES: &[&str] = &["cat", "less", "code", "nvim", "rg"];
    const SOURCE_FILES: &[&str] = &[
        ".rs", ".toml", ".json", ".js", ".ts", ".tsx", ".jsx", ".py", ".go", ".md", ".txt",
    ];

    if DIRECTORIES.contains(&command) {
        return Some(FilesystemSpecification {
            directories_only: true,
            suffixes: &[],
        });
    }
    if FILES.contains(&command) {
        return Some(FilesystemSpecification {
            directories_only: false,
            suffixes: &[],
        });
    }

    match (command, arguments.first().map(String::as_str)) {
        ("git", Some("add")) | ("docker", Some("build")) => Some(FilesystemSpecification {
            directories_only: false,
            suffixes: SOURCE_FILES,
        }),
        _ => None,
    }
}

fn format_command(context: &CommandContext, value: &str) -> String {
    let mut prefix = context.tokens.clone();
    if !context.has_trailing_space {
        prefix.pop();
    }
    prefix.push(value.to_string());
    prefix.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_directory_candidates_for_cd_without_hidden_entries() {
        let temp = tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();

        let candidates =
            generate_filesystem_candidates(&CommandContext::parse("cd s"), temp.path());
        assert_eq!(
            candidates[0],
            ContextCandidate {
                command: "cd src/".to_string(),
                description: Some("directory".to_string()),
                source: "path".to_string(),
                score: 12.0,
            }
        );
    }

    #[test]
    fn creates_special_cd_candidates_for_parent_and_previous() {
        let temp = tempdir().unwrap();
        let candidates =
            generate_filesystem_candidates(&CommandContext::parse("cd "), temp.path());
        assert!(candidates.iter().any(|c| c.command == "cd .." && c.description.as_deref() == Some("parent directory")));
        assert!(candidates.iter().any(|c| c.command == "cd -" && c.description.as_deref() == Some("previous directory")));
    }

    #[test]
    fn creates_file_candidates_for_git_add() {
        let temp = tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.rs"), "").unwrap();

        let candidates =
            generate_filesystem_candidates(&CommandContext::parse("git add src/m"), temp.path());
        assert_eq!(candidates[0].command, "git add src/main.rs");
        assert_eq!(candidates[0].source, "path");
    }

    #[test]
    fn skips_non_file_contexts_and_flag_completion() {
        let temp = tempdir().unwrap();
        assert!(
            generate_filesystem_candidates(&CommandContext::parse("git status"), temp.path())
                .is_empty()
        );
        assert!(
            generate_filesystem_candidates(&CommandContext::parse("cat --ver"), temp.path())
                .is_empty()
        );
    }
}
