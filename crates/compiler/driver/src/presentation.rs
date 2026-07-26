//! ADR 0027 terminal presentation confined to the `pop` driver boundary.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use ratatui::backend::CrosstermBackend;
#[cfg(test)]
use ratatui::backend::TestBackend;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use serde_json::{Value, json};

const DASHBOARD_HEIGHT: u16 = 7;
const MINIMUM_WIDTH: u16 = 48;
const MINIMUM_HEIGHT: u16 = 7;

static OPTIONS: OnceLock<Options> = OnceLock::new();
static INTERACTIVE_RENDERING: AtomicBool = AtomicBool::new(false);

type PanicHook = dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MessageFormat {
    #[default]
    Human,
    Json,
}

impl MessageFormat {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Request {
    pub(crate) interactive: bool,
    pub(crate) color: ColorChoice,
    pub(crate) message_format: MessageFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Options {
    interactive_requested: bool,
    interactive_available: bool,
    color_enabled: bool,
    message_format: MessageFormat,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            interactive_requested: false,
            interactive_available: false,
            color_enabled: false,
            message_format: MessageFormat::Human,
        }
    }
}

pub(crate) fn initialize(request: Request) {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    let presentation_terminal = io::stderr().is_terminal();
    let color_enabled = request.message_format == MessageFormat::Human
        && match request.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => presentation_terminal && !no_color,
        };
    let _ = OPTIONS.set(Options {
        interactive_requested: request.interactive,
        interactive_available: request.interactive
            && request.message_format == MessageFormat::Human
            && io::stdin().is_terminal()
            && presentation_terminal,
        color_enabled,
        message_format: request.message_format,
    });
}

fn options() -> Options {
    OPTIONS.get().copied().unwrap_or_default()
}

pub(crate) fn is_json() -> bool {
    options().message_format == MessageFormat::Json
}

pub(crate) fn write_json(value: &Value) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tone {
    Neutral,
    Information,
    Success,
    Warning,
    Error,
}

impl Tone {
    const fn ansi(self) -> &'static str {
        match self {
            Self::Neutral => "\u{1b}[0m",
            Self::Information => "\u{1b}[36m",
            Self::Success => "\u{1b}[32m",
            Self::Warning => "\u{1b}[33m",
            Self::Error => "\u{1b}[31m",
        }
    }

    const fn bold_ansi(self) -> &'static str {
        match self {
            Self::Neutral => "\u{1b}[1m",
            Self::Information => "\u{1b}[1;36m",
            Self::Success => "\u{1b}[1;32m",
            Self::Warning => "\u{1b}[1;33m",
            Self::Error => "\u{1b}[1;31m",
        }
    }
}

pub(crate) fn write_stderr(text: &str, tone: Tone) -> io::Result<()> {
    suspend_interactive_rendering();
    let mut stderr = io::stderr().lock();
    if options().color_enabled {
        stderr.write_all(tone.ansi().as_bytes())?;
        stderr.write_all(text.as_bytes())?;
        stderr.write_all(Tone::Neutral.ansi().as_bytes())?;
    } else {
        stderr.write_all(text.as_bytes())?;
    }
    Ok(())
}

pub(crate) fn write_stderr_line(text: &str, tone: Tone) -> io::Result<()> {
    let mut line = String::with_capacity(text.len() + 1);
    line.push_str(text);
    line.push('\n');
    write_stderr(&line, tone)
}

fn write_status_line(text: &str, tone: Tone) -> io::Result<()> {
    suspend_interactive_rendering();
    let mut stderr = io::stderr().lock();
    if !options().color_enabled {
        stderr.write_all(text.as_bytes())?;
        return stderr.write_all(b"\n");
    }

    let leading_width = text.len() - text.trim_start().len();
    let label_end = text[leading_width..]
        .find("  ")
        .map_or(text.len(), |separator| leading_width + separator);
    let (label, detail) = text.split_at(label_end);
    stderr.write_all(tone.bold_ansi().as_bytes())?;
    stderr.write_all(label.as_bytes())?;
    stderr.write_all(Tone::Neutral.ansi().as_bytes())?;
    stderr.write_all(detail.as_bytes())?;
    stderr.write_all(b"\n")
}

pub(crate) fn display_width(text: &str) -> usize {
    Line::from(text).width()
}

