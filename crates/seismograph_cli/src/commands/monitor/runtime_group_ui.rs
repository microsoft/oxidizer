// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Accumulated task-type statistics and the tasks in the selected type.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Borders, Cell, Paragraph, Row, Table, TableState};

use super::super::app::{
    GroupHistogram, RuntimeDetailMode, RuntimeSectionFocus, RuntimeTaskRowSort, RuntimeViewState, RuntimeWorkersFocus,
    selected_runtime_group, sort_runtime_tasks,
};
use super::super::data::{RuntimeMonitorSnapshot, RuntimeTaskSummary};
use super::super::mouse::{ListTarget, MouseRows};
use super::super::runtime::{AsynchronousTaskGroupSummary, BlockingTaskGroupSummary};
use super::super::runtime_timeline::histogram;
use super::{draw_empty_panel_with_message, format_count, format_histogram_duration, format_runtime_duration};

pub(super) fn draw_statistics(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    runtime: Option<&RuntimeMonitorSnapshot>,
    unavailable: Option<&str>,
    view: RuntimeViewState,
) {
    let Some(runtime) = runtime else {
        draw_empty_panel_with_message(frame, area, " Task Type Details ", unavailable);
        return;
    };
    let Some((runtime_id, worker_id, descriptor, _kind)) = selected_runtime_group(runtime, &view) else {
        draw_empty_panel_with_message(frame, area, " Task Type Details ", Some("Select a task type."));
        return;
    };
    match view.focus {
        RuntimeWorkersFocus::Asynchronous => {
            let group = runtime
                .asynchronous_groups
                .iter()
                .filter(|group| group.runtime_id == runtime_id)
                .filter(|group| group.worker_id == worker_id)
                .find(|group| group.type_descriptor_id == descriptor);
            draw_async_statistics(
                frame,
                area,
                group,
                view.group_histogram,
                view.detail_mode == RuntimeDetailMode::Group,
            );
        }
        RuntimeWorkersFocus::Blocking => {
            let group = runtime
                .blocking_groups
                .iter()
                .filter(|group| group.runtime_id == runtime_id)
                .filter(|group| group.worker_id == worker_id)
                .find(|group| group.type_descriptor_id == descriptor);
            draw_blocking_statistics(
                frame,
                area,
                group,
                view.group_histogram,
                view.detail_mode == RuntimeDetailMode::Group,
            );
        }
    }
}

pub(super) fn draw_task_list(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    runtime: Option<&RuntimeMonitorSnapshot>,
    unavailable: Option<&str>,
    view: RuntimeViewState,
) {
    let Some(runtime) = runtime else {
        draw_empty_panel_with_message(frame, area, " Tasks ", unavailable);
        return;
    };
    let Some((runtime_id, worker_id, descriptor, kind)) = selected_runtime_group(runtime, &view) else {
        draw_empty_panel_with_message(frame, area, " Tasks ", Some("Select a task type."));
        return;
    };
    let tasks = runtime.tasks_for_group(runtime_id, worker_id, descriptor, kind);
    draw_tasks(frame, mouse, area, &tasks, view);
}

fn block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
}

fn duration(value: Option<u64>) -> String {
    value.map_or_else(|| "-".into(), format_runtime_duration)
}

fn task_spawn(task: &RuntimeTaskSummary) -> String {
    match (task.origin_worker_id, task.worker_ids.first().copied()) {
        (None, _) => "external".into(),
        (Some(origin), Some(worker)) if origin == worker => format!("local w{worker}"),
        (Some(origin), Some(worker)) => format!("w{origin} -> w{worker}"),
        (Some(origin), None) => format!("w{origin} -> ?"),
    }
}

fn sort_headers(headers: Vec<&str>, sorts: &[RuntimeTaskRowSort], view: RuntimeViewState) -> Vec<String> {
    headers
        .into_iter()
        .zip(sorts)
        .map(|(header, sort)| {
            if *sort == view.task_sort {
                format!("{header}{}", if view.task_sort_descending { "▼" } else { "▲" })
            } else {
                header.into()
            }
        })
        .collect()
}

fn register_sort_headers(mouse: &MouseRows, area: Rect, widths: &[Constraint], sorts: &[RuntimeTaskRowSort]) {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if inner.is_empty() {
        return;
    }
    let spacing = u16::try_from(widths.len().saturating_sub(1)).unwrap_or(u16::MAX);
    let fixed = widths.iter().fold(0_u16, |total, width| {
        total.saturating_add(match width {
            Constraint::Length(width) => *width,
            Constraint::Min(_) => 0,
            _ => 0,
        })
    });
    let flexible = inner.width.saturating_sub(fixed).saturating_sub(spacing);
    let mut x = inner.x;
    for (width, sort) in widths.iter().zip(sorts) {
        let width = match width {
            Constraint::Length(width) => *width,
            Constraint::Min(minimum) => flexible.max(*minimum),
            _ => 0,
        }
        .min(inner.right().saturating_sub(x));
        mouse.register_item(Rect::new(x, inner.y, width, 1), ListTarget::RuntimeTaskHeader(*sort), 0);
        x = x.saturating_add(width).saturating_add(1);
    }
}

