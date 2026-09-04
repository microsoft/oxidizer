// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::time::Duration;

use chrono::{DateTime, Local};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Axis, Block, Borders, Cell, Chart, Clear, Dataset, Gauge, GraphType, List, ListItem, ListState, Paragraph, Row, Table, TableState, Tabs,
};
use seismograph_protocol::message::{EventBufferDisposition, RecorderStatistics, RecordingConfiguration, RecordingPolicy};
use seismograph_protocol::monitor::MonitorDescriptor;

use super::app::{
    ActivitySample, AllocationViewState, App, CaptureStep, HeapFocus, HeapViewState, MonitorTab, PrimitiveFocus, PrimitiveViewState,
    RecordingConfigurationField, RecordingConfigurationPopup, RuntimeDetailView, RuntimeFocus, RuntimeViewState, Screen, ThreadFocus,
    ThreadViewState, format_sampling_percentage,
};
use super::data::{
    AllocationHotspot, AllocationSnapshot, AllocationSort, AllocationStackFilter, CapturedSnapshot, MemorySnapshot, MemoryTier,
    PrimitiveSnapshot, PrimitiveSort, ThreadSnapshot,
};

const KEY_COLOR: Color = Color::Cyan;
const CONTENTION_COLOR: Color = Color::Yellow;

impl App {
    pub(super) fn draw(&self, frame: &mut ratatui::Frame<'_>) {
        let [body, footer] = Layout::vertical([Constraint::Min(4), Constraint::Length(1)]).areas(frame.area());
        match &self.screen {
            Screen::Browse => self.draw_browser(frame, body),
            Screen::Connected {
                descriptor,
                recording,
                tab,
                snapshot,
            } => draw_connected(
                frame,
                body,
                &ConnectedView {
                    descriptor,
                    recording: *recording,
                    tab: *tab,
                    snapshot: snapshot.as_deref(),
                    snapshot_error: self.snapshot_error.as_deref(),
                    heap_view: self.heap_view,
                    allocation_view: self.allocation_view,
                    primitive_view: self.primitive_view,
                    thread_view: self.thread_view,
                    runtime_view: self.runtime_view,
                    activity_samples: &self.activity_samples,
                    recorder_statistics: self.recorder_statistics.as_ref(),
                },
            ),
        }
        let line = match &self.screen {
            Screen::Browse => browse_footer(&self.status),
            Screen::Connected { recording, snapshot, .. } => {
                connected_footer(*recording, self.snapshot_options.event_buffers, snapshot.as_deref(), &self.status)
            }
        };
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(Color::DarkGray)), footer);
        if let (Some(started_at), Some(step)) = (self.capture_started_at, self.capture_step) {
            Self::draw_capture_popup(frame, started_at.elapsed(), step);
        }
        if let Some(popup) = self.recording_configuration_popup {
            Self::draw_recording_configuration_popup(frame, popup);
        }
    }

    fn draw_browser(&self, frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect) {
        let items = self.instances.iter().map(|instance| {
            let label = instance.descriptor.instance.as_deref().map_or_else(
                || instance.descriptor.name.clone(),
                |name| format!("{} ({name})", instance.descriptor.name),
            );
            ListItem::new(format!(
                "{label}  pid={}  {}",
                instance.descriptor.process_id,
                recording_configuration_label(instance.recording)
            ))
        });
        let list = List::new(items)
            .block(Block::default().title(" Seismograph applications ").borders(Borders::ALL))
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
            .highlight_symbol("> ");
        let mut state = ListState::default().with_selected((!self.instances.is_empty()).then_some(self.selected));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_capture_popup(frame: &mut ratatui::Frame<'_>, elapsed: Duration, active_step: CaptureStep) {
        const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
        let area = frame.area();
        let width = area.width.min(60);
        let height = area.height.min(9);
        let popup_area = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let frame_index = usize::try_from(elapsed.as_millis() / 150).unwrap_or(usize::MAX) % SPINNER.len();
        frame.render_widget(Clear, popup_area);
        let block = Block::default()
            .title(format!(" Snapshot · {:.1}s ", elapsed.as_secs_f64()))
            .borders(Borders::ALL);
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);
        let [steps_area, progress_area] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(inner);
        let steps = CaptureStep::ALL.into_iter().map(|step| {
            let (marker, style) = if step.index() < active_step.index() {
                ("✓", Style::default().fg(Color::Green))
            } else if step == active_step {
                (SPINNER[frame_index], Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            } else {
                ("·", Style::default().fg(Color::DarkGray))
            };
            Line::from(vec![
                Span::raw("  "),
                Span::styled(marker, style),
                Span::raw("  "),
                Span::styled(step.label(), style),
            ])
        });
        frame.render_widget(Paragraph::new(steps.collect::<Vec<_>>()), steps_area);
        frame.render_widget(
            Gauge::default()
                .ratio(active_step.progress())
                .label(format!("Phase {} of {}", active_step.index() + 1, CaptureStep::ALL.len()))
                .gauge_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            progress_area,
        );
    }

    fn draw_recording_configuration_popup(frame: &mut ratatui::Frame<'_>, popup: RecordingConfigurationPopup) {
        let area = frame.area();
        let width = area.width.min(72);
        let desired_height = u16::try_from(RecordingConfigurationField::ALL.len() + 2).unwrap_or(u16::MAX);
        let height = area.height.saturating_sub(2).min(desired_height).max(3);
        let popup_area = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let items = RecordingConfigurationField::ALL.into_iter().map(|field| {
            let value = field.value(popup.draft);
            if value.is_empty() {
                ListItem::new(format!("  {}", field.label()))
            } else {
                ListItem::new(format!("{:<32} {value:>32}", field.label()))
            }
        });
        let list = List::new(items)
            .block(
                Block::default()
                    .title(" Recording configuration ")
                    .title_bottom(" ↑/↓ field · ←/→ change · Space toggle · Enter select · Esc cancel ")
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
            .highlight_symbol("> ");
        let mut state = ListState::default().with_selected(Some(popup.selected));
        frame.render_widget(Clear, popup_area);
        frame.render_stateful_widget(list, popup_area, &mut state);
    }
}

#[derive(Clone, Copy)]
struct ConnectedView<'a> {
    descriptor: &'a MonitorDescriptor,
    recording: RecordingConfiguration,
    tab: MonitorTab,
    snapshot: Option<&'a CapturedSnapshot>,
    snapshot_error: Option<&'a str>,
    heap_view: HeapViewState,
    allocation_view: AllocationViewState,
    primitive_view: PrimitiveViewState,
    thread_view: ThreadViewState,
    runtime_view: RuntimeViewState,
    activity_samples: &'a VecDeque<ActivitySample>,
    recorder_statistics: Option<&'a RecorderStatistics>,
}

