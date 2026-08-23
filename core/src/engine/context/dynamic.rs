use std::path::Path;

use super::generator::ContextCandidate;
use super::process::ProcessRunner;
use super::workspace::WorkspaceFacts;
use super::CommandContext;

pub fn generate_dynamic_candidates(
    context: &CommandContext,
    cwd: &Path,
    workspace: &WorkspaceFacts,
    runner: &ProcessRunner,
) -> Vec<ContextCandidate> {
    let mut candidates = Vec::new();

    // 1. Tech Stack / Project-level smart suggestions
    candidates.extend(project_stack_candidates(context, workspace));

    let Some(command) = context.command.as_deref() else {
        return candidates;
    };

    match command {
        "git" if workspace.has(".git") => {
            candidates.extend(git_candidates(context, cwd, runner));
            candidates.extend(git_state_candidates(context, cwd, workspace));
        }
        "npm" | "pnpm" | "yarn" | "bun" if workspace.has("package.json") => {
            candidates.extend(package_script_candidates(context, workspace));
        }
        "docker" if docker_subcommand(context).is_some() => {
            candidates.extend(docker_candidates(context, cwd, runner));
        }
        "ssh" | "scp" | "rsync" => candidates.extend(ssh_candidates(context)),
        "z" | "zoxide" => candidates.extend(zoxide_candidates(context, cwd, runner)),
        _ => {}
    }

    candidates
}

fn project_stack_candidates(
    context: &CommandContext,
    workspace: &WorkspaceFacts,
) -> Vec<ContextCandidate> {
    let mut list = Vec::new();
    let token = context.input.trim();

    // Package Managers
    if workspace.has("pnpm-lock.yaml") {
        for cmd in &["pnpm dev", "pnpm install", "pnpm build", "pnpm test", "pnpm run"] {
            if token.is_empty() || cmd.starts_with(token) || cmd.contains(token) {
                list.push(ContextCandidate {
                    command: cmd.to_string(),
                    description: Some("pnpm project command".to_string()),
                    source: "pnpm".to_string(),
                    score: 24.0,
                });
            }
        }
    } else if workspace.has("bun.lockb") || workspace.has("bun.lock") {
        for cmd in &["bun run dev", "bun install", "bun test", "bun run build"] {
            if token.is_empty() || cmd.starts_with(token) || cmd.contains(token) {
                list.push(ContextCandidate {
                    command: cmd.to_string(),
                    description: Some("bun project command".to_string()),
                    source: "bun".to_string(),
                    score: 24.0,
                });
            }
        }
    } else if workspace.has("yarn.lock") {
        for cmd in &["yarn dev", "yarn install", "yarn build", "yarn test"] {
            if token.is_empty() || cmd.starts_with(token) || cmd.contains(token) {
                list.push(ContextCandidate {
                    command: cmd.to_string(),
                    description: Some("yarn project command".to_string()),
                    source: "yarn".to_string(),
                    score: 24.0,
                });
            }
        }
    }

    // Cargo / Rust
    if workspace.has("Cargo.toml") {
        for cmd in &["cargo check", "cargo test", "cargo run", "cargo build --release", "cargo clippy"] {
            if token.is_empty() || cmd.starts_with(token) || cmd.contains(token) {
                list.push(ContextCandidate {
                    command: cmd.to_string(),
                    description: Some("cargo project command".to_string()),
                    source: "cargo".to_string(),
                    score: 24.0,
                });
            }
        }
    }

    // Docker compose
    if workspace.has("docker-compose.yml") || workspace.has("docker-compose.yaml") || workspace.has("compose.yml") || workspace.has("compose.yaml") {
        for cmd in &["docker compose up -d", "docker compose down", "docker compose logs -f", "docker compose ps"] {
            if token.is_empty() || cmd.starts_with(token) || cmd.contains(token) {
                list.push(ContextCandidate {
                    command: cmd.to_string(),
                    description: Some("docker compose stack".to_string()),
                    source: "docker".to_string(),
                    score: 23.0,
                });
            }
        }
    }

    list
}

fn git_state_candidates(
    context: &CommandContext,
    cwd: &Path,
    workspace: &WorkspaceFacts,
) -> Vec<ContextCandidate> {
    let mut list = Vec::new();
    let Some(root) = workspace.root.as_deref().or(Some(cwd)) else {
        return list;
    };
    let git_dir = root.join(".git");
    if !git_dir.exists() {
        return list;
    }

    let token = context.input.trim();

    // Check branch name from HEAD
    let mut branch = String::new();
    if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD")) {
        if let Some(name) = head.trim().strip_prefix("ref: refs/heads/") {
            branch = name.to_string();
        }
    }

    // Fast check if working tree or index exists
    let index_file = git_dir.join("index");
    let has_index = index_file.exists();

    if has_index {
        let push_cmd = if !branch.is_empty() {
            format!("git push origin {branch}")
        } else {
            "git push".to_string()
        };

        for (cmd, desc) in &[
            ("git status", "show working tree status"),
            ("git add .", "stage all changes"),
            ("git commit -m \"", "commit staged changes"),
            ("git diff", "view unstaged changes"),
            (&push_cmd, "push branch to remote"),
            ("git pull", "pull latest from remote"),
            ("git stash", "stash dirty working tree"),
            ("git stash pop", "restore stashed changes"),
        ] {
            if token.is_empty() || cmd.starts_with(token) || cmd.contains(token) {
                list.push(ContextCandidate {
                    command: cmd.to_string(),
                    description: Some(desc.to_string()),
                    source: "git".to_string(),
                    score: 25.0,
                });
            }
        }
    }

    list
}

