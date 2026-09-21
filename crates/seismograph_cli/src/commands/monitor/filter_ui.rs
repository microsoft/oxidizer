// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crossterm::event::KeyCode;
use performables::sync::channel::{Receiver, unbounded};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::app::{App, Screen};
use super::data::CapturedSnapshot;
use super::filter::{FilterSpec, RuntimeStackMode};
use super::filter_index::FilterIndex;

#[derive(Default)]
pub(super) struct Filters {
    pub(super) applied: FilterSpec,
    pub(super) popup: Option<FilterPopup>,
    pending: Option<FilterSpec>,
    queued: bool,
    receiver: Option<Receiver<FilterCompletion>>,
    generation: u64,
    started_at: Option<Instant>,
}

struct FilterCompletion {
    generation: u64,
    index: Arc<FilterIndex>,
    spec: FilterSpec,
    snapshot: Box<CapturedSnapshot>,
}

#[derive(Clone, Debug)]
pub(super) struct FilterPopup {
    includes: String,
    excludes: String,
    show_unknown: bool,
    runtime_stack: RuntimeStackMode,
    selected: usize,
    error: Option<String>,
}

impl FilterPopup {
    fn new(spec: &FilterSpec) -> Self {
        Self {
            includes: spec.includes().to_owned(),
            excludes: spec.excludes().to_owned(),
            show_unknown: spec.show_unknown(),
            runtime_stack: spec.runtime_stack,
            selected: 0,
            error: None,
        }
    }

    fn edit(&mut self, code: KeyCode) {
        match code {
            KeyCode::Tab | KeyCode::Down => self.selected = (self.selected + 1) % 4,
            KeyCode::BackTab | KeyCode::Up => self.selected = (self.selected + 3) % 4,
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') if self.selected >= 2 => {
                if self.selected == 2 {
                    self.show_unknown = !self.show_unknown;
                } else {
                    self.runtime_stack = match self.runtime_stack {
                        RuntimeStackMode::Event => RuntimeStackMode::Spawn,
                        RuntimeStackMode::Spawn => RuntimeStackMode::Event,
                    };
                }
            }
            KeyCode::Char(character) if self.selected < 2 => {
                self.text().push(character);
                self.error = None;
            }
            KeyCode::Backspace | KeyCode::Delete if self.selected < 2 => {
                self.text().pop();
                self.error = None;
            }
            _ => {}
        }
    }

    fn text(&mut self) -> &mut String {
        if self.selected == 0 {
            &mut self.includes
        } else {
            &mut self.excludes
        }
    }

    fn parse(&self) -> Result<FilterSpec, super::filter::FilterParseError> {
        FilterSpec::parse(&self.includes, &self.excludes, self.show_unknown, self.runtime_stack)
    }
}

impl Filters {
    /// Cancel publication, but retain the worker slot until its CPU work finishes.
    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.queued = false;
    }

    fn start(&mut self, capture: &CapturedSnapshot, spec: FilterSpec) -> Result<(), String> {
        let index = capture
            .filter_index
            .as_ref()
            .ok_or("This snapshot has no filter index; capture a new snapshot or reopen the native file.")?;
        if self.receiver.is_some() {
            self.generation = self.generation.wrapping_add(1);
            self.pending = Some(spec);
            self.queued = true;
            return Ok(());
        }
        let index = Arc::clone(index);
        let captured_at = capture.captured_at;
        let captured_instant = capture.captured_instant;
        let heap_error = capture.heap_error.clone();
        let generation = self.generation.wrapping_add(1);
        let worker_spec = spec.clone();
        let (sender, receiver) = unbounded();
        thread::Builder::new()
            .name("seismograph-filter".into())
            .spawn(move || {
                let mut snapshot = index.render(&worker_spec);
                snapshot.captured_at = captured_at;
                snapshot.captured_instant = captured_instant;
                snapshot.heap_error = heap_error;
                let completion = FilterCompletion {
                    generation,
                    index,
                    spec: worker_spec,
                    snapshot,
                };
                // The receiver only closes on application shutdown.
                let _receiver_closed = sender.send_sync(completion);
            })
            .map_err(|error| format!("Failed to start filter worker: {error}"))?;
        self.generation = generation;
        self.pending = Some(spec);
        self.receiver = Some(receiver);
        self.started_at = Some(Instant::now());
        Ok(())
    }

    fn poll(&mut self) -> Option<Result<FilterCompletion, String>> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(completion) => {
                self.receiver = None;
                self.started_at = None;
                Some(Ok(completion))
            }
            Err(error) if error.is_empty() => None,
            Err(_) => {
                self.receiver = None;
                self.started_at = None;
                Some(Err("Filter worker stopped unexpectedly; press F to retry.".into()))
            }
        }
    }
}

