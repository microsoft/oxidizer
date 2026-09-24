// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;

use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::app::{App, CacheFocus, HeapFocus, IoFocus, MonitorTab, PrimitiveFocus, RuntimeDetailView, RuntimeFocus, Screen, ThreadFocus};
use super::help_content::Section;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Context {
    Browser,
    Info,
    HeapBuckets,
    HeapHotspots,
    Allocations,
    PrimitiveTypes,
    PrimitiveOperations,
    PrimitiveHotspots,
    Threads,
    ThreadOperations,
    ThreadParticipants,
    ThreadObjects,
    RuntimeWorkers,
    RuntimeTasks,
    RuntimeDetails,
    RuntimeSpawnStack,
    IoResources,
    IoOperations,
    CacheTiers,
    CacheOperations,
    Recording,
    Filters,
    Capture,
    Loading,
    Error,
}

pub(super) struct Help {
    context: Context,
    document: Vec<&'static Section>,
    scroll: Cell<usize>,
    page: Cell<usize>,
    maximum: Cell<usize>,
}

impl App {
    pub(super) fn handle_help_key(&mut self, code: KeyCode) -> bool {
        if let Some(help) = &self.help {
            if matches!(code, KeyCode::F(1) | KeyCode::Esc | KeyCode::Char('q' | 'Q')) {
                self.help = None;
            } else {
                help.scroll_key(code);
            }
            return true;
        }
        if code != KeyCode::F(1) {
            return false;
        }
        let context = self.help_context();
        self.help = Some(Help::new(context, matches!(self.screen, Screen::Offline { .. })));
        self.panels.cancel_drag();
        true
    }

    fn help_context(&self) -> Context {
        if self.filters.popup.is_some() {
            Context::Filters
        } else if self.recording_configuration_popup.is_some() {
            Context::Recording
        } else if self.capture_started_at.is_some() {
            Context::Capture
        } else if self.snapshot_error.is_some() {
            Context::Error
        } else if matches!(self.screen, Screen::Offline { snapshot: None, .. }) {
            Context::Loading
        } else {
            match self.screen {
                Screen::Browse => Context::Browser,
                Screen::Connected { tab, .. } | Screen::Offline { tab, .. } => self.panel_help_context(tab),
            }
        }
    }

    fn panel_help_context(&self, tab: MonitorTab) -> Context {
        match tab {
            MonitorTab::Info => Context::Info,
            MonitorTab::Heaps => match self.heap_view.focus {
                HeapFocus::Buckets => Context::HeapBuckets,
                HeapFocus::Hotspots => Context::HeapHotspots,
            },
            MonitorTab::Allocations => Context::Allocations,
            MonitorTab::Primitives => match self.primitive_view.focus {
                PrimitiveFocus::Types => Context::PrimitiveTypes,
                PrimitiveFocus::Operations => Context::PrimitiveOperations,
                PrimitiveFocus::Hotspots => Context::PrimitiveHotspots,
            },
            MonitorTab::Threads => match self.thread_view.focus {
                ThreadFocus::Threads => Context::Threads,
                ThreadFocus::Operations => Context::ThreadOperations,
                ThreadFocus::Participants => Context::ThreadParticipants,
                ThreadFocus::Objects => Context::ThreadObjects,
            },
            MonitorTab::Runtime => match self.runtime_view.focus {
                RuntimeFocus::Workers => Context::RuntimeWorkers,
                RuntimeFocus::Tasks => Context::RuntimeTasks,
                RuntimeFocus::Details => match self.runtime_view.detail_view {
                    RuntimeDetailView::Details => Context::RuntimeDetails,
                    RuntimeDetailView::SpawnStack => Context::RuntimeSpawnStack,
                },
            },
            MonitorTab::Io => match self.io_view.focus {
                IoFocus::Resources => Context::IoResources,
                IoFocus::Operations => Context::IoOperations,
            },
            MonitorTab::Cache => match self.cache_view.focus {
                CacheFocus::Tiers => Context::CacheTiers,
                CacheFocus::Operations => Context::CacheOperations,
            },
        }
    }
}

impl Help {
    fn new(context: Context, offline: bool) -> Self {
        Self {
            context,
            document: super::help_content::document(context, offline),
            scroll: Cell::new(0),
            page: Cell::new(1),
            maximum: Cell::new(0),
        }
    }

