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

fn block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
}

fn selected_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().bg(Color::DarkGray)
    }
}

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
    use super::*;

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
}