fn draw_async_statistics(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    group: Option<&AsynchronousTaskGroupSummary>,
    histogram_kind: GroupHistogram,
    focused: bool,
) {
    let Some(group) = group else {
        draw_empty_panel_with_message(frame, area, " Async Task Type ", Some("Task type data is unavailable."));
        return;
    };
    let (label, samples) = match histogram_kind {
        GroupHistogram::Primary => ("Poll duration", group.poll_samples.as_slice()),
        GroupHistogram::Secondary => ("Ready wait", group.ready_samples.as_slice()),
    };
    let [summary, chart] = Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).areas(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(group.spawned_at.as_deref().unwrap_or("Unknown spawn location")),
            Line::from(format!(
                "{} spawned · {} live · {} succeeded · {} canceled · {} panicked",
                format_count(group.outcomes.tasks),
                format_count(group.outcomes.active),
                format_count(group.outcomes.completed),
                format_count(group.outcomes.canceled),
                format_count(group.outcomes.panicked),
            )),
            Line::from(format!(
                "{} workers · {} polls",
                format_count(group.outcomes.workers),
                format_count(group.polls)
            )),
            Line::from(format!(
                "Ready: avg {} · max {}",
                duration(group.ready_time.mean_nanos()),
                duration(group.ready_time.max_nanos)
            )),
            Line::from(format!(
                "Poll: avg {} · max {}",
                duration(group.poll_time.mean_nanos()),
                duration(group.poll_time.max_nanos)
            )),
            Line::from(format!("{label} · t switches histogram")),
        ])
        .block(block(" Task Type Statistics ", focused)),
        summary,
    );
    draw_histogram(frame, chart, label, samples);
}

fn draw_blocking_statistics(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    group: Option<&BlockingTaskGroupSummary>,
    histogram_kind: GroupHistogram,
    focused: bool,
) {
    let Some(group) = group else {
        draw_empty_panel_with_message(frame, area, " Blocking Task Type ", Some("Task type data is unavailable."));
        return;
    };
    let (label, samples) = match histogram_kind {
        GroupHistogram::Primary => ("Execution duration", group.execution_samples.as_slice()),
        GroupHistogram::Secondary => ("Queue wait", group.queue_samples.as_slice()),
    };
    let [summary, chart] = Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).areas(area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(group.spawned_at.as_deref().unwrap_or("Unknown spawn location")),
            Line::from(format!(
                "{} spawned · {} runs · {} live · {} succeeded · {} canceled · {} panicked",
                format_count(group.outcomes.tasks),
                format_count(group.executions),
                format_count(group.outcomes.active),
                format_count(group.outcomes.completed),
                format_count(group.outcomes.canceled),
                format_count(group.outcomes.panicked),
            )),
            Line::from(format!("{} workers", format_count(group.outcomes.workers))),
            Line::from(format!(
                "Queue: avg {} · max {}",
                duration(group.queue_time.mean_nanos()),
                duration(group.queue_time.max_nanos)
            )),
            Line::from(format!(
                "Execution: avg {} · max {}",
                duration(group.execution_time.mean_nanos()),
                duration(group.execution_time.max_nanos)
            )),
            Line::from(format!("{label} · t switches histogram")),
        ])
        .block(block(" Task Type Statistics ", focused)),
        summary,
    );
    draw_histogram(frame, chart, label, samples);
}