fn draw_connected(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, view: &ConnectedView<'_>) {
    let [tabs, content] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    frame.render_widget(
        Tabs::new([" Info ", " Heaps ", " Allocations ", " Primitives ", " Threads ", " Runtime "])
            .select(view.tab.index())
            .block(Block::default().borders(Borders::ALL))
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
            .divider("│"),
        tabs,
    );
    match view.tab {
        MonitorTab::Info => draw_info(
            frame,
            content,
            view.descriptor,
            view.recording,
            view.snapshot.and_then(|capture| capture.allocations.as_ref()),
            view.activity_samples,
            view.recorder_statistics,
        ),
        MonitorTab::Heaps => draw_memory(
            frame,
            content,
            view.snapshot.and_then(|capture| capture.memory.as_ref()),
            view.snapshot
                .and_then(|capture| capture.heap_error.as_deref())
                .or(view.snapshot_error),
            view.heap_view,
        ),
        MonitorTab::Allocations => draw_allocations(
            frame,
            content,
            view.snapshot.and_then(|capture| capture.allocations.as_ref()),
            view.snapshot
                .and_then(|capture| capture.heap_error.as_deref())
                .or(view.snapshot_error),
            view.allocation_view,
        ),
        MonitorTab::Primitives => {
            draw_primitives(
                frame,
                content,
                view.snapshot.map(|snapshot| &snapshot.primitives),
                view.snapshot_error,
                view.primitive_view,
            );
        }
        MonitorTab::Threads => draw_threads(
            frame,
            content,
            view.snapshot.map(|snapshot| &snapshot.threads),
            view.snapshot_error,
            view.thread_view,
        ),
        MonitorTab::Runtime => draw_runtime(
            frame,
            content,
            view.snapshot.map(|snapshot| &snapshot.runtime),
            view.snapshot_error,
            view.runtime_view,
        ),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the three coordinated runtime panes share selection and layout state that is clearest in one renderer"
)]
fn draw_runtime(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    runtime: Option<&super::data::RuntimeMonitorSnapshot>,
    unavailable: Option<&str>,
    view: RuntimeViewState,
) {
    let [workers_area, lower_area] = Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);
    let [tasks_area, details_area] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(lower_area);
    let Some(runtime) = runtime else {
        draw_empty_panel_with_message(frame, workers_area, " Runtime Threads ", unavailable);
        draw_empty_panel_with_message(frame, tasks_area, " Tasks ", unavailable);
        draw_empty_panel_with_message(frame, details_area, " Task Details ", unavailable);
        return;
    };

    let worker_selected = view.worker_selected.min(runtime.workers.len().saturating_sub(1));
    let worker = runtime.workers.get(worker_selected);
    let visible_workers = usize::from(workers_area.height.saturating_sub(3));
    let first_worker = worker_selected.saturating_sub(visible_workers.saturating_sub(1));
    let mut worker_lines = vec![Line::from(Span::styled(
        format!(
            "{:<18} {:<9} {:<9} {:>7} {:>10} {:>10} {:>10}",
            "Runtime / thread", "Role", "State", "Tasks", "Poll busy", "Avg poll", "Max poll"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    worker_lines.extend(
        runtime
            .workers
            .iter()
            .skip(first_worker)
            .take(visible_workers)
            .enumerate()
            .map(|(index, worker)| {
                let thread = worker.thread_id.map_or_else(|| "-".into(), |id| format!("#{id}"));
                primitive_selection_line(
                    Line::from(format!(
                        "{:<18} {:<9} {:<9} {:>7} {:>9.1}% {:>10} {:>10}",
                        format!("{} / {thread}", worker.runtime_name),
                        worker.role,
                        worker.state,
                        format_count(u64::try_from(worker.tasks.len()).unwrap_or(u64::MAX)),
                        worker.average_running_tasks * 100.0,
                        format_runtime_duration(worker.average_poll_nanos),
                        format_runtime_duration(worker.max_poll_nanos),
                    )),
                    first_worker + index == worker_selected,
                    view.focus == RuntimeFocus::Workers,
                )
            }),
    );
    frame.render_widget(
        Paragraph::new(worker_lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(format!(
                        " Runtime Threads · retained {} / {} events · {} lost ({}) · Poll busy = retained-window task polls · ",
                        format_count(runtime.retained_events),
                        format_count(runtime.total_events),
                        format_count(runtime.lost_events),
                        format_event_loss(runtime.lost_events, runtime.total_events),
                    )),
                    key_span("Enter"),
                    Span::raw(" tasks "),
                ]))
                .borders(Borders::ALL),
        ),
        workers_area,
    );

    let sorted_tasks = worker.map_or_else(Vec::new, |worker| worker.sorted_tasks(view.task_sort, view.task_sort_descending));
    let task_selected = view.task_selected.min(sorted_tasks.len().saturating_sub(1));
    let visible_tasks = usize::from(tasks_area.height.saturating_sub(3));
    let first_task = task_selected.saturating_sub(visible_tasks.saturating_sub(1));
    let mut task_lines = vec![Line::from(Span::styled(
        format!(
            "{:<10} {:<10} {:<7} {:>7} {:>10} {:>10} {:>10} {:>10}",
            "Task", "State", "Scope", "Polls", "Avg resume", "Max resume", "Avg stall", "Max stall"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    task_lines.extend(
        sorted_tasks
            .iter()
            .skip(first_task)
            .take(visible_tasks)
            .enumerate()
            .map(|(index, task)| {
                primitive_selection_line(
                    Line::from(format!(
                        "{:<10} {:<10} {:<7} {:>7} {:>10} {:>10} {:>10} {:>10}",
                        format!("#{}", task.task_id),
                        task.state,
                        task.metric_scope.label(),
                        format_count(task.poll_count),
                        format_runtime_duration(task.average_resume_nanos),
                        format_runtime_duration(task.max_resume_nanos),
                        format_runtime_duration(task.average_ready_wait_nanos),
                        format_runtime_duration(task.max_ready_wait_nanos),
                    )),
                    first_task + index == task_selected,
                    view.focus == RuntimeFocus::Tasks,
                )
            }),
    );
    frame.render_widget(
        Paragraph::new(task_lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(format!(
                        " Tasks · sort: {} {} · ",
                        view.task_sort.label(),
                        if view.task_sort_descending { "desc" } else { "asc" }
                    )),
                    key_span("[/]"),
                    Span::raw(" sort · "),
                    key_span("r"),
                    Span::raw(" reverse · "),
                    key_span("Enter"),
                    Span::raw(" details · "),
                    key_span("Backspace"),
                    Span::raw(" threads "),
                ]))
                .borders(Borders::ALL),
        ),
        tasks_area,
    );

    let task = sorted_tasks.get(task_selected).copied();
    let mut detail_lines = Vec::new();
    if let Some(task) = task {
        match view.detail_view {
            RuntimeDetailView::Details => {
                detail_lines.extend([
                    Line::from(format!("Task: #{}", task.task_id)),
                    Line::from(format!("State: {}", task.state)),
                    Line::from(format!("Metric scope: {}", task.metric_scope.label())),
                    Line::from(match task.metric_scope {
                        super::data::RuntimeTaskMetricScope::Lifetime => {
                            "  Poll, resume, and scheduler-stall values are lifetime counters."
                        }
                        super::data::RuntimeTaskMetricScope::RetainedWindow => {
                            "  Poll, resume, and scheduler-stall values cover retained events only."
                        }
                    }),
                    Line::from(format!("Runtime: #{}", task.runtime_id)),
                    Line::from(format!(
                        "Parent: {}",
                        task.parent_id.map_or_else(|| "-".into(), |id| format!("#{id}"))
                    )),
                    Line::from(format!(
                        "Type descriptor: {}",
                        task.type_descriptor_id.map_or_else(|| "-".into(), |id| format!("#{id}"))
                    )),
                    Line::from(format!("Workers: {:?}", task.worker_ids)),
                    Line::from(format!("Polls: {}", format_count(task.poll_count))),
                    Line::from(format!("Total poll time: {}", format_runtime_duration(task.poll_nanos))),
                    Line::from(format!(
                        "Average poll duration: {}",
                        format_runtime_duration(task.average_poll_nanos)
                    )),
                    Line::from(format!("Maximum poll duration: {}", format_runtime_duration(task.max_poll_nanos))),
                    Line::from(format!("Resume samples: {}", format_count(task.resume_count))),
                    Line::from(format!(
                        "Average time between polls: {}",
                        format_runtime_duration(task.average_resume_nanos)
                    )),
                    Line::from(format!(
                        "Maximum time between polls: {}",
                        format_runtime_duration(task.max_resume_nanos)
                    )),
                    Line::from("  Time between polls is measured from a poll finishing until the next poll starts."),
                    Line::from(format!("Ready-wait samples: {}", format_count(task.ready_wait_count))),
                    Line::from(format!("Total scheduler stall: {}", format_runtime_duration(task.ready_wait_nanos))),
                    Line::from(format!(
                        "Average scheduler stall: {}",
                        format_runtime_duration(task.average_ready_wait_nanos)
                    )),
                    Line::from(format!(
                        "Maximum scheduler stall: {}",
                        format_runtime_duration(task.max_ready_wait_nanos)
                    )),
                    Line::from("  Scheduler stall is measured from the first wake until the task is polled."),
                    Line::from(format!("Retained-window enqueues: {}", format_count(task.enqueue_count))),
                    Line::from(format!(
                        "Retained-window materializations: {}",
                        format_count(task.materialization_count)
                    )),
                    Line::from(format!("Retained-window transfer events: {}", format_count(task.transfer_count))),
                    Line::from(format!(
                        "Lifetime: {}",
                        task.spawned_at
                            .zip(task.completed_at)
                            .map_or_else(|| "-".into(), |(start, end)| format_runtime_duration(end.saturating_sub(start)))
                    )),
                ]);
            }
            RuntimeDetailView::SpawnStack if task.spawn_stack.is_empty() => {
                detail_lines.push(Line::from("Backtrace not captured."));
            }
            RuntimeDetailView::SpawnStack => {
                detail_lines.extend(
                    task.spawn_stack
                        .iter()
                        .enumerate()
                        .map(|(index, frame)| Line::from(format!("{index:>3}  {frame}"))),
                );
            }
        }
    }
    let visible_details = usize::from(details_area.height.saturating_sub(2));
    let detail_scroll = view.detail_scroll.min(detail_lines.len().saturating_sub(visible_details));
    frame.render_widget(
        Paragraph::new(
            detail_lines
                .into_iter()
                .skip(detail_scroll)
                .take(visible_details)
                .collect::<Vec<_>>(),
        )
        .block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(format!(" Task {} · ", view.detail_view.label())),
                    key_span("Tab"),
                    Span::raw(" toggle view · "),
                    key_span("PgUp/PgDn"),
                    Span::raw(" scroll · "),
                    key_span("Backspace"),
                    Span::raw(" tasks "),
                ]))
                .borders(Borders::ALL),
        ),
        details_area,
    );
}

fn format_runtime_duration(nanos: u64) -> String {
    if nanos >= 1_000_000_000 {
        format_decimal_duration(nanos, 1_000_000_000, "s")
    } else if nanos >= 1_000_000 {
        format_decimal_duration(nanos, 1_000_000, "ms")
    } else if nanos >= 1_000 {
        format_decimal_duration(nanos, 1_000, "us")
    } else {
        format!("{nanos}ns")
    }
}

fn format_decimal_duration(nanos: u64, unit_nanos: u64, suffix: &str) -> String {
    let whole = nanos / unit_nanos;
    let hundredths = nanos % unit_nanos * 100 / unit_nanos;
    format!("{whole}.{hundredths:02}{suffix}")
}

fn format_event_loss(lost_events: u64, total_events: u64) -> String {
    if total_events == 0 {
        return "0.0%".into();
    }
    let tenths = u128::from(lost_events) * 1_000 / u128::from(total_events);
    format!("{}.{:01}%", tenths / 10, tenths % 10)
}

fn draw_allocations(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    allocations: Option<&AllocationSnapshot>,
    unavailable: Option<&str>,
    view: AllocationViewState,
) {
    const COUNT_WIDTH: usize = 13;
    const BYTES_WIDTH: usize = 12;
    let [hotspots_area, stack_area] = Layout::vertical([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(area);
    let Some(allocations) = allocations else {
        draw_empty_panel_with_message(frame, hotspots_area, " Allocation Hotspots ", unavailable);
        draw_empty_panel_with_message(frame, stack_area, " Stack Trace ", unavailable);
        return;
    };
    let hotspots = allocations.sorted_hotspots(view.sort, view.descending);
    let selected = view.selected.min(hotspots.len().saturating_sub(1));
    let visible_hotspots = usize::from(hotspots_area.height.saturating_sub(3));
    let first_hotspot = selected.saturating_sub(visible_hotspots.saturating_sub(1));
    let heading = |label, width, column| {
        let style = if view.sort == column {
            key_style()
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        Span::styled(format!("{label:>width$}"), style)
    };
    let mut lines = vec![Line::from(vec![
        heading("Allocations", COUNT_WIDTH, AllocationSort::Allocations),
        Span::raw(" "),
        heading("Allocated", BYTES_WIDTH, AllocationSort::AllocatedBytes),
        Span::raw(" "),
        heading("Average", BYTES_WIDTH, AllocationSort::AverageBytes),
        Span::raw(" "),
        heading("Live", COUNT_WIDTH, AllocationSort::LiveAllocations),
        Span::raw(" "),
        heading("Live bytes", BYTES_WIDTH, AllocationSort::LiveBytes),
        Span::styled("  Location", Style::default().add_modifier(Modifier::BOLD)),
    ])];
    lines.extend(
        hotspots
            .iter()
            .skip(first_hotspot)
            .take(visible_hotspots)
            .enumerate()
            .map(|(index, hotspot)| {
                let hotspot_index = first_hotspot + index;
                let line = Line::from(format!(
                    "{count:>COUNT_WIDTH$} {bytes:>BYTES_WIDTH$} {average:>BYTES_WIDTH$} \
                     {live:>COUNT_WIDTH$} {live_bytes:>BYTES_WIDTH$}  {location}",
                    count = format_count(hotspot.allocations),
                    bytes = format_bytes(hotspot.allocated_bytes),
                    average = format_bytes(hotspot.allocated_bytes.checked_div(hotspot.allocations).unwrap_or_default()),
                    live = format_count(hotspot.live_allocations),
                    live_bytes = format_bytes(hotspot.live_bytes),
                    location = hotspot.location(view.stack_filter),
                ));
                if hotspot_index == selected {
                    line.style(Style::default().fg(Color::Black).bg(Color::Cyan))
                } else {
                    line
                }
            }),
    );
    if allocations.hotspots.is_empty() {
        lines.push(Line::from(
            "No allocation stacks captured. Enable recording and backtraces, then take a new snapshot.",
        ));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Hotspots • sort "),
                    key_span("["),
                    Span::raw(" previous • "),
                    key_span("]"),
                    Span::raw(" next • "),
                    key_span("[r]"),
                    Span::raw(" reverse • select "),
                    key_span("↑/↓"),
                    Span::raw(format!(
                        " • {} events • {} lost ",
                        format_count(allocations.total_events),
                        format_count(allocations.lost_events)
                    )),
                ]))
                .borders(Borders::ALL),
        ),
        hotspots_area,
    );
    let Some(hotspot) = hotspots.get(selected) else {
        draw_empty_panel(frame, stack_area, " Stack Trace ");
        return;
    };
    draw_allocation_stack(frame, stack_area, hotspot, view);
}