    fn scroll_key(&self, code: KeyCode) {
        let current = self.scroll.get();
        let next = match code {
            KeyCode::Up => current.saturating_sub(1),
            KeyCode::Down => current.saturating_add(1),
            KeyCode::PageUp => current.saturating_sub(self.page.get()),
            KeyCode::PageDown => current.saturating_add(self.page.get()),
            KeyCode::Home => 0,
            KeyCode::End => self.maximum.get(),
            _ => current,
        };
        self.scroll.set(next.min(self.maximum.get()));
    }

    pub(super) fn handle_mouse(&self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::ScrollUp => self.scroll_key(KeyCode::Up),
            MouseEventKind::ScrollDown => self.scroll_key(KeyCode::Down),
            _ => {}
        }
    }

    pub(super) fn draw(&self, frame: &mut ratatui::Frame<'_>) {
        let screen = frame.area();
        let width = screen.width.min(110);
        let height = screen.height.saturating_sub(2).max(1).min(screen.height);
        let area = Rect::new(
            screen.x + (screen.width - width) / 2,
            screen.y + (screen.height - height) / 2,
            width,
            height,
        );
        let block = Block::default()
            .title(format!(" F1 Help: {} ", super::help_content::title(self.context)))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        // Tiny terminals use every cell for text rather than losing it to borders.
        let inner = if area.width < 3 || area.height < 3 {
            area
        } else {
            block.inner(area)
        };
        let lines = styled_lines(&self.document, usize::from(inner.width.max(1)));
        let page = usize::from(inner.height.max(1));
        let maximum = lines.len().saturating_sub(page);
        self.page.set(page);
        self.maximum.set(maximum);
        let scroll = self.scroll.get().min(maximum);
        self.scroll.set(scroll);
        frame.render_widget(Clear, area);
        frame.render_widget(
            block.title_bottom(format!(
                " F1/Esc/q close | Up/Down PgUp/PgDn Home/End | {}-{} / {} ",
                scroll + 1,
                (scroll + page).min(lines.len()),
                lines.len(),
            )),
            area,
        );
        frame.render_widget(Paragraph::new(lines.into_iter().skip(scroll).take(page).collect::<Vec<_>>()), inner);
    }
}

fn styled_lines(document: &[&Section], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    // Align explanations and continuation lines under the keyword, after its bullet.
    let indent = 4.min(width.saturating_sub(1));
    let padding = " ".repeat(indent);
    let bullet = if indent >= 2 {
        format!("{}* ", " ".repeat(indent - 2))
    } else {
        padding.clone()
    };
    let heading_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let keyword_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    for section in document {
        if !lines.is_empty() {
            lines.push(Line::default());
            lines.push(Line::default());
        }
        lines.extend(
            wrap(section.heading, width)
                .into_iter()
                .map(|line| Line::from(Span::styled(line, heading_style))),
        );
        lines.push(Line::default());
        for (entry_index, entry) in section.entries.iter().enumerate() {
            if entry_index != 0 {
                lines.push(Line::default());
            }
            for (index, keyword) in wrap(entry.keyword, width - indent).into_iter().enumerate() {
                lines.push(Line::from(vec![
                    Span::raw(if index == 0 { bullet.clone() } else { padding.clone() }),
                    Span::styled(keyword, keyword_style),
                ]));
            }
            lines.extend(
                wrap(entry.explanation, width - indent)
                    .into_iter()
                    .map(|explanation| Line::from(vec![Span::raw(padding.clone()), Span::raw(explanation)])),
            );
        }
    }
    lines
}

