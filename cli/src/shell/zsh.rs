use super::hintshell_home;

/// Generate Zsh hook script (Tab-to-FZF, compatible with zsh-autosuggestions)
pub fn hook_script() -> String {
    let hs_bin = hintshell_home().join("bin").join("hintshell");
    let hs_bin_str = hs_bin.to_string_lossy().replace('\\', "/");
    let core_name = if cfg!(windows) { "hintshell-core.exe" } else { "hintshell-core" };
    let core_bin = hintshell_home().join("bin").join(core_name);
    let core_bin_str = core_bin.to_string_lossy().replace('\\', "/");

    [
        "\n# --- HintShell Zsh Integration ---\n".to_string(),
        format!("export HINTSHELL_BIN=\"{}\"\n", hs_bin_str),
        format!("export HINTSHELL_CORE=\"{}\"\n", core_bin_str),

        // Auto-start daemon if not running
        "\n_hintshell_ensure_daemon() {\n".to_string(),
        "    \"$HINTSHELL_BIN\" status >/dev/null 2>&1 && return\n".to_string(),
        "    [[ -x \"$HINTSHELL_CORE\" ]] && (\"$HINTSHELL_CORE\" >/dev/null 2>&1 &)\n".to_string(),
        "    sleep 0.2\n".to_string(),
        "}\n".to_string(),

        // Tab: ZLE widget with fzf picker
        "\n_hintshell_tab() {\n".to_string(),
        "    _hintshell_ensure_daemon\n".to_string(),
        "    local typed=\"$LBUFFER\"\n".to_string(),
        "    [[ -z \"$typed\" ]] && { zle expand-or-complete; return }\n".to_string(),
        "    local suggestions\n".to_string(),
        "    suggestions=$(\"$HINTSHELL_BIN\" suggest \"$typed\" --limit 15 --format fzf 2>/dev/null)\n".to_string(),
        "    [[ -z \"$suggestions\" ]] && { zle expand-or-complete; return }\n".to_string(),

        "    local count=$(echo \"$suggestions\" | wc -l | tr -d ' ')\n".to_string(),
        "    if [[ \"$count\" -eq 1 ]]; then\n".to_string(),
        "        LBUFFER=$(echo \"$suggestions\" | cut -f1)\n".to_string(),
        "    else\n".to_string(),
        "        local selected\n".to_string(),
        "        selected=$(echo \"$suggestions\" | fzf --height 40% --reverse --no-sort --cycle --delimiter='\\t' --with-nth=1,2 --prompt=\"🧠 HintShell> \" --header=\"Tab/Enter: select\")\n".to_string(),
        "        [[ -n \"$selected\" ]] && LBUFFER=$(echo \"$selected\" | cut -f1)\n".to_string(),
        "    fi\n".to_string(),
        "    zle reset-prompt\n".to_string(),
        "}\n".to_string(),
        "zle -N _hintshell_tab\n".to_string(),
        "bindkey '^I' _hintshell_tab\n".to_string(),

        // Record commands to history
        "\n_hintshell_precmd() {\n".to_string(),
        "    _hintshell_ensure_daemon\n".to_string(),
        "    local last_cmd=$(fc -ln -1 2>/dev/null | sed 's/^[[:space:]]*//')\n".to_string(),
        "    if [[ -n \"$last_cmd\" && \"$last_cmd\" != \"$_HS_LAST\" ]]; then\n".to_string(),
        "        _HS_LAST=\"$last_cmd\"\n".to_string(),
        "        (\"$HINTSHELL_BIN\" add --command \"$last_cmd\" --shell zsh >/dev/null 2>&1 &)\n".to_string(),
        "    fi\n".to_string(),
        "}\n".to_string(),
        "precmd_functions+=(_hintshell_precmd)\n".to_string(),
    ].concat()
}

/// Generate the init line for .zshrc
pub fn install_line() -> String {
    let hs_bin_dir = hintshell_home().join("bin");
    let hs_bin_dir_str = hs_bin_dir.to_string_lossy().replace('\\', "/");
    let hs_bin = hs_bin_dir.join("hintshell");
    let hs_bin_str = hs_bin.to_string_lossy().replace('\\', "/");
    format!(
        "\n# HintShell Initialization\nexport PATH=\"{}:$PATH\"\neval \"$({} hook zsh)\"\n",
        hs_bin_dir_str, hs_bin_str
    )
}
