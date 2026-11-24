use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
#[cfg(windows)]
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::cursor::position;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use hintshell_core::api::protocol::{HintShellRequest, SuggestionItem};
use hintshell_core::engine::matcher::Risk;

#[cfg(windows)]
use conpty::ProcessOptions;
#[cfg(unix)]
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const QUERY_DEBOUNCE: Duration = Duration::from_millis(40);
const QUERY_TIMEOUT: Duration = Duration::from_millis(180);
const TAB_ACCEPT_CLEAR_DELAY: Duration = Duration::from_millis(8);

fn trace_startup_enabled(value: Option<&str>) -> bool {
    matches!(value, Some("1"))
}

fn trace_startup(started: Instant, phase: &str) {
    if trace_startup_enabled(env::var("HINTSHELL_TRACE_STARTUP").ok().as_deref()) {
        let line = format!(
            "HintShell startup {phase}: {:.1}ms",
            started.elapsed().as_secs_f64() * 1_000.0
        );
        eprintln!("{line}");
        write_startup_trace(&line);
    }
}

#[cfg(windows)]
fn write_startup_trace(line: &str) {
    let path = crate::shell::hintshell_home().join("startup-trace.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(not(windows))]
fn write_startup_trace(_line: &str) {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveShell {
    Bash,
    Zsh,
}

impl LiveShell {
    fn request_shell(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Bash => "HINTSHELL_LIVE_BASH",
            Self::Zsh => "HINTSHELL_LIVE_ZSH",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Bash => "Bash",
            Self::Zsh => "Zsh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabAction {
    AcceptHintShell,
    ForwardToBash,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnixInputMode {
    PromptOverlay,
    ChildPassthrough,
}

#[derive(Debug, Clone)]
struct OverlayState {
    buffer: String,
    tracking_valid: bool,
    /// Whether the readline cursor sits at the end of the line. History recall leaves it there,
    /// cursor keys do not, and that distinction is what makes Ctrl+U recoverable: discarding
    /// backwards from the end empties the line, while discarding mid-line leaves a tail the
    /// wrapper has no copy of.
    cursor_at_end: bool,
    /// Esc hides the panel for the rest of this input line; typing must not bring it back.
    dismissed: bool,
    /// Display columns of dim ghost text drawn after the cursor (narrow/short terminals).
    ghost_cols: usize,
    generation: u64,
    selected: usize,
    suggestions: Vec<SuggestionItem>,
    rendered_lines: usize,
}

impl OverlayState {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            tracking_valid: true,
            cursor_at_end: true,
            dismissed: false,
            ghost_cols: 0,
            generation: 0,
            selected: 0,
            suggestions: Vec::new(),
            rendered_lines: 0,
        }
    }

    fn clear(&mut self) {
        self.suggestions.clear();
        self.selected = 0;
    }

    fn current(&self) -> Option<&SuggestionItem> {
        self.suggestions.get(self.selected)
    }
}

struct RawModeGuard {
    bracketed_paste_enabled: bool,
}

impl RawModeGuard {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("cannot enable raw terminal mode: {error}"))?;
        let mut guard = Self {
            bracketed_paste_enabled: false,
        };
        guard.enable_bracketed_paste()?;
        Ok(guard)
    }

    fn enable_bracketed_paste(&mut self) -> Result<(), String> {
        if self.bracketed_paste_enabled {
            return Ok(());
        }
        execute!(io::stdout(), EnableBracketedPaste)
            .map_err(|error| format!("cannot enable bracketed paste: {error}"))?;
        self.bracketed_paste_enabled = true;
        Ok(())
    }

    fn disable_bracketed_paste(&mut self) {
        if self.bracketed_paste_enabled {
            let _ = execute!(io::stdout(), DisableBracketedPaste);
            self.bracketed_paste_enabled = false;
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        self.disable_bracketed_paste();
        let _ = disable_raw_mode();
    }
}

#[cfg(windows)]
pub fn run(shell: LiveShell, args: Vec<String>) -> Result<(), String> {
    let started = Instant::now();
    if shell != LiveShell::Bash {
        return Err("live Zsh overlay is available only on macOS".to_string());
    }
    if !supports_live_overlay() {
        return Err(
            "live overlay requires an ANSI-capable terminal; open Git Bash normally to use the legacy Tab/fzf integration"
                .to_string(),
        );
    }

    let (cols, rows) = size().unwrap_or((120, 30));
    let bash = resolve_bash()?;
    trace_startup(started, "resolved Bash");
    // `conpty` forwards its program string directly to CreateProcessW, so the
    // executable must be quoted when Git for Windows is installed under Program Files.
    let mut command = Command::new(quote_windows_argument(&bash));
    // `conpty` builds the child environment from explicit entries only.
    // Preserve the Git Bash session environment before setting the wrapper marker.
    command.envs(env::vars_os());
    command.env(shell.marker(), "1");
    // One-shot token proving this shell is the one the wrapper pumps. The hook consumes it,
    // so a nested `bash` (or an older wrapper that cannot filter markers) emits nothing and no
    // marker text can end up on screen as garbage.
    command.env(MARKER_TOKEN_ENV, std::process::id().to_string());
    let interactive = args.is_empty() || args.iter().any(|argument| argument == "-i");
    if args.is_empty() {
        command.args(["--login", "-i"]);
    } else {
        command.args(args.iter().map(quote_windows_argument));
    }

    let mut process = {
        let mut options = ProcessOptions::default();
        options.set_console_size(Some((cols as i16, rows as i16)));
        options
            .spawn(command)
            .map_err(|error| format!("cannot start Git Bash: {error}"))?
    };
    trace_startup(started, "spawned ConPTY Bash");

    let writer = Arc::new(Mutex::new(Box::new(
        process
            .input()
            .map_err(|error| format!("cannot open Git Bash input: {error}"))?,
    ) as Box<dyn Write + Send>));
    let output = Arc::new(Mutex::new(io::stdout()));
    let (shell_events_tx, shell_events_rx) = mpsc::channel::<ShellEvent>();
    let output_reader = process
        .output()
        .map_err(|error| format!("cannot open Git Bash output: {error}"))?;
    let query_cwd = Arc::new(Mutex::new(
        env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().to_string()),
    ));
    start_output_pump(
        Box::new(output_reader),
        shell_events_tx,
        Some(Arc::clone(&query_cwd)),
    );

    if !interactive {
        return run_batch_process(process, shell_events_rx, &output);
    }

    let (query_tx, query_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    start_query_worker(
        query_rx,
        result_tx,
        Arc::clone(&query_cwd),
        shell.request_shell(),
    );

    let _raw_mode = RawModeGuard::enter()?;
    trace_startup(started, "entered raw mode");
    let mut state = OverlayState::new();
    let mut saw_first_prompt = false;
    let mut running = true;

    while running {
        while let Ok(shell_event) = shell_events_rx.try_recv() {
            match shell_event {
                ShellEvent::Output(bytes) => {
                    // When shell outputs text during interactive typing, do not clear
                    // overlay proactively if tracking is valid; just forward terminal bytes.
                    write_terminal(&output, &bytes)?;
                }
                ShellEvent::StartupPhase(phase) => {
                    trace_startup(started, &format!("rcfile {phase}"));
                }
                ShellEvent::PromptReady => {
                    if !saw_first_prompt {
                        trace_startup(started, "received first prompt marker");
                        saw_first_prompt = true;
                    }
                    clear_overlay(&mut state, &output)?;
                    state.tracking_valid = true;
                    state.cursor_at_end = true;
                    state.dismissed = false;
                    state.buffer.clear();
                    state.clear();
                    trace_state("event PromptReady", &state);
                }
                ShellEvent::Continuation => {
                    clear_overlay(&mut state, &output)?;
                    state.buffer.clear();
                    state.clear();
                    state.tracking_valid = false;
                    // A fresh continuation line is empty with the cursor at its end.
                    state.cursor_at_end = true;
                    state.generation = state.generation.wrapping_add(1);
                    trace_state("event Continuation", &state);
                }
                ShellEvent::Closed => {
                    clear_overlay(&mut state, &output)?;
                    running = false;
                }
            }
        }
        while let Ok(result) = result_rx.try_recv() {
            if result.generation == state.generation && result.buffer == state.buffer {
                state.suggestions = result.suggestions;
                state.selected = 0;
                render_overlay(&mut state, &output)?;
            }
        }

        if event::poll(Duration::from_millis(12)).map_err(|error| error.to_string())? {
            match event::read().map_err(|error| error.to_string())? {
                Event::Resize(columns, rows) => {
                    clear_overlay(&mut state, &output)?;
                    process
                        .resize(columns as i16, rows as i16)
                        .map_err(|error| format!("cannot resize Git Bash terminal: {error}"))?;
                    render_overlay(&mut state, &output)?;
                }
                Event::Paste(text) => {
                    handle_paste(text, &mut state, &writer, &output)?;
                }
                Event::Key(key) if accepts_key_event(&key) => {
                    handle_key(key, &mut state, &writer, &output, &query_tx)?;
                }
                _ => {}
            }
        }

        if !process.is_alive() {
            clear_overlay(&mut state, &output)?;
            running = false;
        }
    }

    clear_overlay(&mut state, &output)?;
    let _ = process.exit(1);
    Ok(())
}

#[cfg(unix)]
pub fn run(shell: LiveShell, args: Vec<String>) -> Result<(), String> {
    if !supports_unix_live_shell(shell) {
        return Err(format!(
            "live {} overlay is unavailable here; WSL2 Bash is enabled by default and macOS requires HINTSHELL_ENABLE_MACOS_LIVE_OVERLAY=1",
            shell.display_name()
        ));
    }
    if !supports_live_overlay() {
        return Err("live overlay requires an ANSI-capable terminal".to_string());
    }

    let (cols, rows) = size().unwrap_or((120, 30));
    let interactive = args.is_empty() || args.iter().any(|argument| argument == "-i");
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("cannot create Unix pseudo-terminal: {error}"))?;

    let cwd = env::current_dir()
        .map_err(|error| format!("cannot determine terminal working directory: {error}"))?;
    let mut command = CommandBuilder::new(resolve_shell(shell)?);
    // portable-pty does not reliably inherit CWD for an interactive child.
    // Preserve the terminal directory selected by the host terminal.
    command.cwd(cwd);
    command.env(shell.marker(), "1");
    // One-shot token proving this shell is the one the wrapper pumps. The hook consumes it,
    // so a nested `bash` (or an older wrapper that cannot filter markers) emits nothing and no
    // marker text can end up on screen as garbage.
    command.env(MARKER_TOKEN_ENV, std::process::id().to_string());
    if shell == LiveShell::Bash && is_wsl_runtime() {
        command.env("HINTSHELL_LIVE_BASH_WSL", "1");
    }
    if args.is_empty() {
        match shell {
            LiveShell::Bash => command.args(["--login", "-i"]),
            LiveShell::Zsh => command.arg("-i"),
        }
    } else {
        command.args(args.iter());
    }

    let mut process = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("cannot start {}: {error}", shell.display_name()))?;
    drop(pair.slave);

    let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(|error| {
        format!("cannot open {} input: {error}", shell.display_name())
    })?));
    let output = Arc::new(Mutex::new(io::stdout()));
    let (shell_events_tx, shell_events_rx) = mpsc::channel::<ShellEvent>();
    let query_cwd = Arc::new(Mutex::new(
        env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().to_string()),
    ));
    let output_reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("cannot open {} output: {error}", shell.display_name()))?;
    start_output_pump(output_reader, shell_events_tx, Some(Arc::clone(&query_cwd)));

    if !interactive {
        return run_unix_batch_process(&mut *process, shell_events_rx, &output);
    }

    let (query_tx, query_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    start_query_worker(query_rx, result_tx, query_cwd, shell.request_shell());

    let mut raw_mode = RawModeGuard::enter()?;
    let mut state = OverlayState::new();
    let mut input_mode = UnixInputMode::ChildPassthrough;
    let mut running = true;

    while running {
        while let Ok(shell_event) = shell_events_rx.try_recv() {
            match shell_event {
                ShellEvent::Output(bytes) => write_terminal(&output, &bytes)?,
                ShellEvent::StartupPhase(_) => {}
                ShellEvent::PromptReady => {
                    raw_mode.enable_bracketed_paste()?;
                    input_mode = UnixInputMode::PromptOverlay;
                    state.clear();
                    state.buffer.clear();
                    state.tracking_valid = true;
                    state.cursor_at_end = true;
                    state.dismissed = false;
                    state.generation = state.generation.wrapping_add(1);
                }
                ShellEvent::Continuation => {
                    clear_overlay(&mut state, &output)?;
                    state.buffer.clear();
                    state.clear();
                    state.tracking_valid = false;
                    state.cursor_at_end = true;
                    state.generation = state.generation.wrapping_add(1);
                }
                ShellEvent::Closed => running = false,
            }
        }
        if input_mode == UnixInputMode::PromptOverlay {
            while let Ok(result) = result_rx.try_recv() {
                if result.generation == state.generation && result.buffer == state.buffer {
                    state.suggestions = result.suggestions;
                    state.selected = 0;
                    render_overlay(&mut state, &output)?;
                }
            }
        }

        match input_mode {
            UnixInputMode::PromptOverlay => {
                if event::poll(Duration::from_millis(12)).map_err(|error| error.to_string())? {
                    match event::read().map_err(|error| error.to_string())? {
                        Event::Resize(columns, rows) => {
                            clear_overlay(&mut state, &output)?;
                            pair.master
                                .resize(PtySize {
                                    rows,
                                    cols: columns,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                })
                                .map_err(|error| {
                                    format!(
                                        "cannot resize {} terminal: {error}",
                                        shell.display_name()
                                    )
                                })?;
                            render_overlay(&mut state, &output)?;
                        }
                        Event::Paste(text) => {
                            handle_paste(text, &mut state, &writer, &output)?;
                        }
                        Event::Key(key) if accepts_key_event(&key) => {
                            let submitted = key.code == KeyCode::Enter;
                            handle_key(key, &mut state, &writer, &output, &query_tx)?;
                            if submitted {
                                raw_mode.disable_bracketed_paste();
                                input_mode = UnixInputMode::ChildPassthrough;
                            }
                        }
                        _ => {}
                    }
                }
            }
            UnixInputMode::ChildPassthrough => {
                forward_stdin_to_shell(&writer)?;
                thread::sleep(Duration::from_millis(4));
            }
        }

        if process
            .try_wait()
            .map_err(|error| format!("cannot check Unix shell: {error}"))?
            .is_some()
        {
            clear_overlay(&mut state, &output)?;
            running = false;
        }
    }

    clear_overlay(&mut state, &output)?;
    let _ = process.kill();
    Ok(())
}

