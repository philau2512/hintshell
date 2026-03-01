# 🧠 HintShell — AI-Powered Terminal Auto-Suggestion Engine (Rust CLI)

[![Rust CLI](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Cross Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-blue.svg)]()
[![Shells](https://img.shields.io/badge/Shell-PowerShell%20%7C%20Bash%20%7C%20Zsh-4D4D4D.svg)]()
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](https://opensource.org/licenses/MIT)

**HintShell** is a cross-platform, fuzzy-matching command intelligence engine built in Rust. It learns your terminal habits to provide real-time auto-suggestions and boosts your command-line productivity.

*(Gợi ý lệnh terminal thông minh dựa trên lịch sử sử dụng của bạn. HintShell học từ mỗi lệnh bạn gõ, kết hợp fuzzy matching + frequency scoring để gợi ý chính xác nhất.)*

---

## ✨ Features

- 🔮 **Auto-suggest real-time** (PowerShell) — gợi ý hiện ngay khi gõ
- 🔍 **Tab → fzf picker** (Bash/Zsh) — nhấn Tab để chọn lệnh
- 📊 **Fuzzy matching** — gõ sai vẫn tìm được lệnh đúng
- 🧮 **Frequency scoring** — lệnh dùng nhiều xếp trên
- 🖥️ **Cross-platform** — Windows, macOS, Linux
- 🐚 **Multi-shell** — PowerShell, Bash, Zsh

## 🏗️ Architecture

```
┌─────────────┐     IPC      ┌──────────────────┐
│  hintshell   │◄────────────►│  hintshell-core   │
│  (CLI)       │  Named Pipe  │  (Daemon)         │
│              │  or UDS      │  SQLite + Fuzzy   │
└─────────────┘              └──────────────────┘
       ▲
       │ eval "$(hintshell hook bash)"
       │ Import-Module HintShellModule
       ▼
┌─────────────────────────┐
│  Shell Integration       │
│  PowerShell / Bash / Zsh │
└─────────────────────────┘
```

- **hintshell** — CLI client, gửi request qua IPC
- **hintshell-core** — Daemon chạy nền, quản lý SQLite + suggestion engine
- **IPC** — Named Pipe (Windows) / Unix Domain Socket (Linux/macOS)

---

## 🚀 Quick Start

### Prerequisites

| Platform | Requirements |
|----------|-------------|
| **Windows** | Rust toolchain, PowerShell 7+ |
| **Linux/macOS** | Rust toolchain, `build-essential`, `fzf` |

### 1. Build

```bash
git clone https://github.com/your-username/ShellMind.git
cd ShellMind
cargo build --release
```

Output: `target/release/hintshell` + `target/release/hintshell-core`

### 2. Install

```bash
./target/release/hintshell init
```

Lệnh `init` sẽ tự động:
- Copy binary vào `~/.hintshell/bin/`
- Copy PowerShell module vào `~/.hintshell/module/`
- Thêm hook vào shell config (`.bashrc`, `.zshrc`, hoặc PowerShell profile)

### 3. Restart shell

```bash
# Bash/Zsh
source ~/.bashrc   # hoặc source ~/.zshrc

# PowerShell
. $PROFILE
```

### 4. Verify

```bash
hintshell status
```

```
🧠 HintShell Daemon v0.1.0
   Commands in history: 42
   Uptime: 120s
```

---

## 📖 Usage

### PowerShell (Auto-suggest)

Gõ lệnh bình thường → gợi ý tự động hiện dưới dạng overlay:

```
PS> git ch
  → git checkout main (12x)
    git cherry-pick (3x)
```

- **↑/↓** — chọn gợi ý
- **Tab** — chấp nhận
- **Esc** — đóng

### Bash / Zsh (Tab → fzf)

Gõ lệnh → nhấn **Tab**:

```bash
$ docker  # nhấn Tab
🧠 HintShell>
  docker compose up -d
  docker ps -a
  docker build -t myapp .
```

- **Tab/Enter** — chọn
- **Esc** — hủy

### CLI Commands

```bash
hintshell start           # Khởi động daemon
hintshell stop            # Dừng daemon
hintshell status          # Xem trạng thái
hintshell suggest "git"   # Gợi ý thủ công
hintshell add -c "cmd"    # Thêm lệnh thủ công
hintshell hook bash       # In hook script (cho eval)
hintshell init            # Cài đặt tự động
```

---

## 🐧 Platform Notes

### Linux / WSL

```bash
# Cài dependencies
sudo apt install -y build-essential fzf

# Build & install
cargo build --release
./target/release/hintshell init
source ~/.bashrc
```

### macOS

```bash
brew install fzf
cargo build --release
./target/release/hintshell init
source ~/.zshrc
```

### Windows (PowerShell)

```powershell
cargo build --release
.\target\release\hintshell.exe init
. $PROFILE
```

---

## 📁 File Structure

```
~/.hintshell/
├── bin/
│   ├── hintshell          # CLI binary
│   └── hintshell-core     # Daemon binary
├── module/
│   └── HintShellModule/   # PowerShell module
└── history.db             # SQLite database
```

---

## 🛠️ Development

```bash
# Build debug
cargo build

# Run CLI trực tiếp
cargo run --bin hintshell -- status

# Run daemon trực tiếp
cargo run --bin hintshell-core

# Reload nhanh (PowerShell)
.\reload-hintshell.ps1
```

---

## 📄 License

MIT
