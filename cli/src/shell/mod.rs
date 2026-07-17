mod bash;
mod powershell;
mod zsh;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

// ~/.hintshell/
pub fn hintshell_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hintshell")
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
    PowerShell,
    Bash,
    Zsh,
}

impl Shell {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "powershell" | "pwsh" => Some(Self::PowerShell),
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            _ => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::PowerShell => "PowerShell",
            Self::Bash => "Bash",
            Self::Zsh => "Zsh",
        }
    }

    /// Path to the shell's rc/profile config file
    pub fn config_path(&self) -> Option<PathBuf> {
        match self {
            Self::PowerShell => {
                #[cfg(windows)]
                {
                    let docs = dirs::document_dir()?;
                    Some(docs.join("PowerShell\\Microsoft.PowerShell_profile.ps1"))
                }
                #[cfg(unix)]
                {
                    let home = dirs::home_dir()?;
                    Some(home.join(".config/powershell/Microsoft.PowerShell_profile.ps1"))
                }
            }
            Self::Bash => Some(dirs::home_dir()?.join(".bashrc")),
            Self::Zsh => Some(dirs::home_dir()?.join(".zshrc")),
        }
    }

    /// Output the hook script to be eval'd by the shell
    pub fn get_hook(&self) -> String {
        let module_dir = hintshell_home().join("module");
        let module_dir_str = module_dir.to_string_lossy().replace('\\', "/");
        let bin_dir = hintshell_home().join("bin");
        let daemon_name = if cfg!(windows) { "hintshell-core.exe" } else { "hintshell-core" };
        let daemon_path = bin_dir.join(daemon_name);
        let daemon_str = daemon_path.to_string_lossy().replace('\\', "/");

        match self {
            Self::PowerShell => powershell::hook_script(&daemon_str, &module_dir_str),
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
            Self::PowerShell => powershell::install_line(&module_str),
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

        // Use the comment marker as idempotency check
        if !content.contains("# HintShell Initialization") {
            let mut new_content = content;
            new_content.push_str(&init_line);
            fs::write(&config, new_content).map_err(|e| e.to_string())?;
            Ok(())
        } else {
            Err("Already installed".to_string())
        }
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
            Ok(())
        } else {
            Ok(()) // Already uninstalled or not found
        }
    }
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

    // 1. Copy hintshell binary itself
    let hs_name = if cfg!(windows) { "hintshell.exe" } else { "hintshell" };
    let dest_hs = bin_dir.join(hs_name);
    let _ = fs::remove_file(&dest_hs); // Remove first to avoid "Text file busy" on Linux
    fs::copy(bin_path, &dest_hs)
        .map_err(|e| format!("Copy hintshell failed: {}", e))?;
    #[cfg(unix)]
    let _ = set_executable(&dest_hs);

    // 1b. Copy hs shorthand alias
    let hs_short = if cfg!(windows) { "hs.exe" } else { "hs" };
    if let Some(parent) = bin_path.parent() {
        let hs_src = parent.join(hs_short);
        if hs_src.exists() {
            let dest_short = bin_dir.join(hs_short);
            let _ = fs::remove_file(&dest_short);
            fs::copy(&hs_src, &dest_short)
                .map_err(|e| format!("Copy hs failed: {}", e))?;
            #[cfg(unix)]
            let _ = set_executable(&dest_short);
        }
    }

    // 2. Copy hintshell-core daemon (sibling of hintshell binary)
    let core_name = if cfg!(windows) { "hintshell-core.exe" } else { "hintshell-core" };
    if let Some(parent) = bin_path.parent() {
        let core_src = parent.join(core_name);
        if core_src.exists() {
            let dest_core = bin_dir.join(core_name);
            let _ = fs::remove_file(&dest_core);
            fs::copy(&core_src, &dest_core)
                .map_err(|e| format!("Copy core failed: {}", e))?;
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
            copy_dir_all(&src, &module_dir)
                .map_err(|e| format!("Copy module failed: {}", e))?;

            // Also copy hintshell-core into module/ so $PSScriptRoot finds it
            if let Some(parent) = bin_path.parent() {
                let core_src = parent.join(core_name);
                if core_src.exists() {
                    let dest_module_core = module_dir.join(core_name);
                    fs::copy(&core_src, &dest_module_core)
                        .map_err(|e| format!("Copy core to module failed: {}", e))?;
                    #[cfg(unix)]
                    let _ = set_executable(&dest_module_core);
                }
            }

            // Also copy default-commands.json into ~/.hintshell/ for runtime loading
            let defaults_src = src.join("default-commands.json");
            if defaults_src.exists() {
                fs::copy(&defaults_src, home.join("default-commands.json"))
                    .map_err(|e| format!("Copy default-commands.json failed: {}", e))?;
            }
        }
        None => {
            return Err(
                "Could not find HintShellModule. Make sure you built the project correctly.".to_string()
            );
        }
    }

    Ok(())
}

fn find_module_src(bin_path: &std::path::Path) -> Option<std::path::PathBuf> {
    // Priority A: adjacent 'module/' dir (for distributed release)
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
        shells.push(Shell::PowerShell);
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
    let cmd = if cfg!(windows) { format!("{}.exe", cmd) } else { cmd.to_string() };
    env::var_os("PATH").map_or(false, |paths| {
        env::split_paths(&paths).any(|p| p.join(&cmd).exists())
    })
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}
