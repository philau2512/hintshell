<h1 align="center">HintShell</h1>
<p align="center"><strong>Real-time Command Auto-Suggestion Engine for Your Terminal</strong></p>

<p align="center">
  <a href="https://www.npmjs.com/package/hintshell"><img src="https://img.shields.io/npm/v/hintshell?color=CB3837&label=npm" alt="NPM Version" /></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Built_with-Rust-DEA584?logo=rust" alt="Rust" /></a>
  <a href="#"><img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-0078D4" alt="Platform" /></a>
  <a href="#"><img src="https://img.shields.io/badge/Shell-PowerShell%20%7C%20Bash%20%7C%20Zsh-4D4D4D" alt="Shells" /></a>
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-green.svg" alt="MIT License" /></a>
</p>

<p align="center">
  HintShell is <strong>not</strong> a terminal emulator. It's a lightweight engine that <strong>embeds into your existing shell</strong> — PowerShell, Bash, or Zsh — and provides real-time command suggestions as you type. Think of it like autocomplete on steroids: a scrollable suggestion list with fuzzy matching, frequency ranking, and inline command descriptions.
</p>

---

## ⚡ Why HintShell?

Most shells offer basic, single-line autocomplete. HintShell replaces that with a **rich, interactive suggestion panel** — right inside your terminal.

| Feature | HintShell | PowerShell <br>(PSReadLine) | Zsh <br>(zsh-autosuggestions) | Bash | Git Bash | Fish |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| **Suggestion UI** | Scrollable list | Single inline ghost | Single inline ghost | None | None | Single inline ghost |
| **Prefix matching** | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
| **Frequency ranking** | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ |
| **Command descriptions** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Cross-shell** | ✅ | PowerShell only | Zsh only | Bash only | — | Fish only |
| **Learns from history** | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ |
| **600+ built-in commands** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **Works with any terminal** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

> **TL;DR** — Other shells give you one dim ghost suggestion. HintShell gives you a full list, ranked by how often you use each command, with a description of what every command does.

---

## 📸 How It Looks

```
PS> git ch
  ┌───────────────────────────────────────────┐
  │ > git checkout main          (12x)  [1/30]│
  │   git cherry-pick             (3x)  [2/30]│
  │   git checkout -b             (2x)  [3/30]│
  │   git checkout --             (1x)  [4/30]│
  │   git cherry-pick --abort     (1x)  [5/30]│
  │ 💡 Switch branches or restore working tree│
  └───────────────────────────────────────────┘
```

- **↑ / ↓** — Navigate the list
- **Tab** — Accept the selected suggestion
- **Enter** — Execute the current command line
- **Esc** — Dismiss the panel

---

## 🚀 Installation

### Option A — via NPM (Recommended)

Works on **Windows**, **macOS**, and **Linux**. Automatically downloads the correct binary for your platform.

```bash
npm install -g hintshell
hintshell init
```

Then **restart your terminal** (or reload your shell config):

```bash
# PowerShell
. $PROFILE

# Bash
source ~/.bashrc

# Zsh
source ~/.zshrc
```

### Option B — Download from GitHub Releases

Go to [**Releases**](https://github.com/philau2512/hintshell/releases), download the archive for your OS, extract it, and run:

```bash
hintshell init
```

### Option C — Build from Source

Requires [Rust](https://rustup.rs/).

```bash
git clone https://github.com/philau2512/hintshell.git
cd hintshell
cargo build --release
./target/release/hintshell init    # Linux / macOS
.\target\release\hintshell.exe init  # Windows
```

---

## 🗑️ Uninstall

### If installed via NPM

```bash
npm uninstall -g hintshell
```

### Complete cleanup (all platforms)

Remove the HintShell data directory and any shell configuration lines it added:

**Windows (PowerShell):**
```powershell
# 1. Stop the daemon
hs stop

# 2. Remove data directory
Remove-Item -Recurse -Force "$HOME\.hintshell"

# 3. Edit your PowerShell profile and remove the HintShell lines
notepad $PROFILE
# Delete lines related to "HintShell" or "Import-Module HintShellModule"
```

**macOS / Linux:**
```bash
# 1. Stop the daemon
hs stop

# 2. Remove data directory
rm -rf ~/.hintshell

# 3. Remove the hook line from your shell config
# For Bash: edit ~/.bashrc
# For Zsh:  edit ~/.zshrc
# Delete the line: eval "$(hintshell hook bash)"  (or zsh)
```

---

## 📖 Usage

### CLI Commands

```bash
hs start       # Start the HintShell daemon
hs stop        # Stop the daemon
hs status      # Check daemon status
hs init        # Set up shell integration (run once)
```

### PowerShell — Auto-Suggest Overlay

Just type — suggestions appear automatically as a floating panel beneath your cursor.

### Bash / Zsh — Tab to Suggest

Type a partial command, then press **Tab** to open the suggestion picker (requires `fzf`).

```bash
$ docker ↹
🧠 HintShell>
  docker compose up -d
  docker ps -a
  docker build -t myapp .
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

| Component | Role |
|---|---|
| `hs` / `hintshell` | CLI client — sends queries over IPC |
| `hintshell-core` | Background daemon — stores history, runs the suggestion engine (SQLite + fuzzy matching) |
| Shell Hook | Thin integration layer — captures keystrokes and renders the suggestion UI |

**IPC:** Named Pipe on Windows, Unix Domain Socket on Linux/macOS.

---

## 📁 File Structure

```
~/.hintshell/
├── bin/
│   ├── hintshell        # CLI binary
│   ├── hintshell-core   # Daemon binary
│   └── hs               # CLI alias
├── module/
│   └── HintShellModule/ # PowerShell module
├── default-commands.json # 600+ built-in commands with descriptions
└── history.db           # SQLite — your command history & frequencies
```

---

## ✨ Key Features

- **🔍 Prefix Matching** — Type `git` and instantly see all commands starting with `git`, ranked by your usage frequency.
- **📊 Frequency Ranking** — Commands you use most float to the top.
- **💡 Command Descriptions** — See what each command does before you run it. 600+ commands pre-loaded across Git, Docker, NPM, Python, Kubernetes, Terraform, Cargo, and more.
- **🖥️ Cross-Platform** — Windows, macOS, Linux. One tool, every OS.
- **🐚 Multi-Shell** — PowerShell, Bash, Zsh. Same experience everywhere.
- **⚡ Built in Rust** — Fast startup, tiny memory footprint, no runtime dependencies.
- **🔒 Local & Private** — Everything stays on your machine. No cloud, no telemetry.

---

## 🛠️ Development

```bash
# Debug build
cargo build

# Run CLI directly
cargo run --bin hintshell -- status

# Run daemon directly
cargo run --bin hintshell-core

# Quick reload (PowerShell)
.\reload-hintshell.ps1
```

---

## 🤝 Contributing

Contributions are welcome! Feel free to open issues or submit pull requests.

## 📄 License

[MIT](LICENSE)

---

<p align="center">
  <strong>Stop memorizing commands. Let HintShell remember for you.</strong>
</p>
