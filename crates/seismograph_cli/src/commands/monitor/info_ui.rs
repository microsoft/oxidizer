// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Live event-class rates and graphical per-thread recorder occupancy.

use std::collections::VecDeque;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, Gauge, GraphType, Paragraph};

use super::super::app::ActivitySample;
use super::super::live_activity::{Availability, CLASS_LABELS, LiveActivity, ThreadActivity};
use super::super::mouse::{ListTarget, MouseRows};
use super::format_count;

const CLASS_COLORS: [Color; 6] = [
    Color::Cyan,
    Color::Green,
    Color::Magenta,
    Color::Yellow,
    Color::LightBlue,
    Color::LightRed,
];

// Explicit RGB avoids low-contrast remapping by the terminal's ANSI palette.
const THREAD_BAR_TRACK: Color = Color::Rgb(12, 16, 22);
const THREAD_BAR_FILL: Color = Color::Rgb(70, 205, 255);
const THREAD_BAR_FULL: Color = Color::Rgb(255, 128, 128);

pub(super) fn draw_activity(frame: &mut ratatui::Frame<'_>, area: Rect, live: &LiveActivity, aggregate: &VecDeque<ActivitySample>) {
    if live.availability != Availability::Supported {
        super::draw_activity(frame, area, aggregate);
        return;
    }
    let title = if live.stale {
        " Event-class rates · stale "
    } else {
        " Live event-class rates · events/s "
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let legend = legend(live, usize::from(inner.width));
    let [labels, plot] = Layout::vertical([
        Constraint::Length(u16::try_from(legend.len()).unwrap_or(u16::MAX)),
        Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(legend), labels);
    let Some(first) = live.samples.front() else {
        frame.render_widget(
            Paragraph::new("Collecting rate baseline...").style(Style::default().fg(Color::DarkGray)),
            plot,
        );
        return;
    };
    let points: [Vec<(f64, f64)>; 6] = std::array::from_fn(|class| {
        live.samples
            .iter()
            .map(|sample| {
                (
                    sample.captured_at.saturating_duration_since(first.captured_at).as_secs_f64(),
                    chart_value(sample.rates[class]),
                )
            })
            .collect()
    });
    let maximum = live.samples.iter().flat_map(|sample| sample.rates).max().unwrap_or(0).max(1);
    let max_x = points[0].last().map_or(0.0, |(time, _)| *time);
    let min_x = if max_x > 0.0 { 0.0 } else { -1.0 };
    let datasets = points
        .iter()
        .enumerate()
        .map(|(index, points)| {
            Dataset::default()
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(CLASS_COLORS[index]))
                .data(points)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Chart::new(datasets)
            .x_axis(Axis::default().bounds([min_x, max_x]).labels([
                Span::raw(format!("-{:.1}s", max_x - min_x)),
                Span::raw(if live.stale { "last sample" } else { "now" }),
            ]))
            .y_axis(
                Axis::default()
                    .bounds([0.0, chart_value(maximum)])
                    .labels([Span::raw("0"), Span::raw(format_count(maximum))]),
            ),
        plot,
    );
}

fn legend(live: &LiveActivity, width: usize) -> Vec<Line<'static>> {
    let rates = (!live.stale).then(|| live.samples.back().map(|sample| sample.rates)).flatten();
    let mut lines = Vec::new();
    let mut row = Line::default();
    for (index, label) in CLASS_LABELS.iter().enumerate() {
        let rate = rates.map_or_else(|| "-".to_owned(), |rates| format_count(rates[index]));
        let span = Span::styled(
            format!("{label} {rate}/s  "),
            Style::default().fg(CLASS_COLORS[index]).add_modifier(Modifier::BOLD),
        );
        if row.width() > 0 && row.width() + span.width() > width {
            lines.push(std::mem::take(&mut row));
        }
        row.spans.push(span);
    }
    lines.push(row);
    lines
}

pub(super) fn draw_threads(frame: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, live: &LiveActivity, selected: usize) {
    let title = if live.stale {
        " Thread activity · stale "
    } else {
        " Thread activity · live "
    };
    let shell = Block::default().borders(Borders::ALL).title(title);
    let inner = shell.inner(area);
    let rows = usize::from(inner.height);
    let selected = selected.min(live.threads.len().saturating_sub(1));
    let first = selected.saturating_sub(rows.saturating_sub(1));
    let footer = format!(
        " {}/{} threads · Up/Down · PgUp/Dn ",
        selected.saturating_add(1).min(live.threads.len()),
        live.threads.len()
    );
    frame.render_widget(shell.title_bottom(footer), area);
    let empty = match live.availability {
        Availability::Waiting => Some("Waiting for live recorder counters..."),
        Availability::Unsupported => Some("Thread/class counters unavailable on this server.\nRebuild the application to enable them."),
        Availability::Supported if live.threads.is_empty() => Some("No recording threads in this session."),
        Availability::Supported => None,
    };
    if let Some(message) = empty {
        frame.render_widget(Paragraph::new(message), inner);
        return;
    }
    for (offset, thread) in live.threads.iter().skip(first).take(rows).enumerate() {
        let index = first + offset;
        let row = Rect::new(inner.x, inner.y + u16::try_from(offset).unwrap_or(u16::MAX), inner.width, 1);
        draw_thread(frame, row, thread, index == selected, live.stale);
        mouse.register_item(row, ListTarget::InfoThreads, index);
    }
}

fn draw_thread(frame: &mut ratatui::Frame<'_>, area: Rect, thread: &ThreadActivity, selected: bool, stale: bool) {
    let stats = &thread.statistics;
    let rate = if stale { None } else { thread.rate };
    let rate = rate.map_or_else(|| "-/s".to_owned(), |rate| format!("{}/s", format_count(rate)));
    let [name, fill, current, history] = Layout::horizontal([
        Constraint::Percentage(28),
        Constraint::Percentage(34),
        Constraint::Length(u16::try_from(rate.len()).unwrap_or(u16::MAX).max(7)),
        Constraint::Min(0),
    ])
    .spacing(1)
    .areas(area);
    let selection_style = if selected {
        Style::default().bg(Color::DarkGray)
    } else {
        Style::default()
    };
    frame.render_widget(
        Paragraph::new(format!(
            "#{} {}{}",
            stats.thread_id,
            stats.name,
            if stats.retired { " (exited)" } else { "" }
        ))
        .style(selection_style),
        name,
    );
    frame.render_widget(Paragraph::new(rate).style(selection_style.fg(Color::Cyan)), current);
    if stats.event_capacity == 0 {
        frame.render_widget(Paragraph::new("ring released").style(Style::default().fg(Color::DarkGray)), fill);
    } else {
        let ratio = (chart_value(stats.retained_events) / chart_value(stats.event_capacity)).clamp(0.0, 1.0);
        let color = if stats.retained_events >= stats.event_capacity {
            THREAD_BAR_FULL
        } else {
            THREAD_BAR_FILL
        };
        let percent = format!("{:.0}%", ratio * 100.0);
        let counts = format!(
            "{}/{} {percent}",
            format_count(stats.retained_events),
            format_count(stats.event_capacity)
        );
        let label = if counts.len() <= usize::from(fill.width) { counts } else { percent };
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .label(label)
                .gauge_style(Style::default().fg(color).bg(THREAD_BAR_TRACK)),
            fill,
        );
    }
    frame.render_widget(Paragraph::new(rate_sparkline(&thread.history, usize::from(history.width))), history);
}

