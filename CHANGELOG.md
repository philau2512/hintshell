# Changelog

All notable changes to HintShell will be documented in this file.

## [0.3.4] - 2026-07-30

### New Features
- **macOS Bash/Zsh live overlay**: `hs init` enables the realtime Unix PTY-backed overlay for interactive macOS Bash and Zsh by default; no environment-variable opt-in or manual shell-profile edits are required.

### Improvements
- **Bash login-shell setup**: On macOS, `hs init` manages a dedicated block in `~/.bash_profile` that loads `~/.bashrc`, allowing the Bash overlay to start reliably in Terminal and iTerm2.
- **Live shell CWD synchronization**: Unix live children preserve their opening directory and emit an internal prompt marker after directory changes, keeping contextual suggestions aligned with `cd` without changing the daemon protocol.
- **Shell-specific requests**: The shared live runtime identifies Bash and Zsh independently in suggestion requests while keeping Git Bash on ConPTY and WSL2 Bash on its Unix PTY policy.
- **Clean uninstall**: `hs uninstall` removes the managed macOS Bash login block together with the HintShell Bash hook.

### Compatibility
- **Per-session escape hatches**: `HINTSHELL_DISABLE_AUTO_BASH=1 bash` and `HINTSHELL_DISABLE_AUTO_ZSH=1 zsh` bypass the live wrapper for one session.
- **Native completion authority**: The live overlay continues to defer path and flag completion to Bash/Zsh.
- **Linux behavior**: Linux Bash and Zsh remain on the native Tab/fzf integration outside WSL2.

## [0.3.2] - 2026-03-14

### New Features
- **WSL2 Bash live overlay**: Interactive WSL2 Bash sessions now use a Unix pseudo-terminal backend for the same realtime command overlay and Bash-native path/flag completion policy as Git Bash.

### Bug Fixes
- **WSL workspace directory**: The Unix PTY child now explicitly inherits the parent terminal's current directory, so a WSL terminal opened at a workspace remains there after the live wrapper starts.

### Compatibility
- **Git Bash isolation**: Git Bash remains on its existing Windows ConPTY backend.
- **PowerShell isolation**: PowerShell module, predictor, handlers, protocol, and overlay behavior are unchanged.
- **Native Unix fallback**: Linux/macOS Bash continue using the existing Tab/fzf picker outside WSL2.

## [0.3.1] - 2026-03-14

### 🐛 Bug Fixes
- **Cross-platform release builds**: Compile the Git Bash live overlay only on Windows, with a clear unsupported-platform response elsewhere. This restores Linux and macOS release builds.
- **Unix warning cleanup**: Gate Windows-only tracing imports and preserve the daemon lock-file contents when opening it on Unix.

## [0.3.0] - 2026-03-14

### ✨ New Features
- **Context-aware suggestions**: The suggestion protocol now carries the current directory and shell. HintShell merges local/global history with bounded candidates for paths, Git branches/remotes, npm scripts, Docker, SSH hosts, and zoxide directories.
- **Local history ranking**: Commands used in the active directory receive a bounded boost without displacing stronger prefix matches or relevant global history.
- **Git Bash live overlay auto-start**: `hs init` now configures normal interactive Git Bash sessions to launch `hintshell bash` automatically. Set `HINTSHELL_DISABLE_AUTO_BASH=1` to bypass it for one session.

### ✨ Improvements
- **Git Bash completion policy**: The live wrapper accepts command-prefix suggestions on Tab and defers path and flag input to native Bash completion.
- **Git Bash overlay viewport**: Up/Down selection scrolls a six-row viewport for longer result sets.
- **Safe contextual I/O**: Filesystem, workspace, and external-process generators use bounded output, deadlines, caches, and fail-closed behavior so suggestion requests do not block terminal input.
- **PowerShell installation refresh**: `hs init` replaces an existing HintShell integration block and updates the installed module asset instead of retaining an older module.

### 🐛 Bug Fixes
- **Overlay at terminal bottom**: Live Git Bash now caps visible rows to the space below the prompt, and hides the frame when even a minimal overlay would scroll the terminal. This prevents stale or duplicated render artifacts.
- **Git Bash path resolution**: Contextual filesystem lookup converts MSYS `/c/...` paths before filesystem access and keeps the user's slash style in displayed suggestions.

## [0.2.1] - 2026-07-18

### 🐛 Bug Fixes
- **Daemon single-instance (Windows)**: Named mutex + pipe first-instance so IDE multi-terminal races no longer stack `hintshell-core` processes.
- **IDE offline / “Daemon is not running”**: Start/status verify real IPC health; prefer `~/.hintshell/bin` over stale `module/` binaries; profile start uses a file lock and waits before killing orphans.
- **npm update while daemon running**: `postinstall` stops the daemon (IPC + force kill) before extracting binaries and runs `hintshell init` so `npm i -g hintshell@latest` can overwrite locked `.exe` files.
- **`hs update` / CLI `update`**: Real upgrade path (stop → npm install → init/start), not version-check only; PowerShell update no longer writes `.disabled`.
- **`hintshell init` file lock**: Force-kill before asset copy; skip self-overwrite of the running CLI on Windows; resolve module from `~/.hintshell/module` when re-init from `bin/`.

### ✨ Improvements
- Clearer logs on `hs start` / `hs stop` / `hs status` / `hs update` (success vs failure).
- SQLite `busy_timeout` + WAL to reduce multi-process DB contention.

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
