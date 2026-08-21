mod bash;
mod powershell;
mod zsh;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

// Defaults to ~/.hintshell/. HINTSHELL_HOME enables isolated test installs.
pub fn hintshell_home() -> PathBuf {
    if let Some(home) = env::var_os("HINTSHELL_HOME") {
        return PathBuf::from(home);
    }

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hintshell")
}

fn shell_config_home() -> PathBuf {
    env::var_os("HINTSHELL_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Convert a Windows path to a Git Bash / MSYS path (`C:\Users\x` → `/c/Users/x`).
/// On Unix, just normalizes separators.
pub fn to_posix_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        format!("/{}{}", drive, &s[2..])
    } else {
        s
    }
}

/// Absolute path to the CLI binary for shell hooks (`.exe` on Windows).
pub fn cli_bin_posix() -> String {
    let name = if cfg!(windows) {
        "hintshell.exe"
    } else {
        "hintshell"
    };
    to_posix_path(&hintshell_home().join("bin").join(name))
}

/// Absolute path to the daemon binary for shell hooks.
pub fn core_bin_posix() -> String {
    let name = if cfg!(windows) {
        "hintshell-core.exe"
    } else {
        "hintshell-core"
    };
    to_posix_path(&hintshell_home().join("bin").join(name))
}

/// Bin directory in POSIX form for PATH export.
pub fn bin_dir_posix() -> String {
    to_posix_path(&hintshell_home().join("bin"))
}

/// Quote a value as one literal Bash word.
pub fn bash_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

const GIT_BASH_FAST_RCFILE_NAME: &str = "git-bash-fast.bashrc";
const GIT_BASH_FAST_HOOK_NAME: &str = "git-bash-fast-hook.bash";

/// Path to the HintShell-managed startup file for a fast Git Bash profile.
pub fn git_bash_fast_rcfile_path() -> PathBuf {
    hintshell_home().join(GIT_BASH_FAST_RCFILE_NAME)
}

/// Path to the static Bash integration sourced by the fast Git Bash rcfile.
fn git_bash_fast_hook_path() -> PathBuf {
    hintshell_home().join(GIT_BASH_FAST_HOOK_NAME)
}

fn git_bash_fast_rcfile_content(rcfile: &Path) -> String {
    let bin_dir = bin_dir_posix();
    let cli = cli_bin_posix();
    let rcfile = bash_single_quote(&to_posix_path(rcfile));
    let hook = bash_single_quote(&to_posix_path(&git_bash_fast_hook_path()));

    format!(
        r#"# HintShell Git Bash Fast initialization
# This managed file intentionally does not source user Bash startup files.
PS1='\[\e[32m\]\u@\h \[\e[33m\]\w\[\e[0m\]\n$ '
export PATH="{bin_dir}:$PATH"
if [[ -n "${{HINTSHELL_LIVE_BASH:-}}" ]]; then
  _hintshell_trace_phase() {{
    [[ "${{HINTSHELL_TRACE_STARTUP:-}}" == "1" ]] || return
    printf '$1'
  }}
  _hintshell_trace_phase 'entered live branch'
  _hintshell_emit_prompt() {{ :; }}
  _hintshell_live_record() {{
    ( local last_cmd
      last_cmd=$(HISTTIMEFORMAT="" history 1 | sed 's/^[ ]*[0-9]*[ ]*//')
      [[ -n "$last_cmd" ]] && "{cli}" add --command "$last_cmd" --directory "$PWD" --shell bash
    ) > /dev/null 2>&1 & disown
  }}
  _hintshell_trace_phase 'configured prompt command'
  PROMPT_COMMAND="_hintshell_emit_prompt;_hintshell_live_record${{PROMPT_COMMAND:+;$PROMPT_COMMAND}}"
elif [ -r {hook} ]; then
  source {hook}
fi
# Run the ConPTY live overlay once; the child reloads this same minimal rcfile.
if [[ -n "${{MSYSTEM:-}}" && $- == *i* && -t 0 && -t 1 && -z "${{HINTSHELL_LIVE_BASH:-}}" && -z "${{HINTSHELL_DISABLE_AUTO_BASH:-}}" ]]; then
  exec "{cli}" bash --rcfile {rcfile} -i
fi
"#,
        bin_dir = bin_dir,
        cli = cli,
        hook = hook,
        rcfile = rcfile,
    )
}

