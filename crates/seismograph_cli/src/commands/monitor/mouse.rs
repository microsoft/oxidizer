// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders};

use super::app::{App, CacheFocus, HeapFocus, IoFocus, MonitorTab, PrimitiveFocus, RuntimeFocus, Screen, ThreadFocus};
use super::data::MemoryTier;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ListTarget {
    Applications,
    HeapTier(MemoryTier),
    HeapBuckets,
    HeapHotspots,
    Allocations,
    PrimitiveTypes,
    PrimitiveOperations,
    PrimitiveHotspots,
    Threads,
    ThreadOperations,
    ThreadParticipants,
    ThreadObjects,
    RuntimeWorkers,
    RuntimeTasks,
    IoResources,
    IoOperations,
    CacheTiers,
    CacheOperations,
}

impl ListTarget {
    fn tab(self) -> Option<MonitorTab> {
        match self {
            Self::Applications => None,
            Self::HeapTier(_) | Self::HeapBuckets | Self::HeapHotspots => Some(MonitorTab::Heaps),
            Self::Allocations => Some(MonitorTab::Allocations),
            Self::PrimitiveTypes | Self::PrimitiveOperations | Self::PrimitiveHotspots => Some(MonitorTab::Primitives),
            Self::Threads | Self::ThreadOperations | Self::ThreadParticipants | Self::ThreadObjects => Some(MonitorTab::Threads),
            Self::RuntimeWorkers | Self::RuntimeTasks => Some(MonitorTab::Runtime),
            Self::IoResources | Self::IoOperations => Some(MonitorTab::Io),
            Self::CacheTiers | Self::CacheOperations => Some(MonitorTab::Cache),
        }
    }
}

#[derive(Default)]
pub(super) struct MouseRows {
    frame: RefCell<Rect>,
    rows: RefCell<Vec<(Rect, ListTarget, usize)>>,
}

impl MouseRows {
    pub(super) fn begin(&self, frame: Rect) {
        *self.frame.borrow_mut() = frame;
        self.rows.borrow_mut().clear();
    }

    /// Records only rendered data rows, excluding the block and column headings.
    pub(super) fn register(&self, area: Rect, heading: u16, first: usize, count: usize, target: ListTarget) {
        let inner = Block::default().borders(Borders::ALL).inner(area);
        let height = inner.height.saturating_sub(heading);
        let mut rows = self.rows.borrow_mut();
        for offset in 0..height {
            let index = first.saturating_add(usize::from(offset));
            if index >= count {
                break;
            }
            let row = Rect::new(inner.x, inner.y.saturating_add(heading).saturating_add(offset), inner.width, 1)
                .intersection(*self.frame.borrow());
            if !row.is_empty() {
                rows.push((row, target, index));
            }
        }
    }

    pub(super) fn register_tab(&self, area: Rect, target: ListTarget) {
        let area = area.intersection(*self.frame.borrow());
        if !area.is_empty() {
            self.rows.borrow_mut().push((area, target, 0));
        }
    }

    pub(super) fn at(&self, frame: Rect, column: u16, row: u16) -> Option<(ListTarget, usize)> {
        if frame != *self.frame.borrow() {
            return None;
        }
        self.rows
            .borrow()
            .iter()
            .find(|(area, ..)| area.contains((column, row).into()))
            .map(|(_, target, index)| (*target, *index))
    }
}

