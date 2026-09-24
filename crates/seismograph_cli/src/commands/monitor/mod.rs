// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod app;
mod client;
mod data;
mod filter;
mod filter_index;
mod filter_ui;
mod help;
mod help_content;
mod mouse;
mod offline;
mod panels;
#[cfg(test)]
mod profile;
mod snapshot;
mod ui;

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::{fmt, io};

use clap::Args;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Arguments for the live monitor TUI.
#[derive(Args, Clone, Copy, Debug)]
pub(crate) struct VerbArgs {
    #[cfg(test)]
    #[arg(skip)]
    pub(crate) terminal_error: Option<io::ErrorKind>,
}

/// Arguments for inspecting a saved snapshot.
#[derive(Args, Debug)]
pub(crate) struct ViewArgs {
    /// Native .seismograph file to open without connecting to a process.
    #[arg(value_name = "SNAPSHOT-FILE")]
    pub(crate) snapshot_file: PathBuf,
}

/// Opens the offline snapshot TUI.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn view(args: ViewArgs) -> Result<(), Error> {
    // Open before entering raw mode so missing files also fail in redirected shells.
    let file = std::fs::File::open(&args.snapshot_file).map_err(|error| Error::snapshot_file(&args.snapshot_file, error))?;
    let app = app::App::offline(args.snapshot_file.clone());
    let loader = offline::Loader::start(args.snapshot_file.clone(), file)?;
    run_terminal(app, Some(loader)).map_err(|error| match error {
        Error::SnapshotFile { .. } => error,
        error => Error::SnapshotFile {
            path: args.snapshot_file,
            message: error.to_string(),
        },
    })
}

/// Runs the live monitor TUI.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn verb(args: VerbArgs) -> Result<(), Error> {
    #[cfg(test)]
    if let Some(kind) = args.terminal_error {
        return Err(Error::Io(kind.into()));
    }
    #[cfg(not(test))]
    let _ = args;

    run_terminal(app::App::new(), None)
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn run_terminal(mut app: app::App, mut loader: Option<offline::Loader>) -> Result<(), Error> {
    let _terminal_guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(Error::Io)?;
    terminal.clear().map_err(Error::Io)?;
    app.refresh();
    let mut load_error = None;

    loop {
        if let Some(active) = &mut loader
            && let Some(result) = active.poll()
        {
            match result {
                Ok(snapshot) => app.finish_offline_load(snapshot),
                Err(error) => {
                    app.snapshot_error = Some(error.to_string());
                    app.status = "Snapshot loading failed; F1 explains unavailable data. q/Esc exits.".into();
                    load_error = Some(error);
                }
            }
            loader = None;
        }
        app.poll_discovery();
        app.poll_snapshot_capture();
        app.poll_filter();
        app.poll_recorder_statistics();
        app.poll_recording_configuration();
        terminal.draw(|frame| app.draw(frame)).map_err(Error::Io)?;
        let wait = app
            .next_refresh()
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(200));
        if event::poll(wait).map_err(Error::Io)? {
            match event::read().map_err(Error::Io)? {
                Event::Key(key) if should_exit(&mut app, key) => return load_error.map_or(Ok(()), Err),
                Event::Mouse(mouse) => {
                    let size = terminal.size().map_err(Error::Io)?;
                    app.handle_mouse(mouse, ratatui::layout::Rect::new(0, 0, size.width, size.height));
                }
                _ => {}
            }
        }
        if refresh_is_due(Instant::now(), app.next_refresh()) {
            app.refresh();
        }
    }
}

fn refresh_is_due(now: Instant, next_refresh: Instant) -> bool {
    now >= next_refresh
}

fn should_exit(app: &mut app::App, key: crossterm::event::KeyEvent) -> bool {
    let control_c = key.code == crossterm::event::KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
    key.kind == KeyEventKind::Press && ((control_c && app.help.is_none()) || app.handle_key(key.code))
}

struct TerminalGuard<W: Write, D: FnMut() -> io::Result<()>> {
    output: W,
    disable_raw_mode: D,
}

impl TerminalGuard<io::Stdout, fn() -> io::Result<()>> {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn enter() -> Result<Self, Error> {
        enable_raw_mode().map_err(Error::Io)?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture) {
            let _ = execute!(io::stdout(), DisableMouseCapture);
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(Error::Io(error));
        }
        Ok(Self {
            output: io::stdout(),
            disable_raw_mode,
        })
    }
}

impl<W: Write, D: FnMut() -> io::Result<()>> Drop for TerminalGuard<W, D> {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn drop(&mut self) {
        let _ = execute!(self.output, DisableMouseCapture);
        let _ = execute!(self.output, LeaveAlternateScreen);
        let _ = (self.disable_raw_mode)();
    }
}

