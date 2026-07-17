#!/usr/bin/env bash
set -euo pipefail
BIN="/d/Admin/Documents/PROJECTS/HintShell/target/release/hintshell.exe"
[[ -x "$BIN" ]] || BIN="/d/Admin/Documents/PROJECTS/HintShell/target/release/hintshell"
[[ -x "$BIN" ]] || { echo "FAIL: binary missing"; exit 1; }

# Load only fzf resolve helpers from generated hook
eval "$(
  "$BIN" hook bash 2>/dev/null | awk '
    /^_hintshell_try_fzf\(\)/ {p=1}
    p {print}
    /^}$/ && p && seen++ { if (seen>=2) exit }
  '
)"

resolved="$(_hintshell_resolve_fzf)"
echo "RESOLVED=$resolved"
# Must not be the WinGet Links shim
case "$resolved" in
  */WinGet/Links/*)
    echo "FAIL: still resolved to Links shim"
    exit 1
    ;;
esac
ver="$("$resolved" --version 2>&1)"
echo "VERSION=$ver"
echo "OK fzf resolve"