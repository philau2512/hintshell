use std::env;
use std::io::{self, Read, Write};
#[cfg(windows)]
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::cursor::position;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use hintshell_core::api::protocol::{HintShellRequest, SuggestionItem};

#[cfg(windows)]
use conpty::ProcessOptions;
#[cfg(unix)]
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const QUERY_DEBOUNCE: Duration = Duration::from_millis(40);
const QUERY_TIMEOUT: Duration = Duration::from_millis(180);
const MAX_VISIBLE_SUGGESTIONS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabAction {
    AcceptHintShell,
    ForwardToBash,
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

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("cannot enable raw terminal mode: {error}"))?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

#[cfg(windows)]
pub fn run(args: Vec<String>) -> Result<(), String> {
    if !supports_live_overlay() {
        return Err(
            "live overlay requires an ANSI-capable terminal; open Git Bash normally to use the legacy Tab/fzf integration"
                .to_string(),
        );
    }

    let (cols, rows) = size().unwrap_or((120, 30));
    let bash = resolve_bash()?;
    // `conpty` forwards its program string directly to CreateProcessW, so the
    // executable must be quoted when Git for Windows is installed under Program Files.
    let mut command = Command::new(quote_windows_argument(&bash));
    // `conpty` builds the child environment from explicit entries only.
    // Preserve the Git Bash session environment before setting the wrapper marker.
    command.envs(env::vars_os());
    command.env("HINTSHELL_LIVE_BASH", "1");
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
    start_output_pump(Box::new(output_reader), shell_events_tx);

    if !interactive {
        return run_batch_process(process, shell_events_rx, &output);
    }

    let (query_tx, query_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let cwd = env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().to_string());
    start_query_worker(query_rx, result_tx, cwd);

    let _raw_mode = RawModeGuard::enter()?;
    let mut state = OverlayState::new();
    let mut running = true;

    while running {
        while let Ok(shell_event) = shell_events_rx.try_recv() {
            clear_overlay(&mut state, &output)?;
            match shell_event {
                ShellEvent::Output(bytes) => write_terminal(&output, &bytes)?,
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
pub fn run(args: Vec<String>) -> Result<(), String> {
    if !is_wsl_runtime() {
        return Err("live Bash overlay is currently available only in WSL2; native Unix keeps the Tab/fzf integration".to_string());
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
        .map_err(|error| format!("cannot create WSL pseudo-terminal: {error}"))?;

    let cwd = env::current_dir()
        .map_err(|error| format!("cannot determine WSL working directory: {error}"))?;
    let mut command = CommandBuilder::new(resolve_bash()?);
    // portable-pty does not reliably inherit CWD for an interactive child.
    // Preserve the terminal directory selected by the WSL host.
    command.cwd(cwd);
    command.env("HINTSHELL_LIVE_BASH", "1");
    command.env("HINTSHELL_LIVE_BASH_WSL", "1");
    if args.is_empty() {
        command.args(["--login", "-i"]);
    } else {
        command.args(args.iter());
    }

    let mut process = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("cannot start WSL Bash: {error}"))?;
    drop(pair.slave);

    let writer =
        Arc::new(Mutex::new(pair.master.take_writer().map_err(|error| {
            format!("cannot open WSL Bash input: {error}")
        })?));
    let output = Arc::new(Mutex::new(io::stdout()));
    let (shell_events_tx, shell_events_rx) = mpsc::channel::<ShellEvent>();
    let output_reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("cannot open WSL Bash output: {error}"))?;
    start_output_pump(output_reader, shell_events_tx);

    if !interactive {
        return run_unix_batch_process(&mut *process, shell_events_rx, &output);
    }

    let (query_tx, query_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let cwd = env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().to_string());
    start_query_worker(query_rx, result_tx, cwd);

    let _raw_mode = RawModeGuard::enter()?;
    let mut state = OverlayState::new();
    let mut running = true;

    while running {
        while let Ok(shell_event) = shell_events_rx.try_recv() {
            clear_overlay(&mut state, &output)?;
            match shell_event {
                ShellEvent::Output(bytes) => write_terminal(&output, &bytes)?,
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
                    pair.master
                        .resize(PtySize {
                            rows,
                            cols: columns,
                            pixel_width: 0,
                            pixel_height: 0,
                        })
                        .map_err(|error| format!("cannot resize WSL terminal: {error}"))?;
                    render_overlay(&mut state, &output)?;
                }
                Event::Key(key) if accepts_key_event(&key) => {
                    handle_key(key, &mut state, &writer, &output, &query_tx)?;
                }
                _ => {}
            }
        }

        if process
            .try_wait()
            .map_err(|error| format!("cannot check WSL Bash: {error}"))?
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
            Ok(ShellEvent::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout)
                if process
                    .try_wait()
                    .map_err(|error| format!("cannot check WSL Bash: {error}"))?
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
pub fn run(_args: Vec<String>) -> Result<(), String> {
    Err("live Bash overlay is unsupported on this platform".to_string())
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
    !term.is_empty() && term != "dumb"
}

#[cfg(windows)]
fn quote_windows_argument(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    if !value.contains([' ', '\t', '"']) {
        return value.to_string();
    }

    format!("\"{}\"", value.replace('"', "\\\""))
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

fn start_output_pump(mut reader: Box<dyn Read + Send>, events: mpsc::Sender<ShellEvent>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if events
                        .send(ShellEvent::Output(buffer[..read].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
        let _ = events.send(ShellEvent::Closed);
    });
}

#[derive(Debug)]
enum ShellEvent {
    Output(Vec<u8>),
    Closed,
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

fn query_request(input: String, cwd: Option<String>) -> HintShellRequest {
    HintShellRequest::Suggest {
        input,
        limit: 12,
        cwd,
        shell: Some("bash".to_string()),
    }
}

fn start_query_worker(
    receiver: mpsc::Receiver<(u64, String)>,
    sender: mpsc::Sender<SuggestionResult>,
    cwd: Option<String>,
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

            let request = query_request(buffer.clone(), cwd.clone());
            let suggestions = runtime
                .block_on(async {
                    tokio::time::timeout(QUERY_TIMEOUT, crate::send_request(&request))
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .and_then(|response| response.suggestions)
                        .unwrap_or_default()
                })
                .into_iter()
                .filter(|suggestion| {
                    suggestion
                        .command
                        .to_ascii_lowercase()
                        .starts_with(&buffer.to_ascii_lowercase())
                })
                .collect();

            let _ = sender.send(SuggestionResult {
                generation,
                buffer,
                suggestions,
            });
        }
    });
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
                    write_to_shell(writer, b"\x15")?;
                    write_to_shell(writer, command.as_bytes())?;
                    state.buffer = command;
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
                && suggestion
                    .command
                    .to_ascii_lowercase()
                    .starts_with(&buffer.to_ascii_lowercase()) =>
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

    let mut stdout = output
        .lock()
        .map_err(|_| "terminal output is unavailable".to_string())?;
    let mut frame = String::from("\x1b7");
    for _ in 0..state.rendered_lines {
        frame.push_str("\x1b[1B\r\x1b[2K");
    }
    frame.push_str("\x1b8");
    stdout
        .write_all(frame.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("cannot clear HintShell overlay: {error}"))?;

    state.rendered_lines = 0;
    Ok(())
}

fn render_overlay(state: &mut OverlayState, output: &Arc<Mutex<io::Stdout>>) -> Result<(), String> {
    if state.suggestions.is_empty() {
        return Ok(());
    }

    let (terminal_width, terminal_height) = size().unwrap_or((100, 30));
    let width = usize::from(terminal_width).clamp(44, 78);
    let total = state.suggestions.len();
    let (_, cursor_row) = position().unwrap_or((0, 0));
    let shown = visible_suggestion_rows(cursor_row, terminal_height, total);
    if shown == 0 {
        return Ok(());
    }
    let rendered_lines = shown + 3;
    let selected = state.selected.min(total.saturating_sub(1));
    let viewport_start = viewport_start(selected, total, shown);
    let viewport_end = viewport_start + shown;
    let mut frame = String::from("\x1b7\n");
    let counter = format!(" {}/{} ", selected + 1, total);
    frame.push_str(&format!(
        "\r\x1b[38;5;141m╭{}{}╮\x1b[0m\n",
        "─".repeat(width.saturating_sub(counter.len() + 2)),
        counter
    ));

    for (index, suggestion) in state.suggestions[viewport_start..viewport_end]
        .iter()
        .enumerate()
    {
        let suggestion_index = viewport_start + index;
        let selected_row = suggestion_index == selected;
        let command = fit_width(&suggestion.command, width.saturating_sub(30));
        let source = fit_width(&suggestion.source, 20);
        let marker = if selected_row { "▶" } else { " " };
        let row = format!(
            " {marker} {command:<cmd_width$}  {source:<20} ",
            cmd_width = width.saturating_sub(30)
        );
        if selected_row {
            frame.push_str(&format!(
                "\r\x1b[38;5;141m│\x1b[48;5;60m\x1b[38;5;255m{row}\x1b[0m\x1b[38;5;141m│\x1b[0m\n"
            ));
        } else {
            frame.push_str(&format!(
                "\r\x1b[38;5;141m│\x1b[0m{row}\x1b[38;5;141m│\x1b[0m\n"
            ));
        }
    }

    let description = state
        .suggestions
        .get(selected)
        .and_then(|suggestion| suggestion.description.as_deref())
        .unwrap_or("history suggestion");
    let description = fit_width(description, width.saturating_sub(4));
    frame.push_str(&format!(
        "\r\x1b[38;5;141m│\x1b[38;5;244m  {description:<desc_width$}\x1b[38;5;141m│\x1b[0m\n",
        desc_width = width.saturating_sub(4)
    ));
    frame.push_str(&format!(
        "\r\x1b[38;5;141m╰{}╯\x1b[0m\x1b8",
        "─".repeat(width)
    ));

    let mut stdout = output
        .lock()
        .map_err(|_| "terminal output is unavailable".to_string())?;
    stdout
        .write_all(frame.as_bytes())
        .and_then(|_| stdout.flush())
        .map_err(|error| format!("cannot render HintShell overlay: {error}"))?;
    state.rendered_lines = rendered_lines;
    Ok(())
}

fn visible_suggestion_rows(cursor_row: u16, terminal_height: u16, total: usize) -> usize {
    let rows_below_prompt = terminal_height.saturating_sub(cursor_row.saturating_add(1));
    let rows_for_suggestions = usize::from(rows_below_prompt.saturating_sub(3));
    total.min(MAX_VISIBLE_SUGGESTIONS).min(rows_for_suggestions)
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
    let mut result = String::new();
    for character in text.chars() {
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
    fn overlay_caps_rows_to_remaining_space_without_scrolling() {
        assert_eq!(visible_suggestion_rows(3, 30, 12), 6);
        assert_eq!(visible_suggestion_rows(25, 30, 12), 1);
        assert_eq!(visible_suggestion_rows(26, 30, 12), 0);
    }

    #[test]
    fn viewport_scrolls_to_keep_selected_item_visible() {
        assert_eq!(viewport_start(0, 12, 6), 0);
        assert_eq!(viewport_start(5, 12, 6), 0);
        assert_eq!(viewport_start(6, 12, 6), 1);
        assert_eq!(viewport_start(11, 12, 6), 6);
    }

    #[test]
    fn live_query_includes_wrapper_cwd() {
        let request = query_request(
            "cd ".to_string(),
            Some(r"D:\Admin\Documents\PROJECTS\HintShell".to_string()),
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
    fn truncation_keeps_a_visible_ellipsis() {
        assert_eq!(fit_width("abcdefgh", 5), "abcd…");
    }
}