#[cfg(unix)]
fn run_unix_batch_process(
    process: &mut dyn portable_pty::Child,
    events: mpsc::Receiver<ShellEvent>,
    output: &Arc<Mutex<io::Stdout>>,
) -> Result<(), String> {
    loop {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(ShellEvent::Output(bytes)) => write_terminal(output, &bytes)?,
            Ok(ShellEvent::StartupPhase(_))
            | Ok(ShellEvent::PromptReady)
            | Ok(ShellEvent::Continuation) => {}
            Ok(ShellEvent::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout)
                if process
                    .try_wait()
                    .map_err(|error| format!("cannot check Unix shell: {error}"))?
                    .is_some() =>
            {
                while let Ok(ShellEvent::Output(bytes)) = events.try_recv() {
                    write_terminal(output, &bytes)?;
                }
                return Ok(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(not(any(windows, unix)))]
pub fn run(_shell: LiveShell, _args: Vec<String>) -> Result<(), String> {
    Err("live shell overlay is unsupported on this platform".to_string())
}

#[cfg(unix)]
fn supports_unix_live_shell(shell: LiveShell) -> bool {
    match shell {
        LiveShell::Bash => is_wsl_runtime() || is_macos_live_overlay_enabled(),
        LiveShell::Zsh => is_macos_live_overlay_enabled(),
    }
}

#[cfg(unix)]
fn is_macos_live_overlay_enabled() -> bool {
    cfg!(target_os = "macos") && env::var_os("HINTSHELL_ENABLE_MACOS_LIVE_OVERLAY").is_some()
}

#[cfg(unix)]
fn is_wsl_runtime() -> bool {
    is_wsl_environment(
        env::var_os("WSL_INTEROP").is_some(),
        env::var_os("WSL_DISTRO_NAME").is_some(),
    )
}

#[cfg(any(unix, test))]
fn is_wsl_environment(has_interop: bool, has_distro_name: bool) -> bool {
    has_interop || has_distro_name
}

#[cfg(windows)]
fn run_batch_process(
    process: conpty::Process,
    events: mpsc::Receiver<ShellEvent>,
    output: &Arc<Mutex<io::Stdout>>,
) -> Result<(), String> {
    loop {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok(ShellEvent::Output(bytes)) => write_terminal(output, &bytes)?,
            Ok(ShellEvent::StartupPhase(_)) | Ok(ShellEvent::PromptReady) | Ok(ShellEvent::Continuation) => {}
            Ok(ShellEvent::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) if !process.is_alive() => {
                while let Ok(ShellEvent::Output(bytes)) = events.try_recv() {
                    write_terminal(output, &bytes)?;
                }
                return Ok(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn accepts_key_event(key: &KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn supports_live_overlay() -> bool {
    if env::var("HINTSHELL_DISABLE_LIVE_OVERLAY").is_ok() {
        return false;
    }

    let term = env::var("TERM").unwrap_or_default().to_ascii_lowercase();
    if term == "dumb" {
        return false;
    }
    if !term.is_empty() {
        return true;
    }

    // On Windows, ConPTY is standard and supported across modern Windows terminals.
    #[cfg(windows)]
    {
        true
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn quote_windows_argument(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    if !value.contains([' ', '\t', '"']) {
        return value.to_string();
    }

    format!("\"{}\"", value.replace('"', "\\\""))
}

#[cfg(unix)]
fn resolve_shell(shell: LiveShell) -> Result<String, String> {
    match shell {
        LiveShell::Bash => resolve_bash(),
        LiveShell::Zsh => Ok(env::var("HINTSHELL_ZSH").unwrap_or_else(|_| "zsh".to_string())),
    }
}

fn resolve_bash() -> Result<String, String> {
    if let Ok(path) = env::var("HINTSHELL_BASH") {
        return Ok(path);
    }

    #[cfg(windows)]
    {
        let candidates = [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
        ];
        for candidate in candidates {
            if std::path::Path::new(candidate).is_file() {
                return Ok(candidate.to_string());
            }
        }
        Ok("bash.exe".to_string())
    }

    #[cfg(not(windows))]
    {
        Ok("bash".to_string())
    }
}

#[derive(Debug)]
enum ShellEvent {
    Output(Vec<u8>),
    StartupPhase(String),
    PromptReady,
    /// readline is showing its continuation prompt: an open quote or heredoc, so the
    /// shadowed buffer no longer describes anything the suggestion engine can rank.
    Continuation,
    Closed,
}

/// Marker wire format: a private OSC sequence (5522) terminated by BEL.
///
/// Bare control bytes cannot be used on Windows. ConPTY re-maps them through the console
/// codepage, so the byte 0x1e arrives as the printable glyph "▲" and no filter recognises it
/// — that is how "▲HINTSHELL_CWD:C:\Users\me▼" landed on users' screens. Escape sequences
/// pass through ConPTY unchanged, and a terminal that does not implement OSC 5522 ignores it,
/// so even a leaked marker is invisible instead of garbage.
const OSC_CWD: &[u8] = b"\x1b]5522;hintshell_cwd=";
const OSC_PROMPT: &[u8] = b"\x1b]5522;hintshell_prompt";
const OSC_CONTINUATION: &[u8] = b"\x1b]5522;hintshell_continuation";
const OSC_PHASE: &[u8] = b"\x1b]5522;hintshell_phase=";
const OSC_END: &[u8] = b"\x07";

/// Legacy RS/US delimited shapes, still stripped so a shell that started with an older hook
/// does not print marker text into a newer wrapper.
const LEGACY_CWD: &[u8] = b"\x1eHINTSHELL_CWD:";
const LEGACY_PROMPT: &[u8] = b"\x1eHINTSHELL_PROMPT\x1f";
const LEGACY_CONTINUATION: &[u8] = b"\x1eHINTSHELL_CONTINUATION\x1f";
const LEGACY_PHASE: &[u8] = b"\x1eHINTSHELL_STARTUP_PHASE:";
const LEGACY_END: &[u8] = b"\x1f";

/// The same legacy shapes after Windows ConPTY has re-mapped the control bytes through the
/// console codepage: 0x1e arrives as "\u{25b2}" and 0x1f as "\u{25bc}". A stale hook or a stale
/// generated rcfile cannot be updated from the wrapper, so their markers are neutralised here.
/// The "HINTSHELL" keyword makes a false match on ordinary shell output practically impossible.
const GLYPH_CWD: &[u8] = "\u{25b2}HINTSHELL_CWD:".as_bytes();
const GLYPH_PROMPT: &[u8] = "\u{25b2}HINTSHELL_PROMPT\u{25bc}".as_bytes();
const GLYPH_CONTINUATION: &[u8] = "\u{25b2}HINTSHELL_CONTINUATION\u{25bc}".as_bytes();
const GLYPH_PHASE: &[u8] = "\u{25b2}HINTSHELL_STARTUP_PHASE:".as_bytes();
const GLYPH_END: &[u8] = "\u{25bc}".as_bytes();

/// (start, end) shapes of one marker; an empty end means the start is the whole marker.
type MarkerForms<'a> = &'a [(&'a [u8], &'a [u8])];

const CWD_FORMS: MarkerForms = &[
    (OSC_CWD, OSC_END),
    (LEGACY_CWD, LEGACY_END),
    (GLYPH_CWD, GLYPH_END),
];
const PROMPT_FORMS: MarkerForms = &[
    (OSC_PROMPT, OSC_END),
    (LEGACY_PROMPT, &[]),
    (GLYPH_PROMPT, &[]),
];
const CONTINUATION_FORMS: MarkerForms = &[
    (OSC_CONTINUATION, OSC_END),
    (LEGACY_CONTINUATION, &[]),
    (GLYPH_CONTINUATION, &[]),
];
const PHASE_FORMS: MarkerForms = &[
    (OSC_PHASE, OSC_END),
    (LEGACY_PHASE, LEGACY_END),
    (GLYPH_PHASE, GLYPH_END),
];

/// Environment variable carrying the one-shot marker token from the wrapper to its shell.
pub const MARKER_TOKEN_ENV: &str = "HINTSHELL_MARKER_TOKEN";

fn start_output_pump(
    mut reader: Box<dyn Read + Send>,
    events: mpsc::Sender<ShellEvent>,
    cwd: Option<Arc<Mutex<Option<String>>>>,
) {
    let mut prompt_marker_buffer = Vec::new();
    let mut continuation_marker_buffer = Vec::new();
    let mut cwd_marker_buffer = Vec::new();
    let mut startup_phase_marker_buffer = Vec::new();
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let bytes = filter_cwd_markers(
                        &filter_startup_phase_markers(
                            &filter_prompt_markers(
                                &filter_continuation_markers(
                                    &buffer[..read],
                                    &mut continuation_marker_buffer,
                                    &events,
                                ),
                                &mut prompt_marker_buffer,
                                &events,
                            ),
                            &mut startup_phase_marker_buffer,
                            &events,
                        ),
                        &mut cwd_marker_buffer,
                        cwd.as_ref(),
                    );
                    if !bytes.is_empty() && events.send(ShellEvent::Output(bytes)).is_err() {
                        return;
                    }
                }
            }
        }
        if !prompt_marker_buffer.is_empty() {
            let _ = events.send(ShellEvent::Output(prompt_marker_buffer));
        }
        if !continuation_marker_buffer.is_empty() {
            let _ = events.send(ShellEvent::Output(continuation_marker_buffer));
        }
        if !cwd_marker_buffer.is_empty() {
            let _ = events.send(ShellEvent::Output(cwd_marker_buffer));
        }
        let _ = events.send(ShellEvent::Closed);
    });
}

/// Strip every known shape of one marker from the stream, reporting the payload (empty for
/// fixed markers) through `on_match`. A marker that has only partly arrived is held back
/// until the next read, so a PTY that splits a sequence anywhere still prints clean text.
fn filter_marker_forms(
    bytes: &[u8],
    pending: &mut Vec<u8>,
    forms: MarkerForms,
    mut on_match: impl FnMut(&[u8]),
) -> Vec<u8> {
    pending.extend_from_slice(bytes);
    let mut output = Vec::new();

    loop {
        // (marker start, payload start, payload end, next byte to inspect)
        let mut found: Option<(usize, usize, usize, usize)> = None;
        let mut waiting_for_end = None;

        for (start, end) in forms {
            let Some(at) = find_subsequence(pending, start) else {
                continue;
            };
            if end.is_empty() {
                let next = at + start.len();
                if found.is_none_or(|previous| at < previous.0) {
                    found = Some((at, next, next, next));
                }
                continue;
            }
            let payload_start = at + start.len();
            let Some(relative_end) = find_subsequence(&pending[payload_start..], end) else {
                // Started but not finished: print what came before and keep the rest.
                waiting_for_end = Some(at);
                break;
            };
            let payload_end = payload_start + relative_end;
            let next = payload_end + end.len();
            if found.is_none_or(|previous| at < previous.0) {
                found = Some((at, payload_start, payload_end, next));
            }
        }

        let hold_from = waiting_for_end.filter(|at| found.is_none_or(|matched| *at <= matched.0));
        if let Some(at) = hold_from {
            output.extend_from_slice(&pending[..at]);
            pending.drain(..at);
            return output;
        }

        let Some((at, payload_start, payload_end, next)) = found else {
            let keep = forms
                .iter()
                .map(|(start, _)| marker_prefix_len(pending, start))
                .max()
                .unwrap_or(0);
            let flush = pending.len().saturating_sub(keep);
            output.extend_from_slice(&pending[..flush]);
            pending.drain(..flush);
            return output;
        };

        output.extend_from_slice(&pending[..at]);
        on_match(&pending[payload_start..payload_end]);
        pending.drain(..next);
    }
}

fn filter_continuation_markers(
    bytes: &[u8],
    pending: &mut Vec<u8>,
    events: &mpsc::Sender<ShellEvent>,
) -> Vec<u8> {
    filter_marker_forms(bytes, pending, CONTINUATION_FORMS, |_| {
        let _ = events.send(ShellEvent::Continuation);
    })
}

fn filter_prompt_markers(
    bytes: &[u8],
    pending: &mut Vec<u8>,
    events: &mpsc::Sender<ShellEvent>,
) -> Vec<u8> {
    filter_marker_forms(bytes, pending, PROMPT_FORMS, |_| {
        let _ = events.send(ShellEvent::PromptReady);
    })
}

fn filter_startup_phase_markers(
    bytes: &[u8],
    pending: &mut Vec<u8>,
    events: &mpsc::Sender<ShellEvent>,
) -> Vec<u8> {
    filter_marker_forms(bytes, pending, PHASE_FORMS, |payload| {
        if let Ok(phase) = std::str::from_utf8(payload) {
            let _ = events.send(ShellEvent::StartupPhase(phase.to_string()));
        }
    })
}

fn marker_prefix_len(pending: &[u8], marker: &[u8]) -> usize {
    let max_prefix = pending.len().min(marker.len().saturating_sub(1));
    (1..=max_prefix)
        .rev()
        .find(|length| pending[pending.len() - length..] == marker[..*length])
        .unwrap_or(0)
}

fn filter_cwd_markers(
    bytes: &[u8],
    pending: &mut Vec<u8>,
    cwd: Option<&Arc<Mutex<Option<String>>>>,
) -> Vec<u8> {
    filter_marker_forms(bytes, pending, CWD_FORMS, |payload| {
        let Some(cwd) = cwd else { return };
        let Ok(path) = std::str::from_utf8(payload) else { return };
        if let Ok(mut current) = cwd.lock() {
            *current = Some(path.to_string());
        }
    })
}

fn find_subsequence(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_terminal(output: &Arc<Mutex<io::Stdout>>, bytes: &[u8]) -> Result<(), String> {
    let mut stdout = output
        .lock()
        .map_err(|_| "terminal output is unavailable".to_string())?;
    stdout
        .write_all(bytes)
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("cannot forward Git Bash output: {error}"))
}

#[derive(Debug)]
struct SuggestionResult {
    generation: u64,
    buffer: String,
    suggestions: Vec<SuggestionItem>,
}

fn query_request(input: String, cwd: Option<String>, shell: &str) -> HintShellRequest {
    HintShellRequest::Suggest {
        input,
        limit: 30,
        cwd,
        shell: Some(shell.to_string()),
    }
}

fn start_query_worker(
    receiver: mpsc::Receiver<(u64, String)>,
    sender: mpsc::Sender<SuggestionResult>,
    cwd: Arc<Mutex<Option<String>>>,
    shell: &'static str,
) {
    thread::spawn(move || {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        while let Ok((mut generation, mut buffer)) = receiver.recv() {
            thread::sleep(QUERY_DEBOUNCE);
            while let Ok((next_generation, next_buffer)) = receiver.try_recv() {
                generation = next_generation;
                buffer = next_buffer;
            }

            let cwd = cwd.lock().ok().and_then(|current| current.clone());
            let request = query_request(buffer.clone(), cwd, shell);
            let mut raw_suggestions = runtime
                .block_on(async {
                    tokio::time::timeout(QUERY_TIMEOUT, crate::send_request(&request))
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .and_then(|response| response.suggestions)
                        .unwrap_or_default()
                });
            for suggestion in &mut raw_suggestions {
                mark_destructive_without_daemon_support(suggestion);
            }

            // The engine already ranked candidates by match tier (prefix > contextual >
            // substring > typo-tolerant > fuzzy). Re-filtering to prefix/substring here hid
            // typo-tolerant and contextual results, so advertised features never appeared.
            let _ = sender.send(SuggestionResult {
                generation,
                buffer,
                suggestions: raw_suggestions,
            });
        }
    });
}

/// A daemon older than this build sends no `risk` field, and a safety warning must not
/// depend on which build answered the request, so the client re-derives it with the same
/// rule the engine uses. Never downgrades a marker the daemon already set.
fn mark_destructive_without_daemon_support(suggestion: &mut SuggestionItem) {
    if suggestion.risk != "destructive"
        && matches!(
            hintshell_core::engine::matcher::risk_for_command(&suggestion.command),
            Risk::Destructive
        )
    {
        suggestion.risk = "destructive".to_string();
    }
}

fn handle_paste(
    text: String,
    state: &mut OverlayState,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    output: &Arc<Mutex<io::Stdout>>,
) -> Result<(), String> {
    clear_overlay(state, output)?;
    state.clear();
    state.tracking_valid = false;
    // Bracketed paste leaves the cursor after the block it inserted, so a discard from there
    // empties the line and the overlay can come back without waiting for the next prompt.
    state.cursor_at_end = true;
    state.generation = state.generation.wrapping_add(1);
    write_to_shell(writer, text.as_bytes())
}

fn handle_key(
    key: KeyEvent,
    state: &mut OverlayState,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    output: &Arc<Mutex<io::Stdout>>,
    query_tx: &mpsc::Sender<(u64, String)>,
) -> Result<(), String> {
    trace_state(&format!("key {:?} {:?}", key.code, key.modifiers), state);
    match key.code {
        KeyCode::Tab => match tab_action(&state.buffer, state.current()) {
            TabAction::AcceptHintShell if state.tracking_valid => {
                let command = state.current().map(|suggestion| suggestion.command.clone());
                if let Some(command) = command {
                    clear_overlay(state, output)?;
                    state.clear();
                    // Bash redraws the current readline buffer after Ctrl-U. Sending
                    // the selected text immediately can race that redraw through ConPTY,
                    // leaving the old prefix on its own visual line. Let readline finish
                    // the discard before inserting the accepted suggestion.
                    let clean_command = command.trim_end_matches(['\r', '\n']).replace(['\r', '\n'], " ");
                    write_to_shell(writer, b"\x15")?;
                    thread::sleep(TAB_ACCEPT_CLEAR_DELAY);
                    write_to_shell(writer, clean_command.as_bytes())?;
                    state.buffer = clean_command;
                    state.generation = state.generation.wrapping_add(1);
                }
            }
            // Without a trustworthy copy of the line, Ctrl-U would throw away what the
            // user has already typed, so completion is left to Bash.
            _ => {
                clear_overlay(state, output)?;
                state.clear();
                write_to_shell(writer, b"\t")?;
            }
        },
        // Alt+Up/Down: Bash history, keeping Up/Down free for the panel.
        KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
            forward_and_invalidate(state, writer, output, b"\x10")?;
            state.cursor_at_end = true;
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
            forward_and_invalidate(state, writer, output, b"\x0e")?;
            state.cursor_at_end = true;
        }
        KeyCode::Up if !state.suggestions.is_empty() => {
            state.selected = state.selected.saturating_sub(1);
            render_overlay(state, output)?;
        }
        KeyCode::Down if !state.suggestions.is_empty() => {
            state.selected = (state.selected + 1).min(state.suggestions.len() - 1);
            render_overlay(state, output)?;
        }
        KeyCode::Up => {
            forward_and_invalidate(state, writer, output, b"\x1b[A")?;
            // Recalled history arrives complete with the cursor after its last character.
            state.cursor_at_end = true;
        }
        KeyCode::Down => {
            forward_and_invalidate(state, writer, output, b"\x1b[B")?;
            state.cursor_at_end = true;
        }
        KeyCode::Esc if !state.suggestions.is_empty() => {
            clear_overlay(state, output)?;
            state.clear();
            // "Not now" has to stick for this line; otherwise the next keystroke
            // re-queries and the panel the user just dismissed reappears.
            state.dismissed = true;
        }
        KeyCode::Esc => forward_and_invalidate(state, writer, output, b"\x1b")?,
        KeyCode::Backspace => {
            if !state.tracking_valid {
                forward_and_invalidate(state, writer, output, b"\x7f")?;
            } else {
                state.buffer.pop();
                write_to_shell(writer, b"\x7f")?;
                if state.buffer.is_empty() {
                    clear_overlay(state, output)?;
                    state.clear();
                    state.generation = state.generation.wrapping_add(1);
                } else {
                    request_suggestions(state, query_tx, output);
                }
            }
        }
        KeyCode::Enter => {
            clear_overlay(state, output)?;
            state.clear();
            state.tracking_valid = true;
            state.cursor_at_end = true;
            state.dismissed = false;
            write_to_shell(writer, b"\r")?;
            state.buffer.clear();
            state.generation = state.generation.wrapping_add(1);
        }
        // Ctrl+Space: bring the panel back after an Esc, without typing another character.
        KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.dismissed = false;
            request_suggestions(state, query_tx, output);
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
            if state.tracking_valid {
                state.buffer.push(character);
            }
            write_to_shell(writer, character.to_string().as_bytes())?;
            request_suggestions(state, query_tx, output);
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clear_overlay(state, output)?;
            state.buffer.clear();
            state.generation = state.generation.wrapping_add(1);
            write_to_shell(writer, b"\x03")?;
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clear_overlay(state, output)?;
            state.buffer.clear();
            // Discarding backwards from the end of the line empties it, so the wrapper knows
            // the line exactly again and the overlay can resume on the next keystroke. With
            // the cursor somewhere inside an unknown line, a tail would survive and the
            // shadow stays untrusted rather than claiming a line it never saw.
            if state.cursor_at_end {
                state.tracking_valid = true;
            }
            state.cursor_at_end = true;
            state.generation = state.generation.wrapping_add(1);
            write_to_shell(writer, b"\x15")?;
        }
        KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clear_overlay(state, output)?;
            state.clear();
            state.tracking_valid = false;
            state.generation = state.generation.wrapping_add(1);
            write_to_shell(writer, b"\x1a")?;
        }
        // Every other readline control chord (Ctrl+R search, Ctrl+D exit, Ctrl+L clear,
        // Ctrl+W kill word, Ctrl+A/E line ends, Ctrl+P/N history) must reach Bash: they
        // all change the line, so the shadowed buffer is dropped.
        KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(sequence) = control_sequence(&key) {
                let history_recall = matches!(key.code, KeyCode::Char('p' | 'n'));
                forward_and_invalidate(state, writer, output, &sequence)?;
                if history_recall {
                    state.cursor_at_end = true;
                }
            }
        }
        // Meta bindings (Alt+F, Alt+B, Alt+D …) are ESC-prefixed sequences; sending the
        // bare character would insert stray text instead of moving the cursor.
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::ALT) => {
            let mut sequence = vec![0x1b];
            sequence.extend_from_slice(character.encode_utf8(&mut [0_u8; 4]).as_bytes());
            forward_and_invalidate(state, writer, output, &sequence)?;
        }
        KeyCode::BackTab => forward_and_invalidate(state, writer, output, b"\x1b[Z")?,
        // Right Arrow or End: accept ghost text / full suggestion if at end of buffer. A
        // destructive command is not offered through this silent one-keypress path; it needs
        // the visible Tab affordance instead.
        KeyCode::Right | KeyCode::End if !state.suggestions.is_empty() && key.modifiers.is_empty() => {
            let accepted = state
                .current()
                .filter(|suggestion| suggestion.risk != "destructive")
                .map(|suggestion| suggestion.command.clone());
            if let Some(cmd) = accepted {
                let clean_cmd = cmd.trim_end_matches(['\r', '\n']).replace(['\r', '\n'], " ");
                if clean_cmd.to_ascii_lowercase().starts_with(&state.buffer.to_ascii_lowercase()) && clean_cmd.len() > state.buffer.len() {
                    clear_overlay(state, output)?;
                    let remaining = &clean_cmd[state.buffer.len()..];
                    write_to_shell(writer, remaining.as_bytes())?;
                    state.buffer = clean_cmd;
                    state.clear();
                    state.generation = state.generation.wrapping_add(1);
                    return Ok(());
                }
            }
            forward_and_invalidate(state, writer, output, if key.code == KeyCode::Right { b"\x1b[C" } else { b"\x05" })?;
        }
        // Alt + Right or Ctrl + Right: accept NEXT WORD of suggestion
        KeyCode::Right if !state.suggestions.is_empty() && (key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::CONTROL)) => {
            if let Some(cmd) = state.current().map(|s| s.command.clone()) {
                let clean_cmd = cmd.trim_end_matches(['\r', '\n']).replace(['\r', '\n'], " ");
                if clean_cmd.to_ascii_lowercase().starts_with(&state.buffer.to_ascii_lowercase()) && clean_cmd.len() > state.buffer.len() {
                    clear_overlay(state, output)?;
                    let remaining = &clean_cmd[state.buffer.len()..];
                    // Take leading whitespace + next word
                    let mut next_chunk = String::new();
                    let mut seen_word = false;
                    for ch in remaining.chars() {
                        if ch.is_whitespace() {
                            if seen_word {
                                break;
                            }
                            next_chunk.push(ch);
                        } else {
                            seen_word = true;
                            next_chunk.push(ch);
                        }
                    }
                    if !next_chunk.is_empty() {
                        write_to_shell(writer, next_chunk.as_bytes())?;
                        state.buffer.push_str(&next_chunk);
                        request_suggestions(state, query_tx, output);
                        return Ok(());
                    }
                }
            }
            forward_and_invalidate(state, writer, output, b"\x1b[1;5C")?;
        }
        // Cursor keys keep their modifiers: Ctrl+Left is a word motion, not a plain `←`,
        // and dropping the modifier silently changes what Bash does with the keypress.
        KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End => {
            forward_and_invalidate(state, writer, output, &cursor_sequence(&key))?
        }
        KeyCode::Delete => {
            if !state.tracking_valid {
                forward_and_invalidate(state, writer, output, b"\x1b[3~")?;
            } else {
                write_to_shell(writer, b"\x1b[3~")?;
                // Delete invalidates simple tail buffer, clear overlay cleanly
                clear_overlay(state, output)?;
                state.clear();
                state.tracking_valid = false;
                state.cursor_at_end = false;
                state.generation = state.generation.wrapping_add(1);
            }
        }
        // Nothing else may simply vanish. A wrapper that swallows PageUp steals the most
        // pressed key in a terminal, so unhandled keys are forwarded as their standard
        // escape sequences and only truly unencodable ones are dropped.
        _ => {
            if let Some(sequence) = passthrough_sequence(&key) {
                forward_and_invalidate(state, writer, output, &sequence)?;
            }
        }
    }
    Ok(())
}

