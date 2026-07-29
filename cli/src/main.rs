#[cfg(windows)]
use std::os::windows::process::CommandExt;

use clap::{Parser, Subcommand};
use hintshell_core::api::protocol::{HintShellRequest, HintShellResponse};
use std::process::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

mod live_bash;
mod shell;

#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;

#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(windows)]
const PIPE_NAME: &str = r"\\.\pipe\hintshell";

#[cfg(unix)]
const SOCKET_PATH: &str = "/tmp/hintshell.sock";

#[derive(Parser)]
#[command(
    name = "hintshell",
    about = "🧠 HintShell - Personal Command Intelligence Engine",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the HintShell daemon in background
    Start,

    /// Stop the running daemon
    Stop,

    /// Show daemon status
    Status,

    /// Get command suggestions for a partial input
    Suggest {
        /// The partial command input
        input: String,

        /// Maximum number of suggestions
        #[arg(short, long, default_value = "5")]
        limit: usize,

        /// Output format: 'human' (default) or 'plain' (command names only, for scripts/fzf)
        #[arg(short, long, default_value = "human")]
        format: String,
    },

    /// Manually add a command to history
    Add {
        /// The command to record
        #[arg(short, long)]
        command: String,

        /// Current directory
        #[arg(short, long)]
        directory: Option<String>,

        /// Shell type (powershell, cmd, bash)
        #[arg(short, long)]
        shell: Option<String>,
    },

    /// Run Git Bash with HintShell's live advisory overlay
    Bash {
        /// Pass arguments directly to Git Bash
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Initialize HintShell for all detected shells
    Init,

    /// Output shell hook code
    Hook {
        /// Shell type (bash, zsh, fish, powershell)
        shell: String,
    },

    /// Uninstall HintShell from the system
    Uninstall,

    /// Update HintShell to the latest version
    Update,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Start => {
            println!("▶ hintshell start");
            start_daemon();
        }
        Commands::Stop => {
            println!("▶ hintshell stop");
            let request = HintShellRequest::Shutdown;
            match send_request(&request).await {
                Ok(resp) => {
                    if resp.success {
                        println!("✅ HintShell daemon stopped via IPC.");
                    } else {
                        println!("❌ Error: {}", resp.error.unwrap_or_default());
                    }
                }
                Err(e) => {
                    println!("ℹ️  IPC stop failed (daemon may already be dead): {}", e);
                }
            }
            // Always sweep orphan processes so IDE/profile races cannot leave zombies
            let killed = kill_orphan_daemons();
            if killed > 0 {
                println!("🧹 Stopped {} leftover hintshell-core process(es).", killed);
            } else {
                println!("   No leftover hintshell-core process(es).");
            }
            println!("✅ hintshell stop complete.");
        }
        Commands::Status => {
            println!("▶ hintshell status");
            let request = HintShellRequest::Status;
            let orphans = count_daemon_processes();
            match send_request(&request).await {
                Ok(resp) => {
                    if let Some(status) = resp.status {
                        println!("✅ Daemon is running");
                        println!("🧠 HintShell Daemon v{}", status.version);
                        println!("   Commands in history: {}", status.total_commands);
                        println!("   Uptime: {}s", status.uptime_seconds);
                        println!("   Pipe/socket: OK");
                        println!("   Processes hintshell-core: {}", orphans);

                        // Check for updates from npm registry
                        check_npm_update(&status.version);
                    } else {
                        println!("⚠️  Daemon responded but status payload was empty.");
                    }
                }
                Err(e) => {
                    println!("❌ Daemon is not running (IPC failed).");
                    println!("   Detail: {}", e);
                    println!("   Processes hintshell-core: {}", orphans);
                    if orphans > 0 {
                        println!(
                            "   Found {} process(es) that are NOT answering IPC (stale).",
                            orphans
                        );
                        println!("   Run: hs start   (will kill orphans and restart)");
                    } else {
                        println!("   Run: hs start");
                    }
                }
            }
        }
        Commands::Suggest { input, limit, format } => {
            let request = HintShellRequest::Suggest { input, limit };
            match send_request(&request).await {
                Ok(resp) => {
                    if let Some(suggestions) = resp.suggestions {
                        if format == "plain" {
                            // Plain: one command per line (for scripts)
                            for s in &suggestions {
                                println!("{}", s.command);
                            }
                        } else if format == "fzf" {
                            // FZF: full command + frequency (do not truncate command — accept must be exact)
                            for s in &suggestions {
                                let cmd = s.command.replace('\t', " ").replace('\n', " ");
                                println!("{}\t({}x)", cmd, s.frequency);
                            }
                        } else {
                            // Human readable
                            if suggestions.is_empty() {
                                println!("(no suggestions)");
                            } else {
                                for (i, s) in suggestions.iter().enumerate() {
                                    println!("  {} {} ({}x)",
                                        if i == 0 { "→" } else { " " },
                                        s.command,
                                        s.frequency
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    if format != "plain" {
                        println!("❌ Cannot connect to daemon: {}", e);
                    }
                }
            }
        }
        Commands::Add {
            command,
            directory,
            shell,
        } => {
            let request = HintShellRequest::AddCommand {
                command,
                directory,
                shell,
            };
            match send_request(&request).await {
                Ok(resp) => {
                    if resp.success {
                        println!("✅ Command recorded.");
                    } else {
                        println!("❌ Error: {}", resp.error.unwrap_or_default());
                    }
                }
                Err(e) => println!("❌ Cannot connect to daemon: {}", e),
            }
        }
        Commands::Bash { args } => {
            if let Err(error) = live_bash::run(args) {
                eprintln!("HintShell Bash: {error}");
                std::process::exit(1);
            }
        }
        Commands::Init => {
            println!("▶ hintshell init");
            // Stop + force-kill so Windows can overwrite ~/.hintshell/*.exe (os error 32)
            println!("🛑 Stopping daemon to release file locks...");
            let shutdown_request = HintShellRequest::Shutdown;
            match send_request(&shutdown_request).await {
                Ok(_) => println!("   IPC shutdown: OK"),
                Err(e) => println!("   IPC shutdown: {} (will force-kill)", e),
            }
            // Always force-kill after IPC (handles slow exit + zombies holding .exe locks)
            let killed = kill_orphan_daemons();
            if killed > 0 {
                println!("   Force-stopped {} hintshell-core process(es).", killed);
            } else {
                // taskkill even when count was 0 (race: process exiting)
                let _ = kill_orphan_daemons();
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            println!("🔍 Detecting shells...");
            let shells = shell::detect_shells();
            if shells.is_empty() {
                println!("⚠️ No supported shells detected.");
                return;
            }

            let bin_path = std::env::current_exe().unwrap_or_else(|_| "hintshell".into());

            // Install binaries & module into ~/.hintshell/
            print!("📦 Installing assets to ~/.hintshell/... ");
            match shell::install_assets(&bin_path) {
                Ok(_) => println!("✅"),
                Err(e) => {
                    // One more kill+retry for stubborn Windows locks
                    println!("⚠️ {}", e);
                    println!("   Retry after force-kill...");
                    let _ = kill_orphan_daemons();
                    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
                    match shell::install_assets(&bin_path) {
                        Ok(_) => println!("✅ (retry OK)"),
                        Err(e2) => println!("❌ {}", e2),
                    }
                }
            }

            for s in shells {
                let name = s.name().to_string();
                match s.install(&bin_path) {
                    Ok(_) => println!("✅ {} → config updated", name),
                    Err(e) => println!("ℹ️  {} → {}", name, e),
                }
            }

            // Auto-start daemon after init
            println!("\n🚀 Starting daemon...");
            start_daemon();
            println!("✅ hintshell init complete. Restart shell or re-import module if needed.");
        }
        Commands::Hook { shell } => {
            let s = shell::Shell::from_str(&shell).expect("Unsupported shell");
            print!("{}", s.get_hook());
        }
        Commands::Uninstall => {
            println!("🗑 Uninstalling HintShell...");

            // 1. Stop daemon (IPC + force kill for Windows file locks)
            let request = HintShellRequest::Shutdown;
            let _ = send_request(&request).await;
            let killed = kill_orphan_daemons();
            if killed > 0 {
                println!("🧹 Force-stopped {} hintshell-core process(es).", killed);
            }
            println!("✅ Daemon stopped.");

            // 2. Remove hooks from all shells
            let shells = shell::detect_shells();
            for s in shells {
                match s.uninstall() {
                    Ok(_) => println!("✅ Cleared {} config.", s.name()),
                    Err(e) => println!("⚠️ Failed to clear {} config: {}", s.name(), e),
                }
            }

            // 3. Remove assets
            match shell::uninstall_assets() {
                Ok(_) => println!("✅ Binaries removed."),
                Err(e) => println!("⚠️ Failed to remove binaries: {}", e),
            }

            println!("\n✨ HintShell uninstalled successfully.");
            println!("👉 Please restart your terminal or source your shell config to complete the process.");
        }
        Commands::Update => {
            // Real update path (not just version check).
            // PowerShell `hs update` also stops + npm + init; CLI must work the same
            // when invoked via npm wrapper / bash / outside the PS module.
            println!("▶ hintshell update");
            println!("🛑 Stopping daemon (release file locks)...");
            let _ = send_request(&HintShellRequest::Shutdown).await;
            let killed = kill_orphan_daemons();
            if killed > 0 {
                println!("   Stopped {} process(es).", killed);
            }

            println!("🔄 Running: npm install -g hintshell@latest");
            let npm_status = Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
                .args(["install", "-g", "hintshell@latest"])
                .status();
            match npm_status {
                Ok(st) if st.success() => {
                    println!("✅ npm install finished.");
                    // postinstall already runs init when HINTSHELL_SKIP_INIT is unset.
                    // Still ensure local assets if user skipped init in env.
                    if std::env::var("HINTSHELL_SKIP_INIT").is_ok() {
                        println!("📦 HINTSHELL_SKIP_INIT set — running init now...");
                        let bin_path =
                            std::env::current_exe().unwrap_or_else(|_| "hintshell".into());
                        let _ = kill_orphan_daemons();
                        match shell::install_assets(&bin_path) {
                            Ok(_) => println!("✅ Assets installed."),
                            Err(e) => println!("⚠️ Assets: {}", e),
                        }
                    }
                    println!("🚀 Ensuring daemon is up...");
                    start_daemon();
                    println!("✅ hintshell update complete.");
                    check_npm_update(env!("CARGO_PKG_VERSION"));
                }
                Ok(st) => {
                    println!("❌ npm install failed (exit {}).", st.code().unwrap_or(-1));
                    println!("   Try: hs stop ; npm i -g hintshell@latest ; hintshell init");
                }
                Err(e) => {
                    println!("❌ Cannot run npm: {}", e);
                    println!("   Is Node.js/npm on PATH?");
                    // Fall back to version check so command is still useful offline
                    check_npm_update(env!("CARGO_PKG_VERSION"));
                }
            }
        }
    }
}

fn start_daemon() {
    println!("   Checking existing daemon via IPC...");
    // 1) Already healthy?
    if probe_daemon_alive() {
        println!("✅ HintShell daemon is already running and healthy.");
        return;
    }

    // 2) If processes exist but IPC is dead: wait briefly (slow IDE cold start), then kill
    let orphans = count_daemon_processes();
    if orphans > 0 {
        println!(
            "🧹 Found {} hintshell-core process(es) not answering IPC; waiting briefly...",
            orphans
        );
        for i in 1..=8 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if probe_daemon_alive() {
                println!(
                    "✅ Daemon became healthy while waiting (~{}ms).",
                    i * 200
                );
                return;
            }
        }
        println!("   Still unhealthy — stopping stale process(es)...");
        let killed = kill_orphan_daemons();
        println!("   Killed {} process(es).", killed);
        std::thread::sleep(std::time::Duration::from_millis(400));
    }

    let exe_path = resolve_core_path();
    if !exe_path.exists() {
        println!("❌ Cannot find hintshell-core at {}", exe_path.display());
        println!("   Make sure HintShell is installed (hs init) or built: cargo build --release");
        return;
    }

    println!("🚀 Starting daemon: {}", exe_path.display());

    let mut cmd = Command::new(&exe_path);

    // Redirect stdout/stderr to null so daemon logs don't pollute the terminal
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    #[cfg(windows)]
    cmd.creation_flags(0x00000008); // DETACHED_PROCESS

    match cmd.spawn() {
        Ok(child) => {
            println!("   Spawned PID {}", child.id());
            // 3) Wait until IPC answers
            let mut ok = false;
            for i in 1..=20 {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if probe_daemon_alive() {
                    ok = true;
                    println!(
                        "✅ HintShell daemon started successfully (ready after ~{}ms).",
                        i * 200
                    );
                    break;
                }
            }
            if !ok {
                let left = count_daemon_processes();
                println!("❌ Daemon process started but IPC is not ready.");
                println!("   Processes still present: {}", left);
                println!("   Pipe: \\\\.\\pipe\\hintshell");
                println!("   Tip: run the core binary in a console to see logs:");
                println!("   {}", exe_path.display());
            }
        }
        Err(e) => println!("❌ Failed to start daemon: {}", e),
    }
}

/// Resolve hintshell-core binary: sibling of CLI → ~/.hintshell/bin → ~/.hintshell/module
fn resolve_core_path() -> std::path::PathBuf {
    let core_name = if cfg!(windows) {
        "hintshell-core.exe"
    } else {
        "hintshell-core"
    };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let sibling = parent.join(core_name);
            if sibling.exists() {
                return sibling;
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        let bin = home.join(".hintshell").join("bin").join(core_name);
        if bin.exists() {
            return bin;
        }
        let module = home.join(".hintshell").join("module").join(core_name);
        if module.exists() {
            return module;
        }
        return bin;
    }

    core_name.into()
}

/// True if status IPC succeeds.
/// Runs on a dedicated thread + runtime to avoid deadlocking #[tokio::main].
fn probe_daemon_alive() -> bool {
    let handle = std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match rt {
            Ok(rt) => rt.block_on(async {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(1200),
                    send_request(&HintShellRequest::Status),
                )
                .await
                {
                    Ok(Ok(_)) => true,
                    _ => false,
                }
            }),
            Err(_) => false,
        }
    });
    handle.join().unwrap_or(false)
}

fn count_daemon_processes() -> usize {
    #[cfg(windows)]
    {
        // Prefer PowerShell — more reliable than tasklist locale/encoding quirks.
        let ps = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "(Get-Process -Name 'hintshell-core' -ErrorAction SilentlyContinue | Measure-Object).Count",
            ])
            .output();
        if let Ok(o) = ps {
            if let Ok(s) = String::from_utf8(o.stdout) {
                if let Ok(n) = s.trim().parse::<usize>() {
                    return n;
                }
            }
        }
        let output = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq hintshell-core.exe", "/NH"])
            .output();
        match output {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.lines()
                    .filter(|l| l.to_ascii_lowercase().contains("hintshell-core.exe"))
                    .count()
            }
            Err(_) => 0,
        }
    }
    #[cfg(not(windows))]
    {
        let output = Command::new("pgrep").args(["-f", "hintshell-core"]).output();
        match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count(),
            Err(_) => 0,
        }
    }
}

