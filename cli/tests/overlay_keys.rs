//! End-to-end keystroke tests: the real wrapper driving a real Bash inside a real ConPTY.
//!
//! Unit tests feed the filters the bytes a shell *writes*, but Windows ConPTY sits between
//! them and re-maps control bytes through the console codepage. These tests remove that layer
//! of guesswork: keystrokes go in one end, screen bytes come out the other, and the overlay
//! behaviour the user actually sees is what gets asserted.
#![cfg(windows)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use conpty::{Process, ProcessOptions};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Top-left corner of the suggestion frame.
const BOX: &[u8] = "╭".as_bytes();

fn contains_sequence(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Every session here talks to the same daemon and waits on wall-clock rendering, so running
/// them concurrently makes the first frame arrive after its deadline. One at a time instead.
fn serialise() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn bash_executable() -> Option<PathBuf> {
    for candidate in [
        "C:\\Program Files\\Git\\bin\\bash.exe",
        "C:\\Program Files (x86)\\Git\\bin\\bash.exe",
    ] {
        let path = PathBuf::from(candidate);
        if path.exists() {
            return Some(path);
        }
    }
    let which = Command::new("where").arg("bash.exe").output().ok()?;
    let first = String::from_utf8_lossy(&which.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    let path = PathBuf::from(first);
    path.exists().then_some(path)
}

struct Session {
    process: Process,
    input: conpty::io::PipeWriter,
    screen: Arc<Mutex<Vec<u8>>>,
}

impl Session {
    fn start(hook: &Path, _bash: &Path) -> Self {
        // The wrapper resolves Bash itself; the explicit --rcfile keeps the session on the
        // generated hook instead of the user's full profile.
        Self::with_args(&[
            "bash".to_string(),
            "--rcfile".to_string(),
            hook.to_str().unwrap().to_string(),
            "-i".to_string(),
        ])
    }

    fn with_args(args: &[String]) -> Self {
        Self::with_env(args, &[])
    }

    fn with_env(args: &[String], envs: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hintshell"));
        command.args(args);
        command.env("TERM", "xterm-256color");
        for (key, value) in envs {
            command.env(key, value);
        }

        let mut options = ProcessOptions::default();
        options.set_console_size(Some((90, 30)));
        let mut process = options
            .spawn(command)
            .expect("cannot start the wrapper inside ConPTY");

        let input = process.input().expect("cannot open ConPTY input");
        let mut reader = process.output().expect("cannot open ConPTY output");
        let screen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&screen);
        thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buffer[..read]);
            }
        });

        let session = Self {
            process,
            input,
            screen,
        };
        // Let the shell start and print its first prompt.
        session.wait_for(|_| true, 0, Duration::from_secs(3));
        session
    }

    fn mark(&self) -> usize {
        self.screen.lock().unwrap().len()
    }

    fn send(&mut self, bytes: &[u8]) {
        self.input
            .write_all(bytes)
            .expect("cannot write keystrokes");
        self.input.flush().ok();
    }

    fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            self.send(character.to_string().as_bytes());
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// Wait until `check` accepts everything the wrapper has printed since `from`.
    fn wait_for(&self, check: impl Fn(&[u8]) -> bool, from: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let screen = self.screen.lock().unwrap();
            if check(&screen[from.min(screen.len())..]) {
                return true;
            }
            drop(screen);
            if Instant::now() > deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn screen_text(&self) -> String {
        String::from_utf8_lossy(&self.screen.lock().unwrap()).to_string()
    }

    /// Type `text` and wait for a frame. Retries exist only for warm-up: the first query of a
    /// session can race a cold daemon. The behaviour under test is always asserted with
    /// `attempts = 1`, so a retry can never mask a real regression.
    fn wait_frame(&mut self, text: &str, attempts: usize, timeout: Duration) -> bool {
        for attempt in 0..attempts {
            if attempt > 0 {
                self.send(b"\x15");
                thread::sleep(Duration::from_millis(250));
            }
            let from = self.mark();
            self.type_text(text);
            if self.wait_for(|bytes| contains_sequence(bytes, BOX), from, timeout) {
                return true;
            }
        }
        false
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Ctrl+U first: the harness leaves a half-typed line behind, and sending it would
        // execute it and write junk into the user's real command history.
        self.send(b"\x15exit\r");
        let _ = self.process.wait(Some(3_000));
    }
}

/// The generated hook, so the session runs the same marker protocol as a real terminal.
fn generated_hook() -> PathBuf {
    let output = Command::new(env!("CARGO_BIN_EXE_hintshell"))
        .args(["hook", "bash"])
        .output()
        .expect("cannot generate the bash hook");
    let path = std::env::temp_dir().join("hintshell-overlay-keys-hook.bash");
    std::fs::write(&path, output.stdout).expect("cannot write the hook");
    path
}

