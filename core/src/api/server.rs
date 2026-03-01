use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(windows)]
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::Notify;
use tracing::{error, info, debug};

use crate::api::protocol::*;
use crate::engine::matcher::SuggestionEngine;
use crate::storage::db::HistoryStore;

#[cfg(windows)]
const PIPE_NAME: &str = r"\\.\pipe\hintshell";

#[cfg(unix)]
const SOCKET_PATH: &str = "/tmp/hintshell.sock";

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct HintShellServer {
    engine: Arc<SuggestionEngine>,
    start_time: Instant,
    shutdown: Arc<Notify>,
}

impl HintShellServer {
    pub fn new(db_path: &PathBuf) -> Result<Self, String> {
        let store = HistoryStore::new(db_path).map_err(|e| format!("Failed to open database: {}", e))?;
        let engine = Arc::new(SuggestionEngine::new(store));

        // Seed default commands (runtime file > embedded fallback)
        let defaults_json = Self::load_defaults_json(db_path);
        match engine.seed_defaults(&defaults_json) {
            Ok(0) => { /* All commands already exist */ }
            Ok(n) => info!("Seeded {} default commands into database", n),
            Err(e) => error!("Failed to seed defaults: {}", e),
        }

        Ok(Self {
            engine,
            start_time: Instant::now(),
            shutdown: Arc::new(Notify::new()),
        })
    }

    /// Load default-commands.json at runtime.
    /// Search order: next to DB file, next to binary, embedded fallback.
    fn load_defaults_json(db_path: &PathBuf) -> String {
        // Embedded fallback (always available)
        const EMBEDDED: &str = include_str!("../../default-commands.json");
        let filename = "default-commands.json";

        // 1. Next to DB file (~/.hintshell/default-commands.json)
        if let Some(db_dir) = db_path.parent() {
            let candidate = db_dir.join(filename);
            if candidate.exists() {
                if let Ok(content) = std::fs::read_to_string(&candidate) {
                    info!("Loaded defaults from: {}", candidate.display());
                    return content;
                }
            }
        }

        // 2. Next to the running binary
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let candidate = exe_dir.join(filename);
                if candidate.exists() {
                    if let Ok(content) = std::fs::read_to_string(&candidate) {
                        info!("Loaded defaults from: {}", candidate.display());
                        return content;
                    }
                }
            }
        }

        // 3. Fallback to embedded
        info!("Using embedded default commands");
        EMBEDDED.to_string()
    }

    #[cfg(windows)]
    pub async fn run(&self) -> Result<(), String> {
        info!("HintShell Daemon v{} starting on {}", VERSION, PIPE_NAME);

        loop {
            let server = ServerOptions::new()
                .first_pipe_instance(false)
                .create(PIPE_NAME)
                .map_err(|e| format!("Failed to create named pipe: {}", e))?;

            tokio::select! {
                _ = server.connect() => {
                    self.handle_client(server).await;
                }
                _ = self.shutdown.notified() => {
                    break;
                }
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    pub async fn run(&self) -> Result<(), String> {
        use tokio::net::UnixListener;
        use std::fs;

        info!("HintShell Daemon v{} starting on {}", VERSION, SOCKET_PATH);

        // Cleanup existing socket
        let _ = fs::remove_file(SOCKET_PATH);

        let listener = UnixListener::bind(SOCKET_PATH)
            .map_err(|e| format!("Failed to bind unix socket: {}", e))?;

        loop {
            tokio::select! {
                Ok((stream, _)) = listener.accept() => {
                    self.handle_client(stream).await;
                }
                _ = self.shutdown.notified() => {
                    break;
                }
            }
        }
        Ok(())
    }

    async fn handle_client<S>(&self, stream: S)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        match reader.read_line(&mut line).await {
            Ok(0) => return,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() { return; }

                let response = match serde_json::from_str::<HintShellRequest>(trimmed) {
                    Ok(request) => {
                        debug!("Processing request: {:?}", request);
                        process_request(request, &self.engine, self.start_time, &self.shutdown)
                    }
                    Err(e) => {
                        error!("Invalid JSON: {}", e);
                        HintShellResponse::err(&format!("Invalid JSON: {}", e))
                    }
                };

                let mut resp_json = serde_json::to_string(&response).unwrap_or_default();
                resp_json.push('\n');

                let _ = writer.write_all(resp_json.as_bytes()).await;
                let _ = writer.flush().await;
            }
            Err(e) => error!("IO error: {}", e),
        }
    }

    pub fn shutdown_signal(&self) -> Arc<Notify> {
        Arc::clone(&self.shutdown)
    }
}

fn process_request(
    request: HintShellRequest,
    engine: &SuggestionEngine,
    start_time: Instant,
    shutdown: &Notify,
) -> HintShellResponse {
    match request {
        HintShellRequest::Suggest { input, limit } => {
            let suggestions = engine.suggest(&input, limit);
            let items: Vec<SuggestionItem> = suggestions
                .into_iter()
                .map(|s| SuggestionItem {
                    command: s.command,
                    score: s.score,
                    frequency: s.frequency,
                })
                .collect();
            info!("Returning {} suggestions", items.len());
            HintShellResponse::ok_suggestions(items)
        }

        HintShellRequest::AddCommand {
            command,
            directory,
            shell,
        } => match engine.add_command(&command, directory.as_deref(), shell.as_deref()) {
            Ok(()) => {
                info!("Command added: {}", command);
                HintShellResponse::ok_empty()
            }
            Err(e) => HintShellResponse::err(&e),
        },

        HintShellRequest::Status => {
            let status = DaemonStatus {
                version: VERSION.to_string(),
                total_commands: engine.total_commands(),
                uptime_seconds: start_time.elapsed().as_secs(),
            };
            HintShellResponse::ok_status(status)
        }

        HintShellRequest::Shutdown => {
            info!("Shutdown requested by client");
            shutdown.notify_one();
            HintShellResponse::ok_empty()
        }
    }
}