impl App {
    pub(super) fn open_filter_popup(&mut self) {
        match self.filter_capture() {
            None if matches!(self.screen, Screen::Browse) => {
                self.status = "Connect to an application with Enter, then capture with s; or open a native snapshot file.".into();
            }
            None => self.status = "No snapshot to filter; press s to capture, or wait for the offline file to load.".into(),
            Some(capture) if capture.filter_index.is_none() => {
                self.status = "This snapshot has no filter index; capture again or reopen the native file.".into();
            }
            Some(_) => {
                let requested = self.filters.pending.as_ref().unwrap_or(&self.filters.applied);
                self.filters.popup = Some(FilterPopup::new(requested));
            }
        }
    }

    pub(super) fn handle_filter_key(&mut self, code: KeyCode) {
        if code == KeyCode::Esc {
            self.filters.popup = None;
            return;
        }
        let Some(popup) = self.filters.popup.as_mut() else {
            return;
        };
        if code != KeyCode::Enter {
            popup.edit(code);
            return;
        }
        match popup.parse() {
            Ok(spec) => match self.start_filter(spec) {
                Ok(()) => self.filters.popup = None,
                Err(error) => {
                    self.status.clone_from(&error);
                    if let Some(popup) = &mut self.filters.popup {
                        popup.error = Some(error);
                    }
                }
            },
            Err(error) => popup.error = Some(error.to_string()),
        }
    }

    fn start_filter(&mut self, spec: FilterSpec) -> Result<(), String> {
        let capture = match &self.screen {
            Screen::Connected { snapshot, .. } | Screen::Offline { snapshot, .. } => snapshot.as_deref(),
            Screen::Browse => None,
        }
        .ok_or("No snapshot to filter; capture a snapshot or open a native file first.")?;
        self.filters.start(capture, spec)?;
        self.status = if self.filters.queued {
            "Latest filter queued; waiting for the current filter worker to finish.".into()
        } else {
            "Filtering snapshot in background; current view remains available.".into()
        };
        Ok(())
    }

    pub(super) fn filter_snapshot_arrived(&mut self) {
        let requested = self.filters.pending.clone().unwrap_or_else(|| self.filters.applied.clone());
        self.filters.invalidate();
        if requested.is_active() {
            if let Err(error) = self.start_filter(requested) {
                self.status = error;
            }
        } else {
            self.filters.applied = requested;
        }
    }

    pub(super) fn poll_filter(&mut self) {
        let Some(result) = self.filters.poll() else {
            return;
        };
        let pending = self.filters.pending.take();
        let queued = std::mem::take(&mut self.filters.queued);
        match result {
            Ok(completion) => self.finish_filter(completion),
            Err(error) => self.status = error,
        }
        if queued
            && let Some(spec) = pending
            && let Err(error) = self.start_filter(spec)
        {
            self.status = error;
        }
    }

