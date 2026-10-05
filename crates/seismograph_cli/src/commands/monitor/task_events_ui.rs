// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Inferred task operations, their task-only occurrences, and the selected stack.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, TableState};

use super::super::app::{RuntimeFocus, RuntimeViewState, TaskEventsFocus, TaskEventsViewState};
use super::super::data::{AllocationStackFilter, RuntimeTaskSummary};
use super::super::mouse::{ListTarget, MouseRows};
use super::super::task_events::{TaskEvent, TaskEventSummary, TaskEventsSnapshot, TaskObject, TaskOperation};
use super::{draw_empty_panel_with_message, format_count};

#[cfg_attr(test, mutants::skip)] // Panel selection and clipping are terminal presentation glue.
pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    task: Option<&RuntimeTaskSummary>,
    snapshot: Option<&TaskEventsSnapshot>,
    runtime_view: RuntimeViewState,
) {
    if area.is_empty() {
        return;
    }
    let events = task.and_then(|task| snapshot?.tasks.get(&(task.runtime_id, task.task_id)));
    let [notice, content] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let caption = task.map_or_else(
        || "Select a task to browse inferred operations.".to_owned(),
        |task| {
            format!(
                "{}:task{} | {} inferred events across workers | capture: {} unassigned, {} ambiguous | e focus, F1 details",
                task.runtime_id,
                task.task_id,
                events.map_or(0, |events| events.inferred_events),
                snapshot.map_or(0, |snapshot| snapshot.unassigned_events),
                snapshot.map_or(0, |snapshot| snapshot.ambiguous_events),
            )
        },
    );
    frame.render_widget(Paragraph::new(caption), notice);
    let view = runtime_view.events;
    let operation = events.and_then(|events| {
        events
            .operations
            .get(view.operation_selected.min(events.operations.len().saturating_sub(1)))
    });
    let occurrence =
        operation.and_then(|operation| operation.occurrence(view.event_selected.min(operation.occurrences().len().saturating_sub(1))));
    let [operations, occurrences, stack] = areas(content, view.focus);
    let focused = runtime_view.focus == RuntimeFocus::Events;
    draw_operations(frame, mouse, operations, events, view, focused);
    draw_occurrences(frame, mouse, occurrences, operation, view, focused);
    draw_stack(frame, stack, occurrence, view);
}

#[cfg_attr(test, mutants::skip)] // Responsive panel splits are terminal layout policy.
fn areas(area: Rect, focus: TaskEventsFocus) -> [Rect; 3] {
    if area.width < 110 {
        return match focus {
            TaskEventsFocus::Operations => [area, Rect::default(), Rect::default()],
            TaskEventsFocus::Occurrences => {
                let [events, stack] = Layout::vertical([Constraint::Percentage(40), Constraint::Min(0)]).areas(area);
                [Rect::default(), events, stack]
            }
        };
    }
    Layout::horizontal([Constraint::Length(32), Constraint::Percentage(40), Constraint::Min(0)]).areas(area)
}

#[cfg_attr(test, mutants::skip)]
fn block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
}

#[cfg_attr(test, mutants::skip)]
fn selected_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().bg(Color::DarkGray)
    }
}

