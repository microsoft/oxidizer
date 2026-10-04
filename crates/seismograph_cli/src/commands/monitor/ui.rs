// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Axis, Block, Borders, Chart, Clear, Dataset, Gauge, GraphType, List, ListItem, ListState, Paragraph, Tabs};
use seismograph_protocol::message::{RecorderStatistics, RecordingConfiguration, RecordingPolicy};
use seismograph_protocol::monitor::MonitorDescriptor;

#[cfg(test)]
use super::app::HeapFocus;
use super::app::{
    ActivitySample, AllocationViewState, App, CacheFocus, CacheViewState, CaptureMode, CaptureStep, HeapViewState, IoFocus, IoViewState,
    MonitorTab, PrimitiveFocus, PrimitiveViewState, RecordingConfigurationPopup, RuntimeViewState, Screen, ThreadFocus, ThreadViewState,
    format_sampling_percentage, recording_policy_label,
};
use super::data::{
    AllocationHotspot, AllocationSnapshot, AllocationSort, AllocationStackFilter, CapturedSnapshot, PrimitiveSnapshot, PrimitiveSort,
    ThreadSnapshot, cache_event_label,
};
#[cfg(test)]
use super::data::{MemorySnapshot, MemoryTier};
use super::live_activity::LiveActivity;
use super::mouse::{ListTarget, MouseRows};

#[path = "info_ui.rs"]
mod info_ui;
#[path = "runtime_ui.rs"]
mod runtime_ui;
#[path = "task_events_ui.rs"]
mod task_events_ui;

const KEY_COLOR: Color = Color::Cyan;
const CONTENTION_COLOR: Color = Color::Yellow;

