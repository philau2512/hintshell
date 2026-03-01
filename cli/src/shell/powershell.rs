/// Generate PowerShell hook script
pub fn hook_script(daemon_str: &str, module_dir_str: &str) -> String {
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

/// Generate the init line for PowerShell profile
pub fn install_line(module_dir_str: &str) -> String {
    format!(
        "\n# HintShell Initialization\nImport-Module \"{}/HintShellModule.psd1\" -DisableNameChecking -ErrorAction SilentlyContinue\nStart-HintShell\n",
        module_dir_str
    )
}