fn git_candidates(
    context: &CommandContext,
    cwd: &Path,
    runner: &ProcessRunner,
) -> Vec<ContextCandidate> {
    let Some(subcommand) = context.arguments.first().map(String::as_str) else {
        return Vec::new();
    };
    let (arguments, source) = match subcommand {
        "switch" | "checkout" | "merge" | "rebase" => {
            (vec!["branch", "--format=%(refname:short)"], "git")
        }
        "push" | "pull" => (vec!["remote"], "git"),
        "tag" => (vec!["tag", "--list"], "git"),
        _ => return Vec::new(),
    };
    let output = runner.run_cached_in_dir(
        format!("git:{subcommand}:{}", cwd.display()),
        Some(cwd),
        "git",
        &arguments,
    );
    lines(&output.stdout)
        .filter(|value| value.starts_with(&context.current_token))
        .map(|value| ContextCandidate {
            command: format_command(context, value),
            description: Some(format!("git {source}")),
            source: "git".to_string(),
            score: 18.0,
        })
        .collect()
}

fn package_script_candidates(
    context: &CommandContext,
    workspace: &WorkspaceFacts,
) -> Vec<ContextCandidate> {
    if !matches!(context.arguments.first().map(String::as_str), Some("run")) {
        return Vec::new();
    }
    let Some(root) = workspace.root.as_deref() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };

    let mut names = scripts.keys().cloned().collect::<Vec<_>>();
    names.sort_by_key(|name| script_priority(name));
    names
        .into_iter()
        .filter(|name| name.starts_with(&context.current_token))
        .map(|name| ContextCandidate {
            command: format_command(context, &name),
            description: Some("package script".to_string()),
            source: "npm".to_string(),
            score: 20.0,
        })
        .collect()
}

fn docker_candidates(
    context: &CommandContext,
    cwd: &Path,
    runner: &ProcessRunner,
) -> Vec<ContextCandidate> {
    let Some(subcommand) = docker_subcommand(context) else {
        return Vec::new();
    };
    let arguments = match subcommand {
        "run" => vec!["image", "ls", "--format", "{{.Repository}}:{{.Tag}}"],
        "exec" | "logs" | "stop" | "rm" => vec!["ps", "-a", "--format", "{{.Names}}"],
        "rmi" => vec!["image", "ls", "--format", "{{.Repository}}:{{.Tag}}"],
        _ => return Vec::new(),
    };
    let output = runner.run_cached_in_dir(
        format!("docker:{subcommand}"),
        Some(cwd),
        "docker",
        &arguments,
    );
    lines(&output.stdout)
        .filter(|value| value.starts_with(&context.current_token))
        .map(|value| ContextCandidate {
            command: format_command(context, value),
            description: Some("docker resource".to_string()),
            source: "docker".to_string(),
            score: 16.0,
        })
        .collect()
}

fn ssh_candidates(context: &CommandContext) -> Vec<ContextCandidate> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(home.join(".ssh").join("config")) else {
        return Vec::new();
    };
    lines(&content)
        .filter_map(|line| line.strip_prefix("Host "))
        .flat_map(str::split_whitespace)
        .filter(|host| !host.contains(['*', '?', '!']))
        .filter(|host| host.starts_with(&context.current_token))
        .map(|host| ContextCandidate {
            command: format_command(context, host),
            description: Some("SSH host".to_string()),
            source: "ssh".to_string(),
            score: 16.0,
        })
        .collect()
}

fn zoxide_candidates(
    context: &CommandContext,
    cwd: &Path,
    runner: &ProcessRunner,
) -> Vec<ContextCandidate> {
    let output = runner.run_cached_in_dir(
        "zoxide:query:list",
        Some(cwd),
        "zoxide",
        &["query", "--list"],
    );
    lines(&output.stdout)
        .filter(|value| value.starts_with(&context.current_token))
        .map(|value| ContextCandidate {
            command: format_command(context, value),
            description: Some("zoxide directory".to_string()),
            source: "zoxide".to_string(),
            score: 14.0,
        })
        .collect()
}

fn docker_subcommand(context: &CommandContext) -> Option<&str> {
    context.arguments.first().map(String::as_str)
}

fn lines(value: &str) -> impl Iterator<Item = &str> {
    value.lines().map(str::trim).filter(|line| !line.is_empty())
}

fn script_priority(name: &str) -> usize {
    ["dev", "start", "build", "test", "lint"]
        .iter()
        .position(|preferred| *preferred == name)
        .unwrap_or(usize::MAX)
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
    fn parses_and_prioritizes_package_scripts() {
        let temp = tempdir().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"scripts":{"custom":"x","test":"x","dev":"x"}}"#,
        )
        .unwrap();
        let facts = WorkspaceFacts {
            root: Some(temp.path().to_path_buf()),
            markers: vec!["package.json".to_string()],
        };
        let candidates = package_script_candidates(&CommandContext::parse("npm run "), &facts);
        assert_eq!(candidates[0].command, "npm run dev");
        assert_eq!(candidates[1].command, "npm run test");
    }

    #[test]
    fn ignores_ssh_wildcard_hosts() {
        let content = "Host build *.internal\nHost deploy";
        let hosts = lines(content)
            .filter_map(|line| line.strip_prefix("Host "))
            .flat_map(str::split_whitespace)
            .filter(|host| !host.contains(['*', '?', '!']))
            .collect::<Vec<_>>();
        assert_eq!(hosts, vec!["build", "deploy"]);
    }
}
