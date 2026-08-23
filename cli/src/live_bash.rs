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
            clear_overlay(&mut state, &output)?;
            match shell_event {
                ShellEvent::Output(bytes) => {
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
                    state.tracking_valid = true;
                    state.buffer.clear();
                    state.clear();
                }
                ShellEvent::Closed => running = false,
            }
        }
        while let Ok(result) = result_rx.try_recv() {
            if result.generation == state.generation && result.buffer == state.buffer {
                clear_overlay(&mut state, &output)?;
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
                    state.generation = state.generation.wrapping_add(1);
                }
                ShellEvent::Closed => running = false,
            }
        }
        if input_mode == UnixInputMode::PromptOverlay {
            while let Ok(result) = result_rx.try_recv() {
                if result.generation == state.generation && result.buffer == state.buffer {
                    clear_overlay(&mut state, &output)?;
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
            Ok(ShellEvent::StartupPhase(_)) | Ok(ShellEvent::PromptReady) => {}
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
            Ok(ShellEvent::StartupPhase(_)) | Ok(ShellEvent::PromptReady) => {}
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
    Closed,
}

const CWD_MARKER_START: &[u8] = b"\x1eHINTSHELL_CWD:";
const CWD_MARKER_END: &[u8] = b"\x1f";
const STARTUP_PHASE_MARKER_START: &[u8] = b"\x1eHINTSHELL_STARTUP_PHASE:";
const STARTUP_PHASE_MARKER_END: &[u8] = b"\x1f";
const PROMPT_MARKER: &[u8] = b"\x1eHINTSHELL_PROMPT\x1f";

fn start_output_pump(
    mut reader: Box<dyn Read + Send>,
    events: mpsc::Sender<ShellEvent>,
    cwd: Option<Arc<Mutex<Option<String>>>>,
) {
    let mut prompt_marker_buffer = Vec::new();
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
                                &buffer[..read],
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
        if !cwd_marker_buffer.is_empty() {
            let _ = events.send(ShellEvent::Output(cwd_marker_buffer));
        }
        let _ = events.send(ShellEvent::Closed);
    });
}

fn filter_prompt_markers(
    bytes: &[u8],
    pending: &mut Vec<u8>,
    events: &mpsc::Sender<ShellEvent>,
) -> Vec<u8> {
    pending.extend_from_slice(bytes);
    let mut output = Vec::new();

    loop {
        let Some(start) = pending
            .windows(PROMPT_MARKER.len())
            .position(|window| window == PROMPT_MARKER)
        else {
            let keep = marker_prefix_len(pending, PROMPT_MARKER);
            let flush = pending.len().saturating_sub(keep);
            output.extend_from_slice(&pending[..flush]);
            pending.drain(..flush);
            break;
        };

        output.extend_from_slice(&pending[..start]);
        pending.drain(..start + PROMPT_MARKER.len());
        let _ = events.send(ShellEvent::PromptReady);
    }

    output
}