#[cfg_attr(test, mutants::skip)]
impl App {
    pub(super) fn screen_areas(&self, area: Rect) -> [Rect; 3] {
        Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(self.filter_banner_height()),
            Constraint::Length(1),
        ])
        .areas(area)
    }

    pub(super) fn draw(&self, frame: &mut ratatui::Frame<'_>) {
        self.draw_with_snapshot_time(frame, snapshot_time);
    }

    fn draw_with_snapshot_time(&self, frame: &mut ratatui::Frame<'_>, format_snapshot_time: impl Fn(&CapturedSnapshot) -> String) {
        self.draw_with_version(frame, format_snapshot_time, env!("CARGO_PKG_VERSION"));
    }

    fn draw_with_version(
        &self,
        frame: &mut ratatui::Frame<'_>,
        format_snapshot_time: impl Fn(&CapturedSnapshot) -> String,
        monitor_version: &str,
    ) {
        self.panels.rows.begin(frame.area());
        let [body, filter_banner, footer] = self.screen_areas(frame.area());
        let server_version = match &self.screen {
            Screen::Browse => "not connected",
            Screen::Connected { .. } => self.server_version.as_deref().unwrap_or("unknown (legacy server)"),
            Screen::Offline { .. } => "unknown (not recorded in snapshot)",
        };
        let snapshot_view = match &self.screen {
            Screen::Browse => {
                self.draw_browser(frame, body);
                None
            }
            Screen::Connected {
                descriptor,
                recording,
                tab,
                snapshot,
            } => Some((ViewOrigin::Live(descriptor, *recording), tab, snapshot)),
            Screen::Offline { path, tab, snapshot } => Some((ViewOrigin::Offline(path), tab, snapshot)),
        };
        if let Some((origin, tab, snapshot)) = snapshot_view {
            draw_connected(
                frame,
                body,
                &ConnectedView {
                    origin,
                    recording_unknown: self.recording_unknown,
                    monitor_version,
                    server_version,
                    tab: *tab,
                    snapshot: snapshot.as_deref(),
                    snapshot_error: self.snapshot_error.as_deref(),
                    heap_view: self.heap_view,
                    allocation_view: self.allocation_view,
                    primitive_view: self.primitive_view,
                    thread_view: self.thread_view,
                    runtime_view: self.runtime_view,
                    io_view: self.io_view,
                    cache_view: self.cache_view,
                    panels: &self.panels,
                    activity_samples: &self.activity_samples,
                    recorder_statistics: self.recorder_statistics.as_ref(),
                    live_activity: &self.live_activity,
                    info_thread_selected: self.info_thread_selected,
                },
            );
        }
        let line = match &self.screen {
            Screen::Browse => browse_footer(&self.status),
            Screen::Offline { .. } => Line::from(format!(
                " F1 help · Offline · read-only · F filters · Tab/1–8 tabs · drag borders to resize · q/Esc quit · {}",
                self.status
            )),
            Screen::Connected { recording, snapshot, .. } => {
                let snapshot_time = snapshot.as_deref().map(format_snapshot_time);
                connected_footer(
                    (!self.recording_unknown).then_some(*recording),
                    self.capture_mode,
                    snapshot_time.as_deref(),
                    &self.status,
                )
            }
        };
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(Color::DarkGray)), footer);
        self.draw_filter_banner(frame, filter_banner);
        if let (Some(started_at), Some(step)) = (self.capture_started_at, self.capture_step) {
            Self::draw_capture_popup(frame, started_at.elapsed(), step);
        }
        if let Some(popup) = self.recording_configuration_popup {
            Self::draw_recording_configuration_popup(frame, popup);
        }
        self.draw_filter_popup(frame);
        if let Some(help) = &self.help {
            help.draw(frame);
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
        self.panels
            .rows
            .register(area, 0, state.offset(), self.instances.len(), ListTarget::Applications);
    }

    fn draw_capture_popup(frame: &mut ratatui::Frame<'_>, elapsed: Duration, active_step: CaptureStep) {
        const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
        let popup_area = centered_rect(frame.area(), 60, 9);
        let frame_index = usize::try_from(elapsed.as_millis() / 150).unwrap_or(usize::MAX) % SPINNER.len();
        frame.render_widget(Clear, popup_area);
        let block = Block::default()
            .title(format!(" Snapshot · {:.1}s · F1 help ", elapsed.as_secs_f64()))
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
        let fields = popup.fields();
        let desired_height = u16::try_from(fields.len().saturating_add(2)).unwrap_or(u16::MAX);
        let height = area.height.saturating_sub(2).min(desired_height).max(3);
        let popup_area = centered_rect(area, width, height);
        let items = fields.into_iter().map(|field| {
            let value = field.value(popup);
            ListItem::new(format!("{:<32} {value:>32}", field.label()))
        });
        let list = List::new(items)
            .block(
                Block::default()
                    .title(" Recording configuration ")
                    .title_bottom(" F1 help · ↑/↓ field · ←/→ change · Enter apply · Esc cancel ")
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
    origin: ViewOrigin<'a>,
    recording_unknown: bool,
    monitor_version: &'a str,
    server_version: &'a str,
    tab: MonitorTab,
    snapshot: Option<&'a CapturedSnapshot>,
    snapshot_error: Option<&'a str>,
    heap_view: HeapViewState,
    allocation_view: AllocationViewState,
    primitive_view: PrimitiveViewState,
    thread_view: ThreadViewState,
    runtime_view: RuntimeViewState,
    io_view: IoViewState,
    cache_view: CacheViewState,
    panels: &'a super::panels::Panels,
    activity_samples: &'a VecDeque<ActivitySample>,
    recorder_statistics: Option<&'a RecorderStatistics>,
    live_activity: &'a LiveActivity,
    info_thread_selected: usize,
}

#[derive(Clone, Copy)]
enum ViewOrigin<'a> {
    Live(&'a MonitorDescriptor, RecordingConfiguration),
    Offline(&'a std::path::Path),
}

fn centered_rect(area: Rect, maximum_width: u16, maximum_height: u16) -> Rect {
    let width = area.width.min(maximum_width);
    let height = area.height.min(maximum_height);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

fn row_is_selected(first: usize, index: usize, selected: usize) -> bool {
    first.checked_add(index) == Some(selected)
}

fn draw_connected(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, view: &ConnectedView<'_>) {
    let [tabs, content] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(area);
    let tabs_block = Block::default().borders(Borders::ALL);
    let tabs_block = match view.origin {
        ViewOrigin::Live(..) => tabs_block,
        ViewOrigin::Offline(path) => tabs_block.title(format!(" {} · offline · read-only ", path.display())),
    };
    frame.render_widget(
        Tabs::new(super::panels::TABS.map(|(_, title)| title))
            .padding("", "")
            .select(view.tab.index())
            .block(tabs_block)
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
            .divider("│"),
        tabs,
    );
    if let ViewOrigin::Offline(path) = view.origin
        && view.snapshot.is_none()
        && view.tab != MonitorTab::Info
    {
        frame.render_widget(
            Paragraph::new(format!(
                "{}\n\n{}\n\nq / Esc to quit",
                path.display(),
                view.snapshot_error.unwrap_or("Loading and decoding snapshot…")
            ))
            .block(Block::default().title(" Offline snapshot · read-only ").borders(Borders::ALL)),
            content,
        );
        return;
    }
    let panels = view.panels.arrange(view.tab, content).areas;
    match view.tab {
        MonitorTab::Info => draw_snapshot_info(frame, content, view),
        MonitorTab::Heaps => draw_native(
            frame,
            &view.panels.rows,
            content,
            view.snapshot.and_then(|capture| capture.native.as_deref()),
            view.snapshot
                .and_then(|capture| capture.heap_error.as_deref())
                .or(view.snapshot_error),
            view.heap_view,
        ),
        MonitorTab::Allocations => draw_allocations(
            frame,
            &view.panels.rows,
            [panels[0], panels[1]],
            view.snapshot.and_then(|capture| capture.allocations.as_ref()),
            view.snapshot_error,
            view.allocation_view,
        ),
        MonitorTab::Primitives => {
            draw_primitives(
                frame,
                &view.panels.rows,
                [panels[0], panels[1], panels[2], panels[3]],
                view.snapshot.map(|snapshot| &snapshot.primitives),
                view.snapshot_error,
                view.primitive_view,
            );
        }
        MonitorTab::Threads => draw_threads(
            frame,
            &view.panels.rows,
            panels,
            view.snapshot.map(|snapshot| &snapshot.threads),
            view.snapshot_error,
            view.thread_view,
        ),
        MonitorTab::Runtime => runtime_ui::draw(
            frame,
            &view.panels.rows,
            view.panels.arrange_runtime(content, view.runtime_view.focus).areas,
            view.snapshot,
            view.snapshot_error,
            view.runtime_view,
        ),
        MonitorTab::Io => draw_io(
            frame,
            &view.panels.rows,
            [panels[0], panels[1]],
            view.snapshot.map(|snapshot| &snapshot.io),
            view.snapshot_error,
            view.io_view,
        ),
        MonitorTab::Cache => draw_cache(
            frame,
            &view.panels.rows,
            [panels[0], panels[1]],
            view.snapshot.map(|snapshot| &snapshot.cache),
            view.snapshot_error,
            view.cache_view,
        ),
    }
}

fn draw_snapshot_info(frame: &mut ratatui::Frame<'_>, area: Rect, view: &ConnectedView<'_>) {
    match view.origin {
        ViewOrigin::Live(descriptor, recording) => draw_info(
            frame,
            {
                let panels = view.panels.arrange(MonitorTab::Info, area).areas;
                [panels[0], panels[1], panels[2]]
            },
            descriptor,
            recording,
            view,
        ),
        ViewOrigin::Offline(path) => {
            let mut lines = vec![
                Line::from(format!("Monitor (seismograph_cli): {}", view.monitor_version)),
                Line::from(format!("Server (seismograph): {}", view.server_version)),
                Line::from(path.display().to_string()),
                Line::from("Offline snapshot · read-only"),
                Line::from("Capture time: not recorded in the snapshot"),
                Line::from("No process connection; recording and capture controls are disabled."),
            ];
            if let Some(snapshot) = view.snapshot {
                lines.push(Line::from(format!(
                    "Source events: {} accepted · {} overwritten · {} threads",
                    format_count(snapshot.primitives.total_events),
                    format_count(snapshot.primitives.lost_events),
                    snapshot.threads.threads.len(),
                )));
                if let Some(error) = &snapshot.heap_error {
                    lines.push(Line::from(error.as_str()));
                }
            } else {
                lines.push(Line::from(view.snapshot_error.unwrap_or("Loading and decoding snapshot…")));
            }
            frame.render_widget(
                Paragraph::new(lines).block(Block::default().title(" Snapshot file ").borders(Borders::ALL)),
                area,
            );
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the paired resource and operation panes share selection and layout state that is clearest in one renderer"
)]
fn draw_io(
    frame: &mut ratatui::Frame<'_>,
    mouse_rows: &MouseRows,
    [resources_area, operations_area]: [Rect; 2],
    io: Option<&super::data::IoMonitorSnapshot>,
    unavailable: Option<&str>,
    view: IoViewState,
) {
    let Some(io) = io else {
        draw_empty_panel_with_message(frame, resources_area, " I/O Resources ", unavailable);
        draw_empty_panel_with_message(frame, operations_area, " Operations ", unavailable);
        return;
    };

    let resource_selected = view.resource_selected.min(io.resources.len().saturating_sub(1));
    let resource = io.resources.get(resource_selected);
    let visible_resources = usize::from(resources_area.height.saturating_sub(3));
    let first_resource = resource_selected.saturating_sub(visible_resources.saturating_sub(1));
    mouse_rows.register(resources_area, 1, first_resource, io.resources.len(), ListTarget::IoResources);
    let mut resource_lines = vec![Line::from(Span::styled(
        format!(
            "{:<12} {:<12} {:>7} {:>7} {:>10} {:>10} {:>7}",
            "Resource", "Kind", "Reads", "Writes", "Requested", "Completed", "Errors"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    resource_lines.extend(
        io.resources
            .iter()
            .skip(first_resource)
            .take(visible_resources)
            .enumerate()
            .map(|(index, resource)| {
                primitive_selection_line(
                    Line::from(format!(
                        "#{:<11} {:<12} {:>7} {:>7} {:>10} {:>10} {:>7}",
                        resource.resource_id,
                        io_resource_kind_label(resource.kind),
                        format_count(resource.reads),
                        format_count(resource.writes),
                        format_bytes(resource.requested_bytes),
                        format_bytes(resource.completed_bytes),
                        format_count(resource.errors.saturating_add(resource.canceled)),
                    )),
                    row_is_selected(first_resource, index, resource_selected),
                    view.focus == IoFocus::Resources,
                )
            }),
    );
    frame.render_widget(
        Paragraph::new(resource_lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(format!(
                        " I/O Resources · source retained {} / {} accepted · {} overwritten ({}) · ",
                        format_count(io.retained_events),
                        format_count(io.total_events),
                        format_count(io.lost_events),
                        format_event_loss(io.lost_events, io.total_events),
                    )),
                    key_span("Enter"),
                    Span::raw(" operations "),
                ]))
                .borders(Borders::ALL),
        ),
        resources_area,
    );

    let operations = resource.map_or(&[][..], |resource| resource.operations.as_slice());
    let operation_selected = view.operation_selected.min(operations.len().saturating_sub(1));
    let visible_operations = usize::from(operations_area.height.saturating_sub(3));
    let first_operation = operation_selected.saturating_sub(visible_operations.saturating_sub(1));
    mouse_rows.register(operations_area, 1, first_operation, operations.len(), ListTarget::IoOperations);
    let mut operation_lines = vec![Line::from(Span::styled(
        format!(
            "{:<11} {:<6} {:<12} {:>8} {:>10} {:>10} {:>10}",
            "Operation", "Type", "Outcome", "Thread", "Requested", "Completed", "Duration"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    operation_lines.extend(
        operations
            .iter()
            .skip(first_operation)
            .take(visible_operations)
            .enumerate()
            .map(|(index, operation)| {
                primitive_selection_line(
                    Line::from(format!(
                        "#{:<10} {:<6} {:<12} #{:<7} {:>10} {:>10} {:>10}",
                        operation.operation_id,
                        operation.kind.label(),
                        io_outcome_label(operation.outcome),
                        operation.thread_id,
                        format_bytes(operation.requested_bytes),
                        format_bytes(operation.completed_bytes),
                        operation.duration_nanos.map_or_else(|| "-".into(), format_runtime_duration),
                    )),
                    row_is_selected(first_operation, index, operation_selected),
                    view.focus == IoFocus::Operations,
                )
            }),
    );
    let details = operations.get(operation_selected).map_or_else(
        || "No retained I/O operations".to_owned(),
        |operation| {
            format!(
                "buffer {} · {} across {} span(s) · resource {}",
                operation.buffer_id.map_or_else(|| "-".into(), |id| format!("#{id}")),
                format_bytes(operation.buffer_len),
                format_count(u64::from(operation.buffer_span_count)),
                io_resource_kind_label(operation.resource_kind),
            )
        },
    );
    frame.render_widget(
        Paragraph::new(operation_lines).block(
            Block::default()
                .title(" Operations ")
                .title_bottom(Line::from(vec![
                    Span::raw(format!(" {details} · ")),
                    key_span("Backspace"),
                    Span::raw(" resources "),
                ]))
                .borders(Borders::ALL),
        ),
        operations_area,
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "the two coordinated cache panes share selection and rendered mouse targets"
)]
fn draw_cache(
    frame: &mut ratatui::Frame<'_>,
    mouse_rows: &MouseRows,
    [tiers_area, operations_area]: [Rect; 2],
    cache: Option<&super::data::CacheMonitorSnapshot>,
    unavailable: Option<&str>,
    view: CacheViewState,
) {
    let Some(cache) = cache else {
        draw_empty_panel_with_message(frame, tiers_area, " Cache Tiers ", unavailable);
        draw_empty_panel_with_message(frame, operations_area, " Outcomes ", unavailable);
        return;
    };

    let tier_selected = view.tier_selected.min(cache.tiers.len().saturating_sub(1));
    let tier = cache.tiers.get(tier_selected);
    let visible_tiers = usize::from(tiers_area.height.saturating_sub(3));
    let first_tier = tier_selected.saturating_sub(visible_tiers.saturating_sub(1));
    mouse_rows.register(tiers_area, 1, first_tier, cache.tiers.len(), ListTarget::CacheTiers);
    let mut tier_lines = vec![Line::from(Span::styled(
        format!(
            "{:<19} {:<10} {:>9} {:>9} {:>9} {:>9} {:>9}",
            "Tier identity", "Role", "Events", "Hits", "Misses", "Errors", "Hit rate"
        ),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    tier_lines.extend(
        cache
            .tiers
            .iter()
            .skip(first_tier)
            .take(visible_tiers)
            .enumerate()
            .map(|(index, tier)| {
                let lookups = tier.hits.saturating_add(tier.misses).saturating_add(tier.errors);
                let hit_rate = if lookups == 0 {
                    "-".into()
                } else {
                    format_hit_rate(tier.hits, lookups)
                };
                primitive_selection_line(
                    Line::from(format!(
                        "0x{:<17x} {:<10} {:>9} {:>9} {:>9} {:>9} {:>9}",
                        tier.tier_id,
                        if tier.fallback { "fallback" } else { "primary" },
                        format_count(tier.events),
                        format_count(tier.hits),
                        format_count(tier.misses),
                        format_count(tier.errors),
                        hit_rate,
                    )),
                    row_is_selected(first_tier, index, tier_selected),
                    view.focus == CacheFocus::Tiers,
                )
            }),
    );
    frame.render_widget(
        Paragraph::new(tier_lines).block(
            Block::default()
                .title(Line::from(vec![
                    Span::raw(format!(
                        " Cache Tiers · source retained {} / {} accepted · {} overwritten ({}) · ",
                        format_count(cache.retained_events),
                        format_count(cache.total_events),
                        format_count(cache.lost_events),
                        format_event_loss(cache.lost_events, cache.total_events),
                    )),
                    key_span("Enter"),
                    Span::raw(" outcomes "),
                ]))
                .borders(Borders::ALL),
        ),
        tiers_area,
    );

    let operations = tier.map_or(&[][..], |tier| tier.operations.as_slice());
    let operation_selected = view.operation_selected.min(operations.len().saturating_sub(1));
    let visible_operations = usize::from(operations_area.height.saturating_sub(3));
    let first_operation = operation_selected.saturating_sub(visible_operations.saturating_sub(1));
    mouse_rows.register(operations_area, 1, first_operation, operations.len(), ListTarget::CacheOperations);
    let mut operation_lines = vec![Line::from(Span::styled(
        format!("{:<28} {:>12}", "Outcome", "Events"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    operation_lines.extend(
        operations
            .iter()
            .skip(first_operation)
            .take(visible_operations)
            .enumerate()
            .map(|(index, operation)| {
                primitive_selection_line(
                    Line::from(format!(
                        "{:<28} {:>12}",
                        cache_event_label(operation.kind),
                        format_count(operation.events)
                    )),
                    row_is_selected(first_operation, index, operation_selected),
                    view.focus == CacheFocus::Operations,
                )
            }),
    );
    frame.render_widget(
        Paragraph::new(operation_lines).block(
            Block::default()
                .title(" Outcomes ")
                .title_bottom(Line::from(vec![key_span("Backspace"), Span::raw(" tiers ")]))
                .borders(Borders::ALL),
        ),
        operations_area,
    );
}

#[expect(
    clippy::useless_let_if_seq,
    reason = "the foreign non-exhaustive enum needs a defensive default while every known variant remains independently covered"
)]
fn io_resource_kind_label(kind: seismograph::recorder::io::IoResourceKind) -> &'static str {
    use seismograph::recorder::io::IoResourceKind;
    let mut label = "Unknown";
    if kind == IoResourceKind::File {
        label = "File";
    }
    if kind == IoResourceKind::TcpStream {
        label = "TCP stream";
    }
    if kind == IoResourceKind::TcpListener {
        label = "TCP listener";
    }
    if kind == IoResourceKind::NamedPipe {
        label = "Named pipe";
    }
    if kind == IoResourceKind::WinHttpRequest {
        label = "WinHTTP";
    }
    if kind == IoResourceKind::Other {
        label = "Other";
    }
    label
}

#[expect(
    clippy::useless_let_if_seq,
    reason = "the foreign non-exhaustive enum needs a defensive default while every known variant remains independently covered"
)]
fn io_outcome_label(outcome: seismograph::recorder::io::IoOutcome) -> &'static str {
    use seismograph::recorder::io::IoOutcome;
    let mut label = "unknown";
    if outcome == IoOutcome::Pending {
        label = "pending";
    }
    if outcome == IoOutcome::Success {
        label = "success";
    }
    if outcome == IoOutcome::EndOfStream {
        label = "end of stream";
    }
    if outcome == IoOutcome::Canceled {
        label = "canceled";
    }
    if outcome == IoOutcome::Error {
        label = "error";
    }
    label
}

fn format_hit_rate(hits: u64, lookups: u64) -> String {
    let tenths = u128::from(hits).saturating_mul(1_000) / u128::from(lookups.max(1));
    format!("{}.{:01}%", tenths / 10, tenths % 10)
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
    mouse_rows: &MouseRows,
    [hotspots_area, stack_area]: [Rect; 2],
    allocations: Option<&AllocationSnapshot>,
    unavailable: Option<&str>,
    view: AllocationViewState,
) {
    const COUNT_WIDTH: usize = 13;
    const BYTES_WIDTH: usize = 12;
    let Some(allocations) = allocations else {
        draw_empty_panel_with_message(frame, hotspots_area, " Allocation Hotspots ", unavailable);
        draw_empty_panel_with_message(frame, stack_area, " Stack Trace ", unavailable);
        return;
    };
    if view.events {
        draw_allocation_records(frame, mouse_rows, [hotspots_area, stack_area], allocations, view);
        return;
    }
    let hotspots = allocations.sorted_hotspots(view.sort, view.descending);
    let selected = view.selected.min(hotspots.len().saturating_sub(1));
    let visible_hotspots = usize::from(hotspots_area.height.saturating_sub(3));
    let first_hotspot = selected.saturating_sub(visible_hotspots.saturating_sub(1));
    mouse_rows.register(hotspots_area, 1, first_hotspot, allocations.hotspots.len(), ListTarget::Allocations);
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
        heading("Unmatched", COUNT_WIDTH, AllocationSort::LiveAllocations),
        Span::raw(" "),
        heading("Unmatched B", BYTES_WIDTH, AllocationSort::LiveBytes),
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
                    Span::raw(" Hotspots · e events • sort "),
                    key_span("["),
                    Span::raw(" previous • "),
                    key_span("]"),
                    Span::raw(" next • "),
                    key_span("[r]"),
                    Span::raw(" reverse • select "),
                    key_span("↑/↓"),
                    Span::raw(format!(
                        " • all-class source: {} accepted • {} overwritten ",
                        format_count(allocations.total_events),
                        format_count(allocations.lost_events)
                    )),
                ]))
                .title_bottom(" Unmatched retained allocations are not proven live allocations or leaks, even with zero overwrites ")
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

fn draw_allocation_records(
    frame: &mut ratatui::Frame<'_>,
    mouse_rows: &MouseRows,
    [records_area, stack_area]: [Rect; 2],
    allocations: &AllocationSnapshot,
    view: AllocationViewState,
) {
    let selected = view.selected.min(allocations.records.len().saturating_sub(1));
    let visible = usize::from(records_area.height.saturating_sub(2));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let mut state = ListState::default().with_selected(Some(selected.saturating_sub(first)));
    frame.render_stateful_widget(
        List::new(
            allocations
                .records
                .iter()
                .skip(first)
                .take(visible)
                .map(|record| ListItem::new(record.label())),
        )
        .block(
            Block::default()
                .title(" Retained allocation/free records · e hotspots · ↑↓ select ")
                .title_bottom(" View IDs correlate retained evidence only; orphan frees remain explicit ")
                .borders(Borders::ALL),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol("> "),
        records_area,
        &mut state,
    );
    mouse_rows.register(records_area, 0, first, allocations.records.len(), ListTarget::Allocations);
    let Some(record) = allocations.records.get(selected) else {
        draw_empty_panel_with_message(
            frame,
            stack_area,
            " Allocation/free evidence ",
            Some("No retained allocation or free records."),
        );
        return;
    };
    let mut lines = record.details();
    lines.push("Captured operation stack:".into());
    lines.extend(record.stack(view.stack_filter).iter().cloned());
    if record.stack(view.stack_filter).is_empty() {
        lines.push("Backtraces were not captured for this operation.".into());
    }
    frame.render_widget(
        wrapped_detail_paragraph(
            &lines,
            Block::default()
                .title(" Operation detail / stack · f frames · PgUp/PgDn scroll ")
                .borders(Borders::ALL),
            stack_area,
            view.stack_scroll,
        ),
        stack_area,
    );
}

fn wrapped_detail_paragraph(lines: &[String], block: Block<'static>, area: Rect, scroll: usize) -> Paragraph<'static> {
    let inner = block.inner(area);
    let paragraph = Paragraph::new(lines.join("\n")).wrap(ratatui::widgets::Wrap { trim: false });
    let max_scroll = paragraph.line_count(inner.width).saturating_sub(usize::from(inner.height));
    paragraph
        .scroll((u16::try_from(scroll.min(max_scroll)).unwrap_or(u16::MAX), 0))
        .block(block)
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
    [activity_area, details_area, threads_area]: [Rect; 3],
    descriptor: &MonitorDescriptor,
    recording: RecordingConfiguration,
    view: &ConnectedView<'_>,
) {
    info_ui::draw_activity(frame, activity_area, view.live_activity, view.activity_samples);
    info_ui::draw_threads(
        frame,
        &view.panels.rows,
        threads_area,
        view.live_activity,
        view.info_thread_selected,
    );
    let event_capacity = u64::from(recording.event_capacity_per_thread);
    let capacity = usize::try_from(recording.event_capacity_per_thread)
        .ok()
        .and_then(seismograph::recorder::EventBufferCapacity::new);
    let memory_per_thread = capacity
        .map(seismograph::recorder::EventBufferCapacity::memory_bytes_per_thread)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .unwrap_or(u64::MAX);
    let mut lines = vec![
        Line::from(format!("Monitor (seismograph_cli): {}", view.monitor_version)),
        Line::from(format!("Server (seismograph): {}", view.server_version)),
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
    if view.recording_unknown {
        lines.truncate(7);
        lines.push(Line::from(
            "Recording state UNKNOWN; waiting for authoritative server configuration.",
        ));
        frame.render_widget(
            Paragraph::new(Text::from(lines)).block(Block::default().title(" Info ").borders(Borders::ALL)),
            details_area,
        );
        return;
    }
    if let Some(statistics) = view.recorder_statistics {
        lines.extend([
            metric_line("Telemetry threads", format_count(statistics.thread_count)),
            metric_line("Telemetry memory total", format_bytes(statistics.allocated_bytes)),
            metric_line("Accepted telemetry events", format_count(statistics.total_events)),
            metric_line("Retained telemetry events", format_count(statistics.retained_events)),
            metric_line("Overwritten telemetry events", format_count(statistics.lost_events)),
        ]);
    } else if let Some(allocations) = view.snapshot.and_then(|capture| capture.allocations.as_ref()) {
        lines.extend([
            metric_line("Telemetry threads", format_count(allocations.thread_count)),
            metric_line(
                "Telemetry memory total",
                format_bytes(memory_per_thread.saturating_mul(allocations.thread_count)),
            ),
            metric_line("All-class source accepted", format_count(allocations.total_events)),
            metric_line("Retained allocator events", format_count(allocations.retained_events)),
            metric_line("All-class source overwritten", format_count(allocations.lost_events)),
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
    mouse_rows: &MouseRows,
    [types_area, operations_area, hotspots_area, stack_area]: [Rect; 4],
    primitives: Option<&PrimitiveSnapshot>,
    unavailable: Option<&str>,
    view: PrimitiveViewState,
) {
    let Some(primitives) = primitives else {
        draw_empty_panel_with_message(frame, types_area, " Primitive Types ", unavailable);
        draw_empty_panel_with_message(frame, operations_area, " Operations ", unavailable);
        draw_empty_panel_with_message(frame, hotspots_area, " Hotspots ", unavailable);
        draw_empty_panel_with_message(frame, stack_area, " Stack Trace ", unavailable);
        return;
    };
    draw_primitive_types(frame, mouse_rows, types_area, primitives, view);
    let group = primitives
        .groups
        .get(view.primitive_selected.min(primitives.groups.len().saturating_sub(1)));
    let operations = group.map(|group| group.sorted_operations(view.sort, view.descending));
    draw_primitive_operations(frame, mouse_rows, operations_area, operations.as_deref(), view);
    let operation = operations
        .as_ref()
        .and_then(|operations| operations.get(view.operation_selected.min(operations.len().saturating_sub(1))))
        .copied();
    draw_primitive_hotspots(frame, mouse_rows, hotspots_area, operation, view);
    draw_primitive_stack(frame, stack_area, operation, view);
}

fn draw_primitive_types(
    frame: &mut ratatui::Frame<'_>,
    mouse_rows: &MouseRows,
    area: Rect,
    primitives: &PrimitiveSnapshot,
    view: PrimitiveViewState,
) {
    const TYPE_WIDTH: usize = 19;

    let visible = usize::from(area.height.saturating_sub(3));
    let selected = view.primitive_selected.min(primitives.groups.len().saturating_sub(1));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    mouse_rows.register(area, 1, first, primitives.groups.len(), ListTarget::PrimitiveTypes);
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
                    row_is_selected(first, index, selected),
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
                        " details · {} retained primitive · {} source accepted · {} overwritten ",
                        format_count(primitives.groups.iter().map(|group| group.events).sum()),
                        format_count(primitives.total_events),
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
    mouse_rows: &MouseRows,
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
        let visible = usize::from(area.height.saturating_sub(3));
        let selected = view.operation_selected.min(operations.len().saturating_sub(1));
        let first = selected.saturating_sub(visible.saturating_sub(1));
        mouse_rows.register(area, 1, first, operations.len(), ListTarget::PrimitiveOperations);
        lines.extend(operations.iter().skip(first).take(visible).enumerate().map(|(index, operation)| {
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
            primitive_selection_line(
                line,
                row_is_selected(first, index, selected),
                view.focus == PrimitiveFocus::Operations,
            )
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
    mouse_rows: &MouseRows,
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
        mouse_rows.register(area, 1, first, operation.hotspots.len(), ListTarget::PrimitiveHotspots);
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
                        row_is_selected(first, index, selected),
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
    mouse_rows: &MouseRows,
    [threads_area, operations_area, participants_area, objects_area, stack_area]: [Rect; 5],
    threads: Option<&ThreadSnapshot>,
    unavailable: Option<&str>,
    view: ThreadViewState,
) {
    let Some(threads) = threads else {
        draw_empty_panel_with_message(frame, threads_area, " Threads ", unavailable);
        draw_empty_panel_with_message(frame, operations_area, " Operations ", unavailable);
        draw_empty_panel_with_message(frame, participants_area, " Related Threads ", unavailable);
        draw_empty_panel_with_message(frame, objects_area, " Objects ", unavailable);
        draw_empty_panel_with_message(frame, stack_area, " Stack Trace ", unavailable);
        return;
    };
    draw_thread_list(frame, mouse_rows, threads_area, threads, view);
    let thread = threads
        .threads
        .get(view.thread_selected.min(threads.threads.len().saturating_sub(1)));
    draw_thread_operations(frame, mouse_rows, operations_area, thread, view);
    let operation = thread.and_then(|thread| {
        thread
            .operations
            .get(view.operation_selected.min(thread.operations.len().saturating_sub(1)))
    });
    draw_thread_participants(
        frame,
        mouse_rows,
        participants_area,
        operation,
        thread.map(|thread| thread.thread_id),
        view,
    );
    let participant = operation.and_then(|operation| {
        operation
            .participants
            .get(view.participant_selected.min(operation.participants.len().saturating_sub(1)))
    });
    draw_thread_objects(frame, mouse_rows, objects_area, participant, view);
    let object = participant.and_then(|participant| {
        participant
            .objects
            .get(view.object_selected.min(participant.objects.len().saturating_sub(1)))
    });
    draw_thread_stack(frame, stack_area, object, view);
}

fn draw_thread_list(frame: &mut ratatui::Frame<'_>, mouse_rows: &MouseRows, area: Rect, threads: &ThreadSnapshot, view: ThreadViewState) {
    let selected = view.thread_selected.min(threads.threads.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    mouse_rows.register(area, 1, first, threads.threads.len(), ListTarget::Threads);
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<18} {:>9} {:>11}", "Thread", "Retained", "Overwritten"),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    lines.extend(threads.threads.iter().skip(first).take(visible).enumerate().map(|(index, thread)| {
        primitive_selection_line(
            Line::from(format!(
                "{:<18} {:>9} {:>11}",
                thread_label(thread.thread_id, &thread.name, 18),
                format_count(thread.retained_events),
                format_count(thread.lost_events),
            )),
            row_is_selected(first, index, selected),
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

fn draw_thread_operations(
    frame: &mut ratatui::Frame<'_>,
    mouse_rows: &MouseRows,
    area: Rect,
    thread: Option<&super::data::ThreadSummary>,
    view: ThreadViewState,
) {
    let selected = thread.map_or(0, |thread| view.operation_selected.min(thread.operations.len().saturating_sub(1)));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    mouse_rows.register(
        area,
        1,
        first,
        thread.map_or(0, |thread| thread.operations.len()),
        ListTarget::ThreadOperations,
    );
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<28} {:>10} {:>10} {:>13}", "Operation", "Events", "Objects", "Threads"),
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
                primitive_selection_line(line, row_is_selected(first, index, selected), view.focus == ThreadFocus::Operations)
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
    mouse_rows: &MouseRows,
    area: Rect,
    operation: Option<&super::data::ThreadOperation>,
    selected_thread_id: Option<u64>,
    view: ThreadViewState,
) {
    let Some(operation) = operation else {
        draw_empty_panel(frame, area, " Related Threads ");
        return;
    };
    let selected = view.participant_selected.min(operation.participants.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    mouse_rows.register(area, 1, first, operation.participants.len(), ListTarget::ThreadParticipants);
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
                        if Some(participant.thread_id) == selected_thread_id {
                            format!("{} (self)", participant.thread_id)
                        } else {
                            thread_label(participant.thread_id, &participant.name, 14)
                        },
                        format_count(u64::try_from(participant.objects.len()).unwrap_or(u64::MAX)),
                        format_count(participant.events),
                    )),
                    row_is_selected(first, index, selected),
                    view.focus == ThreadFocus::Participants,
                )
            }),
    );
    if operation.participants.is_empty() {
        lines.push(Line::from("No related retained thread activity for these objects."));
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
    mouse_rows: &MouseRows,
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
    mouse_rows.register(area, 1, first, participant.objects.len(), ListTarget::ThreadObjects);
    let mut lines = vec![Line::from(Span::styled(
        format!("{:<16} {:>6} {:>7}", "Object", "Own", "Related"),
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
                    row_is_selected(first, index, selected),
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

fn draw_native(
    frame: &mut ratatui::Frame<'_>,
    mouse_rows: &MouseRows,
    area: Rect,
    snapshot: Option<&seismograph_rallocator::native::Snapshot>,
    unavailable: Option<&str>,
    view: HeapViewState,
) {
    let Some(snapshot) = snapshot else {
        draw_empty_panel_with_message(frame, area, " Native v4 structure explorer ", unavailable);
        return;
    };
    let [summary, body, limitations] = Layout::vertical([Constraint::Length(7), Constraint::Min(5), Constraint::Length(5)]).areas(area);
    let [owners, details] = Layout::horizontal([Constraint::Percentage(32), Constraint::Percentage(68)]).areas(body);
    frame.render_widget(
        Paragraph::new(crate::native_view::overview(snapshot).join("\n"))
            .block(Block::default().title(" Global backend / coverage ").borders(Borders::ALL)),
        summary,
    );
    let selected = view.bucket_selected.min(snapshot.owners.len());
    let visible = usize::from(owners.height.saturating_sub(2)).max(1);
    let first = selected.saturating_sub(visible - 1);
    let rows = (first..(snapshot.owners.len() + 1).min(first + visible)).map(|index| {
        if index == 0 {
            return ListItem::new("Global backend");
        }
        let owner = &snapshot.owners[index - 1];
        ListItem::new(crate::native_view::owner_label(snapshot, owner)).style(Style::default().fg(
            if owner.source == seismograph_rallocator::native::ObservationSource::Unavailable {
                Color::Red
            } else if owner.source == seismograph_rallocator::native::ObservationSource::Busy {
                Color::Yellow
            } else {
                match owner.freshness(snapshot) {
                    seismograph_rallocator::native::Freshness::Current | seismograph_rallocator::native::Freshness::IdleInspection => {
                        Color::Green
                    }
                    seismograph_rallocator::native::Freshness::Unknown => Color::DarkGray,
                    _ => Color::Yellow,
                }
            },
        ))
    });
    let mut state = ListState::default().with_selected(Some(selected - first));
    frame.render_stateful_widget(
        List::new(rows)
            .block(Block::default().title(" Backend / owners · ↑↓ select ").borders(Borders::ALL))
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
            .highlight_symbol("> "),
        owners,
        &mut state,
    );
    mouse_rows.register(
        owners,
        0,
        first + state.offset(),
        snapshot.owners.len() + 1,
        ListTarget::HeapBuckets,
    );
    let lines = selected.checked_sub(1).and_then(|index| snapshot.owners.get(index)).map_or_else(
        || {
            let mut lines = crate::native_view::overview(snapshot);
            lines.push("Global cached bins: capacity only; physical residency unknown.".into());
            for (exponent, count) in snapshot.global.ranges.counts.iter().enumerate().filter(|(_, count)| **count != 0) {
                lines.push(format!("  2^{exponent} B: {count} ranges"));
            }
            lines
        },
        |owner| crate::native_view::owner_lines(snapshot, owner),
    );
    frame.render_widget(
        wrapped_detail_paragraph(
            &lines,
            Block::default()
                .title(" Native structures · PgUp/PgDn scroll ")
                .borders(Borders::ALL),
            details,
            view.stack_scroll,
        ),
        details,
    );
    frame.render_widget(
        Paragraph::new(crate::native_view::LIMITATIONS)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().title(" Meaning / limitations ").borders(Borders::ALL)),
        limitations,
    );
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
            recording_policy_label(policy),
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
        key_span(" F1"),
        Span::raw(" help "),
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
    configuration: Option<RecordingConfiguration>,
    capture_mode: CaptureMode,
    snapshot_time: Option<&str>,
    status: &str,
) -> Line<'static> {
    let unknown = configuration.is_none();
    let configuration = configuration.unwrap_or_default();
    let state_style = |enabled| {
        Style::default()
            .fg(if enabled { Color::Green } else { Color::Red })
            .add_modifier(Modifier::BOLD)
    };
    let mut spans = vec![
        key_span(" F1"),
        Span::raw(" help │"),
        Span::raw(if unknown { " Recording UNKNOWN: " } else { " A/E/X/R/I/C: " }),
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
        Span::styled(
            if configuration.io.enabled { "I" } else { "-" },
            state_style(configuration.io.enabled),
        ),
        Span::styled(
            if configuration.cache.enabled { "C" } else { "-" },
            state_style(configuration.cache.enabled),
        ),
        Span::raw(" "),
        key_span("[c configure]"),
        key_span(" [F filters]"),
        Span::raw(" │ "),
        Span::styled(capture_mode.label(), Style::default().fg(Color::Cyan)),
        key_span(" [s]"),
        key_span(" [d mode]"),
        key_span(" [C clear]"),
        Span::raw(" │ "),
        key_span("Tab"),
        Span::raw(" tabs │ drag borders to resize │ "),
        key_span("Esc"),
        Span::raw(" disconnect │ "),
        key_span("q"),
        Span::raw(" quit"),
    ];
    if unknown {
        // Do not render disabled indicators for a state we could not read back.
        spans.drain(3..9);
    }
    if let Some(snapshot_time) = snapshot_time {
        spans.push(Span::raw(format!(" {snapshot_time}")));
    }
    if !status.is_empty() {
        spans.insert(if unknown { 3 } else { 9 }, Span::raw(format!(" │ {status} │")));
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
    snapshot_time_at(snapshot, Instant::now())
}

fn snapshot_time_at(snapshot: &CapturedSnapshot, now: Instant) -> String {
    let (Some(captured_at), Some(captured_instant)) = (snapshot.captured_at, snapshot.captured_instant) else {
        return "capture time not recorded".into();
    };
    let local: DateTime<Local> = captured_at.into();
    format!(
        "{} ({})",
        local.format("%H:%M:%S"),
        format_age(now.saturating_duration_since(captured_instant))
    )
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
    recording_policy_label(policy)
}

fn recording_configuration_label(configuration: RecordingConfiguration) -> &'static str {
    let policies = [
        configuration.allocations,
        configuration.general_events,
        configuration.arc_dereferences,
        configuration.runtime_tasks,
        configuration.io,
        configuration.cache,
    ];
    if policies.iter().all(|policy| !policy.enabled) {
        "off"
    } else if policies
        .iter()
        .filter(|policy| policy.enabled)
        .all(|policy| policy.capture_backtraces && policy.sampling_one_in == 1)
    {
        "on"
    } else {
        "custom"
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use seismograph::recorder::RecordingPolicies;
    use seismograph::recorder::event::{
        Address, Event, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events, NumericEvent, ObjectId,
    };
    use seismograph::recorder::io::{BufferId, IoEvent, IoOperationId, IoOutcome, IoResourceId, IoResourceKind};
    use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, WorkerId};
    use seismograph::recorder::thread::{ThreadId, ThreadLog};
    use seismograph_rallocator::callers::{
        AddressLookup, AddressLookupFields, Callers, CallersFields, Event as AllocationEvent, EventFields as AllocationEventFields,
        EventKind as AllocationEventKind, HeapKind,
    };

    use super::super::app::{RuntimeFocus, TaskEventsFocus, TaskHistogram};
    use super::*;
    use crate::allocator_topology::{Segment, SegmentFields, Slice, SliceKind, TopologyRegion};
    use crate::allocator_view::{Estimate, EstimateFields, Region, SizeClass, SizeClassFields, Snapshot, Version};

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
        allocator.regions.push(Region {
            index: 1,
            reserved_bytes: 1 << 30,
            used_slices: 4,
            free_slices: 12,
        });
        let small = Slice {
            index: 0,
            kind: SliceKind::Small,
            segments: vec![Segment::from_fields(SegmentFields {
                index: 0,
                class_index: 0,
                context: false,
                live_blocks: 1,
                usable_blocks: 4,
                utilization_tracked: true,
            })],
            ..Slice::default()
        };
        let medium = Slice {
            index: 1,
            kind: SliceKind::Medium,
            span_slices: 2,
            owner: 1,
            requested_bytes: 100_000,
            usable_bytes: 131_072,
            ..Slice::default()
        };
        let bump = Slice {
            index: 2,
            kind: SliceKind::Bump,
            ..Slice::default()
        };
        let unknown = Slice {
            index: 3,
            kind: SliceKind::Unknown,
            ..Slice::default()
        };
        allocator.topology.push(TopologyRegion {
            region_index: 1,
            base_address: 0x1000,
            region_bytes: 1 << 30,
            slice_bytes: 64 * 1024,
            used_bitmap: vec![0b1111],
            slices: vec![small, medium, bump, unknown],
        });
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
                    14,
                    EventKind::IoReadStarted,
                    EventPayload::Io(IoEvent {
                        operation_id: IoOperationId::from_raw(1).unwrap(),
                        resource_id: IoResourceId::from_raw(2).unwrap(),
                        buffer_id: BufferId::from_raw(3),
                        requested_bytes: 4_096,
                        completed_bytes: 0,
                        buffer_len: 8_192,
                        buffer_span_count: 2,
                        resource_kind: IoResourceKind::File,
                        outcome: IoOutcome::Pending,
                    }),
                    &[],
                ),
                runtime_event(
                    1,
                    15,
                    EventKind::IoReadFinished,
                    EventPayload::Io(IoEvent {
                        operation_id: IoOperationId::from_raw(1).unwrap(),
                        resource_id: IoResourceId::from_raw(2).unwrap(),
                        buffer_id: BufferId::from_raw(3),
                        requested_bytes: 4_096,
                        completed_bytes: 2_048,
                        buffer_len: 10_240,
                        buffer_span_count: 3,
                        resource_kind: IoResourceKind::File,
                        outcome: IoOutcome::Success,
                    }),
                    &[],
                ),
                runtime_event(
                    2,
                    16,
                    EventKind::CacheHit,
                    EventPayload::Numeric(NumericEvent {
                        object_id: ObjectId::new(0x1234),
                        value: 0,
                    }),
                    &[],
                ),
                runtime_event(
                    2,
                    17,
                    EventKind::CacheMiss,
                    EventPayload::Numeric(NumericEvent {
                        object_id: ObjectId::new(0x1234),
                        value: 0,
                    }),
                    &[],
                ),
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
            native: Some(std::sync::Arc::new(crate::native_view::fixture::snapshot())),
            memory: Some(MemorySnapshot::from_snapshot(&allocator)),
            allocations: Some(AllocationSnapshot::from_snapshot(&allocator)),
            heap_error: None,
            primitives: runtime.primitives,
            runtime: runtime.runtime,
            task_events: runtime.task_events,
            io: runtime.io,
            cache: runtime.cache,
            threads: runtime.threads,
            captured_at: Some(SystemTime::UNIX_EPOCH),
            captured_instant: Some(Instant::now()),
            filter_index: None,
            filter_summary: super::super::filter_index::FilterSummary::default(),
        })
    }

    fn render(app: &App) -> String {
        let backend = TestBackend::new(180, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| app.draw_with_snapshot_time(frame, |_| "00:00:00 (0s ago)".into()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn versions_identify_monitor_and_server_before_capture_and_after_reconnect() {
        use crossterm::event::KeyCode;

        let mut app = App::new();
        app.instances.push(super::super::app::Instance {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            server_version: Some("12.34.56-rc.7".into()),
        });
        let browser = render(&app);
        assert!(!browser.contains("Monitor (seismograph_cli):"));
        assert!(!browser.contains("Server (seismograph):"));
        app.handle_key(KeyCode::Enter);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| app.draw_with_version(frame, |_| "unused before capture".into(), "98.76.54"))
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("Monitor (seismograph_cli): 98.76.54"), "{output}");
        assert!(output.contains("Server (seismograph): 12.34.56-rc.7"), "{output}");
        assert!(!output.contains("Server (seismograph): 98.76.54"));
        assert!(matches!(app.screen, Screen::Connected { snapshot: None, .. }));

        // Selecting another (legacy) instance must not retain the previous server version.
        app.screen = Screen::Browse;
        app.instances[0].server_version = None;
        app.handle_key(KeyCode::Enter);
        let legacy = render(&app);
        assert!(legacy.contains("Server (seismograph): unknown (legacy server)"));
        assert!(!legacy.contains("12.34.56-rc.7"));
    }

    #[test]
    fn offline_version_is_local_and_does_not_claim_to_identify_the_snapshot_producer() {
        let mut app = App::offline("capture.seismograph".into());
        for loaded in [false, true] {
            if loaded {
                app.finish_offline_load(representative_capture());
            }
            let output = render(&app);
            assert!(output.contains(&format!("Monitor (seismograph_cli): {}", env!("CARGO_PKG_VERSION"))));
            assert!(output.contains("Server (seismograph): unknown (not recorded in snapshot)"));
            assert!(!output.contains("unknown (legacy server)"));
        }
    }

    fn mouse_event(kind: crossterm::event::MouseEventKind, column: u16, row: u16) -> crossterm::event::MouseEvent {
        crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_scrolled_rows_follow_resized_panes_and_filter_banner() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let mut capture = representative_capture();
        let worker = capture.runtime.workers[0].clone();
        capture.runtime.workers = (0..60)
            .map(|index| {
                let mut worker = worker.clone();
                worker.runtime_name = format!("worker-{index:02}");
                let task = worker.tasks[0].clone();
                worker.tasks = (0..70)
                    .map(|index| {
                        let mut task = task.clone();
                        task.task_id = 1000 + index;
                        task
                    })
                    .collect();
                worker
            })
            .collect();
        let mut app = App::offline("click-test.seismograph".into());
        app.screen = Screen::Offline {
            path: "click-test.seismograph".into(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        app.runtime_view.worker_selected = 45;
        app.runtime_view.task_selected = 55;
        app.filters.applied =
            super::super::filter::FilterSpec::parse("crate:worker", "", true, super::super::filter::RuntimeStackMode::Event).unwrap();
        let area = Rect::new(0, 0, 180, 60);
        let content = Rect::new(0, 3, 180, 52);
        let panels = app.panels.arrange(MonitorTab::Runtime, content).areas;
        for (kind, column, row) in [
            (MouseEventKind::Down(MouseButton::Left), 5, panels[1].y),
            (MouseEventKind::Drag(MouseButton::Left), 5, 14),
            (MouseEventKind::Up(MouseButton::Left), 5, 14),
        ] {
            app.handle_mouse(mouse_event(kind, column, row), area);
        }
        let panels = app.panels.arrange(MonitorTab::Runtime, content).areas;
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), panels[2].x),
            (MouseEventKind::Drag(MouseButton::Left), 68),
            (MouseEventKind::Up(MouseButton::Left), 68),
        ] {
            app.handle_mouse(mouse_event(kind, column, panels[1].y + 2), area);
        }
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let panels = app.panels.arrange(MonitorTab::Runtime, content).areas;
        assert_eq!(panels[1].width, 68);
        for (pane, target) in [(panels[0], ListTarget::RuntimeWorkers), (panels[1], ListTarget::RuntimeTasks)] {
            let first_row = pane.y + 2;
            let first_column = pane.x + 1;
            assert_eq!(app.panels.rows.at(area, first_column, first_row - 1), None);
            let mut visible = 0;
            for row in first_row..pane.bottom() - 1 {
                let (found, index) = app.panels.rows.at(area, first_column, row).unwrap();
                assert_eq!(found, target);
                assert!(index > 0);
                let text = (pane.x..pane.right())
                    .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                    .collect::<String>();
                let label = if target == ListTarget::RuntimeWorkers {
                    format!("worker-{index:02}")
                } else {
                    format!("#{}", 1000 + index)
                };
                assert!(text.contains(&label), "{target:?} index {index}: {text}");
                visible += 1;
            }
            assert!(visible > 0);
        }
        for row in 55..60 {
            assert_eq!(app.panels.rows.at(area, 1, row), None);
        }
        let (_, index) = app.panels.rows.at(area, panels[1].x + 1, panels[1].y + 2).unwrap();
        app.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), panels[1].x + 1, panels[1].y + 2),
            area,
        );
        assert_eq!(
            (app.runtime_view.task_selected, app.runtime_view.focus),
            (index, RuntimeFocus::Activity)
        );
    }

    #[test]
    fn mouse_heap_table_tracks_actual_visible_rows() {
        let mut capture = representative_capture();
        let native = std::sync::Arc::make_mut(capture.native.as_mut().unwrap());
        native.owners = (1..81)
            .map(|index| seismograph_rallocator::native::Owner {
                id: index,
                leased: true,
                ..Default::default()
            })
            .collect();
        native.owner_count = 80;
        let mut app = App::offline("click-test.seismograph".into());
        app.screen = Screen::Offline {
            path: "click-test.seismograph".into(),
            tab: MonitorTab::Heaps,
            snapshot: Some(capture),
        };
        app.heap_view.bucket_selected = 65;
        let area = Rect::new(0, 0, 180, 40);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let mut found = 0;
        for row in 0..area.height {
            let Some((ListTarget::HeapBuckets, index)) = app.panels.rows.at(area, 1, row) else {
                continue;
            };
            let text = (0..58)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>();
            assert!(text.contains(&format!("0x{index:x}")), "{index}: {text}");
            found += 1;
        }
        assert!(found > 5);
    }

    fn rendered_target(app: &App, target: ListTarget) -> (u16, u16, usize) {
        let area = Rect::new(0, 0, 180, 60);
        (0..area.height)
            .rev()
            .find_map(|row| {
                (0..area.width).find_map(|column| {
                    app.panels
                        .rows
                        .at(area, column, row)
                        .and_then(|(found, index)| (found == target).then_some((column, row, index)))
                })
            })
            .unwrap_or_else(|| panic!("no rendered row for {target:?}"))
    }

    #[test]
    fn mouse_lists_activate_with_the_same_selection_and_enter_semantics() {
        use crossterm::event::{KeyCode, MouseButton, MouseEventKind};

        for (tab, target) in [
            (MonitorTab::Heaps, ListTarget::HeapBuckets),
            (MonitorTab::Allocations, ListTarget::Allocations),
            (MonitorTab::Primitives, ListTarget::PrimitiveTypes),
            (MonitorTab::Primitives, ListTarget::PrimitiveOperations),
            (MonitorTab::Primitives, ListTarget::PrimitiveHotspots),
            (MonitorTab::Threads, ListTarget::Threads),
            (MonitorTab::Threads, ListTarget::ThreadOperations),
            (MonitorTab::Threads, ListTarget::ThreadParticipants),
            (MonitorTab::Threads, ListTarget::ThreadObjects),
            (MonitorTab::Runtime, ListTarget::RuntimeWorkers),
            (MonitorTab::Runtime, ListTarget::RuntimeTasks),
            (MonitorTab::Io, ListTarget::IoResources),
            (MonitorTab::Io, ListTarget::IoOperations),
            (MonitorTab::Cache, ListTarget::CacheTiers),
            (MonitorTab::Cache, ListTarget::CacheOperations),
        ] {
            let make_app = || {
                let mut app = App::offline("click-test.seismograph".into());
                app.screen = Screen::Offline {
                    path: "click-test.seismograph".into(),
                    tab,
                    snapshot: Some(representative_capture()),
                };
                app
            };
            let mut mouse = make_app();
            let mut keyboard = make_app();
            match target {
                ListTarget::PrimitiveOperations => keyboard.primitive_view.focus = PrimitiveFocus::Operations,
                ListTarget::PrimitiveHotspots => keyboard.primitive_view.focus = PrimitiveFocus::Hotspots,
                ListTarget::ThreadOperations => keyboard.thread_view.focus = ThreadFocus::Operations,
                ListTarget::ThreadParticipants => keyboard.thread_view.focus = ThreadFocus::Participants,
                ListTarget::ThreadObjects => keyboard.thread_view.focus = ThreadFocus::Objects,
                ListTarget::RuntimeTasks => keyboard.runtime_view.focus = RuntimeFocus::Tasks,
                ListTarget::IoOperations => keyboard.io_view.focus = IoFocus::Operations,
                ListTarget::CacheOperations => keyboard.cache_view.focus = CacheFocus::Operations,
                _ => {}
            }
            render(&mouse);
            let (column, row, index) = rendered_target(&mouse, target);
            keyboard.handle_key(KeyCode::Up);
            for _ in 0..index {
                keyboard.handle_key(KeyCode::Down);
            }
            keyboard.handle_key(KeyCode::Enter);
            mouse.handle_mouse(
                mouse_event(MouseEventKind::Down(MouseButton::Left), column, row),
                Rect::new(0, 0, 180, 60),
            );
            assert_eq!(
                (
                    mouse.heap_view,
                    mouse.allocation_view,
                    mouse.primitive_view,
                    mouse.thread_view,
                    mouse.runtime_view,
                    mouse.io_view,
                    mouse.cache_view,
                ),
                (
                    keyboard.heap_view,
                    keyboard.allocation_view,
                    keyboard.primitive_view,
                    keyboard.thread_view,
                    keyboard.runtime_view,
                    keyboard.io_view,
                    keyboard.cache_view,
                ),
                "{target:?}",
            );
            assert!(matches!(mouse.screen, Screen::Offline { .. }), "{target:?}");
        }
    }

    #[test]
    fn mouse_browser_uses_the_rendered_scroll_offset_and_connects_once() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let mut app = App::new();
        app.instances = (0..80)
            .map(|process_id| {
                let mut descriptor = descriptor();
                descriptor.process_id = process_id;
                super::super::app::Instance {
                    descriptor,
                    recording: RecordingConfiguration::default(),
                    server_version: None,
                }
            })
            .collect();
        app.selected = 70;
        render(&app);
        let area = Rect::new(0, 0, 180, 60);
        let (_, index) = app.panels.rows.at(area, 1, 1).unwrap();
        assert!(index > 0);
        app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 1, 1), area);
        assert_eq!(app.selected, index);
        assert!(matches!(&app.screen, Screen::Connected { descriptor, .. } if descriptor.process_id == u32::try_from(index).unwrap()));
        app.handle_mouse(mouse_event(MouseEventKind::Up(MouseButton::Left), 1, 1), area);
        assert_eq!(app.selected, index);
    }

    #[test]
    fn native_owner_selection_and_detail_scrolling_are_functional() {
        use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
        let mut app = App::offline("click-test.seismograph".into());
        app.screen = Screen::Offline {
            path: "click-test.seismograph".into(),
            tab: MonitorTab::Heaps,
            snapshot: Some(representative_capture()),
        };
        app.handle_key(KeyCode::Down);
        assert!(render(&app).contains("Small-class slab inventory"));
        app.handle_key(KeyCode::PageDown);
        assert!(app.heap_view.stack_scroll > 0);
        app.handle_key(KeyCode::Down);
        assert!(render(&app).contains("No readable observation"));
        assert_eq!(app.heap_view.stack_scroll, 0);
        render(&app);
        let (column, row, index) = rendered_target(&app, ListTarget::HeapBuckets);
        app.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), column, row),
            Rect::new(0, 0, 180, 60),
        );
        assert_eq!(app.heap_view.bucket_selected, index);
    }

    #[test]
    fn native_wrapped_details_scroll_to_remote_state_on_narrow_terminals() {
        let mut snapshot = crate::native_view::fixture::snapshot();
        let observation = snapshot.owners[0].observation.as_mut().unwrap();
        observation.classes.fill(seismograph_rallocator::native::ClassState {
            object_bytes: 16,
            slab_bytes: 16384,
            capacity: 1024,
            observed_slabs: 1,
            ..Default::default()
        });
        let mut view = App::offline("native-scroll.seismograph".into()).heap_view;
        view.bucket_selected = 1;
        let rows = MouseRows::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut reached_remote = false;
        for scroll in (0..1000).step_by(5) {
            view.stack_scroll = scroll;
            terminal
                .draw(|frame| draw_native(frame, &rows, frame.area(), Some(&snapshot), None, view))
                .unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect();
            if text.contains("Sampled front != back:") {
                reached_remote = true;
                break;
            }
        }
        assert!(
            reached_remote,
            "wrapped native class rows must not make the later remote subsystem unreachable"
        );
    }

    #[test]
    fn wrapped_operation_details_clamp_after_wrapping_without_hiding_the_last_frame() {
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(
                    wrapped_detail_paragraph(
                        &["operation detail ".repeat(100), "application::last_frame".into()],
                        Block::default().borders(Borders::ALL),
                        area,
                        usize::MAX,
                    ),
                    area,
                );
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(text.contains("application::last_frame"));
        assert!(
            text.contains("operation detail"),
            "overscroll clamps to a full final viewport, not a blank panel"
        );
    }

    #[test]
    fn native_inventory_end_navigation_keeps_global_mouse_indices() {
        use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
        let mut snapshot = representative_capture();
        let native = std::sync::Arc::make_mut(snapshot.native.as_mut().unwrap());
        let owner = native.owners[0];
        native.owners = (0..1024)
            .map(|index| seismograph_rallocator::native::Owner {
                id: 0x1000 + index,
                ..owner
            })
            .collect();
        native.owner_count = 1024;
        let mut app = App::offline("large-inventory.seismograph".into());
        app.screen = Screen::Offline {
            path: "large-inventory.seismograph".into(),
            tab: MonitorTab::Heaps,
            snapshot: Some(snapshot),
        };
        app.handle_key(KeyCode::End);
        assert_eq!(app.heap_view.bucket_selected, 1024);
        assert!(render(&app).contains("0x13ff"));
        let (column, row, index) = rendered_target(&app, ListTarget::HeapBuckets);
        assert!(index > 900);
        app.handle_mouse(
            mouse_event(MouseEventKind::Down(MouseButton::Left), column, row),
            Rect::new(0, 0, 180, 60),
        );
        assert_eq!(app.heap_view.bucket_selected, index);
        app.handle_key(KeyCode::Home);
        assert_eq!(app.heap_view.bucket_selected, 0);
        assert!(render(&app).contains("Global backend"));
    }

    #[test]
    fn native_slot_failure_is_explicit_in_owner_selection() {
        use crossterm::event::KeyCode;
        let mut snapshot = representative_capture();
        let native = std::sync::Arc::make_mut(snapshot.native.as_mut().unwrap());
        native.owners[1].source = seismograph_rallocator::native::ObservationSource::Unavailable;
        let mut app = App::offline("slot-failure.seismograph".into());
        app.screen = Screen::Offline {
            path: "slot-failure.seismograph".into(),
            tab: MonitorTab::Heaps,
            snapshot: Some(snapshot),
        };
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Down);
        let output = render(&app);
        for evidence in [
            "UNAVAILABLE",
            "Publication slot System allocation failed",
            "not zero or merely never-observed",
        ] {
            assert!(output.contains(evidence), "missing {evidence}");
        }
    }

    #[test]
    fn allocation_event_mode_renders_operation_identity_and_stack() {
        use crossterm::event::KeyCode;
        let mut app = App::offline("allocation-events.seismograph".into());
        app.screen = Screen::Offline {
            path: "allocation-events.seismograph".into(),
            tab: MonitorTab::Allocations,
            snapshot: Some(representative_capture()),
        };
        app.handle_key(KeyCode::Char('e'));
        assert!(app.allocation_view.events);
        let output = render(&app);
        for label in [
            "Retained allocation/free records",
            "actor recorder thread",
            "View-local lifetime",
            "Captured operation stack",
        ] {
            assert!(output.contains(label), "missing {label}");
        }
        app.handle_key(KeyCode::Down);
        assert_eq!(app.allocation_view.selected, 1);
        app.handle_key(KeyCode::Char('e'));
        assert!(!app.allocation_view.events);
        assert_eq!(app.allocation_view.selected, 0);
    }

    #[test]
    fn mouse_modal_and_non_press_events_do_not_activate_background_rows() {
        use crossterm::event::{KeyCode, MouseButton, MouseEventKind};
        let mut app = App::offline("click-test.seismograph".into());
        app.screen = Screen::Offline {
            path: "click-test.seismograph".into(),
            tab: MonitorTab::Runtime,
            snapshot: Some(representative_capture()),
        };
        let area = Rect::new(0, 0, 180, 60);
        render(&app);
        let (column, row, _) = rendered_target(&app, ListTarget::RuntimeWorkers);
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Down(MouseButton::Right),
        ] {
            app.handle_mouse(mouse_event(kind, column, row), area);
        }
        assert_eq!(app.runtime_view.focus, RuntimeFocus::Workers);
        app.recording_configuration_popup = Some(RecordingConfigurationPopup::new(RecordingConfiguration::default()));
        app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), column, row), area);
        assert_eq!(app.runtime_view.focus, RuntimeFocus::Workers);
        app.recording_configuration_popup = None;
        app.capture_started_at = Some(Instant::now());
        app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), column, row), area);
        assert_eq!(app.runtime_view.focus, RuntimeFocus::Workers);
        app.capture_started_at = None;
        if let Screen::Offline {
            snapshot: Some(snapshot), ..
        } = &mut app.screen
        {
            snapshot.filter_index = Some(std::sync::Arc::new(super::super::filter_index::FilterIndex::new(
                seismograph::snapshot::DecodedSnapshot::default(),
                None,
                None,
                Vec::new(),
                std::collections::HashSet::new(),
            )));
        }
        app.handle_key(KeyCode::Char('F'));
        assert!(app.filters.popup.is_some());
        app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), column, row), area);
        assert_eq!(app.runtime_view.focus, RuntimeFocus::Workers);
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // The fixture's impossible screen mismatch is not product behavior.
    fn offline_tabs_reuse_the_live_snapshot_renderers() {
        for tab in [
            MonitorTab::Heaps,
            MonitorTab::Allocations,
            MonitorTab::Primitives,
            MonitorTab::Threads,
            MonitorTab::Runtime,
            MonitorTab::Io,
            MonitorTab::Cache,
        ] {
            let mut app = App::new();
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab,
                snapshot: Some(representative_capture()),
            };
            let live = render(&app);
            let Screen::Connected { snapshot, .. } = app.screen else {
                unreachable!()
            };
            app.screen = Screen::Offline {
                path: "blob capture.seismograph".into(),
                tab,
                snapshot,
            };
            let offline = render(&app);
            assert!(!live.contains("Monitor (seismograph_cli):"), "{tab:?}");
            assert!(!live.contains("Server (seismograph):"), "{tab:?}");
            assert!(!offline.contains("Monitor (seismograph_cli):"), "{tab:?}");
            assert!(!offline.contains("Server (seismograph):"), "{tab:?}");
            // Only the tab border and footer differ; compare every content cell.
            let content = |text: &str| text.chars().skip(180 * 3).take(180 * 56).collect::<String>();
            assert_eq!(content(&offline), content(&live), "{tab:?}");
        }
    }

    #[test]
    fn offline_loading_errors_and_info_are_not_live_process_data() {
        let mut app = App::offline("blob capture.seismograph".into());
        assert!(render(&app).contains("Loading and decoding snapshot"));
        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Heaps;
        }
        assert!(render(&app).contains("Loading and decoding snapshot"));
        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Info;
        }
        app.snapshot_error = Some("invalid snapshot test error".into());
        assert!(render(&app).contains("invalid snapshot test error"));
        app.snapshot_error = None;
        app.finish_offline_load(representative_capture());
        let output = render(&app);
        assert!(output.contains("blob capture.seismograph"));
        assert!(output.contains("Capture time: not recorded"));
        assert!(!output.contains("Waiting for the first"));
        assert!(!output.contains("Recording configuration"));
    }

    #[test]
    fn offline_info_reports_event_loss_and_missing_heap_data() {
        let mut capture = representative_capture();
        capture.primitives.total_events = 12;
        capture.primitives.lost_events = 3;
        capture.heap_error = Some("allocator source was not recorded".into());
        let threads = capture.threads.threads.len();
        let mut app = App::offline("runtime-only.seismograph".into());
        app.finish_offline_load(capture);
        let output = render(&app);
        assert_eq!(
            (
                output.contains(&format!("Source events: 12 accepted · 3 overwritten · {threads} threads")),
                output.contains("allocator source was not recorded"),
            ),
            (true, true),
        );
    }

    #[test]
    fn offline_info_without_a_snapshot_omits_source_statistics() {
        let app = App::offline("pending.seismograph".into());
        let view = ConnectedView {
            origin: ViewOrigin::Offline(std::path::Path::new("pending.seismograph")),
            recording_unknown: false,
            monitor_version: env!("CARGO_PKG_VERSION"),
            server_version: "unknown (not recorded in snapshot)",
            tab: MonitorTab::Info,
            snapshot: None,
            snapshot_error: None,
            heap_view: app.heap_view,
            allocation_view: app.allocation_view,
            primitive_view: app.primitive_view,
            thread_view: app.thread_view,
            runtime_view: app.runtime_view,
            io_view: app.io_view,
            cache_view: app.cache_view,
            panels: &app.panels,
            activity_samples: &app.activity_samples,
            recorder_statistics: None,
            live_activity: &app.live_activity,
            info_thread_selected: app.info_thread_selected,
        };
        let output = render_frame(|frame| draw_snapshot_info(frame, frame.area(), &view));
        assert_eq!(
            (
                output.contains("pending.seismograph"),
                output.contains("Offline snapshot · read-only"),
                output.contains("Source events:"),
            ),
            (true, true, false),
        );
    }

    #[test]
    fn snapshot_time_does_not_invent_missing_capture_metadata() {
        let mut capture = representative_capture();
        capture.captured_at = None;
        capture.captured_instant = None;
        assert_eq!(snapshot_time(&capture), "capture time not recorded");
    }

    #[test]
    fn live_info_shows_class_rates_and_graphical_threads_without_capturing_events() {
        use crossterm::event::{MouseButton, MouseEventKind};
        use seismograph_protocol::message::{EventClassCounts, RecorderActivity, ThreadRecorderStatistics};

        for (width, height) in [(180, 60), (80, 24)] {
            let mut app = App::new();
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Info,
                snapshot: None,
            };
            let start = Instant::now();
            for step in [1, 2] {
                app.live_activity.record(
                    RecorderActivity {
                        session_id: 1,
                        class_events: EventClassCounts {
                            allocations: step * 5,
                            runtime_tasks: step * 9,
                            ..EventClassCounts::default()
                        },
                        threads: (1..=3)
                            .map(|thread_id| ThreadRecorderStatistics {
                                thread_id,
                                name: format!("live-worker-{thread_id}"),
                                total_events: step * 16,
                                retained_events: step * 16,
                                event_capacity: 64,
                                ..ThreadRecorderStatistics::default()
                            })
                            .collect(),
                        ..RecorderActivity::default()
                    },
                    start + Duration::from_secs(step),
                );
            }
            let area = Rect::new(0, 0, width, height);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            let text = (0..height)
                .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            for expected in [
                "Live event-class rates",
                "Allocations 5/s",
                "Runtime 9/s",
                "Thread activity",
                "32/64",
                "50%",
                "16/s",
            ] {
                assert!(text.contains(expected), "{width}x{height}: missing {expected}\n{text}");
            }
            assert!(text.contains(if width >= 180 { "live-worker-1" } else { "#1 live" }));
            assert!(!text.contains("Source accepted/overwritten counters span"));
            let target = (0..height)
                .find_map(|y| {
                    (0..width).find_map(|x| (app.panels.rows.at(area, x, y) == Some((ListTarget::InfoThreads, 1))).then_some((x, y)))
                })
                .unwrap();
            app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), target.0, target.1), area);
            assert_eq!(app.info_thread_selected, 1);
            app.handle_mouse(mouse_event(MouseEventKind::ScrollDown, target.0, target.1), area);
            assert_eq!(app.info_thread_selected, 2);
            assert!(matches!(app.screen, Screen::Connected { snapshot: None, .. }));
        }
    }

    #[test]
    fn info_keeps_metrics_without_explanatory_scope_notes_in_both_modes() {
        let mut app = App::offline("scope.seismograph".into());
        app.finish_offline_load(representative_capture());
        let offline = render(&app);
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Info,
            snapshot: Some(representative_capture()),
        };
        let live = render(&app);
        for note in [
            "Accepted/overwritten counts exclude",
            "not allocation populations",
            "not session/workload deltas",
            "not per-segment occupancy",
        ] {
            assert!(!offline.contains(note), "offline: {note}");
            assert!(!live.contains(note), "live: {note}");
        }
        assert!(live.contains("All-class source accepted"));
        assert!(live.contains("All-class source overwritten"));
    }

    #[test]
    fn allocation_labels_do_not_claim_unmatched_records_are_live() {
        let capture = representative_capture();
        let output = render_frame(|frame| {
            let app = App::new();
            let areas = app.panels.arrange(MonitorTab::Allocations, frame.area()).areas;
            draw_allocations(
                frame,
                &app.panels.rows,
                [areas[0], areas[1]],
                capture.allocations.as_ref(),
                None,
                app.allocation_view,
            );
        });
        assert!(output.contains("Unmatched B"));
        assert!(output.contains("all-class source:"));
        assert!(output.contains("not proven live allocations or leaks, even with zero overwrites"));
        assert!(!output.contains("Live bytes"));
    }

    fn render_debug(draw: impl FnOnce(&mut ratatui::Frame<'_>)) -> String {
        let backend = TestBackend::new(180, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(draw).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    fn stable_digest(value: &str) -> (usize, u64) {
        let hash = value.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            hash.wrapping_mul(0x0000_0100_0000_01b3) ^ u64::from(byte)
        });
        (value.len(), hash)
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
    fn resized_panels_render_on_small_terminals() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        let mut app = App::offline("capture.seismograph".into());
        app.finish_offline_load(representative_capture());
        for tab in [
            MonitorTab::Info,
            MonitorTab::Heaps,
            MonitorTab::Allocations,
            MonitorTab::Primitives,
            MonitorTab::Threads,
            MonitorTab::Runtime,
            MonitorTab::Io,
            MonitorTab::Cache,
        ] {
            if let Screen::Offline { tab: selected, .. } = &mut app.screen {
                *selected = tab;
            }
            for (width, height) in [(100, 40), (35, 12), (12, 5)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let terminal_area = Rect::new(0, 0, width, height);
                let [body, ..] = app.screen_areas(terminal_area);
                let [_, content] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(body);
                let second = app.panels.arrange(tab, content).areas[1];
                let (column, row) = if matches!(tab, MonitorTab::Threads | MonitorTab::Io | MonitorTab::Cache) {
                    (second.x, second.y.saturating_add(1))
                } else {
                    (second.x.saturating_add(1), second.y)
                };
                for (kind, column, row) in [
                    (MouseEventKind::Down(MouseButton::Left), column, row),
                    (MouseEventKind::Drag(MouseButton::Left), width * 3 / 4, height * 3 / 4),
                ] {
                    app.handle_mouse(
                        MouseEvent {
                            kind,
                            column,
                            row,
                            modifiers: KeyModifiers::NONE,
                        },
                        terminal_area,
                    );
                }
                terminal.draw(|frame| app.draw(frame)).unwrap();
                assert_eq!(terminal.backend().buffer().area, terminal_area);
            }
        }
    }

    #[test]
    fn disabled_recording_state_is_readable() {
        assert_eq!(recording_label(RecordingPolicy::default()), "off");
    }

    #[test]
    fn runtime_medians_distinguish_missing_samples_from_measured_zero() {
        for (median, expected, samples) in [(None, "-", Vec::new()), (Some(0), "0ns", vec![0]), (Some(7), "7ns", vec![7; 3])] {
            let mut capture = representative_capture();
            let runtime = &mut capture.runtime;
            runtime.workers.truncate(1);
            runtime.workers[0].tasks.truncate(1);
            let task = &mut runtime.workers[0].tasks[0];
            task.metrics.poll_count = u64::try_from(samples.len()).unwrap();
            task.metrics.median_poll_nanos = median;
            task.metrics.max_poll_nanos = median;
            task.metrics.poll_samples = samples;
            let output = render_frame(|frame| {
                runtime_ui::draw(
                    frame,
                    &MouseRows::default(),
                    [
                        Rect::new(0, 0, 180, 4),
                        Rect::new(0, 4, 180, 6),
                        Rect::default(),
                        Rect::new(0, 10, 180, 50),
                        Rect::default(),
                    ],
                    Some(&capture),
                    None,
                    App::new().runtime_view,
                );
            });
            assert!(output.contains("Median poll"));
            assert!(output.contains(expected));
            assert!(!output.contains("Median resume"));
            assert!(!output.contains("Median stall"));
        }
    }

    #[test]
    fn runtime_task_columns_align_across_states_and_large_values() {
        use super::super::data::RuntimeTaskSort;

        let mut capture = representative_capture();
        let runtime = &mut capture.runtime;
        runtime.workers.truncate(1);
        let template = runtime.workers[0].tasks[0].clone();
        runtime.workers[0].tasks = [(1, "Pending", 12), (2, "Materialized", 345), (u64::MAX, "Completed", u64::MAX)]
            .map(|(task_id, state, poll_count)| {
                let mut task = template.clone();
                task.task_id = task_id;
                task.state = state.into();
                task.activity.state = state.into();
                task.metrics.poll_count = poll_count;
                task.metrics.median_poll_nanos = Some(11);
                task.metrics.max_poll_nanos = Some(22);
                task
            })
            .to_vec();
        let mut view = App::new().runtime_view;
        view.focus = RuntimeFocus::Tasks;
        view.task_sort = RuntimeTaskSort::Task;
        view.task_sort_descending = false;
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
        let mouse_rows = MouseRows::default();
        terminal
            .draw(|frame| {
                mouse_rows.begin(frame.area());
                runtime_ui::draw(
                    frame,
                    &mouse_rows,
                    [
                        Rect::new(0, 0, 120, 3),
                        Rect::new(0, 3, 120, 8),
                        Rect::default(),
                        Rect::default(),
                        Rect::default(),
                    ],
                    Some(&capture),
                    None,
                    view,
                );
            })
            .unwrap();
        let line = |y| (0..120).map(|x| terminal.backend().buffer()[(x, y)].symbol()).collect::<String>();
        let header = line(4);
        let rows = [line(5), line(6), line(7)];
        let expected = (
            header.find("State").unwrap(),
            header.find("Median poll").unwrap() + "Median poll".len(),
            header.find("Max poll").unwrap() + "Max poll".len(),
        );
        assert_eq!(
            std::array::from_fn::<_, 3, _>(|index| {
                let row = &rows[index];
                (
                    row.find(["Pending", "Materialized", "Completed"][index]).unwrap(),
                    row.find("11ns").unwrap() + "11ns".len(),
                    row.find("22ns").unwrap() + "22ns".len(),
                )
            }),
            [expected; 3],
        );
        assert_eq!(
            [5, 6, 7].map(|y| mouse_rows.at(Rect::new(0, 0, 120, 12), 2, y)),
            [0, 1, 2].map(|index| Some((ListTarget::RuntimeTasks, index))),
        );
    }

    #[test]
    fn future_size_column_shows_exact_bytes_and_unknown() {
        let mut capture = representative_capture();
        let template = capture.runtime.workers[0].tasks[0].clone();
        capture.runtime.workers.truncate(1);
        capture.runtime.workers[0].tasks = [Some(0), Some(16384), None]
            .into_iter()
            .enumerate()
            .map(|(index, future_size_bytes)| super::super::data::RuntimeTaskSummary {
                task_id: u64::try_from(index).unwrap() + 1,
                future_size_bytes,
                ..template.clone()
            })
            .collect();
        let mut app = App::offline("sizes.seismograph".into());
        app.screen = Screen::Offline {
            path: "sizes.seismograph".into(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        app.runtime_view.focus = RuntimeFocus::Tasks;
        app.runtime_view.task_sort = super::super::data::RuntimeTaskSort::FutureSize;
        app.runtime_view.task_sort_descending = false;
        let text = render(&app);
        assert!(text.contains("Future B"));
        let lines = text.chars().collect::<Vec<_>>();
        let rows = lines
            .chunks(180)
            .map(|line| line.iter().collect::<String>())
            .filter(|line| line.contains("Completed") && line.contains('#'))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].contains("#1") && rows[0].contains(" 0 "));
        assert!(rows[1].contains("#2") && rows[1].contains("16384"));
        assert!(rows[2].contains("#3") && rows[2].contains(" - "));
    }

    fn task_event_capture() -> Box<CapturedSnapshot> {
        let poll = |sequence, kind, task, duration| {
            runtime_event(
                1,
                sequence,
                kind,
                EventPayload::Runtime(RuntimeEvent {
                    runtime_id: RuntimeId::from_raw(1).unwrap(),
                    worker_id: WorkerId::from_raw(1),
                    subject_id: task,
                    related_id: 0,
                    value_0: duration,
                    value_1: 0,
                }),
                &[0x7770, 0x9000, 0x9001],
            )
        };
        let operation = |sequence, kind| {
            let frame = if kind == EventKind::MutexContention { 0x1001 } else { 0x1000 };
            runtime_event(
                1,
                sequence,
                kind,
                EventPayload::Object(ObjectId::new(8)),
                &[frame, 0x2000, 0x9000, 0x9001],
            )
        };
        let decoded = seismograph::snapshot::DecodedSnapshot {
            events: Events {
                clock: EventClock::ProcessMonotonic,
                total_events: 8,
                recording: RecordingPolicies {
                    runtime_tasks: seismograph::recorder::RecordingPolicy::all(true),
                    general_events: seismograph::recorder::RecordingPolicy::all(true),
                    ..RecordingPolicies::default()
                },
                threads: vec![ThreadLog {
                    thread_id: ThreadId::new(1),
                    total_events: 8,
                    lost_events: 0,
                    name: "worker".into(),
                }],
                events: vec![
                    poll(1, EventKind::TaskPollStarted, 1, 0),
                    operation(2, EventKind::MutexAccess),
                    operation(3, EventKind::MutexContention),
                    poll(4, EventKind::TaskPollFinished, 1, 300),
                    poll(5, EventKind::TaskPollStarted, 2, 0),
                    operation(6, EventKind::MutexAccess),
                    operation(7, EventKind::MutexRelease),
                    poll(8, EventKind::TaskPollFinished, 2, 300),
                ],
                ..Events::default()
            },
            ..seismograph::snapshot::DecodedSnapshot::default()
        };
        let addresses = [
            (0x1000, "app::operation"),
            (0x1001, "app::contended_operation"),
            (0x2000, "app::task_future::poll"),
            (0x7770, "seismograph_runtime::worker::poll_started"),
            (0x9000, "oxidizer_executor::ExecutorCore::poll"),
            (0x9001, "oxidizer_executor::ExecutorCore::run"),
        ]
        .map(|(address, symbol)| {
            AddressLookup::from_fields(AddressLookupFields {
                address,
                symbol: Some(symbol.into()),
                filename: None,
                line: None,
                column: None,
            })
        });
        let runtime = super::super::data::RuntimeSnapshot::from_events(&decoded, &addresses, None);
        assert_eq!(runtime.task_events.tasks.get(&(1, 1)).map(|task| task.inferred_events), Some(2),);
        let mut capture = representative_capture();
        capture.runtime = runtime.runtime;
        capture.task_events = runtime.task_events;
        capture
    }

    #[test]
    fn dashboard_occurrences_and_stacks_include_only_this_task_and_operation() {
        use crossterm::event::KeyCode;

        let mut app = App::offline("tasks.seismograph".into());
        app.screen = Screen::Offline {
            path: "tasks.seismograph".into(),
            tab: MonitorTab::Runtime,
            snapshot: Some(task_event_capture()),
        };
        app.runtime_view.focus = RuntimeFocus::Tasks;
        app.handle_key(KeyCode::Char('e'));
        let text = render(&app);
        assert!(text.contains("2 inferred events across workers"));
        assert!(text.contains("Mutex"));
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.runtime_view.events.focus, TaskEventsFocus::Occurrences);
        let text = render(&app);
        assert!(text.contains("app::task_future::poll"));
        assert!(text.contains("Task-relative application stack"));
        assert!(!text.contains("oxidizer_executor::ExecutorCore::run"));
        app.handle_key(KeyCode::Char('f'));
        assert!(render(&app).contains("oxidizer_executor::ExecutorCore::run"));
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Down);
        let text = render(&app);
        assert_eq!(app.runtime_view.events.event_selected, 0);
        assert!(text.contains("MutexAccess"));
        assert!(!text.contains("MutexRelease"));
        assert!(text.contains("Worker Activity"));
        assert!(text.contains("Statistics"));
        assert!(!text.contains("Object history"));
        let rows = text.chars().collect::<Vec<_>>();
        let lines = rows.chunks(180).map(|row| row.iter().collect::<String>()).collect::<Vec<_>>();
        let header = lines.iter().find(|line| line.contains("Task occurrences")).unwrap();
        assert!(header.find("Operations").unwrap() < header.find("Task occurrences").unwrap());
        assert!(header.find("Task occurrences").unwrap() < header.find("Event stack").unwrap());
        let area = Rect::new(0, 0, 180, 60);
        let occurrences = (0..60)
            .filter_map(|y| {
                (0..180).find_map(|x| match app.panels.rows.at(area, x, y) {
                    Some((ListTarget::TaskOccurrences, index)) => Some(index),
                    _ => None,
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(occurrences, [0]);
        app.handle_key(KeyCode::Backspace);
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Enter);
        let text = render(&app);
        assert!(text.contains("MutexContention") && text.contains("app::contended_operation"));
        assert!(!text.contains("app::operation"));
        assert_eq!(
            (app.runtime_view.events.event_selected, app.runtime_view.events.stack_scroll),
            (0, 0)
        );
    }

    #[test]
    fn dashboard_mouse_targets_toggle_histograms_and_select_task_occurrences_at_all_sizes() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let mut app = App::offline("tasks.seismograph".into());
        app.screen = Screen::Offline {
            path: "tasks.seismograph".into(),
            tab: MonitorTab::Runtime,
            snapshot: Some(task_event_capture()),
        };
        for (width, height) in [(180, 60), (80, 24)] {
            app.runtime_view.focus = RuntimeFocus::Activity;
            app.runtime_view.task_histogram = TaskHistogram::Poll;
            app.runtime_view.events.focus = TaskEventsFocus::Operations;
            let area = Rect::new(0, 0, width, height);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let target = (0..height)
                .find_map(|y| {
                    (0..width).find_map(|x| {
                        (app.panels.rows.at(area, x, y) == Some((ListTarget::RuntimeHistogram(TaskHistogram::Ready), 0))).then_some((x, y))
                    })
                })
                .unwrap();
            app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), target.0, target.1), area);
            assert_eq!(app.runtime_view.task_histogram, TaskHistogram::Ready);
            app.handle_key(crossterm::event::KeyCode::Char('e'));
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let target = (0..height)
                .find_map(|y| {
                    (0..width).find_map(|x| (app.panels.rows.at(area, x, y) == Some((ListTarget::TaskOperations, 0))).then_some((x, y)))
                })
                .unwrap();
            app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), target.0, target.1), area);
            assert_eq!(app.runtime_view.events.focus, TaskEventsFocus::Occurrences);
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let target = (0..height)
                .find_map(|y| {
                    (0..width).find_map(|x| (app.panels.rows.at(area, x, y) == Some((ListTarget::TaskOccurrences, 0))).then_some((x, y)))
                })
                .unwrap();
            app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), target.0, target.1), area);
            assert_eq!(
                (app.runtime_view.events.focus, app.runtime_view.events.event_selected),
                (TaskEventsFocus::Occurrences, 0)
            );
        }
    }

    #[test]
    fn empty_runtime_view_explains_missing_instrumentation() {
        let mut capture = representative_capture();
        capture.runtime = super::super::data::RuntimeMonitorSnapshot::default();
        let mut app = App::new();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        let text = render(&app);
        assert!(text.contains("0 runtime events"));
        assert!(text.contains("No runtime source or runtime events"));
        assert!(text.contains("the recorder alone does not instrument executors"));
        if let Screen::Connected {
            snapshot: Some(snapshot), ..
        } = &mut app.screen
        {
            snapshot.runtime.source_present = true;
        }
        let text = render(&app);
        assert!(text.contains("capture instrumented activity"));
        assert!(!text.contains("No runtime source or runtime events"));
    }

    #[test]
    fn empty_filtered_runtime_view_explains_selection_not_instrumentation() {
        for source_present in [false, true] {
            let mut capture = representative_capture();
            capture.runtime = super::super::data::RuntimeMonitorSnapshot {
                source_present,
                ..super::super::data::RuntimeMonitorSnapshot::default()
            };
            capture.filter_summary.active = true;
            let mut app = App::new();
            app.screen = Screen::Offline {
                path: "filtered.seismograph".into(),
                tab: MonitorTab::Runtime,
                snapshot: Some(capture),
            };
            let text = render(&app);
            assert!(text.contains("No matching runtime activity."));
            assert!(text.contains("Press F to change or clear stack filters"));
            assert!(!text.contains("No runtime source or runtime events"));
            assert!(!text.contains("instrument executors"));
            assert!(!text.contains("Capture while instrumented"));
        }
    }

    #[test]
    fn unassigned_runtime_tasks_render_without_inventing_worker_metrics() {
        let mut capture = representative_capture();
        let worker = &mut capture.runtime.workers[0];
        worker.worker_id = None;
        worker.thread_id = None;
        worker.role = "Unbound".into();
        worker.observed_tasks = 0;
        worker.window = None;
        worker.metrics.executing_fraction = None;
        worker.metrics.poll_count = 0;
        worker.metrics.median_poll_nanos = None;
        worker.metrics.max_poll_nanos = None;
        worker.metrics.polls.clear();
        worker.metrics.poll_samples.clear();
        let mut app = App::new();
        app.screen = Screen::Offline {
            path: "capture.seismograph".into(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        let text = render(&app);
        assert!(text.contains("unassigned"));
        assert!(text.contains("Unbound"));
        assert!(text.contains("Task Statistics"));
        assert!(text.contains("Window unobserved"));
        assert!(text.contains("No completed samples"));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one render fixture exercises wide and compact geometry with matching mouse navigation"
    )]
    fn runtime_layouts_render_observed_execution_and_preserve_mouse_navigation() {
        use crossterm::event::{KeyCode, MouseButton, MouseEventKind};

        use super::super::runtime_timeline::{Interval, TimeWindow};

        for (width, height) in [(80, 24), (180, 60)] {
            let mut capture = representative_capture();
            capture.runtime.workers.truncate(1);
            let worker = &mut capture.runtime.workers[0];
            worker.window = Some(TimeWindow {
                start: 100,
                end: 12_000_000_200,
            });
            worker.observed_tasks = 1;
            worker.metrics.polls = vec![Interval {
                start: 200,
                end: 12_000_000_200,
            }];
            worker.metrics.executing_fraction = Some(12_000_000_000.0 / 12_000_000_100.0);
            worker.metrics.poll_count = 0;
            worker.metrics.median_poll_nanos = None;
            worker.metrics.max_poll_nanos = None;
            worker.metrics.poll_samples.clear();
            worker.tasks.truncate(1);
            let task = &mut worker.tasks[0];
            task.state = "Running".into();
            task.metrics.polls = worker.metrics.polls.clone();
            task.metrics.ready = vec![Interval { start: 100, end: 200 }].into();
            task.metrics.poll_count = 0;
            task.metrics.median_poll_nanos = None;
            task.metrics.max_poll_nanos = None;
            task.metrics.executing_fraction = worker.metrics.executing_fraction;
            task.metrics.poll_samples.clear();
            task.metrics.ready_samples = vec![0, 3, 7, 63].into();
            task.activity.state = "Running".into();
            task.activity.running_for = Some(12_000_000_000);
            task.activity.ready_for = None;
            task.activity.repoll_requested = true;
            let mut app = App::offline("runtime-ui.seismograph".into());
            app.screen = Screen::Offline {
                path: "runtime-ui.seismograph".into(),
                tab: MonitorTab::Runtime,
                snapshot: Some(capture),
            };
            let area = Rect::new(0, 0, width, height);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            for (focus, target, next) in [
                (RuntimeFocus::Workers, ListTarget::RuntimeWorkers, RuntimeFocus::Tasks),
                (RuntimeFocus::Tasks, ListTarget::RuntimeTasks, RuntimeFocus::Activity),
            ] {
                terminal.draw(|frame| app.draw(frame)).unwrap();
                assert_eq!(app.runtime_view.focus, focus);
                let (column, row) = (0..height)
                    .find_map(|row| {
                        (0..width).find_map(|column| (app.panels.rows.at(area, column, row) == Some((target, 0))).then_some((column, row)))
                    })
                    .unwrap();
                app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), column, row), area);
                assert_eq!(app.runtime_view.focus, next);
            }
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let text = (0..height)
                .map(|row| {
                    (0..width)
                        .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            println!("{width}x{height} Runtime Activity:\n{text}");
            for expected in [
                "Running for 12s",
                "State global · repoll",
                "Window 12.00s · lower bound",
                "Ready global",
                "No completed samples",
                "log10 100s+",
            ] {
                assert!(text.contains(expected), "{width}x{height}: missing {expected}\n{text}");
            }
            assert!(!text.contains("Spawn Stack"));
            let (column, row, index) = (0..height)
                .rev()
                .find_map(|row| {
                    (0..width).find_map(|column| {
                        app.panels
                            .rows
                            .at(area, column, row)
                            .and_then(|(target, index)| (target == ListTarget::RuntimeActivity).then_some((column, row, index)))
                    })
                })
                .unwrap();
            app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), column, row), area);
            assert_eq!(app.runtime_view.activity_scroll, index);
            let before = app.runtime_view;
            app.handle_key(KeyCode::F(1));
            terminal.draw(|frame| app.draw(frame)).unwrap();
            assert!(app.help.is_some());
            let help_text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            assert!(help_text.contains("F1 Help: Runtime task details"));
            app.handle_key(KeyCode::F(1));
            assert_eq!(app.runtime_view, before);
            app.handle_key(KeyCode::Backspace);
            assert_eq!(app.runtime_view.focus, RuntimeFocus::Tasks);
            app.handle_key(KeyCode::Backspace);
            assert_eq!(app.runtime_view.focus, RuntimeFocus::Workers);
        }
    }

    #[test]
    fn enabled_recording_state_is_readable() {
        assert_eq!(
            recording_label(RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                ..Default::default()
            }),
            "custom"
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
            "on"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires the host realtime clock and local timezone")]
    fn snapshot_age_uses_compact_units() {
        assert_eq!(format_age(Duration::from_secs(125)), "2m ago");

        let snapshot = representative_capture();
        let local: DateTime<Local> = snapshot.captured_at.unwrap().into();
        let current = snapshot_time(&snapshot);
        assert!(current.starts_with(&local.format("%H:%M:%S").to_string()));
        assert!(current.ends_with(" ago)"));
        assert_eq!(
            snapshot_time_at(&snapshot, snapshot.captured_instant.unwrap() + Duration::from_secs(125)),
            format!("{} (2m ago)", local.format("%H:%M:%S"))
        );
    }

    #[test]
    fn pure_formatters_cover_boundaries() {
        assert_eq!(
            [
                format_runtime_duration(1),
                format_runtime_duration(1_000),
                format_runtime_duration(1_234),
                format_runtime_duration(1_000_000),
                format_runtime_duration(1_000_000_000),
                format_event_loss(0, 0),
                format_event_loss(1, 3),
                format_hit_rate(7, 16),
                format_age(Duration::from_secs(1)),
                format_age(Duration::from_secs(59)),
                format_age(Duration::from_mins(1)),
                format_age(Duration::from_hours(1)),
                format_count(1_234_567),
                format_bytes(1),
                format_bytes(1_024),
                format_bytes(1_536),
                format_bytes(1_048_576),
                format_bytes(1_073_741_824),
            ],
            [
                "1ns".to_owned(),
                "1.00us".to_owned(),
                "1.23us".to_owned(),
                "1.00ms".to_owned(),
                "1.00s".to_owned(),
                "0.0%".to_owned(),
                "33.3%".to_owned(),
                "43.7%".to_owned(),
                "1s ago".to_owned(),
                "59s ago".to_owned(),
                "1m ago".to_owned(),
                "1h ago".to_owned(),
                "1,234,567".to_owned(),
                "1 B".to_owned(),
                "1.00 KiB".to_owned(),
                "1.50 KiB".to_owned(),
                "1.00 MiB".to_owned(),
                "1.00 GiB".to_owned(),
            ]
        );
    }

    #[test]
    fn labels_and_visual_helpers_cover_empty_and_active_states() {
        assert_eq!(
            (
                thread_label(7, "", 3),
                thread_label(7, "worker", 5),
                recording_configuration_label(RecordingConfiguration::default()),
            ),
            ("#7".into(), "#7 wo".into(), "off",)
        );
        assert_eq!(
            (
                centered_rect(Rect::new(10, 20, 100, 40), 60, 10),
                centered_rect(Rect::new(10, 20, 20, 4), 60, 10),
                row_is_selected(7, 3, 10),
                row_is_selected(7, 2, 10),
                line_text(&primitive_selection_line(Line::from("row"), true, true)),
                line_text(&metric_line("Metric", "42".into())),
                line_text(&recording_policy_line(
                    "Policy",
                    RecordingPolicy {
                        enabled: true,
                        capture_backtraces: true,
                        sampling_one_in: 8,
                    },
                )),
                line_text(&browse_footer("ready")),
                key_span("key").content.into_owned(),
                key_style(),
            ),
            (
                Rect::new(30, 35, 60, 10),
                Rect::new(10, 20, 20, 4),
                true,
                false,
                "row".to_owned(),
                "Metric: 42".to_owned(),
                "Policy: custom; backtraces on; sample 1/8 (12.5%)".to_owned(),
                " F1 help  ready  ↑/↓ select  Enter connect  r refresh  q quit".to_owned(),
                "key".to_owned(),
                Style::default().fg(KEY_COLOR).add_modifier(Modifier::BOLD),
            )
        );

        let mut mixed = RecordingConfiguration::default();
        mixed.allocations.enabled = true;
        assert_eq!(recording_configuration_label(mixed), "custom");
        mixed.allocations.capture_backtraces = true;
        assert_eq!(recording_configuration_label(mixed), "on");
        mixed.general_events.enabled = true;
        assert_eq!(recording_configuration_label(mixed), "custom");
        assert_eq!(
            [
                IoResourceKind::File,
                IoResourceKind::TcpStream,
                IoResourceKind::TcpListener,
                IoResourceKind::NamedPipe,
                IoResourceKind::WinHttpRequest,
                IoResourceKind::Other,
            ]
            .map(io_resource_kind_label),
            ["File", "TCP stream", "TCP listener", "Named pipe", "WinHTTP", "Other"]
        );
        assert_eq!(
            [
                IoOutcome::Pending,
                IoOutcome::Success,
                IoOutcome::EndOfStream,
                IoOutcome::Canceled,
                IoOutcome::Error,
            ]
            .map(io_outcome_label),
            ["pending", "success", "end of stream", "canceled", "error"]
        );
    }

    #[test]
    fn browser_capture_and_configuration_popups_render() {
        let mut app = App::new();
        assert!(render(&app).contains("Seismograph applications"));
        app.instances.push(super::super::app::Instance {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            server_version: None,
        });
        app.status = "ready".into();
        app.capture_started_at = Some(Instant::now().checked_sub(Duration::from_millis(500)).unwrap());
        app.capture_step = Some(CaptureStep::Decode);
        assert!(render(&app).contains("Snapshot"));
        app.recording_configuration_popup = Some(RecordingConfigurationPopup::new(RecordingConfiguration::default()));

        let rendered = render(&app);

        assert!(rendered.contains("worker (west)"));
        assert!(rendered.contains("Recording configuration"));
    }

    #[test]
    fn recording_popup_hides_custom_settings_and_restores_them_when_reselected() {
        use crossterm::event::KeyCode;

        let recording = RecordingConfiguration {
            allocations: RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                sampling_one_in: 8,
            },
            ..Default::default()
        };
        let mut app = App::new();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording,
            tab: MonitorTab::Info,
            snapshot: None,
        };
        app.handle_key(KeyCode::Char('c'));
        let render_popup = |app: &App| {
            render_frame(|frame| App::draw_recording_configuration_popup(frame, app.recording_configuration_popup.unwrap()))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let custom = render_popup(&app);
        app.handle_key(KeyCode::Left);
        let on = render_popup(&app);
        app.handle_key(KeyCode::Left);
        let off = render_popup(&app);
        app.handle_key(KeyCode::Right);
        app.handle_key(KeyCode::Right);
        let restored = render_popup(&app);

        assert_eq!(
            (
                custom.contains("Allocations custom"),
                custom.contains("Backtraces on"),
                custom.contains("Sampling 1/8 (12.5%)"),
                on.contains("Allocations on"),
                on.contains("Backtraces"),
                on.contains("Sampling"),
                off.contains("Allocations off"),
                off.contains("Backtraces"),
                off.contains("Sampling"),
                restored,
            ),
            (true, true, true, true, false, false, true, false, false, custom)
        );
    }

    #[cfg_attr(miri, ignore)]
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
            server_version: None,
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
        assert!(render(&app).contains("No related retained thread activity"));

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
        capture.runtime.workers[0].tasks[0].activity.state = "Running".into();
        capture.runtime.workers[0].tasks[0].activity.running_for = Some(5_000_000_000);
        app.runtime_view.focus = RuntimeFocus::Activity;
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("Running for 5s"));

        let mut capture = representative_capture();
        capture.runtime.workers[0].tasks[0].spawn_stack = vec!["app::spawn".into()];
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        let rendered = render(&app);
        assert!(rendered.contains("Task Statistics"));
        assert!(!rendered.contains("app::spawn"));
        assert!(!rendered.contains("Backtrace not captured"));

        let mut capture = representative_capture();
        capture.runtime.workers[0].tasks[0].spawn_stack.clear();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Runtime,
            snapshot: Some(capture),
        };
        app.runtime_view.task_histogram = TaskHistogram::Ready;
        assert!(render(&app).contains("Wake-to-poll (raw)"));

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
        capture.io.resources[0].operations.clear();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Io,
            snapshot: Some(capture),
        };
        assert!(render(&app).contains("No retained I/O operations"));

        let mut capture = representative_capture();
        capture.cache.tiers[0].hits = 0;
        capture.cache.tiers[0].misses = 0;
        capture.cache.tiers[0].errors = 0;
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Cache,
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
        assert!(rendered.contains("Global backend"));
        assert!(rendered.contains("Owners: 7 / 9"));

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
        assert!(render(&app).contains("physical residency unknown"));

        assert!(
            render_frame(|frame| {
                let area = frame.area();
                draw_empty_panel(frame, area, " Empty ");
            })
            .contains("Press")
        );

        let clear = connected_footer(Some(RecordingConfiguration::default()), CaptureMode::Continue, None, "ready");
        let release = connected_footer(Some(RecordingConfiguration::default()), CaptureMode::Stop, None, "");
        let unknown = connected_footer(None, CaptureMode::Continue, None, "");
        assert_eq!(
            (
                line_text(&clear).contains("clear"),
                line_text(&clear).contains("ready"),
                line_text(&release).contains("Record and stop"),
                line_text(&unknown).contains("configure"),
            ),
            (true, true, true, true)
        );

        let operation = super::super::data::ThreadOperation {
            kind: super::super::data::ThreadOperationKind::MutexAccess,
            events: 1,
            objects: 0,
            participants: vec![super::super::data::ThreadParticipant {
                thread_id: 7,
                name: "worker".into(),
                events: 1,
                objects: Vec::new(),
            }],
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).unwrap();
        let mouse = MouseRows::default();
        terminal
            .draw(|frame| {
                mouse.begin(frame.area());
                draw_thread_participants(
                    frame,
                    &mouse,
                    frame.area(),
                    Some(&operation),
                    Some(7),
                    ThreadViewState {
                        focus: ThreadFocus::Threads,
                        thread_selected: 0,
                        operation_selected: 0,
                        participant_selected: 0,
                        object_selected: 0,
                        stack_scroll: 0,
                        stack_filter: AllocationStackFilter::Application,
                    },
                );
            })
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(text.contains("7 (self)"));
    }

    #[test]
    fn unknown_recording_state_hides_stale_configuration() {
        let mut app = App::new();
        app.screen = Screen::Connected {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            tab: MonitorTab::Info,
            snapshot: None,
        };
        app.recording_unknown = true;

        let output = render(&app);

        assert!(output.contains("Recording state UNKNOWN"));
        assert!(!output.contains("Allocations:"));
    }

    #[cfg_attr(miri, ignore)]
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
            MonitorTab::Io,
            MonitorTab::Cache,
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
            assert!(render(&app).contains("Native v4"));
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

        for focus in [
            RuntimeFocus::Workers,
            RuntimeFocus::Tasks,
            RuntimeFocus::Activity,
            RuntimeFocus::Events,
        ] {
            app.runtime_view.focus = focus;
            app.runtime_view.activity_scroll = usize::MAX;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Runtime,
                snapshot: Some(representative_capture()),
            };
            assert!(render(&app).contains("Task"));
        }

        for (tab, expected) in [(MonitorTab::Io, "I/O Resources"), (MonitorTab::Cache, "Cache Tiers")] {
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab,
                snapshot: Some(representative_capture()),
            };
            assert!(render(&app).contains(expected));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    #[expect(clippy::too_many_lines, reason = "one digest covers every monitor panel and nested focus state")]
    fn monitor_rendering_matches_the_complete_reference_buffer() {
        let mut output = String::new();
        let mut app = App::new();
        app.instances.push(super::super::app::Instance {
            descriptor: descriptor(),
            recording: RecordingConfiguration::default(),
            server_version: None,
        });
        output.push_str(&render_debug(|frame| {
            app.draw_with_version(frame, |_| "12:34:56 (now)".into(), "98.76.54");
        }));
        output.push_str(&render_debug(|frame| {
            App::draw_capture_popup(frame, Duration::from_millis(450), CaptureStep::Decode);
        }));
        output.push_str(&render_debug(|frame| {
            App::draw_recording_configuration_popup(frame, {
                let mut popup = RecordingConfigurationPopup::new(RecordingConfiguration {
                    allocations: RecordingPolicy {
                        enabled: true,
                        capture_backtraces: true,
                        sampling_one_in: 8,
                    },
                    ..Default::default()
                });
                popup.selected = 2;
                popup
            });
        }));

        for tab in [
            MonitorTab::Info,
            MonitorTab::Heaps,
            MonitorTab::Allocations,
            MonitorTab::Primitives,
            MonitorTab::Threads,
            MonitorTab::Runtime,
            MonitorTab::Io,
            MonitorTab::Cache,
        ] {
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab,
                snapshot: Some(representative_capture()),
            };
            output.push_str(&render_debug(|frame| {
                app.draw_with_version(frame, |_| "12:34:56 (now)".into(), "98.76.54");
            }));
        }

        for focus in [PrimitiveFocus::Types, PrimitiveFocus::Operations, PrimitiveFocus::Hotspots] {
            app.primitive_view.focus = focus;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Primitives,
                snapshot: Some(representative_capture()),
            };
            output.push_str(&render_debug(|frame| {
                app.draw_with_version(frame, |_| "12:34:56 (now)".into(), "98.76.54");
            }));
        }
        for focus in [
            ThreadFocus::Threads,
            ThreadFocus::Operations,
            ThreadFocus::Participants,
            ThreadFocus::Objects,
        ] {
            app.thread_view.focus = focus;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Threads,
                snapshot: Some(representative_capture()),
            };
            output.push_str(&render_debug(|frame| {
                app.draw_with_version(frame, |_| "12:34:56 (now)".into(), "98.76.54");
            }));
        }
        for focus in [
            RuntimeFocus::Workers,
            RuntimeFocus::Tasks,
            RuntimeFocus::Activity,
            RuntimeFocus::Events,
        ] {
            app.runtime_view.focus = focus;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Runtime,
                snapshot: Some(representative_capture()),
            };
            output.push_str(&render_debug(|frame| {
                app.draw_with_version(frame, |_| "12:34:56 (now)".into(), "98.76.54");
            }));
        }
        for focus in [IoFocus::Resources, IoFocus::Operations] {
            app.io_view.focus = focus;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Io,
                snapshot: Some(representative_capture()),
            };
            output.push_str(&render_debug(|frame| {
                app.draw_with_version(frame, |_| "12:34:56 (now)".into(), "98.76.54");
            }));
        }
        for focus in [CacheFocus::Tiers, CacheFocus::Operations] {
            app.cache_view.focus = focus;
            app.screen = Screen::Connected {
                descriptor: descriptor(),
                recording: RecordingConfiguration::default(),
                tab: MonitorTab::Cache,
                snapshot: Some(representative_capture()),
            };
            output.push_str(&render_debug(|frame| {
                app.draw_with_version(frame, |_| "12:34:56 (now)".into(), "98.76.54");
            }));
        }

        assert_eq!(stable_digest(&output), (664_188, 15_392_596_828_520_045_353));
    }
}