pub(crate) fn write_diagnostic(text: &str, tone: Tone) -> io::Result<()> {
    suspend_interactive_rendering();
    let mut stderr = io::stderr().lock();
    if !options().color_enabled {
        return stderr.write_all(text.as_bytes());
    }

    for (index, line) in text.split_inclusive('\n').enumerate() {
        let (content, newline) = line
            .strip_suffix('\n')
            .map_or((line, ""), |content| (content, "\n"));
        let trimmed = content.trim_start();
        if index == 0
            && let Some(end) = content.find(']')
        {
            let (heading, message) = content.split_at(end + 1);
            stderr.write_all(tone.bold_ansi().as_bytes())?;
            stderr.write_all(heading.as_bytes())?;
            stderr.write_all(Tone::Neutral.ansi().as_bytes())?;
            stderr.write_all(message.as_bytes())?;
        } else if trimmed.starts_with("-->") || trimmed.starts_with(":::") {
            stderr.write_all(Tone::Information.ansi().as_bytes())?;
            stderr.write_all(content.as_bytes())?;
            stderr.write_all(Tone::Neutral.ansi().as_bytes())?;
        } else if let Some((gutter, body)) = content.split_once('|') {
            stderr.write_all(Tone::Information.ansi().as_bytes())?;
            stderr.write_all(gutter.as_bytes())?;
            stderr.write_all(b"|")?;
            if body.contains('^') {
                stderr.write_all(tone.ansi().as_bytes())?;
                stderr.write_all(body.as_bytes())?;
            } else {
                stderr.write_all(Tone::Neutral.ansi().as_bytes())?;
                stderr.write_all(body.as_bytes())?;
            }
            stderr.write_all(Tone::Neutral.ansi().as_bytes())?;
        } else if !trimmed.is_empty()
            && let Some(separator) = content.find(':')
        {
            let (label, detail) = content.split_at(separator + 1);
            stderr.write_all(Tone::Information.bold_ansi().as_bytes())?;
            stderr.write_all(label.as_bytes())?;
            stderr.write_all(Tone::Neutral.ansi().as_bytes())?;
            stderr.write_all(detail.as_bytes())?;
        } else {
            stderr.write_all(Tone::Neutral.ansi().as_bytes())?;
            stderr.write_all(content.as_bytes())?;
        }
        stderr.write_all(newline.as_bytes())?;
    }
    stderr.write_all(Tone::Neutral.ansi().as_bytes())
}

fn suspend_interactive_rendering() {
    if INTERACTIVE_RENDERING.swap(false, Ordering::AcqRel) {
        let _ = execute!(io::stderr(), Show);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Working,
    Succeeded,
    Failed,
}

#[derive(Debug)]
struct DashboardState<'text> {
    command: &'text str,
    phase: &'text str,
    completed: u32,
    total: u32,
    outcome: Outcome,
    color: bool,
}

fn dashboard_fits(area: Rect) -> bool {
    area.width >= MINIMUM_WIDTH && area.height >= MINIMUM_HEIGHT
}

fn render_dashboard(frame: &mut Frame<'_>, state: &DashboardState<'_>) {
    let area = frame.area();
    if !dashboard_fits(area) {
        return;
    }

    let [heading, progress, status] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(area);
    let accent = if state.color {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let result_style = if state.color {
        match state.outcome {
            Outcome::Working => Style::default().fg(Color::Yellow),
            Outcome::Succeeded => Style::default().fg(Color::Green),
            Outcome::Failed => Style::default().fg(Color::Red),
        }
    } else {
        Style::default()
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Pop Lang", accent),
            Span::raw("  "),
            Span::raw(state.command),
        ]))
        .block(Block::default().borders(Borders::ALL).title(" Command ")),
        heading,
    );

    let ratio = if state.total == 0 {
        0.0
    } else {
        f64::from(state.completed.min(state.total)) / f64::from(state.total)
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(state.phase))
            .gauge_style(accent)
            .ratio(ratio)
            .label(format!("[{}/{}]", state.completed, state.total)),
        progress,
    );

    let status_text = match state.outcome {
        Outcome::Working => "WORKING",
        Outcome::Succeeded => "FINISHED",
        Outcome::Failed => "FAILED",
    };
    frame.render_widget(
        Paragraph::new(format!("Status: {status_text}")).style(result_style),
        status,
    );
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stderr>>,
    previous_hook: Arc<PanicHook>,
}

impl TerminalSession {
    fn start(command: &str, phase: &str, color: bool) -> io::Result<Self> {
        let (width, height) = terminal::size()?;
        if !dashboard_fits(Rect::new(0, 0, width, height)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal is too small for the interactive command dashboard",
            ));
        }
        let previous_hook: Arc<PanicHook> = Arc::from(std::panic::take_hook());
        let panic_previous = Arc::clone(&previous_hook);
        std::panic::set_hook(Box::new(move |information| {
            suspend_interactive_rendering();
            panic_previous(information);
        }));

        let backend = CrosstermBackend::new(io::stderr());
        let mut terminal = match Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(DASHBOARD_HEIGHT),
            },
        ) {
            Ok(terminal) => terminal,
            Err(error) => {
                let restore_previous = Arc::clone(&previous_hook);
                std::panic::set_hook(Box::new(move |information| {
                    restore_previous(information);
                }));
                return Err(error);
            }
        };
        INTERACTIVE_RENDERING.store(true, Ordering::Release);
        if let Err(error) = terminal.draw(|frame| {
            render_dashboard(
                frame,
                &DashboardState {
                    command,
                    phase,
                    completed: 0,
                    total: 1,
                    outcome: Outcome::Working,
                    color,
                },
            );
        }) {
            INTERACTIVE_RENDERING.store(false, Ordering::Release);
            let restore_previous = Arc::clone(&previous_hook);
            std::panic::set_hook(Box::new(move |information| {
                restore_previous(information);
            }));
            return Err(error);
        }
        Ok(Self {
            terminal,
            previous_hook,
        })
    }

    fn finish(&mut self, command: &str, phase: &str, success: bool, color: bool) {
        if !INTERACTIVE_RENDERING.load(Ordering::Acquire) {
            return;
        }
        let _ = self.terminal.draw(|frame| {
            render_dashboard(
                frame,
                &DashboardState {
                    command,
                    phase,
                    completed: 1,
                    total: 1,
                    outcome: if success {
                        Outcome::Succeeded
                    } else {
                        Outcome::Failed
                    },
                    color,
                },
            );
        });
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        suspend_interactive_rendering();
        let previous = Arc::clone(&self.previous_hook);
        std::panic::set_hook(Box::new(move |information| {
            previous(information);
        }));
    }
}

