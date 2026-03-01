use super::hintshell_home;

/// Generate Bash hook script (compatible with macOS Bash 3.2+)
pub fn hook_script() -> String {
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
        "    \"$HINTSHELL_BIN\" status >/dev/null 2>&1 && return\n".to_string(),
        "    [[ -x \"$HINTSHELL_CORE\" ]] && (\"$HINTSHELL_CORE\" >/dev/null 2>&1 &)\n".to_string(),
        "    sleep 0.2\n".to_string(),
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
        // 1 suggestion → accept directly
        "    if [[ \"$count\" -eq 1 ]]; then\n".to_string(),
        "        READLINE_LINE=\"$suggestions\"\n".to_string(),
        "        READLINE_POINT=${#READLINE_LINE}\n".to_string(),
        "    else\n".to_string(),
        "        local selected\n".to_string(),
        "        selected=$(echo \"$suggestions\" | fzf --height 40% --reverse --no-sort --prompt=\"🧠 HintShell> \" --header=\"Tab/Enter: select\")\n".to_string(),
        "        if [[ -n \"$selected\" ]]; then\n".to_string(),
        "            READLINE_LINE=\"$selected\"\n".to_string(),
        "            READLINE_POINT=${#READLINE_LINE}\n".to_string(),
        "        fi\n".to_string(),
        "    fi\n".to_string(),
        "}\n".to_string(),
        "bind -x '\"\\t\": _hintshell_tab'\n".to_string(),

        // Record executed commands
        "\n_hintshell_preexec() {\n".to_string(),
        "    _hintshell_ensure_daemon\n".to_string(),
        "    local last_cmd\n".to_string(),
        "    last_cmd=$(HISTTIMEFORMAT=\"\" history 1 | sed 's/^[ ]*[0-9]*[ ]*//')\n".to_string(),
        "    if [[ -n \"$last_cmd\" && \"$last_cmd\" != \"$_HS_LAST\" ]]; then\n".to_string(),
        "        _HS_LAST=\"$last_cmd\"\n".to_string(),
        "        (\"$HINTSHELL_BIN\" add --command \"$last_cmd\" --shell bash >/dev/null 2>&1 &)\n".to_string(),
        "    fi\n".to_string(),
        "}\n".to_string(),
        "[[ \"$PROMPT_COMMAND\" != *_hintshell_preexec* ]] && PROMPT_COMMAND=\"_hintshell_preexec;$PROMPT_COMMAND\"\n".to_string(),
    ].concat()
}

/// Generate the init line for .bashrc
pub fn install_line() -> String {
    let hs_bin_dir = hintshell_home().join("bin");
    let hs_bin_dir_str = hs_bin_dir.to_string_lossy().replace('\\', "/");
    let hs_bin = hs_bin_dir.join("hintshell");
    let hs_bin_str = hs_bin.to_string_lossy().replace('\\', "/");
    format!(
        "\n# HintShell Initialization\nexport PATH=\"{}:$PATH\"\neval \"$({} hook bash)\"\n",
        hs_bin_dir_str, hs_bin_str
    )
}
