// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Observed execution timelines and completed-duration distributions.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, Borders, Cell, Paragraph, Row, Table, TableState};

use super::super::app::{RUNTIME_HISTOGRAM_BINS, RuntimeFocus, RuntimeViewState, TaskHistogram};
use super::super::data::{CapturedSnapshot, RuntimeMonitorSnapshot, RuntimeTaskSummary, RuntimeWorkerSummary};
use super::super::mouse::{ListTarget, MouseRows};
use super::super::runtime_timeline::{ExecutionMetrics, HistogramBin, Interval, TimeWindow, bins, histogram};
use super::{draw_empty_panel_with_message, format_count, format_runtime_duration};

#[cfg_attr(test, mutants::skip)]
pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    [workers, tasks, worker_activity, task_activity, events_area]: [Rect; 5],
    capture: Option<&CapturedSnapshot>,
    unavailable: Option<&str>,
    view: RuntimeViewState,
) {
    let Some(capture) = capture else {
        for (area, title) in [
            (workers, " Runtime Workers "),
            (tasks, " Tasks "),
            (worker_activity, " Worker Activity "),
            (task_activity, " Task Statistics "),
        ] {
            draw_empty_panel_with_message(frame, area, title, unavailable);
        }
        super::task_events_ui::draw(frame, mouse, events_area, None, None, view);
        return;
    };
    let runtime = &capture.runtime;
    let selected = view.worker_selected.min(runtime.workers.len().saturating_sub(1));
    let worker = runtime.workers.get(selected);
    draw_workers(frame, mouse, workers, runtime, view, capture.filter_summary.active);
    let sorted = worker.map_or_else(Vec::new, |worker| worker.sorted_tasks(view.task_sort, view.task_sort_descending));
    draw_tasks(frame, mouse, tasks, &sorted, view);
    let selected = view.task_selected.min(sorted.len().saturating_sub(1));
    let task = sorted.get(selected).copied();
    if let Some(worker) = worker {
        draw_worker_activity(frame, mouse, worker_activity, worker, view);
        draw_task_details(frame, mouse, task_activity, task, worker.window, view);
    } else {
        draw_empty_panel_with_message(frame, worker_activity, " Worker Activity ", Some("Select a worker."));
        draw_empty_panel_with_message(frame, task_activity, " Task Statistics ", Some("Select a task."));
    }
    super::task_events_ui::draw(frame, mouse, events_area, task, Some(&capture.task_events), view);
}

#[cfg_attr(test, mutants::skip)]
fn block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
}

#[cfg_attr(test, mutants::skip)]
fn fraction(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |value| format!("{:.1}%", value.clamp(0.0, 1.0) * 100.0))
}

#[cfg_attr(test, mutants::skip)]
fn duration(value: Option<u64>) -> String {
    value.map_or_else(|| "-".into(), format_runtime_duration)
}

#[cfg_attr(test, mutants::skip)] // Glyph thresholds are presentation-only terminal formatting.
fn timeline(intervals: &[Interval], window: Option<TimeWindow>, width: usize, color: Color) -> Line<'static> {
    Line::from(
        bins(intervals, window, width)
            .into_iter()
            .map(|value| {
                let glyph = match value {
                    None => "▁",
                    Some(value) if value <= 0.0 => "▁",
                    Some(value) if value < 0.25 => "▂",
                    Some(value) if value < 0.375 => "▃",
                    Some(value) if value < 0.5 => "▄",
                    Some(value) if value < 0.625 => "▅",
                    Some(value) if value < 0.75 => "▆",
                    Some(value) if value < 0.875 => "▇",
                    Some(_) => "█",
                };
                Span::styled(glyph, Style::default().fg(if value.is_some() { color } else { Color::DarkGray }))
            })
            .collect::<Vec<_>>(),
    )
}

#[cfg_attr(test, mutants::skip)]
fn axis(window: Option<TimeWindow>) -> String {
    window.map_or_else(
        || "Window unobserved".into(),
        |window| {
            format!(
                "Window {} · lower bound",
                histogram_duration(window.end.saturating_sub(window.start))
            )
        },
    )
}

