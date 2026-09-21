// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders};

use super::app::{App, MonitorTab, Screen};

pub(super) const TABS: [(MonitorTab, &str); 8] = [
    (MonitorTab::Info, "  Info  "),
    (MonitorTab::Heaps, "  Heaps  "),
    (MonitorTab::Allocations, "  Allocations  "),
    (MonitorTab::Primitives, "  Primitives  "),
    (MonitorTab::Threads, "  Threads  "),
    (MonitorTab::Runtime, "  Runtime  "),
    (MonitorTab::Io, "  I/O  "),
    (MonitorTab::Cache, "  Cache  "),
];

fn tab_at(area: Rect, column: u16, row: u16) -> Option<MonitorTab> {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if row != inner.y || !inner.contains((column, row).into()) {
        return None;
    }
    let mut x = inner.x;
    for (tab, title) in TABS {
        let width = u16::try_from(Line::from(title).width()).unwrap_or(u16::MAX);
        if column >= x && column < x.saturating_add(width).min(inner.right()) {
            return Some(tab);
        }
        x = x.saturating_add(width).saturating_add(1); // One-cell divider.
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Split {
    Info,
    HeapSummary,
    HeapColumns,
    HeapStack,
    Allocations,
    PrimitiveTypes,
    PrimitiveOperations,
    PrimitiveStack,
    ThreadRows,
    ThreadColumns,
    ThreadParticipants,
    ThreadStack,
    RuntimeRows,
    RuntimeColumns,
    Io,
    Cache,
}

#[derive(Default)]
pub(super) struct Panels {
    pub(super) rows: super::mouse::MouseRows,
    sizes: [Option<Constraint>; 16],
    dragging: Option<(MonitorTab, Split)>,
}

pub(super) struct Arrangement {
    pub(super) areas: [Rect; 5],
    dividers: Vec<Divider>,
}

struct Divider {
    split: Split,
    direction: Direction,
    parent: Rect,
    hit: Rect,
}

impl Panels {
    pub(super) fn cancel_drag(&mut self) {
        self.dragging = None;
    }

    pub(super) fn arrange(&self, tab: MonitorTab, area: Rect) -> Arrangement {
        let mut dividers = Vec::new();
        let mut split = |id, direction, parent, default| {
            let constraint = self.sizes[id as usize].unwrap_or(default);
            let [first, second] = Layout::new(direction, [constraint, Constraint::Min(3)]).areas(parent);
            let hit = match direction {
                Direction::Horizontal => Rect::new(second.x.saturating_sub(1), parent.y, 2, parent.height),
                Direction::Vertical => Rect::new(parent.x, second.y.saturating_sub(1), parent.width, 2),
            };
            if first.width > 0 && first.height > 0 && second.width > 0 && second.height > 0 {
                dividers.push(Divider {
                    split: id,
                    direction,
                    parent,
                    hit,
                });
            }
            [first, second]
        };
        let mut areas = [Rect::default(); 5];
        match tab {
            MonitorTab::Info => {
                [areas[0], areas[1]] = split(Split::Info, Direction::Vertical, area, Constraint::Percentage(45));
            }
            MonitorTab::Heaps => {
                let [summary, rest] = split(Split::HeapSummary, Direction::Vertical, area, Constraint::Length(6));
                let [tiers, details] = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(rest);
                let [buckets, side] = split(Split::HeapColumns, Direction::Horizontal, details, Constraint::Percentage(52));
                let [hotspots, stack] = split(Split::HeapStack, Direction::Vertical, side, Constraint::Percentage(52));
                areas = [summary, tiers, buckets, hotspots, stack];
            }
            MonitorTab::Allocations => {
                [areas[0], areas[1]] = split(Split::Allocations, Direction::Vertical, area, Constraint::Percentage(58));
            }
            MonitorTab::Primitives => {
                let [types, rest] = split(Split::PrimitiveTypes, Direction::Vertical, area, Constraint::Length(9));
                let [operations, details] = split(Split::PrimitiveOperations, Direction::Vertical, rest, Constraint::Length(10));
                let [hotspots, stack] = split(Split::PrimitiveStack, Direction::Horizontal, details, Constraint::Percentage(45));
                areas[..4].copy_from_slice(&[types, operations, hotspots, stack]);
            }
            MonitorTab::Threads => {
                let [top, bottom] = split(Split::ThreadRows, Direction::Vertical, area, Constraint::Percentage(42));
                let [threads, operations] = split(Split::ThreadColumns, Direction::Horizontal, top, Constraint::Percentage(36));
                let [participants, rest] = split(Split::ThreadParticipants, Direction::Horizontal, bottom, Constraint::Percentage(30));
                let [objects, stack] = split(Split::ThreadStack, Direction::Horizontal, rest, Constraint::Percentage(43));
                areas = [threads, operations, participants, objects, stack];
            }
            MonitorTab::Runtime => {
                let [workers, rest] = split(Split::RuntimeRows, Direction::Vertical, area, Constraint::Percentage(45));
                let [tasks, details] = split(Split::RuntimeColumns, Direction::Horizontal, rest, Constraint::Percentage(45));
                areas[..3].copy_from_slice(&[workers, tasks, details]);
            }
            MonitorTab::Io => {
                [areas[0], areas[1]] = split(Split::Io, Direction::Horizontal, area, Constraint::Percentage(48));
            }
            MonitorTab::Cache => {
                [areas[0], areas[1]] = split(Split::Cache, Direction::Horizontal, area, Constraint::Percentage(55));
            }
        }
        Arrangement { areas, dividers }
    }

    fn handle_mouse(&mut self, event: MouseEvent, tab: MonitorTab, area: Rect) {
        let arrangement = self.arrange(tab, area);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.dragging = arrangement
                    .dividers
                    .iter()
                    .find(|divider| divider.hit.contains((event.column, event.row).into()))
                    .map(|divider| (tab, divider.split));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some((drag_tab, id)) = self.dragging else { return };
                if tab != drag_tab {
                    self.dragging = None;
                    return;
                }
                let Some(divider) = arrangement.dividers.iter().find(|divider| divider.split == id) else {
                    self.dragging = None;
                    return;
                };
                let (position, start, length, minimum) = match divider.direction {
                    Direction::Horizontal => (event.column, divider.parent.x, divider.parent.width, 12),
                    Direction::Vertical => (event.row, divider.parent.y, divider.parent.height, 3),
                };
                if length == 0 {
                    return;
                }
                let minimum = minimum.min(length / 2);
                let offset = position.saturating_sub(start).clamp(minimum, length - minimum);
                self.sizes[id as usize] = Some(Constraint::Ratio(u32::from(offset), u32::from(length)));
            }
            MouseEventKind::Up(MouseButton::Left) => self.dragging = None,
            _ => {}
        }
    }
}