fn write_git_bash_fast_rcfile_at(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Could not find Git Bash Fast rcfile parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(path, git_bash_fast_rcfile_content(path)).map_err(|error| error.to_string())
}

fn write_git_bash_fast_hook_at(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Could not find Git Bash Fast hook parent directory")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    fs::write(path, bash::hook_script()).map_err(|error| error.to_string())
}

/// Create or replace the managed rcfile used by the opt-in Git Bash Fast profile.
pub fn install_git_bash_fast_rcfile() -> Result<PathBuf, String> {
    let path = git_bash_fast_rcfile_path();
    write_git_bash_fast_hook_at(&git_bash_fast_hook_path())?;
    write_git_bash_fast_rcfile_at(&path)?;
    Ok(path)
}

fn remove_git_bash_fast_rcfile_at(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Remove only the managed Git Bash Fast rcfile. Missing files are ignored.
pub fn uninstall_git_bash_fast_rcfile() -> Result<(), String> {
    remove_git_bash_fast_rcfile_at(&git_bash_fast_rcfile_path())?;
    remove_git_bash_fast_rcfile_at(&git_bash_fast_hook_path())
}

/// Shared fzf resolver for Bash/Zsh.
/// Git Bash cannot exec WinGet "Links" reparse-point shims (Permission denied);
/// the real binary under WinGet/Packages works when called by absolute path.
pub fn fzf_resolve_functions() -> String {
    r#"
# Resolve a working fzf binary (caches in _HINTSHELL_FZF_BIN).
# Never exec WinGet/Links shims — they print "Permission denied" under Git Bash.
_hintshell_try_fzf() {
    local p="$1"
    [[ -z "$p" ]] && return 1
    # command -v on MSYS often omits .exe
    if [[ ! -e "$p" && -e "${p}.exe" ]]; then
        p="${p}.exe"
    fi
    # WinGet Links: resolve symlink only, never execute the shim path
    case "$p" in
        */WinGet/Links/*|*/Microsoft/WinGet/Links/*)
            local real=""
            if [[ -L "$p" ]]; then
                real=$(readlink -f "$p" 2>/dev/null || readlink "$p" 2>/dev/null || true)
            elif [[ -L "${p}.exe" ]]; then
                real=$(readlink -f "${p}.exe" 2>/dev/null || readlink "${p}.exe" 2>/dev/null || true)
            fi
            [[ -n "$real" ]] || return 1
            p="$real"
            ;;
    esac
    if [[ -L "$p" ]]; then
        local real2
        real2=$(readlink -f "$p" 2>/dev/null || readlink "$p" 2>/dev/null || true)
        if [[ -n "$real2" ]]; then
            if [[ "$real2" != /* && "$real2" != [A-Za-z]:* ]]; then
                real2="$(cd "$(dirname "$p")" 2>/dev/null && pwd)/$real2"
            fi
            if [[ -e "$real2" ]]; then
                p="$real2"
            elif [[ -e "${real2}.exe" ]]; then
                p="${real2}.exe"
            fi
        fi
    fi
    # Still a Links path after resolve? refuse
    case "$p" in
        */WinGet/Links/*|*/Microsoft/WinGet/Links/*) return 1 ;;
    esac
    [[ -f "$p" ]] || return 1
    if ! "$p" --version >/dev/null 2>&1; then
        return 1
    fi
    _HINTSHELL_FZF_BIN="$p"
    printf '%s\n' "$p"
    return 0
}

_hintshell_resolve_fzf() {
    if [[ -n "${_HINTSHELL_FZF_BIN:-}" ]]; then
        printf '%s\n' "$_HINTSHELL_FZF_BIN"
        return 0
    fi
    local p uname_s
    if [[ -n "${HINTSHELL_FZF:-}" ]]; then
        _hintshell_try_fzf "$HINTSHELL_FZF" && return 0
    fi
    # On Git Bash / MSYS: prefer real WinGet package binary FIRST
    # (PATH often points at Links shim which is not executable)
    uname_s=$(uname -s 2>/dev/null || true)
    case "$uname_s" in
        MINGW*|MSYS*|CYGWIN*)
            for p in \
                /c/Users/*/AppData/Local/Microsoft/WinGet/Packages/junegunn.fzf_*/fzf.exe \
                /d/Users/*/AppData/Local/Microsoft/WinGet/Packages/junegunn.fzf_*/fzf.exe
            do
                [[ -f "$p" ]] || continue
                _hintshell_try_fzf "$p" && return 0
            done
            ;;
    esac
    for p in fzf.exe fzf; do
        if command -v "$p" >/dev/null 2>&1; then
            _hintshell_try_fzf "$(command -v "$p" 2>/dev/null)" && return 0
        fi
    done
    return 1
}
"#
    .to_string()
}

/// fzf UI flags shared by bash/zsh pickers
pub fn fzf_picker_args() -> &'static str {
    "--height 40% --reverse --no-sort --cycle --delimiter='\\t' --with-nth=1,2 --nth=1 --tabstop=4 --prompt='HintShell> ' --header='Tab/Enter: select'"
}