#[cfg_attr(test, mutants::skip)] // Table paging and highlighting are terminal presentation details.
fn draw_operations(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    task: Option<&TaskEventSummary>,
    view: TaskEventsViewState,
    focused: bool,
) {
    if area.is_empty() {
        return;
    }
    let Some(task) = task.filter(|task| !task.operations.is_empty()) else {
        draw_empty_panel_with_message(
            frame,
            area,
            " Operations ",
            Some("No attributable retained operations. Poll evidence may be missing or sampled."),
        );
        return;
    };
    let selected = view.operation_selected.min(task.operations.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let rows = task.operations.iter().skip(first).take(visible).map(|operation| {
        let row = Row::new([operation.kind.label().to_owned(), format_count(operation.events)]);
        if operation.kind.is_contention() {
            row.style(Style::default().fg(Color::Yellow))
        } else {
            row
        }
    });
    let table = Table::new(rows, [Constraint::Min(12), Constraint::Length(6)])
        .header(Row::new(["Operation", "Events"]).style(Style::default().add_modifier(Modifier::BOLD)))
        .row_highlight_style(selected_style(focused && view.focus == TaskEventsFocus::Operations))
        .block(block(" Operations ", focused && view.focus == TaskEventsFocus::Operations));
    let mut state = TableState::default().with_selected(Some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
    mouse.register(
        area,
        1,
        first.saturating_add(state.offset()),
        task.operations.len(),
        ListTarget::TaskOperations,
    );
}

#[cfg_attr(test, mutants::skip)] // Table paging and highlighting are terminal presentation details.
fn draw_occurrences(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    operation: Option<&TaskOperation>,
    view: TaskEventsViewState,
    focused: bool,
) {
    if area.is_empty() {
        return;
    }
    let Some(operation) = operation else {
        draw_empty_panel_with_message(frame, area, " Task occurrences ", Some("Select an operation."));
        return;
    };
    let count = operation.occurrences().len();
    let selected = view.event_selected.min(count.saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let rows = operation.occurrences().skip(first).take(visible).map(|(object, event)| {
        Row::new([
            event.timestamp.to_string(),
            event.thread_id.to_string(),
            event.sequence.to_string(),
            format!("0x{:x}", object.object_id),
        ])
    });
    let title = format!(" Task occurrences | {} ", operation.kind.label());
    let table = Table::new(
        rows,
        [
            Constraint::Min(12),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(18),
        ],
    )
    .header(Row::new(["Time (ns)", "Thread", "Sequence", "Object"]).style(Style::default().add_modifier(Modifier::BOLD)))
    .row_highlight_style(selected_style(focused && view.focus == TaskEventsFocus::Occurrences))
    .block(block(&title, focused && view.focus == TaskEventsFocus::Occurrences));
    let mut state = TableState::default().with_selected((count > 0).then_some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
    mouse.register(area, 1, first.saturating_add(state.offset()), count, ListTarget::TaskOccurrences);
}

#[cfg_attr(test, mutants::skip)]
fn draw_stack(frame: &mut ratatui::Frame<'_>, area: Rect, occurrence: Option<(&TaskObject, &TaskEvent)>, view: TaskEventsViewState) {
    if area.is_empty() {
        return;
    }
    let Some((object, event)) = occurrence else {
        draw_empty_panel_with_message(frame, area, " Event stack ", Some("Select a retained occurrence."));
        return;
    };
    let mode = match view.stack_filter {
        AllocationStackFilter::All => "Complete captured stack",
        AllocationStackFilter::Application if event.relative_stack_known => "Task-relative application stack",
        AllocationStackFilter::Application => "Application stack; task boundary unknown (untrimmed)",
    };
    let frames = event.stack(view.stack_filter);
    let mut lines = vec![
        Line::from(format!(
            "Thread {} / sequence {} / {}ns",
            event.thread_id, event.sequence, event.timestamp
        )),
        Line::from(format!("{:?} | {}", event.kind, event.detail)),
        Line::from(object.identity_note),
        Line::from(mode),
    ];
    if frames.is_empty() {
        lines.push(Line::from("Backtrace unavailable for this event."));
    } else {
        lines.extend(
            frames
                .iter()
                .enumerate()
                .map(|(index, frame)| Line::from(format!("{index:>3}  {frame}"))),
        );
    }

    let inner = Block::default().borders(Borders::ALL).inner(area);
    let maximum = lines.len().saturating_sub(usize::from(inner.height));
    let horizontal_maximum = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .saturating_sub(usize::from(inner.width));
    let horizontal = view
        .stack_horizontal_scroll
        .min(u16::try_from(horizontal_maximum).unwrap_or(u16::MAX));
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(view.stack_scroll.min(maximum))
                .take(usize::from(inner.height))
                .collect::<Vec<_>>(),
        )
        .scroll((0, horizontal))
        .block(block(" Event stack | f relative/all ", false).title_bottom(" PgUp/Dn scroll | Left/Right pan ")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use seismograph::recorder::event::{Event, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events, ObjectId};
    use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, WorkerId};
    use seismograph::recorder::thread::{ThreadId, ThreadLog};
    use seismograph::recorder::{RecordingPolicies, RecordingPolicy};

    use super::*;

    fn event(sequence: u64, kind: EventKind, payload: EventPayload) -> Event {
        Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence * 10),
            kind,
            payload,
            call_stack: Vec::new(),
        }
    }

    fn task_event(sequence: u64, kind: EventKind, duration: u64) -> Event {
        event(
            sequence,
            kind,
            EventPayload::Runtime(RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: WorkerId::from_raw(1),
                subject_id: 1,
                related_id: 0,
                value_0: duration,
                value_1: 0,
            }),
        )
    }

    #[test]
    fn wide_event_columns_stay_side_by_side_even_in_short_rows() {
        let area = Rect::new(2, 30, 180, 10);
        for focus in [TaskEventsFocus::Operations, TaskEventsFocus::Occurrences] {
            let [operations, occurrences, stack] = areas(area, focus);
            assert_eq!((operations.y, occurrences.y, stack.y), (area.y, area.y, area.y));
            assert_eq!((operations.height, occurrences.height, stack.height), (10, 10, 10));
            assert_eq!(
                (operations.right(), occurrences.right(), stack.right()),
                (occurrences.x, stack.x, area.right())
            );
        }
    }

    #[test]
    fn compact_event_focus_uses_the_available_space_without_hidden_targets() {
        for width in [0, 1, 40, 80, 109] {
            let area = Rect::new(0, 0, width, 20);
            assert_eq!(areas(area, TaskEventsFocus::Operations), [area, Rect::default(), Rect::default()]);
            let [operations, occurrences, stack] = areas(area, TaskEventsFocus::Occurrences);
            assert_eq!(operations, Rect::default());
            assert_eq!((occurrences.width, stack.width), (area.width, area.width));
            assert_eq!((occurrences.bottom(), stack.bottom()), (stack.y, area.bottom()));
        }
    }

    #[test]
    fn stack_panel_explains_unknown_boundaries_and_missing_backtraces() {
        let events = Events {
            clock: EventClock::ProcessMonotonic,
            total_events: 3,
            recording: RecordingPolicies {
                runtime_tasks: RecordingPolicy::all(true),
                general_events: RecordingPolicy::all(true),
                ..RecordingPolicies::default()
            },
            threads: vec![ThreadLog {
                thread_id: ThreadId::new(1),
                total_events: 3,
                ..ThreadLog::default()
            }],
            events: vec![
                task_event(1, EventKind::TaskPollStarted, 0),
                event(2, EventKind::MutexAccess, EventPayload::Object(ObjectId::new(7))),
                task_event(3, EventKind::TaskPollFinished, 20),
            ],
            ..Events::default()
        };
        let mut snapshot = TaskEventsSnapshot::from_events(&events, &[], None);
        let operation = &mut snapshot.tasks.get_mut(&(1, 1)).unwrap().operations[0];
        let object = &mut operation.objects[0];
        Arc::make_mut(&mut object.history)[0].relative_stack_known = false;
        let occurrence = Some((&*object, &object.history[0]));
        let mut view = super::super::super::app::App::new().runtime_view.events;
        view.stack_filter = AllocationStackFilter::Application;
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();

        terminal.draw(|frame| draw_stack(frame, frame.area(), occurrence, view)).unwrap();

        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(text.contains("boundary unknown"));
        assert!(text.contains("Backtrace unavailable"));
    }
}
