// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders};

use super::app::{
    AllocatorBackend, App, CacheFocus, HeapFocus, IoFocus, MonitorTab, PrimitiveFocus, RuntimeDetailMode,
    RuntimeSectionFocus, RuntimeTaskRowSort, RuntimeTaskTypeSort, RuntimeWorkersFocus, Screen, TaskEventsFocus, ThreadFocus,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ListTarget {
    Applications,
    InfoThreads,
    HeapBuckets,
    NativeSubsystems,
    NativeClasses,
    NativeMemory,
    Allocations,
    AllocationHeader(super::data::AllocationSort),
    PrimitiveTypes,
    PrimitiveTypeHeader(super::app::PrimitiveTypeSort),
    PrimitiveOperationHeader(super::data::PrimitiveSort),
    PrimitiveOperations,
    PrimitiveHotspots,
    PrimitiveHotspotHeader(super::app::PrimitiveHotspotSort),
    Threads,
    ThreadHeader(super::app::ThreadListSort),
    ThreadOperations,
    ThreadOperationHeader(super::app::ThreadOperationSort),
    ThreadParticipants,
    ThreadParticipantHeader(super::app::ThreadParticipantSort),
    ThreadObjects,
    ThreadObjectHeader(super::app::ThreadObjectSort),
    AsynchronousWorkersTab,
    BlockingWorkersTab,
    RuntimeWorkerHeader(RuntimeWorkersFocus, super::app::RuntimeWorkerSort),
    AsynchronousWorkers,
    BlockingWorkers,
    AsynchronousTaskGroups,
    BlockingTaskGroups,
    RuntimeTaskTypeHeader(RuntimeTaskTypeSort),
    RuntimeGroupTasks,
    RuntimeTaskHeader(RuntimeTaskRowSort),
    TaskOperations,
    TaskOccurrences,
    IoResources,
    IoResourceHeader(super::app::IoResourceSort),
    IoOperations,
    IoOperationHeader(super::app::IoOperationSort),
    CacheTiers,
    CacheTierHeader(super::app::CacheTierSort),
    CacheOperations,
    CacheOperationHeader(super::app::CacheOperationSort),
}

impl ListTarget {
    fn tab(self) -> Option<MonitorTab> {
        match self {
            Self::Applications => None,
            Self::InfoThreads => Some(MonitorTab::Info),
            Self::HeapBuckets | Self::NativeSubsystems | Self::NativeClasses | Self::NativeMemory => Some(MonitorTab::Heaps),
            Self::Allocations | Self::AllocationHeader(_) => Some(MonitorTab::Allocations),
            Self::PrimitiveTypes
            | Self::PrimitiveTypeHeader(_)
            | Self::PrimitiveOperations
            | Self::PrimitiveOperationHeader(_)
            | Self::PrimitiveHotspots
            | Self::PrimitiveHotspotHeader(_) => Some(MonitorTab::Primitives),
            Self::Threads
            | Self::ThreadHeader(_)
            | Self::ThreadOperations
            | Self::ThreadOperationHeader(_)
            | Self::ThreadParticipants
            | Self::ThreadParticipantHeader(_)
            | Self::ThreadObjects
            | Self::ThreadObjectHeader(_) => Some(MonitorTab::Threads),
            Self::AsynchronousWorkersTab
            | Self::BlockingWorkersTab
            | Self::RuntimeWorkerHeader(_, _)
            | Self::AsynchronousWorkers
            | Self::BlockingWorkers => {
                Some(MonitorTab::Runtime)
            }
            Self::AsynchronousTaskGroups | Self::BlockingTaskGroups | Self::RuntimeTaskTypeHeader(_) | Self::RuntimeGroupTasks => {
                Some(MonitorTab::Runtime)
            }
            Self::RuntimeTaskHeader(_) => Some(MonitorTab::Runtime),
            Self::TaskOperations | Self::TaskOccurrences => Some(MonitorTab::RuntimeTask),
            Self::IoResources | Self::IoResourceHeader(_) | Self::IoOperations | Self::IoOperationHeader(_) => Some(MonitorTab::Io),
            Self::CacheTiers | Self::CacheTierHeader(_) | Self::CacheOperations | Self::CacheOperationHeader(_) => Some(MonitorTab::Cache),
        }
    }
}

#[derive(Default)]
pub(super) struct MouseRows {
    frame: RefCell<Rect>,
    rows: RefCell<Vec<(Rect, ListTarget, usize)>>,
    pub(super) native_scroll_limit: Cell<usize>,
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
        self.register_item(area, target, 0);
    }

    pub(super) fn register_item(&self, area: Rect, target: ListTarget, index: usize) {
        let area = area.intersection(*self.frame.borrow());
        if !area.is_empty() {
            self.rows.borrow_mut().push((area, target, index));
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
        let (tab, snapshot) = match &self.screen {
            Screen::Browse => (None, None),
            Screen::Connected { tab, snapshot, .. } | Screen::Offline { tab, snapshot, .. } => (Some(*tab), snapshot.as_deref()),
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
        if target == ListTarget::InfoThreads {
            self.info_thread_selected = index.min(self.live_activity.threads.len().saturating_sub(1));
            return;
        }
        if let ListTarget::AllocationHeader(sort) = target {
            if self.allocation_view.sort == sort {
                self.allocation_view.descending = !self.allocation_view.descending;
            } else {
                self.allocation_view.sort = sort;
                self.allocation_view.descending = true;
            }
            self.allocation_view.reset_position();
            return;
        }
        if let ListTarget::PrimitiveOperationHeader(sort) = target {
            if self.primitive_view.sort == sort {
                self.primitive_view.descending = !self.primitive_view.descending;
            } else {
                self.primitive_view.sort = sort;
                self.primitive_view.descending = true;
            }
            self.primitive_view.reset_operations();
            return;
        }
        if let ListTarget::PrimitiveTypeHeader(sort) = target {
            if self.primitive_view.type_sort == sort {
                self.primitive_view.type_descending = !self.primitive_view.type_descending;
            } else {
                self.primitive_view.type_sort = sort;
                self.primitive_view.type_descending = sort != super::app::PrimitiveTypeSort::Type;
            }
            self.primitive_view.primitive_selected = 0;
            self.primitive_view.reset_operations();
            return;
        }
        if let ListTarget::PrimitiveHotspotHeader(sort) = target {
            if self.primitive_view.hotspot_sort == sort {
                self.primitive_view.hotspot_descending = !self.primitive_view.hotspot_descending;
            } else {
                self.primitive_view.hotspot_sort = sort;
                self.primitive_view.hotspot_descending = sort != super::app::PrimitiveHotspotSort::Location;
            }
            self.primitive_view.hotspot_selected = 0;
            self.primitive_view.stack_scroll = 0;
            return;
        }
        if let ListTarget::ThreadHeader(sort) = target {
            if self.thread_view.thread_sort == sort {
                self.thread_view.thread_descending = !self.thread_view.thread_descending;
            } else {
                self.thread_view.thread_sort = sort;
                self.thread_view.thread_descending = sort != super::app::ThreadListSort::Thread;
            }
            self.thread_view.thread_selected = 0;
            self.thread_view.reset_operation();
            return;
        }
        if let ListTarget::ThreadOperationHeader(sort) = target {
            if self.thread_view.operation_sort == sort {
                self.thread_view.operation_descending = !self.thread_view.operation_descending;
            } else {
                self.thread_view.operation_sort = sort;
                self.thread_view.operation_descending = sort != super::app::ThreadOperationSort::Operation;
            }
            self.thread_view.operation_selected = 0;
            self.thread_view.reset_participant();
            return;
        }
        if let ListTarget::ThreadParticipantHeader(sort) = target {
            if self.thread_view.participant_sort == sort {
                self.thread_view.participant_descending = !self.thread_view.participant_descending;
            } else {
                self.thread_view.participant_sort = sort;
                self.thread_view.participant_descending = sort != super::app::ThreadParticipantSort::Thread;
            }
            self.thread_view.participant_selected = 0;
            self.thread_view.reset_object();
            return;
        }
        if let ListTarget::ThreadObjectHeader(sort) = target {
            if self.thread_view.object_sort == sort {
                self.thread_view.object_descending = !self.thread_view.object_descending;
            } else {
                self.thread_view.object_sort = sort;
                self.thread_view.object_descending = sort != super::app::ThreadObjectSort::Object;
            }
            self.thread_view.object_selected = 0;
            self.thread_view.stack_scroll = 0;
            return;
        }
        if let ListTarget::IoResourceHeader(sort) = target {
            if self.io_view.resource_sort == sort {
                self.io_view.resource_descending = !self.io_view.resource_descending;
            } else {
                self.io_view.resource_sort = sort;
                self.io_view.resource_descending = true;
            }
            self.io_view.resource_selected = 0;
            self.io_view.operation_selected = 0;
            return;
        }
        if let ListTarget::IoOperationHeader(sort) = target {
            if self.io_view.operation_sort == sort {
                self.io_view.operation_descending = !self.io_view.operation_descending;
            } else {
                self.io_view.operation_sort = sort;
                self.io_view.operation_descending = true;
            }
            self.io_view.operation_selected = 0;
            return;
        }
        if let ListTarget::CacheTierHeader(sort) = target {
            if self.cache_view.tier_sort == sort {
                self.cache_view.tier_descending = !self.cache_view.tier_descending;
            } else {
                self.cache_view.tier_sort = sort;
                self.cache_view.tier_descending = true;
            }
            self.cache_view.tier_selected = 0;
            self.cache_view.operation_selected = 0;
            return;
        }
        if let ListTarget::CacheOperationHeader(sort) = target {
            if self.cache_view.operation_sort == sort {
                self.cache_view.operation_descending = !self.cache_view.operation_descending;
            } else {
                self.cache_view.operation_sort = sort;
                self.cache_view.operation_descending = true;
            }
            self.cache_view.operation_selected = 0;
            return;
        }
        if let ListTarget::RuntimeWorkerHeader(focus, sort) = target {
            let (selected_sort, descending, selected) = match focus {
                RuntimeWorkersFocus::Asynchronous => (
                    &mut self.runtime_view.asynchronous_worker_sort,
                    &mut self.runtime_view.asynchronous_worker_descending,
                    &mut self.runtime_view.asynchronous_selected,
                ),
                RuntimeWorkersFocus::Blocking => (
                    &mut self.runtime_view.blocking_worker_sort,
                    &mut self.runtime_view.blocking_worker_descending,
                    &mut self.runtime_view.blocking_selected,
                ),
            };
            if *selected_sort == sort {
                *descending = !*descending;
            } else {
                *selected_sort = sort;
                *descending = !matches!(sort, super::app::RuntimeWorkerSort::WorkerId | super::app::RuntimeWorkerSort::State);
            }
            *selected = 0;
            self.runtime_view.asynchronous_task_selected = 0;
            self.runtime_view.blocking_task_selected = 0;
            self.runtime_view.group_selected_explicitly = false;
            return;
        }
        if matches!(
            target,
            ListTarget::HeapBuckets | ListTarget::NativeSubsystems | ListTarget::NativeClasses | ListTarget::NativeMemory
        ) {
            if self.heap_view.backend(snapshot) == AllocatorBackend::Mimalloc
                && let Some((_, state)) = snapshot
                    .and_then(|capture| capture.mimalloc.as_deref())
                    .and_then(crate::mimalloc_view::parts)
            {
                self.heap_view.mimalloc.click(target, index, state);
            } else {
                self.heap_view.native.click(target, index);
                self.heap_view
                    .native
                    .reconcile(snapshot.and_then(|snapshot| snapshot.native.as_deref()));
            }
            return;
        }
        if target == ListTarget::AsynchronousWorkersTab {
            self.runtime_view.focus = RuntimeWorkersFocus::Asynchronous;
            return;
        }
        if target == ListTarget::BlockingWorkersTab {
            self.runtime_view.focus = RuntimeWorkersFocus::Blocking;
            return;
        }
        if let ListTarget::RuntimeTaskTypeHeader(sort) = target {
            if self.runtime_view.task_type_sort == sort {
                self.runtime_view.task_type_sort_descending = !self.runtime_view.task_type_sort_descending;
            } else {
                self.runtime_view.task_type_sort = sort;
                self.runtime_view.task_type_sort_descending = true;
            }
            self.runtime_view.asynchronous_task_selected = 0;
            self.runtime_view.blocking_task_selected = 0;
            self.runtime_view.task_selected = 0;
            self.runtime_view.task_selected_explicitly = false;
            return;
        }
        if let ListTarget::RuntimeTaskHeader(sort) = target {
            if self.runtime_view.task_sort == sort {
                self.runtime_view.task_sort_descending = !self.runtime_view.task_sort_descending;
            } else {
                self.runtime_view.task_sort = sort;
                self.runtime_view.task_sort_descending = false;
            }
            self.runtime_view.task_selected = 0;
            self.runtime_view.task_selected_explicitly = false;
            return;
        }
        self.activate_selectable_mouse_row(target, index);
    }

    fn activate_selectable_mouse_row(&mut self, target: ListTarget, index: usize) {
        match target {
            ListTarget::AsynchronousWorkers => {
                self.runtime_view.focus = RuntimeWorkersFocus::Asynchronous;
                self.runtime_view.section = RuntimeSectionFocus::Workers;
                self.runtime_view.asynchronous_selected = index;
                return;
            }
            ListTarget::BlockingWorkers => {
                self.runtime_view.focus = RuntimeWorkersFocus::Blocking;
                self.runtime_view.section = RuntimeSectionFocus::Workers;
                self.runtime_view.blocking_selected = index;
                return;
            }
            ListTarget::AsynchronousTaskGroups => {
                self.runtime_view.focus = RuntimeWorkersFocus::Asynchronous;
                self.runtime_view.section = RuntimeSectionFocus::TaskTypes;
                self.runtime_view.detail_mode = RuntimeDetailMode::Group;
                self.runtime_view.asynchronous_task_selected = index;
                self.runtime_view.group_selected_explicitly = true;
                self.runtime_view.task_selected = 0;
                self.last_runtime_task_click = None;
                return;
            }
            ListTarget::BlockingTaskGroups => {
                self.runtime_view.focus = RuntimeWorkersFocus::Blocking;
                self.runtime_view.section = RuntimeSectionFocus::TaskTypes;
                self.runtime_view.detail_mode = RuntimeDetailMode::Group;
                self.runtime_view.blocking_task_selected = index;
                self.runtime_view.group_selected_explicitly = true;
                self.runtime_view.task_selected = 0;
                self.last_runtime_task_click = None;
                return;
            }
            ListTarget::RuntimeGroupTasks => {
                self.runtime_view.section = RuntimeSectionFocus::GroupTasks;
                self.runtime_view.detail_mode = RuntimeDetailMode::Task;
                self.runtime_view.task_selected = index;
                self.runtime_view.task_selected_explicitly = true;
                let now = self.io.instant();
                let double_click = self.last_runtime_task_click.is_some_and(|(previous, at)| {
                    previous == index && now.saturating_duration_since(at) <= std::time::Duration::from_millis(500)
                });
                if double_click {
                    self.last_runtime_task_click = None;
                    match &mut self.screen {
                        Screen::Connected { tab, .. } | Screen::Offline { tab, .. } => {
                            *tab = MonitorTab::RuntimeTask;
                        }
                        Screen::Browse => {}
                    }
                } else {
                    self.last_runtime_task_click = Some((index, now));
                }
                return;
            }
            _ => {}
        }
        let selected = match target {
            ListTarget::HeapBuckets => {
                self.heap_view.focus = HeapFocus::Buckets;
                &mut self.heap_view.bucket_selected
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
            ListTarget::Applications
            | ListTarget::NativeSubsystems
            | ListTarget::NativeClasses
            | ListTarget::NativeMemory
            | ListTarget::InfoThreads
            | ListTarget::AllocationHeader(_)
            | ListTarget::PrimitiveTypeHeader(_)
            | ListTarget::PrimitiveOperationHeader(_)
            | ListTarget::PrimitiveHotspotHeader(_)
            | ListTarget::ThreadHeader(_)
            | ListTarget::ThreadOperationHeader(_)
            | ListTarget::ThreadParticipantHeader(_)
            | ListTarget::ThreadObjectHeader(_)
            | ListTarget::IoResourceHeader(_)
            | ListTarget::IoOperationHeader(_)
            | ListTarget::CacheTierHeader(_)
            | ListTarget::CacheOperationHeader(_)
            | ListTarget::AsynchronousWorkersTab
            | ListTarget::BlockingWorkersTab
            | ListTarget::RuntimeWorkerHeader(_, _)
            | ListTarget::AsynchronousWorkers
            | ListTarget::BlockingWorkers
            | ListTarget::AsynchronousTaskGroups
            | ListTarget::BlockingTaskGroups
            | ListTarget::RuntimeTaskTypeHeader(_)
            | ListTarget::RuntimeGroupTasks => return,
            ListTarget::RuntimeTaskHeader(_) => return,
            ListTarget::TaskOperations | ListTarget::TaskOccurrences => {
                self.runtime_view.detail_mode = RuntimeDetailMode::Events;
                let view = &mut self.runtime_view.events;
                if target == ListTarget::TaskOperations {
                    view.focus = TaskEventsFocus::Operations;
                    &mut view.operation_selected
                } else {
                    view.focus = TaskEventsFocus::Occurrences;
                    &mut view.event_selected
                }
            }
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
        rows.register(Rect::new(5, 4, 20, 8), 1, 10, 13, ListTarget::AsynchronousTaskGroups);
        assert_eq!(
            (4..12).map(|y| rows.at(frame, 6, y)).collect::<Vec<_>>(),
            vec![
                None,
                None,
                Some((ListTarget::AsynchronousTaskGroups, 10)),
                Some((ListTarget::AsynchronousTaskGroups, 11)),
                Some((ListTarget::AsynchronousTaskGroups, 12)),
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

    #[test]
    fn mouse_dispatch_rejects_stale_tabs_and_selects_runtime_categories() {
        let mut app = App::new();
        app.screen = Screen::Offline {
            path: "capture.seismograph".into(),
            tab: MonitorTab::Runtime,
            snapshot: None,
        };
        for target in [
            ListTarget::HeapBuckets,
            ListTarget::NativeSubsystems,
            ListTarget::NativeClasses,
            ListTarget::NativeMemory,
        ] {
            assert_eq!(target.tab(), Some(MonitorTab::Heaps));
            app.activate_mouse_row(target, usize::MAX);
        }
        app.activate_mouse_row(ListTarget::InfoThreads, 7);
        assert_eq!(app.info_thread_selected, 0);
        app.activate_mouse_row(ListTarget::BlockingWorkersTab, 0);
        assert_eq!(app.runtime_view.focus, RuntimeWorkersFocus::Blocking);
        app.activate_mouse_row(ListTarget::AsynchronousTaskGroups, 2);
        assert_eq!(
            (
                app.runtime_view.focus,
                app.runtime_view.asynchronous_task_selected,
                app.runtime_view.group_selected_explicitly,
            ),
            (RuntimeWorkersFocus::Asynchronous, 2, true)
        );
        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Info;
        }
        app.activate_mouse_row(ListTarget::InfoThreads, usize::MAX);
        assert_eq!(app.info_thread_selected, 0);
        app.activate_selectable_mouse_row(ListTarget::HeapBuckets, 0);
        assert_eq!(app.heap_view.focus, HeapFocus::Buckets);
    }

    #[test]
    fn passive_mouse_targets_do_not_change_selection() {
        let mut app = App::new();
        app.activate_mouse_row(ListTarget::Applications, 7);
        for target in [
            ListTarget::Applications,
            ListTarget::InfoThreads,
            ListTarget::AsynchronousWorkersTab,
            ListTarget::BlockingWorkersTab,
        ] {
            app.activate_selectable_mouse_row(target, 9);
        }

        assert_eq!(app.selected, 7);
    }

    #[test]
    fn double_clicking_a_runtime_task_opens_the_detail_tab() {
        let mut app = App::offline("capture.seismograph".into());
        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Runtime;
        }

        app.activate_selectable_mouse_row(ListTarget::RuntimeGroupTasks, 3);
        assert!(matches!(
            app.screen,
            Screen::Offline {
                tab: MonitorTab::Runtime,
                ..
            }
        ));
        app.activate_selectable_mouse_row(ListTarget::RuntimeGroupTasks, 3);

        assert!(matches!(
            app.screen,
            Screen::Offline {
                tab: MonitorTab::RuntimeTask,
                ..
            }
        ));
        assert_eq!(
            (app.runtime_view.task_selected, app.runtime_view.task_selected_explicitly),
            (3, true)
        );
    }

    #[test]
    fn metric_header_clicks_select_and_reverse_sorting() {
        let mut app = App::offline("capture.seismograph".into());
        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Allocations;
        }
        app.activate_mouse_row(ListTarget::AllocationHeader(super::super::data::AllocationSort::MaximumBytes), 0);
        assert_eq!(app.allocation_view.sort, super::super::data::AllocationSort::MaximumBytes);
        assert!(app.allocation_view.descending);
        app.activate_mouse_row(ListTarget::AllocationHeader(super::super::data::AllocationSort::MaximumBytes), 0);
        assert!(!app.allocation_view.descending);

        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Primitives;
        }
        app.activate_mouse_row(ListTarget::PrimitiveOperationHeader(super::super::data::PrimitiveSort::Objects), 0);
        assert_eq!(app.primitive_view.sort, super::super::data::PrimitiveSort::Objects);
        assert!(app.primitive_view.descending);
        app.activate_mouse_row(
            ListTarget::PrimitiveTypeHeader(super::super::app::PrimitiveTypeSort::Objects),
            0,
        );
        assert_eq!(
            app.primitive_view.type_sort,
            super::super::app::PrimitiveTypeSort::Objects
        );
        assert!(app.primitive_view.type_descending);
        app.activate_mouse_row(
            ListTarget::PrimitiveHotspotHeader(super::super::app::PrimitiveHotspotSort::Location),
            0,
        );
        assert_eq!(
            app.primitive_view.hotspot_sort,
            super::super::app::PrimitiveHotspotSort::Location
        );
        assert!(!app.primitive_view.hotspot_descending);

        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Threads;
        }
        app.activate_mouse_row(
            ListTarget::ThreadHeader(super::super::app::ThreadListSort::Overwritten),
            0,
        );
        assert_eq!(
            app.thread_view.thread_sort,
            super::super::app::ThreadListSort::Overwritten
        );
        assert!(app.thread_view.thread_descending);
        app.activate_mouse_row(
            ListTarget::ThreadOperationHeader(super::super::app::ThreadOperationSort::Events),
            0,
        );
        assert_eq!(
            app.thread_view.operation_sort,
            super::super::app::ThreadOperationSort::Events
        );
        assert!(app.thread_view.operation_descending);
        app.activate_mouse_row(
            ListTarget::ThreadParticipantHeader(super::super::app::ThreadParticipantSort::Events),
            0,
        );
        assert_eq!(
            app.thread_view.participant_sort,
            super::super::app::ThreadParticipantSort::Events
        );
        assert!(app.thread_view.participant_descending);
        app.activate_mouse_row(
            ListTarget::ThreadObjectHeader(super::super::app::ThreadObjectSort::Related),
            0,
        );
        assert_eq!(
            app.thread_view.object_sort,
            super::super::app::ThreadObjectSort::Related
        );
        assert!(app.thread_view.object_descending);

        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Runtime;
        }
        app.activate_mouse_row(
            ListTarget::RuntimeWorkerHeader(
                RuntimeWorkersFocus::Asynchronous,
                super::super::app::RuntimeWorkerSort::Polls,
            ),
            0,
        );
        assert_eq!(
            app.runtime_view.asynchronous_worker_sort,
            super::super::app::RuntimeWorkerSort::Polls
        );
        assert!(app.runtime_view.asynchronous_worker_descending);
        app.activate_mouse_row(
            ListTarget::RuntimeWorkerHeader(
                RuntimeWorkersFocus::Blocking,
                super::super::app::RuntimeWorkerSort::MaximumExecution,
            ),
            0,
        );
        assert_eq!(
            app.runtime_view.blocking_worker_sort,
            super::super::app::RuntimeWorkerSort::MaximumExecution
        );
        assert!(app.runtime_view.blocking_worker_descending);

        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Io;
        }
        app.activate_mouse_row(
            ListTarget::IoResourceHeader(super::super::app::IoResourceSort::Errors),
            0,
        );
        assert_eq!(
            app.io_view.resource_sort,
            super::super::app::IoResourceSort::Errors
        );
        assert!(app.io_view.resource_descending);
        app.activate_mouse_row(
            ListTarget::IoOperationHeader(super::super::app::IoOperationSort::Outcome),
            0,
        );
        assert_eq!(
            app.io_view.operation_sort,
            super::super::app::IoOperationSort::Outcome
        );
        assert!(app.io_view.operation_descending);

        if let Screen::Offline { tab, .. } = &mut app.screen {
            *tab = MonitorTab::Cache;
        }
        app.activate_mouse_row(
            ListTarget::CacheTierHeader(super::super::app::CacheTierSort::HitRate),
            0,
        );
        assert_eq!(
            app.cache_view.tier_sort,
            super::super::app::CacheTierSort::HitRate
        );
        assert!(app.cache_view.tier_descending);
        app.activate_mouse_row(
            ListTarget::CacheOperationHeader(super::super::app::CacheOperationSort::Outcome),
            0,
        );
        assert_eq!(
            app.cache_view.operation_sort,
            super::super::app::CacheOperationSort::Outcome
        );
        assert!(app.cache_view.operation_descending);
        app.activate_mouse_row(
            ListTarget::CacheOperationHeader(super::super::app::CacheOperationSort::Outcome),
            0,
        );
        assert!(!app.cache_view.operation_descending);
    }
}