impl App {
    pub(super) fn handle_mouse(&mut self, event: MouseEvent, area: Rect) {
        if let Some(help) = &self.help {
            help.handle_mouse(event);
            self.panels.cancel_drag();
            return;
        }
        if self.recording_configuration_popup.is_some() || self.filters.popup.is_some() || self.capture_started_at.is_some() {
            self.panels.dragging = None;
            return;
        }
        if matches!(self.screen, Screen::Browse) {
            self.panels.dragging = None;
            self.handle_row_click(event, area);
            return;
        }
        let (Screen::Connected { tab, .. } | Screen::Offline { tab, .. }) = self.screen else {
            self.panels.dragging = None;
            self.panels.rows.begin(area);
            return;
        };
        let [body, _, _] = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(self.filter_banner_height()),
            Constraint::Length(1),
        ])
        .areas(area);
        let [tabs, content] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(body);
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(selected) = tab_at(tabs, event.column, event.row)
        {
            if let Screen::Connected { tab, .. } | Screen::Offline { tab, .. } = &mut self.screen {
                *tab = selected;
            }
            self.panels.dragging = None;
            self.panels.rows.begin(area);
            return;
        }
        if matches!(self.screen, Screen::Offline { .. }) && tab == MonitorTab::Info {
            self.panels.dragging = None;
            return;
        }
        self.panels.handle_mouse(event, tab, content);
        if self.panels.dragging.is_none() {
            self.handle_row_click(event, area);
        }
    }

    fn handle_row_click(&mut self, event: MouseEvent, area: Rect) {
        if event.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some((target, index)) = self.panels.rows.at(area, event.column, event.row)
        {
            self.panels.rows.begin(area);
            self.activate_mouse_row(target, index);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn dragging_changes_only_the_selected_split_and_survives_resize() {
        let mut panels = Panels::default();
        let area = Rect::new(5, 3, 100, 40);
        let before = panels.arrange(MonitorTab::Runtime, area).areas;
        let divider = before[1].y;
        panels.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 10, divider),
            MonitorTab::Runtime,
            area,
        );
        panels.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 10, 33), MonitorTab::Runtime, area);
        panels.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 10, 33), MonitorTab::Runtime, area);
        let after = panels.arrange(MonitorTab::Runtime, area).areas;
        assert_eq!((after[0].height, after[1].width), (30, before[1].width));
        assert_eq!(panels.arrange(MonitorTab::Runtime, Rect::new(0, 0, 100, 80)).areas[0].height, 60);
        assert_eq!(
            panels.arrange(MonitorTab::Allocations, area).areas,
            Panels::default().arrange(MonitorTab::Allocations, area).areas
        );
        panels.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 10, 5), MonitorTab::Runtime, area);
        assert_eq!(panels.arrange(MonitorTab::Runtime, area).areas, after);
    }

    #[test]
    fn nested_horizontal_drag_is_clamped_and_tab_changes_cancel_it() {
        let mut panels = Panels::default();
        let area = Rect::new(0, 0, 100, 40);
        let before = panels.arrange(MonitorTab::Threads, area).areas;
        panels.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), before[4].x, 30),
            MonitorTab::Threads,
            area,
        );
        panels.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 200, 30), MonitorTab::Threads, area);
        let after = panels.arrange(MonitorTab::Threads, area).areas;
        assert_eq!(after[2], before[2]);
        assert!(after[4].width >= 12);
        panels.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0), MonitorTab::Cache, area);
        assert!(panels.dragging.is_none());
    }

    #[test]
    fn small_terminals_and_non_divider_clicks_are_safe() {
        let mut panels = Panels::default();
        for size in 0..8 {
            let area = Rect::new(0, 0, size, size);
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
                assert!(
                    panels
                        .arrange(tab, area)
                        .areas
                        .iter()
                        .all(|pane| pane.right() <= area.right() && pane.bottom() <= area.bottom())
                );
            }
        }
        panels.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 10, 5),
            MonitorTab::Io,
            Rect::new(0, 0, 100, 40),
        );
        assert!(panels.dragging.is_none());
    }

    #[test]
    fn app_routes_viewer_mouse_events_but_blocks_modal_drags() {
        let mut app = App::offline("capture.seismograph".into());
        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Allocations;
        }
        let terminal = Rect::new(0, 0, 100, 44);
        let content = Rect::new(0, 3, 100, 40);
        let before = app.panels.arrange(MonitorTab::Allocations, content).areas;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 10, before[1].y), terminal);
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 10, 33), terminal);
        assert_eq!(app.panels.arrange(MonitorTab::Allocations, content).areas[0].height, 30);
        app.capture_started_at = Some(std::time::Instant::now());
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 10, 10), terminal);
        assert!(app.panels.dragging.is_none());
        assert_eq!(app.panels.arrange(MonitorTab::Allocations, content).areas[0].height, 30);
    }

    #[test]
    fn help_cancels_an_active_drag_and_blocks_row_activation() {
        use crossterm::event::KeyCode;

        let mut app = App::offline("capture.seismograph".into());
        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Runtime;
        }
        let terminal = Rect::new(0, 0, 100, 44);
        let content = Rect::new(0, 3, 100, 40);
        let before = app.panels.arrange(MonitorTab::Runtime, content).areas;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 10, before[1].y), terminal);
        assert!(app.panels.dragging.is_some());
        app.handle_key(KeyCode::F(1));
        assert!(app.panels.dragging.is_none());
        app.panels.rows.begin(terminal);
        app.panels
            .rows
            .register(before[0], 1, 0, 1, super::super::mouse::ListTarget::RuntimeWorkers);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, before[0].y + 2), terminal);
        assert_eq!(app.runtime_view.focus, super::super::app::RuntimeFocus::Workers);
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 10, 33), terminal);
        app.handle_key(KeyCode::Esc);
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 10, 33), terminal);
        assert_eq!(app.panels.arrange(MonitorTab::Runtime, content).areas, before);
    }

    #[test]
    fn clicking_headers_selects_tabs_and_blocks_modal_input() {
        let mut app = App::offline("capture.seismograph".into());
        let terminal = Rect::new(5, 8, 140, 44);
        let mut x = terminal.x + 1;
        for (expected, title) in TABS {
            app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x + 2, terminal.y + 1), terminal);
            let Screen::Offline { tab, .. } = app.screen else {
                panic!("viewer remains offline")
            };
            assert_eq!(tab, expected);
            assert!(app.panels.dragging.is_none());
            x += u16::try_from(title.len()).unwrap() + 1;
        }
        app.recording_configuration_popup = Some(super::super::app::RecordingConfigurationPopup::new(
            seismograph_protocol::message::RecordingConfiguration::default(),
        ));
        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), terminal.x + 3, terminal.y + 1),
            terminal,
        );
        let Screen::Offline { tab, .. } = app.screen else {
            panic!("viewer remains offline")
        };
        assert_eq!(tab, MonitorTab::Cache);
    }

    #[test]
    fn header_hit_testing_excludes_borders_dividers_and_clipped_titles() {
        let area = Rect::new(5, 8, 20, 3);
        assert_eq!(
            [
                tab_at(area, 8, 8),
                tab_at(area, 5, 9),
                tab_at(area, 8, 10),
                tab_at(area, 14, 9),
                tab_at(area, 8, 9),
                tab_at(area, 17, 9),
                tab_at(area, 24, 9),
                tab_at(Rect::new(0, 0, 1, 1), 0, 0),
            ],
            [None, None, None, None, Some(MonitorTab::Info), Some(MonitorTab::Heaps), None, None],
        );
    }
}
