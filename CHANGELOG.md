# Changelog

All notable changes to HintShell will be documented in this file.

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
