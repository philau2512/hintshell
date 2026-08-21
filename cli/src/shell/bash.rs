use super::{bin_dir_posix, cli_bin_posix, core_bin_posix, fzf_picker_args, fzf_resolve_functions};

/// Generate Bash hook script (compatible with macOS Bash 3.2+)
pub fn hook_script() -> String {
    let hs_bin_str = cli_bin_posix();
    let core_bin_str = core_bin_posix();
    let fzf_args = fzf_picker_args();

    [
        "\n# --- HintShell Bash Integration ---\n".to_string(),
        format!("export HINTSHELL_BIN=\"{}\"\n", hs_bin_str),
        format!("export HINTSHELL_CORE=\"{}\"\n", core_bin_str),
        fzf_resolve_functions(),
        // Auto-start daemon if not running
        "\n_hintshell_ensure_daemon() {\n".to_string(),
        "    [[ -n \"${HINTSHELL_SKIP_DAEMON_START:-}\" ]] && return\n".to_string(),
        "    \"$HINTSHELL_BIN\" status >/dev/null 2>&1 && return\n".to_string(),
        "    [[ -x \"$HINTSHELL_CORE\" ]] && (\"$HINTSHELL_CORE\" >/dev/null 2>&1 &)\n".to_string(),
        "    sleep 0.2\n".to_string(),
        "}\n".to_string(),
        // Tab: fzf picker only outside the live PTY wrapper.
        "\nif [[ -z \"${HINTSHELL_LIVE_BASH:-}\" ]]; then\n".to_string(),
        "_hintshell_tab() {\n".to_string(),
        "    _hintshell_ensure_daemon\n".to_string(),
        "    local typed=\"$READLINE_LINE\"\n".to_string(),
        "    [[ -z \"$typed\" ]] && return\n".to_string(),
        // Multi-line buffer: do not open picker over continuation lines
        "    case \"$typed\" in *$'\\n'*|*$'\\r'*) return ;; esac\n".to_string(),
        "    local suggestions\n".to_string(),
        "    suggestions=$(\"$HINTSHELL_BIN\" suggest \"$typed\" --limit 10 --cwd \"$PWD\" --shell bash --format fzf 2>/dev/null)\n"
            .to_string(),
        "    [[ -z \"$suggestions\" ]] && return\n".to_string(),
        "    local count\n".to_string(),
        "    count=$(printf '%s\\n' \"$suggestions\" | wc -l | tr -d ' ')\n".to_string(),
        "    local selected=\"\"\n".to_string(),
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
        // No working fzf: take top match silently (avoid Permission denied noise)
        "            selected=$(printf '%s\\n' \"$suggestions\" | head -n 1)\n".to_string(),
        "        fi\n".to_string(),
        "    fi\n".to_string(),
        "    if [[ -n \"$selected\" ]]; then\n".to_string(),
        "        READLINE_LINE=$(printf '%s\\n' \"$selected\" | awk -F'\\t' '{sub(/ +$/, \"\", $1); print $1}')\n"
            .to_string(),
        "        READLINE_POINT=${#READLINE_LINE}\n".to_string(),
        "    fi\n".to_string(),
        "}\n".to_string(),
        "bind -x '\"\\t\": _hintshell_tab'\nfi\n".to_string(),
        // Record executed commands without blocking live-overlay prompt redraws.
        "\n_hintshell_preexec() {\n".to_string(),
        "    [[ -z \"${HINTSHELL_LIVE_BASH:-}\" ]] && _hintshell_ensure_daemon\n".to_string(),
        "    local last_cmd\n".to_string(),
        "    last_cmd=$(HISTTIMEFORMAT=\"\" history 1 | sed 's/^[ ]*[0-9]*[ ]*//')\n".to_string(),
        "    [[ -n \"$last_cmd\" ]] && (\"$HINTSHELL_BIN\" add --command \"$last_cmd\" --directory \"$PWD\" --shell bash >/dev/null 2>&1 &)\n"
            .to_string(),
        "}\n".to_string(),
        "[[ \"$PROMPT_COMMAND\" != *_hintshell_preexec* ]] && PROMPT_COMMAND=\"_hintshell_preexec;$PROMPT_COMMAND\"\n"
            .to_string(),
        "\nif [[ -n \"${HINTSHELL_LIVE_BASH:-}\" && -z \"${MSYSTEM:-}\" ]]; then\n".to_string(),
        "_hintshell_emit_prompt() { printf '\\036HINTSHELL_CWD:%s\\037\\036HINTSHELL_PROMPT\\037' \"$PWD\"; }\n".to_string(),
        "PROMPT_COMMAND=\"_hintshell_emit_prompt;$PROMPT_COMMAND\"\nfi\n".to_string(),
        "\n# Daemon startup is deferred until a command or non-live Tab query needs it.\n".to_string(),
    ]
    .concat()
}