#[cfg_attr(test, mutants::skip)] // Worker table clipping and column selection are terminal layout.
fn draw_workers(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    runtime: &RuntimeMonitorSnapshot,
    view: RuntimeViewState,
    filtered: bool,
) {
    if area.is_empty() {
        return;
    }
    let title = format!(
        " Runtime Workers · {} runtime events · source retained {}/{} accepted · {} overwritten · Enter tasks · F1 ",
        runtime.runtime_events, runtime.retained_events, runtime.total_events, runtime.lost_events
    );
    if runtime.workers.is_empty() {
        let lines = if filtered {
            vec![
                "No matching runtime activity.",
                "Press F to change or clear stack filters, including unknown-stack and runtime provenance.",
            ]
        } else if !runtime.source_present && runtime.runtime_events == 0 {
            vec![
                "No runtime source or runtime events in this snapshot.",
                "Use an instrumented runtime (e.g. oxidizer_rt); the recorder alone does not instrument executors.",
            ]
        } else {
            vec![
                "No runtime workers or tasks were recorded.",
                "Enable Runtime tasks and capture instrumented activity.",
            ]
        };
        frame.render_widget(
            Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()).block(block(title, view.focus == RuntimeFocus::Workers)),
            area,
        );
        return;
    }
    let selected = view.worker_selected.min(runtime.workers.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let wide = area.width >= 110;
    let graph_width = usize::from(area.width.saturating_sub(if wide { 93 } else { 67 }));
    let rows = runtime.workers.iter().skip(first).take(visible).map(|worker| {
        let thread = if worker.worker_id.is_none() {
            "unassigned".into()
        } else {
            worker.thread_id.map_or_else(|| "?".into(), |id| format!("#{id}"))
        };
        let name = match worker.worker_id {
            None => format!("unassigned / {}", worker.runtime_name),
            Some(id) => format!("{}:w{id} / {thread} {}", worker.runtime_id, worker.runtime_name),
        };
        let mut cells = vec![Cell::from(name)];
        if wide {
            cells.extend([Cell::from(worker.role.clone()), Cell::from(worker.state.clone())]);
        }
        cells.extend([
            Cell::from(Line::from(worker.observed_tasks.to_string()).right_aligned()),
            Cell::from(Line::from(format_count(worker.metrics.poll_count)).right_aligned()),
            Cell::from(Line::from(fraction(worker.metrics.executing_fraction)).right_aligned()),
            Cell::from(Line::from(duration(worker.metrics.median_poll_nanos)).right_aligned()),
            Cell::from(Line::from(duration(worker.metrics.max_poll_nanos)).right_aligned()),
            Cell::from(timeline(&worker.metrics.polls, worker.window, graph_width, Color::Cyan)),
        ]);
        Row::new(cells)
    });
    let mut widths = vec![Constraint::Length(if wide { 22 } else { 16 })];
    let mut headers = vec!["Runtime / thread"];
    if wide {
        widths.extend([Constraint::Length(9), Constraint::Length(9)]);
        headers.extend(["Role", "State"]);
    }
    widths.extend([5, 6, 10, 11, 11].map(Constraint::Length));
    widths.push(Constraint::Min(1));
    headers.extend(["Tasks", "Polls", "Observed %", "Median poll", "Max poll", "Poll timeline"]);
    let table = Table::new(rows, widths)
        .header(Row::new(headers).bold())
        .column_spacing(1)
        .row_highlight_style(selection(view.focus == RuntimeFocus::Workers))
        .block(block(title, view.focus == RuntimeFocus::Workers));
    let mut state = TableState::default().with_selected(Some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
    mouse.register(
        area,
        1,
        first.saturating_add(state.offset()),
        runtime.workers.len(),
        ListTarget::RuntimeWorkers,
    );
}

#[cfg_attr(test, mutants::skip)]
fn selection(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().bg(Color::DarkGray)
    }
}

#[cfg_attr(test, mutants::skip)]
fn task_row(task: &RuntimeTaskSummary) -> Row<'static> {
    Row::new(
        [
            format!("#{}", task.task_id),
            task.state.clone(),
            task.future_size_bytes.map_or_else(|| "-".into(), |size| size.to_string()),
            format_count(task.metrics.poll_count),
            fraction(task.metrics.executing_fraction),
            duration(task.metrics.median_poll_nanos),
            duration(task.metrics.max_poll_nanos),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let line = Line::from(value);
            Cell::from(if index >= 2 { line.right_aligned() } else { line })
        }),
    )
}