fn draw_allocation_stack(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    hotspot: &AllocationHotspot,
    view: AllocationViewState,
) {
    let hotspot_stack = hotspot.stack(view.stack_filter);
    let visible_lines = usize::from(area.height.saturating_sub(2));
    let max_scroll = hotspot_stack.len().saturating_sub(visible_lines);
    let stack_scroll = view.stack_scroll.min(max_scroll);
    let stack = if hotspot_stack.is_empty() {
        vec![Line::from("Backtraces were not captured for this hotspot.")]
    } else {
        hotspot_stack
            .iter()
            .skip(stack_scroll)
            .take(visible_lines)
            .enumerate()
            .map(|(index, stack_frame)| Line::from(format!("{:>3}  {stack_frame}", stack_scroll + index)))
            .collect()
    };
    frame.render_widget(
        Paragraph::new(stack).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Stack Trace • "),
                    key_span("[f]"),
                    Span::raw(format!(
                        " {} • ",
                        match view.stack_filter {
                            AllocationStackFilter::Application => "application frames",
                            AllocationStackFilter::All => "all frames",
                        }
                    )),
                    key_span("PgUp/PgDn"),
                    Span::raw(format!(" scroll • {} ", hotspot.location(view.stack_filter))),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_info(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    descriptor: &MonitorDescriptor,
    recording: RecordingConfiguration,
    allocations: Option<&AllocationSnapshot>,
    activity_samples: &VecDeque<ActivitySample>,
    recorder_statistics: Option<&RecorderStatistics>,
) {
    let [activity_area, details_area] = Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
    draw_activity(frame, activity_area, activity_samples);
    let event_capacity = u64::from(recording.event_capacity_per_thread);
    let capacity = usize::try_from(recording.event_capacity_per_thread)
        .ok()
        .and_then(seismograph::recorder::EventBufferCapacity::new);
    let memory_per_thread = capacity
        .map(seismograph::recorder::EventBufferCapacity::memory_bytes_per_thread)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .unwrap_or(u64::MAX);
    let mut lines = vec![
        metric_line("Name", descriptor.name.clone()),
        metric_line("Instance", descriptor.instance.clone().unwrap_or_else(|| "-".into())),
        metric_line("PID", descriptor.process_id.to_string()),
        metric_line("Monitor port", descriptor.port.to_string()),
        Line::from(""),
        metric_line("Event ring buffer", format!("{} events / thread", format_count(event_capacity))),
        recording_policy_line("Allocations", recording.allocations),
        recording_policy_line("General events", recording.general_events),
        recording_policy_line("Arc dereferences", recording.arc_dereferences),
        recording_policy_line("Runtime tasks", recording.runtime_tasks),
        recording_policy_line("I/O", recording.io),
        recording_policy_line("Cache", recording.cache),
        metric_line("Telemetry memory / thread", format_bytes(memory_per_thread)),
    ];
    if let Some(statistics) = recorder_statistics {
        lines.extend([
            metric_line("Telemetry threads", format_count(statistics.thread_count)),
            metric_line("Telemetry memory total", format_bytes(statistics.allocated_bytes)),
            metric_line("Total telemetry events", format_count(statistics.total_events)),
            metric_line("Retained telemetry events", format_count(statistics.retained_events)),
            metric_line("Overwritten telemetry events", format_count(statistics.lost_events)),
        ]);
    } else if let Some(allocations) = allocations {
        lines.extend([
            metric_line("Telemetry threads", format_count(allocations.thread_count)),
            metric_line(
                "Telemetry memory total",
                format_bytes(memory_per_thread.saturating_mul(allocations.thread_count)),
            ),
            metric_line("Total telemetry events", format_count(allocations.total_events)),
            metric_line("Retained allocator events", format_count(allocations.retained_events)),
            metric_line("Overwritten telemetry events", format_count(allocations.lost_events)),
        ]);
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::default().title(" Info ").borders(Borders::ALL)),
        details_area,
    );
}

fn draw_activity(frame: &mut ratatui::Frame<'_>, area: Rect, samples: &VecDeque<ActivitySample>) {
    if samples.is_empty() {
        frame.render_widget(
            Paragraph::new("Waiting for the first one-second activity sample...")
                .block(Block::default().title(" Live Activity · events / second ").borders(Borders::ALL)),
            area,
        );
        return;
    }
    let points = samples
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            (
                f64::from(index),
                f64::from(u32::try_from(sample.events_per_second).unwrap_or(u32::MAX)),
            )
        })
        .collect::<Vec<_>>();
    let maximum = points.iter().map(|(_, value)| *value).fold(1.0, f64::max);
    let maximum_label = samples.iter().map(|sample| sample.events_per_second).max().unwrap_or(0);
    let current = samples.back().map_or(0, |sample| sample.events_per_second);
    let total = samples.back().map_or(0, |sample| sample.total_events);
    let max_x = f64::from(u32::try_from(points.len().saturating_sub(1).max(1)).unwrap_or(u32::MAX));
    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&points);
    let chart = Chart::new(vec![dataset])
        .block(
            Block::default()
                .title(format!(
                    " Live Activity · {} events/s · {} total · one-second samples ",
                    format_count(current),
                    format_count(total)
                ))
                .borders(Borders::ALL),
        )
        .x_axis(Axis::default().bounds([0.0, max_x]))
        .y_axis(
            Axis::default()
                .bounds([0.0, maximum])
                .labels([Span::raw("0"), Span::raw(format_count(maximum_label))]),
        );
    frame.render_widget(chart, area);
}

fn draw_primitives(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    primitives: Option<&PrimitiveSnapshot>,
    unavailable: Option<&str>,
    view: PrimitiveViewState,
) {
    let [types_area, operations_area, details_area] =
        Layout::vertical([Constraint::Length(9), Constraint::Length(10), Constraint::Min(8)]).areas(area);
    let [hotspots_area, stack_area] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(details_area);
    let Some(primitives) = primitives else {
        draw_empty_panel_with_message(frame, types_area, " Primitive Types ", unavailable);
        draw_empty_panel_with_message(frame, operations_area, " Operations ", unavailable);
        draw_empty_panel_with_message(frame, hotspots_area, " Hotspots ", unavailable);
        draw_empty_panel_with_message(frame, stack_area, " Stack Trace ", unavailable);
        return;
    };
    draw_primitive_types(frame, types_area, primitives, view);
    let group = primitives
        .groups
        .get(view.primitive_selected.min(primitives.groups.len().saturating_sub(1)));
    let operations = group.map(|group| group.sorted_operations(view.sort, view.descending));
    draw_primitive_operations(frame, operations_area, operations.as_deref(), view);
    let operation = operations
        .as_ref()
        .and_then(|operations| operations.get(view.operation_selected.min(operations.len().saturating_sub(1))))
        .copied();
    draw_primitive_hotspots(frame, hotspots_area, operation, view);
    draw_primitive_stack(frame, stack_area, operation, view);
}

