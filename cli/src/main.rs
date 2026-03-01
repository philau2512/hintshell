#[cfg(windows)]
use std::os::windows::process::CommandExt;

use clap::{Parser, Subcommand};
use hintshell_core::api::protocol::{HintShellRequest, HintShellResponse};
use std::process::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
        Commands::Start => start_daemon(),
        Commands::Stop => {
            let request = HintShellRequest::Shutdown;
            match send_request(&request).await {
                Ok(resp) => {
                    if resp.success {
                        println!("✅ HintShell daemon stopped.");
                    } else {
                        println!("❌ Error: {}", resp.error.unwrap_or_default());
                    }
                }
                Err(_) => println!("ℹ️  Daemon is not running."),
            }
        }
        Commands::Status => {
            let request = HintShellRequest::Status;
            match send_request(&request).await {
                Ok(resp) => {
                    if let Some(status) = resp.status {
                        println!("🧠 HintShell Daemon v{}", status.version);
                        println!("   Commands in history: {}", status.total_commands);
                        println!("   Uptime: {}s", status.uptime_seconds);

                        // Check for updates from npm registry
                        check_npm_update(&status.version);
                    }
                }
                Err(_) => println!("❌ Daemon is not running."),
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
                            // FZF: command + frequency, tab-separated
                            for s in &suggestions {
                                let cmd = &s.command;
                                let display = if cmd.len() > 60 {
                                    format!("{}…", &cmd[..59])
                                } else {
                                    cmd.clone()
                                };
                                println!("{:<60}\t({}x)", display, s.frequency);
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
        Commands::Init => {
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
                Err(e) => println!("⚠️ {}", e),
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
            println!("Done! Please restart your shell to activate hooks.");
        }
        Commands::Hook { shell } => {
            let s = shell::Shell::from_str(&shell).expect("Unsupported shell");
            print!("{}", s.get_hook());
        }
        Commands::Uninstall => {
            println!("🗑 Uninstalling HintShell...");
            
            // 1. Stop daemon
            let request = HintShellRequest::Shutdown;
            let _ = send_request(&request).await;
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
            check_npm_update(env!("CARGO_PKG_VERSION"));
        }
    }
}

fn start_daemon() {
    let core_name = if cfg!(windows) { "hintshell-core.exe" } else { "hintshell-core" };
    let exe_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(core_name)))
        .unwrap_or_else(|| core_name.into());

    if !exe_path.exists() {
        println!("❌ Cannot find {} at {:?}", core_name, exe_path);
        println!("   Make sure to build the core first: cargo build --release");
        return;
    }

    let mut cmd = Command::new(&exe_path);

    // Redirect stdout/stderr to null so daemon logs don't pollute the terminal
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    
    #[cfg(windows)]
    cmd.creation_flags(0x00000008); // DETACHED_PROCESS

    match cmd.spawn() {
        Ok(_) => println!("🧠 HintShell daemon started in background."),
        Err(e) => println!("❌ Failed to start daemon: {}", e),
    }
}

#[cfg(windows)]
async fn send_request(request: &HintShellRequest) -> Result<HintShellResponse, String> {
    let pipe = ClientOptions::new()
        .open(PIPE_NAME)
        .map_err(|e| format!("Cannot connect to HintShell daemon: {}", e))?;

    handle_ipc(pipe, request).await
}

#[cfg(unix)]
async fn send_request(request: &HintShellRequest) -> Result<HintShellResponse, String> {
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