#[cfg_attr(test, mutants::skip)]
fn draw_tasks(frame: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, tasks: &[&RuntimeTaskSummary], view: RuntimeViewState) {
    if area.is_empty() {
        return;
    }
    let selected = view.task_selected.min(tasks.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let title = format!(
        " Tasks · this worker · {} {} · [/] sort · r reverse · Enter details · Backspace workers ",
        view.task_sort.label(),
        if view.task_sort_descending { "desc" } else { "asc" }
    );
    let table = Table::new(
        tasks.iter().skip(first).take(visible).map(|task| task_row(task)),
        [9, 13, 10, 7, 10, 11, 11].map(Constraint::Length),
    )
    .header(
        Row::new(
            [
                "Task",
                "State(global)",
                "Future B",
                "Polls",
                "Observed %",
                "Median poll",
                "Max poll",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, label)| {
                let line = Line::from(label);
                Cell::from(if index >= 2 { line.right_aligned() } else { line })
            }),
        )
        .bold(),
    )
    .column_spacing(1)
    .row_highlight_style(selection(view.focus == RuntimeFocus::Tasks))
    .block(block(title, view.focus == RuntimeFocus::Tasks));
    let mut state = TableState::default().with_selected((!tasks.is_empty()).then_some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
    mouse.register(area, 1, first.saturating_add(state.offset()), tasks.len(), ListTarget::RuntimeTasks);
}

#[cfg_attr(test, mutants::skip)]
fn draw_worker_activity(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    worker: &RuntimeWorkerSummary,
    view: RuntimeViewState,
) {
    if area.is_empty() {
        return;
    }
    let shell = block(" Worker Activity · F1 ", false);
    let inner = shell.inner(area);
    frame.render_widget(shell, area);
    let summary_height = inner.height.saturating_sub(5).min(4);
    let [summary, distribution] = Layout::vertical([Constraint::Length(summary_height), Constraint::Min(0)]).areas(inner);
    let mut lines = vec![Line::from(format!(
        "Polls {} · observed {}",
        worker.metrics.poll_count,
        fraction(worker.metrics.executing_fraction)
    ))];
    if summary_height >= 3 {
        lines.push(Line::from(format!(
            "Median {} · max {}",
            duration(worker.metrics.median_poll_nanos),
            duration(worker.metrics.max_poll_nanos),
        )));
    }
    if summary_height >= 4 {
        lines.push(Line::from(axis(worker.window)));
    }
    lines.push(timeline(
        &worker.metrics.polls,
        worker.window,
        usize::from(inner.width),
        Color::Cyan,
    ));
    frame.render_widget(Paragraph::new(lines), summary);
    draw_histogram(frame, mouse, distribution, "Poll · worker", &worker.metrics.poll_samples, view);
}

#[cfg_attr(test, mutants::skip)]
fn draw_task_details(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    task: Option<&RuntimeTaskSummary>,
    window: Option<TimeWindow>,
    view: RuntimeViewState,
) {
    if area.is_empty() {
        return;
    }
    let Some(task) = task else {
        draw_empty_panel_with_message(frame, area, " Task Statistics ", Some("Select a task."));
        return;
    };
    let shell = block(
        format!(" Task Statistics · #{} · F1 ", task.task_id),
        view.focus == RuntimeFocus::Activity,
    );
    let inner = shell.inner(area);
    frame.render_widget(shell, area);
    let [tabs, content] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Poll", selection(view.task_histogram == TaskHistogram::Poll)),
            Span::raw(" | "),
            Span::styled("Ready", selection(view.task_histogram == TaskHistogram::Ready)),
            Span::raw(" · t"),
        ])),
        tabs,
    );
    for (offset, width, tab) in [(0, 4, TaskHistogram::Poll), (7, 5, TaskHistogram::Ready)] {
        mouse.register_tab(
            Rect::new(tabs.x.saturating_add(offset), tabs.y, width, tabs.height).intersection(tabs),
            ListTarget::RuntimeHistogram(tab),
        );
    }
    draw_task_statistics(frame, mouse, content, task, window, view);
}