pub enum Shell {
    Power,
    Bash,
    Zsh,
}

impl Shell {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "powershell" | "pwsh" => Some(Self::Power),
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Power => "PowerShell",
            Self::Bash => "Bash",
            Self::Zsh => "Zsh",
        }
    }

    /// Path to the shell's rc/profile config file
    pub fn config_path(&self) -> Option<PathBuf> {
        match self {
            Self::Power => {
                #[cfg(windows)]
                {
                    let documents = env::var_os("HINTSHELL_CONFIG_HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| {
                            dirs::document_dir()
                                .unwrap_or_else(|| shell_config_home().join("Documents"))
                        });
                    Some(documents.join("PowerShell\\Microsoft.PowerShell_profile.ps1"))
                }
                #[cfg(unix)]
                {
                    Some(
                        shell_config_home()
                            .join(".config/powershell/Microsoft.PowerShell_profile.ps1"),
                    )
                }
            }
            Self::Bash => Some(shell_config_home().join(".bashrc")),
            Self::Zsh => Some(shell_config_home().join(".zshrc")),
        }
    }

    /// Output the hook script to be eval'd by the shell
    pub fn get_hook(&self) -> String {
        let module_dir = hintshell_home().join("module");
        let module_dir_str = module_dir.to_string_lossy().replace('\\', "/");
        let bin_dir = hintshell_home().join("bin");
        let daemon_name = if cfg!(windows) {
            "hintshell-core.exe"
        } else {
            "hintshell-core"
        };
        let daemon_path = bin_dir.join(daemon_name);
        let daemon_str = daemon_path.to_string_lossy().replace('\\', "/");

        match self {
            Self::Power => powershell::hook_script(&daemon_str, &module_dir_str),
            Self::Bash => bash::hook_script(),
            Self::Zsh => zsh::hook_script(),
        }
    }

    /// Install hook line into shell config file
    pub fn install(&self, _bin_path: &std::path::Path) -> Result<(), String> {
        let config = self.config_path().ok_or("Could not find config path")?;

        let module_dir = hintshell_home().join("module");
        let module_str = module_dir.to_string_lossy().replace('\\', "/");

        let init_line = match self {
            Self::Power => powershell::install_line(&module_str),
            Self::Bash => bash::install_line(),
            Self::Zsh => zsh::install_line(),
        };

        // Create parent dirs if needed
        if let Some(parent) = config.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let content = if config.exists() {
            fs::read_to_string(&config).map_err(|e| e.to_string())?
        } else {
            String::new()
        };

        let new_content = replace_init_block(&content, &init_line);
        fs::write(&config, new_content).map_err(|e| e.to_string())?;
        if matches!(self, Self::Bash) {
            install_macos_bash_login_source()?;
        }
        Ok(())
    }

    /// Uninstall hook line from shell config file
    pub fn uninstall(&self) -> Result<(), String> {
        let config = self.config_path().ok_or("Could not find config path")?;
        if !config.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&config).map_err(|e| e.to_string())?;

        let marker = "# HintShell Initialization";
        let end_marker = "# End HintShell";
        if let Some(start_idx) = content.find(marker) {
            // Start of the line containing the marker
            let mut start_of_block = start_idx;
            while start_of_block > 0 && content.as_bytes()[start_of_block - 1] != b'\n' {
                start_of_block -= 1;
            }

            // Prefer explicit end marker; else fall back to ~12 lines (new init block is longer)
            let end_of_block = if let Some(rel) = content[start_idx..].find(end_marker) {
                let mut end = start_idx + rel + end_marker.len();
                if end < content.len() && content.as_bytes()[end] == b'\n' {
                    end += 1;
                }
                end
            } else {
                let mut end = start_idx;
                let mut lines_count = 0;
                while end < content.len() && lines_count < 12 {
                    if content.as_bytes()[end] == b'\n' {
                        lines_count += 1;
                    }
                    end += 1;
                }
                end
            };

            let mut new_content = content;
            new_content.replace_range(start_of_block..end_of_block, "");
            fs::write(&config, new_content).map_err(|e| e.to_string())?;
            if matches!(self, Self::Bash) {
                uninstall_macos_bash_login_source()?;
            }
            Ok(())
        } else {
            Ok(()) // Already uninstalled or not found
        }
    }
}