fn forward_and_invalidate(
    state: &mut OverlayState,
    writer: &Arc<Mutex<Box<dyn Write + Send>>>,
    output: &Arc<Mutex<io::Stdout>>,
    bytes: &[u8],
) -> Result<(), String> {
    clear_overlay(state, output)?;
    state.clear();
    state.tracking_valid = false;
    // Conservative: unless the caller knows the key left the cursor after the line, the
    // position is unknown and Ctrl+U must not claim the line became empty.
    state.cursor_at_end = false;
    state.generation = state.generation.wrapping_add(1);
    write_to_shell(writer, bytes)
}

#[cfg(unix)]
fn forward_stdin_to_shell(writer: &Arc<Mutex<Box<dyn Write + Send>>>) -> Result<(), String> {
    use std::os::fd::AsRawFd;

    let mut ready = libc::pollfd {
        fd: io::stdin().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut ready, 1, 0) };
    if result < 0 {
        return Err(format!(
            "cannot poll terminal input: {}",
            io::Error::last_os_error()
        ));
    }
    if result == 0 || ready.revents & libc::POLLIN == 0 {
        return Ok(());
    }

    let mut buffer = [0_u8; 4096];
    let read = io::stdin()
        .read(&mut buffer)
        .map_err(|error| format!("cannot read terminal input: {error}"))?;
    if read == 0 {
        return Ok(());
    }
    write_to_shell(writer, &buffer[..read])
}