#[cfg_attr(test, mutants::skip)] // Compact summary selection is terminal presentation policy.
fn draw_task_statistics(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    inner: Rect,
    task: &RuntimeTaskSummary,
    window: Option<TimeWindow>,
    view: RuntimeViewState,
) {
    let summary_height = inner.height.saturating_sub(5).min(5);
    let [summary, distribution] = Layout::vertical([Constraint::Length(summary_height), Constraint::Min(0)]).areas(inner);
    let activity = &task.activity;
    let age = match (activity.running_for, activity.ready_for) {
        (Some(age), _) | (_, Some(age)) => format!("{} for {}", activity.state, histogram_duration(age)),
        _ => activity.state.clone(),
    };
    let age = if summary_height < 4 { format!("{age} · global") } else { age };
    let worker = activity.poll_worker_id.map_or_else(String::new, |id| format!(" · on w{id}"));
    let mut poll_line = timeline(
        &task.metrics.polls,
        window,
        usize::from(inner.width.saturating_sub(12)),
        Color::Cyan,
    );
    poll_line.spans.insert(0, Span::raw("Poll worker "));
    let mut ready_line = timeline(
        &task.metrics.ready,
        window,
        usize::from(inner.width.saturating_sub(13)),
        Color::Yellow,
    );
    ready_line.spans.insert(0, Span::raw("Ready global "));
    let (title, samples) = match view.task_histogram {
        TaskHistogram::Poll => ("Poll · worker", task.metrics.poll_samples.as_slice()),
        TaskHistogram::Ready => wake_distribution(&task.metrics),
    };
    if title == "Wake-to-poll (raw)" {
        ready_line = Line::from("Includes self-wake running overlap");
    }
    let mut lines = vec![Line::from(Span::styled(
        age,
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ))];
    if summary_height >= 4 {
        lines.push(Line::from(format!(
            "State global{worker}{}",
            if activity.repoll_requested { " · repoll" } else { "" }
        )));
    }
    if summary_height >= 3 {
        lines.push(Line::from(axis(window)));
    }
    if summary_height >= 5 {
        lines.extend([poll_line, ready_line]);
    } else {
        lines.push(match view.task_histogram {
            TaskHistogram::Poll => poll_line,
            TaskHistogram::Ready => ready_line,
        });
    }
    frame.render_widget(Paragraph::new(lines), summary);
    draw_histogram(frame, mouse, distribution, title, samples, view);
}

#[cfg_attr(test, mutants::skip)]
fn wake_distribution(metrics: &ExecutionMetrics) -> (&'static str, &[u64]) {
    if metrics.ready_samples.is_empty() && !metrics.wake_samples.is_empty() {
        ("Wake-to-poll (raw)", &metrics.wake_samples)
    } else {
        ("Ready wait · global", &metrics.ready_samples)
    }
}

#[cfg_attr(test, mutants::skip)] // Histogram labels and plot sizing are defensive terminal layout.
fn draw_histogram(frame: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, title: &str, samples: &[u64], view: RuntimeViewState) {
    if area.is_empty() {
        return;
    }
    let compact = area.height < 7;
    let shell = if compact {
        Block::default()
    } else {
        block(format!(" {title} · n={} ", samples.len()), view.focus == RuntimeFocus::Activity)
    };
    let inner = shell.inner(area);
    frame.render_widget(shell, area);
    if inner.is_empty() {
        return;
    }
    let count = RUNTIME_HISTOGRAM_BINS.min(usize::from(inner.width));
    let mut bins = histogram(samples, count);
    if bins.is_empty() {
        // Keep the fixed duration axis even without observations; discard the axis seed's count.
        bins = histogram(&[0], count);
        bins[0].count = 0;
    }
    let selected = view.activity_scroll.min(bins.len().saturating_sub(1));
    let maximum = bins.iter().map(|bin| bin.count).max().unwrap_or(0);
    let [counts, detail, plot, ticks] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let count_label = if samples.is_empty() {
        "No completed samples".into()
    } else if compact {
        let source = if title == "Wake-to-poll (raw)" { "Raw wake " } else { "" };
        format!("{source}n={} · count 0..{maximum}", samples.len())
    } else {
        format!("Count 0..{maximum}")
    };
    frame.render_widget(Paragraph::new(count_label), counts);
    let bin = bins[selected];
    let range = match bin.upper.checked_add(1) {
        Some(upper) => format!("{}..<{}", histogram_duration(bin.lower), histogram_duration(upper)),
        None => format!("{}+", histogram_duration(bin.lower)),
    };
    frame.render_widget(Paragraph::new(format!("{range}: {}", bin.count)), detail);
    let width = draw_histogram_bars(frame, mouse, plot, &bins, view);
    frame.render_widget(Paragraph::new(histogram_axis(&bins, usize::from(width))), Rect { width, ..ticks });
}

