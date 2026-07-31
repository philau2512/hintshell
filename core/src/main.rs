use std::path::PathBuf;
#[cfg(windows)]
use tracing::warn;
use tracing::{error, info};

use hintshell_core::api::server::HintShellServer;

fn get_db_path() -> PathBuf {
    if let Some(data_home) = std::env::var_os("HINTSHELL_DATA_HOME") {
        let data_dir = PathBuf::from(data_home);
        std::fs::create_dir_all(&data_dir).ok();
        return data_dir.join("history.db");
    }

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

/// Process-wide single-instance guard (Windows named mutex / Unix lock file).
/// Kept alive for the whole process so the OS releases it on exit.
struct InstanceGuard {
    #[cfg(windows)]
    _handle: *mut std::ffi::c_void,
    #[cfg(unix)]
    _file: std::fs::File,
}

// Mutex handle is only closed on process exit; safe to share ownership marker.
unsafe impl Send for InstanceGuard {}

#[cfg(windows)]
fn try_claim_single_instance() -> Option<InstanceGuard> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    const ERROR_ALREADY_EXISTS: u32 = 183;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(
            lp_mutex_attributes: *mut std::ffi::c_void,
            b_initial_owner: i32,
            lp_name: *const u16,
        ) -> *mut std::ffi::c_void;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }

    // Local\ = current user session only (correct for IDE + external terminals)
    let name: Vec<u16> = OsStr::new("Local\\HintShellCoreDaemon")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let handle = CreateMutexW(std::ptr::null_mut(), 1, name.as_ptr());
        if handle.is_null() {
            warn!("CreateMutexW failed: {}", std::io::Error::last_os_error());
            // Fail open: still try to start; pipe first_instance is the backup.
            return Some(InstanceGuard {
                _handle: std::ptr::null_mut(),
            });
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return None;
        }
        Some(InstanceGuard { _handle: handle })
    }
}

#[cfg(unix)]
fn try_claim_single_instance() -> Option<InstanceGuard> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = "/tmp/hintshell.daemon.lock";
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o644)
        .open(path)
        .ok()?;

    // LOCK_EX|LOCK_NB via flock
    #[link(name = "c")]
    extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;

    use std::os::unix::io::AsRawFd;
    let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if rc != 0 {
        return None;
    }
    let _ = file.set_len(0);
    let _ = write!(file, "{}", std::process::id());
    Some(InstanceGuard { _file: file })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(true)
        .init();

    info!("Starting HintShell Core Daemon...");

    // Claim single-instance BEFORE opening DB / creating pipe.
    // This stops IDE multi-terminal races from stacking hintshell-core processes.
    let _instance_guard = match try_claim_single_instance() {
        Some(g) => g,
        None => {
            info!("Another HintShell daemon instance already holds the single-instance lock; exiting.");
            return;
        }
    };

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
