// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime workers and category-specific worker activity.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Borders, Cell, Paragraph, Row, Table, TableState};

use super::super::app::{RuntimeSectionFocus, RuntimeViewState, RuntimeWorkersFocus};
use super::super::data::{RuntimeMonitorSnapshot, RuntimeWorkerSummary};
use super::super::mouse::{ListTarget, MouseRows};
use super::super::runtime_timeline::histogram;
use super::{draw_empty_panel_with_message, format_count, format_runtime_duration};

pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    [workers, activity]: [Rect; 2],
    runtime: Option<&RuntimeMonitorSnapshot>,
    unavailable: Option<&str>,
    view: RuntimeViewState,
) {
    let Some(runtime) = runtime else {
        draw_empty_panel_with_message(frame, workers, " Runtime Workers ", unavailable);
        draw_empty_panel_with_message(frame, activity, " Worker Activity ", unavailable);
        return;
    };
    let selected_workers = workers_for(runtime, view);
    draw_workers(frame, mouse, workers, runtime, &selected_workers, view);
    let selected = selected_index(view).min(selected_workers.len().saturating_sub(1));
    draw_worker_activity(frame, activity, selected_workers.get(selected).copied(), view.focus);
}

fn block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
}

fn selection(selected: bool) -> Style {
    if selected {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().bg(Color::DarkGray)
    }
}

fn workers_for(runtime: &RuntimeMonitorSnapshot, view: RuntimeViewState) -> Vec<&RuntimeWorkerSummary> {
    let runtime_id = runtime.selected_runtime(view.runtime_selected).map(|runtime| runtime.runtime_id);
    runtime
        .workers
        .iter()
        .filter(|worker| Some(worker.runtime_id) == runtime_id)
        .filter(|worker| worker.worker_id.is_some())
        .filter(|worker| match view.focus {
            RuntimeWorkersFocus::Asynchronous => !is_blocking(worker),
            RuntimeWorkersFocus::Blocking => is_blocking(worker),
        })
        .collect()
}

fn is_blocking(worker: &RuntimeWorkerSummary) -> bool {
    worker.role == "Blocking" || worker.blocking.executions != 0
}

const fn selected_index(view: RuntimeViewState) -> usize {
    match view.focus {
        RuntimeWorkersFocus::Asynchronous => view.asynchronous_selected,
        RuntimeWorkersFocus::Blocking => view.blocking_selected,
    }
}

fn worker_name(worker: &RuntimeWorkerSummary) -> String {
    worker.worker_id.map_or_else(|| "Unknown".into(), |id| format!("w{id}"))
}

fn duration(value: Option<u64>) -> String {
    value.map_or_else(|| "-".into(), format_runtime_duration)
}

fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn task_count_label(count: u64) -> String {
    format!("{} live {}", format_count(count), if count == 1 { "task" } else { "tasks" })
}

fn timeline(
    intervals: &[super::super::runtime_timeline::Interval],
    window: Option<super::super::runtime_timeline::TimeWindow>,
    width: usize,
) -> Line<'static> {
    Line::from(
        super::super::runtime_timeline::bins(intervals, window, width)
            .into_iter()
            .map(|value| {
                let glyph = match value {
                    None | Some(0.0) => "▁",
                    Some(value) if value < 0.25 => "▂",
                    Some(value) if value < 0.375 => "▃",
                    Some(value) if value < 0.5 => "▄",
                    Some(value) if value < 0.625 => "▅",
                    Some(value) if value < 0.75 => "▆",
                    Some(value) if value < 0.875 => "▇",
                    Some(_) => "█",
                };
                Span::styled(
                    glyph,
                    Style::default().fg(if value.is_some() { Color::Cyan } else { Color::DarkGray }),
                )
            })
            .collect::<Vec<_>>(),
    )
}

