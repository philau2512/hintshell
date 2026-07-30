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
    let Some(command) = context.command.as_deref() else {
        return Vec::new();
    };

    match command {
        "git" if workspace.has(".git") => git_candidates(context, cwd, runner),
        "npm" | "pnpm" | "yarn" | "bun" if workspace.has("package.json") => {
            package_script_candidates(context, workspace)
        }
        "docker" if docker_subcommand(context).is_some() => docker_candidates(context, cwd, runner),
        "ssh" | "scp" | "rsync" => ssh_candidates(context),
        "z" | "zoxide" => zoxide_candidates(context, cwd, runner),
        _ => Vec::new(),
    }
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