    fn finish_filter(&mut self, completion: FilterCompletion) {
        if completion.generation != self.filters.generation
            || !self
                .filter_capture()
                .and_then(|capture| capture.filter_index.as_ref())
                .is_some_and(|index| Arc::ptr_eq(index, &completion.index))
        {
            return;
        }
        match &mut self.screen {
            Screen::Connected { snapshot, .. } | Screen::Offline { snapshot, .. } => *snapshot = Some(completion.snapshot),
            Screen::Browse => return,
        }
        self.filters.applied = completion.spec;
        self.reset_filtered_views();
        self.status = if self.filters.applied.is_active() {
            "Stack filters applied; source accepted/overwritten and global counters, and heap topology are unfiltered.".into()
        } else {
            "Stack filters cleared; all captured records are shown.".into()
        };
    }

    fn filter_capture(&self) -> Option<&CapturedSnapshot> {
        match &self.screen {
            Screen::Connected { snapshot, .. } | Screen::Offline { snapshot, .. } => snapshot.as_deref(),
            Screen::Browse => None,
        }
    }

    pub(super) fn filter_banner_height(&self) -> u16 {
        if !matches!(self.screen, Screen::Browse) && (self.filters.applied.is_active() || self.filters.started_at.is_some()) {
            4
        } else {
            0
        }
    }

    pub(super) fn draw_filter_banner(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        if area.height == 0 {
            return;
        }
        let mut lines = Vec::new();
        if let Some(start) = self.filters.started_at {
            let state = if self.filters.queued { "Latest filter queued" } else { "Filtering" };
            lines.push(Line::from(format!(
                " {state}... {:.1}s - current view remains available",
                start.elapsed().as_secs_f64()
            )));
        } else if self.filter_capture().is_some_and(|capture| capture.filter_summary.active) {
            lines.push(Line::from(" Stack filters active (F to edit)"));
        } else {
            lines.push(Line::from(" Stack filter rules retained - current view is unfiltered (F to retry)"));
        }
        if let Some(capture) = self.filter_capture() {
            let summary = capture.filter_summary;
            let count = |name, counts: super::filter_index::FilterCounts| {
                format!("{name} {}/{} (unknown {})", counts.shown, counts.total, counts.unknown)
            };
            lines.push(Line::from(format!(
                " {} | {}",
                count("Events", summary.events),
                count("Allocations", summary.allocations)
            )));
            lines.push(Line::from(format!(" {}", count("Tasks", summary.tasks))));
        } else {
            lines.push(Line::from(" Waiting for an indexed snapshot."));
            lines.push(Line::default());
        }
        lines.push(Line::from(
            " Unfiltered: source accepted/overwritten counters, whole-process counters and heap topology.",
        ));
        frame.render_widget(Paragraph::new(lines).style(Style::default().fg(Color::Yellow)), area);
    }