fn write_to_shell(writer: &Arc<Mutex<Box<dyn Write + Send>>>, bytes: &[u8]) -> Result<(), String> {
    let mut writer = writer
        .lock()
        .map_err(|_| "Git Bash input bridge is unavailable".to_string())?;
    writer
        .write_all(bytes)
        .and_then(|_| writer.flush())
        .map_err(|error| format!("cannot write to Git Bash: {error}"))
}

fn request_suggestions(state: &mut OverlayState, sender: &mpsc::Sender<(u64, String)>, output: &Arc<Mutex<io::Stdout>>) {
    state.generation = state.generation.wrapping_add(1);
    state.clear();
    if !should_query(state) {
        trace_state("skip", state);
        if state.rendered_lines > 0 || state.ghost_cols > 0 {
            let _ = clear_overlay(state, output);
        }
        return;
    }
    trace_state("query", state);
    let _ = sender.send((state.generation, state.buffer.clone()));
}

/// `HINTSHELL_TRACE_KEYS=1` appends one line per keystroke, per query decision and per shell
/// event to ~/.hintshell/keys-trace.log. The overlay only exists on screen: without this, the
/// state machine behind a missing panel cannot be inspected from a test, and a user has no way
/// to report what the wrapper believed at the moment it stopped suggesting.
fn trace_state(action: &str, state: &OverlayState) {
    if env::var_os("HINTSHELL_TRACE_KEYS").is_none() {
        return;
    }
    let line = format!(
        "{action} tracked={} dismissed={} buf={:?} rows={} rendered={} gen={}\n",
        state.tracking_valid,
        state.dismissed,
        state.buffer,
        state.suggestions.len(),
        state.rendered_lines,
        state.generation
    );
    let path = crate::shell::hintshell_home().join("keys-trace.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Whether the current line state is worth querying at all: after Esc the panel must stay
/// away, and an untracked or non-command buffer would only suggest the wrong thing.
fn should_query(state: &OverlayState) -> bool {
    !state.dismissed && state.tracking_valid && is_suggestable(&state.buffer)
}

fn is_suggestable(buffer: &str) -> bool {
    !buffer.trim().is_empty()
        && !buffer.contains(['\n', '\r'])
        && !current_token(buffer).starts_with('-')
        && !looks_like_path(current_token(buffer))
}

fn tab_action(buffer: &str, suggestion: Option<&SuggestionItem>) -> TabAction {
    match suggestion {
        Some(suggestion)
            if is_suggestable(buffer)
                && (suggestion
                    .command
                    .to_ascii_lowercase()
                    .starts_with(&buffer.to_ascii_lowercase())
                    || suggestion
                        .command
                        .to_ascii_lowercase()
                        .contains(&buffer.to_ascii_lowercase())) =>
        {
            TabAction::AcceptHintShell
        }
        _ => TabAction::ForwardToBash,
    }
}

fn current_token(buffer: &str) -> &str {
    buffer.split_whitespace().last().unwrap_or_default()
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with('.')
        || token.starts_with('~')
        || token.contains('/')
        || token.contains('\\')
        || token.contains(':')
}

/// Readline control chords that HintShell does not consume itself must reach Bash as their
/// raw bytes; dropping them silently disabled Ctrl+R, Ctrl+D, Ctrl+L, Ctrl+W and friends.
fn control_sequence(key: &KeyEvent) -> Option<Vec<u8>> {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    let character = match key.code {
        KeyCode::Char(character) => character,
        _ => return None,
    };
    let byte = match character {
        ' ' | '@' => 0x00,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' | '-' => 0x1f,
        other => {
            let lowered = other.to_ascii_lowercase();
            if lowered.is_ascii_alphabetic() {
                (lowered as u8) - 0x60
            } else {
                return None;
            }
        }
    };
    Some(vec![byte])
}

/// Escape sequence a terminal would have sent for a cursor key, modifiers included. Without
/// a modifier Bash's own readline bindings are used (`\x01`/`\x05`), which is what a plain
/// Home/End must keep doing for the live wrapper to be invisible.
fn cursor_sequence(key: &KeyEvent) -> Vec<u8> {
    let modifier_mask = 1
        + key.modifiers.contains(KeyModifiers::SHIFT) as u8
        + key.modifiers.contains(KeyModifiers::ALT) as u8 * 2
        + key.modifiers.contains(KeyModifiers::CONTROL) as u8 * 4;
    if modifier_mask == 1 {
        return match key.code {
            KeyCode::Left => b"\x1b[D".to_vec(),
            KeyCode::Right => b"\x1b[C".to_vec(),
            KeyCode::Home => vec![0x01],
            KeyCode::End => vec![0x05],
            _ => Vec::new(),
        };
    }
    let final_byte = match key.code {
        KeyCode::Left => b'D',
        KeyCode::Right => b'C',
        KeyCode::Home => b'H',
        KeyCode::End => b'F',
        _ => return Vec::new(),
    };
    vec![0x1b, b'[', b'1', b';', b'0' + modifier_mask, final_byte]
}

/// Escape sequence for a key the overlay does not consume, or `None` when there is no
/// standard encoding to send.
fn passthrough_sequence(key: &KeyEvent) -> Option<Vec<u8>> {
    let ss3 = match key.code {
        KeyCode::F(1) => Some(b'P'),
        KeyCode::F(2) => Some(b'Q'),
        KeyCode::F(3) => Some(b'R'),
        KeyCode::F(4) => Some(b'S'),
        _ => None,
    };
    if let Some(letter) = ss3 {
        return Some(vec![0x1b, b'O', letter]);
    }
    let number = match key.code {
        KeyCode::Insert => 2,
        KeyCode::PageUp => 5,
        KeyCode::PageDown => 6,
        KeyCode::F(5) => 15,
        KeyCode::F(6) => 17,
        KeyCode::F(7) => 18,
        KeyCode::F(8) => 19,
        KeyCode::F(9) => 20,
        KeyCode::F(10) => 21,
        KeyCode::F(11) => 23,
        KeyCode::F(12) => 24,
        _ => return None,
    };
    let modifier = 1
        + key.modifiers.contains(KeyModifiers::SHIFT) as u8
        + key.modifiers.contains(KeyModifiers::ALT) as u8 * 2
        + key.modifiers.contains(KeyModifiers::CONTROL) as u8 * 4;
    let sequence = if modifier == 1 {
        format!("\x1b[{number}~")
    } else {
        format!("\x1b[{number};{modifier}~")
    };
    Some(sequence.into_bytes())
}

fn clear_overlay(state: &mut OverlayState, output: &Arc<Mutex<io::Stdout>>) -> Result<(), String> {
    if state.rendered_lines == 0 && state.ghost_cols == 0 {
        return Ok(());
    }

    let (cur_col, cur_row) = position().unwrap_or((0, 0));
    let mut stdout = output
        .lock()
        .map_err(|_| "terminal output is unavailable".to_string())?;

    let mut frame = String::new();
    for _ in 0..state.rendered_lines {
        frame.push_str("\x1b[1B\r\x1b[2K");
    }
    // Ghost text lives on the prompt line to the right of the cursor; erase-to-EOL only
    // touches it because ghost drawing requires an end-of-line cursor (tracking valid).
    let restore_ghost = state.ghost_cols > 0;
    state.rendered_lines = 0;
    state.ghost_cols = 0;
    stdout
        .write_all(frame.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("cannot clear HintShell overlay: {error}"))?;

    let _ = execute!(stdout, crossterm::cursor::MoveTo(cur_col, cur_row));
    if restore_ghost {
        let _ = stdout.write_all(b"\x1b[K");
    }
    let _ = stdout.flush();

    Ok(())
}

fn render_overlay(state: &mut OverlayState, output: &Arc<Mutex<io::Stdout>>) -> Result<(), String> {
    if state.suggestions.is_empty() {
        if state.rendered_lines > 0 || state.ghost_cols > 0 {
            clear_overlay(state, output)?;
        }
        return Ok(());
    }

    let config = crate::config::HintShellConfig::load();
    let is_rainbow = config.is_rainbow();
    let is_apple = config.is_apple();
    let border_color_ansi = config.border_ansi_code();

    let (terminal_width, terminal_height) = size().unwrap_or((100, 30));
    let width = overlay_width(terminal_width);
    let total = state.suggestions.len();
    let (cur_col, cur_row) = position().unwrap_or((0, 0));
    // A frame wider than the terminal wraps, which desynchronises every row we track, so
    // very narrow panes fall back to ghost text instead of drawing a broken box.
    let box_fits = usize::from(terminal_width) > width;
    let shown = if box_fits {
        visible_suggestion_rows(cur_row, terminal_height, total, config.max_visible)
    } else {
        0
    };
    if shown == 0 {
        if state.rendered_lines > 0 {
            clear_overlay(state, output)?;
        }
        // No vertical room: keep suggestions reachable instead of dropping them silently,
        // which is what the PowerShell surface already does in the same situation.
        if config.ghost_text {
            render_ghost(state, output, cur_col, cur_row, terminal_width)?;
        }
        return Ok(());
    }

    let inner_width = overlay_inner_width(width);
    let command_width = overlay_command_width(width);
    let compact = overlay_is_compact(width);
    let old_rendered_lines = state.rendered_lines;
    let rendered_lines = shown + 3;
    let selected = state.selected.min(total.saturating_sub(1));
    let viewport_start = viewport_start(selected, total, shown);
    let viewport_end = viewport_start + shown;
    let mut frame = String::new();
    let counter = format!(" {}/{} ", selected + 1, total);
    let selected_style = selected_row_style(is_apple);

    // Rainbow colors array for Gemini style gradient: Blue -> Cyan -> Green -> Yellow -> Orange -> Pink -> Purple
    let rainbow_colors = [
        "\x1b[38;5;75m",  // Blue
        "\x1b[38;5;51m",  // Cyan
        "\x1b[38;5;48m",  // Green
        "\x1b[38;5;220m", // Yellow
        "\x1b[38;5;208m", // Orange
        "\x1b[38;5;212m", // Pink
        "\x1b[38;5;141m", // Purple
        "\x1b[38;5;69m",  // Indigo
    ];

    let perimeter_top = inner_width.saturating_sub(counter.len());
    let perimeter_right = shown + 1;
    let perimeter_bottom = inner_width;
    let perimeter_left = shown + 1;
    let total_perimeter = (perimeter_top + perimeter_right + perimeter_bottom + perimeter_left).max(1);

    if is_apple {
        let mut top_line = String::new();
        let c_corner = rgb_ansi(apple_perimeter_color(0, total_perimeter));
        top_line.push_str(&format!("{c_corner}╭"));
        for i in 0..perimeter_top {
            let c = rgb_ansi(apple_perimeter_color(1 + i, total_perimeter));
            top_line.push_str(&format!("{c}─"));
        }
        let c_end = rgb_ansi(apple_perimeter_color(perimeter_top, total_perimeter));
        top_line.push_str(&format!("\x1b[1m\x1b[38;2;255;255;255m{counter}{c_end}╮\x1b[0m"));
        frame.push_str(&format!("\x1b[1B\r\x1b[2K{top_line}"));
    } else if is_rainbow {
        let repeat_dashes = inner_width.saturating_sub(counter.len());
        let mut top_line = String::new();
        top_line.push_str("\x1b[38;5;75m╭");
        for (i, _) in (0..repeat_dashes).enumerate() {
            let color = rainbow_colors[i % rainbow_colors.len()];
            top_line.push_str(&format!("{color}─"));
        }
        top_line.push_str(&format!("\x1b[38;5;141m{counter}╮\x1b[0m"));
        frame.push_str(&format!("\x1b[1B\r\x1b[2K{top_line}"));
    } else {
        frame.push_str(&format!(
            "\x1b[1B\r\x1b[2K{border_color_ansi}╭{}{}╮\x1b[0m",
            "─".repeat(inner_width.saturating_sub(counter.len())),
            counter
        ));
    }

    for (index, suggestion) in state.suggestions[viewport_start..viewport_end]
        .iter()
        .enumerate()
    {
        let suggestion_index = viewport_start + index;
        let selected_row = suggestion_index == selected;
        let destructive = suggestion.risk == "destructive";
        let command = fit_width(&suggestion.command, command_width);
        let marker = if selected_row { "▶" } else { " " };
        let highlighted_command = highlight_match(
            &command,
            &state.buffer,
            if selected_row { &selected_style } else { "" },
            command_width,
            destructive,
        );

        // Full layout explains where the item came from and how it matched; compact layout
        // spends those columns on the command itself.
        let info_field = if compact {
            String::new()
        } else {
            let mut info = hintshell_core::engine::matcher::source_label(&suggestion.source).to_string();
            let tag = hintshell_core::engine::matcher::tier_tag(suggestion.tier);
            if !tag.is_empty() {
                info.push(' ');
                info.push_str(tag);
            }
            let frequency = hintshell_core::engine::matcher::frequency_label(suggestion.frequency);
            if !frequency.is_empty() {
                info.push(' ');
                info.push_str(&frequency);
            }
            if destructive {
                info = format!("! {info}");
            }
            pad_to_width(&fit_width(&info, INFO_FIELD_WIDTH), INFO_FIELD_WIDTH)
        };

        let (row_border_left, row_border_right) = if is_apple {
            let left_step = total_perimeter.saturating_sub(1 + index);
            let right_step = perimeter_top + 1 + index;
            (
                rgb_ansi(apple_perimeter_color(left_step, total_perimeter)),
                rgb_ansi(apple_perimeter_color(right_step, total_perimeter)),
            )
        } else if is_rainbow {
            (
                rainbow_colors[index % rainbow_colors.len()].to_string(),
                rainbow_colors[(index + 3) % rainbow_colors.len()].to_string(),
            )
        } else {
            (border_color_ansi.to_string(), border_color_ansi.to_string())
        };

        let content = if compact {
            format!(" {marker} {highlighted_command} ")
        } else {
            format!(" {marker} {highlighted_command}  {info_field} ")
        };

        if selected_row {
            frame.push_str(&format!(
                "\x1b[1B\r\x1b[2K{row_border_left}│{selected_style}{content}\x1b[0m{row_border_right}│\x1b[0m"
            ));
        } else {
            frame.push_str(&format!(
                "\x1b[1B\r\x1b[2K{row_border_left}│\x1b[0m{content}{row_border_right}│\x1b[0m"
            ));
        }
    }

    let selected_item = &state.suggestions[selected];
    let destructive = selected_item.risk == "destructive";
    let description = match selected_item.description.as_deref() {
        Some(text) if !text.trim().is_empty() => text.to_string(),
        // "no description" teaches nothing; usage count and provenance do, and the engine
        // already carries both for history entries.
        _ if selected_item.frequency > 0 => format!(
            "used {}x · {}",
            selected_item.frequency,
            hintshell_core::engine::matcher::source_label(&selected_item.source)
        ),
        _ => hintshell_core::engine::matcher::NO_DESCRIPTION.to_string(),
    };
    let description = if destructive {
        format!("[!] risky · {description}")
    } else {
        description
    };
    let hint = if compact {
        ""
    } else {
        "Tab/→ accept · Alt+→ word"
    };
    let hint_gap = if hint.is_empty() { 0 } else { 2 };
    let description_budget =
        inner_width.saturating_sub(2 + hint_gap + display_width(hint));
    let description = fit_width(&description, description_budget);
    let (desc_border_left, desc_border_right) = if is_apple {
        let left_step = total_perimeter.saturating_sub(1 + shown);
        let right_step = perimeter_top + 1 + shown;
        (
            rgb_ansi(apple_perimeter_color(left_step, total_perimeter)),
            rgb_ansi(apple_perimeter_color(right_step, total_perimeter)),
        )
    } else if is_rainbow {
        ("\x1b[38;5;212m".to_string(), "\x1b[38;5;212m".to_string())
    } else {
        (border_color_ansi.to_string(), border_color_ansi.to_string())
    };
    let used = 2 + display_width(&description) + hint_gap + display_width(hint);
    let desc_pad = inner_width.saturating_sub(used);
    frame.push_str(&format!(
        "\x1b[1B\r\x1b[2K{desc_border_left}│\x1b[38;5;244m  {description}{}{}{hint}\x1b[0m{desc_border_right}│\x1b[0m",
        " ".repeat(desc_pad),
        " ".repeat(hint_gap)
    ));

    if is_apple {
        let mut bot_line = String::new();
        let bottom_start_step = perimeter_top + perimeter_right;
        let c_corner_left = rgb_ansi(apple_perimeter_color(total_perimeter.saturating_sub(shown + 2), total_perimeter));
        let c_corner_right = rgb_ansi(apple_perimeter_color(bottom_start_step, total_perimeter));
        
        bot_line.push_str(&format!("{c_corner_left}╰"));
        for i in 0..perimeter_bottom {
            let step = bottom_start_step + (perimeter_bottom - 1 - i);
            let c = rgb_ansi(apple_perimeter_color(step, total_perimeter));
            bot_line.push_str(&format!("{c}─"));
        }
        bot_line.push_str(&format!("{c_corner_right}╯\x1b[0m"));
        frame.push_str(&format!("\x1b[1B\r\x1b[2K{bot_line}"));
    } else if is_rainbow {
        let mut bot_line = String::new();
        bot_line.push_str("\x1b[38;5;141m╰");
        for (i, _) in (0..inner_width).enumerate() {
            let color = rainbow_colors[(rainbow_colors.len() - 1 - (i % rainbow_colors.len())) % rainbow_colors.len()];
            bot_line.push_str(&format!("{color}─"));
        }
        bot_line.push_str("\x1b[38;5;75m╯\x1b[0m");
        frame.push_str(&format!("\x1b[1B\r\x1b[2K{bot_line}"));
    } else {
        frame.push_str(&format!(
            "\x1b[1B\r\x1b[2K{border_color_ansi}╰{}╯\x1b[0m",
            "─".repeat(inner_width)
        ));
    }

    // If the previous overlay had more lines than the new one, clear the trailing lines
    if old_rendered_lines > rendered_lines {
        for _ in rendered_lines..old_rendered_lines {
            frame.push_str("\x1b[1B\r\x1b[2K");
        }
    }

    let mut stdout = output
        .lock()
        .map_err(|_| "terminal output is unavailable".to_string())?;
    stdout
        .write_all(frame.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("cannot render HintShell overlay: {error}"))?;

    let _ = execute!(stdout, crossterm::cursor::MoveTo(cur_col, cur_row));
    let _ = stdout.flush();

    state.rendered_lines = rendered_lines;
    Ok(())
}

// Apple Intelligence Glow: Interpolates smoothly across a perimeter of length `total_steps`
// Key glow colors: Cyan (#00f5d4) -> Siri Blue (#00bbf9) -> Violet (#7209b7) -> Magenta (#f72585) -> Coral/Amber (#ff9e00)
fn apple_perimeter_color(step: usize, total_steps: usize) -> (u8, u8, u8) {
    let stops: &[(f32, (u8, u8, u8))] = &[
        (0.00, (0, 245, 212)),   // 1. Neon Cyan
        (0.20, (0, 187, 249)),   // 2. Siri Blue
        (0.40, (114, 9, 183)),   // 3. Electric Violet
        (0.65, (247, 37, 133)),  // 4. Hot Apple Magenta
        (0.85, (255, 158, 0)),   // 5. Warm Coral Glow
        (1.00, (0, 245, 212)),   // 6. Wrap around to Neon Cyan
    ];

    if total_steps == 0 {
        return (0, 245, 212);
    }

    let progress = (step as f32 / total_steps as f32).clamp(0.0, 1.0);
    for window in stops.windows(2) {
        let (p0, c0) = window[0];
        let (p1, c1) = window[1];
        if progress >= p0 && progress <= p1 {
            let t = if (p1 - p0) > 0.0 { (progress - p0) / (p1 - p0) } else { 0.0 };
            let r = (c0.0 as f32 + t * (c1.0 as f32 - c0.0 as f32)).round() as u8;
            let g = (c0.1 as f32 + t * (c1.1 as f32 - c0.1 as f32)).round() as u8;
            let b = (c0.2 as f32 + t * (c1.2 as f32 - c0.2 as f32)).round() as u8;
            return (r, g, b);
        }
    }
    (0, 245, 212)
}

fn rgb_ansi(rgb: (u8, u8, u8)) -> String {
    if crate::config::supports_truecolor() {
        format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2)
    } else {
        format!("\x1b[38;5;{}m", truecolor_to_256(rgb))
    }
}

fn rgb_bg_ansi(rgb: (u8, u8, u8)) -> String {
    if crate::config::supports_truecolor() {
        format!("\x1b[48;2;{};{};{}m", rgb.0, rgb.1, rgb.2)
    } else {
        format!("\x1b[48;5;{}m", truecolor_to_256(rgb))
    }
}

/// Nearest colour in the 6x6x6 cube, so gradient themes stay recognisable instead of
/// turning into escape-sequence noise on Conhost or `TERM=xterm-256color`.
fn truecolor_to_256((red, green, blue): (u8, u8, u8)) -> u8 {
    let quantize = |value: u8| (usize::from(value) * 5 / 255).min(5);
    (16 + 36 * quantize(red) + 6 * quantize(green) + quantize(blue)) as u8
}

/// Frosted Siri-blue for the Apple presets, muted indigo elsewhere.
const APPLE_SELECTION_RGB: (u8, u8, u8) = (54, 87, 112);

fn selected_row_style(is_apple: bool) -> String {
    if is_apple {
        format!(
            "{}\x1b[38;2;255;255;255m\x1b[1m",
            rgb_bg_ansi(APPLE_SELECTION_RGB)
        )
    } else {
        "\x1b[48;5;60m\x1b[38;5;255m".to_string()
    }
}

fn highlight_match(
    command: &str,
    buffer: &str,
    selected_style: &str,
    target_width: usize,
    destructive: bool,
) -> String {
    let clean_cmd = command.replace(['\r', '\n', '\t'], " ");
    let buf_lower = buffer.trim().to_ascii_lowercase();
    let cmd_lower = clean_cmd.to_ascii_lowercase();
    if buf_lower.is_empty() {
        return pad_to_width(&clean_cmd, target_width);
    }

    if let Some(pos) = cmd_lower.find(&buf_lower) {
        let match_len = buf_lower.len();
        if !clean_cmd.is_char_boundary(pos) || !clean_cmd.is_char_boundary(pos + match_len) {
            return pad_to_width(&clean_cmd, target_width);
        }
        let before = &clean_cmd[..pos];
        let matched = &clean_cmd[pos..pos + match_len];
        let after = &clean_cmd[pos + match_len..];

        // The tail has to go back to the row's own style, otherwise the selected row shows
        // a colour seam right after the typed prefix.
        let reset = if selected_style.is_empty() { "\x1b[0m" } else { selected_style };
        // Yellow marks the part the user typed; red marks a command that can delete work.
        // Unselected rows sit on the user's own background, so they are also underlined:
        // a bare yellow would vanish on light themes and colour would be the only cue.
        let highlight_code = if destructive {
            if selected_style.is_empty() {
                "\x1b[1;4;38;5;203m"
            } else {
                "\x1b[1;38;5;203m"
            }
        } else if selected_style.is_empty() {
            "\x1b[1;4;38;5;220m"
        } else {
            "\x1b[1;38;5;220m"
        };
        let display_str = format!("{before}{highlight_code}{matched}{reset}{after}");
        let pad_len = target_width.saturating_sub(display_width(&clean_cmd));
        format!("{}{}", display_str, " ".repeat(pad_len))
    } else {
        pad_to_width(&clean_cmd, target_width)
    }
}

/// Below this frame width the info column costs more than it tells, so it is dropped and
/// the columns go to the command itself.
const COMPACT_OVERLAY_WIDTH: usize = 64;
/// Width of the "here ~ 12x" explanation column in the full layout.
const INFO_FIELD_WIDTH: usize = 20;

fn overlay_width(terminal_width: u16) -> usize {
    // Never wider than the terminal: a frame that wraps makes the row bookkeeping wrong and
    // leaves residue behind, which is the bug class already fixed in the PowerShell overlay.
    usize::from(terminal_width)
        .saturating_sub(1)
        .clamp(24, 78)
}

fn overlay_is_compact(width: usize) -> bool {
    width < COMPACT_OVERLAY_WIDTH
}

fn overlay_inner_width(width: usize) -> usize {
    width.saturating_sub(2)
}

fn overlay_command_width(width: usize) -> usize {
    let inner = overlay_inner_width(width);
    if overlay_is_compact(width) {
        // " ▶ " + command + one trailing space.
        inner.saturating_sub(4)
    } else {
        // " ▶ " + command + two spaces + info column + one trailing space.
        inner.saturating_sub(6 + INFO_FIELD_WIDTH)
    }
}

fn visible_suggestion_rows(cursor_row: u16, terminal_height: u16, total: usize, max_visible: usize) -> usize {
    let rows_below_prompt = terminal_height.saturating_sub(cursor_row.saturating_add(1));
    let rows_for_suggestions = usize::from(rows_below_prompt.saturating_sub(3));
    total.min(max_visible.clamp(3, 10)).min(rows_for_suggestions)
}

fn viewport_start(selected: usize, total: usize, viewport_size: usize) -> usize {
    if total <= viewport_size {
        return 0;
    }

    selected
        .saturating_add(1)
        .saturating_sub(viewport_size)
        .min(total - viewport_size)
}

/// Terminal columns a character occupies: combining marks 0, East Asian wide/fullwidth and
/// emoji 2, everything else 1. Counting `chars()` instead misaligns the frame border.
fn character_width(character: char) -> usize {
    if character.is_ascii() {
        return if character.is_control() { 0 } else { 1 };
    }
    let code = character as u32;
    if (0x0300..=0x036F).contains(&code) {
        return 0;
    }
    const WIDE: &[(u32, u32)] = &[
        (0x1100, 0x115F),
        (0x2E80, 0x303E),
        (0x3041, 0x33FF),
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xA000, 0xA4CF),
        (0xAC00, 0xD7A3),
        (0xF900, 0xFAFF),
        (0xFE10, 0xFE19),
        (0xFE30, 0xFE6F),
        (0xFF00, 0xFF60),
        (0xFFE0, 0xFFE6),
        (0x1F300, 0x1FAFF),
        (0x20000, 0x3FFFD),
    ];
    if WIDE
        .iter()
        .any(|&(start, end)| (start..=end).contains(&code))
    {
        return 2;
    }
    1
}

