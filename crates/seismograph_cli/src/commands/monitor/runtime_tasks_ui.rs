// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Descriptor-level asynchronous and blocking task summaries.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use super::super::app::{RuntimeTasksFocus, RuntimeTasksViewState};
use super::super::data::{AsynchronousTaskGroupSummary, BlockingTaskGroupSummary, RuntimeMonitorSnapshot};
use super::super::mouse::{ListTarget, MouseRows};
use super::{draw_empty_panel_with_message, format_count, format_runtime_duration};

pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    [asynchronous, blocking]: [Rect; 2],
    runtime: Option<&RuntimeMonitorSnapshot>,
    unavailable: Option<&str>,
    view: RuntimeTasksViewState,
) {
    let Some(runtime) = runtime else {
        draw_empty_panel_with_message(frame, asynchronous, " Async Task Types ", unavailable);
        draw_empty_panel_with_message(frame, blocking, " Blocking Task Types ", unavailable);
        return;
    };
    draw_asynchronous(frame, mouse, asynchronous, runtime, view);
    draw_blocking(frame, mouse, blocking, runtime, view);
}

fn block(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
}

fn selection(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().bg(Color::DarkGray)
    }
}

fn descriptor(id: Option<u64>) -> String {
    id.map_or_else(|| "unknown".into(), |id| format!("#{id}"))
}

fn duration(value: Option<u64>) -> String {
    value.map_or_else(|| "-".into(), format_runtime_duration)
}

fn draw_empty(frame: &mut ratatui::Frame<'_>, area: Rect, title: String, message: &'static str, focused: bool) {
    frame.render_widget(
        Paragraph::new(message)
            .style(Style::default().fg(Color::DarkGray))
            .block(block(title, focused)),
        area,
    );
}

fn outcome_totals(groups: impl Iterator<Item = (u64, u64, u64, u64, u64)>) -> (u64, u64, u64, u64, u64) {
    groups.fold((0, 0, 0, 0, 0), |total, group| {
        (
            total.0.saturating_add(group.0),
            total.1.saturating_add(group.1),
            total.2.saturating_add(group.2),
            total.3.saturating_add(group.3),
            total.4.saturating_add(group.4),
        )
    })
}