    pub(super) fn draw_filter_popup(&self, frame: &mut ratatui::Frame<'_>) {
        let Some(popup) = &self.filters.popup else {
            return;
        };
        let area = frame.area();
        let width = area.width.min(100);
        let height = area.height.min(17);
        let area = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let block = Block::default()
            .title(" Stack filters - whole records ")
            .title_bottom(" F1 help | Tab/up/down fields | Enter apply | Esc cancel ")
            .borders(Borders::ALL);
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        let [help, fields, error] = Layout::vertical([Constraint::Length(5), Constraint::Length(4), Constraint::Min(1)]).areas(inner);
        frame.render_widget(
            Paragraph::new(
                "Comma/space-separated: crate:name module:crate::module\n\
                 function:crate::module::name | Empty rules reset all records.\n\
                 Any captured frame; includes OR, exclusions win.\n\
                 Symbol owners, not execution lineage or generic type arguments.\n\
                 Missing stacks: enable backtraces or show Unknown. Edit text at end.",
            )
            .wrap(Wrap { trim: false }),
            help,
        );
        let rows = [
            ("Include", popup.includes.clone()),
            ("Exclude", popup.excludes.clone()),
            ("Unknown stacks", if popup.show_unknown { "show" } else { "hide" }.into()),
            (
                "Runtime stack",
                match popup.runtime_stack {
                    RuntimeStackMode::Event => "event stack (left/right to change)",
                    RuntimeStackMode::Spawn => "spawn provenance (left/right to change)",
                }
                .into(),
            ),
        ];
        for (index, (label, value)) in rows.into_iter().enumerate() {
            let y = fields.y.saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
            if y >= fields.bottom() {
                break;
            }
            let selected = index == popup.selected;
            let prefix = format!("{}{label}: ", if selected { "> " } else { "  " });
            let value = if selected && index < 2 { format!("{value}|") } else { value };
            let prefix_width = Line::from(prefix.as_str()).width();
            let value = visible_tail(&value, usize::from(fields.width).saturating_sub(prefix_width));
            let style = if selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            frame.render_widget(
                Paragraph::new(format!("{prefix}{value}")).style(style),
                Rect::new(fields.x, y, fields.width, 1),
            );
        }
        frame.render_widget(
            Paragraph::new(
                popup
                    .error
                    .as_deref()
                    .unwrap_or("Left/right/space changes options. Backspace/Delete removes the last character."),
            )
            .style(Style::default().fg(if popup.error.is_some() { Color::Red } else { Color::Gray }))
            .wrap(Wrap { trim: false }),
            error,
        );
    }
}

