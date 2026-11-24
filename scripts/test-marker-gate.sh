#!/usr/bin/env bash
# Regression test for the marker protocol gate.
#
# The live overlay reads protocol markers out of the shell's own output: OSC 5522 payloads
# for the working directory, end-of-prompt, continuation and startup phase. A marker emitted
# by a shell nobody is pumping reaches the terminal literally, and the user sees garbage like
# "▲HINTSHELL_CWD:C:\Users\ADMIN▼" plus an overlay that flickers because it never receives a
# prompt event. So: only the wrapper's own shell may emit them.
#
# Markers must stay inside an escape sequence. A bare 0x1e byte is re-mapped by Windows
# ConPTY through the console codepage into the printable glyph "▲", which no wrapper can
# recognise; an OSC that a terminal does not implement is ignored instead of printed.
#
# Usage: bash scripts/test-marker-gate.sh [path-to-hintshell]
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${1:-}"
if [ -z "$bin" ]; then
  for candidate in "$repo/target/release/hintshell.exe" "$repo/target/release/hintshell" \
                   "$repo/target/debug/hintshell.exe" "$repo/target/debug/hintshell"; do
    [ -x "$candidate" ] && bin="$candidate" && break
  done
fi
[ -n "$bin" ] || { echo "FAIL: hintshell binary not found (run cargo build --release first)"; exit 1; }

hookfile="$(mktemp)"
trap 'rm -f "$hookfile"' EXIT
"$bin" hook bash > "$hookfile" 2>/dev/null

# cat -v renders ESC as ^[ and BEL as ^G, so the assertions below also prove the markers are
# escape-sequenced rather than plain printable text.
emit() { cat -v | tr -d '\n\r'; }
# `|| true` matters: in the quiet cases the marker function is deliberately undefined, and a
# 127 from the inner shell must not abort the script before the assertions get to judge it.
run() { bash -c "source '$hookfile'; $1" 2>/dev/null | emit || true; }

wrapped="$(HINTSHELL_LIVE_BASH=1 HINTSHELL_MARKER_TOKEN=999 run '_hintshell_emit_prompt')"
old_wrapper="$(HINTSHELL_LIVE_BASH=1 run '_hintshell_emit_prompt')"
nested="$(HINTSHELL_LIVE_BASH=1 HINTSHELL_MARKER_TOKEN=999 \
  run "bash -c \"source '$hookfile'; _hintshell_emit_prompt\"")"
resourced="$(HINTSHELL_LIVE_BASH=1 HINTSHELL_MARKER_TOKEN=999 run "source '$hookfile'; _hintshell_emit_prompt")"
unwrapped="$(run '_hintshell_emit_prompt')"
continuation="$(HINTSHELL_LIVE_BASH=1 HINTSHELL_MARKER_TOKEN=999 run 'printf %s "$PS2"')"
plain_continuation="$(HINTSHELL_LIVE_BASH=1 run 'printf %s "$PS2"')"

echo "wrapper's own shell   : [$wrapped]"
echo "old wrapper (no token): [$old_wrapper]"
echo "nested bash           : [$nested]"
echo "re-sourced shell      : [$resourced]"
echo "unwrapped bash        : [$unwrapped]"
echo "PS2 with token        : [$continuation]"
echo "PS2 without token     : [$plain_continuation]"

fail=0
expect_markers() { case "$2" in *'^[]5522;hintshell_cwd='*'^[]5522;hintshell_prompt^G'*) ;; *) echo "FAIL: $1 emitted no OSC markers"; fail=1;; esac; }
expect_quiet() { [ -z "$2" ] || { echo "FAIL: $1 emitted markers nobody filters"; fail=1; }; }

expect_markers "wrapper's own shell" "$wrapped"
expect_markers "re-sourced shell" "$resourced"
expect_quiet "old wrapper (no token)" "$old_wrapper"
expect_quiet "nested bash" "$nested"
expect_quiet "unwrapped bash" "$unwrapped"
case "$continuation" in *'^[]5522;hintshell_continuation^G'*) ;; *) echo "FAIL: no continuation marker with a token"; fail=1;; esac
expect_quiet "PS2 without token" "$plain_continuation"

[ "$fail" -eq 0 ] && echo "MARKER GATE OK"
exit "$fail"