fn replace_init_block(content: &str, init_line: &str) -> String {
    replace_managed_block(
        content,
        "# HintShell Initialization",
        "# End HintShell",
        init_line,
    )
}

fn replace_managed_block(content: &str, marker: &str, end_marker: &str, block: &str) -> String {
    let Some(start) = content.find(marker) else {
        return format!("{content}{block}");
    };
    let line_start = content[..start].rfind('\n').map_or(0, |index| index + 1);
    let end = content[start..]
        .find(end_marker)
        .map(|offset| {
            let end = start + offset + end_marker.len();
            if content.as_bytes().get(end) == Some(&b'\n') {
                end + 1
            } else {
                end
            }
        })
        .unwrap_or(content.len());

    format!("{}{}{}", &content[..line_start], block, &content[end..])
}

fn remove_managed_block(content: &str, marker: &str, end_marker: &str) -> String {
    let Some(start) = content.find(marker) else {
        return content.to_string();
    };
    let line_start = content[..start].rfind('\n').map_or(0, |index| index + 1);
    let end = content[start..]
        .find(end_marker)
        .map(|offset| {
            let end = start + offset + end_marker.len();
            if content.as_bytes().get(end) == Some(&b'\n') {
                end + 1
            } else {
                end
            }
        })
        .unwrap_or(content.len());

    format!("{}{}", &content[..line_start], &content[end..])
}

const BASH_LOGIN_MARKER: &str = "# HintShell Bash Login Initialization";
const BASH_LOGIN_END_MARKER: &str = "# End HintShell Bash Login";

fn macos_bash_login_source_block() -> &'static str {
    r#"
# HintShell Bash Login Initialization
# macOS starts Bash as a login shell, which does not read ~/.bashrc by default.
if [ -f "$HOME/.bashrc" ]; then
  . "$HOME/.bashrc"
fi
# End HintShell Bash Login
"#
}

fn install_macos_bash_login_source() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }

    let profile = dirs::home_dir()
        .ok_or("Could not find home directory")?
        .join(".bash_profile");
    let content = if profile.exists() {
        fs::read_to_string(&profile).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    let legacy_block = r#"
# Load interactive settings, including HintShell's macOS live overlay.
if [ -f "$HOME/.bashrc" ]; then
  . "$HOME/.bashrc"
fi
"#;
    let content = content.replace(legacy_block, "");
    let updated = replace_managed_block(
        &content,
        BASH_LOGIN_MARKER,
        BASH_LOGIN_END_MARKER,
        macos_bash_login_source_block(),
    );
    fs::write(profile, updated).map_err(|e| e.to_string())
}

fn uninstall_macos_bash_login_source() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }

    let profile = dirs::home_dir()
        .ok_or("Could not find home directory")?
        .join(".bash_profile");
    if !profile.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(&profile).map_err(|e| e.to_string())?;
    let updated = remove_managed_block(&content, BASH_LOGIN_MARKER, BASH_LOGIN_END_MARKER);
    fs::write(profile, updated).map_err(|e| e.to_string())
}

pub fn uninstall_assets() -> Result<(), String> {
    let home = hintshell_home();
    let bin_dir = home.join("bin");
    let module_dir = home.join("module");

    if module_dir.exists() {
        fs::remove_dir_all(&module_dir).map_err(|e| format!("Remove module: {}", e))?;
    }

    if bin_dir.exists() {
        if let Err(_e) = fs::remove_dir_all(&bin_dir) {
            #[cfg(windows)]
            {
                // Self-deleting trick on Windows
                let path_str = bin_dir.to_string_lossy().to_string();
                let cmd_str = format!("ping 127.0.0.1 -n 2 > nul & rmdir /s /q \"{}\"", path_str);

                let _ = std::process::Command::new("cmd")
                    .arg("/c")
                    .arg(&cmd_str)
                    .spawn();

                println!("⏳ Scheduled binary removal after termination...");
            }
            #[cfg(unix)]
            {
                return Err(format!("Remove bin: {}", _e));
            }
        }
    }

    Ok(())
}