fn draw_workers(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    runtime: &RuntimeMonitorSnapshot,
    workers: &[&RuntimeWorkerSummary],
    view: RuntimeViewState,
) {
    if area.is_empty() {
        return;
    }
    let [tabs, table] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let runtime_summary = runtime.selected_runtime(view.runtime_selected);
    let runtime_label = runtime_summary.map_or_else(
        || "Runtime: unavailable".into(),
        |runtime_summary| {
            let position = view.runtime_selected.min(runtime.runtimes.len().saturating_sub(1)) + 1;
            if runtime.runtimes.len() > 1 {
                format!(
                    "Runtime: {} (#{}) [{position}/{}] · [/] select",
                    runtime_summary.name,
                    runtime_summary.runtime_id,
                    runtime.runtimes.len()
                )
            } else {
                format!("Runtime: {} (#{})", runtime_summary.name, runtime_summary.runtime_id)
            }
        },
    );
    let tab_offset = u16::try_from(runtime_label.chars().count().saturating_add(3)).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(runtime_label),
            Span::raw(" | "),
            Span::styled("Async workers", selection(view.focus == RuntimeWorkersFocus::Asynchronous)),
            Span::raw(" | "),
            Span::styled("Blocking workers", selection(view.focus == RuntimeWorkersFocus::Blocking)),
            Span::raw(" · a/b · Enter workers/task types"),
        ])),
        tabs,
    );
    mouse.register_tab(
        Rect::new(tabs.x.saturating_add(tab_offset), tabs.y, 13, tabs.height).intersection(tabs),
        ListTarget::AsynchronousWorkersTab,
    );
    mouse.register_tab(
        Rect::new(tabs.x.saturating_add(tab_offset).saturating_add(16), tabs.y, 16, tabs.height).intersection(tabs),
        ListTarget::BlockingWorkersTab,
    );

    let selected = selected_index(view).min(workers.len().saturating_sub(1));
    let focused = view.section == RuntimeSectionFocus::Workers;
    let visible = usize::from(table.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let live_tasks = match view.focus {
        RuntimeWorkersFocus::Asynchronous => runtime
            .asynchronous_groups
            .iter()
            .filter(|group| Some(group.runtime_id) == runtime_summary.map(|runtime| runtime.runtime_id))
            .fold(0_u64, |total, group| total.saturating_add(group.outcomes.active)),
        RuntimeWorkersFocus::Blocking => runtime
            .blocking_groups
            .iter()
            .filter(|group| Some(group.runtime_id) == runtime_summary.map(|runtime| runtime.runtime_id))
            .fold(0_u64, |total, group| total.saturating_add(group.outcomes.active)),
    };
    let title = format!(
        " {} · {} · {} ",
        match view.focus {
            RuntimeWorkersFocus::Asynchronous => "Async Workers",
            RuntimeWorkersFocus::Blocking => "Blocking Workers",
        },
        count_label(workers.len(), "worker", "workers"),
        task_count_label(live_tasks),
    );
    if workers.is_empty() {
        frame.render_widget(
            Paragraph::new(match view.focus {
                RuntimeWorkersFocus::Asynchronous => "No asynchronous workers are present in this capture.",
                RuntimeWorkersFocus::Blocking => "No blocking workers are present in this capture.",
            })
            .style(Style::default().fg(Color::DarkGray))
            .block(block(title, focused)),
            table,
        );
        return;
    }
    let wide = table.width >= 125;
    let rows = workers
        .iter()
        .skip(first)
        .take(visible)
        .map(|worker| worker_row(worker, view.focus, wide, table.width));
    let (headers, widths) = worker_columns(view.focus, wide);
    let widget = Table::new(rows, widths)
        .header(Row::new(headers).style(Style::default().add_modifier(Modifier::BOLD)))
        .column_spacing(1)
        .row_highlight_style(selection(focused))
        .block(block(title, focused));
    let mut state = TableState::default().with_selected(Some(selected.saturating_sub(first)));
    frame.render_stateful_widget(widget, table, &mut state);
    mouse.register(
        table,
        1,
        first.saturating_add(state.offset()),
        workers.len(),
        match view.focus {
            RuntimeWorkersFocus::Asynchronous => ListTarget::AsynchronousWorkers,
            RuntimeWorkersFocus::Blocking => ListTarget::BlockingWorkers,
        },
    );
}

fn worker_columns(focus: RuntimeWorkersFocus, wide: bool) -> (Vec<&'static str>, Vec<Constraint>) {
    match (focus, wide) {
        (RuntimeWorkersFocus::Asynchronous, true) => (
            vec![
                "Worker ID",
                "State",
                "Types",
                "Tasks",
                "Polls",
                "Median poll",
                "Max poll",
                "Poll timeline",
            ],
            vec![
                Constraint::Length(24),
                Constraint::Length(9),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(12),
                Constraint::Length(12),
                Constraint::Min(8),
            ],
        ),
        (RuntimeWorkersFocus::Asynchronous, false) => (
            vec!["Worker ID", "Types", "Polls", "Max poll"],
            vec![
                Constraint::Min(18),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(12),
            ],
        ),
        (RuntimeWorkersFocus::Blocking, true) => (
            vec![
                "Worker ID",
                "State",
                "Types",
                "Runs",
                "Succeeded",
                "Canceled",
                "Panicked",
                "Avg queue",
                "Max queue",
                "Avg exec",
                "Max exec",
            ],
            vec![24, 9, 7, 8, 10, 9, 9, 12, 12, 12, 12]
                .into_iter()
                .map(Constraint::Length)
                .collect(),
        ),
        (RuntimeWorkersFocus::Blocking, false) => (
            vec!["Worker ID", "Types", "Runs", "Succeeded", "Max queue", "Max exec"],
            vec![
                Constraint::Min(18),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(10),
                Constraint::Length(12),
                Constraint::Length(12),
            ],
        ),
    }
}