#[cfg_attr(test, mutants::skip)] // Bar widths are terminal rendering arithmetic, not snapshot semantics.
fn draw_histogram_bars(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    bins: &[HistogramBin],
    view: RuntimeViewState,
) -> u16 {
    let count = u16::try_from(bins.len()).unwrap_or(u16::MAX);
    let gap = u16::from(area.width >= count.saturating_mul(2).saturating_sub(1));
    let width = count.saturating_mul(1 + gap).saturating_sub(gap);
    let area = Rect {
        width: width.min(area.width),
        ..area
    };
    let [plot, baseline] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    let selected = view.activity_scroll.min(bins.len().saturating_sub(1));
    let maximum = bins.iter().map(|bin| bin.count).max().unwrap_or(0);
    // BarChart quantizes to eighth-cells; keep nonempty buckets visible without changing reported counts.
    let minimum_visible_count = maximum.div_ceil(u64::from(plot.height.max(1)) * 8);
    let bars = bins
        .iter()
        .enumerate()
        .map(|(index, bin)| {
            let display_count = if bin.count == 0 { 0 } else { bin.count.max(minimum_visible_count) };
            Bar::default()
                .value(display_count)
                .text_value(String::new())
                .style(Style::default().fg(if index == selected && view.focus == RuntimeFocus::Activity {
                    Color::Yellow
                } else {
                    Color::Cyan
                }))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        BarChart::default()
            .data(BarGroup::default().bars(&bars))
            .bar_width(1)
            .bar_gap(gap)
            .max(maximum.max(1)),
        plot,
    );
    if !baseline.is_empty() {
        for index in 0..count {
            let x = baseline.x + index * (1 + gap);
            let style = Style::default()
                .fg(Color::Gray)
                .bg(if usize::from(index) == selected && view.focus == RuntimeFocus::Activity {
                    Color::DarkGray
                } else {
                    Color::Reset
                });
            frame.buffer_mut()[(x, baseline.y)].set_symbol("_").set_style(style);
        }
    }
    mouse.register_columns(area, 1, gap, bins.len(), ListTarget::RuntimeActivity);
    width
}

#[cfg_attr(test, mutants::skip)]
fn histogram_axis(bins: &[HistogramBin], width: usize) -> String {
    let first = histogram_duration(bins[0].lower);
    let last = format!("{}+", histogram_duration(bins[bins.len() - 1].lower));
    let padding = width.saturating_sub(first.len() + last.len());
    if padding >= 7 {
        format!("{first}{}log10 {last}", " ".repeat(padding - 6))
    } else {
        "log10 ns".into()
    }
}

#[cfg_attr(test, mutants::skip)]
fn histogram_duration(nanos: u64) -> String {
    if nanos >= 1_000_000_000_000 {
        format!("{:.2e}s", std::time::Duration::from_nanos(nanos).as_secs_f64())
    } else if let Some((unit, suffix)) = [(1_000_000_000, "s"), (1_000_000, "ms"), (1_000, "us")]
        .into_iter()
        .find(|(unit, _)| nanos >= *unit && nanos.is_multiple_of(*unit))
    {
        format!("{}{suffix}", nanos / unit)
    } else {
        format_runtime_duration(nanos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_durations_and_observations_are_not_zero() {
        assert_eq!(
            (
                duration(None),
                duration(Some(0)),
                fraction(None),
                timeline(&[], None, 4, Color::Cyan).to_string()
            ),
            ("-".into(), "0ns".into(), "-".into(), "▁▁▁▁".into())
        );
    }

    #[test]
    fn window_axis_labels_the_compact_observed_span() {
        assert_eq!(axis(Some(TimeWindow { start: 100, end: 200 })), "Window 100ns · lower bound");
        assert_eq!(axis(None), "Window unobserved");
    }

    #[test]
    fn timeline_marks_unobserved_bins_with_a_dim_baseline() {
        let line = timeline(
            &[Interval { start: 0, end: 50 }],
            Some(TimeWindow { start: 0, end: 100 }),
            2,
            Color::Cyan,
        );
        assert_eq!(line.to_string(), "█▁");
        assert_eq!(line.spans[1].style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn timeline_distinguishes_measured_zero_from_unobserved_baseline() {
        let line = timeline(
            &[Interval { start: 25, end: 25 }],
            Some(TimeWindow { start: 0, end: 100 }),
            2,
            Color::Cyan,
        );
        assert_eq!(line.to_string(), "▁▁");
        assert_eq!(
            (line.spans[0].style.fg, line.spans[1].style.fg),
            (Some(Color::Cyan), Some(Color::DarkGray))
        );
    }

    #[test]
    fn timeline_uses_eight_bar_heights() {
        let steps = (0..8)
            .map(|step| {
                timeline(
                    &[Interval {
                        start: 0,
                        end: if step == 0 { 0 } else { 2 * step + 1 },
                    }],
                    Some(TimeWindow { start: 0, end: 16 }),
                    1,
                    Color::Cyan,
                )
                .to_string()
            })
            .collect::<String>();
        assert_eq!(steps, "▁▂▃▄▅▆▇█");
    }

    #[test]
    fn timeline_keeps_any_positive_activity_above_the_baseline() {
        assert_eq!(
            timeline(
                &[Interval { start: 0, end: 1 }],
                Some(TimeWindow { start: 0, end: u64::MAX }),
                1,
                Color::Cyan,
            )
            .to_string(),
            "▂"
        );
    }

    #[test]
    fn observed_fraction_is_bounded_and_distinguishes_missing_from_zero() {
        assert_eq!(
            [None, Some(0.0), Some(0.5), Some(1.0), Some(1.5)].map(fraction),
            ["-", "0.0%", "50.0%", "100.0%", "100.0%"].map(String::from)
        );
    }

    #[test]
    fn legacy_wake_histogram_is_populated_and_clearly_not_ready_queue_time() {
        let task = RuntimeTaskSummary {
            task_id: 1,
            activity: super::super::super::runtime::TaskActivitySummary {
                state: "Ready".into(),
                ready_for: Some(10),
                ..Default::default()
            },
            metrics: ExecutionMetrics {
                wake_samples: vec![10, 100].into(),
                ..ExecutionMetrics::default()
            },
            ..RuntimeTaskSummary::default()
        };
        let mut view = super::super::super::app::App::new().runtime_view;
        view.task_histogram = TaskHistogram::Ready;
        let (buffer, _) = render(40, 24, |frame, mouse, area| {
            draw_task_details(frame, mouse, area, Some(&task), None, view);
        });
        let text = text(&buffer);
        assert!(text.contains("Wake-to-poll (raw)"));
        assert!(text.contains("self-wake running overlap"));
        assert!(text.contains("Ready for 10ns"));
        assert!(text.contains("n=2"));
        assert!(!text.contains("Ready wait"));
    }

    #[test]
    fn coherent_queue_histogram_never_mixes_raw_wake_samples() {
        let metrics = ExecutionMetrics {
            ready_samples: vec![5].into(),
            wake_samples: vec![100].into(),
            ..ExecutionMetrics::default()
        };
        assert_eq!(wake_distribution(&metrics), ("Ready wait · global", [5].as_slice()));
    }

    #[test]
    fn histogram_labels_fit_columns_for_extreme_durations() {
        for nanos in [0, 1, 999, 1_000, 1_000_000, 1_000_000_000, u64::MAX] {
            assert!(histogram_duration(nanos).len() <= 9);
        }
        assert_eq!(histogram_duration(0), "0ns");
        assert_eq!(histogram_duration(u64::MAX), "1.84e10s");
    }

    #[test]
    fn histogram_bars_stay_one_cell_wide_and_mouse_excludes_gaps() {
        let view = super::super::super::app::App::new().runtime_view;
        let (buffer, mouse) = render(80, 14, |frame, mouse, area| {
            draw_histogram(frame, mouse, area, "Poll", &[10, 100, 100, 100, 100], view);
        });
        let baseline = baseline(&buffer);
        assert_eq!(baseline.len(), 12);
        for (index, (x, y)) in baseline.iter().copied().enumerate() {
            assert_eq!(x, 1 + 2 * u16::try_from(index).unwrap());
            assert_eq!(mouse.at(buffer.area, x, y), Some((ListTarget::RuntimeActivity, index)));
            assert_eq!(mouse.at(buffer.area, x, y - 1), Some((ListTarget::RuntimeActivity, index)));
            assert_eq!(mouse.at(buffer.area, x + 1, y), None);
            assert_eq!(buffer[(x + 1, y)].symbol(), " ");
            assert_eq!(buffer[(x + 1, y - 1)].symbol(), " ");
        }
        let top = (0..buffer.area.height)
            .find(|y| mouse.at(buffer.area, baseline[0].0, *y).is_some())
            .unwrap();
        assert_eq!(buffer[(baseline[1].0, top)].symbol(), " ");
        assert_eq!(buffer[(baseline[2].0, top)].symbol(), "█");
    }

    #[test]
    fn histogram_keeps_twelve_bins_then_removes_gaps_before_merging() {
        let view = super::super::super::app::App::new().runtime_view;
        for (width, expected_bins, stride) in [(40, 12, 2), (25, 12, 2), (24, 12, 1), (14, 12, 1), (13, 11, 1)] {
            let (buffer, mouse) = render(width, 12, |frame, mouse, area| {
                draw_histogram(frame, mouse, area, "Poll", &[0, 1, u64::MAX], view);
            });
            let baseline = baseline(&buffer);
            assert_eq!(baseline.len(), expected_bins);
            for (index, (x, y)) in baseline.into_iter().enumerate() {
                assert_eq!(x, 1 + stride * u16::try_from(index).unwrap());
                assert_eq!(mouse.at(buffer.area, x, y), Some((ListTarget::RuntimeActivity, index)));
            }
        }
    }

    #[test]
    fn histogram_nonempty_buckets_keep_a_visible_sliver_without_inflating_counts() {
        let samples = std::iter::repeat_n(100, 10_000).chain([10]).collect::<Vec<_>>();
        for height in [7, 9, 14] {
            let mut view = super::super::super::app::App::new().runtime_view;
            view.focus = RuntimeFocus::Activity;
            view.activity_scroll = 1;
            let (buffer, _) = render(40, height, |frame, mouse, area| {
                draw_histogram(frame, mouse, area, "Poll", &samples, view);
            });
            let baseline = baseline(&buffer);
            assert_eq!(baseline.len(), 12);
            for (x, y) in &baseline {
                assert_eq!(buffer[(*x, *y)].fg, Color::Gray);
            }
            let (zero, bottom) = baseline[0];
            let (tiny, _) = baseline[1];
            assert_eq!(buffer[(zero, bottom - 1)].symbol(), " ");
            assert_eq!(buffer[(tiny, bottom - 1)].symbol(), "▁");
            assert_eq!(buffer[(tiny, bottom - 1)].fg, Color::Yellow);
            assert_eq!(buffer[(tiny, bottom)].bg, Color::DarkGray);
            let text = text(&buffer);
            assert!(text.contains("Count 0..10000"));
            assert!(text.contains("10ns..<100ns: 1"));
        }
    }

    #[test]
    fn histogram_empty_samples_have_gray_baselines_but_no_measured_zero() {
        let view = super::super::super::app::App::new().runtime_view;
        let (empty, mouse) = render(25, 9, |frame, mouse, area| draw_histogram(frame, mouse, area, "Poll", &[], view));
        let (zero, _) = render(25, 9, |frame, mouse, area| draw_histogram(frame, mouse, area, "Poll", &[0], view));
        let baseline = baseline(&empty);
        assert_eq!(baseline.len(), 12);
        for (index, (x, y)) in baseline.iter().copied().enumerate() {
            assert_eq!((empty[(x, y)].symbol(), empty[(x, y)].fg), ("_", Color::Gray));
            assert_eq!(empty[(x, y - 1)].symbol(), " ");
            assert_eq!(mouse.at(empty.area, x, y), Some((ListTarget::RuntimeActivity, index)));
        }
        let (x, y) = baseline[0];
        assert_eq!(zero[(x, y - 1)].symbol(), "█");
        assert!(text(&empty).contains("No completed samples"));
        assert!(!text(&zero).contains("No completed samples"));
    }

    #[test]
    fn histogram_clamps_selection_and_keeps_exact_compact_range_and_axis() {
        let mut view = super::super::super::app::App::new().runtime_view;
        view.focus = RuntimeFocus::Activity;
        view.activity_scroll = usize::MAX;
        let (buffer, mouse) = render(25, 9, |frame, mouse, area| {
            draw_histogram(frame, mouse, area, "Poll", &[u64::MAX], view);
        });
        let (x, y) = baseline(&buffer)[11];
        assert_eq!(mouse.at(buffer.area, x, y), Some((ListTarget::RuntimeActivity, 11)));
        assert_eq!((buffer[(x, y - 1)].fg, buffer[(x, y)].bg), (Color::Yellow, Color::DarkGray));
        let text = text(&buffer);
        assert!(text.contains("100s+: 1"));
        assert!(text.contains("0ns         log10 100s+"));
    }

    #[test]
    fn task_histogram_toggle_changes_samples_and_registers_only_tab_labels() {
        let task = RuntimeTaskSummary {
            task_id: 7,
            metrics: ExecutionMetrics {
                poll_samples: vec![10, 10, 10],
                ready_samples: vec![1_000].into(),
                wake_samples: vec![100, 100].into(),
                ..ExecutionMetrics::default()
            },
            ..RuntimeTaskSummary::default()
        };
        for (selected, heading, count, filled) in [
            (TaskHistogram::Poll, "Poll · worker", 3, 1),
            (TaskHistogram::Ready, "Ready wait · global", 1, 3),
        ] {
            let mut view = super::super::super::app::App::new().runtime_view;
            view.task_histogram = selected;
            let (buffer, mouse) = render(40, 24, |frame, mouse, area| {
                draw_task_details(frame, mouse, area, Some(&task), None, view);
            });
            assert!(text(&buffer).contains(&format!("{heading} · n={count}")));
            assert!(!text(&buffer).contains("Wake-to-poll"));
            let baseline = baseline(&buffer);
            for (index, (x, y)) in baseline.into_iter().enumerate() {
                assert_eq!(buffer[(x, y - 1)].symbol(), if index == filled { "█" } else { " " });
            }
            for x in [1, 4] {
                assert_eq!(
                    mouse.at(buffer.area, x, 1),
                    Some((ListTarget::RuntimeHistogram(TaskHistogram::Poll), 0))
                );
            }
            for x in [8, 12] {
                assert_eq!(
                    mouse.at(buffer.area, x, 1),
                    Some((ListTarget::RuntimeHistogram(TaskHistogram::Ready), 0))
                );
            }
            for x in [0, 5, 6, 7, 13, 14, 15] {
                assert_eq!(mouse.at(buffer.area, x, 1), None);
            }

            let (compact, _) = render(40, 11, |frame, mouse, area| {
                draw_task_details(frame, mouse, area, Some(&task), None, view);
            });
            assert!(text(&compact).contains(if selected == TaskHistogram::Poll {
                "Poll worker"
            } else {
                "Ready global"
            }));
        }
    }

    #[test]
    fn short_activity_rows_keep_both_histograms_above_the_baseline() {
        let view = super::super::super::app::App::new().runtime_view;
        let metrics = ExecutionMetrics {
            poll_samples: vec![10],
            ..ExecutionMetrics::default()
        };
        let worker = RuntimeWorkerSummary {
            metrics: metrics.clone(),
            ..RuntimeWorkerSummary::default()
        };
        let task = RuntimeTaskSummary {
            metrics,
            ..RuntimeTaskSummary::default()
        };
        for height in [10, 12, 14, 16] {
            let (worker, _) = render(40, height, |frame, mouse, area| {
                draw_worker_activity(frame, mouse, area, &worker, view);
            });
            let (task, _) = render(40, height, |frame, mouse, area| {
                draw_task_details(frame, mouse, area, Some(&task), None, view);
            });
            for buffer in [worker, task] {
                let baseline = baseline(&buffer);
                assert_eq!(baseline.len(), 12);
                let (x, y) = baseline[1];
                assert_eq!((buffer[(x, y)].fg, buffer[(x, y - 1)].symbol()), (Color::Gray, "█"));
            }
        }
    }

    #[test]
    fn compact_charts_and_tabs_accept_zero_and_tiny_rectangles() {
        let mut view = super::super::super::app::App::new().runtime_view;
        view.activity_scroll = usize::MAX;
        let task = RuntimeTaskSummary::default();
        for width in 0..5 {
            for height in 0..8 {
                for samples in [&[][..], &[0, u64::MAX][..]] {
                    let area = Rect::new(2, 2, width, height);
                    render(10, 12, |frame, mouse, _| draw_histogram(frame, mouse, area, "Poll", samples, view));
                    render(10, 12, |frame, mouse, _| {
                        draw_task_details(frame, mouse, area, Some(&task), None, view);
                    });
                }
            }
        }
    }

    fn render(
        width: u16,
        height: u16,
        draw: impl FnOnce(&mut ratatui::Frame<'_>, &MouseRows, Rect),
    ) -> (ratatui::buffer::Buffer, MouseRows) {
        let mouse = MouseRows::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                mouse.begin(area);
                draw(frame, &mouse, area);
            })
            .unwrap();
        (terminal.backend().buffer().clone(), mouse)
    }

    fn text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer.content.iter().map(ratatui::buffer::Cell::symbol).collect()
    }

    fn baseline(buffer: &ratatui::buffer::Buffer) -> Vec<(u16, u16)> {
        (buffer.area.top()..buffer.area.bottom())
            .flat_map(|y| {
                (buffer.area.left()..buffer.area.right())
                    .filter(move |x| buffer[(*x, y)].symbol() == "_")
                    .map(move |x| (x, y))
            })
            .collect()
    }
}
