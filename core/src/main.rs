use std::path::PathBuf;
use tracing::{error, info};
use tracing_subscriber;

use hintshell_core::api::server::HintShellServer;

fn get_db_path() -> PathBuf {
    let old_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ShellMind");
    
    let new_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("HintShell");

    // Migrate history.db if old folder exists and new one doesn't
    if old_dir.exists() && !new_dir.exists() {
        info!("Migrating existing history from ShellMind to HintShell...");
        std::fs::create_dir_all(&new_dir).ok();
        let old_db = old_dir.join("history.db");
        let new_db = new_dir.join("history.db");
        if old_db.exists() {
            std::fs::rename(old_db, new_db).ok();
        }
    }

    std::fs::create_dir_all(&new_dir).ok();
    new_dir.join("history.db")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(true)
        .init();

    info!("Starting HintShell Core Daemon...");
    let db_path = get_db_path();
    info!("Database path: {:?}", db_path);

    let server = match HintShellServer::new(&db_path) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to start HintShell: {}", e);
            std::process::exit(1);
        }
    };

    let shutdown = server.shutdown_signal();

    tokio::select! {
        result = server.run() => {
            if let Err(e) = result {
                error!("Server error: {}", e);
            }
        }
        _ = shutdown.notified() => {
            info!("HintShell Daemon shutting down gracefully.");
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl+C received, shutting down...");
        }
    }
}
