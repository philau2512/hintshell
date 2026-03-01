use std::env;
use std::fs;
use std::path::PathBuf;

// ~/.hintshell/
pub fn hintshell_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hintshell")
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
            Self::PowerShell => {
                format!(
                    r#"
# --- HintShell PowerShell Integration ---
$env:HINTSHELL_DAEMON = "{daemon}"
Import-Module "{module}/HintShellModule.psd1" -DisableNameChecking -ErrorAction SilentlyContinue
if (-not (Get-Module -Name HintShellModule)) {{
    Write-Warning "HintShell: Module not found at {module}. Re-run: hintshell init"
}}
"#,
                    daemon = daemon_str,
                    module = module_dir_str
                )
            }
            Self::Bash => {
                let hs_bin = hintshell_home().join("bin").join("hintshell");
                let hs_bin_str = hs_bin.to_string_lossy().replace('\\', "/");
                let core_name = if cfg!(windows) { "hintshell-core.exe" } else { "hintshell-core" };
                let core_bin = hintshell_home().join("bin").join(core_name);
                let core_bin_str = core_bin.to_string_lossy().replace('\\', "/");

                [
                    "\n# --- HintShell Bash Integration ---\n".to_string(),
                    format!("export HINTSHELL_BIN=\"{}\"\n", hs_bin_str),
                    format!("export HINTSHELL_CORE=\"{}\"\n", core_bin_str),

                    // Auto-start daemon if not running
                    "\n_hintshell_ensure_daemon() {\n".to_string(),
                    "    \"$HINTSHELL_BIN\" status &>/dev/null && return\n".to_string(),
                    "    [[ -x \"$HINTSHELL_CORE\" ]] && (\"$HINTSHELL_CORE\" &>/dev/null &)\n".to_string(),
                    "    sleep 0.5\n".to_string(),
                    "}\n".to_string(),

                    // Tab: fzf picker
                    "\n_hintshell_tab() {\n".to_string(),
                    "    _hintshell_ensure_daemon\n".to_string(),
                    "    local typed=\"$READLINE_LINE\"\n".to_string(),
                    "    [[ -z \"$typed\" ]] && return\n".to_string(),
                    "    local suggestions\n".to_string(),
                    "    suggestions=$(\"$HINTSHELL_BIN\" suggest \"$typed\" --limit 10 --format plain 2>/dev/null)\n".to_string(),
                    "    [[ -z \"$suggestions\" ]] && return\n".to_string(),
                    "    local count\n".to_string(),
                    "    count=$(echo \"$suggestions\" | wc -l)\n".to_string(),
                    // 1 suggestion → accept directly, no picker
                    "    if [[ \"$count\" -eq 1 ]]; then\n".to_string(),
                    "        READLINE_LINE=\"$suggestions\"\n".to_string(),
                    "        READLINE_POINT=${#READLINE_LINE}\n".to_string(),
                    "        return\n".to_string(),
                    "    fi\n".to_string(),
                    // Multiple → fzf picker
                    "    local selected\n".to_string(),
                    "    selected=$(echo \"$suggestions\" | fzf \\\n".to_string(),
                    "        --height 40% --reverse --no-sort \\\n".to_string(),
                    "        --prompt=\"🧠 HintShell> \" \\\n".to_string(),
                    "        --header=\"Tab/Enter: select  Esc: cancel\")\n".to_string(),
                    "    if [[ -n \"$selected\" ]]; then\n".to_string(),
                    "        READLINE_LINE=\"$selected\"\n".to_string(),
                    "        READLINE_POINT=${#READLINE_LINE}\n".to_string(),
                    "    fi\n".to_string(),
                    "}\n".to_string(),
                    "bind -x '\"\\t\": _hintshell_tab'\n".to_string(),

                    // Record executed commands to history
                    "\n_hintshell_record() {\n".to_string(),
                    "    local last_cmd\n".to_string(),
                    "    last_cmd=$(HISTTIMEFORMAT=\"\" history 1 | sed 's/^[ ]*[0-9]*[ ]*//')\n".to_string(),
                    "    if [[ -n \"$last_cmd\" && \"$last_cmd\" != \"$_HS_LAST\" ]]; then\n".to_string(),
                    "        _HS_LAST=\"$last_cmd\"\n".to_string(),
                    "        (\"$HINTSHELL_BIN\" add --command \"$last_cmd\" --shell bash &>/dev/null &)\n".to_string(),
                    "    fi\n".to_string(),
                    "}\n".to_string(),
                    "PROMPT_COMMAND=\"_hintshell_record${PROMPT_COMMAND:+; $PROMPT_COMMAND}\"\n".to_string(),
                ].concat()
            }
            Self::Zsh => {
                let hs_bin = hintshell_home().join("bin").join("hintshell");
                let hs_bin_str = hs_bin.to_string_lossy().replace('\\', "/");
                [
                    "\n# --- HintShell Zsh Integration ---\n".to_string(),
                    format!("export HINTSHELL_DAEMON=\"{}\"\n", daemon_str),
                    format!("export HINTSHELL_BIN=\"{}\"\n", hs_bin_str),
                    "\n_hintshell_precmd() {\n".to_string(),
                    "    local last_cmd=$(fc -ln -1 2>/dev/null | sed 's/^[[:space:]]*//')\n".to_string(),
                    "    if [[ -n \"$last_cmd\" && \"$last_cmd\" != \"$HINTSHELL_LAST_CMD\" ]]; then\n".to_string(),
                    "        HINTSHELL_LAST_CMD=\"$last_cmd\"\n".to_string(),
                    "        (\"$HINTSHELL_BIN\" add --command \"$last_cmd\" --shell zsh &>/dev/null &)\n".to_string(),
                    "    fi\n}\n".to_string(),
                    "precmd_functions+=(_hintshell_precmd)\n".to_string(),
                ].concat()
            }
        }
    }

    /// Install hook line into shell config file
    pub fn install(&self, _bin_path: &std::path::Path) -> Result<(), String> {
        let config = self.config_path().ok_or("Could not find config path")?;

        let module_dir = hintshell_home().join("module");
        let module_str = module_dir.to_string_lossy().replace('\\', "/");

        let init_line = match self {
            // PowerShell: Import-Module directly then auto-start
            Self::PowerShell => format!(
                "\n# HintShell Initialization\nImport-Module \"{}/HintShellModule.psd1\" -DisableNameChecking -ErrorAction SilentlyContinue\nStart-HintShell\n",
                module_str
            ),
            // Bash/Zsh: add bin to PATH then eval hook
            _ => {
                let hs_bin_dir = hintshell_home().join("bin");
                let hs_bin_dir_str = hs_bin_dir.to_string_lossy().replace('\\', "/");
                let hs_bin = hs_bin_dir.join("hintshell");
                let hs_bin_str = hs_bin.to_string_lossy().replace('\\', "/");
                format!(
                    "\n# HintShell Initialization\nexport PATH=\"{}:$PATH\"\neval \"$({} hook {})\"\n",
                    hs_bin_dir_str,
                    hs_bin_str,
                    match self {
                        Self::Bash => "bash",
                        Self::Zsh => "zsh",
                        _ => unreachable!(),
                    }
                )
            }
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
    fs::copy(bin_path, bin_dir.join(hs_name))
        .map_err(|e| format!("Copy hintshell failed: {}", e))?;

    // 2. Copy hintshell-core daemon (sibling of hintshell binary)
    let core_name = if cfg!(windows) { "hintshell-core.exe" } else { "hintshell-core" };
    if let Some(parent) = bin_path.parent() {
        let core_src = parent.join(core_name);
        if core_src.exists() {
            fs::copy(&core_src, bin_dir.join(core_name))
                .map_err(|e| format!("Copy core failed: {}", e))?;
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
                    fs::copy(&core_src, module_dir.join(core_name))
                        .map_err(|e| format!("Copy core to module failed: {}", e))?;
                }
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