/// Copy binaries and modules into ~/.hintshell/
pub fn install_assets(bin_path: &std::path::Path) -> Result<(), String> {
    let home = hintshell_home();
    let bin_dir = home.join("bin");
    let module_dir = home.join("module");

    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&module_dir).map_err(|e| e.to_string())?;

    let mut warnings: Vec<String> = Vec::new();

    // 1. Copy hintshell binary itself (skip if we ARE the installed binary — Windows locks running .exe)
    let hs_name = if cfg!(windows) {
        "hintshell.exe"
    } else {
        "hintshell"
    };
    let dest_hs = bin_dir.join(hs_name);
    match copy_replace(bin_path, &dest_hs) {
        Ok(CopyResult::Copied) => {}
        Ok(CopyResult::SkippedSame) => {
            // Running from ~/.hintshell/bin/hintshell.exe — cannot self-overwrite; OK
        }
        Err(e) => warnings.push(format!("hintshell: {}", e)),
    }
    #[cfg(unix)]
    let _ = set_executable(&dest_hs);

    // 1b. Copy hs shorthand alias
    let hs_short = if cfg!(windows) { "hs.exe" } else { "hs" };
    if let Some(parent) = bin_path.parent() {
        let hs_src = parent.join(hs_short);
        if hs_src.exists() {
            let dest_short = bin_dir.join(hs_short);
            match copy_replace(&hs_src, &dest_short) {
                Ok(_) => {}
                Err(e) => warnings.push(format!("hs: {}", e)),
            }
            #[cfg(unix)]
            let _ = set_executable(&dest_short);
        }
    }

    // 2. Copy hintshell-core daemon (sibling of hintshell binary)
    let core_name = if cfg!(windows) {
        "hintshell-core.exe"
    } else {
        "hintshell-core"
    };
    if let Some(parent) = bin_path.parent() {
        let core_src = parent.join(core_name);
        if core_src.exists() {
            let dest_core = bin_dir.join(core_name);
            match copy_replace(&core_src, &dest_core) {
                Ok(CopyResult::Copied) => {}
                Ok(CopyResult::SkippedSame) => {}
                Err(e) => {
                    return Err(format!(
                        "Copy core failed: {} (stop daemon first: hs stop)",
                        e
                    ));
                }
            }
            #[cfg(unix)]
            let _ = set_executable(&dest_core);
        }
    }

    // 3. Find PowerShell module
    //    Priority A: 'module/' directory adjacent to binary (distributed build)
    //    Priority B: Walk up from binary to find integrations/powershell/HintShellModule (dev build)
    let module_src = find_module_src(bin_path);

    match module_src {
        Some(src) => {
            if same_path(&src, &module_dir) {
                // Re-init from ~/.hintshell/bin — module already in place
            } else {
                copy_dir_all(&src, &module_dir)
                    .map_err(|e| format!("Copy module failed: {}", e))?;
            }

            // Also copy hintshell-core into module/ so $PSScriptRoot finds it
            if let Some(parent) = bin_path.parent() {
                let core_src = parent.join(core_name);
                if core_src.exists() {
                    let dest_module_core = module_dir.join(core_name);
                    if let Err(e) = copy_replace(&core_src, &dest_module_core) {
                        warnings.push(format!("module core: {}", e));
                    }
                    #[cfg(unix)]
                    let _ = set_executable(&dest_module_core);
                }
            }

            // Also copy default-commands.json into ~/.hintshell/ for runtime loading
            let defaults_src = src.join("default-commands.json");
            if defaults_src.exists() {
                if let Err(e) = fs::copy(&defaults_src, home.join("default-commands.json")) {
                    warnings.push(format!("default-commands.json: {}", e));
                }
            }
        }
        None => {
            return Err(
                "Could not find HintShellModule. Make sure you built the project correctly."
                    .to_string(),
            );
        }
    }

    if !warnings.is_empty() {
        // Non-fatal: CLI self-replace often fails on Windows when running from ~/.hintshell/bin
        eprintln!("   ⚠️ Partial asset copy warnings: {}", warnings.join("; "));
    }

    Ok(())
}