fn display_width(text: &str) -> usize {
    text.chars().map(character_width).sum()
}

fn pad_to_width(text: &str, columns: usize) -> String {
    format!("{}{}", text, " ".repeat(columns.saturating_sub(display_width(text))))
}

fn fit_width(text: &str, max_width: usize) -> String {
    let sanitized = text.replace(['\r', '\n', '\t'], " ");
    if max_width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0usize;
    for character in sanitized.chars() {
        let width = character_width(character);
        if used + width > max_width.saturating_sub(1) {
            result.push('…');
            return result;
        }
        used += width;
        result.push(character);
    }
    result
}

/// The dimmed tail that would be inserted by accepting the selected suggestion.
fn ghost_suffix(command: &str, buffer: &str) -> Option<String> {
    let command = command
        .trim_end_matches(['\r', '\n'])
        .replace(['\r', '\n'], " ");
    if buffer.is_empty() || command.len() <= buffer.len() {
        return None;
    }
    if !command
        .to_ascii_lowercase()
        .starts_with(&buffer.to_ascii_lowercase())
    {
        return None;
    }
    if !command.is_char_boundary(buffer.len()) {
        return None;
    }
    let suffix = &command[buffer.len()..];
    if suffix.is_empty() {
        None
    } else {
        Some(suffix.to_string())
    }
}

