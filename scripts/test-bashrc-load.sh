#!/usr/bin/env bash
set -e
# Simulate what user does after fix
source "$HOME/.bashrc"
echo "which hintshell: $(which hintshell 2>/dev/null || true)"
echo "type resolve: $(type -t _hintshell_resolve_fzf 2>/dev/null || echo NOT_FOUND)"
if type -t _hintshell_resolve_fzf >/dev/null 2>&1; then
  r="$(_hintshell_resolve_fzf 2>&1)"
  echo "resolve: $r"
  if printf '%s' "$r" | grep -qi 'Permission denied'; then
    echo FAIL_permission
    exit 1
  fi
  case "$r" in
    */Packages/*) echo OK ;;
    *) echo FAIL_path; exit 1 ;;
  esac
else
  echo FAIL_no_function
  # debug
  echo "HINTSHELL_BIN=${HINTSHELL_BIN:-unset}"
  ls -la /c/Users/ADMIN/.hintshell/bin/ 2>&1 | head -10
  exit 1
fi
echo "grep Links in hook: $(/c/Users/ADMIN/.hintshell/bin/hintshell.exe hook bash 2>/dev/null | grep -c 'WinGet/Links' || true)"
echo ALL_OK