fn worker_row(worker: &RuntimeWorkerSummary, focus: RuntimeWorkersFocus, wide: bool, width: u16) -> Row<'static> {
    let mut cells = vec![worker_name(worker)];
    match (focus, wide) {
        (RuntimeWorkersFocus::Asynchronous, true) => {
            cells.extend([
                worker.state.clone(),
                worker.task_types.to_string(),
                worker.observed_tasks.to_string(),
                format_count(worker.metrics.poll_count),
                duration(worker.metrics.median_poll_nanos),
                duration(worker.metrics.max_poll_nanos),
            ]);
            let graph_width = usize::from(width.saturating_sub(86));
            let mut cells = cells
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    let line = Line::from(value);
                    Cell::from(if index >= 2 { line.right_aligned() } else { line })
                })
                .collect::<Vec<_>>();
            cells.push(Cell::from(timeline(&worker.metrics.polls, worker.window, graph_width)));
            return Row::new(cells);
        }
        (RuntimeWorkersFocus::Asynchronous, false) => {
            cells.extend([
                worker.task_types.to_string(),
                format_count(worker.metrics.poll_count),
                duration(worker.metrics.max_poll_nanos),
            ]);
        }
        (RuntimeWorkersFocus::Blocking, true) => {
            cells.extend([
                worker.state.clone(),
                worker.task_types.to_string(),
                format_count(worker.blocking.executions),
                format_count(worker.blocking.succeeded),
                format_count(worker.blocking.canceled),
                format_count(worker.blocking.panicked),
                duration(worker.blocking.queue_time.mean_nanos()),
                duration(worker.blocking.queue_time.max_nanos),
                duration(worker.blocking.execution_time.mean_nanos()),
                duration(worker.blocking.execution_time.max_nanos),
            ]);
        }
        (RuntimeWorkersFocus::Blocking, false) => {
            cells.extend([
                worker.task_types.to_string(),
                format_count(worker.blocking.executions),
                format_count(worker.blocking.succeeded),
                duration(worker.blocking.queue_time.max_nanos),
                duration(worker.blocking.execution_time.max_nanos),
            ]);
        }
    }
    Row::new(cells.into_iter().enumerate().map(|(index, value)| {
        let line = Line::from(value);
        Cell::from(if index >= 1 { line.right_aligned() } else { line })
    }))
}