/// The daemon answers on one fixed named pipe per machine, and the overlay is empty without
/// it. Start the binary this very build produced: `hs start` resolves the core executable
/// from an installed layout that a CI checkout does not have, which silently left every
/// frame assertion here failing on a runner.
fn daemon_is_running() -> bool {
    Command::new(env!("CARGO_BIN_EXE_hs"))
        .arg("status")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("Daemon is running"))
        .unwrap_or(false)
}

// The daemon is deliberately left running after this test process is done: it owns a named
// pipe shared by every session on the machine, and `Session::drop` is not its lifecycle owner.
#[allow(clippy::zombie_processes)]
fn ensure_daemon() {
    if daemon_is_running() {
        return;
    }
    // Cargo only exports CARGO_BIN_EXE_* for binaries of this package, so the daemon is
    // found as a sibling of the CLI binary the test profile just built.
    let core = core_binary().unwrap_or_else(|| {
        panic!(
            "no hintshell-core binary next to the test build; run cargo test -p hintshell-core too"
        )
    });
    let mut daemon = Command::new(&core);
    daemon
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // DETACHED_PROCESS, the same flag `hs start` uses: without it the daemon inherits the
    // output handle cargo test is reading, so the pipe never closes and the whole test run
    // blocks after the tests themselves have passed.
    daemon.creation_flags(0x00000008);
    daemon
        .spawn()
        .unwrap_or_else(|error| panic!("cannot start {}: {error}", core.display()));
    // An already-healthy daemon keeps the pipe, so this extra instance exits on its own.
    for _ in 0..50 {
        if daemon_is_running() {
            thread::sleep(Duration::from_millis(400));
            return;
        }
        thread::sleep(Duration::from_millis(200));
    }
    panic!("the HintShell daemon never answered on its pipe; the overlay has nothing to show");
}