/// Wrap the static ASCII help once per draw, then page the actual visual lines.
/// This avoids a u16 paragraph scroll limit and keeps End accurate after resize.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.len() + 1 + word.len() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            for character in word.chars() {
                if line.len() == width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(character);
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Instant;

    use crossterm::event::{KeyEvent, KeyModifiers, MouseButton};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use seismograph_protocol::message::RecordingConfiguration;

    use super::super::app::{CaptureStep, RecordingConfigurationPopup};
    use super::super::filter::FilterSpec;
    use super::super::filter_index::FilterIndex;
    use super::*;

    fn snapshot() -> Box<super::super::data::CapturedSnapshot> {
        Arc::new(FilterIndex::new(
            seismograph::snapshot::DecodedSnapshot::default(),
            None,
            None,
            Vec::new(),
            HashSet::new(),
        ))
        .render(&FilterSpec::default())
    }

    fn offline() -> App {
        let mut app = App::offline("help-test.seismograph".into());
        app.finish_offline_load(snapshot());
        app
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn every_panel_focus_maps_to_its_own_help() {
        let mut app = offline();
        assert_eq!(app.panel_help_context(MonitorTab::Info), Context::Info);
        assert_eq!(app.panel_help_context(MonitorTab::Allocations), Context::Allocations);
        for (focus, expected) in [
            (HeapFocus::Buckets, Context::HeapBuckets),
            (HeapFocus::Hotspots, Context::HeapHotspots),
        ] {
            app.heap_view.focus = focus;
            assert_eq!(app.panel_help_context(MonitorTab::Heaps), expected);
        }
        for (focus, expected) in [
            (PrimitiveFocus::Types, Context::PrimitiveTypes),
            (PrimitiveFocus::Operations, Context::PrimitiveOperations),
            (PrimitiveFocus::Hotspots, Context::PrimitiveHotspots),
        ] {
            app.primitive_view.focus = focus;
            assert_eq!(app.panel_help_context(MonitorTab::Primitives), expected);
        }
        for (focus, expected) in [
            (ThreadFocus::Threads, Context::Threads),
            (ThreadFocus::Operations, Context::ThreadOperations),
            (ThreadFocus::Participants, Context::ThreadParticipants),
            (ThreadFocus::Objects, Context::ThreadObjects),
        ] {
            app.thread_view.focus = focus;
            assert_eq!(app.panel_help_context(MonitorTab::Threads), expected);
        }
        for (focus, detail, expected) in [
            (RuntimeFocus::Workers, RuntimeDetailView::Details, Context::RuntimeWorkers),
            (RuntimeFocus::Tasks, RuntimeDetailView::Details, Context::RuntimeTasks),
            (RuntimeFocus::Details, RuntimeDetailView::Details, Context::RuntimeDetails),
            (RuntimeFocus::Details, RuntimeDetailView::SpawnStack, Context::RuntimeSpawnStack),
        ] {
            app.runtime_view.focus = focus;
            app.runtime_view.detail_view = detail;
            assert_eq!(app.panel_help_context(MonitorTab::Runtime), expected);
        }
        for (focus, expected) in [
            (IoFocus::Resources, Context::IoResources),
            (IoFocus::Operations, Context::IoOperations),
        ] {
            app.io_view.focus = focus;
            assert_eq!(app.panel_help_context(MonitorTab::Io), expected);
        }
        for (focus, expected) in [
            (CacheFocus::Tiers, Context::CacheTiers),
            (CacheFocus::Operations, Context::CacheOperations),
        ] {
            app.cache_view.focus = focus;
            assert_eq!(app.panel_help_context(MonitorTab::Cache), expected);
        }
    }

    #[test]
    fn dialogs_and_transient_states_take_precedence_over_panel_help() {
        let mut app = App::new();
        assert_eq!(app.help_context(), Context::Browser);
        app = App::offline("snapshot.seismograph".into());
        assert_eq!(app.help_context(), Context::Loading);
        app.snapshot_error = Some("failed".into());
        assert_eq!(app.help_context(), Context::Error);
        app.capture_started_at = Some(Instant::now());
        assert_eq!(app.help_context(), Context::Capture);
        app.recording_configuration_popup = Some(RecordingConfigurationPopup::new(RecordingConfiguration::default()));
        assert_eq!(app.help_context(), Context::Recording);
        app.finish_offline_load(snapshot());
        app.open_filter_popup();
        assert_eq!(app.help_context(), Context::Filters);
    }

    #[test]
    fn modal_keys_preserve_selections_and_recording_draft() {
        let mut app = offline();
        let mut popup = RecordingConfigurationPopup::new(RecordingConfiguration::default());
        popup.selected = 2;
        popup.draft.allocations.capture_backtraces = true;
        app.recording_configuration_popup = Some(popup);
        let selections = (
            app.heap_view,
            app.allocation_view,
            app.primitive_view,
            app.thread_view,
            app.runtime_view,
            app.io_view,
            app.cache_view,
        );
        app.handle_key(KeyCode::F(1));
        for code in [
            KeyCode::Char('s'),
            KeyCode::Char('c'),
            KeyCode::Char('d'),
            KeyCode::Char('F'),
            KeyCode::Char('f'),
            KeyCode::Char('r'),
            KeyCode::Char('8'),
            KeyCode::Char(']'),
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Backspace,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
        ] {
            assert!(!app.handle_key(code));
        }
        assert!(!super::super::should_exit(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
        assert_eq!(app.recording_configuration_popup, Some(popup));
        assert_eq!(
            selections,
            (
                app.heap_view,
                app.allocation_view,
                app.primitive_view,
                app.thread_view,
                app.runtime_view,
                app.io_view,
                app.cache_view
            )
        );
        for close in [KeyCode::Esc, KeyCode::F(1), KeyCode::Char('q'), KeyCode::Char('Q')] {
            if app.help.is_none() {
                app.handle_key(KeyCode::F(1));
            }
            assert!(!app.handle_key(close));
            assert!(app.help.is_none());
            assert_eq!(app.recording_configuration_popup, Some(popup));
        }
    }

    #[test]
    fn filter_draft_and_validation_error_survive_help() {
        let mut app = offline();
        app.open_filter_popup();
        for character in "invalid-rule".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key(KeyCode::Enter);
        let before = format!("{:?}", app.filters.popup);
        app.handle_key(KeyCode::F(1));
        assert_eq!(app.help.as_ref().unwrap().context, Context::Filters);
        app.handle_key(KeyCode::Char('x'));
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Backspace);
        app.handle_key(KeyCode::Tab);
        app.handle_key(KeyCode::Char('q'));
        assert_eq!(format!("{:?}", app.filters.popup), before);
        app.handle_key(KeyCode::Esc);
        assert!(app.filters.popup.is_none());
    }

    #[test]
    fn background_completion_does_not_replace_open_help() {
        let mut app = App::offline("snapshot.seismograph".into());
        app.handle_key(KeyCode::F(1));
        let original = app.help.as_ref().unwrap().document.clone();
        app.finish_offline_load(snapshot());
        assert_eq!(app.help.as_ref().unwrap().document, original);
        assert_eq!(app.help.as_ref().unwrap().context, Context::Loading);
        app.handle_key(KeyCode::Esc);
        app.capture_started_at = Some(Instant::now());
        app.capture_step = Some(CaptureStep::Save);
        app.handle_key(KeyCode::F(1));
        app.capture_started_at = None;
        app.capture_step = None;
        app.finish_offline_load(snapshot());
        assert_eq!(app.help.as_ref().unwrap().context, Context::Capture);
    }

    #[test]
    fn filter_completion_can_reset_focus_without_changing_help_topic() {
        let mut app = offline();
        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Runtime;
        }
        app.runtime_view.focus = RuntimeFocus::Details;
        app.runtime_view.detail_view = RuntimeDetailView::SpawnStack;
        app.open_filter_popup();
        for character in "crate:example".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::F(1));
        let before = app.help.as_ref().unwrap().document.clone();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while !app.filters.applied.is_active() {
            assert!(Instant::now() < deadline, "filter completion did not arrive");
            app.poll_filter();
            std::thread::yield_now();
        }
        assert_eq!(app.help.as_ref().unwrap().context, Context::RuntimeSpawnStack);
        assert_eq!(app.help.as_ref().unwrap().document, before);
    }

    #[test]
    fn help_blocks_background_clicks_and_resizing_but_accepts_wheel() {
        let mut app = offline();
        let area = Rect::new(0, 0, 100, 30);
        let before = app.panels.arrange(MonitorTab::Runtime, area).areas;
        app.handle_key(KeyCode::F(1));
        render(&app, 100, 30);
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(
                MouseEvent {
                    kind,
                    column: 60,
                    row: 1,
                    modifiers: KeyModifiers::NONE,
                },
                area,
            );
        }
        assert!(matches!(app.screen, Screen::Offline { tab: MonitorTab::Info, .. }));
        assert_eq!(app.panels.arrange(MonitorTab::Runtime, area).areas, before);
        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 60,
                row: 1,
                modifiers: KeyModifiers::NONE,
            },
            area,
        );
        assert_eq!(app.help.as_ref().unwrap().scroll.get(), 1);
    }

    #[test]
    fn scrolling_uses_wrapped_lines_and_clamps_after_resize() {
        let mut app = offline();
        app.help = Some(Help::new(Context::HeapBuckets, true));
        render(&app, 32, 12);
        let page = app.help.as_ref().unwrap().page.get();
        app.handle_key(KeyCode::PageDown);
        assert_eq!(app.help.as_ref().unwrap().scroll.get(), page);
        app.handle_key(KeyCode::Up);
        assert_eq!(app.help.as_ref().unwrap().scroll.get(), page - 1);
        app.handle_key(KeyCode::PageUp);
        assert_eq!(app.help.as_ref().unwrap().scroll.get(), 0);
        app.handle_key(KeyCode::End);
        assert_eq!(app.help.as_ref().unwrap().scroll.get(), app.help.as_ref().unwrap().maximum.get());
        render(&app, 110, 50);
        assert_eq!(app.help.as_ref().unwrap().scroll.get(), app.help.as_ref().unwrap().maximum.get());
        assert!(render(&app, 110, 50).contains("offline file."));
        app.handle_key(KeyCode::Home);
        assert_eq!(app.help.as_ref().unwrap().scroll.get(), 0);
    }

    #[test]
    fn tiny_terminals_render_safely_with_help_above_other_dialogs() {
        let mut app = offline();
        app.open_filter_popup();
        app.recording_configuration_popup = Some(RecordingConfigurationPopup::new(RecordingConfiguration::default()));
        app.capture_started_at = Some(Instant::now());
        app.capture_step = Some(CaptureStep::Decode);
        app.handle_key(KeyCode::F(1));
        for (width, height) in [(0, 0), (1, 1), (2, 2), (3, 3), (12, 5), (80, 24)] {
            render(&app, width, height);
            app.handle_key(KeyCode::End);
            render(&app, width, height);
            app.handle_key(KeyCode::Home);
        }
        assert!(render(&app, 100, 30).contains("F1 Help: Stack filters"));
    }

    #[test]
    fn wrapping_preserves_every_word_even_at_one_column() {
        let text = "A long::unbreakable::symbol\n\nlast line";
        for width in [1, 2, 8, 20, 100] {
            let lines = wrap(text, width);
            assert!(lines.iter().all(|line| line.len() <= width));
            assert_eq!(lines.concat().replace(' ', ""), text.replace([' ', '\n'], ""));
        }
    }

    #[test]
    fn rendered_help_has_bold_headers_and_cyan_bullet_keywords() {
        let mut app = App::new();
        app.handle_key(KeyCode::F(1));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        for x in 1..=12 {
            let cell = &buffer[(x, 2)];
            assert_eq!(cell.fg, Color::Yellow);
            assert!(cell.modifier.contains(Modifier::BOLD));
        }
        assert_eq!(buffer[(3, 4)].symbol(), "*");
        for x in 5..27 {
            let cell = &buffer[(x, 4)];
            assert_eq!(cell.fg, Color::Cyan);
            assert!(cell.modifier.contains(Modifier::BOLD));
        }
        assert_eq!(buffer[(5, 5)].symbol(), "D");
        assert!(!buffer[(5, 5)].modifier.contains(Modifier::BOLD));
        assert_eq!((1..5).map(|x| buffer[(x, 5)].symbol()).collect::<String>(), "    ");
    }

    #[test]
    fn explanation_wraps_with_hanging_indent_and_preserves_rule_syntax() {
        let section = Section {
            heading: "FILTER RULE",
            entries: &[super::super::help_content::Entry {
                keyword: "Include",
                explanation: "Match function:crate::module::name then preserve every continuation word.",
            }],
        };
        let lines = styled_lines(&[&section], 24);
        assert_eq!(
            lines[0].spans[0].style,
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        );
        assert_eq!(
            lines[2].spans[1].style,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        );
        assert_eq!(lines[2].to_string(), "  * Include");
        let explanations = &lines[3..];
        assert!(explanations.len() > 2);
        for line in explanations {
            assert_eq!(line.spans[0].content, "    ");
            assert_eq!(line.spans[1].style, Style::default());
            assert!(line.width() <= 24);
        }
        assert_eq!(
            explanations
                .iter()
                .map(|line| line.spans[1].content.as_ref())
                .collect::<String>()
                .replace(' ', ""),
            section.entries[0].explanation.replace(' ', ""),
        );
    }

    #[test]
    fn styled_layout_preserves_all_content_at_tiny_widths_and_separates_sections() {
        let sections = super::super::help_content::document(Context::RuntimeTasks, false);
        for width in [1, 2, 3, 4, 5, 12, 80] {
            let lines = styled_lines(&sections, width);
            assert!(lines.iter().all(|line| line.width() <= width));
            let without_layout = lines
                .iter()
                .flat_map(|line| line.spans.iter().filter(|span| span.content.as_ref().trim() != "*"))
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .replace(' ', "");
            let expected = sections
                .iter()
                .flat_map(|section| {
                    std::iter::once(section.heading).chain(section.entries.iter().flat_map(|entry| [entry.keyword, entry.explanation]))
                })
                .collect::<String>()
                .replace(' ', "");
            assert_eq!(without_layout, expected);
        }
        let lines = styled_lines(&sections, 80);
        let details_header = lines.iter().position(|line| line.to_string() == "TASK DETAILS").unwrap();
        assert!(lines[details_header - 1].spans.is_empty());
        assert!(lines[details_header - 2].spans.is_empty());
    }
}