fn draw_worker_activity(frame: &mut ratatui::Frame<'_>, area: Rect, worker: Option<&RuntimeWorkerSummary>, focus: RuntimeWorkersFocus) {
    if area.is_empty() {
        return;
    }
    let title = match focus {
        RuntimeWorkersFocus::Asynchronous => " Async Worker Activity ",
        RuntimeWorkersFocus::Blocking => " Blocking Worker Activity ",
    };
    let Some(worker) = worker else {
        draw_empty_panel_with_message(frame, area, title, Some("Select a worker."));
        return;
    };
    if focus == RuntimeWorkersFocus::Asynchronous {
        let [summary, chart] = Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(area);
        let lines = vec![
            Line::from(worker_name(worker)),
            Line::from(format!(
                "{} task types · {} tasks · {} polls",
                worker.task_types,
                worker.observed_tasks,
                format_count(worker.metrics.poll_count)
            )),
            Line::from(format!(
                "Poll duration: median {} · max {}",
                duration(worker.metrics.median_poll_nanos),
                duration(worker.metrics.max_poll_nanos)
            )),
            Line::from(timeline(
                &worker.metrics.polls,
                worker.window,
                usize::from(area.width.saturating_sub(2)),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).block(block(title, false)), summary);
        draw_poll_histogram(frame, chart, &worker.metrics.poll_samples);
        return;
    }
    let lines = vec![
        Line::from(worker_name(worker)),
        Line::from(format!(
            "{} task types · {} runs",
            worker.task_types,
            format_count(worker.blocking.executions),
        )),
        Line::from(format!(
            "{} succeeded · {} canceled · {} panicked",
            format_count(worker.blocking.succeeded),
            format_count(worker.blocking.canceled),
            format_count(worker.blocking.panicked),
        )),
        Line::from(format!(
            "Queue: avg {} · max {}",
            duration(worker.blocking.queue_time.mean_nanos()),
            duration(worker.blocking.queue_time.max_nanos),
        )),
        Line::from(format!(
            "Execution: avg {} · max {}",
            duration(worker.blocking.execution_time.mean_nanos()),
            duration(worker.blocking.execution_time.max_nanos),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).block(block(title, false)), area);
}

fn draw_poll_histogram(frame: &mut ratatui::Frame<'_>, area: Rect, samples: &[u64]) {
    if samples.is_empty() {
        draw_empty_panel_with_message(frame, area, " Poll Duration ", Some("No completed poll samples."));
        return;
    }
    let count = usize::from(area.width.saturating_sub(2) / 9).clamp(1, 12);
    let bins = histogram(samples, count);
    let bars = bins
        .iter()
        .map(|bin| {
            Bar::default()
                .value(bin.count)
                .label(Line::from(format_runtime_duration(bin.lower)))
                .style(Style::default().fg(Color::Cyan))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        BarChart::default()
            .block(block(
                format!(
                    " Poll Duration · n={} · max {} ",
                    samples.len(),
                    duration(samples.iter().copied().max())
                ),
                false,
            ))
            .data(BarGroup::default().bars(&bars))
            .bar_width(5)
            .bar_gap(1)
            .max(bins.iter().map(|bin| bin.count).max().unwrap_or(1)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::monitor::runtime::{BlockingWorkerSummary, DurationSummary, RuntimeSummary};
    use crate::commands::monitor::runtime_timeline::ExecutionMetrics;

    fn runtime() -> RuntimeMonitorSnapshot {
        RuntimeMonitorSnapshot {
            runtimes: vec![RuntimeSummary {
                runtime_id: 1,
                name: "demo".into(),
            }],
            workers: vec![
                RuntimeWorkerSummary {
                    runtime_id: 1,
                    runtime_name: "demo".into(),
                    worker_id: Some(1),
                    role: "Core".into(),
                    state: "Running".into(),
                    observed_tasks: 3,
                    task_types: 2,
                    metrics: ExecutionMetrics {
                        poll_count: 7,
                        median_poll_nanos: Some(20),
                        max_poll_nanos: Some(50),
                        ..ExecutionMetrics::default()
                    },
                    ..RuntimeWorkerSummary::default()
                },
                RuntimeWorkerSummary {
                    runtime_id: 1,
                    runtime_name: "demo".into(),
                    worker_id: Some(2),
                    role: "Blocking".into(),
                    state: "Running".into(),
                    task_types: 1,
                    blocking: BlockingWorkerSummary {
                        executions: 4,
                        succeeded: 3,
                        canceled: 1,
                        queue_time: DurationSummary {
                            samples: 4,
                            total_nanos: 80,
                            max_nanos: Some(40),
                        },
                        execution_time: DurationSummary {
                            samples: 4,
                            total_nanos: 400,
                            max_nanos: Some(200),
                        },
                        ..BlockingWorkerSummary::default()
                    },
                    ..RuntimeWorkerSummary::default()
                },
            ],
            ..RuntimeMonitorSnapshot::default()
        }
    }

    #[test]
    fn worker_tabs_show_category_specific_metrics_without_observed_percentage() {
        let runtime = runtime();
        let mouse = MouseRows::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(150, 16)).unwrap();
        for focus in [RuntimeWorkersFocus::Asynchronous, RuntimeWorkersFocus::Blocking] {
            terminal
                .draw(|frame| {
                    mouse.begin(frame.area());
                    let [workers, activity] = Layout::horizontal([Constraint::Percentage(65), Constraint::Min(0)]).areas(frame.area());
                    draw(
                        frame,
                        &mouse,
                        [workers, activity],
                        Some(&runtime),
                        None,
                        RuntimeViewState {
                            focus,
                            ..RuntimeViewState::new()
                        },
                    );
                })
                .unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            assert!(!text.contains("Observed %"));
            match focus {
                RuntimeWorkersFocus::Asynchronous => {
                    assert!(text.contains("Async Workers"));
                    assert!(text.contains("Async Worker Activity"));
                    assert!(text.contains("50ns"));
                }
                RuntimeWorkersFocus::Blocking => {
                    assert!(text.contains("Blocking Workers"));
                    assert!(text.contains("Blocking Worker Activity"));
                    assert!(text.contains("200ns"));
                }
            }
        }
    }
}