/// Force-kill all daemon processes. Returns how many were reported killed (best-effort).
fn kill_orphan_daemons() -> usize {
    let before = count_daemon_processes();
    if before == 0 {
        return 0;
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "hintshell-core.exe"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        // Fallback if taskkill is blocked/partial
        let _ = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-Process -Name 'hintshell-core' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("pkill")
            .args(["-f", "hintshell-core"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    let after = count_daemon_processes();
    if after == 0 {
        before
    } else {
        before.saturating_sub(after)
    }
}

#[cfg(windows)]
pub(crate) async fn send_request(request: &HintShellRequest) -> Result<HintShellResponse, String> {
    let pipe = ClientOptions::new()
        .open(PIPE_NAME)
        .map_err(|e| format!("Cannot connect to HintShell daemon: {}", e))?;

    handle_ipc(pipe, request).await
}

#[cfg(unix)]
pub(crate) async fn send_request(request: &HintShellRequest) -> Result<HintShellResponse, String> {
    let stream = UnixStream::connect(SOCKET_PATH)
        .await
        .map_err(|e| format!("Cannot connect to HintShell socket: {}", e))?;

    handle_ipc(stream, request).await
}

async fn handle_ipc<S>(stream: S, request: &HintShellRequest) -> Result<HintShellResponse, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    let mut json = serde_json::to_string(request).map_err(|e| e.to_string())?;
    json.push('\n');

    writer
        .write_all(json.as_bytes())
        .await
        .map_err(|e| format!("Write failed: {}", e))?;
    writer
        .flush()
        .await
        .map_err(|e| format!("Flush failed: {}", e))?;

    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .await
        .map_err(|e| format!("Read failed: {}", e))?;

    serde_json::from_str(&response_line).map_err(|e| format!("Invalid response: {}", e))
}

fn check_npm_update(local_version: &str) {
    // Quick check — timeout after 2 seconds
    let resp = ureq::get("https://registry.npmjs.org/hintshell")
        .timeout(std::time::Duration::from_secs(2))
        .call();

    if let Ok(resp) = resp {
        if let Ok(body) = resp.into_string() {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                // Check latest tag (stable) from npm
                let latest = json["dist-tags"]["latest"].as_str();

                if let Some(latest_ver) = latest {
                    if is_newer(latest_ver, local_version) {
                        println!();
                        println!("\x1b[33m🆙 Update available: {} → {}\x1b[0m", local_version, latest_ver);
                        println!("   Run \x1b[36mhs update\x1b[0m to upgrade.");
                    } else if latest_ver == local_version {
                        println!();
                        println!("\x1b[32m✅ You are using the latest version.\x1b[0m");
                    }
                }
            }
        }
    }
}

fn is_newer(latest: &str, local: &str) -> bool {
    if latest == local { return false; }
    
    let parse_ver = |v: &str| {
        let base = v.split('-').next().unwrap_or(v);
        let parts: Vec<u32> = base.split('.')
            .map(|s| s.parse::<u32>().unwrap_or(0))
            .collect();
        
        let score = parts.get(0).copied().unwrap_or(0) * 1000000 
                      + parts.get(1).copied().unwrap_or(0) * 1000 
                      + parts.get(2).copied().unwrap_or(0);
        
        // Beta versions have lower priority than non-beta of same version
        let is_beta = v.contains("-beta");
        let beta_num = if is_beta {
            v.split('.').last().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0)
        } else {
            999 // Non-beta is always higher than beta
        };
        
        (score, beta_num)
    };

    let (lat_s, lat_b) = parse_ver(latest);
    let (loc_s, loc_b) = parse_ver(local);

    if lat_s > loc_s { return true; }
    if lat_s < loc_s { return false; }
    lat_b > loc_b
}