impl App {
    pub(super) fn activate_mouse_row(&mut self, target: ListTarget, index: usize) {
        let tab = match self.screen {
            Screen::Browse => None,
            Screen::Connected { tab, .. } | Screen::Offline { tab, .. } => Some(tab),
        };
        if target.tab() != tab {
            return;
        }
        if target == ListTarget::Applications {
            if matches!(self.screen, Screen::Browse) {
                self.selected = index;
                self.handle_key(KeyCode::Enter);
            }

            return;
        }
        if let ListTarget::HeapTier(tier) = target {
            while self.heap_view.tier != tier {
                self.handle_key(KeyCode::Char(']'));
            }
            self.handle_key(KeyCode::Enter);
            return;
        }
        let selected = match target {
            ListTarget::HeapBuckets => {
                self.heap_view.focus = HeapFocus::Buckets;
                &mut self.heap_view.bucket_selected
            }
            ListTarget::HeapHotspots => {
                self.heap_view.focus = HeapFocus::Hotspots;
                &mut self.heap_view.hotspot_selected
            }
            ListTarget::Allocations => &mut self.allocation_view.selected,
            ListTarget::PrimitiveTypes => {
                self.primitive_view.focus = PrimitiveFocus::Types;
                &mut self.primitive_view.primitive_selected
            }
            ListTarget::PrimitiveOperations => {
                self.primitive_view.focus = PrimitiveFocus::Operations;
                &mut self.primitive_view.operation_selected
            }
            ListTarget::PrimitiveHotspots => {
                self.primitive_view.focus = PrimitiveFocus::Hotspots;
                &mut self.primitive_view.hotspot_selected
            }
            ListTarget::Threads => {
                self.thread_view.focus = ThreadFocus::Threads;
                &mut self.thread_view.thread_selected
            }
            ListTarget::ThreadOperations => {
                self.thread_view.focus = ThreadFocus::Operations;
                &mut self.thread_view.operation_selected
            }
            ListTarget::ThreadParticipants => {
                self.thread_view.focus = ThreadFocus::Participants;
                &mut self.thread_view.participant_selected
            }
            ListTarget::ThreadObjects => {
                self.thread_view.focus = ThreadFocus::Objects;
                &mut self.thread_view.object_selected
            }
            ListTarget::RuntimeWorkers => {
                self.runtime_view.focus = RuntimeFocus::Workers;
                &mut self.runtime_view.worker_selected
            }
            ListTarget::RuntimeTasks => {
                self.runtime_view.focus = RuntimeFocus::Tasks;
                &mut self.runtime_view.task_selected
            }
            ListTarget::IoResources => {
                self.io_view.focus = IoFocus::Resources;
                &mut self.io_view.resource_selected
            }
            ListTarget::IoOperations => {
                self.io_view.focus = IoFocus::Operations;
                &mut self.io_view.operation_selected
            }
            ListTarget::CacheTiers => {
                self.cache_view.focus = CacheFocus::Tiers;
                &mut self.cache_view.tier_selected
            }
            ListTarget::CacheOperations => {
                self.cache_view.focus = CacheFocus::Operations;
                &mut self.cache_view.operation_selected
            }
            ListTarget::Applications | ListTarget::HeapTier(_) => return,
        };
        // Reuse keyboard selection (including dependent selections and scroll resets),
        // then the exact same Enter action as keyboard navigation.
        *selected = index.saturating_add(1);
        self.handle_key(KeyCode::Up);
        self.handle_key(KeyCode::Enter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolled_rows_exclude_headers_borders_and_blank_space() {
        let rows = MouseRows::default();
        let frame = Rect::new(0, 0, 80, 30);
        rows.begin(frame);
        rows.register(Rect::new(5, 4, 20, 8), 1, 10, 13, ListTarget::RuntimeTasks);
        assert_eq!(
            (4..12).map(|y| rows.at(frame, 6, y)).collect::<Vec<_>>(),
            vec![
                None,
                None,
                Some((ListTarget::RuntimeTasks, 10)),
                Some((ListTarget::RuntimeTasks, 11)),
                Some((ListTarget::RuntimeTasks, 12)),
                None,
                None,
                None,
            ],
        );
        assert_eq!(rows.at(frame, 5, 6), None);
        assert_eq!(rows.at(frame, 24, 6), None);
    }

    #[test]
    fn clipped_and_stale_viewports_do_not_produce_hidden_targets() {
        let rows = MouseRows::default();
        let frame = Rect::new(0, 0, 10, 6);
        rows.begin(frame);
        rows.register(Rect::new(7, 1, 20, 20), 1, 7, 30, ListTarget::Allocations);
        assert_eq!(rows.at(frame, 9, 5), Some((ListTarget::Allocations, 9)));
        assert_eq!(rows.at(frame, 10, 5), None);
        assert_eq!(rows.at(frame, 9, 6), None);
        assert_eq!(rows.at(Rect::new(0, 0, 11, 6), 9, 5), None);
        rows.begin(frame);
        assert_eq!(rows.at(frame, 9, 5), None);
    }

    #[test]
    fn collapsed_panels_have_no_clickable_rows() {
        let rows = MouseRows::default();
        let frame = Rect::new(0, 0, 20, 20);
        rows.begin(frame);
        for area in [Rect::new(0, 0, 1, 20), Rect::new(0, 0, 20, 3)] {
            rows.register(area, 1, 0, 30, ListTarget::Threads);
        }
        assert_eq!(rows.rows.borrow().len(), 0);
    }
}