enum CopyResult {
    Copied,
    SkippedSame,
}

fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// Replace dest with src. Skips when src and dest are the same file (running self on Windows).
fn copy_replace(src: &std::path::Path, dest: &std::path::Path) -> Result<CopyResult, String> {
    if dest.exists() && same_path(src, dest) {
        return Ok(CopyResult::SkippedSame);
    }
    // Best-effort remove first (helps Unix text-busy; on Windows locked files fail remove)
    let _ = fs::remove_file(dest);
    match fs::copy(src, dest) {
        Ok(_) => Ok(CopyResult::Copied),
        Err(e) => {
            // Windows: sometimes can write to a new name then replace — try temp swap
            #[cfg(windows)]
            {
                let tmp = dest.with_extension("exe.new");
                let _ = fs::remove_file(&tmp);
                if fs::copy(src, &tmp).is_ok() {
                    // If dest is locked, rename won't work either — but try
                    let _ = fs::remove_file(dest);
                    if fs::rename(&tmp, dest).is_ok() {
                        return Ok(CopyResult::Copied);
                    }
                    let _ = fs::remove_file(&tmp);
                }
            }
            Err(e.to_string())
        }
    }
}

fn find_module_src(bin_path: &std::path::Path) -> Option<std::path::PathBuf> {
    // Priority A: adjacent 'module/' dir (npm vendor / release zip layout)
    if let Some(parent) = bin_path.parent() {
        let adjacent = parent.join("module");
        if adjacent.join("HintShellModule.psd1").exists() {
            return Some(adjacent);
        }
    }

    // Priority B: walk up dirs to find 'integrations/powershell/HintShellModule' (dev mode)
    let mut dir = bin_path.parent()?.to_path_buf();
    for _ in 0..6 {
        let candidate = dir.join("integrations/powershell/HintShellModule");
        if candidate.join("HintShellModule.psd1").exists() {
            return Some(candidate);
        }
        dir = dir.parent()?.to_path_buf();
    }

    // Priority C: already-installed module (re-init from HINTSHELL_HOME or ~/.hintshell)
    let installed = hintshell_home().join("module");
    if installed.join("HintShellModule.psd1").exists() {
        return Some(installed);
    }

    None
}

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.join(entry.file_name())).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn detect_shells() -> Vec<Shell> {
    let mut shells = Vec::new();
    if is_command_available("pwsh") || is_command_available("powershell") {
        shells.push(Shell::Power);
    }
    if is_command_available("bash") {
        shells.push(Shell::Bash);
    }
    if is_command_available("zsh") {
        shells.push(Shell::Zsh);
    }
    shells
}

