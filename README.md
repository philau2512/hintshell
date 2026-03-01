# 🧠 HintShell — AI-Powered Terminal Auto-Suggestion Engine (Rust CLI)

[![NPM Version](https://img.shields.io/npm/v/hintshell?color=red&label=npm%20beta)](https://www.npmjs.com/package/hintshell)
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

## 🚀 Quick Start (Recommended)

Cài đặt cực kỳ đơn giản qua **NPM** (hỗ trợ Windows, Linux, macOS):

```bash
# 1. Cài đặt global
npm install -g hintshell

# 2. Khởi tạo (tự động cấu hình shell)
hintshell init

# 3. Khởi động lại terminal hoặc reload config
# PowerShell: . $PROFILE
# Bash/Zsh: source ~/.bashrc (hoặc ~/.zshrc)
```

### Build from Source

Nếu bạn muốn tự build từ mã nguồn:

```bash
git clone https://github.com/philau2512/hintshell.git
cd hintshell
cargo build --release
./target/release/hintshell init
```

---

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

## 📖 Usage

### PowerShell (Auto-suggest)

Gõ lệnh bình thường → gợi ý tự động hiện dưới dạng overlay:

```
PS> git ch
  → git checkout main (12x)  [1/30]
    git cherry-pick (3x)     [2/30]
```

- **↑/↓** — chọn gợi ý
- **Tab** — chấp nhận
- **Esc** — đóng
- **Enter** — thực thi lệnh hiện tại

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
hintshell status          # Xem trạng thái daemon
hintshell stop            # Dừng daemon
hintshell start           # Khởi động lại daemon
hintshell init            # Chạy lại bộ cài đặt
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
