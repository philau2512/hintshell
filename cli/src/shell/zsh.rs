use super::hintshell_home;

/// Generate Zsh hook script (Advanced Overlay with ZLE widgets)
pub fn hook_script() -> String {
    let hs_bin = hintshell_home().join("bin").join("hintshell");
    let hs_bin_str = hs_bin.to_string_lossy().replace('\\', "/");
    let core_name = if cfg!(windows) { "hintshell-core.exe" } else { "hintshell-core" };
    let core_bin = hintshell_home().join("bin").join(core_name);
    let core_bin_str = core_bin.to_string_lossy().replace('\\', "/");

    [
        "\n# --- HintShell Zsh Integration (Advanced Overlay) ---\n".to_string(),
        format!("export HINTSHELL_BIN=\"{}\"\n", hs_bin_str),
        format!("export HINTSHELL_CORE=\"{}\"\n", core_bin_str),
        "typeset -gA _HS_STATE\n".to_string(),
        "_HS_STATE=(suggestions \"\" index 0 visible 0 lines 0 last_input \"\")\n".to_string(),

        // Start daemon logic
        "\n_hintshell_ensure_daemon() {\n".to_string(),
        "    \"$HINTSHELL_BIN\" status >/dev/null 2>&1 && return\n".to_string(),
        "    [[ -x \"$HINTSHELL_CORE\" ]] && (\"$HINTSHELL_CORE\" >/dev/null 2>&1 &)\n".to_string(),
        "    sleep 0.2\n".to_string(),
        "}\n".to_string(),

        // Clear overlay
        "\n_hintshell_clear() {\n".to_string(),
        "    [[ $_HS_STATE[visible] -eq 0 ]] && return\n".to_string(),
        "    local n=$_HS_STATE[lines]\n".to_string(),
        "    [[ $n -eq 0 ]] && return\n".to_string(),
        "    # Move down, clear line, move back up\n".to_string(),
        "    repeat $n; printf \"\\e[1B\\e[2K\"\n".to_string(),
        "    repeat $n; printf \"\\e[1A\"\n".to_string(),
        "    _HS_STATE[visible]=0\n".to_string(),
        "    _HS_STATE[lines]=0\n".to_string(),
        "}\n".to_string(),

        // Draw overlay
        "\n_hintshell_draw() {\n".to_string(),
        "    _hintshell_clear\n".to_string(),
        "    local input=\"$LBUFFER\"\n".to_string(),
        "    [[ -z \"$input\" ]] && return\n".to_string(),
        
        "    local suggestions\n".to_string(),
        "    suggestions=$(\"$HINTSHELL_BIN\" suggest \"$input\" --limit 6 --format plain 2>/dev/null)\n".to_string(),
        "    [[ -z \"$suggestions\" ]] && return\n".to_string(),

        "    local -a lines\n".to_string(),
        "    lines=(${(f)suggestions})\n".to_string(),
        "    local count=${#lines}\n".to_string(),
        "    local idx=$_HS_STATE[index]\n".to_string(),
        
        "    # Boundary check\n".to_string(),
        "    [[ $idx -lt 1 ]] && idx=1\n".to_string(),
        "    [[ $idx -gt $count ]] && idx=$count\n".to_string(),
        "    _HS_STATE[index]=$idx\n".to_string(),

        "    printf \"\\e[s\" # Save cursor\n".to_string(),
        "    printf \"\\n\\e[38;5;238m\" # Move down\n".to_string(),
        "    # Start drawing box\n".to_string(),
        "    printf \"\\u2500\" # Separator\n".to_string(),
        
        "    local i=1\n".to_string(),
        "    for s in $lines; do\n".to_string(),
        "        printf \"\\r\\e[1B\\e[2K\"\n".to_string(),
        "        if [[ $i -eq $idx ]]; then\n".to_string(),
        "            printf \"\\e[48;5;236m\\e[38;5;15m > %-60s\\e[0m\" \"$s\"\n".to_string(),
        "        else\n".to_string(),
        "            printf \"\\e[38;5;248m   %-60s\\e[0m\" \"$s\"\n".to_string(),
        "        fi\n".to_string(),
        "        i=$((i+1))\n".to_string(),
        "    done\n".to_string(),
        
        "    _HS_STATE[visible]=1\n".to_string(),
        "    _HS_STATE[lines]=$((count + 1))\n".to_string(),
        "    _HS_STATE[suggestions]=\"$suggestions\"\n".to_string(),
        
        "    printf \"\\e[u\" # Restore cursor\n".to_string(),
        "}\n".to_string(),

        // Key Handlers
        "\n_hintshell_self_insert() {\n".to_string(),
        "    zle .self-insert\n".to_string(),
        "    _HS_STATE[index]=1\n".to_string(),
        "    _hintshell_draw\n".to_string(),
        "}\n".to_string(),
        
        "\n_hintshell_on_key() {\n".to_string(),
        "    case \"$WIDGET\" in\n".to_string(),
        "        up-line-or-history) \n".to_string(),
        "            if [[ $_HS_STATE[visible] -eq 1 ]]; then\n".to_string(),
        "                _HS_STATE[index]=$((_HS_STATE[index]-1))\n".to_string(),
        "                _hintshell_draw\n".to_string(),
        "                return\n".to_string(),
        "            fi ;;\n".to_string(),
        "        down-line-or-history)\n".to_string(),
        "            if [[ $_HS_STATE[visible] -eq 1 ]]; then\n".to_string(),
        "                _HS_STATE[index]=$((_HS_STATE[index]+1))\n".to_string(),
        "                _hintshell_draw\n".to_string(),
        "                return\n".to_string(),
        "            fi ;;\n".to_string(),
        "        accept-line)\n".to_string(),
        "            _hintshell_clear ;;\n".to_string(),
        "    esac\n".to_string(),
        "    zle .$WIDGET\n".to_string(),
        "}\n".to_string(),

        "\n_hintshell_tab() {\n".to_string(),
        "    if [[ $_HS_STATE[visible] -eq 1 ]]; then\n".to_string(),
        "        local -a lines\n".to_string(),
        "        lines=(${(f)_HS_STATE[suggestions]})\n".to_string(),
        "        LBUFFER=\"$lines[$_HS_STATE[index]]\"\n".to_string(),
        "        _hintshell_clear\n".to_string(),
        "    else\n".to_string(),
        "        zle expand-or-complete\n".to_string(),
        "    fi\n".to_string(),
        "}\n".to_string(),

        // Setup widgets
        "\nzle -N self-insert _hintshell_self_insert\n".to_string(),
        "zle -N _hintshell_tab\n".to_string(),
        "zle -N up-line-or-history _hintshell_on_key\n".to_string(),
        "zle -N down-line-or-history _hintshell_on_key\n".to_string(),
        "zle -N accept-line _hintshell_on_key\n".to_string(),
        "bindkey '^I' _hintshell_tab\n".to_string(),

        "\n_hintshell_precmd() {\n".to_string(),
        "    _hintshell_ensure_daemon\n".to_string(),
        "    local last_cmd=$(fc -ln -1 | sed 's/^[[:space:]]*//')\n".to_string(),
        "    [[ -n \"$last_cmd\" ]] && (\"$HINTSHELL_BIN\" add --command \"$last_cmd\" --shell zsh >/dev/null 2>&1 &)\n".to_string(),
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
