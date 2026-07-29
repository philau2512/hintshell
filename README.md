<p align="center">
  <img src="https://raw.githubusercontent.com/philau2512/hintshell/main/assets/logo.png" alt="HintShell Logo" width="120" />
</p>

<h1 align="center">HintShell</h1>
<p align="center"><strong>Next-Gen AI-Ready Real-time Command Suggestions for Your Terminal</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/hintshell"><img src="https://img.shields.io/npm/v/hintshell?color=CB3837&label=npm" alt="NPM Version" /></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Built_with-Rust-DEA584?logo=rust" alt="Rust" /></a>
  <a href="#"><img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-0078D4" alt="Platform" /></a>
  <a href="#"><img src="https://img.shields.io/badge/Shell-PowerShell%20%7C%20Bash%20%7C%20Zsh-4D4D4D" alt="Shells" /></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-green.svg" alt="MIT License" /></a>
</p>

<p align="center">
  HintShell is an <strong>AI-ready productivity engine</strong> that <strong>embeds into your existing shell</strong> (PowerShell, Bash, or Zsh). It provides <strong>context-aware command suggestions</strong> in real-time, drastically reducing context-switching and boosting developer workflow efficiency. Built with <strong>Rust</strong> for maximum performance and minimum footprint.
</p>

---

## ⚡ Why HintShell?

Most shells offer basic, single-line autocomplete. HintShell replaces that with a <strong>smart, interactive suggestion panel</strong> — a context-aware UI/UX upgrade right inside your terminal. It's the <strong>modern alternative to PSReadLine and zsh-autosuggestions</strong>.

| Feature | HintShell | PowerShell <br>(PSReadLine) | Zsh <br>(zsh-autosuggestions) | Bash | Git Bash | Fish |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| **Suggestion UI** | Scrollable list | Single inline ghost | Single inline ghost | None | None | Single inline ghost |
| **Prefix matching** | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
| **Frequency ranking** | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ |
| **Smart ranking** | ✅ Recent → Default → Most Used → Others | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Command descriptions** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Cross-shell** | ✅ | PowerShell only | Zsh only | Bash only | — | Fish only |
| **Learns from history** | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ |
| **Auto-start daemon** | ✅ | N/A | N/A | N/A | N/A | N/A |
| **600+ built-in commands** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Works with any terminal** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

---

## 🚀 Installation (Recommended)

Follow these steps in order to get HintShell running on your machine.

### 1. Install Dependencies (macOS / Linux / Git Bash)
HintShell uses `fzf` to render the suggestion picker on Bash/Zsh (including **Git Bash** on Windows).
- **macOS**: `brew install fzf`
- **Linux (Ubuntu/Debian)**: `sudo apt install fzf`
- **Windows (Git Bash)**: `winget install junegunn.fzf` (or install fzf another way). PowerShell overlay does **not** require fzf.
- **Windows PowerShell**: No extra dependencies for the real-time overlay.

### 2. Install HintShell
Install via NPM to get the latest pre-built binaries for your platform:
```bash
npm install -g hintshell@latest
```

### 3. Initialize Shell Integration
Run the init command to automatically configure your shell (`.zshrc`, `.bashrc`, or PowerShell profile):
```bash
hs init
```

### 4. Restart Terminal
Restart your terminal or reload your shell config to activate the hooks:
```bash
# Zsh
source ~/.zshrc

# Bash
source ~/.bashrc

# PowerShell
. $PROFILE
```

---

## 📖 Usage

### PowerShell (Windows/Unix)
**Real-time Overlay**: Suggestions appear automatically as a floating panel beneath your cursor as you type. 
- **↑ / ↓** : Navigate
- **Tab** : Accept
- **Esc** : Close

### Zsh / Bash (macOS/Linux/Git Bash)
**Tab-to-Suggest**: To avoid conflicts with `zsh-autosuggestions`, HintShell activates when you press **Tab**.
- **Type `git ` + Tab** : Opens a fuzzy picker with frequencies.
- **Enter** : Select and fill the command line.