fn visible_tail(value: &str, width: usize) -> String {
    let mut used = 0;
    let characters: Vec<_> = value
        .chars()
        .rev()
        .take_while(|character| {
            used += Line::from(character.to_string()).width();
            used <= width
        })
        .collect();
    characters.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::{Duration, SystemTime};

    use seismograph::recorder::event::{Address, Event, EventKind, EventPayload, EventSequence, EventTimestamp, Events, ObjectId};
    use seismograph::recorder::thread::ThreadId;
    use seismograph::snapshot::DecodedSnapshot;
    use seismograph_protocol::message::RecordingConfiguration;
    use seismograph_protocol::monitor::{AuthenticationToken, InstanceId, MonitorDescriptor};
    use seismograph_rallocator::callers::{AddressLookup, AddressLookupFields};

    use super::super::app::MonitorTab;
    use super::*;

    fn capture() -> Box<CapturedSnapshot> {
        let events = (1..=2)
            .map(|id| Event {
                thread_id: ThreadId::new(id),
                sequence: EventSequence::new(id),
                timestamp: EventTimestamp::from_ticks(id),
                kind: EventKind::ArcClone,
                payload: EventPayload::Object(ObjectId::new(id)),
                call_stack: vec![Address::new(id)],
            })
            .collect();
        let addresses = [(1, "app::work"), (2, "noise::work")]
            .into_iter()
            .map(|(address, symbol)| {
                AddressLookup::from_fields(AddressLookupFields {
                    address,
                    symbol: Some(symbol.into()),
                    filename: None,
                    line: None,
                    column: None,
                })
            })
            .collect();
        let index = Arc::new(FilterIndex::new(
            DecodedSnapshot {
                events: Events {
                    total_events: 2,
                    events,
                    ..Events::default()
                },
                ..DecodedSnapshot::default()
            },
            None,
            None,
            addresses,
            HashSet::new(),
        ));
        let mut capture = index.render(&FilterSpec::default());
        capture.captured_at = Some(SystemTime::UNIX_EPOCH);
        capture.captured_instant = Some(Instant::now());
        capture
    }

    fn offline() -> App {
        let mut app = App::offline("example.seismograph".into());
        app.finish_offline_load(capture());
        app
    }

    fn wait_for_filter(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.filters.receiver.is_some() {
            assert!(Instant::now() < deadline, "filter worker did not complete");
            app.poll_filter();
            thread::yield_now();
        }
    }

    fn include_app() -> FilterSpec {
        FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap()
    }

    #[test]
    fn editing_preserves_drafts_and_navigates_options() {
        let mut popup = FilterPopup::new(&FilterSpec::default());
        for character in "crate:app".chars() {
            popup.edit(KeyCode::Char(character));
        }
        popup.edit(KeyCode::Char('q'));
        popup.edit(KeyCode::Delete);
        popup.edit(KeyCode::Tab);
        popup.edit(KeyCode::Down);
        popup.edit(KeyCode::Right);
        popup.edit(KeyCode::Tab);
        popup.edit(KeyCode::Char(' '));
        assert_eq!(
            popup.parse().unwrap(),
            FilterSpec::parse("crate:app", "", true, RuntimeStackMode::Spawn).unwrap()
        );
    }

    #[test]
    fn visible_input_tail_keeps_the_end_cursor() {
        assert_eq!(visible_tail("crate:long_name|", 5), "name|");
        assert_eq!(visible_tail("anything|", 0), "");
    }

    #[test]
    fn no_snapshot_is_an_actionable_error() {
        let mut app = App::offline("example.seismograph".into());
        app.open_filter_popup();
        assert!(app.filters.popup.is_none());
        assert!(app.status.contains("No snapshot"));
    }

    #[test]
    fn modal_input_cannot_quit_and_cancel_preserves_the_applied_view() {
        let mut app = offline();
        let original_index = Arc::clone(app.filter_capture().unwrap().filter_index.as_ref().unwrap());
        assert!(!app.handle_key(KeyCode::Char('F')));
        assert!(!app.handle_key(KeyCode::Char('q')));
        assert_eq!(app.filters.popup.as_ref().unwrap().includes, "q");
        assert!(!app.handle_key(KeyCode::Enter));
        assert!(app.filters.popup.as_ref().unwrap().error.is_some());
        assert!(!app.handle_key(KeyCode::Esc));
        assert!(app.filters.popup.is_none());
        assert_eq!(app.filters.applied, FilterSpec::default());
        assert!(Arc::ptr_eq(
            app.filter_capture().unwrap().filter_index.as_ref().unwrap(),
            &original_index
        ));
        assert!(app.handle_key(KeyCode::Char('q')));
    }

    #[test]
    fn offline_apply_reset_preserves_capture_times_and_draft_text() {
        let mut app = offline();
        let original = app.filter_capture().unwrap();
        let times = (original.captured_at, original.captured_instant);
        app.handle_key(KeyCode::Char('F'));
        for character in "crate:app".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key(KeyCode::Enter);
        assert!(app.filters.popup.is_none());
        wait_for_filter(&mut app);
        let filtered = app.filter_capture().unwrap();
        assert_eq!(filtered.filter_summary.events.shown, 1);
        assert_eq!((filtered.captured_at, filtered.captured_instant), times);
        app.handle_key(KeyCode::Char('F'));
        assert_eq!(app.filters.popup.as_ref().unwrap().includes, "crate:app");
        for _ in 0.."crate:app".len() {
            app.handle_key(KeyCode::Backspace);
        }
        app.handle_key(KeyCode::Enter);
        wait_for_filter(&mut app);
        assert_eq!(app.filter_capture().unwrap().filter_summary.events.shown, 2);
        assert!(!app.filters.applied.is_active());
    }

    #[test]
    fn stale_generation_and_index_cannot_replace_the_current_capture() {
        let mut app = offline();
        let original_index = Arc::clone(app.filter_capture().unwrap().filter_index.as_ref().unwrap());
        let original_generation = app.filters.generation;
        app.finish_offline_load(capture());
        let current_index = Arc::clone(app.filter_capture().unwrap().filter_index.as_ref().unwrap());
        for (generation, index) in [
            (original_generation, Arc::clone(&current_index)),
            (app.filters.generation, original_index),
        ] {
            app.finish_filter(FilterCompletion {
                generation,
                snapshot: index.render(&include_app()),
                index,
                spec: include_app(),
            });
            assert_eq!(app.filter_capture().unwrap().filter_summary.events.shown, 2);
        }
        assert!(Arc::ptr_eq(
            app.filter_capture().unwrap().filter_index.as_ref().unwrap(),
            &current_index
        ));
    }

    #[test]
    fn disconnect_reconnect_invalidates_even_the_same_index() {
        let mut app = offline();
        let index = Arc::clone(app.filter_capture().unwrap().filter_index.as_ref().unwrap());
        let original_generation = app.filters.generation;
        let stale = FilterCompletion {
            generation: app.filters.generation,
            index: Arc::clone(&index),
            spec: include_app(),
            snapshot: index.render(&include_app()),
        };
        app.filters.invalidate();
        app.screen = Screen::Browse;
        app.finish_filter(stale);
        assert!(matches!(app.screen, Screen::Browse));
        assert_eq!(app.filters.applied, FilterSpec::default());
        app.screen = Screen::Offline {
            path: "reopened.seismograph".into(),
            tab: MonitorTab::Info,
            snapshot: Some(index.render(&FilterSpec::default())),
        };
        app.finish_filter(FilterCompletion {
            generation: original_generation,
            snapshot: index.render(&include_app()),
            index,
            spec: include_app(),
        });
        assert_eq!(app.filter_capture().unwrap().filter_summary.events.shown, 2);
    }

    #[test]
    fn new_snapshot_supersedes_pending_filter_and_live_captures_reapply_it() {
        let mut app = offline();
        app.start_filter(include_app()).unwrap();
        app.finish_offline_load(capture());
        let replacement = Arc::clone(app.filter_capture().unwrap().filter_index.as_ref().unwrap());
        wait_for_filter(&mut app);
        assert!(Arc::ptr_eq(
            app.filter_capture().unwrap().filter_index.as_ref().unwrap(),
            &replacement
        ));
        assert_eq!(app.filter_capture().unwrap().filter_summary.events.shown, 1);
        app.screen = Screen::Connected {
            descriptor: MonitorDescriptor {
                name: "test".into(),
                instance: None,
                process_id: 1,
                instance_id: InstanceId::from_bytes([1; 16]),
                port: 0,
                authentication: AuthenticationToken::from_bytes([1; 32]),
            },
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Threads,
            snapshot: Some(capture()),
        };
        app.filter_snapshot_arrived();
        wait_for_filter(&mut app);
        assert_eq!(app.filters.applied, include_app());
        assert_eq!(app.filter_capture().unwrap().filter_summary.events.shown, 1);
    }

    #[test]
    fn pending_reset_stays_reset_when_a_new_capture_arrives() {
        let mut app = offline();
        app.start_filter(include_app()).unwrap();
        wait_for_filter(&mut app);
        app.start_filter(FilterSpec::default()).unwrap();
        app.finish_offline_load(capture());
        assert!(!app.filters.applied.is_active());
        wait_for_filter(&mut app);
        assert_eq!(app.filter_capture().unwrap().filter_summary.events.shown, 2);
    }

    #[test]
    fn busy_filter_requests_coalesce_without_replacing_the_worker_slot() {
        let mut app = offline();
        let index = Arc::clone(app.filter_capture().unwrap().filter_index.as_ref().unwrap());
        let original_generation = app.filters.generation;
        let (sender, receiver) = unbounded();
        app.filters.receiver = Some(receiver);
        app.filters.started_at = Some(Instant::now());
        app.start_filter(include_app()).unwrap();
        app.start_filter(FilterSpec::default()).unwrap();
        let latest = FilterSpec::parse("crate:noise", "", false, RuntimeStackMode::Spawn).unwrap();
        app.start_filter(latest.clone()).unwrap();
        app.open_filter_popup();
        assert_eq!(app.filters.popup.as_ref().unwrap().parse().unwrap(), latest);
        app.handle_filter_key(KeyCode::Esc);
        assert!(app.filters.queued);
        assert_eq!(app.filters.pending.as_ref(), Some(&latest));
        assert!(matches!(app.filters.receiver.as_ref().unwrap().try_recv(), Err(error) if error.is_empty()));
        sender
            .send_sync(FilterCompletion {
                generation: original_generation,
                snapshot: index.render(&FilterSpec::default()),
                index,
                spec: FilterSpec::default(),
            })
            .unwrap_or_else(|_| panic!("queuing requests must retain the active worker receiver"));
        wait_for_filter(&mut app);
        assert_eq!(app.filters.applied, latest);
        assert_eq!(app.filter_capture().unwrap().filter_summary.events.shown, 1);
    }

    #[test]
    fn snapshot_and_connection_invalidation_retain_the_busy_worker_slot() {
        let mut app = offline();
        let index = Arc::clone(app.filter_capture().unwrap().filter_index.as_ref().unwrap());
        let original_generation = app.filters.generation;
        let (sender, receiver) = unbounded();
        app.filters.receiver = Some(receiver);
        app.filters.started_at = Some(Instant::now());
        app.start_filter(include_app()).unwrap();
        app.finish_offline_load(capture());
        assert!(app.filters.queued);
        app.filters.invalidate();
        app.screen = Screen::Browse;
        assert!(app.filters.receiver.is_some());
        assert!(!app.filters.queued);
        app.screen = Screen::Offline {
            path: "reopened.seismograph".into(),
            tab: MonitorTab::Info,
            snapshot: Some(capture()),
        };
        let current_index = Arc::clone(app.filter_capture().unwrap().filter_index.as_ref().unwrap());
        app.start_filter(include_app()).unwrap();
        sender
            .send_sync(FilterCompletion {
                generation: original_generation,
                snapshot: index.render(&FilterSpec::default()),
                index,
                spec: FilterSpec::default(),
            })
            .unwrap_or_else(|_| panic!("snapshot and connection changes must retain the active worker receiver"));
        wait_for_filter(&mut app);
        assert_eq!(app.filters.applied, include_app());
        assert!(Arc::ptr_eq(
            app.filter_capture().unwrap().filter_index.as_ref().unwrap(),
            &current_index
        ));
    }

    #[test]
    fn missing_index_cannot_silently_apply() {
        let mut app = offline();
        let Screen::Offline { snapshot, .. } = &mut app.screen else {
            unreachable!()
        };
        snapshot.as_mut().unwrap().filter_index = None;
        app.handle_key(KeyCode::Char('F'));
        assert!(app.filters.popup.is_none());
        assert!(app.status.contains("no filter index"));
        assert!(app.start_filter(include_app()).is_err());
    }

    #[test]
    fn filter_modal_blocks_an_already_started_panel_drag() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let mut app = offline();
        let Screen::Offline { tab, .. } = &mut app.screen else {
            unreachable!()
        };
        *tab = MonitorTab::Runtime;
        let area = Rect::new(0, 0, 120, 40);
        let content = Rect::new(0, 3, 120, 36);
        let before = app.panels.arrange(MonitorTab::Runtime, content).areas;
        let mouse = |kind, row| MouseEvent {
            kind,
            column: 5,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), before[1].y), area);
        app.handle_key(KeyCode::Char('F'));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 30), area);
        app.handle_key(KeyCode::Esc);
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 32), area);
        assert_eq!(app.panels.arrange(MonitorTab::Runtime, content).areas, before);
    }

    #[test]
    fn filter_popup_and_summary_are_visible_in_the_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = offline();
        app.start_filter(include_app()).unwrap();
        wait_for_filter(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("Events 1/2 (unknown 0)"));
        assert!(rendered.contains("Unfiltered: source accepted/overwritten counters, whole-process counters and heap topology."));
        app.handle_key(KeyCode::Char('F'));
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("Include: crate:app|"));
        assert!(rendered.contains("Esc cancel"));
    }
}
