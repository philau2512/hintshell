use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(80);
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(2);
const DEFAULT_OUTPUT_CAP: usize = 16 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessOutput {
    pub stdout: String,
    pub status: Option<i32>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    output: ProcessOutput,
    expires_at: Instant,
}

#[derive(Default)]
pub struct ProcessRunner {
    cache: Mutex<HashMap<String, CacheEntry>>,
}

impl ProcessRunner {
    pub fn run_cached(
        &self,
        cache_key: impl Into<String>,
        program: &str,
        arguments: &[&str],
    ) -> ProcessOutput {
        self.run_cached_in_dir(cache_key, None, program, arguments)
    }
    pub fn run_cached_in_dir(
        &self,
        cache_key: impl Into<String>,
        cwd: Option<&Path>,
        program: &str,
        arguments: &[&str],
    ) -> ProcessOutput {
        let cache_key = cache_key.into();
        if let Ok(cache) = self.cache.lock() {
            if let Some(entry) = cache.get(&cache_key) {
                if Instant::now() < entry.expires_at {
                    return entry.output.clone();
                }
            }
        }

        let output = run_with_timeout(cwd, program, arguments, DEFAULT_TIMEOUT, DEFAULT_OUTPUT_CAP);
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                cache_key,
                CacheEntry {
                    output: output.clone(),
                    expires_at: Instant::now() + DEFAULT_CACHE_TTL,
                },
            );
        }
        output
    }
}

fn run_with_timeout(
    cwd: Option<&Path>,
    program: &str,
    arguments: &[&str],
    timeout: Duration,
    output_cap: usize,
) -> ProcessOutput {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let Ok(mut child) = command.spawn() else {
        return ProcessOutput::default();
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return ProcessOutput::default();
    };
    let (output_sender, output_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = output_sender.send(read_capped(stdout, output_cap));
    });

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = output_receiver
                    .recv_timeout(Duration::from_millis(20))
                    .map(|bytes| capped_utf8(&bytes, output_cap))
                    .unwrap_or_default();
                return ProcessOutput {
                    stdout,
                    status: status.code(),
                };
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return ProcessOutput::default();
            }
        }
    }
}

fn read_capped(mut reader: impl Read, output_cap: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(output_cap);
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return bytes,
            Ok(read) => {
                let available = output_cap.saturating_sub(bytes.len());
                bytes.extend_from_slice(&buffer[..read.min(available)]);
            }
        }
    }
}

fn capped_utf8(bytes: &[u8], output_cap: usize) -> String {
    let length = bytes.len().min(output_cap);
    String::from_utf8_lossy(&bytes[..length]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_process_output() {
        assert_eq!(capped_utf8(b"abcdef", 3), "abc");
    }

    #[test]
    fn missing_program_fails_closed() {
        let output = run_with_timeout(
            None,
            "hintshell-program-that-does-not-exist",
            &[],
            Duration::from_millis(20),
            100,
        );
        assert_eq!(output, ProcessOutput::default());
    }
}