/// Generate the init line for .bashrc
/// Always call the absolute ~/.hintshell binary so npm global never shadows hook generation.
pub fn install_line() -> String {
    let bin_dir = bin_dir_posix();
    let cli = cli_bin_posix();
    format!(
        r#"
# HintShell Initialization
# Enable the PTY-based live suggestion overlay by default on macOS.
if [[ "$(uname -s 2>/dev/null)" == "Darwin" ]]; then
  export HINTSHELL_ENABLE_MACOS_LIVE_OVERLAY=1
fi
# Use POSIX path so Git Bash finds binaries before npm global shims
export PATH="{bin_dir}:$PATH"
if [ -x "{cli}" ]; then
  _hs_hook="$("{cli}" hook bash 2>/dev/null)" && [ -n "$_hs_hook" ] && eval "$_hs_hook"
  unset _hs_hook
fi
# Start the live overlay for Git Bash/WSL2, or macOS only when explicitly enabled.
# Set HINTSHELL_DISABLE_AUTO_BASH=1 to bypass it for one session.
if [[ ( -n "${{MSYSTEM:-}}" || -n "${{WSL_INTEROP:-}}" || -n "${{WSL_DISTRO_NAME:-}}" || ( "$(uname -s 2>/dev/null)" == "Darwin" && -n "${{HINTSHELL_ENABLE_MACOS_LIVE_OVERLAY:-}}" ) ) && $- == *i* && -t 0 && -t 1 && -z "${{BASH_EXECUTION_STRING:-}}" && -z "${{HINTSHELL_LIVE_BASH:-}}" && -z "${{HINTSHELL_DISABLE_AUTO_BASH:-}}" ]]; then
  exec "{cli}" bash
fi
# End HintShell
"#,
        bin_dir = bin_dir,
        cli = cli
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_bash_hook_skips_prompt_markers_in_git_bash() {
        let hook = hook_script();
        assert!(hook.contains("HINTSHELL_LIVE_BASH:-}\" && -z \"${MSYSTEM:-}"));
    }

    #[test]
    fn live_bash_hook_defers_daemon_startup_until_history_recording_is_needed() {
        let hook = hook_script();
        assert!(hook.contains("[[ -z \"${HINTSHELL_LIVE_BASH:-}\" ]] && _hintshell_ensure_daemon"));
    }

    #[test]
    fn hook_defers_daemon_startup_until_a_non_live_action() {
        let hook = hook_script();
        assert!(hook.contains("Daemon startup is deferred"));
        assert!(!hook.contains("\n_hintshell_ensure_daemon\n"));
        assert!(hook.contains("_hintshell_tab() {\n    _hintshell_ensure_daemon"));
        assert!(hook.contains(
            "_hintshell_preexec() {\n    [[ -z \"${HINTSHELL_LIVE_BASH:-}\" ]] && _hintshell_ensure_daemon"
        ));
    }

    #[test]
    fn install_line_enables_macos_live_overlay_by_default() {
        let line = install_line();
        assert!(line.contains("HINTSHELL_ENABLE_MACOS_LIVE_OVERLAY=1"));
        assert!(line.contains("HINTSHELL_DISABLE_AUTO_BASH"));
        assert!(line.contains("HINTSHELL_ENABLE_MACOS_LIVE_OVERLAY"));
        assert!(line.contains("Darwin"));
        assert!(line.contains("BASH_EXECUTION_STRING"));
        assert!(line.contains("HINTSHELL_LIVE_BASH"));
        assert!(line.contains("MSYSTEM"));
        assert!(line.contains("WSL_INTEROP"));
        assert!(line.contains("WSL_DISTRO_NAME"));
        assert!(line.contains("exec \""));
        assert!(line.contains("\" bash"));
    }
}
