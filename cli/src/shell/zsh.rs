use super::{bin_dir_posix, cli_bin_posix, core_bin_posix, fzf_picker_args, fzf_resolve_functions};

/// Generate Zsh hook script (Tab-to-FZF, compatible with zsh-autosuggestions)
pub fn hook_script() -> String {
    let hs_bin_str = cli_bin_posix();
    let core_bin_str = core_bin_posix();
    let fzf_args = fzf_picker_args();

    [
        "\n# --- HintShell Zsh Integration ---\n".to_string(),
        format!("export HINTSHELL_BIN=\"{}\"\n", hs_bin_str),
        format!("export HINTSHELL_CORE=\"{}\"\n", core_bin_str),
        fzf_resolve_functions(),
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
        // Multi-line buffer: fall back to default completion
        "    case \"$typed\" in *$'\\n'*|*$'\\r'*) zle expand-or-complete; return ;; esac\n".to_string(),
        "    local suggestions\n".to_string(),
        "    suggestions=$(\"$HINTSHELL_BIN\" suggest \"$typed\" --limit 15 --cwd \"$PWD\" --shell zsh --format fzf 2>/dev/null)\n"
            .to_string(),
        "    [[ -z \"$suggestions\" ]] && { zle expand-or-complete; return }\n".to_string(),
        "    local count selected=\"\"\n".to_string(),
        "    count=$(printf '%s\\n' \"$suggestions\" | wc -l | tr -d ' ')\n".to_string(),
        "    if [[ \"$count\" -eq 1 ]]; then\n".to_string(),
        "        selected=\"$suggestions\"\n".to_string(),
        "    else\n".to_string(),
        "        local fzf_bin\n".to_string(),
        "        if fzf_bin=$(_hintshell_resolve_fzf); then\n".to_string(),
        format!(
            "            selected=$(printf '%s\\n' \"$suggestions\" | \"$fzf_bin\" {} 2>/dev/null)\n",
            fzf_args
        ),
        "        else\n".to_string(),
        "            selected=$(printf '%s\\n' \"$suggestions\" | head -n 1)\n".to_string(),
        "        fi\n".to_string(),
        "    fi\n".to_string(),
        "    if [[ -n \"$selected\" ]]; then\n".to_string(),
        "        LBUFFER=$(printf '%s\\n' \"$selected\" | awk -F'\\t' '{sub(/ +$/, \"\", $1); print $1}')\n"
            .to_string(),
        "    fi\n".to_string(),
        "    zle reset-prompt\n".to_string(),
        "}\n".to_string(),
        "zle -N _hintshell_tab\n".to_string(),
        "bindkey '^I' _hintshell_tab\n".to_string(),
        // Record commands to history
        "\n_hintshell_precmd() {\n".to_string(),
        "    _hintshell_ensure_daemon\n".to_string(),
        "    local last_cmd=$(fc -ln -1 2>/dev/null | sed 's/^[[:space:]]*//')\n".to_string(),
        "    [[ -n \"$last_cmd\" ]] && (\"$HINTSHELL_BIN\" add --command \"$last_cmd\" --directory \"$PWD\" --shell zsh >/dev/null 2>&1 &)\n"
            .to_string(),
        "}\n".to_string(),
        "precmd_functions=(${precmd_functions:#_hintshell_precmd} _hintshell_precmd)\n".to_string(),
        "\n# Auto-start daemon\n".to_string(),
        "_hintshell_ensure_daemon\n".to_string(),
    ]
    .concat()
}

/// Generate the init line for .zshrc
pub fn install_line() -> String {
    let bin_dir = bin_dir_posix();
    let cli = cli_bin_posix();
    format!(
        r#"
# HintShell Initialization
export PATH="{bin_dir}:$PATH"
if [ -x "{cli}" ]; then
  _hs_hook="$("{cli}" hook zsh 2>/dev/null)" && [ -n "$_hs_hook" ] && eval "$_hs_hook"
  unset _hs_hook
fi
# End HintShell
"#,
        bin_dir = bin_dir,
        cli = cli
    )
}