fn draw_primitive_types(frame: &mut ratatui::Frame<'_>, area: Rect, primitives: &PrimitiveSnapshot, view: PrimitiveViewState) {
    const TYPE_WIDTH: usize = 19;

    let visible = usize::from(area.height.saturating_sub(3));
    let selected = view.primitive_selected.min(primitives.groups.len().saturating_sub(1));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<TYPE_WIDTH$} {:>14} {:>14} {:>14}", "Type", "Events", "Objects", "Contentions"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(
        primitives
            .groups
            .iter()
            .skip(first)
            .take(visible)
            .enumerate()
            .map(|(index, group)| {
                primitive_selection_line(
                    Line::from(vec![
                        Span::raw(format!(
                            "{:<TYPE_WIDTH$} {:>14} {:>14} ",
                            group.kind.label(),
                            format_count(group.events),
                            format_count(group.objects),
                        )),
                        Span::styled(
                            format!("{:>14}", format_count(group.contentions)),
                            Style::default().fg(CONTENTION_COLOR),
                        ),
                    ]),
                    first + index == selected,
                    view.focus == PrimitiveFocus::Types,
                )
            }),
    );
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Primitive Types · "),
                    key_span("↑/↓"),
                    Span::raw(" select · "),
                    key_span("Enter"),
                    Span::raw(format!(
                        " details · {} primitive / {} total retained · {} lost ",
                        format_count(primitives.groups.iter().map(|group| group.events).sum()),
                        format_count(primitives.total_events.saturating_sub(primitives.lost_events)),
                        format_count(primitives.lost_events)
                    )),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_primitive_operations(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    operations: Option<&[&super::data::PrimitiveOperation]>,
    view: PrimitiveViewState,
) {
    const OPERATION_WIDTH: usize = 28;

    let heading = |label: &str, sort| {
        if view.sort == sort {
            format!("{label} {}", if view.descending { "↓" } else { "↑" })
        } else {
            label.to_owned()
        }
    };
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{:<OPERATION_WIDTH$} {:>14} {:>12} {:>12} {:>12}",
            "Operation",
            heading("Events", PrimitiveSort::Events),
            heading("Objects", PrimitiveSort::Objects),
            heading("Threads", PrimitiveSort::Threads),
            heading("Hotspots", PrimitiveSort::Hotspots),
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if let Some(operations) = operations {
        lines.extend(operations.iter().enumerate().map(|(index, operation)| {
            let line = Line::from(format!(
                "{:<OPERATION_WIDTH$} {:>14} {:>12} {:>12} {:>12}",
                operation.kind.label(),
                format_count(operation.events),
                format_count(operation.objects),
                format_count(operation.threads),
                format_count(u64::try_from(operation.hotspots.len()).unwrap_or(u64::MAX)),
            ));
            let line = if operation.kind.is_contention() {
                line.style(Style::default().fg(CONTENTION_COLOR))
            } else {
                line
            };
            primitive_selection_line(line, index == view.operation_selected, view.focus == PrimitiveFocus::Operations)
        }));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Operations · "),
                    key_span("Enter"),
                    Span::raw(" hotspots · "),
                    key_span("["),
                    Span::raw(" / "),
                    key_span("]"),
                    Span::raw(" sort · "),
                    key_span("[r]"),
                    Span::raw(" reverse · "),
                    key_span("Backspace"),
                    Span::raw(" up "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_primitive_hotspots(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    operation: Option<&super::data::PrimitiveOperation>,
    view: PrimitiveViewState,
) {
    let visible = usize::from(area.height.saturating_sub(3));
    let selected = operation.map_or(0, |operation| view.hotspot_selected.min(operation.hotspots.len().saturating_sub(1)));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!("{:>12}  Location", "Events"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if let Some(operation) = operation {
        lines.extend(
            operation
                .hotspots
                .iter()
                .skip(first)
                .take(visible)
                .enumerate()
                .map(|(index, hotspot)| {
                    primitive_selection_line(
                        Line::from(format!(
                            "{:>12}  {}",
                            format_count(hotspot.count),
                            hotspot.location(view.stack_filter)
                        )),
                        first + index == selected,
                        view.focus == PrimitiveFocus::Hotspots,
                    )
                }),
        );
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Hotspots · "),
                    key_span("↑/↓"),
                    Span::raw(" select · "),
                    key_span("[f]"),
                    Span::raw(" application/all frames · "),
                    key_span("Backspace"),
                    Span::raw(" up "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_primitive_stack(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    operation: Option<&super::data::PrimitiveOperation>,
    view: PrimitiveViewState,
) {
    let hotspot = operation.and_then(|operation| {
        operation
            .hotspots
            .get(view.hotspot_selected.min(operation.hotspots.len().saturating_sub(1)))
    });
    let Some(hotspot) = hotspot else {
        draw_empty_panel(frame, area, " Stack Trace ");
        return;
    };
    let stack = hotspot.stack(view.stack_filter);
    let visible = usize::from(area.height.saturating_sub(2));
    let scroll = view.stack_scroll.min(stack.len().saturating_sub(visible));
    let lines = if stack.is_empty() {
        vec![Line::from("Backtraces were not captured for this hotspot.")]
    } else {
        stack
            .iter()
            .skip(scroll)
            .take(visible)
            .enumerate()
            .map(|(index, frame)| Line::from(format!("{:>3}  {frame}", scroll + index)))
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Stack Trace · "),
                    key_span("PgUp/PgDn"),
                    Span::raw(format!(" scroll · {} ", hotspot.location(view.stack_filter))),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_threads(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    threads: Option<&ThreadSnapshot>,
    unavailable: Option<&str>,
    view: ThreadViewState,
) {
    let [top_area, bottom_area] = Layout::vertical([Constraint::Percentage(42), Constraint::Percentage(58)]).areas(area);
    let [threads_area, operations_area] = Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)]).areas(top_area);
    let [participants_area, objects_area, stack_area] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(30), Constraint::Percentage(40)]).areas(bottom_area);
    let Some(threads) = threads else {
        draw_empty_panel_with_message(frame, threads_area, " Threads ", unavailable);
        draw_empty_panel_with_message(frame, operations_area, " Operations ", unavailable);
        draw_empty_panel_with_message(frame, participants_area, " Related Threads ", unavailable);
        draw_empty_panel_with_message(frame, objects_area, " Objects ", unavailable);
        draw_empty_panel_with_message(frame, stack_area, " Stack Trace ", unavailable);
        return;
    };
    draw_thread_list(frame, threads_area, threads, view);
    let thread = threads
        .threads
        .get(view.thread_selected.min(threads.threads.len().saturating_sub(1)));
    draw_thread_operations(frame, operations_area, thread, view);
    let operation = thread.and_then(|thread| {
        thread
            .operations
            .get(view.operation_selected.min(thread.operations.len().saturating_sub(1)))
    });
    draw_thread_participants(frame, participants_area, operation, view);
    let participant = operation.and_then(|operation| {
        operation
            .participants
            .get(view.participant_selected.min(operation.participants.len().saturating_sub(1)))
    });
    draw_thread_objects(frame, objects_area, participant, view);
    let object = participant.and_then(|participant| {
        participant
            .objects
            .get(view.object_selected.min(participant.objects.len().saturating_sub(1)))
    });
    draw_thread_stack(frame, stack_area, object, view);
}

fn draw_thread_list(frame: &mut ratatui::Frame<'_>, area: Rect, threads: &ThreadSnapshot, view: ThreadViewState) {
    let selected = view.thread_selected.min(threads.threads.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<18} {:>9} {:>9}", "Thread", "Retained", "Lost"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(threads.threads.iter().skip(first).take(visible).enumerate().map(|(index, thread)| {
        primitive_selection_line(
            Line::from(format!(
                "{:<18} {:>9} {:>9}",
                thread_label(thread.thread_id, &thread.name, 18),
                format_count(thread.retained_events),
                format_count(thread.lost_events),
            )),
            first + index == selected,
            view.focus == ThreadFocus::Threads,
        )
    }));
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Threads · "),
                    key_span("↑/↓"),
                    Span::raw(" select · "),
                    key_span("Enter"),
                    Span::raw(" operations · sorted by recorder ID "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_thread_operations(frame: &mut ratatui::Frame<'_>, area: Rect, thread: Option<&super::data::ThreadSummary>, view: ThreadViewState) {
    let selected = thread.map_or(0, |thread| view.operation_selected.min(thread.operations.len().saturating_sub(1)));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<28} {:>10} {:>10} {:>13}", "Operation", "Events", "Objects", "Other threads"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(
        thread
            .into_iter()
            .flat_map(|thread| &thread.operations)
            .skip(first)
            .take(visible)
            .enumerate()
            .map(|(index, operation)| {
                let line = Line::from(format!(
                    "{:<28} {:>10} {:>10} {:>13}",
                    operation.kind.label(),
                    format_count(operation.events),
                    format_count(operation.objects),
                    format_count(u64::try_from(operation.participants.len()).unwrap_or(u64::MAX)),
                ));
                let line = if operation.kind.is_contention() {
                    line.style(Style::default().fg(CONTENTION_COLOR))
                } else {
                    line
                };
                primitive_selection_line(line, first + index == selected, view.focus == ThreadFocus::Operations)
            }),
    );
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Operations · "),
                    key_span("↑/↓"),
                    Span::raw(" select · "),
                    key_span("Enter"),
                    Span::raw(" related threads · "),
                    key_span("Backspace"),
                    Span::raw(" threads "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_thread_participants(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    operation: Option<&super::data::ThreadOperation>,
    view: ThreadViewState,
) {
    let Some(operation) = operation else {
        draw_empty_panel(frame, area, " Related Threads ");
        return;
    };
    let selected = view.participant_selected.min(operation.participants.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<14} {:>7} {:>7}", "Thread", "Objects", "Events"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(
        operation
            .participants
            .iter()
            .skip(first)
            .take(visible)
            .enumerate()
            .map(|(index, participant)| {
                primitive_selection_line(
                    Line::from(format!(
                        "{:<14} {:>7} {:>7}",
                        thread_label(participant.thread_id, &participant.name, 14),
                        format_count(u64::try_from(participant.objects.len()).unwrap_or(u64::MAX)),
                        format_count(participant.events),
                    )),
                    first + index == selected,
                    view.focus == ThreadFocus::Participants,
                )
            }),
    );
    if operation.participants.is_empty() {
        lines.push(Line::from("No other retained thread activity for these objects."));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(format!(" {} · ", operation.kind.relationship_label())),
                    key_span("Enter"),
                    Span::raw(" objects · "),
                    key_span("Backspace"),
                    Span::raw(" operations "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_thread_objects(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    participant: Option<&super::data::ThreadParticipant>,
    view: ThreadViewState,
) {
    let Some(participant) = participant else {
        draw_empty_panel(frame, area, " Objects ");
        return;
    };
    let selected = view.object_selected.min(participant.objects.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<16} {:>6} {:>6}", "Object", "Own", "Other"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(
        participant
            .objects
            .iter()
            .skip(first)
            .take(visible)
            .enumerate()
            .map(|(index, object)| {
                primitive_selection_line(
                    Line::from(format!(
                        "{:016x} {:>6} {:>6}",
                        object.object_id,
                        format_count(object.selected_events),
                        format_count(object.related_events),
                    )),
                    first + index == selected,
                    view.focus == ThreadFocus::Objects,
                )
            }),
    );
    if participant.objects.is_empty() {
        lines.push(Line::from("No shared retained objects."));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Objects · ranked by hotness · "),
                    key_span("↑/↓"),
                    Span::raw(" select · "),
                    key_span("Backspace"),
                    Span::raw(" threads "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_thread_stack(frame: &mut ratatui::Frame<'_>, area: Rect, object: Option<&super::data::ThreadObject>, view: ThreadViewState) {
    let Some(object) = object else {
        draw_empty_panel(frame, area, " Stack Trace ");
        return;
    };
    let mut lines = Vec::new();
    append_thread_stack(&mut lines, "Selected thread operation", object.selected_stack(), view.stack_filter);
    lines.push(Line::from(""));
    append_thread_stack(&mut lines, "Related thread operation", object.related_stack(), view.stack_filter);
    let visible = usize::from(area.height.saturating_sub(2));
    let scroll = view.stack_scroll.min(lines.len().saturating_sub(visible));
    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(scroll).take(visible).collect::<Vec<_>>()).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(format!(" Stack Trace · object 0x{:016x} · ", object.object_id)),
                    key_span("[f]"),
                    Span::raw(" application/all · "),
                    key_span("PgUp/PgDn"),
                    Span::raw(" scroll "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn append_thread_stack(
    lines: &mut Vec<Line<'static>>,
    label: &'static str,
    stack: Option<&super::data::ThreadStack>,
    filter: AllocationStackFilter,
) {
    let Some(stack) = stack else {
        lines.push(Line::from(Span::styled(label, Style::default().add_modifier(Modifier::BOLD))));
        lines.push(Line::from("  Backtraces were not captured."));
        return;
    };
    lines.push(Line::from(Span::styled(
        format!("{label} · {} event(s)", format_count(stack.count)),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let frames = stack.stack(filter);
    if frames.is_empty() {
        lines.push(Line::from("  Backtraces were not captured."));
    } else {
        lines.extend(
            frames
                .iter()
                .enumerate()
                .map(|(index, frame)| Line::from(format!("  {index:>3}  {frame}"))),
        );
    }
}

fn thread_label(thread_id: u64, name: &str, width: usize) -> String {
    let label = if name.is_empty() {
        format!("#{thread_id}")
    } else {
        format!("#{thread_id} {name}")
    };
    label.chars().take(width).collect()
}

fn primitive_selection_line(line: Line<'static>, selected: bool, focused: bool) -> Line<'static> {
    if !selected {
        return line;
    }
    if focused {
        line.style(Style::default().fg(Color::Black).bg(Color::Cyan))
    } else {
        line.style(Style::default().bg(Color::DarkGray))
    }
}

fn draw_memory(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    memory: Option<&MemorySnapshot>,
    unavailable: Option<&str>,
    view: HeapViewState,
) {
    let [summary_area, tiers_area, details_area] =
        Layout::vertical([Constraint::Length(6), Constraint::Length(3), Constraint::Min(8)]).areas(area);
    let [buckets_area, side_area] = Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(details_area);
    let [hotspots_area, stack_area] = Layout::vertical([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(side_area);
    let Some(memory) = memory else {
        draw_empty_panel_with_message(frame, summary_area, " Heap Summary ", unavailable);
        draw_empty_panel_with_message(frame, tiers_area, " Allocation Tiers ", unavailable);
        draw_empty_panel_with_message(frame, buckets_area, " Size Distribution ", unavailable);
        draw_empty_panel_with_message(frame, hotspots_area, " Allocation Locations ", unavailable);
        draw_empty_panel_with_message(frame, stack_area, " Stack Trace ", unavailable);
        return;
    };
    draw_memory_summary(frame, summary_area, memory);
    frame.render_widget(
        Tabs::new([" Small ", " Medium ", " Large / Direct (inferred) "])
            .select(view.tier.index())
            .block(
                Block::default()
                    .title(Line::from(vec![
                        Span::raw(" Allocation Tiers · "),
                        key_span("["),
                        Span::raw(" previous · "),
                        key_span("]"),
                        Span::raw(" next "),
                    ]))
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
            .divider("│"),
        tiers_area,
    );
    let tier = memory.tiers.iter().find(|tier| tier.kind == view.tier);
    draw_memory_buckets(frame, buckets_area, tier, memory, view);
    let bucket = tier.and_then(|tier| tier.buckets.get(view.bucket_selected.min(tier.buckets.len().saturating_sub(1))));
    draw_memory_hotspots(frame, hotspots_area, bucket, view);
    let hotspot = bucket.and_then(|bucket| {
        bucket
            .hotspots
            .get(view.hotspot_selected.min(bucket.hotspots.len().saturating_sub(1)))
    });
    draw_memory_stack(frame, stack_area, hotspot, view);
}

fn draw_empty_panel(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, title: &'static str) {
    draw_empty_panel_with_message(frame, area, title, None);
}

fn draw_empty_panel_with_message(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    title: &'static str,
    unavailable: Option<&str>,
) {
    let line = unavailable.map_or_else(
        || Line::from(vec![Span::raw("Press "), key_span("[s]"), Span::raw(" to capture a snapshot.")]),
        |message| Line::from(message.to_owned()),
    );
    frame.render_widget(
        Paragraph::new(line)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().title(title).borders(Borders::ALL)),
        area,
    );
}

fn draw_memory_summary(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, memory: &MemorySnapshot) {
    let [gauges, regions] = Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).areas(area);
    let [live, peak, mapped] =
        Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(33), Constraint::Percentage(33)]).areas(gauges);
    frame.render_widget(memory_gauge(" Live ", memory.live_bytes, memory.mapped_bytes, Color::Green), live);
    frame.render_widget(
        memory_gauge(" Peak ", memory.peak_live_bytes, memory.mapped_bytes, Color::Yellow),
        peak,
    );
    frame.render_widget(
        memory_gauge(" Mapped ", memory.mapped_bytes, memory.reserved_bytes, Color::Cyan),
        mapped,
    );
    let region_details = if memory.regions.is_empty() {
        "No allocator regions".to_string()
    } else {
        memory
            .regions
            .iter()
            .map(|region| {
                format!(
                    "R{}: {} · {}/{} slices",
                    region.index,
                    format_bytes(region.reserved_bytes),
                    format_count(region.used_slices),
                    format_count(region.used_slices.saturating_add(region.free_slices))
                )
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{} lifetime allocations · {} assigned / {} free slices · {region_details}",
            format_count(memory.allocations),
            format_count(memory.used_slices),
            format_count(memory.free_slices)
        ))
        .block(
            Block::default()
                .title(format!(
                    " Regions: {} • reserved {} • {} slices • small {} • medium {} • bump {} • other {} ",
                    memory.regions.len(),
                    format_bytes(memory.reserved_bytes),
                    format_bytes(memory.slice_bytes),
                    format_count(memory.small_slices),
                    format_count(memory.medium_slices),
                    format_count(memory.bump_slices),
                    format_count(memory.unknown_slices),
                ))
                .borders(Borders::ALL),
        ),
        regions,
    );
}

fn memory_gauge(title: &'static str, value: u64, maximum: u64, color: Color) -> Gauge<'static> {
    Gauge::default()
        .block(Block::default().title(title).borders(Borders::ALL))
        .gauge_style(Style::default().fg(color).bg(Color::Black))
        .percent(percent(value, maximum))
        .label(format!("{} / {}", format_bytes(value), format_bytes(maximum)))
}

fn draw_memory_buckets(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    tier: Option<&super::data::MemoryTierData>,
    memory: &MemorySnapshot,
    view: HeapViewState,
) {
    let selected = tier.map_or(0, |tier| view.bucket_selected.min(tier.buckets.len().saturating_sub(1)));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let maximum = tier
        .into_iter()
        .flat_map(|tier| &tier.buckets)
        .map(|bucket| bucket.allocations)
        .max()
        .unwrap_or(0);
    let title = memory_tier_title(tier, memory);
    let block = Block::default()
        .title(Line::from(vec![
            Span::raw(title),
            key_span("↑/↓"),
            Span::raw(" select · "),
            key_span("Enter"),
            Span::raw(" locations "),
        ]))
        .borders(Borders::ALL);
    let Some(tier) = tier.filter(|tier| !tier.buckets.is_empty()) else {
        frame.render_widget(Paragraph::new("No retained allocation events for this tier.").block(block), area);
        return;
    };
    let size_width = tier
        .buckets
        .iter()
        .map(|bucket| memory_bucket_label(bucket).chars().count())
        .max()
        .unwrap_or("Size".len())
        .max("Size".len());
    let size_width = u16::try_from(size_width).unwrap_or(u16::MAX).min(30);
    let rows = tier.buckets.iter().skip(first).take(visible).map(|bucket| {
        let displayed_live = bucket.topology_live_allocations.unwrap_or(bucket.live_allocations);
        let (bar_value, bar_total) = match (bucket.topology_live_allocations, bucket.capacity_blocks) {
            (Some(live), Some(capacity)) => (live, capacity),
            _ => (bucket.allocations, maximum),
        };
        Row::new(vec![
            Cell::from(memory_bucket_label(bucket)),
            Cell::from(format_count(bucket.allocations)),
            Cell::from(format_bytes(bucket.allocated_bytes)),
            Cell::from(format_count(displayed_live)),
            Cell::from(format_count(u64::try_from(bucket.hotspots.len()).unwrap_or(u64::MAX))),
            Cell::from(Line::from(utilization_bar(bar_value, bar_total, 10))),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(size_width),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(9),
            Constraint::Length(11),
            Constraint::Min(12),
        ],
    )
    .header(
        Row::new(["Size", "Retained", "Bytes", "Live", "Hotspots", "Distribution"]).style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .column_spacing(1)
    .block(block)
    .row_highlight_style(if view.focus == HeapFocus::Buckets {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().bg(Color::DarkGray)
    });
    let mut state = TableState::default().with_selected(Some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
}

fn memory_tier_title(tier: Option<&super::data::MemoryTierData>, memory: &MemorySnapshot) -> String {
    let Some(tier) = tier else {
        return " Size Distribution ".to_owned();
    };
    let live_label = if tier.kind == MemoryTier::Direct {
        "window live"
    } else {
        "current"
    };
    let detail = match tier.kind {
        MemoryTier::Small => format!("{} size classes", memory.size_classes.len()),
        MemoryTier::Medium => format!(
            "{} backing slices · overhead {} · largest {}",
            format_count(memory.medium_allocations.span_slices),
            format_bytes(
                memory
                    .medium_allocations
                    .usable_bytes
                    .saturating_sub(memory.medium_allocations.requested_bytes)
            ),
            format_bytes(memory.medium_allocations.largest_requested_bytes)
        ),
        MemoryTier::Direct => "routing inferred from size/alignment".to_owned(),
    };
    format!(
        " {} · {live_label} {} / {} · retained {} / {} · {detail} ",
        tier.kind.label(),
        format_count(tier.current_allocations),
        format_bytes(tier.current_bytes),
        format_count(tier.retained_allocations()),
        format_bytes(tier.retained_bytes()),
    )
}

fn draw_memory_hotspots(frame: &mut ratatui::Frame<'_>, area: Rect, bucket: Option<&super::data::MemoryBucket>, view: HeapViewState) {
    let selected = bucket.map_or(0, |bucket| view.hotspot_selected.min(bucket.hotspots.len().saturating_sub(1)));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        format!("{:>9} {:>11} {:>9}  Location", "Events", "Bytes", "Live"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if let Some(bucket) = bucket {
        lines.extend(
            bucket
                .hotspots
                .iter()
                .skip(first)
                .take(visible)
                .enumerate()
                .map(|(index, hotspot)| {
                    primitive_selection_line(
                        Line::from(format!(
                            "{:>9} {:>11} {:>9}  {}",
                            format_count(hotspot.allocations),
                            format_bytes(hotspot.allocated_bytes),
                            format_count(hotspot.live_allocations),
                            hotspot.location(view.stack_filter),
                        )),
                        first + index == selected,
                        view.focus == HeapFocus::Hotspots,
                    )
                }),
        );
    }
    if bucket.is_none_or(|bucket| bucket.hotspots.is_empty()) {
        lines.push(Line::from("No retained allocation locations for this bucket."));
    }
    let bucket_detail = bucket.map_or_else(String::new, |bucket| {
        let topology = match (bucket.topology_live_allocations, bucket.capacity_blocks) {
            (Some(live), Some(capacity)) => {
                let bytes = match (bucket.requested_bytes, bucket.usable_bytes) {
                    (Some(requested), Some(usable)) => format!(
                        " · requested {} · waste {}",
                        format_bytes(requested),
                        format_bytes(usable.saturating_sub(requested))
                    ),
                    _ => String::new(),
                };
                format!(" · current class {}/{}{bytes}", format_count(live), format_count(capacity))
            }
            _ => String::new(),
        };
        format!(
            " · {} · retained live {} / {}{topology}",
            memory_bucket_label(bucket),
            format_count(bucket.live_allocations),
            format_bytes(bucket.live_bytes)
        )
    });
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(format!(" Allocation Locations{bucket_detail} · ")),
                    key_span("↑/↓"),
                    Span::raw(" select · "),
                    key_span("Backspace"),
                    Span::raw(" distribution "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_memory_stack(frame: &mut ratatui::Frame<'_>, area: Rect, hotspot: Option<&AllocationHotspot>, view: HeapViewState) {
    let Some(hotspot) = hotspot else {
        draw_empty_panel(frame, area, " Stack Trace ");
        return;
    };
    let stack = hotspot.stack(view.stack_filter);
    let visible = usize::from(area.height.saturating_sub(2));
    let scroll = view.stack_scroll.min(stack.len().saturating_sub(visible));
    let lines = if stack.is_empty() {
        vec![Line::from("Backtraces were not captured for this location.")]
    } else {
        stack
            .iter()
            .skip(scroll)
            .take(visible)
            .enumerate()
            .map(|(index, frame)| Line::from(format!("{:>3}  {frame}", scroll + index)))
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(" Stack Trace · "),
                    key_span("[f]"),
                    Span::raw(" application/all · "),
                    key_span("PgUp/PgDn"),
                    Span::raw(" scroll "),
                ]))
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn memory_bucket_label(bucket: &super::data::MemoryBucket) -> String {
    if bucket.lower_bytes == bucket.upper_bytes {
        format_bytes(bucket.upper_bytes)
    } else {
        format!("{}–{}", format_bytes(bucket.lower_bytes), format_bytes(bucket.upper_bytes))
    }
}

fn utilization_bar(used: u64, total: u64, width: usize) -> Span<'static> {
    let filled = usize::from(percent(used, total)) * width / 100;
    Span::styled(
        format!("[{}{}]", "█".repeat(filled), "░".repeat(width.saturating_sub(filled))),
        Style::default().fg(Color::Green),
    )
}

fn percent(value: u64, maximum: u64) -> u16 {
    if maximum == 0 {
        return 0;
    }
    u16::try_from(value.min(maximum).saturating_mul(100) / maximum).unwrap_or(100)
}

fn metric_line(label: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(value),
    ])
}

fn recording_policy_line(label: &'static str, policy: RecordingPolicy) -> Line<'static> {
    metric_line(
        label,
        format!(
            "{}; backtraces {}; sample 1/{} ({})",
            if policy.enabled { "on" } else { "off" },
            if policy.capture_backtraces { "on" } else { "off" },
            format_count(u64::from(policy.sampling_one_in)),
            format_sampling_percentage(policy.sampling_one_in),
        ),
    )
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

fn format_bytes(bytes: u64) -> String {
    const KIBIBYTE: u64 = 1024;
    const MEBIBYTE: u64 = KIBIBYTE * 1024;
    const GIBIBYTE: u64 = MEBIBYTE * 1024;
    if bytes >= GIBIBYTE {
        format_scaled_bytes(bytes, GIBIBYTE, "GiB")
    } else if bytes >= MEBIBYTE {
        format_scaled_bytes(bytes, MEBIBYTE, "MiB")
    } else if bytes >= KIBIBYTE {
        format_scaled_bytes(bytes, KIBIBYTE, "KiB")
    } else {
        format!("{bytes} B")
    }
}

fn format_scaled_bytes(bytes: u64, unit: u64, suffix: &str) -> String {
    let whole = bytes / unit;
    let hundredths = bytes % unit * 100 / unit;
    format!("{whole}.{hundredths:02} {suffix}")
}

fn browse_footer(status: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!(" {status}")),
        Span::raw("  "),
        key_span("↑/↓"),
        Span::raw(" select  "),
        key_span("Enter"),
        Span::raw(" connect  "),
        key_span("r"),
        Span::raw(" refresh  "),
        key_span("q"),
        Span::raw(" quit"),
    ])
}

fn connected_footer(
    configuration: RecordingConfiguration,
    event_buffers: EventBufferDisposition,
    snapshot: Option<&CapturedSnapshot>,
    status: &str,
) -> Line<'static> {
    let snapshot_buffers = match event_buffers {
        EventBufferDisposition::Retain => "retain",
        EventBufferDisposition::Clear => "clear",
        EventBufferDisposition::Release => "release",
    };
    let state_style = |enabled| {
        Style::default()
            .fg(if enabled { Color::Green } else { Color::Red })
            .add_modifier(Modifier::BOLD)
    };
    let mut spans = vec![
        Span::raw(" A/E/X/R: "),
        Span::styled(
            if configuration.allocations.enabled { "A" } else { "-" },
            state_style(configuration.allocations.enabled),
        ),
        Span::styled(
            if configuration.general_events.enabled { "E" } else { "-" },
            state_style(configuration.general_events.enabled),
        ),
        Span::styled(
            if configuration.arc_dereferences.enabled { "X" } else { "-" },
            state_style(configuration.arc_dereferences.enabled),
        ),
        Span::styled(
            if configuration.runtime_tasks.enabled { "R" } else { "-" },
            state_style(configuration.runtime_tasks.enabled),
        ),
        Span::raw(" "),
        key_span("[c configure]"),
        Span::raw(" │ Snapshot buffers: "),
        Span::styled(snapshot_buffers, Style::default().fg(Color::Cyan)),
        Span::raw(" "),
        key_span("[d]"),
        Span::raw(" │ Snapshot "),
        key_span("[s]"),
        Span::raw(" │ "),
        key_span("Tab"),
        Span::raw(" tabs │ "),
        key_span("Esc"),
        Span::raw(" disconnect │ "),
        key_span("q"),
        Span::raw(" quit"),
    ];
    if let Some(snapshot) = snapshot {
        spans.push(Span::raw(format!(" {}", snapshot_time(snapshot))));
    }
    if !status.is_empty() {
        spans.push(Span::raw(format!(" │ {status}")));
    }
    Line::from(spans)
}

fn key_span(label: &'static str) -> Span<'static> {
    Span::styled(label, key_style())
}

fn key_style() -> Style {
    Style::default().fg(KEY_COLOR).add_modifier(Modifier::BOLD)
}

fn snapshot_time(snapshot: &CapturedSnapshot) -> String {
    let local: DateTime<Local> = snapshot.captured_at.into();
    format!("{} ({})", local.format("%H:%M:%S"), format_age(snapshot.captured_instant.elapsed()))
}

fn format_age(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 60 * 60 {
        format!("{}m ago", seconds / 60)
    } else {
        format!("{}h ago", seconds / (60 * 60))
    }
}

#[cfg(test)]
fn recording_label(policy: RecordingPolicy) -> &'static str {
    match (policy.enabled, policy.capture_backtraces) {
        (false, _) => "off",
        (true, false) => "on",
        (true, true) => "on + backtraces",
    }
}

fn recording_configuration_label(configuration: RecordingConfiguration) -> &'static str {
    let policies = [
        configuration.allocations,
        configuration.general_events,
        configuration.arc_dereferences,
        configuration.runtime_tasks,
        configuration.io,
    ];
    if policies.iter().all(|policy| !policy.enabled) {
        "off"
    } else if policies
        .iter()
        .filter(|policy| policy.enabled)
        .all(|policy| policy.capture_backtraces)
    {
        "on + backtraces"
    } else {
        "on"
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Instant, SystemTime};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use seismograph::recorder::RecordingPolicies;
    use seismograph::recorder::event::{
        Address, Event, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events, ObjectId,
    };
    use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, WorkerId};
    use seismograph::recorder::thread::{ThreadId, ThreadLog};
    use seismograph_rallocator::callers::{
        AddressLookup, AddressLookupFields, Callers, CallersFields, Event as AllocationEvent, EventFields as AllocationEventFields,
        EventKind as AllocationEventKind, HeapKind,
    };
    use seismograph_rallocator::snapshot::{Estimate, EstimateFields, Region, SizeClass, SizeClassFields, Snapshot, Version};
    use seismograph_rallocator::topology::{Segment, SegmentFields, Slice, SliceKind, TopologyRegion};

    use super::*;

    fn descriptor() -> MonitorDescriptor {
        MonitorDescriptor {
            name: "worker".into(),
            instance: Some("west".into()),
            process_id: 42,
            instance_id: seismograph_protocol::monitor::InstanceId::from_bytes([1; 16]),
            port: 1234,
            authentication: seismograph_protocol::monitor::AuthenticationToken::from_bytes([2; 32]),
        }
    }

    fn runtime_event(thread: u64, sequence: u64, kind: EventKind, payload: EventPayload, stack: &[u64]) -> Event {
        Event {
            thread_id: ThreadId::new(thread),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence * 100),
            kind,
            payload,
            call_stack: stack.iter().copied().map(Address::new).collect(),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the representative capture intentionally exercises every monitor panel from one coherent fixture"
    )]
    fn representative_capture() -> Box<CapturedSnapshot> {
        let allocation_event = |allocation_id, thread, kind, size, align, stack: Vec<u64>| {
            AllocationEvent::from_fields(AllocationEventFields {
                thread_log_id: 1,
                event_thread_id: thread,
                sequence: allocation_id,
                allocation_id,
                kind,
                heap_id: 1,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: allocation_id * 16,
                size,
                align,
                call_stack: stack,
            })
        };
        let mut allocator = Snapshot::new(Version::new(1, 0, 0));
        allocator.stats.live_bytes = 100_000;
        allocator.stats.peak_live_bytes = 200_000;
        allocator.stats.mapped_bytes = 400_000;
        allocator.stats.allocations = 3;
        let mut region = Region::default();
        region.region_index = 1;
        region.reserved_bytes = 1 << 30;
        region.used_slices = 4;
        region.free_slices = 12;
        allocator.regions.push(region);
        let mut small = Slice::default();
        small.slice_index = 0;
        small.kind = SliceKind::Small;
        small.segments.push(Segment::from_fields(SegmentFields {
            segment_index: 0,
            class_index: 0,
            context: false,
            live_blocks: 1,
            usable_blocks: 4,
            utilization_tracked: true,
        }));
        let mut medium = Slice::default();
        medium.slice_index = 1;
        medium.kind = SliceKind::Medium;
        medium.span_slices = 2;
        medium.owner = 1;
        medium.requested_bytes = 100_000;
        medium.usable_bytes = 131_072;
        let mut bump = Slice::default();
        bump.slice_index = 2;
        bump.kind = SliceKind::Bump;
        let mut unknown = Slice::default();
        unknown.slice_index = 3;
        unknown.kind = SliceKind::Unknown;
        let mut topology = TopologyRegion::default();
        topology.region_index = 1;
        topology.base_address = 0x1000;
        topology.region_bytes = 1 << 30;
        topology.slice_bytes = 64 * 1024;
        topology.used_bitmap = vec![0b1111];
        topology.slices = vec![small, medium, bump, unknown];
        allocator.topology.push(topology);
        allocator.size_classes.push(SizeClass::from_fields(SizeClassFields {
            class_index: 0,
            block_bytes: 64,
            live_allocations: Estimate::from_fields(EstimateFields {
                value: 1,
                lower_bound: 1,
                upper_bound: 1,
            }),
            requested_bytes: Estimate::from_fields(EstimateFields {
                value: 32,
                lower_bound: 32,
                upper_bound: 32,
            }),
            usable_bytes: Estimate::from_fields(EstimateFields {
                value: 64,
                lower_bound: 64,
                upper_bound: 64,
            }),
        }));
        allocator.callers = Some(Callers::from_fields(CallersFields {
            session_id: 1,
            total_events: 4,
            lost_events: 1,
            threads: Vec::new(),
            events: vec![
                allocation_event(1, 1, AllocationEventKind::Allocated, 32, 8, vec![0x9000, 0x1000]),
                allocation_event(2, 1, AllocationEventKind::Allocated, 100_000, 8, vec![0x2000]),
                allocation_event(3, 1, AllocationEventKind::Allocated, 32, 128 * 1024, Vec::new()),
                allocation_event(1, 2, AllocationEventKind::Deallocated, 32, 8, vec![0x3000]),
            ],
            thread_names: Vec::new(),
        }));
        let addresses = vec![
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x1000,
                symbol: Some("app::allocate".into()),
                filename: Some("app.rs".into()),
                line: Some(10),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x2000,
                symbol: Some("app::allocate_medium".into()),
                filename: Some("medium.rs".into()),
                line: Some(20),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x3000,
                symbol: Some("app::free".into()),
                filename: Some("free.rs".into()),
                line: Some(30),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x9000,
                symbol: Some("rallocator::allocate".into()),
                filename: Some("rallocator/src/lib.rs".into()),
                line: Some(1),
                column: None,
            }),
        ];
        allocator.addresses = addresses.clone();

        let events = Events {
            clock: EventClock::ProcessMonotonic,
            total_events: 10,
            lost_events: 1,
            recording: RecordingPolicies::default(),
            threads: vec![
                ThreadLog {
                    thread_id: ThreadId::new(1),
                    total_events: 7,
                    lost_events: 1,
                    name: "producer".into(),
                },
                ThreadLog {
                    thread_id: ThreadId::new(2),
                    total_events: 3,
                    lost_events: 0,
                    name: "consumer".into(),
                },
            ],
            events: vec![
                runtime_event(1, 1, EventKind::ArcClone, EventPayload::Object(ObjectId::new(7)), &[0x1000]),
                runtime_event(2, 2, EventKind::ArcDeref, EventPayload::Object(ObjectId::new(7)), &[0x3000]),
                runtime_event(1, 11, EventKind::ArcDrop, EventPayload::Object(ObjectId::new(7)), &[]),
                runtime_event(1, 3, EventKind::MutexAccess, EventPayload::Object(ObjectId::new(8)), &[0x1000]),
                runtime_event(2, 4, EventKind::MutexContention, EventPayload::Object(ObjectId::new(8)), &[0x3000]),
                runtime_event(1, 5, EventKind::Allocation, EventPayload::Object(ObjectId::new(9)), &[0x1000]),
                runtime_event(2, 6, EventKind::Deallocation, EventPayload::Object(ObjectId::new(9)), &[0x3000]),
                runtime_event(1, 12, EventKind::Allocation, EventPayload::Object(ObjectId::new(10)), &[]),
                runtime_event(2, 13, EventKind::Deallocation, EventPayload::Object(ObjectId::new(10)), &[]),
                runtime_event(
                    1,
                    7,
                    EventKind::TaskSpawned,
                    EventPayload::Runtime(RuntimeEvent {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: None,
                        subject_id: 10,
                        related_id: 0,
                        value_0: 11,
                        value_1: 0,
                    }),
                    &[0x1000],
                ),
                runtime_event(
                    1,
                    8,
                    EventKind::TaskPollStarted,
                    EventPayload::Runtime(RuntimeEvent {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 25,
                        value_1: 1,
                    }),
                    &[],
                ),
                runtime_event(
                    1,
                    9,
                    EventKind::TaskPollFinished,
                    EventPayload::Runtime(RuntimeEvent {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 200,
                        value_1: 0,
                    }),
                    &[],
                ),
                runtime_event(
                    1,
                    10,
                    EventKind::TaskCompleted,
                    EventPayload::Runtime(RuntimeEvent {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    }),
                    &[],
                ),
            ],
        };
        let decoded = seismograph::snapshot::DecodedSnapshot {
            capture_duration_nanos: 1,
            events,
            sources: Vec::new(),
        };
        let runtime = super::super::data::RuntimeSnapshot::from_events(&decoded, &addresses, None);

        Box::new(CapturedSnapshot {
            memory: Some(MemorySnapshot::from_snapshot(&allocator)),
            allocations: Some(AllocationSnapshot::from_snapshot(&allocator)),
            heap_error: None,
            primitives: runtime.primitives,
            runtime: runtime.runtime,
            threads: runtime.threads,
            captured_at: SystemTime::UNIX_EPOCH,
            captured_instant: Instant::now(),
        })
    }

    fn render(app: &App) -> String {
        let backend = TestBackend::new(180, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    fn render_frame(draw: impl FnOnce(&mut ratatui::Frame<'_>)) -> String {
        let backend = TestBackend::new(180, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(draw).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn disabled_recording_state_is_readable() {
        assert_eq!(recording_label(RecordingPolicy::default()), "off");
    }

    #[test]
    fn enabled_recording_state_is_readable() {
        assert_eq!(
            recording_label(RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                ..Default::default()
            }),
            "on"
        );
    }

    #[test]
    fn backtrace_recording_state_is_readable() {
        assert_eq!(
            recording_label(RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                ..Default::default()
            }),
            "on + backtraces"
        );
    }

    #[test]
    fn snapshot_age_uses_compact_units() {
        assert_eq!(format_age(Duration::from_secs(125)), "2m ago");
    }

    #[test]
    #[expect(
        clippy::duration_suboptimal_units,
        reason = "the test exercises the seconds-based formatter exactly at its one-hour boundary"
    )]
    fn pure_formatters_cover_boundaries() {
        assert_eq!(
            [
                format_runtime_duration(1),
                format_runtime_duration(1_000),
                format_runtime_duration(1_000_000),
                format_runtime_duration(1_000_000_000),
                format_event_loss(0, 0),
                format_event_loss(1, 3),
                format_age(Duration::from_secs(1)),
                format_age(Duration::from_secs(3_600)),
                format_count(1_234_567),
                format_bytes(1),
                format_bytes(1_024),
                format_bytes(1_048_576),
                format_bytes(1_073_741_824),
            ],
            [
                "1ns".to_owned(),
                "1.00us".to_owned(),
                "1.00ms".to_owned(),
                "1.00s".to_owned(),
                "0.0%".to_owned(),
                "33.3%".to_owned(),
                "1s ago".to_owned(),
                "1h ago".to_owned(),
                "1,234,567".to_owned(),
                "1 B".to_owned(),
                "1.00 KiB".to_owned(),
                "1.00 MiB".to_owned(),
                "1.00 GiB".to_owned(),
            ]
        );
        assert_eq!((percent(10, 0), percent(200, 100)), (0, 100));
    }

    #[test]
    fn labels_and_visual_helpers_cover_empty_and_active_states() {
        let bucket = super::super::data::MemoryBucket {
            lower_bytes: 1,
            upper_bytes: 64,
            allocations: 1,
            allocated_bytes: 32,
            live_allocations: 1,
            live_bytes: 32,
            topology_live_allocations: None,
            capacity_blocks: None,
            requested_bytes: None,
            usable_bytes: None,
            hotspots: Vec::new(),
        };
        let exact = super::super::data::MemoryBucket {
            lower_bytes: 64,
            ..bucket.clone()
        };
        assert_eq!(
            (
                memory_bucket_label(&bucket),
                memory_bucket_label(&exact),
                utilization_bar(1, 2, 4).content.into_owned(),
                thread_label(7, "", 3),
                thread_label(7, "worker", 5),
                recording_configuration_label(RecordingConfiguration::default()),
            ),
            (
                "1 B–64 B".into(),
                "64 B".into(),
                "[██░░]".into(),
                "#7".into(),
                "#7 wo".into(),
                "off",
            )
        );

        let mut mixed = RecordingConfiguration::default();
        mixed.allocations.enabled = true;
        assert_eq!(recording_configuration_label(mixed), "on");
        mixed.allocations.capture_backtraces = true;
        assert_eq!(recording_configuration_label(mixed), "on + backtraces");
    }

    #[test]
    fn browser_capture_and_configuration_popups_render() {
        let mut app = App::new();
        assert!(render(&app).contains("Seismograph applications"));
        app.instances.push(super::super::app::Instance {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
        });
        app.status = "ready".into();
        app.capture_started_at = Some(Instant::now().checked_sub(Duration::from_millis(500)).unwrap());
        app.capture_step = Some(CaptureStep::Decode);
        assert!(render(&app).contains("Snapshot"));
        app.recording_configuration_popup = Some(RecordingConfigurationPopup {
            draft: RecordingConfiguration::default(),
            selected: 0,
        });

        let rendered = render(&app);

        assert!(rendered.contains("worker (west)"));
        assert!(rendered.contains("Recording configuration"));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "edge-state rendering is grouped to verify every panel's empty-data fallback coherently"
    )]
    fn panel_edge_states_render_without_panics() {
        let mut app = App::new();
        app.instances.push(super::super::app::Instance {
            descriptor: MonitorDescriptor {
                instance: None,
                ..descriptor()
            },
            recording: RecordingConfiguration::default(),
        });
        assert!(render(&app).contains("worker"));

        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Info,
            snapshot: Some(representative_capture()),
        };
        app.recorder_statistics = None;
        app.activity_samples.clear();
        assert!(render(&app).contains("Waiting for the first"));

        let mut capture = representative_capture();
        capture.allocations.as_mut().unwrap().hotspots.clear();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Allocations,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("No allocation stacks captured"));

        let capture = representative_capture();
        let allocations = capture.allocations.as_ref().unwrap();
        app.allocation_view.selected = allocations
            .hotspots
            .iter()
            .position(|hotspot| hotspot.stack(AllocationStackFilter::All).is_empty())
            .unwrap();
        app.allocation_view.stack_filter = AllocationStackFilter::All;
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Allocations,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("Backtraces were not captured"));

        let mut capture = representative_capture();
        capture.primitives.groups.clear();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Primitives,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("Stack Trace"));

        let capture = representative_capture();
        let operation = capture.primitives.groups[0]
            .operations
            .iter()
            .find(|operation| operation.kind == super::super::data::PrimitiveOperationKind::ArcDrop)
            .unwrap();
        let mut primitive_view = App::new().primitive_view;
        primitive_view.stack_filter = AllocationStackFilter::All;
        assert!(
            render_frame(|frame| {
                draw_primitive_stack(frame, frame.area(), Some(operation), primitive_view);
            })
            .contains("Backtraces were not captured")
        );

        let mut capture = representative_capture();
        capture.threads.threads[0].operations.clear();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Threads,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("Related Threads"));

        let mut capture = representative_capture();
        let operation = &mut capture.threads.threads[0].operations[0];
        operation.participants.clear();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Threads,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("No other retained thread activity"));

        let mut capture = representative_capture();
        let participant = &mut capture.threads.threads[0]
            .operations
            .iter_mut()
            .find(|operation| !operation.participants.is_empty())
            .unwrap()
            .participants[0];
        participant.objects.clear();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Threads,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("No shared retained objects"));

        let capture = representative_capture();
        let empty_stack = capture
            .threads
            .threads
            .iter()
            .flat_map(|thread| &thread.operations)
            .flat_map(|operation| &operation.participants)
            .flat_map(|participant| &participant.objects)
            .filter_map(super::super::data::ThreadObject::selected_stack)
            .find(|stack| stack.stack(AllocationStackFilter::All).is_empty())
            .unwrap();
        let mut stack_lines = Vec::new();
        append_thread_stack(&mut stack_lines, "Missing", None, AllocationStackFilter::Application);
        append_thread_stack(&mut stack_lines, "Empty", Some(empty_stack), AllocationStackFilter::All);
        assert!(stack_lines.len() >= 4);

        let mut capture = representative_capture();
        capture.runtime.workers[0].tasks[0].metric_scope = super::super::data::RuntimeTaskMetricScope::Lifetime;
        app.runtime_view.focus = RuntimeFocus::Details;
        app.runtime_view.detail_view = RuntimeDetailView::Details;
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("lifetime counters"));

        let mut capture = representative_capture();
        capture.runtime.workers[0].tasks[0].spawn_stack.clear();
        app.runtime_view.detail_view = RuntimeDetailView::SpawnStack;
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("Backtrace not captured"));

        let mut capture = representative_capture();
        capture.runtime.workers[0].tasks.clear();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        assert!(!render(&app).is_empty());

        let mut capture = representative_capture();
        let memory = capture.memory.as_mut().unwrap();
        memory.regions.clear();
        memory.tiers[0].buckets.clear();
        app.heap_view.tier = MemoryTier::Small;
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Heaps,
            snapshot: Some(capture),
        };
        let rendered = render(&app);
        assert!(rendered.contains("No allocator regions"));
        assert!(rendered.contains("No retained allocation events"));

        let mut capture = representative_capture();
        let bucket = &mut capture.memory.as_mut().unwrap().tiers[0].buckets[0];
        bucket.requested_bytes = None;
        bucket.usable_bytes = None;
        bucket.hotspots.clear();
        app.heap_view.tier = MemoryTier::Small;
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Heaps,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("No retained allocation locations"));

        assert!(memory_tier_title(None, &representative_capture().memory.unwrap()).contains("Size Distribution"));
        assert!(
            render_frame(|frame| {
                let area = frame.area();
                draw_empty_panel(frame, area, " Empty ");
            })
            .contains("Press")
        );

        let clear = connected_footer(RecordingConfiguration::default(), EventBufferDisposition::Clear, None, "ready");
        let release = connected_footer(RecordingConfiguration::default(), EventBufferDisposition::Release, None, "");
        assert_eq!(
            (
                line_text(&clear).contains("clear"),
                line_text(&clear).contains("ready"),
                line_text(&release).contains("release"),
            ),
            (true, true, true)
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one coherent fixture exercises every connected panel and focus mode"
    )]
    fn every_connected_panel_renders_empty_and_representative_data() {
        let mut app = App::new();
        app.activity_samples.push_back(ActivitySample {
            captured_at: Instant::now(),
            events_per_second: 1_234,
            total_events: 5_678,
        });
        app.recorder_statistics = Some(RecorderStatistics {
            thread_count: 2,
            total_events: 10,
            retained_events: 9,
            lost_events: 1,
            event_capacity_per_thread: 65_536,
            allocated_bytes: 4_096,
            recording: RecordingConfiguration::default(),
        });

        for tab in [
            MonitorTab::Info,
            MonitorTab::Heaps,
            MonitorTab::Allocations,
            MonitorTab::Primitives,
            MonitorTab::Threads,
            MonitorTab::Runtime,
        ] {
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab,
                snapshot: None,
            };
            app.snapshot_error = Some("capture unavailable".into());
            assert!(render(&app).contains("capture unavailable") || tab == MonitorTab::Info);

            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration {
                    allocations: RecordingPolicy {
                        enabled: true,
                        capture_backtraces: true,
                        ..Default::default()
                    },
                    general_events: RecordingPolicy {
                        enabled: true,
                        ..Default::default()
                    },
                    arc_dereferences: RecordingPolicy {
                        enabled: true,
                        ..Default::default()
                    },
                    runtime_tasks: RecordingPolicy {
                        enabled: true,
                        ..Default::default()
                    },
                    io: RecordingPolicy {
                        enabled: true,
                        ..Default::default()
                    },
                    cache: RecordingPolicy {
                        enabled: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                tab,
                snapshot: Some(representative_capture()),
            };
            app.snapshot_error = None;
            assert!(!render(&app).is_empty());
        }

        for tier in [MemoryTier::Small, MemoryTier::Medium, MemoryTier::Direct] {
            app.heap_view.tier = tier;
            app.heap_view.focus = HeapFocus::Hotspots;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Heaps,
                snapshot: Some(representative_capture()),
            };
            assert!(render(&app).contains(tier.label()));
        }

        app.allocation_view.stack_filter = AllocationStackFilter::All;
        app.allocation_view.stack_scroll = usize::MAX;
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Allocations,
            snapshot: Some(representative_capture()),
        };
        assert!(render(&app).contains("all frames"));

        for focus in [PrimitiveFocus::Types, PrimitiveFocus::Operations, PrimitiveFocus::Hotspots] {
            app.primitive_view.focus = focus;
            app.primitive_view.primitive_selected = 1;
            app.primitive_view.operation_selected = 1;
            app.primitive_view.stack_filter = AllocationStackFilter::All;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Primitives,
                snapshot: Some(representative_capture()),
            };
            assert!(render(&app).contains("Primitive Types"));
        }

        for focus in [
            ThreadFocus::Threads,
            ThreadFocus::Operations,
            ThreadFocus::Participants,
            ThreadFocus::Objects,
        ] {
            app.thread_view.focus = focus;
            app.thread_view.stack_filter = AllocationStackFilter::All;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Threads,
                snapshot: Some(representative_capture()),
            };
            assert!(render(&app).contains("Threads"));
        }

        for (focus, detail_view) in [
            (RuntimeFocus::Workers, RuntimeDetailView::Details),
            (RuntimeFocus::Tasks, RuntimeDetailView::Details),
            (RuntimeFocus::Details, RuntimeDetailView::Details),
            (RuntimeFocus::Details, RuntimeDetailView::SpawnStack),
        ] {
            app.runtime_view.focus = focus;
            app.runtime_view.detail_view = detail_view;
            app.runtime_view.detail_scroll = usize::MAX;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Runtime,
                snapshot: Some(representative_capture()),
            };
            assert!(render(&app).contains("Task"));
        }
    }
}
