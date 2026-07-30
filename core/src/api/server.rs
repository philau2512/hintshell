use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(windows)]
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::api::protocol::*;
use crate::engine::matcher::SuggestionEngine;
use crate::storage::db::HistoryStore;

#[cfg(windows)]
const PIPE_NAME: &str = r"\\.\pipe\hintshell";

#[cfg(unix)]
const SOCKET_PATH: &str = "/tmp/hintshell.sock";

const VERSION: &str = env!("CARGO_PKG_VERSION");

// On Windows, named pipes are kernel objects that persist until all handles are closed.
// When a process crashes without proper cleanup, the pipe may remain in a broken state.
// This helper attempts to clean up such stale pipes by trying to connect to them
// and then disconnecting, which releases the kernel object if it's in a broken state.
#[cfg(windows)]
fn delete_pipe_if_exists(pipe_name: &str) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    // Convert the pipe name to wide string
    let wide_name: Vec<u16> = OsStr::new(pipe_name).encode_wide().chain(Some(0)).collect();

    // Try to open the existing pipe to verify it exists and clean it up
    unsafe {
        let handle = CreateFileW(
            wide_name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null_mut(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        );

        if handle != INVALID_HANDLE_VALUE {
            // Connected to the stale pipe, close our handle to release it
            CloseHandle(handle);
            Ok(())
        } else {
            // Pipe doesn't exist or can't be accessed, which is fine
            Err(format!(
                "Pipe not accessible: {}",
                std::io::Error::last_os_error()
            ))
        }
    }
}

// Windows API imports for pipe cleanup
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut u8,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: *mut u8,
    ) -> *mut u8;

    fn CloseHandle(hObject: *mut u8) -> i32;
}

#[cfg(windows)]
const GENERIC_READ: u32 = 0x80000000;
#[cfg(windows)]
const GENERIC_WRITE: u32 = 0x40000000;
#[cfg(windows)]
const OPEN_EXISTING: u32 = 3;
#[cfg(windows)]
const INVALID_HANDLE_VALUE: *mut u8 = !0 as *mut u8;

pub struct HintShellServer {
    engine: Arc<SuggestionEngine>,
    start_time: Instant,
    shutdown: Arc<Notify>,
}

impl HintShellServer {
    pub fn new(db_path: &PathBuf) -> Result<Self, String> {
        let store =
            HistoryStore::new(db_path).map_err(|e| format!("Failed to open database: {}", e))?;
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
    /// Search order: next to DB, ~/.hintshell/, next to binary, embedded fallback.
    fn load_defaults_json(db_path: &Path) -> String {
        // Embedded fallback (always available)
        const EMBEDDED: &str = include_str!("../../default-commands.json");
        let filename = "default-commands.json";

        // 1. Next to DB file (AppData/Local/HintShell/)
        if let Some(db_dir) = db_path.parent() {
            let candidate = db_dir.join(filename);
            if candidate.exists() {
                if let Ok(content) = std::fs::read_to_string(&candidate) {
                    info!("Loaded defaults from: {}", candidate.display());
                    return content;
                }
            }
        }

        // 2. ~/.hintshell/ (where `hintshell init` copies config files)
        if let Some(home) = dirs::home_dir() {
            let candidate = home.join(".hintshell").join(filename);
            if candidate.exists() {
                if let Ok(content) = std::fs::read_to_string(&candidate) {
                    info!("Loaded defaults from: {}", candidate.display());
                    return content;
                }
            }
        }

        // 3. Next to the running binary
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

        // 4. Fallback to embedded
        info!("Using embedded default commands");
        EMBEDDED.to_string()
    }

    #[cfg(windows)]
    pub async fn run(&self) -> Result<(), String> {
        use std::io::ErrorKind;

        info!("HintShell Daemon v{} starting on {}", VERSION, PIPE_NAME);

        // Claim the first pipe instance — if another healthy daemon holds it, exit.
        // Do NOT delete_pipe_if_exists first: that can steal a live daemon's pipe.
        let mut server = match ServerOptions::new()
            .first_pipe_instance(true)
            .create(PIPE_NAME)
        {
            Ok(s) => s,
            Err(e) => {
                let already = e.kind() == ErrorKind::AlreadyExists
                    || e.kind() == ErrorKind::PermissionDenied
                    || e.raw_os_error() == Some(183) // ERROR_ALREADY_EXISTS
                    || e.raw_os_error() == Some(5); // ERROR_ACCESS_DENIED (common for first_pipe_instance)
                if already {
                    info!(
                        "Another HintShell daemon already owns {}; exiting this instance.",
                        PIPE_NAME
                    );
                    return Ok(());
                }
                // Stale/broken pipe: try one cleanup then retry as first instance
                warn!(
                    "Failed to create first pipe instance: {}; cleanup + retry",
                    e
                );
                let _ = delete_pipe_if_exists(PIPE_NAME);
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
                ServerOptions::new()
                    .first_pipe_instance(true)
                    .create(PIPE_NAME)
                    .map_err(|e2| format!("Failed to create pipe after cleanup: {}", e2))?
            }
        };

        info!("Daemon listening on {}", PIPE_NAME);

        loop {
            // Wait for a client on the current instance
            tokio::select! {
                result = server.connect() => {
                    if let Err(e) = result {
                        error!("Pipe connect error: {}", e);
                        server = ServerOptions::new()
                            .first_pipe_instance(false)
                            .create(PIPE_NAME)
                            .map_err(|e2| format!("Failed to recreate pipe: {}", e2))?;
                        continue;
                    }
                }
                _ = self.shutdown.notified() => {
                    info!("Shutdown signal received");
                    break;
                }
            }

            // Pre-create the NEXT instance so another client can connect while we handle this one
            let next = ServerOptions::new()
                .first_pipe_instance(false)
                .create(PIPE_NAME);

            // Move current connected server into the handler
            let connected = server;
            self.handle_client(connected).await;

            // Promote next instance, or recreate if pre-create failed
            server = match next {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to pre-create next pipe instance: {}; recreating", e);
                    ServerOptions::new()
                        .first_pipe_instance(false)
                        .create(PIPE_NAME)
                        .map_err(|e2| format!("Failed to recreate pipe: {}", e2))?
                }
            };
        }
        Ok(())
    }

    #[cfg(unix)]
    pub async fn run(&self) -> Result<(), String> {
        use std::fs;
        use tokio::net::UnixListener;

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
            Ok(0) => {}
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return;
                }

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
        HintShellRequest::Suggest {
            input,
            limit,
            cwd,
            shell,
        } => {
            let suggestions =
                engine.suggest_with_context(&input, limit, cwd.as_deref(), shell.as_deref());
            let items: Vec<SuggestionItem> = suggestions
                .into_iter()
                .map(|s| SuggestionItem {
                    command: s.command,
                    description: s.description,
                    score: s.score,
                    frequency: s.frequency,
                    source: s.source,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_request_returns_contextual_path_candidate() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("src")).unwrap();
        let engine = SuggestionEngine::new(HistoryStore::in_memory().unwrap());
        let shutdown = Notify::new();

        let response = process_request(
            HintShellRequest::Suggest {
                input: "cd s".to_string(),
                limit: 5,
                cwd: Some(directory.path().to_string_lossy().to_string()),
                shell: Some("bash".to_string()),
            },
            &engine,
            Instant::now(),
            &shutdown,
        );

        let suggestion = response
            .suggestions
            .unwrap()
            .into_iter()
            .find(|item| item.command == "cd src/")
            .unwrap();
        assert_eq!(suggestion.source, "path");
    }
}