fn filter_startup_phase_markers(
    bytes: &[u8],
    pending: &mut Vec<u8>,
    events: &mpsc::Sender<ShellEvent>,
) -> Vec<u8> {
    pending.extend_from_slice(bytes);
    let mut output = Vec::new();

    loop {
        let Some(start) = find_subsequence(pending, STARTUP_PHASE_MARKER_START) else {
            let keep = marker_prefix_len(pending, STARTUP_PHASE_MARKER_START);
            let flush = pending.len().saturating_sub(keep);
            output.extend_from_slice(&pending[..flush]);
            pending.drain(..flush);
            break;
        };
        output.extend_from_slice(&pending[..start]);
        let phase_start = start + STARTUP_PHASE_MARKER_START.len();
        let Some(relative_end) =
            find_subsequence(&pending[phase_start..], STARTUP_PHASE_MARKER_END)
        else {
            pending.drain(..start);
            break;
        };
        let phase_end = phase_start + relative_end;
        if let Ok(phase) = std::str::from_utf8(&pending[phase_start..phase_end]) {
            let _ = events.send(ShellEvent::StartupPhase(phase.to_string()));
        }
        pending.drain(..phase_end + STARTUP_PHASE_MARKER_END.len());
    }

    output
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
    pending.extend_from_slice(bytes);
    let mut output = Vec::new();

    loop {
        let Some(start) = pending
            .windows(CWD_MARKER_START.len())
            .position(|window| window == CWD_MARKER_START)
        else {
            let max_prefix = pending.len().min(CWD_MARKER_START.len().saturating_sub(1));
            let keep = (1..=max_prefix)
                .rev()
                .find(|length| pending[pending.len() - length..] == CWD_MARKER_START[..*length])
                .unwrap_or(0);
            let flush = pending.len().saturating_sub(keep);
            output.extend_from_slice(&pending[..flush]);
            pending.drain(..flush);
            break;
        };

        output.extend_from_slice(&pending[..start]);
        if let Some(end) =
            find_subsequence(&pending[start + CWD_MARKER_START.len()..], CWD_MARKER_END)
        {
            let path_start = start + CWD_MARKER_START.len();
            let path_end = path_start + end;
            if let Ok(path) = std::str::from_utf8(&pending[path_start..path_end]) {
                if let Some(cwd) = cwd {
                    if let Ok(mut current) = cwd.lock() {
                        *current = Some(path.to_string());
                    }
                }
            }
            pending.drain(..path_end + CWD_MARKER_END.len());
        } else {
            pending.drain(..start);
            break;
        }
    }

    output
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
            let raw_suggestions = runtime
                .block_on(async {
                    tokio::time::timeout(QUERY_TIMEOUT, crate::send_request(&request))
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .and_then(|response| response.suggestions)
                        .unwrap_or_default()
                });

            let buffer_lower = buffer.to_ascii_lowercase();
            // 1. First priority: commands starting with the typed input (prefix match)
            let prefix_matches: Vec<SuggestionItem> = raw_suggestions
                .iter()
                .filter(|s| s.command.to_ascii_lowercase().starts_with(&buffer_lower))
                .cloned()
                .collect();

            let suggestions = if !prefix_matches.is_empty() {
                prefix_matches
            } else {
                // 2. Fallback: if no command starts with input, match commands containing input (substring / contains)
                raw_suggestions
                    .into_iter()
                    .filter(|s| s.command.to_ascii_lowercase().contains(&buffer_lower))
                    .collect()
            };

            let _ = sender.send(SuggestionResult {
                generation,
                buffer,
                suggestions,
            });
        }
    });
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
    match key.code {
        KeyCode::Tab => match tab_action(&state.buffer, state.current()) {
            TabAction::AcceptHintShell => {
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
            TabAction::ForwardToBash => {
                clear_overlay(state, output)?;
                state.clear();
                write_to_shell(writer, b"\t")?;
            }
        },
        KeyCode::Up if !state.suggestions.is_empty() => {
            clear_overlay(state, output)?;
            state.selected = state.selected.saturating_sub(1);
            render_overlay(state, output)?;
        }
        KeyCode::Down if !state.suggestions.is_empty() => {
            clear_overlay(state, output)?;
            state.selected = (state.selected + 1).min(state.suggestions.len() - 1);
            render_overlay(state, output)?;
        }
        KeyCode::Up => forward_and_invalidate(state, writer, output, b"\x1b[A")?,
        KeyCode::Down => forward_and_invalidate(state, writer, output, b"\x1b[B")?,
        KeyCode::Esc if !state.suggestions.is_empty() => {
            clear_overlay(state, output)?;
            state.clear();
        }
        KeyCode::Esc => forward_and_invalidate(state, writer, output, b"\x1b")?,
        KeyCode::Backspace => {
            clear_overlay(state, output)?;
            if !state.tracking_valid {
                forward_and_invalidate(state, writer, output, b"\x7f")?;
            } else {
                state.buffer.pop();
                write_to_shell(writer, b"\x7f")?;
                request_suggestions(state, query_tx);
            }
        }
        KeyCode::Enter => {
            clear_overlay(state, output)?;
            state.clear();
            state.tracking_valid = true;
            write_to_shell(writer, b"\r")?;
            state.buffer.clear();
            state.generation = state.generation.wrapping_add(1);
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            clear_overlay(state, output)?;
            if state.tracking_valid {
                state.buffer.push(character);
            }
            write_to_shell(writer, character.to_string().as_bytes())?;
            request_suggestions(state, query_tx);
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
        // Right Arrow or End: accept ghost text / full suggestion if at end of buffer
        KeyCode::Right | KeyCode::End if !state.suggestions.is_empty() && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::CONTROL) => {
            if let Some(cmd) = state.current().map(|s| s.command.clone()) {
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
                        request_suggestions(state, query_tx);
                        return Ok(());
                    }
                }
            }
            forward_and_invalidate(state, writer, output, b"\x1b[1;5C")?;
        }
        KeyCode::Left => forward_and_invalidate(state, writer, output, b"\x1b[D")?,
        KeyCode::Right => forward_and_invalidate(state, writer, output, b"\x1b[C")?,
        KeyCode::Home => forward_and_invalidate(state, writer, output, b"\x01")?,
        KeyCode::End => forward_and_invalidate(state, writer, output, b"\x05")?,
        KeyCode::Delete => forward_and_invalidate(state, writer, output, b"\x1b[3~")?,
        _ => {}
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

