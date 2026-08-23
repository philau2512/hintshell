use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

const DEFAULT_MAX_ENTRIES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathCandidate {
    pub value: String,
    pub is_directory: bool,
}

pub fn shell_cwd_to_path(cwd: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let bytes = cwd.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[2] == b'/'
            && bytes[1].is_ascii_alphabetic()
        {
            let drive = (bytes[1] as char).to_ascii_uppercase();
            return PathBuf::from(format!("{drive}:\\{}", cwd[3..].replace('/', "\\")));
        }
    }
    PathBuf::from(cwd)
}

pub fn resolve_path(cwd: &Path, input: &str, home: Option<&Path>) -> Option<PathBuf> {
    let expanded = expand_home(input, home)?;
    let candidate = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };

    Some(normalize_path(candidate))
}

pub fn list_path_candidates(
    cwd: &Path,
    input: &str,
    directories_only: bool,
    allowed_suffixes: &[&str],
) -> Vec<PathCandidate> {
    list_path_candidates_with_limit(
        cwd,
        input,
        directories_only,
        allowed_suffixes,
        DEFAULT_MAX_ENTRIES,
    )
}

pub fn list_path_candidates_with_limit(
    cwd: &Path,
    input: &str,
    directories_only: bool,
    allowed_suffixes: &[&str],
    limit: usize,
) -> Vec<PathCandidate> {
    let home = dirs::home_dir();
    let Some(resolved) = resolve_path(cwd, input, home.as_deref()) else {
        return Vec::new();
    };
    let (directory, prefix) = split_search_path(&resolved, input);
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut entries_with_meta = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || !name.starts_with(&prefix) {
                return None;
            }

            let file_type = entry.file_type().ok()?;
            let is_directory = file_type.is_dir();
            if directories_only && !is_directory {
                return None;
            }
            if !is_directory
                && !allowed_suffixes.is_empty()
                && !allowed_suffixes.iter().any(|suffix| name.ends_with(suffix))
            {
                return None;
            }

            let mut value = display_path(input, &name);
            if is_directory && !value.ends_with('/') {
                value.push('/');
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            Some((
                PathCandidate {
                    value,
                    is_directory,
                },
                modified,
                name,
            ))
        })
        .collect::<Vec<_>>();

    // Sắp xếp: Thư mục lên trước file, trong cùng loại ưu tiên file/folder sửa đổi gần nhất (mtime), rồi theo tên
    entries_with_meta.sort_by(|(left, left_mtime, left_name), (right, right_mtime, right_name)| {
        match (left.is_directory, right.is_directory) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => right_mtime
                .cmp(left_mtime)
                .then_with(|| left_name.cmp(right_name)),
        }
    });

    let mut candidates = entries_with_meta
        .into_iter()
        .map(|(cand, _, _)| cand)
        .collect::<Vec<_>>();
    candidates.truncate(limit);
    candidates
}

fn expand_home(input: &str, home: Option<&Path>) -> Option<PathBuf> {
    if input == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    if input.starts_with('~') {
        return None;
    }
    Some(PathBuf::from(input))
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn split_search_path(resolved: &Path, input: &str) -> (PathBuf, String) {
    if input.is_empty() || input.ends_with('/') || input.ends_with('\\') {
        return (resolved.to_path_buf(), String::new());
    }

    let directory = resolved
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let prefix = resolved
        .file_name()
        .map(|part| part.to_string_lossy().to_string())
        .unwrap_or_default();
    (directory, prefix)
}

fn display_path(input: &str, name: &str) -> String {
    let separator = if input.contains('\\') { '\\' } else { '/' };
    match input.rsplit_once(['/', '\\']) {
        Some((parent, _)) => format!("{parent}{separator}{name}"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    #[cfg(windows)]
    fn converts_git_bash_cwd_to_native_path() {
        assert_eq!(
            shell_cwd_to_path("/c/workspace/hintshell"),
            PathBuf::from("C:\\workspace\\hintshell")
        );
    }

    #[test]
    fn resolves_relative_segments_without_filesystem_access() {
        let root = Path::new("C:/workspace/project/src");
        assert_eq!(
            resolve_path(root, "../tests", None),
            Some(PathBuf::from("C:/workspace/project/tests"))
        );
    }

    #[test]
    fn lists_current_directory_when_input_is_empty() {
        let temp = tempdir().unwrap();
        std::fs::create_dir(temp.path().join("cli")).unwrap();

        assert_eq!(
            list_path_candidates(temp.path(), "", true, &[]),
            vec![PathCandidate {
                value: "cli/".to_string(),
                is_directory: true,
            }]
        );
    }

    #[test]
    fn lists_sorted_visible_directories_before_matching_files() {
        let temp = tempdir().unwrap();
        std::fs::create_dir(temp.path().join("src")).unwrap();
        std::fs::create_dir(temp.path().join(".cache")).unwrap();
        std::fs::write(temp.path().join("sample.rs"), "").unwrap();
        std::fs::write(temp.path().join("sample.txt"), "").unwrap();

        let candidates = list_path_candidates(temp.path(), "s", false, &[".rs"]);
        assert!(candidates.iter().any(|c| c.value == "src/" && c.is_directory));
        assert!(candidates.iter().any(|c| c.value == "sample.rs" && !c.is_directory));
        assert!(!candidates.iter().any(|c| c.value.contains(".cache")));
    }

    #[test]
    fn returns_empty_for_missing_or_unreadable_directory() {
        let temp = tempdir().unwrap();
        assert!(list_path_candidates(temp.path(), "missing/file", false, &[]).is_empty());
    }
}