fn is_command_available(cmd: &str) -> bool {
    let cmd = if cfg!(windows) {
        format!("{}.exe", cmd)
    } else {
        cmd.to_string()
    };
    env::var_os("PATH").is_some_and(|paths| env::split_paths(&paths).any(|p| p.join(&cmd).exists()))
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_replaces_existing_initialization_block() {
        let content = "before\n# HintShell Initialization\nold\n# End HintShell\nafter\n";
        let updated = replace_init_block(
            content,
            "# HintShell Initialization\nnew\n# End HintShell\n",
        );
        assert_eq!(
            updated,
            "before\n# HintShell Initialization\nnew\n# End HintShell\nafter\n"
        );
    }

    #[test]
    fn replace_managed_block_preserves_surrounding_content() {
        let updated =
            replace_managed_block("before\n", "# Start", "# End", "# Start\nnew\n# End\n");
        assert_eq!(updated, "before\n# Start\nnew\n# End\n");

        let replaced = replace_managed_block(
            "before\n# Start\nold\n# End\nafter\n",
            "# Start",
            "# End",
            "# Start\nnew\n# End\n",
        );
        assert_eq!(replaced, "before\n# Start\nnew\n# End\nafter\n");
    }

    #[test]
    fn remove_managed_block_preserves_surrounding_content() {
        let updated = remove_managed_block(
            "before\n# Start\nmanaged\n# End\nafter\n",
            "# Start",
            "# End",
        );
        assert_eq!(updated, "before\nafter\n");
    }

    #[test]
    fn hintshell_home_can_be_overridden_for_isolated_installation() {
        let original = env::var_os("HINTSHELL_HOME");
        let isolated_home = std::env::temp_dir().join("hintshell-isolated-test");
        env::set_var("HINTSHELL_HOME", &isolated_home);
        assert_eq!(hintshell_home(), isolated_home);
        if let Some(original) = original {
            env::set_var("HINTSHELL_HOME", original);
        } else {
            env::remove_var("HINTSHELL_HOME");
        }
    }

    #[test]
    fn shell_config_home_can_be_overridden_for_isolated_installation() {
        let original = env::var_os("HINTSHELL_CONFIG_HOME");
        let config_home = std::env::temp_dir().join("hintshell-config-test");
        env::set_var("HINTSHELL_CONFIG_HOME", &config_home);
        assert_eq!(shell_config_home(), config_home);
        if let Some(original) = original {
            env::set_var("HINTSHELL_CONFIG_HOME", original);
        } else {
            env::remove_var("HINTSHELL_CONFIG_HOME");
        }
    }

    #[test]
    fn git_bash_fast_rcfile_is_minimal_and_recursion_safe() {
        let home = std::env::temp_dir().join("hintshell-git-bash-fast-content");
        let rcfile = home.join(GIT_BASH_FAST_RCFILE_NAME);
        let content = git_bash_fast_rcfile_content(&rcfile);

        assert!(content.contains("PS1='\\[\\e[32m\\]\\u@\\h \\[\\e[33m\\]\\w\\[\\e[0m\\]\\n$ '"));
        assert!(content.contains("source '"));
        assert!(content.contains("_hintshell_live_record"));
        assert!(content.contains(") > /dev/null 2>&1 & disown"));
        assert!(content.contains("add --command \"$last_cmd\""));
        assert!(!content.contains("source \"$HOME/.bashrc\""));
        assert!(!content.contains(". \"$HOME/.bashrc\""));
        assert!(!content.contains("source \"$HOME/.bash_profile\""));
        assert!(!content.contains(". \"$HOME/.bash_profile\""));
    }

    #[test]
    fn git_bash_fast_rcfile_write_replaces_only_managed_file() {
        let root = std::env::temp_dir().join("hintshell-git-bash-fast-write");
        let rcfile = root.join(GIT_BASH_FAST_RCFILE_NAME);
        let user_bashrc = root.join(".bashrc");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(&user_bashrc, "user configuration\n").unwrap();
        fs::write(&rcfile, "stale managed content\n").unwrap();

        write_git_bash_fast_rcfile_at(&rcfile).unwrap();
        let hook = root.join(GIT_BASH_FAST_HOOK_NAME);
        write_git_bash_fast_hook_at(&hook).unwrap();
        assert!(fs::read_to_string(&hook)
            .unwrap()
            .contains("# --- HintShell Bash Integration ---"));
        assert!(fs::read_to_string(&rcfile)
            .unwrap()
            .contains("# HintShell Git Bash Fast initialization"));
        write_git_bash_fast_rcfile_at(&rcfile).unwrap();

        assert!(!fs::read_to_string(&rcfile)
            .unwrap()
            .contains("stale managed content"));
        assert_eq!(
            fs::read_to_string(&user_bashrc).unwrap(),
            "user configuration\n"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_bash_fast_rcfile_removal_is_idempotent() {
        let root = std::env::temp_dir().join("hintshell-git-bash-fast-remove");
        let rcfile = root.join(GIT_BASH_FAST_RCFILE_NAME);
        let user_bashrc = root.join(".bashrc");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(&rcfile, "managed\n").unwrap();
        fs::write(&user_bashrc, "user configuration\n").unwrap();

        remove_git_bash_fast_rcfile_at(&rcfile).unwrap();
        remove_git_bash_fast_rcfile_at(&rcfile).unwrap();

        assert!(!rcfile.exists());
        assert_eq!(
            fs::read_to_string(&user_bashrc).unwrap(),
            "user configuration\n"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn macos_bash_login_block_sources_bashrc() {
        let block = macos_bash_login_source_block();
        assert!(block.contains(BASH_LOGIN_MARKER));
        assert!(block.contains(". \"$HOME/.bashrc\""));
        assert!(block.contains(BASH_LOGIN_END_MARKER));
    }
}