/// Inline preview used when there is no room for the box. Destructive commands are excluded:
/// an unlabelled one-keypress acceptance path is not an appropriate affordance for them.
fn render_ghost(
    state: &mut OverlayState,
    output: &Arc<Mutex<io::Stdout>>,
    cur_col: u16,
    cur_row: u16,
    terminal_width: u16,
) -> Result<(), String> {
    let suggestion = match state.current() {
        Some(suggestion) => suggestion.clone(),
        None => return Ok(()),
    };
    state.ghost_cols = 0;
    if !state.tracking_valid || suggestion.risk == "destructive" || state.dismissed {
        return Ok(());
    }
    let Some(suffix) = ghost_suffix(&suggestion.command, &state.buffer) else {
        return Ok(());
    };
    let budget = usize::from(terminal_width)
        .saturating_sub(usize::from(cur_col).saturating_add(2));
    let suffix = fit_width(&suffix, budget);
    let columns = display_width(&suffix);
    if columns == 0 {
        return Ok(());
    }

    let mut stdout = output
        .lock()
        .map_err(|_| "terminal output is unavailable".to_string())?;
    let _ = execute!(stdout, crossterm::cursor::MoveTo(cur_col, cur_row));
    stdout
        .write_all(format!("\x1b[38;5;244m{suffix}\x1b[0m").as_bytes())
        .and_then(|_| {
            let _ = execute!(stdout, crossterm::cursor::MoveTo(cur_col, cur_row));
            stdout.flush()
        })
        .map_err(|error| format!("cannot render HintShell ghost text: {error}"))?;

    state.ghost_cols = columns;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion(command: &str) -> SuggestionItem {
        SuggestionItem {
            command: command.to_string(),
            description: None,
            score: 1.0,
            frequency: 1,
            source: "history".to_string(),
            tier: 0,
            risk: "normal".to_string(),
        }
    }

    fn risky(command: &str) -> SuggestionItem {
        SuggestionItem {
            risk: "destructive".to_string(),
            ..suggestion(command)
        }
    }

    #[test]
    fn startup_trace_requires_explicit_one() {
        assert!(trace_startup_enabled(Some("1")));
        assert!(!trace_startup_enabled(Some("0")));
        assert!(!trace_startup_enabled(Some("true")));
        assert!(!trace_startup_enabled(Some("")));
        assert!(!trace_startup_enabled(None));
    }

    #[test]
    fn overlay_caps_rows_to_remaining_space_without_scrolling() {
        assert_eq!(visible_suggestion_rows(3, 30, 12, 6), 6);
        assert_eq!(visible_suggestion_rows(25, 30, 12, 6), 1);
        assert_eq!(visible_suggestion_rows(26, 30, 12, 6), 0);
    }

    #[test]
    fn viewport_scrolls_to_keep_selected_item_visible() {
        assert_eq!(viewport_start(0, 12, 6), 0);
        assert_eq!(viewport_start(5, 12, 6), 0);
        assert_eq!(viewport_start(6, 12, 6), 1);
        assert_eq!(viewport_start(11, 12, 6), 6);
    }

    #[test]
    fn live_query_includes_wrapper_cwd_and_shell() {
        let request = query_request(
            "cd ".to_string(),
            Some(r"D:\Admin\Documents\PROJECTS\HintShell".to_string()),
            "bash",
        );
        assert!(matches!(
            request,
            HintShellRequest::Suggest {
                cwd: Some(cwd),
                shell: Some(shell),
                ..
            } if cwd == r"D:\Admin\Documents\PROJECTS\HintShell" && shell == "bash"
        ));
    }

    #[test]
    fn live_query_identifies_zsh() {
        let request = query_request("git ".to_string(), None, "zsh");
        assert!(matches!(
            request,
            HintShellRequest::Suggest { shell: Some(shell), .. } if shell == "zsh"
        ));
    }

    #[test]
    fn cwd_marker_updates_query_directory_without_rendering() {
        let cwd = Arc::new(Mutex::new(None));
        let mut pending = Vec::new();
        let bytes = filter_cwd_markers(
            b"prompt\x1eHINTSHELL_CWD:/tmp/workspace\x1fnext",
            &mut pending,
            Some(&cwd),
        );
        assert_eq!(bytes, b"promptnext");
        assert_eq!(*cwd.lock().unwrap(), Some("/tmp/workspace".to_string()));
    }

    #[test]
    fn cwd_marker_can_span_output_reads() {
        let cwd = Arc::new(Mutex::new(None));
        let mut pending = Vec::new();
        let first = filter_cwd_markers(b"\x1eHINTSHELL_CWD:/tmp/", &mut pending, Some(&cwd));
        let second = filter_cwd_markers(b"project\x1f", &mut pending, Some(&cwd));
        assert!(first.is_empty());
        assert!(second.is_empty());
        assert_eq!(*cwd.lock().unwrap(), Some("/tmp/project".to_string()));
    }

    #[test]
    fn prompt_marker_resumes_overlay_without_rendering() {
        let (events, received) = mpsc::channel();
        let mut pending = Vec::new();
        let first = filter_prompt_markers(b"before\x1eHINTSHELL_", &mut pending, &events);
        let second = filter_prompt_markers(b"PROMPT\x1fafter", &mut pending, &events);

        assert_eq!(first, b"before");
        assert_eq!(second, b"after");
        assert!(matches!(received.try_recv(), Ok(ShellEvent::PromptReady)));
    }

    #[test]
    fn prompt_marker_can_span_output_reads() {
        let (events, received) = mpsc::channel();
        let mut pending = Vec::new();
        let first = filter_prompt_markers(b"\x1eHINTSHELL_PROM", &mut pending, &events);
        let second = filter_prompt_markers(b"PT\x1f", &mut pending, &events);

        assert!(first.is_empty());
        assert!(second.is_empty());
        assert!(matches!(received.try_recv(), Ok(ShellEvent::PromptReady)));
    }

    #[test]
    fn continuation_marker_suppresses_suggestions_without_rendering() {
        let (events, received) = mpsc::channel();
        let mut pending = Vec::new();
        let bytes = filter_continuation_markers(
            b"> \x1eHINTSHELL_CONTINUATION\x1fmore input",
            &mut pending,
            &events,
        );

        assert_eq!(bytes, b"> more input");
        assert!(matches!(
            received.try_recv(),
            Ok(ShellEvent::Continuation)
        ));
    }

    #[test]
    fn continuation_marker_can_span_output_reads() {
        let (events, received) = mpsc::channel();
        let mut pending = Vec::new();
        let first = filter_continuation_markers(b"\x1eHINTSHELL_CONTINU", &mut pending, &events);
        let second = filter_continuation_markers(b"ATION\x1f", &mut pending, &events);

        assert!(first.is_empty());
        assert!(second.is_empty());
        assert!(matches!(
            received.try_recv(),
            Ok(ShellEvent::Continuation)
        ));
    }

    #[test]
    fn ignores_key_release_events() {
        let key = KeyEvent::new_with_kind(
            KeyCode::Char('g'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert!(!accepts_key_event(&key));
    }

    #[test]
    fn accepts_key_press_and_repeat_events() {
        let press = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        let repeat =
            KeyEvent::new_with_kind(KeyCode::Char('g'), KeyModifiers::NONE, KeyEventKind::Repeat);
        assert!(accepts_key_event(&press));
        assert!(accepts_key_event(&repeat));
    }

    #[test]
    fn detects_wsl_from_either_runtime_environment_variable() {
        assert!(is_wsl_environment(true, false));
        assert!(is_wsl_environment(false, true));
        assert!(!is_wsl_environment(false, false));
    }

    #[cfg(windows)]
    #[test]
    fn quotes_windows_arguments_with_spaces_or_quotes() {
        assert_eq!(quote_windows_argument("git status"), "\"git status\"");
        assert_eq!(
            quote_windows_argument("printf 'ok\\n'"),
            "\"printf 'ok\\n'\""
        );
    }

    #[test]
    fn accepts_matching_command_suggestion() {
        assert_eq!(
            tab_action("git che", Some(&suggestion("git checkout main"))),
            TabAction::AcceptHintShell
        );
    }

    #[test]
    fn accepting_a_suggestion_discards_before_inserting() {
        // The input bridge must finish Bash's Ctrl-U redraw before it writes the
        // selected command; otherwise ConPTY can render the prefix on a stray line.
        assert!(TAB_ACCEPT_CLEAR_DELAY > Duration::ZERO);
    }

    #[test]
    fn defers_path_completion_to_bash() {
        assert_eq!(
            tab_action("cd src/comp", Some(&suggestion("cd src/components"))),
            TabAction::ForwardToBash
        );
    }

    #[test]
    fn defers_flag_completion_to_bash() {
        assert_eq!(
            tab_action("git --ver", Some(&suggestion("git --version"))),
            TabAction::ForwardToBash
        );
    }

    #[test]
    fn stale_result_can_be_rejected_by_generation_and_buffer() {
        let mut state = OverlayState::new();
        state.buffer = "git c".to_string();
        state.generation = 3;
        let result = SuggestionResult {
            generation: 2,
            buffer: "git".to_string(),
            suggestions: vec![suggestion("git commit")],
        };
        assert!(result.generation != state.generation || result.buffer != state.buffer);
    }

    #[test]
    fn overlay_frame_does_not_contain_dec_save_restore() {
        let width = 44;
        let inner_width = overlay_inner_width(width);
        let command_width = overlay_command_width(width);
        let row = format!(" ▶ {:<command_width$} ", "git status");
        let description = format!(
            "  {:<desc_width$}",
            "no description",
            desc_width = inner_width - 2
        );

        assert_eq!(inner_width, 42);
        assert_eq!(display_width(&row), inner_width);
        assert_eq!(display_width(&description), inner_width);
    }

    #[test]
    fn truncation_keeps_a_visible_ellipsis() {
        assert_eq!(fit_width("abcdefgh", 5), "abcd…");
        assert_eq!(fit_width("abc", 0), "");
    }

    #[test]
    fn defers_destructive_suggestions_to_explicit_acceptance() {
        // Tab keeps working, but the invisible right-arrow path must not accept damage.
        assert_eq!(
            tab_action("rm", Some(&risky("rm -rf build"))),
            TabAction::AcceptHintShell
        );
        assert_eq!(risky("rm -rf build").risk, "destructive");
    }

    #[test]
    fn an_older_daemon_still_gets_the_safety_marker() {
        // The installed daemon may predate the risk field; the panel must not then offer a
        // silent arrow-accept of an unrecoverable command.
        let mut legacy = suggestion("rm -rf build");
        mark_destructive_without_daemon_support(&mut legacy);
        assert_eq!(legacy.risk, "destructive");

        let mut safe = suggestion("git status");
        mark_destructive_without_daemon_support(&mut safe);
        assert_eq!(safe.risk, "normal", "ordinary commands must stay unmarked");

        let mut marked = suggestion("git status");
        marked.risk = "destructive".to_string();
        mark_destructive_without_daemon_support(&mut marked);
        assert_eq!(marked.risk, "destructive", "a daemon marker is never removed");
    }

    #[test]
    fn overlay_layout_fills_exactly_the_inner_width_in_both_modes() {
        // Full layout (>= 64): command plus the explanation column.
        let full = 78;
        let inner = overlay_inner_width(full);
        let command_width = overlay_command_width(full);
        assert!(!overlay_is_compact(full));
        assert_eq!(inner, 76);
        let row = format!(
            " ▶ {:<cw$}  {:<INFO_FIELD_WIDTH$} ",
            "git status",
            "here 12x",
            cw = command_width
        );
        assert_eq!(display_width(&row), inner);

        // Compact layout (< 64): the column is dropped and the command gets the space.
        let narrow = 44;
        let narrow_inner = overlay_inner_width(narrow);
        let narrow_command_width = overlay_command_width(narrow);
        assert!(overlay_is_compact(narrow));
        assert!(
            narrow_command_width > 30,
            "command must not be cut to a stub in a 44-column pane"
        );
        let narrow_row = format!(" ▶ {:<cw$} ", "git status", cw = narrow_command_width);
        assert_eq!(display_width(&narrow_row), narrow_inner);
    }

    #[test]
    fn frame_never_exceeds_the_terminal_width() {
        // A 30-column pane used to be handed a 44-column frame, which wrapped and left
        // residue on every following row.
        assert_eq!(overlay_width(30), 29);
        assert_eq!(overlay_width(120), 78);
        assert_eq!(overlay_width(8), 24, "floor for absurdly small panes");
        assert!(overlay_width(30) < 30, "box must fit inside the terminal");
    }

    #[test]
    fn display_width_counts_wide_and_combining_characters() {
        assert_eq!(display_width("git status"), 10);
        assert_eq!(display_width("目录"), 4);
        assert_eq!(display_width("…▶"), 2);
        assert_eq!(fit_width("目录目录目录", 5), "目录…");
    }

    #[test]
    fn control_chords_reach_bash_as_readline_bytes() {
        let key = |code: KeyCode, modifiers: KeyModifiers| KeyEvent::new(code, modifiers);
        assert_eq!(
            control_sequence(&key(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            Some(vec![0x12])
        );
        assert_eq!(
            control_sequence(&key(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(vec![0x04])
        );
        assert_eq!(
            control_sequence(&key(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            Some(vec![0x0c])
        );
        assert_eq!(
            control_sequence(&key(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            Some(vec![0x17])
        );
        assert_eq!(
            control_sequence(&key(KeyCode::Char('R'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)),
            Some(vec![0x12])
        );
        assert_eq!(control_sequence(&key(KeyCode::Char('r'), KeyModifiers::NONE)), None);
    }

    #[test]
    fn ghost_text_only_extends_a_real_prefix() {
        assert_eq!(
            ghost_suffix("git checkout main", "git ").as_deref(),
            Some("checkout main")
        );
        assert_eq!(
            ghost_suffix("git checkout main", "GIT ").as_deref(),
            Some("checkout main")
        );
        assert_eq!(ghost_suffix("git status", "git status"), None);
        assert_eq!(ghost_suffix("rm -rf build", "ru"), None);
    }

    #[test]
    fn dismissed_overlay_stays_hidden_while_editing_the_same_line() {
        let mut state = OverlayState::new();
        state.buffer = "git".to_string();
        state.tracking_valid = true;
        state.suggestions = vec![suggestion("git status")];
        assert!(should_query(&state), "a typed command prefix is worth querying");

        // The Esc path: "not now" has to survive the next keystroke on the same line.
        state.dismissed = true;
        assert!(!should_query(&state), "typing must not re-open a dismissed panel");

        state.dismissed = false;
        state.buffer = "git status -".to_string();
        assert!(!should_query(&state), "a flag position is Bash's own completion");

        state.buffer = "cd src/".to_string();
        assert!(!should_query(&state), "a path position is Bash's own completion");

        state.buffer = String::new();
        assert!(!should_query(&state), "an empty buffer has nothing to match");

        state.buffer = "git".to_string();
        state.tracking_valid = false;
        assert!(!should_query(&state), "an untracked line must not be guessed");
    }

    /// Run the four marker filters exactly as `start_output_pump` chains them.
    fn pump_chain(chunks: &[&[u8]]) -> (Vec<u8>, Vec<&'static str>) {
        let cwd = Arc::new(Mutex::new(None::<String>));
        let (events_tx, events_rx) = mpsc::channel();
        let mut continuation = Vec::new();
        let mut prompt = Vec::new();
        let mut startup = Vec::new();
        let mut current_directory = Vec::new();
        let mut written = Vec::new();

        for chunk in chunks {
            let bytes = filter_cwd_markers(
                &filter_startup_phase_markers(
                    &filter_prompt_markers(
                        &filter_continuation_markers(chunk, &mut continuation, &events_tx),
                        &mut prompt,
                        &events_tx,
                    ),
                    &mut startup,
                    &events_tx,
                ),
                &mut current_directory,
                Some(&cwd),
            );
            written.extend_from_slice(&bytes);
        }
        // The pump flushes whatever the filters were still holding when the shell closed.
        for leftover in [continuation, prompt, startup, current_directory] {
            written.extend_from_slice(&leftover);
        }
        drop(events_tx);
        let mut seen = Vec::new();
        while let Ok(event) = events_rx.recv() {
            match event {
                ShellEvent::PromptReady => seen.push("prompt"),
                ShellEvent::Continuation => seen.push("continuation"),
                ShellEvent::StartupPhase(_) => seen.push("startup"),
                ShellEvent::Output(_) | ShellEvent::Closed => {}
            }
        }
        (written, seen)
    }

    #[test]
    fn unhandled_navigation_keys_are_not_swallowed() {
        let key = |code: KeyCode, modifiers: KeyModifiers| KeyEvent::new(code, modifiers);
        assert_eq!(
            passthrough_sequence(&key(KeyCode::PageUp, KeyModifiers::NONE)),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            passthrough_sequence(&key(KeyCode::PageDown, KeyModifiers::NONE)),
            Some(b"\x1b[6~".to_vec())
        );
        assert_eq!(
            passthrough_sequence(&key(KeyCode::F(1), KeyModifiers::NONE)),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            passthrough_sequence(&key(KeyCode::PageUp, KeyModifiers::CONTROL)),
            Some(b"\x1b[5;5~".to_vec())
        );
        assert_eq!(passthrough_sequence(&key(KeyCode::Char('g'), KeyModifiers::NONE)), None);
    }

    #[test]
    fn cursor_keys_keep_their_modifiers() {
        let key = |code: KeyCode, modifiers: KeyModifiers| KeyEvent::new(code, modifiers);
        assert_eq!(cursor_sequence(&key(KeyCode::Left, KeyModifiers::NONE)), b"\x1b[D");
        assert_eq!(cursor_sequence(&key(KeyCode::Right, KeyModifiers::NONE)), b"\x1b[C");
        // Plain Home/End stay readline beginning/end of line, as Bash binds them.
        assert_eq!(cursor_sequence(&key(KeyCode::Home, KeyModifiers::NONE)), vec![0x01]);
        assert_eq!(cursor_sequence(&key(KeyCode::End, KeyModifiers::NONE)), vec![0x05]);
        assert_eq!(
            cursor_sequence(&key(KeyCode::Left, KeyModifiers::CONTROL)),
            b"\x1b[1;5D"
        );
        assert_eq!(
            cursor_sequence(&key(KeyCode::Right, KeyModifiers::ALT)),
            b"\x1b[1;3C"
        );
        assert_eq!(
            cursor_sequence(&key(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT)),
            b"\x1b[1;6C"
        );
    }

    #[test]
    fn marker_text_never_reaches_the_screen_however_the_pty_splits_it() {
        let prompt_text = "ADMIN@host MINGW64 ~\r\n";
        let streams: Vec<Vec<u8>> = vec![
            // Current wire format: markers inside a private OSC sequence.
            format!(
                "{prompt_text}\x1b]5522;hintshell_cwd=C:/Users/ADMIN\x07\x1b]5522;hintshell_prompt\x07"
            )
            .into_bytes(),
            // Legacy RS/US shape, still stripped for a shell that started with an older hook.
            format!("{prompt_text}\x1eHINTSHELL_CWD:C:/Users/ADMIN\x1f\x1eHINTSHELL_PROMPT\x1f")
                .into_bytes(),
        ];

        for stream in &streams {
            for split in 1..=stream.len() {
                let (left, right) = stream.split_at(split);
                let chunks: Vec<&[u8]> = if right.is_empty() {
                    vec![left]
                } else {
                    vec![left, right]
                };
                let (written, seen) = pump_chain(&chunks);
                assert_eq!(
                    String::from_utf8_lossy(&written),
                    prompt_text,
                    "leaked marker text when the stream split at byte {split}"
                );
                assert!(
                    seen.contains(&"prompt"),
                    "the prompt event was lost when splitting at byte {split}: {seen:?}"
                );
            }

            let bytes: Vec<&[u8]> = stream.chunks(1).collect();
            let (written, seen) = pump_chain(&bytes);
            assert_eq!(
                String::from_utf8_lossy(&written),
                prompt_text,
                "byte-by-byte delivery leaked"
            );
            assert!(seen.contains(&"prompt"), "prompt event lost byte by byte");
        }
    }

    #[test]
    fn osc_cwd_marker_updates_the_directory_without_printing_path_text() {
        let cwd = Arc::new(Mutex::new(None::<String>));
        let mut pending = Vec::new();
        let written = filter_cwd_markers(
            b"prompt\x1b]5522;hintshell_cwd=/tmp/workspace\x07next",
            &mut pending,
            Some(&cwd),
        );
        assert_eq!(String::from_utf8_lossy(&written), "promptnext");
        assert_eq!(
            cwd.lock().unwrap().as_deref(),
            Some("/tmp/workspace"),
            "the OSC payload never reached the directory"
        );
    }

    #[test]
    fn legacy_control_byte_markers_are_still_stripped() {
        let cwd = Arc::new(Mutex::new(None::<String>));
        let mut pending = Vec::new();
        let written = filter_cwd_markers(
            b"prompt\x1eHINTSHELL_CWD:/tmp/old\x1fnext",
            &mut pending,
            Some(&cwd),
        );
        assert_eq!(String::from_utf8_lossy(&written), "promptnext");
        assert_eq!(cwd.lock().unwrap().as_deref(), Some("/tmp/old"));
    }

    #[test]
    fn codepage_remapped_marker_glyphs_from_a_stale_hook_are_neutralised() {
        // A generated rcfile that predates the OSC protocol still writes the 0x1e/0x1f form,
        // and ConPTY hands the wrapper those bytes already re-mapped into "\u{25b2}" and
        // "\u{25bc}" text. Nothing may reach the screen, and the signals must still arrive.
        let cwd = Arc::new(Mutex::new(None::<String>));
        let mut pending = Vec::new();
        let written = filter_cwd_markers(
            "before\u{25b2}HINTSHELL_CWD:D:/Admin/proj\u{25bc}after".as_bytes(),
            &mut pending,
            Some(&cwd),
        );
        assert_eq!(
            String::from_utf8_lossy(&written),
            "beforeafter",
            "stale-hook marker glyphs reached the screen"
        );
        assert_eq!(cwd.lock().unwrap().as_deref(), Some("D:/Admin/proj"));

        let (sender, receiver) = mpsc::channel();
        let mut pending = Vec::new();
        let written = filter_prompt_markers(
            "\u{25b2}HINTSHELL_PROMPT\u{25bc}".as_bytes(),
            &mut pending,
            &sender,
        );
        assert!(written.is_empty(), "a stale prompt marker was printed");
        let mut pending = Vec::new();
        let written = filter_continuation_markers(
            "\u{25b2}HINTSHELL_CONTINUATION\u{25bc}".as_bytes(),
            &mut pending,
            &sender,
        );
        assert!(
            written.is_empty(),
            "a stale continuation marker was printed"
        );
        assert!(matches!(receiver.recv(), Ok(ShellEvent::PromptReady)));
        assert!(matches!(receiver.recv(), Ok(ShellEvent::Continuation)));
    }

    /// A byte sink standing in for the ConPTY child input, so key handling can be tested
    /// without a terminal.
    #[derive(Clone)]
    struct ShellSink(Arc<Mutex<Vec<u8>>>);

    impl Write for ShellSink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct KeyHarness {
        state: OverlayState,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        output: Arc<Mutex<io::Stdout>>,
        queries: mpsc::Sender<(u64, String)>,
        answers: mpsc::Receiver<(u64, String)>,
        sent: Arc<Mutex<Vec<u8>>>,
    }

    impl KeyHarness {
        fn new() -> Self {
            let sent = Arc::new(Mutex::new(Vec::new()));
            let writer: Arc<Mutex<Box<dyn Write + Send>>> =
                Arc::new(Mutex::new(Box::new(ShellSink(Arc::clone(&sent)))));
            let (queries, answers) = mpsc::channel();
            Self {
                state: OverlayState::new(),
                writer,
                output: Arc::new(Mutex::new(io::stdout())),
                queries,
                answers,
                sent,
            }
        }

        fn send(&mut self, code: KeyCode, modifiers: KeyModifiers) {
            let key = KeyEvent::new(code, modifiers);
            handle_key(
                key,
                &mut self.state,
                &self.writer,
                &self.output,
                &self.queries,
            )
            .expect("key handling failed");
        }

        fn type_text(&mut self, text: &str) {
            for character in text.chars() {
                self.send(KeyCode::Char(character), KeyModifiers::NONE);
            }
        }

        fn ctrl(&mut self, character: char) {
            self.send(KeyCode::Char(character), KeyModifiers::CONTROL);
        }

        fn last_query(&mut self) -> Option<String> {
            let mut latest = None;
            while let Ok((_, buffer)) = self.answers.try_recv() {
                latest = Some(buffer);
            }
            latest
        }

        fn forwarded(&self) -> Vec<u8> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[test]
    fn ctrl_u_clears_the_line_and_the_next_keystroke_asks_again() {
        let mut harness = KeyHarness::new();
        harness.type_text("git status");
        harness.ctrl('u');

        assert_eq!(harness.state.buffer, "", "Ctrl+U left the shadowed line populated");
        assert!(
            harness.forwarded().ends_with(b"\x15"),
            "Ctrl+U was not forwarded to bash: {:?}",
            harness.forwarded()
        );
        assert!(harness.state.tracking_valid, "Ctrl+U discarded the line model");

        harness.type_text("ls");
        assert_eq!(
            harness.last_query().as_deref(),
            Some("ls"),
            "typing after Ctrl+U stopped querying the engine"
        );
    }

    #[test]
    fn history_recall_then_ctrl_u_brings_the_overlay_back() {
        // Up/Down recall replaces the whole line, which the wrapper cannot model, so the
        // shadow is dropped. Readline leaves the cursor after the recalled text, therefore a
        // Ctrl+U from there empties the line and the next keystroke is trustworthy again.
        let mut harness = KeyHarness::new();
        harness.type_text("git status");
        harness.send(KeyCode::Up, KeyModifiers::NONE);
        assert!(!harness.state.tracking_valid, "history recall kept a stale model");
        harness.ctrl('u');
        assert!(
            harness.state.tracking_valid,
            "Ctrl+U from the end of a recalled line did not restore the model"
        );
        harness.type_text("ls");
        assert_eq!(
            harness.last_query().as_deref(),
            Some("ls"),
            "the overlay stayed dead after Ctrl+U following history recall"
        );
    }

    #[test]
    fn mid_line_cursor_move_then_ctrl_u_stays_conservative() {
        // After moving the cursor inside a line the wrapper cannot model, Ctrl+U leaves a tail
        // it has no copy of. Claiming an empty line here would make an accept insert text into
        // the wrong place, so the shadow deliberately stays untrusted until the next prompt.
        let mut harness = KeyHarness::new();
        harness.type_text("git status");
        harness.send(KeyCode::Left, KeyModifiers::NONE);
        harness.ctrl('u');
        assert!(!harness.state.tracking_valid, "Ctrl+U claimed a line it never saw");
        harness.last_query(); // drain the queries the trusted typing already made
        harness.type_text("ls");
        assert_eq!(
            harness.last_query(),
            None,
            "the wrapper queried with a buffer it does not trust"
        );
    }

    #[test]
    fn ctrl_u_after_accepting_a_suggestion_with_tab_asks_again() {
        // The reported flow: type a prefix, select a row, press Tab to accept it, then press
        // Ctrl+U to throw the line away and start typing again.
        let mut harness = KeyHarness::new();
        harness.type_text("git st");
        harness.state.suggestions = vec![suggestion("git status")];
        harness.send(KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(
            harness.state.buffer, "git status",
            "Tab did not accept the selected row"
        );

        harness.ctrl('u');
        assert_eq!(harness.state.buffer, "");
        harness.type_text("git ");
        assert_eq!(
            harness.last_query().as_deref(),
            Some("git "),
            "typing after Ctrl+U stopped querying after a Tab accept"
        );
    }

    #[test]
    fn ctrl_u_after_bash_tab_completion_asks_again() {
        // Tab can also be handed to Bash for its own completion, which rewrites the line with
        // text the wrapper never saw. Ctrl+U then has to make the line usable again.
        let mut harness = KeyHarness::new();
        harness.type_text("git sta");
        harness.send(KeyCode::Tab, KeyModifiers::NONE);
        harness.ctrl('u');
        harness.type_text("ls");
        assert_eq!(
            harness.last_query().as_deref(),
            Some("ls"),
            "typing after Ctrl+U stopped querying after Bash completion"
        );
    }

    #[test]
    fn continuation_and_prompt_markers_survive_a_split_stream() {
        let chunks: [&[u8]; 2] = [
            b"\x1eHINTSHELL_CWD:/tmp\x1f\x1eHINTSHELL_PROMPT\x1fmore\x1eHINTSHELL_CONTINU",
            b"ATION\x1fend",
        ];
        let (written, seen) = pump_chain(&chunks);
        assert_eq!(String::from_utf8_lossy(&written), "moreend");
        assert_eq!(seen, vec!["prompt", "continuation"], "{seen:?}");
    }
}