/// Live monitor failure.
#[derive(Debug)]
pub(crate) enum Error {
    Io(io::Error),
    Protocol(seismograph_protocol::Error),
    MemorySnapshot(seismograph_rallocator::Error),
    Remote(String),
    Clock(String),
    MissingMemorySource,
    UnexpectedResponse,
    SnapshotFile { path: PathBuf, message: String },
}

impl Error {
    fn snapshot_file(path: &std::path::Path, error: impl fmt::Display) -> Self {
        Self::SnapshotFile {
            path: path.to_owned(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Protocol(error) => write!(formatter, "{error}"),
            Self::MemorySnapshot(error) => write!(formatter, "invalid rallocator snapshot: {error}"),
            Self::Remote(message) => write!(formatter, "monitor rejected the request: {message}"),
            Self::Clock(message) => write!(formatter, "system clock failed: {message}"),
            Self::MissingMemorySource => formatter.write_str("snapshot does not contain rallocator memory telemetry"),
            Self::UnexpectedResponse => formatter.write_str("monitor returned an unexpected response"),
            Self::SnapshotFile { path, message } => write!(formatter, "failed to open snapshot '{}': {message}", path.display()),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::MemorySnapshot(error) => Some(error),
            Self::Remote(_) | Self::Clock(_) | Self::MissingMemorySource | Self::UnexpectedResponse | Self::SnapshotFile { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::error::Error as _;
    use std::io;
    use std::time::Duration;

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::{Error, TerminalGuard, VerbArgs, app, refresh_is_due, should_exit, verb};

    #[test]
    fn verb_propagates_terminal_initialization_failure() {
        assert!(matches!(
            verb(VerbArgs {
                terminal_error: Some(io::ErrorKind::PermissionDenied),
            }),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied
        ));
    }

    #[test]
    fn monitor_errors_have_specific_messages() {
        assert_eq!(
            [
                Error::Io(io::Error::from(io::ErrorKind::PermissionDenied)).to_string(),
                Error::Remote("denied".into()).to_string(),
                Error::Clock("before epoch".into()).to_string(),
                Error::MissingMemorySource.to_string(),
                Error::UnexpectedResponse.to_string(),
            ],
            [
                "permission denied",
                "monitor rejected the request: denied",
                "system clock failed: before epoch",
                "snapshot does not contain rallocator memory telemetry",
                "monitor returned an unexpected response",
            ]
        );
    }

    #[test]
    fn monitor_error_sources_only_wrap_upstream_errors() {
        let io = Error::Io(io::Error::from(io::ErrorKind::BrokenPipe));
        let protocol = Error::Protocol(seismograph_protocol::read_response(&mut &[][..]).unwrap_err());
        let memory = Error::MemorySnapshot(seismograph_rallocator::decode(b"invalid").unwrap_err());
        assert!(io.source().is_some());
        assert!(protocol.source().is_some());
        assert!(memory.source().is_some());
        assert!(!protocol.to_string().is_empty());
        assert!(!memory.to_string().is_empty());
        assert!(Error::Remote("denied".into()).source().is_none());
        assert!(Error::Clock("before epoch".into()).source().is_none());
        assert!(Error::MissingMemorySource.source().is_none());
        assert!(Error::UnexpectedResponse.source().is_none());
    }

    #[test]
    fn key_events_only_exit_for_pressed_quit_keys() {
        let event = |code, modifiers, kind| KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        };
        assert_eq!(
            [
                event(KeyCode::Char('c'), KeyModifiers::CONTROL, KeyEventKind::Press),
                event(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Press),
                event(KeyCode::Char('c'), KeyModifiers::NONE, KeyEventKind::Press),
                event(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Release),
            ]
            .map(|key| should_exit(&mut app::App::new(), key)),
            [true, true, false, false]
        );
    }

    #[test]
    fn refresh_is_due_at_or_after_the_deadline() {
        let deadline = std::time::Instant::now();

        assert_eq!(
            [
                refresh_is_due(deadline.checked_sub(Duration::from_nanos(1)).unwrap(), deadline),
                refresh_is_due(deadline, deadline),
                refresh_is_due(deadline + Duration::from_nanos(1), deadline),
            ],
            [false, true, true]
        );
    }

    #[test]
    #[cfg_attr(all(miri, windows), ignore = "requires access to the Windows console")]
    fn terminal_guard_restores_terminal_when_dropped() {
        let mut output = Vec::new();
        let raw_mode_disabled = Cell::new(false);
        drop(TerminalGuard {
            output: &mut output,
            disable_raw_mode: || {
                raw_mode_disabled.set(true);
                Ok(())
            },
        });

        assert!(output.ends_with(b"\x1b[?1049l"));
        assert!(raw_mode_disabled.get());
    }
}