fn draw_asynchronous(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    runtime: &RuntimeMonitorSnapshot,
    view: RuntimeTasksViewState,
) {
    if area.is_empty() {
        return;
    }
    let focused = view.focus == RuntimeTasksFocus::Asynchronous;
    let selected = view.asynchronous_selected.min(runtime.asynchronous_groups.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let (tasks, active, completed, canceled, panicked) = outcome_totals(runtime.asynchronous_groups.iter().map(|group| {
        (
            group.outcomes.tasks,
            group.outcomes.active,
            group.outcomes.completed,
            group.outcomes.canceled,
            group.outcomes.panicked,
        )
    }));
    let polls = runtime
        .asynchronous_groups
        .iter()
        .fold(0_u64, |total, group| total.saturating_add(group.polls));
    let title = format!(
        " Async Task Types · {} groups · {} tasks · {} active · {} done · {} canceled · {} panicked · {} polls · a focus ",
        runtime.asynchronous_groups.len(),
        format_count(tasks),
        format_count(active),
        format_count(completed),
        format_count(canceled),
        format_count(panicked),
        format_count(polls),
    );
    if runtime.asynchronous_groups.is_empty() {
        draw_empty(
            frame,
            area,
            title,
            "No asynchronous task polls are retained in this capture.",
            focused,
        );
        return;
    }
    let wide = area.width >= 125;
    let rows = runtime
        .asynchronous_groups
        .iter()
        .skip(first)
        .take(visible)
        .map(|group| asynchronous_row(group, wide));
    let (headers, widths): (Vec<_>, Vec<_>) = if wide {
        (
            vec![
                "Descriptor",
                "Tasks",
                "Active",
                "Done",
                "Canceled",
                "Panicked",
                "Workers",
                "Polls",
                "Avg poll",
                "Max poll",
            ],
            vec![14, 8, 8, 8, 9, 9, 8, 10, 12, 12].into_iter().map(Constraint::Length).collect(),
        )
    } else {
        (
            vec!["Descriptor", "Tasks", "Active", "Canceled", "Polls", "Max poll"],
            vec![14, 8, 8, 9, 10, 12].into_iter().map(Constraint::Length).collect(),
        )
    };
    let table = Table::new(rows, widths)
        .header(Row::new(headers).add_modifier(Modifier::BOLD))
        .column_spacing(1)
        .row_highlight_style(selection(focused))
        .block(block(title, focused));
    let mut state = TableState::default().with_selected(Some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
    mouse.register(
        area,
        1,
        first.saturating_add(state.offset()),
        runtime.asynchronous_groups.len(),
        ListTarget::AsynchronousTaskGroups,
    );
}

fn asynchronous_row(group: &AsynchronousTaskGroupSummary, wide: bool) -> Row<'static> {
    let mut cells = vec![
        descriptor(group.type_descriptor_id),
        format_count(group.outcomes.tasks),
        format_count(group.outcomes.active),
    ];
    if wide {
        cells.extend([
            format_count(group.outcomes.completed),
            format_count(group.outcomes.canceled),
            format_count(group.outcomes.panicked),
            format_count(group.outcomes.workers),
            format_count(group.polls),
            duration(group.poll_time.mean_nanos()),
            duration(group.poll_time.max_nanos),
        ]);
    } else {
        cells.extend([
            format_count(group.outcomes.canceled),
            format_count(group.polls),
            duration(group.poll_time.max_nanos),
        ]);
    }
    Row::new(cells.into_iter().enumerate().map(|(index, value)| {
        let line = Line::from(value);
        Cell::from(if index == 0 { line } else { line.right_aligned() })
    }))
}

fn draw_blocking(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    runtime: &RuntimeMonitorSnapshot,
    view: RuntimeTasksViewState,
) {
    if area.is_empty() {
        return;
    }
    let focused = view.focus == RuntimeTasksFocus::Blocking;
    let selected = view.blocking_selected.min(runtime.blocking_groups.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let (tasks, active, completed, canceled, panicked) = outcome_totals(runtime.blocking_groups.iter().map(|group| {
        (
            group.outcomes.tasks,
            group.outcomes.active,
            group.outcomes.completed,
            group.outcomes.canceled,
            group.outcomes.panicked,
        )
    }));
    let executions = runtime
        .blocking_groups
        .iter()
        .fold(0_u64, |total, group| total.saturating_add(group.executions));
    let title = format!(
        " Blocking Task Types · {} groups · {} tasks · {} runs · {} active · {} done · {} canceled · {} panicked · {} global unclassified · b focus ",
        runtime.blocking_groups.len(),
        format_count(tasks),
        format_count(executions),
        format_count(active),
        format_count(completed),
        format_count(canceled),
        format_count(panicked),
        format_count(runtime.unclassified_tasks),
    );
    if runtime.blocking_groups.is_empty() {
        draw_empty(
            frame,
            area,
            title,
            "No blocking task executions are retained in this capture.",
            focused,
        );
        return;
    }
    let wide = area.width >= 155;
    let rows = runtime
        .blocking_groups
        .iter()
        .skip(first)
        .take(visible)
        .map(|group| blocking_row(group, wide));
    let (headers, widths): (Vec<_>, Vec<_>) = if wide {
        (
            vec![
                "Descriptor",
                "Tasks",
                "Runs",
                "Active",
                "Done",
                "Canceled",
                "Panicked",
                "Workers",
                "Avg queue",
                "Max queue",
                "Avg exec",
                "Max exec",
            ],
            vec![14, 8, 8, 8, 8, 9, 9, 8, 12, 12, 12, 12]
                .into_iter()
                .map(Constraint::Length)
                .collect(),
        )
    } else {
        (
            vec!["Descriptor", "Tasks", "Runs", "Canceled", "Max queue", "Max exec"],
            vec![14, 8, 8, 9, 12, 12].into_iter().map(Constraint::Length).collect(),
        )
    };
    let table = Table::new(rows, widths)
        .header(Row::new(headers).add_modifier(Modifier::BOLD))
        .column_spacing(1)
        .row_highlight_style(selection(focused))
        .block(block(title, focused));
    let mut state = TableState::default().with_selected(Some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
    mouse.register(
        area,
        1,
        first.saturating_add(state.offset()),
        runtime.blocking_groups.len(),
        ListTarget::BlockingTaskGroups,
    );
}

fn blocking_row(group: &BlockingTaskGroupSummary, wide: bool) -> Row<'static> {
    let mut cells = vec![
        descriptor(group.type_descriptor_id),
        format_count(group.outcomes.tasks),
        format_count(group.executions),
    ];
    if wide {
        cells.extend([
            format_count(group.outcomes.active),
            format_count(group.outcomes.completed),
            format_count(group.outcomes.canceled),
            format_count(group.outcomes.panicked),
            format_count(group.outcomes.workers),
            duration(group.queue_time.mean_nanos()),
            duration(group.queue_time.max_nanos),
            duration(group.execution_time.mean_nanos()),
            duration(group.execution_time.max_nanos),
        ]);
    } else {
        cells.extend([
            format_count(group.outcomes.canceled),
            duration(group.queue_time.max_nanos),
            duration(group.execution_time.max_nanos),
        ]);
    }
    Row::new(cells.into_iter().enumerate().map(|(index, value)| {
        let line = Line::from(value);
        Cell::from(if index == 0 { line } else { line.right_aligned() })
    }))
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Layout;

    use super::*;
    use crate::commands::monitor::runtime::{DurationSummary, TaskOutcomeSummary};

    #[test]
    fn task_group_tables_show_category_specific_statistics() {
        let runtime = RuntimeMonitorSnapshot {
            asynchronous_groups: vec![AsynchronousTaskGroupSummary {
                type_descriptor_id: Some(42),
                outcomes: TaskOutcomeSummary {
                    tasks: 3,
                    active: 1,
                    completed: 1,
                    canceled: 1,
                    workers: 2,
                    ..TaskOutcomeSummary::default()
                },
                polls: 9,
                poll_time: DurationSummary {
                    samples: 3,
                    total_nanos: 90,
                    max_nanos: Some(50),
                },
            }],
            blocking_groups: vec![BlockingTaskGroupSummary {
                type_descriptor_id: Some(77),
                outcomes: TaskOutcomeSummary {
                    tasks: 2,
                    completed: 1,
                    canceled: 1,
                    workers: 1,
                    ..TaskOutcomeSummary::default()
                },
                executions: 2,
                queue_time: DurationSummary {
                    samples: 2,
                    total_nanos: 60,
                    max_nanos: Some(40),
                },
                execution_time: DurationSummary {
                    samples: 2,
                    total_nanos: 240,
                    max_nanos: Some(200),
                },
            }],
            unclassified_tasks: 1,
            ..RuntimeMonitorSnapshot::default()
        };
        let view = RuntimeTasksViewState {
            focus: RuntimeTasksFocus::Asynchronous,
            asynchronous_selected: 0,
            blocking_selected: 0,
        };
        let mouse = MouseRows::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(180, 18)).unwrap();
        terminal
            .draw(|frame| {
                mouse.begin(frame.area());
                let [asynchronous, blocking] =
                    Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(frame.area());
                draw(frame, &mouse, [asynchronous, blocking], Some(&runtime), None, view);
            })
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();

        assert!(text.contains("Async Task Types"));
        assert!(text.contains("Blocking Task Types"));
        assert!(text.contains("#42"));
        assert!(text.contains("#77"));
        assert!(text.contains("50ns"));
        assert!(text.contains("200ns"));
        assert!(text.contains("1 global unclassified"));
        assert_eq!(
            mouse.at(terminal.backend().buffer().area, 1, 2),
            Some((ListTarget::AsynchronousTaskGroups, 0))
        );
    }
}
