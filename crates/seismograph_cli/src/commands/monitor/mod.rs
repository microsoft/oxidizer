// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod app;
mod client;
mod data;
mod ui;

use std::time::{Duration, Instant};
use std::{fmt, io};

use clap::Args;
use crossterm::event::{self, Event, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Arguments for the live monitor TUI.
#[derive(Args, Debug)]
pub(crate) struct VerbArgs;

/// Runs the live monitor TUI.
#[cfg_attr(
    test,
    expect(dead_code, reason = "unit tests replace interactive command dispatch with a deterministic stub")
)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn verb(_args: VerbArgs) -> Result<(), Error> {
    let _terminal_guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(Error::Io)?;
    terminal.clear().map_err(Error::Io)?;

    let mut app = app::App::new();
    app.refresh();
    loop {
        app.poll_discovery();
        app.poll_snapshot_capture();
        app.poll_recorder_statistics();
        terminal.draw(|frame| app.draw(frame)).map_err(Error::Io)?;
        let wait = app
            .next_refresh()
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(200));
        if event::poll(wait).map_err(Error::Io)? {
            let Event::Key(key) = event::read().map_err(Error::Io)? else {
                continue;
            };
            let control_c = key.code == crossterm::event::KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
            if key.kind == KeyEventKind::Press && (control_c || app.handle_key(key.code)) {
                return Ok(());
            }
        }
        if Instant::now() >= app.next_refresh() {
            app.refresh();
        }
    }
}

struct TerminalGuard;

impl TerminalGuard {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn enter() -> Result<Self, Error> {
        enable_raw_mode().map_err(Error::Io)?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(Error::Io(error));
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
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
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Protocol(error) => Some(error),
            Self::MemorySnapshot(error) => Some(error),
            Self::Remote(_) | Self::Clock(_) | Self::MissingMemorySource | Self::UnexpectedResponse => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::io;

    use super::Error;

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
}
