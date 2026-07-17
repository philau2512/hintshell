#!/usr/bin/env bash
set -e
export PATH="/c/Users/ADMIN/.hintshell/bin:$PATH"
eval "$(hintshell hook bash 2>/dev/null)"
echo "type resolve: $(type -t _hintshell_resolve_fzf)"
# Capture all stderr+stdout from resolve — must not contain Permission denied
out="$(_hintshell_resolve_fzf 2>&1)" || true
echo "OUT=$out"
if printf '%s' "$out" | grep -qi 'Permission denied'; then
  echo FAIL_leaked_error
  exit 1
fi
case "$out" in
  */Packages/*) echo OK_packages_path ;;
  *) echo "OTHER: $out"; exit 1 ;;
esac
# Force try Links path (must not print Permission denied)
err="$(_hintshell_try_fzf '/c/Users/ADMIN/AppData/Local/Microsoft/WinGet/Links/fzf' 2>&1)" || true
echo "TRY_LINKS=$err"
if printf '%s' "$err" | grep -qi 'Permission denied'; then
  echo FAIL_links_leaked
  exit 1
fi
echo ALL_OK