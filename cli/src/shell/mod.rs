mod bash;
mod powershell;
mod zsh;

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
        
        // Use the marker to find the block
        let marker = "# HintShell Initialization";
        if let Some(start_idx) = content.find(marker) {
            // Usually the marker is on a line by itself, we want to find the start of that line
            let mut start_of_block = start_idx;
            while start_of_block > 0 && content.as_bytes()[start_of_block-1] != b'\n' {
                start_of_block -= 1;
            }

            // Find the end of the block (usually 2-4 lines after)
            // For now, let's look for the next blank line or next significant newline sequence
            let mut end_of_block = start_idx;
            let mut lines_count = 0;
            while end_of_block < content.len() && lines_count < 4 {
                if content.as_bytes()[end_of_block] == b'\n' {
                    lines_count += 1;
                }
                end_of_block += 1;
            }

            let mut new_content = content;
            new_content.replace_range(start_of_block..end_of_block, "");
            fs::write(&config, new_content).map_err(|e| e.to_string())?;
            Ok(())
        } else {
            Ok(()) // Already uninstalled or not found
        }
    }
}

/// Remove binaries and modules from ~/.hintshell/
pub fn uninstall_assets() -> Result<(), String> {
    let home = hintshell_home();
    let bin_dir = home.join("bin");
    let module_dir = home.join("module");

    if bin_dir.exists() {
        fs::remove_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    }
    if module_dir.exists() {
        fs::remove_dir_all(&module_dir).map_err(|e| e.to_string())?;
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

    // 1b. Copy hs shorthand alias
    let hs_short = if cfg!(windows) { "hs.exe" } else { "hs" };
    if let Some(parent) = bin_path.parent() {
        let hs_src = parent.join(hs_short);
        if hs_src.exists() {
            let dest_short = bin_dir.join(hs_short);
            let _ = fs::remove_file(&dest_short);
            fs::copy(&hs_src, &dest_short)
                .map_err(|e| format!("Copy hs failed: {}", e))?;
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
