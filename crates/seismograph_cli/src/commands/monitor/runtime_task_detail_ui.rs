// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Rich statistics for one task selected from a task-type group.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Borders, Paragraph};

use super::super::app::{RuntimeDetailMode, RuntimeViewState, TaskHistogram};
use super::super::data::RuntimeTaskSummary;
use super::super::runtime::RuntimeTaskKind;
use super::super::runtime_timeline::{TimeWindow, bins, histogram};
use super::{draw_empty_panel_with_message, format_count, format_runtime_duration};

pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    task: Option<&RuntimeTaskSummary>,
    window: Option<TimeWindow>,
    view: RuntimeViewState,
) {
    let Some(task) = task else {
        draw_empty_panel_with_message(frame, area, " Task Statistics ", Some("Select a task."));
        return;
    };
    let [summary, chart] = Layout::vertical([Constraint::Length(9), Constraint::Min(0)]).areas(area);
    let worker = task.worker_ids.first().map_or_else(|| "-".into(), |worker| format!("w{worker}"));
    let origin = match (task.origin_worker_id, task.worker_ids.first().copied()) {
        (None, _) => "external".into(),
        (Some(origin), Some(execution)) if origin == execution => format!("local on w{execution}"),
        (Some(origin), Some(execution)) => format!("w{origin} -> w{execution}"),
        (Some(origin), None) => format!("w{origin} -> ?"),
    };
    let mut lines = vec![
        Line::from(format!("Task #{} · {} · worker {worker}", task.task_id, task.state)),
        Line::from(format!(
            "Runtime #{} · parent {} · descriptor {}",
            task.runtime_id,
            task.parent_id.map_or_else(|| "-".into(), |parent| format!("#{parent}")),
            task.type_descriptor_id
                .map_or_else(|| "unknown".into(), |descriptor| format!("#{descriptor}")),
        )),
        Line::from(format!("Spawn origin: {origin}")),
        Line::from(format!("Spawned at: {}", task.spawned_at.as_deref().unwrap_or("Unknown"))),
    ];
    let (label, samples) = match task.kind {
        RuntimeTaskKind::Asynchronous => {
            lines.push(Line::from(format!(
                "{} polls · median {} · max {}",
                format_count(task.metrics.poll_count),
                duration(task.metrics.median_poll_nanos),
                duration(task.metrics.max_poll_nanos),
            )));
            let ready_average = (!task.metrics.ready_samples.is_empty()).then(|| {
                task.metrics.ready_samples.iter().copied().fold(0_u64, u64::saturating_add)
                    / u64::try_from(task.metrics.ready_samples.len()).unwrap_or(u64::MAX)
            });
            lines.push(Line::from(format!(
                "Ready wait: {} samples · avg {} · max {}",
                format_count(u64::try_from(task.metrics.ready_samples.len()).unwrap_or(u64::MAX)),
                duration(ready_average),
                duration(task.metrics.ready_samples.iter().copied().max()),
            )));
            lines.push(poll_timeline(task, window, usize::from(area.width.saturating_sub(2))));
            match view.task_histogram {
                TaskHistogram::Poll => ("Poll duration", task.metrics.poll_samples.as_slice()),
                TaskHistogram::Ready => ("Ready wait", task.metrics.ready_samples.as_ref()),
            }
        }
        RuntimeTaskKind::Blocking => {
            lines.push(Line::from(format!(
                "Started {} · queue {} · execution {}",
                if task.blocking.started { "yes" } else { "no" },
                duration(task.blocking.queue_duration_nanos),
                duration(task.blocking.execution_duration_nanos)
            )));
            match view.task_histogram {
                TaskHistogram::Poll => ("Execution duration", task.blocking.execution_duration_nanos.as_slice()),
                TaskHistogram::Ready => ("Queue wait", task.blocking.queue_duration_nanos.as_slice()),
            }
        }
        RuntimeTaskKind::Unknown => ("Duration", &[] as &[u64]),
    };
    lines.push(Line::from(format!("{label} · t switches histogram · e opens inferred events")));
    frame.render_widget(
        Paragraph::new(lines).block(block(
            format!(" Task Statistics · #{} ", task.task_id),
            view.detail_mode == RuntimeDetailMode::Task,
        )),
        summary,
    );
    draw_histogram(frame, chart, label, samples);
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

fn poll_timeline(task: &RuntimeTaskSummary, window: Option<TimeWindow>, width: usize) -> Line<'static> {
    let mut spans = vec![Span::raw("Poll timeline: ")];
    spans.extend(
        bins(&task.metrics.polls, window, width.saturating_sub(15))
            .into_iter()
            .map(|value| {
                let glyph = match value {
                    None | Some(0.0) => "▁",
                    Some(value) if value < 0.25 => "▂",
                    Some(value) if value < 0.5 => "▄",
                    Some(value) if value < 0.75 => "▆",
                    Some(_) => "█",
                };
                Span::styled(
                    glyph,
                    Style::default().fg(if value.is_some() { Color::Cyan } else { Color::DarkGray }),
                )
            }),
    );
    Line::from(spans)
}

fn draw_histogram(frame: &mut ratatui::Frame<'_>, area: Rect, label: &str, samples: &[u64]) {
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
                .label(Line::from(format_runtime_duration(bin.lower)))
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