pub(crate) struct CommandFeedback {
    command: String,
    phase: String,
    phase_id: String,
    terminal: Option<TerminalSession>,
}

impl CommandFeedback {
    pub(crate) fn start(
        command: impl Into<String>,
        phase: impl Into<String>,
        phase_id: impl Into<String>,
        fallback_message: &str,
        started_message: &str,
    ) -> Self {
        let command = command.into();
        let phase = phase.into();
        let phase_id = phase_id.into();
        let settings = options();
        let terminal = if settings.message_format == MessageFormat::Json {
            let _ = write_json(&json!({
                "schemaVersion": 1,
                "kind": "commandStarted",
                "command": command,
                "phase": phase_id,
            }));
            None
        } else if settings.interactive_available {
            if let Ok(session) = TerminalSession::start(&command, &phase, settings.color_enabled) {
                Some(session)
            } else {
                let _ = write_stderr_line(fallback_message, Tone::Information);
                None
            }
        } else {
            if settings.interactive_requested && settings.message_format == MessageFormat::Human {
                let _ = write_stderr_line(fallback_message, Tone::Information);
            }
            None
        };
        if terminal.is_none() && settings.message_format == MessageFormat::Human {
            let _ = write_status_line(started_message, Tone::Information);
        }
        Self {
            command,
            phase,
            phase_id,
            terminal,
        }
    }

    pub(crate) fn finish(mut self, success: bool, progress_message: &str, finished_message: &str) {
        let settings = options();
        if settings.message_format == MessageFormat::Json {
            let _ = write_json(&json!({
                "schemaVersion": 1,
                "kind": "commandProgress",
                "command": self.command,
                "phase": self.phase_id,
                "completed": 1,
                "total": 1,
            }));
            let _ = write_json(&json!({
                "schemaVersion": 1,
                "kind": "commandFinished",
                "command": self.command,
                "outcome": if success { "success" } else { "failure" },
            }));
            return;
        }
        if let Some(terminal) = &mut self.terminal {
            terminal.finish(&self.command, &self.phase, success, settings.color_enabled);
            if INTERACTIVE_RENDERING.load(Ordering::Acquire) {
                return;
            }
        }
        if settings.message_format == MessageFormat::Human {
            let tone = if success { Tone::Success } else { Tone::Error };
            let _ = write_status_line(progress_message, Tone::Information);
            let _ = write_status_line(finished_message, tone);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_text(backend: &TestBackend) -> String {
        let area = backend.buffer().area;
        let mut output = String::new();
        for row in 0..area.height {
            for column in 0..area.width {
                output.push_str(backend.buffer()[(column, row)].symbol());
            }
            output.push('\n');
        }
        output
    }

    #[test]
    fn test_backend_renders_textual_progress_without_color() {
        let backend = TestBackend::new(72, DASHBOARD_HEIGHT);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                render_dashboard(
                    frame,
                    &DashboardState {
                        command: "check",
                        phase: "Checking",
                        completed: 1,
                        total: 1,
                        outcome: Outcome::Succeeded,
                        color: false,
                    },
                );
            })
            .expect("render dashboard");

        let rendered = buffer_text(terminal.backend());
        assert!(rendered.contains("Pop Lang"), "{rendered}");
        assert!(rendered.contains("check"), "{rendered}");
        assert!(rendered.contains("Checking"), "{rendered}");
        assert!(rendered.contains("[1/1]"), "{rendered}");
        assert!(rendered.contains("Status: FINISHED"), "{rendered}");
    }

    #[test]
    fn too_small_layout_renders_nothing_instead_of_truncating_a_state() {
        let backend = TestBackend::new(MINIMUM_WIDTH - 1, MINIMUM_HEIGHT);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                render_dashboard(
                    frame,
                    &DashboardState {
                        command: "build",
                        phase: "Building",
                        completed: 0,
                        total: 1,
                        outcome: Outcome::Working,
                        color: false,
                    },
                );
            })
            .expect("render undersized dashboard");

        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.symbol() == " "),
            "an undersized decision surface must fall back instead of truncate"
        );
    }
}