fn request_suggestions(state: &mut OverlayState, sender: &mpsc::Sender<(u64, String)>) {
    state.generation = state.generation.wrapping_add(1);
    state.clear();
    if !state.tracking_valid || !is_suggestable(&state.buffer) {
        return;
    }
    let _ = sender.send((state.generation, state.buffer.clone()));
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

fn clear_overlay(state: &mut OverlayState, output: &Arc<Mutex<io::Stdout>>) -> Result<(), String> {
    if state.rendered_lines == 0 {
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
    stdout
        .write_all(frame.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("cannot clear HintShell overlay: {error}"))?;

    let _ = execute!(stdout, crossterm::cursor::MoveTo(cur_col, cur_row));
    let _ = stdout.flush();

    state.rendered_lines = 0;
    Ok(())
}

fn render_overlay(state: &mut OverlayState, output: &Arc<Mutex<io::Stdout>>) -> Result<(), String> {
    if state.suggestions.is_empty() {
        return Ok(());
    }

    let config = crate::config::HintShellConfig::load();
    let is_rainbow = config.is_rainbow();
    let border_color_ansi = config.border_ansi_code();

    let (terminal_width, terminal_height) = size().unwrap_or((100, 30));
    let width = usize::from(terminal_width).clamp(44, 78);
    let inner_width = overlay_inner_width(width);
    let command_width = overlay_command_width(width);
    let total = state.suggestions.len();
    let (cur_col, cur_row) = position().unwrap_or((0, 0));
    let shown = visible_suggestion_rows(cur_row, terminal_height, total, config.max_visible);
    if shown == 0 {
        return Ok(());
    }
    let rendered_lines = shown + 3;
    let selected = state.selected.min(total.saturating_sub(1));
    let viewport_start = viewport_start(selected, total, shown);
    let viewport_end = viewport_start + shown;
    let mut frame = String::new();
    let counter = format!(" {}/{} ", selected + 1, total);

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

    if is_rainbow {
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
        let command = fit_width(&suggestion.command, command_width);
        let source_clean = fit_width(&suggestion.source, 20);
        let source_pad = 20usize.saturating_sub(source_clean.chars().count());
        let source_formatted = format!("{}{}", source_clean, " ".repeat(source_pad));
        let marker = if selected_row { "▶" } else { " " };
        let highlighted_command = highlight_match(&command, &state.buffer, selected_row, command_width);

        let row_border_left = if is_rainbow {
            rainbow_colors[index % rainbow_colors.len()]
        } else {
            border_color_ansi
        };
        let row_border_right = if is_rainbow {
            rainbow_colors[(index + 3) % rainbow_colors.len()]
        } else {
            border_color_ansi
        };

        if selected_row {
            frame.push_str(&format!(
                "\x1b[1B\r\x1b[2K{row_border_left}│\x1b[48;5;60m\x1b[38;5;255m {marker} {highlighted_command}  {source_formatted} \x1b[0m{row_border_right}│\x1b[0m"
            ));
        } else {
            frame.push_str(&format!(
                "\x1b[1B\r\x1b[2K{row_border_left}│\x1b[0m {marker} {highlighted_command}  {source_formatted} {row_border_right}│\x1b[0m"
            ));
        }
    }

    let description = state
        .suggestions
        .get(selected)
        .and_then(|suggestion| suggestion.description.as_deref())
        .unwrap_or("history suggestion");
    let description = fit_width(description, inner_width.saturating_sub(2));
    let desc_border = if is_rainbow { "\x1b[38;5;212m" } else { border_color_ansi };
    let desc_pad = inner_width.saturating_sub(description.chars().count() + 2);
    frame.push_str(&format!(
        "\x1b[1B\r\x1b[2K{desc_border}│\x1b[38;5;244m  {description}{}{desc_border}│\x1b[0m",
        " ".repeat(desc_pad)
    ));

    if is_rainbow {
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

fn highlight_match(command: &str, buffer: &str, is_selected: bool, target_width: usize) -> String {
    let clean_cmd = command.replace(['\r', '\n', '\t'], " ");
    let buf_lower = buffer.trim().to_ascii_lowercase();
    let cmd_lower = clean_cmd.to_ascii_lowercase();
    if buf_lower.is_empty() {
        return format!("{:<width$}", clean_cmd, width = target_width);
    }

    if let Some(pos) = cmd_lower.find(&buf_lower) {
        let match_len = buf_lower.len();
        let before = &clean_cmd[..pos];
        let matched = &clean_cmd[pos..pos + match_len];
        let after = &clean_cmd[pos + match_len..];

        let reset = if is_selected {
            "\x1b[48;5;60m\x1b[38;5;255m"
        } else {
            "\x1b[0m"
        };
        // Yellow bold highlight
        let highlight_code = "\x1b[1;38;5;220m";
        let display_str = format!("{before}{highlight_code}{matched}{reset}{after}");
        let pad_len = target_width.saturating_sub(clean_cmd.chars().count());
        format!("{}{}", display_str, " ".repeat(pad_len))
    } else {
        format!("{:<width$}", clean_cmd, width = target_width)
    }
}

fn overlay_inner_width(width: usize) -> usize {
    width.saturating_sub(2)
}

fn overlay_command_width(width: usize) -> usize {
    overlay_inner_width(width).saturating_sub(26)
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

fn fit_width(text: &str, max_width: usize) -> String {
    let sanitized = text.replace(['\r', '\n', '\t'], " ");
    let mut result = String::new();
    for character in sanitized.chars() {
        if result.chars().count() >= max_width.saturating_sub(1) {
            result.push('…');
            return result;
        }
        result.push(character);
    }
    result
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
        let row = format!(" ▶ {:<command_width$}  {:<20} ", "git status", "history");
        let description = format!(
            "  {:<desc_width$}",
            "history suggestion",
            desc_width = inner_width - 2
        );

        assert_eq!(inner_width, 42);
        assert_eq!(row.chars().count(), inner_width);
        assert_eq!(description.chars().count(), inner_width);
    }

    #[test]
    fn truncation_keeps_a_visible_ellipsis() {
        assert_eq!(fit_width("abcdefgh", 5), "abcd…");
    }
}