fn core_binary() -> Option<PathBuf> {
    let cli = PathBuf::from(env!("CARGO_BIN_EXE_hintshell"));
    let directory = cli.parent()?;
    for name in ["hintshell-core.exe", "hintshell-core"] {
        let candidate = directory.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[test]
fn ctrl_u_after_typing_still_shows_suggestions_in_a_real_session() {
    let _guard = serialise();
    let Some(bash) = bash_executable() else {
        eprintln!("skipping: no bash.exe found");
        return;
    };
    let hook = generated_hook();
    ensure_daemon();

    let mut session = Session::start(hook.as_path(), bash.as_path());

    assert!(
        session.wait_frame("git st", 3, Duration::from_secs(6)),
        "no suggestion frame appeared while typing: {}",
        session.screen_text()
    );

    // Discard the line, exactly as a user clearing a mistake does.
    session.send(b"\x15");
    thread::sleep(Duration::from_millis(150));

    assert!(
        session.wait_frame("git stash", 1, Duration::from_secs(8)),
        "the overlay stayed dead after Ctrl+U: {}",
        session.screen_text()
    );
}

#[test]
fn ctrl_u_after_accepting_a_suggestion_still_shows_suggestions() {
    let _guard = serialise();
    let Some(bash) = bash_executable() else {
        eprintln!("skipping: no bash.exe found");
        return;
    };
    let hook = generated_hook();
    ensure_daemon();

    let mut session = Session::start(hook.as_path(), bash.as_path());

    assert!(
        session.wait_frame("git st", 3, Duration::from_secs(6)),
        "no frame before the accept: {}",
        session.screen_text()
    );

    // Tab accepts the selected row, which makes the wrapper send Ctrl+U itself and retype
    // the whole command; the user then clears the line and starts over.
    session.send(b"\t");
    thread::sleep(Duration::from_millis(400));
    session.send(b"\x15");
    thread::sleep(Duration::from_millis(200));

    assert!(
        session.wait_frame("git stash", 1, Duration::from_secs(8)),
        "the overlay never came back after Tab accept then Ctrl+U: {}",
        session.screen_text()
    );
}

#[test]
fn ctrl_u_after_accepting_a_suggestion_works_with_the_installed_rcfile() {
    let _guard = serialise();
    // A real Git Bash window starts through ~/.hintshell/git-bash-fast.bashrc, which is
    // generated once by `hs init --git-bash-fast` and can lag behind the installed binary.
    let Some(bash) = bash_executable() else {
        eprintln!("skipping: no bash.exe found");
        return;
    };
    let rcfile = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
        .join(".hintshell")
        .join("git-bash-fast.bashrc");
    if !rcfile.exists() {
        eprintln!("skipping: {} does not exist", rcfile.display());
        return;
    }
    ensure_daemon();

    let mut session = Session::start(rcfile.as_path(), bash.as_path());

    assert!(
        session.wait_frame("git st", 3, Duration::from_secs(6)),
        "no frame before the accept: {}",
        session.screen_text()
    );

    session.send(b"\t");
    thread::sleep(Duration::from_millis(400));
    session.send(b"\x15");
    thread::sleep(Duration::from_millis(200));

    assert!(
        session.wait_frame("git stash", 1, Duration::from_secs(8)),
        "the overlay never came back after Tab accept then Ctrl+U with the installed rcfile: {}",
        session.screen_text()
    );
}

#[test]
fn ctrl_u_after_accepting_a_suggestion_works_in_a_real_login_shell() {
    let _guard = serialise();
    // The user's own session: /etc/profile plus ~/.bashrc, which carries their fzf and bind
    // customisations and can rebind the very chords the wrapper models.
    if bash_executable().is_none() {
        eprintln!("skipping: no bash.exe found");
        return;
    }
    ensure_daemon();

    let mut session =
        Session::with_args(&["bash".to_string(), "--login".to_string(), "-i".to_string()]);

    assert!(
        session.wait_frame("git st", 3, Duration::from_secs(6)),
        "no frame while typing in a login shell: {}",
        session.screen_text()
    );

    session.send(b"\t");
    thread::sleep(Duration::from_millis(400));
    session.send(b"\x15");
    thread::sleep(Duration::from_millis(200));

    assert!(
        session.wait_frame("git stash", 1, Duration::from_secs(8)),
        "the overlay never came back after Tab accept then Ctrl+U in a login shell: {}",
        session.screen_text()
    );

    // The state in the user's screenshot: a bare command plus its trailing space, after the
    // same Tab-then-Ctrl+U sequence. A frame must still be on screen at the end.
    let from = session.mark();
    session.send(b"\x15");
    thread::sleep(Duration::from_millis(200));
    session.type_text("git ");
    let drawn = session.wait_for(
        |bytes| contains_sequence(bytes, BOX),
        from,
        Duration::from_secs(8),
    );
    if std::env::var("HINTSHELL_DUMP").is_ok() {
        // Escape-visible tail, for judging whether a frame is drawn and then erased.
        let screen = session.screen.lock().unwrap();
        let tail = &screen[from.min(screen.len())..];
        let text = String::from_utf8_lossy(tail).replace('\x1b', "<ESC>");
        eprintln!("DUMP after Ctrl+U typing:\n{text:?}");
    }
    assert!(
        drawn,
        "no frame for a command followed by a space: {}",
        session.screen_text()
    );
}

#[test]
fn key_tracing_records_what_the_wrapper_believed() {
    // The overlay only exists on screen, so a user's "it stopped suggesting" cannot be
    // diagnosed from a screenshot. HINTSHELL_TRACE_KEYS=1 must leave the state machine on disk.
    let _guard = serialise();
    if bash_executable().is_none() {
        eprintln!("skipping: no bash.exe found");
        return;
    }

    // An isolated home keeps this session out of the real history database.
    let home = std::env::temp_dir().join(format!("hintshell-keys-trace-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("cannot create the trace home");

    let mut session = Session::with_env(
        &["bash".to_string(), "--norc".to_string(), "-i".to_string()],
        &[
            ("HINTSHELL_TRACE_KEYS", "1"),
            ("HINTSHELL_HOME", home.to_str().unwrap()),
        ],
    );
    session.type_text("git st");
    session.send(b"\x15");
    session.type_text("ls");
    session.send(b"\x15");

    let path = home.join("keys-trace.log");
    let log = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} was not written: {error}", path.display()));
    let lines: Vec<&str> = log.lines().collect();

    // The Ctrl+U keystroke must be on its own line (crossterm prints modifiers as a bit flag,
    // so only the key identity is asserted here).
    let discarded = lines
        .iter()
        .position(|line| line.starts_with("key Char('u')"))
        .unwrap_or_else(|| panic!("the Ctrl+U keystroke is missing from the trace: {log}"));
    assert!(
        lines[discarded].contains("tracked=true"),
        "the trace lost the tracking state at Ctrl+U: {}",
        lines[discarded]
    );
    // The line right after the discard must show an empty buffer: that is the fact a bug
    // report needs in order to explain a panel that never came back.
    assert!(
        lines[discarded + 1].contains("buf=\"\"") || lines[discarded].contains("buf=\"\""),
        "the trace did not record the emptied line: {:?}",
        &lines[discarded..discarded + 2]
    );
    assert!(
        lines.iter().any(|line| line.starts_with("query ")),
        "the trace never recorded a query: {log}"
    );
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn no_protocol_marker_text_ever_reaches_the_screen() {
    let _guard = serialise();
    let Some(bash) = bash_executable() else {
        eprintln!("skipping: no bash.exe found");
        return;
    };
    let hook = generated_hook();
    ensure_daemon();

    let mut session = Session::start(&hook, &bash);
    session.type_text("pwd\r");
    thread::sleep(Duration::from_millis(700));
    session.send(b"\x15");
    session.type_text("cd /tmp\r");
    thread::sleep(Duration::from_millis(700));

    let screen = session.screen_text();
    for forbidden in ["HINTSHELL", "5522", "\u{25b2}", "\u{25bc}"] {
        assert!(
            !screen.contains(forbidden),
            "marker fragment {forbidden:?} reached the screen: {screen}"
        );
    }
}