fn rate_sparkline(history: &VecDeque<u64>, width: usize) -> Line<'static> {
    let maximum = history.iter().rev().take(width).copied().max().unwrap_or(0).max(1);
    let glyphs = ["_", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let first = history.len().saturating_sub(width);
    let spans = history.iter().skip(first).map(|rate| {
        let level = if *rate == 0 {
            0
        } else {
            (u128::from(*rate) * 8).div_ceil(u128::from(maximum))
        };
        let index = usize::try_from(level).unwrap_or(8).min(8);
        Span::styled(
            glyphs[index],
            Style::default().fg(if *rate == 0 { Color::DarkGray } else { Color::Cyan }),
        )
    });
    Line::from(spans.collect::<Vec<_>>())
}

#[expect(
    clippy::cast_precision_loss,
    reason = "chart coordinates and fill ratios are approximate; displayed counters remain exact u64 values"
)]
fn chart_value(value: u64) -> f64 {
    value as f64
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use seismograph_protocol::message::{EventClassCounts, RecorderActivity, ThreadRecorderStatistics};

    use super::*;

    fn live() -> LiveActivity {
        let mut live = LiveActivity::default();
        let start = Instant::now();
        for count in [1, 2] {
            live.record(
                RecorderActivity {
                    session_id: 1,
                    class_events: EventClassCounts {
                        allocations: count * 3,
                        general_events: count * 5,
                        ..EventClassCounts::default()
                    },
                    threads: (1..=20)
                        .map(|thread_id| ThreadRecorderStatistics {
                            thread_id,
                            name: format!("worker-{thread_id}"),
                            total_events: count * 32,
                            retained_events: count * 32,
                            event_capacity: 64,
                            ..ThreadRecorderStatistics::default()
                        })
                        .collect(),
                    ..RecorderActivity::default()
                },
                start + Duration::from_secs(count),
            );
        }
        live
    }

    #[test]
    fn class_legend_has_exact_current_rates_and_distinct_colors() {
        let live = live();
        let lines = legend(&live, 180);
        let spans = lines.iter().flat_map(|line| &line.spans).collect::<Vec<_>>();
        assert_eq!(spans.iter().map(|span| span.style.fg).collect::<Vec<_>>(), CLASS_COLORS.map(Some));
        let text = lines.iter().map(ToString::to_string).collect::<String>();
        assert!(text.contains("Allocations 3/s") && text.contains("General 5/s") && text.contains("Runtime 0/s"));
        assert!(legend(&live, 40).len() > 1);
    }

    #[test]
    fn thread_rows_show_fill_and_rate_graphics_and_click_exact_scrolled_identity() {
        let live = live();
        let mouse = MouseRows::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| {
                mouse.begin(frame.area());
                draw_threads(frame, &mouse, frame.area(), &live, 19);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer.content.iter().map(ratatui::buffer::Cell::symbol).collect::<String>();
        assert!(text.contains("worker-20") && text.contains("64/64") && text.contains("100%") && text.contains("32/s"));
        assert!(text.contains('█'));
        assert_eq!(mouse.at(buffer.area, 2, 1), Some((ListTarget::InfoThreads, 10)));
        assert_eq!(mouse.at(buffer.area, 2, 9), Some((ListTarget::InfoThreads, 18)));
        assert_eq!(mouse.at(buffer.area, 2, 10), Some((ListTarget::InfoThreads, 19)));
        assert_eq!(mouse.at(buffer.area, 2, 11), None);
        let last_row = (0..80).map(|x| buffer[(x, 10)].symbol()).collect::<String>();
        for part in ["worker-20", "64/64", "100%", "32/s", "█"] {
            assert!(last_row.contains(part), "{last_row}");
        }
    }

    #[test]
    fn thread_bars_and_inverted_labels_keep_high_contrast_at_all_fill_levels() {
        let luminance = |color| {
            let Color::Rgb(red, green, blue) = color else {
                panic!("thread bars must use explicit RGB colors")
            };
            let linear = |channel| {
                let value = f64::from(channel) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.0722_f64.mul_add(linear(blue), 0.7152_f64.mul_add(linear(green), 0.2126 * linear(red)))
        };
        for color in [THREAD_BAR_FILL, THREAD_BAR_FULL] {
            let contrast = (luminance(color) + 0.05) / (luminance(THREAD_BAR_TRACK) + 0.05);
            assert!(contrast >= 7.0, "bar and text contrast is only {contrast:.2}:1");
        }
        for retained in [0, 32, 64] {
            let mut thread = live().threads.remove(0);
            thread.statistics.retained_events = retained;
            let color = if retained == 64 { THREAD_BAR_FULL } else { THREAD_BAR_FILL };
            let mut terminal = Terminal::new(TestBackend::new(80, 1)).unwrap();
            terminal
                .draw(|frame| draw_thread(frame, frame.area(), &thread, true, false))
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert!(buffer.content.iter().any(|cell| cell.fg == color && cell.bg == THREAD_BAR_TRACK));
            if retained > 0 {
                assert!(buffer.content.iter().any(|cell| cell.fg == THREAD_BAR_TRACK && cell.bg == color));
            }
        }
    }

    #[test]
    fn zero_sparkline_is_dim_and_any_positive_rate_remains_visible() {
        let line = rate_sparkline(&VecDeque::from([0, 1, u64::MAX]), 3);
        assert_eq!(line.to_string(), "_▁█");
        assert_eq!(line.spans[0].style.fg, Some(Color::DarkGray));
    }

    #[test]
    fn unavailable_stale_and_small_layouts_do_not_invent_rates() {
        let mut live = live();
        live.stale = true;
        assert!(legend(&live, 180).iter().all(|line| !line.to_string().contains("3/s")));
        for (width, height) in [(0, 0), (1, 1), (20, 8), (80, 24)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw_activity(frame, frame.area(), &live, &VecDeque::new()))
                .unwrap();
            let mouse = MouseRows::default();
            terminal
                .draw(|frame| {
                    mouse.begin(frame.area());
                    draw_threads(frame, &mouse, frame.area(), &live, usize::MAX);
                })
                .unwrap();
        }
    }
}
