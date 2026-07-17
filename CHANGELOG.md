# Changelog

All notable changes to HintShell will be documented in this file.

## [0.2.0] - 2026-07-18

### ✨ Improvements
- **PowerShell overlay layout**: Compact mode on narrow terminals (≤3 items, short frequency labels, no emoji footer) to prevent line wrap and residual ghost text.
- **Display-width aware rendering**: Truncate/pad suggestion rows by terminal display columns; clear uses window-relative available rows.
- **Bash/Zsh fzf resolver**: Resolve a working `fzf` binary on Git Bash/MSYS — prefer real WinGet package path over non-executable `WinGet/Links` shims.
- **Shell init hardening**: `.bashrc` / `.zshrc` install block uses POSIX paths and absolute `~/.hintshell/bin/hintshell[.exe]` so npm global never shadows hook generation.
- **fzf accept accuracy**: Suggestion format no longer truncates the command string to 60 characters before Tab accept.

### 🐛 Bug Fixes
- **Multi-line buffer (PowerShell)**: Suppress suggestion overlay when the edit buffer contains newlines so the panel does not paint over `>>` continuation lines.
- **Git Bash `Permission denied` on fzf**: Fixed Tab-to-suggest failing because MSYS cannot exec WinGet Links reparse-point shims.
- **Hook not loading after `source ~/.bashrc`**: Fixed path/order issues that caused old npm `hintshell` to be used for `hook bash` instead of the installed binary.

### 🔧 Internal
- Shared `_hintshell_try_fzf` / `_hintshell_resolve_fzf` helpers for Bash and Zsh hooks.
- Fallback when fzf is unavailable: accept top suggestion silently (no Permission denied noise).
- Multi-line skip for Bash/Zsh Tab picker (same product decision as PowerShell).
- Helper scripts: `scripts/test-overlay-layout.ps1`, `scripts/test-fzf-resolve-bash.sh`.

## [0.1.8] - 2026-07-02

### ✨ Bug Fixes
- **Update Flow Fix**: Automatically stop running daemon during `hintshell init` to prevent Windows File Lock errors (`os error 32`) and Unix stale background processes during updates.
- **Permissions Fix**: Fix Unix socket connection failure (`os error 2`) on macOS and WSL by automatically setting executable permission (`chmod +x`) on copied binaries.
- **Visual Clean**: Comment out welcome suggestions guides on shell startup to keep output clean.
- **Workflow Release**: Configure release workflow to publish production release rather than pre-release.

## [0.1.5] - 2026-03-16

### ✨ Bug Fixes
- **Update Lock Fix**: Fixed "os error 32" on Windows by stopping the daemon before updating assets.
- **Stable Update**: Removed @beta tag from default update command to ensure stability.

## [0.1.4] - 2026-03-16

### ✨ Improvements
- **Persistent Recent Commands**: Fixed an issue where the `recent` command would disappear or get buried too quickly.
- **Direct DB Recent Match**: Implementation of direct database query for the single most recent matching command to ensure high reliability.
- **Balanced Ranking**: Rebalanced weights between recency (40%) and frequency (35%) to keep recently used commands at the top longer.
- **Smooth Decaying**: Implemented a minute-based smoothing decay function for recency scores, preventing suggestions from "expiring" prematurely.

## [0.1.3] - 2026-03-16

### ✨ New Features
- **Multi-pass Ranking**: Smart suggestion ordering with 4-tier priority system:
  1. **Recent** — The most recently used command (within 30 min) appears first with `(recent)` tag
  2. **Default** — Built-in commands from the 600+ command library
  3. **Most Used** — The top frequently used command with `(most use)` tag
  4. **Others** — All remaining matching commands sorted by relevance
- **Source tracking**: Database now tracks command origin (`user` vs `default`) via `source` column
- **Visual tier tags**: Suggestion overlay displays `(recent)` and `(most use)` labels alongside frequency count

### 🐛 Bug Fixes
- **Bash/Zsh auto-start**: Daemon now auto-starts when opening a new terminal session on macOS/Linux — no more manual `hs start` required
- **Overlay sort override**: Fixed PowerShell overlay re-sorting suggestions by frequency, which was overriding the server's multi-pass ranking order
- **Default command seeding**: Default commands are now inserted with a historical timestamp (`2000-01-01`) and `frequency=0` to prevent them from flooding the "recent" tier on first launch

### 🔧 Internal
- Added `source` column to SQLite `history` table with automatic migration for existing databases
- Added 5 new unit tests for multi-pass ranking logic and deduplication
- Updated `SuggestionItem` protocol to include `source` field

## [0.1.2] - 2026-03-13

- Initial public release
- Real-time suggestion overlay for PowerShell
- Tab-to-fzf integration for Bash/Zsh
- 600+ built-in default commands
- Frequency-based ranking
- Cross-platform support (Windows, macOS, Linux)
