#!/usr/bin/env bash
# End-to-end marker check through a real pseudo-console.
#
# The unit tests cover the filters, but the defect this guards against lived one layer lower:
# Windows ConPTY re-maps bare control bytes through the console codepage (0x1e becomes the
# glyph "▲"), so a filter matching raw bytes can pass every unit test and still print marker
# garbage on a real terminal. Here the wrapper pumps a genuine session and we inspect the very
# bytes it would send to the screen.
#
# Usage: bash scripts/test-marker-conpty.sh [path-to-hintshell]
set -uo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${1:-}"
if [ -z "$bin" ]; then
  for candidate in "$repo/target/release/hintshell.exe" "$repo/target/release/hintshell" \
                   "$repo/target/debug/hintshell.exe" "$repo/target/debug/hintshell"; do
    [ -x "$candidate" ] && bin="$candidate" && break
  done
fi
[ -n "$bin" ] || { echo "FAIL: hintshell binary not found (run cargo build --release first)"; exit 1; }

out="$(mktemp)"; err="$(mktemp)"
trap 'rm -f "$out" "$err"' EXIT

# The wrapper exits when the piped session ends; a timeout keeps a hung run from blocking CI.
printf 'pwd\nexit\n' | timeout 30 "$bin" bash --login -i > "$out" 2> "$err" || true

fail=0
# Prove the pump actually rendered the session, otherwise the checks below are vacuous.
if [ "$(wc -c < "$out")" -lt 40 ]; then
  echo "FAIL: the pumped session produced no output, this test proves nothing"
  sed -n '1,5p' "$err"
  fail=1
fi

if LC_ALL=C grep -qa 'HINTSHELL' "$out"; then
  echo "FAIL: marker text reached the screen"
  LC_ALL=C grep -a -o '.\{0,40\}HINTSHELL.\{0,40\}' "$out" | head -2
  fail=1
fi

if LC_ALL=C grep -qa '5522' "$out"; then
  echo "FAIL: an OSC marker survived the filters"
  fail=1
fi

# 0x1e and 0x1f rendered through the console codepage arrive as these two glyphs.
if LC_ALL=C grep -qa $'\xe2\x96\xb2\|\xe2\x96\xbc' "$out"; then
  echo "FAIL: a control byte was re-mapped into a ▲/▼ glyph on the screen stream"
  fail=1
fi

[ "$fail" -eq 0 ] && echo "CONPTY MARKER STREAM OK"
exit "$fail"