fn draw_histogram(frame: &mut ratatui::Frame<'_>, area: Rect, label: &str, samples: &[u64]) {
    if area.is_empty() {
        return;
    }
    if samples.is_empty() {
        frame.render_widget(
            Paragraph::new("No completed duration samples.")
                .style(Style::default().fg(Color::DarkGray))
                .block(block(format!(" {label} "), false)),
            area,
        );
        return;
    }
    let count = usize::from(area.width.saturating_sub(2) / 9).clamp(1, 12);
    let bins = histogram(samples, count);
    let bars = bins
        .iter()
        .map(|bin| {
            Bar::default()
                .value(bin.count)
                .label(Line::from(format_histogram_duration(bin.lower)))
                .style(Style::default().fg(Color::Cyan))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        BarChart::default()
            .block(block(
                format!(" {label} · n={} · max {} ", samples.len(), duration(samples.iter().copied().max())),
                false,
            ))
            .data(BarGroup::default().bars(&bars))
            .bar_width(5)
            .bar_gap(1)
            .max(bins.iter().map(|bin| bin.count).max().unwrap_or(1)),
        area,
    );
}

fn draw_tasks(frame: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, tasks: &[&RuntimeTaskSummary], view: RuntimeViewState) {
    if area.is_empty() {
        return;
    }
    let mut tasks = tasks.to_vec();
    sort_runtime_tasks(&mut tasks, view);
    let selected = view.task_selected.min(tasks.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let wide = area.width >= 82;
    let rows = tasks.iter().skip(first).take(visible).map(|task| task_row(task, view.focus));
    let (headers, widths, sorts) = match (view.focus, wide) {
        (RuntimeWorkersFocus::Asynchronous, true) => (
            vec!["Task", "State", "Spawn", "Ops", "Polls", "Max poll"],
            vec![8, 11, 16, 7, 8, 12].into_iter().map(Constraint::Length).collect::<Vec<_>>(),
            vec![
                RuntimeTaskRowSort::Task,
                RuntimeTaskRowSort::State,
                RuntimeTaskRowSort::Spawn,
                RuntimeTaskRowSort::Operations,
                RuntimeTaskRowSort::Polls,
                RuntimeTaskRowSort::MaximumPoll,
            ],
        ),
        (RuntimeWorkersFocus::Asynchronous, false) => (
            vec!["Task", "State", "Spawn", "Ops", "Polls", "Max poll"],
            vec![
                Constraint::Length(8),
                Constraint::Length(9),
                Constraint::Min(10),
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Length(11),
            ],
            vec![
                RuntimeTaskRowSort::Task,
                RuntimeTaskRowSort::State,
                RuntimeTaskRowSort::Spawn,
                RuntimeTaskRowSort::Operations,
                RuntimeTaskRowSort::Polls,
                RuntimeTaskRowSort::MaximumPoll,
            ],
        ),
        (RuntimeWorkersFocus::Blocking, true) => (
            vec!["Task", "State", "Spawn", "Ops", "Queue", "Execution"],
            vec![8, 11, 16, 7, 12, 12].into_iter().map(Constraint::Length).collect::<Vec<_>>(),
            vec![
                RuntimeTaskRowSort::Task,
                RuntimeTaskRowSort::State,
                RuntimeTaskRowSort::Spawn,
                RuntimeTaskRowSort::Operations,
                RuntimeTaskRowSort::Queue,
                RuntimeTaskRowSort::Execution,
            ],
        ),
        (RuntimeWorkersFocus::Blocking, false) => (
            vec!["Task", "State", "Spawn", "Ops", "Queue", "Execution"],
            vec![
                Constraint::Length(8),
                Constraint::Length(9),
                Constraint::Min(10),
                Constraint::Length(6),
                Constraint::Length(11),
                Constraint::Length(11),
            ],
            vec![
                RuntimeTaskRowSort::Task,
                RuntimeTaskRowSort::State,
                RuntimeTaskRowSort::Spawn,
                RuntimeTaskRowSort::Operations,
                RuntimeTaskRowSort::Queue,
                RuntimeTaskRowSort::Execution,
            ],
        ),
    };
    register_sort_headers(mouse, area, &widths, &sorts);
    let table = Table::new(rows, widths)
        .header(Row::new(sort_headers(headers, &sorts, view)).style(Style::default().add_modifier(Modifier::BOLD)))
        .column_spacing(1)
        .row_highlight_style(if view.section == RuntimeSectionFocus::GroupTasks {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default().bg(Color::DarkGray)
        })
        .block(block(
            format!(" Tasks · {} · select then open Runtime Task ", tasks.len()),
            view.section == RuntimeSectionFocus::GroupTasks,
        ));
    let mut state = TableState::default().with_selected((!tasks.is_empty()).then_some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
    mouse.register(
        area,
        1,
        first.saturating_add(state.offset()),
        tasks.len(),
        ListTarget::RuntimeGroupTasks,
    );
}

fn task_row(task: &RuntimeTaskSummary, focus: RuntimeWorkersFocus) -> Row<'static> {
    let spawn = task_spawn(task);
    let mut cells = vec![format!("#{}", task.task_id), task.state.clone(), spawn];
    match focus {
        RuntimeWorkersFocus::Asynchronous => cells.extend([
            format_count(task.operations),
            format_count(task.metrics.poll_count),
            duration(task.metrics.max_poll_nanos),
        ]),
        RuntimeWorkersFocus::Blocking => cells.extend([
            format_count(task.operations),
            duration(task.blocking.queue_duration_nanos),
            duration(task.blocking.execution_duration_nanos),
        ]),
    }
    Row::new(cells.into_iter().enumerate().map(|(index, value)| {
        let line = Line::from(value);
        Cell::from(if index >= 3 { line.right_aligned() } else { line })
    }))
}