### Git Bash live overlay (Windows preview)
Run Git Bash through the opt-in wrapper to render local HintShell suggestions while you type, without starting `fzf`:

```bash
hintshell bash
```

- **Tab** accepts a HintShell command suggestion only for command text. For paths such as `cd src/comp`, flags such as `git --ver`, or an empty overlay, it is forwarded to Bash completion.
- **Up / Down** navigates the visible HintShell list; **Esc** closes it; **Enter** executes through Git Bash.
- The wrapper preserves the existing `.bashrc` integration. Open Git Bash normally to use the legacy Tab/fzf flow.
- Use `HINTSHELL_BASH` to set an explicit `bash.exe` path, or `HINTSHELL_DISABLE_LIVE_OVERLAY=1` to refuse the wrapper in unsupported terminals.

> **Preview limitation:** the live wrapper currently targets ANSI-capable Windows Terminal and mintty. Full-screen terminal applications should be opened from a normal Git Bash session until their passthrough behavior is included in a later release.

> **Git Bash note:** HintShell resolves `fzf` to a real executable (e.g. under WinGet Packages). If you previously saw `Permission denied` on `WinGet/Links/fzf`, reinstall/update to **0.2.0+**, run `hs init` (or reload `~/.bashrc`), and open a new terminal.

---

## ✨ What's new in 0.2.1

- Daemon single-instance + IDE terminal reliability (no stacked `hintshell-core`, IPC health checks).
- Safe update while running: `npm i -g hintshell@latest` / `hs update` stop the daemon before replacing Windows binaries.
- Clearer start/status/update logs. See [CHANGELOG.md](./CHANGELOG.md).

## 🔄 Updating

While the daemon is running, Windows locks `hintshell-core.exe`. Use:

```bash
# Recommended
hs update

# Or raw npm — postinstall stops the daemon and runs init
npm i -g hintshell@latest
```

If you still hit a file lock (`os error 32` / "being used by another process"):

```bash
hs stop
# or: taskkill /F /IM hintshell-core.exe
npm i -g hintshell@latest
hintshell init
```

## 🗑️ Uninstallation

If you need to remove HintShell, it now comes with a clean uninstaller that handles everything for you:

```bash
# 1. Run the official uninstaller
hs uninstall

# 2. (Optional) Remove the NPM package
npm uninstall -g hintshell
```
*Note: `hs uninstall` stops the daemon, removes hook lines from your shell configs, and deletes binaries from `~/.hintshell/bin`, but keeps your history database (`history.db`) safe.*

---

## 🏗️ CLI Reference

```bash
hs status      # Check if the daemon is running and see stats
hs start       # Manually start the daemon
hs stop        # Stop the daemon
hs update      # Stop daemon, npm install -g, init, restart
hs uninstall   # Completely remove shell integration and binaries
```

---

## 🏗️ Architecture

HintShell is a **client-daemon** system. It does **not** replace your terminal or shell. It plugs in via a thin hook.

```
┌─────────────────────────────────┐
│  Your Terminal                  │
│  (Windows Terminal, iTerm2,     │
│   Alacritty, any terminal)      │
│                                 │
│  ┌───────────────────────────┐  │
│  │  Your Shell               │  │
│  │  (PowerShell / Bash / Zsh)│  │
│  │       ▲                   │  │
│  │       │ hook / module     │  │
│  │       ▼                   │  │
│  │  ┌─────────┐    IPC    ┌──────────────┐
│  │  │   hs    │◄─────────►│ hintshell    │
│  │  │  (CLI)  │ Named Pipe│ -core        │
│  │  └─────────┘  or UDS   │ (Daemon)     │
│  │                        │ SQLite+Fuzzy │
│  │                        └──────────────┘
│  └───────────────────────────┘  │
└─────────────────────────────────┘
```

---

## 🤝 Contributing & License

Contributions are welcome! Built with 🦀 Rust for speed and safety. 
Licensed under **MIT**.

<p align="left">
  <strong>Stop memorizing commands. Let HintShell remember for you.</strong>
</p>
